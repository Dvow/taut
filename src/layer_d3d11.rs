//! The UI over a program's own Direct3D 11 rendering: created once from its device and context,
//! then drawn each frame with one call, after the program's own drawing.
//!
//! ```ignore
//! // SAFETY: the program's own device, and its context.
//! let mut layer = unsafe { taut::layer_d3d11::Layer::new(device, context) }?;
//! // each frame, once the program's picture is in the render target it has bound:
//! layer.draw(&mut ui)?;
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`) and a texture of the same size. `draw` redraws
//! only what changed since the last call, uploads only that, and draws the texture as one quad
//! over the bound render target, at the viewport's top left, clipped to it, pixel for pixel,
//! with premultiplied alpha blending ("one, one minus source alpha"). Everything it sets on the
//! context (shaders, texture, blend, depth, rasterizer, viewport) is restored. A program that
//! shows the UI otherwise (on a surface in its 3D world, say) calls `update` and draws
//! `texture()` itself.
//!
//! Nothing is loaded or compiled at runtime: the Direct3D calls are declared here as the
//! Windows SDK's d3d11.h declares them, COM methods called by vtable index (noted at each
//! call), and both shaders are precompiled. They are shader model 4, needing feature level 10.0
//! or higher. The render target must store colour linearly (`..._UNORM`, not `..._UNORM_SRGB`):
//! the UI's pixels are already in screen colours.
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

/// A COM object: a pointer to its vtable pointer.
type Com = *mut c_void;

/// A Direct3D error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    /// What the layer was doing, as "making the texture".
    pub doing: &'static str,
    /// Direct3D's `HRESULT`.
    pub code: i32,
    /// Windows' message for that code, in the user's language; empty if it has none.
    pub said: String,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} failed with {:#010x}", self.doing, self.code as u32)?;
        if !self.said.is_empty() {
            write!(f, ": {}", self.said)?;
        }
        Ok(())
    }
}

impl std::error::Error for Failed {}

/// `Err` with what was being done if `code` is a failure.
fn check(code: i32, doing: &'static str) -> Result<(), Failed> {
    if code >= 0 {
        return Ok(());
    }
    let mut said = [0u16; 512];
    // SAFETY: the buffer is as long as stated; system messages take no inserts (0x1000:
    // FORMAT_MESSAGE_FROM_SYSTEM, 0x200: FORMAT_MESSAGE_IGNORE_INSERTS).
    let length = unsafe { FormatMessageW(0x1000 | 0x200, null(), code as u32, 0, said.as_mut_ptr(), said.len() as u32, null()) };
    Err(Failed { doing, code, said: String::from_utf16_lossy(&said[..length as usize]).trim_end().to_string() })
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FormatMessageW(flags: u32, source: *const c_void, id: u32, language: u32, buffer: *mut u16, size: u32, arguments: *const c_void) -> u32;
}

/// Vtable entry `at` of a COM object, as a function of type `F`.
///
/// # Safety
/// The object is alive, and its entry `at` has type `F`.
unsafe fn function<F: Copy>(object: Com, at: usize) -> F {
    // SAFETY: as the caller promises; a function pointer is one pointer-sized value.
    unsafe { std::mem::transmute_copy(&*(*(object as *const *const usize)).add(at)) }
}

/// Releases a COM object (vtable entry 2 of every one), if not null.
///
/// # Safety
/// This holds one reference to the object, and it is not used afterwards.
unsafe fn release(object: Com) {
    if !object.is_null() {
        // SAFETY: as the caller promises.
        unsafe { function::<unsafe extern "system" fn(Com) -> u32>(object, 2)(object) };
    }
}

// Direct3D constants, as d3d11.h has them.
const B8G8R8A8_UNORM: u32 = 87;
const BIND_SHADER_RESOURCE: u32 = 0x8;
const TRIANGLE_STRIP: u32 = 5;
/// Most viewports, and scissor rects, a context holds.
const MOST_VIEWPORTS: usize = 16;
/// Most class instances a shader is set with.
const MOST_INSTANCES: usize = 253;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Viewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

#[repr(C)]
struct TextureDescription {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access_flags: u32,
    misc_flags: u32,
}

#[repr(C)]
struct Pixels {
    first: *const c_void,
    row_pitch: u32,
    slice_pitch: u32,
}

#[repr(C)]
struct Box3 {
    left: u32,
    top: u32,
    front: u32,
    right: u32,
    bottom: u32,
    back: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TargetBlend {
    enable: i32,
    source: u32,
    destination: u32,
    operation: u32,
    source_alpha: u32,
    destination_alpha: u32,
    operation_alpha: u32,
    write_mask: u8,
}

#[repr(C)]
struct BlendDescription {
    alpha_to_coverage: i32,
    independent: i32,
    targets: [TargetBlend; 8],
}

#[repr(C)]
struct DepthDescription {
    depth: i32,
    depth_write_mask: u32,
    depth_function: u32,
    stencil: i32,
    stencil_read_mask: u8,
    stencil_write_mask: u8,
    /// For front and back faces: stencil fail op, depth fail op, pass op, and stencil function.
    faces: [[u32; 4]; 2],
}

#[repr(C)]
struct RasterizerDescription {
    fill: u32,
    cull: u32,
    front_counter_clockwise: i32,
    depth_bias: i32,
    depth_bias_clamp: f32,
    slope_scaled_depth_bias: f32,
    depth_clip: i32,
    scissor: i32,
    multisample: i32,
    antialiased_lines: i32,
}

type CreateShader = unsafe extern "system" fn(Com, *const u32, usize, Com, *mut Com) -> i32;
type GetShader = unsafe extern "system" fn(Com, *mut Com, *mut Com, *mut u32);
type SetShader = unsafe extern "system" fn(Com, Com, *const Com, u32);
type GetObject = unsafe extern "system" fn(Com, *mut Com);
type SetObject = unsafe extern "system" fn(Com, Com);
type Views = unsafe extern "system" fn(Com, u32, u32, *const Com);

/// The quad over the viewport, built from vertex ids (no vertex buffer), each pixel loading the
/// texel at the same place unfiltered (no sampler needed). Vertex shader `corner` and pixel
/// shader `pixel`, as Windows' compiler (`D3DCompile` of d3dcompiler_47.dll, no flags) builds
/// them for `vs_4_0` and `ps_4_0`; a test recompiles this source and compares.
/// ```hlsl
/// float4 corner(uint at : SV_VertexID, out float2 place : PLACE) : SV_Position {
///     place = float2(at & 1, at >> 1);
///     return float4(place.x * 2 - 1, 1 - place.y * 2, 0, 1);
/// }
/// Texture2D<float4> picture;
/// float4 pixel(float4 at : SV_Position, float2 place : PLACE) : SV_Target {
///     uint width, height;
///     picture.GetDimensions(width, height);
///     return picture.Load(int3(place * float2(width, height), 0));
/// }
/// ```
const CORNER: [u32; 171] = [
    0x43425844, 0xb071a0e4, 0x9a386d95, 0xa5e83f0b, 0x6595d75a, 0x00000001, 0x000002ac, 0x00000005, 0x00000034, 0x00000080, 0x000000b4, 0x00000108, 0x00000230, 0x46454452, 0x00000044, 0x00000000,
    0x00000000, 0x00000000, 0x0000001c, 0xfffe0400, 0x00000100, 0x0000001c, 0x7263694d, 0x666f736f, 0x52282074, 0x4c482029, 0x53204c53, 0x65646168, 0x6f432072, 0x6c69706d, 0x31207265, 0x00312e30,
    0x4e475349, 0x0000002c, 0x00000001, 0x00000008, 0x00000020, 0x00000000, 0x00000006, 0x00000001, 0x00000000, 0x00000101, 0x565f5653, 0x65747265, 0x00444978, 0x4e47534f, 0x0000004c, 0x00000002,
    0x00000008, 0x00000038, 0x00000000, 0x00000001, 0x00000003, 0x00000000, 0x0000000f, 0x00000044, 0x00000000, 0x00000000, 0x00000003, 0x00000001, 0x00000c03, 0x505f5653, 0x7469736f, 0x006e6f69,
    0x43414c50, 0xabab0045, 0x52444853, 0x00000120, 0x00010040, 0x00000048, 0x04000060, 0x00101012, 0x00000000, 0x00000006, 0x04000067, 0x001020f2, 0x00000000, 0x00000001, 0x03000065, 0x00102032,
    0x00000001, 0x02000068, 0x00000001, 0x07000001, 0x00100012, 0x00000000, 0x0010100a, 0x00000000, 0x00004001, 0x00000001, 0x05000056, 0x00100012, 0x00000000, 0x0010000a, 0x00000000, 0x09000032,
    0x00102012, 0x00000000, 0x0010000a, 0x00000000, 0x00004001, 0x40000000, 0x00004001, 0xbf800000, 0x07000055, 0x00100042, 0x00000000, 0x0010100a, 0x00000000, 0x00004001, 0x00000001, 0x05000056,
    0x00100022, 0x00000000, 0x0010002a, 0x00000000, 0x0a000032, 0x00102022, 0x00000000, 0x8010001a, 0x00000041, 0x00000000, 0x00004001, 0x40000000, 0x00004001, 0x3f800000, 0x05000036, 0x00102032,
    0x00000001, 0x00100046, 0x00000000, 0x08000036, 0x001020c2, 0x00000000, 0x00004002, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x0100003e, 0x54415453, 0x00000074, 0x00000009, 0x00000001,
    0x00000000, 0x00000003, 0x00000002, 0x00000000, 0x00000002, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000002, 0x00000000, 0x00000002, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
];

/// The pixel shader of the source above.
const PIXEL: [u32; 163] = [
    0x43425844, 0xfa68e194, 0x236a8d59, 0xbeb48c9c, 0xcac8426c, 0x00000001, 0x0000028c, 0x00000005, 0x00000034, 0x000000a8, 0x000000fc, 0x00000130, 0x00000210, 0x46454452, 0x0000006c, 0x00000000,
    0x00000000, 0x00000001, 0x0000001c, 0xffff0400, 0x00000100, 0x00000044, 0x0000003c, 0x00000002, 0x00000005, 0x00000004, 0xffffffff, 0x00000000, 0x00000001, 0x0000000c, 0x74636970, 0x00657275,
    0x7263694d, 0x666f736f, 0x52282074, 0x4c482029, 0x53204c53, 0x65646168, 0x6f432072, 0x6c69706d, 0x31207265, 0x00312e30, 0x4e475349, 0x0000004c, 0x00000002, 0x00000008, 0x00000038, 0x00000000,
    0x00000001, 0x00000003, 0x00000000, 0x0000000f, 0x00000044, 0x00000000, 0x00000000, 0x00000003, 0x00000001, 0x00000303, 0x505f5653, 0x7469736f, 0x006e6f69, 0x43414c50, 0xabab0045, 0x4e47534f,
    0x0000002c, 0x00000001, 0x00000008, 0x00000020, 0x00000000, 0x00000000, 0x00000003, 0x00000000, 0x0000000f, 0x545f5653, 0x65677261, 0xabab0074, 0x52444853, 0x000000d8, 0x00000040, 0x00000036,
    0x04001858, 0x00107000, 0x00000000, 0x00005555, 0x03001062, 0x00101032, 0x00000001, 0x03000065, 0x001020f2, 0x00000000, 0x02000068, 0x00000001, 0x0700103d, 0x001000f2, 0x00000000, 0x00004001,
    0x00000000, 0x00107e46, 0x00000000, 0x05000056, 0x00100032, 0x00000000, 0x00100046, 0x00000000, 0x07000038, 0x00100032, 0x00000000, 0x00100046, 0x00000000, 0x00101046, 0x00000001, 0x0500001b,
    0x00100032, 0x00000000, 0x00100046, 0x00000000, 0x08000036, 0x001000c2, 0x00000000, 0x00004002, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x0700002d, 0x001020f2, 0x00000000, 0x00100e46,
    0x00000000, 0x00107e46, 0x00000000, 0x0100003e, 0x54415453, 0x00000074, 0x00000007, 0x00000001, 0x00000000, 0x00000002, 0x00000001, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000002, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000,
];

/// The UI in a program's Direct3D 11: its pixels, a texture of their size, and what draws it
/// over the program's picture.
pub struct Layer {
    device: Com,
    context: Com,
    /// Vertex and pixel shader.
    shaders: [Com; 2],
    blend: Com,
    depth: Com,
    rasterizer: Com,
    /// The texture (`ID3D11Texture2D`) and the view the pixel shader reads it through; null
    /// until the UI has a size.
    texture: Com,
    view: Com,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).finish_non_exhaustive()
    }
}

impl Layer {
    /// The layer for the program's `device` (`ID3D11Device`) and the `context` it draws with
    /// (an `ID3D11DeviceContext` of that device, usually the immediate one). The layer holds a
    /// reference to both while it lives.
    ///
    /// # Safety
    /// `device` and `context` are alive, and of those types.
    pub unsafe fn new(device: *mut c_void, context: *mut c_void) -> Result<Layer, Failed> {
        // SAFETY: as the caller promises (vtable entry 1 of every COM object is AddRef).
        unsafe { [device, context].into_iter().for_each(|object| _ = function::<unsafe extern "system" fn(Com) -> u32>(object, 1)(object)) };
        let surface = Surface::default();
        let mut layer = Layer { device, context, shaders: [null_mut(); 2], blend: null_mut(), depth: null_mut(), rasterizer: null_mut(), texture: null_mut(), view: null_mut(), surface };
        // Premultiplied alpha: one, one minus source alpha, add (2, 6, 1); all four channels
        // written.
        let target = TargetBlend { enable: 1, source: 2, destination: 6, operation: 1, source_alpha: 2, destination_alpha: 6, operation_alpha: 1, write_mask: 0xf };
        let blend = BlendDescription { alpha_to_coverage: 0, independent: 0, targets: [target; 8] };
        // No depth and no stencil (no writes, test always passes: 0 and 8; stencil ops keep:
        // 1).
        let depth = DepthDescription { depth: 0, depth_write_mask: 0, depth_function: 8, stencil: 0, stencil_read_mask: 0xff, stencil_write_mask: 0xff, faces: [[1, 1, 1, 8]; 2] };
        // Solid fill (3), no culling (1), scissor on.
        let rasterizer = RasterizerDescription {
            fill: 3,
            cull: 1,
            front_counter_clockwise: 0,
            depth_bias: 0,
            depth_bias_clamp: 0.0,
            slope_scaled_depth_bias: 0.0,
            depth_clip: 1,
            scissor: 1,
            multisample: 0,
            antialiased_lines: 0,
        };
        type Make<T> = unsafe extern "system" fn(Com, *const T, *mut Com) -> i32;
        // SAFETY: the device is alive; each description is complete. (Device vtable: 12 and 15
        // create vertex and pixel shaders, 20 blend, 21 depth-stencil, 22 rasterizer state.)
        unsafe {
            check(function::<CreateShader>(device, 12)(device, CORNER.as_ptr(), CORNER.len() * 4, null_mut(), &mut layer.shaders[0]), "making the vertex shader")?;
            check(function::<CreateShader>(device, 15)(device, PIXEL.as_ptr(), PIXEL.len() * 4, null_mut(), &mut layer.shaders[1]), "making the pixel shader")?;
            check(function::<Make<BlendDescription>>(device, 20)(device, &blend, &mut layer.blend), "making the blending")?;
            check(function::<Make<DepthDescription>>(device, 21)(device, &depth, &mut layer.depth), "making the depth state")?;
            check(function::<Make<RasterizerDescription>>(device, 22)(device, &rasterizer, &mut layer.rasterizer), "making the rasterizer state")?;
        }
        Ok(layer)
    }

    /// Updates the texture after a run: redraws what changed and uploads only that. (The first
    /// time, and after a resize, a new texture is made and all of it uploaded.)
    pub fn update(&mut self, ui: &mut Ui) -> Result<(), Failed> {
        let size = (self.surface.width, self.surface.height);
        let changed = self.surface.update(ui);
        if (self.surface.width, self.surface.height) != size || self.view.is_null() {
            return self.make_texture();
        }
        for [left, top, right, bottom] in changed {
            let place = Box3 { left: left as u32, top: top as u32, front: 0, right: right as u32, bottom: bottom as u32, back: 1 };
            let first: *const u32 = &self.surface.pixels[top * self.surface.width + left];
            type Copy = unsafe extern "system" fn(Com, Com, u32, *const Box3, *const u32, u32, u32);
            // SAFETY: the box lies in the texture, which is the surface's size: every row read
            // lies in the surface's pixels, one surface row apart. (Vtable 48:
            // UpdateSubresource.)
            unsafe { function::<Copy>(self.context, 48)(self.context, self.texture, 0, &place, first, (self.surface.width * 4) as u32, 0) };
        }
        Ok(())
    }

    /// A texture of the surface's size, holding all of it, and its view; none while the UI has
    /// no size.
    fn make_texture(&mut self) -> Result<(), Failed> {
        // SAFETY: the old texture and view are held here, and released.
        unsafe { [std::mem::replace(&mut self.view, null_mut()), std::mem::replace(&mut self.texture, null_mut())].into_iter().for_each(|object| release(object)) };
        if self.surface.pixels.is_empty() {
            return Ok(());
        }
        let (width, height) = (self.surface.width as u32, self.surface.height as u32);
        let described = TextureDescription {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format: B8G8R8A8_UNORM,
            sample_count: 1,
            sample_quality: 0,
            usage: 0,
            bind_flags: BIND_SHADER_RESOURCE,
            cpu_access_flags: 0,
            misc_flags: 0,
        };
        let pixels = Pixels { first: self.surface.pixels.as_ptr().cast(), row_pitch: width * 4, slice_pitch: 0 };
        type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const Pixels, *mut Com) -> i32;
        type MakeView = unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32;
        // SAFETY: the device is alive; the initial data has as many pixels as the texture
        // (vtable 5 and 7: CreateTexture2D, CreateShaderResourceView).
        unsafe {
            check(function::<MakeTexture>(self.device, 5)(self.device, &described, &pixels, &mut self.texture), "making the texture")?;
            check(function::<MakeView>(self.device, 7)(self.device, self.texture, null(), &mut self.view), "making the texture's view")
        }
    }

    /// Updates the texture (`update`) and draws it over the bound render target: at the
    /// viewport's top left, pixel for pixel, clipped to the viewport. Everything it sets on the
    /// context is restored.
    pub fn draw(&mut self, ui: &mut Ui) -> Result<(), Failed> {
        self.update(ui)?;
        if self.view.is_null() {
            return Ok(());
        }
        let context = self.context;
        let kept = Kept::take(context);
        let [first, ..] = kept.viewports.1;
        let (x, y, width, height) = match kept.viewports.0 {
            0 => (0.0, 0.0, self.surface.width as f32, self.surface.height as f32),
            _ => (first.x, first.y, first.width, first.height),
        };
        let viewport = Viewport { x, y, width: self.surface.width as f32, height: self.surface.height as f32, min_depth: 0.0, max_depth: 1.0 };
        let scissor = [x as i32, y as i32, (x + width) as i32, (y + height) as i32];
        // SAFETY: the context is alive, and each vtable index is that method: 17
        // IASetInputLayout, 24 IASetPrimitiveTopology, 11/60/64/23/9 the shader of each stage,
        // 8 PSSetShaderResources, 35 OMSetBlendState, 36 OMSetDepthStencilState, 43 RSSetState,
        // 44 RSSetViewports, 45 RSSetScissorRects, 13 Draw.
        unsafe {
            function::<SetObject>(context, 17)(context, null_mut());
            function::<unsafe extern "system" fn(Com, u32)>(context, 24)(context, TRIANGLE_STRIP);
            function::<SetShader>(context, 11)(context, self.shaders[0], null(), 0);
            [60, 64, 23].into_iter().for_each(|at| function::<SetShader>(context, at)(context, null_mut(), null(), 0));
            function::<SetShader>(context, 9)(context, self.shaders[1], null(), 0);
            function::<Views>(context, 8)(context, 0, 1, &self.view);
            function::<unsafe extern "system" fn(Com, Com, *const f32, u32)>(context, 35)(context, self.blend, null(), u32::MAX);
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 36)(context, self.depth, 0);
            function::<SetObject>(context, 43)(context, self.rasterizer);
            function::<unsafe extern "system" fn(Com, u32, *const Viewport)>(context, 44)(context, 1, &viewport);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(context, 45)(context, 1, &scissor);
            function::<unsafe extern "system" fn(Com, u32, u32)>(context, 13)(context, 4, 0);
        }
        kept.put_back(context);
        Ok(())
    }

    /// The texture (`ID3D11Texture2D`, `DXGI_FORMAT_B8G8R8A8_UNORM`, premultiplied alpha), as
    /// `update` left it; null while the UI has no size (or if the last could not be made: the
    /// next `update` retries). Owned by the layer: a program keeping it calls `AddRef`.
    pub fn texture(&self) -> *mut c_void {
        self.texture
    }

    /// The UI's pixels, which the texture holds.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        let held = [self.view, self.texture, self.shaders[0], self.shaders[1], self.blend, self.depth, self.rasterizer, self.context, self.device];
        // SAFETY: each is held once, by this layer.
        unsafe { held.into_iter().for_each(|object| release(object)) };
    }
}

/// The context state `draw` changes, as the program had it, to restore after.
struct Kept {
    /// Each stage's shader (vertex, hull, domain, geometry, pixel), with its class instances.
    stages: [(Com, Vec<Com>); 5],
    /// The pixel shader's first resource, the input layout, and the blend, depth-stencil and
    /// rasterizer states.
    objects: [Com; 5],
    topology: u32,
    blend_factor: [f32; 4],
    sample_mask: u32,
    stencil_reference: u32,
    viewports: (u32, [Viewport; MOST_VIEWPORTS]),
    scissors: (u32, [[i32; 4]; MOST_VIEWPORTS]),
}

/// Each stage's get and set shader vtable indices: vertex, hull, domain, geometry, pixel.
const STAGES: [(usize, usize); 5] = [(76, 11), (98, 60), (102, 64), (82, 23), (74, 9)];

impl Kept {
    /// The context's current state. (Every object it returns holds a reference until restored.)
    fn take(context: Com) -> Kept {
        let stages = STAGES.map(|(get, _)| shader(context, get));
        let mut kept = Kept {
            stages,
            objects: [null_mut(); 5],
            topology: 0,
            blend_factor: [0.0; 4],
            sample_mask: 0,
            stencil_reference: 0,
            viewports: (16, [Viewport::default(); 16]),
            scissors: (16, [[0; 4]; 16]),
        };
        let [picture, layout, blend, depth, rasterizer] = &mut kept.objects;
        // SAFETY: the context is alive, and each vtable index is that method: 73
        // PSGetShaderResources, 78 IAGetInputLayout, 83 IAGetPrimitiveTopology, 91
        // OMGetBlendState, 92 OMGetDepthStencilState, 94 RSGetState, 95 RSGetViewports, 96
        // RSGetScissorRects.
        unsafe {
            function::<unsafe extern "system" fn(Com, u32, u32, *mut Com)>(context, 73)(context, 0, 1, picture);
            function::<GetObject>(context, 78)(context, layout);
            function::<unsafe extern "system" fn(Com, *mut u32)>(context, 83)(context, &mut kept.topology);
            function::<unsafe extern "system" fn(Com, *mut Com, *mut f32, *mut u32)>(context, 91)(context, blend, kept.blend_factor.as_mut_ptr(), &mut kept.sample_mask);
            function::<unsafe extern "system" fn(Com, *mut Com, *mut u32)>(context, 92)(context, depth, &mut kept.stencil_reference);
            function::<GetObject>(context, 94)(context, rasterizer);
            function::<unsafe extern "system" fn(Com, *mut u32, *mut Viewport)>(context, 95)(context, &mut kept.viewports.0, kept.viewports.1.as_mut_ptr());
            function::<unsafe extern "system" fn(Com, *mut u32, *mut [i32; 4])>(context, 96)(context, &mut kept.scissors.0, kept.scissors.1.as_mut_ptr());
        }
        kept
    }

    /// Restores the state on the context, and releases it.
    fn put_back(self, context: Com) {
        let [picture, layout, blend, depth, rasterizer] = self.objects;
        // SAFETY: the context is alive; each vtable index is that method, as above.
        unsafe {
            for ((shader, instances), (_, set)) in self.stages.iter().zip(STAGES) {
                function::<SetShader>(context, set)(context, *shader, instances.as_ptr(), instances.len() as u32);
            }
            function::<Views>(context, 8)(context, 0, 1, &picture);
            function::<SetObject>(context, 17)(context, layout);
            function::<unsafe extern "system" fn(Com, u32)>(context, 24)(context, self.topology);
            function::<unsafe extern "system" fn(Com, Com, *const f32, u32)>(context, 35)(context, blend, self.blend_factor.as_ptr(), self.sample_mask);
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 36)(context, depth, self.stencil_reference);
            function::<SetObject>(context, 43)(context, rasterizer);
            function::<unsafe extern "system" fn(Com, u32, *const Viewport)>(context, 44)(context, self.viewports.0, self.viewports.1.as_ptr());
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(context, 45)(context, self.scissors.0, self.scissors.1.as_ptr());
            let instances = self.stages.iter().flat_map(|(shader, instances)| instances.iter().chain([shader]));
            instances.chain(&self.objects).for_each(|object| release(*object));
        }
    }
}

/// One stage's shader (by its get method, `get`), and its class instances.
fn shader(context: Com, get: usize) -> (Com, Vec<Com>) {
    let (mut shader, mut instances, mut count) = (null_mut(), [null_mut(); MOST_INSTANCES], MOST_INSTANCES as u32);
    // SAFETY: the context is alive, and there is room for as many instances as stated.
    unsafe { function::<GetShader>(context, get)(context, &mut shader, instances.as_mut_ptr(), &mut count) };
    (shader, instances[..count.min(MOST_INSTANCES as u32) as usize].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Compile = unsafe extern "system" fn(*const u8, usize, *const u8, *const c_void, *const c_void, *const u8, *const u8, u32, u32, *mut Com, *mut Com) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(library: isize, name: *const u8) -> usize;
    }

    /// What Windows' compiler makes of `source` for `entry` and `target`: the bytecode, or its
    /// errors.
    fn compiled(source: &str, entry: &str, target: &str) -> Result<Vec<u8>, String> {
        let library: Vec<u16> = "d3dcompiler_47.dll\0".encode_utf16().collect();
        let (entry, target, mut code, mut errors) = (format!("{entry}\0"), format!("{target}\0"), null_mut(), null_mut());
        let bytes = |blob: Com| unsafe {
            let start = function::<unsafe extern "system" fn(Com) -> *const u8>(blob, 3)(blob);
            std::slice::from_raw_parts(start, function::<unsafe extern "system" fn(Com) -> usize>(blob, 4)(blob)).to_vec()
        };
        unsafe {
            let found = GetProcAddress(LoadLibraryW(library.as_ptr()), c"D3DCompile".as_ptr().cast());
            assert_ne!(found, 0, "Windows has its compiler");
            let compile: Compile = std::mem::transmute(found);
            let result = compile(source.as_ptr(), source.len(), null(), null(), null(), entry.as_ptr(), target.as_ptr(), 0, 0, &mut code, &mut errors);
            let made = match result >= 0 {
                true => Ok(bytes(code)),
                false => Err(String::from_utf8_lossy(&bytes(errors)).into_owned()),
            };
            [code, errors].into_iter().for_each(|blob| release(blob));
            made
        }
    }

    /// The shader bytecode is what Windows' compiler makes of the source above it.
    #[test]
    fn the_shaders_are_their_source_compiled() {
        let file = include_str!("layer_d3d11.rs").replace("\r\n", "\n");
        let start = file.find("/// ```hlsl\n").unwrap() + "/// ```hlsl\n".len();
        let end = start + file[start..].find("/// ```\n").unwrap();
        let source: String = file[start..end].lines().map(|line| format!("{}\n", line.strip_prefix("/// ").unwrap_or(""))).collect();
        let words = |code: Vec<u8>| code.chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect::<Vec<u32>>();
        assert_eq!(words(compiled(&source, "corner", "vs_4_0").unwrap()), CORNER, "the vertex shader");
        assert_eq!(words(compiled(&source, "pixel", "ps_4_0").unwrap()), PIXEL, "the pixel shader");
    }

    /// Direct3D errors come with Windows' own message, where it has one.
    #[test]
    fn what_went_wrong_is_said_in_windows_own_words() {
        assert_eq!(check(0, "anything"), Ok(()));
        for code in [0x8007_000E_u32, 0x8007_0057, 0x887A_0005] {
            let failed = check(code as i32, "making the texture").unwrap_err();
            println!("{failed}");
            assert!(!failed.said.is_empty(), "{code:#x} is said");
            assert!(failed.to_string().starts_with(&format!("making the texture failed with {code:#010x}: ")));
        }
    }
}
