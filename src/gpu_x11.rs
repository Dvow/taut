//! X windows shown through the GPU, with the feature `gpu`: by OpenGL (`egl`) or Vulkan
//! (`swapchain`), the first that works in the order asked; else X shows them. Either way the
//! pixels shown are the CPU's, byte for byte.
//!
//! | Asked | Tried |
//! | --- | --- |
//! | `Auto` | OpenGL, then Vulkan; only a device showing frames directly |
//! | `Gpu`, `OpenGL`, `Direct3D11`, `Direct3D12` | OpenGL, then Vulkan |
//! | `Vulkan` | Vulkan, then OpenGL |
//! | `Cpu` | none |
use crate::platform::Renderer;
use crate::x11::{dlclose, dlopen, library, symbol};
use std::ffi::{c_char, c_int, c_ulong, c_void};

/// The frames whose places a window keeps, for back buffers that old.
pub(crate) const KEPT: usize = 3;
/// Most places drawn one by one in a frame; beyond, the area around them all.
const MOST: usize = 16;

/// What windows ask of the GPU: the renderer chosen (whose `tried` gives the order), and
/// whether any device will do (else only one showing frames directly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) chosen: Renderer,
    pub(crate) any: bool,
}

/// What windows asking for `asked` ask of the GPU, as `TAUT_RENDERER` says if set: nothing for
/// the CPU.
pub(crate) fn asked(asked: Renderer) -> Option<Asked> {
    let any = crate::platform::wants_gpu(asked)?;
    Some(Asked { chosen: asked.chosen(std::env::var("TAUT_RENDERER").ok().as_deref()), any })
}

/// The GPU a program's windows are shown through.
pub(crate) enum Gpu {
    OpenGL(crate::gpu_opengl_x11::Gpu),
    Vulkan(crate::gpu_vulkan_x11::Gpu),
}

/// A window shown through the GPU.
pub(crate) enum Shows {
    OpenGL(crate::gpu_opengl_x11::Shows),
    Vulkan(crate::gpu_vulkan_x11::Shows),
}

/// How a window is shown: not yet asked of the GPU, through it, or by X.
pub(crate) enum Way {
    /// Not yet asked of the GPU.
    Unasked,
    /// Through the GPU.
    Gpu(Shows),
    /// By X.
    X,
}

impl Shows {
    /// Its pixels' size.
    pub(crate) fn size(&self) -> [c_int; 2] {
        match self {
            Shows::OpenGL(shows) => shows.size(),
            Shows::Vulkan(shows) => shows.size(),
        }
    }
}

/// Lets go of a window's part on the GPU, where the GPU shows it: X shows it from then on.
pub(crate) fn unshow(gpu: &mut Option<Gpu>, way: &mut Way) {
    if let (Some(gpu), Way::Gpu(shows)) = (gpu.as_mut(), std::mem::replace(way, Way::X)) {
        gpu.forget(shows);
    }
}

impl Gpu {
    /// Opens the first renderer `asked` that works on X display `x`, showing window `id`
    /// through it (`size` pixels, of `visual`). Nothing where none does.
    ///
    /// # Safety
    /// `x` is an open X display, and `id` one of its windows, alive, of `visual`.
    pub(crate) unsafe fn open(x: *mut c_void, visual: *mut c_void, id: c_ulong, size: [c_int; 2], asked: Asked) -> Option<(Gpu, Shows)> {
        // `Auto` takes only a device that shows frames directly: where X offers neither DRI3 nor
        // NVIDIA's own GLX, none does, and no driver is loaded (tens of MB) to find that out.
        // SAFETY: as the caller promises.
        if !asked.any && !unsafe { has_dri3(x) || offers(x, c"NV-GLX") } {
            return None;
        }
        // SAFETY: as the caller promises.
        asked.chosen.tried().find_map(|renderer| unsafe { Gpu::open_by(renderer, x, visual, id, size, asked.any) })
    }

    /// See `open`: one renderer.
    unsafe fn open_by(renderer: Renderer, x: *mut c_void, visual: *mut c_void, id: c_ulong, size: [c_int; 2], any: bool) -> Option<(Gpu, Shows)> {
        #[cfg(test)]
        if NO_DEVICE.get() || MISSING.get() == Some(renderer) {
            return None;
        }
        // SAFETY: as the caller promises.
        if renderer == Renderer::Vulkan {
            let (gpu, shows) = unsafe { crate::gpu_vulkan_x11::Gpu::open(x, visual, id, size, any) }?;
            return Some((Gpu::Vulkan(gpu), Shows::Vulkan(shows)));
        }
        let (gpu, shows) = unsafe { crate::gpu_opengl_x11::Gpu::open(x, visual, id, size, any) }?;
        Some((Gpu::OpenGL(gpu), Shows::OpenGL(shows)))
    }

    /// Which renderer it is.
    pub(crate) fn renderer(&self) -> Renderer {
        match self {
            Gpu::OpenGL(_) => Renderer::OpenGL,
            Gpu::Vulkan(_) => Renderer::Vulkan,
        }
    }

    /// Whether the device was lost, or failed: all of it is to be let go.
    pub(crate) fn lost(&self) -> bool {
        match self {
            Gpu::OpenGL(gpu) => gpu.lost,
            Gpu::Vulkan(gpu) => gpu.lost,
        }
    }

    /// Tests: the device's name.
    #[cfg(test)]
    pub(crate) fn name(&self) -> String {
        match self {
            Gpu::OpenGL(gpu) => gpu.name(),
            Gpu::Vulkan(gpu) => gpu.name(),
        }
    }

    /// Shows window `id` through the device, `size` pixels big. Nothing where it can't.
    pub(crate) fn window(&mut self, id: c_ulong, size: [c_int; 2]) -> Option<Shows> {
        match self {
            Gpu::OpenGL(gpu) => gpu.window(id, size).map(Shows::OpenGL),
            Gpu::Vulkan(gpu) => gpu.window(id, size).map(Shows::Vulkan),
        }
    }

    /// Gives a window's pixels on the GPU their size. Says whether they have it: where the
    /// device makes none that big, the window is better shown by X.
    pub(crate) fn fit(&mut self, shows: &mut Shows, size: [c_int; 2]) -> bool {
        match (self, shows) {
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.fit(shows, size),
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.fit(shows, size),
            _ => false,
        }
    }

    /// Puts part of a painted strip into a window's pixels on the GPU: `place` (left, top,
    /// right, bottom in the window's pixels), the strip's first row being its top, `width`
    /// pixels a row.
    pub(crate) fn put(&mut self, shows: &mut Shows, strip: &[u32], width: c_int, place: [c_int; 4]) {
        match (self, shows) {
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.put(shows, strip, width, place),
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.put(shows, strip, width, place),
            _ => {}
        }
    }

    /// What the GPU records a part of a window `area` pixels big into (left, top, right,
    /// bottom), where it draws such a part itself (see `gpu_draw`): OpenGL and Vulkan do.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn drawing(&mut self, area: [c_int; 4]) -> Option<&mut crate::gpu_draw::DrawList> {
        match self {
            Gpu::OpenGL(gpu) if gpu.drawing.takes(area) => Some(&mut *gpu.list),
            Gpu::Vulkan(gpu) if gpu.drawing.takes(area) => Some(&mut *gpu.list),
            _ => None,
        }
    }

    /// Draws a recorded paint into a window's pixels on the GPU, inside `place`, to be shown
    /// with what was put in; and keeps `list` for the next one. Says whether it drew: if not,
    /// the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, mut list: crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        let drawn = match (&mut *self, shows) {
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) if !list.failed => gpu.draw(shows, &list, place),
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) if !list.failed => gpu.draw(shows, &list, place),
            _ => false,
        };
        list.finish(drawn);
        match self {
            Gpu::OpenGL(gpu) => *gpu.list = list,
            Gpu::Vulkan(gpu) => *gpu.list = list,
        }
        drawn
    }

    /// Shows what was put into a window's pixels since they were last shown.
    pub(crate) fn show(&mut self, shows: &mut Shows) {
        match (self, shows) {
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.show(shows),
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.show(shows),
            _ => {}
        }
    }

    /// Lets go of a window.
    pub(crate) fn forget(&mut self, shows: Shows) {
        match (self, shows) {
            (Gpu::OpenGL(gpu), Shows::OpenGL(shows)) => gpu.forget(shows),
            (Gpu::Vulkan(gpu), Shows::Vulkan(shows)) => gpu.forget(shows),
            _ => {}
        }
    }

    /// Whether frames are shown directly (what `Auto` asks).
    ///
    /// # Safety
    /// `x` is an open X display.
    #[cfg(test)]
    pub(crate) unsafe fn direct(&self, x: *mut c_void) -> bool {
        // SAFETY: as the caller promises.
        match self {
            Gpu::OpenGL(gpu) => unsafe { gpu.direct(x) },
            Gpu::Vulkan(gpu) => unsafe { gpu.direct(x) },
        }
    }

    /// Tests: the places the last frame named.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        match shows {
            Shows::OpenGL(shows) => crate::gpu_opengl_x11::Gpu::named(shows),
            Shows::Vulkan(shows) => crate::gpu_vulkan_x11::Gpu::named(shows),
        }
    }

    /// Tests: the device is lost, as when it is removed.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        match self {
            Gpu::OpenGL(gpu) => gpu.lose(),
            Gpu::Vulkan(gpu) => gpu.lose(),
        }
    }
}

/// Notes a place put into a window's pixels: a strip under the last of the same place makes
/// that place taller.
pub(crate) fn note(put: &mut Vec<[c_int; 4]>, place: [c_int; 4]) {
    let [left, top, right, bottom] = place;
    match put.last_mut() {
        Some(last) if [last[0], last[2], last[3]] == [left, right, top] => last[3] = bottom,
        _ => put.push(place),
    }
}

/// What a back buffer of a window `size` pixels big needs drawn: the places `put` since the last
/// frame, and those of the frames it `missed` (`past`, newest first). All of it where its age is
/// unknown (`None`) or older than the frames kept; the area around them all where they are many.
pub(crate) fn places(put: &[[c_int; 4]], past: &[Vec<[c_int; 4]>], missed: Option<usize>, size: [c_int; 2]) -> Vec<[c_int; 4]> {
    let [width, height] = size;
    let Some(missed) = missed.filter(|missed| *missed <= past.len()) else { return vec![[0, 0, width, height]] };
    let mut places = put.to_vec();
    places.extend(past.iter().take(missed).flatten());
    if places.len() > MOST {
        places = vec![places.iter().fold([width, height, 0, 0], |all, place| [all[0].min(place[0]), all[1].min(place[1]), all[2].max(place[2]), all[3].max(place[3])])];
    }
    places
}

library!(Asking, ["libX11.so.6", "libX11.so"], {
    fn XSetWindowBackgroundPixmap(*mut c_void, c_ulong, c_ulong) -> c_int;
    fn XQueryExtension(*mut c_void, *const c_char, *mut c_int, *mut c_int, *mut c_int) -> c_int;
});

/// Whether X display `x` offers DRI3.
///
/// # Safety
/// `x` is an open X display.
pub(crate) unsafe fn has_dri3(x: *mut c_void) -> bool {
    // SAFETY: as the caller promises.
    unsafe { offers(x, c"DRI3") }
}

/// Whether X display `x` offers the extension `name`.
///
/// # Safety
/// `x` is an open X display.
unsafe fn offers(x: *mut c_void, name: &std::ffi::CStr) -> bool {
    let mut numbers = [0; 3];
    // SAFETY: Xlib's documented call; it fills in three numbers.
    let asked = |lib: &Asking| unsafe { (lib.XQueryExtension)(x, name.as_ptr(), &mut numbers[0], &mut numbers[1], &mut numbers[2]) } != 0;
    Asking::open().as_ref().is_some_and(asked)
}

/// Has X leave a window's background alone (`None`): it fills nothing there before the window
/// is drawn, on mapping, uncovering or resizing.
///
/// # Safety
/// `x` is an open X display, and `id` one of its windows.
pub(crate) unsafe fn unfilled(x: *mut c_void, id: c_ulong) {
    // SAFETY: Xlib's documented call.
    Asking::open().iter().for_each(|lib| _ = unsafe { (lib.XSetWindowBackgroundPixmap)(x, id, 0) });
}

#[cfg(test)]
thread_local! {
    /// Tests: no device is to be had.
    pub(crate) static NO_DEVICE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests: this renderer is missing.
    pub(crate) static MISSING: std::cell::Cell<Option<Renderer>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each choice tries the renderers in the order the table above gives; `Cpu` none.
    #[test]
    fn each_choice_tries_the_renderers_in_its_order() {
        let (gl, vk) = (Renderer::OpenGL, Renderer::Vulkan);
        let chosen = [Renderer::Auto, Renderer::Gpu, Renderer::OpenGL, Renderer::Vulkan, Renderer::Direct3D11, Renderer::Direct3D12, Renderer::Cpu];
        let tried = chosen.map(|chosen| chosen.tried().collect::<Vec<_>>());
        assert_eq!(tried, [vec![gl, vk], vec![gl, vk], vec![gl, vk], vec![vk, gl], vec![gl, vk], vec![gl, vk], vec![]]);
        if std::env::var_os("TAUT_RENDERER").is_none() {
            let any = chosen.map(|chosen| asked(chosen).map(|asked| asked.any));
            assert_eq!(any, [Some(false), Some(true), Some(true), Some(true), Some(true), Some(true), None]);
        }
    }

    /// A back buffer gets the places put, those of the frames it missed, all of it where its age
    /// is unknown or too old, and the area around them where they are many.
    #[test]
    fn a_back_buffer_gets_the_places_it_lacks() {
        let size = [100, 80];
        let past = vec![vec![[0, 0, 10, 10]], vec![[20, 20, 30, 30]], vec![[40, 40, 50, 50]]];
        let put = [[5, 5, 6, 6]];
        assert_eq!(places(&put, &past, Some(0), size), [[5, 5, 6, 6]]);
        assert_eq!(places(&put, &past, Some(2), size), [[5, 5, 6, 6], [0, 0, 10, 10], [20, 20, 30, 30]]);
        assert_eq!(places(&put, &past, Some(4), size), [[0, 0, 100, 80]]);
        assert_eq!(places(&put, &past, None, size), [[0, 0, 100, 80]]);
        let many: Vec<[c_int; 4]> = (0..20).map(|n| [n, n, n + 1, n + 1]).collect();
        assert_eq!(places(&many, &past, Some(0), size), [[0, 0, 20, 20]]);
        let mut noted = Vec::new();
        [[0, 0, 10, 4], [0, 4, 10, 8], [2, 8, 10, 9]].into_iter().for_each(|place| note(&mut noted, place));
        assert_eq!(noted, [[0, 0, 10, 8], [2, 8, 10, 9]]);
    }
}
