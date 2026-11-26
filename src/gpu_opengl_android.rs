//! Android windows shown through OpenGL ES 3 and EGL, with the feature `gpu`, where Vulkan is
//! missing or asked against. The CPU draws every pixel as without it; each place it drew goes
//! into the window's texture (`glTexSubImage2D`; with `gpu-draw` the GPU draws large parts into
//! it itself). A frame clears the window to the theme's background where the texture leaves
//! room (the insets), draws the texture texel for pixel inside them, and swaps naming the places
//! that changed (`eglSwapBuffersWithDamageKHR`), in step with the screen (swap interval 1). The
//! pixels shown are the CPU's, byte for byte.
//!
//! - The surface is the activity's window; it goes with the window, the context stays.
//! - A swap that finds the window gone says that.
//! - libEGL and libGLESv3 are opened by name when the window first asks for the GPU (`gpu_egl`,
//!   which holds what X11's path shares).
//!
//! `Auto` takes the GPU only where it is not the CPU at work (SwiftShader, llvmpipe).
use crate::android_sys::RGBA_8888;
use crate::dynamic_library::library;
use crate::gpu_android::Block;
use crate::gpu_egl::*;
use std::ffi::{CStr, c_int, c_uint, c_void};
use std::ptr::{null, null_mut};

library!(AndroidEgl, ["libEGL.so"], {
    fn eglGetDisplay(*mut c_void) -> Handle;
    fn eglCreateWindowSurface(Handle, Handle, *mut c_void, *const c_int) -> Handle;
    fn eglGetError() -> c_int;
});

library!(AndroidGles, ["libGLESv3.so"], {
    fn glUniform2i(c_int, c_int, c_int);
    fn glClearColor(f32, f32, f32, f32);
    fn glClear(c_uint);
});

// From EGL's and OpenGL ES 3's headers (egl.h, gl3.h).
const ALPHA_SIZE: c_int = 0x3021;
/// What a failed swap says of the window: `EGL_BAD_NATIVE_WINDOW`, `EGL_BAD_SURFACE`.
const WINDOW_GONE: [c_int; 2] = [0x300B, 0x300D];
const UNPACK_SKIP_ROWS: c_uint = 0x0CF3;
const BLEND: c_uint = 0x0BE2;
const COLOR_BUFFER_BIT: c_uint = 0x4000;

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

/// The GPU the window is shown through: EGL's display, one OpenGL ES 3 context, and the program
/// drawing the window's texture.
pub(crate) type Gpu = crate::gpu_egl::Gpu<Android>;

/// The window shown through the GPU.
pub(crate) type Shows = crate::gpu_egl::Shows<Frame>;

/// What the window keeps of EGL: its functions for Android's windows, and for clearing around
/// the texture and placing it; where the program's `at` is (where the texture lies in the
/// window).
pub(crate) struct Android {
    egl: AndroidEgl,
    gl: AndroidGles,
    at: c_int,
}

/// What the window keeps of its frames. The texture is new: all of the window is to be drawn
/// into it. The window's size, where the texture lies in it (left, top), and the colour around
/// it. The places changed since the last frame (left, top, right, bottom in the texture);
/// everything is to be named in the next swap (`whole`).
pub(crate) struct Frame {
    new: bool,
    window: [c_int; 2],
    at: [c_int; 2],
    background: u32,
    changed: Vec<[c_int; 4]>,
    whole: bool,
    /// The window went: its surface is to be let go.
    gone: bool,
    /// Tests: the places the last frame named.
    #[cfg(test)]
    named: Vec<[c_int; 4]>,
}

impl Default for Frame {
    fn default() -> Frame {
        Frame {
            new: true,
            window: [0; 2],
            at: [0; 2],
            background: 0,
            changed: Vec::new(),
            whole: true,
            gone: false,
            #[cfg(test)]
            named: Vec::new(),
        }
    }
}

impl Shows {
    /// Whether all of the window is to be drawn (its texture is new); asked once.
    pub(crate) fn take_whole(&mut self) -> bool {
        std::mem::take(&mut self.system.new)
    }

    /// Whether the window went: its surface is to be let go.
    pub(crate) fn gone(&self) -> bool {
        self.system.gone
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
        let (egl, gl) = (AndroidEgl::open()?, AndroidGles::open()?);
        // SAFETY: EGL's documented call, on the default display.
        let display = unsafe { (egl.eglGetDisplay)(null_mut()) };
        if display.is_null() {
            return None;
        }
        // (`ANativeWindow`'s 8-bit red, green, blue and alpha.)
        let wanted = [SURFACE_TYPE, WINDOW_BIT, RENDERABLE_TYPE, OPENGL_ES3_BIT, RED_SIZE, 8, GREEN_SIZE, 8, BLUE_SIZE, 8, ALPHA_SIZE, 8, NONE];
        let mut gpu = Gpu::make(display, (&wanted, RGBA_8888 as usize), Android { egl, gl, at: -1 })?;
        // SAFETY: as the caller promises.
        let shows = unsafe { gpu.window(window) }?;
        let software = gpu.software();
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !software, crate::platform::setting("TAUT_DRAW").as_deref());
        }
        if (any || !software) && gpu.program(TEXEL) {
            // SAFETY: OpenGL ES's documented call on the current context; the name ends in a zero.
            gpu.system.at = unsafe { (gpu.gl.glGetUniformLocation)(gpu.program, c"at".as_ptr()) };
            return Some((gpu, shows));
        }
        gpu.forget(shows);
        None
    }

    /// Shows `window` (an `ANativeWindow`) through the device: its surface, made current, swapping
    /// in step with the screen; sized by `fit`. Nothing where it can't be made.
    ///
    /// # Safety
    /// As `open`.
    pub(crate) unsafe fn window(&mut self, window: *mut c_void) -> Option<Shows> {
        // SAFETY: the window is alive (the caller's promise).
        let surface = unsafe { (self.system.egl.eglCreateWindowSurface)(self.display, self.config, window, null()) };
        if surface.is_null() {
            return None;
        }
        self.shows(surface, Frame::default())
    }

    /// Gives the window its size (`window` pixels), and its texture its own (`size`, at `at` in
    /// the window): a new texture, all of which is to be drawn, where `size` changed. Says
    /// whether it has it: where the device makes none that big, the window is better shown by
    /// the CPU.
    pub(crate) fn fit(&mut self, shows: &mut Shows, window: [c_int; 2], size: [c_int; 2], at: [c_int; 2]) -> bool {
        let frame = &mut shows.system;
        if (frame.window, frame.at) != (window, at) {
            (frame.window, frame.at, frame.whole) = (window, at, true);
        }
        if shows.size == size && shows.texture != 0 {
            return true;
        }
        let made = self.texture(shows, size);
        let frame = &mut shows.system;
        (frame.new, frame.whole) = (true, true);
        frame.changed.clear();
        made
    }

    /// Puts `place` of a block of pixels (left, top, right, bottom) into the window's texture at
    /// the same place, as far as both reach, to be shown with the next frame.
    pub(crate) fn put(&mut self, shows: &mut Shows, block: &Block, place: [c_int; 4]) {
        let Some([left, top, right, bottom]) = block.cut(place, shows.size).filter(|_| self.current(shows.surface)) else { return };
        let gl = &self.gl;
        // SAFETY: the block holds the place's rows (`cut`), `width` pixels each; OpenGL ES copies
        // them out before it returns.
        unsafe {
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glPixelStorei)(UNPACK_ROW_LENGTH, block.width as c_int);
            (gl.glPixelStorei)(UNPACK_SKIP_PIXELS, left - block.at[0]);
            (gl.glPixelStorei)(UNPACK_SKIP_ROWS, top - block.at[1]);
            (gl.glTexSubImage2D)(TEXTURE_2D, 0, left, top, right - left, bottom - top, RGBA, UNSIGNED_BYTE, block.pixels.as_ptr().cast());
            [UNPACK_ROW_LENGTH, UNPACK_SKIP_PIXELS, UNPACK_SKIP_ROWS].into_iter().for_each(|name| (gl.glPixelStorei)(name, 0));
        }
        shows.system.changed.push([left, top, right, bottom]);
    }

    /// Draws a recorded paint into the window's texture, inside `place` (left, top, right,
    /// bottom), to be shown with the next frame; what it draws with is made the first time.
    /// Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        let drawn = self.draw_into(shows, list, place);
        if drawn {
            shows.system.changed.push(place);
        }
        drawn
    }

    /// Shows the window: cleared to `background` (`0xAARRGGBB`) where the texture leaves room,
    /// the texture drawn inside, swapped in naming what changed. With nothing changed, only
    /// where `again` (the system asks for the window whole). A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows, background: u32, again: bool) {
        let frame = &mut shows.system;
        frame.whole |= again || std::mem::replace(&mut frame.background, background) != background;
        if frame.changed.is_empty() && !frame.whole || !self.current(shows.surface) {
            return;
        }
        let ([width, height], [left, top], [wide, high], gl) = (shows.system.window, shows.system.at, shows.size, &self.gl);
        let [red, green, blue] = [16, 8, 0].map(|shift| ((background >> shift) & 0xff) as f32 / 255.0);
        // SAFETY: OpenGL ES's documented calls on the current context; what drawing put in
        // context state is set again.
        unsafe {
            (gl.glDisable)(SCISSOR_TEST);
            (gl.glDisable)(BLEND);
            (gl.glDisable)(DITHER);
            (gl.glViewport)(0, 0, width, height);
            (self.system.gl.glClearColor)(red, green, blue, 1.0);
            (self.system.gl.glClear)(COLOR_BUFFER_BIT);
            (gl.glViewport)(left, height - top - high, wide, high);
            (gl.glUseProgram)(self.program);
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (self.system.gl.glUniform2i)(self.system.at, left, top);
            (gl.glUniform1i)(self.height, height);
            (gl.glDrawArrays)(TRIANGLES, 0, 3);
        }
        self.swap_named(shows);
    }

    /// Swaps the window's back buffer in, naming the places that changed (all of it after a
    /// new size, texture or background).
    fn swap_named(&mut self, shows: &mut Shows) {
        let ([left, top], height) = (shows.system.at, shows.system.window[1]);
        let changed = std::mem::take(&mut shows.system.changed);
        let named = !std::mem::take(&mut shows.system.whole);
        let damage: Vec<c_int> = changed.iter().flat_map(|[l, t, r, b]| [l + left, height - (b + top), r - l, b - t]).collect();
        let swapped = self.swap(shows, named.then_some(damage.as_slice()));
        // SAFETY: EGL's own question, of this thread's last call.
        let error = if swapped { 0 } else { unsafe { (self.system.egl.eglGetError)() } };
        shows.system.gone |= WINDOW_GONE.contains(&error);
        self.lost |= !swapped && !shows.system.gone;
        #[cfg(test)]
        {
            let window = shows.system.window;
            shows.system.named = if named { changed } else { vec![[0, 0, window[0] - left, window[1] - top]] };
        }
    }

    /// Tests: the places the last frame named, in the texture.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        &shows.system.named
    }
}
