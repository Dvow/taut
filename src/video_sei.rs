//! Closed captions in the SEI of H.264 and HEVC (ATSC A/53 `cc_data`, `GA94`): found in an
//! access unit as MP4 keeps it (each NAL unit after its length) or as a stream carries it (each
//! after a start code), and read in display order into caption tracks (`video_stream_captions`).

use crate::video_stream_captions::ClosedCaptions;
use std::time::Duration;

/// The codec of a video stream whose SEI is read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Codec {
    H264,
    Hevc,
}

impl Codec {
    /// The length of a NAL unit's header.
    fn header_length(self) -> usize {
        match self {
            Codec::H264 => 1,
            Codec::Hevc => 2,
        }
    }
    /// Whether a NAL unit (header first) is SEI: H.264's type 6, HEVC's prefix and suffix
    /// SEI (39, 40).
    fn is_sei(self, unit: &[u8]) -> bool {
        match (self, unit.first()) {
            (Codec::H264, Some(header)) => header & 0x1F == 6,
            (Codec::Hevc, Some(header)) => matches!(header >> 1 & 0x3F, 39 | 40),
            (_, None) => false,
        }
    }
}

/// The `cc_data` triplets in an access unit's SEI messages, in order (empty where none).
pub(crate) fn cc_data_in(access_unit: &[u8], codec: Codec) -> Vec<u8> {
    let units = after_lengths(access_unit).unwrap_or_else(|| after_start_codes(access_unit));
    let seis = units.into_iter().filter(|unit| codec.is_sei(unit));
    seis.filter_map(|sei| cc_data_in_sei(sei, codec)).flatten().collect()
}

/// The `cc_data` triplets of ATSC A/53 closed captions (`GA94`) in an SEI message
/// (`user_data_registered_itu_t_t35`): none where `sei` (a NAL unit, header and all, its
/// emulation prevention bytes still in) holds none.
fn cc_data_in_sei(sei: &[u8], codec: Codec) -> Option<Vec<u8>> {
    let raw = unescaped(sei.get(codec.header_length()..)?);
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

/// The NAL units of an access unit each after its length in 4 bytes: none unless the lengths
/// span it exactly. (A length of 1, or of 256 to 511, begins as a start code does.)
fn after_lengths(mut bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let mut units = Vec::new();
    while !bytes.is_empty() {
        let (length, rest) = bytes.split_first_chunk::<4>()?;
        let (unit, after) = rest.split_at_checked(u32::from_be_bytes(*length) as usize)?;
        units.push(unit);
        bytes = after;
    }
    Some(units)
}

/// The NAL units of an access unit each after a start code.
fn after_start_codes(bytes: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<usize> = (0..bytes.len().saturating_sub(2)).filter(|at| bytes[*at..*at + 3] == [0, 0, 1]).map(|at| at + 3).collect();
    let ends = starts.iter().skip(1).map(|start| start - 3).chain([bytes.len()]);
    starts.iter().zip(ends).map(|(start, end)| bytes[*start..end].strip_suffix(&[0]).unwrap_or(&bytes[*start..end])).collect()
}

/// The captions of a stream's access units, taken in decode order and read in display order,
/// which B-frames make differ: CEA-608's pairs and CEA-708's packets mean something only in
/// display order. Each unit's captions wait until no unit before them in display order can
/// still come.
#[derive(Default)]
pub(crate) struct StreamCaptions {
    waiting: Vec<(Duration, Vec<u8>)>,
    pub(crate) tracks: ClosedCaptions,
}

impl StreamCaptions {
    /// Takes in the captions in an access unit of `codec` shown at `at`, as a decoder is fed it.
    pub(crate) fn fed(&mut self, access_unit: &[u8], codec: Codec, at: Duration) {
        let triplets = cc_data_in(access_unit, codec);
        if !triplets.is_empty() {
            let place = self.waiting.partition_point(|(time, _)| *time <= at);
            self.waiting.insert(place, (at, triplets));
        }
    }

    /// The decoder gave its frame at `at` (or `Duration::MAX` at the end): the captions up to
    /// then are read.
    pub(crate) fn decoded(&mut self, at: Duration) {
        let ready = self.waiting.partition_point(|(time, _)| *time <= at);
        for (time, triplets) in self.waiting.drain(..ready) {
            self.tracks.read_cc_data(&triplets, time);
        }
    }

    /// Starts again at `to` after a seek; what waited is dropped.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.waiting.clear();
        self.tracks.sought(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::{caption_sei, length_prefixed};

    const TRIPLETS: [u8; 9] = [0xFC, 0x94, 0x20, 0xFC, 0x94, 0x20, 0xFF, 0x02, 0x21];

    #[test]
    fn start_codes_and_lengths_both_find_the_sei() {
        let sei = caption_sei(&TRIPLETS, false);
        let with_lengths = [length_prefixed(sei.clone()), vec![0, 0, 0, 2, 0x65, 0x88]].concat();
        let with_start_codes = [vec![0, 0, 0, 1], sei, vec![0, 0, 1, 0x65, 0x88]].concat();
        assert_eq!(cc_data_in(&with_lengths, Codec::H264), TRIPLETS);
        assert_eq!(cc_data_in(&with_start_codes, Codec::H264), TRIPLETS);
    }

    #[test]
    fn a_first_unit_whose_length_begins_as_a_start_code_is_read_by_its_length() {
        // (A slice of 300 bytes: its length is 00 00 01 2C.)
        let slice = [vec![0x01], vec![0x11; 299]].concat();
        let access_unit = [length_prefixed(slice), length_prefixed(caption_sei(&TRIPLETS, false))].concat();
        assert_eq!(cc_data_in(&access_unit, Codec::H264), TRIPLETS);
    }

    #[test]
    fn hevc_prefix_and_suffix_sei_are_read_and_h264_sei_is_not_taken_for_hevc() {
        let prefix = caption_sei(&TRIPLETS, true);
        let mut suffix = prefix.clone();
        suffix[0] = 40 << 1;
        let access_unit = [length_prefixed(prefix), length_prefixed(vec![0x26, 0x01, 0xAF]), length_prefixed(suffix)].concat();
        assert_eq!(cc_data_in(&access_unit, Codec::Hevc), [TRIPLETS, TRIPLETS].concat());
        // (H.264's SEI header, 0x06, is HEVC's type 3: no SEI.)
        assert!(cc_data_in(&length_prefixed(caption_sei(&TRIPLETS, false)), Codec::Hevc).is_empty());
    }

    #[test]
    fn sei_with_other_payloads_and_other_providers_holds_no_captions() {
        let other_provider = [4, 13, 0xB5, 0x00, 0x2F, b'G', b'A', b'9', b'4', 0x03, 0x41, 0xFF, 0xFC, 0x94, 0x80];
        let unregistered = [5, 2, 0xAA, 0xBB];
        let sei = files::nal(0x06, &[&unregistered[..], &other_provider[..], &[0x80]].concat());
        assert!(cc_data_in(&length_prefixed(sei), Codec::H264).is_empty());
    }
}
