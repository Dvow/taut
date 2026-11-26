//! What windows shown through EGL and OpenGL ES 3 share on X11 and Android (the feature `gpu`):
//! the display, one context, the program drawing a window's texture texel for pixel, and each
//! window's surface and texture. `gpu_opengl_x11` and `gpu_opengl_android` add their display,
//! surfaces and how a frame is drawn; what they keep of their own is `Gpu::system` and
//! `Shows::system`.
//!
//! - A context made robust where EGL can (`EGL_EXT_create_context_robustness`) says when the
//!   device was reset: then it is lost.
//! - Swaps name what changed where the display can (`EGL_KHR_swap_buffers_with_damage`).
//! - libEGL and libGLESv3 are linked on Android (every Android has them); on Linux, libEGL and
//!   libGLESv2 are opened by name when a window first asks for the GPU, so a program builds and
//!   starts without them. Once opened they stay: drivers are not safe to unload.
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr::{null, null_mut};

/// Declares a table of a system library's functions: linked to it on Android (`$link`), opened
/// at run time by its file's name elsewhere (the first of `$files` there is) and never closed.
/// The table is laid out as the list of their addresses: one loop finds them.
macro_rules! system_library {
    ($set:ident, $link:literal, [$($file:literal),+], { $(fn $name:ident($($argument:ty),*) $(-> $result:ty)?;)* }) => {
        /// The library's functions.
        #[allow(non_snake_case)] // (The library's own names.)
        #[repr(C)]
        pub(crate) struct $set {
            $(pub(crate) $name: unsafe extern "C" fn($($argument),*) $(-> $result)?,)*
        }
        #[cfg(target_os = "android")]
        #[link(name = $link)]
        unsafe extern "C" {
            $(fn $name($(_: $argument),*) $(-> $result)?;)*
        }
        impl $set {
            /// The functions, linked.
            #[cfg(target_os = "android")]
            pub(crate) fn open() -> Option<$set> {
                Some($set { $($name,)* })
            }

            /// The functions, from the library opened; nothing where it or one of them is
            /// missing.
            #[cfg(not(target_os = "android"))]
            pub(crate) fn open() -> Option<$set> {
                const COUNT: usize = [$(stringify!($name),)*].len();
                let mut found = [std::ptr::null_mut::<c_void>(); COUNT];
                $crate::gpu_egl::opened(&[$(concat!($file, "\0")),+], &[$(concat!(stringify!($name), "\0"),)*], &mut found)?;
                // SAFETY: the table is `repr(C)`, a function pointer for each name in order, each
                // declared as its header declares it; every address found is one (not null).
                Some(unsafe { std::mem::transmute::<[*mut c_void; COUNT], $set>(found) })
            }
        }
    };
}
#[cfg(not(target_os = "android"))]
pub(crate) use system_library;

/// Opens the first of `files` there is (each name ending in a zero), for good, and finds `names`
/// in it (each ending in a zero) into `found`. Nothing where it or one of them is missing.
#[cfg(not(target_os = "android"))]
pub(crate) fn opened(files: &[&str], names: &[&str], found: &mut [*mut c_void]) -> Option<()> {
    // SAFETY: each name ends in a zero; the library is never closed.
    let library = files.iter().map(|name| unsafe { crate::dynamic_library::dlopen(name.as_ptr().cast(), crate::dynamic_library::NOW) }).find(|library| !library.is_null())?;
    names.iter().zip(found).try_for_each(|(name, slot)| crate::dynamic_library::symbol(library, name).map(|address| *slot = address))
}

system_library!(Egl, "EGL", ["libEGL.so.1", "libEGL.so"], {
    fn eglGetProcAddress(*const c_char) -> *mut c_void;
    fn eglInitialize(Handle, *mut c_int, *mut c_int) -> c_uint;
    fn eglTerminate(Handle) -> c_uint;
    fn eglQueryString(Handle, c_int) -> *const c_char;
    fn eglBindAPI(c_uint) -> c_uint;
    fn eglChooseConfig(Handle, *const c_int, *mut Handle, c_int, *mut c_int) -> c_uint;
    fn eglGetConfigAttrib(Handle, Handle, c_int, *mut c_int) -> c_uint;
    fn eglCreateContext(Handle, Handle, Handle, *const c_int) -> Handle;
    fn eglDestroyContext(Handle, Handle) -> c_uint;
    fn eglDestroySurface(Handle, Handle) -> c_uint;
    fn eglMakeCurrent(Handle, Handle, Handle, Handle) -> c_uint;
    fn eglSwapBuffers(Handle, Handle) -> c_uint;
    fn eglSwapInterval(Handle, c_int) -> c_uint;
    fn eglReleaseThread() -> c_uint;
});

system_library!(Gles, "GLESv3", ["libGLESv2.so.2", "libGLESv2.so"], {
    fn glGetError() -> c_uint;
    fn glGetString(c_uint) -> *const c_char;
    fn glGetIntegerv(c_uint, *mut c_int);
    fn glDisable(c_uint);
    fn glGenTextures(c_int, *mut c_uint);
    fn glDeleteTextures(c_int, *const c_uint);
    fn glBindTexture(c_uint, c_uint);
    fn glTexParameteri(c_uint, c_uint, c_int);
    fn glTexStorage2D(c_uint, c_int, c_uint, c_int, c_int);
    fn glTexSubImage2D(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
    fn glPixelStorei(c_uint, c_int);
    fn glCreateShader(c_uint) -> c_uint;
    fn glShaderSource(c_uint, c_int, *const *const c_char, *const c_int);
    fn glCompileShader(c_uint);
    fn glGetShaderiv(c_uint, c_uint, *mut c_int);
    fn glDeleteShader(c_uint);
    fn glCreateProgram() -> c_uint;
    fn glAttachShader(c_uint, c_uint);
    fn glLinkProgram(c_uint);
    fn glGetProgramiv(c_uint, c_uint, *mut c_int);
    fn glUseProgram(c_uint);
    fn glGetUniformLocation(c_uint, *const c_char) -> c_int;
    fn glUniform1i(c_int, c_int);
    fn glViewport(c_int, c_int, c_int, c_int);
    fn glDrawArrays(c_uint, c_int, c_int);
});

/// EGL's handles: display, config, context, surface.
pub(crate) type Handle = *mut c_void;

// From EGL's and OpenGL ES 3's headers (egl.h, eglext.h, gl3.h).
const EXTENSIONS: c_int = 0x3055;
pub(crate) const SURFACE_TYPE: c_int = 0x3033;
pub(crate) const WINDOW_BIT: c_int = 0x0004;
pub(crate) const RENDERABLE_TYPE: c_int = 0x3040;
pub(crate) const OPENGL_ES3_BIT: c_int = 0x0040;
pub(crate) const RED_SIZE: c_int = 0x3024;
pub(crate) const GREEN_SIZE: c_int = 0x3023;
pub(crate) const BLUE_SIZE: c_int = 0x3022;
const NATIVE_VISUAL_ID: c_int = 0x302E;
pub(crate) const NONE: c_int = 0x3038;
const OPENGL_ES_API: c_uint = 0x30A0;
const CONTEXT_MAJOR_VERSION: c_int = 0x3098;
const CONTEXT_MINOR_VERSION: c_int = 0x30FB;
const RESET_NOTIFICATION_STRATEGY: c_int = 0x3138;
const LOSE_CONTEXT_ON_RESET: c_int = 0x31BF;
pub(crate) const TEXTURE_2D: c_uint = 0x0DE1;
pub(crate) const RGBA: c_uint = 0x1908;
const RGBA8: c_uint = 0x8058;
pub(crate) const UNSIGNED_BYTE: c_uint = 0x1401;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
pub(crate) const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
pub(crate) const UNPACK_SKIP_PIXELS: c_uint = 0x0CF4;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;
pub(crate) const TRIANGLES: c_uint = 0x0004;
pub(crate) const SCISSOR_TEST: c_uint = 0x0C11;
pub(crate) const DITHER: c_uint = 0x0BD0;
const RENDERER: c_uint = 0x1F01;
const MAX_TEXTURE_SIZE: c_uint = 0x0D33;

/// One triangle over the whole viewport.
const SPREAD: &CStr = c"#version 300 es
void main() {
    gl_Position = vec4(float((gl_VertexID & 1) << 2) - 1.0, float((gl_VertexID & 2) << 1) - 1.0, 0.0, 1.0);
}
";

/// What GL_RENDERER says of a device that is the CPU at work: Mesa's, Google's and Windows'
/// software rasterizers.
const SOFTWARE: [&str; 6] = ["llvmpipe", "softpipe", "SwiftShader", "Software Rasterizer", "Basic Render Driver", "SWR"];

/// `eglSwapBuffersWithDamageKHR` (or `EXT`): shows a surface's back buffer, naming the places
/// that changed (each left, bottom, width, height).
type SwapWithDamage = unsafe extern "C" fn(Handle, Handle, *const c_int, c_int) -> c_uint;

/// `glGetGraphicsResetStatus` (or `EXT`): not 0 once the device was lost.
type ResetStatus = unsafe extern "C" fn() -> c_uint;

/// The GPU windows are shown through: EGL's display, one OpenGL ES 3 context, the program
/// drawing a window's texture, and what the system keeps (`system`).
pub(crate) struct Gpu<S> {
    pub(crate) egl: Egl,
    pub(crate) gl: Gles,
    pub(crate) display: Handle,
    pub(crate) config: Handle,
    context: Handle,
    /// The surface current now (null: none).
    current: Handle,
    /// The program, and where its `height` is; the widest or tallest texture the device makes
    /// (0 until the program is made).
    pub(crate) program: c_uint,
    pub(crate) height: c_int,
    most: c_int,
    /// What the display offers: swaps naming their damage, and the reset status (where the
    /// context was made robust).
    swap: Option<SwapWithDamage>,
    reset: Option<ResetStatus>,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// The display is never terminated: Mesa's d3d12 driver (WSL's) crashes at thread exit
    /// after it.
    lasting: bool,
    /// Where the GPU draws windows too, what it records to, and what it draws with: not yet
    /// tried, or what that gave (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<Option<Box<crate::gpu_draw_opengl::Pipeline>>>,
    /// What the system keeps: its own functions, and more.
    pub(crate) system: S,
}

/// A window shown through the GPU: its surface, its texture and that texture's size in pixels,
/// and what the system keeps of it (`system`).
pub(crate) struct Shows<W> {
    pub(crate) surface: Handle,
    pub(crate) texture: c_uint,
    pub(crate) size: [c_int; 2],
    /// What the system keeps of the window.
    pub(crate) system: W,
}

impl<S> Gpu<S> {
    /// EGL's `display` (of `egl` and `gl`) initialized, with a config of `wanted` (a list ending
    /// in `NONE`) whose native visual is `visual`, and an OpenGL ES 3.0 context. Nothing where any
    /// of it is missing.
    pub(crate) fn make(egl: Egl, gl: Gles, display: Handle, (wanted, visual): (&[c_int], usize), system: S) -> Option<Gpu<S>> {
        let mut gpu = Gpu {
            egl,
            gl,
            display,
            config: null_mut(),
            context: null_mut(),
            current: null_mut(),
            program: 0,
            height: -1,
            most: 0,
            swap: None,
            reset: None,
            lost: false,
            lasting: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
            system,
        };
        // SAFETY: the display is EGL's; the version is not asked for.
        if unsafe { (gpu.egl.eglInitialize)(display, null_mut(), null_mut()) } == 0 {
            return None;
        }
        gpu.config = gpu.config(wanted, visual)?;
        gpu.context = gpu.context()?;
        gpu.learn();
        Some(gpu)
    }

    /// The config of `wanted` (a list ending in `NONE`) drawing in native visual `visual`.
    fn config(&self, wanted: &[c_int], visual: usize) -> Option<Handle> {
        let (mut configs, mut found) = ([null_mut(); 64], 0);
        // SAFETY: EGL fills in at most 64 configs and their count.
        unsafe { (self.egl.eglChooseConfig)(self.display, wanted.as_ptr(), configs.as_mut_ptr(), 64, &mut found) };
        let chosen = &configs[..found.clamp(0, 64) as usize];
        chosen.iter().find(|config| self.attribute(**config, NATIVE_VISUAL_ID) as usize == visual).copied()
    }

    /// One of a config's attributes (0 where EGL has none).
    fn attribute(&self, config: Handle, name: c_int) -> c_int {
        let mut value = 0;
        // SAFETY: the config is the display's; EGL fills in one number.
        unsafe { (self.egl.eglGetConfigAttrib)(self.display, config, name, &mut value) };
        value
    }

    /// What the display says of `name` (`EGL_EXTENSIONS`, `EGL_VENDOR`): empty where nothing.
    pub(crate) fn said(&self, name: c_int) -> &[u8] {
        // SAFETY: EGL's own string, ending in a zero, alive while the display is.
        let said = unsafe { (self.egl.eglQueryString)(self.display, name) };
        if said.is_null() { &[] } else { unsafe { CStr::from_ptr(said) }.to_bytes() }
    }

    /// Whether the display offers an extension.
    pub(crate) fn offers(&self, name: &str) -> bool {
        self.said(EXTENSIONS).split(|c| *c == b' ').any(|offered| offered == name.as_bytes())
    }

    /// A function by name, from EGL; nothing where there is none.
    pub(crate) fn function(&self, name: &CStr) -> Option<*mut c_void> {
        // SAFETY: the name ends in a zero.
        let found = unsafe { (self.egl.eglGetProcAddress)(name.as_ptr()) };
        (!found.is_null()).then_some(found)
    }

    /// An OpenGL ES 3.0 context: told of device loss where the display can tell, and then the
    /// reset status is kept to ask.
    fn context(&mut self) -> Option<Handle> {
        let robust = [CONTEXT_MAJOR_VERSION, 3, CONTEXT_MINOR_VERSION, 0, RESET_NOTIFICATION_STRATEGY, LOSE_CONTEXT_ON_RESET, NONE];
        let plain = [CONTEXT_MAJOR_VERSION, 3, CONTEXT_MINOR_VERSION, 0, NONE];
        // SAFETY: EGL's documented call.
        unsafe { (self.egl.eglBindAPI)(OPENGL_ES_API) };
        let mut made = null_mut();
        if self.offers("EGL_EXT_create_context_robustness") {
            made = self.made(&robust);
            let found = [c"glGetGraphicsResetStatusEXT", c"glGetGraphicsResetStatus"].into_iter().find_map(|name| self.function(name));
            // SAFETY: the function is declared as gl2ext.h declares it.
            self.reset = found.map(|f| unsafe { std::mem::transmute::<*mut c_void, ResetStatus>(f) });
        }
        if made.is_null() {
            (made, self.reset) = (self.made(&plain), None);
        }
        (!made.is_null()).then_some(made)
    }

    /// A context made as `asked` says (a list ending in `NONE`); null where it can't be.
    fn made(&self, asked: &[c_int]) -> Handle {
        // SAFETY: EGL's documented call; the list ends in `NONE`.
        unsafe { (self.egl.eglCreateContext)(self.display, self.config, null_mut(), asked.as_ptr()) }
    }

    /// Notes whether the display's swaps name their damage.
    fn learn(&mut self) {
        let damage = [("EGL_KHR_swap_buffers_with_damage", c"eglSwapBuffersWithDamageKHR"), ("EGL_EXT_swap_buffers_with_damage", c"eglSwapBuffersWithDamageEXT")];
        let swap = damage.into_iter().find(|(extension, _)| self.offers(extension));
        // SAFETY: the function is declared as eglext.h declares it.
        self.swap = swap.and_then(|(_, name)| self.function(name)).map(|f| unsafe { std::mem::transmute::<*mut c_void, SwapWithDamage>(f) });
    }

    /// What shows a window through `surface` (handed over), made current, swapping in step with
    /// the screen; `system` being what the system keeps of it. Nothing where it can't be.
    pub(crate) fn shows<W>(&mut self, surface: Handle, system: W) -> Option<Shows<W>> {
        let shows = Shows { surface, texture: 0, size: [0; 2], system };
        // SAFETY: the surface is current: the interval is its own.
        if self.current(surface) && unsafe { (self.egl.eglSwapInterval)(self.display, 1) } != 0 {
            return Some(shows);
        }
        self.forget(shows);
        None
    }

    /// Makes `surface` the one drawn to, unless it is. Says whether it is; if it can't be, the
    /// device is lost.
    pub(crate) fn current(&mut self, surface: Handle) -> bool {
        // SAFETY: the surface and context are the display's.
        if self.current != surface && unsafe { (self.egl.eglMakeCurrent)(self.display, surface, surface, self.context) } == 0 {
            (self.lost, self.current) = (true, null_mut());
            return false;
        }
        self.current = surface;
        true
    }

    /// The device's name, from OpenGL ES (as "llvmpipe (LLVM 21.1.8, 256 bits)").
    pub(crate) fn name(&self) -> String {
        // SAFETY: a context is current; the string is OpenGL's, ending in a zero.
        let name = unsafe { (self.gl.glGetString)(RENDERER) };
        if name.is_null() { String::new() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() }
    }

    /// Whether the device is the CPU at work.
    pub(crate) fn software(&self) -> bool {
        let name = self.name();
        SOFTWARE.iter().any(|software| name.contains(software))
    }

    /// Compiles the program drawing a texture with `texel` (the fragment shader: its uniform
    /// `height`, the target's height in pixels), makes it the one used, and notes the largest
    /// texture; the display is never terminated where the device is Mesa's d3d12 (see `lasting`).
    /// Says whether it could.
    pub(crate) fn program(&mut self, texel: &CStr) -> bool {
        self.lasting = self.name().starts_with("D3D12");
        let gl = &self.gl;
        let shaders = [(VERTEX_SHADER, SPREAD), (FRAGMENT_SHADER, texel)].map(|(kind, source)| self.shader(kind, source));
        let mut linked = 0;
        // SAFETY: OpenGL ES's documented calls on the current context; the uniform's name ends
        // in a zero.
        unsafe {
            self.program = (gl.glCreateProgram)();
            shaders.iter().for_each(|shader| (gl.glAttachShader)(self.program, *shader));
            (gl.glLinkProgram)(self.program);
            (gl.glGetProgramiv)(self.program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| (gl.glDeleteShader)(*shader));
            (gl.glUseProgram)(self.program);
            self.height = (gl.glGetUniformLocation)(self.program, c"height".as_ptr());
            (gl.glGetIntegerv)(MAX_TEXTURE_SIZE, &mut self.most);
        }
        linked != 0 && shaders.iter().all(|shader| *shader != 0)
    }

    /// A compiled shader (0 where it didn't compile).
    fn shader(&self, kind: c_uint, source: &CStr) -> c_uint {
        let (gl, mut compiled) = (&self.gl, 0);
        // SAFETY: OpenGL ES's documented calls on the current context; the source ends in a zero.
        unsafe {
            let shader = (gl.glCreateShader)(kind);
            (gl.glShaderSource)(shader, 1, &source.as_ptr(), null());
            (gl.glCompileShader)(shader);
            (gl.glGetShaderiv)(shader, COMPILE_STATUS, &mut compiled);
            if compiled != 0 { shader } else { 0 }
        }
    }

    /// Gives a window a new texture `size` pixels big, unfiltered (nothing in it yet). Says whether
    /// it has it: where the device makes none that big, the window is better shown by the CPU.
    pub(crate) fn texture<W>(&mut self, shows: &mut Shows<W>, size: [c_int; 2]) -> bool {
        let small = self.most == 0 || size.iter().all(|side| (1..=self.most).contains(side));
        if !small || !self.current(shows.surface) {
            return false;
        }
        let gl = &self.gl;
        // SAFETY: OpenGL ES's documented calls on the current context; a texture of a fixed size
        // is made afresh (nothing to copy).
        let made = unsafe {
            (gl.glDeleteTextures)(1, &shows.texture);
            (gl.glGenTextures)(1, &mut shows.texture);
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glTexParameteri)(TEXTURE_2D, MIN_FILTER, NEAREST);
            (gl.glTexParameteri)(TEXTURE_2D, MAG_FILTER, NEAREST);
            (gl.glTexStorage2D)(TEXTURE_2D, 1, RGBA8, size[0], size[1]);
            (gl.glGetError)() == 0
        };
        shows.size = size;
        made
    }

    /// Draws a recorded paint into a window's texture, inside `place` (left, top, right,
    /// bottom); what it draws with is made the first time. Says whether it could: if not, the
    /// CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw_into<W>(&mut self, shows: &Shows<W>, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        if !self.current(shows.surface) {
            return false;
        }
        let egl = &self.egl;
        // SAFETY: the name ends in a zero.
        let mut find = |name: &CStr| unsafe { (egl.eglGetProcAddress)(name.as_ptr()) }.cast_const();
        let made = self.pipeline.get_or_insert_with(|| crate::gpu_draw_opengl::Pipeline::new(&mut find, false).map(Box::new));
        made.as_mut().is_some_and(|pipeline| pipeline.draw(shows.texture, shows.size, list, place))
    }

    /// Swaps a window's back buffer in (it is current), naming `damage` where the display can
    /// (four numbers a place: left, bottom, width, height; none: all of it). Says whether it
    /// swapped; a device reset is lost.
    pub(crate) fn swap<W>(&mut self, shows: &Shows<W>, damage: Option<&[c_int]>) -> bool {
        // SAFETY: the surface is current; the list holds four numbers a place.
        let swapped = unsafe {
            match (self.swap, damage) {
                (Some(swap), Some(damage)) => swap(self.display, shows.surface, damage.as_ptr(), damage.len() as c_int / 4),
                _ => (self.egl.eglSwapBuffers)(self.display, shows.surface),
            }
        };
        // SAFETY: the reset status is the current context's.
        self.lost |= self.reset.is_some_and(|reset| unsafe { reset() } != 0);
        swapped != 0
    }

    /// Lets go of a window: its texture, then its surface.
    pub(crate) fn forget<W>(&mut self, shows: Shows<W>) {
        let current = self.current(shows.surface);
        // SAFETY: the texture is the context's, which is current if it is to be deleted; the
        // surface is released before it is destroyed.
        unsafe {
            if current && shows.texture != 0 {
                (self.gl.glDeleteTextures)(1, &shows.texture);
            }
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            (self.egl.eglDestroySurface)(self.display, shows.surface);
        }
        self.current = null_mut();
    }

    /// Tests: whether it has drawn windows itself (see `gpu_draw`).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        matches!(self.pipeline, Some(Some(_)))
    }

    /// Tests: the device is lost, as when it is removed: its context is destroyed under it.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        // SAFETY: the context is the display's; it is not used again (every later call fails).
        unsafe {
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            (self.egl.eglDestroyContext)(self.display, self.context);
        }
        (self.current, self.context) = (null_mut(), null_mut());
    }
}

impl<S> Drop for Gpu<S> {
    /// Lets go of the context, and with it all it holds, and of the display (unless `lasting`).
    /// (Windows were let go of first.)
    fn drop(&mut self) {
        // SAFETY: EGL's documented calls on the display; nothing of it is used after.
        unsafe {
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            if !self.context.is_null() {
                (self.egl.eglDestroyContext)(self.display, self.context);
            }
            if !self.lasting {
                (self.egl.eglTerminate)(self.display);
            }
            (self.egl.eglReleaseThread)();
        }
    }
}
