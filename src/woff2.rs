//! WOFF2 web fonts, unpacked into the font file they were, which `font_file.rs` reads as any
//! other. Only with the feature `woff2`: unpacking needs Brotli's built-in dictionary, which a
//! program reading no such font need not carry (see `brotli.rs`).
//!
//! A file starts with a 48-byte header, then its table directory, each table named by an index
//! (one of 63 common tables) or by its tag, with its length. All tables lie in one Brotli
//! stream. Some are transformed to pack better: `glyf` as separate streams per kind of value
//! (contour counts, point counts, flags, deltas...), `loca` omitted, and `hmtx` without left
//! side bearings where they equal the glyph's xMin. Those are rebuilt here byte for byte as the
//! reference decoder rebuilds them. A collection lists its tables once, and which each font
//! has.
//!
//! Only the tables `font_file.rs` reads are kept (as for WOFF), and what a file may ask for is
//! bounded the same way: unpacked tables, and rebuilt glyphs, never exceed 64 times the file.
use crate::font_file::{Bytes, listing, taken};

/// The tables a file names by index, in index order.
#[rustfmt::skip]
const KNOWN: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep",
    b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE",
    b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt",
    b"avar", b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty", b"just", b"lcar",
    b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
];

/// How many times the file's size its tables may come to, unpacked and rebuilt. (Fonts pack to
/// about a third; a font of zeros would pack smaller, and is no font.)
const GROWTH: usize = 64;

/// A table directory entry: its tag, whether it is transformed, its original length, and its
/// offset and length in the unpacked stream.
struct Entry {
    tag: [u8; 4],
    changed: bool,
    original: usize,
    at: usize,
    long: usize,
}

/// A font of the file: its flavour and which tables it has.
struct Font {
    kind: [u8; 4],
    tables: Vec<usize>,
}

/// The font a WOFF2 file holds, as the file it was (a collection for a collection) but only
/// with the tables read here. Nothing for a damaged or unreadable file, or one that would
/// unpack to more than allowed.
pub(crate) fn unpacked(file: &[u8]) -> Option<Vec<u8>> {
    let all = Bytes(file);
    (all.u32(0)? == u32::from_be_bytes(*b"wOF2") && all.u32(8)? as usize == file.len()).then_some(())?;
    let (kind, count, packed) = (all.part(4, 4)?.0.try_into().ok()?, all.u16(12)? as usize, all.u32(20)? as usize);
    let mut reading = Reading { all, at: 48 };
    let mut start = 0;
    let entries = (0..count).map(|_| reading.entry(&mut start)).collect::<Option<Vec<_>>>()?;
    let fonts = match &kind {
        b"ttcf" => reading.collection(count)?,
        _ => vec![Font { kind, tables: (0..count).collect() }],
    };
    let most = file.len().saturating_mul(GROWTH);
    (count > 0 && start <= most).then_some(())?;
    let block = crate::brotli::unpacked(all.part(reading.at, packed)?.0, start)?;
    let mut built = Built { entries: &entries, block: &block, pieces: Vec::new(), most };
    let fonts = fonts.iter().map(|font| built.font(font)).collect::<Option<Vec<_>>>()?;
    Some(built.file(&kind, &fonts))
}

/// Reads the directories at the start of a file.
struct Reading<'a> {
    all: Bytes<'a>,
    at: usize,
}

impl Reading<'_> {
    fn u8(&mut self) -> Option<u8> {
        self.at += 1;
        self.all.u8(self.at - 1)
    }

    fn u32(&mut self) -> Option<u32> {
        self.at += 4;
        self.all.u32(self.at - 4)
    }

    /// A UIntBase128: up to five bytes of seven bits, high first, each but the last with its
    /// top bit set. (No leading zero; must fit 32 bits.)
    fn base128(&mut self) -> Option<usize> {
        let mut number = 0u32;
        for n in 0..5 {
            let byte = self.u8()?;
            (!(n == 0 && byte == 0x80) && number >> 25 == 0).then_some(())?;
            number = number << 7 | (byte & 0x7f) as u32;
            if byte & 0x80 == 0 {
                return Some(number as usize);
            }
        }
        None
    }

    /// A 255UInt16: under 253 as is, or after a byte of 253 (two bytes follow), 254 (one, plus
    /// 506) or 255 (one, plus 253).
    fn short(&mut self) -> Option<usize> {
        let mut stream = Stream { bytes: self.all.0.get(self.at..)?, at: 0 };
        let number = stream.short()?;
        self.at += stream.at;
        Some(number)
    }

    /// The next directory entry, lying at `start` in the unpacked stream (advanced past it). A
    /// transformed table states its transformed length too.
    fn entry(&mut self, start: &mut usize) -> Option<Entry> {
        let flags = self.u8()?;
        let tag = match flags & 63 {
            63 => self.u32()?.to_be_bytes(),
            known => *KNOWN[known as usize],
        };
        // The two top bits give the transform: for `glyf` and `loca`, 0 transformed and 3 not;
        // for `hmtx`, 1 transformed; otherwise 0, not.
        let changed = match (flags >> 6, &tag) {
            (0, b"glyf" | b"loca") => true,
            (3, b"glyf" | b"loca") | (0, _) => false,
            (1, b"hmtx") => true,
            _ => return None,
        };
        let original = self.base128()?;
        let long = if changed { self.base128()? } else { original };
        (tag != *b"loca" || !changed || long == 0).then_some(())?;
        let at = *start;
        *start = start.checked_add(long)?;
        Some(Entry { tag, changed, original, at, long })
    }

    /// A collection's fonts: each one's flavour and table indices.
    fn collection(&mut self, count: usize) -> Option<Vec<Font>> {
        self.u32()?;
        let fonts = self.short()?;
        let mut all = Vec::with_capacity(fonts.min(count));
        for _ in 0..fonts {
            let tables = self.short()?;
            let kind = self.u32()?.to_be_bytes();
            let tables = (0..tables).map(|_| self.short().filter(|n| *n < count)).collect::<Option<Vec<_>>>()?;
            all.push(Font { kind, tables });
        }
        (!all.is_empty()).then_some(all)
    }
}

/// A stream of values of one kind, read in order.
struct Stream<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Stream<'a> {
    fn take(&mut self, long: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(long)?)?;
        self.at += long;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    /// A 255UInt16 (see `Reading::short`).
    fn short(&mut self) -> Option<usize> {
        Some(match self.u8()? {
            253 => self.u16()? as usize,
            254 => 506 + self.u8()? as usize,
            255 => 253 + self.u8()? as usize,
            small => small as usize,
        })
    }
}

/// What a font table is made of: a range of the unpacked stream, or a piece rebuilt here
/// (`Built::pieces`).
#[derive(Clone, Copy, PartialEq)]
enum Made {
    Unpacked(usize),
    Rebuilt(usize),
}

/// A font's tables, each its tag and what it is made of, and the font's flavour.
type Listed = (Vec<([u8; 4], Made)>, [u8; 4]);

/// A file's tables as they are rebuilt.
struct Built<'a> {
    entries: &'a [Entry],
    block: &'a [u8],
    /// The pieces rebuilt so far: each with the table it came from (`glyf`, `loca`, `hmtx`, or
    /// `head` with long offsets), and its bytes.
    pieces: Vec<(usize, [u8; 4], Vec<u8>)>,
    /// How many bytes all pieces may come to.
    most: usize,
}

impl Built<'_> {
    /// The bytes of a directory entry as unpacked.
    fn unpacked(&self, n: usize) -> &[u8] {
        let entry = &self.entries[n];
        &self.block[entry.at..entry.at + entry.long]
    }

    /// The bytes of what a table is made of.
    fn bytes(&self, made: Made) -> &[u8] {
        match made {
            Made::Unpacked(n) => self.unpacked(n),
            Made::Rebuilt(n) => &self.pieces[n].2,
        }
    }

    /// The piece rebuilt from table `n` as `what`, if any.
    fn piece(&self, n: usize, what: &[u8; 4]) -> Option<usize> {
        self.pieces.iter().position(|(from, made, _)| *from == n && made == what)
    }

    /// Keeps a piece rebuilt from table `n` if there is room: says which it is.
    fn keep(&mut self, n: usize, what: [u8; 4], bytes: Vec<u8>) -> Option<usize> {
        self.most = self.most.checked_sub(bytes.len())?;
        self.pieces.push((n, what, bytes));
        Some(self.pieces.len() - 1)
    }

    /// The tables of a font that are read here, each with what it is made of; transformed ones
    /// rebuilt first (once, for collection fonts sharing them).
    fn font(&mut self, font: &Font) -> Option<Listed> {
        let find = |tag: &[u8; 4]| font.tables.iter().copied().find(|n| self.entries[*n].tag == *tag);
        let (glyf, loca, hmtx, head) = (find(b"glyf"), find(b"loca"), find(b"hmtx"), find(b"head"));
        let changed = |n: Option<usize>| n.is_some_and(|n| self.entries[n].changed);
        // `glyf` and `loca` go together, both transformed or neither (and in a collection,
        // `loca` right after `glyf` in the directory).
        (glyf.is_some() == loca.is_some() && changed(glyf) == changed(loca)).then_some(())?;
        let (mut glyphs, mut places, mut wider) = (None, None, false);
        if let (Some(glyf), Some(loca), true) = (glyf, loca, changed(glyf)) {
            (font.tables.len() == self.entries.len() || loca == glyf + 1).then_some(())?;
            if self.piece(glyf, b"glyf").is_none() {
                self.rebuild_glyphs(glyf, loca)?;
            }
            (glyphs, places) = (self.piece(glyf, b"glyf"), self.piece(glyf, b"loca"));
            wider = self.piece(glyf, b"wide").is_some();
        }
        let widths = match hmtx {
            Some(hmtx) if self.entries[hmtx].changed => Some(self.rebuild_widths(hmtx, glyf?, find(b"hhea")?)?),
            _ => None,
        };
        // Where offsets no longer fit two bytes, `head` says they take four.
        let longer = match (head, wider) {
            (Some(head), true) => Some(self.longer(head)?),
            _ => None,
        };
        let has = |tag: &[u8; 4]| find(tag).is_some();
        let made = |tag: &[u8; 4]| {
            let rebuilt = match tag {
                b"glyf" => glyphs,
                b"loca" => places,
                b"hmtx" => widths,
                b"head" => longer,
                _ => None,
            };
            rebuilt.map_or(Made::Unpacked(find(tag).unwrap_or(0)), Made::Rebuilt)
        };
        Some((taken(has).map(|tag| (*tag, made(tag))).collect(), font.kind))
    }

    /// Rebuilds table `glyf`'s glyphs and table `loca`'s offsets as pieces.
    fn rebuild_glyphs(&mut self, glyf: usize, loca: usize) -> Option<()> {
        let rebuilt = glyphs(self.unpacked(glyf), self.entries[loca].original, self.most)?;
        if rebuilt.wider {
            self.keep(glyf, *b"wide", Vec::new())?;
        }
        let lefts = rebuilt.lefts.iter().flat_map(|left| left.to_be_bytes()).collect();
        self.keep(glyf, *b"glyf", rebuilt.glyf)?;
        self.keep(glyf, *b"loca", rebuilt.loca)?;
        self.keep(glyf, *b"left", lefts)?;
        Some(())
    }

    /// Rebuilds table `hmtx` as a piece (once) using `glyf`'s glyphs: says which piece.
    fn rebuild_widths(&mut self, hmtx: usize, glyf: usize, hhea: usize) -> Option<usize> {
        if let Some(done) = self.piece(hmtx, b"hmtx") {
            return Some(done);
        }
        let lefts: Vec<i16> = self.bytes(Made::Rebuilt(self.piece(glyf, b"left")?)).as_chunks::<2>().0.iter().map(|pair| i16::from_be_bytes(*pair)).collect();
        let listed = Bytes(self.unpacked(hhea)).u16(34)? as usize;
        let rebuilt = widths(self.unpacked(hmtx), listed, &lefts)?;
        self.keep(hmtx, *b"hmtx", rebuilt)
    }

    /// `head` saying `loca` takes four-byte offsets, as a piece (once).
    fn longer(&mut self, head: usize) -> Option<usize> {
        if let Some(done) = self.piece(head, b"head") {
            return Some(done);
        }
        let mut changed = self.unpacked(head).to_vec();
        *changed.get_mut(50..52)?.first_chunk_mut::<2>()? = [0, 1];
        self.keep(head, *b"head", changed)
    }

    /// The font file the fonts make: one font as is; a collection as its header with each
    /// font's offset, then their table directories, then every table once.
    fn file(&self, kind: &[u8; 4], fonts: &[Listed]) -> Vec<u8> {
        if kind != b"ttcf" {
            let (tables, kind) = &fonts[0];
            let mut file = listing(kind, tables.iter().map(|(tag, made)| (tag, self.bytes(*made).len())));
            tables.iter().for_each(|(_, made)| file.extend_from_slice(self.bytes(*made)));
            return file;
        }
        // Where each table goes: after the collection header and the fonts' directories.
        let lists: usize = fonts.iter().map(|(tables, _)| 12 + tables.len() * 16).sum();
        let mut placed: Vec<(Made, usize)> = Vec::new();
        let mut end = 12 + fonts.len() * 4 + lists;
        for made in fonts.iter().flat_map(|(tables, _)| tables.iter().map(|(_, made)| *made)) {
            if !placed.iter().any(|(known, _)| *known == made) {
                placed.push((made, end));
                end += self.bytes(made).len();
            }
        }
        let mut file = [&b"ttcf"[..], &[0, 1, 0, 0], &(fonts.len() as u32).to_be_bytes()].concat();
        let mut list = 12 + fonts.len() * 4;
        for (tables, _) in fonts {
            file.extend((list as u32).to_be_bytes());
            list += 12 + tables.len() * 16;
        }
        for (tables, kind) in fonts {
            file.extend([&kind[..], &(tables.len() as u16).to_be_bytes(), &[0; 6]].concat());
            for (tag, made) in tables {
                let at = placed.iter().find(|(known, _)| known == made).map_or(0, |(_, at)| *at);
                file.extend([&tag[..], &[0; 4], &(at as u32).to_be_bytes(), &(self.bytes(*made).len() as u32).to_be_bytes()].concat());
            }
        }
        placed.iter().for_each(|(made, _)| file.extend_from_slice(self.bytes(*made)));
        file
    }
}

/// A font's glyphs rebuilt: `glyf`, `loca`, each glyph's xMin (for `hmtx`), and whether offsets
/// needed four bytes where the font said two.
struct Glyphs {
    glyf: Vec<u8>,
    loca: Vec<u8>,
    lefts: Vec<i16>,
    wider: bool,
}

/// `glyf` point flags: on curve, x short, y short, repeat, x same (or, short, positive), y same
/// (or, short, positive), and overlap simple (first point only).
const ON: u8 = 1;
const ACROSS_SHORT: u8 = 2;
const UP_SHORT: u8 = 4;
const AGAIN: u8 = 8;
const ACROSS_SAME: u8 = 16;
const UP_SAME: u8 = 32;
const OVERLAP: u8 = 64;

/// Rebuilds `glyf` and `loca` from the transformed streams (and `loca`'s length as stated,
/// which must fit the glyphs): each glyph from as much of each stream as it takes, aligned to
/// four bytes. Nothing for streams that do not fit their glyphs, or glyphs over `most` bytes.
fn glyphs(table: &[u8], places: usize, most: usize) -> Option<Glyphs> {
    let mut head = Stream { bytes: table, at: 0 };
    let (_, flags, count, long) = (head.u16()?, head.u16()?, head.u16()? as usize, head.u16()? != 0);
    (places == (count + 1) * if long { 4 } else { 2 }).then_some(())?;
    let mut streams: Vec<Stream> = Vec::with_capacity(7);
    let mut at = 36usize;
    for _ in 0..7 {
        let size = head.u32()? as usize;
        streams.push(Stream { bytes: table.get(at..at.checked_add(size)?)?, at: 0 });
        at += size;
    }
    let overlaps = match flags & 1 {
        0 => &[][..],
        _ => table.get(at..at + count.div_ceil(8))?,
    };
    let [mut contours, mut points, mut flags, mut moves, mut parts, mut boxes, mut instructions]: [Stream; 7] = streams.try_into().ok()?;
    let bits = boxes.take(count.div_ceil(32) * 4)?;
    // Rebuilt glyphs come to about half again what they were transformed into.
    let mut rebuilt = Glyphs { glyf: Vec::with_capacity((table.len() / 2 * 3).min(most)), loca: Vec::with_capacity(places), lefts: vec![0; count], wider: false };
    let mut starts = Vec::with_capacity(count + 1);
    let mut glyph = Glyph { record: Vec::new(), points: Vec::new(), ends: Vec::new(), moves: [Vec::new(), Vec::new()] };
    for n in 0..count {
        let boxed = bits[n / 8] & (0x80 >> (n % 8)) != 0;
        let overlap = overlaps.get(n / 8).is_some_and(|byte| byte & (0x80 >> (n % 8)) != 0);
        glyph.record.clear();
        match contours.u16()? {
            0 => (!boxed).then_some(())?,
            0xffff => glyph.made_of(boxed, [&mut parts, &mut boxes, &mut moves, &mut instructions])?,
            outlines => glyph.simple(outlines, boxed, overlap, [&mut points, &mut flags, &mut moves, &mut boxes, &mut instructions])?,
        }
        if !glyph.record.is_empty() {
            rebuilt.lefts[n] = i16::from_be_bytes([glyph.record[2], glyph.record[3]]);
        }
        starts.push(rebuilt.glyf.len());
        (rebuilt.glyf.len() + glyph.record.len() + 3 <= most).then_some(())?;
        rebuilt.glyf.extend_from_slice(&glyph.record);
        rebuilt.glyf.resize(rebuilt.glyf.len().next_multiple_of(4), 0);
    }
    starts.push(rebuilt.glyf.len());
    // A two-byte offset is halved: past 128 KB they take four.
    rebuilt.wider = !long && rebuilt.glyf.len() > 0x1fffe;
    for start in starts {
        match long || rebuilt.wider {
            true => rebuilt.loca.extend((start as u32).to_be_bytes()),
            false => rebuilt.loca.extend(((start >> 1) as u16).to_be_bytes()),
        }
    }
    Some(rebuilt)
}

/// A glyph being rebuilt: its `glyf` record and, meanwhile, its points (x, y, on curve) and
/// contour ends.
struct Glyph {
    record: Vec<u8>,
    points: Vec<(i32, i32, bool)>,
    ends: Vec<i64>,
    moves: [Vec<u8>; 2],
}

impl Glyph {
    /// A composite glyph: its bounding box (required), each component as it was (flags, glyph,
    /// offset, scale), and instructions if a component says there are some.
    fn made_of(&mut self, boxed: bool, [parts, boxes, moves, instructions]: [&mut Stream; 4]) -> Option<()> {
        boxed.then_some(())?;
        let (start, mut more, mut taught) = (parts.at, true, false);
        while more {
            // The glyph index, its offset in two or four bytes, and a scale, x and y scales, or
            // a 2×2 matrix.
            let flags = parts.u16()?;
            let stretched = match flags {
                _ if flags & 8 != 0 => 2,
                _ if flags & 0x40 != 0 => 4,
                _ if flags & 0x80 != 0 => 8,
                _ => 0,
            };
            parts.take(if flags & 1 != 0 { 6 } else { 4 } + stretched)?;
            (more, taught) = (flags & 0x20 != 0, taught || flags & 0x100 != 0);
        }
        let teaching = if taught { moves.short()? } else { 0 };
        self.record.extend([0xff, 0xff]);
        self.record.extend_from_slice(boxes.take(8)?);
        self.record.extend_from_slice(&parts.bytes[start..parts.at]);
        if taught {
            self.record.extend((teaching as u16).to_be_bytes());
            self.record.extend_from_slice(instructions.take(teaching)?);
        }
        Some(())
    }

    /// A simple glyph: each contour's point count, the points (each a flag and a delta of one
    /// to four bytes as the flag says), its instructions, and its bounding box where it is not
    /// its points' box.
    fn simple(&mut self, outlines: u16, boxed: bool, overlap: bool, [counts, flags, moves, boxes, instructions]: [&mut Stream; 5]) -> Option<()> {
        // Each contour's last point index; a contour of no points first ends at -1, written as
        // the reference writes it, 0xffff.
        self.ends.clear();
        let mut total = 0usize;
        for _ in 0..outlines {
            total += counts.short()?;
            self.ends.push(total as i64 - 1);
        }
        let flags = flags.take(total)?;
        self.points.clear();
        let (mut x, mut y) = (0i32, 0i32);
        for flag in flags {
            let (across, up) = moved(*flag & 0x7f, moves)?;
            (x, y) = (x.wrapping_add(across), y.wrapping_add(up));
            self.points.push((x, y, flag & 0x80 == 0));
        }
        let teaching = moves.short()?;
        self.record.extend(outlines.to_be_bytes());
        match boxed {
            true => self.record.extend_from_slice(boxes.take(8)?),
            false => self.record.extend(self.reach().iter().flat_map(|edge| (*edge as i16).to_be_bytes())),
        }
        self.ends.iter().all(|end| *end < 1 << 16).then_some(())?;
        self.record.extend(self.ends.iter().flat_map(|end| (*end as u16).to_be_bytes()));
        self.record.extend((teaching as u16).to_be_bytes());
        self.record.extend_from_slice(instructions.take(teaching)?);
        self.store(overlap);
        Some(())
    }

    /// The points' bounding box: x min, y min, x max, y max.
    fn reach(&self) -> [i32; 4] {
        let Some(&(x, y, _)) = self.points.first() else { return [0; 4] };
        self.points.iter().fold([x, y, x, y], |[left, low, right, high], (x, y, _)| [left.min(*x), low.min(*y), right.max(*x), high.max(*y)])
    }

    /// Writes the points as `glyf` has them, as the reference decoder does: a flag per point
    /// (repeated up to 255 more times), then x deltas, then y deltas, each one byte where it
    /// fits and none where zero.
    fn store(&mut self, overlap: bool) {
        let (mut last, mut again, mut before) = (None, 0u8, (0, 0));
        let mut moves = std::mem::take(&mut self.moves);
        moves.iter_mut().for_each(Vec::clear);
        for (n, &(x, y, on)) in self.points.iter().enumerate() {
            let mut flag = if on { ON } else { 0 } | if overlap && n == 0 { OVERLAP } else { 0 };
            flag |= written(x.wrapping_sub(before.0), [ACROSS_SHORT, ACROSS_SAME], &mut moves[0]);
            flag |= written(y.wrapping_sub(before.1), [UP_SHORT, UP_SAME], &mut moves[1]);
            before = (x, y);
            // A repeated flag gets the repeat bit, then the repeat count.
            if last == Some(flag) && again != 255 {
                let at = self.record.len() - 1 - (again != 0) as usize;
                self.record[at] |= AGAIN;
                self.record.truncate(at + 1);
                again += 1;
                self.record.push(again);
                continue;
            }
            (last, again) = (Some(flag), 0);
            self.record.push(flag);
        }
        self.record.extend_from_slice(&moves[0]);
        self.record.extend_from_slice(&moves[1]);
        self.moves = moves;
    }
}

/// Writes one delta as `glyf` has it and returns its flag bits: none for zero, one byte (with
/// `short`, plus `same` if positive) where it fits, else two bytes.
fn written(by: i32, [short, same]: [u8; 2], moves: &mut Vec<u8>) -> u8 {
    match by {
        0 => same,
        -255..=255 => {
            moves.push(by.unsigned_abs() as u8);
            short | if by > 0 { same } else { 0 }
        }
        _ => {
            moves.extend((by as i16).to_be_bytes());
            0
        }
    }
}

/// A point's x and y deltas, as its flag (less the on-curve bit) says they are written: a byte
/// or more from `moves`.
fn moved(flag: u8, moves: &mut Stream) -> Option<(i32, i32)> {
    let flag = flag as i32;
    let bytes = moves.take([1, 2, 3, 4][(flag >= 84) as usize + (flag >= 120) as usize + (flag >= 124) as usize])?;
    let b = |n: usize| bytes[n] as i32;
    // The flag's lowest bit gives x's sign, the next y's.
    let sign = |bit: i32, value: i32| if bit & 1 == 1 { value } else { -value };
    Some(match flag {
        0..10 => (0, sign(flag, ((flag & 14) << 7) + b(0))),
        10..20 => (sign(flag, (((flag - 10) & 14) << 7) + b(0)), 0),
        20..84 => (sign(flag, 1 + ((flag - 20) & 0x30) + (b(0) >> 4)), sign(flag >> 1, 1 + (((flag - 20) & 0x0c) << 2) + (b(0) & 15))),
        84..120 => (sign(flag, 1 + (((flag - 84) / 12) << 8) + b(0)), sign(flag >> 1, 1 + ((((flag - 84) % 12) >> 2) << 8) + b(1))),
        120..124 => (sign(flag, (b(0) << 4) + (b(1) >> 4)), sign(flag >> 1, ((b(1) & 15) << 8) + b(2))),
        _ => (sign(flag, (b(0) << 8) + b(1)), sign(flag >> 1, (b(2) << 8) + b(3))),
    })
}

/// Rebuilds `hmtx` from the first `listed` glyphs' advances and, where the file omitted them,
/// left side bearings taken as each glyph's xMin.
fn widths(table: &[u8], listed: usize, lefts: &[i16]) -> Option<Vec<u8>> {
    let mut stream = Stream { bytes: table, at: 0 };
    let flags = stream.u8()?;
    let count = lefts.len();
    (flags & 0xfc == 0 && flags & 3 != 0 && listed >= 1 && listed <= count).then_some(())?;
    let advances = (0..listed).map(|_| stream.u16()).collect::<Option<Vec<_>>>()?;
    let mut table = Vec::with_capacity(count * 2 + listed * 2);
    for (n, left) in lefts.iter().enumerate() {
        let given = if n < listed { flags & 1 == 0 } else { flags & 2 == 0 };
        let left = if given { stream.u16()? } else { *left as u16 };
        if n < listed {
            table.extend(advances[n].to_be_bytes());
        }
        table.extend(left.to_be_bytes());
    }
    Some(table)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::font_file::{FontFile, Placing};

    /// Each font's tables in a font file (one or a collection), each with its bytes.
    pub(crate) fn tables_of(file: &[u8]) -> Vec<Vec<([u8; 4], &[u8])>> {
        let all = Bytes(file);
        let starts: Vec<usize> = match all.u32(0) {
            Some(0x7474_6366) => (0..all.u32(8).unwrap() as usize).map(|n| all.u32(12 + n * 4).unwrap() as usize).collect(),
            _ => vec![0],
        };
        let table = |entry: usize| {
            let (at, long) = (all.u32(entry + 8).unwrap() as usize, all.u32(entry + 12).unwrap() as usize);
            (file[entry..entry + 4].try_into().unwrap(), &file[at..at + long])
        };
        starts.iter().map(|start| (0..all.u16(start + 4).unwrap() as usize).map(|n| table(start + 12 + n * 16)).collect()).collect()
    }

    /// Holds what this makes of a WOFF2 file against the reference decoder's result: every
    /// table kept here equals the reference's byte for byte (except `head`'s checksum
    /// adjustment, not kept here), and none read is missing. Says how many tables were
    /// compared.
    pub(crate) fn as_the_reference(name: &str, ours: &[u8], theirs: &[u8]) -> usize {
        let (ours, theirs) = (tables_of(ours), tables_of(theirs));
        assert_eq!(ours.len(), theirs.len(), "{name}: fonts");
        let mut held = 0;
        for (n, (ours, theirs)) in ours.iter().zip(&theirs).enumerate() {
            let wanted: Vec<&[u8; 4]> = taken(|tag| theirs.iter().any(|(known, _)| known == tag)).collect();
            assert_eq!(ours.iter().map(|(tag, _)| tag).collect::<Vec<_>>(), wanted, "{name} #{n}: the tables kept");
            for (tag, bytes) in ours {
                let reference = theirs.iter().find(|(known, _)| known == tag).unwrap().1;
                let summed = |bytes: &[u8]| [&bytes[..8.min(bytes.len())], &bytes[12.min(bytes.len())..]].concat();
                match tag {
                    b"head" => assert!(summed(bytes) == summed(reference), "{name} #{n}: head"),
                    _ => {
                        let first = bytes.iter().zip(reference).position(|(one, other)| one != other);
                        let near = first.map(|at| (&bytes[at.saturating_sub(8)..(at + 8).min(bytes.len())], &reference[at.saturating_sub(8)..(at + 8).min(reference.len())]));
                        assert!(*bytes == reference, "{name} #{n}: {} ({} bytes, the reference's {}), first at {first:?}: {near:?}", String::from_utf8_lossy(tag), bytes.len(), reference.len());
                    }
                }
                held += 1;
            }
        }
        held
    }

    /// Holds a font unpacked here against the font it was packed from: every glyph's outline
    /// and advance, in every font of a collection. Says how many glyphs were compared.
    pub(crate) fn as_the_twin(name: &str, ours: &[u8], twin: &[u8]) -> usize {
        let mut held = 0;
        for nth in 0.. {
            let (Some(ours), Some(twin)) = (FontFile::read(ours.to_vec()).and_then(|font| font.nth(nth)), FontFile::read(twin.to_vec()).and_then(|font| font.nth(nth))) else { break };
            assert_eq!(ours.glyph_count(), twin.glyph_count(), "{name} #{nth}");
            for glyph in 0..ours.glyph_count() {
                let [shape, same] = [&ours, &twin].map(|font| (font.sketch(glyph, Placing::sized(1.0, 0.0)).path, font.width(glyph)));
                assert!(shape == same, "{name} #{nth}: glyph {glyph}");
            }
            held += ours.glyph_count() as usize;
        }
        held
    }

    /// The files in the folders `TAUT_WOFF2` names (separated by `;`): each `x.woff2` with the
    /// reference decoder's `x.ref.ttf`, and if present the original `x.ttf` or `x.ttc`.
    #[test]
    fn fonts_packed_for_the_web_unpack_as_the_reference_decoder_unpacks_them() {
        let Ok(folders) = std::env::var("TAUT_WOFF2") else { return eprintln!("TAUT_WOFF2 names no folders: not checked") };
        let (mut files, mut tables, mut glyphs, mut twins) = (0, 0, 0, 0);
        for folder in folders.split(';').filter(|folder| !folder.is_empty()) {
            let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(folder).unwrap().map(|entry| entry.unwrap().path()).collect();
            names.sort();
            for packed in names.iter().filter(|path| path.extension().is_some_and(|ending| ending == "woff2")) {
                let name = packed.file_name().unwrap().to_string_lossy().to_string();
                let with = |ending: &str| std::fs::read(packed.with_extension(ending)).ok();
                let (reference, twin) = (with("ref.ttf").unwrap_or_else(|| panic!("{name}: no reference")), with("ttf").or_else(|| with("ttc")));
                let started = std::time::Instant::now();
                let ours = unpacked(&std::fs::read(packed).unwrap()).unwrap_or_else(|| panic!("{name} is not read"));
                let took = started.elapsed();
                tables += as_the_reference(&name, &ours, &reference);
                glyphs += as_the_reference_glyphs(&name, &ours, &reference);
                if let Some(twin) = twin {
                    as_the_twin(&name, &ours, &twin);
                    twins += 1;
                }
                files += 1;
                eprintln!("{name}: {} bytes, {:.2} ms", ours.len(), took.as_secs_f64() * 1000.0);
            }
        }
        eprintln!("{files} files ({twins} with their twins): {tables} tables as the reference's, {glyphs} glyphs drawn alike");
    }

    /// The library's test font (simple and composite glyphs, one composed of itself), packed by
    /// the reference encoder (woff2_compress 1.0.2): `glyf` and `loca` transformed, all of it
    /// Brotli-packed.
    #[rustfmt::skip]
    const PACKED: [u8; 300] = [
        0x77, 0x4f, 0x46, 0x32, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x2c, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x02, 0xac,
        0x00, 0x00, 0x00, 0xe5, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46, 0x0a, 0x82, 0x00, 0x82, 0x11, 0x0b, 0x16, 0x00, 0x01, 0x36,
        0x02, 0x24, 0x03, 0x22, 0x13, 0x1e, 0x04, 0x20, 0x1b, 0x10, 0x02, 0xf8, 0x8f, 0xc3, 0x38, 0x86, 0xfc, 0x10, 0x93, 0xa5,
        0x34, 0xb7, 0x78, 0x88, 0xd6, 0x5e, 0xdf, 0xec, 0x5d, 0xf0, 0x12, 0x17, 0xc5, 0x1f, 0x8c, 0x0b, 0x4e, 0x08, 0x74, 0x2c,
        0x29, 0xd3, 0x2d, 0xa9, 0x6f, 0x45, 0xcf, 0xd5, 0x88, 0x3a, 0x16, 0xb2, 0x72, 0x1b, 0xe0, 0xe1, 0xb4, 0xbe, 0x4e, 0xce,
        0xb3, 0xb6, 0xc3, 0x21, 0xc0, 0x50, 0x22, 0x4f, 0x08, 0xf2, 0x3c, 0xb3, 0xc8, 0x9e, 0x00, 0x20, 0x30, 0xcd, 0x29, 0x42,
        0x27, 0xb0, 0xa1, 0x77, 0x88, 0x55, 0xab, 0x0a, 0x26, 0x2e, 0xdd, 0xb5, 0x86, 0x89, 0x73, 0x57, 0xad, 0xb5, 0xdf, 0x6d,
        0x14, 0x02, 0xf4, 0xcc, 0xf7, 0xad, 0x82, 0x75, 0xb0, 0x85, 0x15, 0xec, 0x62, 0x86, 0x3b, 0x3c, 0x81, 0x50, 0x74, 0xad,
        0xd1, 0x4a, 0xd1, 0x5a, 0x3f, 0x1c, 0xba, 0xa1, 0x44, 0xb7, 0x59, 0x9e, 0x87, 0x58, 0x8f, 0xaf, 0xf5, 0x3f, 0x3f, 0x2e,
        0xea, 0x22, 0x16, 0x86, 0x18, 0xe4, 0x88, 0x54, 0xb6, 0x0f, 0xd5, 0xc4, 0xb7, 0xb3, 0xea, 0xaa, 0x1c, 0x98, 0x89, 0x4c,
        0x83, 0x52, 0x30, 0xe1, 0x5d, 0xc7, 0x44, 0xf2, 0x99, 0x42, 0x92, 0x46, 0x28, 0x96, 0x59, 0x1d, 0x50, 0xe5, 0x44, 0x99,
        0x5b, 0x35, 0xa2, 0xc0, 0xff, 0x11, 0x09, 0x52, 0x9d, 0x80, 0x57, 0xa8, 0x90, 0x3e, 0x0d, 0xe0, 0xc7, 0xbe, 0x6f, 0xbf,
        0x74, 0xfc, 0x55, 0x3d, 0x8d, 0x41, 0x97, 0x2d, 0xc9, 0x6c, 0x25, 0x52, 0xc6, 0x48, 0x49, 0x4a, 0x95, 0x81, 0x52, 0x4d,
        0xa2, 0xe2, 0x55, 0x76, 0xff, 0x10, 0xd8, 0x16, 0x0a, 0x20, 0x94, 0x56, 0x15, 0x21, 0x05, 0xd6, 0x01, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn a_font_packed_the_newer_way_is_the_font_it_was() {
        let whole = crate::font_file::tests::file(&[0, 1, 0, 0], &crate::font_file::tests::tables());
        assert_eq!(as_the_twin("test font", &unpacked(&PACKED).expect("the font is unpacked"), &whole), 10);
        // Without the feature `woff2`, such a file is no font to `FontFile::read`.
        if !cfg!(feature = "woff2") {
            return assert!(FontFile::read(PACKED.to_vec()).is_none());
        }
        let (font, same) = (FontFile::read(PACKED.to_vec()).expect("the font is read"), FontFile::read(whole).unwrap());
        for letter in " ABCDEFGH@".chars() {
            assert_eq!((same.outline(letter, 64.0), same.advance(letter, 64.0)), (font.outline(letter, 64.0), font.advance(letter, 64.0)), "{letter:?}");
            assert_eq!(same.kerning('A', letter, 1024.0), font.kerning('A', letter, 1024.0), "{letter:?}");
        }
        // A file stating tables over 64 times its size: nothing, before anything is unpacked.
        // (The first table, `cmap`, said to be some 33 MB.)
        let mut claims = [&PACKED[..49], &[0x8f, 0xff, 0xff, 0x7f], &PACKED[50..]].concat();
        let long = claims.len() as u32;
        claims[8..12].copy_from_slice(&long.to_be_bytes());
        assert_eq!(unpacked(&claims), None);
    }

    #[test]
    fn a_damaged_font_packed_the_newer_way_never_stops_the_program() {
        // Truncated anywhere, or with a bit of any byte flipped (another bit each byte): every
        // third place, each unpacking the dictionary afresh.
        (0..PACKED.len()).step_by(3).for_each(|long| crate::font_file::tests::ask_everything(PACKED[..long].to_vec()));
        for at in (1..PACKED.len()).step_by(3) {
            let mut damaged = PACKED.to_vec();
            damaged[at] ^= 1 << (at % 8);
            crate::font_file::tests::ask_everything(damaged);
        }
    }

    /// Every glyph unpacked here draws as in the reference decoder's file.
    fn as_the_reference_glyphs(name: &str, ours: &[u8], reference: &[u8]) -> usize {
        as_the_twin(name, ours, reference)
    }
}
