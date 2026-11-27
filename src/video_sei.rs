//! Closed captions in H.264's SEI (ATSC A/53 `cc_data`, `GA94`), found in the access units of
//! a stream whose player hands them over (the NDK's extractor on Android) and read by the
//! CEA-608 decoder.

use crate::video_captions::Cue;
use crate::video_cea608::Decoder;
use std::time::Duration;

/// The captions of a stream's access units, taken as the decoder is fed them (decode order) and
/// read in display order, which B-frames make differ: CEA-608's pairs mean something only in
/// display order. Each unit's captions wait until the decoder gives a frame at or after its
/// time.
#[derive(Default)]
pub(crate) struct Captions {
    waiting: Vec<(Duration, Vec<u8>)>,
    decoder: Decoder,
    /// The first cue changed since `changes` was last asked.
    changed_from: Option<usize>,
}

impl Captions {
    /// Takes in the captions in an access unit's SEI (see `sei_units`), shown at `at`.
    pub(crate) fn fed(&mut self, access_unit: &[u8], at: Duration) {
        for triplets in sei_units(access_unit).into_iter().filter_map(cc_data_in_sei) {
            let place = self.waiting.partition_point(|(time, _)| *time <= at);
            self.waiting.insert(place, (at, triplets));
        }
    }

    /// The decoder gave its frame at `at` (or `Duration::MAX` at the end): the captions up to
    /// then are read.
    pub(crate) fn decoded(&mut self, at: Duration) {
        let ready = self.waiting.partition_point(|(time, _)| *time <= at);
        if ready == 0 {
            return;
        }
        let (found, count, last) = (self.decoder.found(), self.decoder.cues().len(), self.decoder.cues().last().cloned());
        for (time, triplets) in self.waiting.drain(..ready) {
            self.decoder.read_cc_data(&triplets, time);
        }
        // (Only the last cue changes, besides cues added or dropped after it. The first data
        // found counts as a change: the captions are a track from then.)
        let changed = self.decoder.cues().len() != count || self.decoder.cues().last() != last.as_ref();
        if changed || found != self.decoder.found() {
            self.changed(count.saturating_sub(1));
        }
    }

    /// Starts again at `to` after a seek (see `Decoder::sought`); what waited is dropped.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.waiting.clear();
        self.decoder.sought(to);
        self.changed(self.decoder.cues().len().saturating_sub(1));
    }

    /// The cues changed since last asked, once caption data was found: from which one on, and
    /// those from there (any after them are gone).
    pub(crate) fn changes(&mut self) -> Option<(usize, &[Cue])> {
        if !self.decoder.found() {
            return None;
        }
        let cues = self.decoder.cues();
        let from = self.changed_from.take()?.min(cues.len());
        Some((from, &cues[from..]))
    }

    /// Marks the cues from the `from`th on as changed.
    fn changed(&mut self, from: usize) {
        self.changed_from = Some(self.changed_from.map_or(from, |was| was.min(from)));
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
/// length in 4 bytes) or as a stream carries it (each after a start code, `00 00 01`). Lengths
/// are taken where they span the access unit exactly: a length of 1, or of 256 to 511, begins
/// as a start code does.
pub(crate) fn sei_units(access_unit: &[u8]) -> Vec<&[u8]> {
    let units = after_lengths(access_unit).unwrap_or_else(|| after_start_codes(access_unit));
    units.into_iter().filter(|unit| unit.first().is_some_and(|header| header & 0x1F == 6)).collect()
}

/// The NAL units of an access unit each after its length in 4 bytes: none unless the lengths
/// span it exactly.
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

    fn length_prefixed(unit: Vec<u8>) -> Vec<u8> {
        [(unit.len() as u32).to_be_bytes().to_vec(), unit].concat()
    }

    fn read(captions: &Captions) -> Vec<(Duration, Duration, &str)> {
        captions.decoder.cues().iter().map(|cue| (cue.from, cue.to, cue.text.as_str())).collect()
    }

    #[test]
    fn the_captions_of_an_mp4_files_sei_are_read_as_written() {
        let cues = [(5, 12, "Hello there"), (18, 26, "Second line")];
        let mut captions = Captions::default();
        for (frame, pairs) in cea608_pairs(&cues, 30).iter().enumerate() {
            captions.fed(&length_prefixed(caption_sei(pairs)), at(frame as u64));
            captions.decoded(at(frame as u64));
        }
        assert_eq!(read(&captions), [(at(5), at(12), "Hello there"), (at(18), at(26), "Second line")]);
    }

    #[test]
    fn captions_fed_in_decode_order_are_read_in_display_order() {
        // (Each I or P frame fed before the two B-frames shown ahead of it, as with B-frames.)
        let cues = [(5, 12, "Hello there"), (18, 26, "Second line")];
        let pairs = cea608_pairs(&cues, 30);
        let decode_order = std::iter::once(0).chain((1..30).step_by(3).flat_map(|first| [first + 2, first, first + 1])).filter(|frame| *frame < 30);
        let (mut captions, mut fed) = (Captions::default(), [false; 30]);
        for frame in decode_order {
            captions.fed(&length_prefixed(caption_sei(&pairs[frame])), at(frame as u64));
            // (The decoder gives each frame once all before it were fed.)
            fed[frame] = true;
            let given = fed.iter().take_while(|fed| **fed).count();
            captions.decoded(at(given as u64).saturating_sub(at(1)));
        }
        captions.decoded(Duration::MAX);
        assert_eq!(read(&captions), [(at(5), at(12), "Hello there"), (at(18), at(26), "Second line")]);
    }

    #[test]
    fn the_changes_handed_on_keep_a_copy_of_the_cues_through_a_seek() {
        let pairs = cea608_pairs(&[(5, 12, "Hello there"), (18, 26, "Second line")], 30);
        let (mut captions, mut copy) = (Captions::default(), Vec::new());
        let mut take_changes = |captions: &mut Captions| {
            if let Some((from, changed)) = captions.changes() {
                assert!(changed.len() <= 2, "{changed:?} handed on from {from}");
                copy.truncate(from);
                copy.extend_from_slice(changed);
            }
            assert_eq!(copy, captions.decoder.cues());
        };
        for frames in [0..30, 15..30] {
            if frames.start > 0 {
                captions.sought(at(frames.start as u64));
            }
            for frame in frames {
                captions.fed(&length_prefixed(caption_sei(&pairs[frame])), at(frame as u64));
                captions.decoded(at(frame as u64));
                take_changes(&mut captions);
            }
        }
        assert_eq!(read(&captions), [(at(5), at(12), "Hello there"), (at(18), at(26), "Second line")]);
    }

    #[test]
    fn start_codes_and_lengths_both_find_the_sei() {
        let sei = caption_sei(&[0x94, 0x20, 0x94, 0x20, 0x80, 0x80]);
        let with_lengths = [length_prefixed(sei.clone()), vec![0, 0, 0, 2, 0x65, 0x88]].concat();
        let with_start_codes = [vec![0, 0, 0, 1], sei.clone(), vec![0, 0, 1, 0x65, 0x88]].concat();
        assert_eq!(sei_units(&with_lengths), [&sei[..]]);
        assert_eq!(sei_units(&with_start_codes), [&sei[..]]);
    }

    #[test]
    fn a_first_unit_whose_length_begins_as_a_start_code_is_read_by_its_length() {
        // (A slice of 300 bytes: its length is 00 00 01 2C.)
        let (slice, sei) = ([vec![0x01], vec![0x11; 299]].concat(), caption_sei(&[0x94, 0x20, 0x94, 0x20, 0x80, 0x80]));
        let access_unit = [length_prefixed(slice), length_prefixed(sei.clone())].concat();
        assert_eq!(sei_units(&access_unit), [&sei[..]]);
    }
}
