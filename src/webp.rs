//! WebP reading: lossless and lossy pictures, each with or without alpha, so an app can show
//! web pictures without another library.
//!
//! A WebP file is a list of chunks: a four-letter name, a length, the data. The picture is in
//! one of two chunk kinds that share nothing but the container.
//!
//! Lossless ("VP8L"): each pixel is stored as its difference from a prediction, with rules
//! given per block; colours may be palette indices, or stored relative to green. The result is
//! Huffman-coded, with back-references for runs and a small colour cache. Reading undoes these
//! in reverse.
//!
//! Lossy ("VP8 "): luma and two half-size chroma planes, in 16×16 macroblocks. Each block is
//! predicted from the pixels above and left in a way the file names, and the residue is stored
//! as DCT coefficients per 4×4, quantized: that rounding is the loss. Coefficients are
//! boolean-coded. Then block edges are loop-filtered and chroma upsampled. All in integers with
//! libwebp's arithmetic, so pixels match that reader bit for bit. Alpha comes in its own chunk
//! ("ALPH"), a byte per pixel, raw or lossless-packed; if that chunk is damaged, the picture is
//! shown opaque.
//!
//! An animated WebP is read as an animation (see `animation`), frames composed as libwebp does;
//! as a picture, its first frame as the animation shows it. A picture is turned upright per its
//! `EXIF` orientation, and colour-managed per its `ICCP` profile (see `color_profiles`). Other
//! metadata is skipped.
use crate::png::{Bits, Codes, codes};

/// The most pixels a file may claim per byte of its size. (A one-colour picture takes almost no
/// bytes at any size, so such a file could ask for all memory; one claiming more is refused.)
const MOST: usize = 2048;

/// Decodes a WebP file: the picture's width, and its pixels in rows from the top, each
/// `0xAARRGGBB` (not premultiplied), as [`Ui::add_picture`](crate::Ui::add_picture) takes them.
/// Turned upright per `EXIF`, colour-managed per `ICCP`. Of an animation, the first frame as
/// the animation shows it. Nothing for a damaged file or no WebP.
pub fn decode(webp: &[u8]) -> Option<(usize, Vec<u32>)> {
    let chunks = chunks(webp)?;
    let (width, pixels) = match chunks.iter().any(|(name, _)| *name == b"ANMF") {
        true => read_animation(webp, 1).map(|mut anim| (anim.size[0], anim.shown(0).0.to_vec()))?,
        false => {
            let (width, mut pixels) = still(&chunks, webp.len())?;
            if let Some(change) = crate::color_profiles::Change::of_profile(first(&chunks, b"ICCP")) {
                change.apply(&mut pixels);
            }
            (width, pixels)
        }
    };
    Some(crate::exif_orientation::turned(crate::exif_orientation::turn(first(&chunks, b"EXIF")), width, pixels))
}

/// The chunks of a WebP file (or of an animation frame, after its header), each as name and
/// data: up to the end or the first truncated one. Nothing for a file not starting as WebP.
fn chunks(webp: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    Some(chunks_of(webp.strip_prefix(b"RIFF")?.get(4..)?.strip_prefix(b"WEBP")?))
}

/// The chunks `rest` holds, as `chunks` returns them.
fn chunks_of(mut rest: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut chunks = Vec::new();
    while let Some((head, after)) = rest.split_first_chunk::<8>() {
        // (An odd-length chunk is followed by a padding byte.)
        let long = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let Some(chunk) = after.get(..long) else { break };
        chunks.push((&head[..4], chunk));
        rest = after.get(long + long % 2..).unwrap_or(&[]);
    }
    chunks
}

/// The data of the first chunk of a name; empty where none.
fn first<'a>(chunks: &[(&[u8], &'a [u8])], called: &[u8]) -> &'a [u8] {
    chunks.iter().find(|(name, _)| *name == called).map_or(&[], |(_, chunk)| chunk)
}

/// The picture among chunks: lossless, or lossy with its preceding alpha chunk. `room` is the
/// file's length. Nothing where none is whole.
fn still(chunks: &[(&[u8], &[u8])], room: usize) -> Option<(usize, Vec<u32>)> {
    let mut solid = None;
    for (name, chunk) in chunks.iter().copied() {
        match name {
            b"VP8L" => return lossless(chunk, room),
            b"VP8 " => return lossy(chunk, solid, room),
            b"ALPH" => solid = Some(chunk),
            _ => {}
        }
    }
    None
}

/// Whether a frame's picture has alpha: a lossless one says so in a header bit, a lossy one by
/// having an alpha chunk.
fn seen_through(chunks: &[(&[u8], &[u8])]) -> bool {
    let bit = |chunk: &[u8]| chunk.get(4).is_some_and(|byte| byte & 0x10 != 0);
    chunks.iter().any(|(name, chunk)| *name == b"ALPH" || (*name == b"VP8L" && bit(chunk)))
}

/// The pixels of an animation frame, from its chunks as stored (with headers, after the frame's
/// own). Nothing where damaged, or not the size the frame says.
pub(crate) fn frame(chunks: &[u8], size: [usize; 2]) -> Option<Vec<u32>> {
    let (width, pixels) = still(&chunks_of(chunks), chunks.len())?;
    (width == size[0] && pixels.len() == size[0] * size[1]).then_some(pixels)
}

/// Reads a WebP as an animation: its first `most` frames at most. A still WebP is a one-frame
/// animation. Nothing if no WebP, or no frame is whole. (An animation ignores `EXIF`
/// orientation.)
///
/// An animated WebP gives its canvas size (`VP8X`) and loop count (`ANIM`); then each frame is
/// an `ANMF` chunk with its offset, duration, dispose and blend flags, and its picture's
/// chunks.
pub(crate) fn read_animation(webp: &[u8], most: usize) -> Option<crate::animation::LoadedAnimation> {
    use crate::animation::{After, Frame, Kind, Laid, LoadedAnimation};
    let chunks = chunks(webp)?;
    let change = crate::color_profiles::Change::of_profile(first(&chunks, b"ICCP"));
    let three = |bytes: &[u8], at: usize| bytes.get(at..at + 3).map_or(0, |three| u32::from_le_bytes([three[0], three[1], three[2], 0]) as usize);
    let frames: Vec<&[u8]> = chunks.iter().filter(|(name, _)| *name == b"ANMF").map(|(_, chunk)| *chunk).take(most).collect();
    // A still file is its picture: all its chunks become the frame's.
    if frames.is_empty() {
        let (width, pixels) = still(&chunks, webp.len())?;
        let mut anim = LoadedAnimation::new([width, pixels.len() / width], Kind::Webp(change.map(Box::new)), Some(1));
        anim.packed = webp.get(12..)?.to_vec();
        anim.frames.push(Frame::new([0, 0, width, pixels.len() / width], [0, anim.packed.len()], 0, After::Stays, Laid::Instead));
        return anim.done();
    }
    let screen = first(&chunks, b"VP8X");
    let size = [three(screen, 4) + 1, three(screen, 7) + 1];
    // (Loop count; 0 is forever.)
    let plays = first(&chunks, b"ANIM").get(4..6).map_or(Some(1), |plays| Some(u16::from_le_bytes([plays[0], plays[1]]) as u64).filter(|plays| *plays > 0));
    if size[0].saturating_mul(size[1]) > webp.len().saturating_mul(MOST) {
        return None;
    }
    let mut anim = LoadedAnimation::new(size, Kind::Webp(change.map(Box::new)), plays);
    for frame in frames {
        let Some((head, chunks)) = frame.split_at_checked(16) else { break };
        let place = [three(head, 0) * 2, three(head, 3) * 2, three(head, 6) + 1, three(head, 9) + 1];
        let after = if head[15] & 1 == 1 { After::Cleared } else { After::Stays };
        let laid = if head[15] & 2 == 0 && seen_through(&chunks_of(chunks)) { Laid::OverWebp } else { Laid::Instead };
        // (A frame off the canvas ends the animation, as does one not whole: each is decoded
        // once to see.)
        if place[0] + place[2] > size[0] || place[1] + place[3] > size[1] || self::frame(chunks, [place[2], place[3]]).is_none() {
            break;
        }
        let packed = [anim.packed.len(), anim.packed.len() + chunks.len()];
        anim.packed.extend_from_slice(chunks);
        anim.frames.push(Frame::new(place, packed, three(head, 12) as u64, after, laid));
    }
    anim.done()
}

/// Decodes a lossless chunk. `room` is the file's length.
fn lossless(chunk: &[u8], room: usize) -> Option<(usize, Vec<u32>)> {
    let mut bits = Bits::of(chunk.strip_prefix(&[0x2f])?);
    bits.fill()?;
    // Fourteen bits each for width and height less one; one alpha hint; three version bits that
    // must be 0.
    let (wide, high, _, version) = (bits.take(14) + 1, bits.take(14) + 1, bits.take(1), bits.take(3));
    if version != 0 || wide * high > room.saturating_mul(MOST) {
        return None;
    }
    picture(&mut bits, [wide, high]).map(|pixels| (wide, pixels))
}

/// A transform applied before encoding and undone after decoding: which of four (see `undo`),
/// the width before it, its data (a sub-image of per-block settings, or a palette), and a
/// number with it: block size, or pixels packed per pixel, as a power of two.
type Change = (usize, usize, Vec<u32>, u32);

/// Decodes a `size[0]` by `size[1]` picture: its transforms, its pixels, then the transforms
/// undone.
fn picture(bits: &mut Bits, [wide, high]: [usize; 2]) -> Option<Vec<u32>> {
    let (mut changes, mut across): (Vec<Change>, usize) = (Vec::new(), wide);
    loop {
        bits.fill()?;
        if bits.take(1) == 0 {
            break;
        }
        // (Each transform may appear once.)
        let kind = bits.take(2);
        if changes.iter().any(|(done, ..)| *done == kind) {
            return None;
        }
        changes.push(match kind {
            // Predictor, or colour transform: per block.
            0 | 1 => {
                let block = bits.take(3) as u32 + 2;
                (kind, across, plain(bits, [across.div_ceil(1 << block), high.div_ceil(1 << block)])?, block)
            }
            // Subtract green.
            2 => (kind, across, Vec::new(), 0),
            // Colour indexing: a palette, each entry delta-coded from the previous; with a
            // small palette, several indices per pixel.
            _ => {
                let listed = bits.take(8) + 1;
                let mut colors = plain(bits, [listed, 1])?;
                (1..listed).for_each(|at| colors[at] = sum(colors[at], colors[at - 1]));
                let (packed, before) = ([3, 3, 2, 2, 1][listed.min(5) - 1] * (listed <= 16) as u32, across);
                across = across.div_ceil(1 << packed);
                (kind, before, colors, packed)
            }
        });
    }
    let mut pixels = whole(bits, [across, high])?;
    for change in changes.iter().rev() {
        pixels = undo(change, pixels, high);
    }
    Some(pixels)
}

/// Undoes one transform of a picture `high` pixels high.
fn undo((kind, wide, with, power): &Change, mut pixels: Vec<u32>, high: usize) -> Vec<u32> {
    let (wide, power) = (*wide, *power);
    // The block setting for a pixel, from the sub-image.
    let treated = |at: usize| with[((at / wide) >> power) * wide.div_ceil(1 << power) + ((at % wide) >> power)];
    match kind {
        0 => unguess(&mut pixels, wide, |at| treated(at) >> 8 & 15),
        // Red and blue were stored less a multiple of green, and blue less one of red too: the
        // block gives the signed multipliers.
        1 => {
            for (at, pixel) in pixels.iter_mut().enumerate() {
                let (by, part) = (treated(at), |of: u32, value: u32| ((of as i8 as i32 * value as i8 as i32) >> 5) as u32);
                let red = (*pixel >> 16).wrapping_add(part(by, *pixel >> 8)) & 255;
                let blue = pixel.wrapping_add(part(by >> 8, *pixel >> 8)).wrapping_add(part(by >> 16, red)) & 255;
                *pixel = *pixel & 0xff00_ff00 | red << 16 | blue;
            }
        }
        2 => pixels.iter_mut().for_each(|pixel| *pixel = sum(*pixel, (*pixel >> 8 & 255) * 0x1_0001)),
        // The indices are in each pixel's green: so many bits per packed pixel. (An index past
        // the palette is transparent.)
        _ => {
            let (bits, narrow) = (8 >> power, wide.div_ceil(1 << power));
            let number = |at: usize| pixels[at / wide * narrow + ((at % wide) >> power)] >> 8 >> (bits * ((at % wide) & ((1 << power) - 1))) & ((1 << bits) - 1);
            pixels = (0..wide * high).map(|at| with.get(number(at) as usize).copied().unwrap_or(0)).collect();
        }
    }
    pixels
}

/// Two pixels added channel by channel (wrapping mod 256).
fn sum(one: u32, other: u32) -> u32 {
    ((one & 0xff00_ff00).wrapping_add(other & 0xff00_ff00) & 0xff00_ff00) | (((one & 0x00ff_00ff) + (other & 0x00ff_00ff)) & 0x00ff_00ff)
}

/// Undoes prediction: each pixel was stored as its difference from one of fourteen predictors,
/// which `rule` gives per position. The neighbours are left, above, above-left and above-right;
/// the first row has only left, the first column only above (the very first pixel is predicted
/// as black).
fn unguess(pixels: &mut [u32], wide: usize, rule: impl Fn(usize) -> u32) {
    // Channel-wise average of two pixels; and each channel clamped to 0..255.
    let half = |one: u32, other: u32| (((one ^ other) & 0xfefe_fefe) >> 1) + (one & other);
    let each = |value: &dyn Fn(u32) -> i32| (0..4).fold(0, |pixel, part| pixel | (value(part * 8).clamp(0, 255) as u32) << (part * 8));
    for at in 0..pixels.len() {
        let before = |back: usize| pixels[at - back];
        let part = |pixel: u32, shift: u32| (pixel >> shift & 255) as i32;
        // (Manhattan distance of a pixel from "left + above - above-left".)
        let far = |from: u32| (0..4).map(|shift| (part(from, shift * 8) - part(before(wide + 1), shift * 8)).abs()).sum::<i32>();
        let guess = match (at, at % wide, if at < wide { 1 } else { rule(at) }) {
            (0, ..) => 0xff00_0000,
            (_, 0, _) => before(wide),
            (_, _, 0) => 0xff00_0000,
            (_, _, 1) => before(1),
            (_, _, 2) => before(wide),
            (_, _, 3) => before(wide - 1),
            (_, _, 4) => before(wide + 1),
            (_, _, 5) => half(half(before(1), before(wide - 1)), before(wide)),
            (_, _, 6) => half(before(1), before(wide + 1)),
            (_, _, 7) => half(before(1), before(wide)),
            (_, _, 8) => half(before(wide + 1), before(wide)),
            (_, _, 9) => half(before(wide), before(wide - 1)),
            (_, _, 10) => half(half(before(1), before(wide + 1)), half(before(wide), before(wide - 1))),
            // Select: whichever of left and above is nearer to "left + above - above-left",
            // over all four channels.
            (_, _, 11) if far(before(wide)) < far(before(1)) => before(1),
            (_, _, 11) => before(wide),
            // Clamp-add-subtract-full; and clamp-add-subtract-half.
            (_, _, 12) => each(&|shift| part(before(1), shift) + part(before(wide), shift) - part(before(wide + 1), shift)),
            (_, _, 13) => each(&|shift| part(half(before(1), before(wide)), shift) + (part(half(before(1), before(wide)), shift) - part(before(wide + 1), shift)) / 2),
            _ => 0xff00_0000,
        };
        pixels[at] = sum(pixels[at], guess);
    }
}

/// A Huffman code for one alphabet: by code lengths, or, where only one symbol occurs, no code
/// and that symbol.
enum Code {
    One(usize),
    Many(Codes),
}

impl Code {
    /// The code for these code lengths (0: symbol unused). The lengths must fill the code space
    /// exactly.
    fn of(lengths: &[u8]) -> Option<Code> {
        let mut used = lengths.iter().enumerate().filter(|(_, length)| **length != 0);
        let (first, second) = (used.next()?, used.next());
        let full = lengths.iter().filter(|length| **length != 0).map(|length| 1u32 << (15 - length.min(&15))).sum::<u32>() == 1 << 15;
        match second {
            None => Some(Code::One(first.0)),
            Some(_) => codes(lengths, 8, |value| 1 << 8 | (value as u32) << 16).filter(|_| full).map(Code::Many),
        }
    }
    /// Reads one symbol. (`fill` first.)
    #[inline(always)]
    fn read(&self, bits: &mut Bits) -> Option<usize> {
        match self {
            Code::One(value) => Some(*value),
            Code::Many(codes) => Some(codes.read(bits)).filter(|entry| entry & 1 << 8 != 0).map(|entry| (entry >> 16) as usize),
        }
    }
}

/// The order in which the code-length code lengths are given.
const ORDER: [usize; 19] = [17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Reads the code for an alphabet of `values` symbols: one or two given outright; or each
/// symbol's code length, coded, where three symbols stand for "repeat the last length" and for
/// runs of zeros.
fn code(bits: &mut Bits, values: usize) -> Option<Code> {
    bits.fill()?;
    let mut lengths = vec![0u8; values];
    if bits.take(1) == 1 {
        // (One symbol or two; the first in one bit or eight.)
        let (two, wide) = (bits.take(1) == 1, bits.take(1) == 1);
        let first = bits.take(if wide { 8 } else { 1 });
        let second = if two { bits.take(8) } else { first };
        (*lengths.get_mut(first)?, *lengths.get_mut(second)?) = (1, 1);
        return Code::of(&lengths);
    }
    let mut first = [0u8; 19];
    for at in &ORDER[..4 + bits.take(4)] {
        bits.fill()?;
        first[*at] = bits.take(3) as u8;
    }
    let first = Code::of(&first)?;
    bits.fill()?;
    // How many lengths are given at most: all, or a number in so many bits.
    let all = bits.take(1) == 0;
    let more = 2 + 2 * bits.take(3 * !all as u32) as u32;
    let mut told = if all { values } else { 2 + bits.take(more) };
    let (mut at, mut last) = (0, 8);
    while at < values && told > 0 {
        bits.fill()?;
        let (length, times) = match first.read(bits)? {
            16 => (last, 3 + bits.take(2)),
            17 => (0, 3 + bits.take(3)),
            18 => (0, 11 + bits.take(7)),
            length => (length as u8, 1),
        };
        lengths.get_mut(at..at + times)?.fill(length);
        (at, told, last) = (at + times, told - 1, if times == 1 && length != 0 { length } else { last });
    }
    Code::of(&lengths)
}

/// The five codes pixels are packed with: green (above its 256 symbols, run lengths and
/// colour-cache indices for a cache of `stored`), red, blue, alpha, and distance.
fn group(bits: &mut Bits, stored: usize) -> Option<[Code; 5]> {
    Some([code(bits, 256 + 24 + stored)?, code(bits, 256)?, code(bits, 256)?, code(bits, 256)?, code(bits, 40)?])
}

/// Decodes a sub-image describing a larger one: with one code group.
fn plain(bits: &mut Bits, size: [usize; 2]) -> Option<Vec<u32>> {
    bits.fill()?;
    let stored = if bits.take(1) == 1 { bits.take(4) as u32 } else { 0 };
    let codes = [group(bits, (1 << stored) * (stored > 0) as usize)?];
    pixels(bits, size, &codes, (&[], 0), stored)
}

/// Decodes the main picture: possibly many code groups, with a sub-image saying which group
/// serves which block.
fn whole(bits: &mut Bits, size: [usize; 2]) -> Option<Vec<u32>> {
    bits.fill()?;
    let stored = if bits.take(1) == 1 { bits.take(4) as u32 } else { 0 };
    let block = if bits.take(1) == 1 { bits.take(3) as u32 + 2 } else { 0 };
    let which = if block > 0 { plain(bits, [size[0].div_ceil(1 << block), size[1].div_ceil(1 << block)])? } else { Vec::new() };
    // (A block's group is in its red and green; there are as many as the highest says.)
    let count = which.iter().map(|group| group >> 8 & 0xffff).max().map_or(1, |most| most as usize + 1);
    let mut groups = Vec::new();
    for _ in 0..count {
        groups.push(group(bits, (1 << stored) * (stored > 0) as usize)?);
    }
    pixels(bits, size, &groups, (&which, block), stored)
}

/// Distance codes 1 to 120 for the nearest pixels: rows up (high nibble), and pixels right of
/// eight to the left (low nibble).
const NEAR: [u8; 120] = [
    0x18, 0x07, 0x17, 0x19, 0x28, 0x06, 0x27, 0x29, 0x16, 0x1a, 0x26, 0x2a, 0x38, 0x05, 0x37, 0x39, 0x15, 0x1b, 0x36, 0x3a, 0x25, 0x2b, 0x48, 0x04, 0x47, 0x49, 0x14, 0x1c, 0x35, 0x3b, 0x46, 0x4a,
    0x24, 0x2c, 0x58, 0x45, 0x4b, 0x34, 0x3c, 0x03, 0x57, 0x59, 0x13, 0x1d, 0x56, 0x5a, 0x23, 0x2d, 0x44, 0x4c, 0x55, 0x5b, 0x33, 0x3d, 0x68, 0x02, 0x67, 0x69, 0x12, 0x1e, 0x66, 0x6a, 0x22, 0x2e,
    0x54, 0x5c, 0x43, 0x4d, 0x65, 0x6b, 0x32, 0x3e, 0x78, 0x01, 0x77, 0x79, 0x53, 0x5d, 0x11, 0x1f, 0x64, 0x6c, 0x42, 0x4e, 0x76, 0x7a, 0x21, 0x2f, 0x75, 0x7b, 0x31, 0x3f, 0x63, 0x6d, 0x52, 0x5e,
    0x00, 0x74, 0x7c, 0x41, 0x4f, 0x10, 0x20, 0x62, 0x6e, 0x30, 0x73, 0x7d, 0x51, 0x5f, 0x40, 0x72, 0x7e, 0x61, 0x6f, 0x50, 0x71, 0x7f, 0x60, 0x70,
];

/// A length or distance from its prefix code: small ones are the code, larger ones take extra
/// bits, more the larger.
#[inline(always)]
fn amount(bits: &mut Bits, code: usize) -> Option<usize> {
    let more = (code.max(2) - 2) as u32 >> 1;
    bits.fill()?;
    Some(if code < 4 { code + 1 } else { ((2 + (code & 1)) << more) + bits.take(more) + 1 })
}

/// Decodes `size[0]` by `size[1]` pixels. Each starts with a green-alphabet symbol: its green
/// (red, blue and alpha follow, each in its own code); or a back-reference copying so many
/// pixels from so far back; or an index into the cache of recent colours, which holds two to
/// the power `stored`. `groups` are the code groups and `which` picks one per block: a
/// sub-image and the block size as a power of two.
fn pixels(bits: &mut Bits, [wide, high]: [usize; 2], groups: &[[Code; 5]], (which, block): (&[u32], u32), stored: u32) -> Option<Vec<u32>> {
    if stored > 11 {
        return None;
    }
    let (mut pixels, mut store, mut at, mut kept) = (vec![0u32; wide * high], vec![0u32; (1 << stored) * (stored > 0) as usize], 0, 0);
    while at < pixels.len() {
        let of_block = if which.is_empty() { 0 } else { which[((at / wide) >> block) * wide.div_ceil(1 << block) + ((at % wide) >> block)] >> 8 & 0xffff };
        let codes = groups.get(of_block as usize)?;
        bits.fill()?;
        let green = codes[0].read(bits)?;
        // Every pixel so far goes into the cache, at a slot hashed from its colour, before a
        // cache index is read.
        while kept < at && green >= 280 {
            store[(pixels[kept].wrapping_mul(0x1e35_a7bd) >> (32 - stored)) as usize] = pixels[kept];
            kept += 1;
        }
        match green {
            0..256 => {
                bits.fill()?;
                let (red, blue, solid) = (codes[1].read(bits)?, codes[2].read(bits)?, codes[3].read(bits)?);
                pixels[at] = (solid << 24 | red << 16 | green << 8 | blue) as u32;
                at += 1;
            }
            256..280 => {
                let long = amount(bits, green - 256)?;
                let far = codes[4].read(bits)?;
                let far = amount(bits, far)?;
                // (The nearest positions are coded by place, not by distance.)
                let back = NEAR.get(far - 1).map_or(far.saturating_sub(120), |near| ((near >> 4) as usize * wide + 8).saturating_sub((near & 15) as usize).max(1));
                (back <= at && long <= pixels.len() - at).then_some(())?;
                (at..at + long).for_each(|to| pixels[to] = pixels[to - back]);
                at += long;
            }
            _ => {
                pixels[at] = *store.get(green - 280)?;
                at += 1;
            }
        }
    }
    Some(pixels)
}

/// The boolean decoder of lossy data: each bit was stored in as little room as its probability
/// allows, so it is read with the probability of 0 (out of 256). Bits come from a value the
/// bytes keep refining, and the width of the range it can still lie in.
struct BoolDecoder<'a> {
    data: &'a [u8],
    /// The next byte not yet in the value.
    at: usize,
    number: u32,
    /// The range width: 128 to 255.
    wide: u32,
    /// Bits shifted out of the value since the last whole byte came in.
    used: u32,
}

impl<'a> BoolDecoder<'a> {
    fn of(data: &'a [u8]) -> BoolDecoder<'a> {
        let first = |at: usize| data.get(at).copied().unwrap_or(0) as u32;
        BoolDecoder { data, at: 2, number: first(0) << 8 | first(1), wide: 255, used: 0 }
    }
    /// A bit whose probability of 0 is `no` in 256. (Past the end the bytes are zeros, as in
    /// every reader: a truncated file gives grey from there on, and nothing stops.)
    #[inline(always)]
    fn yes(&mut self, no: u32) -> bool {
        let split = 1 + (((self.wide - 1) * no) >> 8);
        let yes = self.number >= split << 8;
        (self.wide, self.number) = if yes { (self.wide - split, self.number - (split << 8)) } else { (split, self.number) };
        // The range is renormalized to at least 128 and the value shifted with it, a new byte
        // coming in every eight bits.
        let up = self.wide.leading_zeros() - 24;
        (self.wide, self.number, self.used) = (self.wide << up, self.number << up, self.used + up);
        if self.used >= 8 {
            self.used -= 8;
            self.number |= (self.data.get(self.at).copied().unwrap_or(0) as u32) << self.used;
            self.at += 1;
        }
        yes
    }
    /// An unsigned number of so many bits, each even odds, most significant first.
    fn bits(&mut self, count: u32) -> i32 {
        (0..count).fold(0, |number, _| number << 1 | self.yes(128) as i32)
    }
    /// A number present only if a flag bit says so (else 0), of so many bits, with a sign bit
    /// after.
    fn maybe(&mut self, count: u32) -> i32 {
        if !self.yes(128) {
            return 0;
        }
        let number = self.bits(count);
        if self.yes(128) { -number } else { number }
    }
}

/// The state of a lossy decode: its macroblocks across and down; three planes (luma, two
/// half-size chroma) padded to whole blocks; and what the headers say.
struct Lossy<'a> {
    across: usize,
    down: usize,
    planes: [Vec<u8>; 3],
    /// The partitions holding the coefficients: each macroblock row reads from one.
    parts: Vec<BoolDecoder<'a>>,
    /// The coefficient probabilities.
    spelling: [u8; 1056],
    /// The probability that a macroblock skips its coefficients, where that is coded per block.
    skipped: Option<u32>,
    /// The segment id tree probabilities; none where segments are off.
    kinds: Option<[u32; 3]>,
    /// The dequantization factors per segment: for luma, the Y2 (DC of the sixteen) block, and
    /// chroma; DC and AC.
    steps: [[[i32; 2]; 3]; 4],
    /// Loop filter strength per segment (none if the picture has none); the delta for all
    /// blocks and for those predicted per 4×4; sharpness; and whether the simple filter is
    /// used.
    smooth: [i32; 4],
    nudge: [i32; 2],
    sharp: i32,
    plain: bool,
}

/// How one macroblock's edges are filtered: the edge limit (0: none), the interior limit, the
/// high edge variance threshold, and whether the inner edges are filtered too.
#[derive(Clone, Copy, Default)]
struct Smoothed {
    across: i32,
    beside: i32,
    own: i32,
    inside: bool,
}

/// The neighbouring blocks' context: how each of the four columns of the block above and rows
/// of the block to the left was predicted; and whether their coefficients were all zero: for
/// the four luma columns (or rows), the two of each chroma, and Y2.
struct Around {
    ways_above: Vec<u8>,
    ways_left: [u8; 4],
    full_above: Vec<[bool; 9]>,
    full_left: [bool; 9],
}

/// The zigzag order of a block's sixteen coefficients, lowest frequency first.
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
/// Which of eight bands each position lies in. (One more, for the position past the last.)
const STRETCH: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];
/// The probabilities of the extra bits of large coefficients, from 11, 19, 35 and 67 up.
const LARGE: [&[u8]; 4] = [&[173, 148, 140], &[176, 155, 140, 135], &[180, 157, 141, 134, 130], &[254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129]];
/// How eight of the ten 4×4 intra modes fill a block, pixel by pixel, from the edge pixels left
/// and above (see `Lossy::small`): below 16, the edge pixel at that index averaged with its two
/// neighbours; 16 more, the average of it and the one before.
const SLANTS: [[u8; 16]; 8] = [
    [6, 7, 8, 9, 6, 7, 8, 9, 6, 7, 8, 9, 6, 7, 8, 9],
    [4, 4, 4, 4, 3, 3, 3, 3, 2, 2, 2, 2, 1, 1, 1, 1],
    [7, 8, 9, 10, 8, 9, 10, 11, 9, 10, 11, 12, 10, 11, 12, 13],
    [5, 6, 7, 8, 4, 5, 6, 7, 3, 4, 5, 6, 2, 3, 4, 5],
    [0x15, 0x16, 0x17, 0x18, 5, 6, 7, 8, 4, 0x15, 0x16, 0x17, 3, 5, 6, 7],
    [0x16, 0x17, 0x18, 0x19, 7, 8, 9, 10, 0x17, 0x18, 0x19, 11, 8, 9, 10, 12],
    [0x14, 5, 6, 7, 0x13, 4, 0x14, 5, 0x12, 3, 0x13, 4, 0x11, 2, 0x12, 3],
    [0x13, 3, 0x12, 2, 0x12, 2, 0x11, 1, 0x11, 1, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10],
];

/// Decodes a lossy chunk, and its alpha where the file has some. `room` is the file's length.
fn lossy(chunk: &[u8], solid: Option<&[u8]>, room: usize) -> Option<(usize, Vec<u32>)> {
    // Three bytes of frame tag (key frame, first partition length); a three-byte start code;
    // width and height in fourteen bits each.
    let head = chunk.first_chunk::<10>().filter(|head| head[0] & 1 == 0 && head[3..6] == [0x9d, 0x01, 0x2a])?;
    let first = (u32::from_le_bytes([head[0], head[1], head[2], 0]) >> 5) as usize;
    let (wide, high) = (u16::from_le_bytes([head[6], head[7]]) as usize & 0x3fff, u16::from_le_bytes([head[8], head[9]]) as usize & 0x3fff);
    if wide * high == 0 || wide * high > room.saturating_mul(MOST) {
        return None;
    }
    let (mut header, after) = (BoolDecoder::of(chunk.get(10..10 + first)?), &chunk[10 + first..]);
    let mut lossy = Lossy::read_header(&mut header, after, [wide.div_ceil(16), high.div_ceil(16)])?;
    let smoothed = lossy.blocks(&mut header);
    lossy.smooth(&smoothed);
    let mut pixels = lossy.colours([wide, high]);
    // Alpha comes separately, a byte per pixel.
    if let Some(solid) = solid.and_then(|chunk| solidity(chunk, [wide, high])) {
        pixels.iter_mut().zip(solid).for_each(|(pixel, solid)| *pixel = *pixel & 0x00ff_ffff | (solid as u32) << 24);
    }
    Some((wide, pixels))
}

impl<'a> Lossy<'a> {
    /// Reads the frame header before the blocks: segmentation, loop filter, partitions (in
    /// `after`), quantizers, and coefficient probability updates.
    fn read_header(header: &mut BoolDecoder, after: &'a [u8], [across, down]: [usize; 2]) -> Option<Lossy<'a>> {
        // (Two bits that change nothing here: the colour space, and whether clamping is
        // required, which this reader always does.)
        header.bits(2);
        // Up to four segments, each with its own quantizer and filter level: absolute, or as
        // deltas from the picture's.
        let (apart, mut kinds, mut fineness, mut smoothing, mut outright) = (header.yes(128), None, [0; 4], [0; 4], false);
        if apart {
            let (which, values) = (header.yes(128), header.yes(128));
            if values {
                outright = header.yes(128);
                fineness = [(); 4].map(|_| header.maybe(7));
                smoothing = [(); 4].map(|_| header.maybe(6));
            }
            kinds = which.then(|| [(); 3].map(|_| if header.yes(128) { header.bits(8) as u32 } else { 255 }));
        }
        let (plain, level, sharp) = (header.yes(128), header.bits(6), header.bits(3));
        // Loop filter deltas for all blocks and for those predicted per 4×4. (The six others
        // are for reference frames, which a still picture lacks.)
        let more = if header.yes(128) && header.yes(128) { [(); 8].map(|_| header.maybe(6)) } else { [0; 8] };
        // The partitions: one, two, four or eight, with all but the last one's length in three
        // bytes before the first.
        let count = 1usize << header.bits(2);
        let (lengths, mut rest) = after.split_at_checked(3 * (count - 1))?;
        let mut parts = Vec::new();
        for length in lengths.as_chunks::<3>().0 {
            let (part, more) = rest.split_at_checked(u32::from_le_bytes([length[0], length[1], length[2], 0]) as usize)?;
            parts.push(BoolDecoder::of(part));
            rest = more;
        }
        parts.push(BoolDecoder::of(rest));
        // The quantizer: a base level, and five deltas for the six factors.
        let (usual, off) = (header.bits(7), [(); 5].map(|_| header.maybe(4)));
        let step = |table: &[u16; 128], level: i32| table[level.clamp(0, 127) as usize] as i32;
        let steps = [0, 1, 2, 3].map(|kind| {
            let level = if apart { fineness[kind] + if outright { 0 } else { usual } } else { usual };
            let averages = [step(&FIRST, level + off[1]) * 2, (step(&OTHERS, level + off[2]) * 155 / 100).max(8)];
            [[step(&FIRST, level + off[0]), step(&OTHERS, level)], averages, [step(&FIRST, (level + off[3]).min(117)), step(&OTHERS, level + off[4])]]
        });
        let smooth = [0, 1, 2, 3].map(|kind| {
            if level == 0 {
                -1000
            } else if apart {
                smoothing[kind] + if outright { 0 } else { level }
            } else {
                level
            }
        });
        // (The refresh-entropy bit.) Then the coefficient probability updates.
        header.bits(1);
        let mut spelling = USUAL;
        for (likely, changed) in spelling.iter_mut().zip(CHANGED) {
            if header.yes(changed as u32) {
                *likely = header.bits(8) as u8;
            }
        }
        let skipped = header.yes(128).then(|| header.bits(8) as u32);
        let planes = [vec![0; across * down * 256], vec![0; across * down * 64], vec![0; across * down * 64]];
        Some(Lossy { across, down, planes, parts, spelling, skipped, kinds, steps, smooth, nudge: [more[0], more[4]], sharp, plain })
    }

    /// Decodes every macroblock, row by row, into the planes. Returns how each one's edges are
    /// filtered.
    fn blocks(&mut self, header: &mut BoolDecoder) -> Vec<Smoothed> {
        let (ways_above, full_above) = (vec![0; self.across * 4], vec![[false; 9]; self.across]);
        let mut around = Around { ways_above, ways_left: [0; 4], full_above, full_left: [false; 9] };
        let mut smoothed = Vec::new();
        for at in 0..self.across * self.down {
            if at % self.across == 0 {
                (around.ways_left, around.full_left) = ([0; 4], [false; 9]);
            }
            smoothed.push(self.block(header, [at % self.across, at / self.across], &mut around));
        }
        smoothed
    }

    /// Decodes the macroblock at `x`, `y` (in blocks): its segment, its prediction, and its
    /// coefficients. Puts its pixels into the planes.
    fn block(&mut self, header: &mut BoolDecoder, [x, y]: [usize; 2], around: &mut Around) -> Smoothed {
        let kind = match self.kinds {
            Some(likely) if header.yes(likely[0]) => 2 + header.yes(likely[2]) as usize,
            Some(likely) => header.yes(likely[1]) as usize,
            None => 0,
        };
        let skipped = self.skipped.is_some_and(|likely| header.yes(likely));
        // Luma is predicted whole one way, or per 4×4 subblock each its own way, coded in the
        // context of the modes above and left. (The four whole-block modes count as their 4×4
        // equivalents for that.)
        let (whole, mut ways) = (header.yes(145), [0u8; 16]);
        if whole {
            ways = [if header.yes(156) { 3 - 2 * header.yes(128) as u8 } else { 2 * header.yes(163) as u8 }; 16];
        }
        for part in (0..16).filter(|_| !whole) {
            let above = if part < 4 { around.ways_above[x * 4 + part] } else { ways[part - 4] };
            let left = if part % 4 == 0 { around.ways_left[part / 4] } else { ways[part - 1] };
            ways[part] = way(header, &WAYS[(above as usize * 10 + left as usize) * 9..][..9]);
        }
        around.ways_above[x * 4..x * 4 + 4].copy_from_slice(&ways[12..]);
        around.ways_left = [ways[3], ways[7], ways[11], ways[15]];
        let colour = if !header.yes(142) {
            0
        } else if !header.yes(114) {
            2
        } else if header.yes(183) {
            1
        } else {
            3
        };
        // Its twenty-four 4×4 blocks of coefficients: sixteen luma, four per chroma. A block
        // marked as skipped has none, and counts so for those after; one with none unmarked is
        // not filtered inside.
        let mut numbers = [0i32; 24 * 16];
        let any = !skipped && self.numbers(y & (self.parts.len() - 1), kind, whole, [x, 0], around, &mut numbers);
        if skipped {
            around.full_above[x][..8].fill(false);
            around.full_left[..8].fill(false);
            (around.full_above[x][8], around.full_left[8]) = (around.full_above[x][8] && !whole, around.full_left[8] && !whole);
        }
        // The prediction, plus the residue: luma, then each chroma.
        if whole {
            self.large(0, [x * 16, y * 16], ways[0]);
        }
        for part in 0..16 {
            let corner = [x * 16 + part % 4 * 4, y * 16 + part / 4 * 4];
            if !whole {
                self.small(corner, [x, y], ways[part]);
            }
            self.add(0, corner, &numbers[part * 16..][..16]);
        }
        for part in 16..24 {
            let (plane, corner) = (1 + (part - 16) / 4, [x * 8 + part % 2 * 4, y * 8 + part % 4 / 2 * 4]);
            if part % 4 == 0 {
                self.large(plane, [x * 8, y * 8], colour);
            }
            self.add(plane, corner, &numbers[part * 16..][..16]);
        }
        // The filter level: as its segment says, with the delta for a 4×4-predicted block; the
        // interior limit lowered by sharpness.
        let strong = (self.smooth[kind] + self.nudge[0] + if whole { 0 } else { self.nudge[1] }).clamp(0, 63);
        let dulled = if self.sharp > 0 { (strong >> if self.sharp > 4 { 2 } else { 1 }).min(9 - self.sharp) } else { strong };
        let (beside, own) = (dulled.max(1), if strong >= 40 { 2 } else { (strong >= 15) as i32 });
        Smoothed { across: if strong == 0 { 0 } else { 2 * strong + beside }, beside, own, inside: !whole || any }
    }

    /// Reads a macroblock's coefficients from one partition. Each 4×4 block is coded in the
    /// context of whether the blocks above and left were all zero. Says whether any coefficient
    /// is non-zero.
    fn numbers(&mut self, part: usize, kind: usize, whole: bool, [x, _]: [usize; 2], around: &mut Around, numbers: &mut [i32; 384]) -> bool {
        let (decoder, steps, above, left) = (&mut self.parts[part], self.steps[kind], &mut around.full_above[x], &mut around.full_left);
        // A whole-predicted block has its sixteen DC values in a Y2 block of their own, first,
        // spread by the inverse WHT.
        if whole {
            let mut averages = [0i32; 16];
            let last = spelled(decoder, &self.spelling, [1, above[8] as usize + left[8] as usize, 0], steps[1], &mut averages);
            (above[8], left[8]) = (last > 0, last > 0);
            spread(&averages).iter().enumerate().for_each(|(block, average)| numbers[block * 16] = *average);
        }
        // Then luma and each chroma: each block's place among the nine context columns and rows
        // kept of the neighbours.
        for block in 0..24 {
            let (column, row) = if block < 16 { (block % 4, block / 4) } else { (4 + (block - 16) / 4 * 2 + block % 2, 4 + (block - 16) / 4 * 2 + block % 4 / 2) };
            let (tables, first) = if block >= 16 {
                (2, 0)
            } else if whole {
                (0, 1)
            } else {
                (3, 0)
            };
            let last = spelled(decoder, &self.spelling, [tables, above[column] as usize + left[row] as usize, first], steps[(block >= 16) as usize * 2], &mut numbers[block * 16..][..16]);
            (above[column], left[row]) = (last > first, last > first);
        }
        numbers.iter().any(|number| *number != 0)
    }

    /// Predicts a whole block of a plane (16×16 luma, 8×8 chroma) at `left`, `top`, from the
    /// row above and the column left: DC, their mean (0); TrueMotion, above + left - corner
    /// (1); vertical (2); horizontal (3). Beyond the picture's edge, above is 127 and left 129.
    fn large(&mut self, plane: usize, [left, top]: [usize; 2], way: u8) {
        let (side, wide, pixels) = (if plane == 0 { 16 } else { 8 }, self.across * if plane == 0 { 16 } else { 8 }, &mut self.planes[plane]);
        let above: [i32; 16] = std::array::from_fn(|x| if top == 0 { 127 } else { pixels[(top - 1) * wide + left + x.min(side - 1)] as i32 });
        let beside: [i32; 16] = std::array::from_fn(|y| if left == 0 { 129 } else { pixels[(top + y.min(side - 1)) * wide + left - 1] as i32 });
        let corner = if top == 0 {
            127
        } else if left == 0 {
            129
        } else {
            pixels[(top - 1) * wide + left - 1] as i32
        };
        // (The mean of what exists: of both, of one, or 128 for neither.)
        let sums = [above[..side].iter().sum::<i32>() * (top > 0) as i32, beside[..side].iter().sum::<i32>() * (left > 0) as i32];
        let mean = match (top > 0) as i32 + (left > 0) as i32 {
            2 => (sums[0] + sums[1] + side as i32) / (2 * side as i32),
            1 => (sums[0] + sums[1] + side as i32 / 2) / side as i32,
            _ => 128,
        };
        for at in 0..side * side {
            let (x, y) = (at % side, at / side);
            let guess = match way {
                0 => mean,
                1 => above[x] + beside[y] - corner,
                2 => above[x],
                _ => beside[y],
            };
            pixels[(top + y) * wide + left + x] = guess.clamp(0, 255) as u8;
        }
    }

    /// Predicts a 4×4 luma block at `left`, `top`, in macroblock `block`, one of ten modes:
    /// from the four pixels left, the corner, the four above, and the four after those.
    fn small(&mut self, [left, top]: [usize; 2], [x, y]: [usize; 2], way: u8) {
        let (wide, across, pixels) = (self.across * 16, self.across, &mut self.planes[0]);
        let at = |x: usize, y: usize| pixels[y * wide + x] as i32;
        // The four above-right pixels of a macroblock's last column are always those above the
        // macroblock (the next one's above, or the last of its own repeated), never of the
        // block to the right, not yet decoded.
        let after = |more: usize| match (left % 16 == 12, x + 1 == across) {
            (true, _) if y == 0 => 127,
            (true, true) => at(left + 3, y * 16 - 1),
            (true, false) => at(left + 4 + more, y * 16 - 1),
            _ if top == 0 => 127,
            _ => at(left + 4 + more, top - 1),
        };
        // The edge pixels, from the lowest left through the corner to the last above; with the
        // first and last repeated.
        let edge: [i32; 15] = std::array::from_fn(|place| match place {
            0..5 if left == 0 => 129,
            0..5 => at(left - 1, top + 4 - place.max(1)),
            5 if top == 0 => 127,
            5 if left == 0 => 129,
            5 => at(left - 1, top - 1),
            6..10 if top == 0 => 127,
            6..10 => at(left + place - 6, top - 1),
            _ => after(place.min(13) - 10),
        });
        let mean = (edge[1..5].iter().sum::<i32>() + edge[6..10].iter().sum::<i32>() + 4) >> 3;
        let slants = &SLANTS[way.clamp(2, 9) as usize - 2];
        for place in 0..16 {
            let guess = match way {
                0 => mean,
                1 => edge[4 - place / 4] + edge[6 + place % 4] - edge[5],
                _ => slanted(&edge, slants[place] as usize),
            };
            pixels[(top + place / 4) * wide + left + place % 4] = guess.clamp(0, 255) as u8;
        }
    }

    /// Adds to a 4×4 block of a plane what its sixteen coefficients stand for: the inverse DCT.
    fn add(&mut self, plane: usize, [left, top]: [usize; 2], numbers: &[i32]) {
        if numbers.iter().all(|number| *number == 0) {
            return;
        }
        let (wide, pixels) = (self.across * if plane == 0 { 16 } else { 8 }, &mut self.planes[plane]);
        // One pass down the columns, one along the rows, each four values to four. (A damaged
        // file may hold values larger than any picture has: the result does not matter, only
        // that it does not stop.)
        let turn = |[first, second, third, fourth]: [i32; 4]| {
            let times = |number: i32, by: i32| number.wrapping_mul(by) >> 16;
            let (sum, less) = (first + third, first - third);
            let (cross, straight) = (times(second, 35468) - (fourth + times(fourth, 20091)), second + times(second, 20091) + times(fourth, 35468));
            [sum + straight, less + cross, less - cross, sum - straight]
        };
        let columns: [[i32; 4]; 4] = std::array::from_fn(|x| turn([numbers[x], numbers[4 + x], numbers[8 + x], numbers[12 + x]]));
        for y in 0..4 {
            let row = turn([columns[0][y], columns[1][y], columns[2][y], columns[3][y]]);
            for (pixel, more) in pixels[(top + y) * wide + left..][..4].iter_mut().zip(row) {
                *pixel = (*pixel as i32 + ((more + 4) >> 3)).clamp(0, 255) as u8;
            }
        }
    }

    /// Loop-filters block edges, left hard by prediction and residue: per macroblock its left
    /// edge, inner vertical edges, top edge, and inner horizontal edges.
    fn smooth(&mut self, smoothed: &[Smoothed]) {
        for (at, block) in smoothed.iter().enumerate().filter(|(_, block)| block.across > 0) {
            // Luma, chroma at half size; the simple filter does luma only.
            for (plane, side) in [(0, 16), (1, 8), (2, 8)].into_iter().take(if self.plain { 1 } else { 3 }) {
                self.smooth_one(block, [plane, side], [at % self.across, at / self.across]);
            }
        }
    }

    /// Filters the edges of block `x`, `y` in one plane, blocks `side` pixels square.
    fn smooth_one(&mut self, block: &Smoothed, [plane, side]: [usize; 2], [x, y]: [usize; 2]) {
        let (wide, reach, pixels) = (self.across * side, if self.plain { 0 } else { 1 }, &mut self.planes[plane]);
        let corner = y * side * wide + x * side;
        // Macroblock edges are filtered harder, and deeper, than inner ones.
        for (step, along, between) in [(1, wide, x > 0), (wide, 1, y > 0)] {
            if between {
                edge(pixels, [corner, step, along, side], [2 * (block.across + 4) + 1, block.beside, block.own], 3 * reach);
            }
            for inside in (4..side).step_by(4).filter(|_| block.inside) {
                edge(pixels, [corner + inside * step, step, along, side], [2 * block.across + 1, block.beside, block.own], 2 * reach);
            }
        }
    }

    /// The picture in colour, `size[0]` by `size[1]`: luma with both chroma planes upsampled
    /// (see `upsample`), row by row.
    fn colours(&self, [wide, high]: [usize; 2]) -> Vec<u32> {
        let (full, half) = (self.across * 16, self.across * 8);
        let (mut pixels, mut blue, mut red) = (vec![0; wide * high], vec![0; wide], vec![0; wide]);
        for (y, row) in pixels.chunks_exact_mut(wide).enumerate() {
            let [near, far] = nearest(y, high).map(|y| y * half);
            upsample([&self.planes[1][near..][..half], &self.planes[1][far..][..half]], &mut blue);
            upsample([&self.planes[2][near..][..half], &self.planes[2][far..][..half]], &mut red);
            for (((pixel, bright), blue), red) in row.iter_mut().zip(&self.planes[0][y * full..][..wide]).zip(&blue).zip(&red) {
                *pixel = rgb(*bright as i32, *blue, *red);
            }
        }
        pixels
    }
}

/// The stored chroma sample nearest pixel `at` of `count` along one axis, and the next nearest
/// (at the edges, the nearest again).
fn nearest(at: usize, count: usize) -> [usize; 2] {
    [at / 2, if at % 2 == 1 { (at / 2 + 1).min(count.div_ceil(2) - 1) } else { (at / 2).saturating_sub(1) }]
}

/// One row of a chroma plane at full width, from its nearest stored row and the next nearest:
/// each pixel three parts the nearest stored sample and one the next, across and down, as
/// libwebp's "fancy" upsampling rounds it.
fn upsample([near, far]: [&[u8]; 2], row: &mut [i32]) {
    let last = row.len().div_ceil(2) - 1;
    let mix = |at: usize, next: usize| {
        let [own, beside, below, across] = [near[at], near[next], far[at], far[next]].map(i32::from);
        (((own + beside + below + across + 8 + 2 * (beside + below)) >> 3) + own) >> 1
    };
    // The first pixel is the first sample's; then each pair lies between two samples.
    row[0] = mix(0, 0);
    for (at, pair) in row[1..].chunks_mut(2).enumerate() {
        pair[0] = mix(at, (at + 1).min(last));
        if let Some(second) = pair.get_mut(1) {
            *second = mix(at + 1, at);
        }
    }
}

/// A pixel from luma and chroma, in BT.601's integer steps as libwebp takes them.
#[inline(always)]
fn rgb(bright: i32, blue: i32, red: i32) -> u32 {
    let (held, lit) = (|value: i32| (value >> 6).clamp(0, 255) as u32, (bright * 19077) >> 8);
    let (r, g, b) = (lit + ((red * 26149) >> 8) - 14234, lit - ((blue * 6419) >> 8) - ((red * 13320) >> 8) + 8708, lit + ((blue * 33050) >> 8) - 17685);
    0xff00_0000 | held(r) << 16 | held(g) << 8 | held(b)
}

/// A pixel of a slanted 4×4 mode (see `SLANTS`): an edge pixel averaged with its two
/// neighbours, or from 16 on, the mean of one and the next.
fn slanted(edge: &[i32; 15], slant: usize) -> i32 {
    let at = slant & 15;
    match slant >= 16 {
        true => (edge[at] + edge[at + 1] + 1) >> 1,
        false => (edge[at.max(1) - 1] + 2 * edge[at] + edge[at + 1] + 2) >> 2,
    }
}

/// Reads a 4×4 intra mode, up to nine bits: `likely` gives each one's probability of 0.
fn way(decoder: &mut BoolDecoder, likely: &[u8]) -> u8 {
    let mut no = |question: usize| !decoder.yes(likely[question] as u32);
    match () {
        _ if no(0) => 0,
        _ if no(1) => 1,
        _ if no(2) => 2,
        _ if no(3) => {
            if no(4) {
                3
            } else if no(5) {
                5
            } else {
                6
            }
        }
        _ if no(6) => 4,
        _ if no(7) => 7,
        _ if no(8) => 8,
        _ => 9,
    }
}

/// Reads a 4×4 block's coefficients from position `first` on, into `numbers`, each dequantized
/// (`steps`: DC and AC). Probabilities go by block type (`tables`), band, and the previous
/// coefficient (zero, one, more); for the first, by `around` (how many of the two neighbours
/// were non-zero). Returns the position after the last non-zero coefficient.
fn spelled(decoder: &mut BoolDecoder, spelling: &[u8; 1056], [tables, around, first]: [usize; 3], steps: [i32; 2], numbers: &mut [i32]) -> usize {
    // Eight bands of three contexts of eleven probabilities.
    let bands = &spelling.as_chunks::<11>().0[tables * 24..][..24];
    let likely = |place: usize, before: usize| &bands[STRETCH[place] * 3 + before];
    let (mut place, mut no) = (first, likely(first, around));
    while place < 16 {
        // Is there another non-zero coefficient? And how many zeros before it?
        if !decoder.yes(no[0] as u32) {
            return place;
        }
        while !decoder.yes(no[1] as u32) {
            place += 1;
            if place == 16 {
                return 16;
            }
            no = likely(place, 0);
        }
        // One, or more: up to ten directly, beyond that a base plus extra bits.
        let size = match () {
            _ if !decoder.yes(no[2] as u32) => 1,
            _ if !decoder.yes(no[3] as u32) => {
                let more = decoder.yes(no[4] as u32);
                2 + more as i32 + (more && decoder.yes(no[5] as u32)) as i32
            }
            _ if !decoder.yes(no[6] as u32) => {
                let more = decoder.yes(no[7] as u32);
                let (first, second) = (decoder.yes(if more { 165 } else { 159 }) as i32, (more && decoder.yes(145)) as i32);
                5 + first + more as i32 * (2 + first + second)
            }
            _ => {
                let range = 2 * decoder.yes(no[8] as u32) as usize;
                let range = range + decoder.yes(no[9 + range / 2] as u32) as usize;
                LARGE[range].iter().fold(0, |more, likely| 2 * more + decoder.yes(*likely as u32) as i32) + 3 + (8 << range)
            }
        };
        no = likely(place + 1, if size == 1 { 1 } else { 2 });
        // (Kept in sixteen bits, as every reader does: a damaged file's larger value wraps.)
        numbers[ZIGZAG[place]] = (if decoder.yes(128) { -size } else { size } * steps[(place > 0) as usize]) as i16 as i32;
        place += 1;
    }
    16
}

/// The inverse WHT: a macroblock's sixteen DC values from its Y2 coefficients, by sums and
/// differences down the columns and along the rows.
fn spread(numbers: &[i32; 16]) -> [i32; 16] {
    let turn = |[first, second, third, fourth]: [i32; 4]| [first + fourth + second + third, second - third + first - fourth, first + fourth - second - third, first - fourth - second + third];
    let columns: [[i32; 4]; 4] = std::array::from_fn(|x| turn([numbers[x], numbers[4 + x], numbers[8 + x], numbers[12 + x]]));
    let rows: [[i32; 4]; 4] = std::array::from_fn(|y| turn([columns[0][y], columns[1][y], columns[2][y], columns[3][y]]));
    std::array::from_fn(|at| ((rows[at / 4][at % 4] + 3) >> 3) as i16 as i32)
}

/// Filters the pixels across one block edge. The first pixel after the edge is at `at`, pixels
/// across it lie `step` apart, and the edge runs `along` from one to the next, `count` times.
/// `[across, beside, own]` are the edge limit, the interior limit, and the high edge variance
/// threshold above which the edge is kept sharper. `reach` is how many pixels each side may
/// change: three, two, or only the two at the edge (the simple filter).
fn edge(pixels: &mut [u8], [at, step, along, count]: [usize; 4], [across, beside, own]: [i32; 3], reach: usize) {
    for at in (0..count).map(|place| at + place * along) {
        // The pixels across the edge, four each side; and their differences.
        let near: [i32; 8] = std::array::from_fn(|place| if reach == 0 && !(2..6).contains(&place) { 0 } else { pixels[at + place * step - 4 * step] as i32 });
        let apart = |one: usize, other: usize| (near[one] - near[other]).abs();
        let flat = reach == 0 || [(0, 1), (1, 2), (2, 3), (7, 6), (6, 5), (5, 4)].iter().all(|(one, other)| apart(*one, *other) <= beside);
        if 4 * apart(3, 4) + apart(2, 5) > across || !flat {
            continue;
        }
        let sharp = reach == 0 || apart(2, 3) > own || apart(5, 4) > own;
        // How far the two edge pixels move toward each other, and from that the ones behind
        // them.
        let step_up = 3 * (near[4] - near[3]) + if sharp || reach == 3 { (near[2] - near[5]).clamp(-128, 127) } else { 0 };
        let moves = match (sharp, reach) {
            (true, _) => [0, 0, ((step_up + 3) >> 3).clamp(-16, 15), ((step_up + 4) >> 3).clamp(-16, 15), 0, 0],
            (false, 3) => [9, 18, 27, 27, 18, 9].map(|part| (part * step_up.clamp(-128, 127) + 63) >> 7),
            _ => {
                let (first, second) = (((step_up + 4) >> 3).clamp(-16, 15), ((step_up + 3) >> 3).clamp(-16, 15));
                [0, (first + 1) >> 1, second, first, (first + 1) >> 1, 0]
            }
        };
        for (place, by) in moves.into_iter().enumerate().filter(|(_, by)| *by != 0) {
            pixels[at + place * step - 3 * step] = (near[place + 1] + if place < 3 { by } else { -by }).clamp(0, 255) as u8;
        }
    }
}

/// A lossy picture's alpha, a byte per pixel, from its chunk: raw or lossless-packed (in
/// green); and filtered horizontally, vertically or by gradient. Nothing if damaged: the
/// picture is then opaque.
fn solidity(chunk: &[u8], [wide, high]: [usize; 2]) -> Option<Vec<u8>> {
    let (header, rest) = chunk.split_first()?;
    let mut solid: Vec<u8> = match header & 3 {
        0 => rest.get(..wide * high)?.to_vec(),
        1 => picture(&mut Bits::of(rest), [wide, high])?.iter().map(|pixel| (pixel >> 8) as u8).collect(),
        _ => return None,
    };
    for at in (1..wide * high).filter(|_| header >> 2 & 3 != 0) {
        let before = |back: usize| solid[at - back] as i32;
        solid[at] = solid[at].wrapping_add(match (header >> 2 & 3, at < wide, at % wide == 0) {
            (_, true, _) | (1, _, false) => before(1),
            (_, _, true) | (2, ..) => before(wide),
            _ => (before(1) + before(wide) - before(wide + 1)).clamp(0, 255),
        } as u8);
    }
    Some(solid)
}

/// The 4×4 intra mode probabilities by the modes above and left: nine for the nine tree bits,
/// for each of ten modes above and ten left.
const WAYS: [u8; 900] = [
    231, 120, 48, 89, 115, 113, 120, 152, 112, 152, 179, 64, 126, 170, 118, 46, 70, 95, 175, 69, 143, 80, 85, 82, 72, 155, 103, 56, 58, 10, 171, 218, 189, 17, 13, 152, 144, 71, 10, 38, 171, 213, 144,
    34, 26, 114, 26, 17, 163, 44, 195, 21, 10, 173, 121, 24, 80, 195, 26, 62, 44, 64, 85, 170, 46, 55, 19, 136, 160, 33, 206, 71, 63, 20, 8, 114, 114, 208, 12, 9, 226, 81, 40, 11, 96, 182, 84, 29,
    16, 36, 134, 183, 89, 137, 98, 101, 106, 165, 148, 72, 187, 100, 130, 157, 111, 32, 75, 80, 66, 102, 167, 99, 74, 62, 40, 234, 128, 41, 53, 9, 178, 241, 141, 26, 8, 107, 104, 79, 12, 27, 217,
    255, 87, 17, 7, 74, 43, 26, 146, 73, 166, 49, 23, 157, 65, 38, 105, 160, 51, 52, 31, 115, 128, 87, 68, 71, 44, 114, 51, 15, 186, 23, 47, 41, 14, 110, 182, 183, 21, 17, 194, 66, 45, 25, 102, 197,
    189, 23, 18, 22, 88, 88, 147, 150, 42, 46, 45, 196, 205, 43, 97, 183, 117, 85, 38, 35, 179, 61, 39, 53, 200, 87, 26, 21, 43, 232, 171, 56, 34, 51, 104, 114, 102, 29, 93, 77, 107, 54, 32, 26, 51,
    1, 81, 43, 31, 39, 28, 85, 171, 58, 165, 90, 98, 64, 34, 22, 116, 206, 23, 34, 43, 166, 73, 68, 25, 106, 22, 64, 171, 36, 225, 114, 34, 19, 21, 102, 132, 188, 16, 76, 124, 62, 18, 78, 95, 85, 57,
    50, 48, 51, 193, 101, 35, 159, 215, 111, 89, 46, 111, 60, 148, 31, 172, 219, 228, 21, 18, 111, 112, 113, 77, 85, 179, 255, 38, 120, 114, 40, 42, 1, 196, 245, 209, 10, 25, 109, 100, 80, 8, 43,
    154, 1, 51, 26, 71, 88, 43, 29, 140, 166, 213, 37, 43, 154, 61, 63, 30, 155, 67, 45, 68, 1, 209, 142, 78, 78, 16, 255, 128, 34, 197, 171, 41, 40, 5, 102, 211, 183, 4, 1, 221, 51, 50, 17, 168,
    209, 192, 23, 25, 82, 125, 98, 42, 88, 104, 85, 117, 175, 82, 95, 84, 53, 89, 128, 100, 113, 101, 45, 75, 79, 123, 47, 51, 128, 81, 171, 1, 57, 17, 5, 71, 102, 57, 53, 41, 49, 115, 21, 2, 10,
    102, 255, 166, 23, 6, 38, 33, 13, 121, 57, 73, 26, 1, 85, 41, 10, 67, 138, 77, 110, 90, 47, 114, 101, 29, 16, 10, 85, 128, 101, 196, 26, 57, 18, 10, 102, 102, 213, 34, 20, 43, 117, 20, 15, 36,
    163, 128, 68, 1, 26, 138, 31, 36, 171, 27, 166, 38, 44, 229, 67, 87, 58, 169, 82, 115, 26, 59, 179, 63, 59, 90, 180, 59, 166, 93, 73, 154, 40, 40, 21, 116, 143, 209, 34, 39, 175, 57, 46, 22, 24,
    128, 1, 54, 17, 37, 47, 15, 16, 183, 34, 223, 49, 45, 183, 46, 17, 33, 183, 6, 98, 15, 32, 183, 65, 32, 73, 115, 28, 128, 23, 128, 205, 40, 3, 9, 115, 51, 192, 18, 6, 223, 87, 37, 9, 115, 59, 77,
    64, 21, 47, 104, 55, 44, 218, 9, 54, 53, 130, 226, 64, 90, 70, 205, 40, 41, 23, 26, 57, 54, 57, 112, 184, 5, 41, 38, 166, 213, 30, 34, 26, 133, 152, 116, 10, 32, 134, 75, 32, 12, 51, 192, 255,
    160, 43, 51, 39, 19, 53, 221, 26, 114, 32, 73, 255, 31, 9, 65, 234, 2, 15, 1, 118, 73, 88, 31, 35, 67, 102, 85, 55, 186, 85, 56, 21, 23, 111, 59, 205, 45, 37, 192, 55, 38, 70, 124, 73, 102, 1,
    34, 98, 102, 61, 71, 37, 34, 53, 31, 243, 192, 69, 60, 71, 38, 73, 119, 28, 222, 37, 68, 45, 128, 34, 1, 47, 11, 245, 171, 62, 17, 19, 70, 146, 85, 55, 62, 70, 75, 15, 9, 9, 64, 255, 184, 119,
    16, 37, 43, 37, 154, 100, 163, 85, 160, 1, 63, 9, 92, 136, 28, 64, 32, 201, 85, 86, 6, 28, 5, 64, 255, 25, 248, 1, 56, 8, 17, 132, 137, 255, 55, 116, 128, 58, 15, 20, 82, 135, 57, 26, 121, 40,
    164, 50, 31, 137, 154, 133, 25, 35, 218, 51, 103, 44, 131, 131, 123, 31, 6, 158, 86, 40, 64, 135, 148, 224, 45, 183, 128, 22, 26, 17, 131, 240, 154, 14, 1, 209, 83, 12, 13, 54, 192, 255, 68, 47,
    28, 45, 16, 21, 91, 64, 222, 7, 1, 197, 56, 21, 39, 155, 60, 138, 23, 102, 213, 85, 26, 85, 85, 128, 128, 32, 146, 171, 18, 11, 7, 63, 144, 171, 4, 4, 246, 35, 27, 10, 146, 174, 171, 12, 26, 128,
    190, 80, 35, 99, 180, 80, 126, 54, 45, 85, 126, 47, 87, 176, 51, 41, 20, 32, 101, 75, 128, 139, 118, 146, 116, 128, 85, 56, 41, 15, 176, 236, 85, 37, 9, 62, 146, 36, 19, 30, 171, 255, 97, 27, 20,
    71, 30, 17, 119, 118, 255, 17, 18, 138, 101, 38, 60, 138, 55, 70, 43, 26, 142, 138, 45, 61, 62, 219, 1, 81, 188, 64, 32, 41, 20, 117, 151, 142, 20, 21, 163, 112, 19, 12, 61, 195, 128, 48, 4, 24,
];

/// The default coefficient probabilities: eleven tree bits, by the previous coefficients'
/// context (zero, one, more), by band of eight, by block type of four.
const USUAL: [u8; 1056] = [
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 253, 136, 254, 255, 228, 219,
    128, 128, 128, 128, 128, 189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128, 106, 126, 227, 252, 214, 209, 255, 255, 128, 128, 128, 1, 98, 248, 255, 236, 226, 255, 255, 128, 128, 128, 181,
    133, 238, 254, 221, 234, 255, 154, 128, 128, 128, 78, 134, 202, 247, 198, 180, 255, 219, 128, 128, 128, 1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128, 184, 150, 247, 255, 236, 224, 128,
    128, 128, 128, 128, 77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128, 1, 101, 251, 255, 241, 255, 128, 128, 128, 128, 128, 170, 139, 241, 252, 236, 209, 255, 255, 128, 128, 128, 37, 116, 196,
    243, 228, 255, 255, 255, 128, 128, 128, 1, 204, 254, 255, 245, 255, 128, 128, 128, 128, 128, 207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128, 102, 103, 231, 255, 211, 171, 128, 128, 128,
    128, 128, 1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128, 177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128, 80, 129, 211, 255, 194, 224, 128, 128, 128, 128, 128, 1, 1, 255, 128, 128,
    128, 128, 128, 128, 128, 128, 246, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 198, 35, 237, 223, 193, 187, 162, 160, 145, 155, 62, 131,
    45, 198, 221, 172, 176, 220, 157, 252, 221, 1, 68, 47, 146, 208, 149, 167, 221, 162, 255, 223, 128, 1, 149, 241, 255, 221, 224, 255, 255, 128, 128, 128, 184, 141, 234, 253, 222, 220, 255, 199,
    128, 128, 128, 81, 99, 181, 242, 176, 190, 249, 202, 255, 255, 128, 1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128, 99, 121, 210, 250, 201, 198, 255, 202, 128, 128, 128, 23, 91, 163, 242,
    170, 187, 247, 210, 255, 255, 128, 1, 200, 246, 255, 234, 255, 128, 128, 128, 128, 128, 109, 178, 241, 255, 231, 245, 255, 255, 128, 128, 128, 44, 130, 201, 253, 205, 192, 255, 255, 128, 128,
    128, 1, 132, 239, 251, 219, 209, 255, 165, 128, 128, 128, 94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128, 22, 100, 174, 245, 186, 161, 255, 199, 128, 128, 128, 1, 182, 249, 255, 232, 235,
    128, 128, 128, 128, 128, 124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128, 35, 77, 181, 251, 193, 211, 255, 205, 128, 128, 128, 1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128, 121, 141,
    235, 255, 225, 227, 255, 255, 128, 128, 128, 45, 99, 188, 251, 195, 217, 255, 224, 128, 128, 128, 1, 1, 251, 255, 213, 255, 128, 128, 128, 128, 128, 203, 1, 248, 255, 255, 128, 128, 128, 128,
    128, 128, 137, 1, 177, 255, 224, 255, 128, 128, 128, 128, 128, 253, 9, 248, 251, 207, 208, 255, 192, 128, 128, 128, 175, 13, 224, 243, 193, 185, 249, 198, 255, 255, 128, 73, 17, 171, 221, 161,
    179, 236, 167, 255, 234, 128, 1, 95, 247, 253, 212, 183, 255, 255, 128, 128, 128, 239, 90, 244, 250, 211, 209, 255, 255, 128, 128, 128, 155, 77, 195, 248, 188, 195, 255, 255, 128, 128, 128, 1,
    24, 239, 251, 218, 219, 255, 205, 128, 128, 128, 201, 51, 219, 255, 196, 186, 128, 128, 128, 128, 128, 69, 46, 190, 239, 201, 218, 255, 228, 128, 128, 128, 1, 191, 251, 255, 255, 128, 128, 128,
    128, 128, 128, 223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128, 141, 124, 248, 255, 255, 128, 128, 128, 128, 128, 128, 1, 16, 248, 255, 255, 128, 128, 128, 128, 128, 128, 190, 36, 230, 255,
    236, 255, 128, 128, 128, 128, 128, 149, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 1, 226, 255, 128, 128, 128, 128, 128, 128, 128, 128, 247, 192, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128, 1, 134, 252, 255, 255, 128, 128, 128, 128, 128, 128, 213, 62, 250, 255, 255, 128, 128, 128, 128, 128, 128, 55, 93, 255, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 202, 24,
    213, 235, 186, 191, 220, 160, 240, 175, 255, 126, 38, 182, 232, 169, 184, 228, 174, 255, 187, 128, 61, 46, 138, 219, 151, 178, 240, 170, 255, 216, 128, 1, 112, 230, 250, 199, 191, 247, 159, 255,
    255, 128, 166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128, 39, 77, 162, 232, 172, 180, 245, 178, 255, 255, 128, 1, 52, 220, 246, 198, 199, 249, 220, 255, 255, 128, 124, 74, 191, 243, 183,
    193, 250, 221, 255, 255, 128, 24, 71, 130, 219, 154, 170, 243, 182, 255, 255, 128, 1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128, 149, 150, 226, 252, 216, 205, 255, 171, 128, 128, 128, 28,
    108, 170, 242, 183, 194, 254, 223, 255, 255, 128, 1, 81, 230, 252, 204, 203, 255, 192, 128, 128, 128, 123, 102, 209, 247, 188, 196, 255, 233, 128, 128, 128, 20, 95, 153, 243, 164, 173, 255, 203,
    128, 128, 128, 1, 222, 248, 255, 216, 213, 128, 128, 128, 128, 128, 168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128, 47, 116, 215, 255, 211, 212, 255, 255, 128, 128, 128, 1, 121, 236, 253,
    212, 214, 255, 255, 128, 128, 128, 141, 84, 213, 252, 201, 202, 255, 219, 128, 128, 128, 42, 80, 160, 240, 162, 185, 255, 205, 128, 128, 128, 1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    244, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 238, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
];

/// The probability that a file updates each of those, in this order.
const CHANGED: [u8; 1056] = [
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 176, 246, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255, 249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255, 234,
    254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255, 239, 253, 254, 255, 255, 255, 255,
    255, 255, 255, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 254, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255, 250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 217, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255, 234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 223, 254, 254, 255,
    255, 255, 255, 255, 255, 255, 255, 238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255, 249, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 253, 255, 255, 255,
    255, 255, 255, 255, 255, 250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255, 234, 251, 244, 254, 255, 255, 255,
    255, 255, 255, 255, 251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255, 255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 253,
    253, 254, 254, 255, 255, 255, 255, 255, 255, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255, 248, 254, 249, 253, 255,
    255, 255, 255, 255, 255, 255, 255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255, 246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255, 252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255,
    255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255, 248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 254, 255, 255, 255,
    255, 255, 255, 255, 255, 245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255, 253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255, 252,
    253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 249, 255, 254, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255,
];

/// The DC dequantization factor for each of 128 quantizer levels; and the AC one.
const FIRST: [u16; 128] = [
    4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17, 18, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 25, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 45,
    46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 91, 93, 95,
    96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118, 122, 124, 126, 128, 130, 132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
];
const OTHERS: [u16; 128] = [
    4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
    54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140, 143, 146,
    149, 152, 155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205, 209, 213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// The lossless test files, each beside libwebp's decode (through vips; ffmpeg's own reader
    /// agrees on each).
    const EXACT: [(&str, &[u8], &[u8]); 10] = [
        // A photo-like 61×45 by vips at maximum effort, using everything: prediction, colour
        // transform, many code groups, a cache.
        ("exact", include_bytes!("../tests/pictures/exact.webp"), include_bytes!("../tests/pictures/exact.webp.png")),
        // The same at no effort; and by ffmpeg, which configures libwebp differently.
        ("exact-quick", include_bytes!("../tests/pictures/exact-quick.webp"), include_bytes!("../tests/pictures/exact.webp.png")),
        ("exact-ffmpeg", include_bytes!("../tests/pictures/exact-ffmpeg.webp"), include_bytes!("../tests/pictures/exact.webp.png")),
        // With more and less transparency.
        ("exact-clear", include_bytes!("../tests/pictures/exact-clear.webp"), include_bytes!("../tests/pictures/exact-clear.webp.png")),
        // Two, four, sixteen and a hundred colours: palettes with eight, four, two and one
        // index per pixel.
        ("exact-2", include_bytes!("../tests/pictures/exact-2.webp"), include_bytes!("../tests/pictures/exact-2.webp.png")),
        ("exact-4", include_bytes!("../tests/pictures/exact-4.webp"), include_bytes!("../tests/pictures/exact-4.webp.png")),
        ("exact-16", include_bytes!("../tests/pictures/exact-16.webp"), include_bytes!("../tests/pictures/exact-16.webp.png")),
        ("exact-100", include_bytes!("../tests/pictures/exact-100.webp"), include_bytes!("../tests/pictures/exact-100.webp.png")),
        // One colour all over: no codes at all.
        ("exact-flat", include_bytes!("../tests/pictures/exact-flat.webp"), include_bytes!("../tests/pictures/exact-flat.webp.png")),
        // Squares each graded a different way: many predictors.
        ("exact-weave", include_bytes!("../tests/pictures/exact-weave.webp"), include_bytes!("../tests/pictures/exact-weave.webp.png")),
    ];

    /// Pixels as seen: transparent is transparent, whatever its colour.
    fn seen(pixels: &[u32]) -> Vec<u32> {
        pixels.iter().map(|pixel| if pixel >> 24 == 0 { 0 } else { *pixel }).collect()
    }

    #[test]
    fn pictures_without_loss_come_out_pixel_for_pixel() {
        for (name, webp, wanted) in EXACT {
            let (width, pixels) = decode(webp).unwrap_or_else(|| panic!("{name}.webp is read"));
            let (wanted_width, wanted) = png::decode(wanted).unwrap();
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name}: its size");
            assert!(seen(&pixels) == seen(&wanted), "{name}: its pixels");
        }
    }

    /// The lossy test files, each beside libwebp's decode (through vips).
    const LOSSY: [(&str, &[u8], &[u8]); 13] = [
        // A photo-like 61×45 (not whole macroblocks) by vips: default, coarse, and fine
        // quality.
        ("lossy", include_bytes!("../tests/pictures/lossy.webp"), include_bytes!("../tests/pictures/lossy.webp.png")),
        ("lossy-coarse", include_bytes!("../tests/pictures/lossy-coarse.webp"), include_bytes!("../tests/pictures/lossy-coarse.webp.png")),
        ("lossy-fine", include_bytes!("../tests/pictures/lossy-fine.webp"), include_bytes!("../tests/pictures/lossy-fine.webp.png")),
        // With alpha in its own chunk.
        ("lossy-clear", include_bytes!("../tests/pictures/lossy-clear.webp"), include_bytes!("../tests/pictures/lossy-clear.webp.png")),
        // Graded squares, and as ffmpeg asks for it.
        ("lossy-weave", include_bytes!("../tests/pictures/lossy-weave.webp"), include_bytes!("../tests/pictures/lossy-weave.webp.png")),
        ("lossy-ffmpeg", include_bytes!("../tests/pictures/lossy-ffmpeg.webp"), include_bytes!("../tests/pictures/lossy-ffmpeg.webp.png")),
        // By another encoder, libvpx, using what libwebp never does: filter deltas by
        // prediction mode, four partitions, and the simple filter.
        ("lossy-libvpx", include_bytes!("../tests/pictures/lossy-libvpx.webp"), include_bytes!("../tests/pictures/lossy-libvpx.webp.png")),
        ("lossy-parts", include_bytes!("../tests/pictures/lossy-parts.webp"), include_bytes!("../tests/pictures/lossy-parts.webp.png")),
        ("lossy-plain", include_bytes!("../tests/pictures/lossy-plain.webp"), include_bytes!("../tests/pictures/lossy-plain.webp.png")),
        // Alpha stored raw: unfiltered, and filtered horizontally, vertically and by gradient.
        ("lossy-clear-raw", include_bytes!("../tests/pictures/lossy-clear-raw.webp"), include_bytes!("../tests/pictures/lossy-clear-raw.webp.png")),
        ("lossy-clear-across", include_bytes!("../tests/pictures/lossy-clear-across.webp"), include_bytes!("../tests/pictures/lossy-clear-across.webp.png")),
        ("lossy-clear-down", include_bytes!("../tests/pictures/lossy-clear-down.webp"), include_bytes!("../tests/pictures/lossy-clear-down.webp.png")),
        ("lossy-clear-both", include_bytes!("../tests/pictures/lossy-clear-both.webp"), include_bytes!("../tests/pictures/lossy-clear-both.webp.png")),
    ];

    #[test]
    fn pictures_with_loss_come_out_as_libwebp_reads_them() {
        // Lossy decoding is specified bit-exactly up to the planes, and from there to colours
        // this follows libwebp's own steps: so nothing may differ here either.
        for (name, webp, wanted) in LOSSY {
            let (width, pixels) = decode(webp).unwrap_or_else(|| panic!("{name}.webp is read"));
            let (wanted_width, wanted) = png::decode(wanted).unwrap();
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name}: its size");
            let worst = pixels.iter().zip(&wanted).flat_map(|(one, other)| [24, 16, 8, 0].map(|part| (one >> part & 255).abs_diff(other >> part & 255))).max();
            let differ = pixels.iter().zip(&wanted).filter(|(one, other)| one != other).count();
            assert!(seen(&pixels) == seen(&wanted), "{name}: {differ} of its {} pixels differ, by up to {worst:?}", pixels.len());
        }
    }

    #[test]
    fn a_moving_picture_is_read_as_its_first_frame() {
        // Three frames of two, four and sixteen colours, by ffmpeg: the first is also the
        // two-colour still picture.
        let (moving, still) = (decode(include_bytes!("../tests/pictures/moving.webp")), png::decode(include_bytes!("../tests/pictures/exact-2.webp.png")));
        assert!(moving.is_some() && moving == still);
    }

    #[test]
    fn every_rule_for_guessing_a_pixel_is_undone() {
        // The files use six of the fourteen predictors. Here each is checked against its
        // definition, channel by channel: a 5×4 picture of random pixels, stored as differences
        // from each prediction.
        let noise = |at: usize| (at as u32 + 3).wrapping_mul(2_654_435_761) ^ (at as u32 * 40_503) << 7;
        let (wide, picture): (usize, Vec<u32>) = (5, (0..20).map(noise).collect());
        let part = |pixel: u32, shift: u32| (pixel >> shift & 255) as i32;
        let pixel = |parts: [i32; 4]| parts.iter().enumerate().fold(0, |pixel, (place, part)| pixel | (*part as u32 & 255) << (place * 8));
        let each = |value: &dyn Fn(u32) -> i32| pixel([0, 8, 16, 24].map(value));
        for rule in 0..16 {
            // The prediction from the neighbours: left, above, above-right, above-left.
            let guess = |[left, above, right, corner]: [u32; 4]| {
                let half = |one: u32, other: u32| each(&|shift| (part(one, shift) + part(other, shift)) / 2);
                let far = |from: u32| (0..4).map(|place| (part(from, place * 8) - part(corner, place * 8)).abs()).sum::<i32>();
                match rule {
                    1 => left,
                    2 => above,
                    3 => right,
                    4 => corner,
                    5 => half(half(left, right), above),
                    6 => half(left, corner),
                    7 => half(left, above),
                    8 => half(corner, above),
                    9 => half(above, right),
                    10 => half(half(left, corner), half(above, right)),
                    11 => {
                        if far(above) < far(left) {
                            left
                        } else {
                            above
                        }
                    }
                    12 => each(&|shift| (part(left, shift) + part(above, shift) - part(corner, shift)).clamp(0, 255)),
                    13 => each(&|shift| (part(half(left, above), shift) + (part(half(left, above), shift) - part(corner, shift)) / 2).clamp(0, 255)),
                    _ => 0xff00_0000,
                }
            };
            let guessed = |at: usize| match (at, at % wide) {
                (0, _) => 0xff00_0000,
                (1..5, _) => picture[at - 1],
                (_, 0) => picture[at - wide],
                _ => guess([picture[at - 1], picture[at - wide], picture[at - wide + 1], picture[at - wide - 1]]),
            };
            let mut stored: Vec<u32> = (0..20).map(|at| each(&|shift| part(picture[at], shift) - part(guessed(at), shift))).collect();
            unguess(&mut stored, wide, |_| rule);
            assert!(stored == picture, "rule {rule}");
        }
    }

    #[test]
    fn what_is_no_webp_or_too_good_to_be_true_gives_nothing() {
        let (exact, lossy) = (EXACT[0].1, LOSSY[0].1);
        assert_eq!([decode(b"not a picture"), decode(&[]), decode(&exact[..11]), decode(include_bytes!("../tests/pictures/plain.png"))], [None, None, None, None]);
        let changed = |file: &[u8], at: usize, to: &[u8]| decode(&[&file[..at], to, &file[at + to.len()..]].concat());
        // Another name where "WEBP" stands; and a file whose chunks hold no picture.
        let (first, frame) = (exact.windows(4).position(|name| name == b"VP8L").unwrap() + 8, lossy.windows(4).position(|name| name == b"VP8 ").unwrap() + 8);
        assert!(changed(exact, 8, b"WAVE").is_none() && changed(exact, first - 8, b"JUNK").is_none() && changed(lossy, frame - 8, b"JUNK").is_none());
        // A lossless picture's size is in its first four bytes after a signature; a lossy one's
        // in four bytes after six. Each at the format's maximum, 16,384 and 16,383 pixels a
        // side: in files this small, refused before memory is allocated.
        assert!(decode(exact).is_some() && changed(exact, first + 1, &[0xff, 0xff, 0xff, 0x0f]).is_none());
        assert!(decode(lossy).is_some() && changed(lossy, frame + 6, &[0xff, 0x3f, 0xff, 0x3f]).is_none());
        // And a lossy one whose frame tag says it is an inter frame.
        assert!(changed(lossy, frame, &[lossy[frame] | 1]).is_none());
    }

    #[test]
    fn a_damaged_file_gives_nothing_or_some_picture() {
        let sound = |read: Option<(usize, Vec<u32>)>| read.is_none_or(|(width, pixels)| width > 0 && !pixels.is_empty() && pixels.len() % width == 0);
        // One or two of each: lossless using everything, with a palette, one colour; lossy with
        // alpha, by ffmpeg, four partitions, simple filter, raw alpha.
        for (name, webp, _) in [EXACT[0], EXACT[3], EXACT[5], EXACT[8], LOSSY[0], LOSSY[3], LOSSY[5], LOSSY[7], LOSSY[8], LOSSY[12]] {
            // Truncated at every length; then whole with each byte in turn changed: a bit
            // flipped, zero, and 255.
            assert!((0..webp.len()).all(|cut| sound(decode(&webp[..cut]))), "{name}, cut short");
            for at in 0..webp.len() {
                let damaged = |to: u8| decode(&[&webp[..at], &[to], &webp[at + 1..]].concat());
                assert!([webp[at] ^ (1 << (at % 8)), 0, 255].into_iter().all(|to| sound(damaged(to))), "{name}, byte {at}");
            }
        }
        // A lossless picture truncated anywhere before its last byte is none. (Trailing data
        // can go.)
        let exact = EXACT[0].1;
        let first = exact.windows(4).position(|name| name == b"VP8L").unwrap() + 8;
        let last = first + u32::from_le_bytes([exact[first - 4], exact[first - 3], exact[first - 2], exact[first - 1]]) as usize;
        assert!((0..last - 2).all(|cut| decode(&exact[..cut]).is_none()) && decode(&exact[..last]).is_some());
    }

    /// The test pictures: `kind` says which, `size` how large.
    fn pattern(kind: &str, [wide, high]: [usize; 2]) -> Vec<u32> {
        let noise = |at: usize| (at as u32).wrapping_mul(2_654_435_761) >> 24;
        let pixel = |at: usize| {
            let (x, y) = ((at % wide) as u32, (at / wide) as u32);
            match kind {
                // Smooth here, rough there, with an edge and a flat block.
                "photo" if x > 40 && y < 12 => 0xff20_60c0,
                "photo" => 0xff00_0000 | (x * 4 + noise(at) / 8).min(255) << 16 | (y * 5 + if x > y { 40 } else { 0 }).min(255) << 8 | 255u32.saturating_sub(x * 3 + y) | (noise(at * 7) / 16),
                "clear" => ((x * 6).min(255) << 24) | (y * 8).min(255) << 16 | (noise(at) & 0xf0) << 8 | (x ^ y) << 3 & 255,
                "flat" => 0xff33_6699,
                // 16-pixel squares graded differently: across, down, diagonally, radially, in
                // steps, in stripes; some noisy.
                "weave" => {
                    let (u, v, square) = (x % 16, y % 16, x / 16 + y / 16 * 6);
                    let level = [u * 16, v * 16, (u + v) * 8, (15 + u - v) * 8, (u * u + v * v) / 2, (u / 4) * 60, (v % 2) * 200, u * v][square as usize % 8].min(255);
                    let rough = if square % 3 == 2 { noise(at) / 32 } else { 0 };
                    0xff00_0000 | (level + rough).min(255) << 16 | (255 - level) << 8 | (level / 2 + square * 9) & 255
                }
                // So many colours, in blocks and stripes.
                _ => {
                    let colours: u32 = kind.parse().unwrap();
                    let number = if colours > 16 { noise(at * 3 + 1) % colours } else { (x / 3 + y / 2 * 5 + (noise(at) & 1)) % colours };
                    0xff00_0000 | (number * 0x0a_3d71 + 0x10_2030) & 0xff_ffff
                }
            }
        };
        (0..wide * high).map(pixel).collect()
    }

    /// Rewrites these tests' files, and libwebp's decode of each.
    /// (`cargo test --lib webp -- --ignored`, with vips and ffmpeg installed.)
    #[test]
    #[ignore = "needs vips and ffmpeg, and writes the files the other tests read"]
    fn the_pictures_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-webp-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |program: &str, arguments: &[&str]| {
            let done = Command::new(program).args(arguments).current_dir(&work).stdin(Stdio::null()).stderr(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "{program} {arguments:?}");
        };
        let at = |name: &str| pictures.join(name).to_str().unwrap().to_string();
        for (kind, size) in [("photo", [61, 45]), ("clear", [40, 30]), ("flat", [64, 64]), ("weave", [96, 64]), ("2", [33, 21]), ("4", [33, 21]), ("16", [33, 21]), ("100", [33, 21])] {
            std::fs::write(work.join(format!("{kind}.png")), png::encode_rgba(size[0], &pattern(kind, size)).unwrap()).unwrap();
        }
        run("vips", &["webpsave", "photo.png", &at("exact.webp"), "--lossless", "--effort", "6"]);
        run("vips", &["webpsave", "photo.png", &at("exact-quick.webp"), "--lossless", "--effort", "0"]);
        run("ffmpeg", &["-y", "-loglevel", "error", "-i", "photo.png", "-c:v", "libwebp", "-lossless", "1", &at("exact-ffmpeg.webp")]);
        run("vips", &["webpsave", "clear.png", &at("exact-clear.webp"), "--lossless", "--effort", "4"]);
        run("vips", &["webpsave", "flat.png", &at("exact-flat.webp"), "--lossless"]);
        run("vips", &["webpsave", "weave.png", &at("exact-weave.webp"), "--lossless", "--effort", "6"]);
        for colours in ["2", "4", "16", "100"] {
            run("vips", &["webpsave", &format!("{colours}.png"), &at(&format!("exact-{colours}.webp")), "--lossless", "--effort", "6"]);
        }
        // Lossy by vips: default, coarse, fine; with alpha; a larger picture; and by ffmpeg,
        // tuned for drawings.
        run("vips", &["webpsave", "photo.png", &at("lossy.webp"), "--Q", "75"]);
        run("vips", &["webpsave", "photo.png", &at("lossy-coarse.webp"), "--Q", "12"]);
        run("vips", &["webpsave", "photo.png", &at("lossy-fine.webp"), "--Q", "96", "--preset", "photo"]);
        run("vips", &["webpsave", "clear.png", &at("lossy-clear.webp"), "--Q", "60"]);
        run("vips", &["webpsave", "weave.png", &at("lossy-weave.webp"), "--Q", "50", "--preset", "picture"]);
        run("ffmpeg", &["-y", "-loglevel", "error", "-i", "photo.png", "-c:v", "libwebp", "-q:v", "40", "-preset", "drawing", &at("lossy-ffmpeg.webp")]);
        // A three-frame animation by ffmpeg: its first frame is what is read.
        for (frame, colours) in ["2", "4", "16"].iter().enumerate() {
            std::fs::copy(work.join(format!("{colours}.png")), work.join(format!("m{frame}.png"))).unwrap();
        }
        run("ffmpeg", &["-y", "-loglevel", "error", "-framerate", "5", "-i", "m%d.png", "-c:v", "libwebp_anim", "-lossless", "1", &at("moving.webp")]);
        // (The first six lossy files; the test below makes the rest.)
        for (name, ..) in &LOSSY[..6] {
            run("vips", &["copy", &at(&format!("{name}.webp")), &at(&format!("{name}.webp.png"))]);
        }
        // libwebp's decode of each, through vips; ffmpeg's own reader must agree, or this
        // stops.
        for (name, ..) in EXACT {
            let (webp, wanted) = (at(&format!("{name}.webp")), at(&format!("{name}.webp.png")));
            run("vips", &["copy", &webp, if name.contains("quick") || name.contains("ffmpeg") { "again.png" } else { &wanted }]);
            run("ffmpeg", &["-y", "-loglevel", "error", "-i", &webp, "-pix_fmt", "rgba", "ffmpeg.png"]);
            let read = |file: PathBuf| png::decode(&std::fs::read(file).unwrap()).unwrap();
            let (libwebp, ffmpeg) = (read(if name.contains("quick") || name.contains("ffmpeg") { work.join("again.png") } else { wanted.into() }), read(work.join("ffmpeg.png")));
            assert!(libwebp.0 == ffmpeg.0 && seen(&libwebp.1) == seen(&ffmpeg.1), "{name}: libwebp and ffmpeg read the same");
        }
        // And more than is kept: larger pictures of every kind, at every quality and effort, by
        // vips, read back by it and by this. All equal.
        let mut held = 0;
        for (kind, size) in [("photo", [200, 150]), ("weave", [192, 128]), ("16", [160, 120]), ("clear", [100, 80]), ("flat", [70, 50]), ("2", [17, 250])] {
            std::fs::write(work.join("large.png"), png::encode_rgba(size[0], &pattern(kind, size)).unwrap()).unwrap();
            let fine = ["3", "20", "40", "60", "80", "97"].map(|fine| vec!["--Q", fine]);
            let exact = ["0", "2", "4", "6"].map(|effort| vec!["--lossless", "--effort", effort]);
            for how in fine.iter().chain(&exact).chain(&[vec!["--Q", "50", "--alpha-q", "30"], vec!["--Q", "70", "--preset", "text"], vec!["--near-lossless", "--Q", "60"]]) {
                run("vips", &[&["webpsave", "large.png", "large.webp"], &how[..]].concat());
                run("vips", &["copy", "large.webp", "back.png"]);
                let (wanted, read) = (png::decode(&std::fs::read(work.join("back.png")).unwrap()).unwrap(), decode(&std::fs::read(work.join("large.webp")).unwrap()));
                assert!(read.is_some_and(|read| read.0 == wanted.0 && seen(&read.1) == seen(&wanted.1)), "{kind} written with {how:?}");
                held += 1;
            }
        }
        println!("{held} larger pictures read as libwebp reads them");
        std::fs::remove_dir_all(work).unwrap();
    }

    /// A WebP file assembled around a lossy picture: `frame` is its chunk as some encoder wrote
    /// it, `solid` the alpha chunk before it, `size` its width and height.
    fn wrapped(frame: &[u8], solid: Option<&[u8]>, size: [usize; 2]) -> Vec<u8> {
        let chunk = |name: &[u8], holds: &[u8]| [name, &(holds.len() as u32).to_le_bytes(), holds, &[0][..holds.len() % 2]].concat();
        let (wide, high) = ((size[0] - 1).to_le_bytes(), (size[1] - 1).to_le_bytes());
        let first = chunk(b"VP8X", &[0x10, 0, 0, 0, wide[0], wide[1], wide[2], high[0], high[1], high[2]]);
        let chunks = solid.map_or(chunk(b"VP8 ", frame), |solid| [first, chunk(b"ALPH", solid), chunk(b"VP8 ", frame)].concat());
        [b"RIFF", &(chunks.len() as u32 + 4).to_le_bytes()[..], b"WEBP", &chunks].concat()
    }

    /// Pictures as libwebp never writes them, read by it and by this: the picture chunk by
    /// libvpx (through ffmpeg), which sets other options than libwebp, and alpha written here
    /// in each of its forms. (`cargo test --lib webp -- --ignored`.)
    #[test]
    #[ignore = "needs vips and ffmpeg, and writes the files the other tests read"]
    fn pictures_of_other_writers_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-webp-other-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |program: &str, arguments: &[&str]| {
            let done = Command::new(program).args(arguments).current_dir(&work).stdin(Stdio::null()).stderr(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "{program} {arguments:?}");
        };
        // A picture chunk as libvpx writes it: the one frame of an animation, cut from ffmpeg's
        // container (32 bytes before its frames, 12 before each).
        let by_libvpx = |kind: &str, size: [usize; 2], how: &[&str]| {
            std::fs::write(work.join("in.png"), png::encode_rgba(size[0], &pattern(kind, size)).unwrap()).unwrap();
            run("ffmpeg", &[&["-y", "-loglevel", "error", "-i", "in.png", "-c:v", "libvpx", "-frames:v", "1", "-pix_fmt", "yuv420p"], how, &["-f", "ivf", "out.ivf"]].concat());
            std::fs::read(work.join("out.ivf")).unwrap()[44..].to_vec()
        };
        // libwebp's decode of a file, through vips: kept under a name, where given.
        let by_libwebp = |webp: &[u8], keep: Option<&str>| {
            let (file, read) = keep.map_or((work.join("read.webp"), work.join("read.png")), |name| (pictures.join(format!("{name}.webp")), pictures.join(format!("{name}.webp.png"))));
            std::fs::write(&file, webp).unwrap();
            run("vips", &["copy", file.to_str().unwrap(), read.to_str().unwrap()]);
            png::decode(&std::fs::read(read).unwrap()).unwrap()
        };
        let same = |webp: &[u8], keep: Option<&str>, what: &str| {
            let (wanted, read) = (by_libwebp(webp, keep), decode(webp));
            assert!(read.is_some_and(|read| read.0 == wanted.0 && seen(&read.1) == seen(&wanted.1)), "{what}");
        };
        // Quality: libvpx takes a level from 0 to 63, given three times.
        let fine = |level: &'static str| ["-b:v", "0", "-crf", level, "-qmin", level, "-qmax", level];
        // The kept ones: coarse enough to be loop-filtered, with libvpx's mode-based deltas; in
        // four partitions; and with the simple filter (its "profile 1").
        same(&wrapped(&by_libvpx("photo", [61, 45], &fine("30")), None, [61, 45]), Some("lossy-libvpx"), "kept, by libvpx");
        same(&wrapped(&by_libvpx("photo", [61, 45], &[&["-slices", "4"], &fine("20")[..]].concat()), None, [61, 45]), Some("lossy-parts"), "kept, in parts");
        same(&wrapped(&by_libvpx("photo", [61, 45], &[&["-profile:v", "1"], &fine("50")[..]].concat()), None, [61, 45]), Some("lossy-plain"), "kept, plain");
        // Raw alpha: unfiltered, and filtered horizontally, vertically and by gradient. (Any
        // bytes will do.)
        let noise = |at: usize| ((at as u32).wrapping_mul(2_654_435_761) >> 24) as u8;
        let solid =
            |way: u8, [wide, high]: [usize; 2]| [vec![way << 2], (0..wide * high).map(|at| if way == 0 { (at % wide * 6).min(255) as u8 } else { noise(at * 5 + way as usize) }).collect()].concat();
        let frame = by_libvpx("clear", [40, 30], &fine("12"));
        for (way, name) in ["lossy-clear-raw", "lossy-clear-across", "lossy-clear-down", "lossy-clear-both"].into_iter().enumerate() {
            same(&wrapped(&frame, Some(&solid(way as u8, [40, 30])), [40, 30]), Some(name), name);
        }
        // And more than is kept: every kind by libvpx, with every option that changes its
        // output: two, four and eight partitions, its four profiles, fine to coarse, fast and
        // slow, and with segments.
        let hows: [Vec<&str>; 14] = [
            vec![],
            vec!["-slices", "2"],
            vec!["-slices", "4", "-sharpness", "3"],
            vec!["-slices", "8", "-sharpness", "7"],
            vec!["-profile:v", "1"],
            [&["-profile:v", "1", "-sharpness", "5"], &fine("50")[..]].concat(),
            vec!["-profile:v", "2"],
            vec!["-profile:v", "3"],
            fine("2").to_vec(),
            [&["-sharpness", "1"], &fine("30")[..]].concat(),
            fine("63").to_vec(),
            vec!["-deadline", "realtime", "-cpu-used", "8"],
            vec!["-deadline", "best", "-noise-sensitivity", "2"],
            vec!["-error-resilient", "default", "-static-thresh", "1000"],
        ];
        let mut held = 0;
        for (kind, size) in [("photo", [61, 45]), ("photo", [200, 150]), ("weave", [192, 128]), ("16", [160, 120]), ("flat", [70, 50]), ("2", [17, 250])] {
            for how in &hows {
                same(&wrapped(&by_libvpx(kind, size, how), None, size), None, &format!("{kind} of {size:?} by libvpx with {how:?}"));
                held += 1;
            }
        }
        // Alpha every way for other sizes: raw, and lossless-packed as libwebp does it (taken
        // from a file of its own), with each filter.
        for size in [[100, 80], [1, 9], [9, 1], [16, 16]] {
            let frame = by_libvpx("clear", size, &fine("12"));
            std::fs::write(work.join("clear.png"), png::encode_rgba(size[0], &pattern("clear", size)).unwrap()).unwrap();
            run("vips", &["webpsave", "clear.png", "clear.webp", "--Q", "60"]);
            let whole = std::fs::read(work.join("clear.webp")).unwrap();
            let at = whole.windows(4).position(|name| name == b"ALPH").unwrap() + 8;
            let packed = &whole[at..at + u32::from_le_bytes([whole[at - 4], whole[at - 3], whole[at - 2], whole[at - 1]]) as usize];
            for way in 0..4 {
                same(&wrapped(&frame, Some(&solid(way, size)), size), None, &format!("{size:?}, solid as bytes, way {way}"));
                same(&wrapped(&frame, Some(&[&[packed[0] & 0xf3 | way << 2], &packed[1..]].concat()), size), None, &format!("{size:?}, solid packed, way {way}"));
                held += 2;
            }
        }
        println!("{held} pictures of other writers read as libwebp reads them");
        std::fs::remove_dir_all(work).unwrap();
    }

    use crate::animation::tests::{frames, playable};
    use crate::animation::{After, Laid, LoadedAnimation};

    /// A hand-made animation frame: its offset (even), size, duration, flags (1: dispose to
    /// background; 2: no blend), and its picture's chunks.
    struct Made {
        at: [usize; 2],
        size: [usize; 2],
        shows: u32,
        flags: u8,
        chunks: Vec<u8>,
    }

    /// A WebP chunk: name, length, data, and a padding byte if odd.
    fn chunk(name: &[u8], holds: &[u8]) -> Vec<u8> {
        [name, &(holds.len() as u32).to_le_bytes(), holds, &[0][..holds.len() % 2]].concat()
    }

    /// A hand-made animated WebP: a `size` canvas, `plays` loops, these `frames`.
    fn moving(size: [usize; 2], plays: u16, frames: &[Made]) -> Vec<u8> {
        let three = |number: usize| (number as u32).to_le_bytes()[..3].to_vec();
        let mut chunks = chunk(b"VP8X", &[&[0x12, 0, 0, 0][..], &three(size[0] - 1), &three(size[1] - 1)].concat());
        chunks.extend(chunk(b"ANIM", &[&[0, 0, 0, 0][..], &plays.to_le_bytes()].concat()));
        for made in frames {
            let head = [three(made.at[0] / 2), three(made.at[1] / 2), three(made.size[0] - 1), three(made.size[1] - 1), three(made.shows as usize), vec![made.flags]].concat();
            chunks.extend(chunk(b"ANMF", &[head, made.chunks.clone()].concat()));
        }
        [b"RIFF", &(chunks.len() as u32 + 4).to_le_bytes()[..], b"WEBP", &chunks].concat()
    }

    /// The animated test files: name, file, libwebp's composed frames (stacked), each frame's
    /// duration, and the loop count.
    type Moving = (&'static str, &'static [u8], &'static [u8], &'static [u64], Option<u64>);
    const MOVING: [Moving; 4] = [
        // Five frames, lossless and lossy, by libwebp (through ffmpeg), which after the first
        // writes only the changed rectangle, blended or not and disposed as it finds best.
        ("moving-exact", include_bytes!("../tests/pictures/moving-exact.webp"), include_bytes!("../tests/pictures/moving-exact.webp.frames.png"), &[200; 5], None),
        ("moving-lossy", include_bytes!("../tests/pictures/moving-lossy.webp"), include_bytes!("../tests/pictures/moving-lossy.webp.frames.png"), &[200; 5], None),
        // Hand-made: frames blended semi-transparent, disposed, not blended; a frame where one
        // was disposed, frames on an empty canvas; lossy and lossless; durations of a hundredth
        // and less; three loops.
        ("hand", include_bytes!("../tests/pictures/hand.webp"), include_bytes!("../tests/pictures/hand.webp.frames.png"), &[300, 100, 100, 11, 250, 400, 60], Some(3)),
        // Three lossless frames by ffmpeg, of two, four and sixteen colours.
        ("moving", include_bytes!("../tests/pictures/moving.webp"), include_bytes!("../tests/pictures/moving.webp.frames.png"), &[200; 3], Some(1)),
    ];

    #[test]
    fn moving_pictures_come_out_as_libwebp_shows_them() {
        for (name, webp, wanted, shows, plays) in MOVING {
            let mut anim = LoadedAnimation::read(webp, usize::MAX).unwrap_or_else(|| panic!("{name} is read"));
            let (width, wanted) = png::decode(wanted).unwrap();
            assert_eq!((anim.size[0], anim.frames.len() * anim.size[1]), (width, wanted.len() / width), "{name}: its size, and how many frames it has");
            let (have, want) = (seen(&frames(&mut anim)), seen(&wanted));
            let differ = have.iter().zip(&want).filter(|(one, other)| one != other).count();
            assert!(have == want, "{name}: {differ} pixels differ");
            assert_eq!((anim.frames.iter().map(|frame| frame.shows).collect::<Vec<u64>>(), anim.plays), (shows.to_vec(), plays), "{name}: its times");
            // And its first frame is the file as a picture.
            assert_eq!(decode(webp).map(|(width, pixels)| (width, seen(&pixels))), Some((width, want[..anim.size[0] * anim.size[1]].to_vec())), "{name} as a picture");
        }
    }

    #[test]
    fn frames_lie_and_are_laid_as_their_chunks_say() {
        // Frames of the two-colour lossless picture (33×21) on a 40×30 canvas: at the corner,
        // then at 4, 2, disposed after; then not blended.
        let still = EXACT[4].1;
        let picture = &still[still.windows(4).position(|name| name == b"VP8L").unwrap()..];
        let made = |at: [usize; 2], flags: u8| Made { at, size: [33, 21], shows: 50, flags, chunks: picture.to_vec() };
        let mut anim = LoadedAnimation::read(&moving([40, 30], 0, &[made([0, 0], 0), made([4, 2], 1), made([6, 8], 2)]), usize::MAX).unwrap();
        let laid: Vec<(After, Laid)> = anim.frames.iter().map(|frame| (frame.after, frame.laid)).collect();
        // (That picture is opaque: each frame replaces.)
        assert_eq!(laid, [(After::Stays, Laid::Instead), (After::Cleared, Laid::Instead), (After::Stays, Laid::Instead)]);
        let (_, pixels) = decode(still).unwrap();
        let mut at = |frame: usize, x: usize, y: usize| anim.shown(frame).0[y * 40 + x];
        assert_eq!([at(0, 0, 0), at(0, 32, 20), at(0, 33, 20), at(1, 4, 2), at(1, 3, 2)], [pixels[0], pixels[33 * 21 - 1], 0, pixels[0], pixels[2 * 33 + 3]]);
        // Cleared where the previous frame lay; and the frame lies at its offset.
        assert_eq!([at(2, 5, 3), at(2, 2, 0), at(2, 6, 8)], [0, pixels[2], pixels[0]]);
        // A frame off the canvas ends the animation before it, as does one whose picture is not
        // the frame's size.
        assert_eq!(LoadedAnimation::read(&moving([40, 30], 0, &[made([0, 0], 0), made([8, 10], 0)]), usize::MAX).map(|anim| anim.frames.len()), Some(1));
        let other = Made { size: [32, 21], ..made([0, 0], 0) };
        assert_eq!(LoadedAnimation::read(&moving([40, 30], 0, &[made([0, 0], 0), other]), usize::MAX).map(|anim| anim.frames.len()), Some(1));
        // A still WebP is a one-frame animation: its picture.
        let mut anim = LoadedAnimation::read(still, usize::MAX).unwrap();
        assert_eq!((anim.frames.len(), anim.shown(0).0.to_vec()), (1, pixels));
    }

    #[test]
    fn a_damaged_moving_picture_gives_nothing_or_some_film() {
        for (name, webp, ..) in MOVING {
            // Truncated at every length; then whole with each byte in turn changed.
            assert!((0..webp.len()).all(|cut| playable(LoadedAnimation::read(&webp[..cut], usize::MAX))), "{name}, cut short");
            for at in 0..webp.len() {
                let damaged = |to: u8| [&webp[..at], &[to], &webp[at + 1..]].concat();
                assert!([webp[at] ^ (1 << (at % 8)), 0, 255].into_iter().all(|to| playable(LoadedAnimation::read(&damaged(to), usize::MAX))), "{name}, byte {at}");
            }
        }
    }

    /// libwebp's own animation compositing (`WebPAnimDecoder`, in `libwebpdemux`), loaded from
    /// the Linux system, for the test tools.
    #[cfg(target_os = "linux")]
    mod libwebp {
        use std::ffi::{c_char, c_int, c_void};
        unsafe extern "C" {
            fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
            fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        /// libwebp's `WebPData`, decoder options, and animation info: canvas size, loop count,
        /// background colour, frame count.
        #[repr(C)]
        struct Data(*const u8, usize);
        #[repr(C)]
        struct Options(c_int, c_int, [u32; 7]);
        #[repr(C)]
        struct Info([u32; 5], [u32; 4]);
        /// The libwebp ABI version these layouts match.
        const VERSION: c_int = 0x0107;

        /// Every frame of an animated WebP as libwebp composes it, stacked (not premultiplied),
        /// with the canvas width; each frame's duration, and the loop count (0: forever).
        pub(crate) fn frames(webp: &[u8]) -> (usize, Vec<u32>, Vec<u64>, u32) {
            // SAFETY: libwebp's functions, called as its header declares them, with the bytes
            // alive as long as the decoder reading them.
            unsafe {
                let library = dlopen(c"libwebpdemux.so.2".as_ptr(), 2);
                assert!(!library.is_null(), "libwebpdemux is there");
                let named = |name: &std::ffi::CStr| dlsym(library, name.as_ptr());
                let setup: extern "C" fn(*mut Options, c_int) -> c_int = std::mem::transmute(named(c"WebPAnimDecoderOptionsInitInternal"));
                let open: extern "C" fn(*const Data, *const Options, c_int) -> *mut c_void = std::mem::transmute(named(c"WebPAnimDecoderNewInternal"));
                let told: extern "C" fn(*const c_void, *mut Info) -> c_int = std::mem::transmute(named(c"WebPAnimDecoderGetInfo"));
                let next: extern "C" fn(*mut c_void, *mut *mut u8, *mut c_int) -> c_int = std::mem::transmute(named(c"WebPAnimDecoderGetNext"));
                let close: extern "C" fn(*mut c_void) = std::mem::transmute(named(c"WebPAnimDecoderDelete"));
                let mut options = Options(0, 0, [0; 7]);
                assert_eq!(setup(&mut options, VERSION), 1);
                // (`MODE_RGBA`: a byte each, not premultiplied.)
                options.0 = 1;
                let decoder = open(&Data(webp.as_ptr(), webp.len()), &options, VERSION);
                assert!(!decoder.is_null(), "libwebp reads the animation");
                let mut info = Info([0; 5], [0; 4]);
                assert_eq!(told(decoder, &mut info), 1);
                let ([wide, high, plays, ..], mut all, mut ends) = (info.0, Vec::new(), vec![0]);
                for _ in 0..info.0[4] {
                    let (mut frame, mut when) = (std::ptr::null_mut(), 0);
                    assert_eq!(next(decoder, &mut frame, &mut when), 1);
                    ends.push(when as u64);
                    let bytes = std::slice::from_raw_parts(frame, (wide * high * 4) as usize);
                    all.extend(bytes.as_chunks::<4>().0.iter().map(|[red, green, blue, solid]| u32::from_be_bytes([*solid, *red, *green, *blue])));
                }
                close(decoder);
                (wide as usize, all, ends.windows(2).map(|pair| pair[1] - pair[0]).collect(), plays)
            }
        }
    }

    /// Rewrites the animated test files and libwebp's composition of each, frame by frame; and
    /// reports how far ffmpeg's own reader and vips (which compose their own way) are from it.
    /// (`cargo test --lib webp -- --ignored`, on Linux with ffmpeg, vips and libwebp.)
    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "needs ffmpeg, vips and libwebp, and writes the files the other tests read"]
    fn the_moving_pictures_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-webp-moving-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |program: &str, arguments: &[&str]| {
            let done = Command::new(program).args(arguments).current_dir(&work).stdin(Stdio::null()).stderr(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "{program} {arguments:?}");
        };
        let at = |name: &str| pictures.join(name).to_str().unwrap().to_string();
        for step in 0..5 {
            std::fs::write(work.join(format!("f{step}.png")), png::encode_rgba(24, &crate::animation::tests::pattern(step)).unwrap()).unwrap();
        }
        run("ffmpeg", &["-y", "-loglevel", "error", "-framerate", "5", "-i", "f%d.png", "-c:v", "libwebp_anim", "-lossless", "1", "-loop", "0", &at("moving-exact.webp")]);
        run("ffmpeg", &["-y", "-loglevel", "error", "-framerate", "5", "-i", "f%d.png", "-c:v", "libwebp_anim", "-lossless", "0", "-q:v", "60", "-loop", "0", &at("moving-lossy.webp")]);
        // A hand-made frame's picture, as vips writes it: its chunks, without the first.
        let picture = |size: [usize; 2], solid: &dyn Fn(usize) -> u32, how: &[&str]| {
            let pixels: Vec<u32> = (0..size[0] * size[1]).map(|at| solid(at) << 24 | (at as u32 * 37 % 256) << 16 | (at as u32 * 91 % 256) << 8 | (at as u32 * 13 % 256)).collect();
            std::fs::write(work.join("frame.png"), png::encode_rgba(size[0], &pixels).unwrap()).unwrap();
            run("vips", &[&["webpsave", "frame.png", "frame.webp"], how].concat());
            let file = std::fs::read(work.join("frame.webp")).unwrap();
            let first = if &file[12..16] == b"VP8X" { 30 } else { 12 };
            file[first..].to_vec()
        };
        let exact = ["--lossless", "--effort", "4"];
        let made = |at: [usize; 2], size: [usize; 2], shows: u32, flags: u8, solid: &dyn Fn(usize) -> u32, how: &[&str]| Made { at, size, shows, flags, chunks: picture(size, solid, how) };
        let hand = [
            made([0, 0], [16, 12], 300, 0, &|at| [255, 128, 40][at % 3], &exact),
            made([2, 2], [8, 6], 0, 1, &|at| [200, 90][at % 2], &exact),
            made([4, 4], [8, 6], 10, 0, &|at| [64, 255, 0, 180][at % 4], &["--Q", "70"]),
            made([0, 0], [16, 12], 11, 3, &|_| 255, &exact),
            made([6, 2], [6, 6], 250, 1, &|at| [100, 255][at % 2], &exact),
            made([0, 6], [10, 6], 400, 0, &|at| [30, 255, 0][at % 3], &["--Q", "40"]),
            made([2, 0], [12, 12], 60, 2, &|at| [0, 77, 255][at % 3], &exact),
        ];
        std::fs::write(at("hand.webp"), moving([16, 12], 3, &hand)).unwrap();
        // libwebp's composition of each; and how far the others are from it: how many pixels
        // differ, and by how much at most.
        for (name, ..) in MOVING {
            let webp = std::fs::read(at(&format!("{name}.webp"))).unwrap();
            let (width, frames, shows, plays) = libwebp::frames(&webp);
            // (Its durations are the animation's, and the tests'.)
            let anim = LoadedAnimation::read(&webp, usize::MAX).unwrap();
            let times = (shows.into_iter().map(crate::animation::shows_for).collect::<Vec<u64>>(), Some(plays as u64).filter(|plays| *plays > 0));
            assert_eq!((anim.frames.iter().map(|frame| frame.shows).collect::<Vec<u64>>(), anim.plays), times, "{name}: its times");
            println!("{name}: times {times:?}");
            std::fs::write(at(&format!("{name}.webp.frames.png")), png::encode_rgba(width, &frames).unwrap()).unwrap();
            let count = format!("tile=1x{}", frames.len() / width / LoadedAnimation::read(&webp, 1).unwrap().size[1]);
            run("ffmpeg", &["-y", "-loglevel", "error", "-i", &at(&format!("{name}.webp")), "-vf", &count, "-frames:v", "1", "-pix_fmt", "rgba", "ffmpeg.png"]);
            run("vips", &["copy", &format!("{}[n=-1]", at(&format!("{name}.webp"))), "vips.png"]);
            for other in ["ffmpeg", "vips"] {
                let (_, pixels) = png::decode(&std::fs::read(work.join(format!("{other}.png"))).unwrap()).unwrap();
                let (wanted, pixels) = (seen(&frames), seen(&pixels));
                let worst = wanted.iter().zip(&pixels).flat_map(|(one, other)| [24, 16, 8, 0].map(|part| (one >> part & 255).abs_diff(other >> part & 255))).max();
                let differ = wanted.iter().zip(&pixels).filter(|(one, other)| one != other).count();
                println!("{name}: {other} differs from libwebp in {differ} of {} pixels, by up to {worst:?} ({} pixels to its {})", wanted.len(), pixels.len(), wanted.len());
            }
        }
        std::fs::remove_dir_all(work).unwrap();
    }

    /// A WebP of the two-colour lossless picture (33×21) followed by an `EXIF` orientation
    /// chunk, as an extended file has it.
    fn noted(turn: u16, low_first: bool) -> Vec<u8> {
        let still = EXACT[4].1;
        let first = still.windows(4).position(|name| name == b"VP8L").unwrap();
        let long = u32::from_le_bytes([still[first + 4], still[first + 5], still[first + 6], still[first + 7]]) as usize;
        // (With the padding byte an odd chunk has.)
        let picture = &still[first..first + 8 + long + long % 2];
        let chunks = [chunk(b"VP8X", &[0x08, 0, 0, 0, 32, 0, 0, 20, 0, 0]), picture.to_vec(), chunk(b"EXIF", &crate::exif_orientation::tests::notes(turn, low_first)[6..])].concat();
        [b"RIFF", &(chunks.len() as u32 + 4).to_le_bytes()[..], b"WEBP", &chunks].concat()
    }

    #[test]
    fn a_note_from_the_camera_turns_the_picture_each_of_eight_ways() {
        let (width, stored) = decode(EXACT[4].1).unwrap();
        let height = stored.len() / width;
        for (turn, low_first) in (1..=8).flat_map(|turn| [(turn, true), (turn, false)]) {
            let (across, shown) = decode(&noted(turn, low_first)).expect("is read with a note in it");
            assert_eq!(across, if turn < 5 { width } else { height }, "turn {turn}: its width");
            let from = |at: usize| crate::exif_orientation::tests::from(turn as usize, [width, height], at % across, at / across);
            assert!(shown.iter().copied().eq((0..stored.len()).map(from).map(|(x, y)| stored[y * width + x])), "turn {turn}, low end first: {low_first}");
        }
        assert_eq!(decode(&noted(9, false)), Some((width, stored)));
    }
}
