//! Windows' Vulkan path (the feature `gpu`): a window's pixels, drawn by the CPU, shown through a
//! Vulkan swap chain on a `VK_KHR_win32_surface`.
//!
//! - A paint's strips are written to a buffer laid out as the window, and only those places
//!   are copied into an image holding the whole window (`vkCmdCopyBufferToImage`).
//! - The image is copied into the swap chain's next image and presented in step with the
//!   screen (FIFO), the changed rectangles named where the device has
//!   `VK_KHR_incremental_present`.
//! - Each frame is waited for (a fence), so the buffer is free again for the next.
//! - vulkan-1.dll is loaded when a window first asks for Vulkan (`vk`). Only the surface is
//!   Windows' own.
use crate::gpu_windows::inside;
use crate::vk::*;
use crate::windows::sys::{GetModuleHandleW, HANDLE, RECT};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::Arc;

functions!(Global {
    vkCreateInstance: fn(*const VkInstanceCreateInfo, *const c_void, *mut Object) -> i32;
});

functions!(Calls {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceProperties: fn(Object, *mut VkPhysicalDeviceProperties);
    vkGetPhysicalDeviceFeatures: fn(Object, *mut [u32; 55]);
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkGetPhysicalDeviceWin32PresentationSupportKHR: fn(Object, u32) -> u32;
    vkEnumerateDeviceExtensionProperties: fn(Object, *const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkDestroyDevice: fn(Object, *const c_void);
    vkGetDeviceQueue: fn(Object, u32, u32, *mut Object);
    vkDeviceWaitIdle: fn(Object) -> i32;
    vkCreateWin32SurfaceKHR: fn(Object, *const VkWin32SurfaceCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkDestroySurfaceKHR: fn(Object, Handle, *const c_void);
    vkGetPhysicalDeviceSurfaceSupportKHR: fn(Object, u32, Handle, *mut u32) -> i32;
    vkGetPhysicalDeviceSurfaceCapabilitiesKHR: fn(Object, Handle, *mut VkSurfaceCapabilitiesKHR) -> i32;
    vkGetPhysicalDeviceSurfaceFormatsKHR: fn(Object, Handle, *mut u32, *mut VkSurfaceFormatKHR) -> i32;
    vkCreateSwapchainKHR: fn(Object, *const VkSwapchainCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkDestroySwapchainKHR: fn(Object, Handle, *const c_void);
    vkGetSwapchainImagesKHR: fn(Object, Handle, *mut u32, *mut Handle) -> i32;
    vkAcquireNextImageKHR: fn(Object, Handle, u64, Handle, Handle, *mut u32) -> i32;
    vkQueuePresentKHR: fn(Object, *const VkPresentInfoKHR) -> i32;
    vkCreateImage: fn(Object, *const VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkCreateBuffer: fn(Object, *const VkBufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyBuffer: fn(Object, Handle, *const c_void);
    vkGetBufferMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkAllocateMemory: fn(Object, *const VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
    vkCreateCommandPool: fn(Object, *const VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkBeginCommandBuffer: fn(Object, *const VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdCopyImage: fn(Object, Handle, u32, Handle, u32, u32, *const VkImageCopy);
    vkCreateFence: fn(Object, *const VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
    vkCreateSemaphore: fn(Object, *const VkSemaphoreCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroySemaphore: fn(Object, Handle, *const c_void);
    vkQueueSubmit: fn(Object, u32, *const VkSubmitInfo, Handle) -> i32;
});

/// What a Vulkan device's objects are let go of with: its functions, the instance and the device;
/// and how its functions are found.
#[derive(Clone)]
struct Owner {
    calls: Arc<Calls>,
    #[cfg(test)]
    find: Find,
    instance: Object,
    device: Object,
}

/// The Vulkan device a program's windows are shown with, on the first GPU that presents to
/// windows: its queue, the one command buffer every frame is recorded in, its fence, and the
/// two semaphores between taking a swap chain image, copying into it and presenting it.
pub(crate) struct Device {
    owner: Owner,
    physical: Object,
    family: u32,
    queue: Object,
    pool: Handle,
    commands: Object,
    fence: Handle,
    taken: Handle,
    copied: Handle,
    memory: VkPhysicalDeviceMemoryProperties,
    /// The device has `VK_KHR_incremental_present`.
    regions: bool,
    /// Vulkan said the device was lost.
    lost: Cell<bool>,
    /// What it draws windows with, where it does (see `gpu_draw`), and whether it blends GDI's
    /// runs per channel (`dualSrcBlend`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: Option<Box<crate::gpu_draw_vulkan::Pipeline>>,
    #[cfg(feature = "gpu-draw")]
    dual: bool,
}

impl Device {
    /// A device on the first GPU that presents to windows: not a CPU device. Nothing where there
    /// is no loader or no such GPU.
    pub(crate) fn gpu() -> Option<Device> {
        #[cfg(test)]
        let find = if crate::gpu_windows::tests::faked().software { tests::swiftshader()? } else { loader()? };
        #[cfg(not(test))]
        let find = loader()?;
        let extensions = [c"VK_KHR_surface".as_ptr(), c"VK_KHR_win32_surface".as_ptr()];
        let application = VkApplicationInfo { name: c"taut".as_ptr(), api_version: 1 << 22, ..Default::default() };
        let asked = VkInstanceCreateInfo { kind: 1, application: &application, extension_count: 2, extensions: extensions.as_ptr(), ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: the description and what it points to live through the call.
        // SAFETY: with no instance, the loader gives the functions that make one.
        let global = Global::load(&mut |name| unsafe { find(null_mut(), name.as_ptr()) }).ok()?;
        if unsafe { (global.vkCreateInstance)(&asked, null(), &mut instance) } < 0 {
            return None;
        }
        // SAFETY: the instance is alive.
        let Ok(calls) = Calls::load(&mut |name| unsafe { find(instance, name.as_ptr()) }) else {
            // SAFETY: the instance was just made, and nothing else holds it.
            let destroy = unsafe { find(instance, c"vkDestroyInstance".as_ptr()) };
            if !destroy.is_null() {
                // SAFETY: as vulkan_core.h declares it.
                unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(instance, null()) };
            }
            return None;
        };
        #[cfg(test)]
        let owner = Owner { calls: Arc::new(calls), find, instance, device: null_mut() };
        #[cfg(not(test))]
        let owner = Owner { calls: Arc::new(calls), instance, device: null_mut() };
        #[cfg_attr(not(feature = "gpu-draw"), allow(unused_mut))]
        let mut device = Device::on_a_gpu(owner)?;
        // Its shaders are made here, on the device's own thread.
        #[cfg(feature = "gpu-draw")]
        {
            let (made, memory, dual) = (device.owner.device, device.memory, device.dual);
            // SAFETY: the instance is alive.
            device.drawing = crate::gpu_draw_vulkan::Pipeline::new(&mut |name| unsafe { find(instance, name.as_ptr()) }, made, &memory, dual);
        }
        Some(device)
    }

    /// The device, on the first of the system's GPUs that has a queue that draws and presents to
    /// windows, and swap chains. Lets go of the instance where there is none.
    fn on_a_gpu(mut owner: Owner) -> Option<Device> {
        let (calls, mut count, mut physical) = (owner.calls.clone(), 8, [null_mut(); 8]);
        // SAFETY: the instance is alive; the list has room for as many as is said.
        unsafe { (calls.vkEnumeratePhysicalDevices)(owner.instance, &mut count, physical.as_mut_ptr()) };
        let found = physical[..count.min(8) as usize].iter().find_map(|physical| Device::queue_on(&calls, *physical).map(|(family, regions)| (*physical, family, regions)));
        let Some((physical, family, regions)) = found else {
            // SAFETY: nothing else holds the instance.
            unsafe { (calls.vkDestroyInstance)(owner.instance, null()) };
            return None;
        };
        let names = [c"VK_KHR_swapchain".as_ptr(), c"VK_KHR_incremental_present".as_ptr()];
        let priority = 1.0f32;
        let queue = VkDeviceQueueCreateInfo { kind: 2, family, count: 1, priorities: &priority, ..Default::default() };
        let features = Device::features(&calls, physical);
        let asked =
            VkDeviceCreateInfo { kind: 3, queue_count: 1, queues: &queue, extension_count: 1 + regions as u32, extensions: names.as_ptr(), features: features.as_ptr().cast(), ..Default::default() };
        // SAFETY: the physical device is the instance's; the description lives through the call.
        if unsafe { (calls.vkCreateDevice)(physical, &asked, null(), &mut owner.device) } < 0 {
            // SAFETY: nothing else holds the instance.
            unsafe { (calls.vkDestroyInstance)(owner.instance, null()) };
            return None;
        }
        let lost = Cell::new(false);
        let mut device = Device {
            owner,
            physical,
            family,
            queue: null_mut(),
            pool: 0,
            commands: null_mut(),
            fence: 0,
            taken: 0,
            copied: 0,
            memory: Default::default(),
            regions,
            lost,
            #[cfg(feature = "gpu-draw")]
            drawing: None,
            #[cfg(feature = "gpu-draw")]
            dual: features[7] != 0,
        };
        device.with_commands().then_some(device)
    }

    /// The device features asked for: where windows are drawn (`gpu-draw`), blending with a second
    /// colour (`dualSrcBlend`, the eighth) if the GPU has it; else none.
    fn features(calls: &Calls, physical: Object) -> [u32; 55] {
        let mut wanted = [0u32; 55];
        if cfg!(feature = "gpu-draw") {
            let mut has = [0u32; 55];
            // SAFETY: a plain query of the instance's physical device.
            unsafe { (calls.vkGetPhysicalDeviceFeatures)(physical, &mut has) };
            wanted[7] = has[7];
        }
        wanted
    }

    /// The first queue family of `physical`'s that draws and presents to windows, and whether it
    /// names changed rectangles on present: if it is a GPU (not a CPU device, unless a test
    /// asks for one) with swap chains.
    fn queue_on(calls: &Calls, physical: Object) -> Option<(u32, bool)> {
        let (mut properties, mut families, mut count) = (VkPhysicalDeviceProperties::default(), [VkQueueFamilyProperties::default(); 16], 16);
        let (mut extensions, mut known) = (vec![VkExtensionProperties::default(); 512], 512);
        // SAFETY: the physical device is the instance's; each list has room for as many as is
        // said.
        unsafe {
            (calls.vkGetPhysicalDeviceProperties)(physical, &mut properties);
            (calls.vkGetPhysicalDeviceQueueFamilyProperties)(physical, &mut count, families.as_mut_ptr());
            (calls.vkEnumerateDeviceExtensionProperties)(physical, null(), &mut known, extensions.as_mut_ptr());
        }
        #[cfg(test)]
        let cpu_taken = crate::gpu_windows::tests::faked().software;
        #[cfg(not(test))]
        let cpu_taken = false;
        let has = |wanted: &[u8]| extensions[..known.min(512) as usize].iter().any(|extension| name(&extension.name) == wanted);
        if (properties.device_type == 4 && !cpu_taken) || !has(b"VK_KHR_swapchain") {
            return None;
        }
        // SAFETY: a plain query of the physical device.
        let presents = |family: &u32| families[*family as usize].flags & 1 != 0 && unsafe { (calls.vkGetPhysicalDeviceWin32PresentationSupportKHR)(physical, *family) } != 0;
        let family = (0..count.min(16)).find(presents)?;
        #[cfg(test)]
        println!("Vulkan on {}", String::from_utf8_lossy(name(&properties.name)));
        Some((family, has(b"VK_KHR_incremental_present")))
    }

    /// Makes the queue's command pool, its command buffer, the fence and the semaphores. Says
    /// whether all were made.
    fn with_commands(&mut self) -> bool {
        let (calls, device) = (&self.owner.calls, self.owner.device);
        let pooled = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family: self.family, ..Default::default() };
        // SAFETY: the device is alive; each description is whole.
        unsafe {
            (calls.vkGetDeviceQueue)(device, self.family, 0, &mut self.queue);
            (calls.vkGetPhysicalDeviceMemoryProperties)(self.physical, &mut self.memory);
            if (calls.vkCreateCommandPool)(device, &pooled, null(), &mut self.pool) < 0 {
                return false;
            }
            let asked = VkCommandBufferAllocateInfo { kind: 40, pool: self.pool, count: 1, ..Default::default() };
            (calls.vkAllocateCommandBuffers)(device, &asked, &mut self.commands);
            (calls.vkCreateFence)(device, &VkFenceCreateInfo { kind: 8, ..Default::default() }, null(), &mut self.fence);
            (calls.vkCreateSemaphore)(device, &VkSemaphoreCreateInfo { kind: 9, ..Default::default() }, null(), &mut self.taken);
            (calls.vkCreateSemaphore)(device, &VkSemaphoreCreateInfo { kind: 9, ..Default::default() }, null(), &mut self.copied);
        }
        !self.commands.is_null() && self.fence != 0 && self.taken != 0 && self.copied != 0
    }

    /// Whether Vulkan said the device was lost.
    pub(crate) fn removed(&self) -> bool {
        self.lost.get()
    }

    /// Whether a Vulkan call succeeded; notes a lost device.
    fn succeeded(&self, result: i32) -> bool {
        if result == DEVICE_LOST {
            self.lost.set(true);
        }
        result >= 0
    }

    /// Memory for something with these needs, of a kind that is `wanted`; nothing where the
    /// device has none.
    fn memory(&self, needs: VkMemoryRequirements, wanted: u32) -> Option<Handle> {
        let kind = memory_type(&self.memory, needs.type_bits, wanted)?;
        let (asked, mut memory) = (VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: kind, ..Default::default() }, 0);
        // SAFETY: the device is alive, and the description whole.
        (unsafe { (self.owner.calls.vkAllocateMemory)(self.owner.device, &asked, null(), &mut memory) } >= 0).then_some(memory)
    }

    /// A buffer of `bytes` for `usage`, in memory the CPU writes and reads, and where the CPU
    /// sees it.
    fn buffer(&self, bytes: usize, usage: u32) -> Option<(Handle, Handle, *mut u8)> {
        let (calls, device) = (&self.owner.calls, self.owner.device);
        let made = VkBufferCreateInfo { kind: 12, size: bytes as u64, usage, ..Default::default() };
        let (mut buffer, mut needs, mut there) = (0, VkMemoryRequirements::default(), null_mut());
        // SAFETY: the device is alive; the buffer is made, given memory, and the memory mapped.
        unsafe {
            if (calls.vkCreateBuffer)(device, &made, null(), &mut buffer) < 0 {
                return None;
            }
            (calls.vkGetBufferMemoryRequirements)(device, buffer, &mut needs);
            let Some(memory) = self.memory(needs, HOST_VISIBLE_AND_COHERENT) else {
                (calls.vkDestroyBuffer)(device, buffer, null());
                return None;
            };
            (calls.vkBindBufferMemory)(device, buffer, memory, 0);
            (calls.vkMapMemory)(device, memory, 0, u64::MAX, 0, &mut there);
            Some((buffer, memory, there.cast()))
        }
    }

    /// An image `size` pixels, B8G8R8A8, in the device's own memory, to copy to and from.
    fn image(&self, size: [i32; 2]) -> Option<(Handle, Handle)> {
        let (calls, device) = (&self.owner.calls, self.owner.device);
        let extent = [size[0] as u32, size[1] as u32, 1];
        let usage = USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION | self.drawn();
        let made = VkImageCreateInfo { kind: 14, image_type: 1, format: B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
        let (mut image, mut needs) = (0, VkMemoryRequirements::default());
        // SAFETY: the device is alive; the image is made and given memory.
        unsafe {
            if (calls.vkCreateImage)(device, &made, null(), &mut image) < 0 {
                return None;
            }
            (calls.vkGetImageMemoryRequirements)(device, image, &mut needs);
            let Some(memory) = self.memory(needs, DEVICE_LOCAL) else {
                (calls.vkDestroyImage)(device, image, null());
                return None;
            };
            (calls.vkBindImageMemory)(device, image, memory, 0);
            Some((image, memory))
        }
    }

    /// What a window's image is used for besides copies: drawn into, where the device draws
    /// windows (`gpu-draw`).
    #[inline]
    fn drawn(&self) -> u32 {
        #[cfg(feature = "gpu-draw")]
        if self.drawing.is_some() {
            return crate::gpu_draw_vulkan::USAGE_DRAWN_INTO;
        }
        0
    }

    /// Hands the recorded command buffer to the queue, waiting on `wait` (a semaphore, or 0) and
    /// signalling `signal`, and says whether it was taken.
    fn submit(&self, wait: Handle, signal: Handle) -> bool {
        let stage = STAGE_TRANSFER;
        let submit = VkSubmitInfo {
            kind: 4,
            wait_count: (wait != 0) as u32,
            waits: &wait,
            wait_stages: &stage,
            buffer_count: 1,
            buffers: &self.commands,
            signal_count: (signal != 0) as u32,
            signals: &signal,
            ..Default::default()
        };
        // SAFETY: the buffer was recorded; the fence is this device's and not in use.
        unsafe { self.succeeded((self.owner.calls.vkEndCommandBuffer)(self.commands)) && self.succeeded((self.owner.calls.vkQueueSubmit)(self.queue, 1, &submit, self.fence)) }
    }

    /// Waits for the last submit (two seconds at most), and readies the fence for the next.
    fn wait(&self) -> bool {
        let (calls, device) = (&self.owner.calls, self.owner.device);
        // SAFETY: the fence is this device's.
        unsafe { self.succeeded((calls.vkWaitForFences)(device, 1, &self.fence, 1, 2_000_000_000)) && self.succeeded((calls.vkResetFences)(device, 1, &self.fence)) }
    }

    /// Starts recording the command buffer. (The device is done with it: each submit is waited
    /// for.)
    fn begin(&self) -> bool {
        // SAFETY: as above; recorded once, then submitted.
        self.succeeded(unsafe { (self.owner.calls.vkBeginCommandBuffer)(self.commands, &VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() }) })
    }
}

// SAFETY: the device and what it holds belong to no thread, and are used by one at a time: made
// on a thread of their own, then handed whole to the windows' thread.
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the device is alive; what draws goes first, once it is idle.
        #[cfg(feature = "gpu-draw")]
        unsafe {
            (self.owner.calls.vkDeviceWaitIdle)(self.owner.device);
            drop(self.drawing.take());
        }
        let (calls, device) = (&self.owner.calls, self.owner.device);
        // SAFETY: the device is done with everything (it is waited for first); nothing is let go
        // of where nothing was made (a handle of 0).
        unsafe {
            (calls.vkDeviceWaitIdle)(device);
            (calls.vkDestroySemaphore)(device, self.copied, null());
            (calls.vkDestroySemaphore)(device, self.taken, null());
            (calls.vkDestroyFence)(device, self.fence, null());
            (calls.vkDestroyCommandPool)(device, self.pool, null());
            (calls.vkDestroyDevice)(device, null());
            (calls.vkDestroyInstance)(self.owner.instance, null());
        }
    }
}

/// A window's surface and swap chain, the image holding all of the window's pixels, and the
/// buffer the changed ones go through, laid out as the window.
pub(crate) struct Shown {
    owner: Owner,
    surface: Handle,
    chain: Handle,
    images: Vec<Handle>,
    /// The swap chain's size, and what its images are used for.
    extent: [u32; 2],
    usage: u32,
    picture: Handle,
    picture_memory: Handle,
    buffer: Handle,
    buffer_memory: Handle,
    written: *mut u8,
    waiting: Vec<VkBufferImageCopy>,
    /// The picture holds nothing yet.
    fresh: bool,
    size: [i32; 2],
    #[cfg(test)]
    hwnd: HANDLE,
}

impl Shown {
    /// A surface for window `hwnd` that `device`'s queue presents to, taking B8G8R8A8 images;
    /// nothing where there is none.
    pub(crate) fn new(device: &Device, hwnd: HANDLE) -> Option<Shown> {
        let calls = &device.owner.calls;
        // SAFETY: a plain query of this program's own handle.
        let instance = unsafe { GetModuleHandleW(null()) };
        let described = VkWin32SurfaceCreateInfoKHR { kind: 1_000_009_000, instance, hwnd, ..Default::default() };
        let mut shown = Shown {
            owner: device.owner.clone(),
            surface: 0,
            chain: 0,
            images: Vec::new(),
            extent: [0; 2],
            usage: 0,
            picture: 0,
            picture_memory: 0,
            buffer: 0,
            buffer_memory: 0,
            written: null_mut(),
            waiting: Vec::new(),
            fresh: true,
            size: [0; 2],
            #[cfg(test)]
            hwnd,
        };
        let (mut presents, mut formats, mut count) = (0, [VkSurfaceFormatKHR::default(); 64], 64);
        // SAFETY: the window is alive; the queries are of the device's own GPU and family.
        unsafe {
            if (calls.vkCreateWin32SurfaceKHR)(device.owner.instance, &described, null(), &mut shown.surface) < 0 {
                return None;
            }
            (calls.vkGetPhysicalDeviceSurfaceSupportKHR)(device.physical, device.family, shown.surface, &mut presents);
            (calls.vkGetPhysicalDeviceSurfaceFormatsKHR)(device.physical, shown.surface, &mut count, formats.as_mut_ptr());
        }
        let bgra = formats[..count.min(64) as usize].iter().any(|format| format.format == B8G8R8A8_UNORM && format.colour_space == 0);
        (presents != 0 && bgra).then_some(shown)
    }

    /// Makes the swap chain, the picture and the buffer `size` pixels where they are not. Says
    /// whether they were remade (the window is then to be drawn whole); nothing if that failed.
    pub(crate) fn fit(&mut self, device: &Device, size: [i32; 2]) -> Option<bool> {
        if size == self.size {
            return Some(false);
        }
        self.let_go_of_picture();
        self.size = [0; 2];
        if !self.chained(device, size) {
            return None;
        }
        (self.picture, self.picture_memory) = device.image(size)?;
        (self.buffer, self.buffer_memory, self.written) = device.buffer(size[0] as usize * size[1] as usize * 4, USAGE_TRANSFER_SOURCE)?;
        (self.size, self.fresh) = (size, true);
        Some(true)
    }

    /// A new swap chain, `size` pixels unless the surface says its own, in place of the old one.
    /// Says whether it was made.
    fn chained(&mut self, device: &Device, size: [i32; 2]) -> bool {
        let calls = &self.owner.calls;
        let mut can = VkSurfaceCapabilitiesKHR::default();
        // SAFETY: the surface is this window's, and the device's GPU presents to it.
        if !device.succeeded(unsafe { (calls.vkGetPhysicalDeviceSurfaceCapabilitiesKHR)(device.physical, self.surface, &mut can) }) || can.usage & USAGE_TRANSFER_DESTINATION == 0 {
            return false;
        }
        let extent = if can.current[0] == u32::MAX { size.map(|side| side as u32) } else { can.current };
        let images = if can.max_images == 0 { can.min_images.max(2) } else { can.min_images.max(2).min(can.max_images) };
        // Opaque if it may be (1), else whatever the surface has.
        let alpha = if can.alphas & 1 != 0 { 1 } else { can.alphas & can.alphas.wrapping_neg() };
        let usage = USAGE_TRANSFER_DESTINATION | (can.usage & USAGE_TRANSFER_SOURCE);
        let (old, mut chain) = (self.chain, 0);
        let asked = VkSwapchainCreateInfoKHR {
            kind: 1_000_001_000,
            surface: self.surface,
            min_images: images,
            format: B8G8R8A8_UNORM,
            extent,
            layers: 1,
            usage,
            transform: can.transform,
            alpha,
            present_mode: 2,
            old,
            ..Default::default()
        };
        let (mut count, mut made) = (16, [0; 16]);
        // SAFETY: the device is idle; the old swap chain is let go of once the new one is made.
        unsafe {
            (calls.vkDeviceWaitIdle)(self.owner.device);
            let result = (calls.vkCreateSwapchainKHR)(self.owner.device, &asked, null(), &mut chain);
            (calls.vkDestroySwapchainKHR)(self.owner.device, old, null());
            self.chain = 0;
            if !device.succeeded(result) {
                return false;
            }
            (calls.vkGetSwapchainImagesKHR)(self.owner.device, chain, &mut count, made.as_mut_ptr());
        }
        (self.chain, self.images, self.extent, self.usage) = (chain, made[..count.min(16) as usize].to_vec(), extent, usage);
        true
    }

    /// Writes `size` pixels at `at` (cut to the picture), read from `first`, rows `pitch` pixels
    /// apart, to the buffer, where they are in the window; the next present copies them on.
    ///
    /// # Safety
    /// The pixels read lie at `first`: as many rows as given, each as wide.
    pub(crate) unsafe fn upload(&mut self, at: [i32; 2], size: [i32; 2], first: *const u32, pitch: usize) {
        let [right, bottom] = [(at[0] + size[0]).min(self.size[0]), (at[1] + size[1]).min(self.size[1])];
        if self.written.is_null() || at[0] < 0 || at[1] < 0 || right <= at[0] || bottom <= at[1] {
            return;
        }
        let (wide, [x, y], [width, height]) = (self.size[0] as usize, at.map(|side| side as usize), [right - at[0], bottom - at[1]].map(|side| side as usize));
        for line in 0..height {
            // SAFETY: the rows read lie where the caller promises; the buffer holds the window.
            unsafe { std::ptr::copy_nonoverlapping(first.add(line * pitch), self.written.cast::<u32>().add((y + line) * wide + x), width) };
        }
        let extent = [width as u32, height as u32, 1];
        self.waiting.push(VkBufferImageCopy { buffer_offset: ((y * wide + x) * 4) as u64, row_length: wide as u32, layers: LAYER, offset: [x as i32, y as i32, 0], extent, ..Default::default() });
    }

    /// Draws a recorded paint into the picture, inside `area` (left, top, right, bottom), after
    /// what is waiting is copied in; then waits for the GPU. Says whether it could: if not, the
    /// CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, device: &mut Device, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        let Some(mut drawing) = device.drawing.take() else { return false };
        let drawn = self.picture != 0 && device.begin() && self.draw_with(device, &mut drawing, list, area) && device.submit(0, 0) && device.wait();
        drawing.forget_target();
        device.drawing = Some(drawing);
        drawn
    }

    /// Records what is waiting copied into the picture, then the paint drawn into it, which is
    /// left ready to be copied from. Says whether it could be.
    #[cfg(feature = "gpu-draw")]
    fn draw_with(&mut self, device: &Device, drawing: &mut crate::gpu_draw_vulkan::Pipeline, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        let (calls, commands) = (&self.owner.calls, device.commands);
        let before = if self.fresh { (LAYOUT_UNDEFINED, 0) } else { (LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_READ) };
        let to_copy = barrier(self.picture, before.0, LAYOUT_TRANSFER_DESTINATION, before.1, ACCESS_TRANSFER_WRITE);
        // SAFETY: the command buffer is being recorded; the places lie in the buffer and the
        // picture.
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            if !self.waiting.is_empty() {
                (calls.vkCmdCopyBufferToImage)(commands, self.buffer, self.picture, LAYOUT_TRANSFER_DESTINATION, self.waiting.len() as u32, self.waiting.as_ptr());
            }
        }
        let was = crate::gpu_draw_vulkan::Was { layout: LAYOUT_TRANSFER_DESTINATION, access: ACCESS_TRANSFER_WRITE, stage: STAGE_TRANSFER };
        (self.fresh, self.waiting) = (false, Vec::new());
        drawing.record(commands, self.picture, self.size, list, area, was, LAYOUT_TRANSFER_SOURCE)
    }

    /// Copies what is waiting into the picture, the picture into the swap chain's next image, and
    /// presents it, `dirty` being what changed, at the screen's next refresh; then waits for the
    /// GPU. A swap chain out of date is made again and the picture presented whole. Says whether
    /// it worked.
    pub(crate) fn present(&mut self, device: &Device, dirty: &[RECT]) -> bool {
        if self.chain == 0 {
            return true;
        }
        match self.present_once(device, inside(dirty, self.size)) {
            Some(true) => true,
            Some(false) => self.chained(device, self.size) && self.present_once(device, &[]) == Some(true),
            None => false,
        }
    }

    /// One present: whether it worked; `Some(false)` where the swap chain is out of date.
    fn present_once(&mut self, device: &Device, dirty: &[RECT]) -> Option<bool> {
        let (calls, mut at) = (self.owner.calls.clone(), 0);
        // SAFETY: the swap chain is this window's; the semaphore is free (every frame is waited for).
        let taken = unsafe { (calls.vkAcquireNextImageKHR)(self.owner.device, self.chain, 1_000_000_000, device.taken, 0, &mut at) };
        if taken == OUT_OF_DATE {
            return Some(false);
        }
        if !device.succeeded(taken) || taken == 1 || taken == 2 || !device.begin() {
            return None;
        }
        let image = *self.images.get(at as usize)?;
        #[cfg(test)]
        tests::ready(device, self);
        self.record(device, image);
        if !device.submit(device.taken, device.copied) {
            return None;
        }
        let rects: Vec<VkRectLayerKHR> =
            dirty.iter().map(|part| VkRectLayerKHR { offset: [part.left, part.top], extent: [(part.right - part.left) as u32, (part.bottom - part.top) as u32], layer: 0 }).collect();
        let region = VkPresentRegionKHR { count: rects.len() as u32, rects: rects.as_ptr() };
        let regions = VkPresentRegionsKHR { kind: 1_000_084_000, count: 1, regions: &region, ..Default::default() };
        let next = if device.regions && !rects.is_empty() { (&regions as *const VkPresentRegionsKHR).cast() } else { null() };
        let shown = VkPresentInfoKHR { kind: 1_000_001_001, next, wait_count: 1, waits: &device.copied, count: 1, swapchains: &self.chain, indices: &at, ..Default::default() };
        // SAFETY: the image was taken from this swap chain, and is copied into before this.
        let presented = unsafe { (calls.vkQueuePresentKHR)(device.queue, &shown) };
        let done = device.wait();
        #[cfg(test)]
        tests::read(self, dirty);
        match presented {
            OUT_OF_DATE => Some(false),
            _ => Some(device.succeeded(presented) && done),
        }
    }

    /// Records the frame: the waiting places copied from the buffer into the picture, and the
    /// picture into the swap chain image `image`, made ready to present.
    fn record(&mut self, device: &Device, image: Handle) {
        let (calls, commands) = (&self.owner.calls, device.commands);
        let [width, height] = [self.extent[0].min(self.size[0] as u32), self.extent[1].min(self.size[1] as u32)];
        let whole = VkImageCopy { source_layers: LAYER, destination_layers: LAYER, extent: [width, height, 1], ..Default::default() };
        let before = if self.fresh { (LAYOUT_UNDEFINED, 0) } else { (LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_READ) };
        let to_copy =
            [barrier(self.picture, before.0, LAYOUT_TRANSFER_DESTINATION, before.1, ACCESS_TRANSFER_WRITE), barrier(image, LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE)];
        let to_read = barrier(self.picture, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_READ);
        let to_present = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        // SAFETY: the command buffer is being recorded; the places lie in the buffer and the
        // picture, and the copy in both images.
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 2, to_copy.as_ptr());
            if !self.waiting.is_empty() {
                (calls.vkCmdCopyBufferToImage)(commands, self.buffer, self.picture, LAYOUT_TRANSFER_DESTINATION, self.waiting.len() as u32, self.waiting.as_ptr());
            }
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
            (calls.vkCmdCopyImage)(commands, self.picture, LAYOUT_TRANSFER_SOURCE, image, LAYOUT_TRANSFER_DESTINATION, 1, &whole);
            #[cfg(test)]
            tests::record(self, commands, image);
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &to_present);
        }
        (self.fresh, self.waiting) = (false, Vec::new());
    }

    /// Lets go of the picture and the buffer, and their memory. (The device is idle first.)
    fn let_go_of_picture(&mut self) {
        let (calls, device) = (&self.owner.calls, self.owner.device);
        // SAFETY: each is this window's; nothing is let go of where nothing was made.
        unsafe {
            (calls.vkDeviceWaitIdle)(device);
            (calls.vkDestroyImage)(device, std::mem::take(&mut self.picture), null());
            (calls.vkFreeMemory)(device, std::mem::take(&mut self.picture_memory), null());
            (calls.vkDestroyBuffer)(device, std::mem::take(&mut self.buffer), null());
            (calls.vkFreeMemory)(device, std::mem::take(&mut self.buffer_memory), null());
        }
        (self.written, self.waiting) = (null_mut(), Vec::new());
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        self.let_go_of_picture();
        let calls = &self.owner.calls;
        // SAFETY: the device is idle; both are this window's.
        unsafe {
            (calls.vkDestroySwapchainKHR)(self.owner.device, self.chain, null());
            (calls.vkDestroySurfaceKHR)(self.owner.instance, self.surface, null());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    functions!(Readback {
        vkCmdCopyImageToBuffer: fn(Object, Handle, u32, Handle, u32, *const VkBufferImageCopy);
    });

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
        fn GetProcAddress(library: isize, name: *const u8) -> usize;
    }

    /// SwiftShader, Vulkan on the CPU, as Microsoft Edge carries it; nothing where there is none.
    pub(crate) fn swiftshader_file() -> Option<std::path::PathBuf> {
        let places = [r"C:\Program Files (x86)\Microsoft\Edge\Application", r"C:\Program Files (x86)\Microsoft\EdgeCore", r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application"];
        let versions = places.iter().filter_map(|place| std::fs::read_dir(place).ok()).flatten().flatten();
        versions.map(|version| version.path().join("vk_swiftshader.dll")).find(|file| file.is_file())
    }

    /// SwiftShader's own `vkGetInstanceProcAddr`, used in place of the system's loader: a
    /// loader in an elevated program takes no driver named in its environment.
    pub(crate) fn swiftshader() -> Option<Find> {
        let file: Vec<u16> = swiftshader_file()?.as_os_str().encode_wide().chain([0]).collect();
        // 0x8: `LOAD_WITH_ALTERED_SEARCH_PATH`, what it needs found beside it.
        let found = unsafe { GetProcAddress(LoadLibraryExW(file.as_ptr(), 0, 0x8), c"vkGetInstanceProcAddr".as_ptr().cast()) };
        (found != 0).then(|| unsafe { std::mem::transmute::<usize, Find>(found) })
    }

    thread_local! {
        /// The buffer a present's image is copied into, while a test keeps them: it, its memory,
        /// where the CPU sees it.
        static KEEP: Cell<Option<(Handle, Handle, *mut u8)>> = const { Cell::new(None) };
    }

    /// Makes the buffer the next present's image is copied into, if a test asked for copies and
    /// the swap chain's images can be copied from.
    pub(crate) fn ready(device: &Device, shown: &Shown) {
        if crate::gpu_windows::tests::faked().keep && shown.usage & USAGE_TRANSFER_SOURCE != 0 {
            let made = device.buffer(shown.extent[0] as usize * shown.extent[1] as usize * 4, USAGE_TRANSFER_DESTINATION).expect("no buffer to read back");
            KEEP.set(Some(made));
        }
    }

    /// Records a copy of the swap chain image into the buffer read back, if there is one: between
    /// its copy from the picture and its present.
    pub(crate) fn record(shown: &Shown, commands: Object, image: Handle) {
        let Some((buffer, ..)) = KEEP.get() else { return };
        let (calls, owner) = (&shown.owner.calls, &shown.owner);
        let readback = Readback::load(&mut |name| unsafe { (owner.find)(owner.instance, name.as_ptr()) }).expect("no vkCmdCopyImageToBuffer");
        let whole = VkBufferImageCopy { row_length: shown.extent[0], layers: LAYER, extent: [shown.extent[0], shown.extent[1], 1], ..Default::default() };
        let to_read = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_READ);
        let back = barrier(image, LAYOUT_TRANSFER_SOURCE, LAYOUT_TRANSFER_DESTINATION, ACCESS_TRANSFER_READ, ACCESS_TRANSFER_WRITE);
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
            (readback.vkCmdCopyImageToBuffer)(commands, image, LAYOUT_TRANSFER_SOURCE, buffer, 1, &whole);
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER | 0x4000, 0, 0, null(), 0, null(), 1, &back);
        }
    }

    /// Reads the copy, once the GPU is done, keeps it, and lets go of the buffer.
    pub(crate) fn read(shown: &Shown, dirty: &[RECT]) {
        let Some((buffer, memory, there)) = KEEP.take() else { return };
        let [width, height] = shown.extent.map(|side| side as usize);
        let pixels = unsafe { std::slice::from_raw_parts(there.cast::<u32>(), width * height) }.to_vec();
        unsafe {
            (shown.owner.calls.vkDestroyBuffer)(shown.owner.device, buffer, null());
            (shown.owner.calls.vkFreeMemory)(shown.owner.device, memory, null());
        }
        crate::gpu_windows::tests::kept(shown.hwnd, width, pixels, dirty);
    }
}
