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
//! stays. What Android's path shares is `gpu_vulkan`'s.
//!
//! `Auto` takes a device only where it shows frames directly: not the CPU at work (lavapipe),
//! and through DRI3 where the driver is Mesa's (without it, Mesa puts every frame through X).
use crate::dynamic_library::library;
use crate::gpu_vulkan::{Chain, VULKAN_1_1, Vulkan};
use crate::gpu_x11::{KEPT, has_dri3, note, places};
use crate::vk::*;
use std::ffi::{CStr, c_char, c_int, c_ulong, c_void};
use std::ptr::null;

library!(Connection, ["libX11.so.6", "libX11.so"], {
    fn XOpenDisplay(*const c_char) -> *mut c_void;
    fn XDisplayString(*mut c_void) -> *const c_char;
    fn XCloseDisplay(*mut c_void) -> c_int;
});

functions!(XlibCalls {
    vkCreateXlibSurfaceKHR: fn(Object, *const VkXlibSurfaceCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkGetPhysicalDeviceXlibPresentationSupportKHR: fn(Object, u32, *mut c_void, c_ulong) -> u32;
});

/// `vkGetPhysicalDeviceProperties2` (Vulkan 1.1).
type Properties2 = unsafe extern "system" fn(Object, *mut VkPhysicalDeviceProperties2);

/// Vulkan 1.2, as `VK_MAKE_API_VERSION` makes it.
const VULKAN_1_2: u32 = (1 << 22) | (2 << 12);

/// How long a frame may wait for an image or a copy before the device is taken for lost: a
/// second, in nanoseconds.
const WAITED: u64 = 1_000_000_000;

/// The GPU a program's windows are shown through: one device and queue for all of them.
pub(crate) type Gpu = crate::gpu_vulkan::Gpu<Xlib>;

/// A window shown through Vulkan.
pub(crate) type Shows = crate::gpu_vulkan::Shows<Frames>;

/// What X windows keep of the GPU: Vulkan's own X connection (closed after the instance), and
/// the functions making surfaces on it.
pub(crate) struct Xlib {
    calls: XlibCalls,
    connection: Connection,
    x: *mut c_void,
    /// Tests: whether the device is the CPU at work, and whether its driver is Mesa's.
    #[cfg(test)]
    chosen: (bool, bool),
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

/// What an X window keeps of its frames: how many were shown, the swap chain's size and the
/// frame each of its images last showed (0: none yet, so its pixels are unknown); the places put
/// since the last frame, and those of the last frames, newest first (left, top, right, bottom in
/// pixels from the top left); and where the GPU draws the window (`gpu-draw`), the image it draws
/// in (none until it first draws).
#[derive(Default)]
pub(crate) struct Frames {
    frame: u64,
    extent: [u32; 2],
    showed: Vec<u64>,
    put: Vec<[c_int; 4]>,
    past: Vec<Vec<[c_int; 4]>>,
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

impl Shows {
    /// Its pixels' size.
    pub(crate) fn size(&self) -> [c_int; 2] {
        self.size
    }
}

impl Xlib {
    /// The functions making surfaces of `vulkan`, on an X connection of Vulkan's own to the
    /// display of `windows`. Nothing where any is missing.
    ///
    /// # Safety
    /// `windows` is an open X display.
    unsafe fn open(vulkan: &Vulkan, windows: *mut c_void) -> Option<Xlib> {
        let calls = XlibCalls::load(&mut |name| vulkan.function(name)).ok()?;
        let connection = Connection::open()?;
        // SAFETY: Xlib's documented calls; the name is the open display's own.
        let x = unsafe { (connection.XOpenDisplay)((connection.XDisplayString)(windows)) };
        let xlib = Xlib {
            calls,
            connection,
            x,
            #[cfg(test)]
            chosen: (false, false),
        };
        (!x.is_null()).then_some(xlib)
    }

    /// The device and queue family of `vulkan` (made for Vulkan `api`) best showing windows of
    /// `visual`: a GPU before the CPU at work; unless `any`, only one showing frames directly
    /// (with `dri3` as the display says).
    fn choose(&self, vulkan: &Vulkan, api: u32, visual: c_ulong, any: bool, dri3: bool) -> Option<Chosen> {
        let found = if api >= VULKAN_1_1 { vulkan.function(c"vkGetPhysicalDeviceProperties2") } else { null() };
        // SAFETY: the function is declared as vulkan_core.h declares it.
        let properties2 = (!found.is_null()).then(|| unsafe { std::mem::transmute::<*const c_void, Properties2>(found) });
        let usable = vulkan.devices().into_iter().filter_map(|physical| self.chosen(vulkan, properties2, physical, visual));
        usable.filter(|chosen| any || (!chosen.cpu && (dri3 || !chosen.mesa))).min_by_key(|chosen| chosen.cpu)
    }

    /// What device `physical` is, where it can show windows of `visual`: a queue family that
    /// copies (any family does) and presents to them, and swap chains.
    fn chosen(&self, vulkan: &Vulkan, properties2: Option<Properties2>, physical: Object, visual: c_ulong) -> Option<Chosen> {
        let families = vulkan.families(physical);
        // (Graphics 0x1, compute 0x2, transfer 0x4.)
        let copies = |family: &VkQueueFamilyProperties| family.count > 0 && family.flags & (0x1 | 0x2 | 0x4) != 0;
        // SAFETY: the display is Vulkan's own connection, open.
        let presents = |at: usize| unsafe { (self.calls.vkGetPhysicalDeviceXlibPresentationSupportKHR)(physical, at as u32, self.x, visual) } != 0;
        let family = (0..families.len()).find(|at| copies(&families[*at]) && presents(*at)).filter(|_| vulkan.offers(physical, c"VK_KHR_swapchain"))? as u32;
        let properties = vulkan.properties(physical);
        let (mesa, dozen) = driver(properties2.filter(|_| properties.api_version >= VULKAN_1_2), physical);
        Some(Chosen { physical, family, cpu: properties.device_type == 4, mesa, dozen })
    }
}

impl Drop for Xlib {
    /// Closes Vulkan's X connection. (The instance went first.)
    fn drop(&mut self) {
        if !self.x.is_null() {
            // SAFETY: nothing of Vulkan's uses it after.
            unsafe { (self.connection.XCloseDisplay)(self.x) };
        }
    }
}

/// Whether device `physical` has Mesa's driver (as it has where that can't be learned: without
/// `properties2`, or before Vulkan 1.2), and whether that is dozen.
fn driver(properties2: Option<Properties2>, physical: Object) -> (bool, bool) {
    let Some(properties2) = properties2 else { return (true, false) };
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

impl Gpu {
    /// Opens Vulkan on X display `x` and shows window `id` through it (`size` pixels, of
    /// `visual`). Nothing where Vulkan, a device presenting to X or anything else is missing,
    /// or where frames would not be shown directly and `any` is false.
    ///
    /// # Safety
    /// `x` is an open X display, and `id` one of its windows, alive, of `visual`.
    pub(crate) unsafe fn open(x: *mut c_void, visual: *mut c_void, id: c_ulong, size: [c_int; 2], any: bool) -> Option<(Gpu, Shows)> {
        let (mut vulkan, api) = Vulkan::open(&[c"VK_KHR_surface", c"VK_KHR_xlib_surface"])?;
        // SAFETY: as the caller promises. A `Visual` starts with a pointer, then its id
        // (Xlib.h).
        let (xlib, visual, dri3) = unsafe { (Xlib::open(&vulkan, x)?, *visual.cast::<c_ulong>().add(1), any || has_dri3(x)) };
        let Some(chosen) = xlib.choose(&vulkan, api, visual, any, dri3) else {
            // (The instance goes before Vulkan's X connection.)
            drop(vulkan);
            return None;
        };
        vulkan.lasting = chosen.dozen;
        #[cfg(test)]
        let xlib = {
            let mut xlib = xlib;
            xlib.chosen = (chosen.cpu, chosen.mesa);
            xlib
        };
        let mut gpu = Gpu::make(vulkan, (chosen.physical, chosen.family), WAITED, move |_| Some(xlib))?;
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !chosen.cpu, std::env::var("TAUT_DRAW").ok().as_deref());
        }
        let shows = gpu.window(id, size)?;
        Some((gpu, shows))
    }

    /// Whether frames are shown directly: a device that is not the CPU at work, and X display
    /// `x` offers DRI3 where the driver is Mesa's.
    ///
    /// # Safety
    /// `x` is an open X display.
    #[cfg(test)]
    pub(crate) unsafe fn direct(&self, x: *mut c_void) -> bool {
        let (cpu, mesa) = self.system.chosen;
        // SAFETY: as the caller promises.
        !cpu && (!mesa || unsafe { has_dri3(x) })
    }

    /// Shows window `id` of the display through the device: its surface, swap chain and pixels,
    /// `size` big. Nothing where any of it can't be made.
    pub(crate) fn window(&mut self, id: c_ulong, size: [c_int; 2]) -> Option<Shows> {
        let asked = VkXlibSurfaceCreateInfoKHR { kind: 1_000_004_000, display: self.system.x, window: id, ..Default::default() };
        let (mut surface, mut supported) = (0, 0);
        // SAFETY: the display is Vulkan's own connection, open; the window is alive (the
        // caller's), and the family the device's.
        unsafe {
            if (self.system.calls.vkCreateXlibSurfaceKHR)(self.vulkan.instance, &asked, null(), &mut surface) != 0 {
                return None;
            }
            (self.vulkan.calls.vkGetPhysicalDeviceSurfaceSupportKHR)(self.physical, self.family, surface, &mut supported);
        }
        let mut shows = self.shows(surface, Frames::default())?;
        if supported != 0 && self.fit(&mut shows, size) {
            return Some(shows);
        }
        self.forget(shows);
        None
    }

    /// Gives a window's pixels and swap chain its size. Says whether it has them: where the
    /// device makes none that big, the window is better shown by X.
    pub(crate) fn fit(&mut self, shows: &mut Shows, size: [c_int; 2]) -> bool {
        if shows.size == size && shows.swapchain != 0 {
            return true;
        }
        self.idle();
        self.unpicture(shows);
        shows.system.past = Vec::new();
        self.buffer(shows, size) && self.chain(shows)
    }

    /// Lets go of the image the GPU draws a window in. (The queue is idle.)
    #[cfg(feature = "gpu-draw")]
    fn unpicture(&mut self, shows: &mut Shows) {
        if let (Some(pipeline), Some(picture)) = (self.pipeline.as_ref().and_then(|made| made.as_deref()), shows.system.picture.take()) {
            pipeline.unpicture((picture.image, picture.memory));
        }
    }

    /// Without `gpu-draw` the GPU draws no window: nothing to let go of.
    #[cfg(not(feature = "gpu-draw"))]
    #[inline]
    fn unpicture(&mut self, _: &mut Shows) {}

    /// Makes a window's swap chain (again) at the size its surface has now: images of 8-bit
    /// blue, green, red and alpha, copied into, opaque. Says whether it could.
    fn chain(&mut self, shows: &mut Shows) -> bool {
        let Ok(can) = self.capabilities(shows) else { return false };
        // (sRGB's colour space is 0; alpha ignored 0x1, or as the window system has it 0x8.)
        let format = self.formats(shows).into_iter().find(|offered| offered.format == B8G8R8A8_UNORM && offered.colour_space == 0);
        let alpha = [0x1, 0x8].into_iter().find(|alpha| can.alphas & alpha != 0);
        let extent = if can.current[0] == u32::MAX { shows.size.map(|side| side as u32) } else { can.current };
        let (Some(format), Some(alpha)) = (format, alpha) else { return false };
        let refused = can.usage & USAGE_TRANSFER_DESTINATION == 0 || extent.contains(&0);
        let chain = Chain { format, extent, transform: can.transform, alpha, images: can.min_images };
        if self.make_chain(shows, chain, refused) != 0 {
            return false;
        }
        (shows.system.extent, shows.system.showed) = (extent, vec![0; shows.images.len()]);
        true
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
        note(&mut shows.system.put, place);
        #[cfg(feature = "gpu-draw")]
        if let Some(picture) = shows.system.picture.as_mut() {
            note(&mut picture.pending, place);
        }
    }

    /// Shows what was put into a window's pixels since they were last shown: copied into the
    /// next image where they changed (and where that image lacks older frames), and presented
    /// naming those places. A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows) {
        if shows.system.put.is_empty() || !self.wait(shows) {
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
        let showed = shows.system.showed[at];
        let missed = (showed != 0).then(|| (shows.system.frame - showed) as usize);
        let wanted = places(&shows.system.put, &shows.system.past, missed, shows.size);
        if self.copy(shows, at, &wanted) {
            self.present(shows, at);
        }
    }

    /// The next image of a window's swap chain to copy into, made again first if it no longer
    /// fits. Nothing where none came: then the places put wait for the next frame (or the device
    /// is lost).
    fn acquire(&mut self, shows: &mut Shows) -> Option<usize> {
        let (mut result, mut at) = self.next_image(shows);
        if result == OUT_OF_DATE && self.chain(shows) {
            (result, at) = self.next_image(shows);
        }
        shows.remake |= result == SUBOPTIMAL || result == OUT_OF_DATE;
        self.lost |= result < 0 && result != OUT_OF_DATE;
        let had = result == 0 || result == SUBOPTIMAL;
        (had && at < shows.images.len()).then_some(at)
    }

    /// Copies `wanted` places of a window's pixels into image `at` of its swap chain, once the
    /// image is had: from the image the GPU draws the window in where it has one, else from the
    /// buffer. Says whether the copy was given to the queue; if not, the device is lost.
    fn copy(&mut self, shows: &mut Shows, at: usize, wanted: &[[c_int; 4]]) -> bool {
        let (image, width) = (shows.images[at].image, shows.size[0]);
        let regions: Vec<VkBufferImageCopy> = wanted.iter().filter_map(|place| within(place, shows.system.extent)).map(|place| region(place, width)).collect();
        let old = if shows.system.showed[at] == 0 { LAYOUT_UNDEFINED } else { LAYOUT_PRESENT };
        let before = barrier(image, old, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
        let after = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        let (calls, commands) = (&self.calls, shows.commands);
        self.begin(shows);
        // SAFETY: Vulkan's documented call; the commands are being recorded.
        unsafe { (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &before) };
        if !self.copied_from_picture(shows, image, wanted) && !regions.is_empty() {
            // SAFETY: as above; the places lie in the buffer and the image.
            unsafe { (calls.vkCmdCopyBufferToImage)(commands, shows.buffer, image, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr()) };
        }
        // SAFETY: as above.
        let recorded = unsafe {
            (calls.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &after);
            (calls.vkEndCommandBuffer)(commands)
        };
        self.submit(shows, at, recorded)
    }

    /// Records `wanted` places copied into swap chain image `image` from the image the GPU draws
    /// the window in, where it has one (what was put since going into it first). Says whether
    /// it did; if not, they are copied from the buffer.
    #[cfg(feature = "gpu-draw")]
    fn copied_from_picture(&self, shows: &mut Shows, image: Handle, wanted: &[[c_int; 4]]) -> bool {
        let Some(pipeline) = self.pipeline.as_ref().and_then(|made| made.as_deref()) else { return false };
        let Some(from) = shows.system.picture.as_ref().map(|picture| picture.image) else { return false };
        self.fill_picture(shows, false);
        let inside: Vec<[c_int; 4]> = wanted.iter().filter_map(|place| within(place, shows.system.extent)).collect();
        pipeline.copy_places(shows.commands, from, image, &inside);
        true
    }

    /// Without `gpu-draw` the GPU draws no window: copies come from the buffer.
    #[cfg(not(feature = "gpu-draw"))]
    #[inline]
    fn copied_from_picture(&self, _: &mut Shows, _: Handle, _: &[[c_int; 4]]) -> bool {
        false
    }

    /// Records the places put since copied into the image the GPU draws the window in (all of
    /// it, if `fresh`: just made), which is then ready to be copied from.
    #[cfg(feature = "gpu-draw")]
    fn fill_picture(&self, shows: &mut Shows, fresh: bool) {
        let Some(picture) = shows.system.picture.as_mut() else { return };
        let (image, pending) = (picture.image, std::mem::take(&mut picture.pending));
        self.fill(shows, image, pending, fresh);
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
            note(&mut shows.system.put, place);
        }
        drawn
    }

    /// Records and runs the paint drawn by `pipeline`, as `draw` says, and waits for it. A
    /// failure loses the device: the window's image and buffer no longer agree.
    #[cfg(feature = "gpu-draw")]
    fn draw_with(&mut self, shows: &mut Shows, pipeline: &mut crate::gpu_draw_vulkan::Pipeline, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        let fresh = shows.system.picture.is_none();
        if fresh {
            let Some((image, memory)) = pipeline.picture(shows.size) else { return false };
            shows.system.picture = Some(Box::new(Picture { image, memory, pending: Vec::new() }));
        }
        let picture = shows.system.picture.as_ref().map_or(0, |picture| picture.image);
        self.begin(shows);
        self.fill_picture(shows, fresh);
        let was = crate::gpu_draw_vulkan::Was { layout: LAYOUT_TRANSFER_SOURCE, access: ACCESS_TRANSFER_READ, stage: STAGE_TRANSFER };
        let recorded = pipeline.record(shows.commands, picture, shows.size, list, place, was, LAYOUT_TRANSFER_SOURCE);
        let done = self.run(shows, recorded);
        pipeline.forget_target();
        done
    }

    /// Presents image `at` of a window's swap chain once copied into, naming the places put.
    fn present(&mut self, shows: &mut Shows, at: usize) {
        let rectangles: Vec<VkRectLayerKHR> = shows
            .system
            .put
            .iter()
            .filter_map(|place| within(place, shows.system.extent))
            .map(|[left, top, right, bottom]| VkRectLayerKHR { offset: [left, top], extent: [(right - left) as u32, (bottom - top) as u32], layer: 0 })
            .collect();
        let result = self.queue_present(shows, at, Some(&rectangles));
        shows.remake |= result == SUBOPTIMAL || result == OUT_OF_DATE;
        self.lost |= result < 0 && result != OUT_OF_DATE;
        let frames = &mut shows.system;
        frames.frame += 1;
        frames.showed[at] = frames.frame;
        frames.past.insert(0, std::mem::take(&mut frames.put));
        frames.past.truncate(KEPT);
    }

    /// Lets go of a window: everything made for it, its surface last.
    pub(crate) fn forget(&mut self, mut shows: Shows) {
        self.idle();
        self.unpicture(&mut shows);
        self.let_go(shows);
    }

    /// Tests: the places the last frame named.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        shows.system.past.first().map_or(&[], Vec::as_slice)
    }
}

/// The part of `place` (left, top, right, bottom) within an image `extent` big: nothing where
/// none of it is.
fn within(place: &[c_int; 4], extent: [u32; 2]) -> Option<[c_int; 4]> {
    let [left, top] = [place[0].max(0), place[1].max(0)];
    let [right, bottom] = [place[2].min(extent[0] as c_int), place[3].min(extent[1] as c_int)];
    (left < right && top < bottom).then_some([left, top, right, bottom])
}
