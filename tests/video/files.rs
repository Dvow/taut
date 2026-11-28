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

/// A pixel's luma, in video range.
fn luma(pixel: u32) -> u8 {
    let full = (299 * (pixel >> 16 & 255) + 587 * (pixel >> 8 & 255) + 114 * (pixel & 255) + 500) / 1000;
    (16 + (full * 219 + 127) / 255) as u8
}

/// One frame as an IDR slice of I_PCM macroblocks: its samples as they are, no loss.
pub fn h264_frame(number: usize) -> Vec<u8> {
    let pixels = frame_pixels(number);
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
        slice.bytes.extend((0..256).map(|i| luma(pixels[(my + i / 16) * 64 + mx + i % 16])));
        slice.bytes.extend([128u8; 128]);
    }
    nal(0x65, &slice.end())
}

/// An HEVC NAL unit: its two header bytes (its type; layer 0, temporal id 0) and its bytes
/// with a 3 after any two zeros before a byte up to 3.
pub fn hevc_nal(kind: u8, raw: &[u8]) -> Vec<u8> {
    let mut unit = nal(kind << 1, raw);
    unit.insert(1, 1);
    unit
}

/// HEVC's general profile, tier and level: Main, level 1, progressive frames.
fn hevc_profile(bits: &mut Bits) {
    bits.put(1, 8); // profile space 0, main tier, Main
    bits.put(0x6000_0000, 32); // compatible with Main and Main 10
    bits.put(0b1001, 4); // progressive, frames only
    bits.put(0, 32); // reserved: 44 bits
    bits.put(0, 12);
    bits.put(30, 8);
}

/// HEVC's video, sequence and picture parameters for 64×48 frames: coding blocks of 16 that
/// may be PCM, no loop filters.
pub fn hevc_parameters() -> [Vec<u8>; 3] {
    let mut vps = Bits::default();
    vps.put(0b00_0011, 6); // id 0, base layer internal and available
    vps.put(0, 6); // one layer
    vps.put(0b0001, 4); // one sub-layer, temporal ids nested
    vps.put(0xFFFF, 16);
    hevc_profile(&mut vps);
    vps.put(1, 1);
    for value in [0, 0, 0] {
        vps.ue(value); // one picture buffered, none reordered
    }
    vps.put(0, 6); // highest layer id
    vps.ue(0); // one layer set
    vps.put(0, 2); // no timing, no extension
    let mut sps = Bits::default();
    sps.put(0b0000_0001, 8); // parameters 0, one sub-layer, nested
    hevc_profile(&mut sps);
    for value in [0, 1, 64, 48] {
        sps.ue(value); // id, 4:2:0, 64×48
    }
    sps.put(0, 1); // no cropping
    for value in [0, 0, 4] {
        sps.ue(value); // 8 bits, picture order in 8 bits
    }
    sps.put(1, 1);
    for value in [0, 0, 0, 1, 0, 0, 2, 0, 0] {
        sps.ue(value); // one picture buffered; coding blocks of 16 only, transforms 4 to 16
    }
    sps.put(0b0001, 4); // no scaling lists, asymmetric parts or offsets; PCM
    sps.put(0x77, 8); // PCM samples of 8 bits
    sps.ue(1); // PCM blocks of 16
    sps.ue(0);
    sps.put(1, 1); // no loop filter on PCM
    sps.ue(0); // no reference picture sets
    sps.put(0, 5); // no long-term references, motion vector prediction, smoothing, VUI, extension
    let mut pps = Bits::default();
    pps.ue(0);
    pps.ue(0);
    pps.put(0, 7); // no dependent slices, output flags, extra bits, sign hiding, CABAC init
    for value in [0, 0] {
        pps.ue(value);
    }
    pps.se(0);
    pps.put(0, 3); // no constrained intra, transform skip, QP deltas
    pps.se(0);
    pps.se(0);
    pps.put(0, 7); // no slice offsets, weighted prediction, bypass, tiles, rows, filtering across
    pps.put(0b101, 3); // deblocking control: not overridden, off
    pps.put(0, 2); // no scaling lists, list changes
    pps.ue(0);
    pps.put(0, 2); // no extensions
    [hevc_nal(32, &vps.end()), hevc_nal(33, &sps.end()), hevc_nal(34, &pps.end())]
}

/// HEVC's arithmetic coder, as far as these frames need: bins of the most probable value, and
/// terminating bins.
struct Cabac {
    bits: Bits,
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
}

impl Cabac {
    fn new(bits: Bits) -> Cabac {
        Cabac { bits, low: 0, range: 510, outstanding: 0, first: true }
    }
    /// Starts afresh (after PCM samples).
    fn restart(&mut self) {
        (self.low, self.range, self.outstanding, self.first) = (0, 510, 0, true);
    }
    fn put_bit(&mut self, bit: u32) {
        if !std::mem::take(&mut self.first) {
            self.bits.put(bit, 1);
        }
        for _ in 0..std::mem::take(&mut self.outstanding) {
            self.bits.put(1 - bit, 1);
        }
    }
    fn renormalize(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            (self.range, self.low) = (self.range << 1, self.low << 1);
        }
    }
    /// A bin of the most probable value in a context in this state (from 0, moving on).
    fn most_probable(&mut self, state: &mut usize) {
        const LEAST_PROBABLE: [[u32; 4]; 12] = [
            [128, 176, 208, 240],
            [128, 167, 197, 227],
            [128, 158, 187, 216],
            [123, 150, 178, 205],
            [116, 142, 169, 195],
            [111, 135, 160, 185],
            [105, 128, 152, 175],
            [100, 122, 144, 166],
            [95, 116, 137, 158],
            [90, 110, 130, 150],
            [85, 104, 123, 142],
            [81, 99, 117, 135],
        ];
        self.range -= LEAST_PROBABLE[*state][(self.range >> 6 & 3) as usize];
        *state += 1;
        self.renormalize();
    }
    /// A terminating bin: 1 flushes the coder, its last bit a 1 (the stop bit at a slice's end).
    fn terminate(&mut self, one: bool) {
        self.range -= 2;
        if !one {
            return self.renormalize();
        }
        self.low += self.range;
        self.range = 2;
        self.renormalize();
        self.put_bit(self.low >> 9 & 1);
        self.bits.put((self.low >> 7 & 3) | 1, 2);
    }
}

/// One frame as an HEVC IDR slice of PCM coding blocks: its samples as they are, no loss.
pub fn hevc_frame(number: usize) -> Vec<u8> {
    let pixels = frame_pixels(number);
    let mut slice = Bits::default();
    slice.put(0b10, 2); // the first slice, earlier pictures kept
    slice.ue(0); // parameters 0
    slice.ue(2); // I
    slice.se(0); // QP 26
    slice.put(1, 1);
    slice.align();
    // (Each block of 16: its part mode 2N×2N, a context starting at its most probable value
    // 1; PCM; then the slice's end, or not.)
    let (mut cabac, mut part_mode) = (Cabac::new(slice), 0);
    for at in 0..12 {
        let (x, y) = (at % 4 * 16, at / 4 * 16);
        cabac.most_probable(&mut part_mode);
        cabac.terminate(true);
        cabac.bits.align();
        cabac.bits.bytes.extend((0..256).map(|i| luma(pixels[(y + i / 16) * 64 + x + i % 16])));
        cabac.bits.bytes.extend([128u8; 128]);
        cabac.restart();
        cabac.terminate(at == 11);
    }
    hevc_nal(19, &cabac.bits.bytes)
}

/// HEVC's decoder configuration (`hvcC`): Main, level 1, 4:2:0 in 8 bits, lengths in 4
/// bytes, and the parameters.
fn hvcc() -> Vec<u8> {
    let mut config = vec![1, 0x01];
    config.extend(be32(&[0x6000_0000]));
    config.extend([0x90, 0, 0, 0, 0, 0, 30, 0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 3]);
    for unit in hevc_parameters() {
        config.extend([0x80 | unit[0] >> 1, 0, 1]);
        config.extend((unit.len() as u16).to_be_bytes());
        config.extend(unit);
    }
    config
}


/// A CEA-608 byte with odd parity, as the stream carries it.
pub fn odd(byte: u8) -> u8 {
    byte & 0x7F | if (byte & 0x7F).count_ones().is_multiple_of(2) { 0x80 } else { 0 }
}

/// CEA-608 byte pairs for caption channel 1 shown by these cues (from and to in tenths of a
/// second, and their text), three pairs a frame at 10 frames a second: each cue loaded off
/// screen and shown at its start (pop-on: resume loading, erase what is off screen, row 15,
/// the text, end of caption), what shows erased at its end; each command twice. Null pairs
/// between.
pub fn cea608_pairs(cues: &[(u32, u32, &str)], count: usize) -> Vec<[u8; 3 * 2]> {
    cea608_pairs_of(cues, count, 0x14)
}

/// The same for the caption channel whose commands start with `first`: 0x14 for CC1, 0x1C for
/// CC2 (field 1); 0x15 for CC3, 0x1D for CC4 (field 2).
pub fn cea608_pairs_of(cues: &[(u32, u32, &str)], count: usize, first: u8) -> Vec<[u8; 3 * 2]> {
    let mut pairs = vec![[0x80u8, 0x80]; count * 3];
    let command = |second: u8| [[odd(first), odd(second)]; 2];
    let row_15 = [[odd(0x14 | (first & 0x08)), odd(0x70)]; 2];
    for &(from, to, text) in cues {
        let mut loading: Vec<[u8; 2]> = [command(0x20), command(0x2E), row_15].concat();
        let letters: Vec<u8> = text.bytes().map(odd).collect();
        loading.extend(letters.chunks(2).map(|two| [two[0], two.get(1).copied().unwrap_or(0x80)]));
        loading.extend(command(0x2F));
        let first = (from as usize * 3 + 2).saturating_sub(loading.len() - 1);
        pairs[first..first + loading.len()].copy_from_slice(&loading);
        let end = to as usize * 3;
        if end + 2 <= pairs.len() {
            pairs[end..end + 2].copy_from_slice(&command(0x2C));
        }
    }
    pairs.chunks(3).map(|three| [three[0][0], three[0][1], three[1][0], three[1][1], three[2][0], three[2][1]]).collect()
}

/// An SEI NAL unit of ATSC A/53 closed captions (`GA94`) carrying these `cc_data` triplets
/// (at most 31): H.264's, or HEVC's prefix SEI.
pub fn caption_sei(triplets: &[u8], hevc: bool) -> Vec<u8> {
    let mut payload = vec![0xB5, 0x00, 0x31, b'G', b'A', b'9', b'4', 0x03, 0x40 | (triplets.len() / 3) as u8, 0xFF];
    payload.extend_from_slice(triplets);
    payload.push(0xFF);
    let mut raw = vec![4, payload.len() as u8];
    raw.extend(payload);
    raw.push(0x80);
    if hevc { hevc_nal(39, &raw) } else { nal(0x06, &raw) }
}

/// Captions in the video stream of a made file (from and to in tenths of a second, and the
/// text, at 10 frames a second): CEA-608's channel 1, its other channels by the first byte of
/// their commands (0x1C: CC2; 0x15: CC3, 0x1D: CC4, in field 2), and CEA-708 services by their
/// number.
#[derive(Clone, Copy)]
pub struct InStream<'a> {
    pub cea608: Cues<'a>,
    pub other_channels: &'a [(u8, Cues<'a>)],
    pub services: &'a [(u8, Cues<'a>)],
}

/// Cues: from and to in tenths of a second, and the text.
pub type Cues<'a> = &'a [(u32, u32, &'a str)];

/// Captions in the video stream of the in-stream test files: CEA-608's from 0.5 s to 1.2 s and
/// from 1.8 s to 2.6 s, and CEA-708 service 1's from 0.8 s to 2.2 s.
pub const IN_STREAM: InStream = InStream { cea608: &[(5, 12, "Hello there"), (18, 26, "Second line")], other_channels: &[], services: &[(1, &[(8, 22, "Service one")])] };

/// Captions in the other CEA-608 channels: CC2 from 0.5 s to 1.2 s, CC3 from 0.8 s to 1.5 s,
/// CC4 from 2 s to 2.8 s.
pub const OTHER_CHANNELS: InStream = InStream { cea608: &[], other_channels: &[(0x1C, &[(5, 12, "Second channel")]), (0x15, &[(8, 15, "Third channel")]), (0x1D, &[(20, 28, "Fourth channel")])], services: &[] };

/// What each in-stream track (`CC1`, `Service 1`) shows at times through `IN_STREAM`'s file,
/// each well within a cue or a gap.
pub const IN_STREAM_SHOWN: [(f32, &str, Option<&str>); 6] = [
    (0.7, "CC1", Some("Hello there")),
    (0.95, "Service 1", Some("Service one")),
    (1.5, "CC1", None),
    (1.55, "Service 1", Some("Service one")),
    (2.05, "CC1", Some("Second line")),
    (2.45, "Service 1", None),
];

/// Each frame's CEA-608 pairs of field 1 and of field 2: channel 1's and the other channels',
/// none of them sending in the same place.
pub fn cea608_fields(in_stream: &InStream, count: usize) -> Vec<[[u8; 3 * 2]; 2]> {
    let mut fields = vec![[[0x80; 6]; 2]; count];
    let channels = std::iter::once((0x14, in_stream.cea608)).chain(in_stream.other_channels.iter().copied());
    for (first, cues) in channels {
        let field = usize::from(first & 1 == 1);
        for (frame, pairs) in cea608_pairs_of(cues, count, first).into_iter().enumerate() {
            for (at, pair) in pairs.chunks(2).enumerate().filter(|(_, pair)| *pair != [0x80, 0x80]) {
                assert_eq!(fields[frame][field][at * 2..at * 2 + 2], [0x80, 0x80], "two channels at frame {frame}");
                fields[frame][field][at * 2..at * 2 + 2].copy_from_slice(pair);
            }
        }
    }
    fields
}

/// The `cc_data` triplets of each of `count` frames: three CEA-608 pairs of field 1 and three
/// of field 2 (see `cea608_fields`), then the CEA-708 packets due (see `cea708_packets`), ten
/// pairs a frame at most.
pub fn cc_data(in_stream: &InStream, count: usize) -> Vec<Vec<u8>> {
    let fields = cea608_fields(in_stream, count);
    let (mut waiting, mut next) = (std::collections::VecDeque::new(), cea708_packets(in_stream.services).into_iter().peekable());
    let mut frames = Vec::new();
    for (frame, [one, two]) in fields.iter().enumerate() {
        let mut triplets: Vec<u8> = one.chunks(2).flat_map(|pair| [0xFC, pair[0], pair[1]]).collect();
        triplets.extend(two.chunks(2).flat_map(|pair| [0xFD, pair[0], pair[1]]));
        while let Some((_, packet)) = next.next_if(|(due, _)| *due <= frame) {
            waiting.extend(packet.chunks(2).enumerate().map(|(nth, pair)| [if nth == 0 { 0xFF } else { 0xFE }, pair[0], pair[1]]));
        }
        triplets.extend(waiting.drain(..waiting.len().min(10)).flatten());
        frames.push(triplets);
    }
    frames
}

/// CEA-708 packets for these services' cues, each with the frame it is due at: a window
/// defined hidden and written three frames before the cue (window 0 of one row, 32 columns,
/// near the bottom), shown at its start, deleted at its end.
pub fn cea708_packets(services: &[(u8, Cues)]) -> Vec<(usize, Vec<u8>)> {
    let mut blocks = Vec::new();
    for &(number, cues) in services {
        for &(from, to, text) in cues {
            let define = [0x98, 0x00, 0x80 | 90, 50, 0x70, 31, 0x09];
            blocks.push(((from as usize).saturating_sub(3), number, [&define[..], text.as_bytes()].concat()));
            blocks.push((from as usize, number, vec![0x89, 0x01]));
            blocks.push((to as usize, number, vec![0x8C, 0x01]));
        }
    }
    blocks.sort_by_key(|(frame, _, _)| *frame);
    blocks.into_iter().zip(0..).map(|((frame, number, block), sequence)| (frame, cea708_packet(sequence, number, &block))).collect()
}

/// A DTVCC packet of one service block: its header (sequence number, size in pairs), the
/// block's header (with the extended number from service 7), its bytes (31 at most), padded
/// to whole pairs.
pub fn cea708_packet(sequence: u8, service: u8, block: &[u8]) -> Vec<u8> {
    assert!(block.len() < 32, "a service block of {} bytes", block.len());
    let header = match service {
        0..=6 => vec![service << 5 | block.len() as u8],
        _ => vec![7 << 5 | block.len() as u8, service],
    };
    let mut packet = [&[0][..], &header, block].concat();
    if packet.len() % 2 == 1 {
        packet.push(0);
    }
    packet[0] = (sequence & 3) << 6 | (packet.len() / 2 % 64) as u8;
    packet
}

/// A sound track of a made file: its language (ISO 639-2, as MP4 keeps it), its name, and its
/// tone: a square wave of that many Hz, 8 kHz mono (0: silence).
#[derive(Clone, Copy)]
pub struct Sound<'a> {
    pub language: &'a str,
    pub name: &'a str,
    pub tone: u32,
}

/// A caption track of a made file: its language, its name, its cues (from and to, in tenths of a
/// second, and the text) and how they are kept.
#[derive(Clone, Copy)]
pub struct Captions<'a> {
    pub language: &'a str,
    pub name: &'a str,
    pub cues: &'a [(u32, u32, &'a str)],
    pub format: CaptionFormat,
}

impl Captions<'_> {
    /// What its track holds in an MP4 file.
    fn holds(&self) -> Holds {
        if self.format == CaptionFormat::Placed { Holds::WebVtt } else { Holds::Captions }
    }
}

/// How a made file keeps a caption track.
#[derive(Clone, Copy, PartialEq)]
pub enum CaptionFormat {
    /// Text at the usual place: in MP4 3GPP timed text, in Matroska SRT.
    Plain,
    /// Each cue placed at the top left, a tenth of the frame in: in MP4 WebVTT (`align:start
    /// position:10% line:10%`), in Matroska SSA (`{\an7\pos(64,48)}` on a script of 640 by 480).
    Placed,
}

/// The SSA script header of a made Matroska file's placed captions: 640 by 480, one style.
pub const SSA_HEADER: &str = "[Script Info]\r\nScriptType: v4.00+\r\nPlayResX: 640\r\nPlayResY: 480\r\n\r\n[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\r\n\r\n[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n";

/// The sound tracks of `languages_file`: English at 400 Hz, German at 1000 Hz.
pub const TWO_LANGUAGES: [Sound; 2] = [Sound { language: "eng", name: "English", tone: 400 }, Sound { language: "deu", name: "Deutsch", tone: 1000 }];
/// English captions, each naming its second; none from 2 s to 2.5 s.
pub const ENGLISH: &[(u32, u32, &str)] = &[(0, 10, "English one"), (10, 20, "English two"), (25, 30, "English end")];
/// The same in French.
pub const FRENCH: &[(u32, u32, &str)] = &[(0, 10, "Français un"), (10, 20, "Français deux"), (25, 30, "Français fin")];
/// Its caption tracks: English and French.
pub const TWO_CAPTIONS: [Captions; 2] = [Captions { language: "eng", name: "English", cues: ENGLISH, format: CaptionFormat::Plain }, Captions { language: "fra", name: "Français", cues: FRENCH, format: CaptionFormat::Plain }];
/// Caption tracks one plain, one placed: English as text at the usual place, French at the top
/// left (see `CaptionFormat`).
pub const PLAIN_AND_PLACED: [Captions; 2] = [Captions { language: "eng", name: "English", cues: ENGLISH, format: CaptionFormat::Plain }, Captions { language: "fra", name: "", cues: FRENCH, format: CaptionFormat::Placed }];
/// The subtitle renditions of the test stream (see `hls_stream`): English at the usual place, French
/// placed at the top left.
pub const RENDITIONS: [Captions; 2] = [Captions { language: "eng", name: "English", cues: ENGLISH, format: CaptionFormat::Plain }, Captions { language: "fra", name: "Français", cues: FRENCH, format: CaptionFormat::Placed }];

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
    /// WebVTT, under the handler `text` (as ffmpeg writes it; AVFoundation refuses a file with it
    /// under `sbtl`).
    WebVtt,
    ClosedCaptions,
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
    /// Its frames' width and height.
    size: [u32; 2],
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
        tkhd.extend(be32(&if self.holds == Holds::Sound { [0, 0] } else { self.size.map(|side| side << 16) }));
        let mut mdhd = be32(&[0, 0, 0, self.timescale, self.media_duration]);
        mdhd.extend_from_slice(&packed(self.language));
        mdhd.extend_from_slice(&[0, 0]);
        let (kind, header) = match self.holds {
            Holds::Frames => (b"vide", atom(b"vmhd", &be32(&[1, 0, 0]))),
            Holds::Sound => (b"soun", atom(b"smhd", &be32(&[0, 0]))),
            Holds::Captions => (b"sbtl", atom(b"nmhd", &be32(&[0]))),
            Holds::WebVtt => (b"text", atom(b"nmhd", &be32(&[0]))),
            // (QuickTime's base media header, its graphics mode copy.)
            Holds::ClosedCaptions => (b"clcp", atoms(b"gmhd", &[atom(b"gmin", &[0, 0, 0, 0, 0, 0x40, 0x80, 0, 0x80, 0, 0x80, 0, 0, 0, 0, 0])])),
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

/// The sample entry of 64×48 lossless H.264 frames: `avc1` with its parameters (`avcC`).
fn avc1_entry() -> Vec<u8> {
    let (sps, pps) = parameters();
    let avcc = [&[1, 66, 0xC0, 30, 0xFF, 0xE1][..], &(sps.len() as u16).to_be_bytes(), &sps, &[1], &(pps.len() as u16).to_be_bytes(), &pps].concat();
    visual_entry(b"avc1", atom(b"avcC", &avcc))
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

/// A caption track's sample entry: 3GPP timed text centred at the bottom, white on nothing,
/// one font; or WebVTT.
fn caption_entry(format: CaptionFormat) -> Vec<u8> {
    let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
    if format == CaptionFormat::Placed {
        entry.extend(atom(b"vttC", b"WEBVTT"));
        return atom(b"wvtt", &entry);
    }
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

/// A caption track's samples, in ms: each cue, and empty ones between, to `length`. 3GPP timed
/// text's are their text's length, then the text; WebVTT's a cue box (its settings and text) or
/// an empty box.
fn caption_samples(cues: &[(u32, u32, &str)], length: u32, format: CaptionFormat) -> Vec<(u32, Vec<u8>)> {
    let (mut samples, mut at) = (Vec::new(), 0);
    let text = |words: &str| match (format, words) {
        (CaptionFormat::Plain, _) => [(words.len() as u16).to_be_bytes().to_vec(), words.as_bytes().to_vec()].concat(),
        (CaptionFormat::Placed, "") => atom(b"vtte", &[]),
        (CaptionFormat::Placed, _) => atoms(b"vttc", &[atom(b"sttg", b"align:start position:10% line:10%"), atom(b"payl", words.as_bytes())]),
    };
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
    let coding = if jpeg { Coding::Jpeg } else { Coding::H264 };
    movie_with(count, rate, coding, sounds, captions, None)
}

/// How a made file's frames are coded.
#[derive(Clone, Copy, PartialEq)]
pub enum Coding {
    Jpeg,
    H264,
    Hevc,
}

/// Where a made file's captions in the stream go: the frames' SEI, or (CEA-608's) a QuickTime
/// closed-caption track.
#[derive(Clone, Copy, PartialEq)]
pub enum Carried {
    InSei,
    InCaptionTrack,
}

/// A file of `count` frames at 10 a second (lossless H.264 or HEVC), no sound, with these
/// captions in the stream (see `cc_data`): an MP4 file, or a QuickTime movie with a
/// closed-caption track.
pub fn movie_captioned_in_stream(count: usize, coding: Coding, in_stream: &InStream, carried: Carried) -> Vec<u8> {
    movie_with(count, 10, coding, &[], &[], Some((in_stream, carried)))
}

fn movie_with(count: usize, rate: u32, coding: Coding, sounds: &[Sound], captions: &[Captions], in_stream: Option<(&InStream, Carried)>) -> Vec<u8> {
    let duration_ms = count as u32 * 1000 / rate;
    let samples = count as u32 * 8000 / rate;
    let caption_track = in_stream.filter(|(_, carried)| *carried == Carried::InCaptionTrack).map(|(captions, _)| captions);
    let ftyp = match caption_track {
        Some(_) => atom(b"ftyp", b"qt  \0\0\x02\0qt  "),
        None => atom(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41"),
    };
    let (mut mdat, mut traks) = (Vec::new(), Vec::new());
    let start = ftyp.len() as u32 + 8;
    let mut put = |bytes: &[u8]| {
        mdat.extend_from_slice(bytes);
        start + (mdat.len() - bytes.len()) as u32
    };
    let in_sei = in_stream.filter(|(_, carried)| *carried == Carried::InSei).map(|(captions, _)| cc_data(captions, count));
    let sei = |n: usize| in_sei.as_ref().map_or(Vec::new(), |triplets| length_prefixed(caption_sei(&triplets[n], coding == Coding::Hevc)));
    let frames: Vec<Vec<u8>> = match coding {
        Coding::Jpeg => (0..count).map(|n| taut::jpeg::encode(64, &frame_pixels(n), 90).unwrap()).collect(),
        Coding::H264 => (0..count).map(|n| [sei(n), length_prefixed(h264_frame(n))].concat()).collect(),
        Coding::Hevc => (0..count).map(|n| [sei(n), length_prefixed(hevc_frame(n))].concat()).collect(),
    };
    let offsets: Vec<u32> = frames.iter().map(|frame| put(frame)).collect();
    let sizes: Vec<u32> = frames.iter().map(|frame| frame.len() as u32).collect();
    let entry = match coding {
        Coding::Jpeg => visual_entry(b"jpeg", Vec::new()),
        Coding::H264 => avc1_entry(),
        Coding::Hevc => visual_entry(b"hvc1", atom(b"hvcC", &hvcc())),
    };
    let video = Trak { id: 1, holds: Holds::Frames, enabled: true, group: 0, language: "und", name: "Video", timescale: rate, duration_ms, media_duration: count as u32, size: [64, 48] };
    traks.push(video.atom(entry, table(&sizes, &vec![1; count], &offsets)));
    if let Some(captions) = caption_track {
        // (Field 2's pairs in a `cdt2` atom where it has any but padding.)
        let field_2 = |pairs: &[u8; 6]| if pairs.iter().all(|byte| *byte == 0x80) { Vec::new() } else { atom(b"cdt2", pairs) };
        let samples: Vec<Vec<u8>> = cea608_fields(captions, count).iter().map(|[one, two]| [atom(b"cdat", one), field_2(two)].concat()).collect();
        let offsets: Vec<u32> = samples.iter().map(|sample| put(sample)).collect();
        let sizes: Vec<u32> = samples.iter().map(|sample| sample.len() as u32).collect();
        let trak = Trak { id: 2, holds: Holds::ClosedCaptions, enabled: true, group: 0, language: "eng", name: "Closed captions", ..video };
        traks.push(trak.atom(atom(b"c608", &[0, 0, 0, 0, 0, 0, 0, 1]), table(&sizes, &vec![1; count], &offsets)));
    }
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
        let samples = caption_samples(captions.cues, duration_ms, captions.format);
        let offsets: Vec<u32> = samples.iter().map(|(_, bytes)| put(bytes)).collect();
        let sizes: Vec<u32> = samples.iter().map(|(_, bytes)| bytes.len() as u32).collect();
        let durations: Vec<u32> = samples.iter().map(|(duration, _)| *duration).collect();
        let trak = Trak { id: traks.len() as u32 + 1, holds: captions.holds(), enabled: false, group: 2, language: captions.language, name: captions.name, timescale: 1000, media_duration: duration_ms, ..video };
        traks.push(trak.atom(caption_entry(captions.format), table(&sizes, &durations, &offsets)));
    }
    let mut mvhd = be32(&[0, 0, 0, 1000, duration_ms, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, traks.len() as u32 + 1]));
    let moov = atoms(b"moov", &[vec![atom(b"mvhd", &mvhd)], traks].concat());
    [ftyp, atom(b"mdat", &mdat), moov].concat()
}


/// A fragmented MP4 file `count` tenths of a second long holding only these caption tracks, a
/// fragment a second: each track's samples starting in that second (see `caption_samples`).
pub fn fragmented_with_captions(count: usize, captions: &[Captions]) -> Vec<u8> {
    let length = count as u32 * 100;
    let tracks: Vec<Vec<(u32, u32, Vec<u8>)>> = captions
        .iter()
        .map(|captions| {
            let mut at = 0;
            let samples = caption_samples(captions.cues, length, captions.format);
            let mut timed = Vec::new();
            for (duration, bytes) in samples {
                timed.push((at, duration, bytes));
                at += duration;
            }
            timed
        })
        .collect();
    let mut traks = Vec::new();
    for (nth, captions) in captions.iter().enumerate() {
        let trak = Trak { id: nth as u32 + 1, holds: captions.holds(), enabled: false, group: 2, language: captions.language, name: captions.name, timescale: 1000, duration_ms: 0, media_duration: 0, size: [64, 48] };
        traks.push(trak.atom(caption_entry(captions.format), table(&[], &[], &[])));
    }
    let trex: Vec<Vec<u8>> = (1..=captions.len() as u32).map(|id| atom(b"trex", &be32(&[0, id, 1, 0, 0, 0]))).collect();
    let mut mvhd = be32(&[0, 0, 0, 1000, 0, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, captions.len() as u32 + 1]));
    let moov = atoms(b"moov", &[vec![atom(b"mvhd", &mvhd)], traks, vec![atoms(b"mvex", &trex)]].concat());
    let mut file = [atom(b"ftyp", b"isom\0\0\x02\0isomiso6"), moov].concat();
    for (sequence, second) in (0..length.div_ceil(1000)).enumerate() {
        let starting = |samples: &Vec<(u32, u32, Vec<u8>)>| samples.iter().filter(|(at, _, _)| at / 1000 == second).cloned().collect::<Vec<_>>();
        let parts: Vec<Vec<(u32, u32, Vec<u8>)>> = tracks.iter().map(starting).collect();
        // (A track fragment's data offset counts from the fragment's start: known once the
        // fragment's own size is, which does not change with it.)
        let moof = |data_at: u32| {
            let mut offset = data_at;
            let trafs = parts.iter().enumerate().map(|(nth, samples)| {
                let tfhd = atom(b"tfhd", &be32(&[0x02_0000, nth as u32 + 1]));
                let tfdt = atom(b"tfdt", &[be32(&[1 << 24, 0]), be32(&[samples.first().map_or(second * 1000, |sample| sample.0)])].concat());
                let sizes: Vec<u32> = samples.iter().flat_map(|(_, duration, bytes)| [*duration, bytes.len() as u32]).collect();
                let trun = atom(b"trun", &[be32(&[0x301, samples.len() as u32, offset]), be32(&sizes)].concat());
                offset += samples.iter().map(|(_, _, bytes)| bytes.len() as u32).sum::<u32>();
                atoms(b"traf", &[tfhd, tfdt, trun])
            });
            atoms(b"moof", &[vec![atom(b"mfhd", &be32(&[0, sequence as u32 + 1]))], trafs.collect()].concat())
        };
        let size = moof(0).len() as u32;
        let data: Vec<u8> = parts.iter().flatten().flat_map(|(_, _, bytes)| bytes.clone()).collect();
        file.extend([moof(size + 8), atom(b"mdat", &data)].concat());
    }
    file
}

/// H.264's sequence and picture parameters for baseline frames `width` by `height` (each a
/// multiple of 16 but for a cut of whole pairs of rows at the bottom), level 5.1.
pub fn parameters_sized(width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
    let (across, down) = (width.div_ceil(16), height.div_ceil(16));
    let mut sps = Bits::default();
    sps.put(66, 8);
    sps.put(0xC0, 8);
    sps.put(51, 8);
    for value in [0, 0, 2, 1] {
        sps.ue(value);
    }
    sps.put(0, 1);
    sps.ue(across as u32 - 1);
    sps.ue(down as u32 - 1);
    let cut = (down * 16 - height) / 2;
    sps.put(0b11, 2); // frames only, 8×8 inference
    sps.put((cut > 0) as u32, 1);
    if cut > 0 {
        [0, 0, 0, cut as u32].into_iter().for_each(|side| sps.ue(side));
    }
    sps.put(0, 1); // no VUI
    let (_, pps) = parameters();
    (nal(0x67, &sps.end()), pps)
}

/// `frame_pixels` stretched to `width` by `height`.
pub fn frame_pixels_sized(number: usize, width: usize, height: usize) -> Vec<u32> {
    let small = frame_pixels(number);
    (0..width * height).map(|at| small[(at / width * 48 / height) * 64 + at % width * 64 / width]).collect()
}

/// One frame `width` by `height` (see `parameters_sized`) as an IDR slice of I_PCM
/// macroblocks: `frame_pixels_sized`, grey.
pub fn h264_frame_sized(number: usize, width: usize, height: usize) -> Vec<u8> {
    let pixels = frame_pixels_sized(number, width, height);
    let (across, down) = (width.div_ceil(16), height.div_ceil(16));
    let luma = |x: usize, y: usize| {
        let p = pixels[y.min(height - 1) * width + x.min(width - 1)];
        let full = (299 * (p >> 16 & 255) + 587 * (p >> 8 & 255) + 114 * (p & 255) + 500) / 1000;
        (16 + (full * 219 + 127) / 255) as u8
    };
    let mut slice = Bits::default();
    slice.ue(0);
    slice.ue(7);
    slice.ue(0);
    slice.put(0, 4);
    slice.ue(number as u32 & 0xFFFF);
    slice.put(0, 2);
    slice.se(0);
    for at in 0..across * down {
        let (mx, my) = (at % across * 16, at / across * 16);
        slice.ue(25);
        slice.align();
        slice.bytes.extend((0..256).map(|i| luma(mx + i % 16, my + i / 16)));
        slice.bytes.extend([128u8; 128]);
    }
    nal(0x65, &slice.end())
}

/// An MP4 file of `count` frames `width` by `height` at `rate` a second (lossless H.264, see
/// `h264_frame_sized`), no sound.
pub fn big_mp4(count: usize, rate: u32, width: usize, height: usize) -> Vec<u8> {
    let duration_ms = count as u32 * 1000 / rate;
    let ftyp = atom(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
    let frames: Vec<Vec<u8>> = (0..count).map(|n| length_prefixed(h264_frame_sized(n, width, height))).collect();
    let start = ftyp.len() as u32 + 8;
    let offsets: Vec<u32> = frames.iter().scan(start, |at, frame| Some(std::mem::replace(at, *at + frame.len() as u32))).collect();
    let sizes: Vec<u32> = frames.iter().map(|frame| frame.len() as u32).collect();
    let (sps, pps) = parameters_sized(width, height);
    let avcc = [&[1, 66, 0xC0, 51, 0xFF, 0xE1][..], &(sps.len() as u16).to_be_bytes(), &sps, &[1], &(pps.len() as u16).to_be_bytes(), &pps].concat();
    let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
    entry.extend(be32(&[0, 0, 0, 0]));
    entry.extend_from_slice(&(width as u16).to_be_bytes());
    entry.extend_from_slice(&(height as u16).to_be_bytes());
    entry.extend(be32(&[0x48_0000, 0x48_0000, 0]));
    entry.extend_from_slice(&[0, 1]);
    entry.extend_from_slice(&[0; 32]);
    entry.extend_from_slice(&[0, 24, 0xFF, 0xFF]);
    entry.extend(atom(b"avcC", &avcc));
    let video = Trak { id: 1, holds: Holds::Frames, enabled: true, group: 0, language: "und", name: "Video", timescale: rate, duration_ms, media_duration: count as u32, size: [width as u32, height as u32] };
    let trak = video.atom(atom(b"avc1", &entry), table(&sizes, &vec![1; count], &offsets));
    let mut mvhd = be32(&[0, 0, 0, 1000, duration_ms, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, 2]));
    let moov = atoms(b"moov", &[atom(b"mvhd", &mvhd), trak]);
    [ftyp, atom(b"mdat", &frames.concat()), moov].concat()
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
        let (codec, private) = match captions.format {
            CaptionFormat::Plain => ("S_TEXT/UTF8", Vec::new()),
            CaptionFormat::Placed => ("S_TEXT/ASS", element(0x63A2, SSA_HEADER.as_bytes())),
        };
        tracks.push(entry(first_caption + nth as u8, 0x11, codec, captions.language, captions.name, false, private));
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
                // (SSA's events as Matroska keeps them: read order, layer, style, name, margins,
                // effect, text.)
                let event = |order: usize, text: &str| match captions.format {
                    CaptionFormat::Plain => text.to_owned(),
                    CaptionFormat::Placed => format!("{order},0,Default,,0,0,0,,{{\\an7\\pos(64,48)}}{text}"),
                };
                blocks.extend(starting.enumerate().map(|(order, &(from, to, text))| block(first_caption + nth as u8, at, event(order, text).as_bytes(), Some((to - from) * 100))));
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
    // (Written once: a player may still hold it open, and Windows lets no one write to it then.)
    if !std::fs::metadata(&path).is_ok_and(|known| known.len() == bytes.len() as u64) {
        std::fs::write(&path, bytes).unwrap();
    }
    path
}

/// A NAL unit as MP4 keeps it: its length first.
pub fn length_prefixed(nal: Vec<u8>) -> Vec<u8> {
    [(nal.len() as u32).to_be_bytes().to_vec(), nal].concat()
}

/// An HLS stream of `count` frames at 10 a second (lossless H.264, 64×48) in fragmented MP4
/// segments of a second, with these captions in its video stream and these subtitle renditions
/// (WebVTT, a segment each): its files, each a name and its bytes. `master.m3u8` plays it; it
/// declares the closed captions present (`CC1` to `CC4` and `SERVICE1`…, named as taut names
/// them: `CC1`, `Service 1`).
pub fn hls_stream(count: usize, in_stream: &InStream, subtitles: &[Captions]) -> Vec<(String, Vec<u8>)> {
    let length = count as u32 * 100;
    let mut files = vec![("init.mp4".to_string(), hls_init())];
    let mut video = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n");
    let triplets = cc_data(in_stream, count);
    for (sequence, first) in (0..count).step_by(10).enumerate() {
        let frames: Vec<Vec<u8>> = (first..count.min(first + 10)).map(|n| [length_prefixed(caption_sei(&triplets[n], false)), length_prefixed(h264_frame(n))].concat()).collect();
        video.push_str(&format!("#EXTINF:{:.3},\nvideo-{sequence}.m4s\n", frames.len() as f32 / 10.0));
        files.push((format!("video-{sequence}.m4s"), hls_segment(sequence as u32, first as u32, &frames)));
    }
    video.push_str("#EXT-X-ENDLIST\n");
    files.push(("video.m3u8".into(), video.into_bytes()));
    let mut master = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    for (nth, captions) in subtitles.iter().enumerate() {
        let language = &captions.language[..2];
        master.push_str(&format!("#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subtitles\",NAME=\"{}\",LANGUAGE=\"{language}\",AUTOSELECT=YES,DEFAULT=NO,URI=\"subtitles-{nth}.m3u8\"\n", captions.name));
        let playlist = format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:{:.3},\nsubtitles-{nth}.vtt\n#EXT-X-ENDLIST\n", length.div_ceil(1000), length as f32 / 1000.0);
        files.push((format!("subtitles-{nth}.m3u8"), playlist.into_bytes()));
        files.push((format!("subtitles-{nth}.vtt"), webvtt(captions).into_bytes()));
    }
    let channels = in_stream.other_channels.iter().map(|(first, _)| match first {
        0x1C => "CC2",
        0x15 => "CC3",
        _ => "CC4",
    });
    let cea608 = (!in_stream.cea608.is_empty()).then_some("CC1").into_iter().chain(channels).map(|channel| (channel.to_string(), channel.to_string()));
    let services = in_stream.services.iter().map(|(number, _)| (format!("SERVICE{number}"), format!("Service {number}")));
    for (id, name) in cea608.chain(services) {
        master.push_str(&format!("#EXT-X-MEDIA:TYPE=CLOSED-CAPTIONS,GROUP-ID=\"captions\",NAME=\"{name}\",LANGUAGE=\"en\",INSTREAM-ID=\"{id}\"\n"));
    }
    let subtitled = if subtitles.is_empty() { "" } else { ",SUBTITLES=\"subtitles\"" };
    master.push_str(&format!("#EXT-X-STREAM-INF:BANDWIDTH=800000,AVERAGE-BANDWIDTH=500000,CODECS=\"avc1.42c01e\",RESOLUTION=64x48,FRAME-RATE=10.000{subtitled},CLOSED-CAPTIONS=\"captions\"\nvideo.m3u8\n"));
    files.push(("master.m3u8".into(), master.into_bytes()));
    files
}

/// The initialisation segment of `hls_stream`: a movie of one H.264 track (id 1, 90 kHz) with
/// no samples, its fragments' defaults.
fn hls_init() -> Vec<u8> {
    let video = Trak { id: 1, holds: Holds::Frames, enabled: true, group: 0, language: "und", name: "Video", timescale: 90_000, duration_ms: 0, media_duration: 0, size: [64, 48] };
    let mut mvhd = be32(&[0, 0, 0, 1000, 0, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, 2]));
    let mvex = atoms(b"mvex", &[atom(b"trex", &be32(&[0, 1, 1, 9000, 0, 0]))]);
    [atom(b"ftyp", b"iso6\0\0\x02\0iso6cmfcavc1mp41"), atoms(b"moov", &[atom(b"mvhd", &mvhd), video.atom(avc1_entry(), table(&[], &[], &[])), mvex])].concat()
}

/// A media segment of `hls_stream`: these frames (each a key frame), the first numbered
/// `first`, as fragment `sequence`.
fn hls_segment(sequence: u32, first: u32, frames: &[Vec<u8>]) -> Vec<u8> {
    let moof = |data_at: u32| {
        let samples: Vec<u32> = frames.iter().flat_map(|frame| [9000, frame.len() as u32, 0x0200_0000]).collect();
        let trun = atom(b"trun", &[be32(&[0x701, frames.len() as u32, data_at]), be32(&samples)].concat());
        let tfdt = atom(b"tfdt", &[be32(&[1 << 24, 0]), be32(&[first * 9000])].concat());
        let traf = atoms(b"traf", &[atom(b"tfhd", &be32(&[0x02_0000, 1])), tfdt, trun]);
        atoms(b"moof", &[atom(b"mfhd", &be32(&[0, sequence + 1])), traf])
    };
    let size = moof(0).len() as u32;
    [moof(size + 8), atom(b"mdat", &frames.concat())].concat()
}

/// A caption track as a WebVTT file: plain cues, or placed as `CaptionFormat::Placed` says.
pub fn webvtt(captions: &Captions) -> String {
    let time = |tenths: u32| format!("00:00:{:02}.{:03}", tenths / 10, tenths % 10 * 100);
    let settings = if captions.format == CaptionFormat::Placed { " align:start position:10% line:10%" } else { "" };
    let cues = captions.cues.iter().map(|(from, to, text)| format!("{} --> {}{settings}\n{text}\n", time(*from), time(*to)));
    format!("WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n{}", cues.collect::<Vec<_>>().join("\n"))
}
