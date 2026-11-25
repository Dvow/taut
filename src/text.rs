//! Text: measuring, line breaking, and drawing.
//!
//! Every system already has a good text engine (with its fonts and settings), so each platform
//! backend hands text to it through the [`TextSystem`] trait here. An app bringing its own
//! fonts has them read and drawn by the library itself instead (`font_file.rs`), the same
//! everywhere. Where there is neither, the small built-in font below draws ASCII, so an
//! interface can show on anything with pixels. Emoji are drawn by `emoji.rs`, which wraps
//! whichever of these draws the rest; and `bidi.rs` wraps all of it, reordering bidirectional
//! lines and drawing Hebrew and Arabic itself, shaped by `shaping.rs`.
//!
//! Here too is the grapheme cluster rule: what a user sees as one character, which may be
//! several `char`s: a letter with following accents, an emoji with all its parts. The cursor
//! steps by them, and lines never break inside one. And where the cursor and selection show on
//! a bidirectional line (`marks`).
use crate::canvas::Canvas;
use crate::color::Color;

/// Which system font to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Face {
    /// The system's UI font.
    #[default]
    Regular,
    /// The same font, bold.
    Bold,
    /// Monospace: code, diffs.
    Mono,
    /// The regular font, italic: emphasis, a title. (Where there is no italic face, regular.)
    Italic,
    /// Any other system font, by family and style ([`Face::named`]), or one of these four in
    /// another style (`Face::Regular.italic()`): see `system_fonts`.
    Named(crate::system_fonts::Named),
}

/// A text decoration line, in the text colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decoration {
    /// None.
    #[default]
    None,
    /// Underline.
    Underline,
    /// Strikethrough.
    Strikethrough,
}

/// A text style: a face, a size, and an optional decoration line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    /// Which system face (or app font).
    pub face: Face,
    /// The font size in units, as CSS `font-size` counts it.
    pub size: f32,
    /// An underline, strikethrough, or none.
    pub decoration: Decoration,
}

impl Font {
    /// The regular face at this size, undecorated.
    pub const fn new(size: f32) -> Self {
        Font { face: Face::Regular, size, decoration: Decoration::None }
    }
    /// Underlined: a link.
    pub const fn underlined(self) -> Self {
        Font { decoration: Decoration::Underline, ..self }
    }
    /// Struck through: something removed or no longer valid.
    pub const fn strikethrough(self) -> Self {
        Font { decoration: Decoration::Strikethrough, ..self }
    }
    /// The same, bold.
    pub const fn bold(self) -> Self {
        Font { face: Face::Bold, ..self }
    }
    /// The same, monospace.
    pub const fn mono(self) -> Self {
        Font { face: Face::Mono, ..self }
    }
    /// The same, italic.
    pub const fn italic(self) -> Self {
        Font { face: Face::Italic, ..self }
    }
}

/// A font's vertical metrics: ascent and descent from the baseline. And its cap height and
/// x-height (of "H" and "x"): 0 for a font that does not say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Ascent above the baseline.
    pub ascent: f32,
    /// Descent below it.
    pub descent: f32,
    /// Cap height.
    pub cap_height: f32,
    /// x-height.
    pub x_height: f32,
}

impl Metrics {
    /// Line height: ascent plus descent.
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }
    /// How far above the top of its line slot, `height` tall, a line of text is drawn: so the
    /// band from its baseline to its cap height sits in the slot's middle. Caps, digits and
    /// lowercase look centred, descenders hang below, and every line in one font has the same
    /// baseline whatever its letters. (Placed by ascent and descent, text sits low in a button:
    /// most fonts keep more room above for accents than below.) Negative for a font whose
    /// glyphs sit high. A font without a cap height is centred by ascent and descent. (The line
    /// is snapped to whole pixels when drawn.)
    pub fn lift(&self, height: f32) -> f32 {
        let band = if self.cap_height > 0.0 { self.cap_height } else { self.ascent - self.descent };
        self.ascent - (height + band) / 2.0
    }
}

/// The system's text engine. Everything is in units; `scale` is pixels per unit.
pub trait TextSystem {
    /// This font's vertical metrics at this scale.
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics;
    /// The width of `text` on one line. With `stops`, also each character's end (one per
    /// `char`, from the start): what a cursor needs. The `char`s of one grapheme cluster (see
    /// `cluster`) may all end where it ends.
    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32;
    /// Draws one line of text with its top-left at (`x`, `y`).
    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color);
    /// Each character's visible left and right edges, from the line's left (one pair per
    /// `char`), in a right-to-left (`leftward`) or left-to-right paragraph; and the line's
    /// width. What cursor and selection are drawn by. (By default: each character after the
    /// previous, as `measure` has them.)
    fn places(&mut self, text: &str, font: Font, scale: f32, leftward: bool, places: &mut Vec<[f32; 2]>) -> f32 {
        let _ = leftward;
        let mut stops = Vec::new();
        let wide = self.measure(text, font, scale, Some(&mut stops));
        places.clear();
        places.extend(stops.iter().enumerate().map(|(n, stop)| [if n == 0 { 0.0 } else { stops[n - 1] }, *stop]));
        wide
    }
    /// Draws one line of a right-to-left (`leftward`) or left-to-right paragraph: a line other
    /// than its paragraph's first takes the paragraph's direction, whatever its own first
    /// letter. (By default: as `draw`.)
    #[allow(clippy::too_many_arguments)]
    fn draw_line(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color, leftward: bool) {
        let _ = leftward;
        self.draw(canvas, text, x, y, font, color);
    }
    /// The font file used for a character in a face, where the library reads it (an app font):
    /// right-to-left text is shaped with it. (By default: none.)
    fn file(&mut self, face: Face, letter: char) -> Option<&crate::font_file::FontFile> {
        let _ = (face, letter);
        None
    }
    /// The language of text drawn from now on, for its localized glyph forms (see
    /// [`Language`](crate::Language)): the window's as the app sets it, or the system's. (By
    /// default: no localized forms.)
    fn set_language(&mut self, language: crate::scripts::Language) {
        let _ = language;
    }
}

/// One line of wrapped text: its byte range, and its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Its first byte.
    pub start: usize,
    /// The byte it ends before.
    pub end: usize,
    /// Its width, in units.
    pub width: f32,
}

/// Breaks `text` into lines no wider than `max`: at newlines, else spaces, else (a word longer
/// than a line) wherever it must, but never before a zero-width character: that belongs to the
/// one before (an accent after its letter, a flag's second half). `stops` are its character
/// ends, as `measure` gives them for the whole text on one line. (With `brahmic`, text without
/// spaces between words, Thai, Lao, Khmer and Burmese, also breaks at word boundaries the
/// system knows, and where it must, between syllables: see `line_breaks.rs`.)
pub fn wrap(text: &str, stops: &[f32], max: f32, lines: &mut Vec<Line>) {
    let x_at = |n: usize| if n == 0 { 0.0 } else { stops[n - 1] };
    let unspaced = cfg!(feature = "brahmic") && text.chars().any(crate::line_breaks::unspaced);
    let chances = if unspaced { crate::line_breaks::words(text) } else { Vec::new() };
    let letters: Vec<(usize, char)> = if unspaced { text.char_indices().collect() } else { Vec::new() };
    let (mut start, mut start_n) = (0, 0); // line start: in bytes, and in characters
    // The line's last break opportunity: its byte, its index, and whether it is a space
    // (consumed by the break) or a word boundary.
    let mut space: Option<(usize, usize, bool)> = None;
    let mut n = 0;
    for (at, c) in text.char_indices() {
        if c == '\n' {
            lines.push(Line { start, end: at, width: x_at(n) - x_at(start_n) });
            (start, start_n, space) = (at + 1, n + 1, None);
        } else {
            if c == ' ' || (n > start_n && chances.binary_search(&n).is_ok()) {
                space = Some((at, n, c == ' '));
            }
            if stops[n] - x_at(start_n) > max && n > start_n && stops[n] > x_at(n) {
                // This character does not fit. Break at the last opportunity, or right here.
                let (end, next, next_n) = broken(space, &letters, stops, start_n, (at, n));
                let end_n = if space.is_some_and(|(.., spent)| spent) { next_n - 1 } else { next_n };
                lines.push(Line { start, end, width: x_at(end_n) - x_at(start_n) });
                (start, start_n, space) = (next, next_n, None);
            }
        }
        n += 1;
    }
    // The rest is the last line; after a newline, or for empty text, an empty one.
    lines.push(Line { start, end: text.len(), width: x_at(n) - x_at(start_n) });
}

/// Where a line breaks when character `n` (at byte `at`) does not fit: at its last `space`
/// (consumed by the break) or word boundary, else where it must (see `forced`). Returns the
/// line's end byte, and the next line's first byte and character index.
fn broken(space: Option<(usize, usize, bool)>, letters: &[(usize, char)], stops: &[f32], start_n: usize, (at, n): (usize, usize)) -> (usize, usize, usize) {
    match space {
        Some((byte, number, true)) => (byte, byte + 1, number + 1),
        Some((byte, number, false)) => (byte, byte, number),
        None => forced(letters, stops, start_n, (at, n)),
    }
}

/// Where a line starting at character `start_n` must break when character `n` (at byte `at`)
/// does not fit and no space or word boundary came: right there, except in text without spaces
/// between words (whose `letters` are given with their bytes), before the line's last syllable
/// start. Returns the line's end byte, and the next line's first byte and character index.
fn forced(letters: &[(usize, char)], stops: &[f32], start_n: usize, (at, n): (usize, usize)) -> (usize, usize, usize) {
    if letters.is_empty() {
        return (at, at, n);
    }
    let x_at = |n: usize| if n == 0 { 0.0 } else { stops[n - 1] };
    let chars: Vec<char> = letters.iter().map(|(_, letter)| *letter).collect();
    let starts = |k: &usize| stops[*k] > x_at(*k) && crate::line_breaks::between(&chars, *k);
    match (start_n + 1..=n).rev().find(starts) {
        Some(k) => (letters[k].0, letters[k].0, k),
        None => (at, at, n),
    }
}

// ---- characters, as a person counts them ----

/// The characters that attach to the one before, as ranges (first and last): combining marks
/// after their letter in Latin, Greek, Cyrillic, Hebrew, Arabic, Thai and kana, generic
/// combining marks, joiners, variation selectors, and emoji modifiers and tag characters.
/// Unicode's "Extend" for the scripts drawn here: Indic and Southeast Asian marks, which shape
/// their own way, wait for `brahmic`.
#[rustfmt::skip]
const JOINED: [(u32, u32); 31] = [
    (0x300, 0x36f), (0x483, 0x489), (0x591, 0x5bd), (0x5bf, 0x5bf), (0x5c1, 0x5c2), (0x5c4, 0x5c5), (0x5c7, 0x5c9), (0x610, 0x61a), (0x64b, 0x65f), (0x670, 0x670),
    (0x6d6, 0x6dc), (0x6df, 0x6e4), (0x6e7, 0x6e8), (0x6ea, 0x6ed), (0x898, 0x89f), (0x8ca, 0x8e1), (0x8e3, 0x8ff), (0xe31, 0xe31), (0xe34, 0xe3a), (0xe47, 0xe4e),
    (0x1ab0, 0x1aff), (0x1dc0, 0x1dff), (0x200c, 0x200d), (0x20d0, 0x20f0), (0x3099, 0x309a), (0xfe00, 0xfe0f), (0xfe20, 0xfe2f), (0xff9e, 0xff9f), (0x1f3fb, 0x1f3ff),
    (0xe0020, 0xe007f), (0xe0100, 0xe01ef),
];

/// The pictographic characters (emoji and the ranges reserved for them), as ranges. A few more
/// than Unicode's Extended_Pictographic: whole blocks where its list has gaps, keeping this
/// short.
#[rustfmt::skip]
const PICTURES: [(u32, u32); 19] = [
    (0xa9, 0xa9), (0xae, 0xae), (0x203c, 0x203c), (0x2049, 0x2049), (0x2122, 0x2122), (0x2139, 0x2139), (0x2194, 0x21aa), (0x231a, 0x23fa), (0x24c2, 0x24c2), (0x25aa, 0x25fe),
    (0x2600, 0x27bf), (0x2934, 0x2935), (0x2b05, 0x2b55), (0x3030, 0x3030), (0x303d, 0x303d), (0x3297, 0x3297), (0x3299, 0x3299), (0x1f000, 0x1faff), (0x1fc00, 0x1fffd),
];

/// Whether a character lies in one of some sorted ranges.
pub(crate) fn among(stretches: &[(u32, u32)], letter: char) -> bool {
    let after = stretches.partition_point(|(_, last)| *last < letter as u32);
    stretches.get(after).is_some_and(|(first, _)| *first <= letter as u32)
}

/// Whether a character is pictographic.
pub(crate) fn pictured(letter: char) -> bool {
    among(&PICTURES, letter)
}

/// The byte length of the first grapheme cluster of `text`: a letter with following accents, an
/// emoji with all its parts (a skin tone, joiners and the emoji they join, a flag's second
/// letter), CR LF. The cursor never stands inside one, and nothing splits it. (Unicode's
/// extended grapheme cluster rule, as far as it concerns the scripts drawn here. With
/// `brahmic`, the full rule: also an Indic or Southeast Asian syllable with its vowel signs
/// and, where a virama joins them, its consonants; a Hangul syllable written in jamo; a
/// prepended mark.)
pub fn cluster(text: &str) -> usize {
    if cfg!(feature = "brahmic") {
        return grapheme(text);
    }
    let mut letters = text.char_indices().peekable();
    let Some((_, first)) = letters.next() else { return 0 };
    // CR LF is one; nothing else attaches to a control.
    if first.is_control() {
        return if text.starts_with("\r\n") { 2 } else { first.len_utf8() };
    }
    // A flag's two regional indicators are one.
    let flag = |letter: char| ('\u{1f1e6}'..='\u{1f1ff}').contains(&letter);
    letters.next_if(|(_, second)| flag(first) && flag(*second));
    // Then whatever attaches: marks; and after a pictograph (with its marks) and a ZWJ, the
    // joined pictograph.
    let (mut picture, mut joined) = (pictured(first), false);
    while let Some((_, next)) = letters.next_if(|(_, next)| among(&JOINED, *next) || (joined && pictured(*next))) {
        (picture, joined) = match (next, among(&JOINED, next)) {
            ('\u{200d}', _) => (false, picture),
            (_, true) => (picture, false),
            (_, false) => (true, false),
        };
    }
    letters.peek().map_or(text.len(), |(at, _)| *at)
}

/// Where the grapheme cluster ending at byte `at` of `text`, or containing it, starts.
pub fn cluster_before(text: &str, at: usize) -> usize {
    // A cluster can only be determined from its start. So this goes back to a character that
    // certainly starts one (nothing before it attaches to it), then forward a character at a
    // time.
    let alone = |(start, letter): &(usize, char)| {
        let before = text[..*start].chars().next_back();
        if cfg!(feature = "brahmic") {
            return before.is_none_or(|before| starts(before, *letter));
        }
        !among(&JOINED, *letter) && !pictured(*letter) && before != Some('\r') || before.is_none()
    };
    let mut start = text[..at].char_indices().rev().find(alone).map_or(0, |(start, _)| start);
    while start + cluster(&text[start..]) < at {
        start += cluster(&text[start..]);
    }
    start
}

/// A character's properties for the grapheme rules: its break class (the low four bits of
/// `GRAPHEMES`), whether it is pictographic, and its Indic conjunct role (linker, consonant, or
/// extender).
const CR: u8 = 1;
const LF: u8 = 2;
const CONTROL: u8 = 3;
const EXTEND: u8 = 4;
const ZWJ: u8 = 5;
const REGIONAL: u8 = 6;
const PREPEND: u8 = 7;
const SPACING: u8 = 8;
const L: u8 = 9;
const V: u8 = 10;
const T: u8 = 11;
const LV: u8 = 12;
const LVT: u8 = 13;
const PICTURE: u8 = 16;
const LINKER: u8 = 32;
const CONSONANT: u8 = 64;
const CONJUNCT: u8 = 96;

/// A character's break properties (see `CR` and the rest).
fn grapheme_kind(letter: char) -> u8 {
    let code = letter as u32;
    // (A Hangul syllable is LV every 28th, else LVT.)
    if (0xac00..=0xd7a3).contains(&code) {
        return if (code - 0xac00).is_multiple_of(28) { LV } else { LVT };
    }
    let n = GRAPHEMES.partition_point(|entry| entry >> 7 <= code);
    n.checked_sub(1).map_or(0, |n| (GRAPHEMES[n] & 127) as u8)
}

/// What the cluster so far implies for the next character: how many regional indicators came
/// last in a row; whether a pictograph (with extenders) then a ZWJ came last; and whether an
/// Indic consonant came, then extenders with a linker (virama) among them, which join the next
/// consonant.
#[derive(Clone, Copy, Default)]
struct Run {
    flags: usize,
    picture: u8,
    conjunct: u8,
}

impl Run {
    /// The state after a character of class `kind`.
    fn after(self, kind: u8) -> Run {
        let flags = if kind & 15 == REGIONAL { self.flags + 1 } else { 0 };
        let picture = match (self.picture, kind & 15) {
            _ if kind & PICTURE != 0 => 1,
            (1, EXTEND) => 1,
            (1, ZWJ) => 2,
            _ => 0,
        };
        let conjunct = match (self.conjunct, kind & CONJUNCT) {
            (_, CONSONANT) => 1,
            (1.., LINKER) => 2,
            (1.., CONJUNCT) => self.conjunct,
            _ => 0,
        };
        Run { flags, picture, conjunct }
    }
}

/// Whether a cluster boundary falls between characters of these classes, by Unicode's rules
/// (numbered after each), with `run` the preceding state.
fn boundary(before: u8, now: u8, run: Run) -> bool {
    match (before & 15, now & 15) {
        (CR, LF) => false,                                               // GB3
        (CR | LF | CONTROL, _) | (_, CR | LF | CONTROL) => true,         // GB4, GB5
        (L, L | V | LV | LVT) | (LV | V, V | T) | (LVT | T, T) => false, // GB6 to GB8
        (_, EXTEND | ZWJ | SPACING) | (PREPEND, _) => false,             // GB9, GB9a, GB9b
        _ if run.conjunct == 2 && now & CONJUNCT == CONSONANT => false,  // GB9c
        _ if run.picture == 2 && now & PICTURE != 0 => false,            // GB11
        (REGIONAL, REGIONAL) => run.flags.is_multiple_of(2),             // GB12, GB13
        _ => true,                                                       // GB999
    }
}

/// The byte length of the first grapheme cluster of `text`, by Unicode's full rule (Unicode 17,
/// UAX #29).
fn grapheme(text: &str) -> usize {
    let mut letters = text.char_indices();
    let Some((_, first)) = letters.next() else { return 0 };
    let mut before = grapheme_kind(first);
    let mut run = Run::default().after(before);
    for (at, letter) in letters {
        let now = grapheme_kind(letter);
        if boundary(before, now, run) {
            return at;
        }
        (before, run) = (now, run.after(now));
    }
    text.len()
}

/// Whether a cluster certainly starts at `letter`, whatever came before `before`: nothing that
/// attaches backward, after nothing that attaches forward.
fn starts(before: char, letter: char) -> bool {
    let (before, kind) = (grapheme_kind(before) & 15, grapheme_kind(letter));
    let joins = matches!(kind & 15, EXTEND | ZWJ | SPACING | REGIONAL | L | V | T | LV | LVT) || kind & PICTURE != 0 || kind & CONJUNCT == CONSONANT;
    !joins && before != PREPEND && before != CR
}

/// Each character's grapheme properties as ranges: the first character (times 128) with its
/// break class (see `CR` and the rest), its pictographic flag, and its Indic conjunct role.
/// Unicode 17's Grapheme_Cluster_Break, Extended_Pictographic and Indic_Conjunct_Break; Hangul
/// syllables are computed instead. (Only with `brahmic`.)
#[rustfmt::skip]
const GRAPHEMES: [u32; 1369] = [
    0x3, 0x502, 0x583, 0x681, 0x703, 0x1000, 0x3f83, 0x5000, 0x5490, 0x5500, 0x5683, 0x5710,
    0x5780, 0x18064, 0x1b800, 0x241e4, 0x24500, 0x2c8e4, 0x2df00, 0x2dfe4, 0x2e000, 0x2e0e4, 0x2e180, 0x2e264,
    0x2e300, 0x2e3e4, 0x2e400, 0x30007, 0x30300, 0x30864, 0x30d80, 0x30e03, 0x30e80, 0x325e4, 0x33000, 0x33864,
    0x33880, 0x36b64, 0x36e87, 0x36f00, 0x36fe4, 0x37280, 0x373e4, 0x37480, 0x37564, 0x37700, 0x38787, 0x38800,
    0x388e4, 0x38900, 0x39864, 0x3a580, 0x3d364, 0x3d880, 0x3f5e4, 0x3fa00, 0x3fee4, 0x3ff00, 0x40b64, 0x40d00,
    0x40de4, 0x41200, 0x412e4, 0x41400, 0x414e4, 0x41700, 0x42ce4, 0x42e00, 0x44807, 0x44900, 0x44be4, 0x45000,
    0x46564, 0x47107, 0x471e4, 0x48188, 0x48200, 0x48ac0, 0x49d64, 0x49d88, 0x49e64, 0x49e80, 0x49f08, 0x4a0e4,
    0x4a488, 0x4a6a4, 0x4a708, 0x4a800, 0x4a8e4, 0x4ac40, 0x4b000, 0x4b164, 0x4b200, 0x4bc40, 0x4c000, 0x4c0e4,
    0x4c108, 0x4c200, 0x4cac0, 0x4d480, 0x4d540, 0x4d880, 0x4d940, 0x4d980, 0x4db40, 0x4dd00, 0x4de64, 0x4de80,
    0x4df64, 0x4df88, 0x4e0e4, 0x4e280, 0x4e388, 0x4e480, 0x4e588, 0x4e6a4, 0x4e700, 0x4ebe4, 0x4ec00, 0x4ee40,
    0x4ef00, 0x4efc0, 0x4f000, 0x4f164, 0x4f200, 0x4f840, 0x4f900, 0x4ff64, 0x4ff80, 0x500e4, 0x50188, 0x50200,
    0x51e64, 0x51e80, 0x51f08, 0x520e4, 0x52180, 0x523e4, 0x52480, 0x525e4, 0x52700, 0x528e4, 0x52900, 0x53864,
    0x53900, 0x53ae4, 0x53b00, 0x540e4, 0x54188, 0x54200, 0x54ac0, 0x55480, 0x55540, 0x55880, 0x55940, 0x55a00,
    0x55ac0, 0x55d00, 0x55e64, 0x55e80, 0x55f08, 0x560e4, 0x56300, 0x563e4, 0x56488, 0x56500, 0x56588, 0x566a4,
    0x56700, 0x57164, 0x57200, 0x57cc0, 0x57d64, 0x58000, 0x580e4, 0x58108, 0x58200, 0x58ac0, 0x59480, 0x59540,
    0x59880, 0x59940, 0x59a00, 0x59ac0, 0x59d00, 0x59e64, 0x59e80, 0x59f64, 0x5a008, 0x5a0e4, 0x5a280, 0x5a388,
    0x5a480, 0x5a588, 0x5a6a4, 0x5a700, 0x5aae4, 0x5ac00, 0x5ae40, 0x5af00, 0x5afc0, 0x5b000, 0x5b164, 0x5b200,
    0x5b8c0, 0x5b900, 0x5c164, 0x5c180, 0x5df64, 0x5df88, 0x5e064, 0x5e088, 0x5e180, 0x5e308, 0x5e480, 0x5e508,
    0x5e6e4, 0x5e700, 0x5ebe4, 0x5ec00, 0x60064, 0x60088, 0x60264, 0x60280, 0x60ac0, 0x61480, 0x61540, 0x61d00,
    0x61e64, 0x61e80, 0x61f64, 0x62088, 0x62280, 0x62364, 0x62480, 0x62564, 0x626a4, 0x62700, 0x62ae4, 0x62b80,
    0x62c40, 0x62d80, 0x63164, 0x63200, 0x640e4, 0x64108, 0x64200, 0x65e64, 0x65e80, 0x65f08, 0x65fe4, 0x66088,
    0x66164, 0x66188, 0x66280, 0x66364, 0x66480, 0x66564, 0x66700, 0x66ae4, 0x66b80, 0x67164, 0x67200, 0x67988,
    0x67a00, 0x68064, 0x68108, 0x68200, 0x68ac0, 0x69de4, 0x69e80, 0x69f64, 0x69f88, 0x6a0e4, 0x6a280, 0x6a308,
    0x6a480, 0x6a508, 0x6a6a4, 0x6a707, 0x6a780, 0x6abe4, 0x6ac00, 0x6b164, 0x6b200, 0x6c0e4, 0x6c108, 0x6c200,
    0x6e564, 0x6e580, 0x6e7e4, 0x6e808, 0x6e964, 0x6ea80, 0x6eb64, 0x6eb80, 0x6ec08, 0x6efe4, 0x6f000, 0x6f908,
    0x6fa00, 0x718e4, 0x71900, 0x71988, 0x71a64, 0x71d80, 0x723e4, 0x72780, 0x758e4, 0x75900, 0x75988, 0x75a64,
    0x75e80, 0x76464, 0x76780, 0x78c64, 0x78d00, 0x79ae4, 0x79b00, 0x79be4, 0x79c00, 0x79ce4, 0x79d00, 0x79f08,
    0x7a000, 0x7b8e4, 0x7bf88, 0x7c064, 0x7c280, 0x7c364, 0x7c400, 0x7c6e4, 0x7cc00, 0x7cce4, 0x7de80, 0x7e364,
    0x7e380, 0x80040, 0x81580, 0x816e4, 0x81888, 0x81964, 0x81c00, 0x81ca4, 0x81d64, 0x81d88, 0x81ee4, 0x81fc0,
    0x82000, 0x82840, 0x82b08, 0x82c64, 0x82d40, 0x82f64, 0x830c0, 0x83100, 0x832c0, 0x83380, 0x83740, 0x838e4,
    0x83ac0, 0x84164, 0x84180, 0x84208, 0x842e4, 0x84380, 0x846e4, 0x84740, 0x84780, 0x84ee4, 0x84f00, 0x88009,
    0x8b00a, 0x8d40b, 0x90000, 0x9aee4, 0x9b000, 0xb8964, 0xb8b00, 0xb9964, 0xb9a80, 0xba964, 0xbaa00, 0xbb964,
    0xbba00, 0xbc040, 0xbda64, 0xbdb08, 0xbdbe4, 0xbdf08, 0xbe364, 0xbe388, 0xbe4e4, 0xbe924, 0xbe9e4, 0xbea00,
    0xbeee4, 0xbef00, 0xc05e4, 0xc0703, 0xc07e4, 0xc0800, 0xc42e4, 0xc4380, 0xc54e4, 0xc5500, 0xc9064, 0xc9188,
    0xc93e4, 0xc9488, 0xc9600, 0xc9808, 0xc9964, 0xc9988, 0xc9ce4, 0xc9e00, 0xd0be4, 0xd0c88, 0xd0de4, 0xd0e00,
    0xd1040, 0xd2a88, 0xd2b64, 0xd2b88, 0xd2c64, 0xd2f80, 0xd3024, 0xd3080, 0xd3164, 0xd3180, 0xd32e4, 0xd3688,
    0xd39e4, 0xd3e80, 0xd3fe4, 0xd4000, 0xd5864, 0xd6f00, 0xd7064, 0xd7600, 0xd8064, 0xd8208, 0xd8280, 0xd85c0,
    0xd8680, 0xd89c0, 0xd9a64, 0xd9f08, 0xda164, 0xda224, 0xda2c0, 0xda680, 0xdb5e4, 0xdba00, 0xdc064, 0xdc108,
    0xdc1c0, 0xdd088, 0xdd164, 0xdd308, 0xdd464, 0xdd5a4, 0xdd664, 0xdd740, 0xdd800, 0xdddc0, 0xddf00, 0xdf364,
    0xdf388, 0xdf464, 0xdf508, 0xdf6e4, 0xdf708, 0xdf7e4, 0xdfa00, 0xe1208, 0xe1664, 0xe1a08, 0xe1b64, 0xe1c00,
    0xe6864, 0xe6980, 0xe6a64, 0xe7088, 0xe7164, 0xe7480, 0xe76e4, 0xe7700, 0xe7a64, 0xe7a80, 0xe7b88, 0xe7c64,
    0xe7d00, 0xee064, 0xf0000, 0x100583, 0x100604, 0x1006e5, 0x100703, 0x100800, 0x101403, 0x101780, 0x101e10, 0x101e80,
    0x102490, 0x102500, 0x103003, 0x103800, 0x106864, 0x107880, 0x109110, 0x109180, 0x109c90, 0x109d00, 0x10ca10, 0x10cd00,
    0x10d490, 0x10d580, 0x118d10, 0x118e00, 0x119410, 0x119480, 0x11e790, 0x11e800, 0x11f490, 0x11fa00, 0x11fc10, 0x11fd80,
    0x126110, 0x126180, 0x12d510, 0x12d600, 0x12db10, 0x12db80, 0x12e010, 0x12e080, 0x12fd90, 0x12ff80, 0x130010, 0x130280,
    0x130710, 0x130780, 0x130890, 0x130900, 0x130a10, 0x130b00, 0x130c10, 0x130c80, 0x130e90, 0x130f00, 0x131010, 0x131080,
    0x131110, 0x131200, 0x131310, 0x131380, 0x131510, 0x131580, 0x131710, 0x131800, 0x131c10, 0x131d80, 0x132010, 0x132080,
    0x132110, 0x132180, 0x132410, 0x132a00, 0x132f90, 0x133080, 0x133190, 0x133200, 0x133290, 0x133380, 0x133410, 0x133480,
    0x133d90, 0x133e00, 0x133f10, 0x134000, 0x134910, 0x134c00, 0x134c90, 0x134d00, 0x134d90, 0x134e80, 0x135010, 0x135100,
    0x135390, 0x135400, 0x135510, 0x135600, 0x135810, 0x135900, 0x135e90, 0x135f80, 0x136210, 0x136300, 0x136410, 0x136480,
    0x136710, 0x136800, 0x136890, 0x136900, 0x136990, 0x136a80, 0x137490, 0x137580, 0x137810, 0x137b00, 0x137b90, 0x137d80,
    0x137e90, 0x137f00, 0x138110, 0x138180, 0x138290, 0x138300, 0x138410, 0x138700, 0x138790, 0x138800, 0x138910, 0x138980,
    0x138a10, 0x138a80, 0x138b10, 0x138b80, 0x138e90, 0x138f00, 0x139090, 0x139100, 0x139410, 0x139480, 0x139990, 0x139a80,
    0x13a210, 0x13a280, 0x13a390, 0x13a400, 0x13a610, 0x13a680, 0x13a710, 0x13a780, 0x13a990, 0x13ab00, 0x13ab90, 0x13ac00,
    0x13b190, 0x13b280, 0x13ca90, 0x13cc00, 0x13d090, 0x13d100, 0x13d810, 0x13d880, 0x13df90, 0x13e000, 0x149a10, 0x149b00,
    0x158290, 0x158400, 0x158d90, 0x158e80, 0x15a810, 0x15a880, 0x15aa90, 0x15ab00, 0x1677e4, 0x167900, 0x16bfe4, 0x16c000,
    0x16f064, 0x170000, 0x181564, 0x181810, 0x181880, 0x181e90, 0x181f00, 0x184ce4, 0x184d80, 0x194b90, 0x194c00, 0x194c90,
    0x194d00, 0x5337e4, 0x533980, 0x533a64, 0x533f00, 0x534f64, 0x535000, 0x537864, 0x537900, 0x540164, 0x540180, 0x540364,
    0x540380, 0x5405e4, 0x540600, 0x541188, 0x5412e4, 0x541388, 0x541400, 0x541664, 0x541680, 0x544008, 0x544100, 0x545a08,
    0x546264, 0x546300, 0x547064, 0x547900, 0x547fe4, 0x548000, 0x549364, 0x549700, 0x54a3e4, 0x54a908, 0x54a9e4, 0x54aa00,
    0x54b009, 0x54be80, 0x54c064, 0x54c188, 0x54c200, 0x54c4c0, 0x54c600, 0x54c7c0, 0x54d9e4, 0x54da08, 0x54db64, 0x54dd08,
    0x54de64, 0x54df08, 0x54e024, 0x54e080, 0x54f040, 0x54f2e4, 0x54f300, 0x54f3c0, 0x54f800, 0x54fd40, 0x54ff80, 0x5514e4,
    0x551788, 0x5518e4, 0x551988, 0x551ae4, 0x551b80, 0x5521e4, 0x552200, 0x552664, 0x552688, 0x552700, 0x553040, 0x553800,
    0x5538c0, 0x553a00, 0x553d40, 0x553d80, 0x553e64, 0x553e80, 0x553f40, 0x554000, 0x555864, 0x555880, 0x555964, 0x555a80,
    0x555be4, 0x555c80, 0x555f64, 0x556000, 0x5560e4, 0x556100, 0x557040, 0x557588, 0x557664, 0x557708, 0x557800, 0x557a88,
    0x557b24, 0x557b80, 0x55e040, 0x55ed80, 0x55f188, 0x55f2e4, 0x55f308, 0x55f464, 0x55f488, 0x55f580, 0x55f608, 0x55f6e4,
    0x55f700, 0x6bd80a, 0x6be380, 0x6be58b, 0x6bfe00, 0x7d8f64, 0x7d8f80, 0x7f0064, 0x7f0800, 0x7f1064, 0x7f1800, 0x7f7f83,
    0x7f8000, 0x7fcf64, 0x7fd000, 0x7ff803, 0x7ffe00, 0x80fee4, 0x80ff00, 0x817064, 0x817080, 0x81bb64, 0x81bd80, 0x850040,
    0x8500e4, 0x850200, 0x8502e4, 0x850380, 0x850664, 0x850840, 0x850a00, 0x850ac0, 0x850c00, 0x850cc0, 0x851b00, 0x851c64,
    0x851d80, 0x851fa4, 0x852000, 0x8572e4, 0x857380, 0x869264, 0x869400, 0x86b4e4, 0x86b700, 0x8755e4, 0x875680, 0x877d64,
    0x878000, 0x87a364, 0x87a880, 0x87c164, 0x87c300, 0x880008, 0x8800e4, 0x880108, 0x880180, 0x881c64, 0x882380, 0x883864,
    0x883880, 0x8839e4, 0x883a80, 0x883fe4, 0x884108, 0x884180, 0x885808, 0x8859e4, 0x885b88, 0x885ce4, 0x885d80, 0x885e87,
    0x885f00, 0x886164, 0x886180, 0x886687, 0x886700, 0x888064, 0x8881c0, 0x8893e4, 0x889608, 0x8896e4, 0x8899a4, 0x889a64,
    0x889a80, 0x88a240, 0x88a288, 0x88a3c0, 0x88a400, 0x88b9e4, 0x88ba00, 0x88c064, 0x88c108, 0x88c180, 0x88d988, 0x88db64,
    0x88df88, 0x88e064, 0x88e080, 0x88e107, 0x88e200, 0x88e4e4, 0x88e680, 0x88e708, 0x88e7e4, 0x88e800, 0x891608, 0x8917e4,
    0x891908, 0x891a64, 0x891c00, 0x891f64, 0x891f80, 0x8920e4, 0x892100, 0x896fe4, 0x897008, 0x8971e4, 0x897580, 0x898064,
    0x898108, 0x898200, 0x899de4, 0x899e80, 0x899f64, 0x899f88, 0x89a064, 0x89a088, 0x89a280, 0x89a388, 0x89a480, 0x89a588,
    0x89a6e4, 0x89a700, 0x89abe4, 0x89ac00, 0x89b108, 0x89b200, 0x89b364, 0x89b680, 0x89b864, 0x89ba80, 0x89c040, 0x89c500,
    0x89c5c0, 0x89c600, 0x89c740, 0x89c780, 0x89c840, 0x89db00, 0x89dc64, 0x89dc88, 0x89dde4, 0x89e080, 0x89e164, 0x89e180,
    0x89e2e4, 0x89e300, 0x89e3e4, 0x89e508, 0x89e580, 0x89e608, 0x89e764, 0x89e824, 0x89e887, 0x89e964, 0x89e980, 0x89f0e4,
    0x89f180, 0x8a1a88, 0x8a1c64, 0x8a2008, 0x8a2164, 0x8a2288, 0x8a2364, 0x8a2380, 0x8a2f64, 0x8a2f80, 0x8a5864, 0x8a5888,
    0x8a59e4, 0x8a5c88, 0x8a5d64, 0x8a5d88, 0x8a5ee4, 0x8a5f08, 0x8a5fe4, 0x8a6088, 0x8a6164, 0x8a6200, 0x8ad7e4, 0x8ad808,
    0x8ad964, 0x8adb00, 0x8adc08, 0x8ade64, 0x8adf08, 0x8adfe4, 0x8ae080, 0x8aee64, 0x8aef00, 0x8b1808, 0x8b19e4, 0x8b1d88,
    0x8b1ee4, 0x8b1f08, 0x8b1fe4, 0x8b2080, 0x8b55e4, 0x8b5608, 0x8b56e4, 0x8b5708, 0x8b5864, 0x8b5c00, 0x8b8ee4, 0x8b8f08,
    0x8b8fe4, 0x8b9000, 0x8b9164, 0x8b9308, 0x8b93e4, 0x8b9600, 0x8c1608, 0x8c17e4, 0x8c1c08, 0x8c1ce4, 0x8c1d80, 0x8c8040,
    0x8c8380, 0x8c84c0, 0x8c8500, 0x8c8640, 0x8c8a00, 0x8c8ac0, 0x8c8b80, 0x8c8c40, 0x8c9864, 0x8c9888, 0x8c9b00, 0x8c9b88,
    0x8c9c80, 0x8c9de4, 0x8c9f24, 0x8c9f87, 0x8ca008, 0x8ca087, 0x8ca108, 0x8ca1e4, 0x8ca200, 0x8ce888, 0x8cea64, 0x8cec00,
    0x8ced64, 0x8cee08, 0x8cf064, 0x8cf080, 0x8cf208, 0x8cf280, 0x8d0040, 0x8d00e4, 0x8d05c0, 0x8d19e4, 0x8d1c88, 0x8d1d00,
    0x8d1de4, 0x8d1f80, 0x8d23a4, 0x8d2400, 0x8d2840, 0x8d28e4, 0x8d2b88, 0x8d2ce4, 0x8d2e40, 0x8d4207, 0x8d4564, 0x8d4b88,
    0x8d4c64, 0x8d4ca4, 0x8d4d00, 0x8db064, 0x8db088, 0x8db164, 0x8db288, 0x8db364, 0x8db388, 0x8db400, 0x8e1788, 0x8e1864,
    0x8e1b80, 0x8e1c64, 0x8e1f08, 0x8e1fe4, 0x8e2000, 0x8e4964, 0x8e5400, 0x8e5488, 0x8e5564, 0x8e5888, 0x8e5964, 0x8e5a08,
    0x8e5ae4, 0x8e5b80, 0x8e98e4, 0x8e9b80, 0x8e9d64, 0x8e9d80, 0x8e9e64, 0x8e9f00, 0x8e9fe4, 0x8ea307, 0x8ea3e4, 0x8ea400,
    0x8ec508, 0x8ec780, 0x8ec864, 0x8ec900, 0x8ec988, 0x8ecae4, 0x8ecb08, 0x8ecbe4, 0x8ecc00, 0x8f79e4, 0x8f7a88, 0x8f7b80,
    0x8f8064, 0x8f8107, 0x8f8188, 0x8f8240, 0x8f8880, 0x8f8940, 0x8f9a08, 0x8f9b64, 0x8f9d80, 0x8f9f08, 0x8fa064, 0x8fa124,
    0x8fa180, 0x8fad64, 0x8fad80, 0x9a1803, 0x9a2064, 0x9a2080, 0x9a23e4, 0x9a2b00, 0xb08f64, 0xb09508, 0xb096e4, 0xb09800,
    0xb57864, 0xb57a80, 0xb59864, 0xb59b80, 0xb6b18a, 0xb6b200, 0xb6b38a, 0xb6b580, 0xb7a7e4, 0xb7a800, 0xb7a888, 0xb7c400,
    0xb7c7e4, 0xb7c980, 0xb7f264, 0xb7f280, 0xb7f864, 0xb7f900, 0xde4ee4, 0xde4f80, 0xde5003, 0xde5200, 0xe78064, 0xe79700,
    0xe79864, 0xe7a380, 0xe8b2e4, 0xe8b500, 0xe8b6e4, 0xe8b983, 0xe8bde4, 0xe8c180, 0xe8c2e4, 0xe8c600, 0xe8d564, 0xe8d700,
    0xe92164, 0xe92280, 0xed0064, 0xed1b80, 0xed1de4, 0xed3680, 0xed3ae4, 0xed3b00, 0xed4264, 0xed4280, 0xed4de4, 0xed5000,
    0xed50e4, 0xed5800, 0xf00064, 0xf00380, 0xf00464, 0xf00c80, 0xf00de4, 0xf01100, 0xf011e4, 0xf01280, 0xf01364, 0xf01580,
    0xf047e4, 0xf04800, 0xf09864, 0xf09b80, 0xf15764, 0xf15780, 0xf17664, 0xf17800, 0xf27664, 0xf27800, 0xf2f764, 0xf2f800,
    0xf371e4, 0xf37200, 0xf37364, 0xf37380, 0xf37764, 0xf37800, 0xf37ae4, 0xf37b00, 0xf46864, 0xf46b80, 0xf4a264, 0xf4a580,
    0xf80210, 0xf80280, 0xf81610, 0xf81800, 0xf84a10, 0xf85000, 0xf85790, 0xf85880, 0xf86010, 0xf86080, 0xf86790, 0xf86880,
    0xf87b10, 0xf88000, 0xf8b810, 0xf8b900, 0xf8bf10, 0xf8c000, 0xf8c710, 0xf8c780, 0xf8c890, 0xf8cd80, 0xf8d710, 0xf8f306,
    0xf90000, 0xf90090, 0xf90800, 0xf90d10, 0xf90d80, 0xf91790, 0xf91800, 0xf91910, 0xf91d80, 0xf91e10, 0xf92000, 0xf92490,
    0xf93000, 0xf93310, 0xf99100, 0xf99210, 0xf9ca00, 0xf9cb10, 0xf9cc00, 0xf9cc90, 0xf9ce00, 0xf9cf10, 0xf9f880, 0xf9f990,
    0xf9fb00, 0xf9fb90, 0xf9fde4, 0xfa0010, 0xfa7f00, 0xfa7f90, 0xfa9f00, 0xfaa490, 0xfaa780, 0xfaa810, 0xfab400, 0xfab790,
    0xfab880, 0xfab990, 0xfabd80, 0xfac390, 0xfac400, 0xfac510, 0xfac700, 0xfac810, 0xfac880, 0xfaca90, 0xfacb80, 0xfad210,
    0xfad300, 0xfad410, 0xfad480, 0xfad890, 0xfad980, 0xfade10, 0xfade80, 0xfae110, 0xfae280, 0xfae890, 0xfaea00, 0xfaee10,
    0xfaef80, 0xfaf090, 0xfaf100, 0xfaf190, 0xfaf200, 0xfaf410, 0xfaf480, 0xfaf790, 0xfaf800, 0xfaf990, 0xfafa00, 0xfafd10,
    0xfb2800, 0xfb4010, 0xfb6300, 0xfb6590, 0xfb6980, 0xfb6a90, 0xfb7300, 0xfb7490, 0xfb7500, 0xfb7590, 0xfb7880, 0xfb7990,
    0xfb8000, 0xfbed10, 0xfc0000, 0xfc0610, 0xfc0800, 0xfc2410, 0xfc2800, 0xfc2d10, 0xfc3000, 0xfc4410, 0xfc4800, 0xfc5710,
    0xfc5800, 0xfc5e10, 0xfc6000, 0xfc6110, 0xfc6800, 0xfc6c90, 0xfc8000, 0xfc8610, 0xfc9d80, 0xfc9e10, 0xfca300, 0xfca390,
    0xfd0000, 0xfd2c10, 0xfd3000, 0xfd3710, 0xfd8000, 0xfe0010, 0xffff00, 0x7000003, 0x7001064, 0x7004003, 0x7008064, 0x700f803,
    0x7080000,
];

// ---- where a cursor and a selection are seen ----

/// One grapheme cluster of a line as displayed: its bytes (in the containing text), its left
/// and right edges, and whether it is right-to-left.
pub(crate) struct ShownCluster {
    pub bytes: std::ops::Range<usize>,
    pub edges: [f32; 2],
    pub leftward: bool,
}

/// A line's clusters as displayed: from each `char`'s edges (`places`, as
/// [`TextSystem::places`] returns them), in a right-to-left (`leftward`) paragraph or not.
/// `start` is the line's offset in its text.
pub(crate) fn shown_clusters(line: &str, start: usize, places: &[[f32; 2]], leftward: bool) -> Vec<ShownCluster> {
    let mut order = crate::bidi::Order::default();
    order.of(line, Some(leftward));
    let (mut clusters, mut at, mut n) = (Vec::new(), 0, 0);
    while at < line.len() {
        let next = at + cluster(&line[at..]);
        let count = line[at..next].chars().count();
        let own = places.get(n..n + count).unwrap_or(&[]);
        let edges = own.iter().fold([f32::MAX, f32::MIN], |[left, right], [l, r]| [left.min(*l), right.max(*r)]);
        let edges = if own.is_empty() { [0.0; 2] } else { edges };
        clusters.push(ShownCluster { bytes: start + at..start + next, edges, leftward: order.levels.get(n).is_some_and(|level| level % 2 == 1) });
        (at, n) = (next, n + count);
    }
    clusters
}

/// Where a cursor at byte `at` shows among a line's clusters: at the near edge of the one after
/// it (`after`), else the far edge of the one before. Where directions meet on a line those
/// differ; elsewhere they coincide.
pub(crate) fn caret(clusters: &[ShownCluster], at: usize, after: bool) -> f32 {
    let near = |one: &ShownCluster| if one.leftward { one.edges[1] } else { one.edges[0] };
    let far = |one: &ShownCluster| if one.leftward { one.edges[0] } else { one.edges[1] };
    let (next, before) = (clusters.iter().find(|one| one.bytes.start == at), clusters.iter().find(|one| one.bytes.end == at));
    match (next, before) {
        (Some(next), _) if after => near(next),
        (_, Some(before)) => far(before),
        (Some(next), None) => near(next),
        (None, None) => 0.0,
    }
}

/// What is marked on an edited line: the ranges its selection covers, each left to right (in
/// `marked`); and where the cursor shows, if on this line: as `caret` says (`after`), and
/// where it shows the other way. Those two differ where directions meet, and a split cursor
/// shows at both. The line is `line`, `wide` units wide, starting at byte `start` of its
/// text, in a right-to-left (`leftward`) paragraph or not. The selection is bytes `from` to
/// `to`: one continuing past the line's end includes room for the line break, on the
/// paragraph's ending side. The cursor is at byte `at`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn marks(
    text: &mut dyn TextSystem,
    line: &str,
    start: usize,
    wide: f32,
    font: Font,
    scale: f32,
    leftward: bool,
    [from, to]: [usize; 2],
    at: Option<usize>,
    after: bool,
    marked: &mut Vec<[f32; 2]>,
) -> Option<Carets> {
    marked.clear();
    let end = start + line.len();
    let selected = from < to && from <= end && to > start;
    // Text without right-to-left characters: each character after the previous, as measured.
    if crate::bidi::plain(line) && !leftward {
        let mut width = |to: usize| text.measure(&line[..to.saturating_sub(start).min(line.len())], font, scale, None);
        if selected {
            let right = if to > end { wide + 4.0 } else { width(to) };
            marked.push([width(from), right]);
        }
        return at.map(width).map(alone);
    }
    let mut places = Vec::new();
    text.places(line, font, scale, leftward, &mut places);
    let clusters = shown_clusters(line, start, &places, leftward);
    if selected {
        let mut spans: Vec<[f32; 2]> = clusters.iter().filter(|one| one.bytes.start >= from && one.bytes.end <= to).map(|one| one.edges).collect();
        if to > end {
            spans.push(if leftward { [-4.0, 0.0] } else { [wide, wide + 4.0] });
        }
        spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
        for span in spans {
            match marked.last_mut() {
                Some(last) if span[0] <= last[1] + 0.01 => last[1] = last[1].max(span[1]),
                _ => marked.push(span),
            }
        }
    }
    // (Without `right-to-left` every line shows as typed: the cursor is never split.)
    at.map(|at| carets(&clusters, at, after))
}

/// Where a cursor shows on a line (see `marks`): with `right-to-left`, where it stands and
/// where it would stand the other way, which differ where directions meet; without, one place.
#[cfg(feature = "right-to-left")]
pub(crate) type Carets = [f32; 2];
#[cfg(not(feature = "right-to-left"))]
pub(crate) type Carets = f32;

/// The places of a cursor at byte `at` among a line's clusters, as `caret` has them.
#[cfg(feature = "right-to-left")]
fn carets(clusters: &[ShownCluster], at: usize, after: bool) -> Carets {
    [caret(clusters, at, after), caret(clusters, at, !after)]
}
#[cfg(not(feature = "right-to-left"))]
fn carets(clusters: &[ShownCluster], at: usize, after: bool) -> Carets {
    caret(clusters, at, after)
}

/// A cursor at one place only.
#[cfg(feature = "right-to-left")]
fn alone(x: f32) -> Carets {
    [x; 2]
}
#[cfg(not(feature = "right-to-left"))]
fn alone(x: f32) -> Carets {
    x
}

// ---- the built-in font ----

/// The classic 5×7 dot font, one byte per column with the top dot in the lowest bit: characters
/// from space to `~`.
#[rustfmt::skip]
const DOTS: [[u8; 5]; 95] = [
    [0x00,0x00,0x00,0x00,0x00],[0x00,0x00,0x5F,0x00,0x00],[0x00,0x07,0x00,0x07,0x00],[0x14,0x7F,0x14,0x7F,0x14],[0x24,0x2A,0x7F,0x2A,0x12],[0x23,0x13,0x08,0x64,0x62],[0x36,0x49,0x55,0x22,0x50],[0x00,0x05,0x03,0x00,0x00],
    [0x00,0x1C,0x22,0x41,0x00],[0x00,0x41,0x22,0x1C,0x00],[0x14,0x08,0x3E,0x08,0x14],[0x08,0x08,0x3E,0x08,0x08],[0x00,0x50,0x30,0x00,0x00],[0x08,0x08,0x08,0x08,0x08],[0x00,0x60,0x60,0x00,0x00],[0x20,0x10,0x08,0x04,0x02],
    [0x3E,0x51,0x49,0x45,0x3E],[0x00,0x42,0x7F,0x40,0x00],[0x42,0x61,0x51,0x49,0x46],[0x21,0x41,0x45,0x4B,0x31],[0x18,0x14,0x12,0x7F,0x10],[0x27,0x45,0x45,0x45,0x39],[0x3C,0x4A,0x49,0x49,0x30],[0x01,0x71,0x09,0x05,0x03],
    [0x36,0x49,0x49,0x49,0x36],[0x06,0x49,0x49,0x29,0x1E],[0x00,0x36,0x36,0x00,0x00],[0x00,0x56,0x36,0x00,0x00],[0x08,0x14,0x22,0x41,0x00],[0x14,0x14,0x14,0x14,0x14],[0x00,0x41,0x22,0x14,0x08],[0x02,0x01,0x51,0x09,0x06],
    [0x32,0x49,0x79,0x41,0x3E],[0x7E,0x11,0x11,0x11,0x7E],[0x7F,0x49,0x49,0x49,0x36],[0x3E,0x41,0x41,0x41,0x22],[0x7F,0x41,0x41,0x22,0x1C],[0x7F,0x49,0x49,0x49,0x41],[0x7F,0x09,0x09,0x09,0x01],[0x3E,0x41,0x49,0x49,0x7A],
    [0x7F,0x08,0x08,0x08,0x7F],[0x00,0x41,0x7F,0x41,0x00],[0x20,0x40,0x41,0x3F,0x01],[0x7F,0x08,0x14,0x22,0x41],[0x7F,0x40,0x40,0x40,0x40],[0x7F,0x02,0x0C,0x02,0x7F],[0x7F,0x04,0x08,0x10,0x7F],[0x3E,0x41,0x41,0x41,0x3E],
    [0x7F,0x09,0x09,0x09,0x06],[0x3E,0x41,0x51,0x21,0x5E],[0x7F,0x09,0x19,0x29,0x46],[0x46,0x49,0x49,0x49,0x31],[0x01,0x01,0x7F,0x01,0x01],[0x3F,0x40,0x40,0x40,0x3F],[0x1F,0x20,0x40,0x20,0x1F],[0x3F,0x40,0x38,0x40,0x3F],
    [0x63,0x14,0x08,0x14,0x63],[0x07,0x08,0x70,0x08,0x07],[0x61,0x51,0x49,0x45,0x43],[0x00,0x7F,0x41,0x41,0x00],[0x02,0x04,0x08,0x10,0x20],[0x00,0x41,0x41,0x7F,0x00],[0x04,0x02,0x01,0x02,0x04],[0x40,0x40,0x40,0x40,0x40],
    [0x00,0x01,0x02,0x04,0x00],[0x20,0x54,0x54,0x54,0x78],[0x7F,0x48,0x44,0x44,0x38],[0x38,0x44,0x44,0x44,0x20],[0x38,0x44,0x44,0x48,0x7F],[0x38,0x54,0x54,0x54,0x18],[0x08,0x7E,0x09,0x01,0x02],[0x0C,0x52,0x52,0x52,0x3E],
    [0x7F,0x08,0x04,0x04,0x78],[0x00,0x44,0x7D,0x40,0x00],[0x20,0x40,0x44,0x3D,0x00],[0x7F,0x10,0x28,0x44,0x00],[0x00,0x41,0x7F,0x40,0x00],[0x7C,0x04,0x18,0x04,0x78],[0x7C,0x08,0x04,0x04,0x78],[0x38,0x44,0x44,0x44,0x38],
    [0x7C,0x14,0x14,0x14,0x08],[0x08,0x14,0x14,0x18,0x7C],[0x7C,0x08,0x04,0x04,0x08],[0x48,0x54,0x54,0x54,0x20],[0x04,0x3F,0x44,0x40,0x20],[0x3C,0x40,0x40,0x20,0x7C],[0x1C,0x20,0x40,0x20,0x1C],[0x3C,0x40,0x30,0x40,0x3C],
    [0x44,0x28,0x10,0x28,0x44],[0x0C,0x50,0x50,0x50,0x3C],[0x44,0x64,0x54,0x4C,0x44],[0x00,0x08,0x36,0x41,0x00],[0x00,0x00,0x7F,0x00,0x00],[0x00,0x41,0x36,0x08,0x00],[0x08,0x04,0x08,0x10,0x08],
];
/// The glyph for a character the built-in font lacks; and its two extra characters: the
/// ellipsis truncated text ends with, and the dot standing for a password character.
const UNKNOWN: [u8; 5] = [0x7F, 0x41, 0x41, 0x41, 0x7F];
const DOTTED: [u8; 5] = [0x40, 0x00, 0x40, 0x00, 0x40];
const HIDDEN: [u8; 5] = [0x00, 0x1C, 0x1C, 0x1C, 0x00];

/// A dot font that needs nothing from the system. Each dot is a square of whole pixels, so it
/// comes in only a few sizes: a fallback, not a pretty font.
#[derive(Debug, Default)]
pub struct BuiltInFont;

impl BuiltInFont {
    /// The dot size in pixels for this font size.
    fn dot(font: Font, scale: f32) -> i32 {
        ((font.size * scale / 8.0).round() as i32).max(1)
    }
}

impl TextSystem for BuiltInFont {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        // (Its caps are seven dots in a line of ten: half a dot off centre, which nothing can
        // fix. So it reports no cap height.)
        let dot = Self::dot(font, scale) as f32 / scale;
        Metrics { ascent: 8.0 * dot, descent: 2.0 * dot, cap_height: 0.0, x_height: 0.0 }
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        let dot = Self::dot(font, scale) as f32 / scale;
        let wide = if font.face == Face::Bold { 7.0 } else { 6.0 } * dot;
        let mut x = 0.0;
        for _ in text.chars() {
            x += wide;
            if let Some(stops) = stops.as_deref_mut() {
                stops.push(x);
            }
        }
        x
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let dot = Self::dot(font, scale);
        let bold = font.face == Face::Bold;
        let (wide, columns) = (if bold { 7 } else { 6 } * dot, if bold { 6 } else { 5 });
        // A glyph is seven rows of dots; each dot a square of pixels, twice as wide for bold.
        // It is built a pixel row at a time, then composited in one go.
        let (dot, across, thick) = (dot as usize, (columns * dot) as usize, (if bold { 2 * dot } else { dot }) as usize);
        let mut cover = vec![0u8; across * 7 * dot];
        let (mut px, py) = ((x * scale).round() as i32, (y * scale).round() as i32 + dot as i32);
        for c in text.chars() {
            let glyph = match c {
                '…' => &DOTTED,
                '•' => &HIDDEN,
                _ => DOTS.get((c as usize).wrapping_sub(32)).unwrap_or(&UNKNOWN),
            };
            cover.fill(0);
            for (at, line) in cover.chunks_exact_mut(across).enumerate() {
                let on = (0..5).filter(|column| glyph[*column] >> (at / dot) & 1 == 1);
                on.for_each(|column| line[column * dot..column * dot + thick].fill(255));
            }
            canvas.cover(px, py, across, &cover, color);
            px += wide;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(text: &str, max_chars: usize) -> Vec<&str> {
        let mut stops = Vec::new();
        BuiltInFont.measure(text, Font::new(8.0), 1.0, Some(&mut stops));
        let mut lines = Vec::new();
        wrap(text, &stops, max_chars as f32 * 6.0, &mut lines);
        lines.iter().map(|l| &text[l.start..l.end]).collect()
    }

    #[test]
    fn lines_break_at_spaces() {
        assert_eq!(lines_of("fix the jump arc on slopes", 12), ["fix the jump", "arc on", "slopes"]);
        assert_eq!(lines_of("short", 12), ["short"]);
    }

    #[test]
    fn a_word_longer_than_a_line_is_cut() {
        assert_eq!(lines_of("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn line_ends_are_kept() {
        assert_eq!(lines_of("one\n\ntwo\n", 20), ["one", "", "two", ""]);
        assert_eq!(lines_of("", 20), [""]);
    }

    #[test]
    fn letters_are_drawn_and_counted() {
        let mut px = vec![0u32; 40 * 12];
        let mut canvas = Canvas::new(&mut px, 40, 12, (0, 0), 1.0);
        BuiltInFont.draw(&mut canvas, "Hi!", 1.0, 1.0, Font::new(8.0), Color::WHITE);
        assert!(px.iter().filter(|p| **p != 0).count() > 20);
        assert_eq!(BuiltInFont.measure("Hi!", Font::new(8.0), 1.0, None), 18.0);
        assert_eq!(BuiltInFont.metrics(Font::new(8.0), 1.0).height(), 10.0);
        // A font with ascent 12, descent 3 and cap height 9, in a slot as tall as its line: its
        // cap centre is 4.5 above the baseline, as is the line's centre, so it stays put. With
        // ascent 14 and descent 3 the caps would sit a pixel low, and are drawn that much
        // higher; in a slot a pixel taller, half a pixel higher. A font whose glyphs sit high is
        // drawn lower; one without a cap height is centred by ascent and descent.
        let lift = |ascent: f32, descent: f32, cap: f32, height: f32| Metrics { ascent, descent, cap_height: cap, x_height: 0.0 }.lift(height);
        assert_eq!([lift(12.0, 3.0, 9.0, 15.0), lift(14.0, 3.0, 9.0, 17.0), lift(14.0, 3.0, 9.0, 18.0), lift(8.0, 6.0, 8.0, 14.0)], [0.0, 1.0, 0.5, -3.0]);
        assert_eq!([lift(13.0, 3.0, 0.0, 16.0), lift(13.0, 3.0, 0.0, 17.0)], [0.0, -0.5]);
    }

    /// A text split into grapheme clusters.
    fn characters(text: &str) -> Vec<&str> {
        let starts = std::iter::successors(Some(0), |at| Some(at + cluster(&text[*at..])).filter(|next| next > at));
        starts.take_while(|at| *at < text.len()).map(|at| &text[at..at + cluster(&text[at..])]).collect()
    }

    #[test]
    fn a_character_as_a_person_counts_them_may_be_several() {
        assert_eq!(characters("abc \u{e9}\u{4e2d}"), ["a", "b", "c", " ", "\u{e9}", "\u{4e2d}"]);
        assert_eq!((characters(""), cluster("")), (vec![], 0));
        // A letter with following accents; a kana with its mark; CR LF, but not LF CR, and
        // nothing attaches to a control.
        assert_eq!(characters("e\u{301}x\u{323}\u{302}y"), ["e\u{301}", "x\u{323}\u{302}", "y"]);
        assert_eq!(characters("\u{304b}\u{3099}\u{5d1}\u{5b0}\u{e01}\u{e34}"), ["\u{304b}\u{3099}", "\u{5d1}\u{5b0}", "\u{e01}\u{e34}"]);
        assert_eq!(characters("a\r\nb\n\r\u{301}"), ["a", "\r\n", "b", "\n", "\r", "\u{301}"]);
        // Regional indicators pair from the first; a third stands alone.
        let (d, e, f) = ("\u{1f1e9}", "\u{1f1ea}", "\u{1f1eb}");
        assert_eq!(characters(&[d, e, f, d, f, "x", e].concat()), ["\u{1f1e9}\u{1f1ea}", "\u{1f1eb}\u{1f1e9}", f, "x", e]);
        // A hand with skin tone; a keycap; a tag-sequence flag; a heart with emoji
        // presentation.
        let all = ["\u{1f44d}\u{1f3fd}", "1\u{fe0f}\u{20e3}", "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}", "\u{2764}\u{fe0f}", "\u{1f600}"];
        assert_eq!(characters(&all.concat()), all);
        // Pictographs joined by ZWJs are one, with whatever attaches to each: a family, and a
        // couple of different skin tones with a heart.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let couple = "\u{1f469}\u{1f3fb}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f468}\u{1f3ff}";
        assert_eq!(characters(&[family, couple, "a", family].concat()), [family, couple, "a", family]);
        // ZWJ only joins pictographs: letters stay two (the ZWJ goes with the first), as do a
        // pictograph and a letter, or pictographs with two ZWJs between them.
        assert_eq!(characters("a\u{200d}b\u{1f600}\u{200d}c"), ["a\u{200d}", "b", "\u{1f600}\u{200d}", "c"]);
        assert_eq!(characters("\u{1f600}\u{200d}\u{200d}\u{1f600}"), ["\u{1f600}\u{200d}\u{200d}", "\u{1f600}"]);
        // From any position, the cluster start is found by going back: here from every byte of
        // a text containing all of these.
        let text = ["ab", "e\u{301}", d, e, f, d, f, "\r\n", family, "x", couple, "\u{1f44d}\u{1f3fd}", "\u{301}", family, family, "1\u{fe0f}\u{20e3}", "z"].concat();
        let mut start = 0;
        for character in characters(&text) {
            let inside = (start + 1..=start + character.len()).filter(|at| text.is_char_boundary(*at));
            inside.for_each(|at| assert_eq!(cluster_before(&text, at), start, "from byte {at}, in {character:?}"));
            start += character.len();
        }
        assert_eq!((start, cluster_before(&text, 0)), (text.len(), 0));
    }

    #[test]
    #[cfg(feature = "brahmic")]
    fn every_case_of_unicodes_test_of_characters_as_a_person_counts_them_comes_out_as_it_says() {
        // Unicode's own GraphemeBreakTest.txt (one case per line: code points in hex, ÷ at
        // boundaries, × within), from the file `TAUT_GRAPHEMES` names. Each case is split
        // forward from its start, and found backward from every position.
        let Some(path) = std::env::var_os("TAUT_GRAPHEMES") else { return eprintln!("TAUT_GRAPHEMES names no file: not tested") };
        let all = std::fs::read_to_string(path).expect("Unicode's test of characters");
        let mut cases = 0;
        for line in all.lines().map(|line| line.split('#').next().unwrap_or("").trim()).filter(|line| !line.is_empty()) {
            let (mut written, mut ends) = (String::new(), Vec::new());
            for word in line.split_whitespace() {
                match word {
                    "÷" => ends.push(written.len()),
                    "×" => {}
                    number => written.push(char::from_u32(u32::from_str_radix(number, 16).expect("a number")).expect("a character")),
                }
            }
            let starts = std::iter::successors(Some(0), |at| Some(at + cluster(&written[*at..])).filter(|next| *next > *at));
            let found: Vec<usize> = starts.take_while(|at| *at < written.len()).chain([written.len()]).collect();
            assert_eq!(found, ends, "{line}");
            for (start, end) in ends.iter().zip(&ends[1..]) {
                let inside = (start + 1..=*end).filter(|at| written.is_char_boundary(*at));
                inside.for_each(|at| assert_eq!(cluster_before(&written, at), *start, "{line}: from byte {at}"));
            }
            cases += 1;
        }
        assert!(cases > 700, "{cases} cases");
        println!("{cases} cases, all as Unicode says");
    }

    #[test]
    #[cfg(feature = "brahmic")]
    fn a_syllable_is_one_character_to_a_person() {
        // A consonant with its vowel sign; a virama conjunct of two consonants with its vowel
        // sign (Unicode 15.1's rule), and likewise Khmer consonant, coeng, consonant; Thai sara
        // am; but a Tamil consonant with virama, then another, are two (Unicode does not count
        // Tamil's virama as a linker).
        assert_eq!(characters("\u{915}\u{93f}\u{915}\u{94d}\u{937}\u{93f}"), ["\u{915}\u{93f}", "\u{915}\u{94d}\u{937}\u{93f}"]);
        assert_eq!(characters("\u{e19}\u{e49}\u{e33}"), ["\u{e19}\u{e49}\u{e33}"]);
        assert_eq!(characters("\u{1780}\u{17d2}\u{179a}"), ["\u{1780}\u{17d2}\u{179a}"]);
        assert_eq!(characters("\u{b95}\u{bcd}\u{bb7}"), ["\u{b95}\u{bcd}", "\u{bb7}"]);
    }

    #[test]
    #[cfg(feature = "brahmic")]
    fn thai_breaks_between_words_where_the_system_knows_them_and_else_between_syllables() {
        // ภาษาไทยเป็นภาษาที่สวยงาม ("Thai is a beautiful language"), each character 10 wide
        // except zero-width marks. No line ends with a preposed vowel, and none starts with a
        // vowel or sign that follows its consonant; and on Windows, where no word is wider than
        // a line (the widest, สวยงาม, is 60), every line ends at a Windows word boundary.
        let text = "\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22}\u{e40}\u{e1b}\u{e47}\u{e19}\u{e20}\u{e32}\u{e29}\u{e32}\u{e17}\u{e35}\u{e48}\u{e2a}\u{e27}\u{e22}\u{e07}\u{e32}\u{e21}";
        let mark = |letter: char| matches!(letter as u32, 0xe31 | 0xe34..=0xe3a | 0xe47..=0xe4e);
        let stops: Vec<f32> = text.chars().scan(0.0, |x, letter| Some((*x += if mark(letter) { 0.0 } else { 10.0 }, *x).1)).collect();
        let words = crate::line_breaks::words(text);
        for max in [35.0, 45.0, 60.0, 80.0] {
            let mut lines = Vec::new();
            wrap(text, &stops, max, &mut lines);
            for line in &lines[1..] {
                let (letters, n) = (text.chars().collect::<Vec<_>>(), text[..line.start].chars().count());
                assert!(crate::line_breaks::between(&letters, n), "{max}: a syllable broken at {n}");
                assert!(words.is_empty() || words.contains(&n) || max < 60.0, "{max}: not between words at {n}");
            }
        }
    }

    #[test]
    fn a_line_never_breaks_inside_a_character_of_several() {
        // Two letters 6 wide, a flag 18 wide (its two `char`s end together), and two more
        // letters. In 20 the letters fit and the flag moves whole to the next line; in 10, less
        // than the flag's width, it still gets a line to itself.
        let (text, stops) = ("ab\u{1f1e9}\u{1f1ea}cd", [6.0, 12.0, 30.0, 30.0, 36.0, 42.0]);
        let broken = |max: f32| {
            let mut lines = Vec::new();
            wrap(text, &stops, max, &mut lines);
            lines.iter().map(|line| (&text[line.start..line.end], line.width)).collect::<Vec<_>>()
        };
        assert_eq!(broken(20.0), [("ab", 12.0), ("\u{1f1e9}\u{1f1ea}", 18.0), ("cd", 12.0)]);
        assert_eq!(broken(10.0), [("a", 6.0), ("b", 6.0), ("\u{1f1e9}\u{1f1ea}", 18.0), ("c", 6.0), ("d", 6.0)]);
    }
}
