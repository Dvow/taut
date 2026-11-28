//! A document laid out on pages: what [`Markdown::parse`] read, as a window shows it
//! ([`Ui::markdown`](crate::Ui::markdown), `markdown_view.rs`), cut into pages to print or save
//! as PDF.
//!
//! Blocks are laid out as in a window (same sizes, fonts and line breaking, `rich_text.rs`),
//! measured with the fonts the paper writes, so what is measured is what is written. Pages
//! break between blocks or between rows, never inside a row; a heading moves to the next page
//! with what follows rather than end a page; a quote's bar, code's background and a table's
//! lines continue onto the next page. Links lead where they did in the window (a link to one of
//! the document's headings, to its place in the pages); pictures are those the app gave the
//! paper by name ([`Paper::set_picture`]).
//!
//! Each block is a part of the pages' structure (`pdf_structure.rs`), as a window tells it to
//! screen readers: headings (also bookmarks), paragraphs, lists of items (a number as their
//! label), quotes, code, tables with their column headings, pictures described by their words,
//! and links. Backgrounds, rules, bullets and a table's headings repeated on a later page are
//! decoration.
use crate::color::Color;
use crate::geom::{Rect, Size, rect};
use crate::layout::Align;
use crate::markdown::Markdown;
use crate::markdown_view::{Sizes, code_leftward, font_of, least, leftward, quoted, rules_for, sizes, slug, worded};
use crate::paper::{Mark, Paper};
use crate::pdf_structure::Tag;
use crate::rich_text::{Bit, Kind, Laid, Room, THING, TextStyle, Thing, face_of, heights, lay};
use crate::text::{Font, Metrics, TextSystem};
use crate::theme::Theme;
use crate::ui::Picture;

/// Measures a document's words with the fonts the paper writes them in, so rows break where the
/// written words need.
struct Measured<'a>(&'a mut Paper);

impl TextSystem for Measured<'_> {
    fn metrics(&mut self, font: Font, _scale: f32) -> Metrics {
        self.0.metrics(font)
    }
    fn measure(&mut self, text: &str, font: Font, _scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        match stops {
            Some(stops) => self.0.stops(text, font, stops),
            None => self.0.measure(text, font),
        }
    }
    fn draw(&mut self, _canvas: &mut crate::canvas::Canvas, _text: &str, _x: f32, _y: f32, _font: Font, _color: Color) {}
}

/// An open container block while its content is laid out: its content's x and width, the gap
/// between children, how many it has had; and where it began (page and y), where a quote's bar
/// starts. For a list: the width of its item marks, and its nesting depth. How many structure
/// parts it opened (ended with it); for a numbered item, its label part. A list or quote (and
/// an item of a list) reading right to left has its marks at the right.
#[derive(Clone, Copy)]
struct Open {
    block: usize,
    x: f32,
    w: f32,
    gap: f32,
    count: u32,
    top: (usize, f32),
    marks: f32,
    depth: u32,
    parts: u32,
    label: Option<u32>,
    leftward: bool,
}

/// A document being laid out on pages: the paper, the document, its theme and sizes, the page
/// size and margin; the current block's words, laid out; the open container blocks, outermost
/// first; the current page and y; and an item whose mark still waits for its first row.
struct Pages<'a> {
    paper: &'a mut Paper,
    doc: &'a Markdown,
    t: Theme,
    size: Sizes,
    sheet: Size,
    margin: f32,
    laid: Laid,
    fonts: Vec<(Font, [f32; 2])>,
    open: Vec<Open>,
    page: usize,
    y: f32,
    mark: Option<usize>,
}

impl Paper {
    /// Lays a document out on pages of `size`, `margin` units in from every edge, as a window
    /// shows it in the light theme: headings, paragraphs, lists, quotes, highlighted code,
    /// tables, tasks, rules, links (leading where they do in the window), and pictures given by
    /// name ([`set_picture`](Self::set_picture)). Pages break between blocks or rows, never
    /// inside a row. Starts on a new page; gives back how many pages it took.
    pub fn markdown(&mut self, doc: &Markdown, size: Size, margin: f32) -> usize {
        let (first, size) = (self.pages(), Paper::sheet(size));
        self.page(size);
        let t = Theme::light();
        let margin = margin.max(0.0).min(size.w.min(size.h) / 3.0);
        let whole = Open { block: 0, x: margin, w: (size.w - 2.0 * margin).max(1.0), gap: sizes(&t).gap, count: 0, top: (first, margin), marks: 0.0, depth: 0, parts: 0, label: None, leftward: false };
        let mut pages = Pages { size: sizes(&t), t, doc, sheet: size, margin, laid: Laid::default(), fonts: Vec::new(), open: vec![whole], page: first, y: margin, mark: None, paper: self };
        let mut at = 1;
        while at < doc.writing.blocks.len() {
            pages.close_ended(at);
            at = pages.block(at);
        }
        pages.close_ended(usize::MAX);
        self.pages() - first
    }
}

impl Pages<'_> {
    /// How far down a page the document may come.
    fn bottom(&self) -> f32 {
        self.sheet.h - self.margin
    }

    /// Goes on to the next page (starting it if needed): an open quote's bar runs to this
    /// page's foot and continues from the next page's top.
    fn next_page(&mut self) {
        for at in 0..self.open.len() {
            if self.doc.writing.blocks[self.open[at].block].kind == Kind::Quote {
                self.quote_line(at, self.bottom());
                self.open[at].top = (self.page + 1, self.margin);
            }
        }
        self.page += 1;
        if self.page >= self.paper.pages() {
            self.paper.page(self.sheet);
        }
        self.paper.set_page(self.page);
        self.y = self.margin;
    }

    /// Makes room for something `high` units high: on the next page if it does not fit here
    /// (unless this page is still empty).
    fn room_for(&mut self, high: f32) {
        if self.y + high > self.bottom() && self.y > self.margin {
            self.next_page();
        }
    }

    /// The bar of the quote that is the `at`th open block, from where it began on this page
    /// down to `to`.
    fn quote_line(&mut self, at: usize, to: f32) {
        let (open, color) = (self.open[at], self.t.line_strong);
        let from = if open.top.0 == self.page { open.top.1 } else { self.margin };
        if to > from {
            self.paper.set_page(self.page);
            let x = if open.leftward { open.x + open.w + self.size.inset } else { open.x - 3.0 - self.size.inset };
            self.paper.rounded(rect(x, from, 3.0, to - from), 1.5, color);
        }
    }

    /// Finishes every container that ends before block `at`: a quote gets its bar, an empty
    /// item its mark.
    fn close_ended(&mut self, at: usize) {
        while self.open.len() > 1 && self.open.last().is_some_and(|open| self.doc.writing.blocks[open.block].end as usize <= at) {
            let done = self.open.len() - 1;
            match self.doc.writing.blocks[self.open[done].block].kind {
                Kind::Quote => self.quote_line(done, self.y),
                Kind::Item { .. } if self.mark == Some(self.open[done].block) => self.empty_item(done),
                _ => {}
            }
            (0..self.open[done].parts).for_each(|_| self.paper.end());
            self.open.pop();
        }
    }

    /// An empty item, the `at`th open block: as high as a row, with its mark by it.
    fn empty_item(&mut self, at: usize) {
        let high = least(Kind::Paragraph, self.t.font);
        self.room_for(high);
        let [above, below] = heights(&mut Measured(self.paper), self.t.font, 1.0, &mut self.fonts);
        let base = self.y + ((high - above - below) / 2.0 + above).round();
        self.item_mark(at, base);
        self.y += high;
    }

    /// Lays out block `at`, in the block opened last; gives back the next block.
    fn block(&mut self, at: usize) -> usize {
        let block = self.doc.writing.blocks[at];
        let Some(&up) = self.open.last() else { return at + 1 };
        let space = match (up.count, block.kind) {
            (0, _) => 0.0,
            (_, Kind::Heading(_)) => up.gap + self.size.gap,
            _ => up.gap,
        };
        // At the top of a page there is nothing above to keep clear of.
        self.y += if self.y > self.margin { space } else { 0.0 };
        if let Some(up) = self.open.last_mut() {
            up.count += 1;
        }
        let (x, w, size) = (up.x, up.w, self.size);
        match block.kind {
            Kind::Quote => {
                self.paper.begin_tag(Tag::Quote);
                let leftward = leftward(&self.doc.writing, at);
                let x = if leftward { x } else { x + 3.0 + size.inset };
                self.enter(Open { block: at, x, w: w - 3.0 - size.inset, gap: size.gap, count: 0, top: (self.page, self.y), marks: 0.0, depth: up.depth, parts: 1, label: None, leftward });
            }
            Kind::List { numbered, tight } => self.list(at, numbered, tight, x, w),
            Kind::Item { .. } => self.item(at, up, x, w),
            Kind::Table => return self.table(at, x, w),
            Kind::Rule => {
                self.room_for(1.0);
                self.paper.rect(rect(x, self.y, w, 1.0), self.t.line);
                self.y += 1.0;
            }
            kind if worded(kind) => self.leaf(at, x, w),
            _ => {}
        }
        at + 1
    }

    /// Opens a list item, block `at` in `up`: its label (a number) and then its body, as parts.
    fn item(&mut self, at: usize, up: Open, x: f32, w: f32) {
        self.mark = Some(at);
        let numbered = matches!(self.doc.writing.blocks[self.doc.writing.blocks[at].parent as usize].kind, Kind::List { numbered: true, .. });
        self.paper.begin_tag(Tag::Item);
        let label = numbered.then(|| self.paper.begin_tag(Tag::Label));
        self.paper.open.truncate(self.paper.open.len() - label.is_some() as usize);
        self.paper.begin_tag(Tag::Body);
        self.enter(Open { block: at, x, w, gap: up.gap, count: 0, top: (self.page, self.y), marks: up.marks, depth: up.depth, parts: 2, label, leftward: up.leftward });
    }

    /// Opens a container block.
    fn enter(&mut self, open: Open) {
        self.open.push(Open { w: open.w.max(1.0), ..open });
    }

    /// Opens a list: items are indented by their marks' width (for numbers, the widest number),
    /// close together when tight.
    fn list(&mut self, at: usize, numbered: bool, tight: bool, x: f32, w: f32) {
        let blocks = &self.doc.writing.blocks;
        let items = blocks[at + 1..blocks[at].end as usize].iter().filter(|item| item.parent == at as u32);
        let last = items.fold(0, |last, item| if let Kind::Item { number, .. } = item.kind { last.max(number) } else { last });
        let marks = match numbered {
            true => (self.paper.measure(&format!("{last}."), self.t.font) + 2.0 * self.size.close + 4.0).ceil().max(self.size.marks),
            false => self.size.marks,
        };
        let depth = self.open.last().map_or(0, |up| up.depth) + 1;
        let gap = if tight { self.size.close } else { self.size.gap };
        self.paper.begin_tag(Tag::List);
        let leftward = leftward(&self.doc.writing, at);
        let x = if leftward { x } else { x + marks };
        self.enter(Open { block: at, x, w: w - marks, gap, count: 0, top: (self.page, self.y), marks, depth, parts: 1, label: None, leftward });
    }

    /// The bits of a block of words or code, into `laid` (cleared first).
    fn bits(&mut self, at: usize, font: Font) {
        self.laid = Laid::default();
        let (doc, t) = (self.doc, self.t);
        let block = doc.writing.blocks[at];
        if let Kind::Code(language) = block.kind {
            return self.code_bits(at, language, font);
        }
        let words = if quoted(&doc.writing, at) { t.text_dim } else { t.text };
        for stretch in &doc.writing.pieces[block.pieces[0] as usize..block.pieces[1] as usize] {
            let (look, link) = (stretch.look, stretch.look.link);
            let mut font = Font { face: face_of(font.face, look), decoration: look.rule, ..font };
            let (mut color, back) = (if link != 0 { t.accent } else { words }, if look.mono { t.hover } else { Color::NONE });
            // A picture is the one the app gave the paper by name.
            let given = match stretch.thing {
                Thing::Named(name) => doc.names.get(name as usize).and_then(|name| self.paper.named.iter().find(|(known, _)| known == name)).map(|(_, at)| *at),
                _ => None,
            };
            let missing = given.is_none() && matches!(stretch.thing, Thing::Named(_));
            if missing && doc.writing.words[stretch.words[0] as usize..stretch.words[1] as usize].starts_with(THING) {
                continue;
            }
            if missing {
                (font.face, color) = (face_of(font.face, TextStyle { italic: true, ..TextStyle::default() }), t.text_dim);
            }
            let thing = match given.map(|at| (at, self.paper.picture_size(at))) {
                Some((at, size)) => Thing::Picture(Picture(at), size),
                None => Thing::Words,
            };
            self.laid.bits.push(Bit::new(stretch.words, font, color, back, link, thing));
        }
    }

    /// The bits of a code block: for a language with rules, one bit per run of one token kind
    /// per line (and one per line end) in that kind's colour; else all in the text colour. (As
    /// `markdown_view.rs` has them.)
    fn code_bits(&mut self, at: usize, language: u32, font: Font) {
        let (doc, t) = (self.doc, self.t);
        let [start, end] = doc.writing.blocks[at].words;
        let rules = doc.writing.languages.get(language.wrapping_sub(1) as usize).and_then(|name| rules_for(doc, name));
        let Some(rules) = rules else { return self.laid.bits.push(Bit::new([start, end], font, t.text, Color::NONE, 0, Thing::Words)) };
        let (code, mut open, mut runs, mut from) = (&doc.writing.words, 0, Vec::new(), start as usize);
        while from <= end as usize {
            let line = code[from..end as usize].find('\n').map_or(end as usize, |at| from + at);
            runs.clear();
            open = rules.line(&code[from..line], open, &mut runs);
            let mut run = from;
            for (to, sort) in &runs {
                self.laid.bits.push(Bit::new([run as u32, (from + to) as u32], font, sort.color(&t), Color::NONE, 0, Thing::Words));
                run = from + to;
            }
            if line < end as usize {
                self.laid.bits.push(Bit::new([line as u32, line as u32 + 1], font, t.text, Color::NONE, 0, Thing::Words));
            }
            from = line + 1;
        }
    }

    /// Breaks block `at`'s words into rows in room `w` wide (less its padding): the rows go in
    /// `laid`, from its own top. Gives back its font and padding (across, down).
    fn rows(&mut self, at: usize, w: f32) -> (Font, [f32; 2]) {
        let block = self.doc.writing.blocks[at];
        let head = matches!(self.doc.writing.blocks.get(block.parent as usize).map(|row| row.kind), Some(Kind::Row { head: true }));
        let font = font_of(block.kind, head, &self.t);
        self.bits(at, font);
        let [above, below] = heights(&mut Measured(self.paper), font, 1.0, &mut self.fonts);
        let (pad, side, direction) = match block.kind {
            Kind::Code(_) => ([self.size.pad; 2], Align::Start, Some(code_leftward(&self.doc.writing, at))),
            Kind::Cell(side) => (self.size.cell, side, None),
            _ => ([0.0; 2], Align::Start, None),
        };
        let bits = 0..self.laid.bits.len();
        self.laid.measure(bits.clone(), &self.doc.writing.words, &mut Measured(self.paper), 1.0, &mut self.fonts);
        let room = Room { above, below, high: least(block.kind, font), left: pad[0], wide: (w - 2.0 * pad[0]).max(1.0), side, direction };
        lay(&mut self.laid, 0..bits.end as u32, &self.doc.writing.words, block.words[0], room, 0.0, 1.0);
        (font, pad)
    }

    /// A block of words at `x`, `w` wide, from where the document has come to, row by row, each
    /// on the page it fits on. Code has its background and edge on each page it spans; a
    /// heading of level 1 or 2 has a rule under it.
    fn leaf(&mut self, at: usize, x: f32, w: f32) {
        let kind = self.doc.writing.blocks[at].kind;
        let (font, pad) = self.rows(at, w);
        let rows = self.laid.rows.clone();
        let under = if matches!(kind, Kind::Heading(1 | 2)) { (font.size * 0.3).round() + 1.0 } else { 0.0 };
        // A heading moves to the next page where it would end this one.
        let all: f32 = rows.iter().map(|row| row.high).sum::<f32>() + 2.0 * pad[1] + under;
        if matches!(kind, Kind::Heading(_)) && all < (self.bottom() - self.margin) / 3.0 {
            self.room_for(all + 2.0 * least(Kind::Paragraph, self.t.font));
        }
        // Where each row goes: which page, and how far down.
        self.room_for(pad[1] + rows.first().map_or(0.0, |row| row.high));
        let (first, mut placed) = (self.page, Vec::with_capacity(rows.len()));
        self.y += pad[1];
        for row in &rows {
            if self.y + row.high > self.bottom() && self.y > self.margin + pad[1] {
                self.next_page();
                self.y += pad[1];
            }
            placed.push((self.page, self.y));
            self.y += row.high;
        }
        self.y += pad[1];
        if let Kind::Code(_) = kind {
            self.grounds(x, w, pad[1], first, &placed);
        }
        if let Kind::Heading(_) = kind {
            let words = &self.doc.writing.words[self.doc.writing.blocks[at].words[0] as usize..self.doc.writing.blocks[at].words[1] as usize];
            self.paper.set_page(first);
            self.paper.anchor(&slug(words), placed.first().map_or(self.y, |(_, y)| *y));
        }
        self.paper.begin_tag(match kind {
            Kind::Heading(level) => Tag::Heading(level.clamp(1, 6)),
            Kind::Code(_) => Tag::Code,
            _ => Tag::Paragraph,
        });
        for (row, (page, top)) in rows.iter().enumerate().map(|(n, row)| (*row, placed[n])) {
            self.paper.set_page(page);
            self.write_row(&row, x, top);
        }
        self.paper.end();
        if under > 0.0 {
            self.paper.set_page(self.page);
            self.paper.rect(rect(x, self.y + under - 1.0, w, 1.0), self.t.line);
            self.y += under;
        }
        self.paper.set_page(self.page);
    }

    /// Code's background and edge under its rows on each page: from above its first row there
    /// to below its last.
    fn grounds(&mut self, x: f32, w: f32, pad: f32, first: usize, placed: &[(usize, f32)]) {
        let (fill, line, radius) = (self.t.panel, self.t.line, self.t.radius);
        for page in first..=self.page {
            let on: Vec<f32> = placed.iter().filter(|(at, _)| *at == page).map(|(_, y)| *y).collect();
            let (Some(top), Some(_)) = (on.first().copied(), on.last()) else { continue };
            let last = placed.iter().rposition(|(at, _)| *at == page).unwrap_or(0);
            let bottom = placed[last].1 + self.laid.rows[last].high;
            let r = rect(x, top - pad, w, bottom - top + 2.0 * pad);
            self.paper.set_page(page);
            self.paper.rounded(r, radius, fill);
            self.paper.border(r, radius, 1.0, line);
        }
    }

    /// A row of laid-out words, left edge at `x` and top at `top`: each piece's background if
    /// any, then its words or picture, and a link over each link piece. An item's first row
    /// gets the item's mark.
    fn write_row(&mut self, row: &crate::rich_text::Row, x: f32, top: f32) {
        let base = top + row.base;
        if let Some(item) = self.mark.take() {
            let at = self.open.iter().rposition(|open| open.block == item).unwrap_or(0);
            self.item_mark(at, base);
        }
        for frag in &self.laid.frags[row.frags[0] as usize..row.frags[1] as usize] {
            let bit = self.laid.bits[frag.bit as usize];
            let (left, y) = (x + row.left + frag.x, top + frag.y);
            if bit.back != Color::NONE {
                let (before, after) = (if frag.into <= 0.0 { crate::rich_text::PAD } else { 0.0 }, if frag.words[1] >= bit.words[1] { crate::rich_text::PAD } else { 0.0 });
                self.paper.rounded(rect(left - before, y, frag.wide + before + after, bit.above + bit.below), 3.0, bit.back);
            }
            let link = self.doc.links.get(bit.link.wrapping_sub(1) as usize).map(|(to, _)| to.as_str());
            if link.is_some() {
                self.paper.begin_tag(Tag::Link);
            }
            let said = self.doc.writing.words.get(bit.words[0] as usize..bit.words[1] as usize).unwrap_or("");
            match bit.thing {
                Thing::Picture(picture, _) => figure(self.paper, said, picture.0, rect(left, y, frag.wide, frag.high)),
                Thing::Icon(icon) => self.paper.icon(icon, rect(left, y, frag.high, frag.high), bit.color),
                _ => _ = self.paper.line(&self.doc.writing.words[frag.words[0] as usize..frag.words[1] as usize], left, base, bit.font, bit.color),
            }
            if let Some(to) = link {
                self.paper.link(rect(left, top, frag.wide, row.high), to);
                self.paper.end();
            }
        }
    }

    /// The mark of the item that is the `at`th open block, by the row whose baseline is `base`:
    /// a check box for a task, its number in a numbered list, else a bullet (disc, circle,
    /// square as lists nest). As `markdown_view.rs` draws them.
    fn item_mark(&mut self, at: usize, base: f32) {
        let (t, open) = (self.t, self.open[at]);
        let blocks = &self.doc.writing.blocks;
        let Kind::Item { number, ticked } = blocks[open.block].kind else { return };
        let numbered = matches!(blocks[blocks[open.block].parent as usize].kind, Kind::List { numbered: true, .. });
        let [above, _] = heights(&mut Measured(self.paper), t.font, 1.0, &mut self.fonts);
        let (room, middle) = (open.marks, base - (above * 0.35).round());
        // (The marks' column: left of the item, or right of it right to left.)
        let left = if open.leftward { open.x + open.w } else { open.x - room };
        match (ticked, numbered) {
            (Some(done), _) => self.tick(rect(left + (room - 14.0) / 2.0, middle - 7.0, 14.0, 14.0), done),
            (None, true) => {
                let written = format!("{number}.");
                let said = if open.leftward { format!(".{number}") } else { written.clone() };
                let wide = self.paper.measure(&said, t.font);
                // (Its number is its label. Drawn right to left, it reads as written.)
                let depth = self.paper.open.len();
                self.paper.open.extend(open.label);
                self.paper.read_as(if open.leftward { &written } else { "" });
                self.paper.line(&said, if open.leftward { left + 6.0 } else { open.x - 6.0 - wide }, base, t.font, t.text_dim);
                self.paper.open.truncate(depth);
            }
            (None, false) => {
                let dot = rect(left + room / 2.0 - 3.0, middle - 3.0, 6.0, 6.0);
                let (ring, round) = (open.depth == 2, if open.depth >= 3 { 0.0 } else { 3.0 });
                if ring { self.paper.border(dot, round, 1.0, t.text_dim) } else { self.paper.rounded(dot, round, t.text_dim) }
            }
        }
    }

    /// A task's check box at `r`: filled, with a tick, when done.
    fn tick(&mut self, r: Rect, done: bool) {
        let t = self.t;
        let (edge, fill) = if done { (t.accent, t.accent) } else { (t.line_strong, Color::NONE) };
        self.paper.rounded(r, 3.0, fill);
        self.paper.border(r, 3.0, 1.0, edge);
        if done {
            self.paper.icon(crate::widgets::CHECK, r, t.on_accent);
        }
    }

    /// A table at `x`, `w` wide: each column as wide as its widest unbroken cell, all narrowed
    /// alike if that is too wide; each row as high as its highest cell, on the page it fits on.
    /// Gives back the block after it.
    fn table(&mut self, at: usize, x: f32, w: f32) -> usize {
        let end = self.doc.writing.blocks[at].end as usize;
        let rows: Vec<usize> = (at + 1..end).filter(|row| self.doc.writing.blocks[*row].parent == at as u32).collect();
        let mut wants: Vec<f32> = Vec::new();
        for row in &rows {
            for (column, cell) in self.cells(*row).into_iter().enumerate() {
                let long = self.long(cell);
                wants.resize(wants.len().max(column + 1), 0.0);
                wants[column] = wants[column].max(long + 2.0 * self.size.cell[0] + 1.0);
            }
        }
        let total: f32 = wants.iter().sum();
        let narrowed = if total > w { w / total } else { 1.0 };
        let widths: Vec<f32> = wants.iter().map(|want| (want * narrowed).floor()).collect();
        // (A heading row is repeated at the top of each later page the table goes on to.)
        let head = rows.first().copied().filter(|row| matches!(self.doc.writing.blocks[*row].kind, Kind::Row { head: true }));
        self.paper.begin_tag(Tag::Table);
        for (n, row) in rows.iter().enumerate() {
            self.table_row(*row, x, &widths, n == 0, head);
        }
        self.paper.end();
        // The rule under the last row.
        self.paper.rect(rect(x, self.y - 1.0, widths.iter().sum(), 1.0), self.t.line);
        end
    }

    /// How wide a cell's words would be unbroken.
    fn long(&mut self, cell: usize) -> f32 {
        let blocks = &self.doc.writing.blocks;
        let head = matches!(blocks.get(blocks[cell].parent as usize).map(|row| row.kind), Some(Kind::Row { head: true }));
        self.bits(cell, font_of(blocks[cell].kind, head, &self.t));
        let bits = 0..self.laid.bits.len();
        self.laid.measure(bits, &self.doc.writing.words, &mut Measured(self.paper), 1.0, &mut self.fonts)
    }

    /// The cells of a table row.
    fn cells(&self, row: usize) -> Vec<usize> {
        let blocks = &self.doc.writing.blocks;
        (row + 1..blocks[row].end as usize).filter(|cell| blocks[*cell].parent == row as u32).collect()
    }

    /// A table row with its cells in columns `widths` wide from `x`: on the next page if it
    /// does not fit (with a rule under the part on this page, and the table's heading row
    /// `head` again at the top of the next, as decoration), with its rules above, around and
    /// between cells.
    fn table_row(&mut self, row: usize, x: f32, widths: &[f32], first: bool, head: Option<usize>) {
        let laid = self.lay_row(row, widths);
        let (high, wide, line) = (laid.iter().map(|(_, _, high)| *high).fold(1.0, f32::max), widths.iter().sum::<f32>(), self.t.line);
        let (page, before) = (self.page, self.y);
        self.room_for(high);
        if self.page != page && !first {
            self.paper.set_page(page);
            self.paper.rect(rect(x, before - 1.0, wide, 1.0), line);
            self.paper.set_page(self.page);
        }
        if let Some(head) = head.filter(|head| self.page != page && !first && *head != row) {
            let laid = self.lay_row(head, widths);
            self.paper.begin_decoration();
            self.put_row(head, laid, x, widths);
            self.paper.end();
        }
        self.put_row(row, laid, x, widths);
    }

    /// A table row's cells laid out in their columns: each one's rows, padding and height.
    fn lay_row(&mut self, row: usize, widths: &[f32]) -> Vec<(Laid, [f32; 2], f32)> {
        let mut laid = Vec::new();
        for (column, cell) in self.cells(row).into_iter().enumerate() {
            let pad = self.rows(cell, widths.get(column).copied().unwrap_or(0.0)).1;
            let high = self.laid.rows.iter().map(|row| row.high).sum::<f32>() + 2.0 * pad[1];
            laid.push((std::mem::take(&mut self.laid), pad, high));
        }
        laid
    }

    /// A table row's laid-out cells where the document has come to: its background if a
    /// heading row, rules, and each cell's words (a heading row's told as column headings).
    fn put_row(&mut self, row: usize, laid: Vec<(Laid, [f32; 2], f32)>, x: f32, widths: &[f32]) {
        let head = matches!(self.doc.writing.blocks[row].kind, Kind::Row { head: true });
        let (high, wide, line) = (laid.iter().map(|(_, _, high)| *high).fold(1.0, f32::max), widths.iter().sum::<f32>(), self.t.line);
        let top = self.y;
        self.paper.begin_tag(Tag::Row);
        if head {
            self.paper.rect(rect(x, top, wide, high), self.t.hover);
        }
        self.paper.rect(rect(x, top, wide, 1.0), line);
        let mut left = x;
        for (column, (cell, pad, _)) in laid.into_iter().enumerate() {
            self.laid = cell;
            self.paper.begin_tag(if head { Tag::Head } else { Tag::Cell });
            let mut y = top + pad[1];
            for row in self.laid.rows.clone() {
                self.write_row(&row, left, y);
                y += row.high;
            }
            self.paper.end();
            self.paper.rect(rect(left, top, 1.0, high), line);
            left += widths.get(column).copied().unwrap_or(0.0);
        }
        self.paper.rect(rect(left - 1.0, top, 1.0, high), line);
        self.paper.end();
        self.y = top + high;
    }
}

/// A picture at `r`, described by its words.
fn figure(paper: &mut Paper, said: &str, picture: u32, r: Rect) {
    paper.begin_tag(Tag::Figure);
    paper.describe(said.trim());
    paper.put(Mark::Picture(picture, r));
    paper.end();
}

impl Paper {
    /// A paper picture's size in pixels.
    pub(crate) fn picture_size(&self, at: u32) -> [f32; 2] {
        match &self.pictures[at as usize] {
            crate::paper::Picture::Pixels { width, pixels } => [*width as f32, (pixels.len() / width) as f32],
            crate::paper::Picture::Jpeg { size, .. } => [size[0] as f32, size[1] as f32],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of every kind of block, long enough for several pages.
    fn long() -> Markdown {
        let mut text = String::from("# A report\n\nIt has [a link](https://example.com) and [one to a part](#the-second-part).\n\n");
        for n in 0..12 {
            text.push_str(&format!("## Part {n}\n\nA paragraph of words that goes on for a while, so that it breaks into several rows on a page of this width, with *some* of them **strong** and `code`.\n\n- one\n- two\n  - three, in a list in the list\n\n1. first\n2. second\n\n> Quoted words, in a quote that holds a paragraph.\n\n```rust\nfn main() {{\n    println!(\"{n}\");\n}}\n```\n\n| Name | Size |\n| --- | ---: |\n| a | 1 |\n| b | 22 |\n\n---\n\n- [x] done\n- [ ] not yet\n\n"));
        }
        text.push_str("## The second part\n\nThe end.\n");
        Markdown::parse(&text)
    }

    #[test]
    fn a_document_goes_down_its_pages_and_stays_inside_their_margins() {
        let mut paper = Paper::new();
        let size = Size { w: 420.0, h: 595.0 };
        let pages = paper.markdown(&long(), size, 40.0);
        assert!(pages > 4 && paper.pages() == pages, "{pages} pages");
        for (page, sheet) in paper.pages.iter().enumerate() {
            assert!(!sheet.marks.is_empty(), "page {page} has something on it");
            for mark in &sheet.marks {
                let Mark::Words(words) = mark else { continue };
                let m = paper.files[words.font as usize].metrics(words.size);
                for glyph in &words.glyphs {
                    assert!(glyph.at.y - m.ascent >= 40.0 - 4.0 && glyph.at.y + m.descent <= size.h - 40.0 + 4.0, "page {page}: a row at {} is inside the margins", glyph.at.y);
                    assert!(glyph.at.x >= 40.0 - 1.0 && glyph.at.x <= size.w - 40.0, "page {page}: across at {}", glyph.at.x);
                }
            }
        }
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_right_to_left_list_has_its_numbers_at_the_right_on_paper() {
        let mut paper = Paper::new();
        paper.markdown(&Markdown::parse("1. \u{5d0}\u{5d2} \u{5d2}\u{5d0}\n\nWords between.\n\n1. ab\n"), Size { w: 420.0, h: 595.0 }, 40.0);
        let left_of = |words: &crate::paper::Words| words.glyphs.iter().map(|glyph| glyph.at.x).fold(f32::MAX, f32::min);
        let placed: Vec<(String, f32)> = paper.pages[0].marks.iter().filter_map(|mark| if let Mark::Words(words) = mark { Some((words.text.clone(), left_of(words))) } else { None }).collect();
        let at = |text: &str| placed.iter().find(|(own, _)| own.contains(text)).map(|(_, x)| *x).unwrap_or_else(|| panic!("{text:?} not in {placed:?}"));
        assert!(at(".1") > at("\u{5d0}\u{5d2}"), "right to left, the number is right of the item: {placed:?}");
        assert!(at("1.") < at("ab"), "left to right, left of it");
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_right_to_left_list_number_drawn_otherwise_reads_as_written() {
        use crate::pdf::tests::{objects, streamed};
        let mut paper = Paper::new();
        paper.markdown(&Markdown::parse("1. \u{5d0}\u{5d2} \u{5d2}\u{5d0}\n\nWords between.\n\n1. ab\n"), Paper::A4, 72.0);
        let file = paper.to_pdf();
        // Drawn `.1`, its label reads `1.` (ActualText: a span's in the page, and its element's);
        // drawn `1.`, it needs none.
        let objects = objects(&file);
        let contents: Vec<String> = objects.iter().filter_map(|(_, object)| streamed(object)).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).collect();
        let labels: Vec<&str> = contents.iter().flat_map(|content| content.lines()).filter(|line| line.contains("/MCID") && (line.starts_with("/Lbl") || line.starts_with("/Span"))).collect();
        assert!(labels.len() == 2 && labels[0].starts_with("/Span << /MCID 0 /ActualText <FEFF0031002E> >>") && labels[1].starts_with("/Lbl"), "{labels:?}");
        let elements = objects.iter().map(|(_, object)| String::from_utf8_lossy(object)).filter(|object| object.contains("/S /Lbl"));
        assert_eq!(elements.filter(|element| element.contains("/ActualText <FEFF0031002E>")).count(), 1);
        let Some(read) = crate::pdf::readers::text_of(&file) else { return eprintln!("no pdftotext here: not read back") };
        let first = read.lines().next().unwrap_or_default();
        assert!(first.contains("1.") && !first.contains(".1"), "copied out: {read:?}");
    }

    #[test]
    fn a_heading_goes_onto_the_page_of_what_follows_it() {
        let mut paper = Paper::new();
        paper.markdown(&long(), Size { w: 420.0, h: 595.0 }, 40.0);
        for (page, sheet) in paper.pages.iter().enumerate() {
            let words: Vec<&crate::paper::Words> = sheet.marks.iter().filter_map(|mark| if let Mark::Words(words) = mark { Some(&**words) } else { None }).collect();
            let Some(last) = words.last() else { continue };
            // A page never ends with a heading's words: those are larger.
            assert!(last.size < 17.0 || page + 1 == paper.pages(), "page {page} ends with words of size {}", last.size);
        }
    }

    #[test]
    fn links_in_a_document_lead_where_they_did() {
        let mut paper = Paper::new();
        paper.markdown(&long(), Size { w: 420.0, h: 595.0 }, 40.0);
        let links: Vec<&str> = paper.pages.iter().flat_map(|sheet| &sheet.marks).filter_map(|mark| if let Mark::Link(_, to) = mark { Some(to.as_str()) } else { None }).collect();
        assert!(links.contains(&"https://example.com") && links.contains(&"#the-second-part"), "{links:?}");
        let (_, page, _) = paper.anchors.iter().find(|(name, ..)| name == "the-second-part").expect("the heading is a place a link leads to");
        assert_eq!(*page + 1, paper.pages(), "on the last page");
        let file = paper.to_pdf();
        assert!(String::from_utf8_lossy(&file).contains("/Dest ["));
    }

    #[test]
    fn a_reader_takes_out_a_documents_words() {
        let doc = Markdown::parse("# Title\n\nFirst paragraph, with a [link](https://example.com).\n\n- an item\n\n```\ncode line\n```\n");
        let mut paper = Paper::new();
        paper.markdown(&doc, Paper::A4, 72.0);
        let Some(read) = crate::pdf::readers::text_of(&paper.to_pdf()) else { return eprintln!("no pdftotext here: not read back") };
        let read: Vec<String> = read.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
        assert_eq!(read, ["Title", "First paragraph, with a link.", "an item", "code line"]);
    }

    #[test]
    fn a_long_document_of_every_kind_of_block_lays_out() {
        // The library's own guide: headings, tables, code and lists over many pages.
        let mut paper = Paper::new();
        let pages = paper.markdown(&Markdown::parse(include_str!("../GUIDE.md")), Paper::A4, 72.0);
        assert!(pages > 8, "{pages} pages");
        let file = paper.to_pdf();
        assert!(file.starts_with(b"%PDF-") && file.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn a_document_lays_out_on_pages_of_no_size_and_of_a_size_that_is_no_number() {
        // `f32::clamp` panics when its least is above its most, or either is NaN.
        let doc = Markdown::parse("# A heading\n\nSome words, and a list:\n\n- one\n- two\n");
        for size in [Size { w: -10.0, h: 300.0 }, Size { w: f32::NAN, h: f32::NAN }, Size { w: 300.0, h: -1.0 }, Size { w: 0.0, h: 0.0 }] {
            let mut paper = Paper::new();
            let pages = paper.markdown(&doc, size, 36.0);
            assert!((1..100).contains(&pages), "{size:?}: {pages} pages");
            assert!(paper.to_pdf().starts_with(b"%PDF-"));
        }
    }
}
