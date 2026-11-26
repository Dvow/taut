//! Named locales: the formats CLDR 49 gives 34 places, for an app to set as its own
//! ([`Locale::named`]). Compiled into a program only when it asks for one.
//!
//! As CLDR's short date and time patterns, decimal pattern, symbols, default digits and first
//! weekday have them, with two changes: years are written in full, and a plain space goes
//! between a time and AM/PM.
use crate::locale::{AmPmPlace, DateOrder, Locale};

use AmPmPlace::{After, Before, BeforeTouching};
use DateOrder::{DayMonthYear as Dmy, MonthDayYear as Mdy, YearMonthDay as Ymd};

/// Each place as `language-REGION`: a language's first row is where the language alone goes.
#[rustfmt::skip]
const LOCALES: [(&str, Locale); 34] = [
    ("en-US", Locale { date_order: Mdy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("en-GB", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("en-IN", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 2], decimal_point: '.', zero_digit: '0' }),
    ("de-DE", Locale { date_order: Dmy, date_separator: '.', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("fr-FR", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{202f}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("es-ES", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("es-MX", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("it-IT", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("pt-BR", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("pt-PT", Locale { date_order: Dmy, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("ru-RU", Locale { date_order: Dmy, date_separator: '.', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("ja-JP", Locale { date_order: Ymd, date_separator: '/', date_digits: [2, 2, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("zh-CN", Locale { date_order: Ymd, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("zh-TW", Locale { date_order: Ymd, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: BeforeTouching, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("ko-KR", Locale { date_order: Ymd, date_separator: '.', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: Before, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("ar-EG", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 5, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some('\u{66c}'), group_sizes: [3, 3], decimal_point: '\u{66b}', zero_digit: '\u{660}' }),
    ("ar-SA", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some('\u{66c}'), group_sizes: [3, 3], decimal_point: '\u{66b}', zero_digit: '\u{660}' }),
    ("fa-IR", Locale { date_order: Ymd, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 5, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some('\u{66c}'), group_sizes: [3, 3], decimal_point: '\u{66b}', zero_digit: '\u{6f0}' }),
    ("hi-IN", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 2], decimal_point: '.', zero_digit: '0' }),
    ("bn-BD", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 2], decimal_point: '.', zero_digit: '\u{9e6}' }),
    ("mr-IN", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: true, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 2], decimal_point: '.', zero_digit: '\u{966}' }),
    ("ne-NP", Locale { date_order: Ymd, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 2], decimal_point: '.', zero_digit: '\u{966}' }),
    ("th-TH", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("tr-TR", Locale { date_order: Dmy, date_separator: '.', date_digits: [1, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("pl-PL", Locale { date_order: Dmy, date_separator: '.', date_digits: [1, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("nl-NL", Locale { date_order: Dmy, date_separator: '-', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("id-ID", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: '.', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("vi-VN", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('.'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("uk-UA", Locale { date_order: Dmy, date_separator: '.', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("sv-SE", Locale { date_order: Ymd, date_separator: '-', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 2, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("cs-CZ", Locale { date_order: Dmy, date_separator: '.', date_digits: [2, 2, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("he-IL", Locale { date_order: Dmy, date_separator: '.', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '0' }),
    ("fi-FI", Locale { date_order: Dmy, date_separator: '.', date_digits: [1, 1, 4], first_weekday: 0, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: '.', thousands_separator: Some('\u{a0}'), group_sizes: [3, 3], decimal_point: ',', zero_digit: '0' }),
    ("my-MM", Locale { date_order: Dmy, date_separator: '/', date_digits: [1, 1, 4], first_weekday: 6, twelve_hour_clock: false, am_pm_place: After, hour_digits: 1, time_separator: ':', thousands_separator: Some(','), group_sizes: [3, 3], decimal_point: '.', zero_digit: '\u{1040}' }),
];

impl Locale {
    /// CLDR's formats for a place, as systems name it ("de-DE", "hi_IN.UTF-8", "ar-EG"), or for
    /// a language alone ("de": Germany's). Traditional Chinese ("zh-Hant", "zh-HK") is
    /// Taiwan's. `None` for a place taut lacks. The places: en-US, en-GB, en-IN, de-DE, fr-FR,
    /// es-ES, es-MX, it-IT, pt-BR, pt-PT, ru-RU, ja-JP, zh-CN, zh-TW, ko-KR, ar-EG, ar-SA,
    /// fa-IR, hi-IN, bn-BD, mr-IN, ne-NP, th-TH, tr-TR, pl-PL, nl-NL, id-ID, vi-VN, uk-UA,
    /// sv-SE, cs-CZ, he-IL, fi-FI, my-MM.
    pub fn named(name: &str) -> Option<Locale> {
        let (language, region) = language_and_region(name);
        let of_language = |(code, _): &&(&str, Locale)| code.split('-').next() == Some(language.as_str());
        let exact = region.and_then(|region| LOCALES.iter().filter(of_language).find(|(code, _)| code.ends_with(&region)));
        exact.or_else(|| LOCALES.iter().find(of_language)).map(|(_, locale)| *locale)
    }
}

/// A locale's name as systems write it ("de-DE", "hi_IN.UTF-8", "zh-Hant-HK") in parts: its
/// language in lower case, and its region in upper case where it names one. Traditional
/// Chinese (Hong Kong's and Macao's too) is Taiwan's.
pub(crate) fn language_and_region(name: &str) -> (String, Option<String>) {
    let mut parts = name.split(['-', '_', '.', '@']);
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest: Vec<String> = parts.map(str::to_ascii_uppercase).collect();
    let traditional = language == "zh" && rest.iter().any(|part| ["HANT", "TW", "HK", "MO"].contains(&part.as_str()));
    let region = if traditional { Some("TW".to_owned()) } else { rest.into_iter().find(|part| part.len() == 2) };
    (language, region)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_found_by_the_names_systems_give_them() {
        let region = |name: &str| Locale::named(name).map(|locale| (locale.date_order, locale.group_sizes, locale.zero_digit));
        assert_eq!(region("hi-IN"), Some((Dmy, [3, 2], '0')));
        assert_eq!(region("hi_IN.UTF-8"), region("hi-IN"));
        assert_eq!(region("HI"), region("hi-IN"), "a language alone, in any case");
        assert_eq!([region("ar"), region("ar-SA")].map(|found| found.map(|(.., zero)| zero)), [Some('\u{660}'); 2]);
        assert_eq!(Locale::named("en").map(|locale| locale.date_order), Some(Mdy));
        assert_eq!(Locale::named("en-CA"), Locale::named("en-US"), "an unknown region of a known language");
        assert_eq!(Locale::named("zh-Hant-HK"), Locale::named("zh-TW"));
        assert_eq!(Locale::named("zh_SG"), Locale::named("zh-CN"));
        assert_eq!([Locale::named("tlh"), Locale::named(""), Locale::named("-DE")], [None; 3]);
    }

    #[test]
    fn every_place_is_one_language_and_region_with_sound_formats() {
        for (code, locale) in LOCALES {
            let (language, region) = code.split_once('-').unwrap();
            assert!(language.len() == 2 && region.len() == 2 && region.bytes().all(|letter| letter.is_ascii_uppercase()), "{code}");
            assert_eq!(Locale::named(code), Some(locale), "{code} is found by its own name");
            assert!(locale.date_digits[..2].iter().all(|digits| (1..=2).contains(digits)) && locale.date_digits[2] == 4, "{code}");
            assert!((1..=2).contains(&locale.hour_digits) && locale.first_weekday < 7 && locale.group_sizes[0] > 0, "{code}");
            assert!(['0', '\u{660}', '\u{6f0}', '\u{966}', '\u{9e6}', '\u{1040}'].contains(&locale.zero_digit), "{code}");
        }
    }
}
