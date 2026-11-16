//! What screen readers are told. A screen reader reads a window aloud and operates it for
//! people who cannot see it, and asks what is there. This is the list it gets: every piece of
//! text and every control, with its role, name and state (checked, selected, expanded, value).
//!
//! Most of it is implied. A box holding text is text. A focusable box is a button named by its
//! text. Parts that are something else say so themselves (`tell`): a check box, a slider, a
//! text box, a table. An app does the same for its own parts, and names what has no text:
//!
//! ```ignore
//! let add = ui.icon_button(PLUS, "Add a note");   // called by its hint
//! let dot = ui.el().size(12.0, 12.0).radius(6.0).fill(t.green).sense().show();
//! ui.tell(&dot, Role::Text, None, "Up to date");
//! ```
//!
//! A table is told as a table: headings, then rows, each with its cells, so a screen reader can
//! say a cell's row, column and column heading, and navigate by cell, row and column. The table
//! reports its total row count; the rows told are the visible ones, as everywhere. But a
//! visible row is told whole, and so are the headings: what horizontal scrolling hid is still
//! told, since nobody who cannot see the window scrolls it sideways.
//!
//! A tree is told as a tree: its items in display order, each with its level, its position
//! among its siblings, and whether it is expanded and selected. (So a long tree's items are
//! told even when their parents are out of view, as the web tells a partly rendered tree.)
//!
//! While no screen reader listens, none of this is built: an unread window pays nothing. While
//! one does, a run that changes nothing told keeps the list, and one that only moves boxes
//! (scrolling) moves its elements.
use crate::event::{Key, Mods};
use crate::geom::{Rect, rect};
use crate::id::{Id, fold, hash};
use crate::layout::{Content, NONE, Tree};
use crate::spell::Spelling;
use crate::survey::{Lying, Survey};
use crate::ui::{Response, Stroke, Ui};
use std::cell::OnceCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

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
    Tick,
    /// A toggle switch.
    Switch,
    /// A radio button.
    Choice,
    /// A tab in a tab row.
    Tab,
    /// A selectable list row.
    Row,
    /// A slider.
    Slider,
    /// A spin button: a number typed or stepped. (A number box, and each part of a time.)
    Spin,
    /// A text box.
    TextBox,
    /// A password box.
    Secret,
    /// A disclosure: a heading or button that expands and collapses its content.
    Fold,
    /// A combo box: shows one choice, and opens the list of them.
    Select,
    /// A menu item.
    MenuItem,
    /// A table column header.
    Heading,
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
    MenuChoice,
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
    /// A document heading, of a level from 1 (see [`Said::level`]).
    Title,
    /// A document paragraph.
    Paragraph,
    /// A document list, ordered or not.
    List,
    /// A list item, holding its content.
    Item,
    /// A code block, verbatim.
    Code,
    /// A block quote.
    Quote,
}

impl Role {
    /// Whether it only contains other things and is not itself operable: its contents are told
    /// as inside it.
    pub fn holds(self) -> bool {
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
                | Role::Item
                | Role::Quote
        )
    }
}

/// One element of the window, as told to a screen reader.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    /// Its id, stable from run to run.
    pub id: Id,
    /// Its role.
    pub role: Role,
    /// Its name: as given, else its text.
    pub name: String,
    /// A text box's text (for a password box, the dots it shows); a combo box's chosen item.
    pub value: String,
    /// Its keyboard shortcut, as its part shows it ("Ctrl+O"): usually none.
    pub keys: String,
    /// Whether it is on, selected or expanded, for roles that can be. (For a tree: whether
    /// multiple selection is allowed.)
    pub on: Option<bool>,
    /// For a tree item with children: whether they show. (None for one without.)
    pub open: Option<bool>,
    /// For a tree item, its depth, 1 at the top. For a document heading, its level, 1 for the
    /// main one. (0 otherwise.)
    pub level: u32,
    /// For a slider or progress bar: its value, minimum and maximum.
    pub amount: Option<[f32; 3]>,
    /// A slider's step.
    pub step: f32,
    /// Its position in its set, from 1: the second of five tabs, row 4,000 of 5,000 (of which
    /// only visible rows are told), the third visible heading.
    pub of: Option<[u32; 2]>,
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
    pub within: Option<usize>,
    /// It has focus.
    pub focused: bool,
    /// It is enabled.
    pub usable: bool,
    /// Some of it is visible. An element scrolled out of its scroll box or the window is still
    /// told ([`Ui::told`]), as offscreen; it is not in what shows ([`Ui::said`]). (In a table,
    /// every heading and everything in a visible row count as visible, however far it is
    /// scrolled sideways.)
    pub shown: bool,
    /// For a spell-checked text box ([`Ui::check_spelling`]): its visible misspellings, each
    /// from its first character to one past its last, from the start of its text.
    pub spelt_wrong: Vec<[u32; 2]>,
    /// Its visible part: of `rect`, what its containers and the window leave in view. (Empty
    /// for what does not show.)
    pub(crate) seen: Rect,
    /// Its box, by index among the run's boxes.
    pub(crate) at: u32,
}

/// What a part said about a box this run: which box, and where its name and value lie in the
/// text kept beside these.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Note {
    at: u32,
    role: Role,
    on: Option<bool>,
    /// For a tree item: whether its children show, and its depth.
    open: Option<bool>,
    level: u32,
    /// Its text is the last of the box told before it, and is told with it.
    joined: bool,
    amount: Option<[f32; 3]>,
    step: f32,
    of: Option<[u32; 2]>,
    caret: Option<[u32; 2]>,
    /// For a table: its row count (counted as rows are described), and its visible columns.
    grid: Option<[u32; 2]>,
    /// For a table row: its index from 0; the range of its table's columns in their list; and
    /// the table's own note's index.
    row: Option<[u32; 4]>,
    sorted: Option<bool>,
    name: [u32; 2],
    value: [u32; 2],
    keys: [u32; 2],
    usable: bool,
    /// Told on its own, though inside an element told as one: a tab's close button.
    apart: bool,
}

impl Said {
    /// An element of a role and name, with nothing more said.
    pub(crate) fn plain(id: Id, role: Role, name: String, rect: Rect, within: Option<usize>) -> Said {
        Said {
            id,
            role,
            name,
            value: String::new(),
            keys: String::new(),
            on: None,
            open: None,
            level: 0,
            amount: None,
            step: 0.0,
            of: None,
            grid: None,
            sorted: None,
            caret: None,
            rect,
            within,
            focused: false,
            usable: true,
            shown: true,
            spelt_wrong: Vec::new(),
            seen: rect,
            at: 0,
        }
    }
}

impl Said {
    /// For the column heading the rows are sorted by: the direction, in the word both Windows
    /// and Linux use.
    pub(crate) fn sort_word(&self) -> Option<&'static str> {
        self.sorted.map(|down| if down { "descending" } else { "ascending" })
    }
    /// Whether it can take focus: anything operable, a selectable table row, a document (for
    /// selecting in it), and a table cell (focus there being the table's cell); not text,
    /// progress bars, or containers.
    pub(crate) fn takes_keyboard(&self) -> bool {
        match self.role {
            Role::TableRow => self.on.is_some(),
            Role::Document | Role::Cell => true,
            role => !matches!(role, Role::Text | Role::Progress | Role::Title | Role::Paragraph | Role::Code) && !role.holds(),
        }
    }
    /// Whether what it opens shows: a combo box's list, a disclosure's content, a tree item's
    /// children. None for something that opens nothing.
    #[cfg(windows)]
    pub(crate) fn unfolded(&self) -> Option<bool> {
        match self.role {
            Role::Fold | Role::Select => Some(self.on == Some(true)),
            Role::TreeItem => self.open,
            _ => None,
        }
    }
}

/// A tree item's tree, by list index. (None for a hand-written tree item the app put in no
/// tree.)
#[cfg(windows)]
pub(crate) fn tree_of(said: &[Said], item: usize) -> Option<usize> {
    let tree = said.get(item).filter(|item| item.role == Role::TreeItem)?.within?;
    said.get(tree).is_some_and(|tree| tree.role == Role::Tree).then_some(tree)
}

/// The visible part of an element, by its list index: all of it, but in a table only what lies
/// within the table. (A row is told whole, however far the table is scrolled sideways.)
pub(crate) fn view(said: &[Said], at: usize) -> Rect {
    let (mut seen, mut up) = (said[at].rect, said[at].within);
    while let Some(group) = up.and_then(|group| said.get(group)) {
        seen = if group.role == Role::Table { seen.intersect(group.rect) } else { seen };
        up = group.within;
    }
    seen
}

/// A cell's table, by list index.
pub(crate) fn table_of(said: &[Said], cell: usize) -> Option<usize> {
    let row = said.get(cell).filter(|cell| cell.role == Role::Cell)?.within?;
    said.get(row)?.within.filter(|table| said.get(*table).is_some_and(|table| table.role == Role::Table))
}

/// A table's cell at a row and column from 0: its list index. None for a row out of view: its
/// cells are not told.
pub(crate) fn cell_at(said: &[Said], table: usize, place: [u32; 2]) -> Option<usize> {
    (table + 1..said.len()).find(|at| said[*at].grid == Some(place) && table_of(said, *at) == Some(table))
}

/// The heading over a table column, from 0.
pub(crate) fn heading_at(said: &[Said], table: usize, column: u32) -> Option<usize> {
    let heads = |at: &usize| said[*at].role == Role::Heading && said[*at].within == Some(table);
    (table + 1..said.len()).find(|at| heads(at) && said[*at].of.is_some_and(|of| of[0] == column + 1))
}

/// A table's told rows, by list index: each with its row index from 0.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn rows_of(said: &[Said], table: usize) -> impl Iterator<Item = (usize, u32)> {
    let row = move |at: usize| (said[at].role == Role::TableRow && said[at].within == Some(table)).then_some(at);
    (table + 1..said.len()).filter_map(row).map(|at| (at, said[at].of.map_or(0, |of| of[0].saturating_sub(1))))
}

/// The text in a box and its descendants, piece after piece, up to `most` pieces.
fn words_in(tree: &Tree, at: u32, most: usize) -> String {
    let texts = tree.specs[at as usize..tree.specs[at as usize].end as usize].iter().filter_map(|spec| match spec.content {
        Content::Text { start, end, .. } if start < end => Some(&tree.strings[start as usize..end as usize]),
        _ => None,
    });
    // (One piece, the usual case, is returned as is, with nothing collected.)
    let mut words = String::new();
    for (nth, text) in texts.take(most).enumerate() {
        words.push_str(if nth > 0 { ", " } else { "" });
        words.push_str(text);
    }
    words
}

/// The text just before box `at`, which labels it where it has none: the previous sibling's
/// text, or that of the box before the one it is first in (a part made of a box or two around
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

/// The last note on box `at`, among notes in box order (and, for one box, in the order said).
fn last_note(notes: &[Note], at: u32) -> Option<&Note> {
    notes.partition_point(|note| note.at <= at).checked_sub(1).map(|last| &notes[last]).filter(|note| note.at == at)
}

/// All that was said of box `at`, among notes in box order (and per box in the order said): its
/// part's note, overlaid with later ones where they say something. So an app adding to a part
/// right after it (a name, another role) is heard, and what the part said and the app did not
/// (a slider's value, a combo's choice) still is.
fn said_of(notes: &[Note], at: u32) -> Option<Note> {
    let first = notes.partition_point(|note| note.at < at);
    let mut said = *notes.get(first).filter(|note| note.at == at)?;
    for later in notes[first + 1..].iter().take_while(|note| note.at == at) {
        let given = |range: [u32; 2], was: [u32; 2]| if range[0] < range[1] { range } else { was };
        (said.role, said.name, said.value, said.keys) = (later.role, given(later.name, said.name), given(later.value, said.value), given(later.keys, said.keys));
        (said.on, said.open, said.amount, said.of, said.caret) = (later.on.or(said.on), later.open.or(said.open), later.amount.or(said.amount), later.of.or(said.of), later.caret.or(said.caret));
        (said.grid, said.row, said.sorted) = (later.grid.or(said.grid), later.row.or(said.row), later.sorted.or(said.sorted));
        (said.level, said.step) = (if later.level > 0 { later.level } else { said.level }, if later.step > 0.0 { later.step } else { said.step });
    }
    Some(said)
}

/// Builds what screen readers are told after a run (`Ui::hear`): handed over by the first
/// interface a screen reader listens to ([`Ui::listen`]), and reached by nothing else, so a
/// program no screen reader can listen to (with no library window, and never saying one
/// listens) is built without it.
pub(crate) static HEARS: OnceLock<fn(&mut Ui)> = OnceLock::new();

/// A box told to a screen reader: which, its rectangle, its visible part, and whether it shows
/// ([`Said::shown`]).
type Told = (u32, Rect, Rect, bool);

/// What the told list was made from, kept while a screen reader listens. A run that changed
/// none of it keeps the list; one that only moved boxes moves the list's elements.
#[derive(Default)]
pub(crate) struct Heard {
    /// The boxes walked last (kept so its memory is reused).
    boxes: Vec<Told>,
    /// What parts said, in the order said; and the text.
    notes: Vec<Note>,
    noted: String,
    text: String,
    /// The rest, hashed: see `Ui::told_key`.
    key: u64,
    /// Which list this is: new with every change, unique among windows. 0 for none yet.
    turn: u64,
    /// Lists made, and lists moved.
    made: u32,
    moved: u32,
}

/// The next list's `turn`.
static TURNS: AtomicU64 = AtomicU64::new(1);

/// Walks the boxes told to a screen reader: every box of the layers from the topmost modal one
/// (a dialog) up, visible or not, in `Lying`'s order. (What lies under a dialog is unreachable
/// until it is answered, so it is not told meanwhile. Under an open menu everything stays told:
/// a menu closes at a key, and the reader should find the window as it was.)
struct Telling<'a> {
    tree: &'a Tree,
    notes: &'a [Note],
    lying: &'a mut Lying,
    window: Rect,
    boxes: Vec<Told>,
    /// The boxes shown whole, however they are scrolled: a visible table row, and the box
    /// holding its headings (the table's first); and where such headings would be, after the
    /// last table told.
    whole: [u32; 2],
    heads: u32,
}

impl Telling<'_> {
    /// Enters box `at`: its visible part, and whether it shows. Descends into all of it.
    fn enter(&mut self, at: u32) -> bool {
        // (A hidden box and everything in it does not exist for a screen reader.)
        if self.tree.specs[at as usize].fade == 255 {
            return false;
        }
        let (r, clip) = self.lying.lies(self.tree, at);
        let (seen, within) = (r.intersect(clip).intersect(self.window), at > self.whole[0] && at < self.whole[1]);
        let shown = within || !self.tree.drawn(at).moved(r.x, r.y).intersect(clip).intersect(self.window).is_empty();
        self.boxes.push((at, r, seen, shown));
        let role = last_note(self.notes, at).map(|note| note.role);
        if shown && !within && (role == Some(Role::TableRow) || at == self.heads) {
            self.whole = [at, self.tree.specs[at as usize].end];
        }
        self.heads = if role == Some(Role::Table) { at + 1 } else { self.heads };
        self.lying.go_in(self.tree, at, r, clip);
        true
    }
    /// Leaves box `at`. (A sticky heading at the top of a scroll box hides what is under it, as
    /// the survey has it.)
    fn leave(&mut self, at: u32) -> bool {
        let (r, _) = self.lying.go_out(self.tree, at);
        self.lying.pin(self.tree, at, r);
        false
    }
}

/// The boxes told to a screen reader (see `Telling`), in window `window`, into `boxes`.
fn told_boxes(survey: &mut Survey, boxes: &mut Vec<Told>, tree: &Tree, notes: &[Note], window: Rect) {
    let Survey { roots, told_from, told, .. } = survey;
    boxes.clear();
    let mut telling = Telling { tree, notes, lying: told, window, boxes: std::mem::take(boxes), whole: [0; 2], heads: NONE };
    let mut open = Vec::new();
    for root in roots.get(*told_from..).unwrap_or_default() {
        telling.lying.start();
        tree.walk(*root, &mut open, &mut |at, entering| if entering { telling.enter(at) } else { telling.leave(at) });
    }
    *boxes = telling.boxes;
}

/// Whether two told lists say the same: equal, or equal but for NaN in what an app gave.
#[cfg(debug_assertions)]
fn same_told(one: &[Said], other: &[Said]) -> bool {
    one == other || format!("{one:?}") == format!("{other:?}")
}

/// The misspellings in a `role` text box `id` with text `typed`, where its words are checked:
/// each from its first character to one past its last. (None for anything else, or when
/// unchecked.)
fn spelt_wrong(spelling: &Spelling, role: Role, id: Id, typed: &str) -> Vec<[u32; 2]> {
    let Some(hooks) = spelling.hooks().filter(|_| role == Role::TextBox) else { return Vec::new() };
    let letters = |byte: usize| typed.get(..byte).map_or(0, |before| before.chars().count()) as u32;
    let mut wrong: Vec<[u32; 2]> = (hooks.wrong)(spelling, id).into_iter().map(|word| [letters(word.start), letters(word.end)]).collect();
    wrong.dedup();
    wrong
}

/// What shows of what is told ([`Ui::said`]): the visible elements, each in its nearest visible
/// group.
fn shown_of(told: &[Said]) -> Vec<Said> {
    // (Each element's index in what shows, or none; a group comes before its content.)
    let (mut said, mut places) = (Vec::new(), Vec::with_capacity(told.len()));
    for one in told {
        let mut within = one.within;
        while let Some(group) = within.filter(|group| places[*group] == usize::MAX) {
            within = told[group].within;
        }
        places.push(if one.shown { said.len() } else { usize::MAX });
        if one.shown {
            said.push(Said { within: within.map(|group| places[group]), ..one.clone() });
        }
    }
    said
}

/// What screen readers are told, and what tests find things by.
impl Ui {
    /// A screen reader starts or stops listening to this window: from the next run on, `said`
    /// is kept up to date, or not.
    pub fn listen(&mut self, listening: bool) {
        (self.listening, self.again) = (listening, true);
        if listening {
            HEARS.get_or_init(|| Ui::hear);
        }
        if !listening {
            (self.told, self.said, self.survey.telling) = (Vec::new(), OnceCell::new(), Vec::new());
            // (Nor do the second windows keep theirs: they hear of it at their next run, which
            // may be far off.)
            self.each_window(|window| (window.told, window.said, window.survey.telling) = (Vec::new(), OnceCell::new(), Vec::new()));
        }
    }
    /// Which told list this is: it changes whenever the list may have. (0 while nobody
    /// listens.)
    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    pub(crate) fn told_turn(&self) -> u64 {
        self.survey.telling.first().map_or(0, |heard| heard.turn)
    }
    /// What a screen reader is told of this window that is visible, as of the last run:
    /// elements in order, each group before its content. (`told` has all of it, visible or
    /// not.)
    pub fn said(&self) -> &[Said] {
        self.said.get_or_init(|| shown_of(&self.told))
    }
    /// What a screen reader is told of this window, as of the last run: everything in it,
    /// visible or not ([`Said::shown`]), in order, each group before its content. So a user who
    /// cannot see the window can reach a whole page with the reader's own keys, an element
    /// scrolled out of view is told as offscreen, and scrolled into view when the reader asks
    /// ([`show_for_reader`](Self::show_for_reader)). (A long list describes only visible rows,
    /// so only those are told: each selected one says its position of how many.)
    pub fn told(&self) -> &[Said] {
        &self.told
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
        self.keep_note(of, Role::Select, Some(open), "", chosen);
    }
    /// Adds to the last told element: its keyboard shortcut ("Ctrl+O"), for a screen reader to
    /// announce with it.
    pub fn tell_keys(&mut self, keys: &str) {
        let from = self.noted.len() as u32;
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            self.noted.push_str(keys);
            note.keys = [from, self.noted.len() as u32];
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
    ///     ui.tell_on_its_own(&open);
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
    pub fn tell_on_its_own(&mut self, of: &Response) {
        if !self.listening {
            return;
        }
        let last = self.tree.specs.len().checked_sub(1).filter(|_| of.id == 0);
        let Some(at) = last.or_else(|| self.tree.specs.iter().rposition(|spec| spec.id == of.id)).map(|at| at as u32) else { return };
        // (Its part's note, if any: among the last few notes.)
        if let Some(note) = self.notes.iter_mut().rev().take_while(|note| note.at >= at).find(|note| note.at == at) {
            note.apart = true;
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
            note.apart = true;
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
    pub fn tell_place(&mut self, nth: usize, count: usize) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.of = Some([nth as u32, count as u32]);
        }
    }
    /// Adds to the last told element, a tree item: its depth (1 at the top), and whether its
    /// children show (none for an item without).
    pub fn tell_branch(&mut self, level: usize, open: Option<bool>) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            (note.level, note.open) = (level as u32, open);
        }
    }
    /// Says the last made box holds some of the words of `all`, which a part writes a word per
    /// box (`words`): the first is told as all of them, the rest as inside it. `before` is the
    /// note count before the first.
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
    /// (`rows_told`).
    pub(crate) fn tell_table(&mut self, of: &Response, columns: usize) {
        self.keep_note(of, Role::Table, None, "", "");
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.grid = Some([0, columns as u32]);
        }
    }
    /// The index of the note for the table told as box `at`, whose columns are `columns` in
    /// their list. (None while nobody listens.)
    pub(crate) fn table_note(&self, at: u32, columns: [u32; 2]) -> Option<usize> {
        // (This table's last told row says where it is: so only that row's notes are searched
        // back, not every earlier row's.)
        let ours = |note: &Note| (note.at, note.role) == (at, Role::Table) || note.row.is_some_and(|row| [row[1], row[2]] == columns);
        let found = self.notes.iter().rposition(ours)?;
        Some(self.notes[found].row.map_or(found, |row| row[3] as usize))
    }
    /// How many rows a note's table has been told to have so far, to count on.
    pub(crate) fn rows_told(&mut self, table: usize) -> Option<&mut u32> {
        self.notes.get_mut(table)?.grid.as_mut().map(|grid| &mut grid[0])
    }
    /// Says the next made box is a table row: its index of all rows from 0; the range of the
    /// table's columns in their list; and the table's own note's index.
    pub(crate) fn tell_row(&mut self, row: u32, [first, after]: [u32; 2], table: usize) {
        let (at, end) = (self.tree.specs.len() as u32, self.noted.len() as u32);
        let (role, row, usable) = (Role::TableRow, Some([row, first, after, table as u32]), self.enabled);
        self.notes.push(Note {
            at,
            role,
            on: None,
            open: None,
            level: 0,
            amount: None,
            step: 0.0,
            joined: false,
            of: None,
            caret: None,
            grid: None,
            row,
            sorted: None,
            name: [end; 2],
            value: [end; 2],
            keys: [end; 2],
            usable,
            apart: false,
        });
    }
    /// Adds to the last told table row: it is selectable, and whether selected.
    pub(crate) fn tell_picked(&mut self, picked: bool) {
        if let Some(note) = self.notes.last_mut().filter(|note| note.role == Role::TableRow) {
            note.on = Some(picked);
        }
    }
    /// Adds to the last told heading: whether the rows are sorted by its column, and if so
    /// whether descending.
    pub(crate) fn tell_sorted(&mut self, sorted: Option<bool>) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.sorted = sorted;
        }
    }
    /// Adds to the last told text box: its selection anchor and cursor, in characters.
    pub(crate) fn tell_caret(&mut self, caret: [u32; 2]) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.caret = Some(caret);
        }
    }
    /// The same for something that stands for a value: a slider, or a progress bar. Its value,
    /// minimum and maximum.
    pub fn tell_amount(&mut self, of: &Response, role: Role, amount: [f32; 3]) {
        self.tell_amount_named(of, role, amount, "");
    }
    /// The same, with a name: a library part without text.
    pub(crate) fn tell_amount_named(&mut self, of: &Response, role: Role, amount: [f32; 3], name: &str) {
        self.keep_note(of, role, None, name, "");
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.amount = Some(amount);
        }
    }
    /// Adds to box `of`, already told (as a text box, or by its name): it is of this role
    /// standing for a value, with its value, minimum and maximum. (A number box is a spin
    /// button.)
    pub(crate) fn tell_stands(&mut self, of: &Response, role: Role, amount: [f32; 3]) {
        let Ui { notes, tree, listening, .. } = self;
        if let Some(note) = notes.iter_mut().rev().find(|note| *listening && tree.specs[note.at as usize].id == of.id) {
            (note.role, note.amount) = (role, Some(amount));
        }
    }
    /// Adds to the last told element, a document heading: its level, 1 for the main one.
    pub fn tell_level(&mut self, level: usize) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.level = level as u32;
        }
    }
    /// Adds to the last told slider: its step.
    pub fn tell_step(&mut self, step: f32) {
        if let Some(note) = self.notes.last_mut().filter(|_| self.listening) {
            note.step = step;
        }
    }
    /// Names box `of` `name`, whatever was told of it before: a part beside a label saying what
    /// it is (an inspector row). Called right after the part.
    pub(crate) fn tell_name(&mut self, of: &Response, name: &str) {
        let Some(at) = self.tree.specs.iter().rposition(|spec| spec.id == of.id).filter(|_| self.listening && of.id != 0) else { return };
        let Ui { notes, noted, tree, .. } = self;
        match notes.iter_mut().rev().find(|note| note.at == at as u32) {
            Some(note) => {
                let from = noted.len() as u32;
                noted.push_str(name);
                note.name = [from, noted.len() as u32];
            }
            None if tree.specs[at].frame.focus => self.keep_note(of, Role::Button, None, name, ""),
            None => {}
        }
    }
    pub(crate) fn keep_note(&mut self, of: &Response, role: Role, on: Option<bool>, name: &str, value: &str) {
        // (Nothing is kept or searched for nobody; and the box is among the last few made. A
        // box without an id, a picture, is the last made: called right after it.)
        if !self.listening {
            return;
        }
        let last = self.tree.specs.len().checked_sub(1).filter(|_| of.id == 0);
        let Some(at) = last.or_else(|| self.tree.specs.iter().rposition(|spec| spec.id == of.id)) else { return };
        let from = self.noted.len() as u32;
        self.noted.push_str(name);
        let between = self.noted.len() as u32;
        self.noted.push_str(value);
        let end = self.noted.len() as u32;
        let (name, value, usable) = ([from, between], [between, end], self.enabled);
        self.notes.push(Note {
            at: at as u32,
            role,
            on,
            open: None,
            level: 0,
            amount: None,
            step: 0.0,
            joined: false,
            of: None,
            caret: None,
            grid: None,
            row: None,
            sorted: None,
            name,
            value,
            keys: [end; 2],
            usable,
            apart: false,
        });
    }

    /// Builds what screen readers are told, from the boxes and what parts said about them. Done
    /// after a run, while one listens. Where nothing it is made from changed, the list is kept;
    /// where only boxes moved, its elements are moved.
    pub(crate) fn hear(&mut self) {
        let key = self.told_key();
        let mut heard = self.survey.telling.pop().unwrap_or_default();
        // (The key has the text's length only. Where nothing moved, the text is as it was.)
        let same = heard.turn != 0 && heard.key == key && heard.notes == self.notes && heard.noted == self.noted;
        let same = same && (!self.resurveyed || heard.text == self.tree.strings);
        let moved = same && self.resurveyed;
        if moved {
            heard.moved += 1;
            self.notes.sort_by_key(|note| note.at);
        }
        // (A list whose boxes were not all found again is made afresh: not seen to happen.)
        let kept = same && (!moved || self.move_told(&mut heard.boxes));
        if !same {
            heard.key = key;
            heard.notes.clone_from(&self.notes);
            heard.noted.clone_from(&self.noted);
            heard.text.clone_from(&self.tree.strings);
        }
        if !kept {
            heard.made += 1;
            self.tell_all(&mut heard.boxes);
        }
        if !same || moved {
            heard.turn = TURNS.fetch_add(1, Ordering::Relaxed);
        }
        // (A debug build checks a list kept or moved against one made afresh.)
        #[cfg(debug_assertions)]
        if kept {
            let kept = self.told.clone();
            self.tell_all(&mut heard.boxes);
            assert!(same_told(&kept, &self.told), "the list told to screen readers was kept where it changed");
        }
        self.survey.telling.push(heard);
    }

    /// A hash of what the told list is made from, but where boxes lie and the text: the boxes
    /// (their layout and ids), focus, the cell keyed, columns, radio groups, long-list rows, the
    /// layers told, and misspellings.
    fn told_key(&self) -> u64 {
        let Ui { tree, survey, focus, grids, tracks, rovers, rows_told, notes, asks, layout_key, .. } = self;
        let mut key = fold(fold(*layout_key, *focus), grids.keyed_box().map_or(u64::MAX, u64::from));
        key = tree.specs.iter().fold(key, |key, spec| fold(key, spec.id));
        key = survey.roots.get(survey.told_from..).unwrap_or_default().iter().fold(key, |key, root| fold(key, *root as u64));
        key = tracks.iter().fold(key, |key, track| fold(key, track.place as u64 | (track.shows as u64) << 32));
        key = rovers.iter().fold(key, |key, rover| fold(fold(fold(key, rover.id), rover.group), rover.picks as u64));
        key = rows_told.iter().fold(key, |key, (row, [nth, count])| fold(fold(key, *row as u64), (*nth as u64) << 32 | *count as u64));
        // (Misspellings are found apart from the run: in every text box.)
        let Some(hooks) = asks.spelling.hooks() else { return key };
        for note in notes.iter().filter(|note| note.role == Role::TextBox) {
            let wrong = (hooks.wrong)(&asks.spelling, tree.specs[note.at as usize].id);
            key = wrong.iter().fold(fold(key, note.at as u64), |key, word| fold(key, (word.start as u64) << 32 | word.end as u64));
        }
        key
    }

    /// Moves each told element to where its box now lies, with what of it shows. Says whether
    /// every element's box was found, in order. (Notes sorted.)
    fn move_told(&mut self, boxes: &mut Vec<Told>) -> bool {
        let window = rect(0.0, 0.0, self.size().w, self.size().h);
        let Ui { survey, tree, notes, told, said, .. } = self;
        told_boxes(survey, boxes, tree, notes, window);
        *said = OnceCell::new();
        // (A cell and what is in it are one box: the next element may be of the same box.)
        let mut next = 0;
        for one in told.iter_mut() {
            let Some(found) = boxes[next..].iter().position(|(at, ..)| *at == one.at) else { return false };
            next += found;
            (_, one.rect, one.seen, one.shown) = boxes[next];
        }
        true
    }

    /// Makes the told list afresh, from the boxes (walked into `boxes`) and what parts said.
    fn tell_all(&mut self, boxes: &mut Vec<Told>) {
        let window = rect(0.0, 0.0, self.size().w, self.size().h);
        let Ui { survey, tree, notes, noted, told: said, said: shown, rows_told, focus, tracks, grids, rovers, asks, .. } = self;
        // (Focus is on a table's cell while it is on the table. And misspellings are found
        // where words are checked.)
        let (keyed, spelling) = (grids.keyed_box(), &asks.spelling);
        notes.sort_by_key(|note| note.at);
        said.clear();
        *shown = OnceCell::new();
        // The groups being walked, innermost last: each one's list index, its box and the first
        // box after it; for a table row, its next cell's column and the first column after its
        // table's, in the column list. And the boxes inside an element told as one, which are
        // not told again.
        let (mut groups, mut inside): (Vec<[u32; 5]>, [u32; 2]) = (Vec::new(), [0; 2]);
        // An element without an id is known by the last told element with one and how many
        // id-less ones were told since: so it stays stable from run to run, whatever changes
        // elsewhere.
        let (mut last, mut since) = (0, 0u32);
        // (And the long-list rows told so far: the first element told in such a row gives its
        // position of how many.)
        let mut placed = 0;
        told_boxes(survey, boxes, tree, notes, window);
        for &(at, lies, seen, shown) in boxes.iter() {
            let spec = &tree.specs[at as usize];
            while rows_told.get(placed).is_some_and(|(row, _)| tree.specs[*row as usize].end <= at) {
                placed += 1;
            }
            // (What lies over the page starts fresh: inside nothing before it.)
            if spec.frame.parent == NONE {
                inside = [0; 2];
            }
            while groups.last().is_some_and(|group| at < group[1] || at >= group[2]) {
                groups.pop();
            }
            // (Content of an element told as one is not told again: except what a part said is
            // told on its own.)
            let within_one = at > inside[0] && at < inside[1];
            if within_one && !notes[notes.partition_point(|note| note.at < at)..].iter().take_while(|note| note.at == at).any(|note| note.apart) {
                continue;
            }
            // What a part said of it or its content. (Of what was said of it, the last word: an
            // app adds to a part after the part.)
            let noted_from = notes.partition_point(|note| note.at < at);
            let note = said_of(notes, at);
            let note = note.as_ref();
            if note.is_some_and(|note| note.joined) {
                inside = [at, spec.end];
                continue;
            }
            let used = notes.get(noted_from).is_some_and(|note| note.at < spec.end) || tree.specs[at as usize..spec.end as usize].iter().any(|spec| spec.frame.focus);
            // A box in a table row is a cell of that row, in the next column. (A hidden
            // column's cell does not show: it is not told.)
            let row = groups.last_mut().filter(|group| group[3] < group[4] && spec.frame.parent == group[1]);
            let cell = row.map(|row| (row[3] += 1, (row[0] as usize, row[3] - 1)).1);
            let column = cell.and_then(|(_, column)| tracks.get(column as usize)).filter(|column| column.shows);
            if cell.is_some() && column.is_none() {
                inside = [at, spec.end];
                continue;
            }
            // The cell is told first, named by its text. Where it has only text, that is all;
            // else its content is told as inside it.
            if let Some(((row, number), column)) = cell.zip(column) {
                let (id, grid) = (hash((said[row].id, "cell", number)), [said[row].of.map_or(0, |of| of[0] - 1), column.place]);
                said.push(Said { grid: Some(grid), focused: keyed == Some(at), shown, seen, at, ..Said::plain(id, Role::Cell, words_in(tree, at, usize::MAX), lies, Some(row)) });
                groups.push([said.len() as u32 - 1, at, spec.end, 0, 0]);
            }
            if cell.is_some() && !used {
                inside = [at, spec.end];
                continue;
            }
            let holds_text = matches!(spec.content, Content::Text { start, end, .. } if start < end);
            let role = match note {
                Some(note) => note.role,
                None if spec.frame.focus => Role::Button,
                None if holds_text => Role::Text,
                None => continue,
            };
            let read = |range: [u32; 2]| &noted[range[0] as usize..range[1] as usize];
            let (given, typed, keys) = note.map_or(("", "", ""), |note| (read(note.name), read(note.value), read(note.keys)));
            // What has no name of its own is named by its text: all of it, or for a dialog or
            // group (a card) its first piece, its title. (A text box, combo box or slider is
            // not named by its content: a number dragged sideways has its value in it. Nor are
            // containers, except a table row, named by all its cells' text.)
            let wordy = matches!(role, Role::Dialog | Role::Group | Role::TableRow) || !role.holds();
            let unnamed = given.is_empty() && wordy && !matches!(role, Role::TextBox | Role::Secret | Role::Select | Role::Spin | Role::Slider);
            let mut name = if unnamed { words_in(tree, at, if matches!(role, Role::Dialog | Role::Group) { 1 } else { usize::MAX }) } else { given.to_string() };
            // An operable element without text saying what it is for (a slider, a combo box, a
            // number or text box) is named by the text right before it: as system forms label a
            // field by the text before it.
            if name.is_empty() && !role.holds() && role != Role::Text {
                name = label_before(tree, notes, at).to_string();
            }
            // A table row gives its index among all the table's rows; the table its row and
            // column counts.
            let within = groups.last().map(|group| group[0] as usize);
            let rows = within.and_then(|group| said[group].grid.filter(|_| said[group].role == Role::Table)).map(|table| table[0]);
            let (mut of, cells) = match note.and_then(|note| note.row) {
                Some([row, first, after, _]) => (Some([row + 1, rows.unwrap_or(row + 1)]), [first, after]),
                None => (note.and_then(|note| note.of), [0; 2]),
            };
            let in_list = rows_told.get(placed).filter(|(row, _)| *row <= at).map(|(_, of)| *of);
            (placed, of) = (placed + in_list.is_some() as usize, of.or(in_list));
            said.push(Said {
                id: if spec.id != 0 { spec.id } else { hash(("said", last, since)) },
                value: typed.to_string(),
                keys: keys.to_string(),
                on: note.and_then(|note| note.on),
                open: note.and_then(|note| note.open),
                level: note.map_or(0, |note| note.level),
                amount: note.and_then(|note| note.amount),
                step: note.map_or(0.0, |note| note.step),
                of,
                grid: note.and_then(|note| note.grid),
                sorted: note.and_then(|note| note.sorted),
                caret: note.and_then(|note| note.caret),
                focused: spec.id != 0 && spec.id == *focus,
                usable: note.is_none_or(|note| note.usable),
                shown,
                spelt_wrong: spelt_wrong(spelling, role, spec.id, typed),
                seen,
                at,
                ..Said::plain(0, role, name, lies, within)
            });
            (last, since) = if spec.id != 0 { (spec.id, 0) } else { (last, since + 1) };
            // (An element told on its own inside one told as one leaves the rest of that one
            // untold.)
            match role.holds() {
                true => groups.push([said.len() as u32 - 1, at, spec.end, cells[0], cells[1]]),
                false if within_one => {}
                false => inside = [at, spec.end],
            }
        }
        // A cell without text is named by its content's names.
        for cell in 0..said.len() {
            if said[cell].role != Role::Cell || !said[cell].name.is_empty() {
                continue;
            }
            let inner = said[cell + 1..].iter().take_while(|inner| inner.within.is_some_and(|within| within >= cell));
            let names: Vec<&str> = inner.filter(|inner| inner.within == Some(cell) && !inner.name.is_empty()).map(|inner| inner.name.as_str()).collect();
            said[cell].name = names.join(", ");
        }
        // A tree item that did not give its position (a hand-written one) is placed among its
        // siblings of the same depth under the same parent.
        for item in 0..said.len() {
            if said[item].role != Role::TreeItem || said[item].of.is_some() {
                continue;
            }
            let (level, within) = (said[item].level, said[item].within);
            let beside = |other: &&Said| other.role == Role::TreeItem && other.within == within;
            let before = said[..item].iter().rev().filter(beside).take_while(|other| other.level >= level).filter(|other| other.level == level).count();
            let after = said[item + 1..].iter().filter(beside).take_while(|other| other.level >= level).filter(|other| other.level == level).count();
            said[item].of = Some([before as u32 + 1, (before + after) as u32 + 1]);
        }
        // A radio button is placed among its group's radio buttons.
        for one in said.iter_mut().filter(|one| one.role == Role::Choice && one.of.is_none()) {
            let Some(group) = rovers.iter().find(|rover| rover.id == one.id && rover.picks).map(|rover| rover.group) else { continue };
            let mut choices = rovers.iter().filter(|rover| rover.group == group);
            let (count, nth) = (choices.clone().count(), choices.position(|rover| rover.id == one.id).unwrap_or(0));
            one.of = Some([nth as u32 + 1, count as u32]);
        }
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
        let Some(said) = self.told.iter().find(|said| said.id == id).filter(|said| (said.at as usize) < self.tree.specs.len()) else { return };
        let (at, spot) = (said.at, said.rect);
        self.show(at, spot);
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
        let Some(text) = self.told.iter().find(|said| said.id == id && said.role == Role::TextBox).map(|said| said.value.as_str()) else { return };
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

    /// Lists made, lists moved, and the list's turn.
    fn heard(trial: &Trial<Page>) -> (u32, u32, u64) {
        let heard = trial.ui.survey.telling.first().expect("a list kept while a screen reader listens");
        (heard.made, heard.moved, trial.ui.told_turn())
    }

    #[test]
    fn a_run_that_changes_nothing_told_tells_nothing_anew() {
        let mut trial = Trial::new(Page { first: "a row".into(), ticked: false, keyed_from: 0 }, describe);
        let (made, moved, turn) = heard(&trial);
        let told = trial.told().to_vec();
        assert!(told.len() == 600 && told.iter().any(|said| !said.shown), "every row is told, shown or not");
        for _ in 0..3 {
            trial.run();
        }
        assert_eq!((heard(&trial), trial.told()), ((made, moved, turn), &told[..]));
        // Scrolled, or hovered: the same list, moved. (A debug build checks each list kept or
        // moved against one made afresh.)
        trial.wheel(pt(100.0, 100.0), 0.0, 400.0);
        let now = heard(&trial);
        assert!(now.0 == made && now.1 > moved && now.2 != turn, "{now:?}");
        assert!(trial.told().len() == told.len() && trial.told() != told, "the list moved");
        trial.run();
        assert_eq!(heard(&trial), now);
        trial.wheel(pt(100.0, 100.0), 0.0, -400.0);
        assert_eq!(heard(&trial).0, made);
        // What changes what is told makes a list anew: a name, a state, focus, which box is
        // which.
        let steps: [(&str, Step); 4] = [
            ("a name", |trial| {
                trial.app.first = "the first row".into();
                trial.run();
            }),
            ("a state", |trial| trial.press(Role::Tick, "First")),
            ("focus", |trial| trial.key("Tab")),
            ("ids", |trial| {
                trial.app.keyed_from = 1000;
                trial.run();
            }),
        ];
        for (what, step) in steps {
            let was = heard(&trial);
            step(&mut trial);
            let now = heard(&trial);
            assert!(now.0 > was.0 && now.2 != was.2, "{what}: {was:?} then {now:?}");
            trial.run();
            assert_eq!(heard(&trial), now, "{what}, then nothing");
        }
        assert!(trial.app.ticked && trial.has(Role::Text, "the first row"));
        // A screen reader that stops listening leaves nothing kept; one that starts again is told
        // anew.
        trial.ui.listen(false);
        trial.run();
        assert!(trial.ui.survey.telling.is_empty() && trial.told().is_empty() && trial.ui.told_turn() == 0);
        trial.ui.listen(true);
        trial.run();
        assert_eq!(heard(&trial).0, 1);
        assert_eq!(trial.told().len(), told.len());
    }
}
