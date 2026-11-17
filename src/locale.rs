//! The user's date, time and number formats: date order and separator, first day of the week,
//! 12 or 24-hour clock, and thousands and decimal separators.
//!
//! Read once per program, when first needed: on Windows from the user's region settings; on
//! Linux from the locale the environment names (`LC_ALL`, else `LC_TIME` for dates and
//! `LC_NUMERIC` for numbers, else `LANG`), through glibc's locale data. With none named, or C's
//! ("C", "POSIX", "C.UTF-8"), and on other systems: [`Locale::PLAIN`]. An app can set its own
//! in [`ui.locale`](Ui::locale).
//!
//! Not read, to stay small: month and day names and AM/PM words (the app's, in
//! [`ui.calendar_words`](Ui::calendar_words)); single-digit days and months (two digits always,
//! four for the year); the time separator (always ":"); non-ASCII digits; groups other than
//! three; and settings changed while the program runs.
use crate::date::Date;
use crate::number::Number;
use crate::ui::Ui;
use std::fmt::Write;

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

impl Locale {
    /// The library's own, the same everywhere: 2026-11-30, 14:05, 1234.5, weeks from Monday.
    pub const PLAIN: Locale = Locale { date_order: DateOrder::YearMonthDay, date_separator: '-', first_weekday: 0, twelve_hour_clock: false, thousands_separator: None, decimal_point: '.' };

    /// The system's formats for the user. Read once, when first asked for.
    pub fn of_the_system() -> Locale {
        static READ: std::sync::OnceLock<Locale> = std::sync::OnceLock::new();
        *READ.get_or_init(read)
    }

    /// Writes a date in digits, in this order and with this separator.
    pub(crate) fn write_date(self, into: &mut String, date: Date) {
        let (day, month, year, between) = (date.day, date.month, date.year, self.date_separator);
        let _ = match self.date_order {
            DateOrder::YearMonthDay => write!(into, "{year:04}{between}{month:02}{between}{day:02}"),
            DateOrder::DayMonthYear => write!(into, "{day:02}{between}{month:02}{between}{year:04}"),
            DateOrder::MonthDayYear => write!(into, "{month:02}{between}{day:02}{between}{year:04}"),
        };
    }

    /// Rewrites a number Rust formatted ("-1234.5") the user's way: their decimal separator,
    /// and if `grouped` (for reading, not typing), their thousands separator.
    pub(crate) fn write_number(self, into: &mut String, rust: &str, grouped: bool) {
        let (whole, fraction) = rust.split_once('.').map_or((rust, None), |(whole, fraction)| (whole, Some(fraction)));
        let digits = whole.trim_start_matches('-');
        into.push_str(&whole[..whole.len() - digits.len()]);
        let mark = self.thousands_separator.filter(|_| grouped && digits.bytes().all(|digit| digit.is_ascii_digit()));
        for (at, digit) in digits.char_indices() {
            if let Some(mark) = mark.filter(|_| at > 0 && (digits.len() - at).is_multiple_of(3)) {
                into.push(mark);
            }
            into.push(digit);
        }
        if let Some(fraction) = fraction {
            into.push(self.decimal_point);
            into.push_str(fraction);
        }
    }

    /// Parses typed text as the user writes numbers and as Rust does: spaces are dropped, as is
    /// a thousands separator followed by three digits; the user's separator or a point marks
    /// the fraction (so "1.5" is one and a half everywhere, and "1.500" is 1500 where a point
    /// groups thousands). `None` if it is not a number of this type.
    pub(crate) fn read_number<T: Number>(self, typed: &str) -> Option<T> {
        self.as_rust_writes(typed).parse::<T>().ok().filter(|number| number.to_f64().is_finite())
    }
    /// Typed text rewritten as Rust writes numbers, for Rust to parse. (Separate from parsing,
    /// so it exists once in a program whatever number types it reads.)
    fn as_rust_writes(self, typed: &str) -> String {
        let letters: Vec<char> = typed.trim().chars().filter(|letter| !letter.is_whitespace()).collect();
        let mut plain = String::with_capacity(letters.len());
        for (at, letter) in letters.iter().enumerate() {
            let three = letters[at + 1..].iter().take_while(|next| next.is_ascii_digit()).count() == 3;
            match *letter {
                letter if Some(letter) == self.thousands_separator && three => {}
                letter if letter == self.decimal_point => plain.push('.'),
                letter => plain.push(letter),
            }
        }
        plain
    }
}

/// Dates and numbers, the user's way.
impl Ui {
    /// This window's formats: the app's, if set in [`ui.locale`](Ui::locale), else the
    /// system's.
    pub(crate) fn locale_in_use(&self) -> Locale {
        self.locale.unwrap_or_else(Locale::of_the_system)
    }
    /// A date in this window's format: "30.11.2026" where the system puts the day first.
    /// (`date.to_string()` writes the sortable form.)
    pub fn format_date(&self, date: Date) -> String {
        let mut written = String::new();
        self.locale_in_use().write_date(&mut written, date);
        written
    }
    /// A number for reading in the user's format, with this many decimals: "1,234.5", "1.234,5"
    /// or "1 234,5".
    pub fn format_number(&self, value: f64, decimals: usize) -> String {
        let (mut written, rust) = (String::new(), format!("{value:.decimals$}"));
        self.locale_in_use().write_number(&mut written, &rust, true);
        written
    }
}

/// Date order and separator from a pattern where `d` is the day, `M` the month, `y` the year,
/// and quoted text is literal (Windows' format). `ddd` and longer is the weekday name, not the
/// day. `None` if one of the three is missing.
fn date_order(pattern: &str) -> Option<(DateOrder, char)> {
    // Quoted text is removed first; then the first of each.
    let plain: String = pattern.split('\'').step_by(2).collect();
    let day = plain.match_indices('d').map(|(at, _)| at).find(|at| plain[*at..].bytes().take_while(|letter| *letter == b'd').count() < 3 && (*at == 0 || plain.as_bytes()[at - 1] != b'd'))?;
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

/// The same pattern as glibc writes it: `%d` and `%e` the day, `%m`, `%b`, `%B` and `%h` the
/// month, `%Y`, `%y`, `%C` and `%G` the year, `%D` and `%F` whole dates.
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
fn glibc_pattern(written: &str) -> String {
    let (mut pattern, mut letters) = (String::new(), written.replace("%D", "%m/%d/%y").replace("%F", "%Y-%m-%d").into_bytes().into_iter());
    while let Some(letter) = letters.next() {
        // Skipping padding flags between `%` and the conversion letter.
        let conversion = (letter == b'%').then(|| letters.by_ref().find(|letter| !b"-_0^#".contains(letter))).flatten();
        let as_windows = match conversion {
            None => letter as char,
            Some(b'd' | b'e') => 'd',
            Some(b'm' | b'b' | b'B' | b'h') => 'M',
            Some(b'Y' | b'y' | b'C' | b'G') => 'y',
            Some(_) => ' ',
        };
        pattern.push(as_windows);
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
/// week (0 Monday), decimal separator, thousands separator.
#[cfg(windows)]
const ASKED: [u32; 5] = [0x1F, 0x1003, 0x100C, 0x0E, 0x0F];

#[cfg(windows)]
fn read() -> Locale {
    from_windows(std::ptr::null())
}

/// Windows' formats for locale `name` ("de-DE", NUL-terminated), or the user's if null.
#[cfg(windows)]
fn from_windows(name: *const u16) -> Locale {
    let [date, time, first_day, point, thousands] = ASKED.map(|what| {
        let mut into = [0u16; 80];
        // SAFETY: the system writes at most the room it is told of, which exists; the name is
        // null or NUL-terminated.
        let wrote = unsafe { GetLocaleInfoEx(name, what, into.as_mut_ptr(), into.len() as i32) };
        String::from_utf16_lossy(&into[..wrote.max(1) as usize - 1])
    });
    let (order, between) = date_order(&date).unwrap_or((Locale::PLAIN.date_order, Locale::PLAIN.date_separator));
    // Quoted text in a time pattern is literal: "HH 'h' mm" is a 24-hour clock.
    let twelve_hours = time.split('\'').step_by(2).any(|plain| plain.contains('h'));
    let first_day = first_day.trim().parse::<u8>().ok().filter(|day| *day < 7).unwrap_or(0);
    Locale {
        date_order: order,
        date_separator: between,
        first_weekday: first_day,
        twelve_hour_clock: twelve_hours,
        thousands_separator: first(&thousands),
        decimal_point: first(&point).unwrap_or('.'),
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn newlocale(categories: i32, name: *const std::ffi::c_char, base: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn nl_langinfo_l(item: i32, locale: *mut std::ffi::c_void) -> *const std::ffi::c_char;
    fn freelocale(locale: *mut std::ffi::c_void);
}

/// What glibc is asked about a locale (`langinfo.h`): date format, time format, decimal
/// separator, thousands separator; the week's origin date (19971130, a Sunday), and the first
/// weekday counted from it (1 for that day).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const GLIBC: [i32; 6] = [0x20029, 0x2002a, 0x10000, 0x10001, 0x20066, 0x20068];

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

/// glibc's formats for locale `name`, dates and times and numbers each where asked; for "", the
/// environment's locales. The plain formats if the locale isn't installed.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn from_glibc(name: &std::ffi::CStr, [dates, numbers]: [bool; 2]) -> Locale {
    // LC_TIME's mask is 4 and LC_NUMERIC's 2. SAFETY: glibc makes a locale from a
    // NUL-terminated name, or none.
    let locale = unsafe { newlocale(4 | 2, name.as_ptr(), std::ptr::null_mut()) };
    if locale.is_null() {
        return Locale::PLAIN;
    }
    // SAFETY: for a live locale glibc returns NUL-terminated text; for the week origin (the
    // fifth item), the number itself in place of a pointer, never read as text.
    let said = GLIBC.map(|item| unsafe { nl_langinfo_l(item, locale) });
    let text = |at: usize| match said[at].is_null() || at == 4 {
        true => String::new(),
        false => unsafe { std::ffi::CStr::from_ptr(said[at]) }.to_string_lossy().into_owned(),
    };
    let [date, time, point, thousands, _, first_weekday] = std::array::from_fn(text);
    // SAFETY: the locale is freed once, after everything was read from it.
    unsafe { freelocale(locale) };
    let mut locale = Locale::PLAIN;
    if dates {
        (locale.date_order, locale.date_separator) = date_order(&glibc_pattern(&date)).unwrap_or((locale.date_order, locale.date_separator));
        locale.twelve_hour_clock = ["%I", "%l", "%r", "%p", "%P"].iter().any(|twelve| time.replace("%%", "").contains(twelve));
        // The week origin as days after a Sunday; and the week's first day, counted from 1 for
        // that one.
        let counted_from = said[4] as usize as u32;
        let counted_from = Date::new((counted_from / 10_000) as i32, (counted_from / 100 % 100) as u8, (counted_from % 100) as u8);
        let after_sunday = (counted_from.weekday() as i32 + 1) % 7 + first_weekday.bytes().next().map_or(1, i32::from) - 1;
        locale.first_weekday = (after_sunday + 6).rem_euclid(7) as u8;
    }
    if numbers {
        (locale.decimal_point, locale.thousands_separator) = (first(&point).unwrap_or('.'), first(&thousands));
    }
    locale
}

/// Without Windows or glibc to ask: the library's formats.
#[cfg(not(any(windows, all(target_os = "linux", target_env = "gnu"))))]
fn read() -> Locale {
    Locale::PLAIN
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
        // As glibc writes them (en_US, de_DE, en_GB, nb_NO, ja_JP, hi_IN, and whole dates).
        let glibc = ["%m/%d/%Y", "%d.%m.%Y", "%d/%m/%y", "%d. %b %Y", "%Y年%m月%d日", "%-d/%-m/%y", "%D", "%F"];
        let read: Vec<_> = glibc.iter().map(|pattern| date_order(&glibc_pattern(pattern))).collect();
        let wanted = [(mdy, '/'), (dmy, '.'), (dmy, '/'), (dmy, '.'), (ymd, '/'), (dmy, '/'), (mdy, '/'), (ymd, '-')];
        assert_eq!(read, wanted.map(Some));
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
        // Parsed back: the user's separators, and Rust's point anywhere.
        assert_eq!([german.read_number::<f64>("1.234,5"), german.read_number("1,5"), german.read_number("1.5"), german.read_number(" 1.500 ")], [Some(1234.5), Some(1.5), Some(1.5), Some(1500.0)]);
        assert_eq!(
            [american.read_number::<f64>("1,234.5"), american.read_number("1,5"), french.read_number("12\u{202f}345,5"), french.read_number("12 345")],
            [Some(1234.5), None, Some(12345.5), Some(12345.0)]
        );
        assert_eq!([Locale::PLAIN.read_number::<i32>("-42"), Locale::PLAIN.read_number("4.2"), Locale::PLAIN.read_number("x")], [Some(-42), None, None]);
        assert_eq!([Locale::PLAIN.read_number::<f32>("inf"), Locale::PLAIN.read_number("NaN")], [None, None]);
        assert_eq!(Locale::PLAIN.read_number::<u8>("-1"), None);
        let mut date = String::new();
        for locale in
            [Locale::PLAIN, Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', ..Locale::PLAIN }, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', ..Locale::PLAIN }]
        {
            locale.write_date(&mut date, Date::new(2026, 3, 7));
            date.push(' ');
        }
        assert_eq!(date, "2026-03-07 07.03.2026 03/07/2026 ");
    }

    /// glibc answers for locales by name: American English, always installed, and Germany and
    /// Sweden where a test provides them (compiled by `localedef` into a folder `LOCPATH`
    /// names).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_says_how_places_write_dates_times_and_numbers() {
        let us = from_glibc(c"en_US.UTF-8", [true, true]);
        assert_eq!(us, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', first_weekday: 6, twelve_hour_clock: true, thousands_separator: Some(','), decimal_point: '.' });
        assert_eq!(from_glibc(c"xx_NOWHERE.UTF-8", [true, true]), Locale::PLAIN, "a locale that is not there");
        assert_eq!(from_glibc(c"en_US.UTF-8", [false, true]), Locale { thousands_separator: Some(','), ..Locale::PLAIN });
        if std::env::var_os("LOCPATH").is_some() {
            let germany = Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', first_weekday: 0, twelve_hour_clock: false, thousands_separator: Some('.'), decimal_point: ',' };
            assert_eq!(from_glibc(c"de_DE.UTF-8", [true, true]), germany);
            let sweden = from_glibc(c"sv_SE.UTF-8", [true, true]);
            assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
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
        assert!(american.contains("MonthDayYear") && american.contains("twelve_hour_clock: true"), "{american}");
        // Dates as C writes them, numbers as the US does.
        let mixed = asked(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "C.UTF-8")]);
        assert!(mixed.contains("YearMonthDay") && mixed.contains("thousands_separator: Some(',')"), "{mixed}");
    }

    /// The child of the test above: prints what the system said.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    #[ignore]
    fn what_the_environment_names() {
        println!("{:?}", Locale::of_the_system());
    }

    /// Windows answers for locales by name, so the user's settings are never touched.
    #[cfg(windows)]
    #[test]
    fn windows_says_how_places_write_dates_times_and_numbers() {
        let of = |name: &str| from_windows(name.encode_utf16().chain([0]).collect::<Vec<u16>>().as_ptr());
        let (us, germany, sweden, japan) = (of("en-US"), of("de-DE"), of("sv-SE"), of("ja-JP"));
        assert_eq!(us, Locale { date_order: DateOrder::MonthDayYear, date_separator: '/', first_weekday: 6, twelve_hour_clock: true, thousands_separator: Some(','), decimal_point: '.' });
        assert_eq!(germany, Locale { date_order: DateOrder::DayMonthYear, date_separator: '.', first_weekday: 0, twelve_hour_clock: false, thousands_separator: Some('.'), decimal_point: ',' });
        assert_eq!((sweden.date_order, sweden.date_separator, sweden.first_weekday, sweden.twelve_hour_clock, sweden.decimal_point), (DateOrder::YearMonthDay, '-', 0, false, ','));
        assert_eq!((japan.date_order, japan.date_separator, japan.first_weekday, japan.twelve_hour_clock), (DateOrder::YearMonthDay, '/', 6, false));
        // And the user's own reads without trouble, whatever it is.
        let own = Locale::of_the_system();
        assert!(own.first_weekday < 7 && own.decimal_point != '\0', "{own:?}");
    }
}
