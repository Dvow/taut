//! The caption tracks of MP4 and QuickTime files, read by taut for players that read none of
//! them (Media Foundation): 3GPP timed text (`tx3g`, which ffmpeg calls `mov_text`) and
//! QuickTime text, WebVTT (`wvtt`), and closed captions (`c608`: each CEA-608 service a track).
//! Plain and fragmented files are read, a sample at a time in time order, as a reading thread
//! paced by the video asks (`video_read_ahead`): a file at an address is not read ahead of the
//! player.

use crate::video::Track;
use crate::video_captions::{Caption, Cue, Place};
use crate::video_read_ahead::{ReadAhead, Reading, Step};
use crate::video_stream_captions::ClosedCaptions;
use std::time::Duration;

/// Where a file's bytes come from: a file, bytes, a download.
pub(crate) trait Source: Send {
    /// Reads bytes from `at` into `into`: how many (fewer at the end, 0 past it or on failure).
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize;
}

impl Source for std::fs::File {
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
        use std::io::{Read, Seek, SeekFrom};
        let mut read = 0;
        if self.seek(SeekFrom::Start(at)).is_err() {
            return 0;
        }
        while read < into.len() {
            match self.read(&mut into[read..]) {
                Ok(0) | Err(_) => break,
                Ok(more) => read += more,
            }
        }
        read
    }
}

impl Source for std::sync::Arc<[u8]> {
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
        let Some(rest) = usize::try_from(at).ok().and_then(|at| self.get(at..)) else { return 0 };
        let read = rest.len().min(into.len());
        into[..read].copy_from_slice(&rest[..read]);
        read
    }
}

/// Reads a file's caption tracks into `captions` as the video plays, following its seeks, until
/// the player goes: nothing where it is no MP4 or QuickTime file or has none.
pub(crate) fn read_paced(source: impl Source, captions: &ReadAhead<FileCaptions>) {
    let Some(mut reader) = Reader::open(source) else { return };
    captions.with(|file| file.found(reader.tracks()));
    let mut read_to = Duration::ZERO;
    loop {
        read_to = match captions.next(read_to) {
            Step::Stop => return,
            Step::Seek(to) => {
                reader.seek(to);
                to
            }
            Step::Read => read_sample(&mut reader, captions),
        };
    }
}

/// Reads the next sample into `captions`: its time; at the end, `Duration::MAX`, all read.
fn read_sample<S: Source>(reader: &mut Reader<S>, captions: &ReadAhead<FileCaptions>) -> Duration {
    let Some(sample) = reader.read_next() else {
        captions.ended();
        return Duration::MAX;
    };
    let at = sample.from;
    captions.took(at, |file| file.take(sample));
    at
}

/// The most of a box taut reads into memory: a `moov` or `moof`, or a caption sample.
const MOST: u64 = 64 << 20;
/// The most caption tracks read, and samples kept: of all tracks in a movie box, or a track's
/// from its fragments.
const MOST_TRACKS: usize = 64;
const MOST_SAMPLES: usize = 1 << 20;
/// How far past the next sample fragments are read for a track with none pending (it may have
/// ended, or come in fragments of its own).
const FRAGMENTS_AHEAD: Duration = Duration::from_secs(30);

/// What a caption track holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// 3GPP or QuickTime timed text, at this place (none: the usual).
    TimedText(Option<Place>),
    WebVtt,
    Cea608,
}

/// A caption track: what it holds, its id, its time scale, how its times move on the movie's
/// (edits), and its samples known so far and the next to read.
struct CaptionTrack {
    kind: Kind,
    track: Track,
    id: u32,
    timescale: u64,
    /// Nanoseconds added to each sample's time (an edit list's empty edits less its first
    /// edit's start).
    shift: i128,
    samples: Vec<Sample>,
    next: usize,
}

/// A sample read: what its track holds (the `text`th text track, where it is one), its bytes,
/// and when it shows.
pub(crate) struct ReadSample {
    kind: Kind,
    text: usize,
    bytes: Vec<u8>,
    pub(crate) from: Duration,
    to: Duration,
}

/// A sample: its time and duration (in its track's time scale), where it is and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    time: u64,
    duration: u32,
    at: u64,
    size: u32,
}

impl Sample {
    /// When it ends, in its track's time scale.
    fn end(&self) -> u64 {
        self.time.saturating_add(u64::from(self.duration))
    }
}

impl CaptionTrack {
    /// A time of this track as the movie's.
    fn movie_time(&self, time: u64) -> Duration {
        let nanos = i128::from(time) * 1_000_000_000 / i128::from(self.timescale.max(1)) + self.shift;
        Duration::from_nanos(u64::try_from(nanos.max(0)).unwrap_or(u64::MAX))
    }

    /// The first of its known samples that ends after `to` (none: past the last).
    fn first_ending_after(&self, to: Duration) -> usize {
        self.samples.iter().position(|sample| self.movie_time(sample.end()) > to).unwrap_or(self.samples.len())
    }

    /// When its next sample shows, where one is known.
    fn pending(&self) -> Option<Duration> {
        self.samples.get(self.next).map(|sample| self.movie_time(sample.time))
    }

    /// When its last known sample ends.
    fn known_end(&self) -> Option<Duration> {
        self.samples.last().map(|sample| self.movie_time(sample.end()))
    }
}

/// Reads a file's caption tracks.
pub(crate) struct Reader<S: Source> {
    source: S,
    tracks: Vec<CaptionTrack>,
    /// For a fragmented file: each track's sample duration and size where its fragments give
    /// none (`trex`), by track id; where its fragments start, and the next box to look at.
    defaults: Vec<(u32, (u32, u32))>,
    fragments: Option<(u64, u64)>,
}

impl<S: Source> Reader<S> {
    /// Reads a file's movie box: none where it is no MP4 or QuickTime file, or has no caption
    /// tracks.
    pub(crate) fn open(mut source: S) -> Option<Reader<S>> {
        let mut at = 0;
        let mut first = true;
        let (moov, after) = loop {
            let (kind, body, end) = atom_at(&mut source, at)?;
            if first && !matches!(&kind, b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot") {
                return None;
            }
            first = false;
            if &kind == b"moov" {
                break (read_body(&mut source, body, end)?, end);
            }
            at = end;
        };
        let tracks = caption_tracks(&moov);
        if tracks.is_empty() {
            return None;
        }
        let defaults = child(&moov, b"mvex").map(track_defaults);
        Some(Reader { source, tracks, fragments: defaults.is_some().then_some((after, after)), defaults: defaults.unwrap_or_default() })
    }

    /// The caption tracks (3GPP timed text and WebVTT first, then closed captions).
    pub(crate) fn tracks(&self) -> impl Iterator<Item = (&Track, bool)> {
        self.tracks.iter().map(|track| (&track.track, track.kind == Kind::Cea608))
    }

    /// Reads from `to` on: each track from its first sample that ends after it (in a fragmented
    /// file, read again from the first fragment).
    pub(crate) fn seek(&mut self, to: Duration) {
        if let Some((first, _)) = self.fragments {
            self.fragments = Some((first, first));
            self.tracks.iter_mut().for_each(|track| track.samples.clear());
        }
        loop {
            self.tracks.iter_mut().for_each(|track| track.next = track.first_ending_after(to));
            if !(self.wants_fragment() && self.read_fragment()) {
                return;
            }
        }
    }

    /// Reads the next sample of all tracks, in time order; none after the last.
    pub(crate) fn read_next(&mut self) -> Option<ReadSample> {
        let (nth, sample, from) = self.next_sample()?;
        let bytes = read_body(&mut self.source, sample.at, sample.at.saturating_add(u64::from(sample.size))).unwrap_or_default();
        let track = &self.tracks[nth];
        let to = track.movie_time(sample.end());
        let text = self.tracks[..nth].iter().filter(|track| track.kind != Kind::Cea608).count();
        Some(ReadSample { kind: track.kind, text, bytes, from, to })
    }

    /// The next sample of all tracks, by time: its track, the sample and its time, read on.
    fn next_sample(&mut self) -> Option<(usize, Sample, Duration)> {
        while self.wants_fragment() && self.read_fragment() {}
        let pending = self.tracks.iter().enumerate().filter_map(|(nth, track)| track.samples.get(track.next).map(|sample| (nth, *sample, track.movie_time(sample.time))));
        let (nth, sample, time) = pending.min_by_key(|(_, _, time)| *time)?;
        self.tracks[nth].next += 1;
        Some((nth, sample, time))
    }

    /// Whether to read the next fragment: while a track has read all it knows, unless the
    /// samples known run `FRAGMENTS_AHEAD` past the next one (a track that ended is not waited
    /// for to the file's end).
    fn wants_fragment(&self) -> bool {
        let pending = self.tracks.iter().filter_map(CaptionTrack::pending).min();
        let known = self.tracks.iter().filter_map(CaptionTrack::known_end).max();
        let ahead = pending.zip(known).is_some_and(|(pending, known)| known > pending.saturating_add(FRAGMENTS_AHEAD));
        self.tracks.iter().any(|track| track.next >= track.samples.len()) && !ahead
    }

    /// Reads the next fragment's samples of the caption tracks, those read before dropped:
    /// whether there was one.
    fn read_fragment(&mut self) -> bool {
        let Some((first, mut at)) = self.fragments else { return false };
        while let Some((kind, body, end)) = atom_at(&mut self.source, at) {
            let start = std::mem::replace(&mut at, end);
            if &kind != b"moof" {
                continue;
            }
            let Some(moof) = read_body(&mut self.source, body, end) else { break };
            self.fragments = Some((first, at));
            for track in &mut self.tracks {
                // (The last sample read stays: a fragment without a decode time goes on from it.)
                track.samples.drain(..track.next.saturating_sub(1));
                track.next = track.next.min(1);
            }
            // (The first track fragment's data counts from the fragment, each next one's from
            // where the one before's ends, unless they say otherwise.)
            let mut implicit = start;
            for traf in children(&moof, b"traf") {
                implicit = self.take_fragment(traf, start, implicit);
            }
            return true;
        }
        self.fragments = Some((first, u64::MAX));
        false
    }

    /// Takes in a track fragment's samples where its track is a caption track, of a fragment
    /// whose box starts at `moof`, its data counted from `implicit` where it gives no base:
    /// where its data ends.
    fn take_fragment(&mut self, traf: &[u8], moof: u64, implicit: u64) -> u64 {
        let Some(header) = child(traf, b"tfhd") else { return implicit };
        let (Some(flags), Some(id)) = (be32(header, 0).map(|flags| flags & 0xFF_FFFF), be32(header, 4)) else { return implicit };
        let defaults = self.defaults.iter().find(|(of, _)| *of == id).map_or((0, 0), |(_, defaults)| *defaults);
        let mut field = 8;
        let mut take = |size: usize| {
            let value = if size == 8 { be64(header, field) } else { be32(header, field).map(u64::from) };
            field += size;
            value
        };
        let base = if flags & 1 != 0 { take(8) } else { None };
        if flags & 2 != 0 {
            take(4);
        }
        let duration = if flags & 8 != 0 { take(4).map(|value| value as u32) } else { None }.unwrap_or(defaults.0);
        let size = if flags & 0x10 != 0 { take(4).map(|value| value as u32) } else { None }.unwrap_or(defaults.1);
        // (Its base: given, the fragment's start (`default-base-is-moof`), or as above.)
        let base = base.unwrap_or(if flags & 0x2_0000 != 0 { moof } else { implicit });
        let track = self.tracks.iter_mut().find(|track| track.id == id);
        let mut time = child(traf, b"tfdt").and_then(decode_time).or_else(|| track.as_ref()?.samples.last().map(Sample::end)).unwrap_or(0);
        let (mut at, mut samples) = (base, Vec::new());
        // (A run's data is at its offset from the base, or right after the run before.)
        for run in children(traf, b"trun") {
            let (offset, sizes) = run_samples(run, (duration, size), MOST_SAMPLES - samples.len());
            at = offset.map_or(at, |offset| base.checked_add_signed(offset).unwrap_or(base));
            for (duration, size) in sizes {
                samples.push(Sample { time, duration, at, size });
                (time, at) = (time.saturating_add(u64::from(duration)), at.saturating_add(u64::from(size)));
            }
        }
        if let Some(track) = track {
            let room = MOST_SAMPLES.saturating_sub(track.samples.len());
            track.samples.extend(samples.into_iter().take(room));
        }
        at
    }
}

/// A track run's data offset (from the fragment's base), where it gives one, and its samples'
/// durations and sizes, `most` at most.
fn run_samples(run: &[u8], (duration, size): (u32, u32), most: usize) -> (Option<i64>, Vec<(u32, u32)>) {
    let (Some(flags), Some(count)) = (be32(run, 0), be32(run, 4)) else { return (None, Vec::new()) };
    let offset = if flags & 1 != 0 { be32(run, 8).map(|offset| i64::from(offset as i32)) } else { None };
    let at = 8 + if flags & 1 != 0 { 4 } else { 0 } + if flags & 4 != 0 { 4 } else { 0 };
    let fields = [0x100, 0x200, 0x400, 0x800].iter().filter(|bit| flags & **bit != 0).count();
    // (No more samples than the run has room for, or than a sane run holds.)
    let room = if fields == 0 { 1 << 16 } else { run.len().saturating_sub(at) / (fields * 4) };
    let samples = (0..(count as usize).min(room).min(most)).map(|nth| {
        let field = |order: usize| be32(run, at + (nth * fields + order) * 4);
        let duration = if flags & 0x100 != 0 { field(0).unwrap_or(duration) } else { duration };
        let size = if flags & 0x200 != 0 { field(usize::from(flags & 0x100 != 0)).unwrap_or(size) } else { size };
        (duration, size)
    });
    (offset, samples.collect())
}

/// A track fragment's decode time (`tfdt`, version 0 or 1).
fn decode_time(tfdt: &[u8]) -> Option<u64> {
    match tfdt.first()? {
        1 => be64(tfdt, 4),
        _ => be32(tfdt, 4).map(u64::from),
    }
}

/// The caption tracks of a movie box, text tracks first (`MOST_TRACKS` at most, with
/// `MOST_SAMPLES` among them).
fn caption_tracks(moov: &[u8]) -> Vec<CaptionTrack> {
    let movie_scale = child(moov, b"mvhd").and_then(|mvhd| be32(mvhd, if mvhd.first() == Some(&1) { 20 } else { 12 })).unwrap_or(1000);
    let mut tracks: Vec<CaptionTrack> = Vec::new();
    for trak in children(moov, b"trak") {
        let room = MOST_SAMPLES - tracks.iter().map(|track| track.samples.len()).sum::<usize>();
        tracks.extend(caption_track(trak, u64::from(movie_scale), room));
        if tracks.len() == MOST_TRACKS {
            break;
        }
    }
    tracks.sort_by_key(|track| track.kind == Kind::Cea608);
    tracks
}

/// Each track's sample duration and size for its fragments (`trex`), by track id.
fn track_defaults(mvex: &[u8]) -> Vec<(u32, (u32, u32))> {
    children(mvex, b"trex").filter_map(|trex| Some((be32(trex, 4)?, (be32(trex, 12)?, be32(trex, 16)?)))).collect()
}

/// A track, where it holds captions taut reads (`most` samples at most).
fn caption_track(trak: &[u8], movie_scale: u64, most: usize) -> Option<CaptionTrack> {
    let mdia = child(trak, b"mdia")?;
    let handler = child(mdia, b"hdlr").and_then(|hdlr| hdlr.get(8..12))?;
    let stbl = path(mdia, &[b"minf", b"stbl"])?;
    let stsd = child(stbl, b"stsd")?;
    let (entry, body) = atoms(stsd.get(8..)?).next()?;
    let kind = match (handler, &entry) {
        (b"sbtl" | b"text" | b"subt", b"tx3g") => Kind::TimedText(timed_text_place(body)),
        (b"sbtl" | b"text" | b"subt", b"text") => Kind::TimedText(None),
        (b"sbtl" | b"text" | b"subt", b"wvtt") => Kind::WebVtt,
        (b"clcp", b"c608") => Kind::Cea608,
        _ => return None,
    };
    let mdhd = child(mdia, b"mdhd")?;
    let version_1 = mdhd.first() == Some(&1);
    let timescale = u64::from(be32(mdhd, if version_1 { 20 } else { 12 })?);
    let packed = mdhd.get(if version_1 { 32 } else { 20 }..).and_then(|bytes| bytes.first_chunk::<2>()).map_or(0, |two| u16::from_be_bytes(*two));
    let language = child(mdia, b"elng").map(|elng| text_of(elng.get(4..).unwrap_or_default())).unwrap_or_else(|| packed_language(packed));
    let name = path(trak, &[b"udta", b"name"]).map(text_of).unwrap_or_default();
    let id = child(trak, b"tkhd").and_then(|tkhd| be32(tkhd, if tkhd.first() == Some(&1) { 20 } else { 12 }))?;
    let shift = child(trak, b"edts").and_then(|edts| child(edts, b"elst")).map_or(0, |elst| edit_shift(elst, movie_scale, timescale));
    let samples = sample_table(stbl, most);
    Some(CaptionTrack { kind, track: Track { language, name }, id, timescale, shift, samples, next: 0 })
}

/// A 3GPP timed text sample entry's place: its justification across (0 left, 1 centre, -1
/// right) and down (0 top, 1 middle, -1 bottom), after its reserved bytes, data reference and
/// display flags; none at the usual (centred at the bottom).
fn timed_text_place(entry: &[u8]) -> Option<Place> {
    let [across, down] = entry.get(12..14)?.try_into().ok()?;
    let column = match across as i8 {
        0 => 0,
        -1 => 2,
        _ => 1,
    };
    let row = match down as i8 {
        0 => 2,
        1 => 1,
        _ => 0,
    };
    ((column, row) != (1, 0)).then(|| Place::keypad(row * 3 + column + 1, 0.05, 0.05, 0.05))
}

/// The nanoseconds an edit list moves a track's times by: its empty edits (in the movie's
/// time scale) less where its first other edit starts in the track (in its own).
fn edit_shift(elst: &[u8], movie_scale: u64, timescale: u64) -> i128 {
    let version_1 = elst.first() == Some(&1);
    let (size, count) = (if version_1 { 20 } else { 12 }, be32(elst, 4).unwrap_or(0) as usize);
    let mut empty = 0i128;
    for nth in 0..count.min(elst.len() / size) {
        let at = 8 + nth * size;
        let (duration, start) = match version_1 {
            true => (be64(elst, at).map(i128::from), be64(elst, at + 8).map(|start| i128::from(start as i64))),
            false => (be32(elst, at).map(i128::from), be32(elst, at + 4).map(|start| i128::from(start as i32))),
        };
        let (Some(duration), Some(start)) = (duration, start) else { break };
        if start >= 0 {
            return empty - start * 1_000_000_000 / i128::from(timescale.max(1));
        }
        empty += duration * 1_000_000_000 / i128::from(movie_scale.max(1));
    }
    empty
}

/// A sample table's samples (`most` at most): times from `stts`, sizes from `stsz` (or
/// `stz2`), places from `stsc` and `stco` (or `co64`).
fn sample_table(stbl: &[u8], most: usize) -> Vec<Sample> {
    let durations = child(stbl, b"stts")
        .map_or_else(Vec::new, |stts| entries(stts, 8).flat_map(|entry| std::iter::repeat_n(be32(entry, 4).unwrap_or(0), be32(entry, 0).unwrap_or(0) as usize)).take(most).collect());
    let sizes = sample_sizes(stbl, durations.len());
    let chunks: Vec<u64> = match (child(stbl, b"stco"), child(stbl, b"co64")) {
        (Some(stco), _) => entries(stco, 4).filter_map(|entry| be32(entry, 0).map(u64::from)).collect(),
        (None, Some(co64)) => entries(co64, 8).filter_map(|entry| be64(entry, 0)).collect(),
        (None, None) => Vec::new(),
    };
    let runs: Vec<(usize, usize)> = child(stbl, b"stsc").map_or_else(Vec::new, |stsc| entries(stsc, 12).filter_map(|entry| Some((be32(entry, 0)? as usize, be32(entry, 4)? as usize))).collect());
    let (mut samples, mut time, mut nth, mut run) = (Vec::with_capacity(sizes.len()), 0u64, 0, 0);
    for (chunk, &at) in (1..).zip(&chunks) {
        // (Each `stsc` entry gives the samples per chunk from its first chunk on, counted from 1.)
        while runs.get(run + 1).is_some_and(|(first, _)| *first <= chunk) {
            run += 1;
        }
        let per_chunk = runs.get(run).filter(|(first, _)| *first <= chunk).map_or(0, |(_, count)| *count);
        let mut offset = at;
        for _ in 0..per_chunk {
            let (Some(&size), Some(&duration)) = (sizes.get(nth), durations.get(nth)) else { return samples };
            samples.push(Sample { time, duration, at: offset, size });
            (time, offset, nth) = (time + u64::from(duration), offset.saturating_add(u64::from(size)), nth + 1);
        }
    }
    samples
}

/// A sample table's sizes, `count` at most: `stsz`'s (one size for all, or each its own) or
/// `stz2`'s (4, 8 or 16 bits each).
fn sample_sizes(stbl: &[u8], count: usize) -> Vec<u32> {
    if let Some(stsz) = child(stbl, b"stsz") {
        let (all, listed) = (be32(stsz, 4).unwrap_or(0), be32(stsz, 8).unwrap_or(0) as usize);
        return match all {
            0 => stsz.get(12..).unwrap_or_default().as_chunks::<4>().0.iter().take(listed.min(count)).map(|size| u32::from_be_bytes(*size)).collect(),
            all => vec![all; listed.min(count)],
        };
    }
    let Some(stz2) = child(stbl, b"stz2") else { return Vec::new() };
    let (bits, listed) = (stz2.get(7).copied().unwrap_or(0), be32(stz2, 8).unwrap_or(0) as usize);
    let bytes = stz2.get(12..).unwrap_or_default();
    let size = |nth: usize| match bits {
        4 => bytes.get(nth / 2).map(|byte| u32::from(if nth.is_multiple_of(2) { byte >> 4 } else { byte & 15 })),
        8 => bytes.get(nth).map(|byte| u32::from(*byte)),
        16 => bytes.get(nth * 2..nth * 2 + 2).map(|two| u32::from(u16::from_be_bytes([two[0], two[1]]))),
        _ => None,
    };
    (0..listed.min(count)).map_while(size).collect()
}

/// A full box's entries of `size` bytes after its version, flags and count.
fn entries(full: &[u8], size: usize) -> impl Iterator<Item = &[u8]> {
    let count = be32(full, 4).unwrap_or(0) as usize;
    full.get(8..).unwrap_or_default().chunks_exact(size).take(count)
}

/// A language as MP4 packs it (three letters of five bits), none for `und`.
fn packed_language(packed: u16) -> String {
    let letters: String = [10, 5, 0].iter().map(|shift| char::from(0x60 + ((packed >> shift) & 31) as u8)).collect();
    if letters == "und" || !letters.bytes().all(|letter| letter.is_ascii_lowercase()) { String::new() } else { letters }
}

/// Text as boxes keep it: UTF-8, ending at a zero.
fn text_of(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// The box at `at` of a source: its kind, where its body starts and where it ends.
fn atom_at(source: &mut impl Source, at: u64) -> Option<([u8; 4], u64, u64)> {
    let mut header = [0u8; 16];
    if source.read_at(at, &mut header[..8]) < 8 {
        return None;
    }
    let (size, kind) = (u64::from(u32::from_be_bytes([header[0], header[1], header[2], header[3]])), [header[4], header[5], header[6], header[7]]);
    let (body, end) = match size {
        0 => (at + 8, u64::MAX),
        1 => {
            (source.read_at(at + 8, &mut header[8..]) == 8).then_some(())?;
            (at + 16, at.checked_add(u64::from_be_bytes(header[8..].try_into().ok()?))?)
        }
        size => (at + 8, at.checked_add(size)?),
    };
    (end >= body).then_some((kind, body, end))
}

/// A box's body, from `body` to `end`, read into memory: none past `MOST`, or where the source
/// gives less.
fn read_body(source: &mut impl Source, body: u64, end: u64) -> Option<Vec<u8>> {
    let length = end.checked_sub(body).filter(|length| *length <= MOST)?;
    let mut bytes = vec![0; length as usize];
    (source.read_at(body, &mut bytes) == bytes.len()).then_some(bytes)
}

/// The boxes in `bytes`, each its kind and body (a box cut short ends them).
fn atoms(mut bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let (size, kind) = (be32(bytes, 0)? as usize, bytes.get(4..8)?.try_into().ok()?);
        let (body, end) = match size {
            0 => (8, bytes.len()),
            1 => (16, usize::try_from(be64(bytes, 8)?).ok()?),
            size => (8, size),
        };
        let found = bytes.get(body..end)?;
        bytes = &bytes[end..];
        Some((kind, found))
    })
}

fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(bytes, kind).next()
}

fn children<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> impl Iterator<Item = &'a [u8]> + use<'a> {
    let kind = *kind;
    atoms(bytes).filter(move |(found, _)| *found == kind).map(|(_, body)| body)
}

/// The box down a path of kinds.
fn path<'a>(bytes: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> {
    kinds.iter().try_fold(bytes, |inside, kind| child(inside, kind))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(*bytes.get(at..)?.first_chunk::<8>()?))
}

/// The captions read from a file's caption tracks so far.
#[derive(Default)]
pub(crate) struct FileCaptions {
    /// The 3GPP timed text and WebVTT tracks: what each is, and its cues by their start.
    texts: Vec<(Track, Vec<Cue>)>,
    /// The closed-caption track's CEA-608 services, as found, and the track's language.
    closed: ClosedCaptions,
    closed_language: String,
    /// The tracks changed since they were last listed.
    changed: bool,
}

impl Reading for FileCaptions {
    /// Cues from `to` on are read again; the closed captions start again there.
    fn sought(&mut self, to: Duration) {
        self.texts.iter_mut().for_each(|(_, cues)| cues.retain(|cue| cue.from < to));
        self.closed.sought(to);
    }
}

impl FileCaptions {
    /// The file's caption tracks are these (and closed captions where `closed` holds one).
    pub(crate) fn found<'a>(&mut self, tracks: impl Iterator<Item = (&'a Track, bool)>) {
        for (track, closed) in tracks {
            match closed {
                true => self.closed_language.clone_from(&track.language),
                false => self.texts.push((track.clone(), Vec::new())),
            }
        }
        self.changed = true;
    }

    /// Whether the tracks changed since they were last listed.
    pub(crate) fn unlisted(&self) -> bool {
        self.changed || self.closed.unlisted()
    }

    /// The tracks: the text tracks, then the closed captions' services found; now listed.
    pub(crate) fn tracks(&mut self) -> Vec<Track> {
        self.changed = false;
        let closed = self.closed.tracks().into_iter().map(|track| Track { language: self.closed_language.clone(), ..track });
        self.texts.iter().map(|(track, _)| track.clone()).chain(closed).collect()
    }

    /// The captions the `nth` track shows at `at`.
    pub(crate) fn captions_at(&self, nth: usize, at: Duration) -> Vec<Caption> {
        match self.texts.get(nth) {
            Some((_, cues)) => crate::video_captions::captions_at(cues, at),
            None => self.closed.captions_at(nth - self.texts.len(), at),
        }
    }

    /// Takes in a sample read.
    pub(crate) fn take(&mut self, sample: ReadSample) {
        let ReadSample { kind, text, bytes, from, to } = sample;
        if kind == Kind::Cea608 {
            return crate::video_stream_captions::cea608_pairs(&bytes).for_each(|(field, pair)| self.closed.read_cea608_pair(field, pair, from));
        }
        let Some((_, cues)) = self.texts.get_mut(text) else { return };
        let captions = match kind {
            Kind::TimedText(place) => Some(crate::video_captions::timed_text(&bytes)).filter(|text| !text.is_empty()).map(|text| Caption { place, ..Caption::plain(text) }).into_iter().collect(),
            _ => webvtt_cues(&bytes),
        };
        let at = cues.partition_point(|cue| cue.from < from);
        if cues.get(at).is_some_and(|cue| cue.from == from) {
            return;
        }
        let new: Vec<Cue> = captions.into_iter().map(|caption| Cue { from, to, caption }).collect();
        cues.splice(at..at, new);
    }
}

/// A WebVTT sample's cues (`vttc` boxes: their text in `payl`, their settings in `sttg`); none
/// for a gap (`vtte`).
fn webvtt_cues(sample: &[u8]) -> Vec<Caption> {
    let cue = |vttc: &[u8]| {
        let text = text_of(child(vttc, b"payl")?);
        let settings = child(vttc, b"sttg").map(text_of).unwrap_or_default();
        Some(crate::video_captions::caption_with_settings(&text, &settings))
    };
    children(sample, b"vttc").filter_map(cue).filter(|caption| !caption.text.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Align;
    use crate::video_captions::Down;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::{Carried, Coding, ENGLISH, InStream, PLAIN_AND_PLACED as TWO, atom, be32 as words};

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    fn reader(bytes: Vec<u8>) -> Reader<std::sync::Arc<[u8]>> {
        Reader::open(std::sync::Arc::<[u8]>::from(bytes)).expect("caption tracks")
    }

    /// Reads all of a file's captions in time order.
    fn read_all(bytes: Vec<u8>) -> (Vec<Track>, FileCaptions) {
        let mut reader = reader(bytes);
        let mut captions = FileCaptions::default();
        captions.found(reader.tracks());
        while let Some(sample) = reader.read_next() {
            captions.take(sample);
        }
        (captions.tracks(), captions)
    }

    fn texts_at(captions: &FileCaptions, nth: usize, tenths: u64) -> Vec<String> {
        captions.captions_at(nth, at(tenths)).into_iter().map(|caption| caption.text).collect()
    }

    #[test]
    fn timed_text_and_webvtt_tracks_are_read_with_their_languages_names_and_times() {
        let (tracks, captions) = read_all(files::movie(30, 10, false, &[], &TWO));
        assert_eq!(tracks, [Track { language: "eng".into(), name: "English".into() }, Track { language: "fra".into(), name: String::new() }]);
        assert_eq!(texts_at(&captions, 0, 5), ["English one"]);
        assert_eq!(texts_at(&captions, 1, 15), ["Français deux"]);
        assert!(texts_at(&captions, 0, 22).is_empty());
    }

    #[test]
    fn webvtt_settings_in_a_sample_place_its_cue() {
        let (_, captions) = read_all(files::movie(30, 10, false, &[], &TWO));
        let place = captions.captions_at(1, at(26))[0].place.expect("a place");
        assert_eq!((place.across, place.down), (Align::Start, Down::At(0.1, Align::Start)));
        assert_eq!(captions.captions_at(0, at(26))[0].place, None);
    }

    #[test]
    fn a_closed_caption_track_is_a_track_for_each_service_it_holds() {
        const CAPTIONED: InStream = InStream { cea608: &[(5, 12, "Hello there")], other_channels: &[], services: &[] };
        let (tracks, captions) = read_all(files::movie_captioned_in_stream(30, Coding::H264, &CAPTIONED, Carried::InCaptionTrack));
        assert_eq!(tracks, [Track { language: "eng".into(), name: "CC1".into() }]);
        assert_eq!(texts_at(&captions, 0, 8), ["Hello there"]);
    }

    #[test]
    fn after_a_seek_samples_are_read_from_there_once() {
        let mut reader = reader(files::movie(30, 10, false, &[], &TWO));
        let mut captions = FileCaptions::default();
        captions.found(reader.tracks());
        while let Some(sample) = reader.read_next() {
            captions.take(sample);
        }
        reader.seek(at(15));
        // (English two and Français deux run from 1 s to 2 s: the first samples to end after
        // 1.5 s.)
        assert_eq!(reader.read_next().map(|sample| sample.from), Some(at(10)));
        while let Some(sample) = reader.read_next() {
            captions.take(sample);
        }
        assert_eq!(captions.texts[0].1.len(), ENGLISH.len(), "{:?}", captions.texts[0].1);
    }

    #[test]
    fn fragmented_files_are_read_fragment_by_fragment() {
        let (tracks, captions) = read_all(files::fragmented_with_captions(30, &TWO[..1]));
        assert_eq!(tracks.len(), 1);
        assert_eq!((texts_at(&captions, 0, 5), texts_at(&captions, 0, 26)), (vec!["English one".to_owned()], vec!["English end".to_owned()]));
        // (A seek reads its fragments again from the first.)
        let mut reader = reader(files::fragmented_with_captions(30, &TWO[..1]));
        reader.seek(at(25));
        let times: Vec<Duration> = std::iter::from_fn(|| reader.read_next()).map(|sample| sample.from).collect();
        assert_eq!(times, [at(25)]);
    }

    /// A track fragment of a made file: its track's id and its samples' texts.
    type TrackFragment<'a> = (u32, &'a [&'a str]);

    /// A fragmented file of `TWO`'s tracks (1: timed text, 2: WebVTT) with these fragments:
    /// each its track fragments, a track id and its samples' texts (each a second long, the
    /// first at the fragment's number in seconds). Each run's data counts from the fragment
    /// where `from_fragment`; else each track fragment's but the first's follows the one before.
    fn fragmented(fragments: &[&[TrackFragment]], from_fragment: bool) -> Vec<u8> {
        let mut file = files::fragmented_with_captions(10, &TWO);
        let mut at = 0;
        while &file[at + 4..at + 8] != b"moof" {
            at += u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        }
        file.truncate(at);
        for (second, trafs) in (0..).zip(fragments) {
            let sample = |id: u32, text: &str| match id {
                1 => [(text.len() as u16).to_be_bytes().to_vec(), text.as_bytes().to_vec()].concat(),
                _ => files::atoms(b"vttc", &[atom(b"payl", text.as_bytes())]),
            };
            let data: Vec<Vec<Vec<u8>>> = trafs.iter().map(|(id, texts)| texts.iter().map(|text| sample(*id, text)).collect()).collect();
            let moof = |data_at: u32| {
                let mut offset = data_at;
                let made = trafs.iter().zip(&data).enumerate().map(|(nth, ((id, _), samples))| {
                    let tfhd = atom(b"tfhd", &words(&[if from_fragment { 0x02_0000 } else { 0 }, *id]));
                    let tfdt = atom(b"tfdt", &words(&[0, second * 1000]));
                    let sizes: Vec<u32> = samples.iter().flat_map(|bytes| [1000, bytes.len() as u32]).collect();
                    let run = if from_fragment || nth == 0 { words(&[0x301, samples.len() as u32, offset]) } else { words(&[0x300, samples.len() as u32]) };
                    offset += samples.iter().map(Vec::len).sum::<usize>() as u32;
                    files::atoms(b"traf", &[tfhd, tfdt, atom(b"trun", &[run, words(&sizes)].concat())])
                });
                files::atoms(b"moof", &made.collect::<Vec<_>>())
            };
            let size = moof(0).len() as u32;
            file.extend([moof(size + 8), atom(b"mdat", &data.concat().concat())].concat());
        }
        file
    }

    #[test]
    fn a_track_fragment_without_a_base_follows_the_one_before() {
        let (_, captions) = read_all(fragmented(&[&[(1, &["Plain"]), (2, &["Placed"])]], false));
        assert_eq!((texts_at(&captions, 0, 5), texts_at(&captions, 1, 5)), (vec!["Plain".to_owned()], vec!["Placed".to_owned()]));
    }

    /// Bytes that keep the furthest place read from them.
    struct Watched(std::sync::Arc<[u8]>, std::sync::Arc<std::sync::atomic::AtomicU64>);

    impl Source for Watched {
        fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
            let read = self.0.read_at(at, into);
            self.1.fetch_max(at + read as u64, std::sync::atomic::Ordering::Relaxed);
            read
        }
    }

    #[test]
    fn a_track_that_ends_early_is_not_waited_for_to_the_files_end() {
        // (The French track has one sample, in the first of 100 fragments.)
        let (firsts, more): ([TrackFragment; 2], [TrackFragment; 1]) = ([(1, &["one"]), (2, &["un"])], [(1, &["more"])]);
        let mut fragments: Vec<&[TrackFragment]> = vec![&firsts];
        fragments.extend(std::iter::repeat_n(&more[..], 99));
        let bytes: std::sync::Arc<[u8]> = fragmented(&fragments, true).into();
        let furthest = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut reader = Reader::open(Watched(bytes.clone(), furthest.clone())).expect("caption tracks");
        let times: Vec<Duration> = (0..3).filter_map(|_| reader.read_next()).map(|sample| sample.from).collect();
        assert_eq!(times, [at(0), at(0), at(10)]);
        let read = furthest.load(std::sync::atomic::Ordering::Relaxed);
        assert!(read < bytes.len() as u64 / 2, "{read} bytes of {} read for the first samples", bytes.len());
        assert_eq!(std::iter::from_fn(|| reader.read_next()).count(), 98);
    }

    #[test]
    fn a_sample_tables_chunks_follow_its_runs_and_far_offsets_stay_in_range() {
        // (Chunks 1 and 2 of a sample each, 3 and 4 of two; the last two chunks at the very end.)
        let far = u64::MAX - 5;
        let stbl = [
            atom(b"stts", &words(&[0, 1, 6, 10])),
            atom(b"stsz", &words(&[0, 4, 6])),
            atom(b"stsc", &words(&[0, 2, 1, 1, 1, 3, 2, 1])),
            atom(b"co64", &words(&[0, 4, 0, 100, 0, 200, (far >> 32) as u32, far as u32, u32::MAX, u32::MAX])),
        ]
        .concat();
        let places: Vec<u64> = sample_table(&stbl, 100).iter().map(|sample| sample.at).collect();
        assert_eq!(places, [100, 200, far, far + 4, u64::MAX, u64::MAX]);
        assert_eq!(sample_table(&stbl, 3).len(), 3);
    }

    #[test]
    fn an_edit_list_moves_a_tracks_times() {
        // (An empty edit of 1 s, then the track from 0.5 s on: times move by half a second.)
        let elst = [words(&[0, 2]), words(&[1000, u32::MAX, 0x10000]), words(&[2000, 500, 0x10000])].concat();
        assert_eq!(edit_shift(&elst, 1000, 1000), 500_000_000);
    }

    #[test]
    fn timed_text_justification_places_a_track() {
        let entry = |across: u8, down: u8| [&[0; 6][..], &[0, 1], &[0, 0, 0, 0], &[across, down]].concat();
        assert_eq!(timed_text_place(&entry(1, 0xFF)), None);
        let top_left = timed_text_place(&entry(0, 0)).unwrap();
        assert_eq!((top_left.across, top_left.down), (Align::Start, Down::At(0.05, Align::Start)));
    }

    #[test]
    fn a_paced_reader_lists_the_tracks_reads_as_the_video_plays_and_stops_with_it() {
        let read_ahead = std::sync::Arc::new(ReadAhead::<FileCaptions>::new());
        let bytes = std::sync::Arc::<[u8]>::from(files::movie(30, 10, false, &[], &TWO));
        read_ahead.spawn(move |captions| read_paced(bytes, captions));
        let start = std::time::Instant::now();
        let waited = |done: &dyn Fn(&mut FileCaptions) -> bool| {
            while !read_ahead.with(|captions| done(captions)) {
                assert!(start.elapsed() < Duration::from_secs(10), "the reader did not get there");
                std::thread::yield_now();
            }
        };
        waited(&|captions| captions.unlisted());
        assert_eq!(read_ahead.with(FileCaptions::tracks).len(), 2);
        assert!(!read_ahead.with(|captions| captions.unlisted()));
        // (All of it within 5 s of the start: read without the video moving.)
        waited(&|captions| !captions.captions_at(1, at(26)).is_empty());
        read_ahead.stop();
    }

    #[test]
    fn what_is_no_mp4_or_has_no_caption_tracks_is_not_read() {
        assert!(Reader::open(std::sync::Arc::<[u8]>::from(b"\x1A\x45\xDF\xA3 matroska".to_vec())).is_none());
        assert!(Reader::open(std::sync::Arc::<[u8]>::from(files::mp4(10, 10, false))).is_none());
        // (A box claiming more than there is.)
        let cut = [atom(b"ftyp", b"isom"), words(&[1000]), b"moov".to_vec()].concat();
        assert!(Reader::open(std::sync::Arc::<[u8]>::from(cut)).is_none());
    }
}
