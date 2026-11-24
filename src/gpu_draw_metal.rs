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

/// What comes before the shading: GLSL's names, and how a texel is read.
const PRELUDE: &str = r#"
#include <metal_stdlib>
using namespace metal;
#define vec2 float2
#define vec3 float3
#define vec4 float4
#define ivec2 int2
#define as_float(bits) as_type<float>(bits)
float4 texel(texture2d<float> cache, texture2d<float> patches, int which, int x, int y) {
    if (which == 0) {
        return cache.read(uint2(x, y));
    }
    return patches.read(uint2(x, y));
}
"#;

/// Each quad's corner from the vertex and instance numbers; each pixel what `shade` gives.
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
fragment float4 pixels(Corner in [[stage_in]], texture2d<float> cache [[texture(0)]], texture2d<float> patches [[texture(1)]]) {
    float4 covers = float4(0.0, 0.0, 0.0, 0.0);
    return shade(cache, patches, in.position.xy, in.shape, in.more, in.source, in.color, in.kind, covers);
}
"#;

/// The shading as Metal Shading Language: each function (a line starting a definition) takes
/// the textures first, each call of one passes them, and the colour coverage comes back
/// through a reference.
pub(crate) fn source() -> String {
    let defined: Vec<&str> = SHADING.lines().filter(|line| !line.starts_with(' ') && line.ends_with('{')).filter_map(|line| line.split('(').next()?.split(' ').next_back()).collect();
    let mut called = defined.clone();
    called.push("texel");
    let mut out = String::from(PRELUDE);
    for line in SHADING.lines() {
        let line = line.replace("out vec4 covers", "thread vec4 &covers");
        let opens = !line.starts_with(' ') && line.ends_with('{');
        out.push_str(&if opens { line.replacen('(', "(texture2d<float> cache, texture2d<float> patches, ", 1) } else { passed(&line, &called) });
        out.push('\n');
    }
    out.push_str(ENTRIES);
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
            out.push_str("cache, patches, ");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What draws: the pipeline, and the cache and patch textures (`SIDE` square, kept).
pub(crate) struct Pipeline {
    state: Obj,
    textures: [Obj; 2],
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
            let made = Pipeline { state, textures };
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
}

impl Drop for Pipeline {
    fn drop(&mut self) {
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
        assert!(source.contains("float inside_of(texture2d<float> cache, texture2d<float> patches, vec2 p, vec4 r, float radius) {"));
        assert!(source.contains("vec4 shade(texture2d<float> cache, texture2d<float> patches, vec2 at, vec4 shape, vec4 more, vec4 source, vec4 color, float kind, thread vec4 &covers) {"));
        assert!(source.contains("return floor(texel(cache, patches, which, int(x), int(y)) * 255.0 + 0.5);"));
        assert!(source.contains("float outer = inside_of(cache, patches, p, r, radius);"));
        assert!(source.contains("mixed(cache, patches, bytes(cache, patches, 1, source.x + left"), "calls within calls");
        assert!(source.contains("vec4 top = mixed("), "a variable named like nothing called is left");
        assert!(!source.contains("out vec4"));
        assert_eq!(passed("x = floor(y) + max(a, b);", &["floor"]), "x = floor(cache, patches, y) + max(a, b);");
        assert_eq!(passed("subcell(e, pixel);", &["cell"]), "subcell(e, pixel);", "a name ending another is not it");
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
