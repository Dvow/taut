//! Android windows shown through Vulkan, with the feature `gpu`. The CPU draws every pixel as
//! without it; each place it drew goes into the window's pixels, a buffer the CPU writes and the
//! GPU reads, and from there into the window's picture, an image on the GPU (with `gpu-draw` the
//! GPU draws large parts into the picture itself). A frame blits the whole picture into the swap
//! chain's next image, inside the window's insets, the theme's background around it, and presents
//! it naming the places that changed (`VK_KHR_incremental_present`), in step with the screen
//! (FIFO). The blit only reorders the channels where the swap chain's images are RGBA: the pixels
//! shown are the CPU's, byte for byte.
//!
//! - The surface is the activity's window (`VK_KHR_android_surface`). It goes with the window
//!   (the activity paused, or gone); the device stays, and the next window gets a new surface.
//! - Swap chains are made with no transform of their own: on a turned screen the compositor turns
//!   the frames (and says `VK_SUBOPTIMAL_KHR`, which is no reason to make them again).
//! - The loader (`libvulkan.so`) is opened when the window first asks for the GPU.
//!
//! `Auto` takes a device only where it is not the CPU at work (SwiftShader).
use crate::gpu_android::Block;
use crate::vk::*;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{null, null_mut};

functions!(Global {
    vkCreateInstance: fn(*const VkInstanceCreateInfo, *const c_void, *mut Object) -> i32;
    vkEnumerateInstanceExtensionProperties: fn(*const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
});

functions!(Instance {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceProperties: fn(Object, *mut VkPhysicalDeviceProperties);
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkGetPhysicalDeviceFormatProperties: fn(Object, u32, *mut VkFormatProperties);
    vkEnumerateDeviceExtensionProperties: fn(Object, *const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkGetDeviceProcAddr: fn(Object, *const c_char) -> *const c_void;
    vkCreateAndroidSurfaceKHR: fn(Object, *const VkAndroidSurfaceCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkDestroySurfaceKHR: fn(Object, Handle, *const c_void);
    vkGetPhysicalDeviceSurfaceSupportKHR: fn(Object, u32, Handle, *mut u32) -> i32;
    vkGetPhysicalDeviceSurfaceCapabilitiesKHR: fn(Object, Handle, *mut VkSurfaceCapabilitiesKHR) -> i32;
    vkGetPhysicalDeviceSurfaceFormatsKHR: fn(Object, Handle, *mut u32, *mut VkSurfaceFormatKHR) -> i32;
});

functions!(Device {
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
    vkCreateImage: fn(Object, *const VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkAllocateMemory: fn(Object, *const VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
    vkCreateCommandPool: fn(Object, *const VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkFreeCommandBuffers: fn(Object, Handle, u32, *const Object);
    vkBeginCommandBuffer: fn(Object, *const VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCmdClearColorImage: fn(Object, Handle, u32, *const [f32; 4], u32, *const [u32; 5]);
    vkCmdBlitImage: fn(Object, Handle, u32, Handle, u32, u32, *const VkImageBlit, u32);
    vkCreateSemaphore: fn(Object, *const VkSemaphoreCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroySemaphore: fn(Object, Handle, *const c_void);
    vkCreateFence: fn(Object, *const VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
});

/// Vulkan 1.1, as `VK_MAKE_API_VERSION` makes it.
const VULKAN_1_1: u32 = (1 << 22) | (1 << 12);
/// The swap chain's pixels where they are red first (the format every Android device offers).
const R8G8B8A8_UNORM: u32 = 37;
/// Formats a blit reads from, and writes to (`VK_FORMAT_FEATURE_BLIT_SRC_BIT`, `_DST_BIT`).
const BLITS_FROM: u32 = 0x400;
const BLITS_TO: u32 = 0x800;
/// `VK_ERROR_SURFACE_LOST_KHR`: the window went.
const SURFACE_LOST: i32 = -1_000_000_000;
/// How long a frame may wait for an image or for the GPU's drawing before the device is taken
/// for lost: ten seconds, in nanoseconds (the CPU at work draws a whole window in more than one).
const WAITED: u64 = 10_000_000_000;

/// The instance of Vulkan, with the functions it offers.
struct Vulkan {
    instance: Object,
    calls: Instance,
}

/// A device that can show the window: its queue family (one that draws, so it blits), and
/// whether it is the CPU at work.
struct Chosen {
    physical: Object,
    family: u32,
    cpu: bool,
}

/// The GPU the window is shown through: one device and queue.
pub(crate) struct Gpu {
    vulkan: Vulkan,
    on: Device,
    chosen: Chosen,
    device: Object,
    queue: Object,
    pool: Handle,
    /// The kinds of memory the CPU writes and the GPU reads, and of the GPU's own: one bit each.
    writable: u32,
    local: u32,
    /// Presents name the places that changed.
    regions: bool,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// Where the GPU draws the window too, what it records to, and what it draws with: not yet
    /// tried, or what that gave (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<Option<Box<crate::gpu_draw_vulkan::Pipeline>>>,
    /// Tests: every call from now on says the device was lost.
    #[cfg(test)]
    faked: bool,
}

/// The window shown through Vulkan: its surface and swap chain, its pixels and picture, and what
/// was put into them since the last frame.
pub(crate) struct Shows {
    surface: Handle,
    swapchain: Handle,
    images: Vec<Image>,
    /// Signalled once an image is had.
    acquired: Handle,
    /// The pixels, `size` big, in a buffer the CPU writes (mapped at `pixels`); and the picture,
    /// the same size on the GPU, which is `fresh` (all of it to be copied in) until first filled.
    /// Both are `new` until all of the window is drawn into them.
    size: [i32; 2],
    buffer: Handle,
    memory: Handle,
    pixels: *mut u32,
    picture: Handle,
    picture_memory: Handle,
    fresh: bool,
    new: bool,
    /// The window's size, where the pixels lie in it (left, top), and the colour around them.
    window: [i32; 2],
    at: [i32; 2],
    background: u32,
    /// The commands of a frame, and the fence of their last run, not yet waited for if `busy`.
    commands: Object,
    fence: Handle,
    busy: bool,
    /// The swap chain no longer fits the window: it is made again before the next frame.
    remake: bool,
    /// The window went: its surface is to be let go.
    pub(crate) gone: bool,
    /// The places put into the buffer and not yet into the picture; and those changed since
    /// the last frame (left, top, right, bottom in the pixels).
    pending: Vec<[i32; 4]>,
    changed: Vec<[i32; 4]>,
    /// Everything is to be shown, not only the places that changed.
    whole: bool,
    /// Tests: the places the last frame named.
    #[cfg(test)]
    named: Vec<[i32; 4]>,
}

/// An image of a swap chain, and the semaphore signalled once its blit is done.
#[derive(Clone, Copy)]
struct Image {
    image: Handle,
    drawn: Handle,
}

impl Shows {
    /// How much of the pixels (width, height) shows in the window, from where they lie.
    fn shown(&self) -> [i32; 2] {
        [0, 1].map(|axis| self.size[axis].min(self.window[axis] - self.at[axis]).max(0))
    }

    /// Whether all of the window is to be drawn (its pixels are new); asked once.
    pub(crate) fn take_whole(&mut self) -> bool {
        std::mem::take(&mut self.new)
    }
}

impl Vulkan {
    /// Opens the loader and makes an instance that shows Android windows. Nothing where any of
    /// it is missing.
    fn open() -> Option<Vulkan> {
        let get = loader()?;
        // SAFETY: the instance is null or Vulkan's.
        let find = move |instance: Object, name: &CStr| unsafe { get(instance, name.as_ptr()) };
        let global = Global::load(&mut |name| find(null_mut(), name)).ok()?;
        // SAFETY: Vulkan's documented enumeration.
        let offers = |name| offered(|count, list| unsafe { (global.vkEnumerateInstanceExtensionProperties)(null(), count, list) }, name);
        if !offers(c"VK_KHR_surface") || !offers(c"VK_KHR_android_surface") {
            return None;
        }
        let instance = instance(&global, &[c"VK_KHR_surface".as_ptr(), c"VK_KHR_android_surface".as_ptr()])?;
        let Ok(calls) = Instance::load(&mut |name| find(instance, name)) else {
            let destroy = find(instance, c"vkDestroyInstance");
            // SAFETY: the instance was just made, and nothing of it is used after.
            if !destroy.is_null() {
                unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(instance, null()) };
            }
            return None;
        };
        Some(Vulkan { instance, calls })
    }

    /// A surface on `window` (an `ANativeWindow`); nothing where none is made.
    fn surface(&self, window: *mut c_void) -> Option<Handle> {
        let (asked, mut surface) = (VkAndroidSurfaceCreateInfoKHR { kind: 1_000_008_000, window, ..Default::default() }, 0);
        // SAFETY: the window is alive (the caller's); Vulkan fills in one handle.
        let made = unsafe { (self.calls.vkCreateAndroidSurfaceKHR)(self.instance, &asked, null(), &mut surface) };
        (made == 0).then_some(surface)
    }

    /// The device and queue family best showing `window` (an `ANativeWindow`, alive), asked
    /// through a surface made on it for the question: a GPU before the CPU at work; unless
    /// `any`, only a GPU.
    fn choose(&self, window: *mut c_void, any: bool) -> Option<Chosen> {
        let surface = self.surface(window)?;
        // SAFETY: Vulkan's documented enumeration.
        let devices = listed(null_mut(), |count, list| unsafe { (self.calls.vkEnumeratePhysicalDevices)(self.instance, count, list) });
        let usable = devices.into_iter().filter_map(|physical| self.chosen(physical, surface));
        let chosen = usable.filter(|chosen| any || !chosen.cpu).min_by_key(|chosen| chosen.cpu);
        // SAFETY: the surface was made here, and is not used after.
        unsafe { (self.calls.vkDestroySurfaceKHR)(self.instance, surface, null()) };
        chosen
    }

    /// What device `physical` is, where it can show `surface`: a queue family that draws and
    /// presents to it, swap chains, and blits from the window's picture.
    fn chosen(&self, physical: Object, surface: Handle) -> Option<Chosen> {
        let calls = &self.calls;
        // SAFETY: Vulkan's documented enumerations and questions, of the instance's device.
        let swaps = offered(|count, list| unsafe { (calls.vkEnumerateDeviceExtensionProperties)(physical, null(), count, list) }, c"VK_KHR_swapchain");
        let families = listed(VkQueueFamilyProperties::default(), |count, list| {
            unsafe { (calls.vkGetPhysicalDeviceQueueFamilyProperties)(physical, count, list) };
            0
        });
        let presents = |at: usize| {
            let mut supported = 0;
            unsafe { (calls.vkGetPhysicalDeviceSurfaceSupportKHR)(physical, at as u32, surface, &mut supported) };
            supported != 0
        };
        // (Graphics: 0x1.)
        let family = (0..families.len()).find(|at| families[*at].count > 0 && families[*at].flags & 0x1 != 0 && presents(*at)).filter(|_| swaps)? as u32;
        let mut properties = VkPhysicalDeviceProperties::default();
        let mut picture = VkFormatProperties::default();
        // SAFETY: Vulkan fills in both.
        unsafe {
            (calls.vkGetPhysicalDeviceProperties)(physical, &mut properties);
            (calls.vkGetPhysicalDeviceFormatProperties)(physical, B8G8R8A8_UNORM, &mut picture);
        }
        (picture.optimal & BLITS_FROM != 0).then_some(Chosen { physical, family, cpu: properties.device_type == 4 })
    }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        // SAFETY: nothing made from it is used after (surfaces and devices went first).
        unsafe { (self.calls.vkDestroyInstance)(self.instance, null()) };
    }
}

/// An instance with `extensions`, of Vulkan 1.1 where the loader takes it, else 1.0.
fn instance(global: &Global, extensions: &[*const c_char]) -> Option<Object> {
    [VULKAN_1_1, 1 << 22].into_iter().find_map(|api| {
        let application = VkApplicationInfo { kind: 0, api_version: api, ..Default::default() };
        let asked = VkInstanceCreateInfo { kind: 1, application: &application, extension_count: extensions.len() as u32, extensions: extensions.as_ptr(), ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: Vulkan's documented call; the names end in zeros.
        (unsafe { (global.vkCreateInstance)(&asked, null(), &mut instance) } == 0).then_some(instance)
    })
}

impl Gpu {
    /// Opens Vulkan and shows `window` (an `ANativeWindow`) through it, sized by `fit`. Nothing
    /// where Vulkan, a device presenting to it or anything else is missing, or where the only
    /// device is the CPU at work and `any` is false.
    ///
    /// # Safety
    /// `window` is an `ANativeWindow`, alive until the window is let go of (`forget`).
    pub(crate) unsafe fn open(window: *mut c_void, any: bool) -> Option<(Gpu, Shows)> {
        let vulkan = Vulkan::open()?;
        let chosen = vulkan.choose(window, any)?;
        #[cfg(feature = "gpu-draw")]
        let cpu = chosen.cpu;
        let mut gpu = Gpu::make(vulkan, chosen)?;
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !cpu, crate::platform::setting("TAUT_DRAW").as_deref());
        }
        // SAFETY: as the caller promises.
        let shows = unsafe { gpu.window(window) }?;
        Some((gpu, shows))
    }

    /// A device on `chosen`, with its queue and a pool of commands. Nothing where any of it
    /// can't be made.
    fn make(vulkan: Vulkan, chosen: Chosen) -> Option<Gpu> {
        let (calls, physical, family) = (&vulkan.calls, chosen.physical, chosen.family);
        // SAFETY: Vulkan's documented enumeration.
        let regions = offered(|count, list| unsafe { (calls.vkEnumerateDeviceExtensionProperties)(physical, null(), count, list) }, c"VK_KHR_incremental_present");
        let extensions = [c"VK_KHR_swapchain".as_ptr(), c"VK_KHR_incremental_present".as_ptr()];
        let priority = 1.0f32;
        let queue = VkDeviceQueueCreateInfo { kind: 2, family, count: 1, priorities: &priority, ..Default::default() };
        let count = if regions { 2 } else { 1 };
        let asked = VkDeviceCreateInfo { kind: 3, queue_count: 1, queues: &queue, extension_count: count, extensions: extensions.as_ptr(), ..Default::default() };
        let (mut device, mut memory) = (null_mut(), VkPhysicalDeviceMemoryProperties::default());
        // SAFETY: Vulkan fills in the device's memory, and makes the device; the names end in
        // zeros.
        unsafe {
            (calls.vkGetPhysicalDeviceMemoryProperties)(physical, &mut memory);
            if (calls.vkCreateDevice)(physical, &asked, null(), &mut device) != 0 {
                return None;
            }
        }
        let find = calls.vkGetDeviceProcAddr;
        // SAFETY: the device was just made.
        let on = Device::load(&mut |name| unsafe { find(device, name.as_ptr()) });
        let Ok(on) = on else { return Gpu::unmade(find, device) };
        let mut gpu = Gpu {
            vulkan,
            on,
            chosen,
            device,
            queue: null_mut(),
            pool: 0,
            writable: kinds(&memory, HOST_VISIBLE_AND_COHERENT),
            local: kinds(&memory, DEVICE_LOCAL),
            regions,
            lost: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
            #[cfg(test)]
            faked: false,
        };
        let pool = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family, ..Default::default() };
        // SAFETY: the device's queue 0 of the family it was made with; and a pool for it.
        let made = unsafe {
            (gpu.on.vkGetDeviceQueue)(device, family, 0, &mut gpu.queue);
            (gpu.on.vkCreateCommandPool)(device, &pool, null(), &mut gpu.pool)
        };
        (made == 0).then_some(gpu)
    }

    /// Lets go of a device whose functions could not all be found: nothing.
    fn unmade(find: Find, device: Object) -> Option<Gpu> {
        // SAFETY: the device was just made and is not used after.
        let destroy = unsafe { find(device, c"vkDestroyDevice".as_ptr()) };
        if !destroy.is_null() {
            unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(device, null()) };
        }
        None
    }

    /// Tests: the device's name, from Vulkan (as "SwiftShader Device (LLVM 10.0.0)").
    #[cfg(test)]
    pub(crate) fn name(&self) -> String {
        let mut properties = VkPhysicalDeviceProperties::default();
        // SAFETY: Vulkan fills in the properties; the name ends in a zero within its array.
        unsafe {
            (self.vulkan.calls.vkGetPhysicalDeviceProperties)(self.chosen.physical, &mut properties);
            CStr::from_ptr(properties.name.as_ptr()).to_string_lossy().into_owned()
        }
    }

    /// Shows `window` (an `ANativeWindow`) through the device: a surface on it, the window then
    /// sized by `fit`. Nothing where it can't be made.
    ///
    /// # Safety
    /// As `open`.
    pub(crate) unsafe fn window(&mut self, window: *mut c_void) -> Option<Shows> {
        let surface = self.vulkan.surface(window)?;
        let mut supported = 0;
        // SAFETY: the surface was just made; the family is the device's.
        unsafe { (self.vulkan.calls.vkGetPhysicalDeviceSurfaceSupportKHR)(self.chosen.physical, self.chosen.family, surface, &mut supported) };
        if supported == 0 {
            // SAFETY: the surface was just made, and is not used after.
            unsafe { (self.vulkan.calls.vkDestroySurfaceKHR)(self.vulkan.instance, surface, null()) };
            return None;
        }
        self.shows(surface)
    }

    /// What shows a window through `surface` (handed over), not yet sized; nothing where it
    /// can't be made.
    fn shows(&mut self, surface: Handle) -> Option<Shows> {
        let mut shows = Shows {
            surface,
            swapchain: 0,
            images: Vec::new(),
            acquired: 0,
            size: [0; 2],
            buffer: 0,
            memory: 0,
            pixels: null_mut(),
            picture: 0,
            picture_memory: 0,
            fresh: true,
            new: true,
            window: [0; 2],
            at: [0; 2],
            background: 0xff00_0000,
            commands: null_mut(),
            fence: 0,
            busy: false,
            remake: false,
            gone: false,
            pending: Vec::new(),
            changed: Vec::new(),
            whole: true,
            #[cfg(test)]
            named: Vec::new(),
        };
        if self.parts(&mut shows) {
            return Some(shows);
        }
        self.forget(shows);
        None
    }

    /// Makes a window's commands, their fence and the semaphore of its acquired images. Says
    /// whether all were made.
    fn parts(&mut self, shows: &mut Shows) -> bool {
        let commands = VkCommandBufferAllocateInfo { kind: 40, pool: self.pool, count: 1, ..Default::default() };
        let fence = VkFenceCreateInfo { kind: 8, ..Default::default() };
        // SAFETY: Vulkan's documented calls on the device, filling in one handle each.
        let made = unsafe { [(self.on.vkAllocateCommandBuffers)(self.device, &commands, &mut shows.commands), (self.on.vkCreateFence)(self.device, &fence, null(), &mut shows.fence)] };
        shows.acquired = self.semaphore();
        made == [0; 2] && shows.acquired != 0
    }

    /// A new semaphore (0 where none could be made).
    fn semaphore(&self) -> Handle {
        let (asked, mut made) = (VkSemaphoreCreateInfo { kind: 9, ..Default::default() }, 0);
        // SAFETY: Vulkan's documented call on the device.
        unsafe { (self.on.vkCreateSemaphore)(self.device, &asked, null(), &mut made) };
        made
    }

    /// Gives a window its size (`window` pixels), and its pixels theirs (`size`, at `at` in the
    /// window): a new buffer and picture, all of which is to be drawn, where `size` changed; a
    /// new swap chain where `window` did. Says whether it has them: where the device makes none
    /// that big, the window is better shown by the CPU.
    pub(crate) fn fit(&mut self, shows: &mut Shows, window: [i32; 2], size: [i32; 2], at: [i32; 2]) -> bool {
        if (shows.window, shows.size, shows.at) == (window, size, at) && shows.swapchain != 0 {
            return true;
        }
        self.idle();
        (shows.at, shows.whole) = (at, true);
        if (shows.size != size || shows.buffer == 0) && !self.pixels(shows, size) {
            return false;
        }
        if shows.window == window && shows.swapchain != 0 {
            return true;
        }
        shows.window = window;
        self.chain(shows)
    }

    /// Makes a window's pixels and picture (again), `size` big. Says whether it could.
    fn pixels(&mut self, shows: &mut Shows, size: [i32; 2]) -> bool {
        self.unbuffer(shows);
        shows.size = size;
        size.iter().all(|side| *side > 0) && self.buffer(shows) && self.picture(shows)
    }

    /// Makes a window's pixels: a buffer its size, in memory the CPU writes and the GPU reads,
    /// mapped. Says whether it could.
    fn buffer(&mut self, shows: &mut Shows) -> bool {
        let (on, device, [width, height]) = (&self.on, self.device, shows.size);
        let asked = VkBufferCreateInfo { kind: 12, size: width as u64 * height as u64 * 4, usage: USAGE_TRANSFER_SOURCE, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (on.vkCreateBuffer)(device, &asked, null(), &mut shows.buffer) != 0 {
                return false;
            }
            (on.vkGetBufferMemoryRequirements)(device, shows.buffer, &mut needs);
        }
        let Some(memory) = self.memory(needs, self.writable) else { return false };
        shows.memory = memory;
        let mut pixels = null_mut();
        // SAFETY: as above; the memory is bound whole and mapped whole.
        let made = unsafe { (on.vkBindBufferMemory)(device, shows.buffer, memory, 0) == 0 && (on.vkMapMemory)(device, memory, 0, needs.size, 0, &mut pixels) == 0 };
        shows.pixels = pixels.cast();
        made && !pixels.is_null()
    }

    /// Makes a window's picture: an image its pixels' size in the GPU's own memory, copied into
    /// and blitted from (with `gpu-draw`, drawn into too). Says whether it could.
    fn picture(&mut self, shows: &mut Shows) -> bool {
        #[cfg(feature = "gpu-draw")]
        let usage = USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION | crate::vk::USAGE_DRAWN_INTO;
        #[cfg(not(feature = "gpu-draw"))]
        let usage = USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION;
        let extent = [shows.size[0] as u32, shows.size[1] as u32, 1];
        let asked = VkImageCreateInfo { kind: 14, image_type: 1, format: B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (self.on.vkCreateImage)(self.device, &asked, null(), &mut shows.picture) != 0 {
                return false;
            }
            (self.on.vkGetImageMemoryRequirements)(self.device, shows.picture, &mut needs);
        }
        let Some(memory) = self.memory(needs, self.local) else { return false };
        (shows.picture_memory, shows.fresh, shows.new) = (memory, true, true);
        // SAFETY: as above; the memory is bound whole.
        unsafe { (self.on.vkBindImageMemory)(self.device, shows.picture, memory, 0) == 0 }
    }

    /// Memory for something needing `needs`, of the kinds in `kinds`; nothing where none fits.
    fn memory(&self, needs: VkMemoryRequirements, kinds: u32) -> Option<Handle> {
        let fitting = needs.type_bits & kinds;
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: fitting.trailing_zeros(), ..Default::default() };
        let mut memory = 0;
        // SAFETY: Vulkan's documented call on the device.
        (fitting != 0 && unsafe { (self.on.vkAllocateMemory)(self.device, &asked, null(), &mut memory) } == 0).then_some(memory)
    }

    /// Lets go of a window's pixels and picture.
    fn unbuffer(&mut self, shows: &mut Shows) {
        #[cfg(feature = "gpu-draw")]
        if let Some(pipeline) = self.pipeline.as_mut().and_then(|made| made.as_deref_mut()) {
            pipeline.forget_target();
        }
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            (self.on.vkDestroyBuffer)(self.device, shows.buffer, null());
            (self.on.vkFreeMemory)(self.device, shows.memory, null());
            (self.on.vkDestroyImage)(self.device, shows.picture, null());
            (self.on.vkFreeMemory)(self.device, shows.picture_memory, null());
        }
        (shows.buffer, shows.memory, shows.pixels, shows.picture, shows.picture_memory) = (0, 0, null_mut(), 0, 0);
        (shows.pending, shows.changed) = (Vec::new(), Vec::new());
    }

    /// Makes a window's swap chain (again) at the window's size: images of 8-bit colour, blitted
    /// into, opaque, presented in step with the screen. Says whether it could.
    fn chain(&mut self, shows: &mut Shows) -> bool {
        let (calls, physical) = (&self.vulkan.calls, self.chosen.physical);
        let mut can = VkSurfaceCapabilitiesKHR::default();
        // SAFETY: Vulkan's documented calls on the window's surface.
        let result = unsafe { (calls.vkGetPhysicalDeviceSurfaceCapabilitiesKHR)(physical, shows.surface, &mut can) };
        if result != 0 {
            shows.gone |= result == SURFACE_LOST;
            return false;
        }
        // SAFETY: as above; Vulkan's documented enumeration.
        let formats = listed(VkSurfaceFormatKHR::default(), |count, list| unsafe { (calls.vkGetPhysicalDeviceSurfaceFormatsKHR)(physical, shows.surface, count, list) });
        // (sRGB's colour space is 0; alpha ignored 0x1, or as the window has it 0x8; no
        // transform 0x1.)
        let blits_to = |format: u32| {
            let mut properties = VkFormatProperties::default();
            // SAFETY: Vulkan fills in the format's properties.
            unsafe { (calls.vkGetPhysicalDeviceFormatProperties)(physical, format, &mut properties) };
            properties.optimal & BLITS_TO != 0
        };
        let format = formats.into_iter().find(|offered| [R8G8B8A8_UNORM, B8G8R8A8_UNORM].contains(&offered.format) && offered.colour_space == 0 && blits_to(offered.format));
        let alpha = [0x1, 0x8].into_iter().find(|alpha| can.alphas & alpha != 0);
        let transform = if can.transforms & 0x1 != 0 { 0x1 } else { can.transform };
        let extent = shows.window.map(|side| side as u32);
        let fits = extent.iter().zip(can.least.iter().zip(&can.most)).all(|(side, (least, most))| (least..=most).contains(&side));
        let (Some(format), Some(alpha)) = (format, alpha) else { return false };
        let asked = VkSwapchainCreateInfoKHR {
            kind: 1_000_001_000,
            surface: shows.surface,
            min_images: can.min_images.max(2),
            format: format.format,
            colour_space: format.colour_space,
            extent,
            layers: 1,
            usage: USAGE_TRANSFER_DESTINATION,
            transform,
            alpha,
            present_mode: 2, // FIFO
            old: shows.swapchain,
            ..Default::default()
        };
        let mut made = 0;
        self.idle();
        // SAFETY: Vulkan's documented call; the old chain is retired by it, and let go after.
        let result = if can.usage & USAGE_TRANSFER_DESTINATION == 0 || !fits { -1 } else { unsafe { (self.on.vkCreateSwapchainKHR)(self.device, &asked, null(), &mut made) } };
        self.unchain(shows);
        shows.gone |= result == SURFACE_LOST;
        if result != 0 {
            return false;
        }
        // SAFETY: Vulkan's documented enumeration of the chain's images.
        let images = listed(0, |count, list| unsafe { (self.on.vkGetSwapchainImagesKHR)(self.device, made, count, list) });
        (shows.swapchain, shows.remake, shows.whole) = (made, false, true);
        shows.images = images.into_iter().map(|image| Image { image, drawn: self.semaphore() }).collect();
        !shows.images.is_empty() && shows.images.iter().all(|image| image.drawn != 0)
    }

    /// Lets go of a window's swap chain and the semaphores of its images.
    fn unchain(&mut self, shows: &mut Shows) {
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            shows.images.drain(..).for_each(|image| (self.on.vkDestroySemaphore)(self.device, image.drawn, null()));
            (self.on.vkDestroySwapchainKHR)(self.device, shows.swapchain, null());
        }
        shows.swapchain = 0;
    }

    /// Waits until the queue has done all it was given.
    fn idle(&self) {
        // SAFETY: Vulkan's documented call on the device's queue.
        unsafe { (self.on.vkQueueWaitIdle)(self.queue) };
    }

    /// Waits until a window's last frame is done, if it is not known to be. Says whether it is;
    /// if not, the device is lost.
    fn wait(&mut self, shows: &mut Shows) -> bool {
        // SAFETY: the fence is the device's.
        if shows.busy && unsafe { (self.on.vkWaitForFences)(self.device, 1, &shows.fence, 1, WAITED) } != 0 {
            self.lost = true;
        }
        shows.busy = false;
        !self.lost
    }

    /// Puts `place` of a block of pixels (left, top, right, bottom) into the window's pixels at
    /// the same place, as far as both reach, to be shown with the next frame.
    pub(crate) fn put(&mut self, shows: &mut Shows, block: &Block, place: [i32; 4]) {
        let Some(place) = block.cut(place, shows.size).filter(|_| !shows.pixels.is_null() && self.wait(shows)) else { return };
        let wide = shows.size[0] as usize;
        for (row, from) in block.rows(place) {
            // SAFETY: the pixels are mapped, `wide` by `high`, and the place lies inside them
            // (`cut`).
            unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), shows.pixels.add(row * wide + place[0] as usize), from.len()) };
        }
        shows.pending.push(place);
        shows.changed.push(place);
    }

    /// Shows the window: what was put since goes into the picture, which is blitted whole into
    /// the swap chain's next image, inside the background (`background`, `0xAARRGGBB`), and
    /// presented naming what changed. With nothing changed, only where `again` (the system asks
    /// for the window whole). A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows, background: u32, again: bool) {
        let fresh_background = std::mem::replace(&mut shows.background, background) != background;
        shows.whole |= again || fresh_background;
        if shows.changed.is_empty() && !shows.whole || !self.wait(shows) {
            return;
        }
        #[cfg(test)]
        if self.faked {
            self.lost = true;
            return;
        }
        if shows.remake && !self.chain(shows) {
            self.lost |= !shows.gone;
            return;
        }
        let Some(at) = self.acquire(shows) else { return };
        if self.record(shows, at) {
            self.present(shows, at);
        }
    }

    /// The next image of a window's swap chain to blit into, made again first if it no longer
    /// fits. Nothing where none came: then the places put wait for the next frame (or the
    /// window or device is lost).
    fn acquire(&mut self, shows: &mut Shows) -> Option<usize> {
        let mut at = 0;
        // SAFETY: Vulkan's documented call on the window's swap chain; the semaphore is free
        // (the frame that waited for it is done).
        let next = |gpu: &Gpu, shows: &Shows, at: &mut u32| unsafe { (gpu.on.vkAcquireNextImageKHR)(gpu.device, shows.swapchain, WAITED, shows.acquired, 0, at) };
        let mut result = next(self, shows, &mut at);
        if result == OUT_OF_DATE && self.chain(shows) {
            result = next(self, shows, &mut at);
        }
        self.said(shows, result);
        let had = result == 0 || result == SUBOPTIMAL;
        (had && (at as usize) < shows.images.len()).then_some(at as usize)
    }

    /// Takes in what a call on the swap chain said: a chain out of date is made again, a window
    /// gone or a device failing is said.
    fn said(&mut self, shows: &mut Shows, result: i32) {
        shows.remake |= result == OUT_OF_DATE;
        shows.gone |= result == SURFACE_LOST;
        self.lost |= result < 0 && result != OUT_OF_DATE && result != SURFACE_LOST;
    }

    /// Records and submits a frame into image `at`: the places put go into the picture (all of it
    /// while new), the background is cleared in where the picture leaves room, and the picture
    /// blitted in. Says whether it was given to the queue; if not, the device is lost.
    fn record(&mut self, shows: &mut Shows, at: usize) -> bool {
        let (on, commands, Image { image, drawn }) = (&self.on, shows.commands, shows.images[at]);
        let begin = VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() };
        // SAFETY: Vulkan's documented calls; the commands are not in use (their fence was
        // waited for).
        unsafe {
            (on.vkResetFences)(self.device, 1, &shows.fence);
            (on.vkBeginCommandBuffer)(commands, &begin);
        }
        self.fill_picture(shows);
        let to_blit = barrier(image, LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
        // (The clear and the blit both write the image: the blit waits for the clear.)
        let cleared = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_DESTINATION, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_WRITE);
        let to_show = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        let ([left, top], shown) = (shows.at, shows.shown());
        let room = left > 0 || top > 0 || [left + shown[0], top + shown[1]] != shows.window;
        let colour = [16, 8, 0, 24].map(|shift| ((shows.background >> shift) & 0xff) as f32 / 255.0);
        let blit = VkImageBlit {
            source_layers: LAYER,
            source_offsets: [[0, 0, 0], [shown[0], shown[1], 1]],
            destination_layers: LAYER,
            destination_offsets: [[left, top, 0], [left + shown[0], top + shown[1], 1]],
        };
        // SAFETY: as above; the picture is laid out to be copied from (see `fill_picture`), the
        // image to be written; the blit lies inside both (`shown`).
        let recorded = unsafe {
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_blit);
            if room {
                (on.vkCmdClearColorImage)(commands, image, LAYOUT_TRANSFER_DESTINATION, &colour, 1, &WHOLE);
                (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &cleared);
            }
            if shown.iter().all(|side| *side > 0) {
                (on.vkCmdBlitImage)(commands, shows.picture, LAYOUT_TRANSFER_SOURCE, image, LAYOUT_TRANSFER_DESTINATION, 1, &blit, 0);
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &to_show);
            (on.vkEndCommandBuffer)(commands)
        };
        let stage = STAGE_TRANSFER;
        let submit = VkSubmitInfo { kind: 4, wait_count: 1, waits: &shows.acquired, wait_stages: &stage, buffer_count: 1, buffers: &commands, signal_count: 1, signals: &drawn, ..Default::default() };
        // SAFETY: as above.
        let result = if recorded == 0 { unsafe { (on.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) } } else { recorded };
        shows.busy = result == 0;
        self.lost |= result != 0;
        result == 0
    }

    /// Records the places put since copied from the window's pixels into its picture (all of
    /// it, if new), which is then laid out to be copied from.
    fn fill_picture(&self, shows: &mut Shows) {
        let ([width, height], on, commands, picture) = (shows.size, &self.on, shows.commands, shows.picture);
        let places = if shows.fresh { vec![[0, 0, width, height]] } else { std::mem::take(&mut shows.pending) };
        shows.pending.clear();
        let regions: Vec<VkBufferImageCopy> = places.into_iter().map(|place| region(place, width)).collect();
        let (old, access) = if shows.fresh { (LAYOUT_UNDEFINED, 0) } else { (LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_READ) };
        let to_copy = barrier(picture, old, LAYOUT_TRANSFER_DESTINATION, access, ACCESS_TRANSFER_WRITE);
        let to_read = barrier(picture, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_READ);
        shows.fresh = false;
        if regions.is_empty() && old == LAYOUT_TRANSFER_SOURCE {
            return;
        }
        // SAFETY: the commands are being recorded; the places lie in the buffer and the picture.
        unsafe {
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            if !regions.is_empty() {
                (on.vkCmdCopyBufferToImage)(commands, shows.buffer, picture, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr());
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
        }
    }

    /// Presents image `at` of a window's swap chain once blitted into, naming the places that
    /// changed (all of it after a new swap chain, picture or background).
    fn present(&mut self, shows: &mut Shows, at: usize) {
        let [left, top] = shows.at;
        let changed = std::mem::take(&mut shows.changed);
        let named = !std::mem::take(&mut shows.whole) && self.regions;
        let rectangles: Vec<VkRectLayerKHR> = changed.iter().map(|[l, t, r, b]| VkRectLayerKHR { offset: [l + left, t + top], extent: [(r - l) as u32, (b - t) as u32], layer: 0 }).collect();
        let region = VkPresentRegionKHR { count: rectangles.len() as u32, rects: rectangles.as_ptr() };
        let regions = VkPresentRegionsKHR { kind: 1_000_084_000, count: 1, regions: &region, ..Default::default() };
        let index = at as u32;
        let asked = VkPresentInfoKHR {
            kind: 1_000_001_001,
            next: if named { (&raw const regions).cast() } else { null() },
            wait_count: 1,
            waits: &shows.images[at].drawn,
            count: 1,
            swapchains: &shows.swapchain,
            indices: &index,
            results: null_mut(),
        };
        // SAFETY: Vulkan's documented call on the device's queue; everything named lives until
        // it returns.
        let result = unsafe { (self.on.vkQueuePresentKHR)(self.queue, &asked) };
        self.said(shows, result);
        #[cfg(test)]
        {
            shows.named = if named { changed } else { vec![[0, 0, shows.window[0] - left, shows.window[1] - top]] };
        }
    }

    /// Lets go of a window: everything made for it, its surface last. (Before its
    /// `ANativeWindow` goes.)
    pub(crate) fn forget(&mut self, mut shows: Shows) {
        self.idle();
        self.unchain(&mut shows);
        self.unbuffer(&mut shows);
        let (on, device) = (&self.on, self.device);
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be,
        // and a command buffer only freed where there is one.
        unsafe {
            (on.vkDestroySemaphore)(device, shows.acquired, null());
            (on.vkDestroyFence)(device, shows.fence, null());
            if !shows.commands.is_null() {
                (on.vkFreeCommandBuffers)(device, self.pool, 1, &shows.commands);
            }
            (self.vulkan.calls.vkDestroySurfaceKHR)(self.vulkan.instance, shows.surface, null());
        }
    }

    /// Draws a recorded paint into a window's picture, inside `place` (left, top, right,
    /// bottom), after what was put before, to be shown with the next frame; what it draws with
    /// is made the first time. Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [i32; 4]) -> bool {
        if shows.pixels.is_null() || !self.wait(shows) {
            return false;
        }
        let mut made = self.pipeline.take().unwrap_or_else(|| self.drawer());
        let drawn = made.as_mut().is_some_and(|pipeline| self.draw_with(shows, pipeline, list, place));
        self.pipeline = Some(made);
        if drawn {
            shows.changed.push(place);
        }
        drawn
    }

    /// What the device draws with; nothing where it can't be made.
    #[cfg(feature = "gpu-draw")]
    fn drawer(&self) -> Option<Box<crate::gpu_draw_vulkan::Pipeline>> {
        let (find, device, mut memory) = (self.vulkan.calls.vkGetDeviceProcAddr, self.device, VkPhysicalDeviceMemoryProperties::default());
        // SAFETY: Vulkan fills in the device's memory; the names end in zeros.
        unsafe { (self.vulkan.calls.vkGetPhysicalDeviceMemoryProperties)(self.chosen.physical, &mut memory) };
        crate::gpu_draw_vulkan::Pipeline::new(&mut |name| unsafe { find(device, name.as_ptr()) }, device, &memory, false)
    }

    /// Records and runs the paint drawn by `pipeline`, as `draw` says, and waits for it. A
    /// failure loses the device: the window's picture and pixels no longer agree.
    #[cfg(feature = "gpu-draw")]
    fn draw_with(&mut self, shows: &mut Shows, pipeline: &mut crate::gpu_draw_vulkan::Pipeline, list: &crate::gpu_draw::DrawList, place: [i32; 4]) -> bool {
        let (commands, begin) = (shows.commands, VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() });
        // SAFETY: Vulkan's documented calls; the commands are not in use (their fence was
        // waited for).
        unsafe {
            (self.on.vkResetFences)(self.device, 1, &shows.fence);
            (self.on.vkBeginCommandBuffer)(commands, &begin);
        }
        self.fill_picture(shows);
        let was = crate::gpu_draw_vulkan::Was { layout: LAYOUT_TRANSFER_SOURCE, access: ACCESS_TRANSFER_READ, stage: STAGE_TRANSFER };
        let recorded = pipeline.record(commands, shows.picture, shows.size, list, place, was, LAYOUT_TRANSFER_SOURCE);
        let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &commands, ..Default::default() };
        // SAFETY: as above.
        let result = unsafe { if (self.on.vkEndCommandBuffer)(commands) == 0 && recorded { (self.on.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) } else { -1 } };
        (shows.busy, self.lost) = (result == 0, self.lost || result != 0);
        self.wait(shows)
    }

    /// Tests: whether it has drawn the window itself (see `gpu_draw`).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        matches!(self.pipeline, Some(Some(_)))
    }

    /// Tests: the places the last frame named, in the pixels.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[i32; 4]] {
        &shows.named
    }

    /// Tests: the device is lost, as when it is removed: the next frame says so.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        self.faked = true;
    }
}

impl Drop for Gpu {
    /// Lets go of the pool and the device. (The window was let go of first; the instance goes
    /// after.)
    fn drop(&mut self) {
        // SAFETY: Vulkan's documented calls; nothing of the device is used after.
        unsafe {
            (self.on.vkDeviceWaitIdle)(self.device);
            #[cfg(feature = "gpu-draw")]
            drop(self.pipeline.take());
            (self.on.vkDestroyCommandPool)(self.device, self.pool, null());
            (self.on.vkDestroyDevice)(self.device, null());
        }
    }
}
