//! Closed captions in H.264's SEI (ATSC A/53 `cc_data`, `GA94`), found in the access units of
//! a stream whose player hands them over (the NDK's extractor on Android) and read by the
//! CEA-608 decoder.

use crate::video_cea608::Decoder;
use std::time::Duration;

/// Reads the captions in an H.264 access unit's SEI (see `sei_units`) into `decoder`, at time
/// `at`.
pub(crate) fn read_captions(decoder: &mut Decoder, access_unit: &[u8], at: Duration) {
    for triplets in sei_units(access_unit).into_iter().filter_map(cc_data_in_sei) {
        decoder.read_cc_data(&triplets, at);
    }
}

/// The `cc_data` triplets of ATSC A/53 closed captions (`GA94`) in an SEI message of H.264
/// (`user_data_registered_itu_t_t35`): none where `sei` (a NAL unit, header and all, its
/// emulation prevention bytes still in) holds none.
pub(crate) fn cc_data_in_sei(sei: &[u8]) -> Option<Vec<u8>> {
    let raw = unescaped(sei.get(1..)?);
    let (mut at, mut found) = (0, None);
    while at + 2 <= raw.len() && raw[at] != 0x80 {
        let (kind, after) = sei_number(&raw, at)?;
        let (size, after) = sei_number(&raw, after)?;
        let payload = raw.get(after..after + size)?;
        if kind == 4 {
            found = found.or_else(|| cc_data_of_t35(payload));
        }
        at = after + size;
    }
    found
}

/// One of an SEI message's numbers: bytes of 255 added to the byte after them. The number and
/// where what follows starts.
fn sei_number(raw: &[u8], mut at: usize) -> Option<(usize, usize)> {
    let mut number = 0;
    while *raw.get(at)? == 0xFF {
        (number, at) = (number + 255, at + 1);
    }
    Some((number + *raw.get(at)? as usize, at + 1))
}

/// The triplets of an ITU-T T.35 payload that is ATSC's closed captions: country `0xB5`,
/// provider `0x0031`, `GA94`, data type 3; then flags and `cc_count`, a reserved byte, and the
/// triplets.
fn cc_data_of_t35(payload: &[u8]) -> Option<Vec<u8>> {
    let [0xB5, 0x00, 0x31, b'G', b'A', b'9', b'4', 0x03, flags, _, rest @ ..] = payload else { return None };
    let count = (flags & 0x1F) as usize;
    (flags & 0x40 != 0).then(|| rest.get(..count * 3).map(<[u8]>::to_vec)).flatten()
}

/// A NAL unit's bytes without its emulation prevention bytes (the 3 after two zeros).
fn unescaped(escaped: &[u8]) -> Vec<u8> {
    let (mut out, mut zeros) = (Vec::with_capacity(escaped.len()), 0);
    for &byte in escaped {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

/// The SEI NAL units (type 6) of an H.264 access unit, as MP4 keeps it (each NAL unit after its
/// length in 4 bytes) or as a stream carries it (each after a start code, `00 00 01`).
pub(crate) fn sei_units(access_unit: &[u8]) -> Vec<&[u8]> {
    let units = match access_unit.starts_with(&[0, 0, 1]) || access_unit.starts_with(&[0, 0, 0, 1]) {
        true => after_start_codes(access_unit),
        false => after_lengths(access_unit),
    };
    units.into_iter().filter(|unit| unit.first().is_some_and(|header| header & 0x1F == 6)).collect()
}

/// The NAL units of an access unit each after its length in 4 bytes.
fn after_lengths(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    while let Some((length, rest)) = bytes.split_first_chunk::<4>() {
        let length = u32::from_be_bytes(*length) as usize;
        let Some(unit) = rest.get(..length) else { break };
        units.push(unit);
        bytes = &rest[length..];
    }
    units
}

/// The NAL units of an access unit each after a start code.
fn after_start_codes(bytes: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<usize> = (0..bytes.len().saturating_sub(2)).filter(|at| bytes[*at..*at + 3] == [0, 0, 1]).map(|at| at + 3).collect();
    let ends = starts.iter().skip(1).map(|start| start - 3).chain([bytes.len()]);
    starts.iter().zip(ends).map(|(start, end)| bytes[*start..end].strip_suffix(&[0]).unwrap_or(&bytes[*start..end])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::{caption_sei, cea608_pairs};

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    #[test]
    fn the_captions_of_an_mp4_files_sei_are_read_as_written() {
        let cues = [(5, 12, "Hello there"), (18, 26, "Second line")];
        let mut decoder = Decoder::default();
        for (frame, pairs) in cea608_pairs(&cues, 30).iter().enumerate() {
            let sei = caption_sei(pairs);
            read_captions(&mut decoder, &[(sei.len() as u32).to_be_bytes().to_vec(), sei].concat(), at(frame as u64));
        }
        let read: Vec<(Duration, Duration, &str)> = decoder.cues().iter().map(|cue| (cue.from, cue.to, cue.text.as_str())).collect();
        assert_eq!(read, [(at(5), at(12), "Hello there"), (at(18), at(26), "Second line")]);
    }

    #[test]
    fn start_codes_and_lengths_both_find_the_sei() {
        let sei = caption_sei(&[0x94, 0x20, 0x94, 0x20, 0x80, 0x80]);
        let with_lengths = [(sei.len() as u32).to_be_bytes().to_vec(), sei.clone(), vec![0, 0, 0, 2, 0x65, 0x88]].concat();
        let with_start_codes = [vec![0, 0, 0, 1], sei.clone(), vec![0, 0, 1, 0x65, 0x88]].concat();
        assert_eq!(sei_units(&with_lengths), [&sei[..]]);
        assert_eq!(sei_units(&with_start_codes), [&sei[..]]);
    }
}
