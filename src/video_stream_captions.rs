//! The caption tracks of a video stream, read by taut from its `cc_data` (in the SEI of H.264
//! and HEVC, in GStreamer's caption meta) or a QuickTime closed-caption track: CEA-608's caption
//! and text services (`CC1` to `CC4`, `T1` to `T4`) and each CEA-708 service (`Service 1`…),
//! listed in the order their first data came. For players that hand over the data but do not
//! read it (with `video_sei` where they hand over samples).

use crate::video::Track;
use crate::video_captions::{Caption, Cue};
use crate::{video_cea608, video_cea708};
use std::time::Duration;

/// Where a track's cues come from.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// A CEA-608 service, by its place in `video_cea608::NAMES`.
    Cea608(usize),
    Service(u8),
}

/// The caption tracks read from `cc_data` or CEA-608 byte pairs.
#[derive(Default)]
pub(crate) struct ClosedCaptions {
    cea608: video_cea608::Decoder,
    cea708: video_cea708::Decoder,
    /// The tracks found, in order, and how many of them were listed.
    found: Vec<Source>,
    listed: usize,
}

impl ClosedCaptions {
    /// Reads `cc_data`'s triplets, all at time `at`.
    pub(crate) fn read_cc_data(&mut self, triplets: &[u8], at: Duration) {
        self.cea608.read_cc_data(triplets, at);
        self.cea708.read_cc_data(triplets, at);
        self.note_found();
    }

    /// Reads a CEA-608 byte pair of field 1 or 2 (`field` 0 or 1) at time `at`.
    #[cfg(any(test, windows, target_vendor = "apple", all(unix, not(target_os = "android"))))]
    pub(crate) fn read_cea608_pair(&mut self, field: usize, pair: [u8; 2], at: Duration) {
        self.cea608.read_pair(field, pair, at);
        self.note_found();
    }

    /// Starts again at `to` after a seek (see `CueRecorder::sought`): the tracks stay.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.cea608.sought(to);
        self.cea708.sought(to);
    }

    /// Whether tracks were found since `tracks` was last asked.
    pub(crate) fn unlisted(&self) -> bool {
        self.found.len() > self.listed
    }

    /// The tracks found (named, with no language: the stream gives none), now listed.
    pub(crate) fn tracks(&mut self) -> Vec<Track> {
        self.listed = self.found.len();
        let name = |source: &Source| match source {
            Source::Cea608(nth) => video_cea608::NAMES[*nth].to_owned(),
            Source::Service(number) => format!("Service {number}"),
        };
        self.found.iter().map(|source| Track { language: String::new(), name: name(source) }).collect()
    }

    /// The captions the `nth` track shows at `at`.
    pub(crate) fn captions_at(&self, nth: usize, at: Duration) -> Vec<Caption> {
        self.found.get(nth).map_or_else(Vec::new, |source| crate::video_captions::captions_at(self.cues(*source), at))
    }

    /// The cues of the `nth` track found so far.
    #[cfg(test)]
    pub(crate) fn cues_of_track(&self, nth: usize) -> &[Cue] {
        self.found.get(nth).map_or(&[], |source| self.cues(*source))
    }

    fn cues(&self, source: Source) -> &[Cue] {
        match source {
            Source::Cea608(nth) => self.cea608.services().nth(nth).map_or(&[], |(_, cues)| cues),
            Source::Service(number) => self.cea708.services().find(|(found, _)| *found == number).map_or(&[], |(_, cues)| cues),
        }
    }

    /// Adds the tracks whose first data came.
    fn note_found(&mut self) {
        let cea608 = self.cea608.services().enumerate().filter(|(_, (found, _))| *found).map(|(nth, _)| Source::Cea608(nth));
        let cea708 = self.cea708.services().map(|(number, _)| Source::Service(number));
        let new: Vec<Source> = cea608.chain(cea708).filter(|source| !self.found.contains(source)).collect();
        self.found.extend(new);
    }
}

/// The CEA-608 byte pairs in a QuickTime closed-caption sample, in order, each with its field
/// (0 or 1): those of its `cdat` atoms (field 1) and `cdt2` atoms (field 2).
#[cfg(any(test, windows, target_vendor = "apple"))]
pub(crate) fn cea608_pairs(mut sample: &[u8]) -> impl Iterator<Item = (usize, [u8; 2])> {
    let mut pairs = Vec::new();
    while let Some(&[a, b, c, d, ref kind @ ..]) = sample.first_chunk::<8>() {
        let size = (u32::from_be_bytes([a, b, c, d]) as usize).clamp(8, sample.len());
        let field = match kind {
            b"cdat" => Some(0),
            b"cdt2" => Some(1),
            _ => None,
        };
        if let Some(field) = field {
            pairs.extend(sample[8..size].as_chunks::<2>().0.iter().map(|pair| (field, *pair)));
        }
        sample = &sample[size..];
    }
    pairs.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_sei::{Codec, StreamCaptions};

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::{InStream, caption_sei, cc_data, length_prefixed};

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    const CUES: &[(u32, u32, &str)] = &[(5, 12, "Hello there"), (18, 26, "Second line")];
    const SERVICE: &[(u32, u32, &str)] = &[(8, 15, "Service text")];
    const BOTH: InStream = InStream { cea608: CUES, other_channels: &[], services: &[(1, SERVICE)] };

    /// The access units of 30 frames with these captions in their SEI.
    fn units(in_stream: &InStream, codec: Codec) -> Vec<Vec<u8>> {
        cc_data(in_stream, 30).iter().map(|triplets| length_prefixed(caption_sei(triplets, codec == Codec::Hevc))).collect()
    }

    /// Each track's name and cues.
    fn read(tracks: &mut ClosedCaptions) -> Vec<(String, Vec<Spoken>)> {
        let names = tracks.tracks().into_iter().map(|track| track.name);
        names.zip(0..).map(|(name, nth)| (name, tracks.cues_of_track(nth).iter().map(|cue| (cue.from, cue.to, cue.caption.text.clone())).collect())).collect()
    }

    /// A cue's times and text.
    type Spoken = (Duration, Duration, String);

    fn cue(from: u64, to: u64, text: &str) -> Spoken {
        (at(from), at(to), text.into())
    }

    fn expected() -> Vec<(String, Vec<Spoken>)> {
        let cues = |cues: &[(u32, u32, &str)]| cues.iter().map(|(from, to, text)| cue(*from as u64, *to as u64, text)).collect();
        vec![("CC1".into(), cues(CUES)), ("Service 1".into(), cues(SERVICE))]
    }

    #[test]
    fn cea608_and_cea708_in_h264_and_hevc_sei_are_tracks_read_as_written() {
        for codec in [Codec::H264, Codec::Hevc] {
            let mut captions = StreamCaptions::default();
            for (frame, unit) in (0..).zip(units(&BOTH, codec)) {
                captions.fed(&unit, codec, at(frame));
                captions.decoded(at(frame));
            }
            assert_eq!(read(&mut captions.tracks), expected(), "{codec:?}");
        }
    }

    #[test]
    fn captions_fed_in_decode_order_are_read_in_display_order() {
        // (Each I or P frame fed before the two B-frames shown ahead of it.)
        let units = units(&BOTH, Codec::H264);
        let decode_order = std::iter::once(0).chain((1..30).step_by(3).flat_map(|first| [first + 2, first, first + 1])).filter(|frame| *frame < 30);
        let (mut decoded, mut fed) = (StreamCaptions::default(), [false; 30]);
        for frame in decode_order {
            decoded.fed(&units[frame], Codec::H264, at(frame as u64));
            // (The decoder gives each frame once all before it were fed.)
            fed[frame] = true;
            let given = fed.iter().take_while(|fed| **fed).count();
            decoded.decoded(at(given as u64).saturating_sub(at(1)));
        }
        decoded.decoded(Duration::MAX);
        assert_eq!(read(&mut decoded.tracks), expected());
    }

    #[test]
    fn tracks_are_listed_in_the_order_found_and_told_once() {
        let mut captions = StreamCaptions::default();
        let units = units(&InStream { cea608: &[(15, 20, "Late")], other_channels: &[], services: &[(1, SERVICE)] }, Codec::H264);
        for (frame, unit) in (0..).zip(&units) {
            captions.fed(unit, Codec::H264, at(frame));
            captions.decoded(at(frame));
            if frame == 10 {
                assert!(captions.tracks.unlisted());
                assert_eq!(captions.tracks.tracks().iter().map(|track| track.name.as_str()).collect::<Vec<_>>(), ["Service 1"]);
                assert!(!captions.tracks.unlisted());
            }
        }
        assert!(captions.tracks.unlisted());
        assert_eq!(captions.tracks.tracks().iter().map(|track| track.name.as_str()).collect::<Vec<_>>(), ["Service 1", "CC1"]);
        let late: Vec<Caption> = captions.tracks.captions_at(1, at(17));
        assert_eq!(late.iter().map(|caption| caption.text.as_str()).collect::<Vec<_>>(), ["Late"]);
    }

    #[test]
    fn cc2_to_cc4_and_text_services_are_tracks_once_their_data_comes() {
        // (CC3 in field 2, then T2 (channel 2's text restart) and CC2 in field 1.)
        let mut tracks = ClosedCaptions::default();
        let cc3 = files::cea608_pairs_of(&[(1, 5, "Third")], 10, 0x15);
        for (frame, pairs) in (0..).zip(cc3) {
            let triplets: Vec<u8> = pairs.as_chunks::<2>().0.iter().flat_map(|pair| [0xFD, pair[0], pair[1]]).collect();
            tracks.read_cc_data(&triplets, at(frame));
        }
        assert_eq!(tracks.tracks().iter().map(|track| track.name.as_str()).collect::<Vec<_>>(), ["CC3"]);
        for (tenth, pair) in (20..).zip([[0x9C, 0x2A], [0x9C, 0x2A], [0xC8, 0xE9]]) {
            tracks.read_cea608_pair(0, pair, at(tenth));
        }
        for (frame, pairs) in (30..).zip(files::cea608_pairs_of(&[(35, 38, "Second")], 40, 0x1C).into_iter().skip(30)) {
            pairs.as_chunks::<2>().0.iter().for_each(|pair| tracks.read_cea608_pair(0, *pair, at(frame)));
        }
        assert_eq!(tracks.tracks().iter().map(|track| track.name.as_str()).collect::<Vec<_>>(), ["CC3", "T2", "CC2"]);
        let text = |nth: usize, tenth: u64| tracks.captions_at(nth, at(tenth)).into_iter().map(|caption| caption.text).collect::<Vec<_>>();
        assert_eq!((text(0, 3), text(1, 25), text(2, 36)), (vec!["Third".to_owned()], vec!["Hi".to_owned()], vec!["Second".to_owned()]));
    }

    #[test]
    fn after_a_seek_the_tracks_are_read_again_from_there() {
        let units = units(&BOTH, Codec::H264);
        let mut captions = StreamCaptions::default();
        for frames in [0..30, 13..30] {
            if frames.start > 0 {
                captions.sought(at(frames.start as u64));
            }
            // (From the key frame before: every frame here is one.)
            for frame in frames {
                captions.fed(&units[frame], Codec::H264, at(frame as u64));
                captions.decoded(at(frame as u64));
            }
        }
        // (The service's cue on show ends at the seek; CEA-608's next caption is read again.)
        let found = read(&mut captions.tracks);
        assert_eq!(found[0].1, expected()[0].1);
        assert_eq!(found[1].1, [cue(8, 13, "Service text")]);
    }

    #[test]
    fn cea608_pairs_alone_are_the_track_cc1() {
        let mut tracks = ClosedCaptions::default();
        for (frame, pairs) in (0..).zip(files::cea608_pairs(CUES, 30)) {
            pairs.as_chunks::<2>().0.iter().for_each(|pair| tracks.read_cea608_pair(0, *pair, at(frame)));
        }
        assert_eq!(read(&mut tracks), expected()[..1]);
    }
}
