//! Windows' Direct3D 12 path (the feature `gpu`): a window's pixels, drawn by the CPU, shown
//! through a DXGI flip-model swap chain on a Direct3D 12 queue.
//!
//! - A paint's strips are written to an upload buffer and copied into a texture holding the
//!   whole window (`CopyTextureRegion`): only what changed. The texture is copied to the back
//!   buffer and presented with the changed rectangles (`Present1`, `FLIP_SEQUENTIAL`), at the
//!   screen's refresh.
//! - Each frame is waited for (a fence), so the upload buffer is free again for the next.
//! - d3d12.dll and dxgi.dll are loaded from the system folder when a window first asks for
//!   Direct3D 12. COM methods are called by their place in the vtable, as d3d12.h and
//!   dxgi1_4.h order them.
use crate::gpu_d3d11::{ChainDescription, Changed};
use crate::gpu_windows::{function_in, inside, system_library};
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, function, release};
use crate::windows::named;
use crate::windows::sys::{HANDLE, RECT};
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

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
fn barrier(resource: Com, before: u32, after: u32) -> Barrier {
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

/// A texture `size` pixels, B8G8R8A8, in the GPU's own memory, to copy to and from.
fn texture(device: Com, size: [i32; 2]) -> Option<Com> {
    let described =
        ResourceDescription { dimension: 3, alignment: 0, width: size[0] as u64, height: size[1] as u32, depth: 1, mip_levels: 1, format: B8G8R8A8_UNORM, samples: [1, 0], layout: 0, flags: 0 };
    committed(device, DEFAULT_HEAP, &described, COPY_DESTINATION)
}

/// A buffer of `bytes` in a heap of `kind` (upload or read back), starting in `state`, and where
/// the CPU sees it.
fn buffer(device: Com, kind: u32, bytes: usize, state: u32) -> Option<(Com, *mut u8)> {
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
}

impl Device {
    /// A device on the first GPU DXGI lists that takes Direct3D 12: not a software adapter, not
    /// Microsoft's. Nothing where none does.
    pub(crate) fn gpu() -> Option<Device> {
        let make = function_in(system_library("d3d12.dll"), c"D3D12CreateDevice");
        let factory = function_in(system_library("dxgi.dll"), c"CreateDXGIFactory2");
        // SAFETY: the functions as the headers declare them.
        let (make, factory) = unsafe { (make != 0 && factory != 0).then(|| (std::mem::transmute::<usize, MakeDevice>(make), std::mem::transmute::<usize, MakeFactory>(factory)))? };
        let mut made = Device { device: null_mut(), queue: null_mut(), allocator: null_mut(), list: null_mut(), fence: null_mut(), event: 0, signalled: Cell::new(0), factory: null_mut() };
        // SAFETY: as declared; the factory is released with the device.
        if unsafe { factory(0, &FACTORY, &mut made.factory) } < 0 {
            return None;
        }
        made.device = (0..16).find_map(|at| made.on_adapter(at, make))?;
        made.with_queue().then_some(made)
    }

    /// A Direct3D 12 device on DXGI's adapter `at`, if it is a GPU and takes one (feature level
    /// 11.0, 0xB000).
    fn on_adapter(&self, at: u32, make: MakeDevice) -> Option<Com> {
        let (mut adapter, mut described, mut device) = (null_mut(), [0u64; 40], null_mut());
        // SAFETY: the factory's EnumAdapters1 (12), the adapter's GetDesc1 (10) into room for a
        // `DXGI_ADAPTER_DESC1` (312 bytes: the vendor after the 256-byte name, the flags at 304);
        // D3D12CreateDevice. The adapter is released.
        unsafe {
            #[cfg(test)]
            if crate::gpu_windows::tests::faked().software {
                return tests::on_warp(self.factory, at, make);
            }
            if function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(self.factory, 12)(self.factory, at, &mut adapter) < 0 {
                return None;
            }
            function::<unsafe extern "system" fn(Com, *mut u64) -> i32>(adapter, 10)(adapter, described.as_mut_ptr());
            let software = described[32] as u32 == 0x1414 || described[38] as u32 & 2 != 0;
            let made = (!software && make(adapter, 0xB000, &DEVICE, &mut device) >= 0).then_some(device);
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
        // SAFETY: the list is open, and is this device's.
        unsafe {
            if function::<unsafe extern "system" fn(Com) -> i32>(self.list, 9)(self.list) < 0 {
                return false;
            }
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(self.queue, 10)(self.queue, 1, &self.list);
        }
        self.run_nothing()
    }

    /// Signals the fence after what the queue has (Signal, 14). Says whether that was taken.
    fn run_nothing(&self) -> bool {
        self.signalled.set(self.signalled.get() + 1);
        // SAFETY: the queue and fence are this device's.
        unsafe { function::<unsafe extern "system" fn(Com, Com, u64) -> i32>(self.queue, 14)(self.queue, self.fence, self.signalled.get()) >= 0 }
    }

    /// Waits until the GPU has done all it was given (two seconds at most: a removed device
    /// signals at once). Says whether it has.
    fn wait(&self) -> bool {
        let wanted = self.signalled.get();
        // SAFETY: the fence's GetCompletedValue (8) and SetEventOnCompletion (9), with this
        // device's event.
        unsafe {
            if function::<unsafe extern "system" fn(Com) -> u64>(self.fence, 8)(self.fence) >= wanted {
                return true;
            }
            function::<unsafe extern "system" fn(Com, u64, HANDLE) -> i32>(self.fence, 9)(self.fence, wanted, self.event);
            WaitForSingleObject(self.event as usize, 2000) == 0
        }
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
        // SAFETY: each is held once, by this device; the event is its own.
        unsafe {
            [self.list, self.allocator, self.fence, self.queue, self.device, self.factory].into_iter().for_each(|object| release(object));
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
}

impl Shown {
    /// A swap chain for window `hwnd` on `device`'s queue, `size` pixels: two buffers, no
    /// stretching, the alpha ignored. Nothing where DXGI makes none.
    pub(crate) fn new(device: &Device, hwnd: HANDLE, size: [i32; 2]) -> Option<Shown> {
        let [width, height] = size.map(|side| side.max(1) as u32);
        let described = ChainDescription { width, height, format: B8G8R8A8_UNORM, stereo: 0, sample_count: 1, sample_quality: 0, usage: 0x20, buffers: 2, scaling: 1, effect: 3, alpha: 3, flags: 0 };
        let mut shown = Shown { chain: null_mut(), picture: null_mut(), upload: null_mut(), written: null_mut(), room: 0, next: 0, waiting: Vec::new(), size: [0; 2] };
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
        self.picture = texture(device.device, size)?;
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

    /// Copies the waiting uploads into the texture, the texture to the back buffer, and presents
    /// it, `dirty` being what changed, at the screen's next refresh; then waits for the GPU. Says
    /// whether DXGI took it.
    pub(crate) fn present(&mut self, device: &Device, dirty: &[RECT]) -> bool {
        if self.picture.is_null() {
            return true;
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
        let picture = self.picture;
        let to_back = |list: Com| {
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
        taken && presented >= 0 && done
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        // SAFETY: each is held once, by this window; the GPU is done with them (every frame is
        // waited for).
        unsafe { [self.upload, self.picture, self.chain].into_iter().for_each(|object| release(object)) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
        let iid = named(0x8B4F_173B, 0x2FEA, 0x4B80, [0x8F, 0x58, 0x43, 0x07, 0x19, 0x1A, 0xB9, 0x5D]);
        let mut five = null_mut();
        unsafe {
            assert!(function::<Ask>(device.device, 0)(device.device, &iid, &mut five) >= 0, "no ID3D12Device5");
            function::<unsafe extern "system" fn(Com)>(five, 58)(five);
            release(five);
        }
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
