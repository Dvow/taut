//! The user's date, time and number formats: date order, separator and digit counts; first day
//! of the week; 12 or 24-hour clock, AM/PM's place, hour digits and separator; decimal and
//! thousands separators, group sizes and digits. And the calendar's month and day names and
//! AM/PM.
//!
//! Read when first needed, and read again when the system reports a change:
//!
//! | System | Read from | Followed while running |
//! |---|---|---|
//! | Windows | the user's region settings | `WM_SETTINGCHANGE` "intl" |
//! | Linux (glibc) | the locale `LC_ALL`, else `LC_TIME` or `LC_NUMERIC`, else `LANG` names | GNOME's 12/24-hour clock, through the portal |
//! | macOS, iOS | `NSLocale.currentLocale` | `NSCurrentLocaleDidChangeNotification` |
//! | Android | `Locale.getDefault()`, the 24-hour setting | configuration changes; the 24-hour setting on resume |
//!
//! With no locale named, or C's ("C", "POSIX", "C.UTF-8"), and on other systems:
//! [`Locale::PLAIN`]. An app sets its own in [`ui.locale`](Ui::locale), such as
//! [`Locale::named("hi-IN")`](Locale::named).
use crate::date::{CalendarWords, Date};
use crate::number::Number;
use crate::ui::Ui;
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

/// Where a 12-hour clock writes AM/PM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmPmPlace {
    /// After the time: "2:05 PM".
    After,
    /// Before the time, with a space: "오후 2:05" (Korean).
    Before,
    /// Before the time, touching it: "下午2:05" (Chinese).
    BeforeTouching,
}

/// Date, time and number formats. [`Locale::of_the_system()`](Locale::of_the_system) is the
/// user's; [`Locale::named`] one CLDR gives; [`Locale::PLAIN`] the library's: 2026-11-30,
/// 14:05, 1234.5, weeks from Monday.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locale {
    /// Date order.
    pub date_order: DateOrder,
    /// Date separator: '-', '.' or '/'.
    pub date_separator: char,
    /// Fewest digits of the day, month and year: 1 or 2 for day and month; for the year, 2 (its
    /// last two) or 4 (all).
    pub date_digits: [u8; 3],
    /// First day of the week: 0 Monday to 6 Sunday.
    pub first_weekday: u8,
    /// A 12-hour clock, from 12 to 11 with AM/PM (`ui.calendar_words.halves`), instead of 0 to
    /// 23.
    pub twelve_hour_clock: bool,
    /// Where a 12-hour clock writes AM/PM.
    pub am_pm_place: AmPmPlace,
    /// Fewest digits of the hour: 1 or 2.
    pub hour_digits: u8,
    /// Between hours, minutes and seconds: ':' or '.'.
    pub time_separator: char,
    /// The thousands separator in numbers written for reading ("1,234,567"); `None` if none.
    /// (Text boxes write none.)
    pub thousands_separator: Option<char>,
    /// Digits in the group nearest the decimal separator, then in each group before it (0: no
    /// more groups): `[3, 3]` writes 1,234,567; `[3, 2]` 12,34,567.
    pub group_sizes: [u8; 2],
    /// The decimal separator: '.' or ','.
    pub decimal_point: char,
    /// The zero of the digits numbers are written in: '0', '٠' (Arabic), '۰' (Persian), '०'
    /// (Devanagari), '০' (Bengali), '๐' (Thai)… The other nine follow it in Unicode. Typed
    /// numbers may use these or 0–9.
    pub zero_digit: char,
}

impl Default for Locale {
    fn default() -> Self {
        Locale::PLAIN
    }
}

/// Counts locale changes the system reported: what was read before is read again.
static CHANGES: AtomicU32 = AtomicU32::new(0);

/// The system's formats: read once, and again after a reported change.
fn system() -> Locale {
    static READ: Mutex<Option<(u32, Locale)>> = Mutex::new(None);
    let now = CHANGES.load(Ordering::Relaxed);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    match *read {
        Some((at, locale)) if at == now => locale,
        _ => read.insert((now, desktop_clock(self::read()))).1,
    }
}

/// The system reported a change of locale or region settings: they are read again when next
/// needed.
pub(crate) fn changed() {
    CHANGES.fetch_add(1, Ordering::Relaxed);
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
        changed();
    }
    intl
}

/// The desktop's 12 or 24-hour clock (GNOME's, through the portal): 0 unknown, 1 twelve, 2
/// twenty-four.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
static CLOCK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The desktop said whether its clock has 12 hours (`None`: it says nothing). Returns whether
/// that changed: the formats are then read again.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) fn set_desktop_clock(twelve_hours: Option<bool>) -> bool {
    let said = twelve_hours.map_or(0, |twelve| if twelve { 1 } else { 2 });
    let changes = CLOCK.swap(said, Ordering::Relaxed) != said;
    if changes {
        changed();
    }
    changes
}

/// The system's formats with the desktop's clock, where it named one.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn desktop_clock(locale: Locale) -> Locale {
    with_clock(locale, CLOCK.load(Ordering::Relaxed))
}

/// The system's formats: only X11 and Wayland desktops name a clock of their own.
#[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
fn desktop_clock(locale: Locale) -> Locale {
    locale
}

/// `locale` with the clock the desktop said, as `CLOCK` holds it.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn with_clock(mut locale: Locale, said: u8) -> Locale {
    if said != 0 {
        locale.twelve_hour_clock = said == 1;
    }
    locale
}

impl Locale {
    /// The library's own, the same everywhere: 2026-11-30, 14:05, 1234.5, weeks from Monday.
    pub const PLAIN: Locale = Locale {
        date_order: DateOrder::YearMonthDay,
        date_separator: '-',
        date_digits: [2, 2, 4],
        first_weekday: 0,
        twelve_hour_clock: false,
        am_pm_place: AmPmPlace::After,
        hour_digits: 2,
        time_separator: ':',
        thousands_separator: None,
        group_sizes: [3, 3],
        decimal_point: '.',
        zero_digit: '0',
    };

    /// The system's formats for the user. Read when first asked for, and again after the
    /// system reports a change to them.
    pub fn of_the_system() -> Locale {
        system()
    }

    /// The value of a digit typed: 0–9, or one of this locale's digits.
    pub(crate) fn digit_value(self, letter: char) -> Option<u32> {
        let native = (letter as u32).wrapping_sub(self.zero_digit as u32);
        letter.to_digit(10).or((native < 10).then_some(native))
    }

    /// An ASCII digit as this locale writes it; anything else as it is.
    fn digit(self, letter: char) -> char {
        match letter {
            '0'..='9' => char::from_u32(self.zero_digit as u32 + (letter as u32 - '0' as u32)).unwrap_or(letter),
            _ => letter,
        }
    }

    /// Parses typed text as the user writes numbers and as Rust does: spaces are dropped, as is
    /// a thousands separator followed by three digits (or two and another separator: 1,23,456);
    /// the user's separator or a point marks the fraction (so "1.5" is one and a half
    /// everywhere, and "1.500" is 1500 where a point groups thousands); the locale's digits
    /// count as 0–9. `None` if it is not a number of this type.
    pub(crate) fn read_number<T: Number>(self, typed: &str) -> Option<T> {
        self.as_rust_writes(typed).parse::<T>().ok().filter(|number| number.to_f64().is_finite())
    }
    /// Typed text rewritten as Rust writes numbers, for Rust to parse. (Separate from parsing,
    /// so it exists once in a program whatever number types it reads.)
    fn as_rust_writes(self, typed: &str) -> String {
        let letters: Vec<char> = typed.trim().chars().filter(|letter| !letter.is_whitespace()).collect();
        let is_digit = |letter: &char| self.digit_value(*letter).is_some();
        let mut plain = String::with_capacity(letters.len());
        for (at, letter) in letters.iter().enumerate() {
            let run = letters[at + 1..].iter().take_while(|next| is_digit(next)).count();
            let group = run == 3 || run == 2 && letters.get(at + 1 + run) == Some(letter);
            match (*letter, self.digit_value(*letter)) {
                (_, Some(digit)) => plain.push(char::from(b'0' + digit as u8)),
                (letter, None) if Some(letter) == self.thousands_separator && group => {}
                (letter, None) if letter == self.decimal_point => plain.push('.'),
                (letter, None) => plain.push(letter),
            }
        }
        plain
    }

    /// Writes a whole number with at least `fewest` digits, in this locale's digits. (Digit by
    /// digit: smaller than `write!` with a width, then each digit mapped.)
    pub(crate) fn write_digits(self, into: &mut String, number: i32, fewest: usize) {
        if number < 0 {
            into.push('-');
        }
        let size = number.unsigned_abs();
        let digits = size.checked_ilog10().map_or(1, |log| log as usize + 1);
        for place in (0..digits.max(fewest)).rev() {
            let digit = 10u32.checked_pow(place as u32).map_or(0, |power| size / power % 10);
            into.push(self.digit(char::from(b'0' + digit as u8)));
        }
    }

    /// Writes a date in digits, in this order, with this separator and digit counts.
    pub(crate) fn write_date(self, into: &mut String, date: Date) {
        let [day_digits, month_digits, year_digits] = self.date_digits.map(usize::from);
        let year = if year_digits == 2 { date.year.rem_euclid(100) } else { date.year };
        let day = (i32::from(date.day), day_digits);
        let month = (i32::from(date.month), month_digits);
        let year = (year, year_digits.max(2));
        let parts = match self.date_order {
            DateOrder::YearMonthDay => [year, month, day],
            DateOrder::DayMonthYear => [day, month, year],
            DateOrder::MonthDayYear => [month, day, year],
        };
        for (at, (number, fewest)) in parts.into_iter().enumerate() {
            if at > 0 {
                into.push(self.date_separator);
            }
            self.write_digits(into, number, fewest);
        }
    }

    /// Rewrites a number Rust formatted ("-1234.5") the user's way: their digits and decimal
    /// separator, and if `grouped` (for reading, not typing), their thousands separator and
    /// group sizes.
    pub(crate) fn write_number(self, into: &mut String, rust: &str, grouped: bool) {
        let (whole, fraction) = rust.split_once('.').map_or((rust, None), |(whole, fraction)| (whole, Some(fraction)));
        let digits = whole.trim_start_matches('-');
        into.push_str(&whole[..whole.len() - digits.len()]);
        let mark = self.thousands_separator.filter(|_| grouped && digits.bytes().all(|digit| digit.is_ascii_digit()));
        for (at, digit) in digits.char_indices() {
            if let Some(mark) = mark.filter(|_| at > 0 && starts_group(digits.len() - at, self.group_sizes)) {
                into.push(mark);
            }
            into.push(self.digit(digit));
        }
        if let Some(fraction) = fraction {
            into.push(self.decimal_point);
            into.extend(fraction.chars().map(|digit| self.digit(digit)));
        }
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
        self.locale_in_use().write_date(&mut written, date);
        written
    }
    /// A number for reading in the user's format, with this many decimals: "1,234.5", "1.234,5",
    /// "1 234,5", "12,34,567.5" or "١٬٢٣٤٫٥".
    pub fn format_number(&self, value: f64, decimals: usize) -> String {
        let (mut written, rust) = (String::new(), format!("{value:.decimals$}"));
        self.locale_in_use().write_number(&mut written, &rust, true);
        written
    }
}

impl CalendarWords {
    /// The system's month names, shortest day names and AM/PM words, in the language of the
    /// user's date formats; the rest English. (English where the system names none: C's
    /// locale, or a locale not installed.) Read once, and again after the system reports a
    /// change.
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

#[cfg_attr(not(any(windows, target_vendor = "apple", target_os = "android", all(target_os = "linux", target_env = "gnu"))), allow(dead_code, reason = "no system to ask"))]
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

/// A CLDR date pattern (Apple's, Android's) with its years in Windows' letters: CLDR's `y` is the
/// whole year, Windows' `yyyy`; `yy`, the last two digits, is the same in both. Quoted text
/// stays.
#[cfg(any(test, target_vendor = "apple", target_os = "android"))]
fn whole_years(cldr: &str) -> String {
    let mut pattern = String::with_capacity(cldr.len() + 3);
    for (at, part) in cldr.split('\'').enumerate() {
        if at > 0 {
            pattern.push('\'');
        }
        if at % 2 == 1 {
            pattern.push_str(part);
            continue;
        }
        let mut letters = part.chars().peekable();
        while let Some(letter) = letters.next() {
            if letter != 'y' {
                pattern.push(letter);
                continue;
            }
            let mut run = 1;
            while letters.next_if_eq(&'y').is_some() {
                run += 1;
            }
            pattern.push_str(if run == 2 { "yy" } else { "yyyy" });
        }
    }
    pattern
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

/// Where AM/PM is in a time pattern: Windows' `tt`, or CLDR's `a` or `B` (quoted text literal).
/// After the time unless it comes before the hour; touching it with nothing between.
fn am_pm_place(pattern: &str) -> AmPmPlace {
    let plain: String = pattern.split('\'').step_by(2).collect();
    let (Some(half), Some(hour)) = (plain.find(['t', 'a', 'B']), plain.find(['H', 'h', 'K', 'k'])) else { return AmPmPlace::After };
    let marker = plain.as_bytes()[half];
    let end = half + plain[half..].bytes().take_while(|letter| *letter == marker).count();
    match half < hour {
        false => AmPmPlace::After,
        true if end == hour => AmPmPlace::BeforeTouching,
        true => AmPmPlace::Before,
    }
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

/// The first character of what the system said, if anything. (Android answers characters.)
#[cfg(not(target_os = "android"))]
fn first(said: &str) -> Option<char> {
    said.chars().next()
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocaleInfoEx(name: *const u16, what: u32, into: *mut u16, room: i32) -> i32;
    fn GetLocaleInfoW(locale: u32, what: u32, into: *mut u16, room: i32) -> i32;
    fn GetUserDefaultUILanguage() -> u16;
}

/// What Windows is asked about a locale: short date pattern, time pattern, first day of the
/// week (0 Monday), decimal separator, thousands separator, digit grouping, native digits,
/// and when they are used (0 by context, 1 never, 2 always).
#[cfg(windows)]
const ASKED: [u32; 8] = [0x1F, 0x1003, 0x100C, 0x0E, 0x0F, 0x10, 0x13, 0x1014];

/// What Windows says of locale `name` (NUL-terminated, or null for the user's).
#[cfg(windows)]
fn windows_says(name: *const u16, what: u32) -> String {
    let mut into = [0u16; 80];
    // SAFETY: the system writes at most the room it is told of, which exists; the name is null
    // or NUL-terminated.
    let wrote = unsafe { GetLocaleInfoEx(name, what, into.as_mut_ptr(), into.len() as i32) };
    String::from_utf16_lossy(&into[..wrote.max(1) as usize - 1])
}

/// Whether the user's display language reads right to left: Windows then shows numbers "by
/// context" in native digits, as at the start of a right-to-left paragraph.
#[cfg(windows)]
fn display_reads_right_to_left() -> bool {
    let mut into = [0u16; 4];
    // LOCALE_IREADINGLAYOUT: 1 right to left. SAFETY: the room is told; a language id is a
    // locale id with default sorting.
    let wrote = unsafe { GetLocaleInfoW(u32::from(GetUserDefaultUILanguage()), 0x70, into.as_mut_ptr(), into.len() as i32) };
    wrote > 1 && into[0] == u16::from(b'1')
}

#[cfg(windows)]
fn read() -> Locale {
    from_windows(std::ptr::null(), display_reads_right_to_left())
}

/// Windows' formats for locale `name` ("de-DE", NUL-terminated), or the user's if null;
/// numbers shown "by context" take native digits if `right_to_left`.
#[cfg(windows)]
fn from_windows(name: *const u16, right_to_left: bool) -> Locale {
    let [date, time, first_day, point, thousands, groups, digits, used] = ASKED.map(|what| windows_says(name, what));
    let (order, between) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    // Quoted text in a time pattern is literal: "HH 'h' mm" is a 24-hour clock.
    let twelve_hours = time.split('\'').step_by(2).any(|plain| plain.contains('h'));
    let first_day = first_day.trim().parse::<u8>().ok().filter(|day| *day < 7).unwrap_or(0);
    let (hour_digits, time_separator) = time_details(&time);
    let native = used.trim() == "2" || used.trim() == "0" && right_to_left;
    Locale {
        date_order: order,
        date_separator: between,
        date_digits: date_digits(&date),
        first_weekday: first_day,
        twelve_hour_clock: twelve_hours,
        am_pm_place: am_pm_place(&time),
        hour_digits,
        time_separator,
        thousands_separator: first(&thousands),
        group_sizes: windows_groups(&groups),
        decimal_point: first(&point).unwrap_or('.'),
        zero_digit: first(&digits).filter(|_| native).unwrap_or('0'),
    }
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
/// glibc's names (`langinfo.h`): abbreviated days from Sunday, months, AM and PM, the 12-hour
/// time format, standalone months, and the codeset its time names are written in.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ABDAY_1: i32 = 0x20000;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MON_1: i32 = 0x2001a;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const AM_STR: i32 = 0x20026;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const T_FMT_AMPM: i32 = 0x2002b;
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
fn read() -> Locale {
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
/// environment's locales. The plain formats if the locale isn't installed. (glibc's locales
/// write only 0–9.)
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn from_glibc(name: &std::ffi::CStr, [dates, numbers]: [bool; 2]) -> Locale {
    let mut locale = Locale::PLAIN;
    let Some(glibc) = Glibc::new(name) else { return locale };
    let [date, time, point, thousands, groups, _, first_weekday] = std::array::from_fn(|at| if at == ORIGIN { Vec::new() } else { glibc.says(GLIBC[at]).to_vec() });
    let text = |said: &[u8]| String::from_utf8_lossy(said).into_owned();
    if dates {
        // (`%r` is the 12-hour time format.)
        let twelve_hours = glibc.text(T_FMT_AMPM);
        let time = text(&time).replace("%r", &twelve_hours);
        let date = glibc_pattern(&text(&date));
        (locale.date_order, locale.date_separator) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
        locale.date_digits = date_digits(&date);
        (locale.hour_digits, locale.time_separator) = time_details(&glibc_pattern(&time));
        locale.twelve_hour_clock = ["%I", "%l", "%p", "%P"].iter().any(|twelve| time.replace("%%", "").contains(twelve));
        locale.am_pm_place = am_pm_place(&glibc_pattern(&twelve_hours));
        // The week origin as days after a Sunday; and the week's first day, counted from 1 for
        // that one. SAFETY: for this item glibc answers with the number itself, never read as
        // text.
        let counted_from = unsafe { nl_langinfo_l(GLIBC[ORIGIN], glibc.0) } as usize as u32;
        let counted_from = Date::new((counted_from / 10_000) as i32, (counted_from / 100 % 100) as u8, (counted_from % 100) as u8);
        let after_sunday = (counted_from.weekday() as i32 + 1) % 7 + first_weekday.first().map_or(1, |day| i32::from(*day)) - 1;
        locale.first_weekday = (after_sunday + 6).rem_euclid(7) as u8;
    }
    if numbers {
        (locale.decimal_point, locale.thousands_separator) = (first(&text(&point)).unwrap_or('.'), first(&text(&thousands)));
        locale.group_sizes = glibc_groups(&groups);
    }
    locale
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

#[cfg(target_vendor = "apple")]
fn read() -> Locale {
    let _pool = crate::macos_sys::Pool::new();
    // SAFETY: the user's current locale, kept by Foundation.
    from_foundation(unsafe { crate::macos_sys::send(crate::macos_sys::class(c"NSLocale"), c"currentLocale", ()) })
}

/// Foundation's formats for `locale` (an `NSLocale`): the short date and the hour as the
/// locale writes them (patterns in the letters Windows uses), its calendar's first day (1 Sunday),
/// and a number formatter's separators, group sizes and zero. (Call within a pool.)
#[cfg(target_vendor = "apple")]
fn from_foundation(locale: crate::macos_sys::Obj) -> Locale {
    use crate::macos_sys::{Obj, class, ns_string, rust_string, send};
    // SAFETY: plain questions of the locale, a calendar and a number formatter made for it;
    // what is made is autoreleased into the caller's pool.
    let said = unsafe {
        let pattern = |template: &str| rust_string(send(class(c"NSDateFormatter"), c"dateFormatFromTemplate:options:locale:", (ns_string(template), 0usize, locale))).unwrap_or_default();
        let calendar: Obj = send(locale, c"objectForKey:", (crate::macos_sys::NSLocaleCalendar,));
        let first_day: isize = send(calendar, c"firstWeekday", ());
        let numbers: Obj = send(send::<Obj>(send::<Obj>(class(c"NSNumberFormatter"), c"alloc", ()), c"init", ()), c"autorelease", ());
        send::<()>(numbers, c"setLocale:", (locale,));
        send::<()>(numbers, c"setNumberStyle:", (1usize,));
        let [point, thousands] = [c"decimalSeparator", c"groupingSeparator"].map(|asked| rust_string(send(numbers, asked, ())).unwrap_or_default());
        let [size, then]: [isize; 2] = [c"groupingSize", c"secondaryGroupingSize"].map(|asked| send(numbers, asked, ()));
        let zero = rust_string(send(numbers, c"stringFromNumber:", (send::<Obj>(class(c"NSNumber"), c"numberWithInt:", (0i32,)),))).unwrap_or_default();
        (pattern("yMd"), pattern("jm"), first_day, point, thousands, [size, if then > 0 { then } else { size }], zero)
    };
    let (date, time, first_day, point, thousands, groups, zero) = said;
    let date = whole_years(&date);
    let (order, between) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    let (hour_digits, time_separator) = time_details(&time);
    Locale {
        date_order: order,
        date_separator: between,
        date_digits: date_digits(&date),
        first_weekday: ((first_day + 5).rem_euclid(7)) as u8,
        // Quoted text is literal; `h` and `K` are a 12-hour clock's hours.
        twelve_hour_clock: time.split('\'').step_by(2).any(|plain| plain.contains(['h', 'K'])),
        am_pm_place: am_pm_place(&time),
        hour_digits,
        time_separator,
        thousands_separator: first(&thousands),
        group_sizes: groups.map(|size| size.clamp(0, 9) as u8),
        decimal_point: first(&point).unwrap_or('.'),
        zero_digit: first(&zero).unwrap_or('0'),
    }
}

#[cfg(target_vendor = "apple")]
fn read_names() -> CalendarWords {
    let _pool = crate::macos_sys::Pool::new();
    // SAFETY: the user's current locale, kept by Foundation.
    names_from_foundation(unsafe { crate::macos_sys::send(crate::macos_sys::class(c"NSLocale"), c"currentLocale", ()) })
}

/// Foundation's standalone month names, short standalone day names (Monday first, without a
/// final point) and AM/PM for `locale`. (Call within a pool.)
#[cfg(target_vendor = "apple")]
fn names_from_foundation(locale: crate::macos_sys::Obj) -> CalendarWords {
    use crate::macos_sys::{Obj, array_items, class, rust_string, send};
    let mut words = CalendarWords::default();
    // SAFETY: a date formatter made for the locale and asked its names; autoreleased.
    let [months, days, halves] = unsafe {
        let dates: Obj = send(send::<Obj>(send::<Obj>(class(c"NSDateFormatter"), c"alloc", ()), c"init", ()), c"autorelease", ());
        send::<()>(dates, c"setLocale:", (locale,));
        let words = |asked: &std::ffi::CStr| array_items(send(dates, asked, ())).into_iter().map(|said| rust_string(said).unwrap_or_default()).collect::<Vec<String>>();
        let halves = [c"AMSymbol", c"PMSymbol"].map(|asked| rust_string(send(dates, asked, ())).unwrap_or_default());
        [words(c"standaloneMonthSymbols"), words(c"shortStandaloneWeekdaySymbols"), halves.to_vec()]
    };
    names(months.into_iter(), &mut words.months);
    names((0..7).map(|at| days.get((at + 1) % 7).map_or("", |day| day.trim_end_matches('.')).to_owned()), &mut words.days);
    names(halves.into_iter(), &mut words.halves);
    words
}

/// Group sizes of a CLDR number pattern, as Java writes it: "#,##0.###" threes, "#,##,##0.###"
/// three then twos, "0.###" none.
#[cfg(any(test, target_os = "android"))]
fn pattern_groups(pattern: &str) -> [u8; 2] {
    let whole = pattern.split([';', '.']).next().unwrap_or_default();
    let mut groups = whole.rsplit(',').map(|group| group.len().min(9) as u8);
    match (groups.next(), groups.next(), groups.next()) {
        (Some(_), None, _) | (None, ..) => [0, 0],
        (Some(last), Some(_), None) => [last, last],
        (Some(last), Some(before), Some(_)) => [last, before],
    }
}

#[cfg(target_os = "android")]
fn read() -> Locale {
    use crate::android_jni::{self as jni, Value};
    let Some(env) = jni::thread_env() else { return Locale::PLAIN };
    let context = crate::android::last_activity();
    env.frame(|| {
        let mut locale = from_java(env, java_locale(env));
        // The person's 12 or 24-hour setting, which may differ from the locale's.
        if !context.is_null() {
            locale.twelve_hour_clock = !env.call_static(c"android/text/format/DateFormat", c"is24HourFormat", c"(Landroid/content/Context;)Z", &[Value::obj(context)]).as_bool();
        }
        locale
    })
}

/// Android said the formats may have changed: with every configuration change (a turned screen
/// too) and every resume (the 24-hour setting is no configuration change). They count as
/// changed, and are read again when next needed, only where the locale (with its regional
/// preferences) or the formats differ from those seen last. Returns whether they did.
#[cfg(target_os = "android")]
pub(crate) fn read_again() -> bool {
    static SEEN: Mutex<Option<(String, Locale)>> = Mutex::new(None);
    let Some(env) = crate::android_jni::thread_env() else { return false };
    let now = (env.frame(|| env.text(java_locale(env))).unwrap_or_default(), read());
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    // (Nothing was read before the first notice: the activity resumes before it is drawn.)
    let differs = seen.as_ref().is_some_and(|seen| *seen != now);
    *seen = Some(now);
    if differs {
        changed();
    }
    differs
}

/// The person's locale: `Locale.getDefault()`.
#[cfg(target_os = "android")]
fn java_locale(env: crate::android_jni::Env) -> crate::android_jni::Obj {
    env.call_static(c"java/util/Locale", c"getDefault", c"()Ljava/util/Locale;", &[]).object()
}

/// Java's formats for `locale` (a `java.util.Locale`): the best short date and hour patterns
/// (in CLDR's letters, which are Windows' for these), `java.text`'s number symbols and pattern,
/// and the calendar's first weekday (1 Sunday). (Call within a frame.)
#[cfg(target_os = "android")]
fn from_java(env: crate::android_jni::Env, locale: crate::android_jni::Obj) -> Locale {
    use crate::android_jni::Value;
    let pattern = |skeleton: &str| {
        let args = [Value::obj(locale), Value::obj(env.string(skeleton))];
        env.text(env.call_static(c"android/text/format/DateFormat", c"getBestDateTimePattern", c"(Ljava/util/Locale;Ljava/lang/String;)Ljava/lang/String;", &args).object()).unwrap_or_default()
    };
    let (date, time) = (whole_years(&pattern("yMd")), pattern("jm"));
    let symbols = env.call_static(c"java/text/DecimalFormatSymbols", c"getInstance", c"(Ljava/util/Locale;)Ljava/text/DecimalFormatSymbols;", &[Value::obj(locale)]).object();
    let letter = |asked: &std::ffi::CStr| char::from_u32(env.call(symbols, asked, c"()C", &[]).as_int() as u32).filter(|letter| *letter != '\0');
    let numbers = env.call_static(c"java/text/NumberFormat", c"getInstance", c"(Ljava/util/Locale;)Ljava/text/NumberFormat;", &[Value::obj(locale)]).object();
    let number_pattern = env.text(env.call(numbers, c"toPattern", c"()Ljava/lang/String;", &[]).object()).unwrap_or_default();
    let calendar = env.call_static(c"java/util/Calendar", c"getInstance", c"(Ljava/util/Locale;)Ljava/util/Calendar;", &[Value::obj(locale)]).object();
    let first_day = env.call(calendar, c"getFirstDayOfWeek", c"()I", &[]).as_int();
    let (date_order, date_separator) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    let (hour_digits, time_separator) = time_details(&time);
    Locale {
        date_order,
        date_separator,
        date_digits: date_digits(&date),
        first_weekday: (first_day + 5).rem_euclid(7) as u8,
        // Quoted text is literal; `h` and `K` are a 12-hour clock's hours.
        twelve_hour_clock: time.split('\'').step_by(2).any(|plain| plain.contains(['h', 'K'])),
        am_pm_place: am_pm_place(&time),
        hour_digits,
        time_separator,
        thousands_separator: letter(c"getGroupingSeparator"),
        group_sizes: pattern_groups(&number_pattern),
        decimal_point: letter(c"getDecimalSeparator").unwrap_or('.'),
        zero_digit: letter(c"getZeroDigit").unwrap_or('0'),
    }
}

#[cfg(target_os = "android")]
fn read_names() -> CalendarWords {
    let Some(env) = crate::android_jni::thread_env() else { return CalendarWords::default() };
    env.frame(|| names_from_java(env, java_locale(env)))
}

/// ICU's standalone month names, short standalone day names (Monday first, without a final
/// point) and AM/PM for `locale`. (Call within a frame.)
#[cfg(target_os = "android")]
fn names_from_java(env: crate::android_jni::Env, locale: crate::android_jni::Obj) -> CalendarWords {
    use crate::android_jni::{self as jni, Value};
    let mut words = CalendarWords::default();
    let symbols = env.call_static(c"android/icu/text/DateFormatSymbols", c"getInstance", c"(Ljava/util/Locale;)Landroid/icu/text/DateFormatSymbols;", &[Value::obj(locale)]).object();
    let texts = |array: Value| jni::objects(env, array.object()).into_iter().map(|said| env.text(said).unwrap_or_default()).collect::<Vec<String>>();
    // (Contexts: 1 standalone; widths: 1 wide, 3 short.)
    let months = texts(env.call(symbols, c"getMonths", c"(II)[Ljava/lang/String;", &[Value::int(1), Value::int(1)]));
    let days = texts(env.call(symbols, c"getWeekdays", c"(II)[Ljava/lang/String;", &[Value::int(1), Value::int(3)]));
    let halves = texts(env.call(symbols, c"getAmPmStrings", c"()[Ljava/lang/String;", &[]));
    names(months.into_iter(), &mut words.months);
    // (Weekdays count from 1, Sunday; 0 is empty.)
    names((0..7).map(|at| days.get((at + 1) % 7 + 1).map_or("", |day| day.trim_end_matches('.')).to_owned()), &mut words.days);
    names(halves.into_iter(), &mut words.halves);
    words
}

/// Without Windows, Foundation, Android or glibc to ask: the library's formats.
#[cfg(not(any(windows, target_vendor = "apple", target_os = "android", all(target_os = "linux", target_env = "gnu"))))]
fn read() -> Locale {
    Locale::PLAIN
}

/// Without Windows, Foundation, Android or glibc to ask: English.
#[cfg(not(any(windows, target_vendor = "apple", target_os = "android", all(target_os = "linux", target_env = "gnu"))))]
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
        // As CLDR writes them (Apple, Android): `y` is the whole year.
        let cldr = ["M/d/y", "dd.MM.yy", "y/MM/dd", "d 'de' MMM 'y' y"];
        assert_eq!(cldr.map(whole_years), ["M/d/yyyy", "dd.MM.yy", "yyyy/MM/dd", "d 'de' MMM 'y' yyyy"]);
        assert_eq!(cldr.map(|pattern| date_digits(&whole_years(pattern))), [[1, 1, 4], [2, 2, 2], [2, 2, 4], [1, 2, 4]]);
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
    fn am_pm_is_placed_where_each_pattern_puts_it() {
        let (after, before, touching) = (AmPmPlace::After, AmPmPlace::Before, AmPmPlace::BeforeTouching);
        // Windows: the US, Korea, Taiwan; a 24-hour clock; quoted text is no AM/PM.
        assert_eq!(["h:mm:ss tt", "tt h:mm:ss", "tt hh:mm", "HH:mm", "H 'at' mm"].map(am_pm_place), [after, before, before, after, after]);
        // CLDR (Apple, Android): the US, Korea, Taiwan's flexible "B", Chinese.
        assert_eq!(["h:mm\u{202f}a", "a h:mm", "Bh:mm", "ah:mm"].map(am_pm_place), [after, before, touching, touching]);
        // glibc: en_US, ko_KR, ja_JP.
        assert_eq!(["%I:%M:%S %p", "%p %I시 %M분 %S초", "%p%I時%M分%S秒"].map(|pattern| am_pm_place(&glibc_pattern(pattern))), [after, before, touching]);
    }

    #[test]
    fn group_sizes_are_read_as_each_system_writes_them() {
        assert_eq!(["3;0", "3;2;0", "3", "0", ""].map(windows_groups), [[3, 3], [3, 2], [3, 0], [0, 0], [0, 0]]);
        assert_eq!([&[3u8, 3][..], &[3, 2], &[3], &[3, 127], &[]].map(glibc_groups), [[3, 3], [3, 2], [3, 3], [3, 0], [0, 0]]);
        assert_eq!(["#,##0.###", "#,##,##0.###", "0.###", "#,##0", "#,##0.###;-#,##0.###"].map(pattern_groups), [[3, 3], [3, 2], [0, 0], [3, 3], [3, 3]]);
    }

    #[test]
    fn numbers_are_written_and_read_with_the_marks_of_the_place() {
        let german = Locale { thousands_separator: Some('.'), decimal_point: ',', ..Locale::PLAIN };
        let american = Locale { thousands_separator: Some(','), ..Locale::PLAIN };
        let french = Locale { thousands_separator: Some('\u{202f}'), decimal_point: ',', ..Locale::PLAIN };
        let written = |locale: Locale, rust: &str, grouped: bool| {
            let mut into = String::new();
            locale.write_number(&mut into, rust, grouped);
            into
        };
        assert_eq!([written(german, "-1234567.25", true), written(american, "-1234567.25", true), written(german, "1234.5", false)], ["-1.234.567,25", "-1,234,567.25", "1234,5"]);
        assert_eq!([written(american, "123", true), written(american, "1000", true), written(french, "12345", true)], ["123", "1,000", "12\u{202f}345"]);
        assert_eq!([written(Locale::PLAIN, "NaN", true), written(american, "inf", true)], ["NaN", "inf"]);
        // Groups of other sizes: India's three then twos; one group only; none.
        let groups = |group_sizes: [u8; 2], rust: &str| written(Locale { group_sizes, ..american }, rust, true);
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
    }

    #[test]
    fn dates_and_hours_are_written_in_their_order_digits_and_marks() {
        let written = |locale: Locale, date: Date| {
            let mut into = String::new();
            locale.write_date(&mut into, date);
            into
        };
        let day_first = Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', ..Locale::PLAIN };
        let month_first = Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', ..Locale::PLAIN };
        let march = Date::new(2026, 3, 7);
        assert_eq!([written(Locale::PLAIN, march), written(day_first, march), written(month_first, march)], ["2026-03-07", "07.03.2026", "03/07/2026"]);
        // Fewer digits: the US's 3/7/2026, and two-digit years.
        let us = Locale { date_digits: [1, 1, 4], hour_digits: 1, ..month_first };
        let short = Locale { date_digits: [2, 2, 2], date_separator: '/', ..day_first };
        assert_eq!([written(us, march), written(us, Date::new(2026, 11, 30)), written(short, Date::new(2005, 3, 7))], ["3/7/2026", "11/30/2026", "07/03/05"]);
        let mut hours = String::new();
        us.write_digits(&mut hours, 9, usize::from(us.hour_digits));
        short.write_digits(&mut hours, 9, usize::from(short.hour_digits));
        assert_eq!(hours, "909");
    }

    #[test]
    fn digits_of_other_scripts_are_written_and_read() {
        let arabic = Locale { thousands_separator: Some('\u{66c}'), decimal_point: '\u{66b}', zero_digit: '\u{660}', ..Locale::PLAIN };
        let devanagari = Locale { thousands_separator: Some(','), group_sizes: [3, 2], zero_digit: '\u{966}', ..Locale::PLAIN };
        let mut written = String::new();
        arabic.write_number(&mut written, "-1234567.25", true);
        assert_eq!(written, "-١٬٢٣٤٬٥٦٧٫٢٥");
        written.clear();
        devanagari.write_number(&mut written, "1234567.5", true);
        assert_eq!(written, "१२,३४,५६७.५");
        written.clear();
        Locale { date_order: DateOrder::DayMonthYear, date_separator: '/', ..arabic }.write_date(&mut written, Date::new(2026, 3, 7));
        assert_eq!(written, "٠٧/٠٣/٢٠٢٦");
        // Typed in the locale's digits, in 0–9, or mixed; other scripts' digits are no number.
        assert_eq!([arabic.read_number::<f64>("١٬٢٣٤٫٥"), arabic.read_number("1234.5"), arabic.read_number("١2٣")], [Some(1234.5), Some(1234.5), Some(123.0)]);
        assert_eq!([devanagari.read_number::<i32>("१२,३४,५६७"), devanagari.read_number("٣"), Locale::PLAIN.read_number("१")], [Some(1234567), None, None]);
        assert_eq!([arabic.digit_value('٧'), arabic.digit_value('7'), arabic.digit_value('۷'), arabic.digit_value('a')], [Some(7), Some(7), None, None]);
    }

    #[test]
    fn a_window_takes_the_systems_formats_unless_the_app_set_its_own() {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        // (Not the clock, which another test sets meanwhile.)
        let without_clock = |locale: Locale| Locale { twelve_hour_clock: false, ..locale };
        assert_eq!(without_clock(ui.locale_in_use()), without_clock(system()));
        ui.locale = Locale::named("hi-IN");
        assert_eq!((ui.locale_in_use().group_sizes, ui.format_number(1234567.5, 1)), ([3, 2], "12,34,567.5".to_owned()));
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
        assert_eq!(Locale::of_the_system(), from_windows(std::ptr::null(), display_reads_right_to_left()));
    }

    /// GNOME's clock setting, as the portal reports it, overrides the locale's; each change has
    /// the formats read again.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn the_desktops_clock_setting_is_followed() {
        let american = Locale::named("en-US").unwrap();
        assert_eq!([0, 1, 2].map(|said| with_clock(american, said).twelve_hour_clock), [true, true, false]);
        assert_eq!(with_clock(Locale::PLAIN, 1), Locale { twelve_hour_clock: true, ..Locale::PLAIN });
        let before = CHANGES.load(Ordering::Relaxed);
        assert_eq!([Some(true), Some(true), Some(false), None].map(set_desktop_clock), [true, false, true, true]);
        assert!(CHANGES.load(Ordering::Relaxed) >= before + 3);
    }

    /// glibc answers for locales by name: American English, always installed, and others
    /// where a test provides them (compiled by `localedef` into a folder `LOCPATH` names).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_says_how_places_write_dates_times_and_numbers() {
        let us = from_glibc(c"en_US.UTF-8", [true, true]);
        let american = Locale::named("en-US").map(|cldr| Locale { date_digits: [2, 2, 4], hour_digits: 2, ..cldr });
        assert_eq!(Some(us), american, "CLDR's but for two-digit days, months and hours");
        assert_eq!(from_glibc(c"xx_NOWHERE.UTF-8", [true, true]), Locale::PLAIN, "a locale that is not there");
        assert_eq!(from_glibc(c"en_US.UTF-8", [false, true]), Locale { thousands_separator: Some(','), ..Locale::PLAIN });
        let names = names_from_glibc(c"en_US.UTF-8");
        assert_eq!((names.months[10], names.days, names.halves), ("November", ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], ["AM", "PM"]));
        assert_eq!(names_from_glibc(c"xx_NOWHERE.UTF-8"), CalendarWords::default());
        if std::env::var_os("LOCPATH").is_none() {
            return;
        }
        let germany = from_glibc(c"de_DE.UTF-8", [true, true]);
        assert_eq!(Some(germany), Locale::named("de-DE"));
        let sweden = from_glibc(c"sv_SE.UTF-8", [true, true]);
        assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
        assert_eq!(from_glibc(c"en_IN.UTF-8", [true, true]).group_sizes, [3, 2]);
        assert_eq!(from_glibc(c"fi_FI.UTF-8", [true, true]).time_separator, '.');
        let korea = from_glibc(c"ko_KR.UTF-8", [true, true]);
        assert_eq!((korea.twelve_hour_clock, korea.am_pm_place), (false, AmPmPlace::Before), "24 hours, but AM/PM first in its 12-hour format");
        assert_eq!(from_glibc(c"ja_JP.UTF-8", [true, true]).am_pm_place, AmPmPlace::BeforeTouching);
        let german = names_from_glibc(c"de_DE.UTF-8");
        assert_eq!((german.months[2], german.days[0], german.days[6]), ("März", "Mo", "So"));
        let russian = names_from_glibc(c"ru_RU.UTF-8");
        assert_eq!(russian.months[0].to_lowercase(), "январь", "the month's own name, not 'of January'");
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

    /// Foundation answers for locales by name: taut writes a number, a date and a time with
    /// what it reads as Foundation's own formatters write them (spaces and direction marks set
    /// aside). Dates where the place's calendar is not Gregorian (Persian), or written with
    /// spaces (Korean), are not compared.
    #[cfg(target_os = "macos")]
    #[test]
    fn foundation_says_how_places_write_dates_times_and_numbers() {
        use crate::macos_sys::{Obj, class, ns_string, rust_string, send};
        let _pool = crate::macos_sys::Pool::new();
        let plain = |text: Option<String>| text.unwrap_or_default().replace(['\u{202f}', '\u{a0}'], " ").replace(['\u{200e}', '\u{200f}', '\u{61c}'], "");
        let (day, time) = (Date::new(2026, 3, 7), crate::clock::Time::new(14, 5, 0));
        let seconds = (day.days() * 86_400 + i64::from(time.seconds())) as f64;
        for name in ["en_US", "de_DE", "fr_FR", "sv_SE", "hi_IN", "bn_BD", "ar_EG", "ja_JP", "zh_TW", "ko_KR", "fa_IR"] {
            // SAFETY: a locale and formatters made for it, autoreleased into the pool.
            let (locale, theirs) = unsafe {
                let locale: Obj = send(class(c"NSLocale"), c"localeWithLocaleIdentifier:", (ns_string(name),));
                let numbers: Obj = send(send::<Obj>(class(c"NSNumberFormatter"), c"new", ()), c"autorelease", ());
                send::<()>(numbers, c"setLocale:", (locale,));
                send::<()>(numbers, c"setNumberStyle:", (1usize,));
                send::<()>(numbers, c"setMinimumFractionDigits:", (1usize,));
                let number = rust_string(send(numbers, c"stringFromNumber:", (send::<Obj>(class(c"NSNumber"), c"numberWithDouble:", (1_234_567.5f64,)),)));
                let dates: Obj = send(send::<Obj>(class(c"NSDateFormatter"), c"new", ()), c"autorelease", ());
                send::<()>(dates, c"setLocale:", (locale,));
                send::<()>(dates, c"setTimeZone:", (send::<Obj>(class(c"NSTimeZone"), c"timeZoneForSecondsFromGMT:", (0isize,)),));
                let at: Obj = send(class(c"NSDate"), c"dateWithTimeIntervalSince1970:", (seconds,));
                let written = |template: &str| {
                    let pattern: Obj = send(class(c"NSDateFormatter"), c"dateFormatFromTemplate:options:locale:", (ns_string(template), 0usize, locale));
                    send::<()>(dates, c"setDateFormat:", (pattern,));
                    rust_string(send(dates, c"stringFromDate:", (at,)))
                };
                (locale, [number, written("yMd"), written("jm")].map(plain))
            };
            let ours = from_foundation(locale);
            let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
            (ui.locale, ui.calendar_words) = (Some(ours), names_from_foundation(locale));
            let written = [ui.format_number(1_234_567.5, 1), ui.format_date(day), ui.format_time(time, false)].map(|text| plain(Some(text)));
            assert_eq!(written[0], theirs[0], "{name}'s number");
            if !["ko_KR", "fa_IR"].contains(&name) {
                assert_eq!(written[1], theirs[1], "{name}'s date");
            }
            assert_eq!(written[2], theirs[2], "{name}'s time");
        }
        // The week's first day, from each place's calendar.
        // SAFETY: a locale made by name, autoreleased into the pool.
        let first = |name: &str| from_foundation(unsafe { send(class(c"NSLocale"), c"localeWithLocaleIdentifier:", (ns_string(name),)) }).first_weekday;
        assert_eq!([first("en_US"), first("de_DE"), first("ar_EG")], [6, 0, 5]);
        // And the user's own reads without trouble, whatever it is.
        assert!(Locale::of_the_system().first_weekday < 7 && CalendarWords::of_the_system().months.iter().all(|month| !month.is_empty()));
    }

    /// Windows answers for locales by name, so the user's settings are never touched. Its
    /// formats match CLDR's but where the two differ (Windows writes years in full, and India
    /// and some others in different digit counts and separators).
    #[cfg(windows)]
    #[test]
    fn windows_says_how_places_write_dates_times_and_numbers() {
        let named = |name: &str| name.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let of = |name: &str| from_windows(named(name).as_ptr(), false);
        // Where Windows writes as CLDR does, the two agree field for field.
        for name in ["en-US", "en-GB", "de-DE", "fr-FR", "it-IT", "pt-BR", "pt-PT", "ja-JP", "sv-SE", "cs-CZ", "fi-FI", "tr-TR", "pl-PL"] {
            assert_eq!(Some(of(name)), Locale::named(name), "{name}");
        }
        let us = of("en-US");
        let sweden = of("sv-SE");
        assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
        let japan = of("ja-JP");
        assert_eq!((japan.date_order, japan.date_separator, japan.first_weekday, japan.twelve_hour_clock), (DateOrder::YearMonthDay, '/', 6, false));
        assert_eq!(of("hi-IN").group_sizes, [3, 2], "India groups by three, then two");
        assert_eq!(of("fi-FI").time_separator, '.');
        // AM/PM first in Korea and Taiwan.
        assert_eq!([of("ko-KR"), of("zh-TW"), us].map(|locale| (locale.twelve_hour_clock, locale.am_pm_place)), [(true, AmPmPlace::Before), (true, AmPmPlace::Before), (true, AmPmPlace::After)]);
        // Native digits: Bengali always; Arabic by context, so where the display language reads
        // right to left; never in Hindi.
        let rtl = |name: &str| from_windows(named(name).as_ptr(), true).zero_digit;
        assert_eq!([of("bn-BD").zero_digit, of("ar-SA").zero_digit, rtl("ar-SA"), rtl("fa-IR"), rtl("hi-IN"), rtl("de-DE")], ['\u{9e6}', '0', '\u{660}', '\u{6f0}', '0', '0']);
        // Names, in the place's own language.
        let names = |name: &str| names_from_windows(named(name).as_ptr());
        let (english, german, french) = (names("en-US"), names("de-DE"), names("fr-FR"));
        assert_eq!((english.months[10], english.days, english.halves), ("November", ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"], ["AM", "PM"]));
        assert_eq!((german.months[2], german.days[0], german.days[6]), ("März", "Mo", "So"));
        assert_eq!((french.months[7], french.days[0]), ("août", "lu"));
        assert_eq!(names("ru-RU").months[0].to_lowercase(), "январь", "the month's own name, not 'of January'");
        // The translations' month names (CLDR's) are Windows' own, but for case and where the two
        // write them differently (Hindi, Vietnamese, traditional Chinese).
        #[cfg(feature = "translations")]
        for (language, place) in
            [("it", "it-IT"), ("ru", "ru-RU"), ("ko", "ko-KR"), ("ar", "ar-EG"), ("tr", "tr-TR"), ("pl", "pl-PL"), ("nl", "nl-NL"), ("id", "id-ID"), ("th", "th-TH"), ("uk", "uk-UA"), ("sv", "sv-SE")]
        {
            let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
            assert!(ui.translate(language));
            let lower = |months: [&str; 12]| months.map(str::to_lowercase);
            assert_eq!(lower(ui.calendar_words.months), lower(names(place).months), "{language}");
        }
        // And the user's own reads without trouble, whatever it is.
        let own = Locale::of_the_system();
        assert!(own.first_weekday < 7 && own.decimal_point != '\0', "{own:?}");
        assert!(CalendarWords::of_the_system().months.iter().all(|month| !month.is_empty()));
    }
}
