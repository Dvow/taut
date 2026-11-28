//! A Markdown document drawn ([`Ui::markdown`]): parsed text (`rich_text::Writing`) laid out as
//! a page, selectable and copyable by pointer, keys and its context menu, and told to screen
//! readers as a document.
//!
//! It is laid out as a page, not box by box, and only around what is seen. Blocks are placed
//! top down and their text wrapped into rows (`rich_text.rs`), each text run measured once, as
//! its block is first laid out: a stretch from a screen above what shows to a screen below,
//! which may start at any block. Above and below it the height is estimated from the text's
//! length; a far jump lays out a new stretch where the estimate puts it, and what shows stays in
//! place as the estimate is corrected. Only blocks in view are described, found by halving each
//! container's children, each a box inside its parent block's box: a document of any length
//! costs a screenful each run. Boxes nest as blocks do, so screen readers get each block as what
//! it is, and a table as rows and cells under its headings. Links in view get boxes after the
//! blocks, with the link before and after them, so Tab visits every link in order and scrolls
//! to each.
//!
//! Nesting has no limit here: containers are walked with stacks of their own, not by calling
//! back in, and indents stop where they would leave the text too narrow.
use crate::accessibility::{AccessNode, Letters, Role};
use crate::code_editor::Syntax;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::{Id, fold, hash};
use crate::layout::{Align, Flow, Length};
use crate::markdown::Markdown;
use crate::rich_text::{Bit, Kind, Laid, Links, NO_WRAP, Room, Shape, THING, Target, TextStyle, Thing, Writing, Writings, face_of, heights, lay, row_of};
use crate::slots::Slots;
use crate::text::{Decoration, Face, Font, TextSystem};
use crate::text_edit::{step, word, word_edge};
use crate::theme::Theme;
use crate::ui::{LINE, Measure, Response, Stored, Stroke, Ui};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Whether readers here read a document by all its text, with its selection however far it
/// runs (UI Automation, AT-SPI, macOS). Phone readers and the web read each block's own.
pub(crate) const WHOLE_TOLD: bool = cfg!(not(any(target_os = "android", target_os = "ios", target_family = "wasm")));
/// What gives documents' nodes their whole text (see `WHOLE_TOLD`), as nodes are built: set as
/// the first document is told, so a program with none carries no code for it.
pub(crate) static WHOLE: OnceLock<fn(&Writings, &mut [AccessNode])> = OnceLock::new();

/// Gives each document's node its whole text and its lines, kept once for its parse and shared
/// (see `WHOLE`). They go along with the node as it moves (`accessibility::follow_letters`).
fn tell_whole(writings: &Writings, nodes: &mut [AccessNode]) {
    let Some(kept) = writings.kept.as_ref().and_then(|kept| kept.downcast_ref::<crate::rich_text::Kept>()) else { return };
    for node in nodes.iter_mut().filter(|node| node.role == Role::Document) {
        let told = kept.documents.get(&node.id).and_then(|shown| shown.told.clone());
        (node.letters, node.text_box, node.text_rect) = (told, node.at, node.rect);
    }
}

/// A document's text as its node tells it whole: all of it, and where its lines start (after
/// each line break), in characters.
fn whole_letters(doc: &Markdown) -> Letters {
    let text = doc.writing.words.clone();
    let breaks = text.chars().enumerate().filter(|(_, letter)| *letter == '\n').map(|(at, _)| at as u32 + 1);
    let lines = std::iter::once(0).chain(breaks).collect();
    Letters { key: doc.serial, lines, places: Vec::new(), text }
}

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
    /// What is laid out: from block `first` (the containers around it opened), to before the
    /// next block to lay out; the containers open there, the page first. (Above and below, the
    /// height is estimated.)
    first: usize,
    next: usize,
    open: Vec<Open>,
    fonts: Vec<(Font, [f32; 2])>,
    /// The top and the bottom of what is laid out.
    top: f32,
    bottom: f32,
    /// The view's height when last laid out for: how far around it to lay out.
    screen: f32,
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
    /// Its text as told to readers that read a document whole (see `WHOLE_TOLD`), once one
    /// listens; and where each end of the selection was last counted in characters (a byte and
    /// the characters before it), to count on from there.
    told: Option<Arc<Letters>>,
    counted: [[u32; 2]; 2],
    /// The link with keyboard focus and its box, and the link its context menu is for (0 for
    /// none).
    focused: u32,
    focused_box: Id,
    menu_link: u32,
    /// Each code block described last run, and how far it was scrolled sideways.
    code_scrolls: Vec<(u32, f32)>,
    /// When a selection drag last scrolled the view (ms, never 0).
    dragged_at: u16,
    /// Described this run.
    pub used: bool,
}

/// A block's layout: its pieces (once measured), its place from the document's top left, and
/// its rows; its children (a range of `Shown::kids`), and whether it is in a quote. For an
/// item: its first row's baseline offset (where its marker goes), and its index and count in
/// its list. For a list: its marker column's width, and its nesting depth. For a table row: its
/// index among the body rows (the header row is none of them), and their count. A list or quote
/// reading right to left has its marks at the right; a code block reading so, its rows.
#[derive(Clone, Copy, Default)]
struct Area {
    bits: [u32; 2],
    measured: bool,
    rect: Rect,
    rows: [u32; 2],
    kids: [u32; 2],
    quoted: bool,
    leftward: bool,
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

/// Whether a list or quote reads right to left, as a browser has it for `dir="auto"`: whether
/// its first strong letter does.
pub(crate) fn leftward(writing: &Writing, at: usize) -> bool {
    let [from, to] = writing.blocks[at].words;
    writing.words[from as usize..to as usize].split('\n').find_map(crate::bidi::strong_direction) == Some(true)
}

/// Whether a code block reads right to left, as GitHub shows it: one naming its language as
/// `dir="auto"` has it (its first strong letter decides, for all its lines); one naming none,
/// never.
pub(crate) fn code_leftward(writing: &Writing, at: usize) -> bool {
    matches!(writing.blocks[at].kind, Kind::Code(language) if language != 0) && leftward(writing, at)
}

/// Where a row's content lies from its left edge: its first fragment's left and its last one's
/// right.
fn row_span(laid: &Laid, row: &crate::rich_text::Row) -> [f32; 2] {
    let frags = &laid.frags[row.frags[0] as usize..row.frags[1] as usize];
    let [left, right] = frags.iter().fold([f32::MAX, 0.0f32], |[left, right], frag| [left.min(frag.x), right.max(frag.x + frag.wide)]);
    [left.min(right), right]
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

/// The table block `at` is, or is in.
fn table_around(writing: &Writing, at: usize) -> Option<usize> {
    let mut up = at;
    while up != 0 && up < writing.blocks.len() {
        if writing.blocks[up].kind == Kind::Table {
            return Some(up);
        }
        up = writing.blocks[up].parent as usize;
    }
    None
}

/// Each link's text, by number from 1: from its first piece's start to its last's end. (A link
/// with no text is empty where the one before it ends: the starts stay in order.)
fn link_words(writing: &Writing, count: usize) -> Vec<[u32; 2]> {
    let mut links = vec![[0; 2]; count];
    for stretch in &writing.pieces {
        if let Some(words) = links.get_mut(stretch.look.link.wrapping_sub(1) as usize) {
            *words = if words[1] == 0 { stretch.words } else { [words[0], stretch.words[1]] };
        }
    }
    for at in 1..links.len() {
        if links[at][1] == 0 {
            links[at] = [links[at - 1][1]; 2];
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
            (self.told, self.counted) = (None, [[0; 2]; 2]);
        }
        (self.laid, self.areas, self.links) = (Laid::default(), vec![Area::default(); writing.blocks.len()], link_words(writing, doc.links.len()));
        self.kids = children(writing, &mut self.areas);
        self.sample = [0.0; 2];
        self.fonts.clear();
        self.title = writing.blocks.iter().find(|block| matches!(block.kind, Kind::Heading(_))).map_or([0; 2], |block| block.words);
    }

    /// Starts laying out again, in a document `width` wide, from the top.
    fn start_layout(&mut self, width: f32, t: &Theme) {
        (self.width, self.long) = (width, 0.0);
        if let Some(page) = self.areas.first_mut() {
            page.rect = rect(0.0, 0.0, width, 0.0);
        }
        self.restart(1, 0.0, sizes(t).gap);
    }

    /// Drops what is laid out: laying out starts again at block `at`, `y` down, inside the page
    /// alone (whose blocks are `gap` apart).
    fn restart(&mut self, at: usize, y: f32, gap: f32) {
        (self.first, self.next, self.top, self.bottom) = (at, at, y, y);
        (self.laid.rows, self.laid.frags, self.targets) = (Vec::new(), Vec::new(), Vec::new());
        self.open = vec![Open { block: 0, x: 0.0, w: self.width, top: 0.0, pen: y, gap, count: 0, lists: 0 }];
    }

    /// Measures block `at`'s text, once: its pieces, and where each character ends.
    fn measure(&mut self, doc: &Markdown, at: usize, given: &mut Given) {
        let (writing, block) = (&doc.writing, doc.writing.blocks[at]);
        if self.areas[at].measured || !worded(block.kind) {
            return;
        }
        self.areas[at].measured = true;
        let first = self.laid.bits.len();
        match block.kind {
            Kind::Code(language) => self.code_bits(doc, at, language, given.t),
            _ => self.word_bits(doc, at, given),
        }
        let wide = self.laid.measure(first..self.laid.bits.len(), &writing.words, given.system, given.scale, &mut self.fonts);
        self.areas[at].bits = [first as u32, self.laid.bits.len() as u32];
        self.sample = [self.sample[0] + wide, self.sample[1] + (block.words[1] - block.words[0]) as f32];
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
        self.high = self.areas[0].rect.h.max(self.bottom) + self.estimate(doc, &t, self.next, count);
    }

    /// Lays out afresh from block `at` (from its table, in one: a table is laid out whole), where
    /// its height is estimated to be.
    fn lay_from(&mut self, doc: &Markdown, given: &mut Given, at: usize) {
        let at = table_around(&doc.writing, at).unwrap_or(at);
        let y = if at <= 1 { 0.0 } else { self.estimated_y(doc, given.t, at) };
        let size = sizes(given.t);
        Laying { shown: self, doc, writing: &doc.writing, size, given }.start_at(at, y);
    }

    /// Lays out through block `at`: on from what is laid out where `at` is near (within two
    /// screens below it), else afresh from `at`.
    fn lay_to(&mut self, doc: &Markdown, given: &mut Given, at: usize) {
        let blocks = &doc.writing.blocks;
        if at >= self.first && at < self.next || at < self.first && blocks[at].end as usize > self.first {
            return;
        }
        let near = at >= self.next && self.estimated_y(doc, given.t, at) - self.bottom <= 2.0 * self.screen;
        if !near {
            self.lay_from(doc, given, at);
        }
        self.lay_more(doc, given, 0.0, at);
    }

    /// Lays out what `view` shows and a screen above and below it, through the end where the
    /// view is at it (so the end it stays at is found at once): on from what is laid out, or
    /// afresh where the view is far from it. Returns how far the view must scroll to keep what
    /// it shows in place (from laying out afresh above it).
    fn cover(&mut self, doc: &Markdown, given: &mut Given, view: [f32; 2]) -> f32 {
        let (t, count) = (given.t, doc.writing.blocks.len());
        self.screen = (view[1] - view[0]).max(1.0);
        let [top, bottom] = [view[0] - self.screen, view[1] + self.screen];
        let far_below = self.next < count && view[0] > self.bottom + 2.0 * self.screen;
        let end = if self.first < self.next && view[1] >= self.high - 0.5 { count - 1 } else { 0 };
        let mut moved = 0.0;
        if far_below || view[1] < self.top {
            let at = self.block_at(doc, t, top);
            self.lay_from(doc, given, at);
        } else if top < self.top && self.first > 1 {
            // (Laid out afresh from further up, by whatever the estimate above was off: the
            // first block that showed stays where it showed, and the view moves with it.)
            let anchor = self.anchor(&doc.writing, view);
            let was = anchor.map_or(0.0, |at| self.areas[at].rect.y);
            let at = self.block_at(doc, t, top - self.screen);
            self.lay_from(doc, given, at);
            self.lay_more(doc, given, 0.0, anchor.unwrap_or(0));
            moved = anchor.map_or(0.0, |at| self.areas[at].rect.y - was);
        }
        self.lay_more(doc, given, bottom + moved, end);
        moved
    }

    /// The first block in view that holds no others (its place is its own, not where laying
    /// out started).
    fn anchor(&self, writing: &Writing, view: [f32; 2]) -> Option<usize> {
        let order = self.in_view(writing, view);
        order.into_iter().map(|at| at as usize).find(|at| self.areas[*at].kids[0] == self.areas[*at].kids[1])
    }

    /// The block not laid out that starts at or above `y`, nearest it: above what is laid out,
    /// or below it.
    fn block_at(&self, doc: &Markdown, t: &Theme, y: f32) -> usize {
        let count = doc.writing.blocks.len();
        let (mut low, mut high) = if y < self.top { (1, self.first) } else { (self.next, count) };
        let lowest = low;
        while low < high {
            let middle = (low + high) / 2;
            match self.estimated_y(doc, t, middle) <= y {
                true => low = middle + 1,
                false => high = middle,
            }
        }
        low.saturating_sub(1).max(lowest).min(count - 1)
    }

    /// Where a block not laid out is estimated to start: what is laid out, less or plus the
    /// estimated height between.
    fn estimated_y(&self, doc: &Markdown, t: &Theme, at: usize) -> f32 {
        match at < self.first {
            true => (self.top - self.estimate(doc, t, at, self.first)).max(0.0),
            false => self.bottom + self.estimate(doc, t, self.next, at),
        }
    }

    /// The height of blocks `from` to before `to`, not laid out: their text at the measured
    /// text's width a byte, in rows of body text, and a gap between every two.
    fn estimate(&self, doc: &Markdown, t: &Theme, from: usize, to: usize) -> f32 {
        let blocks = to.saturating_sub(from) as f32;
        if blocks == 0.0 {
            return 0.0;
        }
        let start = |at: usize| doc.writing.blocks.get(at).map_or(doc.writing.words.len(), |block| block.words[0] as usize) as f32;
        let per_byte = if self.sample[1] > 0.0 { self.sample[0] / self.sample[1] } else { t.font.size * 0.5 };
        let rows = (start(to) - start(from)).max(0.0) * per_byte / self.width.max(1.0) + blocks * 0.5;
        (rows * least(Kind::Paragraph, t.font) + blocks * sizes(t).gap / 2.0).round()
    }

    /// Block `at`'s unwrapped text width. (A code block's is its widest row: see `widest_row`.)
    fn long(&self, at: usize) -> f32 {
        let [from, to] = self.areas[at].bits;
        let last = (to > from).then(|| self.laid.bits[to as usize - 1]);
        last.map_or(0.0, |bit| bit.x + bit.wide)
    }

    /// The widest of block `at`'s rows, as laid out.
    fn widest_row(&self, at: usize) -> f32 {
        let [first, end] = self.areas[at].rows.map(|row| row as usize);
        self.laid.rows[first..end].iter().map(|row| row_span(&self.laid, row)).fold(0.0, |widest, [left, right]| widest.max(right - left))
    }

    /// Moves block `at`'s rows to the right of `wide`, or of the widest row where wider.
    fn align_rows_right(&mut self, at: usize, wide: f32) {
        let (wide, [first, end]) = (self.widest_row(at).max(wide), self.areas[at].rows.map(|row| row as usize));
        for row in first..end {
            let shift = ((wide - row_span(&self.laid, &self.laid.rows[row])[1]) * self.scale).round() / self.scale;
            let [from, to] = self.laid.rows[row].frags.map(|frag| frag as usize);
            self.laid.frags[from..to].iter_mut().for_each(|frag| frag.x += shift);
        }
    }

    /// The blocks in view (`view`, from the top), each container before its content, in
    /// order: in each container from the first child reaching into view, to the last starting
    /// in it.
    fn in_view(&self, writing: &Writing, view: [f32; 2]) -> Vec<u32> {
        let (mut order, mut stack) = (Vec::new(), vec![(0, self.first_in_view(writing, 0, view))]);
        while let Some((up, next)) = stack.last_mut() {
            let at = self.kids[*next..self.areas[*up].kids[1] as usize].first().map_or(usize::MAX, |at| *at as usize);
            if at >= self.next || self.areas[at].rect.y > view[1] {
                stack.pop();
                continue;
            }
            *next += 1;
            order.push(at as u32);
            if self.areas[at].kids[0] < self.areas[at].kids[1] {
                stack.push((at, self.first_in_view(writing, at, view)));
            }
        }
        order
    }

    /// Container `up`'s first laid out child that reaches into view, as an index of `kids`.
    /// (Children ending before what is laid out are above it.)
    fn first_in_view(&self, writing: &Writing, up: usize, view: [f32; 2]) -> usize {
        let [from, to] = self.areas[up].kids.map(|at| at as usize);
        let above = |at: &u32| writing.blocks[*at as usize].end as usize <= self.first || (*at as usize) < self.next && self.areas[*at as usize].rect.bottom() < view[0];
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
    /// before them and the one after, the focused link and those either side of it, and the
    /// first and last links, wherever they are: so Tab and Shift+Tab move on in text order, and
    /// wrap round. Links not laid out are returned apart, in order, those above what is laid out
    /// and those below, each to get a box of its own.
    fn reachable(&self, writing: &Writing, order: &[u32], view: [f32; 2]) -> (Vec<Target>, [Vec<u32>; 2]) {
        let worded = order.iter().map(|at| *at as usize).filter(|at| worded(writing.blocks[*at].kind));
        let [low, high] = worded.map(|at| self.rows_in_view(at, view)).fold([usize::MAX, 0], |[low, high], [from, to]| [low.min(from), high.max(to)]);
        let [from, to] = [low, high].map(|row| self.targets.partition_point(|(at, _)| (*at as usize) < row));
        // (No text in view: none between the links before and after.)
        let to = to.max(from);
        let tab_stop = |at: &usize| self.laid.frags[self.targets[*at].1 as usize].part == 0;
        let (before, after) = ((0..from).rev().find(tab_stop), (to..self.targets.len()).find(tab_stop));
        let mut picked: Vec<usize> = before.into_iter().chain(from..to).chain(after).collect();
        // (Where no link laid out is before or after the view: the links next to what is laid
        // out, `ahead` of it and after the last laid out.)
        let start = writing.blocks.get(self.first).map_or(u32::MAX, |block| block.words[0]);
        let ahead = self.links.partition_point(|words| words[0] < start) as u32;
        let last = self.targets.last().map_or(ahead, |target| link_of(self, target));
        let next_to = [if before.is_none() { ahead } else { 0 }, if after.is_none() { last + 1 } else { 0 }];
        let focus = if self.focused == 0 { [0; 3] } else { [self.focused - 1, self.focused, self.focused + 1] };
        let ends = [1, self.links.len() as u32];
        let mut apart = [Vec::new(), Vec::new()];
        for link in next_to.into_iter().chain(focus).chain(ends).filter(|link| *link != 0 && *link as usize <= self.links.len()) {
            self.pick_link(link, ahead, &mut picked, &mut apart);
        }
        (picked.into_iter().map(|at| self.targets[at]).collect(), apart)
    }

    /// Gives link `link` a box, once, in text order: its first piece among `picked` where it is
    /// laid out; else apart, above what is laid out (where the `ahead` links are) or below.
    fn pick_link(&self, link: u32, ahead: u32, picked: &mut Vec<usize>, apart: &mut [Vec<u32>; 2]) {
        let at = self.targets.partition_point(|target| link_of(self, target) < link);
        if self.targets.get(at).is_some_and(|target| link_of(self, target) == link) {
            if let Err(place) = picked.binary_search(&at) {
                picked.insert(place, at);
            }
            return;
        }
        let side = &mut apart[usize::from(link > ahead)];
        if let Err(place) = side.binary_search(&link) {
            side.insert(place, link);
        }
    }

    /// The text byte nearest a point (from the document's top left): in the nearest text block
    /// laid out (vertically first, then horizontally), on the row level with it; in a code
    /// block, as scrolled sideways.
    fn hit(&self, text: &str, x: f32, y: f32) -> u32 {
        let mut nearest: Option<([f32; 2], usize)> = None;
        for (at, area) in self.areas.iter().enumerate().take(self.next).skip(self.first).filter(|(_, area)| area.rows[0] < area.rows[1]) {
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

    /// Whether a link starts on a row below the focused link (`down`), or above it. A link not
    /// laid out counts as on another row.
    fn link_beyond(&self, down: bool) -> bool {
        let end = if down { self.links.len() as u32 } else { 1 };
        if self.focused == 0 || self.focused == end {
            return false;
        }
        let rows = &self.laid.rows;
        let laid = |at: u32| rows.first().is_some_and(|first| first.words[0] <= at) && rows.last().is_some_and(|last| at < last.words[1]);
        let row = |link: u32| Some(self.links[link as usize - 1][0]).filter(|at| laid(*at)).map(|at| self.row_at(at));
        match (row(self.focused), row(end)) {
            (Some(here), Some(there)) => here != there,
            _ => true,
        }
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

    /// The selection's end `end` (0 its anchor, 1 its cursor) in characters from the start of
    /// `text`: counted on from where it was counted last, so a run costs what it moved.
    fn letters_at(&mut self, text: &str, end: usize) -> u32 {
        let ([byte, letters], to) = (self.counted[end], self.picked[end]);
        let between = |from: u32, to: u32| text.get(from as usize..to as usize).map_or(0, |part| part.chars().count() as u32);
        let now = if to >= byte { letters + between(byte, to) } else { letters.saturating_sub(between(to, byte)) };
        self.counted[end] = [to, now];
        now
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
    /// Starts laying out afresh at block `at` (in no table), `y` down: the containers around it
    /// are opened there, as if laid out down to it.
    fn start_at(&mut self, at: usize, y: f32) {
        let blocks = &self.writing.blocks;
        let mut around = Vec::new();
        let mut up = blocks[at].parent as usize;
        while up != 0 && up < blocks.len() {
            around.push(up);
            up = blocks[up].parent as usize;
        }
        self.shown.restart(at, y, self.size.gap);
        for container in around.into_iter().rev() {
            self.ready_for(container, y);
            self.block(container);
        }
        self.ready_for(at, y);
    }

    /// Readies the open container for its child `at`, `y` down: as many children before it as
    /// it has.
    fn ready_for(&mut self, at: usize, y: f32) {
        let Some(up) = self.shown.open.last_mut() else { return };
        let [from, to] = self.shown.areas[up.block].kids.map(|kid| kid as usize);
        (up.count, up.pen) = (self.shown.kids[from..to].partition_point(|kid| (*kid as usize) < at) as u32, y);
    }

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
        self.shown.areas[at].quoted = self.writing.blocks[up.block].kind == Kind::Quote || self.shown.areas[up.block].quoted;
        match block.kind {
            Kind::Quote => {
                let inset = self.indent(w, 3.0 + size.inset);
                let leftward = self.leftward(at);
                self.open(at, [if leftward { x } else { x + inset }, w - inset, y], size.gap, up.lists);
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

    /// Whether list or quote `at` reads right to left (see `leftward`), noted in its area.
    fn leftward(&mut self, at: usize) -> bool {
        self.shown.areas[at].leftward = leftward(self.writing, at);
        self.shown.areas[at].leftward
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
        let x = if self.leftward(at) { x } else { x + inset };
        self.open(at, [x, w - inset, y], if tight { self.size.close } else { self.size.gap }, lists + 1);
    }

    /// Wraps block `at`'s text into rows at `x`, `y`, in width `w` (code padded and never
    /// wrapped, a level 1 or 2 heading underlined). Returns its height.
    fn leaf(&mut self, at: usize, x: f32, y: f32, w: f32) -> f32 {
        let given = &mut *self.given;
        self.shown.measure(self.doc, at, given);
        let block = self.writing.blocks[at];
        let font = font_of(block.kind, heads(self.writing, at), given.t);
        let [above, below] = heights(given.system, font, given.scale, &mut self.shown.fonts);
        let (pad, side, wide, direction) = match block.kind {
            Kind::Code(_) => ([self.size.pad; 2], Align::Start, NO_WRAP, Some(code_leftward(self.writing, at))),
            Kind::Cell(side) => (self.size.cell, side, w - 2.0 * self.size.cell[0], None),
            _ => ([0.0; 2], Align::Start, w, None),
        };
        let room = Room { above, below, high: least(block.kind, font), left: x + pad[0], wide: wide.max(1.0), side, direction };
        let ([from, to], first) = (self.shown.areas[at].bits, self.shown.laid.rows.len() as u32);
        let high = lay(&mut self.shown.laid, from..to, &self.writing.words, block.words[0], room, y + pad[1], given.scale);
        self.shown.areas[at].rows = [first, self.shown.laid.rows.len() as u32];
        // (Right-to-left code lies at the right of its block, or of its widest row past that.)
        self.shown.areas[at].leftward = direction == Some(true);
        if direction == Some(true) {
            self.shown.align_rows_right(at, w - 2.0 * pad[0]);
        }
        let long = if direction.is_some() { self.shown.widest_row(at) } else { self.shown.long(at) };
        self.shown.long = self.shown.long.max(x + 2.0 * pad[0] + long);
        let under = if matches!(block.kind, Kind::Heading(1 | 2)) { (font.size * 0.3).round() + 1.0 } else { 0.0 };
        high + 2.0 * pad[1] + under
    }

    /// A table at `x`, `y` in width `w`: each column as wide as its widest unwrapped cell, all
    /// narrowed alike if that exceeds the room; each row as high as its tallest cell. Returns
    /// the block after it.
    fn table(&mut self, at: usize, [x, y, w]: [f32; 3]) -> usize {
        let (end, quoted) = (self.writing.blocks[at].end as usize, self.shown.areas[at].quoted);
        for inside in at + 1..end {
            self.shown.areas[inside].quoted = quoted;
            self.shown.measure(self.doc, inside, self.given);
        }
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
        let body = rows.iter().filter(|row| self.writing.blocks[**row].kind == Kind::Row { head: false }).count() as u32;
        let (mut top, mut nth) = (y, 0);
        for row in rows {
            let head = self.writing.blocks[row].kind == Kind::Row { head: true };
            self.shown.areas[row].nth = [if head { u32::MAX } else { nth }, body];
            nth += !head as u32;
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
    /// described, and only what is around it is laid out (a jump to the end lays out the end).
    ///
    /// Select with the pointer (double click: word, triple: block, Shift extends; dragged past
    /// the view's edge, it scrolls) or, once it or a link of it is focused, with the keys
    /// (Shift+arrows, Home/End; Ctrl+Shift for words and the ends; Ctrl+A); Ctrl+C copies. Its
    /// context menu copies, selects all, and on a link copies the link. Tab visits it, each code
    /// block (arrows scroll it), then each link, and wraps round. On a link, Up and Down go to
    /// the link above or below, or with none, scroll a line. A link's title shows while it is
    /// hovered or focused. Screen readers get a document: headings with levels, paragraphs,
    /// lists and items, quotes, code, tables with their headings, links with their titles, and
    /// the selection.
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
        let on_link = shown.focused != 0 && self.focus == shown.focused_box;
        if state.focused || on_link {
            self.document_keys(shown, doc, within, view);
        }
        if on_link {
            self.link_arrows(shown, within);
        }
        // Its height is its blocks', so layout's result is known ahead: its width and its place
        // in the scroll box are read back, and it is described again if either differs.
        self.measures.push(Measure { at, within: at, id, read: [read[0], shown.high, 0.0] });
        self.measures.push(Measure { at, within, id: view_id, read: seen });
        let whole = self.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Px(shown.high)).sense().focusable().open();
        let [from, to] = shown.title;
        self.tell(&whole, Role::Document, None, doc.writing.words.get(from as usize..to as usize).unwrap_or(""));
        if WHOLE_TOLD && self.listening {
            self.tell_whole_document(shown, doc);
        }
        // In a box that sizes to its content, it asks for its widest block.
        self.el().w(Length::Px(shown.long)).show();
        let shape = Shape { laid: &shown.laid, text: &doc.writing.words, links: &shown.links, titles: &doc.links, picked: in_order(shown.picked), keyed: state.focused };
        let mut page = Page { shape, shown, writing: &doc.writing, view, lit: Vec::new(), table: None, code_scrolls: Vec::new() };
        let (links, apart) = self.document_blocks(&mut page);
        self.close();
        let code_scrolls = page.code_scrolls;
        (shown.used, shown.code_scrolls) = (true, code_scrolls);
        self.document_menu(shown, doc, &whole, &links);
        self.follow_focus(shown, doc, links.focused, &apart, within, view);
        let link = doc.links.get(links.pressed.wrapping_sub(1) as usize).map(|(url, _)| url.as_str());
        let heading = link
            .and_then(|url| url.strip_prefix('#'))
            .and_then(|name| doc.writing.blocks.iter().position(|block| matches!(block.kind, Kind::Heading(_)) && slug(&doc.writing.words[block.words[0] as usize..block.words[1] as usize]) == name));
        if let Some(at) = heading {
            self.lay_document_to(shown, doc, at);
            self.scroll_document(within, shown.areas[at].rect.y - view[0]);
        }
        self.keep_writings(kept);
        link.filter(|_| heading.is_none())
    }

    /// Updates a document's cache: started again for another parse, theme or scale, laid out
    /// again for another width (what showed at the view's top stays there); then laid out
    /// around `view`. Scrolled to its end, it stays there as laying out corrects its height.
    fn fit_document(&mut self, shown: &mut Shown, doc: &Markdown, width: f32, view: [f32; 2], within: u32) {
        let (t, scale) = (self.theme, self.scale);
        let restart = shown.serial != doc.serial || shown.theme != Some(t) || shown.scale != scale;
        // (Laid out afresh for the same text, the block at the view's top stays where it shows.)
        let relaid = shown.serial == doc.serial && (restart || shown.width != width);
        let anchor = relaid.then(|| shown.anchor(&doc.writing, view)).flatten().map(|at| (at, shown.areas[at].rect.y - view[0]));
        if restart {
            shown.start(doc, &t, scale);
        }
        if shown.width != width {
            shown.start_layout(width, &t);
        }
        let (high, at_end) = (shown.high, shown.high > view[1] - view[0] && view[1] >= shown.high - 0.5);
        let mut by = 0.0;
        if let Some((at, offset)) = anchor {
            self.lay_document_to(shown, doc, at);
            by = shown.areas[at].rect.y - view[0] - offset;
        }
        by += self.laying(shown, |shown, given| shown.cover(doc, given, [view[0] + by, view[1] + by]));
        if at_end && shown.high != high {
            by = shown.high - high;
        }
        if by != 0.0 {
            self.scroll_document(within, by);
        }
    }

    /// Runs `work` on a document's layout, with what laying out takes from the window.
    fn laying<T>(&mut self, shown: &mut Shown, work: impl FnOnce(&mut Shown, &mut Given) -> T) -> T {
        let t = self.theme;
        let mut given = Given { t: &t, system: self.text.as_mut(), scale: self.scale, pictures: &self.pictures };
        work(shown, &mut given)
    }

    /// Lays out a document through block `at`: on from what is laid out, or afresh where far.
    fn lay_document_to(&mut self, shown: &mut Shown, doc: &Markdown, at: usize) {
        self.laying(shown, |shown, given| shown.lay_to(doc, given, at));
    }

    /// Notes the focused link. One focus just reached through a stand-in box (links `apart`) is
    /// laid out, and scrolled to (in scroll box `within`, now showing `view`).
    fn follow_focus(&mut self, shown: &mut Shown, doc: &Markdown, focused: u32, apart: &[Vec<u32>; 2], within: u32, view: [f32; 2]) {
        let arrived = shown.focused != focused && apart.iter().flatten().any(|link| *link == focused);
        (shown.focused, shown.focused_box) = (focused, if focused == 0 { 0 } else { self.focus });
        let Some(words) = shown.links.get(focused.wrapping_sub(1) as usize).filter(|_| arrived) else { return };
        self.lay_document_to(shown, doc, block_of(&doc.writing, words[0]));
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
        // (With nothing selected, Ctrl+C is the app's.)
        if shown.picked[0] != shown.picked[1] && self.key(Key::Char('c'), Mods::CTRL) {
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
        self.lay_document_to(shown, doc, block_of(&doc.writing, shown.picked[1]));
        if let Some(row) = shown.laid.rows.get(shown.row_at(shown.picked[1])) {
            self.show_document_row(within, view, *row);
        }
    }

    /// Up and Down (Ctrl too) on a focused link with no link that way: the view scrolls a line.
    /// (Arrow navigation would scroll on, pass after pass, looking for one.)
    fn link_arrows(&mut self, shown: &Shown, within: u32) {
        for (key, by) in [(Key::Up, -LINE), (Key::Down, LINE)] {
            if !shown.link_beyond(key == Key::Down) && (self.key(key, Mods::NONE) || self.key(key, Mods::CTRL)) {
                self.scroll_document(within, by);
            }
        }
    }

    /// Tells readers that read a document whole (see `WHOLE_TOLD`) its selection in characters,
    /// however far it runs (right after the document is told); and its text, as its node is
    /// built (see `WHOLE`). A new parse is a new text: the nodes are built anew for it.
    fn tell_whole_document(&mut self, shown: &mut Shown, doc: &Markdown) {
        WHOLE.get_or_init(|| tell_whole);
        shown.told.get_or_insert_with(|| Arc::new(whole_letters(doc)));
        self.layout_key = fold(self.layout_key, doc.serial);
        if shown.picked[0] != shown.picked[1] {
            let caret = [0, 1].map(|end| shown.letters_at(&doc.writing.words, end));
            self.tell_caret(caret);
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
        if let Some((address, _)) = doc.links.get(shown.menu_link.wrapping_sub(1) as usize)
            && self.menu_item(self.edit_words[6]).clicked
        {
            self.copy(address);
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
    /// boxes. Returns what was done to its links, and the links given stand-in boxes, before and
    /// after the rest (0 for none).
    fn document_blocks(&mut self, page: &mut Page) -> (Links, [Vec<u32>; 2]) {
        let (blocks, shown, view, rule) = (&page.writing.blocks, page.shown, page.view, self.theme.line_strong);
        let order = shown.in_view(page.writing, view);
        let (targets, apart) = shown.reachable(page.writing, &order, view);
        let mut links = self.link_states(&page.shape, &targets);
        for link in apart.iter().flatten() {
            links.note(*link, self.link_state(*link, 0));
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
        // (Links not laid out: boxes at the top of the view or of what is laid out for those
        // above, and at the foot for those below, which Tab reaches in text order.)
        let [above, below] = &apart;
        above.iter().for_each(|link| self.stand_in_link(page, *link, view[0].max(shown.top)));
        self.link_boxes(&page.shape, &targets, [0.0; 2]);
        below.iter().for_each(|link| self.stand_in_link(page, *link, (view[1].min(shown.bottom) - 1.0).max(0.0)));
        (links, apart)
    }

    /// A box for link `link`, not laid out: a point at `y` that Tab reaches.
    fn stand_in_link(&mut self, page: &Page, link: u32, y: f32) {
        let Some([from, to]) = page.shown.links.get(link.wrapping_sub(1) as usize).copied() else { return };
        let name = page.writing.words.get(from as usize..to as usize).unwrap_or("");
        self.titled_link_box(&page.shape, link, 0, rect(0.0, y, 1.0, 1.0), name);
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
                let bar = if area.leftward { r.w - 3.0 } else { 0.0 };
                self.el().offset(bar, 0.0).size(3.0, r.h).fill(t.line_strong).radius(1.5).show();
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

    /// An item's marker, before it (left of it, or right in a right-to-left list): a check box
    /// for a task, its number in a numbered list, else a bullet (disc, circle, square, by
    /// depth). None for an item whose start is not laid out.
    fn document_mark(&mut self, page: &Page, at: usize, number: u32, ticked: Option<bool>) {
        if at < page.shown.first {
            return;
        }
        let t = self.theme;
        let list = page.shown.areas[page.writing.blocks[at].parent as usize];
        let ([room, mark], [depth, _]) = ([list.mark, page.shown.areas[at].mark], list.nth);
        let numbered = matches!(page.writing.blocks[page.writing.blocks[at].parent as usize].kind, Kind::List { numbered: true, .. });
        let [above, _] = heights(self.text.as_mut(), t.font, self.scale, &mut Vec::new());
        // The x-height's middle on the marker's row; the marker column's left.
        let middle = mark - (above * 0.35).round();
        let left = if list.leftward { page.shown.areas[at].rect.w } else { -room };
        match (ticked, numbered) {
            (Some(done), _) => self.document_tick(left + (room - 14.0) / 2.0, middle - 7.0, done),
            (None, true) => {
                // (Right to left, the number reads as a browser shows it: its point first.)
                let (words, side, x) = if list.leftward { (format!(".{number}"), Align::Start, left + 6.0) } else { (format!("{number}."), Align::End, left) };
                self.el().text(&words, t.font, t.text_dim).select(None, false).text_align(side).w(Length::Px(room - 6.0)).offset(x, mark - above).show();
                // Screen readers announce position in the list: the number is not told on its
                // own, but as part of the item.
                self.tell_words("", usize::MAX);
            }
            (None, false) => {
                let dot = self.el().offset(left + room / 2.0 - 3.0, middle - 3.0).size(6.0, 6.0).radius(if depth >= 3 { 0.0 } else { 3.0 });
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
        for row in &shown.kids[shown.first_in_view(page.writing, at, page.view).max(from + 1)..to] {
            let top = shown.areas[*row as usize].rect.y;
            if top > page.view[1] {
                break;
            }
            self.el().offset(0.0, top - table.rect.y).size(r.w, 1.0).fill(line).show();
        }
    }

    /// A text block at `r` in its parent's box: told as what it is (a heading with its level, a
    /// paragraph, a header cell, a cell), with its rows in view and the part of the selection in
    /// it.
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
            Kind::Cell(_) => self.tell(&state, Role::Cell, None, words),
            _ => self.tell(&state, Role::Paragraph, None, words),
        }
        self.tell_picked(page, at);
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
        let (wide, box_at) = ((page.shown.widest_row(at) + 2.0 * pad).max(r.w), self.open.last().map_or(0, |open| open.at));
        let across = self.tree.specs[box_at as usize].offset[0];
        // (Right-to-left code wider than its block comes into view scrolled to its right end,
        // where its lines start.)
        let arrives = !page.shown.code_scrolls.iter().any(|(block, _)| *block as usize == at);
        if area.leftward && arrives && wide > r.w && !self.scrolls.contains_key(&state.id) {
            self.scrolls.insert(state.id, [wide - r.w, 0.0]);
            self.again = true;
        }
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
    use crate::testing::Trial;

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
        let mut ui = shown_in_a_window(Box::new(crate::text::BuiltInFont));
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

    /// A window 420 by 300 that writes with `system`.
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
        let (measured, laid, high) = with_shown(&mut ui, |shown| (measured(shown), shown.next, shown.high));
        assert!(measured < 60 && laid < 60, "{measured} measured, {laid} laid out");
        assert!(high > 300_000.0, "{high}");
    }

    /// How many blocks have been measured.
    fn measured(shown: &Shown) -> usize {
        shown.areas.iter().filter(|area| area.measured).count()
    }

    /// Ten thousand short paragraphs, `before` above them and `after` below, in a scroll box
    /// filling a window 420 by 300.
    fn long_document(before: &str, after: &str) -> Trial<Markdown> {
        let lines: String = (0..10_000).map(|n| format!("Line {n} of a long document.\n\n")).collect();
        scrolled_document(&format!("{before}{lines}{after}"))
    }

    /// `text` in a scroll box filling a window 420 by 300.
    fn scrolled_document(text: &str) -> Trial<Markdown> {
        let mut trial = Trial::new(Markdown::parse(text), |doc: &mut Markdown, ui| {
            ui.column().grow().scroll().open();
            ui.markdown(doc);
            ui.close();
        });
        trial.resize(420.0, 300.0, 1.0);
        trial
    }

    // (Where readers read a document whole: see `WHOLE_TOLD`.)
    #[cfg(not(any(target_os = "android", target_os = "ios", target_family = "wasm")))]
    #[test]
    fn a_selection_running_past_the_view_is_told_whole_on_the_document() {
        let text: String = (0..400).map(|n| format!("Paragraph {n} of many.\n\n")).collect();
        let mut trial = scrolled_document(&text);
        trial.click(crate::geom::pt(40.0, 8.0));
        trial.key("Ctrl+Shift+End");
        let words = trial.app.writing.words.clone();
        let document = |trial: &Trial<Markdown>| trial.access_nodes().iter().find(|node| node.role == Role::Document).cloned().unwrap();
        let told = document(&trial);
        // The view is at the end; the selection runs from the first paragraph, which is not
        // described.
        assert!(!trial.has(Role::Paragraph, "Paragraph 0 of many.") && trial.has(Role::Paragraph, "Paragraph 399 of many."));
        let [anchor, cursor] = told.caret.expect("a selection");
        assert!(anchor > 0 && anchor < 12, "{anchor}");
        assert_eq!((told.text(), cursor as usize), (words.as_str(), words.chars().count()));
        // Back up a line: the cursor follows; the text is the same, shared, never copied.
        trial.key("Shift+Up");
        let after = document(&trial);
        assert!(after.caret.is_some_and(|[_, moved]| moved < cursor && moved > cursor - 40), "{:?}", after.caret);
        assert!(after.letters.zip(told.letters).is_some_and(|(now, was)| Arc::ptr_eq(&now, &was)));
    }

    /// Four thousand lines, each with code `high` lines high below it, then `Last words.`: code
    /// is estimated by its bytes as if wrapped, so laid out it comes out far higher.
    fn coded_document(high: usize) -> Trial<Markdown> {
        let code = format!("```\n{}```\n\n", "x\n".repeat(high));
        let text: String = (0..4000).map(|n| format!("Line {n}.\n\n{code}")).collect();
        scrolled_document(&format!("{text}Last words.\n"))
    }

    /// Where paragraph `name` shows: its top in the window.
    fn top_of(trial: &Trial<Markdown>, name: &str) -> f32 {
        trial.find(Role::Paragraph, name).rect.y
    }

    #[test]
    fn a_jump_to_the_end_lays_out_only_the_end() {
        let mut trial = long_document("", "");
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        let (measured, laid) = with_shown(&mut trial.ui, |shown| (measured(shown), shown.next - shown.first));
        assert!(measured < 100 && laid < 100, "{measured} measured, {laid} laid out");
        let last = trial.find(Role::Paragraph, "Line 9999 of a long document.").rect;
        assert!(last.bottom() <= 300.0 && last.bottom() > 250.0, "the end shows at the foot: {last:?}");
    }

    #[test]
    fn scrolled_back_up_from_a_jump_what_shows_stays_in_place() {
        let mut trial = long_document("", "");
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        // Each step up moves every paragraph in view down by the step, though what lies above is
        // laid out afresh time and again, its estimated height corrected.
        for _ in 0..40 {
            let shown = trial.visible_access_nodes().iter().find(|node| node.role == Role::Paragraph && node.rect.y >= 0.0).map(|node| (node.name.clone(), node.rect.y)).expect("a paragraph in view");
            trial.wheel(pt(200.0, 150.0), 0.0, -60.0);
            assert_eq!(top_of(&trial, &shown.0), shown.1 + 60.0, "{} moved by other than the step", shown.0);
        }
        let first = with_shown(&mut trial.ui, |shown| shown.first);
        assert!(first > 9000, "laid out from {first}: only around the view");
        // At the top, the first line is where it belongs.
        trial.wheel(pt(200.0, 150.0), 0.0, -1e9);
        assert_eq!(top_of(&trial, "Line 0 of a long document."), 0.0);
    }

    #[test]
    fn a_link_to_a_far_heading_and_ctrl_shift_end_lay_out_only_around_where_they_go() {
        let after: String = (0..20).map(|n| format!("More words {n}.\n\n")).collect();
        let mut trial = long_document("[To the end](#the-end)\n\n", &format!("## The end\n\n{after}Last words.\n"));
        trial.press(Role::Link, "To the end");
        let heading = trial.find(Role::Heading, "The end").rect;
        assert!(heading.y.abs() < 1.0, "the heading is at the top: {heading:?}");
        assert!(with_shown(&mut trial.ui, measured) < 100);
        // Back at the top, clicked in, Ctrl+Shift+End selects to the end, which shows.
        trial.wheel(pt(200.0, 150.0), 0.0, -1e9);
        trial.click(pt(200.0, 100.0));
        trial.key("Ctrl+Shift+End");
        assert!(trial.find(Role::Paragraph, "Last words.").rect.bottom() <= 300.0);
        assert!(with_shown(&mut trial.ui, measured) < 150);
    }

    #[test]
    fn tab_reaches_a_link_above_what_is_laid_out() {
        let mut trial = long_document("[First link](#a)\n\n", "[Last link](#b)\n");
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        assert!(trial.has(Role::Link, "Last link"));
        // From the document, Tab goes to its first link, at the top: laid out, and shown.
        trial.key("Tab");
        trial.key("Tab");
        let first = trial.find(Role::Link, "First link");
        assert!(first.focused && first.rect.y >= 0.0 && first.rect.y < 300.0, "{first:?}");
        assert!(trial.has(Role::Paragraph, "Line 0 of a long document."));
    }

    #[test]
    fn a_table_at_the_top_is_at_the_top_when_the_selection_jumps_back_to_it() {
        let mut trial = long_document("| Name | Size |\n|---|---|\n| a | 1 |\n\n", "");
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        trial.click(pt(200.0, 150.0));
        trial.key("Ctrl+Shift+Home");
        // Nothing lies above it: scrolled up, its top is the view's.
        trial.wheel(pt(200.0, 150.0), 0.0, -100.0);
        assert_eq!(trial.find(Role::ColumnHeader, "Name").rect.y, 0.0);
    }

    #[test]
    fn tab_and_shift_tab_move_on_from_a_link_far_from_the_view() {
        let text: String = (0..3000).map(|n| if n % 1000 == 500 { format!("Line {n} [link {}](#a).\n\n", n / 1000) } else { format!("Line {n}.\n\n") }).collect();
        let mut trial = scrolled_document(&text);
        trial.key("Tab");
        trial.key("Tab");
        assert!(trial.find(Role::Link, "link 0").focused);
        // The view jumps to the end, far below the focused link: Tab goes on to the next link,
        // laid out and shown; Shift+Tab back.
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        trial.key("Tab");
        let next = trial.find(Role::Link, "link 1");
        assert!(next.focused && next.rect.y >= 0.0 && next.rect.bottom() <= 300.0, "{next:?}");
        assert!(trial.has(Role::Paragraph, "Line 1500 link 1."));
        trial.key("Shift+Tab");
        assert!(trial.find(Role::Link, "link 0").focused);
    }

    #[test]
    fn tab_and_shift_tab_wrap_round_a_documents_links() {
        let text: String = (0..3000).map(|n| if n % 1000 == 500 { format!("Line {n} [link {}](#a).\n\n", n / 1000) } else { format!("Line {n}.\n\n") }).collect();
        let mut trial = scrolled_document(&text);
        // From the document, Shift+Tab wraps to its last link, laid out and shown; Tab back.
        trial.key("Tab");
        trial.key("Shift+Tab");
        let last = trial.find(Role::Link, "link 2");
        assert!(last.focused && last.rect.y >= 0.0 && last.rect.bottom() <= 300.0, "{last:?}");
        trial.key("Tab");
        assert!(trial.find(Role::Document, "").focused);
        // From the document scrolled to its middle, Tab goes to its first link; Down to the next.
        trial.wheel(pt(200.0, 150.0), 0.0, 40_000.0);
        trial.key("Tab");
        assert!(trial.find(Role::Link, "link 0").focused);
        trial.key("Down");
        assert!(trial.find(Role::Link, "link 1").focused);
    }

    #[test]
    fn shift_up_and_down_on_a_focused_link_select_in_one_run() {
        let mut trial = long_document("[First link](#a)\n\n", "");
        trial.key("Tab");
        trial.key("Tab");
        // (Trial fails a key that has not settled after a run.)
        trial.key("Shift+Down");
        trial.key("Shift+Down");
        let picked = with_shown(&mut trial.ui, |shown| shown.picked);
        assert!(picked[1] > picked[0], "{picked:?}");
        trial.key("Shift+Up");
        assert!(with_shown(&mut trial.ui, |shown| shown.picked[1]) < picked[1]);
        assert!(trial.find(Role::Link, "First link").focused);
    }

    #[test]
    fn up_and_down_on_a_link_with_none_that_way_scroll_a_line_in_one_run() {
        let mut trial = long_document("[First link](#a)\n\n", "");
        trial.key("Tab");
        trial.key("Tab");
        let top = top_of(&trial, "Line 0 of a long document.");
        trial.key("Down");
        assert_eq!(top_of(&trial, "Line 0 of a long document."), top - LINE);
        trial.key("Ctrl+Down");
        assert_eq!(top_of(&trial, "Line 0 of a long document."), top - 2.0 * LINE);
        trial.key("Up");
        assert_eq!(top_of(&trial, "Line 0 of a long document."), top - LINE);
        assert!(trial.find(Role::Link, "First link").focused);
    }

    #[test]
    fn a_new_width_keeps_what_shows_at_the_top_there() {
        let mut trial = long_document("", "");
        trial.wheel(pt(200.0, 150.0), 0.0, 150_000.0);
        let shown = trial.visible_access_nodes().iter().find(|node| node.role == Role::Paragraph && node.rect.y >= 0.0).map(|node| (node.name.clone(), node.rect.y)).expect("a paragraph in view");
        trial.resize(300.0, 300.0, 1.0);
        assert!((top_of(&trial, &shown.0) - shown.1).abs() < 1.0, "{} moved", shown.0);
        assert!(with_shown(&mut trial.ui, measured) < 100);
    }

    #[test]
    fn a_jump_to_the_end_finds_it_at_once_however_far_off_the_estimate() {
        // (A trial fails where a run would not settle.)
        let mut trial = coded_document(30);
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        let last = trial.find(Role::Paragraph, "Last words.").rect;
        assert!(last.bottom() <= 300.0 && last.bottom() > 250.0, "the end shows at the foot: {last:?}");
    }

    #[test]
    fn laid_out_afresh_from_further_up_it_reaches_a_screen_below_where_the_view_moves() {
        let mut trial = coded_document(10);
        trial.wheel(pt(200.0, 150.0), 0.0, 1e9);
        let mut kept = trial.ui.take_writings();
        let shown = kept.documents.values_mut().next().expect("a document");
        // A view from the first line laid out, below its code: what is above is laid out.
        let line = (shown.first..shown.next).find(|at| trial.app.writing.blocks[*at].kind == Kind::Paragraph).expect("a line laid out");
        let view = [shown.areas[line].rect.y, shown.areas[line].rect.y + 300.0];
        let moved = trial.ui.laying(shown, |shown, given| shown.cover(&trial.app, given, view));
        assert!(moved > 0.0 && shown.top < view[0] + moved - 300.0, "laid out afresh from further up, by {moved}");
        assert!(shown.bottom >= view[1] + moved + 300.0, "laid out to {}, the view moved to {:?}", shown.bottom, [view[0] + moved, view[1] + moved]);
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

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_right_to_left_list_or_quote_has_its_marks_at_the_right() {
        let doc = Markdown::parse("- \u{5d0}\u{5d2}\n- \u{5d2}\u{5d0}\n\n1. \u{5d0}\n\n> \u{5d2}\u{5d0}\n\n- ab\n");
        let mut ui = shown_in_a_window(Box::new(crate::bidi::tests::hebrew_letters()));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let inset = sizes(&ui.theme).inset;
        with_shown(&mut ui, |shown| {
            let kinds = doc.writing.blocks.iter().map(|block| block.kind);
            let containers: Vec<(Area, Area)> = kinds.enumerate().filter(|(_, kind)| matches!(kind, Kind::List { .. } | Kind::Quote)).map(|(at, _)| (shown.areas[at], shown.areas[at + 1])).collect();
            let sides: Vec<bool> = containers.iter().map(|(up, _)| up.leftward).collect();
            assert_eq!(sides, [true, true, true, false], "Hebrew lists and quote read right to left; the Latin list not");
            for (up, first) in &containers[..2] {
                assert_eq!([first.rect.x, first.rect.w], [up.rect.x, up.rect.w - up.mark], "items start at the list's left, its marks at the right");
            }
            let (quote, inside) = containers[2];
            assert_eq!([inside.rect.x, inside.rect.w], [quote.rect.x, quote.rect.w - 3.0 - inset]);
            let (list, item) = containers[3];
            assert_eq!(item.rect.x, list.rect.x + list.mark);
        });
        assert!(ui.tree.strings.contains(".1"), "a number reads point first, right to left");
    }

    /// The code blocks of a document, by block index.
    fn code_blocks(doc: &Markdown) -> Vec<usize> {
        (0..doc.writing.blocks.len()).filter(|at| matches!(doc.writing.blocks[*at].kind, Kind::Code(_))).collect()
    }

    /// Each row of block `at`: its content's left and right, from the document's left.
    #[cfg(feature = "right-to-left")]
    fn row_ends(shown: &Shown, at: usize) -> Vec<[f32; 2]> {
        let [first, end] = shown.areas[at].rows.map(|row| row as usize);
        let ends = |row: &crate::rich_text::Row| row_span(&shown.laid, row).map(|x| row.left + x);
        shown.laid.rows[first..end].iter().map(ends).collect()
    }

    #[test]
    fn a_code_blocks_width_is_its_widest_line_not_all_its_lines() {
        let text = format!("```\n{}```\n", "abcdefghij\n".repeat(40));
        let doc = Markdown::parse(&text);
        let mut ui = shown_in_a_window(Box::new(crate::text::BuiltInFont));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let line = ui.text.measure("abcdefghij", ui.theme.font.mono(), 1.0, None);
        let (long, widest) = with_shown(&mut ui, |shown| (shown.long, shown.widest_row(code_blocks(&doc)[0])));
        assert_eq!((widest, long), (line, line + 2.0 * sizes(&ui.theme).pad));
        // So it has nothing to scroll sideways.
        let code = ui.tree.specs.iter().position(|spec| spec.frame.scroll[0]).unwrap();
        assert_eq!(ui.tree.scroll_end(code as u32, 0), 0.0);
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn right_to_left_code_naming_its_language_lies_at_the_right_and_naming_none_at_the_left() {
        let (hebrew, latin) = ("\u{5d0}\u{5d2} \u{5d2}\u{5d0}", "ABC");
        // As GitHub has them: a block naming its language reads as its first strong letter does,
        // every line of it; one naming none reads left to right.
        let doc = Markdown::parse(&format!("```rust\n{hebrew}\n{latin}\n```\n\n```\n{hebrew}\n```\n\n```rust\n{latin}\n{hebrew}\n```\n"));
        let mut ui = shown_in_a_window(Box::new(crate::bidi::tests::hebrew_letters()));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let pad = sizes(&ui.theme).pad;
        let blocks = code_blocks(&doc);
        let rows: Vec<Vec<[f32; 2]>> = with_shown(&mut ui, |shown| blocks.iter().map(|at| row_ends(shown, *at)).collect());
        let (right, left) = (420.0 - pad, pad);
        assert!(rows[0].iter().all(|[_, end]| (end - right).abs() < 1.0), "both lines at the right: {:?}", rows[0]);
        assert!(rows[1].iter().all(|[start, _]| (start - left).abs() < 1.0), "at the left: {:?}", rows[1]);
        assert!(rows[2].iter().all(|[start, _]| (start - left).abs() < 1.0), "a Latin first line leads: {:?}", rows[2]);
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn right_to_left_code_wider_than_its_block_comes_scrolled_to_where_its_lines_start() {
        let doc = Markdown::parse(&format!("```rust\n{}\n```\n", "\u{5d0}\u{5d2} ".repeat(60)));
        let mut trial = Trial::new(doc, |doc, ui| _ = ui.markdown(doc));
        trial.run();
        let code = trial.ui.tree.specs.iter().position(|spec| spec.frame.scroll[0]).unwrap() as u32;
        let end = trial.ui.tree.scroll_end(code, 0);
        assert!(end > 0.0 && trial.ui.tree.specs[code as usize].offset[0] == end, "scrolled to {} of {end}", trial.ui.tree.specs[code as usize].offset[0]);
        // Scrolled back by the person, it stays.
        trial.wheel(crate::geom::pt(100.0, 20.0), -1e6, 0.0);
        assert!(trial.ui.tree.specs[code as usize].offset[0] < end);
    }

    #[test]
    fn links_given_boxes_with_no_text_in_view_stay_in_text_order() {
        let doc = Markdown::parse("[one](#a) and [two](#b).\n");
        let mut ui = shown_in_a_window(Box::new(crate::text::BuiltInFont));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let (targets, _) = with_shown(&mut ui, |shown| shown.reachable(&doc.writing, &[], [1000.0, 1300.0]));
        assert!(!targets.is_empty() && targets.is_sorted(), "{targets:?}");
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn right_to_left_code_lies_in_its_block() {
        let doc = Markdown::parse("```\n\u{5d0}\u{5d2} \u{5d2}\u{5d0}\n```\n");
        let mut ui = shown_in_a_window(Box::new(crate::bidi::tests::hebrew_letters()));
        let mut build = |ui: &mut Ui| _ = ui.markdown(&doc);
        ui.run(&mut build);
        ui.run(&mut build);
        let ends = with_shown(&mut ui, |shown| shown.laid.frags.iter().map(|frag| shown.laid.rows[0].left + frag.x + frag.wide).fold(0.0f32, f32::max));
        assert!(ends <= 420.0, "{ends}");
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

/// Lists and code in Hebrew and in Latin, held against Chrome's drawing of the same, in Arial:
/// each list's text and markers lie where Chrome puts them for `dir="auto"` (the marker column
/// as wide as the library's), the markers of a right-to-left list at the right; each code
/// block's lines lie where they do in GitHub's HTML for it (`dir="auto"` where it names its
/// language).
#[cfg(all(test, windows, feature = "right-to-left"))]
mod against_edge {
    use super::*;
    use crate::browser_check::{browser, ink_middle, lasting_differences};
    use crate::font_file::{AppFonts, FontFile};

    /// The text's ink and the markers'.
    const INKS: [u32; 2] = [0x000000, 0xd00000];

    /// Each list as Markdown and as HTML, and whether it reads right to left.
    const CASES: [(&str, &str, bool); 4] = [
        ("- \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}\n", "<ul dir=auto><li>\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}</li></ul>", true),
        ("1. \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}\n", "<ol dir=auto><li>\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}</li></ol>", true),
        ("- Hello world\n", "<ul dir=auto><li>Hello world</li></ul>", false),
        ("1. Hello world\n", "<ol dir=auto><li>Hello world</li></ol>", false),
    ];

    /// Where case `n` is drawn: its left and top.
    fn place(n: usize) -> [f32; 2] {
        [10.0, 10.0 + 50.0 * n as f32]
    }

    /// The cases as a page, in the font at `path`, each list's markers as wide apart from its
    /// text as the library's (`marks`).
    fn page(path: &str, marks: f32) -> String {
        let mut html = format!(
            "<!doctype html><meta charset=utf-8><style>@font-face {{ font-family: Probe; src: url('file:///{path}'); }} body {{ margin: 0; background: #fff; }} div {{ position: absolute; width: 400px; font: 20px Probe; line-height: 30px; color: #000; font-kerning: none; }} ul, ol {{ margin: 0; padding-inline-start: {marks}px; }} li::marker {{ color: #d00000; }}</style>"
        );
        for (n, (_, list, _)) in CASES.iter().enumerate() {
            let [x, y] = place(n);
            html += &format!("<div style='left: {x}px; top: {y}px'>{list}</div>");
        }
        html
    }

    /// Edge or Chrome, Arial's path, and a window of `size` writing in Arial at 20 units: text in
    /// the first ink, dim text in the second, on white. None where either is missing.
    fn in_arial(size: [usize; 2]) -> Option<(std::path::PathBuf, std::path::PathBuf, Ui)> {
        let Some(browser) = browser() else {
            eprintln!("no Edge or Chrome here: not compared");
            return None;
        };
        let path = std::path::Path::new(&std::env::var_os("WINDIR").unwrap_or_default()).join(r"Fonts\arial.ttf");
        let Some(font) = FontFile::open(&path) else {
            eprintln!("no Arial here: not compared");
            return None;
        };
        let mut ui = Ui::new(Box::new(AppFonts::new(font)), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: size[0] as f32, h: size[1] as f32 }, 1.0);
        (ui.theme.bg, ui.theme.text, ui.theme.text_dim, ui.theme.font) = (Color::WHITE, Color::rgb(INKS[0]), Color::rgb(INKS[1]), Font::new(20.0));
        Some((browser, path, ui))
    }

    #[test]
    fn markers_of_right_to_left_lists_lie_at_the_right_as_in_chrome() {
        let size = [440, 220];
        let Some((browser, path, mut ui)) = in_arial(size) else { return };
        let docs: Vec<Markdown> = CASES.iter().map(|(text, ..)| Markdown::parse(text)).collect();
        let mut written = |ui: &mut Ui| {
            ui.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Grow(1.0)).open();
            for (n, doc) in docs.iter().enumerate() {
                let [x, y] = place(n);
                ui.el().offset(x, y).w(Length::Px(400.0)).open();
                ui.markdown(doc);
                ui.close();
            }
            ui.close();
        };
        ui.run(&mut written);
        ui.run(&mut written);
        let ours = crate::surface::Surface::draw(&mut ui, 64).pixels;
        let differ = |theirs: &[u32]| {
            let mut wrong = Vec::new();
            for (n, (text, _, leftward)) in CASES.iter().enumerate() {
                let [x, y] = place(n);
                let band = rect(x - 10.0, y - 5.0, 420.0, 40.0);
                let middles = |pixels: &[u32]| INKS.map(|ink| ink_middle(pixels, size[0], band, ink, &INKS).map(|[x, _]| x));
                let ([words, marker], [edge_words, edge_marker]) = (middles(&ours), middles(theirs));
                let (Some(words), Some(marker), Some(edge_words), Some(edge_marker)) = (words, marker, edge_words, edge_marker) else {
                    wrong.push((format!("{text:?}"), "no text or marker found".into()));
                    continue;
                };
                // (Within 3 for the text; within 8 for a marker: Chrome sets a bullet as a
                // glyph beside the text, the library centres its dot in the marker column.)
                if (marker > words) != *leftward || (words - edge_words).abs() > 3.0 || (marker - edge_marker).abs() > 8.0 {
                    wrong.push((format!("{text:?}"), format!("text at {words}, marker at {marker}; Edge's at {edge_words} and {edge_marker}")));
                }
            }
            wrong
        };
        let html = page(&path.to_string_lossy().replace('\\', "/"), sizes(&ui.theme).marks);
        let wrong = lasting_differences(&browser, &html, size, "list-markers", differ);
        assert!(wrong.is_empty(), "{} lists lie elsewhere than in Chrome", wrong.len());
    }

    /// Code blocks as Markdown and as GitHub writes them in HTML, and whether they read right to
    /// left.
    const CODE: [(&str, &str, bool); 4] = [
        ("```rust\n\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}\n```\n", "<div dir=auto><pre>\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}</pre></div>", true),
        ("```\n\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}\n```\n", "<div><pre>\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}</pre></div>", false),
        ("```rust\n\u{5e9}\u{5dc}\u{5d5}\u{5dd}\nabc xyz\n```\n", "<div dir=auto><pre>\u{5e9}\u{5dc}\u{5d5}\u{5dd}\nabc xyz</pre></div>", true),
        ("```rust\nabc xyz\n\u{5e9}\u{5dc}\u{5d5}\u{5dd}\n```\n", "<div dir=auto><pre>abc xyz\n\u{5e9}\u{5dc}\u{5d5}\u{5dd}</pre></div>", false),
    ];

    /// Where code case `n` is drawn: its left and top.
    fn code_place(n: usize) -> [f32; 2] {
        [10.0, 10.0 + 100.0 * n as f32]
    }

    #[test]
    fn right_to_left_code_lies_where_github_has_it_in_chrome() {
        let size = [440, 420];
        let Some((browser, path, mut ui)) = in_arial(size) else { return };
        // (The code's panel and edge in white: only the letters are ink.)
        (ui.theme.panel, ui.theme.line) = (Color::WHITE, Color::WHITE);
        let docs: Vec<Markdown> = CODE.iter().map(|(text, ..)| Markdown::parse(text)).collect();
        let mut written = |ui: &mut Ui| {
            ui.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Grow(1.0)).open();
            for (n, doc) in docs.iter().enumerate() {
                let [x, y] = code_place(n);
                ui.el().offset(x, y).w(Length::Px(400.0)).open();
                ui.markdown(doc);
                ui.close();
            }
            ui.close();
        };
        ui.run(&mut written);
        ui.run(&mut written);
        let ours = crate::surface::Surface::draw(&mut ui, 64).pixels;
        let (pad, high) = (sizes(&ui.theme).pad, least(Kind::Code(0), ui.theme.font.mono()));
        let differ = |theirs: &[u32]| {
            let mut wrong = Vec::new();
            for (n, (text, _, leftward)) in CODE.iter().enumerate() {
                let [x, y] = code_place(n);
                // Each line's ink, held apart: the first line, then the second.
                for line in 0..2 {
                    let band = rect(x - 5.0, y + pad + high * line as f32, 410.0, high);
                    let middle = |pixels: &[u32]| ink_middle(pixels, size[0], band, INKS[0], &INKS).map(|[x, _]| x);
                    let (here, there) = (middle(&ours), middle(theirs));
                    let side = here.map(|here| here > x + 200.0);
                    let far = here.zip(there).is_some_and(|(here, there)| (here - there).abs() > 3.0);
                    if far || here.is_some() != there.is_some() || line == 0 && side != Some(*leftward) {
                        wrong.push((format!("{text:?} line {line}"), format!("ours at {here:?}, Edge's at {there:?}")));
                    }
                }
            }
            wrong
        };
        let path = path.to_string_lossy().replace('\\', "/");
        let mut html = format!(
            "<!doctype html><meta charset=utf-8><style>@font-face {{ font-family: Probe; src: url('file:///{path}'); }} body {{ margin: 0; background: #fff; }} body > div {{ position: absolute; width: 400px; }} pre {{ margin: 0; padding: {pad}px; font: 20px Probe; line-height: {high}px; color: #000; font-kerning: none; }}</style>"
        );
        for (n, (_, code, _)) in CODE.iter().enumerate() {
            let [x, y] = code_place(n);
            html += &code.replacen("<div", &format!("<div style='left: {x}px; top: {y}px'"), 1);
        }
        let wrong = lasting_differences(&browser, &html, size, "rtl-code", differ);
        assert!(wrong.is_empty(), "{} lines of code lie elsewhere than in Chrome", wrong.len());
    }
}
