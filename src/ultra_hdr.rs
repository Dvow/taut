//! Ultra HDR JPEGs (the feature `hdr`, see `hdr`): a JPEG whose picture is followed by a second
//! JPEG, its gain map, saying how much brighter each part is in HDR (ISO 21496-1, Adobe's gain
//! maps, Android's Ultra HDR, Apple's HDR gain maps). Read as libultrahdr reads them:
//!
//! - The picture: the first JPEG, as every reader shows it.
//! - The map: the first JPEG after it whose metadata says how its values become gains: ISO
//!   21496-1's (APP2), else the `hdrgm` XMP of Adobe and Android (APP1), else Apple's
//!   `HDRGainMapHeadroom` (its XMP, or the picture's EXIF maker notes). Decoded as stored (no
//!   colour profile), turned as the picture is, kept at its own size: `hdr` samples it where
//!   the picture is shown.
//! - Read as an SDR JPEG instead: a map larger than the picture, damaged metadata, or a map
//!   made from an HDR picture to an SDR one (ISO's "backward direction", `BaseRenditionIsHDR`).
use crate::hdr::{Curve, GainMap};

/// What comes before the ISO 21496-1 metadata in an APP2 segment.
const ISO: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
/// What comes before XMP in an APP1 segment.
const XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// What comes before EXIF notes in an APP1 segment.
const EXIF: &[u8] = b"Exif\0\0";
/// The namespace of Adobe's gain map XMP.
const GAIN_MAP: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
/// What Apple's maker notes start with in EXIF.
const APPLE_NOTES: &[u8] = b"Apple iOS\0\0\x01MM";

/// Reads an Ultra HDR JPEG: its width, its SDR pixels as `Ui::add_picture` takes them, and its
/// gain map. Nothing for a JPEG without a gain map, or one read as SDR (see above).
pub(crate) fn read(file: &[u8]) -> Option<(usize, Vec<u32>, GainMap)> {
    let exif = segments(file).find_map(|(marker, body)| body.strip_prefix(EXIF).filter(|_| marker == 0xE1));
    let (map, curve) = gain_map_of(file, exif)?;
    let (width, sdr) = crate::jpeg::decode(file)?;
    let turn = exif.and_then(crate::exif_orientation::turn);
    let map = decoded_map(map, curve, turn, [width, sdr.len() / width.max(1)])?;
    Some((width, sdr, map))
}

/// The gain map's JPEG in `file`, and how its values become gains: the first JPEG after the
/// picture whose metadata says so. `exif`: the picture's EXIF notes.
fn gain_map_of<'a>(file: &'a [u8], exif: Option<&[u8]>) -> Option<(&'a [u8], Curve)> {
    let mut after = length(file)?;
    loop {
        let start = after + file.get(after..)?.windows(3).position(|bytes| bytes == [0xFF, 0xD8, 0xFF])?;
        let jpeg = &file[start..start + length(&file[start..])?];
        if let Some(curve) = curve_of(jpeg, exif) {
            return Some((jpeg, curve));
        }
        after = start + jpeg.len();
    }
}

/// The gain map in `jpeg` (a JPEG of grey or colour gains): decoded as stored, turned `turn`
/// way (EXIF), as values 0 to 1. Nothing where its frame says it is larger than the picture's
/// `size` (or says its height later): it is then not decoded.
fn decoded_map(jpeg: &[u8], curve: Curve, turn: Option<usize>, size: [usize; 2]) -> Option<GainMap> {
    // The frame's header: precision, height, width, components.
    let frame = segments(jpeg).find_map(|(marker, body)| body.get(..6).filter(|_| matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)))?;
    let [high, wide] = [[frame[1], frame[2]], [frame[3], frame[4]]].map(|two| u16::from_be_bytes(two) as usize);
    let [across, down] = if turn.is_some_and(|turn| turn >= 5) { [high, wide] } else { [wide, high] };
    if across == 0 || down == 0 || across > size[0] || down > size[1] {
        return None;
    }
    let (width, pixels) = crate::jpeg::decode(&kept(jpeg, |marker, _| !matches!(marker, 0xE1 | 0xE2)))?;
    let (width, pixels) = crate::exif_orientation::turned(turn, width, pixels);
    let colours = if frame[5] == 1 { 1 } else { 3 };
    let mut values = Vec::with_capacity(pixels.len() * colours);
    for pixel in pixels {
        values.extend(pixel.to_be_bytes()[1..=colours].iter().map(|byte| crate::hdr::half(*byte as f32 / 255.0)));
    }
    Some(GainMap { width, colours, values, curve, serial: 0 })
}

/// A JPEG with only the segments before its first scan that `keep` keeps (by marker and
/// body). (Without its EXIF notes and colour profile, APP1 and APP2, its samples are read as
/// stored: not turned or converted.)
fn kept(jpeg: &[u8], keep: impl Fn(u8, &[u8]) -> bool) -> Vec<u8> {
    let (mut made, mut header) = (vec![0xFF, 0xD8], 2);
    for (marker, body) in segments(jpeg) {
        header += body.len() + 4;
        if keep(marker, body) {
            made.extend_from_slice(&jpeg[header - body.len() - 4..header]);
        }
    }
    made.extend_from_slice(&jpeg[header..]);
    made
}

/// A JPEG's segments before its first scan: each marker and body.
fn segments(jpeg: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut rest = jpeg.get(2..).unwrap_or_default();
    std::iter::from_fn(move || {
        let &[0xFF, marker, high, low, ..] = rest else { return None };
        let length = u16::from_be_bytes([high, low]) as usize;
        let body = rest.get(4..2 + length).filter(|_| !matches!(marker, 0xD9 | 0xDA))?;
        rest = &rest[2 + length..];
        Some((marker, body))
    })
}

/// How long the JPEG at the start of `file` is, to its end marker; nothing where it has none.
fn length(file: &[u8]) -> Option<usize> {
    let mut at = 2;
    if !file.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    loop {
        let &[0xFF, marker] = file.get(at..at + 2)? else { return None };
        at += match marker {
            0xD9 => return Some(at + 2),
            0xFF => 1,
            0x01 | 0xD0..=0xD7 => 2,
            _ => 2 + u16::from_be_bytes([*file.get(at + 2)?, *file.get(at + 3)?]) as usize,
        };
        if marker == 0xDA {
            at = scanned(file, at)?;
        }
    }
}

/// Where the coded data of a scan starting at `at` ends: the next marker that is not a restart
/// or a stuffed zero.
fn scanned(file: &[u8], mut at: usize) -> Option<usize> {
    loop {
        at += file.get(at..)?.iter().position(|byte| *byte == 0xFF)?;
        match *file.get(at + 1)? {
            0x00 | 0xD0..=0xD7 => at += 2,
            0xFF => at += 1,
            _ => return Some(at),
        }
    }
}

/// How the values of gain map `jpeg` become gains, by its metadata: ISO 21496-1, else Adobe's
/// XMP, else Apple's (`exif`: the picture's EXIF notes). Nothing where it has none, or damaged.
fn curve_of(jpeg: &[u8], exif: Option<&[u8]>) -> Option<Curve> {
    if let Some(iso) = segments(jpeg).find_map(|(marker, body)| body.strip_prefix(ISO).filter(|_| marker == 0xE2)) {
        return iso_curve(iso).filter(sound);
    }
    let xmp = segments(jpeg).find_map(|(marker, body)| body.strip_prefix(XMP).filter(|_| marker == 0xE1))?;
    let xmp = std::str::from_utf8(xmp).ok()?;
    adobe_curve(xmp).or_else(|| apple_curve(xmp, exif)).filter(sound)
}

/// Whether a curve is one libultrahdr takes: every number finite, each colour's most gain not
/// under its least, gammas above 0, offsets not below it, its full headroom not under its least.
fn sound(curve: &Curve) -> bool {
    let ends = [curve.offset, curve.none, curve.full];
    curve.gamma.iter().chain(&curve.low).chain(&curve.high).chain(&ends).all(|number| number.is_finite())
        && (0..3).all(|colour| curve.high[colour] >= curve.low[colour] && curve.gamma[colour] > 0.0)
        && curve.offset >= 0.0
        && curve.full >= curve.none
}

/// ISO 21496-1's metadata: version 0, then flags (three colours, one denominator for all, the
/// map made from the HDR picture), then fractions: the least and full headroom, and each
/// colour's least and most gain (stops), gamma, SDR and HDR offset.
fn iso_curve(data: &[u8]) -> Option<Curve> {
    let mut at = 0;
    let mut next = |bytes: usize| {
        let read = data.get(at..at + bytes)?.iter().fold(0u32, |sum, byte| sum << 8 | *byte as u32);
        at += bytes;
        Some(read)
    };
    let (least_version, _writer, flags) = (next(2)?, next(2)?, next(1)?);
    if least_version != 0 || flags & 4 != 0 {
        return None;
    }
    let shared = if flags & 8 != 0 { Some(next(4)?) } else { None };
    let mut fraction = |signed: bool| {
        let (numerator, denominator) = (
            next(4)?,
            match shared {
                Some(shared) => shared,
                None => next(4)?,
            },
        );
        let numerator = if signed { numerator as i32 as f32 } else { numerator as f32 };
        (denominator != 0).then(|| numerator / denominator as f32)
    };
    let (none, full) = (fraction(false)?, fraction(false)?);
    let mut curve = Curve { gamma: [1.0; 3], low: [0.0; 3], high: [0.0; 3], offset: 0.0, none, full };
    let colours = if flags & 0x80 != 0 { 3 } else { 1 };
    let mut offsets = 0.0;
    for colour in 0..colours {
        [curve.low[colour], curve.high[colour], curve.gamma[colour]] = [fraction(true)?, fraction(true)?, fraction(false)?];
        offsets += fraction(true)? + fraction(true)?;
    }
    for colour in colours..3 {
        (curve.low[colour], curve.high[colour], curve.gamma[colour]) = (curve.low[0], curve.high[0], curve.gamma[0]);
    }
    curve.offset = offsets / (2 * colours) as f32;
    Some(curve)
}

/// Adobe's gain map XMP (Android's Ultra HDR writes it too): version 1.0, the most gain and full
/// headroom (stops) said; the rest as its defaults where not.
fn adobe_curve(xmp: &str) -> Option<Curve> {
    let prefix = prefix_of(xmp, GAIN_MAP)?;
    let numbers_or = |name: &str, otherwise: f32| said(xmp, prefix, name).map_or(Some([otherwise; 3]), numbers);
    let hdr_base = said(xmp, prefix, "BaseRenditionIsHDR").is_some_and(|said| said.trim().eq_ignore_ascii_case("true"));
    if said(xmp, prefix, "Version")?.trim() != "1.0" || hdr_base {
        return None;
    }
    let (high, [full, ..]) = (said(xmp, prefix, "GainMapMax").and_then(numbers)?, said(xmp, prefix, "HDRCapacityMax").and_then(numbers)?);
    let (low, gamma, [none, ..]) = (numbers_or("GainMapMin", 0.0)?, numbers_or("Gamma", 1.0)?, numbers_or("HDRCapacityMin", 0.0)?);
    let (sdr, hdr) = (numbers_or("OffsetSDR", 1.0 / 64.0)?, numbers_or("OffsetHDR", 1.0 / 64.0)?);
    let offset = sdr.iter().chain(&hdr).sum::<f32>() / 6.0;
    Some(Curve { gamma, low, high, offset, none, full })
}

/// Apple's gain map XMP: its version said, its headroom (stops) said there or in the picture's
/// maker notes (`exif`). Gains from none to that headroom.
fn apple_curve(xmp: &str, exif: Option<&[u8]>) -> Option<Curve> {
    said_anywhere(xmp, "HDRGainMapVersion")?;
    let headroom = said_anywhere(xmp, "HDRGainMapHeadroom").and_then(|said| said.trim().parse().ok()).or_else(|| apple_headroom(exif?))?;
    Some(Curve { gamma: [1.0; 3], low: [0.0; 3], high: [headroom; 3], offset: 0.0, none: 0.0, full: headroom })
}

/// The prefix XMP gives `namespace` (`xmlns:prefix="namespace"`).
fn prefix_of<'a>(xmp: &'a str, namespace: &str) -> Option<&'a str> {
    xmp.split("xmlns:").skip(1).find_map(|declared| {
        let (prefix, value) = declared.split_once('=')?;
        let value = value.trim_start();
        let quote = value.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
        value[1..].strip_prefix(namespace)?.starts_with(quote).then_some(prefix.trim())
    })
}

/// What XMP says of `prefix:name`: an attribute's value, or an element's content.
fn said<'a>(xmp: &'a str, prefix: &str, name: &str) -> Option<&'a str> {
    let full = format!("{prefix}:{name}");
    let attribute = xmp.match_indices(&full).find_map(|(at, _)| quoted(xmp[at + full.len()..].trim_start().strip_prefix('=')?));
    attribute.or_else(|| {
        let start = xmp.find(&format!("<{full}>"))? + full.len() + 2;
        Some(&xmp[start..start + xmp[start..].find(&format!("</{full}>"))?])
    })
}

/// What XMP says of `name` under any prefix: an attribute's value, or an element's content.
fn said_anywhere<'a>(xmp: &'a str, name: &str) -> Option<&'a str> {
    xmp.match_indices(name).find_map(|(at, _)| {
        let rest = &xmp[at + name.len()..];
        rest.strip_prefix('>').and_then(|content| content.split('<').next()).or_else(|| quoted(rest.trim_start().strip_prefix('=')?))
    })
}

/// The text between the quotes `text` starts with (after spaces).
fn quoted(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let quote = text.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
    text[1..].split(quote).next()
}

/// One number for every colour, or three (an `rdf:Seq` of red, green and blue).
fn numbers(said: &str) -> Option<[f32; 3]> {
    let listed: Vec<f32> = said.split("<rdf:li>").skip(1).map(|item| item.split('<').next()?.trim().parse().ok()).collect::<Option<_>>()?;
    match listed[..] {
        [] => Some([said.trim().parse().ok()?; 3]),
        [red, green, blue] => Some([red, green, blue]),
        _ => None,
    }
}

/// The headroom Apple's maker notes give a photo, in stops (from its notes 33 and 48, as
/// libultrahdr reads them).
fn apple_headroom(exif: &[u8]) -> Option<f32> {
    let little = match exif.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let first = number(exif, 4, 4, little)? as usize;
    let notes = entry(exif, entry(exif, first, 0x8769, little)?.1 as usize, 0x927C, little)?.1 as usize;
    let maker = exif.get(notes..).filter(|maker| maker.starts_with(APPLE_NOTES))?;
    let rational = |tag: u16| -> Option<f64> {
        let (10, at) = entry(maker, APPLE_NOTES.len(), tag, false)? else { return None };
        let (numerator, denominator) = (number(maker, at as usize, 4, false)? as i32, number(maker, at as usize + 4, 4, false)?);
        (denominator != 0).then(|| numerator as f64 / denominator as f64)
    };
    let (note_33, note_48) = (rational(33), rational(48));
    note_33.or(note_48)?;
    let (note_33, note_48) = (note_33.unwrap_or(0.0), note_48.unwrap_or(0.0));
    let stops = match (note_33 < 1.0, note_48 <= 0.01) {
        (true, true) => -20.0 * note_48 + 1.8,
        (true, false) => -0.101 * note_48 + 1.601,
        (false, true) => -70.0 * note_48 + 3.0,
        (false, false) => -0.303 * note_48 + 2.303,
    };
    Some(stops as f32)
}

/// Entry `tag` of the TIFF list at `list` in `tiff`: its type and value (or where its value is).
fn entry(tiff: &[u8], list: usize, tag: u16, little: bool) -> Option<(u32, u32)> {
    let count = number(tiff, list, 2, little)? as usize;
    let at = (0..count).map(|entry| list + 2 + entry * 12).find(|at| number(tiff, *at, 2, little) == Some(tag as u32))?;
    Some((number(tiff, at + 2, 2, little)?, number(tiff, at + 8, 4, little)?))
}

/// The number of `bytes` bytes at `at` in `data`, little or big end first.
fn number(data: &[u8], at: usize, bytes: usize, little: bool) -> Option<u32> {
    let read = data.get(at..at.checked_add(bytes)?)?;
    let ordered = |sum: u32, byte: &u8| sum << 8 | *byte as u32;
    Some(if little { read.iter().rev().fold(0, ordered) } else { read.iter().fold(0, ordered) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// The test files: each one's name, what it is, whether libultrahdr marked its SDR picture
    /// Display P3, and how far from libultrahdr's decoding it may be read (a part of the light,
    /// at most).
    const MADE: [(&str, &str, bool, f32); 7] = [
        ("android", "libultrahdr from linear HDR, ISO and XMP: one gain for all colours, the map a quarter as wide", true, 0.03),
        ("colours", "libultrahdr: a gain per colour, the map as large as the picture", true, 0.03),
        // llvmpipe filters 8-bit textures in 8 bits: where gamma 2 makes the map steep near 0,
        // libultrahdr's GPU decoding is coarser than this one.
        ("gamma", "libultrahdr: the map's gamma 2, half as wide", true, 0.2),
        ("adobe", "Adobe's XMP alone: least gain 0.5, offsets 1/64, shown from 1.5 times SDR white", false, 0.03),
        ("iso", "ISO 21496-1 with a map of its own: each colour its own gains and gamma", false, 0.03),
        ("apple", "Apple's XMP: a headroom of 2.5 stops", true, 0.03),
        ("turned", "the android file, held turned (EXIF orientation 6)", true, 0.03),
    ];
    /// The test pictures' size.
    const SIZE: [usize; 2] = [48, 32];

    fn pictures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures")
    }

    /// sRGB's curve undone: a byte's linear value.
    fn linear(byte: u32) -> f32 {
        let part = byte as f32 / 255.0;
        if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
    }

    /// libultrahdr's decoding of test file `name` at full headroom: linear light per pixel (red,
    /// green, blue; 1.0 is SDR white).
    fn decoded(name: &str) -> Vec<[f32; 3]> {
        let raw = std::fs::read(pictures().join(format!("ultra-hdr-{name}.jpg.linear"))).unwrap();
        raw.chunks(8).map(|pixel| std::array::from_fn(|at| crate::hdr::from_half(u16::from_le_bytes([pixel[at * 2], pixel[at * 2 + 1]])))).collect()
    }

    /// What `read` makes of test file `name` at full headroom, shown pixel for pixel: its width,
    /// its SDR pixels, and their light: each SDR colour made linear, offset, times its gain,
    /// offset back (as `hdr`'s passes do).
    fn shown(name: &str) -> (usize, Vec<u32>, Vec<[f32; 3]>) {
        let file = std::fs::read(pictures().join(format!("ultra-hdr-{name}.jpg"))).unwrap();
        let (width, sdr, map) = read(&file).unwrap_or_else(|| panic!("{name}: an Ultra HDR JPEG"));
        let (shape, offset) = ([0.0, 0.0, width as f32, (sdr.len() / width) as f32], map.curve.offset);
        let light = |(at, pixel): (usize, &u32)| -> [f32; 3] {
            let gains = map.sampled(shape, (at % width) as i32, (at / width) as i32, 1.0);
            let colour = [pixel >> 16 & 0xff, pixel >> 8 & 0xff, pixel & 0xff].map(linear);
            std::array::from_fn(|part| ((colour[part] + offset) * gains[part] - offset).max(0.0))
        };
        let lit = sdr.iter().enumerate().map(light).collect();
        (width, sdr, lit)
    }

    /// libultrahdr's decoding of test file `name` as this crate shows it: turned where the file
    /// is (libultrahdr doesn't turn), and in sRGB's colours where it is in Display P3 and the
    /// feature `color-profiles` converts the SDR picture so.
    fn wanted(name: &str, p3: bool) -> Vec<[f32; 3]> {
        let decoded = decoded(if name == "turned" { "android" } else { name });
        let (_, order) = crate::exif_orientation::turned(if name == "turned" { Some(6) } else { None }, SIZE[0], (0..(SIZE[0] * SIZE[1]) as u32).collect());
        let to_srgb = crate::hdr::to_709(if p3 && cfg!(feature = "color-profiles") { 12 } else { 1 }).unwrap();
        let in_srgb = |pixel: [f32; 3]| to_srgb.map(|row| (row[0] * pixel[0] + row[1] * pixel[1] + row[2] * pixel[2]).max(0.0));
        order.into_iter().map(|at| in_srgb(decoded[at as usize])).collect()
    }

    #[test]
    fn ultra_hdr_jpegs_are_read_as_libultrahdr_decodes_them() {
        for (name, made, p3, most) in MADE {
            let ((width, sdr, got), wanted) = (shown(name), wanted(name, p3));
            assert_eq!((width, got.len()), (if name == "turned" { SIZE[1] } else { SIZE[0] }, wanted.len()), "{name}: its size");
            // (Where the SDR picture was converted to sRGB, colours outside it were cut: those
            // pixels can't be compared.)
            let cut = |pixel: u32| p3 && cfg!(feature = "color-profiles") && [16, 8, 0].iter().any(|shift| matches!(pixel >> shift & 0xff, 0 | 255));
            let compared = got.iter().zip(&wanted).zip(sdr).filter(|(_, sdr)| !cut(*sdr));
            let apart: Vec<f32> = compared.flat_map(|((got, wanted), _)| (0..3).map(|part| (got[part] - wanted[part]).abs() / wanted[part].max(0.05))).collect();
            let (worst, mean) = (apart.iter().copied().fold(0.0, f32::max), apart.iter().sum::<f32>() / apart.len() as f32);
            println!("{name} ({made}): {} pixels compared, apart by {:.2}% at most, {:.3}% on average", apart.len() / 3, worst * 100.0, mean * 100.0);
            // (Converted, by the profile LittleCMS would read, not by the standard's numbers: a few
            // percent more.)
            let most = if p3 && cfg!(feature = "color-profiles") { most + 0.05 } else { most };
            assert!(worst <= most && mean <= 0.02 && apart.len() * 2 > got.len() * 3, "{name}: apart by {worst} at most, {mean} on average");
        }
    }

    #[test]
    fn a_picture_file_keeps_an_ultra_hdr_jpegs_gain_map_and_its_sdr_picture() {
        let mut trial = crate::testing::Trial::new((), |_: &mut (), _: &mut crate::Ui| {});
        let file = std::fs::read(pictures().join("ultra-hdr-colours.jpg")).unwrap();
        let picture = trial.ui.add_picture_file(&file).expect("an Ultra HDR JPEG");
        let map = &trial.ui.light.maps[&picture.0];
        assert_eq!((map.width, map.colours, map.values.len()), (48, 3, 48 * 32 * 3));
        let sdr = &trial.ui.pictures.get(picture.0).expect("its picture").pixels;
        assert!(*sdr == crate::jpeg::decode(&file).unwrap().1, "the SDR picture every reader shows");
        let plain = trial.ui.add_picture_file(&std::fs::read(pictures().join("plain.jpg")).unwrap()).expect("a JPEG");
        assert!(!trial.ui.light.maps.contains_key(&plain.0), "no gain map");
    }

    #[test]
    fn a_jpeg_without_a_gain_map_or_with_a_broken_one_is_left_to_the_usual_reader() {
        let plain = std::fs::read(pictures().join("plain.jpg")).unwrap();
        assert!(read(&plain).is_none(), "one JPEG alone");
        let android = std::fs::read(pictures().join("ultra-hdr-android.jpg")).unwrap();
        // A second JPEG with no metadata saying how it brightens: no gain map.
        assert!(read(&[&plain[..], &plain[..]].concat()).is_none(), "two plain JPEGs");
        // Cut short in the map, or the map's ISO metadata damaged: read as SDR.
        assert!(read(&android[..android.len() - 40]).is_none(), "cut short");
        let iso = android.windows(ISO.len()).rposition(|bytes| bytes == ISO).unwrap() + ISO.len();
        let damaged = [&android[..iso], &[0, 1], &android[iso + 2..]].concat();
        assert!(read(&damaged).is_none(), "ISO 21496-1 of an unknown version");
        // A map larger than its picture: refused, the map read no further.
        let (map, curve) = gain_map_of(&android, None).unwrap();
        assert!(decoded_map(map, curve, None, [48, 32]).is_some() && decoded_map(map, curve, None, [8, 8]).is_none(), "a map larger than the picture");
    }

    #[test]
    fn xmp_says_its_numbers_under_any_prefix_as_attributes_or_elements_and_by_colour() {
        let xmp = "<rdf:Description xmlns:gm='http://ns.adobe.com/hdr-gain-map/1.0/' gm:Version='1.0' gm:HDRCapacityMax = \"2.5\">\
            <gm:GainMapMax><rdf:Seq><rdf:li>3</rdf:li><rdf:li>2</rdf:li><rdf:li>1</rdf:li></rdf:Seq></gm:GainMapMax><gm:Gamma>2</gm:Gamma></rdf:Description>";
        let curve = adobe_curve(xmp).expect("Adobe's XMP");
        assert_eq!((curve.high, curve.gamma, curve.low, curve.full, curve.none), ([3.0, 2.0, 1.0], [2.0; 3], [0.0; 3], 2.5, 0.0));
        assert_eq!(curve.offset, 1.0 / 64.0, "the default offsets");
        assert!(adobe_curve(&xmp.replace("'1.0'", "'2.0'")).is_none(), "another version");
        assert!(adobe_curve(&xmp.replace("<rdf:Description", "<rdf:Description gm:BaseRenditionIsHDR='True'")).is_none(), "made from an HDR picture");
        assert!(adobe_curve(&xmp.replace("gm:HDRCapacityMax = \"2.5\"", "")).is_none(), "no full headroom said");
    }

    #[test]
    fn apples_headroom_is_read_from_its_maker_notes_as_libultrahdr_reads_it() {
        // EXIF: the first list naming the EXIF list, which names the maker notes: Apple's, with
        // notes 33 (1.5) and 48 (0.005), each a fraction.
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        let entry = |tag: u16, kind: u16, value: u32| [&tag.to_be_bytes()[..], &kind.to_be_bytes(), &1u32.to_be_bytes(), &value.to_be_bytes()].concat();
        tiff.extend([&1u16.to_be_bytes()[..], &entry(0x8769, 4, 26), &[0; 4]].concat());
        tiff.extend([&1u16.to_be_bytes()[..], &entry(0x927C, 7, 44), &[0; 4]].concat());
        let notes_at = tiff.len();
        assert_eq!(notes_at, 44);
        tiff.extend(APPLE_NOTES);
        tiff.extend([&2u16.to_be_bytes()[..], &entry(33, 10, 44), &entry(48, 10, 52), &[0; 4]].concat());
        tiff.extend([3i32.to_be_bytes(), 2i32.to_be_bytes(), 5i32.to_be_bytes(), 1000i32.to_be_bytes()].concat());
        let headroom = apple_headroom(&tiff).expect("Apple's headroom");
        assert!((headroom - (-70.0 * 0.005 + 3.0)).abs() < 1e-5, "{headroom}");
        let xmp = "<x:xmpmeta><rdf:Description><HDRGainMap:HDRGainMapVersion>65536</HDRGainMap:HDRGainMapVersion></rdf:Description></x:xmpmeta>";
        let curve = apple_curve(xmp, Some(&tiff)).expect("Apple's gain map");
        assert_eq!((curve.high, curve.full), ([headroom; 3], headroom));
        assert!(apple_curve(xmp, None).is_none(), "no headroom said anywhere");
    }

    /// A segment of `marker` holding `body`.
    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        [&[0xFF, marker][..], &(body.len() as u16 + 2).to_be_bytes(), body].concat()
    }

    /// An Ultra HDR file's two JPEGs made again: of each, the segments `keep` keeps (by its
    /// place, 0 or 1, marker and body), and `added` after its start.
    fn remade(file: &[u8], keep: impl Fn(usize, u8, &[u8]) -> bool, added: [Vec<u8>; 2]) -> Vec<u8> {
        let first = length(file).unwrap();
        let jpegs = [&file[..first], &file[first..]];
        assert!(jpegs[1].starts_with(&[0xFF, 0xD8]), "the gain map right after the picture");
        let made = |at: usize| [&[0xFF, 0xD8][..], &added[at], &kept(jpegs[at], |marker, body| keep(at, marker, body))[2..]].concat();
        [made(0), made(1)].concat()
    }

    /// Writes the test files afresh: an HDR picture encoded by libultrahdr's `ultrahdr_app`
    /// several ways, or its pieces (encoded by libjpeg-turbo's `cjpeg`) put together by it with
    /// metadata given; some then changed here. And libultrahdr's decoding of each on the GPU
    /// (its GLES path, sampling the map as GPUs do; `EGL_PLATFORM=surfaceless` will do).
    /// (`cargo test --lib --features hdr ultra_hdr -- --ignored`, on Linux, with an
    /// `ultrahdr_app` built with `UHDR_ENABLE_GLES`, `UHDR_WRITE_XMP` and `UHDR_WRITE_ISO`.)
    #[test]
    #[ignore = "needs libultrahdr's ultrahdr_app and libjpeg-turbo's cjpeg, and writes the files the other tests read"]
    fn the_test_files_are_made_afresh() {
        let work = std::env::temp_dir().join(format!("taut-ultra-hdr-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |program: &str, arguments: &[&str]| {
            let done = Command::new(program).args(arguments).current_dir(&work).stdin(Stdio::null()).stdout(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "{program} {arguments:?}");
        };
        // Every hue across, from a quarter of SDR white at the top to 8 times it at the bottom.
        let [wide, high] = SIZE;
        let hdr: Vec<[f32; 3]> = (0..wide * high)
            .map(|at| {
                let (x, y) = ((at % wide) as f32 / (wide - 1) as f32, (at / wide) as f32 / (high - 1) as f32);
                let hue = |shift: f32| ((x * 6.0 + shift).rem_euclid(6.0) - 3.0).abs().clamp(1.0, 2.0) - 1.0;
                [hue(0.0), hue(4.0), hue(2.0)].map(|part| (0.15 + 0.85 * part) * (y * 5.0 - 2.0).exp2())
            })
            .collect();
        let halves: Vec<u8> = hdr.iter().flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 1.0]).flat_map(|part| crate::hdr::half(part).to_le_bytes()).collect();
        std::fs::write(work.join("hdr.raw"), halves).unwrap();
        let encode = |more: &[&str]| run("ultrahdr_app", &[&["-m", "0", "-p", "hdr.raw", "-a", "4", "-t", "0", "-C", "0", "-c", "0", "-w", "48", "-h", "32"][..], more].concat());
        encode(&["-M", "0", "-s", "4", "-z", "android.jpg"]);
        encode(&["-M", "1", "-s", "1", "-z", "colours.jpg"]);
        encode(&["-M", "0", "-s", "2", "-G", "2", "-z", "gamma.jpg"]);
        // Pieces of their own: the SDR picture (HDR cut at white), a grey map half as wide, and
        // one of each colour's gain (least, most and gamma as `iso.cfg` says).
        let sdr: Vec<[f32; 3]> = hdr.iter().map(|pixel| pixel.map(|part| part.min(1.0))).collect();
        let netpbm = |kind: &str, width: usize, bytes: Vec<u8>| [format!("{kind}\n{width} {}\n255\n", bytes.len() / width / if kind == "P6" { 3 } else { 1 }).into_bytes(), bytes].concat();
        let srgb = |part: f32| ((if part <= 0.003_130_8 { 12.92 * part } else { 1.055 * part.powf(1.0 / 2.4) - 0.055 }) * 255.0 + 0.5) as u8;
        std::fs::write(work.join("sdr.ppm"), netpbm("P6", wide, sdr.iter().flatten().map(|part| srgb(*part)).collect())).unwrap();
        let stops = |at: usize, part: usize| (hdr[at][part] / sdr[at][part]).log2();
        let grey: Vec<u8> = (0..wide * high / 4).map(|at| ((stops((at / 24) * 2 * wide + at % 24 * 2, 1) + 1.0) / 4.0 * 255.0 + 0.5) as u8).collect();
        std::fs::write(work.join("grey.pgm"), netpbm("P5", wide / 2, grey)).unwrap();
        let ([least, most], gamma) = ([[0.0, -0.415, -1.0], [3.0, 2.0, 1.0]], [1.0, 1.5, 2.0]);
        let coloured: Vec<u8> =
            (0..wide * high).flat_map(|at| (0..3).map(move |part| (((stops(at, part) - least[part]) / (most[part] - least[part])).clamp(0.0, 1.0).powf(gamma[part]) * 255.0 + 0.5) as u8)).collect();
        std::fs::write(work.join("coloured.ppm"), netpbm("P6", wide, coloured)).unwrap();
        run("cjpeg", &["-sample", "1x1", "-quality", "95", "-outfile", "sdr.jpg", "sdr.ppm"]);
        run("cjpeg", &["-grayscale", "-quality", "95", "-outfile", "grey.jpg", "grey.pgm"]);
        run("cjpeg", &["-sample", "1x1", "-quality", "95", "-outfile", "coloured.jpg", "coloured.ppm"]);
        let adobe = "--maxContentBoost 8\n--minContentBoost 0.5\n--gamma 1\n--offsetSdr 0.015625\n--offsetHdr 0.015625\n--hdrCapacityMin 1.5\n--hdrCapacityMax 8\n--useBaseColorSpace 1\n";
        std::fs::write(work.join("adobe.cfg"), adobe).unwrap();
        run("ultrahdr_app", &["-m", "0", "-i", "sdr.jpg", "-g", "grey.jpg", "-f", "adobe.cfg", "-z", "adobe-both.jpg"]);
        run("ultrahdr_app", &["-m", "0", "-i", "sdr.jpg", "-g", "coloured.jpg", "-f", "adobe.cfg", "-z", "coloured-both.jpg"]);
        // ISO 21496-1 for the coloured map (XMP can't say a gain per colour): three colours, the
        // SDR picture's colours, one denominator; headroom from 0 to 3 stops; each colour's
        // least and most gain and gamma as above, offsets 0.02.
        let whole = |number: f32| ((number * 1e6).round() as i32).to_be_bytes();
        let mut iso = vec![0, 0, 0, 0, 0x80 | 0x40 | 8];
        iso.extend([1_000_000u32.to_be_bytes(), whole(0.0), whole(3.0)].concat());
        for part in 0..3 {
            iso.extend([whole(least[part]), whole(most[part]), whole(gamma[part]), whole(0.02), whole(0.02)].concat());
        }
        // Each with one kind of metadata only (Adobe's XMP as libultrahdr 1 wrote it); Apple's in
        // place of the android file's; that file held turned.
        let read = |name: &str| std::fs::read(work.join(name)).unwrap();
        let (is_iso, is_xmp) = (|marker: u8, body: &[u8]| marker == 0xE2 && body.starts_with(ISO), |marker: u8, body: &[u8]| marker == 0xE1 && body.starts_with(XMP));
        let adobe_xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 5.1.2\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" hdrgm:Version=\"1.0\" hdrgm:GainMapMin=\"-1\" hdrgm:GainMapMax=\"3\" hdrgm:Gamma=\"1\" hdrgm:OffsetSDR=\"0.015625\" hdrgm:OffsetHDR=\"0.015625\" hdrgm:HDRCapacityMin=\"0.584963\" hdrgm:HDRCapacityMax=\"3\" hdrgm:BaseRenditionIsHDR=\"False\"/></rdf:RDF></x:xmpmeta>";
        let apple_xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:HDRGainMap=\"http://ns.apple.com/HDRGainMap/1.0/\"><HDRGainMap:HDRGainMapVersion>65536</HDRGainMap:HDRGainMapVersion><HDRGainMap:HDRGainMapHeadroom>2.5</HDRGainMap:HDRGainMapHeadroom></rdf:Description></rdf:RDF></x:xmpmeta>";
        let held = segment(0xE1, &[&b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0"[..]].concat());
        let made = [
            ("android", read("android.jpg")),
            ("colours", read("colours.jpg")),
            ("gamma", read("gamma.jpg")),
            ("adobe", remade(&read("adobe-both.jpg"), |_, marker, body| !is_iso(marker, body) && !is_xmp(marker, body), [Vec::new(), segment(0xE1, &[XMP, &adobe_xmp[..]].concat())])),
            ("iso", remade(&read("coloured-both.jpg"), |_, marker, body| !is_iso(marker, body) && !is_xmp(marker, body), [Vec::new(), segment(0xE2, &[ISO, &iso[..]].concat())])),
            ("apple", remade(&read("android.jpg"), |_, marker, body| !is_iso(marker, body) && !is_xmp(marker, body), [Vec::new(), segment(0xE1, &[XMP, &apple_xmp[..]].concat())])),
            ("turned", remade(&read("android.jpg"), |_, _, _| true, [held, Vec::new()])),
        ];
        for (name, file) in made {
            let at = pictures().join(format!("ultra-hdr-{name}.jpg"));
            std::fs::write(&at, file).unwrap();
            if name != "turned" {
                run("ultrahdr_app", &["-m", "1", "-u", "1", "-j", at.to_str().unwrap(), "-o", "0", "-O", "4", "-z", &format!("{}.linear", at.display())]);
            }
        }
        std::fs::remove_dir_all(work).unwrap();
    }
}
