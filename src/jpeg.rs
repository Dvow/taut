//! JPEG decoding and encoding. Decodes what cameras, phones and editors write, and the rarer
//! kinds scanners and medical systems keep: baseline or progressive, 8 or 12 bits, Huffman or
//! arithmetic coded, or lossless at up to 16 bits; greyscale, colour or CMYK, rotated upright
//! per a camera's EXIF orientation, and converted to screen colours per an embedded ICC
//! profile. With the feature `rare-jpeg`, every process of the standard: hierarchical too, and
//! lossless arithmetic coded. Shows a photo with no other library.
//!
//! A JPEG stores each colour component in its own plane, usually luma at full size and two
//! chroma planes at half. A plane is cut into 8 by 8 blocks, each stored as 64 DCT
//! coefficients. Each was divided by a quantization table entry, so most high-frequency ones
//! became zero, and the rest Huffman-coded. Decoding undoes these in reverse.
//!
//! Each rare kind changes one step. Arithmetic coding spells the coefficients out as binary
//! decisions, each costing as little as it was probable. Lossless has no blocks or DCT: each
//! sample is stored as its difference from a prediction from its neighbours. Deeper than 8
//! bits, everything is computed at its own depth; only finished pixels are reduced to 8 bits.
//! Hierarchical is a series of frames: a small first image, then each later frame the
//! difference from the image so far, doubled in size where the file says.
//!
//! Encoding does the baseline or progressive steps in reverse, each in integer arithmetic
//! exactly as libjpeg does, so the output is byte for byte what libjpeg writes: RGB to YCbCr;
//! chroma halved each way, or not; forward DCT, quantized and rounded; Huffman-coded, with the
//! standard tables in one scan, or progressive in libjpeg's ten scans with tables made for each.
//! EXIF orientation, an ICC profile and XMP go in front. A program that writes no JPEG carries
//! none of this.

use std::num::Wrapping;

/// Coefficients are stored in zigzag order from lowest frequency to highest; this is each one's
/// position in row-major order.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52,
    45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Decodes a JPEG. Returns its width in pixels and its pixels in rows from the top,
/// `0xAARRGGBB` (always opaque): what [`Ui::add_picture`](crate::Ui::add_picture) takes. `None`
/// for a damaged file, not a JPEG, or an unsupported kind.
pub fn decode(jpeg: &[u8]) -> Option<(usize, Vec<u32>)> {
    let (mut read, mut profile) = (Jpeg::new(jpeg.len()), Vec::new());
    // The file is a sequence of segments: 0xFF, a marker byte, a length, and the payload.
    let mut rest = jpeg.strip_prefix(&[0xFF, 0xD8])?;
    while let Some((kind, body, after)) = part(rest) {
        rest = after;
        match kind {
            0xC0..=0xC3 | 0xC9 | 0xCA => read.frame(body, kind)?,
            // Hierarchical (differential frames, and the DHP and EXP segments), and lossless
            // arithmetic coded: with the feature `rare-jpeg`.
            0xC5..=0xC7 | 0xCB | 0xCD..=0xCF if RARE => read.frame(body, kind)?,
            0xDE if RARE => read.hierarchy(body)?,
            0xDF if RARE => read.expand(body)?,
            0xC5..=0xC8 | 0xCB | 0xCD..=0xCF | 0xDE | 0xDF => return None,
            0xC4 => read.codes(body)?,
            0xCC => read.sorting(body)?,
            0xD9 => break,
            0xDA => rest = read.scan(body, after)?,
            0xDB => read.dividers(body)?,
            0xDD => read.stretch = u16::from_be_bytes([*body.first()?, *body.get(1)?]) as usize,
            0xE0 => read.bright |= body.starts_with(b"JFIF\0"),
            0xE1 => read.turn = crate::exif_orientation::turn(body).unwrap_or(read.turn),
            0xEE => read.adobe = body.strip_prefix(b"Adobe").and_then(|rest| rest.get(6)).copied().or(read.adobe),
            // An ICC profile, in chunks: each gives its sequence number and the chunk count,
            // then its part.
            0xE2 => profile.extend(body.strip_prefix(b"ICC_PROFILE\0").and_then(|piece| Some((*piece.first()?, piece.get(2..)?)))),
            _ => {}
        }
    }
    let (width, mut pixels) = read.picture()?;
    // Chunks in their stated order, wherever they are in the file.
    let pieces = (1..=profile.len() as u8).flat_map(|which| profile.iter().filter(move |(each, _)| *each == which));
    if let Some(change) = crate::color_profiles::Change::of_profile(&pieces.flat_map(|(_, piece)| piece.iter().copied()).collect::<Vec<u8>>()) {
        change.apply(&mut pixels);
    }
    Some((width, pixels))
}

/// Whether the rarest processes are read (the feature `rare-jpeg`): only where they are is their
/// code compiled in.
const RARE: bool = cfg!(feature = "rare-jpeg");

/// The next segment: its marker, payload, and the rest of the file.
fn part(rest: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    // Fill bytes of 0xFF may precede a marker.
    let at = rest.windows(2).position(|pair| pair[0] == 0xFF && pair[1] != 0xFF && pair[1] != 0)?;
    let (kind, rest) = (rest[at + 1], &rest[at + 2..]);
    // A few markers have no payload.
    if matches!(kind, 0x01 | 0xD0..=0xD9) {
        return Some((kind, &[], rest));
    }
    let long = u16::from_be_bytes([*rest.first()?, *rest.get(1)?]) as usize;
    Some((kind, rest.get(2..long)?, &rest[long..]))
}

/// A frame header's fields (or the DHP segment's, which has the same): bits per sample, width,
/// height, and three bytes per plane: its id, sampling factors across and down (4 bits each,
/// 1 to 4), and quantization table (up to 3). One to four planes.
fn header(body: &[u8]) -> Option<(u32, usize, usize, &[u8])> {
    let [depth, high, low, wide, narrow, count, listed @ ..] = body else { return None };
    let (width, height) = (u16::from_be_bytes([*wide, *narrow]) as usize, u16::from_be_bytes([*high, *low]) as usize);
    let listed = listed.get(..3 * *count as usize).filter(|_| (1..=4).contains(count) && width * height != 0)?;
    let factor = |factor: u8| (1..=4).contains(&factor);
    listed.chunks(3).all(|plane| factor(plane[1] >> 4) && factor(plane[1] & 15) && plane[2] <= 3).then_some((*depth as u32, width, height, listed))
}

/// What the planes hold.
#[derive(Clone, Copy)]
enum Kind {
    Grey,
    /// RGB.
    Colours,
    /// YCbCr: what most images are stored as.
    Bright,
    /// CMYK, each inverted.
    Inks,
    /// YCCK: CMY stored as YCbCr, plus K.
    BrightInks,
}

/// A file's state as decoded so far.
struct Jpeg {
    /// Up to four quantization tables.
    dividers: [[u16; 64]; 4],
    /// The Huffman tables for DC coefficients…
    averages: [Codes; 4],
    /// …and for the 63 AC coefficients.
    details: [Codes; 4],
    /// Arithmetic coding conditioning. For DC, up to four tables: the bounds, as powers of two,
    /// below which the last difference counts as zero and above which as large…
    bounds: [[u8; 2]; 4],
    /// …and for AC: up to which coefficient the sizes have their own contexts.
    edges: [u8; 4],
    /// The restart interval in MCUs; 0 for none.
    stretch: usize,
    /// The EXIF orientation, 1 to 8; 1 is as stored.
    turn: usize,
    /// The Adobe marker's transform: 0 for RGB or CMYK as is, anything else for YCbCr.
    adobe: Option<u8>,
    /// Whether a JFIF marker says the three planes are YCbCr.
    bright: bool,
    width: usize,
    height: usize,
    /// Bits per sample.
    depth: u32,
    /// Progressive.
    steps: bool,
    /// Arithmetic coded, not Huffman.
    arithmetic: bool,
    /// Lossless: sample by sample, no DCT blocks.
    exact: bool,
    /// A differential frame of a hierarchical image: what it brings is added to the image so far.
    differential: bool,
    /// What the frame's blocks hold.
    blocks: BlockKind,
    /// The largest sampling factors of any plane, across and down.
    most: (usize, usize),
    planes: Vec<Plane>,
    /// The bytes of the file not yet claimed by a frame's blocks (see `frame`).
    room: usize,
    /// A hierarchical image's finished planes, as far as its frames have built them.
    hierarchy: Option<Hierarchy>,
}

impl Jpeg {
    /// A file of `room` bytes with nothing decoded yet. (The arithmetic conditioning is the
    /// default for files that don't set it.)
    fn new(room: usize) -> Self {
        let (dividers, averages, details, bounds, edges) = ([[0; 64]; 4], [Codes::NONE; 4], [Codes::NONE; 4], [[0, 1]; 4], [5; 4]);
        let (depth, steps, arithmetic, exact, differential, most, planes) = (8, false, false, false, false, (1, 1), Vec::new());
        let (stretch, turn, adobe, bright, width, height, hierarchy) = (0, 1, None, false, 0, 0, None);
        let blocks = BlockKind::Bytes;
        Jpeg { dividers, averages, details, bounds, edges, stretch, turn, adobe, bright, width, height, depth, steps, arithmetic, exact, differential, blocks, most, planes, room, hierarchy }
    }

    /// The SOF segment: size, and per plane its sampling factors and quantization table. `kind`
    /// is the marker, which says how the image is coded.
    fn frame(&mut self, body: &[u8], kind: u8) -> Option<()> {
        let (depth, width, height, listed) = header(body)?;
        let (steps, exact, arithmetic, differential) = (kind & 3 == 2, kind & 3 == 3, kind & 8 != 0, kind & 4 != 0);
        // DCT gives 8 or 12 bits; lossless, anything from 2 to 16.
        let deep_enough = if exact { (2..=16).contains(&depth) } else { matches!(depth, 8 | 12) };
        if !deep_enough {
            return None;
        }
        // Only a hierarchical image has more than one frame, or frames of two planes.
        match RARE && self.hierarchy.is_some() {
            true => self.fits(depth, (width, height), listed, differential)?,
            false if differential || !self.planes.is_empty() || listed.len() / 3 == 2 => return None,
            false => {}
        }
        let (across, down) = (|plane: &[u8]| (plane[1] >> 4) as usize, |plane: &[u8]| (plane[1] & 15) as usize);
        let most = (listed.chunks(3).map(across).max()?, listed.chunks(3).map(down).max()?);
        // A block is 8 samples a side; lossless, one sample. Every block of every plane takes
        // at least a bit of the file, each frame's of its own part. (Arithmetic coding can pack
        // a near-empty image tighter: such a one can't be told from a lie.) A file claiming more
        // blocks than it has bits lies about its size, and nothing is allocated for it.
        let side = if exact { 1 } else { 8 };
        let blocks = |plane: &[u8]| (width * across(plane)).div_ceil(most.0).div_ceil(side) * (height * down(plane)).div_ceil(most.1).div_ceil(side);
        self.room = self.room.checked_sub(listed.chunks(3).map(blocks).sum::<usize>() / 8)?;
        // An MCU is the smallest area holding whole blocks of every plane. Samples wider than a
        // byte, and differences, get planes of 16 bits.
        let (units, deep) = ((width.div_ceil(side * most.0), height.div_ceil(side * most.1)), exact || depth != 8 || differential);
        let plane = |plane: &[u8]| Plane::new(plane[0], (across(plane), down(plane)), plane[2] as usize, units, side, steps, deep);
        (self.width, self.height, self.depth, self.most) = (width, height, depth, most);
        (self.steps, self.arithmetic, self.exact, self.differential) = (steps, arithmetic, exact, differential);
        self.blocks = match (differential, depth) {
            (false, 8) => BlockKind::Bytes,
            (false, _) => BlockKind::Twelve,
            (true, 8) => BlockKind::EightBitDifferences,
            (true, _) => BlockKind::TwelveBitDifferences,
        };
        self.planes = listed.chunks(3).map(plane).collect();
        Some(())
    }

    /// Ends the frame before, if any, and whether the next fits the hierarchical image: as
    /// deep, no larger, of its planes, and differential only where it has something to add to.
    fn fits(&mut self, depth: u32, (width, height): (usize, usize), listed: &[u8], differential: bool) -> Option<()> {
        if !self.planes.is_empty() {
            self.fold()?;
        }
        let hierarchy = self.hierarchy.as_ref()?;
        let built = |name: u8| hierarchy.planes.iter().find(|built| built.name == name).map(|built| !built.samples.is_empty());
        let known = listed.chunks(3).all(|plane| built(plane[0]).is_some_and(|built| built || !differential));
        (depth == hierarchy.depth && width <= hierarchy.size.0 && height <= hierarchy.size.1 && known).then_some(())
    }

    /// The DHP segment, before the first frame of a hierarchical image: its finished size and
    /// planes, as a frame header gives them.
    fn hierarchy(&mut self, body: &[u8]) -> Option<()> {
        let (depth, width, height, listed) = header(body)?;
        if self.hierarchy.is_some() || !self.planes.is_empty() || listed.len() / 3 == 2 {
            return None;
        }
        let planes = listed.chunks(3).map(|plane| Built { name: plane[0], across: (plane[1] >> 4) as usize, down: (plane[1] & 15) as usize, width: 0, height: 0, samples: Vec::new() });
        self.hierarchy = Some(Hierarchy { size: (width, height), depth, planes: planes.collect(), lossless: false, expand: (false, false) });
        Some(())
    }

    /// The EXP segment: the next frame adds to the image so far doubled across, down, or both.
    /// Ends the frame before.
    fn expand(&mut self, body: &[u8]) -> Option<()> {
        if !self.planes.is_empty() {
            self.fold()?;
        }
        let ([both], Some(hierarchy)) = (body, self.hierarchy.as_mut()) else { return None };
        hierarchy.expand = (both >> 4 == 1, both & 15 == 1);
        (both & 0xEE == 0).then_some(())
    }

    /// DQT: tables, each with its index and 8- or 16-bit precision, then its entries.
    fn dividers(&mut self, mut body: &[u8]) -> Option<()> {
        while let [head, rest @ ..] = body {
            let (long, table) = (64 << (head >> 4).min(2), self.dividers.get_mut((head & 15) as usize)?);
            let listed = rest.get(..long).filter(|_| long <= 128)?;
            for (at, divider) in table.iter_mut().enumerate() {
                *divider = if long == 64 { listed[at] as u16 } else { u16::from_be_bytes([listed[at * 2], listed[at * 2 + 1]]) };
            }
            body = &rest[long..];
        }
        Some(())
    }

    /// DHT: tables, each with its class and index, then the code count per length 1 to 16, then
    /// the symbols.
    fn codes(&mut self, mut body: &[u8]) -> Option<()> {
        while let [head, rest @ ..] = body {
            let counts = rest.get(..16)?;
            let long = 16 + counts.iter().map(|count| *count as usize).sum::<usize>();
            let symbols = rest.get(16..long)?;
            // A DC value is coded as its bit count, never more than 16.
            let table = match head >> 4 {
                0 if symbols.iter().all(|symbol| *symbol <= 16) => &mut self.averages,
                1 => &mut self.details,
                _ => return None,
            };
            *table.get_mut((head & 15) as usize)? = Codes::new(counts, symbols)?;
            body = &rest[long..];
        }
        Some(())
    }

    /// DAC: arithmetic conditioning, pairs of table index and its bounds (two in a byte) or its
    /// AC edge.
    fn sorting(&mut self, body: &[u8]) -> Option<()> {
        for pair in body.chunks(2) {
            let [which, value] = *pair else { return None };
            let (low, high) = (value & 15, value >> 4);
            match which >> 4 {
                0 if low <= high => *self.bounds.get_mut((which & 15) as usize)? = [low, high],
                1 => *self.edges.get_mut((which & 15) as usize)? = value,
                _ => return None,
            }
        }
        Some(())
    }

    /// One scan. Its header gives its planes and tables, and for progressive images, the
    /// coefficient band and successive approximation bits. The entropy-coded data follows;
    /// returns the file after it.
    fn scan<'a>(&mut self, head: &[u8], data: &'a [u8]) -> Option<&'a [u8]> {
        let (count, rest) = head.split_first()?;
        let (listed, band) = rest.split_at_checked(2 * *count as usize)?;
        let [first, last, bits] = *band else { return None };
        // Lossless, the three values mean other things: the predictor (none in a differential
        // frame), nothing, and the point transform (low bits dropped).
        let rule = if self.differential { first == 0 } else { (1..=7).contains(&first) };
        if self.exact && (!rule || last != 0 || bits as u32 >= self.depth) {
            return None;
        }
        let (from, to, more, shift) = match (self.steps, self.exact) {
            (true, _) => (first as usize, last as usize, bits >> 4 != 0, bits & 15),
            (_, true) => (0, 0, false, bits),
            _ => (0, 63, false, 0),
        };
        // A DC scan carries nothing else, and an AC scan only one plane.
        if from > to || to > 63 || (shift > 13 && !self.exact) || (self.steps && from == 0 && to != 0) || (from != 0 && *count != 1) {
            return None;
        }
        // With one plane an MCU is one block; with more, as many blocks of each plane as its
        // sampling factors.
        let mut slots = Vec::new();
        for chosen in listed.chunks(2) {
            let plane = self.planes.iter().position(|plane| plane.name == chosen[0])?;
            let step = if *count == 1 { (1, 1) } else { (self.planes[plane].across, self.planes[plane].down) };
            let codes = ((chosen[1] >> 4) as usize & 3, (chosen[1] & 3) as usize);
            // A plane's quantization table is the one at its first scan, as libjpeg takes it: a
            // later frame may define another.
            if self.planes[plane].brought.iter().all(Option::is_none) {
                self.planes[plane].dividers = self.dividers[self.planes[plane].divider];
            }
            // A first scan of coefficients starts them; a refinement brings the next lower bit.
            let brought = &mut self.planes[plane].brought[from..=to];
            if brought.iter().any(|low| *low != more.then_some(shift + 1)) {
                return None;
            }
            brought.fill(Some(shift));
            slots.extend((0..step.0 * step.1).map(|block| Slot { plane, codes, step, across: block % step.0, down: block / step.0 }));
        }
        let plane = &self.planes[slots.first()?.plane];
        let alone = ((self.width * plane.across).div_ceil(self.most.0).div_ceil(plane.side), (self.height * plane.down).div_ceil(self.most.1).div_ceil(plane.side));
        let (wide, high) = if *count == 1 { alone } else { (plane.wide / plane.across, plane.high / plane.down) };
        // Lossless prediction uses the row above, so restarts may only fall on MCU row
        // boundaries.
        if self.exact && !self.stretch.is_multiple_of(wide) {
            return None;
        }
        // Both decoders stand ready; MCUs are read with the one matching the image's coding.
        let (bits, mut answers) = (Bits { data, at: 0, held: 0, count: 0 }, Answers { data, at: 0, above: 0, span: 0, ahead: 0 });
        answers.begin();
        let (guesses, leaning) = (vec![[0; 256]; if self.arithmetic { 8 } else { 0 }], [0; 4]);
        // Lossless and arithmetic coded, each sample's difference conditions its neighbours'.
        let conditions = self.planes.iter().map(|plane| vec![0; if self.exact && self.arithmetic { plane.wide * plane.high } else { 0 }]).collect();
        let (step, carried, shift) = (1 << shift.min(13), if self.differential { 0 } else { -1 }, shift as u32);
        let mut pass = Pass { bits, answers, guesses, leaning, from, to, step, more, before: [0; 4], carried, empty: 0, rule: first, shift, top: 0, conditions };
        match (self.exact, self.arithmetic) {
            (true, true) if RARE => self.units(&mut pass, &slots, (wide, high), Pass::arithmetic_exact_unit)?,
            (true, true) => return None,
            (true, false) => self.units(&mut pass, &slots, (wide, high), Pass::exact_unit)?,
            (false, true) => self.units(&mut pass, &slots, (wide, high), Pass::arithmetic_unit)?,
            (false, false) => self.units(&mut pass, &slots, (wide, high), Pass::unit)?,
        }
        data.get(if self.arithmetic { pass.answers.at } else { pass.bits.at }..)
    }

    /// Reads a scan's MCUs, `wide` per row, each with `unit`: the decoder for the image's kind.
    /// (Each kind gets its own monomorphized loop, none inlined into `scan`: the common one
    /// carries nothing of the others, which measured a few percent faster.)
    #[inline(never)]
    fn units<'a>(&mut self, pass: &mut Pass<'a>, slots: &[Slot], (wide, high): (usize, usize), unit: impl Fn(&mut Pass<'a>, usize, usize, &[Slot], &mut Jpeg) -> Option<()>) -> Option<()> {
        for at in 0..wide * high {
            if self.stretch != 0 && at != 0 && at % self.stretch == 0 {
                pass.restart(at / self.stretch, at / wide, self.arithmetic)?;
            }
            unit(pass, at % wide, at / wide, slots, self)?;
            // A truncated file is caught here, where more was read than exists. (Not with
            // arithmetic coding: trailing zeros may be omitted, and read as if present.)
            if pass.bits.count < 0 {
                return None;
            }
        }
        Some(())
    }

    /// What the planes hold: by their count, the Adobe transform, and component ids where they
    /// spell it out.
    fn kind(&self) -> Kind {
        let spelt = self.planes.iter().map(|plane| plane.name).eq(*b"RGB");
        // Lossless, three planes of one size with no marker are RGB: converting to YCbCr and
        // back wouldn't be lossless. (With different sizes the larger is luma: nobody
        // subsamples red against green.)
        let whole = self.exact && self.most == (1, 1) && !self.bright;
        match (self.planes.len(), self.adobe) {
            (1, _) => Kind::Grey,
            (3, Some(0)) => Kind::Colours,
            (3, None) if spelt || whole => Kind::Colours,
            (3, _) => Kind::Bright,
            (_, Some(0) | None) => Kind::Inks,
            _ => Kind::BrightInks,
        }
    }

    /// The finished image: each plane upsampled, converted to RGB row by row, each row placed
    /// where it goes once rotated upright.
    fn picture(mut self) -> Option<(usize, Vec<u32>)> {
        match RARE && self.hierarchy.is_some() {
            true => self.compose()?,
            false => self.finish_frame()?,
        }
        let (width, height, kind) = (self.width, self.height, self.kind());
        let spreads: Vec<Vec<Spot>> = self.planes.iter().map(|plane| plane.spread(width, self.most)).collect();
        let (mut lines, mut sums) = ([(); 4].map(|_| vec![0u8; width + 2]), vec![0u16; width + 1]);
        // Samples wider than a byte have their own rows, and are reduced to bytes only as RGB.
        let deep = self.planes.iter().any(|plane| plane.samples.is_empty());
        let (mut wide_lines, shown) = ([(); 4].map(|_| vec![0u16; if deep { width } else { 0 }]), if deep { shown(self.depth) } else { Vec::new() });
        let (mut row, mut pixels) = (vec![0u32; width], vec![0u32; width * height]);
        for y in 0..height {
            match deep {
                false => self.lines(y, &spreads, &mut sums, &mut lines),
                true => self.deep_lines(y, kind, &spreads, &shown, &mut wide_lines, &mut lines),
            }
            // Unrotated, rows go where they belong.
            colour(if deep { Kind::Colours } else { kind }, &lines, if self.turn == 1 { &mut pixels[y * width..][..width] } else { &mut row });
            if self.turn != 1 {
                crate::exif_orientation::lay_row(self.turn, [width, height], y, &row, &mut pixels);
            }
        }
        Some((crate::exif_orientation::width(self.turn, [width, height]), pixels))
    }

    /// Ends a frame: every plane must have had a scan. In a progressive image, blocks are still
    /// coefficients: they become samples.
    fn finish_frame(&mut self) -> Option<()> {
        if self.planes.is_empty() || self.planes.iter().any(|plane| plane.brought[0].is_none()) {
            return None;
        }
        for plane in &mut self.planes {
            let stored = std::mem::take(&mut plane.waves);
            for (block, waves) in stored.as_chunks::<64>().0.iter().enumerate() {
                plane.draw(block, waves, self.blocks);
            }
        }
        Some(())
    }

    /// Ends a frame of a hierarchical image: each plane it brought becomes that plane of the
    /// image so far, or, from a differential frame, is added to it, doubled first where the EXP
    /// segment said.
    fn fold(&mut self) -> Option<()> {
        self.finish_frame()?;
        let (planes, most, (width, height)) = (std::mem::take(&mut self.planes), self.most, (self.width, self.height));
        let hierarchy = self.hierarchy.as_mut()?;
        let expand = std::mem::take(&mut hierarchy.expand);
        if !self.differential {
            hierarchy.lossless = self.exact;
        }
        for plane in planes {
            let size = ((width * plane.across).div_ceil(most.0), (height * plane.down).div_ceil(most.1));
            let built = hierarchy.planes.iter_mut().find(|built| built.name == plane.name)?;
            built.add(&plane, size, self.differential.then_some(expand))?;
        }
        Some(())
    }

    /// The finished hierarchical image: the last frame folded in, and each plane as built, at
    /// the size and sampling the DHP segment gave.
    fn compose(&mut self) -> Option<()> {
        self.fold()?;
        let hierarchy = self.hierarchy.take()?;
        let (width, height) = hierarchy.size;
        let most = (hierarchy.planes.iter().map(|built| built.across).max()?, hierarchy.planes.iter().map(|built| built.down).max()?);
        let side = if hierarchy.lossless { 1 } else { 8 };
        let finished = |built: &Built| built.finished(((width * built.across).div_ceil(most.0), (height * built.down).div_ceil(most.1)), side, self.depth);
        self.planes = hierarchy.planes.iter().map(finished).collect::<Option<_>>()?;
        (self.width, self.height, self.most, self.exact) = (width, height, most, hierarchy.lossless);
        Some(())
    }

    /// Row `y` of the image from every plane, each at full width.
    fn lines(&self, y: usize, spreads: &[Vec<Spot>], sums: &mut [u16], lines: &mut [Vec<u8>; 4]) {
        for ((plane, spots), line) in self.planes.iter().zip(spreads).zip(lines) {
            plane.line(y, (self.width, self.height), self.most, spots, sums, line);
        }
    }

    /// The same for samples wider than a byte: planes at their own depth, then RGB bytes, with
    /// `shown` mapping each level. (Out of `picture`, for the reason `units` is out of `scan`.)
    #[inline(never)]
    fn deep_lines(&self, y: usize, kind: Kind, spreads: &[Vec<Spot>], shown: &[u8], wide_lines: &mut [Vec<u16>; 4], lines: &mut [Vec<u8>; 4]) {
        for ((plane, spots), line) in self.planes.iter().zip(spreads).zip(wide_lines.iter_mut()) {
            plane.deep_line(y, (self.width, self.height), self.most, spots, line);
        }
        narrow(kind, shown, wide_lines, lines);
    }
}

/// One plane of the image: grey, or a colour component.
struct Plane {
    /// Its component id, as scans name it.
    name: u8,
    /// Its sampling factors, across and down: below `most` where subsampled.
    across: usize,
    down: usize,
    /// Its quantization table.
    divider: usize,
    /// Its block count across and down, including padding to whole MCUs.
    wide: usize,
    high: usize,
    /// Samples per block side: 8, or 1 for lossless.
    side: usize,
    /// Its samples, in rows of `wide` blocks.
    samples: Vec<u8>,
    /// The same for samples wider than a byte: `samples` is then empty.
    deep: Vec<u16>,
    /// For progressive images: every block's 64 coefficients, as far as scans have brought
    /// them.
    waves: Vec<i16>,
    /// For each coefficient: the lowest bit scans have brought, if any. Each bit comes once,
    /// which also stops a file from requesting the same work over and over.
    brought: [Option<u8>; 64],
    /// Its quantization table's entries, as they were at its first scan.
    dividers: [u16; 64],
}

impl Plane {
    fn new(name: u8, (across, down): (usize, usize), divider: usize, units: (usize, usize), side: usize, steps: bool, deep: bool) -> Self {
        let (wide, high) = (units.0 * across, units.1 * down);
        let room = |wanted: bool| if wanted { wide * high * side * side } else { 0 };
        let (samples, deep, waves) = (vec![0; room(!deep)], vec![0; room(deep)], vec![0; room(steps)]);
        Plane { name, across, down, divider, wide, high, side, samples, deep, waves, brought: [None; 64], dividers: [0; 64] }
    }

    /// Writes a block's samples from its coefficients, as `blocks` says they are kept.
    fn draw(&mut self, block: usize, waves: &[i16; 64], blocks: BlockKind) {
        let stride = self.wide * 8;
        let corner = block / self.wide * 8 * stride + block % self.wide * 8;
        if blocks != BlockKind::Bytes {
            return self.draw_deep(corner, stride, waves, blocks);
        }
        put(&mut self.samples, corner, stride, &samples::<8, u8>(waves, &self.dividers, |level| (level + 128).clamp(0, 255) as u8));
    }

    /// The same for 12-bit samples, and differences. (Out of `draw`, so common images don't
    /// carry it in their inner loop.) Differences have no level shift, and wrap as the image
    /// they are added to does.
    #[inline(never)]
    fn draw_deep(&mut self, corner: usize, stride: usize, waves: &[i16; 64], blocks: BlockKind) {
        let block = match blocks {
            BlockKind::EightBitDifferences if RARE => samples::<8, u16>(waves, &self.dividers, |level| level as u16),
            BlockKind::TwelveBitDifferences if RARE => samples::<12, u16>(waves, &self.dividers, |level| level as u16),
            _ => samples::<12, u16>(waves, &self.dividers, |level| (level + 2048).clamp(0, 4095) as u16),
        };
        put(&mut self.deep, corner, stride, &block);
    }

    /// How each sample of an image row is interpolated from a row of this plane. Empty for a
    /// full size plane (no interpolation), or a byte plane at half width: `line` has a faster
    /// path for that.
    fn spread(&self, width: usize, most: (usize, usize)) -> Vec<Spot> {
        // Halves round up and down alternately, as libjpeg alternates, so images match its
        // output.
        let round = |x: usize| 2048 - (self.across != most.0 && (x % 2 == 1) == (self.down != most.1)) as u16;
        let spot = |x: usize, (before, far): (usize, usize)| Spot { before: before as u32, far: far as u16, round: round(x) };
        let smooth = self.smooth(width, most);
        let quick = (self.across, self.down) == most || (self.deep.is_empty() && most.0 == 2 * self.across && smooth);
        (0..width).filter(|_| !quick).map(|x| spot(x, between(x, self.across, most.0, smooth))).collect()
    }

    /// Whether a subsampled plane is upsampled smoothly, or each sample replicated: smoothly,
    /// as libjpeg does, except for lossless images, and a half-width plane only two samples
    /// wide.
    fn smooth(&self, width: usize, most: (usize, usize)) -> bool {
        self.side == 8 && !(most.0 == 2 * self.across && (width * self.across).div_ceil(most.0) <= 2)
    }

    /// Row `y` of the image from this plane, at full width. Where subsampled, each image sample
    /// is bilinear from the plane's four nearest, so half-size chroma comes back smooth, not
    /// blocky. `sums` is scratch space.
    fn line(&self, y: usize, size: (usize, usize), most: (usize, usize), spots: &[Spot], sums: &mut [u16], line: &mut [u8]) {
        let (wide, rows) = ((size.0 * self.across).div_ceil(most.0), (size.1 * self.down).div_ceil(most.1));
        let row = |row: usize| &self.samples[row * self.wide * 8..][..wide];
        if (self.across, self.down) == most {
            return line[..wide].copy_from_slice(row(y));
        }
        // Vertically first: each column's blend of the two rows, kept times 64 so it rounds
        // once. The last column is doubled, for samples past its centre.
        let smooth = self.smooth(size.0, most);
        let (above, far) = between(y, self.down, most.1, smooth);
        let (near, far, below) = (64 - far as u16, far as u16, (above + 1).min(rows - 1));
        for (sum, (above, below)) in sums.iter_mut().zip(row(above).iter().zip(row(below))) {
            *sum = *above as u16 * near + *below as u16 * far;
        }
        sums[wide] = sums[wide - 1];
        if most.0 == 2 * self.across && smooth {
            return double(&sums[..=wide], line, self.down != most.1);
        }
        let turn = (self.across == most.0 && y.is_multiple_of(2)) as u32;
        for (sample, spot) in line.iter_mut().zip(spots) {
            let (before, far) = (spot.before as usize, spot.far as u32);
            *sample = ((sums[before] as u32 * (64 - far) + sums[before + 1] as u32 * far + spot.round as u32 - turn) >> 12) as u8;
        }
    }

    /// The same row from a plane of samples wider than a byte. A subsampled plane uses the one
    /// generic interpolation for every factor: such images are rare.
    fn deep_line(&self, y: usize, size: (usize, usize), most: (usize, usize), spots: &[Spot], line: &mut [u16]) {
        let (wide, rows) = ((size.0 * self.across).div_ceil(most.0), (size.1 * self.down).div_ceil(most.1));
        let row = |row: usize| &self.deep[row.min(rows - 1) * self.wide * self.side..][..wide];
        if (self.across, self.down) == most {
            return line[..wide].copy_from_slice(row(y));
        }
        let (above, far) = between(y, self.down, most.1, self.smooth(size.0, most));
        let (upper, lower, far) = (row(above), row(above + 1), far as u32);
        let sum = |at: usize| upper[at.min(wide - 1)] as u32 * (64 - far) + lower[at.min(wide - 1)] as u32 * far;
        let turn = (self.across == most.0 && y.is_multiple_of(2)) as u32;
        for (sample, spot) in line.iter_mut().zip(spots) {
            let (before, far) = (spot.before as usize, spot.far as u32);
            *sample = ((sum(before) * (64 - far) + sum(before + 1) * far + spot.round as u32 - turn) >> 12) as u16;
        }
    }
}

/// What a frame's blocks hold, and how its planes keep them.
#[derive(Clone, Copy, PartialEq)]
enum BlockKind {
    /// 8-bit samples, a byte each.
    Bytes,
    /// 12-bit samples.
    Twelve,
    /// In a differential frame: differences to add to the image so far, of 8- or 12-bit
    /// samples.
    EightBitDifferences,
    TwelveBitDifferences,
}

/// A hierarchical image: its finished size, depth and planes (the DHP segment), and each plane
/// as far as its frames have built it.
struct Hierarchy {
    size: (usize, usize),
    depth: u32,
    planes: Vec<Built>,
    /// Whether its first frames were lossless (else DCT).
    lossless: bool,
    /// Whether the next frame's planes are added to these doubled across, and down (the EXP
    /// segment).
    expand: (bool, bool),
}

/// One plane of a hierarchical image: its id and sampling factors, and its samples so far
/// (none before its first frame), `width` by `height`.
struct Built {
    name: u8,
    across: usize,
    down: usize,
    width: usize,
    height: usize,
    samples: Vec<u16>,
}

impl Built {
    /// Takes in a frame's plane of `width` by `height` samples: as is, or added to what was
    /// built (modulo 2^16, as the standard has it), doubled first as `expand` says.
    fn add(&mut self, plane: &Plane, (width, height): (usize, usize), expand: Option<(bool, bool)>) -> Option<()> {
        let (stride, bytes) = (plane.wide * plane.side, !plane.samples.is_empty());
        let sample = |x: usize, y: usize| if bytes { plane.samples[y * stride + x] as u16 } else { plane.deep[y * stride + x] };
        let (base_width, base) = match expand {
            Some(expand) => self.doubled(expand),
            None => (width, vec![0; width * height]),
        };
        if base.len() / base_width.max(1) < height || base_width < width {
            return None;
        }
        self.samples = (0..width * height).map(|at| base[at / width * base_width + at % width].wrapping_add(sample(at % width, at / width))).collect();
        (self.width, self.height) = (width, height);
        Some(())
    }

    /// The samples so far and their width, doubled across, down, or both, in that order: each
    /// new sample the mean of those either side, rounded down, the last repeated past the edge.
    fn doubled(&self, (across, down): (bool, bool)) -> (usize, Vec<u16>) {
        let mean = |one: u16, other: u16| ((one as u32 + other as u32) / 2) as u16;
        let (mut width, mut samples) = (self.width, self.samples.clone());
        if across {
            let row = |row: &[u16]| (0..2 * width).map(|x| mean(row[x / 2], row[(x / 2 + x % 2).min(width - 1)])).collect::<Vec<u16>>();
            (samples, width) = (samples.chunks_exact(width).flat_map(row).collect(), 2 * width);
        }
        if down {
            let rows: Vec<&[u16]> = samples.chunks_exact(width).collect();
            let row = |y: usize| (0..width).map(|x| mean(rows[y / 2][x], rows[(y / 2 + y % 2).min(rows.len() - 1)][x])).collect::<Vec<u16>>();
            samples = (0..2 * rows.len()).flat_map(row).collect();
        }
        (width, samples)
    }

    /// The finished plane, `width` by `height` samples of `depth` bits, in blocks of `side`.
    /// `None` if the frames built less. (Levels outside the depth, which only a damaged file
    /// gives, are clamped, taken as two's complement.)
    fn finished(&self, (width, height): (usize, usize), side: usize, depth: u32) -> Option<Plane> {
        if self.width < width || self.height < height {
            return None;
        }
        let (wide, most) = (width.div_ceil(side), (1i32 << depth) - 1);
        let level = |at: usize| if depth == 16 { self.samples[at] } else { (self.samples[at] as i16 as i32).clamp(0, most) as u16 };
        let sample = |at: usize| level((at / (wide * side)) * self.width + (at % (wide * side)).min(width - 1));
        let (deep, high) = ((0..wide * side * height).map(sample).collect(), height.div_ceil(side));
        let (across, down, samples, waves) = (self.across, self.down, Vec::new(), Vec::new());
        Some(Plane { name: self.name, across, down, divider: 0, wide, high, side, samples, deep, waves, brought: [Some(0); 64], dividers: [0; 64] })
    }
}

/// Places a block's 8 by 8 samples into its plane (rows `stride` long), first sample at
/// `corner`.
fn put<T: Copy>(samples: &mut [T], corner: usize, stride: usize, block: &[T; 64]) {
    for (y, row) in block.as_chunks::<8>().0.iter().enumerate() {
        samples[corner + y * stride..][..8].copy_from_slice(row);
    }
}

/// A row upsampled 2× from its column sums. Most images subsample so, and it has its own fast
/// path: each sample is a quarter of the way from one sum to the next. `both` says whether the
/// plane is also subsampled vertically, which swaps the rounding alternation.
fn double(sums: &[u16], line: &mut [u8], both: bool) {
    let mix = |near: u16, far: u16, up: bool| ((near as u32 * 48 + far as u32 * 16 + 2047 + up as u32) >> 12) as u8;
    line[0] = mix(sums[0], sums[0], both);
    for (pair, sums) in line[1..].as_chunks_mut::<2>().0.iter_mut().zip(sums.windows(2)) {
        *pair = [mix(sums[0], sums[1], !both), mix(sums[1], sums[0], both)];
    }
}

/// How one image sample is interpolated from a subsampled row: the sample before it, the weight
/// of the next (out of 64), and the rounding bias.
struct Spot {
    before: u32,
    far: u16,
    round: u16,
}

/// Where image sample `at` falls in a plane with `coarse` samples per `fine` image samples: the
/// plane sample before it, and how far towards the next, out of 64. (Before the first one's
/// centre, it is that one.) Where not `smooth`, as lossless, nothing is interpolated: each
/// plane sample covers its image samples.
fn between(at: usize, coarse: usize, fine: usize, smooth: bool) -> (usize, usize) {
    if !smooth {
        return (at * coarse / fine, 0);
    }
    // In half image samples, where a sample's centre is a whole number.
    let Some(place) = ((2 * at + 1) * coarse).checked_sub(fine) else { return (0, 0) };
    (place / (2 * fine), place % (2 * fine) * 32 / fine)
}

/// What each level of a `depth`-bit sample becomes in 8 bits: a deeper one's top 8 bits, a
/// shallower one scaled so the top level is 255. (libjpeg's tools write such images as PNG,
/// deeper ones scaled to 16 bits; on an 8-bit screen that shows the same, level for level.)
fn shown(depth: u32) -> Vec<u8> {
    let most = (1u32 << depth) - 1;
    (0..=most).map(|level| if depth > 8 { level >> (depth - 8) } else { (level * 255 + most / 2) / most } as u8).collect()
}

/// A row of wide samples as the RGB bytes `colour` takes, with `shown` mapping each level.
/// Converted at their own depth first, as libjpeg does: YCbCr to RGB, CMYK multiplied out.
fn narrow(kind: Kind, shown: &[u8], wide_lines: &[Vec<u16>; 4], lines: &mut [Vec<u8>; 4]) {
    let most = shown.len() as i64 - 1;
    let printed = |ink: i64, black: i64| (ink * black + most / 2) / most;
    for x in 0..wide_lines[0].len() {
        let [first, second, third, fourth] = wide_lines.each_ref().map(|line| (line[x] as i64).min(most));
        let colours = match kind {
            Kind::Grey => [first; 3],
            Kind::Colours => [first, second, third],
            Kind::Bright => lit_at(first, second, third, most),
            Kind::Inks => [first, second, third].map(|ink| printed(ink, fourth)),
            Kind::BrightInks => lit_at(first, second, third, most).map(|light| printed(most - light, fourth)),
        };
        for (line, level) in lines.iter_mut().zip(colours) {
            line[x] = shown[level as usize];
        }
    }
}

/// A row of pixels from the same row of every plane.
fn colour(kind: Kind, lines: &[Vec<u8>; 4], row: &mut [u32]) {
    match kind {
        Kind::Grey => paint(lines, row, |grey, _, _, _| [grey; 3]),
        Kind::Colours => paint(lines, row, |red, green, blue, _| [red, green, blue]),
        Kind::Bright => paint(lines, row, |bright, blue, red, _| lit(bright, blue, red)),
        Kind::Inks => paint(lines, row, |cyan, magenta, yellow, black| [cyan, magenta, yellow].map(|ink| printed(ink, black))),
        Kind::BrightInks => paint(lines, row, |bright, blue, red, black| lit(bright, blue, red).map(|light| printed(255 - light, black))),
    }
}

/// Fills a pixel row from the four planes' samples. (One rule for the whole row, so it
/// vectorizes.)
fn paint(lines: &[Vec<u8>; 4], row: &mut [u32], pixel: impl Fn(u8, u8, u8, u8) -> [u8; 3]) {
    let [first, second, third, fourth] = lines.each_ref().map(|line| &line[..row.len()]);
    for (x, place) in row.iter_mut().enumerate() {
        let [red, green, blue] = pixel(first[x], second[x], third[x], fourth[x]);
        *place = 0xff00_0000 | (red as u32) << 16 | (green as u32) << 8 | blue as u32;
    }
}

/// RGB from YCbCr (128 is neutral chroma). The standard's coefficients times 65536. (Always
/// inlined, as `take` is, and for the same reason: it runs once per pixel.)
#[inline(always)]
fn lit(bright: u8, blue: u8, red: u8) -> [u8; 3] {
    let (bright, blue, red) = (bright as i32, blue as i32 - 128, red as i32 - 128);
    [bright + ((91881 * red + 32768) >> 16), bright + ((32768 - 22554 * blue - 46802 * red) >> 16), bright + ((116130 * blue + 32768) >> 16)].map(|part| part.clamp(0, 255) as u8)
}

/// The same at any depth, `most` being the top level: chroma is neutral at the middle level.
fn lit_at(bright: i64, blue: i64, red: i64, most: i64) -> [i64; 3] {
    let (blue, red) = (blue - (most + 1) / 2, red - (most + 1) / 2);
    [bright + ((91881 * red + 32768) >> 16), bright + ((32768 - 22554 * blue - 46802 * red) >> 16), bright + ((116130 * blue + 32768) >> 16)].map(|part| part.clamp(0, most))
}

/// The lightness of paper under an ink and black, each stored inverted.
fn printed(ink: u8, black: u8) -> u8 {
    ((ink as u32 * black as u32 + 127) / 255) as u8
}

/// A block's 8 by 8 samples from its 64 coefficients: each times its quantizer gives a basis
/// pattern's strength, and the samples are the sum of the patterns (an inverse DCT), first down
/// columns, then along rows. Integer arithmetic in libjpeg's steps, so images match most other
/// programs to the bit. (Sums wrap where a damaged file overflows 32 bits; a sound one never
/// does.) Samples have `DEPTH` bits, 8 or 12; `sample` maps each value, centred on zero (before
/// the level shift), to the plane's storage.
fn samples<const DEPTH: usize, T: Copy + Default>(waves: &[i16; 64], dividers: &[u16; 64], sample: impl Fn(i32) -> T) -> [T; 64] {
    // Between the two passes, deeper samples keep sums at half precision: they need the room.
    // The final rounding makes up for it.
    let (coarse, fine) = if DEPTH == 8 { (11, 18) } else { (12, 17) };
    let sample = |sum: Wrapping<i32>| sample(((sum + Wrapping(1 << (fine - 1))) >> fine).0);
    let whole = |at: usize| Wrapping(waves[at] as i32 * dividers[at] as i32);
    // Much of a smooth image is DC-only blocks.
    if waves[1..].iter().all(|wave| *wave == 0) {
        return [sample(whole(0) << (fine - 3)); 64];
    }
    let mut grid = [Wrapping(0); 64];
    for (at, place) in ZIGZAG.iter().enumerate() {
        grid[*place as usize] = whole(at);
    }
    let mut half = [Wrapping(0); 64];
    for x in 0..8 {
        let column = sum_of_waves(std::array::from_fn(|y| grid[y * 8 + x]));
        (0..8).for_each(|y| half[y * 8 + x] = (column[y] + Wrapping(1 << (coarse - 1))) >> coarse);
    }
    let mut samples = [T::default(); 64];
    for (row, samples) in half.as_chunks::<8>().0.iter().zip(samples.as_chunks_mut::<8>().0) {
        *samples = sum_of_waves(*row).map(sample);
    }
    samples
}

/// Eight samples from eight coefficients, times 8192: the inverse DCT with the fewest
/// multiplications (Loeffler, Ligtenberg and Moschytz), integer coefficients times 8192 too.
fn sum_of_waves(v: [Wrapping<i32>; 8]) -> [Wrapping<i32>; 8] {
    let times = |sum: Wrapping<i32>, by: i32| sum * Wrapping(by);
    // The even part…
    let shared = times(v[2] + v[6], 4433);
    let (a, b) = (shared + times(v[2], 6270), shared - times(v[6], 15137));
    let (c, d) = ((v[0] + v[4]) << 13, (v[0] - v[4]) << 13);
    let even = [c + a, d + b, d - b, c - a];
    // …and the odd part.
    let shared = times(v[1] + v[3] + v[5] + v[7], 9633);
    let (p, q) = (shared - times(v[7] + v[3], 16069), shared - times(v[5] + v[1], 3196));
    let (r, s) = (times(v[7] + v[1], -7373), times(v[5] + v[3], -20995));
    let odd = [times(v[1], 12299) + r + q, times(v[3], 25172) + s + p, times(v[5], 16819) + s + q, times(v[7], 2446) + r + p];
    [even[0] + odd[0], even[1] + odd[1], even[2] + odd[2], even[3] + odd[3], even[3] - odd[3], even[2] - odd[2], even[1] - odd[1], even[0] - odd[0]]
}

/// One block of an MCU: its plane, its DC and AC tables, and its position in an MCU holding
/// `step` blocks of the plane across and down.
struct Slot {
    plane: usize,
    codes: (usize, usize),
    step: (usize, usize),
    across: usize,
    down: usize,
}

/// A scan being decoded: its data, and what carries over between blocks. (`repr(C)` with the
/// common kinds' fields first: field reordering by the compiler made them measurably slower.)
#[repr(C)]
struct Pass<'a> {
    /// The Huffman-coded data.
    bits: Bits<'a>,
    /// The first and last coefficient the scan brings.
    from: usize,
    to: usize,
    /// The value of the lowest bit the scan brings: later scans bring lower ones.
    step: i16,
    /// A refinement scan: an earlier one started these coefficients, and this brings a bit
    /// more.
    more: bool,
    /// The previous block's DC per plane: the next is stored as a difference…
    before: [i32; 4],
    /// …unless this is zero (in a differential frame), not all ones: then each is stored as is.
    carried: i32,
    /// The count of following blocks with nothing new (end-of-band run).
    empty: u32,
    /// For lossless: the predictor (one of seven, or none in a differential frame), the point
    /// transform, and the MCU row of the last restart.
    rule: u8,
    shift: u32,
    top: usize,
    /// The arithmetic-coded data.
    answers: Answers<'a>,
    /// The arithmetic decoder's contexts: one per kind of decision, four sets for DC then four
    /// for AC. A context is the more probable symbol (128 for one) and its state (an index into
    /// `GUESSES`); all start as "zero, equally likely".
    guesses: Vec<[u8; 256]>,
    /// The context each plane's next DC starts with.
    leaning: [usize; 4],
    /// Lossless and arithmetic coded: per plane, how each sample's coded difference conditions
    /// the decisions of those right of and below it (see `condition`).
    conditions: Vec<Vec<u8>>,
}

impl Pass<'_> {
    /// Reads an MCU: for each block, what the scan brings of it.
    fn unit(&mut self, column: usize, row: usize, slots: &[Slot], jpeg: &mut Jpeg) -> Option<()> {
        for slot in slots {
            let (plane, averages, details) = (&mut jpeg.planes[slot.plane], &jpeg.averages[slot.codes.0], &jpeg.details[slot.codes.1]);
            let block = (row * slot.step.1 + slot.down) * plane.wide + column * slot.step.0 + slot.across;
            if !jpeg.steps {
                let mut waves = [0; 64];
                self.whole(averages, details, slot.plane, &mut waves)?;
                plane.draw(block, &waves, jpeg.blocks);
                continue;
            }
            let waves = plane.waves.as_chunks_mut::<64>().0.get_mut(block)?;
            match (self.from == 0, self.more) {
                (true, false) => waves[0] = self.average(averages, slot.plane)?.wrapping_mul(self.step as i32) as i16,
                (true, true) => waves[0] |= self.bits.take(1) as i16 * self.step,
                (false, false) => self.rest(details, waves)?,
                (false, true) => self.sharpen(details, waves)?,
            }
        }
        Some(())
    }

    /// The next restart interval, in MCU row `row`. The data restarts on a byte boundary after
    /// a marker cycling RST0 to RST7, so damage doesn't spread past it. Here the marker shows
    /// whether one is missing.
    fn restart(&mut self, stretch: usize, row: usize, arithmetic: bool) -> Option<()> {
        let (bits, answers, number) = (&mut self.bits, &mut self.answers, 0xD0 + (stretch + 7) as u8 % 8);
        (self.before, self.empty, self.top) = ([0; 4], 0, row);
        // With arithmetic coding, an interval needn't have been read to its last byte: the
        // marker is the next one wherever reading stands. And the contexts reset.
        if arithmetic {
            let (kind, _, after) = part(answers.data.get(answers.at..)?)?;
            (answers.at, self.leaning) = (answers.data.len() - after.len(), [0; 4]);
            self.guesses.fill([0; 256]);
            answers.begin();
            return (kind == number).then_some(());
        }
        let mark = bits.at + bits.data.get(bits.at..)?.iter().position(|byte| *byte != 0xFF)?;
        if mark == bits.at || bits.count < 0 || bits.data[mark] != number {
            return None;
        }
        (bits.at, bits.held, bits.count) = (mark + 1, 0, 0);
        Some(())
    }

    /// A block's DC: its difference from the previous block's, as a bit count then the bits.
    fn average(&mut self, codes: &Codes, plane: usize) -> Option<i32> {
        let size = self.bits.symbol(codes)? as u32;
        self.before[plane] = (self.before[plane] & self.carried).wrapping_add(self.bits.number(size));
        Some(self.before[plane])
    }

    /// A whole block, in a baseline image. After its DC, each symbol gives the zero run before
    /// the next coefficient and its bit count; symbol zero-zero ends the block.
    fn whole(&mut self, averages: &Codes, details: &Codes, plane: usize, waves: &mut [i16; 64]) -> Option<()> {
        waves[0] = self.average(averages, plane)? as i16;
        let mut at = 1;
        while at < 64 {
            let symbol = self.bits.symbol(details)?;
            let (noughts, size) = ((symbol >> 4) as usize, (symbol & 15) as u32);
            if size == 0 && noughts != 15 {
                break;
            }
            // ZRL: fifteen zeros and a zero-bit coefficient are sixteen zeros.
            at += noughts;
            *waves.get_mut(at)? = self.bits.number(size) as i16;
            at += 1;
        }
        Some(())
    }

    /// A first AC scan of a band: as a whole block, but only the high bits, and the
    /// end-of-block symbol can say the following blocks have nothing in the band either.
    fn rest(&mut self, codes: &Codes, waves: &mut [i16; 64]) -> Option<()> {
        let mut at = if self.empty > 0 { 64 } else { self.from };
        self.empty = self.empty.saturating_sub(1);
        while at <= self.to {
            let symbol = self.bits.symbol(codes)?;
            let (noughts, size) = ((symbol >> 4) as usize, (symbol & 15) as u32);
            if size == 0 && noughts != 15 {
                self.empty = (1 << noughts) - 1 + self.bits.take(noughts as u32);
                break;
            }
            at += noughts;
            *waves.get_mut(at)? = (self.bits.number(size) * self.step as i32) as i16;
            at += 1;
        }
        Some(())
    }

    /// An AC refinement scan: one more bit of every known coefficient, and among them, those
    /// that start at this bit. A symbol gives the count of zero coefficients before a new one;
    /// the refinement bits of known ones on the way follow it.
    fn sharpen(&mut self, codes: &Codes, waves: &mut [i16; 64]) -> Option<()> {
        let mut at = self.from;
        while self.empty == 0 && at <= self.to {
            let symbol = self.bits.symbol(codes)?;
            let (noughts, size) = ((symbol >> 4) as usize, symbol & 15);
            if size == 0 && noughts != 15 {
                self.empty = (1 << noughts) + self.bits.take(noughts as u32);
                break;
            }
            // A new coefficient is one step either way; its sign comes before the refinement
            // bits.
            let new = if size == 0 { 0 } else { self.step * (self.bits.take(1) as i16 * 2 - 1) };
            at = self.walk(waves, at, noughts);
            if at > self.to {
                return None;
            }
            waves[at] = new;
            at += 1;
        }
        if self.empty > 0 {
            self.walk(waves, at, 64);
            self.empty -= 1;
        }
        Some(())
    }

    /// Walks on from `at` through the band: a known coefficient gets its next bit, a zero one
    /// is skipped `noughts` times. Returns where it stopped: at the zero after those, or past
    /// the band.
    fn walk(&mut self, waves: &mut [i16; 64], mut at: usize, mut noughts: usize) -> usize {
        while at <= self.to && (waves[at] != 0 || noughts > 0) {
            let known = waves[at];
            noughts -= (known == 0) as usize;
            // The bit says whether the coefficient moves one step further from zero.
            if known != 0 && self.bits.take(1) == 1 && (known & self.step) == 0 {
                waves[at] = known.wrapping_add(if known > 0 { self.step } else { -self.step });
            }
            at += 1;
        }
        at
    }

    /// Reads a lossless MCU: for each sample, its difference from the prediction from the
    /// samples left, above, and above-left. Differences are coded as DC is: bit count, then the
    /// bits.
    fn exact_unit(&mut self, column: usize, row: usize, slots: &[Slot], jpeg: &mut Jpeg) -> Option<()> {
        for slot in slots {
            let (plane, codes) = (&mut jpeg.planes[slot.plane], &jpeg.averages[slot.codes.0]);
            let (x, y) = (column * slot.step.0 + slot.across, row * slot.step.1 + slot.down);
            let size = self.bits.symbol(codes)? as u32;
            // The one 16-bit difference has no bits after its size.
            let change = if size == 16 { 32768 } else { self.bits.number(size) };
            self.place(plane, (x, y), y == self.top * slot.step.1, change, jpeg.depth)?;
        }
        Some(())
    }

    /// Reads an arithmetic-coded lossless MCU: as `exact_unit`, but each difference decided as
    /// an arithmetic-coded DC is, the decisions conditioned on the differences left of and
    /// above it (none at a row's start, nor in the first row after a restart).
    fn arithmetic_exact_unit(&mut self, column: usize, row: usize, slots: &[Slot], jpeg: &mut Jpeg) -> Option<()> {
        for slot in slots {
            let plane = &mut jpeg.planes[slot.plane];
            let (x, y) = (column * slot.step.0 + slot.across, row * slot.step.1 + slot.down);
            let (at, first_row, conditions) = (y * plane.wide + x, y == self.top * slot.step.1, &mut self.conditions[slot.plane]);
            let left = if x == 0 { 0 } else { conditions[at - 1] as usize };
            let above = if first_row { 0 } else { conditions[at - plane.wide] as usize };
            // Magnitudes have contexts of their own after a large difference above.
            let (guesses, sizes) = (&mut self.guesses[slot.codes.0], if above >= 3 { 129 } else { 100 });
            let change = self.answers.difference(guesses, 20 * left + 4 * above, sizes)?;
            *conditions.get_mut(at)? = condition(change, jpeg.bounds[slot.codes.0]) as u8;
            self.place(plane, (x, y), first_row, change, jpeg.depth)?;
        }
        Some(())
    }

    /// Stores a lossless sample at `x`, `y` of `plane`: its prediction from the samples left,
    /// above and above-left, plus `change`, the point transform's bits put back. The first row
    /// after a restart has nothing above, and predicts from the left; its first sample from the
    /// middle of the range. Any other row's first sample predicts from above. The rest use the
    /// scan's predictor. A differential frame predicts nothing.
    fn place(&self, plane: &mut Plane, (x, y): (usize, usize), first_row: bool, change: i32, depth: u32) -> Option<()> {
        let at = y * plane.wide + x;
        let near = |back: usize| at.checked_sub(back).map_or(0, |at| (plane.deep[at] >> self.shift) as i32);
        let (before, above, slant) = (near(1), near(plane.wide), near(plane.wide + 1));
        let guess = match (self.rule, first_row, x == 0) {
            (0, ..) => 0,
            (_, true, true) => 1 << (depth - self.shift - 1),
            (_, true, false) | (1, false, false) => before,
            (_, false, true) | (2, ..) => above,
            (3, ..) => slant,
            (4, ..) => before + above - slant,
            (5, ..) => before + ((above - slant) >> 1),
            (6, ..) => above + ((before - slant) >> 1),
            _ => (before + above) >> 1,
        };
        *plane.deep.get_mut(at)? = ((guess + change) << self.shift) as u16;
        Some(())
    }

    /// Reads an arithmetic-coded MCU. A baseline image reads as a progressive one whose one
    /// scan brings whole blocks: DC, then AC.
    fn arithmetic_unit(&mut self, column: usize, row: usize, slots: &[Slot], jpeg: &mut Jpeg) -> Option<()> {
        for slot in slots {
            let plane = &mut jpeg.planes[slot.plane];
            let block = (row * slot.step.1 + slot.down) * plane.wide + column * slot.step.0 + slot.across;
            let mut whole = [0; 64];
            let waves = if jpeg.steps { plane.waves.as_chunks_mut::<64>().0.get_mut(block)? } else { &mut whole };
            match (self.from == 0, self.more) {
                (true, false) => waves[0] = self.arithmetic_average(slot, jpeg.bounds[slot.codes.0])?.wrapping_mul(self.step as i32) as i16,
                (true, true) => waves[0] |= self.answers.either() as i16 * self.step,
                _ => {}
            }
            match (self.to > 0, self.more) {
                (true, false) => self.arithmetic_rest(slot, jpeg.edges[slot.codes.1] as usize, waves)?,
                (true, true) => self.arithmetic_sharpen(slot, waves)?,
                _ => {}
            }
            if !jpeg.steps {
                plane.draw(block, &whole, jpeg.blocks);
            }
        }
        Some(())
    }

    /// An arithmetic-coded DC: its difference from the previous block's. The contexts depend on
    /// how the previous block differed (see `condition`, which `bounds` set).
    fn arithmetic_average(&mut self, slot: &Slot, bounds: [u8; 2]) -> Option<i32> {
        let (guesses, leaning) = (&mut self.guesses[slot.codes.0], &mut self.leaning[slot.plane]);
        let change = self.answers.difference(guesses, std::mem::take(leaning), 20)?;
        *leaning = 4 * condition(change, bounds);
        self.before[slot.plane] = (self.before[slot.plane] & self.carried).wrapping_add(change);
        Some(self.before[slot.plane])
    }

    /// An arithmetic first AC scan of a band. Before each coefficient, end-of-block; then, per
    /// position, whether it is zero; then sign and magnitude. Each position has its own
    /// contexts for these. Magnitude contexts are shared by positions up to `edge`, and by
    /// those after.
    fn arithmetic_rest(&mut self, slot: &Slot, edge: usize, waves: &mut [i16; 64]) -> Option<()> {
        let (answers, guesses, mut at) = (&mut self.answers, &mut self.guesses[4 + slot.codes.1], self.from.max(1));
        while at <= self.to && !answers.answer(&mut guesses[3 * at - 3]) {
            while at <= self.to && !answers.answer(&mut guesses[3 * at - 2]) {
                at += 1;
            }
            // A block that doesn't end has a coefficient to come.
            if at > self.to {
                return None;
            }
            let lower = answers.either();
            // "Above one" and then "above two" share a context.
            let by = match () {
                _ if !answers.answer(&mut guesses[3 * at - 1]) => 1,
                _ if !answers.answer(&mut guesses[3 * at - 1]) => 2,
                _ => answers.size(guesses, if at <= edge { 189 } else { 217 }, 2)?,
            };
            waves[at] = (if lower { -by } else { by }).wrapping_mul(self.step as i32) as i16;
            at += 1;
        }
        Some(())
    }

    /// An arithmetic AC refinement scan: per known coefficient, whether it moves a step further
    /// from zero; per zero one, whether it starts at this bit, then its sign.
    fn arithmetic_sharpen(&mut self, slot: &Slot, waves: &mut [i16; 64]) -> Option<()> {
        let (answers, guesses) = (&mut self.answers, &mut self.guesses[4 + slot.codes.1]);
        // End-of-block is decoded before each zero run, but not before the last coefficient
        // earlier scans started: until there it can't end.
        let known = (self.from..=self.to).rev().find(|at| waves[*at] != 0).unwrap_or(0);
        let (mut at, mut run) = (self.from, false);
        while at <= self.to {
            if !run && at > known && answers.answer(&mut guesses[3 * at - 3]) {
                return Some(());
            }
            let wave = &mut waves[at];
            let step = if *wave < 0 { -self.step } else { self.step };
            if *wave != 0 {
                *wave = wave.wrapping_add(step * answers.answer(&mut guesses[3 * at - 1]) as i16);
            } else if answers.answer(&mut guesses[3 * at - 2]) {
                *wave = if answers.either() { -step } else { step };
            }
            (run, at) = (*wave == 0, at + 1);
        }
        // A block that doesn't end has a coefficient to come.
        (!run).then_some(())
    }
}

/// How a coded difference conditions the decisions after it: 0 about zero, 1 and 2 small
/// (positive, negative), 3 and 4 large. Small is above `2^(low - 1)` (above zero where `low` is
/// 0), and up to `2^high`.
fn condition(change: i32, [low, high]: [u8; 2]) -> usize {
    let (size, below) = (change.unsigned_abs(), (change < 0) as usize);
    match () {
        _ if size <= (1 << low) >> 1 => 0,
        _ if size <= 1 << high => 1 + below,
        _ => 3 + below,
    }
}

/// The QM-coder's 113 probability states plus one fixed. Each packs three values: the high half
/// is Qe, the LPS interval size (out of 0x8000..0x10000 for all); then a byte each, the next
/// state after an LPS, and after an MPS. (Where the first has 0x80 added, the MPS flips there.)
/// Every decoder must transition exactly so; the values are the standard's.
const GUESSES: [u32; 114] = [
    0x5a1d8101, 0x25860e02, 0x11141003, 0x080b1204, 0x03d81405, 0x01da1706, 0x00e51907, 0x006f1c08, 0x00361e09, 0x001a210a, 0x000d230b, 0x0006090c, 0x00030a0d, 0x00010c0d, 0x5a7f8f0f, 0x3f252410,
    0x2cf22611, 0x207c2712, 0x17b92813, 0x11822a14, 0x0cef2b15, 0x09a12d16, 0x072f2e17, 0x055c3018, 0x04063119, 0x0303331a, 0x0240341b, 0x01b1361c, 0x0144381d, 0x00f5391e, 0x00b73b1f, 0x008a3c20,
    0x00683e21, 0x004e3f22, 0x003b2023, 0x002c2109, 0x5ae1a525, 0x484c4026, 0x3a0d4127, 0x2ef14328, 0x261f4429, 0x1f33452a, 0x19a8462b, 0x1518482c, 0x1177492d, 0x0e744a2e, 0x0bfb4b2f, 0x09f84d30,
    0x08614e31, 0x07064f32, 0x05cd3033, 0x04de3234, 0x040f3235, 0x03633336, 0x02d43437, 0x025c3538, 0x01f83639, 0x01a4373a, 0x0160383b, 0x0125393c, 0x00f63a3d, 0x00cb3b3e, 0x00ab3d3f, 0x008f3d20,
    0x5b12c141, 0x4d045042, 0x412c5143, 0x37d85244, 0x2fe85345, 0x293c5446, 0x23795647, 0x1edf5748, 0x1aa95749, 0x174e484a, 0x1424484b, 0x119c4a4c, 0x0f6b4a4d, 0x0d514b4e, 0x0bb64d4f, 0x0a404d30,
    0x5832d051, 0x4d1c5852, 0x438e5953, 0x3bdd5a54, 0x34ee5b55, 0x2eae5c56, 0x299a5d57, 0x25165647, 0x5570d859, 0x4ca95f5a, 0x44d9605b, 0x3e22615c, 0x3824635d, 0x32b4635e, 0x2e175d56, 0x56a8df60,
    0x4f466561, 0x47e56662, 0x41cf6763, 0x3c3d6864, 0x375e635d, 0x52316966, 0x4c0f6a67, 0x46396b68, 0x415e6763, 0x5627e96a, 0x50e76c6b, 0x4b856d67, 0x55976e6d, 0x504f6f6b, 0x5a10ee6f, 0x5522706d,
    0x59ebf06f, 0x5a1d7171,
];

/// An arithmetic-coded data stream, read as binary decisions. A restart interval is one long
/// number. Before each decision, the possible values form an interval; the decision splits it,
/// the smaller part for the less probable symbol, and where the number lies is the result. So a
/// near-certain decision narrows it little, costing a fraction of a bit.
struct Answers<'a> {
    data: &'a [u8],
    at: usize,
    /// How far the number lies above the interval's base, as far as read.
    above: u32,
    /// The interval's width: doubled whenever below 0x8000, so 16 bits always measure it finely
    /// enough.
    span: u32,
    /// Bits of the number read beyond the interval's precision: `above` is that much finer. A
    /// byte is read at a time, and each doubling uses a bit.
    ahead: u32,
}

impl Answers<'_> {
    /// Starts an interval: the number's first 16 bits, in the widest interval.
    fn begin(&mut self) {
        (self.above, self.span, self.ahead) = (self.byte() << 8 | self.byte(), 0x10000, 0);
    }

    /// The number's next byte. Its end reads as zeros: at the file's end, or at a 0xFF starting
    /// the next marker (a data 0xFF is followed by a stuffed zero).
    fn byte(&mut self) -> u32 {
        let Some(&byte) = self.data.get(self.at) else { return 0 };
        if byte == 0xFF && self.data.get(self.at + 1) != Some(&0) {
            return 0;
        }
        self.at += 1 + (byte == 0xFF) as usize;
        byte as u32
    }

    /// The next decision, with its context; the context adapts to the result.
    fn answer(&mut self, guess: &mut u8) -> bool {
        // The interval is kept at least half its full width: narrower, it is doubled and the
        // number reads another bit, until it isn't.
        while self.span < 0x8000 {
            if self.ahead == 0 {
                (self.above, self.ahead) = (self.above << 8 | self.byte(), 8);
            }
            (self.span, self.ahead) = (self.span << 1, self.ahead - 1);
        }
        let (step, likely) = (GUESSES[(*guess & 127) as usize], *guess & 128);
        // The LPS gets the upper part, as wide as the state says; but where that leaves the
        // lower part smaller, the two swap.
        let (rare, lower) = (step >> 16, self.span - (step >> 16));
        let upper = self.above >= lower << self.ahead;
        let unlikely = upper != (lower < rare);
        self.above -= if upper { lower << self.ahead } else { 0 };
        self.span = if upper { rare } else { lower };
        // A context only adapts when the interval must be doubled.
        if self.span < 0x8000 {
            *guess = likely ^ if unlikely { (step >> 8) as u8 } else { step as u8 };
        }
        (likely != 0) != unlikely
    }

    /// A difference coded as DC differences are: whether it is zero (with the context at
    /// `first`), whether negative (`first + 1`), whether beyond one (`first + 2`, or `+ 3` when
    /// negative), and its magnitude (contexts from `sizes`).
    fn difference(&mut self, guesses: &mut [u8], first: usize, sizes: usize) -> Option<i32> {
        if !self.answer(&mut guesses[first]) {
            return Some(0);
        }
        let lower = self.answer(&mut guesses[first + 1]);
        let by = if self.answer(&mut guesses[first + 2 + lower as usize]) { self.size(guesses, sizes, 1)? } else { 1 };
        Some(if lower { -by } else { by })
    }

    /// A decision equally likely either way, such as a sign: with the fixed state.
    fn either(&mut self) -> bool {
        self.answer(&mut 113)
    }

    /// The magnitude of a value known to exceed `top` (1 or 2). First how many times `top`
    /// doubles without passing the value minus one, each doubling a decision with its own
    /// context from `at`. Then the bits below the top one, all with the context 14 further on.
    /// `None` for a magnitude larger than a block can hold.
    fn size(&mut self, guesses: &mut [u8], mut at: usize, mut top: i32) -> Option<i32> {
        while self.answer(&mut guesses[at]) {
            (at, top) = (at + 1, top << 1);
            if top == 0x8000 {
                return None;
            }
        }
        let below = (0..top.trailing_zeros()).rev().filter(|_| self.answer(&mut guesses[at + 14])).fold(0, |below, bit| below | 1 << bit);
        Some(top + below + 1)
    }
}

/// Huffman-coded data, read a few bits at a time from each byte's high end.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    /// The next bits, from the top.
    held: u64,
    /// How many are from the file. Past the data's end the bits read as zeros, and this goes
    /// negative once more were taken than existed.
    count: i32,
}

impl Bits<'_> {
    /// Reads bytes until at least 57 bits are held or the data ends: at the file's end, or at a
    /// 0xFF starting the next marker. (A data 0xFF is followed by a stuffed zero.)
    fn fill(&mut self) {
        while self.count <= 56 {
            let Some(&byte) = self.data.get(self.at) else { return };
            if byte == 0xFF && self.data.get(self.at + 1) != Some(&0) {
                return;
            }
            self.held |= (byte as u64) << (56 - self.count);
            (self.at, self.count) = (self.at + 1 + (byte == 0xFF) as usize, self.count + 8);
        }
    }

    /// The next `count` bits, at most 16, as a number. (Always inlined. Otherwise a size build
    /// keeps it a call for every value and bit, and its speed depends on where the call lands.)
    #[inline(always)]
    fn take(&mut self, count: u32) -> u32 {
        if self.count < 16 {
            self.fill();
        }
        let bits = (self.held >> 32 >> (32 - count)) as u32;
        (self.held, self.count) = (self.held << count, self.count - count as i32);
        bits
    }

    /// A value coded in `size` bits: the upper half of the range is values needing exactly that
    /// many bits, the lower half the same negated.
    fn number(&mut self, size: u32) -> i32 {
        let (bits, most) = (self.take(size) as i32, (1 << size) - 1);
        if bits > most >> 1 { bits } else { bits - most }
    }

    /// The next symbol of a Huffman table: by the next nine bits where its code is that short,
    /// else trying longer lengths in turn.
    fn symbol(&mut self, codes: &Codes) -> Option<u8> {
        if self.count < 32 {
            self.fill();
        }
        let ahead = (self.held >> 48) as u32;
        let quick = codes.quick[(ahead >> 7) as usize];
        let long = if quick != 0 { (quick >> 8) as usize } else { (10..=16).find(|long| ahead < codes.past[*long])? };
        (self.held, self.count) = (self.held << long, self.count - long as i32);
        if quick != 0 {
            return Some(quick as u8);
        }
        codes.symbols.get(((ahead >> (16 - long)) as i32 + codes.place[long]) as usize).copied()
    }
}

/// A Huffman table laid out for fast decoding. The file gives the code count per length and the
/// symbols in code order; the codes count up from zero, appending a zero bit at each new
/// length.
struct Codes {
    /// For any nine bits: the symbol whose code they start with, plus its length times 256.
    /// Zero for codes longer than nine bits.
    quick: [u16; 512],
    /// For each longer length: the first 16-bit value past its codes…
    past: [u32; 17],
    /// …and the offset from a code of that length to its symbol index.
    place: [i32; 17],
    symbols: [u8; 256],
}

impl Codes {
    /// A table never defined: nothing decodes with it.
    const NONE: Codes = Codes { quick: [0; 512], past: [0; 17], place: [0; 17], symbols: [0; 256] };

    /// From the code count per length (1 to 16) and the symbols.
    fn new(counts: &[u8], symbols: &[u8]) -> Option<Self> {
        let mut codes = Codes::NONE;
        codes.symbols.get_mut(..symbols.len())?.copy_from_slice(symbols);
        let lengths = counts.iter().enumerate().flat_map(|(long, count)| std::iter::repeat_n(long + 1, *count as usize));
        let (mut code, mut last) = (0u32, 0);
        for (at, (long, symbol)) in lengths.zip(symbols).enumerate() {
            (code, last) = (code << (long - last), long);
            // More codes than the length has room for.
            if code >= 1 << long {
                return None;
            }
            (codes.past[long], codes.place[long]) = ((code + 1) << (16 - long), at as i32 - code as i32);
            let nines = if long <= 9 { (code << (9 - long)) as usize..((code + 1) << (9 - long)) as usize } else { 0..0 };
            codes.quick[nines].fill((long as u16) << 8 | *symbol as u16);
            code += 1;
        }
        Some(codes)
    }
}

/// Encodes an image as JPEG. `width` is its width in pixels; `pixels` are rows from the top,
/// `0xRRGGBB` (the alpha byte, which JPEG cannot store, is ignored). `quality` 1 to 100 sets
/// how much detail is kept, as every JPEG writer uses it: the standard's quantization tables
/// scaled as libjpeg scales them, and the output is byte for byte what libjpeg's
/// `cjpeg -baseline -quality` writes. Below 90, chroma is halved each way, as cameras and the
/// web do; from 90, full. `None` for an empty image, or one over 65,535 pixels either way (JPEG
/// stores two bytes each). [`Encoder`] writes progressive JPEGs and metadata too.
pub fn encode(width: usize, pixels: &[u32], quality: u8) -> Option<Vec<u8>> {
    Encoder::new(quality).encode(width, pixels)
}

/// How a JPEG is written: [`encode`]'s quality, and optionally progressive, with an EXIF
/// orientation, an ICC profile and XMP.
///
/// ```
/// let pixels = vec![0xff20_8040; 64 * 48];
/// // An ICC profile's bytes, where the pixels are in another space than sRGB.
/// let profile = [0u8; 0];
/// let jpeg = taut::jpeg::Encoder::new(85).progressive().orientation(6).icc_profile(&profile);
/// assert!(jpeg.encode(64, &pixels).is_some());
/// ```
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Encoder<'a> {
    quality: u8,
    progressive: bool,
    orientation: Option<u8>,
    icc_profile: &'a [u8],
    xmp: &'a str,
}

impl<'a> Encoder<'a> {
    /// Writes at `quality`, 1 to 100 (see [`encode`]): baseline, with no metadata.
    pub fn new(quality: u8) -> Self {
        Encoder { quality, progressive: false, orientation: None, icc_profile: &[], xmp: "" }
    }

    /// Progressive: the image comes in ten scans, coarse to fine, each with Huffman tables made
    /// for it, byte for byte as libjpeg's `cjpeg -progressive` writes it. Usually a few percent
    /// smaller than baseline, and shown blurred before it has all come.
    pub fn progressive(self) -> Self {
        Encoder { progressive: true, ..self }
    }

    /// An EXIF orientation, 1 to 8 (1 is as stored): how a viewer turns the image upright, as a
    /// camera says it was held. [`decode`] turns it so.
    pub fn orientation(self, orientation: u8) -> Self {
        Encoder { orientation: Some(orientation), ..self }
    }

    /// The ICC profile of the pixels' colours, split over as many APP2 segments as it needs, as
    /// libjpeg splits it. Empty for none (sRGB).
    pub fn icc_profile(self, profile: &'a [u8]) -> Self {
        Encoder { icc_profile: profile, ..self }
    }

    /// An XMP packet (XML metadata), up to 65,504 bytes. Empty for none.
    pub fn xmp(self, packet: &'a str) -> Self {
        Encoder { xmp: packet, ..self }
    }

    /// Encodes an image, as [`encode`] does. Also `None` for an orientation outside 1 to 8, an
    /// ICC profile over 16 MB (255 segments), or XMP over 65,504 bytes.
    pub fn encode(&self, width: usize, pixels: &[u32]) -> Option<Vec<u8>> {
        let height = pixels.len() / width.max(1);
        if width == 0 || height == 0 || width > 0xFFFF || height > 0xFFFF {
            return None;
        }
        // The standard's tables get coarser below quality 50 and finer above. Baseline, none
        // exceeds 255, the most it holds; progressive, as libjpeg has it, none 32767.
        let quality = self.quality.clamp(1, 100) as u32;
        let (scale, most) = (if quality < 50 { 5000 / quality } else { 200 - 2 * quality }, if self.progressive { 32767 } else { 255 });
        let dividers = DIVIDERS.map(|table| table.map(|divider| (divider as u32 * scale + 50) / 100).map(|divider| divider.clamp(1, most) as u16));
        let mut out = Vec::with_capacity(width * height / 4 + 1024 + self.icc_profile.len() + self.xmp.len());
        // SOI, and JFIF 1.01 (square pixels).
        out.extend([0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
        self.metadata(&mut out)?;
        frame_header(&mut out, &dividers, [width, height], quality < 90, self.progressive);
        let mut writing = Writing::new(pixels, [width, height], quality < 90, &dividers);
        if self.progressive {
            let mut kept = Kept::default();
            writing.each_unit(&mut kept);
            return Some(kept.scans(out, [width, height], writing.units, quality < 90));
        }
        let mut coded = Sequential::new(out);
        writing.each_unit(&mut coded);
        Some(coded.packed.end())
    }

    /// Writes the metadata segments: EXIF (APP1, with only the orientation), XMP (APP1), and
    /// the ICC profile (APP2, in chunks). `None` where one is out of range.
    fn metadata(&self, out: &mut Vec<u8>) -> Option<()> {
        let segment = |out: &mut Vec<u8>, kind: u8, parts: &[&[u8]]| {
            let long = 2 + parts.iter().map(|part| part.len()).sum::<usize>();
            out.extend([&[0xFF, kind][..], &(long as u16).to_be_bytes()].concat());
            parts.iter().for_each(|part| out.extend_from_slice(part));
        };
        if let Some(turn) = self.orientation {
            // A big-endian TIFF header, then one IFD of one entry: tag 0x112, a SHORT, one of it.
            let tiff = [b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, turn, 0, 0, 0, 0, 0, 0];
            (1..=8).contains(&turn).then_some(())?;
            segment(out, 0xE1, &[b"Exif\0\0", &tiff]);
        }
        if !self.xmp.is_empty() {
            (self.xmp.len() <= 65504).then_some(())?;
            segment(out, 0xE1, &[b"http://ns.adobe.com/xap/1.0/\0", self.xmp.as_bytes()]);
        }
        let chunks = self.icc_profile.chunks(65519);
        let count = u8::try_from(chunks.len()).ok()?;
        for (at, chunk) in chunks.enumerate() {
            segment(out, 0xE2, &[b"ICC_PROFILE\0", &[at as u8 + 1, count], chunk]);
        }
        Some(())
    }
}

/// Writes both quantization tables, each in zigzag order (an entry in two bytes where one
/// needs them), and the frame header: SOF0 or SOF2, 8 bits, the size, and three components,
/// the first sampled twice the others' each way or the same, each with its table.
fn frame_header(out: &mut Vec<u8>, dividers: &[[u16; 64]; 2], size: [usize; 2], half: bool, progressive: bool) {
    for (which, table) in dividers.iter().enumerate() {
        let (wide, entries) = (table.iter().any(|divider| *divider > 255), ZIGZAG.map(|at| table[at as usize]));
        out.extend([&[0xFF, 0xDB][..], &(67 + 64 * wide as u16).to_be_bytes(), &[(wide as u8) << 4 | which as u8]].concat());
        match wide {
            true => out.extend(entries.iter().flat_map(|entry| entry.to_be_bytes())),
            false => out.extend(entries.map(|entry| entry as u8)),
        }
    }
    let [high, wide] = [size[1] as u16, size[0] as u16].map(u16::to_be_bytes);
    let kind = if progressive { 0xC2 } else { 0xC0 };
    out.extend([0xFF, kind, 0, 17, 8, high[0], high[1], wide[0], wide[1], 3, 1, [0x11, 0x22][half as usize], 0, 2, 0x11, 1, 3, 0x11, 1]);
}

/// The standard's quantization tables (Annex K), row-major: luma and chroma, at quality 50.
/// (The eye misses high frequencies more than low, and fine chroma more than luma.)
const DIVIDERS: [[u8; 64]; 2] = [
    [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
        92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ],
    [
        17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
        99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    ],
];

/// The standard's Huffman tables (Annex K) as a file gives them: code count per length 1 to 16,
/// and the symbols for the shorter codes, in code order. The 16-bit codes take all remaining
/// symbols, in order. For luma DC, luma AC, chroma DC, chroma AC.
const PACKINGS: [([u8; 16], &[u8]); 4] = [
    ([0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    (
        [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125],
        &[
            0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1,
            0xf0, 0x24, 0x33, 0x62, 0x72, 0x82,
        ],
    ),
    ([0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    (
        [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119],
        &[
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52,
            0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1,
        ],
    ),
];

/// A table's symbols in code order (see `PACKINGS`). An AC symbol gives the zero run before a
/// coefficient (up to 15) and its bit count (1 to 10); plus two more, ZRL (0xF0) and EOB (0).
fn symbols((counts, given): &([u8; 16], &[u8])) -> Vec<u8> {
    let symbol = |byte: &u8| matches!(byte & 15, 1..=10) || matches!(byte, 0 | 0xF0);
    let left = (0..=255).filter(|byte| symbol(byte) && !given.contains(byte));
    let all: usize = counts.iter().map(|count| *count as usize).sum();
    given.iter().copied().chain(left).take(all).collect()
}

/// An image being encoded: the image; its MCU count across and down (an MCU is 8 by 8 pixels,
/// or 16 by 16 with chroma halved); and the samples of the current MCU row.
struct Writing<'a> {
    pixels: &'a [u32],
    size: [usize; 2],
    half: bool,
    units: [usize; 2],
    /// Samples per plane row (more than the image where it ends inside an MCU: each row's last
    /// pixel is repeated).
    stride: usize,
    /// YCbCr for every pixel of the current MCUs; with chroma halved, Cb and Cr at that size.
    planes: [Vec<u8>; 3],
    halves: [Vec<u8>; 2],
    /// Per quantization table, how each coefficient is divided (see `divide`).
    dividers: [[[u32; 3]; 64]; 2],
}

impl<'a> Writing<'a> {
    /// Starts encoding an image of `size` pixels, chroma halved or full, quantized by
    /// `dividers`.
    fn new(pixels: &'a [u32], size: [usize; 2], half: bool, dividers: &[[u16; 64]; 2]) -> Self {
        let side = if half { 16 } else { 8 };
        let units = size.map(|length| length.div_ceil(side));
        let (stride, rows) = (units[0] * side, side);
        let planes = [vec![0; stride * rows], vec![0; stride * rows], vec![0; stride * rows]];
        let halves = [vec![0; stride / 2 * 8], vec![0; stride / 2 * 8]];
        let dividers = dividers.map(|table| table.map(|divider| divide(divider as u32 * 8)));
        Writing { pixels, size, half, units, stride, planes, halves, dividers }
    }

    /// Gives every MCU's blocks to `out`, row by row.
    fn each_unit(&mut self, out: &mut impl Blocks) {
        for row in 0..self.units[1] {
            self.band(row);
            (0..self.units[0]).for_each(|column| self.unit(row, column, out));
        }
    }

    /// Fills the planes with MCU row `row`. Past the image's right and bottom, its last pixels
    /// are repeated.
    fn band(&mut self, row: usize) {
        let ([wide, high], side, stride) = (self.size, if self.half { 16 } else { 8 }, self.stride);
        let [bright, blue, red] = &mut self.planes;
        let lines = bright.chunks_exact_mut(stride).zip(blue.chunks_exact_mut(stride)).zip(red.chunks_exact_mut(stride));
        for (line, ((bright, blue), red)) in lines.enumerate() {
            let y = (row * side + line).min(high - 1);
            let samples = bright.iter_mut().zip(blue.iter_mut()).zip(red.iter_mut());
            for (pixel, ((bright, blue), red)) in self.pixels[y * wide..(y + 1) * wide].iter().zip(samples) {
                [*bright, *blue, *red] = brightness_and_colour(*pixel);
            }
            for plane in [bright, blue, red] {
                let last = plane[wide - 1];
                plane[wide..].fill(last);
            }
        }
        if !self.half {
            return;
        }
        // Chroma halved each way. Below the image's last chroma row, that row repeats; and its
        // last pixel row for one past it.
        let last = high.div_ceil(2) - 1;
        for line in 0..8 {
            let at = (row * 8 + line).min(last);
            let [upper, lower] = [2 * at, (2 * at + 1).min(high - 1)].map(|y| (y - row * 16) * stride);
            for (half, plane) in self.halves.iter_mut().zip(&self.planes[1..]) {
                halve(&plane[upper..upper + stride], &plane[lower..lower + stride], &mut half[line * stride / 2..(line + 1) * stride / 2]);
            }
        }
    }

    /// Gives `out` the MCU at `column` of row `row`: its luma blocks (four, with chroma
    /// halved), then one per chroma plane. A luma block wholly outside the image repeats the
    /// previous block, DC only.
    fn unit(&self, row: usize, column: usize, out: &mut impl Blocks) {
        let (stride, blocks) = (self.stride, self.size.map(|length| length.div_ceil(8)));
        let side = 1 + self.half as usize;
        let mut average = 0;
        for at in 0..side * side {
            let [x, y] = [column * side + at % side, row * side + at / side];
            let waves = match x < blocks[0] && y < blocks[1] {
                true => self.waves(&self.planes[0], stride, [8 * x, 8 * (at / side)], 0),
                false => (std::array::from_fn(|at| [0, average][(at == 0) as usize]), 1),
            };
            average = waves.0[0];
            out.block(&waves, 0);
        }
        let colours = [0, 1].map(|plane| if self.half { &self.halves[plane] } else { &self.planes[plane + 1] });
        let waves = colours.map(|samples| self.waves(samples, stride / side, [8 * column, 0], 1));
        out.block(&waves[0], 1);
        out.block(&waves[1], 2);
    }

    /// The 64 coefficients of the block at `corner` of `samples` (`stride` per row), quantized
    /// by table `table`, in zigzag order; and a bit per nonzero one.
    fn waves(&self, samples: &[u8], stride: usize, [x, y]: [usize; 2], table: usize) -> ([i32; 64], u64) {
        let mut block = [0; 64];
        for (row, line) in block.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            for (sample, from) in line.iter_mut().zip(&samples[(y + row) * stride + x..][..8]) {
                *sample = *from as i32 - 128;
            }
        }
        waves_in::<true>(&mut block);
        waves_in::<false>(&mut block);
        let (mut zigzag, mut set) = ([0; 64], 0u64);
        for (place, at) in ZIGZAG.iter().enumerate() {
            let ([by, add, shift], wave) = (self.dividers[table][*at as usize], block[*at as usize]);
            let divided = (((wave.unsigned_abs() + add) * by) >> shift) as i32;
            zigzag[place] = if wave < 0 { -divided } else { divided };
            set |= ((divided != 0) as u64) << place;
        }
        (zigzag, set)
    }
}

/// Where an encoder's blocks go, in MCU order, each as its quantized coefficients in zigzag
/// order and a bit per nonzero one: coded at once, or kept for the scans of a progressive
/// image.
trait Blocks {
    fn block(&mut self, waves: &([i32; 64], u64), plane: usize);
}

/// A baseline image's one scan: its blocks Huffman-coded as they come, with the standard
/// tables.
struct Sequential {
    /// Per Huffman table, each symbol's code, above its length.
    codes: [[u32; 256]; 4],
    /// Each plane's last DC, which the next one is coded against.
    last: [i32; 3],
    packed: Packed,
}

impl Sequential {
    /// Writes the tables (DC then AC, luma then chroma) and the scan's header after `out`.
    fn new(mut out: Vec<u8>) -> Self {
        for (which, packing) in PACKINGS.iter().enumerate() {
            let symbols = symbols(packing);
            out.extend([0xFF, 0xC4, 0, 19 + symbols.len() as u8, (((which % 2) << 4) | (which / 2)) as u8]);
            out.extend(packing.0.iter().chain(&symbols));
        }
        // All three planes, the first with its tables and the others with theirs.
        out.extend([0xFF, 0xDA, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
        Sequential { codes: PACKINGS.map(|packing| codes(&packing)), last: [0; 3], packed: Packed { out, bits: 0, count: 0 } }
    }
}

impl Blocks for Sequential {
    /// Huffman-codes a block (and its nonzero bits) for `plane`: its DC as the difference from
    /// the plane's last, then each nonzero AC after its zero run.
    fn block(&mut self, (waves, set): &([i32; 64], u64), plane: usize) {
        let (average, rest) = (&self.codes[2 * plane.min(1)], &self.codes[2 * plane.min(1) + 1]);
        self.packed.number(average, 0, waves[0] - self.last[plane]);
        self.last[plane] = waves[0];
        let (mut set, mut after) = (set & !1, 0);
        while set != 0 {
            let at = set.trailing_zeros();
            // ZRL codes sixteen zeros at a time.
            let noughts = at - after - 1;
            for _ in 0..noughts / 16 {
                self.packed.number(rest, 0xF0, 0);
            }
            self.packed.number(rest, (noughts % 16) << 4, waves[at as usize]);
            (after, set) = (at, set & (set - 1));
        }
        // And EOB where only zeros remain.
        if after < 63 {
            self.packed.number(rest, 0, 0);
        }
    }
}

/// A chroma row halved from two image rows (`upper` and `lower`): four samples summed and
/// divided by four, rounding down and up alternately, as libjpeg does.
fn halve(upper: &[u8], lower: &[u8], half: &mut [u8]) {
    let fours = upper.as_chunks::<4>().0.iter().zip(lower.as_chunks::<4>().0);
    for (pair, (up, down)) in half.as_chunks_mut::<2>().0.iter_mut().zip(fours) {
        let sum = |at: usize| up[at] as u32 + up[at + 1] as u32 + down[at] as u32 + down[at + 1] as u32;
        *pair = [((sum(0) + 1) >> 2) as u8, ((sum(2) + 2) >> 2) as u8];
    }
}

/// Each symbol's code above its bit count: counting up from zero, appending a zero bit at each
/// new length (as decoded, in `Codes::new`).
fn codes(packing: &([u8; 16], &[u8])) -> [u32; 256] {
    let mut codes = [0; 256];
    let lengths = packing.0.iter().enumerate().flat_map(|(long, count)| std::iter::repeat_n(long as u32 + 1, *count as usize));
    let (mut code, mut last) = (0u32, 0);
    for (long, symbol) in lengths.zip(symbols(packing)) {
        (code, last) = (code << (long - last), long);
        codes[symbol as usize] = code << 8 | long;
        code += 1;
    }
    codes
}

/// Divides by `divider` and rounds as libjpeg's fast path does: add, multiply, shift
/// (Alverson's method). Equal to the division for every value a block can hold.
fn divide(divider: u32) -> [u32; 3] {
    let shift = 16 + 31 - divider.leading_zeros();
    let (by, left) = ((1 << shift) / divider, (1 << shift) % divider);
    match left {
        // A power of two.
        0 => [by >> 1, divider / 2, shift - 1],
        _ if left <= divider / 2 => [by, divider / 2 + 1, shift],
        _ => [by + 1, divider / 2, shift],
    }
}

/// YCbCr of a pixel `0xRRGGBB`, in integers as libjpeg computes them (16 fraction bits,
/// rounded; chroma biased a hair below a half, so none reaches 256).
#[inline(always)]
fn brightness_and_colour(pixel: u32) -> [u8; 3] {
    let (r, g, b) = ((pixel >> 16 & 255) as i32, (pixel >> 8 & 255) as i32, (pixel & 255) as i32);
    let bright = (19595 * r + 38470 * g + 7471 * b + 32768) >> 16;
    let blue = (-11059 * r - 21709 * g + 32768 * b + (128 << 16) + 32767) >> 16;
    let red = (32768 * r - 27439 * g - 5329 * b + (128 << 16) + 32767) >> 16;
    [bright as u8, blue as u8, red as u8]
}

/// A block's forward DCT in place, as libjpeg's "slow, accurate" integer DCT (Loeffler,
/// Ligtenberg and Moschytz, 13 fraction bits): along each row (`ROWS`), leaving results 4× too
/// large, then down each column, which removes that and leaves all 8× too large (so the
/// quantizers are taken 8×).
fn waves_in<const ROWS: bool>(block: &mut [i32; 64]) {
    let (step, apart, shift) = if ROWS { (1, 8, 11) } else { (8, 1, 15) };
    let down = |value: i32| (value + (1 << (shift - 1))) >> shift;
    let even = |value: i32| if ROWS { value << 2 } else { (value + 2) >> 2 };
    for line in 0..8 {
        let at = |k: usize| line * apart + k * step;
        let (sum0, less7) = (block[at(0)] + block[at(7)], block[at(0)] - block[at(7)]);
        let (sum1, less6) = (block[at(1)] + block[at(6)], block[at(1)] - block[at(6)]);
        let (sum2, less5) = (block[at(2)] + block[at(5)], block[at(2)] - block[at(5)]);
        let (sum3, less4) = (block[at(3)] + block[at(4)], block[at(3)] - block[at(4)]);
        let (even0, even3, even1, even2) = (sum0 + sum3, sum0 - sum3, sum1 + sum2, sum1 - sum2);
        let turned = (even2 + even3) * 4433;
        let both = (less4 + less7 + less5 + less6) * 9633;
        let (one, two) = ((less4 + less7) * -7373, (less5 + less6) * -20995);
        let (three, four) = ((less4 + less6) * -16069 + both, (less5 + less7) * -3196 + both);
        block[at(0)] = even(even0 + even1);
        block[at(4)] = even(even0 - even1);
        block[at(2)] = down(turned + even3 * 6270);
        block[at(6)] = down(turned - even2 * 15137);
        block[at(7)] = down(less4 * 2446 + one + three);
        block[at(5)] = down(less5 * 16819 + two + four);
        block[at(3)] = down(less6 * 25172 + two + three);
        block[at(1)] = down(less7 * 12299 + one + four);
    }
}

/// Output bytes, and pending bits not yet making four bytes.
struct Packed {
    out: Vec<u8>,
    bits: u64,
    count: u32,
}

impl Packed {
    /// Appends the low `count` bits of `bits` (at most 27), four bytes at a time.
    #[inline(always)]
    fn put(&mut self, bits: u32, count: u32) {
        (self.bits, self.count) = (self.bits << count | bits as u64, self.count + count);
        if self.count >= 32 {
            self.count -= 32;
            self.bytes(&((self.bits >> self.count) as u32).to_be_bytes());
        }
    }

    /// Appends whole bytes. A 0xFF byte gets a stuffed zero after it, so it isn't read as a
    /// marker.
    fn bytes(&mut self, bytes: &[u8]) {
        if !bytes.contains(&0xFF) {
            return self.out.extend_from_slice(bytes);
        }
        for byte in bytes {
            self.out.extend_from_slice(&[*byte, 0][..1 + (*byte == 0xFF) as usize]);
        }
    }

    /// Appends a value as its symbol's code (its bit count, with `before` above that), then its
    /// bits: for a negative value, those of one less.
    #[inline(always)]
    fn number(&mut self, codes: &[u32; 256], before: u32, number: i32) {
        let size = 32 - number.unsigned_abs().leading_zeros();
        let code = codes[(before | size) as usize];
        let bits = (number - (number < 0) as i32) as u32 & ((1 << size) - 1);
        self.put((code >> 8) << size | bits, (code & 255) + size);
    }

    /// Pads the last byte with ones and writes all that is held: the end of a scan.
    fn flush(&mut self) {
        self.put(0x7F, 7);
        let whole = self.count / 8;
        let left = (self.bits >> (self.count % 8)) as u32;
        self.bytes(&left.to_be_bytes()[4 - whole as usize..]);
        (self.bits, self.count) = (0, 0);
    }

    /// Flushes, and appends EOI.
    fn end(mut self) -> Vec<u8> {
        self.flush();
        self.out.extend([0xFF, 0xD9]);
        self.out
    }
}

/// The blocks of a progressive image, kept as they come for its scans: in MCU order, four luma
/// blocks to an MCU with chroma halved (else one), and one of each chroma plane.
#[derive(Default)]
struct Kept {
    planes: [Vec<[i16; 64]>; 3],
}

impl Blocks for Kept {
    fn block(&mut self, (waves, _): &([i32; 64], u64), plane: usize) {
        self.planes[plane].push(waves.map(|wave| wave as i16));
    }
}

/// One scan of a progressive image: its planes, its band of coefficients (zigzag positions
/// `from` to `to`), and its bits: from bit `low` up, or (where `high` isn't 0) bit `low` alone,
/// below those an earlier scan brought.
struct Scan {
    planes: &'static [usize],
    from: usize,
    to: usize,
    high: u32,
    low: u32,
}

/// libjpeg's progression for YCbCr (`jpeg_simple_progression`): DC but its last bit; luma's
/// lowest AC, chroma's AC and the rest of luma's, each but its last bits; luma's next bit; then
/// the last bit of each.
const SCANS: [Scan; 10] = [
    Scan { planes: &[0, 1, 2], from: 0, to: 0, high: 0, low: 1 },
    Scan { planes: &[0], from: 1, to: 5, high: 0, low: 2 },
    Scan { planes: &[2], from: 1, to: 63, high: 0, low: 1 },
    Scan { planes: &[1], from: 1, to: 63, high: 0, low: 1 },
    Scan { planes: &[0], from: 6, to: 63, high: 0, low: 2 },
    Scan { planes: &[0], from: 1, to: 63, high: 2, low: 1 },
    Scan { planes: &[0, 1, 2], from: 0, to: 0, high: 1, low: 0 },
    Scan { planes: &[2], from: 1, to: 63, high: 1, low: 0 },
    Scan { planes: &[1], from: 1, to: 63, high: 1, low: 0 },
    Scan { planes: &[0], from: 1, to: 63, high: 1, low: 0 },
];

impl Kept {
    /// Writes the scans after `out` (the headers), and EOI. Each scan is read twice: once to
    /// count its symbols, which makes its Huffman tables (written before it), then to code it.
    /// An image of `size` pixels, `units` MCUs, chroma halved or not.
    fn scans(&self, out: Vec<u8>, size: [usize; 2], units: [usize; 2], half: bool) -> Vec<u8> {
        let (mut packed, layout) = (Packed { out, bits: 0, count: 0 }, Layout { size, units, side: 1 + half as usize });
        for scan in &SCANS {
            let codes = self.tables(scan, &layout, &mut packed.out);
            scan_header(scan, &mut packed.out);
            let mut coded = Coded { codes, packed };
            self.scan(scan, &layout, &mut coded);
            packed = coded.packed;
            packed.flush();
        }
        packed.out.extend([0xFF, 0xD9]);
        packed.out
    }

    /// Makes the Huffman tables a scan uses, from its symbols' counts, and writes them after
    /// `out`: luma's, then chroma's (a DC refinement's bits need none). Returns their codes.
    fn tables(&self, scan: &Scan, layout: &Layout, out: &mut Vec<u8>) -> [[u32; 256]; 2] {
        let mut counts = Counting([[0; 257]; 2]);
        self.scan(scan, layout, &mut counts);
        let mut tables: Vec<usize> = scan.planes.iter().map(|plane| *plane.min(&1)).filter(|_| scan.from != 0 || scan.high == 0).collect();
        tables.dedup();
        let mut made = [[0; 256]; 2];
        for table in tables {
            let (lengths, symbols) = made_codes(&counts.0[table]);
            let class = (scan.from != 0) as u8;
            out.extend([&[0xFF, 0xC4][..], &(19 + symbols.len() as u16).to_be_bytes(), &[class << 4 | table as u8], &lengths, &symbols].concat());
            made[table] = codes(&(lengths, &symbols));
        }
        made
    }

    /// Codes one scan with `coding`: its DC in MCUs, or one plane's AC block by block across
    /// and down the plane.
    fn scan(&self, scan: &Scan, layout: &Layout, coding: &mut impl Coding) {
        if scan.from == 0 {
            let mut last = [0; 3];
            for unit in 0..layout.units[0] * layout.units[1] {
                self.averages(scan, layout, unit, &mut last, coding);
            }
            return;
        }
        let mut band = Band { coding, table: scan.planes[0].min(1), run: 0, corrections: Vec::new() };
        for block in layout.blocks(scan.planes[0]).map(|at| &self.planes[scan.planes[0]][at]) {
            match scan.high {
                0 => band.first(block, scan),
                _ => band.refine(block, scan),
            }
        }
        band.end_run();
    }

    /// An MCU's DC: for a first scan, each block's as a difference from the plane's last; for
    /// a refinement, the next bit of each.
    fn averages(&self, scan: &Scan, layout: &Layout, unit: usize, last: &mut [i32; 3], coding: &mut impl Coding) {
        let squares = layout.side * layout.side;
        let blocks = self.planes[0][unit * squares..][..squares].iter().map(|block| (0, block)).chain([1, 2].map(|plane| (plane, &self.planes[plane][unit])));
        for (plane, block) in blocks {
            let average = block[0] as i32 >> scan.low;
            match scan.high {
                0 => coding.number(plane.min(1), 0, average - std::mem::replace(&mut last[plane], average)),
                _ => coding.bits(average as u32 & 1, 1),
            }
        }
    }
}

/// Writes a progressive scan's header: its planes, each with its table, DC or AC (none where
/// unused), its band, and its bits.
fn scan_header(scan: &Scan, out: &mut Vec<u8>) {
    let table = |plane: usize| match (scan.from, scan.high) {
        (0, 0) => (plane.min(1) as u8) << 4,
        (0, _) => 0,
        _ => plane.min(1) as u8,
    };
    let chosen = scan.planes.iter().flat_map(|plane| [*plane as u8 + 1, table(*plane)]);
    let head: Vec<u8> = [scan.planes.len() as u8].into_iter().chain(chosen).chain([scan.from as u8, scan.to as u8, (scan.high << 4 | scan.low) as u8]).collect();
    out.extend([&[0xFF, 0xDA][..], &(2 + head.len() as u16).to_be_bytes(), &head].concat());
}

/// Where a progressive image's blocks lie in `Kept`: its size, MCU count, and luma blocks to
/// an MCU across and down.
struct Layout {
    size: [usize; 2],
    units: [usize; 2],
    side: usize,
}

impl Layout {
    /// Where each of a plane's blocks lies, across then down the plane: luma's are kept in
    /// MCUs, and those past its last pixels (which only make MCUs whole) are left out.
    fn blocks(&self, plane: usize) -> impl Iterator<Item = usize> + '_ {
        let side = if plane == 0 { self.side } else { 1 };
        let [wide, high] = if plane == 0 { self.size.map(|length| length.div_ceil(8)) } else { self.units };
        let at = move |x: usize, y: usize| (y / side * self.units[0] + x / side) * side * side + y % side * side + x % side;
        (0..wide * high).map(move |block| at(block % wide, block / wide))
    }
}

/// What coding a progressive scan does with each symbol and run of bits: counts the symbols,
/// to make the scan's Huffman tables; or codes them with those tables.
trait Coding {
    fn symbol(&mut self, table: usize, symbol: u8);
    fn bits(&mut self, bits: u32, count: u32);

    /// A value as its bit count's symbol (with `before` above it), then its bits: for a
    /// negative value, those of one less.
    fn number(&mut self, table: usize, before: u8, number: i32) {
        let size = 32 - number.unsigned_abs().leading_zeros();
        self.symbol(table, before | size as u8);
        self.bits((number - (number < 0) as i32) as u32 & ((1 << size) - 1), size);
    }
}

/// Each symbol's count, per table (and one more, for the code no symbol may have).
struct Counting([[u64; 257]; 2]);

impl Coding for Counting {
    fn symbol(&mut self, table: usize, symbol: u8) {
        self.0[table][symbol as usize] += 1;
    }

    fn bits(&mut self, _: u32, _: u32) {}
}

/// A scan's codes, per table (see `codes`), and the output.
struct Coded {
    codes: [[u32; 256]; 2],
    packed: Packed,
}

impl Coding for Coded {
    fn symbol(&mut self, table: usize, symbol: u8) {
        let code = self.codes[table][symbol as usize];
        self.packed.put(code >> 8, code & 255);
    }

    fn bits(&mut self, bits: u32, count: u32) {
        self.packed.put(bits, count);
    }
}

/// An AC scan of one plane being coded: its table, the count of blocks since the last with
/// anything new (an end-of-band run, coded as one symbol), and the refinement bits of those
/// blocks, which follow that symbol.
struct Band<'a, C: Coding> {
    coding: &'a mut C,
    table: usize,
    run: u32,
    corrections: Vec<u8>,
}

impl<C: Coding> Band<'_, C> {
    /// A first scan of a block's band: each coefficient's top bits, as baseline codes them,
    /// after its zero run (sixteen at a time where longer); a block with nothing more joins the
    /// run.
    fn first(&mut self, block: &[i16; 64], scan: &Scan) {
        let mut noughts = 0;
        for wave in &block[scan.from..=scan.to] {
            let size = (wave.unsigned_abs() >> scan.low) as i32;
            if size == 0 {
                noughts += 1;
                continue;
            }
            self.end_run();
            (0..noughts / 16).for_each(|_| self.coding.symbol(self.table, 0xF0));
            self.coding.number(self.table, (noughts % 16) << 4, if *wave < 0 { -size } else { size });
            noughts = 0;
        }
        if noughts > 0 {
            self.run += 1;
            if self.run == 0x7FFF {
                self.end_run();
            }
        }
    }

    /// A refinement of a block's band, by one bit: coefficients that start at it, each after
    /// its zero run with its sign; the bit of those already started follows the next such (or
    /// sixteen zeros, or the end of the run).
    fn refine(&mut self, block: &[i16; 64], scan: &Scan) {
        let size = |wave: &i16| wave.unsigned_abs() >> scan.low;
        let last = (scan.from..=scan.to).rev().find(|at| size(&block[*at]) == 1).unwrap_or(0);
        let (mut noughts, mut pending) = (0, Vec::new());
        for (at, wave) in block.iter().enumerate().take(scan.to + 1).skip(scan.from) {
            let started = size(wave);
            if started == 0 {
                noughts += 1;
                continue;
            }
            // Sixteen zeros are coded as such only before the last coefficient that starts.
            while noughts > 15 && at <= last {
                self.end_run();
                self.coding.symbol(self.table, 0xF0);
                (noughts, pending) = (noughts - 16, self.send(pending));
            }
            if started > 1 {
                pending.push(started as u8 & 1);
                continue;
            }
            self.end_run();
            self.coding.symbol(self.table, (noughts << 4 | 1) as u8);
            self.coding.bits((*wave >= 0) as u32, 1);
            (noughts, pending) = (0, self.send(pending));
        }
        if noughts > 0 || !pending.is_empty() {
            self.run += 1;
            self.corrections.extend(pending);
            // libjpeg keeps at most 1000 refinement bits waiting, less a block's worth.
            if self.run == 0x7FFF || self.corrections.len() > 937 {
                self.end_run();
            }
        }
    }

    /// Ends the run of blocks with nothing new, if any: its symbol, the length's low bits,
    /// then the refinement bits of its blocks.
    fn end_run(&mut self) {
        if self.run == 0 {
            return;
        }
        let size = 31 - self.run.leading_zeros();
        self.coding.symbol(self.table, (size << 4) as u8);
        self.coding.bits(self.run & ((1 << size) - 1), size);
        self.run = 0;
        let corrections = std::mem::take(&mut self.corrections);
        self.send(corrections);
    }

    /// Codes refinement bits, and returns the emptied list.
    fn send(&mut self, mut bits: Vec<u8>) -> Vec<u8> {
        bits.drain(..).for_each(|bit| self.coding.bits(bit as u32, 1));
        bits
    }
}

/// A Huffman table made for a scan's symbol counts, as libjpeg makes it
/// (`jpeg_gen_optimal_table`): code lengths from joining the two rarest, again and again; none
/// longer than 16 bits; and no code all ones (a 257th symbol takes it). Returns the code count
/// per length and the symbols in code order.
fn made_codes(counts: &[u64; 257]) -> ([u8; 16], Vec<u8>) {
    let mut counts = *counts;
    counts[256] = 1;
    // Past 32 bits, where libjpeg gives up, counts are halved.
    let lengths = loop {
        if let Some(lengths) = code_lengths(counts) {
            break lengths;
        }
        counts = counts.map(|count| count.div_ceil(2));
    };
    let mut bits = [0u16; 33];
    lengths.iter().filter(|long| **long > 0).for_each(|long| bits[*long] += 1);
    // Two codes longer than 16 bits become one a bit shorter and, with a shorter code, two a
    // bit longer than that (Annex K.3).
    for long in (17..=32).rev() {
        while bits[long] > 0 {
            let mut shorter = long - 2;
            while bits[shorter] == 0 {
                shorter -= 1;
            }
            (bits[long], bits[long - 1], bits[shorter + 1], bits[shorter]) = (bits[long] - 2, bits[long - 1] + 1, bits[shorter + 1] + 2, bits[shorter] - 1);
        }
    }
    let longest = (1..=16).rev().find(|long| bits[*long] > 0).unwrap_or(16);
    bits[longest] -= 1;
    let symbols = (1..=32).flat_map(|long| (0..256).filter(move |symbol| lengths[*symbol] == long)).map(|symbol| symbol as u8);
    (std::array::from_fn(|at| bits[at + 1] as u8), symbols.collect())
}

/// Each symbol's code length: the two rarest are joined, again and again, and every symbol of
/// each becomes a bit longer. `None` past 32 bits.
fn code_lengths(mut counts: [u64; 257]) -> Option<[usize; 257]> {
    let (mut lengths, mut next) = ([0; 257], [None; 257]);
    while let Some((one, other)) = rarest_two(&counts) {
        (counts[one], counts[other]) = (counts[one] + counts[other], 0);
        let end = deepen(&mut lengths, &next, one);
        next[end] = Some(other);
        deepen(&mut lengths, &next, other);
    }
    lengths.iter().all(|long| *long <= 32).then_some(lengths)
}

/// Makes every symbol joined from `first` on a bit longer; returns the last of them.
fn deepen(lengths: &mut [usize; 257], next: &[Option<usize>; 257], first: usize) -> usize {
    let mut at = first;
    lengths[at] += 1;
    while let Some(then) = next[at] {
        at = then;
        lengths[at] += 1;
    }
    at
}

/// The rarest two symbols that occur, each of equal counts the highest-numbered (as libjpeg
/// picks them). `None` if fewer than two are left.
fn rarest_two(counts: &[u64; 257]) -> Option<(usize, usize)> {
    let rarest = |skip: usize| (0..257).filter(|at| counts[*at] != 0 && *at != skip).min_by_key(|at| (counts[*at], std::cmp::Reverse(*at)));
    let one = rarest(257)?;
    Some((one, rarest(one)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baseline, chroma halved both ways, a size filling no whole MCU. Drawn and saved by
    /// Windows' GDI+; the others are libjpeg's unless said otherwise.
    const PLAIN: &[u8] = include_bytes!("../tests/pictures/plain.jpg");
    /// Baseline, full chroma, restarts every three MCUs, and EXIF orientation of a quarter
    /// turn.
    const TURNED: &[u8] = include_bytes!("../tests/pictures/turned.jpg");
    /// Greyscale, progressive.
    const GREY: &[u8] = include_bytes!("../tests/pictures/grey.jpg");
    /// CMYK as Adobe stores it (YCCK, chroma halved), progressive, restarts every four MCUs.
    const INKS: &[u8] = include_bytes!("../tests/pictures/inks.jpg");
    /// Arithmetic coded: baseline, chroma halved both ways, restarts every two MCUs, with
    /// non-default conditioning.
    const ARITHMETIC: &[u8] = include_bytes!("../tests/pictures/arithmetic.jpg");
    /// Arithmetic coded, progressive: chroma at half width, restarts every three MCUs.
    const ARITHMETIC_STEPS: &[u8] = include_bytes!("../tests/pictures/arithmetic-steps.jpg");
    /// 12-bit: baseline, chroma halved, so coarse its quantizers take two bytes each.
    const DEEP: &[u8] = include_bytes!("../tests/pictures/deep.jpg");
    /// 12-bit greyscale, progressive.
    const DEEP_STEPS: &[u8] = include_bytes!("../tests/pictures/deep-steps.jpg");
    /// 12-bit CMYK as Adobe stores it, arithmetic coded, progressive, restarts every four MCUs.
    const DEEP_ARITHMETIC: &[u8] = include_bytes!("../tests/pictures/deep-arithmetic.jpg");
    /// Lossless: RGB, predictor 4, restarts every three rows.
    const EXACT: &[u8] = include_bytes!("../tests/pictures/exact.jpg");
    /// Lossless, 16-bit: greyscale, predictor 5.
    const EXACT_DEEP: &[u8] = include_bytes!("../tests/pictures/exact-deep.jpg");
    /// Lossless, 3-bit: eight levels per colour, predictor 2.
    const EXACT_SHALLOW: &[u8] = include_bytes!("../tests/pictures/exact-shallow.jpg");
    /// Lossless, full luma and chroma halved both ways, predictor 1, with no component marker:
    /// as ffmpeg writes it.
    const EXACT_COARSE: &[u8] = include_bytes!("../tests/pictures/exact-coarse.jpg");
    /// Lossless, built by hand per the standard: 12-bit, chroma at half width, a scan per plane
    /// with its own predictor (6, 7 and 3), tables and point transform, restarts every two
    /// rows, and Adobe's marker saying YCbCr.
    const EXACT_BY_HAND: &[u8] = include_bytes!("../tests/pictures/exact-by-hand.jpg");

    /// Every test image, with its name and libjpeg's decoding.
    const PICTURES: [(&str, &[u8], &[u8]); 14] = [
        ("plain", PLAIN, include_bytes!("../tests/pictures/plain.png")),
        ("turned", TURNED, include_bytes!("../tests/pictures/turned.png")),
        ("grey", GREY, include_bytes!("../tests/pictures/grey.png")),
        ("inks", INKS, include_bytes!("../tests/pictures/inks.png")),
        ("arithmetic", ARITHMETIC, include_bytes!("../tests/pictures/arithmetic.png")),
        ("arithmetic-steps", ARITHMETIC_STEPS, include_bytes!("../tests/pictures/arithmetic-steps.png")),
        ("deep", DEEP, include_bytes!("../tests/pictures/deep.png")),
        ("deep-steps", DEEP_STEPS, include_bytes!("../tests/pictures/deep-steps.png")),
        ("deep-arithmetic", DEEP_ARITHMETIC, include_bytes!("../tests/pictures/deep-arithmetic.png")),
        ("exact", EXACT, include_bytes!("../tests/pictures/exact.png")),
        ("exact-deep", EXACT_DEEP, include_bytes!("../tests/pictures/exact-deep.png")),
        ("exact-shallow", EXACT_SHALLOW, include_bytes!("../tests/pictures/exact-shallow.png")),
        ("exact-coarse", EXACT_COARSE, include_bytes!("../tests/pictures/exact-coarse.png")),
        ("exact-by-hand", EXACT_BY_HAND, include_bytes!("../tests/pictures/exact-by-hand.png")),
    ];

    /// The largest per-channel difference between two images.
    fn furthest(one: &[u32], other: &[u32]) -> u32 {
        one.iter().zip(other).flat_map(|(one, other)| [16, 8, 0].map(|part| (one >> part & 255).abs_diff(other >> part & 255))).max().unwrap_or(0)
    }

    #[test]
    fn pictures_come_out_as_libjpeg_reads_them() {
        // Beside each image is libjpeg's decoding: rotated per its EXIF, CMYK multiplied out,
        // deeper and shallower samples brought to 8 bits as its tools do. Sound decoders differ
        // by a level or two in rounding. This one follows libjpeg's steps exactly, so no
        // difference is allowed. (The last two it decodes sample for sample but leaves as
        // YCbCr: it makes no RGB of lossless images. Those references were converted here, with
        // its own coefficients.)
        for (name, jpeg, png) in PICTURES {
            let (width, pixels) = decode(jpeg).unwrap_or_else(|| panic!("{name}.jpg is read"));
            let (wanted_width, wanted) = crate::png::decode(png).unwrap_or_else(|| panic!("{name}.png is read"));
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name}: its size");
            assert_eq!(furthest(&pixels, &wanted), 0, "{name}: its colours");
            assert!(pixels.iter().all(|pixel| pixel >> 24 == 255), "{name}: solid all over");
        }
    }

    /// What only the ISO reference codec writes (Thomas Richter's libjpeg,
    /// github.com/thorfdbg/libjpeg), 45 by 29, each beside its own decoding, and how far this
    /// decoder may be from that. Lossless, not at all. Hierarchical, by a few levels: the
    /// reference keeps fractions of a level from frame to frame (of its inverse DCTs, and of
    /// doubling across before down), where the standard rounds each step to whole levels, as
    /// this does (and `hierarchical_pictures_without_loss_come_back_sample_for_sample` proves);
    /// colour more, as YCbCr to RGB spreads chroma's.
    const BY_THE_REFERENCE: [(&str, &[u8], &[u8], u32); 11] = [
        // Grey, a first frame of half the size each way, then one doubling it: Huffman or
        // arithmetic coded, sequential or progressive; 12 bits, Huffman.
        ("hierarchical", include_bytes!("../tests/pictures/hierarchical.jpg"), include_bytes!("../tests/pictures/hierarchical.png"), 3),
        ("hierarchical-arithmetic", include_bytes!("../tests/pictures/hierarchical-arithmetic.jpg"), include_bytes!("../tests/pictures/hierarchical-arithmetic.png"), 3),
        ("hierarchical-steps", include_bytes!("../tests/pictures/hierarchical-steps.jpg"), include_bytes!("../tests/pictures/hierarchical-steps.png"), 3),
        ("hierarchical-arithmetic-steps", include_bytes!("../tests/pictures/hierarchical-arithmetic-steps.jpg"), include_bytes!("../tests/pictures/hierarchical-arithmetic-steps.png"), 3),
        ("hierarchical-deep", include_bytes!("../tests/pictures/hierarchical-deep.jpg"), include_bytes!("../tests/pictures/hierarchical-deep.png"), 1),
        // YCbCr, three frames.
        ("hierarchical-colour", include_bytes!("../tests/pictures/hierarchical-colour.jpg"), include_bytes!("../tests/pictures/hierarchical-colour.png"), 6),
        // RGB lossless, two frames, Huffman or arithmetic coded.
        ("hierarchical-exact", include_bytes!("../tests/pictures/hierarchical-exact.jpg"), include_bytes!("../tests/pictures/hierarchical-exact.png"), 1),
        ("hierarchical-exact-arithmetic", include_bytes!("../tests/pictures/hierarchical-exact-arithmetic.jpg"), include_bytes!("../tests/pictures/hierarchical-exact-arithmetic.png"), 1),
        // Lossless arithmetic coded: RGB; grey restarting every two rows; 16-bit grey.
        ("exact-arithmetic", include_bytes!("../tests/pictures/exact-arithmetic.jpg"), include_bytes!("../tests/pictures/exact-arithmetic.png"), 0),
        ("exact-arithmetic-restarts", include_bytes!("../tests/pictures/exact-arithmetic-restarts.jpg"), include_bytes!("../tests/pictures/exact-arithmetic-restarts.png"), 0),
        ("exact-arithmetic-deep", include_bytes!("../tests/pictures/exact-arithmetic-deep.jpg"), include_bytes!("../tests/pictures/exact-arithmetic-deep.png"), 0),
    ];

    #[test]
    #[cfg(feature = "rare-jpeg")]
    fn pictures_only_the_reference_writes_come_out_as_it_reads_them() {
        for (name, jpeg, png, most) in BY_THE_REFERENCE {
            let (width, pixels) = decode(jpeg).unwrap_or_else(|| panic!("{name}.jpg is read"));
            let (wanted_width, wanted) = crate::png::decode(png).unwrap_or_else(|| panic!("{name}.png is read"));
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name}: its size");
            assert!(furthest(&pixels, &wanted) <= most, "{name}: its colours, {} apart", furthest(&pixels, &wanted));
        }
    }

    /// A picture as a PPM (or PGM, `grey`) file: its samples, `most` the top level.
    fn netpbm(width: usize, samples: &[u16], grey: bool, most: u16) -> Vec<u8> {
        let head = format!("P{} {width} {} {most}\n", if grey { 5 } else { 6 }, samples.len() / width / if grey { 1 } else { 3 });
        let bytes = samples.iter().flat_map(|sample| if most > 255 { sample.to_be_bytes().to_vec() } else { vec![*sample as u8] });
        head.into_bytes().into_iter().chain(bytes).collect()
    }

    /// The samples of a PPM or PGM file, and its top level.
    fn from_netpbm(file: &[u8]) -> (Vec<u16>, u16) {
        // After "P5" or "P6": width, height and the top level, each followed by whitespace.
        let fields: Vec<usize> = file.split(u8::is_ascii_whitespace).filter(|field| !field.is_empty()).skip(1).take(3).map(|field| std::str::from_utf8(field).unwrap().parse().unwrap()).collect();
        let start = file.iter().enumerate().filter(|(_, byte)| byte.is_ascii_whitespace()).nth(3).unwrap().0 + 1;
        let samples = match fields[2] > 255 {
            true => file[start..].as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect(),
            false => file[start..].iter().map(|byte| *byte as u16).collect(),
        };
        (samples, fields[2] as u16)
    }

    /// Regenerates `BY_THE_REFERENCE` with the ISO reference codec's `jpeg` (built from
    /// github.com/thorfdbg/libjpeg; its lossless mode also needs `Tables::QuantizationTableIndexOf`
    /// to return 0 where no DQT is defined). Each beside its decoding, brought to 8 bits as
    /// `decode` brings it; YCbCr from the planes as the reference keeps them, made RGB as
    /// libjpeg makes it. (`cargo test --lib jpeg -- --ignored`.)
    #[test]
    #[ignore = "needs the ISO reference codec, and writes the files the other tests read"]
    fn the_pictures_only_the_reference_writes_are_made_afresh() {
        use std::process::Command;
        let (pictures, work) = (std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures"), std::env::temp_dir().join(format!("taut-reference-{}", std::process::id())));
        std::fs::create_dir_all(&work).unwrap();
        let [width, height] = WRITTEN;
        let colours: Vec<u16> = photo(width, height).iter().flat_map(|pixel| [16, 8, 0].map(|shift| (pixel >> shift & 255) as u16)).collect();
        let deep = |depth: u32| (0..width * height).map(|at| ((at % width * 65535 / width) as u32 ^ ((at * 977) as u32 % 4099)) as u16 >> (16 - depth)).collect::<Vec<u16>>();
        std::fs::write(work.join("colour.ppm"), netpbm(width, &colours, false, 255)).unwrap();
        std::fs::write(work.join("grey.pgm"), netpbm(width, &colours.iter().skip(1).step_by(3).copied().collect::<Vec<u16>>(), true, 255)).unwrap();
        std::fs::write(work.join("deep.pgm"), netpbm(width, &deep(12), true, 4095)).unwrap();
        std::fs::write(work.join("deeper.pgm"), netpbm(width, &deep(16), true, 65535)).unwrap();
        let made: [(&str, &str, &[&str]); 11] = [
            ("hierarchical", "grey.pgm", &["-q", "85", "-y", "2", "-h"]),
            ("hierarchical-arithmetic", "grey.pgm", &["-q", "85", "-y", "2", "-a"]),
            ("hierarchical-steps", "grey.pgm", &["-q", "85", "-y", "2", "-v", "-h"]),
            ("hierarchical-arithmetic-steps", "grey.pgm", &["-q", "85", "-y", "2", "-v", "-a"]),
            ("hierarchical-deep", "deep.pgm", &["-q", "85", "-y", "2", "-h"]),
            ("hierarchical-colour", "colour.ppm", &["-q", "85", "-y", "3", "-h"]),
            ("hierarchical-exact", "colour.ppm", &["-p", "-y", "2", "-c", "-h"]),
            ("hierarchical-exact-arithmetic", "colour.ppm", &["-p", "-y", "2", "-c", "-a"]),
            ("exact-arithmetic", "colour.ppm", &["-p", "-a", "-c"]),
            ("exact-arithmetic-restarts", "grey.pgm", &["-p", "-a", "-z", &(2 * WRITTEN[0]).to_string()]),
            ("exact-arithmetic-deep", "deeper.pgm", &["-p", "-a"]),
        ];
        let run = |arguments: &[&str]| assert!(Command::new("jpeg").args(arguments).current_dir(&work).output().is_ok_and(|done| done.status.success()), "jpeg {arguments:?}");
        for (name, source, how) in made {
            let jpeg = pictures.join(format!("{name}.jpg"));
            run(&[how, &[source, jpeg.to_str().unwrap()]].concat());
            // The colour file's planes as they are (`-c`), to be made RGB here.
            let planes = name == "hierarchical-colour";
            run(&[&["-c"][..planes as usize], &[jpeg.to_str().unwrap(), "read.pnm"]].concat());
            let (samples, most) = from_netpbm(&std::fs::read(work.join("read.pnm")).unwrap());
            let byte = |sample: u16| shown(16 - most.leading_zeros())[sample as usize] as u32;
            let pixels: Vec<u32> = match (source.ends_with(".pgm"), planes) {
                (true, _) => samples.iter().map(|sample| 0xff00_0000 | (byte(*sample) * 0x01_0101)).collect(),
                (false, false) => samples.as_chunks::<3>().0.iter().map(|[red, green, blue]| 0xff00_0000 | byte(*red) << 16 | byte(*green) << 8 | byte(*blue)).collect(),
                (false, true) => samples
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|planes| 0xff00_0000 | lit(planes[0] as u8, planes[1] as u8, planes[2] as u8).iter().fold(0, |pixel, part| pixel << 8 | *part as u32))
                    .collect(),
            };
            std::fs::write(pictures.join(format!("{name}.png")), crate::png::encode(width, &pixels).unwrap()).unwrap();
        }
        std::fs::remove_dir_all(work).unwrap();
    }

    /// A JPEG with an EXIF orientation inserted after SOI, little- or big-endian.
    fn noted(jpeg: &[u8], turn: u16, low_first: bool) -> Vec<u8> {
        let notes = crate::exif_orientation::tests::notes(turn, low_first);
        let head = [0xFF, 0xE1, 0, notes.len() as u8 + 2];
        [&jpeg[..2], &head, &notes, &jpeg[2..]].concat()
    }

    #[test]
    fn a_note_from_the_camera_turns_the_picture_each_of_eight_ways() {
        let (width, stored) = decode(PLAIN).expect("is read");
        let height = stored.len() / width;
        for (turn, low_first) in (1..=8).flat_map(|turn| [(turn, true), (turn, false)]) {
            let (across, shown) = decode(&noted(PLAIN, turn, low_first)).expect("is read with a note in it");
            assert_eq!((across, shown.len()), (if turn < 5 { width } else { height }, stored.len()), "turn {turn}: its size");
            let from = |at: usize| crate::exif_orientation::tests::from(turn as usize, [width, height], at % across, at / across);
            let wanted = (0..stored.len()).map(from).map(|(x, y)| stored[y * width + x]);
            assert!(shown.iter().copied().eq(wanted), "turn {turn}, low end first: {low_first}");
        }
        // An invalid orientation rotates nothing.
        assert_eq!(decode(&noted(PLAIN, 9, false)), Some((width, stored)));
    }

    #[test]
    fn padding_long_dividers_and_a_missing_end_change_nothing() {
        let wanted = decode(PLAIN);
        assert!(wanted.is_some());
        // Fill bytes of 0xFF may precede any marker.
        let frame = PLAIN.windows(2).position(|pair| pair == [0xFF, 0xC0]).expect("says what the picture is");
        assert_eq!(decode(&[&PLAIN[..frame], &[0xFF; 3], &PLAIN[frame..]].concat()), wanted);
        // The first quantization table again, at 16-bit precision.
        let table = PLAIN.windows(2).position(|pair| pair == [0xFF, 0xDB]).expect("has dividers");
        let long: Vec<u8> = [0xFF, 0xDB, 0, 131, PLAIN[table + 4] | 0x10].into_iter().chain(PLAIN[table + 5..table + 69].iter().flat_map(|divider| [0, *divider])).collect();
        assert_eq!(decode(&[&PLAIN[..table], &long, &PLAIN[table + 69..]].concat()), wanted);
        // EOI omitted, and trailing data after it.
        assert_eq!(decode(&PLAIN[..PLAIN.len() - 2]), wanted);
        assert_eq!(decode(&[PLAIN, b"and more"].concat()), wanted);
    }

    #[test]
    fn colour_stored_coarser_comes_back_smooth() {
        // An even gradient plane, subsampled each way: at a half, a quarter, a third, and at
        // factors not dividing the image. It must come back an even gradient, constant past its
        // outermost samples' centres.
        let (width, height) = (36usize, 37usize);
        let place = |at: usize, coarse: usize, fine: usize, count: usize| ((at as f64 + 0.5) * coarse as f64 / fine as f64 - 0.5).clamp(0.0, count as f64 - 1.0);
        for (across, down, most) in [(1, 1, (2, 1)), (1, 1, (2, 2)), (2, 2, (4, 4)), (1, 1, (1, 2)), (1, 1, (4, 1)), (1, 1, (3, 3)), (2, 1, (3, 2)), (3, 2, (4, 4))] {
            let units = (width.div_ceil(8 * most.0), height.div_ceil(8 * most.1));
            let (mut plane, mut deep) = (Plane::new(0, (across, down), 0, units, 8, false, false), Plane::new(0, (across, down), 0, units, 8, false, true));
            let (stride, wide, rows) = (plane.wide * 8, (width * across).div_ceil(most.0), (height * down).div_ceil(most.1));
            // The same plane twice: in bytes, and 16 times as steep in 12-bit samples.
            let level = |at: usize| (5 * (at % stride) + 4 * (at / stride)).min(255);
            plane.samples.iter_mut().enumerate().for_each(|(at, sample)| *sample = level(at) as u8);
            deep.deep.iter_mut().enumerate().for_each(|(at, sample)| *sample = 16 * level(at) as u16);
            let (spots, mut sums, mut line) = (plane.spread(width, most), vec![0; width + 1], vec![0; width + 2]);
            let (deep_spots, mut deep_line) = (deep.spread(width, most), vec![0; width]);
            for (x, y) in (0..height).flat_map(|y| (0..width).map(move |x| (x, y))) {
                plane.line(y, (width, height), most, &spots, &mut sums, &mut line);
                deep.deep_line(y, (width, height), most, &deep_spots, &mut deep_line);
                let wanted = 5.0 * place(x, across, most.0, wide) + 4.0 * place(y, down, most.1, rows);
                assert!((line[x] as f64 - wanted).abs() < 0.75, "{across} of {} across, {down} of {} down, at {x}, {y}: {} for {wanted}", most.0, most.1, line[x]);
                assert!((deep_line[x] as f64 / 16.0 - wanted).abs() < 0.75, "the same, twelve bits deep: {} for {wanted}", deep_line[x]);
            }
        }
    }

    #[test]
    fn levels_of_any_depth_come_to_eight_bits_as_libjpeg_shows_them() {
        // What `djpeg -png` of libjpeg-turbo 3.2 makes of these depths' levels. Deeper ones it
        // scales to 16 bits, rounded; a screen shows the top 8 of those, which are the level's
        // own top 8.
        assert_eq!(shown(2), [0, 85, 170, 255]);
        assert_eq!(shown(3), [0, 36, 73, 109, 146, 182, 219, 255]);
        assert_eq!(shown(5)[..4], [0, 8, 16, 25]);
        assert!(shown(8).iter().copied().eq(0..=255));
        assert_eq!([0, 1, 2047, 2048, 4079, 4080, 4095].map(|level| shown(12)[level]), [0, 0, 127, 128, 254, 255, 255]);
        assert!(shown(16).iter().enumerate().all(|(level, byte)| *byte as usize == level >> 8));
    }

    /// A lossless JPEG written per the standard: `notes`, then one frame (see `lossless_frame`).
    fn without_loss(size: (usize, usize), depth: u32, planes: &[(usize, usize)], sample: impl Fn(usize, usize, usize) -> u16, coding: (u8, u32, usize), notes: &[u8]) -> Vec<u8> {
        [&[0xFF, 0xD8][..], notes, &lossless_frame(0xC3, size, depth, planes, sample, coding), &[0xFF, 0xD9]].concat()
    }

    /// A lossless frame of kind `kind` (0xC3, or 0xC7 differential) written per the standard. All
    /// planes in one scan; each given its sampling factors, and `sample` gives a plane's sample
    /// at a position. Every sample uses predictor `rule` (0: none, in a differential frame) with
    /// point transform `shift`, restarting every `rows` MCU rows (0: never).
    fn lossless_frame(kind: u8, size: (usize, usize), depth: u32, planes: &[(usize, usize)], sample: impl Fn(usize, usize, usize) -> u16, (rule, shift, rows): (u8, u32, usize)) -> Vec<u8> {
        let most = (planes.iter().map(|plane| plane.0).max().unwrap(), planes.iter().map(|plane| plane.1).max().unwrap());
        let (wide, high) = (size.0.div_ceil(most.0), size.1.div_ceil(most.1));
        // Code length per difference bit count, 0 to 16; the codes count up through them.
        const LONG: [u8; 17] = [2, 3, 3, 3, 3, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];
        let (mut codes, mut next) = ([0u32; 17], 0);
        for bits in 0..17 {
            next <<= LONG[bits] - LONG[bits.saturating_sub(1)];
            codes[bits] = next;
            next += 1;
        }
        let part = |kind: u8, body: &[u8]| [&[0xFF, kind][..], &(body.len() as u16 + 2).to_be_bytes(), body].concat();
        let listed: Vec<u8> = planes.iter().enumerate().flat_map(|(n, plane)| [n as u8 + 1, (plane.0 << 4 | plane.1) as u8, 0]).collect();
        let counts: Vec<u8> = (1..=16).map(|long| LONG.iter().filter(|code| **code == long).count() as u8).collect();
        let chosen: Vec<u8> = (1..=planes.len() as u8).flat_map(|name| [name, 0]).collect();
        let mut file = Vec::new();
        file.extend(part(kind, &[&[depth as u8, (size.1 >> 8) as u8, size.1 as u8, (size.0 >> 8) as u8, size.0 as u8, planes.len() as u8][..], &listed].concat()));
        file.extend(part(0xC4, &[&[0][..], &counts, &(0..17).collect::<Vec<u8>>()].concat()));
        file.extend(part(0xDD, &((rows * wide) as u16).to_be_bytes()));
        file.extend(part(0xDA, &[&[planes.len() as u8][..], &chosen, &[rule, 0, shift as u8]].concat()));
        // An interval's bits as bytes: the last padded with ones, and a stuffed zero after each
        // 0xFF.
        let written = |bits: &mut Vec<bool>| {
            bits.resize(bits.len().div_ceil(8) * 8, true);
            let bytes: Vec<u8> = bits.chunks(8).map(|byte| byte.iter().fold(0, |byte, bit| byte << 1 | *bit as u8)).collect();
            bits.clear();
            bytes.into_iter().flat_map(|byte| if byte == 0xFF { vec![0xFF, 0] } else { vec![byte] }).collect::<Vec<u8>>()
        };
        // A sample as stored: without its dropped bits, and past the plane's edge, the edge's.
        let stored = |n: usize, x: usize, y: usize| {
            let (across, down) = ((size.0 * planes[n].0).div_ceil(most.0), (size.1 * planes[n].1).div_ceil(most.1));
            (sample(n, x.min(across - 1), y.min(down - 1)) >> shift) as i32
        };
        let (mut bits, mut top) = (Vec::new(), 0);
        for (row, column) in (0..high).flat_map(|row| (0..wide).map(move |column| (row, column))) {
            if rows != 0 && row != 0 && column == 0 && row % rows == 0 {
                file.extend(written(&mut bits));
                file.extend([0xFF, 0xD0 + ((row / rows + 7) % 8) as u8]);
                top = row;
            }
            for (n, (x, y)) in planes.iter().enumerate().flat_map(|(n, plane)| (0..plane.0 * plane.1).map(move |at| (n, (column * plane.0 + at % plane.0, row * plane.1 + at / plane.0)))) {
                let (before, above, slant) = (stored(n, x.saturating_sub(1), y), stored(n, x, y.saturating_sub(1)), stored(n, x.saturating_sub(1), y.saturating_sub(1)));
                let guess = match (y == top * planes[n].1, x == 0, rule) {
                    (_, _, 0) => 0,
                    (true, true, _) => 1 << (depth - shift - 1),
                    (true, false, _) | (false, false, 1) => before,
                    (false, true, _) | (_, _, 2) => above,
                    (_, _, 3) => slant,
                    (_, _, 4) => before + above - slant,
                    (_, _, 5) => before + ((above - slant) >> 1),
                    (_, _, 6) => above + ((before - slant) >> 1),
                    _ => (before + above) >> 1,
                };
                // The difference modulo 2^16; -32768 has no bits after its size, and a negative
                // one is stored as one less.
                let change = (stored(n, x, y) - guess) as i16 as i32;
                let long = if change == -32768 { 16 } else { 32 - change.unsigned_abs().leading_zeros() as usize };
                bits.extend((0..LONG[long]).rev().map(|bit| codes[long] >> bit & 1 == 1));
                bits.extend((0..long % 16).rev().map(|bit| (change - (change < 0) as i32) >> bit & 1 == 1));
            }
        }
        file.extend(written(&mut bits));
        file
    }

    #[test]
    fn pictures_without_loss_come_back_sample_for_sample() {
        // Random samples at every depth with every predictor: grey, three planes of one size,
        // and three of different sizes (marked RGB by Adobe's marker); with and without point
        // transform and restarts.
        let noise = |n: usize, x: usize, y: usize| ((n * 7919 + x * 31 + y * 17 + x * y) as u32).wrapping_mul(2_654_435_761) >> 12;
        let adobe = [&[0xFF, 0xEE, 0, 14][..], b"Adobe\0d\0\0\0\0\0"].concat();
        let layouts: [&[(usize, usize)]; 4] = [&[(1, 1)], &[(1, 1); 3], &[(2, 2), (1, 1), (1, 1)], &[(1, 2), (2, 1), (1, 1)]];
        for (depth, rule) in (2..=16).flat_map(|depth| (1..=7).map(move |rule| (depth, rule))) {
            let (planes, size) = (layouts[(depth as usize + rule) % 4], (3 + rule + depth as usize % 4, 9 - rule + depth as usize % 3));
            let (shift, rows) = ((depth * rule as u32 % 5).min(depth - 1) * (rule as u32 % 2), (depth as usize + rule) % 3);
            let sample = |n: usize, x: usize, y: usize| (noise(n, x, y) & ((1 << depth) - 1)) as u16;
            let file = without_loss(size, depth, planes, sample, (rule as u8, shift, rows), &adobe);
            let most = (planes.iter().map(|plane| plane.0).max().unwrap(), planes.iter().map(|plane| plane.1).max().unwrap());
            // Each pixel has the sample of every plane covering it, minus the dropped bits.
            let level = |n: usize, x: usize, y: usize| {
                shown(depth)[(sample(n.min(planes.len() - 1), x * planes[n.min(planes.len() - 1)].0 / most.0, y * planes[n.min(planes.len() - 1)].1 / most.1) >> shift << shift) as usize] as u32
            };
            let wanted: Vec<u32> = (0..size.0 * size.1).map(|at| (at % size.0, at / size.0)).map(|(x, y)| 0xff00_0000 | level(0, x, y) << 16 | level(1, x, y) << 8 | level(2, x, y)).collect();
            assert_eq!(decode(&file), Some((size.0, wanted)), "{depth} bits, rule {rule}, {shift} bits left out, afresh every {rows} rows, {} planes", planes.len());
        }
    }

    #[test]
    fn what_the_planes_of_a_picture_without_loss_are_goes_by_its_notes_and_their_sizes() {
        // Three planes, the first at 200 and the others at mid-level: greyish red if RGB, light
        // grey if YCbCr.
        let stored = |planes: &[(usize, usize)], notes: &[u8]| decode(&without_loss((5, 3), 8, planes, |n, _, _| if n == 0 { 200 } else { 128 }, (1, 0, 0), notes));
        let (colours, bright) = (Some((5, vec![0xffc8_8080; 15])), Some((5, vec![0xffc8_c8c8; 15])));
        let adobe = |kind: u8| [&[0xFF, 0xEE, 0, 14][..], b"Adobe\0d\0\0\0\0", &[kind]].concat();
        let jfif = [&[0xFF, 0xE0, 0, 16][..], b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"].concat();
        // Same-size planes with no marker are RGB; a marker says otherwise.
        assert_eq!([stored(&[(1, 1); 3], &[]), stored(&[(1, 1); 3], &adobe(0))], [colours.clone(), colours.clone()]);
        assert_eq!([stored(&[(1, 1); 3], &jfif), stored(&[(1, 1); 3], &adobe(1))], [bright.clone(), bright.clone()]);
        // Different-size planes with no marker are YCbCr.
        assert_eq!([stored(&[(2, 2), (1, 1), (1, 1)], &[]), stored(&[(2, 2), (1, 1), (1, 1)], &adobe(0))], [bright, colours]);
    }

    /// A hierarchical lossless JPEG per the standard, `size` pixels in `planes` (sampling
    /// factors) of `depth` bits: a first frame of `sample`'s planes, half as wide or high where
    /// `expand` says, with predictor `rule`; then a differential frame of what turns those, doubled
    /// as the standard doubles them, into `sample`'s, with point transform `shift`, restarting
    /// every `rows` MCU rows. Returns the file, and what it decodes to: each plane's samples in
    /// rows.
    #[cfg(feature = "rare-jpeg")]
    fn hierarchical(
        size: (usize, usize),
        depth: u32,
        planes: &[(usize, usize)],
        sample: &dyn Fn(usize, usize, usize) -> u16,
        expand: (bool, bool),
        (rule, shift, rows): (u8, u32, usize),
    ) -> (Vec<u8>, Vec<Vec<u16>>) {
        let most = (planes.iter().map(|plane| plane.0).max().unwrap(), planes.iter().map(|plane| plane.1).max().unwrap());
        let small = (if expand.0 { size.0.div_ceil(2) } else { size.0 }, if expand.1 { size.1.div_ceil(2) } else { size.1 });
        let sides = |n: usize, (width, height): (usize, usize)| ((width * planes[n].0).div_ceil(most.0), (height * planes[n].1).div_ceil(most.1));
        // The first frame takes every other sample where halved.
        let low = |n: usize, x: usize, y: usize| sample(n, (x << expand.0 as usize).min(sides(n, size).0 - 1), (y << expand.1 as usize).min(sides(n, size).1 - 1));
        // Doubled, each new sample is the mean of those either side, rounded down: across, then
        // down. Past the edge, the last sample repeats.
        let across = |n: usize, x: usize, y: usize| match expand.0 {
            false => low(n, x, y) as u32,
            true => (low(n, x / 2, y) as u32 + low(n, (x / 2 + x % 2).min(sides(n, small).0 - 1), y) as u32) / 2,
        };
        let doubled = move |n: usize, x: usize, y: usize| match expand.1 {
            false => across(n, x, y) as u16,
            true => ((across(n, x, y / 2) + across(n, x, (y / 2 + y % 2).min(sides(n, small).1 - 1))) / 2) as u16,
        };
        let difference = |n: usize, x: usize, y: usize| sample(n, x, y).wrapping_sub(doubled(n, x, y));
        let part = |kind: u8, body: &[u8]| [&[0xFF, kind][..], &(body.len() as u16 + 2).to_be_bytes(), body].concat();
        let listed: Vec<u8> = planes.iter().enumerate().flat_map(|(n, plane)| [n as u8 + 1, (plane.0 << 4 | plane.1) as u8, 0]).collect();
        let whole = [&[depth as u8, (size.1 >> 8) as u8, size.1 as u8, (size.0 >> 8) as u8, size.0 as u8, planes.len() as u8][..], &listed].concat();
        let adobe = [&[0xFF, 0xEE, 0, 14][..], b"Adobe\0d\0\0\0\0\0"].concat();
        let first = lossless_frame(0xC3, small, depth, planes, low, (rule, 0, 0));
        let then = lossless_frame(0xC7, size, depth, planes, difference, (0, shift, rows));
        let file = [&[0xFF, 0xD8][..], &adobe, &part(0xDE, &whole), &first, &part(0xDF, &[(expand.0 as u8) << 4 | expand.1 as u8]), &then, &[0xFF, 0xD9]].concat();
        // What is dropped of each difference is missing from the image.
        let made = |n: usize| {
            let width = sides(n, size).0;
            (0..width * sides(n, size).1).map(|at| doubled(n, at % width, at / width).wrapping_add(difference(n, at % width, at / width) >> shift << shift)).collect()
        };
        (file, (0..planes.len()).map(made).collect())
    }

    #[test]
    #[cfg(feature = "rare-jpeg")]
    fn hierarchical_pictures_without_loss_come_back_sample_for_sample() {
        // Random samples at four depths: a first frame halved across, down, both or neither,
        // then what doubles it to the image; grey, three planes of one size, and three of
        // different sizes; with and without point transform and restarts.
        let noise = |n: usize, x: usize, y: usize| ((n * 7919 + x * 31 + y * 17 + x * y) as u32).wrapping_mul(2_654_435_761) >> 12;
        let layouts: [&[(usize, usize)]; 4] = [&[(1, 1)], &[(1, 1); 3], &[(2, 2), (1, 1), (1, 1)], &[(1, 2), (2, 1), (1, 1)]];
        let expands = [(true, true), (true, false), (false, true), (false, false)];
        for (depth, (turn, expand)) in [2, 8, 12, 16].into_iter().flat_map(|depth| expands.into_iter().enumerate().map(move |each| (depth, each))) {
            let (planes, size, rule) = (layouts[(depth as usize + turn) % 4], (5 + turn * 3 + depth as usize % 5, 11 - turn), (turn + depth as usize) as u8 % 7 + 1);
            let (shift, rows) = ((turn as u32 * depth / 4).min(depth - 1), (depth as usize + turn) % 3);
            let sample = |n: usize, x: usize, y: usize| (noise(n, x, y) & ((1 << depth) - 1)) as u16;
            let (file, made) = hierarchical(size, depth, planes, &sample, expand, (rule, shift, rows));
            let most = (planes.iter().map(|plane| plane.0).max().unwrap(), planes.iter().map(|plane| plane.1).max().unwrap());
            // Each pixel has the sample of every plane covering it; a level beyond the depth
            // (where dropped bits pushed it past an end) is clamped.
            let level = |n: usize, x: usize, y: usize| {
                let n = n.min(planes.len() - 1);
                let made = made[n][y * planes[n].1 / most.1 * (size.0 * planes[n].0).div_ceil(most.0) + x * planes[n].0 / most.0];
                shown(depth)[if depth == 16 { made as usize } else { (made as i16).clamp(0, (1 << depth) - 1) as usize }] as u32
            };
            let wanted: Vec<u32> = (0..size.0 * size.1).map(|at| (at % size.0, at / size.0)).map(|(x, y)| 0xff00_0000 | level(0, x, y) << 16 | level(1, x, y) << 8 | level(2, x, y)).collect();
            assert_eq!(decode(&file), Some((size.0, wanted)), "{depth} bits, doubled {expand:?}, {shift} bits left out, afresh every {rows} rows, {} planes", planes.len());
        }
    }

    #[test]
    fn guesses_are_sorted_as_the_file_says() {
        // The image sets its conditioning. Decoded as if it hadn't, it is another image, or
        // none.
        let said = ARITHMETIC.windows(2).position(|pair| pair == [0xFF, 0xCC]).expect("says how they are sorted");
        let silent = [&ARITHMETIC[..said], &ARITHMETIC[said + 2 + ARITHMETIC[said + 3] as usize..]].concat();
        assert!(decode(ARITHMETIC).is_some() && decode(&silent) != decode(ARITHMETIC));
        // Bounds in order with an edge; bounds out of order; a nonexistent table; half a pair.
        let sorting = |body: &[u8]| Jpeg::new(0).sorting(body);
        assert_eq!([sorting(&[0x00, 0x31, 0x13, 9]), sorting(&[0x00, 0x13]), sorting(&[0x24, 5]), sorting(&[0x00])], [Some(()), None, None, None]);
    }

    #[test]
    fn a_damaged_file_gives_nothing_or_some_picture() {
        let sound = |read: Option<(usize, Vec<u32>)>| read.is_none_or(|(width, pixels)| width > 0 && !pixels.is_empty() && pixels.len() % width == 0);
        let every = PICTURES.into_iter().map(|(_, jpeg, _)| jpeg).chain(BY_THE_REFERENCE.into_iter().map(|(_, jpeg, ..)| jpeg));
        for (jpeg, at) in every.flat_map(|jpeg| (0..jpeg.len()).step_by(17).map(move |at| (jpeg, at))) {
            // Truncated there; then whole, with a bit flipped there; then with a marker byte
            // there.
            assert!(sound(decode(&jpeg[..at])));
            let mut damaged = jpeg.to_vec();
            damaged[at] ^= 1 << (at % 8);
            assert!(sound(decode(&damaged)));
            damaged[at] = 0xFF;
            assert!(sound(decode(&damaged)));
        }
        // A baseline or lossless image truncated anywhere before its end isn't decoded at all.
        assert!([PLAIN, EXACT].iter().all(|jpeg| (0..jpeg.len() - 2).step_by(7).all(|cut| decode(&jpeg[..cut]).is_none())));
    }

    #[test]
    fn what_is_no_jpeg_or_of_a_kind_that_is_not_read_gives_nothing() {
        assert_eq!(decode(b"not a picture"), None);
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&PLAIN[..2]), None);
        assert_eq!(decode(include_bytes!("../tests/pictures/plain.png")), None);
        let part = |jpeg: &[u8], kind: u8| jpeg.windows(2).position(|pair| pair == [0xFF, kind]).expect("has such a part");
        let changed = |jpeg: &[u8], at: usize, to: u8| decode(&[&jpeg[..at], &[to], &jpeg[at + 1..]].concat());
        // Differential frames (0xC5 to 0xC7, and 0xCD to 0xCF arithmetic coded) outside a
        // hierarchical image, and the reserved kind.
        let frame = part(PLAIN, 0xC0);
        for kind in [0xC5, 0xC6, 0xC7, 0xC8, 0xCD, 0xCE, 0xCF] {
            assert_eq!(changed(PLAIN, frame + 1, kind), None, "kind {kind:#x}");
        }
        // A depth DCT doesn't come in, and depths no sample has.
        let exact = part(EXACT, 0xC3);
        assert_eq!([changed(PLAIN, frame + 4, 10), changed(EXACT, exact + 4, 1), changed(EXACT, exact + 4, 17)], [None, None, None]);
        // Lossless: a nonexistent predictor, a point transform beyond the depth, and a restart
        // interval splitting a row.
        let (pass, afresh) = (part(EXACT, 0xDA), part(EXACT, 0xDD));
        assert_eq!([changed(EXACT, pass + 11, 0), changed(EXACT, pass + 11, 8), changed(EXACT, pass + 13, 8), changed(EXACT, afresh + 5, 119)], [None, None, None, None]);
        // A 65535 by 65535 size, which a file this small could never hold: refused when read,
        // before anything is allocated.
        let told = |size: [u8; 4]| Jpeg::new(PLAIN.len()).frame(&[&[8], &size[..], &PLAIN[frame + 9..frame + 19]].concat(), 0xC0);
        assert_eq!((told([0, 45, 0, 67]), told([0xFF; 4])), (Some(()), None));
        assert_eq!(decode(&[&PLAIN[..frame + 5], &[0xFF; 4], &PLAIN[frame + 9..]].concat()), None);
    }

    #[test]
    #[cfg(not(feature = "rare-jpeg"))]
    fn the_rarest_kinds_are_read_only_with_their_feature() {
        assert!(BY_THE_REFERENCE.iter().all(|(_, jpeg, ..)| decode(jpeg).is_none()));
    }

    #[test]
    #[cfg(feature = "rare-jpeg")]
    fn a_hierarchical_picture_that_contradicts_itself_gives_nothing() {
        let (file, _) = hierarchical((9, 5), 8, &[(1, 1)], &|_, x, y| (x * y) as u16, (true, true), (1, 0, 0));
        assert!(decode(&file).is_some());
        let part = |kind: u8| file.windows(2).position(|pair| pair == [0xFF, kind]).expect("has such a part");
        let changed = |at: usize, to: u8| decode(&[&file[..at], &[to], &file[at + 1..]].concat());
        let (whole, first, then, expand) = (part(0xDE), part(0xC3), part(0xC7), part(0xDF));
        // A frame wider than the image; the differential frame first; a plane the image lacks;
        // the second frame not differential; a frame at another depth; doubling by more than
        // two.
        let wrongs = [changed(first + 8, 10), changed(first + 1, 0xC7), changed(first + 10, 9), changed(then + 1, 0xC3), changed(then + 4, 12), changed(expand + 4, 0x21)];
        assert_eq!(wrongs, [None, None, None, None, None, None]);
        // A second DHP segment; and one after the first frame.
        let dhp = &file[whole..whole + 13];
        assert_eq!(decode(&[&file[..first], dhp, &file[first..]].concat()), None);
        assert_eq!(decode(&[&file[..whole], &file[whole + 13..expand], dhp, &file[expand..]].concat()), None);
        // Each frame takes its blocks' bits from the room the file has: two frames of 64 by 64
        // samples need at least 1024 bytes, though each alone fits in 600. The second is refused
        // before anything is allocated for it.
        let (grey, mut read) = ([8, 0, 64, 0, 64, 1, 1, 0x11, 0], Jpeg::new(600));
        assert_eq!([read.hierarchy(&grey), read.frame(&grey, 0xCB)], [Some(()), Some(())]);
        read.planes[0].brought[0] = Some(0);
        assert_eq!(read.frame(&grey, 0xCF), None);
    }

    /// A `width` by `height` test image: gradients across and down, rings, sharp edges and a
    /// little noise, exercising every frequency. (Integer math, so it is the same on every
    /// system.)
    fn photo(width: usize, height: usize) -> Vec<u32> {
        let noise = |at: usize| (at as u32).wrapping_mul(2_654_435_761) >> 28;
        let pixel = |at: usize| {
            let (x, y) = (at % width, at / width);
            let edge = ((x / 6 + y / 4) % 3 == 0) as u32 * 90;
            let red = (x * 255 / width) as u32 ^ noise(at);
            let green = ((y * 255 / height) as u32 + edge).min(255);
            0xff00_0000 | red << 16 | green << 8 | ((x * x + y * y) / 3 % 256) as u32
        };
        (0..width * height).map(pixel).collect()
    }

    /// The size of the image in the libjpeg-written files kept with the tests.
    const WRITTEN: [usize; 2] = [45, 29];

    /// Those files, each with its quality: below 90 with chroma halved each way, from 90 full.
    /// (At 3 and 20 the scaled tables exceed 255, and are clamped.)
    const BY_LIBJPEG: [(u8, &[u8]); 6] = [
        (3, include_bytes!("../tests/pictures/written-3.jpg")),
        (20, include_bytes!("../tests/pictures/written-20.jpg")),
        (50, include_bytes!("../tests/pictures/written-50.jpg")),
        (75, include_bytes!("../tests/pictures/written-75.jpg")),
        (90, include_bytes!("../tests/pictures/written-90.jpg")),
        (100, include_bytes!("../tests/pictures/written-100.jpg")),
    ];

    /// The same image written progressive by libjpeg (`cjpeg -progressive`), at three
    /// qualities.
    const PROGRESSIVE_BY_LIBJPEG: [(u8, &[u8]); 3] = [
        (20, include_bytes!("../tests/pictures/written-progressive-20.jpg")),
        (75, include_bytes!("../tests/pictures/written-progressive-75.jpg")),
        (95, include_bytes!("../tests/pictures/written-progressive-95.jpg")),
    ];

    /// The same at quality 75 with `P3`'s ICC profile (`cjpeg -progressive -icc`).
    const WITH_PROFILE_BY_LIBJPEG: &[u8] = include_bytes!("../tests/pictures/written-profile.jpg");

    /// A Display P3 photo: its ICC profile is `profile`'s.
    const P3: &[u8] = include_bytes!("../tests/pictures/p3.jpg");

    /// The ICC profile in a JPEG, from its APP2 segments (in order, as these tests' files have
    /// them).
    fn profile(jpeg: &[u8]) -> Vec<u8> {
        let mut rest = &jpeg[2..];
        let mut profile = Vec::new();
        while let Some((kind, body, after)) = part(rest).filter(|(kind, ..)| *kind != 0xDA) {
            profile.extend(body.strip_prefix(b"ICC_PROFILE\0").map_or(&[][..], |piece| &piece[2..]).iter().filter(|_| kind == 0xE2));
            rest = after;
        }
        profile
    }

    /// The system's libjpeg-turbo tools: `cjpeg` encodes an image (as PPM) as `encode` would,
    /// chroma halved below quality 90, as `how` says (`-baseline`, `-progressive`, `-icc`);
    /// `djpeg` decodes. Each returns its output, or `None` if it refused.
    fn by_libjpeg(width: usize, pixels: &[u32], quality: u8, how: &[&str]) -> Option<Vec<u8>> {
        use std::process::{Command, Stdio};
        let work = std::env::temp_dir().join(format!("taut-written-{}-{width}-{quality}", std::process::id()));
        let picture = [format!("P6 {width} {} 255\n", pixels.len() / width).as_bytes(), &pixels.iter().flat_map(|pixel| [16, 8, 0].map(|shift| (pixel >> shift) as u8)).collect::<Vec<u8>>()].concat();
        std::fs::write(work.with_extension("ppm"), picture).ok()?;
        let sampled = if quality < 90 { "2x2" } else { "1x1" };
        let quality = quality.to_string();
        let mut written = Command::new("cjpeg").args(how).args(["-quality", &quality, "-sample", sampled]).arg(work.with_extension("ppm")).stderr(Stdio::null()).output().ok()?;
        let _ = std::fs::remove_file(work.with_extension("ppm"));
        written.status.success().then(|| std::mem::take(&mut written.stdout))
    }

    /// What libjpeg-turbo's `djpeg` decodes of a JPEG: its pixels, `0xffRRGGBB`.
    fn read_by_libjpeg(jpeg: &[u8]) -> Option<Vec<u32>> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut reading = Command::new("djpeg").arg("-ppm").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        reading.stdin.take()?.write_all(jpeg).ok()?;
        let read = reading.wait_with_output().ok().filter(|read| read.status.success())?.stdout;
        // After "P6": width, height and maxval, each followed by whitespace.
        let start = read.iter().enumerate().filter(|(_, byte)| byte.is_ascii_whitespace()).nth(3)?.0 + 1;
        Some(read[start..].as_chunks::<3>().0.iter().map(|rgb| 0xff00_0000 | (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32).collect())
    }

    /// Regenerates `BY_LIBJPEG`, `PROGRESSIVE_BY_LIBJPEG` and `WITH_PROFILE_BY_LIBJPEG`, then
    /// compares `Encoder` with libjpeg-turbo for images of many sizes at many qualities,
    /// baseline and progressive: its files must be libjpeg's byte for byte, and `djpeg`'s
    /// decoding of them must equal `decode`'s. Then asks exiv2 and vips for a file's metadata.
    /// (`cargo test --lib jpeg -- --ignored`, with libjpeg-turbo's `cjpeg` and `djpeg`, exiv2
    /// and vips installed.)
    #[test]
    #[ignore = "needs libjpeg-turbo's tools, exiv2 and vips, and writes the files the other tests read"]
    fn the_written_pictures_are_made_afresh() {
        let (pictures, written) = (std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures"), photo(WRITTEN[0], WRITTEN[1]));
        for (quality, _) in BY_LIBJPEG {
            std::fs::write(pictures.join(format!("written-{quality}.jpg")), by_libjpeg(WRITTEN[0], &written, quality, &["-baseline"]).unwrap()).unwrap();
        }
        for (quality, _) in PROGRESSIVE_BY_LIBJPEG {
            std::fs::write(pictures.join(format!("written-progressive-{quality}.jpg")), by_libjpeg(WRITTEN[0], &written, quality, &["-progressive"]).unwrap()).unwrap();
        }
        let icc = std::env::temp_dir().join(format!("taut-written-{}.icc", std::process::id()));
        std::fs::write(&icc, profile(P3)).unwrap();
        std::fs::write(pictures.join("written-profile.jpg"), by_libjpeg(WRITTEN[0], &written, 75, &["-progressive", "-icc", icc.to_str().unwrap()]).unwrap()).unwrap();
        std::fs::remove_file(icc).unwrap();
        let sizes = [[1, 1], [1, 17], [23, 1], [8, 8], [16, 16], [17, 15], [31, 33], [64, 48], [100, 7], [7, 100], [203, 151]];
        let every = sizes.into_iter().flat_map(|size| [1, 5, 25, 40, 60, 85, 89, 90, 95, 99, 100].map(|quality| (size, quality)));
        for (([width, height], quality), progressive) in every.flat_map(|each| [(each, false), (each, true)]) {
            let (pixels, how) = (photo(width, height), if progressive { "-progressive" } else { "-baseline" });
            let ours = if progressive { Encoder::new(quality).progressive().encode(width, &pixels) } else { encode(width, &pixels, quality) }.unwrap();
            assert!(by_libjpeg(width, &pixels, quality, &[how]).is_some_and(|theirs| theirs == ours), "{width} by {height} at {quality}, {how}: not libjpeg's file");
            assert_eq!(read_by_libjpeg(&ours), decode(&ours).map(|(_, read)| read), "{width} by {height} at {quality}, {how}: read otherwise");
        }
        metadata_is_found_by_exiv2_and_vips();
    }

    #[test]
    fn a_picture_is_written_as_libjpeg_writes_it() {
        let pixels = photo(WRITTEN[0], WRITTEN[1]);
        for (quality, file) in BY_LIBJPEG {
            assert!(encode(WRITTEN[0], &pixels, quality).is_some_and(|written| written == file), "quality {quality}");
        }
        for (quality, file) in PROGRESSIVE_BY_LIBJPEG {
            assert!(Encoder::new(quality).progressive().encode(WRITTEN[0], &pixels).is_some_and(|written| written == file), "progressive, quality {quality}");
        }
        // An ICC profile goes after JFIF, in APP2 segments.
        let profile = profile(P3);
        assert!(Encoder::new(75).progressive().icc_profile(&profile).encode(WRITTEN[0], &pixels).is_some_and(|written| written == WITH_PROFILE_BY_LIBJPEG));
        // Alpha is ignored; a quality past either end is clamped.
        let clear: Vec<u32> = pixels.iter().map(|pixel| pixel & 0xff_ffff).collect();
        assert!(encode(WRITTEN[0], &clear, 75).is_some_and(|written| written == BY_LIBJPEG[3].1));
        assert_eq!([encode(WRITTEN[0], &pixels, 0), encode(WRITTEN[0], &pixels, 255)], [encode(WRITTEN[0], &pixels, 1), encode(WRITTEN[0], &pixels, 100)]);
    }

    #[test]
    fn what_is_written_is_read_back_close_to_what_it_was() {
        // Every size against blocks and MCUs (one pixel, a row or column, whole MCUs, a pixel
        // past them), at every quality: finer is closer. (Mean and largest per-channel error.)
        let sizes = [[1, 1], [1, 9], [9, 1], [16, 16], [17, 17], [40, 23]];
        for ([width, height], quality) in sizes.into_iter().flat_map(|size| [1, 30, 75, 90, 100].map(|quality| (size, quality))) {
            let pixels = photo(width, height);
            let (read_width, read) = decode(&encode(width, &pixels, quality).unwrap()).unwrap();
            assert_eq!((read_width, read.len()), (width, pixels.len()), "{width} by {height} at {quality}: its size");
            let off: Vec<u32> = pixels.iter().zip(&read).flat_map(|(one, other)| [16, 8, 0].map(|shift| (one >> shift & 255).abs_diff(other >> shift & 255))).collect();
            let (mean, most) = (off.iter().sum::<u32>() as f32 / off.len() as f32, *off.iter().max().unwrap());
            let (mean_at_most, most_at_most) = match quality {
                100 => (1.0, 4),
                90 => (5.0, 30),
                75 => (13.0, 150),
                30 => (20.0, 180),
                _ => (35.0, 255),
            };
            assert!(mean <= mean_at_most && most <= most_at_most, "{width} by {height} at {quality}: off by {mean} on the whole, {most} at the most");
        }
    }

    #[test]
    fn a_picture_of_no_size_or_too_large_is_not_written() {
        assert_eq!([encode(0, &[1, 2], 75), encode(3, &[], 75), encode(3, &[1, 2], 75)], [None, None, None]);
        assert!(encode(0xFFFF, &vec![0; 0xFFFF], 75).is_some() && encode(1, &vec![0; 0xFFFF], 75).is_some());
        assert_eq!([encode(0x1_0000, &vec![0; 0x1_0000], 75), encode(1, &vec![0; 0x1_0000], 75)], [None, None]);
        // Progressive too.
        let progressive = |width: usize, height: usize| Encoder::new(75).progressive().encode(width, &vec![0; width * height]);
        assert!(progressive(0xFFFF, 1).is_some() && progressive(1, 0xFFFF).is_some() && progressive(0x1_0000, 1).is_none());
    }

    #[test]
    fn a_progressive_picture_is_read_back_as_its_baseline_twin() {
        // The same coefficients, sent in other scans: decoded alike, pixel for pixel. A few
        // percent smaller. (From quality 25: below, progressive keeps quantizers past 255, as
        // libjpeg does, where baseline stops at 255.)
        for ([width, height], quality) in [[1, 1], [9, 1], [1, 9], [17, 17], [40, 23], [203, 151]].into_iter().flat_map(|size| [25, 50, 89, 90, 100].map(|quality| (size, quality))) {
            let pixels = photo(width, height);
            let (baseline, progressive) = (encode(width, &pixels, quality).unwrap(), Encoder::new(quality).progressive().encode(width, &pixels).unwrap());
            assert_eq!(decode(&progressive), decode(&baseline), "{width} by {height} at {quality}");
            assert!(width < 100 || progressive.len() < baseline.len(), "{width} by {height} at {quality}: {} bytes progressive, {} baseline", progressive.len(), baseline.len());
        }
    }

    /// The APP segments of a JPEG up to its first frame: each marker and payload.
    fn notes(jpeg: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut rest = &jpeg[2..];
        let mut notes = Vec::new();
        while let Some((kind, body, after)) = part(rest).filter(|(kind, ..)| (0xE0..=0xEF).contains(kind)) {
            (notes, rest) = ([notes, vec![(kind, body.to_vec())]].concat(), after);
        }
        notes
    }

    #[test]
    fn metadata_is_written_where_readers_look_for_it() {
        let (pixels, width) = (photo(WRITTEN[0], WRITTEN[1]), WRITTEN[0]);
        let plain = encode(width, &pixels, 75).unwrap();
        // EXIF orientation: `decode` turns the picture as a note put in by hand turns it, each
        // of eight ways.
        for turn in 1..=8 {
            let written = Encoder::new(75).orientation(turn).encode(width, &pixels).unwrap();
            assert_eq!(decode(&written), decode(&noted(&plain, turn as u16, false)), "turned {turn}");
        }
        // JFIF first, then EXIF, XMP and the profile, each as given.
        let (packet, profile) = ("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>", profile(P3));
        let written = Encoder::new(75).orientation(6).xmp(packet).icc_profile(&profile).encode(width, &pixels).unwrap();
        let kinds: Vec<u8> = notes(&written).iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, [0xE0, 0xE1, 0xE1, 0xE2]);
        assert_eq!(notes(&written)[2].1, [&b"http://ns.adobe.com/xap/1.0/\0"[..], packet.as_bytes()].concat());
        assert_eq!(crate::exif_orientation::turn(&notes(&written)[1].1), Some(6));
        assert_eq!(self::profile(&written), profile);
        // The image itself is the same as without them.
        assert_eq!(written[written.len() - plain.len() + 20..], plain[20..]);
        // A profile too long for 255 segments is split no further: `None`. As is an orientation
        // that isn't one, and XMP longer than a segment holds.
        let (long, longer) = (vec![0; 255 * 65519], vec![0; 255 * 65519 + 1]);
        assert!(Encoder::new(75).icc_profile(&long).encode(1, &[0]).is_some_and(|written| notes(&written).len() == 256));
        assert_eq!(Encoder::new(75).icc_profile(&longer).encode(1, &[0]), None);
        assert_eq!([Encoder::new(75).orientation(0).encode(1, &[0]), Encoder::new(75).orientation(9).encode(1, &[0])], [None, None]);
        let (most, more) = ("x".repeat(65504), "x".repeat(65505));
        assert_eq!([Encoder::new(75).xmp(&most).encode(1, &[0]).is_some(), Encoder::new(75).xmp(&more).encode(1, &[0]).is_some()], [true, false]);
    }

    /// Writes a JPEG with all of its metadata, and asks exiv2 and vips what they find in it.
    fn metadata_is_found_by_exiv2_and_vips() {
        use std::process::Command;
        let (packet, profile) = (
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:format=\"image/jpeg\"/></rdf:RDF></x:xmpmeta>",
            profile(P3),
        );
        let pixels = photo(WRITTEN[0], WRITTEN[1]);
        let file = std::env::temp_dir().join(format!("taut-noted-{}.jpg", std::process::id()));
        std::fs::write(&file, Encoder::new(75).progressive().orientation(6).xmp(packet).icc_profile(&profile).encode(WRITTEN[0], &pixels).unwrap()).unwrap();
        let said = |program: &str, arguments: &[&str]| String::from_utf8(Command::new(program).args(arguments).arg(&file).output().unwrap().stdout).unwrap();
        assert_eq!(said("exiv2", &["-Pv", "-K", "Exif.Image.Orientation"]).trim(), "6");
        assert_eq!(said("exiv2", &["-Pv", "-K", "Xmp.dc.format"]).trim(), "image/jpeg");
        assert_eq!(said("vipsheader", &["-f", "orientation"]).trim(), "6");
        assert!(said("vipsheader", &["-a"]).contains(&format!("icc-profile-data: {} bytes", profile.len())));
        std::fs::remove_file(file).unwrap();
    }
}
