//! Vertical text boxes: text typed top to bottom, in columns right to left, as `vertical_layout`
//! lays it out.
//!
//! The cursor, selection, undo and clipboard are a text box's (`text_edit::Edit`); only where
//! they are differs. Up and Down move along the column (back and forth through the text), Left
//! and Right across columns (Left to the next, which is to the left), keeping the place along
//! the column; Home and End go to the column's ends, Ctrl with Up and Down by words, Ctrl with
//! Home and End to the text's ends; Shift selects. A press places the cursor at the nearest
//! place between characters, a drag selects, two presses select a word, three everything.
//!
//! The cursor is a bar across the column. An input method composes where it is: the systems are
//! told its place (`Ui::ime_area`) after layout, as for any text box. Screen readers hear a text
//! box with its text, cursor and selection, read it by column, and find each character where it
//! is drawn. (A box's runs of digits are not set across one em, and it is not spell checked.)
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::Id;
use crate::layout::{Flow, Length};
use crate::rich_text::Span;
use crate::text_edit::step;
use crate::ui::{Measure, Response, Stroke, Ui};
use crate::vertical_layout::{Laid, middles};
use crate::vertical_paragraph::{Kept, Shown};

/// The vertical text box being edited: its id; the cursor's place in the box holding its text
/// (a bar across its column); and the place along a column the cursor keeps while moving across
/// columns.
#[derive(Default)]
pub(crate) struct Edited {
    pub id: Id,
    pub caret: Option<Rect>,
    pub along: Option<f32>,
}

/// A vertical text box's kind: one column, or columns between so many and so many before it
/// scrolls.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Column,
    Columns(usize, usize),
}

impl Ui {
    /// A one-column vertical text box, top to bottom, as Japanese and Chinese are written: it
    /// fills the height it has and scrolls along its column. `hint` shows while it is empty.
    /// The keys follow the column: Up and Down move along it. An input method composes at its
    /// cursor.
    pub fn vertical_text_box(&mut self, text: &mut String, hint: &str) -> Response {
        self.vertical_edit_box(text, hint, Kind::Column)
    }

    /// A vertical text box of several columns, top to bottom and right to left, breaking where
    /// its height ends as CJK line breaking allows: `columns.0` columns wide, growing to
    /// `columns.1`, then scrolling sideways. Up and Down move along a column, Left and Right
    /// across columns.
    pub fn vertical_text_area(&mut self, text: &mut String, hint: &str, columns: (usize, usize)) -> Response {
        self.vertical_edit_box(text, hint, Kind::Columns(columns.0, columns.1))
    }

    /// A vertical text box of `kind`.
    fn vertical_edit_box(&mut self, text: &mut String, hint: &str, kind: Kind) -> Response {
        #[cfg(target_os = "android")]
        crate::android_input_method::hand_over();
        let t = self.theme;
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let (mut state, enter_left) = (self.interact(id), std::mem::take(&mut self.asks.enter_left));
        let many = matches!(kind, Kind::Columns(..));
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, self.size().h, 0.0]);
        let room = if many { read[1].max(1.0) } else { f32::INFINITY };
        let mut kept = self.take_vertical();
        let mut shown = kept.shown.remove(&id).unwrap_or_default();
        self.lay_box(&mut shown, text, room);
        if state.hovered {
            self.set_cursor(Cursor::Text);
        }
        if state.focused {
            state.changed = self.type_vertical(text, &state, many, enter_left, &mut shown, &mut kept.edit);
        }
        let empty = text.is_empty();
        if empty {
            self.lay_vertical(&mut shown, &[Span::text(hint).color(t.text_faint)], room, 0);
        }
        let line = shown.laid.lines.first().copied().unwrap_or(t.font.size);
        let (wide, long) = (shown.laid.wide.ceil().max(line), shown.laid.long.ceil().max(t.font.size));
        let edge = if state.focused {
            t.accent
        } else if state.hovered {
            t.text_faint
        } else {
            t.line_strong
        };
        let frame = self.el().flow(Flow::Row).h(Length::Grow(1.0)).fill(t.panel).border(edge, 1.0).radius(t.radius).pad((6.0, 8.0, 6.0, 8.0)).sense().focusable();
        let frame = match kind {
            Kind::Column => frame.w(Length::Px(line + 12.0)).scroll(),
            Kind::Columns(least, most) => frame.min(least as f32 * line + 12.0, 0.0).max(most as f32 * line + 12.0, f32::INFINITY).scroll_x(),
        };
        frame.open();
        if many {
            self.measures.push(Measure { at, within: at, id, read });
            if !self.scrolls.contains_key(&id) {
                self.scroll_to(&state, [f32::INFINITY, f32::NAN]);
            }
        }
        self.tell_text(&state, hint, text);
        let letters = |at: usize| text[..at].chars().count() as u32;
        if state.focused {
            self.tell_caret([letters(self.edit.anchor), letters(self.edit.cursor)]);
        }
        let content = self.tree.specs.len() as u32;
        self.el().flow(Flow::Stack).size(wide, long).open();
        if state.focused && !empty {
            self.write_vertical_selection(&shown.laid, [self.edit.anchor, self.edit.cursor]);
        }
        let ink = if empty { t.text_faint } else { t.text };
        let mut path = crate::path::Path::new();
        shown.paths.iter().for_each(|outline| _ = path.add(outline));
        self.el().size(wide, long).shape(&path, ink).show();
        let middles = middles(&shown.laid.columns, shown.laid.wide.ceil());
        let caret = if empty { rect(wide / 2.0 - line / 2.0, 0.0, line, 1.0) } else { caret_place(&shown.laid, &middles, self.edit.cursor) };
        if state.focused && self.window_focused && self.blink(self.edit.moved) {
            self.el().offset(caret.x, caret.y.round()).size(caret.w, 1.0).fill(t.text).show();
        }
        self.close();
        self.close();
        if state.focused {
            kept.edit = Edited { id, caret: Some(caret), along: kept.edit.along.filter(|_| kept.edit.id == id) };
        }
        shown.content = content;
        if let Some(letters) = shown.letters.clone().filter(|_| !empty) {
            kept.told.push((at, content, letters));
        }
        shown.used = true;
        kept.shown.insert(id, shown);
        self.keep_vertical(kept);
        self.edit_menu(&state, false);
        state
    }

    /// Lays a box's text out afresh where it, the theme or the room changed.
    fn lay_box(&mut self, shown: &mut Shown, text: &str, room: f32) {
        let color = self.theme.text;
        self.lay_vertical(shown, &[Span::text(text).color(color)], room, 0);
    }

    /// The selection, behind the text: in each column, from where it starts to where it ends
    /// there; past a column's end where it goes on.
    fn write_vertical_selection(&mut self, laid: &Laid, [anchor, cursor]: [usize; 2]) {
        let [from, to] = [anchor.min(cursor), anchor.max(cursor)];
        if from == to {
            return;
        }
        let color = self.theme.accent.with_alpha(0.35);
        let middles = middles(&laid.columns, laid.wide.ceil());
        for (column, x) in laid.columns.iter().zip(&middles) {
            let Some([top, bottom]) = selected_in(laid, column, [from, to]) else { continue };
            self.el().offset(x - column.wide / 2.0, top).size(column.wide, bottom - top).fill(color).show();
        }
    }

    /// Feeds pointer, keys and typed text to the focused vertical box (laid out in `shown`),
    /// Enter left to the app or not. Returns whether the text changed.
    fn type_vertical(&mut self, text: &mut String, state: &Response, many: bool, enter_left: bool, shown: &mut Shown, edited: &mut Edited) -> bool {
        let pressed = (state.hovered && self.input.pressed[0]).then_some(self.input.mods.shift);
        let was = self.edit.start(state.id, text, enter_left, std::mem::take(&mut self.select_all), pressed);
        if let (true, Some(at)) = (state.pressed, self.input.pointer) {
            self.point_vertical(text, at, shown);
        }
        let strokes = std::mem::take(&mut self.input.strokes);
        for stroke in strokes {
            let used = self.stroke_vertical(text, &stroke, many, shown, edited);
            if !used {
                self.input.strokes.push(stroke);
            }
        }
        #[cfg(target_os = "ios")]
        {
            self.edit.empty = text.is_empty();
            crate::ios_text_input::hand_text_boxes();
        }
        let changed = self.edit.done(was, self.now);
        self.again |= changed;
        if changed {
            let room = shown.room;
            self.lay_box(shown, text, room);
        }
        changed
    }

    /// One key or typed text, for the focused vertical box; whether it was used.
    fn stroke_vertical(&mut self, text: &mut String, stroke: &Stroke, many: bool, shown: &mut Shown, edited: &mut Edited) -> bool {
        match stroke {
            Stroke::Text(typed) => self.edit.insert(text, typed, many, true),
            #[cfg(any(target_os = "android", test))]
            Stroke::DeleteAround(around) => self.edit.delete_around(text, *around),
            #[cfg(target_os = "ios")]
            Stroke::Replace(done) => self.edit.keyboard(text, done, many),
            Stroke::Key(_, mods) if mods.alt => return false,
            Stroke::Key(key, mods) => return self.key_vertical(text, (*key, *mods), many, shown, edited),
        }
        true
    }

    /// A key for the focused vertical box: one that edits as in any text box, or moves the
    /// cursor along or across columns (on the text as laid out now). Whether it was used.
    fn key_vertical(&mut self, text: &mut String, (key, mods): (Key, Mods), many: bool, shown: &mut Shown, edited: &mut Edited) -> bool {
        if self.edit.change(text, key, mods, many, self.clipboard.as_mut()) {
            edited.along = None;
            return true;
        }
        if shown.laid.text != *text {
            let room = shown.room;
            self.lay_box(shown, text, room);
        }
        let Some(to) = moved(&shown.laid, text, self.edit.cursor, key, mods, &mut edited.along) else { return false };
        self.edit.cursor = to;
        if !mods.shift {
            self.edit.anchor = to;
        }
        true
    }

    /// The pointer is down on the focused vertical box at `at` (in the window): a single press
    /// places the cursor (dragging selects from there), two select the word, three everything.
    fn point_vertical(&mut self, text: &str, at: Point, shown: &Shown) {
        let Some(corner) = shown.corner else { return };
        let middles = middles(&shown.laid.columns, shown.laid.wide.ceil());
        let to = offset_at(&shown.laid, &middles, pt(at.x - corner.x, at.y - corner.y)).min(text.len());
        self.edit.press_at(text, to, self.input.clicks);
    }
}

/// After layout: the focused vertical box's cursor in the window, for input methods; scrolled
/// into view where it just moved.
pub(crate) fn place_cursor(ui: &mut Ui, kept: &mut Kept) {
    let edited = &kept.edit;
    if ui.focus() != edited.id || ui.edit.id != edited.id || edited.id == 0 {
        return;
    }
    let (Some(caret), Some(shown)) = (edited.caret, kept.shown.get(&edited.id)) else { return };
    let Some(corner) = shown.corner else { return };
    let spot = caret.moved(corner.x, corner.y);
    ui.ime_area = Some(spot);
    if std::mem::take(&mut ui.edit.reveal) {
        ui.show(shown.content, spot.grow(2.0));
    }
}

/// Where a cursor before byte `at` shows: a bar across its column, at the top of the step it
/// is before; at the end of the text, after the last step. (At a column's break, the next
/// column's top.)
pub(crate) fn caret_place(laid: &Laid, middles: &[f32], at: usize) -> Rect {
    let mut last = None;
    for (column, x) in laid.columns.iter().zip(middles) {
        let left = x - column.wide / 2.0;
        for (n, step) in (column.steps[0]..column.steps[1]).enumerate() {
            let [top, bottom] = span_of(laid, column, n);
            let bytes = laid.steps[step as usize].bytes;
            if bytes[0] as usize == at {
                return rect(left, top, column.wide, 1.0);
            }
            if bytes[1] as usize == at {
                last = Some(rect(left, bottom, column.wide, 1.0));
            }
        }
        if column.steps[0] == column.steps[1] {
            return rect(left, 0.0, column.wide, 1.0);
        }
    }
    last.unwrap_or_else(|| rect(middles.first().copied().unwrap_or(0.0) - laid.lines.first().copied().unwrap_or(0.0) / 2.0, 0.0, laid.lines.first().copied().unwrap_or(0.0), 1.0))
}

/// Where step `n` of a column starts and ends along it, without what is cut off it.
fn span_of(laid: &Laid, column: &crate::vertical_layout::Column, n: usize) -> [f32; 2] {
    let at = column.steps[0] as usize + n;
    let step = laid.steps[at];
    let [start, end] = laid.cuts(at, column.steps[0] as usize, column.steps[1] as usize);
    [column.tops[n] + start, column.tops[n] + step.long - end]
}

/// The part of a column a selection of bytes `from` to `to` covers: its top and bottom, past the
/// column's end (by 4) where the selection goes on.
fn selected_in(laid: &Laid, column: &crate::vertical_layout::Column, [from, to]: [usize; 2]) -> Option<[f32; 2]> {
    let steps = (column.steps[0]..column.steps[1]).enumerate().filter(|(_, at)| {
        let bytes = laid.steps[*at as usize].bytes;
        bytes[0] as usize >= from && bytes[1] as usize <= to
    });
    let spans: Vec<[f32; 2]> = steps.map(|(n, _)| span_of(laid, column, n)).collect();
    let (first, last) = (spans.first()?, spans.last()?);
    let end = laid.steps.get(column.steps[1] as usize).map_or(laid.text.len(), |next| next.bytes[0] as usize);
    Some([first[0], if to > end { last[1] + 4.0 } else { last[1] }])
}

/// The place between characters nearest a point (in units from the text's top left): in the
/// column whose middle is nearest it, at the nearest step's edge along it.
pub(crate) fn offset_at(laid: &Laid, middles: &[f32], at: Point) -> usize {
    let nearest = middles.iter().enumerate().min_by(|(_, one), (_, other)| (*one - at.x).abs().total_cmp(&(*other - at.x).abs())).map(|(n, _)| n);
    let Some(n) = nearest else { return 0 };
    along_column(laid, &laid.columns[n], at.y)
}

/// The place between characters in a column nearest `y` along it. (Never after a newline or
/// spaces ending it: that is the next column's start.)
fn along_column(laid: &Laid, column: &crate::vertical_layout::Column, y: f32) -> usize {
    let start = laid.steps.get(column.steps[0] as usize).map_or(laid.text.len(), |step| step.bytes[0] as usize);
    let mut best = (start, f32::MAX);
    for n in 0..(column.steps[1] - column.steps[0]) as usize {
        let step = laid.steps[column.steps[0] as usize + n];
        let [top, bottom] = span_of(laid, column, n);
        let shown = !laid.text[step.bytes[0] as usize..].starts_with(['\n', ' ']) || column.steps[1] as usize == laid.steps.len();
        for (edge, byte) in [(top, step.bytes[0]), (bottom, step.bytes[1])] {
            if (edge - y).abs() < best.1 && (byte == step.bytes[0] || shown) {
                best = (byte as usize, (edge - y).abs());
            }
        }
    }
    best.0
}

/// Where a movement key takes a vertical box's cursor from `at`: `None` for other keys. `along`
/// keeps the place along the column while Left and Right go across columns.
pub(crate) fn moved(laid: &Laid, text: &str, at: usize, key: Key, mods: Mods, along: &mut Option<f32>) -> Option<usize> {
    let kept = along.take();
    Some(match key {
        Key::Up | Key::Down if mods.ctrl => crate::text_edit::word(text, at, key == Key::Down),
        Key::Up | Key::Down => step(text, at, key == Key::Down),
        Key::Home if mods.ctrl => 0,
        Key::End if mods.ctrl => text.len(),
        Key::Home | Key::End => column_end(laid, text, at, key == Key::End),
        Key::Left | Key::Right => across(laid, at, key == Key::Left, kept, along).min(text.len()),
        _ => return None,
    })
}

/// The column the cursor at byte `at` shows in, by index.
fn column_of(laid: &Laid, at: usize) -> usize {
    let starts = |column: &crate::vertical_layout::Column| laid.steps.get(column.steps[0] as usize).map_or(usize::MAX, |step| step.bytes[0] as usize);
    laid.columns.iter().rposition(|column| starts(column) <= at).unwrap_or(0)
}

/// The start or end of the column the cursor at `at` is in (its end before a newline or spaces
/// ending it).
fn column_end(laid: &Laid, text: &str, at: usize, end: bool) -> usize {
    let Some(column) = laid.columns.get(column_of(laid, at)) else { return at };
    let first = laid.steps.get(column.steps[0] as usize).map_or(text.len(), |step| step.bytes[0] as usize);
    if !end {
        return first;
    }
    match laid.steps.get(column.steps[1] as usize) {
        Some(next) => first + text[first..next.bytes[0] as usize].trim_end_matches(['\n', ' ']).len(),
        None => text.len(),
    }
}

/// Where Left (`next`, to the next column) or Right takes the cursor at `at`: the nearest place
/// in that column at the same place along it (as kept from the last such move, else the
/// cursor's own). Past the last column, the text's end; before the first, its start.
fn across(laid: &Laid, at: usize, next: bool, kept: Option<f32>, along: &mut Option<f32>) -> usize {
    let middles = middles(&laid.columns, laid.wide.ceil());
    let y = kept.unwrap_or_else(|| caret_place(laid, &middles, at).y);
    *along = Some(y);
    let column = column_of(laid, at);
    match (next, column.checked_sub(1)) {
        (true, _) if column + 1 >= laid.columns.len() => laid.text.len(),
        (true, _) => along_column(laid, &laid.columns[column + 1], y),
        (false, None) => 0,
        (false, Some(before)) => along_column(laid, &laid.columns[before], y),
    }
}

#[cfg(test)]
mod tests {
    use crate::accessibility::Role;
    use crate::font_file::AppFonts;
    use crate::geom::pt;
    use crate::layout::Length;
    use crate::testing::Trial;
    use crate::ui::Ui;
    use crate::vertical_text::tests::writing_down;

    /// A vertical text area named "Note" in a row 100 high (columns of four "あ" at the theme's
    /// 15 units), written in the vertical test font.
    fn trial(text: &str) -> Trial<String> {
        let mut trial = Trial::new(text.to_string(), |text: &mut String, ui: &mut Ui| {
            ui.row().pad(10.0).h(Length::Px(100.0)).open();
            ui.vertical_text_area(text, "Note", (1, 4));
            ui.close();
        });
        trial.ui.set_fonts(AppFonts::new(writing_down(true, false)));
        trial.ui.theme.font.size = 15.0;
        trial.run();
        trial.run();
        trial
    }

    /// The box's cursor and anchor, in bytes.
    fn cursor(trial: &Trial<String>) -> [usize; 2] {
        let id = trial.find(Role::TextBox, "Note").id;
        let state = trial.ui.text_edit_state(id).expect("the box is edited");
        [state.selection[1], state.selection[0]]
    }

    #[test]
    fn up_and_down_go_along_the_column_and_left_and_right_across() {
        let mut trial = trial("ああああ\nああ");
        trial.press(Role::TextBox, "Note");
        trial.key("Ctrl+Home");
        trial.key("Down");
        assert_eq!(cursor(&trial), [3, 3]);
        // Left goes to the next column (on the left), as far down as it was.
        trial.key("Left");
        assert_eq!(cursor(&trial), [16, 16]);
        trial.key("Right");
        assert_eq!(cursor(&trial), [3, 3]);
        // End goes to the column's end (before its newline); Home to its start.
        trial.key("End");
        assert_eq!(cursor(&trial), [12, 12]);
        trial.key("Home");
        assert_eq!(cursor(&trial), [0, 0]);
        // Shift selects; typing replaces what is selected.
        trial.key("Shift+Down");
        trial.key("Shift+Down");
        assert_eq!(cursor(&trial), [6, 0]);
        trial.type_text("A");
        assert_eq!(trial.app, "Aああ\nああ");
        // Left past the last column goes to the end; Right before the first to the start.
        trial.key("Left");
        trial.key("Left");
        assert_eq!(cursor(&trial), [trial.app.len(); 2]);
    }

    #[test]
    fn a_press_puts_the_cursor_between_the_characters_nearest_it() {
        let mut trial = trial("ああああ\nああ");
        let node = trial.find(Role::TextBox, "Note").clone();
        let letters = node.letters.clone().expect("its letters are told");
        assert_eq!(letters.lines, [0, 5]);
        // The second column's second "あ" lies where readers are told; a press just below its
        // middle puts the cursor after it.
        let second = node.letter_rect(6);
        assert!(second.x < node.letter_rect(0).x && second.y > node.letter_rect(5).y, "{second:?}");
        trial.click(pt(second.x + second.w / 2.0, second.y + second.h * 0.75));
        assert_eq!(cursor(&trial), [19, 19]);
        // Two presses select the word there.
        trial.double_click(pt(node.letter_rect(1).x + 2.0, node.letter_rect(1).y + 2.0));
        assert_eq!(cursor(&trial), [12, 0]);
    }

    #[test]
    fn an_input_method_composes_at_the_cursor_across_its_column() {
        let mut trial = trial("ああ");
        trial.press(Role::TextBox, "Note");
        trial.key("Ctrl+Home");
        trial.key("Down");
        let (spot, first) = (trial.ui.ime_area().expect("the cursor's place"), trial.find(Role::TextBox, "Note").letter_rect(0));
        // A bar as wide as the column, where the first "あ" ends.
        assert_eq!((spot.x, spot.w, spot.h), (first.x, first.w, 1.0));
        assert!((spot.y - first.bottom()).abs() < 0.01, "{spot:?} {first:?}");
        // Screen readers hear where it is, in characters.
        assert_eq!(trial.find(Role::TextBox, "Note").caret, Some([1, 1]));
    }

    #[test]
    fn what_is_typed_wraps_into_new_columns_and_undoes() {
        let mut trial = trial("");
        trial.press(Role::TextBox, "Note");
        trial.type_text("ああああああああ");
        assert_eq!(trial.find(Role::TextBox, "Note").letters.clone().expect("letters").lines, [0, 4]);
        trial.key("Ctrl+Z");
        assert_eq!(trial.app, "");
        trial.key("Ctrl+Y");
        trial.key("Ctrl+A");
        trial.key("Ctrl+X");
        assert_eq!(trial.app, "");
        trial.key("Ctrl+V");
        trial.key("Ctrl+V");
        assert_eq!(trial.app, "ああああああああ".repeat(2));
    }
}
