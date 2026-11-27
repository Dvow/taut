//! Metal drawing (the feature `gpu-draw`): a recorded paint (`gpu_draw::DrawList`) drawn into a
//! window's texture in a render pass of the frame's command buffer.
//!
//! - The shaders are `gpu_draw::SHADING`, the same math as HLSL's and GLSL's, made Metal Shading
//!   Language by `source` and compiled by the device when Metal is opened, fast math off.
//! - Metal has no textures a function can reach but through its arguments: `source` gives every
//!   function of the shading the two textures as its first arguments, and every call passes them.
//! - The quads go up in a buffer the vertex function reads by instance; glyphs into a kept cache
//!   texture, patches, pictures, stops and edges into a texture of this part's. Both hold
//!   `0xAARRGGBB` words as BGRA, so a texel reads red first.
//! - No GDI runs here: blending is "one, one minus source alpha".
use crate::gpu_draw::{DrawList, Quad, SHADING, SIDE};
use crate::gpu_metal::{COPY_IN, Three};
use crate::macos_sys::{Bool, Obj, class, made, ns_string, send};
use std::ffi::c_void;

/// Blend factors one and one minus source alpha.
const ONE: usize = 1;
const ONE_MINUS_SOURCE_ALPHA: usize = 5;
/// `MTLPrimitiveTypeTriangleStrip`.
const TRIANGLE_STRIP: usize = 4;
/// Texture usage: read by shaders.
const SHADER_READ: usize = 1;
/// `MTLViewport` (left, top, width, height, near, far) and `MTLScissorRect` (left, top, width,
/// height).
#[repr(C)]
struct Viewport([f64; 6]);
#[repr(C)]
struct Scissor([usize; 4]);
/// `MTLLoadActionLoad`, `MTLStoreActionStore`.
const LOAD: usize = 1;
const STORE: usize = 1;

/// What comes before the shading: GLSL's names.
const PRELUDE: &str = r#"
#include <metal_stdlib>
using namespace metal;
#define vec2 float2
#define vec3 float3
#define vec4 float4
#define ivec2 int2
#define as_float(bits) as_type<float>(bits)
"#;

/// The textures every function of the shading takes first (`TAKEN`) and every call passes
/// (`PASSED`); how a texel is read; the pixel function's textures. With `video` also the
/// frames (`gpu_draw::FRAME`): texture 2 + n is frame n, opaque.
#[cfg(not(feature = "video"))]
const TAKEN: &str = "texture2d<float> cache, texture2d<float> patches, ";
#[cfg(not(feature = "video"))]
const PASSED: &str = "cache, patches, ";
#[cfg(not(feature = "video"))]
const TEXEL: &str = r#"
float4 texel(texture2d<float> cache, texture2d<float> patches, int which, int x, int y) {
    if (which == 0) {
        return cache.read(uint2(x, y));
    }
    return patches.read(uint2(x, y));
}
"#;
#[cfg(not(feature = "video"))]
const BOUND: &str = "texture2d<float> cache [[texture(0)]], texture2d<float> patches [[texture(1)]]";
#[cfg(feature = "video")]
const TAKEN: &str = "texture2d<float> cache, texture2d<float> patches, array<texture2d<float>, 4> frames, ";
#[cfg(feature = "video")]
const PASSED: &str = "cache, patches, frames, ";
#[cfg(feature = "video")]
const TEXEL: &str = r#"
#define FRAMES
float4 texel(texture2d<float> cache, texture2d<float> patches, array<texture2d<float>, 4> frames, int which, int x, int y) {
    if (which == 0) {
        return cache.read(uint2(x, y));
    } else if (which == 1) {
        return patches.read(uint2(x, y));
    }
    return float4(frames[which - 2].read(uint2(x, y)).rgb, 1.0);
}
"#;
#[cfg(feature = "video")]
const BOUND: &str = "texture2d<float> cache [[texture(0)]], texture2d<float> patches [[texture(1)]], array<texture2d<float>, 4> frames [[texture(2)]]";

/// Each quad's corner from the vertex and instance numbers; each pixel what `shade` gives
/// (`pixels`, made with the textures bound).
const ENTRIES: &str = r#"
struct Quad { float4 place; float4 shape; float4 more; float4 source; float4 color; float4 kind; };
struct Corner {
    float4 position [[position]];
    float4 shape [[flat]];
    float4 more [[flat]];
    float4 source [[flat]];
    float4 color [[flat]];
    float kind [[flat]];
};
vertex Corner corners(uint nth [[vertex_id]], uint quad [[instance_id]], const device Quad* quads [[buffer(0)]]) {
    Quad q = quads[quad];
    float2 p = float2((nth & 1) != 0 ? q.place.z : q.place.x, (nth & 2) != 0 ? q.place.w : q.place.y);
    Corner corner;
    corner.position = float4(p.x / q.kind.z * 2.0 - 1.0, 1.0 - p.y / q.kind.w * 2.0, 0.0, 1.0);
    corner.shape = q.shape;
    corner.more = q.more;
    corner.source = q.source;
    corner.color = q.color;
    corner.kind = q.kind.x;
    return corner;
}
"#;

/// The pixel function, of the textures `BOUND`.
fn pixels() -> String {
    format!(
        "fragment float4 pixels(Corner in [[stage_in]], {BOUND}) {{\n    float4 covers = float4(0.0, 0.0, 0.0, 0.0);\n    return shade({PASSED}in.position.xy, in.shape, in.more, in.source, in.color, in.kind, covers);\n}}\n"
    )
}

/// The shading as Metal Shading Language: each function (a line starting a definition) takes
/// the textures first, each call of one passes them, and the colour coverage comes back
/// through a reference.
pub(crate) fn source() -> String {
    let defined: Vec<&str> = SHADING.lines().filter(|line| !line.starts_with(' ') && line.ends_with('{')).filter_map(|line| line.split('(').next()?.split(' ').next_back()).collect();
    let mut called = defined.clone();
    called.push("texel");
    let mut out = format!("{PRELUDE}{TEXEL}");
    for line in SHADING.lines() {
        let line = line.replace("out vec4 covers", "thread vec4 &covers");
        let opens = !line.starts_with(' ') && line.ends_with('{');
        out.push_str(&if opens { line.replacen('(', &format!("({TAKEN}"), 1) } else { passed(&line, &called) });
        out.push('\n');
    }
    out.push_str(ENTRIES);
    out.push_str(&pixels());
    out
}

/// A line with the textures passed to each call of the functions `called`.
fn passed(line: &str, called: &[&str]) -> String {
    let mut out = String::with_capacity(line.len() + 32);
    let mut rest = line;
    while let Some(at) = rest.find('(') {
        let (before, after) = rest.split_at(at + 1);
        let name = before[..at].rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_').next().unwrap_or("");
        out.push_str(before);
        if called.contains(&name) {
            out.push_str(PASSED);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What draws: the pipeline, the cache and patch textures (`SIDE` square, kept) and, with
/// `video`, what draws videos' frames.
pub(crate) struct Pipeline {
    state: Obj,
    textures: [Obj; 2],
    #[cfg(feature = "video")]
    frames: Option<Frames>,
}

/// Videos' frames drawn from their pixel buffers: CoreVideo's cache of Metal textures of them
/// (`video_apple::metal_texture_cache`), and those of each frame's commands not yet done.
#[cfg(feature = "video")]
struct Frames {
    cache: *mut c_void,
    held: Vec<Held>,
}

/// What a frame's commands read of videos' frames, held until the GPU is done with it: the
/// command buffer (+1), the CoreVideo textures (+1 each), and the frames (so their pixel buffers
/// go back to the decoder only then).
#[cfg(feature = "video")]
struct Held {
    commands: Obj,
    textures: Vec<*mut c_void>,
    _frames: Vec<std::sync::Arc<crate::video::FrameTexture>>,
}

/// `MTLCommandBufferStatusCommitted` and `…Completed`: from the first to before the second,
/// the GPU is yet to be done with a command buffer.
#[cfg(feature = "video")]
const COMMITTED: usize = 2;
#[cfg(feature = "video")]
const COMPLETED: usize = 4;

#[cfg(feature = "video")]
impl Held {
    /// The command buffer's status.
    fn status(&self) -> usize {
        // SAFETY: a live command buffer answers its status.
        unsafe { send::<usize>(self.commands, c"status", ()) }
    }
    /// Whether the GPU is done with it.
    fn done(&self) -> bool {
        self.status() >= COMPLETED
    }
}

#[cfg(feature = "video")]
impl Drop for Held {
    /// Lets go of it once the GPU is done: waits for a command buffer committed, not yet done.
    /// (One never committed is never read.)
    fn drop(&mut self) {
        // SAFETY: what this holds, released once.
        unsafe {
            if (COMMITTED..COMPLETED).contains(&self.status()) {
                send::<()>(self.commands, c"waitUntilCompleted", ());
            }
            self.textures.iter().for_each(|texture| crate::macos_sys::CFRelease(*texture));
            send::<()>(self.commands, c"release", ());
        }
    }
}

#[cfg(feature = "video")]
impl Drop for Frames {
    fn drop(&mut self) {
        self.held.clear();
        // SAFETY: the cache this made, released once, after the textures it made.
        unsafe { crate::macos_sys::CFRelease(self.cache) };
    }
}

impl std::fmt::Debug for Pipeline {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").finish_non_exhaustive()
    }
}

impl Pipeline {
    /// What `device` draws with: the shading compiled (fast math off), a pipeline drawing into
    /// BGRA, the two textures. Nothing where any part can't be made (the compiler's words go to
    /// standard error).
    pub(crate) fn new(device: Obj) -> Option<Pipeline> {
        let library = Pipeline::library(device)?;
        // SAFETY: a pipeline of the library's two functions for one BGRA colour attachment,
        // blending premultiplied colour; what is made here is released, but the pipeline and
        // the textures, kept.
        unsafe {
            let described: Obj = send(made(c"MTLRenderPipelineDescriptor"), c"autorelease", ());
            let (corners, pixels): (Obj, Obj) = (send(library, c"newFunctionWithName:", (ns_string("corners"),)), send(library, c"newFunctionWithName:", (ns_string("pixels"),)));
            send::<()>(described, c"setVertexFunction:", (corners,));
            send::<()>(described, c"setFragmentFunction:", (pixels,));
            let attachment: Obj = send(send::<Obj>(described, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setPixelFormat:", (crate::gpu_metal::BGRA8,));
            send::<()>(attachment, c"setBlendingEnabled:", (1 as Bool,));
            [c"setSourceRGBBlendFactor:", c"setSourceAlphaBlendFactor:"].into_iter().for_each(|name| send::<()>(attachment, name, (ONE,)));
            [c"setDestinationRGBBlendFactor:", c"setDestinationAlphaBlendFactor:"].into_iter().for_each(|name| send::<()>(attachment, name, (ONE_MINUS_SOURCE_ALPHA,)));
            let mut error: Obj = std::ptr::null_mut();
            let state: Obj = send(device, c"newRenderPipelineStateWithDescriptor:error:", (described, &mut error));
            [corners, pixels, library].into_iter().for_each(|made| send::<()>(made, c"release", ()));
            let textures = [0, 1].map(|_| Pipeline::texture(device));
            #[cfg(not(feature = "video"))]
            let made = Pipeline { state, textures };
            #[cfg(feature = "video")]
            let made = Pipeline { state, textures, frames: crate::video_apple::metal_texture_cache(device).map(|cache| Frames { cache, held: Vec::new() }) };
            (!state.is_null() && !textures.contains(&std::ptr::null_mut())).then_some(made)
        }
    }

    /// The shading compiled by `device` (+1), fast math off; nothing where it fails.
    fn library(device: Obj) -> Option<Obj> {
        let mut error: Obj = std::ptr::null_mut();
        // SAFETY: options made and released here; the source is copied by the device.
        unsafe {
            let options = made(c"MTLCompileOptions");
            send::<()>(options, c"setFastMathEnabled:", (0 as Bool,));
            let library: Obj = send(device, c"newLibraryWithSource:options:error:", (ns_string(&source()), options, &mut error));
            send::<()>(options, c"release", ());
            if library.is_null() {
                let said = crate::macos_sys::rust_string(send(error, c"localizedDescription", ())).unwrap_or_default();
                eprintln!("taut: Metal did not compile the drawing shaders: {said}");
            }
            (!library.is_null()).then_some(library)
        }
    }

    /// A `SIDE` square texture the shaders read (+1).
    fn texture(device: Obj) -> Obj {
        // SAFETY: a descriptor of a plain 2D texture, handed to the device.
        unsafe {
            let described: Obj =
                send(class(c"MTLTextureDescriptor"), c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:", (crate::gpu_metal::BGRA8, SIDE as usize, SIDE as usize, 0 as Bool));
            send::<()>(described, c"setUsage:", (SHADER_READ,));
            send::<()>(described, c"setStorageMode:", (crate::gpu_metal::PRIVATE,));
            send(device, c"newTextureWithDescriptor:", (described,))
        }
    }

    /// Encodes `list` into `commands`: its texels put in, then its quads drawn into `target` (a
    /// window's texture and its size) inside `area` (left, top, right, bottom), blended over
    /// what is there. Says whether it was all encoded.
    pub(crate) fn draw(&mut self, device: Obj, commands: Obj, (target, size): (Obj, [usize; 2]), list: &DrawList, area: [i32; 4]) -> bool {
        let [left, top] = [area[0].max(0) as usize, area[1].max(0) as usize];
        let [right, bottom] = [(area[2].max(0) as usize).min(size[0]), (area[3].max(0) as usize).min(size[1])];
        if !self.put_up(device, commands, list) {
            return false;
        }
        if list.quads.is_empty() || left >= right || top >= bottom {
            return true;
        }
        #[cfg(feature = "video")]
        let Some(frames) = self.frame_textures(commands, &list.frames) else { return false };
        // SAFETY: the quads in a buffer of their own (+1, released once encoded: the command
        // buffer keeps it); a pass loading and storing the window's texture; the viewport all of
        // it, the scissor inside it.
        unsafe {
            let quads: Obj = send(device, c"newBufferWithBytes:length:options:", (list.quads.as_ptr().cast::<c_void>(), list.quads.len() * size_of::<Quad>(), 0usize));
            let pass: Obj = send(class(c"MTLRenderPassDescriptor"), c"renderPassDescriptor", ());
            let attachment: Obj = send(send::<Obj>(pass, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setTexture:", (target,));
            send::<()>(attachment, c"setLoadAction:", (LOAD,));
            send::<()>(attachment, c"setStoreAction:", (STORE,));
            let encoder: Obj = send(commands, c"renderCommandEncoderWithDescriptor:", (pass,));
            if quads.is_null() || encoder.is_null() {
                send::<()>(encoder, c"endEncoding", ());
                send::<()>(quads, c"release", ());
                return false;
            }
            send::<()>(encoder, c"setRenderPipelineState:", (self.state,));
            send::<()>(encoder, c"setViewport:", (Viewport([0.0, 0.0, size[0] as f64, size[1] as f64, 0.0, 1.0]),));
            send::<()>(encoder, c"setScissorRect:", (Scissor([left, top, right - left, bottom - top]),));
            send::<()>(encoder, c"setVertexBuffer:offset:atIndex:", (quads, 0usize, 0usize));
            send::<()>(encoder, c"setFragmentTexture:atIndex:", (self.textures[0], 0usize));
            send::<()>(encoder, c"setFragmentTexture:atIndex:", (self.textures[1], 1usize));
            // (Each frame's slot holds a texture: the patches where there is no frame.)
            #[cfg(feature = "video")]
            for nth in 0..crate::gpu_draw::MOST_FRAMES {
                send::<()>(encoder, c"setFragmentTexture:atIndex:", (frames.get(nth).copied().unwrap_or(self.textures[1]), 2 + nth));
            }
            send::<()>(encoder, c"drawPrimitives:vertexStart:vertexCount:instanceCount:", (TRIANGLE_STRIP, 0usize, 4usize, list.quads.len()));
            send::<()>(encoder, c"endEncoding", ());
            send::<()>(quads, c"release", ());
        }
        true
    }

    /// Encodes each of `list`'s uploads into its texture, from one buffer of its staging.
    fn put_up(&self, device: Obj, commands: Obj, list: &DrawList) -> bool {
        if list.uploads.is_empty() {
            return true;
        }
        // SAFETY: a shared buffer of the staging (+1, released once encoded); each upload's rows
        // lie in it from where it says, as wide as it is, and inside its texture.
        unsafe {
            let staged: Obj = send(device, c"newBufferWithBytes:length:options:", (list.staging.as_ptr().cast::<c_void>(), list.staging.len() * 4, 0usize));
            if staged.is_null() {
                return false;
            }
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            for upload in &list.uploads {
                let [x, y, wide, high] = upload.at.map(|side| side as usize);
                let texture = self.textures[upload.texture as usize];
                send::<()>(blit, COPY_IN, (staged, upload.from * 4, wide * 4, wide * high * 4, Three([wide, high, 1]), texture, 0usize, 0usize, Three([x, y, 0])));
            }
            send::<()>(blit, c"endEncoding", ());
            send::<()>(staged, c"release", ());
        }
        true
    }

    /// Whether it draws videos' frames from their pixel buffers.
    #[cfg(feature = "video")]
    pub(crate) fn takes_frames(&self) -> bool {
        self.frames.is_some()
    }

    /// Lets go of the frames `commands` held: a window's commands it let go of uncommitted, which
    /// the GPU never reads.
    #[cfg(feature = "video")]
    pub(crate) fn let_go_of_frames_of(&mut self, commands: Obj) {
        if let Some(frames) = self.frames.as_mut() {
            frames.held.retain(|held| held.commands != commands);
        }
    }

    /// Metal textures of `frames` (as the shaders read them, frame n at texture 2 + n), held
    /// with `commands` until the GPU is done with them; nothing where one can't be made. What
    /// earlier commands held is let go of where they are done.
    #[cfg(feature = "video")]
    fn frame_textures(&mut self, commands: Obj, frames: &[std::sync::Arc<crate::video::FrameTexture>]) -> Option<Vec<Obj>> {
        if frames.is_empty() {
            return Some(Vec::new());
        }
        let held = self.frames.as_mut()?;
        held.held.retain(|commands| !commands.done());
        crate::video_apple::flush_metal_textures(held.cache);
        let (mut textures, mut drawn): (Vec<*mut c_void>, Vec<Obj>) = (Vec::with_capacity(frames.len()), Vec::with_capacity(frames.len()));
        for frame in frames {
            let Some((texture, seen)) = frame.metal_texture(held.cache) else {
                // SAFETY: made just now, used by nothing.
                textures.iter().for_each(|texture| unsafe { crate::macos_sys::CFRelease(*texture) });
                return None;
            };
            textures.push(texture);
            drawn.push(seen);
        }
        // SAFETY: the frame's command buffer, retained while held.
        let commands = unsafe { send(commands, c"retain", ()) };
        held.held.push(Held { commands, textures, _frames: frames.to_vec() });
        Some(drawn)
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        #[cfg(feature = "video")]
        drop(self.frames.take());
        // SAFETY: what this made, released once (nil answers nothing).
        unsafe { [self.state, self.textures[0], self.textures[1]].into_iter().for_each(|held| send::<()>(held, c"release", ())) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_draw::tests::{Scene, compare};
    use crate::gpu_metal::tests::{gpu, read, shown};
    use crate::macos_sys::Pool;

    #[test]
    fn every_function_takes_the_textures_and_every_call_passes_them() {
        let source = source();
        assert!(source.contains(&format!("float inside_of({TAKEN}vec2 p, vec4 r, float radius) {{")));
        assert!(source.contains(&format!("vec4 shade({TAKEN}vec2 at, vec4 shape, vec4 more, vec4 source, vec4 color, float kind, thread vec4 &covers) {{")));
        assert!(source.contains(&format!("return floor(texel({PASSED}which, int(x), int(y)) * 255.0 + 0.5);")));
        assert!(source.contains(&format!("float outer = inside_of({PASSED}p, r, radius);")));
        assert!(source.contains(&format!("mixed({PASSED}bytes({PASSED}which, source.x + left")), "calls within calls");
        assert!(source.contains("vec4 top = mixed("), "a variable named like nothing called is left");
        assert!(source.contains(&format!("return shade({PASSED}in.position.xy")), "the pixel function passes what it is bound");
        assert!(!source.contains("out vec4"));
        assert_eq!(passed("x = floor(y) + max(a, b);", &["floor"]), format!("x = floor({PASSED}y) + max(a, b);"));
        assert_eq!(passed("subcell(e, pixel);", &["cell"]), "subcell(e, pixel);", "a name ending another is not it");
    }

    /// `list` drawn by `gpu` into a window `size` pixels whose texture first held `under`, and
    /// read back.
    #[cfg(feature = "video")]
    fn drawn(gpu: &mut crate::gpu_metal::Gpu, list: crate::gpu_draw::DrawList, size: [i32; 2], under: &[u32]) -> Vec<u32> {
        let (_layer, mut shows) = shown(gpu, size.map(|side| side as usize));
        let under = crate::Surface { width: size[0] as usize, height: size[1] as usize, pixels: under.to_vec() };
        gpu.put(&mut shows, &under, [0, 0, under.width, under.height]);
        assert!(gpu.draw(&mut shows, list, [0, 0, size[0], size[1]]), "Metal drew it");
        gpu.show(&mut shows, false);
        let texture = shows.texture();
        let pixels = read(gpu, &mut shows, texture);
        gpu.forget(shows);
        pixels
    }

    #[cfg(feature = "video")]
    #[test]
    fn metal_draws_a_frame_from_its_pixel_buffer_as_its_pixels_read_back() {
        use crate::gpu_draw::tests::{FRAME_PLACES, frame_part};
        let _pool = Pool::new();
        let _one = crate::gpu_metal::tests::one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        let pixels: Vec<u32> = (0..64u32 * 48).map(|at| 0xff00_0000 | at.wrapping_mul(2_654_435_761) >> 8).collect();
        let frame = crate::video_apple::tests::frame_of(64, &pixels).expect("a frame");
        let (size, under) = ([220, 170], vec![0xff20_2830; 220 * 170]);
        for r in FRAME_PLACES {
            let from_buffer = drawn(&mut gpu, frame_part(&frame, size, r, true), size, &under);
            let read_back = drawn(&mut gpu, frame_part(&frame, size, r, false), size, &under);
            let differ = from_buffer.iter().zip(&read_back).filter(|(a, b)| a != b).count();
            assert_eq!(differ, 0, "at {r:?}: {differ} pixels differ from the frame read back and drawn as a picture");
        }
    }

    #[test]
    fn metal_draws_every_kind_as_the_cpu_does() {
        let _pool = Pool::new();
        let _one = crate::gpu_metal::tests::one();
        let Some(mut gpu) = gpu() else { return eprintln!("no Metal device here: not tested") };
        assert!(gpu.drawing([0, 0, 4096, 4096]).is_some(), "the shaders compiled");
        for scale in [1.0, 1.5, 2.0] {
            let mut scene = Scene::new(scale);
            let (_under, mut shows) = shown(&mut gpu, scene.size.map(|side| side as usize));
            let under = crate::Surface { width: scene.size[0] as usize, height: scene.size[1] as usize, pixels: scene.under.clone() };
            gpu.put(&mut shows, &under, [0, 0, under.width, under.height]);
            let mut list = std::mem::take(&mut *gpu.list);
            scene.record(&mut list);
            assert!(!list.failed);
            assert!(gpu.draw(&mut shows, list, [0, 0, scene.size[0], scene.size[1]]), "Metal drew it");
            gpu.show(&mut shows, false);
            let texture = shows.texture();
            let drawn = read(&gpu, &mut shows, texture);
            compare(&scene, &drawn, &format!("Metal at {scale}"));
            // Again: the glyphs are kept, and only a part is drawn.
            let mut list = std::mem::take(&mut *gpu.list);
            scene.record(&mut list);
            assert!(list.uploads.iter().all(|upload| upload.texture == 1), "kept glyphs are not put in again");
            assert!(gpu.draw(&mut shows, list, [0, 0, scene.size[0], scene.size[1] / 2]));
            gpu.show(&mut shows, false);
            compare(&scene, &read(&gpu, &mut shows, texture), &format!("Metal at {scale}, again"));
            gpu.forget(shows);
        }
        assert!(gpu.has_drawn());
    }
}
