//! Rich text: a paragraph of differently styled words, with links, icons and small pictures
//! inline ([`Ui::rich`]); and a displayed document (`Writing`): its blocks (headings,
//! paragraphs, code, quotes, lists and items, tables), each before its content as boxes are,
//! and each block's text in runs of one style. Markdown parses into that (`markdown.rs`), and
//! `markdown_view.rs` draws it.
//!
//! Both are laid out here, the same way. Each run of text is measured once: where each
//! character ends. A paragraph's runs are placed end to end on one long line, which is broken
//! into rows no wider than the room: after the last space that fits, at a newline, or, for a
//! word longer than a row, where it must be. Each row is as tall as its tallest glyphs, all on
//! one baseline. What a run has on a row then becomes one text box, positioned in the
//! paragraph's box: measured, drawn and diffed against the last run as any label is, so a
//! letter costs here what it does in a label. Only a new width re-breaks rows; only new text or
//! fonts re-measure.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::Cursor;
use crate::id::{ById, hash};
use crate::layout::{Align, Flow, Length};
use crate::text::{Decoration, Face, Font, TextSystem, cluster};
use crate::ui::{Icon, Measure, Picture, Ui};
use std::ops::Range;

/// The padding a background ([`Span::background`], inline code) leaves each side of its text.
pub(crate) const PAD: f32 = 3.0;

/// A document block's kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    /// The whole document: everything is inside.
    Page,
    /// Wrapping text.
    Paragraph,
    /// A heading, level 1 (largest) to 6.
    Heading(u8),
    /// Verbatim monospace code; and its language: its index in the document's list, from 1 (0
    /// for none named).
    Code(u32),
    /// A block quote.
    Quote,
    /// A list, numbered or bulleted; and whether tight (no space between items).
    List { numbered: bool, tight: bool },
    /// A list item: its number, in a numbered list; and for a task, whether done.
    Item { number: u32, ticked: Option<bool> },
    /// A thematic break.
    Rule,
    /// Rows of cells, the first being headers.
    Table,
    /// A table row, and whether it is the header row.
    Row { head: bool },
    /// A cell, and its text alignment.
    Cell(Align),
}

/// A document block. Blocks are listed each before its content, as boxes are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Block {
    pub kind: Kind,
    /// Its parent block.
    pub parent: u32,
    /// The first block after all its content.
    pub end: u32,
    /// Its runs, in the document's list of them.
    pub pieces: [u32; 2],
    /// Its text, as a byte range of the document's text.
    pub words: [u32; 2],
}

/// What a run is: text, or something inline among it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Thing {
    Words,
    /// A picture the app names (by name index): its text is the alt text.
    Named(u32),
    /// An icon, as tall as the text and in its colour.
    Icon(Icon),
    /// A kept picture, and its displayed width and height.
    Picture(Picture, [f32; 2]),
    /// Text with its reading (ruby) over it: the reading's bytes. (Its words are the base.)
    #[cfg(feature = "ruby")]
    Ruby([u32; 2]),
}

/// A run's style: anything unset follows the block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    /// Monospace.
    pub mono: bool,
    /// A line through the text, or none.
    pub rule: Decoration,
    /// Its link, from 1; 0 for none.
    pub link: u32,
}

/// A run of a block's text in one style, or an inline thing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stretch {
    /// As a byte range of the document's text.
    pub words: [u32; 2],
    pub look: TextStyle,
    pub thing: Thing,
}

/// A displayed document: its text, blocks and runs; and the languages its code blocks name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Writing {
    /// Every block's text in order, each followed by a newline.
    pub words: String,
    pub blocks: Vec<Block>,
    pub pieces: Vec<Stretch>,
    pub languages: Vec<String>,
}

/// The character standing in the text for a picture without alt text.
pub(crate) const THING: char = '\u{fffc}';

/// Once every block is written: each one's end among the blocks, and each container's text (all
/// of its children's).
pub(crate) fn finish(writing: &mut Writing) {
    for at in (0..writing.blocks.len()).rev() {
        let block = writing.blocks[at];
        let end = block.end.max(at as u32 + 1);
        writing.blocks[at].end = end;
        if let Some(parent) = writing.blocks.get_mut(block.parent as usize) {
            (parent.end, parent.words[1]) = (parent.end.max(end), parent.words[1].max(block.words[1]));
        }
    }
}

// ---- pieces of rich text, as an app gives them ----

/// A piece of a rich text paragraph ([`Ui::rich`]): text in its own style, or an icon or small
/// picture inline. Anything unset follows the theme: its font and text colour.
///
/// ```ignore
/// let pressed = ui.rich(&[Span::text("Read the "), Span::text("guide").link(), Span::text(" first.").bold()]);
/// if pressed == Some(1) { /* the guide */ }
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span<'a> {
    pub(crate) words: &'a str,
    pub(crate) look: TextStyle,
    /// Its font size: 0 for the theme's.
    pub(crate) size: f32,
    pub(crate) face: Option<Face>,
    pub(crate) color: Option<Color>,
    pub(crate) back: Option<Color>,
    pub(crate) thing: Thing,
    /// Its reading (ruby): empty for none.
    #[cfg(feature = "ruby")]
    pub(crate) ruby: &'a str,
    /// In vertical text, set upright across one em (tate-chu-yoko).
    #[cfg(feature = "ruby")]
    pub(crate) upright: bool,
}

impl<'a> Span<'a> {
    /// Text, in the theme's font and text colour.
    pub const fn text(words: &'a str) -> Self {
        let look = TextStyle { bold: false, italic: false, mono: false, rule: Decoration::None, link: 0 };
        Span {
            words,
            look,
            size: 0.0,
            face: None,
            color: None,
            back: None,
            thing: Thing::Words,
            #[cfg(feature = "ruby")]
            ruby: "",
            #[cfg(feature = "ruby")]
            upright: false,
        }
    }
    /// An icon, as tall as the surrounding text, in its colour.
    pub const fn icon(icon: Icon) -> Span<'static> {
        Span { thing: Thing::Icon(icon), ..Span::text("") }
    }
    /// A picture added with [`Ui::add_picture`], shown `width` by `height` units, on the
    /// baseline (the row grows for it where needed).
    pub const fn picture(picture: Picture, width: f32, height: f32) -> Span<'static> {
        Span { thing: Thing::Picture(picture, [width, height]), ..Span::text("") }
    }
    /// Bold.
    pub const fn bold(mut self) -> Self {
        self.look.bold = true;
        self
    }
    /// Italic.
    pub const fn italic(mut self) -> Self {
        self.look.italic = true;
        self
    }
    /// Monospace on its own background: inline code.
    pub const fn code(mut self) -> Self {
        self.look.mono = true;
        self
    }
    /// Its font size, in units (as [`Font::size`] counts).
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    /// Another face: a system font by name, a specific weight (`Face::Regular.weight(600)`), or
    /// an app font.
    pub const fn face(mut self, face: Face) -> Self {
        self.face = Some(face);
        self
    }
    /// In this colour.
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
    /// On this background colour: a search hit, a highlighted name.
    pub const fn background(mut self, color: Color) -> Self {
        self.back = Some(color);
        self
    }
    /// Underlined.
    pub const fn underlined(mut self) -> Self {
        self.look.rule = Decoration::Underline;
        self
    }
    /// Struck through.
    pub const fn strikethrough(mut self) -> Self {
        self.look.rule = Decoration::Strikethrough;
        self
    }
    /// Its reading (ruby, furigana): small text over it (at its right in vertical text), as
    /// CSS `<ruby>` sets it. Screen readers hear it after the text, in brackets. (Feature
    /// `ruby`.)
    #[cfg(feature = "ruby")]
    pub fn ruby(mut self, reading: &'a str) -> Self {
        crate::rich_ruby::RUBY.get_or_init(|| &crate::rich_ruby::HOOKS);
        self.ruby = reading;
        self
    }
    /// In vertical text ([`Ui::vertical_rich`]), set upright across one em, squeezed to fit:
    /// tate-chu-yoko, as CSS `text-combine-upright: all` sets a few digits or letters. (Feature
    /// `ruby`; [`Ui::upright_digits`] needs none.)
    #[cfg(feature = "ruby")]
    pub const fn upright(mut self) -> Self {
        self.upright = true;
        self
    }
    /// A link: in the accent colour, underlined while hovered or focused, activated by a click,
    /// and by Enter once Tab reaches it. [`Ui::rich`] says which was pressed; screen readers
    /// hear a link.
    pub const fn link(mut self) -> Self {
        self.look.link = 1;
        self
    }
}

/// The face a style uses within face `face`: monospace, then bold, then italic, as the style
/// says.
pub(crate) fn face_of(face: Face, look: TextStyle) -> Face {
    let face = if look.mono { Face::Mono } else { face };
    let face = match (look.bold, face) {
        (false, face) | (true, face @ Face::Bold) => face,
        (true, Face::Regular) => Face::Bold,
        (true, face) => face.weight(700),
    };
    match (look.italic, face) {
        (false, face) => face,
        (true, Face::Regular) => Face::Italic,
        (true, face) => face.italic(),
    }
}

// ---- laying out ----

/// A piece being laid out: its text (bytes of a text kept beside it), its style, and once
/// measured, its start on the paragraph's long line and its ascent and descent.
#[derive(Clone, Copy)]
pub(crate) struct Bit {
    pub words: [u32; 2],
    pub font: Font,
    pub color: Color,
    /// Its background: [`Color::NONE`] for none.
    pub back: Color,
    /// Its link, from 1; 0 for none.
    pub link: u32,
    pub thing: Thing,
    /// Where its character ends start among the stops; where its text starts on the long line,
    /// and its width (a thing's width).
    pub stops: u32,
    pub x: f32,
    pub wide: f32,
    /// Its ascent and descent from the baseline (each lifted as a single line is, see
    /// [`Metrics::lift`](crate::text::Metrics::lift)).
    pub above: f32,
    pub below: f32,
}

impl Bit {
    /// A text piece, or a thing, not yet measured.
    pub fn new(words: [u32; 2], font: Font, color: Color, back: Color, link: u32, thing: Thing) -> Bit {
        Bit { words, font, color, back, link, thing, stops: 0, x: 0.0, wide: 0.0, above: 0.0, below: 0.0 }
    }
}

/// A laid-out paragraph row: its fragment range; its left edge and top (as the paragraph was
/// laid out); its height and baseline; the text byte range it covers; and whether its
/// paragraph runs right to left.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub frags: [u32; 2],
    pub left: f32,
    pub top: f32,
    pub high: f32,
    pub base: f32,
    pub words: [u32; 2],
    pub leftward: bool,
}

/// What one piece has on one row: the piece; its text bytes; its first character's stop; its
/// offset from the row's left edge (and into the piece's text); its width, and a thing's
/// height; its box's offset down the row; for a link piece, which of that link's fragments it
/// is, from 0; and its bidi level (odd: it reads right to left).
///
/// Fragments are kept in text order; a row with right-to-left text has them split where the
/// level changes, and placed in display order (`x`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Frag {
    pub bit: u32,
    pub words: [u32; 2],
    pub first: u32,
    pub x: f32,
    pub into: f32,
    pub wide: f32,
    pub high: f32,
    pub y: f32,
    pub part: u32,
    pub level: u8,
}

impl Frag {
    /// Whether it reads right to left. (Never without `right-to-left`: what handles it is then
    /// left out.)
    fn leftward(&self) -> bool {
        cfg!(feature = "right-to-left") && self.level % 2 == 1
    }
}

impl Row {
    /// Whether its fragments may be out of text order: some read right to left, or its
    /// paragraph does. (Never without `right-to-left`.)
    fn turned(&self, frags: &[Frag]) -> bool {
        cfg!(feature = "right-to-left") && (self.leftward || frags.iter().any(Frag::leftward))
    }
}

/// Laid-out text: its pieces, their character ends, and its rows of fragments.
#[derive(Default)]
pub(crate) struct Laid {
    pub bits: Vec<Bit>,
    pub stops: Vec<f32>,
    pub rows: Vec<Row>,
    pub frags: Vec<Frag>,
}

/// A font's ascent and descent, lifted as a single line is (see
/// [`Metrics::lift`](crate::text::Metrics::lift)): asked of the system once per font, cached in
/// `fonts`.
pub(crate) fn heights(system: &mut dyn TextSystem, font: Font, scale: f32, fonts: &mut Vec<(Font, [f32; 2])>) -> [f32; 2] {
    let plain = Font { decoration: Decoration::None, ..font };
    if let Some((_, known)) = fonts.iter().find(|(known, _)| *known == plain) {
        return *known;
    }
    let metrics = system.metrics(plain, scale);
    let lift = metrics.lift(metrics.height());
    let both = [metrics.ascent - lift, metrics.descent + lift];
    fonts.push((plain, both));
    both
}

impl Laid {
    /// Measures pieces `bits`, one paragraph's: each character end, glyph heights, and each
    /// piece's start on the paragraph's long line. Returns that line's length. (Consecutive
    /// same-font pieces without a background are measured together: a multicoloured line of
    /// code, a link among plain text. It is one system call for all, and they stay as the
    /// system would set them.)
    pub fn measure(&mut self, bits: Range<usize>, text: &str, system: &mut dyn TextSystem, scale: f32, fonts: &mut Vec<(Font, [f32; 2])>) -> f32 {
        let (mut x, mut at) = (0.0, bits.start);
        while at < bits.end {
            let first = self.bits[at];
            let alone = !matches!(first.thing, Thing::Words) || first.back != Color::NONE;
            let mut end = at + 1;
            while !alone
                && end < bits.end
                && self.bits[end].font == first.font
                && self.bits[end].back == Color::NONE
                && matches!(self.bits[end].thing, Thing::Words)
                && self.bits[end].words[0] == self.bits[end - 1].words[1]
            {
                end += 1;
            }
            x = self.measure_run(at..end, x, text, system, scale, fonts);
            at = end;
        }
        x
    }

    /// Measures a run of pieces measured together, starting `x` along the long line; returns
    /// where it ends.
    fn measure_run(&mut self, run: Range<usize>, x: f32, text: &str, system: &mut dyn TextSystem, scale: f32, fonts: &mut Vec<(Font, [f32; 2])>) -> f32 {
        let (first, last) = (self.bits[run.start], self.bits[run.end - 1]);
        let pad = if first.back == Color::NONE { 0.0 } else { PAD };
        let mut ends = Vec::new();
        let whole = match first.thing {
            Thing::Words | Thing::Named(_) => system.measure(&text[first.words[0] as usize..last.words[1] as usize], first.font, scale, Some(&mut ends)),
            Thing::Icon(_) => first.font.size,
            Thing::Picture(_, [wide, _]) => wide,
            #[cfg(feature = "ruby")]
            Thing::Ruby(_) => return crate::rich_ruby::RUBY.get().map_or(x, |ruby| (ruby.measure)(self, run.start, text, system, scale, x)),
        };
        let (heights, mut count) = (heights(system, first.font, scale, fonts), 0);
        for at in run {
            let bit = &mut self.bits[at];
            let words = matches!(bit.thing, Thing::Words | Thing::Named(_));
            let letters = if words { text[bit.words[0] as usize..bit.words[1] as usize].chars().count() } else { 0 };
            // (Each piece's ends count from its own start.)
            let before = if count == 0 { 0.0 } else { ends[count - 1] };
            let own = &ends[count..count + letters];
            let wide = match (words, own.last()) {
                (false, _) => whole,
                (true, last) => last.map_or(0.0, |end| end - before),
            };
            [bit.above, bit.below] = heights;
            (bit.x, bit.stops, bit.wide) = (x + pad + before, self.stops.len() as u32, wide);
            self.stops.extend(own.iter().map(|end| end - before));
            count += letters;
        }
        x + pad + whole + pad
    }

    /// The offset from row `row`'s left edge of the position before byte `at` of `text` (past
    /// the row's content: the row's end, on its paragraph's ending side).
    pub fn x_at(&self, text: &str, row: usize, at: u32) -> f32 {
        let row = self.rows[row];
        let frags = &self.frags[row.frags[0] as usize..row.frags[1] as usize];
        match frags.iter().find(|frag| at < frag.words[1]) {
            Some(frag) => self.frag_x(text, frag, at),
            None if row.leftward => frags.iter().map(|frag| frag.x).reduce(f32::min).unwrap_or(0.0),
            None => frags.iter().map(|frag| frag.x + frag.wide).reduce(f32::max).unwrap_or(0.0),
        }
    }

    /// The offset from its row's left edge of the position before byte `at` in a fragment
    /// (`at` within its text, or at its end): measured from its right edge where it reads right
    /// to left.
    fn frag_x(&self, text: &str, frag: &Frag, at: u32) -> f32 {
        let words = matches!(self.bits[frag.bit as usize].thing, Thing::Words | Thing::Named(_));
        let into = match at {
            _ if at <= frag.words[0] => 0.0,
            _ if at >= frag.words[1] || !words => frag.wide,
            _ => self.stops[(frag.first + text[frag.words[0] as usize..at as usize].chars().count() as u32 - 1) as usize] - frag.into,
        };
        if frag.leftward() { frag.x + frag.wide - into } else { frag.x + into }
    }

    /// The byte of `text` nearest `x` units from row `row`'s left edge: before or after the
    /// character under it, whichever edge is nearer (never inside a grapheme cluster).
    pub fn at_x(&self, text: &str, row: usize, x: f32) -> u32 {
        let row = self.rows[row];
        let frags = &self.frags[row.frags[0] as usize..row.frags[1] as usize];
        if !row.turned(frags) {
            return frags.iter().find(|frag| x < frag.x + frag.wide).map_or(row.words[1], |frag| self.at_frag(text, frag, x));
        }
        // (Fragments in display order are not in text order: the one under `x`, else the
        // nearest.)
        let away = |frag: &&Frag| (frag.x - x).max(x - frag.x - frag.wide).max(0.0);
        frags.iter().min_by(|a, b| away(a).total_cmp(&away(b))).map_or(row.words[0], |frag| self.at_frag(text, frag, x))
    }

    /// The byte nearest `x` in one fragment: before or after the character under it, read the
    /// way the fragment reads.
    fn at_frag(&self, text: &str, frag: &Frag, x: f32) -> u32 {
        // (Right to left, `x` is mirrored within the fragment: then it reads as left to right.)
        let x = if frag.leftward() { 2.0 * frag.x + frag.wide - x } else { x };
        match self.bits[frag.bit as usize].thing {
            Thing::Words | Thing::Named(_) => self.at_in(text, frag, x),
            _ => frag.words[(x >= frag.x + frag.wide / 2.0) as usize],
        }
    }

    /// The same within one text fragment read left to right.
    fn at_in(&self, text: &str, frag: &Frag, x: f32) -> u32 {
        let words = &text[frag.words[0] as usize..frag.words[1] as usize];
        let (mut at, mut left, mut count) = (0, frag.x, 0);
        while at < words.len() {
            let next = at + cluster(&words[at..]);
            count += words[at..next].chars().count() as u32;
            let right = frag.x + self.stops[(frag.first + count - 1) as usize] - frag.into;
            if x < (left + right) / 2.0 {
                return frag.words[0] + at as u32;
            }
            (at, left) = (next, right);
        }
        frag.words[1]
    }
}

/// A position in a paragraph being broken: a piece, a text byte, that character's stop, and its
/// offset along the long line.
#[derive(Clone, Copy, PartialEq)]
struct At {
    bit: u32,
    byte: u32,
    stop: u32,
    x: f32,
}

/// A width rows are never wrapped in (a code block's): they start at the left, whichever way
/// they read, as there is no right edge.
pub(crate) const NO_WRAP: f32 = f32::MAX;

/// Every row's minimum and placement: ascent and descent, row height; left edge, width, and
/// where narrower rows align.
#[derive(Clone, Copy)]
pub(crate) struct Room {
    pub above: f32,
    pub below: f32,
    pub high: f32,
    pub left: f32,
    pub wide: f32,
    pub side: Align,
}

/// Breaks a paragraph into rows.
struct Breaker<'a> {
    laid: &'a mut Laid,
    text: &'a str,
    room: Room,
    scale: f32,
    /// The current row's start, and its top.
    start: At,
    top: f32,
    /// The current row's last break opportunity: after a space, the row ending before that
    /// space and the next starting after it.
    space: Option<(At, At)>,
    /// The piece after the paragraph's last.
    end: u32,
    /// The paragraph runs right to left: its first strong letter does.
    leftward: bool,
}

/// Lays out pieces `bits` of `laid`, their text starting at byte `from` of `text`, as a
/// paragraph in `room` with its top at `top`: adds its rows and fragments. Returns its height.
pub(crate) fn lay(laid: &mut Laid, bits: Range<u32>, text: &str, from: u32, room: Room, top: f32, scale: f32) -> f32 {
    let start = laid.bits.get(bits.start as usize).filter(|_| !bits.is_empty()).map_or(At { bit: bits.start, byte: from, stop: 0, x: 0.0 }, |bit| At {
        bit: bits.start,
        byte: bit.words[0],
        stop: bit.stops,
        x: 0.0,
    });
    let text_end = laid.bits.get(bits.end.wrapping_sub(1) as usize).filter(|_| !bits.is_empty()).map_or(from, |bit| bit.words[1]);
    let leftward = crate::bidi::leftward(&text[start.byte as usize..text_end.max(start.byte) as usize]);
    let mut breaker = Breaker { laid, text, room, scale, start, top, space: None, end: bits.end, leftward };
    for at in bits.clone() {
        let bit = breaker.laid.bits[at as usize];
        match bit.thing {
            Thing::Words | Thing::Named(_) => breaker.words(at, bit),
            _ => breaker.thing(at, bit),
        }
    }
    let last = bits.end.checked_sub(1).filter(|_| !bits.is_empty()).map(|at| breaker.laid.bits[at as usize]);
    let end = last.map_or(start, |bit| At { bit: bits.end, byte: bit.words[1], stop: bit.stops, x: bit.x + bit.wide });
    breaker.row(end, end);
    breaker.top - top
}

impl Breaker<'_> {
    /// Walks a text piece's characters, breaking the row where full.
    fn words(&mut self, at: u32, bit: Bit) {
        let text = self.text;
        let mut left = bit.x;
        for (count, (byte, letter)) in text[bit.words[0] as usize..bit.words[1] as usize].char_indices().enumerate() {
            let stop = bit.stops + count as u32;
            let right = bit.x + self.laid.stops[stop as usize];
            let here = At { bit: at, byte: bit.words[0] + byte as u32, stop, x: left };
            let after = At { bit: at, byte: here.byte + letter.len_utf8() as u32, stop: stop + 1, x: right };
            // (A zero-width character, an accent, belongs to the one before: a row never breaks
            // before one.)
            match letter {
                '\n' => self.row(here, after),
                ' ' => self.space = Some((here, after)),
                _ if right - self.start.x > self.room.wide && here != self.start && right > left => self.full(here),
                _ => {}
            }
            left = right;
        }
    }

    /// An icon or picture: it moves to the next row where it does not fit.
    fn thing(&mut self, at: u32, bit: Bit) {
        let here = At { bit: at, byte: bit.words[0], stop: bit.stops, x: bit.x };
        if bit.x + bit.wide.min(self.room.wide) - self.start.x > self.room.wide && here != self.start {
            self.full(here);
        }
    }

    /// The row is full at `here`: it ends at its last space, or else right here.
    fn full(&mut self, here: At) {
        match self.space.take() {
            Some((end, next)) => self.row(end, next),
            None => self.row(here, here),
        }
    }

    /// Ends the current row at `end`, and starts the next at `next`.
    fn row(&mut self, end: At, next: At) {
        let first = self.laid.frags.len() as u32;
        for at in self.start.bit..end.bit.saturating_add(1).min(self.end) {
            self.frag(at, end);
        }
        let words = [self.start.byte, end.byte.max(self.start.byte)];
        if cfg!(feature = "right-to-left") && (self.leftward || !crate::bidi::plain(&self.text[words[0] as usize..words[1] as usize])) {
            self.turn_row(first as usize, words);
        }
        let frags = first as usize..self.laid.frags.len();
        let (mut above, mut below) = (self.room.above, self.room.below);
        for frag in &self.laid.frags[frags.clone()] {
            let bit = &self.laid.bits[frag.bit as usize];
            match bit.thing {
                Thing::Picture(..) => above = above.max(frag.high),
                _ => (above, below) = (above.max(bit.above), below.max(bit.below)),
            }
        }
        let pixel = |units: f32| (units * self.scale).round() / self.scale;
        let high = ((above + below).max(self.room.high) * self.scale).ceil() / self.scale;
        let base = pixel((high - above - below) / 2.0 + above);
        let wide = end.x - self.start.x;
        // (A right-to-left paragraph starts at the right, as a label's does, where it has one.)
        let side = [0.0, 0.5, 1.0, 0.0][self.room.side as usize];
        let side = if self.leftward && self.room.wide < NO_WRAP { 1.0 - side } else { side };
        let shift = pixel((self.room.wide - wide).max(0.0) * side);
        for frag in &mut self.laid.frags[frags] {
            let bit = &self.laid.bits[frag.bit as usize];
            frag.x += shift;
            frag.y = pixel(match bit.thing {
                Thing::Picture(..) => base - frag.high,
                Thing::Icon(_) => base - bit.above + (bit.above + bit.below - frag.high) / 2.0,
                _ => base - bit.above,
            });
        }
        let row = Row { frags: [first, self.laid.frags.len() as u32], left: self.room.left, top: self.top, high, base, words: [self.start.byte, end.byte], leftward: self.leftward };
        self.laid.rows.push(row);
        (self.start, self.space, self.top) = (next, None, self.top + high);
    }

    /// Puts the fragments of the row from `first` (text bytes `words`) in display order, as the
    /// bidi algorithm (UAX #9) has it: each split where its characters' level changes; every
    /// run of them at a level or above reversed, from the highest level down; then placed left
    /// to right in that order, each with its background's padding.
    fn turn_row(&mut self, first: usize, [from, to]: [u32; 2]) {
        let mut order = crate::bidi::Order::default();
        order.of(&self.text[from as usize..to as usize], Some(self.leftward));
        let row: Vec<Frag> = self.laid.frags.drain(first..).collect();
        let (mut letter, mut byte) = (0, from);
        for frag in row {
            letter += self.text[byte as usize..frag.words[0].max(byte) as usize].chars().count();
            letter = self.split(frag, letter, &order.levels);
            byte = frag.words[1].max(frag.words[0]).max(byte);
        }
        // (Pieces of a link split apart are its further fragments.)
        for at in first..self.laid.frags.len() {
            let (frag, before) = (self.laid.frags[at], at.checked_sub(1).map(|before| self.laid.frags[before]));
            let link = self.laid.bits[frag.bit as usize].link;
            let same = before.filter(|before| link != 0 && self.laid.bits[before.bit as usize].link == link);
            self.laid.frags[at].part = same.map_or(0, |before| before.part + 1);
        }
        let mut shown: Vec<usize> = (first..self.laid.frags.len()).collect();
        let levels = shown.iter().map(|at| self.laid.frags[*at].level);
        let [lowest, highest] = levels.fold([u8::MAX, 0], |[low, high], level| [low.min(level | 1), high.max(level)]);
        for level in (lowest..=highest).rev() {
            crate::bidi::turn(&mut shown, |at| self.laid.frags[*at].level >= level);
        }
        let mut pen = 0.0;
        for at in shown {
            let Laid { frags, bits, .. } = &mut self.laid;
            let bit = &bits[frags[at].bit as usize];
            let frag = &mut frags[at];
            let pad = |edge: bool| if bit.back != Color::NONE && edge { PAD } else { 0.0 };
            let (before, after) = (pad(frag.into <= 0.0), pad(frag.words[1] >= bit.words[1]));
            frag.x = pen + before;
            pen += before + frag.wide + after;
        }
    }

    /// Adds a row's fragment, split where its characters' level (`levels`, by character of the
    /// row, the fragment's first being `letter`) changes. Returns the character after it.
    fn split(&mut self, frag: Frag, letter: usize, levels: &[u8]) -> usize {
        let words = &self.text[frag.words[0] as usize..frag.words[1] as usize];
        let count = words.chars().count();
        if count == 0 || !matches!(self.laid.bits[frag.bit as usize].thing, Thing::Words | Thing::Named(_)) {
            // (A thing without text goes as the text either side does, where that agrees.)
            let near = [letter.checked_sub(1).and_then(|before| levels.get(before)), levels.get(letter)];
            let level = match near {
                _ if count > 0 => levels.get(letter).copied().unwrap_or(self.leftward as u8),
                [Some(before), Some(after)] if before % 2 == after % 2 => *before.min(after),
                _ => self.leftward as u8,
            };
            self.laid.frags.push(Frag { level, ..frag });
            return letter + count;
        }
        let mut piece = Frag { level: levels.get(letter).copied().unwrap_or(0), ..frag };
        for (n, (byte, _)) in words.char_indices().enumerate().skip(1) {
            let level = levels.get(letter + n).copied().unwrap_or(piece.level);
            if level == piece.level {
                continue;
            }
            let (cut, into) = (frag.words[0] + byte as u32, self.laid.stops[(frag.first + n as u32 - 1) as usize]);
            let head = Frag { words: [piece.words[0], cut], wide: into - piece.into, ..piece };
            self.laid.frags.push(head);
            piece = Frag { words: [cut, frag.words[1]], first: frag.first + n as u32, into, wide: piece.wide - head.wide, level, ..piece };
        }
        self.laid.frags.push(piece);
        letter + count
    }

    /// What piece `at` has on the row ending at `end`.
    fn frag(&mut self, at: u32, end: At) {
        let bit = self.laid.bits[at as usize];
        let from = if at == self.start.bit { self.start } else { At { bit: at, byte: bit.words[0], stop: bit.stops, x: bit.x } };
        let words = !matches!(bit.thing, Thing::Icon(_) | Thing::Picture(..));
        let (to, right) = if at == end.bit { (end.byte, end.x) } else { (bit.words[1], bit.x + bit.wide) };
        // (Text has nothing on a row that starts at its end or ends at its start; a thing is on
        // the row it starts.)
        if words && to <= from.byte || !words && at == end.bit {
            return;
        }
        let wide = if words { right - from.x } else { bit.wide.min(self.room.wide) };
        let high = match bit.thing {
            Thing::Picture(_, [w, h]) => h * wide / w.max(1.0),
            Thing::Icon(_) => bit.font.size,
            _ => 0.0,
        };
        let part = match self.laid.frags.last() {
            Some(last) if bit.link != 0 && self.laid.bits[last.bit as usize].link == bit.link => last.part + 1,
            _ => 0,
        };
        let frag = Frag { bit: at, words: [from.byte, if words { to } else { bit.words[1] }], first: from.stop, x: from.x - self.start.x, into: from.x - bit.x, wide, high, y: 0.0, part, level: 0 };
        self.laid.frags.push(frag);
    }
}

// ---- describing laid-out text ----

/// Laid-out text being described: its text, each link's text (by number, from 1) and its
/// destination and title (none for rich text); the selection range, and whether it shows as
/// the focused selection.
pub(crate) struct Shape<'a> {
    pub laid: &'a Laid,
    pub text: &'a str,
    pub links: &'a [[u32; 2]],
    pub titles: &'a [(String, String)],
    pub picked: [u32; 2],
    pub keyed: bool,
}

/// A link fragment the pointer can press, or (a link's first) the keyboard reach: its row and
/// fragment.
pub(crate) type Target = (u32, u32);

/// What the user did to some text's links this run: those hovered or focused (to underline),
/// and the one clicked, right-clicked and focused (each 0 for none).
#[derive(Default)]
pub(crate) struct Links {
    pub lit: Vec<u32>,
    pub pressed: u32,
    pub right_clicked: u32,
    pub focused: u32,
}

impl Links {
    /// Adds what was done to a box of link `link`.
    pub fn note(&mut self, link: u32, state: crate::ui::Response) {
        if state.hovered || state.focused {
            self.lit.push(link);
        }
        let [pressed, right_clicked, focused] = [state.clicked, state.right_clicked, state.focused].map(|done| if done { link } else { 0 });
        (self.pressed, self.right_clicked, self.focused) = (self.pressed.max(pressed), self.right_clicked.max(right_clicked), self.focused.max(focused));
    }
}

impl Ui {
    /// The user's interaction with each link of some text, via its pressable fragments
    /// (`targets`), each identified within the open box by link number and fragment index.
    pub(crate) fn link_states(&mut self, shape: &Shape, targets: &[Target]) -> Links {
        let mut links = Links::default();
        for (_, frag) in targets {
            let frag = shape.laid.frags[*frag as usize];
            let link = shape.laid.bits[frag.bit as usize].link;
            links.note(link, self.link_state(link, frag.part));
        }
        links
    }

    /// Boxes over the pressable link fragments `link_states` queried: each as tall as its row,
    /// placed from `corner` (the origin of row positions in the open box). Each link's first
    /// fragment is a Tab stop, told to screen readers as the link, by all its text.
    pub(crate) fn link_boxes(&mut self, shape: &Shape, targets: &[Target], corner: [f32; 2]) {
        for (row, frag) in targets {
            let (row, frag) = (shape.laid.rows[*row as usize], shape.laid.frags[*frag as usize]);
            let link = shape.laid.bits[frag.bit as usize].link;
            let place = crate::geom::rect(corner[0] + row.left + frag.x, corner[1] + row.top, frag.wide, row.high);
            let [from, to] = shape.links.get(link as usize - 1).copied().unwrap_or_default();
            self.titled_link_box(shape, link, frag.part, place, shape.text.get(from as usize..to as usize).unwrap_or(""));
        }
    }

    /// A link's box (as `link_box` makes it) with its title, if it has one: shown by it while
    /// hovered or focused, and told to screen readers as its description.
    pub(crate) fn titled_link_box(&mut self, shape: &Shape, link: u32, part: u32, place: crate::geom::Rect, name: &str) {
        let state = self.link_box(link, part, place, name);
        let Some((_, title)) = shape.titles.get(link as usize - 1).filter(|(_, title)| !title.is_empty()) else { return };
        if part == 0 {
            self.tell_description(title);
        }
        self.tooltip(&state, title);
    }

    /// The user's interaction with fragment `part` of link `link` (from 1) in the open box: the
    /// first fragment is pressed by the keyboard too; the pointer is a hand over any.
    pub(crate) fn link_state(&mut self, link: u32, part: u32) -> crate::ui::Response {
        let id = self.id(("link", link, part));
        let state = if part == 0 { self.press(id) } else { self.interact(id) };
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        state
    }

    /// The pressable box `link_state` asked about, at `place` in the open box: the link's first
    /// fragment a Tab stop, told to screen readers as the link, `name`.
    pub(crate) fn link_box(&mut self, link: u32, part: u32, place: crate::geom::Rect, name: &str) -> crate::ui::Response {
        let target = self.el().key(("link", link, part)).offset(place.x, place.y).size(place.w, place.h).radius(2.0).sense();
        if part > 0 {
            return target.show();
        }
        let state = target.focusable().show();
        self.tell(&state, Role::Link, None, name);
        state
    }

    /// Describes rows `rows` of some text into the open box, placed from `corner` (the origin
    /// of row positions in that box): on each row its selected part, piece backgrounds, and the
    /// pieces. Links in `lit` are underlined.
    pub(crate) fn write_rows(&mut self, shape: &Shape, rows: Range<usize>, corner: [f32; 2], lit: &[u32]) {
        for at in rows {
            let row = shape.laid.rows[at];
            let corner = [corner[0] + row.left, corner[1] + row.top];
            self.write_selection(shape, at, corner);
            for frag in &shape.laid.frags[row.frags[0] as usize..row.frags[1] as usize] {
                self.write_frag(shape, frag, corner, lit);
            }
        }
    }

    /// A row's selected part, as a highlight behind its text: extended past its end where the
    /// selection continues.
    fn write_selection(&mut self, shape: &Shape, at: usize, corner: [f32; 2]) {
        let ([from, to], row) = (shape.picked, shape.laid.rows[at]);
        if from >= to || to <= row.words[0] || from > row.words[1] {
            return;
        }
        let color = self.theme.accent.with_alpha(if shape.keyed { 0.35 } else { 0.2 });
        let laid = shape.laid;
        let frags = &laid.frags[row.frags[0] as usize..row.frags[1] as usize];
        if !row.turned(frags) {
            let left = laid.x_at(shape.text, at, from.max(row.words[0]));
            let right = if to > row.words[1] { laid.x_at(shape.text, at, row.words[1]) + 4.0 } else { laid.x_at(shape.text, at, to) };
            self.el().offset(corner[0] + left, corner[1]).size(right - left, row.high).fill(color).show();
            return;
        }
        // (In display order a selection may be in several places: its part of each fragment,
        // joined where they meet; and past the row's end, on its ending side.)
        let part = |frag: &Frag| [laid.frag_x(shape.text, frag, from.max(frag.words[0])), laid.frag_x(shape.text, frag, to.min(frag.words[1]))];
        let parts = frags.iter().filter(|frag| frag.words[0] < to && frag.words[1] > from).map(part).map(|[a, b]| [a.min(b), a.max(b)]);
        let end = laid.x_at(shape.text, at, row.words[1]);
        let past = (to > row.words[1]).then_some(if row.leftward { [end - 4.0, end] } else { [end, end + 4.0] });
        // (A row has few: each goes in where it belongs, left to right.)
        let mut spans: Vec<[f32; 2]> = Vec::new();
        for span in parts.chain(past) {
            spans.insert(spans.partition_point(|one| one[0] < span[0]), span);
        }
        let mut joined: Vec<[f32; 2]> = Vec::new();
        for span in spans {
            match joined.last_mut() {
                Some(last) if span[0] <= last[1] + 0.01 => last[1] = last[1].max(span[1]),
                _ => joined.push(span),
            }
        }
        for [left, right] in joined {
            self.el().offset(corner[0] + left, corner[1]).size(right - left, row.high).fill(color).show();
        }
    }

    /// One fragment of a row: its piece's background, if any, and its text, icon or picture.
    /// (Text is placed as a single line is: pieces of one font share a baseline.)
    fn write_frag(&mut self, shape: &Shape, frag: &Frag, [x, y]: [f32; 2], lit: &[u32]) {
        let bit = shape.laid.bits[frag.bit as usize];
        let (x, y) = (x + frag.x, y + frag.y);
        if bit.back != Color::NONE {
            let (before, after) = (if frag.into <= 0.0 { PAD } else { 0.0 }, if frag.words[1] >= bit.words[1] { PAD } else { 0.0 });
            self.el().offset(x - before, y).size(frag.wide + before + after, bit.above + bit.below).fill(bit.back).radius(3.0).show();
        }
        match bit.thing {
            #[cfg(feature = "ruby")]
            Thing::Ruby(_) => crate::rich_ruby::RUBY.get().into_iter().for_each(|ruby| (ruby.write)(self, shape, frag, [x, y], lit)),
            Thing::Icon(icon) => _ = self.el().icon(icon, frag.high, bit.color).offset(x, y).show(),
            Thing::Picture(picture, _) => _ = self.el().picture(picture).size(frag.wide, frag.high).offset(x, y).show(),
            _ => {
                let font = if bit.link != 0 && lit.contains(&bit.link) { bit.font.underlined() } else { bit.font };
                let words = &shape.text[frag.words[0] as usize..frag.words[1] as usize];
                let turned = turned_if_bare(frag, words);
                self.el().text(turned.as_deref().unwrap_or(words), font, bit.color).select(None, false).offset(x, y).show();
            }
        }
    }
}

// ---- what is kept from run to run ----

/// What a window's rich text and documents keep between runs (`Kept`): created when the first
/// is described, so a program with none has no state and no code for it. `settle` drops, after
/// a run, the ones not described in it.
#[derive(Default)]
pub(crate) struct Writings {
    pub kept: Option<Box<dyn std::any::Any>>,
    pub settle: Option<fn(&mut Ui)>,
}

/// What is kept, by each paragraph's and document's box id.
#[derive(Default)]
pub(crate) struct Kept {
    pub paragraphs: ById<Paragraph>,
    pub documents: ById<crate::markdown_view::Shown>,
    /// What vertical text keeps (see `vertical_paragraph`): none until it is described.
    pub vertical: crate::vertical_paragraph::Held,
}

/// A rich text paragraph as last laid out: a hash of its input, its pieces' text in order, each
/// link's text, its layout and the width it was laid out for, its resulting height and
/// single-line width; and whether it was described this run.
#[derive(Default)]
pub(crate) struct Paragraph {
    key: u64,
    text: String,
    links: Vec<[u32; 2]>,
    laid: Laid,
    width: f32,
    high: f32,
    long: f32,
    used: bool,
}

/// After a run: drops every paragraph and document not described in it.
fn forget_unused(ui: &mut Ui) {
    let Some(kept) = ui.writings.kept.as_mut().and_then(|kept| kept.downcast_mut::<Kept>()) else { return };
    kept.paragraphs.retain(|_, paragraph| std::mem::take(&mut paragraph.used));
    kept.documents.retain(|_, shown| std::mem::take(&mut shown.used));
}

/// Text in many styles, with links, icons and small pictures inline.
impl Ui {
    /// The rich text state, taken out of the window while a part is described (restored with
    /// `keep_writings`).
    pub(crate) fn take_writings(&mut self) -> Box<Kept> {
        self.writings.settle = Some(forget_unused);
        self.writings.kept.take().and_then(|kept| kept.downcast::<Kept>().ok()).unwrap_or_default()
    }
    /// Restores the rich text state.
    pub(crate) fn keep_writings(&mut self, kept: Box<Kept>) {
        self.writings.kept = Some(kept);
    }

    /// A rich text paragraph: pieces of text each with its own style (colour, size, face,
    /// weight, italic, monospace, underline or strikethrough, background), links among them,
    /// and inline icons and small pictures. It wraps to the given width as a wrapping label
    /// does, and a newline ends a row. Returns the index of the link piece pressed, if any.
    /// Screen readers hear all its text as one piece, and each link as a link.
    pub fn rich(&mut self, pieces: &[Span]) -> Option<usize> {
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, 0.0, 0.0]);
        let mut kept = self.take_writings();
        let paragraph = kept.paragraphs.entry(id).or_default();
        self.lay_paragraph(paragraph, pieces, read[0]);
        // (The box is as tall as its rows, so its layout is known ahead: only its width is read
        // back, and it is described again if that differs.)
        self.measures.push(Measure { at, within: at, id, read: [read[0], paragraph.high, 0.0] });
        self.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Px(paragraph.high)).open();
        // (In a shrink-to-fit box, it asks for its text on one line.)
        self.el().w(Length::Px(paragraph.long)).show();
        let rows = 0..paragraph.laid.rows.len();
        let shape = Shape { laid: &paragraph.laid, text: &paragraph.text, links: &paragraph.links, titles: &[], picked: [0; 2], keyed: false };
        let frags = shape.laid.frags.iter().enumerate();
        let targets: Vec<Target> = frags.filter(|(_, frag)| shape.laid.bits[frag.bit as usize].link != 0).map(|(at, _)| (row_of(shape.laid, at as u32), at as u32)).collect();
        let links = self.link_states(&shape, &targets);
        let words = self.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Px(paragraph.high)).open();
        self.tell(&words, Role::Text, None, &paragraph.text);
        self.write_rows(&shape, rows, [0.0; 2], &links.lit);
        self.close();
        self.link_boxes(&shape, &targets, [0.0; 2]);
        self.close();
        paragraph.used = true;
        self.keep_writings(kept);
        links.pressed.checked_sub(1).map(|link| link as usize)
    }

    /// Updates a paragraph for its pieces, theme and scale (re-measuring where any changed),
    /// and for its width (re-breaking rows where that changed).
    fn lay_paragraph(&mut self, paragraph: &mut Paragraph, pieces: &[Span], width: f32) {
        let (t, scale) = (self.theme, self.scale);
        let key = pieces.iter().fold(hash((t.font.size.to_bits(), t.font.face, t.text, t.accent, t.hover, scale.to_bits())), |key, piece| crate::id::fold(key, piece_key(piece)));
        if key != paragraph.key {
            (paragraph.key, paragraph.width) = (key, -1.0);
            paragraph.text.clear();
            paragraph.links.clear();
            let laid = &mut paragraph.laid;
            laid.bits.clear();
            laid.stops.clear();
            for piece in pieces {
                let start = paragraph.text.len() as u32;
                paragraph.text.push_str(piece.words);
                let words = [start, paragraph.text.len() as u32];
                #[cfg(feature = "ruby")]
                let thing = crate::rich_ruby::RUBY.get().filter(|_| !piece.ruby.is_empty()).map_or(piece.thing, |ruby| (ruby.told)(&mut paragraph.text, piece.ruby));
                #[cfg(not(feature = "ruby"))]
                let thing = piece.thing;
                let font = Font { face: face_of(piece.face.unwrap_or(t.font.face), piece.look), size: if piece.size > 0.0 { piece.size } else { t.font.size }, decoration: piece.look.rule };
                let link = if piece.look.link != 0 { paragraph.links.len() as u32 + 1 } else { 0 };
                let color = piece.color.unwrap_or(if link != 0 { t.accent } else { t.text });
                let back = piece.back.unwrap_or(if piece.look.mono { t.hover } else { Color::NONE });
                laid.bits.push(Bit::new(words, font, color, back, link, thing));
                paragraph.links.push(words);
            }
            paragraph.long = laid.measure(0..laid.bits.len(), &paragraph.text, self.text.as_mut(), scale, &mut Vec::new());
        }
        if width == paragraph.width {
            return;
        }
        let laid = &mut paragraph.laid;
        (laid.rows, laid.frags, paragraph.width) = (Vec::new(), Vec::new(), width);
        let [above, below] = heights(self.text.as_mut(), t.font, scale, &mut Vec::new());
        let room = Room { above, below, high: 0.0, left: 0.0, wide: width.max(1.0), side: Align::Start };
        paragraph.high = lay(laid, 0..laid.bits.len() as u32, &paragraph.text, 0, room, 0.0, scale);
    }
}

/// A right-to-left fragment's text as it shows, where it has no right-to-left letter (the text
/// system would set it left to right): turned round. None for any other.
fn turned_if_bare(frag: &Frag, words: &str) -> Option<String> {
    let strong = |letter: char| matches!(crate::bidi::kind(letter), crate::bidi::Kind::Right | crate::bidi::Kind::Arabic);
    (frag.leftward() && !words.chars().any(strong)).then(|| turned_round(words))
}

/// Text as it shows right to left: its grapheme clusters in reverse order, brackets mirrored.
fn turned_round(words: &str) -> String {
    let mut clusters = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let next = at + crate::text::cluster(&words[at..]);
        clusters.push(&words[at..next]);
        at = next;
    }
    clusters.iter().rev().flat_map(|cluster| cluster.chars()).map(|letter| crate::bidi::mirrored(letter).unwrap_or(letter)).collect()
}

/// A fragment's row.
pub(crate) fn row_of(laid: &Laid, frag: u32) -> u32 {
    laid.rows.partition_point(|row| row.frags[1] <= frag) as u32
}

/// A hash of everything a piece specifies.
pub(crate) fn piece_key(piece: &Span) -> u64 {
    let look = piece.look;
    let flags = look.bold as u64 | (look.italic as u64) << 1 | (look.mono as u64) << 2 | (look.rule as u64) << 3 | (look.link as u64) << 5;
    let thing = match piece.thing {
        Thing::Icon(icon) => hash((icon.path, icon.view.to_bits(), icon.stroke.to_bits())),
        Thing::Picture(picture, [wide, high]) => hash((picture.0, wide.to_bits(), high.to_bits())),
        _ => 0,
    };
    #[cfg(feature = "ruby")]
    let thing = thing ^ crate::rich_ruby::RUBY.get().map_or(0, |ruby| (ruby.key)(piece.ruby)) ^ piece.upright as u64;
    hash((piece.words, flags, piece.size.to_bits(), piece.face, piece.color, piece.back, thing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::BuiltInFont;

    /// The rows of some text pieces laid out in the built-in font (6 wide at size 8) in `wide`,
    /// each as its pieces' text.
    fn rows_of(pieces: &[(&str, Font)], wide: f32) -> Vec<Vec<String>> {
        let mut laid = Laid::default();
        let mut text = String::new();
        for (words, font) in pieces {
            let start = text.len() as u32;
            text.push_str(words);
            laid.bits.push(Bit::new([start, text.len() as u32], *font, Color::WHITE, Color::NONE, 0, Thing::Words));
        }
        laid.measure(0..laid.bits.len(), &text, &mut BuiltInFont, 1.0, &mut Vec::new());
        let room = Room { above: 0.0, below: 0.0, high: 0.0, left: 0.0, wide, side: Align::Start };
        let count = laid.bits.len() as u32;
        lay(&mut laid, 0..count, &text, 0, room, 0.0, 1.0);
        let frags = |row: &Row| laid.frags[row.frags[0] as usize..row.frags[1] as usize].iter().map(|frag| text[frag.words[0] as usize..frag.words[1] as usize].to_string()).collect();
        laid.rows.iter().map(frags).collect()
    }

    #[test]
    fn pieces_break_into_rows_together_at_spaces() {
        let (plain, bold) = (Font::new(8.0), Font::new(8.0).bold());
        // Plain letters are 6 wide and bold 7: in 60, "fix the jump" (76) does not fit and
        // breaks after "the"; "jump arc" (52) does. The space at a break is on neither row.
        assert_eq!(rows_of(&[("fix the ", plain), ("jump", bold), (" arc", plain)], 60.0), [vec!["fix the"], vec!["jump", " arc"]]);
        // A word longer than a row breaks where it must; a newline ends a row.
        assert_eq!(rows_of(&[("abcdefghij", plain)], 24.0), [vec!["abcd"], vec!["efgh"], vec!["ij"]]);
        assert_eq!(rows_of(&[("one\n", plain), ("two", bold)], 100.0), [vec!["one"], vec!["two"]]);
        // Pieces without a space between stay together.
        assert_eq!(rows_of(&[("aa ", plain), ("bb", bold), ("cc", plain)], 36.0), [vec!["aa"], vec!["bb", "cc"]]);
    }

    #[test]
    fn places_in_a_row_are_found_both_ways() {
        let mut laid = Laid::default();
        let text = "ab cd";
        laid.bits.push(Bit::new([0, 2], Font::new(8.0), Color::WHITE, Color::NONE, 0, Thing::Words));
        laid.bits.push(Bit::new([2, 5], Font::new(8.0).bold(), Color::WHITE, Color::NONE, 0, Thing::Words));
        laid.measure(0..2, text, &mut BuiltInFont, 1.0, &mut Vec::new());
        let room = Room { above: 0.0, below: 0.0, high: 0.0, left: 0.0, wide: 100.0, side: Align::Start };
        lay(&mut laid, 0..2, text, 0, room, 0.0, 1.0);
        // Plain letters are 6 wide, bold 7: "ab" ends at 12, " cd" at 12 + 21.
        assert_eq!([0, 1, 2, 3, 5].map(|at| laid.x_at(text, 0, at)), [0.0, 6.0, 12.0, 19.0, 33.0]);
        assert_eq!([-5.0, 2.0, 4.0, 13.0, 16.0, 40.0].map(|x| laid.at_x(text, 0, x)), [0, 0, 1, 2, 3, 5]);
    }

    #[test]
    fn a_face_takes_what_a_look_adds() {
        let bold = TextStyle { bold: true, ..TextStyle::default() };
        assert_eq!([face_of(Face::Regular, bold), face_of(Face::Bold, bold), face_of(Face::Regular, TextStyle { mono: true, ..bold })], [Face::Bold, Face::Bold, Face::Mono.weight(700)]);
        assert_eq!(face_of(Face::Regular, TextStyle { italic: true, ..TextStyle::default() }), Face::Italic);
        assert_eq!(face_of(Face::Bold, TextStyle { italic: true, ..TextStyle::default() }), Face::Bold.italic());
    }

    /// Pieces laid out as one paragraph `wide` units wide in the small Hebrew test font
    /// (`bidi::tests::hebrew`: alef 7 wide, gimel 10, space 4, Latin 12, at 16): its text and
    /// layout.
    #[cfg(feature = "right-to-left")]
    fn laid_both_ways(pieces: &[&str], wide: f32) -> (String, Laid) {
        let (mut laid, mut text, font) = (Laid::default(), String::new(), Font::new(16.0));
        for (n, words) in pieces.iter().enumerate() {
            let start = text.len() as u32;
            text.push_str(words);
            let font = if n % 2 == 1 { font.bold() } else { font };
            laid.bits.push(Bit::new([start, text.len() as u32], font, Color::WHITE, Color::NONE, 0, Thing::Words));
        }
        laid.measure(0..laid.bits.len(), &text, &mut crate::bidi::tests::hebrew(), 1.0, &mut Vec::new());
        let room = Room { above: 0.0, below: 0.0, high: 0.0, left: 0.0, wide, side: Align::Start };
        let count = laid.bits.len() as u32;
        lay(&mut laid, 0..count, &text, 0, room, 0.0, 1.0);
        (text, laid)
    }

    /// Each fragment of a row as its text and place, left to right.
    #[cfg(feature = "right-to-left")]
    fn shown_left_to_right(text: &str, laid: &Laid, row: usize) -> Vec<(String, f32)> {
        let row = laid.rows[row];
        let mut frags: Vec<(String, f32)> =
            laid.frags[row.frags[0] as usize..row.frags[1] as usize].iter().map(|frag| (text[frag.words[0] as usize..frag.words[1] as usize].to_string(), frag.x)).collect();
        frags.sort_by(|a, b| a.1.total_cmp(&b.1));
        frags
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn hebrew_in_a_left_to_right_paragraph_reads_right_to_left_across_its_pieces() {
        // Two Hebrew words in two pieces, between Latin: the space between the words goes with
        // them, the spaces at their edges with the Latin; the second word shows first.
        let (text, laid) = laid_both_ways(&["ab ", "\u{5d0}\u{5d2}", " \u{5d2}\u{5d0}", " cd"], 400.0);
        let shown = shown_left_to_right(&text, &laid, 0);
        let expected = [("ab ", 0.0), (" \u{5d2}\u{5d0}", 28.0), ("\u{5d0}\u{5d2}", 49.0), (" cd", 66.0)];
        assert_eq!(shown, expected.map(|(words, x)| (words.to_string(), x)));
        // The cursor before alef stands at the right of its word; a point just right of that
        // word's left edge is after its last letter, gimel; and after gimel is its left edge.
        let alef = 3;
        assert_eq!(laid.x_at(&text, 0, alef), 66.0);
        assert_eq!(laid.at_x(&text, 0, 50.0), alef + 4);
        assert_eq!(laid.at_x(&text, 0, 64.0), alef);
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_right_to_left_paragraph_starts_at_the_right_and_runs_leftward() {
        // Hebrew first: the paragraph runs right to left, so its rows end at the right; the
        // Latin word in it reads left to right, at the left of the Hebrew before it.
        let (text, laid) = laid_both_ways(&["\u{5d0}\u{5d2} ", "cd", " \u{5d2}"], 100.0);
        let shown = shown_left_to_right(&text, &laid, 0);
        // Alef-gimel and its space 21, "cd" in bold 28, space and gimel 14: 63 in all, from 37.
        let expected = [(" \u{5d2}", 37.0), ("cd", 51.0), ("\u{5d0}\u{5d2} ", 79.0)];
        assert_eq!(shown, expected.map(|(words, x)| (words.to_string(), x)));
        assert!(laid.rows[0].leftward);
        // Past the row's end is its left end.
        assert_eq!(laid.x_at(&text, 0, text.len() as u32), 37.0);
    }
}

/// Hebrew and Arabic across pieces of rich text, held against Chrome's drawing of the same
/// paragraphs (`dir="auto"`, each piece a span in its own colour), in Arial: each piece's ink
/// lies where Chrome puts it.
#[cfg(all(test, windows, feature = "right-to-left"))]
mod against_edge {
    use super::*;
    use crate::browser_check::{browser, ink_middle, lasting_differences};
    use crate::font_file::{AppFonts, FontFile};
    use crate::geom::rect;

    /// The colours pieces are written in, in turn.
    const INKS: [u32; 5] = [0x000000, 0xd00000, 0x008000, 0x0000ff, 0xb000b0];

    /// Paragraphs, a line each: Hebrew in a left-to-right one, split across pieces; Hebrew
    /// first, with Latin in it; Arabic first, with digits; Arabic-Indic digits and Arabic in a
    /// left-to-right one; and punctuation alone between Hebrew words.
    const CASES: [&[&str]; 5] = [
        &["Read ", "\u{5e9}\u{5dc}\u{5d5}\u{5dd}", " \u{5e2}\u{5d5}\u{5dc}\u{5dd}", " now."],
        &["\u{5e9}\u{5dc}\u{5d5}\u{5dd} ", "world", " \u{5e2}\u{5d5}\u{5dc}\u{5dd}", "!"],
        &["\u{645}\u{631}\u{62d}\u{628}\u{627} ", "\u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}", " 123", " test"],
        &["Price: ", "\u{663}\u{664}\u{665}", " \u{631}\u{64a}\u{627}\u{644}", " each"],
        &["\u{5e9}\u{5dc}\u{5d5}\u{5dd} ", "?!", " \u{5e2}\u{5d5}\u{5dc}\u{5dd}"],
    ];

    /// Where case `n` is drawn: its left and top.
    fn place(n: usize) -> [f32; 2] {
        [10.0, 10.0 + 40.0 * n as f32]
    }

    /// The cases as a page, in the font at `path`.
    fn page(path: &str) -> String {
        let mut html = format!(
            "<!doctype html><meta charset=utf-8><style>@font-face {{ font-family: Probe; src: url('file:///{path}'); }} body {{ margin: 0; background: #fff; }} div {{ position: absolute; width: 500px; font: 20px Probe; line-height: 30px; white-space: nowrap; font-kerning: none; }}</style>"
        );
        for (n, pieces) in CASES.iter().enumerate() {
            let [x, y] = place(n);
            html += &format!("<div dir=auto style='left: {x}px; top: {y}px'>");
            for (at, words) in pieces.iter().enumerate() {
                html += &format!("<span style='color: #{:06x}'>{words}</span>", INKS[at]);
            }
            html += "</div>";
        }
        html
    }

    /// The same, written by the library as rich text.
    fn written(ui: &mut Ui) {
        ui.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Grow(1.0)).open();
        for (n, pieces) in CASES.iter().enumerate() {
            let [x, y] = place(n);
            ui.el().offset(x, y).w(Length::Px(500.0)).open();
            let spans: Vec<Span> = pieces.iter().enumerate().map(|(at, words)| Span::text(words).size(20.0).color(Color::rgb(INKS[at]))).collect();
            ui.rich(&spans);
            ui.close();
        }
        ui.close();
    }

    #[test]
    fn pieces_of_hebrew_and_arabic_lie_where_chrome_sets_them() {
        let Some(browser) = browser() else { return eprintln!("no Edge or Chrome here: not compared") };
        let path = std::path::Path::new(&std::env::var_os("WINDIR").unwrap_or_default()).join(r"Fonts\arial.ttf");
        let Some(font) = FontFile::open(&path) else { return eprintln!("no Arial here: not compared") };
        let size = [540, 220];
        let mut ui = Ui::new(Box::new(AppFonts::new(font)), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: size[0] as f32, h: size[1] as f32 }, 1.0);
        ui.theme.bg = Color::WHITE;
        ui.run(&mut written);
        let ours = crate::surface::Surface::draw(&mut ui, 64).pixels;
        let differ = |theirs: &[u32]| {
            let mut wrong = Vec::new();
            for (n, pieces) in CASES.iter().enumerate() {
                let [x, y] = place(n);
                let band = rect(x - 5.0, y - 5.0, 510.0, 40.0);
                for (at, words) in pieces.iter().enumerate() {
                    let middle = |pixels: &[u32]| ink_middle(pixels, size[0], band, INKS[at], &INKS).map(|[x, _]| x);
                    let (mine, edge) = (middle(&ours), middle(theirs));
                    // (Within 3: Latin and digits advance by whole pixels here, by fractions in
                    // Chrome, so a line drifts by a pixel or two. A piece out of order is a word
                    // away.)
                    if !mine.zip(edge).is_some_and(|(mine, edge)| (mine - edge).abs() <= 3.0) {
                        wrong.push((format!("{words:?}"), format!("ours at {mine:?}, Edge's at {edge:?}")));
                    }
                }
            }
            wrong
        };
        let wrong = lasting_differences(&browser, &page(&path.to_string_lossy().replace('\\', "/")), size, "bidi", differ);
        assert!(wrong.is_empty(), "{} pieces lie elsewhere than in Chrome", wrong.len());
    }
}
