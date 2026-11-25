//! The Android window shown through the GPU, with the feature `gpu`: by Vulkan
//! (`gpu_vulkan_android`) or OpenGL ES 3 (`gpu_opengl_android`), the first that works in the
//! order asked; else the CPU shows it (`android`). The CPU draws each place that changed into the
//! window's `Surface` as without the feature, and that place goes up; with `gpu-draw` the GPU
//! draws large places itself. Either way the pixels shown are the CPU's, byte for byte (within a
//! few levels at edges where the GPU draws).
//!
//! | Asked | Tried |
//! | --- | --- |
//! | `Auto` | Vulkan, then OpenGL ES; only a GPU, not the CPU at work (SwiftShader) |
//! | `Gpu`, `Vulkan`, `Direct3D11`, `Direct3D12`, `Metal` | Vulkan, then OpenGL ES |
//! | `OpenGL` | OpenGL ES, then Vulkan |
//! | `Cpu` | none |
//!
//! The device is opened when the window first shows, on the main thread. It outlives the
//! activity's window: a window that goes (the activity paused or gone) takes only its surface
//! with it, and the next is drawn whole. A device lost is let go of and opened again, drawn
//! whole; after three, the CPU shows the window.
use crate::android_input_method::Panel;
use crate::geom::{Rect, rect};
use crate::platform::{Renderer, ShownBy};
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::c_void;

/// Devices lost before the CPU shows the window for good.
const MOST_LOST: u8 = 3;

/// Pixels to put up: rows `width` wide, whose first lies at `at` (left, top) in the window's
/// pixels.
pub(crate) struct Block<'a> {
    pub(crate) pixels: &'a [u32],
    pub(crate) width: usize,
    pub(crate) at: [i32; 2],
}

impl Block<'_> {
    /// `place` (left, top, right, bottom) cut to the block and to pixels `size` big; nothing
    /// where nothing is left.
    pub(crate) fn cut(&self, place: [i32; 4], size: [i32; 2]) -> Option<[i32; 4]> {
        let held = self.place();
        let [left, top] = [place[0].max(held[0]).max(0), place[1].max(held[1]).max(0)];
        let [right, bottom] = [place[2].min(held[2]).min(size[0]), place[3].min(held[3]).min(size[1])];
        (left < right && top < bottom).then_some([left, top, right, bottom])
    }

    /// Where the block lies (left, top, right, bottom).
    pub(crate) fn place(&self) -> [i32; 4] {
        let height = (self.pixels.len() / self.width.max(1)) as i32;
        [self.at[0], self.at[1], self.at[0] + self.width as i32, self.at[1] + height]
    }

    /// Each row of `place` (cut to the block): its number in the window's pixels, and its
    /// pixels.
    pub(crate) fn rows(&self, place: [i32; 4]) -> impl Iterator<Item = (usize, &[u32])> {
        let [left, right] = [place[0], place[2]].map(|x| (x - self.at[0]) as usize);
        (place[1]..place[3]).map(move |row| {
            let from = (row - self.at[1]) as usize * self.width;
            (row as usize, &self.pixels[from + left..from + right])
        })
    }
}

/// The GPU the window is shown through.
enum Gpu {
    Vulkan(Box<crate::gpu_vulkan_android::Gpu>),
    OpenGL(crate::gpu_opengl_android::Gpu),
}

/// The window shown through the GPU.
enum Shows {
    Vulkan(crate::gpu_vulkan_android::Shows),
    OpenGL(crate::gpu_opengl_android::Shows),
}

/// The activity's window as a frame finds it.
pub(crate) struct Window<'a> {
    /// The `ANativeWindow` (null while there is none, or the activity is paused).
    pub(crate) native: *mut c_void,
    /// Its size in pixels, and its insets (left, top, right, bottom): the interface lies inside.
    pub(crate) size: [i32; 2],
    pub(crate) insets: [i32; 4],
    /// The system asks for the window whole.
    pub(crate) again: bool,
    /// What an input method composes, as last shown and now (see `android_input_method`).
    pub(crate) panels: [Option<&'a Panel>; 2],
}

/// What the window keeps of the GPU: the device once open and the window's part on it; what it
/// asks (nothing once none is to be had), and how many devices were lost.
pub(crate) struct Screen {
    gpu: Option<Gpu>,
    shows: Option<Shows>,
    asks: Option<Renderer>,
    any: bool,
    losses: u8,
}

impl Screen {
    /// What a window asking for `renderer` keeps (as `TAUT_RENDERER` says, if set): nothing
    /// open yet.
    pub(crate) fn new(renderer: Renderer) -> Screen {
        let any = crate::platform::wants_gpu(renderer);
        let asks = any.map(|_| renderer.chosen(crate::platform::setting("TAUT_RENDERER").as_deref()));
        Screen { gpu: None, shows: None, asks, any: any.unwrap_or(false), losses: 0 }
    }

    /// What shows the window (while it is away too).
    pub(crate) fn renderer(&self) -> Renderer {
        match &self.gpu {
            Some(Gpu::Vulkan(_)) => Renderer::Vulkan,
            Some(Gpu::OpenGL(_)) => Renderer::OpenGL,
            None => Renderer::Cpu,
        }
    }

    /// The window goes: its part on the GPU goes with it, before its `ANativeWindow` does.
    pub(crate) fn window_gone(&mut self) {
        if let (Some(gpu), Some(shows)) = (self.gpu.as_mut(), self.shows.take()) {
            gpu.forget(shows);
        }
    }

    /// Draws what changed of `ui` and shows it through the GPU, where it shows the window: each
    /// place drawn by the CPU into `surface` and put up, or with `gpu-draw` drawn by the GPU;
    /// what an input method composes over it. Says how it was shown.
    pub(crate) fn draw(&mut self, ui: &mut Ui, surface: &mut Surface, window: &Window) -> ShownBy {
        if self.asks.is_none() {
            return ShownBy::Cpu;
        }
        if window.native.is_null() {
            return if self.gpu.is_some() { ShownBy::Gpu } else { ShownBy::Cpu };
        }
        let scale = ui.scale();
        let size = [(ui.size().w * scale).round() as i32, (ui.size().h * scale).round() as i32];
        if !self.ready(window, size) {
            self.let_go();
            self.asks = None;
            return ShownBy::CpuAfresh;
        }
        let (Some(gpu), Some(shows)) = (self.gpu.as_mut(), self.shows.as_mut()) else { return ShownBy::Cpu };
        let changed = gpu.draw_changes(shows, ui, surface, window);
        if changed || window.again || window.panels[0] != window.panels[1] {
            gpu.show(shows, ui.theme.bg.solid().0, window.again);
        }
        self.mend();
        if self.asks.is_some() { ShownBy::Gpu } else { ShownBy::CpuAfresh }
    }

    /// Opens the device and shows the window through it, where neither is yet, and gives the
    /// window its size (`size`, the interface's pixels). Says whether the GPU shows it.
    fn ready(&mut self, window: &Window, size: [i32; 2]) -> bool {
        if self.gpu.is_none() {
            self.gpu = self.asks.and_then(|asked| self.open(asked, window.native));
        }
        let Some(gpu) = self.gpu.as_mut() else { return false };
        if self.shows.is_none() {
            // SAFETY: the window is alive until `window_gone` (the activity says so first).
            self.shows = unsafe { gpu.window(window.native) };
        }
        let at = [window.insets[0], window.insets[1]];
        self.shows.as_mut().is_some_and(|shows| gpu.fit(shows, window.size, size, at))
    }

    /// The first renderer `asked` that opens and shows `native`.
    fn open(&mut self, asked: Renderer, native: *mut c_void) -> Option<Gpu> {
        let any = self.any;
        // SAFETY: the window is alive until `window_gone`.
        let (gpu, shows) = asked.tried().find_map(|renderer| unsafe { Gpu::open(renderer, native, any) })?;
        self.shows = Some(shows);
        Some(gpu)
    }

    /// Lets go of a device lost, and of a window gone: the next frame opens a new one (unless
    /// too many were lost), or shows the next window, whole.
    fn mend(&mut self) {
        if self.shows.as_ref().is_some_and(Shows::gone) {
            self.window_gone();
        }
        if self.gpu.as_ref().is_some_and(Gpu::lost) {
            self.let_go();
            self.losses += 1;
            self.asks = self.asks.filter(|_| self.losses <= MOST_LOST);
            crate::android::heard(crate::android::Heard::Redraw);
            crate::android::kick();
        }
    }

    /// Lets go of the window's part on the GPU, then of the GPU.
    fn let_go(&mut self) {
        self.window_gone();
        self.gpu = None;
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.let_go();
    }
}

impl Shows {
    /// Whether the window went (its surface is lost).
    fn gone(&self) -> bool {
        match self {
            Shows::Vulkan(shows) => shows.gone,
            Shows::OpenGL(shows) => shows.gone,
        }
    }

    /// Whether all of the window is to be drawn (its pixels are new); asked once.
    fn take_whole(&mut self) -> bool {
        match self {
            Shows::Vulkan(shows) => shows.take_whole(),
            Shows::OpenGL(shows) => shows.take_whole(),
        }
    }
}

impl Gpu {
    /// Opens `renderer` (Vulkan, else OpenGL ES) and shows `native` through it. Nothing where it
    /// can't, or where its only device is the CPU at work and `any` is false.
    ///
    /// # Safety
    /// `native` is an `ANativeWindow`, alive until the window is let go of.
    unsafe fn open(renderer: Renderer, native: *mut c_void, any: bool) -> Option<(Gpu, Shows)> {
        #[cfg(test)]
        if MISSING.get() == Some(renderer) {
            return None;
        }
        // SAFETY: as the caller promises.
        if renderer == Renderer::Vulkan {
            let (gpu, shows) = unsafe { crate::gpu_vulkan_android::Gpu::open(native, any) }?;
            return Some((Gpu::Vulkan(Box::new(gpu)), Shows::Vulkan(shows)));
        }
        let (gpu, shows) = unsafe { crate::gpu_opengl_android::Gpu::open(native, any) }?;
        Some((Gpu::OpenGL(gpu), Shows::OpenGL(shows)))
    }

    /// Shows `native` through the device. Nothing where it can't.
    ///
    /// # Safety
    /// As `open`.
    unsafe fn window(&mut self, native: *mut c_void) -> Option<Shows> {
        // SAFETY: as the caller promises.
        match self {
            Gpu::Vulkan(gpu) => unsafe { gpu.window(native) }.map(Shows::Vulkan),
            Gpu::OpenGL(gpu) => unsafe { gpu.window(native) }.map(Shows::OpenGL),
        }
    }

    /// Whether the device was lost, or failed: all of it is to be let go.
    fn lost(&self) -> bool {
        match self {
            Gpu::Vulkan(gpu) => gpu.lost,
            Gpu::OpenGL(gpu) => gpu.lost,
        }
    }

    /// Gives the window its size (`window` pixels), and the interface's pixels theirs (`size`,
    /// at `at` in the window). Says whether it has them.
    fn fit(&mut self, shows: &mut Shows, window: [i32; 2], size: [i32; 2], at: [i32; 2]) -> bool {
        match (self, shows) {
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.fit(shows, window, size, at),
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.fit(shows, window, size, at),
            _ => false,
        }
    }

    /// Puts `place` of a block of pixels up, to be shown with the next frame.
    fn put(&mut self, shows: &mut Shows, block: &Block, place: [i32; 4]) {
        match (self, shows) {
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.put(shows, block, place),
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.put(shows, block, place),
            _ => {}
        }
    }

    /// Shows the window with what was put, around it `background`; with nothing put, only
    /// where `again`.
    fn show(&mut self, shows: &mut Shows, background: u32, again: bool) {
        match (self, shows) {
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.show(shows, background, again),
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.show(shows, background, again),
            _ => {}
        }
    }

    /// Lets go of the window's part.
    fn forget(&mut self, shows: Shows) {
        match (self, shows) {
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.forget(shows),
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.forget(shows),
            _ => {}
        }
    }

    /// Draws what changed of `ui` (all of it where the window's pixels are new), the places an
    /// input method's panel left, and the panel over them. Says whether anything was drawn.
    fn draw_changes(&mut self, shows: &mut Shows, ui: &mut Ui, surface: &mut Surface, window: &Window) -> bool {
        let (scale, at) = (ui.scale(), [window.insets[0], window.insets[1]]);
        let everything = rect(0.0, 0.0, ui.size().w, ui.size().h);
        let mut places: Vec<Rect> = if shows.take_whole() { vec![everything] } else { ui.changes().collect() };
        if window.panels[0] != window.panels[1] {
            places.extend(window.panels[0].map(|panel| units(panel.place, at, scale)));
        }
        places.iter().for_each(|place| self.draw_place(shows, surface, ui, *place));
        ui.changed();
        let panel = window.panels[1].filter(|_| !places.is_empty() || window.panels[0] != window.panels[1]);
        if let Some(panel) = panel {
            let width = (panel.place[2] - panel.place[0]) as usize;
            let block = Block { pixels: &panel.pixels, width, at: [panel.place[0] - at[0], panel.place[1] - at[1]] };
            self.put(shows, &block, block.place());
        }
        !places.is_empty() || panel.is_some()
    }

    /// Draws one place `ui` changed (in units): by the GPU where it draws a part that big
    /// (`gpu-draw`), else by the CPU into `surface`, which then goes up.
    fn draw_place(&mut self, shows: &mut Shows, surface: &mut Surface, ui: &mut Ui, place: Rect) {
        let scale = ui.scale();
        let size = [(ui.size().w * scale).round() as i32, (ui.size().h * scale).round() as i32];
        let snapped = place.snap_to_pixels(scale);
        let edge = |at: f32, most: i32| ((at * scale).round() as i32).clamp(0, most);
        let pixels = [edge(snapped.x, size[0]), edge(snapped.y, size[1]), edge(snapped.right(), size[0]), edge(snapped.bottom(), size[1])];
        if pixels[0] >= pixels[2] || pixels[1] >= pixels[3] {
            return;
        }
        #[cfg(feature = "gpu-draw")]
        if self.drawn_by_gpu(shows, ui, snapped, pixels, size) {
            return;
        }
        surface.redraw(ui, place);
        let block = Block { pixels: &surface.pixels, width: surface.width, at: [0, 0] };
        self.put(shows, &block, pixels);
    }

    /// Draws `area` of the window (`place` in pixels: left, top, right, bottom) on the GPU, where
    /// it draws a part that big itself (see `gpu_draw`). Says whether it did; if not, the CPU is
    /// to.
    #[cfg(feature = "gpu-draw")]
    fn drawn_by_gpu(&mut self, shows: &mut Shows, ui: &mut Ui, area: Rect, place: [i32; 4], size: [i32; 2]) -> bool {
        let list = match self {
            Gpu::Vulkan(gpu) if gpu.drawing.takes(place) => &mut gpu.list,
            Gpu::OpenGL(gpu) if gpu.drawing.takes(place) => &mut gpu.list,
            _ => return false,
        };
        let mut list = std::mem::take(&mut **list);
        list.record(ui, area, size);
        let drawn = match (&mut *self, shows) {
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) if !list.failed => gpu.draw(shows, &list, place),
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) if !list.failed => gpu.draw(shows, &list, place),
            _ => false,
        };
        list.finish(drawn);
        match self {
            Gpu::Vulkan(gpu) => *gpu.list = list,
            Gpu::OpenGL(gpu) => *gpu.list = list,
        }
        drawn
    }
}

/// A place in the window's pixels (left, top, right, bottom), the interface lying at `at`, in
/// the interface's units at `scale`.
fn units(place: [i32; 4], at: [i32; 2], scale: f32) -> Rect {
    let [left, top, right, bottom] = [place[0] - at[0], place[1] - at[1], place[2] - at[0], place[3] - at[1]].map(|side| side as f32 / scale);
    rect(left, top, right - left, bottom - top)
}

#[cfg(test)]
thread_local! {
    /// Tests: this renderer is missing.
    static MISSING: std::cell::Cell<Option<Renderer>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point, Size, pt};
    use crate::{Button, Color, Event, LocalClipboard, Mods, text::BuiltInFont};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    #[link(name = "mediandk")]
    unsafe extern "C" {
        fn AImageReader_newWithUsage(width: i32, height: i32, format: i32, usage: u64, most: i32, reader: *mut *mut c_void) -> i32;
        fn AImageReader_delete(reader: *mut c_void);
        fn AImageReader_getWindow(reader: *mut c_void, window: *mut *mut c_void) -> i32;
        fn AImageReader_setImageListener(reader: *mut c_void, listener: *mut Listener) -> i32;
        fn AImageReader_acquireLatestImage(reader: *mut c_void, image: *mut *mut c_void) -> i32;
        fn AImage_delete(image: *mut c_void);
        fn AImage_getPlaneData(image: *const c_void, plane: i32, data: *mut *mut u8, length: *mut i32) -> i32;
        fn AImage_getPlaneRowStride(image: *const c_void, plane: i32, stride: *mut i32) -> i32;
    }

    /// `AImageReader_ImageListener`.
    #[repr(C)]
    struct Listener {
        context: *mut c_void,
        heard: extern "C" fn(*mut c_void, *mut c_void),
    }

    /// Frames come: how many so far.
    type Frames = (Mutex<u32>, Condvar);

    extern "C" fn image_came(context: *mut c_void, _: *mut c_void) {
        let frames = unsafe { &*context.cast::<Frames>() };
        *frames.0.lock().unwrap() += 1;
        frames.1.notify_all();
    }

    /// A window whose frames are read back: an image reader's (`AIMAGE_FORMAT_RGBA_8888`, drawn
    /// into by the GPU and read by the CPU).
    struct Reader {
        reader: *mut c_void,
        window: *mut c_void,
        size: [i32; 2],
        frames: Arc<Frames>,
        _listener: Box<Listener>,
    }

    impl Reader {
        fn new(size: [i32; 2]) -> Reader {
            let (mut reader, mut window) = (std::ptr::null_mut(), std::ptr::null_mut());
            let frames: Arc<Frames> = Arc::default();
            let mut listener = Box::new(Listener { context: Arc::as_ptr(&frames).cast_mut().cast(), heard: image_came });
            unsafe {
                assert_eq!(AImageReader_newWithUsage(size[0], size[1], 1, 0x200 | 0x3, 4, &mut reader), 0, "an image reader");
                AImageReader_getWindow(reader, &mut window);
                AImageReader_setImageListener(reader, &mut *listener);
            }
            Reader { reader, window, size, frames, _listener: listener }
        }

        /// How many frames came so far.
        fn count(&self) -> u32 {
            *self.frames.0.lock().unwrap()
        }

        /// The newest frame once more than `after` came, as `0xAARRGGBB` words; nothing after two
        /// seconds.
        fn frame(&self, after: u32) -> Option<Vec<u32>> {
            let (count, came) = &*self.frames;
            let waited = came.wait_timeout_while(count.lock().unwrap(), Duration::from_secs(2), |count| *count <= after).unwrap();
            if waited.1.timed_out() {
                return None;
            }
            drop(waited);
            let mut image = std::ptr::null_mut();
            unsafe {
                assert_eq!(AImageReader_acquireLatestImage(self.reader, &mut image), 0);
                let (mut data, mut length, mut stride) = (std::ptr::null_mut(), 0, 0);
                AImage_getPlaneData(image, 0, &mut data, &mut length);
                AImage_getPlaneRowStride(image, 0, &mut stride);
                let bytes = std::slice::from_raw_parts(data, length as usize);
                let [width, height] = self.size.map(|side| side as usize);
                let words = (0..width * height).map(|at| {
                    let i = (at / width) * stride as usize + (at % width) * 4;
                    0xff00_0000 | u32::from(bytes[i]) << 16 | u32::from(bytes[i + 1]) << 8 | u32::from(bytes[i + 2])
                });
                let pixels = words.collect();
                AImage_delete(image);
                Some(pixels)
            }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            unsafe { AImageReader_delete(self.reader) };
        }
    }

    /// The insets of the test window: left, top, right, bottom.
    const INSETS: [i32; 4] = [6, 30, 10, 14];
    const BACKGROUND: u32 = 0xff30_4050;

    /// A window drawn through a `Screen`, read back.
    struct Bench {
        screen: Screen,
        ui: Ui,
        surface: Surface,
        reader: Reader,
        count: u32,
        app: Box<dyn FnMut(&mut Ui)>,
    }

    /// An interface with a count and a button, a tick box and a rounded box.
    fn counter() -> Box<dyn FnMut(&mut Ui)> {
        let mut count = 0;
        Box::new(move |ui: &mut Ui| {
            ui.column().pad(10.0).gap(8.0).open();
            ui.label(&format!("Pressed {count} times"));
            count += ui.button("Press").clicked as u32;
            ui.el().size(120.0, 40.0).fill(Color::rgb(0x3060c0)).radius(8.0).show();
            ui.close();
        })
    }

    impl Bench {
        /// `app` in a window `size` pixels at `scale` shown as `renderer` asks.
        fn new(renderer: Renderer, size: [i32; 2], scale: f32, app: Box<dyn FnMut(&mut Ui)>) -> Bench {
            let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            ui.theme.bg = Color(BACKGROUND);
            let mut bench = Bench { screen: Screen::new(renderer), ui, surface: Surface::default(), reader: Reader::new(size), count: 0, app };
            bench.resize(scale);
            bench
        }

        /// The interface sized to the window inside the insets, at `scale`, run.
        fn resize(&mut self, scale: f32) {
            let [width, height] = self.reader.size;
            let inner = [width - INSETS[0] - INSETS[2], height - INSETS[1] - INSETS[3]];
            self.ui.resize(Size { w: inner[0] as f32 / scale, h: inner[1] as f32 / scale }, scale);
            self.run();
        }

        fn run(&mut self) {
            let app = &mut self.app;
            self.ui.run(&mut |ui| app(ui));
            self.ui.theme.bg = Color(BACKGROUND);
        }

        /// Draws through the screen, with `panels` (what an input method composed before, now);
        /// how it was shown.
        fn draw(&mut self, again: bool, panels: [Option<&Panel>; 2]) -> ShownBy {
            let window = Window { native: self.reader.window, size: self.reader.size, insets: INSETS, again, panels };
            self.screen.draw(&mut self.ui, &mut self.surface, &window)
        }

        /// The frame shown after the last draw.
        fn shown(&mut self) -> Vec<u32> {
            let frame = self.reader.frame(self.count).expect("a frame comes");
            self.count = self.reader.count();
            frame
        }

        /// A click at `at`, in units, then a run.
        fn click(&mut self, at: Point) {
            self.ui.event(Event::Move(at));
            self.ui.event(Event::Press(Button::Left, Mods::NONE));
            self.ui.event(Event::Release(Button::Left));
            self.run();
        }

        /// Checks a frame: the interface, as the CPU draws it afresh, inside the insets; the
        /// background around. Says how many pixels differ, at most `tolerance` levels apart.
        fn check(&mut self, frame: &[u32], tolerance: u32, what: &str) -> usize {
            let drawn = Surface::draw(&mut self.ui, 64);
            let width = self.reader.size[0] as usize;
            let (mut differ, mut most, mut at) = (0, 0, 0);
            for (i, pixel) in frame.iter().enumerate() {
                let [x, y] = [(i % width) as i32 - INSETS[0], (i / width) as i32 - INSETS[1]];
                let inside = (0..drawn.width as i32).contains(&x) && (0..drawn.height as i32).contains(&y);
                let wanted = if inside { drawn.pixels[y as usize * drawn.width + x as usize] } else { BACKGROUND };
                let by = [16, 8, 0].map(|shift| (pixel >> shift & 255).abs_diff(wanted >> shift & 255)).into_iter().max().unwrap_or(0);
                differ += usize::from(by > 0);
                if by > most {
                    (most, at) = (by, i);
                }
            }
            eprintln!("{what}: {differ} pixels differ, at most by {most}");
            assert!(most <= tolerance, "{what}: pixel {}, {} is {:08x}, {most} levels off", at % width, at / width, frame[at]);
            differ
        }

        /// The places the last frame named, in the interface's pixels.
        fn named(&self) -> Vec<[i32; 4]> {
            match &self.screen.shows {
                Some(Shows::Vulkan(shows)) => crate::gpu_vulkan_android::Gpu::named(shows).to_vec(),
                Some(Shows::OpenGL(shows)) => crate::gpu_opengl_android::Gpu::named(shows).to_vec(),
                None => Vec::new(),
            }
        }

        /// The device has the GPU draw every part (`gpu-draw`).
        #[cfg(feature = "gpu-draw")]
        fn draw_every_part(&mut self) {
            match self.screen.gpu.as_mut() {
                Some(Gpu::Vulkan(gpu)) => gpu.drawing = crate::gpu_draw::Drawing::Always,
                Some(Gpu::OpenGL(gpu)) => gpu.drawing = crate::gpu_draw::Drawing::Always,
                None => panic!("no GPU"),
            }
        }

        /// The device is lost, as when it is removed.
        fn lose(&mut self) {
            match self.screen.gpu.as_mut() {
                Some(Gpu::Vulkan(gpu)) => gpu.lose(),
                Some(Gpu::OpenGL(gpu)) => gpu.lose(),
                None => panic!("no GPU"),
            }
        }
    }

    /// The renderers to check, each named, with the device's name.
    const BOTH: [Renderer; 2] = [Renderer::Vulkan, Renderer::OpenGL];

    /// One check on the GPU at a time.
    fn one() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn a_block_puts_only_what_it_holds_where_the_window_has_room() {
        let pixels: Vec<u32> = (0..12).collect();
        let block = Block { pixels: &pixels, width: 4, at: [10, 20] };
        assert_eq!(block.cut([0, 0, 100, 100], [100, 100]), Some([10, 20, 14, 23]));
        assert_eq!(block.cut([12, 21, 100, 100], [13, 100]), Some([12, 21, 13, 23]));
        assert_eq!(block.cut([0, 0, 10, 100], [100, 100]), None);
        let rows: Vec<(usize, Vec<u32>)> = block.rows([11, 21, 13, 23]).map(|(row, from)| (row, from.to_vec())).collect();
        assert_eq!(rows, [(21, vec![5, 6]), (22, vec![9, 10])]);
    }

    #[test]
    fn each_renderer_shows_the_cpus_pixels_byte_for_byte_and_names_only_what_changed() {
        let _one = one();
        for renderer in BOTH {
            let mut bench = Bench::new(renderer, [360, 300], 1.5, counter());
            assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu, "{renderer:?} shows the window");
            assert_eq!(bench.screen.renderer(), renderer);
            let frame = bench.shown();
            assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, the first frame")), 0);
            bench.click(pt(30.0, 40.0));
            let places: Vec<[i32; 4]> = bench.ui.places.iter().map(|place| place.snap_to_pixels(1.5)).map(|p| [p.x, p.y, p.right(), p.bottom()].map(|side| (side * 1.5).round() as i32)).collect();
            assert!(!places.is_empty());
            bench.draw(false, [None, None]);
            let frame = bench.shown();
            assert_eq!(bench.named(), places, "{renderer:?}: the places the run changed are named, and no others");
            assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, after a click")), 0);
            assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu);
            assert!(bench.reader.frame(bench.count).is_none(), "{renderer:?}: nothing changed, nothing shown");
            bench.draw(true, [None, None]);
            let frame = bench.shown();
            assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, shown again whole")), 0);
        }
    }

    #[test]
    fn a_window_that_goes_takes_its_surface_and_the_next_is_shown_whole() {
        let _one = one();
        for renderer in BOTH {
            let mut bench = Bench::new(renderer, [360, 300], 2.0, counter());
            bench.draw(false, [None, None]);
            bench.shown();
            bench.screen.window_gone();
            assert!(bench.screen.gpu.is_some() && bench.screen.shows.is_none(), "{renderer:?}: the device stays");
            // The window turned: a new one, its sides swapped.
            (bench.reader, bench.count) = (Reader::new([300, 360]), 0);
            bench.resize(2.0);
            assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu);
            let frame = bench.shown();
            assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, the next window")), 0);
        }
    }

    #[test]
    fn a_lost_device_is_opened_again_and_after_too_many_the_cpu_shows() {
        let _one = one();
        for renderer in BOTH {
            let mut bench = Bench::new(renderer, [200, 160], 1.0, counter());
            bench.draw(false, [None, None]);
            bench.shown();
            for lost in 1..=MOST_LOST {
                bench.lose();
                bench.click(pt(30.0, 40.0));
                assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu, "{renderer:?}: lost {lost} times, the GPU still shows");
                assert!(bench.screen.gpu.is_none(), "{renderer:?}: the lost device is let go");
                assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu);
                let frame = bench.shown();
                assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, a new device after {lost} lost")), 0);
            }
            bench.lose();
            bench.click(pt(30.0, 40.0));
            assert_eq!(bench.draw(false, [None, None]), ShownBy::CpuAfresh, "{renderer:?}: one lost too many");
            assert_eq!((bench.screen.renderer(), bench.draw(false, [None, None])), (Renderer::Cpu, ShownBy::Cpu));
        }
    }

    #[test]
    fn a_missing_renderer_gives_way_to_the_next() {
        let _one = one();
        for (missing, then) in [(Renderer::Vulkan, Renderer::OpenGL), (Renderer::OpenGL, Renderer::Vulkan)] {
            MISSING.set(Some(missing));
            let mut bench = Bench::new(missing, [200, 160], 1.0, counter());
            assert_eq!(bench.draw(false, [None, None]), ShownBy::Gpu);
            assert_eq!(bench.screen.renderer(), then);
            let frame = bench.shown();
            assert_eq!(bench.check(&frame, 0, &format!("{then:?} for {missing:?}")), 0);
        }
        MISSING.set(None);
        let mut cpu = Bench::new(Renderer::Cpu, [200, 160], 1.0, counter());
        assert_eq!(cpu.draw(false, [None, None]), ShownBy::Cpu, "the CPU asked: no GPU");
    }

    #[test]
    fn auto_takes_no_device_that_is_the_cpu_at_work() {
        let _one = one();
        let mut bench = Bench::new(Renderer::Vulkan, [64, 48], 1.0, counter());
        bench.draw(false, [None, None]);
        let software = match &bench.screen.gpu {
            Some(Gpu::Vulkan(gpu)) => gpu.name().contains("SwiftShader"),
            _ => panic!("Vulkan shows"),
        };
        let mut auto = Bench::new(Renderer::Auto, [64, 48], 1.0, counter());
        let shown = auto.draw(false, [None, None]);
        eprintln!("Auto: {shown:?} ({})", if software { "SwiftShader" } else { "a GPU" });
        assert_eq!(shown == ShownBy::Gpu, !software);
    }

    #[test]
    fn what_an_input_method_composes_shows_over_the_window_and_leaves_it_as_it_was() {
        let _one = one();
        for renderer in BOTH {
            let mut bench = Bench::new(renderer, [300, 240], 1.0, counter());
            bench.draw(false, [None, None]);
            bench.shown();
            let place = [INSETS[0] + 20, INSETS[1] + 30, INSETS[0] + 60, INSETS[1] + 46];
            let panel = Panel { place, pixels: vec![0xffee_2211; 40 * 16] };
            bench.draw(false, [None, Some(&panel)]);
            let frame = bench.shown();
            let width = bench.reader.size[0];
            let inside = |at: usize| (place[0]..place[2]).contains(&(at as i32 % width)) && (place[1]..place[3]).contains(&(at as i32 / width));
            assert!(frame.iter().enumerate().all(|(at, pixel)| !inside(at) || *pixel == 0xffee_2211), "{renderer:?}: the panel shows");
            bench.draw(false, [Some(&panel), None]);
            let frame = bench.shown();
            assert_eq!(bench.check(&frame, 0, &format!("{renderer:?}, the panel gone")), 0);
        }
    }

    #[cfg(feature = "gpu-draw")]
    #[test]
    fn each_renderer_draws_every_kind_of_part_within_a_few_levels_of_the_cpu() {
        let _one = one();
        for renderer in BOTH {
            let mut pictures = None;
            let app = Box::new(move |ui: &mut Ui| crate::gpu_draw::tests::every_kind(ui, &mut pictures));
            let mut bench = Bench::new(renderer, [INSETS[0] + INSETS[2] + 540, INSETS[1] + INSETS[3] + 315], 1.5, app);
            bench.draw(false, [None, None]);
            bench.shown();
            // A new window: all of it is drawn, by the GPU.
            bench.draw_every_part();
            bench.screen.window_gone();
            (bench.reader, bench.count) = (Reader::new(bench.reader.size), 0);
            bench.draw(false, [None, None]);
            let frame = bench.shown();
            let drawn = match &bench.screen.gpu {
                Some(Gpu::Vulkan(gpu)) => gpu.has_drawn(),
                Some(Gpu::OpenGL(gpu)) => gpu.has_drawn(),
                None => false,
            };
            assert!(drawn, "{renderer:?} drew the parts itself");
            bench.check(&frame, crate::gpu_draw::tests::TOLERANCE, &format!("{renderer:?}, drawn by the GPU"));
        }
    }
}
