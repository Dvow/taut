//! Android windows shown through Vulkan, with the feature `gpu`. The CPU draws every pixel as
//! without it; each place it drew goes into the window's pixels, a buffer the CPU writes and the
//! GPU reads, and from there into the window's picture, an image on the GPU (with `gpu-draw` the
//! GPU draws large parts into the picture itself). A frame blits the whole picture into the swap
//! chain's next image, inside the window's insets, the theme's background around it, and presents
//! it naming the places that changed (`VK_KHR_incremental_present`), in step with the screen
//! (FIFO). The blit only reorders the channels where the swap chain's images are BGRA: the pixels
//! shown are the CPU's, byte for byte.
//!
//! - The surface is the activity's window (`VK_KHR_android_surface`). It goes with the window
//!   (the activity paused, or gone); the device stays, and the next window gets a new surface.
//! - Swap chains take the window's own RGBA where they can, so the CPU finds the window as it
//!   left it if the GPU is given up. They are made with no transform of their own: on a turned
//!   screen the compositor turns the frames (and says `VK_SUBOPTIMAL_KHR`, which is no reason to
//!   make them again).
//! - The loader (`libvulkan.so`) is opened when the window first asks for the GPU. What X11's
//!   path shares is `gpu_vulkan`'s.
//!
//! `Auto` takes a device only where it is not the CPU at work (SwiftShader).
use crate::gpu_android::Block;
use crate::gpu_vulkan::{Chain, Vulkan};
use crate::vk::*;
use std::ffi::c_void;
use std::ptr::null;

functions!(AndroidCalls {
    vkCreateAndroidSurfaceKHR: fn(Object, *const VkAndroidSurfaceCreateInfoKHR, *const c_void, *mut Handle) -> i32;
    vkGetPhysicalDeviceFormatProperties: fn(Object, u32, *mut VkFormatProperties);
});

functions!(PictureCalls {
    vkCreateImage: fn(Object, *const VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkCmdClearColorImage: fn(Object, Handle, u32, *const [f32; 4], u32, *const [u32; 5]);
    vkCmdBlitImage: fn(Object, Handle, u32, Handle, u32, u32, *const VkImageBlit, u32);
});

/// The swap chain's pixels where they are red first, as the window's are (the format every
/// Android device offers).
const R8G8B8A8_UNORM: u32 = 37;
/// Formats a blit reads from, and writes to (`VK_FORMAT_FEATURE_BLIT_SRC_BIT`, `_DST_BIT`).
const BLITS_FROM: u32 = 0x400;
const BLITS_TO: u32 = 0x800;
/// `VK_ERROR_SURFACE_LOST_KHR`: the window went.
const SURFACE_LOST: i32 = -1_000_000_000;
/// How long a frame may wait for an image or for the GPU's drawing before the device is taken
/// for lost: ten seconds, in nanoseconds (the CPU at work draws a whole window in more than one).
const WAITED: u64 = 10_000_000_000;

/// The GPU the window is shown through: one device and queue.
pub(crate) type Gpu = crate::gpu_vulkan::Gpu<Pictures>;

/// The window shown through Vulkan.
pub(crate) type Shows = crate::gpu_vulkan::Shows<Picture>;

/// What Android keeps of the GPU: the functions making surfaces and pictures and blitting them,
/// and the kinds of the GPU's own memory (one bit each).
pub(crate) struct Pictures {
    surfaces: AndroidCalls,
    calls: PictureCalls,
    local: u32,
}

/// A device that can show the window: its queue family (one that draws, so it blits), and
/// whether it is the CPU at work.
struct Chosen {
    physical: Object,
    family: u32,
    cpu: bool,
}

/// What the window keeps besides its pixels: its picture, an image of them on the GPU, which is
/// `fresh` (all of it to be copied in) until first filled; both are `new` until all of the window
/// is drawn into them. The window's size, where the pixels lie in it (left, top), and the colour
/// around them. The places put into the pixels and not yet into the picture, and those changed
/// since the last frame (left, top, right, bottom in the pixels); everything is to be shown
/// (`whole`), not only those.
pub(crate) struct Picture {
    image: Handle,
    memory: Handle,
    fresh: bool,
    new: bool,
    window: [i32; 2],
    at: [i32; 2],
    background: u32,
    pending: Vec<[i32; 4]>,
    changed: Vec<[i32; 4]>,
    whole: bool,
    /// The window went: its surface is to be let go.
    gone: bool,
    /// Tests: the places the last frame named.
    #[cfg(test)]
    named: Vec<[i32; 4]>,
}

impl Default for Picture {
    fn default() -> Picture {
        Picture {
            image: 0,
            memory: 0,
            fresh: true,
            new: true,
            window: [0; 2],
            at: [0; 2],
            background: 0xff00_0000,
            pending: Vec::new(),
            changed: Vec::new(),
            whole: true,
            gone: false,
            #[cfg(test)]
            named: Vec::new(),
        }
    }
}

impl Shows {
    /// How much of the pixels (width, height) shows in the window, from where they lie.
    fn shown(&self) -> [i32; 2] {
        [0, 1].map(|axis| self.size[axis].min(self.system.window[axis] - self.system.at[axis]).max(0))
    }

    /// Whether all of the window is to be drawn (its pixels are new); asked once.
    pub(crate) fn take_whole(&mut self) -> bool {
        std::mem::take(&mut self.system.new)
    }

    /// Whether the window went: its surface is to be let go.
    pub(crate) fn gone(&self) -> bool {
        self.system.gone
    }
}

/// A surface of `vulkan` on `window` (an `ANativeWindow`); nothing where none is made.
fn surface(vulkan: &Vulkan, surfaces: &AndroidCalls, window: *mut c_void) -> Option<Handle> {
    let (asked, mut surface) = (VkAndroidSurfaceCreateInfoKHR { kind: 1_000_008_000, window, ..Default::default() }, 0);
    // SAFETY: the window is alive (the caller's); Vulkan fills in one handle.
    let made = unsafe { (surfaces.vkCreateAndroidSurfaceKHR)(vulkan.instance, &asked, null(), &mut surface) };
    (made == 0).then_some(surface)
}

/// Whether device `physical` blits images of `format` laid out for the GPU in the way `wanted`
/// says (`BLITS_FROM`, `BLITS_TO`).
fn blits(surfaces: &AndroidCalls, physical: Object, format: u32, wanted: u32) -> bool {
    let mut properties = VkFormatProperties::default();
    // SAFETY: Vulkan fills in the format's properties.
    unsafe { (surfaces.vkGetPhysicalDeviceFormatProperties)(physical, format, &mut properties) };
    properties.optimal & wanted != 0
}

/// The device and queue family best showing `window` (an `ANativeWindow`, alive), asked through
/// a surface made on it for the question, and whether it is the CPU at work: a GPU before the
/// CPU; unless `any`, only a GPU.
fn choose(vulkan: &Vulkan, surfaces: &AndroidCalls, window: *mut c_void, any: bool) -> Option<Chosen> {
    let surface = surface(vulkan, surfaces, window)?;
    let usable = vulkan.devices().into_iter().filter_map(|physical| chosen(vulkan, surfaces, physical, surface));
    let chosen = usable.filter(|chosen| any || !chosen.cpu).min_by_key(|chosen| chosen.cpu);
    vulkan.destroy(surface);
    chosen
}

/// What device `physical` is, where it can show `surface`: a queue family that draws and
/// presents to it, swap chains, and blits from the window's picture.
fn chosen(vulkan: &Vulkan, surfaces: &AndroidCalls, physical: Object, surface: Handle) -> Option<Chosen> {
    let families = vulkan.families(physical);
    let presents = |at: usize| {
        let mut supported = 0;
        // SAFETY: Vulkan's documented question, of the instance's device and the surface.
        unsafe { (vulkan.calls.vkGetPhysicalDeviceSurfaceSupportKHR)(physical, at as u32, surface, &mut supported) };
        supported != 0
    };
    // (Graphics: 0x1.)
    let family = (0..families.len()).find(|at| families[*at].count > 0 && families[*at].flags & 0x1 != 0 && presents(*at)).filter(|_| vulkan.offers(physical, c"VK_KHR_swapchain"))? as u32;
    let cpu = vulkan.properties(physical).device_type == 4;
    blits(surfaces, physical, B8G8R8A8_UNORM, BLITS_FROM).then_some(Chosen { physical, family, cpu })
}

impl Gpu {
    /// Opens Vulkan and shows `window` (an `ANativeWindow`) through it, sized by `fit`. Nothing
    /// where Vulkan, a device presenting to it or anything else is missing, or where the only
    /// device is the CPU at work and `any` is false.
    ///
    /// # Safety
    /// `window` is an `ANativeWindow`, alive until the window is let go of (`forget`).
    pub(crate) unsafe fn open(window: *mut c_void, any: bool) -> Option<(Gpu, Shows)> {
        let (vulkan, _) = Vulkan::open(&[c"VK_KHR_surface", c"VK_KHR_android_surface"])?;
        let surfaces = AndroidCalls::load(&mut |name| vulkan.function(name)).ok()?;
        let chosen = choose(&vulkan, &surfaces, window, any)?;
        let mut memory = VkPhysicalDeviceMemoryProperties::default();
        // SAFETY: Vulkan fills in the device's memory.
        unsafe { (vulkan.calls.vkGetPhysicalDeviceMemoryProperties)(chosen.physical, &mut memory) };
        let local = kinds(&memory, DEVICE_LOCAL);
        let pictures = move |find: &mut dyn FnMut(&std::ffi::CStr) -> *const c_void| Some(Pictures { surfaces, calls: PictureCalls::load(find).ok()?, local });
        let mut gpu = Gpu::make(vulkan, (chosen.physical, chosen.family), WAITED, pictures)?;
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !chosen.cpu, crate::platform::setting("TAUT_DRAW").as_deref());
        }
        // SAFETY: as the caller promises.
        let shows = unsafe { gpu.window(window) }?;
        Some((gpu, shows))
    }

    /// Shows `window` (an `ANativeWindow`) through the device: a surface on it, the window then
    /// sized by `fit`. Nothing where it can't be made.
    ///
    /// # Safety
    /// As `open`.
    pub(crate) unsafe fn window(&mut self, window: *mut c_void) -> Option<Shows> {
        let surface = surface(&self.vulkan, &self.system.surfaces, window)?;
        let mut supported = 0;
        // SAFETY: the surface was just made; the family is the device's.
        unsafe { (self.vulkan.calls.vkGetPhysicalDeviceSurfaceSupportKHR)(self.physical, self.family, surface, &mut supported) };
        if supported == 0 {
            self.vulkan.destroy(surface);
            return None;
        }
        self.shows(surface, Picture::default())
    }

    /// Gives a window its size (`window` pixels), and its pixels theirs (`size`, at `at` in the
    /// window): a new buffer and picture, all of which is to be drawn, where `size` changed; a
    /// new swap chain where `window` did. Says whether it has them: where the device makes none
    /// that big, the window is better shown by the CPU.
    pub(crate) fn fit(&mut self, shows: &mut Shows, window: [i32; 2], size: [i32; 2], at: [i32; 2]) -> bool {
        if (shows.system.window, shows.size, shows.system.at) == (window, size, at) && shows.swapchain != 0 {
            return true;
        }
        self.idle();
        (shows.system.at, shows.system.whole) = (at, true);
        if (shows.size != size || shows.buffer == 0) && !self.pixels(shows, size) {
            return false;
        }
        if shows.system.window == window && shows.swapchain != 0 {
            return true;
        }
        shows.system.window = window;
        self.chain(shows)
    }

    /// Makes a window's pixels and picture (again), `size` big. Says whether it could.
    fn pixels(&mut self, shows: &mut Shows, size: [i32; 2]) -> bool {
        self.unpicture(shows);
        self.buffer(shows, size) && self.picture(shows)
    }

    /// Makes a window's picture: an image its pixels' size in the GPU's own memory, copied into
    /// and blitted from (with `gpu-draw`, drawn into too). Says whether it could.
    fn picture(&mut self, shows: &mut Shows) -> bool {
        #[cfg(feature = "gpu-draw")]
        let usage = USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION | crate::vk::USAGE_DRAWN_INTO;
        #[cfg(not(feature = "gpu-draw"))]
        let usage = USAGE_TRANSFER_SOURCE | USAGE_TRANSFER_DESTINATION;
        let (calls, extent) = (&self.system.calls, [shows.size[0] as u32, shows.size[1] as u32, 1]);
        let asked = VkImageCreateInfo { kind: 14, image_type: 1, format: B8G8R8A8_UNORM, extent, mip_levels: 1, array_layers: 1, samples: 1, usage, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (calls.vkCreateImage)(self.device, &asked, null(), &mut shows.system.image) != 0 {
                return false;
            }
            (calls.vkGetImageMemoryRequirements)(self.device, shows.system.image, &mut needs);
        }
        let Some(memory) = self.memory(needs, self.system.local) else { return false };
        let picture = &mut shows.system;
        (picture.memory, picture.fresh, picture.new) = (memory, true, true);
        // SAFETY: as above; the memory is bound whole.
        unsafe { (calls.vkBindImageMemory)(self.device, picture.image, memory, 0) == 0 }
    }

    /// Lets go of a window's picture, and of what was put into its pixels. (The queue is idle.)
    fn unpicture(&mut self, shows: &mut Shows) {
        #[cfg(feature = "gpu-draw")]
        if let Some(pipeline) = self.pipeline.as_mut().and_then(|made| made.as_deref_mut()) {
            pipeline.forget_target();
        }
        let picture = &mut shows.system;
        // SAFETY: nothing on the device uses them (the queue is idle); null handles are let be.
        unsafe {
            (self.system.calls.vkDestroyImage)(self.device, picture.image, null());
            (self.calls.vkFreeMemory)(self.device, picture.memory, null());
        }
        (picture.image, picture.memory, picture.pending, picture.changed) = (0, 0, Vec::new(), Vec::new());
    }

    /// Makes a window's swap chain (again) at the window's size: images of 8-bit colour, the
    /// window's own RGBA where the device blits into it, else BGRA; opaque. Says whether it could.
    fn chain(&mut self, shows: &mut Shows) -> bool {
        let can = match self.capabilities(shows) {
            Ok(can) => can,
            Err(result) => {
                shows.system.gone |= result == SURFACE_LOST;
                return false;
            }
        };
        let (surfaces, physical) = (&self.system.surfaces, self.physical);
        let offered = self.formats(shows);
        // (sRGB's colour space is 0; alpha ignored 0x1, or as the window has it 0x8; no
        // transform 0x1.)
        let takes = |format: u32| offered.iter().copied().find(|offered| offered.format == format && offered.colour_space == 0 && blits(surfaces, physical, format, BLITS_TO));
        let format = takes(R8G8B8A8_UNORM).or_else(|| takes(B8G8R8A8_UNORM));
        let alpha = [0x1, 0x8].into_iter().find(|alpha| can.alphas & alpha != 0);
        let transform = if can.transforms & 0x1 != 0 { 0x1 } else { can.transform };
        let extent = shows.system.window.map(|side| side as u32);
        let fits = extent.iter().zip(can.least.iter().zip(&can.most)).all(|(side, (least, most))| (least..=most).contains(&side));
        let (Some(format), Some(alpha)) = (format, alpha) else { return false };
        let refused = can.usage & USAGE_TRANSFER_DESTINATION == 0 || !fits;
        let result = self.make_chain(shows, Chain { format, extent, transform, alpha, images: can.min_images.max(2) }, refused);
        shows.system.gone |= result == SURFACE_LOST;
        shows.system.whole |= result == 0;
        result == 0
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
        shows.system.pending.push(place);
        shows.system.changed.push(place);
    }

    /// Shows the window: what was put since goes into the picture, which is blitted whole into
    /// the swap chain's next image, inside the background (`background`, `0xAARRGGBB`), and
    /// presented naming what changed. With nothing changed, only where `again` (the system asks
    /// for the window whole). A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows, background: u32, again: bool) {
        let fresh_background = std::mem::replace(&mut shows.system.background, background) != background;
        shows.system.whole |= again || fresh_background;
        if shows.system.changed.is_empty() && !shows.system.whole || !self.wait(shows) {
            return;
        }
        #[cfg(test)]
        if self.faked {
            self.lost = true;
            return;
        }
        if shows.remake && !self.chain(shows) {
            self.lost |= !shows.system.gone;
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
        let (mut result, mut at) = self.next_image(shows);
        if result == OUT_OF_DATE && self.chain(shows) {
            (result, at) = self.next_image(shows);
        }
        self.said(shows, result);
        let had = result == 0 || result == SUBOPTIMAL;
        (had && at < shows.images.len()).then_some(at)
    }

    /// Takes in what a call on the swap chain said: a chain out of date is made again, a window
    /// gone or a device failing is said.
    fn said(&mut self, shows: &mut Shows, result: i32) {
        shows.remake |= result == OUT_OF_DATE;
        shows.system.gone |= result == SURFACE_LOST;
        self.lost |= result < 0 && result != OUT_OF_DATE && result != SURFACE_LOST;
    }

    /// Records and submits a frame into image `at`: the places put go into the picture (all of it
    /// while new), the background is cleared in where the picture leaves room, and the picture
    /// blitted in. Says whether it was given to the queue; if not, the device is lost.
    fn record(&mut self, shows: &mut Shows, at: usize) -> bool {
        let (calls, commands, image) = (&self.system.calls, shows.commands, shows.images[at].image);
        self.begin(shows);
        self.fill_picture(shows);
        let to_blit = barrier(image, LAYOUT_UNDEFINED, LAYOUT_TRANSFER_DESTINATION, 0, ACCESS_TRANSFER_WRITE);
        // (The clear and the blit both write the image: the blit waits for the clear.)
        let cleared = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_TRANSFER_DESTINATION, ACCESS_TRANSFER_WRITE, ACCESS_TRANSFER_WRITE);
        let to_show = barrier(image, LAYOUT_TRANSFER_DESTINATION, LAYOUT_PRESENT, ACCESS_TRANSFER_WRITE, 0);
        let ([left, top], shown, picture) = (shows.system.at, shows.shown(), &shows.system);
        let room = left > 0 || top > 0 || [left + shown[0], top + shown[1]] != picture.window;
        let colour = [16, 8, 0, 24].map(|shift| ((picture.background >> shift) & 0xff) as f32 / 255.0);
        let blit = VkImageBlit {
            source_layers: LAYER,
            source_offsets: [[0, 0, 0], [shown[0], shown[1], 1]],
            destination_layers: LAYER,
            destination_offsets: [[left, top, 0], [left + shown[0], top + shown[1], 1]],
        };
        let on = &self.calls;
        // SAFETY: Vulkan's documented calls; the commands are being recorded, the picture is laid
        // out to be copied from (see `fill_picture`), the image to be written; the blit lies
        // inside both (`shown`).
        let recorded = unsafe {
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &to_blit);
            if room {
                (calls.vkCmdClearColorImage)(commands, image, LAYOUT_TRANSFER_DESTINATION, &colour, 1, &WHOLE);
                (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &cleared);
            }
            if shown.iter().all(|side| *side > 0) {
                (calls.vkCmdBlitImage)(commands, picture.image, LAYOUT_TRANSFER_SOURCE, image, LAYOUT_TRANSFER_DESTINATION, 1, &blit, 0);
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &to_show);
            (on.vkEndCommandBuffer)(commands)
        };
        self.submit(shows, at, recorded)
    }

    /// Records the places put since copied from the window's pixels into its picture (all of
    /// it, if new), which is then laid out to be copied from.
    fn fill_picture(&self, shows: &mut Shows) {
        let picture = &mut shows.system;
        let (image, pending, fresh) = (picture.image, std::mem::take(&mut picture.pending), std::mem::replace(&mut picture.fresh, false));
        self.fill(shows, image, pending, fresh);
    }

    /// Presents image `at` of a window's swap chain once blitted into, naming the places that
    /// changed (all of it after a new swap chain, picture or background).
    fn present(&mut self, shows: &mut Shows, at: usize) {
        let [left, top] = shows.system.at;
        let changed = std::mem::take(&mut shows.system.changed);
        let named = !std::mem::take(&mut shows.system.whole);
        let rectangles: Vec<VkRectLayerKHR> = changed.iter().map(|[l, t, r, b]| VkRectLayerKHR { offset: [l + left, t + top], extent: [(r - l) as u32, (b - t) as u32], layer: 0 }).collect();
        let result = self.queue_present(shows, at, named.then_some(rectangles.as_slice()));
        self.said(shows, result);
        #[cfg(test)]
        {
            let window = shows.system.window;
            shows.system.named = if named && self.regions { changed } else { vec![[0, 0, window[0] - left, window[1] - top]] };
        }
    }

    /// Lets go of a window: everything made for it, its surface last. (Before its
    /// `ANativeWindow` goes.)
    pub(crate) fn forget(&mut self, mut shows: Shows) {
        self.idle();
        self.unpicture(&mut shows);
        self.let_go(shows);
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
        let drawn = made.as_mut().is_some_and(|pipeline| pipeline.ready_frames(list) && self.draw_with(shows, pipeline, list, place));
        self.pipeline = Some(made);
        if drawn {
            shows.system.changed.push(place);
        }
        drawn
    }

    /// Records and runs the paint drawn by `pipeline`, as `draw` says, and waits for it. A
    /// failure loses the device: the window's picture and pixels no longer agree.
    #[cfg(feature = "gpu-draw")]
    fn draw_with(&mut self, shows: &mut Shows, pipeline: &mut crate::gpu_draw_vulkan::Pipeline, list: &crate::gpu_draw::DrawList, place: [i32; 4]) -> bool {
        self.begin(shows);
        self.fill_picture(shows);
        let was = crate::gpu_draw_vulkan::Was { layout: LAYOUT_TRANSFER_SOURCE, access: ACCESS_TRANSFER_READ, stage: STAGE_TRANSFER };
        let recorded = pipeline.record(shows.commands, shows.system.image, shows.size, list, place, was, LAYOUT_TRANSFER_SOURCE);
        self.run(shows, recorded)
    }

    /// Tests: the places the last frame named, in the pixels.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[i32; 4]] {
        &shows.system.named
    }
}
