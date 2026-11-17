//! The UI over a program's own Vulkan rendering: made once from its device and queue, then drawn
//! each frame with one call, in the program's command buffer, inside its render pass.
//!
//! ```ignore
//! // SAFETY: the program's own handles, alive while the layer is.
//! let mut layer = unsafe { taut::layer_vulkan::Layer::new(&handles) }?;
//! // each frame, in the program's render pass, after its own drawing:
//! unsafe { layer.draw(&mut ui, commands, [0, 0, width, height]) }?;
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`), an image of the same size, and a buffer the
//! processor writes, laid out like the surface. `update` redraws only what changed, writes only
//! that to the buffer and copies only that to the image, with commands of its own submitted to
//! the program's queue: they come before the frame that draws it. `draw` updates, then records
//! one quad over the given area of the target: the UI at its top left, pixel for pixel, clipped
//! to it, with premultiplied alpha blending ("one, one minus source alpha"). Vulkan keeps no state
//! to restore: after `draw` the program binds its own pipeline, descriptor sets, viewport and
//! scissor again. A program that shows the UI otherwise calls `update` and samples `texture()`.
//!
//! Nothing is linked: the system's loader (vulkan-1.dll, libvulkan.so.1) is opened by name and
//! every function found through it. The shaders are SPIR-V, compiled from the GLSL beside them.
//! The target must store colour linearly (`..._UNORM`, not `..._SRGB`): the UI's pixels are
//! already in screen colours.
// Every function here is `#[inline]`: only a program that calls one compiles it. (Compiled in
// the library itself, even unused, this code moved how the rest is grouped and optimised.)
use crate::surface::Surface;
use crate::ui::Ui;
use crate::vk::{self, Handle, Object, VkBufferImageCopy, VkPhysicalDeviceMemoryProperties, VkRect2D, VkViewport};
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};

/// The program's Vulkan, as the layer is given it: its handles, as raw values.
#[derive(Clone, Copy)]
pub struct Handles {
    /// `VkInstance`.
    pub instance: *mut c_void,
    /// `VkPhysicalDevice`: the one the device is made on.
    pub physical_device: *mut c_void,
    /// `VkDevice`.
    pub device: *mut c_void,
    /// `VkQueue` that the program draws with: the layer submits its copies to it.
    pub queue: *mut c_void,
    /// The queue's family.
    pub queue_family: u32,
    /// `VkRenderPass` that the layer is drawn in (or one like it).
    pub render_pass: u64,
    /// The subpass of `render_pass`: one colour attachment, of one sample.
    pub subpass: u32,
}

impl std::fmt::Debug for Handles {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Handles { instance, physical_device, device, queue, queue_family, render_pass, subpass } = self;
        let mut shown = f.debug_struct("Handles");
        shown.field("instance", instance).field("physical_device", physical_device).field("device", device).field("queue", queue);
        shown.field("queue_family", queue_family).field("render_pass", render_pass).field("subpass", subpass).finish()
    }
}

/// A Vulkan error.
#[derive(Clone, PartialEq, Eq)]
pub struct Error {
    /// What the layer was doing, as "making the image".
    pub operation: &'static str,
    /// Vulkan's `VkResult`; 0 where a function was missing.
    pub code: i32,
    /// Vulkan's name for it, as "VK_ERROR_DEVICE_LOST"; or the function that was missing.
    pub message: String,
}

impl std::fmt::Debug for Error {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Error").field("operation", &self.operation).field("code", &self.code).field("message", &self.message).finish()
    }
}

impl std::fmt::Display for Error {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.operation, self.message)
    }
}

impl std::error::Error for Error {}

/// The failure of a function that this Vulkan does not have.
#[inline]
fn missing(name: &str) -> Error {
    Error { operation: "finding Vulkan's functions", code: 0, message: format!("this Vulkan has no {name}") }
}

/// `Err` with what was being done, if `result` is an error.
#[inline]
fn check(result: i32, doing: &'static str) -> Result<(), Error> {
    if result >= 0 { Ok(()) } else { Err(Error { operation: doing, code: result, message: vk::named(result).to_string() }) }
}

vk::structures! {
    VkDescriptorSetLayoutBinding { binding: u32, descriptor_type: u32, count: u32, stages: u32, samplers: *const Handle, }
    VkDescriptorSetLayoutCreateInfo { kind: u32, next: *const c_void, flags: u32, binding_count: u32, bindings: *const VkDescriptorSetLayoutBinding, }
    VkDescriptorPoolSize { descriptor_type: u32, count: u32, }
    VkDescriptorPoolCreateInfo { kind: u32, next: *const c_void, flags: u32, max_sets: u32, size_count: u32, sizes: *const VkDescriptorPoolSize, }
    VkDescriptorSetAllocateInfo { kind: u32, next: *const c_void, pool: Handle, count: u32, layouts: *const Handle, }
    VkDescriptorImageInfo { sampler: Handle, view: Handle, layout: u32, }
    VkWriteDescriptorSet { kind: u32, next: *const c_void, set: Handle, binding: u32, element: u32, count: u32, descriptor_type: u32, images: *const VkDescriptorImageInfo, buffers: *const c_void, texel_buffers: *const Handle, }
    VkPipelineLayoutCreateInfo { kind: u32, next: *const c_void, flags: u32, set_layout_count: u32, set_layouts: *const Handle, push_range_count: u32, push_ranges: *const c_void, }
    VkShaderModuleCreateInfo { kind: u32, next: *const c_void, flags: u32, size: usize, code: *const u32, }
    VkPipelineShaderStageCreateInfo { kind: u32, next: *const c_void, flags: u32, stage: u32, module: Handle, name: *const c_char, specialization: *const c_void, }
    VkPipelineVertexInputStateCreateInfo { kind: u32, next: *const c_void, flags: u32, binding_count: u32, bindings: *const c_void, attribute_count: u32, attributes: *const c_void, }
    VkPipelineInputAssemblyStateCreateInfo { kind: u32, next: *const c_void, flags: u32, topology: u32, restart: u32, }
    VkPipelineViewportStateCreateInfo { kind: u32, next: *const c_void, flags: u32, viewport_count: u32, viewports: *const c_void, scissor_count: u32, scissors: *const c_void, }
    VkPipelineRasterizationStateCreateInfo { kind: u32, next: *const c_void, flags: u32, depth_clamp: u32, discard: u32, polygon_mode: u32, cull_mode: u32, front_face: u32, depth_bias: u32, bias_constant: f32, bias_clamp: f32, bias_slope: f32, line_width: f32, }
    VkPipelineMultisampleStateCreateInfo { kind: u32, next: *const c_void, flags: u32, samples: u32, sample_shading: u32, min_sample_shading: f32, sample_mask: *const u32, alpha_to_coverage: u32, alpha_to_one: u32, }
    VkPipelineColorBlendAttachmentState { enable: u32, source: u32, destination: u32, operation: u32, source_alpha: u32, destination_alpha: u32, operation_alpha: u32, write_mask: u32, }
    VkPipelineColorBlendStateCreateInfo { kind: u32, next: *const c_void, flags: u32, logic_op_enable: u32, logic_op: u32, attachment_count: u32, attachments: *const VkPipelineColorBlendAttachmentState, constants: [f32; 4], }
    VkPipelineDynamicStateCreateInfo { kind: u32, next: *const c_void, flags: u32, count: u32, states: *const u32, }
    VkGraphicsPipelineCreateInfo { kind: u32, next: *const c_void, flags: u32, stage_count: u32, stages: *const VkPipelineShaderStageCreateInfo, vertex_input: *const VkPipelineVertexInputStateCreateInfo, input_assembly: *const VkPipelineInputAssemblyStateCreateInfo, tessellation: *const c_void, viewport: *const VkPipelineViewportStateCreateInfo, rasterization: *const VkPipelineRasterizationStateCreateInfo, multisample: *const VkPipelineMultisampleStateCreateInfo, depth_stencil: *const c_void, colour_blend: *const VkPipelineColorBlendStateCreateInfo, dynamic: *const VkPipelineDynamicStateCreateInfo, layout: Handle, render_pass: Handle, subpass: u32, base: Handle, base_index: i32, }
}

vk::functions!(Instance {
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkGetDeviceProcAddr: fn(Object, *const c_char) -> *const c_void;
});

vk::functions!(Device {
    vkCreateImage: fn(Object, *const vk::VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut vk::VkMemoryRequirements);
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkCreateImageView: fn(Object, *const vk::VkImageViewCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImageView: fn(Object, Handle, *const c_void);
    vkCreateBuffer: fn(Object, *const vk::VkBufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyBuffer: fn(Object, Handle, *const c_void);
    vkGetBufferMemoryRequirements: fn(Object, Handle, *mut vk::VkMemoryRequirements);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkAllocateMemory: fn(Object, *const vk::VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
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
    vkCreateCommandPool: fn(Object, *const vk::VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const vk::VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkBeginCommandBuffer: fn(Object, *const vk::VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCreateFence: fn(Object, *const vk::VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
    vkQueueSubmit: fn(Object, u32, *const vk::VkSubmitInfo, Handle) -> i32;
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const vk::VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdBindPipeline: fn(Object, u32, Handle);
    vkCmdSetViewport: fn(Object, u32, u32, *const VkViewport);
    vkCmdSetScissor: fn(Object, u32, u32, *const VkRect2D);
    vkCmdBindDescriptorSets: fn(Object, u32, Handle, u32, u32, *const Handle, u32, *const u32);
    vkCmdDraw: fn(Object, u32, u32, u32, u32);
});

/// A sampled image, in a descriptor set.
const SAMPLED_IMAGE: u32 = 2;

/// The vertex shader: the quad over the viewport, made from the numbers of its four corners (no
/// vertex buffer), and each corner's place in the image, from 0 to 1.
/// ```glsl
/// #version 450
/// layout(location = 0) out vec2 place;
/// void main() {
///     vec2 corner = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
///     place = corner;
///     gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
/// }
/// ```
/// As `glslc -O --target-env=vulkan1.0` (shaderc 2026.3) compiles it; `spirv-val` takes it.
const CORNERS: [u32; 220] = [
    0x07230203, 0x00010000, 0x000d000b, 0x0000002d, 0x00000000, 0x00020011, 0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
    0x0008000f, 0x00000000, 0x00000004, 0x6e69616d, 0x00000000, 0x0000000c, 0x00000016, 0x0000001e, 0x00040047, 0x0000000c, 0x0000000b, 0x0000002a, 0x00040047, 0x00000016, 0x0000001e, 0x00000000,
    0x00030047, 0x0000001c, 0x00000002, 0x00050048, 0x0000001c, 0x00000000, 0x0000000b, 0x00000000, 0x00050048, 0x0000001c, 0x00000001, 0x0000000b, 0x00000001, 0x00050048, 0x0000001c, 0x00000002,
    0x0000000b, 0x00000003, 0x00050048, 0x0000001c, 0x00000003, 0x0000000b, 0x00000004, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017,
    0x00000007, 0x00000006, 0x00000002, 0x00040015, 0x0000000a, 0x00000020, 0x00000001, 0x00040020, 0x0000000b, 0x00000001, 0x0000000a, 0x0004003b, 0x0000000b, 0x0000000c, 0x00000001, 0x0004002b,
    0x0000000a, 0x0000000e, 0x00000001, 0x00040020, 0x00000015, 0x00000003, 0x00000007, 0x0004003b, 0x00000015, 0x00000016, 0x00000003, 0x00040017, 0x00000018, 0x00000006, 0x00000004, 0x00040015,
    0x00000019, 0x00000020, 0x00000000, 0x0004002b, 0x00000019, 0x0000001a, 0x00000001, 0x0004001c, 0x0000001b, 0x00000006, 0x0000001a, 0x0006001e, 0x0000001c, 0x00000018, 0x00000006, 0x0000001b,
    0x0000001b, 0x00040020, 0x0000001d, 0x00000003, 0x0000001c, 0x0004003b, 0x0000001d, 0x0000001e, 0x00000003, 0x0004002b, 0x0000000a, 0x0000001f, 0x00000000, 0x0004002b, 0x00000006, 0x00000021,
    0x40000000, 0x0004002b, 0x00000006, 0x00000023, 0x3f800000, 0x0004002b, 0x00000006, 0x00000026, 0x00000000, 0x00040020, 0x0000002a, 0x00000003, 0x00000018, 0x0005002c, 0x00000007, 0x0000002c,
    0x00000023, 0x00000023, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0004003d, 0x0000000a, 0x0000000d, 0x0000000c, 0x000500c7, 0x0000000a, 0x0000000f,
    0x0000000d, 0x0000000e, 0x0004006f, 0x00000006, 0x00000010, 0x0000000f, 0x000500c3, 0x0000000a, 0x00000012, 0x0000000d, 0x0000000e, 0x0004006f, 0x00000006, 0x00000013, 0x00000012, 0x00050050,
    0x00000007, 0x00000014, 0x00000010, 0x00000013, 0x0003003e, 0x00000016, 0x00000014, 0x0005008e, 0x00000007, 0x00000022, 0x00000014, 0x00000021, 0x00050083, 0x00000007, 0x00000025, 0x00000022,
    0x0000002c, 0x00050051, 0x00000006, 0x00000027, 0x00000025, 0x00000000, 0x00050051, 0x00000006, 0x00000028, 0x00000025, 0x00000001, 0x00070050, 0x00000018, 0x00000029, 0x00000027, 0x00000028,
    0x00000026, 0x00000023, 0x00050041, 0x0000002a, 0x0000002b, 0x0000001e, 0x0000001f, 0x0003003e, 0x0000002b, 0x00000029, 0x000100fd, 0x00010038,
];

/// The fragment shader: each pixel the image's pixel at the same place, unfiltered (no sampler).
/// ```glsl
/// #version 450
/// #extension GL_EXT_samplerless_texture_functions : require
/// layout(set = 0, binding = 0) uniform texture2D picture;
/// layout(location = 0) in vec2 place;
/// layout(location = 0) out vec4 colour;
/// void main() {
///     colour = texelFetch(picture, ivec2(place * vec2(textureSize(picture, 0))), 0);
/// }
/// ```
const PIXELS: [u32; 154] = [
    0x07230203, 0x00010000, 0x000d000b, 0x0000001b, 0x00000000, 0x00020011, 0x00000001, 0x00020011, 0x00000032, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e,
    0x00000000, 0x00000001, 0x0007000f, 0x00000004, 0x00000004, 0x6e69616d, 0x00000000, 0x00000009, 0x00000010, 0x00030010, 0x00000004, 0x00000007, 0x00040047, 0x00000009, 0x0000001e, 0x00000000,
    0x00040047, 0x0000000c, 0x00000021, 0x00000000, 0x00040047, 0x0000000c, 0x00000022, 0x00000000, 0x00040047, 0x00000010, 0x0000001e, 0x00000000, 0x00020013, 0x00000002, 0x00030021, 0x00000003,
    0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000004, 0x00040020, 0x00000008, 0x00000003, 0x00000007, 0x0004003b, 0x00000008, 0x00000009, 0x00000003,
    0x00090019, 0x0000000a, 0x00000006, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00040020, 0x0000000b, 0x00000000, 0x0000000a, 0x0004003b, 0x0000000b, 0x0000000c,
    0x00000000, 0x00040017, 0x0000000e, 0x00000006, 0x00000002, 0x00040020, 0x0000000f, 0x00000001, 0x0000000e, 0x0004003b, 0x0000000f, 0x00000010, 0x00000001, 0x00040015, 0x00000013, 0x00000020,
    0x00000001, 0x0004002b, 0x00000013, 0x00000014, 0x00000000, 0x00040017, 0x00000015, 0x00000013, 0x00000002, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005,
    0x0004003d, 0x0000000a, 0x0000000d, 0x0000000c, 0x0004003d, 0x0000000e, 0x00000011, 0x00000010, 0x0004003d, 0x0000000a, 0x00000012, 0x0000000c, 0x00050067, 0x00000015, 0x00000016, 0x00000012,
    0x00000014, 0x0004006f, 0x0000000e, 0x00000017, 0x00000016, 0x00050085, 0x0000000e, 0x00000018, 0x00000011, 0x00000017, 0x0004006e, 0x00000015, 0x00000019, 0x00000018, 0x0007005f, 0x00000007,
    0x0000001a, 0x0000000d, 0x00000019, 0x00000002, 0x00000014, 0x0003003e, 0x00000009, 0x0000001a, 0x000100fd, 0x00010038,
];

/// The UI in a program's Vulkan: its pixels, an image of their size, a buffer the processor
/// writes them to, and what copies and draws them.
pub struct Layer {
    vk: Device,
    device: Object,
    queue: Object,
    memory: VkPhysicalDeviceMemoryProperties,
    /// The layer's own commands, their pool, and the fence their submit signals.
    pool: Handle,
    commands: Object,
    fence: Handle,
    /// The fence is submitted and not yet waited for.
    busy: bool,
    set_layout: Handle,
    descriptors: Handle,
    set: Handle,
    pipeline_layout: Handle,
    pipeline: Handle,
    /// The image, its memory and its view; 0 while the UI has no size.
    image: [Handle; 3],
    /// The buffer and its memory, and where the processor writes it.
    buffer: [Handle; 2],
    written: *mut u32,
    /// The image has been copied to, and is laid out for shaders to read.
    filled: bool,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).finish_non_exhaustive()
    }
}

impl Layer {
    /// The layer for the program's device, drawn in its render pass.
    ///
    /// # Safety
    /// The handles are alive, and are what they are said to be. They stay alive while the
    /// layer is used.
    #[inline]
    pub unsafe fn new(handles: &Handles) -> Result<Layer, Error> {
        let find = vk::loader().ok_or(Error { operation: "finding Vulkan", code: 0, message: "this system has no Vulkan loader".into() })?;
        // SAFETY: the instance is alive (as the caller promises); the loader finds its functions.
        let instance = Instance::load(&mut |name| unsafe { find(handles.instance, name.as_ptr()) }).map_err(missing)?;
        let device = handles.device;
        // SAFETY: as above, for the device.
        let vk = Device::load(&mut |name| unsafe { (instance.vkGetDeviceProcAddr)(device, name.as_ptr()) }).map_err(missing)?;
        let mut memory = VkPhysicalDeviceMemoryProperties::default();
        // SAFETY: the physical device is alive.
        unsafe { (instance.vkGetPhysicalDeviceMemoryProperties)(handles.physical_device, &mut memory) };
        let mut layer = Layer {
            vk,
            device,
            queue: handles.queue,
            memory,
            pool: 0,
            commands: null_mut(),
            fence: 0,
            busy: false,
            set_layout: 0,
            descriptors: 0,
            set: 0,
            pipeline_layout: 0,
            pipeline: 0,
            image: [0; 3],
            buffer: [0; 2],
            written: null_mut(),
            filled: false,
            surface: Surface::default(),
        };
        // SAFETY: the device is alive; what was made before a failure is let go of.
        if let Err(failed) = unsafe { layer.make(handles) } {
            unsafe { layer.delete() };
            return Err(failed);
        }
        Ok(layer)
    }

    /// The layer's commands, its fence, its descriptor set, and its pipeline.
    ///
    /// # Safety
    /// The device is alive.
    #[inline]
    unsafe fn make(&mut self, handles: &Handles) -> Result<(), Error> {
        let (vk, device) = (&self.vk, self.device);
        let binding = VkDescriptorSetLayoutBinding { descriptor_type: SAMPLED_IMAGE, count: 1, stages: 0x10, ..Default::default() };
        let sizes = VkDescriptorPoolSize { descriptor_type: SAMPLED_IMAGE, count: 1 };
        // Commands made again each time they are recorded (0x2).
        let pooled = vk::VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family: handles.queue_family, ..Default::default() };
        // SAFETY: the device is alive; each description is whole, and what it points to lives
        // through the call.
        unsafe {
            check((vk.vkCreateCommandPool)(device, &pooled, null(), &mut self.pool), "making the command pool")?;
            let asked = vk::VkCommandBufferAllocateInfo { kind: 40, pool: self.pool, count: 1, ..Default::default() };
            check((vk.vkAllocateCommandBuffers)(device, &asked, &mut self.commands), "making the command buffer")?;
            check((vk.vkCreateFence)(device, &vk::VkFenceCreateInfo { kind: 8, ..Default::default() }, null(), &mut self.fence), "making the fence")?;
            let described = VkDescriptorSetLayoutCreateInfo { kind: 32, binding_count: 1, bindings: &binding, ..Default::default() };
            check((vk.vkCreateDescriptorSetLayout)(device, &described, null(), &mut self.set_layout), "making the descriptor set's layout")?;
            let pool = VkDescriptorPoolCreateInfo { kind: 33, max_sets: 1, size_count: 1, sizes: &sizes, ..Default::default() };
            check((vk.vkCreateDescriptorPool)(device, &pool, null(), &mut self.descriptors), "making the descriptor pool")?;
            let asked = VkDescriptorSetAllocateInfo { kind: 34, pool: self.descriptors, count: 1, layouts: &self.set_layout, ..Default::default() };
            check((vk.vkAllocateDescriptorSets)(device, &asked, &mut self.set), "making the descriptor set")?;
            let laid = VkPipelineLayoutCreateInfo { kind: 30, set_layout_count: 1, set_layouts: &self.set_layout, ..Default::default() };
            check((vk.vkCreatePipelineLayout)(device, &laid, null(), &mut self.pipeline_layout), "making the pipeline's layout")?;
            self.pipeline = pipeline(vk, device, self.pipeline_layout, handles)?;
        }
        Ok(())
    }

    /// Updates the image after a run: redraws what changed, and copies only that. (The first
    /// time, and after a resize, a new image is made and all of it copied.)
    ///
    /// # Safety
    /// The program's handles are alive, and its queue is not in use on another thread.
    #[inline]
    pub unsafe fn update(&mut self, ui: &mut Ui) -> Result<(), Error> {
        let size = (self.surface.width, self.surface.height);
        let mut changed = self.surface.update(ui);
        if (self.surface.width, self.surface.height) != size || self.image[0] == 0 {
            // SAFETY: as the caller promises.
            unsafe { self.make_image() }.inspect_err(|_| self.surface = Surface::default())?;
            changed = [[0, 0, self.surface.width, self.surface.height]].into_iter().filter(|_| self.image[0] != 0).collect();
        }
        if changed.is_empty() {
            return Ok(());
        }
        // SAFETY: as the caller promises; once the last copy is done, the buffer and the commands
        // are the layer's to change.
        unsafe { self.settle() }?;
        let width = self.surface.width;
        for &[left, top, right, bottom] in &changed {
            for row in top..bottom {
                let from = &self.surface.pixels[row * width + left..row * width + right];
                // SAFETY: the buffer holds as many pixels as the surface, and the row lies in it.
                unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), self.written.add(row * width + left), from.len()) };
            }
        }
        let place = |&[left, top, right, bottom]: &[usize; 4]| VkBufferImageCopy {
            buffer_offset: ((top * width + left) * 4) as u64,
            row_length: width as u32,
            image_height: 0,
            layers: [0x1, 0, 0, 1],
            offset: [left as i32, top as i32, 0],
            extent: [(right - left) as u32, (bottom - top) as u32, 1],
        };
        let places: Vec<VkBufferImageCopy> = changed.iter().map(place).collect();
        // SAFETY: as the caller promises.
        unsafe { self.copy(&places) }
    }

    /// Records the copies of `places` from the buffer to the image, and submits them.
    ///
    /// # Safety
    /// The device and queue are alive; the layer's commands are not in use.
    #[inline]
    unsafe fn copy(&mut self, places: &[VkBufferImageCopy]) -> Result<(), Error> {
        let (vk, commands, image) = (&self.vk, self.commands, self.image[0]);
        // An image never copied to holds nothing that matters yet; once copied to, shaders may
        // still be reading it.
        let before = if self.filled { (vk::LAYOUT_SHADER_READ_ONLY, vk::STAGE_FRAGMENT_SHADER) } else { (vk::LAYOUT_UNDEFINED, vk::STAGE_TOP) };
        let to_copy = vk::barrier(image, before.0, vk::LAYOUT_TRANSFER_DESTINATION, 0, vk::ACCESS_TRANSFER_WRITE);
        let to_read = vk::barrier(image, vk::LAYOUT_TRANSFER_DESTINATION, vk::LAYOUT_SHADER_READ_ONLY, vk::ACCESS_TRANSFER_WRITE, vk::ACCESS_SHADER_READ);
        let submit = vk::VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &self.commands, ..Default::default() };
        // SAFETY: the commands are recorded once and submitted (1: used once); the places lie in
        // the buffer and the image.
        unsafe {
            check((vk.vkBeginCommandBuffer)(commands, &vk::VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() }), "recording the copy")?;
            (vk.vkCmdPipelineBarrier)(commands, before.1, vk::STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            (vk.vkCmdCopyBufferToImage)(commands, self.buffer[0], image, vk::LAYOUT_TRANSFER_DESTINATION, places.len() as u32, places.as_ptr());
            (vk.vkCmdPipelineBarrier)(commands, vk::STAGE_TRANSFER, vk::STAGE_FRAGMENT_SHADER, 0, 0, null(), 0, null(), 1, &to_read);
            check((vk.vkEndCommandBuffer)(commands), "recording the copy")?;
            check((vk.vkQueueSubmit)(self.queue, 1, &submit, self.fence), "submitting the copy")?;
        }
        (self.busy, self.filled) = (true, true);
        Ok(())
    }

    /// Waits for the layer's last submit, if it is not done.
    ///
    /// # Safety
    /// The device is alive.
    #[inline]
    unsafe fn settle(&mut self) -> Result<(), Error> {
        if !std::mem::take(&mut self.busy) {
            return Ok(());
        }
        // SAFETY: the fence is the layer's, and was submitted.
        unsafe {
            check((self.vk.vkWaitForFences)(self.device, 1, &self.fence, 1, u64::MAX), "waiting for the copy")?;
            check((self.vk.vkResetFences)(self.device, 1, &self.fence), "waiting for the copy")
        }
    }

    /// An image of the surface's size, a buffer to match, and the descriptor set pointing at the
    /// image; none while the UI has no size. Everything submitted to the queue before is waited
    /// for first: the frames that drew the old image are done with it.
    ///
    /// # Safety
    /// The device and queue are alive, and the queue is not in use on another thread.
    #[inline]
    unsafe fn make_image(&mut self) -> Result<(), Error> {
        // SAFETY: as the caller promises (a submit of nothing signals once all before it is done).
        unsafe {
            self.settle()?;
            if self.image[0] != 0 {
                check((self.vk.vkQueueSubmit)(self.queue, 0, null(), self.fence), "waiting for the frames before")?;
                self.busy = true;
                self.settle()?;
            }
            self.let_go_of_image();
        }
        if self.surface.pixels.is_empty() {
            return Ok(());
        }
        let (width, height) = (self.surface.width, self.surface.height);
        // The image is copied to and read by shaders (0x2, 0x4); the buffer copied from (0x1).
        let extent = [width as u32, height as u32, 1];
        let made = vk::VkImageCreateInfo { kind: 14, image_type: 1, format: vk::B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage: 0x2 | 0x4, ..Default::default() };
        let buffer = vk::VkBufferCreateInfo { kind: 12, size: (width * height * 4) as u64, usage: 0x1, ..Default::default() };
        let (vk, device) = (&self.vk, self.device);
        let mut needs = vk::VkMemoryRequirements::default();
        // SAFETY: the device is alive; each description is whole; each thing made is held here
        // and let go of with the image.
        unsafe {
            check((vk.vkCreateImage)(device, &made, null(), &mut self.image[0]), "making the image")?;
            (vk.vkGetImageMemoryRequirements)(device, self.image[0], &mut needs);
            self.image[1] = self.allocate(needs, vk::DEVICE_LOCAL, "making the image's memory")?;
            check((vk.vkBindImageMemory)(device, self.image[0], self.image[1], 0), "making the image's memory")?;
            let seen = vk::VkImageViewCreateInfo { kind: 15, image: self.image[0], view_type: 1, format: vk::B8G8R8A8_UNORM, range: vk::WHOLE, ..Default::default() };
            check((vk.vkCreateImageView)(device, &seen, null(), &mut self.image[2]), "making the image's view")?;
            check((vk.vkCreateBuffer)(device, &buffer, null(), &mut self.buffer[0]), "making the buffer")?;
            (vk.vkGetBufferMemoryRequirements)(device, self.buffer[0], &mut needs);
            self.buffer[1] = self.allocate(needs, vk::HOST_VISIBLE_AND_COHERENT, "making the buffer's memory")?;
            check((vk.vkBindBufferMemory)(device, self.buffer[0], self.buffer[1], 0), "making the buffer's memory")?;
            let mut there = null_mut();
            check((vk.vkMapMemory)(device, self.buffer[1], 0, u64::MAX, 0, &mut there), "mapping the buffer")?;
            self.written = there.cast();
            let image = VkDescriptorImageInfo { sampler: 0, view: self.image[2], layout: vk::LAYOUT_SHADER_READ_ONLY };
            let write = VkWriteDescriptorSet { kind: 35, set: self.set, count: 1, descriptor_type: SAMPLED_IMAGE, images: &image, ..Default::default() };
            (vk.vkUpdateDescriptorSets)(device, 1, &write, 0, null());
        }
        Ok(())
    }

    /// Memory for something with `needs`, of a kind with all of `wanted`.
    ///
    /// # Safety
    /// The device is alive.
    #[inline]
    unsafe fn allocate(&self, needs: vk::VkMemoryRequirements, wanted: u32, doing: &'static str) -> Result<Handle, Error> {
        let kind = vk::memory_type(&self.memory, needs.type_bits, wanted).ok_or(Error { operation: doing, code: 0, message: "the device has no memory of the kind needed".into() })?;
        let asked = vk::VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: kind, ..Default::default() };
        let mut memory = 0;
        // SAFETY: the device is alive, and the description whole.
        check(unsafe { (self.vk.vkAllocateMemory)(self.device, &asked, null(), &mut memory) }, doing)?;
        Ok(memory)
    }

    /// Lets go of the image, the buffer and their memory. (The device is done with them.)
    ///
    /// # Safety
    /// The device is alive.
    #[inline]
    unsafe fn let_go_of_image(&mut self) {
        let (vk, device, [image, image_memory, view], [buffer, buffer_memory]) = (&self.vk, self.device, self.image, self.buffer);
        // SAFETY: each is the layer's (a handle of 0 is nothing, which Vulkan lets go of too).
        unsafe {
            (vk.vkDestroyImageView)(device, view, null());
            (vk.vkDestroyImage)(device, image, null());
            (vk.vkFreeMemory)(device, image_memory, null());
            (vk.vkDestroyBuffer)(device, buffer, null());
            (vk.vkFreeMemory)(device, buffer_memory, null());
        }
        (self.image, self.buffer, self.written, self.filled) = ([0; 3], [0; 2], null_mut(), false);
    }

    /// Updates the image (`update`) and records it drawn into `commands`, inside the program's
    /// render pass: in `area` of the target (left, top, width, height, in pixels from its top
    /// left), the UI at the area's top left, pixel for pixel, clipped to the area. The program
    /// binds its own pipeline, descriptor sets, viewport and scissor again after.
    ///
    /// # Safety
    /// As `update`; and `commands` is the program's `VkCommandBuffer`, recording, inside a
    /// render pass like the layer's, submitted later to the layer's queue.
    #[inline]
    pub unsafe fn draw(&mut self, ui: &mut Ui, commands: *mut c_void, area: [i32; 4]) -> Result<(), Error> {
        // SAFETY: as the caller promises.
        unsafe { self.update(ui) }?;
        if !self.filled {
            return Ok(());
        }
        let [x, y, width, height] = area;
        let viewport = VkViewport { x: x as f32, y: y as f32, width: self.surface.width as f32, height: self.surface.height as f32, min_depth: 0.0, max_depth: 1.0 };
        let (left, top) = (x.max(0), y.max(0));
        let extent = [(x + width - left).max(0) as u32, (y + height - top).max(0) as u32];
        let scissor = VkRect2D { offset: [left, top], extent };
        let vk = &self.vk;
        // SAFETY: as the caller promises; the pipeline, its layout and the set are the layer's.
        unsafe {
            (vk.vkCmdBindPipeline)(commands, 0, self.pipeline);
            (vk.vkCmdSetViewport)(commands, 0, 1, &viewport);
            (vk.vkCmdSetScissor)(commands, 0, 1, &scissor);
            (vk.vkCmdBindDescriptorSets)(commands, 0, self.pipeline_layout, 0, 1, &self.set, 0, null());
            (vk.vkCmdDraw)(commands, 4, 1, 0, 0);
        }
        Ok(())
    }

    /// The image's view (`VkImageView`, `VK_FORMAT_B8G8R8A8_UNORM`, premultiplied alpha), laid
    /// out for shaders to read (`VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL`) once `update` has
    /// copied to it; 0 while the UI has no size. Owned by the layer.
    #[inline]
    pub fn texture(&self) -> u64 {
        self.image[2]
    }

    /// The image itself (`VkImage`), whose view `texture` is; 0 while the UI has no size.
    #[inline]
    pub fn image(&self) -> u64 {
        self.image[0]
    }

    /// The UI's pixels, which the image holds.
    #[inline]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Lets go of everything the layer made, once its last copy is done. (A layer only dropped
    /// leaves them to the device, which lets go of them when destroyed.)
    ///
    /// # Safety
    /// The device is alive, and done with every frame that drew the layer.
    #[inline]
    pub unsafe fn delete(mut self) {
        // SAFETY: as the caller promises; a lost device fails the wait, and is let go of anyway.
        unsafe {
            _ = self.settle();
            self.let_go_of_image();
            let (vk, device) = (&self.vk, self.device);
            (vk.vkDestroyPipeline)(device, self.pipeline, null());
            (vk.vkDestroyPipelineLayout)(device, self.pipeline_layout, null());
            (vk.vkDestroyDescriptorPool)(device, self.descriptors, null());
            (vk.vkDestroyDescriptorSetLayout)(device, self.set_layout, null());
            (vk.vkDestroyFence)(device, self.fence, null());
            (vk.vkDestroyCommandPool)(device, self.pool, null());
        }
    }
}

/// The pipeline that draws the layer, in the program's render pass: the quad as a strip of two
/// triangles, nothing culled, its colour blended as "one, one minus source alpha", the viewport
/// and scissor set when it is drawn.
///
/// # Safety
/// The device is alive, and the render pass is its.
#[inline]
unsafe fn pipeline(vk: &Device, device: Object, layout: Handle, handles: &Handles) -> Result<Handle, Error> {
    let (mut corners, mut pixels, mut pipeline) = (0, 0, 0);
    let module = |code: &[u32]| VkShaderModuleCreateInfo { kind: 16, size: code.len() * 4, code: code.as_ptr(), ..Default::default() };
    let stage = |stage, module| VkPipelineShaderStageCreateInfo { kind: 18, stage, module, name: c"main".as_ptr(), ..Default::default() };
    let vertex_input = VkPipelineVertexInputStateCreateInfo { kind: 19, ..Default::default() };
    let input_assembly = VkPipelineInputAssemblyStateCreateInfo { kind: 20, topology: 4, ..Default::default() };
    let viewport = VkPipelineViewportStateCreateInfo { kind: 22, viewport_count: 1, scissor_count: 1, ..Default::default() };
    let rasterization = VkPipelineRasterizationStateCreateInfo { kind: 23, line_width: 1.0, ..Default::default() };
    let multisample = VkPipelineMultisampleStateCreateInfo { kind: 24, samples: 1, ..Default::default() };
    // One (1), one minus source alpha (7), added (0), for colour and alpha alike; all four written.
    let blend = VkPipelineColorBlendAttachmentState { enable: 1, source: 1, destination: 7, operation: 0, source_alpha: 1, destination_alpha: 7, operation_alpha: 0, write_mask: 0xf };
    let colour_blend = VkPipelineColorBlendStateCreateInfo { kind: 26, attachment_count: 1, attachments: &blend, ..Default::default() };
    // The viewport (0) and the scissor (1), set when drawn.
    let states = [0u32, 1];
    let dynamic = VkPipelineDynamicStateCreateInfo { kind: 27, count: 2, states: states.as_ptr(), ..Default::default() };
    // SAFETY: the device is alive; everything each description points to lives through the call.
    let made = unsafe {
        check((vk.vkCreateShaderModule)(device, &module(&CORNERS), null(), &mut corners), "making the vertex shader")?;
        let made = check((vk.vkCreateShaderModule)(device, &module(&PIXELS), null(), &mut pixels), "making the fragment shader");
        let stages = [stage(0x1, corners), stage(0x10, pixels)];
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
            layout,
            render_pass: handles.render_pass,
            subpass: handles.subpass,
            base_index: -1,
            ..Default::default()
        };
        let made = made.and_then(|_| check((vk.vkCreateGraphicsPipelines)(device, 0, 1, &described, null(), &mut pipeline), "making the pipeline"));
        (vk.vkDestroyShaderModule)(device, corners, null());
        (vk.vkDestroyShaderModule)(device, pixels, null());
        made
    };
    made.map(|_| pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layer's own structures are as large as vulkan_core.h makes them on 64-bit systems.
    #[test]
    fn the_structures_are_laid_out_as_vulkan_lays_them_out() {
        assert_eq!(size_of::<VkDescriptorSetLayoutBinding>(), 24);
        assert_eq!(size_of::<VkDescriptorSetLayoutCreateInfo>(), 32);
        assert_eq!(size_of::<VkDescriptorPoolCreateInfo>(), 40);
        assert_eq!(size_of::<VkDescriptorSetAllocateInfo>(), 40);
        assert_eq!(size_of::<VkDescriptorImageInfo>(), 24);
        assert_eq!(size_of::<VkWriteDescriptorSet>(), 64);
        assert_eq!(size_of::<VkPipelineLayoutCreateInfo>(), 48);
        assert_eq!(size_of::<VkShaderModuleCreateInfo>(), 40);
        assert_eq!(size_of::<VkPipelineShaderStageCreateInfo>(), 48);
        assert_eq!(size_of::<VkPipelineVertexInputStateCreateInfo>(), 48);
        assert_eq!(size_of::<VkPipelineInputAssemblyStateCreateInfo>(), 32);
        assert_eq!(size_of::<VkPipelineViewportStateCreateInfo>(), 48);
        assert_eq!(size_of::<VkPipelineRasterizationStateCreateInfo>(), 64);
        assert_eq!(size_of::<VkPipelineMultisampleStateCreateInfo>(), 48);
        assert_eq!(size_of::<VkPipelineColorBlendAttachmentState>(), 32);
        assert_eq!(size_of::<VkPipelineColorBlendStateCreateInfo>(), 56);
        assert_eq!(size_of::<VkPipelineDynamicStateCreateInfo>(), 32);
        assert_eq!(size_of::<VkGraphicsPipelineCreateInfo>(), 144);
    }

    /// The SPIR-V is what glslc makes of the GLSL above it. Checked where glslc of shaderc
    /// 2026.3 is on the path (another version may lay out the same code differently).
    #[test]
    #[ignore = "needs glslc"]
    fn the_shaders_are_their_source_compiled() {
        let version = std::process::Command::new("glslc").arg("--version").output().map(|said| String::from_utf8_lossy(&said.stdout).into_owned());
        if !version.as_deref().unwrap_or_default().lines().next().unwrap_or_default().contains("2026.3") {
            return println!("no glslc of shaderc 2026.3 here: nothing to check ({version:?})");
        }
        let file = include_str!("layer_vulkan.rs").replace("\r\n", "\n");
        let folder = std::env::temp_dir().join(format!("taut-spirv-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for (stage, wanted, at) in [("vert", &CORNERS[..], 0), ("frag", &PIXELS[..], 1)] {
            let start = file.match_indices("/// ```glsl\n").nth(at).unwrap().0 + "/// ```glsl\n".len();
            let end = start + file[start..].find("/// ```\n").unwrap();
            let source: String = file[start..end].lines().map(|line| format!("{}\n", line.strip_prefix("/// ").unwrap_or(""))).collect();
            let (source_file, code_file) = (folder.join(format!("layer.{stage}")), folder.join(format!("layer.{stage}.spv")));
            std::fs::write(&source_file, source).unwrap();
            let done = std::process::Command::new("glslc").args(["-O", "--target-env=vulkan1.0", "-o"]).arg(&code_file).arg(&source_file).status().expect("glslc");
            assert!(done.success());
            let code: Vec<u32> = std::fs::read(&code_file).unwrap().chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
            assert_eq!(code, wanted, "the {stage} shader");
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// A Vulkan error says what was being done and Vulkan's name for it.
    #[test]
    fn what_went_wrong_is_said_in_vulkans_words() {
        assert_eq!(check(0, "anything"), Ok(()));
        let failed = check(-4, "submitting the copy").unwrap_err();
        assert_eq!(failed.to_string(), "submitting the copy failed: VK_ERROR_DEVICE_LOST");
        assert_eq!(failed.code, -4);
    }
}
