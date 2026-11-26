//! HDR through OpenGL on Windows (the feature `hdr`, see `hdr` and `hdr_windows`): windows given
//! a float pixel format, presented in scRGB as Direct3D's are.
//!
//! - A window keeps its pixel format for life, so the choice is the device's, made once: 16-bit
//!   floats (`WGL_ARB_pixel_format_float`) where a screen is HDR as the device is made, else
//!   8-bit as without the feature.
//! - Every window of a float device is drawn into its back buffer through the pass, lit or
//!   not: the SDR colour made linear, offset, times each colour's gain (capped at the peak),
//!   offset back, times SDR white (1.0 on an SDR screen, which shows it as before). The gains
//!   are a texture (`RGBA16F`) their changed part goes up to.
//! - The shaders are GLSL 3.30, compiled by the driver.
use crate::functions::functions;
use crate::hdr::Gains;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ptr::null;

functions!(Gl {
    glViewport: fn(c_int, c_int, c_int, c_int);
    glActiveTexture: fn(c_uint);
    glGenTextures: fn(c_int, *mut c_uint);
    glDeleteTextures: fn(c_int, *const c_uint);
    glBindTexture: fn(c_uint, c_uint);
    glTexParameteri: fn(c_uint, c_uint, c_int);
    glTexImage2D: fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
    glTexSubImage2D: fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void);
    glPixelStorei: fn(c_uint, c_int);
    glBindFramebuffer: fn(c_uint, c_uint);
    glGenVertexArrays: fn(c_int, *mut c_uint);
    glDeleteVertexArrays: fn(c_int, *const c_uint);
    glBindVertexArray: fn(c_uint);
    glDrawArrays: fn(c_uint, c_int, c_int);
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
    glUniform4f: fn(c_int, f32, f32, f32, f32);
});

// From glcorearb.h.
const TEXTURE_2D: c_uint = 0x0DE1;
const TEXTURE0: c_uint = 0x84C0;
const RGBA: c_uint = 0x1908;
const RGBA16F: c_int = 0x881A;
const HALF_FLOAT: c_uint = 0x140B;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
const UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const DRAW_FRAMEBUFFER: c_uint = 0x8CA9;
const TRIANGLES: c_uint = 0x0004;
const VERTEX_SHADER: c_uint = 0x8B31;
const FRAGMENT_SHADER: c_uint = 0x8B30;
const COMPILE_STATUS: c_uint = 0x8B81;
const LINK_STATUS: c_uint = 0x8B82;

/// The attributes `wglChoosePixelFormatARB` is asked for (wglext.h): drawn to a window by
/// OpenGL, double-buffered, on the GPU, four 16-bit floats.
pub(crate) const FLOAT_FORMAT: [c_int; 19] = [0x2001, 1, 0x2010, 1, 0x2011, 1, 0x2003, 0x2027, 0x2013, 0x21A0, 0x2015, 16, 0x2017, 16, 0x2019, 16, 0x201B, 16, 0];

/// The vertex shader: one triangle over the whole target.
const CORNERS: &str = "#version 330 core
void main() {
    vec2 at = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    gl_Position = vec4(at * 2.0 - 1.0, 0.0, 1.0);
}
\0";

/// The pixel shader: the window's pixel (its texture's rows from the top, the back buffer's
/// from the bottom) lit by its gains (red, green, blue, and a gain map's offset) where `light.z`
/// says the window is lit; `light.x` SDR white, `light.y` the most a gain may be.
const LIGHT: &str = "#version 330 core
uniform sampler2D picture;
uniform sampler2D gains;
uniform vec4 light;
out vec4 shown;
void main() {
    ivec2 at = ivec2(int(gl_FragCoord.x), textureSize(picture, 0).y - 1 - int(gl_FragCoord.y));
    vec3 c = texelFetch(picture, at, 0).rgb;
    vec3 sdr = mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), greaterThan(c, vec3(0.04045)));
    vec4 lit = light.z > 0.5 ? texelFetch(gains, at, 0) : vec4(1.0, 1.0, 1.0, 0.0);
    vec3 gain = min(lit.rgb, vec3(light.y));
    shown = vec4(max((sdr + lit.a) * gain - lit.a, 0.0) * light.x, 1.0);
}
\0";

/// The pass into a float back buffer: its functions, program, the places of its uniforms, an
/// empty vertex array, the gains' texture and its size.
pub(crate) struct Pass {
    gl: Gl,
    program: c_uint,
    light: c_int,
    vertices: c_uint,
    gains: c_uint,
    size: [i32; 2],
}

impl Pass {
    /// The pass in the current context, its functions found by `find`; nothing where it can't
    /// be made.
    pub(crate) fn new(find: &mut dyn FnMut(&CStr) -> *const c_void) -> Option<Pass> {
        let gl = Gl::load(find).ok()?;
        let mut pass = Pass { gl, program: 0, light: -1, vertices: 0, gains: 0, size: [0; 2] };
        pass.program = pass.program();
        if pass.program == 0 {
            return None;
        }
        let gl = &pass.gl;
        // SAFETY: OpenGL's documented calls on the current context; the names end in zeros.
        unsafe {
            (gl.glUseProgram)(pass.program);
            (gl.glUniform1i)((gl.glGetUniformLocation)(pass.program, c"picture".as_ptr()), 0);
            (gl.glUniform1i)((gl.glGetUniformLocation)(pass.program, c"gains".as_ptr()), 1);
            pass.light = (gl.glGetUniformLocation)(pass.program, c"light".as_ptr());
            (gl.glUseProgram)(0);
            (gl.glGenVertexArrays)(1, &mut pass.vertices);
            (gl.glGenTextures)(1, &mut pass.gains);
            (gl.glBindTexture)(TEXTURE_2D, pass.gains);
            (gl.glTexParameteri)(TEXTURE_2D, MIN_FILTER, NEAREST);
            (gl.glTexParameteri)(TEXTURE_2D, MAG_FILTER, NEAREST);
        }
        Some(pass)
    }

    /// The program of the two shaders; 0 where they don't compile or link.
    fn program(&self) -> c_uint {
        let (gl, mut linked) = (&self.gl, 0);
        let shaders = [(VERTEX_SHADER, CORNERS), (FRAGMENT_SHADER, LIGHT)].map(|(kind, source)| self.shader(kind, source));
        // SAFETY: OpenGL's documented calls on the current context; deleting shader 0 does
        // nothing.
        unsafe {
            if shaders.contains(&0) {
                shaders.iter().for_each(|shader| (gl.glDeleteShader)(*shader));
                return 0;
            }
            let program = (gl.glCreateProgram)();
            shaders.iter().for_each(|shader| (gl.glAttachShader)(program, *shader));
            (gl.glLinkProgram)(program);
            (gl.glGetProgramiv)(program, LINK_STATUS, &mut linked);
            shaders.iter().for_each(|shader| (gl.glDeleteShader)(*shader));
            if linked == 0 {
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
            if compiled == 0 {
                (gl.glDeleteShader)(shader);
                return 0;
            }
            shader
        }
    }

    /// Puts the gains that changed into their texture: all of them into a new one where the
    /// window has another size.
    fn put_up(&mut self, gains: &mut Gains) {
        let (gl, size) = (&self.gl, gains.size);
        if size != self.size {
            // SAFETY: the context is current; the texture is the pass's.
            unsafe {
                (gl.glBindTexture)(TEXTURE_2D, self.gains);
                (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA16F, size[0], size[1], 0, RGBA, HALF_FLOAT, null());
            }
            (self.size, gains.changed) = (size, Some([0, 0, size[0], size[1]]));
        }
        let Some([left, top, right, bottom]) = gains.changed.take() else { return };
        let first = gains.plane[top as usize * size[0] as usize + left as usize..].as_ptr();
        // SAFETY: as above; the rows read lie in the plane, as wide as the texture.
        unsafe {
            (gl.glBindTexture)(TEXTURE_2D, self.gains);
            (gl.glPixelStorei)(UNPACK_ROW_LENGTH, size[0]);
            (gl.glTexSubImage2D)(TEXTURE_2D, 0, left, top, right - left, bottom - top, RGBA, HALF_FLOAT, first.cast());
            (gl.glPixelStorei)(UNPACK_ROW_LENGTH, 0);
        }
    }

    /// Draws `picture` (a window's texture, `size` pixels) into the back buffer, lit by `gains`
    /// where `lit`, with `constants` (SDR white in scRGB, the most a gain may be).
    pub(crate) fn draw(&mut self, picture: c_uint, size: [i32; 2], gains: Option<&mut Gains>, constants: [f32; 4]) {
        let lit = gains.is_some();
        if let Some(gains) = gains {
            self.put_up(gains);
        }
        let gl = &self.gl;
        // SAFETY: the context is current; everything bound is the pass's or the window's.
        unsafe {
            (gl.glBindFramebuffer)(DRAW_FRAMEBUFFER, 0);
            (gl.glViewport)(0, 0, size[0], size[1]);
            (gl.glUseProgram)(self.program);
            (gl.glUniform4f)(self.light, constants[0], constants[1], lit as u8 as f32, 0.0);
            (gl.glActiveTexture)(TEXTURE0 + 1);
            (gl.glBindTexture)(TEXTURE_2D, self.gains);
            (gl.glActiveTexture)(TEXTURE0);
            (gl.glBindTexture)(TEXTURE_2D, picture);
            (gl.glBindVertexArray)(self.vertices);
            (gl.glDrawArrays)(TRIANGLES, 0, 3);
            (gl.glBindVertexArray)(0);
            (gl.glUseProgram)(0);
        }
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        let gl = &self.gl;
        // SAFETY: the context is current (the device's, which outlives its windows' passes);
        // each is the pass's.
        unsafe {
            (gl.glDeleteTextures)(1, &self.gains);
            (gl.glDeleteVertexArrays)(1, &self.vertices);
            (gl.glDeleteProgram)(self.program);
        }
    }
}
