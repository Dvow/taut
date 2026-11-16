//! GIF decoding: still images, and animations as frames (see `film`) with their delays.
//!
//! A GIF has a canvas, and one or more frames. A frame is a block of pixels on the canvas, each
//! an index into a palette of up to 256 colours, one of which may be transparent. Indices are
//! LZW-compressed. A frame is laid over what earlier frames left, and says how it is disposed
//! of when the next comes: kept, cleared, or the previous content restored.
//!
//! Read: GIF87a and GIF89a, global and local palettes, interlacing, transparency, all three
//! disposal methods, delays, and loop count. Not read: plain text extensions and comments
//! (nothing writes the first), and the user input flag. A file is refused if its canvas is
//! larger than its bytes could fill; an animation ends before the first damaged frame.
use crate::film::{After, Frame, Kind, Laid, Reel};

/// The most pixels one byte of LZW data can expand to: a 12-bit code for a run as long as there
/// are codes.
const MOST: usize = 2730;
/// The number of codes, and so the longest run.
const CODES: usize = 4096;

/// Decodes a GIF's first frame as an image. Returns its width in pixels and its pixels in rows
/// from the top, `0xAARRGGBB` (transparent where the file has nothing): what
/// [`Ui::add_picture`](crate::Ui::add_picture) takes. `None` for a damaged file or not a GIF.
pub fn decode(gif: &[u8]) -> Option<(usize, Vec<u32>)> {
    let mut reel = film(gif, 1)?;
    let (pixels, width) = reel.shown(0);
    Some((width, pixels.to_vec()))
}

/// What the file says of the next frame beforehand (its graphic control extension): the
/// transparent index, disposal, and delay (0 if none).
#[derive(Clone, Copy, Default)]
struct Said {
    clear: Option<u8>,
    after: u8,
    shows: u64,
}

/// Reads a GIF as an animation: at most its first `most` frames. `None` if not a GIF, or no
/// whole frame.
pub(crate) fn film(gif: &[u8], most: usize) -> Option<Reel> {
    let head = gif.first_chunk::<13>().filter(|head| head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"))?;
    let size = [u16::from_le_bytes([head[6], head[7]]) as usize, u16::from_le_bytes([head[8], head[9]]) as usize];
    // An empty canvas is no image; one the whole file could not fill gets no memory.
    if size[0] * size[1] == 0 || size[0] * size[1] > gif.len().saturating_mul(MOST) {
        return None;
    }
    let mut reel = Reel::new(size, Kind::Gif(Vec::new(), Vec::new()), Some(1));
    let mut rest = &gif[13..];
    let (shared, mut said) = (list(&mut reel, &mut rest, head[10])?, Said::default());
    // The file is a sequence of blocks: a frame, an extension about the next frame or the file,
    // or the trailer. (The file ends before anything truncated or damaged.)
    while let Some((kind, after)) = rest.split_first().filter(|_| reel.frames.len() < most) {
        rest = after;
        let sound = match kind {
            0x2C => frame(&mut reel, &mut rest, shared, &mut said),
            0x21 => note(&mut reel, &mut rest, &mut said),
            _ => None,
        };
        if sound.is_none() {
            break;
        }
    }
    reel.done()
}

/// A GIF animation's palettes and index buffer.
fn kept(kind: &mut Kind) -> (&mut Vec<u32>, &mut Vec<u8>) {
    match kind {
        Kind::Gif(colors, numbers) => (colors, numbers),
        // Only GIF animations are ever read here.
        _ => unreachable!(),
    }
}

/// Takes a palette, if `flags` says one follows: 2^(low three bits + 1) colours of three bytes.
/// Returns its range in the kept colours.
fn list(reel: &mut Reel, rest: &mut &[u8], flags: u8) -> Option<[usize; 2]> {
    let colors = kept(&mut reel.kind).0;
    let (first, count) = (colors.len(), if flags & 0x80 == 0 { 0 } else { 2 << (flags & 7) });
    let (list, after) = rest.split_at_checked(count * 3)?;
    colors.extend(list.as_chunks::<3>().0.iter().map(|[red, green, blue]| u32::from_be_bytes([255, *red, *green, *blue])));
    *rest = after;
    Some([first, colors.len()])
}

/// Takes an extension between frames: a label byte, then data sub-blocks. For the next frame:
/// transparent index, disposal, delay. For the file: the loop count.
fn note(reel: &mut Reel, rest: &mut &[u8], said: &mut Said) -> Option<()> {
    let (kind, after) = rest.split_first()?;
    let mut note = Vec::new();
    *rest = after;
    blocks(rest, &mut note)?;
    match (kind, &note[..]) {
        (0xF9, [flags, low, high, clear, ..]) => {
            let hundredths = u16::from_le_bytes([*low, *high]) as u64;
            *said = Said { clear: (flags & 1 == 1).then_some(*clear), after: flags >> 2 & 7, shows: hundredths * 10 };
        }
        // Once, and so many times again; none means for ever.
        (0xFF, [name @ .., 1, low, high]) if name == b"NETSCAPE2.0" || name == b"ANIMEXTS1.0" => {
            let again = u16::from_le_bytes([*low, *high]) as u64;
            reel.plays = (again > 0).then_some(again + 1);
        }
        _ => {}
    }
    Some(())
}

/// Takes a frame: its rect, its own palette if any (else the file's, `shared`), and its packed
/// pixels, decoded once to check they are complete. It takes `said`, which then holds for no
/// other frame. `None` for a truncated or damaged frame.
fn frame(reel: &mut Reel, rest: &mut &[u8], shared: [usize; 2], said: &mut Said) -> Option<()> {
    let (head, after) = rest.split_first_chunk::<9>()?;
    *rest = after;
    let place = [0, 2, 4, 6].map(|at| u16::from_le_bytes([head[at], head[at + 1]]) as usize);
    let (own, first) = (list(reel, rest, head[8])?, reel.packed.len());
    let (bits, after) = rest.split_first()?;
    *rest = after;
    blocks(rest, &mut reel.packed)?;
    let Said { clear, after, shows } = std::mem::take(said);
    let after = match after {
        2 => After::Cleared,
        3 => After::PutBack,
        _ => After::Stays,
    };
    let mut frame = Frame::new(place, [first, reel.packed.len()], shows, after, Laid::OverPng);
    (frame.colors, frame.clear, frame.passes, frame.bits) = (if own[0] == own[1] { shared } else { own }, clear, head[8] & 0x40 != 0, *bits as u32);
    // A frame its bytes could not fill gets no memory either.
    let count = place[2] * place[3];
    if count == 0 || count > (reel.packed.len() - first).saturating_mul(MOST) || !(1..=8).contains(&frame.bits) {
        return None;
    }
    unpack(&reel.packed[first..], frame.bits, kept(&mut reel.kind).1, count)?;
    reel.frames.push(frame);
    Some(())
}

/// Lays a GIF frame on its animation's canvas: `packed` are its LZW data, `colors` all the
/// file's palettes, and `numbers` the index buffer. The canvas is `size` pixels, showing the
/// previous frame, disposed of for this one.
pub(crate) fn lay(packed: &[u8], frame: &Frame, colors: &[u32], numbers: &mut Vec<u8>, (pixels, [wide, high]): (&mut [u32], [usize; 2])) {
    let [left, top, across, down] = frame.place;
    // It was decoded once already, at load: it is complete.
    let _ = unpack(packed, frame.bits, numbers, across * down);
    // Rows in storage order: sequential, or four interlace passes.
    let passes = if frame.passes { [(0, 8), (4, 8), (2, 4), (1, 2)] } else { [(0, 1), (down, 1), (down, 1), (down, 1)] };
    let lines = passes.iter().flat_map(|(first, step)| (*first..down).step_by(*step));
    let colors = &colors[frame.colors[0]..frame.colors[1]];
    for (line, numbers) in lines.zip(numbers.chunks_exact(across)).filter(|(line, _)| top + line < high) {
        let row = &mut pixels[(top + line) * wide + left.min(wide)..(top + line) * wide + (left + across).min(wide)];
        // An index past the palette's end is black: palettes may be short.
        let seen = row.iter_mut().zip(numbers).filter(|(_, number)| Some(**number) != frame.clear);
        seen.for_each(|(pixel, number)| *pixel = colors.get(*number as usize).copied().unwrap_or(0xff00_0000));
    }
}

/// Reads a run of data sub-blocks (a length byte, then that many bytes), up to the empty block
/// ending them, appending their bytes to `into`. `None` if the file ends first.
fn blocks(rest: &mut &[u8], into: &mut Vec<u8>) -> Option<()> {
    loop {
        let (long, after) = rest.split_first()?;
        let (block, after) = after.split_at_checked(*long as usize)?;
        into.extend_from_slice(block);
        *rest = after;
        if *long == 0 {
            return Some(());
        }
    }
}

/// Decodes a frame's LZW data into `count` indices in `numbers`. A code of `bits + 1` bits or
/// more is a literal index, clear, end, or a run seen before: the previous code's run plus one
/// index, which every code read adds to the table. `None` if the codes end early, or one refers
/// to nothing.
fn unpack(packed: &[u8], bits: u32, numbers: &mut Vec<u8>, count: usize) -> Option<()> {
    let (clear, end) = (1usize << bits, (1usize << bits) + 1);
    // Per code: the code of its run without its last index, that last index, its first, and its
    // length.
    let (mut before, mut last, mut first, mut long) = ([0u16; CODES], [0u8; CODES], [0u8; CODES], [1u16; CODES]);
    for number in 0..clear {
        (last[number], first[number]) = (number as u8, number as u8);
    }
    numbers.clear();
    numbers.resize(count, 0);
    // Current code size, next code to assign, and the last code read; bits buffered, their
    // count, and the next byte; and how many indices are out.
    let (mut size, mut next, mut prior) = (bits + 1, end + 1, None);
    let (mut held, mut have, mut at, mut done) = (0u32, 0, 0, 0);
    while done < count {
        while have < size {
            (held, have, at) = (held | (*packed.get(at)? as u32) << have, have + 8, at + 1);
        }
        let code = (held & ((1 << size) - 1)) as usize;
        (held, have) = (held >> size, have - size);
        if code == clear {
            (size, next, prior) = (bits + 1, end + 1, None);
            continue;
        }
        // The end before all indices are out; or a code never assigned.
        if code == end || code > next || (code == next && prior.is_none()) || next > CODES {
            return None;
        }
        // The previous code plus the first index of this one's run is a new entry: which is
        // this code's own, if this code is the one being assigned.
        if let Some(prior) = prior.filter(|_| next < CODES) {
            let begins = first[if code == next { prior } else { code }];
            (before[next], last[next], first[next], long[next]) = (prior as u16, begins, first[prior], long[prior] + 1);
            next += 1;
            size += (next == 1 << size && size < 12) as u32;
        }
        // The run is written from its last index back to its first.
        let (run, mut link) = (long[code] as usize, code);
        for slot in (done..done + run).rev() {
            if let Some(number) = numbers.get_mut(slot) {
                *number = last[link];
            }
            link = before[link] as usize;
        }
        (done, prior) = (done + run, Some(code));
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// A test file: its name; the file; what libnsgif makes of all its frames, stacked
    /// vertically, as vips passes them on (ffmpeg gives the same for each, except the last,
    /// where it takes only the first frame); each frame's delay in ms; and the loop count (both
    /// as giflib's `giftext` reads them).
    type Tested = (&'static str, &'static [u8], &'static [u8], &'static [u64], Option<u64>);

    /// The test files.
    const FILMS: [Tested; 6] = [
        // One frame, as ffmpeg writes it: a 256-colour global palette, 40 ms, looping for ever.
        ("still", include_bytes!("../tests/pictures/still.gif"), include_bytes!("../tests/pictures/still.frames.png"), &[40], None),
        // The same, interlaced by giflib's giftool, which writes GIF87a and drops the graphic
        // control extension.
        ("passes", include_bytes!("../tests/pictures/passes.gif"), include_bytes!("../tests/pictures/passes.frames.png"), &[100], Some(1)),
        // Five frames as ffmpeg writes them: after the first, only the changed block, with a
        // transparent index for its unchanged pixels; looping for ever.
        ("moving", include_bytes!("../tests/pictures/moving.gif"), include_bytes!("../tests/pictures/moving.frames.png"), &[200; 5], None),
        // The same animation written by cgif (through vips), which compresses differently.
        ("moving-cgif", include_bytes!("../tests/pictures/moving-cgif.gif"), include_bytes!("../tests/pictures/moving-cgif.frames.png"), &[200; 5], None),
        // Hand-written and built by giflib's gifbuild: a local palette, an interlaced frame off
        // the corner, frames cleared and restoring, a frame without delay; played three times.
        ("hand", include_bytes!("../tests/pictures/hand.gif"), include_bytes!("../tests/pictures/hand.frames.png"), &[70, 100, 300, 50], Some(3)),
        // Two frames without graphic control extensions, in GIF87a.
        ("two", include_bytes!("../tests/pictures/two.gif"), include_bytes!("../tests/pictures/two.frames.png"), &[100, 100], Some(1)),
    ];

    /// Every frame of an animation as the canvas stands while it shows, stacked vertically.
    fn frames(reel: &mut Reel) -> Vec<u32> {
        (0..reel.frames.len()).flat_map(|frame| reel.shown(frame).0.to_vec()).collect()
    }
    /// Pixels as they look: transparent is transparent, whatever colour it holds.
    fn seen(pixels: &[u32]) -> Vec<u32> {
        pixels.iter().map(|pixel| if pixel >> 24 == 0 { 0 } else { *pixel }).collect()
    }

    #[test]
    fn frames_come_out_as_ffmpeg_and_libnsgif_show_them() {
        // GIF is lossless, so nothing may differ: every pixel of every frame matches the
        // reference.
        for (name, gif, wanted, shows, plays) in FILMS {
            let mut reel = Reel::read(gif, usize::MAX).unwrap_or_else(|| panic!("{name}.gif is read"));
            let (width, wanted) = png::decode(wanted).unwrap();
            assert_eq!((reel.size[0], reel.frames.len() * reel.size[1]), (width, wanted.len() / width), "{name}: its size, and how many frames it has");
            assert!(seen(&frames(&mut reel)) == seen(&wanted), "{name}: its pixels");
            assert_eq!((reel.frames.iter().map(|frame| frame.shows).collect::<Vec<u64>>(), reel.plays), (shows.to_vec(), plays), "{name}: its times");
            // The first frame is the file as an image.
            assert_eq!(decode(gif).map(|(width, pixels)| (width, seen(&pixels))), Some((width, seen(&wanted[..width * reel.size[1]]))), "{name} as a picture");
        }
    }

    #[test]
    fn a_frame_is_shown_the_same_however_the_film_got_to_it() {
        for (name, gif, ..) in FILMS {
            let mut reel = Reel::read(gif, usize::MAX).unwrap();
            let (count, forward) = (reel.frames.len(), frames(&mut reel));
            let each = forward.len() / count;
            // Back from the last to the first, each from wherever the canvas stands; the same
            // twice in a row; and past the last, which is the last.
            for frame in (0..count).rev().chain([0, 0, count - 1, count + 7]) {
                let at = frame.min(count - 1) * each;
                assert!(reel.shown(frame).0 == &forward[at..at + each], "{name}: frame {frame}");
            }
        }
    }

    #[test]
    fn the_frame_that_is_due_goes_by_the_times_of_the_frames() {
        let ms = Duration::from_millis;
        let hand = Reel::read(FILMS[4].1, usize::MAX).unwrap();
        // 70, 100, 300 and 50 ms: 520 per loop, three loops.
        assert_eq!(hand.due(ms(0)), (0, Some(ms(70))));
        assert_eq!([hand.due(ms(69)), hand.due(ms(70)), hand.due(ms(169))], [(0, Some(ms(1))), (1, Some(ms(100))), (1, Some(ms(1)))]);
        assert_eq!([hand.due(ms(170)), hand.due(ms(470)), hand.due(ms(519))], [(2, Some(ms(300))), (3, Some(ms(50))), (3, Some(ms(1)))]);
        assert_eq!([hand.due(ms(520)), hand.due(ms(520 + 75)), hand.due(ms(3 * 520 - 1))], [(0, Some(ms(70))), (1, Some(ms(95))), (3, Some(ms(1)))]);
        // After the last loop it stays at its last frame, and nothing more is due.
        assert_eq!([hand.due(ms(3 * 520)), hand.due(ms(1_000_000_000))], [(3, None); 2]);
        // A looping animation never ends; a one-frame one is never due again.
        let (moving, still) = (Reel::read(FILMS[2].1, usize::MAX).unwrap(), Reel::read(FILMS[0].1, usize::MAX).unwrap());
        assert_eq!([moving.due(ms(999)), moving.due(ms(1000)), moving.due(ms(86_400_000 * 365 + 450))], [(4, Some(ms(1))), (0, Some(ms(200))), (2, Some(ms(150)))]);
        assert_eq!([still.due(ms(0)), still.due(ms(5000))], [(0, None); 2]);
    }

    /// A hand-made GIF: a canvas of `size`, a global palette, then `parts` as bytes.
    fn file(size: [u16; 2], colors: &[[u8; 3]], parts: &[Vec<u8>]) -> Vec<u8> {
        let flags = if colors.is_empty() { 0 } else { 0x80 | (colors.len().trailing_zeros() as u8 - 1) };
        let head = [&b"GIF89a"[..], &size[0].to_le_bytes(), &size[1].to_le_bytes(), &[flags, 0, 0]].concat();
        [head, colors.concat(), parts.concat(), vec![0x3B]].concat()
    }
    /// A frame for such a file: rect, flags, and its indices in the simplest encoding: each a
    /// literal 3-bit code, with a clear code before every one, so no code is ever a run.
    fn frame(place: [u16; 4], flags: u8, numbers: &[u8]) -> Vec<u8> {
        let codes = numbers.iter().flat_map(|number| [4, *number as u32]).chain([5]);
        let (mut bits, mut have, mut packed) = (0u32, 0, Vec::new());
        for code in codes {
            (bits, have) = (bits | code << have, have + 3);
            while have >= 8 {
                packed.push(bits as u8);
                (bits, have) = (bits >> 8, have - 8);
            }
        }
        packed.push(bits as u8);
        let head = place.iter().flat_map(|side| side.to_le_bytes()).collect::<Vec<u8>>();
        [&[0x2C][..], &head, &[flags, 2, packed.len() as u8], &packed, &[0]].concat()
    }
    /// A graphic control extension: flags (disposal times four; plus one if transparent), delay
    /// in hundredths, and the transparent index.
    fn said(flags: u8, shows: u16, clear: u8) -> Vec<u8> {
        [&[0x21, 0xF9, 4, flags][..], &shows.to_le_bytes(), &[clear, 0]].concat()
    }
    const COLORS: [[u8; 3]; 4] = [[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]];
    const BLACK: u32 = 0xff00_0000;
    const RED: u32 = 0xffff_0000;
    const GREEN: u32 = 0xff00_ff00;
    const BLUE: u32 = 0xff00_00ff;

    #[test]
    fn frames_make_room_for_the_next_as_they_say() {
        // A 4 by 1 canvas. A red frame over all of it; a green one over the middle two,
        // cleared; a blue one at the right, restoring the previous content; then one whose
        // second index is transparent.
        let parts = [
            frame([0, 0, 4, 1], 0, &[1, 1, 1, 1]),
            said(2 << 2, 5, 0),
            frame([1, 0, 2, 1], 0, &[2, 2]),
            said(3 << 2, 5, 0),
            frame([3, 0, 1, 1], 0, &[3]),
            said(1, 5, 0),
            frame([0, 0, 2, 1], 0, &[2, 0]),
        ];
        let mut reel = Reel::read(&file([4, 1], &COLORS, &parts), usize::MAX).unwrap();
        assert_eq!(frames(&mut reel), [[RED, RED, RED, RED], [RED, GREEN, GREEN, RED], [RED, 0, 0, BLUE], [GREEN, 0, 0, RED]].concat());
        // Off-canvas parts are dropped; a local palette wins over the global one; an index past
        // the palette is black; with no palettes at all, everything is.
        let own = frame([3, 0, 4, 1], 0x80, &[0, 1, 1, 1]);
        let own = [&own[..10], &[9, 9, 9, 8, 8, 8], &own[10..]].concat();
        let mut reel = Reel::read(&file([4, 1], &COLORS, &[frame([0, 0, 4, 1], 0, &[3, 3, 3, 3]), own]), usize::MAX).unwrap();
        assert_eq!(frames(&mut reel), [[BLUE; 4], [BLUE, BLUE, BLUE, 0xff09_0909]].concat());
        let mut reel = Reel::read(&file([2, 1], &COLORS[..2], &[frame([0, 0, 2, 1], 0, &[1, 3])]), 1).unwrap();
        assert_eq!(reel.shown(0).0, [RED, BLACK]);
        assert_eq!(decode(&file([2, 1], &[], &[frame([0, 0, 2, 1], 0, &[1, 3])])), Some((2, vec![BLACK; 2])));
    }

    #[test]
    fn what_is_no_gif_or_cannot_be_what_it_says_gives_nothing() {
        let good = file([2, 1], &COLORS, &[frame([0, 0, 2, 1], 0, &[1, 2])]);
        assert_eq!(decode(&good), Some((2, vec![RED, GREEN])));
        let changed = |at: usize, to: u8| decode(&[&good[..at], &[to], &good[at + 1..]].concat());
        assert_eq!([decode(b"not a picture"), decode(&[]), decode(&good[..12]), decode(include_bytes!("../tests/pictures/plain.png")), changed(4, b'8')], [None, None, None, None, None]);
        // An empty canvas or frame; a frame of more pixels than its bytes hold; and code sizes
        // too large or too small.
        let frame_at = good.iter().position(|byte| *byte == 0x2C).unwrap();
        assert!([changed(6, 0), changed(8, 0), changed(frame_at + 5, 0), changed(frame_at + 5, 3), changed(frame_at + 10, 9), changed(frame_at + 10, 0)].iter().all(Option::is_none));
        // A 65535 by 65535 canvas in a file this small: refused before any memory is allocated.
        assert_eq!(decode(&[&good[..6], &[255; 4], &good[10..]].concat()), None);
        // An animation ends before the first incomplete frame: here the second.
        let two = file([2, 1], &COLORS, &[frame([0, 0, 2, 1], 0, &[1, 2]), frame([0, 0, 2, 1], 0, &[3, 3])]);
        let frames_read = |file: &[u8]| Reel::read(file, usize::MAX).map(|reel| reel.frames.len());
        assert_eq!([frames_read(&two), frames_read(&two[..two.len() - 3]), frames_read(&two[..two.len() - 1])], [Some(2), Some(1), Some(2)]);
        // Only as many frames are read as asked for.
        assert_eq!(Reel::read(&two, 1).map(|reel| reel.frames.len()), Some(1));
    }

    #[test]
    fn a_damaged_file_gives_nothing_or_some_film() {
        // Whatever the file, every frame shows and fills the canvas.
        let sound = |gif: &[u8]| {
            let Some(mut reel) = Reel::read(gif, usize::MAX) else { return true };
            let screen = reel.size[0] * reel.size[1];
            (0..reel.frames.len()).rev().all(|frame| reel.shown(frame).0.len() == screen) && decode(gif).is_some()
        };
        for (name, gif, ..) in FILMS {
            // Truncated at every length; then whole, with each byte in turn changed: a bit
            // flipped, 0, and 255.
            assert!((0..gif.len()).all(|cut| sound(&gif[..cut])), "{name}, cut short");
            for at in 0..gif.len() {
                let damaged = |to: u8| [&gif[..at], &[to], &gif[at + 1..]].concat();
                assert!([gif[at] ^ (1 << (at % 8)), 0, 255].into_iter().all(|to| sound(&damaged(to))), "{name}, byte {at}");
            }
        }
        // A one-frame image truncated anywhere is none at all.
        let still = FILMS[0].1;
        assert!((0..still.len() - 1).all(|cut| decode(&still[..cut]).is_none()));
    }

    /// Regenerates these tests' animations, and what ffmpeg makes of each.
    /// (`cargo test --lib gif -- --ignored`, with ffmpeg, vips and giflib's tools installed.)
    #[test]
    #[ignore = "needs ffmpeg, vips and giflib's tools, and writes the files the other tests read"]
    fn the_films_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-films-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        // Five 24 by 16 frames: colour bands, a transparent corner, and a block moving left to
        // right.
        for step in 0..5usize {
            let pixel = |at: usize| {
                let (x, y) = (at % 24, at / 24);
                match () {
                    _ if x + y < 5 => 0,
                    _ if (4 * step + 2..4 * step + 8).contains(&x) && (5..11).contains(&y) => 0xfff5_9e0b,
                    _ => [0xff1e_3a8a, 0xff0f_766e, 0xff7c_2d12, 0xfff1_f5f9][y / 4],
                }
            };
            std::fs::write(work.join(format!("f{step}.png")), png::encode_clear(24, &(0..24 * 16).map(pixel).collect::<Vec<u32>>()).unwrap()).unwrap();
        }
        let run = |program: &str, arguments: &[&str], input: Option<&PathBuf>, output: Option<&PathBuf>| {
            let mut command = Command::new(program);
            command.args(arguments).current_dir(&work).stdin(input.map_or(Stdio::null(), |file| std::fs::File::open(file).unwrap().into()));
            command.stdout(output.map_or(Stdio::null(), |file| std::fs::File::create(file).unwrap().into()));
            assert!(command.status().is_ok_and(|status| status.success()), "{program} {arguments:?}");
        };
        let at = |name: &str| pictures.join(name);
        let text = |name: &str| at(name).to_str().unwrap().to_string();
        // With a palette made for these images and one entry reserved for transparency.
        let colours = "split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse";
        run("ffmpeg", &["-y", "-loglevel", "error", "-i", "f0.png", "-filter_complex", colours, &text("still.gif")], None, None);
        run("giftool", &["-i", "1"], Some(&at("still.gif")), Some(&at("passes.gif")));
        run("ffmpeg", &["-y", "-loglevel", "error", "-framerate", "5", "-i", "f%d.png", "-filter_complex", colours, "-loop", "0", &text("moving.gif")], None, None);
        run("vips", &["copy", &format!("{}[n=-1]", text("moving.gif")), &text("moving-cgif.gif")], None, None);
        run("gifbuild", &[], Some(&at("hand.txt")), Some(&at("hand.gif")));
        run("gifbuild", &[], Some(&at("two.txt")), Some(&at("two.gif")));
        // What libnsgif makes of each, through vips: all frames stacked. And ffmpeg, which must
        // match, or this stops here. (Except for the GIF87a file, where ffmpeg takes only the
        // first frame.)
        for (name, ..) in FILMS {
            let (gif, frames) = (text(&format!("{name}.gif")), text(&format!("{name}.frames.png")));
            run("vips", &["copy", &format!("{gif}[n=-1]"), &frames], None, None);
            let nsgif = png::decode(&std::fs::read(&frames).unwrap()).unwrap();
            let under = format!("tile=1x{}", nsgif.1.len() / decode(&std::fs::read(&gif).unwrap()).unwrap().1.len());
            run("ffmpeg", &["-y", "-loglevel", "error", "-i", &gif, "-vf", &under, "-frames:v", "1", "-pix_fmt", "rgba", "ffmpeg.png"], None, None);
            let ffmpeg = png::decode(&std::fs::read(work.join("ffmpeg.png")).unwrap()).unwrap();
            assert!(name == "two" || (ffmpeg.0 == nsgif.0 && seen(&ffmpeg.1) == seen(&nsgif.1)), "{name}: ffmpeg and libnsgif show the same");
        }
        std::fs::remove_dir_all(work).unwrap();
    }
}
