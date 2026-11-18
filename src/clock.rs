//! Times of day: a `Time` value, a time box, and a date-and-time box.
//!
//! ```ignore
//! ui.time(&mut app.alarm, false); // 07:30, or 7:30 AM on a 12-hour clock
//! ui.date_time(&mut app.day, &mut app.alarm);
//! ```
//!
//! A time box has a field each for hours, minutes, seconds (if asked) and AM/PM (on a 12-hour
//! clock). Tab and Left/Right move between fields. A field takes Up/Down (minutes and seconds
//! by ten with Shift, or Page Up/Down), Home/End, the wheel, and typed digits; a full field
//! moves on to the next. Screen readers get each field as a spin button in a group named by the
//! whole time.
use crate::accessibility::Role;
use crate::color::Color;
use crate::date::Date;
use crate::event::{Key, Mods};
use crate::id::Id;
use crate::layout::{Align, Length};
use crate::ui::{Response, Stroke, Ui};
use crate::widgets::DOWN;
use std::fmt::Write;

/// A time of day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time {
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
    /// 0 to 59.
    pub second: u8,
}

impl Time {
    /// A time from hour (0 to 23), minute and second.
    pub const fn new(hour: u8, minute: u8, second: u8) -> Self {
        Time { hour, minute, second }
    }
    /// Seconds since midnight.
    pub fn seconds(self) -> u32 {
        self.hour as u32 * 3600 + self.minute as u32 * 60 + self.second as u32
    }
    /// The time `seconds` after midnight, wrapping each day.
    pub fn from_seconds(seconds: u32) -> Self {
        let seconds = seconds % 86_400;
        Time::new((seconds / 3600) as u8, (seconds / 60 % 60) as u8, (seconds % 60) as u8)
    }
    /// The local time now.
    pub fn now() -> Self {
        let [.., hour, minute, second] = crate::date::local_now();
        Time::new(hour as u8, minute as u8, second as u8)
    }
}

/// Sortable 24-hour form: `14:05:09`. For the user's own format, see
/// [`ui.format_time(time, seconds)`](Ui::format_time).
impl std::fmt::Display for Time {
    fn fmt(&self, to: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(to, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }
}

/// The fields of a time, in order: hours, minutes, seconds, AM/PM.
const HOURS: usize = 0;
const MINUTES: usize = 1;
const SECONDS: usize = 2;
const HALF: usize = 3;

/// Times of day, and days with a time.
impl Ui {
    /// A time in the window's locale: "14:05", or "2:05 PM" on a 12-hour clock; with seconds if
    /// asked.
    pub fn format_time(&self, time: Time, seconds: bool) -> String {
        let mut written = String::new();
        self.write_time(&mut written, time, seconds);
        written
    }
    fn write_time(&self, into: &mut String, time: Time, seconds: bool) {
        let formats = self.formats();
        let (twelve, mark) = (formats.locale.twelve_hour_clock, formats.fine.time_separator);
        formats.write_hour(into, if twelve { (time.hour % 12 + 11) % 12 + 1 } else { time.hour });
        let _ = write!(into, "{mark}{:02}", time.minute);
        if seconds {
            let _ = write!(into, "{mark}{:02}", time.second);
        }
        if twelve {
            let _ = write!(into, " {}", self.calendar_words_in_use().halves[(time.hour / 12).min(1) as usize]);
        }
    }

    /// A time box: hours and minutes, and seconds if `seconds`. `changed` is set when the user
    /// changes it.
    pub fn time(&mut self, time: &mut Time, seconds: bool) -> Response {
        let was = *time;
        let mut state = self.time_frame(*time, seconds);
        let first = self.time_parts(time, seconds);
        self.close();
        // A click beside the fields focuses the first.
        if state.clicked {
            self.set_focus(first);
        }
        state.changed = *time != was;
        self.again |= state.changed;
        state
    }

    /// A date and time in one box: the date opens a calendar, as [`date`](Ui::date) does; hours
    /// and minutes work as in [`time`](Ui::time). `changed` is set when either changes.
    pub fn date_time(&mut self, date: &mut Date, time: &mut Time) -> Response {
        let (t, was) = (self.theme, (*date, *time));
        let mut state = self.time_frame(*time, false);
        // The date: a field like the others, that opens the calendar.
        let mut written = std::mem::take(&mut self.scratch);
        written.clear();
        self.formats().write_date(&mut written, *date);
        let opener = self.press(self.id("date"));
        if opener.clicked {
            self.open_calendar(opener.id, state.rect);
        }
        let open = self.popups.iter().any(|popup| popup.id == opener.id);
        let fill = if opener.hovered || opener.focused || open { t.hover } else { Color::NONE };
        let high = self.line_part();
        self.row().key("date").h(Length::Px(high - 8.0)).pad((4.0, 0.0)).gap(4.0).align(Align::Start, Align::Center).fill(fill).radius(3.0).sense().focusable().open();
        self.text(&written, t.font, t.text);
        self.el().icon(DOWN, 12.0, t.text_dim).show();
        self.close();
        self.tell(&opener, Role::Fold, Some(open), "");
        self.scratch = written;
        self.el().w(Length::Px(1.0)).h(Length::Px(high - 12.0)).fill(t.line).show();
        self.el().w(Length::Px(4.0)).show();
        self.time_parts(time, false);
        self.close();
        if let Some(day) = self.calendar(opener.id, [*date; 2]) {
            *date = day;
            self.close_popup();
        }
        if state.clicked {
            self.set_focus(opener.id);
        }
        state.changed = (*date, *time) != was;
        self.again |= state.changed;
        state
    }

    /// Opens a time box's frame for its fields: a group named by the time.
    fn time_frame(&mut self, time: Time, seconds: bool) -> Response {
        let t = self.theme;
        let state = self.interact(self.next_id());
        let edge = if state.hovered { t.text_faint } else { t.line_strong };
        let high = self.line_part();
        self.row().h(Length::Px(high)).pad((4.0, 0.0)).gap(0.0).align(Align::Start, Align::Center).fill(t.panel).border(edge, 1.0).radius(t.radius).sense().open();
        if self.listening {
            let name = self.format_time(time, seconds);
            self.tell(&state, Role::Group, None, &name);
        }
        state
    }

    /// The time's fields side by side in the open box; returns the first one's id.
    fn time_parts(&mut self, time: &mut Time, seconds: bool) -> Id {
        let twelve = self.locale_in_use().twelve_hour_clock;
        let last = if twelve {
            HALF
        } else if seconds {
            SECONDS
        } else {
            MINUTES
        };
        // On a 12-hour clock hours run 12 to 11, and AM/PM follows.
        let (hour, half) = (time.hour % 24, time.hour % 24 / 12);
        let (shown, hours) = if twelve { ((hour + 11) % 12 + 1, [1, 12]) } else { (hour, [0, 23]) };
        let (first, mut hour) = self.time_part(HOURS, shown, hours, last);
        self.time_mark();
        let minute = self.time_part(MINUTES, time.minute, [0, 59], last).1;
        let second = match seconds {
            true => self.time_mark().time_part(SECONDS, time.second, [0, 59], last).1,
            false => time.second,
        };
        if twelve {
            self.el().w(Length::Px(4.0)).show();
            let half = self.time_part(HALF, half, [0, 1], last).1;
            hour = hour % 12 + 12 * half;
        }
        *time = Time::new(hour, minute, second);
        first.id
    }

    /// The separator between fields.
    fn time_mark(&mut self) -> &mut Self {
        let (t, mut mark) = (self.theme, [0; 4]);
        let mark = self.formats().fine.time_separator.encode_utf8(&mut mark);
        self.el().text(mark, t.font, t.text_dim).show();
        self
    }

    /// One field: a number from `least` to `most` that wraps, two digits; or the AM/PM word.
    /// Returns its response and new value.
    fn time_part(&mut self, which: usize, value: u8, [least, most]: [u8; 2], last: usize) -> (Response, u8) {
        let (t, words) = (self.theme, self.calendar_words_in_use());
        let state = self.interact(self.id(which));
        let mut to = value as i32;
        if state.focused {
            to = self.time_part_keys(&state, which, to, [least, most], which == last);
        } else if self.entry.0 == state.id {
            // Digits typed before focus left are forgotten.
            self.entry.0 = 0;
        }
        if let Some((_, set)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            to = set.round() as i32;
        }
        let to = (least as i32 + (to - least as i32).rem_euclid((most - least) as i32 + 1)) as u8;
        let mut written = std::mem::take(&mut self.scratch);
        written.clear();
        let _ = match which {
            HALF => write!(written, "{}", words.halves[to as usize]),
            HOURS => {
                self.formats().write_hour(&mut written, to);
                Ok(())
            }
            _ => write!(written, "{to:02}"),
        };
        let (fill, ink) = match (state.focused, state.hovered) {
            (true, _) => (t.accent, t.on_accent),
            (false, true) => (t.hover, t.text),
            (false, false) => (Color::NONE, t.text),
        };
        let high = self.line_part();
        let part = self.el().key(which).h(Length::Px(high - 8.0)).pad((2.0, 0.0)).text(&written, t.font, ink).text_align(Align::Center);
        part.fill(fill).radius(3.0).sense().focusable().show();
        self.keep_note(&state, Role::SpinButton, None, words.clock[which], &written);
        self.retell_as_value(&state, Role::SpinButton, [to as f32, least as f32, most as f32]);
        self.tell_step(1.0);
        self.scratch = written;
        (state, to)
    }

    /// Applies keys, wheel and typed text to the focused field `which` at `value`. The result
    /// may be out of range: it wraps.
    fn time_part_keys(&mut self, state: &Response, which: usize, value: i32, [least, most]: [u8; 2], last: bool) -> i32 {
        let large = if matches!(which, MINUTES | SECONDS) { 10 } else { 1 };
        let mut key = |key: Key, mods: Mods| self.key(key, mods) as i32;
        let steps = key(Key::Up, Mods::NONE) - key(Key::Down, Mods::NONE);
        let large_steps = key(Key::Up, Mods::SHIFT) - key(Key::Down, Mods::SHIFT) + key(Key::PageUp, Mods::NONE) - key(Key::PageDown, Mods::NONE);
        let [home, end, cleared] = [key(Key::Home, Mods::NONE), key(Key::End, Mods::NONE), key(Key::Backspace, Mods::NONE) + key(Key::Delete, Mods::NONE)];
        let [wheel, large_wheel] = self.wheel_steps(state);
        let mut to = match (home, end, cleared) {
            (1, ..) | (_, _, 1..) => least as i32,
            (_, 1, _) => most as i32,
            _ => value + steps + wheel + large * (large_steps + large_wheel),
        };
        if cleared > 0 {
            self.entry.1.clear();
        }
        let typed = self.typed();
        for (at, letter) in typed.char_indices() {
            let moved;
            (to, moved) = self.time_typed(state.id, which, letter, to, [least, most], last);
            if moved {
                // The rest of the typed text goes to the next field.
                self.input.strokes.push(Stroke::Text(typed[at + letter.len_utf8()..].into()));
                break;
            }
        }
        to
    }

    /// A character typed into a field. A digit is appended to those typed before; a full field
    /// moves focus to the next (not from the `last`). In AM/PM, the first letter of either word
    /// picks it. Returns the new value, and whether focus moved on.
    fn time_typed(&mut self, id: Id, which: usize, letter: char, value: i32, [least, most]: [u8; 2], last: bool) -> (i32, bool) {
        let lower = letter.to_lowercase().next();
        if which == HALF {
            let picked = self.calendar_words_in_use().halves.iter().position(|word| word.chars().next().and_then(|first| first.to_lowercase().next()) == lower);
            return (picked.map_or(value, |half| half as i32), false);
        }
        let Some(digit) = letter.to_digit(10) else { return (value, false) };
        if self.entry.0 != id {
            (self.entry.0, self.entry.1) = (id, String::new());
        }
        self.entry.1.push(char::from_digit(digit, 10).unwrap_or('0'));
        let typed: i32 = self.entry.1.parse().unwrap_or(0);
        // Full: two digits, or one no second could follow (7 minutes, 3 hours).
        let full = self.entry.1.len() >= 2 || typed * 10 > most as i32;
        if full {
            self.entry.1.clear();
        }
        if full && !last {
            self.move_focus(true);
        }
        (typed.clamp(least as i32, most as i32), full && !last)
    }
}
