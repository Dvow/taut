//! taut under Vulkan. The program renders with Vulkan as usual, and the interface lies over it:
//! `taut::layer_vulkan::Layer`, made once from the program's device and queue, draws it with one
//! call per frame, inside the program's render pass.
//!
//! Around it is a windowless program: it renders its own picture, the interface over it,
//! changes something and renders again, and writes the frame to a PNG file. (A windowed program
//! has a swap chain, whose images are drawn into the same way.)
//!   cargo run --example vulkan [<file.png>]
//!
//! Only the system's Vulkan loader (vulkan-1.dll, libvulkan.so.1), opened by name; the API used
//! is declared here as vulkan_core.h declares it. (A program using a Vulkan bindings crate hands
//! the layer its handles' raw values.)
// The handles handed to the functions here are the program's own Vulkan objects, trusted as
// Vulkan trusts them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};
use taut::geom::Size;
use taut::layer_vulkan::{Handles, Layer};
use taut::text::TextSystem;
use taut::{Color, Event, Key, Length, LocalClipboard, Mods, Ui};

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

/// A handle that is a number (an image, a buffer, a render pass), not a pointer.
pub type Handle = u64;
/// A handle that is a pointer (the instance, the device, a queue, a command buffer).
pub type Object = *mut c_void;

structures! {
    VkApplicationInfo { kind: u32, next: *const c_void, name: *const c_char, version: u32, engine: *const c_char, engine_version: u32, api_version: u32, }
    VkInstanceCreateInfo { kind: u32, next: *const c_void, flags: u32, application: *const VkApplicationInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, }
    VkQueueFamilyProperties { flags: u32, count: u32, timestamp_bits: u32, granularity: [u32; 3], }
    VkDeviceQueueCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, count: u32, priorities: *const f32, }
    VkDeviceCreateInfo { kind: u32, next: *const c_void, flags: u32, queue_count: u32, queues: *const VkDeviceQueueCreateInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, features: *const c_void, }
    VkMemoryType { flags: u32, heap: u32, }
    VkMemoryHeap { size: u64, flags: u32, }
    VkPhysicalDeviceMemoryProperties { type_count: u32, types: [VkMemoryType; 32], heap_count: u32, heaps: [VkMemoryHeap; 16], }
    VkMemoryRequirements { size: u64, alignment: u64, type_bits: u32, }
    VkMemoryAllocateInfo { kind: u32, next: *const c_void, size: u64, type_index: u32, }
    VkImageCreateInfo { kind: u32, next: *const c_void, flags: u32, image_type: u32, format: u32, extent: [u32; 3], mip_levels: u32, array_layers: u32, samples: u32, tiling: u32, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, initial_layout: u32, }
    VkImageViewCreateInfo { kind: u32, next: *const c_void, flags: u32, image: Handle, view_type: u32, format: u32, components: [u32; 4], range: [u32; 5], }
    VkBufferCreateInfo { kind: u32, next: *const c_void, flags: u32, size: u64, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, }
    VkAttachmentDescription { flags: u32, format: u32, samples: u32, load: u32, store: u32, stencil_load: u32, stencil_store: u32, initial_layout: u32, final_layout: u32, }
    VkAttachmentReference { attachment: u32, layout: u32, }
    VkSubpassDescription { flags: u32, bind_point: u32, input_count: u32, inputs: *const VkAttachmentReference, colour_count: u32, colours: *const VkAttachmentReference, resolves: *const VkAttachmentReference, depth_stencil: *const VkAttachmentReference, preserve_count: u32, preserves: *const u32, }
    VkRenderPassCreateInfo { kind: u32, next: *const c_void, flags: u32, attachment_count: u32, attachments: *const VkAttachmentDescription, subpass_count: u32, subpasses: *const VkSubpassDescription, dependency_count: u32, dependencies: *const c_void, }
    VkFramebufferCreateInfo { kind: u32, next: *const c_void, flags: u32, render_pass: Handle, attachment_count: u32, attachments: *const Handle, width: u32, height: u32, layers: u32, }
    VkCommandPoolCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, }
    VkCommandBufferAllocateInfo { kind: u32, next: *const c_void, pool: Handle, level: u32, count: u32, }
    VkCommandBufferBeginInfo { kind: u32, next: *const c_void, flags: u32, inheritance: *const c_void, }
    VkRect2D { offset: [i32; 2], extent: [u32; 2], }
    VkRenderPassBeginInfo { kind: u32, next: *const c_void, render_pass: Handle, framebuffer: Handle, area: VkRect2D, clear_count: u32, clears: *const c_void, }
    VkSubmitInfo { kind: u32, next: *const c_void, wait_count: u32, waits: *const Handle, wait_stages: *const u32, buffer_count: u32, buffers: *const Object, signal_count: u32, signals: *const Handle, }
    VkFenceCreateInfo { kind: u32, next: *const c_void, flags: u32, }
    VkMemoryBarrier { kind: u32, next: *const c_void, source_access: u32, destination_access: u32, }
    VkImageMemoryBarrier { kind: u32, next: *const c_void, source_access: u32, destination_access: u32, old_layout: u32, new_layout: u32, source_family: u32, destination_family: u32, image: Handle, range: [u32; 5], }
    VkBufferImageCopy { buffer_offset: u64, row_length: u32, image_height: u32, layers: [u32; 4], offset: [i32; 3], extent: [u32; 3], }
}

functions!(Program {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkDestroyDevice: fn(Object, *const c_void);
    vkGetDeviceQueue: fn(Object, u32, u32, *mut Object);
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

/// Pixels of B8G8R8A8_UNORM: blue, green, red and alpha bytes, as the interface's are.
pub const B8G8R8A8_UNORM: u32 = 44;
/// All of an image's colour: aspect, first level, levels, first layer, layers.
pub const WHOLE: [u32; 5] = [0x1, 0, 1, 0, 1];

/// Stops the program with what Vulkan said, where it said something went wrong.
pub fn check(result: i32, what: &str) {
    assert!(result >= 0, "{what} failed: {result}");
}

/// An image's layout changing, and what was done to it before made visible to what is done
/// after.
pub fn barrier(image: Handle, old_layout: u32, new_layout: u32, source_access: u32, destination_access: u32) -> VkImageMemoryBarrier {
    VkImageMemoryBarrier { kind: 45, source_access, destination_access, old_layout, new_layout, source_family: !0, destination_family: !0, image, range: WHOLE, ..Default::default() }
}

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
    let found = unsafe { Some(dlopen(c"libvulkan.so.1".as_ptr(), 2)).filter(|library| !library.is_null()).map(|library| dlsym(library, c"vkGetInstanceProcAddr".as_ptr()).cast_const()) };
    // (A system with neither, as a browser page, has no loader.)
    #[cfg(not(any(windows, unix)))]
    let found: Option<*const c_void> = None;
    // SAFETY: the loader's function is declared as vulkan_core.h declares it.
    found.filter(|found| !found.is_null()).map(|found| unsafe { std::mem::transmute(found) })
}

/// A Vulkan device with a queue that draws, a command buffer, and a fence to wait on it.
pub struct Device {
    pub program: Program,
    instance: Object,
    physical: Object,
    pub device: Object,
    memory: VkPhysicalDeviceMemoryProperties,
    pub queue: Object,
    family: u32,
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
        let program = Program::load(&mut |name| unsafe { find(instance, name.as_ptr().cast()) })?;
        Device::on_first(program, instance)
    }

    /// The device, on the first of the system's that has a queue that draws.
    fn on_first(program: Program, instance: Object) -> Option<Device> {
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
            let (mut device, mut memory) = (null_mut(), VkPhysicalDeviceMemoryProperties::default());
            (program.vkGetPhysicalDeviceMemoryProperties)(physical, &mut memory);
            ((program.vkCreateDevice)(physical, &asked, null(), &mut device) >= 0).then_some(())?;
            Some(Device::with_commands(program, instance, physical, device, memory, family))
        }
    }

    /// The device's queue, a command buffer that is used again and again, and its fence.
    fn with_commands(program: Program, instance: Object, physical: Object, device: Object, memory: VkPhysicalDeviceMemoryProperties, family: u32) -> Device {
        let (mut queue, mut pool, mut commands, mut fence) = (null_mut(), 0, null_mut(), 0);
        // SAFETY: the device is alive; each description is whole.
        unsafe {
            (program.vkGetDeviceQueue)(device, family, 0, &mut queue);
            let pooled = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family, ..Default::default() };
            check((program.vkCreateCommandPool)(device, &pooled, null(), &mut pool), "making a command pool");
            let asked = VkCommandBufferAllocateInfo { kind: 40, pool, count: 1, ..Default::default() };
            check((program.vkAllocateCommandBuffers)(device, &asked, &mut commands), "making a command buffer");
            check((program.vkCreateFence)(device, &VkFenceCreateInfo { kind: 8, ..Default::default() }, null(), &mut fence), "making a fence");
        }
        Device { program, instance, physical, device, memory, queue, family, pool, commands, fence }
    }

    /// What the layer is given: the device, its queue, and the render pass it is drawn in.
    pub fn handles(&self, render_pass: Handle) -> Handles {
        Handles { instance: self.instance, physical_device: self.physical, device: self.device, queue: self.queue, queue_family: self.family, render_pass, subpass: 0 }
    }

    /// Memory for something with these needs, of a kind that is `wanted` (device-local, or
    /// that the processor writes and reads without flushing).
    pub fn memory(&self, needs: VkMemoryRequirements, wanted: u32) -> Handle {
        let fits = |at: &u32| needs.type_bits >> at & 1 == 1 && self.memory.types[*at as usize].flags & wanted == wanted;
        let kind = (0..self.memory.type_count).find(fits).expect("the device has no memory of the kind needed");
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: kind, ..Default::default() };
        let mut memory = 0;
        // SAFETY: the device is alive, and the description is whole.
        check(unsafe { (self.program.vkAllocateMemory)(self.device, &asked, null(), &mut memory) }, "allocating memory");
        memory
    }

    /// A buffer of `size` bytes for `usage`, in memory the processor writes and reads, which
    /// stays where the processor can see it: there.
    pub fn buffer(&self, size: usize, usage: u32) -> (Handle, Handle, *mut u32) {
        let made = VkBufferCreateInfo { kind: 12, size: size as u64, usage, ..Default::default() };
        let (program, mut buffer, mut needs, mut there) = (&self.program, 0, VkMemoryRequirements::default(), null_mut());
        // SAFETY: the device is alive; the buffer is made, given memory, and the memory mapped.
        unsafe {
            check((program.vkCreateBuffer)(self.device, &made, null(), &mut buffer), "making a buffer");
            (program.vkGetBufferMemoryRequirements)(self.device, buffer, &mut needs);
            let memory = self.memory(needs, 0x2 | 0x4);
            check((program.vkBindBufferMemory)(self.device, buffer, memory, 0), "binding a buffer's memory");
            check((program.vkMapMemory)(self.device, memory, 0, u64::MAX, 0, &mut there), "mapping memory");
            (buffer, memory, there.cast())
        }
    }

    /// An image `width` by `height` for `usage`, in the device's own memory; and a view of it.
    pub fn image(&self, width: usize, height: usize, usage: u32) -> (Handle, Handle, Handle) {
        let extent = [width as u32, height as u32, 1];
        let made = VkImageCreateInfo { kind: 14, image_type: 1, format: B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
        let (program, mut image, mut needs, mut view) = (&self.program, 0, VkMemoryRequirements::default(), 0);
        // SAFETY: the device is alive; the image is made, given memory, and looked at whole.
        unsafe {
            check((program.vkCreateImage)(self.device, &made, null(), &mut image), "making an image");
            (program.vkGetImageMemoryRequirements)(self.device, image, &mut needs);
            let memory = self.memory(needs, 0x1);
            check((program.vkBindImageMemory)(self.device, image, memory, 0), "binding an image's memory");
            let view_info = VkImageViewCreateInfo { kind: 15, image, view_type: 1, format: B8G8R8A8_UNORM, range: WHOLE, ..Default::default() };
            check((program.vkCreateImageView)(self.device, &view_info, null(), &mut view), "making an image's view");
            (image, memory, view)
        }
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
            check((self.program.vkWaitForFences)(self.device, 1, &self.fence, 1, u64::MAX), "waiting");
            check((self.program.vkResetFences)(self.device, 1, &self.fence), "resetting the fence");
        }
    }

    /// Waits until the device has done everything it was given.
    pub fn wait(&self) {
        // SAFETY: the device is alive.
        check(unsafe { (self.program.vkDeviceWaitIdle)(self.device) }, "waiting for the device");
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let (program, device) = (&self.program, self.device);
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
    pub image: Handle,
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
        let program = &device.program;
        // Copied to and from (0x2, 0x1), drawn into (0x10); the buffer both ways too.
        let (image, memory, view) = device.image(width, height, 0x1 | 0x2 | 0x10);
        let (buffer, buffer_memory, there) = device.buffer(width * height * 4, 0x1 | 0x2);
        // One colour attachment, its pixels kept (load 0, store 0), in the layout for drawing (2).
        let attachment = VkAttachmentDescription { format: B8G8R8A8_UNORM, samples: 1, stencil_load: 2, stencil_store: 1, initial_layout: 2, final_layout: 2, ..Default::default() };
        let colour = VkAttachmentReference { attachment: 0, layout: 2 };
        let subpass = VkSubpassDescription { colour_count: 1, colours: &colour, ..Default::default() };
        let pass = VkRenderPassCreateInfo { kind: 38, attachment_count: 1, attachments: &attachment, subpass_count: 1, subpasses: &subpass, ..Default::default() };
        let (mut render_pass, mut framebuffer) = (0, 0);
        // SAFETY: the device is alive; each description is whole.
        unsafe {
            check((program.vkCreateRenderPass)(device.device, &pass, null(), &mut render_pass), "making the render pass");
            let made = VkFramebufferCreateInfo { kind: 37, render_pass, attachment_count: 1, attachments: &view, width: width as u32, height: height as u32, layers: 1, ..Default::default() };
            check((program.vkCreateFramebuffer)(device.device, &made, null(), &mut framebuffer), "making the framebuffer");
        }
        Target { render_pass, image, memory, view, framebuffer, buffer, buffer_memory, there, width, height }
    }

    /// One frame: `picture`, the program's own, copied into the image; `draw` called inside
    /// the render pass with the command buffer; and the frame read back.
    pub fn frame(&self, device: &Device, picture: &[u32], draw: &mut dyn FnMut(Object)) -> Vec<u32> {
        let (program, commands) = (&device.program, device.commands);
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
            (program.vkCmdBeginRenderPass)(commands, &begun, 0);
            draw(commands);
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
        let (program, gpu) = (&device.program, device.device);
        // SAFETY: each is this target's.
        unsafe {
            (program.vkDestroyFramebuffer)(gpu, self.framebuffer, null());
            (program.vkDestroyRenderPass)(gpu, self.render_pass, null());
            (program.vkDestroyImageView)(gpu, self.view, null());
            (program.vkDestroyImage)(gpu, self.image, null());
            (program.vkFreeMemory)(gpu, self.memory, null());
            (program.vkDestroyBuffer)(gpu, self.buffer, null());
            (program.vkFreeMemory)(gpu, self.buffer_memory, null());
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
    ui.column().w(Length::Px(260.0)).offset(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.with_alpha(0.9)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
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
    // SAFETY: the program's own handles, which live longer than the layer.
    let mut layer = unsafe { Layer::new(&device.handles(target.render_pass)) }.unwrap_or_else(|failed| panic!("{failed}"));
    let mut ui = Ui::new(letters(), Box::new(LocalClipboard::default()));
    ui.theme.bg = Color::NONE;
    ui.resize(Size { w: width as f32, h: height as f32 }, 1.0);
    let mut app = App::default();
    // The program's own picture: a gradient.
    let picture: Vec<u32> = (0..width * height).map(|at| 0xff00_0000 | ((at % width * 255 / width) as u32) << 16 | ((at / width * 255 / height) as u32) << 8 | 0x60).collect();
    // The first frame copies all of the interface; the next, after the keyboard has come to
    // the switch and turned it on, only the places that changed.
    for event in [None, Some(Event::Key(Key::Tab, Mods::NONE)), Some(Event::Key(Key::Space, Mods::NONE))] {
        if let Some(event) = event {
            ui.event(event);
        }
        ui.run(&mut |ui| describe(&mut app, ui));
        // SAFETY: the command buffer is the program's, recording, in its render pass.
        let frame = target.frame(&device, &picture, &mut |commands| unsafe { layer.draw(&mut ui, commands, [0, 0, width as i32, height as i32]) }.unwrap_or_else(|failed| panic!("{failed}")));
        std::fs::write(&file, taut::Surface { width, height, pixels: frame }.to_png()).expect("the picture could not be written");
    }
    println!("the last frame is in {file}");
    device.wait();
    // SAFETY: the device is done with every frame.
    unsafe { layer.delete() };
    target.destroy(&device);
}
