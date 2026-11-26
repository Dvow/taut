// Test videos made here, with no tool: frames that show their number, as lossless H.264 or
// JPEG in MP4 and as MJPEG in AVI, with or without sound tracks (silent, or a tone for each
// language) and caption tracks. Included by the unit tests of `src/video.rs` and by
// `tests/video_apple.rs`.
#[allow(unused_imports)]
use std::path::PathBuf;

/// Frames 64×48 whose number shows in eight cells, white for a one: 4 across, 2 down,
/// lowest bit at the top left. The background is grey.
pub fn frame_pixels(number: usize) -> Vec<u32> {
    let mut pixels = vec![0xFF80_8080u32; 64 * 48];
    for (at, pixel) in pixels.iter_mut().enumerate() {
        let (x, y) = (at % 64, at / 64);
        let cell = x / 16 + y / 24 * 4;
        let inner = (2..14).contains(&(x % 16)) && (3..21).contains(&(y % 24));
        if inner {
            *pixel = if number >> cell & 1 == 1 { 0xFFFF_FFFF } else { 0xFF00_0000 };
        }
    }
    pixels
}

/// The number a frame shows, read from pixels `width` wide of a picture `scale` times its
/// size, its corner at `(x, y)`.
pub fn number_in(pixels: &[u32], width: usize, (x, y): (usize, usize), scale: f32) -> usize {
    let bright = |p: u32| (p >> 16 & 255) + (p >> 8 & 255) + (p & 255) > 384;
    (0..8)
        .map(|cell| {
            let (cx, cy) = (x + ((cell % 4 * 16 + 8) as f32 * scale) as usize, y + ((cell / 4 * 24 + 12) as f32 * scale) as usize);
            (bright(pixels[cy * width + cx]) as usize) << cell
        })
        .sum()
}

pub fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}
pub fn atoms(kind: &[u8; 4], inside: &[Vec<u8>]) -> Vec<u8> {
    atom(kind, &inside.concat())
}
pub fn be32(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
pub const MATRIX: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

/// Bits written from the top of each byte, as H.264 reads them.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}
impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for at in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            *self.bytes.last_mut().unwrap() |= ((value >> at & 1) as u8) << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }
    /// Exp-Golomb, unsigned and signed.
    fn ue(&mut self, value: u32) {
        let long = 32 - (value + 1).leading_zeros();
        self.put(0, long - 1);
        self.put(value + 1, long);
    }
    fn se(&mut self, value: i32) {
        self.ue(if value > 0 { 2 * value as u32 - 1 } else { 2 * value.unsigned_abs() });
    }
    fn align(&mut self) {
        self.used = 0;
    }
    /// The stop bit and zeros to the byte's end.
    fn end(mut self) -> Vec<u8> {
        self.put(1, 1);
        self.bytes
    }
}

/// A NAL unit: its header byte, and its bytes with a 3 after any two zeros before a byte up
/// to 3.
pub fn nal(header: u8, raw: &[u8]) -> Vec<u8> {
    let (mut out, mut zeros) = (vec![header], 0);
    for &byte in raw {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// H.264's sequence and picture parameters for 64×48 baseline frames.
pub fn parameters() -> (Vec<u8>, Vec<u8>) {
    let mut sps = Bits::default();
    sps.put(66, 8);
    sps.put(0xC0, 8);
    sps.put(30, 8);
    for value in [0, 0, 2, 1] {
        sps.ue(value); // id, frame numbers of 4 bits, picture order from them, one reference
    }
    sps.put(0, 1);
    sps.ue(3); // 4 macroblocks across
    sps.ue(2); // 3 down
    sps.put(0b1100, 4); // frames only, 8×8 inference, no cropping, no VUI
    let mut pps = Bits::default();
    for value in [0, 0] {
        pps.ue(value);
    }
    pps.put(0, 2); // CAVLC, no field order
    for value in [0, 0, 0] {
        pps.ue(value);
    }
    pps.put(0, 3); // no weighted prediction
    for value in [0, 0, 0] {
        pps.se(value);
    }
    pps.put(0, 3); // no deblocking control, constrained intra or redundancy
    (nal(0x67, &sps.end()), nal(0x68, &pps.end()))
}

/// One frame as an IDR slice of I_PCM macroblocks: its samples as they are, no loss.
pub fn h264_frame(number: usize) -> Vec<u8> {
    let pixels = frame_pixels(number);
    let luma = |x: usize, y: usize| {
        let p = pixels[y * 64 + x];
        let full = (299 * (p >> 16 & 255) + 587 * (p >> 8 & 255) + 114 * (p & 255) + 500) / 1000;
        (16 + (full * 219 + 127) / 255) as u8
    };
    let mut slice = Bits::default();
    slice.ue(0); // first macroblock
    slice.ue(7); // all I
    slice.ue(0); // parameters 0
    slice.put(0, 4); // frame number
    slice.ue(number as u32 & 0xFFFF); // IDR id, another each frame
    slice.put(0, 2); // keep earlier pictures, no long-term reference
    slice.se(0);
    for at in 0..12 {
        let (mx, my) = (at % 4 * 16, at / 4 * 16);
        slice.ue(25);
        slice.align();
        slice.bytes.extend((0..256).map(|i| luma(mx + i % 16, my + i / 16)));
        slice.bytes.extend([128u8; 128]);
    }
    nal(0x65, &slice.end())
}

/// A sound track of a made file: its language (ISO 639-2, as MP4 keeps it), its name, and its
/// tone: a square wave of that many Hz, 8 kHz mono (0: silence).
#[derive(Clone, Copy)]
pub struct Sound<'a> {
    pub language: &'a str,
    pub name: &'a str,
    pub tone: u32,
}

/// A caption track of a made file: its language, its name, and its cues (from and to, in tenths
/// of a second, and the text). In MP4 as 3GPP timed text, in Matroska as SRT.
#[derive(Clone, Copy)]
pub struct Captions<'a> {
    pub language: &'a str,
    pub name: &'a str,
    pub cues: &'a [(u32, u32, &'a str)],
}

/// The sound tracks of `languages_file`: English at 400 Hz, German at 1000 Hz.
pub const TWO_LANGUAGES: [Sound; 2] = [Sound { language: "eng", name: "English", tone: 400 }, Sound { language: "deu", name: "Deutsch", tone: 1000 }];
/// English captions, each naming its second; none from 2 s to 2.5 s.
pub const ENGLISH: &[(u32, u32, &str)] = &[(0, 10, "English one"), (10, 20, "English two"), (25, 30, "English end")];
/// The same in French.
pub const FRENCH: &[(u32, u32, &str)] = &[(0, 10, "Français un"), (10, 20, "Français deux"), (25, 30, "Français fin")];
/// Its caption tracks: English and French.
pub const TWO_CAPTIONS: [Captions; 2] = [Captions { language: "eng", name: "English", cues: ENGLISH }, Captions { language: "fra", name: "Français", cues: FRENCH }];

/// `count` samples of a square wave of `hertz` at 8 kHz, a quarter of full scale.
pub fn tone(hertz: u32, count: usize) -> Vec<i16> {
    let high = |at: usize| (at as u64 * hertz as u64 * 2 / 8000).is_multiple_of(2);
    (0..count).map(|at| if hertz == 0 { 0 } else if high(at) { 8192 } else { -8192 }).collect()
}

/// The tone in samples: how many times a second they cross zero, halved (a square wave's Hz).
pub fn hertz_of(samples: &[f32], rate: u32) -> f32 {
    let crossings = samples.windows(2).filter(|two| (two[0] < 0.0) != (two[1] < 0.0)).count();
    crossings as f32 * rate as f32 / samples.len().max(1) as f32 / 2.0
}

/// What a track holds.
#[derive(Clone, Copy, PartialEq)]
enum Holds {
    Frames,
    Sound,
    Captions,
}

/// A track's header, media header and handler, as written around its sample entry.
struct Trak<'a> {
    id: u32,
    holds: Holds,
    /// Played unless another of its group is chosen; and its group of alternatives.
    enabled: bool,
    group: u16,
    language: &'a str,
    name: &'a str,
    timescale: u32,
    duration_ms: u32,
    media_duration: u32,
}

/// A language as MP4 packs it: three letters of five bits.
fn packed(language: &str) -> [u8; 2] {
    language.bytes().take(3).fold(0u16, |all, letter| all << 5 | (letter - 0x60) as u16).to_be_bytes()
}

impl Trak<'_> {
    fn atom(&self, entry: Vec<u8>, table: Vec<Vec<u8>>) -> Vec<u8> {
        let mut tkhd = be32(&[if self.enabled { 3 } else { 2 }, 0, 0, self.id, 0, self.duration_ms, 0, 0]);
        tkhd.extend_from_slice(&[0, 0]); // layer
        tkhd.extend_from_slice(&self.group.to_be_bytes());
        tkhd.extend_from_slice(&[if self.holds == Holds::Sound { 1 } else { 0 }, 0, 0, 0]); // volume, reserved
        tkhd.extend(be32(&MATRIX));
        tkhd.extend(be32(&if self.holds == Holds::Sound { [0, 0] } else { [64 << 16, 48 << 16] }));
        let mut mdhd = be32(&[0, 0, 0, self.timescale, self.media_duration]);
        mdhd.extend_from_slice(&packed(self.language));
        mdhd.extend_from_slice(&[0, 0]);
        let (kind, header) = match self.holds {
            Holds::Frames => (b"vide", atom(b"vmhd", &be32(&[1, 0, 0]))),
            Holds::Sound => (b"soun", atom(b"smhd", &be32(&[0, 0]))),
            Holds::Captions => (b"sbtl", atom(b"nmhd", &be32(&[0]))),
        };
        let mut hdlr = be32(&[0, 0]);
        hdlr.extend_from_slice(kind);
        hdlr.extend(be32(&[0, 0, 0]));
        hdlr.extend_from_slice(self.name.as_bytes());
        hdlr.push(0);
        let dinf = atoms(b"dinf", &[atom(b"dref", &[be32(&[0, 1]), atom(b"url ", &be32(&[1]))].concat())]);
        let stsd = atom(b"stsd", &[be32(&[0, 1]), entry].concat());
        let stbl = atoms(b"stbl", &[vec![stsd], table].concat());
        let minf = atoms(b"minf", &[header, dinf, stbl]);
        let mdia = atoms(b"mdia", &[atom(b"mdhd", &mdhd), atom(b"hdlr", &hdlr), minf]);
        let udta = atoms(b"udta", &[atom(b"name", self.name.as_bytes())]);
        atoms(b"trak", &[atom(b"tkhd", &tkhd), mdia, udta])
    }
}

/// A visual sample entry of 64×48 frames, of this kind, with what follows it.
fn visual_entry(kind: &[u8; 4], more: Vec<u8>) -> Vec<u8> {
    let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
    entry.extend(be32(&[0, 0, 0, 0]));
    entry.extend_from_slice(&[0, 64, 0, 48]);
    entry.extend(be32(&[0x48_0000, 0x48_0000, 0]));
    entry.extend_from_slice(&[0, 1]);
    entry.extend_from_slice(&[0; 32]);
    entry.extend_from_slice(&[0, 24, 0xFF, 0xFF]);
    entry.extend(more);
    atom(kind, &entry)
}

/// 3GPP timed text's sample entry: centred at the bottom, white on nothing, one font.
fn caption_entry() -> Vec<u8> {
    let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
    entry.extend(be32(&[0]));
    entry.extend_from_slice(&[1, 0xFF, 0, 0, 0, 0]); // centred, at the bottom; no background
    entry.extend_from_slice(&[0, 0, 0, 0, 0, 48, 0, 64]); // the text box
    entry.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 12, 255, 255, 255, 255]); // style: font 1, 12, white
    entry.extend(atom(b"ftab", &[&[0, 1, 0, 1, 5][..], b"Serif"].concat()));
    atom(b"tx3g", &entry)
}

/// A sample table of samples in chunks of their own at `offsets`, `durations` long.
fn table(sizes: &[u32], durations: &[u32], offsets: &[u32]) -> Vec<Vec<u8>> {
    let count = sizes.len() as u32;
    let times: Vec<u32> = durations.iter().flat_map(|&duration| [1, duration]).collect();
    vec![
        atom(b"stts", &[be32(&[0, count]), be32(&times)].concat()),
        atom(b"stsc", &be32(&[0, 1, 1, 1, 1])),
        atom(b"stsz", &[be32(&[0, 0, count]), be32(sizes)].concat()),
        atom(b"stco", &[be32(&[0, count]), be32(offsets)].concat()),
    ]
}

/// A caption track's samples, in ms: each cue, and empty ones between, to `length` (each its
/// text's length, then the text).
fn caption_samples(cues: &[(u32, u32, &str)], length: u32) -> Vec<(u32, Vec<u8>)> {
    let (mut samples, mut at) = (Vec::new(), 0);
    let text = |words: &str| [(words.len() as u16).to_be_bytes().to_vec(), words.as_bytes().to_vec()].concat();
    for &(from, to, words) in cues {
        if from * 100 > at {
            samples.push((from * 100 - at, text("")));
        }
        samples.push((to * 100 - from * 100, text(words)));
        at = to * 100;
    }
    if at < length {
        samples.push((length - at, text("")));
    }
    samples
}

/// An MP4 file of `count` frames at `rate` a second (64×48: lossless H.264, or JPEG where
/// `jpeg`), with these sound and caption tracks, each as long. The first sound track plays
/// unless another is chosen; sound tracks are one group of alternatives, caption tracks
/// another.
pub fn movie(count: usize, rate: u32, jpeg: bool, sounds: &[Sound], captions: &[Captions]) -> Vec<u8> {
    let duration_ms = count as u32 * 1000 / rate;
    let samples = count as u32 * 8000 / rate;
    let ftyp = atom(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
    let (mut mdat, mut traks) = (Vec::new(), Vec::new());
    let start = ftyp.len() as u32 + 8;
    let mut put = |bytes: &[u8]| {
        mdat.extend_from_slice(bytes);
        start + (mdat.len() - bytes.len()) as u32
    };
    let frames: Vec<Vec<u8>> = match jpeg {
        true => (0..count).map(|n| taut::jpeg::encode(64, &frame_pixels(n), 90).unwrap()).collect(),
        false => (0..count).map(h264_frame).map(|f| [(f.len() as u32).to_be_bytes().to_vec(), f].concat()).collect(),
    };
    let offsets: Vec<u32> = frames.iter().map(|frame| put(frame)).collect();
    let sizes: Vec<u32> = frames.iter().map(|frame| frame.len() as u32).collect();
    let entry = match jpeg {
        true => visual_entry(b"jpeg", Vec::new()),
        false => {
            let (sps, pps) = parameters();
            let avcc = [&[1, 66, 0xC0, 30, 0xFF, 0xE1][..], &(sps.len() as u16).to_be_bytes(), &sps, &[1], &(pps.len() as u16).to_be_bytes(), &pps].concat();
            visual_entry(b"avc1", atom(b"avcC", &avcc))
        }
    };
    let video = Trak { id: 1, holds: Holds::Frames, enabled: true, group: 0, language: "und", name: "Video", timescale: rate, duration_ms, media_duration: count as u32 };
    traks.push(video.atom(entry, table(&sizes, &vec![1; count], &offsets)));
    for (nth, sound) in sounds.iter().enumerate() {
        let bytes: Vec<u8> = tone(sound.tone, samples as usize).iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let at = put(&bytes);
        let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
        entry.extend(be32(&[0, 0]));
        entry.extend_from_slice(&[0, 1, 0, 16, 0, 0, 0, 0]);
        entry.extend(be32(&[8000 << 16]));
        // (In chunks of a frame's time: a player switching tracks waits for the next chunk.)
        let per_chunk = 8000 / rate;
        let chunks: Vec<u32> = (0..samples / per_chunk).map(|chunk| at + chunk * per_chunk * 2).collect();
        let table = vec![
            atom(b"stts", &be32(&[0, 1, samples, 1])),
            atom(b"stsc", &be32(&[0, 1, 1, per_chunk, 1])),
            atom(b"stsz", &be32(&[0, 2, samples])),
            atom(b"stco", &[be32(&[0, chunks.len() as u32]), be32(&chunks)].concat()),
        ];
        let trak = Trak { id: traks.len() as u32 + 1, holds: Holds::Sound, enabled: nth == 0, group: 1, language: sound.language, name: sound.name, timescale: 8000, media_duration: samples, ..video };
        traks.push(trak.atom(atom(b"sowt", &entry), table));
    }
    for captions in captions {
        let samples = caption_samples(captions.cues, duration_ms);
        let offsets: Vec<u32> = samples.iter().map(|(_, bytes)| put(bytes)).collect();
        let sizes: Vec<u32> = samples.iter().map(|(_, bytes)| bytes.len() as u32).collect();
        let durations: Vec<u32> = samples.iter().map(|(duration, _)| *duration).collect();
        let trak = Trak { id: traks.len() as u32 + 1, holds: Holds::Captions, enabled: false, group: 2, language: captions.language, name: captions.name, timescale: 1000, media_duration: duration_ms, ..video };
        traks.push(trak.atom(caption_entry(), table(&sizes, &durations, &offsets)));
    }
    let mut mvhd = be32(&[0, 0, 0, 1000, duration_ms, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, traks.len() as u32 + 1]));
    let moov = atoms(b"moov", &[vec![atom(b"mvhd", &mvhd)], traks].concat());
    [ftyp, atom(b"mdat", &mdat), moov].concat()
}

/// An MP4 file of `count` frames at `rate` a second (H.264, 64×48, lossless), with a
/// silent 8 kHz mono track as long where `sound`.
pub fn mp4(count: usize, rate: u32, sound: bool) -> Vec<u8> {
    let silent = [Sound { language: "und", name: "Sound", tone: 0 }];
    movie(count, rate, false, if sound { &silent } else { &[] }, &[])
}

/// An EBML element: its id, its size in eight bytes, its body.
pub fn element(id: u32, body: &[u8]) -> Vec<u8> {
    let id = id.to_be_bytes();
    let skip = id.iter().take_while(|byte| **byte == 0).count();
    [&id[skip..], &(1u64 << 56 | body.len() as u64).to_be_bytes(), body].concat()
}
pub fn number(id: u32, value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}
pub fn float(id: u32, value: f64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}
pub fn words(id: u32, text: &str) -> Vec<u8> {
    element(id, text.as_bytes())
}

/// A Matroska block of track `track`, `at` ms after its cluster's time: a key frame (simple
/// block), or with a duration (a block group).
fn block(track: u8, at: u32, bytes: &[u8], duration: Option<u32>) -> Vec<u8> {
    let head = [&[0x80 | track][..], &(at as i16).to_be_bytes(), &[if duration.is_none() { 0x80 } else { 0 }]].concat();
    match duration {
        None => element(0xA3, &[head, bytes.to_vec()].concat()),
        Some(duration) => element(0xA0, &[element(0xA1, &[head, bytes.to_vec()].concat()), number(0x9B, duration as u64)].concat()),
    }
}

/// A Matroska file of `count` frames at `rate` a second (lossless H.264, 64×48), with these
/// sound tracks (16-bit PCM) and caption tracks (SRT), each as long.
pub fn matroska(count: usize, rate: u32, sounds: &[Sound], captions: &[Captions]) -> Vec<u8> {
    let header = element(0x1A45_DFA3, &[number(0x4286, 1), number(0x42F7, 1), number(0x42F2, 4), number(0x42F3, 8), words(0x4282, "matroska"), number(0x4287, 4), number(0x4285, 2)].concat());
    let (frame_ms, samples_per_frame) = (1000 / rate, 8000 / rate as usize);
    let duration_ms = count as u32 * frame_ms;
    let info = element(0x1549_A966, &[number(0x2A_D7B1, 1_000_000), float(0x4489, duration_ms as f64), words(0x4D80, "taut tests"), words(0x5741, "taut tests")].concat());
    let (sps, pps) = parameters();
    let avcc = [&[1, 66, 0xC0, 30, 0xFF, 0xE1][..], &(sps.len() as u16).to_be_bytes(), &sps, &[1], &(pps.len() as u16).to_be_bytes(), &pps].concat();
    let entry = |track: u8, kind: u64, codec: &str, language: &str, name: &str, default: bool, more: Vec<u8>| {
        let common = [number(0xD7, track as u64), number(0x73C5, track as u64), number(0x83, kind), number(0x88, default as u64), number(0x9C, 0), words(0x22_B59C, language), words(0x536E, name), words(0x86, codec)];
        element(0xAE, &[common.concat(), more].concat())
    };
    let mut tracks = vec![entry(1, 1, "V_MPEG4/ISO/AVC", "und", "Video", true, [element(0x63A2, &avcc), number(0x23_E383, frame_ms as u64 * 1_000_000), element(0xE0, &[number(0xB0, 64), number(0xBA, 48)].concat())].concat())];
    for (nth, sound) in sounds.iter().enumerate() {
        let audio = element(0xE1, &[float(0xB5, 8000.0), number(0x9F, 1), number(0x6264, 16)].concat());
        tracks.push(entry(2 + nth as u8, 2, "A_PCM/INT/LIT", sound.language, sound.name, nth == 0, audio));
    }
    let first_caption = 2 + sounds.len() as u8;
    for (nth, captions) in captions.iter().enumerate() {
        tracks.push(entry(first_caption + nth as u8, 0x11, "S_TEXT/UTF8", captions.language, captions.name, false, Vec::new()));
    }
    let tones: Vec<Vec<i16>> = sounds.iter().map(|sound| tone(sound.tone, count * samples_per_frame)).collect();
    // (A seek head first, where a player looks for the cues: its positions, from the segment's
    // start, are known once the rest is made; its own size is the same whatever they are.)
    let seek = |id: u32, at: u64| element(0x4DBB, &[element(0x53AB, &id.to_be_bytes()), number(0x53AC, at)].concat());
    let head = |at: [u64; 3]| element(0x114D_9B74, &[seek(0x1549_A966, at[0]), seek(0x1654_AE6B, at[1]), seek(0x1C53_BB6B, at[2])].concat());
    let skipped = head([0; 3]).len() as u64;
    let tracks_at = skipped + info.len() as u64;
    let mut body = [info, element(0x1654_AE6B, &tracks.concat())].concat();
    // (A cluster a second, as muxers write them, each found by a cue point.)
    let mut cues = Vec::new();
    for first in (0..count).step_by(rate as usize) {
        let start = first as u32 * frame_ms;
        let mut blocks = vec![number(0xE7, start as u64)];
        for frame in first..(first + rate as usize).min(count) {
            let at = frame as u32 * frame_ms - start;
            let h264 = h264_frame(frame);
            blocks.push(block(1, at, &[(h264.len() as u32).to_be_bytes().to_vec(), h264].concat(), None));
            for (nth, samples) in tones.iter().enumerate() {
                let bytes: Vec<u8> = samples[frame * samples_per_frame..][..samples_per_frame].iter().flat_map(|sample| sample.to_le_bytes()).collect();
                blocks.push(block(2 + nth as u8, at, &bytes, None));
            }
            for (nth, captions) in captions.iter().enumerate() {
                let starting = captions.cues.iter().filter(|cue| cue.0 * 100 == at + start);
                blocks.extend(starting.map(|&(from, to, text)| block(first_caption + nth as u8, at, text.as_bytes(), Some((to - from) * 100))));
            }
        }
        let point = [number(0xB3, start as u64), element(0xB7, &[number(0xF7, 1), number(0xF1, skipped + body.len() as u64)].concat())].concat();
        cues.push(element(0xBB, &point));
        body.extend(element(0x1F43_B675, &blocks.concat()));
    }
    let cues_at = skipped + body.len() as u64;
    body.extend(element(0x1C53_BB6B, &cues.concat()));
    [header, element(0x1853_8067, &[head([skipped, tracks_at, cues_at]), body].concat())].concat()
}

pub fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}
pub fn list(kind: &[u8; 4], inside: &[Vec<u8>]) -> Vec<u8> {
    chunk(b"LIST", &[kind.to_vec(), inside.concat()].concat())
}
pub fn le32(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// An AVI file of `count` frames at `rate` a second (MJPEG, 64×48), with a silent 8 kHz
/// mono track as long where `sound`.
pub fn avi(count: usize, rate: u32, sound: bool) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = (0..count).map(|n| taut::jpeg::encode(64, &frame_pixels(n), 90).unwrap()).collect();
    let biggest = frames.iter().map(Vec::len).max().unwrap_or(0) as u32;
    let samples = count as u32 * 8000 / rate;
    let streams = 1 + sound as u32;
    let avih = le32(&[1_000_000 / rate, 0, 0, 0x10, count as u32, 0, streams, biggest, 64, 48, 0, 0, 0, 0]);
    let mut strh = b"vidsMJPG".to_vec();
    strh.extend(le32(&[0, 0, 0, 1, rate, 0, count as u32, biggest, u32::MAX, 0]));
    strh.extend_from_slice(&[0, 0, 0, 0, 64, 0, 48, 0]);
    let mut strf = le32(&[40, 64, 48]);
    strf.extend_from_slice(&[1, 0, 24, 0]);
    strf.extend_from_slice(b"MJPG");
    strf.extend(le32(&[64 * 48 * 3, 0, 0, 0, 0]));
    let mut headers = vec![chunk(b"avih", &avih), list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)])];
    if sound {
        let mut strh = b"auds".to_vec();
        strh.extend(le32(&[0, 0, 0, 0, 2, 16000, 0, samples, 16000, u32::MAX, 2]));
        strh.extend_from_slice(&[0; 8]);
        let strf = [&[1u8, 0, 1, 0][..], &le32(&[8000, 16000]), &[2, 0, 16, 0, 0, 0]].concat();
        headers.push(list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)]));
    }
    let mut movi = Vec::new();
    let mut index = Vec::new();
    for frame in &frames {
        index.extend_from_slice(b"00dc");
        index.extend(le32(&[0x10, movi.len() as u32 + 4, frame.len() as u32]));
        movi.extend(chunk(b"00dc", frame));
    }
    if sound {
        index.extend_from_slice(b"01wb");
        index.extend(le32(&[0x10, movi.len() as u32 + 4, samples * 2]));
        movi.extend(chunk(b"01wb", &vec![0; samples as usize * 2]));
    }
    let body = [b"AVI ".to_vec(), list(b"hdrl", &headers), list(b"movi", &[movi]), chunk(b"idx1", &index)].concat();
    chunk(b"RIFF", &body)
}

/// The file in a folder of its own, named so.
pub fn written(name: &str, bytes: &[u8]) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("taut-video-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}
