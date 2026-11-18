//! The UI over a program's own Metal rendering (macOS): made once from its device and queue,
//! then drawn each frame with one call, in its render command encoder, after its own drawing.
//!
//! ```ignore
//! // SAFETY: the program's own device and queue, alive while the layer is.
//! let mut layer = unsafe { taut::layer_metal::Layer::new(device, queue, pixel_format) }?;
//! // each frame, in the program's render pass, after its own drawing:
//! unsafe { layer.draw(&mut ui, encoder, [0, 0, width, height]) }?;
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`) and a texture of their size, private to the
//! GPU. `update` redraws only what changed and copies only that into the texture: from a buffer
//! of its own, by a blit in a command buffer of its own, committed to the program's queue. It
//! comes before the frame's command buffer, which the program commits after (unless that was
//! enqueued earlier). `draw` updates, then encodes one quad over the given area of the target:
//! the UI at its top left, pixel for pixel, clipped to it, with premultiplied alpha blending
//! ("one, one minus source alpha"). Metal keeps no state to restore: after `draw` the program
//! sets its own pipeline, viewport, scissor and textures again. A program that shows the UI
//! otherwise calls `update` and samples `texture()`.
//!
//! Nothing is linked: the program's Metal objects are spoken to through the Objective-C runtime,
//! and the shaders are Metal source the program's device compiles when the layer is made. The
//! target's pixel format is the program's; colour is stored as is (an `..._sRGB` format would
//! convert the UI's pixels, which are already in screen colours).
// Every function here is `#[inline]`: only a program that calls one compiles it.
use crate::macos_sys::{Bool, Obj, Pool, class, made, ns_string, rust_string, send};
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::c_void;

/// A Metal error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// What the layer was doing, as "compiling the shaders".
    pub operation: &'static str,
    /// What Metal said.
    pub message: String,
}

impl std::fmt::Display for Error {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.operation, self.message)
    }
}

impl std::error::Error for Error {}

/// `MTLPixelFormatBGRA8Unorm`: the UI's pixels, `0xAARRGGBB` words in memory.
const BGRA8: usize = 80;
/// Texture usage: read by shaders. Storage: private to the GPU.
const SHADER_READ: usize = 1;
const PRIVATE: usize = 2;
/// Blend factors one and one minus source alpha; the add operation.
const ONE: usize = 1;
const ONE_MINUS_SOURCE_ALPHA: usize = 5;
/// `MTLPrimitiveTypeTriangleStrip`.
const TRIANGLE_STRIP: usize = 4;

/// The quad's corners from the vertex number, over all of the viewport; each pixel read from the
/// texture at its place in the area (whose top left the program's draw gives).
const SHADERS: &str = "
#include <metal_stdlib>
using namespace metal;
struct Corner { float4 place [[position]]; };
vertex Corner corners(uint nth [[vertex_id]]) {
    float2 at = float2(nth & 1, nth >> 1);
    Corner corner;
    corner.place = float4(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, 0.0, 1.0);
    return corner;
}
fragment float4 pixels(Corner in [[stage_in]], texture2d<float> picture [[texture(0)]], constant uint2 &origin [[buffer(0)]]) {
    return picture.read(uint2(in.place.xy) - origin);
}
";

/// `MTLViewport`.
#[repr(C)]
struct Viewport {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    near: f64,
    far: f64,
}

/// `MTLScissorRect`, `MTLOrigin`, `MTLSize`.
#[repr(C)]
struct Three([usize; 3]);
#[repr(C)]
struct Scissor([usize; 4]);

/// The UI in a program's Metal: its pixels, a texture of their size, and the pipeline that draws
/// it over the program's picture.
pub struct Layer {
    device: Obj,
    queue: Obj,
    pipeline: Obj,
    /// The texture (private to the GPU); null until the first update.
    texture: Obj,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).finish_non_exhaustive()
    }
}

/// What an `NSError` says, or `fallback` for none.
#[inline]
fn said(error: Obj, fallback: &str) -> String {
    // SAFETY: an error (or nil) answers its description.
    rust_string(unsafe { send(error, c"localizedDescription", ()) }).unwrap_or_else(|| fallback.to_string())
}

impl Layer {
    /// The layer for a program's device (`id<MTLDevice>`) and queue (`id<MTLCommandQueue>`),
    /// drawn into targets of `format` (an `MTLPixelFormat`, as `MTLPixelFormatBGRA8Unorm`, 80).
    ///
    /// # Safety
    /// `device` and `queue` are the program's, made from that device, and outlive the layer.
    #[inline]
    pub unsafe fn new(device: *mut c_void, queue: *mut c_void, format: usize) -> Result<Layer, Error> {
        let _pool = Pool::new();
        let mut error: Obj = std::ptr::null_mut();
        // SAFETY: the device compiles the source (+1 library, released); a pipeline is made from
        // its two functions for one colour attachment of the program's format, blending
        // premultiplied colour. Everything made here is released, but the pipeline, kept.
        unsafe {
            let library: Obj = send(device, c"newLibraryWithSource:options:error:", (ns_string(SHADERS), std::ptr::null_mut::<c_void>(), &mut error));
            if library.is_null() {
                return Err(Error { operation: "compiling the shaders", message: said(error, "no library") });
            }
            let described: Obj = send(made(c"MTLRenderPipelineDescriptor"), c"autorelease", ());
            let (corners, pixels): (Obj, Obj) = (send(library, c"newFunctionWithName:", (ns_string("corners"),)), send(library, c"newFunctionWithName:", (ns_string("pixels"),)));
            send::<()>(described, c"setVertexFunction:", (corners,));
            send::<()>(described, c"setFragmentFunction:", (pixels,));
            let attachment: Obj = send(send::<Obj>(described, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setPixelFormat:", (format,));
            send::<()>(attachment, c"setBlendingEnabled:", (1 as Bool,));
            send::<()>(attachment, c"setSourceRGBBlendFactor:", (ONE,));
            send::<()>(attachment, c"setSourceAlphaBlendFactor:", (ONE,));
            send::<()>(attachment, c"setDestinationRGBBlendFactor:", (ONE_MINUS_SOURCE_ALPHA,));
            send::<()>(attachment, c"setDestinationAlphaBlendFactor:", (ONE_MINUS_SOURCE_ALPHA,));
            let pipeline: Obj = send(device, c"newRenderPipelineStateWithDescriptor:error:", (described, &mut error));
            [corners, pixels, library].into_iter().for_each(|made| send::<()>(made, c"release", ()));
            if pipeline.is_null() {
                return Err(Error { operation: "making the pipeline", message: said(error, "no pipeline") });
            }
            send::<Obj>(device, c"retain", ());
            send::<Obj>(queue, c"retain", ());
            Ok(Layer { device, queue, pipeline, texture: std::ptr::null_mut(), surface: Surface::default() })
        }
    }

    /// Updates the texture after a run: redraws what changed and copies only that into it, by a
    /// blit committed to the program's queue. (The first time, and after a resize, the texture
    /// is made anew and all of it copied.)
    ///
    /// # Safety
    /// The layer's device and queue are alive.
    #[inline]
    pub unsafe fn update(&mut self, ui: &mut Ui) -> Result<(), Error> {
        let size = (self.surface.width, self.surface.height);
        let changed = self.surface.update(ui);
        let (width, height) = (self.surface.width, self.surface.height);
        if width * height == 0 {
            return Ok(());
        }
        let _pool = Pool::new();
        let whole = (width, height) != size || self.texture.is_null();
        if whole {
            // SAFETY: as the caller promises.
            unsafe { self.make_texture() }?;
        }
        let boxes: Vec<[usize; 4]> = if whole { vec![[0, 0, width, height]] } else { changed };
        if boxes.is_empty() {
            return Ok(());
        }
        // Each box's rows, one after another, in one buffer.
        let mut rows = Vec::with_capacity(boxes.iter().map(|[left, top, right, bottom]| (right - left) * (bottom - top)).sum());
        for [left, top, right, bottom] in &boxes {
            (*top..*bottom).for_each(|row| rows.extend_from_slice(&self.surface.pixels[row * width + left..row * width + right]));
        }
        // SAFETY: a shared buffer holding the rows (+1, released once encoded: the command
        // buffer keeps it); each copy reads one box's rows from where they start and writes
        // them where the box lies in the texture, which is the surface's size.
        unsafe {
            let staged: Obj = send(self.device, c"newBufferWithBytes:length:options:", (rows.as_ptr().cast::<c_void>(), rows.len() * 4, 0usize));
            if staged.is_null() {
                return Err(Error { operation: "making the upload buffer", message: format!("no buffer of {} bytes", rows.len() * 4) });
            }
            let commands: Obj = send(self.queue, c"commandBuffer", ());
            let blit: Obj = send(commands, c"blitCommandEncoder", ());
            let mut from = 0;
            for [left, top, right, bottom] in boxes {
                let (wide, high) = (right - left, bottom - top);
                let copy = c"copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:";
                send::<()>(blit, copy, (staged, from * 4, wide * 4, wide * high * 4, Three([wide, high, 1]), self.texture, 0usize, 0usize, Three([left, top, 0])));
                from += wide * high;
            }
            send::<()>(blit, c"endEncoding", ());
            send::<()>(commands, c"commit", ());
            send::<()>(staged, c"release", ());
        }
        Ok(())
    }

    /// A texture of the surface's size, private to the GPU, for the old one.
    ///
    /// # Safety
    /// The layer's device is alive.
    #[inline]
    unsafe fn make_texture(&mut self) -> Result<(), Error> {
        let (width, height) = (self.surface.width, self.surface.height);
        // SAFETY: a descriptor for a plain 2D texture of one level; the old texture (+1) is
        // released, the new one (+1) kept.
        unsafe {
            let described: Obj = send(class(c"MTLTextureDescriptor"), c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:", (BGRA8, width, height, 0 as Bool));
            send::<()>(described, c"setUsage:", (SHADER_READ,));
            send::<()>(described, c"setStorageMode:", (PRIVATE,));
            let texture: Obj = send(self.device, c"newTextureWithDescriptor:", (described,));
            send::<()>(self.texture, c"release", ());
            self.texture = texture;
        }
        if self.texture.is_null() {
            self.surface = Surface::default(); // redrawn whole, and retried, next time
            return Err(Error { operation: "making the texture", message: format!("no texture of {width} by {height} pixels") });
        }
        Ok(())
    }

    /// Updates the texture (`update`) and encodes it over `area` of the target (left, top, width,
    /// height, in pixels), in the program's render command encoder: at the area's top left,
    /// pixel for pixel, clipped to it.
    ///
    /// # Safety
    /// `encoder` is an `id<MTLRenderCommandEncoder>` of the layer's device, encoding into a
    /// target of the layer's format that holds `area`.
    #[inline]
    pub unsafe fn draw(&mut self, ui: &mut Ui, encoder: *mut c_void, area: [usize; 4]) -> Result<(), Error> {
        // SAFETY: as the caller promises.
        unsafe { self.update(ui) }?;
        let [left, top, wide, high] = area;
        let (width, height) = (self.surface.width, self.surface.height);
        if width * height * wide * high == 0 {
            return Ok(());
        }
        let viewport = Viewport { x: left as f64, y: top as f64, width: width as f64, height: height as f64, near: 0.0, far: 1.0 };
        let origin = [left as u32, top as u32];
        // SAFETY: the pipeline and texture are the layer's; the viewport covers the UI at the
        // area's top left, the scissor the part of the area it covers.
        unsafe {
            send::<()>(encoder, c"setRenderPipelineState:", (self.pipeline,));
            send::<()>(encoder, c"setViewport:", (viewport,));
            send::<()>(encoder, c"setScissorRect:", (Scissor([left, top, wide.min(width), high.min(height)]),));
            send::<()>(encoder, c"setFragmentTexture:atIndex:", (self.texture, 0usize));
            send::<()>(encoder, c"setFragmentBytes:length:atIndex:", (origin.as_ptr().cast::<c_void>(), 8usize, 0usize));
            send::<()>(encoder, c"drawPrimitives:vertexStart:vertexCount:", (TRIANGLE_STRIP, 0usize, 4usize));
        }
        Ok(())
    }

    /// The texture (`id<MTLTexture>`, `MTLPixelFormatBGRA8Unorm`, premultiplied alpha, rows from
    /// the top), as `update` left it: null before the first.
    #[inline]
    pub fn texture(&self) -> *mut c_void {
        self.texture
    }

    /// The UI's pixels, which the texture holds.
    #[inline]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }
}

impl Drop for Layer {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: what the layer made or retained, released once (nil answers nothing).
        unsafe { [self.texture, self.pipeline, self.queue, self.device].into_iter().for_each(|held| send::<()>(held, c"release", ())) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MTLClearColor`.
    #[repr(C)]
    struct Clear {
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    }

    /// `MTLRegion`.
    #[repr(C)]
    struct Region {
        origin: Three,
        size: Three,
    }

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> Obj;
    }

    /// Draws the layer over a grey target in a pass of its own, and reads the target back.
    fn drawn(layer: &mut Layer, ui: &mut Ui, device: Obj, queue: Obj, [width, height]: [usize; 2], area: [usize; 4]) -> Vec<u32> {
        // SAFETY: a shared target the test makes, cleared to grey, drawn into and read back
        // once its command buffer completed.
        unsafe {
            let described: Obj = send(class(c"MTLTextureDescriptor"), c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:", (BGRA8, width, height, 0 as Bool));
            send::<()>(described, c"setUsage:", (4usize | SHADER_READ,));
            send::<()>(described, c"setStorageMode:", (0usize,));
            let target: Obj = send(device, c"newTextureWithDescriptor:", (described,));
            let pass: Obj = send(class(c"MTLRenderPassDescriptor"), c"renderPassDescriptor", ());
            let attachment: Obj = send(send::<Obj>(pass, c"colorAttachments", ()), c"objectAtIndexedSubscript:", (0usize,));
            send::<()>(attachment, c"setTexture:", (target,));
            send::<()>(attachment, c"setLoadAction:", (2usize,));
            send::<()>(attachment, c"setStoreAction:", (1usize,));
            send::<()>(attachment, c"setClearColor:", (Clear { red: 0.5, green: 0.5, blue: 0.5, alpha: 1.0 },));
            let commands: Obj = send(queue, c"commandBuffer", ());
            let encoder: Obj = send(commands, c"renderCommandEncoderWithDescriptor:", (pass,));
            layer.draw(ui, encoder, area).expect("the layer draws");
            send::<()>(encoder, c"endEncoding", ());
            send::<()>(commands, c"commit", ());
            send::<()>(commands, c"waitUntilCompleted", ());
            let mut pixels = vec![0u32; width * height];
            let region = Region { origin: Three([0, 0, 0]), size: Three([width, height, 1]) };
            send::<()>(target, c"getBytes:bytesPerRow:fromRegion:mipmapLevel:", (pixels.as_mut_ptr().cast::<c_void>(), width * 4, region, 0usize));
            send::<()>(target, c"release", ());
            pixels
        }
    }

    /// A premultiplied pixel over an opaque one, as the blend does it.
    fn over(top: u32, under: u32) -> u32 {
        let alpha = top >> 24;
        (0..4).fold(0, |all, part| {
            let (a, b) = (top >> (part * 8) & 255, under >> (part * 8) & 255);
            all | ((a + (b * (255 - alpha) + 127) / 255).min(255)) << (part * 8)
        })
    }

    #[test]
    fn the_ui_is_drawn_over_the_programs_picture_where_asked_and_what_changed_follows() {
        let _pool = Pool::new();
        // SAFETY: the system's device and a queue of it (+1 each, held by the layer too).
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        if device.is_null() {
            return eprintln!("no Metal device here: not tested");
        }
        let queue: Obj = unsafe { send(device, c"newCommandQueue", ()) };
        let mut layer = unsafe { Layer::new(device, queue, BGRA8) }.expect("the layer is made");
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 120.0, h: 60.0 }, 1.0);
        let mut count = 0;
        let mut build = |ui: &mut Ui| {
            ui.column().pad(8.0).open();
            count += ui.button(&format!("Pressed {count}")).clicked as u32;
            ui.close();
        };
        ui.run(&mut build);
        let (size, area, grey) = ([160, 100], [20, 10, 100, 80], 0xff80_8080u32);
        for round in 0..2 {
            let target = drawn(&mut layer, &mut ui, device, queue, size, area);
            let pixels = &layer.surface().pixels;
            // Inside the area, the UI over grey; outside it (and past the area's width), grey.
            for (at, got) in target.iter().enumerate() {
                let (x, y) = (at % size[0], at / size[0]);
                let inside = x >= area[0] && y >= area[1] && x < area[0] + area[2] && y < area[1] + area[3];
                let want = if inside { over(pixels[(y - area[1]) * 120 + x - area[0]], grey) } else { grey };
                let near = (0..4).all(|part| (got >> (part * 8) & 255).abs_diff(want >> (part * 8) & 255) <= 1);
                assert!(near, "round {round}: at {x}, {y} the target has {got:08x}, not {want:08x}");
            }
            // A click: the button's label changes, and only that is copied the next time.
            ui.event(crate::Event::Move(crate::geom::pt(30.0, 20.0)));
            ui.event(crate::Event::Press(crate::Button::Left, crate::Mods::NONE));
            ui.event(crate::Event::Release(crate::Button::Left));
            ui.run(&mut build);
        }
        assert_eq!(count, 2);
        unsafe { send::<()>(queue, c"release", ()) };
        unsafe { send::<()>(device, c"release", ()) };
    }
}
