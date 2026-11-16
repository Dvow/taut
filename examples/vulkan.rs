//! taut under Vulkan. The program renders with Vulkan as usual, and the interface lies over it:
//! one image, drawn as one quad, its pixels blended over what is there. `Layer` is what a
//! program copies: a surface-sized image and a host-writable staging buffer with the surface's
//! layout; changed regions are written to the buffer and copied to the image (only those), with
//! layout transitions around the copy; and the image is drawn in the program's own render pass.
//!
//! Around it is a windowless program: it renders its own scene, the interface over it, changes
//! something and renders again, and writes the frame to a PNG file. (A windowed program has a
//! swap chain, whose images are drawn into the same way.)
//!   cargo run -p taut --example vulkan [<file.png>]
//!
//! Only the system's Vulkan loader (vulkan-1.dll, libvulkan.so.1), opened by name; the API used
//! is declared here as vulkan_core.h declares it. The two shaders are SPIR-V, compiled from the
//! GLSL beside them.
//!
//! Vulkan is explicit about everything: so this is by far the longest example, though it does
//! what the others do.
// The handles handed to the functions here (devices, contexts, command buffers) are the
// renderer's own objects, which the program got from it and hands back as it would to the
// renderer itself: they are trusted as the renderer trusts them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};
use taut::geom::Size;
use taut::text::TextSystem;
use taut::{Color, Event, Key, Length, LocalClipboard, Mods, Surface, Ui};

/// Declares a set of Vulkan's functions, found by name.
macro_rules! functions {
    ($set:ident { $($name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)* }) => {
        /// Functions of Vulkan's, found by name.
        #[allow(non_snake_case)]
        pub struct $set {
            $(pub $name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)*
        }
        impl $set {
            /// Finds each function with `find`, which is handed its name ending in a zero.
            pub fn load(find: &mut dyn FnMut(&str) -> *const c_void) -> Option<$set> {
                let mut found = |name: &str| Some(find(name)).filter(|found| !found.is_null());
                // SAFETY: each function is declared as Vulkan declares it.
                Some($set { $($name: unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn($($arg),*) $(-> $out)?>(found(concat!(stringify!($name), "\0"))?) },)* })
            }
        }
    };
}

/// Declares structures of Vulkan's, as vulkan_core.h lays them out. Each starts out all
/// zero, which says nothing in any of them: no pointers, no flags, nothing chained.
macro_rules! structures {
    ($($name:ident { $($field:ident: $type:ty,)* })*) => {$(
        #[repr(C)]
        #[derive(Clone, Copy)]
        #[allow(non_snake_case)]
        pub struct $name {
            $(pub $field: $type,)*
        }
        impl Default for $name {
            fn default() -> Self {
                // SAFETY: every field is a number or a pointer, for which zero is a value.
                unsafe { std::mem::zeroed() }
            }
        }
    )*};
}

/// A handle that is a number (an image, a buffer, a pipeline), not a pointer.
pub type Handle = u64;
/// A handle that is a pointer (the instance, the device, a queue, a command buffer).
pub type Object = *mut c_void;

structures! {
    VkImageCreateInfo { kind: u32, next: *const c_void, flags: u32, image_type: u32, format: u32, extent: [u32; 3], mip_levels: u32, array_layers: u32, samples: u32, tiling: u32, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, initial_layout: u32, }
    VkImageViewCreateInfo { kind: u32, next: *const c_void, flags: u32, image: Handle, view_type: u32, format: u32, components: [u32; 4], range: [u32; 5], }
    VkBufferCreateInfo { kind: u32, next: *const c_void, flags: u32, size: u64, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, }
    VkMemoryRequirements { size: u64, alignment: u64, type_bits: u32, }
    VkMemoryAllocateInfo { kind: u32, next: *const c_void, size: u64, type_index: u32, }
    VkMemoryType { flags: u32, heap: u32, }
    VkMemoryHeap { size: u64, flags: u32, }
    VkPhysicalDeviceMemoryProperties { type_count: u32, types: [VkMemoryType; 32], heap_count: u32, heaps: [VkMemoryHeap; 16], }
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
    VkBufferImageCopy { buffer_offset: u64, row_length: u32, image_height: u32, layers: [u32; 4], offset: [i32; 3], extent: [u32; 3], }
    VkImageMemoryBarrier { kind: u32, next: *const c_void, source_access: u32, destination_access: u32, old_layout: u32, new_layout: u32, source_family: u32, destination_family: u32, image: Handle, range: [u32; 5], }
    VkViewport { x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32, }
    VkRect2D { offset: [i32; 2], extent: [u32; 2], }
}

functions!(Vk {
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
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
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdBindPipeline: fn(Object, u32, Handle);
    vkCmdSetViewport: fn(Object, u32, u32, *const VkViewport);
    vkCmdSetScissor: fn(Object, u32, u32, *const VkRect2D);
    vkCmdBindDescriptorSets: fn(Object, u32, Handle, u32, u32, *const Handle, u32, *const u32);
    vkCmdDraw: fn(Object, u32, u32, u32, u32);
});

// Vulkan's numbers for what is said here, as vulkan_core.h has them.
pub const B8G8R8A8_UNORM: u32 = 44;
const LAYOUT_UNDEFINED: u32 = 0;
const LAYOUT_SHADER_READ_ONLY: u32 = 5;
const LAYOUT_TRANSFER_DESTINATION: u32 = 7;
const ACCESS_SHADER_READ: u32 = 0x20;
const ACCESS_TRANSFER_WRITE: u32 = 0x1000;
const STAGE_TOP: u32 = 0x1;
const STAGE_FRAGMENT_SHADER: u32 = 0x80;
const STAGE_TRANSFER: u32 = 0x1000;
pub const DEVICE_LOCAL: u32 = 0x1;
pub const HOST_VISIBLE_AND_COHERENT: u32 = 0x2 | 0x4;
const SAMPLED_IMAGE: u32 = 2;
const GRAPHICS: u32 = 0;
/// All of an image's colour: aspect, first level, levels, first layer, layers.
pub const WHOLE: [u32; 5] = [0x1, 0, 1, 0, 1];

/// The vertex shader: the quad, made from the numbers of its four corners, so there is no
/// buffer of corners.
/// ```glsl
/// #version 450
/// void main() {
///     vec2 corner = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
///     gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
/// }
/// ```
/// As `glslc -O --target-env=vulkan1.0` (shaderc 2026.3) compiles it; `spirv-val` takes it.
const CORNERS: [u32; 204] = [
    0x07230203, 0x00010000, 0x000d000b, 0x0000002a, 0x00000000, 0x00020011, 0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
    0x0007000f, 0x00000000, 0x00000004, 0x6e69616d, 0x00000000, 0x0000000c, 0x0000001b, 0x00040047, 0x0000000c, 0x0000000b, 0x0000002a, 0x00030047, 0x00000019, 0x00000002, 0x00050048, 0x00000019,
    0x00000000, 0x0000000b, 0x00000000, 0x00050048, 0x00000019, 0x00000001, 0x0000000b, 0x00000001, 0x00050048, 0x00000019, 0x00000002, 0x0000000b, 0x00000003, 0x00050048, 0x00000019, 0x00000003,
    0x0000000b, 0x00000004, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000002, 0x00040015, 0x0000000a,
    0x00000020, 0x00000001, 0x00040020, 0x0000000b, 0x00000001, 0x0000000a, 0x0004003b, 0x0000000b, 0x0000000c, 0x00000001, 0x0004002b, 0x0000000a, 0x0000000e, 0x00000001, 0x00040017, 0x00000015,
    0x00000006, 0x00000004, 0x00040015, 0x00000016, 0x00000020, 0x00000000, 0x0004002b, 0x00000016, 0x00000017, 0x00000001, 0x0004001c, 0x00000018, 0x00000006, 0x00000017, 0x0006001e, 0x00000019,
    0x00000015, 0x00000006, 0x00000018, 0x00000018, 0x00040020, 0x0000001a, 0x00000003, 0x00000019, 0x0004003b, 0x0000001a, 0x0000001b, 0x00000003, 0x0004002b, 0x0000000a, 0x0000001c, 0x00000000,
    0x0004002b, 0x00000006, 0x0000001e, 0x40000000, 0x0004002b, 0x00000006, 0x00000020, 0x3f800000, 0x0004002b, 0x00000006, 0x00000023, 0x00000000, 0x00040020, 0x00000027, 0x00000003, 0x00000015,
    0x0005002c, 0x00000007, 0x00000029, 0x00000020, 0x00000020, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0004003d, 0x0000000a, 0x0000000d, 0x0000000c,
    0x000500c7, 0x0000000a, 0x0000000f, 0x0000000d, 0x0000000e, 0x0004006f, 0x00000006, 0x00000010, 0x0000000f, 0x000500c3, 0x0000000a, 0x00000012, 0x0000000d, 0x0000000e, 0x0004006f, 0x00000006,
    0x00000013, 0x00000012, 0x00050050, 0x00000007, 0x00000014, 0x00000010, 0x00000013, 0x0005008e, 0x00000007, 0x0000001f, 0x00000014, 0x0000001e, 0x00050083, 0x00000007, 0x00000022, 0x0000001f,
    0x00000029, 0x00050051, 0x00000006, 0x00000024, 0x00000022, 0x00000000, 0x00050051, 0x00000006, 0x00000025, 0x00000022, 0x00000001, 0x00070050, 0x00000015, 0x00000026, 0x00000024, 0x00000025,
    0x00000023, 0x00000020, 0x00050041, 0x00000027, 0x00000028, 0x0000001b, 0x0000001c, 0x0003003e, 0x00000028, 0x00000026, 0x000100fd, 0x00010038,
];

/// The fragment shader: each pixel is the image's pixel at the same place, read as it is
/// (Vulkan counts a pixel's place from the top left), with no sampler and no filtering.
/// ```glsl
/// #version 450
/// #extension GL_EXT_samplerless_texture_functions : require
/// layout(set = 0, binding = 0) uniform texture2D picture;
/// layout(location = 0) out vec4 colour;
/// void main() {
///     colour = texelFetch(picture, ivec2(gl_FragCoord.xy), 0);
/// }
/// ```
const PIXELS: [u32; 141] = [
    0x07230203, 0x00010000, 0x000d000b, 0x00000018, 0x00000000, 0x00020011, 0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
    0x0007000f, 0x00000004, 0x00000004, 0x6e69616d, 0x00000000, 0x00000009, 0x0000000f, 0x00030010, 0x00000004, 0x00000007, 0x00040047, 0x00000009, 0x0000001e, 0x00000000, 0x00040047, 0x0000000c,
    0x00000021, 0x00000000, 0x00040047, 0x0000000c, 0x00000022, 0x00000000, 0x00040047, 0x0000000f, 0x0000000b, 0x0000000f, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016,
    0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000004, 0x00040020, 0x00000008, 0x00000003, 0x00000007, 0x0004003b, 0x00000008, 0x00000009, 0x00000003, 0x00090019, 0x0000000a,
    0x00000006, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00040020, 0x0000000b, 0x00000000, 0x0000000a, 0x0004003b, 0x0000000b, 0x0000000c, 0x00000000, 0x00040020,
    0x0000000e, 0x00000001, 0x00000007, 0x0004003b, 0x0000000e, 0x0000000f, 0x00000001, 0x00040017, 0x00000010, 0x00000006, 0x00000002, 0x00040015, 0x00000013, 0x00000020, 0x00000001, 0x00040017,
    0x00000014, 0x00000013, 0x00000002, 0x0004002b, 0x00000013, 0x00000016, 0x00000000, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0004003d, 0x0000000a,
    0x0000000d, 0x0000000c, 0x0004003d, 0x00000007, 0x00000011, 0x0000000f, 0x0007004f, 0x00000010, 0x00000012, 0x00000011, 0x00000011, 0x00000000, 0x00000001, 0x0004006e, 0x00000014, 0x00000015,
    0x00000012, 0x0007005f, 0x00000007, 0x00000017, 0x0000000d, 0x00000015, 0x00000002, 0x00000016, 0x0003003e, 0x00000009, 0x00000017, 0x000100fd, 0x00010038,
];

/// Stops the program with what Vulkan said, where it said something went wrong.
pub fn check(result: i32, what: &str) {
    assert!(result >= 0, "{what} failed: {result}");
}

/// A device of Vulkan's, as the layer needs it: the device, and the kinds of memory it has.
pub struct Gpu {
    pub device: Object,
    pub memory: VkPhysicalDeviceMemoryProperties,
}

impl Gpu {
    /// Memory for something with these needs, of a kind that is `wanted` (device-local, or
    /// that the processor writes and reads without flushing).
    pub fn memory(&self, vk: &Vk, needs: VkMemoryRequirements, wanted: u32) -> Handle {
        let fits = |at: &u32| needs.type_bits >> at & 1 == 1 && self.memory.types[*at as usize].flags & wanted == wanted;
        let kind = (0..self.memory.type_count).find(fits).expect("the device has no memory of the kind needed");
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: kind, ..Default::default() };
        let mut memory = 0;
        // SAFETY: the device is alive, and the description is whole.
        check(unsafe { (vk.vkAllocateMemory)(self.device, &asked, null(), &mut memory) }, "allocating memory");
        memory
    }

    /// A buffer of `size` bytes for `usage`, in memory the processor writes and reads, which
    /// stays where the processor can see it: there.
    pub fn buffer(&self, vk: &Vk, size: usize, usage: u32) -> (Handle, Handle, *mut u32) {
        let made = VkBufferCreateInfo { kind: 12, size: size as u64, usage, ..Default::default() };
        let (mut buffer, mut needs, mut there) = (0, VkMemoryRequirements::default(), null_mut());
        // SAFETY: the device is alive; the buffer is made, given memory, and the memory mapped.
        unsafe {
            check((vk.vkCreateBuffer)(self.device, &made, null(), &mut buffer), "making a buffer");
            (vk.vkGetBufferMemoryRequirements)(self.device, buffer, &mut needs);
            let memory = self.memory(vk, needs, HOST_VISIBLE_AND_COHERENT);
            check((vk.vkBindBufferMemory)(self.device, buffer, memory, 0), "binding a buffer's memory");
            check((vk.vkMapMemory)(self.device, memory, 0, u64::MAX, 0, &mut there), "mapping memory");
            (buffer, memory, there.cast())
        }
    }

    /// An image `width` by `height` for `usage`, in the device's own memory; and a view of it.
    pub fn image(&self, vk: &Vk, width: usize, height: usize, usage: u32) -> (Handle, Handle, Handle) {
        let extent = [width as u32, height as u32, 1];
        let made = VkImageCreateInfo { kind: 14, image_type: 1, format: B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
        let (mut image, mut needs, mut view) = (0, VkMemoryRequirements::default(), 0);
        // SAFETY: the device is alive; the image is made, given memory, and looked at whole.
        unsafe {
            check((vk.vkCreateImage)(self.device, &made, null(), &mut image), "making an image");
            (vk.vkGetImageMemoryRequirements)(self.device, image, &mut needs);
            let memory = self.memory(vk, needs, DEVICE_LOCAL);
            check((vk.vkBindImageMemory)(self.device, image, memory, 0), "binding an image's memory");
            let seen = VkImageViewCreateInfo { kind: 15, image, view_type: 1, format: B8G8R8A8_UNORM, range: WHOLE, ..Default::default() };
            check((vk.vkCreateImageView)(self.device, &seen, null(), &mut view), "making an image's view");
            (image, memory, view)
        }
    }
}

/// The interface in Vulkan: an image as large as the surface, a buffer the processor writes
/// laid out as the surface is, and what draws the image.
pub struct Layer {
    image: Handle,
    image_memory: Handle,
    view: Handle,
    buffer: Handle,
    buffer_memory: Handle,
    /// The buffer, where the processor writes it: a pixel for each of the surface's.
    pub written: *mut u32,
    set_layout: Handle,
    pool: Handle,
    set: Handle,
    pipeline_layout: Handle,
    pipeline: Handle,
    /// The places written to the buffer that are still to be copied to the image.
    pub waiting: Vec<VkBufferImageCopy>,
    /// The image holds nothing yet: what it held before can go.
    fresh: bool,
    /// How large the image is, in pixels.
    pub width: usize,
    /// How high.
    pub height: usize,
}

impl Layer {
    /// What draws the image, in subpass 0 of the program's `render_pass` (or of any pass
    /// like it: one colour attachment of B8G8R8A8, one sample); and an image `width` by
    /// `height`.
    pub fn new(vk: &Vk, gpu: &Gpu, render_pass: Handle, width: usize, height: usize) -> Layer {
        let device = gpu.device;
        let binding = VkDescriptorSetLayoutBinding { descriptor_type: SAMPLED_IMAGE, count: 1, stages: 0x10, ..Default::default() };
        let sizes = VkDescriptorPoolSize { descriptor_type: SAMPLED_IMAGE, count: 1 };
        let mut layer = Layer {
            image: 0,
            image_memory: 0,
            view: 0,
            buffer: 0,
            buffer_memory: 0,
            written: null_mut(),
            set_layout: 0,
            pool: 0,
            set: 0,
            pipeline_layout: 0,
            pipeline: 0,
            waiting: Vec::new(),
            fresh: true,
            width: 0,
            height: 0,
        };
        // SAFETY: the device is alive; each description is whole, and what it points to lives
        // through the call.
        unsafe {
            let described = VkDescriptorSetLayoutCreateInfo { kind: 32, binding_count: 1, bindings: &binding, ..Default::default() };
            check((vk.vkCreateDescriptorSetLayout)(device, &described, null(), &mut layer.set_layout), "making the descriptor set's layout");
            let pool = VkDescriptorPoolCreateInfo { kind: 33, max_sets: 1, size_count: 1, sizes: &sizes, ..Default::default() };
            check((vk.vkCreateDescriptorPool)(device, &pool, null(), &mut layer.pool), "making the descriptor pool");
            let asked = VkDescriptorSetAllocateInfo { kind: 34, pool: layer.pool, count: 1, layouts: &layer.set_layout, ..Default::default() };
            check((vk.vkAllocateDescriptorSets)(device, &asked, &mut layer.set), "making the descriptor set");
            let laid = VkPipelineLayoutCreateInfo { kind: 30, set_layout_count: 1, set_layouts: &layer.set_layout, ..Default::default() };
            check((vk.vkCreatePipelineLayout)(device, &laid, null(), &mut layer.pipeline_layout), "making the pipeline's layout");
        }
        layer.pipeline = pipeline(vk, device, layer.pipeline_layout, render_pass);
        layer.resize(vk, gpu, width, height);
        layer
    }

    /// A new image `width` by `height`, and a buffer to match, for a surface of that size.
    /// (The device must be done with the old ones: the program waits for it first.)
    pub fn resize(&mut self, vk: &Vk, gpu: &Gpu, width: usize, height: usize) {
        (self.width, self.height) = (width.max(1), height.max(1));
        self.let_go_of_image(vk, gpu.device);
        // The image is copied to and read by shaders (0x2, 0x4); the buffer is copied from (0x1).
        (self.image, self.image_memory, self.view) = gpu.image(vk, self.width, self.height, 0x2 | 0x4);
        (self.buffer, self.buffer_memory, self.written) = gpu.buffer(vk, self.width * self.height * 4, 0x1);
        (self.fresh, self.waiting) = (true, Vec::new());
        let image = VkDescriptorImageInfo { sampler: 0, view: self.view, layout: LAYOUT_SHADER_READ_ONLY };
        let write = VkWriteDescriptorSet { kind: 35, set: self.set, count: 1, descriptor_type: SAMPLED_IMAGE, images: &image, ..Default::default() };
        // SAFETY: the set and the view are this layer's.
        unsafe { (vk.vkUpdateDescriptorSets)(gpu.device, 1, &write, 0, null()) };
    }

    /// Writes the pixels of the surface that lie in `place` (left, top, right, bottom) to the
    /// buffer, where they are in the surface; the next `record` copies them on to the image.
    /// (The device must be done with the copies before: the program's frames are.)
    pub fn update(&mut self, surface: &Surface, [left, top, right, bottom]: [usize; 4]) {
        if left >= right || top >= bottom || (surface.width, surface.height) != (self.width, self.height) {
            return;
        }
        for row in top..bottom {
            let from = &surface.pixels[row * surface.width + left..row * surface.width + right];
            // SAFETY: the buffer holds as many pixels as the surface, and the row lies in it.
            unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), self.written.add(row * surface.width + left), from.len()) };
        }
        let offset = [left as i32, top as i32, 0];
        let extent = [(right - left) as u32, (bottom - top) as u32, 1];
        self.waiting.push(VkBufferImageCopy { buffer_offset: ((top * surface.width + left) * 4) as u64, row_length: surface.width as u32, image_height: 0, layers: [0x1, 0, 0, 1], offset, extent });
    }

    /// Copies what is waiting in the buffer to the image: the image is made a place to copy
    /// to, copied to, and made a place for shaders to read again. Recorded outside the render
    /// pass, before it.
    pub fn record(&mut self, vk: &Vk, commands: Object) {
        if self.waiting.is_empty() {
            return;
        }
        let before = if self.fresh { (LAYOUT_UNDEFINED, 0, STAGE_TOP) } else { (LAYOUT_SHADER_READ_ONLY, ACCESS_SHADER_READ, STAGE_FRAGMENT_SHADER) };
        let to_copy = barrier(self.image, before.0, LAYOUT_TRANSFER_DESTINATION, before.1, ACCESS_TRANSFER_WRITE);
        let to_read = barrier(self.image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_SHADER_READ_ONLY, ACCESS_TRANSFER_WRITE, ACCESS_SHADER_READ);
        // SAFETY: the command buffer is being recorded; the regions lie in the buffer and image.
        unsafe {
            (vk.vkCmdPipelineBarrier)(commands, before.2, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            (vk.vkCmdCopyBufferToImage)(commands, self.buffer, self.image, LAYOUT_TRANSFER_DESTINATION, self.waiting.len() as u32, self.waiting.as_ptr());
            (vk.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_FRAGMENT_SHADER, 0, 0, null(), 0, null(), 1, &to_read);
        }
        (self.fresh, self.waiting) = (false, Vec::new());
    }

    /// Draws the image over the render pass's target, at its top left corner, pixel for
    /// pixel, blended over what is there. Recorded inside the render pass.
    pub fn draw(&self, vk: &Vk, commands: Object) {
        let (width, height) = (self.width as u32, self.height as u32);
        let viewport = VkViewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 };
        let scissor = VkRect2D { offset: [0, 0], extent: [width, height] };
        // SAFETY: the command buffer is being recorded, inside a render pass like the layer's.
        unsafe {
            (vk.vkCmdBindPipeline)(commands, GRAPHICS, self.pipeline);
            (vk.vkCmdSetViewport)(commands, 0, 1, &viewport);
            (vk.vkCmdSetScissor)(commands, 0, 1, &scissor);
            (vk.vkCmdBindDescriptorSets)(commands, GRAPHICS, self.pipeline_layout, 0, 1, &self.set, 0, null());
            (vk.vkCmdDraw)(commands, 4, 1, 0, 0);
        }
    }

    /// Lets go of the image and the buffer, and their memory.
    fn let_go_of_image(&mut self, vk: &Vk, device: Object) {
        // SAFETY: each is this layer's, and the device is done with it. (Nothing is let go of
        // where nothing was made: a handle of 0.)
        unsafe {
            (vk.vkDestroyImageView)(device, self.view, null());
            (vk.vkDestroyImage)(device, self.image, null());
            (vk.vkFreeMemory)(device, self.image_memory, null());
            (vk.vkDestroyBuffer)(device, self.buffer, null());
            (vk.vkFreeMemory)(device, self.buffer_memory, null());
        }
    }

    /// Lets go of everything the layer holds. (The device is done with it.)
    pub fn destroy(mut self, vk: &Vk, device: Object) {
        self.let_go_of_image(vk, device);
        // SAFETY: as above.
        unsafe {
            (vk.vkDestroyPipeline)(device, self.pipeline, null());
            (vk.vkDestroyPipelineLayout)(device, self.pipeline_layout, null());
            (vk.vkDestroyDescriptorPool)(device, self.pool, null());
            (vk.vkDestroyDescriptorSetLayout)(device, self.set_layout, null());
        }
    }
}

/// An image's layout changing, and what was done to it before being made visible to what
/// is done after.
pub fn barrier(image: Handle, old_layout: u32, new_layout: u32, source_access: u32, destination_access: u32) -> VkImageMemoryBarrier {
    VkImageMemoryBarrier { kind: 45, source_access, destination_access, old_layout, new_layout, source_family: !0, destination_family: !0, image, range: WHOLE, ..Default::default() }
}

/// The pipeline that draws the layer: the quad as a strip of two triangles, nothing culled,
/// and its colour blended as "one, one minus source alpha", the viewport and scissor set
/// when it is drawn.
fn pipeline(vk: &Vk, device: Object, layout: Handle, render_pass: Handle) -> Handle {
    let module = |code: &[u32]| {
        let made = VkShaderModuleCreateInfo { kind: 16, size: code.len() * 4, code: code.as_ptr(), ..Default::default() };
        let mut module = 0;
        // SAFETY: the device is alive, and the code is SPIR-V, as long as is said.
        check(unsafe { (vk.vkCreateShaderModule)(device, &made, null(), &mut module) }, "making a shader");
        module
    };
    let (corners, pixels) = (module(&CORNERS), module(&PIXELS));
    let stage = |stage, module| VkPipelineShaderStageCreateInfo { kind: 18, stage, module, name: c"main".as_ptr(), ..Default::default() };
    let stages = [stage(0x1, corners), stage(0x10, pixels)];
    let vertex_input = VkPipelineVertexInputStateCreateInfo { kind: 19, ..Default::default() };
    let input_assembly = VkPipelineInputAssemblyStateCreateInfo { kind: 20, topology: 4, ..Default::default() };
    let viewport = VkPipelineViewportStateCreateInfo { kind: 22, viewport_count: 1, scissor_count: 1, ..Default::default() };
    let rasterization = VkPipelineRasterizationStateCreateInfo { kind: 23, line_width: 1.0, ..Default::default() };
    let multisample = VkPipelineMultisampleStateCreateInfo { kind: 24, samples: 1, ..Default::default() };
    // One (1), one minus source alpha (7), added (0), for colour and alpha alike; all four written.
    let blend = VkPipelineColorBlendAttachmentState { enable: 1, source: 1, destination: 7, operation: 0, source_alpha: 1, destination_alpha: 7, operation_alpha: 0, write_mask: 0xf };
    let colour_blend = VkPipelineColorBlendStateCreateInfo { kind: 26, attachment_count: 1, attachments: &blend, ..Default::default() };
    let dynamic_states = [0, 1];
    let dynamic = VkPipelineDynamicStateCreateInfo { kind: 27, count: 2, states: dynamic_states.as_ptr(), ..Default::default() };
    let made = VkGraphicsPipelineCreateInfo {
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
        render_pass,
        base_index: -1,
        ..Default::default()
    };
    let mut pipeline = 0;
    // SAFETY: the device is alive; everything the description points to lives through the call.
    unsafe {
        check((vk.vkCreateGraphicsPipelines)(device, 0, 1, &made, null(), &mut pipeline), "making the pipeline");
        (vk.vkDestroyShaderModule)(device, corners, null());
        (vk.vkDestroyShaderModule)(device, pixels, null());
    }
    pipeline
}

// Everything from here on is the program's own: its device, the image it draws into, what
// it draws.

structures! {
    VkApplicationInfo { kind: u32, next: *const c_void, name: *const c_char, version: u32, engine: *const c_char, engine_version: u32, api_version: u32, }
    VkInstanceCreateInfo { kind: u32, next: *const c_void, flags: u32, application: *const VkApplicationInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, }
    VkQueueFamilyProperties { flags: u32, count: u32, timestamp_bits: u32, granularity: [u32; 3], }
    VkDeviceQueueCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, count: u32, priorities: *const f32, }
    VkDeviceCreateInfo { kind: u32, next: *const c_void, flags: u32, queue_count: u32, queues: *const VkDeviceQueueCreateInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, features: *const c_void, }
    VkAttachmentDescription { flags: u32, format: u32, samples: u32, load: u32, store: u32, stencil_load: u32, stencil_store: u32, initial_layout: u32, final_layout: u32, }
    VkAttachmentReference { attachment: u32, layout: u32, }
    VkSubpassDescription { flags: u32, bind_point: u32, input_count: u32, inputs: *const VkAttachmentReference, colour_count: u32, colours: *const VkAttachmentReference, resolves: *const VkAttachmentReference, depth_stencil: *const VkAttachmentReference, preserve_count: u32, preserves: *const u32, }
    VkRenderPassCreateInfo { kind: u32, next: *const c_void, flags: u32, attachment_count: u32, attachments: *const VkAttachmentDescription, subpass_count: u32, subpasses: *const VkSubpassDescription, dependency_count: u32, dependencies: *const c_void, }
    VkFramebufferCreateInfo { kind: u32, next: *const c_void, flags: u32, render_pass: Handle, attachment_count: u32, attachments: *const Handle, width: u32, height: u32, layers: u32, }
    VkCommandPoolCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, }
    VkCommandBufferAllocateInfo { kind: u32, next: *const c_void, pool: Handle, level: u32, count: u32, }
    VkCommandBufferBeginInfo { kind: u32, next: *const c_void, flags: u32, inheritance: *const c_void, }
    VkRenderPassBeginInfo { kind: u32, next: *const c_void, render_pass: Handle, framebuffer: Handle, area: VkRect2D, clear_count: u32, clears: *const c_void, }
    VkSubmitInfo { kind: u32, next: *const c_void, wait_count: u32, waits: *const Handle, wait_stages: *const u32, buffer_count: u32, buffers: *const Object, signal_count: u32, signals: *const Handle, }
    VkFenceCreateInfo { kind: u32, next: *const c_void, flags: u32, }
    VkMemoryBarrier { kind: u32, next: *const c_void, source_access: u32, destination_access: u32, }
}

functions!(Program {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkDestroyDevice: fn(Object, *const c_void);
    vkGetDeviceQueue: fn(Object, u32, u32, *mut Object);
    vkCreateRenderPass: fn(Object, *const VkRenderPassCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyRenderPass: fn(Object, Handle, *const c_void);
    vkCreateFramebuffer: fn(Object, *const VkFramebufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFramebuffer: fn(Object, Handle, *const c_void);
    vkCreateCommandPool: fn(Object, *const VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkBeginCommandBuffer: fn(Object, *const VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCmdBeginRenderPass: fn(Object, *const VkRenderPassBeginInfo, u32);
    vkCmdEndRenderPass: fn(Object);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdCopyImageToBuffer: fn(Object, Handle, u32, Handle, u32, *const VkBufferImageCopy);
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const VkMemoryBarrier, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCreateFence: fn(Object, *const VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
    vkQueueSubmit: fn(Object, u32, *const VkSubmitInfo, Handle) -> i32;
    vkDeviceWaitIdle: fn(Object) -> i32;
});

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryA(name: *const c_char) -> isize;
    fn GetProcAddress(library: isize, name: *const u8) -> *const c_void;
}
#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(file: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// The loader's one function that finds all the others, if the system has a loader.
fn loader() -> Option<unsafe extern "system" fn(Object, *const c_char) -> *const c_void> {
    // SAFETY: the names end in a zero; the library stays open while the program runs.
    #[cfg(windows)]
    let found = unsafe { Some(LoadLibraryA(c"vulkan-1.dll".as_ptr())).filter(|library| *library != 0).map(|library| GetProcAddress(library, c"vkGetInstanceProcAddr".as_ptr().cast())) };
    #[cfg(unix)]
    let found = unsafe { Some(dlopen(c"libvulkan.so.1".as_ptr(), 2)).filter(|library| !library.is_null()).map(|library| dlsym(library, c"vkGetInstanceProcAddr".as_ptr())) };
    // SAFETY: the loader's function is declared as vulkan_core.h declares it.
    found.filter(|found| !found.is_null()).map(|found| unsafe { std::mem::transmute(found) })
}

/// A Vulkan device with a queue that draws, a command buffer, and a fence to wait on it.
pub struct Device {
    pub vk: Vk,
    pub program: Program,
    instance: Object,
    pub gpu: Gpu,
    queue: Object,
    pool: Handle,
    pub commands: Object,
    fence: Handle,
}

impl Device {
    /// The first device the system has that draws; nothing, where there is none.
    pub fn new() -> Option<Device> {
        type Make = unsafe extern "system" fn(*const VkInstanceCreateInfo, *const c_void, *mut Object) -> i32;
        let find = loader()?;
        let application = VkApplicationInfo { name: c"taut".as_ptr(), api_version: 1 << 22, ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: the loader hands out functions by name; the instance is made once.
        unsafe {
            let make = Some(find(null_mut(), c"vkCreateInstance".as_ptr())).filter(|make| !make.is_null())?;
            let asked = VkInstanceCreateInfo { kind: 1, application: &application, ..Default::default() };
            (std::mem::transmute::<*const c_void, Make>(make)(&asked, null(), &mut instance) >= 0).then_some(())?;
        }
        let mut found = |name: &str| unsafe { find(instance, name.as_ptr().cast()) };
        let (vk, program) = (Vk::load(&mut found)?, Program::load(&mut found)?);
        Device::on_first(vk, program, instance)
    }

    /// The device, on the first of the system's that has a queue that draws.
    fn on_first(vk: Vk, program: Program, instance: Object) -> Option<Device> {
        let (mut count, mut physical, mut families, mut family) = (1, null_mut(), [VkQueueFamilyProperties::default(); 16], 16);
        let priority = 1.0f32;
        // SAFETY: the instance is alive; each list has room for as many as is said.
        unsafe {
            (program.vkEnumeratePhysicalDevices)(instance, &mut count, &mut physical);
            (count == 1).then_some(())?;
            (program.vkGetPhysicalDeviceQueueFamilyProperties)(physical, &mut family, families.as_mut_ptr());
            let family = families[..family as usize].iter().position(|kind| kind.flags & 1 == 1)? as u32;
            let queue = VkDeviceQueueCreateInfo { kind: 2, family, count: 1, priorities: &priority, ..Default::default() };
            let asked = VkDeviceCreateInfo { kind: 3, queue_count: 1, queues: &queue, ..Default::default() };
            let mut gpu = Gpu { device: null_mut(), memory: VkPhysicalDeviceMemoryProperties::default() };
            (vk.vkGetPhysicalDeviceMemoryProperties)(physical, &mut gpu.memory);
            ((program.vkCreateDevice)(physical, &asked, null(), &mut gpu.device) >= 0).then_some(())?;
            Some(Device::with_commands(vk, program, instance, gpu, family))
        }
    }

    /// The device's queue, a command buffer that is used again and again, and its fence.
    fn with_commands(vk: Vk, program: Program, instance: Object, gpu: Gpu, family: u32) -> Device {
        let (device, mut queue, mut pool, mut commands, mut fence) = (gpu.device, null_mut(), 0, null_mut(), 0);
        // SAFETY: the device is alive; each description is whole.
        unsafe {
            (program.vkGetDeviceQueue)(device, family, 0, &mut queue);
            let pooled = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family, ..Default::default() };
            check((program.vkCreateCommandPool)(device, &pooled, null(), &mut pool), "making a command pool");
            let asked = VkCommandBufferAllocateInfo { kind: 40, pool, count: 1, ..Default::default() };
            check((program.vkAllocateCommandBuffers)(device, &asked, &mut commands), "making a command buffer");
            check((program.vkCreateFence)(device, &VkFenceCreateInfo { kind: 8, ..Default::default() }, null(), &mut fence), "making a fence");
        }
        Device { vk, program, instance, gpu, queue, pool, commands, fence }
    }

    /// Starts recording the command buffer.
    pub fn begin(&self) {
        // SAFETY: the device is done with the buffer: every submit was waited for.
        check(unsafe { (self.program.vkBeginCommandBuffer)(self.commands, &VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() }) }, "recording");
    }

    /// Hands what was recorded to the device, and waits until it is done.
    pub fn submit(&self) {
        let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &self.commands, ..Default::default() };
        // SAFETY: the buffer was recorded; the fence is this device's, and not in use.
        unsafe {
            check((self.program.vkEndCommandBuffer)(self.commands), "ending the recording");
            check((self.program.vkQueueSubmit)(self.queue, 1, &submit, self.fence), "submitting");
            check((self.program.vkWaitForFences)(self.gpu.device, 1, &self.fence, 1, u64::MAX), "waiting");
            check((self.program.vkResetFences)(self.gpu.device, 1, &self.fence), "resetting the fence");
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let (program, device) = (&self.program, self.gpu.device);
        // SAFETY: the device is done with everything (it is waited for first).
        unsafe {
            (program.vkDeviceWaitIdle)(device);
            (program.vkDestroyFence)(device, self.fence, null());
            (program.vkDestroyCommandPool)(device, self.pool, null());
            (program.vkDestroyDevice)(device, null());
            (program.vkDestroyInstance)(self.instance, null());
        }
    }
}

/// The image the program draws into, `width` by `height`; the render pass that draws into it,
/// keeping what is there; and a buffer the processor writes the program's own picture into
/// and reads the frame back from.
pub struct Target {
    pub render_pass: Handle,
    image: Handle,
    memory: Handle,
    view: Handle,
    framebuffer: Handle,
    buffer: Handle,
    buffer_memory: Handle,
    there: *mut u32,
    pub width: usize,
    pub height: usize,
}

impl Target {
    pub fn new(device: &Device, width: usize, height: usize) -> Target {
        let (vk, program, gpu) = (&device.vk, &device.program, &device.gpu);
        // Copied to and from (0x2, 0x1), drawn into (0x10); the buffer both ways too.
        let (image, memory, view) = gpu.image(vk, width, height, 0x1 | 0x2 | 0x10);
        let (buffer, buffer_memory, there) = gpu.buffer(vk, width * height * 4, 0x1 | 0x2);
        // One colour attachment, its pixels kept (load 0, store 0), in the layout for drawing (2).
        let attachment = VkAttachmentDescription { format: B8G8R8A8_UNORM, samples: 1, stencil_load: 2, stencil_store: 1, initial_layout: 2, final_layout: 2, ..Default::default() };
        let colour = VkAttachmentReference { attachment: 0, layout: 2 };
        let subpass = VkSubpassDescription { colour_count: 1, colours: &colour, ..Default::default() };
        let pass = VkRenderPassCreateInfo { kind: 38, attachment_count: 1, attachments: &attachment, subpass_count: 1, subpasses: &subpass, ..Default::default() };
        let (mut render_pass, mut framebuffer) = (0, 0);
        // SAFETY: the device is alive; each description is whole.
        unsafe {
            check((program.vkCreateRenderPass)(gpu.device, &pass, null(), &mut render_pass), "making the render pass");
            let made = VkFramebufferCreateInfo { kind: 37, render_pass, attachment_count: 1, attachments: &view, width: width as u32, height: height as u32, layers: 1, ..Default::default() };
            check((program.vkCreateFramebuffer)(gpu.device, &made, null(), &mut framebuffer), "making the framebuffer");
        }
        Target { render_pass, image, memory, view, framebuffer, buffer, buffer_memory, there, width, height }
    }

    /// One frame: `picture`, the program's own, copied into the image; the layer's waiting
    /// copies; the layer drawn over the picture in the render pass; and the frame read back.
    pub fn frame(&self, device: &Device, layer: &mut Layer, picture: &[u32]) -> Vec<u32> {
        let (vk, program, commands) = (&device.vk, &device.program, device.commands);
        let whole = VkBufferImageCopy { row_length: self.width as u32, layers: [0x1, 0, 0, 1], extent: [self.width as u32, self.height as u32, 1], ..Default::default() };
        let area = VkRect2D { offset: [0, 0], extent: [self.width as u32, self.height as u32] };
        let begun = VkRenderPassBeginInfo { kind: 43, render_pass: self.render_pass, framebuffer: self.framebuffer, area, ..Default::default() };
        // The image: copied to, drawn into, copied from; and the buffer read by the processor.
        let (to_copy, to_draw) = (barrier(self.image, 0, 7, 0, 0x1000), barrier(self.image, 7, 2, 0x1000, 0x80 | 0x100));
        let (to_read, by_hand) = (barrier(self.image, 2, 6, 0x100, 0x800), VkMemoryBarrier { kind: 46, source_access: 0x1000, destination_access: 0x2000, ..Default::default() });
        // SAFETY: the buffer is as large as the picture; the device is done with it (each
        // frame is waited for); the command buffer is recorded and submitted once.
        unsafe {
            std::ptr::copy_nonoverlapping(picture.as_ptr(), self.there, self.width * self.height);
            device.begin();
            (program.vkCmdPipelineBarrier)(commands, 0x1, 0x1000, 0, 0, null(), 0, null(), 1, &to_copy);
            (program.vkCmdCopyBufferToImage)(commands, self.buffer, self.image, 7, 1, &whole);
            (program.vkCmdPipelineBarrier)(commands, 0x1000, 0x400, 0, 0, null(), 0, null(), 1, &to_draw);
            layer.record(vk, commands);
            (program.vkCmdBeginRenderPass)(commands, &begun, 0);
            layer.draw(vk, commands);
            (program.vkCmdEndRenderPass)(commands);
            (program.vkCmdPipelineBarrier)(commands, 0x400, 0x1000, 0, 0, null(), 0, null(), 1, &to_read);
            (program.vkCmdCopyImageToBuffer)(commands, self.image, 6, self.buffer, 1, &whole);
            (program.vkCmdPipelineBarrier)(commands, 0x1000, 0x4000, 0, 1, &by_hand, 0, null(), 0, null());
            device.submit();
            std::slice::from_raw_parts(self.there, self.width * self.height).to_vec()
        }
    }

    /// Lets go of all of it. (The device is done with it.)
    pub fn destroy(self, device: &Device) {
        let (vk, program, gpu) = (&device.vk, &device.program, device.gpu.device);
        // SAFETY: each is this target's.
        unsafe {
            (program.vkDestroyFramebuffer)(gpu, self.framebuffer, null());
            (program.vkDestroyRenderPass)(gpu, self.render_pass, null());
            (vk.vkDestroyImageView)(gpu, self.view, null());
            (vk.vkDestroyImage)(gpu, self.image, null());
            (vk.vkFreeMemory)(gpu, self.memory, null());
            (vk.vkDestroyBuffer)(gpu, self.buffer, null());
            (vk.vkFreeMemory)(gpu, self.buffer_memory, null());
        }
    }
}

/// What the program's own parts are set to.
#[derive(Default)]
struct App {
    on: bool,
    level: f32,
    name: String,
}

fn describe(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().w(Length::Px(260.0)).at(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.alpha(0.9)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
    ui.text("Over Vulkan", t.font.bold(), t.text);
    ui.switch(&mut app.on, "A switch");
    ui.slider(&mut app.level, 0.0..=4.0);
    ui.text_box(&mut app.name, "Type here");
    ui.close();
}

/// The letters of the system, where this library knows how to reach them.
fn letters() -> Box<dyn TextSystem> {
    taut::platform::system_text()
}

fn main() {
    let file = std::env::args().nth(1).unwrap_or("vulkan.png".into());
    let Some(device) = Device::new() else { return println!("there is no Vulkan here: no loader, or no device that draws") };
    let (width, height) = (480, 320);
    let target = Target::new(&device, width, height);
    let mut layer = Layer::new(&device.vk, &device.gpu, target.render_pass, width, height);
    let mut ui = Ui::new(letters(), Box::new(LocalClipboard::default()));
    ui.theme.bg = Color::NONE;
    ui.resize(Size { w: width as f32, h: height as f32 }, 1.0);
    let (mut app, mut surface) = (App::default(), Surface::default());
    // The program's own picture: a gradient.
    let picture: Vec<u32> = (0..width * height).map(|at| 0xff00_0000 | ((at % width * 255 / width) as u32) << 16 | ((at / width * 255 / height) as u32) << 8 | 0x60).collect();
    // The first frame copies all of the surface; the next, after the keyboard has come to the
    // switch and turned it on, only the places that changed.
    for event in [None, Some(Event::Key(Key::Tab, Mods::NONE)), Some(Event::Key(Key::Space, Mods::NONE))] {
        if let Some(event) = event {
            ui.event(event);
        }
        ui.run(&mut |ui| describe(&mut app, ui));
        for place in surface.update(&mut ui) {
            layer.update(&surface, place);
        }
        println!("copied to the image: {:?}", layer.waiting.iter().map(|copy| (copy.offset, copy.extent)).collect::<Vec<_>>());
        let frame = target.frame(&device, &mut layer, &picture);
        std::fs::write(&file, Surface { width, height, pixels: frame }.png()).expect("the picture could not be written");
    }
    println!("the last frame is in {file}");
    layer.destroy(&device.vk, device.gpu.device);
    target.destroy(&device);
}
