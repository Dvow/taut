//! Windows' GPU paths (the feature `gpu`): which one shows the windows, and the calls they all
//! take. Each path shows the pixels the CPU drew, uploading only what changed:
//!
//! | Path | Module | Presents |
//! | --- | --- | --- |
//! | Direct3D 11 | `gpu_d3d11` | a DXGI flip-model swap chain, with dirty rects |
//! | Direct3D 12 | `gpu_d3d12` | a DXGI flip-model swap chain, with dirty rects |
//! | Vulkan | `gpu_vulkan_windows` | a `VK_KHR_win32_surface` swap chain, with `VK_KHR_incremental_present` where the device has it |
//! | OpenGL | `gpu_opengl_windows` | WGL, OpenGL 3.3, whole frames |
//!
//! A path asked for that is missing falls back along that order. Windows shows no GDI drawing in
//! a window that has had a flip-model swap chain, so after a loss with no GPU left, Direct3D 11's
//! software device (WARP) shows the windows, on the CPU.
use crate::platform::Renderer;
use crate::windows::sys::{GetSystemMetrics, HANDLE, RECT};
use crate::{gpu_d3d11, gpu_d3d12, gpu_opengl_windows, gpu_vulkan_windows};
use std::ffi::CStr;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn GetProcAddress(library: isize, name: *const u8) -> usize;
}

/// A DLL of the system folder, loaded for good: its handle, 0 if missing.
pub(crate) fn system_library(name: &str) -> isize {
    let name = crate::windows::wide(name);
    // SAFETY: a name ending in a zero; 0x800 (`LOAD_LIBRARY_SEARCH_SYSTEM32`) searches only
    // the system folder.
    unsafe { LoadLibraryExW(name.as_ptr(), 0, 0x800) }
}

/// A function of a loaded library: its address, 0 if missing.
pub(crate) fn function_in(library: isize, name: &CStr) -> usize {
    // SAFETY: a name ending in a zero.
    unsafe { GetProcAddress(library, name.as_ptr().cast()) }
}

/// A device of one of the paths.
enum Made {
    D3d11(gpu_d3d11::Device),
    D3d12(gpu_d3d12::Device),
    Vulkan(Box<gpu_vulkan_windows::Device>),
    Gl(gpu_opengl_windows::Device),
}

/// The device the windows are shown with.
pub(crate) struct Device {
    made: Made,
    /// It was asked for by `Renderer::Auto`, which a new one after a loss is too.
    fussy: bool,
    /// Where the GPU draws the windows too, and what it records to (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: crate::gpu_draw::DrawList,
    /// What OpenGL draws with: not yet tried, or what that gave.
    #[cfg(feature = "gpu-draw")]
    opengl: Option<Option<crate::gpu_draw_opengl::Pipeline>>,
}

impl Device {
    /// A device of a path, asked for by `Renderer::Auto` if `fussy`. Where the feature
    /// `gpu-draw` is on it draws the windows too, as `TAUT_DRAW` and `gpu_draw::Drawing` say.
    fn new(made: Made, fussy: bool) -> Device {
        #[cfg(feature = "gpu-draw")]
        let gpu = match &made {
            Made::D3d11(device) => device.gpu && device.drawing.is_some(),
            Made::D3d12(device) => device.drawing.is_some(),
            Made::Vulkan(device) => device.drawing.is_some(),
            Made::Gl(_) => true,
        };
        #[cfg(feature = "gpu-draw")]
        let drawing = crate::gpu_draw::Drawing::chosen(fussy, gpu, std::env::var("TAUT_DRAW").ok().as_deref());
        #[cfg(feature = "gpu-draw")]
        let mut list = crate::gpu_draw::DrawList::default();
        #[cfg(feature = "gpu-draw")]
        {
            list.no_runs = matches!(&made, Made::Vulkan(device) if !device.drawing.as_ref().is_some_and(|pipeline| pipeline.dual));
        }
        #[cfg(feature = "gpu-draw")]
        return Device { made, fussy, drawing, list, opengl: None };
        #[cfg(not(feature = "gpu-draw"))]
        Device { made, fussy }
    }

    /// Whether the GPU draws a part of a window, `area` pixels (left, top, right, bottom).
    #[cfg(feature = "gpu-draw")]
    #[inline]
    pub(crate) fn draws(&self, area: [i32; 4]) -> bool {
        let path = match &self.made {
            Made::D3d11(device) => device.drawing.is_some(),
            Made::D3d12(device) => device.drawing.is_some(),
            Made::Vulkan(device) => device.drawing.is_some(),
            Made::Gl(_) => !matches!(self.opengl, Some(None)),
        };
        path && self.drawing.takes(area)
    }

    /// A device on the first of `asked`'s GPU paths that has one (see `Renderer::tried`). If
    /// `fussy` (`Renderer::Auto`), none over a remote desktop. Never a software device.
    pub(crate) fn gpu(asked: Renderer, fussy: bool) -> Option<Device> {
        // SAFETY: a plain query (`SM_REMOTESESSION`).
        if fussy && !cfg!(test) && unsafe { GetSystemMetrics(0x1000) } != 0 {
            return None;
        }
        asked.tried().find_map(|path| Some(Device::new(Device::of(path)?, fussy)))
    }

    /// A GPU device of `path`, if it has one.
    fn of(path: Renderer) -> Option<Made> {
        #[cfg(test)]
        if tests::faked().no_gpu || tests::faked().missing.contains(&path) {
            return None;
        }
        match path {
            Renderer::Direct3D11 => gpu_d3d11::Device::gpu().map(Made::D3d11),
            Renderer::Direct3D12 => gpu_d3d12::Device::gpu().map(Made::D3d12),
            Renderer::Vulkan => gpu_vulkan_windows::Device::gpu().map(|device| Made::Vulkan(Box::new(device))),
            _ => gpu_opengl_windows::Device::gpu().map(Made::Gl),
        }
    }

    /// Direct3D 11's software device, WARP: the CPU, for windows that cannot go back to GDI.
    pub(crate) fn software() -> Option<Device> {
        Some(Device::new(Made::D3d11(gpu_d3d11::Device::software()?), false))
    }

    /// A device after `lost` was lost: a GPU one, its path first, if it was a GPU; else WARP.
    pub(crate) fn again(lost: Device) -> Option<Device> {
        let (was, fussy) = (lost.renderer(), lost.fussy);
        drop(lost);
        Some(was).filter(|was| *was != Renderer::Cpu).and_then(|was| Device::gpu(was, fussy)).or_else(Device::software)
    }

    /// What shows the windows: the path, or the CPU for WARP.
    pub(crate) fn renderer(&self) -> Renderer {
        match &self.made {
            Made::D3d11(device) if device.gpu => Renderer::Direct3D11,
            Made::D3d11(_) => Renderer::Cpu,
            Made::D3d12(_) => Renderer::Direct3D12,
            Made::Vulkan(_) => Renderer::Vulkan,
            Made::Gl(_) => Renderer::OpenGL,
        }
    }

    /// Whether the device is gone: removed, reset or hung.
    pub(crate) fn removed(&self) -> bool {
        #[cfg(test)]
        if tests::taken_loss() {
            return true;
        }
        match &self.made {
            Made::D3d11(device) => device.removed(),
            Made::D3d12(device) => device.removed(),
            Made::Vulkan(device) => device.removed(),
            Made::Gl(device) => device.removed(),
        }
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").field("renderer", &self.renderer()).field("fussy", &self.fussy).finish()
    }
}

/// A window shown by one of the paths.
pub(crate) enum Shown {
    D3d11(gpu_d3d11::Shown),
    D3d12(gpu_d3d12::Shown),
    Vulkan(gpu_vulkan_windows::Shown),
    Gl(gpu_opengl_windows::Shown),
}

impl Shown {
    /// Window `hwnd` shown by `device`, `size` pixels (made to fit); nothing where it can't be.
    pub(crate) fn new(device: &Device, hwnd: HANDLE, size: [i32; 2]) -> Option<Shown> {
        let mut shown = match &device.made {
            Made::D3d11(device) => Shown::D3d11(gpu_d3d11::Shown::new(device, hwnd, size)?),
            Made::D3d12(device) => Shown::D3d12(gpu_d3d12::Shown::new(device, hwnd, size)?),
            Made::Vulkan(device) => Shown::Vulkan(gpu_vulkan_windows::Shown::new(device, hwnd)?),
            Made::Gl(device) => Shown::Gl(gpu_opengl_windows::Shown::new(device, hwnd)?),
        };
        shown.fit(device, size).map(|_| shown)
    }

    /// Makes what shows the window `size` pixels where it is not. Says whether it was remade
    /// (the window is then to be drawn whole); nothing if that failed.
    pub(crate) fn fit(&mut self, device: &Device, size: [i32; 2]) -> Option<bool> {
        if size[0] <= 0 || size[1] <= 0 {
            return Some(false);
        }
        match (self, &device.made) {
            (Shown::D3d11(shown), Made::D3d11(device)) => shown.fit(device, size),
            (Shown::D3d12(shown), Made::D3d12(device)) => shown.fit(device, size),
            (Shown::Vulkan(shown), Made::Vulkan(device)) => shown.fit(device, size),
            (Shown::Gl(shown), Made::Gl(device)) => shown.fit(device, size),
            _ => None,
        }
    }

    /// Puts `size` pixels in at `at` (cut to the window), read from `first`, rows `pitch` pixels
    /// apart.
    ///
    /// # Safety
    /// The pixels read lie at `first`: as many rows as given, each as wide.
    pub(crate) unsafe fn upload(&mut self, device: &Device, at: [i32; 2], size: [i32; 2], first: *const u32, pitch: usize) {
        // SAFETY: as the caller promises.
        unsafe {
            match (self, &device.made) {
                (Shown::D3d11(shown), Made::D3d11(device)) => shown.upload(device, at, size, first, pitch),
                (Shown::D3d12(shown), Made::D3d12(device)) => shown.upload(device, at, size, first, pitch),
                (Shown::Vulkan(shown), Made::Vulkan(_)) => shown.upload(at, size, first, pitch),
                (Shown::Gl(shown), Made::Gl(device)) => shown.upload(device, at, size, first, pitch),
                _ => {}
            }
        }
    }

    /// Draws a recorded paint into what shows the window, inside `area` (left, top, right,
    /// bottom). Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, device: &mut Device, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        let drawn = match (self, &mut device.made) {
            (Shown::D3d11(shown), Made::D3d11(device)) => shown.draw(device, list, area),
            (Shown::D3d12(shown), Made::D3d12(device)) => shown.draw(device, list, area),
            (Shown::Vulkan(shown), Made::Vulkan(device)) => shown.draw(device, list, area),
            (Shown::Gl(shown), Made::Gl(_)) => shown.draw(&mut device.opengl, list, area),
            _ => false,
        };
        #[cfg(test)]
        tests::DRAWN.set(tests::DRAWN.get() + drawn as u32);
        drawn
    }

    /// What the window keeps of HDR, where its path presents in HDR: Direct3D 11 and 12 (see
    /// `hdr_windows`), Vulkan where the surface takes it (`hdr_vulkan`), OpenGL where its pixel
    /// format is of floats (`hdr_opengl`).
    #[cfg(feature = "hdr")]
    pub(crate) fn hdr(&mut self) -> Option<&mut crate::hdr_windows::Hdr> {
        match self {
            Shown::D3d11(shown) => Some(&mut shown.hdr),
            Shown::D3d12(shown) => Some(&mut shown.hdr),
            Shown::Vulkan(shown) => shown.hdr(),
            Shown::Gl(shown) => shown.hdr(),
        }
    }

    /// Shows what was put in, `dirty` being what changed, at the screen's next refresh. Says
    /// whether that worked.
    pub(crate) fn present(&mut self, device: &Device, dirty: &[RECT]) -> bool {
        #[cfg(test)]
        if tests::lose_now() {
            return false;
        }
        let presented = match (self, &device.made) {
            (Shown::D3d11(shown), Made::D3d11(device)) => shown.present(device, dirty),
            (Shown::D3d12(shown), Made::D3d12(device)) => shown.present(device, dirty),
            (Shown::Vulkan(shown), Made::Vulkan(device)) => shown.present(device, dirty),
            (Shown::Gl(shown), Made::Gl(device)) => shown.present(device),
            _ => false,
        };
        #[cfg(test)]
        tests::failed(presented);
        presented
    }
}

/// Whether `dirty` lies all inside a window `size` pixels, each part not empty: else the whole
/// window changed, as far as a present knows.
pub(crate) fn inside(dirty: &[RECT], size: [i32; 2]) -> &[RECT] {
    let fits = |part: &RECT| part.left >= 0 && part.top >= 0 && part.right <= size[0] && part.bottom <= size[1] && part.left < part.right && part.top < part.bottom;
    if dirty.iter().all(fits) { dirty } else { &[] }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::Rect;
    use crate::windows::open;
    use crate::windows::sys::*;
    use crate::windows::tests::{ALONE, is_on_its_own_desktop, on_a_desktop_of_its_own, picture, pump};
    use crate::{Color, Ui, Window};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// What the tests make of devices on this thread.
    #[derive(Clone, Copy, Default)]
    pub(crate) struct Faked {
        /// The GPU paths take software devices as GPUs: WARP for Direct3D 11 and 12, a CPU
        /// device for Vulkan (SwiftShader, where the test found one). No real GPU is touched.
        pub software: bool,
        /// No GPU device can be made.
        pub no_gpu: bool,
        /// Nor a WARP one.
        pub no_warp: bool,
        /// These paths have no device.
        pub missing: &'static [Renderer],
        /// The next present reports the device lost, and so does the device when asked.
        pub lose: bool,
        /// Direct3D 12 removes its device for real before the next present.
        pub remove: bool,
        /// Each present keeps a copy of what it shows (`KEPT`).
        pub keep: bool,
        /// The device comes as it does outside tests, later: not waited for at the start.
        pub later: bool,
    }

    /// What a present showed: its window, row length, pixels and changed rectangles.
    type Presented = (HANDLE, usize, Vec<u32>, Vec<RECT>);

    thread_local! {
        static FAKED: Cell<Faked> = Cell::new(Faked::default());
        static LOST: Cell<bool> = const { Cell::new(false) };
        /// How many times a device being made came to the windows.
        static CAME: Cell<u32> = const { Cell::new(0) };
        static KEPT: RefCell<Vec<Presented>> = const { RefCell::new(Vec::new()) };
        /// How many presents failed, not counting losses a test asked for.
        static FAILED: Cell<u32> = const { Cell::new(0) };
        /// How many parts the GPU drew itself (`gpu-draw`).
        pub(crate) static DRAWN: Cell<u32> = const { Cell::new(0) };
    }

    pub(crate) fn failed(presented: bool) {
        FAILED.set(FAILED.get() + !presented as u32);
    }

    pub(crate) fn faked() -> Faked {
        FAKED.get()
    }
    pub(crate) fn fake(faked: Faked) {
        FAKED.set(faked);
    }

    /// Whether this present is to fail as if the device were lost, as a test asked.
    pub(crate) fn lose_now() -> bool {
        let mut faked = faked();
        if !std::mem::take(&mut faked.lose) {
            return false;
        }
        fake(faked);
        LOST.set(true);
        true
    }
    pub(crate) fn taken_loss() -> bool {
        LOST.take()
    }
    pub(crate) fn came() {
        CAME.set(CAME.get() + 1);
    }

    /// Keeps what a present shows, if a test asked: window `hwnd`'s pixels, `width` a row.
    pub(crate) fn kept(hwnd: HANDLE, width: usize, pixels: Vec<u32>, dirty: &[RECT]) {
        KEPT.with_borrow_mut(|kept| kept.push((hwnd, width, pixels, dirty.to_vec())));
    }

    /// Handles the windows' messages until a device being made came since `before` (ten seconds
    /// at most).
    fn until_the_device_came(before: u32) {
        let start = std::time::Instant::now();
        while CAME.get() == before && start.elapsed() < std::time::Duration::from_secs(10) {
            pump();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(CAME.get() > before, "no device came");
    }

    /// The last frame presented for a window: its pixels, row length and changed rectangles
    /// (none: all).
    fn last_presented(hwnd: HANDLE) -> (Vec<u32>, usize, Vec<RECT>) {
        let last = KEPT.with_borrow(|kept| kept.iter().rev().find(|(of, ..)| *of == hwnd).cloned());
        let (_, width, pixels, dirty) = last.expect("nothing presented");
        (pixels, width, dirty)
    }

    #[test]
    fn the_users_choice_goes_over_the_apps() {
        let said = [Some("cpu"), Some("GPU"), Some("d3d11"), Some("D3D12"), Some("vulkan"), Some("OpenGL"), Some("Metal"), Some(""), Some("glide"), None];
        for asked in [Renderer::Auto, Renderer::Cpu, Renderer::Gpu, Renderer::Vulkan] {
            let chosen = said.map(|said| asked.chosen(said));
            assert_eq!(chosen, [Renderer::Cpu, Renderer::Gpu, Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Vulkan, Renderer::OpenGL, Renderer::Metal, asked, asked, asked]);
        }
    }

    #[test]
    fn a_path_named_comes_first_then_the_best() {
        let tried = |asked: Renderer| asked.tried().collect::<Vec<_>>();
        let [d3d11, d3d12, vulkan, gl] = [Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Vulkan, Renderer::OpenGL];
        assert_eq!(tried(Renderer::Auto), [d3d11, d3d12, vulkan, gl]);
        assert_eq!(tried(Renderer::Gpu), [d3d11, d3d12, vulkan, gl]);
        assert_eq!(tried(gl), [gl, d3d11, d3d12, vulkan]);
        assert_eq!(tried(d3d12), [d3d12, d3d11, vulkan, gl]);
        assert!(tried(Renderer::Cpu).is_empty());
    }

    /// What an app saw on its last run.
    #[derive(Clone, Copy, Default)]
    struct Saw {
        renderer: Option<Renderer>,
        button: Rect,
        scale: f32,
        presses: u32,
    }

    /// An interface with text, a button that counts, a text box, rounded boxes and a gradient,
    /// that tells what it saw.
    fn busy(saw: Rc<Cell<Saw>>) -> impl FnMut(&mut Ui) + 'static {
        let mut text = String::from("Typed text");
        move |ui: &mut Ui| {
            let mut now = saw.get();
            ui.column().pad(12.0).gap(8.0).open();
            ui.label(&format!("Pressed {} times", now.presses));
            let button = ui.button("Press");
            now.presses += button.clicked as u32;
            ui.text_box(&mut text, "Type here");
            ui.row().gap(6.0).open();
            for shade in 0..6u32 {
                ui.el().size(24.0, 24.0).fill(Color::rgb(0x28_0000 * shade + 0x00_3c5a)).radius(6.0).show();
            }
            ui.el().size(60.0, 24.0).gradient(Color::rgb(0xff_2020), Color::rgb(0x20_20ff)).show();
            ui.close();
            ui.close();
            (now.renderer, now.button, now.scale) = (Some(ui.renderer()), button.rect, ui.scale());
            saw.set(now);
        }
    }

    /// Runs a window's interface (`WM_WAKE`, as `Waker` does) and handles what follows.
    fn wake(hwnd: HANDLE) {
        unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
        pump();
    }

    #[test]
    fn a_window_with_no_gpu_to_be_had_is_drawn_by_the_cpu() {
        fake(Faked { no_gpu: true, ..Faked::default() });
        let (saw, before) = (Rc::new(Cell::new(Saw::default())), CAME.get());
        let hwnd = open(Window { renderer: Renderer::Gpu, ..Window::default() }, Box::new(busy(saw.clone())));
        until_the_device_came(before);
        wake(hwnd);
        unsafe { DestroyWindow(hwnd) };
        fake(Faked::default());
        assert_eq!(saw.get().renderer, Some(Renderer::Cpu));
    }

    #[test]
    fn a_window_asked_to_be_drawn_by_the_cpu_never_makes_a_device() {
        // Asking for none, the window never has one made.
        let (saw, before) = (Rc::new(Cell::new(Saw::default())), CAME.get());
        let hwnd = open(Window { renderer: Renderer::Cpu, ..Window::default() }, Box::new(busy(saw.clone())));
        wake(hwnd);
        std::thread::sleep(std::time::Duration::from_millis(100));
        pump();
        unsafe { DestroyWindow(hwnd) };
        assert_eq!((saw.get().renderer, CAME.get()), (Some(Renderer::Cpu), before));
    }

    #[test]
    fn a_path_that_is_missing_falls_back_along_the_order() {
        // With software devices only Direct3D 11 and 12 (WARP) have one here, and Vulkan where
        // SwiftShader is found: the order is what is checked.
        let made = |asked: Renderer, missing: &'static [Renderer]| {
            fake(Faked { software: true, missing, ..Faked::default() });
            let renderer = Device::gpu(asked, false).map(|device| device.renderer());
            fake(Faked::default());
            renderer
        };
        let [d3d11, d3d12, vulkan, gl] = [Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Vulkan, Renderer::OpenGL];
        assert_eq!(made(d3d12, &[]), Some(d3d12));
        assert_eq!(made(Renderer::Gpu, &[]), Some(d3d11));
        assert_eq!(made(gl, &[Renderer::OpenGL]), Some(d3d11));
        assert_eq!(made(vulkan, &[Renderer::Vulkan, Renderer::Direct3D11]), Some(d3d12));
        assert_eq!(made(Renderer::Auto, &[Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Vulkan, Renderer::OpenGL]), None);
    }

    /// Opens a window as `run` does, at a place of its own on this private desktop, and shows it
    /// without taking focus; one asking for the GPU once its device came.
    fn shown(renderer: Renderer, x: i32, saw: &Rc<Cell<Saw>>) -> HANDLE {
        let before = CAME.get();
        let hwnd = open(Window { renderer, position: Some((x, 0)), width: 360.0, height: 220.0, ..Window::default() }, Box::new(busy(saw.clone())));
        unsafe { ShowWindow(hwnd, 4) };
        pump();
        if renderer != Renderer::Cpu {
            until_the_device_came(before);
        }
        // Run once more, so every box knows where the first run put it.
        wake(hwnd);
        hwnd
    }

    /// Presses a window's button, as the person would with the mouse.
    fn press(hwnd: HANDLE, saw: &Cell<Saw>) {
        let Saw { button, scale, .. } = saw.get();
        let (x, y) = (((button.x + button.w / 2.0) * scale) as isize, ((button.y + button.h / 2.0) * scale) as isize);
        for (msg, w) in [(0x0200, 0), (0x0201, 1), (0x0202, 0)] {
            unsafe { PostMessageW(hwnd, msg, w, x | y << 16) };
        }
        pump();
    }

    /// Whether two windows show the same pixels, byte for byte.
    /// (A frame reaches the compositor, where the picture is taken, some time after its present:
    /// a frame later for OpenGL. So the pictures are taken again for two seconds at most.)
    fn same(a: HANDLE, b: HANDLE) -> bool {
        let start = std::time::Instant::now();
        loop {
            let ((one, wide), (other, also)) = (picture(a), picture(b));
            if (wide == also && alike(&one, &other)) || start.elapsed() > std::time::Duration::from_secs(2) {
                return wide == also && alike(&one, &other);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            pump();
        }
    }

    /// Whether two pictures are the same in colour: byte for byte; or where the GPU draws
    /// (`gpu-draw`), no channel further apart than it may be (see `gpu_draw`).
    fn alike(one: &[u32], other: &[u32]) -> bool {
        #[cfg(feature = "gpu-draw")]
        let near = |a: u32, b: u32| [16, 8, 0].iter().all(|shift| (a >> shift & 255).abs_diff(b >> shift & 255) <= crate::gpu_draw::tests::TOLERANCE);
        #[cfg(not(feature = "gpu-draw"))]
        let near = |a: u32, b: u32| a & 0xff_ffff == b & 0xff_ffff;
        one.len() == other.len() && one.iter().zip(other).all(|(a, b)| near(*a, *b))
    }

    /// How two windows' pixels differ: how many, and the first.
    fn differ(a: HANDLE, b: HANDLE) -> String {
        let ((one, wide), (other, _)) = (picture(a), picture(b));
        let differing: Vec<usize> = (0..one.len().min(other.len())).filter(|at| one[*at] != other[*at]).collect();
        let first = differing.first().map(|at| (at % wide, at / wide, one[*at], other[*at]));
        let off = |at: &usize| [16, 8, 0].map(|shift| (one[*at] >> shift & 255).abs_diff(other[*at] >> shift & 255)).into_iter().max().unwrap_or(0);
        let most = differing.iter().max_by_key(|at| off(at)).map(|at| (at % wide, at / wide, one[*at], other[*at]));
        format!("{} pixels of {}, first {first:x?}, most {most:x?}", differing.len(), one.len())
    }

    /// Whether a presented frame has the pixels a window drawn by the CPU shows, in colour. (Its
    /// alpha byte is the strip's, which neither shows.)
    fn presented_as(presented: &(Vec<u32>, usize, Vec<RECT>), cpu: HANDLE) -> bool {
        let (shown, wide) = picture(cpu);
        presented.1 == wide && alike(&presented.0, &shown)
    }

    /// The faked devices a path is tested with on this computer, and whether it can be tested
    /// at all: WARP for Direct3D, SwiftShader for Vulkan where Edge has it, else the computer's
    /// own GPU (briefly), and that for OpenGL.
    fn tested_with(path: Renderer) -> Option<Faked> {
        let software = match path {
            Renderer::Direct3D11 | Renderer::Direct3D12 => true,
            Renderer::Vulkan => crate::gpu_vulkan_windows::tests::swiftshader_file().is_some(),
            _ => false,
        };
        fake(Faked { software, ..Faked::default() });
        let made = Device::of(path).is_some();
        fake(Faked::default());
        made.then_some(Faked { software, keep: true, ..Faked::default() })
    }

    /// A window shown by `path` and one by the CPU show the same pixels: at first, after a
    /// press (only what changed is presented, where the path names it), once resized, and after
    /// a lost device was made again. Both what is presented (read back from the GPU) and the
    /// window's picture; but on a desktop of its own the picture of a window shown by WGL is
    /// never one of its frames (NVIDIA's, here), so for OpenGL only what is presented.
    fn the_same_pixels_by(path: Renderer) {
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        let Some(usual) = tested_with(path) else { return println!("no {path:?} device here: nothing to compare") };
        let same = |gpu: HANDLE, cpu: HANDLE| path == Renderer::OpenGL || same(gpu, cpu);
        // The device comes as it does for a program: the CPU shows the window until then.
        fake(Faked { later: true, ..usual });
        let (on_gpu, on_cpu) = (Rc::new(Cell::new(Saw::default())), Rc::new(Cell::new(Saw::default())));
        let (gpu, cpu) = (shown(path, 0, &on_gpu), shown(Renderer::Cpu, 900, &on_cpu));
        assert_eq!((on_gpu.get().renderer, on_cpu.get().renderer), (Some(path), Some(Renderer::Cpu)));
        assert!(same(gpu, cpu), "the first frames differ: {}", differ(gpu, cpu));
        let first = last_presented(gpu);
        assert!(presented_as(&first, cpu), "the first frame presented differs");
        #[cfg(feature = "gpu-draw")]
        assert!(DRAWN.get() > 0, "{path:?}: drawn by the GPU");
        // A press: the windows still agree; only what changed is named, where the path names it.
        press(gpu, &on_gpu);
        press(cpu, &on_cpu);
        assert_eq!((on_gpu.get().presses, on_cpu.get().presses), (1, 1));
        let after = last_presented(gpu);
        let all = (first.1 * (first.0.len() / first.1)) as i32;
        let small = after.2.iter().all(|part| (part.right - part.left) * (part.bottom - part.top) < all / 4);
        assert!(path == Renderer::OpenGL || (!after.2.is_empty() && small), "presented whole: {:?}", after.2);
        assert!(presented_as(&after, cpu), "the frame presented differs after a press");
        assert!(same(gpu, cpu), "the windows differ after a press: {}", differ(gpu, cpu));
        // Resized, both are drawn again, all of them.
        for hwnd in [gpu, cpu] {
            unsafe { SetWindowPos(hwnd, 0, 0, 0, 520, 380, 0x16) };
        }
        pump();
        assert!(same(gpu, cpu) && presented_as(&last_presented(gpu), cpu), "the windows differ once resized");
        assert_eq!(FAILED.get(), 0, "a present failed");
        // The device is lost at the next present, and made again on the same path: the app
        // hears nothing, and the window is drawn again, whole.
        fake(Faked { lose: path != Renderer::Direct3D12, remove: path == Renderer::Direct3D12, ..usual });
        press(gpu, &on_gpu);
        press(cpu, &on_cpu);
        let (pixels, wide, dirty) = last_presented(gpu);
        let whole = RECT { left: 0, top: 0, right: wide as i32, bottom: (pixels.len() / wide) as i32 };
        assert!(dirty.is_empty() || dirty == [whole], "not presented whole after the loss: {dirty:?}");
        assert_eq!(on_gpu.get().renderer, Some(path));
        assert!(same(gpu, cpu) && presented_as(&last_presented(gpu), cpu), "the windows differ after a device was made again");
        // Only the present a real removal failed.
        assert_eq!(FAILED.get(), (path == Renderer::Direct3D12) as u32, "a present failed");
        for hwnd in [gpu, cpu] {
            unsafe { DestroyWindow(hwnd) };
        }
    }

    #[test]
    fn direct3d_11_and_the_cpu_show_the_same_pixels() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_direct3d_11_shows_the_same_pixels");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_direct3d_11_shows_the_same_pixels() {
        if std::env::var_os(ALONE).is_some() {
            the_same_pixels_by(Renderer::Direct3D11);
        }
    }

    #[test]
    fn direct3d_12_and_the_cpu_show_the_same_pixels() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_direct3d_12_shows_the_same_pixels");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_direct3d_12_shows_the_same_pixels() {
        if std::env::var_os(ALONE).is_some() {
            the_same_pixels_by(Renderer::Direct3D12);
        }
    }

    #[test]
    fn vulkan_and_the_cpu_show_the_same_pixels() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_vulkan_shows_the_same_pixels");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_vulkan_shows_the_same_pixels() {
        if std::env::var_os(ALONE).is_some() {
            the_same_pixels_by(Renderer::Vulkan);
        }
    }

    #[test]
    fn opengl_and_the_cpu_show_the_same_pixels() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_opengl_shows_the_same_pixels");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_opengl_shows_the_same_pixels() {
        if std::env::var_os(ALONE).is_some() {
            the_same_pixels_by(Renderer::OpenGL);
        }
    }

    #[test]
    fn a_device_lost_for_good_is_left_to_the_cpu() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_a_device_lost_for_good_is_left_to_the_cpu");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_device_lost_for_good_is_left_to_the_cpu() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        let usual = Faked { software: true, keep: true, ..Faked::default() };
        fake(usual);
        let (on_gpu, on_cpu) = (Rc::new(Cell::new(Saw::default())), Rc::new(Cell::new(Saw::default())));
        let (gpu, cpu) = (shown(Renderer::Direct3D12, 0, &on_gpu), shown(Renderer::Cpu, 900, &on_cpu));
        assert_eq!(on_gpu.get().renderer, Some(Renderer::Direct3D12));
        // Lost, and no GPU to be had: the software device shows it, on the CPU.
        fake(Faked { lose: true, no_gpu: true, ..usual });
        press(gpu, &on_gpu);
        press(cpu, &on_cpu);
        wake(gpu);
        assert_eq!(on_gpu.get().renderer, Some(Renderer::Cpu));
        assert!(same(gpu, cpu) && presented_as(&last_presented(gpu), cpu), "the windows differ on the software device");
        // Lost once more, with no device at all: it is left to GDI, and goes on working.
        fake(Faked { lose: true, no_gpu: true, no_warp: true, ..usual });
        press(gpu, &on_gpu);
        press(gpu, &on_gpu);
        assert_eq!((on_gpu.get().renderer, on_gpu.get().presses), (Some(Renderer::Cpu), 3));
        for hwnd in [gpu, cpu] {
            unsafe { DestroyWindow(hwnd) };
        }
    }

    #[test]
    fn on_this_computers_own_gpu_every_path_shows_the_same_pixels() {
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_on_this_computers_own_gpu_the_pixels_are_the_same");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_on_this_computers_own_gpu_the_pixels_are_the_same() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        // A computer with no GPU of its own has nothing to show here.
        for path in Renderer::Gpu.tried() {
            if Device::of(path).is_none() {
                println!("no {path:?} on a GPU of its own: nothing to compare");
                continue;
            }
            fake(Faked { keep: true, ..Faked::default() });
            let (on_gpu, on_cpu) = (Rc::new(Cell::new(Saw::default())), Rc::new(Cell::new(Saw::default())));
            let (gpu, cpu) = (shown(path, 0, &on_gpu), shown(Renderer::Cpu, 900, &on_cpu));
            // (On a desktop of its own the picture of a window shown by WGL is never one of its
            // frames: for OpenGL only what is presented is compared.)
            let same = |gpu: HANDLE, cpu: HANDLE| path == Renderer::OpenGL || same(gpu, cpu);
            assert_eq!(on_gpu.get().renderer, Some(path));
            assert!(same(gpu, cpu) && presented_as(&last_presented(gpu), cpu), "{path:?}: the windows differ");
            press(gpu, &on_gpu);
            press(cpu, &on_cpu);
            assert!(same(gpu, cpu) && presented_as(&last_presented(gpu), cpu), "{path:?}: the windows differ after a press");
            for hwnd in [gpu, cpu] {
                unsafe { DestroyWindow(hwnd) };
            }
            pump();
        }
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetGuiResources(process: HANDLE, which: u32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> HANDLE;
        fn GetProcessHandleCount(process: HANDLE, count: *mut u32) -> i32;
    }

    #[test]
    fn windows_the_gpu_showed_leave_nothing_behind() {
        // In a process of its own: other tests' windows and devices are not counted.
        on_a_desktop_of_its_own("gpu_windows::tests::on_a_desktop_nobody_sees_windows_the_gpu_showed_leave_nothing_behind");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_windows_the_gpu_showed_leave_nothing_behind() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        // GDI (0) and USER (1) objects, and handles, of the whole program.
        let held = || unsafe {
            let mut handles = 0;
            GetProcessHandleCount(GetCurrentProcess(), &mut handles);
            [GetGuiResources(GetCurrentProcess(), 0), GetGuiResources(GetCurrentProcess(), 1), handles]
        };
        for path in [Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Vulkan, Renderer::OpenGL] {
            let Some(usual) = tested_with(path) else { continue };
            let usual = Faked { keep: false, ..usual };
            // A window shown by the path, presented, lost once (all made again), closed.
            let round = || {
                fake(usual);
                let (saw, before) = (Rc::new(Cell::new(Saw::default())), CAME.get());
                let hwnd = open(Window { renderer: path, ..Window::default() }, Box::new(busy(saw.clone())));
                until_the_device_came(before);
                fake(Faked { lose: true, ..usual });
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                wake(hwnd);
                assert_eq!(saw.get().renderer, Some(path));
                unsafe { DestroyWindow(hwnd) };
                pump();
            };
            round();
            let before = held();
            (0..20).for_each(|_| round());
            let after = held();
            fake(Faked::default());
            // One kept a round would be 20 more; drivers' thread pools may take a few handles more.
            // NVIDIA's OpenGL keeps a USER object for each thread that made a context, after the
            // thread ends: each round's device is made on a thread of its own.
            let threads = if path == Renderer::OpenGL { [0, 20, 0] } else { [0; 3] };
            let kept = (0..3).all(|at| after[at] < before[at] + 16 + threads[at]);
            assert!(kept, "{path:?}: before {before:?}, after {after:?}");
        }
    }

    #[test]
    fn with_the_feature_the_test_program_carries_the_gpu_path() {
        // The scan `windows::tests` makes without the feature, shown to find it with.
        assert!(crate::windows::tests::has_device_creation());
    }
}
