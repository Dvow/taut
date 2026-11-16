//! BMP and ICO decoding, and ICO encoding: Windows' long-standing image formats, shown without
//! another library, and app-made icons for programs and shortcuts.
//!
//! A BMP has a signature, its pixel data offset, then a header: size, bits per pixel, and
//! compression. Pixels of up to 8 bits index a palette that follows; 16 bits or more hold red,
//! green, blue and sometimes alpha, in bitfields the file gives or the defaults. Rows go
//! bottom-up unless the height is negative, each padded to 4 bytes. 4- and 8-bit rows may be
//! RLE-compressed: "n of this", "these literally", "end of row", "skip right and up".
//!
//! An ICO holds one icon at several sizes. Each is a BMP without its 14-byte file header, whose
//! height is doubled because a 1-bit transparency mask row follows each pixel row; or a whole
//! PNG. The largest is read. Written, each size is stored as Windows stores its own: PNG from
//! 256 pixels, smaller as 32-bit BMP. A program that writes no icon carries none of this.
//!
//! Not read: BMPs holding JPEG or PNG (only for printers), the two OS/2-only compressions, and
//! colour profiles (ignored: colours are taken as stored).
use crate::png;

/// The most pixels per file byte an RLE image may claim. (A 255-pixel run takes two bytes, and
/// "the rest is empty" two more, at any size. One claiming more is refused.)
const MOST: usize = 256;

/// Decodes a BMP. Returns its width in pixels and its pixels in rows from the top,
/// `0xAARRGGBB`, not premultiplied: what [`Ui::add_picture`](crate::Ui::add_picture) takes.
/// `None` for a damaged, truncated or non-BMP file.
pub fn decode(bmp: &[u8]) -> Option<(usize, Vec<u32>)> {
    let rows = bmp.get(number(bmp.strip_prefix(b"BM")?, 8, 4)? as usize..)?;
    picture(&bmp[14..], Some(rows), false)
}

/// Decodes an ICO (or CUR, a cursor): the image with the most pixels, and of several that size,
/// the deepest. Returns what [`decode`] does.
pub fn decode_icon(ico: &[u8]) -> Option<(usize, Vec<u32>)> {
    let count = number(ico, 4, 2).filter(|_| matches!(ico, [0, 0, 1 | 2, 0, ..]))? as usize;
    // Each 16-byte directory entry gives an image's offset and length. (And its size: but in a
    // byte each way, which the image itself gives better.)
    let held = |entry: &[u8; 16]| {
        let rest = ico.get(number(entry, 12, 4)? as usize..)?;
        let picture = rest.get(..number(entry, 8, 4)? as usize).unwrap_or(rest);
        Some((weight(picture)?, picture))
    };
    let (_, best) = ico[6..].as_chunks::<16>().0.iter().take(count).filter_map(held).max_by_key(|(weight, _)| *weight)?;
    if best.starts_with(b"\x89PNG") {
        return png::decode(best);
    }
    picture(best, None, true)
}

/// Encodes an ICO: one icon at several sizes, each as [`decode_icon`] returns one (width, and
/// `0xAARRGGBB` pixels in rows from the top, not premultiplied). Images 256 pixels wide or high
/// are stored as PNG, as Windows stores its large icons; smaller ones as 32-bit BMP, with the
/// 1-bit transparency mask after them (for readers that need it). `None` for no images, or one
/// empty or over 256 pixels either way (ICO stores a byte each, 0 meaning 256).
pub fn encode_icon(sizes: &[(usize, &[u32])]) -> Option<Vec<u8>> {
    let held: Vec<Vec<u8>> = sizes.iter().map(|(width, pixels)| held(*width, pixels)).collect::<Option<_>>()?;
    let mut out = [&[0, 0, 1, 0][..], &(u16::try_from(sizes.len()).ok().filter(|count| *count > 0)?).to_le_bytes()].concat();
    // Each 16-byte directory entry: size (a byte each way), bits per pixel, length, and offset.
    let mut at = 6 + 16 * sizes.len();
    for ((width, pixels), picture) in sizes.iter().zip(&held) {
        let [wide, high] = [*width, pixels.len() / width].map(|side| side as u8);
        out.extend([&[wide, high, 0, 0, 1, 0, 32, 0][..], &(picture.len() as u32).to_le_bytes(), &(at as u32).to_le_bytes()].concat());
        at += picture.len();
    }
    out.extend(held.concat());
    Some(out)
}

/// One ICO image (see `encode_icon`): a PNG, or a BMP header (with doubled height), rows
/// bottom-up at 4 bytes per pixel (blue, green, red, alpha), and a 1-bit mask set where a pixel
/// is fully transparent, each row padded to 4 bytes.
fn held(width: usize, pixels: &[u32]) -> Option<Vec<u8>> {
    let height = pixels.len() / width.max(1);
    if !(1..=256).contains(&width) || !(1..=256).contains(&height) {
        return None;
    }
    if width == 256 || height == 256 {
        return png::encode_clear(width, &pixels[..width * height]);
    }
    let said = [&40u32.to_le_bytes()[..], &(width as u32).to_le_bytes(), &(2 * height as u32).to_le_bytes(), &[1, 0, 32, 0], &[0; 24]].concat();
    let rows = pixels.chunks_exact(width).rev().flatten().flat_map(|pixel| pixel.to_le_bytes());
    Some(said.into_iter().chain(rows).chain(pixels.chunks_exact(width).rev().flat_map(clear_bits)).collect())
}

/// A row's transparency mask bits, first pixel in the highest bit, padded to 4 bytes.
fn clear_bits(row: &[u32]) -> impl Iterator<Item = u8> + '_ {
    (0..row.len().div_ceil(32) * 4).map(move |byte| (0..8).fold(0, |bits, bit| bits << 1 | row.get(byte * 8 + bit).is_some_and(|pixel| pixel >> 24 == 0) as u8))
}

/// An ICO image's pixel count and bits per pixel.
fn weight(picture: &[u8]) -> Option<(usize, usize)> {
    if picture.starts_with(b"\x89PNG") {
        let size = [16, 20].map(|at| picture.get(at..at + 4).map_or(0, |n| u32::from_be_bytes([n[0], n[1], n[2], n[3]]) as usize));
        return Some((size[0].saturating_mul(size[1]), 32));
    }
    told(picture, true).map(|told| (told.size[0].saturating_mul(told.size[1]), told.bits))
}

/// A little-endian number of `long` bytes at `at` in `bytes`.
fn number(bytes: &[u8], at: usize, long: usize) -> Option<u32> {
    Some(bytes.get(at..at + long)?.iter().rev().fold(0, |sum, byte| sum << 8 | *byte as u32))
}

/// A BMP's header data.
struct Told {
    /// Width and height in pixels.
    size: [usize; 2],
    /// Rows go top-down. (Else bottom-up.)
    top_first: bool,
    /// Bits per pixel.
    bits: usize,
    /// Rows are RLE-compressed.
    packed: bool,
    /// Bitmasks for red, green, blue and alpha.
    places: [u32; 4],
    /// The palette that pixels of up to 8 bits index.
    colours: Vec<u32>,
    /// The header and palette's length: where rows start, unless said otherwise.
    long: usize,
}

/// Reads a BMP header. Inside an ICO (`icon`), the stated height is double.
fn told(said: &[u8], icon: bool) -> Option<Told> {
    // It starts with its own length. The oldest (12 bytes) gives its size in two bytes each and
    // no compression; all later ones share their first 40 bytes.
    let long = number(said, 0, 4)? as usize;
    let (old, step) = (long == 12, if long == 12 { 2 } else { 4 });
    let (wide, high) = (number(said, 4, step)? as i32, number(said, 4 + step, step)? as i32);
    let bits = number(said, 6 + 2 * step, 2)? as usize;
    let (packing, listed) = if old { (0, 0) } else { (number(said, 16, 4)?, number(said, 32, 4)? as usize) };
    let fits = match packing {
        0 => [1, 2, 4, 8, 16, 24, 32].contains(&bits),
        1 => bits == 8,
        2 => bits == 4,
        3 | 6 => bits == 16 || bits == 32,
        _ => false,
    };
    let rows = high.unsigned_abs() as usize / if icon { 2 } else { 1 };
    if !fits || (!old && long < 40) || wide <= 0 || rows == 0 {
        return None;
    }
    // Bitmasks for 16 bits or more: four-byte values after the first 40 (alpha only in headers
    // long enough, or that say so), or the defaults: 5 bits each in 16, a byte each otherwise.
    let (own, place) = (packing == 3 || packing == 6, |at: usize| number(said, 40 + 4 * at, 4));
    let places = match (own, bits) {
        (true, _) => [place(0)?, place(1)?, place(2)?, place(3).filter(|_| long >= 56 || packing == 6).unwrap_or(0)],
        (false, 16) => [0x7c00, 0x03e0, 0x001f, 0],
        _ => USUAL,
    };
    // Then the palette, each entry blue, green, red and a pad byte (which the oldest header
    // omits): as many as stated, or as the bits can index.
    let list = long + if own && long == 40 { [12, 16][(packing == 6) as usize] } else { 0 };
    let (each, listed) = (if old { 3 } else { 4 }, if listed == 0 && bits <= 8 { 1 << bits } else { listed });
    let colour = |n: usize| said.get(list + n * each..)?.first_chunk::<3>().map(|c| 0xff00_0000 | (c[2] as u32) << 16 | (c[1] as u32) << 8 | c[0] as u32);
    let colours = (0..listed.min(256)).map_while(colour).collect();
    Some(Told { size: [wide as usize, rows], top_first: high < 0, bits, packed: packing == 1 || packing == 2, places, colours, long: list.saturating_add(listed.saturating_mul(each)) })
}

/// The default masks for 24 or 32 bits: red, green, blue, alpha, a byte each, as returned.
const USUAL: [u32; 4] = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000];

/// Decodes an image from its header (`said`) and rows, which follow it if not given. Inside an
/// ICO (`icon`), a transparency mask follows the rows.
fn picture(said: &[u8], rows: Option<&[u8]>, icon: bool) -> Option<(usize, Vec<u32>)> {
    let told = told(said, icon)?;
    let ([wide, high], rows) = (told.size, rows.or(said.get(told.long..))?);
    if told.packed && wide.checked_mul(high)? > rows.len().saturating_mul(MOST) {
        return None;
    }
    // RLE rows decompress to a byte per pixel. Others are each padded to 4 bytes, and all must
    // be there: except the last row's padding.
    let unpacked = if told.packed { runs(rows, [wide, high], told.bits) } else { Vec::new() };
    let (rows, bits, row) = if told.packed { (&unpacked[..], 8, wide) } else { (rows, told.bits, wide.checked_mul(told.bits)?.div_ceil(32) * 4) };
    if rows.len() < row.checked_mul(high - 1)?.checked_add((wide * bits).div_ceil(8))? {
        return None;
    }
    // A pixel of up to 8 bits indexes the palette, the first in a byte's highest bits. Larger
    // ones are little-endian values whose fields are extracted and scaled to 0..255, unless
    // already laid out as wanted.
    let (usual, line) = (told.places == USUAL, |at: usize| if told.top_first { at / wide } else { high - 1 - at / wide });
    let pixel = |at: usize| {
        let (x, row) = (at % wide, &rows[row * line(at)..]);
        if bits <= 8 {
            let number = row[x * bits / 8] >> (8 - bits - x * bits % 8) & (0xff >> (8 - bits));
            return told.colours.get(number as usize).copied().unwrap_or(0xff00_0000);
        }
        let value = number(row, x * bits / 8, bits / 8).unwrap_or(0);
        if usual {
            return value;
        }
        let [red, green, blue, solid] = told.places.map(|place| part(value, place));
        solid.unwrap_or(255) << 24 | red.unwrap_or(0) << 16 | green.unwrap_or(0) << 8 | blue.unwrap_or(0)
    };
    let mut pixels: Vec<u32> = (0..wide * high).map(pixel).collect();
    // An image with an alpha channel saying "fully transparent" for every pixel doesn't mean
    // it: it is opaque. (Most 32-bit ones leave those bits 0; 24-bit ones have none.)
    let says = pixels.iter().any(|pixel| pixel >> 24 != 0);
    pixels.iter_mut().filter(|_| !says).for_each(|pixel| *pixel |= 0xff00_0000);
    // In an icon without its own alpha, a 1-bit mask per pixel marks the transparent ones: in
    // rows like the image's, after them.
    let clear = rows.get(row * high..).filter(|_| icon && !(says && bits == 32)).unwrap_or(&[]);
    let masked = pixels.iter_mut().enumerate().take(if clear.is_empty() { 0 } else { wide * high });
    for (at, pixel) in masked {
        let bit = clear.get(line(at) * (wide.div_ceil(32) * 4) + at % wide / 8).is_some_and(|byte| byte << (at % wide % 8) & 0x80 != 0);
        *pixel = [*pixel, 0][bit as usize];
    }
    Some((wide, pixels))
}

/// The field of a pixel `place` selects, scaled to 0..255. `None` if `place` is empty.
fn part(value: u32, place: u32) -> Option<u32> {
    let most = place.checked_shr(place.trailing_zeros())? as u64;
    Some(((((value & place) >> place.trailing_zeros()) as u64 * 255 + most / 2) / most) as u32)
}

/// Decompresses RLE rows to a byte per pixel, rows contiguous. `bits` is 8, or 4 for runs of
/// two pixels per byte. Skipped pixels get palette entry 0.
fn runs(packed: &[u8], [wide, high]: [usize; 2], bits: usize) -> Vec<u8> {
    let (mut out, mut at, mut x, mut y) = (vec![0; wide * high], 0, 0, 0);
    while let (Some(&[count, value]), true) = (packed.get(at..).and_then(|rest| rest.first_chunk::<2>()), y < high) {
        at += 2;
        // Two bytes: a count and the colour of all of them. With a zero count, the second says
        // something else: end of row, end of image, a skip right and up (in two more bytes), or
        // so many literal pixels, padded to an even byte count.
        let (long, own) = match (count, value) {
            (0, 0) => {
                (x, y) = (0, y + 1);
                continue;
            }
            (0, 1) => break,
            (0, 2) => {
                let by = packed.get(at..at + 2).unwrap_or(&[0, 0]);
                (x, y, at) = (x + by[0] as usize, y + by[1] as usize, at + 2);
                continue;
            }
            (0, long) => (long as usize, (long as usize * bits).div_ceil(8)),
            (long, _) => (long as usize, 0),
        };
        let these = packed.get(at..at + own).unwrap_or(&[]);
        for n in 0..long.min(wide.saturating_sub(x)) {
            // With two pixels per byte, the first is in the high nibble: a one-byte run
            // alternates two colours.
            let byte = if own > 0 { these.get(n * bits / 8).copied().unwrap_or(0) } else { value };
            out[y * wide + x + n] = if bits == 8 {
                byte
            } else if n % 2 == 0 {
                byte >> 4
            } else {
                byte & 15
            };
        }
        (x, at) = (x + long, at + own + own % 2);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// A hand-made BMP: its header, palette and rows.
    fn file(said: &[u8], colours: &[u8], rows: &[u8]) -> Vec<u8> {
        let start = 14 + said.len() + colours.len();
        [b"BM", &((start + rows.len()) as u32).to_le_bytes()[..], &[0; 4], &(start as u32).to_le_bytes(), said, colours, rows].concat()
    }

    /// A BMP header of `long` bytes: size, bits per pixel, compression, and palette size.
    fn said(long: usize, [wide, high]: [i32; 2], bits: u16, packing: u32, listed: u32) -> Vec<u8> {
        let mut said = [&(long as u32).to_le_bytes()[..], &wide.to_le_bytes(), &high.to_le_bytes(), &[1, 0], &bits.to_le_bytes(), &packing.to_le_bytes(), &[0; 12], &listed.to_le_bytes()].concat();
        said.resize(long, 0);
        said
    }

    /// The test image, 23 by 17 (so rows need padding): colours varying across and down, noisy
    /// in places, more transparent to the left.
    const SIZE: [usize; 2] = [23, 17];

    /// The test image's pixels.
    fn pattern() -> Vec<u32> {
        let noise = |at: usize| (at as u32).wrapping_mul(2_654_435_761) >> 24;
        (0..SIZE[0] * SIZE[1]).map(|at| ((at % 23) as u32 * 12).min(255) << 24 | ((at % 23) as u32 * 11) << 16 | ((at / 23) as u32 * 15) << 8 | noise(at) & 0xf8).collect()
    }

    /// A 16-colour palette as BMP stores it (`each` bytes per entry).
    fn sixteen(each: usize) -> Vec<u8> {
        (0..16u8).flat_map(|n| [n * 16, 255 - n * 13, (n * 7) ^ 0x55, 0][..each].to_vec()).collect()
    }

    /// The palette index at `x`, `y` in the 16-colour images.
    fn numbered(x: usize, y: usize) -> u8 {
        ((x / 3 + y / 2 * 5 + (x * y) % 3) % 16) as u8
    }

    /// The 16-colour image's rows, uncompressed: `bits` per pixel (4 or 8), bottom-up, each
    /// padded to 4 bytes.
    fn rows(bits: usize) -> Vec<u8> {
        let row = |y: usize| {
            let mut row = vec![0u8; (SIZE[0] * bits).div_ceil(32) * 4];
            (0..SIZE[0]).for_each(|x| row[x * bits / 8] |= numbered(x, y) << (8 - bits - x * bits % 8));
            row
        };
        (0..SIZE[1]).rev().flat_map(row).collect()
    }

    /// The 16-colour image's rows RLE-compressed, at a byte (`bits` 8) or a nibble (4) per
    /// pixel. Each row differently, to cover everything: single-colour runs, literal runs (odd
    /// lengths, needing padding), a skip right, an early end of row, a whole row skipped, and
    /// an end before the last row. Skipped pixels keep palette entry 0.
    fn packed(bits: usize) -> Vec<u8> {
        let one = |colour: u8| if bits == 8 { colour } else { colour << 4 | colour };
        // `count` pixels of a row from `x`, as single-colour runs.
        let runs = |line: &[u8], x: usize, count: usize| {
            let mut out = Vec::new();
            for (at, colour) in line.iter().enumerate().skip(x).take(count) {
                match out.last_chunk_mut::<2>() {
                    Some([long, last]) if *last == one(*colour) && at > x => *long += 1,
                    _ => out.extend([1, one(*colour)]),
                }
            }
            out
        };
        // `count` literal pixels: the count, then the pixels, padded to an even byte count.
        let as_they_are = |line: &[u8], x: usize, count: usize| {
            let mut held = vec![0u8; (count * bits).div_ceil(8)];
            (0..count).for_each(|n| held[n * bits / 8] |= line[x + n] << ((8 - bits - n * bits % 8) % 8));
            [&[0, count as u8][..], &held, &[0][..held.len() % 2]].concat()
        };
        let mut out = Vec::new();
        for row in 0..SIZE[1] - 1 {
            let line: Vec<u8> = (0..SIZE[0]).map(|x| numbered(x, SIZE[1] - 1 - row)).collect();
            out.extend(match row % 5 {
                0 => runs(&line, 0, 23),
                1 => [as_they_are(&line, 0, 3), as_they_are(&line, 3, 4), as_they_are(&line, 7, 5), as_they_are(&line, 12, 7), as_they_are(&line, 19, 4)].concat(),
                2 => [runs(&line, 0, 5), vec![0, 2, 4, 0], runs(&line, 9, 14)].concat(),
                3 => runs(&line, 0, 11),
                // A whole row skipped: one up, from where the previous row ended.
                _ => vec![0, 2, 0, 1],
            });
            out.extend(if row % 5 == 4 { &[][..] } else { &[0, 0] });
        }
        [out, vec![0, 1]].concat()
    }

    /// Regenerates these tests' files, and what ffmpeg decodes of each.
    /// (`cargo test --lib bmp -- --ignored`, with ffmpeg installed.)
    #[test]
    #[ignore = "needs ffmpeg, and writes the files the other tests read"]
    fn the_pictures_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-bmp-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |arguments: &[&str]| {
            let done = Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(arguments).current_dir(&work).stdin(Stdio::null()).stderr(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "ffmpeg {arguments:?}");
        };
        let at = |name: &str| pictures.join(name).to_str().unwrap().to_string();
        let (pattern, [wide, high]) = (pattern(), SIZE.map(|side| side as i32));
        std::fs::write(work.join("in.png"), png::encode_clear(SIZE[0], &pattern).unwrap()).unwrap();
        // As ffmpeg writes them: every pixel format it has.
        for (name, how) in [("24", "bgr24"), ("32", "bgra"), ("555", "rgb555le"), ("565", "rgb565le"), ("444", "rgb444le"), ("8", "rgb8"), ("grey", "gray"), ("1", "monob")] {
            run(&["-i", "in.png", "-pix_fmt", how, &at(&format!("bmp-{name}.bmp"))]);
        }
        // And by hand, what it doesn't write: 16 colours at 4 bits per pixel; the same with the
        // oldest header; and RLE at 8 and 4 bits.
        std::fs::write(at("bmp-4.bmp"), file(&said(40, [wide, high], 4, 0, 16), &sixteen(4), &rows(4))).unwrap();
        let old = [&12u32.to_le_bytes()[..], &(wide as u16).to_le_bytes(), &(high as u16).to_le_bytes(), &[1, 0, 4, 0]].concat();
        std::fs::write(at("bmp-old.bmp"), file(&old, &sixteen(3), &rows(4))).unwrap();
        std::fs::write(at("bmp-runs-8.bmp"), file(&said(40, [wide, high], 8, 1, 16), &sixteen(4), &packed(8))).unwrap();
        std::fs::write(at("bmp-runs-4.bmp"), file(&said(40, [wide, high], 4, 2, 16), &sixteen(4), &packed(4))).unwrap();
        // Top-down rows, 3 bytes per pixel.
        let down: Vec<u8> = pattern.chunks(SIZE[0]).flat_map(|row| [row.iter().flat_map(|pixel| pixel.to_le_bytes()[..3].to_vec()).collect(), vec![0; 3]].concat()).collect();
        std::fs::write(at("bmp-top.bmp"), file(&said(40, [wide, -high], 24, 0, 0), &[], &down)).unwrap();
        // The longest header, with bitmasks: here red first, alpha last.
        let mut places = said(124, [wide, high], 32, 3, 0);
        places[40..56].copy_from_slice(&[0xff, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0xff]);
        let up: Vec<u8> = pattern.chunks(SIZE[0]).rev().flatten().flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8, (pixel >> 24) as u8]).collect();
        std::fs::write(at("bmp-places.bmp"), file(&places, &[], &up)).unwrap();
        // What ffmpeg decodes of each: as PNG; and for 16-bit ones, the raw 16-bit values
        // (which it scales to bytes by truncating).
        for (name, ..) in KEPT {
            run(&["-i", &at(&format!("{name}.bmp")), "-pix_fmt", "rgba", &at(&format!("{name}.bmp.png"))]);
        }
        for (name, how) in [("555", "rgb555le"), ("565", "rgb565le"), ("444", "rgb444le")] {
            run(&["-i", &at(&format!("bmp-{name}.bmp")), "-f", "rawvideo", "-pix_fmt", how, &at(&format!("bmp-{name}.bmp.raw"))]);
        }
        // Icons: two sizes in one file, as BMP and as PNG. ffmpeg decodes the larger.
        std::fs::write(work.join("small.png"), png::encode_clear(9, &pattern[..63]).unwrap()).unwrap();
        run(&["-i", "small.png", "-i", "in.png", "-map", "0", "-map", "1", "-c:v", "bmp", "-pix_fmt", "bgra", &at("icon.ico")]);
        run(&["-i", "in.png", "-i", "small.png", "-map", "0", "-map", "1", "-c:v", "png", &at("icon-png.ico")]);
        for name in ["icon", "icon-png"] {
            run(&["-i", &at(&format!("{name}.ico")), "-pix_fmt", "rgba", &at(&format!("{name}.ico.png"))]);
        }
        std::fs::remove_dir_all(work).unwrap();
    }

    /// The test files with whole-byte or palette pixels, each beside ffmpeg's decoding.
    const KEPT: [(&str, &[u8], &[u8]); 11] = [
        // By ffmpeg: 24-bit, 32-bit (with alpha), 8-bit palette of 256 colours or greys, and
        // 1-bit (black and white).
        ("bmp-24", include_bytes!("../tests/pictures/bmp-24.bmp"), include_bytes!("../tests/pictures/bmp-24.bmp.png")),
        ("bmp-32", include_bytes!("../tests/pictures/bmp-32.bmp"), include_bytes!("../tests/pictures/bmp-32.bmp.png")),
        ("bmp-8", include_bytes!("../tests/pictures/bmp-8.bmp"), include_bytes!("../tests/pictures/bmp-8.bmp.png")),
        ("bmp-grey", include_bytes!("../tests/pictures/bmp-grey.bmp"), include_bytes!("../tests/pictures/bmp-grey.bmp.png")),
        ("bmp-1", include_bytes!("../tests/pictures/bmp-1.bmp"), include_bytes!("../tests/pictures/bmp-1.bmp.png")),
        // By hand: 16 colours; the same with the oldest header; RLE at 8 and 4 bits; top-down
        // rows; and the longest header, with channels in another order.
        ("bmp-4", include_bytes!("../tests/pictures/bmp-4.bmp"), include_bytes!("../tests/pictures/bmp-4.bmp.png")),
        ("bmp-old", include_bytes!("../tests/pictures/bmp-old.bmp"), include_bytes!("../tests/pictures/bmp-old.bmp.png")),
        ("bmp-runs-8", include_bytes!("../tests/pictures/bmp-runs-8.bmp"), include_bytes!("../tests/pictures/bmp-runs-8.bmp.png")),
        ("bmp-runs-4", include_bytes!("../tests/pictures/bmp-runs-4.bmp"), include_bytes!("../tests/pictures/bmp-runs-4.bmp.png")),
        ("bmp-top", include_bytes!("../tests/pictures/bmp-top.bmp"), include_bytes!("../tests/pictures/bmp-top.bmp.png")),
        ("bmp-places", include_bytes!("../tests/pictures/bmp-places.bmp"), include_bytes!("../tests/pictures/bmp-places.bmp.png")),
    ];

    #[test]
    fn pictures_come_out_as_ffmpeg_reads_them() {
        for (name, bmp, wanted) in KEPT {
            let (read, wanted) = (decode(bmp).unwrap_or_else(|| panic!("{name}.bmp is read")), png::decode(wanted).unwrap());
            let differ = read.1.iter().zip(&wanted.1).filter(|(one, other)| one != other).count();
            assert!(read == wanted, "{name}: {differ} of its {} pixels differ", wanted.1.len());
        }
        // The hand-made ones also match their source image.
        let sixteen: Vec<u32> = sixteen(4).chunks(4).map(|c| 0xff00_0000 | (c[2] as u32) << 16 | (c[1] as u32) << 8 | c[0] as u32).collect();
        let numbered: Vec<u32> = (0..SIZE[0] * SIZE[1]).map(|at| sixteen[numbered(at % SIZE[0], at / SIZE[0]) as usize]).collect();
        assert!(decode(KEPT[5].1).unwrap().1 == numbered && decode(KEPT[6].1).unwrap().1 == numbered);
        assert!(decode(KEPT[10].1).unwrap().1 == pattern());
        assert!(decode(KEPT[9].1).unwrap().1 == pattern().iter().map(|pixel| pixel | 0xff00_0000).collect::<Vec<u32>>());
        // In the RLE ones, skipped pixels get palette entry 0: part of a row, a row's end, a
        // whole row, and the top row.
        let runs = decode(KEPT[7].1).unwrap().1;
        let left_out = |x: usize, y: usize| matches!((16 - y) % 5, 2 if (5..9).contains(&x)) || matches!((16 - y) % 5, 3 if x >= 11) || (16 - y) % 5 == 4 || y == 0;
        assert!((0..SIZE[0] * SIZE[1]).all(|at| runs[at] == if left_out(at % 23, at / 23) { sixteen[0] } else { numbered[at] }));
        assert!(runs == decode(KEPT[8].1).unwrap().1);
    }

    /// A 16-bit test file: its name, the file, ffmpeg's raw 16-bit decoding, and the red, green
    /// and blue bit counts.
    type Sixteen = (&'static str, &'static [u8], &'static [u8], [u32; 3]);

    #[test]
    fn pixels_of_sixteen_bits_are_stretched_by_rounding() {
        // Beside each file, ffmpeg's raw 16-bit values, little-endian. Each field is scaled to
        // 0..255 and rounded, so the maximum gives 255. (ffmpeg, asked for bytes, truncates:
        // its maximum from four bits is 240.)
        let files: [Sixteen; 3] = [
            ("555", include_bytes!("../tests/pictures/bmp-555.bmp"), include_bytes!("../tests/pictures/bmp-555.bmp.raw"), [5, 5, 5]),
            ("565", include_bytes!("../tests/pictures/bmp-565.bmp"), include_bytes!("../tests/pictures/bmp-565.bmp.raw"), [5, 6, 5]),
            ("444", include_bytes!("../tests/pictures/bmp-444.bmp"), include_bytes!("../tests/pictures/bmp-444.bmp.raw"), [4, 4, 4]),
        ];
        for (name, bmp, raw, bits) in files {
            let stretched = |value: u32, above: u32, bits: u32| ((value >> above & ((1 << bits) - 1)) * 255 + ((1 << bits) - 1) / 2) / ((1 << bits) - 1);
            let wanted = raw.chunks(2).map(|pair| {
                let value = u16::from_le_bytes([pair[0], pair[1]]) as u32;
                0xff00_0000 | stretched(value, bits[1] + bits[2], bits[0]) << 16 | stretched(value, bits[2], bits[1]) << 8 | stretched(value, 0, bits[2])
            });
            let (width, pixels) = decode(bmp).unwrap();
            assert!(width == 23 && pixels.len() == 23 * 17 && pixels.iter().copied().eq(wanted), "bmp-{name}");
        }
    }

    /// The colour of palette entry `n` of the 16.
    fn listed(n: u8) -> u32 {
        let colour = &sixteen(4)[n as usize * 4..];
        0xff00_0000 | (colour[2] as u32) << 16 | (colour[1] as u32) << 8 | colour[0] as u32
    }

    #[test]
    fn pixels_lie_where_the_file_says() {
        // 2 bits per pixel: four colours, four pixels per byte, the first in the highest bits.
        let two = file(&said(40, [5, 1], 2, 0, 4), &sixteen(4)[..16], &[0b00_01_10_11, 0b10_00_00_00, 0, 0]);
        assert_eq!(decode(&two), Some((5, [0, 1, 2, 3, 2].map(listed).to_vec())));
        // 10 bits each of red, green and blue, as three masks after the first 40 bytes: scaled
        // to 0..255 and rounded. No alpha mask: opaque.
        let places = |places: &[u32]| places.iter().flat_map(|place| place.to_le_bytes()).collect::<Vec<u8>>();
        let rows = |pixels: &[u32]| pixels.iter().flat_map(|pixel| pixel.to_le_bytes()).collect::<Vec<u8>>();
        let ten = file(&[said(40, [3, 1], 32, 3, 0), places(&[0x3ff0_0000, 0x000f_fc00, 0x0000_03ff])].concat(), &[], &rows(&[1023 << 20, 512 << 10, 1 | 1023 << 10]));
        assert_eq!(decode(&ten), Some((3, vec![0xffff_0000, 0xff00_8000, 0xff00_ff00])));
        // 4 bits each, and 4 of alpha, in a header long enough to hold the alpha mask.
        let mut four = said(56, [2, 1], 16, 3, 0);
        four[40..].copy_from_slice(&places(&[0x0f00, 0x00f0, 0x000f, 0xf000]));
        assert_eq!(decode(&file(&four, &[], &[0x0f, 0xf0, 0xf0, 0x80])), Some((2, vec![0xff00_00ff, 0x8800_ff00])));
        // The same as four masks after the first 40 bytes, in a file whose compression says so.
        let told = file(&[said(40, [1, 1], 32, 6, 0), places(&[0xff00_0000, 0x00ff_0000, 0x0000_ff00, 0x0000_00ff])].concat(), &[], &rows(&[0x1122_3380]));
        assert_eq!(decode(&told), Some((1, vec![0x8011_2233])));
        // 4 bytes per pixel, the fourth 0 in all: opaque. Nonzero in one: then they are alpha.
        let solid = |last: u8| decode(&file(&said(40, [2, 1], 32, 0, 0), &[], &[1, 2, 3, 0, 4, 5, 6, last]));
        assert_eq!(solid(0), Some((2, vec![0xff03_0201, 0xff06_0504])));
        assert_eq!(solid(0x80), Some((2, vec![0x0003_0201, 0x8006_0504])));
    }

    /// A hand-made ICO of these images.
    fn icon(pictures: &[Vec<u8>]) -> Vec<u8> {
        let (mut at, mut out) = (6 + 16 * pictures.len(), [&[0, 0, 1, 0][..], &(pictures.len() as u16).to_le_bytes()].concat());
        for picture in pictures {
            out.extend([&[0; 8][..], &(picture.len() as u32).to_le_bytes(), &(at as u32).to_le_bytes()].concat());
            at += picture.len();
        }
        [out, pictures.concat()].concat()
    }

    /// An ICO image by hand: `bits` per pixel, each the value `value` gives, then a 1-bit mask
    /// set where `clear` says.
    fn held(bits: usize, [wide, high]: [usize; 2], value: impl Fn(usize, usize) -> u32, clear: impl Fn(usize, usize) -> bool) -> Vec<u8> {
        let (mut rows, mut bit_rows) = (Vec::new(), Vec::new());
        for y in (0..high).rev() {
            let (mut row, mut bit_row) = (vec![0u8; (wide * bits).div_ceil(32) * 4], vec![0u8; wide.div_ceil(32) * 4]);
            for x in 0..wide {
                let bytes = (value(x, y) << (32 - bits - x * bits % 8)).to_be_bytes();
                (0..4.min(row.len() - x * bits / 8)).for_each(|n| row[x * bits / 8 + n] |= if bits <= 8 { bytes[n] } else { value(x, y).to_le_bytes()[n] });
                bit_row[x / 8] |= if clear(x, y) { 0x80 >> (x % 8) } else { 0 };
            }
            rows.extend(row);
            bit_rows.extend(bit_row);
        }
        [said(40, [wide as i32, 2 * high as i32], bits as u16, 0, 0), if bits <= 8 { sixteen(4) } else { Vec::new() }, rows, bit_rows].concat()
    }

    #[test]
    fn an_icon_is_read_as_the_largest_picture_in_it() {
        // By ffmpeg: one icon at two sizes, as BMP (the larger last) and as PNG (the larger
        // first). The larger, 23 by 17, is what ffmpeg decodes, and what this does.
        let by_ffmpeg: [(&str, &[u8], &[u8]); 2] = [
            ("icon", include_bytes!("../tests/pictures/icon.ico"), include_bytes!("../tests/pictures/icon.ico.png")),
            ("icon-png", include_bytes!("../tests/pictures/icon-png.ico"), include_bytes!("../tests/pictures/icon-png.ico.png")),
        ];
        for (name, ico, wanted) in by_ffmpeg {
            assert!(decode_icon(ico) == png::decode(wanted) && decode_icon(ico).is_some_and(|read| read.0 == 23), "{name}");
        }
        // By hand, 16 colours, 5 by 3: every other pixel is masked transparent, and is.
        let (numbers, checked) = (|x: usize, y: usize| numbered(x, y) as u32, |x: usize, y: usize| (x + y) % 2 == 1);
        let wanted = |colour: &dyn Fn(usize, usize) -> u32| (0..15).map(|at| if checked(at % 5, at / 5) { 0 } else { colour(at % 5, at / 5) }).collect::<Vec<u32>>();
        let sixteen = held(4, [5, 3], numbers, checked);
        assert_eq!(decode_icon(&icon(std::slice::from_ref(&sixteen))), Some((5, wanted(&|x, y| listed(numbered(x, y))))));
        // 3 bytes per pixel, the same way. 4 bytes with no alpha: the same. 4 bytes with alpha:
        // then the mask is ignored.
        let colour = |x: usize, y: usize| 0x0010_2030 + (x * 5 + y * 0x300) as u32;
        assert_eq!(decode_icon(&icon(&[held(24, [5, 3], colour, checked)])), Some((5, wanted(&|x, y| 0xff00_0000 | colour(x, y)))));
        assert_eq!(decode_icon(&icon(&[held(32, [5, 3], colour, checked)])), Some((5, wanted(&|x, y| 0xff00_0000 | colour(x, y)))));
        let says = |x: usize, y: usize| 0x4000_0000 | colour(x, y);
        assert_eq!(decode_icon(&icon(&[held(32, [5, 3], says, checked)])), Some((5, (0..15).map(|at| says(at % 5, at / 5)).collect())));
        // Of several, the one with the most pixels, wherever it is; of two the same size, the
        // deeper.
        let (larger, deeper) = (held(24, [6, 3], colour, |_, _| false), held(24, [5, 3], colour, |_, _| false));
        assert!([[&sixteen, &larger], [&larger, &sixteen]].iter().all(|pair| decode_icon(&icon(&pair.map(|picture| picture.clone()))).is_some_and(|read| read.0 == 6)));
        assert!([[&sixteen, &deeper], [&deeper, &sixteen]].iter().all(|pair| decode_icon(&icon(&pair.map(|picture| picture.clone()))) == decode_icon(&icon(std::slice::from_ref(&deeper)))));
        // A cursor file reads alike. One of another type, one with no images, and one whose
        // only image lies outside it, are `None`.
        let mut file = icon(&[sixteen]);
        file[2] = 2;
        assert!(decode_icon(&file).is_some());
        file[2] = 3;
        assert!(decode_icon(&file).is_none() && decode_icon(&icon(&[])).is_none());
        file[2..4].copy_from_slice(&[1, 0]);
        file[18..22].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
        assert!(decode_icon(&file).is_none() && decode_icon(&file[..5]).is_none());
    }

    #[test]
    fn what_a_file_says_wrongly_gives_nothing() {
        let good = file(&said(40, [2, 2], 24, 0, 0), &[], &[0; 16]);
        let with = |change: &dyn Fn(&mut Vec<u8>)| {
            let mut bmp = good.clone();
            change(&mut bmp);
            decode(&bmp)
        };
        assert!(decode(&good).is_some());
        // Not a BMP. A header length no version has. A bit depth none has. An unread
        // compression (JPEG inside). Zero width, zero height, negative width.
        assert!(with(&|bmp| bmp[0] = b'C').is_none() && with(&|bmp| bmp[14] = 20).is_none());
        assert!(with(&|bmp| bmp[28] = 3).is_none() && with(&|bmp| bmp[30] = 4).is_none());
        assert!(with(&|bmp| bmp[18] = 0).is_none() && with(&|bmp| bmp[22] = 0).is_none() && with(&|bmp| bmp[21] = 0x80).is_none());
        // Missing rows: though the last row's padding may be missing.
        assert!(decode(&good[..good.len() - 2]).is_some() && decode(&good[..good.len() - 3]).is_none());
        // A size no file could hold: `None`, at once.
        assert!(with(&|bmp| bmp[18..26].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f, 0xff, 0xff, 0xff, 0x7f])).is_none());
        // RLE can claim "the rest is empty" at any size: more pixels than so many per byte is
        // refused.
        let empty = |size: [i32; 2]| decode(&file(&said(40, size, 8, 1, 2), &[9, 8, 7, 0, 0, 0, 0, 0], &[0, 1]));
        assert_eq!(empty([16, 16]), Some((16, vec![0xff07_0809; 256])));
        assert!(empty([4000, 4000]).is_none());
    }

    #[test]
    fn a_damaged_file_gives_nothing_or_some_picture() {
        let sound = |read: Option<(usize, Vec<u32>)>| read.is_none_or(|(width, pixels)| width > 0 && !pixels.is_empty() && pixels.len() % width == 0);
        let more: [(&str, &[u8]); 4] = [
            ("bmp-555", include_bytes!("../tests/pictures/bmp-555.bmp")),
            ("bmp-565", include_bytes!("../tests/pictures/bmp-565.bmp")),
            ("icon", include_bytes!("../tests/pictures/icon.ico")),
            ("icon-png", include_bytes!("../tests/pictures/icon-png.ico")),
        ];
        // Every test file: truncated at every length; then whole, with each byte in turn
        // changed: a bit flipped, 0, and 255.
        for (name, file) in KEPT.iter().map(|kept| (kept.0, kept.1)).chain(more) {
            let read = |file: &[u8]| if name.starts_with("icon") { decode_icon(file) } else { decode(file) };
            assert!((0..file.len()).all(|cut| sound(read(&file[..cut]))), "{name}, cut short");
            for at in 0..file.len() {
                let damaged = |to: u8| read(&[&file[..at], &[to], &file[at + 1..]].concat());
                assert!([file[at] ^ (1 << (at % 8)), 0, 255].into_iter().all(|to| sound(damaged(to))), "{name}, byte {at}");
            }
        }
    }
    /// An icon image of `width` by `height`, for the encoding tests: colours varying across and
    /// down, some pixels fully transparent (with a colour under them anyway), some half, the
    /// rest opaque.
    fn icon_picture(width: usize, height: usize) -> Vec<u32> {
        let solid = |x: usize, y: usize| [0, 0x80, 0xff, 0xff][(x * 7 + y * 3) % 4] << 24;
        (0..width * height).map(|at| solid(at % width, at / width) | ((at % width * 255 / width) as u32) << 16 | ((at / width * 255 / height) as u32) << 8 | 0x40).collect()
    }

    /// The sizes the tests encode: those Windows asks for, and two non-square ones.
    const ICON_SIZES: [[usize; 2]; 8] = [[16, 16], [24, 24], [32, 32], [48, 48], [64, 64], [256, 256], [5, 3], [7, 256]];

    #[test]
    fn an_icon_written_is_read_back_at_every_size() {
        // Each size alone in a file round-trips: smaller ones as BMP, those 256 wide or high as
        // PNG.
        for [width, height] in ICON_SIZES {
            let pixels = icon_picture(width, height);
            let ico = encode_icon(&[(width, &pixels)]).expect("is written");
            assert_eq!(decode_icon(&ico), Some((width, pixels)), "{width} by {height}");
            assert_eq!(ico[22..26] == *b"\x89PNG", width == 256 || height == 256, "{width} by {height}: kept as a PNG");
        }
        // All of them in one file, whose directory gives each one's size (256 as 0) and offset:
        // the largest is read.
        let pictures: Vec<(usize, Vec<u32>)> = ICON_SIZES.iter().map(|[width, height]| (*width, icon_picture(*width, *height))).collect();
        let lent: Vec<(usize, &[u32])> = pictures.iter().map(|(width, pixels)| (*width, &pixels[..])).collect();
        let ico = encode_icon(&lent).unwrap();
        assert_eq!(decode_icon(&ico), Some((256, icon_picture(256, 256))));
        let said: Vec<[u8; 2]> = ico[6..].as_chunks::<16>().0.iter().take(8).map(|entry| [entry[0], entry[1]]).collect();
        assert_eq!(said, [[16, 16], [24, 24], [32, 32], [48, 48], [64, 64], [0, 0], [5, 3], [7, 0]]);
        // The mask after a small image marks its fully transparent pixels, row by row
        // bottom-up, each row in 4 bytes.
        let small = encode_icon(&[(5, &icon_picture(5, 3))]).unwrap();
        let clear: Vec<u8> = (0..3).rev().map(|y| (0..5).fold(0, |bits, x| bits | ((icon_picture(5, 3)[y * 5 + x] >> 24 == 0) as u8) << (7 - x))).collect();
        assert_eq!(small[small.len() - 12..], [clear[0], 0, 0, 0, clear[1], 0, 0, 0, clear[2], 0, 0, 0]);
    }

    #[test]
    fn what_cannot_be_an_icon_is_not_written() {
        let pixels = icon_picture(16, 16);
        assert!(encode_icon(&[(16, &pixels)]).is_some());
        assert_eq!([encode_icon(&[]), encode_icon(&[(0, &pixels)]), encode_icon(&[(16, &pixels[..8])]), encode_icon(&[(16, &pixels), (3, &[])])], [None, None, None, None]);
        assert_eq!([encode_icon(&[(257, &vec![0; 257])]), encode_icon(&[(1, &vec![0; 257])])], [None, None]);
    }

    /// What Windows makes of an icon file: the image it loads for a size, read back as the
    /// icon's colour bitmap (blue, green, red, alpha).
    #[cfg(windows)]
    fn loaded_by_windows(file: &std::path::Path, [width, height]: [usize; 2]) -> Option<Vec<u32>> {
        #[repr(C)]
        struct Header {
            size: u32,
            width: i32,
            height: i32,
            planes: u16,
            bits: u16,
            rest: [u32; 6],
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn LoadImageW(instance: isize, name: *const u16, kind: u32, width: i32, height: i32, load: u32) -> isize;
            fn GetIconInfo(icon: isize, info: *mut ICONINFO) -> i32;
            fn DestroyIcon(icon: isize) -> i32;
        }
        #[link(name = "gdi32")]
        unsafe extern "system" {
            fn CreateCompatibleDC(dc: isize) -> isize;
            fn GetDIBits(dc: isize, bitmap: isize, first: u32, lines: u32, bits: *mut u32, info: *mut Header, usage: u32) -> i32;
            fn DeleteDC(dc: isize) -> i32;
            fn DeleteObject(object: isize) -> i32;
        }
        use crate::windows::sys::ICONINFO;
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();
        let mut pixels = vec![0u32; width * height];
        // SAFETY: the name is NUL-terminated; the icon, its bitmaps and the DC are released
        // before returning; GetDIBits writes `height` rows of `width` 32-bit pixels, top-down
        // (negative height), which `pixels` has room for.
        unsafe {
            // IMAGE_ICON, LR_LOADFROMFILE.
            let icon = LoadImageW(0, name.as_ptr(), 1, width as i32, height as i32, 0x10);
            let mut info = ICONINFO::default();
            if icon == 0 || GetIconInfo(icon, &mut info) == 0 {
                return None;
            }
            let (dc, mut header) = (CreateCompatibleDC(0), Header { size: 40, width: width as i32, height: -(height as i32), planes: 1, bits: 32, rest: [0; 6] });
            let read = GetDIBits(dc, info.color, 0, height as u32, pixels.as_mut_ptr(), &mut header, 0);
            DeleteDC(dc);
            DeleteObject(info.color);
            DeleteObject(info.mask);
            DestroyIcon(icon);
            (read == height as i32).then_some(pixels)
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_loads_a_written_icon_at_every_size() {
        // One file of every size: Windows' icon loading takes the image of each requested size
        // from it as written, PNG ones too.
        let pictures: Vec<(usize, Vec<u32>)> = ICON_SIZES.iter().map(|[width, height]| (*width, icon_picture(*width, *height))).collect();
        let lent: Vec<(usize, &[u32])> = pictures.iter().map(|(width, pixels)| (*width, &pixels[..])).collect();
        let file = std::env::temp_dir().join(format!("taut-icon-{}.ico", std::process::id()));
        std::fs::write(&file, encode_icon(&lent).unwrap()).unwrap();
        for [width, height] in ICON_SIZES.into_iter().filter(|[width, height]| width == height) {
            assert_eq!(loaded_by_windows(&file, [width, height]), Some(icon_picture(width, height)), "{width} by {height}");
        }
        std::fs::remove_file(file).unwrap();
    }
}
