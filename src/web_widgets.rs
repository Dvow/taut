//! Common web and mobile widgets for desktop apps: breadcrumbs, a stepper, pagination, tags and
//! a tag input, a star rating, a search box, a split button, an avatar, a card, a banner,
//! keyboard shortcut keys, and skeleton placeholders.
//!
//! Each is a few boxes in the theme's colours, reporting `clicked`, `changed` or the picked
//! index as other parts do. Parts that may overflow (long breadcrumbs, many pages) read their
//! width from the last layout and leave out what doesn't fit.
//!
//! What takes three lines of app code is in the guide instead: toggle button groups, badges,
//! accordions, empty states, and wizard back/next buttons.
use crate::accessibility::Role;
use crate::color::Color;
use crate::el::El;
use crate::event::{Cursor, Key, Mods};
use crate::id::hash;
use crate::layout::{Align, Flow, Length};
use crate::ui::{FRAME, Icon, Measure, Picture, Response, Ui};
use crate::widgets::{Before, CHECK, CROSS, DOWN, RIGHT};

const LEFT: Icon = Icon::line("M9.5 4 5.5 8 9.5 12");
const FIRST: Icon = Icon::line("M8.5 4 4.5 8 8.5 12M12.5 4 8.5 8 12.5 12");
const LAST: Icon = Icon::line("M7.5 4 11.5 8 7.5 12M3.5 4 7.5 8 3.5 12");
const GLASS: Icon = Icon::line("M7 2.5a4.5 4.5 0 1 1 0 9 4.5 4.5 0 0 1 0-9zM10.3 10.3 13.5 13.5");
/// A five-pointed star, filled and outlined.
const STAR: &str = "M8 1.6 9.57 6.44 14.66 6.44 10.54 9.43 12.11 14.26 8 11.27 3.89 14.26 5.46 9.43 1.34 6.44 6.43 6.44z";
const STAR_FILLED: Icon = Icon::solid(STAR);
const STAR_EMPTY: Icon = Icon::line(STAR);
/// Banner icons: info, success, warning, error.
const INFO: Icon = Icon::line("M8 2a6 6 0 1 1 0 12A6 6 0 0 1 8 2zM8 7.2v4M8 4.8v.2");
const DONE: Icon = Icon::line("M8 2a6 6 0 1 1 0 12A6 6 0 0 1 8 2zM5.4 8.2l1.8 1.8 3.4-3.6");
const WARNING: Icon = Icon::line("M8 2.2 14.2 13H1.8zM8 6.4v3.2M8 11.2v.2");
const ERROR: Icon = Icon::line("M8 2a6 6 0 1 1 0 12A6 6 0 0 1 8 2zM6 6l4 4M10 6l-4 4");

/// Padding each side of a breadcrumb; the separator arrow's width with the gaps.
const CRUMB_PAD: f32 = 6.0;
const ARROW: f32 = 16.0;
/// A star's width, with its padding.
const STAR_SIZE: f32 = 20.0;
/// How long the shimmer takes to cross a placeholder, and where it starts and ends, as
/// fractions of the width (it enters before the start and leaves past the end).
const SHEEN_MS: u128 = 1400;
const SHEEN_WAY: [f32; 2] = [-0.4, 1.4];
/// Initials' colour: dark, since every avatar colour is light.
const INITIALS_INK: Color = Color::rgb(0x1b1f2c);

/// Words these parts show or tell screen readers, and the names of a range's thumbs. English
/// until the app sets its own ([`ui.widget_words`](Ui::widget_words)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidgetWords {
    /// The overflow button: hidden breadcrumbs, or a split button's other actions.
    pub more: &'static str,
    /// A tag's remove button, before the tag's word: "Remove rust".
    pub remove: &'static str,
    /// A banner's close button.
    pub close: &'static str,
    /// Pagination's first-page button.
    pub first: &'static str,
    /// Previous page.
    pub previous: &'static str,
    /// Next page.
    pub next: &'static str,
    /// Last page.
    pub last: &'static str,
    /// After the name of a completed step.
    pub done: &'static str,
    /// A range's low thumb, after the range's label: "Price, low end".
    pub low: &'static str,
    /// Its high thumb: "Price, high end".
    pub high: &'static str,
}

impl Default for WidgetWords {
    fn default() -> Self {
        WidgetWords {
            more: "More",
            remove: "Remove",
            close: "Close",
            first: "First page",
            previous: "Previous page",
            next: "Next page",
            last: "Last page",
            done: "done",
            low: "low end",
            high: "high end",
        }
    }
}

/// A banner's severity: its colour and icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Information, in blue.
    Info,
    /// Success, in green.
    Done,
    /// A warning, in amber.
    Warning,
    /// An error, in red.
    Error,
}

/// Common web and mobile widgets: breadcrumbs, stepper, tags, rating, pagination, cards,
/// banners.
impl Ui {
    /// Opens a row filling its width, and returns that width as of the last layout (before any,
    /// the window's width): for parts that drop what doesn't fit. A part that read another
    /// width than this run's layout gives is described once more.
    fn measured_row(&mut self, gap: f32) -> (Response, f32) {
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, 0.0, 0.0]);
        let row = self.row().w(Length::Grow(1.0)).gap(gap).open();
        self.measures.push(Measure { at, within: at, id, read });
        (row, read[0])
    }

    /// Breadcrumbs, from the first place (home, a drive) to the current one, with arrows
    /// between: clicking any but the last returns its index. When short of room, places after
    /// the first fold into an overflow button ("…") listing them; the last always shows, ending
    /// in "…" if even it doesn't fit.
    pub fn breadcrumbs(&mut self, places: &[&str]) -> Option<usize> {
        let (t, last) = (self.theme, places.len().saturating_sub(1));
        let (_, room) = self.measured_row(2.0);
        let widths: Vec<f32> = places.iter().map(|place| self.text_width(place, t.font) + 2.0 * CRUMB_PAD + ARROW).collect();
        // Places from the second fold away, earliest first, until the rest fits (never the
        // last: the current place). The overflow button takes about as much as a short place.
        let (mut need, mut folds) = (widths.iter().sum::<f32>(), 1);
        while need > room && folds < last {
            need += if folds == 1 { 2.0 * CRUMB_PAD + ARROW + 12.0 } else { 0.0 } - widths[folds];
            folds += 1;
        }
        let mut went = None;
        for number in (0..places.len()).filter(|number| *number == 0 || *number >= folds) {
            if number > 0 {
                self.el().icon(RIGHT, 12.0, t.text_faint).show();
            }
            if number == folds && folds > 1 {
                went = went.or(self.folded_places(&places[1..folds]));
                self.el().icon(RIGHT, 12.0, t.text_faint).show();
            }
            went = went.or(self.crumb(places[number], number, number == last).then_some(number));
        }
        self.close();
        went
    }
    /// One breadcrumb: clickable; or the last, the current place. Returns whether clicked.
    fn crumb(&mut self, place: &str, number: usize, here: bool) -> bool {
        let t = self.theme;
        let state = if here { Response::default() } else { self.press(self.id(number)) };
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        // Underlined on hover and focus, like a link.
        let (font, ink) = match (here, state.hovered || state.focused) {
            (true, _) => (t.font, t.text),
            (false, true) => (t.font.underlined(), t.text),
            (false, false) => (t.font, t.text_dim),
        };
        let place_box = self.el().key(number).text(place, font, ink).h(Length::Px(t.control_height)).pad((CRUMB_PAD, 0.0));
        if here {
            place_box.shrink(1).show();
            return false;
        }
        place_box.radius(4.0).sense().focusable().shrink(2).show();
        self.tell(&state, Role::Link, None, "");
        state.clicked
    }
    /// The overflow button for hidden breadcrumbs, and its menu: returns the picked index.
    fn folded_places(&mut self, places: &[&str]) -> Option<usize> {
        let t = self.theme;
        let state = self.press(self.id("folded"));
        if state.clicked {
            self.open_popup(state.id, state.rect);
        }
        let open = self.popups.iter().any(|popup| popup.id == state.id);
        let fill = if state.hovered || open { t.hover } else { Color::NONE };
        self.el().key("folded").text("\u{2026}", t.font, t.text_dim).h(Length::Px(t.control_height)).pad((CRUMB_PAD + 2.0, 0.0)).fill(fill).radius(4.0).sense().focusable().show();
        self.tell(&state, Role::Fold, Some(open), self.widget_words.more);
        if !self.menu(state.id) {
            return None;
        }
        let mut picked = None;
        for (number, place) in places.iter().enumerate() {
            if self.menu_item(place).clicked {
                picked = Some(number + 1);
            }
        }
        self.close();
        picked
    }

    /// A stepper: a task's steps along a line. Done steps have a tick and can be clicked to go
    /// back; the current one is marked; later ones are muted. `step` is the current index;
    /// going back sets it, and `changed` is set. (Short of room, names shrink, others' first.
    /// Going forward is the app's own button.)
    pub fn steps(&mut self, step: &mut usize, names: &[&str]) -> Response {
        let (t, at, enabled) = (self.theme, *step, self.enabled);
        let mut all = self.row().w(Length::Grow(1.0)).gap(8.0).open();
        self.tell(&all, Role::Tabs, None, "");
        for (number, name) in names.iter().enumerate() {
            if number > 0 {
                let line = if number <= at { t.accent } else { t.line_strong };
                self.el().w(Length::Grow(1.0)).min(8.0, 0.0).h(Length::Px(2.0)).radius(1.0).fill(line).show();
            }
            // Later steps are disabled: paler, and skipped by focus.
            self.set_enabled(enabled && number <= at);
            if self.step_mark(name, number, [at, names.len()]) {
                *step = number;
            }
        }
        self.set_enabled(enabled);
        self.close();
        all.changed = *step != at;
        self.again |= all.changed;
        all
    }
    /// One step: a round mark (a tick when done, else its number) and its name, `number` of
    /// `[at, count]`. Returns whether it was clicked to go back.
    fn step_mark(&mut self, name: &str, number: usize, [at, count]: [usize; 2]) -> bool {
        let t = self.theme;
        let (done, here) = (number < at, number == at);
        let state = self.press(self.id(number));
        if done && state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        // Short of room, a step shrinks down to its mark, earlier ones first.
        let fill = if done && state.hovered { t.hover } else { Color::NONE };
        let step = self.row().key(number).gap(6.0).pad(2.0).min(26.0, 0.0).radius(4.0).fill(fill).shrink(if here { 1 } else { 2 }).sense();
        match done {
            true => step.focusable().open(),
            false => step.open(),
        };
        // Reported as a tab in a tab list, the current step selected.
        let reader_name = match (self.listening, done) {
            (false, _) => String::new(),
            (true, true) => format!("{name}, {}", self.widget_words.done),
            (true, false) => name.to_string(),
        };
        self.tell(&state, Role::Tab, Some(here), &reader_name);
        self.tell_position(number + 1, count);
        let (fill, edge) = match (done, here) {
            (true, _) => (t.accent, t.accent),
            (false, true) => (Color::NONE, t.accent),
            (false, false) => (Color::NONE, t.line_strong),
        };
        self.row().size(22.0, 22.0).radius(11.0).fill(fill).border(edge, if here { 2.0 } else { 1.0 }).align(Align::Center, Align::Center).open();
        let _ = match done {
            true => self.el().icon(CHECK, 12.0, t.on_accent).show(),
            false => self.el().text(&(number + 1).to_string(), t.small_font.bold(), if here { t.accent } else { t.text_dim }).show(),
        };
        self.close();
        let font = if here { t.font.bold() } else { t.font };
        self.el().text(name, font, if done || here { t.text } else { t.text_dim }).shrink(if here { 1 } else { 2 }).show();
        self.close();
        done && state.clicked
    }

    /// A tag: a word in a small pill with a remove button. `clicked` means removal was asked:
    /// by the button, or Delete, Backspace, Enter or Space while focused. The app then removes
    /// it from its list, and focus moves to the tag taking its place. Screen readers get a
    /// button that removes the word ("Remove rust").
    pub fn tag(&mut self, word: &str) -> Response {
        let t = self.theme;
        let mut state = self.interact(self.next_id());
        let keys = [Key::Delete, Key::Backspace, Key::Enter, Key::Space];
        let keyed = state.focused && keys.into_iter().any(|key| self.key(key, Mods::NONE));
        let edge = if state.focused {
            t.accent
        } else if state.contains_pointer {
            t.line_strong
        } else {
            t.line
        };
        self.row().h(Length::Px(24.0)).pad((10.0, 0.0, 4.0, 0.0)).gap(4.0).fill(t.raised).border(edge, 1.0).radius(12.0).sense().focusable().open();
        let reader_name = if self.listening { format!("{} {word}", self.widget_words.remove) } else { String::new() };
        self.tell(&state, Role::Button, None, &reader_name);
        self.el().text(word, t.font, t.text).shrink(1).show();
        let cross = self.interact(self.next_id());
        if cross.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let (fill, ink) = if cross.hovered { (t.hover, t.text) } else { (Color::NONE, t.text_faint) };
        self.el().icon(CROSS, 12.0, ink).size(16.0, 16.0).pad(2.0).radius(8.0).fill(fill).sense().show();
        self.close();
        state.clicked = cross.clicked || keyed;
        self.again |= state.clicked;
        state
    }

    /// A tag input: Enter or a comma turns the text before it into a tag; a tag goes with its
    /// remove button, or Delete while focused. Backspace in the empty box focuses the last tag,
    /// and again removes it. `tags` are the tags, `text` what is being typed; `hint` is the
    /// placeholder, and the screen reader name. `changed` is set when the tags change.
    /// (Duplicates are not added.)
    pub fn tag_input(&mut self, tags: &mut Vec<String>, text: &mut String, hint: &str) -> Response {
        let mut all = self.column().gap(6.0).open();
        self.tell(&all, Role::Group, None, hint);
        let (mut gone, mut keyed, mut last) = (None, false, 0);
        if !tags.is_empty() {
            self.wrapping_row().gap(4.0).open();
            for (number, tag) in tags.iter().enumerate() {
                last = self.next_id();
                let state = self.tag(tag);
                gone = gone.or(state.clicked.then_some(number));
                keyed |= state.clicked && state.focused;
            }
            self.close();
        }
        // The text box sits in its own row, so its id is the same however many tags precede it.
        self.row().key("typed").open();
        let typed = self.next_id();
        if self.focus() == typed && text.is_empty() && last != 0 && self.key(Key::Backspace, Mods::NONE) {
            self.set_focus(last);
        }
        let state = self.text_box(text, hint);
        self.close();
        self.close();
        // A comma ends a tag, and Enter the one being typed.
        let enter = state.focused && self.key(Key::Enter, Mods::NONE);
        let ends = if enter { Some(text.len()) } else { text.rfind(',') };
        let had = tags.len();
        if let Some(at) = ends {
            let rest = text.split_off(at);
            let made = std::mem::replace(text, rest.trim_start_matches(',').trim_start().to_string());
            add_tags(tags, &made);
        }
        if let Some(at) = gone {
            tags.remove(at);
        }
        // A tag removed by key returns focus to the box.
        if keyed {
            self.set_focus(typed);
        }
        all.changed = gone.is_some() || tags.len() != had;
        self.again |= all.changed;
        all
    }

    /// A star rating: `value` from 0 to `stars`, in steps of `step` (1 for whole stars, 0.5 for
    /// halves). Hover previews a click; clicking the current value resets to zero. Arrows step,
    /// Home/End to none and all; screen readers get a slider. `changed` is set on change.
    pub fn rating(&mut self, value: &mut f32, stars: u8, step: f32) -> Response {
        let (t, was, most, step) = (self.theme, *value, stars as f32, step.clamp(0.05, 1.0));
        let mut state = self.interact(self.next_id());
        let near = |to: f32| ((to / step).round() * step).clamp(0.0, most);
        // What a click at the pointer would give: the star (or part of one) under it.
        let at = state.rect.x;
        let pointed = self.pointer().filter(|_| state.hovered).map(|p| (((p.x - at) / STAR_SIZE / step).ceil() * step).clamp(step, most));
        if state.hovered {
            self.follow_pointer();
            self.set_cursor(Cursor::Hand);
        }
        if let (true, Some(to)) = (state.clicked, pointed) {
            *value = if near(*value) == to { 0.0 } else { to };
        }
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *value = near(to);
        }
        if state.focused {
            let by = self.key(Key::Right, Mods::NONE) as i8 - self.key(Key::Left, Mods::NONE) as i8;
            let ends = [self.key(Key::Home, Mods::NONE), self.key(Key::End, Mods::NONE)];
            *value = match ends {
                [true, _] => 0.0,
                [_, true] => most,
                _ => near(*value) + by as f32 * step,
            };
        }
        *value = value.clamp(0.0, most);
        state.changed = *value != was;
        self.again |= state.changed;
        let shown = pointed.unwrap_or(*value);
        self.row().radius(4.0).sense().focusable().open();
        self.tell_value(&state, Role::Slider, [*value, 0.0, most]);
        self.tell_step(step);
        for star in 0..stars {
            self.star((shown - star as f32).clamp(0.0, 1.0), t.amber, t.text_faint);
        }
        self.close();
        state
    }
    /// One star, `part` filled: an outline, overlaid with that much of a filled star.
    fn star(&mut self, part: f32, filled: Color, empty: Color) {
        let inset = (STAR_SIZE - 16.0) / 2.0;
        self.el().flow(Flow::Stack).size(STAR_SIZE, STAR_SIZE).open();
        self.el().icon(STAR_EMPTY, 16.0, if part < 1.0 { empty } else { filled }).offset(inset, inset).show();
        self.el().w(Length::Px(STAR_SIZE * part)).h(Length::Px(STAR_SIZE)).clip().open();
        self.el().icon(STAR_FILLED, 16.0, filled).offset(inset, inset).show();
        self.close();
        self.close();
    }

    /// Pagination: first and previous buttons, page numbers around the current one (first and
    /// last always, "…" for gaps), next and last buttons. `page` is the current page from 0 of
    /// `count`; a click sets it, and `changed` is set. Short of room, fewer numbers show.
    /// Screen readers get the numbers as tabs.
    pub fn pagination(&mut self, page: &mut usize, count: usize) -> Response {
        let (t, words, last) = (self.theme, self.widget_words, count.saturating_sub(1));
        *page = (*page).min(last);
        let was = *page;
        let (mut all, room) = self.measured_row(2.0);
        self.tell(&all, Role::Tabs, None, "");
        // As many numbers as fit after the four arrow buttons: an odd count, at most seven.
        let wide = (self.text_width(&count.to_string(), t.font) + 12.0).max(t.control_height) + 2.0;
        let fit = ((room - 4.0 * (t.control_height + 2.0)) / wide).clamp(1.0, 7.0) as usize;
        let slots = (fit - 1) / 2 * 2 + 1;
        let arrows = [(FIRST, words.first, 0), (LEFT, words.previous, was.saturating_sub(1)), (RIGHT, words.next, (was + 1).min(last)), (LAST, words.last, last)];
        for (icon, name, to) in arrows.into_iter().take(2) {
            *page = if self.page_arrow(icon, name, to != was) { to } else { *page };
        }
        for slot in 0..slots.min(count) {
            let Some(number) = page_in(slot, slots, was, count) else {
                self.el().text("\u{2026}", t.font, t.text_faint).pad((4.0, 0.0)).show();
                continue;
            };
            if self.page_number(number, number == was, count) {
                *page = number;
            }
        }
        for (icon, name, to) in arrows.into_iter().skip(2) {
            *page = if self.page_arrow(icon, name, to != was) { to } else { *page };
        }
        self.close();
        all.changed = *page != was;
        self.again |= all.changed;
        all
    }
    /// A pagination arrow button, disabled where it would go nowhere. Returns whether clicked.
    fn page_arrow(&mut self, icon: Icon, name: &str, goes: bool) -> bool {
        let enabled = self.enabled;
        self.set_enabled(enabled && goes);
        let state = self.button_like(Before::Icon(icon), "", None, false);
        self.tell(&state, Role::Button, None, name);
        self.set_enabled(enabled);
        state.clicked
    }
    /// A page number button, `number` of `count`: a tab, selected if current. Keyed by its
    /// number so focus stays on it while the numbers around change. Returns whether clicked.
    fn page_number(&mut self, number: usize, shown: bool, count: usize) -> bool {
        let t = self.theme;
        let state = self.press(self.id(("page", number)));
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let (fill, edge, ink) = match (shown, state.hovered) {
            (true, _) => (t.selected, t.accent, t.text),
            (false, true) => (t.hover, t.line_strong, t.text),
            (false, false) => (Color::NONE, Color::NONE, t.text_dim),
        };
        let digits = (number + 1).to_string();
        let font = if shown { t.font.bold() } else { t.font };
        self.el()
            .key(("page", number))
            .text(&digits, font, ink)
            .text_align(Align::Center)
            .h(Length::Px(t.control_height))
            .min(t.control_height, 0.0)
            .pad((6.0, 0.0))
            .fill(fill)
            .border(edge, 1.0)
            .radius(t.radius)
            .sense()
            .focusable()
            .show();
        self.tell(&state, Role::Tab, Some(shown), "");
        self.tell_position(number + 1, count);
        state.clicked
    }

    /// A split button: the main action, joined to an arrow opening a menu of alternatives (save
    /// and save as; send and send later). `clicked` means the main part was pressed. The menu
    /// is the popup of the button's `id`: while [`ui.menu(response.id)`](Ui::menu) says it is
    /// open, describe its rows, then [`close`](Self::close) it. The arrow opens it, as does
    /// Alt+Down with focus on either part.
    pub fn split_button(&mut self, label: &str) -> Response {
        let t = self.theme;
        self.row().h(Length::Px(t.control_height)).pad(1.0).fill(t.raised).border(t.line, 1.0).radius(t.radius).open();
        let main = self.press(self.next_id());
        let lit = |state: &Response| match (state.pressed, state.hovered) {
            (true, _) => t.selected,
            (false, true) => t.hover,
            (false, false) => Color::NONE,
        };
        let inner = t.control_height - 2.0;
        self.el().text(label, t.font, t.text).text_align(Align::Center).h(Length::Px(inner)).pad((10.0, 0.0)).fill(lit(&main)).radius(t.radius - 1.0).sense().focusable().show();
        self.el().w(Length::Px(1.0)).h(Length::Px(inner - 10.0)).fill(t.line).show();
        let arrow = self.press(self.next_id());
        let open = self.popups.iter().any(|popup| popup.id == main.id);
        let alt_down = (main.focused || arrow.focused) && self.key(Key::Down, Mods { alt: true, ..Mods::NONE });
        if (arrow.clicked || alt_down) && !open {
            self.open_popup(main.id, main.rect.union(arrow.rect));
        }
        let fill = if open { t.selected } else { lit(&arrow) };
        self.el().icon(DOWN, 12.0, t.text_dim).size(24.0, inner).pad((6.0, (inner - 12.0) / 2.0)).fill(fill).radius(t.radius - 1.0).sense().focusable().show();
        self.tell(&arrow, Role::Fold, Some(open), self.widget_words.more);
        self.close();
        if arrow.hovered || main.hovered {
            self.set_cursor(Cursor::Hand);
        }
        main
    }

    /// A search box: a magnifier before the text and, while there is text, a clear button after
    /// it; Escape clears too (and, when empty, passes on to a dialog's Escape). `changed` is
    /// set when the text changes, so the app searches again.
    pub fn search(&mut self, text: &mut String, hint: &str) -> Response {
        let t = self.theme;
        self.el().flow(Flow::Stack).w(Length::Grow(1.0)).open();
        self.text_margin(22.0, if text.is_empty() { 0.0 } else { 22.0 });
        let mut state = self.text_box(text, hint);
        let escaped = state.focused && !text.is_empty() && self.key(Key::Escape, Mods::NONE);
        self.el().icon(GLASS, 14.0, t.text_faint).place((Align::Start, Align::Center)).offset(9.0, 0.0).show();
        let cross = if text.is_empty() { Response::default() } else { self.interact(self.next_id()) };
        if !text.is_empty() {
            let (fill, ink) = if cross.hovered { (t.hover, t.text) } else { (Color::NONE, t.text_faint) };
            self.el().icon(CROSS, 12.0, ink).size(18.0, 18.0).pad(3.0).radius(9.0).fill(fill).place((Align::End, Align::Center)).offset(-6.0, 0.0).sense().show();
        }
        self.close();
        if cross.hovered {
            self.set_cursor(Cursor::Hand);
        }
        if escaped || cross.clicked {
            text.clear();
            state.changed = true;
            self.set_focus(state.id);
        }
        self.again |= state.changed;
        state
    }

    /// An avatar: a round picture, or without one, initials on a colour picked by the name (the
    /// same for the same name). `size` is its width and height. Screen readers get the name.
    pub fn avatar(&mut self, name: &str, picture: Option<Picture>, size: f32) -> Response {
        let t = self.theme;
        let face = self.el();
        let face = match picture {
            Some(picture) => face.picture(picture),
            None => face.fill(face_color(name)).flow(Flow::Row).align(Align::Center, Align::Center),
        };
        let shown = face.size(size, size).radius(size / 2.0).open();
        if picture.is_none() {
            let letters = initials(name);
            self.el().text(&letters, crate::text::Font { size: (size * 0.4).round(), ..t.font.bold() }, INITIALS_INK).show();
        }
        self.close();
        self.tell(&shown, Role::Text, None, name);
        shown
    }

    /// Opens a card: a rounded box for one item of many (a heading, an image, some text, its
    /// own buttons), clickable as a whole: anywhere but its buttons, or Enter or Space while
    /// focused. Describe its content, then [`close`](Self::close) it. `clicked` is set when
    /// pressed. Screen readers get a group named by its first words.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn card(&mut self) -> Response {
        let t = self.theme;
        let state = self.press(self.next_id());
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let (fill, edge) = if state.contains_pointer { (t.raised, t.line_strong) } else { (t.panel, t.line) };
        self.column().w(Length::Grow(1.0)).pad(12.0).gap(8.0).fill(fill).border(edge, 1.0).radius(10.0).sense().focusable().open();
        self.tell(&state, Role::Group, None, "");
        state
    }

    /// Opens a banner across the page: `text` with an icon, coloured by `tone`, and a close
    /// button. The app's own buttons may follow inside (an action on the message); then
    /// [`close`](Self::close) it. `clicked` means its close button was pressed: the app then
    /// stops showing it. Screen readers announce it when it first shows.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn banner(&mut self, tone: Severity, text: &str) -> Response {
        let t = self.theme;
        let (ink, mark) = match tone {
            Severity::Info => (t.blue, INFO),
            Severity::Done => (t.green, DONE),
            Severity::Warning => (t.amber, WARNING),
            Severity::Error => (t.red, ERROR),
        };
        let whole = self.el().flow(Flow::Stack).w(Length::Grow(1.0)).fill(ink.with_alpha(0.12)).border(ink.with_alpha(0.45), 1.0).radius(t.radius).sense().open();
        // Not in the last layout: it has just appeared.
        if self.visible_rect(whole.id).is_empty() {
            self.announce(text);
        }
        self.tell(&whole, Role::Group, None, text);
        let cross = self.press(self.next_id());
        let (fill, cross_ink) = if cross.hovered { (t.hover, t.text) } else { (Color::NONE, t.text_dim) };
        self.el().icon(CROSS, 12.0, cross_ink).size(24.0, 24.0).pad(6.0).radius(4.0).fill(fill).place((Align::End, Align::Center)).offset(-6.0, 0.0).sense().focusable().show();
        self.tell(&cross, Role::Button, None, self.widget_words.close);
        if cross.hovered {
            self.set_cursor(Cursor::Hand);
        }
        // The app's content goes into this row, which closes with the banner.
        self.join_next = true;
        self.row().w(Length::Grow(1.0)).pad((12.0, 8.0, 40.0, 8.0)).gap(10.0).open();
        self.el().icon(mark, 16.0, ink).show();
        self.el().text(text, t.font, t.text).wrap().w(Length::Grow(1.0)).show();
        Response { clicked: cross.clicked, ..whole }
    }

    /// Keyboard keys, each in a small box: `"Ctrl+S"` shows Ctrl and S. (`"Ctrl++"` is Ctrl and
    /// plus.) Screen readers get the text as written.
    pub fn shortcut(&mut self, keys: &str) -> Response {
        let t = self.theme;
        let all = self.row().gap(3.0).open();
        self.tell(&all, Role::Text, None, keys);
        let (held, key) = match keys.strip_suffix("++") {
            Some(held) => (held, "+"),
            None => keys.rsplit_once('+').unwrap_or(("", keys)),
        };
        for name in held.split('+').filter(|name| !name.is_empty()).chain([key]) {
            self.el().text(name, t.small_font, t.text).text_align(Align::Center).h(Length::Px(20.0)).min(20.0, 0.0).pad((5.0, 0.0)).fill(t.raised).border(t.line_strong, 1.0).radius(4.0).show();
        }
        self.close();
        all
    }

    /// A skeleton placeholder for something loading: grey, with a shimmer sweeping over it
    /// while shown (still, if the user asked the system for reduced motion). It fills the width
    /// and is one text line high unless set otherwise: set more, then `show` it; shape a few
    /// like what is coming (`.size(40.0, 40.0).radius(20.0)` for an avatar). While any shows,
    /// the window redraws 60 times a second, only where they are. Screen readers get nothing,
    /// as there is nothing to read: [`ui.announce`](Ui::announce) that something is loading.
    pub fn skeleton(&mut self) -> El<'_> {
        let t = self.theme;
        let ground = t.line;
        let light = ground.lerp(Color::WHITE, if t.bg.is_dark() { 0.06 } else { 0.6 });
        let still = self.reduced_motion();
        if !still {
            self.wake_after(FRAME);
        }
        let band = sheen(self.now.as_millis(), ground, light);
        let place = self.el().w(Length::Grow(1.0)).h(Length::Px(12.0)).radius(4.0);
        match still {
            true => place.fill(ground),
            false => place.gradient_along(90.0, &band),
        }
    }
}

/// The page that slot `slot` of `slots` (an odd count) shows, the current page being `page` of
/// `count`; `None` for "…". With more pages than slots: first and last always, as many as fit
/// around the current, "…" for gaps; with fewer than five slots, only those around it.
fn page_in(slot: usize, slots: usize, page: usize, count: usize) -> Option<usize> {
    if count <= slots {
        return Some(slot);
    }
    if slots < 5 {
        return Some(page.saturating_sub(slots / 2).min(count - slots) + slot);
    }
    let (last, near) = (count - 1, slots - 3);
    match slot {
        0 => Some(0),
        _ if slot == slots - 1 => Some(last),
        // Near the first page: the first ones in sequence, then "…".
        _ if page < near => (slot != slots - 2).then_some(slot),
        // Near the last: "…", then the last ones.
        _ if page > last - near => (slot != 1).then_some(count - slots + slot),
        // Between: "…", the current one with as many either side, "…".
        _ if slot == 1 || slot == slots - 2 => None,
        _ => Some(page + slot - 2 - (slots - 5) / 2),
    }
}

/// Adds each comma-separated word in `made` as a tag, unless already there.
fn add_tags(tags: &mut Vec<String>, made: &str) {
    for word in made.split(',').map(str::trim).filter(|word| !word.is_empty()) {
        if !tags.iter().any(|tag| tag == word) {
            tags.push(word.to_string());
        }
    }
}

/// The capitalized first letters of a name's first and last words: "Ada Lovelace" is "AL",
/// "Plato" is "P". (A word's first letter or digit: a bracket or quote before it doesn't
/// count.)
fn initials(name: &str) -> String {
    let mut words = name.split_whitespace().filter_map(|word| word.chars().find(|c| c.is_alphanumeric()));
    let first = words.next();
    first.into_iter().chain(words.next_back()).flat_map(char::to_uppercase).collect()
}

/// The background colour for a person without a picture: one of many light colours, picked by
/// the name, so it is the same every time and on every system.
fn face_color(name: &str) -> Color {
    Color::hsv((hash(name) % 360) as f32 / 360.0, 0.42, 0.88)
}

/// A placeholder's colours at `millis`: its ground, with a `light` band crossing it once per
/// `SHEEN_MS`, as gradient stops.
fn sheen(millis: u128, ground: Color, light: Color) -> [(f32, Color); 3] {
    let [from, to] = SHEEN_WAY;
    let along = from + (millis % SHEEN_MS) as f32 / SHEEN_MS as f32 * (to - from);
    [(along - 0.25, ground), (along, light), (along + 0.25, ground)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_show_the_first_the_last_and_those_around_the_one_shown() {
        let shown = |slots: usize, page: usize, count: usize| -> String {
            let one = |slot| page_in(slot, slots, page, count).map_or("_".to_string(), |page| (page + 1).to_string());
            (0..slots.min(count)).map(one).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(shown(7, 2, 5), "1 2 3 4 5", "all of them, where they fit");
        assert_eq!(shown(7, 0, 20), "1 2 3 4 5 _ 20");
        assert_eq!(shown(7, 3, 20), "1 2 3 4 5 _ 20");
        assert_eq!(shown(7, 4, 20), "1 _ 4 5 6 _ 20");
        assert_eq!(shown(7, 10, 20), "1 _ 10 11 12 _ 20");
        assert_eq!(shown(7, 15, 20), "1 _ 15 16 17 _ 20");
        assert_eq!(shown(7, 16, 20), "1 _ 16 17 18 19 20");
        assert_eq!(shown(7, 19, 20), "1 _ 16 17 18 19 20");
        assert_eq!(shown(5, 0, 20), "1 2 3 _ 20");
        assert_eq!(shown(5, 9, 20), "1 _ 10 _ 20");
        assert_eq!(shown(5, 19, 20), "1 _ 18 19 20");
        assert_eq!(shown(3, 0, 20), "1 2 3", "too few places for the ends: the ones around the shown one");
        assert_eq!(shown(3, 9, 20), "9 10 11");
        assert_eq!(shown(3, 19, 20), "18 19 20");
        assert_eq!(shown(1, 9, 20), "10");
        // Every page appears exactly once.
        for page in 0..30 {
            for slots in [1, 3, 5, 7] {
                let all: Vec<_> = (0..slots).filter_map(|slot| page_in(slot, slots, page, 30)).collect();
                assert!(all.contains(&page) && all.windows(2).all(|two| two[0] < two[1]), "{slots} places, page {page}: {all:?}");
            }
        }
    }

    #[test]
    fn a_name_gives_its_first_letters_and_always_the_same_colour() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("  grace   brewster murray hopper "), "GH");
        assert_eq!(initials("Plato"), "P");
        assert_eq!(initials("(Émile) Zola"), "ÉZ");
        assert_eq!(initials("ßeta"), "SS", "a capital may be more than one letter");
        assert_eq!(initials(""), "");
        assert_eq!(face_color("Ada Lovelace"), face_color("Ada Lovelace"));
        assert_ne!(face_color("Ada Lovelace"), face_color("Alan Turing"));
        // Light enough for dark initials, whatever the name.
        for name in ["a", "b", "Ada", "Zed", "Ünal", "李小龍"] {
            assert!(face_color(name).to_hsv()[2] > 0.85 && !face_color(name).is_dark(), "{name}");
        }
    }

    #[test]
    fn tags_are_made_between_commas_and_never_twice() {
        let mut tags = vec!["rust".to_string()];
        add_tags(&mut tags, " gui, rust ,, fast ,gui");
        assert_eq!(tags, ["rust", "gui", "fast"]);
    }

    #[test]
    fn the_band_of_light_goes_over_a_placeholder_and_comes_round_again() {
        let (ground, light) = (Color::rgb(0x262b38), Color::rgb(0x323848));
        let middle = |millis| sheen(millis, ground, light)[1].0;
        assert!(middle(0) < 0.0 && middle(SHEEN_MS - 1) > 1.0, "it starts before the start and ends beyond the end");
        assert!(middle(300) < middle(600) && middle(SHEEN_MS) == middle(0));
    }
}
