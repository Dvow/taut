//! macOS and iOS windows shown through Metal, with the feature `gpu`. Each window's view gets a
//! `CAMetalLayer` over its own layer, and a texture of the window's size, private to the GPU.
//! The CPU draws as without the feature (the window's `Surface`); only the places a run changed
//! go up, their rows into one buffer, then a blit each. A frame copies the texture into the
//! layer's next drawable and presents it, in step with the screen: the pixels shown are the
//! CPU's, byte for byte. With `gpu-draw` the GPU draws large parts itself (`gpu_draw_metal`).
//!
//! - Metal is opened by name on a thread of its own when a window first asks (with `gpu-draw`
//!   its shaders compile there too); the CPU shows the windows until then.
//! - A frame the device failed (removed, reset) loses it: the windows are drawn whole through a
//!   new one, or by the CPU where none comes.
//! - macOS: a window on a screen another GPU drives (its layer's `preferredDevice`) moves every
//!   window to that GPU, drawn whole.
//! - iOS: nothing goes to the GPU while the app is in the background (`gpu_ios`).
//! - While a window's edge is dragged, its frames go with Core Animation's transaction
//!   (`presentsWithTransaction`), so the picture keeps up with the frame.
//! - The layer holds 8-bit BGRA in sRGB, as the CPU's pictures do; with the feature `hdr`,
//!   16-bit floats in EDR while the window is lit on a screen with headroom (`hdr_metal`).
use crate::geom::Rect;
use crate::macos_sys::{Bool, CGColorSpaceCreateWithName, CGColorSpaceRelease, NSRect, NSSize, Obj, Pool, class, kCAGravityTopLeft, kCGColorSpaceSRGB, made, rust_string, send};
use crate::platform::Renderer;
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::{c_char, c_int, c_void};

/// `MTLPixelFormatBGRA8Unorm`: `0xAARRGGBB` words in memory.
pub(crate) const BGRA8: usize = 80;
/// Texture usage: drawn into.
pub(crate) const RENDER_TARGET: usize = 4;
/// `MTLStorageModePrivate`: on the GPU alone.
pub(crate) const PRIVATE: usize = 2;
/// The widest and tallest texture every Mac's Metal makes (asking for more aborts the program).
const MOST: usize = 16_384;
/// `MTLCommandBufferStatusError`.
const FAILED: usize = 5;

/// `MTLOrigin` and `MTLSize`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Three(pub(crate) [usize; 3]);

/// The blit that copies rows from a buffer into a texture.
pub(crate) const COPY_IN: &std::ffi::CStr = c"copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:";

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// What windows asking for `asked` ask of Metal, as `TAUT_RENDERER` says if set: nothing for
/// the CPU; else whether any device will do. (Every Metal device is a GPU: `Auto` takes one
/// too, and with `gpu-draw` has it draw large parts.)
pub(crate) fn asked(asked: Renderer) -> Option<bool> {
    crate::platform::wants_gpu(asked)
}

/// The system's own Metal device (+1), Metal opened by name; none where it has none.
fn system_device() -> Obj {
    // SAFETY: the framework is opened (2: `RTLD_NOW`) and never closed; its function is declared
    // as Metal's header declares it, and makes a device the caller owns.
    unsafe {
        let library = dlopen(c"/System/Library/Frameworks/Metal.framework/Metal".as_ptr(), 2);
        let made = if library.is_null() { std::ptr::null_mut() } else { dlsym(library, c"MTLCreateSystemDefaultDevice".as_ptr()) };
        if made.is_null() {
            return std::ptr::null_mut();
        }
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> Obj>(made)()
    }
}

/// Metal being opened, on a thread of its own.
pub(crate) type Opening = std::thread::JoinHandle<Option<Gpu>>;

/// Opens Metal on a thread of its own: on `prefer`'s device (retained, handed over) or the
/// system's own; drawing as `any` says (see `asked`). The loop is woken when it is ready.
pub(crate) fn open(any: bool, prefer: Obj) -> Opening {
    let prefer = prefer as usize;
    std::thread::spawn(move || {
        let gpu = Gpu::open(any, prefer as Obj);
        crate::Waker.wake();
        gpu
    })
}

/// What a program's windows keep of Metal: the GPU once open, or being opened; what they ask
/// (nothing once none is to be had); the device to open next, and how many were lost.
pub(crate) struct Metal {
    pub(crate) gpu: Option<Gpu>,
    opening: Option<Opening>,
    asks: Option<bool>,
    prefer: Obj,
    losses: u8,
}

/// Devices lost before the CPU shows the windows for good.
const MOST_LOST: u8 = 3;

impl Metal {
    /// What windows asking for `renderer` keep: nothing open yet.
    pub(crate) fn new(renderer: Renderer) -> Metal {
        Metal { gpu: None, opening: None, asks: asked(renderer), prefer: std::ptr::null_mut(), losses: 0 }
    }

    /// Takes Metal in once it is open, or asks for it where the windows want it and none is open
    /// or coming. Where none came, the CPU shows the windows for good.
    pub(crate) fn keep_opening(&mut self) {
        if self.opening.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            self.gpu = self.opening.take().and_then(|opening| opening.join().ok()).flatten();
            self.asks = self.asks.filter(|_| self.gpu.is_some());
        }
        if let (None, None, Some(any)) = (&self.gpu, &self.opening, self.asks) {
            self.opening = Some(open(any, std::mem::replace(&mut self.prefer, std::ptr::null_mut())));
        }
    }

    /// Tests: waits until the device being opened, if any, is open, and takes it in.
    #[cfg(all(test, target_os = "ios"))]
    pub(crate) fn opened(&mut self) {
        if let Some(opening) = self.opening.take() {
            self.gpu = opening.join().ok().flatten();
        }
    }

    /// The device to open next (+1, handed over): another GPU drives a window's screen.
    pub(crate) fn prefer(&mut self, device: Obj) {
        // SAFETY: the one kept before (or nil), released once.
        unsafe { send::<()>(std::mem::replace(&mut self.prefer, device), c"release", ()) };
    }

    /// Lets go of a window's part, where the GPU shows it: the CPU shows it from then on.
    pub(crate) fn unshow(&mut self, way: &mut Way) {
        if let (Some(gpu), Way::Gpu(shows)) = (self.gpu.as_mut(), std::mem::replace(way, Way::Cpu)) {
            gpu.forget(shows);
        }
    }

    /// Lets go of the GPU, once each window's part is: a new one is opened, unless too many
    /// were lost.
    pub(crate) fn let_go(&mut self) {
        let lost = self.gpu.take().is_some_and(|gpu| gpu.lost);
        self.losses += lost as u8;
        self.asks = self.asks.filter(|_| self.losses <= MOST_LOST);
    }
}

impl Drop for Metal {
    fn drop(&mut self) {
        self.prefer(std::ptr::null_mut());
    }
}

/// The GPU the windows are shown through: its device and queue; with `gpu-draw`, what draws.
pub(crate) struct Gpu {
    device: Obj,
    queue: Obj,
    /// A frame failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// Which parts the GPU draws itself.
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    /// The paint being recorded, kept for its room.
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<crate::gpu_draw_metal::Pipeline>,
    /// Something was drawn by the GPU (tests).
    #[cfg(feature = "gpu-draw")]
    drawn: bool,
    /// The pass to EDR, once tried (see `hdr_metal`).
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    lit: Option<Option<crate::hdr_metal::Pass>>,
}

// SAFETY: Metal's devices, queues, textures and pipelines may be used from any thread. A `Gpu`
// is made on the opening thread, then used on the main thread alone.
unsafe impl Send for Gpu {}

/// A window shown through the GPU: its layer, its texture, and the frame being made.
pub(crate) struct Shows {
    layer: Obj,
    texture: Obj,
    size: [usize; 2],
    scale: f32,
    /// Its texture is new: all of the window is to go in.
    whole: bool,
    /// This frame's command buffer (retained); null until something goes in.
    frame: Obj,
    /// The last frame's (retained): one that failed loses the device.
    last: Obj,
    /// This frame's rows to go up, place after place; and the places (left, top, right, bottom).
    rows: Vec<u32>,
    places: Vec<[usize; 4]>,
    /// The places the last frame put up (tests).
    #[cfg(test)]
    named: Vec<[usize; 4]>,
    /// Its HDR (see `hdr_metal`).
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    pub(crate) lit: Box<crate::hdr_metal::Lit>,
}

/// How a window is shown: not yet asked of the GPU, through it, or by the CPU.
pub(crate) enum Way {
    Unasked,
    Gpu(Shows),
    Cpu,
}

impl Shows {
    /// Its pixels' size, and its scale.
    pub(crate) fn size(&self) -> ([usize; 2], f32) {
        (self.size, self.scale)
    }

    /// Whether all of the window is to go in (its texture is new); asked once.
    pub(crate) fn take_whole(&mut self) -> bool {
        std::mem::take(&mut self.whole)
    }

    /// Its texture (tests).
    #[cfg(all(test, any(target_os = "ios", feature = "gpu-draw")))]
    pub(crate) fn texture(&self) -> Obj {
        self.texture
    }

    /// Tests: whether it is presented in EDR.
    #[cfg(all(test, target_os = "macos", feature = "hdr"))]
    pub(crate) fn in_edr(&self) -> bool {
        self.lit.on
    }
}

impl Gpu {
    /// Metal on `prefer` (retained, handed over), or on the system's own device; with
    /// `gpu-draw` its shaders compiled. Nothing where there is no device.
    fn open(any: bool, prefer: Obj) -> Option<Gpu> {
        let _pool = Pool::new();
        #[cfg(test)]
        if tests::NO_DEVICE.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let device = if prefer.is_null() { system_device() } else { prefer };
        // SAFETY: a device answers `newCommandQueue` (+1, kept).
        let queue: Obj = unsafe { send(device, c"newCommandQueue", ()) };
        if queue.is_null() {
            // SAFETY: the device this owns (nil answers nothing).
            unsafe { send::<()>(device, c"release", ()) };
            return None;
        }
        #[cfg(not(feature = "gpu-draw"))]
        let _ = any;
        #[cfg(feature = "gpu-draw")]
        let drawing = crate::gpu_draw::Drawing::chosen(!any, true, std::env::var("TAUT_DRAW").ok().as_deref());
        #[cfg(feature = "gpu-draw")]
        let pipeline = (drawing != crate::gpu_draw::Drawing::Never).then(|| crate::gpu_draw_metal::Pipeline::new(device)).flatten();
        #[cfg(feature = "gpu-draw")]
        let list = Box::<crate::gpu_draw::DrawList>::default();
        #[cfg(all(feature = "gpu-draw", feature = "video"))]
        let mut list = list;
        #[cfg(all(feature = "gpu-draw", feature = "video"))]
        {
            list.takes_frames = pipeline.as_ref().is_some_and(crate::gpu_draw_metal::Pipeline::takes_frames);
        }
        Some(Gpu {
            device,
            queue,
            lost: false,
            #[cfg(feature = "gpu-draw")]
            drawing: if pipeline.is_some() { drawing } else { crate::gpu_draw::Drawing::Never },
            #[cfg(feature = "gpu-draw")]
            list,
            #[cfg(feature = "gpu-draw")]
            pipeline,
            #[cfg(feature = "gpu-draw")]
            drawn: false,
            #[cfg(all(target_os = "macos", feature = "hdr"))]
            lit: None,
        })
    }

    /// Which renderer it is.
    pub(crate) fn renderer(&self) -> Renderer {
        Renderer::Metal
    }

    /// The device's name.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn name(&self) -> String {
        // SAFETY: a device answers its name.
        rust_string(unsafe { send(self.device, c"name", ()) }).unwrap_or_default()
    }

    /// Shows a window through the device: a layer over all of `under` (the view's layer, whose
    /// `bounds` they are, in points), `size` pixels at `scale`. Nothing where it can't.
    pub(crate) fn window(&mut self, under: Obj, bounds: NSRect, size: [usize; 2], scale: f32) -> Option<Shows> {
        let layer = made(c"CAMetalLayer");
        if layer.is_null() {
            return None;
        }
        // SAFETY: a layer of the library's, set up for the device: BGRA in sRGB, opaque, its
        // drawables readable by blits, at the top left; the view's layer keeps it as well.
        unsafe {
            send::<()>(layer, c"setDevice:", (self.device,));
            send::<()>(layer, c"setPixelFormat:", (BGRA8,));
            send::<()>(layer, c"setFramebufferOnly:", (0 as Bool,));
            send::<()>(layer, c"setOpaque:", (1 as Bool,));
            send::<()>(layer, c"setContentsGravity:", (kCAGravityTopLeft,));
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            send::<()>(layer, c"setColorspace:", (space,));
            CGColorSpaceRelease(space);
            send::<()>(under, c"addSublayer:", (layer,));
        }
        let mut shows = Shows {
            layer,
            texture: std::ptr::null_mut(),
            size: [0, 0],
            scale: 0.0,
            whole: true,
            frame: std::ptr::null_mut(),
            last: std::ptr::null_mut(),
            rows: Vec::new(),
            places: Vec::new(),
            #[cfg(test)]
            named: Vec::new(),
            #[cfg(all(target_os = "macos", feature = "hdr"))]
            lit: Default::default(),
        };
        if !self.fit(&mut shows, bounds, size, scale) {
            self.forget(shows);
            return None;
        }
        Some(shows)
    }

    /// Gives a window's layer its place (`bounds`, in points) and scale, and its texture
    /// `size`: a new one, whose every pixel is to go in. Says whether it has it: where the
    /// device makes none that big, the window is better shown by the CPU.
    pub(crate) fn fit(&mut self, shows: &mut Shows, bounds: NSRect, size: [usize; 2], scale: f32) -> bool {
        // SAFETY: the layer is the window's; no animation moves it.
        unsafe {
            send::<()>(class(c"CATransaction"), c"begin", ());
            send::<()>(class(c"CATransaction"), c"setDisableActions:", (1 as Bool,));
            send::<()>(shows.layer, c"setFrame:", (bounds,));
            send::<()>(shows.layer, c"setContentsScale:", (f64::from(scale),));
            send::<()>(shows.layer, c"setDrawableSize:", (NSSize { width: size[0] as f64, height: size[1] as f64 },));
            send::<()>(class(c"CATransaction"), c"commit", ());
        }
        shows.scale = scale;
        if shows.size == size && !shows.texture.is_null() {
            return true;
        }
        // SAFETY: the old texture (+1, or nil) goes; the new one (+1) is kept.
        unsafe {
            send::<()>(shows.texture, c"release", ());
            shows.texture = self.texture(size, RENDER_TARGET);
        }
        (shows.size, shows.whole) = (size, true);
        !shows.texture.is_null() && size[0] * size[1] > 0
    }

    /// A texture `size` big, BGRA, private to the GPU, for `usage` (+1); nil where the device
    /// makes none.
    pub(crate) fn texture(&self, size: [usize; 2], usage: usize) -> Obj {
        if size[0] * size[1] == 0 || size[0].max(size[1]) > MOST {
            return std::ptr::null_mut();
        }
        // SAFETY: a descriptor of a plain 2D texture of one level, handed to the device.
        unsafe {
            let described: Obj = send(class(c"MTLTextureDescriptor"), c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:", (BGRA8, size[0], size[1], 0 as Bool));
            send::<()>(described, c"setUsage:", (usage,));
            send::<()>(described, c"setStorageMode:", (PRIVATE,));
            send(self.device, c"newTextureWithDescriptor:", (described,))
        }
    }

    /// Notes a place of the window's pixels (left, top, right, bottom) to go up with the frame.
    pub(crate) fn put(&mut self, shows: &mut Shows, surface: &Surface, place: [usize; 4]) {
        let [left, right] = [place[0], place[2]].map(|x| x.min(surface.width).min(shows.size[0]));
        let [top, bottom] = [place[1], place[3]].map(|y| y.min(surface.height).min(shows.size[1]));
        if left >= right || top >= bottom {
            return;
        }
        let width = surface.width;
        (top..bottom).for_each(|row| shows.rows.extend_from_slice(&surface.pixels[row * width + left..row * width + right]));
        shows.places.push([left, top, right, bottom]);
    }

    /// The frame's command buffer, made when first needed.
    pub(crate) fn frame(&self, shows: &mut Shows) -> Obj {
        if shows.frame.is_null() {
            // SAFETY: a queue answers `commandBuffer` (autoreleased): retained until committed.
            shows.frame = unsafe { send(send::<Obj>(self.queue, c"commandBuffer", ()), c"retain", ()) };
        }
        shows.frame
    }

    /// Encodes what was put since: the rows into one buffer, then a blit into the texture
    /// for each place.
    fn flush(&mut self, shows: &mut Shows) {
        if shows.places.is_empty() {
            return;
        }
        let commands = self.frame(shows);
        // SAFETY: a shared buffer holding the rows (+1, released once encoded: the command buffer
        // keeps it); each blit reads one place's rows where they start and writes them where the
        // place lies in the texture, inside it (see `put`).
        unsafe {
            let staged: Obj = send(self.device, c"newBufferWithBytes:length:options:", (shows.rows.as_ptr().cast::<c_void>(), shows.rows.len() * 4, 0usize));
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            let mut from = 0;
            for [left, top, right, bottom] in &shows.places {
                let (wide, high) = (right - left, bottom - top);
                send::<()>(blit, COPY_IN, (staged, from * 4, wide * 4, wide * high * 4, Three([wide, high, 1]), shows.texture, 0usize, 0usize, Three([*left, *top, 0])));
                from += wide * high;
            }
            send::<()>(blit, c"endEncoding", ());
            send::<()>(staged, c"release", ());
        }
        shows.rows.clear();
        #[cfg(test)]
        shows.named.clone_from(&shows.places);
        shows.places.clear();
    }

    /// Ends the frame: what was put goes up, and the texture is copied into the layer's next
    /// drawable and presented. `live`: with Core Animation's transaction, as while the window's
    /// edge is dragged. A frame before that failed loses the device.
    pub(crate) fn show(&mut self, shows: &mut Shows, live: bool) {
        // SAFETY: the last frame answers its status, which is read without waiting.
        if !shows.last.is_null() && unsafe { send::<usize>(shows.last, c"status", ()) } == FAILED {
            self.lost = true;
        }
        self.flush(shows);
        let commands = self.frame(shows);
        // SAFETY: the layer's drawable (autoreleased, or nil when none came in time) is as big as
        // the texture, and copied into whole; presented by the command buffer, or after it is
        // scheduled while live. The frame's buffer is kept as the last one (released when the
        // next comes).
        unsafe {
            send::<()>(shows.layer, c"setPresentsWithTransaction:", (Bool::from(live),));
            let drawable: Obj = send(shows.layer, c"nextDrawable", ());
            let by_commands = !drawable.is_null() && !live;
            if !drawable.is_null() {
                self.copy_or_light(commands, shows, send(drawable, c"texture", ()));
            }
            if by_commands {
                send::<()>(commands, c"presentDrawable:", (drawable,));
            }
            send::<()>(commands, c"commit", ());
            if live && !drawable.is_null() {
                send::<()>(commands, c"waitUntilScheduled", ());
                send::<()>(drawable, c"present", ());
            }
            send::<()>(shows.last, c"release", ());
        }
        (shows.last, shows.frame) = (commands, std::ptr::null_mut());
    }

    /// Encodes the window's texture into `target` (its drawable's): drawn lit while in EDR, else
    /// copied.
    fn copy_or_light(&self, commands: Obj, shows: &mut Shows, target: Obj) {
        #[cfg(all(target_os = "macos", feature = "hdr"))]
        if let (true, Some(Some(pass))) = (shows.lit.on, &self.lit) {
            let gains = shows.lit.put_up(self.device, commands);
            return pass.draw(commands, shows.texture, gains, target, shows.lit.constants());
        }
        self.copy_out(commands, shows.texture, target);
    }

    /// Takes in what lights window `shows` on `screen` after a run of `ui`: its gains drawn
    /// again where they changed; its layer put in EDR while lit on a screen with headroom, and
    /// back. Says whether the headroom changed (the app is to run again) and whether the window
    /// is to be shown again.
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    pub(crate) fn light(&mut self, shows: &mut Shows, screen: crate::hdr::Screen, ui: &mut crate::Ui) -> (bool, bool) {
        let news = ui.set_screen(screen);
        let glows = if screen.hdr { ui.glows() } else { Vec::new() };
        let changed = shows.lit.gains.take(glows, shows.size.map(|side| side as i32), &ui.light.maps).is_some();
        if screen.hdr && shows.lit.gains.any() && self.lit.is_none() {
            self.lit = Some(crate::hdr_metal::Pass::new(self.device));
        }
        let on = screen.hdr && shows.lit.gains.any() && matches!(self.lit, Some(Some(_)));
        let shown_again = changed || on != shows.lit.on || (on && screen != shows.lit.screen);
        if on != shows.lit.on {
            shows.lit.switch(shows.layer, on);
        }
        shows.lit.screen = screen;
        (news, shown_again)
    }

    /// Encodes a copy of all of `from` into `to`, as far as both reach.
    fn copy_out(&self, commands: Obj, from: Obj, to: Obj) {
        // SAFETY: two textures of the device, of one format; the copy lies inside both.
        unsafe {
            let size = |of: Obj| [send::<usize>(of, c"width", ()), send::<usize>(of, c"height", ())];
            let ([wide, high], [far, deep]) = (size(from), size(to));
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            let copy = c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:";
            let (slice, level, origin) = (0usize, 0usize, Three([0, 0, 0]));
            send::<()>(blit, copy, (from, slice, level, origin, Three([wide.min(far), high.min(deep), 1]), to, slice, level, origin));
            send::<()>(blit, c"endEncoding", ());
        }
    }

    /// The device that drives the window's screen, where that is another (+1): every window is
    /// to move there.
    #[cfg(target_os = "macos")]
    pub(crate) fn moved(&self, shows: &Shows) -> Option<Obj> {
        // SAFETY: a layer answers the device it would have (+0).
        let preferred: Obj = unsafe { send(shows.layer, c"preferredDevice", ()) };
        // SAFETY: retained for whoever opens it.
        (!preferred.is_null() && preferred != self.device).then(|| unsafe { send(preferred, c"retain", ()) })
    }

    /// Waits until the window's last frame is on the GPU: nothing is left to send to it.
    #[cfg(target_os = "ios")]
    pub(crate) fn settle(&self, shows: &Shows) {
        // SAFETY: the last frame (or nil, which answers nothing) was committed.
        unsafe { send::<()>(shows.last, c"waitUntilScheduled", ()) };
    }

    /// Draws one place a run changed (`place`, in units) into a window shown `size` pixels at
    /// `scale`: by the GPU where it draws a part that big (`gpu-draw`), else by the CPU into the
    /// window's pixels, which then go up.
    pub(crate) fn draw_place(&mut self, shows: &mut Shows, surface: &mut Surface, ui: &mut Ui, place: Rect, (size, scale): ([usize; 2], f32)) {
        let snapped = place.snap_to_pixels(scale);
        let edge = |at: f32, most: usize| ((at * scale).round().max(0.0) as usize).min(most);
        let pixels = [edge(snapped.x, size[0]), edge(snapped.y, size[1]), edge(snapped.right(), size[0]), edge(snapped.bottom(), size[1])];
        if pixels[0] >= pixels[2] || pixels[1] >= pixels[3] {
            return;
        }
        #[cfg(feature = "gpu-draw")]
        if self.drawn_by_gpu(shows, ui, snapped, pixels.map(|side| side as i32), size) {
            return;
        }
        surface.redraw(ui, place);
        self.put(shows, surface, pixels);
    }

    /// Draws `area` of a window `size` pixels big (`place` in pixels: left, top, right, bottom)
    /// on the GPU, where it draws a part that big itself (see `gpu_draw`). Says whether it did;
    /// if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    fn drawn_by_gpu(&mut self, shows: &mut Shows, ui: &mut Ui, area: Rect, place: [i32; 4], size: [usize; 2]) -> bool {
        let Some(list) = self.drawing(place) else { return false };
        let mut list = std::mem::take(list);
        list.record(ui, area, size.map(|side| side as i32));
        self.draw(shows, list, place)
    }

    /// Lets go of a window: its layer leaves its view.
    pub(crate) fn forget(&mut self, shows: Shows) {
        // SAFETY: what the window held, released once (nil answers nothing); a frame never
        // committed is let go as it is.
        unsafe {
            send::<()>(shows.layer, c"removeFromSuperlayer", ());
            [shows.frame, shows.last, shows.texture, shows.layer].into_iter().for_each(|held| send::<()>(held, c"release", ()));
        }
    }

    /// What the GPU records a part of a window `area` pixels big into (left, top, right,
    /// bottom), where it draws such a part itself (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn drawing(&mut self, area: [i32; 4]) -> Option<&mut crate::gpu_draw::DrawList> {
        (self.pipeline.is_some() && self.drawing.takes(area)).then_some(&mut *self.list)
    }

    /// Draws a recorded paint into a window's texture, inside `place`, after what was put
    /// before; and keeps `list` for the next one. Says whether it drew: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, mut list: crate::gpu_draw::DrawList, place: [i32; 4]) -> bool {
        self.flush(shows);
        let (commands, device) = (self.frame(shows), self.device);
        let target = (shows.texture, shows.size);
        let drawn = !list.failed && self.pipeline.as_mut().is_some_and(|pipeline| pipeline.draw(device, commands, target, &list, place));
        list.finish(drawn);
        *self.list = list;
        self.drawn |= drawn;
        drawn
    }

    /// Whether the GPU has drawn anything itself (tests).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        self.drawn
    }

    /// Tests: the device is lost, as when it is removed.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        self.lost = true;
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        #[cfg(feature = "gpu-draw")]
        drop(self.pipeline.take());
        #[cfg(all(target_os = "macos", feature = "hdr"))]
        drop(self.lit.take());
        // SAFETY: what this made, released once.
        unsafe { [self.queue, self.device].into_iter().for_each(|held| send::<()>(held, c"release", ())) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{LocalClipboard, geom::Size, text::BuiltInFont};
    use std::sync::atomic::AtomicBool;

    /// Tests: no device is to be had.
    pub(crate) static NO_DEVICE: AtomicBool = AtomicBool::new(false);

    /// One test on the GPU at a time: one says there is no device for a while.
    pub(crate) fn one() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opened on this thread, as the opening thread opens it.
    pub(crate) fn gpu() -> Option<Gpu> {
        Gpu::open(false, std::ptr::null_mut())
    }

    /// A window that is a plain layer, `size` pixels at scale 1.
    pub(crate) fn shown(gpu: &mut Gpu, size: [usize; 2]) -> (Obj, Shows) {
        let under = made(c"CALayer");
        let bounds = NSRect::new(0.0, 0.0, size[0] as f64, size[1] as f64);
        let shows = gpu.window(under, bounds, size, 1.0).expect("the layer is made");
        (under, shows)
    }

    /// Waits for the last frame, and reads `texture` back (what the GPU shows) through a buffer.
    pub(crate) fn read(gpu: &Gpu, shows: &mut Shows, texture: Obj) -> Vec<u32> {
        let [width, height] = shows.size;
        let mut pixels = vec![0u32; width * height];
        unsafe {
            send::<()>(shows.last, c"waitUntilCompleted", ());
            let buffer: Obj = send(gpu.device, c"newBufferWithLength:options:", (width * height * 4, 0usize));
            let commands: Obj = send(gpu.queue, c"commandBuffer", ());
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            let copy = c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:";
            send::<()>(blit, copy, (texture, 0usize, 0usize, Three([0, 0, 0]), Three([width, height, 1]), buffer, 0usize, width * 4, width * height * 4));
            send::<()>(blit, c"endEncoding", ());
            send::<()>(commands, c"commit", ());
            send::<()>(commands, c"waitUntilCompleted", ());
            assert_ne!(send::<usize>(commands, c"status", ()), FAILED, "the read back failed");
            let there: *const u32 = send(buffer, c"contents", ());
            std::ptr::copy_nonoverlapping(there, pixels.as_mut_ptr(), pixels.len());
            send::<()>(buffer, c"release", ());
        }
        pixels
    }

    /// Fills all of the window's texture with one pixel, by hand, and waits.
    fn fill(gpu: &mut Gpu, shows: &mut Shows, pixel: u32) {
        let [width, height] = shows.size;
        let surface = Surface { width, height, pixels: vec![pixel; width * height] };
        gpu.put(shows, &surface, [0, 0, width, height]);
        gpu.flush(shows);
        let commands = std::mem::replace(&mut shows.frame, std::ptr::null_mut());
        unsafe {
            send::<()>(commands, c"commit", ());
            send::<()>(commands, c"waitUntilCompleted", ());
            send::<()>(commands, c"release", ());
        }
    }

    /// What the window's texture holds, once the last frame is done.
    fn held(gpu: &mut Gpu, shows: &mut Shows) -> Vec<u32> {
        let texture = shows.texture;
        read(gpu, shows, texture)
    }

    /// An interface with a count and a button, `size` units at `scale`.
    fn counter(size: Size, scale: f32) -> (Ui, impl FnMut(&mut Ui)) {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(size, scale);
        let mut count = 0;
        let describe = move |ui: &mut Ui| {
            ui.column().pad(10.0).gap(8.0).open();
            ui.label(&format!("Pressed {count} times"));
            count += ui.button("Press").clicked as u32;
            ui.el().size(120.0, 40.0).fill(crate::Color::rgb(0x3060c0)).radius(8.0).show();
            ui.close();
        };
        (ui, describe)
    }

    #[test]
    fn the_gpu_shows_the_cpus_pixels_byte_for_byte_and_only_what_changed_goes_up() {
        let _pool = Pool::new();
        let _one = one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        eprintln!("Metal: {}", gpu.name());
        assert_eq!(gpu.renderer(), Renderer::Metal);
        for scale in [1.0, 2.0] {
            let (mut ui, mut describe) = counter(Size { w: 400.0, h: 300.0 }, scale);
            ui.run(&mut describe);
            let mut surface = Surface::default();
            let whole = surface.update(&mut ui);
            let size = [surface.width, surface.height];
            let (_under, mut shows) = shown(&mut gpu, size);
            whole.iter().for_each(|place| gpu.put(&mut shows, &surface, *place));
            gpu.show(&mut shows, false);
            assert!(held(&mut gpu, &mut shows) == surface.pixels, "at {scale}: the texture holds the CPU's pixels");
            // A click: the label and the button change; only they go up.
            const MARK: u32 = 0xff20_4060;
            fill(&mut gpu, &mut shows, MARK);
            ui.event(crate::Event::Move(crate::geom::pt(30.0, 40.0)));
            ui.event(crate::Event::Press(crate::Button::Left, crate::Mods::NONE));
            ui.event(crate::Event::Release(crate::Button::Left));
            ui.run(&mut describe);
            let places = surface.update(&mut ui);
            places.iter().for_each(|place| gpu.put(&mut shows, &surface, *place));
            gpu.show(&mut shows, false);
            let texture = held(&mut gpu, &mut shows);
            assert_eq!(shows.named, places, "each place the run changed went up, and no other");
            let copied = texture.iter().filter(|texel| **texel != MARK).count();
            let inside = |at: usize| places.iter().any(|[l, t, r, b]| (*l..*r).contains(&(at % size[0])) && (*t..*b).contains(&(at / size[0])));
            assert!(texture.iter().enumerate().all(|(at, texel)| if inside(at) { *texel == surface.pixels[at] } else { *texel == MARK }), "at {scale}: the places and nothing else");
            eprintln!("at {scale}: {copied} of {} pixels went up", texture.len());
            assert!(copied > 0 && copied * 3 < texture.len());
            gpu.forget(shows);
        }
    }

    #[test]
    fn a_drawable_is_presented_with_the_whole_texture_in_it() {
        let _pool = Pool::new();
        let _one = one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        let (mut ui, mut describe) = counter(Size { w: 200.0, h: 120.0 }, 1.0);
        ui.run(&mut describe);
        let mut surface = Surface::default();
        let whole = surface.update(&mut ui);
        let (_under, mut shows) = shown(&mut gpu, [surface.width, surface.height]);
        whole.iter().for_each(|place| gpu.put(&mut shows, &surface, *place));
        // The drawable is taken here, not by `show`: its texture is read back after the copy.
        unsafe {
            let drawable: Obj = send(shows.layer, c"nextDrawable", ());
            assert!(!drawable.is_null(), "a layer of no window has drawables");
            gpu.flush(&mut shows);
            let commands = gpu.frame(&mut shows);
            gpu.copy_out(commands, shows.texture, send(drawable, c"texture", ()));
            send::<()>(commands, c"commit", ());
            send::<()>(shows.last, c"release", ());
            (shows.last, shows.frame) = (commands, std::ptr::null_mut());
            let target: Obj = send(drawable, c"texture", ());
            assert_eq!([send::<usize>(target, c"width", ()), send::<usize>(target, c"height", ())], shows.size, "the drawable is the window's size");
            assert!(read(&gpu, &mut shows, target) == surface.pixels, "the drawable holds the CPU's pixels");
        }
        // Live, as while a window's edge is dragged: presented after the frame is scheduled.
        gpu.show(&mut shows, true);
        gpu.show(&mut shows, false);
        assert!(!gpu.lost);
        gpu.forget(shows);
    }

    #[test]
    fn a_window_too_big_for_the_device_is_not_shown_and_a_new_size_is_a_new_texture() {
        let _pool = Pool::new();
        let _one = one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        let under = made(c"CALayer");
        assert!(gpu.window(under, NSRect::new(0.0, 0.0, 1.0, 1.0), [100_000, 10], 1.0).is_none(), "no texture that wide");
        let (_under, mut shows) = shown(&mut gpu, [64, 48]);
        let before = shows.texture;
        assert!(gpu.fit(&mut shows, NSRect::new(0.0, 0.0, 64.0, 48.0), [64, 48], 2.0) && shows.texture == before, "the same size keeps its texture");
        assert!(gpu.fit(&mut shows, NSRect::new(0.0, 0.0, 40.0, 30.0), [80, 60], 2.0) && shows.size == [80, 60]);
        gpu.forget(shows);
    }

    /// Waits for the last frame, and reads `texture` back (RGBA16Float: an EDR drawable's) as
    /// floats.
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    fn read_light(gpu: &Gpu, shows: &mut Shows, texture: Obj) -> Vec<[f32; 4]> {
        let [width, height] = shows.size;
        let mut halves = vec![[0u16; 4]; width * height];
        unsafe {
            send::<()>(shows.last, c"waitUntilCompleted", ());
            let buffer: Obj = send(gpu.device, c"newBufferWithLength:options:", (width * height * 8, 0usize));
            let commands: Obj = send(gpu.queue, c"commandBuffer", ());
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            let copy = c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:";
            send::<()>(blit, copy, (texture, 0usize, 0usize, Three([0, 0, 0]), Three([width, height, 1]), buffer, 0usize, width * 8, width * height * 8));
            send::<()>(blit, c"endEncoding", ());
            send::<()>(commands, c"commit", ());
            send::<()>(commands, c"waitUntilCompleted", ());
            let there: *const [u16; 4] = send(buffer, c"contents", ());
            std::ptr::copy_nonoverlapping(there, halves.as_mut_ptr(), halves.len());
            send::<()>(buffer, c"release", ());
        }
        halves.into_iter().map(|pixel| pixel.map(crate::hdr::from_half)).collect()
    }

    /// The EDR test's window: a picture of white, grey, a red and black, with a gain map of a
    /// gain per colour; and a white box eight times as bright.
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    fn lit_window(picture: &mut Option<crate::Picture>, ui: &mut Ui) {
        use crate::hdr::{Curve, GainMap, half};
        let shown = *picture.get_or_insert_with(|| {
            let added = ui.add_picture(4, vec![0xffff_ffff, 0xff80_8080, 0xffc0_4020, 0xff00_0000]);
            let stops = [[0.0f32, 0.0, 0.0], [1.0, 1.0, 1.0], [3.0, 1.0, -1.0], [2.0, 2.0, 2.0]];
            let values = stops.iter().flatten().map(|stop| half(*stop)).collect();
            ui.light.keep_map(added, GainMap { width: 4, colours: 3, values, curve: Curve::STOPS, serial: 0 });
            added
        });
        ui.el().flow(crate::Flow::Stack).size(8.0, 1.0).open();
        ui.el().picture(shown).size(4.0, 1.0).show();
        ui.el().offset(4.0, 0.0).size(4.0, 1.0).fill(crate::Color::WHITE).brightness(8.0).show();
        ui.close();
    }

    #[test]
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    fn a_window_lit_on_a_screen_with_headroom_is_shown_in_edr_and_back() {
        let _pool = Pool::new();
        let _one = one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        // This Mac's own screens, as they are (read only).
        unsafe {
            for screen in crate::macos_sys::array_items(send(class(c"NSScreen"), c"screens", ())) {
                let [now, most]: [f64; 2] = [send(screen, c"maximumExtendedDynamicRangeColorComponentValue", ()), send(screen, c"maximumPotentialExtendedDynamicRangeColorComponentValue", ())];
                eprintln!("a screen: EDR headroom {now} now, {most} at most");
            }
        }
        let mut trial = crate::testing::Trial::new(None, lit_window);
        trial.resize(8.0, 1.0, 1.0);
        let surface = Surface::draw(&mut trial.ui, 64);
        let (_under, mut shows) = shown(&mut gpu, [8, 1]);
        gpu.put(&mut shows, &surface, [0, 0, 8, 1]);
        // An EDR screen of 5 times SDR white: in EDR, each colour lit by its gain, at most 5.
        let edr = crate::hdr::Screen { hdr: true, white: 80.0, peak: 400.0 };
        assert_eq!(gpu.light(&mut shows, edr, &mut trial.ui), (true, true), "the app hears the headroom; the window is shown again");
        assert_eq!(trial.ui.headroom(), 5.0);
        unsafe {
            assert_eq!(send::<usize>(shows.layer, c"pixelFormat", ()), crate::hdr_metal::HALVES, "the layer takes 16-bit floats");
            assert_ne!(send::<Bool>(shows.layer, c"wantsExtendedDynamicRangeContent", ()), 0, "the layer wants EDR");
            let drawable: Obj = send(shows.layer, c"nextDrawable", ());
            assert!(!drawable.is_null(), "a layer of no window has drawables");
            let target: Obj = send(drawable, c"texture", ());
            assert_eq!(send::<usize>(target, c"pixelFormat", ()), crate::hdr_metal::HALVES, "the drawable is of 16-bit floats");
            gpu.flush(&mut shows);
            let commands = gpu.frame(&mut shows);
            gpu.copy_or_light(commands, &mut shows, target);
            send::<()>(commands, c"commit", ());
            send::<()>(shows.last, c"release", ());
            (shows.last, shows.frame) = (commands, std::ptr::null_mut());
            let light = read_light(&gpu, &mut shows, target);
            let linear = |byte: u32| {
                let part = byte as f32 / 255.0;
                if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
            };
            let picture = [[0xff; 3], [0x80; 3], [0xc0, 0x40, 0x20], [0; 3]].map(|pixel| pixel.map(linear));
            let gains = [[1.0, 1.0, 1.0], [2.0, 2.0, 2.0], [5.0, 2.0, 0.5], [4.0, 4.0, 4.0]];
            let wanted: Vec<[f32; 3]> = (0..4).map(|at| std::array::from_fn(|part| picture[at][part] * gains[at][part])).chain([[5.0; 3]; 4]).collect();
            eprintln!("EDR: {light:?}");
            for (at, (got, wanted)) in light.iter().zip(&wanted).enumerate() {
                assert!((0..3).all(|part| (got[part] - wanted[part]).abs() <= wanted[part] / 500.0 + 1e-3) && got[3] == 1.0, "at {at}: {got:?}, not {wanted:?}");
            }
        }
        // Shown, and then the screen has no headroom: back to BGRA in sRGB, the CPU's pixels.
        gpu.show(&mut shows, false);
        assert!(!gpu.lost, "an EDR frame shows");
        assert_eq!(gpu.light(&mut shows, crate::hdr::Screen::SDR, &mut trial.ui), (true, true));
        unsafe {
            assert_eq!(send::<usize>(shows.layer, c"pixelFormat", ()), BGRA8);
            assert_eq!(send::<Bool>(shows.layer, c"wantsExtendedDynamicRangeContent", ()), 0);
        }
        gpu.show(&mut shows, false);
        assert!(held(&mut gpu, &mut shows) == surface.pixels && !shows.in_edr(), "SDR: the CPU's pixels, as ever");
        gpu.forget(shows);
    }

    #[test]
    fn a_lost_device_is_said_and_a_new_one_shows_the_window_whole() {
        let _pool = Pool::new();
        let _one = one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        gpu.lose();
        assert!(gpu.lost);
        drop(gpu);
        NO_DEVICE.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(super::open(true, std::ptr::null_mut()).join().unwrap().is_none(), "none to be had: the CPU shows the windows");
        NO_DEVICE.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut again = super::open(true, std::ptr::null_mut()).join().unwrap().expect("a new device");
        let surface = Surface { width: 30, height: 20, pixels: (0..600).map(|at| 0xff00_0000 | (at * 0x0001_0203)).collect() };
        let (_under, mut shows) = shown(&mut again, [30, 20]);
        again.put(&mut shows, &surface, [0, 0, 30, 20]);
        again.show(&mut shows, false);
        assert!(held(&mut again, &mut shows) == surface.pixels);
        again.forget(shows);
    }
}
