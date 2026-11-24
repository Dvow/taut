//! The built-in parts placed in the page: text, buttons, check boxes, sliders, tabs, plots.
//! Each is a few boxes in the theme's colours. (Text boxes are in `text_edit`, overlays in
//! `popups` and `panel`, tables and long lists in `table`.)
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Insets, Rect};
use crate::id::{Id, hash};
use crate::layout::{Align, Flow, Length, NONE};
use crate::number::Number;
use crate::path::Path;
use crate::text::Font;
use crate::ui::{FRAME, Icon, Picture, Response, Rover, Stroke, Ui};
use std::ops::RangeInclusive;
use std::time::Duration;

pub(crate) const CHECK: Icon = Icon::line("M3 8.5 6.5 12 13 4.5");
pub(crate) const DOWN: Icon = Icon::line("M4 6.5 8 10.5 12 6.5");
pub(crate) const RIGHT: Icon = Icon::line("M6.5 4 10.5 8 6.5 12");
pub(crate) const CROSS: Icon = Icon::line("M4.5 4.5 11.5 11.5M11.5 4.5 4.5 11.5");
/// How long a switch takes to slide.
const SLIDE: Duration = Duration::from_millis(120);
/// The key of the box holding an open tree heading's children (indented a step).
const BRANCH: &str = "branch";
/// The height of a button's icon, or a picture in its place.
const LEAD: f32 = 16.0;

/// What precedes a button's label: nothing, an icon, or an app picture with its drawing
/// function. (That function is handed over by the part that has a picture: a program without
/// one is built without it.)
#[derive(Clone, Copy)]
pub(crate) enum Before {
    None,
    Icon(Icon),
    Picture(Picture, fn(&mut Ui, Picture)),
}

/// The built-in parts: text, buttons, check boxes, sliders, tabs, plots.
impl Ui {
    /// A line of text in the normal font and colour.
    pub fn label(&mut self, text: &str) -> Response {
        self.text(text, self.theme.font, self.theme.text)
    }
    /// A line of muted, smaller text.
    pub fn note(&mut self, text: &str) -> Response {
        self.text(text, self.theme.small_font, self.theme.text_dim)
    }
    /// A line of text.
    pub fn text(&mut self, text: &str, font: Font, color: Color) -> Response {
        self.el().text(text, font, color).show()
    }
    /// Text wrapped to as many lines as its width needs.
    pub fn paragraph(&mut self, text: &str) -> Response {
        let (font, color) = (self.theme.font, self.theme.text);
        self.el().w(Length::Grow(1.0)).text(text, font, color).wrap().show()
    }
    /// Link-style text: in the accent colour, underlined while hovered or focused. `clicked`
    /// says when to follow it.
    pub fn link(&mut self, label: &str) -> Response {
        let t = self.theme;
        let state = self.press(self.next_id());
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let font = if state.hovered || state.focused { t.font.underlined() } else { t.font };
        self.el().text(label, font, t.accent).radius(2.0).sense().focusable().show();
        self.tell(&state, Role::Link, None, "");
        state
    }
    /// Text as words that each flow to their own place. In a wrapping row
    /// ([`wrapping_row`](Self::wrapping_row), no gap), consecutive calls, each with its own
    /// font and colour and with links between them (`link`), wrap together as one paragraph. A
    /// newline in the text ends the line.
    pub fn words(&mut self, text: &str, font: Font, color: Color) {
        // (Screen readers hear them as one piece of text, not word by word.)
        let before = self.notes.len();
        for line in text.split_inclusive('\n') {
            // (Each word carries its trailing space: nothing is lost between pieces.)
            for word in line.trim_end_matches('\n').split_inclusive(' ') {
                self.el().text(word, font, color).show();
                self.tell_words(text, before);
            }
            // (A growing box takes a line to itself: nothing follows on this one.)
            if line.ends_with('\n') {
                self.el().w(Length::Grow(1.0)).show();
            }
        }
    }
    /// An icon in the muted text colour.
    pub fn icon(&mut self, icon: Icon) -> Response {
        let color = self.theme.text_dim;
        self.el().icon(icon, 16.0, color).show()
    }
    /// A picture added with [`add_picture`](Self::add_picture), at its own size.
    pub fn picture(&mut self, picture: Picture) -> Response {
        self.el().picture(picture).show()
    }
    /// A thin horizontal rule between groups.
    pub fn separator(&mut self) {
        let line = self.theme.line;
        self.el().w(Length::Grow(1.0)).h(Length::Px(1.0)).fill(line).show();
    }

    /// A button with a label. The result's `clicked` says when to act.
    pub fn button(&mut self, label: &str) -> Response {
        self.button_like(Before::None, label, None, false)
    }
    /// The button for the main action here: it stands out.
    pub fn primary_button(&mut self, label: &str) -> Response {
        self.button_like(Before::None, label, None, true)
    }
    /// A button with an icon before its label: "Save" with a disk. `clicked` says when to act.
    pub fn button_with_icon(&mut self, icon: Icon, label: &str) -> Response {
        self.button_like(Before::Icon(icon), label, None, false)
    }
    /// A button with an app picture before its label (added with
    /// [`add_picture`](Ui::add_picture) or [`add_picture_file`](Ui::add_picture_file)): icon
    /// height, width by its aspect ratio.
    pub fn button_with_picture(&mut self, picture: Picture, label: &str) -> Response {
        self.button_like(Before::Picture(picture, Ui::lead_picture), label, None, false)
    }
    /// An icon-only button. `hint` says what it does, shown when hovered.
    pub fn icon_button(&mut self, icon: Icon, hint: &str) -> Response {
        let state = self.button_like(Before::Icon(icon), "", None, false);
        self.tell(&state, Role::Button, None, hint);
        self.tooltip(&state, hint);
        state
    }

    /// A button that stays pressed while its state is on: bold, in a tool bar. Flips `on` when
    /// clicked; `changed` says it did.
    pub fn toggle(&mut self, on: &mut bool, label: &str) -> Response {
        self.toggle_like(on, None, label)
    }
    /// The same, icon only. `hint` says what it stands for, shown when hovered.
    pub fn icon_toggle(&mut self, on: &mut bool, icon: Icon, hint: &str) -> Response {
        let state = self.toggle_like(on, Some(icon), hint);
        self.tooltip(&state, hint);
        state
    }
    /// What both are: a pressable box that looks held while `on`. (Screen readers hear a toggle
    /// button.)
    fn toggle_like(&mut self, on: &mut bool, icon: Option<Icon>, name: &str) -> Response {
        let t = self.theme;
        let mut state = self.press(self.next_id());
        (*on, state.changed) = (*on != state.clicked, state.clicked);
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let (fill, ink, edge) = match (*on, state.hovered) {
            (true, _) => (t.selected, t.text, t.accent),
            (false, true) => (t.hover, t.text, t.line_strong),
            (false, false) => (t.raised, t.text_dim, t.line),
        };
        let frame = self.el().h(Length::Px(t.control_height)).min(t.control_height, 0.0).fill(fill).border(edge, 1.0).radius(t.radius).sense().focusable();
        let Some(icon) = icon else {
            frame.text(name, t.font, ink).text_align(Align::Center).pad((11.0, 0.0)).show();
            self.tell(&state, Role::Button, Some(*on), "");
            return state;
        };
        frame.flow(Flow::Row).align(Align::Center, Align::Center).open();
        self.el().icon(icon, 16.0, ink).show();
        self.close();
        self.tell(&state, Role::Button, Some(*on), name);
        state
    }

    /// What every button is: a pressable row with a leading icon, a label and a trailing icon,
    /// whichever there are.
    pub(crate) fn button_like(&mut self, lead: Before, label: &str, trail: Option<Icon>, primary: bool) -> Response {
        let (t, icon_only) = (self.theme, label.is_empty());
        let state = self.press(self.next_id());
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        let (fill, ink, edge) = match (primary, state.pressed, state.hovered) {
            (true, true, _) => (t.accent.with_alpha(0.8), t.on_accent, Color::NONE),
            (true, false, true) => (t.accent.with_alpha(0.9), t.on_accent, Color::NONE),
            (true, false, false) => (t.accent, t.on_accent, Color::NONE),
            (false, true, _) => (t.selected, t.text, t.line_strong),
            (false, false, true) => (t.hover, t.text, t.line_strong),
            (false, false, false) if icon_only => (Color::NONE, t.text_dim, Color::NONE),
            (false, false, false) => (t.raised, t.text, t.line),
        };
        let pad = if icon_only { Insets::ZERO } else { Insets::xy(11.0, 0.0) };
        // A label-only button is one box, the label centred.
        if let (Before::None, None, false) = (lead, trail, icon_only) {
            let font = if primary { t.font.bold() } else { t.font };
            self.el()
                .text(label, font, ink)
                .text_align(Align::Center)
                .h(Length::Px(t.control_height))
                .min(t.control_height, 0.0)
                .pad(pad)
                .fill(fill)
                .border(edge, 1.0)
                .radius(t.radius)
                .sense()
                .focusable()
                .show();
            // (Told as a button, also while disabled: one that cannot be used then. A part that
            // is something else says so after this.)
            self.tell(&state, Role::Button, None, "");
            return state;
        }
        self.el()
            .flow(Flow::Row)
            .align(Align::Center, Align::Center)
            .h(Length::Px(t.control_height))
            .min(t.control_height, 0.0)
            .pad(pad)
            .gap(6.0)
            .fill(fill)
            .border(edge, 1.0)
            .radius(t.radius)
            .sense()
            .focusable()
            .open();
        match lead {
            Before::Icon(icon) => _ = self.el().icon(icon, LEAD, ink).show(),
            Before::Picture(picture, show) => show(self, picture),
            Before::None => {}
        }
        if !icon_only {
            self.text(label, if primary { t.font.bold() } else { t.font }, ink);
        }
        if let Some(icon) = trail {
            self.el().icon(icon, 12.0, t.text_dim).show();
        }
        self.close();
        self.tell(&state, Role::Button, None, "");
        state
    }

    /// A picture before a button's label: `LEAD` high, as wide as its aspect ratio makes it (at
    /// most four times that).
    fn lead_picture(&mut self, picture: Picture) {
        let wide = self.picture_size(picture).map_or(LEAD, |(wide, high)| (LEAD * wide as f32 / high.max(1) as f32).min(4.0 * LEAD));
        self.el().picture(picture).size(wide.round(), LEAD).show();
    }

    /// What check boxes, switches and radio buttons share: a pressable row with a mark and a
    /// label. Opens the row and returns the user's interaction; the mark and label are
    /// described next, then it is closed.
    fn choice(&mut self) -> Response {
        let state = self.press(self.next_id());
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        // (Narrower in an overfull row, its label wrapping onto more lines; never shorter in an
        // overfull column: a check box is not squeezed away.)
        self.row().gap(8.0).radius(4.0).align(Align::Start, Align::Start).shrink_width(1).sense().focusable().open();
        state
    }
    /// The label beside such a mark: on as many lines as its width needs.
    fn choice_label(&mut self, label: &str) {
        let (font, color) = (self.theme.font, self.theme.text);
        self.el().text(label, font, color).wrap().shrink(1).show();
    }

    /// A check box with a label. Flips `on` when clicked; `changed` says it did.
    pub fn checkbox(&mut self, on: &mut bool, label: &str) -> Response {
        let t = self.theme;
        let mut state = self.choice();
        (*on, state.changed) = (*on != state.clicked, state.clicked);
        self.tell(&state, Role::CheckBox, Some(*on), "");
        let (fill, edge) = if *on { (t.accent, t.accent) } else { (t.panel, if state.hovered { t.text_faint } else { t.line_strong }) };
        self.row().size(16.0, 16.0).fill(fill).border(edge, 1.0).radius(4.0).align(Align::Center, Align::Center).open();
        if *on {
            self.el().icon(CHECK, 12.0, t.on_accent).show();
        }
        self.close();
        self.choice_label(label);
        self.close();
        state
    }

    /// A switch with a label: on or off, sliding when flipped.
    pub fn switch(&mut self, on: &mut bool, label: &str) -> Response {
        let t = self.theme;
        let mut state = self.choice();
        (*on, state.changed) = (*on != state.clicked, state.clicked);
        self.tell(&state, Role::Switch, Some(*on), "");
        let over = self.animate(state.id, if *on { 1.0 } else { 0.0 }, SLIDE);
        self.row().size(30.0, 16.0).pad(2.0).radius(8.0).fill(if *on { t.accent } else { t.line_strong }).open();
        self.el().w(Length::Px(14.0 * over)).show();
        self.el().size(12.0, 12.0).radius(6.0).fill(if *on { t.on_accent } else { t.text }).show();
        self.close();
        self.choice_label(label);
        self.close();
        state
    }

    /// One of several mutually exclusive choices: selecting it sets `choice` to `value`. One
    /// `choice`'s radio buttons are a single Tab stop, at the selected one; arrow keys cycle
    /// them and select the one reached.
    pub fn radio<T: PartialEq>(&mut self, choice: &mut T, value: T, label: &str) -> Response {
        let t = self.theme;
        let mut state = self.choice();
        // (Reached by an arrow from another of its choice, it is selected.)
        let arrived = state.focused && state.id != 0 && self.arrowed == state.id;
        state.changed = (state.clicked || arrived) && *choice != value;
        // (Selected so, it informs the others of its choice, already described.)
        self.again |= state.changed;
        let selected = state.changed || *choice == value;
        if self.enabled {
            let group = hash(("radio", choice as *const T as usize));
            self.rovers.push(Rover { group, id: state.id, stop: selected, selects: true });
        }
        if state.changed {
            *choice = value;
        }
        self.tell(&state, Role::RadioButton, Some(selected), "");
        let edge = if selected {
            t.accent
        } else if state.hovered {
            t.text_faint
        } else {
            t.line_strong
        };
        self.row().size(16.0, 16.0).fill(t.panel).border(edge, 1.0).radius(8.0).align(Align::Center, Align::Center).open();
        if selected {
            self.el().size(8.0, 8.0).radius(4.0).fill(t.accent).show();
        }
        self.close();
        self.choice_label(label);
        self.close();
        state
    }

    /// A knob on a track, dragged to set `value` anywhere in `range`. While focused, the usual
    /// slider keys move it: Right and Up a twentieth further, Left and Down back, Page Up and
    /// Page Down four times as far, Home and End to its ends.
    pub fn slider(&mut self, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (t, knob, was) = (self.theme, 14.0, *value);
        let (low, span) = (*range.start(), range.end() - range.start());
        let mut state = self.interact(self.next_id());
        if let (true, Some(p)) = (state.pressed, self.pointer()) {
            *value = low + span * ((p.x - state.rect.x - knob / 2.0) / (state.rect.w - knob).max(1.0)).clamp(0.0, 1.0);
        }
        // (Where a screen reader set it, it goes.)
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *value = to.clamp(low.min(low + span), low.max(low + span));
        }
        if state.focused {
            let steps = self.key_steps(true);
            *value = (*value + steps * span / 20.0).clamp(low.min(low + span), low.max(low + span));
        }
        state.changed = *value != was;
        self.again |= state.changed;
        // The track before and after the knob share the width by its position.
        let along = ((*value - low) / span).clamp(0.0, 1.0);
        self.row().w(Length::Grow(1.0)).h(Length::Px(t.control_height)).radius(4.0).sense().focusable().open();
        self.tell_value(&state, Role::Slider, [*value, low, low + span]);
        self.tell_step(span / 20.0);
        self.el().w(Length::Grow(along)).h(Length::Px(4.0)).radius(2.0).fill(t.accent).show();
        self.el().size(knob, knob).radius(knob / 2.0).fill(if state.hovered || state.pressed { t.text } else { t.text_dim }).show();
        self.el().w(Length::Grow(1.0 - along)).h(Length::Px(4.0)).radius(2.0).fill(t.line_strong).show();
        self.close();
        state
    }

    /// Two knobs on one track, dragged to set both ends of a span within `range`: `low` and
    /// `high`, neither passing the other. A press grabs the nearer knob; Tab visits each, and
    /// arrows move the focused one a twentieth.
    pub fn range(&mut self, low: &mut f32, high: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (t, knob, was) = (self.theme, 14.0, (*low, *high));
        let (least, span) = (*range.start(), (range.end() - range.start()).max(f32::MIN_POSITIVE));
        let mut state = self.interact(self.next_id());
        // The value under the pointer, and the knob grabbed at the press: the nearer one (of
        // two at one spot, the one on the pointer's side).
        let at = self.pointer().map(|p| least + span * ((p.x - state.rect.x - knob) / (state.rect.w - 2.0 * knob).max(1.0)).clamp(0.0, 1.0));
        let nearer = at.is_some_and(|at| (at - *high).abs() < (at - *low).abs() || *low == *high && at > *high);
        let taken = self.drag_delta(&state, [nearer as u8 as f32, 0.0, 0.0, 0.0]).map(|(_, caught)| caught[0] > 0.0);
        match (taken, at) {
            (Some(true), Some(at)) => *high = at.max(*low),
            (Some(false), Some(at)) => *low = at.min(*high),
            _ => {}
        }
        self.row().w(Length::Grow(1.0)).h(Length::Px(t.control_height)).radius(4.0).sense().open();
        // (Each knob is named by the span's label and its end: "Price, low end".)
        let line = self.open.last().map_or(0, |open| open.at);
        let names: [String; 2] = match self.listening {
            true => [self.widget_words.low, self.widget_words.high].map(|end| [crate::accessibility::label_before(&self.tree, &self.notes, line), end].join(", ").trim_start_matches(", ").to_string()),
            false => Default::default(),
        };
        // The knobs are queried before the track is made, so it reflects the keys. And the
        // grabbed knob takes focus from then on.
        let [first, second] = ["low", "high"].map(|name| self.interact(self.id(name)));
        let held = taken.map(|high| [first.id, second.id][high as usize]);
        if let Some(knob) = held.filter(|knob| self.focus() != *knob) {
            self.set_focus(knob);
        }
        let step = span / 20.0;
        *low = (*low + self.arrows(&first) * step).clamp(least, *high);
        *high = (*high + self.arrows(&second) * step).clamp(*low, least + span);
        let along = |value: f32| ((value - least) / span).clamp(0.0, 1.0);
        let lit = |of: &Response, held: bool| if of.focused || state.pressed && taken == Some(held) { t.text } else { t.text_dim };
        self.el().w(Length::Grow(along(*low))).h(Length::Px(4.0)).radius(2.0).fill(t.line_strong).show();
        self.el().key("low").size(knob, knob).radius(knob / 2.0).fill(lit(&first, false)).focusable().show();
        self.tell_value_named(&first, Role::Slider, [*low, least, *high], &names[0]);
        self.tell_step(step);
        self.el().w(Length::Grow(along(*high) - along(*low))).h(Length::Px(4.0)).fill(t.accent).show();
        self.el().key("high").size(knob, knob).radius(knob / 2.0).fill(lit(&second, true)).focusable().show();
        self.tell_value_named(&second, Role::Slider, [*high, *low, least + span], &names[1]);
        self.tell_step(step);
        self.el().w(Length::Grow(1.0 - along(*high))).h(Length::Px(4.0)).radius(2.0).fill(t.line_strong).show();
        self.close();
        state.changed = (*low, *high) != was;
        self.again |= state.changed;
        state
    }
    /// How many steps the keys move a slider while `of` has focus (see `steps`): none for
    /// another box.
    fn arrows(&mut self, of: &Response) -> f32 {
        if of.focused { self.key_steps(true) } else { 0.0 }
    }
    /// How many steps the standard slider keys move it: Right and Up one up, Left and Down one
    /// down, Page Up and Page Down four; and with `ends`, Home and End all the way. Each key
    /// goes to the slider that asks.
    fn key_steps(&mut self, ends: bool) -> f32 {
        let keys = [(Key::Right, 1.0), (Key::Up, 1.0), (Key::Left, -1.0), (Key::Down, -1.0), (Key::PageUp, 4.0), (Key::PageDown, -4.0), (Key::Home, -1e9), (Key::End, 1e9)];
        keys.into_iter().take(if ends { 8 } else { 6 }).filter(|(key, _)| self.key(*key, Mods::NONE)).map(|(_, steps)| steps).sum()
    }

    /// A bar showing how much of something is done: `part`, from 0 to 1. Returns its box: to
    /// name it for screen readers (`ui.tell(&bar, Role::Progress, None, "Copying")`), as what
    /// is progressing must be said.
    pub fn progress(&mut self, part: f32) -> Response {
        let (t, part) = (self.theme, part.clamp(0.0, 1.0));
        let bar = self.row().w(Length::Grow(1.0)).h(Length::Px(6.0)).radius(3.0).fill(t.line_strong).clip().open();
        self.tell_value(&bar, Role::Progress, [part, 0.0, 1.0]);
        self.el().w(Length::Grow(part)).h(Length::Grow(1.0)).radius(3.0).fill(t.accent).show();
        self.el().w(Length::Grow(1.0 - part)).show();
        self.close();
        bar
    }

    /// A progress bar as [`progress`](Ui::progress), with its percentage beside it: "42 %", in
    /// the user's number format. (Named for screen readers as `progress` is.)
    pub fn progress_with_percent(&mut self, part: f32) -> Response {
        let part = if part.is_nan() { 0.0 } else { part.clamp(0.0, 1.0) };
        self.row().w(Length::Grow(1.0)).gap(10.0).align(Align::Start, Align::Center).open();
        let bar = self.progress(part);
        self.slide_value((part * 100.0).round(), [0.0, 100.0], 1.0, "%");
        self.close();
        bar
    }

    /// A spinning ring: something is happening, with no telling how far along. (While one
    /// shows, the window redraws sixty times a second: where progress is known, `progress`
    /// costs nothing.) With reduced motion it stays still and slowly fades in and out instead.
    #[doc(alias("loading"))]
    pub fn spinner(&mut self) -> Response {
        let (t, size, millis) = (self.theme, 16.0f32, self.now.as_millis());
        // Three quarters of a circle, one turn per second; or still, its opacity cycling every
        // two seconds, drawn twenty times a second.
        let still = self.reduced_motion();
        let turned = if still { 0.0 } else { (millis % 1000) as f32 / 1000.0 * std::f32::consts::TAU };
        let solid = if still { 0.55 + 0.45 * ((millis % 2000) as f32 / 2000.0 * std::f32::consts::TAU).cos() } else { 1.0 };
        let mut ring = Path::new();
        ring.arc(size / 2.0, size / 2.0, size / 2.0 - 1.5, turned.to_degrees(), 270.0);
        self.wake_after(if still { FRAME * 3 } else { FRAME });
        // (Opened and closed though empty: an opened box has a name, by which screen readers
        // hear it.)
        let shown = self.el().size(size, size).outline(&ring, 2.0, t.accent.with_alpha(solid)).open();
        self.close();
        self.tell(&shown, Role::Progress, None, "");
        shown
    }

    /// A few choices side by side in one frame, one selected: day, week, month. Clicking one
    /// sets `choice` to its index; `changed` says when.
    pub fn segmented(&mut self, choice: &mut usize, names: &[&str]) -> Response {
        let (t, was) = (self.theme, *choice);
        // (Sized to its choices wherever placed: at the start of a stretching column,
        // vertically centred in a row.)
        let mut all = self.row().place((Align::Start, Align::Center)).pad(2.0).gap(2.0).fill(t.panel).border(t.line, 1.0).radius(t.radius).open();
        self.tell(&all, Role::Tabs, None, "");
        for (number, name) in names.iter().enumerate() {
            let state = self.press(self.next_id());
            *choice = if state.clicked { number } else { *choice };
            let (fill, ink) = match (*choice == number, state.hovered) {
                (true, _) => (t.selected, t.text),
                (false, true) => (t.hover, t.text),
                (false, false) => (Color::NONE, t.text_dim),
            };
            self.el().text(name, t.font, ink).text_align(Align::Center).h(Length::Px(t.control_height - 6.0)).pad((10.0, 0.0)).fill(fill).radius((t.radius - 2.0).max(0.0)).sense().focusable().show();
            self.tell(&state, Role::Tab, Some(*choice == number), "");
            self.tell_position(number + 1, names.len());
            self.tab_stop(all.id, state.id, *choice == number);
        }
        self.close();
        all.changed = *choice != was;
        all
    }

    /// A tab row. Clicking a tab sets `choice` to its index; the app then describes that tab's
    /// page. The row is one Tab stop, at the active tab; Left and Right move along it, Home and
    /// End to its ends, and Enter or Space activate the focused tab. A row too narrow for all
    /// wraps onto more lines.
    pub fn tabs(&mut self, choice: &mut usize, names: &[&str]) -> Response {
        let (t, was) = (self.theme, *choice);
        let mut all = self.wrapping_row().gap(2.0).open();
        self.tell(&all, Role::Tabs, None, "");
        for (number, name) in names.iter().enumerate() {
            let state = self.press(self.next_id());
            if state.clicked {
                *choice = number;
            }
            let selected = *choice == number;
            self.column().pad((10.0, 6.0, 10.0, 0.0)).gap(5.0).sense().focusable().open();
            self.tell(&state, Role::Tab, Some(selected), "");
            self.tell_position(number + 1, names.len());
            self.tab_stop(all.id, state.id, selected);
            self.text(name, t.font, if selected || state.hovered { t.text } else { t.text_dim });
            self.el().h(Length::Px(2.0)).radius(1.0).fill(if selected { t.accent } else { Color::NONE }).show();
            self.close();
        }
        self.close();
        all.changed = *choice != was;
        all
    }

    /// A tab row the user can close and reorder: one tab per item of `items`, named by `name`
    /// (names identify tabs, so keep them unique). A tab's close button removes its item; a tab
    /// dragged past the middle of its neighbour swaps with it; and `choice` follows its item.
    /// `changed` says when any of that happened. By keyboard (as `tabs`), Delete closes the
    /// focused tab, and Control+Shift with Left or Right moves it.
    pub fn tabs_of<T>(&mut self, choice: &mut usize, items: &mut Vec<T>, name: impl Fn(&T) -> &str) -> Response {
        let was = (*choice, items.len());
        let mut all = self.wrapping_row().gap(2.0).open();
        self.tell(&all, Role::Tabs, None, "");
        let (mut closed, mut moved, mut after) = (None, None, 0);
        for number in 0..items.len() {
            let beside = [number.checked_sub(1), Some(number + 1).filter(|after| *after < items.len())].map(|at| at.map(|at| name(&items[at])));
            let [selected, shut, back, on, keyed] = self.closing_tab(name(&items[number]), *choice == number, beside, all.id);
            self.tell_position(number + 1, items.len());
            *choice = if selected { number } else { *choice };
            closed = closed.or(shut.then_some(number));
            moved = moved.or(back.then(|| (number, number - 1))).or(on.then_some((number, number + 1)));
            // (A tab closed by keyboard passes focus to the next tab, or the previous.)
            if keyed && shut {
                after = beside[1].or(beside[0]).map_or(0, |name| self.id(name));
            }
        }
        self.close();
        if after != 0 {
            self.set_focus(after);
            self.focus_shown = true;
        }
        // The selected item stays selected, wherever it moves.
        if let Some((from, to)) = moved {
            items.swap(from, to);
            *choice = [to, from, *choice][if *choice == from { 0 } else { 1 + (*choice != to) as usize }];
        }
        if let Some(at) = closed {
            items.remove(at);
            *choice = (*choice - (*choice > at) as usize).min(items.len().saturating_sub(1));
        }
        all.changed = was != (*choice, items.len()) || moved.is_some();
        self.again |= all.changed;
        all
    }

    /// One closable, draggable tab in tab row `row`. Returns whether it was just selected;
    /// whether to close it (close clicked, or Delete on it); whether it is dragged past the
    /// middle of the previous or next tab (or moved there by keys); and whether the keyboard
    /// did it.
    fn closing_tab(&mut self, name: &str, selected: bool, beside: [Option<&str>; 2], row: Id) -> [bool; 5] {
        let t = self.theme;
        let tab = self.press(self.id(name));
        let dragged = self.drag_delta(&tab, [0.0; 4]).is_some_and(|(gone, _)| gone.x.abs() > 4.0);
        // The pointer while dragging the tab, and the neighbours' middles; a missing one
        // compares against nothing.
        let at = self.pointer().filter(|_| dragged).map_or(f32::NAN, |at| at.x);
        let middle = |of: Rect| if of.is_empty() { f32::NAN } else { of.x + of.w / 2.0 };
        let [before, after] = beside.map(|name| name.map_or(f32::NAN, |name| middle(self.visible_rect(self.id(name)))));
        // A closable tab's keys: Delete closes it (as ARIA has it), Control+Shift with Left or
        // Right moves it.
        let moves = |ui: &mut Ui, key: Key, to: Option<&str>| tab.focused && to.is_some() && ui.key(key, Mods { ctrl: true, shift: true, alt: false });
        let (back, on) = (moves(self, Key::Left, beside[0]), moves(self, Key::Right, beside[1]));
        let deleted = tab.focused && self.key(Key::Delete, Mods::NONE);
        self.column().key(name).pad((10.0, 4.0, 6.0, 0.0)).gap(3.0).sense().focusable().open();
        self.row().gap(6.0).open();
        self.text(name, t.font, if selected || tab.hovered { t.text } else { t.text_dim });
        let cross = self.press(self.next_id());
        self.el().icon(CROSS, 12.0, if cross.hovered { t.text } else { t.text_faint }).sense().show();
        // (The close button is told as a button of its own inside the tab, named by its action
        // and tab: "Close a.txt".)
        if self.listening {
            self.tell(&cross, Role::Button, None, &format!("{} {name}", self.frame_words.close));
            self.tell_separately(&cross);
        }
        self.close();
        self.el().h(Length::Px(2.0)).radius(1.0).fill(if selected { t.accent } else { Color::NONE }).show();
        self.close();
        // (Told last, so its position of how many can be added after this.)
        self.tell(&tab, Role::Tab, Some(selected), "");
        self.tab_stop(row, tab.id, selected);
        let keyed = deleted || back || on;
        [tab.clicked && self.gone().x.abs() <= 4.0, cross.clicked || deleted, at < before || back, at >= after || on, keyed]
    }
    /// Makes a tab of tab row `row` part of a roving group, its Tab stop at the active one,
    /// `selected`.
    fn tab_stop(&mut self, row: Id, tab: Id, selected: bool) {
        if self.enabled {
            self.rovers.push(Rover { group: row, id: tab, stop: selected, selects: false });
        }
    }

    /// A disclosure heading that hides what follows. Clicking flips `open`; returns whether it
    /// is open, and the app describes the content when it is.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn fold(&mut self, open: &mut bool, title: &str) -> bool {
        let state = self.press(self.next_id());
        *open ^= state.clicked;
        self.folding(&state, *open, title, Color::NONE);
        self.tell(&state, Role::Fold, Some(*open), "");
        *open
    }
    /// What disclosure and tree headings look like: an arrow showing whether it is open, and
    /// the title.
    #[cfg_attr(debug_assertions, track_caller)]
    fn folding(&mut self, state: &Response, open: bool, title: &str, fill: Color) {
        let t = self.theme;
        if state.hovered {
            self.set_cursor(Cursor::Hand);
        }
        self.row().gap(4.0).h(Length::Px(t.control_height)).radius(4.0).fill(fill).sense().focusable().open();
        self.el().icon(if open { DOWN } else { RIGHT }, 12.0, t.text_dim).show();
        self.text(title, t.font.bold(), t.text);
        self.close();
    }

    /// A selectable row in a list. Shows whether it is `picked`; `clicked` says when the user
    /// selects it; the app keeps which is selected.
    pub fn selectable(&mut self, picked: bool, label: &str) -> Response {
        let t = self.theme;
        let state = self.press_to_select(self.next_id());
        let fill = if picked {
            t.selected
        } else if state.hovered {
            t.hover
        } else {
            Color::NONE
        };
        self.el().text(label, t.font, t.text).h(Length::Px(t.control_height)).pad((10.0, 0.0)).fill(fill).radius(4.0).sense().focusable().show();
        self.tell(&state, Role::Row, Some(picked), "");
        // (Its position of how many in a long list: the list has counted this row already, and
        // before the first, the spacer above the visible rows.)
        let place = self.open.last().filter(|around| around.row > 0.0).map(|around| (around.children as usize - 1, around.count as usize));
        place.into_iter().for_each(|(nth, count)| self.tell_position(nth, count));
        state
    }

    /// A tree heading: collapses like `fold`, with its children indented a step. While `open`
    /// is true after it, describe its children (more of these, as deep as needed), then
    /// [`close`](Self::close) it. When focused, Right expands it (or, expanded, moves to its
    /// first child) and Left collapses it (or, collapsed, moves to its parent). The result says
    /// what the user did: `clicked` that it was pressed (pointer, Enter or Space), which also
    /// toggles it; `changed` that it expanded or collapsed, by press or arrows. For a tree of
    /// many items, see `tree_view`.
    ///
    /// ```ignore
    /// let music = ui.tree(&mut app.open, "Music");
    /// if music.clicked {
    ///     app.shown = Folder::Music; // a press shows the folder too
    /// }
    /// if app.open {
    ///     ui.label("song.ogg");
    ///     ui.close();
    /// }
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn tree(&mut self, open: &mut bool, title: &str) -> Response {
        self.tree_heading(open, title, None)
    }
    /// The same, for a tree whose headings are selectable (the folder shown, the document
    /// section visited): a `picked` one shows it, and screen readers hear it. The app keeps
    /// which is selected (a press sets `clicked`).
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn selectable_tree(&mut self, open: &mut bool, title: &str, picked: bool) -> Response {
        self.tree_heading(open, title, Some(picked))
    }
    /// A tree heading, selected or not, or (`None`) in a tree without selection.
    #[cfg_attr(debug_assertions, track_caller)]
    fn tree_heading(&mut self, open: &mut bool, title: &str, selected: Option<bool>) -> Response {
        let (mut state, was) = (self.press(self.next_id()), *open);
        // (Its place in the tree is computed only when needed: to tell, or to navigate.)
        let (level, above) = if state.focused || self.listening { self.tree_around() } else { (1, 0) };
        if state.focused {
            self.walk_tree(open, above);
        }
        *open ^= state.clicked;
        state.changed = *open != was;
        let fill = if selected == Some(true) { self.theme.selected } else { Color::NONE };
        self.folding(&state, *open, title, fill);
        self.tell(&state, Role::TreeItem, selected, "");
        self.tell_tree_item(level, Some(*open));
        if *open {
            self.column().key(BRANCH).pad((16.0, 0.0, 0.0, 0.0)).open();
        }
        state
    }
    /// The depth of a tree heading described here (1 at the top) and its parent heading (0:
    /// none): from the boxes of open headings' children it is inside, each known by its key;
    /// the heading over such a box is the one its preceding box is in.
    fn tree_around(&self) -> (usize, Id) {
        let specs = &self.tree.specs;
        let under = |at: u32| specs[at as usize].frame.parent != NONE && specs[at as usize].id == hash((specs[specs[at as usize].frame.parent as usize].id, hash(BRANCH)));
        let (count, last) = self.open.iter().map(|open| open.at).filter(|at| under(*at)).fold((0, None), |(count, _), at| (count + 1, Some(at)));
        (count + 1, last.map_or(0, |at| specs[specs[at as usize - 1].frame.parent as usize].id))
    }
    /// Arrow keys on a focused tree heading: Right expands it, or when expanded moves to its
    /// first child (right below: Down goes there); Left collapses it, or when collapsed moves
    /// to its parent.
    fn walk_tree(&mut self, open: &mut bool, above: Id) {
        match (self.key(Key::Right, Mods::NONE), *open) {
            (true, false) => (*open, self.again) = (true, true),
            (true, true) => self.input.strokes.push(Stroke::Key(Key::Down, Mods::NONE)),
            _ => {}
        }
        match (self.key(Key::Left, Mods::NONE), *open, above) {
            (true, true, _) => (*open, self.again) = (false, true),
            (true, false, above) if above != 0 => self.set_focus(above),
            _ => {}
        }
    }

    /// A number of any type changed by dragging horizontally: by `per_unit` per unit of pointer
    /// movement. It shows as many decimals as that step needs. It is focusable, and slider keys
    /// move it ten steps (Page Up and Page Down four times that); screen readers hear a slider.
    pub fn drag_value<T: Number>(&mut self, value: &mut T, per_unit: f64) -> Response {
        self.dragged(value, None, per_unit)
    }
    /// The same, clamped to `range`: dragged or stepped past an end, it stops there, and screen
    /// readers hear both ends. (An app value outside it shows as is, until the user changes
    /// it.)
    pub fn drag_value_in<T: Number>(&mut self, value: &mut T, range: RangeInclusive<T>, per_unit: f64) -> Response {
        let (low, high) = (range.start().to_f64(), range.end().to_f64());
        self.dragged(value, Some([low.min(high), high.max(low)]), per_unit)
    }
    /// What both are: a number dragged horizontally, within `range` if given.
    fn dragged<T: Number>(&mut self, value: &mut T, range: Option<[f64; 2]>, per_unit: f64) -> Response {
        let (t, was) = (self.theme, *value);
        let [low, high] = range.unwrap_or([f64::MIN, f64::MAX]);
        let within = |value: f64| T::from_f64(value.clamp(low, high));
        let mut state = self.interact(self.next_id());
        if state.hovered || state.pressed {
            self.set_cursor(Cursor::ResizeX);
        }
        // (The number is held as two smaller ones, so a large one loses nothing.)
        let whole = was.to_f64() as f32;
        if let Some((gone, caught)) = self.drag_delta(&state, [whole, (was.to_f64() - whole as f64) as f32, 0.0, 0.0]) {
            *value = within(caught[0] as f64 + caught[1] as f64 + gone.x as f64 * per_unit);
        }
        if state.focused {
            let steps = self.key_steps(false) as f64;
            *value = if steps == 0.0 { *value } else { within(value.to_f64() + steps * per_unit * 10.0) };
        }
        // (Where a screen reader set it, it goes.)
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *value = within(to as f64);
        }
        state.changed = *value != was;
        self.again |= state.changed;
        let decimals = (-per_unit.abs().max(1e-6).log10()).ceil().clamp(0.0, 6.0) as usize;
        let edge = if state.hovered || state.pressed { t.text_faint } else { t.line_strong };
        self.row().h(Length::Px(t.control_height)).min(64.0, 0.0).pad((8.0, 0.0)).align(Align::Center, Align::Center).fill(t.panel).border(edge, 1.0).radius(t.radius).sense().focusable().open();
        self.label(&format!("{:.*}", decimals, value.to_f64()));
        self.close();
        self.tell_value(&state, Role::Slider, [value.to_f64() as f32, low.max(f32::MIN as f64) as f32, high.min(f32::MAX as f64) as f32]);
        self.tell_step((per_unit * 10.0) as f32);
        state
    }

    /// A smooth curve through `values`, left to right, in a box of this size: the lowest at the
    /// bottom, the highest at the top.
    pub fn plot_lines(&mut self, values: &[f32], width: f32, height: f32) -> Response {
        self.plot(values, width, height, false)
    }
    /// A bar per value of `values`, from zero (or from the lowest, if lower).
    pub fn plot_bars(&mut self, values: &[f32], width: f32, height: f32) -> Response {
        self.plot(values, width, height, true)
    }
    fn plot(&mut self, values: &[f32], width: f32, height: f32, bars: bool) -> Response {
        let t = self.theme;
        let (low, high) = values.iter().fold((if bars { 0.0 } else { f32::MAX }, f32::MIN), |(low, high), v| (low.min(*v), high.max(*v)));
        let (span, last) = ((high - low).max(1e-6), values.len().saturating_sub(1).max(1) as f32);
        let step = width / values.len().max(1) as f32;
        let y = |v: f32| 3.0 + (height - 6.0) * (1.0 - (v - low) / span);
        let mut shape = Path::new();
        if bars {
            for (i, v) in values.iter().enumerate() {
                let (left, right) = (i as f32 * step + 1.0, (i + 1) as f32 * step - 1.0);
                shape.move_to(left, height).line_to(left, y(*v)).line_to(right, y(*v)).line_to(right, height).close();
            }
        } else {
            // (A curve through the values, not straight lines from one to the next.)
            let points: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, v)| (i as f32 * width / last, y(*v))).collect();
            shape.smooth_through(&points);
        }
        // (Its own key, to be told: by its values, as text.)
        let seat = self.next_id();
        let frame = self.el().key(("plot", seat)).size(width, height).fill(t.panel).radius(4.0);
        let plot = if bars { frame.shape(&shape, t.accent).show() } else { frame.outline(&shape, 1.5, t.accent).show() };
        if self.listening {
            self.tell(&plot, Role::Text, None, &crate::chart::values_written(values));
        }
        plot
    }

    /// A splitter between two panes, dragged horizontally to change `width`: the width of the
    /// pane before it, which the app keeps. Keyboard too: Left and Right move it a step, Home
    /// and End to `least` and `most`; screen readers hear a slider named by the first text of
    /// the pane before it.
    pub fn splitter(&mut self, width: &mut f32, least: f32, most: f32) -> Response {
        let (t, was, step) = (self.theme, *width, 10.0);
        let mut state = self.interact(self.next_id());
        if state.hovered || state.pressed {
            self.set_cursor(Cursor::ResizeX);
        }
        if let Some((gone, caught)) = self.drag_delta(&state, [was, 0.0, 0.0, 0.0]) {
            *width = (caught[0] + gone.x).clamp(least, most);
        }
        if state.focused {
            let keys = [(Key::Right, step), (Key::Left, -step), (Key::Home, f32::MIN), (Key::End, f32::MAX)];
            let by: f32 = keys.into_iter().filter(|(key, _)| self.key(*key, Mods::NONE)).map(|(_, by)| by).sum();
            *width = (*width + by).clamp(least, most.max(least));
        }
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *width = to.clamp(least, most.max(least));
        }
        state.changed = *width != was;
        self.again |= state.changed;
        let line = if state.hovered || state.pressed || state.focused { t.accent } else { t.line };
        self.row().w(Length::Px(5.0)).h(Length::Grow(1.0)).align(Align::Center, Align::Spread).sense().focusable().open();
        if self.listening {
            let at = self.open.last().map_or(0, |open| open.at);
            let name = crate::accessibility::words_before(&self.tree, at).unwrap_or(self.frame_words.splitter).to_string();
            self.tell_value_named(&state, Role::Slider, [*width, least, most], &name);
            self.tell_step(step);
        }
        self.el().w(Length::Px(1.0)).fill(line).show();
        self.close();
        state
    }
}

#[cfg(test)]
mod tests {
    use crate::accessibility::{AccessNode, Role};
    use crate::layout::Length;
    use crate::testing::Trial;
    use crate::ui::Ui;

    /// The focused element as screen readers hear it: role and name.
    fn on<A>(trial: &Trial<A>) -> Option<(Role, String)> {
        trial.visible_access_nodes().iter().find(|said| said.focused).map(|said| (said.role, said.name.clone()))
    }
    /// Where Tab goes, from nothing, so many times.
    fn tab_stops<A>(trial: &mut Trial<A>, times: usize) -> Vec<String> {
        trial.ui.set_focus(0);
        trial.run();
        (0..times).map(|_| (trial.key("Tab"), on(trial).map_or(String::new(), |(_, name)| name)).1).collect()
    }

    #[derive(Default)]
    struct Choosing {
        size: u8,
        tone: u8,
    }

    fn choosing(app: &mut Choosing, ui: &mut Ui) {
        ui.column().pad(8.0).gap(4.0).open();
        ui.button("Before");
        ui.label("Size");
        for (value, name) in ["Small", "Medium", "Large"].into_iter().enumerate() {
            ui.radio(&mut app.size, value as u8, name);
        }
        ui.label("Tone");
        for (value, name) in ["Warm", "Cold"].into_iter().enumerate() {
            ui.radio(&mut app.tone, value as u8, name);
        }
        ui.button("After");
        ui.close();
    }

    #[test]
    fn the_radio_buttons_of_a_choice_are_one_stop_and_the_arrows_go_round_them_picking() {
        let mut trial = Trial::new(Choosing { size: 1, tone: 0 }, choosing);
        // One stop per radio group, at the selected one; two groups side by side are two.
        assert_eq!(tab_stops(&mut trial, 5), ["Before", "Medium", "Warm", "After", "Before"]);
        trial.key("Shift+Tab");
        trial.key("Shift+Tab");
        assert_eq!(on(&trial), Some((Role::RadioButton, "Warm".into())), "Shift and Tab come back to the choice before");
        trial.key("Shift+Tab");
        assert_eq!(on(&trial), Some((Role::RadioButton, "Medium".into())));
        // Each tells its position of how many, in its own group.
        let places: Vec<Option<[u32; 2]>> = trial.visible_access_nodes().iter().filter(|said| said.role == Role::RadioButton).map(|said| said.position).collect();
        assert_eq!(places, [Some([1, 3]), Some([2, 3]), Some([3, 3]), Some([1, 2]), Some([2, 2])]);
        // Arrows cycle the group and select: past the last, to the first.
        trial.key("Down");
        assert_eq!((trial.app.size, on(&trial)), (2, Some((Role::RadioButton, "Large".into()))));
        trial.key("Right");
        assert_eq!((trial.app.size, on(&trial)), (0, Some((Role::RadioButton, "Small".into()))));
        trial.key("Up");
        assert_eq!((trial.app.size, trial.app.tone), (2, 0), "Up from the first goes round to the last, and the other choice is left as it was");
        assert!(trial.find(Role::RadioButton, "Large").on == Some(true));
    }

    struct Paging {
        page: usize,
        span: usize,
        paper: usize,
        papers: Vec<&'static str>,
    }

    fn paging(app: &mut Paging, ui: &mut Ui) {
        ui.column().pad(8.0).gap(6.0).open();
        ui.tabs(&mut app.page, &["One", "Two", "Three"]);
        ui.segmented(&mut app.span, &["Day", "Week"]);
        ui.tabs_of(&mut app.paper, &mut app.papers, |paper| paper);
        ui.button("After");
        ui.close();
    }

    #[test]
    fn a_row_of_tabs_is_one_stop_gone_along_with_the_arrows_home_and_end() {
        let mut trial = Trial::new(Paging { page: 1, span: 0, paper: 0, papers: vec!["a.txt", "b.txt", "c.txt"] }, paging);
        assert_eq!(tab_stops(&mut trial, 4), ["Two", "Day", "a.txt", "After"]);
        trial.ui.set_focus(0);
        trial.run();
        trial.key("Tab");
        // Left and Right cycle the row; Home and End go to its ends; none of them activates.
        for (key, to) in [("Right", "Three"), ("Right", "One"), ("Left", "Three"), ("Home", "One"), ("End", "Three")] {
            trial.key(key);
            assert_eq!(on(&trial), Some((Role::Tab, to.into())), "{key}");
        }
        assert_eq!(trial.app.page, 1);
        trial.key("Enter");
        assert_eq!(trial.app.page, 2);
        // (Down is not the row's: it moves on, as anywhere.)
        trial.key("Down");
        assert!(on(&trial).is_some_and(|(_, name)| name == "Day" || name == "Week"), "{:?}", on(&trial));
    }

    #[test]
    fn tabs_that_close_are_closed_and_moved_with_keys_and_their_cross_is_told() {
        let mut trial = Trial::new(Paging { page: 0, span: 0, paper: 1, papers: vec!["a.txt", "b.txt", "c.txt"] }, paging);
        let cross = trial.find(Role::Button, "Close b.txt").clone();
        assert_eq!(trial.visible_access_nodes()[cross.parent.unwrap()].role, Role::Tabs, "the cross is told in the row of tabs");
        let stops = tab_stops(&mut trial, 3);
        assert_eq!(stops[2], "b.txt", "the row stops at the tab in front");
        // Control+Shift+Right moves the tab on: the selection follows it.
        trial.key("Ctrl+Shift+Right");
        assert_eq!((trial.app.papers.as_slice(), trial.app.paper), (["a.txt", "c.txt", "b.txt"].as_slice(), 2));
        assert_eq!(on(&trial), Some((Role::Tab, "b.txt".into())));
        trial.key("Ctrl+Shift+Right");
        assert_eq!(trial.app.papers, ["a.txt", "c.txt", "b.txt"], "the last goes no further");
        trial.key("Ctrl+Shift+Left");
        trial.key("Ctrl+Shift+Left");
        assert_eq!((trial.app.papers.as_slice(), trial.app.paper), (["b.txt", "a.txt", "c.txt"].as_slice(), 0));
        // Delete closes the tab, and focus moves to the next.
        trial.key("Delete");
        assert_eq!(trial.app.papers, ["a.txt", "c.txt"]);
        assert_eq!(on(&trial), Some((Role::Tab, "a.txt".into())));
        trial.key("End");
        trial.key("Delete");
        assert_eq!((trial.app.papers.as_slice(), on(&trial)), (["a.txt"].as_slice(), Some((Role::Tab, "a.txt".into()))), "the last tab hands the keyboard back to the one before");
    }

    #[test]
    fn with_larger_text_tabs_that_do_not_fit_go_onto_another_line() {
        let mut trial = Trial::new(Paging { page: 0, span: 0, paper: 0, papers: vec!["a.txt", "b.txt", "c.txt"] }, paging);
        trial.ui.theme = trial.ui.theme.text_scaled(2.5);
        trial.resize(300.0, 400.0, 1.0);
        let tabs: Vec<&AccessNode> = trial.visible_access_nodes().iter().filter(|said| said.role == Role::Tab).collect();
        assert!(tabs.iter().all(|said| said.rect.right() <= 300.0), "{:?}", tabs.iter().map(|said| said.rect).collect::<Vec<_>>());
        assert!(tabs.windows(2).any(|pair| pair[1].rect.y > pair[0].rect.y), "on more than one line");
    }

    struct Splitting {
        side: f32,
        named: bool,
    }

    fn splitting(app: &mut Splitting, ui: &mut Ui) {
        ui.row().grow().open();
        ui.column().w(Length::Px(app.side)).open();
        if app.named {
            ui.label("Folders");
        }
        ui.close();
        ui.splitter(&mut app.side, 60.0, 200.0);
        ui.button("Beside");
        ui.close();
    }

    #[test]
    fn a_splitter_is_reached_and_moved_with_keys_and_told_as_a_slider() {
        let mut trial = Trial::new(Splitting { side: 100.0, named: true }, splitting);
        trial.key("Tab");
        let told = trial.find(Role::Slider, "Folders").clone();
        assert!(told.focused && told.value_range == Some([100.0, 60.0, 200.0]) && told.step == 10.0, "{told:?}");
        for (key, to) in [("Right", 110.0), ("Left", 100.0), ("End", 200.0), ("Right", 200.0), ("Home", 60.0)] {
            trial.key(key);
            assert_eq!(trial.app.side, to, "{key}");
        }
        // A pane without text keeps the name the app's text gives it.
        trial.app.named = false;
        trial.run();
        assert!(trial.has(Role::Slider, trial.ui.frame_words.splitter));
    }

    #[derive(Default)]
    struct Form {
        volume: f32,
        size: usize,
        copies: i32,
        town: String,
        low: f32,
        high: f32,
        loose: f32,
    }

    fn form(app: &mut Form, ui: &mut Ui) {
        ui.column().pad(8.0).gap(4.0).open();
        ui.label("Volume");
        ui.slider(&mut app.volume, 0.0..=10.0);
        ui.label("Size");
        ui.select(&mut app.size, &["Small", "Large"]);
        ui.label("Copies");
        ui.number(&mut app.copies, 1);
        ui.label("Town");
        ui.text_box(&mut app.town, "");
        ui.label("Price");
        ui.row().open();
        ui.range(&mut app.low, &mut app.high, 0.0..=10.0);
        ui.close();
        ui.button("Words, not a label");
        ui.slider(&mut app.loose, 0.0..=1.0);
        ui.close();
    }

    #[test]
    fn a_part_with_no_words_of_its_own_is_called_by_the_label_in_front_of_it() {
        let trial = Trial::new(Form { copies: 3, ..Form::default() }, form);
        let names: Vec<(Role, &str)> = trial.visible_access_nodes().iter().filter(|said| said.role != Role::Text).map(|said| (said.role, said.name.as_str())).collect();
        let wanted = [
            (Role::Slider, "Volume"),
            (Role::ComboBox, "Size"),
            (Role::SpinButton, "Copies"),
            (Role::TextBox, "Town"),
            (Role::Slider, "Price, low end"),
            (Role::Slider, "Price, high end"),
            (Role::Button, "Words, not a label"),
            (Role::Slider, ""),
        ];
        assert_eq!(names, wanted, "(a button's words are no label: the slider after it has no name)");
        // (What parts say of themselves stays: what is selected, how many.)
        assert_eq!(trial.find(Role::ComboBox, "Size").value, "Small");
        assert_eq!(trial.find(Role::SpinButton, "Copies").value, "3");
    }

    struct Long {
        on: bool,
    }

    fn long(app: &mut Long, ui: &mut Ui) {
        ui.row().gap(4.0).open();
        ui.column().w(Length::Px(40.0)).open();
        ui.label("Words much wider than their column");
        ui.close();
        ui.column().open();
        ui.toggle(&mut app.on, "On");
        ui.close();
        ui.close();
    }

    #[test]
    fn words_wider_than_their_box_are_drawn_again_with_what_they_spill_over() {
        let mut trial = Trial::new(Long { on: false }, long);
        // (The trial compares the incrementally redrawn window with a full fresh draw.)
        trial.press(Role::Button, "On");
        trial.press(Role::Button, "On");
        assert!(!trial.app.on);
    }

    struct Rows;

    fn rows(_: &mut Rows, ui: &mut Ui) {
        ui.column().grow().open();
        for row in ui.list(100, 20.0) {
            ui.selectable(false, &format!("Row {row}"));
        }
        ui.close();
        ui.close();
    }

    #[test]
    fn rows_are_never_lower_than_a_line_of_the_themes_text() {
        let mut trial = Trial::new(Rows, rows);
        assert_eq!(trial.find(Role::Row, "Row 1").rect.h, 20.0);
        trial.ui.theme = trial.ui.theme.text_scaled(2.5);
        trial.run();
        assert_eq!(trial.find(Role::Row, "Row 1").rect.h, 40.0);
    }
}
