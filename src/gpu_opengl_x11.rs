//! X windows shown through the GPU, with the feature `gpu`: EGL and OpenGL ES 3. The CPU draws
//! every pixel as without it (`x11`'s strip); each strip goes into the window's texture
//! (`glTexSubImage2D`). Once a window's changes are all in, the texture is drawn into the back
//! buffer where they are, texel for pixel and unfiltered, and shown with those places named
//! (`eglSwapBuffersWithDamage`), in step with the screen (swap interval 1). So the pixels shown
//! are the CPU's, byte for byte.
//!
//! A back buffer older than one frame (`EGL_EXT_buffer_age`) also gets the places of the frames
//! it missed; one of unknown age is drawn whole.
//!
//! libEGL and libGLESv2 are opened by name when a window first asks for the GPU, so a program
//! builds and starts without them. Once opened they stay: drivers are not safe to unload.
//!
//! `Auto` takes the GPU only where it shows frames directly: a hardware device, presenting
//! through DRI3 (or through its own vendor's EGL, as NVIDIA's). Elsewhere (a software device
//! such as llvmpipe; an X server without DRI3, such as Xvfb, VNC or a VM's) Mesa copies every
//! frame whole through X, which costs more than the CPU's own drawing: the CPU shows the
//! windows. Any other choice of the GPU takes the device either way.
use crate::gpu_x11::{KEPT, has_dri3, note, places};
use crate::x11::{dlclose, dlopen, library, symbol};
use std::ffi::{CStr, c_char, c_int, c_uint, c_ulong, c_void};
use std::mem::ManuallyDrop;
use std::ptr::{null, null_mut};

library!(Egl, ["libEGL.so.1", "libEGL.so"], {
    fn eglGetProcAddress(*const c_char) -> *mut c_void;
    fn eglGetPlatformDisplay(c_uint, *mut c_void, *const isize) -> Handle;
    fn eglInitialize(Handle, *mut c_int, *mut c_int) -> c_uint;
    fn eglTerminate(Handle) -> c_uint;
    fn eglQueryString(Handle, c_int) -> *const c_char;
    fn eglBindAPI(c_uint) -> c_uint;
    fn eglChooseConfig(Handle, *const c_int, *mut Handle, c_int, *mut c_int) -> c_uint;
    fn eglGetConfigAttrib(Handle, Handle, c_int, *mut c_int) -> c_uint;
    fn eglCreateContext(Handle, Handle, Handle, *const c_int) -> Handle;
    fn eglDestroyContext(Handle, Handle) -> c_uint;
    fn eglCreatePlatformWindowSurface(Handle, Handle, *mut c_void, *const isize) -> Handle;
    fn eglDestroySurface(Handle, Handle) -> c_uint;
    fn eglMakeCurrent(Handle, Handle, Handle, Handle) -> c_uint;
    fn eglSwapBuffers(Handle, Handle) -> c_uint;
    fn eglSwapInterval(Handle, c_int) -> c_uint;
    fn eglQuerySurface(Handle, Handle, c_int, *mut c_int) -> c_uint;
    fn eglReleaseThread() -> c_uint;
});

library!(Gles, ["libGLESv2.so.2", "libGLESv2.so"], {
    fn glGetError() -> c_uint;
    fn glGetString(c_uint) -> *const c_char;
    fn glGetIntegerv(c_uint, *mut c_int);
    fn glEnable(c_uint);
    fn glDisable(c_uint);
    fn glGenTextures(c_int, *mut c_uint);
    fn glDeleteTextures(c_int, *const c_uint);
    fn glBindTexture(c_uint, c_uint);
    fn glTexParameteri(c_uint, c_uint, c_int);
    fn glTexImage2D(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
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
    fn glScissor(c_int, c_int, c_int, c_int);
    fn glDrawArrays(c_uint, c_int, c_int);
});

// From EGL's and OpenGL ES 3's headers (egl.h, eglext.h, gl3.h).
const PLATFORM_X11: c_uint = 0x31D5;
const EXTENSIONS: c_int = 0x3055;
const SURFACE_TYPE: c_int = 0x3033;
const WINDOW_BIT: c_int = 0x0004;
const RENDERABLE_TYPE: c_int = 0x3040;
const OPENGL_ES3_BIT: c_int = 0x0040;
const RED_SIZE: c_int = 0x3024;
const GREEN_SIZE: c_int = 0x3023;
const BLUE_SIZE: c_int = 0x3022;
const NATIVE_VISUAL_ID: c_int = 0x302E;
const NONE: c_int = 0x3038;
const OPENGL_ES_API: c_uint = 0x30A0;
const CONTEXT_MAJOR_VERSION: c_int = 0x3098;
const CONTEXT_MINOR_VERSION: c_int = 0x30FB;
const RESET_NOTIFICATION_STRATEGY: c_int = 0x3138;
const LOSE_CONTEXT_ON_RESET: c_int = 0x31BF;
const BUFFER_AGE: c_int = 0x313D;
const VENDOR: c_int = 0x3053;
const TEXTURE_2D: c_uint = 0x0DE1;
const RGBA: c_uint = 0x1908;
const RGBA8: c_int = 0x8058;
const UNSIGNED_BYTE: c_uint = 0x1401;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const UNPACK_SKIP_PIXELS: c_uint = 0x0CF4;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;
const TRIANGLES: c_uint = 0x0004;
const SCISSOR_TEST: c_uint = 0x0C11;
const DITHER: c_uint = 0x0BD0;
const RENDERER: c_uint = 0x1F01;
const MAX_TEXTURE_SIZE: c_uint = 0x0D33;

/// One triangle over the whole viewport.
const SPREAD: &CStr = c"#version 300 es
void main() {
    gl_Position = vec4(float((gl_VertexID & 1) << 2) - 1.0, float((gl_VertexID & 2) << 1) - 1.0, 0.0, 1.0);
}
";

/// Each pixel from its texel, rows from the top, B, G, R in memory (`0xAARRGGBB`, uploaded as
/// RGBA bytes); opaque.
const TEXEL: &CStr = c"#version 300 es
precision highp float;
uniform highp sampler2D shown;
uniform int height;
out vec4 color;
void main() {
    vec4 texel = texelFetch(shown, ivec2(int(gl_FragCoord.x), height - 1 - int(gl_FragCoord.y)), 0);
    color = vec4(texel.b, texel.g, texel.r, 1.0);
}
";

/// What GL_RENDERER says of a device that is the CPU at work: Mesa's and Windows' software
/// rasterizers.
const SOFTWARE: [&str; 5] = ["llvmpipe", "softpipe", "Software Rasterizer", "Basic Render Driver", "SWR"];

/// EGL's handles: display, config, context, surface.
type Handle = *mut c_void;

/// `eglSwapBuffersWithDamageKHR` (or `EXT`): shows a surface's back buffer, naming the places
/// that changed (each left, bottom, width, height).
type SwapWithDamage = unsafe extern "C" fn(Handle, Handle, *const c_int, c_int) -> c_uint;

/// `glGetGraphicsResetStatus` (or `EXT`): not 0 once the device was lost.
type ResetStatus = unsafe extern "C" fn() -> c_uint;

/// The GPU a program's windows are shown through: EGL's display on the X connection, one
/// OpenGL ES 3 context for all of them, and the program drawing a window's texture.
pub(crate) struct Gpu {
    egl: ManuallyDrop<Egl>,
    gl: ManuallyDrop<Gles>,
    display: Handle,
    config: Handle,
    context: Handle,
    /// The surface current now (null: none).
    current: Handle,
    /// Where the program's `height` is; and the widest or tallest texture the device makes.
    height: c_int,
    most: c_int,
    /// What the device offers: swaps naming their damage, the age of back buffers, and the
    /// reset status (where the context was made robust).
    swap: Option<SwapWithDamage>,
    aged: bool,
    reset: Option<ResetStatus>,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// The display is never terminated: Mesa's d3d12 driver (WSL's) crashes at thread exit
    /// after it.
    lasting: bool,
    /// Where the GPU draws the windows too, what it records to, and what it draws with: not yet
    /// tried, or what that gave (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<Option<Box<crate::gpu_draw_opengl::Pipeline>>>,
}

/// A window shown through the GPU: its surface, its texture and that texture's size in pixels.
pub(crate) struct Shows {
    surface: Handle,
    texture: c_uint,
    size: [c_int; 2],
    /// The places put into the texture since it was last shown, and those shown in the last
    /// frames, newest first: left, top, right, bottom, in pixels from the top left.
    put: Vec<[c_int; 4]>,
    past: Vec<Vec<[c_int; 4]>>,
}

impl Shows {
    /// Its texture's size in pixels.
    pub(crate) fn size(&self) -> [c_int; 2] {
        self.size
    }
}

impl Gpu {
    /// Opens EGL on X display `x` and shows window `id` through it (`size` pixels, of
    /// `visual`). Nothing where EGL, OpenGL ES 3 or a config for the visual is missing, where
    /// anything fails, or where frames would not be shown directly and `any` is false.
    ///
    /// # Safety
    /// `x` is an open X display, and `id` one of its windows, alive, of `visual`.
    pub(crate) unsafe fn open(x: *mut c_void, visual: *mut c_void, id: c_ulong, size: [c_int; 2], any: bool) -> Option<(Gpu, Shows)> {
        let egl = ManuallyDrop::new(Egl::open()?);
        let gl = ManuallyDrop::new(Gles::open()?);
        // SAFETY: EGL's documented call, for an X display (EGL_EXT_platform_x11).
        let display = unsafe { (egl.eglGetPlatformDisplay)(PLATFORM_X11, x, null()) };
        if display.is_null() {
            return None;
        }
        let mut gpu = Gpu {
            egl,
            gl,
            display,
            config: null_mut(),
            context: null_mut(),
            current: null_mut(),
            height: -1,
            most: 0,
            swap: None,
            aged: false,
            reset: None,
            lost: false,
            lasting: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
        };
        // SAFETY: the display was just found; the version is not asked for.
        if unsafe { (gpu.egl.eglInitialize)(display, null_mut(), null_mut()) } == 0 {
            return None;
        }
        // SAFETY: the visual is X's (the caller's promise).
        gpu.config = unsafe { gpu.config_for(visual) }?;
        gpu.context = gpu.context()?;
        gpu.learn();
        let shows = gpu.window(id, size)?;
        gpu.lasting = gpu.name().starts_with("D3D12");
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !gpu.software(), std::env::var("TAUT_DRAW").ok().as_deref());
        }
        // SAFETY: the display is open (the caller's promise).
        if (any || unsafe { gpu.direct(x) }) && gpu.program() {
            return Some((gpu, shows));
        }
        gpu.forget(shows);
        None
    }

    /// The config drawing in `visual`: 8-bit red, green and blue, for windows, OpenGL ES 3.
    ///
    /// # Safety
    /// `visual` is an X `Visual`.
    unsafe fn config_for(&self, visual: *mut c_void) -> Option<Handle> {
        let wanted = [SURFACE_TYPE, WINDOW_BIT, RENDERABLE_TYPE, OPENGL_ES3_BIT, RED_SIZE, 8, GREEN_SIZE, 8, BLUE_SIZE, 8, NONE];
        let (mut configs, mut found) = ([null_mut(); 64], 0);
        // SAFETY: EGL fills in at most 64 configs and their count. A `Visual` starts with a
        // pointer, then its id (Xlib.h).
        let id = unsafe {
            (self.egl.eglChooseConfig)(self.display, wanted.as_ptr(), configs.as_mut_ptr(), 64, &mut found);
            *visual.cast::<c_ulong>().add(1)
        };
        let mut chosen = configs[..found.clamp(0, 64) as usize].iter();
        chosen.find(|config| self.attribute(**config, NATIVE_VISUAL_ID) as c_ulong == id).copied()
    }

    /// One of a config's attributes (0 where EGL has none).
    fn attribute(&self, config: Handle, name: c_int) -> c_int {
        let mut value = 0;
        // SAFETY: the config is the display's; EGL fills in one number.
        unsafe { (self.egl.eglGetConfigAttrib)(self.display, config, name, &mut value) };
        value
    }

    /// Whether the display offers an extension.
    fn offers(&self, name: &str) -> bool {
        // SAFETY: EGL's own string, ending in a zero, alive while the display is.
        let all = unsafe { (self.egl.eglQueryString)(self.display, EXTENSIONS) };
        !all.is_null() && unsafe { CStr::from_ptr(all) }.to_bytes().split(|c| *c == b' ').any(|offered| offered == name.as_bytes())
    }

    /// A function by name, from EGL; nothing where there is none.
    fn function(&self, name: &CStr) -> Option<*mut c_void> {
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

    /// Notes what the display offers: swaps naming their damage, and the age of back buffers.
    fn learn(&mut self) {
        let damage = [("EGL_KHR_swap_buffers_with_damage", c"eglSwapBuffersWithDamageKHR"), ("EGL_EXT_swap_buffers_with_damage", c"eglSwapBuffersWithDamageEXT")];
        let swap = damage.into_iter().find(|(extension, _)| self.offers(extension));
        // SAFETY: the function is declared as eglext.h declares it.
        self.swap = swap.and_then(|(_, name)| self.function(name)).map(|f| unsafe { std::mem::transmute::<*mut c_void, SwapWithDamage>(f) });
        self.aged = self.offers("EGL_EXT_buffer_age");
    }

    /// Makes `surface` the one drawn to, unless it is. Says whether it is; if it can't be, the
    /// device is lost.
    fn current(&mut self, surface: Handle) -> bool {
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

    /// Whether frames are shown directly: a hardware device, and X display `x` offers DRI3
    /// where the EGL is Mesa's.
    ///
    /// # Safety
    /// `x` is an open X display.
    pub(crate) unsafe fn direct(&self, x: *mut c_void) -> bool {
        // SAFETY: EGL's own string, ending in a zero, alive while the display is.
        let vendor = unsafe { (self.egl.eglQueryString)(self.display, VENDOR) };
        let mesa = vendor.is_null() || unsafe { CStr::from_ptr(vendor) }.to_bytes().starts_with(b"Mesa");
        // SAFETY: as the caller promises.
        !self.software() && (!mesa || unsafe { has_dri3(x) })
    }

    /// Compiles the program drawing a texture and makes it the one used. Says whether it could.
    fn program(&mut self) -> bool {
        let gl = &self.gl;
        let shaders = [(VERTEX_SHADER, SPREAD), (FRAGMENT_SHADER, TEXEL)].map(|(kind, source)| self.shader(kind, source));
        let mut linked = 0;
        // SAFETY: OpenGL ES's documented calls on the current context; the uniform's name ends
        // in a zero.
        unsafe {
            let program = (gl.glCreateProgram)();
            shaders.iter().for_each(|shader| (gl.glAttachShader)(program, *shader));
            (gl.glLinkProgram)(program);
            (gl.glGetProgramiv)(program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| (gl.glDeleteShader)(*shader));
            (gl.glUseProgram)(program);
            self.height = (gl.glGetUniformLocation)(program, c"height".as_ptr());
            (gl.glDisable)(DITHER);
            (gl.glEnable)(SCISSOR_TEST);
            (gl.glGetIntegerv)(MAX_TEXTURE_SIZE, &mut self.most);
        }
        linked != 0 && shaders.iter().all(|shader| *shader != 0)
    }

    /// A compiled shader (0 where it didn't compile).
    fn shader(&self, kind: c_uint, source: &CStr) -> c_uint {
        let (gl, mut compiled) = (&self.gl, 0);
        // SAFETY: as above; the source ends in a zero.
        unsafe {
            let shader = (gl.glCreateShader)(kind);
            (gl.glShaderSource)(shader, 1, &source.as_ptr(), null());
            (gl.glCompileShader)(shader);
            (gl.glGetShaderiv)(shader, COMPILE_STATUS, &mut compiled);
            if compiled != 0 { shader } else { 0 }
        }
    }

    /// Shows window `id` of the display through the device: its surface, made current, and a
    /// texture `size` pixels big. Nothing where either can't be made.
    pub(crate) fn window(&mut self, id: c_ulong, size: [c_int; 2]) -> Option<Shows> {
        let mut native = id;
        // SAFETY: for X, EGL reads the window's id through the pointer, during the call
        // (EGL_EXT_platform_x11).
        let surface = unsafe { (self.egl.eglCreatePlatformWindowSurface)(self.display, self.config, (&raw mut native).cast(), null()) };
        if surface.is_null() {
            return None;
        }
        let mut shows = Shows { surface, texture: 0, size: [0; 2], put: Vec::new(), past: Vec::new() };
        // SAFETY: the surface is current: the interval is its own.
        if self.current(surface) && unsafe { (self.egl.eglSwapInterval)(self.display, 1) } != 0 && self.fit(&mut shows, size) {
            return Some(shows);
        }
        self.forget(shows);
        None
    }

    /// Gives a window's texture its size. Says whether it has it: where the device makes none
    /// that big, the window is better shown by X.
    pub(crate) fn fit(&mut self, shows: &mut Shows, size: [c_int; 2]) -> bool {
        if shows.size == size {
            return true;
        }
        let small = self.most == 0 || size.iter().all(|side| (1..=self.most).contains(side));
        if !small || !self.current(shows.surface) {
            return false;
        }
        let gl = &self.gl;
        // SAFETY: OpenGL ES's documented calls on the current context; nothing to copy yet.
        let made = unsafe {
            if shows.texture == 0 {
                (gl.glGenTextures)(1, &mut shows.texture);
            }
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glTexParameteri)(TEXTURE_2D, MIN_FILTER, NEAREST);
            (gl.glTexParameteri)(TEXTURE_2D, MAG_FILTER, NEAREST);
            (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA8, size[0], size[1], 0, RGBA, UNSIGNED_BYTE, null());
            (gl.glGetError)() == 0
        };
        (shows.size, shows.past) = (size, Vec::new());
        made
    }

    /// Puts part of a painted strip into a window's texture: `place` (left, top, right, bottom
    /// in the window's pixels), the strip's first row being its top, `width` pixels a row.
    pub(crate) fn put(&mut self, shows: &mut Shows, strip: &[u32], width: c_int, place: [c_int; 4]) {
        let [left, top, right, bottom] = place;
        if !self.current(shows.surface) || strip.len() < (width * (bottom - top)) as usize {
            return;
        }
        let gl = &self.gl;
        // SAFETY: the strip holds the rows read, `width` pixels each; OpenGL ES copies them out
        // before it returns.
        unsafe {
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glPixelStorei)(UNPACK_ROW_LENGTH, width);
            (gl.glPixelStorei)(UNPACK_SKIP_PIXELS, left);
            (gl.glTexSubImage2D)(TEXTURE_2D, 0, left, top, right - left, bottom - top, RGBA, UNSIGNED_BYTE, strip.as_ptr().cast());
        }
        note(&mut shows.put, place);
    }

    /// Draws a recorded paint into a window's texture, inside `place` (left, top, right,
    /// bottom), to be shown with what was put in; what it draws with is made the first time.
    /// Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        if !self.current(shows.surface) {
            return false;
        }
        let egl = &self.egl;
        // SAFETY: the name ends in a zero.
        let mut find = |name: &CStr| unsafe { (egl.eglGetProcAddress)(name.as_ptr()) }.cast_const();
        let made = self.pipeline.get_or_insert_with(|| crate::gpu_draw_opengl::Pipeline::new(&mut find, false).map(Box::new));
        let drawn = made.as_mut().is_some_and(|pipeline| pipeline.draw(shows.texture, shows.size, list, place));
        if drawn {
            note(&mut shows.put, place);
        }
        drawn
    }

    /// Shows what was put into a window's texture since it was last shown: drawn into the back
    /// buffer where it changed (and where an older back buffer lacks), and swapped in naming
    /// those places. A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows) {
        if shows.put.is_empty() || !self.current(shows.surface) {
            return;
        }
        let ([width, height], age) = (shows.size, self.age(shows.surface));
        let wanted = places(&shows.put, &shows.past, age.checked_sub(1), shows.size);
        let gl = &self.gl;
        // SAFETY: OpenGL ES's documented calls on the current context, with the program in use.
        unsafe {
            (gl.glViewport)(0, 0, width, height);
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glUniform1i)(self.height, height);
            for [left, top, right, bottom] in wanted {
                (gl.glScissor)(left, height - bottom, right - left, bottom - top);
                (gl.glDrawArrays)(TRIANGLES, 0, 3);
            }
        }
        let rects: Vec<c_int> = shows.put.iter().flat_map(|[left, top, right, bottom]| [*left, height - bottom, right - left, bottom - top]).collect();
        // SAFETY: the surface is current; the list holds four numbers a place.
        let swapped = unsafe {
            match self.swap {
                Some(swap) => swap(self.display, shows.surface, rects.as_ptr(), shows.put.len() as c_int),
                None => (self.egl.eglSwapBuffers)(self.display, shows.surface),
            }
        };
        // SAFETY: the reset status is the current context's.
        self.lost |= swapped == 0 || self.reset.is_some_and(|reset| unsafe { reset() } != 0);
        shows.past.insert(0, std::mem::take(&mut shows.put));
        shows.past.truncate(KEPT);
    }

    /// How many frames old the current back buffer of a surface is (0: unknown).
    fn age(&self, surface: Handle) -> usize {
        let mut age = 0;
        // SAFETY: the surface is the display's and current; EGL fills in one number.
        if self.aged && unsafe { (self.egl.eglQuerySurface)(self.display, surface, BUFFER_AGE, &mut age) } == 0 {
            return 0;
        }
        age.max(0) as usize
    }

    /// Lets go of a window: its texture, then its surface.
    pub(crate) fn forget(&mut self, shows: Shows) {
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

    /// Tests: the places the last frame named.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        shows.past.first().map_or(&[], Vec::as_slice)
    }

    /// Tests: the device is lost, as when it is removed: its context is destroyed under it.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        // SAFETY: the context is the display's; it is not used again (every later call fails).
        unsafe {
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            (self.egl.eglDestroyContext)(self.display, self.context);
        }
        self.current = null_mut();
    }
}

impl Drop for Gpu {
    /// Lets go of the context and the display. (Windows were let go of first.)
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
