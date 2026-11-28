//! Videos' frames drawn by OpenGL ES (EGL) from the decoder's own buffer (`video` with
//! `gpu-draw`): no frame is read back while it plays.
//!
//! - Android: the image reader's hardware buffer as an EGL image
//!   (`EGL_ANDROID_get_native_client_buffer`, `EGL_ANDROID_image_native_buffer`), read as an
//!   external texture (`GL_OES_EGL_image_external_essl3`): the driver makes it RGB, by the
//!   matrix it takes the buffer to be in.
//! - Linux: each plane of a linear dmabuf as an EGL image of its own
//!   (`EGL_EXT_image_dma_buf_import`, as `R8` and `GR88`), a texture each; the shaders make them
//!   RGB as the CPU does (`video_yuv::FROM_YUV`), so both give the same pixels.
//! - Imports are kept as `gpu_frames` says.
use crate::functions::functions;
use crate::gpu_draw::MOST_FRAMES;
use crate::gpu_frames::ImportedFrames;
use crate::video::FrameTexture;
use std::ffi::{CStr, c_int, c_uint, c_void};
use std::ptr::null_mut;
use std::sync::Arc;

type Handle = *mut c_void;

functions!(Calls {
    eglCreateImageKHR: fn(Handle, Handle, c_uint, Handle, *const c_int) -> Handle;
    eglDestroyImageKHR: fn(Handle, Handle) -> c_uint;
    glEGLImageTargetTexture2DOES: fn(c_uint, Handle);
    glGenTextures: fn(c_int, *mut c_uint);
    glDeleteTextures: fn(c_int, *const c_uint);
    glBindTexture: fn(c_uint, c_uint);
    glActiveTexture: fn(c_uint);
    glTexParameteri: fn(c_uint, c_uint, c_int);
    glGetUniformLocation: fn(c_uint, *const std::ffi::c_char) -> c_int;
    glUniform1i: fn(c_int, c_int);
    glUniform4fv: fn(c_int, c_int, *const f32);
    #[cfg(target_os = "android")]
    eglGetNativeClientBufferANDROID: fn(*const c_void) -> Handle;
    #[cfg(not(target_os = "android"))]
    glUniform4iv: fn(c_int, c_int, *const c_int);
});

// From EGL's and OpenGL's headers (eglext.h, gl2ext.h).
const NONE: c_int = 0x3038;
const TEXTURE0: c_uint = 0x84C0;
const MIN_FILTER: c_uint = 0x2801;
const MAG_FILTER: c_uint = 0x2800;
const NEAREST: c_int = 0x2600;
/// Texture units: 0 and 1 are the cache's and the patches'; frames' from 2.
const FIRST_UNIT: c_uint = 2;

/// The textures, read red first, before `SHADING` (after `#version`, then `EXTENSION`, then
/// the precisions): the cache and patches, four frames read through the driver (Android) or
/// their planes (Linux), and the numbers that make each RGB (eight a frame).
#[cfg(target_os = "android")]
pub(crate) const EXTENSION: &str = "#extension GL_OES_EGL_image_external_essl3 : require\n";
#[cfg(target_os = "android")]
const TEXTURES: &str = r#"
#define FRAMES
#define as_float uintBitsToFloat
uniform highp sampler2D cache;
uniform highp sampler2D patches;
uniform highp samplerExternalOES frame0;
uniform highp samplerExternalOES frame1;
uniform highp samplerExternalOES frame2;
uniform highp samplerExternalOES frame3;
uniform highp vec4 numbers[8];
vec4 frame_texel(highp samplerExternalOES frame, vec4 b, int x, int y) {
    return vec4(texture(frame, (vec2(x, y) + 0.5) / b.zw).rgb, 1.0);
}
vec4 texel(int which, int x, int y) {
    if (which == 0) {
        return texelFetch(cache, ivec2(x, y), 0).bgra;
    } else if (which == 1) {
        return texelFetch(patches, ivec2(x, y), 0).bgra;
    } else if (which == 2) {
        return frame_texel(frame0, numbers[1], x, y);
    } else if (which == 3) {
        return frame_texel(frame1, numbers[3], x, y);
    } else if (which == 4) {
        return frame_texel(frame2, numbers[5], x, y);
    }
    return frame_texel(frame3, numbers[7], x, y);
}
"#;
#[cfg(not(target_os = "android"))]
pub(crate) const EXTENSION: &str = "";
#[cfg(not(target_os = "android"))]
const UNIFORMS: &str = r#"
#define FRAMES
#define as_float uintBitsToFloat
uniform highp sampler2D cache;
uniform highp sampler2D patches;
uniform highp sampler2D luma0;
uniform highp sampler2D luma1;
uniform highp sampler2D luma2;
uniform highp sampler2D luma3;
uniform highp sampler2D chroma0;
uniform highp sampler2D chroma1;
uniform highp sampler2D chroma2;
uniform highp sampler2D chroma3;
uniform highp sampler2D extra0;
uniform highp sampler2D extra1;
uniform highp sampler2D extra2;
uniform highp sampler2D extra3;
uniform highp vec4 numbers[8];
uniform highp ivec4 planes;
"#;
#[cfg(not(target_os = "android"))]
const TEXEL: &str = r#"
vec4 frame_texel(highp sampler2D luma, highp sampler2D chroma, highp sampler2D extra, int count, vec4 a, vec4 b, int x, int y) {
    ivec2 half_at = ivec2(x, y) / 2;
    vec4 uv = texelFetch(chroma, half_at, 0);
    float v = count == 2 ? uv.g : texelFetch(extra, half_at, 0).r;
    return from_yuv(floor(vec3(texelFetch(luma, ivec2(x, y), 0).r, uv.r, v) * 255.0 + 0.5), a, b);
}
vec4 texel(int which, int x, int y) {
    if (which == 0) {
        return texelFetch(cache, ivec2(x, y), 0).bgra;
    } else if (which == 1) {
        return texelFetch(patches, ivec2(x, y), 0).bgra;
    } else if (which == 2) {
        return frame_texel(luma0, chroma0, extra0, planes.x, numbers[0], numbers[1], x, y);
    } else if (which == 3) {
        return frame_texel(luma1, chroma1, extra1, planes.y, numbers[2], numbers[3], x, y);
    } else if (which == 4) {
        return frame_texel(luma2, chroma2, extra2, planes.z, numbers[4], numbers[5], x, y);
    }
    return frame_texel(luma3, chroma3, extra3, planes.w, numbers[6], numbers[7], x, y);
}
"#;

/// The fragment shader's part before `SHADING`: `TEXTURES` (Linux: `UNIFORMS`, `FROM_YUV`,
/// `TEXEL`).
pub(crate) fn textures() -> String {
    #[cfg(target_os = "android")]
    return TEXTURES.to_owned();
    #[cfg(not(target_os = "android"))]
    format!("{UNIFORMS}{}{TEXEL}", crate::video_yuv::FROM_YUV)
}

/// The samplers of the frames, by texture unit from `FIRST_UNIT`, each name ending in a zero.
#[cfg(target_os = "android")]
const SAMPLERS: [&CStr; MOST_FRAMES] = [c"frame0", c"frame1", c"frame2", c"frame3"];
#[cfg(not(target_os = "android"))]
const SAMPLERS: [&CStr; MOST_FRAMES * 3] = [c"luma0", c"luma1", c"luma2", c"luma3", c"chroma0", c"chroma1", c"chroma2", c"chroma3", c"extra0", c"extra1", c"extra2", c"extra3"];

/// The textures a frame is read from (Android: one external texture; Linux: one for each plane).
#[cfg(target_os = "android")]
const PER_FRAME: usize = 1;
#[cfg(not(target_os = "android"))]
const PER_FRAME: usize = 3;

/// A decoder's buffer imported: its EGL images and their textures (0 where none); Android: the
/// hardware buffer, held while imported.
struct Imported {
    images: [Handle; PER_FRAME],
    textures: [c_uint; PER_FRAME],
    #[cfg(target_os = "android")]
    _held: crate::video_android::HeldBuffer,
}

/// What draws videos' frames in an EGL context: its functions, EGL's display, the buffers
/// imported, the program's uniforms (`numbers`, Linux: `planes`), and the numbers that make this
/// part's frames RGB.
pub(crate) struct Frames {
    calls: Calls,
    display: Handle,
    imported: ImportedFrames<Imported>,
    uniforms: [c_int; 2],
    numbers: [f32; MOST_FRAMES * 8],
}

/// Whether an EGL display offering `extensions` (as `eglQueryString` says) imports the
/// decoder's buffers.
pub(crate) fn imports(extensions: &[u8]) -> bool {
    let offers = |name: &str| extensions.split(|c| *c == b' ').any(|offered| offered == name.as_bytes());
    #[cfg(target_os = "android")]
    let buffers = offers("EGL_ANDROID_get_native_client_buffer") && offers("EGL_ANDROID_image_native_buffer");
    #[cfg(not(target_os = "android"))]
    let buffers = offers("EGL_EXT_image_dma_buf_import");
    buffers && offers("EGL_KHR_image_base")
}

impl Frames {
    /// What draws frames in the context of EGL's `display`, which offers `extensions` (as
    /// `eglQueryString` says), its functions found by `find`; none where it can't.
    pub(crate) fn new(find: &mut dyn FnMut(&CStr) -> *const c_void, display: Handle, extensions: &[u8]) -> Option<Box<Frames>> {
        if !imports(extensions) {
            return None;
        }
        let calls = Calls::load(find).ok()?;
        Some(Box::new(Frames { calls, display, imported: ImportedFrames::default(), uniforms: [-1; 2], numbers: [0.0; MOST_FRAMES * 8] }))
    }

    /// Tells `program` (current, made with `TEXTURES`) which unit each frame's sampler reads,
    /// and notes where its numbers go.
    pub(crate) fn learn(&mut self, program: c_uint) {
        let calls = &self.calls;
        // SAFETY: OpenGL's documented calls on the current context, its program current; the
        // names end in zeros.
        unsafe {
            for (unit, name) in (FIRST_UNIT..).zip(SAMPLERS) {
                (calls.glUniform1i)((calls.glGetUniformLocation)(program, name.as_ptr()), unit as c_int);
            }
            self.uniforms = [(calls.glGetUniformLocation)(program, c"numbers".as_ptr()), (calls.glGetUniformLocation)(program, c"planes".as_ptr())];
        }
    }

    /// Readies `frames` (this part's) to be drawn: each imported (as `gpu_frames` says) and
    /// bound to its units, their numbers set (the program current where there are any). Whether
    /// all are; where one can't be, the context refuses frames.
    pub(crate) fn ready(&mut self, frames: &[Arc<FrameTexture>]) -> bool {
        let mut imported = std::mem::take(&mut self.imported);
        let ready = imported.ready(frames, |frame| self.imported_of(frame), |gone| self.let_go_of(gone));
        if ready && !frames.is_empty() {
            for (nth, (import, frame)) in imported.drawn().zip(frames).enumerate() {
                self.bind(nth, import);
                self.numbers[nth * 8..nth * 8 + 8].copy_from_slice(&frame.for_shaders());
            }
            self.set_numbers(frames);
        }
        self.imported = imported;
        ready
    }

    /// Notes that frames can't be drawn in this context. Gives false.
    pub(crate) fn refuse(&mut self) -> bool {
        self.imported.refuse()
    }

    /// Whether frames can't be drawn in this context: its parts then draw them read back.
    pub(crate) fn refused(&self) -> bool {
        self.imported.refused()
    }

    /// A texture of EGL image `image`, as `target` (unfiltered); 0 where it can't be made.
    fn texture(&self, target: c_uint, image: Handle) -> c_uint {
        let (calls, mut texture) = (&self.calls, 0);
        if image.is_null() {
            return 0;
        }
        // SAFETY: OpenGL's documented calls on the current context; the image is alive.
        unsafe {
            (calls.glGenTextures)(1, &mut texture);
            (calls.glBindTexture)(target, texture);
            (calls.glTexParameteri)(target, MIN_FILTER, NEAREST);
            (calls.glTexParameteri)(target, MAG_FILTER, NEAREST);
            (calls.glEGLImageTargetTexture2DOES)(target, image);
            (calls.glBindTexture)(target, 0);
        }
        texture
    }

    /// Lets go of an import: its textures, then its images.
    fn let_go_of(&self, imported: Imported) {
        // SAFETY: the textures are the current context's (0 is let be), the images the
        // display's.
        unsafe { (self.calls.glDeleteTextures)(PER_FRAME as c_int, imported.textures.as_ptr()) };
        self.let_go_of_images(&imported);
    }

    /// Lets go of an import's EGL images.
    fn let_go_of_images(&self, imported: &Imported) {
        // SAFETY: each image is the display's, used by nothing after.
        imported.images.iter().filter(|image| !image.is_null()).for_each(|image| unsafe { _ = (self.calls.eglDestroyImageKHR)(self.display, *image) });
    }
}

impl Drop for Frames {
    /// Lets go of the EGL images (the textures go with the context, which may not be current).
    fn drop(&mut self) {
        let mut imported = std::mem::take(&mut self.imported);
        imported.let_go_of_all(|import| self.let_go_of_images(&import));
    }
}

// ---- Android: the image reader's hardware buffers ----

/// An EGL image of a hardware buffer (`EGL_NATIVE_BUFFER_ANDROID`), its content kept
/// (`EGL_IMAGE_PRESERVED_KHR`); an external texture (`GL_TEXTURE_EXTERNAL_OES`).
#[cfg(target_os = "android")]
const NATIVE_BUFFER: c_uint = 0x3140;
#[cfg(target_os = "android")]
const PRESERVED: c_int = 0x30D2;
#[cfg(target_os = "android")]
const EXTERNAL: c_uint = 0x8D65;

#[cfg(target_os = "android")]
impl Frames {
    /// `frame`'s hardware buffer as an EGL image and an external texture of it; the buffer held
    /// while imported.
    fn imported_of(&self, frame: &FrameTexture) -> Option<Imported> {
        let (held, asked) = (frame.hold_buffer(), [PRESERVED, 1, NONE]);
        // SAFETY: a live buffer; EGL's documented calls on the display, the list ending in
        // `NONE`.
        let image = unsafe {
            let client = (self.calls.eglGetNativeClientBufferANDROID)(frame.buffer);
            (self.calls.eglCreateImageKHR)(self.display, null_mut(), NATIVE_BUFFER, client, asked.as_ptr())
        };
        let imported = Imported { images: [image], textures: [self.texture(EXTERNAL, image)], _held: held };
        if imported.textures[0] == 0 {
            self.let_go_of(imported);
            return None;
        }
        Some(imported)
    }

    /// Binds `imported` to frame `nth`'s unit, its image named again (so the texture reads
    /// what the decoder wrote since).
    fn bind(&self, nth: usize, imported: &Imported) {
        // SAFETY: OpenGL's documented calls on the current context; the texture and image are
        // alive.
        unsafe {
            (self.calls.glActiveTexture)(TEXTURE0 + FIRST_UNIT + nth as c_uint);
            (self.calls.glBindTexture)(EXTERNAL, imported.textures[0]);
            (self.calls.glEGLImageTargetTexture2DOES)(EXTERNAL, imported.images[0]);
            (self.calls.glActiveTexture)(TEXTURE0);
        }
    }

    /// Sets the program's numbers.
    fn set_numbers(&self, _: &[Arc<FrameTexture>]) {
        // SAFETY: OpenGL's documented call on the current program; eight vectors of four.
        unsafe { (self.calls.glUniform4fv)(self.uniforms[0], (MOST_FRAMES * 2) as c_int, self.numbers.as_ptr()) };
    }
}

// ---- Linux: dmabufs ----

/// An EGL image of a dmabuf (`EGL_LINUX_DMA_BUF_EXT`) and its attributes: its size, DRM format,
/// file, and where its plane starts and its bytes a row; DRM's formats of one and two bytes a
/// sample (`R8`, `GR88`: red the first byte); a plain texture (`GL_TEXTURE_2D`).
#[cfg(not(target_os = "android"))]
const DMA_BUF: c_uint = 0x3270;
#[cfg(not(target_os = "android"))]
const ATTRIBUTES: [c_int; 6] = [0x3057, 0x3056, 0x3271, 0x3272, 0x3273, 0x3274];
#[cfg(not(target_os = "android"))]
const R8: c_int = 0x2020_3852;
#[cfg(not(target_os = "android"))]
const GR88: c_int = 0x3838_5247;
#[cfg(not(target_os = "android"))]
const TEXTURE_2D: c_uint = 0x0DE1;

#[cfg(not(target_os = "android"))]
impl Frames {
    /// `frame`'s dmabuf as an EGL image and a texture for each plane.
    fn imported_of(&self, frame: &FrameTexture) -> Option<Imported> {
        let mut imported = Imported { images: [null_mut(); 3], textures: [0; 3] };
        let planes = &frame.dmabuf.planes;
        for (nth, plane) in planes.iter().enumerate() {
            let (size, format) = match nth {
                0 => (frame.whole, R8),
                _ => ((frame.whole.0.div_ceil(2), frame.whole.1.div_ceil(2)), if planes.len() == 2 { GR88 } else { R8 }),
            };
            let [width, height, fourcc, fd, offset, pitch] = ATTRIBUTES;
            let asked = [width, size.0 as c_int, height, size.1 as c_int, fourcc, format, fd, frame.dmabuf.fd, offset, plane.offset as c_int, pitch, plane.pitch as c_int, NONE];
            // SAFETY: EGL's documented call on the display, the list ending in `NONE`; the file
            // is the frame's (EGL takes a reference of its own).
            imported.images[nth] = unsafe { (self.calls.eglCreateImageKHR)(self.display, null_mut(), DMA_BUF, null_mut(), asked.as_ptr()) };
            imported.textures[nth] = self.texture(TEXTURE_2D, imported.images[nth]);
        }
        if imported.textures[..planes.len()].contains(&0) {
            self.let_go_of(imported);
            return None;
        }
        Some(imported)
    }

    /// Binds `imported`'s planes to frame `nth`'s units (a plane it lacks reads its first).
    fn bind(&self, nth: usize, imported: &Imported) {
        for plane in 0..PER_FRAME {
            let texture = if imported.textures[plane] != 0 { imported.textures[plane] } else { imported.textures[0] };
            // SAFETY: OpenGL's documented calls on the current context; the texture is alive.
            unsafe {
                (self.calls.glActiveTexture)(TEXTURE0 + FIRST_UNIT + (plane * MOST_FRAMES + nth) as c_uint);
                (self.calls.glBindTexture)(TEXTURE_2D, texture);
            }
        }
        // SAFETY: as above.
        unsafe { (self.calls.glActiveTexture)(TEXTURE0) };
    }

    /// Sets the program's numbers, and each frame's count of planes.
    fn set_numbers(&self, frames: &[Arc<FrameTexture>]) {
        let planes: [c_int; MOST_FRAMES] = std::array::from_fn(|nth| frames.get(nth).map_or(2, |frame| frame.dmabuf.planes.len() as c_int));
        // SAFETY: OpenGL's documented calls on the current program; eight vectors of four, and
        // one.
        unsafe {
            (self.calls.glUniform4fv)(self.uniforms[0], (MOST_FRAMES * 2) as c_int, self.numbers.as_ptr());
            (self.calls.glUniform4iv)(self.uniforms[1], 1, planes.as_ptr());
        }
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use crate::gpu_draw::DrawList;
    use crate::gpu_draw::tests::{FRAME_PLACES, frame_part};
    use crate::gpu_draw_opengl::Pipeline;
    use crate::video_gstreamer::tests::{dmabuf_frame, pitchless_dmabuf_frame};

    functions!(Testing {
        eglGetPlatformDisplayEXT: fn(c_uint, Handle, *const c_int) -> Handle;
        glGenFramebuffers: fn(c_int, *mut c_uint);
        glBindFramebuffer: fn(c_uint, c_uint);
        glFramebufferTexture2D: fn(c_uint, c_uint, c_uint, c_uint, c_int);
        glReadPixels: fn(c_int, c_int, c_int, c_int, c_uint, c_uint, *mut c_void);
    });

    /// Mesa's surfaceless EGL display with an OpenGL ES 3 context current (no surface), its
    /// extensions, and the functions the checks read with; none where there is none.
    fn surfaceless() -> Option<(Handle, Vec<u8>, Testing)> {
        let (egl, _) = crate::gpu_egl::libraries()?;
        let testing = Testing::load(&mut |name| unsafe { (egl.eglGetProcAddress)(name.as_ptr()) }.cast_const()).ok()?;
        let (mut config, mut configs) = (null_mut(), 0);
        unsafe {
            let display = (testing.eglGetPlatformDisplayEXT)(0x31DD, null_mut(), [NONE].as_ptr());
            if display.is_null() || (egl.eglInitialize)(display, null_mut(), null_mut()) == 0 {
                return None;
            }
            (egl.eglBindAPI)(0x30A0);
            (egl.eglChooseConfig)(display, [0x3040, 0x40, 0x3033, 0, NONE].as_ptr(), &mut config, 1, &mut configs);
            let context = (egl.eglCreateContext)(display, config, null_mut(), [0x3098, 3, NONE].as_ptr());
            if context.is_null() || (egl.eglMakeCurrent)(display, null_mut(), null_mut(), context) == 0 {
                return None;
            }
            let extensions = CStr::from_ptr((egl.eglQueryString)(display, 0x3055)).to_bytes().to_vec();
            Some((display, extensions, testing))
        }
    }

    /// `list` drawn by `pipeline` into a new texture `size` pixels, first filled with `under`,
    /// and read back.
    fn drawn(pipeline: &mut Pipeline, testing: &Testing, list: &DrawList, size: [c_int; 2], under: &[u32]) -> Vec<u32> {
        let (_, gles) = crate::gpu_egl::libraries().expect("EGL");
        let (mut texture, mut framebuffer, mut pixels) = (0, 0, vec![0u32; (size[0] * size[1]) as usize]);
        unsafe {
            (gles.glGenTextures)(1, &mut texture);
            (gles.glBindTexture)(TEXTURE_2D, texture);
            (gles.glTexStorage2D)(TEXTURE_2D, 1, 0x8058, size[0], size[1]);
            (gles.glTexSubImage2D)(TEXTURE_2D, 0, 0, 0, size[0], size[1], 0x1908, 0x1401, under.as_ptr().cast());
        }
        assert!(pipeline.draw(texture, size, list, [0, 0, size[0], size[1]]), "OpenGL drew it");
        unsafe {
            (testing.glGenFramebuffers)(1, &mut framebuffer);
            (testing.glBindFramebuffer)(0x8D40, framebuffer);
            (testing.glFramebufferTexture2D)(0x8D40, 0x8CE0, TEXTURE_2D, texture, 0);
            (testing.glReadPixels)(0, 0, size[0], size[1], 0x1908, 0x1401, pixels.as_mut_ptr().cast());
            (gles.glDeleteTextures)(1, &texture);
        }
        pixels
    }

    #[test]
    fn a_dmabufs_frame_is_drawn_from_there_as_its_pixels_read_back_are() {
        let Some(frame) = dmabuf_frame() else { return println!("no GStreamer dmabuf allocator here: not tested") };
        let Some((display, extensions, testing)) = surfaceless() else { return println!("no surfaceless EGL here: not tested") };
        assert!(imports(&extensions), "this EGL imports dmabufs");
        let (egl, _) = crate::gpu_egl::libraries().expect("EGL");
        let mut find = |name: &CStr| unsafe { (egl.eglGetProcAddress)(name.as_ptr()) }.cast_const();
        let mut pipeline = Pipeline::new(&mut find, false).expect("the pipeline is made");
        pipeline.frames = Frames::new(&mut find, display, &extensions);
        assert!(pipeline.frames.is_some(), "frames are drawn in this context");
        let size = [220, 170];
        let under = vec![0xff20_2830; (size[0] * size[1]) as usize];
        for r in FRAME_PLACES {
            let from_dmabuf = drawn(&mut pipeline, &testing, &frame_part(&frame, size, r, true), size, &under);
            let read_back = drawn(&mut pipeline, &testing, &frame_part(&frame, size, r, false), size, &under);
            let differ = from_dmabuf.iter().zip(&read_back).filter(|(a, b)| a != b).count();
            assert_eq!(differ, 0, "at {r:?}: {differ} pixels differ from the frame read back and drawn as a picture");
        }
    }

    #[test]
    fn a_frame_that_cant_be_imported_refuses_the_context() {
        let Some(pitchless) = pitchless_dmabuf_frame() else { return println!("no GStreamer dmabuf allocator here: not tested") };
        let Some((display, extensions, _)) = surfaceless() else { return println!("no surfaceless EGL here: not tested") };
        let (egl, _) = crate::gpu_egl::libraries().expect("EGL");
        let mut find = |name: &CStr| unsafe { (egl.eglGetProcAddress)(name.as_ptr()) }.cast_const();
        let mut frames = Frames::new(&mut find, display, &extensions).expect("frames are drawn in this context");
        assert!(!frames.ready(&[pitchless]), "a frame of planes of no pitch was imported");
        assert!(frames.refused(), "the context refuses frames");
    }
}
