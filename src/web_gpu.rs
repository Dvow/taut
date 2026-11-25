//! A page shown through WebGL2, with the feature `gpu`. The CPU draws every pixel as without it
//! (into the page's `Surface`); what changed goes into a texture (`texSubImage2D`), and each
//! frame that texture is drawn onto a canvas of WebGL2's, texel for pixel. With `gpu-draw` the
//! GPU draws large parts itself, as on the desktops: `gpu_draw_opengl`'s pipeline, its shaders
//! in GLSL ES 3.0, through the loader's WebGL2 functions (imported under OpenGL's names).
//!
//! | Asked | Taken |
//! | --- | --- |
//! | `Auto` | WebGL2 on a GPU of the computer's own (not SwiftShader, llvmpipe or WARP) |
//! | `Gpu`, a path named | WebGL2 on any device |
//! | `Cpu` | none |
//!
//! The loader's options `renderer` and `draw` say what `TAUT_RENDERER` and `TAUT_DRAW` say on
//! the desktops. Where WebGL2 is missing, fails or is lost, the 2D canvas shows the CPU's
//! pixels again, all of them drawn afresh.
use crate::geom::Rect;
use crate::platform::Renderer;
use crate::surface::Surface;
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts, took};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr::null;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn gpu_open() -> u32;
    fn gpu_close();
    fn setting(name: *const u8, length: usize) -> i32;
}

// WebGL2's functions under OpenGL ES 3's names: the loader's, and `gpu_draw_opengl` finds them
// by name (as `found` gives them).
#[link(wasm_import_module = "taut_gl")]
unsafe extern "C" {
    fn glGetError() -> c_uint;
    fn glGetIntegerv(name: c_uint, into: *mut c_int);
    fn glEnable(what: c_uint);
    fn glDisable(what: c_uint);
    fn glBlendFuncSeparate(color_from: c_uint, color_to: c_uint, alpha_from: c_uint, alpha_to: c_uint);
    fn glViewport(x: c_int, y: c_int, width: c_int, height: c_int);
    fn glScissor(x: c_int, y: c_int, width: c_int, height: c_int);
    fn glActiveTexture(unit: c_uint);
    fn glGenTextures(count: c_int, into: *mut c_uint);
    fn glBindTexture(target: c_uint, texture: c_uint);
    fn glTexParameteri(target: c_uint, name: c_uint, value: c_int);
    fn glTexImage2D(target: c_uint, level: c_int, inner: c_int, width: c_int, height: c_int, border: c_int, format: c_uint, kind: c_uint, pixels: *const c_void);
    fn glTexSubImage2D(target: c_uint, level: c_int, x: c_int, y: c_int, width: c_int, height: c_int, format: c_uint, kind: c_uint, pixels: *const c_void);
    fn glPixelStorei(name: c_uint, value: c_int);
    fn glGenFramebuffers(count: c_int, into: *mut c_uint);
    fn glBindFramebuffer(target: c_uint, frame: c_uint);
    fn glFramebufferTexture2D(target: c_uint, attachment: c_uint, texture_target: c_uint, texture: c_uint, level: c_int);
    fn glCheckFramebufferStatus(target: c_uint) -> c_uint;
    fn glGenBuffers(count: c_int, into: *mut c_uint);
    fn glBindBuffer(target: c_uint, buffer: c_uint);
    fn glBufferData(target: c_uint, size: isize, data: *const c_void, usage: c_uint);
    fn glBufferSubData(target: c_uint, offset: isize, size: isize, data: *const c_void);
    fn glGenVertexArrays(count: c_int, into: *mut c_uint);
    fn glBindVertexArray(array: c_uint);
    fn glEnableVertexAttribArray(index: c_uint);
    fn glVertexAttribPointer(index: c_uint, size: c_int, kind: c_uint, normalized: u8, stride: c_int, offset: *const c_void);
    fn glVertexAttribDivisor(index: c_uint, divisor: c_uint);
    fn glDrawArrays(mode: c_uint, first: c_int, count: c_int);
    fn glDrawArraysInstanced(mode: c_uint, first: c_int, count: c_int, instances: c_int);
    fn glCreateShader(kind: c_uint) -> c_uint;
    fn glShaderSource(shader: c_uint, count: c_int, sources: *const *const c_char, lengths: *const c_int);
    fn glCompileShader(shader: c_uint);
    fn glGetShaderiv(shader: c_uint, name: c_uint, into: *mut c_int);
    fn glDeleteShader(shader: c_uint);
    fn glCreateProgram() -> c_uint;
    fn glAttachShader(program: c_uint, shader: c_uint);
    fn glLinkProgram(program: c_uint);
    fn glGetProgramiv(program: c_uint, name: c_uint, into: *mut c_int);
    fn glDeleteProgram(program: c_uint);
    fn glUseProgram(program: c_uint);
    fn glGetUniformLocation(program: c_uint, name: *const c_char) -> c_int;
    fn glUniform1i(location: c_int, value: c_int);
}

// From OpenGL ES 3's header (gl3.h).
const TEXTURE_2D: c_uint = 0x0DE1;
const RGBA: c_uint = 0x1908;
const RGBA8: c_int = 0x8058;
const UNSIGNED_BYTE: c_uint = 0x1401;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;
const TRIANGLES: c_uint = 0x0004;
const SCISSOR_TEST: c_uint = 0x0C11;
const BLEND: c_uint = 0x0BE2;
const MAX_TEXTURE_SIZE: c_uint = 0x0D33;
const FRAMEBUFFER: c_uint = 0x8D40;
const TEXTURE0: c_uint = 0x84C0;

/// One triangle over the whole canvas.
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

/// What the loader answers `gpu_open`: no WebGL2; WebGL2 on a GPU; on the CPU (a software
/// device).
const NONE: u32 = 0;
const SOFTWARE: u32 = 2;

/// What the loader hears: the context was lost.
const LOST: u32 = 0;

thread_local! {
    /// The context was lost: the CPU shows the page from now on.
    static LOST_CONTEXT: Cell<bool> = const { Cell::new(false) };
}

/// The page's WebGL2: the program showing the texture, the texture and its size, and where the
/// GPU draws parts itself, what it records to and draws with.
pub(crate) struct Gpu {
    program: c_uint,
    height: c_int,
    texture: c_uint,
    size: [c_int; 2],
    most: c_int,
    #[cfg(feature = "gpu-draw")]
    drawing: crate::gpu_draw::Drawing,
    #[cfg(feature = "gpu-draw")]
    list: Box<crate::gpu_draw::DrawList>,
    #[cfg(feature = "gpu-draw")]
    pipeline: Option<Option<Box<crate::gpu_draw_opengl::Pipeline>>>,
}

impl Gpu {
    /// WebGL2 for a page asking for `asked` (as the loader's `renderer` says, if set). Nothing
    /// for the CPU, where WebGL2 is missing or fails, or where only a software device is had and
    /// `Auto` was asked.
    pub(crate) fn open(asked: Renderer) -> Option<Gpu> {
        let chosen = asked.chosen(setting_of("renderer").as_deref());
        if chosen == Renderer::Cpu {
            return None;
        }
        // SAFETY: a plain request.
        let device = unsafe { gpu_open() };
        let software = device == SOFTWARE;
        if device == NONE {
            return None;
        }
        if software && chosen == Renderer::Auto {
            close();
            return None;
        }
        add_part(parts::GPU, Part { after: |_| {}, heard });
        let mut gpu = Gpu {
            program: 0,
            height: -1,
            texture: 0,
            size: [0; 2],
            most: 0,
            #[cfg(feature = "gpu-draw")]
            drawing: crate::gpu_draw::Drawing::chosen(chosen == Renderer::Auto, !software, setting_of("draw").as_deref()),
            #[cfg(feature = "gpu-draw")]
            list: Box::default(),
            #[cfg(feature = "gpu-draw")]
            pipeline: None,
        };
        gpu.made().then_some(gpu)
    }

    /// Makes the program showing the texture, and the texture. Says whether it could.
    fn made(&mut self) -> bool {
        let shaders = [(VERTEX_SHADER, SPREAD), (FRAGMENT_SHADER, TEXEL)].map(|(kind, source)| shader(kind, source));
        let mut linked = 0;
        // SAFETY: WebGL2's calls through the loader; the uniform's name ends in a zero.
        unsafe {
            self.program = glCreateProgram();
            shaders.iter().for_each(|shader| glAttachShader(self.program, *shader));
            glLinkProgram(self.program);
            glGetProgramiv(self.program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| glDeleteShader(*shader));
            self.height = glGetUniformLocation(self.program, c"height".as_ptr());
            glGetIntegerv(MAX_TEXTURE_SIZE, &mut self.most);
            glGenTextures(1, &mut self.texture);
            linked != 0 && !shaders.contains(&0) && glGetError() == 0
        }
    }

    /// Brings the canvas up to date after a run: draws what changed into the texture (the CPU,
    /// or with `gpu-draw` the GPU for large parts), and shows it. Says whether it could; if not,
    /// the CPU is to show the page from now on.
    pub(crate) fn draw(&mut self, ui: &mut Ui, surface: &mut Surface) -> bool {
        let scale = ui.scale();
        let size = [(ui.size().w * scale).round() as c_int, (ui.size().h * scale).round() as c_int];
        if size.contains(&0) {
            return true;
        }
        if LOST_CONTEXT.get() || size.iter().any(|side| *side > self.most) {
            return false;
        }
        let mut places: Vec<Rect> = ui.changes().as_slice().to_vec();
        if self.fit(size, surface) {
            places = vec![Rect { x: 0.0, y: 0.0, w: ui.size().w, h: ui.size().h }];
        }
        for area in places {
            let place = pixels_of(area, scale, size);
            if place[0] >= place[2] || place[1] >= place[3] {
                continue;
            }
            #[cfg(feature = "gpu-draw")]
            if ui.theme.bg.a() == 255 && self.drawn_by_gpu(ui, area, place) {
                continue;
            }
            surface.redraw(ui, area);
            put(self.texture, surface, place);
        }
        self.show();
        // SAFETY: a plain question.
        !LOST_CONTEXT.get() && unsafe { glGetError() } == 0
    }

    /// Gives the texture and the surface the page's size: says whether they changed (all of it
    /// is drawn then).
    fn fit(&mut self, size: [c_int; 2], surface: &mut Surface) -> bool {
        let (width, height) = (size[0] as usize, size[1] as usize);
        if self.size == size && (surface.width, surface.height) == (width, height) {
            return false;
        }
        *surface = Surface { width, height, pixels: vec![0; width * height] };
        self.size = size;
        // SAFETY: WebGL2's calls through the loader; nothing to copy yet.
        unsafe {
            glBindTexture(TEXTURE_2D, self.texture);
            glTexParameteri(TEXTURE_2D, MIN_FILTER, NEAREST);
            glTexParameteri(TEXTURE_2D, MAG_FILTER, NEAREST);
            glTexImage2D(TEXTURE_2D, 0, RGBA8, size[0], size[1], 0, RGBA, UNSIGNED_BYTE, null());
        }
        true
    }

    /// Draws `area` of the page (`place` in pixels: left, top, right, bottom) on the GPU, where it
    /// draws a part that big itself (see `gpu_draw`). Says whether it did; if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    fn drawn_by_gpu(&mut self, ui: &mut Ui, area: Rect, place: [c_int; 4]) -> bool {
        if !self.drawing.takes(place) {
            return false;
        }
        let mut list = std::mem::take(&mut *self.list);
        list.begin(self.size);
        let mut canvas = crate::canvas::Canvas::recording(&mut list, self.size[0] as usize, self.size[1] as usize, (0, 0), ui.scale());
        canvas.clip(area.snap_to_pixels(ui.scale()));
        ui.paint(&mut canvas);
        drop(canvas);
        let made = self.pipeline.get_or_insert_with(|| crate::gpu_draw_opengl::Pipeline::new(&mut found, false).map(Box::new));
        let drawn = !list.failed && made.as_mut().is_some_and(|pipeline| pipeline.draw(self.texture, self.size, &list, place));
        list.finish(drawn);
        *self.list = list;
        drawn
    }

    /// Draws the texture onto the canvas, all of it.
    fn show(&self) {
        let [width, height] = self.size;
        // SAFETY: WebGL2's calls through the loader, on the canvas's own framebuffer.
        unsafe {
            glBindFramebuffer(FRAMEBUFFER, 0);
            glDisable(SCISSOR_TEST);
            glDisable(BLEND);
            glViewport(0, 0, width, height);
            glUseProgram(self.program);
            glActiveTexture(TEXTURE0);
            glBindTexture(TEXTURE_2D, self.texture);
            glUniform1i(self.height, height);
            glDrawArrays(TRIANGLES, 0, 3);
        }
    }
}

impl Drop for Gpu {
    /// The 2D canvas shows the page again.
    fn drop(&mut self) {
        close();
    }
}

/// Lets WebGL2 go: the 2D canvas shows the page.
fn close() {
    // SAFETY: a plain request.
    unsafe { gpu_close() };
}

/// The loader heard the context was lost.
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    if heard.what != LOST {
        return (Then::Wait, 0);
    }
    LOST_CONTEXT.set(true);
    (Then::Run, 0)
}

/// A place of the page in pixels (left, top, right, bottom), cut to a canvas `size` pixels big.
fn pixels_of(area: Rect, scale: f32, size: [c_int; 2]) -> [c_int; 4] {
    let area = area.snap_to_pixels(scale);
    let edge = |at: f32, most: c_int| ((at * scale).round() as c_int).clamp(0, most);
    [edge(area.x, size[0]), edge(area.y, size[1]), edge(area.right(), size[0]), edge(area.bottom(), size[1])]
}

/// Puts a place of the surface into the texture.
fn put(texture: c_uint, surface: &Surface, [left, top, right, bottom]: [c_int; 4]) {
    let from = top as usize * surface.width + left as usize;
    // SAFETY: the surface holds the rows read, `width` pixels each, from `from` on; the loader
    // copies them before it returns.
    unsafe {
        glBindTexture(TEXTURE_2D, texture);
        glPixelStorei(UNPACK_ROW_LENGTH, surface.width as c_int);
        glTexSubImage2D(TEXTURE_2D, 0, left, top, right - left, bottom - top, RGBA, UNSIGNED_BYTE, surface.pixels[from..].as_ptr().cast());
        glPixelStorei(UNPACK_ROW_LENGTH, 0);
    }
}

/// A compiled shader (0 where it didn't compile).
fn shader(kind: c_uint, source: &CStr) -> c_uint {
    let mut compiled = 0;
    // SAFETY: WebGL2's calls through the loader; the source ends in a zero.
    unsafe {
        let shader = glCreateShader(kind);
        glShaderSource(shader, 1, &source.as_ptr(), null());
        glCompileShader(shader);
        glGetShaderiv(shader, COMPILE_STATUS, &mut compiled);
        if compiled != 0 { shader } else { 0 }
    }
}

/// A setting the page gave the loader (`renderer`, `draw`), if it gave it.
fn setting_of(name: &str) -> Option<String> {
    // SAFETY: the loader reads `length` bytes, and answers with the length of the value it has.
    let length = unsafe { setting(name.as_ptr(), name.len()) };
    String::from_utf8(took(usize::try_from(length).ok()?)).ok()
}

/// A WebGL2 function by its OpenGL name, as `gpu_draw_opengl` asks for them; null for one the
/// loader has not.
#[cfg(feature = "gpu-draw")]
fn found(name: &CStr) -> *const c_void {
    macro_rules! by_name {
        ($($function:ident),*) => {
            match name.to_bytes() {
                $(name if name == stringify!($function).as_bytes() => $function as *const c_void,)*
                _ => null(),
            }
        };
    }
    by_name!(
        glGetError,
        glGetIntegerv,
        glEnable,
        glDisable,
        glBlendFuncSeparate,
        glViewport,
        glScissor,
        glActiveTexture,
        glGenTextures,
        glBindTexture,
        glTexParameteri,
        glTexImage2D,
        glTexSubImage2D,
        glPixelStorei,
        glGenFramebuffers,
        glBindFramebuffer,
        glFramebufferTexture2D,
        glCheckFramebufferStatus,
        glGenBuffers,
        glBindBuffer,
        glBufferData,
        glBufferSubData,
        glGenVertexArrays,
        glBindVertexArray,
        glEnableVertexAttribArray,
        glVertexAttribPointer,
        glVertexAttribDivisor,
        glDrawArraysInstanced,
        glCreateShader,
        glShaderSource,
        glCompileShader,
        glGetShaderiv,
        glDeleteShader,
        glCreateProgram,
        glAttachShader,
        glLinkProgram,
        glGetProgramiv,
        glDeleteProgram,
        glUseProgram,
        glGetUniformLocation,
        glUniform1i
    )
}
