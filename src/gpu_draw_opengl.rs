//! OpenGL drawing (the feature `gpu-draw`): a recorded paint (`gpu_draw::DrawList`) drawn into
//! a window's texture through a framebuffer, by OpenGL ES 3 (EGL) or OpenGL 3.3.
//!
//! - The functions are found by name through the context's loader, so nothing links to a GL
//!   library. The shaders are compiled by the driver from the source here.
//! - The quads go up into a buffer read as instanced attributes; glyphs and runs into a kept
//!   cache texture, patches and pictures into a texture of this part's.
//! - The cache and patch textures hold `0xAARRGGBB` words as they are in memory, blue first:
//!   the shaders swap red and blue as they read them. EGL's windows' textures are the same;
//!   WGL's hold red first.
//! - OpenGL 3.3 blends GDI's runs per channel (dual-source blending); OpenGL ES has no GDI runs.
//! - What it changes in the context it puts back: the program, framebuffer, vertex array,
//!   blending and the texture of unit 0. What it makes goes with the context.
use crate::functions::functions;
use crate::gpu_draw::{DrawList, Quad, SHADING, SIDE};
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr::null;

functions!(Gl {
    glGetError: fn() -> c_uint;
    glGetIntegerv: fn(c_uint, *mut c_int);
    glEnable: fn(c_uint);
    glDisable: fn(c_uint);
    glBlendFuncSeparate: fn(c_uint, c_uint, c_uint, c_uint);
    glViewport: fn(c_int, c_int, c_int, c_int);
    glScissor: fn(c_int, c_int, c_int, c_int);
    glActiveTexture: fn(c_uint);
    glGenTextures: fn(c_int, *mut c_uint);
    glBindTexture: fn(c_uint, c_uint);
    glTexParameteri: fn(c_uint, c_uint, c_int);
    glTexImage2D: fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
    glTexSubImage2D: fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
    glPixelStorei: fn(c_uint, c_int);
    glGenFramebuffers: fn(c_int, *mut c_uint);
    glBindFramebuffer: fn(c_uint, c_uint);
    glFramebufferTexture2D: fn(c_uint, c_uint, c_uint, c_uint, c_int);
    glCheckFramebufferStatus: fn(c_uint) -> c_uint;
    glGenBuffers: fn(c_int, *mut c_uint);
    glBindBuffer: fn(c_uint, c_uint);
    glBufferData: fn(c_uint, isize, *const c_void, c_uint);
    glBufferSubData: fn(c_uint, isize, isize, *const c_void);
    glGenVertexArrays: fn(c_int, *mut c_uint);
    glBindVertexArray: fn(c_uint);
    glEnableVertexAttribArray: fn(c_uint);
    glVertexAttribPointer: fn(c_uint, c_int, c_uint, u8, c_int, *const c_void);
    glVertexAttribDivisor: fn(c_uint, c_uint);
    glDrawArraysInstanced: fn(c_uint, c_int, c_int, c_int);
    glCreateShader: fn(c_uint) -> c_uint;
    glShaderSource: fn(c_uint, c_int, *const *const c_char, *const c_int);
    glCompileShader: fn(c_uint);
    glGetShaderiv: fn(c_uint, c_uint, *mut c_int);
    glDeleteShader: fn(c_uint);
    glCreateProgram: fn() -> c_uint;
    glAttachShader: fn(c_uint, c_uint);
    glLinkProgram: fn(c_uint);
    glGetProgramiv: fn(c_uint, c_uint, *mut c_int);
    glDeleteProgram: fn(c_uint);
    glUseProgram: fn(c_uint);
    glGetUniformLocation: fn(c_uint, *const c_char) -> c_int;
    glUniform1i: fn(c_int, c_int);
});

// From OpenGL's headers (gl3.h, glext.h).
const TEXTURE_2D: c_uint = 0x0DE1;
const TEXTURE0: c_uint = 0x84C0;
const RGBA: c_uint = 0x1908;
const RGBA8: c_int = 0x8058;
const UNSIGNED_BYTE: c_uint = 0x1401;
const FLOAT: c_uint = 0x1406;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const UNPACK_SKIP_PIXELS: c_uint = 0x0CF4;
const UNPACK_SKIP_ROWS: c_uint = 0x0CF3;
const FRAMEBUFFER: c_uint = 0x8D40;
const COLOR_ATTACHMENT0: c_uint = 0x8CE0;
const FRAMEBUFFER_COMPLETE: c_uint = 0x8CD5;
const FRAMEBUFFER_BINDING: c_uint = 0x8CA6;
const ARRAY_BUFFER: c_uint = 0x8892;
const STREAM_DRAW: c_uint = 0x88E0;
const VERTEX_ARRAY_BINDING: c_uint = 0x85B5;
const CURRENT_PROGRAM: c_uint = 0x8B8D;
const TEXTURE_BINDING_2D: c_uint = 0x8069;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;
const TRIANGLE_STRIP: c_uint = 0x0005;
const BLEND: c_uint = 0x0BE2;
const SCISSOR_TEST: c_uint = 0x0C11;
const SCISSOR_BOX: c_uint = 0x0C10;
const VIEWPORT: c_uint = 0x0BA2;
const ONE: c_uint = 1;
const ONE_MINUS_SRC_ALPHA: c_uint = 0x0303;
const ONE_MINUS_SRC1_COLOR: c_uint = 0x88FA;
const ONE_MINUS_SRC1_ALPHA: c_uint = 0x88FB;

/// OpenGL ES's precisions, after its version and extensions (the frames' program's).
#[cfg(all(feature = "video", unix))]
const PRECISION: &str = "precision highp float;\nprecision highp int;\n";

/// Each quad's six rows in, its corner placed from the vertex id; what the pixels need passed
/// on unchanged.
const CORNER: &str = r#"
layout(location = 0) in vec4 place;
layout(location = 1) in vec4 shape;
layout(location = 2) in vec4 more;
layout(location = 3) in vec4 source;
layout(location = 4) in vec4 color;
layout(location = 5) in vec4 kind;
flat out vec4 shape_of_quad;
flat out vec4 more_of_quad;
flat out vec4 source_of_quad;
flat out vec4 color_of_quad;
flat out float kind_of_quad;
void main() {
    vec2 p = vec2((gl_VertexID & 1) != 0 ? place.z : place.x, (gl_VertexID & 2) != 0 ? place.w : place.y);
    gl_Position = vec4(p.x / kind.z * 2.0 - 1.0, p.y / kind.w * 2.0 - 1.0, 0.0, 1.0);
    shape_of_quad = shape;
    more_of_quad = more;
    source_of_quad = source;
    color_of_quad = color;
    kind_of_quad = kind.x;
}
"#;

/// The textures, read red first.
const TEXTURES: &str = r#"
#define as_float uintBitsToFloat
uniform highp sampler2D cache;
uniform highp sampler2D patches;
vec4 texel(int which, int x, int y) {
    if (which == 0) {
        return texelFetch(cache, ivec2(x, y), 0).bgra;
    }
    return texelFetch(patches, ivec2(x, y), 0).bgra;
}
"#;

/// Each pixel: what `SHADING` gives; with dual-source blending (OpenGL 3.3, WGL's windows,
/// whose textures hold red first) also how much of each channel it covers, else blue first.
const PIXEL: &str = r#"
flat in vec4 shape_of_quad;
flat in vec4 more_of_quad;
flat in vec4 source_of_quad;
flat in vec4 color_of_quad;
flat in float kind_of_quad;
#ifdef DUAL
layout(location = 0, index = 0) out vec4 drawn_color;
layout(location = 0, index = 1) out vec4 drawn_covers;
#else
out vec4 drawn_color;
#endif
void main() {
    vec4 covers = vec4(0.0, 0.0, 0.0, 0.0);
    vec4 shaded = shade(gl_FragCoord.xy, shape_of_quad, more_of_quad, source_of_quad, color_of_quad, kind_of_quad, covers);
#ifdef DUAL
    drawn_color = shaded;
    drawn_covers = covers;
#else
    drawn_color = shaded.bgra;
#endif
}
"#;

/// What a context draws with: its functions, program, vertex array and the quads' buffer (room
/// for `room`), the cache and patch textures, the framebuffer drawing into a window's texture,
/// and whether GDI's runs blend per channel.
pub(crate) struct Pipeline {
    gl: Gl,
    program: c_uint,
    array: c_uint,
    buffer: c_uint,
    room: usize,
    textures: [c_uint; 2],
    frame: c_uint,
    dual: bool,
    /// What draws videos' frames from the decoder's buffers, where EGL imports them
    /// (`video_opengl`), and the program that reads them: made when a part first has frames
    /// (0 before; `FAILED` where it didn't compile).
    #[cfg(all(feature = "video", unix))]
    pub(crate) frames: Option<Box<crate::video_opengl::Frames>>,
    #[cfg(all(feature = "video", unix))]
    framing: c_uint,
}

/// A program that could not be made: nothing is drawn with it, and it isn't tried again.
#[cfg(all(feature = "video", unix))]
const FAILED: c_uint = c_uint::MAX;

impl std::fmt::Debug for Pipeline {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("room", &self.room).field("dual", &self.dual).finish_non_exhaustive()
    }
}

impl Pipeline {
    /// What the current context draws with, its functions found by `find`: OpenGL ES 3 (EGL's),
    /// or with `desktop` OpenGL 3.3 core (WGL's: windows' textures red first, GDI's runs blended
    /// per channel). Nothing where any part can't be made.
    pub(crate) fn new(find: &mut dyn FnMut(&CStr) -> *const c_void, desktop: bool) -> Option<Pipeline> {
        let gl = Gl::load(find).ok()?;
        let head = if desktop { "#version 330 core\n#define DUAL\n" } else { "#version 300 es\nprecision highp float;\nprecision highp int;\n" };
        let corner = format!("{head}{CORNER}\0");
        let pixel = format!("{head}{TEXTURES}{SHADING}{PIXEL}\0");
        let mut made = Pipeline {
            program: 0,
            array: 0,
            buffer: 0,
            room: 0,
            textures: [0; 2],
            frame: 0,
            dual: desktop,
            gl,
            #[cfg(all(feature = "video", unix))]
            frames: None,
            #[cfg(all(feature = "video", unix))]
            framing: 0,
        };
        made.program = made.program(&corner, &pixel);
        let gl = &made.gl;
        // SAFETY: OpenGL's documented calls on the current context; the names end in zeros.
        unsafe {
            (gl.glGenVertexArrays)(1, &mut made.array);
            (gl.glGenBuffers)(1, &mut made.buffer);
            (gl.glGenFramebuffers)(1, &mut made.frame);
            (gl.glGenTextures)(2, made.textures.as_mut_ptr());
            for texture in made.textures {
                (gl.glBindTexture)(TEXTURE_2D, texture);
                (gl.glTexParameteri)(TEXTURE_2D, MIN_FILTER, NEAREST);
                (gl.glTexParameteri)(TEXTURE_2D, MAG_FILTER, NEAREST);
                (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA8, SIDE, SIDE, 0, RGBA, UNSIGNED_BYTE, null());
            }
            (gl.glBindTexture)(TEXTURE_2D, 0);
            (gl.glBindVertexArray)(made.array);
            (gl.glBindBuffer)(ARRAY_BUFFER, made.buffer);
            for row in 0..6 {
                (gl.glEnableVertexAttribArray)(row);
                (gl.glVertexAttribPointer)(row, 4, FLOAT, 0, size_of::<Quad>() as c_int, (row as usize * 16) as *const c_void);
                (gl.glVertexAttribDivisor)(row, 1);
            }
            (gl.glBindVertexArray)(0);
            if made.program != 0 {
                let mut before = 0;
                (gl.glGetIntegerv)(CURRENT_PROGRAM, &mut before);
                (gl.glUseProgram)(made.program);
                (gl.glUniform1i)((gl.glGetUniformLocation)(made.program, c"cache".as_ptr()), 0);
                (gl.glUniform1i)((gl.glGetUniformLocation)(made.program, c"patches".as_ptr()), 1);
                (gl.glUseProgram)(before as c_uint);
            }
            let fine = (gl.glGetError)() == 0;
            (fine && made.program != 0).then_some(made)
        }
    }

    /// The program of the two shaders' sources (each ending in a zero); 0 where they don't
    /// compile or link.
    fn program(&self, corner: &str, pixel: &str) -> c_uint {
        let (gl, mut linked) = (&self.gl, 0);
        let shaders = [(VERTEX_SHADER, corner), (FRAGMENT_SHADER, pixel)].map(|(kind, source)| self.shader(kind, source));
        // SAFETY: OpenGL's documented calls on the current context.
        unsafe {
            let program = (gl.glCreateProgram)();
            shaders.iter().for_each(|shader| (gl.glAttachShader)(program, *shader));
            (gl.glLinkProgram)(program);
            (gl.glGetProgramiv)(program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| (gl.glDeleteShader)(*shader));
            if linked == 0 || shaders.contains(&0) {
                (gl.glDeleteProgram)(program);
                return 0;
            }
            program
        }
    }

    /// A compiled shader of `source` (ending in a zero); 0 where it didn't compile.
    fn shader(&self, kind: c_uint, source: &str) -> c_uint {
        let (gl, mut compiled) = (&self.gl, 0);
        // SAFETY: as above; the source ends in a zero.
        unsafe {
            let shader = (gl.glCreateShader)(kind);
            (gl.glShaderSource)(shader, 1, &source.as_ptr().cast(), null());
            (gl.glCompileShader)(shader);
            (gl.glGetShaderiv)(shader, COMPILE_STATUS, &mut compiled);
            if compiled != 0 { shader } else { 0 }
        }
    }

    /// Draws `list` into `texture` (a window's, `size` pixels, rows from the top), inside `area`
    /// (left, top, right, bottom): its texels put in first. The context is current; what this
    /// changes in it is put back. Says whether OpenGL took it all.
    pub(crate) fn draw(&mut self, texture: c_uint, size: [c_int; 2], list: &DrawList, area: [c_int; 4]) -> bool {
        let kept = self.kept();
        self.put_up(list);
        let gl = &self.gl;
        // SAFETY: OpenGL's documented calls on the current context, with its own framebuffer.
        let complete = unsafe {
            (gl.glBindFramebuffer)(FRAMEBUFFER, self.frame);
            (gl.glFramebufferTexture2D)(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture, 0);
            (gl.glCheckFramebufferStatus)(FRAMEBUFFER) == FRAMEBUFFER_COMPLETE
        };
        #[cfg(all(feature = "video", unix))]
        let complete = complete && self.ready_frames(list);
        if complete && !list.quads.is_empty() {
            self.quads(list, size, area);
        }
        self.put_back(kept);
        // SAFETY: as above.
        complete && unsafe { (self.gl.glGetError)() } == 0
    }

    /// Readies the list's videos' frames for the shaders (see `video_opengl`), their program
    /// made the first time: whether they are, or it has none.
    #[cfg(all(feature = "video", unix))]
    fn ready_frames(&mut self, list: &DrawList) -> bool {
        if list.frames.is_empty() {
            return true;
        }
        if self.framing == 0 {
            self.framing = self.framing_program();
        }
        let (Some(frames), true) = (self.frames.as_mut(), self.framing != FAILED) else { return false };
        // SAFETY: OpenGL's documented call on the current context (`put_back` puts back the
        // program that was).
        unsafe { (self.gl.glUseProgram)(self.framing) };
        frames.ready(&list.frames)
    }

    /// The program that draws videos' frames too, its samplers told their units; `FAILED`
    /// where it can't be made.
    #[cfg(all(feature = "video", unix))]
    fn framing_program(&mut self) -> c_uint {
        use crate::video_opengl::{EXTENSION, textures};
        let corner = format!("#version 300 es\n{PRECISION}{CORNER}\0");
        let pixel = format!("#version 300 es\n{EXTENSION}{PRECISION}{}{SHADING}{PIXEL}\0", textures());
        let (made, gl) = (self.program(&corner, &pixel), &self.gl);
        let Some(frames) = self.frames.as_mut().filter(|_| made != 0) else { return FAILED };
        // SAFETY: OpenGL's documented calls on the current context; the names end in zeros.
        unsafe {
            (gl.glUseProgram)(made);
            (gl.glUniform1i)((gl.glGetUniformLocation)(made, c"cache".as_ptr()), 0);
            (gl.glUniform1i)((gl.glGetUniformLocation)(made, c"patches".as_ptr()), 1);
        }
        frames.learn(made);
        made
    }

    /// The program a part is drawn with: the one reading frames where it has any.
    fn program_for(&self, list: &DrawList) -> c_uint {
        #[cfg(all(feature = "video", unix))]
        if !list.frames.is_empty() {
            return self.framing;
        }
        #[cfg(not(all(feature = "video", unix)))]
        let _ = list;
        self.program
    }

    /// What drawing changes in the context, as it is: the program, framebuffer, vertex array,
    /// unit 0's texture, the scissor test, the scissor box, the viewport.
    fn kept(&self) -> [c_int; 13] {
        let (gl, mut kept) = (&self.gl, [0; 13]);
        // SAFETY: OpenGL's documented queries on the current context, each into room for it.
        unsafe {
            (gl.glActiveTexture)(TEXTURE0);
            let names = [CURRENT_PROGRAM, FRAMEBUFFER_BINDING, VERTEX_ARRAY_BINDING, TEXTURE_BINDING_2D, SCISSOR_TEST];
            names.iter().zip(&mut kept).for_each(|(name, room)| (gl.glGetIntegerv)(*name, room));
            (gl.glGetIntegerv)(SCISSOR_BOX, kept[5..9].as_mut_ptr());
            (gl.glGetIntegerv)(VIEWPORT, kept[9..13].as_mut_ptr());
        }
        kept
    }

    /// Puts back what `kept` took.
    fn put_back(&self, kept: [c_int; 13]) {
        let gl = &self.gl;
        // SAFETY: OpenGL's documented calls on the current context, with what it said before.
        unsafe {
            (gl.glFramebufferTexture2D)(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, 0, 0);
            (gl.glBindFramebuffer)(FRAMEBUFFER, kept[1] as c_uint);
            (gl.glBindVertexArray)(kept[2] as c_uint);
            (gl.glUseProgram)(kept[0] as c_uint);
            (gl.glBindTexture)(TEXTURE_2D, kept[3] as c_uint);
            (gl.glScissor)(kept[5], kept[6], kept[7], kept[8]);
            (gl.glViewport)(kept[9], kept[10], kept[11], kept[12]);
            if kept[4] == 0 {
                (gl.glDisable)(SCISSOR_TEST);
            }
        }
    }

    /// Puts each of `list`'s uploads into its texture.
    fn put_up(&self, list: &DrawList) {
        let gl = &self.gl;
        // SAFETY: OpenGL's documented calls on the current context. The staging holds every
        // upload's rows, each as wide as said, from its start; read straight from memory.
        unsafe {
            [UNPACK_ROW_LENGTH, UNPACK_SKIP_PIXELS, UNPACK_SKIP_ROWS].into_iter().for_each(|name| (gl.glPixelStorei)(name, 0));
            for upload in &list.uploads {
                let [x, y, width, height] = upload.at;
                (gl.glBindTexture)(TEXTURE_2D, self.textures[upload.texture as usize]);
                (gl.glTexSubImage2D)(TEXTURE_2D, 0, x, y, width, height, RGBA, UNSIGNED_BYTE, list.staging[upload.from..].as_ptr().cast());
            }
        }
    }

    /// Puts `list`'s quads up and draws them into the framebuffer, `size` pixels, inside
    /// `area`, blended over what is there.
    fn quads(&mut self, list: &DrawList, size: [c_int; 2], [left, top, right, bottom]: [c_int; 4]) {
        let (gl, bytes) = (&self.gl, (list.quads.len() * size_of::<Quad>()) as isize);
        let blend = if self.dual { [ONE, ONE_MINUS_SRC1_COLOR, ONE, ONE_MINUS_SRC1_ALPHA] } else { [ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA] };
        let grown = (list.quads.len() > self.room).then(|| list.quads.len().next_power_of_two().max(1024));
        // SAFETY: OpenGL's documented calls on the current context; the buffer is made as large
        // as the quads before they go up.
        unsafe {
            (gl.glBindVertexArray)(self.array);
            (gl.glBindBuffer)(ARRAY_BUFFER, self.buffer);
            if let Some(room) = grown {
                (gl.glBufferData)(ARRAY_BUFFER, (room * size_of::<Quad>()) as isize, null(), STREAM_DRAW);
            }
            (gl.glBufferSubData)(ARRAY_BUFFER, 0, bytes, list.quads.as_ptr().cast());
            (gl.glUseProgram)(self.program_for(list));
            (gl.glActiveTexture)(TEXTURE0 + 1);
            (gl.glBindTexture)(TEXTURE_2D, self.textures[1]);
            (gl.glActiveTexture)(TEXTURE0);
            (gl.glBindTexture)(TEXTURE_2D, self.textures[0]);
            (gl.glEnable)(BLEND);
            (gl.glBlendFuncSeparate)(blend[0], blend[1], blend[2], blend[3]);
            (gl.glEnable)(SCISSOR_TEST);
            (gl.glViewport)(0, 0, size[0], size[1]);
            (gl.glScissor)(left, top, right - left, bottom - top);
            (gl.glDrawArraysInstanced)(TRIANGLE_STRIP, 0, 4, list.quads.len() as c_int);
            (gl.glDisable)(BLEND);
        }
        self.room = grown.unwrap_or(self.room);
    }
}
