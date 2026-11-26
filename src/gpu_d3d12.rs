//! Windows' Direct3D 12 path (the feature `gpu`): a window's pixels, drawn by the CPU, shown
//! through a DXGI flip-model swap chain on a Direct3D 12 queue.
//!
//! - A paint's strips are written to an upload buffer and copied into a texture holding the
//!   whole window (`CopyTextureRegion`): only what changed. The texture is copied to the back
//!   buffer and presented with the changed rectangles (`Present1`, `FLIP_SEQUENTIAL`), at the
//!   screen's refresh.
//! - Each frame is waited for (a fence), so the upload buffer is free again for the next. A
//!   removed software device (WARP) is waited for by `Marks` instead.
//! - d3d12.dll and dxgi.dll are loaded from the system folder when a window first asks for
//!   Direct3D 12. COM methods are called by their place in the vtable, as d3d12.h and
//!   dxgi1_4.h order them.
use crate::gpu_d3d11::{ChainDescription, Changed};
use crate::gpu_windows::inside;
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, function, release};
use crate::windows::sys::{HANDLE, RECT};
use crate::windows::{function_in, named, system_library};
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(security: *const c_void, manual: i32, initial: i32, name: *const u16) -> HANDLE;
    fn WaitForSingleObject(handle: usize, millis: u32) -> u32;
    fn CloseHandle(handle: usize) -> i32;
}

/// `D3D12CreateDevice` and `CreateDXGIFactory2`, as d3d12.h and dxgi1_3.h declare them.
type MakeDevice = unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32;
type MakeFactory = unsafe extern "system" fn(u32, *const [u32; 4], *mut Com) -> i32;
type Ask = unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32;

/// The interfaces asked for.
const DEVICE: [u32; 4] = named(0x1898_19F1, 0x1DB6, 0x4B57, [0xBE, 0x54, 0x18, 0x21, 0x33, 0x9B, 0x85, 0xF7]);
const QUEUE: [u32; 4] = named(0x0EC8_70A6, 0x5D7E, 0x4C22, [0x8C, 0xFC, 0x5B, 0xAA, 0xE0, 0x76, 0x16, 0xED]);
const ALLOCATOR: [u32; 4] = named(0x6102_DEE4, 0xAF59, 0x4B09, [0xB9, 0x99, 0xB4, 0x4D, 0x73, 0xF0, 0x9B, 0x24]);
const LIST: [u32; 4] = named(0x5B16_0D0F, 0xAC1B, 0x4185, [0x8B, 0xA8, 0xB3, 0xAE, 0x42, 0xA5, 0xA4, 0x55]);
const LIST2: [u32; 4] = named(0x38C3_E585, 0xFF17, 0x412C, [0x91, 0x50, 0x4F, 0xC6, 0xF9, 0xD7, 0x2A, 0x28]);
const RESOURCE: [u32; 4] = named(0x6964_42BE, 0xA72E, 0x4059, [0xBC, 0x79, 0x5B, 0x5C, 0x98, 0x04, 0x0F, 0xAD]);
const FENCE: [u32; 4] = named(0x0A75_3DCF, 0xC4D8, 0x4B91, [0xAD, 0xF6, 0xBE, 0x5A, 0x60, 0xD9, 0x5A, 0x76]);
const FACTORY: [u32; 4] = named(0x1BC6_EA02, 0xEF36, 0x464F, [0xBF, 0x0C, 0x21, 0xCA, 0x39, 0xE5, 0x16, 0x8A]);
const CHAIN3: [u32; 4] = named(0x94D9_9BDB, 0xF1F8, 0x4AB0, [0xB2, 0x36, 0x7D, 0xA0, 0x17, 0x0E, 0xDA, 0xB1]);

// Resource states and heap kinds, as d3d12.h has them.
const PRESENT: u32 = 0;
const COPY_DESTINATION: u32 = 0x400;
const COPY_SOURCE: u32 = 0x800;
const GENERIC_READ: u32 = 0xAC3;
const DEFAULT_HEAP: u32 = 1;
const UPLOAD_HEAP: u32 = 2;
const READBACK_HEAP: u32 = 3;
/// A row of an upload's pixels starts at a multiple of this, and each upload at twice it.
const ROW_ALIGNMENT: usize = 256;

/// `D3D12_HEAP_PROPERTIES`.
#[repr(C)]
struct Heap {
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
    mip_levels: u16,
    format: u32,
    samples: [u32; 2],
    layout: u32,
    flags: u32,
}

/// `D3D12_TEXTURE_COPY_LOCATION`: a texture's first subresource (`kind` 0), or a placed
/// footprint in a buffer (1): its offset, format, width, height, depth and row pitch.
#[repr(C)]
pub(crate) struct Place {
    pub(crate) resource: Com,
    pub(crate) kind: u32,
    pub(crate) offset: u64,
    pub(crate) footprint: [u32; 5],
}

/// `D3D12_RESOURCE_BARRIER`, a transition of a whole resource.
#[repr(C)]
pub(crate) struct Barrier {
    kind: u32,
    flags: u32,
    resource: Com,
    subresource: u32,
    before: u32,
    after: u32,
}

/// A transition of all of `resource` from `before` to `after`.
pub(crate) fn barrier(resource: Com, before: u32, after: u32) -> Barrier {
    Barrier { kind: 0, flags: 0, resource, subresource: 0xFFFF_FFFF, before, after }
}

/// A committed resource of `described` in a heap of `kind`, starting in `state`.
fn committed(device: Com, kind: u32, described: &ResourceDescription, state: u32) -> Option<Com> {
    let (heap, mut made) = (Heap { kind, page: 0, pool: 0, creation_mask: 0, visible_mask: 0 }, null_mut());
    type Make = unsafe extern "system" fn(Com, *const Heap, u32, *const ResourceDescription, u32, *const c_void, *const [u32; 4], *mut Com) -> i32;
    // SAFETY: the device's CreateCommittedResource (27), with whole descriptions.
    let result = unsafe { function::<Make>(device, 27)(device, &heap, 0, described, state, null(), &RESOURCE, &mut made) };
    (result >= 0).then_some(made)
}

/// A texture `size` pixels, B8G8R8A8, in the GPU's own memory, to copy to and from; and to draw
/// into, where the device draws windows (`gpu-draw`).
fn texture(device: &Device, size: [i32; 2]) -> Option<Com> {
    let flags = drawn(device);
    let described =
        ResourceDescription { dimension: 3, alignment: 0, width: size[0] as u64, height: size[1] as u32, depth: 1, mip_levels: 1, format: B8G8R8A8_UNORM, samples: [1, 0], layout: 0, flags };
    committed(device.device, DEFAULT_HEAP, &described, COPY_DESTINATION)
}

/// A texture `size` pixels of `format` in the GPU's own memory, to copy to first: an HDR
/// window's gains (`hdr_windows`).
#[cfg(feature = "hdr")]
pub(crate) fn texture_of(device: Com, size: [i32; 2], format: u32) -> Option<Com> {
    let described = ResourceDescription { dimension: 3, alignment: 0, width: size[0] as u64, height: size[1] as u32, depth: 1, mip_levels: 1, format, samples: [1, 0], layout: 0, flags: 0 };
    committed(device, DEFAULT_HEAP, &described, COPY_DESTINATION)
}

/// How a window's texture is made: drawn into where the device draws windows, else only
/// copied.
#[inline]
fn drawn(device: &Device) -> u32 {
    #[cfg(feature = "gpu-draw")]
    if device.drawing.is_some() {
        return crate::gpu_draw_d3d12::DRAWN_INTO;
    }
    let _ = device;
    0
}

/// A buffer of `bytes` in a heap of `kind` (upload or read back), starting in `state`, and where
/// the CPU sees it.
pub(crate) fn buffer(device: Com, kind: u32, bytes: usize, state: u32) -> Option<(Com, *mut u8)> {
    let described = ResourceDescription { dimension: 1, alignment: 0, width: bytes as u64, height: 1, depth: 1, mip_levels: 1, format: 0, samples: [1, 0], layout: 1, flags: 0 };
    let (made, mut there) = (committed(device, kind, &described, state)?, null_mut());
    // SAFETY: the buffer's Map (8), with nothing read (an empty range).
    if unsafe { function::<unsafe extern "system" fn(Com, u32, *const [usize; 2], *mut *mut u8) -> i32>(made, 8)(made, 0, &[0, 0], &mut there) } < 0 {
        // SAFETY: held here alone.
        unsafe { release(made) };
        return None;
    }
    Some((made, there))
}

/// `D3D12_WRITEBUFFERIMMEDIATE_PARAMETER`.
#[repr(C)]
struct Mark {
    at: u64,
    value: u32,
}

/// How far a software device (WARP) really is. Once removed, its fence reads done at once, while
/// it goes on running the commands it was given. So each command list ends by writing its
/// number here, once all before it is done (`WriteBufferImmediate`, `MARKER_OUT`).
struct Marks {
    /// The command list, as `ID3D12GraphicsCommandList2`.
    list: Com,
    /// A buffer read back, where the CPU sees it, and its GPU address.
    buffer: Com,
    seen: *const AtomicU32,
    at: u64,
    /// The number of the last list run.
    last: Cell<u32>,
}

/// A removed software device still ran ten seconds after it was waited for. It may write to
/// anything Direct3D 12 made, so nothing is released from then on.
static ADRIFT: AtomicBool = AtomicBool::new(false);

/// The Direct3D 12 device a program's windows are shown with: its queue, the one command list
/// every frame is recorded in, a fence for waiting on it, and the DXGI factory.
pub(crate) struct Device {
    device: Com,
    queue: Com,
    allocator: Com,
    list: Com,
    fence: Com,
    event: HANDLE,
    /// The fence's value last asked for.
    signalled: Cell<u64>,
    factory: Com,
    /// Where it is a software device.
    marks: Option<Marks>,
    /// What it draws windows with, where it does (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: Option<crate::gpu_draw_d3d12::Pipeline>,
}

impl Device {
    /// A device on the first GPU DXGI lists that takes Direct3D 12: not a software adapter, not
    /// Microsoft's. Nothing where none does.
    pub(crate) fn gpu() -> Option<Device> {
        let make = function_in(system_library("d3d12.dll"), c"D3D12CreateDevice");
        let factory = function_in(system_library("dxgi.dll"), c"CreateDXGIFactory2");
        // SAFETY: the functions as the headers declare them.
        let (make, factory) = unsafe { (make != 0 && factory != 0).then(|| (std::mem::transmute::<usize, MakeDevice>(make), std::mem::transmute::<usize, MakeFactory>(factory)))? };
        let mut made = Device {
            device: null_mut(),
            queue: null_mut(),
            allocator: null_mut(),
            list: null_mut(),
            fence: null_mut(),
            event: 0,
            signalled: Cell::new(0),
            factory: null_mut(),
            marks: None,
            #[cfg(feature = "gpu-draw")]
            drawing: None,
        };
        // SAFETY: as declared; the factory is released with the device.
        if unsafe { factory(0, &FACTORY, &mut made.factory) } < 0 {
            return None;
        }
        let software;
        (made.device, software) = (0..16).find_map(|at| made.on_adapter(at, make))?;
        if !made.with_queue() || (software && !made.with_marks()) {
            return None;
        }
        // Its shaders are compiled here, on the device's own thread.
        #[cfg(feature = "gpu-draw")]
        {
            made.drawing = crate::gpu_draw_d3d12::Pipeline::new(made.device);
        }
        Some(made)
    }

    /// A Direct3D 12 device on DXGI's adapter `at`, if it is a GPU and takes one (feature level
    /// 11.0, 0xB000); and whether it is a software device.
    fn on_adapter(&self, at: u32, make: MakeDevice) -> Option<(Com, bool)> {
        let (mut adapter, mut described, mut device) = (null_mut(), [0u64; 40], null_mut());
        // SAFETY: the factory's EnumAdapters1 (12), the adapter's GetDesc1 (10) into room for a
        // `DXGI_ADAPTER_DESC1` (312 bytes: the vendor after the 256-byte name, the flags at 304);
        // D3D12CreateDevice. The adapter is released.
        unsafe {
            #[cfg(test)]
            if crate::gpu_windows::tests::faked().software {
                return tests::on_warp(self.factory, at, make).map(|device| (device, true));
            }
            if function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(self.factory, 12)(self.factory, at, &mut adapter) < 0 {
                return None;
            }
            function::<unsafe extern "system" fn(Com, *mut u64) -> i32>(adapter, 10)(adapter, described.as_mut_ptr());
            let software = described[32] as u32 == 0x1414 || described[38] as u32 & 2 != 0;
            let made = (!software && make(adapter, 0xB000, &DEVICE, &mut device) >= 0).then_some((device, false));
            release(adapter);
            made
        }
    }

    /// Makes the queue, the command list (closed) and its allocator, the fence and its event.
    /// Says whether all were made.
    fn with_queue(&mut self) -> bool {
        let (device, queue) = (self.device, [0u32; 4]);
        type MakeAllocator = unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32;
        type MakeList = unsafe extern "system" fn(Com, u32, u32, Com, Com, *const [u32; 4], *mut Com) -> i32;
        type MakeFence = unsafe extern "system" fn(Com, u64, u32, *const [u32; 4], *mut Com) -> i32;
        // SAFETY: the device's CreateCommandQueue (8, a direct queue), CreateCommandAllocator (9),
        // CreateCommandList (12) and CreateFence (36); the list's Close (9).
        unsafe {
            function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 8)(device, &queue, &QUEUE, &mut self.queue);
            function::<MakeAllocator>(device, 9)(device, 0, &ALLOCATOR, &mut self.allocator);
            if !self.allocator.is_null() {
                function::<MakeList>(device, 12)(device, 0, 0, self.allocator, null_mut(), &LIST, &mut self.list);
            }
            if !self.list.is_null() {
                function::<unsafe extern "system" fn(Com) -> i32>(self.list, 9)(self.list);
            }
            function::<MakeFence>(device, 36)(device, 0, 0, &FENCE, &mut self.fence);
            self.event = CreateEventW(null(), 0, 0, null());
        }
        ![self.queue, self.allocator, self.list, self.fence].iter().any(|made| made.is_null()) && self.event != 0
    }

    /// Makes `marks`. Says whether it could.
    fn with_marks(&mut self) -> bool {
        let Some((buffer, seen)) = buffer(self.device, READBACK_HEAP, 4, COPY_DESTINATION) else { return false };
        let mut list = null_mut();
        // SAFETY: the list's QueryInterface (0); the buffer's GetGPUVirtualAddress (11).
        let at = unsafe {
            function::<Ask>(self.list, 0)(self.list, &LIST2, &mut list);
            function::<unsafe extern "system" fn(Com) -> u64>(buffer, 11)(buffer)
        };
        self.marks = Some(Marks { list, buffer, seen: seen.cast(), at, last: Cell::new(0) });
        !list.is_null()
    }

    /// Whether the device is gone: removed, reset or hung (`GetDeviceRemovedReason`, 37).
    pub(crate) fn removed(&self) -> bool {
        // SAFETY: a plain query of a live device.
        unsafe { function::<unsafe extern "system" fn(Com) -> i32>(self.device, 37)(self.device) != 0 }
    }

    /// Opens the command list for recording.
    fn open(&self) {
        // SAFETY: the GPU is done with the allocator (every frame is waited for): its Reset (8),
        // and the list's (10).
        unsafe {
            function::<unsafe extern "system" fn(Com) -> i32>(self.allocator, 8)(self.allocator);
            function::<unsafe extern "system" fn(Com, Com, Com) -> i32>(self.list, 10)(self.list, self.allocator, null_mut());
        }
    }

    /// Closes the command list (9), has the queue carry it out (ExecuteCommandLists, 10), and
    /// signals the fence (Signal, 14). Says whether that was taken.
    fn run(&self) -> bool {
        let next = self.marks.as_ref().map(|marks| self.mark(marks));
        // SAFETY: the list is open, and is this device's.
        unsafe {
            if function::<unsafe extern "system" fn(Com) -> i32>(self.list, 9)(self.list) < 0 {
                return false;
            }
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(self.queue, 10)(self.queue, 1, &self.list);
        }
        if let (Some(marks), Some(next)) = (&self.marks, next) {
            marks.last.set(next);
        }
        self.run_nothing()
    }

    /// Ends the open command list by writing its number to `marks`. Gives the number.
    fn mark(&self, marks: &Marks) -> u32 {
        let mark = Mark { at: marks.at, value: marks.last.get().wrapping_add(1) };
        // SAFETY: the list's WriteBufferImmediate (66), after all before it (`MARKER_OUT`, 2), to
        // a buffer read back, which is always a copy's destination.
        unsafe { function::<unsafe extern "system" fn(Com, u32, *const Mark, *const u32)>(marks.list, 66)(marks.list, 1, &mark, &2) };
        mark.value
    }

    /// Signals the fence after what the queue has (Signal, 14). Says whether that was taken.
    fn run_nothing(&self) -> bool {
        self.signalled.set(self.signalled.get() + 1);
        // SAFETY: the queue and fence are this device's.
        unsafe { function::<unsafe extern "system" fn(Com, Com, u64) -> i32>(self.queue, 14)(self.queue, self.fence, self.signalled.get()) >= 0 }
    }

    /// Waits until the GPU has done all it was given (two seconds at most, ten once removed). Says
    /// whether it has.
    fn wait(&self) -> bool {
        let wanted = self.signalled.get();
        // SAFETY: the fence's GetCompletedValue (8) and SetEventOnCompletion (9), with this
        // device's event.
        let done = || unsafe { function::<unsafe extern "system" fn(Com) -> u64>(self.fence, 8)(self.fence) };
        if done() < wanted {
            // SAFETY: as above.
            unsafe {
                function::<unsafe extern "system" fn(Com, u64, HANDLE) -> i32>(self.fence, 9)(self.fence, wanted, self.event);
                WaitForSingleObject(self.event as usize, 2000);
            }
        }
        // A removed device's fence reads all ones at once, done or not.
        match done() {
            u64::MAX => self.settled(),
            done => done >= wanted,
        }
    }

    /// After a removal, waits until the device has run every list it was given, as `marks` says
    /// (ten seconds at most). A GPU stops at once, and Windows keeps its memory until it has.
    /// Says whether it has; if not, nothing is released from then on (`ADRIFT`).
    fn settled(&self) -> bool {
        let Some(marks) = &self.marks else { return true };
        let start = Instant::now();
        // SAFETY: the buffer stays where it was mapped while the device lives.
        while unsafe { (*marks.seen).load(Ordering::Acquire) } != marks.last.get() {
            if start.elapsed() > Duration::from_secs(10) {
                ADRIFT.store(true, Ordering::Relaxed);
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}

// SAFETY: the device and what it holds belong to no thread, and are used by one at a time: made
// on a thread of their own, then handed whole to the windows' thread.
unsafe impl Send for Device {}

impl Drop for Device {
    /// Waits for the GPU, then releases everything.
    fn drop(&mut self) {
        if !self.queue.is_null() && !self.fence.is_null() && self.run_nothing() {
            self.wait();
        }
        if ADRIFT.load(Ordering::Relaxed) {
            #[cfg(feature = "gpu-draw")]
            std::mem::forget(self.drawing.take());
            return;
        }
        #[cfg(feature = "gpu-draw")]
        drop(self.drawing.take());
        let marks = self.marks.take().map_or([null_mut(); 2], |marks| [marks.list, marks.buffer]);
        // SAFETY: each is held once, by this device; the event is its own.
        unsafe {
            [marks[0], marks[1], self.list, self.allocator, self.fence, self.queue, self.device, self.factory].into_iter().for_each(|object| release(object));
            if self.event != 0 {
                CloseHandle(self.event as usize);
            }
        }
    }
}

/// An upload written and waiting to be copied into the window's texture: where it is in the
/// upload buffer, its row pitch in bytes, and where it goes.
struct Waiting {
    offset: usize,
    pitch: u32,
    at: [u32; 2],
    size: [u32; 2],
}

/// A window's swap chain, the texture holding all of the window's pixels, and the upload
/// buffer the changed ones go through.
pub(crate) struct Shown {
    chain: Com,
    picture: Com,
    upload: Com,
    /// The upload buffer where the CPU writes it, and its size.
    written: *mut u8,
    room: usize,
    /// Where the next upload goes in it.
    next: usize,
    waiting: Vec<Waiting>,
    /// Width and height in pixels of the swap chain and the texture.
    size: [i32; 2],
    /// Its HDR, and the pass to scRGB once tried (see `hdr_windows`).
    #[cfg(feature = "hdr")]
    pub(crate) hdr: crate::hdr_windows::Hdr,
    #[cfg(feature = "hdr")]
    pass: Option<Option<crate::hdr_windows::Pass12>>,
}

impl Shown {
    /// A swap chain for window `hwnd` on `device`'s queue, `size` pixels: two buffers, no
    /// stretching, the alpha ignored. Nothing where DXGI makes none.
    pub(crate) fn new(device: &Device, hwnd: HANDLE, size: [i32; 2]) -> Option<Shown> {
        let [width, height] = size.map(|side| side.max(1) as u32);
        let described = ChainDescription { width, height, format: B8G8R8A8_UNORM, stereo: 0, sample_count: 1, sample_quality: 0, usage: 0x20, buffers: 2, scaling: 1, effect: 3, alpha: 3, flags: 0 };
        let mut shown = Shown {
            chain: null_mut(),
            picture: null_mut(),
            upload: null_mut(),
            written: null_mut(),
            room: 0,
            next: 0,
            waiting: Vec::new(),
            size: [0; 2],
            #[cfg(feature = "hdr")]
            hdr: Default::default(),
            #[cfg(feature = "hdr")]
            pass: None,
        };
        type MakeChain = unsafe extern "system" fn(Com, Com, HANDLE, *const ChainDescription, *const c_void, Com, *mut Com) -> i32;
        let mut chain = null_mut();
        // SAFETY: the factory's CreateSwapChainForHwnd (15) for a live window, on the queue; its
        // MakeWindowAssociation (8): DXGI keeps out of the window's messages. The chain is asked
        // for `IDXGISwapChain3`, and the first one released.
        unsafe {
            if function::<MakeChain>(device.factory, 15)(device.factory, device.queue, hwnd, &described, null(), null_mut(), &mut chain) < 0 {
                return None;
            }
            function::<unsafe extern "system" fn(Com, HANDLE, u32) -> i32>(device.factory, 8)(device.factory, hwnd, 3);
            function::<Ask>(chain, 0)(chain, &CHAIN3, &mut shown.chain);
            release(chain);
        }
        (!shown.chain.is_null()).then_some(shown)
    }

    /// Makes the buffers, the texture and the upload buffer `size` pixels where they are not.
    /// Says whether they were remade (the window is then to be drawn whole); nothing if that
    /// failed.
    pub(crate) fn fit(&mut self, device: &Device, size: [i32; 2]) -> Option<bool> {
        if size == self.size {
            return Some(false);
        }
        if ADRIFT.load(Ordering::Relaxed) {
            return None;
        }
        self.size = [0; 2];
        self.waiting.clear();
        // The GPU is done with the old ones: each frame is waited for.
        // SAFETY: held here alone; the swap chain's ResizeBuffers (13) while nothing holds its
        // buffers.
        unsafe {
            [std::mem::replace(&mut self.picture, null_mut()), std::mem::replace(&mut self.upload, null_mut())].into_iter().for_each(|object| release(object));
            if function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32) -> i32>(self.chain, 13)(self.chain, 0, size[0] as u32, size[1] as u32, 0, 0) < 0 {
                return None;
            }
        }
        self.picture = texture(device, size)?;
        self.room = (size[0] as usize * 4).next_multiple_of(ROW_ALIGNMENT) * size[1] as usize + 2 * ROW_ALIGNMENT;
        (self.upload, self.written) = buffer(device.device, UPLOAD_HEAP, self.room, GENERIC_READ)?;
        self.size = size;
        Some(true)
    }

    /// Writes `size` pixels at `at` (cut to the texture), read from `first`, rows `pitch` pixels
    /// apart, to the upload buffer; the next present copies them into the texture. A full
    /// buffer is copied first.
    ///
    /// # Safety
    /// The pixels read lie at `first`: as many rows as given, each as wide.
    pub(crate) unsafe fn upload(&mut self, device: &Device, at: [i32; 2], size: [i32; 2], first: *const u32, pitch: usize) {
        let [right, bottom] = [(at[0] + size[0]).min(self.size[0]), (at[1] + size[1]).min(self.size[1])];
        if self.upload.is_null() || at[0] < 0 || at[1] < 0 || right <= at[0] || bottom <= at[1] {
            return;
        }
        let ([width, height], row) = ([right - at[0], bottom - at[1]].map(|side| side as usize), ((right - at[0]) as usize * 4).next_multiple_of(ROW_ALIGNMENT));
        let mut offset = self.next.next_multiple_of(2 * ROW_ALIGNMENT);
        if offset + row * height > self.room {
            self.copy_waiting(device, None);
            device.wait();
            offset = 0;
        }
        for line in 0..height {
            // SAFETY: the rows read lie where the caller promises; those written in the buffer,
            // whose room was checked above.
            unsafe { std::ptr::copy_nonoverlapping(first.add(line * pitch).cast::<u8>(), self.written.add(offset + line * row), width * 4) };
        }
        self.next = offset + row * height;
        self.waiting.push(Waiting { offset, pitch: row as u32, at: at.map(|side| side as u32), size: [width as u32, height as u32] });
    }

    /// Records the waiting uploads' copies into the texture, then `then`, and has the queue carry
    /// them out. Says whether that was taken.
    fn copy_waiting(&mut self, device: &Device, then: Option<&dyn Fn(Com)>) -> bool {
        device.open();
        let list = device.list;
        type CopyRegion = unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3);
        for upload in self.waiting.drain(..) {
            let to = Place { resource: self.picture, kind: 0, offset: 0, footprint: [0; 5] };
            let from = Place { resource: self.upload, kind: 1, offset: upload.offset as u64, footprint: [B8G8R8A8_UNORM, upload.size[0], upload.size[1], 1, upload.pitch] };
            // SAFETY: the list's CopyTextureRegion (16): the footprint lies in the buffer, the
            // place in the texture.
            unsafe { function::<CopyRegion>(list, 16)(list, &to, upload.at[0], upload.at[1], 0, &from, null()) };
        }
        self.next = 0;
        if let Some(then) = then {
            then(list);
        }
        device.run()
    }

    /// Draws a recorded paint into the texture, inside `area` (left, top, right, bottom), after
    /// the uploads waiting; then waits for the GPU. Says whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, device: &mut Device, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        let Some(mut drawing) = device.drawing.take() else { return false };
        let (picture, size) = (self.picture, self.size);
        let ready = !picture.is_null() && drawing.prepare(device.device, list);
        let record = |commands: Com| drawing.record(device.device, commands, picture, size, list, area);
        let drawn = ready && self.copy_waiting(device, Some(&record)) && device.wait();
        if drawn {
            drawing.ran();
        }
        device.drawing = Some(drawing);
        drawn
    }

    /// Copies the waiting uploads into the texture, the texture to the back buffer, and presents
    /// it, `dirty` being what changed, at the screen's next refresh; then waits for the GPU. Says
    /// whether DXGI took it.
    pub(crate) fn present(&mut self, device: &Device, dirty: &[RECT]) -> bool {
        if self.picture.is_null() {
            return true;
        }
        #[cfg(feature = "hdr")]
        if !self.go_hdr(device) {
            return false;
        }
        let (dirty, mut back) = (inside(dirty, self.size), null_mut());
        let changed = Changed { count: dirty.len() as u32, rects: dirty.as_ptr(), scroll: null(), offset: null() };
        // SAFETY: the swap chain's GetCurrentBackBufferIndex (36) and GetBuffer (9).
        unsafe {
            let index = function::<unsafe extern "system" fn(Com) -> u32>(self.chain, 36)(self.chain);
            if function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(self.chain, 9)(self.chain, index, &RESOURCE, &mut back) < 0 {
                return false;
            }
        }
        #[cfg(test)]
        let kept = tests::Kept::new(device, back, self.size, self.chain, dirty);
        #[cfg(feature = "hdr")]
        let (pass, constants, size) = (self.lit(device), self.hdr.constants(), self.size);
        let picture = self.picture;
        let to_back = |list: Com| {
            #[cfg(feature = "hdr")]
            if let Some(pass) = &pass {
                return pass.record(device.device, list, picture, back, size, constants);
            }
            let before = [barrier(picture, COPY_DESTINATION, COPY_SOURCE), barrier(back, PRESENT, COPY_DESTINATION)];
            let after = [barrier(picture, COPY_SOURCE, COPY_DESTINATION), barrier(back, COPY_DESTINATION, PRESENT)];
            type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
            // SAFETY: the list's ResourceBarrier (26) and CopyResource (17), between textures of
            // one size and format.
            unsafe {
                function::<Barriers>(list, 26)(list, 2, before.as_ptr());
                function::<unsafe extern "system" fn(Com, Com, Com)>(list, 17)(list, back, picture);
                #[cfg(test)]
                kept.record(list);
                function::<Barriers>(list, 26)(list, 2, after.as_ptr());
            }
        };
        let taken = self.copy_waiting(device, Some(&to_back));
        #[cfg(test)]
        tests::remove_if_asked(device);
        // SAFETY: Present1 (22), in step with the screen (1), with rectangles inside the buffer;
        // the back buffer is released once presented.
        let presented = unsafe {
            let presented = function::<unsafe extern "system" fn(Com, u32, u32, *const Changed) -> i32>(self.chain, 22)(self.chain, 1, 0, &changed);
            release(back);
            presented
        };
        let done = device.run_nothing() && device.wait();
        #[cfg(test)]
        kept.read();
        #[cfg(feature = "hdr")]
        self.lit_ran(pass);
        taken && presented >= 0 && done
    }

    /// Puts the swap chain in scRGB where the window is lit on an HDR screen, its pass made first,
    /// and back where not. Says whether that worked.
    #[cfg(feature = "hdr")]
    fn go_hdr(&mut self, device: &Device) -> bool {
        if self.hdr.wanted() && self.pass.is_none() {
            self.pass = Some(crate::hdr_windows::Pass12::new(device.device));
        }
        self.hdr.switch(self.chain, self.size, self.pass.as_ref().is_some_and(Option::is_some))
    }

    /// The pass to scRGB, its gains ready, taken out while the frame is recorded: where the swap
    /// chain is in scRGB.
    #[cfg(feature = "hdr")]
    fn lit(&mut self, device: &Device) -> Option<crate::hdr_windows::Pass12> {
        if !self.hdr.on {
            return None;
        }
        let mut pass = self.pass.as_mut()?.take()?;
        if pass.prepare(device.device, &mut self.hdr.gains) {
            return Some(pass);
        }
        self.pass = Some(Some(pass));
        None
    }

    /// Puts the pass back once its frame ran.
    #[cfg(feature = "hdr")]
    fn lit_ran(&mut self, pass: Option<crate::hdr_windows::Pass12>) {
        let Some(mut pass) = pass else { return };
        pass.ran();
        #[cfg(test)]
        crate::hdr_windows::tests::read_kept();
        self.pass = Some(Some(pass));
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        if ADRIFT.load(Ordering::Relaxed) {
            return;
        }
        // SAFETY: each is held once, by this window; the GPU is done with them (every frame is
        // waited for).
        unsafe { [self.upload, self.picture, self.chain].into_iter().for_each(|object| release(object)) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(feature = "gpu-draw")]
    impl Device {
        /// The `ID3D12Device`.
        pub(crate) fn com(&self) -> Com {
            self.device
        }
    }

    /// A device on WARP, as the paths make one when a test fakes software devices.
    pub(crate) fn warp() -> Device {
        crate::gpu_windows::tests::fake(crate::gpu_windows::tests::Faked { software: true, ..Default::default() });
        let device = Device::gpu().expect("Direct3D 12 on WARP");
        crate::gpu_windows::tests::fake(Default::default());
        device
    }

    /// A texture `size` pixels drawn into, first filled with `under`, then given to `record`
    /// (with the device and its open command list), and read back once that has run.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn drawn_on_warp(device: &Device, size: [i32; 2], under: &[u32], record: impl FnOnce(Com, Com, Com)) -> Vec<u32> {
        let ([width, height], pitch) = (size.map(|side| side as usize), (size[0] as usize * 4).next_multiple_of(ROW_ALIGNMENT));
        let picture = texture(device, size).expect("a texture");
        let (upload, written) = buffer(device.device, UPLOAD_HEAP, pitch * height, GENERIC_READ).expect("an upload buffer");
        let (readback, read) = buffer(device.device, 3, pitch * height, COPY_DESTINATION).expect("a buffer to read back");
        for row in 0..height {
            unsafe { std::ptr::copy_nonoverlapping(under[row * width..].as_ptr().cast::<u8>(), written.add(row * pitch), width * 4) };
        }
        let footprint = [B8G8R8A8_UNORM, width as u32, height as u32, 1, pitch as u32];
        let whole = Place { resource: picture, kind: 0, offset: 0, footprint: [0; 5] };
        type CopyRegion = unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3);
        device.open();
        unsafe { function::<CopyRegion>(device.list, 16)(device.list, &whole, 0, 0, 0, &Place { resource: upload, kind: 1, offset: 0, footprint }, null()) };
        record(device.device, device.list, picture);
        unsafe {
            function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(device.list, 26)(device.list, 1, &barrier(picture, COPY_DESTINATION, COPY_SOURCE));
            function::<CopyRegion>(device.list, 16)(device.list, &Place { resource: readback, kind: 1, offset: 0, footprint }, 0, 0, 0, &whole, null());
        }
        assert!(device.run() && device.wait(), "the commands ran");
        let rows = (0..height).map(|row| unsafe { std::slice::from_raw_parts(read.add(row * pitch).cast::<u32>(), width) });
        let pixels = rows.flatten().copied().collect();
        unsafe { [readback, upload, picture].into_iter().for_each(|object| release(object)) };
        pixels
    }

    /// A device on WARP, the software adapter (the factory's EnumWarpAdapter, 27, for an
    /// `IDXGIAdapter1`), for adapter 0.
    pub(crate) fn on_warp(factory: Com, at: u32, make: MakeDevice) -> Option<Com> {
        let iid = named(0x2903_8F61, 0x3839, 0x4626, [0x91, 0xFD, 0x08, 0x68, 0x79, 0x01, 0x1A, 0x05]);
        let (mut adapter, mut device) = (null_mut(), null_mut());
        unsafe {
            function::<Ask>(factory, 27)(factory, &iid, &mut adapter);
            let made = (at == 0 && !adapter.is_null() && make(adapter, 0xB000, &DEVICE, &mut device) >= 0).then_some(device);
            release(adapter);
            made
        }
    }

    /// Removes the device for real (`ID3D12Device5::RemoveDevice`, 58), if a test asked.
    pub(crate) fn remove_if_asked(device: &Device) {
        let mut faked = crate::gpu_windows::tests::faked();
        if !std::mem::take(&mut faked.remove) {
            return;
        }
        crate::gpu_windows::tests::fake(faked);
        remove(device);
    }

    /// Removes the device for real (`ID3D12Device5::RemoveDevice`, 58).
    fn remove(device: &Device) {
        let iid = named(0x8B4F_173B, 0x2FEA, 0x4B80, [0x8F, 0x58, 0x43, 0x07, 0x19, 0x1A, 0xB9, 0x5D]);
        let mut five = null_mut();
        unsafe {
            assert!(function::<Ask>(device.device, 0)(device.device, &iid, &mut five) >= 0, "no ID3D12Device5");
            function::<unsafe extern "system" fn(Com)>(five, 58)(five);
            release(five);
        }
    }

    #[test]
    fn a_removed_software_device_is_waited_for_until_it_has_run_all_it_was_given() {
        // Removing a WARP device keeps any other from being made in the process until it is
        // released: so in a process of its own.
        crate::windows::tests::on_a_desktop_of_its_own("gpu_d3d12::tests::alone_a_removed_software_device_is_waited_for");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own"]
    fn alone_a_removed_software_device_is_waited_for() {
        if std::env::var_os(crate::windows::tests::ALONE).is_none() {
            return;
        }
        // Copies WARP takes a while over, removed as soon as they are given.
        let device = warp();
        let (from, to) = (texture(&device, [4096, 4096]).expect("a texture"), texture(&device, [4096, 4096]).expect("a texture"));
        device.open();
        unsafe {
            function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(device.list, 26)(device.list, 1, &barrier(from, COPY_DESTINATION, COPY_SOURCE));
            (0..8).for_each(|_| function::<unsafe extern "system" fn(Com, Com, Com)>(device.list, 17)(device.list, to, from));
        }
        assert!(device.run(), "the copies were taken");
        remove(&device);
        // Its fence reads done at once; the device may still be copying.
        let fence = unsafe { function::<unsafe extern "system" fn(Com) -> u64>(device.fence, 8)(device.fence) };
        let marks = device.marks.as_ref().expect("a software device keeps marks");
        let ran = || unsafe { (*marks.seen).load(Ordering::Acquire) };
        println!("at the removal: the fence {fence:#x}, lists run {} of {}", ran(), marks.last.get());
        assert_eq!(fence, u64::MAX);
        // Waited for, it has run them all: what they write to can go.
        assert!(device.wait(), "the device was waited for");
        assert_eq!(ran(), marks.last.get());
        unsafe { [to, from].into_iter().for_each(|object| release(object)) };
    }

    /// A copy of a back buffer as presented, if a test asked: copied into a buffer read back
    /// (`D3D12_HEAP_TYPE_READBACK`, 3) by the frame's own commands, read once it is done.
    pub(crate) struct Kept {
        back: Com,
        buffer: Option<(Com, *mut u8)>,
        size: [i32; 2],
        hwnd: HANDLE,
        dirty: Vec<RECT>,
    }

    impl Kept {
        pub(crate) fn new(device: &Device, back: Com, size: [i32; 2], chain: Com, dirty: &[RECT]) -> Kept {
            let mut hwnd = 0;
            unsafe { function::<unsafe extern "system" fn(Com, *mut HANDLE) -> i32>(chain, 20)(chain, &mut hwnd) };
            let pitch = (size[0] as usize * 4).next_multiple_of(ROW_ALIGNMENT);
            let buffer = crate::gpu_windows::tests::faked().keep.then(|| buffer(device.device, 3, pitch * size[1] as usize, COPY_DESTINATION).expect("no buffer to read back"));
            Kept { back, buffer, size, hwnd, dirty: dirty.to_vec() }
        }

        /// Records the back buffer's copy, between its copy from the texture and its present.
        pub(crate) fn record(&self, list: Com) {
            let Some((buffer, _)) = self.buffer else { return };
            let pitch = (self.size[0] as u32 * 4).next_multiple_of(ROW_ALIGNMENT as u32);
            let to = Place { resource: buffer, kind: 1, offset: 0, footprint: [B8G8R8A8_UNORM, self.size[0] as u32, self.size[1] as u32, 1, pitch] };
            let from = Place { resource: self.back, kind: 0, offset: 0, footprint: [0; 5] };
            type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
            unsafe {
                function::<Barriers>(list, 26)(list, 1, &barrier(self.back, COPY_DESTINATION, COPY_SOURCE));
                function::<unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3)>(list, 16)(list, &to, 0, 0, 0, &from, null());
                function::<Barriers>(list, 26)(list, 1, &barrier(self.back, COPY_SOURCE, COPY_DESTINATION));
            }
        }

        /// Reads the copy, once the GPU is done, and keeps it.
        pub(crate) fn read(self) {
            let Some((buffer, there)) = self.buffer else { return };
            let ([width, height], pitch) = (self.size.map(|side| side as usize), (self.size[0] as usize * 4).next_multiple_of(ROW_ALIGNMENT));
            let rows = (0..height).map(|row| unsafe { std::slice::from_raw_parts(there.add(row * pitch).cast::<u32>(), width) });
            let pixels = rows.flatten().copied().collect();
            unsafe { release(buffer) };
            crate::gpu_windows::tests::kept(self.hwnd, width, pixels, &self.dirty);
        }
    }
}
