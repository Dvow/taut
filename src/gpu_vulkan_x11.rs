//! X windows shown through Vulkan, with the feature `gpu`. The CPU draws every pixel as without
//! it (`x11`'s strip); each strip goes into the window's pixels, a buffer the CPU writes and the
//! GPU reads, kept whole. Once a window's changes are all in, the places that changed are copied
//! from it into the swap chain's next image (`vkCmdCopyBufferToImage`), which is presented
//! naming them (`VK_KHR_incremental_present`), in step with the screen (FIFO). No shader touches
//! a pixel, so the pixels shown are the CPU's, byte for byte.
//!
//! An image older than one frame also gets the places of the frames it missed; one not yet
//! shown (new, or after a resize) is copied whole.
//!
//! The surface is X's (`VK_KHR_xlib_surface`), on an X connection of Vulkan's own: its driver
//! may present from a thread of its own. The loader (`libvulkan.so.1`) is opened by name when a
//! window first asks for Vulkan, so a program builds and starts without it; once opened it
//! stays.
//!
//! `Auto` takes a device only where it shows frames directly: not the CPU at work (lavapipe),
//! and through DRI3 where the driver is Mesa's (without it, Mesa puts every frame through X).
use crate::gpu_x11::{KEPT, has_dri3, note, places};
use crate::vk::*;
use crate::x11::{dlclose, dlopen, library, symbol};
use std::ffi::{CStr, c_char, c_int, c_ulong, c_void};
use std::ptr::{null, null_mut};

library!(Connection, ["libX11.so.6", "libX11.so"], {
    fn XOpenDisplay(*const c_char) -> *mut c_void;
    fn XDisplayString(*mut c_void) -> *const c_char;
    fn XCloseDisplay(*mut c_void) -> c_int;
});

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
    vkEnumerateDeviceExtensionProperties: fn(Object, *const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkGetDeviceProcAddr: fn(Object, *const c_char) -> *const c_void;
    vkCreateXlibSurfaceKHR: fn(Object, *const VkXlibSurfaceCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkGetPhysicalDeviceXlibPresentationSupportKHR: fn(Object, u32, *mut c_void, c_ulong) -> u32;
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

/// `vkGetPhysicalDeviceProperties2` (Vulkan 1.1).
type Properties2 = unsafe extern "system" fn(Object, *mut VkPhysicalDeviceProperties2);

/// Vulkan 1.1 and 1.2, as `VK_MAKE_API_VERSION` makes them.
const VULKAN_1_1: u32 = (1 << 22) | (1 << 12);
const VULKAN_1_2: u32 = (1 << 22) | (2 << 12);

/// How long a frame may wait for an image or a copy before the device is taken for lost: a
/// second, in nanoseconds.
const WAITED: u64 = 1_000_000_000;

/// The instance of Vulkan, on an X connection of its own, with the functions it offers.
struct Vulkan {
    xlib: Connection,
    x: *mut c_void,
    instance: Object,
    calls: Instance,
    properties2: Option<Properties2>,
    /// The instance is never destroyed: Mesa's dozen (Vulkan on Direct3D 12, WSL's) crashes at
    /// thread exit after it.
    lasting: bool,
}

/// A device that can show windows: its queue family, whether it is the CPU at work, and
/// whether its driver is Mesa's, and dozen.
struct Chosen {
    physical: Object,
    family: u32,
    cpu: bool,
    mesa: bool,
    dozen: bool,
}

/// The GPU a program's windows are shown through: one device and queue for all of them.
pub(crate) struct Gpu {
    vulkan: Vulkan,
    on: Device,
    chosen: Chosen,
    device: Object,
    queue: Object,
    pool: Handle,
    /// The kinds of memory the CPU writes and the GPU reads, one bit each.
    writable: u32,
    /// Presents name the places that changed.
    regions: bool,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// Where the GPU draws windows too, what it records to, and what it draws with: not yet
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

/// A window shown through Vulkan: its surface and swap chain, its pixels, and what was put
/// into them since the last frame.
pub(crate) struct Shows {
    surface: Handle,
    swapchain: Handle,
    images: Vec<Image>,
    /// The frames shown so far.
    frame: u64,
    extent: [u32; 2],
    /// Signalled once an image is had.
    acquired: Handle,
    /// The pixels, `size` big, in a buffer the CPU writes (mapped at `pixels`).
    size: [c_int; 2],
    buffer: Handle,
    memory: Handle,
    pixels: *mut u32,
    /// The commands copying them, and the fence of their last run, not yet waited for if
    /// `busy`.
    commands: Object,
    fence: Handle,
    busy: bool,
    /// The swap chain no longer fits the window: it is made again before the next frame.
    remake: bool,
    /// The places put since the last frame, and those of the last frames, newest first: left,
    /// top, right, bottom in pixels from the top left.
    put: Vec<[c_int; 4]>,
    past: Vec<Vec<[c_int; 4]>>,
    /// Where the GPU draws the window (`gpu-draw`): the image it draws in (none until it first
    /// draws).
    #[cfg(feature = "gpu-draw")]
    picture: Option<Box<Picture>>,
}

/// An image of a window's pixels the GPU draws in (`gpu-draw`), its memory, and the places put
/// into the buffer since they were copied into it.
#[cfg(feature = "gpu-draw")]
struct Picture {
    image: Handle,
    memory: Handle,
    pending: Vec<[c_int; 4]>,
}

/// An image of a swap chain: the frame it last showed (0: none yet, so its pixels are unknown),
/// and the semaphore signalled once its copy is done.
#[derive(Clone, Copy)]
struct Image {
    image: Handle,
    showed: u64,
    drawn: Handle,
}

impl Shows {
    /// Its pixels' size.
    pub(crate) fn size(&self) -> [c_int; 2] {
        self.size
    }
}

impl Vulkan {
    /// Opens the loader and makes an instance that shows X windows, on an X connection of its
    /// own to the display of `windows`. Nothing where any of it is missing.
    ///
    /// # Safety
    /// `windows` is an open X display.
    unsafe fn open(windows: *mut c_void) -> Option<Vulkan> {
        let get = loader()?;
        // SAFETY: the instance is null or Vulkan's.
        let find = move |instance: Object, name: &CStr| unsafe { get(instance, name.as_ptr()) };
        let global = Global::load(&mut |name| find(null_mut(), name)).ok()?;
        let wanted = [c"VK_KHR_surface".as_ptr(), c"VK_KHR_xlib_surface".as_ptr()];
        // SAFETY: Vulkan's documented enumeration.
        let offers = |name| offered(|count, list| unsafe { (global.vkEnumerateInstanceExtensionProperties)(null(), count, list) }, name);
        if !offers(c"VK_KHR_surface") || !offers(c"VK_KHR_xlib_surface") {
            return None;
        }
        let xlib = Connection::open()?;
        let (instance, api) = instance(&global, &wanted)?;
        let Ok(calls) = Instance::load(&mut |name| find(instance, name)) else {
            let destroy = find(instance, c"vkDestroyInstance");
            // SAFETY: the instance was just made, and nothing of it is used after.
            if !destroy.is_null() {
                unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(instance, null()) };
            }
            return None;
        };
        let found = if api >= VULKAN_1_1 { find(instance, c"vkGetPhysicalDeviceProperties2") } else { null() };
        // SAFETY: the function is declared as vulkan_core.h declares it.
        let properties2 = (!found.is_null()).then(|| unsafe { std::mem::transmute::<*const c_void, Properties2>(found) });
        // SAFETY: Xlib's documented calls; the name is the open display's own.
        let x = unsafe { (xlib.XOpenDisplay)((xlib.XDisplayString)(windows)) };
        let vulkan = Vulkan { xlib, x, instance, calls, properties2, lasting: false };
        (!x.is_null()).then_some(vulkan)
    }

    /// The device and queue family best showing windows of `visual`: a GPU before the CPU at
    /// work; unless `any`, only one showing frames directly (with `dri3` as the display says).
    fn choose(&self, visual: c_ulong, any: bool, dri3: bool) -> Option<Chosen> {
        // SAFETY: Vulkan's documented enumeration.
        let devices = listed(null_mut(), |count, list| unsafe { (self.calls.vkEnumeratePhysicalDevices)(self.instance, count, list) });
        let usable = devices.into_iter().filter_map(|physical| self.chosen(physical, visual));
        usable.filter(|chosen| any || (!chosen.cpu && (dri3 || !chosen.mesa))).min_by_key(|chosen| chosen.cpu)
    }

    /// What device `physical` is, where it can show windows of `visual`: a queue family that
    /// copies (any family does) and presents to them, and swap chains.
    fn chosen(&self, physical: Object, visual: c_ulong) -> Option<Chosen> {
        let calls = &self.calls;
        // SAFETY: Vulkan's documented enumerations, of the instance's device.
        let swaps = offered(|count, list| unsafe { (calls.vkEnumerateDeviceExtensionProperties)(physical, null(), count, list) }, c"VK_KHR_swapchain");
        let families = listed(VkQueueFamilyProperties::default(), |count, list| {
            unsafe { (calls.vkGetPhysicalDeviceQueueFamilyProperties)(physical, count, list) };
            0
        });
        // (Graphics 0x1, compute 0x2, transfer 0x4.)
        let copies = |family: &VkQueueFamilyProperties| family.count > 0 && family.flags & (0x1 | 0x2 | 0x4) != 0;
        // SAFETY: the display is Vulkan's own connection, open.
        let presents = |at: usize| unsafe { (calls.vkGetPhysicalDeviceXlibPresentationSupportKHR)(physical, at as u32, self.x, visual) } != 0;
        let family = (0..families.len()).find(|at| copies(&families[*at]) && presents(*at)).filter(|_| swaps)? as u32;
        let mut properties = VkPhysicalDeviceProperties::default();
        // SAFETY: Vulkan fills in the properties.
        unsafe { (calls.vkGetPhysicalDeviceProperties)(physical, &mut properties) };
        let (mesa, dozen) = self.driver(physical, properties.api_version);
        Some(Chosen { physical, family, cpu: properties.device_type == 4, mesa, dozen })
    }

    /// Whether device `physical` (of Vulkan `api`) has Mesa's driver (as it has where that
    /// can't be learned: before Vulkan 1.2), and whether that is dozen.
    fn driver(&self, physical: Object, api: u32) -> (bool, bool) {
        let Some(properties2) = self.properties2.filter(|_| api >= VULKAN_1_2) else { return (true, false) };
        let mut driver = VkPhysicalDeviceDriverProperties { kind: 1_000_196_000, ..Default::default() };
        let mut asked = VkPhysicalDeviceProperties2 { kind: 1_000_059_001, next: (&raw mut driver).cast(), ..Default::default() };
        // SAFETY: Vulkan fills in both; the driver's words end in a zero within their array.
        let info = unsafe {
            properties2(physical, &mut asked);
            CStr::from_ptr(driver.info.as_ptr())
        };
        // (`VK_DRIVER_ID_MESA_DOZEN` is 23.)
        (driver.id == 0 || info.to_bytes().starts_with(b"Mesa"), driver.id == 23)
    }
}

impl Drop for Vulkan {
    /// Lets go of the instance, then of its X connection. (Surfaces and devices went first.)
    fn drop(&mut self) {
        // SAFETY: nothing made from either is used after.
        unsafe {
            if !self.lasting {
                (self.calls.vkDestroyInstance)(self.instance, null());
            }
            if !self.x.is_null() {
                (self.xlib.XCloseDisplay)(self.x);
            }
        }
    }
}

/// An instance with `extensions`, of Vulkan 1.1 where the loader takes it, else 1.0; and which.
fn instance(global: &Global, extensions: &[*const c_char]) -> Option<(Object, u32)> {
    for api in [VULKAN_1_1, 1 << 22] {
        let application = VkApplicationInfo { kind: 0, api_version: api, ..Default::default() };
        let asked = VkInstanceCreateInfo { kind: 1, application: &application, extension_count: extensions.len() as u32, extensions: extensions.as_ptr(), ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: Vulkan's documented call; the names end in zeros.
        if unsafe { (global.vkCreateInstance)(&asked, null(), &mut instance) } == 0 {
            return Some((instance, api));
        }
    }
    None
}

impl Gpu {
    /// Opens Vulkan on X display `x` and shows window `id` through it (`size` pixels, of
    /// `visual`). Nothing where Vulkan, a device presenting to X or anything else is missing,
    /// or where frames would not be shown directly and `any` is false.
    ///
    /// # Safety
    /// `x` is an open X display, and `id` one of its windows, alive, of `visual`.
    pub(crate) unsafe fn open(x: *mut c_void, visual: *mut c_void, id: c_ulong, size: [c_int; 2], any: bool) -> Option<(Gpu, Shows)> {
        // SAFETY: as the caller promises. A `Visual` starts with a pointer, then its id
        // (Xlib.h).
        let (vulkan, visual, dri3) = unsafe { (Vulkan::open(x)?, *visual.cast::<c_ulong>().add(1), any || has_dri3(x)) };
        let chosen = vulkan.choose(visual, any, dri3)?;
        #[cfg(feature = "gpu-draw")]
        let cpu = chosen.cpu;
        let mut gpu = Gpu::make(vulkan, chosen)?;
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !cpu, std::env::var("TAUT_DRAW").ok().as_deref());
        }
        let shows = gpu.window(id, size)?;
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
        // SAFETY: Vulkan fills in the device's memory.
        unsafe { (calls.vkGetPhysicalDeviceMemoryProperties)(physical, &mut memory) };
        let types = memory.types.iter().take(memory.type_count as usize).enumerate();
        let writable = types.filter(|(_, kind)| kind.flags & HOST_VISIBLE_AND_COHERENT == HOST_VISIBLE_AND_COHERENT).fold(0, |all, (at, _)| all | 1 << at);
        // SAFETY: Vulkan's documented calls; the names end in zeros.
        if unsafe { (calls.vkCreateDevice)(physical, &asked, null(), &mut device) } != 0 {
            return None;
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
            writable,
            regions,
            lost: false,
            #[cfg(test)]
            faked: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
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

    /// Tests: the device's name, from Vulkan (as "llvmpipe (LLVM 21.1.8, 256 bits)").
    #[cfg(test)]
    pub(crate) fn name(&self) -> String {
        let mut properties = VkPhysicalDeviceProperties::default();
        // SAFETY: Vulkan fills in the properties; the name ends in a zero within its array.
        unsafe {
            (self.vulkan.calls.vkGetPhysicalDeviceProperties)(self.chosen.physical, &mut properties);
            CStr::from_ptr(properties.name.as_ptr()).to_string_lossy().into_owned()
        }
    }

    /// Whether frames are shown directly: a device that is not the CPU at work, and X display
    /// `x` offers DRI3 where the driver is Mesa's.
    ///
    /// # Safety
    /// `x` is an open X display.
    #[cfg(test)]
    pub(crate) unsafe fn direct(&self, x: *mut c_void) -> bool {
        // SAFETY: as the caller promises.
        !self.chosen.cpu && (!self.chosen.mesa || unsafe { has_dri3(x) })
    }

    /// Shows window `id` of the display through the device: its surface, swap chain and pixels,
    /// `size` big. Nothing where any of it can't be made.
    pub(crate) fn window(&mut self, id: c_ulong, size: [c_int; 2]) -> Option<Shows> {
        let (calls, instance) = (&self.vulkan.calls, self.vulkan.instance);
        let asked = VkXlibSurfaceCreateInfoKHR { kind: 1_000_004_000, display: self.vulkan.x, window: id, ..Default::default() };
        let (mut surface, mut supported) = (0, 0);
        // SAFETY: the display is Vulkan's own connection, open; the window is alive (the
        // caller's), and the family the device's.
        unsafe {
            if (calls.vkCreateXlibSurfaceKHR)(instance, &asked, null(), &mut surface) != 0 {
                return None;
            }
            (calls.vkGetPhysicalDeviceSurfaceSupportKHR)(self.chosen.physical, self.chosen.family, surface, &mut supported);
        }
        let mut shows = Shows {
            surface,
            swapchain: 0,
            images: Vec::new(),
            frame: 0,
            extent: [0; 2],
            acquired: 0,
            size: [0; 2],
            buffer: 0,
            memory: 0,
            pixels: null_mut(),
            commands: null_mut(),
            fence: 0,
            busy: false,
            remake: false,
            put: Vec::new(),
            past: Vec::new(),
            #[cfg(feature = "gpu-draw")]
            picture: None,
        };
        if supported != 0 && self.parts(&mut shows) && self.fit(&mut shows, size) {
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

    /// Gives a window's pixels and swap chain its size. Says whether it has them: where the
    /// device makes none that big, the window is better shown by X.
    pub(crate) fn fit(&mut self, shows: &mut Shows, size: [c_int; 2]) -> bool {
        if shows.size == size && shows.swapchain != 0 {
            return true;
        }
        self.idle();
        self.unbuffer(shows);
        (shows.size, shows.past) = (size, Vec::new());
        size.iter().all(|side| *side > 0) && self.buffer(shows) && self.chain(shows)
    }

    /// Makes a window's pixels: a buffer its size, in memory the CPU writes and the GPU reads,
    /// mapped. Says whether it could.
    fn buffer(&mut self, shows: &mut Shows) -> bool {
        let (on, device, [width, height]) = (&self.on, self.device, shows.size);
        let bytes = width as u64 * height as u64 * 4;
        let asked = VkBufferCreateInfo { kind: 12, size: bytes, usage: USAGE_TRANSFER_SOURCE, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (on.vkCreateBuffer)(device, &asked, null(), &mut shows.buffer) != 0 {
                return false;
            }
            (on.vkGetBufferMemoryRequirements)(device, shows.buffer, &mut needs);
        }
        let fitting = needs.type_bits & self.writable;
        if fitting == 0 {
            return false;
        }
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: fitting.trailing_zeros(), ..Default::default() };
        let mut pixels = null_mut();
        // SAFETY: as above; the memory is bound whole and mapped whole.
        let made = unsafe {
            (on.vkAllocateMemory)(device, &asked, null(), &mut shows.memory) == 0
                && (on.vkBindBufferMemory)(device, shows.buffer, shows.memory, 0) == 0
                && (on.vkMapMemory)(device, shows.memory, 0, needs.size, 0, &mut pixels) == 0
        };
        shows.pixels = pixels.cast();
        made && !pixels.is_null()
    }

    /// Lets go of a window's pixels, and the image the GPU draws them in.
    fn unbuffer(&mut self, shows: &mut Shows) {
        #[cfg(feature = "gpu-draw")]
        if let (Some(pipeline), Some(picture)) = (self.pipeline.as_ref().and_then(|made| made.as_deref()), shows.picture.take()) {
            pipeline.unpicture((picture.image, picture.memory));
        }
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            (self.on.vkDestroyBuffer)(self.device, shows.buffer, null());
            (self.on.vkFreeMemory)(self.device, shows.memory, null());
        }
        (shows.buffer, shows.memory, shows.pixels) = (0, 0, null_mut());
    }

    /// Makes a window's swap chain (again) at the size its surface has now: images of 8-bit
    /// blue, green, red and alpha, copied into, opaque, presented in step with the screen. Says
    /// whether it could.
    fn chain(&mut self, shows: &mut Shows) -> bool {
        let (calls, physical) = (&self.vulkan.calls, self.chosen.physical);
        let mut can = VkSurfaceCapabilitiesKHR::default();
        // SAFETY: Vulkan's documented calls on the window's surface.
        if unsafe { (calls.vkGetPhysicalDeviceSurfaceCapabilitiesKHR)(physical, shows.surface, &mut can) } != 0 {
            return false;
        }
        let formats = listed(VkSurfaceFormatKHR::default(), |count, list| unsafe { (calls.vkGetPhysicalDeviceSurfaceFormatsKHR)(physical, shows.surface, count, list) });
        // (sRGB's colour space is 0; alpha ignored 0x1, or as the window system has it 0x8.)
        let format = formats.into_iter().find(|offered| offered.format == B8G8R8A8_UNORM && offered.colour_space == 0);
        let alpha = [0x1, 0x8].into_iter().find(|alpha| can.alphas & alpha != 0);
        let extent = if can.current[0] == u32::MAX { shows.size.map(|side| side as u32) } else { can.current };
        let (Some(format), Some(alpha)) = (format, alpha) else { return false };
        let asked = VkSwapchainCreateInfoKHR {
            kind: 1_000_001_000,
            surface: shows.surface,
            min_images: can.min_images,
            format: format.format,
            colour_space: format.colour_space,
            extent,
            layers: 1,
            usage: USAGE_TRANSFER_DESTINATION,
            transform: can.transform,
            alpha,
            present_mode: 2, // FIFO
            old: shows.swapchain,
            ..Default::default()
        };
        let mut made = 0;
        self.idle();
        // SAFETY: Vulkan's documented call; the old chain is retired by it, and let go after.
        let result = if can.usage & USAGE_TRANSFER_DESTINATION == 0 || extent.contains(&0) { -1 } else { unsafe { (self.on.vkCreateSwapchainKHR)(self.device, &asked, null(), &mut made) } };
        self.unchain(shows);
        if result != 0 {
            return false;
        }
        // SAFETY: Vulkan's documented enumeration of the chain's images.
        let images = listed(0, |count, list| unsafe { (self.on.vkGetSwapchainImagesKHR)(self.device, made, count, list) });
        (shows.swapchain, shows.extent, shows.remake) = (made, extent, false);
        shows.images = images.into_iter().map(|image| Image { image, showed: 0, drawn: self.semaphore() }).collect();
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

    /// Waits until a window's last copy is done, if it is not known to be. Says whether it is;
    /// if not, the device is lost.
    fn wait(&mut self, shows: &mut Shows) -> bool {
        // SAFETY: the fence is the device's.
        if shows.busy && unsafe { (self.on.vkWaitForFences)(self.device, 1, &shows.fence, 1, WAITED) } != 0 {
            self.lost = true;
        }
        shows.busy = false;
        !self.lost
    }

    /// Puts part of a painted strip into a window's pixels: `place` (left, top, right, bottom
    /// in the window's pixels), the strip's first row being its top, `width` pixels a row.
    pub(crate) fn put(&mut self, shows: &mut Shows, strip: &[u32], width: c_int, place: [c_int; 4]) {
        let ([left, top, right, bottom], [wide, high]) = (place, shows.size);
        let inside = 0 <= left && left < right && right <= wide && 0 <= top && top < bottom && bottom <= high && width == wide;
        if !inside || strip.len() < (width * (bottom - top)) as usize || shows.pixels.is_null() || !self.wait(shows) {
            return;
        }
        let (rows, wide, (left, top, right)) = ((bottom - top) as usize, wide as usize, (left as usize, top as usize, right as usize));
        // Whole rows go at once.
        let (count, length) = if right - left == wide { (1, wide * rows) } else { (rows, right - left) };
        for row in 0..count {
            // SAFETY: the pixels are mapped, `wide` by `high`, and the place lies inside them
            // and inside the strip.
            unsafe { std::ptr::copy_nonoverlapping(strip.as_ptr().add(row * wide + left), shows.pixels.add((top + row) * wide + left), length) };
        }
        note(&mut shows.put, place);
        #[cfg(feature = "gpu-draw")]
        if let Some(picture) = shows.picture.as_mut() {
            note(&mut picture.pending, place);
        }
    }

    /// Shows what was put into a window's pixels since they were last shown: copied into the
    /// next image where they changed (and where that image lacks older frames), and presented
    /// naming those places. A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows) {
        if shows.put.is_empty() || !self.wait(shows) {
            return;
        }
        #[cfg(test)]
        if self.faked {
            self.lost = true;
            return;
        }
        if shows.remake && !self.chain(shows) {
            self.lost = true;
            return;
        }
        let Some(at) = self.acquire(shows) else { return };
        let showed = shows.images[at].showed;
        let missed = (showed != 0).then(|| (shows.frame - showed) as usize);
        let wanted = places(&shows.put, &shows.past, missed, shows.size);
        if self.copy(shows, at, &wanted) {
            self.present(shows, at);
        }
    }

    /// The next image of a window's swap chain to copy into, made again first if it no longer
    /// fits. Nothing where none came: then the places put wait for the next frame (or the device
    /// is lost).
    fn acquire(&mut self, shows: &mut Shows) -> Option<usize> {
        let mut at = 0;
        // SAFETY: Vulkan's documented call on the window's swap chain; the semaphore is free
        // (the copy that waited for it is done).
        let next = |gpu: &Gpu, shows: &Shows, at: &mut u32| unsafe { (gpu.on.vkAcquireNextImageKHR)(gpu.device, shows.swapchain, WAITED, shows.acquired, 0, at) };
        let mut result = next(self, shows, &mut at);
        if result == OUT_OF_DATE && self.chain(shows) {
            result = next(self, shows, &mut at);
        }
        shows.remake |= result == SUBOPTIMAL || result == OUT_OF_DATE;
        self.lost |= result < 0 && result != OUT_OF_DATE;
        let had = result == 0 || result == SUBOPTIMAL;
        (had && (at as usize) < shows.images.len()).then_some(at as usize)
    }

    /// Copies `wanted` places of a window's pixels into image `at` of its swap chain, once the
    /// image is had. Says whether the copy was given to the queue; if not, the device is lost.
    #[cfg(not(feature = "gpu-draw"))]
    fn copy(&mut self, shows: &mut Shows, at: usize, wanted: &[[c_int; 4]]) -> bool {
        let (on, commands, Image { image, showed, drawn }, width) = (&self.on, shows.commands, shows.images[at], shows.size[0]);
        let regions: Vec<VkBufferImageCopy> = wanted.iter().filter_map(|place| within(place, shows.extent)).map(|place| region(place, width)).collect();
        let old = if showed == 0 { LAYOUT_UNDEFINED } else { LAYOUT_PRESENT };
        let before = barrier(image, old, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
        let after = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        let begin = VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() };
        // SAFETY: Vulkan's documented calls; the commands are not in use (their fence was
        // waited for), the buffer and image are the window's.
        let recorded = unsafe {
            (on.vkResetFences)(self.device, 1, &shows.fence);
            (on.vkBeginCommandBuffer)(commands, &begin);
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &before);
            if !regions.is_empty() {
                (on.vkCmdCopyBufferToImage)(commands, shows.buffer, image, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr());
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &after);
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

    /// Copies `wanted` places of a window's pixels into image `at` of its swap chain, once the
    /// image is had. Says whether the copy was given to the queue; if not, the device is lost.
    #[cfg(feature = "gpu-draw")]
    fn copy(&mut self, shows: &mut Shows, at: usize, wanted: &[[c_int; 4]]) -> bool {
        let (on, commands, Image { image, showed, drawn }, width) = (&self.on, shows.commands, shows.images[at], shows.size[0]);
        let regions: Vec<VkBufferImageCopy> = wanted.iter().filter_map(|place| within(place, shows.extent)).map(|place| region(place, width)).collect();
        let old = if showed == 0 { LAYOUT_UNDEFINED } else { LAYOUT_PRESENT };
        let before = barrier(image, old, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
        let after = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        let begin = VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() };
        // SAFETY: Vulkan's documented calls; the commands are not in use (their fence was
        // waited for), the buffer and image are the window's.
        unsafe {
            (on.vkResetFences)(self.device, 1, &shows.fence);
            (on.vkBeginCommandBuffer)(commands, &begin);
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &before);
        }
        if !self.copied_from_picture(shows, image, wanted) && !regions.is_empty() {
            // SAFETY: as above; the places lie in the buffer and the image.
            unsafe { (on.vkCmdCopyBufferToImage)(commands, shows.buffer, image, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr()) };
        }
        // SAFETY: as above.
        let recorded = unsafe {
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &after);
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

    /// Records `wanted` places copied into swap chain image `image` from the image the GPU draws
    /// the window in, where it has one (what was put since going into it first). Says whether
    /// it did; if not, they are copied from the buffer.
    #[cfg(feature = "gpu-draw")]
    fn copied_from_picture(&self, shows: &mut Shows, image: Handle, wanted: &[[c_int; 4]]) -> bool {
        let Some(pipeline) = self.pipeline.as_ref().and_then(|made| made.as_deref()) else { return false };
        let Some(from) = shows.picture.as_ref().map(|picture| picture.image) else { return false };
        self.fill_picture(shows, false);
        let inside: Vec<[c_int; 4]> = wanted.iter().filter_map(|place| within(place, shows.extent)).collect();
        pipeline.copy_places(shows.commands, from, image, &inside);
        true
    }

    /// Records the places put since copied into the image the GPU draws the window in (all of
    /// it, if `fresh`: just made), which is then ready to be copied from.
    #[cfg(feature = "gpu-draw")]
    fn fill_picture(&self, shows: &mut Shows, fresh: bool) {
        let ([width, height], buffer, commands) = (shows.size, shows.buffer, shows.commands);
        let Some(held) = shows.picture.as_mut().filter(|held| fresh || !held.pending.is_empty()) else { return };
        let picture = held.image;
        let places = if fresh { vec![[0, 0, width, height]] } else { std::mem::take(&mut held.pending) };
        held.pending.clear();
        let regions: Vec<VkBufferImageCopy> = places.into_iter().map(|place| region(place, width)).collect();
        let (old, access) = if fresh { (LAYOUT_UNDEFINED, 0) } else { (LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_READ) };
        let to_copy = barrier(picture, old, LAYOUT_TRANSFER_DESTINATION, access, ACCESS_TRANSFER_WRITE);
        let to_read = barrier(picture, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_SOURCE, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_READ);
        let on = &self.on;
        // SAFETY: the commands are being recorded; the places lie in the buffer and the image.
        unsafe {
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_copy);
            if !regions.is_empty() {
                (on.vkCmdCopyBufferToImage)(commands, buffer, picture, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr());
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_read);
        }
    }

    /// Draws a recorded paint into the image the GPU draws a window in (made the first time,
    /// from the buffer), inside `place` (left, top, right, bottom), to be shown with what was put
    /// in; what it draws with is made the first time. Says whether it could: if not, the CPU is
    /// to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        if shows.pixels.is_null() || !self.wait(shows) {
            return false;
        }
        let mut made = self.pipeline.take().unwrap_or_else(|| self.drawer());
        let drawn = made.as_mut().is_some_and(|pipeline| self.draw_with(shows, pipeline, list, place));
        self.pipeline = Some(made);
        if drawn {
            note(&mut shows.put, place);
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
    /// failure loses the device: the window's image and buffer no longer agree.
    #[cfg(feature = "gpu-draw")]
    fn draw_with(&mut self, shows: &mut Shows, pipeline: &mut crate::gpu_draw_vulkan::Pipeline, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        let fresh = shows.picture.is_none();
        if fresh {
            let Some((image, memory)) = pipeline.picture(shows.size) else { return false };
            shows.picture = Some(Box::new(Picture { image, memory, pending: Vec::new() }));
        }
        let picture = shows.picture.as_ref().map_or(0, |picture| picture.image);
        let (commands, begin) = (shows.commands, VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() });
        // SAFETY: Vulkan's documented calls; the commands are not in use (their fence was
        // waited for).
        unsafe {
            (self.on.vkResetFences)(self.device, 1, &shows.fence);
            (self.on.vkBeginCommandBuffer)(commands, &begin);
        }
        self.fill_picture(shows, fresh);
        let was = crate::gpu_draw_vulkan::Was { layout: LAYOUT_TRANSFER_SOURCE, access: ACCESS_TRANSFER_READ, stage: STAGE_TRANSFER };
        let recorded = pipeline.record(commands, picture, shows.size, list, place, was, LAYOUT_TRANSFER_SOURCE);
        let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &commands, ..Default::default() };
        // SAFETY: as above.
        let result = unsafe { if (self.on.vkEndCommandBuffer)(commands) == 0 && recorded { (self.on.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) } else { -1 } };
        (shows.busy, self.lost) = (result == 0, self.lost || result != 0);
        let done = self.wait(shows);
        pipeline.forget_target();
        done
    }

    /// Tests: whether it has drawn windows itself (see `gpu_draw`).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        matches!(self.pipeline, Some(Some(_)))
    }

    /// Presents image `at` of a window's swap chain once copied into, naming the places put.
    fn present(&mut self, shows: &mut Shows, at: usize) {
        let rectangles: Vec<VkRectLayerKHR> = shows
            .put
            .iter()
            .filter_map(|place| within(place, shows.extent))
            .map(|[left, top, right, bottom]| VkRectLayerKHR { offset: [left, top], extent: [(right - left) as u32, (bottom - top) as u32], layer: 0 })
            .collect();
        let region = VkPresentRegionKHR { count: rectangles.len() as u32, rects: rectangles.as_ptr() };
        let regions = VkPresentRegionsKHR { kind: 1_000_084_000, count: 1, regions: &region, ..Default::default() };
        let index = at as u32;
        let asked = VkPresentInfoKHR {
            kind: 1_000_001_001,
            next: if self.regions { (&raw const regions).cast() } else { null() },
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
        shows.remake |= result == SUBOPTIMAL || result == OUT_OF_DATE;
        self.lost |= result < 0 && result != OUT_OF_DATE;
        shows.frame += 1;
        shows.images[at].showed = shows.frame;
        shows.past.insert(0, std::mem::take(&mut shows.put));
        shows.past.truncate(KEPT);
    }

    /// Lets go of a window: everything made for it, its surface last.
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

    /// Tests: the places the last frame named.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        shows.past.first().map_or(&[], Vec::as_slice)
    }

    /// Tests: the device is lost, as when it is removed: the next frame says so.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        self.faked = true;
    }
}

impl Drop for Gpu {
    /// Lets go of the pool and the device. (Windows were let go of first; the instance goes
    /// after, unless the device is dozen's.)
    fn drop(&mut self) {
        // SAFETY: Vulkan's documented calls; nothing of the device is used after.
        unsafe {
            (self.on.vkDeviceWaitIdle)(self.device);
            #[cfg(feature = "gpu-draw")]
            drop(self.pipeline.take());
            (self.on.vkDestroyCommandPool)(self.device, self.pool, null());
            (self.on.vkDestroyDevice)(self.device, null());
        }
        self.vulkan.lasting = self.chosen.dozen;
    }
}

/// The part of `place` (left, top, right, bottom) within an image `extent` big: nothing where
/// none of it is.
fn within(place: &[c_int; 4], extent: [u32; 2]) -> Option<[c_int; 4]> {
    let [left, top] = [place[0].max(0), place[1].max(0)];
    let [right, bottom] = [place[2].min(extent[0] as c_int), place[3].min(extent[1] as c_int)];
    (left < right && top < bottom).then_some([left, top, right, bottom])
}
