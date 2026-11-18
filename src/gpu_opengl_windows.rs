//! Windows' OpenGL path (the feature `gpu`): a window's pixels, drawn by the CPU, shown through
//! WGL and an OpenGL 3.3 core context.
//!
//! - A paint's strips go into a texture holding the whole window (`glTexSubImage2D`): only what
//!   changed. The texture is copied to the back buffer, rows flipped (`glBlitFramebuffer`,
//!   unfiltered), and swapped whole in step with the screen (`WGL_EXT_swap_control`): WGL has
//!   no partial swap.
//! - One context shows every window, made current with each in turn.
//! - A robust context (`WGL_ARB_create_context_robustness`) tells a GPU reset.
//! - opengl32.dll is loaded from the system folder when a window first asks for OpenGL; the
//!   context is made on a hidden window of the device's own thread.
use crate::gpu_windows::{function_in, system_library};
use crate::windows::sys::{CreateWindowExW, DestroyWindow, GetModuleHandleW, HANDLE};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering::Relaxed;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(hwnd: HANDLE) -> HANDLE;
    fn ReleaseDC(hwnd: HANDLE, dc: HANDLE) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn ChoosePixelFormat(dc: HANDLE, wanted: *const PixelFormat) -> i32;
    fn DescribePixelFormat(dc: HANDLE, format: i32, size: u32, described: *mut PixelFormat) -> i32;
    fn GetPixelFormat(dc: HANDLE) -> i32;
    fn SetPixelFormat(dc: HANDLE, format: i32, wanted: *const PixelFormat) -> i32;
    fn SwapBuffers(dc: HANDLE) -> i32;
}

/// `PIXELFORMATDESCRIPTOR`: its flags, then its byte fields from `iPixelType` to `bReserved`,
/// then its three masks.
#[repr(C)]
#[derive(Clone, Copy)]
struct PixelFormat {
    size: u16,
    version: u16,
    flags: u32,
    bytes: [u8; 20],
    masks: [u32; 3],
}

/// Drawn to a window, by OpenGL, double-buffered, composited by DWM (`PFD_DRAW_TO_WINDOW`,
/// `PFD_SUPPORT_OPENGL`, `PFD_DOUBLEBUFFER`, `PFD_SUPPORT_COMPOSITION`): RGBA with 24 bits of
/// colour, no depth.
const WANTED: PixelFormat = PixelFormat { size: 40, version: 1, flags: 0x4 | 0x20 | 0x1 | 0x8000, bytes: [0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], masks: [0; 3] };

/// The WGL functions of opengl32.dll's that are called.
struct Wgl {
    create: unsafe extern "system" fn(HANDLE) -> HANDLE,
    delete: unsafe extern "system" fn(HANDLE) -> i32,
    current: unsafe extern "system" fn(HANDLE, HANDLE) -> i32,
    find: unsafe extern "system" fn(*const c_char) -> *const c_void,
}

impl Wgl {
    /// WGL's functions in opengl32.dll; nothing if one is missing.
    fn load(library: isize) -> Option<Wgl> {
        // (The library is loaded.)
        let found = |name: &CStr| Some(function_in(library, name)).filter(|found| *found != 0);
        let [create, delete, current, find] = [c"wglCreateContext", c"wglDeleteContext", c"wglMakeCurrent", c"wglGetProcAddress"].map(found);
        // SAFETY: each as wingdi.h declares it.
        unsafe {
            Some(Wgl {
                create: std::mem::transmute::<usize, unsafe extern "system" fn(HANDLE) -> HANDLE>(create?),
                delete: std::mem::transmute::<usize, unsafe extern "system" fn(HANDLE) -> i32>(delete?),
                current: std::mem::transmute::<usize, unsafe extern "system" fn(HANDLE, HANDLE) -> i32>(current?),
                find: std::mem::transmute::<usize, unsafe extern "system" fn(*const c_char) -> *const c_void>(find?),
            })
        }
    }
}

/// Declares the OpenGL functions called, loaded by name.
macro_rules! functions {
    ($($(#[$only:meta])? $name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)*) => {
        /// The OpenGL functions called.
        #[allow(non_snake_case)]
        struct Gl { $($(#[$only])? $name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)* }
        impl Gl {
            /// Each function through `find`; nothing if one is missing.
            fn load(find: &dyn Fn(&str) -> *const c_void) -> Option<Gl> {
                let found = |name: &str| Some(find(name)).filter(|found| !found.is_null());
                // SAFETY: each function is declared as glcorearb.h declares it.
                Some(Gl { $($(#[$only])? $name: unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn($($arg),*) $(-> $out)?>(found(concat!(stringify!($name), "\0"))?) },)* })
            }
        }
    };
}

functions! {
    glGetString: fn(u32) -> *const c_char;
    glGetError: fn() -> u32;
    glGenTextures: fn(i32, *mut u32);
    glDeleteTextures: fn(i32, *const u32);
    glBindTexture: fn(u32, u32);
    glTexParameteri: fn(u32, u32, i32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexSubImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glPixelStorei: fn(u32, i32);
    #[cfg(test)] glReadPixels: fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    glGenFramebuffers: fn(i32, *mut u32);
    glDeleteFramebuffers: fn(i32, *const u32);
    glBindFramebuffer: fn(u32, u32);
    glFramebufferTexture2D: fn(u32, u32, u32, u32, i32);
    glCheckFramebufferStatus: fn(u32) -> u32;
    glGenBuffers: fn(i32, *mut u32);
    glDeleteBuffers: fn(i32, *const u32);
    glBindBuffer: fn(u32, u32);
    glBufferData: fn(u32, isize, *const c_void, u32);
    glMapBufferRange: fn(u32, isize, isize, u32) -> *mut c_void;
    glUnmapBuffer: fn(u32) -> u8;
    glBlitFramebuffer: fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32);
}

// From glcorearb.h and wglext.h.
const TEXTURE_2D: u32 = 0x0DE1;
const RGBA8: i32 = 0x8058;
const BGRA: u32 = 0x80E1;
/// `GL_UNSIGNED_INT_8_8_8_8_REV`: with `GL_BGRA`, the bytes of `0xAARRGGBB` in memory, which
/// drivers copy as they are (with `GL_UNSIGNED_BYTE` they may convert each pixel).
const PACKED: u32 = 0x8367;
const MIN_FILTER: u32 = 0x2801;
const MAG_FILTER: u32 = 0x2800;
const NEAREST: u32 = 0x2600;
const READ_FRAMEBUFFER: u32 = 0x8CA8;
const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const COLOR_ATTACHMENT0: u32 = 0x8CE0;
const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const COLOR_BUFFER_BIT: u32 = 0x4000;
const RENDERER: u32 = 0x1F01;
const UNPACK_ROW_LENGTH: u32 = 0x0CF2;
const PIXEL_UNPACK_BUFFER: u32 = 0x88EC;
/// Uploads of more bytes than this go through a pixel buffer.
const LARGE: isize = 1 << 18;
const STREAM_DRAW: u32 = 0x88E0;
/// `GL_MAP_WRITE_BIT | GL_MAP_INVALIDATE_BUFFER_BIT`.
const MAP_TO_WRITE: u32 = 0x2 | 0x8;
/// `WGL_CONTEXT_MAJOR_VERSION_ARB` 3, `MINOR` 3, `PROFILE_MASK` core; and robust
/// (`WGL_CONTEXT_FLAGS_ARB` robust access, losing the context on a reset).
const CORE: [i32; 6] = [0x2091, 3, 0x2092, 3, 0x9126, 1];
const ROBUST: [i32; 4] = [0x2094, 0x4, 0x8256, 0x8252];

/// What a context's objects are let go of with: its functions, WGL's, and the context.
#[derive(Clone)]
struct Owner {
    gl: Arc<Gl>,
    wgl: Arc<Wgl>,
    context: HANDLE,
    /// The window device context the context is current with, on the windows' thread.
    current: Arc<AtomicIsize>,
}

impl Owner {
    /// Makes the context current with `dc`, if it is not. Says whether it is.
    fn with(&self, dc: HANDLE) -> bool {
        if self.current.load(Relaxed) == dc {
            return true;
        }
        // SAFETY: the context is alive; the device context is a live window's.
        let made = unsafe { (self.wgl.current)(dc, self.context) } != 0;
        self.current.store(if made { dc } else { 0 }, Relaxed);
        made
    }
}

/// The OpenGL context a program's windows are shown with.
pub(crate) struct Device {
    owner: Owner,
    /// The pixel format every window is given.
    format: i32,
    /// `wglSwapIntervalEXT`, and `glGetGraphicsResetStatusARB` of a robust context.
    interval: Option<unsafe extern "system" fn(i32) -> i32>,
    reset: Option<unsafe extern "system" fn() -> u32>,
    /// A reset was told, or a swap failed.
    lost: Cell<bool>,
}

impl Device {
    /// An OpenGL 3.3 core context on a GPU (not GDI's software OpenGL, not a software
    /// rasterizer); nothing where there is none. Made current on no thread.
    pub(crate) fn gpu() -> Option<Device> {
        let library = system_library("opengl32.dll");
        let wgl = Some(library).filter(|library| *library != 0).and_then(Wgl::load)?;
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a popup window of the system's static class, never shown; let go of below.
        let hidden = unsafe { CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0x8000_0000, 0, 0, 1, 1, 0, 0, GetModuleHandleW(null()), null_mut()) };
        // SAFETY: the window is alive.
        let dc = unsafe { GetDC(hidden) };
        let made = Device::on(dc, library, wgl);
        // SAFETY: as made above, on this thread.
        unsafe {
            ReleaseDC(hidden, dc);
            DestroyWindow(hidden);
        }
        made
    }

    /// The context, made on device context `dc` of a hidden window.
    fn on(dc: HANDLE, library: isize, wgl: Wgl) -> Option<Device> {
        // SAFETY: the device context is alive; the format is described into room for it.
        let format = unsafe {
            let format = ChoosePixelFormat(dc, &WANTED);
            let mut described = WANTED;
            DescribePixelFormat(dc, format, 40, &mut described);
            // GDI's own (`PFD_GENERIC_FORMAT`, not `PFD_GENERIC_ACCELERATED`) draws on the CPU.
            if format == 0 || described.flags & 0x1040 == 0x40 || SetPixelFormat(dc, format, &WANTED) == 0 {
                return None;
            }
            format
        };
        // SAFETY: a first, plain context, current to find the extensions' functions; let go of
        // once the real one is made.
        let context = unsafe {
            let first = (wgl.create)(dc);
            if first == 0 || (wgl.current)(dc, first) == 0 {
                (wgl.delete)(first);
                return None;
            }
            let context = Device::core(&wgl, dc);
            (wgl.current)(dc, context);
            (wgl.delete)(first);
            context
        };
        if context == 0 {
            return None;
        }
        Device::loaded(library, wgl, context, format)
    }

    /// An OpenGL 3.3 core context, robust where the driver offers it, made with the current
    /// one's `wglCreateContextAttribsARB`; 0 where none is.
    ///
    /// # Safety
    /// A context is current on `dc`.
    unsafe fn core(wgl: &Wgl, dc: HANDLE) -> HANDLE {
        type Make = unsafe extern "system" fn(HANDLE, HANDLE, *const i32) -> HANDLE;
        // SAFETY: as the caller promises; the function as wglext.h declares it.
        unsafe {
            let make = (wgl.find)(c"wglCreateContextAttribsARB".as_ptr());
            if matches!(make as isize, -1..=3) {
                return 0;
            }
            let make = std::mem::transmute::<*const c_void, Make>(make);
            let robust = [CORE.as_slice(), &ROBUST, &[0]].concat();
            let made = make(dc, 0, robust.as_ptr());
            if made != 0 { made } else { make(dc, 0, [CORE.as_slice(), &[0]].concat().as_ptr()) }
        }
    }

    /// The device, with its functions found while the context is current; current on no thread
    /// after. Nothing where a function is missing or it draws on the CPU.
    fn loaded(library: isize, wgl: Wgl, context: HANDLE, format: i32) -> Option<Device> {
        let find = |name: &str| {
            // SAFETY: the context is current; the names end in a zero. Drivers give nothing (or
            // 1, 2, 3 or -1) for OpenGL 1.1's own functions, which opengl32.dll has.
            let found = unsafe { (wgl.find)(name.as_ptr().cast()) };
            if matches!(found as isize, -1..=3) { function_in(library, CStr::from_bytes_until_nul(name.as_bytes()).unwrap_or_default()) as *const c_void } else { found }
        };
        let gl = Gl::load(&find).filter(|gl| !software(gl));
        // SAFETY: as declared in wglext.h and glcorearb.h.
        let interval = Some(find("wglSwapIntervalEXT\0")).filter(|found| !found.is_null()).map(|found| unsafe { std::mem::transmute(found) });
        let reset = Some(find("glGetGraphicsResetStatusARB\0")).filter(|found| !found.is_null()).map(|found| unsafe { std::mem::transmute(found) });
        let wgl = Arc::new(wgl);
        // SAFETY: let go of on this thread, to be made current on the windows' thread.
        unsafe { (wgl.current)(0, 0) };
        let Some(gl) = gl else {
            // SAFETY: nothing else holds the context.
            unsafe { (wgl.delete)(context) };
            return None;
        };
        let owner = Owner { gl: Arc::new(gl), wgl, context, current: Arc::new(AtomicIsize::new(0)) };
        Some(Device { owner, format, interval, reset, lost: Cell::new(false) })
    }

    /// Whether the GPU was reset (a robust context says so), or a swap failed.
    pub(crate) fn removed(&self) -> bool {
        // SAFETY: the context is current on this thread (a window was shown with it).
        let reset = self.reset.is_some_and(|reset| self.owner.current.load(Relaxed) != 0 && unsafe { reset() } != 0);
        self.lost.get() || reset
    }
}

/// Whether the current context draws on the CPU (`GL_RENDERER`): Mesa's llvmpipe or softpipe,
/// SwiftShader, or Microsoft's adapters for no GPU.
fn software(gl: &Gl) -> bool {
    #[cfg(test)]
    if crate::gpu_windows::tests::faked().software {
        return false;
    }
    // SAFETY: a context is current; the string is the driver's, read at once.
    let renderer = unsafe {
        let text = (gl.glGetString)(RENDERER);
        if text.is_null() { String::new() } else { CStr::from_ptr(text).to_string_lossy().into_owned() }
    };
    #[cfg(test)]
    println!("OpenGL on {renderer}");
    ["llvmpipe", "softpipe", "SwiftShader", "GDI Generic", "Basic Render"].iter().any(|soft| renderer.contains(soft))
}

// SAFETY: the context is current on no thread while it is handed over: made on a thread of its
// own, then handed whole to the windows' thread.
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the context is this device's alone; current nowhere after.
        unsafe {
            (self.owner.wgl.current)(0, 0);
            (self.owner.wgl.delete)(self.owner.context);
        }
    }
}

/// A window's device context, and the texture holding all of its pixels, with the framebuffer
/// it is read through and the buffer uploads go through.
pub(crate) struct Shown {
    owner: Owner,
    hwnd: HANDLE,
    dc: HANDLE,
    texture: u32,
    framebuffer: u32,
    buffer: u32,
    size: [i32; 2],
}

impl Shown {
    /// Window `hwnd` given the device's pixel format and swapped in step with the screen; nothing
    /// where it can't be.
    pub(crate) fn new(device: &Device, hwnd: HANDLE) -> Option<Shown> {
        // SAFETY: the window is alive; its device context is kept until the window is let go of.
        let dc = unsafe { GetDC(hwnd) };
        let mut shown = Shown { owner: device.owner.clone(), hwnd, dc, texture: 0, framebuffer: 0, buffer: 0, size: [0; 2] };
        // SAFETY: as above: a window has one pixel format for its life.
        let formatted = dc != 0 && unsafe { GetPixelFormat(dc) == device.format || SetPixelFormat(dc, device.format, &WANTED) != 0 };
        if !formatted || !shown.owner.with(dc) {
            return None;
        }
        let gl = &shown.owner.gl;
        // SAFETY: the context is current.
        unsafe {
            if let Some(interval) = device.interval {
                interval(1);
            }
            (gl.glGenTextures)(1, &mut shown.texture);
            (gl.glGenFramebuffers)(1, &mut shown.framebuffer);
            (gl.glGenBuffers)(1, &mut shown.buffer);
            (gl.glBindTexture)(TEXTURE_2D, shown.texture);
            (gl.glTexParameteri)(TEXTURE_2D, MIN_FILTER, NEAREST as i32);
            (gl.glTexParameteri)(TEXTURE_2D, MAG_FILTER, NEAREST as i32);
        }
        Some(shown)
    }

    /// Makes the texture `size` pixels where it is not. Says whether it was remade (the window is
    /// then to be drawn whole); nothing if that failed.
    pub(crate) fn fit(&mut self, device: &Device, size: [i32; 2]) -> Option<bool> {
        if size == self.size {
            return Some(false);
        }
        let gl = &self.owner.gl;
        self.size = [0; 2];
        if !self.owner.with(self.dc) {
            device.lost.set(true);
            return None;
        }
        // SAFETY: the context is current; the texture and framebuffer are this window's.
        let complete = unsafe {
            (gl.glBindTexture)(TEXTURE_2D, self.texture);
            (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA8, size[0], size[1], 0, BGRA, PACKED, null());
            (gl.glBindFramebuffer)(READ_FRAMEBUFFER, self.framebuffer);
            (gl.glFramebufferTexture2D)(READ_FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, self.texture, 0);
            (gl.glCheckFramebufferStatus)(READ_FRAMEBUFFER) == FRAMEBUFFER_COMPLETE && (gl.glGetError)() == 0
        };
        self.size = size;
        complete.then_some(true)
    }

    /// Puts `size` pixels into the texture at `at` (cut to it), read from `first`, rows `pitch`
    /// pixels apart. More than 256 KB go through a fresh pixel buffer, which the GPU copies on:
    /// from the program's memory NVIDIA's driver takes three times as long (15 ms for 4K, 5.5
    /// ms this way); fewer go directly, which costs less than a buffer.
    ///
    /// # Safety
    /// The pixels read lie at `first`: as many rows as given, each as wide.
    pub(crate) unsafe fn upload(&mut self, device: &Device, at: [i32; 2], size: [i32; 2], first: *const u32, pitch: usize) {
        let [right, bottom] = [(at[0] + size[0]).min(self.size[0]), (at[1] + size[1]).min(self.size[1])];
        if at[0] < 0 || at[1] < 0 || right <= at[0] || bottom <= at[1] || !self.owner.with(self.dc) {
            return;
        }
        let _ = device;
        let (gl, [width, height]) = (&self.owner.gl, [right - at[0], bottom - at[1]]);
        let bytes = width as isize * height as isize * 4;
        if bytes <= LARGE {
            // SAFETY: the context is current; the rows read lie where the caller promises, the
            // place in the texture (its rows from the top, as the window's).
            unsafe {
                (gl.glBindTexture)(TEXTURE_2D, self.texture);
                (gl.glPixelStorei)(UNPACK_ROW_LENGTH, pitch as i32);
                (gl.glTexSubImage2D)(TEXTURE_2D, 0, at[0], at[1], width, height, BGRA, PACKED, first.cast());
                (gl.glPixelStorei)(UNPACK_ROW_LENGTH, 0);
            }
            return;
        }
        // SAFETY: as above; the buffer is this window's, as large as the pixels copied into it.
        unsafe {
            (gl.glBindBuffer)(PIXEL_UNPACK_BUFFER, self.buffer);
            (gl.glBufferData)(PIXEL_UNPACK_BUFFER, bytes, null(), STREAM_DRAW);
            let there = (gl.glMapBufferRange)(PIXEL_UNPACK_BUFFER, 0, bytes, MAP_TO_WRITE).cast::<u32>();
            if !there.is_null() {
                let width = width as usize;
                (0..height as usize).for_each(|row| std::ptr::copy_nonoverlapping(first.add(row * pitch), there.add(row * width), width));
                (gl.glUnmapBuffer)(PIXEL_UNPACK_BUFFER);
                (gl.glBindTexture)(TEXTURE_2D, self.texture);
                (gl.glTexSubImage2D)(TEXTURE_2D, 0, at[0], at[1], width as i32, height, BGRA, PACKED, null());
            }
            (gl.glBindBuffer)(PIXEL_UNPACK_BUFFER, 0);
        }
    }

    /// Draws a recorded paint into the texture, inside `area` (left, top, right, bottom), with
    /// what `drawing` holds: made here the first time (nothing, if it can't be). Says whether it
    /// could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, drawing: &mut Option<Option<crate::gpu_draw_opengl::Pipeline>>, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        if self.size[0] <= 0 || !self.owner.with(self.dc) {
            return false;
        }
        let made = drawing.get_or_insert_with(|| crate::gpu_draw_opengl::Pipeline::new(&mut crate::gpu_draw_windows::wgl_function, true));
        made.as_mut().is_some_and(|pipeline| pipeline.draw(self.texture, self.size, list, area))
    }

    /// Copies the texture to the back buffer, its rows from the top, and swaps it, at the
    /// screen's next refresh. Says whether that worked.
    pub(crate) fn present(&mut self, device: &Device) -> bool {
        if self.size[0] <= 0 || !self.owner.with(self.dc) {
            return false;
        }
        let ([width, height], gl) = (self.size, &self.owner.gl);
        // SAFETY: the context is current; the framebuffers are the texture's and the window's.
        unsafe {
            (gl.glBindFramebuffer)(READ_FRAMEBUFFER, self.framebuffer);
            (gl.glBindFramebuffer)(DRAW_FRAMEBUFFER, 0);
            (gl.glBlitFramebuffer)(0, 0, width, height, 0, height, width, 0, COLOR_BUFFER_BIT, NEAREST);
        }
        #[cfg(test)]
        tests::keep(self);
        // SAFETY: the window's device context, of the context's pixel format.
        let swapped = unsafe { SwapBuffers(self.dc) } != 0;
        if !swapped {
            device.lost.set(true);
        }
        swapped && !device.removed()
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        let gl = &self.owner.gl;
        // SAFETY: the texture and framebuffer are this window's, let go of while the context is
        // current with its device context, which is then given back.
        unsafe {
            if self.owner.with(self.dc) {
                (gl.glDeleteFramebuffers)(1, &self.framebuffer);
                (gl.glDeleteBuffers)(1, &self.buffer);
                (gl.glDeleteTextures)(1, &self.texture);
            }
            (self.owner.wgl.current)(0, 0);
            self.owner.current.store(0, Relaxed);
            ReleaseDC(self.hwnd, self.dc);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Keeps the back buffer about to be swapped, if a test asked: read with `glReadPixels`,
    /// rows from the bottom, turned to rows from the top.
    pub(crate) fn keep(shown: &Shown) {
        if !crate::gpu_windows::tests::faked().keep {
            return;
        }
        let ([width, height], gl) = (shown.size.map(|side| side as usize), &shown.owner.gl);
        let mut read = vec![0u32; width * height];
        unsafe {
            (gl.glBindFramebuffer)(READ_FRAMEBUFFER, 0);
            (gl.glReadPixels)(0, 0, width as i32, height as i32, BGRA, PACKED, read.as_mut_ptr().cast());
        }
        let pixels = read.chunks(width).rev().flatten().copied().collect();
        crate::gpu_windows::tests::kept(shown.hwnd, width, pixels, &[]);
    }
}
