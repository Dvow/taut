//! Videos' frames imported on a GPU device (`video` with `gpu-draw`): the rule every path keeps
//! them by (Direct3D 11 and 12, Vulkan, OpenGL, OpenGL ES).
//!
//! - A frame's buffer is imported the first time a device draws it, and found again by its key:
//!   decoders hand out a few buffers again and again.
//! - An import is kept while its frame's source lives (the decoder's texture on Windows, its
//!   image reader on Android, its player on Linux), the `KEPT` most recently drawn at most.
//! - Where a frame can't be imported, the device refuses frames: from then on its parts draw
//!   them read back (`DrawList::takes_frames`).
use std::sync::{Arc, Weak};

/// Imports kept at most on a device.
const KEPT: usize = 8;

/// What the rule needs of a frame (`video::FrameTexture` on each system).
pub(crate) trait Frame {
    /// What tells its buffer apart from the decoder's others; none where nothing can.
    fn key(&self) -> Option<u64>;
    /// What lives while its buffer may be handed out again.
    fn source(this: &Arc<Self>) -> Weak<dyn Send + Sync>;
}

/// An import, the key of its buffer, and its frame's source.
struct Kept<T> {
    key: u64,
    source: Weak<dyn Send + Sync>,
    import: T,
}

/// Videos' frames imported on a device, as `T`: the least recently drawn first; the places of
/// the frames of the part readied last, in its order; whether one could not be imported.
pub(crate) struct ImportedFrames<T> {
    kept: Vec<Kept<T>>,
    drawn: Vec<usize>,
    refused: bool,
}

impl<T> Default for ImportedFrames<T> {
    #[inline]
    fn default() -> Self {
        ImportedFrames { kept: Vec::new(), drawn: Vec::new(), refused: false }
    }
}

impl<T> ImportedFrames<T> {
    /// Readies `frames` (a part's): each found among those kept, or imported by `import` now.
    /// Imports whose source is gone, and the least recently drawn beyond `KEPT`, are let go of
    /// by `let_go`. Whether all are ready; where one can't be imported, the device refuses
    /// frames.
    pub(crate) fn ready<F: Frame>(&mut self, frames: &[Arc<F>], mut import: impl FnMut(&F) -> Option<T>, mut let_go: impl FnMut(T)) -> bool {
        self.drawn.clear();
        self.kept.extract_if(.., |kept| kept.source.strong_count() == 0).for_each(|gone| let_go(gone.import));
        let mut keys = Vec::with_capacity(frames.len());
        for frame in frames {
            let Some(key) = frame.key() else { return self.refuse() };
            let found = self.kept.iter().position(|kept| kept.key == key).map(|at| self.kept.remove(at));
            let Some(kept) = found.or_else(|| import(frame).map(|import| Kept { key, source: F::source(frame), import })) else { return self.refuse() };
            self.kept.push(kept);
            keys.push(key);
        }
        let beyond = self.kept.len().saturating_sub(KEPT.max(frames.len()));
        self.kept.drain(..beyond).for_each(|gone| let_go(gone.import));
        self.drawn.extend(keys.iter().filter_map(|key| self.kept.iter().position(|kept| kept.key == *key)));
        true
    }

    /// The imports of the part readied last, in its frames' order.
    pub(crate) fn drawn(&self) -> impl Iterator<Item = &T> {
        self.drawn.iter().map(|at| &self.kept[*at].import)
    }

    /// Notes that frames can't be drawn on this device. Gives false.
    pub(crate) fn refuse(&mut self) -> bool {
        self.refused = true;
        false
    }

    /// Whether frames can't be drawn on this device.
    pub(crate) fn refused(&self) -> bool {
        self.refused
    }

    /// Lets go of every import, by `let_go`.
    pub(crate) fn let_go_of_all(&mut self, let_go: impl FnMut(T)) {
        self.drawn.clear();
        self.kept.drain(..).map(|kept| kept.import).for_each(let_go);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of a buffer: its key, and the source handing it out.
    struct Buffer {
        key: Option<u64>,
        source: Arc<()>,
    }

    impl Frame for Buffer {
        fn key(&self) -> Option<u64> {
            self.key
        }
        fn source(this: &Arc<Self>) -> Weak<dyn Send + Sync> {
            Arc::downgrade(&this.source) as Weak<dyn Send + Sync>
        }
    }

    /// Frames of buffers 1 to `count`, each of a source of its own.
    fn frames(count: u64) -> Vec<Arc<Buffer>> {
        (1..=count).map(|key| Arc::new(Buffer { key: Some(key), source: Arc::new(()) })).collect()
    }

    /// Readies `frames` in `imported`, each imported as its key (noted in `made`), those let go
    /// of noted in `gone`: the imports drawn, or none.
    fn readied(imported: &mut ImportedFrames<u64>, frames: &[Arc<Buffer>], made: &mut Vec<u64>, gone: &mut Vec<u64>) -> Option<Vec<u64>> {
        let ready = imported.ready(frames, |frame| frame.key.inspect(|key| made.push(*key)), |key| gone.push(key));
        ready.then(|| imported.drawn().copied().collect())
    }

    #[test]
    fn a_buffer_is_imported_once_and_found_again_by_its_key() {
        let (buffers, mut imported, mut made, mut gone) = (frames(2), ImportedFrames::default(), Vec::new(), Vec::new());
        for _ in 0..3 {
            assert_eq!(readied(&mut imported, &buffers, &mut made, &mut gone), Some(vec![1, 2]));
        }
        let again = Arc::new(Buffer { key: Some(2), source: buffers[1].source.clone() });
        assert_eq!(readied(&mut imported, &[again], &mut made, &mut gone), Some(vec![2]), "a later frame of the same buffer");
        assert_eq!((made, gone), (vec![1, 2], vec![]));
    }

    #[test]
    fn an_import_whose_source_is_gone_is_let_go_of() {
        let (mut buffers, mut imported, mut made, mut gone) = (frames(2), ImportedFrames::default(), Vec::new(), Vec::new());
        readied(&mut imported, &buffers, &mut made, &mut gone);
        buffers.remove(0);
        assert_eq!(readied(&mut imported, &[], &mut made, &mut gone), Some(vec![]));
        assert_eq!(gone, [1], "let go of with nothing drawn");
        imported.let_go_of_all(|key| gone.push(key));
        assert_eq!(gone, [1, 2]);
    }

    #[test]
    fn beyond_eight_the_least_recently_drawn_is_let_go_of() {
        let (buffers, mut imported, mut made, mut gone) = (frames(10), ImportedFrames::default(), Vec::new(), Vec::new());
        for frame in &buffers[..8] {
            readied(&mut imported, std::slice::from_ref(frame), &mut made, &mut gone);
        }
        readied(&mut imported, &buffers[..1], &mut made, &mut gone);
        assert_eq!(readied(&mut imported, &buffers[8..], &mut made, &mut gone), Some(vec![9, 10]));
        assert_eq!(gone, [2, 3], "buffer 1 was drawn again since");
    }

    #[test]
    fn a_frame_that_cant_be_imported_refuses_the_device() {
        let mut imported = ImportedFrames::<u64>::default();
        assert!(!imported.ready(&frames(1), |_| None, drop));
        assert!(imported.refused());
    }

    #[test]
    fn a_frame_of_no_key_refuses_the_device() {
        let mut imported = ImportedFrames::<u64>::default();
        assert!(!imported.ready(&[Arc::new(Buffer { key: None, source: Arc::new(()) })], |_| Some(0), drop));
        assert!(imported.refused());
    }
}
