//! Glyph outlines of OpenType fonts with PostScript outlines. In the `CFF ` table each glyph is
//! a Type 2 charstring: operands, and operators that consume them (moves, lines, curves,
//! subroutine calls and returns, arithmetic). Hints are skipped. The table stores things in
//! INDEXes (offset arrays) and locates them through DICTs (operands followed by their key).
//!
//! `CFF2` is the same for variable fonts (see `variable_fonts.rs`): an operand may come with
//! deltas per variation region, and `blend` resolves them at the font's instance. Its INDEX
//! counts are 4 bytes, not 2; charstrings have no widths; glyphs have no names; there is always
//! an FDArray; and the operand stack holds 513, not 48.
use crate::font_file::{Bytes, FontFile, Placing, Sketch, first_not};
use crate::geom::{Point, pt};
use crate::variable_fonts::Regions;
use std::ops::Range;

/// A parsed `CFF ` or `CFF2` table, where each glyph is a charstring.
pub(crate) struct Programs {
    /// The table's range in the file; all offsets in it are from its start.
    at: Range<usize>,
    /// Every glyph's charstring (CharStrings INDEX).
    glyphs: List,
    /// Global subroutines,
    shared: List,
    /// and for each Private DICT, its local subroutines, and (in `CFF2`) its default `vsindex`.
    own: Vec<(List, u16)>,
    /// The FDSelect offset: only in a CID-keyed font (several Private DICTs).
    which: Option<usize>,
    /// The charset offset: only in a font with one Private DICT. (A CID-keyed font numbers its
    /// glyphs and doesn't name them.)
    names: Option<usize>,
    /// Whether it is `CFF2`, and its variation store offset from the table's start (0: none).
    newer: bool,
    store: usize,
}

impl Programs {
    /// Finds a `CFF ` table's INDEXes. After the header come four: Name, Top DICT (one font),
    /// String, and Global Subrs. The Top DICT locates everything else.
    pub fn read(all: Bytes, at: Range<usize>) -> Option<Programs> {
        let cff = all.within(&at);
        if cff.u8(0)? == 2 {
            return Programs::newer(all, at);
        }
        let names = List::read(cff, cff.u8(2)? as usize)?;
        let fonts = List::read(cff, names.end(cff)?)?;
        let words = List::read(cff, fonts.end(cff)?)?;
        let shared = List::read(cff, words.end(cff)?)?;
        let font = fonts.item(cff, 0)?;
        // Type 1 charstrings (CharstringType 1) are not read.
        if dict_operands(font, 0x0c06).is_some_and(|kind| kind[1] != 2) {
            return None;
        }
        let glyphs = List::read(cff, dict_operands(font, 17)?[1])?;
        // A Private DICT (key 18: size and offset) locates its local subroutines (key 19,
        // relative to the DICT). A font has one, or several in an FDArray (key 12 36) with an
        // FDSelect (12 37).
        let own = |dictionary: Bytes| {
            let Some([long, at]) = dict_operands(dictionary, 18) else { return Some((List::default(), 0)) };
            match dict_operands(cff.part(at, long)?, 19) {
                Some([_, pieces]) => Some((List::read(cff, at.checked_add(pieces)?)?, 0)),
                None => Some((List::default(), 0)),
            }
        };
        let Some([_, several]) = dict_operands(font, 0x0c24) else {
            // The charset is under key 15: without it, a predefined one.
            let names = dict_operands(font, 15).map_or(0, |[_, names]| names);
            return Some(Programs { at, glyphs, shared, own: vec![own(font)?], which: None, names: Some(names), newer: false, store: 0 });
        };
        let dictionaries = List::read(cff, several)?;
        let own = (0..dictionaries.count.min(256)).map(|n| own(dictionaries.item(cff, n)?)).collect::<Option<_>>()?;
        let which = dict_operands(font, 0x0c25)?[1];
        (which < cff.0.len()).then_some(Programs { at, glyphs, shared, own, which: Some(which), names: None, newer: false, store: 0 })
    }

    /// Finds a `CFF2` table's parts. After the header (whose third byte is its size, and the
    /// next two the Top DICT's length) come the Top DICT and Global Subrs. The Top DICT locates
    /// the rest, as in `CFF `, plus the variation store (key 24), which starts with its length.
    /// There is always an FDArray, and each Private DICT may set its `vsindex` (key 22).
    fn newer(all: Bytes, at: Range<usize>) -> Option<Programs> {
        let cff = all.within(&at);
        let (start, long) = (cff.u8(2)? as usize, cff.u16(3)? as usize);
        let font = cff.part(start, long)?;
        let (shared, glyphs) = (List::newer(cff, start + long)?, List::newer(cff, dict_operands(font, 17)?[1])?);
        let own = |dictionary: Bytes| {
            let Some([long, at]) = dict_operands(dictionary, 18) else { return Some((List::default(), 0)) };
            let private = cff.part(at, long)?;
            let blends = dict_operands(private, 22).map_or(0, |[_, list]| list.min(u16::MAX as usize) as u16);
            match dict_operands(private, 19) {
                Some([_, pieces]) => Some((List::newer(cff, at.checked_add(pieces)?)?, blends)),
                None => Some((List::default(), blends)),
            }
        };
        let dictionaries = List::newer(cff, dict_operands(font, 0x0c24)?[1])?;
        let own = (0..dictionaries.count.min(256)).map(|n| own(dictionaries.item(cff, n)?)).collect::<Option<_>>()?;
        let which = dict_operands(font, 0x0c25).map(|[_, which]| which).filter(|which| *which < cff.0.len());
        let store = dict_operands(font, 24).map_or(0, |[_, store]| store.saturating_add(2));
        Some(Programs { at, glyphs, shared, own, which, names: None, newer: true, store })
    }

    /// The Private DICT a glyph uses. FDSelect format 0 is a byte per glyph; format 3 is ranges
    /// (first glyph in 2 bytes, FD in 1); format 4 (`CFF2`) the same in 4 and 2.
    fn dictionary(&self, cff: Bytes, glyph: usize) -> Option<usize> {
        let Some(at) = self.which else { return Some(0) };
        let stretch = |n: usize| at + 3 + n * 3;
        match cff.u8(at)? {
            0 => cff.u8(at + 1 + glyph).map(usize::from),
            3 => {
                let after = first_not(cff.u16(at + 1)? as usize, |n| Some(cff.u16(stretch(n))? as usize <= glyph))?;
                cff.u8(stretch(after.checked_sub(1)?) + 2).map(usize::from)
            }
            4 => {
                let stretch = |n: usize| at + 5 + n * 6;
                let after = first_not(cff.u32(at + 1)? as usize, |n| Some(cff.u32(stretch(n))? as usize <= glyph))?;
                cff.u16(stretch(after.checked_sub(1)?) + 4).map(usize::from)
            }
            _ => None,
        }
    }

    /// Draws a glyph into a sketch by running its charstring. It may end with `endchar`'s seac
    /// operands, asking for a base letter and an accent moved onto it: how older fonts built
    /// accented letters. Both are named by Standard Encoding codes, and may not do the same in
    /// turn.
    pub fn draw(&self, font: &FontFile, glyph: u16, place: Placing, sketch: &mut Sketch) -> Option<()> {
        let (cff, axes) = (font.bytes().within(&self.at), font.varied.as_ref().map_or(&[][..], |varied| &varied.place[..]));
        let Some([right, up, letter, accent]) = self.run(cff, glyph, (place, axes), sketch)? else { return Some(()) };
        for (character, place) in [(letter, place), (accent, place.after(Placing([1.0, 0.0, 0.0, 1.0, right, up])))] {
            let asked = self.run(cff, self.called(cff, character)?, (place, axes), sketch)?;
            asked.is_none().then_some(())?;
        }
        Some(())
    }

    /// Runs one glyph's charstring for a font instance at `axes` (see `variable_fonts.rs`).
    /// Returns the four seac operands, if it had them.
    fn run(&self, cff: Bytes, glyph: u16, (place, axes): (Placing, &[i32]), sketch: &mut Sketch) -> Option<Option<[f32; 4]>> {
        let (own, blends) = *self.own.get(self.dictionary(cff, glyph as usize)?)?;
        let code = self.glyphs.item(cff, glyph as usize)?;
        // The operand stack holds 48; in `CFF2`, 513.
        let (callers, mut numbers, transient) = ([(code, 0); 16], [0.0; 513], [0.0; 32]);
        let numbers = &mut numbers[..if self.newer { 513 } else { 48 }];
        let store = Bytes(cff.0.get(self.store..).filter(|_| self.store != 0).unwrap_or(&[]));
        let blending = Blending { newer: self.newer, store, list: blends, axes };
        let mut run =
            Run { cff, lists: [own, self.shared], code, at: 0, callers, depth: 0, numbers, count: 0, transient, hints: 0, pen: pt(0.0, 0.0), drawing: false, asked: None, place, sketch, blending };
        while run.step()? {}
        run.lift();
        Some(run.asked)
    }

    /// The glyph of a Standard Encoding code. There characters have SIDs (an index into the
    /// standard strings every `CFF ` table knows), and the charset names every glyph but the
    /// first.
    fn called(&self, cff: Bytes, character: f32) -> Option<u16> {
        let (character, at, count) = (u8::try_from(character as i32).ok()?, self.names?, self.glyphs.count.min(65535));
        let name = NAMED.iter().find(|(first, _, many)| (*first..first + many).contains(&character)).map(|(first, name, _)| (name + (character - first)) as usize)?;
        let glyph = match (at, cff.u8(at)?) {
            // Predefined charsets: ISOAdobe, where each glyph's SID is its index, and Expert
            // and ExpertSubset, which have no such letters.
            (0, _) => name,
            (1 | 2, _) => return None,
            // Format 0: a SID per glyph, 2 bytes; formats 1 and 2: ranges of consecutive SIDs,
            // the first SID and a 1- or 2-byte count of more.
            (_, 0) => 1 + (0..count.saturating_sub(1)).position(|n| cff.u16(at + 1 + n * 2) == Some(name as u16))?,
            (_, kind @ 1..=2) => stretched(cff, at + 1, kind as usize + 2, count, name)?,
            _ => return None,
        };
        (glyph < count).then_some(glyph as u16)
    }
}

/// The SIDs of Standard Encoding codes, in ranges of consecutive codes with consecutive SIDs:
/// first code, its SID, and count.
const NAMED: [(u8, u8, u8); 14] = [
    (32, 1, 95),
    (161, 96, 15),
    (177, 111, 4),
    (182, 115, 8),
    (191, 123, 1),
    (193, 124, 8),
    (202, 132, 2),
    (205, 134, 4),
    (225, 138, 1),
    (227, 139, 1),
    (232, 140, 4),
    (241, 144, 1),
    (245, 145, 1),
    (248, 146, 4),
];

/// The glyph with SID `name`, in a charset of ranges `long` bytes each from `at`. (Glyph 0 has
/// no name; ranges continue until every glyph has one.)
fn stretched(cff: Bytes, mut at: usize, long: usize, count: usize, name: usize) -> Option<usize> {
    let mut glyph = 1;
    while glyph < count {
        let (first, more) = (cff.u16(at)? as usize, if long == 3 { cff.u8(at + 2)? as usize } else { cff.u16(at + 2)? as usize });
        if (first..=first + more).contains(&name) {
            return Some(glyph + name - first);
        }
        (at, glyph) = (at + long, glyph + more + 1);
    }
    None
}

/// A `CFF ` INDEX: a count, then offsets (from one byte before the data, 1 to 4 bytes each),
/// then the data.
#[derive(Clone, Copy, Default)]
struct List {
    count: usize,
    /// Where the offsets start in the table, and their size.
    at: usize,
    wide: usize,
}

impl List {
    /// The INDEX at `at`.
    fn read(cff: Bytes, at: usize) -> Option<List> {
        let count = cff.u16(at)? as usize;
        // An empty INDEX is just its count.
        if count == 0 {
            return Some(List { count, at: at + 2, wide: 0 });
        }
        Some(List { count, at: at + 3, wide: cff.u8(at + 2).filter(|wide| (1..=4).contains(wide))? as usize })
    }
    /// The `CFF2` INDEX at `at`, whose count is 4 bytes. (No INDEX holds more items than its
    /// table has bytes.)
    fn newer(cff: Bytes, at: usize) -> Option<List> {
        let count = cff.u32(at).map(|count| count as usize).filter(|count| *count <= cff.0.len())?;
        if count == 0 {
            return Some(List { count, at: at + 4, wide: 0 });
        }
        Some(List { count, at: at + 5, wide: cff.u8(at + 4).filter(|wide| (1..=4).contains(wide))? as usize })
    }
    /// Item `n`'s start in the table; for `n` the count, the end of the last.
    fn start(&self, cff: Bytes, n: usize) -> Option<usize> {
        let number = cff.part(self.at + n * self.wide, self.wide)?.0.iter().fold(0, |number, byte| number << 8 | *byte as usize);
        (self.at + (self.count + 1) * self.wide).checked_add(number)?.checked_sub(1)
    }
    /// Item `n`.
    fn item<'a>(&self, cff: Bytes<'a>, n: usize) -> Option<Bytes<'a>> {
        if n >= self.count {
            return None;
        }
        cff.0.get(self.start(cff, n)?..self.start(cff, n + 1)?).map(Bytes)
    }
    /// The INDEX's end in the table: where what follows starts.
    fn end(&self, cff: Bytes) -> Option<usize> {
        if self.count == 0 { Some(self.at) } else { self.start(cff, self.count) }
    }
    /// The subroutine bias: charstrings number subroutines from the middle, so numbers stay
    /// small and encode short.
    fn middle(&self) -> i32 {
        match self.count {
            ..1240 => 107,
            1240..33900 => 1131,
            _ => 32768,
        }
    }
}

/// A DICT's last two operands before `key`. Operands precede their operator; an operator is a
/// byte below 28, or 12 and one more byte (asked for here as `0x0c00` plus that byte).
fn dict_operands(dictionary: Bytes, key: u16) -> Option<[usize; 2]> {
    let (mut at, mut numbers) = (0, [0; 2]);
    while let Some(first) = dictionary.u8(at) {
        let second = dictionary.u8(at + 1);
        let (number, long) = match first {
            12 => (None, 2),
            0..=27 => (None, 1),
            28 => (Some(dictionary.i16(at + 1)? as i32), 3),
            29 => (Some(dictionary.u32(at + 1)? as i32), 5),
            // A real number: nibble digits up to a nibble of all ones. Nothing asked for here
            // is real.
            30 => (Some(0), 2 + dictionary.0.get(at + 1..)?.iter().position(|byte| byte & 15 == 15 || byte >> 4 == 15)?),
            _ => (Some(small(first, second)?), if first > 246 { 2 } else { 1 }),
        };
        let word = if first == 12 { 0x0c00 | second? as u16 } else { first as u16 };
        if number.is_none() && word == key {
            return Some([usize::try_from(numbers[0]).ok()?, usize::try_from(numbers[1]).ok()?]);
        }
        numbers = number.map_or([0; 2], |number| [numbers[1], number]);
        at += long;
    }
    None
}

/// A small integer as DICTs and charstrings encode it: in its first byte, or that and the next.
fn small(first: u8, second: Option<u8>) -> Option<i32> {
    match first {
        32..=246 => Some(first as i32 - 139),
        247..=250 => Some((first as i32 - 247) * 256 + second? as i32 + 108),
        251..=254 => Some(-(first as i32 - 251) * 256 - second? as i32 - 108),
        _ => None,
    }
}

/// A charstring being run: operands, and operators that consume them (moves, lines, curves,
/// subroutine calls and returns; and hints, which are skipped).
struct Run<'a> {
    cff: Bytes<'a>,
    /// The callable subroutines: local and global.
    lists: [List; 2],
    /// The charstring being read, and the position in it.
    code: Bytes<'a>,
    at: usize,
    /// The return stack: 16 deep at most. (The limit is 10, but fonts in use go deeper.)
    callers: [(Bytes<'a>, usize); 16],
    depth: usize,
    /// The operands since the last operator, and the transient array.
    numbers: &'a mut [f32],
    count: usize,
    transient: [f32; 32],
    /// Stem hints declared: a `hintmask` that follows has a bit for each.
    hints: usize,
    /// The pen position in font units, and whether it has drawn since it moved.
    pen: Point,
    drawing: bool,
    /// The four seac operands before `endchar`, if any: a base letter and an accent.
    asked: Option<[f32; 4]>,
    place: Placing,
    sketch: &'a mut Sketch,
    blending: Blending<'a>,
}

/// What `blend` uses in a `CFF2` charstring: the variation store, the `vsindex` (the charstring
/// may set another), and the instance's axis coordinates (none: at the default, where an
/// operand is its default value).
struct Blending<'a> {
    newer: bool,
    store: Bytes<'a>,
    list: u16,
    axes: &'a [i32],
}

impl Run<'_> {
    /// Reads and runs the next operand or operator. `false` once the glyph is done.
    fn step(&mut self) -> Option<bool> {
        self.sketch.spend(1)?;
        // A subroutine ending without `return` returns; the glyph's own charstring ends.
        let Some(first) = self.code.u8(self.at) else { return Some(self.back().is_some()) };
        self.at += 1;
        match first {
            28 => self.number(self.code.i16(self.at)? as f32, 2),
            255 => self.number(self.code.u32(self.at)? as i32 as f32 / 65536.0, 4),
            32..=254 => self.number(small(first, self.code.u8(self.at))? as f32, if first > 246 { 1 } else { 0 }),
            10 => self.call(self.lists[0]),
            29 => self.call(self.lists[1]),
            11 => self.back().map(|()| true),
            12 => self.second().map(|()| true),
            14 => self.end(),
            15 if self.blending.newer => self.choose().map(|()| true),
            16 if self.blending.newer => self.blend().map(|()| true),
            _ => self.command(first).map(|()| true),
        }
    }

    /// Pushes an operand. It took `long` bytes after its first.
    fn number(&mut self, number: f32, long: usize) -> Option<bool> {
        *self.numbers.get_mut(self.count)? = number;
        (self.count, self.at) = (self.count + 1, self.at + long);
        Some(true)
    }

    /// Calls a subroutine from an INDEX: the one the last operand names.
    fn call(&mut self, list: List) -> Option<bool> {
        self.count = self.count.checked_sub(1)?;
        let piece = list.item(self.cff, usize::try_from(self.numbers[self.count] as i32 + list.middle()).ok()?)?;
        *self.callers.get_mut(self.depth)? = (self.code, self.at);
        (self.code, self.at, self.depth) = (piece, 0, self.depth + 1);
        Some(true)
    }

    /// Returns to the calling charstring. `None` if there is none.
    fn back(&mut self) -> Option<()> {
        self.depth = self.depth.checked_sub(1)?;
        (self.code, self.at) = self.callers[self.depth];
        Some(())
    }

    /// `endchar`. (A width may precede it, or a width and four seac operands.)
    fn end(&mut self) -> Option<bool> {
        self.asked = self.numbers[..self.count].last_chunk().copied();
        Some(false)
    }

    /// Runs a two-byte operator. Four are the flex variants. The rest are arithmetic on the
    /// last operands: they pop some and push the result, for later operators. (Fonts
    /// interpolated from several weights of one family compute their outlines this way.)
    fn second(&mut self) -> Option<()> {
        let kind = self.code.u8(self.at)?;
        self.at += 1;
        if (34..=37).contains(&kind) {
            let (numbers, count) = (std::mem::take(&mut self.numbers), std::mem::take(&mut self.count));
            self.flex(kind, &numbers[..count])?;
            self.numbers = numbers;
            return Some(());
        }
        // How many it pops. (One pops all and does nothing: `dotsection`, which older fonts put
        // around the dots of letters.)
        let takes = match kind {
            0 => self.count,
            23 => 0,
            5 | 9 | 14 | 18 | 21 | 26 | 27 | 29 => 1,
            3 | 4 | 10 | 11 | 12 | 15 | 20 | 24 | 28 | 30 => 2,
            22 => 4,
            _ => return None,
        };
        self.count = self.count.checked_sub(takes)?;
        let taken: [f32; 4] = std::array::from_fn(|n| self.numbers[(self.count + n).min(self.numbers.len() - 1)]);
        let ([a, b, ..], whole) = (taken, |number: f32| usize::try_from(number as i32).ok());
        // `put` stores a value at an index;
        if kind == 20 {
            *self.transient.get_mut(whole(b)?)? = a;
        }
        // `roll` rotates the last so many operands by so many places.
        if kind == 30 {
            let many = (a as i32).max(1);
            self.numbers.get_mut(self.count.checked_sub(many as usize)?..self.count)?.rotate_right((b as i32).rem_euclid(many) as usize);
        }
        let (made, many) = match kind {
            0 | 18 | 20 | 30 => ([0.0; 2], 0),
            // `get` fetches a stored value; `dup` repeats the last; `exch` swaps the last two;
            // `index` copies one further down (the last, for a negative index).
            21 => ([*self.transient.get(whole(a)?)?, 0.0], 1),
            27 => ([a, a], 2),
            28 => ([b, a], 2),
            29 => ([*self.numbers[..self.count].iter().rev().nth((a as i32).max(0) as usize)?, 0.0], 1),
            _ => ([summed(kind, taken)?, 0.0], 1),
        };
        for number in &made[..many] {
            *self.numbers.get_mut(self.count).filter(|_| number.is_finite())? = *number;
            self.count += 1;
        }
        Some(())
    }

    /// Runs an operator, which consumes the operands before it. (They are taken out while it
    /// draws, then put back.)
    fn command(&mut self, command: u8) -> Option<()> {
        let (numbers, count) = (std::mem::take(&mut self.numbers), std::mem::take(&mut self.count));
        let operands = &numbers[..count];
        match command {
            // Hints come in pairs of operands. A mask has a bit per hint so far, and may have
            // the last hints right before it.
            1 | 3 | 18 | 23 => self.hints += count / 2,
            19 | 20 => (self.hints, self.at) = (self.hints + count / 2, self.at + (self.hints + count / 2).div_ceil(8)),
            // `rmoveto`, `hmoveto`, `vmoveto`. (A glyph's first move may be preceded by a
            // width, which isn't needed.)
            21 => self.go(*operands.get(count.wrapping_sub(2))?, *operands.last()?),
            22 => self.go(*operands.last()?, 0.0),
            4 => self.go(0.0, *operands.last()?),
            // `rlineto`; or `hlineto`/`vlineto`, alternating horizontal and vertical.
            5 => operands.as_chunks().0.iter().for_each(|&[dx, dy]| self.line(dx, dy)),
            6 | 7 => operands.iter().enumerate().for_each(|(n, by)| if (n % 2 == 0) == (command == 6) { self.line(*by, 0.0) } else { self.line(0.0, *by) }),
            // `rrcurveto`, six operands each: with a line after them (`rcurveline`), or lines
            // before one (`rlinecurve`).
            8 => operands.as_chunks().0.iter().for_each(|by| self.curve(*by)),
            24 => {
                let (curves, line) = operands.as_chunks();
                curves.iter().for_each(|by| self.curve(*by));
                self.line(*line.first()?, *line.get(1)?);
            }
            25 => {
                let (lines, curve) = operands.split_at(count.checked_sub(6)?);
                lines.as_chunks().0.iter().for_each(|&[dx, dy]| self.line(dx, dy));
                self.curve(curve.try_into().ok()?);
            }
            // Shorter curves, for ones starting or ending horizontal or vertical.
            26 | 27 => self.leaning(operands, command == 27),
            30 | 31 => self.turning(operands, command == 31),
            _ => return None,
        }
        self.numbers = numbers;
        Some(())
    }

    /// `vsindex`, in a `CFF2` charstring: the variation data the following blends use.
    fn choose(&mut self) -> Option<()> {
        self.count = self.count.checked_sub(1)?;
        self.blending.list = u16::try_from(self.numbers[self.count] as i32).ok()?;
        Some(())
    }

    /// `blend`, in a `CFF2` charstring. The last operand is how many operands are blended.
    /// Before it are their default values, then for each, its deltas per region (see
    /// `variable_fonts.rs`). What remains is each operand at the font's instance.
    fn blend(&mut self) -> Option<()> {
        self.count = self.count.checked_sub(1)?;
        let many = usize::try_from(self.numbers[self.count] as i32).ok()?;
        let regions = Regions::of(self.blending.store, self.blending.list)?;
        let first = self.count.checked_sub(many.checked_mul(regions.count + 1)?)?;
        self.sketch.spend(regions.count + many)?;
        if !self.blending.axes.is_empty() {
            let says: Vec<f32> = (0..regions.count).map(|n| regions.say(n, self.blending.axes)).collect();
            for n in 0..many {
                let moves = self.numbers.get(first + many + n * regions.count..)?.iter().zip(&says);
                self.numbers[first + n] += moves.map(|(by, say)| by * say).sum::<f32>();
            }
        }
        self.count = first + many;
        Some(())
    }

    /// Lifts the pen: the current contour is closed.
    fn lift(&mut self) {
        if std::mem::take(&mut self.drawing) {
            self.sketch.path.close();
        }
    }

    /// Lifts the pen and moves it.
    fn go(&mut self, dx: f32, dy: f32) {
        self.lift();
        self.pen = pt(self.pen.x + dx, self.pen.y + dy);
    }

    /// Puts the pen down where it is, if not drawing yet, and returns the point.
    fn down(&mut self) -> Point {
        if !std::mem::replace(&mut self.drawing, true) {
            self.sketch.move_to(self.place.of(self.pen));
        }
        self.pen
    }

    /// A line from the pen, `dx` right and `dy` up.
    fn line(&mut self, dx: f32, dy: f32) {
        let from = self.down();
        self.pen = pt(from.x + dx, from.y + dy);
        self.sketch.line_to(self.place.of(self.pen));
    }

    /// A curve: from the pen to the first control point, from there to the second, and from
    /// there to the end.
    fn curve(&mut self, by: [f32; 6]) {
        let from = self.down();
        let a = pt(from.x + by[0], from.y + by[1]);
        let b = pt(a.x + by[2], a.y + by[3]);
        self.pen = pt(b.x + by[4], b.y + by[5]);
        self.sketch.cubic_to(self.place.of(a), self.place.of(b), self.place.of(self.pen));
    }

    /// Curves starting and ending in the same direction, `across` or up: four operands each.
    /// The first may start off-axis, by an extra operand before the others.
    fn leaning(&mut self, operands: &[f32], across: bool) {
        let (aside, curves) = operands.as_rchunks::<4>();
        let mut aside = aside.first().copied().unwrap_or(0.0);
        for by in curves {
            self.curve(if across { [by[0], aside, by[1], by[2], by[3], 0.0] } else { [aside, by[0], by[1], by[2], 0.0, by[3]] });
            aside = 0.0;
        }
    }

    /// Curves alternately starting `across` and ending vertical, and the reverse: four operands
    /// each. The last may end off-axis, by an extra operand after the others.
    fn turning(&mut self, operands: &[f32], mut across: bool) {
        let (curves, aside) = operands.as_chunks::<4>();
        for (n, by) in curves.iter().enumerate() {
            let aside = if n + 1 == curves.len() { aside.first().copied().unwrap_or(0.0) } else { 0.0 };
            self.curve(if across { [by[0], 0.0, by[1], by[2], aside, by[3]] } else { [0.0, by[0], by[1], by[2], by[3], aside] });
            across = !across;
        }
    }

    /// Two curves in one operator, in four encodings omitting zeros. (Meant to flatten to a
    /// line at small sizes; here always curves.)
    fn flex(&mut self, kind: u8, operands: &[f32]) -> Option<()> {
        let (first, second) = match (kind, operands) {
            (34, &[.., a, b, c, d, e, f, g]) => ([a, 0.0, b, c, d, 0.0], [e, 0.0, f, -c, g, 0.0]),
            (35, &[.., a, b, c, d, e, f, g, h, i, j, k, l, _]) => ([a, b, c, d, e, f], [g, h, i, j, k, l]),
            (36, &[.., a, b, c, d, e, f, g, h, i]) => ([a, b, c, d, e, 0.0], [f, 0.0, g, h, i, -(b + d + h)]),
            // The last operand is the end offset along whichever axis it went further; on the
            // other it ends where it began.
            (37, &[.., a, b, c, d, e, f, g, h, i, j, k]) if (a + c + e + g + i).abs() > (b + d + f + h + j).abs() => ([a, b, c, d, e, f], [g, h, i, j, k, -(b + d + f + h + j)]),
            (37, &[.., a, b, c, d, e, f, g, h, i, j, k]) => ([a, b, c, d, e, f], [g, h, i, j, -(a + c + e + g + i), k]),
            _ => return None,
        };
        self.curve(first);
        self.curve(second);
        Some(())
    }
}

/// The single result of a `CFF ` arithmetic operator on the operands it pops.
fn summed(kind: u8, [a, b, c, d]: [f32; 4]) -> Option<f32> {
    let truth = |so: bool| so as u8 as f32;
    Some(match kind {
        // `and`, `or`, `not`, `eq`: 1 for true, 0 for false.
        3 => truth(a != 0.0 && b != 0.0),
        4 => truth(a != 0.0 || b != 0.0),
        5 => truth(a == 0.0),
        15 => truth(a == b),
        9 => a.abs(),
        10 => a + b,
        11 => a - b,
        12 => a / b,
        14 => -a,
        24 => a * b,
        26 => a.sqrt(),
        // `ifelse`: the first of four if the third is no more than the fourth; else the second.
        22 => [b, a][(c <= d) as usize],
        // `random`, "above 0 and up to 1": always 1 here, as in HarfBuzz, so a glyph draws the
        // same every time.
        23 => 1.0,
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    // What only the tests that read such fonts use is unused without the `cff` feature, which
    // they need.
    #![cfg_attr(not(feature = "cff"), allow(unused_imports, dead_code))]
    use super::*;
    use crate::canvas::Canvas;
    use crate::color::Color;
    use crate::font_file::AppFonts;
    use crate::font_file::tests::{file, ink, words};
    use crate::path::Path;
    use crate::text::{Font, Metrics, TextSystem};

    /// A `CFF ` charstring from text: numbers, operator names, and `#` before a raw byte (mask
    /// bits).
    pub(crate) fn program(text: &str) -> Vec<u8> {
        const COMMANDS: [(&str, &[u8]); 50] = [
            ("hstem", &[1]),
            ("vstem", &[3]),
            ("vmoveto", &[4]),
            ("rlineto", &[5]),
            ("hlineto", &[6]),
            ("vlineto", &[7]),
            ("rrcurveto", &[8]),
            ("callsubr", &[10]),
            ("return", &[11]),
            ("endchar", &[14]),
            ("hstemhm", &[18]),
            ("hintmask", &[19]),
            ("cntrmask", &[20]),
            ("rmoveto", &[21]),
            ("hmoveto", &[22]),
            ("vstemhm", &[23]),
            ("rcurveline", &[24]),
            ("rlinecurve", &[25]),
            ("vvcurveto", &[26]),
            ("hhcurveto", &[27]),
            ("callgsubr", &[29]),
            ("vhcurveto", &[30]),
            ("hvcurveto", &[31]),
            ("hflex", &[12, 34]),
            ("flex", &[12, 35]),
            ("hflex1", &[12, 36]),
            ("flex1", &[12, 37]),
            ("dotsection", &[12, 0]),
            ("and", &[12, 3]),
            ("or", &[12, 4]),
            ("not", &[12, 5]),
            ("abs", &[12, 9]),
            ("add", &[12, 10]),
            ("sub", &[12, 11]),
            ("div", &[12, 12]),
            ("neg", &[12, 14]),
            ("eq", &[12, 15]),
            ("drop", &[12, 18]),
            ("put", &[12, 20]),
            ("get", &[12, 21]),
            ("ifelse", &[12, 22]),
            ("random", &[12, 23]),
            ("mul", &[12, 24]),
            ("sqrt", &[12, 26]),
            ("dup", &[12, 27]),
            ("exch", &[12, 28]),
            ("index", &[12, 29]),
            ("roll", &[12, 30]),
            ("vsindex", &[15]),
            ("blend", &[16]),
        ];
        // Each number in its shortest encoding: 1, 2 or 3 bytes, or with a fraction, 5.
        let number = |number: f32| match number as i32 {
            _ if number.fract() != 0.0 => [&[255][..], &((number * 65536.0) as i32).to_be_bytes()].concat(),
            whole @ -107..=107 => vec![(whole + 139) as u8],
            whole @ 108..=1131 => vec![((whole - 108) >> 8) as u8 + 247, (whole - 108) as u8],
            whole @ -1131..=-108 => vec![((-whole - 108) >> 8) as u8 + 251, (-whole - 108) as u8],
            whole => [&[28][..], &(whole as i16).to_be_bytes()].concat(),
        };
        let word = |word: &str| match (COMMANDS.iter().find(|(name, _)| *name == word), word.strip_prefix('#')) {
            (Some((_, bytes)), _) => bytes.to_vec(),
            (None, Some(byte)) => vec![u8::from_str_radix(byte, 16).unwrap()],
            (None, None) => number(word.parse().unwrap()),
        };
        text.split_whitespace().flat_map(word).collect()
    }

    /// An INDEX as `CFF ` encodes one.
    pub(crate) fn list(things: &[Vec<u8>]) -> Vec<u8> {
        if things.is_empty() {
            return vec![0, 0];
        }
        // Each item's start and the last one's end, from 1, in 2 bytes.
        let starts: Vec<i32> = (0..=things.len()).map(|n| things[..n].concat().len() as i32 + 1).collect();
        [words(&[things.len() as i32]), vec![2], words(&starts), things.concat()].concat()
    }

    /// A `CFF ` DICT entry: operands (5 bytes each, so the DICT's length doesn't depend on
    /// them), and their key.
    pub(crate) fn says(numbers: &[usize], key: &[u8]) -> Vec<u8> {
        [numbers.iter().flat_map(|number| [&[29][..], &(*number as u32).to_be_bytes()].concat()).collect(), key.to_vec()].concat()
    }

    /// The notdef charstring of the font below: a box, in lines up and across.
    const BOX: &str = "100 0 rmoveto 500 300 -500 vlineto endchar";
    /// A width and hints to skip; the plainer lines and curves; and calls of a local subroutine
    /// (passed an operand) and a global one.
    const LINES: &str = "50 0 100 hstemhm 0 50 hintmask #80 100 200 rmoveto 100 0 0 100 rlineto -50 50 -50 hlineto 10 20 30 40 50 60 rrcurveto \
        20 -107 callsubr -107 callgsubr 1 2 3 4 5 6 7 8 rcurveline 1 1 2 2 10 0 0 10 -10 0 rlinecurve endchar";
    /// The shorter curves, numbers in every encoding, and the four flex variants.
    const CURVES: &str = "300 vmoveto 10 20 30 40 50 vvcurveto 5 60 70 80 90 hhcurveto 10 20 30 40 50 60 70 80 5 vhcurveto -100 hmoveto 10 20 30 40 hvcurveto \
        1000 -1000 -500 500 0.5 0.25 rlineto -865.5 -255.25 rmoveto 10 20 30 40 50 60 70 hflex 1 2 3 4 5 6 7 8 9 10 11 12 50 flex 1 2 3 4 5 6 7 8 9 hflex1 \
        1 2 3 4 5 6 7 8 9 10 11 flex1 10 1 10 1 10 1 10 1 10 1 5 flex1 endchar";

    /// A 1000-unit OpenType font with PostScript outlines, built byte by byte. Its glyphs are
    /// the box, and "A", "B" and "C": lines, curves, and the lines again. With `which` it has
    /// two Private DICTs and `which` as its FDSelect; the two have different local subroutines
    /// at the same index.
    pub(crate) fn curved(which: Option<Vec<u8>>) -> Vec<u8> {
        curved_by(which, [BOX, LINES, CURVES, LINES], "-10 rlineto return")
    }

    /// The same font with other charstrings for its four glyphs, and another subroutine for
    /// them to call.
    fn curved_by(which: Option<Vec<u8>>, programs: [&str; 4], piece: &str) -> Vec<u8> {
        curved_with(which, programs, piece, &[])
    }

    /// And with a charset, if `called` is one. (A single byte selects a predefined charset.)
    fn curved_with(which: Option<Vec<u8>>, programs: [&str; 4], piece: &str, called: &[u8]) -> Vec<u8> {
        let (several, which) = (which.is_some(), which.unwrap_or_default());
        let (names, shared, glyphs) = (list(&[b"T".to_vec()]), list(&[program("-20 hlineto")]), list(&programs.map(program)));
        let own = [list(&[program(piece)]), list(&[program("10 rlineto 5 5 rlineto return")])];
        // A Private DICT locates its subroutines: right after itself.
        let private = says(&[6], &[19]);
        // The Top DICT's length doesn't depend on its offsets, so they can be computed before
        // it is written.
        let top = |glyphs_at: usize, private_at: usize, which_at: usize, dictionaries_at: usize, called_at: usize| match several {
            false if called.is_empty() => [says(&[glyphs_at], &[17]), says(&[private.len(), private_at], &[18])].concat(),
            false => [says(&[glyphs_at], &[17]), says(&[private.len(), private_at], &[18]), says(&[called_at], &[15])].concat(),
            true => [says(&[0, 0, 0], &[12, 30]), says(&[glyphs_at], &[17]), says(&[dictionaries_at], &[12, 36]), says(&[which_at], &[12, 37])].concat(),
        };
        let which_at = 4 + names.len() + list(&[top(0, 0, 0, 0, 0)]).len() + 2 + shared.len();
        let glyphs_at = which_at + which.len();
        let dictionaries_at = glyphs_at + glyphs.len();
        // Two 11-byte DICTs make a 31-byte INDEX.
        let private_at = if several { [dictionaries_at + 31, dictionaries_at + 31 + private.len() + own[0].len()] } else { [dictionaries_at; 2] };
        let rest = match several {
            false => [private.clone(), own[0].clone()].concat(),
            true => [list(&private_at.map(|at| says(&[private.len(), at], &[18]))), private.clone(), own[0].clone(), private.clone(), own[1].clone()].concat(),
        };
        // The charset goes last.
        let (called_at, called) = if called.len() == 1 { (called[0] as usize, &[][..]) } else { (dictionaries_at + rest.len(), called) };
        let top = list(&[top(glyphs_at, private_at[0], which_at, dictionaries_at, called_at)]);
        let cff = [vec![1, 0, 4, 1], names, top, vec![0, 0], shared, which, glyphs, rest, called.to_vec()].concat();
        let tables = vec![
            (*b"CFF ", cff),
            (*b"cmap", words(&[0, 1, 3, 1, 0, 12, 6, 16, 0, 65, 3, 1, 2, 3])),
            (*b"head", [words(&[1, 0, 0, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1000]), vec![0; 34]].concat()),
            (*b"hhea", [words(&[1, 0, 800, -200]), vec![0; 26], words(&[4])].concat()),
            (*b"hmtx", words(&[500, 100, 600, 100, 700, 0, 600, 100])),
            (*b"maxp", words(&[0, 0x5000, 4])),
        ];
        file(b"OTTO", &tables)
    }

    /// What the lines draw, with the local subroutine offset 20 right and 10 down.
    const DRAWN: &str = "M100 600 200 600 200 500 150 500 150 450 100 450C110 430 140 390 190 330L210 340 190 340C191 338 194 334 199 328L206 320 207 319 209 317C219 317 219 307 209 307Z";

    #[test]
    #[cfg(feature = "cff")]
    fn programs_of_a_cff_table_draw_lines_and_curves() {
        let font = FontFile::read(curved(None)).expect("the font is read");
        assert_eq!((font.metrics(1000.0), font.advance('A', 1000.0), font.advance('B', 10.0)), (Metrics { ascent: 800.0, descent: 200.0, cap_height: 0.0, x_height: 0.0 }, 600.0, 7.0));
        assert_eq!(font.outline('@', 1000.0), Path::svg("M100 800 100 300 400 300 400 800Z"), "the missing glyph");
        assert_eq!(font.outline('A', 1000.0), Path::svg(DRAWN));
        let curves = "M0 500C10 480 40 440 40 390C100 385 170 305 260 305C260 295 280 265 320 265C370 265 430 195 435 115ZM335 115C345 115 365 85 365 45L1365 1045 865 545 865.5 544.75Z\
            M0 800C10 800 30 770 70 770C120 770 180 800 250 800C251 798 254 794 259 788C266 780 275 770 286 758C287 756 290 752 295 752C301 752 308 744 317 758\
            C318 756 321 752 326 746C333 738 342 728 317 717C327 716 337 715 347 714C357 713 367 712 372 717Z";
        assert_eq!(font.outline('B', 1000.0), Path::svg(curves));
        // Rasterized, it has soft edges and stays inside its glyph's box.
        let mut px = vec![0u32; 24 * 24];
        AppFonts::new(font).draw(&mut Canvas::new(&mut px, 24, 24, (0, 0), 1.0), "A", 4.0, 2.0, Font::new(20.0), Color::WHITE);
        let (lit, whole) = ink(&px, 24);
        assert!(!whole && lit.len() > 10 && lit.iter().all(|at| (6..9).contains(&at.0) && (8..14).contains(&at.1)), "{lit:?}");
    }

    #[test]
    #[cfg(feature = "cff")]
    fn a_cff_font_with_several_dictionaries_gives_each_glyph_its_own_pieces() {
        // FDSelect as ranges (the box, "A" and "B" on the first, "C" on the second), and as a
        // byte per glyph.
        for which in [vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 4], vec![0, 0, 0, 0, 1]] {
            let font = FontFile::read(curved(Some(which.clone()))).expect("the font is read");
            assert_eq!(font.outline('@', 1000.0), Path::svg("M100 800 100 300 400 300 400 800Z"), "{which:?}");
            // "A" and "C" are the same charstring, but each calls a different local subroutine:
            // "C"'s goes 20 right and 10 up, then on by 5 and 5.
            assert_eq!(font.outline('A', 1000.0), Path::svg(DRAWN), "{which:?}");
            let other = "M100 600 200 600 200 500 150 500 150 450 100 450C110 430 140 390 190 330L210 320 215 315 195 315C196 313 199 309 204 303L211 295 212 294 214 292C224 292 224 282 214 282Z";
            assert_eq!(font.outline('C', 1000.0), Path::svg(other), "{which:?}");
        }
        // A glyph whose FD doesn't exist is left blank.
        let font = FontFile::read(curved(Some(vec![0, 0, 9, 0, 0]))).expect("the font is read");
        assert_eq!([font.outline('A', 1000.0), font.outline('@', 1000.0)], [Path::new(), Path::svg("M100 800 100 300 400 300 400 800Z")]);
    }

    #[test]
    #[cfg(feature = "cff")]
    fn what_a_cff_program_may_not_do_leaves_its_glyph_blank() {
        // "A" has an unknown operator; "B" an unknown two-byte one; "C" calls a subroutine that
        // calls itself.
        let programs = [BOX, "100 100 rmoveto 1 2 #02 3 rlineto endchar", "100 100 rmoveto 1 2 #0c #63 3 rlineto endchar", "100 100 rmoveto 5 5 rlineto -107 callsubr endchar"];
        let font = FontFile::read(curved_by(None, programs, "5 5 rlineto -107 callsubr")).expect("the font is read");
        assert_eq!("ABC@".chars().map(|letter| font.outline(letter, 1000.0) == Path::new()).collect::<Vec<_>>(), [true, true, true, false]);
        // A glyph is complete if its charstring ends without `endchar`, or moves twice before
        // drawing; and one that draws nothing is empty.
        let programs = [BOX, "100 0 rmoveto 500 300 -500 vlineto", "100 0 rmoveto 50 hmoveto 500 300 -500 vlineto endchar", "endchar"];
        let font = FontFile::read(curved_by(None, programs, "return")).expect("the font is read");
        assert_eq!(font.outline('A', 1000.0), Path::svg("M100 800 100 300 400 300 400 800Z"));
        assert_eq!(font.outline('B', 1000.0), Path::svg("M150 800 150 300 450 300 450 800Z"));
        assert_eq!(font.outline('C', 1000.0), Path::new());
    }

    /// The outline of an "A" whose charstring draws a line rightwards for each of these
    /// expressions, as long as its result.
    fn summing(sums: &[&str]) -> Path {
        let lines: String = sums.iter().map(|sum| format!("{sum} 0 rlineto ")).collect();
        let font = FontFile::read(curved_by(None, [BOX, &format!("0 0 rmoveto {lines}endchar"), BOX, BOX], "return"));
        font.expect("the font is read").outline('A', 1000.0)
    }

    #[test]
    #[cfg(feature = "cff")]
    fn a_cff_program_does_sums_with_the_numbers_written_last() {
        // Each expression, and its result.
        let sums = [
            ("30 12 add", 42.0),
            ("30 12 sub", 18.0),
            ("6 7 mul", 42.0),
            ("42 4 div", 10.5),
            ("-9 abs", 9.0),
            ("-9 neg", 9.0),
            ("16 sqrt", 4.0),
            ("10 5 0 and add", 10.0),
            ("10 5 3 and add", 11.0),
            ("10 0 0 or add", 10.0),
            ("10 0 3 or add", 11.0),
            ("10 0 not add", 11.0),
            ("10 4 not add", 10.0),
            ("10 4 4 eq add", 11.0),
            ("10 4 5 eq add", 10.0),
            ("3 4 1 2 ifelse", 3.0),
            ("3 4 2 1 ifelse", 4.0),
            ("10 random add", 11.0),
            ("7 dup add", 14.0),
            ("1 20 exch sub", 19.0),
            ("5 6 drop", 5.0),
            ("33 2 put 5 31 put 2 get 31 get add", 38.0),
            ("40 50 60 1 index add add add", 200.0),
            ("40 50 -1 index add add", 140.0),
            ("10 20 30 3 1 roll sub add", 20.0),
            ("10 20 30 3 -1 roll sub add", 40.0),
            ("10 20 30 3 5 roll sub add", 40.0),
            ("10 20 30 0 7 roll sub add", 0.0),
            ("5 1 2 3 dotsection 5", 5.0),
        ];
        // The lines lie end to end on the baseline, 800 down.
        let mut far = 0.0;
        let ends: Vec<String> = sums.iter().map(|(_, long)| (far += long, format!(" {far} 800")).1).collect();
        assert_eq!(summing(&sums.map(|(sum, _)| sum)), Path::svg(&format!("M0 800{}Z", ends.concat())));
        // An expression giving NaN, wanting a missing operand, or overflowing the operand stack
        // (48) leaves the glyph blank.
        let crowded = ["1 ".repeat(47).as_str(), "1 dup add"].concat();
        for wrong in ["1 0 div", "-4 sqrt", "0 0 div", "add", "1 sub", "1 2 3 ifelse", "5 32 put 5", "32 get", "5 -1 put 5", "-1 get", "7 1 index", "1 2 3 4 3 roll", &crowded] {
            assert_eq!(summing(&[wrong]), Path::new(), "{wrong}");
        }
    }

    #[test]
    #[cfg(feature = "cff")]
    fn a_cff_program_ends_by_setting_an_accent_on_a_letter() {
        // "B" is a base letter (the box) and "C" an accent. "A" asks for both via seac, the
        // accent 150 right and 20 up, by Standard Encoding codes. Without a charset each
        // glyph's SID is its index: glyph 2 is SID 2, code 33, and glyph 3 code 34.
        let (accent, both) = ("0 600 rmoveto 100 50 -100 vlineto endchar", "M100 800 100 300 400 300 400 800ZM150 180 150 80 200 80 200 180Z");
        let drawn = |asking: &str, called: &[u8]| FontFile::read(curved_with(None, [BOX, asking, BOX, accent], "return", called)).expect("the font is read").outline('A', 1000.0);
        assert_eq!(drawn("150 20 33 34 endchar", &[]), Path::svg(both));
        assert_eq!(drawn("500 150 20 33 34 endchar", &[]), Path::svg(both), "with a width before the four numbers");
        assert_eq!(drawn("30 0 rmoveto 40 0 0 40 rlineto 150 20 33 34 endchar", &[]), Path::svg(&["M30 800 70 800 70 760Z", both].concat()), "after lines of its own");
        // With a charset: a SID per glyph (5, 34 and 124: "$", "A" and grave, codes 65 and
        // 193); or ranges, where SIDs 123 and 124 are consecutive (codes 191 and 193).
        assert_eq!(drawn("150 20 65 193 endchar", &[0, 0, 5, 0, 34, 0, 124]), Path::svg(both));
        assert_eq!(drawn("150 20 191 193 endchar", &[1, 0, 5, 0, 0, 123, 1]), Path::svg(both));
        assert_eq!(drawn("150 20 191 193 endchar", &[2, 0, 5, 0, 0, 0, 123, 0, 1]), Path::svg(both));
        // A code without a glyph leaves it blank: one with no SID (127), one whose SID no glyph
        // has, one in an Expert charset font (predefined).
        let lacking: [(&str, &[u8]); 5] =
            [("150 20 33 127 endchar", &[]), ("150 20 33 40 endchar", &[]), ("150 20 65 194 endchar", &[0, 0, 5, 0, 34, 0, 124]), ("150 20 33 34 endchar", &[1]), ("150 20 33 34 endchar", &[2])];
        for (asking, called) in lacking {
            assert_eq!(drawn(asking, called), Path::new(), "{asking} {called:?}");
        }
        // So do a base or accent that itself uses seac, and a font whose glyphs are numbered,
        // not named.
        let nested = FontFile::read(curved_by(None, [BOX, "150 20 33 34 endchar", BOX, "1 2 33 33 endchar"], "return")).expect("the font is read");
        let numbered = FontFile::read(curved_by(Some(vec![0, 0, 0, 0, 0]), [BOX, "150 20 33 34 endchar", BOX, accent], "return")).expect("the font is read");
        assert_eq!([nested.outline('A', 1000.0), numbered.outline('A', 1000.0)], [Path::new(), Path::new()]);
        assert_eq!(numbered.outline('C', 1000.0), Path::svg("M0 200 0 100 50 100 50 200Z"));
    }

    /// A charstring font: "A" uses seac for a base and an accent, which are "B" and "C" by its
    /// charset, and "B" does every kind of arithmetic.
    pub(crate) fn summed_and_accented() -> Vec<u8> {
        let sums = "0 0 rmoveto 30 12 add 2 put 2 get dup 5 exch sub abs 0 rlineto 10 20 30 3 1 roll 1 index div mul neg 100 rlineto \
            4 sqrt 3 4 1 2 ifelse and random or not 50 rlineto 7 8 eq drop dotsection endchar";
        curved_with(None, [BOX, "150 20 191 193 endchar", sums, "0 600 rmoveto 100 50 -100 vlineto endchar"], "return", &[1, 0, 5, 0, 0, 123, 1])
    }
}
