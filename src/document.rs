//! A Markdown document drawn ([`Ui::markdown`]): parsed text (`rich::Writing`) laid out as a
//! page, selectable and copyable by pointer and keys, and reported to screen readers as a
//! document.
//!
//! It is laid out as a whole, not box by box. Each text run is measured once (`rich.rs`); then
//! blocks are placed top down and their text wrapped into rows, again only when the width
//! changes. Only blocks in view are described, each a box inside its parent block's box: any
//! length costs a screenful. Boxes nest as blocks do (a list holds its items, an item its
//! content), so screen readers get each block as what it is. Links come last, a box over each,
//! in view or not, so Tab visits all of them in order and scrolls to each.
use crate::code::Rules;
use crate::color::Color;
use crate::edit::{step, word, word_edge};
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::hash;
use crate::layout::{Align, Flow, Length};
use crate::markdown::Markdown;
use crate::rich::{Bit, Kind, Laid, Look, Room, Shape, THING, Target, Thing, Writing, face_of, heights, lay, row_of};
use crate::text::{Face, Font, Rule, TextSystem};
use crate::theme::Theme;
use crate::told::Role;
use crate::ui::{Measure, Response, Stored, Ui};

/// A document as last laid out, and the user's selection: kept by its box's id.
#[derive(Default)]
pub(crate) struct Shown {
    /// What it was measured for: the parse, theme and scale; and the layout width.
    serial: u64,
    theme: Option<Theme>,
    scale: f32,
    width: f32,
    laid: Laid,
    /// Per block: its text pieces, place and rows.
    areas: Vec<Area>,
    /// Each link's text, by number from 1; and every clickable piece of a link, by row and
    /// fragment.
    links: Vec<[u32; 2]>,
    targets: Vec<Target>,
    /// Its height, and its widest block's unwrapped width.
    high: f32,
    long: f32,
    /// The selection: anchor and focus (text bytes); the click count that started it, and the
    /// unit the first click took (a point, a word, a block).
    picked: [u32; 2],
    clicks: u8,
    grabbed: [u32; 2],
    /// Described this run.
    pub used: bool,
}

/// A block's layout: its pieces, its place from the document's top left, and its rows. For an
/// item: its first row's baseline offset (where its marker goes), and its index and count in
/// its list. For a list: its marker column's width, and its nesting depth.
#[derive(Clone, Copy, Default)]
struct Area {
    bits: [u32; 2],
    rect: Rect,
    rows: [u32; 2],
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

/// A block's base font, before inline styles: headings larger and bold, code monospace, header
/// cells bold.
pub(crate) fn font_of(kind: Kind, head: bool, t: &Theme) -> Font {
    match kind {
        Kind::Heading(level) => Font { face: Face::Bold, size: (t.font.size * [2.0, 1.6, 1.3, 1.1, 1.0, 0.9][level.clamp(1, 6) as usize - 1]).round(), rule: Rule::None },
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

/// The highlighting rules for a code language: the app's for that name, else the library's for
/// a common name it knows.
pub(crate) fn rules_for(doc: &Markdown, name: &str) -> Option<&'static Rules> {
    let given = doc.rules.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, rules)| *rules);
    given.or(match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(&Rules::RUST),
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" => Some(&Rules::C),
        "python" | "py" => Some(&Rules::PYTHON),
        "javascript" | "js" | "jsx" | "mjs" | "typescript" | "ts" | "tsx" | "json" => Some(&Rules::JAVASCRIPT),
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

impl Shown {
    /// Measures every block's text again: for another parse, theme or scale.
    fn measure(&mut self, doc: &Markdown, t: &Theme, system: &mut dyn TextSystem, scale: f32, pictures: &crate::shelf::Shelf<Stored>) {
        let writing = &doc.writing;
        (self.theme, self.scale, self.width) = (Some(*t), scale, -1.0);
        if self.serial != doc.serial {
            (self.serial, self.picked, self.clicks) = (doc.serial, [0; 2], 0);
        }
        (self.laid, self.areas, self.links) = (Laid::default(), vec![Area::default(); writing.blocks.len()], vec![[0; 2]; doc.links.len()]);
        let mut fonts = Vec::new();
        for at in 0..writing.blocks.len() {
            let first = self.laid.bits.len();
            match writing.blocks[at].kind {
                Kind::Code(language) => self.code_bits(doc, at, language, t),
                kind if worded(kind) => self.word_bits(doc, at, t, pictures),
                _ => continue,
            }
            self.laid.measure(first..self.laid.bits.len(), &writing.words, system, scale, &mut fonts);
            self.areas[at].bits = [first as u32, self.laid.bits.len() as u32];
        }
    }

    /// A text block's pieces: each run in the block's font plus its inline style, in its colour
    /// (links stand out; quotes are muted), inline code on its own background, and an image
    /// where the app gave one for its name (else its alt text, in italics).
    fn word_bits(&mut self, doc: &Markdown, at: usize, t: &Theme, pictures: &crate::shelf::Shelf<Stored>) {
        let (writing, block) = (&doc.writing, doc.writing.blocks[at]);
        let head = matches!(writing.blocks.get(block.parent as usize).map(|row| row.kind), Some(Kind::Row { head: true }));
        let (base, words) = (font_of(block.kind, head, t), if quoted(writing, at) { t.text_dim } else { t.text });
        for stretch in &writing.pieces[block.pieces[0] as usize..block.pieces[1] as usize] {
            let (look, link) = (stretch.look, stretch.look.link);
            let mut font = Font { face: face_of(base.face, look), rule: look.rule, ..base };
            let (mut color, back) = (if link != 0 { t.accent } else { words }, if look.mono { t.hover } else { Color::NONE });
            if let Some(words) = self.links.get_mut(link.wrapping_sub(1) as usize) {
                *words = if words[1] == 0 { stretch.words } else { [words[0], stretch.words[1]] };
            }
            let given = match stretch.thing {
                Thing::Named(name) => doc.given.get(name as usize).copied().flatten().and_then(|picture| pictures.get(picture.0).map(|stored| (picture, stored))),
                _ => None,
            };
            // An image with no alt text, and none given by the app, takes no room.
            let missing = given.is_none() && matches!(stretch.thing, Thing::Named(_));
            if missing && writing.words[stretch.words[0] as usize..stretch.words[1] as usize].starts_with(THING) {
                continue;
            }
            if missing {
                (font.face, color) = (face_of(font.face, Look { italic: true, ..Look::default() }), t.text_dim);
            }
            let thing = match given {
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

    /// Places every block in a document `width` wide, and wraps each one's text into rows.
    fn lay(&mut self, doc: &Markdown, t: &Theme, width: f32, system: &mut dyn TextSystem, scale: f32) {
        let writing = &doc.writing;
        (self.width, self.long) = (width, 0.0);
        (self.laid.rows, self.laid.frags) = (Vec::new(), Vec::new());
        let open = vec![Open { block: 0, x: 0.0, w: width, top: 0.0, pen: 0.0, gap: sizes(t).gap, count: 0 }];
        let mut laying = Laying { shown: self, writing, t: *t, size: sizes(t), system, scale, fonts: Vec::new(), open };
        laying.shown.areas[0].rect = rect(0.0, 0.0, width, 0.0);
        let mut at = 1;
        while at < writing.blocks.len() {
            laying.close_ended(at);
            at = laying.block(at);
        }
        laying.close_ended(usize::MAX);
        self.high = self.areas[0].rect.h;
        let laid = &self.laid;
        let links = laid.frags.iter().enumerate().filter(|(_, frag)| laid.bits[frag.bit as usize].link != 0);
        self.targets = links.map(|(at, _)| (row_of(laid, at as u32), at as u32)).collect();
    }

    /// The text byte nearest a point (from the document's top left): in the nearest text block
    /// (vertically first, then horizontally), on the row level with it.
    fn hit(&self, text: &str, x: f32, y: f32) -> u32 {
        let mut nearest: Option<([f32; 2], usize)> = None;
        for (at, area) in self.areas.iter().enumerate().filter(|(_, area)| area.rows[0] < area.rows[1]) {
            let r = area.rect;
            let away = [(r.y - y).max(y - r.bottom()).max(0.0), (r.x - x).max(x - r.right()).max(0.0)];
            if nearest.is_none_or(|(least, _)| away < least) {
                nearest = Some((away, at));
            }
        }
        let Some((_, at)) = nearest else { return 0 };
        let [first, end] = self.areas[at].rows;
        let rows = &self.laid.rows[first as usize..end as usize];
        let row = first as usize + rows.partition_point(|row| row.top + row.high <= y).min(rows.len() - 1);
        self.laid.at_x(text, row, x - self.laid.rows[row].left)
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
        let block = writing.blocks.iter().find(|block| worded(block.kind) && block.words[0] <= at && at <= block.words[1]);
        match (self.clicks, block) {
            (2, _) => [edge(false), edge(true)],
            (3, Some(block)) => block.words,
            _ => [at; 2],
        }
    }
}

/// A container block while its content is laid out: its content's x and width, its start y and
/// how far down it has got, the gap between children, and its child count so far.
#[derive(Clone, Copy)]
struct Open {
    block: usize,
    x: f32,
    w: f32,
    top: f32,
    pen: f32,
    gap: f32,
    count: u32,
}

/// What laying out a document passes around.
struct Laying<'a> {
    shown: &'a mut Shown,
    writing: &'a Writing,
    t: Theme,
    size: Sizes,
    system: &'a mut dyn TextSystem,
    scale: f32,
    fonts: Vec<(Font, [f32; 2])>,
    /// Container blocks being laid out, the page first.
    open: Vec<Open>,
}

impl Laying<'_> {
    /// Closes every container that ends before block `at`: it is as high as its content, and
    /// its parent continues below it.
    fn close_ended(&mut self, at: usize) {
        while let Some(done) = self.open.pop_if(|open| self.writing.blocks[open.block].end as usize <= at) {
            // An empty item is still as high as its marker's row.
            let row = if matches!(self.writing.blocks[done.block].kind, Kind::Item { .. }) { least(Kind::Paragraph, self.t.font) } else { 0.0 };
            self.shown.areas[done.block].rect.h = (done.pen - done.top).max(row);
            if self.open.is_empty() {
                return;
            }
            self.ends(done.block);
        }
    }

    /// Block `at` has its height: its parent continues below it. (The first rows of an item's
    /// first block place the item's marker.)
    fn ends(&mut self, at: usize) {
        let r = self.shown.areas[at].rect;
        let Some(up) = self.open.last_mut() else { return };
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
        let Some(&up) = self.open.last() else { return at + 1 };
        let space = match (up.count, block.kind) {
            (0, _) => 0.0,
            (_, Kind::Heading(_)) => up.gap + self.size.gap,
            _ => up.gap,
        };
        let (x, w, y, size) = (up.x, up.w, up.pen + space, self.size);
        if let Some(up) = self.open.last_mut() {
            up.count += 1;
        }
        self.shown.areas[at].rect = rect(x, y, w, 0.0);
        match block.kind {
            Kind::Quote => self.open(at, x + 3.0 + size.inset, w - 3.0 - size.inset, y, size.gap),
            Kind::List { numbered, tight } => self.list(at, numbered, tight, [x, y, w]),
            Kind::Item { .. } => {
                let ([above, below], high) = (heights(self.system, self.t.font, self.scale, &mut self.fonts), least(Kind::Paragraph, self.t.font));
                let list = self.shown.areas[up.block].nth;
                (self.shown.areas[at].mark, self.shown.areas[at].nth) = (((high - above - below) / 2.0 + above).round(), [up.count + 1, list[1]]);
                self.open(at, x, w, y, up.gap);
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

    /// Opens a container block: content at `x`, `w` wide, from `y` down, with `gap` between.
    fn open(&mut self, at: usize, x: f32, w: f32, y: f32, gap: f32) {
        self.open.push(Open { block: at, x, w: w.max(1.0), top: y, pen: y, gap, count: 0 });
    }

    /// Opens a list: items indented by the marker column (for numbers, as wide as the widest),
    /// with little gap in a tight list.
    fn list(&mut self, at: usize, numbered: bool, tight: bool, [x, y, w]: [f32; 3]) {
        let end = self.writing.blocks[at].end as usize;
        let items = self.writing.blocks[at + 1..end].iter().filter(|item| item.parent == at as u32);
        let (count, last) = items.fold((0, 0), |(count, last), item| (count + 1, if let Kind::Item { number, .. } = item.kind { last.max(number) } else { last }));
        let depth = self.open.iter().filter(|open| matches!(self.writing.blocks[open.block].kind, Kind::List { .. })).count() as u32 + 1;
        let marks = match numbered {
            true => (self.system.measure(&format!("{last}."), self.t.font, self.scale, None) + 2.0 * self.size.close + 4.0).ceil().max(self.size.marks),
            false => self.size.marks,
        };
        (self.shown.areas[at].mark, self.shown.areas[at].nth) = (marks, [depth, count]);
        self.open(at, x + marks, w - marks, y, if tight { self.size.close } else { self.size.gap });
    }

    /// Wraps block `at`'s text into rows at `x`, `y`, in width `w` (code padded, a level 1 or 2
    /// heading underlined). Returns its height.
    fn leaf(&mut self, at: usize, x: f32, y: f32, w: f32) -> f32 {
        let block = self.writing.blocks[at];
        let head = matches!(self.writing.blocks.get(block.parent as usize).map(|row| row.kind), Some(Kind::Row { head: true }));
        let font = font_of(block.kind, head, &self.t);
        let [above, below] = heights(self.system, font, self.scale, &mut self.fonts);
        let (pad, side) = match block.kind {
            Kind::Code(_) => ([self.size.pad; 2], Align::Start),
            Kind::Cell(side) => (self.size.cell, side),
            _ => ([0.0; 2], Align::Start),
        };
        let room = Room { above, below, high: least(block.kind, font), left: x + pad[0], wide: (w - 2.0 * pad[0]).max(1.0), side };
        let ([from, to], first) = (self.shown.areas[at].bits, self.shown.laid.rows.len() as u32);
        let high = lay(&mut self.shown.laid, from..to, &self.writing.words, block.words[0], room, y + pad[1], self.scale);
        self.shown.areas[at].rows = [first, self.shown.laid.rows.len() as u32];
        self.shown.long = self.shown.long.max(x + 2.0 * pad[0] + self.long(at));
        let under = if matches!(block.kind, Kind::Heading(1 | 2)) { (font.size * 0.3).round() + 1.0 } else { 0.0 };
        high + 2.0 * pad[1] + under
    }

    /// Block `at`'s unwrapped text width.
    fn long(&self, at: usize) -> f32 {
        let [from, to] = self.shown.areas[at].bits;
        let last = (to > from).then(|| self.shown.laid.bits[to as usize - 1]);
        last.map_or(0.0, |bit| bit.x + bit.wide)
    }

    /// A table at `x`, `y` in width `w`: each column as wide as its widest unwrapped cell, all
    /// narrowed alike if that exceeds the room; each row as high as its tallest cell. Returns
    /// the block after it.
    fn table(&mut self, at: usize, [x, y, w]: [f32; 3]) -> usize {
        let end = self.writing.blocks[at].end as usize;
        let mut wants: Vec<f32> = Vec::new();
        for row in (at + 1..end).filter(|row| self.writing.blocks[*row].parent == at as u32) {
            let cells = (row + 1..end).filter(|cell| self.writing.blocks[*cell].parent == row as u32);
            for (column, cell) in cells.enumerate() {
                let want = self.long(cell) + 2.0 * self.size.cell[0] + 1.0;
                wants.resize(wants.len().max(column + 1), 0.0);
                wants[column] = wants[column].max(want);
            }
        }
        let total: f32 = wants.iter().sum();
        let narrowed = if total > w { w / total } else { 1.0 };
        let widths: Vec<f32> = wants.iter().map(|want| (want * narrowed).floor()).collect();
        let mut top = y;
        for row in (at + 1..end).filter(|row| self.writing.blocks[*row].parent == at as u32) {
            top += self.table_row(row, x, top, &widths);
        }
        self.shown.areas[at].rect = rect(x, y, widths.iter().sum(), top - y);
        self.shown.long = self.shown.long.max(x + total);
        self.ends(at);
        end
    }

    /// A table row at `top`, its cells in columns `widths` wide from `x`. Returns its height.
    fn table_row(&mut self, row: usize, x: f32, top: f32, widths: &[f32]) -> f32 {
        let end = self.writing.blocks[row].end as usize;
        let (mut left, mut high) = (x, 0.0f32);
        for (column, cell) in (row + 1..end).filter(|cell| self.writing.blocks[*cell].parent == row as u32).enumerate() {
            let wide = widths.get(column).copied().unwrap_or(0.0);
            let cell_high = self.leaf(cell, left, top, wide);
            (self.shown.areas[cell].rect, left, high) = (rect(left, top, wide, cell_high), left + wide, high.max(cell_high));
        }
        for area in &mut self.shown.areas[row + 1..end] {
            area.rect.h = high;
        }
        self.shown.areas[row].rect = rect(x, top, left - x, high);
        high
    }
}

/// A document while described: its blocks and text, layout, the part in view (from its top),
/// and the links under the pointer or focus.
struct Page<'a> {
    shape: Shape<'a>,
    shown: &'a Shown,
    writing: &'a Writing,
    view: [f32; 2],
    lit: Vec<u32>,
}

/// Markdown documents.
impl Ui {
    /// Draws a document [`Markdown::read`] parsed: headings, paragraphs, nested lists, quotes,
    /// code highlighted by its language, tables, task check boxes, rules, and images the app
    /// supplies by name ([`Markdown::set_picture`]). As wide as its room, as high as it comes
    /// out; in a scroll box, only what is in view is described.
    ///
    /// Select with the pointer (double click: word, triple: block, Shift extends) or, once
    /// focused, with the keys (Shift+arrows, Home/End; Ctrl+Shift for words and the ends;
    /// Ctrl+A); Ctrl+C copies. Tab visits it, then each link. Screen readers get a document:
    /// headings with levels, paragraphs, lists and items, quotes, code, links.
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
        let mut kept = self.take_writings();
        let shown = kept.documents.entry(id).or_default();
        self.fit_document(shown, doc, read[0]);
        let scrolled = self.tree.specs[within as usize].offset[1];
        let view = [scrolled - seen[2], scrolled - seen[2] + seen[1]];
        let state = self.interact(id);
        let corner = self.document_corner(&state, within, seen[2]);
        self.document_pointer(shown, doc, &state, corner);
        if state.focused {
            self.document_keys(shown, doc, within, view);
        }
        // Its height is its blocks', so layout's result is known ahead: its width and its place
        // in the scroll box are read back, and it is described again if either differs.
        self.measures.push(Measure { at, within: at, id, read: [read[0], shown.high, 0.0] });
        self.measures.push(Measure { at, within, id: view_id, read: seen });
        let whole = self.el().flow(Flow::Layers).w(Length::Grow(1.0)).h(Length::Px(shown.high)).sense().focusable().open();
        let title = doc.writing.blocks.iter().find(|block| matches!(block.kind, Kind::Heading(_))).map_or("", |block| &doc.writing.words[block.words[0] as usize..block.words[1] as usize]);
        self.tell(&whole, Role::Document, None, title);
        // In a box that sizes to its content, it asks for its widest block.
        self.el().w(Length::Px(shown.long)).show();
        let shape = Shape { laid: &shown.laid, text: &doc.writing.words, links: &shown.links, picked: in_order(shown.picked), keyed: state.focused };
        let mut page = Page { shape, shown, writing: &doc.writing, view, lit: Vec::new() };
        let pressed = self.document_blocks(&mut page);
        self.close();
        shown.used = true;
        let heading = |url: &str| {
            url.strip_prefix('#').and_then(|name| {
                doc.writing.blocks.iter().position(|block| matches!(block.kind, Kind::Heading(_)) && slug(&doc.writing.words[block.words[0] as usize..block.words[1] as usize]) == name)
            })
        };
        let link = doc.links.get(pressed.wrapping_sub(1) as usize).map(|(url, _)| url.as_str());
        let followed = link.and_then(heading).map(|at| shown.areas[at].rect.y);
        if let Some(y) = followed {
            self.scroll_document(within, y - view[0]);
        }
        self.keep_writings(kept);
        link.filter(|_| followed.is_none())
    }

    /// Updates a document's cache: measured again for another parse, theme or scale, laid out
    /// again for another width.
    fn fit_document(&mut self, shown: &mut Shown, doc: &Markdown, width: f32) {
        let (t, scale) = (self.theme, self.scale);
        if shown.serial != doc.serial || shown.theme != Some(t) || shown.scale != scale {
            shown.measure(doc, &t, self.text.as_mut(), scale, &self.pictures);
        }
        if shown.width != width {
            shown.lay(doc, &t, width, self.text.as_mut(), scale);
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
        let scroller = self.survey.scrollers.iter().find(|scroller| scroller.id == spec.id && spec.frame.scroll[1]);
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

    /// Keys while a document has focus: Ctrl+A selects all, Ctrl+C copies, and Shift with
    /// arrows or Home/End (with Ctrl: by words, and to the ends) extends the selection,
    /// scrolling to it.
    fn document_keys(&mut self, shown: &mut Shown, doc: &Markdown, within: u32, view: [f32; 2]) {
        let text = &doc.writing.words;
        if self.key(Key::Char('a'), Mods::CTRL) {
            shown.picked = [0, text.trim_end_matches('\n').len() as u32];
        }
        let [from, to] = in_order(shown.picked);
        if from < to && self.key(Key::Char('c'), Mods::CTRL) {
            self.copy(&text[from as usize..to as usize].replace(THING, ""));
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
        // Extended out of view, it scrolls there.
        let Some(row) = shown.laid.rows.get(shown.row_at(shown.picked[1])) else { return };
        let by = (row.top - view[0]).min(0.0) + (row.top + row.high - view[1]).max(0.0);
        if by != 0.0 {
            self.scroll_document(within, by);
        }
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
    /// boxes. Returns the clicked link's number (0 for none).
    fn document_blocks(&mut self, page: &mut Page) -> u32 {
        let (blocks, shown, view, line) = (&page.writing.blocks, page.shown, page.view, self.theme.line_strong);
        let in_view = |row: u32| shown.laid.rows.get(row as usize).is_some_and(|row| row.top <= view[1] && row.top + row.high >= view[0]);
        let targets: Vec<Target> = shown.targets.iter().copied().filter(|(row, frag)| shown.laid.frags[*frag as usize].part == 0 || in_view(*row)).collect();
        let (lit, pressed) = self.link_states(&page.shape, &targets);
        page.lit = lit;
        // The containers entered, innermost last (the page is the document's own box).
        let (mut open, mut at) = (vec![0], 1);
        while at < blocks.len() {
            while open.len() > 1 && open.last().is_some_and(|open| blocks[*open].end as usize <= at) {
                self.close();
                open.pop();
            }
            let (area, up) = (shown.areas[at], shown.areas[open.last().copied().unwrap_or(0)].rect);
            if area.rect.y > view[1] || area.rect.bottom() < view[0] {
                at = blocks[at].end as usize;
                continue;
            }
            let r = area.rect.moved(-up.x, -up.y);
            match blocks[at].kind {
                Kind::Rule => _ = self.el().at(r.x, r.y).size(r.w, 1.0).fill(line).show(),
                kind if worded(kind) => self.document_leaf(page, at, r),
                _ => self.document_box(page, at, r, &mut open),
            }
            at += 1;
        }
        for _ in 1..open.len() {
            self.close();
        }
        self.link_boxes(&page.shape, &targets, [0.0; 2]);
        pressed
    }

    /// A container block at `r` in its parent's box: reported as what it is, with its marks (a
    /// quote's bar, an item's marker, a table's lines). Left open, and pushed onto `open`.
    fn document_box(&mut self, page: &Page, at: usize, r: Rect, open: &mut Vec<usize>) {
        open.push(at);
        let (t, kind) = (self.theme, page.writing.blocks[at].kind);
        let (fill, line) = match kind {
            Kind::Table => (Color::NONE, 1.0),
            Kind::Row { head: true } => (t.hover, 0.0),
            _ => (Color::NONE, 0.0),
        };
        let state = self.el().flow(Flow::Layers).at(r.x, r.y).size(r.w, r.h).fill(fill).border(t.line, line).open();
        match kind {
            Kind::Quote => {
                self.tell(&state, Role::Quote, None, "");
                self.el().size(3.0, r.h).fill(t.line_strong).radius(1.5).show();
            }
            Kind::List { .. } => self.tell(&state, Role::List, None, ""),
            Kind::Item { number, ticked } => {
                self.tell(&state, Role::Item, ticked, "");
                let nth = page.shown.areas[at].nth;
                self.tell_place(nth[0] as usize, nth[1] as usize);
                self.document_mark(page, at, number, ticked);
            }
            Kind::Row { .. } => self.document_row_lines(page, at, r),
            _ => {}
        }
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
                self.el().text(&format!("{number}."), t.font, t.text_dim).select(None, false).text_align(Align::End).w(Length::Px(room - 6.0)).at(-room, mark - above).show();
                // Screen readers announce position in the list: the number is not reported on
                // its own, but as part of the item.
                self.tell_words("", usize::MAX);
            }
            (None, false) => {
                let dot = self.el().at(-room / 2.0 - 3.0, middle - 3.0).size(6.0, 6.0).radius(if depth >= 3 { 0.0 } else { 3.0 });
                _ = if depth == 2 { dot.border(t.text_dim, 1.0) } else { dot.fill(t.text_dim) }.show();
            }
        }
    }

    /// A task's check box at `x`, `y` in its item's box: filled, with a tick, when done.
    fn document_tick(&mut self, x: f32, y: f32, done: bool) {
        let t = self.theme;
        let (edge, fill) = if done { (t.accent, t.accent) } else { (t.line_strong, Color::NONE) };
        let tick = self.el().at(x, y).size(14.0, 14.0).radius(3.0).border(edge, 1.0).fill(fill);
        if !done {
            tick.show();
            return;
        }
        tick.flow(Flow::Layers).open();
        self.el().icon(crate::widgets::CHECK, 14.0, t.on_accent).show();
        self.close();
    }

    /// The lines between a table row's cells, and above it unless it is the first.
    fn document_row_lines(&mut self, page: &Page, at: usize, r: Rect) {
        let (line, blocks) = (self.theme.line, &page.writing.blocks);
        if blocks[at - 1].kind != Kind::Table {
            self.el().size(r.w, 1.0).fill(line).show();
        }
        let cells = (at + 1..blocks[at].end as usize).filter(|cell| blocks[*cell].parent == at as u32).skip(1);
        for cell in cells {
            let x = page.shown.areas[cell].rect.x - page.shown.areas[at].rect.x;
            self.el().at(x, 0.0).size(1.0, r.h).fill(line).show();
        }
    }

    /// A text block at `r` in its parent's box: reported as what it is (a heading with its
    /// level, a paragraph, code), with its rows in view.
    fn document_leaf(&mut self, page: &Page, at: usize, r: Rect) {
        let (t, block, area) = (self.theme, page.writing.blocks[at], page.shown.areas[at]);
        let (role, fill, line) = match block.kind {
            Kind::Code(_) => (Role::Code, t.panel, 1.0),
            Kind::Heading(_) => (Role::Title, Color::NONE, 0.0),
            _ => (Role::Paragraph, Color::NONE, 0.0),
        };
        let state = self.el().flow(Flow::Layers).at(r.x, r.y).size(r.w, r.h).fill(fill).border(t.line, line).radius(line * t.radius).open();
        let words = page.writing.words.get(block.words[0] as usize..block.words[1] as usize).unwrap_or("");
        self.tell(&state, role, None, words.trim_end());
        if let Kind::Heading(level) = block.kind {
            self.tell_level(level as usize);
        }
        if matches!(block.kind, Kind::Heading(1 | 2)) {
            self.el().at(0.0, r.h - 1.0).size(r.w, 1.0).fill(t.line).show();
        }
        let rows = &page.shown.laid.rows[area.rows[0] as usize..area.rows[1] as usize];
        let (from, to) = (rows.partition_point(|row| row.top + row.high < page.view[0]), rows.partition_point(|row| row.top <= page.view[1]));
        let first = area.rows[0] as usize;
        self.write_rows(&page.shape, first + from..first + to.max(from), [-area.rect.x, -area.rect.y], &page.lit);
        self.close();
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
        let doc = Markdown::read(&text);
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
        let mut build = |ui: &mut Ui| {
            ui.column().grow().scroll().open();
            ui.markdown(&doc);
            ui.close();
        };
        ui.run(&mut build);
        ui.run(&mut build);
        // A screenful: some fifteen paragraphs of a few boxes each; and a box over each of the
        // 5,000 links, for Tab.
        let boxes = ui.tree.specs.len();
        assert!(boxes < 5000 + 200, "{boxes} boxes");
        // Scrolled to the end, the last paragraphs are described.
        ui.event(crate::event::Event::Move(crate::geom::pt(200.0, 150.0)));
        ui.event(crate::event::Event::Wheel(0.0, 1e9));
        ui.run(&mut build);
        ui.run(&mut build);
        let last = ui.tree.strings.contains("Paragraph 4999");
        assert!(last && !ui.tree.strings.contains("Paragraph 0,"));
    }

    #[test]
    fn a_heading_is_known_by_its_words_as_github_writes_them() {
        assert_eq!(slug("What is in it"), "what-is-in-it");
        assert_eq!(slug(" Beside Dear ImGui! "), "beside-dear-imgui");
        assert_eq!(slug("C++ and snake_case"), "c-and-snake_case");
    }

    #[test]
    fn code_is_coloured_by_the_rules_of_the_language_it_names() {
        let mut doc = Markdown::read("```rs\nfn x() {}\n```\n\n```toml\na = 1\n```\n");
        assert_eq!(doc.writing.languages, ["rs", "toml"]);
        assert_eq!(rules_for(&doc, "rs"), Some(&Rules::RUST));
        assert_eq!(rules_for(&doc, "toml"), None);
        doc.set_rules("TOML", &Rules::PYTHON);
        assert_eq!(rules_for(&doc, "toml"), Some(&Rules::PYTHON));
    }
}
