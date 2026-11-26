//! The accessibility tree: what screen readers get. A screen reader reads a window aloud and
//! operates it for people who cannot see it. It gets a list of nodes: every piece of text and
//! every control, with its role, name and state (checked, selected, expanded, value).
//!
//! Most of it is implied. A box holding text is text. A focusable box is a button named by its
//! text. Widgets that are something else say so (`tell`): a check box, a slider, a text box, a
//! table. An app does the same for its own widgets, and names what has no text:
//!
//! ```ignore
//! let add = ui.icon_button(PLUS, "Add a note");   // named by its hint
//! let dot = ui.el().size(12.0, 12.0).radius(6.0).fill(t.green).sense().show();
//! ui.tell(&dot, Role::Text, None, "Up to date");
//! ```
//!
//! A table is a table node: column headers, then rows of cells, so a screen reader can say a
//! cell's row, column and header, and move by cell, row and column. It reports its total row
//! count and its selected rows; only visible rows have nodes, and a reader asking for another
//! scrolls it into view. A visible row has all its cells, and the headers all theirs, however
//! far the table is scrolled sideways.
//!
//! A tree is a tree node: its items in display order, each with its level, its position among
//! its siblings, and whether it is expanded and selected. Tree items the app put in no tree
//! are told in one.
//!
//! Text that shows is read by line as wrapped, and each letter's place is known, where the
//! text is one box's.
//!
//! While no screen reader listens, nothing is built. While one does, a run that changes no
//! input keeps the nodes, and one that only moves boxes (scrolling) moves them.
use crate::event::{Key, Mods};
use crate::geom::{Rect, rect};
use crate::id::{Id, fold, hash};
use crate::layout::{Align, Content, NONE, Tree};
use crate::spell::Spelling;
use crate::table::Track;
use crate::text::{Line, TextSystem};
use crate::ui::{Response, Rover, Stroke, Ui};
use crate::visible_boxes::{Lying, VisibleBoxes};
use std::cell::OnceCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// A box's role, for a screen reader to announce and to know what can be done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[doc(alias("accessibility", "aria"))]
pub enum Role {
    /// Text to read.
    Text,
    /// A button.
    Button,
    /// A check box.
    CheckBox,
    /// A toggle switch.
    Switch,
    /// A radio button.
    RadioButton,
    /// A tab in a tab row.
    Tab,
    /// A selectable list row.
    Row,
    /// A slider.
    Slider,
    /// A spin button: a number typed or stepped. (A number box, and each part of a time.)
    SpinButton,
    /// A text box.
    TextBox,
    /// A password box.
    PasswordBox,
    /// A disclosure: a heading or button that expands and collapses its content.
    Fold,
    /// A combo box: shows one choice, and opens the list of them.
    ComboBox,
    /// A menu item.
    MenuItem,
    /// A table column header.
    ColumnHeader,
    /// A progress bar.
    Progress,
    /// A named group: a panel.
    Group,
    /// A modal dialog.
    Dialog,
    /// A menu: the items opened from a button or another item.
    Menu,
    /// A menu bar.
    MenuBar,
    /// A tab row.
    Tabs,
    /// A link.
    Link,
    /// A table: headings, and rows of cells under them.
    Table,
    /// A table row containing its cells; possibly selectable.
    TableRow,
    /// A table cell.
    Cell,
    /// A radio menu item.
    MenuItemRadio,
    /// A menu item opening a submenu: on while the submenu is open.
    Submenu,
    /// A tool bar.
    ToolBar,
    /// A tree: items that expand to show their children (folders, an outline, settings by
    /// category).
    Tree,
    /// A tree item; possibly selectable, and expandable where it has children.
    TreeItem,
    /// A picture, drawing or animation: described by its name.
    Picture,
    /// A document (a help page, a message), holding headings, paragraphs, lists, code and
    /// quotes.
    Document,
    /// A document heading, of a level from 1 (see [`AccessNode::level`]).
    Heading,
    /// A document paragraph.
    Paragraph,
    /// A document list, ordered or not.
    List,
    /// A list item, holding its content.
    ListItem,
    /// A code block, verbatim.
    Code,
    /// A block quote.
    Quote,
}

impl Role {
    /// Whether it only contains other things and is not itself operable: its contents are told
    /// as inside it.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            Role::Group
                | Role::Dialog
                | Role::Menu
                | Role::MenuBar
                | Role::Tabs
                | Role::Table
                | Role::TableRow
                | Role::Cell
                | Role::ToolBar
                | Role::Tree
                | Role::Document
                | Role::List
                | Role::ListItem
                | Role::Quote
        )
    }
}

/// One element of the window, as told to a screen reader.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessNode {
    /// Its id, stable from run to run.
    pub id: Id,
    /// Its role.
    pub role: Role,
    /// Its name: as given, else its text.
    pub name: String,
    /// A text box's text (for a password box, the dots it shows); a combo box's chosen item.
    pub value: String,
    /// Its keyboard shortcut, as its part shows it ("Ctrl+O"): usually none.
    pub shortcut: String,
    /// What more it says than its name: a link's title. Usually none.
    pub description: String,
    /// Whether it is on, selected or expanded, for roles that can be. (For a tree: whether
    /// multiple selection is allowed.)
    pub on: Option<bool>,
    /// For a tree item with children: whether they show. (None for one without.)
    pub expanded: Option<bool>,
    /// For a tree item, its depth, 1 at the top. For a document heading, its level, 1 for the
    /// main one. (0 otherwise.)
    pub level: u32,
    /// For a slider or progress bar: its value, minimum and maximum.
    pub value_range: Option<[f32; 3]>,
    /// A slider's step.
    pub step: f32,
    /// Its position in its set, from 1: the second of five tabs, row 4,000 of 5,000 (of which
    /// only visible rows are told), the third visible heading.
    pub position: Option<[u32; 2]>,
    /// For a table: its total rows, and its visible columns. For a cell: its row and column
    /// from 0, among all rows and among the visible columns.
    pub grid: Option<[u32; 2]>,
    /// For the column heading the rows are sorted by: whether descending.
    pub sorted: Option<bool>,
    /// For a text box while focused: the selection anchor and the cursor, in characters from
    /// its start.
    pub caret: Option<[u32; 2]>,
    /// Where it is in the window. (A table's content may lie outside what the table shows: see
    /// `view`.)
    pub rect: Rect,
    /// The group containing it, by its index in the list.
    pub parent: Option<usize>,
    /// It has focus.
    pub focused: bool,
    /// It is enabled.
    pub enabled: bool,
    /// Some of it is visible. An element scrolled out of its scroll box or the window is still
    /// told ([`Ui::access_nodes`]), as offscreen; it is not in what shows
    /// ([`Ui::visible_access_nodes`]). (In a table, every heading and everything in a visible
    /// row count as visible, however far it is scrolled sideways.)
    pub visible: bool,
    /// For a spell-checked text box ([`Ui::check_spelling`]): its misspellings, each from its
    /// first character to one past its last, from the start of its text. All of them in up to
    /// 64 KB of text; past that, those visible.
    pub misspelled: Vec<[u32; 2]>,
    /// Its visible part: of `rect`, what its containers and the window leave in view. (Empty
    /// for what does not show.)
    pub(crate) seen: Rect,
    /// Its box, by index among the run's boxes.
    pub(crate) at: u32,
    /// For text read by line and letter ([`AccessNode::reads_text`]) that shows: its lines and
    /// letters, where its text is one box's; the box, and where it lies.
    pub(crate) letters: Option<Arc<Letters>>,
    pub(crate) text_box: u32,
    pub(crate) text_rect: Rect,
    /// For a table: its selected rows, seen or not, each range from its first to one past its
    /// last. (As the app told them, else those of the rows told.)
    pub(crate) selected_rows: Vec<[u32; 2]>,
}

/// What screen readers ask of tables, and what apps tell of them.
#[derive(Default)]
pub(crate) struct TableReading {
    /// A row a screen reader asked for, until its table shows it: the table's id, the row
    /// from 0, and whether it is then selected alone (`Some(true)`) or toggled.
    pub(crate) asked: Option<(Id, u32, Option<bool>)>,
    /// The selections told this run (`Ui::tell_selection`): each table's box, and its selected
    /// rows, each range from its first to one past its last.
    pub(crate) told: Vec<(u32, Vec<[u32; 2]>)>,
}

/// Where a text's lines start and its letters lie, as drawn: lines as wrapped.
#[derive(Clone, Debug)]
pub(crate) struct Letters {
    /// A hash of how they were laid out: the text, its font and box, and its lines.
    pub(crate) key: u64,
    /// Each line's first character; the first is 0.
    pub(crate) lines: Vec<u32>,
    /// Each character's place, from its box's corner. (A line break, or a space a wrap left
    /// out, is empty at its line's end.)
    pub(crate) places: Vec<Rect>,
}

/// Letters are the same where they lie the same, however their key was made.
impl PartialEq for Letters {
    fn eq(&self, other: &Letters) -> bool {
        (&self.lines, &self.places) == (&other.lines, &other.places)
    }
}

// (Measured letters are never NaN. Being `Eq`, those shared between node lists compare by
// pointer first.)
impl Eq for Letters {}

/// What a widget or app told of a box this run. Its name, value and shortcut are ranges of the
/// note text (`Ui::noted`).
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Note {
    /// The box, by index.
    at: u32,
    role: Role,
    on: Option<bool>,
    /// For a tree item: whether its children show, and its depth.
    expanded: Option<bool>,
    level: u32,
    /// Its text is the rest of the previous note's, and is read with it.
    joined: bool,
    value_range: Option<[f32; 3]>,
    step: f32,
    position: Option<[u32; 2]>,
    caret: Option<[u32; 2]>,
    /// For a table: its row count (counted as rows are described), and its visible columns.
    grid: Option<[u32; 2]>,
    /// For a table row: its index from 0; the range of its table's columns in their list; and
    /// the table's own note's index.
    row: Option<[u32; 4]>,
    sorted: Option<bool>,
    name: [u32; 2],
    value: [u32; 2],
    shortcut: [u32; 2],
    description: [u32; 2],
    enabled: bool,
    /// A node of its own inside a node read as one: a tab's close button.
    separate: bool,
}

impl Note {
    /// A note of a role, name and value on box `at`, with nothing more told.
    fn new(at: u32, role: Role, name: [u32; 2], value: [u32; 2], enabled: bool) -> Note {
        Note {
            at,
            role,
            on: None,
            expanded: None,
            level: 0,
            joined: false,
            value_range: None,
            step: 0.0,
            position: None,
            caret: None,
            grid: None,
            row: None,
            sorted: None,
            name,
            value,
            shortcut: [value[1]; 2],
            description: [value[1]; 2],
            enabled,
            separate: false,
        }
    }
}

impl AccessNode {
    /// A node of a role and name, with nothing more told.
    pub(crate) fn plain(id: Id, role: Role, name: String, rect: Rect, parent: Option<usize>) -> AccessNode {
        AccessNode {
            id,
            role,
            name,
            value: String::new(),
            shortcut: String::new(),
            description: String::new(),
            on: None,
            expanded: None,
            level: 0,
            value_range: None,
            step: 0.0,
            position: None,
            grid: None,
            sorted: None,
            caret: None,
            rect,
            parent,
            focused: false,
            enabled: true,
            visible: true,
            misspelled: Vec::new(),
            seen: rect,
            at: 0,
            letters: None,
            text_box: 0,
            text_rect: Rect::default(),
            selected_rows: Vec::new(),
        }
    }

    /// Whether a reader reads its text by line and letter: a text box, plain text, a heading,
    /// a paragraph, code.
    pub(crate) fn reads_text(&self) -> bool {
        matches!(self.role, Role::TextBox | Role::PasswordBox | Role::Text | Role::Heading | Role::Paragraph | Role::Code)
    }
    /// The text it is read by: a text box's own (`value`), else its name.
    pub(crate) fn text(&self) -> &str {
        if matches!(self.role, Role::TextBox | Role::PasswordBox) { &self.value } else { &self.name }
    }
    /// Each line's first character: as wrapped where its text is laid out, else after each
    /// line break.
    pub(crate) fn line_starts(&self) -> Vec<u32> {
        if let Some(letters) = &self.letters {
            return letters.lines.clone();
        }
        let breaks = self.text().chars().enumerate().filter(|(_, letter)| *letter == '\n').map(|(at, _)| at as u32 + 1);
        std::iter::once(0).chain(breaks).collect()
    }
    /// The line (from 0) character `letter` is on. (The text's end is on the last.)
    #[cfg(target_vendor = "apple")]
    pub(crate) fn line_of(&self, letter: u32) -> usize {
        self.line_starts().partition_point(|start| *start <= letter).saturating_sub(1)
    }
    /// Line `line`'s characters: from its first up to the next line's (its break included).
    /// None past the last.
    #[cfg(any(target_vendor = "apple", target_os = "android", test))]
    pub(crate) fn line_range(&self, line: usize) -> Option<[u32; 2]> {
        let (starts, count) = (self.line_starts(), self.text().chars().count() as u32);
        Some([*starts.get(line)?, starts.get(line + 1).copied().unwrap_or(count)])
    }
    /// Where character `letter` lies in the window: as drawn, else its whole box. Past the end,
    /// an empty place after the last.
    pub(crate) fn letter_rect(&self, letter: u32) -> Rect {
        let Some(letters) = &self.letters else { return self.rect };
        let corner = |place: Rect| place.moved(self.text_rect.x, self.text_rect.y);
        match letters.places.get(letter as usize) {
            Some(place) => corner(*place),
            None => letters.places.last().map_or(self.rect, |last| corner(rect(last.right(), last.y, 0.0, last.h))),
        }
    }
    /// Where characters `from` to `to` show: a rectangle per line, cut to what shows. (An empty
    /// range: the cursor's place there, one unit wide.)
    #[cfg(any(windows, target_os = "ios"))]
    pub(crate) fn range_rects(&self, [from, to]: [u32; 2]) -> Vec<Rect> {
        if from >= to {
            let at = self.letter_rect(from);
            let caret = rect(at.x, at.y, 1.0, at.h).intersect(self.seen);
            return if caret.is_empty() { Vec::new() } else { vec![caret] };
        }
        let starts = self.line_starts();
        let mut lines = Vec::new();
        for (line, start) in starts.iter().enumerate() {
            let end = starts.get(line + 1).copied().unwrap_or(u32::MAX);
            let (first, last) = ((*start).max(from), end.min(to));
            let spans = (first..last).map(|letter| self.letter_rect(letter)).reduce(|one, other| one.union(other));
            lines.extend(spans.map(|span| span.intersect(self.seen)).filter(|span| !span.is_empty()));
        }
        lines
    }
    /// The characters that show: from the first to one past the last. (All, where its letters
    /// are not known.)
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn shown_letters(&self) -> [u32; 2] {
        let count = self.text().chars().count() as u32;
        let Some(letters) = &self.letters else { return [0, count] };
        let shows = |place: &Rect| !place.moved(self.text_rect.x, self.text_rect.y).grow(0.5).intersect(self.seen).is_empty();
        let first = letters.places.iter().position(shows).unwrap_or(0);
        let last = letters.places.iter().rposition(shows).map_or(first, |last| last + 1);
        [first as u32, last as u32]
    }
    /// The character at a point in the window, or nearest it on the line there. None where
    /// its letters are not known, or for a point outside it.
    #[cfg(any(not(target_os = "android"), test))]
    pub(crate) fn letter_at(&self, point: crate::geom::Point) -> Option<u32> {
        let letters = self.letters.as_ref().filter(|_| self.rect.contains(point))?;
        let near = |place: &Rect| {
            let place = place.moved(self.text_rect.x, self.text_rect.y);
            let (dx, dy) = ((place.x - point.x).max(point.x - place.right()).max(0.0), (place.y - point.y).max(point.y - place.bottom()).max(0.0));
            (dy, dx)
        };
        let nearest = letters.places.iter().enumerate().min_by(|(_, one), (_, other)| near(one).partial_cmp(&near(other)).unwrap_or(std::cmp::Ordering::Equal));
        nearest.map(|(at, _)| at as u32)
    }
    /// The text unit a reader moves to from character `from`: the next one forward, or the one
    /// before; from its first character to one past its last. None at the end.
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn unit_range(&self, from: u32, unit: TextUnit, forward: bool) -> Option<[u32; 2]> {
        let letters: Vec<char> = self.text().chars().collect();
        let (count, from) = (letters.len() as u32, from.min(letters.len() as u32));
        match unit {
            TextUnit::Character if forward => (from < count).then_some([from, from + 1]),
            TextUnit::Character => from.checked_sub(1).map(|before| [before, from]),
            TextUnit::Word => next_run(&letters, from, forward, char::is_alphanumeric),
            TextUnit::Paragraph => next_run(&letters, from, forward, |letter| letter != '\n'),
            TextUnit::Line => self.line_range_from(from, count, forward),
        }
    }
    /// The line a reader moves to from character `from`: forward, the line starting there or
    /// the next; back, the line before it starts. (As wrapped.)
    #[cfg(any(target_os = "android", test))]
    fn line_range_from(&self, from: u32, count: u32, forward: bool) -> Option<[u32; 2]> {
        let starts = self.line_starts();
        let line = if forward { starts.iter().position(|start| *start >= from)? } else { starts.partition_point(|start| *start < from).checked_sub(1)? };
        let range = self.line_range(line)?;
        (range[0] < count).then_some(range)
    }

    /// For the column heading the rows are sorted by: the direction, in the word both Windows
    /// and Linux use.
    #[cfg(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
    pub(crate) fn sort_word(&self) -> Option<&'static str> {
        self.sorted.map(|down| if down { "descending" } else { "ascending" })
    }
    /// Whether it can take focus: anything operable, a selectable table row, a document (for
    /// selecting in it), and a table cell (focus there being the table's cell); not text,
    /// progress bars, or containers.
    #[cfg(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android"))), test, target_family = "wasm"))]
    pub(crate) fn takes_keyboard(&self) -> bool {
        match self.role {
            Role::TableRow => self.on.is_some(),
            Role::Document | Role::Cell => true,
            role => !matches!(role, Role::Text | Role::Progress | Role::Heading | Role::Paragraph | Role::Code) && !role.is_container(),
        }
    }
    /// Whether what it opens shows: a combo box's list, a disclosure's content, a tree item's
    /// children. None for something that opens nothing.
    #[cfg(windows)]
    pub(crate) fn unfolded(&self) -> Option<bool> {
        match self.role {
            Role::Fold | Role::ComboBox => Some(self.on == Some(true)),
            Role::TreeItem => self.expanded,
            _ => None,
        }
    }
}

/// A tree item's tree, by node index. (None for a hand-written tree item the app put in no
/// tree.)
#[cfg(windows)]
pub(crate) fn tree_of(nodes: &[AccessNode], item: usize) -> Option<usize> {
    let tree = nodes.get(item).filter(|item| item.role == Role::TreeItem)?.parent?;
    nodes.get(tree).is_some_and(|tree| tree.role == Role::Tree).then_some(tree)
}

/// The visible part of a node, by its index: all of it, but in a table only what lies within
/// the table. (A row has all its cells, however far the table is scrolled sideways.)
#[cfg(not(target_os = "android"))]
pub(crate) fn view(nodes: &[AccessNode], at: usize) -> Rect {
    let (mut seen, mut up) = (nodes[at].rect, nodes[at].parent);
    while let Some(group) = up.and_then(|group| nodes.get(group)) {
        seen = if group.role == Role::Table { seen.intersect(group.rect) } else { seen };
        up = group.parent;
    }
    seen
}

/// A cell's table, by node index.
pub(crate) fn table_of(nodes: &[AccessNode], cell: usize) -> Option<usize> {
    let row = nodes.get(cell).filter(|cell| cell.role == Role::Cell)?.parent?;
    nodes.get(row)?.parent.filter(|table| nodes.get(*table).is_some_and(|table| table.role == Role::Table))
}

/// A table's cell at a row and column from 0: its node index. None for a row out of view,
/// whose cells have no nodes.
#[cfg(not(target_os = "android"))]
pub(crate) fn cell_at(nodes: &[AccessNode], table: usize, place: [u32; 2]) -> Option<usize> {
    (table + 1..nodes.len()).find(|at| nodes[*at].grid == Some(place) && table_of(nodes, *at) == Some(table))
}

/// The rows of a long table told now (those in view): from the first to one past the last,
/// from 0 among all its rows.
#[cfg(any(target_os = "ios", target_os = "android", test))]
pub(crate) fn rows_in_view(nodes: &[AccessNode], table: usize) -> Option<[u32; 2]> {
    let rows = nodes[table + 1..].iter().filter(|node| node.parent == Some(table) && node.role == Role::TableRow);
    let indices = rows.filter_map(|row| row.position.map(|[nth, _]| nth - 1));
    indices.fold(None, |span, row| Some(span.map_or([row, row + 1], |[from, to]: [u32; 2]| [from.min(row), to.max(row + 1)])))
}

/// The row a reader paging through a long table has shown: as many rows past those in view as
/// show, or before them. None at the end it pages toward.
#[cfg(any(target_os = "ios", target_os = "android", test))]
pub(crate) fn row_a_page_away(nodes: &[AccessNode], table: usize, forward: bool) -> Option<u32> {
    let ([from, to], [rows, _]) = (rows_in_view(nodes, table)?, nodes[table].grid?);
    let shown = to - from;
    if forward { (to < rows).then(|| (to - 1 + shown).min(rows - 1)) } else { (from > 0).then(|| from.saturating_sub(shown)) }
}

/// The heading over a table column, from 0.
#[cfg(not(any(target_os = "macos", target_os = "android")))]
pub(crate) fn heading_at(nodes: &[AccessNode], table: usize, column: u32) -> Option<usize> {
    let heads = |at: &usize| nodes[*at].role == Role::ColumnHeader && nodes[*at].parent == Some(table);
    (table + 1..nodes.len()).find(|at| heads(at) && nodes[*at].position.is_some_and(|of| of[0] == column + 1))
}

/// A unit a reader moves through text by: a character, word, line (as wrapped) or paragraph.
#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextUnit {
    Character,
    Word,
    Line,
    Paragraph,
}

/// The next run of letters that are `inside` from `from` (or the one before it): from its
/// first letter to one past its last.
#[cfg(any(target_os = "android", test))]
fn next_run(letters: &[char], from: u32, forward: bool, inside: fn(char) -> bool) -> Option<[u32; 2]> {
    let within = |at: u32| letters.get(at as usize).is_some_and(|letter| inside(*letter));
    let count = letters.len() as u32;
    if forward {
        let start = (from..count).find(|at| within(*at))?;
        let end = (start..count).find(|at| !within(*at)).unwrap_or(count);
        return Some([start, end]);
    }
    let end = (0..from).rev().find(|at| within(*at))? + 1;
    let start = (0..end).rev().find(|at| !within(*at)).map_or(0, |out| out + 1);
    Some([start, end])
}

/// UTF-16 units in the first `letters` characters of a text: where a character starts as
/// Apple's and Android's text count.
#[cfg(any(target_vendor = "apple", target_os = "android", test))]
pub(crate) fn units_before(text: &str, letters: u32) -> u32 {
    text.chars().take(letters as usize).map(|letter| letter.len_utf16() as u32).sum()
}

/// The character holding UTF-16 unit `unit` of a text, from its start (the text's end, past it).
#[cfg(any(target_vendor = "apple", target_os = "android", test))]
pub(crate) fn letter_at_unit(text: &str, unit: u32) -> u32 {
    let mut seen = 0;
    let before = text.chars().take_while(|letter| {
        seen += letter.len_utf16() as u32;
        seen <= unit
    });
    before.count() as u32
}

/// The text in a box and its descendants, piece after piece, up to `most` pieces.
fn words_in(tree: &Tree, at: u32, most: usize) -> String {
    let texts = tree.specs[at as usize..tree.specs[at as usize].end as usize].iter().filter_map(|spec| match spec.content {
        Content::Text { start, end, .. } if start < end => Some(&tree.strings[start as usize..end as usize]),
        _ => None,
    });
    let mut words = String::new();
    for (nth, text) in texts.take(most).enumerate() {
        words.push_str(if nth > 0 { ", " } else { "" });
        words.push_str(text);
    }
    words
}

/// The text just before box `at`, which labels it where it has none: the previous sibling's
/// text, or that of the box before the one it is first in (a widget made of a box or two around
/// it), where that is only text: plain text or a link (a form whose labels link to help). ""
/// where none.
pub(crate) fn label_before<'t>(tree: &'t Tree, notes: &[Note], at: u32) -> &'t str {
    let mut first = at;
    for _ in 0..2 {
        let around = tree.specs[first as usize].frame.parent;
        if around == NONE || around + 1 != first {
            break;
        }
        first = around;
    }
    let (Some(before), Some(here)) = (first.checked_sub(1).and_then(|before| tree.specs.get(before as usize)), tree.specs.get(first as usize)) else { return "" };
    let link = || last_note(notes, first - 1).is_some_and(|note| note.role == Role::Link);
    let label = before.frame.parent == here.frame.parent && here.frame.parent != NONE && before.end == first && (!before.frame.senses && !before.frame.focus || link());
    match before.content {
        Content::Text { start, end, .. } if label => &tree.strings[start as usize..end as usize],
        _ => "",
    }
}

/// The first text in the sibling just before box `at`: what a splitter between two panes is
/// named by, the pane before it. None where it holds none.
pub(crate) fn words_before(tree: &Tree, at: u32) -> Option<&str> {
    let parent = tree.specs.get(at as usize)?.frame.parent;
    let mut before = at.checked_sub(1)?;
    while before != parent && tree.specs[before as usize].frame.parent != parent {
        before = tree.specs[before as usize].frame.parent;
    }
    let texts = tree.specs.get(before as usize..at as usize).filter(|_| before != parent)?.iter();
    texts.filter_map(|spec| if let Content::Text { start, end, .. } = spec.content { Some(&tree.strings[start as usize..end as usize]) } else { None }).find(|text| !text.is_empty())
}

/// The last note on box `at`, among notes in box order (and, for one box, in the order told).
fn last_note(notes: &[Note], at: u32) -> Option<&Note> {
    notes.partition_point(|note| note.at <= at).checked_sub(1).map(|last| &notes[last]).filter(|note| note.at == at)
}

/// The notes on box `at`, among notes in box order, in the order told.
fn notes_on(notes: &[Note], at: u32) -> impl Iterator<Item = &Note> {
    notes[notes.partition_point(|note| note.at < at)..].iter().take_while(move |note| note.at == at)
}

/// All notes on box `at` in one, among notes in box order: its widget's note, overlaid with
/// later ones where they tell something. So an app adding to a widget right after it (a name,
/// another role) is heard, and what the widget told and the app did not (a slider's value, a
/// combo's choice) still is.
fn merged_note(notes: &[Note], at: u32) -> Option<Note> {
    let mut on_box = notes_on(notes, at);
    let mut merged = *on_box.next()?;
    for later in on_box {
        let or_was = |range: [u32; 2], was: [u32; 2]| if range[0] < range[1] { range } else { was };
        (merged.role, merged.name, merged.value, merged.shortcut) = (later.role, or_was(later.name, merged.name), or_was(later.value, merged.value), or_was(later.shortcut, merged.shortcut));
        merged.description = or_was(later.description, merged.description);
        (merged.on, merged.expanded, merged.value_range) = (later.on.or(merged.on), later.expanded.or(merged.expanded), later.value_range.or(merged.value_range));
        (merged.position, merged.caret, merged.grid) = (later.position.or(merged.position), later.caret.or(merged.caret), later.grid.or(merged.grid));
        (merged.row, merged.sorted) = (later.row.or(merged.row), later.sorted.or(merged.sorted));
        merged.level = if later.level > 0 { later.level } else { merged.level };
        merged.step = if later.step > 0.0 { later.step } else { merged.step };
    }
    Some(merged)
}

/// Appends `piece` to the note text: its range there.
fn push_text(text: &mut String, piece: &str) -> [u32; 2] {
    let from = text.len() as u32;
    text.push_str(piece);
    [from, text.len() as u32]
}

/// Updates the accessibility tree after a run (`Ui::update_access_nodes`). Set by the first
/// call to [`Ui::set_screen_reader`] and reached by nothing else, so a program no screen reader
/// can listen to is built without it.
pub(crate) static UPDATE_ACCESS_NODES: OnceLock<fn(&mut Ui)> = OnceLock::new();

/// A box walked for the accessibility tree: its index, its rectangle, its visible part, and
/// whether it shows ([`AccessNode::visible`]).
pub(crate) type AccessBox = (u32, Rect, Rect, bool);

/// What the nodes were built from, kept while a screen reader listens. A run that changed none
/// of it keeps the nodes; one that only moved boxes moves them.
#[derive(Default)]
pub(crate) struct AccessCache {
    /// The boxes walked last (kept so its memory is reused).
    boxes: Vec<AccessBox>,
    /// The notes and their text; and the boxes' text.
    notes: Vec<Note>,
    note_text: String,
    text: String,
    /// The rest, hashed: see `Ui::inputs_hash`.
    key: u64,
    /// Which nodes these are: new with every change, unique among windows. 0 for none yet.
    version: u64,
    /// Which nodes these were moved from, where only their places changed since. 0 for none.
    moved_from: u64,
    /// Times the nodes were built, and moved.
    built: u32,
    moved: u32,
}

/// The next nodes' `version`.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

/// Walks the boxes of the accessibility tree: every box of the layers from the topmost modal
/// one (a dialog) up, visible or not, in `Lying`'s order. (What lies under a dialog is
/// unreachable until it is answered, so it has no nodes meanwhile. Under an open menu
/// everything keeps its nodes: a menu closes at a key, and the reader should find the window as
/// it was.)
struct AccessWalk<'a> {
    tree: &'a Tree,
    notes: &'a [Note],
    lying: &'a mut Lying,
    window: Rect,
    boxes: Vec<AccessBox>,
    /// The boxes shown whole, however they are scrolled: a visible table row, and the box
    /// holding its headings (the table's first).
    shown_whole: [u32; 2],
    /// Where the headings of the last table walked would be.
    headings: u32,
    /// The first note on a box after the last one walked: boxes come in order, but for each
    /// layer starting again.
    next_note: usize,
}

impl AccessWalk<'_> {
    /// Enters box `at`: its visible part, and whether it shows. Descends into all of it.
    fn enter(&mut self, at: u32) -> bool {
        // (A hidden box and everything in it does not exist for a screen reader.)
        if self.tree.specs[at as usize].fade == 255 {
            return false;
        }
        let (r, clip) = self.lying.lies(self.tree, at);
        let (seen, whole) = (r.intersect(clip).intersect(self.window), at > self.shown_whole[0] && at < self.shown_whole[1]);
        let shown = whole || !self.tree.drawn(at).moved(r.x, r.y).intersect(clip).intersect(self.window).is_empty();
        self.boxes.push((at, r, seen, shown));
        let role = self.role_of(at);
        if shown && !whole && (role == Some(Role::TableRow) || at == self.headings) {
            self.shown_whole = [at, self.tree.specs[at as usize].end];
        }
        self.headings = if role == Some(Role::Table) { at + 1 } else { self.headings };
        self.lying.go_in(self.tree, at, r, clip);
        true
    }
    /// The role last told of box `at`, if any.
    fn role_of(&mut self, at: u32) -> Option<Role> {
        let notes = self.notes;
        if self.next_note > 0 && notes.get(self.next_note - 1).is_some_and(|note| note.at > at) {
            self.next_note = notes.partition_point(|note| note.at < at);
        }
        while notes.get(self.next_note).is_some_and(|note| note.at <= at) {
            self.next_note += 1;
        }
        notes.get(self.next_note.wrapping_sub(1)).filter(|note| note.at == at).map(|note| note.role)
    }
    /// Leaves box `at`. (A sticky heading at the top of a scroll box hides what is under it, as
    /// `VisibleBoxes` has it.)
    fn leave(&mut self, at: u32) -> bool {
        let (r, _) = self.lying.go_out(self.tree, at);
        self.lying.pin(self.tree, at, r);
        false
    }
}

/// The boxes of the accessibility tree (see `AccessWalk`), in window `window`, into `boxes`.
fn walk_access_boxes(visible: &mut VisibleBoxes, boxes: &mut Vec<AccessBox>, tree: &Tree, notes: &[Note], window: Rect) {
    let VisibleBoxes { roots, reader_root: told_from, reader_lying: told, .. } = visible;
    boxes.clear();
    let mut walk = AccessWalk { tree, notes, lying: told, window, boxes: std::mem::take(boxes), shown_whole: [0; 2], headings: NONE, next_note: 0 };
    let mut open = Vec::new();
    for root in roots.get(*told_from..).unwrap_or_default() {
        walk.lying.start();
        tree.walk(*root, &mut open, &mut |at, entering| if entering { walk.enter(at) } else { walk.leave(at) });
    }
    *boxes = walk.boxes;
}

/// Brings a copy of the nodes the current ones were moved from (`Ui::access_moved_from`) up
/// to date: where each lies, what of it shows, and the letters of what shows.
pub(crate) fn follow_move(kept: &mut [AccessNode], now: &[AccessNode]) {
    for (kept, now) in kept.iter_mut().zip(now) {
        (kept.rect, kept.seen, kept.visible, kept.text_box, kept.text_rect) = (now.rect, now.seen, now.visible, now.text_box, now.text_rect);
        kept.letters.clone_from(&now.letters);
    }
    #[cfg(debug_assertions)]
    assert!(same_nodes(kept, now), "only places changed");
}

/// Whether two node lists are the same: equal, or equal but for NaN in what an app gave.
#[cfg(debug_assertions)]
fn same_nodes(one: &[AccessNode], other: &[AccessNode]) -> bool {
    one == other || format!("{one:?}") == format!("{other:?}")
}

/// The misspellings in a `role` text box `id` with text `typed`, where its words are checked:
/// each from its first character to one past its last. (None for anything else, or when
/// unchecked.)
fn misspellings(spelling: &Spelling, role: Role, id: Id, typed: &str) -> Vec<[u32; 2]> {
    let Some(hooks) = spelling.hooks().filter(|_| role == Role::TextBox) else { return Vec::new() };
    let letters = |byte: usize| typed.get(..byte).map_or(0, |before| before.chars().count()) as u32;
    let mut wrong: Vec<[u32; 2]> = (hooks.wrong)(spelling, id).into_iter().map(|word| [letters(word.start), letters(word.end)]).collect();
    wrong.dedup();
    wrong
}

/// The visible nodes ([`Ui::visible_access_nodes`]), each in its nearest visible group.
fn visible_nodes(nodes: &[AccessNode]) -> Vec<AccessNode> {
    // (Each node's index among the visible ones, or none; a group comes before its content.)
    let (mut visible, mut places) = (Vec::new(), Vec::with_capacity(nodes.len()));
    for one in nodes {
        let mut parent = one.parent;
        while let Some(group) = parent.filter(|group| places[*group] == usize::MAX) {
            parent = nodes[group].parent;
        }
        places.push(if one.visible { visible.len() } else { usize::MAX });
        if one.visible {
            visible.push(AccessNode { parent: parent.map(|group| places[group]), ..one.clone() });
        }
    }
    visible
}

/// A group being walked while nodes are built.
struct Group {
    /// Its node's index.
    node: u32,
    /// Its box, and the first box after it.
    start: u32,
    end: u32,
    /// For a table row: its next cell's column, and the first column after its table's, in the
    /// column list.
    next_column: u32,
    columns_end: u32,
}

/// Builds the nodes, a box at a time, in walking order.
struct NodeBuilder<'a> {
    tree: &'a Tree,
    /// The notes in box order, and their text.
    notes: &'a [Note],
    note_text: &'a str,
    /// The long-list rows described (`Ui::access_rows`), and how many were placed: the first
    /// node in such a row gives its position of how many.
    list_rows: &'a [(u32, [u32; 2])],
    rows_placed: usize,
    columns: &'a [Track],
    focus: Id,
    /// The box of the table cell keyed: focus is on it while on its table.
    focused_cell: Option<u32>,
    spelling: &'a Spelling,
    nodes: &'a mut Vec<AccessNode>,
    /// The groups being walked, innermost last.
    groups: Vec<Group>,
    /// The boxes inside a node read as one, which get no nodes of their own.
    merged: [u32; 2],
    /// A node without an id is known by the last node with one and how many without one came
    /// since: so it stays stable from run to run, whatever changes elsewhere.
    last_id: Id,
    since_id: u32,
}

impl NodeBuilder<'_> {
    /// Adds the nodes of one box walked: none, a cell, a node, or a cell and a node.
    fn add(&mut self, (at, rect, seen, visible): AccessBox) {
        let end = self.tree.specs[at as usize].end;
        self.enter(at);
        // (Content of a node read as one gets no node: except what a widget told apart.)
        let in_merged = at > self.merged[0] && at < self.merged[1];
        if in_merged && !notes_on(self.notes, at).any(|note| note.separate) {
            return;
        }
        let note = merged_note(self.notes, at);
        let note = note.as_ref();
        if note.is_some_and(|note| note.joined) {
            return self.merged = [at, end];
        }
        // A cell with only text is all; else its content is inside it.
        if self.add_cell(at, rect, seen, visible).is_some_and(|added| !added || !self.holds_parts(at, end)) {
            return self.merged = [at, end];
        }
        let Some(role) = self.role_of(note, at) else { return };
        // (Tree items the app put in no tree are told in one: so readers give their levels.)
        if role == Role::TreeItem && !self.groups.iter().any(|group| self.nodes[group.node as usize].role == Role::Tree) {
            self.add_tree(at, rect, seen, visible);
        }
        let (name, position) = (self.name_of(note, role, at), self.position_of(note, at));
        let note_text = self.note_text;
        let text = |range: [u32; 2]| &note_text[range[0] as usize..range[1] as usize];
        let (value, shortcut, description) = note.map_or(("", "", ""), |note| (text(note.value), text(note.shortcut), text(note.description)));
        let (id, parent) = (self.tree.specs[at as usize].id, self.groups.last().map(|group| group.node as usize));
        self.nodes.push(AccessNode {
            id: if id != 0 { id } else { hash(("said", self.last_id, self.since_id)) },
            value: value.to_string(),
            shortcut: shortcut.to_string(),
            description: description.to_string(),
            on: note.and_then(|note| note.on),
            expanded: note.and_then(|note| note.expanded),
            level: note.map_or(0, |note| note.level),
            value_range: note.and_then(|note| note.value_range),
            step: note.map_or(0.0, |note| note.step),
            position,
            grid: note.and_then(|note| note.grid),
            sorted: note.and_then(|note| note.sorted),
            caret: note.and_then(|note| note.caret),
            focused: id != 0 && id == self.focus,
            enabled: note.is_none_or(|note| note.enabled),
            visible,
            misspelled: misspellings(self.spelling, role, id, value),
            seen,
            at,
            ..AccessNode::plain(0, role, name, rect, parent)
        });
        (self.last_id, self.since_id) = if id != 0 { (id, 0) } else { (self.last_id, self.since_id + 1) };
        // (A node apart inside one read as one leaves the rest of that one unread.)
        let columns = note.and_then(|note| note.row).map_or([0; 2], |row| [row[1], row[2]]);
        match role.is_container() {
            true => self.groups.push(Group { node: self.nodes.len() as u32 - 1, start: at, end, next_column: columns[0], columns_end: columns[1] }),
            false if in_merged => {}
            false => self.merged = [at, end],
        }
    }

    /// Moves to box `at`: past the long-list rows that end before it, and out of the groups it
    /// is not in. (What lies over the page starts fresh: inside nothing before it.)
    fn enter(&mut self, at: u32) {
        let specs = &self.tree.specs;
        while self.list_rows.get(self.rows_placed).is_some_and(|(row, _)| specs[*row as usize].end <= at) {
            self.rows_placed += 1;
        }
        if specs[at as usize].frame.parent == NONE {
            self.merged = [0; 2];
        }
        while self.groups.last().is_some_and(|group| at < group.start || at >= group.end) {
            self.groups.pop();
        }
    }

    /// Adds the cell a box in a table row is, in the row's next column, named by its text.
    /// Says whether it was added: not for a hidden column, which does not show. (None for a box
    /// not in a table row.)
    fn add_cell(&mut self, at: u32, rect: Rect, seen: Rect, visible: bool) -> Option<bool> {
        let parent = self.tree.specs[at as usize].frame.parent;
        let row = self.groups.last_mut().filter(|group| group.next_column < group.columns_end && parent == group.start)?;
        row.next_column += 1;
        let (row, column) = (row.node as usize, row.next_column - 1);
        let Some(track) = self.columns.get(column as usize).filter(|track| track.shows) else { return Some(false) };
        let (id, grid) = (hash((self.nodes[row].id, "cell", column)), [self.nodes[row].position.map_or(0, |of| of[0] - 1), track.place]);
        let name = words_in(self.tree, at, usize::MAX);
        self.nodes.push(AccessNode { grid: Some(grid), focused: self.focused_cell == Some(at), visible, seen, at, ..AccessNode::plain(id, Role::Cell, name, rect, Some(row)) });
        self.groups.push(Group { node: self.nodes.len() as u32 - 1, start: at, end: self.tree.specs[at as usize].end, next_column: 0, columns_end: 0 });
        Some(true)
    }

    /// Adds a tree around tree item `at` and its siblings after it that hold tree items (the
    /// boxes their children are in): a hand-written tree. It lies where its first item does.
    fn add_tree(&mut self, at: u32, rect: Rect, seen: Rect, visible: bool) {
        let (specs, notes) = (&self.tree.specs, self.notes);
        let parent = specs[at as usize].frame.parent;
        let last = specs.get(parent as usize).map_or(specs.len() as u32, |parent| parent.end);
        let items = |from: u32| notes[notes.partition_point(|note| note.at < from)..].iter().take_while(|note| note.at < specs[from as usize].end).any(|note| note.role == Role::TreeItem);
        let mut end = specs[at as usize].end;
        while end < last && items(end) {
            end = specs[end as usize].end;
        }
        let (id, within) = (hash((specs[at as usize].id, "tree")), self.groups.last().map(|group| group.node as usize));
        self.nodes.push(AccessNode { visible, seen, at, ..AccessNode::plain(id, Role::Tree, String::new(), rect, within) });
        self.groups.push(Group { node: self.nodes.len() as u32 - 1, start: at, end, next_column: 0, columns_end: 0 });
    }

    /// Whether box `at` (up to `end`) holds anything with a node of its own: a note, or a
    /// focusable box.
    fn holds_parts(&self, at: u32, end: u32) -> bool {
        let noted = self.notes.get(self.notes.partition_point(|note| note.at < at)).is_some_and(|note| note.at < end);
        noted || self.tree.specs[at as usize..end as usize].iter().any(|spec| spec.frame.focus)
    }

    /// A box's role: as told, else a button where focusable, else text where it holds some.
    /// (None for a box that is none of these.)
    fn role_of(&self, note: Option<&Note>, at: u32) -> Option<Role> {
        let spec = &self.tree.specs[at as usize];
        match note {
            Some(note) => Some(note.role),
            None if spec.frame.focus => Some(Role::Button),
            None if matches!(spec.content, Content::Text { start, end, .. } if start < end) => Some(Role::Text),
            None => None,
        }
    }

    /// A node's name: as told, else its text: all of it, or for a dialog or group (a card) its
    /// first piece, its title. (A text box, combo box or slider is not named by its content: a
    /// number dragged sideways has its value in it. Nor are containers, except a table row,
    /// named by all its cells' text.) An operable node still without one (a slider, a combo
    /// box, a text box) is named by the text right before it, as system forms label a field.
    fn name_of(&self, note: Option<&Note>, role: Role, at: u32) -> String {
        let given = note.map_or("", |note| &self.note_text[note.name[0] as usize..note.name[1] as usize]);
        let wordy = matches!(role, Role::Dialog | Role::Group | Role::TableRow) || !role.is_container();
        let unnamed = given.is_empty() && wordy && !matches!(role, Role::TextBox | Role::PasswordBox | Role::ComboBox | Role::SpinButton | Role::Slider);
        let mut name = if unnamed { words_in(self.tree, at, if matches!(role, Role::Dialog | Role::Group) { 1 } else { usize::MAX }) } else { given.to_string() };
        if name.is_empty() && !role.is_container() && role != Role::Text {
            name = label_before(self.tree, self.notes, at).to_string();
        }
        name
    }

    /// A node's position in its set: a table row's among all its table's rows; else as told, or
    /// as its long-list row's.
    fn position_of(&mut self, note: Option<&Note>, at: u32) -> Option<[u32; 2]> {
        let parent = self.groups.last().map(|group| &self.nodes[group.node as usize]);
        let rows = parent.and_then(|table| table.grid.filter(|_| table.role == Role::Table)).map(|grid| grid[0]);
        let told = match note.and_then(|note| note.row) {
            Some([row, ..]) => Some([row + 1, rows.unwrap_or(row + 1)]),
            None => note.and_then(|note| note.position),
        };
        let in_list = self.list_rows.get(self.rows_placed).filter(|(row, _)| *row <= at).map(|(_, of)| *of);
        self.rows_placed += in_list.is_some() as usize;
        told.or(in_list)
    }
}

/// The letters of text box `at`, lying at `r`, as `Painter::words` draws them: those it had
/// (`was`) where laid out alike.
fn lay_letters(tree: &Tree, text: &mut dyn TextSystem, scale: f32, (at, r): (u32, Rect), was: Option<&Arc<Letters>>) -> Option<Arc<Letters>> {
    let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
    let Content::Text { start, end, font, wrap, side, .. } = spec.content else { return None };
    let (start, end) = (start as usize, end as usize);
    let inside = rect(0.0, 0.0, r.w, r.h).inset(spec.frame.pad);
    let asked = place.fit[0] - spec.frame.pad.horizontal();
    let wide = if side == Align::Start { asked } else { tree.text_width(at, scale).unwrap_or(asked) };
    let whole = [Line { start, end, width: wide }];
    let lines = if wrap { &tree.lines[place.lines.0 as usize..place.lines.1 as usize] } else { &whole[..] };
    let sizes = [font.size, scale, inside.x, inside.y, inside.w, inside.h, place.line_height].map(f32::to_bits);
    let key = hash((&tree.strings[start..end], font.face, sizes, side as u8, wrap));
    let key = lines.iter().fold(key, |key, line| fold(fold(key, (line.start - start) as u64 | ((line.end - start) as u64) << 32), line.width.to_bits() as u64));
    if let Some(was) = was.filter(|was| was.key == key) {
        return Some(was.clone());
    }
    let (high, before) = (place.line_height.max(1.0), [0.0, 0.5, 1.0, 0.0][side as usize]);
    let top = inside.y + if wrap { 0.0 } else { ((inside.h - place.line_height) / 2.0).max(0.0) };
    let (mut letters, mut edges) = (Letters { key, lines: Vec::new(), places: Vec::new() }, Vec::new());
    // (What a wrap leaves out, a line break or a space, lies at the end of the line before.)
    let (mut done, mut after) = (start, rect(inside.x, top, 0.0, high));
    for (row, line) in lines.iter().enumerate() {
        let y = top + row as f32 * high;
        letters.places.extend(std::iter::repeat_n(after, tree.strings[done..line.start.max(done)].chars().count()));
        letters.lines.push(letters.places.len() as u32);
        let paragraph = crate::text::Paragraph::of(&tree.strings[start..end], line.start - start..line.end - start);
        let leftward = paragraph.right_to_left;
        let x = inside.x + ((inside.w - line.width).max(0.0) * if leftward { 1.0 - before } else { before }).round();
        text.places(&tree.strings[line.start..line.end], font, scale, paragraph, &mut edges);
        letters.places.extend(edges.iter().map(|[left, right]| rect(x + left, y, right - left, high)));
        (after, done) = (rect(if leftward { x } else { x + line.width }, y, 0.0, high), line.end.max(done));
    }
    letters.places.extend(std::iter::repeat_n(after, tree.strings[done..end].chars().count()));
    if letters.lines.first() != Some(&0) {
        letters.lines.insert(0, 0);
    }
    Some(Arc::new(letters))
}

/// Gives a node read by line and letter its letters, where it shows and its text is one box's:
/// that box, the first in it holding all of its text (`boxes` start at the node's own). Those
/// it had (`was`) where laid out alike. (Letters are of use only where they show.)
fn lay_node_letters(node: &mut AccessNode, boxes: &[AccessBox], tree: &Tree, text: &mut dyn TextSystem, scale: f32, was: Option<&Arc<Letters>>) {
    (node.letters, node.text_box, node.text_rect) = (None, 0, Rect::default());
    let (end, wanted) = (tree.specs[node.at as usize].end, node.text());
    if !node.visible || !node.reads_text() || wanted.is_empty() {
        return;
    }
    let holds = |at: u32| matches!(tree.specs[at as usize].content, Content::Text { start, end, .. } if &tree.strings[start as usize..end as usize] == wanted);
    let Some(&(text_box, r, ..)) = boxes.iter().take_while(|(at, ..)| *at < end).find(|(at, ..)| holds(*at)) else { return };
    (node.letters, node.text_box, node.text_rect) = (lay_letters(tree, text, scale, (text_box, r), was), text_box, r);
}

/// Gives the nodes that show and are read by line and letter their letters (see
/// `lay_node_letters`), taking those laid out alike from the node of the same id in the last
/// list (`old`: its nodes with letters), found near where the last was.
fn lay_all_letters(nodes: &mut [AccessNode], old: &[(Id, Arc<Letters>)], boxes: &[AccessBox], tree: &Tree, text: &mut dyn TextSystem, scale: f32) {
    let (mut next, mut near) = (0, 0);
    for node in nodes.iter_mut() {
        let Some(found) = boxes[next..].iter().position(|(at, ..)| *at == node.at) else { continue };
        next += found;
        if !node.visible || !node.reads_text() {
            continue;
        }
        let was = old.get(near..).unwrap_or_default().iter().take(64).position(|(id, _)| *id == node.id).map(|found| near + found);
        near = was.map_or(near, |was| was + 1);
        lay_node_letters(node, &boxes[next..], tree, text, scale, was.map(|was| &old[was].1));
    }
}

/// Brings a node's letters along as it moves (`boxes` start at its own): where it still shows,
/// they lie where its text's box now does; come into view, they are laid out; gone out of it,
/// it has none.
fn follow_letters(node: &mut AccessNode, boxes: &[AccessBox], tree: &Tree, text: &mut dyn TextSystem, scale: f32) {
    let shown = node.letters.take().filter(|_| node.visible);
    let text_box = shown.as_ref().and_then(|_| boxes.iter().find(|(at, ..)| *at == node.text_box));
    match (shown, text_box) {
        (Some(letters), Some(found)) => (node.letters, node.text_rect) = (Some(letters), found.1),
        (was, _) => lay_node_letters(node, boxes, tree, text, scale, was.as_ref()),
    }
}

/// Gives each table its selected rows: as told (`told`, by box), else those of its rows told.
#[inline(always)]
fn select_rows(nodes: &mut [AccessNode], told: &[(u32, Vec<[u32; 2]>)]) {
    for table in 0..nodes.len() {
        if nodes[table].role != Role::Table {
            continue;
        }
        if let Some((_, rows)) = told.iter().rfind(|(at, _)| *at == nodes[table].at) {
            nodes[table].selected_rows.clone_from(rows);
            continue;
        }
        let picked = nodes[table + 1..].iter().filter(|row| row.parent == Some(table) && row.role == Role::TableRow && row.on == Some(true));
        let rows: Vec<[u32; 2]> = picked.filter_map(|row| row.position).map(|[nth, _]| [nth - 1, nth]).collect();
        nodes[table].selected_rows = rows;
    }
}

/// Names each cell without text by its content's names.
#[inline(always)]
fn name_cells_by_content(nodes: &mut [AccessNode]) {
    for cell in 0..nodes.len() {
        if nodes[cell].role != Role::Cell || !nodes[cell].name.is_empty() {
            continue;
        }
        let inner = nodes[cell + 1..].iter().take_while(|inner| inner.parent.is_some_and(|parent| parent >= cell));
        let names: Vec<&str> = inner.filter(|inner| inner.parent == Some(cell) && !inner.name.is_empty()).map(|inner| inner.name.as_str()).collect();
        nodes[cell].name = names.join(", ");
    }
}

/// Places each tree item that did not give its position (a hand-written one) among its
/// siblings of the same depth under the same parent.
#[inline(always)]
fn place_tree_items(nodes: &mut [AccessNode]) {
    for item in 0..nodes.len() {
        if nodes[item].role != Role::TreeItem || nodes[item].position.is_some() {
            continue;
        }
        let (level, parent) = (nodes[item].level, nodes[item].parent);
        let beside = |other: &&AccessNode| other.role == Role::TreeItem && other.parent == parent;
        let before = nodes[..item].iter().rev().filter(beside).take_while(|other| other.level >= level).filter(|other| other.level == level).count();
        let after = nodes[item + 1..].iter().filter(beside).take_while(|other| other.level >= level).filter(|other| other.level == level).count();
        nodes[item].position = Some([before as u32 + 1, (before + after) as u32 + 1]);
    }
}

/// Places each radio button among its group's radio buttons.
#[inline(always)]
fn place_radio_buttons(nodes: &mut [AccessNode], rovers: &[Rover]) {
    for one in nodes.iter_mut().filter(|one| one.role == Role::RadioButton && one.position.is_none()) {
        let Some(group) = rovers.iter().find(|rover| rover.id == one.id && rover.selects).map(|rover| rover.group) else { continue };
        let mut choices = rovers.iter().filter(|rover| rover.group == group);
        let (count, nth) = (choices.clone().count(), choices.position(|rover| rover.id == one.id).unwrap_or(0));
        one.position = Some([nth as u32 + 1, count as u32]);
    }
}

/// What screen readers are told, and what tests find things by.
impl Ui {
    /// A screen reader starts or stops listening to this window: from the next run on,
    /// [`visible_access_nodes`](Self::visible_access_nodes) is kept up to date, or not.
    pub fn set_screen_reader(&mut self, listening: bool) {
        (self.listening, self.again) = (listening, true);
        if listening {
            UPDATE_ACCESS_NODES.get_or_init(|| Ui::update_access_nodes);
        }
        if !listening {
            (self.access_nodes, self.visible_access_nodes, self.visible_boxes.access_cache) = (Vec::new(), OnceCell::new(), Vec::new());
            // (Nor do the second windows keep theirs: they hear of it at their next run, which
            // may be far off.)
            self.each_window(|window| (window.access_nodes, window.visible_access_nodes, window.visible_boxes.access_cache) = (Vec::new(), OnceCell::new(), Vec::new()));
        }
    }
    /// Which nodes these are: it changes whenever they may have. (0 while nobody listens.)
    pub(crate) fn access_version(&self) -> u64 {
        self.visible_boxes.access_cache.first().map_or(0, |cache| cache.version)
    }
    /// Which nodes these were moved from, where only where they lie and what of them shows (and
    /// so which have letters) changed since: their version. A copy of those follows with `follow_move`.
    pub(crate) fn access_moved_from(&self) -> Option<u64> {
        self.visible_boxes.access_cache.first().map(|cache| cache.moved_from).filter(|from| *from != 0)
    }
    /// What a screen reader is told of this window that is visible, as of the last run:
    /// elements in order, each group before its content. (`access_nodes` has all of it, visible
    /// or not.)
    pub fn visible_access_nodes(&self) -> &[AccessNode] {
        self.visible_access_nodes.get_or_init(|| visible_nodes(&self.access_nodes))
    }
    /// What a screen reader is told of this window, as of the last run: everything in it,
    /// visible or not ([`AccessNode::visible`]), in order, each group before its content. So a
    /// user who cannot see the window can reach a whole page with the reader's own keys, an
    /// element scrolled out of view is told as offscreen, and scrolled into view when the
    /// reader asks ([`show_for_reader`](Self::show_for_reader)). (A long list describes only
    /// visible rows, so only those are told: each selected one says its position of how many.)
    pub fn access_nodes(&self) -> &[AccessNode] {
        &self.access_nodes
    }

    /// Says what box `of` is, where it is not what it would be taken for: its role, whether it
    /// is on (checked, selected, expanded), and a name where its text does not say ("" leaves
    /// it to the text). Called right after the box is made.
    #[doc(alias("accessibility", "aria"))]
    pub fn tell(&mut self, of: &Response, role: Role, on: Option<bool>, name: &str) {
        self.keep_note(of, role, on, name, "");
    }
    /// The same for a text box: its purpose and its content.
    pub fn tell_text(&mut self, of: &Response, name: &str, value: &str) {
        self.keep_note(of, Role::TextBox, None, name, value);
    }
    /// The same for a combo box: its chosen item.
    pub fn tell_choice(&mut self, of: &Response, open: bool, chosen: &str) {
        self.keep_note(of, Role::ComboBox, Some(open), "", chosen);
    }
    /// Adds to the last told element: its keyboard shortcut ("Ctrl+O"), for a screen reader to
    /// announce with it.
    pub fn tell_shortcut(&mut self, keys: &str) {
        let Ui { notes, noted, listening, .. } = self;
        if let Some(note) = notes.last_mut().filter(|_| *listening) {
            note.shortcut = push_text(noted, keys);
        }
    }
    /// Adds to the last told element: what more it says than its name (a link's title), for a
    /// screen reader to read after it.
    pub fn tell_description(&mut self, words: &str) {
        let Ui { notes, noted, listening, .. } = self;
        if let Some(note) = notes.last_mut().filter(|_| *listening) {
            note.description = push_text(noted, words);
        }
    }
    /// Says box `of` is told on its own, though inside an element told as one: a button in a
    /// list row, a tab's close button. (Content of an element told as one, such as a
    /// [`Role::Row`], is otherwise part of it: its text is the row's name, and nothing in it is
    /// reachable alone.) Called right after the box is made, and after anything else said of
    /// it.
    ///
    /// ```
    /// use taut::testing::Trial;
    /// use taut::{Role, Ui};
    ///
    /// fn describe(opened: &mut u32, ui: &mut Ui) {
    ///     let row = ui.row().key("report").gap(8.0).sense().focusable().open();
    ///     ui.label("Report.pdf");
    ///     let open = ui.button("Open");
    ///     ui.tell_separately(&open);
    ///     ui.close();
    ///     ui.tell(&row, Role::Row, Some(false), "");
    ///     *opened += open.clicked as u32;
    /// }
    ///
    /// let mut trial = Trial::new(0, describe);
    /// assert!(trial.has(Role::Row, "Report.pdf, Open") && trial.has(Role::Button, "Open"));
    /// trial.press(Role::Button, "Open");
    /// assert_eq!(trial.app, 1);
    /// ```
    pub fn tell_separately(&mut self, of: &Response) {
        let Some(at) = self.noted_box(of) else { return };
        // (Its widget's note, if any: among the last few notes.)
        if let Some(note) = self.notes.iter_mut().rev().take_while(|note| note.at >= at).find(|note| note.at == at) {
            note.separate = true;
            return;
        }
        // Else it is told as what it would be taken for: a button, or text.
        let spec = &self.tree.specs[at as usize];
        let role = match spec.content {
            _ if spec.frame.focus => Role::Button,
            Content::Text { start, end, .. } if start < end => Role::Text,
            _ => Role::Group,
        };
        self.keep_note(of, role, None, "", "");
        if let Some(note) = self.notes.last_mut() {
            note.separate = true;
        }
    }
    /// Adds to text box `of`: whether a list of completions is open under it (a box a screen
    /// reader is then told it can expand).
    pub(crate) fn tell_listed(&mut self, of: &Response, open: bool) {
        if !self.listening {
            return;
        }
        let at = self.tree.specs.iter().rposition(|spec| spec.id == of.id).map(|at| at as u32);
        if let Some(note) = self.notes.iter_mut().rev().find(|note| Some(note.at) == at && note.role == Role::TextBox) {
            note.on = Some(open);
        }
    }
    /// Adds to the last told element: it is the `nth` of `count`, from 1. (A long list's row,
    /// where only visible rows are told.)
    pub fn tell_position(&mut self, nth: usize, count: usize) {
        if let Some(note) = self.last_note_mut() {
            note.position = Some([nth as u32, count as u32]);
        }
    }
    /// Adds to the last told element, a tree item: its depth (1 at the top), and whether its
    /// children show (none for an item without).
    pub fn tell_tree_item(&mut self, level: usize, open: Option<bool>) {
        if let Some(note) = self.last_note_mut() {
            (note.level, note.expanded) = (level as u32, open);
        }
    }
    /// Says the last made box holds some of the words of `all`, which a widget writes a word
    /// per box (`words`): the first is read as all of them, the rest as part of it. `before` is
    /// the note count before the first.
    pub(crate) fn tell_words(&mut self, all: &str, before: usize) {
        if !self.listening {
            return;
        }
        if self.notes.len() == before {
            return self.keep_note(&Response::default(), Role::Text, None, all.trim(), "");
        }
        self.keep_note(&Response::default(), Role::Text, None, "", "");
        if let Some(note) = self.notes.last_mut() {
            note.joined = true;
        }
    }
    /// Says box `of` is a table of so many visible columns. Its rows are counted as described
    /// (`table_row_count`).
    pub(crate) fn tell_table(&mut self, of: &Response, columns: usize) {
        self.keep_note(of, Role::Table, None, "", "");
        if let Some(note) = self.last_note_mut() {
            note.grid = Some([0, columns as u32]);
        }
    }
    /// The index of the note for the table told as box `at`, whose columns are `columns` in
    /// their list. (None while nobody listens.)
    pub(crate) fn table_note(&self, at: u32, columns: [u32; 2]) -> Option<usize> {
        // (This table's last row says where its note is: so only that row's notes are searched
        // back, not every earlier row's.)
        let ours = |note: &Note| (note.at, note.role) == (at, Role::Table) || note.row.is_some_and(|row| [row[1], row[2]] == columns);
        let found = self.notes.iter().rposition(ours)?;
        Some(self.notes[found].row.map_or(found, |row| row[3] as usize))
    }
    /// How many rows a note's table has been told to have so far, to count on.
    pub(crate) fn table_row_count(&mut self, table: usize) -> Option<&mut u32> {
        self.notes.get_mut(table)?.grid.as_mut().map(|grid| &mut grid[0])
    }
    /// Says the next made box is a table row: its index of all rows from 0; the range of the
    /// table's columns in their list; and the table's own note's index.
    pub(crate) fn tell_row(&mut self, row: u32, [first, after]: [u32; 2], table: usize) {
        let (at, end) = (self.tree.specs.len() as u32, self.noted.len() as u32);
        let note = Note { row: Some([row, first, after, table as u32]), ..Note::new(at, Role::TableRow, [end; 2], [end; 2], self.enabled) };
        self.notes.push(note);
    }
    /// Adds to the last told table row: it is selectable, and whether selected.
    pub(crate) fn tell_selected(&mut self, selected: bool) {
        if let Some(note) = self.notes.last_mut().filter(|note| note.role == Role::TableRow) {
            note.on = Some(selected);
        }
    }
    /// Adds to the last told heading: whether the rows are sorted by its column, and if so
    /// whether descending.
    pub(crate) fn tell_sorted(&mut self, sorted: Option<bool>) {
        if let Some(note) = self.last_note_mut() {
            note.sorted = sorted;
        }
    }
    /// Adds to the last told text box: its selection anchor and cursor, in characters.
    pub(crate) fn tell_caret(&mut self, caret: [u32; 2]) {
        if let Some(note) = self.last_note_mut() {
            note.caret = Some(caret);
        }
    }
    /// The same for something that stands for a value: a slider, or a progress bar. Its value,
    /// minimum and maximum.
    pub fn tell_value(&mut self, of: &Response, role: Role, amount: [f32; 3]) {
        self.tell_value_named(of, role, amount, "");
    }
    /// The same, with a name: a library widget without text.
    pub(crate) fn tell_value_named(&mut self, of: &Response, role: Role, range: [f32; 3], name: &str) {
        self.keep_note(of, role, None, name, "");
        if let Some(note) = self.last_note_mut() {
            note.value_range = Some(range);
        }
    }
    /// Retells box `of`, already told (as a text box, or by its name), as of `role` with a
    /// value, minimum and maximum. (A number box is a spin button.)
    pub(crate) fn retell_as_value(&mut self, of: &Response, role: Role, range: [f32; 3]) {
        let Ui { notes, tree, listening, .. } = self;
        if let Some(note) = notes.iter_mut().rev().find(|note| *listening && tree.specs[note.at as usize].id == of.id) {
            (note.role, note.value_range) = (role, Some(range));
        }
    }
    /// Adds to the last told element, a document heading: its level, 1 for the main one.
    pub fn tell_level(&mut self, level: usize) {
        if let Some(note) = self.last_note_mut() {
            note.level = level as u32;
        }
    }
    /// Adds to the last told slider: its step.
    pub fn tell_step(&mut self, step: f32) {
        if let Some(note) = self.last_note_mut() {
            note.step = step;
        }
    }
    /// Names box `of` `name`, whatever was told of it before: a widget beside a label saying
    /// what it is (an inspector row). Called right after the widget.
    pub(crate) fn tell_name(&mut self, of: &Response, name: &str) {
        let Some(at) = self.tree.specs.iter().rposition(|spec| spec.id == of.id).filter(|_| self.listening && of.id != 0) else { return };
        let Ui { notes, noted, tree, .. } = self;
        match notes.iter_mut().rev().find(|note| note.at == at as u32) {
            Some(note) => note.name = push_text(noted, name),
            None if tree.specs[at].frame.focus => self.keep_note(of, Role::Button, None, name, ""),
            None => {}
        }
    }
    /// Notes box `of`'s role, state, name and value, while a screen reader listens.
    pub(crate) fn keep_note(&mut self, of: &Response, role: Role, on: Option<bool>, name: &str, value: &str) {
        let Some(at) = self.noted_box(of) else { return };
        let (name, value) = (push_text(&mut self.noted, name), push_text(&mut self.noted, value));
        self.notes.push(Note { on, ..Note::new(at, role, name, value, self.enabled) });
    }
    /// The box `of` was made as, while a screen reader listens: the last made where it has no
    /// id (a picture: called right after it), else the last with its id (among the last few).
    fn noted_box(&self, of: &Response) -> Option<u32> {
        if !self.listening {
            return None;
        }
        let last = self.tree.specs.len().checked_sub(1).filter(|_| of.id == 0);
        last.or_else(|| self.tree.specs.iter().rposition(|spec| spec.id == of.id)).map(|at| at as u32)
    }
    /// The last note, while a screen reader listens.
    fn last_note_mut(&mut self) -> Option<&mut Note> {
        self.notes.last_mut().filter(|_| self.listening)
    }

    /// Updates the nodes after a run, while a screen reader listens. Where nothing they are
    /// built from changed, they are kept; where only boxes moved, they are moved.
    pub(crate) fn update_access_nodes(&mut self) {
        let key = self.inputs_hash();
        let mut cache = self.visible_boxes.access_cache.pop().unwrap_or_default();
        // (The key has the text's length only. Where nothing moved, the text is as it was.)
        let same = cache.version != 0 && cache.key == key && cache.notes == self.notes && cache.note_text == self.noted;
        let same = same && (!self.boxes_moved || cache.text == self.tree.strings);
        let moved = same && self.boxes_moved;
        if moved {
            cache.moved += 1;
            self.notes.sort_by_key(|note| note.at);
        }
        // (Nodes whose boxes were not all found again are built afresh: not seen to happen.)
        let kept = same && (!moved || self.move_nodes(&mut cache.boxes));
        if !same {
            cache.key = key;
            cache.notes.clone_from(&self.notes);
            cache.note_text.clone_from(&self.noted);
            cache.text.clone_from(&self.tree.strings);
        }
        if !kept {
            cache.built += 1;
            self.build_nodes(&mut cache.boxes);
        }
        if !same || moved {
            cache.moved_from = if moved && kept { cache.version } else { 0 };
            cache.version = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
        }
        // (A debug build checks nodes kept or moved against nodes built afresh.)
        #[cfg(debug_assertions)]
        if kept {
            let kept = self.access_nodes.clone();
            self.build_nodes(&mut cache.boxes);
            assert!(same_nodes(&kept, &self.access_nodes), "the accessibility tree was kept where it changed");
        }
        self.visible_boxes.access_cache.push(cache);
    }

    /// A hash of what the nodes are built from, but where boxes lie and the text: the boxes
    /// (their layout and ids), focus, the cell keyed, columns, radio groups, long-list rows,
    /// the layers walked, and misspellings.
    fn inputs_hash(&self) -> u64 {
        let Ui { tree, visible_boxes: survey, focus, grids, tracks, rovers, access_rows: rows_told, notes, asks, layout_key, table_reading, .. } = self;
        let mut key = fold(fold(*layout_key, *focus), grids.keyed_box().map_or(u64::MAX, u64::from));
        key = tree.specs.iter().fold(key, |key, spec| fold(key, spec.id));
        key = survey.roots.get(survey.reader_root..).unwrap_or_default().iter().fold(key, |key, root| fold(key, *root as u64));
        key = tracks.iter().fold(key, |key, track| fold(key, track.place as u64 | (track.shows as u64) << 32));
        key = rovers.iter().fold(key, |key, rover| fold(fold(fold(key, rover.id), rover.group), rover.selects as u64));
        key = rows_told.iter().fold(key, |key, (row, [nth, count])| fold(fold(key, *row as u64), (*nth as u64) << 32 | *count as u64));
        for (table, rows) in &table_reading.told {
            key = rows.iter().fold(fold(key, *table as u64), |key, [from, to]| fold(key, (*from as u64) << 32 | *to as u64));
        }
        // (Misspellings are found apart from the run: in every text box.)
        let Some(hooks) = asks.spelling.hooks() else { return key };
        for note in notes.iter().filter(|note| note.role == Role::TextBox) {
            let wrong = (hooks.wrong)(&asks.spelling, tree.specs[note.at as usize].id);
            key = wrong.iter().fold(fold(key, note.at as u64), |key, word| fold(key, (word.start as u64) << 32 | word.end as u64));
        }
        key
    }

    /// Moves each node to where its box now lies, with what of it shows. Says whether every
    /// node's box was found, in order. (Notes sorted.)
    fn move_nodes(&mut self, boxes: &mut Vec<AccessBox>) -> bool {
        let window = rect(0.0, 0.0, self.size().w, self.size().h);
        let Ui { visible_boxes: survey, tree, notes, access_nodes: nodes, visible_access_nodes: visible, text, scale, .. } = self;
        walk_access_boxes(survey, boxes, tree, notes, window);
        *visible = OnceCell::new();
        // (A cell and what is in it are one box: the next node may be of the same box.)
        let mut next = 0;
        for one in nodes.iter_mut() {
            let Some(found) = boxes[next..].iter().position(|(at, ..)| *at == one.at) else { return false };
            next += found;
            (_, one.rect, one.seen, one.visible) = boxes[next];
            follow_letters(one, &boxes[next..], tree, text.as_mut(), *scale);
        }
        true
    }

    /// Builds the nodes afresh, from the boxes (walked into `boxes`) and the notes; their
    /// letters from the last ones', where laid out alike.
    fn build_nodes(&mut self, boxes: &mut Vec<AccessBox>) {
        let window = rect(0.0, 0.0, self.size().w, self.size().h);
        let Ui {
            visible_boxes: survey,
            tree,
            notes,
            noted,
            access_nodes: nodes,
            visible_access_nodes: visible,
            access_rows: rows_told,
            focus,
            tracks,
            grids,
            rovers,
            asks,
            text,
            scale,
            table_reading,
            writings,
            ..
        } = self;
        notes.sort_by_key(|note| note.at);
        let old: Vec<(Id, Arc<Letters>)> = nodes.iter().filter_map(|node| Some((node.id, node.letters.clone()?))).collect();
        nodes.clear();
        *visible = OnceCell::new();
        walk_access_boxes(survey, boxes, tree, notes, window);
        let mut builder = NodeBuilder {
            tree,
            notes,
            note_text: noted,
            list_rows: rows_told,
            rows_placed: 0,
            columns: tracks,
            focus: *focus,
            focused_cell: grids.keyed_box(),
            spelling: &asks.spelling,
            nodes,
            groups: Vec::new(),
            merged: [0; 2],
            last_id: 0,
            since_id: 0,
        };
        boxes.iter().for_each(|&one| builder.add(one));
        name_cells_by_content(nodes);
        place_tree_items(nodes);
        place_radio_buttons(nodes, rovers);
        lay_all_letters(nodes, &old, boxes, tree, text.as_mut(), *scale);
        // (Vertical text is drawn as shapes: it tells where its letters are itself.)
        if let Some(vertical) = crate::vertical_paragraph::VERTICAL.get() {
            (vertical.tell_letters)(writings, nodes, boxes);
        }
        select_rows(nodes, &table_reading.told);
    }

    /// Presses element `id` for a screen reader that asked: focus goes to it, and it is pressed
    /// as Enter presses it. (Then the interface runs.)
    pub fn press_for_reader(&mut self, id: Id) {
        self.set_focus(id);
        self.input.strokes.push(Stroke::Key(Key::Enter, Mods::NONE));
    }
    /// Scrolls element `id` into view for a screen reader that asked: its containers scroll so
    /// all of it shows, or as much as fits, as for keyboard focus (which does not move to it).
    /// (Then the interface runs.)
    pub fn show_for_reader(&mut self, id: Id) {
        let Some(node) = self.access_nodes.iter().find(|node| node.id == id).filter(|node| (node.at as usize) < self.tree.specs.len()) else { return };
        let (at, rect) = (node.at, node.rect);
        self.show(at, rect);
    }
    /// Steps a slider for a screen reader: up, or down.
    pub fn step_for_reader(&mut self, id: Id, more: bool) {
        self.set_focus(id);
        self.input.strokes.push(Stroke::Key(if more { Key::Right } else { Key::Left }, Mods::NONE));
    }
    /// Expands or collapses a tree item for a screen reader: focus goes to it, and it expands
    /// as Right expands a collapsed one, or collapses as Left collapses an expanded one. (Asked
    /// only of an item not already so: those keys would move on to its neighbours.)
    pub fn open_for_reader(&mut self, id: Id, open: bool) {
        self.set_focus(id);
        self.input.strokes.push(Stroke::Key(if open { Key::Right } else { Key::Left }, Mods::NONE));
    }
    /// Adds a tree item to the selection for a screen reader, or removes it: focus goes to it,
    /// and Control+Space is pressed.
    pub fn pick_for_reader(&mut self, id: Id) {
        self.set_focus(id);
        self.input.strokes.push(Stroke::Key(Key::Space, Mods::CTRL));
    }
    /// Adds to the table being described: which of all its rows are selected, shown or not. So
    /// a screen reader is told every selected row of a long table, not only of those described.
    /// Called anywhere inside the table.
    pub fn tell_selection(&mut self, selection: &crate::Selection) {
        let Some(table) = self.open_table_note() else { return };
        let rows = selection.spans().iter().map(|rows| [rows.start as u32, rows.end as u32]).collect();
        self.table_reading.told.push((self.notes[table].at, rows));
    }
    /// Has a table's row (from 0) show for a screen reader that asked: scrolled into view where
    /// it is a long list's. With `pick`, it is then selected alone (`Some(true)`), as a click
    /// does, or added to the selection or taken out of it, as a Control+click does.
    pub(crate) fn row_for_reader(&mut self, table: Id, row: u32, pick: Option<bool>) {
        let at = self.access_nodes.iter().position(|node| node.id == table && node.role == Role::Table);
        let told = at.and_then(|at| self.access_nodes[at + 1..].iter().find(|node| node.parent == Some(at) && node.role == Role::TableRow && node.position.is_some_and(|of| of[0] == row + 1)));
        match told.map(|node| node.id).filter(|_| pick.is_none()) {
            Some(id) => self.show_for_reader(id),
            None => (self.table_reading.asked, self.again) = (Some((table, row, pick)), true),
        }
    }
    /// The row of a long list a screen reader asked for (`row_for_reader`), if in this one: its
    /// index among the list's `count`. The list is in the table told by note `table`, after
    /// `before` of its rows. (A row to select is still asked for until described.)
    pub(crate) fn reader_row(&mut self, table: Option<usize>, before: Option<u32>, count: usize) -> Option<usize> {
        let ((id, row, pick), table, before) = (self.table_reading.asked?, table?, before?);
        let index = row.checked_sub(before).filter(|index| (*index as usize) < count && self.tree.specs[self.notes[table].at as usize].id == id)?;
        if pick.is_none() {
            self.table_reading.asked = None;
        }
        Some(index as usize)
    }
    /// Whether the table row told by note `note` is the one a screen reader asked to select,
    /// and how: alone (true) or toggled. Asked once.
    pub(crate) fn reader_picks(&mut self, note: usize) -> Option<bool> {
        let [row, _, _, table] = self.notes.get(note)?.row?;
        let (id, asked, pick) = self.table_reading.asked?;
        let pick = pick.filter(|_| self.tree.specs[self.notes[table as usize].at as usize].id == id && asked == row)?;
        self.table_reading.asked = None;
        Some(pick)
    }

    /// Has a screen reader announce something the user should hear without looking, or that
    /// shows nowhere: "Saved", "3 files copied". Said once, after this run. (To nobody, and
    /// kept nowhere, while no screen reader listens.)
    pub fn announce(&mut self, text: &str) {
        if self.listening {
            self.announced.push(text.into());
        }
    }
    /// For the window code: what the app announced during the last run.
    pub fn announced(&self) -> &[String] {
        &self.announced
    }

    /// Sets a slider to `amount` for a screen reader, or as near as its range allows: focus
    /// goes to it, and it is there after the next run.
    pub fn set_for_reader(&mut self, id: Id, amount: f32) {
        self.set_focus(id);
        self.reader_sets = Some((id, amount));
    }
    /// Selects part of a text box's text for a screen reader: from one character to another,
    /// from the start (the same twice places the cursor). Focus goes to the box, which shows it
    /// after the next run. (Not in a password box.)
    pub fn select_for_reader(&mut self, id: Id, [from, to]: [u32; 2]) {
        let Some(text) = self.access_nodes.iter().find(|node| node.id == id && node.role == Role::TextBox).map(|node| node.value.as_str()) else { return };
        let byte = |nth: u32| text.char_indices().nth(nth as usize).map_or(text.len(), |(at, _)| at);
        self.reader_selects = Some((id, [byte(from), byte(to)], false));
        self.set_focus(id);
    }
    /// Replaces a text box's text with `text` for a screen reader.
    pub fn type_for_reader(&mut self, id: Id, text: &str) {
        self.set_focus(id);
        self.input.strokes.push(Stroke::Key(Key::Char('a'), Mods::CTRL));
        self.input.strokes.push(Stroke::Text(text.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pt;
    use crate::testing::Trial;

    /// A long page in a box that scrolls: rows of a name, a tick box and a button.
    struct Page {
        first: String,
        ticked: bool,
        keyed_from: usize,
    }

    fn describe(page: &mut Page, ui: &mut Ui) {
        ui.column().grow().scroll().gap(4.0).open();
        for row in 0..200 {
            ui.row().key(row + page.keyed_from).gap(8.0).open();
            ui.label(if row == 0 { &page.first } else { "a row" });
            match row {
                0 => ui.checkbox(&mut page.ticked, "First"),
                _ => ui.checkbox(&mut false, "Done"),
            };
            ui.button("Open");
            ui.close();
        }
        ui.close();
    }

    /// A step taken in a trial.
    type Step = fn(&mut Trial<Page>);

    /// Times the nodes were built and moved, and their version.
    fn counts(trial: &Trial<Page>) -> (u32, u32, u64) {
        let cache = trial.ui.visible_boxes.access_cache.first().expect("a cache kept while a screen reader listens");
        (cache.built, cache.moved, trial.ui.access_version())
    }

    #[test]
    fn a_run_that_changes_nothing_told_tells_nothing_anew() {
        let mut trial = Trial::new(Page { first: "a row".into(), ticked: false, keyed_from: 0 }, describe);
        let (built, moved, version) = counts(&trial);
        let nodes = trial.access_nodes().to_vec();
        assert!(nodes.len() == 600 && nodes.iter().any(|node| !node.visible), "every row has nodes, shown or not");
        for _ in 0..3 {
            trial.run();
        }
        assert_eq!((counts(&trial), trial.access_nodes()), ((built, moved, version), &nodes[..]));
        // Scrolled, or hovered: the same nodes, moved. (A debug build checks nodes kept or
        // moved against nodes built afresh.)
        trial.wheel(pt(100.0, 100.0), 0.0, 400.0);
        let now = counts(&trial);
        assert!(now.0 == built && now.1 > moved && now.2 != version, "{now:?}");
        assert!(trial.access_nodes().len() == nodes.len() && trial.access_nodes() != nodes, "the nodes moved");
        trial.run();
        assert_eq!(counts(&trial), now);
        trial.wheel(pt(100.0, 100.0), 0.0, -400.0);
        assert_eq!(counts(&trial).0, built);
        // What changes the tree builds it anew: a name, a state, focus, which box is which.
        let steps: [(&str, Step); 4] = [
            ("a name", |trial| {
                trial.app.first = "the first row".into();
                trial.run();
            }),
            ("a state", |trial| trial.press(Role::CheckBox, "First")),
            ("focus", |trial| trial.key("Tab")),
            ("ids", |trial| {
                trial.app.keyed_from = 1000;
                trial.run();
            }),
        ];
        for (what, step) in steps {
            let was = counts(&trial);
            step(&mut trial);
            let now = counts(&trial);
            assert!(now.0 > was.0 && now.2 != was.2, "{what}: {was:?} then {now:?}");
            trial.run();
            assert_eq!(counts(&trial), now, "{what}, then nothing");
        }
        assert!(trial.app.ticked && trial.has(Role::Text, "the first row"));
        // A screen reader that stops listening leaves nothing kept; one that starts again gets
        // the tree anew.
        trial.ui.set_screen_reader(false);
        trial.run();
        assert!(trial.ui.visible_boxes.access_cache.is_empty() && trial.access_nodes().is_empty() && trial.ui.access_version() == 0);
        trial.ui.set_screen_reader(true);
        trial.run();
        assert_eq!(counts(&trial).0, 1);
        assert_eq!(trial.access_nodes().len(), nodes.len());
    }

    const LONG: &str = "one two three four five six seven eight nine ten eleven twelve";

    /// A wrapping paragraph and a text area in a narrow box that scrolls.
    fn letters_page(notes: &mut String, ui: &mut Ui) {
        ui.column().w(crate::Length::Px(200.0)).h(crate::Length::Px(200.0)).scroll().open();
        ui.paragraph(LONG);
        ui.text_area(notes, "Notes", (3, 6));
        ui.el().h(crate::Length::Px(600.0)).show();
        ui.close();
    }

    #[test]
    fn text_is_read_by_its_lines_as_wrapped_and_its_letters_where_drawn() {
        let mut trial = Trial::new(String::from("first\nsecond"), letters_page);
        let check = |trial: &Trial<String>| {
            let text = trial.find(Role::Text, LONG);
            let lines = text.line_starts();
            assert!(lines.len() > 2 && lines[0] == 0, "{lines:?}");
            // Each line starts after a space, at the left; its letters go right, a line down.
            for (row, start) in lines.iter().enumerate() {
                let first = text.letter_rect(*start);
                assert_eq!(LONG.as_bytes()[*start as usize - (row > 0) as usize], if row > 0 { b' ' } else { b'o' });
                assert_eq!((first.x, first.y), (text.letter_rect(0).x, text.letter_rect(0).y + row as f32 * first.h));
                assert!(text.letter_rect(start + 1).x > first.x);
            }
            // Every letter is found where it is drawn.
            for letter in 0..LONG.len() as u32 {
                let place = text.letter_rect(letter);
                assert!(place.w == 0.0 || text.letter_at(pt(place.x + place.w / 2.0, place.y + place.h / 2.0)) == Some(letter), "{letter}");
            }
            // A text area by its typed lines, its text where the box draws it.
            let area = trial.find(Role::TextBox, "Notes");
            assert_eq!(area.line_starts(), [0, 6]);
            assert!(area.letter_rect(6).y > area.letter_rect(0).y && area.rect.contains(pt(area.letter_rect(0).x, area.letter_rect(0).y)));
            (text.letter_rect(0), text.letters.clone())
        };
        let (first, letters) = check(&trial);
        // Scrolled, the letters move with their text and are laid out no further. (A debug
        // build checks moved nodes against nodes built afresh.)
        let built = counts_of(&trial).0;
        trial.wheel(pt(100.0, 100.0), 0.0, 30.0);
        let (now, kept) = check(&trial);
        assert!(now.y < first.y && now.x == first.x, "{first:?} {now:?}");
        assert!(counts_of(&trial).0 == built && Arc::ptr_eq(kept.as_ref().unwrap(), letters.as_ref().unwrap()));
        // Scrolled out of view, it has none; back, it has them again. (A provider's copy follows
        // the move: `follow_move` checks it in a debug build.)
        let (mut copy, was) = (trial.access_nodes().to_vec(), trial.ui.access_version());
        trial.wheel(pt(100.0, 100.0), 0.0, 600.0);
        assert_eq!(trial.ui.access_moved_from(), Some(was));
        follow_move(&mut copy, trial.access_nodes());
        let gone = trial.access_nodes().iter().find(|node| node.name == LONG).unwrap();
        assert!(!gone.visible && gone.letters.is_none());
        trial.wheel(pt(100.0, 100.0), 0.0, -570.0);
        assert_eq!(trial.find(Role::Text, LONG).letters, letters);
        let letters = trial.find(Role::Text, LONG).letters.clone();
        // Built afresh for another change, they are not laid out again.
        trial.app.push_str(" more");
        trial.run();
        assert!(counts_of(&trial).0 > built && Arc::ptr_eq(trial.find(Role::Text, LONG).letters.as_ref().unwrap(), letters.as_ref().unwrap()));
    }

    /// A hand-written tree between two labels: Music (open, holding Rock, open, holding a
    /// song), then Video.
    fn grove(open: &mut [bool; 3], ui: &mut Ui) {
        ui.label("Files");
        ui.tree(&mut open[0], "Music");
        if open[0] {
            ui.tree(&mut open[1], "Rock");
            if open[1] {
                ui.label("song.ogg");
                ui.close();
            }
            ui.close();
        }
        ui.tree(&mut open[2], "Video");
        ui.label("After");
    }

    #[test]
    fn tree_items_the_app_put_in_no_tree_are_told_in_one() {
        let trial = Trial::new([true, true, false], grove);
        let nodes = trial.access_nodes();
        let trees: Vec<usize> = (0..nodes.len()).filter(|at| nodes[*at].role == Role::Tree).collect();
        assert_eq!(trees.len(), 1, "{}", trial.dump());
        let of = |name: &str| nodes.iter().find(|node| node.name == name).unwrap();
        for (name, level, place) in [("Music", 1, [1, 2]), ("Rock", 2, [1, 1]), ("Video", 1, [2, 2])] {
            assert_eq!((of(name).role, of(name).parent, of(name).level, of(name).position), (Role::TreeItem, Some(trees[0]), level, Some(place)), "{name}");
        }
        assert!(of("song.ogg").parent == Some(trees[0]) && of("Files").parent.is_none() && of("After").parent.is_none());
    }

    /// A table of a thousand rows, its selection told.
    #[derive(Default)]
    struct Ledger {
        table: crate::Table,
        selection: crate::Selection,
    }

    fn ledger(app: &mut Ledger, ui: &mut Ui) {
        const COLUMNS: [crate::Column; 2] = [crate::Column::new("Name"), crate::Column::new("Size")];
        ui.table(&mut app.table, &COLUMNS);
        ui.tell_selection(&app.selection);
        for row in ui.rows(1000, 24.0) {
            if ui.selectable_row(row, app.selection.contains(row)).clicked {
                app.selection.click(row, ui.mods());
            }
            ui.label(&format!("Row {row}"));
            ui.label("1 KB");
            ui.close();
        }
        ui.close();
        ui.close();
    }

    #[test]
    fn every_row_of_a_long_table_is_reached_and_selected_through_it() {
        let mut trial = Trial::new(Ledger::default(), ledger);
        trial.app.selection.add(700..703);
        trial.run();
        let table = |trial: &Trial<Ledger>| trial.access_nodes().iter().find(|node| node.role == Role::Table).unwrap().clone();
        // Its selection is all of it, told or not; its rows told are those in view.
        assert_eq!((table(&trial).grid, &table(&trial).selected_rows[..]), (Some([1000, 2]), &[[700, 703]][..]));
        assert!(!trial.has(Role::Cell, "Row 500"));
        // A row asked for is scrolled into view.
        let id = table(&trial).id;
        trial.ui.row_for_reader(id, 500, None);
        trial.run();
        assert!(trial.find(Role::Cell, "Row 500").visible);
        // Asked to select one, it is selected alone; asked to toggle one, it joins.
        trial.ui.row_for_reader(id, 900, Some(true));
        trial.run();
        assert_eq!(trial.app.selection.iter().collect::<Vec<_>>(), [900]);
        trial.ui.row_for_reader(id, 902, Some(false));
        trial.run();
        assert_eq!((trial.app.selection.iter().collect::<Vec<_>>(), &table(&trial).selected_rows[..]), (vec![900, 902], &[[900, 901], [902, 903]][..]));
        assert!(trial.find(Role::Cell, "Row 902").visible && trial.ui.table_reading.asked.is_none());
    }

    #[test]
    fn a_reader_pages_through_a_long_table_a_screenful_at_a_time() {
        let mut trial = Trial::new(Ledger::default(), ledger);
        trial.run();
        let table = |trial: &Trial<Ledger>| trial.access_nodes().iter().position(|node| node.role == Role::Table).unwrap();
        let view = |trial: &Trial<Ledger>| rows_in_view(trial.access_nodes(), table(trial)).unwrap();
        let [first, end] = view(&trial);
        assert_eq!((first, row_a_page_away(trial.access_nodes(), table(&trial), false)), (0, None), "nothing before the first row");
        let shown = end - first;
        let id = trial.access_nodes()[table(&trial)].id;
        // A page on: the rows after those that showed.
        let row = row_a_page_away(trial.access_nodes(), table(&trial), true).unwrap();
        trial.ui.row_for_reader(id, row, None);
        trial.run();
        assert_eq!(view(&trial), [end, end + shown]);
        // And back again.
        let row = row_a_page_away(trial.access_nodes(), table(&trial), false).unwrap();
        trial.ui.row_for_reader(id, row, None);
        trial.run();
        assert_eq!(view(&trial), [first, end]);
        // At the last row, there is no page after.
        trial.ui.row_for_reader(id, 999, None);
        trial.run();
        assert_eq!((view(&trial)[1], row_a_page_away(trial.access_nodes(), table(&trial), true)), (1000, None));
    }

    /// What a scroll costs a page of 10,000 rows (40,000 boxes): without a screen reader; with
    /// one; with one and a provider copying what changed; with one and a provider following a
    /// move. The quickest of many, in µs, each way in turn. (Run in release: `cargo test
    /// --release --lib -- --ignored --nocapture how_long_a_scroll`.)
    #[test]
    #[ignore]
    fn how_long_a_scroll_takes_while_a_reader_listens() {
        let describe = |ui: &mut Ui| {
            ui.column().grow().scroll().pad(10.0).gap(6.0).open();
            for i in 0..10_000 {
                ui.row().gap(8.0).pad((10.0, 5.0)).open();
                ui.label("a row with a name");
                ui.note(if i % 2 == 0 { "even" } else { "odd" });
                ui.button("Open");
                ui.close();
            }
            ui.close();
        };
        let mut line = String::new();
        for way in ["no reader", "reader", "copying", "following"].repeat(3) {
            let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::LocalClipboard::default()));
            ui.resize(crate::Size { w: 800.0, h: 600.0 }, 1.0);
            ui.set_screen_reader(way != "no reader");
            let mut build = describe;
            (0..2).for_each(|_| _ = ui.run(&mut build));
            let (mut kept, mut seen, mut down) = (ui.access_nodes().to_vec(), ui.access_version(), 40.0);
            ui.event(crate::Event::Move(pt(300.0, 300.0)));
            ui.run(&mut build);
            let mut scroll = || {
                let start = std::time::Instant::now();
                ui.event(crate::Event::Wheel(0.0, down));
                down = -down;
                ui.run(&mut build);
                match (way, ui.access_moved_from() == Some(seen)) {
                    ("following", true) => follow_move(&mut kept, ui.access_nodes()),
                    ("copying" | "following", _) if kept != ui.access_nodes() => kept = ui.access_nodes().to_vec(),
                    _ => {}
                }
                seen = ui.access_version();
                start.elapsed()
            };
            let quickest = (0..40).map(|_| scroll()).min().unwrap();
            line += &format!("{way} {:.0}  ", quickest.as_secs_f64() * 1e6);
        }
        println!("{line}");
    }

    /// Times the nodes were built and moved.
    fn counts_of<A>(trial: &Trial<A>) -> (u32, u32) {
        let cache = trial.ui.visible_boxes.access_cache.first().expect("a cache kept while a screen reader listens");
        (cache.built, cache.moved)
    }

    #[test]
    fn a_reader_moves_through_text_by_character_word_line_and_paragraph() {
        let mut trial = Trial::new(String::new(), |_: &mut String, ui: &mut Ui| {
            ui.column().w(crate::Length::Px(200.0)).open();
            ui.paragraph(LONG);
            ui.close();
        });
        trial.run();
        let text = trial.find(Role::Text, LONG).clone();
        assert_eq!(text.unit_range(0, TextUnit::Character, true), Some([0, 1]));
        assert_eq!(text.unit_range(0, TextUnit::Character, false), None);
        assert_eq!(text.unit_range(3, TextUnit::Word, true), Some([4, 7]), "past the space, \"two\"");
        assert_eq!(text.unit_range(5, TextUnit::Word, false), Some([4, 5]), "back from inside a word, to its start");
        assert_eq!(text.unit_range(4, TextUnit::Word, false), Some([0, 3]));
        let count = LONG.chars().count() as u32;
        assert_eq!(text.unit_range(count, TextUnit::Word, true), None);
        // Lines as wrapped: forward from a line's start is that line; back is the one before.
        let lines = text.line_starts();
        assert!(lines.len() > 2, "{lines:?}");
        assert_eq!(text.unit_range(0, TextUnit::Line, true), Some([0, lines[1]]));
        assert_eq!(text.unit_range(1, TextUnit::Line, true), Some([lines[1], lines[2]]));
        assert_eq!(text.unit_range(lines[1], TextUnit::Line, false), Some([0, lines[1]]));
        assert_eq!(text.unit_range(0, TextUnit::Line, false), None);
        let last = *lines.last().unwrap();
        assert_eq!(text.unit_range(last, TextUnit::Line, true), Some([last, count]));
        assert_eq!(text.unit_range(count, TextUnit::Line, true), None);
        // Paragraphs split at line breaks.
        let mut two = text.clone();
        (two.name, two.letters) = ("one\n\ntwo three".into(), None);
        assert_eq!(two.unit_range(0, TextUnit::Paragraph, true), Some([0, 3]));
        assert_eq!(two.unit_range(3, TextUnit::Paragraph, true), Some([5, 14]));
        assert_eq!(two.unit_range(14, TextUnit::Paragraph, false), Some([5, 14]));
        assert_eq!(two.unit_range(5, TextUnit::Paragraph, false), Some([0, 3]));
        assert_eq!(two.line_starts(), [0, 4, 5]);
    }

    #[test]
    fn utf16_units_and_letters_convert_both_ways() {
        let text = "a\u{1f600}b\u{e9}";
        assert_eq!([0, 1, 2, 3, 4].map(|letters| units_before(text, letters)), [0, 1, 3, 4, 5]);
        assert_eq!([0, 1, 2, 3, 4, 5, 9].map(|unit| letter_at_unit(text, unit)), [0, 1, 1, 2, 3, 4, 4]);
    }
}
