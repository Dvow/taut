//! HDR through Metal on macOS (the feature `hdr`, see `hdr`): a window lit on a screen with EDR
//! headroom presented in extended dynamic range.
//!
//! - The screen: its window's `NSScreen`, whose `maximumExtendedDynamicRangeColorComponentValue`
//!   is the headroom now. It changes with the screen's brightness and what else shows HDR, and
//!   nothing says so: it is read again every run, and a quarter of a second apart while a window
//!   is lit.
//! - While lit there, the window's `CAMetalLayer` takes `RGBA16Float` in extended linear sRGB
//!   (`wantsExtendedDynamicRangeContent`), 1.0 being SDR white; else BGRA8 in sRGB as ever, the
//!   same pixels.
//! - In EDR the window's texture reaches the drawable through a render pass, not a blit: the SDR
//!   colour made linear, offset, times each colour's gain (capped at the headroom), offset
//!   back. The gains are a texture (`RGBA16Float`) their changed part goes up to.
//! - The shaders are compiled by the device from the source here, fast math off.
use crate::gpu_metal::{COPY_IN, PRIVATE, Three};
use crate::hdr::{Gains, Screen};
use crate::macos_sys::{Bool, CGColorSpaceCreateWithName, CGColorSpaceRelease, Obj, class, kCGColorSpaceExtendedLinearSRGB, kCGColorSpaceSRGB, made, ns_string, rust_string, send};
use std::ffi::c_void;

/// `MTLPixelFormatRGBA16Float`: four 16-bit floats.
pub(crate) const HALVES: usize = 115;
/// Texture usage: read by shaders.
const SHADER_READ: usize = 1;
/// A pass's attachment: not loaded, stored.
const LOAD_NOTHING: usize = 0;
const STORE: usize = 1;
/// `MTLPrimitiveTypeTriangle`.
const TRIANGLES: usize = 3;
/// How long a lit window's screen may go unread, at most.
pub(crate) const WATCHED: std::time::Duration = std::time::Duration::from_millis(250);

/// The shaders: one triangle over the drawable, each pixel the window's lit by its gains (red,
/// green, blue, and a gain map's offset) where `light.z` says the window is lit; `light.x` SDR
/// white, `light.y` the most a gain may be.
const SHADERS: &str = "#include <metal_stdlib>
using namespace metal;
struct Corner { float4 at [[position]]; };
vertex Corner corner(uint at [[vertex_id]]) {
    float2 p = float2((at << 1) & 2, at & 2);
    return Corner { float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1) };
}
fragment float4 lit(Corner in [[stage_in]], texture2d<float, access::read> picture [[texture(0)]], texture2d<float, access::read> gains [[texture(1)]], constant float4 &light [[buffer(0)]]) {
    uint2 at = uint2(in.at.xy);
    float3 c = picture.read(at).rgb;
    float3 sdr = select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
    float4 g = light.z > 0.5 ? gains.read(at) : float4(1, 1, 1, 0);
    float3 gain = min(g.rgb, light.y);
    return float4(max((sdr + g.a) * gain - g.a, 0.0) * light.x, 1);
}
";

/// The screen window `window` is on: HDR where it has EDR headroom now (SDR white at 80 nits,
/// its peak that many times brighter).
pub(crate) fn screen_of(window: Obj) -> Screen {
    #[cfg(test)]
    if let Some(faked) = tests::FAKED.get() {
        return faked;
    }
    // SAFETY: a window answers its screen (+0, nil off every screen), and a screen its headroom.
    let most = unsafe {
        let screen: Obj = send(window, c"screen", ());
        if screen.is_null() { 1.0 } else { send::<f64>(screen, c"maximumExtendedDynamicRangeColorComponentValue", ()) }
    };
    Screen { hdr: most > 1.0, white: Screen::SDR.white, peak: Screen::SDR.white * most as f32 }
}

/// The pass from a window's texture to an EDR drawable: its pipeline.
pub(crate) struct Pass {
    state: Obj,
}

impl Pass {
    /// The pass on `device`; nothing where it can't be made (the compiler's words go to standard
    /// error).
    pub(crate) fn new(device: Obj) -> Option<Pass> {
        let mut error: Obj = std::ptr::null_mut();
        // SAFETY: options and functions made and released here; the source is copied by the
        // device; a pipeline of the two functions for one RGBA16Float attachment, kept.
        unsafe {
            let options = made(c"MTLCompileOptions");
            send::<()>(options, c"setFastMathEnabled:", (0 as Bool,));
            let library: Obj = send(device, c"newLibraryWithSource:options:error:", (ns_string(SHADERS), options, &mut error));
            send::<()>(options, c"release", ());
            if library.is_null() {
                let said = rust_string(send(error, c"localizedDescription", ())).unwrap_or_default();
                eprintln!("taut: Metal did not compile the HDR shaders: {said}");
                return None;
            }
            let described: Obj = send(made(c"MTLRenderPipelineDescriptor"), c"autorelease", ());
            let (corner, lit): (Obj, Obj) = (send(library, c"newFunctionWithName:", (ns_string("corner"),)), send(library, c"newFunctionWithName:", (ns_string("lit"),)));
            send::<()>(described, c"setVertexFunction:", (corner,));
            send::<()>(described, c"setFragmentFunction:", (lit,));
            let attachment: Obj = send(send::<Obj>(described, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setPixelFormat:", (HALVES,));
            let state: Obj = send(device, c"newRenderPipelineStateWithDescriptor:error:", (described, &mut error));
            [corner, lit, library].into_iter().for_each(|made| send::<()>(made, c"release", ()));
            (!state.is_null()).then_some(Pass { state })
        }
    }

    /// Encodes into `commands` `picture` (a window's texture) drawn into `target` (an EDR
    /// drawable's texture), lit by `gains` (a texture as large, or none) with `constants`.
    pub(crate) fn draw(&self, commands: Obj, picture: Obj, gains: Option<Obj>, target: Obj, constants: [f32; 2]) {
        let light = [constants[0], constants[1], gains.is_some() as u8 as f32, 0.0];
        // SAFETY: a pass writing all of `target`, which the pipeline's attachment's format is;
        // the constants copied in; both textures are the device's.
        unsafe {
            let pass: Obj = send(class(c"MTLRenderPassDescriptor"), c"renderPassDescriptor", ());
            let attachment: Obj = send(send::<Obj>(pass, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setTexture:", (target,));
            send::<()>(attachment, c"setLoadAction:", (LOAD_NOTHING,));
            send::<()>(attachment, c"setStoreAction:", (STORE,));
            let encoder: Obj = send(commands, c"renderCommandEncoderWithDescriptor:", (pass,));
            if encoder.is_null() {
                return;
            }
            send::<()>(encoder, c"setRenderPipelineState:", (self.state,));
            send::<()>(encoder, c"setFragmentTexture:atIndex:", (picture, 0usize));
            send::<()>(encoder, c"setFragmentTexture:atIndex:", (gains.unwrap_or(picture), 1usize));
            send::<()>(encoder, c"setFragmentBytes:length:atIndex:", (light.as_ptr().cast::<c_void>(), 16usize, 0usize));
            send::<()>(encoder, c"drawPrimitives:vertexStart:vertexCount:", (TRIANGLES, 0usize, 3usize));
            send::<()>(encoder, c"endEncoding", ());
        }
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        // SAFETY: held once, here.
        unsafe { send::<()>(self.state, c"release", ()) };
    }
}

/// A window's HDR: its gains, its screen, whether it is presented in EDR, and the gains'
/// texture (`size` pixels).
#[derive(Debug)]
pub(crate) struct Lit {
    pub(crate) gains: Gains,
    pub(crate) screen: Screen,
    pub(crate) on: bool,
    texture: Obj,
    size: [i32; 2],
}

impl Default for Lit {
    fn default() -> Lit {
        Lit { gains: Gains::default(), screen: Screen::SDR, on: false, texture: std::ptr::null_mut(), size: [0; 2] }
    }
}

impl Lit {
    /// Puts `layer` in EDR (RGBA16Float, extended linear sRGB) or back (BGRA8, sRGB).
    pub(crate) fn switch(&mut self, layer: Obj, on: bool) {
        // SAFETY: the window's layer; the colour space is made and released here (the layer
        // keeps its own); its name is CoreGraphics' constant.
        unsafe {
            let (format, name) = if on { (HALVES, kCGColorSpaceExtendedLinearSRGB) } else { (crate::gpu_metal::BGRA8, kCGColorSpaceSRGB) };
            send::<()>(layer, c"setPixelFormat:", (format,));
            send::<()>(layer, c"setWantsExtendedDynamicRangeContent:", (on as Bool,));
            let space = CGColorSpaceCreateWithName(name);
            send::<()>(layer, c"setColorspace:", (space,));
            CGColorSpaceRelease(space);
        }
        self.on = on;
    }

    /// The pass's constants: SDR white (1.0 in EDR), the most a gain may be.
    pub(crate) fn constants(&self) -> [f32; 2] {
        [1.0, self.screen.headroom()]
    }

    /// Encodes into `commands` the gains that changed put into their texture (all of them into
    /// a new one, made on `device`, where the window has another size); gives the texture,
    /// where there is one.
    pub(crate) fn put_up(&mut self, device: Obj, commands: Obj) -> Option<Obj> {
        let size = self.gains.size;
        if size != self.size {
            // SAFETY: the old texture (+1, or nil) goes; a plain 2D texture made for the shaders.
            unsafe {
                send::<()>(self.texture, c"release", ());
                let described: Obj = send(class(c"MTLTextureDescriptor"), c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:", (HALVES, size[0] as usize, size[1] as usize, 0 as Bool));
                send::<()>(described, c"setUsage:", (SHADER_READ,));
                send::<()>(described, c"setStorageMode:", (PRIVATE,));
                self.texture = send(device, c"newTextureWithDescriptor:", (described,));
            }
            (self.size, self.gains.changed) = (size, Some([0, 0, size[0], size[1]]));
        }
        if self.texture.is_null() {
            return None;
        }
        let Some([left, top, right, bottom]) = self.gains.changed.take() else { return Some(self.texture) };
        let (wide, high) = ((right - left) as usize, (bottom - top) as usize);
        let rows: Vec<[u16; 4]> = (top..bottom).flat_map(|y| &self.gains.plane[y as usize * size[0] as usize + left as usize..][..wide]).copied().collect();
        // SAFETY: a shared buffer holding the rows (+1, released once encoded: the command buffer
        // keeps it), blitted where they lie in the texture.
        unsafe {
            let staged: Obj = send(device, c"newBufferWithBytes:length:options:", (rows.as_ptr().cast::<c_void>(), rows.len() * 8, 0usize));
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            send::<()>(blit, COPY_IN, (staged, 0usize, wide * 8, wide * high * 8, Three([wide, high, 1]), self.texture, 0usize, 0usize, Three([left as usize, top as usize, 0])));
            send::<()>(blit, c"endEncoding", ());
            send::<()>(staged, c"release", ());
        }
        Some(self.texture)
    }
}

impl Drop for Lit {
    fn drop(&mut self) {
        // SAFETY: held once, here (nil answers nothing).
        unsafe { send::<()>(self.texture, c"release", ()) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        /// The screen every window is on, as a test has it.
        pub(crate) static FAKED: Cell<Option<Screen>> = const { Cell::new(None) };
    }
}
