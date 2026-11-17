//! Dates: a `Date` value, a date picker, and a date range picker.
//!
//! ```ignore
//! // in the app's state:  due: Date::today()
//! if ui.date(&mut app.due).changed {
//!     plan_again();
//! }
//! ui.date_range(&mut app.away, &mut app.back); // the first day pressed, then the last
//! ```
//!
//! Dates are written in the user's format ([`ui.locale`](Ui::locale)), and weeks start on the
//! user's first day. In the calendar, focus starts on the picked day: arrows move a day or a
//! week, Page Up/Down a month (with Shift, a year), Home/End to the week's start and end, Enter
//! picks.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::geom::Rect;
use crate::id::Id;
use crate::layout::Align;
use crate::ui::{Icon, Response, Ui};
use crate::widgets::{Before, DOWN, RIGHT};
#[cfg(unix)]
use std::ffi::{c_int, c_long};
use std::fmt::Write;

const LEFT: Icon = Icon::line("M9.5 4 5.5 8 9.5 12");
/// Day numbers, written out once.
const NUMBERS: [&str; 31] =
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31"];
/// Minimum width and height of a calendar day (more with larger text).
const DAY: [f32; 2] = [30.0, 26.0];

/// A calendar day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// The year.
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// Day of the month, from 1.
    pub day: u8,
}

impl Date {
    /// A date from year, month (1 to 12) and day (from 1).
    pub const fn new(year: i32, month: u8, day: u8) -> Self {
        Date { year, month, day }
    }

    /// Days since 1970-01-01; negative before it.
    pub fn days(self) -> i64 {
        // Years counted from March, so the leap day comes last, in 400-year cycles after which
        // the calendar repeats. (Howard Hinnant's algorithm.)
        let year = self.year as i64 - (self.month <= 2) as i64;
        let (era, month) = (year.div_euclid(400), (self.month as i64 + 9) % 12);
        let of_year = (153 * month + 2) / 5 + self.day as i64 - 1;
        let of_era = year.rem_euclid(400) * 365 + year.rem_euclid(400) / 4 - year.rem_euclid(400) / 100 + of_year;
        era * 146_097 + of_era - 719_468
    }
    /// The date `days` after 1970-01-01. Beyond the representable years, the first or last.
    pub fn from_days(days: i64) -> Self {
        let shifted = days.saturating_add(719_468);
        let (era, of_era) = (shifted.div_euclid(146_097), shifted.rem_euclid(146_097));
        let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
        let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month = (5 * of_year + 2) / 153;
        let (day, month) = (of_year - (153 * month + 2) / 5 + 1, (month + 2) % 12 + 1);
        let year = (year_of_era + era * 400 + (month <= 2) as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Date { year, month: month as u8, day: day as u8 }
    }
    /// Day of the week: 0 Monday to 6 Sunday.
    pub fn weekday(self) -> u8 {
        (self.days() + 3).rem_euclid(7) as u8
    }
    /// Days in its month.
    pub fn month_length(self) -> u8 {
        let leap = self.year % 4 == 0 && (self.year % 100 != 0 || self.year % 400 == 0);
        match self.month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }
    /// The same day `months` later (earlier, if negative); the month's last day if it has
    /// fewer.
    pub fn add_months(self, months: i32) -> Self {
        let counted = self.year as i64 * 12 + self.month as i64 - 1 + months as i64;
        let year = counted.div_euclid(12).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let first = Date::new(year, counted.rem_euclid(12) as u8 + 1, 1);
        Date { day: self.day.min(first.month_length()), ..first }
    }
    /// Today, in local time.
    pub fn today() -> Self {
        let [year, month, day, ..] = local_now();
        Date::new(year, month as u8, day as u8)
    }
}

/// 1970-01-01.
impl Default for Date {
    fn default() -> Self {
        Date::new(1970, 1, 1)
    }
}

/// ISO 8601, which sorts: `2026-11-30`. For the user's format, see
/// [`ui.format_date(date)`](Ui::format_date).
impl std::fmt::Display for Date {
    fn fmt(&self, to: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(to, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Local time now: year, month, day, hour, minute, second.
#[cfg(windows)]
pub(crate) fn local_now() -> [i32; 6] {
    crate::windows::local_time().map(i32::from)
}

/// C's `struct tm` on every Unix: `int` fields, and the UTC offset as a `long`.
#[cfg(unix)]
#[repr(C)]
struct Moment {
    second: c_int,
    minute: c_int,
    hour: c_int,
    day: c_int,
    month: c_int,
    year: c_int,
    rest: [c_int; 3],
    ahead: c_long,
    zone: *const u8,
}

/// C's `time_t`: seconds since 1970, a `long`, as these functions take it on 32 and 64 bits.
#[cfg(unix)]
type Seconds = c_long;

#[cfg(unix)]
unsafe extern "C" {
    fn time(into: *mut Seconds) -> Seconds;
    fn localtime_r(time: *const Seconds, into: *mut Moment) -> *mut Moment;
}

#[cfg(unix)]
pub(crate) fn local_now() -> [i32; 6] {
    let mut moment = Moment { second: 0, minute: 0, hour: 0, day: 1, month: 0, year: 70, rest: [0; 3], ahead: 0, zone: std::ptr::null() };
    // SAFETY: the C library fills in the struct it is given; both pointers outlive the calls.
    unsafe { localtime_r(&time(std::ptr::null_mut()), &mut moment) };
    [moment.year + 1900, moment.month + 1, moment.day, moment.hour, moment.minute, moment.second]
}

/// Without a system to ask, UTC.
#[cfg(not(any(windows, unix)))]
pub(crate) fn local_now() -> [i32; 6] {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let (day, of_day) = (Date::from_days((since / 86_400) as i64), (since % 86_400) as i32);
    [day.year, day.month as i32, day.day as i32, of_day / 3600, of_day / 60 % 60, of_day % 60]
}

/// The local UTC offset at `at` (seconds since 1970, UTC), in seconds: added to a UTC time to
/// get local time.
pub(crate) fn ahead_of_greenwich(at: i64) -> i64 {
    ahead(at)
}

#[cfg(unix)]
fn ahead(at: i64) -> i64 {
    let mut moment = Moment { second: 0, minute: 0, hour: 0, day: 1, month: 0, year: 70, rest: [0; 3], ahead: 0, zone: std::ptr::null() };
    // SAFETY: the C library fills in the struct it is given; both pointers outlive the call.
    unsafe { localtime_r(&at, &mut moment) };
    moment.ahead
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(time: *mut [u16; 8]);
}

/// From Windows' local and UTC time now; taken to hold at any moment.
#[cfg(windows)]
fn ahead(_at: i64) -> i64 {
    let mut time = [0u16; 8];
    // SAFETY: the system fills in the struct it is given.
    unsafe { GetLocalTime(&mut time) };
    let here = Date::new(time[0] as i32, time[1] as u8, time[3] as u8).days() * 86_400 + time[4] as i64 * 3600 + time[5] as i64 * 60 + time[6] as i64;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64);
    // The two clocks were read a moment apart: zones differ by whole quarter hours.
    ((here - now) as f64 / 900.0).round() as i64 * 900
}

#[cfg(not(any(windows, unix)))]
fn ahead(_at: i64) -> i64 {
    0
}

/// Calendar and clock words: English until the app sets its own in `ui.calendar_words`. (Date
/// and time formats and the week's first day come from the system: see
/// [`ui.locale`](Ui::locale).)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalendarWords {
    /// The names of the twelve months.
    pub months: [&'static str; 12],
    /// Short weekday names, Monday first.
    pub days: [&'static str; 7],
    /// AM and PM.
    pub halves: [&'static str; 2],
    /// Screen reader names of a time's fields: hours, minutes, seconds, AM/PM.
    pub clock: [&'static str; 4],
    /// Screen reader names of the previous and next month arrows.
    pub arrows: [&'static str; 2],
}

impl Default for CalendarWords {
    fn default() -> Self {
        CalendarWords {
            months: ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
            days: ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"],
            halves: ["AM", "PM"],
            clock: ["Hours", "Minutes", "Seconds", "AM or PM"],
            arrows: ["The month before", "The month after"],
        }
    }
}

/// The open calendar: its box, the month shown, the first end of a range being picked, and the
/// day under the pointer or focus last run, to which a half-picked range is shown.
#[derive(Clone, Copy, Default)]
pub(crate) struct Shown {
    of: Id,
    month: (i32, u8),
    first: Option<Date>,
    pointed: Option<Date>,
}

/// Date and date range pickers.
impl Ui {
    /// A date box that opens a calendar to pick another. `changed` is set when the user picks.
    pub fn date(&mut self, date: &mut Date) -> Response {
        let mut written = std::mem::take(&mut self.scratch);
        written.clear();
        self.locale_in_use().write_date(&mut written, *date);
        let mut state = self.opens_calendar(&written);
        self.scratch = written;
        if let Some(day) = self.calendar(state.id, [*date; 2]) {
            (state.changed, *date) = (day != *date, day);
            self.close_popup();
        }
        state
    }

    /// A date range box, `from` to `to`, that opens a calendar: the first day pressed is one
    /// end, the second the other (the earlier is `from`). Until the second, the range is shown
    /// up to the day under the pointer or focus, and nothing changes. `changed` is set when a
    /// range is picked.
    pub fn date_range(&mut self, from: &mut Date, to: &mut Date) -> Response {
        let (mut written, locale) = (std::mem::take(&mut self.scratch), self.locale_in_use());
        written.clear();
        locale.write_date(&mut written, *from);
        written.push_str(" \u{2013} ");
        locale.write_date(&mut written, *to);
        let mut state = self.opens_calendar(&written);
        self.scratch = written;
        let Some(day) = self.calendar(state.id, [*from, *to]) else { return state };
        let Some(first) = self.calendar_open.first else {
            self.calendar_open.first = Some(day);
            return state;
        };
        state.changed = (first.min(day), first.max(day)) != (*from, *to);
        (*from, *to) = (first.min(day), first.max(day));
        self.close_popup();
        state
    }

    /// The box showing the pick, `written`; pressed, it opens the calendar. Screen readers get
    /// a button named by its text, with a popup, expanded or not.
    fn opens_calendar(&mut self, written: &str) -> Response {
        let state = self.button_like(Before::None, written, Some(DOWN), false);
        if state.clicked {
            self.open_calendar(state.id, state.rect);
        }
        let open = self.popups.iter().any(|popup| popup.id == state.id);
        self.tell(&state, Role::Fold, Some(open), "");
        state
    }

    /// Opens the calendar of box `of`, below `under`: at the picked month, nothing half picked.
    pub(crate) fn open_calendar(&mut self, of: Id, under: Rect) {
        self.open_popup(of, under);
        self.calendar_open.of = 0;
    }

    /// The calendar under a box while open: month name, previous and next arrows, and the days
    /// in week rows from the user's first weekday. `picked` are the first and last picked days
    /// (the same, for one). Returns the day pressed, if any.
    pub(crate) fn calendar(&mut self, of: Id, picked: [Date; 2]) -> Option<Date> {
        if !self.popup(of) {
            return None;
        }
        let t = self.theme;
        let fresh = self.calendar_open.of != of;
        if fresh {
            self.calendar_open = Shown { of, month: (picked[0].year, picked[0].month), ..Shown::default() };
        }
        // Never larger than the window, either way: what doesn't fit scrolls.
        let [wide, high] = self.popup_most();
        self.column().pad(8.0).gap(6.0).max(wide, high).scroll().scroll_x().fill(t.raised).border(t.line_strong, 1.0).radius(8.0).shadow(16.0).sense().open();
        let first = self.calendar_month_row();
        self.grid(7).gap(2.0).open();
        // On opening, focus goes to the picked day, and keys move between days. Handled here,
        // where the days' ids are known and before them, so no other box gets the keys first.
        if fresh {
            self.set_focus(self.id(picked[0].days()));
        }
        self.calendar_keys(first);
        let size = self.day_size();
        self.calendar_weekdays(first, size);
        let pressed = self.calendar_days(first, picked, size);
        self.close();
        self.close();
        pressed
    }

    /// The calendar's top row: the previous-month arrow, the month and year, the next-month
    /// arrow. Returns the first day of the month shown, after the arrows.
    fn calendar_month_row(&mut self) -> Date {
        let (t, words, (year, month)) = (self.theme, self.calendar_words, self.calendar_open.month);
        self.row().gap(6.0).open();
        let back = self.button_like(Before::Icon(LEFT), "", None, false);
        self.tell(&back, Role::Button, None, words.arrows[0]);
        self.spacer();
        self.text(words.months[(month as usize + 11) % 12], t.font.bold(), t.text);
        self.year_label(year);
        self.spacer();
        let on = self.button_like(Before::Icon(RIGHT), "", None, false);
        self.tell(&on, Role::Button, None, words.arrows[1]);
        self.close();
        let (back, on) = (back.clicked, on.clicked);
        let first = Date::new(year, month, 1).add_months(on as i32 - back as i32);
        self.calendar_open.month = (first.year, first.month);
        self.again |= on || back;
        first
    }

    /// The weekday names, from the user's first weekday; then blanks for the weekdays before
    /// the 1st of `first`'s month.
    fn calendar_weekdays(&mut self, first: Date, size: [f32; 2]) {
        let (t, words, first_day) = (self.theme, self.calendar_words, self.locale_in_use().first_weekday);
        let heading = 18f32.max(self.text.metrics(t.small_font, self.scale).height().ceil() + 2.0);
        for day in 0..7 {
            self.el().size(size[0], heading).text(words.days[(day + first_day as usize) % 7], t.small_font, t.text_faint).text_align(Align::Center).show();
        }
        for _ in 0..(first.weekday() + 7 - first_day % 7) % 7 {
            self.el().size(size[0], size[1]).show();
        }
    }

    /// The days of `first`'s month, `picked` marked. Returns the day pressed, if any.
    fn calendar_days(&mut self, first: Date, picked: [Date; 2], size: [f32; 2]) -> Option<Date> {
        // A half-picked range shows up to the day pointed at.
        let shown = match (self.calendar_open.first, self.calendar_open.pointed) {
            (Some(start), Some(end)) => [start.min(end), start.max(end)],
            (Some(start), None) => [start; 2],
            (None, _) => picked,
        };
        let (today, mut pressed, mut hovered, mut keyed) = (Date::today(), None, None, None);
        for day in (1..=first.month_length()).map(|day| Date::new(first.year, first.month, day)) {
            let state = self.calendar_day(day, shown, today, size);
            pressed = pressed.or(state.clicked.then_some(day));
            (hovered, keyed) = (hovered.or(state.hovered.then_some(day)), keyed.or(state.focused.then_some(day)));
        }
        // The pointer's day if over one; else focus. A half-picked range is redrawn up to it.
        let pointed = hovered.or(keyed);
        let half_picked = self.calendar_open.first.is_some();
        self.again |= half_picked && std::mem::replace(&mut self.calendar_open.pointed, pointed) != pointed;
        pressed
    }

    /// Calendar keys while focus is on a day of the month starting `first`: move focus, and the
    /// month shown.
    fn calendar_keys(&mut self, first: Date) {
        let mut days = (0..first.month_length() as i64).map(|day| Date::from_days(first.days() + day));
        let Some(on) = days.find(|day| self.id(day.days()) == self.focus) else { return };
        // Home and End go to the user's week start and end.
        let into_week = (on.weekday() as i64 - self.locale_in_use().first_weekday as i64).rem_euclid(7);
        let mut key = |key: Key, mods: Mods| self.key(key, mods) as i64;
        let along = key(Key::Right, Mods::NONE) - key(Key::Left, Mods::NONE) + 7 * (key(Key::Down, Mods::NONE) - key(Key::Up, Mods::NONE));
        let months = key(Key::PageDown, Mods::NONE) - key(Key::PageUp, Mods::NONE) + 12 * (key(Key::PageDown, Mods::SHIFT) - key(Key::PageUp, Mods::SHIFT));
        let ends = (6 - into_week) * key(Key::End, Mods::NONE) - into_week * key(Key::Home, Mods::NONE);
        let to = Date::from_days(on.add_months(months as i32).days() + along + ends);
        if to != on {
            self.calendar_open.month = (to.year, to.month);
            self.set_focus(self.id(to.days()));
        }
    }

    /// The height of a one-line part: the theme's, or the text's, if larger text needs more.
    pub(crate) fn line_part(&mut self) -> f32 {
        self.theme.control_height.max(self.text.metrics(self.theme.font, self.scale).height().ceil() + 8.0)
    }
    /// A calendar day's size: room for "30".
    fn day_size(&mut self) -> [f32; 2] {
        let (font, wide) = (self.theme.font, self.text_width("30", self.theme.font));
        [DAY[0].max(wide.ceil() + 10.0), DAY[1].max(self.text.metrics(font, self.scale).height().ceil() + 6.0)]
    }

    /// The year, beside the month's name.
    fn year_label(&mut self, year: i32) {
        let (t, mut written) = (self.theme, std::mem::take(&mut self.scratch));
        written.clear();
        let _ = write!(written, "{year}");
        self.text(&written, t.font.bold(), t.text_dim);
        self.scratch = written;
    }

    /// One calendar day: filled at the ends of the pick, tinted between them, ringed if today.
    /// Screen readers get a selectable row named in full ("11 November 2026"), its day number,
    /// and whether it is picked.
    fn calendar_day(&mut self, day: Date, [start, end]: [Date; 2], today: Date, [wide, high]: [f32; 2]) -> Response {
        let t = self.theme;
        let state = self.press(self.id(day.days()));
        let (ends, within) = (day == start || day == end, start < day && day < end);
        let (fill, ink) = match (ends, within, state.hovered || state.focused) {
            (true, ..) => (t.accent, t.on_accent),
            (false, _, true) => (t.hover, t.text),
            (false, true, false) => (t.selected, t.text),
            (false, false, false) => (Color::NONE, t.text),
        };
        // On a day filled with the accent, the focus ring takes the number's colour, which
        // shows on it.
        let edge = match (ends && state.focused, day == today) {
            (true, _) => t.on_accent,
            (false, true) => t.accent,
            (false, false) => Color::NONE,
        };
        let cell = self.el().key(day.days()).size(wide, high).text(NUMBERS[day.day as usize - 1], t.font, ink).text_align(Align::Center);
        cell.fill(fill).border(edge, 1.0).radius(5.0).sense().focusable().show();
        if self.listening {
            let name = format!("{} {} {}", day.day, self.calendar_words.months[day.month as usize - 1], day.year);
            self.tell(&state, Role::Row, Some(ends || within), &name);
            self.tell_position(day.day as usize, day.month_length() as usize);
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_counted_and_named_right() {
        assert_eq!(Date::new(1970, 1, 1).days(), 0);
        assert_eq!(Date::new(1970, 1, 1).weekday(), 3, "a Thursday");
        assert_eq!(Date::new(2026, 11, 11).weekday(), 2, "a Wednesday");
        assert_eq!(Date::new(2000, 2, 29).days() + 1, Date::new(2000, 3, 1).days(), "2000 was a leap year");
        assert_eq!([Date::new(2024, 2, 1).month_length(), Date::new(2023, 2, 1).month_length(), Date::new(1900, 2, 1).month_length()], [29, 28, 28]);
        assert_eq!([Date::new(2026, 12, 5).month_length(), Date::new(2026, 4, 30).month_length()], [31, 30]);
        assert_eq!(Date::new(2026, 3, 7).to_string(), "2026-03-07");
        assert!(Date::new(2026, 3, 7) < Date::new(2026, 11, 1));
    }

    #[test]
    fn a_month_on_or_back_keeps_the_day_where_the_month_has_it() {
        let last = Date::new(2026, 1, 31);
        assert_eq!(
            [last.add_months(1), last.add_months(2), last.add_months(-2), last.add_months(13)],
            [Date::new(2026, 2, 28), Date::new(2026, 3, 31), Date::new(2025, 11, 30), Date::new(2027, 2, 28)]
        );
        assert_eq!(Date::new(2024, 2, 29).add_months(-12), Date::new(2023, 2, 28));
        assert_eq!(Date::new(2026, 11, 13).add_months(0), Date::new(2026, 11, 13));
    }

    #[test]
    fn every_day_for_eight_hundred_years_comes_back_as_itself() {
        let mut day = Date::new(1600, 1, 1);
        for days in day.days()..Date::new(2400, 1, 1).days() {
            assert_eq!(Date::from_days(days), day);
            assert_eq!(day.days(), days);
            let last = day.day == day.month_length();
            day = match (last, day.month) {
                (false, _) => Date::new(day.year, day.month, day.day + 1),
                (true, 12) => Date::new(day.year + 1, 1, 1),
                (true, _) => Date::new(day.year, day.month + 1, 1),
            };
        }
    }
}
