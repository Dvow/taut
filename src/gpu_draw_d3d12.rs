//! Direct3D 12 drawing (the feature `gpu-draw`): a recorded paint (`gpu_draw::DrawList`) drawn
//! by Direct3D 12 into a window's texture, as Direct3D 11 draws it (`gpu_draw_windows`).
//!
//! - The shaders are Direct3D 11's HLSL, compiled when the device is made by
//!   `d3dcompiler_47.dll` for shader model 5.1 (`vs_5_1`, `ps_5_1`), with a root signature of
//!   one table: the quads, the cache and the patches (`rootsig_1_0`).
//! - The quads go up into a buffer in the upload heap, read where they are; the texels through
//!   an upload buffer, copied into the cache and patch textures.
//! - Everything is recorded into the device's command list, after the window's waiting copies:
//!   the caller runs it and waits.
//! - With `video`, videos' frames are read from the decoder's texture, opened here, in views
//!   four to seven of the table (`t3` to `t6`); the queue waits for the decoder's fence first
//!   (`gpu_frames_windows`).
use crate::gpu_draw::{DrawList, Quad, SIDE};
use crate::gpu_draw_windows::{compiled, source};
#[cfg(feature = "video")]
use crate::gpu_frames::ImportedFrames;
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, function, release};
#[cfg(feature = "video")]
use crate::video::FrameTexture;
use crate::windows::named;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

/// The interfaces made.
const ROOT: [u32; 4] = named(0xC54A_6B66, 0x72DF, 0x4EE8, [0x8B, 0xE5, 0xA9, 0x46, 0xA1, 0x42, 0x92, 0x14]);
const PIPELINE: [u32; 4] = named(0x765A_30F3, 0xF624, 0x4C6F, [0xA8, 0x28, 0xAC, 0xE9, 0x48, 0x62, 0x24, 0x45]);
const HEAP: [u32; 4] = named(0x8EFB_471D, 0x616C, 0x4F49, [0x90, 0xF7, 0x12, 0x7B, 0xB7, 0x63, 0xFA, 0x51]);
const RESOURCE: [u32; 4] = named(0x6964_42BE, 0xA72E, 0x4059, [0xBC, 0x79, 0x5B, 0x5C, 0x98, 0x04, 0x0F, 0xAD]);

// Resource states, as d3d12.h has them.
#[cfg(feature = "video")]
const COMMON: u32 = 0;
pub(crate) const RENDER_TARGET: u32 = 0x4;
const PIXEL_SHADER_RESOURCE: u32 = 0x80;
const COPY_DESTINATION: u32 = 0x400;
const GENERIC_READ: u32 = 0xAC3;
/// A resource that may be drawn into (`D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET`).
pub(crate) const DRAWN_INTO: u32 = 0x1;
/// Rows copied from a buffer start a multiple of this apart, and each copy at twice it.
const PITCH: usize = 256;
/// The root signature: one table of three views, the quads (t0), the cache (t1) and the
/// patches (t2), seen by every stage; with `video`, four more: videos' frames (t3 to t6).
#[cfg(not(feature = "video"))]
const SIGNATURE: &str = "#define ROOT \"DescriptorTable(SRV(t0, numDescriptors = 3))\"\n";
#[cfg(feature = "video")]
const SIGNATURE: &str = "#define ROOT \"DescriptorTable(SRV(t0, numDescriptors = 7))\"\n";
/// How many views the table has.
const VIEWS: usize = if cfg!(feature = "video") { 7 } else { 3 };
/// `ID3D12Fence`.
#[cfg(feature = "video")]
const FENCE: [u32; 4] = named(0x0A75_3DCF, 0xC4D8, 0x4B91, [0xAD, 0xF6, 0xBE, 0x5A, 0x60, 0xD9, 0x5A, 0x76]);

/// `D3D12_HEAP_PROPERTIES`.
#[repr(C)]
struct HeapProperties {
    kind: u32,
    page: u32,
    pool: u32,
    creation_mask: u32,
    visible_mask: u32,
}

/// `D3D12_RESOURCE_DESC`.
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

/// `D3D12_TEXTURE_COPY_LOCATION`: a texture's first subresource (`kind` 0), or a placed
/// footprint in a buffer (1): its offset, format, width, height, depth and row pitch.
#[repr(C)]
struct Place {
    resource: Com,
    kind: u32,
    offset: u64,
    footprint: [u32; 5],
}

/// `D3D12_RESOURCE_BARRIER`, a transition of a whole resource.
#[repr(C)]
struct Barrier {
    kind: u32,
    flags: u32,
    resource: Com,
    subresource: u32,
    before: u32,
    after: u32,
}

/// A transition of all of `resource` from `before` to `after`.
#[inline]
fn barrier(resource: Com, before: u32, after: u32) -> Barrier {
    Barrier { kind: 0, flags: 0, resource, subresource: 0xFFFF_FFFF, before, after }
}

/// `D3D12_SHADER_BYTECODE`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Bytecode {
    code: *const u32,
    size: usize,
}

/// `D3D12_STREAM_OUTPUT_DESC`.
#[repr(C)]
struct StreamOutput {
    declarations: *const c_void,
    count: u32,
    strides: *const u32,
    stride_count: u32,
    rasterized: u32,
}

/// `D3D12_RENDER_TARGET_BLEND_DESC`.
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
    stream_output: StreamOutput,
    alpha_to_coverage: i32,
    independent_blend: i32,
    targets: [TargetBlend; 8],
    sample_mask: u32,
    /// Fill, cull, front counter-clockwise, depth bias, its clamp and slope (as bits), depth
    /// clip, multisample, antialiased lines, forced sample count, conservative rasterization.
    rasterizer: [u32; 11],
    /// Depth and stencil, all off.
    depth: [u32; 13],
    input_layout: [usize; 2],
    strip_cut: u32,
    topology: u32,
    target_count: u32,
    formats: [u32; 8],
    depth_format: u32,
    samples: [u32; 2],
    node_mask: u32,
    cached: [usize; 2],
    flags: u32,
}

/// `D3D12_SHADER_RESOURCE_VIEW_DESC` of a buffer: format, dimension (1), component mapping,
/// then the first element, the count, the stride and flags.
#[repr(C)]
struct BufferView {
    format: u32,
    dimension: u32,
    mapping: u32,
    first: u64,
    count: u32,
    stride: u32,
    flags: u32,
}

/// A buffer in the upload heap: it, where the CPU writes it, and its size in bytes.
#[derive(Clone, Copy, Debug)]
struct Written {
    buffer: Com,
    at: *mut u8,
    room: usize,
}

/// A committed resource of `described` in a heap of `kind`, starting in `state`.
fn committed(device: Com, kind: u32, described: &ResourceDescription, state: u32) -> Option<Com> {
    let (heap, mut made) = (HeapProperties { kind, page: 0, pool: 0, creation_mask: 0, visible_mask: 0 }, null_mut());
    type Make = unsafe extern "system" fn(Com, *const HeapProperties, u32, *const ResourceDescription, u32, *const c_void, *const [u32; 4], *mut Com) -> i32;
    // SAFETY: the device's CreateCommittedResource (27), with whole descriptions.
    let result = unsafe { function::<Make>(device, 27)(device, &heap, 0, described, state, null(), &RESOURCE, &mut made) };
    (result >= 0).then_some(made)
}

/// A buffer of `bytes` in the upload heap, mapped; nothing where none is made.
fn upload_buffer(device: Com, bytes: usize) -> Option<Written> {
    let described = ResourceDescription { dimension: 1, alignment: 0, width: bytes as u64, height: 1, depth: 1, levels: 1, format: 0, samples: [1, 0], layout: 1, flags: 0 };
    let (buffer, mut at) = (committed(device, 2, &described, GENERIC_READ)?, null_mut());
    // SAFETY: the buffer's Map (8), with nothing read; released where it fails.
    unsafe {
        if function::<unsafe extern "system" fn(Com, u32, *const [usize; 2], *mut *mut u8) -> i32>(buffer, 8)(buffer, 0, &[0, 0], &mut at) < 0 {
            release(buffer);
            return None;
        }
    }
    Some(Written { buffer, at, room: bytes })
}

/// What a Direct3D 12 device draws with: its root signature and pipeline state, a shader-seen
/// heap of the three views and a heap of the target's, the cache and patch textures, and the
/// buffers the texels and quads go up through.
pub(crate) struct Pipeline {
    root: Com,
    pipeline: Com,
    views: Com,
    targets: Com,
    /// The views' step in their heap.
    step: usize,
    textures: [Com; 2],
    /// The textures are ready for shaders (else, never copied into yet).
    readied: bool,
    staging: Option<Written>,
    quads: Option<Written>,
    /// Videos' frames opened on this device; those the prepared list draws.
    #[cfg(feature = "video")]
    opened: ImportedFrames<OpenedFrame>,
    #[cfg(feature = "video")]
    frames: Vec<Taken>,
}

/// A video's frame opened on a device: its texture and the decoder's fence.
#[cfg(feature = "video")]
struct OpenedFrame {
    texture: Com,
    fence: Com,
}

#[cfg(feature = "video")]
impl OpenedFrame {
    /// `frame`'s texture and fence opened on `device` by their NT handles (OpenSharedHandle, 32);
    /// nothing where either can't be (another adapter's).
    fn new(device: Com, frame: &FrameTexture) -> Option<OpenedFrame> {
        let mut opened = OpenedFrame { texture: null_mut(), fence: null_mut() };
        if frame.handle == 0 || frame.fence_handle == 0 {
            return None;
        }
        type Open = unsafe extern "system" fn(Com, isize, *const [u32; 4], *mut Com) -> i32;
        // SAFETY: the handles are the live frame's; what is opened is held by this.
        unsafe {
            function::<Open>(device, 32)(device, frame.handle, &RESOURCE, &mut opened.texture);
            function::<Open>(device, 32)(device, frame.fence_handle, &FENCE, &mut opened.fence);
        }
        (!opened.texture.is_null() && !opened.fence.is_null()).then_some(opened)
    }
}

#[cfg(feature = "video")]
impl Drop for OpenedFrame {
    fn drop(&mut self) {
        // SAFETY: each is held once by this, or null; the device is done with them.
        unsafe { [self.fence, self.texture].into_iter().for_each(|object| release(object)) };
    }
}

/// A frame the prepared list draws: its texture here, and the fence and value the queue waits
/// for before.
#[cfg(feature = "video")]
#[derive(Clone, Copy)]
pub(crate) struct Taken {
    texture: Com,
    pub(crate) fence: Com,
    pub(crate) value: u64,
}

// SAFETY: as its device: made on the device's thread, then used by the windows' alone.
unsafe impl Send for Pipeline {}

impl std::fmt::Debug for Pipeline {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("readied", &self.readied).finish_non_exhaustive()
    }
}

/// A texture `SIDE` square that shaders read, in its first state (copied into).
fn texture(device: Com) -> Option<Com> {
    let side = SIDE as u32;
    let described = ResourceDescription { dimension: 3, alignment: 0, width: side as u64, height: side, depth: 1, levels: 1, format: B8G8R8A8_UNORM, samples: [1, 0], layout: 0, flags: 0 };
    committed(device, 1, &described, COPY_DESTINATION)
}

/// A descriptor heap of `kind` (0 views, 2 render targets) holding `count`, seen by shaders if
/// `seen`.
fn heap(device: Com, kind: u32, count: u32, seen: bool) -> Option<Com> {
    let (described, mut made) = ([kind, count, seen as u32, 0], null_mut());
    // SAFETY: the device's CreateDescriptorHeap (14) from a whole description.
    let result = unsafe { function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 14)(device, &described, &HEAP, &mut made) };
    (result >= 0).then_some(made)
}

/// Where `heap`'s descriptors start, for the CPU.
fn start_of(heap: Com) -> usize {
    let mut start = 0usize;
    // SAFETY: the heap's GetCPUDescriptorHandleForHeapStart (9), its result through a pointer.
    unsafe { function::<unsafe extern "system" fn(Com, *mut usize) -> *mut usize>(heap, 9)(heap, &mut start) };
    start
}

impl Pipeline {
    /// What `device` (an `ID3D12Device`) draws with; nothing where any part can't be made.
    pub(crate) fn new(device: Com) -> Option<Pipeline> {
        let source = source();
        let (corner, pixel) = (compiled(&source, c"corner", c"vs_5_1")?, compiled(&source, c"pixel", c"ps_5_1")?);
        let signature = compiled(SIGNATURE, c"ROOT", c"rootsig_1_0")?;
        let mut made = Pipeline {
            root: null_mut(),
            pipeline: null_mut(),
            views: null_mut(),
            targets: null_mut(),
            step: 0,
            textures: [null_mut(); 2],
            readied: false,
            staging: None,
            quads: None,
            #[cfg(feature = "video")]
            opened: ImportedFrames::default(),
            #[cfg(feature = "video")]
            frames: Vec::new(),
        };
        type MakeRoot = unsafe extern "system" fn(Com, u32, *const u32, usize, *const [u32; 4], *mut Com) -> i32;
        // SAFETY: the device's CreateRootSignature (16) from compiled words; its
        // GetDescriptorHandleIncrementSize (15) of views (0).
        unsafe {
            if function::<MakeRoot>(device, 16)(device, 0, signature.as_ptr(), signature.len() * 4, &ROOT, &mut made.root) < 0 {
                return None;
            }
            made.step = function::<unsafe extern "system" fn(Com, u32) -> u32>(device, 15)(device, 0) as usize;
        }
        made.pipeline = pipeline(device, made.root, &corner, &pixel)?;
        (made.views, made.targets) = (heap(device, 0, VIEWS as u32, true)?, heap(device, 2, 1, false)?);
        made.textures = [texture(device)?, texture(device)?];
        // The cache and patches in the second and third places; the cache in the frames' too,
        // until frames are drawn.
        for at in 1..VIEWS {
            made.view(device, made.textures[(at == 2) as usize], at);
        }
        Some(made)
    }

    /// A view of all of `texture` in the heap's place `at` (the device's
    /// CreateShaderResourceView, 18).
    fn view(&self, device: Com, texture: Com, at: usize) {
        // SAFETY: the texture is alive; the place lies in the heap.
        unsafe { function::<unsafe extern "system" fn(Com, Com, *const c_void, usize)>(device, 18)(device, texture, null(), start_of(self.views) + at * self.step) };
    }

    /// Readies `list` to be recorded: its texels and quads written to the upload buffers, made
    /// larger where they must be. (The device is done with them: each frame is waited for.)
    /// Says whether they hold it.
    pub(crate) fn prepare(&mut self, device: Com, list: &DrawList) -> bool {
        #[cfg(feature = "video")]
        if !self.take_frames(device, &list.frames) {
            return false;
        }
        let staged = list.uploads.iter().map(|upload| (upload.at[2] as usize * 4).next_multiple_of(PITCH) * upload.at[3] as usize + 2 * PITCH).sum::<usize>();
        let quads = list.quads.len().max(1) * size_of::<Quad>();
        let grown = self.quads.is_none_or(|written| written.room < quads);
        let (Some(staging), Some(quads)) = (Pipeline::hold(&mut self.staging, device, staged), Pipeline::hold(&mut self.quads, device, quads)) else { return false };
        if grown {
            self.view_quads(device, quads);
        }
        // SAFETY: the buffer holds the quads (checked above), mapped, read by nothing now.
        unsafe { std::ptr::copy_nonoverlapping(list.quads.as_ptr(), quads.at.cast::<Quad>(), list.quads.len()) };
        let mut offset = 0;
        for upload in &list.uploads {
            let ([width, height], pitch) = ([upload.at[2] as usize, upload.at[3] as usize], (upload.at[2] as usize * 4).next_multiple_of(PITCH));
            for row in 0..height {
                let from = &list.staging[upload.from + row * width..upload.from + (row + 1) * width];
                // SAFETY: the buffer holds every row at its pitch (summed above).
                unsafe { std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), staging.at.add(offset + row * pitch), width * 4) };
            }
            offset = (offset + pitch * height).next_multiple_of(2 * PITCH);
        }
        true
    }

    /// Readies `frames` to be drawn: each opened (as `gpu_frames` says), viewed in its place
    /// (four to seven, the places left the cache's), its fence's value now noted. Says whether
    /// all could be.
    #[cfg(feature = "video")]
    fn take_frames(&mut self, device: Com, frames: &[std::sync::Arc<FrameTexture>]) -> bool {
        self.frames.clear();
        if !self.opened.ready(frames, |frame| OpenedFrame::new(device, frame), drop) {
            return false;
        }
        let opened: Vec<[Com; 2]> = self.opened.drawn().map(|opened| [opened.texture, opened.fence]).collect();
        for (at, ([texture, fence], frame)) in opened.into_iter().zip(frames).enumerate() {
            self.view(device, texture, 3 + at);
            self.frames.push(Taken { texture, fence, value: frame.fence_value.load(std::sync::atomic::Ordering::Acquire) });
        }
        (3 + frames.len()..VIEWS).for_each(|at| self.view(device, self.textures[0], at));
        true
    }

    /// The frames the prepared list draws.
    #[cfg(feature = "video")]
    pub(crate) fn frames(&self) -> &[Taken] {
        &self.frames
    }

    /// Whether a video's frame could not be opened on this device.
    #[cfg(feature = "video")]
    pub(crate) fn refuses_frames(&self) -> bool {
        self.opened.refused()
    }

    /// The upload buffer `written`, made at least `bytes` big where it is not.
    fn hold(written: &mut Option<Written>, device: Com, bytes: usize) -> Option<Written> {
        if let Some(held) = written.filter(|held| held.room >= bytes) {
            return Some(held);
        }
        if let Some(old) = written.take() {
            // SAFETY: held here alone; the device is done with it.
            unsafe { release(old.buffer) };
        }
        *written = upload_buffer(device, bytes.next_power_of_two().max(65_536));
        *written
    }

    /// The quads' view in the heap's first place: `quads` as rows of four floats.
    fn view_quads(&self, device: Com, quads: Written) {
        // Four floats an element (`DXGI_FORMAT_R32G32B32A32_FLOAT`, 2), a buffer (1), every
        // component as it is (0x1688).
        let view = BufferView { format: 2, dimension: 1, mapping: 0x1688, first: 0, count: (quads.room / 16) as u32, stride: 0, flags: 0 };
        // SAFETY: the device's CreateShaderResourceView (18) from a whole description.
        unsafe { function::<unsafe extern "system" fn(Com, Com, *const BufferView, usize)>(device, 18)(device, quads.buffer, &view, start_of(self.views)) };
    }

    /// Records into `commands` (`ID3D12GraphicsCommandList`, open) the prepared list drawn into
    /// `picture` (a window's texture, `size` pixels, made to be drawn into, copied into before
    /// and after) inside `area` (left, top, right, bottom): its texels copied up first.
    pub(crate) fn record(&self, device: Com, commands: Com, picture: Com, size: [i32; 2], list: &DrawList, area: [i32; 4]) {
        self.put_up(commands, list);
        let target = start_of(self.targets);
        let mut views = 0u64;
        let (viewport, before, after) = ([0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0], barrier(picture, COPY_DESTINATION, RENDER_TARGET), barrier(picture, RENDER_TARGET, COPY_DESTINATION));
        type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
        // SAFETY: the device's CreateRenderTargetView (20), the whole texture; the heap's
        // GetGPUDescriptorHandleForHeapStart (10). The list's methods by place in its vtable,
        // each with what it takes: 26 ResourceBarrier, 30 SetGraphicsRootSignature, 25
        // SetPipelineState, 28 SetDescriptorHeaps, 32 SetGraphicsRootDescriptorTable, 20
        // IASetPrimitiveTopology (a strip, 5), 21 RSSetViewports, 22 RSSetScissorRects, 46
        // OMSetRenderTargets, 12 DrawInstanced.
        unsafe {
            function::<unsafe extern "system" fn(Com, Com, *const c_void, usize)>(device, 20)(device, picture, null(), target);
            function::<unsafe extern "system" fn(Com, *mut u64) -> *mut u64>(self.views, 10)(self.views, &mut views);
            function::<Barriers>(commands, 26)(commands, 1, &before);
            function::<unsafe extern "system" fn(Com, Com)>(commands, 30)(commands, self.root);
            function::<unsafe extern "system" fn(Com, Com)>(commands, 25)(commands, self.pipeline);
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(commands, 28)(commands, 1, &self.views);
            function::<unsafe extern "system" fn(Com, u32, u64)>(commands, 32)(commands, 0, views);
            function::<unsafe extern "system" fn(Com, u32)>(commands, 20)(commands, 5);
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(commands, 21)(commands, 1, &viewport);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(commands, 22)(commands, 1, &area);
            function::<unsafe extern "system" fn(Com, u32, *const usize, i32, *const usize)>(commands, 46)(commands, 1, &target, 0, null());
            #[cfg(feature = "video")]
            self.frames_to(commands, COMMON, PIXEL_SHADER_RESOURCE);
            if !list.quads.is_empty() {
                function::<unsafe extern "system" fn(Com, u32, u32, u32, u32)>(commands, 12)(commands, 4, list.quads.len() as u32, 0, 0);
            }
            #[cfg(feature = "video")]
            self.frames_to(commands, PIXEL_SHADER_RESOURCE, COMMON);
            function::<Barriers>(commands, 26)(commands, 1, &after);
        }
    }

    /// Records the frames drawn moved from state `before` to `after`: read by shaders, or back
    /// to the state shared textures are in between.
    #[cfg(feature = "video")]
    fn frames_to(&self, commands: Com, before: u32, after: u32) {
        let barriers: Vec<Barrier> = self.frames.iter().map(|frame| barrier(frame.texture, before, after)).collect();
        if barriers.is_empty() {
            return;
        }
        // SAFETY: the list's ResourceBarrier (26); each texture is alive, opened on this device.
        unsafe { function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(commands, 26)(commands, barriers.len() as u32, barriers.as_ptr()) };
    }

    /// Records the list's texels copied from the upload buffer into the textures, which are
    /// then ready for shaders.
    fn put_up(&self, commands: Com, list: &DrawList) {
        let Some(staging) = self.staging else { return };
        type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
        type CopyRegion = unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3);
        let (copied, mut offset) = ([0, 1].map(|texture| list.uploads.iter().any(|upload| upload.texture == texture)), 0);
        // Ready textures go back to being copied into, where they are; new ones are so.
        let to_copy: Vec<Barrier> = (0..2).filter(|at| copied[*at] && self.readied).map(|at| barrier(self.textures[at], PIXEL_SHADER_RESOURCE, COPY_DESTINATION)).collect();
        let to_read: Vec<Barrier> = (0..2).filter(|at| copied[*at] || !self.readied).map(|at| barrier(self.textures[at], COPY_DESTINATION, PIXEL_SHADER_RESOURCE)).collect();
        // SAFETY: the list's ResourceBarrier (26) and CopyTextureRegion (16): each footprint
        // lies in the buffer as `prepare` wrote it, each place in its texture.
        unsafe {
            if !to_copy.is_empty() {
                function::<Barriers>(commands, 26)(commands, to_copy.len() as u32, to_copy.as_ptr());
            }
            for upload in &list.uploads {
                let ([x, y, width, height], pitch) = (upload.at.map(|side| side as u32), (upload.at[2] as usize * 4).next_multiple_of(PITCH));
                let to = Place { resource: self.textures[upload.texture as usize], kind: 0, offset: 0, footprint: [0; 5] };
                let from = Place { resource: staging.buffer, kind: 1, offset: offset as u64, footprint: [B8G8R8A8_UNORM, width, height, 1, pitch as u32] };
                function::<CopyRegion>(commands, 16)(commands, &to, x, y, 0, &from, null());
                offset = (offset + pitch * height as usize).next_multiple_of(2 * PITCH);
            }
            if !to_read.is_empty() {
                function::<Barriers>(commands, 26)(commands, to_read.len() as u32, to_read.as_ptr());
            }
        }
    }

    /// Notes that what `record` recorded was run: the textures are ready for shaders.
    pub(crate) fn ran(&mut self) {
        self.readied = true;
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let written = [self.staging, self.quads].map(|written| written.map_or(null_mut(), |written| written.buffer));
        let all = [self.pipeline, self.root, self.views, self.targets, self.textures[0], self.textures[1], written[0], written[1]];
        // SAFETY: each is held once, by this pipeline; the device is done with them.
        unsafe { all.into_iter().for_each(|object| release(object)) };
    }
}

/// The pipeline state: the quads as triangle strips, nothing culled, no depth, blended over
/// what is there as Direct3D 11's: one, one minus the second colour.
fn pipeline(device: Com, root: Com, corner: &[u32], pixel: &[u32]) -> Option<Com> {
    let none = Bytecode { code: null(), size: 0 };
    let shaders = [Bytecode { code: corner.as_ptr(), size: corner.len() * 4 }, Bytecode { code: pixel.as_ptr(), size: pixel.len() * 4 }, none, none, none];
    // One (2), one minus the second colour (17), added (1); alpha likewise (2, 19, 1); logic
    // "no op" (4) off; all four written.
    let target = TargetBlend { enable: 1, logic_enable: 0, source: 2, destination: 17, operation: 1, source_alpha: 2, destination_alpha: 19, operation_alpha: 1, logic: 4, write_mask: 0xf };
    let described = PipelineDescription {
        root,
        shaders,
        stream_output: StreamOutput { declarations: null(), count: 0, strides: null(), stride_count: 0, rasterized: 0 },
        alpha_to_coverage: 0,
        independent_blend: 0,
        targets: [target; 8],
        sample_mask: u32::MAX,
        rasterizer: [3, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        depth: [0, 0, 8, 0, 0xffff, 1, 1, 1, 8, 1, 1, 1, 8],
        input_layout: [0; 2],
        strip_cut: 0,
        topology: 3,
        target_count: 1,
        formats: [B8G8R8A8_UNORM, 0, 0, 0, 0, 0, 0, 0],
        depth_format: 0,
        samples: [1, 0],
        node_mask: 0,
        cached: [0; 2],
        flags: 0,
    };
    let mut made = null_mut();
    type Make = unsafe extern "system" fn(Com, *const PipelineDescription, *const [u32; 4], *mut Com) -> i32;
    // SAFETY: the device's CreateGraphicsPipelineState (10); the description and the code it
    // points to are whole.
    (unsafe { function::<Make>(device, 10)(device, &described, &PIPELINE, &mut made) } >= 0).then_some(made)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::gpu_draw::tests::{Scene, compare};

    #[test]
    fn the_structures_are_laid_out_as_direct3d_lays_them_out() {
        assert_eq!(size_of::<PipelineDescription>(), 656);
        assert_eq!(std::mem::offset_of!(PipelineDescription, formats), 580);
        assert_eq!(size_of::<BufferView>(), 40);
        assert_eq!(std::mem::offset_of!(BufferView, first), 16);
        assert_eq!(size_of::<Place>(), 48);
        assert_eq!(size_of::<Barrier>(), 32);
    }

    /// Draws the scene on WARP's Direct3D 12 into a texture first filled with `under`, and
    /// reads it back.
    fn drawn(device: &crate::gpu_d3d12::Device, pipeline: &mut Pipeline, scene: &Scene, list: &DrawList) -> Vec<u32> {
        crate::gpu_d3d12::tests::drawn_on_warp(device, scene.size, &scene.under, |device, commands, picture| {
            assert!(pipeline.prepare(device, list));
            pipeline.record(device, commands, picture, scene.size, list, [0, 0, scene.size[0], scene.size[1]]);
        })
    }

    #[test]
    fn direct3d_12_draws_every_kind_of_thing_as_the_cpu_does() {
        let device = crate::gpu_d3d12::tests::warp();
        let mut pipeline = Pipeline::new(device.com()).expect("the shaders compile and the states are made");
        for scale in [1.0, 1.5, 2.0] {
            let (mut scene, mut list) = (Scene::new(scale), DrawList::default());
            scene.record(&mut list);
            assert!(!list.failed, "all of it on the GPU");
            let gpu = drawn(&device, &mut pipeline, &scene, &list);
            pipeline.ran();
            compare(&scene, &gpu, &format!("Direct3D 12 (WARP) at {scale}"));
            scene.record(&mut list);
            let again = drawn(&device, &mut pipeline, &scene, &list);
            compare(&scene, &again, &format!("Direct3D 12 again at {scale}"));
        }
        drop(pipeline);
    }
}
