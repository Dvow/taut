//! Windows' GPU path (the feature `gpu`): a window's pixels, drawn by the CPU as on the CPU path,
//! shown through Direct3D 11 and a DXGI flip-model swap chain.
//!
//! - A paint's strips go into a texture holding the whole window (`UpdateSubresource`): only
//!   what changed. The texture is copied to the back buffer and presented with the changed
//!   rectangles (`Present1`), at the screen's refresh.
//! - The swap chain is `FLIP_SEQUENTIAL`: DXGI refuses changed rectangles with `FLIP_DISCARD`
//!   (`DXGI_ERROR_INVALID_CALL`).
//! - d3d11.dll is loaded from the system folder when a window first asks for the GPU. COM
//!   methods are called by their place in the vtable, as d3d11.h and dxgi1_2.h order them.
//! - Its software device (WARP) shows windows after a loss with no GPU left (`gpu_windows`).
use crate::gpu_windows::{function_in, inside, system_library};
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, TextureDescription, function, release};
use crate::windows::named;
use crate::windows::sys::{HANDLE, RECT};
use std::ffi::c_void;
use std::ptr::{null, null_mut};

/// `D3D11CreateDevice`, as d3d11.h declares it.
type MakeDevice = unsafe extern "system" fn(Com, u32, isize, u32, *const u32, u32, u32, *mut Com, *mut u32, *mut Com) -> i32;
type Ask = unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32;

/// Driver types: hardware, and Direct3D's software device (WARP).
const HARDWARE: u32 = 1;
const WARP: u32 = 5;
/// The interfaces asked for: `IDXGIDevice1`, `IDXGIFactory2`, `ID3D11Texture2D`.
const DXGI_DEVICE: [u32; 4] = named(0x77DB_970F, 0x6276, 0x48BA, [0xBA, 0x28, 0x07, 0x01, 0x43, 0xB4, 0x39, 0x2C]);
const FACTORY: [u32; 4] = named(0x50C8_3A1C, 0xE072, 0x4C48, [0x87, 0xB0, 0x36, 0x30, 0xFA, 0x36, 0xA6, 0xD0]);
const TEXTURE: [u32; 4] = named(0x6F15_AAF2, 0xD208, 0x4E89, [0x9A, 0xB4, 0x48, 0x95, 0x35, 0xD3, 0x4F, 0x9C]);

/// `DXGI_SWAP_CHAIN_DESC1`.
#[repr(C)]
pub(crate) struct ChainDescription {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) stereo: i32,
    pub(crate) sample_count: u32,
    pub(crate) sample_quality: u32,
    pub(crate) usage: u32,
    pub(crate) buffers: u32,
    pub(crate) scaling: u32,
    pub(crate) effect: u32,
    pub(crate) alpha: u32,
    pub(crate) flags: u32,
}

/// `DXGI_PRESENT_PARAMETERS`: the changed rectangles.
#[repr(C)]
pub(crate) struct Changed {
    pub(crate) count: u32,
    pub(crate) rects: *const RECT,
    pub(crate) scroll: *const RECT,
    pub(crate) offset: *const c_void,
}

/// The Direct3D 11 device a program's windows are shown with, its immediate context, and the
/// DXGI factory that makes their swap chains.
pub(crate) struct Device {
    device: Com,
    context: Com,
    factory: Com,
    /// It is a GPU, not WARP.
    pub(crate) gpu: bool,
    /// What it draws windows with, where it does (see `gpu_draw`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) drawing: Option<crate::gpu_draw_windows::Pipeline>,
}

impl Device {
    /// A device on a GPU: not Microsoft's software adapter (a VM's or a remote session's).
    /// Nothing where none works.
    pub(crate) fn gpu() -> Option<Device> {
        #[cfg(test)]
        if crate::gpu_windows::tests::faked().software {
            let mut device = Device::made(WARP)?.drawing();
            device.gpu = true;
            return Some(device);
        }
        let device = Device::made(HARDWARE)?.drawing();
        (!device.vendor_is_microsoft()).then_some(device)
    }

    /// Direct3D's software device, WARP: the CPU, for windows that cannot go back to GDI.
    pub(crate) fn software() -> Option<Device> {
        #[cfg(test)]
        if crate::gpu_windows::tests::faked().no_warp {
            return None;
        }
        Device::made(WARP)
    }

    /// A device of the driver type, with its DXGI factory; nothing where any part fails. Its
    /// frame queue is one frame long: a present waits for the one before to be shown.
    fn made(driver: u32) -> Option<Device> {
        let mut made = Device {
            device: null_mut(),
            context: null_mut(),
            factory: null_mut(),
            gpu: driver == HARDWARE,
            #[cfg(feature = "gpu-draw")]
            drawing: None,
        };
        let (mut level, mut dxgi, mut adapter) = (0, null_mut(), null_mut());
        let found = function_in(system_library("d3d11.dll"), c"D3D11CreateDevice");
        // SAFETY: `D3D11CreateDevice` (0x29: single-threaded, no driver threads, BGRA) and the
        // COM calls are as the headers declare them: QueryInterface (0); IDXGIDevice1's
        // GetAdapter (7) and SetMaximumFrameLatency (12); the adapter's GetParent (6). What is
        // got is released.
        unsafe {
            if found == 0 || std::mem::transmute::<usize, MakeDevice>(found)(null_mut(), driver, 0, 0x29, null(), 0, 7, &mut made.device, &mut level, &mut made.context) < 0 {
                return None;
            }
            if function::<Ask>(made.device, 0)(made.device, &DXGI_DEVICE, &mut dxgi) >= 0 {
                function::<unsafe extern "system" fn(Com, u32) -> i32>(dxgi, 12)(dxgi, 1);
                function::<unsafe extern "system" fn(Com, *mut Com) -> i32>(dxgi, 7)(dxgi, &mut adapter);
            }
            if !adapter.is_null() {
                function::<Ask>(adapter, 6)(adapter, &FACTORY, &mut made.factory);
            }
            [adapter, dxgi].into_iter().for_each(|object| release(object));
        }
        (!made.factory.is_null()).then_some(made)
    }

    /// Whether the device's adapter is Microsoft's (vendor 0x1414): the Basic Render Driver,
    /// The same device, made to draw windows too where the feature `gpu-draw` is on (see
    /// `gpu_draw`). Its shaders are compiled here, on the device's own thread.
    #[inline]
    #[cfg_attr(not(feature = "gpu-draw"), allow(unused_mut))]
    fn drawing(mut self) -> Device {
        // (Not `Device { drawing, ..self }`: that copies the COM pointers, then releases them
        // with `self`.)
        #[cfg(feature = "gpu-draw")]
        {
            self.drawing = crate::gpu_draw_windows::Pipeline::new(self.device);
        }
        self
    }

    /// which draws on the CPU.
    fn vendor_is_microsoft(&self) -> bool {
        let (mut dxgi, mut adapter, mut described) = (null_mut(), null_mut(), [0u64; 40]);
        // SAFETY: QueryInterface (0), GetAdapter (7), and the adapter's GetDesc1 (10) into room
        // for a `DXGI_ADAPTER_DESC1` (304 bytes), whose vendor follows its 256-byte name.
        unsafe {
            if function::<Ask>(self.device, 0)(self.device, &DXGI_DEVICE, &mut dxgi) >= 0 {
                function::<unsafe extern "system" fn(Com, *mut Com) -> i32>(dxgi, 7)(dxgi, &mut adapter);
            }
            if !adapter.is_null() {
                function::<unsafe extern "system" fn(Com, *mut u64) -> i32>(adapter, 10)(adapter, described.as_mut_ptr());
            }
            [adapter, dxgi].into_iter().for_each(|object| release(object));
        }
        described[32] as u32 == 0x1414
    }

    /// Whether the device is gone: removed, reset or hung (`GetDeviceRemovedReason`, 39).
    pub(crate) fn removed(&self) -> bool {
        // SAFETY: a plain query of a live device.
        unsafe { function::<unsafe extern "system" fn(Com) -> i32>(self.device, 39)(self.device) != 0 }
    }
}

// SAFETY: the device and what it holds belong to no thread, and are used by one at a time: made
// on a thread of their own, then handed whole to the windows' thread.
unsafe impl Send for Device {}

impl Drop for Device {
    /// Lets the context finish (`Flush`, 111), then releases all three: a window's next swap
    /// chain needs the last one gone.
    fn drop(&mut self) {
        #[cfg(feature = "gpu-draw")]
        drop(self.drawing.take());
        // SAFETY: each is held once, by this device.
        unsafe {
            if !self.context.is_null() {
                function::<unsafe extern "system" fn(Com)>(self.context, 111)(self.context);
            }
            [self.factory, self.context, self.device].into_iter().for_each(|object| release(object));
        }
    }
}

/// How a window's texture is bound: drawn into by the GPU (`D3D11_BIND_RENDER_TARGET`) where
/// the device draws windows, else only copied.
#[inline]
fn drawn(device: &Device) -> u32 {
    // (Read by shaders too where it may be presented in scRGB: `hdr_windows`.)
    let read = if cfg!(feature = "hdr") { 0x8 } else { 0 };
    #[cfg(feature = "gpu-draw")]
    if device.drawing.is_some() {
        return 0x20 | read;
    }
    let _ = device;
    read
}

/// A window's swap chain, and the texture holding all of the window's pixels.
pub(crate) struct Shown {
    chain: Com,
    /// Null until the window has a size.
    picture: Com,
    /// Width and height in pixels of both.
    size: [i32; 2],
    /// The view the GPU draws into the texture through, where its device draws (null if not).
    #[cfg(feature = "gpu-draw")]
    target: Com,
    /// Its HDR, and the pass to scRGB once tried (see `hdr_windows`).
    #[cfg(feature = "hdr")]
    pub(crate) hdr: crate::hdr_windows::Hdr,
    #[cfg(feature = "hdr")]
    pass: Option<Option<crate::hdr_windows::Pass11>>,
}

impl Shown {
    /// A swap chain for window `hwnd` on `device`, `size` pixels: two buffers, no stretching,
    /// the alpha ignored. Nothing where DXGI makes none.
    pub(crate) fn new(device: &Device, hwnd: HANDLE, size: [i32; 2]) -> Option<Shown> {
        let [width, height] = size.map(|side| side.max(1) as u32);
        let described = ChainDescription { width, height, format: B8G8R8A8_UNORM, stereo: 0, sample_count: 1, sample_quality: 0, usage: 0x20, buffers: 2, scaling: 1, effect: 3, alpha: 3, flags: 0 };
        let mut shown = Shown {
            chain: null_mut(),
            picture: null_mut(),
            size: [0; 2],
            #[cfg(feature = "gpu-draw")]
            target: null_mut(),
            #[cfg(feature = "hdr")]
            hdr: Default::default(),
            #[cfg(feature = "hdr")]
            pass: None,
        };
        type MakeChain = unsafe extern "system" fn(Com, Com, HANDLE, *const ChainDescription, *const c_void, Com, *mut Com) -> i32;
        // SAFETY: the factory's CreateSwapChainForHwnd (15) for a live window; its
        // MakeWindowAssociation (8) with 3 (`DXGI_MWA_NO_WINDOW_CHANGES | DXGI_MWA_NO_ALT_ENTER`):
        // DXGI keeps out of the window's messages.
        unsafe {
            if function::<MakeChain>(device.factory, 15)(device.factory, device.device, hwnd, &described, null(), null_mut(), &mut shown.chain) < 0 {
                return None;
            }
            function::<unsafe extern "system" fn(Com, HANDLE, u32) -> i32>(device.factory, 8)(device.factory, hwnd, 3);
        }
        Some(shown)
    }

    /// Makes the buffers and the texture `size` pixels where they are not. Says whether they were
    /// remade (their pixels are then undefined: the window is to be drawn whole); nothing if that
    /// failed.
    pub(crate) fn fit(&mut self, device: &Device, size: [i32; 2]) -> Option<bool> {
        if size == self.size || size[0] <= 0 || size[1] <= 0 {
            return Some(false);
        }
        let [width, height] = size.map(|side| side as u32);
        let described = TextureDescription {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format: B8G8R8A8_UNORM,
            sample_count: 1,
            sample_quality: 0,
            usage: 0,
            bind_flags: drawn(device),
            cpu_access_flags: 0,
            misc_flags: 0,
        };
        type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32;
        self.size = [0; 2];
        // SAFETY: the old texture is held here alone; the swap chain's ResizeBuffers (13) while
        // nothing holds its buffers; the device's CreateTexture2D (5).
        unsafe {
            #[cfg(feature = "gpu-draw")]
            release(std::mem::replace(&mut self.target, null_mut()));
            release(std::mem::replace(&mut self.picture, null_mut()));
            if function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32) -> i32>(self.chain, 13)(self.chain, 0, width, height, 0, 0) < 0 {
                return None;
            }
            if function::<MakeTexture>(device.device, 5)(device.device, &described, null(), &mut self.picture) < 0 {
                return None;
            }
        }
        #[cfg(feature = "gpu-draw")]
        if device.drawing.is_some() {
            self.target = crate::gpu_draw_windows::Pipeline::target(device.device, self.picture);
        }
        self.size = size;
        Some(true)
    }

    /// Puts `size` pixels into the texture at `at` (cut to the texture), read from `first`, rows
    /// `pitch` pixels apart.
    ///
    /// # Safety
    /// The pixels read lie at `first`: as many rows as given, each as wide.
    pub(crate) unsafe fn upload(&self, device: &Device, at: [i32; 2], size: [i32; 2], first: *const u32, pitch: usize) {
        let [right, bottom] = [(at[0] + size[0]).min(self.size[0]), (at[1] + size[1]).min(self.size[1])];
        if self.picture.is_null() || at[0] < 0 || at[1] < 0 || right <= at[0] || bottom <= at[1] {
            return;
        }
        let place = Box3 { left: at[0] as u32, top: at[1] as u32, front: 0, right: right as u32, bottom: bottom as u32, back: 1 };
        type Copy = unsafe extern "system" fn(Com, Com, u32, *const Box3, *const u32, u32, u32);
        // SAFETY: the box lies in the texture; the rows read lie where the caller promises. (The
        // context's UpdateSubresource, 48.)
        unsafe { function::<Copy>(device.context, 48)(device.context, self.picture, 0, &place, first, (pitch * 4) as u32, 0) };
    }

    /// Draws a recorded paint into the texture, inside `area` (left, top, right, bottom). Says
    /// whether it could: if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn draw(&mut self, device: &mut Device, list: &crate::gpu_draw::DrawList, area: [i32; 4]) -> bool {
        let Some(drawing) = device.drawing.as_mut() else { return false };
        drawing.draw(device.device, device.context, self.target, list, area, self.size)
    }

    /// Copies the texture to the back buffer and presents it, `dirty` being what changed, at the
    /// screen's next refresh. Says whether DXGI took it.
    pub(crate) fn present(&mut self, device: &Device, dirty: &[RECT]) -> bool {
        if self.picture.is_null() {
            return true;
        }
        #[cfg(feature = "hdr")]
        if !self.go_hdr(device) {
            return false;
        }
        let dirty = inside(dirty, self.size);
        let (changed, mut back) = (Changed { count: dirty.len() as u32, rects: dirty.as_ptr(), scroll: null(), offset: null() }, null_mut());
        // SAFETY: the swap chain's GetBuffer (9) gives its back buffer, released after the
        // context's CopyResource (47) between textures of one size and format; Present1 (22),
        // in step with the screen (1), with rectangles inside the buffer.
        let presented = unsafe {
            if function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(self.chain, 9)(self.chain, 0, &TEXTURE, &mut back) < 0 {
                return false;
            }
            #[cfg(feature = "hdr")]
            let lit = self.lit(device, back);
            #[cfg(not(feature = "hdr"))]
            let lit = false;
            if !lit {
                function::<unsafe extern "system" fn(Com, Com, Com)>(device.context, 47)(device.context, back, self.picture);
                #[cfg(test)]
                tests::keep(device, back, self.size, self.chain, dirty);
            }
            release(back);
            function::<unsafe extern "system" fn(Com, u32, u32, *const Changed) -> i32>(self.chain, 22)(self.chain, 1, 0, &changed)
        };
        presented >= 0
    }

    /// Puts the swap chain in scRGB where the window is lit on an HDR screen, its pass made first,
    /// and back where not. Says whether that worked.
    #[cfg(feature = "hdr")]
    fn go_hdr(&mut self, device: &Device) -> bool {
        if self.hdr.wanted() && self.pass.is_none() {
            self.pass = Some(crate::hdr_windows::Pass11::new(device.device));
        }
        self.hdr.switch(self.chain, self.size, self.pass.as_ref().is_some_and(Option::is_some))
    }

    /// Draws the texture into `back` in scRGB, where the swap chain is. Says whether it did: if
    /// not, the texture is copied.
    #[cfg(feature = "hdr")]
    fn lit(&mut self, device: &Device, back: Com) -> bool {
        let Some(Some(pass)) = self.pass.as_mut().filter(|_| self.hdr.on) else { return false };
        let drawn = pass.draw([device.device, device.context], &mut self.hdr, self.picture, back, self.size);
        #[cfg(test)]
        crate::hdr_windows::tests::keep(device.device, device.context, back, self.size);
        drawn
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        // SAFETY: each is held once, by this window.
        unsafe { [self.picture, self.chain].into_iter().for_each(|object| release(object)) };
        #[cfg(feature = "gpu-draw")]
        // SAFETY: held once, by this window.
        unsafe {
            release(self.target)
        };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::layer_d3d11::Pixels;

    /// Keeps a copy of a back buffer about to be presented, if a test asked: through a staging
    /// texture (`D3D11_USAGE_STAGING`, 3; CPU reads, 0x20000), mapped for reading (1).
    pub(crate) fn keep(device: &Device, back: Com, size: [i32; 2], chain: Com, dirty: &[RECT]) {
        if !crate::gpu_windows::tests::faked().keep {
            return;
        }
        // The swap chain's window (GetHwnd, 20).
        let mut hwnd = 0;
        unsafe { function::<unsafe extern "system" fn(Com, *mut HANDLE) -> i32>(chain, 20)(chain, &mut hwnd) };
        let [width, height] = size.map(|side| side as u32);
        let described = TextureDescription {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format: B8G8R8A8_UNORM,
            sample_count: 1,
            sample_quality: 0,
            usage: 3,
            bind_flags: 0,
            cpu_access_flags: 0x20000,
            misc_flags: 0,
        };
        let (mut staging, mut mapped) = (null_mut(), Pixels { first: null(), row_pitch: 0, slice_pitch: 0 });
        type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32;
        unsafe {
            assert!(function::<MakeTexture>(device.device, 5)(device.device, &described, null(), &mut staging) >= 0);
            function::<unsafe extern "system" fn(Com, Com, Com)>(device.context, 47)(device.context, staging, back);
            assert!(function::<unsafe extern "system" fn(Com, Com, u32, u32, u32, *mut Pixels) -> i32>(device.context, 14)(device.context, staging, 0, 1, 0, &mut mapped) >= 0);
            let rows = (0..height as usize).map(|row| std::slice::from_raw_parts(mapped.first.cast::<u8>().add(row * mapped.row_pitch as usize).cast::<u32>(), width as usize));
            let pixels = rows.flatten().copied().collect();
            function::<unsafe extern "system" fn(Com, Com, u32)>(device.context, 15)(device.context, staging, 0);
            release(staging);
            crate::gpu_windows::tests::kept(hwnd, width as usize, pixels, dirty);
        }
    }
}
