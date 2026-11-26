//! Text editing state: cursor, selection, undo.
//!
//! Only one box has focus at a time, so there is one `Edit`, owned by the focused box. The text
//! itself stays with the app as a plain `String`.
//!
//! In bidirectional text (Hebrew or Arabic among Latin and digits), arrows move visually: Left
//! moves left on screen past the character there, whichever way it runs, as system text boxes
//! and browsers do. Home/End, Ctrl+arrows, deleting and selection follow logical order: a
//! selection is one range of the text, possibly shown as several pieces. Where two directions
//! meet, one text position shows at two places on the line (after the last letter of one run,
//! before the first of the other); the cursor remembers which (`after`), as the arrow or
//! pointer put it, and after typing it stands after what was typed.
use crate::accessibility::Role;
use crate::event::{Cursor, Key, Mods};
use crate::geom::Point;
use crate::id::Id;
use crate::layout::{Align, Flow, Length};
use crate::platform::Clipboard;
use crate::text::{Font, Line, Paragraph, ShownCluster, caret, cluster, cluster_before, shown_clusters};
use crate::ui::{Response, Stroke, Ui};
use std::ops::Range;
use std::time::Duration;

/// Gives each character's left and right edge on one line (from the piece's left), as it shows
/// in its paragraph, and returns the piece's width.
type Places<'a> = &'a mut dyn FnMut(&str, Paragraph, &mut Vec<[f32; 2]>) -> f32;

/// Cursor blink half-period.
const BLINK: Duration = Duration::from_millis(530);
/// Undo depth.
const HISTORY: usize = 200;
/// The character a password box shows for each character.
const DOT: char = '•';

/// A text box's cursor, selection and undo state ([`Ui::text_edit_state`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextEditState {
    /// Selection anchor and cursor, in bytes: the text between is selected (none if equal).
    pub selection: [usize; 2],
    /// Undo is available.
    pub can_undo: bool,
    /// Redo is available.
    pub can_redo: bool,
}

impl TextEditState {
    /// The cursor, in bytes.
    pub fn cursor(&self) -> usize {
        self.selection[1]
    }
    /// The selection as a range, whichever way it was made.
    pub fn range(&self) -> Range<usize> {
        self.selection[0].min(self.selection[1])..self.selection[0].max(self.selection[1])
    }
    /// The cursor's line and column in `text`, from 1: lines end at newlines (not wraps), and
    /// columns count grapheme clusters.
    pub fn line_column(&self, text: &str) -> (usize, usize) {
        let before = &text[..snap(text, self.cursor())];
        let start = before.rfind('\n').map_or(0, |end| end + 1);
        let mut column = 1;
        let mut at = start;
        while at < before.len() {
            (at, column) = (step(before, at, true), column + 1);
        }
        (before.matches('\n').count() + 1, column)
    }
}

/// An edit command for a text box from an app's own menu ([`Ui::text_command`]): what the box's
/// keys and context menu do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextCommand {
    /// Undoes the last change (Ctrl+Z).
    Undo,
    /// Redoes the last undone change (Ctrl+Y).
    Redo,
    /// Copies the selection to the clipboard and deletes it (Ctrl+X).
    Cut,
    /// Copies the selection to the clipboard (Ctrl+C).
    Copy,
    /// Replaces the selection with the clipboard text (Ctrl+V).
    Paste,
    /// Selects all text (Ctrl+A).
    SelectAll,
}

/// The kind of text box.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    /// One line.
    Line,
    /// Wrapping lines: at least so many high, at most so many before it scrolls.
    Lines(usize, usize),
    /// Wrapping lines in a box that fills its room and scrolls.
    Page,
    /// One hidden line: a password.
    Secret,
}

/// One change: `removed` went, `added` came, at byte `at`.
struct Change {
    at: usize,
    removed: String,
    added: String,
    /// Cursor and anchor before it.
    before: (usize, usize),
    /// It was typed, so letters typed right after it undo with it.
    typed: bool,
}

#[derive(Default)]
pub(crate) struct Edit {
    /// The box this belongs to.
    pub id: Id,
    /// The box hides its text: nothing goes to the clipboard, and the cursor sees no words
    /// (which would reveal where they end).
    pub secret: bool,
    /// Enter without Shift goes to the app ([`Ui::enter_submits`]).
    enter_left: bool,
    /// Cursor and anchor, in bytes: the text between is selected.
    pub cursor: usize,
    pub anchor: usize,
    /// The cursor shows at the near edge of the next character, not the far edge of the
    /// previous one (two places where directions meet).
    pub after: bool,
    /// The press moving the cursor has already set the anchor.
    anchor_set: bool,
    /// The word at the cursor is being written: the last change was to the text, and the cursor
    /// has not moved by itself since. (A misspelled word is marked only once ended: see
    /// `spell`.)
    pub writing: bool,
    /// The x the cursor keeps while moving up and down lines.
    column: Option<f32>,
    /// When the cursor last moved: it blinks from then.
    pub moved: Duration,
    /// The cursor moved and is not yet scrolled into view.
    pub reveal: bool,
    changes: Vec<Change>,
    /// How many of the last changes are undone now.
    undone: usize,
    /// Counts every change to the text.
    pub version: u32,
    /// The text's lines as last laid out (in bytes), the first one's window position, and line
    /// height.
    lines: Vec<Line>,
    origin: Point,
    line_height: f32,
    /// The box's text width: an RTL line is right-aligned in it.
    room: f32,
    places: Vec<[f32; 2]>,
    /// The text is empty (the box shows its hint), and where the system's edit menu was asked
    /// for (iOS).
    #[cfg(target_os = "ios")]
    pub empty: bool,
    #[cfg(target_os = "ios")]
    pub menu_at: Option<Point>,
}

/// What the system's keyboard did to the focused text box (iOS): bytes `range` replaced by
/// `with`, then the selection set (anchor, cursor).
#[cfg(target_os = "ios")]
pub(crate) struct Replace {
    pub range: Range<usize>,
    pub with: String,
    pub selection: [usize; 2],
}

/// What an input method deletes around the selection (Android's `deleteSurroundingText`): so
/// many characters before it and after it, counted in UTF-16 units, or in code points. A count
/// ending inside a character takes all of it.
#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DeleteAround {
    pub before: usize,
    pub after: usize,
    pub in_code_points: bool,
}

#[cfg(any(target_os = "android", test))]
impl DeleteAround {
    /// The bytes it deletes with `selected`: from before the selection to after it.
    pub(crate) fn range(self, text: &str, selected: Range<usize>) -> Range<usize> {
        let before = self.bytes(text[..selected.start].chars().rev(), self.before);
        let after = self.bytes(text[selected.end..].chars(), self.after);
        selected.start - before..selected.end + after
    }

    /// How many bytes of `chars` (going away from the selection) `count` covers: whole
    /// characters, as far as there are any.
    fn bytes(self, chars: impl Iterator<Item = char>, count: usize) -> usize {
        let size = |letter: char| if self.in_code_points { 1 } else { letter.len_utf16() };
        let taken = chars.scan(0, |counted, letter| {
            if *counted >= count {
                return None;
            }
            *counted += size(letter);
            Some(letter.len_utf8())
        });
        taken.sum()
    }
}

impl Edit {
    fn new(id: Id, cursor: usize) -> Self {
        Edit { id, cursor, anchor: cursor, ..Default::default() }
    }

    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    /// Clamps the cursor to the text, which the app may have changed.
    fn fit(&mut self, text: &str) {
        (self.cursor, self.anchor) = (snap(text, self.cursor), snap(text, self.anchor));
    }

    /// Takes this run's input for focused box `id`: made afresh for a new box, the cursor
    /// fitted to `text` (all of it selected where asked), Enter left to the app or not; a
    /// button `pressed` over the box (with Shift held or not) starts a new press. Returns the
    /// cursor, anchor and version before the input, for `done`.
    pub(crate) fn start(&mut self, id: Id, text: &str, enter_left: bool, select_all: bool, pressed: Option<bool>) -> (usize, usize, u32) {
        if self.id != id {
            *self = Edit::new(id, text.len());
        }
        self.enter_left = enter_left;
        self.fit(text);
        if select_all {
            (self.anchor, self.cursor) = (0, text.len());
        }
        if let Some(shift) = pressed {
            self.anchor_set = shift; // Shift keeps the anchor
        }
        (self.cursor, self.anchor, self.version)
    }

    /// Ends this run's input, started at `was`, at `now`: where the cursor moved, it blinks
    /// afresh and is brought into view. Whether the text changed.
    pub(crate) fn done(&mut self, was: (usize, usize, u32), now: Duration) -> bool {
        if was != (self.cursor, self.anchor, self.version) {
            (self.moved, self.reveal) = (now, true);
        }
        was.2 != self.version
    }

    /// Takes layout's result: the lines (in bytes of the whole run's text, this text starting
    /// at `start`), the first one's corner, line height, and available width.
    pub fn laid_out(&mut self, lines: &[Line], start: usize, origin: Point, line_height: f32, room: f32) {
        self.lines.clear();
        self.lines.extend(lines.iter().map(|l| Line { start: l.start - start, end: l.end - start, width: l.width }));
        // A password box was laid out as its dots, not as many bytes as the text: its one line
        // is all of the text.
        self.lines.iter_mut().filter(|_| self.secret).for_each(|line| line.end = usize::MAX);
        (self.origin, self.line_height, self.room) = (origin, line_height, room);
    }

    /// Replaces `range` with `with`, cursor after it.
    pub(crate) fn replace(&mut self, text: &mut String, range: Range<usize>, with: &str, typed: bool) {
        if range.is_empty() && with.is_empty() {
            return;
        }
        self.changes.truncate(self.changes.len() - self.undone);
        self.undone = 0;
        // Consecutive typed letters undo together, a word at a time.
        let last = self.changes.last_mut().filter(|c| typed && c.typed && range.is_empty() && c.at + c.added.len() == range.start);
        match last.filter(|c| !c.added.ends_with(char::is_whitespace) || with.starts_with(char::is_whitespace)) {
            Some(last) => last.added.push_str(with),
            None => self.changes.push(Change { at: range.start, removed: text[range.clone()].to_string(), added: with.to_string(), before: (self.cursor, self.anchor), typed }),
        }
        if self.changes.len() > HISTORY {
            self.changes.remove(0);
        }
        text.replace_range(range.clone(), with);
        (self.cursor, self.anchor, self.after) = (range.start + with.len(), range.start + with.len(), false);
        self.version += 1;
    }

    /// Inserts text at the cursor, replacing the selection. A one-line box turns newlines into
    /// spaces.
    pub(crate) fn insert(&mut self, text: &mut String, new: &str, many: bool, typed: bool) {
        let new: String = new.chars().filter(|c| *c != '\r').map(|c| if c == '\n' && !many { ' ' } else { c }).collect();
        self.replace(text, self.selection(), &new, typed);
    }

    /// Does what the system's keyboard did (iOS): a one-line box turns newlines into spaces;
    /// ends inside a character snap to its start.
    #[cfg(target_os = "ios")]
    pub(crate) fn keyboard(&mut self, text: &mut String, Replace { range, with, selection }: &Replace, many: bool) {
        let with: String = with.chars().filter(|c| *c != '\r').map(|c| if c == '\n' && !many { ' ' } else { c }).collect();
        let from = snap(text, range.start);
        self.replace(text, from..snap(text, range.end).max(from), &with, true);
        (self.anchor, self.cursor, self.after) = (snap(text, selection[0]), snap(text, selection[1]), false);
    }

    /// Deletes what an input method asks around the selection, which stays (Android).
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn delete_around(&mut self, text: &mut String, around: DeleteAround) {
        let selected = self.selection();
        let range = around.range(text, selected.clone());
        if range == selected {
            return;
        }
        let (kept, anchor_first) = (text[selected].to_string(), self.anchor <= self.cursor);
        self.replace(text, range.clone(), &kept, false);
        let [from, to] = [range.start, range.start + kept.len()];
        (self.anchor, self.cursor) = if anchor_first { (from, to) } else { (to, from) };
    }

    /// Undoes the last change, or (`again`) redoes the last undone one.
    fn undo(&mut self, text: &mut String, again: bool) {
        let at = if again { self.changes.len() - self.undone } else { self.changes.len().wrapping_sub(self.undone + 1) };
        let Some(change) = self.changes.get(at).filter(|_| !again || self.undone > 0) else { return };
        let (from, to) = if again { (&change.removed, &change.added) } else { (&change.added, &change.removed) };
        let range = change.at..change.at + from.len();
        if text.get(range.clone()) != Some(from) {
            // The app changed the text meanwhile: the history no longer fits it.
            self.changes.clear();
            self.undone = 0;
            return;
        }
        text.replace_range(range, to);
        (self.cursor, self.anchor) = if again { (change.at + to.len(), change.at + to.len()) } else { change.before };
        self.after = false;
        self.undone = if again { self.undone - 1 } else { self.undone + 1 };
        self.version += 1;
    }

    /// Line `row`'s byte range, clamped to the current text.
    fn span(&self, text: &str, row: usize) -> Range<usize> {
        let start = snap(text, self.lines[row].start);
        start..snap(text, self.lines[row].end).max(start)
    }

    /// The line byte `at` is on, as last laid out: its index and range. Before any layout, the
    /// text's own line.
    fn row(&self, text: &str, at: usize) -> (usize, Range<usize>) {
        match self.lines.iter().rposition(|l| l.start <= at) {
            Some(row) => (row, self.span(text, row)),
            None => (0, text[..at].rfind('\n').map_or(0, |i| i + 1)..at + text[at..].find('\n').unwrap_or(text.len() - at)),
        }
    }

    /// The cursor position in line `row` nearest `x` units from the line's left: between
    /// grapheme clusters; and whether it shows at the next character's edge.
    fn offset_in(&mut self, text: &str, row: usize, x: f32, places: Places) -> (usize, bool) {
        let span = self.span(text, row);
        let paragraph = self.paragraph(text, span.clone());
        let x = x - self.indent(row, paragraph.right_to_left);
        if self.secret || paragraph.plain(&text[span.clone()]) {
            return (self.ahead_offset(text, span, x, places), false);
        }
        let clusters = self.clusters(text, span.clone(), paragraph, places);
        nearest(&clusters, x).unwrap_or((span.start, false))
    }

    /// The same, in a line that runs one way only.
    fn ahead_offset(&mut self, text: &str, span: Range<usize>, x: f32, places: Places) -> usize {
        places(&text[span.clone()], Paragraph::whole(false), &mut self.places);
        // `places` has each `char`'s edges; a multi-`char` cluster ends with its last.
        let (mut at, mut ended, mut left) = (span.start, 0, 0.0);
        while at < span.end {
            let next = step(text, at, true).min(span.end);
            ended += text[at..next].chars().count();
            let right = self.places.get(ended - 1).map_or(left, |place| place[1]);
            if x < (left + right) / 2.0 {
                return at;
            }
            (at, left) = (next, right);
        }
        span.end
    }

    /// Line `row`'s indent from the box's left: an RTL line (`leftward`) is right-aligned.
    fn indent(&self, row: usize, leftward: bool) -> f32 {
        let wide = self.lines.get(row).map_or(self.room, |line| line.width);
        if leftward { (self.room - wide).max(0.0).round() } else { 0.0 }
    }

    /// The paragraph of byte range `span` (a line, or the cursor), as `Paragraph::of` finds it. (A
    /// password box shows dots, which run left to right.)
    fn paragraph<'a>(&self, text: &'a str, span: Range<usize>) -> Paragraph<'a> {
        if self.secret { Paragraph::whole(false) } else { Paragraph::of(text, span) }
    }

    /// The characters of byte range `span` (one line of `paragraph`), as shown.
    fn clusters(&mut self, text: &str, span: Range<usize>, paragraph: Paragraph, places: Places) -> Vec<ShownCluster> {
        places(&text[span.clone()], paragraph, &mut self.places);
        shown_clusters(&text[span.clone()], span.start, &self.places, paragraph.right_to_left)
    }

    /// The x where the cursor at byte `at` shows on its line, from the line's left.
    fn caret_in(&mut self, text: &str, span: Range<usize>, at: usize, places: Places) -> f32 {
        let paragraph = self.paragraph(text, span.clone());
        if self.secret || paragraph.plain(&text[span.clone()]) {
            return places(&text[span.start..at.clamp(span.start, span.end)], Paragraph::whole(false), &mut self.places);
        }
        let clusters = self.clusters(text, span, paragraph, places);
        caret(&clusters, at, self.after)
    }

    /// Where Left (or Right) moves the cursor visually: past the neighbouring character on that
    /// side. At the line's end on that side, to the next or previous line, by paragraph
    /// direction.
    fn beside(&mut self, text: &str, at: usize, right: bool, places: Places) -> (usize, bool) {
        let (row, span) = self.row(text, at);
        let paragraph = self.paragraph(text, span.clone());
        let forward = right != paragraph.right_to_left;
        if self.secret || paragraph.plain(&text[span.clone()]) {
            return (step(text, at, forward), false);
        }
        let clusters = self.clusters(text, span, paragraph, places);
        if let Some(place) = past(&clusters, caret(&clusters, at, self.after), right) {
            return place;
        }
        match (forward, self.lines.is_empty()) {
            (_, true) => (step(text, at, forward), forward),
            (true, false) => self.lines.get(row + 1).map_or((at, self.after), |next| (snap(text, next.start), true)),
            (false, false) => row.checked_sub(1).map_or((at, self.after), |before| (snap(text, self.lines[before].end), false)),
        }
    }

    /// The pointer is down on the text at `p`. A single click places the cursor (dragging
    /// selects from there); a double click selects the word, a triple click everything.
    fn point(&mut self, text: &str, p: Point, clicks: u8, places: Places) {
        let row = (((p.y - self.origin.y) / self.line_height.max(1.0)).max(0.0) as usize).min(self.lines.len() - 1);
        let (at, after) = self.offset_in(text, row, p.x - self.origin.x, places);
        self.after = after;
        self.press_at(text, at, clicks);
    }

    /// The pointer is down at byte `at`, the `clicks`th press in a row: as `point` says.
    pub(crate) fn press_at(&mut self, text: &str, at: usize, clicks: u8) {
        match (self.anchor_set, clicks) {
            (true, 2..) => {}
            (true, _) => self.cursor = at,
            (false, 2) if !self.secret => (self.anchor, self.cursor) = (word_edge(text, at, false), word_edge(text, at, true)),
            (false, 2..) => (self.anchor, self.cursor) = (0, text.len()),
            (false, _) => (self.anchor, self.cursor) = (at, at),
        }
        self.anchor_set = true;
    }

    /// Handles a key; returns whether the box used it. Unused keys are left to the app.
    fn key(&mut self, text: &mut String, key: Key, mods: Mods, many: bool, places: Places, clipboard: &mut dyn Clipboard) -> bool {
        if mods.alt {
            return false;
        }
        if self.change(text, key, mods, many, clipboard) {
            return true;
        }
        let Some((to, after)) = self.go(text, key, mods, many, places) else { return false };
        (self.cursor, self.after) = (to, after);
        if !mods.shift {
            self.anchor = to;
        }
        true
    }

    /// A key that changes the text or the selection; returns whether it was one.
    pub(crate) fn change(&mut self, text: &mut String, key: Key, mods: Mods, many: bool, clipboard: &mut dyn Clipboard) -> bool {
        let (at, selected) = (self.cursor, self.selection());
        // What Backspace and Delete remove with no selection: a character, or with Ctrl a word,
        // before or after the cursor.
        let forward = key == Key::Delete;
        let other = if mods.ctrl { self.word(text, at, forward) } else { step(text, at, forward) };
        let doomed = if selected.is_empty() { at.min(other)..at.max(other) } else { selected.clone() };
        match key {
            Key::Backspace | Key::Delete => self.replace(text, doomed, "", false),
            Key::Enter if many && (mods == Mods::SHIFT || mods == Mods::NONE && !self.enter_left) => self.replace(text, selected, "\n", true),
            Key::Char('a') if mods == Mods::CTRL => (self.anchor, self.cursor) = (0, text.len()),
            Key::Char('c') if mods == Mods::CTRL && !selected.is_empty() && !self.secret => clipboard.set(&text[selected]),
            Key::Char('x') if mods == Mods::CTRL && !selected.is_empty() && !self.secret => self.cut(text, selected, clipboard),
            Key::Char('c' | 'x') if mods == Mods::CTRL => {}
            Key::Char('v') if mods == Mods::CTRL => clipboard.get().iter().for_each(|pasted| self.insert(text, pasted, many, false)),
            Key::Char('z' | 'y') if mods.ctrl => self.undo(text, mods.shift || key == Key::Char('y')),
            _ => return false,
        }
        self.column = None;
        true
    }

    /// Copies `range` to the clipboard and deletes it.
    fn cut(&mut self, text: &mut String, range: Range<usize>, clipboard: &mut dyn Clipboard) {
        clipboard.set(&text[range.clone()]);
        self.replace(text, range, "", false);
    }

    /// Where a cursor movement key takes the cursor, and whether it then shows at the next
    /// character's edge; `None` for other keys. (In an RTL paragraph, word-wise Left goes
    /// forward through the text, Right back.)
    fn go(&mut self, text: &str, key: Key, mods: Mods, many: bool, places: Places) -> Option<(usize, bool)> {
        let (at, selected) = (self.cursor, self.selection());
        let column = self.column.take();
        let on = (key == Key::Right) != self.paragraph(text, at..at).right_to_left;
        Some(match key {
            Key::Left | Key::Right if mods.ctrl => (self.word(text, at, on), false),
            Key::Left | Key::Right if !mods.shift && !selected.is_empty() => (if on { selected.end } else { selected.start }, false),
            Key::Left | Key::Right => self.beside(text, at, key == Key::Right, places),
            Key::Home if mods.ctrl => (0, true),
            Key::End if mods.ctrl => (text.len(), false),
            Key::Home => (self.row(text, at).1.start, true),
            Key::End => (self.row(text, at).1.end, false),
            Key::Up | Key::Down if many => self.line_away(text, at, key == Key::Up, column, places),
            // In a single line, Up goes to its start and Down to its end, and focus stays, as
            // in system text boxes: screen reader users press them to hear the line again.
            Key::Up => (0, true),
            Key::Down => (text.len(), false),
            _ => return None,
        })
    }

    /// Where Ctrl+arrow moves the cursor from `at`: a word on or back; in a password box, to
    /// the end or start.
    fn word(&self, text: &str, at: usize, forward: bool) -> usize {
        match (self.secret, forward) {
            (false, _) => word(text, at, forward),
            (true, true) => text.len(),
            (true, false) => 0,
        }
    }

    /// The position one line up or down, at the x the cursor keeps while moving vertically.
    fn line_away(&mut self, text: &str, at: usize, up: bool, column: Option<f32>, places: Places) -> (usize, bool) {
        let (row, line) = self.row(text, at);
        let x = match column {
            Some(x) => x,
            None => self.caret_in(text, line.clone(), at, places) + self.indent(row, self.paragraph(text, line).right_to_left),
        };
        self.column = Some(x);
        match up {
            true if row == 0 => (0, true),
            true => self.offset_in(text, row - 1, x, places),
            false if row + 1 >= self.lines.len() => (text.len(), false),
            false => self.offset_in(text, row + 1, x, places),
        }
    }
}

/// The position past the character to the right (or left) of where the cursor shows, `x`: that
/// character's far side, at its edge. `None` at the line's end on that side.
fn past(clusters: &[ShownCluster], x: f32, right: bool) -> Option<(usize, bool)> {
    let next = clusters.iter().filter(|one| one.edges[1] > one.edges[0]).find(|one| (if right { one.edges[0] } else { one.edges[1] } - x).abs() < 0.01)?;
    Some(match right == next.leftward {
        true => (next.bytes.start, true),
        false => (next.bytes.end, false),
    })
}

/// The cursor position nearest `x` on a line: each edge of each character is one, before or
/// after it in its direction. `None` for an empty line.
fn nearest(clusters: &[ShownCluster], x: f32) -> Option<(usize, bool)> {
    let before = |one: &ShownCluster| (one.bytes.start, true);
    let after = |one: &ShownCluster| (one.bytes.end, false);
    let edges = clusters.iter().flat_map(|one| match one.leftward {
        true => [(one.edges[0], after(one)), (one.edges[1], before(one))],
        false => [(one.edges[0], before(one)), (one.edges[1], after(one))],
    });
    edges.min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs())).map(|(_, place)| place)
}

/// The nearest character boundary at or before byte `at`.
fn snap(text: &str, at: usize) -> usize {
    (0..=at.min(text.len())).rev().find(|at| text.is_char_boundary(*at)).unwrap_or(0)
}

/// The next (or previous) cursor position after `at`: past one grapheme cluster, which may be
/// several `char`s (a letter and its combining accent, an emoji sequence). The cursor never
/// stands inside one, so it is stepped over, selected and deleted whole.
pub(crate) fn step(text: &str, at: usize, forward: bool) -> usize {
    if forward { at + cluster(&text[at..]) } else { cluster_before(text, at) }
}

/// Whether the character at `at` is part of a word (judged by its first `char`: combining marks
/// go with their letter).
pub(crate) fn in_word(text: &str, at: usize) -> bool {
    text[at..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// The end (or start) of the word at, or just before, `at`.
pub(crate) fn word_edge(text: &str, at: usize, end: bool) -> usize {
    let mut to = at;
    while end && in_word(text, to) {
        to = step(text, to, true);
    }
    while !end && to > 0 && in_word(text, step(text, to, false)) {
        to = step(text, to, false);
    }
    // Not on a word: the one character there.
    if end && to == at { step(text, at, true) } else { to }
}

/// Where Ctrl+arrow moves the cursor: past the gap between words, then to the word's far end.
pub(crate) fn word(text: &str, at: usize, forward: bool) -> usize {
    let mut to = at;
    while forward && to < text.len() && !in_word(text, to) {
        to = step(text, to, true);
    }
    while !forward && to > 0 && !in_word(text, step(text, to, false)) {
        to = step(text, to, false);
    }
    word_edge(text, to, forward)
}

/// Text boxes: one line, password, multi-line, page.
impl Ui {
    /// A one-line text box. `hint` shows while empty. `changed` is set when the user edits the
    /// text; `focused` while they type in it.
    pub fn text_box(&mut self, text: &mut String, hint: &str) -> Response {
        self.edit_box(text, hint, Kind::Line, f32::INFINITY)
    }
    /// A password box: shows a dot per character. Nothing can be copied or cut, a double click
    /// selects all, and screen readers get only the dots. (To show the password, describe a
    /// [`text_box`](Self::text_box) in its place: it is the same box, so the cursor stays.)
    #[doc(alias("password"))]
    pub fn password_box(&mut self, text: &mut String, hint: &str) -> Response {
        self.edit_box(text, hint, Kind::Secret, f32::INFINITY)
    }
    /// A multi-line, wrapping text box: `rows.0` lines high, growing to `rows.1`, then
    /// scrolling.
    #[doc(alias("textarea", "multiline"))]
    pub fn text_area(&mut self, text: &mut String, hint: &str, rows: (usize, usize)) -> Response {
        self.edit_box(text, hint, Kind::Lines(rows.0, rows.1), f32::INFINITY)
    }
    /// A multi-line, wrapping text box that fills its room (as a box growing both ways does)
    /// and scrolls: an editor page, a message body. A click anywhere in it, below the last line
    /// too, focuses it.
    pub fn text_page(&mut self, text: &mut String, hint: &str) -> Response {
        self.edit_box(text, hint, Kind::Page, f32::INFINITY)
    }
    /// Makes the next multi-line box leave Enter to the app (to send a message: check
    /// [`ui.key(Key::Enter, Mods::NONE)`](Ui::key) after the box); Shift+Enter makes a new
    /// line. (By default Enter makes a new line, with Shift or not.)
    pub fn enter_submits(&mut self) {
        self.asks.enter_left = true;
    }

    /// Makes the next text box keep this much free at its left and right ends, in units: for an
    /// app's own icon or button laid over it (a search glass, a clear button). Text never runs
    /// under it.
    pub fn text_margin(&mut self, left: f32, right: f32) {
        self.edit_room = [left, right];
    }

    /// A text box of `kind`, filling its width up to `widest`.
    pub(crate) fn edit_box(&mut self, text: &mut String, hint: &str, kind: Kind, widest: f32) -> Response {
        #[cfg(target_os = "android")]
        crate::android_input_method::hand_over();
        let (t, many, secret) = (self.theme, matches!(kind, Kind::Lines(..) | Kind::Page), kind == Kind::Secret);
        let line = self.text.metrics(t.font, self.scale).height().ceil();
        let (mut state, enter_left) = (self.interact(self.next_id()), std::mem::take(&mut self.asks.enter_left));
        if state.hovered {
            self.set_cursor(Cursor::Text);
        }
        if state.focused {
            state.changed = self.type_into(text, &state, kind, enter_left);
        }
        // A password shows a dot per character, and the cursor and selection sit among the dots
        // as among the characters.
        let hidden: String = text.chars().filter(|_| secret).map(|_| DOT).collect();
        let seat = |at: usize| if secret { text[..at].chars().count() * DOT.len_utf8() } else { at };
        let range = state.focused.then(|| [seat(self.edit.anchor) as u32, seat(self.edit.cursor) as u32]);
        let cursor = state.focused && self.window_focused && self.blink(self.edit.moved);
        let edge = if state.focused {
            t.accent
        } else if state.hovered {
            t.text_faint
        } else {
            t.line_strong
        };
        let (shown, ink) = match (text.is_empty(), secret) {
            (true, _) => (hint, t.text_faint),
            (false, true) => (hidden.as_str(), t.text),
            (false, false) => (text.as_str(), t.text),
        };
        let [left, right] = std::mem::take(&mut self.edit_room);
        // A single RTL line fills the box and is right-aligned, as in any box: so the box's
        // line is laid out as a column.
        let filled = !many && !secret && crate::bidi::leftward(shown);
        let (flow, along, across) = if filled { (Flow::Column, Align::Center, Align::Start) } else { (Flow::Row, Align::Start, Align::Center) };
        let frame = self.el().flow(flow).w(Length::Grow(1.0)).fill(t.panel).border(edge, 1.0).radius(t.radius).sense().focusable();
        // One line: button height, scrolled sideways. Several: as high as its wrapped lines,
        // between the minimum and maximum.
        let frame = match kind {
            Kind::Lines(least, most) => frame.pad((8.0 + left, 6.0, 8.0 + right, 6.0)).min(0.0, least as f32 * line + 12.0).max(f32::INFINITY, most as f32 * line + 12.0).scroll(),
            Kind::Page => frame.pad((8.0 + left, 6.0, 8.0 + right, 6.0)).h(Length::Grow(1.0)).min(0.0, line + 12.0).scroll(),
            _ => frame.h(Length::Px(t.control_height)).max(widest, f32::INFINITY).pad((8.0 + left, 0.0, 8.0 + right, 0.0)).align(along, across).scroll_x(),
        };
        frame.open();
        // Screen readers get the text (or the dots), and while focused, the cursor position in
        // characters.
        match secret {
            true => self.keep_note(&state, Role::PasswordBox, None, hint, &hidden),
            false => self.tell_text(&state, hint, text),
        }
        let letters = |at: usize| text[..at].chars().count() as u32;
        if state.focused {
            self.tell_caret([letters(self.edit.anchor), letters(self.edit.cursor)]);
        }
        let words = self.el().text(shown, t.font, ink).select(range, cursor);
        match (many, filled) {
            (true, _) => words.w(Length::Grow(1.0)).wrap().show(),
            (false, true) => words.w(Length::Grow(1.0)).show(),
            (false, false) => words.show(),
        };
        // A spell-checked box is noted with where its text is: see `spell`. Not its hint or
        // dots.
        if let Some(hooks) = self.asks.spelling.hooks() {
            let at = (!text.is_empty() && !secret).then(|| self.tree.specs.len() as u32 - 1);
            (hooks.noted)(self, &state, at);
        }
        self.close();
        self.edit_menu(&state, secret);
        state
    }

    /// A text box's context menu: cut, copy, paste and select all, for those who don't know the
    /// keys. A row does what its key does: focus returns to the box, and the key counts next
    /// run. Hidden text is neither cut nor copied; with no selection, there is nothing to cut
    /// or copy. (On a misspelled word in a spell-checked box, it starts with suggestions: see
    /// `spell`.)
    pub(crate) fn edit_menu(&mut self, state: &Response, secret: bool) {
        if state.right_clicked {
            self.set_focus(state.id);
        }
        // On iOS the system's edit menu shows instead, where the box was pressed.
        #[cfg(target_os = "ios")]
        if state.right_clicked {
            self.edit.menu_at = self.pointer();
        }
        if cfg!(target_os = "ios") || !self.context_menu(state) {
            return;
        }
        if let Some(hooks) = self.asks.spelling.hooks() {
            (hooks.rows)(self, state);
        }
        let takes = !secret && self.edit.id == state.id && self.edit.cursor != self.edit.anchor;
        for (word, letter, on) in [(0, 'x', takes), (1, 'c', takes), (2, 'v', true), (3, 'a', true)] {
            self.set_enabled(on);
            if self.menu_item(self.edit_words[word]).clicked {
                self.again_key = Some((Key::Char(letter), Mods::CTRL));
                self.set_focus(state.id);
            }
        }
        self.set_enabled(true);
        self.close();
    }

    /// Feeds pointer, keys and typed text to the focused box. Returns whether the text changed.
    fn type_into(&mut self, text: &mut String, state: &Response, kind: Kind, enter_left: bool) -> bool {
        // In a pan-and-zoom area, text is scaled with it.
        let font = Font { size: self.theme.font.size * self.zoom_here(), ..self.theme.font };
        let (many, secret) = (matches!(kind, Kind::Lines(..) | Kind::Page), kind == Kind::Secret);
        let Ui { edit, input, text: system, clipboard, scale, now, again, select_all, asks, .. } = self;
        // A password is as wide as its dots: those are measured.
        let mut dots = String::new();
        let places: Places = &mut |piece, paragraph, places| {
            dots.extend(piece.chars().filter(|_| secret).map(|_| DOT));
            let wide = system.places(if secret { &dots } else { piece }, font, *scale, paragraph, places);
            dots.clear();
            wide
        };
        let pressed = (state.hovered && input.pressed[0]).then_some(input.mods.shift);
        let was = edit.start(state.id, text, enter_left, std::mem::take(select_all), pressed);
        edit.secret = secret;
        if let (true, Some(p)) = (state.pressed, input.pointer) {
            match edit.lines.is_empty() {
                true => *again = true, // not yet laid out as the focused box: next run
                false => edit.point(text, p, input.clicks, places),
            }
        }
        input.strokes.retain(|stroke| match stroke {
            Stroke::Key(key, mods) => !edit.key(text, *key, *mods, many, places, clipboard.as_mut()),
            Stroke::Text(typed) => {
                edit.insert(text, typed, many, true);
                false
            }
            #[cfg(any(target_os = "android", test))]
            Stroke::DeleteAround(around) => {
                edit.delete_around(text, *around);
                false
            }
            #[cfg(target_os = "ios")]
            Stroke::Replace(done) => {
                edit.keyboard(text, done, many);
                false
            }
        });
        // The system's keyboard reads text boxes once one is typed in (iOS: see `ios_text_input`).
        #[cfg(target_os = "ios")]
        {
            edit.empty = text.is_empty();
            crate::ios_text_input::hand_text_boxes();
        }
        // What spell checking needs to know of typing, and of a word corrected from its menu:
        // see `spell`.
        if let Some(hooks) = asks.spelling.hooks() {
            (hooks.typed)(&mut asks.spelling, edit, text, state.id, was);
        }
        let changed = edit.done(was, *now);
        *again |= changed; // an earlier box may show the text too
        changed
    }

    /// The cursor, selection and undo state of text box `id` (focused, or the last that was),
    /// as last described. For a status line ("line 3, column 7") or an app's Edit menu, whose
    /// rows can be disabled when there is nothing to do. `None` for other boxes.
    pub fn text_edit_state(&self, id: Id) -> Option<TextEditState> {
        let edit = &self.edit;
        (id != 0 && edit.id == id).then_some(TextEditState { selection: [edit.anchor, edit.cursor], can_undo: edit.undone < edit.changes.len(), can_redo: edit.undone > 0 })
    }
    /// Selects bytes `from` to `to` in text box `id` (cursor at `to`; equal values place the
    /// cursor), focuses the box and scrolls the selection into view: for find, or go to. A
    /// position inside a character snaps to its start.
    pub fn select_text(&mut self, id: Id, [from, to]: [usize; 2]) {
        self.reader_selects = Some((id, [from, to], false));
        self.set_focus(id);
    }
    /// Runs an Edit menu command on text box `id`: undo, redo, cut, copy, paste, select all, as
    /// its keys and context menu do. Focus returns to the box, and it happens before anything
    /// is drawn. What cannot be done (nothing selected to cut) does nothing.
    pub fn text_command(&mut self, id: Id, command: TextCommand) {
        let letter = ['z', 'y', 'x', 'c', 'v', 'a'][command as usize];
        self.again_key = Some((Key::Char(letter), Mods::CTRL));
        self.set_focus(id);
    }

    /// Whether a blinking cursor last moved at `moved` shows now; asks to rerun when that
    /// changes.
    pub(crate) fn blink(&mut self, moved: Duration) -> bool {
        let since = self.now.saturating_sub(moved).as_millis();
        let period = BLINK.as_millis();
        self.wake_after(Duration::from_millis((period - since % period) as u64));
        (since / period).is_multiple_of(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::LocalClipboard;
    use crate::text::{BuiltInFont, Font, TextSystem};

    /// A text box with the cursor at its end, and keys to press. Letters are 6 wide.
    struct Typing(String, Edit, LocalClipboard);
    impl Typing {
        fn new(text: &str) -> Self {
            Typing(text.into(), Edit::new(1, text.len()), LocalClipboard::default())
        }
        fn key(&mut self, key: Key, mods: Mods) -> &mut Self {
            let places: Places = &mut |piece, paragraph, places| BuiltInFont.places(piece, Font::new(8.0), 1.0, paragraph, places);
            assert!(self.1.key(&mut self.0, key, mods, true, places, &mut self.2), "{key:?} was not used");
            self
        }
        fn keys(&mut self, keys: &[Key]) -> &mut Self {
            keys.iter().for_each(|key| _ = self.key(*key, Mods::NONE));
            self
        }
        fn typed(&mut self, text: &str) -> &mut Self {
            self.1.insert(&mut self.0, text, true, true);
            self
        }
        /// An input method deletes `before` and `after` the selection, in UTF-16 units or code
        /// points.
        fn delete_around(&mut self, before: usize, after: usize, in_code_points: bool) -> &mut Self {
            self.1.delete_around(&mut self.0, DeleteAround { before, after, in_code_points });
            self
        }
        /// The text, with `|` at the cursor and `[` `]` around the selection.
        fn shown(&self) -> String {
            let (mut text, picked) = (self.0.clone(), self.1.selection());
            if picked.is_empty() {
                text.insert(self.1.cursor, '|');
            } else {
                text.insert(picked.end, ']');
                text.insert(picked.start, '[');
            }
            text
        }
    }

    #[test]
    fn typing_goes_where_the_cursor_is_and_replaces_what_is_selected() {
        let mut t = Typing::new("fix arc");
        t.keys(&[Key::Left, Key::Left, Key::Left]).typed("jump ");
        assert_eq!(t.shown(), "fix jump |arc");
        t.key(Key::Left, Mods { ctrl: true, shift: true, alt: false }).typed("the ");
        assert_eq!(t.shown(), "fix the |arc");
        t.key(Key::Char('a'), Mods::CTRL);
        assert_eq!(t.shown(), "[fix the arc]");
        t.keys(&[Key::Right, Key::Backspace]);
        assert_eq!(t.shown(), "fix the ar|");
    }

    #[test]
    fn an_input_method_deletes_whole_emoji_counted_in_utf16_units() {
        // An emoji is two units: both go, and nothing before it.
        assert_eq!(Typing::new("a😀").delete_around(2, 0, false).shown(), "a|");
        // Half of one takes it whole.
        assert_eq!(Typing::new("a😀").delete_around(1, 0, false).shown(), "a|");
        assert_eq!(Typing::new("a😀b😀").keys(&[Key::Left, Key::Left]).delete_around(1, 3, false).shown(), "a|");
        // In code points, an emoji is one.
        assert_eq!(Typing::new("😀😀").delete_around(1, 0, true).shown(), "😀|");
    }

    #[test]
    fn an_input_method_deletes_a_combining_mark_without_its_letter() {
        // "e" and a combining acute: one character on screen, two units and two code points.
        let accented = "xe\u{301}";
        assert_eq!(Typing::new(accented).delete_around(1, 0, false).shown(), "xe|");
        assert_eq!(Typing::new(accented).delete_around(2, 0, true).shown(), "x|");
        // More than there is deletes what there is.
        assert_eq!(Typing::new(accented).delete_around(9, 9, false).shown(), "|");
    }

    #[test]
    fn a_focused_box_flat_or_vertical_deletes_what_an_input_method_asks_in_its_run() {
        for vertical in [false, true] {
            let mut trial = crate::testing::Trial::new(String::from("a😀"), move |text: &mut String, ui| {
                let edit = if vertical { ui.vertical_text_box(text, "") } else { ui.text_box(text, "") };
                if ui.focus() == 0 {
                    ui.set_focus(edit.id);
                }
            });
            trial.run();
            trial.ui.input.strokes.push(Stroke::Key(Key::End, Mods::CTRL));
            trial.ui.input.strokes.push(Stroke::DeleteAround(DeleteAround { before: 2, after: 0, in_code_points: false }));
            trial.run();
            assert_eq!(trial.app, "a", "vertical: {vertical}");
        }
    }

    #[test]
    fn an_input_method_deletes_around_the_selection_which_stays_and_undoes_at_once() {
        let mut t = Typing::new("abcdef");
        t.keys(&[Key::Left, Key::Left]).key(Key::Left, Mods::SHIFT).key(Key::Left, Mods::SHIFT);
        assert_eq!(t.delete_around(1, 1, false).shown(), "a[cd]f");
        assert_eq!(t.1.cursor, 1, "the cursor stays at the selection's start, where it was");
        t.key(Key::Char('z'), Mods::CTRL);
        assert_eq!(t.shown(), "ab[cd]ef");
        assert_eq!(Typing::new("abc").delete_around(0, 0, false).1.changes.len(), 0, "nothing deleted, nothing to undo");
    }

    #[test]
    fn the_cursor_moves_by_character_word_and_line() {
        let mut t = Typing::new("one two\nthree");
        t.key(Key::Home, Mods::NONE);
        assert_eq!(t.shown(), "one two\n|three");
        t.key(Key::Left, Mods::CTRL);
        assert_eq!(t.shown(), "one |two\nthree");
        t.key(Key::Right, Mods::CTRL).key(Key::Right, Mods::CTRL);
        assert_eq!(t.shown(), "one two\nthree|");
        t.key(Key::Home, Mods::CTRL).key(Key::End, Mods::SHIFT);
        assert_eq!(t.shown(), "[one two]\nthree");
        t.key(Key::Delete, Mods::NONE).key(Key::Delete, Mods::CTRL);
        assert_eq!(t.shown(), "|");
    }

    #[test]
    fn up_and_down_in_one_line_go_to_its_start_and_its_end() {
        let mut t = Typing::new("one two");
        let places: Places = &mut |piece, paragraph, places| BuiltInFont.places(piece, Font::new(8.0), 1.0, paragraph, places);
        assert!(t.1.key(&mut t.0, Key::Up, Mods::NONE, false, places, &mut t.2));
        assert_eq!(t.shown(), "|one two");
        assert!(t.1.key(&mut t.0, Key::Down, Mods::SHIFT, false, places, &mut t.2));
        assert_eq!(t.shown(), "[one two]");
        assert!(t.1.key(&mut t.0, Key::Up, Mods::SHIFT, false, places, &mut t.2) && t.1.key(&mut t.0, Key::Down, Mods::NONE, false, places, &mut t.2));
        assert_eq!(t.shown(), "one two|");
    }

    #[test]
    fn up_and_down_keep_to_their_column() {
        let mut t = Typing::new("a long line\nab\nanother one");
        t.1.laid_out(&[(0, 11), (12, 14), (15, 26)].map(|(start, end)| Line { start, end, width: 0.0 }), 0, Point::default(), 10.0, 0.0);
        t.key(Key::Home, Mods::CTRL).keys(&[Key::Right, Key::Right, Key::Right, Key::Right, Key::Right, Key::Down]);
        assert_eq!(t.shown(), "a long line\nab|\nanother one", "a short line holds the cursor at its end");
        t.keys(&[Key::Down]);
        assert_eq!(t.shown(), "a long line\nab\nanoth|er one", "and the next line gets it back where it was");
        t.keys(&[Key::Down]);
        assert_eq!(t.shown(), "a long line\nab\nanother one|", "below the last line is the end");
        t.keys(&[Key::Up, Key::Up, Key::Up]);
        assert_eq!(t.shown(), "|a long line\nab\nanother one", "above the first line is the start");
    }

    #[test]
    fn a_character_made_of_several_is_stepped_over_whole() {
        let mut t = Typing::new("e\u{301}👍🏽x");
        t.keys(&[Key::Left, Key::Left]);
        assert_eq!(t.shown(), "e\u{301}|👍🏽x");
        t.keys(&[Key::Backspace]);
        assert_eq!(t.shown(), "|👍🏽x");
        t.keys(&[Key::Right]);
        assert_eq!(t.shown(), "👍🏽|x");
        // A flag, a family, a keycap and a CRLF: each stepped over whole, either way, never
        // entered.
        let (flag, family, key) = ("\u{1f1e9}\u{1f1ea}", "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}", "1\u{fe0f}\u{20e3}");
        let all = ["a", flag, flag, family, "\r\n", key, "b"];
        let mut t = Typing::new(&all.concat());
        for stood in (0..all.len()).rev() {
            t.keys(&[Key::Left]);
            assert_eq!(t.shown(), [all[..stood].concat(), "|".into(), all[stood..].concat()].concat());
        }
        for stood in 1..=all.len() {
            t.keys(&[Key::Right]);
            assert_eq!(t.shown(), [all[..stood].concat(), "|".into(), all[stood..].concat()].concat());
        }
        // Selected whole with Shift, and deleted whole from either side.
        let mut t = Typing::new(&["a", family, flag, "b"].concat());
        t.keys(&[Key::Left]).key(Key::Left, Mods::SHIFT).key(Key::Left, Mods::SHIFT);
        assert_eq!(t.shown(), ["a[", family, flag, "]b"].concat());
        t.keys(&[Key::Right, Key::Backspace]);
        assert_eq!(t.shown(), ["a", family, "|b"].concat());
        t.keys(&[Key::Home, Key::Right, Key::Delete]);
        assert_eq!(t.shown(), "a|b");
    }

    #[test]
    fn words_and_the_pointer_keep_to_whole_characters() {
        // A word with a combining accent is one word to Ctrl+arrow and double click; an emoji
        // between words is passed like a space, and a keycap's mark goes with its digit.
        let (hand, key) = ("\u{1f44d}\u{1f3fd}", "1\u{fe0f}\u{20e3}");
        let mut t = Typing::new(&["cafe\u{301} ", hand, " x", key].concat());
        t.key(Key::Left, Mods::CTRL);
        assert_eq!(t.shown(), ["cafe\u{301} ", hand, " |x", key].concat());
        t.key(Key::Left, Mods::CTRL);
        assert_eq!(t.shown(), ["|cafe\u{301} ", hand, " x", key].concat());
        t.key(Key::Right, Mods::CTRL);
        assert_eq!(t.shown(), ["cafe\u{301}| ", hand, " x", key].concat());
        t.key(Key::Right, Mods::CTRL);
        assert_eq!(t.shown(), ["cafe\u{301} ", hand, " x", key, "|"].concat());
        // Every `char` is 6 wide here, so the hand is two: 12 wide, from 36 to 48. A click on
        // its left half places the cursor before it, on its right half after.
        let places: Places = &mut |piece, paragraph, places| BuiltInFont.places(piece, Font::new(8.0), 1.0, paragraph, places);
        t.1.laid_out(&[Line { start: 0, end: t.0.len(), width: 0.0 }], 0, Point::default(), 10.0, 0.0);
        for (x, shown) in [(40.0, ["cafe\u{301} |", hand, " x", key].concat()), (44.0, ["cafe\u{301} ", hand, "| x", key].concat()), (500.0, ["cafe\u{301} ", hand, " x", key, "|"].concat())] {
            t.1.anchor_set = false;
            t.1.point(&t.0, Point { x, y: 4.0 }, 1, places);
            assert_eq!(t.shown(), shown, "pressed at {x}");
        }
        // A double click on the word selects all of it, accent included; on the hand, the hand.
        for (x, shown) in [(20.0, ["[cafe\u{301}] ", hand, " x", key].concat()), (40.0, ["cafe\u{301} [", hand, "] x", key].concat())] {
            t.1.anchor_set = false;
            t.1.point(&t.0, Point { x, y: 4.0 }, 2, places);
            assert_eq!(t.shown(), shown, "pressed twice at {x}");
        }
    }

    /// A bidirectional text box, written with the small Hebrew test font
    /// (`bidi::tests::hebrew`): alef 7 wide, gimel 10, space 4, at 16 units; Latin in the
    /// built-in font, 12 wide. Laid out as one line.
    #[cfg(feature = "right-to-left")]
    struct Both(String, Edit, LocalClipboard, crate::bidi::BidiText);
    #[cfg(feature = "right-to-left")]
    impl Both {
        fn new(text: &str) -> Self {
            let mut both = Both(text.into(), Edit::new(1, 0), LocalClipboard::default(), crate::bidi::tests::hebrew());
            let wide = both.3.measure(text, Font::new(16.0), 1.0, None);
            both.1.laid_out(&[Line { start: 0, end: text.len(), width: wide }], 0, Point::default(), 20.0, wide);
            both
        }
        fn key(&mut self, key: Key, mods: Mods) -> (usize, bool, f32) {
            let Both(text, edit, clipboard, writer) = self;
            let places: Places = &mut |piece, paragraph, places| writer.places(piece, Font::new(16.0), 1.0, paragraph, places);
            assert!(edit.key(text, key, mods, true, places, clipboard), "{key:?} was not used");
            self.at()
        }
        /// The cursor: its byte, whether it shows at the next character, and its x.
        fn at(&mut self) -> (usize, bool, f32) {
            let Both(text, edit, _, writer) = self;
            let places: Places = &mut |piece, paragraph, places| writer.places(piece, Font::new(16.0), 1.0, paragraph, places);
            let x = edit.caret_in(text, 0..text.len(), edit.cursor, places);
            (edit.cursor, edit.after, x)
        }
        fn press(&mut self, x: f32, clicks: u8) -> (usize, bool, f32) {
            let Both(text, edit, _, writer) = self;
            let places: Places = &mut |piece, paragraph, places| writer.places(piece, Font::new(16.0), 1.0, paragraph, places);
            edit.anchor_set = false;
            edit.point(text, Point { x, y: 4.0 }, clicks, places);
            self.at()
        }
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn the_arrows_go_by_what_is_seen_in_text_that_runs_both_ways() {
        // "xy", a space, then alef and gimel, in an LTR paragraph: shown as x, y, space, then
        // gimel and alef right to left. Right moves across one character at a time, whichever
        // way each runs: before the gimel it is after the space logically (byte 3), shown at
        // the space's edge; past the gimel, before it logically (byte 5), at the gimel's right
        // edge; past the alef, before it (byte 3 again), at the line's end. There it stays.
        let mut t = Both::new("xy \u{5d0}\u{5d2}");
        assert_eq!(t.key(Key::Home, Mods::NONE), (0, true, 0.0));
        let right: Vec<_> = (0..6).map(|_| t.key(Key::Right, Mods::NONE)).collect();
        assert_eq!(right, [(1, false, 12.0), (2, false, 24.0), (3, false, 28.0), (5, true, 38.0), (3, true, 45.0), (3, true, 45.0)]);
        // And back with Left, the same positions reversed.
        let left: Vec<_> = (0..5).map(|_| t.key(Key::Left, Mods::NONE)).collect();
        assert_eq!(left, [(5, false, 38.0), (7, false, 28.0), (2, true, 24.0), (1, true, 12.0), (0, true, 0.0)]);
        // Home and End are logical: the end is after the gimel, shown next to the space, where
        // the next Hebrew letter goes.
        assert_eq!(t.key(Key::End, Mods::NONE), (7, false, 28.0));
        // The pointer places the cursor at the nearest edge, and records which side. (Where
        // space and gimel meet: after the space, logically.)
        assert_eq!([t.press(44.0, 1), t.press(40.0, 1), t.press(29.0, 1), t.press(3.0, 1)], [(3, true, 45.0), (5, false, 38.0), (3, false, 28.0), (0, true, 0.0)]);
        // After typing, the cursor is after the typed letter, at its far edge: a gimel typed at
        // the start makes the paragraph RTL, right-aligned; the cursor is at the gimel's left.
        t.1.insert(&mut t.0, "\u{5d2}", false, true);
        let at = t.at();
        assert_eq!((t.0.as_str(), at), ("\u{5d2}xy \u{5d0}\u{5d2}", (2, false, 45.0)));
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn in_a_paragraph_that_runs_from_right_to_left_its_start_is_at_the_right() {
        // Alef and gimel, a space and "xy": shown as x, y, space, gimel, alef. Home goes to the
        // start, at the right; Left moves forward through the text, as shown.
        let mut t = Both::new("\u{5d0}\u{5d2} xy");
        assert_eq!(t.key(Key::Home, Mods::NONE), (0, true, 45.0));
        let left: Vec<_> = (0..5).map(|_| t.key(Key::Left, Mods::NONE)).collect();
        assert_eq!(left, [(2, false, 38.0), (4, false, 28.0), (5, false, 24.0), (6, true, 12.0), (5, true, 0.0)]);
        // Ctrl+arrow goes by words, Left forward through the text and Right back.
        assert_eq!(t.key(Key::Home, Mods::NONE).0, 0);
        assert_eq!(t.key(Key::Left, Mods::CTRL).0, 4);
        assert_eq!(t.key(Key::Left, Mods::CTRL).0, 7);
        assert_eq!(t.key(Key::Right, Mods::CTRL).0, 5);
        // With a selection, Left and Right go to its end and start.
        t.1.anchor = 0;
        assert_eq!(
            (t.key(Key::Left, Mods::NONE).0, {
                t.1.anchor = 0;
                t.key(Key::Right, Mods::NONE).0
            }),
            (5, 0)
        );
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_selection_across_both_directions_is_seen_in_pieces() {
        // From after "x" to between alef and gimel: "y", the space and the alef. The first two
        // show together at the left, the alef alone at the right.
        let mut writer = crate::bidi::tests::hebrew();
        let text = "xy \u{5d0}\u{5d2}";
        let mut marked = Vec::new();
        let at = crate::text::marks(&mut writer, text, 0, 45.0, Font::new(16.0), 1.0, Paragraph::whole(false), [1, 5], Some(5), true, &mut marked);
        assert_eq!((at, marked.clone()), (Some([38.0; 2]), vec![[12.0, 28.0], [38.0, 45.0]]));
        // A selection continuing past the line covers the line end's room: at its right in an
        // LTR paragraph, at its left in an RTL one.
        let at = crate::text::marks(&mut writer, text, 0, 45.0, Font::new(16.0), 1.0, Paragraph::whole(false), [3, 9], None, false, &mut marked);
        assert_eq!((at, marked.clone()), (None, vec![[28.0, 49.0]]));
        crate::text::marks(&mut writer, "\u{5d0}\u{5d2}", 0, 17.0, Font::new(16.0), 1.0, Paragraph::whole(true), [0, 9], None, false, &mut marked);
        assert_eq!(marked, [[-4.0, 17.0]]);
        // Text with nothing RTL behaves as before.
        crate::text::marks(&mut writer, "abc", 0, 36.0, Font::new(16.0), 1.0, Paragraph::whole(false), [1, 2], None, false, &mut marked);
        assert_eq!(marked, [[12.0, 24.0]]);
        // Between the space and the alef directions meet: the cursor shows at the alef (its
        // right) and at the space (its right), the one it stands by first.
        let mut at = |after: bool| crate::text::marks(&mut writer, text, 0, 45.0, Font::new(16.0), 1.0, Paragraph::whole(false), [3, 3], Some(3), after, &mut Vec::new());
        assert_eq!([at(true), at(false)], [Some([45.0, 28.0]), Some([28.0, 45.0])]);
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn where_directions_meet_the_cursor_is_split_in_two_halves() {
        use crate::surface::Surface;
        use crate::{Event, Key};
        let mut ui = crate::ui::Ui::new(Box::new(crate::bidi::tests::hebrew()), Box::new(LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 160.0, h: 50.0 }, 1.0);
        let (mut text, mut first) = ("xy \u{5d0}\u{5d2}".to_string(), true);
        let mut build = |ui: &mut crate::ui::Ui| {
            ui.column().pad(6.0).open();
            let typed = ui.text_box(&mut text, "");
            if std::mem::take(&mut first) {
                ui.set_focus(typed.id);
            }
            ui.close();
        };
        // After every step only what changed is drawn again, and that is what a full draw gives.
        let mut screen = Surface::draw(&mut ui, 64);
        let mut step = |ui: &mut crate::ui::Ui, key: Option<Key>| {
            key.into_iter().for_each(|key| _ = ui.event(Event::Key(key, Mods::NONE)));
            let changed = ui.run(&mut build);
            screen.redraw(ui, changed);
            let fresh = Surface::draw(ui, 64);
            assert!(screen.pixels == fresh.pixels, "drawing only what changed ({changed:?}) left the window different");
            fresh
        };
        step(&mut ui, None);
        // At the start, one cursor; three to the right, where the space meets the alef, two.
        let start = step(&mut ui, Some(Key::Home));
        let split = [Key::Right, Key::Right, Key::Right].map(|key| step(&mut ui, Some(key))).into_iter().last().unwrap();
        let mut columns = std::collections::BTreeMap::<usize, [usize; 2]>::new();
        for (at, _) in start.pixels.iter().zip(&split.pixels).enumerate().filter(|(_, (one, other))| one != other) {
            let (x, y) = (at % start.width, at / start.width);
            let rows = columns.entry(x).or_insert([y, y]);
            *rows = [rows[0].min(y), rows[1].max(y)];
        }
        // The first column is the cursor at the start; then one half at each place, the
        // Hebrew's width apart, one under the other.
        let halves: Vec<(usize, [usize; 2])> = columns.into_iter().skip(1).collect();
        assert_eq!(halves.len(), 2, "{halves:?}");
        let ((left, low), (right, high)) = (halves[0], halves[1]);
        assert_eq!(right - left, 14, "the Hebrew between, in the box's 14 units");
        assert!(low[1] < high[0] || high[1] < low[0], "{halves:?}");
        assert_eq!(low[1].max(high[1]) - low[0].min(high[0]), low[1] - low[0] + high[1] - high[0] + 1);
    }

    #[test]
    fn typing_is_undone_a_word_at_a_time_and_can_be_done_again() {
        let mut t = Typing::new("");
        for letter in "fix the arc".chars() {
            t.typed(&letter.to_string());
        }
        t.key(Key::Backspace, Mods::CTRL);
        assert_eq!(t.shown(), "fix the |");
        t.key(Key::Char('z'), Mods::CTRL);
        assert_eq!(t.shown(), "fix the arc|");
        t.key(Key::Char('z'), Mods::CTRL).key(Key::Char('z'), Mods::CTRL);
        assert_eq!(t.shown(), "fix |");
        t.key(Key::Char('y'), Mods::CTRL);
        assert_eq!(t.shown(), "fix the |");
        t.typed("jump");
        t.key(Key::Char('y'), Mods::CTRL);
        assert_eq!(t.shown(), "fix the jump|", "after a new change there is nothing to do again");
        // Text the app replaced is not undone into.
        t.0 = "something else".into();
        t.1.fit(&t.0.clone());
        t.key(Key::Char('z'), Mods::CTRL);
        assert_eq!(t.0, "something else");
    }

    #[test]
    fn cut_copy_and_paste_go_through_the_clipboard() {
        let mut t = Typing::new("one two");
        t.key(Key::Left, Mods { ctrl: true, shift: true, alt: false }).key(Key::Char('x'), Mods::CTRL);
        assert_eq!((t.shown().as_str(), t.2.text.as_str()), ("one |", "two"));
        t.key(Key::Char('v'), Mods::CTRL).key(Key::Char('v'), Mods::CTRL);
        assert_eq!(t.shown(), "one twotwo|");
    }

    #[test]
    fn the_pointer_puts_the_cursor_and_selects() {
        let mut t = Typing::new("fix the arc");
        let places: Places = &mut |piece, paragraph, places| BuiltInFont.places(piece, Font::new(8.0), 1.0, paragraph, places);
        t.1.laid_out(&[Line { start: 0, end: 11, width: 66.0 }], 0, Point { x: 10.0, y: 20.0 }, 10.0, 0.0);
        // Letters are 6 wide, so 37 units in is nearest the gap after the sixth letter.
        t.1.anchor_set = false;
        t.1.point(&t.0, Point { x: 47.0, y: 24.0 }, 1, places);
        assert_eq!(t.shown(), "fix th|e arc");
        t.1.point(&t.0, Point { x: 500.0, y: 24.0 }, 1, places);
        assert_eq!(t.shown(), "fix th[e arc]", "dragging selects from where the press was");
        t.1.anchor_set = false;
        t.1.point(&t.0, Point { x: 47.0, y: 24.0 }, 2, places);
        assert_eq!(t.shown(), "fix [the] arc", "a double press takes the word");
    }

    #[test]
    fn text_that_is_hidden_gives_nothing_away() {
        let mut t = Typing::new("open sesame");
        t.1.secret = true;
        // No words: Ctrl+arrow goes all the way, a double click selects everything.
        t.key(Key::Left, Mods::CTRL);
        assert_eq!(t.shown(), "|open sesame");
        let places: Places = &mut |piece, paragraph, places| BuiltInFont.places(piece, Font::new(8.0), 1.0, paragraph, places);
        // It was laid out as its dots, three bytes each.
        t.1.laid_out(&[Line { start: 0, end: 33, width: 66.0 }], 0, Point::default(), 10.0, 0.0);
        t.1.anchor_set = false;
        t.1.point(&t.0, Point { x: 31.0, y: 4.0 }, 2, places);
        assert_eq!(t.shown(), "[open sesame]");
        // Nothing reaches the clipboard, by copy or cut.
        t.key(Key::Char('c'), Mods::CTRL).key(Key::Char('x'), Mods::CTRL);
        assert_eq!((t.shown().as_str(), t.2.text.as_str()), ("[open sesame]", ""));
        // But pasting works, and a single click places the cursor.
        t.2.text = "!".into();
        t.key(Key::End, Mods::NONE).key(Key::Char('v'), Mods::CTRL);
        t.1.anchor_set = false;
        t.1.point(&t.0, Point { x: 31.0, y: 4.0 }, 1, places);
        assert_eq!(t.shown(), "open |sesame!");
    }
}
