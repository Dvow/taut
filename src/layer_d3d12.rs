//! The UI over a program's own Direct3D 12 rendering: made once from its device and the queue it
//! draws with, then drawn each frame with one call, in the program's command list.
//!
//! ```ignore
//! // SAFETY: the program's own device and command queue.
//! let mut layer = unsafe { taut::layer_d3d12::Layer::new(device, queue, format) }?;
//! // each frame, in the program's command list, with its render target bound:
//! unsafe { layer.draw(&mut ui, list, [0, 0, width, height]) }?;
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`), a texture of the same size, and an upload
//! buffer laid out like the surface. `update` redraws only what changed, writes only that to the
//! buffer and copies only that to the texture, in a command list of its own run on the program's
//! queue: it comes before the frame that draws it. `draw` updates, then records one quad over the
//! given area of the bound render target: the UI at its top left, pixel for pixel, clipped to it,
//! with premultiplied alpha blending ("one, one minus source alpha"). Direct3D 12 keeps no state
//! to restore: after `draw` the program sets its own root signature, pipeline state, descriptor
//! heaps, viewport and scissor again. A program that shows the UI otherwise calls `update` and
//! reads `texture()`.
//!
//! Nothing is loaded or compiled at run time: COM methods are called by their place in the
//! vtable, as d3d12.h orders them, and the shaders and root signature are precompiled (shader
//! model 5.0). The render target must store colour linearly (`..._UNORM`, not `..._UNORM_SRGB`):
//! the UI's pixels are already in screen colours.
// Every function here is `#[inline]`: only a program that calls one compiles it. (Compiled in
// the library itself, even unused, this code moved how the rest is grouped and optimised.)
pub use crate::layer_d3d11::Error;
use crate::layer_d3d11::{B8G8R8A8_UNORM, Com, check, function, release};
use crate::surface::Surface;
use crate::ui::Ui;
use crate::windows::named;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

/// The interfaces made: `ID3D12CommandAllocator`, `ID3D12GraphicsCommandList`, `ID3D12Fence`,
/// `ID3D12RootSignature`, `ID3D12PipelineState`, `ID3D12DescriptorHeap`, `ID3D12Resource`.
const ALLOCATOR: [u32; 4] = named(0x6102_DEE4, 0xAF59, 0x4B09, [0xB9, 0x99, 0xB4, 0x4D, 0x73, 0xF0, 0x9B, 0x24]);
const LIST: [u32; 4] = named(0x5B16_0D0F, 0xAC1B, 0x4185, [0x8B, 0xA8, 0xB3, 0xAE, 0x42, 0xA5, 0xA4, 0x55]);
const FENCE: [u32; 4] = named(0x0A75_3DCF, 0xC4D8, 0x4B91, [0xAD, 0xF6, 0xBE, 0x5A, 0x60, 0xD9, 0x5A, 0x76]);
const ROOT: [u32; 4] = named(0xC54A_6B66, 0x72DF, 0x4EE8, [0x8B, 0xE5, 0xA9, 0x46, 0xA1, 0x42, 0x92, 0x14]);
const PIPELINE: [u32; 4] = named(0x765A_30F3, 0xF624, 0x4C6F, [0xA8, 0x28, 0xAC, 0xE9, 0x48, 0x62, 0x24, 0x45]);
const HEAP: [u32; 4] = named(0x8EFB_471D, 0x616C, 0x4F49, [0x90, 0xF7, 0x12, 0x7B, 0xB7, 0x63, 0xFA, 0x51]);
const RESOURCE: [u32; 4] = named(0x6964_42BE, 0xA72E, 0x4059, [0xBC, 0x79, 0x5B, 0x5C, 0x98, 0x04, 0x0F, 0xAD]);

// Resource states, as d3d12.h has them.
const COPY_DEST: u32 = 0x400;
const PIXEL_SHADER_RESOURCE: u32 = 0x80;
const GENERIC_READ: u32 = 0xAC3;
/// Rows of a texture copied from a buffer are this many bytes apart, or a multiple of it.
const PITCH: usize = 256;

#[repr(C)]
struct HeapProperties {
    kind: u32,
    page: u32,
    pool: u32,
    creation_mask: u32,
    visible_mask: u32,
}

#[repr(C)]
struct ResourceDescription {
    dimension: u32,
    alignment: u64,
    width: u64,
    height: u32,
    depth: u16,
    levels: u16,
    format: u32,
    samples: [u32; 2],
    layout: u32,
    flags: u32,
}

/// `D3D12_RESOURCE_BARRIER` of the transition kind (0) for all subresources.
#[repr(C)]
struct Barrier {
    kind: u32,
    flags: u32,
    resource: Com,
    subresource: u32,
    before: u32,
    after: u32,
}

/// `D3D12_TEXTURE_COPY_LOCATION`: a subresource (`kind` 0, `offset` its index), or a buffer
/// laid out as `footprint` says (1: format, width, height, depth, row pitch) from `offset`.
#[repr(C)]
struct CopyPlace {
    resource: Com,
    kind: u32,
    offset: u64,
    footprint: [u32; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Bytecode {
    code: *const u32,
    size: usize,
}

#[repr(C)]
struct StreamOutput {
    declarations: *const c_void,
    count: u32,
    strides: *const u32,
    stride_count: u32,
    rasterized: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TargetBlend {
    enable: i32,
    logic_enable: i32,
    source: u32,
    destination: u32,
    operation: u32,
    source_alpha: u32,
    destination_alpha: u32,
    operation_alpha: u32,
    logic: u32,
    write_mask: u8,
}

/// `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, as d3d12.h lays it out.
#[repr(C)]
struct PipelineDescription {
    root: Com,
    /// Vertex, pixel, domain, hull and geometry shaders.
    shaders: [Bytecode; 5],
    /// Stream output: declarations and their count, strides and their count, the stream drawn.
    stream_output: StreamOutput,
    alpha_to_coverage: i32,
    independent_blend: i32,
    targets: [TargetBlend; 8],
    sample_mask: u32,
    /// Fill, cull, front counter-clockwise, depth bias, its clamp and slope (as bits), depth
    /// clip, multisample, antialiased lines, forced sample count, conservative rasterization.
    rasterizer: [u32; 11],
    /// Depth on, its write mask and test; stencil on, its read and write masks (one byte each,
    /// in one word), and the stencil fail, depth fail and pass operations and the test of front
    /// and back faces.
    depth: [u32; 13],
    /// The input layout's elements and their count.
    input_layout: [usize; 2],
    strip_cut: u32,
    topology: u32,
    target_count: u32,
    formats: [u32; 8],
    depth_format: u32,
    samples: [u32; 2],
    node_mask: u32,
    /// A cached pipeline state and its size.
    cached: [usize; 2],
    flags: u32,
}

/// The quad over the viewport, built from vertex ids (no vertex buffer), each pixel loading the
/// texel at the same place unfiltered (no sampler needed); its root signature names the texture
/// alone. Vertex shader `corner`, pixel shader `pixel` and root signature `TAUT`, as Windows'
/// compiler (`D3DCompile` of d3dcompiler_47.dll, no flags) builds them for `vs_5_0`, `ps_5_0`
/// and `rootsig_1_0`; a test recompiles this source and compares.
/// ```hlsl
/// #define TAUT "DescriptorTable(SRV(t0), visibility = SHADER_VISIBILITY_PIXEL)"
/// float4 corner(uint at : SV_VertexID, out float2 place : PLACE) : SV_Position {
///     place = float2(at & 1, at >> 1);
///     return float4(place.x * 2 - 1, 1 - place.y * 2, 0, 1);
/// }
/// Texture2D<float4> picture : register(t0);
/// float4 pixel(float4 at : SV_Position, float2 place : PLACE) : SV_Target {
///     uint width, height;
///     picture.GetDimensions(width, height);
///     return picture.Load(int3(place * float2(width, height), 0));
/// }
/// ```
const CORNER: [u32; 188] = [
    0x43425844, 0x724a9f6a, 0x9a27a619, 0xb8c19895, 0x9d266890, 0x00000001, 0x000002f0, 0x00000005, 0x00000034, 0x000000a0, 0x000000d4, 0x00000128, 0x00000254, 0x46454452, 0x00000064, 0x00000000,
    0x00000000, 0x00000000, 0x0000003c, 0xfffe0500, 0x00000100, 0x0000003c, 0x31314452, 0x0000003c, 0x00000018, 0x00000020, 0x00000028, 0x00000024, 0x0000000c, 0x00000000, 0x7263694d, 0x666f736f,
    0x52282074, 0x4c482029, 0x53204c53, 0x65646168, 0x6f432072, 0x6c69706d, 0x31207265, 0x00312e30, 0x4e475349, 0x0000002c, 0x00000001, 0x00000008, 0x00000020, 0x00000000, 0x00000006, 0x00000001,
    0x00000000, 0x00000101, 0x565f5653, 0x65747265, 0x00444978, 0x4e47534f, 0x0000004c, 0x00000002, 0x00000008, 0x00000038, 0x00000000, 0x00000001, 0x00000003, 0x00000000, 0x0000000f, 0x00000044,
    0x00000000, 0x00000000, 0x00000003, 0x00000001, 0x00000c03, 0x505f5653, 0x7469736f, 0x006e6f69, 0x43414c50, 0xabab0045, 0x58454853, 0x00000124, 0x00010050, 0x00000049, 0x0100086a, 0x04000060,
    0x00101012, 0x00000000, 0x00000006, 0x04000067, 0x001020f2, 0x00000000, 0x00000001, 0x03000065, 0x00102032, 0x00000001, 0x02000068, 0x00000001, 0x07000001, 0x00100012, 0x00000000, 0x0010100a,
    0x00000000, 0x00004001, 0x00000001, 0x05000056, 0x00100012, 0x00000000, 0x0010000a, 0x00000000, 0x09000032, 0x00102012, 0x00000000, 0x0010000a, 0x00000000, 0x00004001, 0x40000000, 0x00004001,
    0xbf800000, 0x07000055, 0x00100042, 0x00000000, 0x0010100a, 0x00000000, 0x00004001, 0x00000001, 0x05000056, 0x00100022, 0x00000000, 0x0010002a, 0x00000000, 0x0a000032, 0x00102022, 0x00000000,
    0x8010001a, 0x00000041, 0x00000000, 0x00004001, 0x40000000, 0x00004001, 0x3f800000, 0x05000036, 0x00102032, 0x00000001, 0x00100046, 0x00000000, 0x08000036, 0x001020c2, 0x00000000, 0x00004002,
    0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x0100003e, 0x54415453, 0x00000094, 0x00000009, 0x00000001, 0x00000000, 0x00000003, 0x00000002, 0x00000000, 0x00000002, 0x00000001, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000002, 0x00000000, 0x00000002, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
];

/// The pixel shader of the source above.
const PIXEL: [u32; 184] = [
    0x43425844, 0xac159bdd, 0x01dcc9ed, 0xd3a0945a, 0x85a3a132, 0x00000001, 0x000002e0, 0x00000005, 0x00000034, 0x000000c8, 0x0000011c, 0x00000150, 0x00000244, 0x46454452, 0x0000008c, 0x00000000,
    0x00000000, 0x00000001, 0x0000003c, 0xffff0500, 0x00000100, 0x00000064, 0x31314452, 0x0000003c, 0x00000018, 0x00000020, 0x00000028, 0x00000024, 0x0000000c, 0x00000000, 0x0000005c, 0x00000002,
    0x00000005, 0x00000004, 0xffffffff, 0x00000000, 0x00000001, 0x0000000d, 0x74636970, 0x00657275, 0x7263694d, 0x666f736f, 0x52282074, 0x4c482029, 0x53204c53, 0x65646168, 0x6f432072, 0x6c69706d,
    0x31207265, 0x00312e30, 0x4e475349, 0x0000004c, 0x00000002, 0x00000008, 0x00000038, 0x00000000, 0x00000001, 0x00000003, 0x00000000, 0x0000000f, 0x00000044, 0x00000000, 0x00000000, 0x00000003,
    0x00000001, 0x00000303, 0x505f5653, 0x7469736f, 0x006e6f69, 0x43414c50, 0xabab0045, 0x4e47534f, 0x0000002c, 0x00000001, 0x00000008, 0x00000020, 0x00000000, 0x00000000, 0x00000003, 0x00000000,
    0x0000000f, 0x545f5653, 0x65677261, 0xabab0074, 0x58454853, 0x000000ec, 0x00000050, 0x0000003b, 0x0100086a, 0x04001858, 0x00107000, 0x00000000, 0x00005555, 0x03001062, 0x00101032, 0x00000001,
    0x03000065, 0x001020f2, 0x00000000, 0x02000068, 0x00000001, 0x8900103d, 0x800000c2, 0x00155543, 0x00100032, 0x00000000, 0x00004001, 0x00000000, 0x00107e46, 0x00000000, 0x05000056, 0x00100032,
    0x00000000, 0x00100046, 0x00000000, 0x07000038, 0x00100032, 0x00000000, 0x00100046, 0x00000000, 0x00101046, 0x00000001, 0x0500001b, 0x00100032, 0x00000000, 0x00100046, 0x00000000, 0x08000036,
    0x001000c2, 0x00000000, 0x00004002, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x8900002d, 0x800000c2, 0x00155543, 0x001020f2, 0x00000000, 0x00100e46, 0x00000000, 0x00107e46, 0x00000000,
    0x0100003e, 0x54415453, 0x00000094, 0x00000007, 0x00000001, 0x00000000, 0x00000002, 0x00000001, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0x00000000, 0x00000002, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
];

/// The root signature of the source above.
const SIGNATURE: [u32; 27] = [
    0x43425844, 0x0eef3b37, 0x67bb9168, 0x73dbf8e0, 0xc9aa0db9, 0x00000001, 0x0000006c, 0x00000001, 0x00000024, 0x30535452, 0x00000040, 0x00000001, 0x00000001, 0x00000018, 0x00000000, 0x00000040,
    0x00000000, 0x00000000, 0x00000005, 0x00000024, 0x00000001, 0x0000002c, 0x00000000, 0x00000001, 0x00000000, 0x00000000, 0xffffffff,
];

/// The UI in a program's Direct3D 12: its pixels, a texture of their size, a buffer they are
/// uploaded through, and what copies and draws them.
pub struct Layer {
    device: Com,
    queue: Com,
    /// The layer's own commands, and the fence its submits signal with `fenced`.
    allocator: Com,
    list: Com,
    fence: Com,
    fenced: u64,
    root: Com,
    pipeline: Com,
    /// A heap of one descriptor, shaders see: the texture's.
    heap: Com,
    /// The texture (`ID3D12Resource`); null until the UI has a size.
    texture: Com,
    /// The upload buffer, where the processor writes it, and its rows' pitch in bytes.
    upload: Com,
    written: *mut u8,
    pitch: usize,
    /// The texture has been copied to, and is ready for pixel shaders.
    filled: bool,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).finish_non_exhaustive()
    }
}

impl Layer {
    /// The layer for the program's `device` (`ID3D12Device`), drawn with `queue` (an
    /// `ID3D12CommandQueue` of the direct kind), into render targets of `format` (a
    /// `DXGI_FORMAT`, as 87 for `B8G8R8A8_UNORM` or 28 for `R8G8B8A8_UNORM`; one sample). The
    /// layer holds a reference to both while it lives.
    ///
    /// # Safety
    /// `device` and `queue` are alive, and of those types.
    #[inline]
    pub unsafe fn new(device: *mut c_void, queue: *mut c_void, format: u32) -> Result<Layer, Error> {
        // SAFETY: as the caller promises (vtable entry 1 of every COM object is AddRef).
        unsafe { [device, queue].into_iter().for_each(|object| _ = function::<unsafe extern "system" fn(Com) -> u32>(object, 1)(object)) };
        let mut layer = Layer {
            device,
            queue,
            allocator: null_mut(),
            list: null_mut(),
            fence: null_mut(),
            fenced: 0,
            root: null_mut(),
            pipeline: null_mut(),
            heap: null_mut(),
            texture: null_mut(),
            upload: null_mut(),
            written: null_mut(),
            pitch: 0,
            filled: false,
            surface: Surface::default(),
        };
        type Make = unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32;
        type MakeList = unsafe extern "system" fn(Com, u32, u32, Com, Com, *const [u32; 4], *mut Com) -> i32;
        type MakeFence = unsafe extern "system" fn(Com, u64, u32, *const [u32; 4], *mut Com) -> i32;
        type MakeRoot = unsafe extern "system" fn(Com, u32, *const u32, usize, *const [u32; 4], *mut Com) -> i32;
        // The heap: one shader resource view (0), that shaders see (1).
        let heap = [0u32, 1, 1, 0];
        // SAFETY: the device is alive; each description is whole. (Device vtable: 9 makes a
        // command allocator, 12 a command list, 36 a fence, 16 a root signature, 14 a descriptor
        // heap; the list's 9 closes it.)
        unsafe {
            check(function::<Make>(device, 9)(device, 0, &ALLOCATOR, &mut layer.allocator), "making the command allocator")?;
            check(function::<MakeList>(device, 12)(device, 0, 0, layer.allocator, null_mut(), &LIST, &mut layer.list), "making the command list")?;
            check(function::<unsafe extern "system" fn(Com) -> i32>(layer.list, 9)(layer.list), "making the command list")?;
            check(function::<MakeFence>(device, 36)(device, 0, 0, &FENCE, &mut layer.fence), "making the fence")?;
            check(function::<MakeRoot>(device, 16)(device, 0, SIGNATURE.as_ptr(), SIGNATURE.len() * 4, &ROOT, &mut layer.root), "making the root signature")?;
            check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 14)(device, &heap, &HEAP, &mut layer.heap), "making the descriptor heap")?;
        }
        layer.pipeline = pipeline(device, layer.root, format)?;
        Ok(layer)
    }

    /// Updates the texture after a run: redraws what changed, and copies only that. (The first
    /// time, and after a resize, a new texture is made and all of it copied.)
    #[inline]
    pub fn update(&mut self, ui: &mut Ui) -> Result<(), Error> {
        // SAFETY: the device is alive (vtable 37: GetDeviceRemovedReason).
        check(unsafe { function::<unsafe extern "system" fn(Com) -> i32>(self.device, 37)(self.device) }, "drawing")?;
        let size = (self.surface.width, self.surface.height);
        let mut changed = self.surface.update(ui);
        if (self.surface.width, self.surface.height) != size || self.texture.is_null() {
            self.make_texture().inspect_err(|_| self.surface = Surface::default())?;
            changed = [[0, 0, self.surface.width, self.surface.height]].into_iter().filter(|_| !self.texture.is_null()).collect();
        }
        if changed.is_empty() {
            return Ok(());
        }
        self.settle();
        let (width, pitch) = (self.surface.width, self.pitch);
        for &[left, top, right, bottom] in &changed {
            for row in top..bottom {
                let from = &self.surface.pixels[row * width + left..row * width + right];
                // SAFETY: the buffer holds as many rows as the surface, `pitch` bytes apart, each
                // as long as a surface row or longer.
                unsafe { std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), self.written.add(row * pitch + left * 4), from.len() * 4) };
            }
        }
        self.copy(&changed)
    }

    /// Records the copies of `places` from the buffer to the texture, and runs them on the
    /// queue.
    #[inline]
    fn copy(&mut self, places: &[[usize; 4]]) -> Result<(), Error> {
        let (list, footprint) = (self.list, [B8G8R8A8_UNORM, self.surface.width as u32, self.surface.height as u32, 1, self.pitch as u32]);
        let to = CopyPlace { resource: self.texture, kind: 0, offset: 0, footprint: [0; 5] };
        let from = CopyPlace { resource: self.upload, kind: 1, offset: 0, footprint };
        let barrier = |before, after| Barrier { kind: 0, flags: 0, resource: self.texture, subresource: u32::MAX, before, after };
        type CopyRegion = unsafe extern "system" fn(Com, *const CopyPlace, u32, u32, u32, *const CopyPlace, *const [u32; 6]);
        type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
        // SAFETY: the layer's own commands, done with (`settle`); the places lie in the buffer
        // and the texture. (Allocator 8 and list 10 reset; list 26 places barriers, 16 copies,
        // 9 closes; queue 10 runs it and 14 signals.)
        unsafe {
            check(function::<unsafe extern "system" fn(Com) -> i32>(self.allocator, 8)(self.allocator), "recording the copy")?;
            check(function::<unsafe extern "system" fn(Com, Com, Com) -> i32>(list, 10)(list, self.allocator, null_mut()), "recording the copy")?;
            if self.filled {
                function::<Barriers>(list, 26)(list, 1, &barrier(PIXEL_SHADER_RESOURCE, COPY_DEST));
            }
            for &[left, top, right, bottom] in places {
                let area = [left as u32, top as u32, 0, right as u32, bottom as u32, 1];
                function::<CopyRegion>(list, 16)(list, &to, left as u32, top as u32, 0, &from, &area);
            }
            function::<Barriers>(list, 26)(list, 1, &barrier(COPY_DEST, PIXEL_SHADER_RESOURCE));
            check(function::<unsafe extern "system" fn(Com) -> i32>(list, 9)(list), "recording the copy")?;
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(self.queue, 10)(self.queue, 1, &list);
        }
        self.filled = true;
        self.signal("running the copy")
    }

    /// Has the queue signal the layer's fence once everything given to it so far is done.
    #[inline]
    fn signal(&mut self, doing: &'static str) -> Result<(), Error> {
        self.fenced += 1;
        // SAFETY: the queue and the fence are alive (queue vtable 14: Signal).
        check(unsafe { function::<unsafe extern "system" fn(Com, Com, u64) -> i32>(self.queue, 14)(self.queue, self.fence, self.fenced) }, doing)
    }

    /// Waits until the fence has reached the last value signalled: the layer's last copy, and
    /// all before it, are done. (A device that was removed reaches every value.)
    #[inline]
    fn settle(&self) {
        // SAFETY: the fence is alive (vtable 8: GetCompletedValue; 9: SetEventOnCompletion,
        // which with no event waits).
        unsafe {
            if function::<unsafe extern "system" fn(Com) -> u64>(self.fence, 8)(self.fence) < self.fenced {
                function::<unsafe extern "system" fn(Com, u64, isize) -> i32>(self.fence, 9)(self.fence, self.fenced, 0);
            }
        }
    }

    /// A texture of the surface's size, an upload buffer for it, and the view shaders see it
    /// through; none while the UI has no size. Everything run on the queue before is waited for
    /// first: the frames that drew the old texture are done with it.
    #[inline]
    fn make_texture(&mut self) -> Result<(), Error> {
        if !self.texture.is_null() {
            self.signal("waiting for the frames before")?;
        }
        self.settle();
        // SAFETY: the old texture and buffer are held here, and done with.
        unsafe { [std::mem::replace(&mut self.texture, null_mut()), std::mem::replace(&mut self.upload, null_mut())].into_iter().for_each(|object| release(object)) };
        (self.written, self.filled) = (null_mut(), false);
        if self.surface.pixels.is_empty() {
            return Ok(());
        }
        let (width, height) = (self.surface.width, self.surface.height);
        self.pitch = (width * 4).div_ceil(PITCH) * PITCH;
        // The texture in the device's memory (heap 1), copied to; the buffer in memory the
        // processor writes (heap 2), copied from (dimensions 3 and 1, a buffer's rows in order).
        let texture = ResourceDescription { dimension: 3, alignment: 0, width: width as u64, height: height as u32, depth: 1, levels: 1, format: B8G8R8A8_UNORM, samples: [1, 0], layout: 0, flags: 0 };
        let buffer = ResourceDescription { dimension: 1, alignment: 0, width: (self.pitch * height) as u64, height: 1, depth: 1, levels: 1, format: 0, samples: [1, 0], layout: 1, flags: 0 };
        let heap = |kind| HeapProperties { kind, page: 0, pool: 0, creation_mask: 0, visible_mask: 0 };
        type Make = unsafe extern "system" fn(Com, *const HeapProperties, u32, *const ResourceDescription, u32, *const c_void, *const [u32; 4], *mut Com) -> i32;
        let (device, mut there, mut start) = (self.device, null_mut(), 0usize);
        // SAFETY: the device is alive; each description is whole. (Device 27 makes a committed
        // resource, 18 a shader resource view; resource 8 maps it, reading nothing; heap 9 is
        // where its descriptors start.)
        unsafe {
            check(function::<Make>(device, 27)(device, &heap(1), 0, &texture, COPY_DEST, null(), &RESOURCE, &mut self.texture), "making the texture")?;
            check(function::<Make>(device, 27)(device, &heap(2), 0, &buffer, GENERIC_READ, null(), &RESOURCE, &mut self.upload), "making the upload buffer")?;
            check(function::<unsafe extern "system" fn(Com, u32, *const [usize; 2], *mut *mut c_void) -> i32>(self.upload, 8)(self.upload, 0, &[0, 0], &mut there), "mapping the upload buffer")?;
            function::<unsafe extern "system" fn(Com, *mut usize) -> *mut usize>(self.heap, 9)(self.heap, &mut start);
            function::<unsafe extern "system" fn(Com, Com, *const c_void, usize)>(device, 18)(device, self.texture, null(), start);
        }
        self.written = there.cast();
        Ok(())
    }

    /// Updates the texture (`update`) and records it drawn into `list`, over the bound render
    /// target: in `area` (left, top, width, height, in pixels from its top left), the UI at
    /// the area's top left, pixel for pixel, clipped to the area. The program sets its own root
    /// signature, pipeline state, descriptor heaps, viewport and scissor again after.
    ///
    /// # Safety
    /// `list` is the program's `ID3D12GraphicsCommandList`, recording, with a render target of
    /// the layer's format bound; it runs later on the layer's queue.
    #[inline]
    pub unsafe fn draw(&mut self, ui: &mut Ui, list: *mut c_void, area: [i32; 4]) -> Result<(), Error> {
        self.update(ui)?;
        if !self.filled {
            return Ok(());
        }
        let [x, y, width, height] = area;
        let viewport = [x as f32, y as f32, self.surface.width as f32, self.surface.height as f32, 0.0, 1.0];
        let scissor = [x.max(0), y.max(0), x + width, y + height];
        let mut start = 0u64;
        // SAFETY: as the caller promises; the root signature, pipeline and heap are the layer's.
        // (Heap 10: where its descriptors start, for shaders. List 30 sets the root signature,
        // 25 the pipeline state, 28 the descriptor heaps, 32 the root descriptor table, 20 the
        // topology (5: a triangle strip), 21 the viewport, 22 the scissor; 12 draws.)
        unsafe {
            function::<unsafe extern "system" fn(Com, *mut u64) -> *mut u64>(self.heap, 10)(self.heap, &mut start);
            function::<unsafe extern "system" fn(Com, Com)>(list, 30)(list, self.root);
            function::<unsafe extern "system" fn(Com, Com)>(list, 25)(list, self.pipeline);
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(list, 28)(list, 1, &self.heap);
            function::<unsafe extern "system" fn(Com, u32, u64)>(list, 32)(list, 0, start);
            function::<unsafe extern "system" fn(Com, u32)>(list, 20)(list, 5);
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(list, 21)(list, 1, &viewport);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(list, 22)(list, 1, &scissor);
            function::<unsafe extern "system" fn(Com, u32, u32, u32, u32)>(list, 12)(list, 4, 1, 0, 0);
        }
        Ok(())
    }

    /// The texture (`ID3D12Resource`, `DXGI_FORMAT_B8G8R8A8_UNORM`, premultiplied alpha), in
    /// the state `D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE` once `update` has copied to it;
    /// null while the UI has no size. Owned by the layer: a program keeping it calls `AddRef`.
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
    /// Waits until the queue has done everything given to it so far (the frames that drew the
    /// layer), then lets go of all of it.
    #[inline]
    fn drop(&mut self) {
        if !self.fence.is_null() && self.signal("letting go").is_ok() {
            self.settle();
        }
        let held = [self.upload, self.texture, self.heap, self.pipeline, self.root, self.fence, self.list, self.allocator, self.queue, self.device];
        // SAFETY: each is held once, by this layer.
        unsafe { held.into_iter().for_each(|object| release(object)) };
    }
}

/// The pipeline state that draws the layer into targets of `format`: the quad as a strip of two
/// triangles, nothing culled, no depth or stencil, its colour blended as "one, one minus source
/// alpha".
#[inline]
fn pipeline(device: Com, root: Com, format: u32) -> Result<Com, Error> {
    let none = Bytecode { code: null(), size: 0 };
    let shaders = [Bytecode { code: CORNER.as_ptr(), size: CORNER.len() * 4 }, Bytecode { code: PIXEL.as_ptr(), size: PIXEL.len() * 4 }, none, none, none];
    // One, one minus source alpha, add (2, 6, 1); logic "no op" (4) off; all four written.
    let target = TargetBlend { enable: 1, logic_enable: 0, source: 2, destination: 6, operation: 1, source_alpha: 2, destination_alpha: 6, operation_alpha: 1, logic: 4, write_mask: 0xf };
    let described = PipelineDescription {
        root,
        shaders,
        stream_output: StreamOutput { declarations: null(), count: 0, strides: null(), stride_count: 0, rasterized: 0 },
        alpha_to_coverage: 0,
        independent_blend: 0,
        targets: [target; 8],
        sample_mask: u32::MAX,
        // Solid (3), nothing culled (1), depth clipped.
        rasterizer: [3, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        // No depth (test always passes: 8) and no stencil (masks 0xff, operations keep: 1).
        depth: [0, 0, 8, 0, 0xffff, 1, 1, 1, 8, 1, 1, 1, 8],
        input_layout: [0; 2],
        strip_cut: 0,
        // Triangles (3).
        topology: 3,
        target_count: 1,
        formats: [format, 0, 0, 0, 0, 0, 0, 0],
        depth_format: 0,
        samples: [1, 0],
        node_mask: 0,
        cached: [0; 2],
        flags: 0,
    };
    let mut pipeline = null_mut();
    type Make = unsafe extern "system" fn(Com, *const PipelineDescription, *const [u32; 4], *mut Com) -> i32;
    // SAFETY: the device is alive; the description, and the code it points to, are whole
    // (device vtable 10: CreateGraphicsPipelineState).
    check(unsafe { function::<Make>(device, 10)(device, &described, &PIPELINE, &mut pipeline) }, "making the pipeline state")?;
    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structures handed to Direct3D 12 are as large as d3d12.h makes them on 64-bit
    /// Windows.
    #[test]
    fn the_structures_are_laid_out_as_direct3d_lays_them_out() {
        assert_eq!(size_of::<HeapProperties>(), 20);
        assert_eq!(size_of::<ResourceDescription>(), 56);
        assert_eq!(size_of::<Barrier>(), 32);
        assert_eq!(size_of::<CopyPlace>(), 48);
        assert_eq!(size_of::<TargetBlend>(), 40);
        assert_eq!(size_of::<PipelineDescription>(), 656);
        assert_eq!(std::mem::offset_of!(PipelineDescription, sample_mask), 448);
        assert_eq!(std::mem::offset_of!(PipelineDescription, input_layout), 552);
        assert_eq!(std::mem::offset_of!(PipelineDescription, formats), 580);
        assert_eq!(std::mem::offset_of!(PipelineDescription, cached), 632);
    }

    /// The bytecode is what Windows' compiler makes of the source above it.
    #[test]
    fn the_shaders_are_their_source_compiled() {
        type Compile = unsafe extern "system" fn(*const u8, usize, *const u8, *const c_void, *const c_void, *const u8, *const u8, u32, u32, *mut Com, *mut Com) -> i32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryW(name: *const u16) -> isize;
            fn GetProcAddress(library: isize, name: *const u8) -> usize;
        }
        let file = include_str!("layer_d3d12.rs").replace("\r\n", "\n");
        let start = file.find("/// ```hlsl\n").unwrap() + "/// ```hlsl\n".len();
        let end = start + file[start..].find("/// ```\n").unwrap();
        let source: String = file[start..end].lines().map(|line| format!("{}\n", line.strip_prefix("/// ").unwrap_or(""))).collect();
        let library: Vec<u16> = "d3dcompiler_47.dll\0".encode_utf16().collect();
        let compiled = |entry: &str, target: &str| unsafe {
            let compile: Compile = std::mem::transmute(GetProcAddress(LoadLibraryW(library.as_ptr()), c"D3DCompile".as_ptr().cast()));
            let (entry, target, mut code, mut errors) = (format!("{entry}\0"), format!("{target}\0"), null_mut(), null_mut());
            assert!(compile(source.as_ptr(), source.len(), null(), null(), null(), entry.as_ptr(), target.as_ptr(), 0, 0, &mut code, &mut errors) >= 0);
            let start = function::<unsafe extern "system" fn(Com) -> *const u8>(code, 3)(code);
            let bytes = std::slice::from_raw_parts(start, function::<unsafe extern "system" fn(Com) -> usize>(code, 4)(code));
            let words: Vec<u32> = bytes.chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
            [code, errors].into_iter().for_each(|blob| release(blob));
            words
        };
        assert_eq!(compiled("corner", "vs_5_0"), CORNER, "the vertex shader");
        assert_eq!(compiled("pixel", "ps_5_0"), PIXEL, "the pixel shader");
        assert_eq!(compiled("TAUT", "rootsig_1_0"), SIGNATURE, "the root signature");
    }
}
