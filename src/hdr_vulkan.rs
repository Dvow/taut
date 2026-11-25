//! HDR through Vulkan on Windows (the feature `hdr`, see `hdr` and `hdr_windows`): a window lit
//! on an HDR screen presented in scRGB, as Direct3D's are, or in HDR10 where the GPU offers no
//! scRGB.
//!
//! - The swap chain is made again in `R16G16B16A16_SFLOAT` and
//!   `VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR_EXT`, or `A2B10G10R10_UNORM` and `HDR10_ST2084`
//!   (`VK_EXT_swapchain_colorspace`), and its HDR metadata set where the device has
//!   `VK_EXT_hdr_metadata`; back to `B8G8R8A8_UNORM` once nothing is lit.
//! - The window's image reaches the swap chain's through a render pass, not a copy: the SDR
//!   colour made linear, offset, times each colour's gain (capped at the peak), offset back,
//!   times SDR white; for HDR10 in BT.2020's primaries and PQ. The gains are an image
//!   (`R16G16B16A16_SFLOAT`) the plane's changed part goes up to.
//! - The shaders are SPIR-V, compiled from the GLSL here (`glslc -O`, shaderc 2026.3).
use crate::hdr::{Gains, Screen};
use crate::layer_vulkan::{
    VkDescriptorImageInfo, VkDescriptorPoolCreateInfo, VkDescriptorPoolSize, VkDescriptorSetAllocateInfo, VkDescriptorSetLayoutBinding, VkDescriptorSetLayoutCreateInfo, VkGraphicsPipelineCreateInfo,
    VkPipelineColorBlendAttachmentState, VkPipelineColorBlendStateCreateInfo, VkPipelineDynamicStateCreateInfo, VkPipelineInputAssemblyStateCreateInfo, VkPipelineLayoutCreateInfo,
    VkPipelineMultisampleStateCreateInfo, VkPipelineRasterizationStateCreateInfo, VkPipelineShaderStageCreateInfo, VkPipelineVertexInputStateCreateInfo, VkPipelineViewportStateCreateInfo,
    VkShaderModuleCreateInfo, VkWriteDescriptorSet,
};
use crate::vk::*;
use std::ffi::{CStr, c_void};
use std::ptr::{null, null_mut};

functions!(Calls {
    vkCreateImage: fn(Object, *const VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkCreateImageView: fn(Object, *const VkImageViewCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImageView: fn(Object, Handle, *const c_void);
    vkCreateBuffer: fn(Object, *const VkBufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyBuffer: fn(Object, Handle, *const c_void);
    vkGetBufferMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkAllocateMemory: fn(Object, *const VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
    vkCreateSampler: fn(Object, *const VkSamplerCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroySampler: fn(Object, Handle, *const c_void);
    vkCreateDescriptorSetLayout: fn(Object, *const VkDescriptorSetLayoutCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyDescriptorSetLayout: fn(Object, Handle, *const c_void);
    vkCreateDescriptorPool: fn(Object, *const VkDescriptorPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyDescriptorPool: fn(Object, Handle, *const c_void);
    vkAllocateDescriptorSets: fn(Object, *const VkDescriptorSetAllocateInfo, *mut Handle) -> i32;
    vkUpdateDescriptorSets: fn(Object, u32, *const VkWriteDescriptorSet, u32, *const c_void);
    vkCreatePipelineLayout: fn(Object, *const VkPipelineLayoutCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyPipelineLayout: fn(Object, Handle, *const c_void);
    vkCreateShaderModule: fn(Object, *const VkShaderModuleCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyShaderModule: fn(Object, Handle, *const c_void);
    vkCreateGraphicsPipelines: fn(Object, Handle, u32, *const VkGraphicsPipelineCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyPipeline: fn(Object, Handle, *const c_void);
    vkCreateRenderPass: fn(Object, *const VkRenderPassCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyRenderPass: fn(Object, Handle, *const c_void);
    vkCreateFramebuffer: fn(Object, *const VkFramebufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFramebuffer: fn(Object, Handle, *const c_void);
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdBeginRenderPass: fn(Object, *const VkRenderPassBeginInfo, u32);
    vkCmdEndRenderPass: fn(Object);
    vkCmdBindPipeline: fn(Object, u32, Handle);
    vkCmdBindDescriptorSets: fn(Object, u32, Handle, u32, u32, *const Handle, u32, *const u32);
    vkCmdPushConstants: fn(Object, Handle, u32, u32, u32, *const c_void);
    vkCmdSetViewport: fn(Object, u32, u32, *const VkViewport);
    vkCmdSetScissor: fn(Object, u32, u32, *const VkRect2D);
    vkCmdDraw: fn(Object, u32, u32, u32, u32);
});

#[cfg(windows)]
functions!(Metadata {
    vkSetHdrMetadataEXT: fn(Object, u32, *const Handle, *const VkHdrMetadataEXT);
});

structures! {
    VkPushConstantRange { stages: u32, offset: u32, size: u32, }
    VkSubpassDependency { source: u32, destination: u32, source_stage: u32, destination_stage: u32, source_access: u32, destination_access: u32, flags: u32, }
}

#[cfg(windows)]
structures! {
    VkXYColorEXT { x: f32, y: f32, }
    VkHdrMetadataEXT { kind: u32, next: *const c_void, red: VkXYColorEXT, green: VkXYColorEXT, blue: VkXYColorEXT, white: VkXYColorEXT, most_luminance: f32, least_luminance: f32, most_content_light: f32, most_frame_average_light: f32, }
}

/// The gains' format: four 16-bit floats.
const HALVES: u32 = 97;
/// HDR10's format: 10 bits of red, green and blue, 2 of alpha.
const TEN_BITS: u32 = 64;
/// scRGB's colour space, and HDR10's.
const EXTENDED_SRGB_LINEAR: u32 = 1_000_104_002;
const HDR10_ST2084: u32 = 1_000_104_008;

/// How a swap chain shows HDR: its format and colour space, and how the pass writes its light
/// (0 linear, 1 PQ in BT.2020's primaries: see `LIGHT_GLSL`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Kind {
    pub(crate) format: u32,
    pub(crate) space: u32,
    signal: f32,
}

/// scRGB: linear, BT.709's primaries, 1.0 at 80 nits.
pub(crate) const SCRGB: Kind = Kind { format: HALVES, space: EXTENDED_SRGB_LINEAR, signal: 0.0 };
/// HDR10: PQ, BT.2020's primaries.
pub(crate) const HDR10: Kind = Kind { format: TEN_BITS, space: HDR10_ST2084, signal: 1.0 };

/// The HDR kind a surface offering `formats` takes: scRGB where it is offered, else HDR10.
pub(crate) fn kind_of(formats: &[VkSurfaceFormatKHR]) -> Option<Kind> {
    let offered = |kind: &&Kind| formats.iter().any(|format| format.format == kind.format && format.colour_space == kind.space);
    #[cfg(test)]
    let kinds = if tests::HDR10_FIRST.get() { [HDR10, SCRGB] } else { [SCRGB, HDR10] };
    #[cfg(not(test))]
    let kinds = [SCRGB, HDR10];
    kinds.iter().find(offered).copied()
}

/// The pass's constants for a screen: SDR white (scRGB: nits over 80; HDR10: nits over
/// 10,000), the most a gain may be, and the kind's signal.
pub(crate) fn constants(kind: Kind, screen: Screen) -> [f32; 3] {
    let white = if kind == HDR10 { screen.white / 10_000.0 } else { screen.white / 80.0 };
    [white, screen.headroom(), kind.signal]
}

/// The vertex shader: one triangle over the whole target.
#[cfg(test)]
const CORNERS_GLSL: &str = "#version 450
void main() {
    vec2 at = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(at * 2.0 - 1.0, 0.0, 1.0);
}
";

/// The pixel shader: the window's pixel lit by its gains (red, green, blue, and a gain map's
/// offset; 0 where none). `signal`: 0 linear (scRGB); 1 PQ in BT.2020's primaries (HDR10).
#[cfg(test)]
const LIGHT_GLSL: &str = "#version 450
layout(binding = 0) uniform sampler2D picture;
layout(binding = 1) uniform sampler2D gains;
layout(push_constant) uniform Light { float white; float most; float signal; } light;
layout(location = 0) out vec4 shown;
vec3 pq(vec3 part) {
    vec3 raised = pow(clamp(part, 0.0, 1.0), vec3(0.1593017578125));
    return pow((0.8359375 + 18.8515625 * raised) / (1.0 + 18.6875 * raised), vec3(78.84375));
}
void main() {
    ivec2 at = ivec2(gl_FragCoord.xy);
    vec3 c = texelFetch(picture, at, 0).rgb;
    vec3 sdr = mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), greaterThan(c, vec3(0.04045)));
    vec4 lit = texelFetch(gains, at, 0);
    vec3 gain = min(mix(vec3(1.0), lit.rgb, greaterThan(lit.rgb, vec3(0.0))), vec3(light.most));
    vec3 seen = max((sdr + lit.a) * gain - lit.a, 0.0) * light.white;
    mat3 bt2020 = mat3(0.6274, 0.0691, 0.0164, 0.3293, 0.9195, 0.0880, 0.0433, 0.0114, 0.8956);
    shown = vec4(light.signal > 0.5 ? pq(bt2020 * seen) : seen, 1.0);
}
";

/// `CORNERS_GLSL`, as `glslc -O --target-env=vulkan1.0` (shaderc 2026.3) compiles it.
const CORNERS: [u32; 213] = [
    0x07230203, 0x00010000, 0x000d000b, 0x0000002c, 0x00000000, 0x00020011, 0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
    0x0007000f, 0x00000000, 0x00000004, 0x6e69616d, 0x00000000, 0x0000000c, 0x0000001d, 0x00040047, 0x0000000c, 0x0000000b, 0x0000002a, 0x00030047, 0x0000001b, 0x00000002, 0x00050048, 0x0000001b,
    0x00000000, 0x0000000b, 0x00000000, 0x00050048, 0x0000001b, 0x00000001, 0x0000000b, 0x00000001, 0x00050048, 0x0000001b, 0x00000002, 0x0000000b, 0x00000003, 0x00050048, 0x0000001b, 0x00000003,
    0x0000000b, 0x00000004, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000002, 0x00040015, 0x0000000a,
    0x00000020, 0x00000001, 0x00040020, 0x0000000b, 0x00000001, 0x0000000a, 0x0004003b, 0x0000000b, 0x0000000c, 0x00000001, 0x0004002b, 0x0000000a, 0x0000000e, 0x00000001, 0x0004002b, 0x0000000a,
    0x00000010, 0x00000002, 0x00040017, 0x00000017, 0x00000006, 0x00000004, 0x00040015, 0x00000018, 0x00000020, 0x00000000, 0x0004002b, 0x00000018, 0x00000019, 0x00000001, 0x0004001c, 0x0000001a,
    0x00000006, 0x00000019, 0x0006001e, 0x0000001b, 0x00000017, 0x00000006, 0x0000001a, 0x0000001a, 0x00040020, 0x0000001c, 0x00000003, 0x0000001b, 0x0004003b, 0x0000001c, 0x0000001d, 0x00000003,
    0x0004002b, 0x0000000a, 0x0000001e, 0x00000000, 0x0004002b, 0x00000006, 0x00000020, 0x40000000, 0x0004002b, 0x00000006, 0x00000022, 0x3f800000, 0x0004002b, 0x00000006, 0x00000025, 0x00000000,
    0x00040020, 0x00000029, 0x00000003, 0x00000017, 0x0005002c, 0x00000007, 0x0000002b, 0x00000022, 0x00000022, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005,
    0x0004003d, 0x0000000a, 0x0000000d, 0x0000000c, 0x000500c4, 0x0000000a, 0x0000000f, 0x0000000d, 0x0000000e, 0x000500c7, 0x0000000a, 0x00000011, 0x0000000f, 0x00000010, 0x0004006f, 0x00000006,
    0x00000012, 0x00000011, 0x000500c7, 0x0000000a, 0x00000014, 0x0000000d, 0x00000010, 0x0004006f, 0x00000006, 0x00000015, 0x00000014, 0x00050050, 0x00000007, 0x00000016, 0x00000012, 0x00000015,
    0x0005008e, 0x00000007, 0x00000021, 0x00000016, 0x00000020, 0x00050083, 0x00000007, 0x00000024, 0x00000021, 0x0000002b, 0x00050051, 0x00000006, 0x00000026, 0x00000024, 0x00000000, 0x00050051,
    0x00000006, 0x00000027, 0x00000024, 0x00000001, 0x00070050, 0x00000017, 0x00000028, 0x00000026, 0x00000027, 0x00000025, 0x00000022, 0x00050041, 0x00000029, 0x0000002a, 0x0000001d, 0x0000001e,
    0x0003003e, 0x0000002a, 0x00000028, 0x000100fd, 0x00010038,
];

/// `LIGHT_GLSL`, as `glslc -O --target-env=vulkan1.0` (shaderc 2026.3) compiles it.
const LIGHT: [u32; 645] = [
    0x07230203, 0x00010000, 0x000d000b, 0x000000c4, 0x00000000, 0x00020011, 0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
    0x0007000f, 0x00000004, 0x00000004, 0x6e69616d, 0x00000000, 0x0000002e, 0x00000092, 0x00030010, 0x00000004, 0x00000007, 0x00040047, 0x0000002e, 0x0000000b, 0x0000000f, 0x00040047, 0x00000037,
    0x00000021, 0x00000000, 0x00040047, 0x00000037, 0x00000022, 0x00000000, 0x00040047, 0x00000056, 0x00000021, 0x00000001, 0x00040047, 0x00000056, 0x00000022, 0x00000000, 0x00030047, 0x00000064,
    0x00000002, 0x00050048, 0x00000064, 0x00000000, 0x00000023, 0x00000000, 0x00050048, 0x00000064, 0x00000001, 0x00000023, 0x00000004, 0x00050048, 0x00000064, 0x00000002, 0x00000023, 0x00000008,
    0x00040047, 0x00000092, 0x0000001e, 0x00000000, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000003,
    0x0004002b, 0x00000006, 0x0000000f, 0x00000000, 0x0004002b, 0x00000006, 0x00000010, 0x3f800000, 0x0004002b, 0x00000006, 0x00000014, 0x3e232000, 0x0006002c, 0x00000007, 0x00000015, 0x00000014,
    0x00000014, 0x00000014, 0x0004002b, 0x00000006, 0x00000017, 0x3f560000, 0x0004002b, 0x00000006, 0x00000018, 0x4196d000, 0x0004002b, 0x00000006, 0x0000001d, 0x41958000, 0x0004002b, 0x00000006,
    0x00000023, 0x429db000, 0x0006002c, 0x00000007, 0x00000024, 0x00000023, 0x00000023, 0x00000023, 0x00040015, 0x00000028, 0x00000020, 0x00000001, 0x00040017, 0x00000029, 0x00000028, 0x00000002,
    0x00040017, 0x0000002c, 0x00000006, 0x00000004, 0x00040020, 0x0000002d, 0x00000001, 0x0000002c, 0x0004003b, 0x0000002d, 0x0000002e, 0x00000001, 0x00040017, 0x0000002f, 0x00000006, 0x00000002,
    0x00090019, 0x00000034, 0x00000006, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x0003001b, 0x00000035, 0x00000034, 0x00040020, 0x00000036, 0x00000000, 0x00000035,
    0x0004003b, 0x00000036, 0x00000037, 0x00000000, 0x0004002b, 0x00000028, 0x0000003a, 0x00000000, 0x0004002b, 0x00000006, 0x00000044, 0x3d6147ae, 0x0004002b, 0x00000006, 0x0000004a, 0x4019999a,
    0x0006002c, 0x00000007, 0x0000004b, 0x0000004a, 0x0000004a, 0x0000004a, 0x0004002b, 0x00000006, 0x0000004e, 0x3d25aee6, 0x0006002c, 0x00000007, 0x0000004f, 0x0000004e, 0x0000004e, 0x0000004e,
    0x00020014, 0x00000050, 0x00040017, 0x00000051, 0x00000050, 0x00000003, 0x0004003b, 0x00000036, 0x00000056, 0x00000000, 0x0006002c, 0x00000007, 0x0000005c, 0x00000010, 0x00000010, 0x00000010,
    0x0006002c, 0x00000007, 0x00000061, 0x0000000f, 0x0000000f, 0x0000000f, 0x0005001e, 0x00000064, 0x00000006, 0x00000006, 0x00000006, 0x00040020, 0x00000065, 0x00000009, 0x00000064, 0x0004003b,
    0x00000065, 0x00000066, 0x00000009, 0x0004002b, 0x00000028, 0x00000067, 0x00000001, 0x00040020, 0x00000068, 0x00000009, 0x00000006, 0x00040018, 0x00000081, 0x00000007, 0x00000003, 0x0004002b,
    0x00000006, 0x00000084, 0x3f209d49, 0x0004002b, 0x00000006, 0x00000085, 0x3d8d844d, 0x0004002b, 0x00000006, 0x00000086, 0x3c86594b, 0x0006002c, 0x00000007, 0x00000087, 0x00000084, 0x00000085,
    0x00000086, 0x0004002b, 0x00000006, 0x00000088, 0x3ea89a02, 0x0004002b, 0x00000006, 0x00000089, 0x3f6b645a, 0x0004002b, 0x00000006, 0x0000008a, 0x3db43958, 0x0006002c, 0x00000007, 0x0000008b,
    0x00000088, 0x00000089, 0x0000008a, 0x0004002b, 0x00000006, 0x0000008c, 0x3d315b57, 0x0004002b, 0x00000006, 0x0000008d, 0x3c3ac711, 0x0004002b, 0x00000006, 0x0000008e, 0x3f65460b, 0x0006002c,
    0x00000007, 0x0000008f, 0x0000008c, 0x0000008d, 0x0000008e, 0x0006002c, 0x00000081, 0x00000090, 0x00000087, 0x0000008b, 0x0000008f, 0x00040020, 0x00000091, 0x00000003, 0x0000002c, 0x0004003b,
    0x00000091, 0x00000092, 0x00000003, 0x0004002b, 0x00000028, 0x00000093, 0x00000002, 0x0004002b, 0x00000006, 0x00000096, 0x3f000000, 0x0006002c, 0x00000007, 0x000000bd, 0x00000044, 0x00000044,
    0x00000044, 0x0006002c, 0x00000007, 0x000000bf, 0x00000017, 0x00000017, 0x00000017, 0x0004002b, 0x00000006, 0x000000c0, 0x3d9e8391, 0x0006002c, 0x00000007, 0x000000c1, 0x000000c0, 0x000000c0,
    0x000000c0, 0x0004002b, 0x00000006, 0x000000c2, 0x3f72a76f, 0x0006002c, 0x00000007, 0x000000c3, 0x000000c2, 0x000000c2, 0x000000c2, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003,
    0x000200f8, 0x00000005, 0x0004003d, 0x0000002c, 0x00000030, 0x0000002e, 0x0007004f, 0x0000002f, 0x00000031, 0x00000030, 0x00000030, 0x00000000, 0x00000001, 0x0004006e, 0x00000029, 0x00000032,
    0x00000031, 0x0004003d, 0x00000035, 0x00000038, 0x00000037, 0x00040064, 0x00000034, 0x0000003b, 0x00000038, 0x0007005f, 0x0000002c, 0x0000003c, 0x0000003b, 0x00000032, 0x00000002, 0x0000003a,
    0x0008004f, 0x00000007, 0x0000003d, 0x0000003c, 0x0000003c, 0x00000000, 0x00000001, 0x00000002, 0x00050085, 0x00000007, 0x00000042, 0x0000003d, 0x000000c1, 0x00050081, 0x00000007, 0x00000046,
    0x0000003d, 0x000000bd, 0x00050085, 0x00000007, 0x00000049, 0x00000046, 0x000000c3, 0x0007000c, 0x00000007, 0x0000004c, 0x00000001, 0x0000001a, 0x00000049, 0x0000004b, 0x000500ba, 0x00000051,
    0x00000052, 0x0000003d, 0x0000004f, 0x000600a9, 0x00000007, 0x00000053, 0x00000052, 0x0000004c, 0x00000042, 0x0004003d, 0x00000035, 0x00000057, 0x00000056, 0x00040064, 0x00000034, 0x00000059,
    0x00000057, 0x0007005f, 0x0000002c, 0x0000005a, 0x00000059, 0x00000032, 0x00000002, 0x0000003a, 0x0008004f, 0x00000007, 0x0000005e, 0x0000005a, 0x0000005a, 0x00000000, 0x00000001, 0x00000002,
    0x000500ba, 0x00000051, 0x00000062, 0x0000005e, 0x00000061, 0x000600a9, 0x00000007, 0x00000063, 0x00000062, 0x0000005e, 0x0000005c, 0x00050041, 0x00000068, 0x00000069, 0x00000066, 0x00000067,
    0x0004003d, 0x00000006, 0x0000006a, 0x00000069, 0x00060050, 0x00000007, 0x0000006b, 0x0000006a, 0x0000006a, 0x0000006a, 0x0007000c, 0x00000007, 0x0000006c, 0x00000001, 0x00000025, 0x00000063,
    0x0000006b, 0x00050051, 0x00000006, 0x00000073, 0x0000005a, 0x00000003, 0x00060050, 0x00000007, 0x00000074, 0x00000073, 0x00000073, 0x00000073, 0x00050081, 0x00000007, 0x00000075, 0x00000053,
    0x00000074, 0x00050085, 0x00000007, 0x00000077, 0x00000075, 0x0000006c, 0x00050083, 0x00000007, 0x0000007b, 0x00000077, 0x00000074, 0x0007000c, 0x00000007, 0x0000007d, 0x00000001, 0x00000028,
    0x0000007b, 0x00000061, 0x00050041, 0x00000068, 0x0000007e, 0x00000066, 0x0000003a, 0x0004003d, 0x00000006, 0x0000007f, 0x0000007e, 0x0005008e, 0x00000007, 0x00000080, 0x0000007d, 0x0000007f,
    0x00050041, 0x00000068, 0x00000094, 0x00000066, 0x00000093, 0x0004003d, 0x00000006, 0x00000095, 0x00000094, 0x000500ba, 0x00000050, 0x00000097, 0x00000095, 0x00000096, 0x000300f7, 0x0000009a,
    0x00000000, 0x000400fa, 0x00000097, 0x00000099, 0x000000a0, 0x000200f8, 0x00000099, 0x00050091, 0x00000007, 0x0000009d, 0x00000090, 0x00000080, 0x0008000c, 0x00000007, 0x000000ad, 0x00000001,
    0x0000002b, 0x0000009d, 0x00000061, 0x0000005c, 0x0007000c, 0x00000007, 0x000000ae, 0x00000001, 0x0000001a, 0x000000ad, 0x00000015, 0x0005008e, 0x00000007, 0x000000b0, 0x000000ae, 0x00000018,
    0x00050081, 0x00000007, 0x000000b2, 0x000000bf, 0x000000b0, 0x0005008e, 0x00000007, 0x000000b4, 0x000000ae, 0x0000001d, 0x00050081, 0x00000007, 0x000000b6, 0x0000005c, 0x000000b4, 0x00050088,
    0x00000007, 0x000000b7, 0x000000b2, 0x000000b6, 0x0007000c, 0x00000007, 0x000000b8, 0x00000001, 0x0000001a, 0x000000b7, 0x00000024, 0x000200f9, 0x0000009a, 0x000200f8, 0x000000a0, 0x000200f9,
    0x0000009a, 0x000200f8, 0x0000009a, 0x000700f5, 0x00000007, 0x000000bb, 0x000000b8, 0x00000099, 0x00000080, 0x000000a0, 0x00050051, 0x00000006, 0x000000a3, 0x000000bb, 0x00000000, 0x00050051,
    0x00000006, 0x000000a4, 0x000000bb, 0x00000001, 0x00050051, 0x00000006, 0x000000a5, 0x000000bb, 0x00000002, 0x00070050, 0x0000002c, 0x000000a6, 0x000000a3, 0x000000a4, 0x000000a5, 0x00000010,
    0x0003003e, 0x00000092, 0x000000a6, 0x000100fd, 0x00010038,
];

/// An image of the pass's: it, its memory and its view.
#[derive(Clone, Copy, Debug, Default)]
struct Image {
    image: Handle,
    memory: Handle,
    view: Handle,
}

/// The pass from a window's image to an HDR swap chain's: its functions, render pass, pipeline
/// and what it reads (a sampler, the descriptor set, which views it names); the gains' image
/// (`size` pixels, ready for shaders once `ready`), the buffer they go up through and the part
/// waiting there; and a framebuffer for each swap chain image.
pub(crate) struct Pass {
    calls: Calls,
    device: Object,
    local: u32,
    writable: u32,
    render_pass: Handle,
    set_layout: Handle,
    layout: Handle,
    pipeline: Handle,
    pool: Handle,
    set: Handle,
    sampler: Handle,
    named: [Handle; 2],
    picture_view: (Handle, Handle),
    gains: Image,
    upload: (Handle, Handle, *mut u8),
    size: [i32; 2],
    ready: bool,
    waiting: Option<[i32; 4]>,
    targets: Vec<(Handle, Handle)>,
}

// SAFETY: as its device: made and used on the windows' thread alone.
unsafe impl Send for Pass {}

impl Pass {
    /// The pass on `device`, its functions found by `find`, its memory as `memory` says, into
    /// swap chain images of `kind`; nothing where any part can't be made.
    pub(crate) fn new(find: &mut dyn FnMut(&CStr) -> *const c_void, device: Object, memory: &VkPhysicalDeviceMemoryProperties, kind: Kind) -> Option<Box<Pass>> {
        let calls = Calls::load(find).ok()?;
        let (local, writable) = (kinds(memory, DEVICE_LOCAL), kinds(memory, HOST_VISIBLE_AND_COHERENT));
        let mut made = Box::new(Pass {
            calls,
            device,
            local,
            writable,
            render_pass: 0,
            set_layout: 0,
            layout: 0,
            pipeline: 0,
            pool: 0,
            set: 0,
            sampler: 0,
            named: [0; 2],
            picture_view: (0, 0),
            gains: Image::default(),
            upload: (0, 0, null_mut()),
            size: [0; 2],
            ready: false,
            waiting: None,
            targets: Vec::new(),
        });
        made.render_pass = made.render_pass(kind.format)?;
        made.sampler = made.sampler()?;
        made.set = made.descriptor_set()?;
        made.pipeline = made.pipeline()?;
        Some(made)
    }

    /// The render pass writing a swap chain image whole, made ready to present; once the image
    /// is taken (the submit waits for it at the stage writing colours).
    fn render_pass(&self, format: u32) -> Option<Handle> {
        // Not loaded (2), stored (0); no stencil.
        let attachment =
            VkAttachmentDescription { format, samples: 1, load: 2, store: 0, stencil_load: 2, stencil_store: 1, initial_layout: LAYOUT_UNDEFINED, final_layout: LAYOUT_PRESENT, ..Default::default() };
        let colour = VkAttachmentReference { attachment: 0, layout: LAYOUT_COLOUR };
        let subpass = VkSubpassDescription { colour_count: 1, colours: &colour, ..Default::default() };
        let taken = VkSubpassDependency { source: !0, destination: 0, source_stage: STAGE_COLOUR, destination_stage: STAGE_COLOUR, destination_access: ACCESS_COLOUR, ..Default::default() };
        let asked = VkRenderPassCreateInfo {
            kind: 38,
            attachment_count: 1,
            attachments: &attachment,
            subpass_count: 1,
            subpasses: &subpass,
            dependency_count: 1,
            dependencies: (&raw const taken).cast(),
            ..Default::default()
        };
        let mut pass = 0;
        // SAFETY: the device is alive; the description and what it points to live through it.
        (unsafe { (self.calls.vkCreateRenderPass)(self.device, &asked, null(), &mut pass) } == 0).then_some(pass)
    }

    /// The sampler texels are fetched through: nearest.
    fn sampler(&self) -> Option<Handle> {
        let asked = VkSamplerCreateInfo { kind: 31, address: [2; 3], ..Default::default() };
        let mut sampler = 0;
        // SAFETY: as above.
        (unsafe { (self.calls.vkCreateSampler)(self.device, &asked, null(), &mut sampler) } == 0).then_some(sampler)
    }

    /// The descriptor set naming the window's image and the gains, its layout, pool and the
    /// pipeline's layout made with it (with twelve bytes of constants).
    fn descriptor_set(&mut self) -> Option<Handle> {
        let (calls, device) = (&self.calls, self.device);
        let binding = |at| VkDescriptorSetLayoutBinding { binding: at, descriptor_type: SAMPLED, count: 1, stages: 0x10, ..Default::default() };
        let (bindings, sizes) = ([binding(0), binding(1)], VkDescriptorPoolSize { descriptor_type: SAMPLED, count: 2 });
        let pushed = VkPushConstantRange { stages: 0x10, offset: 0, size: 12 };
        let mut set = 0;
        // SAFETY: the device is alive; each description is whole and what it points to lives
        // through the call. What is made is held by the pass.
        unsafe {
            let described = VkDescriptorSetLayoutCreateInfo { kind: 32, binding_count: 2, bindings: bindings.as_ptr(), ..Default::default() };
            (calls.vkCreateDescriptorSetLayout)(device, &described, null(), &mut self.set_layout);
            let pooled = VkDescriptorPoolCreateInfo { kind: 33, max_sets: 1, size_count: 1, sizes: &sizes, ..Default::default() };
            (calls.vkCreateDescriptorPool)(device, &pooled, null(), &mut self.pool);
            if self.set_layout == 0 || self.pool == 0 {
                return None;
            }
            let asked = VkDescriptorSetAllocateInfo { kind: 34, pool: self.pool, count: 1, layouts: &self.set_layout, ..Default::default() };
            let laid = VkPipelineLayoutCreateInfo { kind: 30, set_layout_count: 1, set_layouts: &self.set_layout, push_range_count: 1, push_ranges: (&raw const pushed).cast(), ..Default::default() };
            if (calls.vkAllocateDescriptorSets)(device, &asked, &mut set) != 0 || (calls.vkCreatePipelineLayout)(device, &laid, null(), &mut self.layout) != 0 {
                return None;
            }
        }
        Some(set)
    }

    /// The pipeline: one triangle over the target, nothing culled, not blended, the viewport
    /// and scissor its size (made per swap chain: see `targets`).
    fn pipeline(&self) -> Option<Handle> {
        let (calls, device) = (&self.calls, self.device);
        let module = |code: &[u32]| VkShaderModuleCreateInfo { kind: 16, size: code.len() * 4, code: code.as_ptr(), ..Default::default() };
        let (mut corners, mut lit, mut pipeline) = (0, 0, 0);
        let vertex_input = VkPipelineVertexInputStateCreateInfo { kind: 19, ..Default::default() };
        let input_assembly = VkPipelineInputAssemblyStateCreateInfo { kind: 20, topology: 3, ..Default::default() };
        let viewport = VkPipelineViewportStateCreateInfo { kind: 22, viewport_count: 1, scissor_count: 1, ..Default::default() };
        let rasterization = VkPipelineRasterizationStateCreateInfo { kind: 23, line_width: 1.0, ..Default::default() };
        let multisample = VkPipelineMultisampleStateCreateInfo { kind: 24, samples: 1, ..Default::default() };
        let blend = VkPipelineColorBlendAttachmentState { write_mask: 0xf, ..Default::default() };
        let colour_blend = VkPipelineColorBlendStateCreateInfo { kind: 26, attachment_count: 1, attachments: &blend, ..Default::default() };
        let states = [0u32, 1];
        let dynamic = VkPipelineDynamicStateCreateInfo { kind: 27, count: 2, states: states.as_ptr(), ..Default::default() };
        // SAFETY: the device is alive; everything each description points to lives through
        // the call; the modules go once the pipeline is made.
        unsafe {
            (calls.vkCreateShaderModule)(device, &module(&CORNERS), null(), &mut corners);
            (calls.vkCreateShaderModule)(device, &module(&LIGHT), null(), &mut lit);
            let stage = |stage, module| VkPipelineShaderStageCreateInfo { kind: 18, stage, module, name: c"main".as_ptr(), ..Default::default() };
            let stages = [stage(0x1, corners), stage(0x10, lit)];
            let described = VkGraphicsPipelineCreateInfo {
                kind: 28,
                stage_count: 2,
                stages: stages.as_ptr(),
                vertex_input: &vertex_input,
                input_assembly: &input_assembly,
                viewport: &viewport,
                rasterization: &rasterization,
                multisample: &multisample,
                colour_blend: &colour_blend,
                dynamic: &dynamic,
                layout: self.layout,
                render_pass: self.render_pass,
                base_index: -1,
                ..Default::default()
            };
            let made = corners != 0 && lit != 0 && (calls.vkCreateGraphicsPipelines)(device, 0, 1, &described, null(), &mut pipeline) == 0;
            (calls.vkDestroyShaderModule)(device, corners, null());
            (calls.vkDestroyShaderModule)(device, lit, null());
            made.then_some(pipeline)
        }
    }

    /// Memory for something needing `needs`, of the kinds in `kinds`.
    fn memory(&self, needs: VkMemoryRequirements, kinds: u32) -> Option<Handle> {
        let fitting = needs.type_bits & kinds;
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: fitting.trailing_zeros(), ..Default::default() };
        let mut memory = 0;
        // SAFETY: the device is alive; the description is whole.
        (fitting != 0 && unsafe { (self.calls.vkAllocateMemory)(self.device, &asked, null(), &mut memory) } == 0).then_some(memory)
    }

    /// A view of `image` (of `format`), all of its colour.
    fn view(&self, image: Handle, format: u32) -> Option<Handle> {
        let (seen, mut view) = (VkImageViewCreateInfo { kind: 15, image, view_type: 1, format, range: WHOLE, ..Default::default() }, 0);
        // SAFETY: the image is alive; the description is whole.
        (unsafe { (self.calls.vkCreateImageView)(self.device, &seen, null(), &mut view) } == 0).then_some(view)
    }

    /// Makes a view and framebuffer for each of a swap chain's `images` (of `format`, `extent`
    /// pixels), in place of the last swap chain's. Says whether all were made.
    pub(crate) fn target(&mut self, images: &[Handle], format: u32, extent: [u32; 2]) -> bool {
        self.forget_targets();
        for image in images {
            let Some(view) = self.view(*image, format) else { return false };
            let asked =
                VkFramebufferCreateInfo { kind: 37, render_pass: self.render_pass, attachment_count: 1, attachments: &view, width: extent[0], height: extent[1], layers: 1, ..Default::default() };
            let mut frame = 0;
            // SAFETY: the view is alive; the description is whole.
            unsafe { (self.calls.vkCreateFramebuffer)(self.device, &asked, null(), &mut frame) };
            self.targets.push((view, frame));
            if frame == 0 {
                return false;
            }
        }
        true
    }

    /// Lets go of the swap chain images' views and framebuffers. (The device is idle.)
    pub(crate) fn forget_targets(&mut self) {
        for (view, frame) in std::mem::take(&mut self.targets) {
            // SAFETY: the device is done with them; null handles are let be.
            unsafe {
                (self.calls.vkDestroyFramebuffer)(self.device, frame, null());
                (self.calls.vkDestroyImageView)(self.device, view, null());
            }
        }
    }

    /// Readies the gains for `record`: the part that changed written to the upload buffer (all
    /// of them, into a new image, where the window has another size). Says whether they are
    /// ready. (The device is done with both: each frame is waited for.)
    pub(crate) fn prepare(&mut self, gains: &mut Gains) -> bool {
        let size = gains.size;
        if size != self.size {
            self.let_go_of_gains();
            if !self.make_gains(size) {
                return false;
            }
            (self.size, self.ready, gains.changed) = (size, false, Some([0, 0, size[0], size[1]]));
        }
        self.waiting = gains.changed.take();
        let Some([left, top, right, bottom]) = self.waiting else { return true };
        let wide = size[0] as usize;
        for y in top..bottom {
            let from = &gains.plane[y as usize * wide + left as usize..][..(right - left) as usize];
            // SAFETY: the buffer holds the whole plane, laid out as it.
            unsafe { std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), self.upload.2.add((y as usize * wide + left as usize) * 8), from.len() * 8) };
        }
        true
    }

    /// Makes the gains' image `size` pixels, its view, and the buffer they go up through.
    fn make_gains(&mut self, size: [i32; 2]) -> bool {
        let (calls, device) = (&self.calls, self.device);
        let extent = [size[0] as u32, size[1] as u32, 1];
        let asked =
            VkImageCreateInfo { kind: 14, image_type: 1, format: HALVES, extent, mip_levels: 1, array_layers: 1, samples: 1, usage: USAGE_SAMPLED | USAGE_TRANSFER_DESTINATION, ..Default::default() };
        let bytes = VkBufferCreateInfo { kind: 12, size: size[0] as u64 * size[1] as u64 * 8, usage: USAGE_TRANSFER_SOURCE, ..Default::default() };
        let (mut image_needs, mut buffer_needs, mut at) = (VkMemoryRequirements::default(), VkMemoryRequirements::default(), null_mut());
        // SAFETY: the device is alive; each description is whole. What is made is held by the
        // pass, and let go of with it.
        unsafe {
            (calls.vkCreateImage)(device, &asked, null(), &mut self.gains.image);
            (calls.vkCreateBuffer)(device, &bytes, null(), &mut self.upload.0);
            if self.gains.image == 0 || self.upload.0 == 0 {
                return false;
            }
            (calls.vkGetImageMemoryRequirements)(device, self.gains.image, &mut image_needs);
            (calls.vkGetBufferMemoryRequirements)(device, self.upload.0, &mut buffer_needs);
        }
        (self.gains.memory, self.upload.1) = (self.memory(image_needs, self.local).unwrap_or(0), self.memory(buffer_needs, self.writable).unwrap_or(0));
        // SAFETY: as above; each is bound to memory made for it, the buffer mapped whole.
        let bound = unsafe {
            self.gains.memory != 0
                && self.upload.1 != 0
                && (calls.vkBindImageMemory)(device, self.gains.image, self.gains.memory, 0) == 0
                && (calls.vkBindBufferMemory)(device, self.upload.0, self.upload.1, 0) == 0
                && (calls.vkMapMemory)(device, self.upload.1, 0, u64::MAX, 0, &mut at) == 0
        };
        self.upload.2 = at.cast();
        self.gains.view = if bound { self.view(self.gains.image, HALVES).unwrap_or(0) } else { 0 };
        self.gains.view != 0
    }

    /// Lets go of the gains' image and buffer. (The device is idle.)
    fn let_go_of_gains(&mut self) {
        let (calls, device) = (&self.calls, self.device);
        let (gains, (buffer, memory, _)) = (std::mem::take(&mut self.gains), std::mem::replace(&mut self.upload, (0, 0, null_mut())));
        // SAFETY: the device is done with them; null handles are let be.
        unsafe {
            (calls.vkDestroyImageView)(device, gains.view, null());
            (calls.vkDestroyImage)(device, gains.image, null());
            (calls.vkFreeMemory)(device, gains.memory, null());
            (calls.vkDestroyBuffer)(device, buffer, null());
            (calls.vkFreeMemory)(device, memory, null());
        }
        self.size = [0; 2];
    }

    /// Names `picture` (the window's image, B8G8R8A8) and the gains in the descriptor set,
    /// where it names others. (The device is idle: each frame is waited for.)
    fn name(&mut self, picture: Handle) -> bool {
        if self.picture_view.0 != picture {
            // SAFETY: the device is done with the old view.
            unsafe { (self.calls.vkDestroyImageView)(self.device, self.picture_view.1, null()) };
            self.picture_view = (picture, self.view(picture, B8G8R8A8_UNORM).unwrap_or(0));
        }
        let views = [self.picture_view.1, self.gains.view];
        if views.contains(&0) || views == self.named {
            return !views.contains(&0);
        }
        let images = views.map(|view| VkDescriptorImageInfo { sampler: self.sampler, view, layout: LAYOUT_SHADER_READ_ONLY });
        let write = |at: usize| VkWriteDescriptorSet { kind: 35, set: self.set, binding: at as u32, count: 1, descriptor_type: SAMPLED, images: &images[at], ..Default::default() };
        let writes = [write(0), write(1)];
        // SAFETY: the set and views are alive; the writes point into `images`, alive here.
        unsafe { (self.calls.vkUpdateDescriptorSets)(self.device, 2, writes.as_ptr(), 0, null()) };
        self.named = views;
        true
    }

    /// Records into `commands` the gains waiting copied up, and `picture` (the window's image,
    /// copied into before, and in a layout to be copied from before and after) drawn lit into
    /// swap chain image `at`, `extent` pixels, with `constants`. Says whether it could.
    pub(crate) fn record(&mut self, commands: Object, picture: Handle, at: usize, extent: [u32; 2], constants: [f32; 3]) -> bool {
        let Some(&(_, frame)) = self.targets.get(at) else { return false };
        if !self.name(picture) {
            return false;
        }
        self.copy_waiting(commands);
        let to_read = barrier(picture, LAYOUT_TRANSFER_SOURCE, LAYOUT_SHADER_READ_ONLY, ACCESS_TRANSFER_WRITE, ACCESS_SHADER_READ);
        let back = barrier(picture, LAYOUT_SHADER_READ_ONLY, LAYOUT_TRANSFER_SOURCE, ACCESS_SHADER_READ, ACCESS_TRANSFER_WRITE);
        let area = VkRect2D { offset: [0; 2], extent };
        let viewport = VkViewport { width: extent[0] as f32, height: extent[1] as f32, max_depth: 1.0, ..Default::default() };
        let begun = VkRenderPassBeginInfo { kind: 43, render_pass: self.render_pass, framebuffer: frame, area, ..Default::default() };
        let calls = &self.calls;
        // SAFETY: the command buffer is being recorded; everything named is alive and laid out
        // as the barriers say.
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_FRAGMENT_SHADER, 0, 0, null(), 0, null(), 1, &to_read);
            (calls.vkCmdBeginRenderPass)(commands, &begun, 0);
            (calls.vkCmdBindPipeline)(commands, 0, self.pipeline);
            (calls.vkCmdBindDescriptorSets)(commands, 0, self.layout, 0, 1, &self.set, 0, null());
            (calls.vkCmdPushConstants)(commands, self.layout, 0x10, 0, 12, constants.as_ptr().cast());
            (calls.vkCmdSetViewport)(commands, 0, 1, &viewport);
            (calls.vkCmdSetScissor)(commands, 0, 1, &area);
            (calls.vkCmdDraw)(commands, 3, 1, 0, 0);
            (calls.vkCmdEndRenderPass)(commands);
            (calls.vkCmdPipelineBarrier)(commands, STAGE_FRAGMENT_SHADER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &back);
        }
        (self.ready, self.waiting) = (true, None);
        true
    }

    /// Records the waiting gains copied from the upload buffer into their image, left ready for
    /// shaders.
    fn copy_waiting(&self, commands: Object) {
        let Some([left, top, right, bottom]) = self.waiting else { return };
        let was = if self.ready { (LAYOUT_SHADER_READ_ONLY, ACCESS_SHADER_READ) } else { (LAYOUT_UNDEFINED, 0) };
        let to_copy = barrier(self.gains.image, was.0, LAYOUT_TRANSFER_DESTINATION, was.1, ACCESS_TRANSFER_WRITE);
        let to_read = barrier(self.gains.image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_SHADER_READ_ONLY, ACCESS_TRANSFER_WRITE, ACCESS_SHADER_READ);
        let place = VkBufferImageCopy {
            buffer_offset: (top as u64 * self.size[0] as u64 + left as u64) * 8,
            row_length: self.size[0] as u32,
            layers: LAYER,
            offset: [left, top, 0],
            extent: [(right - left) as u32, (bottom - top) as u32, 1],
            ..Default::default()
        };
        let calls = &self.calls;
        // SAFETY: the command buffer is being recorded; the place lies in the buffer and image.
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_FRAGMENT_SHADER | STAGE_TOP, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            (calls.vkCmdCopyBufferToImage)(commands, self.upload.0, self.gains.image, LAYOUT_TRANSFER_DESTINATION, 1, &place);
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_FRAGMENT_SHADER, 0, 0, null(), 0, null(), 1, &to_read);
        }
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        self.forget_targets();
        self.let_go_of_gains();
        let (calls, device) = (&self.calls, self.device);
        // SAFETY: the device is done with the pass (each frame is waited for); each is held
        // once, here; null handles are let be.
        unsafe {
            (calls.vkDestroyImageView)(device, self.picture_view.1, null());
            (calls.vkDestroyPipeline)(device, self.pipeline, null());
            (calls.vkDestroyPipelineLayout)(device, self.layout, null());
            (calls.vkDestroyDescriptorPool)(device, self.pool, null());
            (calls.vkDestroyDescriptorSetLayout)(device, self.set_layout, null());
            (calls.vkDestroySampler)(device, self.sampler, null());
            (calls.vkDestroyRenderPass)(device, self.render_pass, null());
        }
    }
}

/// The kinds of `memory` that have all of `wanted`, one bit each.
fn kinds(memory: &VkPhysicalDeviceMemoryProperties, wanted: u32) -> u32 {
    let types = memory.types.iter().take(memory.type_count as usize).enumerate();
    types.filter(|(_, kind)| kind.flags & wanted == wanted).fold(0, |all, (at, _)| all | 1 << at)
}

#[cfg(windows)]
/// Sets swap chain `chain`'s HDR metadata from `screen`, where the device has
/// `VK_EXT_hdr_metadata` (`metadata`): BT.709's primaries, its peak.
pub(crate) fn describe(metadata: &Metadata, device: Object, chain: Handle, screen: Screen) {
    let xy = |x, y| VkXYColorEXT { x, y };
    let said = VkHdrMetadataEXT {
        kind: 1_000_105_000,
        red: xy(0.64, 0.33),
        green: xy(0.30, 0.60),
        blue: xy(0.15, 0.06),
        white: xy(0.3127, 0.3290),
        most_luminance: screen.peak,
        least_luminance: 0.0,
        most_content_light: screen.peak,
        most_frame_average_light: screen.white,
        ..Default::default()
    };
    // SAFETY: the swap chain is the device's; the description is whole.
    unsafe { (metadata.vkSetHdrMetadataEXT)(device, 1, &chain, &said) };
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        /// A test asks for HDR10 where scRGB is offered too.
        pub(crate) static HDR10_FIRST: Cell<bool> = const { Cell::new(false) };
    }

    /// A swap chain image of `kind` (its words, as read back) as scRGB: red, green, blue and
    /// alpha, linear, BT.709's primaries, 1.0 at 80 nits.
    pub(crate) fn light_of(kind: Kind, words: &[u32]) -> Vec<[f32; 4]> {
        if kind == SCRGB {
            return words.chunks(2).map(|two| [two[0] as u16, (two[0] >> 16) as u16, two[1] as u16, (two[1] >> 16) as u16].map(crate::hdr::from_half)).collect();
        }
        let to_709 = crate::hdr::to_709(9).unwrap();
        let nits = |code: u32| {
            let raised = (code as f32 / 1023.0).powf(1.0 / 78.843_75);
            ((raised - 0.835_937_5).max(0.0) / (18.851_563 - 18.6875 * raised)).powf(1.0 / 0.159_301_76) * 10_000.0
        };
        let light = |word: &u32| {
            let wide = [nits(word & 0x3ff), nits(word >> 10 & 0x3ff), nits(word >> 20 & 0x3ff)];
            let [red, green, blue] = to_709.map(|row| (row[0] * wide[0] + row[1] * wide[1] + row[2] * wide[2]) / 80.0);
            [red, green, blue, (word >> 30) as f32 / 3.0]
        };
        words.iter().map(light).collect()
    }

    #[test]
    fn the_structures_are_laid_out_as_vulkan_lays_them_out() {
        assert_eq!(size_of::<VkSubpassDependency>(), 28);
        assert_eq!(size_of::<VkPushConstantRange>(), 12);
        #[cfg(windows)]
        assert_eq!((size_of::<VkHdrMetadataEXT>(), std::mem::offset_of!(VkHdrMetadataEXT, most_luminance)), (64, 48));
    }

    /// Draws `picture` (B8G8R8A8, `WIDE` pixels a row) lit by `gains` through a pass of `kind`
    /// on `bench`'s device, and reads the result back as scRGB.
    #[cfg(feature = "gpu-draw")]
    fn drawn(bench: &crate::gpu_draw_vulkan::tests::Bench, made: &crate::gpu_draw_vulkan::tests::Made, kind: Kind, picture: &[u32], gains: &mut Gains, screen: Screen) -> Vec<[f32; 4]> {
        const WIDE: u32 = 4;
        let (find, instance) = (bench.find, bench.instance);
        let mut pass = Pass::new(&mut |name| unsafe { find(instance, name.as_ptr()) }, bench.device, &bench.memory, kind).expect("the pass is made");
        let calls = Calls::load(&mut |name| unsafe { find(instance, name.as_ptr()) }).unwrap();
        let extent = [WIDE, picture.len() as u32 / WIDE];
        let image = |format, usage| {
            let asked = VkImageCreateInfo { kind: 14, image_type: 1, format, extent: [extent[0], extent[1], 1], mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
            let (mut image, mut needs) = (0, VkMemoryRequirements::default());
            unsafe {
                assert_eq!((calls.vkCreateImage)(bench.device, &asked, null(), &mut image), 0);
                (calls.vkGetImageMemoryRequirements)(bench.device, image, &mut needs);
                let memory = pass.memory(needs, pass.local).unwrap();
                (calls.vkBindImageMemory)(bench.device, image, memory, 0);
                (image, memory)
            }
        };
        let (window, target) = (image(B8G8R8A8_UNORM, USAGE_SAMPLED | USAGE_TRANSFER_DESTINATION), image(kind.format, USAGE_DRAWN_INTO | USAGE_TRANSFER_SOURCE));
        let made_buffer = VkBufferCreateInfo { kind: 12, size: 4096, usage: USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION, ..Default::default() };
        let (mut buffer, mut needs, mut at) = (0, VkMemoryRequirements::default(), null_mut());
        unsafe {
            (calls.vkCreateBuffer)(bench.device, &made_buffer, null(), &mut buffer);
            (calls.vkGetBufferMemoryRequirements)(bench.device, buffer, &mut needs);
            let memory = pass.memory(needs, pass.writable).unwrap();
            (calls.vkBindBufferMemory)(bench.device, buffer, memory, 0);
            (calls.vkMapMemory)(bench.device, memory, 0, u64::MAX, 0, &mut at);
            std::ptr::copy_nonoverlapping(picture.as_ptr(), at.cast::<u32>(), picture.len());
        }
        assert!(pass.target(&[target.0], kind.format, extent) && pass.prepare(gains));
        let whole = VkBufferImageCopy { row_length: extent[0], layers: LAYER, extent: [extent[0], extent[1], 1], ..Default::default() };
        unsafe {
            (made.vkBeginCommandBuffer)(bench.commands, &VkCommandBufferBeginInfo { kind: 42, flags: 1, ..Default::default() });
            let fill = barrier(window.0, LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
            (calls.vkCmdPipelineBarrier)(bench.commands, STAGE_TOP, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &fill);
            (calls.vkCmdCopyBufferToImage)(bench.commands, buffer, window.0, LAYOUT_TRANSFER_DESTINATION, 1, &whole);
            let filled = barrier(window.0, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_WRITE);
            (calls.vkCmdPipelineBarrier)(bench.commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &filled);
        }
        assert!(pass.record(bench.commands, window.0, 0, extent, constants(kind, screen)));
        unsafe {
            let to_read = barrier(target.0, LAYOUT_PRESENT, LAYOUT_TRANSFER_SOURCE, ACCESS_COLOUR, ACCESS_TRANSFER_READ);
            (calls.vkCmdPipelineBarrier)(bench.commands, STAGE_COLOUR, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
            (made.vkCmdCopyImageToBuffer)(bench.commands, target.0, LAYOUT_TRANSFER_SOURCE, buffer, 1, &whole);
            assert_eq!((made.vkEndCommandBuffer)(bench.commands), 0);
            let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &bench.commands, ..Default::default() };
            assert_eq!((made.vkQueueSubmit)(bench.queue, 1, &submit, bench.fence), 0);
            assert_eq!((made.vkWaitForFences)(bench.device, 1, &bench.fence, 1, 10_000_000_000), 0);
            (made.vkResetFences)(bench.device, 1, &bench.fence);
        }
        let words = if kind == SCRGB { 2 } else { 1 } * picture.len();
        let read = light_of(kind, unsafe { std::slice::from_raw_parts(at.cast::<u32>(), words) });
        unsafe {
            (made.vkDeviceWaitIdle)(bench.device);
            for (image, memory) in [window, target] {
                (calls.vkDestroyImage)(bench.device, image, null());
                (calls.vkFreeMemory)(bench.device, memory, null());
            }
            (calls.vkDestroyBuffer)(bench.device, buffer, null());
        }
        drop(pass);
        read
    }

    #[test]
    #[cfg(feature = "gpu-draw")]
    fn the_pass_lights_each_colour_by_its_gain_in_scrgb_and_hdr10() {
        use crate::hdr::half;
        let Some((bench, made)) = crate::gpu_draw_vulkan::tests::Bench::new() else { return println!("no Vulkan CPU device here: nothing to check") };
        // White, grey, a red, black; each lit its own way: not, twice, past the peak per colour,
        // and by a gain map with an offset.
        let picture = [0xffff_ffff, 0xff80_8080, 0xffc0_4020, 0xff00_0000];
        let gains = [[1.0, 1.0, 1.0, 0.0], [2.0, 2.0, 2.0, 0.0], [8.0, 2.0, 0.5, 0.0], [4.0, 4.0, 4.0, 0.25]];
        let screen = Screen { hdr: true, white: 200.0, peak: 1000.0 };
        let linear = |byte: u32| {
            let part = byte as f32 / 255.0;
            if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
        };
        for (kind, part) in [(SCRGB, 1.0 / 500.0), (HDR10, 1.0 / 50.0)] {
            let mut lit = Gains::default();
            (lit.size, lit.plane, lit.changed) = ([4, 1], gains.iter().map(|gain| gain.map(half)).collect(), Some([0, 0, 4, 1]));
            let shown = drawn(&bench, &made, kind, &picture, &mut lit, screen);
            for (at, (pixel, gain)) in picture.iter().zip(gains).enumerate() {
                let wanted: [f32; 3] = std::array::from_fn(|colour| ((linear(pixel >> (16 - 8 * colour) & 0xff) + gain[3]) * gain[colour].min(5.0) - gain[3]).max(0.0) * 2.5);
                let got = shown[at];
                assert!((0..3).all(|colour| (got[colour] - wanted[colour]).abs() <= wanted[colour] * part + part / 5.0), "{kind:?} at {at}: {got:?}, not {wanted:?}");
            }
            println!("{kind:?}: {shown:?}");
        }
        bench.finish(made);
    }

    #[test]
    fn scrgb_is_taken_where_offered_else_hdr10() {
        let format = |format, colour_space| VkSurfaceFormatKHR { format, colour_space };
        let both = [format(44, 0), format(HALVES, EXTENDED_SRGB_LINEAR), format(TEN_BITS, HDR10_ST2084)];
        assert_eq!(kind_of(&both), Some(SCRGB));
        assert_eq!(kind_of(&both[2..]), Some(HDR10));
        assert_eq!(kind_of(&[format(44, 0), format(TEN_BITS, 0)]), None);
        let screen = Screen { hdr: true, white: 200.0, peak: 1000.0 };
        assert_eq!(constants(SCRGB, screen), [2.5, 5.0, 0.0]);
        assert_eq!(constants(HDR10, screen), [0.02, 5.0, 1.0]);
    }

    /// The SPIR-V is what glslc makes of the GLSL above it. Checked where glslc of shaderc
    /// 2026.3 is on the path; on a difference, the words glslc made are written next to the
    /// sources and named.
    #[test]
    #[ignore = "needs glslc"]
    fn the_shaders_are_their_source_compiled() {
        let version = std::process::Command::new("glslc").arg("--version").output().map(|said| String::from_utf8_lossy(&said.stdout).into_owned());
        if !version.as_deref().unwrap_or_default().lines().next().unwrap_or_default().contains("2026.3") {
            return println!("no glslc of shaderc 2026.3 here: nothing to check ({version:?})");
        }
        let folder = std::env::temp_dir().join(format!("taut-hdr-spirv-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut differ = Vec::new();
        for (name, stage, source, wanted) in [("CORNERS", "vert", CORNERS_GLSL, &CORNERS[..]), ("LIGHT", "frag", LIGHT_GLSL, &LIGHT[..])] {
            let (source_file, code_file) = (folder.join(format!("{name}.{stage}")), folder.join(format!("{name}.spv")));
            std::fs::write(&source_file, source).unwrap();
            let done = std::process::Command::new("glslc").args(["-O", "--target-env=vulkan1.0", "-o"]).arg(&code_file).arg(&source_file).status().expect("glslc");
            assert!(done.success(), "{name} doesn't compile");
            let code: Vec<u32> = std::fs::read(&code_file).unwrap().chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
            if code != wanted {
                let lines: Vec<String> = code.chunks(16).map(|row| format!("    {},", row.iter().map(|word| format!("{word:#010x}")).collect::<Vec<_>>().join(", "))).collect();
                std::fs::write(folder.join(format!("{name}.rs")), format!("const {name}: [u32; {}] = [\n{}\n];\n", code.len(), lines.join("\n"))).unwrap();
                differ.push(name);
            }
        }
        assert!(differ.is_empty(), "{differ:?} differ: glslc's words are in {}", folder.display());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
