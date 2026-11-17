//! PNG reading and writing. Reading takes any PNG a program is likely to ship (every colour
//! type and depth, interlaced too), so an app can show pictures without another library.
//! Writing favours speed over size: a window screenshot comes to a few dozen kilobytes.

const START: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Encodes a picture as PNG. `width` is its width, `pixels` its pixels in rows from the top,
/// each `0xRRGGBB` (the alpha byte is ignored: see [`encode_rgba`] to keep it). It is as many
/// rows high as `pixels` has whole rows. Nothing for no pixels, or a size PNG cannot hold
/// (2,147,483,647 pixels).
pub fn encode(width: usize, pixels: &[u32]) -> Option<Vec<u8>> {
    let height = high(width, pixels)?;
    Some(file(width, height, [8, 2], &[], &rows::<3>(width, &pixels[..width * height])))
}
/// The same with alpha: each pixel `0xAARRGGBB`, not premultiplied (what [`decode`] returns and
/// [`Ui::add_picture`](crate::Ui::add_picture) takes).
pub fn encode_rgba(width: usize, pixels: &[u32]) -> Option<Vec<u8>> {
    let height = high(width, pixels)?;
    Some(file(width, height, [8, 6], &[], &rows::<4>(width, &pixels[..width * height])))
}

/// How many whole rows of `width` there are, where PNG can hold that size; none where not.
fn high(width: usize, pixels: &[u32]) -> Option<usize> {
    let (height, most) = (pixels.len() / width.max(1), i32::MAX as usize);
    (width > 0 && height > 0 && width <= most && height <= most).then_some(height)
}

/// A picture's rows as PNG stores them, `N` bytes per pixel: red, green, blue, and alpha as a
/// fourth.
fn rows<const N: usize>(width: usize, pixels: &[u32]) -> Vec<u8> {
    // Each row: filter type 2 (up), then each byte as its difference from the byte above. (The
    // fastest of the five filters both ways. Choosing per row takes several times as long and
    // was measured to make files no smaller.)
    let (stride, nothing) = (1 + width * N, vec![0; width]);
    let (mut raw, above) = (vec![0u8; pixels.len() / width.max(1) * stride], std::iter::once(&nothing[..]).chain(pixels.chunks_exact(width.max(1))));
    for ((stored, row), above) in raw.chunks_exact_mut(stride).zip(pixels.chunks_exact(width.max(1))).zip(above) {
        stored[0] = 2;
        for ((bytes, pixel), up) in stored[1..].as_chunks_mut::<N>().0.iter_mut().zip(row).zip(above) {
            let less = |part: u32| ((pixel >> part) as u8).wrapping_sub((up >> part) as u8);
            // (Each byte is set by name: copying a run is a call of its own in a size-optimized
            // build. With three bytes the last is set twice, the same both times.)
            (bytes[0], bytes[1], bytes[2], bytes[N - 1]) = (less(16), less(8), less(0), less([0, 24][N - 3]));
        }
    }
    raw
}

/// A PNG file around rows already laid out for its pixel type: `kind` is bit depth and colour
/// type.
fn file(width: usize, height: usize, kind: [u8; 2], palette: &[u8], raw: &[u8]) -> Vec<u8> {
    let header = [&(width as u32).to_be_bytes()[..], &(height as u32).to_be_bytes(), &[kind[0], kind[1], 0, 0, 0]].concat();
    let palette = if palette.is_empty() { Vec::new() } else { chunk(b"PLTE", palette) };
    [&START[..], &chunk(b"IHDR", &header), &palette, &chunk(b"IDAT", &deflate(raw)), &chunk(b"IEND", &[])].concat()
}

/// A PNG chunk: length, name, data, and a CRC over name and data, a byte at a time through a
/// table.
pub(crate) fn chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let checks: [u32; 256] = std::array::from_fn(|byte| (0..8).fold(byte as u32, |check, _| if check & 1 == 1 { check >> 1 ^ 0xEDB8_8320 } else { check >> 1 }));
    let check = name.iter().chain(body).fold(!0u32, |check, byte| checks[(check as u8 ^ byte) as usize] ^ check >> 8);
    [&(body.len() as u32).to_be_bytes()[..], name, body, &(!check).to_be_bytes()].concat()
}

/// How many symbols go into one deflate block, each block with its own codes.
const BLOCK: usize = 1 << 15;

/// Compresses any bytes as a zlib stream, as PNG does its rows: an installer's payload, a file
/// to keep small. `zlib_decompress` restores them.
pub fn zlib_compress(bytes: &[u8]) -> Vec<u8> {
    deflate(bytes)
}

/// Decompresses a zlib stream (from `zlib_compress`, or anything writing one). Nothing if
/// damaged, or if it inflates to more than `most` bytes: the caller gives a limit, since a few
/// bytes can inflate a thousandfold.
pub fn zlib_decompress(packed: &[u8], most: usize) -> Option<Vec<u8>> {
    inflate(packed, most)
}

/// Compresses bytes as a zlib stream. Where four bytes match four less than 32768 bytes back, a
/// back-reference replaces them and all matching after; other bytes are literals. Both are
/// Huffman-coded.
fn deflate(raw: &[u8]) -> Vec<u8> {
    let mut out = Packed { bytes: vec![0x78, 0x01], held: 0, count: 0 };
    // Where each four bytes were last seen (plus one; 0 for never), by a hash of the four.
    let (mut seen, mut symbols, mut at) = (vec![0u32; 1 << 15], Vec::with_capacity(BLOCK), 0);
    let place = |four: &[u8; 4]| (u32::from_le_bytes(*four).wrapping_mul(0x9E37_79B1) >> 17) as usize;
    while at < raw.len() {
        let (mut long, mut back) = (1, 0);
        if let Some(four) = raw[at..].first_chunk::<4>() {
            back = at + 1 - std::mem::replace(&mut seen[place(four)], at as u32 + 1) as usize;
            // (Never seen, too far back, or a hash collision?)
            if back <= at.min(32768) && raw[at - back..].first_chunk::<4>() == Some(four) {
                long = 4 + raw[at + 4..].iter().zip(&raw[at - back + 4..]).take(254).take_while(|(now, then)| now == then).count();
            }
        }
        // A symbol is a literal byte, or (above 255) a length and distance.
        symbols.push(if long == 1 { raw[at] as u32 } else { (long << 16 | back) as u32 });
        at += long;
        // The last four positions of a match are hashed too: what follows often matches again,
        // and distances one to four are the shortest to code.
        let noted = if long == 1 { 0 } else { 4 };
        for last in at - noted..at {
            if let Some(four) = raw[last..].first_chunk::<4>() {
                seen[place(four)] = last as u32 + 1;
            }
        }
        if symbols.len() == BLOCK {
            write_block(&mut out, &symbols, false);
            symbols.clear();
        }
    }
    write_block(&mut out, &symbols, true);
    // The remaining bits are flushed in whole bytes; then the Adler-32 checksum.
    out.bytes.extend_from_slice(&out.held.to_le_bytes()[..out.count.div_ceil(8) as usize]);
    out.bytes.extend(sum(raw).to_be_bytes());
    out.bytes
}

/// Bits on their way out, filling each byte from its low end.
struct Packed {
    bytes: Vec<u8>,
    held: u64,
    count: u32,
}

impl Packed {
    /// Writes the low `n` bits of `bits` (which has no others), 32 at most.
    #[inline(always)]
    fn put(&mut self, bits: u32, n: u32) {
        self.held |= (bits as u64) << self.count;
        self.count += n;
        if self.count >= 32 {
            self.bytes.extend_from_slice(&(self.held as u32).to_le_bytes());
            (self.held, self.count) = (self.held >> 32, self.count - 32);
        }
    }
    /// Writes a code: its bits, with its length from bit 16 up.
    #[inline(always)]
    fn code(&mut self, code: u32) {
        self.put(code & 0xffff, code >> 16);
    }
}

/// Writes one block: its dynamic codes by their lengths, then its symbols in them.
fn write_block(out: &mut Packed, symbols: &[u32], last: bool) {
    // The table entry covering a number: the last that does not start above it.
    let range_of = |table: &[u16], number: u32| table.partition_point(|from| *from as u32 <= number) - 1;
    // Symbol frequencies: literals, end of block and lengths in one alphabet; distances in
    // another.
    let (mut literals, mut distances) = ([0u32; 286], [0u32; 30]);
    literals[256] = 1;
    for symbol in symbols {
        let (long, back) = (symbol >> 16, symbol & 0xffff);
        if long == 0 {
            literals[back as usize] += 1;
            continue;
        }
        literals[257 + range_of(&LONG, long)] += 1;
        distances[range_of(&BACK, back)] += 1;
    }
    let lengths = [lengths_of(&literals, 15), lengths_of(&distances, 15)].concat();
    // The code lengths are coded in a third alphabet, where 17 and 18 stand for runs of zeros
    // (3 to 10, and 11 to 138): a symbol, then the count above the minimum.
    let (mut coded, mut at) = (Vec::new(), 0);
    while at < lengths.len() {
        let run = lengths[at..].iter().take_while(|length| **length == 0).count().min(138);
        coded.push(match run {
            11.. => (18, run - 11, 7),
            3.. => (17, run - 3, 3),
            _ => (lengths[at] as usize, 0, 0),
        });
        at += if run < 3 { 1 } else { run };
    }
    let mut third = [0u32; 19];
    coded.iter().for_each(|(symbol, _, _)| third[*symbol] += 1);
    let third = lengths_of(&third, 7);
    // BFINAL; BTYPE 2 (dynamic); HLIT, HDIST, HCLEN (above their minimums); then the
    // code-length code lengths in their own order.
    out.put(last as u32 | 2 << 1 | 29 << 3 | 29 << 8 | 15 << 13, 17);
    ORDER.iter().for_each(|symbol| out.put(third[*symbol] as u32, 3));
    let third = written(&third);
    for (symbol, more, bits) in coded {
        out.code(third[symbol]);
        out.put(more as u32, bits);
    }
    let (codes, back_codes) = (written(&lengths[..286]), written(&lengths[286..]));
    for symbol in symbols {
        let (long, back) = (symbol >> 16, symbol & 0xffff);
        if long == 0 {
            out.code(codes[back as usize]);
            continue;
        }
        let (so_many, so_far) = (range_of(&LONG, long), range_of(&BACK, back));
        out.code(codes[257 + so_many]);
        out.put(long - LONG[so_many] as u32, LONG_MORE[so_many] as u32);
        out.code(back_codes[so_far]);
        out.put(back - BACK[so_far] as u32, BACK_MORE[so_far] as u32);
    }
    out.code(codes[256]);
}

/// Code lengths for symbols with these frequencies: Huffman's, the commoner the shorter, none
/// longer than `longest`.
fn lengths_of(often: &[u32], longest: usize) -> Vec<u8> {
    // (An alphabet has at least two codes, even if nothing occurs: some decoders require it.)
    let mut often = often.to_vec();
    for symbol in 0..2 {
        often[symbol] += (often.iter().filter(|times| **times != 0).count() < 2 && often[symbol] == 0) as u32;
    }
    // The occurring symbols, rarest first. The two rarest nodes merge into one with their
    // summed weight until one is left; a symbol's code length is its depth. `weight` holds the
    // symbols then the merged nodes, `parent` each one's parent. Both runs are in ascending
    // order, so the rarest are at their fronts. (Ordered through a heap: for so few it is fast,
    // and much less code than a full sort.)
    let order = std::collections::BinaryHeap::from_iter((0..often.len()).filter(|symbol| often[*symbol] != 0).map(|symbol| (often[symbol], symbol))).into_sorted_vec();
    let (count, mut weight) = (order.len(), order.iter().map(|(times, _)| *times as u64).collect::<Vec<u64>>());
    let (mut parent, mut symbol, mut made) = (vec![0; 2 * count - 1], 0, count);
    while weight.len() < 2 * count - 1 {
        let mut both = 0;
        for _ in 0..2 {
            let is_symbol = symbol < count && (made == weight.len() || weight[symbol] <= weight[made]);
            let rarest = if is_symbol { symbol } else { made };
            (symbol, made) = (symbol + is_symbol as usize, made + !is_symbol as usize);
            (parent[rarest], both) = (weight.len(), both + weight[rarest]);
        }
        weight.push(both);
    }
    let mut deep = vec![0usize; weight.len()];
    (0..weight.len() - 1).rev().for_each(|at| deep[at] = deep[parent[at]] + 1);
    // How many codes of each length. Over-long ones are cut to the limit; that over-fills the
    // code space, so for each code too many, a longest code is paired with a shorter one made a
    // bit longer.
    let mut many = [0usize; 16];
    deep[..count].iter().for_each(|length| many[(*length).min(longest)] += 1);
    let mut over = (1..=longest).map(|length| many[length] << (longest - length)).sum::<usize>() - (1 << longest);
    while let Some(shorter) = (1..longest).rev().find(|length| over > 0 && many[*length] != 0) {
        many[longest] -= 1;
        many[shorter] -= 1;
        many[shorter + 1] += 2;
        over -= 1;
    }
    // The rarest symbols get the longest codes.
    let (mut lengths, mut rarest) = (vec![0u8; often.len()], order.iter());
    for length in (1..=longest).rev() {
        rarest.by_ref().take(many[length]).for_each(|(_, symbol)| lengths[*symbol] = length as u8);
    }
    lengths
}

/// The codes for these code lengths, as written: bits in output order (first lowest), length
/// from bit 16 up.
fn written(lengths: &[u8]) -> Vec<u32> {
    let mut next = first_codes(lengths).0;
    let code = |length: usize| {
        next[length] += 1;
        ((next[length] - 1) as u16).reverse_bits() as u32 >> (16 - length) | (length as u32) << 16
    };
    lengths.iter().map(|length| *length as usize & 15).map(code).collect()
}

/// The first code of each length, and how many there are, for an alphabet given by code lengths
/// (0: unused). Codes of a length count up from where the shorter ones end.
fn first_codes(lengths: &[u8]) -> ([u32; 16], [u32; 16]) {
    let (mut first, mut many) = ([0u32; 16], [0u32; 16]);
    lengths.iter().filter(|length| **length != 0).for_each(|length| many[*length as usize & 15] += 1);
    for length in 1..16 {
        first[length] = (first[length - 1] + many[length - 1]) << 1;
    }
    (first, many)
}

/// The order in which code-length code lengths are given.
const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// The Adler-32 checksum a zlib stream ends with: two sums, of the bytes and of the first sum
/// as it grows, each modulo 65521.
fn sum(bytes: &[u8]) -> u32 {
    const EVERY_OTHER: u64 = 0x00ff_00ff_00ff_00ff;
    let (mut low, mut high) = (1u32, 0u32);
    // (5552 bytes are the most that can be summed before a 32-bit sum could overflow.)
    for block in bytes.chunks(5552) {
        let (eights, rest) = block.as_chunks::<8>();
        // Eight bytes at a time as one number. Over those the second sum grows by eight times
        // the first, plus the first byte eight times, the next seven, and so on. Even and odd
        // bytes are separated, leaving room above each, so one multiplication sums four of them
        // in its top sixteen bits, weighted or not.
        for eight in eights {
            let all = u64::from_le_bytes(*eight);
            let (even, odd) = (all & EVERY_OTHER, all >> 8 & EVERY_OTHER);
            let weighted = (even.wrapping_mul(0x0008_0006_0004_0002) >> 48) + (odd.wrapping_mul(0x0007_0005_0003_0001) >> 48);
            high += 8 * low + weighted as u32;
            low += ((even + odd).wrapping_mul(0x0001_0001_0001_0001) >> 48) as u32;
        }
        for byte in rest {
            low += *byte as u32;
            high += low;
        }
        (low, high) = (low % 65521, high % 65521);
    }
    high << 16 | low
}

/// Decodes a PNG file: the picture's width, and its pixels in rows from the top, each
/// `0xAARRGGBB`, as [`Ui::add_picture`](crate::Ui::add_picture) takes them. Turned upright per
/// `eXIf`, and colour-managed per the file's colour chunks (see `colours_of`). Of an APNG, its
/// default image. Nothing for a damaged file or no PNG.
pub fn decode(png: &[u8]) -> Option<(usize, Vec<u32>)> {
    let (chunks, whole) = chunks(png)?;
    if !whole {
        return None;
    }
    let (mut notes, mut packed) = (None, Vec::with_capacity(png.len()));
    for (name, body) in chunks.iter().copied() {
        match name {
            b"IDAT" => packed.extend_from_slice(body),
            b"eXIf" => notes = Some(body),
            _ => {}
        }
    }
    let (size, stored) = Stored::of(&chunks)?;
    let pixels = stored.frame(size, &packed)?;
    Some(crate::exif_orientation::turned(notes.and_then(crate::exif_orientation::turn), size[0], pixels))
}

/// A PNG chunk as read: its four-letter name and its data.
type Chunk<'a> = (&'a [u8], &'a [u8]);

/// A PNG file's chunks, each as name and data, up to the end or the first truncated one; and
/// whether none was. Nothing for a file without the PNG signature.
fn chunks(png: &[u8]) -> Option<(Vec<Chunk<'_>>, bool)> {
    // The file is a list of chunks: length, name, data, CRC. (A few trailing bytes, too few for
    // a chunk, end it.)
    let (mut rest, mut chunks) = (png.strip_prefix(&START)?, Vec::new());
    while rest.len() >= 12 {
        let long = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        if long > rest.len() - 12 {
            return Some((chunks, false));
        }
        chunks.push((&rest[4..8], &rest[8..8 + long]));
        rest = &rest[12 + long..];
    }
    Some((chunks, true))
}

/// The data of the first chunk of a name; empty where none.
fn first<'a>(chunks: &[(&[u8], &'a [u8])], called: &[u8]) -> &'a [u8] {
    chunks.iter().find(|(name, _)| *name == called).map_or(&[], |(_, body)| body)
}

/// How a PNG's pixels are stored, per its header: bit depth, colour type, and interlacing; the
/// palette with alpha; the one colour it may mark transparent (`tRNS`, two bytes per sample);
/// and its colour conversion for the screen, if any.
pub(crate) struct Stored {
    depth: usize,
    kind: u8,
    passes: bool,
    colours: [u32; 256],
    named: Vec<u8>,
    change: Option<crate::color_profiles::Change>,
}

impl Stored {
    /// Reads a file's header, palette, `tRNS` and colour chunks: the picture's size and how it
    /// is stored. Nothing for a colour type that does not come in that depth, or no size.
    fn of(chunks: &[(&[u8], &[u8])]) -> Option<([usize; 2], Stored)> {
        let (head, palette, clear): (&[u8; 13], _, _) = (first(chunks, b"IHDR").try_into().ok()?, first(chunks, b"PLTE"), first(chunks, b"tRNS"));
        let (width, height) = (u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize, u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize);
        let (depth, kind) = (head[8] as usize, head[9]);
        // Not every colour type comes in every depth.
        if !matches!((kind, depth), (0, 1 | 2 | 4 | 8 | 16) | (3, 1 | 2 | 4 | 8) | (2 | 4 | 6, 8 | 16)) || head[12] > 1 || width == 0 {
            return None;
        }
        // The palette's colours with alpha, as pixels.
        let colours: [u32; 256] = std::array::from_fn(|n| {
            let part = |k: usize| palette.get(n * 3 + k).map_or(0, |part| *part as u32);
            clear.get(n).map_or(255, |solid| *solid as u32) << 24 | part(0) << 16 | part(1) << 8 | part(2)
        });
        // A picture without palette or alpha channel can mark one colour transparent wherever
        // it stands: its samples, two bytes each.
        let named = if (kind == 0 && clear.len() == 2) || (kind == 2 && clear.len() == 6) { clear.to_vec() } else { Vec::new() };
        Some(([width, height], Stored { depth, kind, passes: head[12] == 1, colours, named, change: colours_of(chunks) }))
    }

    /// The pixels of a `size` picture (or an APNG frame), stored as this says, from its
    /// `packed` rows, in screen colours. Nothing if damaged, or the rows come to more or fewer.
    pub(crate) fn frame(&self, size: [usize; 2], packed: &[u8]) -> Option<Vec<u32>> {
        let mut pixels = pixels(self, size, packed)?;
        if let Some(change) = &self.change {
            change.apply(&mut pixels);
        }
        Some(pixels)
    }
}

/// A picture's pixels from its packed rows: as `Stored::frame`, but in the file's colours.
fn pixels(stored: &Stored, [width, height]: [usize; 2], packed: &[u8]) -> Option<Vec<u32>> {
    let (depth, kind) = (stored.depth, stored.kind);
    // Samples per pixel: grey, RGB, palette index, and each of the first two with alpha.
    let values = [1, 0, 3, 1, 2, 0, 4][kind as usize];
    let passes = if stored.passes { &PASSES[1..] } else { &PASSES[..1] };
    // A pass's pixels per row, rows, and bytes per row. (In a small picture a pass can be
    // empty: then it is absent.)
    let size = |[left, top, across, down]: [usize; 4]| {
        let (wide, high) = (width.saturating_sub(left).div_ceil(across), height.saturating_sub(top).div_ceil(down));
        [wide, if wide == 0 { 0 } else { high }, wide.saturating_mul(values * depth).div_ceil(8)]
    };
    // (A file whose rows come to less or more than it says is no picture: it gets no memory for
    // what it only claims.)
    let needed = passes.iter().map(|pass| size(*pass)).fold(0usize, |sum, [_, high, row_bytes]| sum.saturating_add(high.saturating_mul(row_bytes + 1)));
    let mut raw = inflate(packed, needed)?;
    if raw.len() < needed {
        return None;
    }
    // A zero row above each pass's first row; room for a row's samples as bytes where stored
    // otherwise; and for one pass row's pixels.
    let (nothing, mut eight, mut sparse) = (vec![0u8; size(passes[passes.len() - 1])[2]], vec![0u8; width * values], vec![0u32; width]);
    let (mut pixels, mut rest, step) = (vec![0u32; width * height], &mut raw[..], (values * depth).div_ceil(8));
    for &[left, top, across, down] in passes {
        let [wide, high, row_bytes] = size([left, top, across, down]);
        let (rows, after) = rest.split_at_mut(high * (row_bytes + 1));
        let mut above = &nothing[..row_bytes];
        for (y, row) in rows.chunks_exact_mut(row_bytes + 1).enumerate() {
            let (rule, row) = row.split_first_mut()?;
            unfilter(*rule, row, above, step)?;
            let (bytes, line) = (in_bytes(row, depth, kind, &mut eight[..wide * values]), &mut pixels[(top + y * down) * width..][..width]);
            // (A pass that skips no pixel of its rows fills them in place.)
            if across == 1 {
                paint(kind, bytes, line, &stored.colours);
                clear_named(row, depth, &stored.named, line);
            } else {
                paint(kind, bytes, &mut sparse[..wide], &stored.colours);
                clear_named(row, depth, &stored.named, &mut sparse[..wide]);
                line[left..].iter_mut().step_by(across).zip(&sparse).for_each(|(pixel, read)| *pixel = *read);
            }
            above = row;
        }
        rest = after;
    }
    Some(pixels)
}

/// How a picture's colours convert for the screen, per its chunks: not at all where they say
/// sRGB (`sRGB`) or nothing; else per its embedded profile (`iCCP`, zlib-compressed), or its
/// gamma (`gAMA`) and primaries (`cHRM`).
fn colours_of(chunks: &[(&[u8], &[u8])]) -> Option<crate::color_profiles::Change> {
    if chunks.iter().any(|(name, _)| *name == b"sRGB") {
        return None;
    }
    // (A profile chunk: a name, a NUL, a compression byte, and the profile.)
    let profile = first(chunks, b"iCCP");
    let packed = profile.iter().position(|byte| *byte == 0).and_then(|end| profile.get(end + 2..));
    if let Some(profile) = packed.and_then(|packed| inflate(packed, crate::color_profiles::LONGEST)) {
        return crate::color_profiles::Change::of_profile(&profile);
    }
    let number = |body: &[u8], at: usize| body.get(at..at + 4).map(|four| u32::from_be_bytes([four[0], four[1], four[2], four[3]]) as f32 / 100_000.0);
    let gamma = number(first(chunks, b"gAMA"), 0).filter(|gamma| *gamma > 0.0);
    // (The chunk lists white first, then red, green and blue.)
    let chroma: Option<Vec<f32>> = [2, 3, 4, 5, 6, 7, 0, 1].iter().map(|at| number(first(chunks, b"cHRM"), at * 4)).collect();
    crate::color_profiles::Change::of_light(gamma, chroma.and_then(|chroma| chroma.try_into().ok()))
}

/// Reads an APNG as an animation: its first `most` frames at most. A still PNG is a one-frame
/// animation. Nothing if no PNG, or no frame is whole. (It ignores `eXIf` orientation.)
///
/// An APNG is a PNG with extra frames: `acTL` marks it animated with its loop count, an `fcTL`
/// before each frame gives its region and how it is shown, and frame data goes in `fdAT`
/// chunks, each with a sequence number first. The default image is the first frame where an
/// `fcTL` precedes it, else no frame.
pub(crate) fn read_animation(png: &[u8], most: usize) -> Option<crate::animation::Reel> {
    use crate::animation::{After, Frame, Kind, Laid, Reel};
    let (chunks, _) = chunks(png)?;
    let (size, stored) = Stored::of(&chunks)?;
    // (Every PNG holds its default image, canvas-sized, frame or not: a canvas larger than its
    // packed rows could inflate to gets no memory. Deflate inflates at most 1,032 times, and a
    // byte holds at most eight pixels.)
    let held = chunks.iter().filter(|(name, _)| *name == b"IDAT").map(|(_, body)| body.len()).sum::<usize>();
    if size[0].saturating_mul(size[1]) > held.saturating_mul(1032 * 8) {
        return None;
    }
    let moves = chunks.iter().any(|(name, _)| *name == b"acTL");
    // Where each frame's rows start in the data kept, with its `fcTL`.
    let (mut starts, mut packed): (Vec<(&[u8], usize)>, Vec<u8>) = (Vec::new(), Vec::new());
    for (name, body) in chunks.iter().copied() {
        match name {
            b"fcTL" if moves => starts.push((body, packed.len())),
            b"IDAT" if !moves && starts.is_empty() => starts.push((&[], 0)),
            b"fdAT" if moves && !starts.is_empty() => packed.extend_from_slice(body.get(4..).unwrap_or(&[])),
            _ => {}
        }
        // (The default image's rows, where they are the first frame's.)
        if name == b"IDAT" && starts.len() == 1 {
            packed.extend_from_slice(body);
        }
    }
    // (Once, so many times, or forever where it says 0.)
    let plays = first(&chunks, b"acTL").get(4..8).map_or(Some(1), |plays| Some(u32::from_be_bytes([plays[0], plays[1], plays[2], plays[3]]) as u64).filter(|plays| *plays > 0));
    let ends: Vec<usize> = starts.iter().skip(1).map(|(_, start)| *start).chain([packed.len()]).collect();
    let mut frames = Vec::new();
    for ((control, start), end) in starts.into_iter().zip(ends).take(most) {
        let still = Frame::new([0, 0, size[0], size[1]], [start, end], 0, After::Stays, Laid::Instead);
        let Some(frame) = (if moves { frame_of(control, size, [start, end]) } else { Some(still) }) else { break };
        // (Each frame is decoded once, to check it is whole.)
        if stored.frame([frame.place[2], frame.place[3]], &packed[start..end]).is_none() {
            break;
        }
        frames.push(frame);
    }
    let mut reel = Reel::new(size, Kind::Png(Box::new(stored)), plays);
    (reel.frames, reel.packed) = (frames, packed);
    reel.done()
}

/// An APNG frame per its `fcTL`, on a `size` canvas: its region, duration (a fraction of a
/// second; a 0 denominator means a hundredth), dispose and blend ops. Nothing for a frame off
/// the canvas, or an op that does not exist.
fn frame_of(control: &[u8], size: [usize; 2], packed: [usize; 2]) -> Option<crate::animation::Frame> {
    use crate::animation::{After, Frame, Laid};
    let said = control.first_chunk::<26>()?;
    let long = |at: usize| u32::from_be_bytes([said[at], said[at + 1], said[at + 2], said[at + 3]]) as usize;
    let place = [long(12), long(16), long(4), long(8)];
    if place[2] == 0 || place[3] == 0 || place[0].saturating_add(place[2]) > size[0] || place[1].saturating_add(place[3]) > size[1] {
        return None;
    }
    let (part, whole) = (u16::from_be_bytes([said[20], said[21]]) as u64, u16::from_be_bytes([said[22], said[23]]) as u64);
    let after = *[After::Stays, After::Cleared, After::PutBack].get(said[24] as usize)?;
    let laid = *[Laid::Instead, Laid::OverPng].get(said[25] as usize)?;
    Some(Frame::new(place, packed, part * 1000 / if whole == 0 { 100 } else { whole }, after, laid))
}

/// Which pixels a pass holds: the first one's column and row, and the step across and down. The
/// first entry is for a non-interlaced picture; the seven after are Adam7's passes, each a
/// finer grid of what the ones before skipped, so a slowly arriving picture can show roughly at
/// once.
const PASSES: [[usize; 4]; 8] = [[0, 0, 1, 1], [0, 0, 8, 8], [4, 0, 8, 8], [0, 4, 4, 8], [2, 0, 4, 4], [0, 2, 2, 4], [1, 0, 2, 2], [0, 1, 1, 2]];

/// A row's samples, a byte each. Eight-bit samples already are. Of sixteen, the high byte
/// counts. Fewer bits are spread into `spread`: grey scaled so the highest value is 255, a
/// palette index as is.
fn in_bytes<'a>(row: &'a [u8], depth: usize, kind: u8, spread: &'a mut [u8]) -> &'a [u8] {
    let (most, times) = (255u8 >> (8 - depth.min(8)), if kind == 3 { 1 } else { [0, 255, 85, 0, 17][depth.min(4)] });
    match depth {
        8 => return row,
        16 => {
            for (value, wide) in spread.iter_mut().zip(row.as_chunks::<2>().0) {
                *value = wide[0];
            }
        }
        _ => {
            for (n, value) in spread.iter_mut().enumerate() {
                *value = (row[n * depth / 8] >> (8 - depth - n * depth % 8) & most) * times;
            }
        }
    }
    spread
}

/// Makes transparent the pixels of a row exactly matching the `tRNS` colour. The colour is in
/// the picture's own depth and compared with the stored samples, not the bytes they became: at
/// sixteen bits two colours can share their high bytes. (Rare: most pictures name no such
/// colour.)
fn clear_named(row: &[u8], depth: usize, named: &[u8], pixels: &mut [u32]) {
    let values = named.len() / 2;
    let stored = |n: usize| match depth {
        16 => u16::from_be_bytes([row[n * 2], row[n * 2 + 1]]),
        8 => row[n] as u16,
        _ => (row[n * depth / 8] >> (8 - depth - n * depth % 8) & 255 >> (8 - depth)) as u16,
    };
    let same = |x: usize| (0..values).all(|value| stored(x * values + value) == u16::from_be_bytes([named[value * 2], named[value * 2 + 1]]));
    for (x, pixel) in pixels.iter_mut().enumerate().filter(|_| values > 0) {
        *pixel &= if same(x) { 0x00ff_ffff } else { !0 };
    }
}

/// Turns a row's byte samples into `0xAARRGGBB` pixels: a loop per colour type.
fn paint(kind: u8, row: &[u8], pixels: &mut [u32], colours: &[u32; 256]) {
    match kind {
        0 => {
            for (pixel, grey) in pixels.iter_mut().zip(row) {
                *pixel = 0xff00_0000 | (*grey as u32 * 0x01_0101);
            }
        }
        2 => {
            for (pixel, [red, green, blue]) in pixels.iter_mut().zip(row.as_chunks::<3>().0) {
                *pixel = u32::from_be_bytes([255, *red, *green, *blue]);
            }
        }
        3 => {
            for (pixel, number) in pixels.iter_mut().zip(row) {
                *pixel = colours[*number as usize];
            }
        }
        4 => {
            for (pixel, [grey, solid]) in pixels.iter_mut().zip(row.as_chunks::<2>().0) {
                *pixel = ((*solid as u32) << 24) | (*grey as u32 * 0x01_0101);
            }
        }
        _ => {
            for (pixel, [red, green, blue, solid]) in pixels.iter_mut().zip(row.as_chunks::<4>().0) {
                *pixel = u32::from_be_bytes([*solid, *red, *green, *blue]);
            }
        }
    }
}

/// Unfilters one row. Each PNG row is stored as its difference from a filter's prediction from
/// the pixel before (`step` bytes back), the row above, or both; `rule` says which. Nothing for
/// an unknown filter.
fn unfilter(rule: u8, row: &mut [u8], above: &[u8], step: usize) -> Option<()> {
    match (rule, step) {
        (0, _) => {}
        (2, _) => add(row, above),
        (1 | 3 | 4, 1) => by_pixels::<1>(rule, row, above),
        (1 | 3 | 4, 2) => by_pixels::<2>(rule, row, above),
        (1 | 3 | 4, 3) => by_pixels::<3>(rule, row, above),
        (1 | 3 | 4, 4) => by_pixels::<4>(rule, row, above),
        (1 | 3 | 4, 6) => by_pixels::<6>(rule, row, above),
        (1 | 3 | 4, _) => by_pixels::<8>(rule, row, above),
        _ => return None,
    }
    Some(())
}

/// The Up filter: each byte stored as its difference from the one above. Eight bytes are added
/// at once as one number; each byte's top bit is added separately so no byte carries into the
/// next.
fn add(row: &mut [u8], above: &[u8]) {
    const TOPS: u64 = 0x8080_8080_8080_8080;
    let ((eights, rest), (ups, last)) = (row.as_chunks_mut::<8>(), above.as_chunks::<8>());
    for (eight, up) in eights.iter_mut().zip(ups) {
        let (a, b) = (u64::from_le_bytes(*eight), u64::from_le_bytes(*up));
        *eight = (((a & !TOPS) + (b & !TOPS)) ^ ((a ^ b) & TOPS)).to_le_bytes();
    }
    for (byte, up) in rest.iter_mut().zip(last) {
        *byte = byte.wrapping_add(*up);
    }
}

/// The filters using the pixel before, for `N`-byte pixels: Sub, Average (of it and the one
/// above), and Paeth.
fn by_pixels<const N: usize>(rule: u8, row: &mut [u8], above: &[u8]) {
    match rule {
        1 => each_pixel::<N>(row, above, |before, _, _| before),
        3 => each_pixel::<N>(row, above, |before, up, _| (before + up) / 2),
        // SAFETY: every x86-64 processor has SSE2.
        #[cfg(target_arch = "x86_64")]
        _ if N == 3 || N == 4 => unsafe { nearest_at_once::<N>(row, above) },
        _ => each_pixel::<N>(row, above, nearest),
    }
}

/// Adds to every byte of a row what `rule` makes of the bytes before, above, and above-before.
/// Built once per filter and pixel size, so the pixel before stays in registers and is never
/// reloaded.
#[inline(always)]
fn each_pixel<const N: usize>(row: &mut [u8], above: &[u8], rule: impl Fn(i32, i32, i32) -> i32 + Copy) {
    let (pixels, ups, mut before) = (row.as_chunks_mut::<N>().0, above.as_chunks::<N>().0, [0; N]);
    // (The first pixel has none before it, nor above-before.)
    let Some(((first, rest), up)) = pixels.split_first_mut().zip(ups.first()) else { return };
    one_pixel(first, &mut before, [up, &[0; N]], rule);
    for ((pixel, up), slant) in rest.iter_mut().zip(&ups[1..]).zip(ups) {
        one_pixel(pixel, &mut before, [up, slant], rule);
    }
}

/// One pixel of `each_pixel`. Its bytes are unrolled by hand: the compiler does not always
/// unroll a loop this short, and then nothing stays in registers.
#[inline(always)]
fn one_pixel<const N: usize>(pixel: &mut [u8; N], before: &mut [i32; N], above: [&[u8; N]; 2], rule: impl Fn(i32, i32, i32) -> i32 + Copy) {
    one_byte(0, pixel, before, above, rule);
    one_byte(1, pixel, before, above, rule);
    one_byte(2, pixel, before, above, rule);
    one_byte(3, pixel, before, above, rule);
    one_byte(4, pixel, before, above, rule);
    one_byte(5, pixel, before, above, rule);
    one_byte(6, pixel, before, above, rule);
    one_byte(7, pixel, before, above, rule);
}

/// Byte `n` of a pixel, if it has that many. (Computed as 32-bit numbers: the sums need the
/// room, and registers hold nothing narrower without extra work.)
#[inline(always)]
fn one_byte<const N: usize>(n: usize, pixel: &mut [u8; N], before: &mut [i32; N], [up, slant]: [&[u8; N]; 2], rule: impl Fn(i32, i32, i32) -> i32) {
    if n < N {
        before[n] = (pixel[n] as i32 + rule(before[n], up[n] as i32, slant[n] as i32)) & 255;
        pixel[n] = before[n] as u8;
    }
}

/// Paeth's predictor: of before, above and above-before, the one nearest "before + above -
/// above-before"; ties go to the earlier. Computed without the three distances: above-before is
/// nearest only when it lies between the other two, and `edge` says on which side of their
/// middle, and how far.
#[inline(always)]
fn nearest(before: i32, up: i32, slant: i32) -> i32 {
    let (low, high, edge) = (before.min(up), before.max(up), slant * 3 - before - up);
    let other = if high <= edge { low } else { slant };
    if edge <= low { high } else { other }
}

/// Paeth for three- or four-byte pixels, a whole pixel at once: each byte in a 16-bit lane of
/// one SSE2 register, which unfilters photos (mostly Paeth-filtered) in about half the time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
fn nearest_at_once<const N: usize>(row: &mut [u8], above: &[u8]) {
    use std::arch::x86_64::*;
    // A pixel's bytes, each in its own lane. (Bytes are moved one by one: copying a run is a
    // call of its own in a size-optimized build. With three bytes the last also fills the
    // fourth lane, which is computed and stored twice: the same both times.)
    let lanes = |pixel: &[u8; N]| _mm_unpacklo_epi8(_mm_cvtsi32_si128(i32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[N - 1]])), _mm_setzero_si128());
    let size = |lanes| _mm_max_epi16(lanes, _mm_sub_epi16(_mm_setzero_si128(), lanes));
    let pick = |mask, yes, no| _mm_or_si128(_mm_and_si128(mask, yes), _mm_andnot_si128(mask, no));
    // The pixel before, and above-before: zero at a row's start.
    let (mut before, mut slant) = (_mm_setzero_si128(), _mm_setzero_si128());
    for (pixel, up) in row.as_chunks_mut::<N>().0.iter_mut().zip(above.as_chunks::<N>().0) {
        let up = lanes(up);
        // Each one's distance from "before + above - above-before".
        let (from_before, from_up) = (_mm_sub_epi16(up, slant), _mm_sub_epi16(before, slant));
        let (far_before, far_up, far_slant) = (size(from_before), size(from_up), size(_mm_add_epi16(from_before, from_up)));
        let least = _mm_min_epi16(far_slant, _mm_min_epi16(far_before, far_up));
        let nearest = pick(_mm_cmpeq_epi16(least, far_before), before, pick(_mm_cmpeq_epi16(least, far_up), up, slant));
        before = _mm_and_si128(_mm_add_epi16(lanes(pixel), nearest), _mm_set1_epi16(255));
        slant = up;
        let now = _mm_cvtsi128_si32(_mm_packus_epi16(before, before)).to_le_bytes();
        (pixel[0], pixel[1], pixel[2], pixel[N - 1]) = (now[0], now[1], now[2], now[N - 1]);
    }
}

/// A bit reader over compressed bytes, from the low end of each byte. Up to 64 bits are
/// buffered, refilled eight bytes at a time.
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// The first byte not yet buffered.
    at: usize,
    held: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    /// The bits of `data`, from the first.
    pub(crate) fn of(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0, held: 0, count: 0 }
    }
    /// Makes sure 56 or more bits are buffered: enough for several codes in a row, and for
    /// everything after a length code (5 extra length bits, a distance code of up to 15, and 13
    /// extra). Nothing once more bits were consumed than the data has.
    #[inline(always)]
    pub(crate) fn fill(&mut self) -> Option<()> {
        let Some(next) = self.data.get(self.at..self.at + 8) else { return self.fill_at_the_end() };
        self.held |= u64::from_le_bytes(next.try_into().ok()?) << self.count;
        // (Whole bytes only: the bits of the last one that did not fit come again.)
        self.at += (63 - self.count as usize) / 8;
        self.count |= 56;
        Some(())
    }
    /// The same within eight bytes of the end, a byte at a time. Past the end zeros are
    /// buffered, so readers need not check; consuming them is noticed here on the next call.
    #[cold]
    fn fill_at_the_end(&mut self) -> Option<()> {
        if self.taken() > self.data.len() * 8 {
            return None;
        }
        while self.count <= 56 {
            self.held |= (self.data.get(self.at).copied().unwrap_or(0) as u64) << self.count;
            (self.at, self.count) = (self.at + 1, self.count + 8);
        }
        Some(())
    }
    /// Bits consumed so far.
    fn taken(&self) -> usize {
        self.at * 8 - self.count as usize
    }
    /// The next `n` bits, without consuming them. (`fill` first.)
    #[inline(always)]
    pub(crate) fn peek(&self, n: u32) -> usize {
        (self.held & ((1 << n) - 1)) as usize
    }
    #[inline(always)]
    pub(crate) fn skip(&mut self, n: u32) {
        (self.held, self.count) = (self.held >> n, self.count - n);
    }
    #[inline(always)]
    pub(crate) fn take(&mut self, n: u32) -> usize {
        let bits = self.peek(n);
        self.skip(n);
        bits
    }
}

/// A code table entry. Low four bits: the code's length. The next four: extra bits that follow
/// it. From bit 16 up, what it stands for, one of: a literal, end of block, a base length the
/// extra bits are added to, or where a subtable starts.
const BYTE: u32 = 1 << 8;
const END: u32 = 1 << 9;
const COUNT: u32 = 1 << 10;
const FURTHER: u32 = 1 << 11;

/// A Huffman decoding table: indexed by the next `root` bits, an entry gives the symbol and how
/// many of the bits are its code. Longer codes go to a subtable the entry for their first bits
/// points to.
pub(crate) struct Codes {
    table: Vec<u32>,
    root: u32,
}

impl Codes {
    /// Reads one code, returning its entry. (An entry standing for nothing: the bits are no
    /// code.)
    #[inline(always)]
    pub(crate) fn read(&self, bits: &mut Bits) -> u32 {
        let mut entry = self.table.get(bits.peek(self.root)).copied().unwrap_or(0);
        if entry & FURTHER != 0 {
            bits.skip(self.root);
            entry = self.table.get((entry >> 16) as usize + bits.peek(entry >> 4 & 15)).copied().unwrap_or(0);
        }
        bits.skip(entry & 15);
        entry
    }
}

/// The table for a code given by each symbol's code length (0: unused). `means` gives a
/// symbol's entry, without the length. Nothing if the lengths over-fill the code space.
pub(crate) fn codes(lengths: &[u8], root: u32, means: fn(usize) -> u32) -> Option<Codes> {
    let (mut next, many) = first_codes(lengths);
    if next[15] + many[15] > 1 << 15 {
        return None;
    }
    let longest = (1..16).rev().find(|length| many[*length] != 0).unwrap_or(1) as u32;
    let (root, further) = (root.min(longest), longest.saturating_sub(root));
    let mut table = vec![0u32; 1 << root];
    for (symbol, length) in lengths.iter().enumerate().filter(|(_, length)| **length != 0) {
        let length = *length as u32 & 15;
        // The code as its bits arrive, first lowest. Every table slot whose low bits are the
        // code gets its entry.
        let mut code = (next[length as usize] as u16).reverse_bits() as usize >> (16 - length);
        next[length as usize] += 1;
        let (mut from, mut to, mut own) = (0, 1 << root, length);
        if length > root {
            let first = code & ((1 << root) - 1);
            if table[first] == 0 {
                table[first] = FURTHER | (table.len() as u32) << 16 | further << 4;
                table.resize(table.len() + (1 << further), 0);
            }
            (from, code, own) = ((table[first] >> 16) as usize, code >> root, length - root);
            to = from + (1 << further);
        }
        (from + code..to).step_by(1 << own).for_each(|at| table[at] = means(symbol) | own);
    }
    Some(Codes { table, root })
}

/// The base length of each length code (symbols 257 on), and how many extra bits follow it.
const LONG: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LONG_MORE: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// The same for distance codes.
const BACK: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const BACK_MORE: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// What a literal/length symbol means: a literal, end of block, or a length.
fn byte_or_long(symbol: usize) -> u32 {
    match symbol {
        0..256 => BYTE | (symbol as u32) << 16,
        256 => END,
        257..286 => COUNT | (LONG[symbol - 257] as u32) << 16 | (LONG_MORE[symbol - 257] as u32) << 4,
        _ => 0,
    }
}

/// What a distance symbol means.
fn back(symbol: usize) -> u32 {
    if symbol < 30 { COUNT | (BACK[symbol] as u32) << 16 | (BACK_MORE[symbol] as u32) << 4 } else { 0 }
}

/// The first-level index bits of the literal/length and distance tables. Short codes are the
/// common ones, so most resolve at the first lookup.
const ROOTS: [u32; 2] = [11, 8];

/// How far `block` may write past its position before checking room again: the bytes one bit
/// refill can hold as literals, the longest match, and the fifteen bytes a 16-byte copy can
/// overshoot by.
const SPARE: usize = 64 + 258 + 16;

/// The output buffer's starting size as a multiple of the compressed size, and its growth
/// factor when full, up to the limit. Fonts and photos inflate two to five times, window
/// screenshots dozens: those read up to a seventh slower for it (a quarter millisecond for
/// 900×600) than with room for 64 times at once. Allocating everything a file claims would
/// allocate just as much for one that only claims it.
const ROOM: usize = 4;

/// Decompresses a zlib stream, as PNG stores its rows (and WOFF its tables). Nothing if damaged
/// (the stream ends with a checksum of all output), or if it inflates past `most`, which the
/// caller knows beforehand: a few bytes can inflate a thousandfold, and no file gets memory for
/// more than it claims, nor much more than it holds.
pub(crate) fn inflate(packed: &[u8], most: usize) -> Option<Vec<u8>> {
    // (Two header bytes, and four checksum bytes after the data.)
    let (out, long) = deflated(packed.get(2..)?, most)?;
    let check = packed.get(2 + long..)?.first_chunk::<4>()?;
    (u32::from_be_bytes(*check) == sum(&out)).then_some(out)
}

/// Inflates raw deflate data, as in a zlib stream or a gzip file: the output, and how many
/// input bytes it took (to the end of the byte holding the last bit). Nothing if damaged or
/// past `most`.
pub(crate) fn deflated(packed: &[u8], most: usize) -> Option<(Vec<u8>, usize)> {
    let mut bits = Bits { data: packed, at: 0, held: 0, count: 0 };
    // The output, with spare room after it; `at` is how far it has come. It starts at a few
    // times the input and grows as needed (see `ROOM`): never more than a few times the input
    // or the output.
    let (mut out, mut at) = (vec![0; most.min(packed.len().saturating_mul(ROOM)) + SPARE], 0);
    loop {
        bits.fill()?;
        let (last, kind) = (bits.take(1), bits.take(2));
        (bits, at) = match kind {
            0 => stored(bits, &mut out, at, most)?,
            1 => block(bits, &mut out, at, most, &usual()?)?,
            2 => {
                let codes = own(&mut bits)?;
                block(bits, &mut out, at, most, &codes)?
            }
            _ => return None,
        };
        if at > most {
            return None;
        }
        if last == 1 {
            break;
        }
    }
    // (What follows starts at a byte boundary. Having consumed more than there is means the
    // input was truncated.)
    let long = bits.taken().div_ceil(8);
    // (The result keeps no spare room.)
    out.truncate(at);
    out.shrink_to_fit();
    (long <= packed.len()).then_some((out, long))
}

/// A stored block: its length, the same inverted, then its raw bytes.
fn stored<'a>(bits: Bits<'a>, out: &mut Vec<u8>, at: usize, most: usize) -> Option<(Bits<'a>, usize)> {
    // (It starts at a byte boundary: buffered bytes past that one are given back.)
    let from = bits.taken().div_ceil(8);
    let head = bits.data.get(from..)?.first_chunk::<4>()?;
    let long = u16::from_le_bytes([head[0], head[1]]) as usize;
    let bytes = bits.data.get(from + 4..from + 4 + long)?;
    if long != !u16::from_le_bytes([head[2], head[3]]) as usize || at + long > most {
        return None;
    }
    if out.len() < at + long + SPARE {
        out.resize(at + long + SPARE, 0);
    }
    out[at..at + long].copy_from_slice(bytes);
    Some((Bits { data: bits.data, at: from + 4 + long, held: 0, count: 0 }, at + long))
}

/// The fixed codes, for a block without its own.
fn usual() -> Option<[Codes; 2]> {
    let lengths: [u8; 288] = std::array::from_fn(|symbol| [8, 9, 7, 8][(symbol > 143) as usize + (symbol > 255) as usize + (symbol > 279) as usize]);
    Some([codes(&lengths, ROOTS[0], byte_or_long)?, codes(&[5; 30], ROOTS[1], back)?])
}

/// A dynamic block's codes: their lengths first, coded with a third code where three symbols
/// mean "repeat" and "so many zeros".
fn own(bits: &mut Bits) -> Option<[Codes; 2]> {
    bits.fill()?;
    let (literals, distances, told) = (bits.take(5) + 257, bits.take(5) + 1, bits.take(4) + 4);
    let mut first = [0u8; 19];
    for at in &ORDER[..told] {
        bits.fill()?;
        first[*at] = bits.take(3) as u8;
    }
    let first = codes(&first, 7, |symbol| BYTE | (symbol as u32) << 16)?;
    let (mut lengths, mut at) = ([0u8; 320], 0);
    while at < literals + distances {
        bits.fill()?;
        let entry = first.read(bits);
        let (length, times) = match entry >> 16 {
            _ if entry & BYTE == 0 => return None,
            16 => (*lengths.get(at.wrapping_sub(1))?, 3 + bits.take(2)),
            17 => (0, 3 + bits.take(3)),
            18 => (0, 11 + bits.take(7)),
            length => (length as u8, 1),
        };
        lengths[..literals + distances].get_mut(at..at + times)?.fill(length);
        at += times;
    }
    Some([codes(&lengths[..literals], ROOTS[0], byte_or_long)?, codes(&lengths[literals..at], ROOTS[1], back)?])
}

/// The bytes of one compressed block: each code is a literal, end of block, or a length and
/// distance. (Bits and position are passed and returned by value, not by reference, so they
/// stay in registers through the block.)
fn block<'a>(mut bits: Bits<'a>, out: &mut Vec<u8>, mut at: usize, most: usize, [lengths, distances]: &[Codes; 2]) -> Option<(Bits<'a>, usize)> {
    loop {
        // (How far output can come before one more code might not fit.)
        let (limit, room) = (most.min(out.len() - SPARE), &mut out[..]);
        while at <= limit {
            bits.fill()?;
            let mut entry = lengths.read(&mut bits);
            // Literals one after another, while a whole code is buffered.
            while entry & BYTE != 0 && bits.count >= 15 {
                *room.get_mut(at)? = (entry >> 16) as u8;
                (at, entry) = (at + 1, lengths.read(&mut bits));
            }
            if entry & BYTE != 0 {
                *room.get_mut(at)? = (entry >> 16) as u8;
                at += 1;
                continue;
            }
            bits.fill()?;
            if entry & COUNT == 0 {
                return (entry & END != 0).then_some((bits, at));
            }
            let long = (entry >> 16) as usize + bits.take(entry >> 4 & 15);
            let entry = distances.read(&mut bits);
            let back = (entry >> 16) as usize + bits.take(entry >> 4 & 15);
            if entry & COUNT == 0 || back > at {
                return None;
            }
            repeat(room, at, back, long)?;
            at += long;
        }
        if at > most {
            return None;
        }
        // (Exactly this much more: a vector left to grow by itself may take twice the limit.)
        let room = most.min(out.len().saturating_mul(ROOM)) + SPARE;
        out.reserve_exact(room - out.len());
        out.resize(room, 0);
    }
}

/// Writes at `at` the `long` bytes lying `back` before it, sixteen at a time (so up to fifteen
/// bytes after are written too, with garbage).
#[inline(always)]
fn repeat(out: &mut [u8], at: usize, back: usize, long: usize) -> Option<()> {
    let (from, end, mut to, mut gap) = (at - back, at + long, at, back);
    // Fewer than sixteen bytes back, source and destination overlap: what lies between repeats.
    // Each pass writes it once more, doubling how much there is to copy from.
    while gap < 16 && to < end {
        *out.get_mut(to..)?.first_chunk_mut::<16>()? = *out.get(from..)?.first_chunk::<16>()?;
        (to, gap) = (to + gap, gap * 2);
    }
    while to < end {
        *out.get_mut(to..)?.first_chunk_mut::<16>()? = *out.get(to - gap..)?.first_chunk::<16>()?;
        to += 16;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pseudo-random bytes, the same every time.
    fn noise(count: usize) -> Vec<u8> {
        (0..count as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
    }

    #[test]
    fn a_picture_that_is_written_reads_back_the_same() {
        let solid = |pixels: &[u32]| pixels.iter().map(|pixel| 0xff00_0000 | pixel).collect::<Vec<u32>>();
        // Random pixels in pictures of many shapes: nothing in them compresses.
        let pixels: Vec<u32> = (0..300 * 200u32).map(|i| i.wrapping_mul(2_654_435_761) >> 8 | i.wrapping_mul(40_503) << 24).collect();
        for (width, height) in [(300, 200), (1, 1), (1, 9), (9, 1), (7, 5), (1000, 3)] {
            let pixels = &pixels[..width * height];
            assert_eq!(decode(&encode(width, pixels).unwrap()), Some((width, solid(pixels))), "{width} by {height}");
            // And with alpha, the high byte of the same noise.
            assert_eq!(decode(&encode_rgba(width, pixels).unwrap()), Some((width, pixels.to_vec())), "{width} by {height}, seen through");
        }
        // A one-colour picture comes to almost nothing, and a window-like one (bands, boxes in
        // them) to a small part of its three bytes per pixel.
        let flat = vec![0x33_66_99; 900 * 600];
        let window: Vec<u32> =
            (0..900 * 600).map(|i| if i / 900 % 40 < 3 || (i % 900 % 120 < 2 && i / 900 % 80 > 40) { 0x20_20_20 } else { 0xf0_f0_f4 - (i as u32 / 900 / 80) * 0x0a_0a_08 }).collect();
        for (pixels, most) in [(&flat, 4_000), (&window, 20_000)] {
            let png = encode(900, pixels).unwrap();
            assert!(png.len() < most, "{} bytes", png.len());
            assert_eq!(decode(&png), Some((900, solid(pixels))));
        }
    }

    #[test]
    fn only_a_picture_that_a_png_can_hold_is_written() {
        // A picture of nothing, of no width, or fewer pixels than one row, is none; a picture
        // is as high as its whole rows (the remainder is dropped).
        for (width, count) in [(0, 0), (0, 12), (4, 0), (4, 3)] {
            assert_eq!(encode(width, &vec![0; count]), None, "{width} wide, {count} pixels");
            assert_eq!(encode_rgba(width, &vec![0; count]), None, "{width} wide, {count} pixels, seen through");
        }
        let pixels: Vec<u32> = (0..14).map(|at| 0xff00_0000 | (at * 0x0a_0b0c)).collect();
        assert_eq!(decode(&encode_rgba(4, &pixels).unwrap()), Some((4, pixels[..12].to_vec())), "three rows of four");
    }

    #[test]
    fn pictures_from_other_programs_are_read() {
        // Compressed with dynamic codes, by a drawing library and by zlib; one with alpha, one
        // without.
        for (name, png) in [("windows", &include_bytes!("../docs/windows.png")[..]), ("linux", &include_bytes!("../docs/linux.png")[..])] {
            let (width, pixels) = decode(png).unwrap_or_else(|| panic!("{name}.png is read"));
            assert!(width >= 600 && pixels.len() % width == 0 && pixels.len() / width >= 400, "{name}: {width} wide, {} pixels", pixels.len());
            assert!(pixels.iter().all(|pixel| pixel >> 24 == 255), "{name}: a picture of a window is solid all over");
        }
        assert_eq!(decode(b"not a picture"), None);
        let mut damaged = include_bytes!("../docs/linux.png").to_vec();
        damaged[2000] ^= 0x10;
        assert_eq!(decode(&damaged), None, "a damaged file is noticed");
    }

    #[test]
    fn a_damaged_file_is_no_picture_and_stops_nothing() {
        let pixels: Vec<u32> = noise(12 * 9).iter().map(|byte| *byte as u32 * 0x01_0305).collect();
        let png = encode(12, &pixels).unwrap();
        let whole = decode(&png);
        assert!(whole.is_some());
        // Truncated anywhere before its last chunk (which is empty), it is none.
        for cut in 0..png.len() - 12 {
            assert_eq!(decode(&png[..cut]), None, "cut after {cut} bytes");
        }
        // With any one bit flipped it is the same picture, another, or none: it must not crash.
        // In the compressed rows it is never another: the checksum catches it. (The same it can
        // be: the padding bits of the last compressed byte mean nothing.)
        let packed = png.windows(4).position(|name| name == b"IDAT").unwrap() + 4..png.len() - 16;
        for bit in 0..png.len() * 8 {
            let mut damaged = png.clone();
            damaged[bit / 8] ^= 1 << (bit % 8);
            let read = decode(&damaged);
            assert!(!packed.contains(&(bit / 8)) || read.is_none() || read == whole, "bit {bit} of the packed rows");
        }
    }

    /// Paeth's predictor as specified: of before, above and above-before, the one nearest
    /// "before + above - above-before".
    fn by_the_book(before: u8, up: u8, slant: u8) -> u8 {
        let [a, b, c] = [before as i16, up as i16, slant as i16];
        let (from_a, from_b, from_c) = ((b - c).abs(), (a - c).abs(), (a + b - 2 * c).abs());
        match () {
            _ if from_a <= from_b && from_a <= from_c => before,
            _ if from_b <= from_c => up,
            _ => slant,
        }
    }

    #[test]
    fn the_nearest_is_found_as_the_book_says() {
        for before in 0..=255u8 {
            for up in 0..=255u8 {
                for slant in 0..=255u8 {
                    assert_eq!(nearest(before as i32, up as i32, slant as i32), by_the_book(before, up, slant) as i32, "{before} {up} {slant}");
                }
            }
        }
    }

    /// A row of `N`-byte pixels un-Paethed as `unfilter` does it (a whole pixel at once, where
    /// the processor can), and a byte at a time.
    fn both_ways<const N: usize>(seed: u32) -> [Vec<u8>; 2] {
        let noise = |seed: u32| (0..N as u32 * 61).map(|i| ((i + seed).wrapping_mul(2_654_435_761) >> 13) as u8).collect::<Vec<u8>>();
        let (above, mut quick, mut plain) = (noise(seed), noise(seed + 1000), noise(seed + 1000));
        unfilter(4, &mut quick, &above, N).unwrap();
        each_pixel::<N>(&mut plain, &above, nearest);
        [quick, plain]
    }

    #[test]
    fn a_whole_pixel_at_once_comes_to_what_a_byte_at_a_time_does() {
        for seed in 0..200 {
            let ([quick, plain], [wide, narrow]) = (both_ways::<3>(seed), both_ways::<4>(seed));
            assert!(quick == plain && wide == narrow, "seed {seed}");
        }
    }

    /// Filters rows with a filter type: each byte as its difference from the prediction. (The
    /// inverse of `unfilter`.)
    fn filter(rows: &[Vec<u8>], step: usize, rule: u8) -> Vec<u8> {
        let mut raw = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            raw.push(rule);
            for i in 0..row.len() {
                let (before, up) = (if i < step { 0 } else { row[i - step] }, if y == 0 { 0 } else { rows[y - 1][i] });
                let slant = if i < step || y == 0 { 0 } else { rows[y - 1][i - step] };
                raw.push(row[i].wrapping_sub([0, before, up, ((before as u16 + up as u16) / 2) as u8, by_the_book(before, up, slant)][rule as usize]));
            }
        }
        raw
    }

    #[test]
    fn every_rule_for_storing_a_row_is_undone() {
        // Four rows of eleven pixels, for every colour type with whole-byte samples: each
        // sample's value, and bytes per pixel. (Of two bytes the first counts.)
        let palette: Vec<u8> = (0..768).map(|i| (i * 7 + i / 3) as u8).collect();
        for (kind, depth, step) in [(0, 8, 1), (3, 8, 1), (4, 8, 2), (0, 16, 2), (2, 8, 3), (6, 8, 4), (4, 16, 4), (2, 16, 6), (6, 16, 8)] {
            let rows: Vec<Vec<u8>> = (0..4).map(|y| (0..11 * step).map(|i| (i * 37 + y * 101 + i * i * y) as u8).collect()).collect();
            let value = |pixel: &[u8], n: usize| pixel[n * depth as usize / 8] as u32;
            let listed = |pixel: &[u8]| (0..3).fold(0, |colour, part| colour << 8 | palette[value(pixel, 0) as usize * 3 + part] as u32);
            let seen = |pixel: &[u8]| match kind {
                0 => 0xff00_0000 | (value(pixel, 0) * 0x01_0101),
                2 => 0xff00_0000 | value(pixel, 0) << 16 | value(pixel, 1) << 8 | value(pixel, 2),
                3 => 0xff00_0000 | listed(pixel),
                4 => (value(pixel, 1) << 24) | (value(pixel, 0) * 0x01_0101),
                _ => value(pixel, 3) << 24 | value(pixel, 0) << 16 | value(pixel, 1) << 8 | value(pixel, 2),
            };
            let expected: Vec<u32> = rows.iter().flat_map(|row| row.chunks(step).map(seen)).collect();
            for rule in 0..5 {
                let png = file(11, 4, [depth, kind], &palette[..if kind == 3 { 768 } else { 0 }], &filter(&rows, step, rule));
                assert_eq!(decode(&png), Some((11, expected.clone())), "kind {kind} of {depth} bits, rule {rule}");
            }
            // An unknown filter: no picture.
            let mut raw = filter(&rows, step, 0);
            raw[11 * step + 1] = 5;
            assert_eq!(decode(&file(11, 4, [depth, kind], &palette, &raw)), None);
        }
        // And a colour type not in that depth is none: four-bit RGB.
        assert_eq!(decode(&file(2, 1, [4, 2], &[], &[0, 0x12, 0x34, 0x56])), None);
    }

    /// The same picture as `rows` (pixels of `step` bytes), interlaced: each pass's rows, with
    /// each pass's filter.
    fn in_passes(rows: &[Vec<u8>], step: usize) -> Vec<u8> {
        let (width, mut raw) = (rows[0].len() / step, Vec::new());
        for (pass, &[left, top, across, down]) in PASSES[1..].iter().enumerate() {
            let pixels = |row: &Vec<u8>| (left..width).step_by(across).flat_map(|x| row[x * step..][..step].to_vec()).collect::<Vec<u8>>();
            let of_pass: Vec<Vec<u8>> = rows.iter().skip(top).step_by(down).map(pixels).filter(|row| !row.is_empty()).collect();
            raw.extend(filter(&of_pass, step, pass as u8 % 5));
        }
        raw
    }

    #[test]
    fn pictures_stored_in_passes_are_read() {
        // Every size up to a little over the coarsest pass's eight-pixel step: in the small
        // ones, some passes are empty.
        for (width, height) in (1..11).flat_map(|width| (1..11).map(move |height| (width, height))) {
            let rows: Vec<Vec<u8>> = (0..height).map(|y| (0..width * 4).map(|i| (i * 37 + y * 101 + i * i * y) as u8).collect()).collect();
            let expected: Vec<u32> = rows.iter().flat_map(|row| row.chunks(4).map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)).collect();
            let mut png = file(width, height, [8, 6], &[], &in_passes(&rows, 4));
            png[28] = 1; // "stored in passes"
            assert_eq!(decode(&png), Some((width, expected)), "{width} by {height}");
        }
        // One made by libpng, of a picture also here non-interlaced.
        let (passes, plain) = (decode(include_bytes!("../tests/pictures/passes.png")), decode(include_bytes!("../tests/pictures/plain.png")));
        assert!(passes.is_some() && passes == plain);
        // A file claiming a size its rows do not come to is no picture.
        let mut large = file(5, 3, [8, 6], &[], &in_passes(&[vec![0; 20], vec![0; 20], vec![0; 20]], 4));
        (large[16], large[28]) = (0x40, 1); // a thousand million pixels wide
        assert_eq!(decode(&large), None);
        // Nor one with more rows than it says: eight, in a file that says one.
        let mut short = encode(8, &[0xff12_3456; 64]).unwrap();
        assert!(decode(&short).is_some());
        short[23] = 1;
        assert_eq!(decode(&short), None);
    }

    #[test]
    fn small_depths_palettes_and_grey_are_read() {
        // Ten one-bit black-and-white pixels per row.
        let png = file(10, 1, [1, 0], &[], &[0, 0b1010_0000, 0b1100_0000]);
        let (black, white) = (0xff00_0000, 0xffff_ffff);
        assert_eq!(decode(&png), Some((10, vec![white, black, white, black, black, black, black, black, white, white])));
        // Four two-bit pixels indexing a three-colour palette.
        let mut png = file(4, 1, [2, 3], &[255, 0, 0, 0, 255, 0, 0, 0, 255], &[0, 0b00_01_10_00]);
        assert_eq!(decode(&png), Some((4, vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_0000])));
        // The same with a `tRNS` chunk before the rows, giving the first two colours' alpha.
        let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
        png.splice(rows..rows, [&[0, 0, 0, 2][..], b"tRNS", &[0x80, 0], &[0; 4]].concat());
        assert_eq!(decode(&png), Some((4, vec![0x80ff_0000, 0x0000_ff00, 0xff00_00ff, 0x80ff_0000])));
        // Greys, one marked transparent: wherever it stands, and nothing else.
        let mut png = file(3, 1, [8, 0], &[], &[0, 10, 20, 10]);
        let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
        png.splice(rows..rows, [&[0, 0, 0, 2][..], b"tRNS", &[0, 10], &[0; 4]].concat());
        assert_eq!(decode(&png), Some((3, vec![0x000a_0a0a, 0xff14_1414, 0x000a_0a0a])));
        // A sixteen-bit colour is the transparent one only when all its bits match.
        let mut png = file(2, 1, [16, 2], &[], &[0, 0x12, 0x34, 0, 0, 0xff, 0xff, 0x12, 0x35, 0, 0, 0xff, 0xff]);
        let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
        png.splice(rows..rows, [&[0, 0, 0, 6][..], b"tRNS", &[0x12, 0x34, 0, 0, 0xff, 0xff], &[0; 4]].concat());
        assert_eq!(decode(&png), Some((2, vec![0x0012_00ff, 0xff12_00ff])));
        // Five four-bit greys: the highest is white.
        let png = file(5, 1, [4, 0], &[], &[0, 0x0f, 0x71, 0x80]);
        assert_eq!(decode(&png), Some((5, vec![black, white, 0xff77_7777, 0xff11_1111, 0xff88_8888])));
        // Sixteen-bit grey with alpha: the high bytes count.
        let png = file(1, 1, [16, 4], &[], &[0, 0x80, 0x12, 0x40, 0x34]);
        assert_eq!(decode(&png), Some((1, vec![0x4080_8080])));
    }

    #[test]
    fn what_is_packed_unpacks_to_the_same() {
        // Nothing; one byte; a long run; random bytes; a few of those repeated, also from
        // closer than their length; and symbols each about as frequent as the two before
        // together, whose codes come out as long as allowed.
        let noise = noise(200_000);
        let again: Vec<u8> = noise[..30_000].chunks(7).flat_map(|seven| [seven, seven, &seven[..3], &[seven[0]; 40]].concat()).collect();
        let skewed: Vec<u8> = (0..24u32).flat_map(|symbol| vec![symbol as u8; 1 << (symbol * 2 / 3)]).zip(&noise).map(|(symbol, shift)| symbol + (shift & 1)).collect();
        for (bytes, at_most) in [(&[][..], 24), (&[7], 24), (&[0; 100_000], 200), (&noise, 200_400), (&again, 62_000), (&skewed, skewed.len() / 2)] {
            let packed = deflate(bytes);
            assert!(packed.len() <= at_most, "{} bytes pack to {}", bytes.len(), packed.len());
            assert_eq!(inflate(&packed, bytes.len()).as_deref(), Some(bytes));
            assert_eq!(inflate(&packed, bytes.len() + 1000).as_deref(), Some(bytes));
            // Output past the limit, by one byte or many, is none.
            assert!(bytes.is_empty() || (inflate(&packed, bytes.len() - 1).is_none() && inflate(&packed, bytes.len() / 3).is_none()));
        }
        // Truncated anywhere, a stream is none. With any one bit flipped it is none too, but
        // for the bits that mean nothing (the two header bytes, and the last compressed byte's
        // padding before the checksum): never other bytes.
        let bytes = &again[..1500];
        let packed = deflate(bytes);
        for cut in 0..packed.len() {
            assert_eq!(inflate(&packed[..cut], bytes.len()), None, "cut after {cut} bytes");
        }
        let mut unnoticed = 0;
        for bit in 0..packed.len() * 8 {
            let mut damaged = packed.clone();
            damaged[bit / 8] ^= 1 << (bit % 8);
            let read = inflate(&damaged, bytes.len());
            assert!(read.is_none() || read.as_deref() == Some(bytes), "bit {bit}");
            unnoticed += read.is_some() as usize;
        }
        assert!(unnoticed < 16 + 8, "{unnoticed} bits can be turned over and nobody notices");
    }

    #[test]
    fn streams_of_other_packers_are_unpacked() {
        // Stored blocks: "abc", then "de" in a final one. Each gives its length twice, the
        // second inverted; if they disagree, none.
        let stored = [0x78, 0x01, 0, 3, 0, 0xfc, 0xff, b'a', b'b', b'c', 1, 2, 0, 0xfd, 0xff, b'd', b'e', 0x05, 0xc8, 0x01, 0xf0];
        assert_eq!(inflate(&stored, 5).as_deref(), Some(&b"abcde"[..]));
        assert_eq!(inflate(&stored, 4), None);
        let mut other = stored;
        other[5] = 0xfb;
        assert_eq!(inflate(&other, 5), None);
        // Fixed codes, as zlib writes them: "hello hello hello hello".
        let usual = [0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1];
        assert_eq!(inflate(&usual, 23).as_deref(), Some(&b"hello hello hello hello"[..]));
        // A block type that does not exist, and code lengths over-filling the code space.
        assert_eq!(inflate(&[0x78, 0x01, 0b111, 0, 0, 0, 0, 0], 9), None);
        assert!(codes(&[1, 1, 1], 7, byte_or_long).is_none() && codes(&[1, 2, 2], 7, byte_or_long).is_some());
    }

    #[test]
    fn codes_are_no_longer_than_they_may_be_and_leave_nothing_over() {
        // Symbols each as frequent as the two before together: unlimited, the rarest would get
        // a code about as long as there are symbols.
        let mut rising = vec![1u32, 1];
        (2..40).for_each(|n| rising.push(rising[n - 1] + rising[n - 2]));
        for (often, longest) in [(&rising[..], 15), (&rising[..19], 7), (&[0, 0, 5][..], 15), (&[0; 30][..], 15), (&[3, 0, 0, 9, 1][..], 7)] {
            let lengths = lengths_of(often, longest);
            assert!(lengths.iter().all(|length| *length as usize <= longest), "{lengths:?}");
            assert!(often.iter().zip(&lengths).all(|(times, length)| *times == 0 || *length != 0), "{lengths:?}");
            // Together the codes fill the code space exactly.
            assert_eq!(lengths.iter().filter(|length| **length != 0).map(|length| 1u32 << (15 - length)).sum::<u32>(), 1 << 15, "{lengths:?}");
        }
        assert_eq!(lengths_of(&[8, 1, 1, 2, 4], 15), [1, 4, 4, 3, 2]);
        // What is written for those lengths is what a reader takes them to be.
        let lengths = lengths_of(&rising, 15);
        let (written, read) = (written(&lengths), codes(&lengths, 11, |symbol| BYTE | (symbol as u32) << 16).unwrap());
        for (symbol, code) in written.iter().enumerate() {
            let mut bits = Bits { data: &(code & 0xffff).to_le_bytes(), at: 0, held: 0, count: 0 };
            bits.fill();
            assert_eq!((read.read(&mut bits) >> 16, bits.taken() as u32), (symbol as u32, code >> 16), "symbol {symbol}");
        }
    }

    #[test]
    fn the_sum_is_the_one_a_zlib_stream_ends_with() {
        let by_the_book = |bytes: &[u8]| bytes.iter().fold((1u32, 0u32), |(low, high), byte| ((low + *byte as u32) % 65521, (high + low + *byte as u32) % 65521));
        assert_eq!(sum(b"Wikipedia"), 0x11e6_0398);
        // Every length around the eight-byte step and around the 5552-byte block; random bytes,
        // and 255s.
        for long in [0, 1, 7, 8, 9, 15, 16, 17, 5551, 5552, 5553, 70_001] {
            for bytes in [noise(long), vec![255; long]] {
                let (low, high) = by_the_book(&bytes);
                assert_eq!(sum(&bytes), high << 16 | low, "{long} bytes");
            }
        }
    }

    use crate::animation::tests::{frames, pattern, playable, seen};
    use crate::animation::{After, Laid, Reel};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// A hand-made APNG frame: its region, duration (numerator and denominator), dispose op (0
    /// to 2), blend op (0, source; 1, over), and its rows each preceded by its filter type
    /// (from `rows` or `in_passes`).
    #[derive(Clone)]
    struct Made {
        place: [u32; 4],
        shows: [u16; 2],
        after: u8,
        laid: u8,
        raw: Vec<u8>,
    }

    /// A hand-made APNG: a `size` canvas with the pixel `kind` a header names (bit depth,
    /// colour type, interlace), a `palette`, `plays` loops. Its default image is `picture`
    /// (rows as `Made` has them) and not a frame; or, where none, its first frame.
    fn moving(size: [u32; 2], kind: [u8; 3], palette: &[u8], plays: u32, picture: Option<&[u8]>, frames: &[Made]) -> Vec<u8> {
        let header = [&size[0].to_be_bytes()[..], &size[1].to_be_bytes(), &[kind[0], kind[1], 0, 0, kind[2]]].concat();
        let mut png = [&START[..], &chunk(b"IHDR", &header), &chunk(b"acTL", &[(frames.len() as u32).to_be_bytes(), plays.to_be_bytes()].concat())].concat();
        if !palette.is_empty() {
            png.extend(chunk(b"PLTE", &palette[..palette.len() / 4 * 3]));
            png.extend(chunk(b"tRNS", &palette[palette.len() / 4 * 3..]));
        }
        let mut number = 0u32;
        let mut next = || (number, number += 1).0.to_be_bytes();
        png.extend(picture.map(|raw| chunk(b"IDAT", &deflate(raw))).unwrap_or_default());
        for (at, made) in frames.iter().enumerate() {
            let [left, top, wide, high] = made.place;
            let said =
                [&next()[..], &wide.to_be_bytes(), &high.to_be_bytes(), &left.to_be_bytes(), &top.to_be_bytes(), &made.shows[0].to_be_bytes(), &made.shows[1].to_be_bytes(), &[made.after, made.laid]]
                    .concat();
            png.extend(chunk(b"fcTL", &said));
            let packed = deflate(&made.raw);
            png.extend(match at == 0 && picture.is_none() {
                true => chunk(b"IDAT", &packed),
                false => chunk(b"fdAT", &[&next()[..], &packed].concat()),
            });
        }
        [png, chunk(b"IEND", &[])].concat()
    }

    /// Rows of pixels with alpha, as `moving` takes them.
    fn solid_rows(width: usize, pixels: &[u32]) -> Vec<u8> {
        rows::<4>(width, pixels)
    }

    /// The APNG test files: name, file, ffmpeg's composed frames (stacked), each frame's
    /// duration, and the loop count.
    type Moving = (&'static str, &'static [u8], &'static [u8], &'static [u64], Option<u64>);
    const MOVING: [Moving; 5] = [
        // Five frames by ffmpeg, which after the first writes only the changed rectangle,
        // blended or not and disposed as it finds best.
        ("moving", include_bytes!("../tests/pictures/moving.apng.png"), include_bytes!("../tests/pictures/moving.apng.frames.png"), &[200; 5], None),
        // Hand-made: a default image that is no frame, and frames blended semi-transparent,
        // disposed to background, to previous, not blended; durations every way; three loops.
        ("hand", include_bytes!("../tests/pictures/hand.apng.png"), include_bytes!("../tests/pictures/hand.apng.frames.png"), &[500, 100, 30, 100, 700], Some(3)),
        // Hand-made: the default image is the first frame, which disposes to previous.
        ("first", include_bytes!("../tests/pictures/first.apng.png"), include_bytes!("../tests/pictures/first.apng.frames.png"), &[250, 250, 250], Some(1)),
        // Hand-made: palette indices, colours fully and half transparent.
        ("palette", include_bytes!("../tests/pictures/palette.apng.png"), include_bytes!("../tests/pictures/palette.apng.frames.png"), &[100, 200, 300], None),
        // Hand-made: every frame interlaced.
        ("passes", include_bytes!("../tests/pictures/passes.apng.png"), include_bytes!("../tests/pictures/passes.apng.frames.png"), &[40, 40, 40], None),
    ];

    #[test]
    fn moving_pictures_come_out_as_ffmpeg_shows_them() {
        for (name, png, wanted, shows, plays) in MOVING {
            let mut reel = Reel::read(png, usize::MAX).unwrap_or_else(|| panic!("{name} is read"));
            let (width, wanted) = decode(wanted).unwrap();
            assert_eq!((reel.size[0], reel.frames.len() * reel.size[1]), (width, wanted.len() / width), "{name}: its size, and how many frames it has");
            let (have, want) = (seen(&frames(&mut reel)), seen(&wanted));
            let differ = have.iter().zip(&want).filter(|(one, other)| one != other).count();
            let first = have.iter().zip(&want).position(|(one, other)| one != other).map(|at| (at, have[at], want[at]));
            assert!(have == want, "{name}: {differ} pixels differ, the first (where, what, for what): {first:x?}");
            assert_eq!((reel.frames.iter().map(|frame| frame.shows).collect::<Vec<u64>>(), reel.plays), (shows.to_vec(), plays), "{name}: its times");
        }
    }

    #[test]
    fn a_moving_picture_is_a_picture_where_it_does_not_move() {
        // Its default image where that is no frame; its first frame where it is.
        let (hand, first) = (decode(MOVING[1].1).unwrap(), decode(MOVING[2].1).unwrap());
        assert_eq!(hand, (8, vec![0xff33_6699; 48]));
        let mut reel = Reel::read(MOVING[2].1, 1).unwrap();
        assert_eq!(first, (reel.size[0], reel.shown(0).0.to_vec()));
        // And a still PNG is a one-frame animation: its picture.
        let still = include_bytes!("../tests/pictures/plain.png");
        let mut reel = Reel::read(still, usize::MAX).unwrap();
        assert_eq!((reel.frames.len(), (reel.shown(0).0.to_vec(), reel.size[0])), (1, (decode(still).unwrap().1, decode(still).unwrap().0)));
    }

    #[test]
    fn frames_lie_and_are_laid_as_their_chunks_say() {
        // A 4×1 canvas: red over all; green over the middle two, disposed to background; blue
        // at the right, disposed to previous; then half-transparent white blended over the left
        // three, leaving the third as it was where clear.
        let (red, green, blue, half) = (0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0x80ff_ffff);
        let made = |place: [u32; 4], after: u8, laid: u8, pixels: &[u32]| Made { place, shows: [1, 10], after, laid, raw: solid_rows(place[2] as usize, pixels) };
        let four = [made([0, 0, 4, 1], 0, 0, &[red; 4]), made([1, 0, 2, 1], 1, 0, &[green; 2]), made([3, 0, 1, 1], 2, 0, &[blue]), made([0, 0, 3, 1], 0, 1, &[half, half, 0])];
        let mut reel = Reel::read(&moving([4, 1], [8, 6, 0], &[], 0, None, &four), usize::MAX).unwrap();
        let laid = reel.frames.iter().map(|frame| (frame.after, frame.laid)).collect::<Vec<_>>();
        assert_eq!(laid, [(After::Stays, Laid::Instead), (After::Cleared, Laid::Instead), (After::PutBack, Laid::Instead), (After::Stays, Laid::OverPng)]);
        assert_eq!(frames(&mut reel), [[red, red, red, red], [red, green, green, red], [red, 0, 0, blue], [0xffff_8080, 0x80ff_ffff, 0, red]].concat());
        // Two-bit palette indices, in a frame not starting at the left edge: it lies at its
        // offset, though its pixels do not start on a byte.
        let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255];
        let listed = |place: [u32; 4], raw: &[u8]| Made { place, shows: [1, 10], after: 0, laid: 0, raw: raw.to_vec() };
        let two = [listed([0, 0, 5, 1], &[0, 0x1b, 0]), listed([3, 0, 2, 1], &[0, 0x90])];
        let mut reel = Reel::read(&moving([5, 1], [2, 3, 0], &palette, 0, None, &two), usize::MAX).unwrap();
        assert_eq!(frames(&mut reel), [[red, green, blue, 0xffff_ffff, red], [red, green, blue, blue, green]].concat());
        // A frame off the canvas ends the animation before it, as do one with a dispose or
        // blend op that does not exist, and a damaged one.
        let off = [made([0, 0, 4, 1], 0, 0, &[red; 4]), made([2, 0, 3, 1], 0, 0, &[green; 3])];
        assert_eq!(Reel::read(&moving([4, 1], [8, 6, 0], &[], 0, None, &off), usize::MAX).map(|reel| reel.frames.len()), Some(1));
        let odd = [made([0, 0, 4, 1], 0, 0, &[red; 4]), made([0, 0, 4, 1], 3, 0, &[red; 4]), made([0, 0, 4, 1], 0, 2, &[red; 4])];
        assert_eq!(Reel::read(&moving([4, 1], [8, 6, 0], &[], 0, None, &odd[..2]), usize::MAX).map(|reel| reel.frames.len()), Some(1));
        assert_eq!(Reel::read(&moving([4, 1], [8, 6, 0], &[], 0, None, &[&odd[..1], &odd[2..]].concat()), usize::MAX).map(|reel| reel.frames.len()), Some(1));
    }

    #[test]
    fn a_damaged_moving_picture_gives_nothing_or_some_film() {
        for (name, png, ..) in MOVING {
            // Truncated at every length; then whole with each byte in turn changed.
            assert!((0..png.len()).all(|cut| playable(Reel::read(&png[..cut], usize::MAX))), "{name}, cut short");
            for at in 0..png.len() {
                let damaged = |to: u8| [&png[..at], &[to], &png[at + 1..]].concat();
                assert!([png[at] ^ (1 << (at % 8)), 0, 255].into_iter().all(|to| playable(Reel::read(&damaged(to), usize::MAX))), "{name}, byte {at}");
            }
        }
    }

    /// Rewrites the APNG test files and ffmpeg's composition of each, frame by frame.
    /// (`cargo test --lib png -- --ignored`, with ffmpeg installed.)
    #[test]
    #[ignore = "needs ffmpeg, and writes the files the other tests read"]
    fn the_moving_pictures_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-apng-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |arguments: &[&str]| {
            let done = Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(arguments).current_dir(&work).stdin(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "ffmpeg {arguments:?}");
        };
        let at = |name: &str| pictures.join(name).to_str().unwrap().to_string();
        for step in 0..5 {
            std::fs::write(work.join(format!("f{step}.png")), encode_rgba(24, &pattern(step)).unwrap()).unwrap();
        }
        run(&["-framerate", "5", "-i", "f%d.png", "-plays", "0", "-f", "apng", &at("moving.apng.png")]);
        // Hand-made, on an 8×6 canvas.
        let colour = |at: u32, solid: u32| solid << 24 | (at * 37 % 256) << 16 | (at * 91 % 256) << 8 | (at * 13 % 256);
        let block = |[wide, high]: [u32; 2], solid: &dyn Fn(u32) -> u32| (0..wide * high).map(|at| colour(at + wide, solid(at))).collect::<Vec<u32>>();
        let made = |place: [u32; 4], shows: [u16; 2], after: u8, laid: u8, solid: &dyn Fn(u32) -> u32| Made {
            place,
            shows,
            after,
            laid,
            raw: solid_rows(place[2] as usize, &block([place[2], place[3]], solid)),
        };
        let hand = [
            made([0, 0, 8, 6], [50, 100], 0, 0, &|at| [255, 128, 40][at as usize % 3]),
            made([2, 1, 4, 3], [0, 0], 1, 1, &|at| [200, 90][at as usize % 2]),
            made([1, 1, 3, 3], [3, 0], 2, 1, &|at| [64, 255, 0][at as usize % 3]),
            made([4, 2, 4, 4], [1, 1000], 0, 0, &|at| [0, 255, 170][at as usize % 3]),
            made([0, 0, 8, 6], [7, 10], 0, 1, &|_| 64),
        ];
        std::fs::write(at("hand.apng.png"), moving([8, 6], [8, 6, 0], &[], 3, Some(&solid_rows(8, &[0xff33_6699; 48])), &hand)).unwrap();
        let first =
            [made([0, 0, 8, 6], [1, 4], 2, 0, &|at| [255, 100][at as usize % 2]), made([2, 2, 4, 2], [1, 4], 0, 1, &|_| 150), made([0, 3, 8, 3], [1, 4], 1, 1, &|at| [30, 255][at as usize % 2])];
        std::fs::write(at("first.apng.png"), moving([8, 6], [8, 6, 0], &[], 1, None, &first)).unwrap();
        // Sixteen colours, the first transparent and the second half; a byte per index, each
        // row preceded by its filter. (With fewer bits per index ffmpeg places a frame not at
        // the left edge too far right: see the next.)
        let palette: Vec<u8> = (0..16u8).flat_map(|n| [n * 16, 255 - n * 16, n * 7]).chain([0, 128]).chain([255; 14]).collect();
        let numbers = |place: [u32; 4], seed: u32| (0..place[3]).flat_map(|y| [vec![0], (0..place[2]).map(|x| ((x + y * 3 + seed) % 16) as u8).collect()].concat()).collect::<Vec<u8>>();
        let listed = |place: [u32; 4], shows: [u16; 2], after: u8, seed: u32| Made { place, shows, after, laid: 0, raw: numbers(place, seed) };
        let numbered = [listed([0, 0, 7, 5], [1, 10], 0, 0), listed([1, 1, 5, 3], [2, 10], 1, 5), listed([2, 0, 3, 5], [3, 10], 2, 9)];
        std::fs::write(at("palette.apng.png"), moving([7, 5], [8, 3, 0], &palette, 0, None, &numbered)).unwrap();
        // Every frame interlaced: each pass's rows, each pass with its own filter.
        let passed = |place: [u32; 4], laid: u8, step: usize| {
            let pixels: Vec<u8> = pattern(step).iter().take((place[2] * place[3]) as usize).flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8, (pixel >> 24) as u8]).collect();
            Made { place, shows: [1, 25], after: 0, laid, raw: in_passes(&pixels.chunks(place[2] as usize * 4).map(<[u8]>::to_vec).collect::<Vec<Vec<u8>>>(), 4) }
        };
        std::fs::write(at("passes.apng.png"), moving([11, 9], [8, 6, 1], &[], 0, None, &[passed([0, 0, 11, 9], 0, 0), passed([3, 2, 7, 6], 1, 2), passed([0, 4, 9, 5], 1, 4)])).unwrap();
        // ffmpeg's composition of each, frame by frame, stacked.
        for (name, ..) in MOVING {
            for old in std::fs::read_dir(&work).unwrap().flatten().filter(|file| file.file_name().to_string_lossy().starts_with("out")) {
                std::fs::remove_file(old.path()).unwrap();
            }
            run(&["-f", "apng", "-i", &at(&format!("{name}.apng.png")), "-fps_mode", "passthrough", "-pix_fmt", "rgba", "out%03d.png"]);
            let mut frames: Vec<PathBuf> = std::fs::read_dir(&work).unwrap().flatten().map(|file| file.path()).filter(|path| path.file_name().unwrap().to_string_lossy().starts_with("out")).collect();
            frames.sort();
            let read: Vec<(usize, Vec<u32>)> = frames.iter().map(|frame| decode(&std::fs::read(frame).unwrap()).unwrap()).collect();
            let all: Vec<u32> = read.iter().flat_map(|(_, pixels)| pixels.clone()).collect();
            std::fs::write(at(&format!("{name}.apng.frames.png")), encode_rgba(read[0].0, &all).unwrap()).unwrap();
            println!("{name}: {} frames", read.len());
        }
        std::fs::remove_dir_all(work).unwrap();
    }

    /// A `width` by `height` PNG of random pixels with an `eXIf` orientation chunk before its
    /// rows (without the prefix JPEG puts first).
    fn noted(width: usize, height: usize, turn: u16, low_first: bool) -> (Vec<u8>, Vec<u32>) {
        let pixels: Vec<u32> = (0..width * height).map(|at| 0xff00_0000 | (at as u32).wrapping_mul(2_654_435_761) >> 8).collect();
        let png = encode(width, &pixels).unwrap();
        let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
        let notes = chunk(b"eXIf", &crate::exif_orientation::tests::notes(turn, low_first)[6..]);
        ([&png[..rows], &notes, &png[rows..]].concat(), pixels)
    }

    #[test]
    fn a_note_from_the_camera_turns_the_picture_each_of_eight_ways() {
        let (width, height) = (5, 3);
        for (turn, low_first) in (1..=8).flat_map(|turn| [(turn, true), (turn, false)]) {
            let (png, stored) = noted(width, height, turn, low_first);
            let (across, shown) = decode(&png).expect("is read with a note in it");
            assert_eq!(across, if turn < 5 { width } else { height }, "turn {turn}: its width");
            let from = |at: usize| crate::exif_orientation::tests::from(turn as usize, [width, height], at % across, at / across);
            assert!(shown.iter().copied().eq((0..stored.len()).map(from).map(|(x, y)| stored[y * width + x])), "turn {turn}, low end first: {low_first}");
        }
        // An orientation value that does not exist turns nothing.
        let (png, stored) = noted(width, height, 9, true);
        assert_eq!(decode(&png), Some((width, stored)));
    }
}
