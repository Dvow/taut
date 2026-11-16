//! Tool bars: icon buttons in a row (or a column), in groups with a line between them. What
//! does not fit goes into a menu behind a button at the bar's end; a tool's hint shows its keys
//! when the pointer rests on it.
//!
//! ```ignore
//! ui.tool_bar();
//! if ui.tool(OPEN, "Open\tCtrl+O").clicked { open(); }
//! ui.tool(SAVE, "Save\tCtrl+S");
//! ui.tool_group();
//! ui.tool_toggle(&mut app.bold, BOLD, "Bold\tCtrl+B");
//! ui.close();
//! ```
//!
//! A bar knows its room from the last layout and its tools' needs from the last time they were
//! described, so each tool knows whether it fits. One that does not becomes a row of the end
//! menu, which is described after the rest of the window; a row picked there goes to its tool
//! when next described, at once.
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::id::Id;
use crate::layout::{Align, Length};
use crate::menus::{Lead, Row};
use crate::told::Role;
use crate::ui::{Icon, Measure, Response, Ui};

/// The room a group separator takes along the bar, and the gap between tools.
const LINE: f32 = 9.0;
const GAP: f32 = 2.0;
/// The icon of the button that opens what did not fit.
const MORE: Icon = Icon::solid("M3.5 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4zM8 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4zM12.5 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4z");

/// A tool bar being described: its room and its tools' needs.
pub(crate) struct Bar {
    id: Id,
    /// Its tools go one under the other.
    down: bool,
    /// Its room along its tools as of the last layout; what all its tools needed last time; and
    /// in this pass, what the shown tools take and what all need.
    room: f32,
    need: f32,
    taken: f32,
    all: f32,
    /// How many tools it had in this pass; whether one did not fit (so neither do the ones
    /// after); whether a line goes before the next tool; and whether it was described in this
    /// pass.
    count: u32,
    out: bool,
    line: bool,
    seen: bool,
    /// The button that opens the menu of what did not fit.
    more: Id,
}

/// A tool that did not fit, as the bar's end menu shows it.
pub(crate) struct Left {
    bar: Id,
    tool: Id,
    icon: Icon,
    hint: String,
    on: Option<bool>,
    /// A line goes before it: it starts a group.
    line: bool,
}

/// Tool bars.
impl Ui {
    /// Starts a tool bar: a row taking the room across, in which tools are described (`tool`,
    /// [`tool_toggle`](Self::tool_toggle), [`tool_group`](Self::tool_group)); then
    /// [`close`](Self::close) it. What does not fit goes into a menu at its end. Screen readers
    /// are told it is a tool bar.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn tool_bar(&mut self) -> Response {
        self.bar_of_tools(false)
    }
    /// The same as a column taking the room down, its menu opening beside it.
    pub fn tool_column(&mut self) -> Response {
        self.bar_of_tools(true)
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn bar_of_tools(&mut self, down: bool) -> Response {
        let (t, id, at) = (self.theme, self.next_id(), self.tree.specs.len() as u32);
        let read = self.rooms.get(&id).copied();
        let along = if down { self.column().h(Length::Grow(1.0)).align(Align::Start, Align::Center) } else { self.row().w(Length::Grow(1.0)) };
        let bar = along.pad(4.0).gap(GAP).fill(t.panel).clip().open();
        // Its room is read from the layout: until known, everything fits, and the window is
        // described once more when it is.
        self.measures.push(Measure { at, within: at, id, read: read.unwrap_or([f32::NAN; 3]) });
        self.tell(&bar, Role::ToolBar, None, "");
        self.frame.later[0] = Some(Ui::show_left);
        let room = read.map_or(f32::INFINITY, |read| read[down as usize]);
        let more = self.id("more");
        let fresh = Bar { id, down, room, need: 0.0, taken: 0.0, all: 0.0, count: 0, out: false, line: false, seen: true, more };
        match self.frame.bars.iter_mut().find(|bar| bar.id == id) {
            Some(bar) => *bar = Bar { need: bar.need, ..fresh },
            None => self.frame.bars.push(fresh),
        }
        bar
    }
    /// The tool bar the current box is in, by its index among the bars.
    fn this_bar(&self) -> Option<usize> {
        let around = self.tree.specs[self.open.last()?.at as usize].id;
        self.frame.bars.iter().position(|bar| bar.id == around)
    }

    /// A tool: an icon button. `hint` says what it does and shows when the pointer rests on it;
    /// after a tab come its keys (`"Save\tCtrl+S"`), shown with it, for the app to handle with
    /// `key`. Where it does not fit, it is a row of the end menu, and `clicked` says when
    /// picked there.
    pub fn tool(&mut self, icon: Icon, hint: &str) -> Response {
        self.tool_like(icon, hint, None)
    }
    /// A tool that stays down while `on`: pressing it flips that, and `changed` says so.
    pub fn tool_toggle(&mut self, on: &mut bool, icon: Icon, hint: &str) -> Response {
        let mut state = self.tool_like(icon, hint, Some(*on));
        if state.clicked {
            (*on, state.changed) = (!*on, true);
        }
        state
    }
    /// The tools after this form a group: a line separates them from those before (in the end
    /// menu too).
    pub fn tool_group(&mut self) {
        if let Some(at) = self.this_bar() {
            self.frame.bars[at].line = true;
        }
    }

    /// What every tool is: shown in its bar if it fits, else noted for the end menu. `on`:
    /// whether it is down, for a toggle.
    fn tool_like(&mut self, icon: Icon, hint: &str, on: Option<bool>) -> Response {
        let Some(at) = self.this_bar() else { return self.tool_button(None, icon, hint, on) };
        let (control, bar) = (self.theme.control, &mut self.frame.bars[at]);
        let (nth, line) = (bar.count, std::mem::take(&mut bar.line) && bar.count > 0);
        let wide = control + GAP + if line { LINE + GAP } else { 0.0 };
        // Room is kept for the end button while not all tools fit.
        let more = if bar.need > bar.room { control + GAP } else { 0.0 };
        let fits = !bar.out && bar.taken + wide + more <= bar.room + GAP;
        (bar.count, bar.all, bar.taken) = (nth + 1, bar.all + wide, bar.taken + if fits { wide } else { 0.0 });
        let id = self.id(("tool", nth));
        if fits {
            if line {
                self.tool_line(at);
            }
            return self.tool_button(Some(nth), icon, hint, on);
        }
        if !std::mem::replace(&mut self.frame.bars[at].out, true) {
            self.more_button(at);
        }
        self.leave_out(at, id, icon, hint, on, line);
        // Picked in that menu: it is pressed.
        let clicked = self.frame.picked == id;
        self.frame.picked = if clicked { 0 } else { self.frame.picked };
        Response { id, clicked, ..Default::default() }
    }

    /// A tool in its bar: a quiet square icon button, held down while `on`. In a bar its id is
    /// its index there (`nth`), whether or not those before it fit; elsewhere its place.
    fn tool_button(&mut self, nth: Option<u32>, icon: Icon, hint: &str, on: Option<bool>) -> Response {
        let t = self.theme;
        let state = self.press(nth.map_or_else(|| self.next_id(), |nth| self.id(("tool", nth))));
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let down = on == Some(true) || state.pressed;
        let (fill, ink) = match (down, state.hovered) {
            (true, _) => (t.selected, t.text),
            (false, true) => (t.hover, t.text),
            (false, false) => (Color::NONE, t.text_dim),
        };
        let edge = if on == Some(true) { t.accent } else { Color::NONE };
        let frame = self.row().size(t.control, t.control).align(Align::Center, Align::Center).fill(fill).border(edge, 1.0).radius(t.radius);
        match nth {
            Some(nth) => frame.key(("tool", nth)).sense().focusable().open(),
            None => frame.sense().focusable().open(),
        };
        self.el().icon(icon, 16.0, ink).show();
        self.close();
        let (name, keys) = hint.split_once('\t').unwrap_or((hint, ""));
        self.tell(&state, Role::Button, on, name);
        self.tell_keys(keys);
        self.tooltip(&state, hint);
        state
    }
    /// The line between two groups of tools.
    fn tool_line(&mut self, at: usize) {
        let (t, down) = (self.theme, self.frame.bars[at].down);
        let [wide, high] = if down { [t.control, LINE] } else { [LINE, t.control] };
        self.row().size(wide, high).align(Align::Center, Align::Center).open();
        self.el().size(if down { t.control - 10.0 } else { 1.0 }, if down { 1.0 } else { t.control - 10.0 }).fill(t.line).show();
        self.close();
    }

    /// The button at a bar's end that opens the menu of tools that did not fit: under it, or
    /// beside it for a column.
    fn more_button(&mut self, at: usize) {
        let (t, more, down) = (self.theme, self.frame.bars[at].more, self.frame.bars[at].down);
        let state = self.press(more);
        let open = self.popups.iter().any(|popup| popup.id == more);
        if !open && (state.clicked || state.focused && self.key(if down { Key::Right } else { Key::Down }, Mods::NONE)) {
            self.pop_up(more, state.rect, down);
        }
        let fill = if open || state.hovered { t.hover } else { Color::NONE };
        self.row().key("more").size(t.control, t.control).align(Align::Center, Align::Center).fill(fill).radius(t.radius).sense().focusable().open();
        self.el().icon(MORE, 16.0, if open || state.hovered { t.text } else { t.text_dim }).show();
        self.close();
        let words = self.frame_words.more;
        self.tell(&state, Role::Fold, Some(open), words);
        self.tooltip(&state, words);
    }

    /// Notes a tool that did not fit, for its bar's end menu. (The menu's content is kept
    /// between passes and only rewritten where it changed.)
    fn leave_out(&mut self, at: usize, tool: Id, icon: Icon, hint: &str, on: Option<bool>, line: bool) {
        let (bar, count) = (self.frame.bars[at].id, self.frame.left_count);
        // The first left-out tool of a bar starts no group in its menu.
        let line = line && self.frame.left[..count].last().is_some_and(|left| left.bar == bar);
        self.frame.left_count += 1;
        if let Some(left) = self.frame.left.get_mut(count) {
            (left.bar, left.tool, left.icon, left.on, left.line) = (bar, tool, icon, on, line);
            if left.hint != hint {
                left.hint.clear();
                left.hint.push_str(hint);
            }
            return;
        }
        self.frame.left.push(Left { bar, tool, icon, hint: hint.to_string(), on, line });
    }

    /// After the rest of the window: the end menu of each tool bar whose menu is open, with a
    /// row per tool that did not fit. A picked row goes to its tool, which is described once
    /// more for it. What each bar needed in this pass is what the next goes by: where that
    /// differs, the window is described again.
    pub(crate) fn show_left(&mut self) {
        let left = std::mem::take(&mut self.frame.left);
        let count = std::mem::replace(&mut self.frame.left_count, 0);
        self.frame.bars.retain(|bar| bar.seen);
        for at in 0..self.frame.bars.len() {
            let bar = &mut self.frame.bars[at];
            (self.again, bar.need, bar.seen) = (self.again || bar.need != bar.all, bar.all, false);
            let (id, more) = (bar.id, bar.more);
            if !self.menu(more) {
                continue;
            }
            for tool in left[..count].iter().filter(|tool| tool.bar == id) {
                self.left_row(tool);
            }
            self.close();
        }
        self.frame.left = left;
        self.frame.left.truncate(count);
    }
    /// The row of a tool that did not fit, in its bar's end menu.
    fn left_row(&mut self, tool: &Left) {
        if tool.line {
            self.separator();
        }
        let row = Row { lead: Lead::Icon(tool.icon, tool.on == Some(true)), on: tool.on, ..Row::PLAIN };
        if self.menu_row(&tool.hint, row).clicked {
            (self.frame.picked, self.again) = (tool.tool, true);
            self.close_popup();
        }
    }
}
