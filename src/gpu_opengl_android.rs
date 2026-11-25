//! Android windows shown through OpenGL ES 3 and EGL, with the feature `gpu`, where Vulkan is
//! missing or asked against. The CPU draws every pixel as without it; each place it drew goes
//! into the window's texture (`glTexSubImage2D`; with `gpu-draw` the GPU draws large parts into
//! it itself). A frame clears the window to the theme's background where the texture leaves
//! room (the insets), draws the texture texel for pixel inside them, and swaps naming the places
//! that changed (`eglSwapBuffersWithDamageKHR`), in step with the screen (swap interval 1). The
//! pixels shown are the CPU's, byte for byte.
//!
//! - The surface is the activity's window; it goes with the window, the context stays.
//! - A context made robust where EGL can (`EGL_EXT_create_context_robustness`) says when the
//!   device was reset: then it is lost. A swap that finds the window gone says that.
//! - libEGL and libGLESv3 are the system's own (every Android has them).
//!
//! `Auto` takes the GPU only where it is not the CPU at work (SwiftShader, llvmpipe).
use crate::gpu_android::Block;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr::{null, null_mut};

/// EGL's handles: display, config, context, surface.
type Handle = *mut c_void;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
    fn eglGetDisplay(native: *mut c_void) -> Handle;
    fn eglInitialize(display: Handle, major: *mut c_int, minor: *mut c_int) -> c_uint;
    fn eglTerminate(display: Handle) -> c_uint;
    fn eglQueryString(display: Handle, name: c_int) -> *const c_char;
    fn eglBindAPI(api: c_uint) -> c_uint;
    fn eglChooseConfig(display: Handle, wanted: *const c_int, configs: *mut Handle, room: c_int, found: *mut c_int) -> c_uint;
    fn eglGetConfigAttrib(display: Handle, config: Handle, name: c_int, value: *mut c_int) -> c_uint;
    fn eglCreateContext(display: Handle, config: Handle, shared: Handle, asked: *const c_int) -> Handle;
    fn eglDestroyContext(display: Handle, context: Handle) -> c_uint;
    fn eglCreateWindowSurface(display: Handle, config: Handle, window: *mut c_void, asked: *const c_int) -> Handle;
    fn eglDestroySurface(display: Handle, surface: Handle) -> c_uint;
    fn eglMakeCurrent(display: Handle, draw: Handle, read: Handle, context: Handle) -> c_uint;
    fn eglSwapBuffers(display: Handle, surface: Handle) -> c_uint;
    fn eglSwapInterval(display: Handle, interval: c_int) -> c_uint;
    fn eglGetError() -> c_int;
    fn eglReleaseThread() -> c_uint;
}

#[link(name = "GLESv3")]
unsafe extern "C" {
    fn glGetError() -> c_uint;
    fn glGetString(name: c_uint) -> *const c_char;
    fn glGetIntegerv(name: c_uint, value: *mut c_int);
    fn glDisable(what: c_uint);
    fn glGenTextures(count: c_int, textures: *mut c_uint);
    fn glDeleteTextures(count: c_int, textures: *const c_uint);
    fn glBindTexture(target: c_uint, texture: c_uint);
    fn glTexParameteri(target: c_uint, name: c_uint, value: c_int);
    fn glTexStorage2D(target: c_uint, levels: c_int, format: c_uint, width: c_int, height: c_int);
    fn glTexSubImage2D(target: c_uint, level: c_int, x: c_int, y: c_int, width: c_int, height: c_int, format: c_uint, kind: c_uint, pixels: *const c_void);
    fn glPixelStorei(name: c_uint, value: c_int);
    fn glCreateShader(kind: c_uint) -> c_uint;
    fn glShaderSource(shader: c_uint, count: c_int, sources: *const *const c_char, lengths: *const c_int);
    fn glCompileShader(shader: c_uint);
    fn glGetShaderiv(shader: c_uint, name: c_uint, value: *mut c_int);
    fn glDeleteShader(shader: c_uint);
    fn glCreateProgram() -> c_uint;
    fn glAttachShader(program: c_uint, shader: c_uint);
    fn glLinkProgram(program: c_uint);
    fn glGetProgramiv(program: c_uint, name: c_uint, value: *mut c_int);

    fn glUseProgram(program: c_uint);
    fn glGetUniformLocation(program: c_uint, name: *const c_char) -> c_int;
    fn glUniform1i(at: c_int, value: c_int);
    fn glUniform2i(at: c_int, x: c_int, y: c_int);
    fn glViewport(x: c_int, y: c_int, width: c_int, height: c_int);
    fn glClearColor(red: f32, green: f32, blue: f32, alpha: f32);
    fn glClear(what: c_uint);
    fn glDrawArrays(mode: c_uint, first: c_int, count: c_int);
}

// From EGL's and OpenGL ES 3's headers (egl.h, eglext.h, gl3.h).
const EXTENSIONS: c_int = 0x3055;
const SURFACE_TYPE: c_int = 0x3033;
const WINDOW_BIT: c_int = 0x0004;
const RENDERABLE_TYPE: c_int = 0x3040;
const OPENGL_ES3_BIT: c_int = 0x0040;
const RED_SIZE: c_int = 0x3024;
const GREEN_SIZE: c_int = 0x3023;
const BLUE_SIZE: c_int = 0x3022;
const ALPHA_SIZE: c_int = 0x3021;
const NATIVE_VISUAL_ID: c_int = 0x302E;
const NONE: c_int = 0x3038;
const OPENGL_ES_API: c_uint = 0x30A0;
const CONTEXT_MAJOR_VERSION: c_int = 0x3098;
const CONTEXT_MINOR_VERSION: c_int = 0x30FB;
const RESET_NOTIFICATION_STRATEGY: c_int = 0x3138;
const LOSE_CONTEXT_ON_RESET: c_int = 0x31BF;
/// What a failed swap says of the window: `EGL_BAD_NATIVE_WINDOW`, `EGL_BAD_SURFACE`.
const WINDOW_GONE: [c_int; 2] = [0x300B, 0x300D];
const TEXTURE_2D: c_uint = 0x0DE1;
const RGBA: c_uint = 0x1908;
const RGBA8: c_uint = 0x8058;
const UNSIGNED_BYTE: c_uint = 0x1401;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const UNPACK_SKIP_PIXELS: c_uint = 0x0CF4;
const UNPACK_SKIP_ROWS: c_uint = 0x0CF3;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;
const TRIANGLES: c_uint = 0x0004;
const SCISSOR_TEST: c_uint = 0x0C11;
const BLEND: c_uint = 0x0BE2;
const DITHER: c_uint = 0x0BD0;
const COLOR_BUFFER_BIT: c_uint = 0x4000;
const RENDERER: c_uint = 0x1F01;
const MAX_TEXTURE_SIZE: c_uint = 0x0D33;
/// `ANativeWindow`'s 8-bit red, green, blue and alpha (`WINDOW_FORMAT_RGBA_8888`).
const RGBA_8888: c_int = 1;

/// One triangle over the whole viewport.
const SPREAD: &CStr = c"#version 300 es
void main() {
    gl_Position = vec4(float((gl_VertexID & 1) << 2) - 1.0, float((gl_VertexID & 2) << 1) - 1.0, 0.0, 1.0);
}
";

/// Each pixel from its texel, the texture's top left `at` (left, top) in a window `height`
/// pixels tall; B, G, R in memory (`0xAARRGGBB`, uploaded as RGBA bytes); opaque.
const TEXEL: &CStr = c"#version 300 es
precision highp float;
uniform highp sampler2D shown;
uniform ivec2 at;
uniform int height;
out vec4 color;
void main() {
    vec4 texel = texelFetch(shown, ivec2(int(gl_FragCoord.x) - at.x, height - 1 - int(gl_FragCoord.y) - at.y), 0);
    color = vec4(texel.b, texel.g, texel.r, 1.0);
}
";

/// What GL_RENDERER says of a device that is the CPU at work.
const SOFTWARE: [&str; 3] = ["SwiftShader", "llvmpipe", "softpipe"];

/// `eglSwapBuffersWithDamageKHR` (or `EXT`): shows a surface's back buffer, naming the places
/// that changed (each left, bottom, width, height).
type SwapWithDamage = unsafe extern "C" fn(Handle, Handle, *const c_int, c_int) -> c_uint;

/// `glGetGraphicsResetStatus` (or `EXT`): not 0 once the device was lost.
type ResetStatus = unsafe extern "C" fn() -> c_uint;

/// The GPU the window is shown through: EGL's display, one OpenGL ES 3 context, and the program
/// drawing the window's texture.
pub(crate) struct Gpu {
    display: Handle,
    config: Handle,
    context: Handle,
    /// The surface current now (null: none).
    current: Handle,
    /// The program, and where its `at` and `height` are; the widest or tallest texture the
    /// device makes.
    program: c_uint,
    at: c_int,
    height: c_int,
    most: c_int,
    /// What the display offers: swaps naming their damage, and the reset status (where the
    /// context was made robust).
    swap: Option<SwapWithDamage>,
    reset: Option<ResetStatus>,
    /// The device was lost, or failed: all of it is to be let go.
    pub(crate) lost: bool,
    /// Where the GPU draws the window too, what it records to, and what it draws with: not yet
    /// tried, or what that gave (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    pub(crate) list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<Option<Box<crate::gpu_draw_opengl::Pipeline>>>,
}

/// The window shown through the GPU: its surface, and its texture of the interface's pixels.
pub(crate) struct Shows {
    surface: Handle,
    texture: c_uint,
    size: [c_int; 2],
    /// The texture is new: all of the window is to be drawn into it.
    new: bool,
    /// The window's size, where the texture lies in it (left, top), and the colour around it.
    window: [c_int; 2],
    at: [c_int; 2],
    background: u32,
    /// The places changed since the last frame (left, top, right, bottom in the texture).
    changed: Vec<[c_int; 4]>,
    /// Everything is to be named in the next swap.
    whole: bool,
    /// The window went: its surface is to be let go.
    pub(crate) gone: bool,
    /// Tests: the places the last frame named.
    #[cfg(test)]
    named: Vec<[c_int; 4]>,
}

impl Shows {
    /// Whether all of the window is to be drawn (its texture is new); asked once.
    pub(crate) fn take_whole(&mut self) -> bool {
        std::mem::take(&mut self.new)
    }
}

impl Gpu {
    /// Opens EGL and OpenGL ES 3, and shows `window` (an `ANativeWindow`) through them, sized by
    /// `fit`. Nothing where any of it is missing or fails, or where the device is the CPU at
    /// work and `any` is false.
    ///
    /// # Safety
    /// `window` is an `ANativeWindow`, alive until the window is let go of (`forget`).
    pub(crate) unsafe fn open(window: *mut c_void, any: bool) -> Option<(Gpu, Shows)> {
        // SAFETY: EGL's documented calls, on the default display; the version is not asked for.
        let display = unsafe { eglGetDisplay(null_mut()) };
        if display.is_null() || unsafe { eglInitialize(display, null_mut(), null_mut()) } == 0 {
            return None;
        }
        let mut gpu = Gpu {
            display,
            config: null_mut(),
            context: null_mut(),
            current: null_mut(),
            program: 0,
            at: -1,
            height: -1,
            most: 0,
            swap: None,
            reset: None,
            lost: false,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::Never,
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
        };
        gpu.config = gpu.config()?;
        gpu.context = gpu.context()?;
        gpu.learn();
        // SAFETY: as the caller promises.
        let shows = unsafe { gpu.window(window) }?;
        let software = gpu.software();
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !software, crate::platform::setting("TAUT_DRAW").as_deref());
        }
        if (any || !software) && gpu.make_program() {
            return Some((gpu, shows));
        }
        gpu.forget(shows);
        None
    }

    /// The config drawing into windows of `ANativeWindow`'s RGBA: 8-bit red, green, blue and
    /// alpha, OpenGL ES 3.
    fn config(&self) -> Option<Handle> {
        let wanted = [SURFACE_TYPE, WINDOW_BIT, RENDERABLE_TYPE, OPENGL_ES3_BIT, RED_SIZE, 8, GREEN_SIZE, 8, BLUE_SIZE, 8, ALPHA_SIZE, 8, NONE];
        let (mut configs, mut found) = ([null_mut(); 64], 0);
        // SAFETY: EGL fills in at most 64 configs and their count.
        unsafe { eglChooseConfig(self.display, wanted.as_ptr(), configs.as_mut_ptr(), 64, &mut found) };
        let chosen = &configs[..found.clamp(0, 64) as usize];
        chosen.iter().find(|config| self.attribute(**config, NATIVE_VISUAL_ID) == RGBA_8888).copied()
    }

    /// One of a config's attributes (0 where EGL has none).
    fn attribute(&self, config: Handle, name: c_int) -> c_int {
        let mut value = 0;
        // SAFETY: the config is the display's; EGL fills in one number.
        unsafe { eglGetConfigAttrib(self.display, config, name, &mut value) };
        value
    }

    /// Whether the display offers an extension.
    fn offers(&self, name: &str) -> bool {
        // SAFETY: EGL's own string, ending in a zero, alive while the display is.
        let all = unsafe { eglQueryString(self.display, EXTENSIONS) };
        !all.is_null() && unsafe { CStr::from_ptr(all) }.to_bytes().split(|c| *c == b' ').any(|offered| offered == name.as_bytes())
    }

    /// A function by name, from EGL; nothing where there is none.
    fn function(name: &CStr) -> Option<*mut c_void> {
        // SAFETY: the name ends in a zero.
        let found = unsafe { eglGetProcAddress(name.as_ptr()) };
        (!found.is_null()).then_some(found)
    }

    /// An OpenGL ES 3.0 context: told of device loss where the display can tell, and then the
    /// reset status is kept to ask.
    fn context(&mut self) -> Option<Handle> {
        let robust = [CONTEXT_MAJOR_VERSION, 3, CONTEXT_MINOR_VERSION, 0, RESET_NOTIFICATION_STRATEGY, LOSE_CONTEXT_ON_RESET, NONE];
        let plain = [CONTEXT_MAJOR_VERSION, 3, CONTEXT_MINOR_VERSION, 0, NONE];
        // SAFETY: EGL's documented call.
        unsafe { eglBindAPI(OPENGL_ES_API) };
        let mut made = null_mut();
        if self.offers("EGL_EXT_create_context_robustness") {
            made = self.made(&robust);
            let found = [c"glGetGraphicsResetStatusEXT", c"glGetGraphicsResetStatus"].into_iter().find_map(Gpu::function);
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
        unsafe { eglCreateContext(self.display, self.config, null_mut(), asked.as_ptr()) }
    }

    /// Notes whether the display's swaps name their damage.
    fn learn(&mut self) {
        let damage = [("EGL_KHR_swap_buffers_with_damage", c"eglSwapBuffersWithDamageKHR"), ("EGL_EXT_swap_buffers_with_damage", c"eglSwapBuffersWithDamageEXT")];
        let swap = damage.into_iter().find(|(extension, _)| self.offers(extension));
        // SAFETY: the function is declared as eglext.h declares it.
        self.swap = swap.and_then(|(_, name)| Gpu::function(name)).map(|f| unsafe { std::mem::transmute::<*mut c_void, SwapWithDamage>(f) });
    }

    /// Makes `surface` the one drawn to, unless it is. Says whether it is; if it can't be, the
    /// device is lost.
    fn current(&mut self, surface: Handle) -> bool {
        // SAFETY: the surface and context are the display's.
        if self.current != surface && unsafe { eglMakeCurrent(self.display, surface, surface, self.context) } == 0 {
            (self.lost, self.current) = (true, null_mut());
            return false;
        }
        self.current = surface;
        true
    }

    /// The device's name, from OpenGL ES (as "Android Emulator OpenGL ES Translator (Google
    /// SwiftShader)").
    pub(crate) fn name(&self) -> String {
        // SAFETY: a context is current; the string is OpenGL's, ending in a zero.
        let name = unsafe { glGetString(RENDERER) };
        if name.is_null() { String::new() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() }
    }

    /// Whether the device is the CPU at work.
    fn software(&self) -> bool {
        let name = self.name();
        SOFTWARE.iter().any(|software| name.contains(software))
    }

    /// Compiles the program drawing a texture. Says whether it could.
    fn make_program(&mut self) -> bool {
        let shaders = [(VERTEX_SHADER, SPREAD), (FRAGMENT_SHADER, TEXEL)].map(|(kind, source)| shader(kind, source));
        let mut linked = 0;
        // SAFETY: OpenGL ES's documented calls on the current context; the uniforms' names end in
        // zeros.
        unsafe {
            self.program = glCreateProgram();
            shaders.iter().for_each(|shader| glAttachShader(self.program, *shader));
            glLinkProgram(self.program);
            glGetProgramiv(self.program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| glDeleteShader(*shader));
            self.at = glGetUniformLocation(self.program, c"at".as_ptr());
            self.height = glGetUniformLocation(self.program, c"height".as_ptr());
            glGetIntegerv(MAX_TEXTURE_SIZE, &mut self.most);
        }
        linked != 0 && shaders.iter().all(|shader| *shader != 0)
    }

    /// Shows `window` (an `ANativeWindow`) through the device: its surface, made current, swapping
    /// in step with the screen; sized by `fit`. Nothing where it can't be made.
    ///
    /// # Safety
    /// As `open`.
    pub(crate) unsafe fn window(&mut self, window: *mut c_void) -> Option<Shows> {
        // SAFETY: the window is alive (the caller's promise).
        let surface = unsafe { eglCreateWindowSurface(self.display, self.config, window, null()) };
        if surface.is_null() {
            return None;
        }
        let shows = Shows {
            surface,
            texture: 0,
            size: [0; 2],
            new: true,
            window: [0; 2],
            at: [0; 2],
            background: 0,
            changed: Vec::new(),
            whole: true,
            gone: false,
            #[cfg(test)]
            named: Vec::new(),
        };
        // SAFETY: the surface is current: the interval is its own.
        if self.current(surface) && unsafe { eglSwapInterval(self.display, 1) } != 0 {
            return Some(shows);
        }
        self.forget(shows);
        None
    }

    /// Gives the window its size (`window` pixels), and its texture its own (`size`, at `at` in
    /// the window): a new texture, all of which is to be drawn, where `size` changed. Says
    /// whether it has it: where the device makes none that big, the window is better shown by
    /// the CPU.
    pub(crate) fn fit(&mut self, shows: &mut Shows, window: [c_int; 2], size: [c_int; 2], at: [c_int; 2]) -> bool {
        if (shows.window, shows.at) != (window, at) {
            (shows.window, shows.at, shows.whole) = (window, at, true);
        }
        if shows.size == size && shows.texture != 0 {
            return true;
        }
        let small = size.iter().all(|side| (1..=self.most.max(1)).contains(side));
        if !small || !self.current(shows.surface) {
            return false;
        }
        // SAFETY: OpenGL ES's documented calls on the current context; a texture of a fixed size
        // is made afresh (nothing to copy).
        let made = unsafe {
            glDeleteTextures(1, &shows.texture);
            glGenTextures(1, &mut shows.texture);
            glBindTexture(TEXTURE_2D, shows.texture);
            glTexParameteri(TEXTURE_2D, MIN_FILTER, NEAREST);
            glTexParameteri(TEXTURE_2D, MAG_FILTER, NEAREST);
            glTexStorage2D(TEXTURE_2D, 1, RGBA8, size[0], size[1]);
            glGetError() == 0
        };
        (shows.size, shows.new, shows.whole) = (size, true, true);
        shows.changed.clear();
        made
    }

    /// Puts `place` of a block of pixels (left, top, right, bottom) into the window's texture at
    /// the same place, as far as both reach, to be shown with the next frame.
    pub(crate) fn put(&mut self, shows: &mut Shows, block: &Block, place: [c_int; 4]) {
        let Some([left, top, right, bottom]) = block.cut(place, shows.size).filter(|_| self.current(shows.surface)) else { return };
        // SAFETY: the block holds the place's rows (`cut`), `width` pixels each; OpenGL ES copies
        // them out before it returns.
        unsafe {
            glBindTexture(TEXTURE_2D, shows.texture);
            glPixelStorei(UNPACK_ROW_LENGTH, block.width as c_int);
            glPixelStorei(UNPACK_SKIP_PIXELS, left - block.at[0]);
            glPixelStorei(UNPACK_SKIP_ROWS, top - block.at[1]);
            glTexSubImage2D(TEXTURE_2D, 0, left, top, right - left, bottom - top, RGBA, UNSIGNED_BYTE, block.pixels.as_ptr().cast());
            [UNPACK_ROW_LENGTH, UNPACK_SKIP_PIXELS, UNPACK_SKIP_ROWS].into_iter().for_each(|name| glPixelStorei(name, 0));
        }
        shows.changed.push([left, top, right, bottom]);
    }

    /// Draws a recorded paint into the window's texture, inside `place` (left, top, right,
    /// bottom), to be shown with the next frame; what it draws with is made the first time.
    /// Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        if !self.current(shows.surface) {
            return false;
        }
        let made = self.pipeline.get_or_insert_with(|| {
            // SAFETY: the name ends in a zero.
            let mut find = |name: &CStr| unsafe { eglGetProcAddress(name.as_ptr()) }.cast_const();
            crate::gpu_draw_opengl::Pipeline::new(&mut find, false).map(Box::new)
        });
        let drawn = made.as_mut().is_some_and(|pipeline| pipeline.draw(shows.texture, shows.size, list, place));
        if drawn {
            shows.changed.push(place);
        }
        drawn
    }

    /// Shows the window: cleared to `background` (`0xAARRGGBB`) where the texture leaves room,
    /// the texture drawn inside, swapped in naming what changed. With nothing changed, only
    /// where `again` (the system asks for the window whole). A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows, background: u32, again: bool) {
        shows.whole |= again || std::mem::replace(&mut shows.background, background) != background;
        if shows.changed.is_empty() && !shows.whole || !self.current(shows.surface) {
            return;
        }
        let ([width, height], [left, top], [wide, high]) = (shows.window, shows.at, shows.size);
        let [red, green, blue] = [16, 8, 0].map(|shift| ((background >> shift) & 0xff) as f32 / 255.0);
        // SAFETY: OpenGL ES's documented calls on the current context; what drawing put in
        // context state is set again.
        unsafe {
            glDisable(SCISSOR_TEST);
            glDisable(BLEND);
            glDisable(DITHER);
            glViewport(0, 0, width, height);
            glClearColor(red, green, blue, 1.0);
            glClear(COLOR_BUFFER_BIT);
            glViewport(left, height - top - high, wide, high);
            glUseProgram(self.program);
            glBindTexture(TEXTURE_2D, shows.texture);
            glUniform2i(self.at, left, top);
            glUniform1i(self.height, height);
            glDrawArrays(TRIANGLES, 0, 3);
        }
        self.swap(shows);
    }

    /// Swaps the window's back buffer in, naming the places that changed (all of it after a
    /// new size, texture or background).
    fn swap(&mut self, shows: &mut Shows) {
        let ([left, top], height) = (shows.at, shows.window[1]);
        let changed = std::mem::take(&mut shows.changed);
        let named = !std::mem::take(&mut shows.whole);
        let rects: Vec<c_int> = changed.iter().flat_map(|[l, t, r, b]| [l + left, height - (b + top), r - l, b - t]).collect();
        // SAFETY: the surface is current; the list holds four numbers a place.
        let swapped = unsafe {
            match self.swap.filter(|_| named) {
                Some(swap) => swap(self.display, shows.surface, rects.as_ptr(), changed.len() as c_int),
                None => eglSwapBuffers(self.display, shows.surface),
            }
        };
        // SAFETY: EGL's and the context's own questions.
        let (error, reset) = unsafe { (if swapped == 0 { eglGetError() } else { 0 }, self.reset.is_some_and(|reset| reset() != 0)) };
        shows.gone |= WINDOW_GONE.contains(&error);
        self.lost |= reset || swapped == 0 && !shows.gone;
        #[cfg(test)]
        {
            shows.named = if named { changed } else { vec![[0, 0, shows.window[0] - left, shows.window[1] - top]] };
        }
    }

    /// Lets go of the window: its texture, then its surface. (Before its `ANativeWindow` goes.)
    pub(crate) fn forget(&mut self, shows: Shows) {
        let current = self.current(shows.surface);
        // SAFETY: the texture is the context's, which is current if it is to be deleted; the
        // surface is released before it is destroyed.
        unsafe {
            if current && shows.texture != 0 {
                glDeleteTextures(1, &shows.texture);
            }
            eglMakeCurrent(self.display, null_mut(), null_mut(), null_mut());
            eglDestroySurface(self.display, shows.surface);
        }
        self.current = null_mut();
    }

    /// Tests: whether it has drawn the window itself (see `gpu_draw`).
    #[cfg(all(test, feature = "gpu-draw"))]
    pub(crate) fn has_drawn(&self) -> bool {
        matches!(self.pipeline, Some(Some(_)))
    }

    /// Tests: the places the last frame named, in the texture.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        &shows.named
    }

    /// Tests: the device is lost, as when it is removed: its context is destroyed under it.
    #[cfg(test)]
    pub(crate) fn lose(&mut self) {
        // SAFETY: the context is the display's; it is not used again (every later call fails).
        unsafe {
            eglMakeCurrent(self.display, null_mut(), null_mut(), null_mut());
            eglDestroyContext(self.display, self.context);
        }
        (self.current, self.context) = (null_mut(), null_mut());
    }
}

impl Drop for Gpu {
    /// Lets go of the context, and with it all it holds, and of the display. (The window was let
    /// go of first.)
    fn drop(&mut self) {
        // SAFETY: EGL's documented calls on the display; nothing of it is used after.
        unsafe {
            eglMakeCurrent(self.display, null_mut(), null_mut(), null_mut());
            if !self.context.is_null() {
                eglDestroyContext(self.display, self.context);
            }
            eglTerminate(self.display);
            eglReleaseThread();
        }
    }
}

/// A compiled shader (0 where it didn't compile).
fn shader(kind: c_uint, source: &CStr) -> c_uint {
    let mut compiled = 0;
    // SAFETY: OpenGL ES's documented calls on the current context; the source ends in a zero.
    unsafe {
        let shader = glCreateShader(kind);
        glShaderSource(shader, 1, &source.as_ptr(), null());
        glCompileShader(shader);
        glGetShaderiv(shader, COMPILE_STATUS, &mut compiled);
        if compiled != 0 { shader } else { 0 }
    }
}
