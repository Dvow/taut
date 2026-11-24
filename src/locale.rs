//! The user's date, time and number formats (date order, separators and digit counts; first
//! day of the week; 12 or 24-hour clock, its separator and hour digits; decimal and thousands
//! separators and group sizes), and the calendar's month and day names and AM/PM.
//!
//! Read when first needed: on Windows from the user's region settings, read again when Windows
//! reports a change; on Linux from the locale the environment names (`LC_ALL`, else `LC_TIME`
//! for dates and `LC_NUMERIC` for numbers, else `LANG`), through glibc's locale data. With none
//! named, or C's ("C", "POSIX", "C.UTF-8"), and on other systems: [`Locale::PLAIN`]. An app can
//! set its own in [`ui.locale`](Ui::locale).
//!
//! Group sizes, digit counts, the time separator and hour digits are not fields of [`Locale`]:
//! they are the system's while `ui.locale` is `None`; with an app's own locale, plain (groups
//! of three, two-digit days, months and hours, four-digit years, ":"). Not read: non-ASCII
//! digits.
use crate::date::{CalendarWords, Date};
use crate::number::Number;
use crate::ui::Ui;
use std::fmt::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

/// The order of a date's day, month and year.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    /// Year first, which sorts: 2026-11-30.
    YearMonthDay,
    /// Day first: 30.11.2026.
    DayMonthYear,
    /// Month first: 11/30/2026.
    MonthDayYear,
}

/// Date, time and number formats. [`Locale::of_the_system()`](Locale::of_the_system) is the
/// user's; [`Locale::PLAIN`] the library's: 2026-11-30, 14:05, 1234.5, weeks from Monday.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locale {
    /// Date order.
    pub date_order: DateOrder,
    /// Date separator: '-', '.' or '/'.
    pub date_separator: char,
    /// First day of the week: 0 Monday to 6 Sunday.
    pub first_weekday: u8,
    /// A 12-hour clock, from 12 to 11 with AM/PM after it (`ui.calendar_words.halves`), instead
    /// of 0 to 23.
    pub twelve_hour_clock: bool,
    /// The thousands separator in numbers written for reading ("1,234,567"); `None` if none.
    /// (Text boxes write none.)
    pub thousands_separator: Option<char>,
    /// The decimal separator: '.' or ','.
    pub decimal_point: char,
}

impl Default for Locale {
    fn default() -> Self {
        Locale::PLAIN
    }
}

/// The finer points of a locale's formats, kept beside a [`Locale`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fine {
    /// Digits in the group nearest the decimal separator, and in each group before it (0: no
    /// more groups).
    pub groups: [u8; 2],
    /// Fewest digits of the day and month (1 or 2); of the year, 2 for its last two digits, else
    /// all.
    pub digits: [u8; 3],
    /// Fewest digits of the hour (1 or 2).
    pub hour_digits: u8,
    /// Between hours, minutes and seconds.
    pub time_separator: char,
}

impl Fine {
    /// Groups of three, two-digit days, months and hours, four-digit years, ":".
    pub const PLAIN: Fine = Fine { groups: [3, 3], digits: [2, 2, 4], hour_digits: 2, time_separator: ':' };
}

/// What dates, times and numbers are written with: a locale and its finer points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Formats {
    pub locale: Locale,
    pub fine: Fine,
}

/// The system's formats, as last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct System {
    locale: Locale,
    fine: Fine,
}

/// Counts region setting changes the system reported: what was read before is read again.
static CHANGES: AtomicU32 = AtomicU32::new(0);

/// The system's formats: read once, and again after a reported change.
fn system() -> System {
    static READ: Mutex<Option<(u32, System)>> = Mutex::new(None);
    let now = CHANGES.load(Ordering::Relaxed);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    match *read {
        Some((at, system)) if at == now => system,
        _ => read.insert((now, self::read())).1,
    }
}

/// Windows' word on a changed setting (`WM_SETTINGCHANGE`'s text, or null): the region
/// settings ("intl") are read again when next needed. Returns whether they changed.
///
/// # Safety
/// `said` is null or points to NUL-terminated UTF-16 text.
#[cfg(windows)]
pub(crate) unsafe fn region_changed(said: isize) -> bool {
    // (Each character is read only while the ones before it matched: never past the NUL.)
    // SAFETY: as the caller promises.
    let intl = said != 0 && "intl\0".encode_utf16().enumerate().all(|(at, letter)| unsafe { *(said as *const u16).add(at) } == letter);
    if intl {
        CHANGES.fetch_add(1, Ordering::Relaxed);
    }
    intl
}

impl Locale {
    /// The library's own, the same everywhere: 2026-11-30, 14:05, 1234.5, weeks from Monday.
    pub const PLAIN: Locale = Locale { date_order: DateOrder::YearMonthDay, date_separator: '-', first_weekday: 0, twelve_hour_clock: false, thousands_separator: None, decimal_point: '.' };

    /// The system's formats for the user. Read when first asked for, and again after Windows
    /// reports a change to them.
    pub fn of_the_system() -> Locale {
        system().locale
    }

    /// Parses typed text as the user writes numbers and as Rust does: spaces are dropped, as is
    /// a thousands separator followed by three digits (or two and another separator: 1,23,456);
    /// the user's separator or a point marks the fraction (so "1.5" is one and a half
    /// everywhere, and "1.500" is 1500 where a point groups thousands). `None` if it is not a
    /// number of this type.
    pub(crate) fn read_number<T: Number>(self, typed: &str) -> Option<T> {
        self.as_rust_writes(typed).parse::<T>().ok().filter(|number| number.to_f64().is_finite())
    }
    /// Typed text rewritten as Rust writes numbers, for Rust to parse. (Separate from parsing,
    /// so it exists once in a program whatever number types it reads.)
    fn as_rust_writes(self, typed: &str) -> String {
        let letters: Vec<char> = typed.trim().chars().filter(|letter| !letter.is_whitespace()).collect();
        let mut plain = String::with_capacity(letters.len());
        for (at, letter) in letters.iter().enumerate() {
            let run = letters[at + 1..].iter().take_while(|next| next.is_ascii_digit()).count();
            let group = run == 3 || run == 2 && letters.get(at + 1 + run) == Some(letter);
            match *letter {
                letter if Some(letter) == self.thousands_separator && group => {}
                letter if letter == self.decimal_point => plain.push('.'),
                letter => plain.push(letter),
            }
        }
        plain
    }
}

impl Formats {
    /// Plain finer points for this locale.
    #[cfg(test)]
    pub fn plain(locale: Locale) -> Formats {
        Formats { locale, fine: Fine::PLAIN }
    }

    /// Writes a date in digits, in this order, with this separator and digit counts.
    pub fn write_date(self, into: &mut String, date: Date) {
        let between = self.locale.date_separator;
        let [day_digits, month_digits, year_digits] = self.fine.digits.map(usize::from);
        let (day, month) = (date.day, date.month);
        let year = if year_digits == 2 { date.year.rem_euclid(100) } else { date.year };
        let year_digits = year_digits.max(2);
        let _ = match self.locale.date_order {
            DateOrder::YearMonthDay => write!(into, "{year:0year_digits$}{between}{month:0month_digits$}{between}{day:0day_digits$}"),
            DateOrder::DayMonthYear => write!(into, "{day:0day_digits$}{between}{month:0month_digits$}{between}{year:0year_digits$}"),
            DateOrder::MonthDayYear => write!(into, "{month:0month_digits$}{between}{day:0day_digits$}{between}{year:0year_digits$}"),
        };
    }

    /// Rewrites a number Rust formatted ("-1234.5") the user's way: their decimal separator,
    /// and if `grouped` (for reading, not typing), their thousands separator and group sizes.
    pub fn write_number(self, into: &mut String, rust: &str, grouped: bool) {
        let (whole, fraction) = rust.split_once('.').map_or((rust, None), |(whole, fraction)| (whole, Some(fraction)));
        let digits = whole.trim_start_matches('-');
        into.push_str(&whole[..whole.len() - digits.len()]);
        let mark = self.locale.thousands_separator.filter(|_| grouped && digits.bytes().all(|digit| digit.is_ascii_digit()));
        for (at, digit) in digits.char_indices() {
            if let Some(mark) = mark.filter(|_| at > 0 && starts_group(digits.len() - at, self.fine.groups)) {
                into.push(mark);
            }
            into.push(digit);
        }
        if let Some(fraction) = fraction {
            into.push(self.locale.decimal_point);
            into.push_str(fraction);
        }
    }

    /// Writes a number of hours with the locale's fewest digits.
    pub fn write_hour(self, into: &mut String, hour: u8) {
        let _ = write!(into, "{hour:0digits$}", digits = usize::from(self.fine.hour_digits));
    }
}

/// Whether a group of digits starts where `left` digits remain before the decimal separator,
/// in groups of `first` and then of `then` (0: no more groups).
fn starts_group(left: usize, [first, then]: [u8; 2]) -> bool {
    let (first, then) = (usize::from(first), usize::from(then));
    first > 0 && left >= first && (left == first || then > 0 && (left - first).is_multiple_of(then))
}

/// Dates and numbers, the user's way.
impl Ui {
    /// This window's formats: the app's, if set in [`ui.locale`](Ui::locale), else the
    /// system's.
    pub(crate) fn locale_in_use(&self) -> Locale {
        self.locale.unwrap_or_else(Locale::of_the_system)
    }
    /// This window's formats with their finer points: the system's, or the app's locale with
    /// plain ones.
    pub(crate) fn formats(&self) -> Formats {
        match self.locale {
            Some(locale) => Formats { locale, fine: Fine::PLAIN },
            None => Formats { locale: system().locale, fine: system().fine },
        }
    }
    /// The words calendars and clocks are written in: the app's, and where it left them
    /// English while the system's formats are in use, the system's month and day names and
    /// AM/PM.
    pub(crate) fn calendar_words_in_use(&self) -> CalendarWords {
        let (mut words, english) = (self.calendar_words, CalendarWords::default());
        let left = [words.months == english.months, words.days == english.days, words.halves == english.halves];
        if self.locale.is_some() || left == [false; 3] {
            return words;
        }
        let system = CalendarWords::of_the_system();
        if left[0] {
            words.months = system.months;
        }
        if left[1] {
            words.days = system.days;
        }
        if left[2] {
            words.halves = system.halves;
        }
        words
    }
    /// A date in this window's format: "30.11.2026" where the system puts the day first.
    /// (`date.to_string()` writes the sortable form.)
    pub fn format_date(&self, date: Date) -> String {
        let mut written = String::new();
        self.formats().write_date(&mut written, date);
        written
    }
    /// A number for reading in the user's format, with this many decimals: "1,234.5", "1.234,5",
    /// "1 234,5" or "12,34,567.5".
    pub fn format_number(&self, value: f64, decimals: usize) -> String {
        let (mut written, rust) = (String::new(), format!("{value:.decimals$}"));
        self.formats().write_number(&mut written, &rust, true);
        written
    }
}

impl CalendarWords {
    /// The system's month names, shortest day names and AM/PM words, in the language of the
    /// user's date formats; the rest English. (English where the system names none: C's
    /// locale, or a locale not installed.) Read once, and again after Windows reports a change.
    pub fn of_the_system() -> CalendarWords {
        static READ: Mutex<Option<(u32, CalendarWords)>> = Mutex::new(None);
        let now = CHANGES.load(Ordering::Relaxed);
        let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
        match *read {
            Some((at, words)) if at == now => words,
            _ => read.insert((now, read_names())).1,
        }
    }
}

#[cfg_attr(target_vendor = "apple", allow(dead_code, reason = "macOS reads no names yet"))]
/// Names the system said, made to last the program's life (each read leaks them once: once per
/// program, and once per change of settings). English where the system said nothing.
fn names(said: impl Iterator<Item = String>, into: &mut [&'static str]) {
    for (into, said) in into.iter_mut().zip(said) {
        if !said.trim().is_empty() {
            *into = Box::leak(said.trim().to_owned().into_boxed_str());
        }
    }
}

/// Date order and separator from a pattern where `d` is the day, `M` the month, `y` the year,
/// and quoted text is literal (Windows' format). `ddd` and longer is the weekday name, not the
/// day. `None` if one of the three is missing.
fn date_order(pattern: &str) -> Option<(DateOrder, char)> {
    // Quoted text is removed first; then the first of each.
    let plain: String = pattern.split('\'').step_by(2).collect();
    let day = day_at(&plain)?;
    let (month, year) = (plain.find('M')?, plain.find('y')?);
    let order = match (year < month, day < month) {
        (true, _) => DateOrder::YearMonthDay,
        (false, true) => DateOrder::DayMonthYear,
        (false, false) => DateOrder::MonthDayYear,
    };
    // A date written with words (Japanese, Chinese, Korean) gets the separator most of them use
    // in digits: a slash.
    Some((order, plain.chars().find(|letter| ".-/".contains(*letter)).unwrap_or('/')))
}

/// Where the day is in a date pattern without quoted text: the first run of one or two `d`.
fn day_at(plain: &str) -> Option<usize> {
    plain.match_indices('d').map(|(at, _)| at).find(|at| plain[*at..].bytes().take_while(|letter| *letter == b'd').count() < 3 && (*at == 0 || plain.as_bytes()[at - 1] != b'd'))
}

/// Fewest digits of day, month and year in a date pattern (Windows' format): `d` one, `dd` two;
/// `M` one, `MM` (or a month name) two; `yy` two, else all.
fn date_digits(pattern: &str) -> [u8; 3] {
    let plain: String = pattern.split('\'').step_by(2).collect();
    let run = |at: Option<usize>, letter: u8| at.map_or(0, |at| plain[at..].bytes().take_while(|next| *next == letter).count());
    let (day, month, year) = (run(day_at(&plain), b'd'), run(plain.find('M'), b'M'), run(plain.find('y'), b'y'));
    [if day == 1 { 1 } else { 2 }, if month == 1 { 1 } else { 2 }, if year == 1 || year == 2 { 2 } else { 4 }]
}

/// Fewest hour digits and the time separator in a time pattern (Windows' format: `H` or `h`
/// the hour, `m` the minutes, quoted text literal). The separator is the one character between
/// hours and minutes, else ":".
fn time_details(pattern: &str) -> (u8, char) {
    let plain: String = pattern.split('\'').step_by(2).collect();
    let Some(hour) = plain.find(['H', 'h']) else { return (2, ':') };
    let digits = plain[hour..].bytes().take_while(|letter| *letter == plain.as_bytes()[hour]).count().min(2) as u8;
    let after = &plain[hour + digits as usize..];
    let between = after.find('m').map_or("", |minutes| &after[..minutes]);
    let mut letters = between.chars();
    let mark = match (letters.next(), letters.next()) {
        (Some(mark), None) if !mark.is_alphanumeric() => mark,
        _ => ':',
    };
    (digits.max(1), mark)
}

/// Group sizes from Windows' grouping ("3;0" threes, "3;2;0" three then twos, "3" one group,
/// "0" none).
#[cfg(any(test, windows))]
fn windows_groups(said: &str) -> [u8; 2] {
    let sizes: Vec<u8> = said.split(';').map_while(|size| size.trim().parse().ok()).collect();
    match sizes[..] {
        [first, 0] => [first, first],
        [first, then, ..] => [first, then],
        [first] => [first, 0],
        [] => [0, 0],
    }
}

/// Group sizes from glibc's grouping bytes: the last size repeats; 127 or 255 (`CHAR_MAX`)
/// ends grouping.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
fn glibc_groups(said: &[u8]) -> [u8; 2] {
    let size = |size: Option<&u8>| size.copied().filter(|size| *size != 127 && *size != 255);
    match said {
        [] => [0, 0],
        [first] => [*first, *first],
        [first, then, ..] => [*first, size(Some(then)).unwrap_or(0)],
    }
}

/// The same pattern as glibc writes it: `%d` and `%e` the day, `%m`, `%b`, `%B` and `%h` the
/// month, `%Y`, `%y`, `%C` and `%G` the year, `%D` and `%F` whole dates; `%H`, `%k`, `%I` and `%l`
/// the hour, `%M` the minutes, `%S` the seconds, `%p` AM/PM, `%T` and `%R` whole times. A `-`
/// or `_` flag drops a padding zero; `0` keeps one.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
fn glibc_pattern(written: &str) -> String {
    let whole = written.replace("%D", "%m/%d/%y").replace("%F", "%Y-%m-%d").replace("%T", "%H:%M:%S").replace("%R", "%H:%M");
    let (mut pattern, mut letters) = (String::new(), whole.bytes());
    while let Some(letter) = letters.next() {
        // (Letters written as they are are quoted, as Windows quotes them.)
        if letter.is_ascii_alphabetic() {
            pattern.extend(['\'', letter as char, '\'']);
            continue;
        }
        if letter != b'%' {
            pattern.push(letter as char);
            continue;
        }
        // Padding flags between `%` and the conversion letter.
        let (mut flag, mut conversion) = (None, letters.next());
        while let Some(padding) = conversion.filter(|letter| b"-_0^#".contains(letter)) {
            (flag, conversion) = (Some(padding), letters.next());
        }
        let short = matches!(flag, Some(b'-' | b'_'));
        pattern.push_str(match conversion {
            Some(b'd') | Some(b'e') if short || conversion == Some(b'e') && flag != Some(b'0') => "d",
            Some(b'd' | b'e') => "dd",
            Some(b'm') if short => "M",
            Some(b'm') => "MM",
            Some(b'b' | b'B' | b'h') => "MMM",
            Some(b'y') => "yy",
            Some(b'Y' | b'C' | b'G') => "yyyy",
            Some(b'H') if short => "H",
            Some(b'H') => "HH",
            Some(b'k') => "H",
            Some(b'I') if short => "h",
            Some(b'I') => "hh",
            Some(b'l') => "h",
            Some(b'M') => "mm",
            Some(b'S') => "ss",
            Some(b'p' | b'P') => "tt",
            _ => " ",
        });
    }
    pattern
}

/// The first character of what the system said, if anything.
fn first(said: &str) -> Option<char> {
    said.chars().next()
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocaleInfoEx(name: *const u16, what: u32, into: *mut u16, room: i32) -> i32;
}

/// What Windows is asked about a locale: short date pattern, time pattern, first day of the
/// week (0 Monday), decimal separator, thousands separator, digit grouping.
#[cfg(windows)]
const ASKED: [u32; 6] = [0x1F, 0x1003, 0x100C, 0x0E, 0x0F, 0x10];

/// What Windows says of locale `name` (NUL-terminated, or null for the user's).
#[cfg(windows)]
fn windows_says(name: *const u16, what: u32) -> String {
    let mut into = [0u16; 80];
    // SAFETY: the system writes at most the room it is told of, which exists; the name is null
    // or NUL-terminated.
    let wrote = unsafe { GetLocaleInfoEx(name, what, into.as_mut_ptr(), into.len() as i32) };
    String::from_utf16_lossy(&into[..wrote.max(1) as usize - 1])
}

#[cfg(windows)]
fn read() -> System {
    from_windows(std::ptr::null())
}

/// Windows' formats for locale `name` ("de-DE", NUL-terminated), or the user's if null.
#[cfg(windows)]
fn from_windows(name: *const u16) -> System {
    let [date, time, first_day, point, thousands, groups] = ASKED.map(|what| windows_says(name, what));
    let (order, between) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    // Quoted text in a time pattern is literal: "HH 'h' mm" is a 24-hour clock.
    let twelve_hours = time.split('\'').step_by(2).any(|plain| plain.contains('h'));
    let first_day = first_day.trim().parse::<u8>().ok().filter(|day| *day < 7).unwrap_or(0);
    let locale = Locale {
        date_order: order,
        date_separator: between,
        first_weekday: first_day,
        twelve_hour_clock: twelve_hours,
        thousands_separator: first(&thousands),
        decimal_point: first(&point).unwrap_or('.'),
    };
    let (hour_digits, time_separator) = time_details(&time);
    System { locale, fine: Fine { groups: windows_groups(&groups), digits: date_digits(&date), hour_digits, time_separator } }
}

#[cfg(windows)]
fn read_names() -> CalendarWords {
    names_from_windows(std::ptr::null())
}

/// Windows' month names, shortest day names (Monday first) and AM/PM for locale `name`, or the
/// user's if null.
#[cfg(windows)]
fn names_from_windows(name: *const u16) -> CalendarWords {
    let mut words = CalendarWords::default();
    names((0x38..0x44).map(|what| windows_says(name, what)), &mut words.months);
    names((0x60..0x67).map(|what| windows_says(name, what)), &mut words.days);
    names([0x28, 0x29].into_iter().map(|what| windows_says(name, what)), &mut words.halves);
    words
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn newlocale(categories: i32, name: *const std::ffi::c_char, base: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn nl_langinfo_l(item: i32, locale: *mut std::ffi::c_void) -> *const std::ffi::c_char;
    fn freelocale(locale: *mut std::ffi::c_void);
}

/// What glibc is asked about a locale (`langinfo.h`): date format, time format, decimal
/// separator, thousands separator, grouping; the week's origin date (19971130, a Sunday), and
/// the first weekday counted from it (1 for that day).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const GLIBC: [i32; 7] = [0x20029, 0x2002a, 0x10000, 0x10001, 0x10002, 0x20066, 0x20068];
/// The items that are numbers in place of a pointer: the week's origin.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ORIGIN: usize = 5;
/// glibc's names (`langinfo.h`): abbreviated days from Sunday, months, AM and PM, standalone
/// months, and the codeset its time names are written in.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ABDAY_1: i32 = 0x20000;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MON_1: i32 = 0x2001a;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const AM_STR: i32 = 0x20026;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ALTMON_1: i32 = 0x2006f;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const TIME_CODESET: i32 = 0x2006e;

/// The locale the environment names for one category, as the C library looks it up; `None` for
/// C's own.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn named(kind: &str) -> Option<String> {
    let name = ["LC_ALL", kind, "LANG"].iter().find_map(|variable| std::env::var(variable).ok().filter(|name| !name.is_empty()))?;
    (!matches!(name.split(['.', '@']).next(), Some("C" | "POSIX"))).then_some(name)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn read() -> System {
    from_glibc(c"", [named("LC_TIME").is_some(), named("LC_NUMERIC").is_some()])
}

/// A glibc locale of this name ("" for the environment's), its time and number categories;
/// freed when dropped.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
struct Glibc(*mut std::ffi::c_void);

#[cfg(all(target_os = "linux", target_env = "gnu"))]
impl Glibc {
    fn new(name: &std::ffi::CStr) -> Option<Glibc> {
        // LC_TIME's mask is 4 and LC_NUMERIC's 2. SAFETY: glibc makes a locale from a
        // NUL-terminated name, or none.
        let locale = unsafe { newlocale(4 | 2, name.as_ptr(), std::ptr::null_mut()) };
        (!locale.is_null()).then(|| Glibc(locale))
    }
    /// What it says of `item`, as bytes. (Never for an item that is a number.)
    fn says(&self, item: i32) -> &[u8] {
        // SAFETY: for a live locale glibc returns NUL-terminated text that lives as long.
        let said = unsafe { nl_langinfo_l(item, self.0) };
        if said.is_null() { &[] } else { unsafe { std::ffi::CStr::from_ptr(said) }.to_bytes() }
    }
    fn text(&self, item: i32) -> String {
        String::from_utf8_lossy(self.says(item)).into_owned()
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
impl Drop for Glibc {
    fn drop(&mut self) {
        // SAFETY: the locale is freed once, after everything was read from it.
        unsafe { freelocale(self.0) };
    }
}

/// glibc's formats for locale `name`, dates and times and numbers each where asked; for "", the
/// environment's locales. The plain formats if the locale isn't installed.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn from_glibc(name: &std::ffi::CStr, [dates, numbers]: [bool; 2]) -> System {
    let mut system = System { locale: Locale::PLAIN, fine: Fine::PLAIN };
    let Some(locale) = Glibc::new(name) else { return system };
    let [date, time, point, thousands, groups, _, first_weekday] = std::array::from_fn(|at| if at == ORIGIN { Vec::new() } else { locale.says(GLIBC[at]).to_vec() });
    let text = |said: &[u8]| String::from_utf8_lossy(said).into_owned();
    if dates {
        // (`%r` is the 12-hour time format, T_FMT_AMPM.)
        let time = text(&time).replace("%r", &locale.text(0x2002b));
        let date = glibc_pattern(&text(&date));
        (system.locale.date_order, system.locale.date_separator) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
        (system.fine.digits, (system.fine.hour_digits, system.fine.time_separator)) = (date_digits(&date), time_details(&glibc_pattern(&time)));
        system.locale.twelve_hour_clock = ["%I", "%l", "%p", "%P"].iter().any(|twelve| time.replace("%%", "").contains(twelve));
        // The week origin as days after a Sunday; and the week's first day, counted from 1 for
        // that one. SAFETY: for this item glibc answers with the number itself, never read as
        // text.
        let counted_from = unsafe { nl_langinfo_l(GLIBC[ORIGIN], locale.0) } as usize as u32;
        let counted_from = Date::new((counted_from / 10_000) as i32, (counted_from / 100 % 100) as u8, (counted_from % 100) as u8);
        let after_sunday = (counted_from.weekday() as i32 + 1) % 7 + first_weekday.first().map_or(1, |day| i32::from(*day)) - 1;
        system.locale.first_weekday = (after_sunday + 6).rem_euclid(7) as u8;
    }
    if numbers {
        (system.locale.decimal_point, system.locale.thousands_separator) = (first(&text(&point)).unwrap_or('.'), first(&text(&thousands)));
        system.fine.groups = glibc_groups(&groups);
    }
    system
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn read_names() -> CalendarWords {
    match named("LC_TIME") {
        Some(_) => names_from_glibc(c""),
        None => CalendarWords::default(),
    }
}

/// glibc's standalone month names, abbreviated day names (Monday first, without a final
/// point) and AM/PM for locale `name`; English if it isn't installed or not in UTF-8.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn names_from_glibc(name: &std::ffi::CStr) -> CalendarWords {
    let mut words = CalendarWords::default();
    let Some(locale) = Glibc::new(name).filter(|locale| locale.says(TIME_CODESET).eq_ignore_ascii_case(b"UTF-8")) else { return words };
    let month = |at: i32| Some(locale.text(ALTMON_1 + at)).filter(|said| !said.is_empty()).unwrap_or_else(|| locale.text(MON_1 + at));
    names((0..12).map(month), &mut words.months);
    names((0..7).map(|at| locale.text(ABDAY_1 + (at + 1) % 7).trim_end_matches('.').to_owned()), &mut words.days);
    names((0..2).map(|at| locale.text(AM_STR + at)), &mut words.halves);
    words
}

/// Foundation's formats for the user's locale: the short date and the hour as the locale
/// writes them, hours and minutes (patterns in the letters Windows uses), the week's first day (1 Sunday), and the
/// number separators.
#[cfg(target_vendor = "apple")]
fn read() -> System {
    use crate::macos_sys::{class, ns_string, rust_string, send};
    let _pool = crate::macos_sys::Pool::new();
    // SAFETY: plain questions of the user's locale and calendar; nil answers nothing.
    let said = unsafe {
        let locale: crate::macos_sys::Obj = send(class(c"NSLocale"), c"currentLocale", ());
        let pattern = |template: &str| rust_string(send(class(c"NSDateFormatter"), c"dateFormatFromTemplate:options:locale:", (ns_string(template), 0usize, locale))).unwrap_or_default();
        let first_day: isize = send(send::<crate::macos_sys::Obj>(class(c"NSCalendar"), c"currentCalendar", ()), c"firstWeekday", ());
        let [point, thousands] = [c"decimalSeparator", c"groupingSeparator"].map(|asked| rust_string(send(locale, asked, ())).unwrap_or_default());
        (pattern("yMd"), pattern("jm"), first_day, point, thousands)
    };
    let (date, time, first_day, point, thousands) = said;
    let (order, between) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    // Quoted text is literal; `h` and `K` are a 12-hour clock's hours.
    let twelve_hours = time.split('\'').step_by(2).any(|plain| plain.contains(['h', 'K']));
    let locale = Locale {
        date_order: order,
        date_separator: between,
        first_weekday: ((first_day + 5).rem_euclid(7)) as u8,
        twelve_hour_clock: twelve_hours,
        thousands_separator: first(&thousands),
        decimal_point: first(&point).unwrap_or('.'),
    };
    // Foundation's patterns use the letters Windows does; group sizes stay threes.
    let (hour_digits, time_separator) = time_details(&time);
    System { locale, fine: Fine { groups: Fine::PLAIN.groups, digits: date_digits(&date), hour_digits, time_separator } }
}

/// macOS: English names until read from Foundation.
#[cfg(target_vendor = "apple")]
fn read_names() -> CalendarWords {
    CalendarWords::default()
}

/// Without Windows, Foundation or glibc to ask: the library's formats.
#[cfg(not(any(windows, target_vendor = "apple", all(target_os = "linux", target_env = "gnu"))))]
fn read() -> System {
    System { locale: Locale::PLAIN, fine: Fine::PLAIN }
}

/// Without Windows, Foundation or glibc to ask: English.
#[cfg(not(any(windows, target_vendor = "apple", all(target_os = "linux", target_env = "gnu"))))]
fn read_names() -> CalendarWords {
    CalendarWords::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_of_dates_are_read_for_their_order_and_mark() {
        // As Windows writes them, for the US, Germany, Sweden, Japan, Slovakia, and a pattern
        // with the weekday name.
        let windows = ["M/d/yyyy", "dd.MM.yyyy", "yyyy-MM-dd", "yyyy/MM/dd", "d. M. yyyy", "dddd, dd-MM-yyyy", "yyyy'年'M'月'd'日'"];
        let read: Vec<_> = windows.iter().map(|pattern| date_order(pattern)).collect();
        let (ymd, dmy, mdy) = (DateOrder::YearMonthDay, DateOrder::DayMonthYear, DateOrder::MonthDayYear);
        assert_eq!(read, [Some((mdy, '/')), Some((dmy, '.')), Some((ymd, '-')), Some((ymd, '/')), Some((dmy, '.')), Some((dmy, '-')), Some((ymd, '/'))]);
        assert_eq!(date_order("HH:mm"), None, "no date in it");
        let digits: Vec<_> = windows.iter().map(|pattern| date_digits(pattern)).collect();
        assert_eq!(digits, [[1, 1, 4], [2, 2, 4], [2, 2, 4], [2, 2, 4], [1, 1, 4], [2, 2, 4], [1, 1, 4]]);
        assert_eq!([date_digits("dd/MM/yy"), date_digits("d-MMM-yy")], [[2, 2, 2], [1, 2, 2]]);
        // As glibc writes them (en_US, de_DE, en_GB, nb_NO, ja_JP, hi_IN, and whole dates).
        let glibc = ["%m/%d/%Y", "%d.%m.%Y", "%d/%m/%y", "%d. %b %Y", "%Y年%m月%d日", "%-d/%-m/%y", "%D", "%F"];
        let read: Vec<_> = glibc.iter().map(|pattern| date_order(&glibc_pattern(pattern))).collect();
        let wanted = [(mdy, '/'), (dmy, '.'), (dmy, '/'), (dmy, '.'), (ymd, '/'), (dmy, '/'), (mdy, '/'), (ymd, '-')];
        assert_eq!(read, wanted.map(Some));
        let digits: Vec<_> = glibc.iter().map(|pattern| date_digits(&glibc_pattern(pattern))).collect();
        assert_eq!(digits, [[2, 2, 4], [2, 2, 4], [2, 2, 2], [2, 2, 4], [2, 2, 4], [1, 1, 2], [2, 2, 2], [2, 2, 4]]);
        assert_eq!([date_digits(&glibc_pattern("%e.%m.%Y")), date_digits(&glibc_pattern("%0e.%m.%Y"))], [[1, 2, 4], [2, 2, 4]]);
    }

    #[test]
    fn patterns_of_times_are_read_for_hour_digits_and_mark() {
        // Windows: the US, Germany, Finland, Canadian French, Korean (AM/PM first).
        let windows = ["h:mm:ss tt", "HH:mm:ss", "H.mm.ss", "HH 'h' mm", "tt h:mm:ss"];
        let read: Vec<_> = windows.iter().map(|pattern| time_details(pattern)).collect();
        assert_eq!(read, [(1, ':'), (2, ':'), (1, '.'), (2, ':'), (1, ':')]);
        // glibc: en_US's 12-hour clock, de_DE, fi_FI, a padded and an unpadded hour.
        let glibc = ["%I:%M:%S %p", "%T", "%H.%M.%S", "%k:%M", "%-H:%M"];
        let read: Vec<_> = glibc.iter().map(|pattern| time_details(&glibc_pattern(pattern))).collect();
        assert_eq!(read, [(2, ':'), (2, ':'), (2, '.'), (1, ':'), (1, ':')]);
        assert_eq!(time_details("mm:ss"), (2, ':'));
    }

    #[test]
    fn group_sizes_are_read_as_each_system_writes_them() {
        assert_eq!(["3;0", "3;2;0", "3", "0", ""].map(windows_groups), [[3, 3], [3, 2], [3, 0], [0, 0], [0, 0]]);
        assert_eq!([&[3u8, 3][..], &[3, 2], &[3], &[3, 127], &[]].map(glibc_groups), [[3, 3], [3, 2], [3, 3], [3, 0], [0, 0]]);
    }

    #[test]
    fn numbers_are_written_and_read_with_the_marks_of_the_place() {
        let german = Locale { thousands_separator: Some('.'), decimal_point: ',', ..Locale::PLAIN };
        let american = Locale { thousands_separator: Some(','), ..Locale::PLAIN };
        let french = Locale { thousands_separator: Some('\u{202f}'), decimal_point: ',', ..Locale::PLAIN };
        let written = |locale: Locale, rust: &str, grouped: bool| {
            let mut into = String::new();
            Formats::plain(locale).write_number(&mut into, rust, grouped);
            into
        };
        assert_eq!([written(german, "-1234567.25", true), written(american, "-1234567.25", true), written(german, "1234.5", false)], ["-1.234.567,25", "-1,234,567.25", "1234,5"]);
        assert_eq!([written(american, "123", true), written(american, "1000", true), written(french, "12345", true)], ["123", "1,000", "12\u{202f}345"]);
        assert_eq!([written(Locale::PLAIN, "NaN", true), written(american, "inf", true)], ["NaN", "inf"]);
        // Groups of other sizes: India's three then twos; one group only; none.
        let groups = |groups: [u8; 2], rust: &str| {
            let mut into = String::new();
            Formats { locale: american, fine: Fine { groups, ..Fine::PLAIN } }.write_number(&mut into, rust, true);
            into
        };
        assert_eq!([groups([3, 2], "1234567.5"), groups([3, 2], "123"), groups([3, 2], "-12345")], ["12,34,567.5", "123", "-12,345"]);
        assert_eq!([groups([3, 0], "1234567"), groups([0, 0], "1234567"), groups([4, 4], "123456789")], ["1234,567", "1234567", "1,2345,6789"]);
        // Parsed back: the user's separators, and Rust's point anywhere.
        assert_eq!([german.read_number::<f64>("1.234,5"), german.read_number("1,5"), german.read_number("1.5"), german.read_number(" 1.500 ")], [Some(1234.5), Some(1.5), Some(1.5), Some(1500.0)]);
        assert_eq!(
            [american.read_number::<f64>("1,234.5"), american.read_number("1,5"), french.read_number("12\u{202f}345,5"), french.read_number("12 345")],
            [Some(1234.5), None, Some(12345.5), Some(12345.0)]
        );
        assert_eq!([american.read_number::<f64>("12,34,567.5"), american.read_number("1,50")], [Some(1234567.5), None], "India's groups read back");
        assert_eq!([Locale::PLAIN.read_number::<i32>("-42"), Locale::PLAIN.read_number("4.2"), Locale::PLAIN.read_number("x")], [Some(-42), None, None]);
        assert_eq!([Locale::PLAIN.read_number::<f32>("inf"), Locale::PLAIN.read_number("NaN")], [None, None]);
        assert_eq!(Locale::PLAIN.read_number::<u8>("-1"), None);
        let mut date = String::new();
        for locale in
            [Locale::PLAIN, Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', ..Locale::PLAIN }, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', ..Locale::PLAIN }]
        {
            Formats::plain(locale).write_date(&mut date, Date::new(2026, 3, 7));
            date.push(' ');
        }
        assert_eq!(date, "2026-03-07 07.03.2026 03/07/2026 ");
        // Fewer digits: the US's 3/7/2026, and two-digit years.
        let us = Formats { locale: Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', ..Locale::PLAIN }, fine: Fine { digits: [1, 1, 4], hour_digits: 1, ..Fine::PLAIN } };
        let short = Formats { locale: Locale { date_order: DateOrder::DayMonthYear, date_separator: '/', ..Locale::PLAIN }, fine: Fine { digits: [2, 2, 2], ..Fine::PLAIN } };
        date.clear();
        us.write_date(&mut date, Date::new(2026, 3, 7));
        date.push(' ');
        us.write_date(&mut date, Date::new(2026, 11, 30));
        date.push(' ');
        short.write_date(&mut date, Date::new(2005, 3, 7));
        date.push(' ');
        us.write_hour(&mut date, 9);
        short.write_hour(&mut date, 9);
        assert_eq!(date, "3/7/2026 11/30/2026 07/03/05 909");
    }

    #[test]
    fn a_window_takes_the_finer_points_of_the_system_only_with_its_formats() {
        let system = system();
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        assert_eq!(ui.formats(), Formats { locale: system.locale, fine: system.fine });
        // The app's own locale, even the system's itself: plain, the same on every system.
        ui.locale = Some(system.locale);
        assert_eq!(ui.formats(), Formats { locale: system.locale, fine: Fine::PLAIN });
    }

    #[test]
    fn calendars_take_the_systems_names_where_the_app_left_them_english() {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        let system = CalendarWords::of_the_system();
        let in_use = ui.calendar_words_in_use();
        assert_eq!((in_use.months, in_use.days, in_use.halves, in_use.clock), (system.months, system.days, system.halves, CalendarWords::default().clock));
        // The app's own months stay; days and AM/PM left English are still the system's.
        ui.calendar_words.months[0] = "Jan";
        assert_eq!((ui.calendar_words_in_use().months[0], ui.calendar_words_in_use().days), ("Jan", system.days));
        // With the app's own formats, its own words: English unless set.
        ui.locale = Some(Locale::PLAIN);
        assert_eq!(ui.calendar_words_in_use(), ui.calendar_words);
        // A time is written with the formats' hour digits and separator.
        ui.locale = Some(Locale { twelve_hour_clock: true, ..Locale::PLAIN });
        assert_eq!(ui.format_time(crate::clock::Time::new(21, 5, 9), true), "09:05:09 PM");
    }

    /// Windows' word on a changed setting is read for "intl" alone, and has the system's
    /// formats read afresh.
    #[cfg(windows)]
    #[test]
    fn a_change_of_region_settings_has_them_read_again() {
        let said = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let before = CHANGES.load(Ordering::Relaxed);
        let [intl, other, longer, shorter] = [said("intl"), said("ImmersiveColorSet"), said("intlx"), said("int")];
        // SAFETY: each is NUL-terminated text.
        let heard = [&intl, &other, &longer, &shorter].map(|text| unsafe { region_changed(text.as_ptr() as isize) });
        assert_eq!(heard, [true, false, false, false]);
        assert!(!unsafe { region_changed(0) });
        assert!(CHANGES.load(Ordering::Relaxed) > before);
        assert_eq!(Locale::of_the_system(), from_windows(std::ptr::null()).locale);
    }

    /// glibc answers for locales by name: American English, always installed, and Germany,
    /// Sweden, India and Finland where a test provides them (compiled by `localedef` into a folder
    /// `LOCPATH` names).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_says_how_places_write_dates_times_and_numbers() {
        let us = from_glibc(c"en_US.UTF-8", [true, true]);
        assert_eq!(us.locale, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', first_weekday: 6, twelve_hour_clock: true, thousands_separator: Some(','), decimal_point: '.' });
        assert_eq!(us.fine, Fine { groups: [3, 3], digits: [2, 2, 4], hour_digits: 2, time_separator: ':' });
        assert_eq!(from_glibc(c"xx_NOWHERE.UTF-8", [true, true]), System { locale: Locale::PLAIN, fine: Fine::PLAIN }, "a locale that is not there");
        assert_eq!(from_glibc(c"en_US.UTF-8", [false, true]).locale, Locale { thousands_separator: Some(','), ..Locale::PLAIN });
        let names = names_from_glibc(c"en_US.UTF-8");
        assert_eq!((names.months[10], names.days, names.halves), ("November", ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], ["AM", "PM"]));
        assert_eq!(names_from_glibc(c"xx_NOWHERE.UTF-8"), CalendarWords::default());
        if std::env::var_os("LOCPATH").is_some() {
            let germany = Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', first_weekday: 0, twelve_hour_clock: false, thousands_separator: Some('.'), decimal_point: ',' };
            assert_eq!(from_glibc(c"de_DE.UTF-8", [true, true]).locale, germany);
            let sweden = from_glibc(c"sv_SE.UTF-8", [true, true]).locale;
            assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
            assert_eq!(from_glibc(c"en_IN.UTF-8", [true, true]).fine.groups, [3, 2]);
            assert_eq!(from_glibc(c"fi_FI.UTF-8", [true, true]).fine.time_separator, '.');
            let german = names_from_glibc(c"de_DE.UTF-8");
            assert_eq!((german.months[2], german.days[0], german.days[6]), ("März", "Mo", "So"));
            let russian = names_from_glibc(c"ru_RU.UTF-8");
            assert_eq!(russian.months[0].to_lowercase(), "январь", "the month's own name, not 'of January'");
        }
    }

    /// The system's formats come from the locale the environment names: asked in a child
    /// process, so this test's own environment is untouched.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn the_system_is_read_from_the_locale_the_environment_names() {
        let asked = |names: &[(&str, &str)]| {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "locale::tests::what_the_environment_names", "--ignored", "--nocapture"]);
            ["LC_ALL", "LC_TIME", "LC_NUMERIC", "LANG"].iter().for_each(|name| _ = child.env_remove(name));
            String::from_utf8_lossy(&child.envs(names.iter().copied()).output().unwrap().stdout).into_owned()
        };
        let plain = format!("{:?}", Locale::PLAIN);
        assert!(asked(&[("LANG", "C.UTF-8")]).contains(&plain), "C's own is the plain way");
        assert!(asked(&[("LANG", "en_US.UTF-8"), ("LC_ALL", "POSIX")]).contains(&plain), "LC_ALL comes first");
        assert!(asked(&[]).contains(&plain), "none named");
        let american = asked(&[("LANG", "en_US.UTF-8")]);
        assert!(american.contains("MonthDayYear") && american.contains("twelve_hour_clock: true") && american.contains("\"Mon\""), "{american}");
        // Dates as C writes them, numbers as the US does.
        let mixed = asked(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "C.UTF-8")]);
        assert!(mixed.contains("YearMonthDay") && mixed.contains("thousands_separator: Some(',')") && mixed.contains("\"Mo\""), "{mixed}");
        if std::env::var_os("LOCPATH").is_some() {
            let german = asked(&[("LANG", "de_DE.UTF-8")]);
            assert!(german.contains("DayMonthYear") && german.contains("\"März\""), "{german}");
        }
    }

    /// The child of the test above: prints what the system said.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    #[ignore]
    fn what_the_environment_names() {
        println!("{:?} {:?}", Locale::of_the_system(), CalendarWords::of_the_system());
    }

    /// Windows answers for locales by name, so the user's settings are never touched.
    #[cfg(windows)]
    #[test]
    fn windows_says_how_places_write_dates_times_and_numbers() {
        let named = |name: &str| name.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let of = |name: &str| from_windows(named(name).as_ptr());
        let (us, germany, sweden, japan) = (of("en-US"), of("de-DE"), of("sv-SE"), of("ja-JP"));
        assert_eq!(us.locale, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', first_weekday: 6, twelve_hour_clock: true, thousands_separator: Some(','), decimal_point: '.' });
        assert_eq!(us.fine, Fine { groups: [3, 3], digits: [1, 1, 4], hour_digits: 1, time_separator: ':' });
        assert_eq!(germany.locale, Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', first_weekday: 0, twelve_hour_clock: false, thousands_separator: Some('.'), decimal_point: ',' });
        assert_eq!(germany.fine, Fine { groups: [3, 3], digits: [2, 2, 4], hour_digits: 2, time_separator: ':' });
        let sweden = sweden.locale;
        assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
        let japan = japan.locale;
        assert_eq!((japan.date_order, japan.date_separator, japan.first_weekday, japan.twelve_hour_clock), (DateOrder::YearMonthDay, '/', 6, false));
        assert_eq!(of("hi-IN").fine.groups, [3, 2], "India groups by three, then two");
        assert_eq!(of("fi-FI").fine.time_separator, '.');
        // Names, in the place's own language.
        let names = |name: &str| names_from_windows(named(name).as_ptr());
        let (english, german, french) = (names("en-US"), names("de-DE"), names("fr-FR"));
        assert_eq!((english.months[10], english.days, english.halves), ("November", ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"], ["AM", "PM"]));
        assert_eq!((german.months[2], german.days[0], german.days[6]), ("März", "Mo", "So"));
        assert_eq!((french.months[7], french.days[0]), ("août", "lu"));
        assert_eq!(names("ru-RU").months[0].to_lowercase(), "январь", "the month's own name, not 'of January'");
        // And the user's own reads without trouble, whatever it is.
        let own = Locale::of_the_system();
        assert!(own.first_weekday < 7 && own.decimal_point != '\0', "{own:?}");
        assert!(CalendarWords::of_the_system().months.iter().all(|month| !month.is_empty()));
    }
}
