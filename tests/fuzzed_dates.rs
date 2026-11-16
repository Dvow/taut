//! Fuzzer finds in dates as an app supplies them: a `Date` is three app-chosen numbers, and
//! arithmetic on them overflowed at their extremes (a panic in debug builds; a wrong month in
//! release). Regression tests: each failed before its fix.
mod common;

use common::{click, middle, settle, window};
use taut::{Date, Rect, Ui};

/// A date field whose year is past 178,956,970 (a year read from a file, or typed, that the
/// app did not hold to anything): opening its calendar works out the month shown by
/// `months_on`, which multiplies the year by twelve in 32 bits.
#[test]
fn the_calendar_of_a_date_far_ahead_opens() {
    let mut ui = window(1.0);
    let mut day = Date::new(200_000_000, 6, 1);
    let mut at = Rect::default();
    settle(&mut ui, |ui: &mut Ui| at = ui.date(&mut day).rect);
    click(&mut ui, middle(at), |ui: &mut Ui| {
        ui.date(&mut day);
    });
}

/// The sums of a date at the ends of what its numbers hold: the length of December of the
/// last year (`year + 1`), of a month numbered 255 (`month + 1`), a month on from the last
/// year (`year * 12`), and the day a number of days that is no date at all comes to.
#[test]
fn a_date_at_the_ends_of_what_its_numbers_hold_is_worked_out_without_overflowing() {
    assert_eq!(Date::new(i32::MAX, 12, 1).month_length(), 31);
    assert_eq!(Date::new(i32::MIN, 2, 1).month_length(), 29);
    let _ = Date::new(2026, 255, 1).month_length();
    let on = Date::new(i32::MAX, 1, 31).months_on(1);
    assert_eq!((on.month, on.day), (2, 28));
    let _ = Date::new(i32::MIN, 1, 1).months_on(i32::MIN);
    let _ = (Date::from_days(i64::MAX), Date::from_days(i64::MIN));
    // (And nothing changes for the dates people have.)
    assert_eq!([Date::new(2024, 2, 1).month_length(), Date::new(2023, 2, 1).month_length(), Date::new(1900, 2, 1).month_length(), Date::new(2000, 2, 1).month_length()], [29, 28, 28, 29]);
    assert_eq!(Date::new(2026, 1, 31).months_on(1), Date::new(2026, 2, 28));
    assert_eq!(Date::from_days(Date::new(2026, 11, 13).days()), Date::new(2026, 11, 13));
}
