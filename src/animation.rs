//! Animations: the frames of a GIF, APNG or animated WebP composited on a canvas of their own,
//! each shown as long as its file says. Shows common animated images with no other library.
//!
//! All three formats play the same way. An animation has a canvas of fixed size, and frames,
//! each a block of pixels somewhere on it. A frame either replaces the pixels under it or is
//! blended over them by alpha. And it says what happens to it when the next comes: it stays,
//! its area is cleared, or the previous content is restored.
//!
//! An animation is kept as its file packed it, plus the canvas at one frame: the next frame is
//! decoded onto it when due. So 100 frames cost the file's memory and one image's, not a
//! hundred. Each frame is decoded once at load to check it: an animation ends before the first
//! damaged frame, and is none if that is the first.
//!
//! A frame delay of 10 ms or less shows for 100 ms, as everywhere: files saying so were made
//! when that meant "as fast as possible". Each format blends partly transparent frames with its
//! own arithmetic, matched here to its reference: libwebp's for WebP, and ffmpeg's for APNG (no
//! library from the format's authors composites frames).
use std::time::Duration;

/// An animation the UI keeps, to show wherever asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(alias("gif", "animation"))]
pub struct Animation(pub(crate) u32);

/// What happens to a frame when the next one comes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum After {
    Stays,
    /// Its area is cleared.
    Cleared,
    /// The previous content is restored.
    PutBack,
}

/// How a frame is laid on the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Laid {
    /// Its pixels replace those under it, transparent ones too.
    Instead,
    /// Its pixels are blended over those under it by alpha: APNG's way, or animated WebP's. (A
    /// GIF frame's pixels are opaque or transparent, so both give the same.)
    OverPng,
    OverWebp,
}

/// One frame, as kept.
#[derive(Clone, Copy)]
pub(crate) struct Frame {
    /// Its rect on the canvas: left, top, width, height.
    pub place: [usize; 4],
    /// Its packed pixels: a byte range in the kept bytes.
    pub packed: [usize; 2],
    /// How long it shows, in ms.
    pub shows: u64,
    pub after: After,
    pub laid: Laid,
    /// For a GIF frame: its palette range in the kept colours, its transparent index, whether
    /// its rows are interlaced, and its maximum code size. (The first codes are a bit longer.)
    pub colors: [usize; 2],
    pub clear: Option<u8>,
    pub passes: bool,
    pub bits: u32,
}

impl Frame {
    /// A frame at `place`, packed in bytes `packed[0]..packed[1]`, as APNG and animated WebP
    /// have them.
    pub(crate) fn new(place: [usize; 4], packed: [usize; 2], shows: u64, after: After, laid: Laid) -> Frame {
        Frame { place, packed, shows: shows_for(shows), after, laid, colors: [0; 2], clear: None, passes: false, bits: 0 }
    }
}

/// How long a frame shows whose file says `said` ms.
pub(crate) fn shows_for(said: u64) -> u64 {
    if said <= 10 { 100 } else { said }
}

/// An animation's format, and what is kept to decode its frames.
pub(crate) enum Kind {
    /// GIF: all its palettes, as pixels; and a buffer for a frame's indices.
    Gif(Vec<u32>, Vec<u8>),
    /// APNG: how its rows are stored.
    Png(Box<crate::png::Stored>),
    /// Animated WebP: and its colour transform for the screen, if any.
    Webp(Option<Box<crate::color_profiles::Change>>),
}

/// An animation as kept: the file's frames, still packed, and the canvas at one of them.
pub(crate) struct Reel {
    /// The canvas size in pixels.
    pub size: [usize; 2],
    pub frames: Vec<Frame>,
    pub kind: Kind,
    /// Every frame's packed pixels, in sequence.
    pub packed: Vec<u8>,
    /// How many times it plays; `None` for ever.
    pub plays: Option<u64>,
    /// The canvas, and which frame it shows: none at first.
    pixels: Vec<u32>,
    at: Option<usize>,
    /// The canvas before a frame that is to be undone.
    before: Vec<u32>,
    /// Whether the frame on the canvas was laid on a blank one (WebP's key frame).
    afresh: bool,
    /// When it started playing, by the UI's clock.
    pub(crate) since: Duration,
}

impl Reel {
    /// Reads an animation: GIF, PNG or WebP, by its signature; at most its first `most` frames.
    /// (A still image is a one-frame animation.) `None` if none of these, or no whole frame.
    pub(crate) fn read(file: &[u8], most: usize) -> Option<Reel> {
        match file {
            [b'G', b'I', b'F', ..] => crate::gif::read_animation(file, most),
            [b'R', b'I', b'F', b'F', ..] => crate::webp::read_animation(file, most),
            _ => crate::png::read_animation(file, most),
        }
    }

    /// An animation with a canvas of `size` and no frames yet, from a file of `kind`.
    pub(crate) fn new(size: [usize; 2], kind: Kind, plays: Option<u64>) -> Reel {
        Reel { size, frames: Vec::new(), kind, packed: Vec::new(), plays, pixels: Vec::new(), at: None, before: Vec::new(), afresh: false, since: Duration::ZERO }
    }

    /// The animation once its frames are in: `None` if it has none. Its canvas starts clear.
    pub(crate) fn done(mut self) -> Option<Reel> {
        self.pixels = vec![0; if self.frames.is_empty() { 0 } else { self.size[0] * self.size[1] }];
        (!self.frames.is_empty()).then_some(self)
    }

    /// The canvas size in pixels.
    pub(crate) fn size(&self) -> [usize; 2] {
        self.size
    }

    /// The frame due after playing for `played`, and the time until the next; `None` when no
    /// other frame comes any more.
    pub(crate) fn due(&self, played: Duration) -> (usize, Option<Duration>) {
        let (round, played) = (self.frames.iter().map(|frame| frame.shows).sum::<u64>(), played.as_millis() as u64);
        // One frame is a still image; one that has played as often as it should stays at its
        // last frame.
        if self.frames.len() < 2 || self.plays.is_some_and(|plays| played / round >= plays) {
            return (self.frames.len() - 1, None);
        }
        let (mut frame, mut left) = (0, played % round);
        while left >= self.frames[frame].shows {
            (frame, left) = (frame + 1, left - self.frames[frame].shows);
        }
        (frame, Some(Duration::from_millis(self.frames[frame].shows - left)))
    }

    /// The canvas while frame `frame` shows, and its width: rows of `0xAARRGGBB`, transparent
    /// where no frame drew. Frames from the current one up to it are laid on; all from the
    /// first if the canvas is already past it.
    pub(crate) fn shown(&mut self, frame: usize) -> (&[u32], usize) {
        let frame = frame.min(self.frames.len() - 1);
        if self.at.is_some_and(|at| at > frame) {
            self.pixels.fill(0);
            self.at = None;
        }
        while self.at != Some(frame) {
            self.lay(self.at.map_or(0, |at| at + 1));
        }
        (&self.pixels, self.size[0])
    }

    /// The canvas rows a frame at `place` covers, each as its pixel range. (Off-canvas parts
    /// are dropped.)
    fn rows(&self, [left, top, across, down]: [usize; 4]) -> impl Iterator<Item = std::ops::Range<usize>> + use<> {
        let [wide, high] = self.size;
        (top.min(high)..(top + down).min(high)).map(move |y| y * wide + left.min(wide)..y * wide + (left + across).min(wide))
    }

    /// Lays frame `next` on the canvas, which shows the frame before it.
    fn lay(&mut self, next: usize) {
        let frame = self.frames[next];
        let before = next.checked_sub(1).map(|before| self.frames[before]);
        before.into_iter().for_each(|before| self.dispose(before));
        // The current content is saved if this frame is to be undone.
        if frame.after == After::PutBack {
            self.before.clone_from(&self.pixels);
        }
        let (packed, size) = (&self.packed[frame.packed[0]..frame.packed[1]], [frame.place[2], frame.place[3]]);
        let pixels = match &mut self.kind {
            Kind::Gif(colors, numbers) => {
                crate::gif::lay(packed, &frame, colors, numbers, (&mut self.pixels, self.size));
                self.at = Some(next);
                return;
            }
            Kind::Png(stored) => stored.frame(size, packed),
            Kind::Webp(_) => crate::webp::frame(packed, size),
        };
        // It was decoded once already, at load: it is complete.
        let mut pixels = pixels.unwrap_or_default();
        if let Kind::Webp(Some(change)) = &self.kind {
            change.apply(&mut pixels);
        }
        self.blend(&frame, &pixels, before);
        self.at = Some(next);
    }

    /// Disposes of frame `before` as it said: its area cleared, or what was under it put back.
    fn dispose(&mut self, before: Frame) {
        for row in self.rows(before.place) {
            match before.after {
                After::Cleared => self.pixels[row].fill(0),
                After::PutBack => self.pixels[row.clone()].copy_from_slice(&self.before[row]),
                After::Stays => {}
            }
        }
    }

    /// Lays a frame's decoded `pixels` on the canvas as it says, `before` being the frame
    /// before it.
    fn blend(&mut self, frame: &Frame, pixels: &[u32], before: Option<Frame>) {
        let [left, top, across, down] = frame.place;
        // Animated WebP lays a frame on a blank canvas as is: the first, one covering the whole
        // canvas and replacing it, and one after a frame that was cleared and left nothing (it
        // covered all, or was itself laid on blank).
        let all = |place: [usize; 4]| place == [0, 0, self.size[0], self.size[1]];
        let cleared = before.filter(|before| before.after == After::Cleared);
        self.afresh = before.is_none() || (frame.laid == Laid::Instead && all(frame.place)) || cleared.is_some_and(|before| all(before.place) || self.afresh);
        let laid = if self.afresh && frame.laid == Laid::OverWebp { Laid::Instead } else { frame.laid };
        // And it does not blend in where the previous frame was cleared.
        let gone = cleared.filter(|_| laid == Laid::OverWebp).map_or([0; 4], |before| before.place);
        for (y, row) in pixels.chunks_exact(across.max(1)).enumerate().take(down) {
            let at = (top + y) * self.size[0] + left;
            let screen = &mut self.pixels[at..at + across];
            let blank = if (gone[1]..gone[1] + gone[3]).contains(&(top + y)) { [gone[0], gone[0] + gone[2]] } else { [0; 2] };
            match laid {
                Laid::Instead => screen.copy_from_slice(row),
                Laid::OverPng => screen.iter_mut().zip(row).for_each(|(under, top)| *under = over_png(*top, *under)),
                Laid::OverWebp => over_webp(screen, row, left, blank),
            }
        }
    }
}

/// Alpha-blends a pixel over another as ffmpeg does for APNG: integer arithmetic, dividing by
/// 255 rounding to nearest where the lower pixel is opaque, and down where not.
fn over_png(top: u32, under: u32) -> u32 {
    let (solid, under_solid) = (top >> 24, under >> 24);
    match solid {
        255 => return top,
        0 => return under,
        _ => {}
    }
    let by_255 = |x: u32| ((x + 128) * 257) >> 16;
    let all = solid + by_255((255 - solid) * under_solid);
    let part = |shift: u32| {
        let (mine, other) = (top >> shift & 255, under >> shift & 255);
        match under_solid {
            255 => by_255(solid * mine + (255 - solid) * other),
            _ => (255 * solid * mine + (255 - solid) * under_solid * other) / (255 * all),
        }
    };
    all << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// Alpha-blends a frame row over the canvas as libwebp does for animated WebP; but between
/// `blank[0]` and `blank[1]` (where the previous frame was cleared) it is copied as is. The row
/// starts at `left`.
fn over_webp(screen: &mut [u32], row: &[u32], left: usize, blank: [usize; 2]) {
    for (x, (under, top)) in screen.iter_mut().zip(row).enumerate() {
        let solid = top >> 24;
        *under = match solid {
            255 => *top,
            _ if (blank[0]..blank[1]).contains(&(left + x)) => *top,
            0 => *under,
            _ => {
                // How much of the lower pixel shows, and the combined alpha: each in 256ths, as
                // libwebp computes them.
                let showing = ((*under >> 24) * (256 - solid)) >> 8;
                let (all, part) = (solid + showing, |shift: u32| (top >> shift & 255) * solid + (*under >> shift & 255) * showing);
                let scale = (1 << 24) / all;
                all << 24 | ((part(16) * scale) >> 24) << 16 | ((part(8) * scale) >> 24) << 8 | (part(0) * scale) >> 24
            }
        };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every frame of an animation as the canvas stands while it shows, stacked vertically.
    pub(crate) fn frames(reel: &mut Reel) -> Vec<u32> {
        (0..reel.frames.len()).flat_map(|frame| reel.shown(frame).0.to_vec()).collect()
    }

    /// Pixels as they look: transparent is transparent, whatever colour it holds.
    pub(crate) fn seen(pixels: &[u32]) -> Vec<u32> {
        pixels.iter().map(|pixel| if pixel >> 24 == 0 { 0 } else { *pixel }).collect()
    }

    /// The test animations' frames: `step` of five, each 24 by 16. Colour bands, the lowest
    /// half transparent; a transparent corner; and a block moving left to right, opaque in its
    /// middle, less so around it.
    pub(crate) fn pattern(step: usize) -> Vec<u32> {
        let pixel = |at: usize| {
            let (x, y) = (at % 24, at / 24);
            let block = (4 * step + 2..4 * step + 8).contains(&x) && (5..11).contains(&y);
            let middle = (4 * step + 3..4 * step + 7).contains(&x) && (6..10).contains(&y);
            match () {
                _ if x + y < 5 => 0,
                _ if middle => 0xfff5_9e0b,
                _ if block => 0x60f5_9e0b,
                _ => [0xff1e_3a8a, 0xff0f_766e, 0xff7c_2d12, 0x80f1_f5f9][y / 4],
            }
        };
        (0..24 * 16).map(pixel).collect()
    }

    /// Whether a damaged animation is still playable: every frame shows, from the last back to
    /// the first, and fills the canvas.
    pub(crate) fn playable(reel: Option<Reel>) -> bool {
        let Some(mut reel) = reel else { return true };
        let screen = reel.size[0] * reel.size[1];
        (0..reel.frames.len()).rev().all(|frame| reel.shown(frame).0.len() == screen)
    }

    #[test]
    fn a_frame_of_a_hundredth_of_a_second_or_less_shows_for_a_tenth() {
        assert_eq!([0, 1, 10, 11, 20, 1000].map(shows_for), [100, 100, 100, 11, 20, 1000]);
    }

    #[test]
    fn what_is_less_than_solid_is_mixed_as_each_kind_mixes_it() {
        // Opaque over anything is itself, and transparent over anything changes nothing.
        for under in [0, 0x8012_3456, 0xff65_4321] {
            assert_eq!([over_png(0xff11_2233, under), over_png(0x0011_2233, under)], [0xff11_2233, under]);
        }
        // Half over opaque: the midpoint, rounded to nearest; over half: more opaque.
        assert_eq!(over_png(0x80ff_0000, 0xff00_00ff), 0xff80_007f);
        assert_eq!(over_png(0x80ff_0000, 0x8000_00ff), 0xc0aa_0054);
        // libwebp: half over opaque, in 256ths rounded down; over a transparent pixel, the
        // colour as is; over half.
        let mut screen = [0xff00_00ff, 0x0000_0000, 0x8000_00ff];
        over_webp(&mut screen, &[0x80ff_0000; 3], 0, [0, 0]);
        assert_eq!(screen, [0xff7f_007e, 0x80ff_0000, 0xc0a9_0054]);
        // Where the previous frame was cleared, the frame's pixels are copied as is.
        let mut screen = [0xff00_00ff; 4];
        over_webp(&mut screen, &[0x80ff_0000; 4], 5, [6, 8]);
        assert_eq!(screen, [0xff7f_007e, 0x80ff_0000, 0x80ff_0000, 0xff7f_007e]);
    }
}
