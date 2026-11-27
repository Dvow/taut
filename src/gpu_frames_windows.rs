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
//! A frame is opened on a device the first time it is drawn there and let go of once no video
//! holds it. Where one can't be opened (another adapter's), the device takes no frames from then
//! on: their parts are drawn by the CPU, the frames read back. Each path waits for its drawing
//! before the next frame is put in.
use crate::gpu_d3d11::TEXTURE;
use crate::layer_d3d11::{Com, function, release};
use crate::video::FrameTexture;
use crate::video_windows::{A_KEYED_MUTEX, FRAME_WAIT};
use crate::windows::named;
use std::ptr::null_mut;
use std::sync::{Arc, Weak};

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

/// Videos' frames opened on a device, as `T`.
pub(crate) struct OpenedFrames<T> {
    opened: Vec<(Weak<FrameTexture>, T)>,
    /// A frame could not be opened here.
    pub(crate) refused: bool,
}

impl<T> Default for OpenedFrames<T> {
    #[inline]
    fn default() -> Self {
        OpenedFrames { opened: Vec::new(), refused: false }
    }
}

impl<T> OpenedFrames<T> {
    /// What `frames` are opened as, in their order: each opened by `open` the first time it is
    /// drawn. Nothing where one can't be (it is then `refused`). Frames no video holds are let go
    /// of by `let_go` first.
    pub(crate) fn opened(&mut self, frames: &[Arc<FrameTexture>], mut open: impl FnMut(&FrameTexture) -> Option<T>, mut let_go: impl FnMut(T)) -> Option<Vec<&T>> {
        self.opened.extract_if(.., |(frame, _)| frame.strong_count() == 0).for_each(|(_, gone)| let_go(gone));
        let mut places = Vec::with_capacity(frames.len());
        for frame in frames {
            let found = self.opened.iter().position(|(kept, _)| std::ptr::eq(kept.as_ptr(), Arc::as_ptr(frame)));
            if let Some(at) = found {
                places.push(at);
                continue;
            }
            let Some(made) = open(frame) else {
                self.refused = true;
                return None;
            };
            self.opened.push((Arc::downgrade(frame), made));
            places.push(self.opened.len() - 1);
        }
        Some(places.into_iter().map(|at| &self.opened[at].1).collect())
    }

    /// Lets go of every frame opened, by `let_go`.
    pub(crate) fn let_go_of_all(&mut self, let_go: impl FnMut(T)) {
        self.opened.drain(..).map(|(_, opened)| opened).for_each(let_go);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_opened_once_and_let_go_of_when_no_video_holds_it() {
        let frames: Vec<Arc<FrameTexture>> = (0..2).map(|_| Arc::new(FrameTexture::stand_in())).collect();
        let (mut opened, mut made, mut gone) = (OpenedFrames::default(), 0, Vec::new());
        for _ in 0..3 {
            let got = opened.opened(&frames, |_| Some(made).inspect(|_| made += 1), |id| gone.push(id)).expect("opened");
            assert_eq!(got, [&0, &1]);
        }
        assert_eq!(made, 2, "each opened once");
        let kept = frames[1].clone();
        drop(frames);
        assert_eq!(opened.opened(&[kept], |_| None, |id| gone.push(id)), Some(vec![&1]));
        assert_eq!(gone, [0], "the frame no video holds");
        opened.let_go_of_all(|id| gone.push(id));
        assert_eq!(gone, [0, 1]);
    }

    #[test]
    fn a_frame_that_cant_be_opened_refuses_the_device() {
        let frames = [Arc::new(FrameTexture::stand_in())];
        let mut opened = OpenedFrames::<u32>::default();
        assert!(opened.opened(&frames, |_| None, drop).is_none());
        assert!(opened.refused);
    }
}
