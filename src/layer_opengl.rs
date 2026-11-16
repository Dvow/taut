//! The UI over a program's own OpenGL rendering (3.3 or newer, core or compatibility profile):
//! created once in its context, then drawn each frame with one call, after the program's own
//! drawing.
//!
//! ```ignore
//! // SAFETY: the program's context is current, and `find` gives its functions by name.
//! let mut layer = unsafe { taut::layer_opengl::Layer::new(&mut |name| find(name)) }?;
//! // each frame, once the program's picture is in the framebuffer it has bound:
//! unsafe { layer.draw(&mut ui) }?;
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`) and a texture of the same size. `draw` redraws
//! only what changed since the last call, uploads only that, and draws the texture as one quad
//! over the bound framebuffer, at the viewport's top left, clipped to it, pixel for pixel, with
//! premultiplied alpha blending ("one, one minus source alpha"). Everything it sets (program,
//! texture, blend, depth, stencil, culling, scissor, viewport, unpack state) is restored. A
//! program that shows the UI otherwise (on a surface in its 3D world, say) calls `update` and
//! draws `texture()` itself.
//!
//! Functions are loaded by name through the program's loader (`wglGetProcAddress`,
//! `eglGetProcAddress`, `glXGetProcAddress`, or its window library's); on Windows the OpenGL
//! 1.1 ones, which `wglGetProcAddress` doesn't return, come from opengl32.dll. Each is declared
//! as glcorearb.h declares it. The shaders are compiled by the program's OpenGL when the layer
//! is created: if its compiler rejects them, `new` returns its log.
//!
//! Every call needs the layer's context current on the thread: OpenGL cannot be asked which is,
//! so the calls are `unsafe`.
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::null;

/// An OpenGL error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    /// What the layer was doing, as "compiling the vertex shader".
    pub doing: &'static str,
    /// What OpenGL said: its compiler log, or its version and limits.
    pub said: String,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.doing, self.said)
    }
}

impl std::error::Error for Failed {}

/// Declares the OpenGL functions the layer calls, loaded by name.
macro_rules! functions {
    ($($name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)*) => {
        /// The OpenGL functions the layer calls.
        #[allow(non_snake_case)]
        struct Gl {
            $($name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)*
        }
        impl Gl {
            /// Each function, loaded through `find`; or the name of the first missing one.
            fn load(find: &mut dyn FnMut(&CStr) -> *const c_void) -> Result<Gl, Failed> {
                // SAFETY: each function is declared as OpenGL declares it.
                Ok(Gl { $($name: unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn($($arg),*) $(-> $out)?>(found(find, concat!(stringify!($name), "\0"))?) },)* })
            }
        }
    };
}

functions! {
    glGetIntegerv: fn(u32, *mut i32);
    glGetString: fn(u32) -> *const c_char;
    glIsEnabled: fn(u32) -> u8;
    glEnable: fn(u32);
    glDisable: fn(u32);
    glGenTextures: fn(i32, *mut u32);
    glDeleteTextures: fn(i32, *const u32);
    glBindTexture: fn(u32, u32);
    glActiveTexture: fn(u32);
    glTexParameteri: fn(u32, u32, i32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexSubImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glPixelStorei: fn(u32, i32);
    glBindBuffer: fn(u32, u32);
    glCreateShader: fn(u32) -> u32;
    glShaderSource: fn(u32, i32, *const *const c_char, *const i32);
    glCompileShader: fn(u32);
    glGetShaderiv: fn(u32, u32, *mut i32);
    glGetShaderInfoLog: fn(u32, i32, *mut i32, *mut c_char);
    glDeleteShader: fn(u32);
    glCreateProgram: fn() -> u32;
    glAttachShader: fn(u32, u32);
    glLinkProgram: fn(u32);
    glGetProgramiv: fn(u32, u32, *mut i32);
    glGetProgramInfoLog: fn(u32, i32, *mut i32, *mut c_char);
    glUseProgram: fn(u32);
    glDeleteProgram: fn(u32);
    glGenVertexArrays: fn(i32, *mut u32);
    glBindVertexArray: fn(u32);
    glDeleteVertexArrays: fn(i32, *const u32);
    glBlendFuncSeparate: fn(u32, u32, u32, u32);
    glBlendEquationSeparate: fn(u32, u32);
    glPolygonMode: fn(u32, u32);
    glViewport: fn(i32, i32, i32, i32);
    glScissor: fn(i32, i32, i32, i32);
    glDrawArrays: fn(u32, i32, i32);
}

/// The function `name` (NUL-terminated) as `find` returns it; on Windows the OpenGL 1.1 ones
/// from opengl32.dll, for which the driver returns nothing (or 1, 2, 3 or -1).
fn found(find: &mut dyn FnMut(&CStr) -> *const c_void, name: &'static str) -> Result<*const c_void, Failed> {
    let name = CStr::from_bytes_with_nul(name.as_bytes()).unwrap_or_default();
    #[allow(unused_mut)]
    let mut found = find(name);
    #[cfg(windows)]
    if matches!(found as isize, -1..=3) {
        let library: Vec<u16> = "opengl32.dll\0".encode_utf16().collect();
        // SAFETY: both names are NUL-terminated.
        found = unsafe { GetProcAddress(GetModuleHandleW(library.as_ptr()), name.as_ptr().cast()) } as *const c_void;
    }
    match found.is_null() {
        true => Err(Failed { doing: "finding OpenGL's functions", said: format!("this OpenGL has no {}", name.to_string_lossy()) }),
        false => Ok(found),
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetProcAddress(library: isize, name: *const u8) -> usize;
}

// OpenGL constants, as glcorearb.h has them.
const TEXTURE_2D: u32 = 0x0DE1;
const TEXTURE0: u32 = 0x84C0;
const RGBA8: i32 = 0x8058;
const BGRA: u32 = 0x80E1;
const UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;
const PIXEL_UNPACK_BUFFER: u32 = 0x88EC;
const UNPACK_ROW_LENGTH: u32 = 0x0CF2;
const UNPACK_SKIP_ROWS: u32 = 0x0CF3;
const UNPACK_SKIP_PIXELS: u32 = 0x0CF4;
const UNPACK_ALIGNMENT: u32 = 0x0CF5;
const BLEND: u32 = 0x0BE2;
const DEPTH_TEST: u32 = 0x0B71;
const STENCIL_TEST: u32 = 0x0B90;
const CULL_FACE: u32 = 0x0B44;
const SCISSOR_TEST: u32 = 0x0C11;
const FRAMEBUFFER_SRGB: u32 = 0x8DB9;
const FRONT_AND_BACK: u32 = 0x0408;
const ONE: u32 = 1;
const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const FUNC_ADD: u32 = 0x8006;
const FILL: u32 = 0x1B02;
const TRIANGLE_STRIP: u32 = 5;
const VERTEX_SHADER: u32 = 0x8B31;
const FRAGMENT_SHADER: u32 = 0x8B30;

/// What `draw` enables (`true`) and disables: blending; depth test, stencil test, face culling,
/// scissor test, and sRGB framebuffer conversion (the UI's pixels are already in screen
/// colours).
const SWITCHES: [(u32, bool); 6] = [(BLEND, true), (DEPTH_TEST, false), (STENCIL_TEST, false), (CULL_FACE, false), (SCISSOR_TEST, true), (FRAMEBUFFER_SRGB, false)];

/// The quad over the viewport, built from vertex ids (no vertex buffer), with each pixel's
/// texture position from the top left.
const CORNERS: &CStr = c"#version 330 core
uniform sampler2D picture;
out vec2 place;
void main() {
    vec2 corner = vec2(gl_VertexID & 1, gl_VertexID >> 1);
    place = corner * vec2(textureSize(picture, 0));
    gl_Position = vec4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
}
";

/// Each pixel loads the texel at the same place, unfiltered: no sampler matters.
const PIXELS: &CStr = c"#version 330 core
uniform sampler2D picture;
in vec2 place;
out vec4 colour;
void main() {
    colour = texelFetch(picture, ivec2(place), 0);
}
";

/// The UI in a program's OpenGL: its pixels, a texture of their size, and what draws it over
/// the program's picture.
pub struct Layer {
    gl: Gl,
    program: u32,
    /// The vertex array bound while drawing the quad: empty, but the core profile draws nothing
    /// without one.
    corners: u32,
    texture: u32,
    /// The largest texture size this OpenGL supports.
    most: usize,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).field("most", &self.most).finish_non_exhaustive()
    }
}

impl Layer {
    /// The layer, in the current context, whose functions `find` returns by name
    /// (NUL-terminated).
    ///
    /// # Safety
    /// A context is current on this thread, and `find` returns its functions.
    pub unsafe fn new(find: &mut dyn FnMut(&CStr) -> *const c_void) -> Result<Layer, Failed> {
        let gl = Gl::load(find)?;
        // 0x821B, 0x821C: GL_MAJOR_VERSION, GL_MINOR_VERSION; 0x0D33: GL_MAX_TEXTURE_SIZE.
        let [major, minor, most] = read(&gl, [0x821B, 0x821C, 0x0D33]);
        if (major, minor) < (3, 3) {
            // SAFETY: OpenGL returns its version as NUL-terminated text, or null.
            let version = unsafe { (gl.glGetString)(0x1F02) };
            let said = match version.is_null() {
                true => "this OpenGL does not say its version".to_string(),
                false => format!("version 3.3 or newer is needed, and this is {}", unsafe { CStr::from_ptr(version) }.to_string_lossy()),
            };
            return Err(Failed { doing: "making the layer", said });
        }
        let program = linked(&gl)?;
        let mut layer = Layer { gl, program, corners: 0, texture: 0, most: most.max(0) as usize, surface: Surface::default() };
        let gl = &layer.gl;
        // SAFETY: the names are created here and kept. The texture has one level, complete with
        // any sampler a program binds (0x813D: GL_TEXTURE_MAX_LEVEL); unfiltered (0x2801,
        // 0x2800: GL_TEXTURE_MIN/MAG_FILTER, 0x2600: GL_NEAREST). The binding is restored.
        unsafe {
            (gl.glGenVertexArrays)(1, &mut layer.corners);
            (gl.glGenTextures)(1, &mut layer.texture);
            let [bound] = read(gl, [0x8069]);
            (gl.glBindTexture)(TEXTURE_2D, layer.texture);
            [(0x813D, 0), (0x2801, 0x2600), (0x2800, 0x2600)].into_iter().for_each(|(what, to)| (gl.glTexParameteri)(TEXTURE_2D, what, to));
            (gl.glBindTexture)(TEXTURE_2D, bound as u32);
        }
        Ok(layer)
    }

    /// Updates the texture after a run: redraws what changed and uploads only that. (The first
    /// time, and after a resize, the texture is reallocated and all of it uploaded.)
    ///
    /// # Safety
    /// The layer's context is current on this thread.
    pub unsafe fn update(&mut self, ui: &mut Ui) -> Result<(), Failed> {
        let size = (self.surface.width, self.surface.height);
        let changed = self.surface.update(ui);
        let (width, height, gl) = (self.surface.width, self.surface.height, &self.gl);
        if (width, height) != size && width.max(height) > self.most {
            let said = format!("the interface is {width} by {height} pixels, and this OpenGL makes textures of {} at most", self.most);
            self.surface = Surface::default(); // redrawn whole, and retried, next time
            return Err(Failed { doing: "making the texture", said });
        }
        // The pixel unpack state that is changed, saved to restore.
        let kept = read(gl, [0x8069, 0x88EF, UNPACK_ROW_LENGTH, UNPACK_SKIP_PIXELS, UNPACK_SKIP_ROWS, UNPACK_ALIGNMENT]);
        let unpacking = [(UNPACK_ROW_LENGTH, width as i32), (UNPACK_SKIP_PIXELS, 0), (UNPACK_SKIP_ROWS, 0), (UNPACK_ALIGNMENT, 4)];
        // SAFETY: the context is current; each box lies in the texture, which is the surface's
        // size, so every row read lies in the surface's pixels, one surface row apart. Each
        // pixel is one `0xAARRGGBB` word, which is GL_BGRA with GL_UNSIGNED_INT_8_8_8_8_REV:
        // the same on either byte order.
        unsafe {
            (gl.glBindTexture)(TEXTURE_2D, self.texture);
            (gl.glBindBuffer)(PIXEL_UNPACK_BUFFER, 0);
            unpacking.into_iter().for_each(|(what, to)| (gl.glPixelStorei)(what, to));
            if (width, height) != size && width * height > 0 {
                (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA8, width as i32, height as i32, 0, BGRA, UNSIGNED_INT_8_8_8_8_REV, self.surface.pixels.as_ptr().cast());
            }
            for [left, top, right, bottom] in changed.into_iter().filter(|_| (width, height) == size) {
                let first: *const u32 = &self.surface.pixels[top * width + left];
                (gl.glTexSubImage2D)(TEXTURE_2D, 0, left as i32, top as i32, (right - left) as i32, (bottom - top) as i32, BGRA, UNSIGNED_INT_8_8_8_8_REV, first.cast());
            }
            (gl.glBindTexture)(TEXTURE_2D, kept[0] as u32);
            (gl.glBindBuffer)(PIXEL_UNPACK_BUFFER, kept[1] as u32);
            unpacking.into_iter().zip(&kept[2..]).for_each(|((what, _), was)| (gl.glPixelStorei)(what, *was));
        }
        Ok(())
    }

    /// Updates the texture (`update`) and draws it over the bound framebuffer: at the
    /// viewport's top left, pixel for pixel, clipped to the viewport. Everything it sets is
    /// restored.
    ///
    /// # Safety
    /// The layer's context is current on this thread.
    pub unsafe fn draw(&mut self, ui: &mut Ui) -> Result<(), Failed> {
        // SAFETY: as the caller promises.
        unsafe { self.update(ui) }?;
        if self.surface.pixels.is_empty() {
            return Ok(());
        }
        let (gl, width, height) = (&self.gl, self.surface.width as i32, self.surface.height as i32);
        let kept = Kept::take(gl);
        let [x, y, wide, high] = kept.viewport;
        // SAFETY: the context is current, and the names are this layer's. (OpenGL counts rows
        // from the bottom: the viewport's top is its y plus its height.)
        unsafe {
            (gl.glViewport)(x, y + high - height, width, height);
            (gl.glScissor)(x, y, wide, high);
            SWITCHES.into_iter().for_each(|(what, on)| [gl.glDisable, gl.glEnable][on as usize](what));
            (gl.glBlendFuncSeparate)(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
            (gl.glBlendEquationSeparate)(FUNC_ADD, FUNC_ADD);
            (gl.glPolygonMode)(FRONT_AND_BACK, FILL);
            (gl.glUseProgram)(self.program);
            (gl.glBindVertexArray)(self.corners);
            (gl.glActiveTexture)(TEXTURE0);
            (gl.glBindTexture)(TEXTURE_2D, self.texture);
            (gl.glDrawArrays)(TRIANGLE_STRIP, 0, 4);
        }
        kept.put_back(gl);
        Ok(())
    }

    /// The texture name (`GL_TEXTURE_2D`, `GL_RGBA8`, premultiplied alpha, rows from the top),
    /// as `update` left it.
    pub fn texture(&self) -> u32 {
        self.texture
    }

    /// The UI's pixels, which the texture holds.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Deletes the layer's OpenGL objects. (A layer only dropped leaves them to the context,
    /// which frees them when destroyed.)
    ///
    /// # Safety
    /// The layer's context is current on this thread.
    pub unsafe fn delete(self) {
        let gl = &self.gl;
        // SAFETY: the names are this layer's and are not used after this.
        unsafe {
            (gl.glDeleteTextures)(1, &self.texture);
            (gl.glDeleteVertexArrays)(1, &self.corners);
            (gl.glDeleteProgram)(self.program);
        }
    }
}

/// The current values of `names`.
fn read<const N: usize>(gl: &Gl, names: [u32; N]) -> [i32; N] {
    let mut values = [0; N];
    // SAFETY: the context is current (as every caller promises); each name has one value. (An
    // unknown name leaves its 0.)
    unsafe { names.iter().zip(&mut values).for_each(|(name, value)| (gl.glGetIntegerv)(*name, value)) };
    values
}

/// The shaders, compiled and linked; or the compiler's log.
fn linked(gl: &Gl) -> Result<u32, Failed> {
    let corners = compiled(gl, VERTEX_SHADER, CORNERS, "compiling the vertex shader")?;
    let pixels = compiled(gl, FRAGMENT_SHADER, PIXELS, "compiling the fragment shader");
    let mut done = 0;
    // SAFETY: the shaders were just created; the program is created here, and its log read into
    // a buffer of the stated length.
    unsafe {
        let pixels = pixels.inspect_err(|_| (gl.glDeleteShader)(corners))?;
        let program = (gl.glCreateProgram)();
        [corners, pixels].into_iter().for_each(|shader| (gl.glAttachShader)(program, shader));
        (gl.glLinkProgram)(program);
        [corners, pixels].into_iter().for_each(|shader| (gl.glDeleteShader)(shader));
        (gl.glGetProgramiv)(program, 0x8B82, &mut done);
        if done != 0 {
            return Ok(program);
        }
        let said = log(gl.glGetProgramiv, gl.glGetProgramInfoLog, program);
        (gl.glDeleteProgram)(program);
        Err(Failed { doing: "linking the shaders", said })
    }
}

/// One shader, compiled; or the compiler's log.
fn compiled(gl: &Gl, kind: u32, source: &CStr, doing: &'static str) -> Result<u32, Failed> {
    let mut done = 0;
    // SAFETY: the source is NUL-terminated; the shader is created here.
    unsafe {
        let shader = (gl.glCreateShader)(kind);
        (gl.glShaderSource)(shader, 1, &source.as_ptr(), null());
        (gl.glCompileShader)(shader);
        (gl.glGetShaderiv)(shader, 0x8B81, &mut done);
        if done != 0 {
            return Ok(shader);
        }
        let said = log(gl.glGetShaderiv, gl.glGetShaderInfoLog, shader);
        (gl.glDeleteShader)(shader);
        Err(Failed { doing, said })
    }
}

/// The info log of a shader or program (`length` reads its length, `text` the log).
///
/// # Safety
/// The context is current, and `name` is of the kind both functions take.
unsafe fn log(length: unsafe extern "system" fn(u32, u32, *mut i32), text: unsafe extern "system" fn(u32, i32, *mut i32, *mut c_char), name: u32) -> String {
    let (mut long, mut written) = (0, 0);
    // SAFETY: as the caller promises; the buffer has the length OpenGL reported (0x8B84:
    // GL_INFO_LOG_LENGTH), and it returns how much it wrote.
    unsafe {
        length(name, 0x8B84, &mut long);
        let mut said = vec![0u8; long.max(1) as usize];
        text(name, said.len() as i32, &mut written, said.as_mut_ptr().cast());
        said.truncate(written.clamp(0, long.max(0)) as usize);
        String::from_utf8_lossy(&said).trim_end().to_string()
    }
}

/// The state `draw` changes, as the program had it, to restore after.
struct Kept {
    /// Each of `SWITCHES`, enabled or not.
    switches: [bool; 6],
    viewport: [i32; 4],
    scissor: [i32; 4],
    /// Blend factors (source and destination for colour and alpha), and the colour and alpha
    /// equations.
    blending: [i32; 6],
    /// The program, the vertex array, and the active texture unit.
    bound: [i32; 3],
    /// The texture of unit 0.
    texture: i32,
    /// Polygon mode: front, and back.
    polygons: [i32; 2],
}

impl Kept {
    /// The context's current state. (Leaves texture unit 0 active.)
    fn take(gl: &Gl) -> Kept {
        // SAFETY: the context is current (as every caller promises).
        let switches = SWITCHES.map(|(what, _)| unsafe { (gl.glIsEnabled)(what) } != 0);
        let (mut viewport, mut scissor, mut polygons) = ([0; 4], [0; 4], [FILL as i32; 2]);
        let blending = read(gl, [0x80C9, 0x80C8, 0x80CB, 0x80CA, 0x8009, 0x883D]);
        let bound = read(gl, [0x8B8D, 0x85B5, 0x84E0]);
        // SAFETY: as above; each array holds as many values as OpenGL returns for its name
        // (0x0BA2: GL_VIEWPORT, 0x0C10: GL_SCISSOR_BOX, 0x0B40: GL_POLYGON_MODE).
        unsafe {
            (gl.glGetIntegerv)(0x0BA2, viewport.as_mut_ptr());
            (gl.glGetIntegerv)(0x0C10, scissor.as_mut_ptr());
            (gl.glGetIntegerv)(0x0B40, polygons.as_mut_ptr());
            (gl.glActiveTexture)(TEXTURE0);
        }
        let [texture] = read(gl, [0x8069]);
        Kept { switches, viewport, scissor, blending, bound, texture, polygons }
    }

    /// Restores the context's state.
    fn put_back(self, gl: &Gl) {
        let [source, destination, source_alpha, destination_alpha, colour, alpha] = self.blending.map(|number| number as u32);
        let ([x, y, wide, high], [left, bottom, width, height]) = (self.viewport, self.scissor);
        let [program, corners, unit] = self.bound.map(|number| number as u32);
        // SAFETY: the context is current; each value is one OpenGL returned. (Texture unit 0 is
        // active until the last line.)
        unsafe {
            SWITCHES.iter().zip(self.switches).for_each(|((what, _), on)| [gl.glDisable, gl.glEnable][on as usize](*what));
            (gl.glViewport)(x, y, wide, high);
            (gl.glScissor)(left, bottom, width, height);
            (gl.glBlendFuncSeparate)(source, destination, source_alpha, destination_alpha);
            (gl.glBlendEquationSeparate)(colour, alpha);
            (gl.glPolygonMode)(FRONT_AND_BACK, self.polygons[0] as u32);
            (gl.glUseProgram)(program);
            (gl.glBindVertexArray)(corners);
            (gl.glBindTexture)(TEXTURE_2D, self.texture as u32);
            (gl.glActiveTexture)(unit);
        }
    }
}
