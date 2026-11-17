//! Picking by typing, for lists too long to read through:
//!
//! ```ignore
//! // One of many: the menu has a filter box.
//! if ui.combo(&mut app.country, &COUNTRIES).changed { .. }
//! // Free text, with suggestions listed while typing.
//! if ui.autocomplete(&mut app.city, "City", &CITIES).changed { .. }
//! // A command palette over the window, with each command's shortcut.
//! if ui.key(Key::Char('k'), Mods::CTRL) { app.finding = true; }
//! if let Some(command) = ui.command_palette(&mut app.finding, &["Open a file\tCtrl+O", "Save\tCtrl+S"]) { .. }
//! ```
//!
//! An option stays if it holds every typed word, case-insensitive; prefix matches first. Arrows
//! move the highlight, Enter picks. Only visible rows cost anything, even with thousands.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::id::{Id, hash};
use crate::layout::Length;
use crate::popups::Row;
use crate::ui::{Anchor, Response, Ui};
use crate::widgets::{Before, DOWN};

/// Options shown at once; more scroll.
const SHOWN: usize = 8;
/// Tells a suggestion layer's id from its box's; also the palette's id.
const SUGGESTED: Id = 0x5a99_e575_5a99_e575;
const PALETTE: Id = 0xc0aa_a4d5_c0aa_a4d5;

/// The open option list, if any: the filter text and what it leaves. One at a time: it closes
/// when anything else is pressed.
#[derive(Default)]
pub(crate) struct Narrowed {
    /// The box these options belong to; 0 while none is open.
    of: Id,
    /// The filter typed into a `combo`'s menu. (An `autocomplete` uses its own text.)
    typed: String,
    /// Indices of the options left: prefix matches, then others that match. Also the filter and
    /// options it was computed for, as one hash.
    left: Vec<u32>,
    sifted: Option<(String, u64)>,
    /// The highlighted one, if any: what Enter picks.
    lit: Option<usize>,
    /// The focus before the list opened, for a list that gives it back.
    back: Id,
}

/// Whether `text` starts with `typed` (lower case), ignoring case.
fn begins(text: &str, typed: &str) -> bool {
    let mut small = text.chars().flat_map(char::to_lowercase);
    typed.chars().all(|letter| small.next() == Some(letter))
}

/// Whether `text` holds every word of `typed`, anywhere.
fn holds(text: &str, typed: &str) -> bool {
    typed.split_whitespace().all(|word| text.char_indices().any(|(from, _)| begins(&text[from..], word)))
}

impl Narrowed {
    /// The list of box `of` as it opens: no filter, no highlight.
    fn opened(of: Id) -> Self {
        Narrowed { of, ..Default::default() }
    }
    /// Filters `options` by `typed`. Only reruns when either changed: a pointer move over a
    /// list of 100,000 hashes it once and does not filter again.
    fn sift(&mut self, options: &[&str], typed: &str) {
        let these = hash(options);
        if self.sifted.as_ref().is_some_and(|(text, those)| text == typed && *those == these) {
            return;
        }
        let small = typed.trim().to_lowercase();
        let all = 0..options.len() as u32;
        self.left.clear();
        self.left.extend(all.clone().filter(|at| begins(options[*at as usize], &small)));
        self.left.extend(all.filter(|at| !begins(options[*at as usize], &small) && holds(options[*at as usize], &small)));
        self.sifted = Some((typed.to_string(), these));
    }
    /// Moves the highlight `step` options; returns the option it lands on. From none it goes to
    /// the first, or the last going back.
    fn step(&mut self, step: isize) -> Option<usize> {
        let last = self.left.len().checked_sub(1)?;
        let to = match (self.lit, step > 0) {
            (Some(lit), _) => lit.saturating_add_signed(step).min(last),
            (None, true) => 0,
            (None, false) => last,
        };
        self.lit = Some(to);
        Some(self.left[to] as usize)
    }
    /// The highlighted option.
    fn lit_option(&self) -> Option<usize> {
        self.lit.and_then(|lit| self.left.get(lit)).map(|at| *at as usize)
    }
}

/// Choices filtered by typing: combo box, command palette, suggestions.
impl Ui {
    /// A combo box: shows one of `options` and opens a menu with a filter box, listing the
    /// options that hold every typed word (case-insensitive; prefix matches first). Arrows move
    /// the highlight, Enter picks. `choice` is the picked index; `changed` is set when the user
    /// picks. [`ui.search_word`](Ui::search_word) is the empty box's placeholder.
    #[doc(alias("dropdown", "combobox"))]
    pub fn combo(&mut self, choice: &mut usize, options: &[&str]) -> Response {
        let mut state = self.button_like(Before::None, options.get(*choice).copied().unwrap_or("-"), Some(DOWN), false);
        if state.clicked {
            self.open_popup(state.id, state.rect);
        }
        let open = self.popups.iter().any(|popup| popup.id == state.id);
        self.tell_choice(&state, open, options.get(*choice).copied().unwrap_or(""));
        if !self.popup(state.id) {
            // Closed without a pick, it opens afresh next time.
            self.narrowed.of = if self.narrowed.of == state.id { 0 } else { self.narrowed.of };
            return state;
        }
        self.menu_box(state.rect.w.max(200.0));
        let picked = self.narrowing(state.id, options, *choice, false);
        self.close();
        let Some(picked) = picked else { return state };
        (*choice, state.changed, self.narrowed.of) = (picked, true, 0);
        self.close_popup();
        state
    }

    /// A command palette over the middle of the window while `open`: lists the `commands` that
    /// match what is typed (as in [`combo`](Ui::combo)); arrows move, Enter or a click picks.
    /// Returns the picked index and closes; Escape or a click outside closes with none. Text
    /// after a tab is the shortcut, shown at the row's end ("Save\tCtrl+S"); the app handles it
    /// with [`key`](Ui::key). Commands are written like menu rows ("&Save\tCtrl+S"), so both
    /// can share them: the `&` is not shown.
    pub fn command_palette(&mut self, open: &mut bool, commands: &[&str]) -> Option<usize> {
        let (t, of) = (self.theme, self.next_id() ^ PALETTE);
        if !*open {
            // Closed by the app, it opens afresh next time.
            self.narrowed.of = if self.narrowed.of == of { 0 } else { self.narrowed.of };
            return None;
        }
        let away = self.key(Key::Escape, Mods::NONE);
        let beside = self.layer(of, Anchor::Middle, true, Color::BLACK.with_alpha(0.4));
        let whole = self.column().pad(8.0).min(260.0, 0.0).max(460.0, f32::INFINITY).fill(t.panel).border(t.line_strong, 1.0).radius(10.0).shadow(24.0).sense().open();
        self.tell(&whole, Role::Dialog, None, "");
        let picked = self.narrowing(of, commands, 0, true);
        self.close();
        if picked.is_some() || away || beside.clicked {
            // Focus returns to where it was.
            let back = std::mem::take(&mut self.narrowed).back;
            self.set_focus(back);
            (*open, self.again) = (false, true);
        }
        picked
    }

    /// Shared by `combo` and `command_palette`: a focused filter box, with the matching options
    /// under it and one highlighted (at first `chosen`). Arrows and Enter go to the list before
    /// the box. Returns the option picked by key or click. `commands` are written as menu rows.
    fn narrowing(&mut self, of: Id, options: &[&str], chosen: usize, commands: bool) -> Option<usize> {
        let step = self.key(Key::Down, Mods::NONE) as isize - self.key(Key::Up, Mods::NONE) as isize;
        let enter = self.key(Key::Enter, Mods::NONE);
        let first = self.narrowed.of != of;
        if first {
            self.narrowed = Narrowed { back: self.focus(), ..Narrowed::opened(of) };
            self.set_focus(self.next_id());
        }
        let mut typed = std::mem::take(&mut self.narrowed.typed);
        let changed = self.text_box(&mut typed, self.search_word).changed;
        self.narrowed.sift(options, &typed);
        self.narrowed.typed = typed;
        self.el().h(Length::Px(4.0)).show();
        match (first, changed) {
            (true, _) => self.narrowed.lit = self.narrowed.left.iter().position(|at| *at as usize == chosen),
            (false, true) => self.narrowed.lit = Some(0),
            (false, false) if step != 0 => self.light_steps(options, step),
            _ => {}
        }
        let pressed = self.options_left(options, first || changed || step != 0, commands);
        pressed.or(self.narrowed.lit_option().filter(|_| enter))
    }

    /// A one-line text box with suggestions listed under it while typing: the `options` that
    /// match (as in [`combo`](Ui::combo)). One picked by pointer, or arrows and Enter, becomes
    /// the text, and `changed` is set, as for typing. Any other text is allowed: Enter picks
    /// only a highlighted option, and Escape closes the list.
    pub fn autocomplete(&mut self, text: &mut String, hint: &str, options: &[&str]) -> Response {
        let id = self.next_id();
        // While focused, the arrows go to the list; with an option highlighted, Enter and
        // Escape too. The box never gets them.
        let listed = self.focus == id && self.narrowed.of == id;
        let step = if self.focus == id { self.key(Key::Down, Mods::NONE) as isize - self.key(Key::Up, Mods::NONE) as isize } else { 0 };
        let enter = listed && self.narrowed.lit.is_some() && self.key(Key::Enter, Mods::NONE);
        let away = listed && self.key(Key::Escape, Mods::NONE);
        let mut state = self.text_box(text, hint);
        self.tell_listed(&state, false);
        // Typing, or an arrow, lists suggestions.
        if state.changed || step != 0 && !listed {
            self.narrowed = Narrowed::opened(id);
        }
        // The list closes with focus, unless the pointer is picking from it.
        let held = self.hot_layer == id ^ SUGGESTED && (self.input.down[0] || self.input.released[0]);
        if self.narrowed.of == id && (away || !state.focused && !held) {
            self.narrowed.of = 0;
        }
        if self.narrowed.of != id {
            return state;
        }
        self.narrowed.sift(options, text);
        // Nothing to list: no match, or the text is the only match.
        let itself = self.narrowed.left.len() == 1 && options[self.narrowed.left[0] as usize] == text;
        if self.narrowed.left.is_empty() || itself {
            return state;
        }
        if step != 0 && listed {
            self.light_steps(options, step);
        }
        self.tell_listed(&state, true);
        self.layer(id ^ SUGGESTED, Anchor::Under(state.rect, false), false, Color::NONE);
        self.menu_box(state.rect.w);
        let pressed = self.options_left(options, state.changed || step != 0, false);
        self.close();
        let Some(picked) = pressed.or(self.narrowed.lit_option().filter(|_| enter)) else { return state };
        // The pick becomes the text, cursor after it, focus (back) in the box.
        text.clear();
        text.push_str(options[picked]);
        (self.edit.cursor, self.edit.anchor, self.edit.reveal) = (text.len(), text.len(), true);
        (self.narrowed.of, state.changed) = (0, true);
        self.set_focus(id);
        state
    }

    /// Moves the highlight `step` options and has screen readers announce it (focus stays in
    /// the text box, so nothing else would).
    fn light_steps(&mut self, options: &[&str], step: isize) {
        if let Some(option) = self.narrowed.step(step) {
            self.announce(options[option]);
        }
    }

    /// The options left, one highlighted, in a box that scrolls past a few; scrolled to the
    /// highlight when it moved (`reveal`). Returns the clicked option. `commands` are written
    /// as menu rows.
    fn options_left(&mut self, options: &[&str], reveal: bool, commands: bool) -> Option<usize> {
        let (row, count, lit) = (self.theme.control_height, self.narrowed.left.len(), self.narrowed.lit);
        // Only as many whole rows as fit below the text box.
        let room = ((self.popup_most()[1] - row - 12.0) / row).floor().max(1.0) as usize;
        let (id, high) = (self.id("options"), count.min(SHOWN).min(room) as f32 * row);
        if let Some(lit) = lit.filter(|_| reveal) {
            let was = self.scrolls.get(&id).copied().unwrap_or_default()[1];
            self.scrolls.insert(id, [0.0, was.min(lit as f32 * row).max((lit + 1) as f32 * row - high)]);
        }
        self.column().key("options").h(Length::Px(high)).scroll().open();
        let mut pressed = None;
        for nth in self.rows(count, row) {
            let option = self.narrowed.left[nth] as usize;
            // The highlighted one is reported as selected: Enter takes it.
            let state = self.menu_row(options[option], Row { lit: lit == Some(nth), on: Some(lit == Some(nth)), letters: commands, ..Row::PLAIN });
            self.tell_position(nth + 1, count);
            pressed = pressed.or(state.clicked.then_some(option));
        }
        self.close(); // the rows
        self.close(); // the box around them
        pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_leaves_the_options_that_hold_it_and_those_that_begin_with_it_first() {
        let options = ["Argentina", "India", "Indonesia", "China", "Finland", "United Kingdom", "ÉIRE"];
        let mut list = Narrowed::opened(1);
        let mut left = |typed: &str| {
            list.sift(&options, typed);
            list.left.iter().map(|at| options[*at as usize]).collect::<Vec<_>>()
        };
        assert_eq!(left(""), options);
        // Case-insensitive, in typed text and options, in any alphabet.
        assert_eq!(left("IN"), ["India", "Indonesia", "Argentina", "China", "Finland", "United Kingdom"]);
        assert_eq!(left("éi"), ["ÉIRE"]);
        // Every word must match, in any order; spaces around them don't count.
        assert_eq!(left(" king uni "), ["United Kingdom"]);
        assert_eq!(left("king x"), [""; 0]);
    }

    #[test]
    fn the_light_goes_up_and_down_what_is_left_and_stops_at_its_ends() {
        let options = ["a", "ab", "abc", "b"];
        let mut list = Narrowed::opened(1);
        list.sift(&options, "a");
        assert_eq!((list.left.as_slice(), list.lit_option()), (&[0, 1, 2][..], None));
        // From none to the first, or the last going back.
        assert_eq!([list.step(1), list.step(1), list.step(1), list.step(1)], [Some(0), Some(1), Some(2), Some(2)]);
        assert_eq!([list.step(-1), list.step(-5)], [Some(1), Some(0)]);
        list.lit = None;
        assert_eq!((list.step(-1), list.lit_option()), (Some(2), Some(2)));
        // Nothing left, nothing to highlight.
        list.sift(&options, "zz");
        list.lit = None;
        assert_eq!((list.step(1), list.lit_option()), (None, None));
    }
}
