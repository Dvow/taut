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
//! libEGL and libGLESv2 are opened by name when a window first asks for the GPU (`gpu_egl`,
//! which holds what Android's path shares).
//!
//! `Auto` takes the GPU only where it shows frames directly: a hardware device, presenting
//! through DRI3 (or through its own vendor's EGL, as NVIDIA's). Elsewhere (a software device
//! such as llvmpipe; an X server without DRI3, such as Xvfb, VNC or a VM's) Mesa copies every
//! frame whole through X, which costs more than the CPU's own drawing: the CPU shows the
//! windows. Any other choice of the GPU takes the device either way.
use crate::gpu_egl::*;
use crate::gpu_x11::{KEPT, has_dri3, note, places};
use std::ffi::{CStr, c_int, c_uint, c_ulong, c_void};
use std::ptr::null;

system_library!(PlatformEgl, "EGL", ["libEGL.so.1", "libEGL.so"], {
    fn eglGetPlatformDisplay(c_uint, *mut c_void, *const isize) -> Handle;
    fn eglCreatePlatformWindowSurface(Handle, Handle, *mut c_void, *const isize) -> Handle;
    fn eglQuerySurface(Handle, Handle, c_int, *mut c_int) -> c_uint;
});

system_library!(Scissors, "GLESv3", ["libGLESv2.so.2", "libGLESv2.so"], {
    fn glEnable(c_uint);
    fn glScissor(c_int, c_int, c_int, c_int);
});

// From EGL's headers (egl.h, eglext.h).
const PLATFORM_X11: c_uint = 0x31D5;
const BUFFER_AGE: c_int = 0x313D;
const VENDOR: c_int = 0x3053;

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

/// The GPU a program's windows are shown through: EGL's display on the X connection, one
/// OpenGL ES 3 context for all of them, and the program drawing a window's texture.
pub(crate) type Gpu = crate::gpu_egl::Gpu<X11>;

/// A window shown through the GPU.
pub(crate) type Shows = crate::gpu_egl::Shows<Frames>;

/// What X windows keep of EGL: its functions for X's displays and windows, whether the display
/// tells the age of back buffers, and the scissor drawing only what changed.
pub(crate) struct X11 {
    platform: PlatformEgl,
    scissors: Scissors,
    aged: bool,
}

/// The places put into a window's texture since it was last shown, and those shown in the last
/// frames, newest first: left, top, right, bottom, in pixels from the top left.
#[derive(Default)]
pub(crate) struct Frames {
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
        let (egl, gl, platform, scissors) = (Egl::open()?, Gles::open()?, PlatformEgl::open()?, Scissors::open()?);
        // SAFETY: EGL's documented call, for an X display (EGL_EXT_platform_x11). A `Visual`
        // starts with a pointer, then its id (Xlib.h).
        let (display, visual) = unsafe { ((platform.eglGetPlatformDisplay)(PLATFORM_X11, x, null()), *visual.cast::<c_ulong>().add(1)) };
        if display.is_null() {
            return None;
        }
        let wanted = [SURFACE_TYPE, WINDOW_BIT, RENDERABLE_TYPE, OPENGL_ES3_BIT, RED_SIZE, 8, GREEN_SIZE, 8, BLUE_SIZE, 8, NONE];
        let mut gpu = Gpu::make(egl, gl, display, (&wanted, visual as usize), X11 { platform, scissors, aged: false })?;
        gpu.system.aged = gpu.offers("EGL_EXT_buffer_age");
        let shows = gpu.window(id, size)?;
        #[cfg(feature = "gpu-draw")]
        {
            gpu.drawing = crate::gpu_draw::Drawing::chosen(!any, !gpu.software(), std::env::var("TAUT_DRAW").ok().as_deref());
        }
        // SAFETY: the display is open (the caller's promise).
        if (any || unsafe { gpu.direct(x) }) && gpu.program(TEXEL) {
            // SAFETY: OpenGL ES's documented calls on the current context.
            unsafe {
                (gpu.gl.glDisable)(DITHER);
                (gpu.system.scissors.glEnable)(SCISSOR_TEST);
            }
            return Some((gpu, shows));
        }
        gpu.forget(shows);
        None
    }

    /// Whether frames are shown directly: a hardware device, and X display `x` offers DRI3
    /// where the EGL is Mesa's.
    ///
    /// # Safety
    /// `x` is an open X display.
    pub(crate) unsafe fn direct(&self, x: *mut c_void) -> bool {
        let vendor = self.said(VENDOR);
        let mesa = vendor.is_empty() || vendor.starts_with(b"Mesa");
        // SAFETY: as the caller promises.
        !self.software() && (!mesa || unsafe { has_dri3(x) })
    }

    /// Shows window `id` of the display through the device: its surface, made current, and a
    /// texture `size` pixels big. Nothing where either can't be made.
    pub(crate) fn window(&mut self, id: c_ulong, size: [c_int; 2]) -> Option<Shows> {
        let mut native = id;
        // SAFETY: for X, EGL reads the window's id through the pointer, during the call
        // (EGL_EXT_platform_x11).
        let surface = unsafe { (self.system.platform.eglCreatePlatformWindowSurface)(self.display, self.config, (&raw mut native).cast(), null()) };
        if surface.is_null() {
            return None;
        }
        let mut shows = self.shows(surface, Frames::default())?;
        if self.fit(&mut shows, size) {
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
        shows.system.past = Vec::new();
        self.texture(shows, size)
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
        note(&mut shows.system.put, place);
    }

    /// Draws a recorded paint into a window's texture, inside `place` (left, top, right,
    /// bottom), to be shown with what was put in; what it draws with is made the first time.
    /// Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, shows: &mut Shows, list: &crate::gpu_draw::DrawList, place: [c_int; 4]) -> bool {
        let drawn = self.draw_into(shows, list, place);
        if drawn {
            note(&mut shows.system.put, place);
        }
        drawn
    }

    /// Shows what was put into a window's texture since it was last shown: drawn into the back
    /// buffer where it changed (and where an older back buffer lacks), and swapped in naming
    /// those places. A device that fails is lost.
    pub(crate) fn show(&mut self, shows: &mut Shows) {
        if shows.system.put.is_empty() || !self.current(shows.surface) {
            return;
        }
        let ([width, height], age) = (shows.size, self.age(shows));
        let wanted = places(&shows.system.put, &shows.system.past, age.checked_sub(1), shows.size);
        let (gl, scissor) = (&self.gl, self.system.scissors.glScissor);
        // SAFETY: OpenGL ES's documented calls on the current context, with the program in use.
        unsafe {
            (gl.glViewport)(0, 0, width, height);
            (gl.glBindTexture)(TEXTURE_2D, shows.texture);
            (gl.glUniform1i)(self.height, height);
            for [left, top, right, bottom] in wanted {
                scissor(left, height - bottom, right - left, bottom - top);
                (gl.glDrawArrays)(TRIANGLES, 0, 3);
            }
        }
        let damage: Vec<c_int> = shows.system.put.iter().flat_map(|[left, top, right, bottom]| [*left, height - bottom, right - left, bottom - top]).collect();
        self.lost |= !self.swap(shows, Some(&damage));
        let frames = &mut shows.system;
        frames.past.insert(0, std::mem::take(&mut frames.put));
        frames.past.truncate(KEPT);
    }

    /// How many frames old the current back buffer of a window is (0: unknown).
    fn age(&self, shows: &Shows) -> usize {
        let mut age = 0;
        // SAFETY: the surface is the display's and current; EGL fills in one number.
        if self.system.aged && unsafe { (self.system.platform.eglQuerySurface)(self.display, shows.surface, BUFFER_AGE, &mut age) } == 0 {
            return 0;
        }
        age.max(0) as usize
    }

    /// Tests: the places the last frame named.
    #[cfg(test)]
    pub(crate) fn named(shows: &Shows) -> &[[c_int; 4]] {
        shows.system.past.first().map_or(&[], Vec::as_slice)
    }
}
