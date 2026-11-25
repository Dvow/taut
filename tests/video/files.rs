// Test videos made here, with no tool: frames that show their number, as lossless H.264 in
// MP4 and as MJPEG in AVI, with or without a silent sound track. Included by the unit tests of
// `src/video.rs` and by `tests/video_apple.rs`.
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

/// A track's header, media header, handler and sample table, around its sample entry.
pub fn track(id: u32, video: bool, timescale: u32, duration_ms: u32, media_duration: u32, entry: Vec<u8>, table: Vec<Vec<u8>>) -> Vec<u8> {
    let mut tkhd = be32(&[0x0000_0003, 0, 0, id, 0, duration_ms, 0, 0]);
    tkhd.extend_from_slice(&[0, 0, 0, 0]); // layer, alternate group
    tkhd.extend_from_slice(&[if video { 0 } else { 1 }, 0, 0, 0]); // volume, reserved
    tkhd.extend(be32(&MATRIX));
    tkhd.extend(be32(&if video { [64 << 16, 48 << 16] } else { [0, 0] }));
    let mut mdhd = be32(&[0, 0, 0, timescale, media_duration]);
    mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]);
    let mut hdlr = be32(&[0, 0]);
    hdlr.extend_from_slice(if video { b"vide" } else { b"soun" });
    hdlr.extend(be32(&[0, 0, 0]));
    hdlr.push(0);
    let header = if video { atom(b"vmhd", &be32(&[1, 0, 0])) } else { atom(b"smhd", &be32(&[0, 0])) };
    let dinf = atoms(b"dinf", &[atom(b"dref", &[be32(&[0, 1]), atom(b"url ", &be32(&[1]))].concat())]);
    let stsd = atom(b"stsd", &[be32(&[0, 1]), entry].concat());
    let stbl = atoms(b"stbl", &[vec![stsd], table].concat());
    let minf = atoms(b"minf", &[header, dinf, stbl]);
    let mdia = atoms(b"mdia", &[atom(b"mdhd", &mdhd), atom(b"hdlr", &hdlr), minf]);
    atoms(b"trak", &[atom(b"tkhd", &tkhd), mdia])
}

/// An MP4 file of `count` frames at `rate` a second (H.264, 64×48, lossless), with a
/// silent 8 kHz mono track as long where `sound`.
pub fn mp4(count: usize, rate: u32, sound: bool) -> Vec<u8> {
    let (sps, pps) = parameters();
    let frames: Vec<Vec<u8>> = (0..count).map(h264_frame).map(|f| [(f.len() as u32).to_be_bytes().to_vec(), f].concat()).collect();
    let samples = count as u32 * 8000 / rate;
    let ftyp = atom(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
    let mut offsets = Vec::new();
    let mut at = ftyp.len() as u32 + 8;
    for frame in &frames {
        offsets.push(at);
        at += frame.len() as u32;
    }
    let sound_at = at;
    let mut mdat = frames.concat();
    if sound {
        mdat.extend(vec![0u8; samples as usize * 2]);
    }
    let duration_ms = count as u32 * 1000 / rate;
    let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
    entry.extend(be32(&[0, 0, 0, 0]));
    entry.extend_from_slice(&[0, 64, 0, 48]);
    entry.extend(be32(&[0x48_0000, 0x48_0000, 0]));
    entry.extend_from_slice(&[0, 1]);
    entry.extend_from_slice(&[0; 32]);
    entry.extend_from_slice(&[0, 24, 0xFF, 0xFF]);
    let avcc = [&[1, 66, 0xC0, 30, 0xFF, 0xE1][..], &(sps.len() as u16).to_be_bytes(), &sps, &[1], &(pps.len() as u16).to_be_bytes(), &pps].concat();
    entry.extend(atom(b"avcC", &avcc));
    let video_table = vec![
        atom(b"stts", &be32(&[0, 1, count as u32, 1])),
        atom(b"stsc", &be32(&[0, 1, 1, 1, 1])),
        atom(b"stsz", &[be32(&[0, 0, count as u32]), be32(&frames.iter().map(|f| f.len() as u32).collect::<Vec<_>>())].concat()),
        atom(b"stco", &[be32(&[0, count as u32]), be32(&offsets)].concat()),
    ];
    let mut traks = vec![track(1, true, rate, duration_ms, count as u32, atom(b"avc1", &entry), video_table)];
    if sound {
        let mut entry = vec![0, 0, 0, 0, 0, 0, 0, 1];
        entry.extend(be32(&[0, 0]));
        entry.extend_from_slice(&[0, 1, 0, 16, 0, 0, 0, 0]);
        entry.extend(be32(&[8000 << 16]));
        let table = vec![atom(b"stts", &be32(&[0, 1, samples, 1])), atom(b"stsc", &be32(&[0, 1, 1, samples, 1])), atom(b"stsz", &be32(&[0, 2, samples])), atom(b"stco", &be32(&[0, 1, sound_at]))];
        traks.push(track(2, false, 8000, duration_ms, samples, atom(b"sowt", &entry), table));
    }
    let mut mvhd = be32(&[0, 0, 0, 1000, duration_ms, 0x10000]);
    mvhd.extend_from_slice(&[1, 0, 0, 0]);
    mvhd.extend(be32(&[0, 0]));
    mvhd.extend(be32(&MATRIX));
    mvhd.extend(be32(&[0, 0, 0, 0, 0, 0, traks.len() as u32 + 1]));
    let moov = atoms(b"moov", &[vec![atom(b"mvhd", &mvhd)], traks].concat());
    [ftyp, atom(b"mdat", &mdat), moov].concat()
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
