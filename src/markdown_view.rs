//! A Markdown document drawn ([`Ui::markdown`]): parsed text (`rich_text::Writing`) laid out as
//! a page, selectable and copyable by pointer, keys and its context menu, and told to screen
//! readers as a document.
//!
//! It is laid out as a page, not box by box, and only as far as it is seen. Blocks are placed
//! top down and their text wrapped into rows (`rich_text.rs`), each text run measured as its
//! block is first laid out: down to a screen past what shows. Below that the height is
//! estimated from the text's length, and laid out for real as it scrolls into view; a new width
//! lays it out again the same way. Only blocks in view are described, found by halving each
//! container's children, each a box inside its parent block's box: a document of any length
//! costs a screenful each run. Boxes nest as blocks do, so screen readers get each block as what
//! it is, and a table as rows and cells under its headings. Links in view get boxes after the
//! blocks, with the link before and after them, so Tab visits every link in order and scrolls
//! to each.
//!
//! Nesting has no limit here: containers are walked with stacks of their own, not by calling
//! back in, and indents stop where they would leave the text too narrow.
use crate::accessibility::Role;
use crate::code_editor::Syntax;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::hash;
use crate::layout::{Align, Flow, Length};
use crate::markdown::Markdown;
use crate::rich_text::{Bit, Kind, Laid, Links, Room, Shape, THING, Target, TextStyle, Thing, Writing, face_of, heights, lay, row_of};
use crate::slots::Slots;
use crate::text::{Decoration, Face, Font, TextSystem};
use crate::text_edit::{step, word, word_edge};
use crate::theme::Theme;
use crate::ui::{Measure, Response, Stored, Stroke, Ui};
use std::time::Duration;

/// How often the view scrolls while a selection is dragged past its edge.
const TICK: Duration = Duration::from_millis(16);
/// How near the view's edge a selection drag starts scrolling it, and how fast it then goes
/// (units a second, per unit nearer or past).
const EDGE: f32 = 24.0;
const SPEED: f32 = 20.0;

/// A document as last laid out, and the user's selection: kept by its box's id.
#[derive(Default)]
pub(crate) struct Shown {
    /// What it was measured for: the parse, theme and scale; and the layout width.
    serial: u64,
    theme: Option<Theme>,
    scale: f32,
    width: f32,
    laid: Laid,
    /// Per block: its text pieces, place, rows and children.
    areas: Vec<Area>,
    /// Every container's children, by block, as `Area::kids` ranges.
    kids: Vec<u32>,
    /// Each link's text, by number from 1; and every laid out piece of a link, by row and
    /// fragment.
    links: Vec<[u32; 2]>,
    targets: Vec<Target>,
    /// The blocks measured so far, and the next block to lay out; the containers open there,
    /// the page first.
    measured: usize,
    next: usize,
    open: Vec<Open>,
    fonts: Vec<(Font, [f32; 2])>,
    /// The bottom of what is laid out, and the end of its text.
    bottom: f32,
    laid_bytes: u32,
    /// The measured text's width and its bytes: what the rest's height is estimated by.
    sample: [f32; 2],
    /// Its first heading's text: its name for screen readers.
    title: [u32; 2],
    /// Its height (the part not laid out yet estimated), and its widest block's unwrapped
    /// width so far.
    high: f32,
    long: f32,
    /// The selection: anchor and focus (text bytes); the click count that started it, and the
    /// unit the first click took (a point, a word, a block).
    picked: [u32; 2],
    clicks: u8,
    grabbed: [u32; 2],
    /// The link with keyboard focus, and the one its context menu is for (0 for none).
    focused: u32,
    menu_link: u32,
    /// Each code block described last run, and how far it was scrolled sideways.
    code_scrolls: Vec<(u32, f32)>,
    /// When a selection drag last scrolled the view (ms, never 0).
    dragged_at: u16,
    /// Described this run.
    pub used: bool,
}

/// A block's layout: its pieces, its place from the document's top left, and its rows; its
/// children (a range of `Shown::kids`), and whether it is in a quote. For an item: its first
/// row's baseline offset (where its marker goes), and its index and count in its list. For a
/// list: its marker column's width, and its nesting depth. For a table row: its index among the
/// body rows (the header row is none of them), and their count.
#[derive(Clone, Copy, Default)]
struct Area {
    bits: [u32; 2],
    rect: Rect,
    rows: [u32; 2],
    kids: [u32; 2],
    quoted: bool,
    mark: f32,
    nth: [u32; 2],
}

/// A theme's document spacing: between blocks, and between tight list items; a quote's text
/// from its bar; padding around code and around cell text (x, y); and the list marker column.
#[derive(Clone, Copy)]
pub(crate) struct Sizes {
    pub gap: f32,
    pub close: f32,
    pub inset: f32,
    pub pad: f32,
    pub cell: [f32; 2],
    pub marks: f32,
}

pub(crate) fn sizes(t: &Theme) -> Sizes {
    let s = t.font.size;
    Sizes { gap: (s * 0.85).round(), close: (s * 0.3).round(), inset: (s * 1.2).round(), pad: (s * 0.75).round(), cell: [(s * 0.6).round(), (s * 0.35).round()], marks: (s * 1.7).round() }
}

/// The narrowest a nested block's content gets by indenting: deeper lists and quotes indent no
/// further.
fn narrowest(t: &Theme) -> f32 {
    (t.font.size * 12.0).round()
}

/// A block's base font, before inline styles: headings larger and bold, code monospace, header
/// cells bold.
pub(crate) fn font_of(kind: Kind, head: bool, t: &Theme) -> Font {
    match kind {
        Kind::Heading(level) => Font { face: Face::Bold, size: (t.font.size * [2.0, 1.6, 1.3, 1.1, 1.0, 0.9][level.clamp(1, 6) as usize - 1]).round(), decoration: Decoration::None },
        Kind::Code(_) => t.font.mono(),
        _ if head => t.font.bold(),
        _ => t.font,
    }
}

/// A block's minimum row height: body text gets line spacing; headings are as high as their
/// letters.
pub(crate) fn least(kind: Kind, font: Font) -> f32 {
    match kind {
        Kind::Heading(_) => 0.0,
        Kind::Code(_) => (font.size * 1.45).round(),
        _ => (font.size * 1.5).round(),
    }
}

/// Whether a block is in a quote.
pub(crate) fn quoted(writing: &Writing, at: usize) -> bool {
    let mut up = writing.blocks[at].parent;
    while let Some(block) = writing.blocks.get(up as usize) {
        if block.kind == Kind::Quote {
            return true;
        }
        up = block.parent;
    }
    false
}

/// Whether a block has its own text (and so rows).
pub(crate) fn worded(kind: Kind) -> bool {
    matches!(kind, Kind::Paragraph | Kind::Heading(_) | Kind::Code(_) | Kind::Cell(_))
}

/// Whether a block is a cell of a table's header row.
fn heads(writing: &Writing, at: usize) -> bool {
    matches!(writing.blocks.get(writing.blocks[at].parent as usize).map(|row| row.kind), Some(Kind::Row { head: true }))
}

/// The highlighting rules for a code language: the app's for that name, else the library's for
/// a common name it knows.
pub(crate) fn rules_for(doc: &Markdown, name: &str) -> Option<&'static Syntax> {
    let given = doc.rules.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, rules)| *rules);
    given.or(match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(&Syntax::RUST),
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" => Some(&Syntax::C),
        "python" | "py" => Some(&Syntax::PYTHON),
        "javascript" | "js" | "jsx" | "mjs" | "typescript" | "ts" | "tsx" | "json" => Some(&Syntax::JAVASCRIPT),
        _ => None,
    })
}

/// A heading's anchor (`#what-it-says`), as GitHub makes it: lower case, spaces as hyphens, and
/// of the rest only letters, digits, hyphens and underscores.
pub(crate) fn slug(words: &str) -> String {
    let kept = words.trim().chars().filter(|letter| letter.is_alphanumeric() || matches!(letter, ' ' | '-' | '_'));
    kept.flat_map(char::to_lowercase).map(|letter| if letter == ' ' { '-' } else { letter }).collect()
}

/// A selection as start and end, whichever way it was made.
fn in_order([a, b]: [u32; 2]) -> [u32; 2] {
    [a.min(b), a.max(b)]
}

/// The block a text byte is in: the last to start at or before it.
fn block_of(writing: &Writing, at: u32) -> usize {
    writing.blocks.partition_point(|block| block.words[0] <= at).saturating_sub(1)
}

/// Each link's text, by number from 1: from its first piece's start to its last's end.
fn link_words(writing: &Writing, count: usize) -> Vec<[u32; 2]> {
    let mut links = vec![[0; 2]; count];
    for stretch in &writing.pieces {
        if let Some(words) = links.get_mut(stretch.look.link.wrapping_sub(1) as usize) {
            *words = if words[1] == 0 { stretch.words } else { [words[0], stretch.words[1]] };
        }
    }
    links
}

/// Every container's children, each after the one before ends: into `kids`, and each block's
/// range of them into its area.
fn children(writing: &Writing, areas: &mut [Area]) -> Vec<u32> {
    let mut kids = Vec::with_capacity(writing.blocks.len());
    for (at, block) in writing.blocks.iter().enumerate() {
        let first = kids.len() as u32;
        let mut child = at + 1;
        while child < block.end as usize {
            kids.push(child as u32);
            child = writing.blocks[child].end as usize;
        }
        areas[at].kids = [first, kids.len() as u32];
    }
    kids
}

impl Shown {
    /// Starts again for another parse, theme or scale: nothing measured or laid out.
    fn start(&mut self, doc: &Markdown, t: &Theme, scale: f32) {
        let writing = &doc.writing;
        (self.theme, self.scale, self.width) = (Some(*t), scale, -1.0);
        if self.serial != doc.serial {
            (self.serial, self.picked, self.clicks) = (doc.serial, [0; 2], 0);
        }
        (self.laid, self.areas, self.links) = (Laid::default(), vec![Area::default(); writing.blocks.len()], link_words(writing, doc.links.len()));
        self.kids = children(writing, &mut self.areas);
        (self.measured, self.sample) = (0, [0.0; 2]);
        self.fonts.clear();
        self.title = writing.blocks.iter().find(|block| matches!(block.kind, Kind::Heading(_))).map_or([0; 2], |block| block.words);
    }

    /// Starts laying out again, in a document `width` wide.
    fn start_layout(&mut self, width: f32, t: &Theme) {
        (self.width, self.long, self.bottom, self.laid_bytes, self.next) = (width, 0.0, 0.0, 0, 1);
        (self.laid.rows, self.laid.frags, self.targets) = (Vec::new(), Vec::new(), Vec::new());
        self.open = vec![Open { block: 0, x: 0.0, w: width, top: 0.0, pen: 0.0, gap: sizes(t).gap, count: 0, lists: 0 }];
        if let Some(page) = self.areas.first_mut() {
            page.rect = rect(0.0, 0.0, width, 0.0);
        }
    }

    /// Measures the text of the blocks up to `last`, in order: each block's pieces, and where
    /// each character ends.
    fn measure_through(&mut self, doc: &Markdown, last: usize, given: &mut Given) {
        let writing = &doc.writing;
        while self.measured <= last.min(writing.blocks.len().saturating_sub(1)) {
            let (at, block) = (self.measured, writing.blocks[self.measured]);
            if let Some(up) = writing.blocks.get(block.parent as usize) {
                self.areas[at].quoted = up.kind == Kind::Quote || self.areas[block.parent as usize].quoted;
            }
            self.measured += 1;
            let first = self.laid.bits.len();
            match block.kind {
                Kind::Code(language) => self.code_bits(doc, at, language, given.t),
                kind if worded(kind) => self.word_bits(doc, at, given),
                _ => continue,
            }
            let wide = self.laid.measure(first..self.laid.bits.len(), &writing.words, given.system, given.scale, &mut self.fonts);
            self.areas[at].bits = [first as u32, self.laid.bits.len() as u32];
            self.sample = [self.sample[0] + wide, self.sample[1] + (block.words[1] - block.words[0]) as f32];
        }
    }

    /// A text block's pieces: each run in the block's font plus its inline style, in its colour
    /// (links stand out; quotes are muted), inline code on its own background, and an image
    /// where the app gave one for its name (else its alt text, in italics).
    fn word_bits(&mut self, doc: &Markdown, at: usize, given: &Given) {
        let (writing, block, t) = (&doc.writing, doc.writing.blocks[at], given.t);
        let (base, words) = (font_of(block.kind, heads(writing, at), t), if self.areas[at].quoted { t.text_dim } else { t.text });
        for stretch in &writing.pieces[block.pieces[0] as usize..block.pieces[1] as usize] {
            let (look, link) = (stretch.look, stretch.look.link);
            let mut font = Font { face: face_of(base.face, look), decoration: look.rule, ..base };
            let (mut color, back) = (if link != 0 { t.accent } else { words }, if look.mono { t.hover } else { Color::NONE });
            let picture = match stretch.thing {
                Thing::Named(name) => doc.given.get(name as usize).copied().flatten().and_then(|picture| given.pictures.get(picture.0).map(|stored| (picture, stored))),
                _ => None,
            };
            // An image with no alt text, and none given by the app, takes no room.
            let missing = picture.is_none() && matches!(stretch.thing, Thing::Named(_));
            if missing && writing.words[stretch.words[0] as usize..stretch.words[1] as usize].starts_with(THING) {
                continue;
            }
            if missing {
                (font.face, color) = (face_of(font.face, TextStyle { italic: true, ..TextStyle::default() }), t.text_dim);
            }
            let thing = match picture {
                Some((picture, stored)) => Thing::Picture(picture, [stored.width as f32, (stored.pixels.len() / stored.width.max(1)) as f32]),
                None => Thing::Words,
            };
            self.laid.bits.push(Bit::new(stretch.words, font, color, back, link, thing));
        }
    }

    /// A code block's pieces: in a language with rules, a piece per token run on each line (and
    /// one per line end) in its token colour; else all in the text colour.
    fn code_bits(&mut self, doc: &Markdown, at: usize, language: u32, t: &Theme) {
        let (font, [start, end]) = (t.font.mono(), doc.writing.blocks[at].words);
        let rules = doc.writing.languages.get(language.wrapping_sub(1) as usize).and_then(|name| rules_for(doc, name));
        let Some(rules) = rules else { return self.laid.bits.push(Bit::new([start, end], font, t.text, Color::NONE, 0, Thing::Words)) };
        let (code, mut open, mut runs, mut from) = (&doc.writing.words, 0, Vec::new(), start as usize);
        while from <= end as usize {
            let line = code[from..end as usize].find('\n').map_or(end as usize, |at| from + at);
            runs.clear();
            open = rules.line(&code[from..line], open, &mut runs);
            let mut run = from;
            for (to, sort) in &runs {
                self.laid.bits.push(Bit::new([run as u32, (from + to) as u32], font, sort.color(t), Color::NONE, 0, Thing::Words));
                run = from + to;
            }
            if line < end as usize {
                self.laid.bits.push(Bit::new([line as u32, line as u32 + 1], font, t.text, Color::NONE, 0, Thing::Words));
            }
            from = line + 1;
        }
    }

    /// Lays out more of the document: on from where it stopped, until what is laid out reaches
    /// `bottom` and block `block`, or the end. Below that, its height is an estimate.
    fn lay_more(&mut self, doc: &Markdown, given: &mut Given, bottom: f32, block: usize) {
        let count = doc.writing.blocks.len();
        if self.next >= count {
            return;
        }
        let (frags, t) = (self.laid.frags.len(), *given.t);
        let mut laying = Laying { shown: self, doc, writing: &doc.writing, size: sizes(&t), given };
        while laying.shown.next < count && (laying.shown.bottom <= bottom || laying.shown.next <= block) {
            let at = laying.shown.next;
            laying.close_ended(at);
            laying.shown.next = laying.block(at);
        }
        if laying.shown.next >= count {
            laying.close_ended(usize::MAX);
        }
        // Containers still open are as high as what they hold so far.
        for open in &self.open {
            self.areas[open.block].rect.h = (self.bottom - open.top).max(0.0);
        }
        let laid = &self.laid;
        let links = laid.frags.iter().enumerate().skip(frags).filter(|(_, frag)| laid.bits[frag.bit as usize].link != 0);
        self.targets.extend(links.map(|(at, _)| (row_of(laid, at as u32), at as u32)));
        self.high = self.areas[0].rect.h.max(self.bottom) + self.estimate(doc, &t);
    }

    /// The height of what is not laid out yet: its text at the measured text's width a byte, in
    /// rows of body text, and a gap between every two of its blocks.
    fn estimate(&self, doc: &Markdown, t: &Theme) -> f32 {
        let blocks = doc.writing.blocks.len().saturating_sub(self.next) as f32;
        if blocks == 0.0 {
            return 0.0;
        }
        let bytes = doc.writing.words.len().saturating_sub(self.laid_bytes as usize) as f32;
        let per_byte = if self.sample[1] > 0.0 { self.sample[0] / self.sample[1] } else { t.font.size * 0.5 };
        let rows = bytes * per_byte / self.width.max(1.0) + blocks * 0.5;
        (rows * least(Kind::Paragraph, t.font) + blocks * sizes(t).gap / 2.0).round()
    }

    /// Block `at`'s unwrapped text width.
    fn long(&self, at: usize) -> f32 {
        let [from, to] = self.areas[at].bits;
        let last = (to > from).then(|| self.laid.bits[to as usize - 1]);
        last.map_or(0.0, |bit| bit.x + bit.wide)
    }

    /// The blocks in view (`view`, from the top), each container before its content, in
    /// order: in each container from the first child reaching into view, to the last starting
    /// in it.
    fn in_view(&self, view: [f32; 2]) -> Vec<u32> {
        let (mut order, mut stack) = (Vec::new(), vec![(0, self.first_in_view(0, view))]);
        while let Some((up, next)) = stack.last_mut() {
            let at = self.kids[*next..self.areas[*up].kids[1] as usize].first().map_or(usize::MAX, |at| *at as usize);
            if at >= self.next || self.areas[at].rect.y > view[1] {
                stack.pop();
                continue;
            }
            *next += 1;
            order.push(at as u32);
            if self.areas[at].kids[0] < self.areas[at].kids[1] {
                stack.push((at, self.first_in_view(at, view)));
            }
        }
        order
    }

    /// Container `up`'s first laid out child that reaches into view, as an index of `kids`.
    fn first_in_view(&self, up: usize, view: [f32; 2]) -> usize {
        let [from, to] = self.areas[up].kids.map(|at| at as usize);
        let above = |at: &u32| (*at as usize) < self.next && self.areas[*at as usize].rect.bottom() < view[0];
        from + self.kids[from..to].partition_point(above)
    }

    /// Text block `at`'s rows in view, as indices of all rows.
    fn rows_in_view(&self, at: usize, view: [f32; 2]) -> [usize; 2] {
        let [first, end] = self.areas[at].rows.map(|row| row as usize);
        let rows = &self.laid.rows[first..end];
        let from = rows.partition_point(|row| row.top + row.high < view[0]);
        [first + from, first + rows.partition_point(|row| row.top <= view[1]).max(from)]
    }

    /// The link pieces given boxes, in order: those in view (in the blocks `order`), the link
    /// before them and the one after (so Tab reaches them), and the focused link wherever it
    /// is. A link with focus or next after the last laid out one, but not laid out itself, is
    /// returned apart: it gets a box at the end of what is laid out.
    fn reachable(&self, writing: &Writing, order: &[u32], view: [f32; 2]) -> (Vec<Target>, u32) {
        let worded = order.iter().map(|at| *at as usize).filter(|at| worded(writing.blocks[*at].kind));
        let [low, high] = worded.map(|at| self.rows_in_view(at, view)).fold([usize::MAX, 0], |[low, high], [from, to]| [low.min(from), high.max(to)]);
        let [from, to] = [low, high].map(|row| self.targets.partition_point(|(at, _)| (*at as usize) < row));
        let tab_stop = |at: &usize| self.laid.frags[self.targets[*at].1 as usize].part == 0;
        let (before, after) = ((0..from).rev().find(tab_stop), (to..self.targets.len()).find(tab_stop));
        let focused = self.targets.partition_point(|target| link_of(self, target) < self.focused);
        let focused = self.targets.get(focused).filter(|target| link_of(self, target) == self.focused).map(|_| focused);
        // (In text order: the focused one goes in where it belongs, if not there already.)
        let mut picked: Vec<usize> = before.into_iter().chain(from..to.max(from)).chain(after).collect();
        if let Some(focused) = focused
            && let Err(at) = picked.binary_search(&focused)
        {
            picked.insert(at, focused);
        }
        // (Not laid out yet: the focused link, else the next one where none laid out follows.)
        let laid = self.targets.last().map_or(0, |target| link_of(self, target));
        let apart = match after {
            _ if self.focused > laid => self.focused,
            None => laid + 1,
            Some(_) => 0,
        };
        (picked.into_iter().map(|at| self.targets[at]).collect(), if (apart as usize) <= self.links.len() { apart } else { 0 })
    }

    /// The text byte nearest a point (from the document's top left): in the nearest text block
    /// laid out (vertically first, then horizontally), on the row level with it; in a code
    /// block, as scrolled sideways.
    fn hit(&self, text: &str, x: f32, y: f32) -> u32 {
        let mut nearest: Option<([f32; 2], usize)> = None;
        for (at, area) in self.areas[..self.next.min(self.areas.len())].iter().enumerate().filter(|(_, area)| area.rows[0] < area.rows[1]) {
            let r = area.rect;
            let away = [(r.y - y).max(y - r.bottom()).max(0.0), (r.x - x).max(x - r.right()).max(0.0)];
            if nearest.is_none_or(|(least, _)| away < least) {
                nearest = Some((away, at));
            }
        }
        let Some((_, at)) = nearest else { return 0 };
        let across = self.code_scrolls.iter().find(|(code, _)| *code as usize == at).map_or(0.0, |(_, across)| *across);
        let [first, end] = self.areas[at].rows;
        let rows = &self.laid.rows[first as usize..end as usize];
        let row = first as usize + rows.partition_point(|row| row.top + row.high <= y).min(rows.len() - 1);
        self.laid.at_x(text, row, x + across - self.laid.rows[row].left)
    }

    /// The row text byte `at` is on.
    fn row_at(&self, at: u32) -> usize {
        self.laid.rows.partition_point(|row| row.words[0] <= at).saturating_sub(1)
    }

    /// A pointer press at text byte `at`: the first of `clicks` presses, with Shift or not; or,
    /// for zero clicks, a drag with the button held, which extends the selection by the first
    /// press's unit (a point, a word, a block).
    fn point(&mut self, writing: &Writing, at: u32, clicks: u8, shift: bool) {
        match (clicks, shift) {
            (0, _) => {}
            (1, true) => (self.clicks, self.grabbed) = (1, [self.picked[0]; 2]),
            (clicks, _) => {
                self.clicks = clicks;
                (self.grabbed, self.picked) = (self.unit(writing, at), self.unit(writing, at));
                return;
            }
        }
        let unit = self.unit(writing, at);
        self.picked = if at < self.grabbed[0] { [self.grabbed[1], unit[0]] } else { [self.grabbed[0], unit[1]] };
    }

    /// What a press at byte `at` selects, by click count: the point, the word there, or its
    /// whole block.
    fn unit(&self, writing: &Writing, at: u32) -> [u32; 2] {
        let text = &writing.words;
        let edge = |end: bool| word_edge(text, at as usize, end).min(text.len()) as u32;
        let block = writing.blocks[block_of(writing, at)];
        match self.clicks {
            2 => [edge(false), edge(true)],
            3 if worded(block.kind) => block.words,
            _ => [at; 2],
        }
    }
}

/// A target's link number.
fn link_of(shown: &Shown, (_, frag): &Target) -> u32 {
    shown.laid.bits[shown.laid.frags[*frag as usize].bit as usize].link
}

/// What measuring and laying out text takes from the window: its theme, text system and scale,
/// and the pictures the app added.
struct Given<'a> {
    t: &'a Theme,
    system: &'a mut dyn TextSystem,
    scale: f32,
    pictures: &'a Slots<Stored>,
}

/// A container block while its content is laid out: its content's x and width, its start y and
/// how far down it has got, the gap between children, its child count so far, and how many
/// lists it is in (itself included).
#[derive(Clone, Copy)]
struct Open {
    block: usize,
    x: f32,
    w: f32,
    top: f32,
    pen: f32,
    gap: f32,
    count: u32,
    lists: u32,
}

/// What laying out a document passes around.
struct Laying<'a, 'b> {
    shown: &'a mut Shown,
    doc: &'a Markdown,
    writing: &'a Writing,
    size: Sizes,
    given: &'a mut Given<'b>,
}

impl Laying<'_, '_> {
    /// Closes every container that ends before block `at`: it is as high as its content, and
    /// its parent continues below it.
    fn close_ended(&mut self, at: usize) {
        while let Some(done) = self.shown.open.pop_if(|open| self.writing.blocks[open.block].end as usize <= at) {
            // An empty item is still as high as its marker's row.
            let row = if matches!(self.writing.blocks[done.block].kind, Kind::Item { .. }) { least(Kind::Paragraph, self.given.t.font) } else { 0.0 };
            self.shown.areas[done.block].rect.h = (done.pen - done.top).max(row);
            if self.shown.open.is_empty() {
                return;
            }
            self.ends(done.block);
        }
    }

    /// Block `at` has its height: its parent continues below it. (The first rows of an item's
    /// first block place the item's marker.)
    fn ends(&mut self, at: usize) {
        let r = self.shown.areas[at].rect;
        self.shown.bottom = self.shown.bottom.max(r.bottom());
        let Some(up) = self.shown.open.last_mut() else { return };
        up.pen = r.y + r.h;
        let first = self.shown.areas[at].rows[0] as usize;
        if up.count == 1 && matches!(self.writing.blocks[up.block].kind, Kind::Item { .. }) && first < self.shown.laid.rows.len() && self.shown.areas[at].rows[1] > 0 {
            let row = self.shown.laid.rows[first];
            self.shown.areas[up.block].mark = row.top + row.base - up.top;
        }
    }

    /// Lays out block `at`, inside the last opened container: its place and content. Returns
    /// the next block.
    fn block(&mut self, at: usize) -> usize {
        let block = self.writing.blocks[at];
        let Some(&up) = self.shown.open.last() else { return at + 1 };
        let space = match (up.count, block.kind) {
            (0, _) => 0.0,
            (_, Kind::Heading(_)) => up.gap + self.size.gap,
            _ => up.gap,
        };
        let (x, w, y, size) = (up.x, up.w, up.pen + space, self.size);
        if let Some(up) = self.shown.open.last_mut() {
            up.count += 1;
        }
        self.shown.areas[at].rect = rect(x, y, w, 0.0);
        match block.kind {
            Kind::Quote => {
                let inset = self.indent(w, 3.0 + size.inset);
                self.open(at, [x + inset, w - inset, y], size.gap, up.lists);
            }
            Kind::List { numbered, tight } => self.list(at, numbered, tight, [x, y, w], up.lists),
            Kind::Item { .. } => {
                let t = self.given.t;
                let ([above, below], high) = (heights(self.given.system, t.font, self.given.scale, &mut self.shown.fonts), least(Kind::Paragraph, t.font));
                let list = self.shown.areas[up.block].nth;
                (self.shown.areas[at].mark, self.shown.areas[at].nth) = (((high - above - below) / 2.0 + above).round(), [up.count + 1, list[1]]);
                self.open(at, [x, w, y], up.gap, up.lists);
            }
            Kind::Table => return self.table(at, [x, y, w]),
            Kind::Rule => {
                self.shown.areas[at].rect.h = 1.0;
                self.ends(at);
            }
            kind if worded(kind) => {
                self.shown.areas[at].rect.h = self.leaf(at, x, y, w);
                self.ends(at);
            }
            _ => {}
        }
        at + 1
    }

    /// How far a block `w` wide indents its content, `by` where that leaves it wide enough.
    fn indent(&self, w: f32, by: f32) -> f32 {
        if w - by >= narrowest(self.given.t) { by } else { 0.0 }
    }

    /// Opens a container block: content at `x`, `w` wide, from `y` down, with `gap` between;
    /// inside `lists` lists.
    fn open(&mut self, at: usize, [x, w, y]: [f32; 3], gap: f32, lists: u32) {
        self.shown.open.push(Open { block: at, x, w: w.max(1.0), top: y, pen: y, gap, count: 0, lists });
    }

    /// Opens a list: items indented by the marker column (for numbers, as wide as the widest),
    /// with little gap in a tight list.
    fn list(&mut self, at: usize, numbered: bool, tight: bool, [x, y, w]: [f32; 3], lists: u32) {
        let [from, to] = self.shown.areas[at].kids.map(|kid| kid as usize);
        let numbers = self.shown.kids[from..to].iter().map(|item| self.writing.blocks[*item as usize].kind);
        let last = numbers.fold(0, |last, kind| if let Kind::Item { number, .. } = kind { last.max(number) } else { last });
        let t = self.given.t;
        let marks = match numbered {
            true => (self.given.system.measure(&format!("{last}."), t.font, self.given.scale, None) + 2.0 * self.size.close + 4.0).ceil().max(self.size.marks),
            false => self.size.marks,
        };
        (self.shown.areas[at].mark, self.shown.areas[at].nth) = (marks, [lists + 1, (to - from) as u32]);
        let inset = self.indent(w, marks);
        self.open(at, [x + inset, w - inset, y], if tight { self.size.close } else { self.size.gap }, lists + 1);
    }

    /// Wraps block `at`'s text into rows at `x`, `y`, in width `w` (code padded and never
    /// wrapped, a level 1 or 2 heading underlined). Returns its height.
    fn leaf(&mut self, at: usize, x: f32, y: f32, w: f32) -> f32 {
        let given = &mut *self.given;
        self.shown.measure_through(self.doc, at, given);
        let block = self.writing.blocks[at];
        let font = font_of(block.kind, heads(self.writing, at), given.t);
        let [above, below] = heights(given.system, font, given.scale, &mut self.shown.fonts);
        let (pad, side, wide) = match block.kind {
            Kind::Code(_) => ([self.size.pad; 2], Align::Start, f32::MAX),
            Kind::Cell(side) => (self.size.cell, side, w - 2.0 * self.size.cell[0]),
            _ => ([0.0; 2], Align::Start, w),
        };
        let room = Room { above, below, high: least(block.kind, font), left: x + pad[0], wide: wide.max(1.0), side };
        let ([from, to], first) = (self.shown.areas[at].bits, self.shown.laid.rows.len() as u32);
        let high = lay(&mut self.shown.laid, from..to, &self.writing.words, block.words[0], room, y + pad[1], given.scale);
        self.shown.areas[at].rows = [first, self.shown.laid.rows.len() as u32];
        self.shown.laid_bytes = self.shown.laid_bytes.max(block.words[1]);
        self.shown.long = self.shown.long.max(x + 2.0 * pad[0] + self.shown.long(at));
        let under = if matches!(block.kind, Kind::Heading(1 | 2)) { (font.size * 0.3).round() + 1.0 } else { 0.0 };
        high + 2.0 * pad[1] + under
    }

    /// A table at `x`, `y` in width `w`: each column as wide as its widest unwrapped cell, all
    /// narrowed alike if that exceeds the room; each row as high as its tallest cell. Returns
    /// the block after it.
    fn table(&mut self, at: usize, [x, y, w]: [f32; 3]) -> usize {
        let end = self.writing.blocks[at].end as usize;
        self.shown.measure_through(self.doc, end - 1, self.given);
        let rows: Vec<usize> = self.kids(at).collect();
        let mut wants: Vec<f32> = Vec::new();
        for row in &rows {
            for (column, cell) in self.kids(*row).enumerate() {
                let want = self.shown.long(cell) + 2.0 * self.size.cell[0] + 1.0;
                wants.resize(wants.len().max(column + 1), 0.0);
                wants[column] = wants[column].max(want);
            }
        }
        let total: f32 = wants.iter().sum();
        let narrowed = if total > w { w / total } else { 1.0 };
        let widths: Vec<f32> = wants.iter().map(|want| (want * narrowed).floor()).collect();
        let (mut top, body) = (y, rows.iter().filter(|row| self.writing.blocks[**row].kind == Kind::Row { head: false }).count() as u32);
        for row in rows {
            let head = self.writing.blocks[row].kind == Kind::Row { head: true };
            let before = self.shown.areas[at].nth[0];
            self.shown.areas[row].nth = if head { [u32::MAX, body] } else { [before, body] };
            self.shown.areas[at].nth[0] += !head as u32;
            top += self.table_row(row, x, top, &widths);
        }
        (self.shown.areas[at].rect, self.shown.areas[at].nth) = (rect(x, y, widths.iter().sum(), top - y), [body, wants.len() as u32]);
        self.shown.long = self.shown.long.max(x + total);
        self.ends(at);
        end
    }

    /// A table row at `top`, its cells in columns `widths` wide from `x`. Returns its height.
    fn table_row(&mut self, row: usize, x: f32, top: f32, widths: &[f32]) -> f32 {
        let (mut left, mut high) = (x, 0.0f32);
        let cells: Vec<usize> = self.kids(row).collect();
        for (column, cell) in cells.iter().copied().enumerate() {
            let wide = widths.get(column).copied().unwrap_or(0.0);
            let cell_high = self.leaf(cell, left, top, wide);
            (self.shown.areas[cell].rect, left, high) = (rect(left, top, wide, cell_high), left + wide, high.max(cell_high));
        }
        for cell in cells {
            self.shown.areas[cell].rect.h = high;
        }
        self.shown.areas[row].rect = rect(x, top, left - x, high);
        high
    }

    /// Block `at`'s children.
    fn kids(&self, at: usize) -> impl Iterator<Item = usize> + use<'_> {
        let [from, to] = self.shown.areas[at].kids;
        self.shown.kids[from as usize..to as usize].iter().map(|kid| *kid as usize)
    }
}

/// A document while described: its blocks and text, layout, the part in view (from its top);
/// the links under the pointer or focus; the table being described, by its note and columns;
/// and each code block described, with how far it is scrolled sideways.
struct Page<'a> {
    shape: Shape<'a>,
    shown: &'a Shown,
    writing: &'a Writing,
    view: [f32; 2],
    lit: Vec<u32>,
    table: Option<(usize, [u32; 2])>,
    code_scrolls: Vec<(u32, f32)>,
}

/// Markdown documents.
impl Ui {
    /// Draws a document [`Markdown::parse`] parsed: headings, paragraphs, nested lists, quotes,
    /// code highlighted by its language (scrolling sideways where wide), tables, task check
    /// boxes, rules, and images the app supplies by name ([`Markdown::set_picture`]). As wide
    /// as its room, as high as it comes out; in a scroll box, only what is in view is
    /// described, and only what has been in view is laid out.
    ///
    /// Select with the pointer (double click: word, triple: block, Shift extends; dragged past
    /// the view's edge, it scrolls) or, once focused, with the keys (Shift+arrows, Home/End;
    /// Ctrl+Shift for words and the ends; Ctrl+A); Ctrl+C copies. Its context menu copies,
    /// selects all, and on a link copies the link. Tab visits it, each code block (arrows
    /// scroll it), then each link; a link's title shows while it is hovered or focused. Screen
    /// readers get a document: headings with levels, paragraphs, lists and items, quotes, code,
    /// tables with their headings, links with their titles, and the selection.
    ///
    /// Returns the target of a clicked link. A link to one of its own headings
    /// (`#what-it-says`) is followed by the document itself, which scrolls to the heading.
    pub fn markdown<'a>(&mut self, doc: &'a Markdown) -> Option<&'a str> {
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let page = self.open.first().map_or(0, |open| open.at);
        // The view is what the nearest enclosing scroll box shows.
        let within = self.open.iter().rev().map(|open| open.at).find(|at| self.tree.specs[*at as usize].frame.scroll[1]).unwrap_or(page);
        let view_id = hash((id, "in view"));
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, 0.0, 0.0]);
        let seen = self.rooms.get(&view_id).copied().unwrap_or([0.0, self.size().h, 0.0]);
        let scrolled = self.tree.specs[within as usize].offset[1];
        let view = [scrolled - seen[2], scrolled - seen[2] + seen[1]];
        let mut kept = self.take_writings();
        let shown = kept.documents.entry(id).or_default();
        self.fit_document(shown, doc, read[0], view, within);
        let state = self.interact(id);
        let corner = self.document_corner(&state, within, seen[2]);
        self.document_pointer(shown, doc, &state, corner);
        self.drag_past_edge(shown, &state, within);
        if state.focused {
            self.document_keys(shown, doc, within, view);
        }
        // Its height is its blocks', so layout's result is known ahead: its width and its place
        // in the scroll box are read back, and it is described again if either differs.
        self.measures.push(Measure { at, within: at, id, read: [read[0], shown.high, 0.0] });
        self.measures.push(Measure { at, within, id: view_id, read: seen });
        let whole = self.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Px(shown.high)).sense().focusable().open();
        let [from, to] = shown.title;
        self.tell(&whole, Role::Document, None, doc.writing.words.get(from as usize..to as usize).unwrap_or(""));
        // In a box that sizes to its content, it asks for its widest block.
        self.el().w(Length::Px(shown.long)).show();
        let shape = Shape { laid: &shown.laid, text: &doc.writing.words, links: &shown.links, titles: &doc.links, picked: in_order(shown.picked), keyed: state.focused };
        let mut page = Page { shape, shown, writing: &doc.writing, view, lit: Vec::new(), table: None, code_scrolls: Vec::new() };
        let (links, apart) = self.document_blocks(&mut page);
        self.close();
        let code_scrolls = page.code_scrolls;
        (shown.used, shown.code_scrolls) = (true, code_scrolls);
        self.document_menu(shown, doc, &whole, &links);
        self.follow_focus(shown, doc, [links.focused, apart], within, view);
        let link = doc.links.get(links.pressed.wrapping_sub(1) as usize).map(|(url, _)| url.as_str());
        let heading = link
            .and_then(|url| url.strip_prefix('#'))
            .and_then(|name| doc.writing.blocks.iter().position(|block| matches!(block.kind, Kind::Heading(_)) && slug(&doc.writing.words[block.words[0] as usize..block.words[1] as usize]) == name));
        if let Some(at) = heading {
            self.lay_document(shown, doc, 0.0, at);
            self.scroll_document(within, shown.areas[at].rect.y - view[0]);
        }
        self.keep_writings(kept);
        link.filter(|_| heading.is_none())
    }

    /// Updates a document's cache: started again for another parse, theme or scale, laid out
    /// again for another width; then laid out to a screen past `view`. Scrolled to its end, it
    /// stays there as laying out corrects its height.
    fn fit_document(&mut self, shown: &mut Shown, doc: &Markdown, width: f32, view: [f32; 2], within: u32) {
        let (t, scale) = (self.theme, self.scale);
        if shown.serial != doc.serial || shown.theme != Some(t) || shown.scale != scale {
            shown.start(doc, &t, scale);
        }
        if shown.width != width {
            shown.start_layout(width, &t);
        }
        let (high, at_end) = (shown.high, shown.high > view[1] - view[0] && view[1] >= shown.high - 0.5);
        self.lay_document(shown, doc, view[1] + (view[1] - view[0]).max(0.0), 0);
        if at_end && shown.high != high {
            self.scroll_document(within, shown.high - high);
        }
    }

    /// Lays out more of a document, down to `bottom` and through block `block`.
    fn lay_document(&mut self, shown: &mut Shown, doc: &Markdown, bottom: f32, block: usize) {
        let t = self.theme;
        let mut given = Given { t: &t, system: self.text.as_mut(), scale: self.scale, pictures: &self.pictures };
        shown.lay_more(doc, &mut given, bottom, block);
    }

    /// Notes the focused link. One Tab just reached through its stand-in box (`apart`) is laid
    /// out, and scrolled to (in scroll box `within`, now showing `view`).
    fn follow_focus(&mut self, shown: &mut Shown, doc: &Markdown, [focused, apart]: [u32; 2], within: u32, view: [f32; 2]) {
        shown.focused = focused;
        let Some(words) = shown.links.get(focused.wrapping_sub(1) as usize).filter(|_| focused == apart) else { return };
        self.lay_document(shown, doc, 0.0, block_of(&doc.writing, words[0]));
        let first = shown.targets.partition_point(|target| link_of(shown, target) < focused);
        if let Some((row, _)) = shown.targets.get(first) {
            self.show_document_row(within, view, shown.laid.rows[*row as usize]);
        }
        self.again = true;
    }

    /// Scrolls the document's scroll box (`within`, now showing `view`) as little as brings a
    /// row into view.
    fn show_document_row(&mut self, within: u32, view: [f32; 2], row: crate::rich_text::Row) {
        let by = (row.top - view[0]).min(0.0) + (row.top + row.high - view[1]).max(0.0);
        if by != 0.0 {
            self.scroll_document(within, by);
        }
    }

    /// A document box's top left in the window, for the pointer: x where its box is seen; y
    /// where it stands in its scroll box (`within`), `top` below that box's start, as currently
    /// scrolled. `None` before it was first seen.
    fn document_corner(&self, state: &Response, within: u32, top: f32) -> Option<Point> {
        if state.rect.is_empty() {
            return None;
        }
        let spec = &self.tree.specs[within as usize];
        let scroller = self.visible_boxes.scrollers.iter().find(|scroller| scroller.id == spec.id && spec.frame.scroll[1]);
        Some(pt(state.rect.x, scroller.map_or(state.rect.y, |scroller| scroller.rect.y + spec.frame.pad.top + top - spec.offset[1])))
    }

    /// The pointer on a document: a press selects.
    fn document_pointer(&mut self, shown: &mut Shown, doc: &Markdown, state: &Response, corner: Option<Point>) {
        if state.hovered {
            self.set_cursor(Cursor::Text);
        }
        let (Some(p), Some(corner), true) = (self.input.pointer, corner, state.pressed) else { return };
        let at = shown.hit(&doc.writing.words, p.x - corner.x, p.y - corner.y);
        let clicks = if self.input.pressed[0] && state.hovered { self.input.clicks } else { 0 };
        shown.point(&doc.writing, at, clicks, self.input.mods.shift);
    }

    /// While a selection is dragged near the edge of the document's scroll box (`within`) or
    /// past it, the box scrolls that way, faster further out, and runs again until it stops.
    fn drag_past_edge(&mut self, shown: &mut Shown, state: &Response, within: u32) {
        let spec = &self.tree.specs[within as usize];
        let scroller = self.visible_boxes.scrollers.iter().find(|scroller| scroller.id == spec.id && spec.frame.scroll[1]).map(|scroller| scroller.rect);
        let (Some(view), Some(p), true) = (scroller, self.input.pointer, state.pressed && self.input.down[0]) else { return shown.dragged_at = 0 };
        let near = EDGE.min(view.h / 4.0);
        let push = (p.y - (view.bottom() - near)).max(0.0) - (view.y + near - p.y).max(0.0);
        if push == 0.0 || near <= 0.0 {
            shown.dragged_at = 0;
            return;
        }
        let now = self.now.as_millis() as u16 | 1;
        let since = if shown.dragged_at == 0 { TICK.as_millis() as u16 } else { now.wrapping_sub(shown.dragged_at).min(50) };
        let [across, down] = spec.offset;
        self.scrolls.insert(spec.id, [across, (down + push * SPEED * since as f32 / 1000.0).max(0.0)]);
        shown.dragged_at = now;
        self.wake_after(TICK);
    }

    /// Keys while a document has focus: Ctrl+A selects all, Ctrl+C copies, and Shift with
    /// arrows or Home/End (with Ctrl: by words, and to the ends) extends the selection,
    /// scrolling to it.
    fn document_keys(&mut self, shown: &mut Shown, doc: &Markdown, within: u32, view: [f32; 2]) {
        let text = &doc.writing.words;
        if self.key(Key::Char('a'), Mods::CTRL) {
            shown.picked = [0, text.trim_end_matches('\n').len() as u32];
        }
        if self.key(Key::Char('c'), Mods::CTRL) {
            self.copy_selection(shown, doc);
        }
        let (shift, both) = (Mods::SHIFT, Mods { ctrl: true, shift: true, alt: false });
        let keys = [
            (Key::Left, shift),
            (Key::Right, shift),
            (Key::Left, both),
            (Key::Right, both),
            (Key::Home, shift),
            (Key::End, shift),
            (Key::Up, shift),
            (Key::Down, shift),
            (Key::Home, both),
            (Key::End, both),
        ];
        let Some(pressed) = keys.iter().position(|(key, mods)| self.key(*key, *mods)) else { return };
        let (cursor, row) = (shown.picked[1], shown.row_at(shown.picked[1]));
        let away = |up: bool| if up { row.saturating_sub(1) } else { (row + 1).min(shown.laid.rows.len().saturating_sub(1)) };
        shown.picked[1] = match pressed {
            0 | 1 => step(text, cursor as usize, pressed == 1) as u32,
            2 | 3 => word(text, cursor as usize, pressed == 3) as u32,
            4 | 5 => shown.laid.rows.get(row).map_or(cursor, |row| row.words[pressed - 4]),
            6 | 7 if !shown.laid.rows.is_empty() => shown.laid.at_x(text, away(pressed == 6), shown.laid.x_at(text, row, cursor)),
            8 => 0,
            9 => text.len() as u32,
            _ => cursor,
        };
        // Extended out of view, it scrolls there (laid out that far first).
        self.lay_document(shown, doc, 0.0, block_of(&doc.writing, shown.picked[1]));
        if let Some(row) = shown.laid.rows.get(shown.row_at(shown.picked[1])) {
            self.show_document_row(within, view, *row);
        }
    }

    /// Copies the selected text, if any.
    fn copy_selection(&mut self, shown: &Shown, doc: &Markdown) {
        let [from, to] = in_order(shown.picked);
        if from < to {
            self.copy(&doc.writing.words[from as usize..to as usize].replace(THING, ""));
        }
    }

    /// The document's context menu, opened on it or on a link (`links`): copy, select all, and
    /// on a link, copy its address.
    fn document_menu(&mut self, shown: &mut Shown, doc: &Markdown, whole: &Response, links: &Links) {
        let (clicked, focused) = (whole.right_clicked || links.right_clicked != 0, whole.focused || links.focused != 0);
        let menu_keys = |stroke: &Stroke| matches!(stroke, Stroke::Key(Key::Menu, Mods::NONE) | Stroke::Key(Key::F(10), Mods::SHIFT));
        if clicked || focused && self.input.strokes.iter().any(menu_keys) {
            shown.menu_link = if clicked { links.right_clicked } else { links.focused };
        }
        if !self.context_menu(&Response { right_clicked: clicked, focused, ..*whole }) {
            return;
        }
        let [from, to] = in_order(shown.picked);
        self.set_enabled(from < to);
        if self.menu_item(self.edit_words[1]).clicked {
            self.copy_selection(shown, doc);
        }
        self.set_enabled(true);
        if self.menu_item(self.edit_words[3]).clicked {
            shown.picked = [0, doc.writing.words.trim_end_matches('\n').len() as u32];
        }
        let address = doc.links.get(shown.menu_link.wrapping_sub(1) as usize).map(|(url, _)| url.as_str());
        if address.is_some_and(|_| self.menu_item(&doc.copy_link_word).clicked) {
            self.copy(address.unwrap_or_default());
        }
        self.close();
    }

    /// Scrolls the document's scroll box (`within`) by `by`, clamped: from the next run.
    fn scroll_document(&mut self, within: u32, by: f32) {
        let spec = &self.tree.specs[within as usize];
        if !spec.frame.scroll[1] {
            return;
        }
        let [across, down] = spec.offset;
        self.scrolls.insert(spec.id, [across, (down + by).max(0.0)]);
        self.again = true;
    }

    /// The document's blocks in view, each a box inside its parent block's box; then its links'
    /// boxes. Returns what was done to its links, and the link given a stand-in box (0 for none).
    fn document_blocks(&mut self, page: &mut Page) -> (Links, u32) {
        let (blocks, shown, view, rule) = (&page.writing.blocks, page.shown, page.view, self.theme.line_strong);
        let order = shown.in_view(view);
        let (targets, apart) = shown.reachable(page.writing, &order, view);
        let mut links = self.link_states(&page.shape, &targets);
        if apart != 0 {
            links.note(apart, self.link_state(apart, 0));
        }
        page.lit = links.lit.clone();
        // The containers entered, innermost last (the page is the document's own box).
        let mut open = vec![0];
        for at in order.into_iter().map(|at| at as usize) {
            while open.len() > 1 && open.last().is_some_and(|open| blocks[*open].end as usize <= at) {
                self.close();
                open.pop();
            }
            let (area, up) = (shown.areas[at], shown.areas[open.last().copied().unwrap_or(0)].rect);
            let r = area.rect.moved(-up.x, -up.y);
            match blocks[at].kind {
                Kind::Rule => _ = self.el().offset(r.x, r.y).size(r.w, 1.0).fill(rule).show(),
                Kind::Code(_) => self.document_code(page, at, r),
                kind if worded(kind) => self.document_leaf(page, at, r),
                _ => self.document_box(page, at, r, &mut open),
            }
        }
        for _ in 1..open.len() {
            self.close();
        }
        self.link_boxes(&page.shape, &targets, [0.0; 2]);
        if apart != 0 {
            // (A link not laid out yet: a box at the foot of the view, or of what is laid out,
            // which Tab reaches.)
            let [from, to] = shown.links[apart as usize - 1];
            let name = page.writing.words.get(from as usize..to as usize).unwrap_or("");
            self.titled_link_box(&page.shape, apart, 0, rect(0.0, (view[1].min(shown.bottom) - 1.0).max(0.0), 1.0, 1.0), name);
        }
        (links, apart)
    }

    /// A container block at `r` in its parent's box: told as what it is, with its marks (a
    /// quote's bar, an item's marker, a table's lines). Left open, and pushed onto `open`.
    fn document_box(&mut self, page: &mut Page, at: usize, r: Rect, open: &mut Vec<usize>) {
        open.push(at);
        let (t, kind, area) = (self.theme, page.writing.blocks[at].kind, page.shown.areas[at]);
        let (fill, line) = match kind {
            Kind::Table => (Color::NONE, 1.0),
            Kind::Row { head: true } => (t.hover, 0.0),
            _ => (Color::NONE, 0.0),
        };
        if let (Kind::Row { head: false }, Some((table, columns))) = (kind, page.table) {
            self.tell_row(area.nth[0], columns, table);
        }
        let state = self.el().flow(Flow::Stack).offset(r.x, r.y).size(r.w, r.h).fill(fill).border(t.line, line).open();
        match kind {
            Kind::Quote => {
                self.tell(&state, Role::Quote, None, "");
                self.el().size(3.0, r.h).fill(t.line_strong).radius(1.5).show();
            }
            Kind::List { .. } => self.tell(&state, Role::List, None, ""),
            Kind::Item { number, ticked } => {
                self.tell(&state, Role::ListItem, ticked, "");
                self.tell_position(area.nth[0] as usize, area.nth[1] as usize);
                self.document_mark(page, at, number, ticked);
            }
            Kind::Table => {
                page.table = self.tell_document_table(&state, area.nth);
                self.document_table_lines(page, at, r);
            }
            _ => {}
        }
    }

    /// Tells screen readers box `of` is a table of `[rows, columns]`, its header row apart.
    /// Returns its note and its columns, while they listen.
    fn tell_document_table(&mut self, of: &Response, [rows, columns]: [u32; 2]) -> Option<(usize, [u32; 2])> {
        if !self.listening {
            return None;
        }
        self.tell_table(of, columns as usize);
        let table = self.notes.len() - 1;
        *self.table_row_count(table)? = rows;
        Some((table, self.plain_columns(columns as usize)))
    }

    /// An item's marker, left of it: a check box for a task, its number in a numbered list,
    /// else a bullet (disc, circle, square, by depth).
    fn document_mark(&mut self, page: &Page, at: usize, number: u32, ticked: Option<bool>) {
        let t = self.theme;
        let list = page.shown.areas[page.writing.blocks[at].parent as usize];
        let ([room, mark], [depth, _]) = ([list.mark, page.shown.areas[at].mark], list.nth);
        let numbered = matches!(page.writing.blocks[page.writing.blocks[at].parent as usize].kind, Kind::List { numbered: true, .. });
        let [above, _] = heights(self.text.as_mut(), t.font, self.scale, &mut Vec::new());
        // The x-height's middle on the marker's row.
        let middle = mark - (above * 0.35).round();
        match (ticked, numbered) {
            (Some(done), _) => self.document_tick(-room + (room - 14.0) / 2.0, middle - 7.0, done),
            (None, true) => {
                self.el().text(&format!("{number}."), t.font, t.text_dim).select(None, false).text_align(Align::End).w(Length::Px(room - 6.0)).offset(-room, mark - above).show();
                // Screen readers announce position in the list: the number is not told on its
                // own, but as part of the item.
                self.tell_words("", usize::MAX);
            }
            (None, false) => {
                let dot = self.el().offset(-room / 2.0 - 3.0, middle - 3.0).size(6.0, 6.0).radius(if depth >= 3 { 0.0 } else { 3.0 });
                _ = if depth == 2 { dot.border(t.text_dim, 1.0) } else { dot.fill(t.text_dim) }.show();
            }
        }
    }

    /// A task's check box at `x`, `y` in its item's box: filled, with a tick, when done.
    fn document_tick(&mut self, x: f32, y: f32, done: bool) {
        let t = self.theme;
        let (edge, fill) = if done { (t.accent, t.accent) } else { (t.line_strong, Color::NONE) };
        let tick = self.el().offset(x, y).size(14.0, 14.0).radius(3.0).border(edge, 1.0).fill(fill);
        if !done {
            tick.show();
            return;
        }
        tick.flow(Flow::Stack).open();
        self.el().icon(crate::widgets::CHECK, 14.0, t.on_accent).show();
        self.close();
    }

    /// A table's lines, in its box before its rows (whose boxes hold only their cells): between
    /// its columns, down all of it; and above each row in view but the first.
    fn document_table_lines(&mut self, page: &Page, at: usize, r: Rect) {
        let (line, shown, table) = (self.theme.line, page.shown, page.shown.areas[at]);
        let [from, to] = table.kids.map(|kid| kid as usize);
        let Some(&first) = shown.kids[from..to].first() else { return };
        let [cells, end] = shown.areas[first as usize].kids.map(|kid| kid as usize);
        for cell in &shown.kids[(cells + 1).min(end)..end] {
            self.el().offset(shown.areas[*cell as usize].rect.x - table.rect.x, 0.0).size(1.0, r.h).fill(line).show();
        }
        for row in &shown.kids[shown.first_in_view(at, page.view).max(from + 1)..to] {
            let top = shown.areas[*row as usize].rect.y;
            if top > page.view[1] {
                break;
            }
            self.el().offset(0.0, top - table.rect.y).size(r.w, 1.0).fill(line).show();
        }
    }

    /// A text block at `r` in its parent's box: told as what it is (a heading with its level, a
    /// paragraph; a header cell, or a cell by its text), with its rows in view and the part of
    /// the selection in it.
    fn document_leaf(&mut self, page: &Page, at: usize, r: Rect) {
        let (t, block, area) = (self.theme, page.writing.blocks[at], page.shown.areas[at]);
        let state = self.el().flow(Flow::Stack).offset(r.x, r.y).size(r.w, r.h).open();
        let words = page.writing.words.get(block.words[0] as usize..block.words[1] as usize).unwrap_or("").trim_end();
        match block.kind {
            Kind::Heading(level) => {
                self.tell(&state, Role::Heading, None, words);
                self.tell_level(level as usize);
            }
            Kind::Cell(_) if page.table.is_some() && heads(page.writing, at) => {
                self.tell(&state, Role::ColumnHeader, None, words);
                let [from, to] = page.shown.areas[block.parent as usize].kids;
                let column = page.shown.kids[from as usize..to as usize].iter().position(|cell| *cell as usize == at).unwrap_or(0);
                self.tell_position(column + 1, (to - from) as usize);
            }
            Kind::Cell(_) => {}
            _ => self.tell(&state, Role::Paragraph, None, words),
        }
        if !matches!(block.kind, Kind::Cell(_)) {
            self.tell_picked(page, at);
        }
        if matches!(block.kind, Kind::Heading(1 | 2)) {
            self.el().offset(0.0, r.h - 1.0).size(r.w, 1.0).fill(t.line).show();
        }
        let [from, to] = page.shown.rows_in_view(at, page.view);
        self.write_rows(&page.shape, from..to, [-area.rect.x, -area.rect.y], &page.lit);
        self.close();
    }

    /// Adds to the text block just told: the part of the selection in it, in characters from
    /// its start (anchor and cursor, each at its nearer end where outside).
    fn tell_picked(&mut self, page: &Page, at: usize) {
        let ([from, to], [start, end]) = (in_order(page.shown.picked), page.writing.blocks[at].words);
        if !self.listening || from >= to || to <= start || from >= end {
            return;
        }
        let letters = |byte: u32| page.writing.words[start as usize..byte.clamp(start, end) as usize].chars().count() as u32;
        self.tell_caret(page.shown.picked.map(letters));
    }

    /// A code block at `r`: told as code, its text in rows never wrapped. Where wider than the
    /// page it scrolls sideways, by the wheel or, once Tab reaches it, by Left, Right, Home and
    /// End.
    fn document_code(&mut self, page: &mut Page, at: usize, r: Rect) {
        let (t, block, area, pad) = (self.theme, page.writing.blocks[at], page.shown.areas[at], sizes(&self.theme).pad);
        let state = self.el().key(("code", at)).flow(Flow::Stack).offset(r.x, r.y).size(r.w, r.h).fill(t.panel).border(t.line, 1.0).radius(t.radius).scroll_x().focusable().open();
        let words = page.writing.words.get(block.words[0] as usize..block.words[1] as usize).unwrap_or("").trim_end();
        self.tell(&state, Role::Code, None, words);
        self.tell_picked(page, at);
        let (wide, box_at) = ((page.shown.long(at) + 2.0 * pad).max(r.w), self.open.last().map_or(0, |open| open.at));
        let across = self.tree.specs[box_at as usize].offset[0];
        page.code_scrolls.push((at as u32, across));
        self.el().size(wide, r.h).show();
        let [from, to] = page.shown.rows_in_view(at, page.view);
        self.write_rows(&page.shape, from..to, [-area.rect.x, -area.rect.y], &page.lit);
        self.close();
        // (A box that takes focus but no pointer is told focus by the window, not its response.)
        if state.id == self.focus {
            self.code_keys(state.id, across, wide - r.w);
        }
    }

    /// Keys on a focused code block: Left and Right scroll it sideways a little, Home and End
    /// to its ends (`most` its widest scroll).
    fn code_keys(&mut self, id: crate::id::Id, across: f32, most: f32) {
        let step = (self.theme.font.size * 3.0).round();
        let to = match () {
            _ if self.key(Key::Left, Mods::NONE) => across - step,
            _ if self.key(Key::Right, Mods::NONE) => across + step,
            _ if self.key(Key::Home, Mods::NONE) => 0.0,
            _ if self.key(Key::End, Mods::NONE) => most,
            _ => return,
        };
        self.scrolls.insert(id, [to.clamp(0.0, most.max(0.0)), 0.0]);
        self.again = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Size;

    #[test]
    fn only_what_is_in_view_of_a_long_document_is_described() {
        let text: String = (0..5000)
            .map(|n| {
                format!(
                    "Paragraph {n}, of [a few](#) words, *some* of them `code`.

"
                )
            })
            .collect();
        let doc = Markdown::parse(&text);
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
        let mut build = |ui: &mut Ui| {
            ui.column().grow().scroll().open();
            ui.markdown(&doc);
            ui.close();
        };
        ui.run(&mut build);
        ui.run(&mut build);
        // A screenful: some fifteen paragraphs of a few boxes each, and their links.
        let boxes = ui.tree.specs.len();
        assert!(boxes < 200, "{boxes} boxes");
        // Scrolled to the end, the last paragraphs are described.
        ui.event(crate::event::Event::Move(crate::geom::pt(200.0, 150.0)));
        ui.event(crate::event::Event::Wheel(0.0, 1e9));
        for _ in 0..4 {
            ui.run(&mut build);
        }
        let last = ui.tree.strings.contains("Paragraph 4999");
        assert!(last && !ui.tree.strings.contains("Paragraph 0,"));
    }

    /// A window 420 by 300 written with `system`, showing `doc` in a scroll box; and how to
    /// describe it.
    fn shown_in_a_window(system: Box<dyn TextSystem>) -> Ui {
        let mut ui = Ui::new(system, Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
        ui
    }

    /// Reads the state the window keeps for its one document.
    fn with_shown<T>(ui: &mut Ui, read: impl Fn(&Shown) -> T) -> T {
        let kept = ui.take_writings();
        let found = read(kept.documents.values().next().expect("a document"));
        ui.keep_writings(kept);
        found
    }

    #[test]
    fn a_long_document_is_measured_and_laid_out_only_as_far_as_it_is_seen() {
        let text: String = (0..10_000).map(|n| format!("Line {n} of a long document.\n\n")).collect();
        let doc = Markdown::parse(&text);
        let mut ui = shown_in_a_window(Box::new(crate::text::BuiltInFont));
        let mut build = |ui: &mut Ui| {
            ui.column().grow().scroll().open();
            ui.markdown(&doc);
            ui.close();
        };
        ui.run(&mut build);
        ui.run(&mut build);
        // A screen in view and one below it: some twenty of the ten thousand paragraphs, and an
        // estimate of the rest's height.
        let (measured, laid, high) = with_shown(&mut ui, |shown| (shown.measured, shown.next, shown.high));
        assert!(measured < 60 && laid < 60, "{measured} measured, {laid} laid out");
        assert!(high > 300_000.0, "{high}");
        // At the end, everything is, and the end shows.
        ui.event(crate::event::Event::Move(crate::geom::pt(200.0, 150.0)));
        ui.event(crate::event::Event::Wheel(0.0, 1e9));
        for _ in 0..3 {
            ui.run(&mut build);
        }
        let (measured, count) = with_shown(&mut ui, |shown| (shown.measured, shown.areas.len()));
        assert_eq!(measured, count);
        assert!(ui.tree.strings.contains("Line 9999 "));
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_right_to_left_paragraph_of_a_document_ends_at_the_right() {
        // In the small Hebrew test font (alef 7, gimel 10, space 4, Latin 12): Hebrew first,
        // its second word in italics, then Latin. The paragraph ends at the right edge; the
        // italic word shows left of the first; the Latin, left of both.
        let doc = Markdown::parse("\u{5d0}\u{5d2} *\u{5d2}\u{5d0}* cd\n\nab\n");
        let mut ui = shown_in_a_window(Box::new(crate::bidi::tests::hebrew_letters()));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let shown = with_shown(&mut ui, |shown| {
            let row = shown.laid.rows[0];
            let frags = &shown.laid.frags[row.frags[0] as usize..row.frags[1] as usize];
            frags.iter().map(|frag| (doc.writing.words[frag.words[0] as usize..frag.words[1] as usize].to_string(), frag.x, frag.x + frag.wide)).collect::<Vec<_>>()
        });
        let place = |words: &str| shown.iter().find(|(own, ..)| own == words).map(|(_, left, right)| [*left, *right]).expect(words);
        let [first, italic, latin] = [place("\u{5d0}\u{5d2} "), place("\u{5d2}\u{5d0}"), place("cd")];
        assert!((first[1] - 420.0).abs() < 1.0, "{shown:?}");
        assert!(latin[1] <= italic[0] && italic[1] <= first[0], "{shown:?}");
    }

    #[test]
    fn a_heading_is_known_by_its_words_as_github_writes_them() {
        assert_eq!(slug("What is in it"), "what-is-in-it");
        assert_eq!(slug(" Beside Dear ImGui! "), "beside-dear-imgui");
        assert_eq!(slug("C++ and snake_case"), "c-and-snake_case");
    }

    #[test]
    fn code_is_coloured_by_the_rules_of_the_language_it_names() {
        let mut doc = Markdown::parse("```rs\nfn x() {}\n```\n\n```toml\na = 1\n```\n");
        assert_eq!(doc.writing.languages, ["rs", "toml"]);
        assert_eq!(rules_for(&doc, "rs"), Some(&Syntax::RUST));
        assert_eq!(rules_for(&doc, "toml"), None);
        doc.set_syntax("TOML", &Syntax::PYTHON);
        assert_eq!(rules_for(&doc, "toml"), Some(&Syntax::PYTHON));
    }
}
