//! What windows shown through Vulkan share on X11 and Android (the feature `gpu`): the instance,
//! one device and queue, and each window's swap chain, its pixels (a buffer the CPU writes and
//! the GPU reads) and the commands of its frames. `gpu_vulkan_x11` and `gpu_vulkan_android` add
//! their surface, how they choose a device, and how a frame is recorded; what they keep of their
//! own is `Gpu::system` and `Shows::system`.
//!
//! The loader is opened when a window first asks for Vulkan (`vk::loader`).
use crate::vk::*;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{null, null_mut};

functions!(Global {
    vkCreateInstance: fn(*const VkInstanceCreateInfo, *const c_void, *mut Object) -> i32;
    vkEnumerateInstanceExtensionProperties: fn(*const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
});

functions!(InstanceCalls {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceProperties: fn(Object, *mut VkPhysicalDeviceProperties);
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkEnumerateDeviceExtensionProperties: fn(Object, *const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkGetDeviceProcAddr: fn(Object, *const c_char) -> *const c_void;
    vkDestroySurfaceKHR: fn(Object, Handle, *const c_void);
    vkGetPhysicalDeviceSurfaceSupportKHR: fn(Object, u32, Handle, *mut u32) -> i32;
    vkGetPhysicalDeviceSurfaceCapabilitiesKHR: fn(Object, Handle, *mut VkSurfaceCapabilitiesKHR) -> i32;
    vkGetPhysicalDeviceSurfaceFormatsKHR: fn(Object, Handle, *mut u32, *mut VkSurfaceFormatKHR) -> i32;
});

functions!(DeviceCalls {
    vkDestroyDevice: fn(Object, *const c_void);
    vkGetDeviceQueue: fn(Object, u32, u32, *mut Object);
    vkDeviceWaitIdle: fn(Object) -> i32;
    vkQueueWaitIdle: fn(Object) -> i32;
    vkQueueSubmit: fn(Object, u32, *const VkSubmitInfo, Handle) -> i32;
    vkCreateSwapchainKHR: fn(Object, *const VkSwapchainCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkDestroySwapchainKHR: fn(Object, Handle, *const c_void);
    vkGetSwapchainImagesKHR: fn(Object, Handle, *mut u32, *mut Handle) -> i32;
    vkAcquireNextImageKHR: fn(Object, Handle, u64, Handle, Handle, *mut u32) -> i32;
    vkQueuePresentKHR: fn(Object, *const VkPresentInfoKHR) -> i32;
    vkCreateBuffer: fn(Object, *const VkBufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyBuffer: fn(Object, Handle, *const c_void);
    vkGetBufferMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkAllocateMemory: fn(Object, *const VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
    vkCreateCommandPool: fn(Object, *const VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkFreeCommandBuffers: fn(Object, Handle, u32, *const Object);
    vkBeginCommandBuffer: fn(Object, *const VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCreateSemaphore: fn(Object, *const VkSemaphoreCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroySemaphore: fn(Object, Handle, *const c_void);
    vkCreateFence: fn(Object, *const VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
});

/// Vulkan 1.1, as `VK_MAKE_API_VERSION` makes it.
pub(crate) const VULKAN_1_1: u32 = (1 << 22) | (1 << 12);

/// The instance of Vulkan, with the functions it offers.
pub(crate) struct Vulkan {
    pub(crate) instance: Object,
    pub(crate) calls: InstanceCalls,
    find: Find,
    /// It is never destroyed: Mesa's dozen (Vulkan on Direct3D 12, WSL's) crashes at thread exit
    /// after it.
    pub(crate) lasting: bool,
}

/// The GPU a program's windows are shown through: one device and queue, a pool of commands,
/// and what the system keeps (`system`).
pub(crate) struct Gpu<S> {
    pub(crate) vulkan: Vulkan,
    pub(crate) calls: DeviceCalls,
    pub(crate) physical: Object,
    /// The queue family it was made with: one that copies (and on Android draws) and presents.
    pub(crate) family: u32,
    pub(crate) device: Object,
    pub(crate) queue: Object,
    pool: Handle,
    /// The kinds of memory the CPU writes and the GPU reads, one bit each.
    writable: u32,
    /// Presents name the places that changed.
    pub(crate) regions: bool,
    /// How long a frame may wait for an image or the GPU before the device is taken for lost, in
    /// nanoseconds.
    waited: u64,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// Where the GPU draws windows too, what it records to, and what it draws with: not yet
    /// tried, or what that gave (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pub(crate) pipeline: Option<Option<Box<crate::gpu_draw_vulkan::Pipeline>>>,
    /// Tests: every frame from now on says the device was lost.
    #[cfg(test)]
    pub(crate) faked: bool,
    /// What the system keeps: its surfaces' functions, and more. (Let go of after the instance.)
    pub(crate) system: S,
}

/// A window shown through Vulkan: its surface and swap chain, its pixels, the commands of its
/// frames, and what the system keeps of it (`system`).
pub(crate) struct Shows<W> {
    pub(crate) surface: Handle,
    pub(crate) swapchain: Handle,
    pub(crate) images: Vec<Image>,
    /// Signalled once an image is had.
    pub(crate) acquired: Handle,
    /// The pixels, `size` big, in a buffer the CPU writes (mapped at `pixels`).
    pub(crate) size: [i32; 2],
    pub(crate) buffer: Handle,
    memory: Handle,
    pub(crate) pixels: *mut u32,
    /// The commands of a frame, and the fence of their last run, not yet waited for if `busy`.
    pub(crate) commands: Object,
    fence: Handle,
    busy: bool,
    /// The swap chain no longer fits the window: it is made again before the next frame.
    pub(crate) remake: bool,
    /// What the system keeps of the window.
    pub(crate) system: W,
}

/// An image of a swap chain, and the semaphore signalled once its frame is done.
#[derive(Clone, Copy)]
pub(crate) struct Image {
    pub(crate) image: Handle,
    pub(crate) drawn: Handle,
}

/// What a swap chain is made of: its images' format and colour space, size, transform and
/// alpha, and how many images at least.
pub(crate) struct Chain {
    pub(crate) format: VkSurfaceFormatKHR,
    pub(crate) extent: [u32; 2],
    pub(crate) transform: u32,
    pub(crate) alpha: u32,
    pub(crate) images: u32,
}

impl Vulkan {
    /// Opens the loader and makes an instance with `extensions` (`VK_KHR_surface` and the
    /// system's own); and the Vulkan version it was made for. Nothing where any of it is missing.
    pub(crate) fn open(extensions: &[&CStr]) -> Option<(Vulkan, u32)> {
        let find = loader()?;
        // SAFETY: with no instance, the loader gives the functions that make one.
        let global = Global::load(&mut |name| unsafe { find(null_mut(), name.as_ptr()) }).ok()?;
        // SAFETY: Vulkan's documented enumeration.
        let offers = |name: &&CStr| offered(|count, list| unsafe { (global.vkEnumerateInstanceExtensionProperties)(null(), count, list) }, name);
        if !extensions.iter().all(offers) {
            return None;
        }
        let names: Vec<*const c_char> = extensions.iter().map(|name| name.as_ptr()).collect();
        let (instance, api) = instance(&global, &names)?;
        // SAFETY: the instance was just made.
        let Ok(calls) = InstanceCalls::load(&mut |name| unsafe { find(instance, name.as_ptr()) }) else {
            // SAFETY: as above; nothing of it is used after.
            let destroy = unsafe { find(instance, c"vkDestroyInstance".as_ptr()) };
            if !destroy.is_null() {
                unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(instance, null()) };
            }
            return None;
        };
        Some((Vulkan { instance, calls, find, lasting: false }, api))
    }

    /// One of the instance's functions by name (null where it has none).
    pub(crate) fn function(&self, name: &CStr) -> *const c_void {
        // SAFETY: the instance is alive; the name ends in a zero.
        unsafe { (self.find)(self.instance, name.as_ptr()) }
    }

    /// The system's devices.
    pub(crate) fn devices(&self) -> Vec<Object> {
        // SAFETY: Vulkan's documented enumeration.
        listed(null_mut(), |count, list| unsafe { (self.calls.vkEnumeratePhysicalDevices)(self.instance, count, list) })
    }

    /// The queue families of device `physical`.
    pub(crate) fn families(&self, physical: Object) -> Vec<VkQueueFamilyProperties> {
        listed(VkQueueFamilyProperties::default(), |count, list| {
            // SAFETY: Vulkan's documented enumeration, of the instance's device.
            unsafe { (self.calls.vkGetPhysicalDeviceQueueFamilyProperties)(physical, count, list) };
            0
        })
    }

    /// Whether device `physical` offers extension `name`.
    pub(crate) fn offers(&self, physical: Object, name: &CStr) -> bool {
        // SAFETY: Vulkan's documented enumeration, of the instance's device.
        offered(|count, list| unsafe { (self.calls.vkEnumerateDeviceExtensionProperties)(physical, null(), count, list) }, name)
    }

    /// What device `physical` is.
    pub(crate) fn properties(&self, physical: Object) -> VkPhysicalDeviceProperties {
        let mut properties = VkPhysicalDeviceProperties::default();
        // SAFETY: Vulkan fills in the properties.
        unsafe { (self.calls.vkGetPhysicalDeviceProperties)(physical, &mut properties) };
        properties
    }

    /// Lets go of a surface.
    pub(crate) fn destroy(&self, surface: Handle) {
        // SAFETY: the surface is the instance's, and nothing uses it after; 0 is let be.
        unsafe { (self.calls.vkDestroySurfaceKHR)(self.instance, surface, null()) };
    }
}

impl Drop for Vulkan {
    /// Lets go of the instance, unless `lasting`. (Surfaces and devices went first.)
    fn drop(&mut self) {
        if !self.lasting {
            // SAFETY: nothing made from it is used after.
            unsafe { (self.calls.vkDestroyInstance)(self.instance, null()) };
        }
    }
}

/// An instance with `extensions`, of Vulkan 1.1 where the loader takes it, else 1.0; and which.
fn instance(global: &Global, extensions: &[*const c_char]) -> Option<(Object, u32)> {
    [VULKAN_1_1, 1 << 22].into_iter().find_map(|api| {
        let application = VkApplicationInfo { kind: 0, api_version: api, ..Default::default() };
        let asked = VkInstanceCreateInfo { kind: 1, application: &application, extension_count: extensions.len() as u32, extensions: extensions.as_ptr(), ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: Vulkan's documented call; the names end in zeros.
        (unsafe { (global.vkCreateInstance)(&asked, null(), &mut instance) } == 0).then_some((instance, api))
    })
}

impl<S> Gpu<S> {
    /// A device on `physical`, with queue 0 of `family` and a pool of commands; frames wait
    /// `waited` nanoseconds at most. What the system keeps is made by `system`, given how to find
    /// the device's functions. Nothing where any of it can't be made.
    pub(crate) fn make(vulkan: Vulkan, (physical, family): (Object, u32), waited: u64, system: impl FnOnce(&mut dyn FnMut(&CStr) -> *const c_void) -> Option<S>) -> Option<Gpu<S>> {
        let regions = vulkan.offers(physical, c"VK_KHR_incremental_present");
        let extensions = [c"VK_KHR_swapchain".as_ptr(), c"VK_KHR_incremental_present".as_ptr()];
        let priority = 1.0f32;
        let queue = VkDeviceQueueCreateInfo { kind: 2, family, count: 1, priorities: &priority, ..Default::default() };
        let count = if regions { 2 } else { 1 };
        let asked = VkDeviceCreateInfo { kind: 3, queue_count: 1, queues: &queue, extension_count: count, extensions: extensions.as_ptr(), ..Default::default() };
        let (mut device, mut memory) = (null_mut(), VkPhysicalDeviceMemoryProperties::default());
        // SAFETY: Vulkan fills in the device's memory, and makes the device; the names end in
        // zeros.
        unsafe {
            (vulkan.calls.vkGetPhysicalDeviceMemoryProperties)(physical, &mut memory);
            if (vulkan.calls.vkCreateDevice)(physical, &asked, null(), &mut device) != 0 {
                // (The instance goes before what the system keeps, which `system` holds.)
                drop(vulkan);
                return None;
            }
        }
        let find = vulkan.calls.vkGetDeviceProcAddr;
        // SAFETY: the device was just made.
        let mut found = |name: &CStr| unsafe { find(device, name.as_ptr()) };
        let Ok(calls) = DeviceCalls::load(&mut found) else { return Gpu::unmade(find, device, vulkan) };
        let Some(system) = system(&mut found) else { return Gpu::unmade(find, device, vulkan) };
        let mut gpu = Gpu {
            vulkan,
            calls,
            physical,
            family,
            device,
            queue: null_mut(),
            pool: 0,
            writable: kinds(&memory, HOST_VISIBLE_AND_COHERENT),
            regions,
            waited,
            lost: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
            #[cfg(test)]
            faked: false,
            system,
        };
        let pool = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family, ..Default::default() };
        // SAFETY: the device's queue 0 of the family it was made with; and a pool for it.
        let made = unsafe {
            (gpu.calls.vkGetDeviceQueue)(device, family, 0, &mut gpu.queue);
            (gpu.calls.vkCreateCommandPool)(device, &pool, null(), &mut gpu.pool)
        };
        (made == 0).then_some(gpu)
    }

    /// Lets go of a device whose functions could not all be found, then of its instance:
    /// nothing.
    fn unmade(find: Find, device: Object, vulkan: Vulkan) -> Option<Gpu<S>> {
        // SAFETY: the device was just made and is not used after.
        let destroy = unsafe { find(device, c"vkDestroyDevice".as_ptr()) };
        if !destroy.is_null() {
            unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(device, null()) };
        }
        drop(vulkan);
        None
    }

    /// Tests: the device's name, from Vulkan (as "llvmpipe (LLVM 21.1.8, 256 bits)").
    #[cfg(test)]
    pub(crate) fn name(&self) -> String {
        let properties = self.vulkan.properties(self.physical);
        // SAFETY: the name ends in a zero within its array.
        unsafe { CStr::from_ptr(properties.name.as_ptr()) }.to_string_lossy().into_owned()
    }

    /// What shows a window through `surface` (handed over), `system` being what the system
    /// keeps of it: its commands, their fence and the semaphore of its acquired images. Nothing
    /// where they can't be made.
    pub(crate) fn shows<W>(&mut self, surface: Handle, system: W) -> Option<Shows<W>> {
        let mut shows = Shows {
            surface,
            swapchain: 0,
            images: Vec::new(),
            acquired: 0,
            size: [0; 2],
            buffer: 0,
            memory: 0,
            pixels: null_mut(),
            commands: null_mut(),
            fence: 0,
            busy: false,
            remake: false,
            system,
        };
        let commands = VkCommandBufferAllocateInfo { kind: 40, pool: self.pool, count: 1, ..Default::default() };
        let fence = VkFenceCreateInfo { kind: 8, ..Default::default() };
        // SAFETY: Vulkan's documented calls on the device, filling in one handle each.
        let made = unsafe { [(self.calls.vkAllocateCommandBuffers)(self.device, &commands, &mut shows.commands), (self.calls.vkCreateFence)(self.device, &fence, null(), &mut shows.fence)] };
        shows.acquired = self.semaphore();
        if made == [0; 2] && shows.acquired != 0 {
            return Some(shows);
        }
        self.let_go(shows);
        None
    }

    /// A new semaphore (0 where none could be made).
    fn semaphore(&self) -> Handle {
        let (asked, mut made) = (VkSemaphoreCreateInfo { kind: 9, ..Default::default() }, 0);
        // SAFETY: Vulkan's documented call on the device.
        unsafe { (self.calls.vkCreateSemaphore)(self.device, &asked, null(), &mut made) };
        made
    }

    /// Memory for something needing `needs`, of the kinds in `kinds`; nothing where none fits.
    pub(crate) fn memory(&self, needs: VkMemoryRequirements, kinds: u32) -> Option<Handle> {
        let fitting = needs.type_bits & kinds;
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: fitting.trailing_zeros(), ..Default::default() };
        let mut memory = 0;
        // SAFETY: Vulkan's documented call on the device.
        (fitting != 0 && unsafe { (self.calls.vkAllocateMemory)(self.device, &asked, null(), &mut memory) } == 0).then_some(memory)
    }

    /// Makes a window's pixels, `size` big (after letting go of its old ones): a buffer in memory
    /// the CPU writes and the GPU reads, mapped. Says whether it could.
    pub(crate) fn buffer<W>(&mut self, shows: &mut Shows<W>, size: [i32; 2]) -> bool {
        self.unbuffer(shows);
        shows.size = size;
        let (calls, device, [width, height]) = (&self.calls, self.device, size);
        if width <= 0 || height <= 0 {
            return false;
        }
        let asked = VkBufferCreateInfo { kind: 12, size: width as u64 * height as u64 * 4, usage: USAGE_TRANSFER_SOURCE, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (calls.vkCreateBuffer)(device, &asked, null(), &mut shows.buffer) != 0 {
                return false;
            }
            (calls.vkGetBufferMemoryRequirements)(device, shows.buffer, &mut needs);
        }
        let Some(memory) = self.memory(needs, self.writable) else { return false };
        shows.memory = memory;
        let mut pixels = null_mut();
        // SAFETY: as above; the memory is bound whole and mapped whole.
        let made = unsafe { (calls.vkBindBufferMemory)(device, shows.buffer, memory, 0) == 0 && (calls.vkMapMemory)(device, memory, 0, needs.size, 0, &mut pixels) == 0 };
        shows.pixels = pixels.cast();
        made && !pixels.is_null()
    }

    /// Lets go of a window's pixels. (The queue is idle.)
    fn unbuffer<W>(&mut self, shows: &mut Shows<W>) {
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            (self.calls.vkDestroyBuffer)(self.device, shows.buffer, null());
            (self.calls.vkFreeMemory)(self.device, shows.memory, null());
        }
        (shows.buffer, shows.memory, shows.pixels) = (0, 0, null_mut());
    }

    /// What a window's surface can do now; or what Vulkan said where it couldn't be asked.
    pub(crate) fn capabilities<W>(&self, shows: &Shows<W>) -> Result<VkSurfaceCapabilitiesKHR, i32> {
        let mut can = VkSurfaceCapabilitiesKHR::default();
        // SAFETY: Vulkan's documented call on the window's surface.
        let result = unsafe { (self.vulkan.calls.vkGetPhysicalDeviceSurfaceCapabilitiesKHR)(self.physical, shows.surface, &mut can) };
        if result == 0 { Ok(can) } else { Err(result) }
    }

    /// The formats a window's surface offers.
    pub(crate) fn formats<W>(&self, shows: &Shows<W>) -> Vec<VkSurfaceFormatKHR> {
        // SAFETY: Vulkan's documented enumeration on the window's surface.
        listed(VkSurfaceFormatKHR::default(), |count, list| unsafe { (self.vulkan.calls.vkGetPhysicalDeviceSurfaceFormatsKHR)(self.physical, shows.surface, count, list) })
    }

    /// Makes a window's swap chain again, as `chain` says, its images written by copies and
    /// blits and presented in step with the screen (FIFO), unless `refused` (the surface can't
    /// take it). The old one goes either way. Says what Vulkan said (-1 where refused).
    pub(crate) fn make_chain<W>(&mut self, shows: &mut Shows<W>, chain: Chain, refused: bool) -> i32 {
        let asked = VkSwapchainCreateInfoKHR {
            kind: 1_000_001_000,
            surface: shows.surface,
            min_images: chain.images,
            format: chain.format.format,
            colour_space: chain.format.colour_space,
            extent: chain.extent,
            layers: 1,
            usage: USAGE_TRANSFER_DESTINATION,
            transform: chain.transform,
            alpha: chain.alpha,
            present_mode: 2, // FIFO
            old: shows.swapchain,
            ..Default::default()
        };
        let mut made = 0;
        self.idle();
        // SAFETY: Vulkan's documented call; the old chain is retired by it, and let go after.
        let result = if refused { -1 } else { unsafe { (self.calls.vkCreateSwapchainKHR)(self.device, &asked, null(), &mut made) } };
        self.unchain(shows);
        if result != 0 {
            return result;
        }
        // SAFETY: Vulkan's documented enumeration of the chain's images.
        let images = listed(0, |count, list| unsafe { (self.calls.vkGetSwapchainImagesKHR)(self.device, made, count, list) });
        (shows.swapchain, shows.remake) = (made, false);
        shows.images = images.into_iter().map(|image| Image { image, drawn: self.semaphore() }).collect();
        let whole = !shows.images.is_empty() && shows.images.iter().all(|image| image.drawn != 0);
        if whole { 0 } else { -1 }
    }

    /// Lets go of a window's swap chain and the semaphores of its images.
    fn unchain<W>(&mut self, shows: &mut Shows<W>) {
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            shows.images.drain(..).for_each(|image| (self.calls.vkDestroySemaphore)(self.device, image.drawn, null()));
            (self.calls.vkDestroySwapchainKHR)(self.device, shows.swapchain, null());
        }
        shows.swapchain = 0;
    }

    /// Waits until the queue has done all it was given.
    pub(crate) fn idle(&self) {
        // SAFETY: Vulkan's documented call on the device's queue.
        unsafe { (self.calls.vkQueueWaitIdle)(self.queue) };
    }

    /// Waits until a window's last frame is done, if it is not known to be. Says whether it is;
    /// if not, the device is lost.
    pub(crate) fn wait<W>(&mut self, shows: &mut Shows<W>) -> bool {
        // SAFETY: the fence is the device's.
        if shows.busy && unsafe { (self.calls.vkWaitForFences)(self.device, 1, &shows.fence, 1, self.waited) } != 0 {
            self.lost = true;
        }
        shows.busy = false;
        !self.lost
    }

    /// Asks for the next image of a window's swap chain: what Vulkan said, and which image.
    pub(crate) fn next_image<W>(&self, shows: &Shows<W>) -> (i32, usize) {
        let mut at = 0;
        // SAFETY: Vulkan's documented call on the window's swap chain; the semaphore is free
        // (the frame that waited for it is done).
        let result = unsafe { (self.calls.vkAcquireNextImageKHR)(self.device, shows.swapchain, self.waited, shows.acquired, 0, &mut at) };
        (result, at as usize)
    }

    /// Starts recording a window's commands. (They are not in use: their fence was waited for.)
    pub(crate) fn begin<W>(&self, shows: &Shows<W>) {
        let begin = VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() };
        // SAFETY: Vulkan's documented calls on the window's commands and fence.
        unsafe {
            (self.calls.vkResetFences)(self.device, 1, &shows.fence);
            (self.calls.vkBeginCommandBuffer)(shows.commands, &begin);
        }
    }

    /// Records `places` (left, top, right, bottom) copied from a window's pixels into `image`,
    /// which is then laid out to be copied from; all of it, laid out afresh, if `fresh`. (Into
    /// the window's picture: Android's always, X11's where the GPU draws.)
    #[cfg(any(target_os = "android", feature = "gpu-draw"))]
    pub(crate) fn fill<W>(&self, shows: &Shows<W>, image: Handle, places: Vec<[i32; 4]>, fresh: bool) {
        let ([width, height], commands) = (shows.size, shows.commands);
        let places = if fresh { vec![[0, 0, width, height]] } else { places };
        let regions: Vec<VkBufferImageCopy> = places.into_iter().map(|place| region(place, width)).collect();
        if regions.is_empty() {
            return;
        }
        let (old, access) = if fresh { (LAYOUT_UNDEFINED, 0) } else { (LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_READ) };
        let to_copy = barrier(image, old, LAYOUT_TRANSFER_DESTINATION, access, ACCESS_TRANSFER_WRITE);
        let to_read = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_READ);
        let calls = &self.calls;
        // SAFETY: the commands are being recorded; the places lie in the buffer and the image.
        unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            (calls.vkCmdCopyBufferToImage)(commands, shows.buffer, image, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr());
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
        }
    }

    /// Ends a frame's commands (`recorded`: what ending them said, or an earlier failure) and
    /// hands them to the queue, once image `at` is had, signalling its semaphore when done. Says
    /// whether the queue took them; if not, the device is lost.
    pub(crate) fn submit<W>(&mut self, shows: &mut Shows<W>, at: usize, recorded: i32) -> bool {
        let stage = STAGE_TRANSFER;
        let drawn = shows.images[at].drawn;
        let submit =
            VkSubmitInfo { kind: 4, wait_count: 1, waits: &shows.acquired, wait_stages: &stage, buffer_count: 1, buffers: &shows.commands, signal_count: 1, signals: &drawn, ..Default::default() };
        // SAFETY: Vulkan's documented call; the commands are recorded and the fence free.
        let result = if recorded == 0 { unsafe { (self.calls.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) } } else { recorded };
        shows.busy = result == 0;
        self.lost |= result != 0;
        result == 0
    }

    /// Ends commands that drew into a window's picture (if `recorded`), runs them and waits for
    /// them. Says whether they ran; if not, the device is lost.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn run<W>(&mut self, shows: &mut Shows<W>, recorded: bool) -> bool {
        let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &shows.commands, ..Default::default() };
        // SAFETY: Vulkan's documented calls; the commands are being recorded, the fence is free.
        let result = unsafe { if (self.calls.vkEndCommandBuffer)(shows.commands) == 0 && recorded { (self.calls.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) } else { -1 } };
        (shows.busy, self.lost) = (result == 0, self.lost || result != 0);
        self.wait(shows)
    }

    /// Presents image `at` of a window's swap chain once its frame is done, naming `places` where
    /// the device can (none: all of it changed). Says what Vulkan said.
    pub(crate) fn queue_present<W>(&mut self, shows: &Shows<W>, at: usize, places: Option<&[VkRectLayerKHR]>) -> i32 {
        let rectangles = places.unwrap_or_default();
        let region = VkPresentRegionKHR { count: rectangles.len() as u32, rects: rectangles.as_ptr() };
        let regions = VkPresentRegionsKHR { kind: 1_000_084_000, count: 1, regions: &region, ..Default::default() };
        let index = at as u32;
        let asked = VkPresentInfoKHR {
            kind: 1_000_001_001,
            next: if self.regions && places.is_some() { (&raw const regions).cast() } else { null() },
            wait_count: 1,
            waits: &shows.images[at].drawn,
            count: 1,
            swapchains: &shows.swapchain,
            indices: &index,
            results: null_mut(),
        };
        // SAFETY: Vulkan's documented call on the device's queue; everything named lives until
        // it returns.
        unsafe { (self.calls.vkQueuePresentKHR)(self.queue, &asked) }
    }

    /// Lets go of a window: its swap chain, pixels, commands, and its surface last. (The queue
    /// is idle, and what the system made for it is gone.)
    pub(crate) fn let_go<W>(&mut self, mut shows: Shows<W>) {
        self.unchain(&mut shows);
        self.unbuffer(&mut shows);
        let (calls, device) = (&self.calls, self.device);
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be,
        // and a command buffer only freed where there is one.
        unsafe {
            (calls.vkDestroySemaphore)(device, shows.acquired, null());
            (calls.vkDestroyFence)(device, shows.fence, null());
            if !shows.commands.is_null() {
                (calls.vkFreeCommandBuffers)(device, self.pool, 1, &shows.commands);
            }
        }
        self.vulkan.destroy(shows.surface);
    }

    /// What the device draws with; nothing where it can't be made.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn drawer(&self) -> Option<Box<crate::gpu_draw_vulkan::Pipeline>> {
        let (find, device, mut memory) = (self.vulkan.calls.vkGetDeviceProcAddr, self.device, VkPhysicalDeviceMemoryProperties::default());
        // SAFETY: Vulkan fills in the device's memory; the names end in zeros.
        unsafe { (self.vulkan.calls.vkGetPhysicalDeviceMemoryProperties)(self.physical, &mut memory) };
        crate::gpu_draw_vulkan::Pipeline::new(&mut |name| unsafe { find(device, name.as_ptr()) }, device, &memory, false)
    }

    /// Tests: whether it has drawn windows itself (see `gpu_draw`).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        matches!(self.pipeline, Some(Some(_)))
    }

    /// Tests: the device is lost, as when it is removed: the next frame says so.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        self.faked = true;
    }
}

impl<S> Drop for Gpu<S> {
    /// Lets go of what draws, the pool and the device. (Windows were let go of first; the
    /// instance goes after, then what the system keeps.)
    fn drop(&mut self) {
        // SAFETY: Vulkan's documented calls; nothing of the device is used after.
        unsafe {
            (self.calls.vkDeviceWaitIdle)(self.device);
            #[cfg(feature = "gpu-draw")]
            drop(self.pipeline.take());
            (self.calls.vkDestroyCommandPool)(self.device, self.pool, null());
            (self.calls.vkDestroyDevice)(self.device, null());
        }
    }
}
