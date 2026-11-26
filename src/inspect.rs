//! Property inspector: an app's value as rows of a name and an editor, in two columns with a
//! draggable divider, as tools and game editors show the selection.
//!
//! ```ignore
//! #[derive(Clone, Default)]
//! struct Lamp {
//!     name: String,
//!     power: f32,
//!     on: bool,
//! }
//! taut::inspect!(Lamp { name: "Name", power: Field::new("Power").unit("W").within(0.0, 2000.0), on: "On" });
//!
//! ui.inspect(&mut app.inspector, &mut app.lamp);
//! ```
//!
//! A struct lists its fields ([`Inspect::fields`]), each with its options ([`Field`]); the rest
//! follows from each field's type. A number is typed and stepped, with a slider where its range
//! is set; text is typed; a `bool` is a check box; an enum is a dropdown; colours, dates and
//! paths (with the file dialog) have their own editors; nested lists, maps and structs are
//! collapsible headings with rows of their own. List rows can be added, removed, and reordered
//! by dragging or Alt+arrows.
//!
//! The inspector walks the value as it lists itself, counting rows, and describes only the rows
//! in view. The rest are counted and skipped: an off-screen field costs a counter step, and a
//! list skips its off-screen items entirely (one row each, unless expanded). So 1,000 fields,
//! or a 10,000-item list, cost about a screenful.
//!
//! A run walks the first value to count rows and note those in view; walks every other
//! inspected value, and the initial state, to compare those rows with the first; and walks the
//! first again to describe them. What the user does to a row (edit, reset, add, undo) is then
//! applied to each value, walking straight down to that row only.
use crate::accessibility::Role;
use crate::color::Color;
use crate::date::Date;
use crate::event::{Cursor, Key, Mods};
use crate::file_dialog::FileDialog;
use crate::id::{ById, Id, fold, hash};
use crate::layout::{Align, Flow, Length};
use crate::number::Number;
use crate::ui::{Icon, Measure, Response, Ui};
use crate::widgets::{Before, CROSS, DOWN, RIGHT};
use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

const PLUS: Icon = Icon::line("M8 3.5v9M3.5 8h9");
/// A curved arrow: resets a field to its initial value.
const BACK: Icon = Icon::line("M4.4 6.2A4.4 4.4 0 1 1 3.8 10.3M4.2 2.6v3.8H8");
/// Indent per nesting level.
const STEP_IN: f32 = 14.0;
/// The index of a row that is not a list item: a field, identified by name.
const NAMED: usize = usize::MAX;
/// Comparison results at a row: the values differ there; it differs from the initial value;
/// and, while comparing one of them, that it has that row at all.
const MIXED: u8 = 1;
const CHANGED: u8 = 2;
const MET: u8 = 4;
/// Undo depth, and how close together edits of one row merge into one step.
const STEPS: usize = 100;
const ONE_STEP: Duration = Duration::from_secs(1);

/// The path to a row: each heading on the way, with its index in its list (`NAMED` for a
/// field), and the row last.
type Way = Vec<(Id, usize)>;

/// A field's options: its name and how its editor behaves. A plain name (`"Power"`) is a field
/// with defaults.
///
/// ```ignore
/// fields.field(Field::new("Power").unit("W").within(0.0, 2000.0).step(10.0), &mut self.power);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field<'a> {
    /// The row's label.
    pub name: &'a str,
    /// A number's unit, shown after it ("W", "m/s"); empty for none.
    pub unit: &'a str,
    /// A number's minimum. With a maximum too, a slider shows beside it.
    pub min: f64,
    /// A number's maximum.
    pub max: f64,
    /// What Up and Down add or subtract; 0 for one.
    pub step: f64,
    /// Shown disabled: not editable here.
    pub read_only: bool,
    /// A tooltip, shown when the pointer rests on its name.
    pub note: &'a str,
    /// For a path: the extensions the file dialog lists (`&[".png"]`); none for all files.
    pub only: &'a [&'a str],
    /// For a path: the file dialog picks a folder, not a file.
    pub folder: bool,
}

impl<'a> Field<'a> {
    /// A field named `name`, with defaults.
    pub const fn new(name: &'a str) -> Self {
        Field { name, unit: "", min: f64::NEG_INFINITY, max: f64::INFINITY, step: 0.0, read_only: false, note: "", only: &[], folder: false }
    }
    /// Its number's unit.
    pub const fn unit(self, unit: &'a str) -> Self {
        Field { unit, ..self }
    }
    /// Its number's minimum.
    pub const fn min(self, least: f64) -> Self {
        Field { min: least, ..self }
    }
    /// Its number's maximum.
    pub const fn max(self, most: f64) -> Self {
        Field { max: most, ..self }
    }
    /// Its number's range: a slider shows between the two.
    pub const fn range(self, least: f64, most: f64) -> Self {
        Field { min: least, max: most, ..self }
    }
    /// What Up and Down add or subtract.
    pub const fn step(self, step: f64) -> Self {
        Field { step, ..self }
    }
    /// Shown, not editable here.
    pub const fn read_only(self) -> Self {
        Field { read_only: true, ..self }
    }
    /// A tooltip.
    pub const fn note(self, note: &'a str) -> Self {
        Field { note, ..self }
    }
    /// For a path: the file dialog lists only files with one of these extensions.
    pub const fn only(self, endings: &'a [&'a str]) -> Self {
        Field { only: endings, ..self }
    }
    /// For a path: a folder, picked with the file dialog.
    pub const fn folder(self) -> Self {
        Field { folder: true, ..self }
    }
}

impl<'a> From<&'a str> for Field<'a> {
    fn from(name: &'a str) -> Self {
        Field::new(name)
    }
}

/// Something an inspector shows: a single-row value with an editor beside its name (a number,
/// text, a colour), or a heading that expands to rows of its own (an app struct, a list, a
/// map).
///
/// An app struct just lists its fields:
///
/// ```ignore
/// impl Inspect for Lamp {
///     fn fields(&mut self, fields: &mut Fields) {
///         fields.field("Name", &mut self.name);
///         fields.field(Field::new("Power").unit("W"), &mut self.power);
///     }
/// }
/// ```
///
/// (`taut::inspect!` writes that for a plain struct, and the rest for a fieldless enum.) Every
/// number type, `bool`, `String`, [`Color`], [`Date`], `PathBuf`, `Vec`, `BTreeMap` with
/// `String` keys, and arrays are implemented. Values are `Clone`: a copy is kept for undo and
/// reset.
pub trait Inspect: Clone + 'static {
    /// A single-row value with an editor beside its name; not a heading.
    const ROW: bool = false;
    /// For a heading: lists its children, each with `fields.field(..)`, in display order.
    fn fields(&mut self, fields: &mut Fields) {
        let _ = fields;
    }
    /// For a single-row value: its editor, described beside the name; `changed` is set when the
    /// user edits it. For a heading: what shows beside its name (a list shows its length).
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        let _ = row;
        Response::default()
    }
    /// The same where the inspected values differ: an editor showing that they do; when edited
    /// (`changed`), every value is set as the first then is. By default, a "—" box that does
    /// that when pressed.
    fn mixed(&mut self, row: &mut InspectorRow) -> Response {
        row.differs()
    }
    /// A hash of the value: different values give different hashes. With several values
    /// inspected it shows where they differ; compared with the initial state, what was edited.
    /// (A struct's is 0: its rows are compared one by one.)
    fn hashed(&self) -> u64 {
        0
    }
}

/// Where an inspector row's editor is described: the UI, and the field's options.
pub struct InspectorRow<'a> {
    /// The UI: the editor is described with it.
    pub ui: &'a mut Ui,
    /// The field's options.
    pub field: Field<'a>,
    /// The inspected values differ here.
    pub mixed: bool,
    ins: &'a mut Inspector,
}

/// An inspector's state between runs, kept by the app: divider position, search text, expanded
/// headings, initial values, and the undo history.
pub struct Inspector {
    /// The divider position, as a fraction of the inspector's width. The user drags it.
    pub split: f32,
    /// Search text: only rows whose names contain it show, with their headings.
    pub search: String,
    /// The dialog a path row opens to pick a file or folder
    /// ([`FileDialog::native()`](FileDialog::native) for the system's).
    pub file_dialog: FileDialog,
    /// The inspector's own words, for localization: "Put back", "Add one", "Take out", "items",
    /// "Pick", "New key".
    pub words: [&'static str; 6],
    /// Expanded headings: each id, its parent heading, and its index in that one's list
    /// (`NAMED` for a field).
    opened: Vec<(Id, Id, usize)>,
    /// The initial values, if the app set them.
    start: Option<Box<dyn Any>>,
    /// The undo steps, the last last; and those undone, for redo.
    steps: Vec<Step>,
    undone: Vec<Step>,
    walk: Walk,
}

impl Default for Inspector {
    fn default() -> Self {
        let words = ["Put back", "Add one", "Take out", "items", "Pick", "New key"];
        Inspector { split: 0.4, search: String::new(), file_dialog: FileDialog::default(), words, opened: Vec::new(), start: None, steps: Vec::new(), undone: Vec::new(), walk: Walk::default() }
    }
}

impl std::fmt::Debug for Inspector {
    /// Its divider, search text, and undo and redo counts.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inspector").field("split", &self.split).field("search", &self.search).field("undo", &self.steps.len()).field("redo", &self.undone.len()).finish_non_exhaustive()
    }
}

impl Inspector {
    /// Sets the inspected values' initial state (as created, or last saved): a field that
    /// differs is marked and gets a reset button.
    pub fn start<T: Inspect>(&mut self, start: T) {
        self.start = Some(Box::new(start));
    }
    /// Whether undo and redo are available.
    pub fn can_undo(&self) -> [bool; 2] {
        [!self.steps.is_empty(), !self.undone.is_empty()]
    }
    /// Undoes the last edit on the inspector's next run (Ctrl+Z while focused); redo is Ctrl+Y.
    pub fn undo(&mut self) {
        self.walk.asked = Some(true);
    }
    /// Redoes the last undone edit on the inspector's next run.
    pub fn redo(&mut self) {
        self.walk.asked = Some(false);
    }
    /// Whether a heading is expanded.
    fn is_open(&self, id: Id) -> bool {
        self.opened.iter().any(|(open, ..)| *open == id)
    }
    /// Expands or collapses a heading.
    fn flip(&mut self, id: Id, index: usize) {
        match self.opened.iter().position(|(open, ..)| *open == id) {
            Some(at) => drop(self.opened.swap_remove(at)),
            None => self.opened.push((id, self.walk.parent(), index)),
        }
    }
    /// Prepares a run: where the visible rows are, the search, and whether the values are the
    /// ones the undo history was made with.
    fn begin<T: Inspect>(&mut self, line: f32, read: [f32; 3], offset: f32, things: &[&mut T]) {
        let walk = &mut self.walk;
        (walk.line, walk.names_wide) = (line, (read[0] * self.split.clamp(0.1, 0.9)).round());
        walk.view = (offset / line).max(0.0) as usize..((offset + read[1]) / line).ceil().max(0.0) as usize;
        walk.finding.clear();
        walk.finding.extend(self.search.trim().chars().flat_map(char::to_lowercase));
        walk.holding = things.len() > 1 || self.start.is_some();
        // Other values than last time: the history was theirs.
        let who = things.iter().fold(things.len() as u64, |so_far, thing| fold(so_far, &**thing as *const T as u64));
        if std::mem::replace(&mut walk.who, who) != who {
            self.steps.clear();
            self.undone.clear();
        }
        walk.seen.clear();
        walk.through.clear();
        walk.asks.clear();
    }
    /// Starts a walk through a value: from the first row, nothing entered.
    fn walk(&mut self, mode: Mode) {
        let walk = &mut self.walk;
        (walk.mode, walk.row) = (mode, 0);
        walk.frames.clear();
        walk.names.clear();
    }
    /// The next list item to look at while counting and describing rows: the next one in view,
    /// or expanded, after `from`. Skipped items are counted, a row each.
    fn next_listed(&mut self, len: usize, from: usize, rows: bool) -> Option<usize> {
        let Inspector { walk, opened, .. } = self;
        // While searching inside a list that didn't match: single-row items have no names to
        // match; each item with rows of its own is searched.
        if !walk.found() {
            return (!rows && from < len).then_some(from);
        }
        let list = walk.parent();
        let open = opened.iter().filter(|(_, under, at)| *under == list && *at >= from && *at < len).map(|(_, _, at)| *at).min().unwrap_or(len);
        let ahead = match walk.row {
            row if row < walk.view.start => from + (walk.view.start - row),
            row if row < walk.view.end => from,
            _ => len,
        };
        let next = ahead.min(open).min(len);
        walk.row += next - from;
        (next < len).then_some(next)
    }
    /// Records an undo step. An edit of the same row from the same editor as the last, while
    /// the pointer holds it or soon after, merges into that one.
    fn keep(&mut self, step: Step, joins: bool, held: bool) {
        self.undone.clear();
        let last = self.steps.last_mut().filter(|last| joins && last.way == step.way && last.focus == step.focus && (held || step.at.saturating_sub(last.at) < ONE_STEP));
        match last {
            Some(last) => (last.after, last.at) = (step.after, step.at),
            None => self.steps.push(step),
        }
        if self.steps.len() > STEPS {
            self.steps.remove(0);
        }
    }
    /// After list rows were removed or moved, expanded ones stay expanded at their new indices.
    fn reopen(&mut self, way: &[(Id, usize)], change: &Change) {
        let list = way.last().map_or(0, |(id, _)| *id);
        let to = |at: usize| match *change {
            Change::Remove(gone) if at == gone => None,
            Change::Remove(gone) => Some(at - (at > gone) as usize),
            Change::Move(from, to) if at == from => Some(to),
            Change::Move(from, to) if from < to && at > from && at <= to => Some(at - 1),
            Change::Move(from, to) if to < from && at >= to && at < from => Some(at + 1),
            _ => Some(at),
        };
        self.opened.retain_mut(|(id, under, at)| *under != list || to(*at).map(|now| (*at, *id) = (now, hash((list, now)))).is_some());
    }
}

/// A run's walk state, kept to reuse its memory.
#[derive(Default)]
struct Walk {
    mode: Mode,
    /// Row height, and the name column's width.
    line: f32,
    names_wide: f32,
    /// Rows counted so far; the total, as last counted; the visible ones.
    row: usize,
    count: usize,
    view: Range<usize>,
    /// The headings being walked, innermost last.
    frames: Vec<Frame>,
    /// The search, lower-cased; and the names of headings waiting to show.
    finding: String,
    names: String,
    /// Other values are compared with the first: several are inspected, or the app set initial
    /// values.
    holding: bool,
    /// The visible rows: the first value's hash at each, and the comparison results.
    seen: ById<(u64, u8)>,
    /// The headings on the way to the visible rows: for a list, its first and last item on the
    /// way.
    through: ById<[usize; 2]>,
    /// The path a walk goes down, each heading with its index in its list; what it puts there,
    /// or how it changes a list or map there; and what it found and left there.
    route: Way,
    deed: Option<Box<dyn Any>>,
    change: Option<Change>,
    acted: Option<(Box<dyn Any>, Box<dyn Any>)>,
    /// What the user asked during the run, each with its row's path; and an app-requested undo
    /// (`true`) or redo.
    asks: Vec<(Way, Ask)>,
    asked: Option<bool>,
    /// The row being described: its id and index.
    here: (Id, usize),
    /// The path to the row the file dialog was opened for.
    asking: Option<Way>,
    /// A map key being typed: for which map, and the text.
    key: (Id, String),
    /// A list row being dragged: the list's path, from where, to where.
    dragged_row: Option<(Way, usize, usize)>,
    /// The row focus goes to once described: which, which of its boxes, and whether this run or
    /// retried from the last.
    refocus: Option<(Id, usize, bool)>,
    /// Which values were inspected: when others are, the undo history no longer applies.
    who: u64,
    /// A row number, formatted.
    scratch: String,
}

/// A walk's purpose.
#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    /// Counting rows and noting the visible ones.
    #[default]
    Count,
    /// Comparing with the first value at the visible rows, marking differences with this.
    Hold(u8),
    /// Describing the visible rows.
    Draw,
    /// Going down to one row, to act there.
    Go,
}

/// A heading being walked.
struct Frame {
    id: Id,
    /// Its index in its list (`NAMED` for a field).
    index: usize,
    /// Everything under it follows expansion: no search, or it or an ancestor matched.
    found: bool,
    /// It shows once something under it matches: its name's range in `names`.
    waiting: Option<[u32; 2]>,
    /// Its kind, and a list's length.
    kind: Kind,
    len: usize,
}

/// A heading's kind: a struct; a list whose rows are added, removed and moved; a fixed-length
/// list; a map, whose rows are added and removed.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Struct,
    List,
    Fixed,
    Map,
}

/// A user request on a row during a run, applied to each value once the rows are described.
enum Ask {
    /// The first value here was edited, from the first to the second (the third is what the
    /// others get): whether it may merge into the last step.
    Changed(Box<dyn Any>, Box<dyn Any>, Box<dyn Any>, bool),
    /// Every value gets this here: a path the file dialog picked, a list as the first value has
    /// it.
    Put(Box<dyn Any>),
    /// Every value is reset here.
    Back,
    /// The list or map here changes.
    Change(Change),
}

/// How a list or map changes.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    Add,
    Remove(usize),
    Move(usize, usize),
    AddKey(String),
    RemoveKey(String),
}

/// An undoable edit: its row's path, and each value's content there before and after (`None`
/// where a value lacks the row).
struct Step {
    way: Way,
    before: Vec<Option<Box<dyn Any>>>,
    after: Vec<Option<Box<dyn Any>>>,
    /// The focused box, and when it was made.
    focus: Id,
    at: Duration,
}

impl Walk {
    /// The heading the current rows are under: 0 for the value itself.
    fn parent(&self) -> Id {
        self.frames.last().map_or(0, |frame| frame.id)
    }
    /// A row's id: its heading, and its name or list index.
    fn id_of(&self, name: &str, index: usize) -> Id {
        match index {
            NAMED => hash((self.parent(), name)),
            index => hash((self.parent(), index)),
        }
    }
    /// Whether the current rows follow expansion: no search, or under a matching heading.
    fn found(&self) -> bool {
        self.frames.last().map_or(self.finding.is_empty(), |frame| frame.found)
    }
    /// Whether a name contains the search, case-insensitive.
    fn matches(&self, name: &str) -> bool {
        let small = || name.chars().flat_map(char::to_lowercase);
        let (long, wanted) = (small().count(), self.finding.chars().count());
        !name.is_empty() && (0..=long.saturating_sub(wanted)).any(|from| small().skip(from).take(wanted).eq(self.finding.chars()))
    }
    /// Records a heading waiting to show: its name, or its list index.
    fn keep_name(&mut self, name: &str, index: usize) -> [u32; 2] {
        let from = self.names.len() as u32;
        match index {
            NAMED => self.names.push_str(name),
            index => drop(write!(self.names, "{index}")),
        }
        [from, self.names.len() as u32]
    }
    /// The path to the row being described, or to its heading.
    fn way(&self, to_row: bool) -> Way {
        let row = to_row.then_some(self.here);
        self.frames.iter().map(|frame| (frame.id, frame.index)).chain(row).collect()
    }
    /// Records a visible row and its path, for the other values to be compared with.
    fn note_seen(&mut self, id: Id, index: usize, hashed: u64) {
        self.seen.insert(id, (hashed, 0));
        let Walk { frames, through, .. } = self;
        let (mut under, down) = (0, frames.iter().map(|frame| (frame.id, frame.index)).chain([(id, index)]));
        for (on, at) in down {
            let span = through.entry(under).or_insert([usize::MAX, 0]);
            *span = if at == NAMED { *span } else { [span[0].min(at), span[1].max(at)] };
            under = on;
        }
    }
    /// Compares another value at a visible row with the first's.
    fn hold(&mut self, id: Id, hashed: u64, mark: u8) {
        if let Some(seen) = self.seen.get_mut(&id) {
            seen.1 |= MET | if seen.0 == hashed { 0 } else { mark };
        }
    }
    /// After comparing a value: a visible row it lacks differs too.
    fn held(&mut self, mark: u8) {
        for seen in self.seen.values_mut() {
            seen.1 = (seen.1 | if seen.1 & MET == 0 { mark } else { 0 }) & !MET;
        }
    }
}

/// The rows of an inspected value, as it lists them: what [`Inspect::fields`] gets.
pub struct Fields<'a> {
    ui: &'a mut Ui,
    ins: &'a mut Inspector,
}

impl std::fmt::Debug for Fields<'_> {
    /// The inspector being described into.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fields").field("inspector", &self.ins).finish_non_exhaustive()
    }
}

impl Fields<'_> {
    /// A field: its options (a plain name, or a [`Field`]), and its value, which its row shows
    /// and edits.
    pub fn field<'f, T: Inspect>(&mut self, field: impl Into<Field<'f>>, value: &mut T) {
        self.visit(field.into(), value, NAMED);
    }
    /// A list item, at its index.
    fn item<T: Inspect>(&mut self, index: usize, value: &mut T) {
        self.visit(Field::new(""), value, index);
    }
    fn visit<T: Inspect>(&mut self, field: Field, value: &mut T, index: usize) {
        match self.ins.walk.mode {
            Mode::Count | Mode::Draw => self.listed(field, value, index),
            _ => self.routed(field.name, value, index),
        }
    }
    /// For a list or map: records its kind and length; returns its change, if this walk goes to
    /// it to change it.
    fn changes(&mut self, kind: Kind, len: usize) -> Option<Change> {
        let walk = &mut self.ins.walk;
        if let Some(frame) = walk.frames.last_mut() {
            (frame.kind, frame.len) = (kind, len);
        }
        (walk.mode == Mode::Go && walk.frames.len() == walk.route.len()).then(|| walk.change.take()).flatten()
    }
    /// The next list item to look at, from `from`, of `len` (`rows`: single-row items). Skipped
    /// items are counted where visible.
    fn next_item(&mut self, len: usize, from: usize, rows: bool) -> Option<usize> {
        let walk = &mut self.ins.walk;
        match walk.mode {
            Mode::Count | Mode::Draw => self.ins.next_listed(len, from, rows),
            Mode::Hold(_) => {
                let span = walk.through.get(&walk.parent()).copied()?;
                let next = from.max(span[0]);
                (next <= span[1] && next < len).then_some(next)
            }
            Mode::Go => walk.route.get(walk.frames.len()).map(|(_, at)| *at).filter(|at| *at >= from && *at < len),
        }
    }

    /// A row while counting and describing: counted where visible, described where in view, and
    /// entered if an expanded heading (or, while searching, one that didn't match itself).
    fn listed<T: Inspect>(&mut self, field: Field, value: &mut T, index: usize) {
        let walk = &mut self.ins.walk;
        // Most common: a single-row value, no search. Out of view, it is just counted.
        if T::ROW && walk.finding.is_empty() {
            walk.row += 1;
            if walk.view.contains(&(walk.row - 1)) {
                self.in_view(field, value, index, false);
            }
            return;
        }
        let found = walk.found();
        let matched = !found && walk.matches(field.name);
        if T::ROW && !found && !matched {
            return;
        }
        let id = walk.id_of(field.name, index);
        let open = !T::ROW && self.ins.is_open(id);
        if found || matched {
            self.show_waiting();
            let walk = &mut self.ins.walk;
            walk.row += 1;
            if walk.view.contains(&(walk.row - 1)) {
                self.in_view(field, value, index, open);
            }
        }
        if !open && (found || matched) {
            return;
        }
        let walk = &mut self.ins.walk;
        let waiting = (!found && !matched).then(|| walk.keep_name(field.name, index));
        walk.frames.push(Frame { id, index, found: found || matched, waiting, kind: Kind::Struct, len: 0 });
        value.fields(self);
        self.ins.walk.frames.pop();
    }
    /// Something under the waiting headings matched: they show, a row each.
    fn show_waiting(&mut self) {
        for at in 0..self.ins.walk.frames.len() {
            let walk = &mut self.ins.walk;
            let Some(name) = walk.frames[at].waiting.take() else { continue };
            walk.row += 1;
            if walk.mode == Mode::Draw && walk.view.contains(&(walk.row - 1)) {
                self.waiting_heading(at, name);
            }
        }
    }
    /// A visible row: recorded, or described.
    fn in_view<T: Inspect>(&mut self, field: Field, value: &mut T, index: usize, open: bool) {
        let walk = &mut self.ins.walk;
        let id = walk.id_of(field.name, index);
        match walk.mode {
            Mode::Count if walk.holding => walk.note_seen(id, index, value.hashed()),
            Mode::Draw => self.row(field, value, id, index, open),
            _ => {}
        }
    }

    /// A row while going down to one row (to act there), or while comparing another value with
    /// the first.
    fn routed<T: Inspect>(&mut self, name: &str, value: &mut T, index: usize) {
        let walk = &mut self.ins.walk;
        let (id, depth) = (walk.id_of(name, index), walk.frames.len());
        if let Mode::Hold(mark) = walk.mode {
            walk.hold(id, value.hashed(), mark);
            if !T::ROW && walk.through.contains_key(&id) {
                self.go_into(id, index, value);
            }
            return;
        }
        match walk.route.get(depth) {
            Some((to, _)) if *to == id && depth + 1 < walk.route.len() => self.go_into(id, index, value),
            Some((to, _)) if *to == id => self.act(id, index, value),
            _ => {}
        }
    }
    fn go_into<T: Inspect>(&mut self, id: Id, index: usize, value: &mut T) {
        self.ins.walk.frames.push(Frame { id, index, found: true, waiting: None, kind: Kind::Struct, len: 0 });
        value.fields(self);
        self.ins.walk.frames.pop();
    }
    /// At the walk's target row: its content is taken, and the request applied.
    fn act<T: Inspect>(&mut self, id: Id, index: usize, value: &mut T) {
        let walk = &mut self.ins.walk;
        let before = Box::new(value.clone());
        if let Some(new) = walk.deed.as_deref().and_then(|deed| deed.downcast_ref::<T>()) {
            *value = new.clone();
        }
        if walk.change.is_some() {
            self.go_into(id, index, value);
        }
        self.ins.walk.acted = Some((before, Box::new(value.clone())));
    }

    /// Describes a row: the name, and the editor.
    fn row<T: Inspect>(&mut self, field: Field, value: &mut T, id: Id, index: usize, open: bool) {
        let (t, walk) = (self.ui.theme, &mut self.ins.walk);
        let marks = walk.seen.get(&id).map_or(0, |seen| seen.1);
        let (kind, len) = walk.frames.last().map_or((Kind::Fixed, 0), |frame| (frame.kind, frame.len));
        walk.here = (id, index);
        // A list row being dragged, and where it would go.
        let dragged = walk.dragged_row.as_ref().filter(|(way, ..)| way.last().map_or(0, |(list, _)| *list) == walk.parent() && index != NAMED);
        let fill = match dragged {
            Some((_, from, _)) if *from == index => t.selected,
            Some((_, _, to)) if *to == index => t.hover,
            _ => Color::NONE,
        };
        let reach = self.ui.reachable.len();
        self.ui.row().key(id).h(Length::Px(walk.line)).fill(fill).open();
        let name = self.name_cell(field, index, !T::ROW, open, marks & CHANGED != 0);
        if !T::ROW && (name.clicked || name.focused && (!open && self.ui.key(Key::Right, Mods::NONE) || open && self.ui.key(Key::Left, Mods::NONE))) {
            self.ins.flip(id, index);
            self.ui.again = true;
        }
        if kind == Kind::List && index != NAMED {
            self.drag_row(&name, index, len);
        }
        self.part_cell(field, value, index, marks, kind);
        self.ui.close();
        self.row_keys(id, index, kind, len, reach);
    }
    /// A row's name, indented by depth: for a heading, its expand toggle.
    fn name_cell(&mut self, field: Field, index: usize, heads: bool, open: bool, changed: bool) -> Response {
        let (ui, walk) = (&mut *self.ui, &mut self.ins.walk);
        let (t, item) = (ui.theme, index != NAMED && walk.frames.last().is_some_and(|frame| frame.kind == Kind::List));
        let state = if heads { ui.press(ui.next_id()) } else { ui.interact(ui.next_id()) };
        let cell = ui.row().w(Length::Px(walk.names_wide)).h(Length::Px(walk.line)).pad((6.0 + walk.frames.len() as f32 * STEP_IN, 0.0, 6.0, 0.0)).gap(4.0).clip();
        match (heads, item || !field.note.is_empty()) {
            (true, _) => cell.sense().focusable().open(),
            (false, true) => cell.sense().open(),
            (false, false) => cell.open(),
        };
        if heads {
            ui.tell(&state, Role::Fold, Some(open), "");
            ui.el().icon(if open { DOWN } else { RIGHT }, 12.0, t.text_dim).show();
        }
        walk.scratch.clear();
        let _ = if field.name.is_empty() { write!(walk.scratch, "{index}") } else { walk.scratch.write_str(field.name) };
        ui.el().text(&walk.scratch, t.font, if changed { t.accent } else { t.text }).shrink(1).show();
        ui.close();
        if item && (state.hovered || state.pressed) {
            ui.set_cursor(Cursor::Move);
        }
        ui.tooltip(&state, field.note);
        state
    }
    /// A heading shown because something under it matched: its name, expanded.
    fn waiting_heading(&mut self, at: usize, name: [u32; 2]) {
        let (ui, walk) = (&mut *self.ui, &mut self.ins.walk);
        let t = ui.theme;
        ui.row().key(walk.frames[at].id).h(Length::Px(walk.line)).pad((6.0 + at as f32 * STEP_IN, 0.0, 6.0, 0.0)).gap(4.0).open();
        ui.el().icon(DOWN, 12.0, t.text_faint).show();
        ui.el().text(&walk.names[name[0] as usize..name[1] as usize], t.font, t.text_dim).shrink(1).show();
        ui.close();
    }
    /// A row's editor: per its type, disabled if read-only; and the reset and remove buttons.
    fn part_cell<T: Inspect>(&mut self, field: Field, value: &mut T, index: usize, marks: u8, kind: Kind) {
        let (line, enabled) = (self.ins.walk.line, self.ui.enabled);
        self.ui.row().w(Length::Grow(1.0)).h(Length::Px(line)).pad((6.0, 0.0, 4.0, 0.0)).gap(6.0).clip().open();
        let before = T::ROW.then(|| value.clone());
        self.ui.set_enabled(enabled && !field.read_only);
        let mut row = InspectorRow { ui: &mut *self.ui, field, mixed: marks & MIXED != 0, ins: &mut *self.ins };
        let state = if row.mixed { value.mixed(&mut row) } else { value.part(&mut row) };
        // A list item is named by its index, formatted only for a listening screen reader.
        let mut named = String::new();
        if field.name.is_empty() && self.ui.listening {
            let _ = write!(named, "{index}");
        }
        self.ui.tell_name(&state, if field.name.is_empty() { &named } else { field.name });
        // An edited single-row value is undone to its old value; a heading edited where the
        // values differ (a list of another length) is applied to all as in the first.
        if state.changed {
            let ask = match before {
                Some(before) => Ask::Changed(Box::new(before), Box::new(value.clone()), Box::new(value.clone()), !state.clicked),
                None => Ask::Put(Box::new(value.clone())),
            };
            self.ask(true, ask);
        }
        let words = self.ins.words;
        if marks & CHANGED != 0 && self.small_button(BACK, words[0]) {
            self.ask(true, Ask::Back);
        }
        let taken = (kind == Kind::Map || kind == Kind::List && index != NAMED) && self.small_button(CROSS, words[2]);
        if taken {
            let change = if kind == Kind::Map { Change::RemoveKey(field.name.into()) } else { Change::Remove(index) };
            self.ask(false, Ask::Change(change));
        }
        self.ui.set_enabled(enabled);
        self.ui.close();
    }
    /// An icon-only button named `name`, at a row's end. Returns whether clicked.
    fn small_button(&mut self, icon: Icon, name: &str) -> bool {
        let pressed = self.ui.button_like(Before::Icon(icon), "", None, false);
        self.ui.tell(&pressed, Role::Button, None, name);
        pressed.clicked
    }
    /// Requests an action at the row being described (`to_row`), or at its heading.
    fn ask(&mut self, to_row: bool, ask: Ask) {
        let way = self.ins.walk.way(to_row);
        self.ins.walk.asks.push((way, ask));
    }
    /// A list item grabbed by its name: while dragged, where it would go.
    fn drag_row(&mut self, name: &Response, index: usize, len: usize) {
        let Some((gone, _)) = self.ui.drag_delta(name, [0.0; 4]) else { return };
        let walk = &mut self.ins.walk;
        let to = (index as f32 + (gone.y / walk.line).round()).clamp(0.0, len.saturating_sub(1) as f32) as usize;
        if gone.y.abs() >= 4.0 && walk.dragged_row.as_ref().is_none_or(|(_, _, was)| *was != to) {
            (walk.dragged_row, self.ui.again) = (Some((walk.way(false), index, to)), true);
        }
    }
    /// Keys on a described row: focus moves to it where a row was moved there; and Alt+Up/Down
    /// moves a focused list row.
    fn row_keys(&mut self, id: Id, index: usize, kind: Kind, len: usize, reach: usize) {
        let (ui, walk) = (&mut *self.ui, &mut self.ins.walk);
        if let Some((_, nth, _)) = walk.refocus.take_if(|(row, ..)| *row == id) {
            let to = ui.reachable.get(reach + nth).map(|(_, to, _)| *to);
            to.into_iter().for_each(|to| ui.set_focus(to));
        }
        let Some(nth) = ui.reachable[reach..].iter().position(|(_, on, _)| *on == ui.focus).filter(|_| kind == Kind::List && index != NAMED) else { return };
        let alt = Mods { alt: true, ..Mods::NONE };
        let to = match (ui.key(Key::Up, alt), ui.key(Key::Down, alt)) {
            (true, _) if index > 0 => index - 1,
            (_, true) if index + 1 < len => index + 1,
            _ => return,
        };
        walk.refocus = Some((hash((walk.parent(), to)), nth, true));
        let way = walk.way(false);
        walk.asks.push((way, Ask::Change(Change::Move(index, to))));
    }
}

impl std::fmt::Debug for InspectorRow<'_> {
    /// The row's field, and whether the inspected values differ in it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectorRow").field("field", &self.field).field("mixed", &self.mixed).finish_non_exhaustive()
    }
}

impl InspectorRow<'_> {
    /// A number of any type, per its field: typed, stepped by Up and Down, clamped to its range
    /// (with a slider if both ends are set), and its unit after it. (What every number type
    /// shows.)
    pub fn number<N: Number>(&mut self, value: &mut N) -> Response {
        let (ui, field, was) = (&mut *self.ui, self.field, *value);
        let ranged = field.min.is_finite() && field.max.is_finite();
        let mut slid = false;
        if ranged && !self.mixed {
            let mut at = value.to_f64() as f32;
            let slider = ui.slider(&mut at, field.min as f32..=field.max as f32);
            ui.tell_name(&slider, field.name);
            (*value, slid) = if slider.changed { (N::from_f64(at as f64), true) } else { (*value, false) };
        }
        ui.el().w(Length::Px(if ranged { 72.0 } else { 120.0 })).open();
        let step = N::from_f64(if field.step > 0.0 { field.step } else { 1.0 });
        let mut state = if self.mixed { ui.entry_box(format_args!("\u{2014}"), ui.locale_in_use(), "") } else { ui.number(value, step) };
        // Where values differ, text typed after the dash applies as soon as it is a number; the
        // box then shows it as the number of all of them.
        let typed = self.mixed && state.changed;
        let number = typed.then(|| ui.entry.1.trim().trim_start_matches('—').trim().parse::<N>().ok()).flatten();
        if let Some(number) = number {
            (*value, ui.entry.0) = (number, 0);
        }
        ui.close();
        let at = value.to_f64();
        if at < field.min || at > field.max {
            *value = N::from_f64(at.clamp(field.min, field.max));
        }
        if !field.unit.is_empty() {
            ui.note(field.unit);
        }
        state.changed = *value != was || slid || number.is_some();
        state
    }
    /// One of `cases`, each named by `names` at its index, picked from a dropdown (with more
    /// than twelve, a filterable one). What an enum's row shows: `taut::inspect!` writes it.
    pub fn cases<T: Clone + PartialEq>(&mut self, value: &mut T, cases: &[T], names: &[&str]) -> Response {
        let mut at = cases.iter().position(|case| case == value).unwrap_or(usize::MAX);
        let state = if names.len() > 12 { self.ui.combo(&mut at, names) } else { self.ui.select(&mut at, names) };
        if let Some(case) = cases.get(at).filter(|_| state.changed) {
            *value = case.clone();
        }
        state
    }
    /// A "—" box: where the inspected values differ, pressing it sets them all as the first is
    /// (`changed`), and its row shows their editor from then on.
    pub fn differs(&mut self) -> Response {
        let mut state = self.ui.button_like(Before::None, "\u{2014}", None, false);
        state.changed = state.clicked;
        state
    }
    /// A check box with a dash, where the values differ: pressing it checks them all.
    fn tick_differs(&mut self, value: &mut bool) -> Response {
        let ui = &mut *self.ui;
        let t = ui.theme;
        let mut state = ui.press(ui.next_id());
        let edge = if state.hovered { t.text_faint } else { t.line_strong };
        ui.row().size(16.0, 16.0).fill(t.panel).border(edge, 1.0).radius(4.0).align(Align::Center, Align::Center).sense().focusable().open();
        ui.el().size(8.0, 2.0).fill(t.text_dim).show();
        ui.close();
        ui.tell(&state, Role::CheckBox, None, "");
        *value |= state.clicked;
        state.changed = state.clicked;
        state
    }
    /// A path: typed, or picked with the file dialog (a folder, if the field says so).
    fn path(&mut self, value: &mut PathBuf) -> Response {
        let mut written = value.to_string_lossy().into_owned();
        let state = self.ui.text_box(&mut written, "");
        if state.changed {
            *value = PathBuf::from(written);
        }
        let pick = self.ui.button("\u{2026}");
        self.ui.tell(&pick, Role::Button, None, self.ins.words[4]);
        if !pick.clicked {
            return state;
        }
        // The dialog starts at the path, or the working folder.
        let folder = if self.field.folder { Some(value.as_path()) } else { value.parent() };
        let from = folder.filter(|folder| folder.is_dir()).map_or(PathBuf::from("."), PathBuf::from);
        let (field, ins) = (self.field, &mut *self.ins);
        match field.folder {
            true => ins.file_dialog.pick_folder(from),
            false => {
                ins.file_dialog.only(field.only);
                ins.file_dialog.pick(from);
            }
        }
        ins.walk.asking = Some(ins.walk.way(true));
        state
    }
    /// What a list heading shows beside its name: its length, and an add button.
    fn count(&mut self, len: usize, adds: bool) -> Response {
        let (t, ins) = (self.ui.theme, &mut *self.ins);
        ins.walk.scratch.clear();
        let _ = write!(ins.walk.scratch, "{len} {}", ins.words[3]);
        self.ui.text(&ins.walk.scratch, t.small_font, t.text_dim);
        if !adds {
            return Response::default();
        }
        let add = self.ui.button_like(Before::Icon(PLUS), "", None, false);
        self.ui.tell(&add, Role::Button, None, ins.words[1]);
        if add.clicked {
            let way = ins.walk.way(true);
            ins.walk.asks.push((way, Ask::Change(Change::Add)));
        }
        Response::default()
    }
    /// What a map heading shows: its length, a box for a new key, and an add button (Enter in
    /// the box too). An existing key (`has`) isn't added again.
    fn new_key(&mut self, len: usize, has: &dyn Fn(&str) -> bool) -> Response {
        self.count(len, false);
        let (ui, ins) = (&mut *self.ui, &mut *self.ins);
        let (id, word) = (ins.walk.here.0, ins.words[5]);
        let mut key = if ins.walk.key.0 == id { std::mem::take(&mut ins.walk.key.1) } else { String::new() };
        let typed = ui.text_box(&mut key, word);
        let add = ui.button_like(Before::Icon(PLUS), "", None, false);
        ui.tell(&add, Role::Button, None, ins.words[1]);
        let adds = (add.clicked || typed.focused && ui.key(Key::Enter, Mods::NONE)) && !key.is_empty() && !has(&key);
        if adds {
            let way = ins.walk.way(true);
            ins.walk.asks.push((way, Ask::Change(Change::AddKey(std::mem::take(&mut key)))));
        }
        if ins.walk.key.0 == id || !key.is_empty() {
            ins.walk.key = (id, key);
        }
        Response::default()
    }
}

/// Property inspectors: the app's values, viewed and edited in place.
impl Ui {
    /// An inspector for `value`: a row per field, name and editor, in two columns with a
    /// draggable divider; a search box above to filter fields by name; and collapsible headings
    /// for nested structs, lists and maps. Only visible rows are described. Ctrl+Z undoes while
    /// focused, Ctrl+Y redoes. `state` keeps its state between runs; `changed` is set when the
    /// user edits something.
    pub fn inspect<T: Inspect>(&mut self, state: &mut Inspector, value: &mut T) -> Response {
        self.inspect_all(state, &mut [value])
    }
    /// The same for several values at once (a multi-selection): the first one's rows, with "—"
    /// where the others differ, and an edit sets all.
    pub fn inspect_all<T: Inspect>(&mut self, state: &mut Inspector, things: &mut [&mut T]) -> Response {
        let (line, word) = (self.theme.control_height + 4.0, self.search_word);
        let mut all = self.column().grow().gap(6.0).sense().open();
        let reach = self.reachable.len();
        self.text_box(&mut state.search, word);
        self.el().key("body").flow(Flow::Stack).grow().open();
        let rows = self.id("rows");
        let read = self.rooms.get(&rows).copied().unwrap_or([self.size().w, self.size().h, 0.0]);
        let offset = self.scrolls.get(&rows).map_or(0.0, |offset| offset[1]);
        state.begin(line, read, offset, things);
        self.count_rows(state, things);
        // A box as tall as all rows, so its scroll bar is right; in it, a spacer for the rows
        // above the visible ones, then those.
        let at = self.tree.specs.len() as u32;
        self.column().key("rows").grow().scroll().open();
        self.measures.push(Measure { at, within: at, id: rows, read });
        self.column().h(Length::Px(state.walk.count as f32 * line)).open();
        self.el().h(Length::Px(state.walk.view.start as f32 * line)).show();
        state.walk(Mode::Draw);
        if let Some(first) = things.first_mut() {
            first.fields(&mut Fields { ui: self, ins: state });
        }
        state.walk.refocus = state.walk.refocus.take().filter(|(.., fresh)| *fresh).map(|(row, nth, _)| (row, nth, false));
        self.close();
        self.close();
        self.divider(state, read[0]);
        self.close();
        all.changed = self.answer_all(state, things, reach, all.contains_pointer);
        self.close();
        all
    }

    /// Counts the first value's rows, noting the visible ones; and compares the others with it
    /// there, and with the initial values.
    fn count_rows<T: Inspect>(&mut self, state: &mut Inspector, things: &mut [&mut T]) {
        state.walk(Mode::Count);
        let Some((first, others)) = things.split_first_mut() else { return state.walk.count = 0 };
        first.fields(&mut Fields { ui: self, ins: state });
        let walk = &mut state.walk;
        walk.count = walk.row;
        walk.view = walk.view.start.min(walk.count)..walk.view.end.min(walk.count);
        for other in others {
            state.walk(Mode::Hold(MIXED));
            other.fields(&mut Fields { ui: self, ins: state });
            state.walk.held(MIXED);
        }
        let Some(mut start) = state.start.take() else { return };
        if let Some(start) = start.downcast_mut::<T>() {
            state.walk(Mode::Hold(CHANGED));
            start.fields(&mut Fields { ui: self, ins: state });
            state.walk.held(CHANGED);
        }
        state.start = Some(start);
    }

    /// The draggable divider between names and editors.
    fn divider(&mut self, state: &mut Inspector, wide: f32) {
        let t = self.theme;
        let grip = self.interact(self.id("divider"));
        if grip.hovered || grip.pressed {
            self.set_cursor(Cursor::ResizeX);
        }
        if let Some((gone, was)) = self.drag_delta(&grip, [state.split, 0.0, 0.0, 0.0]) {
            let split = (was[0] + gone.x / wide.max(1.0)).clamp(0.1, 0.9);
            self.again |= split != state.split;
            state.split = split;
        }
        let line = if grip.hovered || grip.pressed { t.accent } else { t.line };
        self.row().key("divider").offset(state.walk.names_wide - 3.0, 0.0).w(Length::Px(7.0)).h(Length::Grow(1.0)).align(Align::Center, Align::Spread).sense().open();
        self.el().w(Length::Px(1.0)).fill(line).show();
        self.close();
    }

    /// Applies the user's requests of the run to every value: the file dialog's pick, a list
    /// row dropped elsewhere, the edits, and an undo or redo (by keys while focused, or nowhere
    /// while the pointer is `over` it). Returns whether anything was done.
    fn answer_all<T: Inspect>(&mut self, state: &mut Inspector, things: &mut [&mut T], reach: usize, over: bool) -> bool {
        let walk = &mut state.walk;
        let picked = self.file_dialog(&mut state.file_dialog).map(|path| (walk.asking.take(), path));
        if let Some((Some(way), path)) = picked {
            walk.asks.push((way, Ask::Put(Box::new(path))));
        }
        if let Some((way, from, to)) = walk.dragged_row.take_if(|_| !self.input.down[0]).filter(|(_, from, to)| from != to) {
            walk.asks.push((way, Ask::Change(Change::Move(from, to))));
        }
        let mine = self.reachable[reach..].iter().any(|(_, on, _)| *on == self.focus) || self.focus == 0 && over;
        let keyed = match mine {
            true if self.keys("Ctrl+Z") => Some(true),
            true if self.keys("Ctrl+Y") || self.keys("Ctrl+Shift+Z") => Some(false),
            _ => None,
        };
        let back = walk.asked.take().or(keyed);
        let done = !walk.asks.is_empty() || back.is_some();
        for (way, ask) in std::mem::take(&mut state.walk.asks) {
            self.answer(state, things, way, ask);
        }
        if let Some(back) = back {
            self.step_back(state, things, back);
        }
        // What is shown is what the values hold now: a number box being typed into shows it
        // afresh.
        if done {
            (self.again, self.entry.0) = (true, 0);
        }
        done
    }
    /// Applies one row request to every value, and records it as an undo step.
    fn answer<T: Inspect>(&mut self, state: &mut Inspector, things: &mut [&mut T], way: Way, ask: Ask) {
        let mut step = Step { way, before: Vec::new(), after: Vec::new(), focus: self.focus, at: self.now };
        // What each value gets there, from which value on (the first may have its edit
        // already).
        let (deed, change, from, joins) = match ask {
            Ask::Changed(before, after, deed, joins) => {
                (step.before, step.after) = (vec![Some(before)], vec![Some(after)]);
                (Some(deed), None, 1, joins)
            }
            Ask::Put(deed) => (Some(deed), None, 0, false),
            Ask::Back => (self.as_started::<T>(state, &step.way), None, 0, false),
            Ask::Change(change) => (None, Some(change), 0, false),
        };
        if deed.is_none() && change.is_none() {
            return;
        }
        if let Some(change) = &change {
            state.reopen(&step.way, change);
        }
        (state.walk.mode, state.walk.route, state.walk.deed) = (Mode::Go, std::mem::take(&mut step.way), deed);
        for thing in things.iter_mut().skip(from) {
            (state.walk.change, state.walk.frames) = (change.clone(), Vec::new());
            thing.fields(&mut Fields { ui: self, ins: state });
            let (before, after) = state.walk.acted.take().unzip();
            step.before.push(before);
            step.after.push(after);
        }
        (step.way, state.walk.deed) = (std::mem::take(&mut state.walk.route), None);
        state.keep(step, joins, self.input.down[0]);
    }
    /// The value at the end of a path in the initial state, if the app set it.
    fn as_started<T: Inspect>(&mut self, state: &mut Inspector, way: &[(Id, usize)]) -> Option<Box<dyn Any>> {
        let mut start = state.start.take()?;
        (state.walk.mode, state.walk.deed, state.walk.change) = (Mode::Go, None, None);
        state.walk.route.clear();
        state.walk.route.extend_from_slice(way);
        if let Some(start) = start.downcast_mut::<T>() {
            state.walk.frames.clear();
            start.fields(&mut Fields { ui: self, ins: state });
        }
        state.start = Some(start);
        state.walk.acted.take().map(|(before, _)| before)
    }
    /// Undoes the last step (`back`), or redoes the last undone one: each value gets back what
    /// it held before, or after.
    fn step_back<T: Inspect>(&mut self, state: &mut Inspector, things: &mut [&mut T], back: bool) {
        let Some(mut step) = (if back { state.steps.pop() } else { state.undone.pop() }) else { return };
        (state.walk.mode, state.walk.route, state.walk.change) = (Mode::Go, std::mem::take(&mut step.way), None);
        for (at, thing) in things.iter_mut().enumerate() {
            let kept = if back { &mut step.before } else { &mut step.after };
            state.walk.deed = kept.get_mut(at).and_then(Option::take);
            state.walk.frames.clear();
            thing.fields(&mut Fields { ui: self, ins: state });
            let kept = if back { &mut step.before } else { &mut step.after };
            kept.get_mut(at).into_iter().for_each(|kept| *kept = state.walk.deed.take());
            state.walk.acted = None;
        }
        step.way = std::mem::take(&mut state.walk.route);
        if back { state.undone.push(step) } else { state.steps.push(step) }
    }
}

macro_rules! numbers {
    ($($kind:ty)*) => {$(
        impl Inspect for $kind {
            const ROW: bool = true;
            fn part(&mut self, row: &mut InspectorRow) -> Response {
                row.number(self)
            }
            fn mixed(&mut self, row: &mut InspectorRow) -> Response {
                row.number(self)
            }
            fn hashed(&self) -> u64 {
                Number::to_f64(*self).to_bits()
            }
        }
    )*};
}
numbers!(i8 i16 i32 i64 isize u8 u16 u32 u64 usize f32 f64);

impl Inspect for bool {
    const ROW: bool = true;
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.ui.checkbox(self, "")
    }
    fn mixed(&mut self, row: &mut InspectorRow) -> Response {
        row.tick_differs(self)
    }
    fn hashed(&self) -> u64 {
        *self as u64
    }
}

impl Inspect for String {
    const ROW: bool = true;
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.ui.text_box(self, "")
    }
    /// A box showing "—" until something is typed, which every value then gets.
    fn mixed(&mut self, row: &mut InspectorRow) -> Response {
        let mut typed = String::new();
        let state = row.ui.text_box(&mut typed, "\u{2014}");
        if state.changed {
            *self = typed;
        }
        state
    }
    fn hashed(&self) -> u64 {
        hash(self.as_str())
    }
}

impl Inspect for Color {
    const ROW: bool = true;
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.ui.color_picker(self)
    }
    fn hashed(&self) -> u64 {
        self.0 as u64
    }
}

impl Inspect for Date {
    const ROW: bool = true;
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.ui.date(self)
    }
    fn hashed(&self) -> u64 {
        self.days() as u64
    }
}

impl Inspect for PathBuf {
    const ROW: bool = true;
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.path(self)
    }
    fn hashed(&self) -> u64 {
        hash(self.as_os_str())
    }
}

/// A list: a heading showing its length with an add button, and a row per item, removable, and
/// movable by dragging its name or with Alt+arrows.
impl<T: Inspect + Default> Inspect for Vec<T> {
    fn fields(&mut self, fields: &mut Fields) {
        match fields.changes(Kind::List, self.len()) {
            Some(Change::Add) => self.push(T::default()),
            Some(Change::Remove(at)) if at < self.len() => drop(self.remove(at)),
            Some(Change::Move(from, to)) if from.max(to) < self.len() => {
                let item = self.remove(from);
                self.insert(to, item);
            }
            _ => {}
        }
        let mut next = 0;
        while let Some(at) = fields.next_item(self.len(), next, T::ROW) {
            fields.item(at, &mut self[at]);
            next = at + 1;
        }
    }
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.count(self.len(), true)
    }
    fn hashed(&self) -> u64 {
        self.len() as u64
    }
}

/// A fixed-length list: a row per item.
impl<T: Inspect, const N: usize> Inspect for [T; N] {
    fn fields(&mut self, fields: &mut Fields) {
        fields.changes(Kind::Fixed, N);
        let mut next = 0;
        while let Some(at) = fields.next_item(N, next, T::ROW) {
            fields.item(at, &mut self[at]);
            next = at + 1;
        }
    }
}

/// A map by name: a row per key, in order, removable; and on its heading a box for a new key.
impl<T: Inspect + Default> Inspect for BTreeMap<String, T> {
    fn fields(&mut self, fields: &mut Fields) {
        match fields.changes(Kind::Map, self.len()) {
            Some(Change::AddKey(key)) => drop(self.entry(key).or_default()),
            Some(Change::RemoveKey(key)) => drop(self.remove(&key)),
            _ => {}
        }
        for (key, value) in self.iter_mut() {
            fields.visit(Field::new(key), value, NAMED);
        }
    }
    fn part(&mut self, row: &mut InspectorRow) -> Response {
        row.new_key(self.len(), &|key| self.contains_key(key))
    }
    fn hashed(&self) -> u64 {
        self.len() as u64
    }
}

/// Implements [`Inspect`] for a plain app struct, from each field's options; or for a fieldless
/// enum, from each variant's name.
///
/// ```ignore
/// taut::inspect!(Lamp { name: "Name", power: Field::new("Power").unit("W"), on: "On", kind: "Kind" });
/// taut::inspect!(enum Kind { Point: "A point", Spot: "A spot", Sun: "The sun" });
/// ```
///
/// The struct must be `Clone`, and every named field [`Inspect`]; the enum `Clone` and
/// `PartialEq`.
#[macro_export]
macro_rules! inspect {
    (enum $name:ident { $($case:ident: $label:expr),* $(,)? }) => {
        impl $crate::Inspect for $name {
            const ROW: bool = true;
            fn part(&mut self, row: &mut $crate::InspectorRow) -> $crate::Response {
                row.cases(self, &[$($name::$case),*], &[$($label),*])
            }
            fn hashed(&self) -> u64 {
                [$($name::$case),*].iter().position(|case| case == self).map_or(u64::MAX, |at| at as u64)
            }
        }
    };
    ($name:ident { $($field:ident: $about:expr),* $(,)? }) => {
        impl $crate::Inspect for $name {
            fn fields(&mut self, fields: &mut $crate::Fields) {
                $(fields.field($about, &mut self.$field);)*
            }
        }
    };
}
