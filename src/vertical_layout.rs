//! Vertical text laid out in columns, as CSS `writing-mode: vertical-rl` lays it out: top to
//! bottom, columns right to left.
//!
//! A paragraph's pieces (each in its own font and colour) are shaped into steps down the
//! column: a grapheme cluster each; a run set upright across one em (tate-chu-yoko, CSS
//! `text-combine-upright`); a ruby base with its reading beside it (`ruby.rs`). Steps break
//! into columns no longer than the room where CJK line breaking allows (`line_breaks.rs`),
//! after a newline, and, for a word longer than a column, where they must. Spaces at a
//! column's end hang past it.
//!
//! Fullwidth punctuation beside other such punctuation is set half width where the font has
//! half-width forms (`vhal`), as Chrome trims it (CSS `text-spacing-trim: normal`): an opening
//! bracket after another, a middle dot or a closing mark; a closing mark before another, a
//! middle dot or a narrow closing mark. Not at a column's start or end.
//!
//! A run set across is shaped as horizontal text. Wider than 1.1 em (Chrome's and EPUB's room),
//! it takes the font's half, third or quarter width forms (`hwid`, `twid`, `qwid`) where they
//! fit, else is squeezed to fit, as Chrome does; centred in the column, an em long.
//!
//! Glyphs come from the app's font for each piece's face where it has the letters, else the
//! system's CJK font (see `vertical_text.rs`).
use crate::color::Color;
use crate::font_file::{FontFile, Placing};
use crate::geom::{Rect, rect};
use crate::path::Path;
use crate::ruby::{Spread, can_spread};
use crate::scripts::Language;
use crate::shaping::{Plan, Script};
use crate::text::{Font, TextSystem, cluster};
use crate::vertical_text::{Stands, middle, next_run, stands};
use std::ops::Range;

/// How much smaller a ruby reading is than its base.
pub(crate) const READING: f32 = 0.5;

/// How wide, in ems, a run set across may be before it is narrowed.
const ACROSS: f32 = 1.1;

/// A piece of vertical text: its bytes of the paragraph's text, style, link (from 1; 0 for
/// none), its reading's bytes (empty for none), and whether all of it is set upright across one
/// em.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Piece {
    pub words: [u32; 2],
    pub font: Font,
    pub color: Color,
    pub back: Color,
    pub link: u32,
    pub reading: [u32; 2],
    pub upright: bool,
}

/// A glyph: its piece, whether from the system's CJK font (else the piece's own), and where its
/// points go: across from its column's middle, along from its step's top.
#[derive(Clone, Copy)]
pub(crate) struct Glyph {
    pub piece: u32,
    pub system: bool,
    pub glyph: u16,
    pub place: Placing,
}

/// A grapheme cluster's place: its bytes, and its box across from the column's middle and along
/// from its step's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub bytes: [u32; 2],
    pub rect: Rect,
}

/// One step down a column: a cluster, or what stays together (a run set across one em, a ruby
/// base with its reading). Its bytes, length along the column, glyphs and places, piece;
/// whether a line may break before it; how much may be cut off its start and end (punctuation
/// set half width beside other punctuation); and for a ruby step, how far its reading may reach
/// over the steps before and after it, and how far right of the column's middle its em box
/// ends.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Step {
    pub bytes: [u32; 2],
    pub long: f32,
    pub glyphs: [u32; 2],
    pub places: [u32; 2],
    pub piece: u32,
    pub chance: bool,
    pub cut: [f32; 2],
    pub reach: f32,
    pub over: f32,
}

/// A column: its steps; its width, and the room right of it for its readings (beyond what the
/// column before leaves free); where each step's glyphs start along it (before what is cut off
/// its start), and its length (without spaces hanging at its end).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Column {
    pub steps: [u32; 2],
    pub wide: f32,
    pub room: f32,
    pub tops: Vec<f32>,
    pub long: f32,
}

/// A paragraph laid out: its text (with each reading after its base, in brackets), pieces and
/// their lines' widths, steps with their glyphs and places, and columns (first on the right);
/// its width, and its longest column's length.
#[derive(Default)]
pub(crate) struct Laid {
    pub text: String,
    pub pieces: Vec<Piece>,
    pub lines: Vec<f32>,
    pub steps: Vec<Step>,
    pub glyphs: Vec<Glyph>,
    pub places: Vec<Place>,
    pub columns: Vec<Column>,
    pub wide: f32,
    pub long: f32,
}

/// The fonts a piece is written in: its face's own, and the system's CJK font.
type Fonts<'a> = [Option<&'a FontFile>; 2];

/// What laying out asks for: the language, how many digits in a row at most are set across one
/// em (0 for none), and the room down a column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Asked {
    pub language: Language,
    pub digits: u8,
    pub room: f32,
}

/// Which way a run is set.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Way {
    /// Upright, shaped vertically.
    Upright,
    /// Rotated a quarter clockwise, shaped horizontally.
    Turned,
    /// Upright across one em (tate-chu-yoko).
    Across,
}

/// A run being shaped: its piece, bytes, size and way.
#[derive(Clone, Copy)]
struct Run {
    piece: u32,
    bytes: [u32; 2],
    size: f32,
    way: Way,
}

impl Laid {
    /// Adds a piece of text: its words, then its reading (if any) in brackets.
    pub fn add(&mut self, words: &str, font: Font, color: Color, reading: &str) -> &mut Piece {
        let start = self.text.len() as u32;
        self.text.push_str(words);
        let end = self.text.len() as u32;
        let mut read = [end; 2];
        if !reading.is_empty() {
            self.text.push('(');
            read[0] = self.text.len() as u32;
            self.text.push_str(reading);
            read[1] = self.text.len() as u32;
            self.text.push(')');
        }
        self.pieces.push(Piece { words: [start, end], font, color, back: Color::NONE, link: 0, reading: read, upright: false });
        self.pieces.last_mut().expect("just added")
    }

    /// Lays out the pieces in columns as `asked`: in the app's fonts from `text`, else the
    /// `system`'s CJK font.
    pub fn lay(&mut self, text: &mut dyn TextSystem, system: Option<&FontFile>, asked: Asked) {
        (self.steps, self.glyphs, self.places, self.lines) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for at in 0..self.pieces.len() {
            let own = own_font(text, &self.text, self.pieces[at]);
            let fonts: Fonts = [own, system];
            let line = fonts.iter().flatten().next().map_or(0.0, |font| line_of(font, self.pieces[at].font.size));
            self.lines.push(line);
            self.add_piece(&fonts, at as u32, asked);
        }
        self.mark_cuts();
        self.mark_chances();
        self.break_columns(asked.room);
    }

    /// Shapes a piece into steps: its runs, joined into one step with its reading if it has
    /// one.
    fn add_piece(&mut self, fonts: &Fonts, piece: u32, asked: Asked) {
        let at = self.steps.len();
        let given = self.pieces[piece as usize];
        self.add_runs(fonts, piece, given.words[0] as usize..given.words[1] as usize, given.font.size, asked);
        if given.reading[0] < given.reading[1] {
            self.join_ruby(fonts, piece, at, asked);
        }
    }

    /// Shapes bytes `words` of a piece at `size` into steps, run by run.
    fn add_runs(&mut self, fonts: &Fonts, piece: u32, words: Range<usize>, size: f32, asked: Asked) {
        let across = self.pieces[piece as usize].upright;
        let found: Vec<&FontFile> = fonts.iter().flatten().copied().collect();
        let mut at = words.start;
        while at < words.end {
            let rest = &self.text[at..words.end];
            let before = self.text[words.start..at].chars().next_back();
            let (long, way) = if across { (rest.len(), Way::Across) } else { run_of(&found, rest, asked.digits, before) };
            self.add_run(fonts, Run { piece, bytes: [at as u32, (at + long) as u32], size, way }, asked.language);
            at += long;
        }
    }

    /// Shapes a run and adds its steps, a cluster each (all of it, set across).
    fn add_run(&mut self, fonts: &Fonts, run: Run, language: Language) {
        let text = &self.text[run.bytes[0] as usize..run.bytes[1] as usize];
        let first = text.chars().next().unwrap_or(' ');
        let system = fonts[0].is_none_or(|own| own.glyph(first) == 0 && fonts[1].is_some_and(|other| other.glyph(first) != 0));
        let Some(font) = fonts[system as usize].or(fonts[0]) else { return };
        let system = system && fonts[1].is_some();
        let script = text.chars().find_map(Script::of).unwrap_or(Script::None);
        let mut shaped = Vec::new();
        let halves = run.way == Way::Upright && crate::shaping::positions_with(font, b"vhal");
        match run.way {
            Way::Upright => _ = font.shape(&Plan::downward(font, script, language), text, &mut shaped),
            Way::Turned => _ = font.shape(&Plan::of(font, script, false, language), text, &mut shaped),
            Way::Across => return self.add_across(font, system, run, language),
        }
        let (unit, wide) = (run.size / font.units(), line_of(font, run.size));
        let line = middle(font, unit);
        let starts = cluster_starts(text, run.bytes[0]);
        let mut glyph = 0;
        for (n, (_, byte)) in starts.iter().enumerate() {
            let (end, end_byte) = starts.get(n + 1).copied().unwrap_or((usize::MAX, run.bytes[1]));
            let first_glyph = self.glyphs.len() as u32;
            let mut along = 0.0;
            while let Some(laid) = shaped.get(glyph).filter(|laid| (laid.from as usize) < end) {
                let place = placing(run.way == Way::Turned, laid.offset.map(|at| at as f32 * unit), [unit, line, along]);
                self.glyphs.push(Glyph { piece: run.piece, system, glyph: laid.glyph, place });
                along += laid.advance as f32 * unit;
                glyph += 1;
            }
            self.push_step([*byte, end_byte], along, [first_glyph, self.glyphs.len() as u32], run.piece, wide);
            if halves {
                self.steps.last_mut().expect("just added").cut = [run.size / 2.0; 2];
            }
        }
    }

    /// A run set upright across one em: shaped horizontally; wider than 1.1 em, in the font's
    /// narrower forms where they fit, else squeezed to that; centred in the column.
    fn add_across(&mut self, font: &FontFile, system: bool, run: Run, language: Language) {
        let text = &self.text[run.bytes[0] as usize..run.bytes[1] as usize];
        let script = text.chars().find_map(Script::of).unwrap_or(Script::None);
        let (unit, mut shaped) = (run.size / font.units(), Vec::new());
        let plan = || Plan::of(font, script, false, language);
        let mut width = font.shape(&plan(), text, &mut shaped) as f32 * unit;
        let room = run.size * ACROSS;
        for form in [b"hwid", b"twid", b"qwid"] {
            let mut narrower = Vec::new();
            if width <= room {
                break;
            }
            let wide = font.shape(&plan().with_feature(font, form), text, &mut narrower) as f32 * unit;
            if wide <= room {
                (width, shaped) = (wide, narrower);
            }
        }
        let squeeze = if width > room { room / width } else { 1.0 };
        let metrics = font.metrics(run.size);
        let baseline = metrics.ascent - (metrics.ascent + metrics.descent - run.size) / 2.0;
        let left = -width * squeeze / 2.0;
        let (first_glyph, first_place) = (self.glyphs.len() as u32, self.places.len() as u32);
        let starts = cluster_starts(text, run.bytes[0]);
        let mut pen = 0.0;
        for laid in &shaped {
            let [x, y] = [laid.offset[0] as f32 * unit, laid.offset[1] as f32 * unit];
            let place = Placing([unit * squeeze, 0.0, 0.0, -unit, left + (pen + x) * squeeze, baseline - y]);
            self.glyphs.push(Glyph { piece: run.piece, system, glyph: laid.glyph, place });
            let n = starts.partition_point(|(start, _)| *start <= laid.from as usize).saturating_sub(1);
            let bytes = [starts[n].1, starts.get(n + 1).map_or(run.bytes[1], |next| next.1)];
            self.place_across(bytes, left + pen * squeeze, laid.advance as f32 * unit * squeeze, run.size);
            pen += laid.advance as f32 * unit;
        }
        let (glyphs, places) = ([first_glyph, self.glyphs.len() as u32], [first_place, self.places.len() as u32]);
        self.steps.push(Step { bytes: run.bytes, long: run.size, glyphs, places, piece: run.piece, ..Step::default() });
    }

    /// The place of a cluster set across: `left` across, `advance` wide, an em long. (A cluster
    /// of several glyphs widens its place.)
    fn place_across(&mut self, bytes: [u32; 2], left: f32, advance: f32, size: f32) {
        match self.places.last_mut().filter(|last| last.bytes == bytes) {
            Some(last) => last.rect.w = left + advance - last.rect.x,
            None => self.places.push(Place { bytes, rect: rect(left, 0.0, advance, size) }),
        }
    }

    /// Adds a step of one cluster, its place as wide as its line.
    fn push_step(&mut self, bytes: [u32; 2], long: f32, glyphs: [u32; 2], piece: u32, wide: f32) {
        self.places.push(Place { bytes, rect: rect(-wide / 2.0, 0.0, wide, long) });
        let places = [self.places.len() as u32 - 1, self.places.len() as u32];
        self.steps.push(Step { bytes, long, glyphs, places, piece, ..Step::default() });
    }
}

/// Where a glyph's points go, `along` down from its step's top, offset by `offset` (as shaped,
/// in units): upright, centred as shaped vertically; or (`turned`) shaped horizontally and
/// turned a quarter clockwise, its baseline `line` from the column's middle. `unit` is units
/// per font unit.
fn placing(turned: bool, [x, y]: [f32; 2], [unit, line, along]: [f32; 3]) -> Placing {
    match turned {
        true => Placing([0.0, unit, unit, 0.0, line + y, along + x]),
        false => Placing([unit, 0.0, 0.0, -unit, x, along - y]),
    }
}

/// The length in bytes of the next run of `text` and its way: a run of at most `digits` ASCII
/// digits with no digit just before (`before`) or after it, set across; else as far as
/// characters stand alike (see `vertical_text::next_run`), up to such digits.
fn run_of(fonts: &[&FontFile], text: &str, digits: u8, before: Option<char>) -> (usize, Way) {
    let counted = text.bytes().take_while(u8::is_ascii_digit).count();
    if counted > 0 && counted <= digits as usize && !before.is_some_and(|before| before.is_ascii_digit()) {
        return (counted, Way::Across);
    }
    let (long, turned) = next_run(fonts, text);
    let long = if digits > 0 { digits_start(text, long, digits).unwrap_or(long) } else { long };
    (long, if turned { Way::Turned } else { Way::Upright })
}

/// Where, within the first `long` bytes of `text`, digits start that are set across: at most
/// `digits` of them, after something other than a digit.
fn digits_start(text: &str, long: usize, digits: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let starts = |at: &usize| bytes[*at].is_ascii_digit() && !bytes[*at - 1].is_ascii_digit();
    let fits = |at: &usize| bytes[*at..].iter().take_while(|byte| byte.is_ascii_digit()).count() <= digits as usize;
    (1..long).filter(starts).find(fits)
}

/// Where each grapheme cluster of `text` starts: its character index, and its byte (offset by
/// `start`).
fn cluster_starts(text: &str, start: u32) -> Vec<(usize, u32)> {
    let (mut starts, mut at, mut letters) = (Vec::new(), 0, 0);
    while at < text.len() {
        starts.push((letters, start + at as u32));
        let next = at + cluster(&text[at..]);
        letters += text[at..next].chars().count();
        at = next;
    }
    starts
}

/// A font's line at a size: its ascent and descent, rounded up.
pub(crate) fn line_of(font: &FontFile, size: f32) -> f32 {
    let metrics = font.metrics(size);
    (metrics.ascent + metrics.descent).ceil()
}

/// The app's font for a piece: its face's, chosen by its first upright letter.
fn own_font<'a>(text: &'a mut dyn TextSystem, words: &str, piece: Piece) -> Option<&'a FontFile> {
    let letters = &words[piece.words[0] as usize..piece.words[1] as usize];
    let upright = letters.chars().find(|letter| stands(*letter) != Stands::Turned).unwrap_or(' ');
    text.file(piece.font.face, upright)
}

/// The first character of each of some steps.
fn firsts(text: &str, steps: &[Step]) -> Vec<char> {
    steps.iter().map(|step| text[step.bytes[0] as usize..].chars().next().unwrap_or(' ')).collect()
}

// ---- ruby ----

impl Laid {
    /// Joins the steps from `first` on (a ruby base) into one step with the piece's reading
    /// beside it: the shorter of the two spread over the other's length.
    fn join_ruby(&mut self, fonts: &Fonts, piece: u32, first: usize, asked: Asked) {
        let given = self.pieces[piece as usize];
        let size = given.font.size * READING;
        let reading_first = self.steps.len();
        self.add_runs(fonts, piece, given.reading[0] as usize..given.reading[1] as usize, size, asked);
        let length = |steps: &[Step]| steps.iter().map(|step| step.long).sum::<f32>();
        let (base, reading) = (first..reading_first, reading_first..self.steps.len());
        let (base_long, reading_long) = (length(&self.steps[base.clone()]), length(&self.steps[reading.clone()]));
        let long = base_long.max(reading_long);
        let (base_letters, reading_letters) = (firsts(&self.text, &self.steps[base.clone()]), firsts(&self.text, &self.steps[reading.clone()]));
        let base_places = crate::ruby::places(&base_letters);
        let across = (given.font.size / 2.0).ceil() + (size / 2.0).ceil();
        self.move_steps(base.clone(), crate::ruby::spread(long - base_long, base_places, None), 0.0);
        self.move_steps(reading, crate::ruby::spread(long - reading_long, crate::ruby::places(&reading_letters), Some(size)), across);
        let glyphs = [self.steps[first].glyphs[0], self.glyphs.len() as u32];
        let places = [self.steps[first].places[0], self.places.len() as u32];
        let reach = crate::ruby::reach(long - base_long, base_places, size);
        let bytes = [given.words[0], given.reading[1] + 1];
        self.steps.truncate(first);
        self.steps.push(Step { bytes, long, glyphs, places, piece, reach, over: across + (size / 2.0).ceil(), ..Step::default() });
    }

    /// Moves a stretch of steps' glyphs and places to stand one after another from the joined
    /// step's top, spread by `spread`, and `across` right of the column's middle.
    fn move_steps(&mut self, steps: Range<usize>, spread: Spread, across: f32) {
        let mut top = spread.lead;
        let mut before: Option<char> = None;
        for at in steps {
            let step = self.steps[at];
            let letter = self.text[step.bytes[0] as usize..].chars().next().unwrap_or(' ');
            top += if before.is_some_and(|before| can_spread(before, letter)) { spread.gap } else { 0.0 };
            let moved = Placing([1.0, 0.0, 0.0, 1.0, across, top]);
            self.glyphs[step.glyphs[0] as usize..step.glyphs[1] as usize].iter_mut().for_each(|glyph| glyph.place = moved.after(glyph.place));
            self.places[step.places[0] as usize..step.places[1] as usize].iter_mut().for_each(|place| place.rect = place.rect.moved(across, top));
            (top, before) = (top + step.long, Some(letter));
        }
    }

    /// How far steps `one` and `one + 1`, one after the other in a column, overlap: a ruby
    /// step's reading reaching over its neighbour by as much as it may and half that
    /// neighbour's length, where that neighbour is text no larger than its base.
    fn overlap(&self, one: usize) -> f32 {
        let (before, after) = (self.steps[one], self.steps[one + 1]);
        let reaches = |ruby: Step, other: Step| ruby.reach > 0.0 && other.reach == 0.0 && !self.is_newline(other) && self.size_of(other) <= self.size_of(ruby);
        let back = if reaches(after, before) { after.reach.min(before.long / 2.0) } else { 0.0 };
        let forth = if reaches(before, after) { before.reach.min(after.long / 2.0) } else { 0.0 };
        back + forth
    }

    fn size_of(&self, step: Step) -> f32 {
        self.pieces[step.piece as usize].font.size
    }

    fn is_newline(&self, step: Step) -> bool {
        self.text[step.bytes[0] as usize..].starts_with('\n')
    }
}

// ---- punctuation set half width ----

/// How a fullwidth punctuation mark sits in its em (CSS Text 4's text spacing classes, as a
/// Japanese font has them), for trimming the space between such marks.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mark {
    Other,
    Open,
    Close,
    Middle,
    OpenNarrow,
    CloseNarrow,
}

/// The kind of mark a character is.
fn mark_of(letter: char) -> Mark {
    match letter as u32 {
        0x3008 | 0x300a | 0x300c | 0x300e | 0x3010 | 0x3014 | 0x3016 | 0x3018 | 0x301a | 0x301d | 0xff08 | 0xff3b | 0xff5b | 0xff5f => Mark::Open,
        0x3001 | 0x3002 | 0x3009 | 0x300b | 0x300d | 0x300f | 0x3011 | 0x3015 | 0x3017 | 0x3019 | 0x301b | 0x301e | 0x301f | 0xff09 | 0xff0c | 0xff0e | 0xff3d | 0xff5d | 0xff60 => Mark::Close,
        0x30fb | 0xff1a | 0xff1b => Mark::Middle,
        0x28 | 0x5b | 0x7b | 0x2018 | 0x201c | 0xff62 => Mark::OpenNarrow,
        0x29 | 0x5d | 0x7d | 0x2019 | 0x201d | 0xff63 => Mark::CloseNarrow,
        _ => Mark::Other,
    }
}

impl Laid {
    /// Keeps what may be cut off each step only where the step is punctuation set half width
    /// beside other punctuation (see the module's notes): its start for an opening bracket, its
    /// end for a closing mark.
    fn mark_cuts(&mut self) {
        let marks: Vec<Mark> = firsts(&self.text, &self.steps).into_iter().map(mark_of).collect();
        for at in 0..self.steps.len() {
            let (before, after) = (at.checked_sub(1).map_or(Mark::Other, |before| marks[before]), marks.get(at + 1).copied().unwrap_or(Mark::Other));
            let start = marks[at] == Mark::Open && matches!(before, Mark::Open | Mark::Middle | Mark::Close | Mark::OpenNarrow);
            let end = marks[at] == Mark::Close && matches!(after, Mark::Close | Mark::Middle | Mark::CloseNarrow);
            let cut = &mut self.steps[at].cut;
            *cut = [if start { cut[0] } else { 0.0 }, if end { cut[1] } else { 0.0 }];
        }
    }

    /// What is cut off step `at`, in a column of steps `from` to `to`: nothing at the column's
    /// edges.
    pub(crate) fn cuts(&self, at: usize, from: usize, to: usize) -> [f32; 2] {
        let cut = self.steps[at].cut;
        [if at > from { cut[0] } else { 0.0 }, if at + 1 < to { cut[1] } else { 0.0 }]
    }
}

// ---- columns ----

impl Laid {
    /// Marks the steps a column may start at: where line breaking allows, and after a
    /// newline.
    fn mark_chances(&mut self) {
        let mut chances = Vec::new();
        let mut start = 0;
        let mut first = 0;
        for paragraph in self.text.split('\n') {
            crate::line_breaks::break_chances(paragraph, &mut chances);
            let end = start + paragraph.len();
            while let Some(step) = self.steps.get_mut(first).filter(|step| step.bytes[0] as usize <= end) {
                let at = step.bytes[0] as usize;
                step.chance = at == start || chances.binary_search(&(at - start)).is_ok();
                first += 1;
            }
            start = end + 1;
        }
    }

    /// Breaks the steps into columns no longer than `room`: after a newline, at the last chance
    /// that fits, else (a word longer than a column) where they must.
    fn break_columns(&mut self, room: f32) {
        self.columns.clear();
        let (mut start, mut at, mut end) = (0, 0, 0.0);
        while at < self.steps.len() {
            let top = if at == start { 0.0 } else { end - self.overlap(at - 1) };
            let [cut, _] = self.cuts(at, start, usize::MAX);
            let long = self.steps[at].long - cut - self.steps[at].cut[1];
            let shown = !self.text[self.steps[at].bytes[0] as usize..].starts_with([' ', '\n']);
            if at > start && shown && top + long > room + 0.01 {
                let chance = (start + 1..=at).rev().find(|n| self.steps[*n].chance).unwrap_or(at);
                self.end_column(start, chance);
                (start, at, end) = (chance, chance, 0.0);
                continue;
            }
            end = top + long;
            at += 1;
            if self.is_newline(self.steps[at - 1]) {
                self.end_column(start, at);
                (start, end) = (at, 0.0);
            }
        }
        if start < self.steps.len() || self.columns.is_empty() || self.text.ends_with('\n') {
            self.end_column(start, self.steps.len());
        }
        self.wide = self.columns.iter().map(|column| column.wide + column.room).sum();
        self.long = self.columns.iter().map(|column| column.long).fold(0.0, f32::max);
    }

    /// Ends a column of steps `from` to `to`: each step's top, the length to its last shown
    /// step, and its width: its widest piece's line (the last piece's, for an empty one). Its
    /// readings reaching past its right edge take the room the column before leaves free
    /// beyond its text's em boxes; the rest is added at its right (as Chrome does).
    fn end_column(&mut self, from: usize, to: usize) {
        let (mut tops, mut end) = (Vec::with_capacity(to - from), 0.0);
        let mut ends = Vec::with_capacity(to - from);
        for at in from..to {
            let top = if at == from { 0.0 } else { end - self.overlap(at - 1) };
            let [start, cut] = self.cuts(at, from, to);
            tops.push(top - start);
            end = top - start + self.steps[at].long - cut;
            ends.push(end);
        }
        let shown = (from..to).rev().find(|at| !self.text[self.steps[*at].bytes[0] as usize..].starts_with([' ', '\n']));
        let long = shown.map_or(0.0, |at| ends[at - from]);
        let empty = self.lines.last().copied().unwrap_or(0.0);
        let wide = (from..to).map(|at| self.lines[self.steps[at].piece as usize]).fold(if from == to { empty } else { 0.0 }, f32::max);
        let over = (from..to).map(|at| self.steps[at].over).fold(0.0, f32::max);
        let free = self.columns.last().map_or(0.0, |before| before.wide / 2.0 - self.em_half(before));
        let room = (over - wide / 2.0).max(0.0);
        let room = if room > 0.0 { (room - free).max(0.0) } else { 0.0 };
        self.columns.push(Column { steps: [from as u32, to as u32], wide, room, tops, long });
    }

    /// How far a column's text reaches either side of its middle: half its largest em, rounded
    /// up.
    fn em_half(&self, column: &Column) -> f32 {
        let steps = &self.steps[column.steps[0] as usize..column.steps[1] as usize];
        steps.iter().map(|step| (self.size_of(*step) / 2.0).ceil()).fold(0.0, f32::max)
    }
}

/// Where each column's middle is, from the paragraph's left: the first column at the right,
/// each after the room its readings take.
pub(crate) fn middles(columns: &[Column], wide: f32) -> Vec<f32> {
    let mut right = wide;
    let mut middle = |column: &Column| {
        right -= column.room + column.wide;
        right + column.wide / 2.0
    };
    columns.iter().map(&mut middle).collect()
}

/// A column's glyphs drawn, each piece's into its own path of `paths`: the column's middle at
/// `x` across and its top at `y` down.
pub(crate) fn outlines(laid: &Laid, text: &mut dyn TextSystem, system: Option<&FontFile>, column: &Column, [x, y]: [f32; 2], paths: &mut [Path]) {
    for (n, at) in (column.steps[0]..column.steps[1]).enumerate() {
        let step = laid.steps[at as usize];
        let own = own_font(text, &laid.text, laid.pieces[step.piece as usize]);
        let moved = Placing([1.0, 0.0, 0.0, 1.0, x, y + column.tops[n]]);
        for glyph in &laid.glyphs[step.glyphs[0] as usize..step.glyphs[1] as usize] {
            let Some(file) = (if glyph.system { system } else { own }) else { continue };
            paths[glyph.piece as usize].add(&file.sketch(glyph.glyph, moved.after(glyph.place)).path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_file::AppFonts;
    use crate::vertical_text::tests::writing_down;

    /// Lays `pieces` (text, reading, upright) out in the vertical test font at 1024 units
    /// (each "あ" 1024 long, "A" 500 wide), `room` down a column, digits as `digits` says.
    fn laid(pieces: &[(&str, &str, bool)], room: f32, digits: u8) -> Laid {
        let mut fonts = AppFonts::new(writing_down(true, false));
        let mut laid = Laid::default();
        for (words, reading, upright) in pieces {
            laid.add(words, Font::new(1024.0), Color::NONE, reading).upright = *upright;
        }
        laid.lay(&mut fonts, None, Asked { language: Language::NONE, digits, room });
        laid
    }

    /// Each column's text.
    fn columns(laid: &Laid) -> Vec<&str> {
        let end = |at: u32| laid.steps.get(at as usize).map_or(laid.text.len(), |step| step.bytes[0] as usize);
        laid.columns.iter().map(|column| &laid.text[end(column.steps[0])..end(column.steps[1])]).collect()
    }

    #[test]
    fn a_column_stands_upright_what_is_upright_and_turns_the_rest() {
        // "あ" upright, "AA" rotated, "。" in its vertical form; a rotated run is as long as it
        // is wide.
        let laid = laid(&[("\u{3042}AA\u{3002}", "", false)], f32::INFINITY, 0);
        assert_eq!(laid.glyphs.iter().map(|glyph| glyph.glyph).collect::<Vec<_>>(), [3, 4, 4, 2]);
        assert_eq!(laid.columns[0].tops, [0.0, 1024.0, 1524.0, 2024.0]);
        assert_eq!(laid.long, 1024.0 + 500.0 + 500.0 + 1024.0);
        // Upright: the glyph's origin (0, 0) half its width left of the middle and its top
        // below the pen; up in the glyph is up the page.
        let (upright, turned) = (laid.glyphs[0].place, laid.glyphs[1].place);
        assert_eq!((upright.of(crate::geom::pt(0.0, 0.0)), upright.of(crate::geom::pt(0.0, 100.0))), (crate::geom::pt(-500.0, 812.0), crate::geom::pt(-500.0, 712.0)));
        // Rotated a quarter clockwise: along its baseline is down the column, up from it is
        // right, and its letters' middle is on the column's middle.
        let line = -(832.0 - 192.0) / 2.0;
        assert_eq!(
            (turned.of(crate::geom::pt(0.0, 0.0)), turned.of(crate::geom::pt(100.0, 0.0)), turned.of(crate::geom::pt(0.0, 100.0))),
            (crate::geom::pt(line, 0.0), crate::geom::pt(line, 100.0), crate::geom::pt(line + 100.0, 0.0))
        );
    }

    #[test]
    fn columns_break_where_cjk_text_may_and_go_right_to_left() {
        // Three to a column, but "。" never starts one: the "あ" before it goes with it.
        let laid = laid(&[("あああ。ああ", "", false)], 3.0 * 1024.0, 0);
        assert_eq!(columns(&laid), ["ああ", "あ。あ", "あ"]);
        // Each a line wide, the first at the right.
        assert_eq!((laid.wide, middles(&laid.columns, laid.wide)), (3.0 * 1024.0, vec![2.5 * 1024.0, 1.5 * 1024.0, 0.5 * 1024.0]));
        // A newline ends a column; spaces at a column's end hang past it.
        let laid = self::laid(&[("ああ\nあ  あ", "", false)], 2.0 * 1024.0, 0);
        assert_eq!(columns(&laid), ["ああ\n", "あ  ", "あ"]);
        assert_eq!(laid.columns[1].long, 1024.0);
        // A word longer than a column breaks where it must.
        let laid = self::laid(&[("AAAAAAAAA", "", false)], 2000.0, 0);
        assert_eq!(columns(&laid), ["AAAA", "AAAA", "A"]);
    }

    #[test]
    fn digits_set_across_take_one_em() {
        // With up to two digits set across, "12" is one step an em long; "123" is not.
        let across = laid(&[("あ12あ123", "", false)], f32::INFINITY, 2);
        let lengths: Vec<f32> = across.steps.iter().map(|step| step.long).collect();
        assert_eq!(lengths[..3], [1024.0, 1024.0, 1024.0]);
        assert_eq!(across.steps.len(), 3 + 3);
        // Wider than 1.1 em (two glyphs 1000 wide here), squeezed to that, centred.
        let place = across.places[across.steps[1].places[0] as usize];
        assert_eq!((place.rect.x, place.rect.w, place.rect.h), (-563.2, 563.2, 1024.0));
        // A piece set upright is set across whole.
        let upright = laid(&[("あ", "", false), ("AA", "", true)], f32::INFINITY, 0);
        assert_eq!(upright.steps.len(), 2);
        assert_eq!(upright.places[1].rect.w + upright.places[2].rect.w, 1000.0);
    }

    #[test]
    fn a_reading_stands_beside_its_base_spread_over_it() {
        // One "あ" under a reading of one: the reading half the size, centred along the base,
        // its em box against the base's at the right.
        let ruby = laid(&[("あ", "あ", false)], f32::INFINITY, 0);
        assert_eq!((ruby.text.as_str(), ruby.steps.len(), ruby.steps[0].long), ("あ(あ)", 1, 1024.0));
        let reading = ruby.places[1].rect;
        assert_eq!((reading.y, reading.h), (256.0, 512.0));
        assert_eq!(reading.x + reading.w / 2.0, 512.0 + 256.0);
        // A reading of three over one: the base centred along it.
        let ruby = laid(&[("あ", "あああ", false)], f32::INFINITY, 0);
        assert_eq!(ruby.steps[0].long, 3.0 * 512.0);
        assert_eq!(ruby.places.iter().map(|place| place.rect.y).collect::<Vec<_>>(), [256.0, 0.0, 512.0, 1024.0]);
        // A reading of three over two: the reading spread, a third of the spare length at the
        // ends (a sixth each) and the rest between its characters.
        let ruby = laid(&[("ああ", "あああ", false)], f32::INFINITY, 0);
        let tops: Vec<f32> = ruby.places[2..].iter().map(|place| place.rect.y).collect();
        let (lead, gap) = (512.0 / 6.0, 512.0 / 3.0);
        assert!(tops.iter().zip([lead, lead + 512.0 + gap, lead + 2.0 * (512.0 + gap)]).all(|(top, wanted)| (top - wanted).abs() < 0.01), "{tops:?}");
    }

    #[test]
    fn a_long_reading_reaches_over_the_text_beside_it() {
        // "あ" with a reading of three, between two "あ": the reading 1536 long, its base 512
        // shorter: it reaches 256 over each neighbour (half its own size, 256).
        let ruby = laid(&[("あ", "", false), ("あ", "あああ", false), ("あ", "", false)], f32::INFINITY, 0);
        assert_eq!(ruby.columns[0].tops, [0.0, 1024.0 - 256.0, 1024.0 - 256.0 + 1536.0 - 256.0]);
        // At a column's start it reaches over nothing.
        let alone = laid(&[("あ", "あああ", false), ("あ", "", false)], f32::INFINITY, 0);
        assert_eq!(alone.columns[0].tops, [0.0, 1536.0 - 256.0]);
    }
}
