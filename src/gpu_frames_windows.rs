//! Videos' frames drawn from the decoder's own texture by Windows' GPU paths (the features
//! `gpu-draw` and `video`): what the paths share. The texture is the decoder's Direct3D 11 one
//! (`video_windows::FrameTexture`), shared by an NT handle:
//!
//! | Path | Opens it by | Waits for the decoder by |
//! | --- | --- | --- |
//! | Direct3D 11 | `OpenSharedResource1` | its keyed mutex, on the CPU |
//! | Direct3D 12 | `OpenSharedHandle` | the decoder's fence, on its queue |
//! | Vulkan | `VK_KHR_external_memory_win32` (`video_vulkan`) | its keyed mutex, in the submit (`VK_KHR_win32_keyed_mutex`) |
//! | OpenGL | `WGL_NV_DX_interop2`, through a Direct3D 11 device of its own | its keyed mutex, on the CPU |
//!
//! Opened frames are kept as `gpu_frames` says; a device that can't open one (another adapter's)
//! refuses frames. Each path waits for its drawing before the next frame is put in; the decoder
//! never waits for a texture a device holds (`video_windows::Frames::transfer`).
use crate::gpu_d3d11::TEXTURE;
use crate::layer_d3d11::{Com, function, release};
use crate::video::FrameTexture;
use crate::video_windows::{A_KEYED_MUTEX, FRAME_WAIT};
use crate::windows::named;
use std::ptr::null_mut;

/// `ID3D11Device1`.
const A_DEVICE1: [u32; 4] = named(0xA04B_FB29, 0x08EF, 0x43D6, [0xA4, 0x9C, 0xA9, 0xBD, 0xBD, 0xCB, 0xE6, 0x86]);

/// `frame`'s texture opened on Direct3D 11 `device` by its NT handle
/// (`ID3D11Device1::OpenSharedResource1`, 48), and its keyed mutex; nothing where it can't be
/// (another adapter's).
pub(crate) fn opened_on_direct3d_11(device: Com, frame: &FrameTexture) -> Option<[Com; 2]> {
    let (mut device1, mut texture, mut mutex) = (null_mut(), null_mut(), null_mut());
    if frame.handle == 0 {
        return None;
    }
    // SAFETY: the device is alive; the handle is the live frame texture's. What is given is
    // held by the caller, or released here where the rest is missing.
    unsafe {
        function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(device, 0)(device, &A_DEVICE1, &mut device1);
        if device1.is_null() {
            return None;
        }
        function::<unsafe extern "system" fn(Com, isize, *const [u32; 4], *mut Com) -> i32>(device1, 48)(device1, frame.handle, &TEXTURE, &mut texture);
        release(device1);
        if texture.is_null() {
            return None;
        }
        function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(texture, 0)(texture, &A_KEYED_MUTEX, &mut mutex);
        if mutex.is_null() {
            release(texture);
            return None;
        }
    }
    Some([texture, mutex])
}

/// Takes each keyed mutex for this device (`IDXGIKeyedMutex::AcquireSync`, 8, with key 0,
/// within `FRAME_WAIT`): the decoder's work on the frame is done first, and its next waits until
/// they are given back. Says whether all were; where one was not, those taken are given back.
pub(crate) fn take_all(mutexes: &[Com]) -> bool {
    for (at, mutex) in mutexes.iter().enumerate() {
        // SAFETY: the mutex is alive. Only `S_OK` holds it: a time-out and an abandoned mutex
        // are success codes.
        if unsafe { function::<unsafe extern "system" fn(Com, u64, u32) -> i32>(*mutex, 8)(*mutex, 0, FRAME_WAIT) } != 0 {
            give_back(&mutexes[..at]);
            return false;
        }
    }
    true
}

/// Gives back keyed mutexes taken (`ReleaseSync`, 9, key 0).
pub(crate) fn give_back(mutexes: &[Com]) {
    // SAFETY: each mutex is alive and was taken with key 0.
    unsafe { mutexes.iter().for_each(|mutex| _ = function::<unsafe extern "system" fn(Com, u64) -> i32>(*mutex, 9)(*mutex, 0)) };
}
