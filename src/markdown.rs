//! CommonMark Markdown, parsed into a document taut shows
//! ([`Ui::markdown`](crate::Ui::markdown)): headings, paragraphs, emphasis and strong emphasis,
//! inline and block code, block quotes, ordered and bullet lists (nested to any depth), links,
//! images by a name the app supplies a picture for, thematic breaks, and hard line breaks; and
//! from GitHub's extensions, tables, task lists and strikethrough.
//!
//! Not supported: raw HTML, inline or block, is recognized and shown as literal text, not
//! rendered. Of entity names (`&copy;`), HTML 4's are decoded, and every numeric reference
//! (`&#169;`); names only later HTML has stay literal. Footnotes, bare-URL autolinks and front
//! matter are not CommonMark: they stay literal text. For emphasis rules, punctuation is any
//! non-ASCII character that is not a letter, digit or space (the spec asks for Unicode's
//! punctuation and symbol categories: combining marks count here too).
//!
//! Parsing follows the spec's appendix. Blocks line by line: each line is matched against the
//! open blocks outside in, as far as they continue; may open new ones; and is added to the
//! last. Then each text block's inlines, with a delimiter stack for emphasis and a bracket
//! stack for links. Nothing recurses: nesting is kept in lists, as boxes are.
use crate::code_editor::Syntax;
use crate::layout::Align;
use crate::rich_text::{self, Stretch, THING, TextStyle, Thing, Writing};
use crate::text::Decoration;
use crate::ui::Picture;
use std::collections::HashMap;

/// A tab stop is every this many columns.
const TAB: usize = 4;
/// No block, or no inline.
const NONE: u32 = u32::MAX;
/// The longest link label, in bytes.
const LABEL: usize = 999;
/// How deep blocks may always nest (a list and each item count one each, as does a quote).
/// Deeper is allowed while each blank line, which walks every open list item, costs no more
/// than `BLANK_STEPS` in all; past that, what would open another block is text.
const DEEPEST: usize = 100;
const BLANK_STEPS: usize = 4_000_000;

// ---- blocks ----

/// A block's kind, while parsing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Document,
    Quote,
    /// A list: ordered or not; its delimiter (after the number, or the bullet); its start
    /// number; and whether it is tight.
    List {
        numbered: bool,
        mark: u8,
        start: u32,
        tight: bool,
    },
    /// A list item: its marker's indent, and its content's offset from the marker; and its
    /// number, in an ordered list.
    Item {
        indent: usize,
        padding: usize,
        number: u32,
    },
    Paragraph,
    Heading {
        level: u8,
        setext: bool,
    },
    /// Code: fenced by this character (0 for indented code), so many of them, with the fence's
    /// indent.
    Code {
        fence: u8,
        length: usize,
        indent: usize,
    },
    /// HTML, of one of the spec's seven start conditions.
    Html(u8),
    Rule,
    /// A table (column alignments in its `info`, a letter each), its rows and their cells.
    Table,
    Row {
        head: bool,
    },
    Cell(Align),
    /// A paragraph of only link reference definitions, now nothing.
    Gone,
}

/// A block being parsed: its kind, its place in the tree, and its text.
struct Block {
    kind: Kind,
    /// Its parent, first and last children, and previous and next siblings.
    parent: u32,
    first: u32,
    last: u32,
    prev: u32,
    next: u32,
    /// Its text content: a text block's lines; code's lines as written.
    content: String,
    /// Fenced code's info string; a table's column alignments.
    info: String,
    /// Its last line was blank (what makes a list loose).
    blank: bool,
    /// How deep it is: the document is 0, its children 1.
    depth: u32,
    /// The line it started on.
    line: u32,
    /// For a task list item: whether checked.
    ticked: Option<bool>,
    /// For a text block, once its inlines are parsed: their root.
    inlines: u32,
}

/// Whether a block of this kind takes text lines, and so has no child blocks.
fn takes_lines(kind: Kind) -> bool {
    matches!(kind, Kind::Paragraph | Kind::Heading { .. } | Kind::Code { .. })
}

/// Whether `outer` can contain `inner`.
fn holds(outer: Kind, inner: Kind) -> bool {
    match outer {
        Kind::Document | Kind::Quote | Kind::Item { .. } => !matches!(inner, Kind::Item { .. }),
        Kind::List { .. } => matches!(inner, Kind::Item { .. }),
        Kind::Table => matches!(inner, Kind::Row { .. }),
        Kind::Row { .. } => matches!(inner, Kind::Cell(_)),
        _ => false,
    }
}

/// A list item marker: ordered or not, its delimiter character, its number, and its length in
/// bytes.
struct Mark {
    numbered: bool,
    mark: u8,
    number: u32,
    wide: usize,
}

/// The block parser, a line at a time.
struct Reader {
    blocks: Vec<Block>,
    /// The open blocks, document first: the last is where a text line goes.
    open: Vec<u32>,
    /// Link reference definitions: label to destination and title.
    refs: HashMap<String, (String, String)>,
    /// The current line with its line ending, and its number.
    line: String,
    number: u32,
    /// How far the line is consumed: in bytes and in columns (a tab counts to the next stop);
    /// and whether a tab there is partly consumed.
    offset: usize,
    column: usize,
    partial: bool,
    /// The first non-space character, in bytes and columns; the columns of space before it; and
    /// whether the line is blank from there.
    nonspace: usize,
    nonspace_column: usize,
    indent: usize,
    blank: bool,
    /// The open block the last line marked as ending blank, if any: the only open block that
    /// can be marked so (NONE for none).
    marked: u32,
    /// Where on the line a thematic break can start (`breaks_from`).
    breaks_from: [usize; 3],
    /// How deep blocks may nest in this text.
    deepest: usize,
}

impl Reader {
    /// A parser for a text of `blank_lines` blank lines.
    fn new(blank_lines: usize) -> Self {
        let document = Block {
            kind: Kind::Document,
            parent: NONE,
            first: NONE,
            last: NONE,
            prev: NONE,
            next: NONE,
            content: String::new(),
            info: String::new(),
            blank: false,
            depth: 0,
            line: 0,
            ticked: None,
            inlines: NONE,
        };
        Reader {
            blocks: vec![document],
            open: vec![0],
            refs: HashMap::new(),
            line: String::new(),
            number: 0,
            offset: 0,
            column: 0,
            partial: false,
            nonspace: 0,
            nonspace_column: 0,
            indent: 0,
            blank: false,
            marked: NONE,
            breaks_from: [0; 3],
            deepest: DEEPEST.max(BLANK_STEPS / (blank_lines + 1)),
        }
    }

    fn kind(&self, block: u32) -> Kind {
        self.blocks[block as usize].kind
    }
    fn tip(&self) -> u32 {
        self.open.last().copied().unwrap_or(0)
    }
    fn peek(&self, at: usize) -> u8 {
        self.line.as_bytes().get(at).copied().unwrap_or(0)
    }

    /// Finds the first non-space from the current position.
    fn find_nonspace(&mut self) {
        let (bytes, mut at, mut column) = (self.line.as_bytes(), self.offset, self.column);
        while let Some(space) = bytes.get(at).filter(|byte| matches!(byte, b' ' | b'\t')) {
            column += if *space == b'\t' { TAB - column % TAB } else { 1 };
            at += 1;
        }
        (self.nonspace, self.nonspace_column, self.indent) = (at, column, column - self.column);
        self.blank = matches!(bytes.get(at), None | Some(b'\n' | b'\r'));
    }

    /// Consumes `count` bytes, or `count` columns: a tab consumed by columns may be partly
    /// consumed, its rest then spaces.
    fn advance(&mut self, mut count: usize, columns: bool) {
        while count > 0 && self.offset < self.line.len() {
            let (tab, to_stop) = (self.line.as_bytes()[self.offset] == b'\t', TAB - self.column % TAB);
            let (gone, used) = match (tab, columns) {
                (true, true) => (to_stop.min(count), to_stop.min(count)),
                (true, false) => (to_stop, 1),
                (false, _) => (1, 1),
            };
            self.partial = tab && columns && to_stop > count;
            (self.column, self.offset, count) = (self.column + gone, self.offset + !self.partial as usize, count - used);
        }
    }

    /// Parses one line: through the open blocks, into new ones, then into the last.
    fn read_line(&mut self, line: String) {
        (self.breaks_from, self.line, self.number) = (breaks_from(line.as_bytes()), line, self.number + 1);
        (self.offset, self.column, self.partial, self.blank) = (0, 0, false, false);
        let Some((matched, all)) = self.match_open() else { return };
        let mut container = matched;
        self.open_new(&mut container, all);
        self.add_text(container, matched);
    }

    /// Matches the open blocks, document first: each consumes its prefix of the line, or stops
    /// continuing. Returns the last that continues, and whether all did; nothing where a
    /// closing fence took the line.
    fn match_open(&mut self) -> Option<(u32, bool)> {
        let mut matched = 0;
        for step in 1..self.open.len() {
            let at = self.open[step];
            self.find_nonspace();
            match self.goes_on(at) {
                Some(true) => matched = at,
                Some(false) => return Some((matched, false)),
                None => return None,
            }
        }
        Some((matched, true))
    }

    /// Whether an open block continues on this line, its prefix consumed; nothing for a closing
    /// fence.
    fn goes_on(&mut self, at: u32) -> Option<bool> {
        match self.kind(at) {
            Kind::Quote => Some(self.quote_mark()),
            Kind::Item { indent, padding, .. } => Some(self.item_goes_on(at, indent + padding)),
            Kind::Code { fence: 0, .. } => Some(self.indented_goes_on()),
            Kind::Code { fence, length, indent } => self.fence_goes_on(at, fence, length, indent),
            Kind::Heading { .. } | Kind::Rule => Some(false),
            Kind::Html(kind) => Some(kind <= 5 || !self.blank),
            Kind::Paragraph | Kind::Table => Some(!self.blank),
            _ => Some(true),
        }
    }

    /// Consumes a block quote marker, if the line has one.
    fn quote_mark(&mut self) -> bool {
        if self.indent > 3 || self.peek(self.nonspace) != b'>' {
            return false;
        }
        self.advance(self.indent + 1, true);
        if matches!(self.peek(self.offset), b' ' | b'\t') {
            self.advance(1, true);
        }
        true
    }

    /// An item continues on a line indented as far as its content, and on a blank line once it
    /// has content.
    fn item_goes_on(&mut self, at: u32, needs: usize) -> bool {
        if self.indent >= needs {
            self.advance(needs, true);
            return true;
        }
        let goes_on = self.blank && self.blocks[at as usize].first != NONE;
        if goes_on {
            self.advance(self.nonspace - self.offset, false);
        }
        goes_on
    }

    /// Indented code continues on a line indented as far, and on a blank one.
    fn indented_goes_on(&mut self) -> bool {
        if self.indent >= 4 {
            self.advance(4, true);
            return true;
        }
        if self.blank {
            self.advance(self.nonspace - self.offset, false);
        }
        self.blank
    }

    /// Fenced code continues until a closing fence at least as long; every other line loses up
    /// to its fence's indent.
    fn fence_goes_on(&mut self, at: u32, fence: u8, length: usize, indent: usize) -> Option<bool> {
        let closing = if self.indent <= 3 && self.peek(self.nonspace) == fence { closing_fence(self.line.as_bytes(), self.nonspace) } else { 0 };
        if closing >= length {
            self.close(at);
            return None;
        }
        for _ in 0..indent {
            if !matches!(self.peek(self.offset), b' ' | b'\t') {
                break;
            }
            self.advance(1, true);
        }
        Some(true)
    }

    /// Opens the blocks the rest of the line starts, nested, as long as it starts one (no
    /// deeper than `deepest`).
    fn open_new(&mut self, container: &mut u32, all: bool) {
        let mut lazy = self.kind(self.tip()) == Kind::Paragraph;
        while !matches!(self.kind(*container), Kind::Code { .. } | Kind::Html(_)) && (self.blocks[*container as usize].depth as usize) < self.deepest {
            self.find_nonspace();
            let Some(opened) = self.start(*container, all, lazy) else { break };
            *container = opened;
            if takes_lines(self.kind(opened)) {
                break;
            }
            lazy = false;
        }
    }

    /// Opens the block the line starts here, in `container`, if any, and returns the block for
    /// the rest of the line.
    fn start(&mut self, container: u32, all: bool, lazy: bool) -> Option<u32> {
        let (indented, kind, at) = (self.indent >= 4, self.kind(container), self.nonspace);
        let bytes = self.line.as_bytes();
        if !indented && bytes[at] == b'>' {
            return Some(self.open_quote(container));
        }
        if let Some(marks) = atx_start(bytes, at).filter(|_| !indented) {
            return Some(self.open_heading(container, marks));
        }
        if let Some(length) = fence_start(bytes, at).filter(|_| !indented) {
            return Some(self.open_fence(container, length));
        }
        if let Some(html) = html_start(bytes, at, kind != Kind::Paragraph).filter(|_| !indented) {
            return Some(self.add_child(container, Kind::Html(html)));
        }
        if let Some(level) = setext_line(bytes, at).filter(|_| !indented && kind == Kind::Paragraph) {
            return Some(self.setext(container, level));
        }
        if !indented && !(kind == Kind::Paragraph && !all) && thematic_break(bytes, at, &self.breaks_from) {
            let rule = self.add_child(container, Kind::Rule);
            self.advance(self.line.len() - 1 - self.offset, false);
            return Some(rule);
        }
        if let Some(mark) = list_mark(bytes, at, kind == Kind::Paragraph).filter(|_| self.indent < 4) {
            return Some(self.open_item(container, mark));
        }
        if indented && !lazy && !self.blank {
            self.advance(4, true);
            return Some(self.add_child(container, Kind::Code { fence: 0, length: 0, indent: 0 }));
        }
        if !indented && kind == Kind::Paragraph {
            return self.open_table(container);
        }
        None
    }

    fn open_quote(&mut self, container: u32) -> u32 {
        self.advance(self.nonspace + 1 - self.offset, false);
        if matches!(self.peek(self.offset), b' ' | b'\t') {
            self.advance(1, true);
        }
        self.add_child(container, Kind::Quote)
    }

    /// `marks` is how much of the line the heading markers and following spaces take.
    fn open_heading(&mut self, container: u32, marks: usize) -> u32 {
        let level = self.line.as_bytes()[self.nonspace..].iter().take_while(|byte| **byte == b'#').count() as u8;
        self.advance(self.nonspace + marks - self.offset, false);
        self.add_child(container, Kind::Heading { level, setext: false })
    }

    fn open_fence(&mut self, container: u32, length: usize) -> u32 {
        let (fence, indent) = (self.peek(self.nonspace), self.nonspace - self.offset);
        let code = self.add_child(container, Kind::Code { fence, length, indent });
        self.advance(self.nonspace + length - self.offset, false);
        code
    }

    /// A setext underline makes a paragraph a heading: unless it held only link definitions,
    /// which the line then follows.
    fn setext(&mut self, paragraph: u32, level: u8) -> u32 {
        if self.define_links(paragraph) {
            self.blocks[paragraph as usize].kind = Kind::Heading { level, setext: true };
            self.advance(self.line.len() - 1 - self.offset, false);
        }
        paragraph
    }

    fn open_item(&mut self, container: u32, mark: Mark) -> u32 {
        self.advance(self.nonspace + mark.wide - self.offset, false);
        let (offset, column, partial) = (self.offset, self.column, self.partial);
        while self.column - column <= 5 && matches!(self.peek(self.offset), b' ' | b'\t') {
            self.advance(1, true);
        }
        // Content starts after the spaces following the marker: after one where there are five
        // or more (the rest is indented code), or where there are none before the line end.
        let spaces = self.column - column;
        let words_follow = (1..5).contains(&spaces) && !matches!(self.peek(self.offset), b'\n' | b'\r' | 0);
        if !words_follow {
            (self.offset, self.column, self.partial) = (offset, column, partial);
        }
        if !words_follow && spaces > 0 {
            self.advance(1, true);
        }
        let padding = mark.wide + if words_follow { spaces } else { 1 };
        let (numbered, kind) = (mark.numbered, self.kind(container));
        let same = matches!(kind, Kind::List { numbered: was, mark: had, .. } if was == numbered && had == mark.mark);
        let list = if same { container } else { self.add_child(container, Kind::List { numbered, mark: mark.mark, start: mark.number, tight: false }) };
        self.add_child(list, Kind::Item { indent: self.indent, padding, number: mark.number })
    }

    /// A paragraph whose last line has as many cells as this delimiter row has columns is a
    /// table header, which this line starts. (Lines before its last stay a paragraph.)
    fn open_table(&mut self, paragraph: u32) -> Option<u32> {
        let sides = table_sides(&self.line[self.nonspace..])?;
        let content = &self.blocks[paragraph as usize].content;
        let lines = content.trim_end_matches(['\n', '\r']);
        let (before, head) = lines.rsplit_once('\n').map_or(("", lines), |(before, head)| (before, head));
        let heads = cells(head);
        if heads.len() != sides.len() {
            return None;
        }
        let (before, parent) = (before.to_string(), self.blocks[paragraph as usize].parent);
        let table = if before.is_empty() {
            self.blocks[paragraph as usize].kind = Kind::Table;
            paragraph
        } else {
            self.blocks[paragraph as usize].content = before + "\n";
            self.add_child(parent, Kind::Table)
        };
        self.blocks[table as usize].info = sides.iter().map(|side| ['l', 'c', 'r', '-'][*side as usize]).collect();
        self.add_row(table, heads, true);
        self.advance(self.line.len() - 1 - self.offset, false);
        Some(table)
    }

    /// Adds a table row: as many cells as it has columns, of these texts.
    fn add_row(&mut self, table: u32, words: Vec<String>, head: bool) {
        let row = self.add_block(table, Kind::Row { head }, String::new());
        let sides: Vec<Align> = self.blocks[table as usize].info.chars().map(|side| [Align::Start, Align::Center, Align::End, Align::Spread]["lcr-".find(side).unwrap_or(3)]).collect();
        let mut words = words.into_iter();
        for side in sides {
            self.add_block(row, Kind::Cell(side), words.next().unwrap_or_default());
        }
    }

    /// Whether open block `outer` holds `inner`, which is open too.
    fn holds_open(&self, outer: u32, inner: u32) -> bool {
        let depth = self.blocks[outer as usize].depth as usize;
        self.open.get(depth) == Some(&outer) && self.blocks[inner as usize].depth as usize > depth
    }

    /// Puts the rest of the line where it belongs: a lazily continued paragraph, the open
    /// block, or a new paragraph.
    fn add_text(&mut self, container: u32, matched: u32) {
        self.find_nonspace();
        let (blank, kind) = (self.blank, self.kind(container));
        if let Some(last) = Some(self.blocks[container as usize].last).filter(|last| blank && *last != NONE) {
            self.blocks[last as usize].blank = true;
        }
        // (A blank line counts against its block unless that cannot be loosened by it: a quote,
        // a heading, a fence, a just-started item.)
        let block = &self.blocks[container as usize];
        let starts_empty = matches!(kind, Kind::Item { .. }) && block.first == NONE && block.line == self.number;
        let fenced = matches!(kind, Kind::Code { fence, .. } if fence != 0);
        let ends_blank = blank && !starts_empty && !fenced && !matches!(kind, Kind::Quote | Kind::Heading { .. } | Kind::Rule);
        self.blocks[container as usize].blank = ends_blank;
        // (A line inside a block ends its blank run. Only one open block can be in one: the
        // last marked, while open.)
        let marked = self.marked;
        if marked == container || marked != NONE && self.holds_open(marked, container) {
            (self.blocks[marked as usize].blank, self.marked) = (marked == container && ends_blank, NONE);
        }
        if ends_blank {
            self.marked = container;
        }
        let tip = self.tip();
        if tip != matched && container == matched && !blank && self.kind(tip) == Kind::Paragraph {
            return self.add_line(tip);
        }
        self.close_to(container);
        match kind {
            Kind::Code { .. } => self.add_line(container),
            Kind::Html(html) => self.add_html(container, html),
            _ if blank => {}
            Kind::Table => self.add_row(container, cells(&self.line[self.nonspace..]), false),
            Kind::Paragraph | Kind::Heading { .. } => self.add_words(container),
            _ => {
                let paragraph = self.add_child(container, Kind::Paragraph);
                self.add_words(paragraph);
            }
        }
    }

    /// Adds the line to a text block, from its first non-space; for an ATX heading, without its
    /// closing sequence.
    fn add_words(&mut self, block: u32) {
        if matches!(self.kind(block), Kind::Heading { setext: false, .. }) {
            let end = closed_heading(self.line.as_bytes());
            self.line.truncate(end.max(self.nonspace));
        }
        self.advance(self.nonspace - self.offset, false);
        self.add_line(block);
    }

    /// Adds the line to an HTML block, which it may end.
    fn add_html(&mut self, block: u32, html: u8) {
        self.add_line(block);
        if html_ends(html, &self.line.as_bytes()[self.nonspace..]) {
            self.close(block);
        }
    }

    /// Adds the rest of the line to a block, a partly consumed tab as its remaining spaces.
    fn add_line(&mut self, block: u32) {
        let Reader { blocks, line, offset, column, partial, .. } = self;
        let content = &mut blocks[block as usize].content;
        if *partial {
            *offset += 1;
            content.extend(std::iter::repeat_n(' ', TAB - *column % TAB));
        }
        content.push_str(line.get(*offset..).unwrap_or_default());
    }

    /// Opens a `kind` block in `parent`, or in the nearest ancestor that can contain it,
    /// closing the open ones between.
    fn add_child(&mut self, parent: u32, kind: Kind) -> u32 {
        self.close_to(parent);
        while !holds(self.kind(self.tip()), kind) {
            self.close(self.tip());
        }
        let child = self.add_block(self.tip(), kind, String::new());
        self.open.push(child);
        child
    }

    /// Appends a child to `parent` without opening it.
    fn add_block(&mut self, parent: u32, kind: Kind, content: String) -> u32 {
        let (child, prev) = (self.blocks.len() as u32, self.blocks[parent as usize].last);
        let depth = self.blocks[parent as usize].depth + 1;
        let block = Block { kind, parent, first: NONE, last: NONE, prev, next: NONE, content, info: String::new(), blank: false, depth, line: self.number, ticked: None, inlines: NONE };
        self.blocks.push(block);
        match prev {
            NONE => self.blocks[parent as usize].first = child,
            prev => self.blocks[prev as usize].next = child,
        }
        self.blocks[parent as usize].last = child;
        child
    }

    /// Closes the open blocks inside `block`.
    fn close_to(&mut self, block: u32) {
        while self.tip() != block && self.open.len() > 1 {
            self.close(self.tip());
        }
    }

    /// Closes the innermost open block, `block`: its content is final.
    fn close(&mut self, block: u32) {
        debug_assert_eq!(self.tip(), block);
        self.open.pop();
        match self.kind(block) {
            Kind::Paragraph if !self.define_links(block) => self.drop_block(block),
            Kind::Code { fence: 0, .. } => end_code(&mut self.blocks[block as usize].content),
            Kind::Code { .. } => self.split_info(block),
            _ => {}
        }
    }

    /// Strips link reference definitions from the start of a paragraph. Says whether anything
    /// is left.
    fn define_links(&mut self, block: u32) -> bool {
        let content = std::mem::take(&mut self.blocks[block as usize].content);
        let mut at = 0;
        while content[at..].starts_with('[') {
            let Some(used) = definition(&content[at..], &mut self.refs) else { break };
            at += used;
        }
        let left = &content[at..];
        let anything = left.bytes().any(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
        self.blocks[block as usize].content = left.to_string();
        anything
    }

    /// Removes an empty block from its parent. (It is the last child.)
    fn drop_block(&mut self, block: u32) {
        let Block { parent, prev, .. } = self.blocks[block as usize];
        self.blocks[block as usize].kind = Kind::Gone;
        self.blocks[parent as usize].last = prev;
        match prev {
            NONE => self.blocks[parent as usize].first = NONE,
            prev => self.blocks[prev as usize].next = NONE,
        }
    }

    /// Fenced code's first line is the info string.
    fn split_info(&mut self, block: u32) {
        let content = std::mem::take(&mut self.blocks[block as usize].content);
        let (first, rest) = content.split_once('\n').unwrap_or((&content, ""));
        self.blocks[block as usize].info = unescape(first.trim_matches([' ', '\t', '\r'])).trim().to_string();
        self.blocks[block as usize].content = rest.to_string();
    }

    /// Once everything is parsed: which lists are tight, and which items are tasks.
    fn finish(&mut self) {
        for at in 0..self.blocks.len() {
            match self.blocks[at].kind {
                Kind::List { numbered, mark, start, .. } => self.blocks[at].kind = Kind::List { numbered, mark, start, tight: self.tight(at as u32) },
                Kind::Item { .. } => self.task(at as u32),
                _ => {}
            }
        }
    }

    /// A list is tight unless a blank line separates two items, or two blocks within one.
    fn tight(&self, list: u32) -> bool {
        let mut item = self.blocks[list as usize].first;
        while item != NONE {
            let (next, mut inner) = (self.blocks[item as usize].next, self.blocks[item as usize].first);
            if self.blocks[item as usize].blank && next != NONE {
                return false;
            }
            while inner != NONE && !((next != NONE || self.blocks[inner as usize].next != NONE) && self.ends_blank(inner)) {
                inner = self.blocks[inner as usize].next;
            }
            if inner != NONE {
                return false;
            }
            item = next;
        }
        true
    }

    /// Whether a block ends with a blank line: it, or its last descendant, for a list or item.
    fn ends_blank(&self, mut block: u32) -> bool {
        while block != NONE {
            if self.blocks[block as usize].blank {
                return true;
            }
            block = if matches!(self.kind(block), Kind::List { .. } | Kind::Item { .. }) { self.blocks[block as usize].last } else { NONE };
        }
        false
    }

    /// An item whose first paragraph starts with `[ ]` or `[x]` and a space is a task.
    fn task(&mut self, item: u32) {
        let first = self.blocks[item as usize].first;
        if first == NONE || self.kind(first) != Kind::Paragraph {
            return;
        }
        let content = &self.blocks[first as usize].content;
        let words = content.trim_start_matches(' ');
        let ticked = match words.as_bytes() {
            [b'[', b' ' | b'\t', b']', b' ' | b'\t' | b'\n', ..] => false,
            [b'[', b'x' | b'X', b']', b' ' | b'\t' | b'\n', ..] => true,
            _ => return,
        };
        let rest = words[3..].trim_start_matches([' ', '\t']).to_string();
        self.blocks[first as usize].content = rest;
        self.blocks[item as usize].ticked = Some(ticked);
    }
}

/// Indented code ends at its last non-blank line.
fn end_code(content: &mut String) {
    let last = content.rfind(|letter: char| !matches!(letter, ' ' | '\t' | '\n' | '\r'));
    let end = last.map_or(0, |at| at + content[at..].find(['\n', '\r']).unwrap_or(content.len() - at));
    content.truncate(end);
    content.push('\n');
}

/// A text's lines, each with a line ending, whatever kind: two characters, one, or none at the
/// very end. (NUL becomes the replacement character.)
fn lines(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let after = if rest[end..].starts_with("\r\n") { end + 2 } else { (end + 1).min(rest.len()) };
        let line = rest[..end].replace('\0', "\u{fffd}") + "\n";
        rest = &rest[after..];
        Some(line)
    })
}

// ---- how blocks start, as the line says ----

/// An ATX heading opener at `at` (one to six `#`, then a space or the line end): its length
/// with the following spaces.
fn atx_start(line: &[u8], at: usize) -> Option<usize> {
    let marks = line[at..].iter().take_while(|byte| **byte == b'#').count();
    let after = line.get(at + marks).copied().unwrap_or(b'\n');
    if !(1..=6).contains(&marks) || !matches!(after, b' ' | b'\t' | b'\n' | b'\r') {
        return None;
    }
    Some(marks + line[at + marks..].iter().take_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r')).count().max(1))
}

/// Where an ATX heading's content ends: before an optional closing sequence (preceded by a
/// space) and trailing spaces.
fn closed_heading(line: &[u8]) -> usize {
    let trimmed = |end: usize| line[..end].iter().rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r')).map_or(0, |at| at + 1);
    let end = trimmed(line.len());
    let marks = line[..end].iter().rev().take_while(|byte| **byte == b'#').count();
    match end.checked_sub(marks + 1).map(|before| line[before]) {
        Some(b' ' | b'\t') if marks > 0 => trimmed(end - marks),
        None if marks > 0 => 0,
        _ => end,
    }
}

/// A code fence opener at `at`: three or more backticks or tildes (for backticks, none later on
/// the line). How many.
fn fence_start(line: &[u8], at: usize) -> Option<usize> {
    let fence = *line.get(at).filter(|byte| matches!(byte, b'`' | b'~'))?;
    let length = line[at..].iter().take_while(|byte| **byte == fence).count();
    (length >= 3 && (fence == b'~' || !line[at + length..].contains(&b'`'))).then_some(length)
}

/// A closing code fence at `at`: its character count if only spaces follow; else 0.
fn closing_fence(line: &[u8], at: usize) -> usize {
    let length = line[at..].iter().take_while(|byte| **byte == line[at]).count();
    let rest = &line[at + length..];
    if length >= 3 && rest.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r')) { length } else { 0 }
}

/// A setext underline of `=` (1) or `-` (2), making that heading level.
fn setext_line(line: &[u8], at: usize) -> Option<u8> {
    let mark = *line.get(at).filter(|byte| matches!(byte, b'=' | b'-'))?;
    let length = line[at..].iter().take_while(|byte| **byte == mark).count();
    let rest = &line[at + length..];
    rest.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r')).then_some(if mark == b'=' { 1 } else { 2 })
}

/// The characters a thematic break is made of.
const BREAK_MARKS: [u8; 3] = *b"*-_";

/// For each of `BREAK_MARKS`, where on a line a break of it can start: past every byte that is
/// neither it nor a space. (So a line of nested list markers is not read again from each one.)
fn breaks_from(line: &[u8]) -> [usize; 3] {
    BREAK_MARKS.map(|mark| line.iter().rposition(|byte| *byte != mark && !matches!(byte, b' ' | b'\t' | b'\n' | b'\r')).map_or(0, |last| last + 1))
}

/// Three or more `*`, `-` or `_` alone on a line, with or without spaces between: from `at`, on
/// a line whose breaks can start as `from` says (`breaks_from`).
fn thematic_break(line: &[u8], at: usize, from: &[usize; 3]) -> bool {
    let Some(kind) = line.get(at).and_then(|byte| BREAK_MARKS.iter().position(|mark| mark == byte)) else { return false };
    at >= from[kind] && line[at..].iter().filter(|byte| **byte == BREAK_MARKS[kind]).take(3).count() == 3
}

/// A list marker at `at`: `-`, `+` or `*`, or up to nine digits then `.` or `)`; then a space
/// or the line end. One interrupting a paragraph must have content and, if ordered, start at 1.
fn list_mark(line: &[u8], at: usize, in_paragraph: bool) -> Option<Mark> {
    let figures = line[at..].iter().take(10).take_while(|byte| byte.is_ascii_digit()).count();
    let (numbered, wide) = match line[at] {
        b'-' | b'+' | b'*' => (false, 1),
        _ if (1..=9).contains(&figures) && matches!(line.get(at + figures), Some(b'.' | b')')) => (true, figures + 1),
        _ => return None,
    };
    let after = line.get(at + wide).copied().unwrap_or(b'\n');
    if !matches!(after, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        return None;
    }
    let number = std::str::from_utf8(&line[at..at + figures]).ok().and_then(|number| number.parse().ok()).unwrap_or(0);
    let words = line[at + wide..].iter().any(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
    if in_paragraph && (!words || numbered && number != 1) {
        return None;
    }
    Some(Mark { numbered, mark: line[at + wide - 1], number, wide })
}

/// HTML's block tag names: a line starting with one as a tag starts HTML ending at a blank
/// line.
const BLOCK_TAGS: [&str; 62] = [
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// The tags whose HTML continues until they close, blank lines included.
const RAW_TAGS: [&str; 4] = ["pre", "script", "style", "textarea"];

/// Whether `text` starts with `word`, case-insensitively.
fn starts_with_word(text: &[u8], word: &str) -> bool {
    text.get(..word.len()).is_some_and(|start| start.eq_ignore_ascii_case(word.as_bytes()))
}

/// The HTML block kind a line starts at `at`, by the spec's numbering; the seventh (a complete
/// tag alone on its line) only where `seventh` allows.
fn html_start(line: &[u8], at: usize, seventh: bool) -> Option<u8> {
    let rest = &line[at..];
    if rest.first() != Some(&b'<') {
        return None;
    }
    let ends_name = |byte: Option<&u8>| matches!(byte, None | Some(b' ' | b'\t' | b'\n' | b'\r' | b'>'));
    if RAW_TAGS.iter().any(|tag| starts_with_word(&rest[1..], tag) && ends_name(rest.get(1 + tag.len()))) {
        return Some(1);
    }
    let starts = [(&b"<!--"[..], 2), (b"<?", 3), (b"<![CDATA[", 5)];
    if let Some((_, kind)) = starts.iter().find(|(start, _)| rest.starts_with(start)) {
        return Some(*kind);
    }
    if rest.starts_with(b"<!") && rest.get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some(4);
    }
    let name = if rest.starts_with(b"</") { &rest[2..] } else { &rest[1..] };
    let block = BLOCK_TAGS.iter().find(|tag| starts_with_word(name, tag) && (ends_name(name.get(tag.len())) || name[tag.len()..].starts_with(b"/>")));
    if block.is_some() {
        return Some(6);
    }
    let tag = if rest.get(1) == Some(&b'/') { closing_tag(&rest[1..]) } else { opening_tag(&rest[1..]) }.filter(|_| seventh)?;
    let raw = RAW_TAGS.iter().any(|raw| starts_with_word(&rest[1..], raw) && !rest.get(1 + raw.len()).is_some_and(u8::is_ascii_alphanumeric));
    let alone = rest[1 + tag..].iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c));
    (alone && !raw).then_some(7)
}

/// Whether a line ends HTML of kind `html` (the first five end at a marker).
fn html_ends(html: u8, line: &[u8]) -> bool {
    let has = |mark: &[u8]| line.windows(mark.len()).any(|window| window.eq_ignore_ascii_case(mark));
    match html {
        1 => RAW_TAGS.iter().any(|tag| has(format!("</{tag}>").as_bytes())),
        2 => has(b"-->"),
        3 => has(b"?>"),
        4 => has(b">"),
        5 => has(b"]]>"),
        _ => false,
    }
}

/// Whitespace within a tag: spaces, tabs and line endings.
fn tag_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// HTML starting at the `<` just before `at`: an open or closing tag, a comment, a processing
/// instruction, a declaration, or a CDATA section. Its length after the `<`.
fn html_tag(text: &[u8], at: usize) -> Option<usize> {
    let rest = &text[at..];
    let found = |end: &[u8], from: usize| rest[from..].windows(end.len()).position(|window| window == end).map(|found| from + found + end.len());
    match rest {
        [b'!', b'-', b'-', b'>', ..] => Some(4),
        [b'!', b'-', b'-', b'-', b'>', ..] => Some(5),
        [b'!', b'-', b'-', ..] => found(b"-->", 3),
        [b'?', ..] => found(b"?>", 1),
        [b'!', b'[', b'C', b'D', b'A', b'T', b'A', b'[', ..] => found(b"]]>", 8),
        [b'!', letter, ..] if letter.is_ascii_alphabetic() => rest.iter().position(|byte| *byte == b'>').map(|end| end + 1),
        [b'/', ..] => closing_tag(rest),
        _ => opening_tag(rest),
    }
}

/// A tag name: a letter, then letters, digits and hyphens. Its length.
fn tag_name(text: &[u8]) -> usize {
    match text.first() {
        Some(letter) if letter.is_ascii_alphabetic() => 1 + text[1..].iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-').count(),
        _ => 0,
    }
}

/// `/name>`, with optional spaces before the `>`.
fn closing_tag(text: &[u8]) -> Option<usize> {
    let name = tag_name(&text[1..]);
    let end = 1 + name + text[1 + name..].iter().take_while(|byte| tag_space(**byte)).count();
    (name > 0 && text.get(end) == Some(&b'>')).then_some(end + 1)
}

/// `name`, attributes, an optional `/`, and `>`.
fn opening_tag(text: &[u8]) -> Option<usize> {
    let mut at = tag_name(text);
    if at == 0 {
        return None;
    }
    while let Some(after) = attribute(text, at) {
        at = after;
    }
    at += text[at..].iter().take_while(|byte| tag_space(**byte)).count();
    at += (text.get(at) == Some(&b'/')) as usize;
    (text.get(at) == Some(&b'>')).then_some(at + 1)
}

/// A tag attribute at `at`: whitespace, a name, and an optional `=` and value. Where it ends.
fn attribute(text: &[u8], at: usize) -> Option<usize> {
    let spaces = text[at..].iter().take_while(|byte| tag_space(**byte)).count();
    let name_starts = |byte: &u8| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':');
    if spaces == 0 || !text.get(at + spaces).is_some_and(name_starts) {
        return None;
    }
    let name = 1 + text[at + spaces + 1..].iter().take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')).count();
    let after = at + spaces + name;
    let before_value = after + text[after..].iter().take_while(|byte| tag_space(**byte)).count();
    if text.get(before_value) != Some(&b'=') {
        return Some(after);
    }
    let start = before_value + 1 + text[before_value + 1..].iter().take_while(|byte| tag_space(**byte)).count();
    let value = match text.get(start) {
        Some(quote @ (b'"' | b'\'')) => text[start + 1..].iter().position(|byte| byte == quote).map(|end| end + 2),
        _ => Some(text[start..].iter().take_while(|byte| !tag_space(**byte) && !matches!(byte, b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')).count()).filter(|value| *value > 0),
    };
    value.map(|value| start + value)
}

// ---- tables ----

/// A table row's cells: the text between unescaped pipes (leading and trailing pipes optional),
/// trimmed, with escaped pipes unescaped.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let mut rest = line.strip_prefix('|').unwrap_or(line);
    let mut cells = Vec::new();
    while !rest.is_empty() || cells.is_empty() && !line.is_empty() {
        let bytes = rest.as_bytes();
        let pipe = (0..bytes.len()).find(|at| bytes[*at] == b'|' && (*at == 0 || bytes[*at - 1] != b'\\'));
        let (cell, after) = pipe.map_or((rest, ""), |at| (&rest[..at], &rest[at + 1..]));
        cells.push(cell.trim_matches([' ', '\t', '\x0b', '\x0c']).replace("\\|", "|"));
        rest = after;
    }
    cells
}

/// The delimiter row under a table header: a cell of hyphens per column, with a colon at either
/// end, or both, for left, right or centre alignment.
fn table_sides(line: &str) -> Option<Vec<Align>> {
    let sides: Option<Vec<Align>> = cells(line).iter().map(|cell| table_side(cell)).collect();
    sides.filter(|sides| !sides.is_empty())
}

fn table_side(cell: &str) -> Option<Align> {
    let (left, right) = (cell.starts_with(':'), cell.ends_with(':') && cell.len() > 1);
    let dashes = cell.trim_start_matches(':').trim_end_matches(':');
    if dashes.is_empty() || !dashes.bytes().all(|byte| byte == b'-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => Align::Center,
        (false, true) => Align::End,
        (true, false) => Align::Start,
        (false, false) => Align::Spread,
    })
}
// ---- what lies in a line ----

/// An inline's kind.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    /// The root of one block's inlines.
    Root,
    Text,
    /// A soft line break, shown as a space; and a hard one.
    Soft,
    Hard,
    Code,
    Html,
    Emphasis,
    Strong,
    Struck,
    Link,
    Image,
}

/// An inline: text, or a container (emphasis, a link), among its block's others.
struct Inline {
    kind: Span,
    /// Its text (of text, code and HTML); for a link or image, its destination.
    text: String,
    /// A link's or image's title.
    title: String,
    parent: u32,
    first: u32,
    last: u32,
    prev: u32,
    next: u32,
}

/// Detaches an inline from its parent.
fn unlink(nodes: &mut [Inline], node: u32) {
    let Inline { parent, prev, next, .. } = nodes[node as usize];
    if parent == NONE {
        return;
    }
    match prev {
        NONE => nodes[parent as usize].first = next,
        prev => nodes[prev as usize].next = next,
    }
    match next {
        NONE => nodes[parent as usize].last = prev,
        next => nodes[next as usize].prev = prev,
    }
    (nodes[node as usize].parent, nodes[node as usize].prev, nodes[node as usize].next) = (NONE, NONE, NONE);
}

/// Appends an inline to `parent`'s children, detaching it first.
fn append(nodes: &mut [Inline], parent: u32, node: u32) {
    unlink(nodes, node);
    let last = nodes[parent as usize].last;
    (nodes[node as usize].parent, nodes[node as usize].prev) = (parent, last);
    match last {
        NONE => nodes[parent as usize].first = node,
        last => nodes[last as usize].next = node,
    }
    nodes[parent as usize].last = node;
}

/// Inserts an inline just before another, or (`after`) just after it.
fn insert(nodes: &mut [Inline], beside: u32, node: u32, after: bool) {
    unlink(nodes, node);
    let parent = nodes[beside as usize].parent;
    let (prev, next) = if after { (beside, nodes[beside as usize].next) } else { (nodes[beside as usize].prev, beside) };
    (nodes[node as usize].parent, nodes[node as usize].prev, nodes[node as usize].next) = (parent, prev, next);
    match prev {
        NONE => nodes[parent as usize].first = node,
        prev => nodes[prev as usize].next = node,
    }
    match next {
        NONE => nodes[parent as usize].last = node,
        next => nodes[next as usize].prev = node,
    }
}

/// No delimiter.
const NO: usize = usize::MAX;

/// A run of `*`, `_` or `~` that may open or close emphasis.
struct Delimiter {
    inline: u32,
    mark: u8,
    /// The run's original length (for the rule of three).
    length: usize,
    opens: bool,
    closes: bool,
    /// The previous and next delimiters still on the stack.
    prev: usize,
    next: usize,
}

/// A `[` or `![` that may start a link or image.
struct Bracket {
    inline: u32,
    image: bool,
    /// Whether a link may still start here: not once a link was made after it.
    active: bool,
    /// Where its text starts, in bytes.
    position: usize,
    /// How many delimiters preceded it.
    delimiters: usize,
    /// Another bracket was opened after it.
    after: bool,
}

/// Parses one text block's inlines.
struct Inlines<'a> {
    text: &'a str,
    at: usize,
    nodes: &'a mut Vec<Inline>,
    refs: &'a HashMap<String, (String, String)>,
    delimiters: Vec<Delimiter>,
    /// The last delimiter still on the stack.
    last: usize,
    brackets: Vec<Bracket>,
}

/// Whether a byte may start something other than plain text.
fn special(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r' | b'\\' | b'`' | b'&' | b'*' | b'_' | b'~' | b'[' | b']' | b'!' | b'<')
}

/// Whether a character counts as punctuation for emphasis (see the top of this file).
fn punctuation(letter: char) -> bool {
    if letter.is_ascii() { letter.is_ascii_punctuation() } else { !letter.is_alphanumeric() && !letter.is_whitespace() }
}

impl Inlines<'_> {
    fn make(&mut self, kind: Span, text: String) -> u32 {
        self.nodes.push(Inline { kind, text, title: String::new(), parent: NONE, first: NONE, last: NONE, prev: NONE, next: NONE });
        self.nodes.len() as u32 - 1
    }

    /// Parses all the text into `root`.
    fn read(&mut self, root: u32) {
        while self.at < self.text.len() {
            if let Some(inline) = self.next_inline() {
                append(self.nodes, root, inline);
            }
        }
        self.emphasis(0);
    }

    /// The next inline: nothing where the parse merged into existing ones (a link).
    fn next_inline(&mut self) -> Option<u32> {
        let bytes = self.text.as_bytes();
        match bytes[self.at] {
            b'\n' | b'\r' => Some(self.line_end()),
            b'`' => Some(self.backticks()),
            b'\\' => Some(self.backslash()),
            b'&' => Some(self.entity()),
            b'<' => Some(self.pointy()),
            mark @ (b'*' | b'_' | b'~') => Some(self.delimiter(mark)),
            b'[' => Some(self.bracket(false)),
            b'!' if bytes.get(self.at + 1) == Some(&b'[') => Some(self.bracket(true)),
            b']' => self.close_bracket(),
            _ => Some(self.plain()),
        }
    }

    /// Text up to the next possibly special character; spaces before a line ending are
    /// excluded.
    fn plain(&mut self) -> u32 {
        let (bytes, start) = (self.text.as_bytes(), self.at);
        self.at = bytes[start + 1..].iter().position(|byte| special(*byte)).map_or(bytes.len(), |found| start + 1 + found);
        let mut words = &self.text[start..self.at];
        if matches!(bytes.get(self.at), Some(b'\n' | b'\r')) {
            words = words.trim_end_matches([' ', '\t']);
        }
        self.make(Span::Text, words.to_string())
    }

    /// A line ending: hard after two spaces, else soft. Leading spaces of the next line are
    /// dropped.
    fn line_end(&mut self) -> u32 {
        let (bytes, end) = (self.text.as_bytes(), self.at);
        self.at += if bytes[end..].starts_with(b"\r\n") { 2 } else { 1 };
        self.at += bytes[self.at..].iter().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
        let hard = end >= 2 && bytes[end - 1] == b' ' && bytes[end - 2] == b' ';
        self.make(if hard { Span::Hard } else { Span::Soft }, String::new())
    }

    /// A code span between equal backtick runs; unmatched, the run is text.
    fn backticks(&mut self) -> u32 {
        let (bytes, start) = (self.text.as_bytes(), self.at);
        let ticks = bytes[start..].iter().take_while(|byte| **byte == b'`').count();
        let Some(close) = closing_ticks(bytes, start + ticks, ticks) else {
            self.at = start + ticks;
            return self.make(Span::Text, self.text[start..self.at].to_string());
        };
        self.at = close + ticks;
        self.make(Span::Code, code_words(&self.text[start + ticks..close]))
    }

    /// A backslash escapes a following punctuation character, and before a line ending makes a
    /// hard break.
    fn backslash(&mut self) -> u32 {
        let next = self.text.as_bytes().get(self.at + 1).copied().unwrap_or(0);
        if next.is_ascii_punctuation() {
            self.at += 2;
            return self.make(Span::Text, (next as char).to_string());
        }
        if matches!(next, b'\n' | b'\r') {
            self.at += 1 + if self.text[self.at + 1..].starts_with("\r\n") { 2 } else { 1 };
            return self.make(Span::Hard, String::new());
        }
        self.at += 1;
        self.make(Span::Text, "\\".to_string())
    }

    /// An entity or numeric character reference, or else an `&`.
    fn entity(&mut self) -> u32 {
        let (letter, used) = character(&self.text[self.at..]).unwrap_or_else(|| ("&".to_string(), 1));
        self.at += used;
        self.make(Span::Text, letter)
    }

    /// An autolink in angle brackets, inline HTML, or else a `<`.
    fn pointy(&mut self) -> u32 {
        let bytes = self.text.as_bytes();
        if let Some((length, email)) = autolink(bytes, self.at + 1) {
            let address = character_words(&self.text[self.at + 1..self.at + 1 + length]);
            self.at += length + 2;
            let link = self.make(Span::Link, if email { format!("mailto:{address}") } else { address.clone() });
            let words = self.make(Span::Text, address);
            append(self.nodes, link, words);
            return link;
        }
        if let Some(length) = html_tag(bytes, self.at + 1) {
            let html = self.text[self.at..self.at + 1 + length].to_string();
            self.at += 1 + length;
            return self.make(Span::Html, html);
        }
        self.at += 1;
        self.make(Span::Text, "<".to_string())
    }

    /// A run of `*`, `_` or `~`: text, pushed on the delimiter stack if it may open or close
    /// emphasis (or, for one or two `~`, strikethrough).
    fn delimiter(&mut self, mark: u8) -> u32 {
        let start = self.at;
        let count = self.text.as_bytes()[start..].iter().take_while(|byte| **byte == mark).count();
        self.at += count;
        let (before, after) = (self.text[..start].chars().next_back().unwrap_or('\n'), self.text[self.at..].chars().next().unwrap_or('\n'));
        let left = !after.is_whitespace() && (!punctuation(after) || before.is_whitespace() || punctuation(before));
        let right = !before.is_whitespace() && (!punctuation(before) || after.is_whitespace() || punctuation(after));
        let (opens, closes) = match mark {
            b'_' => (left && (!right || punctuation(before)), right && (!left || punctuation(after))),
            _ => (left, right),
        };
        let inline = self.make(Span::Text, self.text[start..self.at].to_string());
        if (opens || closes) && (mark != b'~' || count <= 2) {
            self.delimiters.push(Delimiter { inline, mark, length: count, opens, closes, prev: self.last, next: NO });
            let new = self.delimiters.len() - 1;
            if let Some(last) = self.delimiters.get_mut(self.last) {
                last.next = new;
            }
            self.last = new;
        }
        inline
    }

    /// A `[` or `![`: text, pushed on the bracket stack.
    fn bracket(&mut self, image: bool) -> u32 {
        let width = 1 + image as usize;
        let inline = self.make(Span::Text, self.text[self.at..self.at + width].to_string());
        self.at += width;
        if let Some(last) = self.brackets.last_mut() {
            last.after = true;
        }
        self.brackets.push(Bracket { inline, image, active: true, position: self.at, delimiters: self.delimiters.len(), after: false });
        inline
    }

    /// A `]`: closes a link or image if its bracket starts one and a destination follows (or is
    /// defined); else text.
    fn close_bracket(&mut self) -> Option<u32> {
        self.at += 1;
        let after_text = self.at;
        let Some(&Bracket { inline: opener, image, active, position, delimiters, after }) = self.brackets.last() else { return Some(self.make(Span::Text, "]".to_string())) };
        let found = if active { self.inline_link(after_text).or_else(|| self.reference(position, after_text, after)) } else { None };
        let Some((url, title, end)) = found else {
            self.brackets.pop();
            self.at = after_text;
            return Some(self.make(Span::Text, "]".to_string()));
        };
        self.at = end;
        let link = self.make(if image { Span::Image } else { Span::Link }, url);
        self.nodes[link as usize].title = title;
        insert(self.nodes, opener, link, false);
        let mut moving = self.nodes[opener as usize].next;
        while moving != NONE {
            let next = self.nodes[moving as usize].next;
            append(self.nodes, link, moving);
            moving = next;
        }
        unlink(self.nodes, opener);
        self.emphasis(delimiters);
        self.brackets.pop();
        // (Links do not nest: earlier brackets can no longer start one.)
        if !image {
            self.brackets.iter_mut().filter(|bracket| !bracket.image).for_each(|bracket| bracket.active = false);
        }
        None
    }

    /// An inline link's destination after its text: `(destination "title")`. It, its title, and
    /// where it ends.
    fn inline_link(&self, at: usize) -> Option<(String, String, usize)> {
        let bytes = self.text.as_bytes();
        if bytes.get(at) != Some(&b'(') {
            return None;
        }
        let start = at + 1 + whitespace(bytes, at + 1);
        let (url, used) = link_destination(self.text, start)?;
        let before_title = start + used;
        let title_at = before_title + whitespace(bytes, before_title);
        let title = if title_at == before_title { 0 } else { link_title(bytes, title_at) };
        let end = title_at + title + whitespace(bytes, title_at + title);
        (bytes.get(end) == Some(&b')')).then(|| (clean_url(url), clean_title(&self.text[title_at..title_at + title]), end + 1))
    }

    /// A reference link's destination: the label in brackets after its text, or the text itself
    /// (`[text][]`, `[text]`).
    fn reference(&self, position: usize, after_text: usize, bracket_after: bool) -> Option<(String, String, usize)> {
        let (label, end) = match link_label(self.text, after_text) {
            Some((label, end)) if !label.is_empty() => (label, end),
            Some((_, end)) if !bracket_after => (&self.text[position..after_text - 1], end),
            None if !bracket_after => (&self.text[position..after_text - 1], after_text),
            _ => return None,
        };
        let (url, title) = self.refs.get(&label_key(label)).filter(|_| label.len() <= LABEL)?;
        Some((url.clone(), title.clone(), end))
    }

    /// Matches delimiters from `bottom` on, closers with earlier openers, into emphasis, strong
    /// emphasis and strikethrough, per the spec's appendix.
    fn emphasis(&mut self, bottom: usize) {
        // How far back an opener for each closer type is still worth seeking (openers_bottom).
        let mut floors = [bottom; 14];
        let mut closer = self.last;
        while closer != NO && self.delimiters[closer].prev != NO && self.delimiters[closer].prev >= bottom {
            closer = self.delimiters[closer].prev;
        }
        closer = if closer != NO && closer >= bottom { closer } else { NO };
        while closer != NO {
            let Delimiter { mark, length, opens, closes, next, .. } = self.delimiters[closer];
            if !closes {
                closer = next;
                continue;
            }
            let floor = match mark {
                b'~' => 12 + opens as usize,
                b'_' => opens as usize * 3 + length % 3,
                _ => 6 + opens as usize * 3 + length % 3,
            };
            match self.opener(closer, floors[floor]) {
                Some(opener) => closer = self.emphasize(opener, closer),
                None => (floors[floor], closer) = (closer, self.pass_over(closer, opens, next)),
            }
        }
        while self.last != NO && self.last >= bottom {
            self.remove(self.last);
        }
    }

    /// Leaves a closer that closes nothing, removing it unless it may still open. Returns the
    /// next delimiter.
    fn pass_over(&mut self, closer: usize, opens: bool, next: usize) -> usize {
        if !opens {
            self.remove(closer);
        }
        next
    }

    /// The nearest delimiter before `closer`, not before `floor`, that it closes.
    fn opener(&self, closer: usize, floor: usize) -> Option<usize> {
        let Delimiter { mark, length, opens, inline, .. } = self.delimiters[closer];
        let mut opener = self.delimiters[closer].prev;
        while opener != NO && opener >= floor {
            let before = &self.delimiters[opener];
            // (The rule of three: when either run can both open and close, their lengths must
            // not sum to a multiple of three unless both are multiples of three.)
            let threes = !(opens || before.closes) || length % 3 == 0 || !(before.length + length).is_multiple_of(3);
            let tildes = mark != b'~' || self.nodes[before.inline as usize].text.len() == self.nodes[inline as usize].text.len();
            if before.opens && before.mark == mark && threes && tildes {
                return Some(opener);
            }
            opener = before.prev;
        }
        None
    }

    /// Wraps what lies between an opener and its closer in emphasis (one marker each), strong
    /// emphasis (two) or strikethrough; removes the used markers from both; returns the closer
    /// to continue with.
    fn emphasize(&mut self, opener: usize, closer: usize) -> usize {
        let (open, close, mark) = (self.delimiters[opener].inline, self.delimiters[closer].inline, self.delimiters[opener].mark);
        let (have_open, have_close) = (self.nodes[open as usize].text.len(), self.nodes[close as usize].text.len());
        let used = if mark == b'~' {
            have_close
        } else if have_open >= 2 && have_close >= 2 {
            2
        } else {
            1
        };
        self.nodes[open as usize].text.truncate(have_open - used);
        self.nodes[close as usize].text.truncate(have_close - used);
        let mut between = self.delimiters[closer].prev;
        while between != opener && between != NO {
            let before = self.delimiters[between].prev;
            self.remove(between);
            between = before;
        }
        let kind = match (mark, used) {
            (b'~', _) => Span::Struck,
            (_, 1) => Span::Emphasis,
            _ => Span::Strong,
        };
        let emphasis = self.make(kind, String::new());
        let mut moving = self.nodes[open as usize].next;
        while moving != close && moving != NONE {
            let next = self.nodes[moving as usize].next;
            append(self.nodes, emphasis, moving);
            moving = next;
        }
        insert(self.nodes, open, emphasis, true);
        if have_open == used {
            unlink(self.nodes, open);
            self.remove(opener);
        }
        if have_close > used {
            return closer;
        }
        let next = self.delimiters[closer].next;
        unlink(self.nodes, close);
        self.remove(closer);
        next
    }

    /// Removes a delimiter from the stack.
    fn remove(&mut self, at: usize) {
        let Delimiter { prev, next, .. } = self.delimiters[at];
        if let Some(before) = self.delimiters.get_mut(prev) {
            before.next = next;
        }
        match self.delimiters.get_mut(next) {
            Some(after) => after.prev = prev,
            None => self.last = prev,
        }
    }
}

/// Where a closing run of exactly `ticks` backticks starts, from `from`.
fn closing_ticks(bytes: &[u8], from: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        let run = bytes[at..].iter().take_while(|byte| **byte == b'`').count();
        if run == ticks {
            return Some(at);
        }
        at += run.max(1);
    }
    None
}

/// A code span's text: line endings become spaces, and one space is stripped from each end
/// where both have one and it is not all spaces.
fn code_words(code: &str) -> String {
    let words = code.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let padded = words.len() >= 2 && words.starts_with(' ') && words.ends_with(' ') && words.bytes().any(|byte| byte != b' ');
    if padded { words[1..words.len() - 1].to_string() } else { words }
}

/// An autolink after a `<` at `at`: a scheme, a colon and no spaces, or an email address; then
/// `>`. Its length up to the `>`, and whether it is an email address.
fn autolink(bytes: &[u8], at: usize) -> Option<(usize, bool)> {
    let rest = &bytes[at..];
    let scheme = rest.iter().take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')).count();
    if (2..=32).contains(&scheme) && rest[0].is_ascii_alphabetic() && rest.get(scheme) == Some(&b':') {
        let end = scheme + 1 + rest[scheme + 1..].iter().take_while(|byte| **byte > b' ' && !matches!(byte, b'<' | b'>')).count();
        return (rest.get(end) == Some(&b'>')).then_some((end, false));
    }
    let local = rest.iter().take_while(|byte| byte.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(byte)).count();
    if local == 0 || rest.get(local) != Some(&b'@') {
        return None;
    }
    let mut end = local + 1;
    loop {
        let label = rest[end..].iter().take(64).take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-').count();
        if label == 0 || label > 63 || rest[end] == b'-' || rest[end + label - 1] == b'-' {
            return None;
        }
        end += label;
        match rest.get(end) {
            Some(b'.') => end += 1,
            Some(b'>') => return Some((end, true)),
            _ => return None,
        }
    }
}

/// The length of spaces, tabs and line endings at `at`.
fn whitespace(bytes: &[u8], at: usize) -> usize {
    bytes.get(at..).unwrap_or_default().iter().take_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).count()
}

/// A link label at `at`: the text between `[` and `]` (no unescaped bracket, not too long),
/// trimmed; and where it ends.
fn link_label(text: &str, at: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let mut end = at + 1;
    while end < bytes.len() && end - at <= LABEL + 1 {
        match bytes[end] {
            b'[' => return None,
            b']' => return Some((text[at + 1..end].trim_matches([' ', '\t', '\n', '\r']), end + 1)),
            b'\\' if bytes.get(end + 1).is_some_and(u8::is_ascii_punctuation) => end += 2,
            _ => end += 1,
        }
    }
    None
}

/// A link destination at `at`: in angle brackets, or a run without spaces and with balanced
/// parentheses. Its text, and length.
fn link_destination(text: &str, at: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(at) == Some(&b'<') {
        let mut end = at + 1;
        while end < bytes.len() && !matches!(bytes[end], b'>' | b'<' | b'\n' | b'\r') {
            end += if bytes[end] == b'\\' && end + 1 < bytes.len() { 2 } else { 1 };
        }
        return (bytes.get(end) == Some(&b'>')).then(|| (&text[at + 1..end], end + 1 - at));
    }
    let (mut end, mut open) = (at, 0);
    while end < bytes.len() {
        match bytes[end] {
            b'\\' if bytes.get(end + 1).is_some_and(u8::is_ascii_punctuation) => end += 1,
            b'(' if open == 32 => return None,
            b'(' => open += 1,
            b')' if open == 0 => break,
            b')' => open -= 1,
            byte if byte <= b' ' || byte == 0x7f => break,
            _ => {}
        }
        end += 1;
    }
    (open == 0 && (end > at || bytes.get(end) == Some(&b')'))).then(|| (&text[at..end], end - at))
}

/// A link title at `at`: in double quotes, single quotes or parentheses, with that delimiter
/// escaped inside. Its length with delimiters; 0 for none.
fn link_title(bytes: &[u8], at: usize) -> usize {
    let close = match bytes.get(at) {
        Some(b'"') => b'"',
        Some(b'\'') => b'\'',
        Some(b'(') => b')',
        _ => return 0,
    };
    let mut end = at + 1;
    while end < bytes.len() && bytes[end] != close {
        if close == b')' && bytes[end] == b'(' {
            return 0;
        }
        end += if bytes[end] == b'\\' && end + 1 < bytes.len() { 2 } else { 1 };
    }
    if end < bytes.len() { end + 1 - at } else { 0 }
}

/// A link destination as used: trimmed, with entities and escapes decoded.
fn clean_url(url: &str) -> String {
    unescape(url.trim_matches([' ', '\t', '\n', '\r']))
}

/// A link title without delimiters, entities and escapes decoded.
fn clean_title(title: &str) -> String {
    if title.len() < 2 { String::new() } else { unescape(&title[1..title.len() - 1]) }
}

/// The text with escapes and entities decoded.
fn unescape(text: &str) -> String {
    let (bytes, mut at, mut out) = (text.as_bytes(), 0, String::with_capacity(text.len()));
    while at < bytes.len() {
        let next = bytes.get(at + 1).copied().unwrap_or(0);
        if bytes[at] == b'\\' && next.is_ascii_punctuation() {
            out.push(next as char);
            at += 2;
            continue;
        }
        if let Some((letter, used)) = character(&text[at..]).filter(|_| bytes[at] == b'&') {
            out.push_str(&letter);
            at += used;
            continue;
        }
        let wide = text[at..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[at..at + wide]);
        at += wide;
    }
    out
}

/// The text with entities decoded (escapes kept).
fn character_words(text: &str) -> String {
    let (mut at, mut out) = (0, String::with_capacity(text.len()));
    while let Some(amp) = text[at..].find('&') {
        out.push_str(&text[at..at + amp]);
        let (letter, used) = character(&text[at + amp..]).unwrap_or_else(|| ("&".to_string(), 1));
        out.push_str(&letter);
        at += amp + used;
    }
    out.push_str(&text[at..]);
    out
}

/// A link reference definition at the start of a paragraph: `[label]: destination "title"`, on
/// lines of its own. Added to the definitions (the first per label wins); returns its length.
fn definition(text: &str, refs: &mut HashMap<String, (String, String)>) -> Option<usize> {
    let bytes = text.as_bytes();
    let (label, after) = link_label(text, 0)?;
    if label.is_empty() || bytes.get(after) != Some(&b':') {
        return None;
    }
    let start = line_space(bytes, after + 1);
    let (url, used) = link_destination(text, start)?;
    let before_title = start + used;
    // (An empty destination is allowed only as `<>`.)
    if used == 0 || bytes[start] != b'<' && url.is_empty() {
        return None;
    }
    let title_at = line_space(bytes, before_title);
    let title = if title_at == before_title { 0 } else { link_title(bytes, title_at) };
    let spaces = |at: usize| at + bytes[at..].iter().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
    // A title followed by more on its line is no title: the definition then ends after the
    // destination, if nothing follows that on its line.
    let (title, end) = match title {
        0 => (0, spaces(before_title)),
        _ if ends_line(bytes, spaces(title_at + title)) => (title, spaces(title_at + title)),
        _ => (0, spaces(before_title)),
    };
    if !ends_line(bytes, end) {
        return None;
    }
    let past = end + if bytes[end..].starts_with(b"\r\n") { 2 } else { (end < bytes.len()) as usize };
    let key = label_key(label);
    if !key.is_empty() {
        let title = if title > 0 { clean_title(&text[title_at..title_at + title]) } else { String::new() };
        refs.entry(key).or_insert((clean_url(url), title));
    }
    Some(past)
}

/// Past spaces, at most one line ending, and spaces.
fn line_space(bytes: &[u8], at: usize) -> usize {
    let spaces = |at: usize| at + bytes[at..].iter().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
    let at = spaces(at);
    spaces(at + if bytes[at..].starts_with(b"\r\n") { 2 } else { matches!(bytes.get(at), Some(b'\n' | b'\r')) as usize })
}

/// Whether a line, or the text, ends at `at`.
fn ends_line(bytes: &[u8], at: usize) -> bool {
    matches!(bytes.get(at), None | Some(b'\n' | b'\r'))
}

/// A label as looked up: case-folded, whitespace runs collapsed.
fn label_key(label: &str) -> String {
    let folded = label.to_lowercase().replace('ß', "ss");
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The entity names from U+00A0 on, in order.
const LATIN: &str = "nbsp iexcl cent pound curren yen brvbar sect uml copy ordf laquo not shy reg macr deg plusmn sup2 sup3 acute micro para middot cedil sup1 ordm raquo frac14 frac12 frac34 \
    iquest Agrave Aacute Acirc Atilde Auml Aring AElig Ccedil Egrave Eacute Ecirc Euml Igrave Iacute Icirc Iuml ETH Ntilde Ograve Oacute Ocirc Otilde Ouml times Oslash Ugrave Uacute \
    Ucirc Uuml Yacute THORN szlig agrave aacute acirc atilde auml aring aelig ccedil egrave eacute ecirc euml igrave iacute icirc iuml eth ntilde ograve oacute ocirc otilde ouml divide \
    oslash ugrave uacute ucirc uuml yacute thorn yuml";
/// The Greek letter names from U+0391 on, and from U+03B1 on.
const GREEK: [&str; 2] = [
    "Alpha Beta Gamma Delta Epsilon Zeta Eta Theta Iota Kappa Lambda Mu Nu Xi Omicron Pi Rho - Sigma Tau Upsilon Phi Chi Psi Omega",
    "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigmaf sigma tau upsilon phi chi psi omega",
];
/// HTML 4's other entity names (and `apos`), each with its code point.
const NAMED: &str = "quot 22 amp 26 apos 27 lt 3c gt 3e OElig 152 oelig 153 Scaron 160 scaron 161 Yuml 178 fnof 192 circ 2c6 tilde 2dc thetasym 3d1 upsih 3d2 piv 3d6 \
    ensp 2002 emsp 2003 thinsp 2009 zwnj 200c zwj 200d lrm 200e rlm 200f ndash 2013 mdash 2014 lsquo 2018 rsquo 2019 sbquo 201a ldquo 201c rdquo 201d bdquo 201e \
    dagger 2020 Dagger 2021 bull 2022 hellip 2026 permil 2030 prime 2032 Prime 2033 lsaquo 2039 rsaquo 203a oline 203e frasl 2044 euro 20ac image 2111 weierp 2118 \
    real 211c trade 2122 alefsym 2135 larr 2190 uarr 2191 rarr 2192 darr 2193 harr 2194 crarr 21b5 lArr 21d0 uArr 21d1 rArr 21d2 dArr 21d3 hArr 21d4 forall 2200 \
    part 2202 exist 2203 empty 2205 nabla 2207 isin 2208 notin 2209 ni 220b prod 220f sum 2211 minus 2212 lowast 2217 radic 221a prop 221d infin 221e ang 2220 \
    and 2227 or 2228 cap 2229 cup 222a int 222b there4 2234 sim 223c cong 2245 asymp 2248 ne 2260 equiv 2261 le 2264 ge 2265 sub 2282 sup 2283 nsub 2284 \
    sube 2286 supe 2287 oplus 2295 otimes 2297 perp 22a5 sdot 22c5 lceil 2308 rceil 2309 lfloor 230a rfloor 230b lang 27e8 rang 27e9 loz 25ca spades 2660 \
    clubs 2663 hearts 2665 diams 2666";

/// The character an entity name stands for, of those above.
fn named(name: &str) -> Option<char> {
    let in_list = |list: &str, first: u32| list.split_whitespace().position(|one| one == name).and_then(|at| char::from_u32(first + at as u32));
    let mut pairs = NAMED.split_whitespace();
    let other = std::iter::from_fn(|| Some((pairs.next()?, pairs.next()?))).find(|(one, _)| *one == name);
    let numbered = other.and_then(|(_, number)| char::from_u32(u32::from_str_radix(number, 16).ok()?));
    in_list(LATIN, 0xa0).or_else(|| in_list(GREEK[0], 0x391)).or_else(|| in_list(GREEK[1], 0x3b1)).or(numbered)
}

/// An entity (`&copy;`) or numeric reference (`&#169;`, `&#xA9;`) at the start of the text: its
/// character, and the reference's length.
fn character(text: &str) -> Option<(String, usize)> {
    let end = text.as_bytes().iter().take(34).position(|byte| *byte == b';')?;
    let name = text.get(1..end)?;
    let letter = match name.strip_prefix('#') {
        Some(number) => {
            let (figures, radix, most) = number.strip_prefix(['x', 'X']).map_or((number, 10, 7), |hex| (hex, 16, 6));
            let all = !figures.is_empty() && figures.len() <= most && figures.chars().all(|figure| figure.is_digit(radix));
            let code = u32::from_str_radix(figures, radix).ok().filter(|_| all)?;
            char::from_u32(code).filter(|letter| *letter != '\0').unwrap_or('\u{fffd}')
        }
        None if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric()) => named(name)?,
        None => return None,
    };
    Some((letter.to_string(), end + 1))
}

// ---- the document ----

/// A parsed text: its blocks in document order, and its text blocks' inlines.
struct Read {
    blocks: Vec<Block>,
    nodes: Vec<Inline>,
}

/// Parses a text: blocks, then each text block's inlines.
fn read(text: &str) -> Read {
    let blank_lines = text.lines().filter(|line| line.bytes().all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))).count();
    let mut reader = Reader::new(blank_lines);
    for line in lines(text) {
        reader.read_line(line);
    }
    reader.close_to(0);
    reader.finish();
    let Reader { mut blocks, refs, .. } = reader;
    let mut nodes = Vec::new();
    for block in blocks.iter_mut().filter(|block| matches!(block.kind, Kind::Paragraph | Kind::Heading { .. } | Kind::Cell(_))) {
        let words = block.content.trim_matches([' ', '\t', '\n', '\r']);
        nodes.push(Inline { kind: Span::Root, text: String::new(), title: String::new(), parent: NONE, first: NONE, last: NONE, prev: NONE, next: NONE });
        block.inlines = nodes.len() as u32 - 1;
        let mut inlines = Inlines { text: words, at: 0, nodes: &mut nodes, refs: &refs, delimiters: Vec::new(), last: NO, brackets: Vec::new() };
        inlines.read(block.inlines);
    }
    Read { blocks, nodes }
}

/// A Markdown document, parsed once and kept by the app, to show with
/// [`Ui::markdown`](crate::Ui::markdown). (What it supports: see the top of this file, or the
/// guide.)
#[derive(Clone, Debug, PartialEq)]
pub struct Markdown {
    pub(crate) writing: Writing,
    /// Each link's destination by index, and its title.
    pub(crate) links: Vec<(String, String)>,
    /// The image names it shows, each once; and the pictures the app supplied for them, by
    /// index.
    pub(crate) names: Vec<String>,
    pub(crate) given: Vec<Option<Picture>>,
    /// Code syntax rules supplied by the app, by language name.
    pub(crate) rules: Vec<(String, &'static Syntax)>,
    /// A unique id for this parse, so its layout can be cached. (Supplying a picture or rules
    /// gives a new one.)
    pub(crate) serial: u64,
}

/// A parse id never used before.
fn serial() -> u64 {
    static READINGS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    READINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Markdown {
    /// Parses Markdown text.
    pub fn parse(text: &str) -> Markdown {
        let markdown = Markdown { writing: Writing::default(), links: Vec::new(), names: Vec::new(), given: Vec::new(), rules: Vec::new(), serial: serial() };
        let mut writer = Writer { markdown, read: read(text) };
        writer.write();
        writer.markdown
    }
    /// The image names it shows (`![alt](name)`), each once, in order: for the app to supply a
    /// picture for each it has ([`set_picture`](Self::set_picture)).
    pub fn pictures(&self) -> &[String] {
        &self.names
    }
    /// Supplies the picture the document names `name` (one of `pictures`): shown in place at
    /// its own size, or smaller where the document is narrower. (Until supplied, its alt text
    /// shows there.)
    pub fn set_picture(&mut self, name: &str, picture: Picture) {
        let Some(at) = self.names.iter().position(|known| known == name) else { return };
        self.given.resize(self.names.len(), None);
        (self.given[at], self.serial) = (Some(picture), serial());
    }
    /// Highlights code blocks that name `language` (````
    /// ```toml ````) with `rules`: an app's own language. (Rust, C and C++, Python, JavaScript, TypeScript and JSON are highlighted by their usual names already.)
    pub fn set_syntax(&mut self, language: &str, rules: &'static Syntax) {
        self.rules.retain(|(known, _)| !known.eq_ignore_ascii_case(language));
        self.rules.push((language.to_string(), rules));
        self.serial = serial();
    }
    /// Its text as shown, a block per line: what copying all of it gives.
    pub fn text(&self) -> &str {
        &self.writing.words
    }
}

/// Turns the parse into the displayed document.
struct Writer {
    markdown: Markdown,
    read: Read,
}

impl Writer {
    /// Every block in order, with its text.
    fn write(&mut self) {
        // Each block's index among the displayed ones.
        let mut shown = vec![NONE; self.read.blocks.len()];
        for at in 0..self.read.blocks.len() {
            let Some(kind) = self.kind(at) else { continue };
            let parent = self.read.blocks[at].parent;
            let parent = if parent == NONE { NONE } else { shown[parent as usize] };
            let (words, pieces) = (self.markdown.writing.words.len() as u32, self.markdown.writing.pieces.len() as u32);
            self.markdown.writing.blocks.push(rich_text::Block { kind, parent, end: 0, pieces: [pieces; 2], words: [words; 2] });
            shown[at] = self.markdown.writing.blocks.len() as u32 - 1;
            self.words(at);
        }
        rich_text::finish(&mut self.markdown.writing);
    }

    /// What a parsed block is shown as; nothing for an empty one.
    fn kind(&mut self, at: usize) -> Option<rich_text::Kind> {
        let (kind, ticked) = (self.read.blocks[at].kind, self.read.blocks[at].ticked);
        Some(match kind {
            Kind::Document => rich_text::Kind::Page,
            Kind::Quote => rich_text::Kind::Quote,
            Kind::List { numbered, tight, .. } => rich_text::Kind::List { numbered, tight },
            Kind::Item { number, .. } => rich_text::Kind::Item { number, ticked },
            Kind::Paragraph => rich_text::Kind::Paragraph,
            Kind::Heading { level, .. } => rich_text::Kind::Heading(level),
            Kind::Code { .. } => rich_text::Kind::Code(self.language(at)),
            Kind::Html(_) => rich_text::Kind::Code(0),
            Kind::Rule => rich_text::Kind::Rule,
            Kind::Table => rich_text::Kind::Table,
            Kind::Row { head } => rich_text::Kind::Row { head },
            Kind::Cell(side) => rich_text::Kind::Cell(side),
            Kind::Gone => return None,
        })
    }

    /// The index of the language a code block names (by the first word of its info string) in
    /// the document's list, from 1; 0 for none.
    fn language(&mut self, at: usize) -> u32 {
        let Some(name) = self.read.blocks[at].info.split_whitespace().next() else { return 0 };
        let languages = &mut self.markdown.writing.languages;
        let known = languages.iter().position(|known| known == name);
        known.unwrap_or_else(|| (languages.push(name.to_string()), languages.len() - 1).1) as u32 + 1
    }

    /// A block's text, and a line ending after it.
    fn words(&mut self, at: usize) {
        let block = &self.read.blocks[at];
        match block.kind {
            Kind::Code { .. } | Kind::Html(_) => {
                let code = tabs_to_spaces(block.content.strip_suffix('\n').unwrap_or(&block.content));
                self.add(&code, TextStyle { mono: true, ..TextStyle::default() }, Thing::Words);
            }
            _ if block.inlines != NONE => self.inlines(block.inlines),
            _ => return,
        }
        let writing = &mut self.markdown.writing;
        if let Some(block) = writing.blocks.last_mut() {
            block.words[1] = writing.words.len() as u32;
            block.pieces[1] = writing.pieces.len() as u32;
        }
        writing.words.push('\n');
    }

    /// Adds text in a style: merged into the last piece if that has the same style.
    fn add(&mut self, words: &str, look: TextStyle, thing: Thing) {
        let writing = &mut self.markdown.writing;
        let start = writing.words.len() as u32;
        writing.words.push_str(words);
        let end = writing.words.len() as u32;
        let block_start = writing.blocks.last().map_or(0, |block| block.pieces[0]) as usize;
        let joins = writing.pieces.len() > block_start && thing == Thing::Words;
        match writing.pieces.last_mut().filter(|last| joins && last.look == look && last.thing == Thing::Words && last.words[1] == start) {
            Some(last) => last.words[1] = end,
            None => writing.pieces.push(Stretch { words: [start, end], look, thing }),
        }
    }

    /// The inlines under `root` in order, each in its container's style.
    fn inlines(&mut self, root: u32) {
        // The containers of the current inline, innermost last, each with the style it gives
        // its content.
        let mut open: Vec<(u32, TextStyle)> = vec![(root, TextStyle::default())];
        let mut at = self.read.nodes[root as usize].first;
        while let Some(&(_, look)) = open.last() {
            if at == NONE {
                let (closed, _) = open.pop().unwrap_or((root, look));
                at = if open.is_empty() { NONE } else { self.read.nodes[closed as usize].next };
                continue;
            }
            let first = self.read.nodes[at as usize].first;
            let holder = self.inline(at, look).filter(|_| first != NONE);
            if let Some(inner) = holder {
                open.push((at, inner));
            }
            at = if holder.is_some() { first } else { self.read.nodes[at as usize].next };
        }
    }

    /// Adds one inline's text; for a container, returns its content's style.
    fn inline(&mut self, at: u32, look: TextStyle) -> Option<TextStyle> {
        let node = &self.read.nodes[at as usize];
        let flat = node.text.replace(['\n', '\r', '\t'], " ");
        match node.kind {
            Span::Text | Span::Html => self.add(&flat, look, Thing::Words),
            Span::Soft => self.add(" ", look, Thing::Words),
            Span::Hard => self.add("\n", look, Thing::Words),
            Span::Code => self.add(&flat, TextStyle { mono: true, ..look }, Thing::Words),
            Span::Emphasis => return Some(TextStyle { italic: true, ..look }),
            Span::Strong => return Some(TextStyle { bold: true, ..look }),
            Span::Struck => return Some(TextStyle { rule: Decoration::Strikethrough, ..look }),
            Span::Link => {
                self.markdown.links.push((node.text.clone(), node.title.clone()));
                return Some(TextStyle { link: self.markdown.links.len() as u32, ..look });
            }
            Span::Image => self.picture(at, look),
            Span::Root => {}
        }
        None
    }

    /// An image: by its name, with its alt text standing in.
    fn picture(&mut self, at: u32, look: TextStyle) {
        let name = self.read.nodes[at as usize].text.clone();
        let number = match self.markdown.names.iter().position(|known| *known == name) {
            Some(number) => number,
            None => (self.markdown.names.push(name), self.markdown.names.len() - 1).1,
        };
        let shows = plain_words(&self.read.nodes, at);
        let shows = if shows.is_empty() { THING.to_string() } else { shows };
        self.add(&shows, look, Thing::Named(number as u32));
    }
}

/// All text under an inline without styles: an image's alt text.
fn plain_words(nodes: &[Inline], root: u32) -> String {
    let (mut words, mut at) = (String::new(), nodes[root as usize].first);
    while at != NONE && at != root {
        let node = &nodes[at as usize];
        match node.kind {
            Span::Text | Span::Code | Span::Html => words.push_str(&node.text.replace(['\n', '\r', '\t'], " ")),
            Span::Soft | Span::Hard => words.push(' '),
            _ => {}
        }
        at = if node.first != NONE { node.first } else { next_after(nodes, at, root) };
    }
    words
}

/// The inline after `at` and its descendants, staying within `root`.
fn next_after(nodes: &[Inline], mut at: u32, root: u32) -> u32 {
    while at != root && nodes[at as usize].next == NONE {
        at = nodes[at as usize].parent;
    }
    if at == root { NONE } else { nodes[at as usize].next }
}

/// Code tabs expanded to the next stop.
fn tabs_to_spaces(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut column = 0;
    for letter in code.chars() {
        match letter {
            '\t' => (0..TAB - column % TAB).for_each(|_| out.push(' ')),
            _ => out.push(letter),
        }
        column = if letter == '\n' {
            0
        } else if letter == '\t' {
            column + TAB - column % TAB
        } else {
            column + 1
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parse rendered as the HTML the spec's examples give (as the reference implementations
    /// do: cmark, and cmark-gfm for GitHub's extensions).
    fn html(text: &str) -> String {
        let read = read(text);
        let mut out = String::new();
        block_html(&read, 0, false, &mut out);
        out
    }

    fn line(out: &mut String) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    }

    fn escaped(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// A link destination as cmark escapes it in an attribute.
    fn href(url: &str) -> String {
        let mut out = String::new();
        for byte in url.bytes() {
            match byte {
                b'&' => out.push_str("&amp;"),
                b'\'' => out.push_str("&#x27;"),
                b'!' | b'#' | b'$' | b'%' | b'(' | b')' | b'*' | b'+' | b',' | b'-' | b'.' | b'/' | b':' | b';' | b'=' | b'?' | b'@' | b'_' | b'~' => out.push(byte as char),
                _ if byte.is_ascii_alphanumeric() => out.push(byte as char),
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }

    fn children(read: &Read, at: u32, tight: bool, out: &mut String) {
        let mut child = read.blocks[at as usize].first;
        while child != NONE {
            block_html(read, child, tight, out);
            child = read.blocks[child as usize].next;
        }
    }

    fn block_html(read: &Read, at: u32, tight: bool, out: &mut String) {
        let block = &read.blocks[at as usize];
        match block.kind {
            Kind::Document => children(read, at, false, out),
            Kind::Quote => {
                line(out);
                out.push_str("<blockquote>\n");
                children(read, at, false, out);
                line(out);
                out.push_str("</blockquote>\n");
            }
            Kind::List { numbered, start, tight, .. } => {
                line(out);
                out.push_str(&match (numbered, start) {
                    (false, _) => "<ul>\n".to_string(),
                    (true, 1) => "<ol>\n".to_string(),
                    (true, start) => format!("<ol start=\"{start}\">\n"),
                });
                children(read, at, tight, out);
                out.push_str(if numbered { "</ol>\n" } else { "</ul>\n" });
            }
            Kind::Item { .. } => {
                line(out);
                out.push_str("<li>");
                match block.ticked {
                    Some(true) => out.push_str("<input checked=\"\" disabled=\"\" type=\"checkbox\"> "),
                    Some(false) => out.push_str("<input disabled=\"\" type=\"checkbox\"> "),
                    None => {}
                }
                children(read, at, tight, out);
                out.push_str("</li>\n");
            }
            Kind::Paragraph => {
                if !tight {
                    line(out);
                    out.push_str("<p>");
                }
                inline_html(read, block.inlines, false, out);
                if !tight {
                    out.push_str("</p>\n");
                }
            }
            Kind::Heading { level, .. } => {
                line(out);
                out.push_str(&format!("<h{level}>"));
                inline_html(read, block.inlines, false, out);
                out.push_str(&format!("</h{level}>\n"));
            }
            Kind::Code { .. } => {
                line(out);
                let language = block.info.split([' ', '\t', '\n']).next().unwrap_or_default();
                match language {
                    "" => out.push_str("<pre><code>"),
                    language => out.push_str(&format!("<pre><code class=\"language-{}\">", escaped(language))),
                }
                out.push_str(&escaped(&block.content));
                out.push_str("</code></pre>\n");
            }
            Kind::Html(_) => {
                line(out);
                out.push_str(&block.content);
                line(out);
            }
            Kind::Rule => {
                line(out);
                out.push_str("<hr />\n");
            }
            Kind::Table => {
                line(out);
                out.push_str("<table>\n");
                let mut row = block.first;
                let mut body = false;
                while row != NONE {
                    let head = matches!(read.blocks[row as usize].kind, Kind::Row { head: true });
                    if head {
                        out.push_str("<thead>\n");
                    } else if !body {
                        out.push_str("<tbody>\n");
                        body = true;
                    }
                    out.push_str("<tr>\n");
                    let mut cell = read.blocks[row as usize].first;
                    while cell != NONE {
                        let tag = if head { "th" } else { "td" };
                        let side = match read.blocks[cell as usize].kind {
                            Kind::Cell(Align::Start) => " align=\"left\"",
                            Kind::Cell(Align::Center) => " align=\"center\"",
                            Kind::Cell(Align::End) => " align=\"right\"",
                            _ => "",
                        };
                        out.push_str(&format!("<{tag}{side}>"));
                        inline_html(read, read.blocks[cell as usize].inlines, false, out);
                        out.push_str(&format!("</{tag}>\n"));
                        cell = read.blocks[cell as usize].next;
                    }
                    out.push_str("</tr>\n");
                    if head {
                        out.push_str("</thead>\n");
                    }
                    row = read.blocks[row as usize].next;
                }
                if body {
                    out.push_str("</tbody>\n");
                }
                out.push_str("</table>\n");
            }
            Kind::Row { .. } | Kind::Cell(_) | Kind::Gone => {}
        }
    }

    fn inline_children(read: &Read, at: u32, plain: bool, out: &mut String) {
        let mut child = read.nodes[at as usize].first;
        while child != NONE {
            inline_html(read, child, plain, out);
            child = read.nodes[child as usize].next;
        }
    }

    fn inline_html(read: &Read, at: u32, plain: bool, out: &mut String) {
        if at == NONE {
            return;
        }
        let node = &read.nodes[at as usize];
        let wrapped = |tag: &str, out: &mut String| {
            if !plain {
                out.push_str(&format!("<{tag}>"));
            }
            inline_children(read, at, plain, out);
            if !plain {
                out.push_str(&format!("</{tag}>"));
            }
        };
        match node.kind {
            Span::Root => inline_children(read, at, plain, out),
            Span::Text => out.push_str(&escaped(&node.text)),
            Span::Soft => out.push_str(if plain { " " } else { "\n" }),
            Span::Hard => out.push_str(if plain { " " } else { "<br />\n" }),
            Span::Code if plain => out.push_str(&escaped(&node.text)),
            Span::Code => out.push_str(&format!("<code>{}</code>", escaped(&node.text))),
            Span::Html if plain => out.push_str(&escaped(&node.text)),
            Span::Html => out.push_str(&node.text),
            Span::Emphasis => wrapped("em", out),
            Span::Strong => wrapped("strong", out),
            Span::Struck => wrapped("del", out),
            Span::Link if plain => inline_children(read, at, plain, out),
            Span::Link => {
                out.push_str(&format!("<a href=\"{}\"", href(&node.text)));
                if !node.title.is_empty() {
                    out.push_str(&format!(" title=\"{}\"", escaped(&node.title)));
                }
                out.push('>');
                inline_children(read, at, plain, out);
                out.push_str("</a>");
            }
            Span::Image if plain => inline_children(read, at, plain, out),
            Span::Image => {
                out.push_str(&format!("<img src=\"{}\" alt=\"", href(&node.text)));
                inline_children(read, at, true, out);
                out.push('"');
                if !node.title.is_empty() {
                    out.push_str(&format!(" title=\"{}\"", escaped(&node.title)));
                }
                out.push_str(" />");
            }
        }
    }

    #[test]
    fn blocks_and_what_lies_in_their_lines_are_read_as_commonmark_has_them() {
        // Headings, emphasis, code, a reference link (defined below), a nested list, a quote,
        // indented code, a thematic break, and a hard break by two spaces.
        let text = "# Title\n\nSome *stressed* and **strong** words, `code` and a [link][1].\n\n1. one\n   - inner\n2. two\n\n> quoted\n\n    indented\n\n---\nend  \nline\n\n[1]: /here \"There\"\n";
        let wanted = "<h1>Title</h1>\n<p>Some <em>stressed</em> and <strong>strong</strong> words, <code>code</code> and a <a href=\"/here\" title=\"There\">link</a>.</p>\n\
            <ol>\n<li>one\n<ul>\n<li>inner</li>\n</ul>\n</li>\n<li>two</li>\n</ol>\n<blockquote>\n<p>quoted</p>\n</blockquote>\n<pre><code>indented\n</code></pre>\n<hr />\n<p>end<br />\nline</p>\n";
        assert_eq!(html(text), wanted);
    }

    #[test]
    fn githubs_tables_task_lists_and_struck_words_are_read() {
        let text = "| a | b |\n|:--|--:|\n| 1 | ~~2~~ |\n\n- [x] done\n- [ ] not\n";
        let table = "<table>\n<thead>\n<tr>\n<th align=\"left\">a</th>\n<th align=\"right\">b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td align=\"left\">1</td>\n<td align=\"right\"><del>2</del></td>\n</tr>\n</tbody>\n</table>\n";
        let tasks = "<ul>\n<li><input checked=\"\" disabled=\"\" type=\"checkbox\"> done</li>\n<li><input disabled=\"\" type=\"checkbox\"> not</li>\n</ul>\n";
        assert_eq!(html(text), format!("{table}{tasks}"));
    }

    #[test]
    fn what_is_not_read_is_kept_as_the_text_it_is() {
        // HTML is recognized and kept literal; so is an entity only HTML 5 has.
        let doc = Markdown::parse("<b>bold</b> &copy; &HilbertSpace;\n\n<div>\nblock\n</div>\n");
        assert_eq!(doc.text(), "<b>bold</b> \u{a9} &HilbertSpace;\n<div>\nblock\n</div>\n");
    }

    #[test]
    fn a_document_is_its_blocks_and_their_words_in_pieces_of_one_look() {
        let doc = Markdown::parse("## Head\n\nPlain *slanted* `code` [to](u \"t\") ![a cat](cat.png)\n\n- one\n- two\n");
        let writing = &doc.writing;
        assert_eq!(writing.words, "Head\nPlain slanted code to a cat\none\ntwo\n");
        let kinds: Vec<rich_text::Kind> = writing.blocks.iter().map(|block| block.kind).collect();
        let (paragraph, item) = (rich_text::Kind::Paragraph, rich_text::Kind::Item { number: 0, ticked: None });
        assert_eq!(kinds, [rich_text::Kind::Page, rich_text::Kind::Heading(2), paragraph, rich_text::Kind::List { numbered: false, tight: true }, item, paragraph, item, paragraph]);
        // Each block ends after its content; a container has all its children's text.
        let ends: Vec<u32> = writing.blocks.iter().map(|block| block.end).collect();
        assert_eq!(ends, [8, 2, 3, 8, 6, 6, 8, 8]);
        assert_eq!([writing.blocks[3].words, writing.blocks[4].words, writing.blocks[0].words], [[33, 40], [33, 36], [0, 40]]);
        // The paragraph's pieces, each in one style: plain, emphasis, code, a link, an image.
        let [from, to] = writing.blocks[2].pieces;
        let looks: Vec<(&str, bool, bool, u32, bool)> = writing.pieces[from as usize..to as usize]
            .iter()
            .map(|piece| (&writing.words[piece.words[0] as usize..piece.words[1] as usize], piece.look.italic, piece.look.mono, piece.look.link, piece.thing != Thing::Words))
            .collect();
        let plain = |words| (words, false, false, 0, false);
        assert_eq!(
            looks,
            [
                plain("Plain "),
                ("slanted", true, false, 0, false),
                plain(" "),
                ("code", false, true, 0, false),
                plain(" "),
                ("to", false, false, 1, false),
                plain(" "),
                ("a cat", false, false, 0, true)
            ]
        );
        assert_eq!((doc.links.as_slice(), doc.pictures()), (&[("u".to_string(), "t".to_string())][..], &["cat.png".to_string()][..]));
    }

    #[test]
    fn texts_made_to_be_hard_are_read_in_time() {
        // Inputs the reference implementations test for pathological slowness: deep nesting,
        // long runs of openers that close nothing.
        let started = std::time::Instant::now();
        for text in ["*a ".repeat(3000), "[".repeat(3000), "> ".repeat(500) + "a", "- ".repeat(300) + "a", "`".repeat(3000), "a**b".repeat(2000), "[a](".repeat(2000)] {
            let _ = Markdown::parse(&text);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }

    /// How deep a parsed text's blocks nest, and its innermost text.
    fn deepest(text: &str) -> (u32, String) {
        let doc = Markdown::parse(text);
        let depth = |mut at: usize| std::iter::from_fn(|| (at != NONE as usize).then(|| at = doc.writing.blocks[at].parent as usize)).count() as u32 - 1;
        let last = doc.writing.blocks.len() - 1;
        let [from, to] = doc.writing.blocks[last].words;
        (depth(last), doc.writing.words[from as usize..to as usize].to_string())
    }

    #[test]
    fn blocks_nest_as_deep_as_written() {
        // Five thousand lists, each in an item of the one before; and as many quotes.
        assert_eq!(deepest(&("- ".repeat(5000) + "x")), (10_001, "x".into()));
        assert_eq!(deepest(&("> ".repeat(5000) + "x")), (5001, "x".into()));
        // Continued on the next line, as deep.
        assert_eq!(deepest(&format!("{}x\n{}y", "- ".repeat(300), "  ".repeat(300))), (601, "x y".into()));
    }

    #[test]
    fn blocks_past_a_depth_are_words_where_blank_lines_would_cost_too_much() {
        // Each blank line walks the open items: with a hundred thousand of them, blocks nest a
        // hundred deep (fifty lists in items), and what would open more is the innermost
        // item's text. A quote counts one; so does a list continued on the next line.
        let blank = "\n".repeat(100_000);
        let deep = html(&("- ".repeat(52) + "x" + &blank));
        assert_eq!((deep.matches("<ul>").count(), deep.matches("<li>").count()), (50, 50));
        assert!(deep.ends_with(&format!("<li>- - x</li>\n{}", "</ul>\n</li>\n".repeat(49) + "</ul>\n")), "{deep}");
        let quoted = html(&("> ".repeat(102) + "x" + &blank));
        assert!(quoted.matches("<blockquote>").count() == 100 && quoted.contains("<p>&gt; &gt; x</p>"), "{quoted}");
        let then = html(&format!("{}x\n{}y{blank}", "- ".repeat(60), "  ".repeat(50)));
        assert!(then.matches("<ul>").count() == 50 && then.contains("<li>- - - - - - - - - - x\ny</li>"), "{then}");
        // Blank lines in a deep list cost no more than that.
        let started = std::time::Instant::now();
        let _ = Markdown::parse(&format!("{}x\n{}", "- ".repeat(20_000), "\n".repeat(20_000)));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    // ---- the specification's examples ----

    /// The strings of the spec's examples in its JSON: each one's Markdown, HTML and number. (A
    /// reader for just that file's shape.)
    fn examples(json: &str) -> Vec<(String, String, u32)> {
        let (bytes, mut at, mut found) = (json.as_bytes(), 0, Vec::new());
        let (mut markdown, mut html, mut key) = (String::new(), String::new(), String::new());
        while at < bytes.len() {
            match bytes[at] {
                b'"' => {
                    let (text, end) = json_string(json, at);
                    at = end;
                    let after = json[at..].trim_start();
                    if after.starts_with(':') {
                        key = text;
                    } else if key == "markdown" {
                        markdown = text;
                    } else if key == "html" {
                        html = text;
                    }
                }
                b'0'..=b'9' => {
                    let digits = json[at..].bytes().take_while(u8::is_ascii_digit).count();
                    if key == "example" {
                        found.push((markdown.clone(), html.clone(), json[at..at + digits].parse().unwrap()));
                    }
                    at += digits;
                }
                _ => at += 1,
            }
        }
        found
    }

    fn json_string(json: &str, at: usize) -> (String, usize) {
        let (bytes, mut end, mut out) = (json.as_bytes(), at + 1, String::new());
        let mut pending: Option<u32> = None;
        while bytes[end] != b'"' {
            if bytes[end] != b'\\' {
                let letter = json[end..].chars().next().unwrap();
                out.push(letter);
                end += letter.len_utf8();
                continue;
            }
            let escape = bytes[end + 1];
            end += 2;
            match escape {
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                b'r' => out.push('\r'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'u' => {
                    let unit = u32::from_str_radix(&json[end..end + 4], 16).unwrap();
                    end += 4;
                    match (pending.take(), unit) {
                        (Some(high), 0xdc00..=0xdfff) => out.push(char::from_u32(0x10000 + ((high - 0xd800) << 10) + (unit - 0xdc00)).unwrap()),
                        (_, 0xd800..=0xdbff) => pending = Some(unit),
                        (_, unit) => out.push(char::from_u32(unit).unwrap_or('\u{fffd}')),
                    }
                }
                other => out.push(other as char),
            }
        }
        (out, end + 1)
    }

    /// GitHub's own examples (tables, task lists, strikethrough) from its spec.txt: each with
    /// Markdown, HTML and extension.
    fn github_examples(spec: &str) -> Vec<(String, String, String)> {
        let fence = "`".repeat(32);
        let mut found = Vec::new();
        let mut rest = spec;
        while let Some(start) = rest.find(&format!("{fence} example ")) {
            let after = &rest[start + fence.len() + 9..];
            let (kind, body) = after.split_once('\n').unwrap();
            let end = body.find(&fence).unwrap();
            let (markdown, html) = body[..end].split_once("\n.\n").unwrap_or(("", ""));
            let tabs = |text: &str| text.replace('\u{2192}', "\t");
            if ["table", "disabled", "strikethrough"].contains(&kind.trim()) {
                found.push((tabs(markdown) + "\n", tabs(html), kind.trim().to_string()));
            }
            rest = &body[end + fence.len()..];
        }
        found
    }

    /// Parses every CommonMark spec example (0.31.2) and GitHub's own, and compares the HTML
    /// rendered from the parse with the spec's. The spec is not in the repository: set
    /// `TAUT_COMMONMARK_SPEC` to its `spec.json` (from spec.commonmark.org) and `TAUT_GFM_SPEC`
    /// to GitHub's `spec.txt` (from the cmark-gfm repository), and run with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_specifications_own_examples() {
        // (Without the spec there is nothing to compare: say so, and pass.)
        let Some(file) = std::env::var_os("TAUT_COMMONMARK_SPEC") else { return eprintln!("TAUT_COMMONMARK_SPEC is not set: the specification's examples are not read") };
        let spec = std::fs::read_to_string(file).unwrap();
        let (mut passed, mut failed) = (0, Vec::new());
        let all = examples(&spec);
        for (markdown, expected, number) in &all {
            let got = std::panic::catch_unwind(|| html(markdown)).unwrap_or_else(|_| "(panicked)".to_string());
            if got == *expected {
                passed += 1;
            } else {
                failed.push(*number);
                if std::env::var("TAUT_SPEC_SHOW").is_ok() {
                    println!("example {number}:\n{markdown:?}\nwanted {expected:?}\ngot    {got:?}\n");
                }
            }
        }
        println!("CommonMark 0.31.2: {passed} of {} examples pass; these do not: {failed:?}", all.len());
        if let Ok(path) = std::env::var("TAUT_GFM_SPEC") {
            let github = github_examples(&std::fs::read_to_string(path).unwrap());
            let mut ok = 0;
            for (markdown, expected, kind) in &github {
                let got = html(markdown);
                ok += (got == *expected) as usize;
                if got != *expected {
                    println!("GitHub {kind}:\n{markdown:?}\nwanted {expected:?}\ngot    {got:?}\n");
                }
            }
            println!("GitHub's own: {ok} of {} examples pass", github.len());
        }
    }
}
