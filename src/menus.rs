//! Overlays that float over the page until chosen from, answered or dismissed: menus, tooltips,
//! popovers and dialogs; and menu keyboard access.
//!
//! A menu is started where the app's own state says it is open, filled with rows, and closed:
//!
//! ```ignore
//! if ui.menu_button("&File") {
//!     if ui.icon_menu_item(OPEN, "&Open\tCtrl+O").clicked { open(); }
//!     if ui.submenu("&Recent") {
//!         for file in &app.recent { ui.menu_item(file); }
//!         ui.close();
//!     }
//!     ui.menu_check(&mut app.wrap, "&Wrap lines");
//!     ui.menu_radio(&mut app.view, View::List, "As a &list");
//!     ui.close();
//! }
//! ```
//!
//! The letter after a `&` is the access key (`&&` is a literal `&`): Alt with it opens a menu
//! bar menu from anywhere, and the letter alone picks a row of the front menu. Access keys are
//! underlined while Alt is held and in menus opened by keyboard, as on Windows and Linux
//! desktops (GNOME and KDE apps). Alt pressed and released alone focuses the menu bar, as on
//! Windows and in KDE, and unfocuses it again; F10 opens the first menu, as GNOME apps do.
//! Shift+F10 and the menu key open the focused box's context menu.
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::geom::{Point, Rect, Size, pt};
use crate::id::{Id, hash};
use crate::layout::{Align, Flow, Length};
use crate::path::Path;
use crate::text::Font;
use crate::told::Role;
use crate::ui::{Anchor, Icon, Response, Ui};
use crate::widgets::{Before, CHECK, DOWN, RIGHT};
use std::time::Duration;

/// How long the pointer rests before a tooltip shows; and on a menu row before submenus of the
/// other rows close.
const TIP_AFTER: Duration = Duration::from_millis(500);
const MENU_AFTER: Duration = Duration::from_millis(150);
/// Key bits distinguishing a box's tooltip, dialog, popover, popover arrow and menu layers from
/// the box itself.
const TIP: Id = 0x7171_7171_7171_7171;
const DIALOG: Id = 0xd1a1_09d1_a109_d1a1;
const CONTEXT: Id = 0xc0c0_7e87_c0c0_7e87;
const INNER: Id = 0x5b5b_3e3e_5b5b_3e3e;
const PINNED: Id = 0x0710_71ce_0710_71ce;
const NOTIFIED: Id = 0x5a1d_0471_5a1d_0471;
const POPOVER: Id = 0x9090_0e7e_9090_0e7e;
const ARROW: Id = 0xa770_3a77_a770_3a77;
/// A popover arrow's length and width.
const POINT: [f32; 2] = [8.0, 16.0];
/// The margin floating boxes keep from the window edges.
const EDGE: f32 = 4.0;
/// Alt alone.
const ALT: Mods = Mods { alt: true, ..Mods::NONE };
/// A radio menu row's dot.
const DOT: Icon = Icon::solid("M8 5a3 3 0 1 1 0 6 3 3 0 0 1 0-6z");

/// The app frame's state between runs: held modifiers, menus reserving room for leading marks,
/// tool bars and their overflow, popover arrow targets, panel positions, and where focus goes
/// in a dock.
#[derive(Default)]
pub(crate) struct Framing {
    /// The held modifiers (Shift, Control, Alt), as the window last reported.
    pub held: Mods,
    /// Alt went down alone, nothing else happened since, and it came up: the menu bar should
    /// take focus, or give it back.
    pub alone: bool,
    tapped: bool,
    /// What had focus before Alt moved it to the menu bar.
    back: Id,
    /// Focus is on a menu bar menu, as of the last pass; and so far in this one.
    on_bar: bool,
    on_bar_now: bool,
    /// The open menus were opened by keyboard: their access keys show. And the row of the front
    /// menu to focus, by index (`u32::MAX`: the last), once described.
    keyed: bool,
    go_row: Option<u32>,
    /// Each menu's row count (by id), as of the last pass; and so far in this one.
    rows: [Vec<(Id, u32)>; 2],
    /// The menus whose rows reserve room for a leading mark (check, dot, icon), by id, as of
    /// the last pass; and those with a mark in this one.
    leads: Vec<Id>,
    marked: Vec<Id>,
    /// Where each popover described this pass points its arrow, and whether it hangs below its
    /// anchor (else above).
    arrows: Vec<(Id, Point, bool)>,
    /// The tool bars, and this pass's tools that overflowed them; their count; and the tool an
    /// overflow menu row was just chosen for.
    pub bars: Vec<crate::toolbar::Bar>,
    pub left: Vec<crate::toolbar::Left>,
    pub left_count: usize,
    pub picked: Id,
    /// The panels, each with its rectangle, last pass and this: for docking a moved panel
    /// against the others. And the window size last pass.
    pub panels: [Vec<(Id, Rect)>; 2],
    pub window: Size,
    /// The dock panel focus should enter, its tab id, and once described, its box index and
    /// that id. And how many docks this pass described.
    pub into: Option<String>,
    pub into_tab: Id,
    pub into_box: Option<(u32, Id)>,
    pub docks: u32,
    /// The closable dock panels (by their name ids), last pass and this; and whether a floating
    /// dock panel's title was just double-clicked.
    pub closable: [Vec<Id>; 2],
    pub title_twice: bool,
    /// Work done after the app's description, once the app used a feature needing it (so an app
    /// that never does is built without it): tool bar overflow menus, popover arrows, focus
    /// entering a dock panel.
    pub later: [Option<fn(&mut Ui)>; 3],
    /// How many notices the app has posted: the next one's id.
    pub notices: u64,
}

/// The labels the app frame uses on icon-only buttons, for screen readers and tooltips. English
/// until the app sets its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameWords {
    /// The button at a tool bar's end that opens its overflow.
    pub more: &'static str,
    /// A panel's or tab's close button.
    pub close: &'static str,
    /// The button at a dock's tab row end that maximizes the front panel in the dock.
    pub fill: &'static str,
    /// The same button while maximized: it restores the panel.
    pub back: &'static str,
    /// A splitter between panes (`splitter`), where the pane before it has no text to name it
    /// by.
    pub splitter: &'static str,
}

impl Default for FrameWords {
    fn default() -> Self {
        FrameWords { more: "More", close: "Close", fill: "Fill the dock", back: "Back to its place", splitter: "Width of the pane" }
    }
}

/// What precedes a menu row's label: nothing, a check or radio dot (shown while on), or an icon
/// (pressed while its state is on).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Lead {
    None,
    Tick(bool),
    Dot(bool),
    Icon(Icon, bool),
}

/// A menu row's properties besides its label.
#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub lead: Lead,
    /// A submenu arrow at its end.
    pub inner: bool,
    /// It looks hovered.
    pub lit: bool,
    /// Its screen reader role, and its on state for roles that have one.
    pub role: Role,
    pub on: Option<bool>,
    /// A `&` in its label marks its access key.
    pub letters: bool,
}

impl Row {
    /// A plain row without access key: a list option.
    pub const PLAIN: Row = Row { lead: Lead::None, inner: false, lit: false, role: Role::MenuItem, on: None, letters: false };
    /// An app menu row.
    const ITEM: Row = Row { letters: true, ..Row::PLAIN };
}

/// Writes a name as a menu shows it into `words`: without the access key's `&`, and `&&` as
/// `&`. Returns the access key's position in `words` and the letter (lowercase). A `&` before
/// anything but a letter or digit is literal.
pub(crate) fn lettered(name: &str, words: &mut String) -> Option<(usize, char)> {
    words.clear();
    let (mut letter, mut chars) = (None, name.chars().peekable());
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        // (Of `&&`, the second is skipped and the first written.)
        if c == '&' && next == Some('&') {
            chars.next();
        }
        match next.filter(|next| c == '&' && next.is_alphanumeric() && letter.is_none()) {
            Some(marked) => letter = Some((words.len(), marked)),
            None => words.push(c),
        }
    }
    letter.map(|(at, c)| (at, c.to_lowercase().next().unwrap_or(c)))
}

/// Overlays: tooltips, menus, popovers, dialogs and notices.
impl Ui {
    /// The held modifiers changed to `held` (as the window reports with
    /// [`Event::Held`](crate::Event::Held)). Says whether the interface must run: while there
    /// is a menu bar or menu, whose access keys show while Alt is held, or Alt was released
    /// alone.
    pub(crate) fn hold(&mut self, held: Mods) -> bool {
        let was = std::mem::replace(&mut self.frame.held, held);
        let (down, up) = (held.alt && !was.alt, was.alt && !held.alt);
        self.frame.tapped |= up && self.frame.alone && held == Mods::NONE;
        self.frame.alone = held == ALT && (down || self.frame.alone);
        let shown = !self.bar.is_empty() || !self.popups.is_empty();
        (down || up) && shown || self.frame.tapped
    }
    /// Whether the window handles Alt itself, alone or with a letter: it has a menu bar.
    /// (Without one Alt goes to the system. Linux systems take nothing of it.)
    #[cfg(windows)]
    pub(crate) fn takes_alt(&self) -> bool {
        !self.bar.is_empty()
    }
    /// Whether access keys are underlined now: while Alt is held, and in keyboard-opened menus.
    fn letters_show(&self) -> bool {
        let held = self.frame.held;
        held.alt && !held.ctrl && self.window_focused || self.frame.on_bar || self.frame.keyed
    }
    /// Menu text, with the letter at `letter` underlined while access keys show.
    fn menu_words(&mut self, words: &str, letter: Option<usize>, font: Font, color: Color) {
        let Some(at) = letter.filter(|_| self.letters_show()) else {
            self.text(words, font, color);
            return;
        };
        let end = at + words[at..].chars().next().map_or(0, char::len_utf8);
        self.row().open();
        for (piece, font) in [(&words[..at], font), (&words[at..end], font.underlined()), (&words[end..], font)] {
            if !piece.is_empty() {
                self.text(piece, font, color);
            }
        }
        self.close();
    }

    /// Whether the pointer has rested on its box for `wait`. If not yet, the interface runs
    /// again when it has.
    pub(crate) fn has_rested(&mut self, wait: Duration) -> bool {
        let Some(since) = self.rested else { return false };
        let so_far = self.now.saturating_sub(since);
        if so_far < wait {
            self.wake_after(wait - so_far);
        }
        so_far >= wait
    }

    /// Shows `text` in a small box by `of`'s box once the pointer rests there, and while it has
    /// keyboard focus that arrived by key. After a tab, its shortcut follows, muted:
    /// `"Bold\tCtrl+B"`.
    pub fn tooltip(&mut self, of: &Response, text: &str) {
        let keyboard = of.focused && self.focus_shows();
        if text.is_empty() || !keyboard && !(of.hovered && self.has_rested(TIP_AFTER)) {
            return;
        }
        let t = self.theme;
        let (words, keys) = text.split_once('\t').unwrap_or((text, ""));
        self.layer(of.id ^ TIP, Anchor::Under(of.rect, true), false, Color::NONE);
        let tip = self.el().pad((8.0, 5.0)).max(280.0, f32::INFINITY).fill(t.raise).border(t.line_strong, 1.0).radius(5.0).shadow(8.0);
        // (Text alone may wrap; with a shortcut it stays on one short line.)
        if keys.is_empty() {
            tip.open();
            self.el().text(words, t.small, t.text).wrap().show();
        } else {
            tip.flow(Flow::Row).gap(12.0).align(Align::Start, Align::Center).open();
            self.text(words, t.small, t.text);
            self.text(keys, t.small, t.text_dim);
        }
        self.close();
    }

    /// Starts a menu by the box that opened it, if popup `id` is open (see
    /// [`open_popup`](Self::open_popup)), and says whether it is. When it is, describe its rows
    /// ([`menu_item`](Self::menu_item), [`icon_menu_item`](Self::icon_menu_item),
    /// [`menu_check`](Self::menu_check), [`menu_radio`](Self::menu_radio), `submenu`, a
    /// `separator`), then [`close`](Self::close) it. Up and Down move through its rows. A menu
    /// taller than the window scrolls.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn menu(&mut self, id: Id) -> bool {
        let Some(depth) = self.popups.iter().position(|popup| popup.id == id) else { return false };
        // Left leaves a submenu.
        if depth > 0 && depth + 1 == self.popups.len() && self.key(Key::Left, Mods::NONE) {
            self.close_popups(depth);
            return false;
        }
        // Below a box, it is at least as wide as the box.
        let wide = if self.popups[depth].beside { 0.0 } else { self.popups[depth].anchor.w };
        if !self.popup(id) {
            return false;
        }
        let step = self.key(Key::Down, Mods::NONE) as i8 - self.key(Key::Up, Mods::NONE) as i8;
        if step != 0 {
            self.move_focus(step > 0);
        }
        // Home and End go to the first and last row, in the front menu.
        let front = depth + 1 == self.popups.len();
        if front && self.key(Key::Home, Mods::NONE) {
            self.frame.go_row = Some(0);
        }
        if front && self.key(Key::End, Mods::NONE) {
            self.frame.go_row = Some(u32::MAX);
        }
        self.menu_box(wide);
        true
    }
    /// The largest size of a popup's own box: the window, inset a little. (Larger content
    /// inside scrolls.)
    pub(crate) fn popup_most(&self) -> [f32; 2] {
        [self.size().w - 2.0 * EDGE, self.size().h - 2.0 * EDGE].map(|most| most.max(self.theme.control))
    }
    /// Starts the box holding a menu's rows, at least `wide`, never taller than the window: the
    /// rest scrolls.
    #[cfg_attr(debug_assertions, track_caller)]
    pub(crate) fn menu_box(&mut self, wide: f32) {
        let (t, high) = (self.theme, self.size().h - 2.0 * EDGE);
        let rows = self.column().pad(4.0).min(wide.max(170.0), 0.0).max(f32::INFINITY, high.max(t.control)).scroll();
        let rows = rows.fill(t.raise).border(t.line_strong, 1.0).radius(8.0).shadow(16.0).sense().open();
        self.tell(&rows, Role::Menu, None, "");
    }
    /// Starts a menu at the pointer when `of` is right-clicked, and says whether it is open.
    /// When it is, describe its rows, then [`close`](Self::close) it. While `of` has focus,
    /// Shift+F10 and the menu key open it too, below `of`.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn context_menu(&mut self, of: &Response) -> bool {
        if let (true, Some(at)) = (of.right_clicked, self.pointer()) {
            self.open_popup(of.id ^ CONTEXT, Rect { x: at.x, y: at.y, w: 0.0, h: 0.0 });
        }
        if of.focused && (self.key(Key::Menu, Mods::NONE) || self.key(Key::F(10), Mods::SHIFT)) {
            self.open_popup(of.id ^ CONTEXT, of.rect);
            (self.frame.keyed, self.frame.go_row) = (true, Some(0));
        }
        self.menu(of.id ^ CONTEXT)
    }

    /// What menu rows share. After a tab, `label` continues with muted text for the row's far
    /// end: the shortcut, aligned across rows. `row` gives its other properties.
    pub(crate) fn menu_row(&mut self, label: &str, row: Row) -> Response {
        let t = self.theme;
        let mut state = self.press(self.next_id());
        let (name, keys) = label.split_once('\t').unwrap_or((label, ""));
        let mut words = std::mem::take(&mut self.scratch);
        words.clear();
        let letter = if row.letters { lettered(name, &mut words) } else { (words.push_str(name), None).1 };
        // Its access key picks it, alone or with Alt, while its menu is in front.
        let depth = self.depth();
        let front = depth > 0 && depth == self.popups.len();
        let picked = letter.filter(|_| front).is_some_and(|(_, c)| self.key(Key::Char(c), Mods::NONE) || self.key(Key::Char(c), ALT));
        let fill = if state.hovered || state.focused || row.lit { t.hover } else { Color::NONE };
        self.row().h(Length::Px(t.control)).pad((10.0, 0.0)).gap(8.0).fill(fill).radius(4.0).sense().focusable().open();
        self.tell(&state, row.role, row.on, &words);
        self.tell_keys(keys);
        let (nth, count) = self.menu_place(depth);
        if count > 0 {
            self.tell_place(nth as usize + 1, count as usize);
        }
        self.lead(row.lead, depth);
        self.menu_words(&words, letter.map(|(at, _)| at), t.font, t.text);
        self.el().w(Length::Grow(1.0)).min(12.0, 0.0).show();
        self.note(keys);
        if row.inner {
            self.el().icon(RIGHT, 12.0, t.text_dim).show();
        }
        self.close();
        self.scratch = words;
        // The pointer has settled on this row: submenus of other rows close.
        if state.hovered && !row.lit && self.popups.len() > depth && self.has_rested(MENU_AFTER) {
            self.close_popups(depth);
        }
        // The row focus was sent to takes it: the first, as a menu opens by keyboard; the
        // selected one, as a combo box opens; the last, at End. (Only the last is found by row
        // count: as a menu first opens, that is only known so far.)
        let sent = self.frame.go_row.map(|row| if row == u32::MAX { count.saturating_sub(1) } else { row });
        if front && sent == Some(nth) {
            (self.frame.go_row, self.focus_shown) = (None, true);
            self.set_focus(state.id);
        }
        (state.clicked, self.again) = (state.clicked || picked, self.again || picked);
        state
    }
    /// Counts a row of the menu being described (`depth` popups deep): its index from 0, and
    /// the menu's row count as the last pass found (or as far as this one has, if more). None
    /// for rows in no menu (a list below a box).
    fn menu_place(&mut self, depth: usize) -> (u32, u32) {
        let Some(menu) = depth.checked_sub(1).and_then(|at| self.popups.get(at)).map(|popup| popup.id) else { return (0, 0) };
        let [last, now] = &mut self.frame.rows;
        let nth = match now.iter_mut().find(|(id, _)| *id == menu) {
            Some((_, count)) => std::mem::replace(count, *count + 1),
            None => (now.push((menu, 1)), 0).1,
        };
        let known = last.iter().find(|(id, _)| *id == menu).map_or(0, |(_, count)| *count);
        (nth, known.max(nth + 1))
    }
    /// A menu row's leading mark, in the room the menu reserves when any row has one (only
    /// then: a text-only menu starts rows at its edge).
    fn lead(&mut self, lead: Lead, depth: usize) {
        let t = self.theme;
        let menu = depth.checked_sub(1).and_then(|at| self.popups.get(at)).map_or(0, |popup| popup.id);
        if lead != Lead::None && menu != 0 && !self.frame.marked.contains(&menu) {
            self.frame.marked.push(menu);
        }
        if menu == 0 || !self.frame.leads.contains(&menu) {
            return;
        }
        let room = self.el().size(16.0, 16.0);
        let _ = match lead {
            Lead::None | Lead::Tick(false) | Lead::Dot(false) => room.show(),
            Lead::Tick(true) => room.icon(CHECK, 16.0, t.text).pad(2.0).show(),
            Lead::Dot(true) => room.icon(DOT, 16.0, t.text).show(),
            Lead::Icon(icon, down) => room.icon(icon, 16.0, t.text).fill(if down { t.selected } else { Color::NONE }).radius(3.0).show(),
        };
    }
    /// A menu item. Choosing it closes the menu. (`"Open\tCtrl+O"` shows the shortcut at the
    /// row's far end; the app still handles it itself, with `key`. And `"&Open"` makes O its
    /// access key.)
    pub fn menu_item(&mut self, label: &str) -> Response {
        self.menu_pick(label, Lead::None)
    }
    /// The same, with an icon before its label.
    pub fn icon_menu_item(&mut self, icon: Icon, label: &str) -> Response {
        self.menu_pick(label, Lead::Icon(icon, false))
    }
    /// A menu row that is chosen, then closes the menu.
    pub(crate) fn menu_pick(&mut self, label: &str, lead: Lead) -> Response {
        let state = self.menu_row(label, Row { lead, ..Row::ITEM });
        if state.clicked {
            self.close_popup();
        }
        state
    }
    /// A check menu row, checked while `on`: choosing it flips that and closes the menu.
    pub fn menu_check(&mut self, on: &mut bool, label: &str) -> Response {
        let mut state = self.menu_row(label, Row { lead: Lead::Tick(*on), on: Some(*on), ..Row::ITEM });
        if state.clicked {
            (*on, state.changed) = (!*on, true);
            self.close_popup();
        }
        state
    }
    /// A radio menu row, one of several mutually exclusive choices, dotted while selected:
    /// choosing it sets `choice` to `value` and closes the menu.
    pub fn menu_radio<T: PartialEq>(&mut self, choice: &mut T, value: T, label: &str) -> Response {
        let picked = *choice == value;
        let mut state = self.menu_row(label, Row { lead: Lead::Dot(picked), role: Role::MenuChoice, on: Some(picked), ..Row::ITEM });
        if state.clicked {
            (state.changed, *choice) = (!picked, value);
            self.close_popup();
        }
        state
    }
    /// A menu row opening a submenu beside it; says whether that is open. When it is, describe
    /// its rows, then [`close`](Self::close) it. It opens when the pointer rests on the row, on
    /// click, with Right, and with its access key.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn submenu(&mut self, label: &str) -> bool {
        let (id, depth) = (self.next_id() ^ INNER, self.depth());
        let open = self.popups.get(depth).is_some_and(|popup| popup.id == id);
        let state = self.menu_row(label, Row { inner: true, lit: open, role: Role::Submenu, on: Some(open), ..Row::ITEM });
        let keyed = state.focused && self.key(Key::Right, Mods::NONE) || state.clicked && !self.input.released[0];
        let asked = state.clicked || keyed || state.hovered && self.has_rested(MENU_AFTER);
        if asked && !open {
            self.pop_up(id, state.rect, true);
            self.frame.go_row = if keyed { Some(0) } else { self.frame.go_row };
        }
        self.menu(id)
    }

    /// A quiet button opening a menu below it; says whether the menu is open. When it is,
    /// describe its rows, then [`close`](Self::close) it. Side by side in a
    /// [`menu_bar`](Self::menu_bar): once one is open, hovering opens the others, and Left and
    /// Right move between them. `"&File"` makes F its access key: Alt+F opens it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn menu_button(&mut self, label: &str) -> bool {
        let t = self.theme;
        let state = self.press(self.next_id());
        self.bar.push((state.id, state.rect));
        self.frame.on_bar_now |= state.focused;
        let open = self.popups.first().is_some_and(|popup| popup.id == state.id);
        let mut words = std::mem::take(&mut self.scratch);
        let letter = lettered(label, &mut words);
        // Its access key opens it: with Alt from anywhere, alone while the bar has focus.
        let alone = self.frame.on_bar && self.popups.is_empty();
        let lettered = letter.is_some_and(|(_, c)| self.key(Key::Char(c), ALT) || alone && self.key(Key::Char(c), Mods::NONE));
        let keyed = lettered || state.focused && self.key(Key::Down, Mods::NONE) || state.clicked && !self.input.released[0];
        // (Hovering opens it by moving over it, not by lying under a stationary pointer while
        // keys are pressed.)
        let over = self.input.moved && self.pointer().is_some_and(|at| state.rect.contains(at));
        if state.clicked || keyed || !open && over && self.popups.first().is_some_and(|popup| popup.bar) {
            self.open_popup(state.id, state.rect);
            self.popups.iter_mut().for_each(|popup| popup.bar = true);
            (self.frame.keyed, self.frame.go_row) = (self.frame.keyed || keyed, keyed.then_some(0));
        }
        // Escape moves focus off the bar, back to where it was.
        if state.focused && self.popups.is_empty() && self.key(Key::Escape, Mods::NONE) {
            self.set_focus(self.frame.back);
        }
        let lit = open || state.hovered || state.focused;
        self.row().h(Length::Px(t.control - 4.0)).pad((9.0, 0.0)).fill(if lit { t.hover } else { Color::NONE }).radius(4.0).sense().focusable().open();
        self.tell(&state, Role::Fold, Some(open), &words);
        self.menu_words(&words, letter.map(|(at, _)| at), t.font, if lit { t.text } else { t.text_dim });
        self.close();
        self.scratch = words;
        self.menu(state.id)
    }
    /// Starts the menu bar across the window's top: describe a
    /// [`menu_button`](Self::menu_button) per menu, then [`close`](Self::close) the bar. Alt
    /// pressed and released alone focuses its first menu (and returns focus the next time), and
    /// F10 opens that menu.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn menu_bar(&mut self) -> Response {
        let t = self.theme;
        let bar = self.row().pad((4.0, 2.0)).gap(2.0).fill(t.panel).open();
        self.tell(&bar, Role::MenuBar, None, "");
        let first = self.next_id();
        if std::mem::take(&mut self.frame.tapped) {
            self.tap_bar(first);
        }
        if self.key(Key::F(10), Mods::NONE) {
            self.open_popup(first, self.seen(first));
            self.popups.iter_mut().for_each(|popup| popup.bar = true);
            (self.frame.keyed, self.frame.go_row) = (true, Some(0));
        }
        bar
    }
    /// Alt was released alone: focus moves to the bar's first menu, or off the bar and its
    /// menus, back to where it was.
    fn tap_bar(&mut self, first: Id) {
        if self.frame.on_bar || !self.popups.is_empty() {
            let back = self.frame.back;
            self.close_popup();
            self.set_focus(back);
            return;
        }
        self.frame.back = self.focus();
        self.set_focus(first);
    }

    /// A combo box showing one of `options`, opening a menu to choose another. `choice` is the
    /// selected index; `changed` says when the user chose.
    #[doc(alias("dropdown", "combobox"))]
    pub fn select(&mut self, choice: &mut usize, options: &[&str]) -> Response {
        self.select_named(choice, options.len(), &|at| options[at])
    }
    /// The same for an app value: `options` are its possible values, each with a label;
    /// choosing one sets `value`. (A value matching none shows as "-" until one is chosen.)
    ///
    /// ```
    /// # fn describe(ui: &mut taut::Ui) {
    /// #[derive(Clone, PartialEq)]
    /// enum Size { Small, Large }
    /// let mut size = Size::Small;
    /// ui.select_value(&mut size, &[(Size::Small, "Small"), (Size::Large, "Large")]);
    /// # }
    /// ```
    pub fn select_value<T: PartialEq + Clone>(&mut self, value: &mut T, options: &[(T, &str)]) -> Response {
        let mut choice = options.iter().position(|(option, _)| option == value).unwrap_or(usize::MAX);
        let state = self.select_named(&mut choice, options.len(), &|at| options[at].1);
        if let Some((option, _)) = options.get(choice).filter(|_| state.changed) {
            *value = option.clone();
        }
        state
    }
    /// What both are: `count` options, each labelled by `name`.
    fn select_named<'a>(&mut self, choice: &mut usize, count: usize, name: &dyn Fn(usize) -> &'a str) -> Response {
        let shown = if *choice < count { name(*choice) } else { "-" };
        let mut state = self.button_like(Before::None, shown, Some(DOWN), false);
        // (Down opens it too, as Enter and Space do; it opens with focus on the selected
        // option.)
        if state.clicked || state.focused && self.key(Key::Down, Mods::NONE) {
            self.open_popup(state.id, state.rect);
            self.frame.go_row = Some(if *choice < count { *choice as u32 } else { 0 });
        }
        let open = self.popups.iter().any(|popup| popup.id == state.id);
        self.tell_choice(&state, open, if *choice < count { name(*choice) } else { "" });
        if !self.menu(state.id) {
            return state;
        }
        for number in 0..count {
            if self.menu_row(name(number), Row { on: Some(number == *choice), ..Row::PLAIN }).clicked {
                (*choice, state.changed) = (number, true);
                self.close_popup();
            }
        }
        self.close();
        state
    }

    /// Starts a box out of the box popup `id` was opened against (see
    /// [`open_popup`](Self::open_popup)), with a small arrow pointing at it, if open; says
    /// whether it is. When it is, describe its content (anything), then [`close`](Self::close)
    /// it. It hangs below its anchor, or above where there is no room below, and stays inside
    /// the window; it closes as a menu does, on a press outside or Escape.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn popover(&mut self, id: Id) -> bool {
        let Some(depth) = self.popups.iter().position(|popup| popup.id == id) else { return false };
        let by = self.popups[depth].anchor;
        if depth + 1 == self.popups.len() && self.key(Key::Escape, Mods::NONE) {
            self.close_popups(depth);
            return false;
        }
        let t = self.theme;
        let around = self.layer(id ^ POPOVER, Anchor::At(pt(0.0, 0.0)), depth == 0, Color::NONE);
        let layer = self.layers.len() - 1;
        (self.layers[layer].keys, self.layers[layer].depth) = (true, depth as u32 + 1);
        if around.hovered && (self.input.pressed[0] || self.input.pressed[1]) {
            self.close_popup();
        }
        // Its position follows from its size last time: below its anchor, centred on it, inside
        // the window.
        let whole = self.id("popover");
        let (was, room) = (self.seen(whole), self.size());
        let under = by.bottom() + POINT[0] + was.h + EDGE <= room.h || by.y - POINT[0] - was.h < EDGE;
        let x = (by.x + by.w / 2.0 - was.w / 2.0).min(room.w - was.w - EDGE).max(EDGE).round();
        let y = if under { by.bottom() + POINT[0] } else { by.y - POINT[0] - was.h }.round();
        self.layers[layer].anchor = Anchor::At(pt(x, y));
        // (Until it is in place, the window is described once more.)
        self.again |= (was.x, was.y) != (x, y) || was.is_empty();
        // The arrow points at the anchor's centre, as far as the popover's corners allow.
        let tip = (by.x + by.w / 2.0).min(x + was.w - POINT[1]).max(x + POINT[1]).round();
        self.frame.arrows.push((id, pt(tip, if under { y } else { y + was.h }), under));
        self.frame.later[1] = Some(Ui::show_arrows);
        let whole = self.column().key("popover").pad(12.0).gap(t.gap).fill(t.raise).border(t.line_strong, 1.0).radius(8.0).shadow(16.0).sense().open();
        self.tell(&whole, Role::Group, None, "");
        true
    }
    /// The arrows of this pass's popovers, each over its popover's edge.
    fn show_arrows(&mut self) {
        let (t, arrows) = (self.theme, std::mem::take(&mut self.frame.arrows));
        let [deep, wide] = POINT;
        for (id, at, under) in arrows.iter().copied() {
            // Drawn pointing up: its tip, its base on the popover's edge, and a strip below
            // covering that edge (in the popover's colour); flipped for a popover above its
            // anchor.
            let y = |y: f32| if under { y } else { deep + 2.0 - y };
            let mut fill = Path::new();
            fill.move_to(wide / 2.0, y(0.5)).line_to(0.0, y(deep + 0.5)).line_to(0.0, y(deep + 2.0)).line_to(wide, y(deep + 2.0)).line_to(wide, y(deep + 0.5)).close();
            let mut sides = Path::new();
            sides.move_to(0.0, y(deep + 0.5)).line_to(wide / 2.0, y(0.5)).line_to(wide, y(deep + 0.5));
            let top = if under { at.y - deep } else { at.y - 2.0 };
            self.layer(id ^ ARROW, Anchor::At(pt(at.x - wide / 2.0, top)), false, Color::NONE);
            self.el().flow(Flow::Layers).size(wide, deep + 2.0).open();
            self.el().grow().shape(&fill, t.raise).show();
            self.el().grow().outline(&sides, 1.0, t.line_strong).show();
            self.close();
        }
        self.frame.arrows = arrows;
        self.frame.arrows.clear();
    }

    /// Starts a modal box centred in the window, over everything, that must be answered before
    /// anything under it is reachable. Describe its content, then [`close`](Self::close) it; do
    /// so for as long as the app's state says it is open. The result reports the pointer
    /// outside the box: `clicked` there is a common way to dismiss it, and so is Escape, which
    /// reports the same.
    #[cfg_attr(debug_assertions, track_caller)]
    #[doc(alias("modal"))]
    pub fn dialog(&mut self) -> Response {
        self.dialog_up_to(520.0)
    }
    /// Pins the next opened box to a window corner, or the middle of an edge (`across` and
    /// `down` say which: `End` for both is bottom right, `Center` for one the middle that way),
    /// over the page, panels and everything described before: a bottom task bar, a floating
    /// corner button. It takes nothing from what is under it but its own position. Open the
    /// box, describe its content, and [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn pin(&mut self, across: Align, down: Align) {
        let id = hash((self.next_id(), PINNED, across as u8, down as u8));
        self.layer(id, Anchor::Corner(across, down), false, Color::NONE);
    }
    /// Starts a column in a window corner (as `pin` places it), inset a little, for transient
    /// notices. Describe a box for each, then [`close`](Self::close) it. (Described last, they
    /// lie over dialogs too.)
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn notices(&mut self, across: Align, down: Align) {
        let gap = self.theme.gap;
        self.pin(across, down);
        self.column().pad(12.0).gap(gap).align(Align::Start, across).open();
    }

    /// Shows a message in a window corner for a few seconds, over everything: "Saved", "3 files
    /// copied". It disappears by itself, or when pressed, and screen readers announce it once.
    /// Several stack, newest at the bottom. Call once, when the event happens; the library
    /// shows it from then on.
    #[doc(alias("toast"))]
    pub fn notice(&mut self, text: &str) {
        self.notice_for(text, Duration::from_secs(4));
    }
    /// The same, for `stays`.
    pub fn notice_for(&mut self, text: &str, stays: Duration) {
        self.frame.notices += 1;
        self.notified.push((text.into(), self.now.saturating_add(stays), self.frame.notices));
        self.announce(text);
        self.again = true;
    }

    /// What is described after the app's description every pass: tool bar overflow menus,
    /// popover arrows, and notices; and what the frame keeps of this pass for the next.
    pub(crate) fn show_after(&mut self) {
        for later in self.frame.later.into_iter().flatten() {
            later(self);
        }
        // A menu with a marked row reserves room for marks in all rows from now on, and one
        // without stops; a closable dock panel gets a close button from now on: the window is
        // described once more for either.
        let (size, quiet, frame) = (self.size(), self.popups.is_empty(), &mut self.frame);
        let news = frame.marked != frame.leads || frame.closable[0] != frame.closable[1] || frame.rows[0] != frame.rows[1];
        std::mem::swap(&mut frame.marked, &mut frame.leads);
        frame.closable.swap(0, 1);
        frame.rows.swap(0, 1);
        frame.rows[1].clear();
        frame.panels.swap(0, 1);
        (frame.tapped, frame.on_bar, frame.on_bar_now, frame.window, frame.docks) = (false, frame.on_bar_now, false, size, 0);
        (frame.keyed, frame.go_row) = (frame.keyed && !quiet, frame.go_row.filter(|_| !quiet));
        [&mut frame.marked, &mut frame.closable[1]].into_iter().for_each(Vec::clear);
        frame.panels[1].clear();
        self.again |= news;
        self.show_notified();
    }

    /// Shows the app's `notice`s that have not expired, in the bottom right corner; and has the
    /// interface run again when the next expires.
    fn show_notified(&mut self) {
        let (t, now) = (self.theme, self.now);
        self.notified.retain(|(_, until, _)| *until > now);
        if self.notified.is_empty() {
            return;
        }
        let notified = std::mem::take(&mut self.notified);
        self.layer(NOTIFIED, Anchor::Corner(Align::End, Align::End), false, Color::NONE);
        self.column().pad(12.0).gap(t.gap).align(Align::Start, Align::End).open();
        let mut pressed = None;
        // (Each has its own id: a press on one does not pass to another when an earlier one
        // expires meanwhile.)
        for (text, until, number) in &notified {
            let state = self.press(self.id((NOTIFIED, number)));
            self.el().key((NOTIFIED, number)).pad((12.0, 8.0)).max(320.0, f32::INFINITY).fill(t.raise).border(t.line_strong, 1.0).radius(8.0).shadow(12.0).sense().open();
            self.el().text(text, t.font, t.text).wrap().show();
            self.close();
            pressed = pressed.or(state.clicked.then_some(*number));
            self.wake_after(*until - now);
        }
        self.close();
        self.notified = notified;
        if let Some(number) = pressed {
            self.notified.retain(|(.., kept)| *kept != number);
            self.again = true;
        }
    }

    /// The same, for a dialog up to `widest` units wide. (Escape also dismisses it: the result
    /// is then `clicked`, as for a press outside.)
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn dialog_up_to(&mut self, widest: f32) -> Response {
        let t = self.theme;
        let mut beside = self.layer(self.next_id() ^ DIALOG, Anchor::Middle, true, Color::BLACK.alpha(0.5));
        // Escape counts as a press outside too: when nothing in it took the key (a menu, a list
        // below a box, a clearable box, a nested question), which is known once all of it is
        // described; then the next pass reports it. It closes the innermost dialog described.
        let away = std::mem::take(&mut self.escaped) == Some(beside.id);
        (beside.clicked, self.again, self.dialog_escape) = (beside.clicked || away, self.again || away, Some(beside.id));
        let whole = self.column().pad(18.0).gap(12.0).min(320.0f32.min(widest), 0.0).max(widest, f32::INFINITY).fill(t.panel).border(t.line_strong, 1.0).radius(10.0).shadow(24.0).sense().open();
        self.tell(&whole, Role::Dialog, None, "");
        beside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_letter_of_a_name_is_found_and_the_marks_are_left_out() {
        let mut words = String::new();
        let mut read = |name: &str| (lettered(name, &mut words), words.clone());
        assert_eq!(read("&File"), (Some((0, 'f')), "File".to_string()));
        assert_eq!(read("E&xit"), (Some((1, 'x')), "Exit".to_string()));
        assert_eq!(read("Save &As…"), (Some((5, 'a')), "Save As…".to_string()));
        // `&&` is `&`; a `&` before a non-letter is literal; only the first access key counts.
        assert_eq!(read("R&&D"), (None, "R&D".to_string()));
        assert_eq!(read("Rock & Roll"), (None, "Rock & Roll".to_string()));
        assert_eq!(read("&One &Two"), (Some((0, 'o')), "One &Two".to_string()));
        assert_eq!(read("Ö&ffnen && &Speichern"), (Some((2, 'f')), "Öffnen & &Speichern".to_string()));
        assert_eq!(read("Plain"), (None, "Plain".to_string()));
        assert_eq!(read("Ends in &"), (None, "Ends in &".to_string()));
    }
}
