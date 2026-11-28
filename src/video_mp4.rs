//! The caption tracks of MP4 and QuickTime files, read by taut for players that read none of
//! them (Media Foundation): 3GPP timed text (`tx3g`, which ffmpeg calls `mov_text`) and
//! QuickTime text, WebVTT (`wvtt`), and closed captions (`c608`: each CEA-608 service a track).
//! Plain and fragmented files are read, a sample at a time in time order, as a reading thread
//! paced by the video asks (`video_read_ahead`): a file at an address is not read ahead of the
//! player. A seek in a fragmented file starts at the fragment it needs, found in the file's
//! index (`mfra`, `sidx`) or among the fragments read before.

use crate::video::Track;
use crate::video_captions::{Caption, Cue, Place};
use crate::video_read_ahead::{ReadAhead, Reading, Step};
use crate::video_stream_captions::ClosedCaptions;
use std::time::Duration;

/// Where a file's bytes come from: a file, bytes, a download.
pub(crate) trait Source: Send {
    /// Reads bytes from `at` into `into`: how many (fewer at the end, 0 past it or on failure).
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize;
    /// How many bytes there are, where known.
    fn size(&mut self) -> Option<u64>;
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
    fn size(&mut self) -> Option<u64> {
        self.metadata().ok().map(|known| known.len())
    }
}

impl Source for std::sync::Arc<[u8]> {
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
        let Some(rest) = usize::try_from(at).ok().and_then(|at| self.get(at..)) else { return 0 };
        let read = rest.len().min(into.len());
        into[..read].copy_from_slice(&rest[..read]);
        read
    }
    fn size(&mut self) -> Option<u64> {
        Some(self.len() as u64)
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
/// The most fragments remembered of a caption track, and boxes looked at for segment indexes
/// (`sidx`, which may point to more).
const MOST_FRAGMENTS: usize = 1 << 14;
const MOST_INDEXES: usize = 1024;
/// How much of a box running to the end of its file is read at a time.
const CHUNK: usize = 1 << 16;

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
    /// The fragments it has samples in, as far as known, by where they are.
    fragments: Vec<Fragment>,
}

/// A fragment a caption track has samples in: where its box starts, and when the track's first
/// sample in it shows and its last ends, in the track's time scale (the end `u64::MAX` where it
/// was not read: listed by the file's index).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fragment {
    at: u64,
    from: u64,
    to: u64,
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

    /// Where reading fragments must start to have its samples from `to` on: at the fragment of
    /// its sample showing then, else of its first after. Its fragments before `known_to` are all
    /// known; past it, only some may be.
    fn reading_start(&self, to: Duration, known_to: u64) -> u64 {
        let showing = self.fragments.iter().rposition(|fragment| self.movie_time(fragment.from) <= to);
        // (Its samples before that fragment end by the time it starts.)
        if let Some(fragment) = showing.map(|nth| self.fragments[nth])
            && (self.movie_time(fragment.to) > to || fragment.at >= known_to)
        {
            return fragment.at;
        }
        let next = self.fragments.get(showing.map_or(0, |nth| nth + 1));
        next.map_or(known_to, |fragment| fragment.at.min(known_to))
    }

    /// Remembers a fragment it has samples in, `MOST_FRAGMENTS` at most: one read takes the
    /// place of one listed by the index, and track fragments read of one fragment make one.
    fn remember(&mut self, found: Fragment) {
        match self.fragments.binary_search_by_key(&found.at, |fragment| fragment.at) {
            Ok(_) if found.to == u64::MAX => {}
            Ok(nth) if self.fragments[nth].to == u64::MAX => self.fragments[nth] = found,
            Ok(nth) => {
                let known = &mut self.fragments[nth];
                (known.from, known.to) = (known.from.min(found.from), known.to.max(found.to));
            }
            Err(nth) if self.fragments.len() < MOST_FRAGMENTS => self.fragments.insert(nth, found),
            Err(_) => {}
        }
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
    /// Up to where fragments were read in order from the first: each caption track's fragments
    /// before it are all known.
    known_to: u64,
    /// Every caption track fragment read gave its decode time (`tfdt`): a seek may start past
    /// the first fragment.
    timed: bool,
}

impl<S: Source> Reader<S> {
    /// Reads a file's movie box, and a fragmented file's index: none where it is no MP4 or
    /// QuickTime file, or has no caption tracks.
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
        let fragmented = defaults.is_some();
        let mut reader = Reader { source, tracks, fragments: fragmented.then_some((after, after)), defaults: defaults.unwrap_or_default(), known_to: after, timed: true };
        if fragmented {
            reader.take_random_access_index();
            reader.take_segment_indexes_from(after);
        }
        Some(reader)
    }

    /// Takes in the fragments of caption tracks that the file's random access index lists (an
    /// `mfra` box at its end, its size in the `mfro` box that ends it).
    fn take_random_access_index(&mut self) {
        let Some(length) = self.source.size().filter(|length| *length >= 16) else { return };
        let mut tail = [0u8; 16];
        if self.source.read_at(length - 16, &mut tail) < 16 || &tail[4..8] != b"mfro" {
            return;
        }
        let Some(at) = length.checked_sub(u64::from(u32::from_be_bytes([tail[12], tail[13], tail[14], tail[15]]))) else { return };
        let Some((_, body, end)) = atom_at(&mut self.source, at).filter(|(kind, ..)| kind == b"mfra") else { return };
        let Some(mfra) = read_body(&mut self.source, body, end) else { return };
        for tfra in children(&mfra, b"tfra") {
            self.take_random_access_entries(tfra);
        }
    }

    /// Takes in a track's random access entries (`tfra`) where it is a caption track: those of
    /// a fragment's first sample (its first track fragment's first run's first sample).
    fn take_random_access_entries(&mut self, tfra: &[u8]) {
        let (Some(id), Some(sizes), Some(count)) = (be32(tfra, 4), be32(tfra, 8), be32(tfra, 12)) else { return };
        let Some(track) = self.tracks.iter_mut().find(|track| track.id == id) else { return };
        let wide = tfra.first() == Some(&1);
        let times = if wide { 16 } else { 8 };
        // (Its numbers of track fragment, run and sample, 1 to 4 bytes each, all 1.)
        let firsts: Vec<u8> = [(sizes >> 4) & 3, (sizes >> 2) & 3, sizes & 3].iter().flat_map(|size| std::iter::repeat_n(0, *size as usize).chain([1])).collect();
        for entry in tfra.get(16..).unwrap_or_default().chunks_exact(times + firsts.len()).take(count as usize) {
            let (from, at) = match wide {
                true => (be64(entry, 0), be64(entry, 8)),
                false => (be32(entry, 0).map(u64::from), be32(entry, 4).map(u64::from)),
            };
            if let (Some(from), Some(at), true) = (from, at, entry[times..] == firsts[..]) {
                track.remember(Fragment { at, from, to: u64::MAX });
            }
        }
    }

    /// Takes in the segment index boxes (`sidx`) between the movie box, ending at `at`, and the
    /// first fragment.
    fn take_segment_indexes_from(&mut self, mut at: u64) {
        for _ in 0..MOST_INDEXES {
            let Some((kind, _, end)) = atom_at(&mut self.source, at) else { return };
            match &kind {
                b"moof" | b"mdat" => return,
                b"sidx" => self.take_segment_indexes(at),
                _ => {}
            }
            at = end;
        }
    }

    /// Takes in the fragments of caption tracks that the segment index box at `at` lists, and
    /// the boxes it points to (`MOST_INDEXES` at most).
    fn take_segment_indexes(&mut self, at: u64) {
        let (mut pending, mut read) = (vec![at], 0);
        while let Some(at) = pending.pop().filter(|_| read < MOST_INDEXES) {
            read += 1;
            let Some((_, body, end)) = atom_at(&mut self.source, at).filter(|(kind, ..)| kind == b"sidx") else { continue };
            if let Some(sidx) = read_body(&mut self.source, body, end) {
                pending.extend(self.take_segment_index(&sidx, end));
            }
        }
    }

    /// Takes in the fragments a segment index box lists where its track is a caption track:
    /// each reference to media is one (the first `first_offset` past the box's `end`, each next
    /// right after it). Gives where the segment index boxes it points to are.
    fn take_segment_index(&mut self, sidx: &[u8], end: u64) -> Vec<u64> {
        let (Some(id), Some(scale)) = (be32(sidx, 4), be32(sidx, 8)) else { return Vec::new() };
        let (time, offset, references) = match sidx.first() == Some(&1) {
            true => (be64(sidx, 12), be64(sidx, 20), 28),
            false => (be32(sidx, 12).map(u64::from), be32(sidx, 16).map(u64::from), 20),
        };
        let (Some(mut time), Some(offset), Some(count)) = (time, offset, be32(sidx, references).map(|both| both & 0xFFFF)) else { return Vec::new() };
        let mut track = self.tracks.iter_mut().find(|track| track.id == id);
        let (mut at, mut nested) = (end.saturating_add(offset), Vec::new());
        for reference in sidx.get(references + 4..).unwrap_or_default().as_chunks::<12>().0.iter().take(count as usize) {
            let (Some(kind_and_size), Some(duration)) = (be32(reference, 0), be32(reference, 4)) else { break };
            if kind_and_size >> 31 == 1 {
                nested.push(at);
            } else if let Some(track) = track.as_mut() {
                track.remember(Fragment { at, from: rescaled(time, scale, track.timescale), to: u64::MAX });
            }
            at = at.saturating_add(u64::from(kind_and_size & 0x7FFF_FFFF));
            time = time.saturating_add(u64::from(duration));
        }
        nested
    }

    /// The caption tracks (3GPP timed text and WebVTT first, then closed captions).
    pub(crate) fn tracks(&self) -> impl Iterator<Item = (&Track, bool)> {
        self.tracks.iter().map(|track| (&track.track, track.kind == Kind::Cea608))
    }

    /// Reads from `to` on: each track from its first sample that ends after it (in a fragmented
    /// file, read again from the fragment that holds it, as far as known).
    pub(crate) fn seek(&mut self, to: Duration) {
        if let Some((first, _)) = self.fragments {
            let start = self.tracks.iter().map(|track| track.reading_start(to, self.known_to)).min().filter(|_| self.timed);
            self.fragments = Some((first, start.map_or(first, |start| start.max(first))));
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
    /// whether there was one. Segment index boxes on the way are taken in.
    fn read_fragment(&mut self) -> bool {
        let Some((first, mut at)) = self.fragments else { return false };
        let in_order = at <= self.known_to;
        while let Some((kind, body, end)) = atom_at(&mut self.source, at) {
            let start = std::mem::replace(&mut at, end);
            match &kind {
                b"moof" => return self.take_moof(start, (body, end), in_order),
                b"sidx" => self.take_segment_indexes(start),
                _ => {}
            }
        }
        if in_order {
            self.known_to = u64::MAX;
        }
        self.fragments = Some((first, u64::MAX));
        false
    }

    /// Takes in the fragment whose box starts at `moof` (its body from `body` to `end`): whether
    /// it could be read. Fragments are known up to it where they were read `in_order`.
    fn take_moof(&mut self, moof: u64, (body, end): (u64, u64), in_order: bool) -> bool {
        let Some((first, _)) = self.fragments else { return false };
        let Some(bytes) = read_body(&mut self.source, body, end) else {
            self.fragments = Some((first, u64::MAX));
            return false;
        };
        self.fragments = Some((first, end));
        for track in &mut self.tracks {
            // (The last sample read stays: a fragment without a decode time goes on from it.)
            track.samples.drain(..track.next.saturating_sub(1));
            track.next = track.next.min(1);
        }
        // (The first track fragment's data counts from the fragment, each next one's from where
        // the one before's ends, unless they say otherwise.)
        let mut implicit = moof;
        for traf in children(&bytes, b"traf") {
            implicit = self.take_fragment(traf, moof, implicit);
        }
        // (A track whose fragments are no longer all remembered leaves the rest unknown.)
        if in_order && self.tracks.iter().all(|track| track.fragments.len() < MOST_FRAGMENTS) {
            self.known_to = self.known_to.max(end);
        }
        true
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
        let decode_time = child(traf, b"tfdt").and_then(decode_time);
        self.timed &= decode_time.is_some() || track.is_none();
        let mut time = decode_time.or_else(|| track.as_ref()?.samples.last().map(Sample::end)).unwrap_or(0);
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
        let Some(track) = track else { return at };
        if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
            track.remember(Fragment { at: moof, from: first.time, to: last.end() });
        }
        let room = MOST_SAMPLES.saturating_sub(track.samples.len());
        track.samples.extend(samples.into_iter().take(room));
        at
    }
}

/// A time in `from` units a second as `to` units a second.
fn rescaled(time: u64, from: u32, to: u64) -> u64 {
    u64::try_from(u128::from(time) * u128::from(to) / u128::from(from.max(1))).unwrap_or(u64::MAX)
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
    Some(CaptionTrack { kind, track: Track { language, name }, id, timescale, shift, samples, next: 0, fragments: Vec::new() })
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

/// The box at `at` of a source: its kind, where its body starts and where it ends (`u64::MAX`
/// for one that runs to the end of the source).
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

/// A box's body, from `body` to `end` (`u64::MAX`: the source's end), read into memory: none
/// past `MOST`, or where the source gives less.
fn read_body(source: &mut impl Source, body: u64, end: u64) -> Option<Vec<u8>> {
    if end == u64::MAX {
        return read_rest(source, body);
    }
    let length = end.checked_sub(body).filter(|length| *length <= MOST)?;
    let mut bytes = vec![0; length as usize];
    (source.read_at(body, &mut bytes) == bytes.len()).then_some(bytes)
}

/// The rest of a source from `body`, `CHUNK` bytes at a time until it gives less: none past
/// `MOST`.
fn read_rest(source: &mut impl Source, body: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let start = bytes.len();
        bytes.resize(start + CHUNK, 0);
        let read = source.read_at(body.checked_add(start as u64)?, &mut bytes[start..]);
        bytes.truncate(start + read);
        if read < CHUNK {
            return Some(bytes);
        }
        if bytes.len() as u64 >= MOST {
            return None;
        }
    }
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
        // (A seek before any fragment was read reads them from the first.)
        let mut reader = reader(files::fragmented_with_captions(30, &TWO[..1]));
        reader.seek(at(25));
        let times: Vec<Duration> = std::iter::from_fn(|| reader.read_next()).map(|sample| sample.from).collect();
        assert_eq!(times, [at(25)]);
    }

    /// A made file's index of its fragments.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Index {
        None,
        /// A random access index at its end (`mfra`).
        RandomAccess,
        /// Segment index boxes before its fragments: track 2's, and track 1's through a box that
        /// points to it.
        Segments,
    }

    /// A fragment of a made file: its track fragments, each its track's id and its samples, each
    /// its text and how many seconds it lasts.
    type Made = Vec<(u32, Vec<(String, u32)>)>;

    /// `count` fragments, each a second of each track: "English" and "Français" and its number.
    fn each_second(count: u32) -> Vec<Made> {
        (0..count).map(|n| vec![(1, vec![(format!("English {n}"), 1)]), (2, vec![(format!("Français {n}"), 1)])]).collect()
    }

    /// A fragmented file of `TWO`'s tracks (1: timed text, 2: WebVTT) with these fragments, the
    /// `n`th from `n` s, and this index. Each run's data counts from the fragment where
    /// `from_fragment`; else each track fragment's but the first's follows the one before.
    fn fragmented(fragments: &[Made], from_fragment: bool, index: Index) -> Vec<u8> {
        let mut head = files::fragmented_with_captions(10, &TWO);
        let mut at = 0;
        while &head[at + 4..at + 8] != b"moof" {
            at += u32::from_be_bytes(head[at..at + 4].try_into().unwrap()) as usize;
        }
        head.truncate(at);
        let made: Vec<Vec<u8>> = (0..).zip(fragments).map(|(second, trafs)| fragment(second, trafs, from_fragment)).collect();
        match index {
            Index::None => [head, made.concat()].concat(),
            Index::RandomAccess => {
                let index = random_access_index(fragments, &made, head.len());
                [head, made.concat(), index].concat()
            }
            Index::Segments => {
                let spans = |id: u32| track_spans(fragments, &made, id);
                let (one, two) = (spans(1), spans(2));
                let second = segment_index(2, &two, 0);
                let first = segment_index(1, &one, second.len() as u32);
                let pointing = segment_index(1, &(one.0, vec![(true, first.len() as u32, 0)]), 0);
                [head, pointing, first, second, made.concat()].concat()
            }
        }
    }

    /// A made fragment from `second` s: its `moof` and `mdat`.
    fn fragment(second: u32, trafs: &Made, from_fragment: bool) -> Vec<u8> {
        let sample = |id: u32, text: &str| match id {
            1 => [(text.len() as u16).to_be_bytes().to_vec(), text.as_bytes().to_vec()].concat(),
            _ => files::atoms(b"vttc", &[atom(b"payl", text.as_bytes())]),
        };
        let data: Vec<Vec<Vec<u8>>> = trafs.iter().map(|(id, samples)| samples.iter().map(|(text, _)| sample(*id, text)).collect()).collect();
        let moof = |data_at: u32| {
            let mut offset = data_at;
            let made = trafs.iter().zip(&data).enumerate().map(|(nth, ((id, samples), bytes))| {
                let tfhd = atom(b"tfhd", &words(&[if from_fragment { 0x02_0000 } else { 0 }, *id]));
                let tfdt = atom(b"tfdt", &words(&[0, second * 1000]));
                let sizes: Vec<u32> = samples.iter().zip(bytes).flat_map(|((_, seconds), bytes)| [seconds * 1000, bytes.len() as u32]).collect();
                let run = if from_fragment || nth == 0 { words(&[0x301, samples.len() as u32, offset]) } else { words(&[0x300, samples.len() as u32]) };
                offset += bytes.iter().map(Vec::len).sum::<usize>() as u32;
                files::atoms(b"traf", &[tfhd, tfdt, atom(b"trun", &[run, words(&sizes)].concat())])
            });
            files::atoms(b"moof", &made.collect::<Vec<_>>())
        };
        let size = moof(0).len() as u32;
        [moof(size + 8), atom(b"mdat", &data.concat().concat())].concat()
    }

    /// The fragments track `id` has samples in: the number of each and its samples' seconds.
    fn track_fragments(fragments: &[Made], id: u32) -> Vec<(usize, u32)> {
        let seconds = |trafs: &Made| trafs.iter().find(|(of, _)| *of == id).map(|(_, samples)| samples.iter().map(|(_, seconds)| seconds).sum());
        fragments.iter().enumerate().filter_map(|(nth, trafs)| Some((nth, seconds(trafs)?))).collect()
    }

    /// A random access index (`mfra`) of both tracks' fragments, the first made fragment `head`
    /// bytes into the file: an entry for each, of its first sample (numbers of one byte).
    fn random_access_index(fragments: &[Made], made: &[Vec<u8>], head: usize) -> Vec<u8> {
        let starts: Vec<u32> = made.iter().scan(head, |at, bytes| Some(std::mem::replace(at, *at + bytes.len()) as u32)).collect();
        let tfra = |id: u32| {
            let listed = track_fragments(fragments, id);
            let entries: Vec<u8> = listed.iter().flat_map(|(nth, _)| [words(&[*nth as u32 * 1000, starts[*nth]]), vec![1, 1, 1]].concat()).collect();
            atom(b"tfra", &[words(&[0, id, 0, listed.len() as u32]), entries].concat())
        };
        let tfras = [tfra(1), tfra(2)].concat();
        let mfro = atom(b"mfro", &words(&[0, (8 + tfras.len() + 16) as u32]));
        files::atoms(b"mfra", &[tfras, mfro])
    }

    /// A segment index's references: its first time in seconds, and each reference whether it
    /// points to another index, its size and seconds.
    type References = (u32, Vec<(bool, u32, u32)>);

    /// Track `id`'s fragments as references to media: each from one of its fragments to its next
    /// (or the last's end).
    fn track_spans(fragments: &[Made], made: &[Vec<u8>], id: u32) -> References {
        let listed = track_fragments(fragments, id);
        let size = |nth: usize| {
            let to = listed.get(nth + 1).map_or(made.len(), |(next, _)| *next);
            made[listed[nth].0..to].iter().map(Vec::len).sum::<usize>() as u32
        };
        (listed[0].0 as u32, (0..listed.len()).map(|nth| (false, size(nth), listed[nth].1)).collect())
    }

    /// A segment index box (`sidx`) of track `id`'s references, the first `first` bytes past it.
    fn segment_index(id: u32, (from, references): &References, first: u32) -> Vec<u8> {
        let listed: Vec<u32> = references.iter().flat_map(|(points, size, seconds)| [u32::from(*points) << 31 | size, seconds * 1000, 0]).collect();
        atom(b"sidx", &[words(&[0, id, 1000, from * 1000, first, references.len() as u32]), words(&listed)].concat())
    }

    #[test]
    fn a_track_fragment_without_a_base_follows_the_one_before() {
        let fragments = [vec![(1, vec![("Plain".into(), 1)]), (2, vec![("Placed".into(), 1)])]];
        let (_, captions) = read_all(fragmented(&fragments, false, Index::None));
        assert_eq!((texts_at(&captions, 0, 5), texts_at(&captions, 1, 5)), (vec!["Plain".to_owned()], vec!["Placed".to_owned()]));
    }

    /// Bytes that count how many were read of them.
    struct Watched(std::sync::Arc<[u8]>, std::sync::Arc<std::sync::atomic::AtomicU64>);

    impl Source for Watched {
        fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
            let read = self.0.read_at(at, into);
            self.1.fetch_add(read as u64, std::sync::atomic::Ordering::Relaxed);
            read
        }
        fn size(&mut self) -> Option<u64> {
            self.0.size()
        }
    }

    /// A reader of `bytes` that counts how many it read of them.
    fn watched(bytes: &[u8]) -> (Reader<Watched>, std::sync::Arc<std::sync::atomic::AtomicU64>) {
        let read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        (Reader::open(Watched(bytes.into(), read.clone())).expect("caption tracks"), read)
    }

    /// How many bytes were read while `read` ran.
    fn bytes_read(counted: &std::sync::atomic::AtomicU64, read: impl FnOnce()) -> u64 {
        let before = counted.load(std::sync::atomic::Ordering::Relaxed);
        read();
        counted.load(std::sync::atomic::Ordering::Relaxed) - before
    }

    /// The next `count` samples read: when each shows, and its text.
    fn next_samples<S: Source>(reader: &mut Reader<S>, count: usize) -> Vec<(Duration, String)> {
        let text = |sample: &ReadSample| match sample.kind {
            Kind::TimedText(_) => crate::video_captions::timed_text(&sample.bytes),
            _ => webvtt_cues(&sample.bytes).into_iter().map(|caption| caption.text).collect(),
        };
        (0..count).filter_map(|_| reader.read_next()).map(|sample| (sample.from, text(&sample))).collect()
    }

    #[test]
    fn a_track_that_ends_early_is_not_waited_for_to_the_files_end() {
        // (The French track has one sample, in the first of 100 fragments.)
        let mut fragments = vec![vec![(1, vec![("one".into(), 1)]), (2, vec![("un".into(), 1)])]];
        fragments.extend((1..100).map(|_| vec![(1, vec![("more".into(), 1)])]));
        let bytes = fragmented(&fragments, true, Index::None);
        let (mut reader, counted) = watched(&bytes);
        let mut times = Vec::new();
        let read = bytes_read(&counted, || times = (0..3).filter_map(|_| reader.read_next()).map(|sample| sample.from).collect());
        assert_eq!(times, [at(0), at(0), at(10)]);
        assert!(read < bytes.len() as u64 / 2, "{read} bytes of {} read for the first samples", bytes.len());
        assert_eq!(std::iter::from_fn(|| reader.read_next()).count(), 98);
    }

    #[test]
    fn a_seek_reads_from_the_fragment_the_files_index_gives() {
        for index in [Index::RandomAccess, Index::Segments] {
            let bytes = fragmented(&each_second(100), true, index);
            let (mut reader, counted) = watched(&bytes);
            assert!(reader.tracks.iter().all(|track| track.fragments.len() == 100), "{index:?}: the index lists every fragment");
            let mut shown = Vec::new();
            let read = bytes_read(&counted, || {
                reader.seek(at(805));
                shown = next_samples(&mut reader, 2);
            });
            assert_eq!(shown, [(at(800), "English 80".to_owned()), (at(800), "Français 80".to_owned())], "{index:?}");
            assert!(read < bytes.len() as u64 / 20, "{index:?}: {read} bytes of {} read for a seek", bytes.len());
        }
    }

    #[test]
    fn without_an_index_a_seek_starts_at_a_fragment_read_before() {
        let bytes = fragmented(&each_second(100), true, Index::None);
        let (mut reader, counted) = watched(&bytes);
        // (Before any was read: from the first.)
        let first = bytes_read(&counted, || reader.seek(at(505)));
        assert!(first > bytes.len() as u64 / 3, "{first} bytes of {} read for a first seek", bytes.len());
        while reader.read_next().is_some_and(|sample| sample.from < at(900)) {}
        // (Back among the fragments read, and on past them.)
        for (to, second) in [(205, 20), (955, 95)] {
            let mut shown = Vec::new();
            let read = bytes_read(&counted, || {
                reader.seek(at(to));
                shown = next_samples(&mut reader, 1);
            });
            assert_eq!(shown, [(at(second * 10), format!("English {second}"))]);
            assert!(read < bytes.len() as u64 / 20, "{read} bytes of {} read for a seek to {:?}", bytes.len(), at(to));
        }
    }

    #[test]
    fn a_caption_that_began_fragments_before_a_seek_is_read() {
        // (English "Long" runs from 10 s to 25 s, in fragment 10; French has a sample a second.)
        let mut fragments = each_second(30);
        fragments[10][0].1 = vec![("Long".into(), 15)];
        fragments[11..25].iter_mut().for_each(|trafs| _ = trafs.remove(0));
        for (index, read_first) in [(Index::None, false), (Index::None, true), (Index::RandomAccess, false), (Index::Segments, false)] {
            let mut reader = reader(fragmented(&fragments, true, index));
            if read_first {
                while reader.read_next().is_some() {}
            }
            reader.seek(at(205));
            assert_eq!(next_samples(&mut reader, 2), [(at(100), "Long".to_owned()), (at(200), "Français 20".to_owned())], "{index:?}, read first: {read_first}");
        }
    }

    #[test]
    fn a_file_whose_fragments_give_no_decode_time_is_read_again_from_the_first() {
        let mut bytes = fragmented(&each_second(30), true, Index::RandomAccess);
        // (Each decode time box made one to skip: a fragment's times go on from the one before.)
        while let Some(at) = bytes.windows(4).position(|kind| kind == b"tfdt") {
            bytes[at..at + 4].copy_from_slice(b"free");
        }
        let mut reader = reader(bytes);
        while reader.read_next().is_some() {}
        reader.seek(at(205));
        assert_eq!(next_samples(&mut reader, 2), [(at(200), "English 20".to_owned()), (at(200), "Français 20".to_owned())]);
    }

    #[test]
    fn a_movie_box_that_runs_to_the_files_end_is_read() {
        let mut bytes = files::movie(30, 10, false, &[], &TWO);
        let moov = bytes.windows(4).rposition(|kind| kind == b"moov").unwrap() - 4;
        bytes[moov..moov + 4].fill(0);
        let (tracks, captions) = read_all(bytes);
        assert_eq!(tracks.len(), 2);
        assert_eq!((texts_at(&captions, 0, 5), texts_at(&captions, 1, 15)), (vec!["English one".to_owned()], vec!["Français deux".to_owned()]));
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
