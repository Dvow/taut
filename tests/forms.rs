//! Form parts as an app uses them: a time, a date and time, a date range, a date in the
//! system's format, a number with limits and unit, the colour picker's values, shortcuts and
//! recent colours, and sliders with steps or a curve. Each step goes through the window
//! comparing incremental redraws with full ones.
mod common;

use common::*;
use taut::geom::{Point, pt};
use taut::{Color, Date, Event, Key, Locale, Mods, Order, Rect, Role, Said, Time, Ui};

/// The thing a screen reader is told of under this name.
fn told(ui: &Window, name: &str) -> Said {
    ui.said().iter().find(|said| said.name == name).unwrap_or_else(|| panic!("nothing is told as {name:?}")).clone()
}
/// The things a screen reader is told of as of this kind, in order.
fn told_as(ui: &Window, role: Role) -> Vec<Said> {
    ui.said().iter().filter(|said| said.role == role).cloned().collect()
}
/// How many pixels of the window's picture inside `area` are not the window's ground.
fn inked(ui: &Window, area: Rect) -> usize {
    let ground = ui.theme.bg.0;
    let across = area.x as usize..area.right() as usize;
    let rows = (area.y as usize..area.bottom() as usize).map(|y| &ui.screen.pixels[y * ui.screen.width..][across.clone()]);
    rows.flatten().filter(|pixel| **pixel != ground).count()
}
fn left_of(rect: Rect) -> Point {
    pt(rect.x + 2.0, rect.y + rect.h / 2.0)
}

/// A clock that goes round twice a day, the month first, weeks that start on Sunday.
const AMERICAN: Locale = Locale { order: Order::MonthDayYear, between: '/', first_day: 6, twelve_hours: true, thousands: Some(','), point: '.' };
/// The day first, a 24-hour clock, a comma before a fraction.
const GERMAN: Locale = Locale { order: Order::DayMonthYear, between: '.', first_day: 0, twelve_hours: false, thousands: Some('.'), point: ',' };

#[test]
fn a_time_is_typed_stepped_by_keys_and_the_wheel_and_told_part_by_part() {
    let mut ui = window(1.0);
    ui.locale = Some(Locale::PLAIN);
    ui.listen(true);
    let (mut at, mut frame) = (Time::new(7, 30, 0), Rect::EMPTY);
    let build = |ui: &mut Ui, at: &mut Time, frame: &mut Rect| {
        ui.row().pad(10.0).open();
        *frame = ui.time(at, true).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut at, &mut frame));
    // A press on the box beside its parts puts the keyboard on the hours; digits fill each
    // part in turn, and what is typed in one go goes on into the next part.
    click(&mut ui, left_of(frame), |ui| build(ui, &mut at, &mut frame));
    assert!(told(&ui, "Hours").focused);
    typed(&mut ui, "0945", |ui| build(ui, &mut at, &mut frame));
    assert_eq!((at, told(&ui, "Seconds").focused), (Time::new(9, 45, 0), true));
    // A digit that no second could follow fills the part: 7 seconds. And the last part keeps
    // the keyboard.
    typed(&mut ui, "7", |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(9, 45, 7));
    typed(&mut ui, "58", |ui| build(ui, &mut at, &mut frame));
    assert_eq!((at, told(&ui, "Seconds").focused), (Time::new(9, 45, 58), true));
    // Up and Down go a step, round past the ends; with Shift, ten; Home and End to the ends.
    for (pressed, mods, second) in [(Key::Up, Mods::NONE, 59), (Key::Up, Mods::NONE, 0), (Key::Up, Mods::SHIFT, 10), (Key::End, Mods::NONE, 59), (Key::Home, Mods::NONE, 0)] {
        key(&mut ui, pressed, mods, |ui| build(ui, &mut at, &mut frame));
        assert_eq!(at.second, second, "after {pressed:?} with {mods:?}");
    }
    // Shift and Tab go back to the minutes; the wheel over them steps them.
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut at, &mut frame));
    let minutes = told(&ui, "Minutes").rect;
    does(&mut ui, Event::Move(pt(minutes.x + minutes.w / 2.0, minutes.y + minutes.h / 2.0)), |ui| build(ui, &mut at, &mut frame));
    does(&mut ui, Event::Wheel(0.0, -48.0), |ui| build(ui, &mut at, &mut frame));
    does(&mut ui, Event::Wheel(-48.0, 0.0), |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(9, 56, 0), "a notch up, and one sideways (Shift and the wheel): ten");
    // A screen reader hears each part as a spin button in a group named by the time, and
    // can set one.
    let said = told(&ui, "Minutes");
    assert_eq!((said.role, said.value.as_str(), said.amount, said.step), (Role::Spin, "56", Some([56.0, 0.0, 59.0]), 1.0));
    assert_eq!(told(&ui, "09:56:00").role, Role::Group);
    ui.set_for_reader(said.id, 15.0);
    ui.run(&mut |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(9, 15, 0));
    // Backspace takes a part back to its least.
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(9, 0, 0));
    assert_eq!(ui.written_time(Time::new(21, 5, 9), true), "21:05:09");
}

#[test]
fn on_a_clock_that_goes_round_twice_a_day_the_hours_go_from_12_and_the_half_follows() {
    let mut ui = window(1.0);
    ui.locale = Some(AMERICAN);
    ui.listen(true);
    let (mut at, mut frame) = (Time::new(0, 5, 0), Rect::EMPTY);
    let build = |ui: &mut Ui, at: &mut Time, frame: &mut Rect| {
        ui.row().pad(10.0).open();
        *frame = ui.time(at, false).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut at, &mut frame));
    assert_eq!([told(&ui, "Hours").value, told(&ui, "AM or PM").value], ["12", "AM"], "midnight is twelve in the morning");
    assert_eq!(told(&ui, "Hours").amount, Some([12.0, 1.0, 12.0]));
    // Eleven, then the first letter of the afternoon's word.
    click(&mut ui, left_of(frame), |ui| build(ui, &mut at, &mut frame));
    typed(&mut ui, "11", |ui| build(ui, &mut at, &mut frame));
    assert!(told(&ui, "Minutes").focused);
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut at, &mut frame));
    typed(&mut ui, "p", |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(23, 5, 0));
    // Twelve in the afternoon is noon; the half steps round too.
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut at, &mut frame));
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut at, &mut frame));
    typed(&mut ui, "12", |ui| build(ui, &mut at, &mut frame));
    assert_eq!(at, Time::new(12, 5, 0));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut at, &mut frame));
    key(&mut ui, Key::Up, Mods::NONE, |ui| build(ui, &mut at, &mut frame));
    assert_eq!((at, told(&ui, "12:05 AM").role), (Time::new(0, 5, 0), Role::Group));
    assert_eq!([ui.written_time(Time::new(13, 7, 0), false), ui.written_time(Time::new(12, 0, 30), true)], ["01:07 PM", "12:00:30 PM"]);
}

#[test]
fn a_date_is_written_as_the_person_writes_one_and_its_calendar_walked_by_the_keys() {
    let mut ui = window(1.0);
    ui.locale = Some(GERMAN);
    ui.listen(true);
    let (mut day, mut at) = (Date::new(2026, 11, 11), Rect::EMPTY);
    let build = |ui: &mut Ui, day: &mut Date, at: &mut Rect| {
        ui.column().pad(10.0).open();
        *at = ui.date(day).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut day, &mut at));
    let said = told_as(&ui, Role::Fold).remove(0);
    assert_eq!((said.name.as_str(), said.on), ("11.11.2026", Some(false)));
    assert_eq!(ui.written_date(Date::new(2026, 3, 7)), "07.03.2026");
    // It opens with the keyboard on the day that is picked, told by its whole name.
    click(&mut ui, middle(at), |ui| build(ui, &mut day, &mut at));
    let eleventh = told(&ui, "11 November 2026");
    assert_eq!((eleventh.role, eleventh.on, eleventh.of, eleventh.focused), (Role::Row, Some(true), Some([11, 30]), true));
    for pressed in [Key::Left; 10] {
        key(&mut ui, pressed, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    }
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    assert_eq!(day, Date::new(2026, 11, 1));
    // Page Down: the same day of the next month; Shift and Page Up a year back; End and
    // Home the last and the first day of the week (Monday to Sunday here); across months.
    click(&mut ui, middle(at), |ui| build(ui, &mut day, &mut at));
    for (pressed, mods) in [(Key::PageDown, Mods::NONE), (Key::PageUp, Mods::SHIFT), (Key::End, Mods::NONE)] {
        key(&mut ui, pressed, mods, |ui| build(ui, &mut day, &mut at));
    }
    assert!(told(&ui, "7 December 2025").focused, "the Sunday of the week of the first of December 2025");
    key(&mut ui, Key::Home, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    key(&mut ui, Key::Left, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    assert!(told(&ui, "30 November 2025").focused, "a day back from Monday the first is in the month before");
    key(&mut ui, Key::Space, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    assert_eq!(day, Date::new(2025, 11, 30));
    // Escape closes it, and changes nothing.
    click(&mut ui, middle(at), |ui| build(ui, &mut day, &mut at));
    key(&mut ui, Key::Right, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut day, &mut at));
    assert_eq!((day, told_as(&ui, Role::Row).len()), (Date::new(2025, 11, 30), 0));
}

#[test]
fn the_calendar_starts_its_weeks_on_the_day_the_person_does() {
    // The first of November 2026 is a Sunday: the first place of a week that starts on
    // Sunday, the last of one that starts on Monday.
    let mut places = Vec::new();
    for locale in [AMERICAN, GERMAN] {
        let mut ui = window(1.0);
        ui.locale = Some(locale);
        ui.listen(true);
        let (mut day, mut at) = (Date::new(2026, 11, 11), Rect::EMPTY);
        let build = |ui: &mut Ui, day: &mut Date, at: &mut Rect| {
            ui.column().pad(10.0).open();
            *at = ui.date(day).rect;
            ui.close();
        };
        settle(&mut ui, |ui| build(ui, &mut day, &mut at));
        assert_eq!(told_as(&ui, Role::Fold)[0].name, if locale == AMERICAN { "11/11/2026" } else { "11.11.2026" });
        click(&mut ui, middle(at), |ui| build(ui, &mut day, &mut at));
        places.push(told(&ui, "1 November 2026").rect.x - told(&ui, "2 November 2026").rect.x);
    }
    assert_eq!(places, [-36.0, 216.0], "after it, or six places before (a day is as wide as 30 needs: 34)");
}

#[test]
fn a_span_of_days_is_picked_in_one_calendar_by_the_pointer_and_by_the_keys() {
    let mut ui = window(1.0);
    ui.locale = Some(Locale::PLAIN);
    ui.listen(true);
    let (mut from, mut to, mut at) = (Date::new(2026, 11, 2), Date::new(2026, 11, 3), Rect::EMPTY);
    let build = |ui: &mut Ui, from: &mut Date, to: &mut Date, at: &mut Rect| {
        ui.column().pad(10.0).open();
        *at = ui.date_range(from, to).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut from, &mut to, &mut at));
    assert_eq!(told_as(&ui, Role::Fold)[0].name, "2026-11-02 \u{2013} 2026-11-03");
    // The later day first: the span is the same.
    click(&mut ui, middle(at), |ui| build(ui, &mut from, &mut to, &mut at));
    let twentieth = told(&ui, "20 November 2026").rect;
    click(&mut ui, middle(twentieth), |ui| build(ui, &mut from, &mut to, &mut at));
    assert_eq!((from, to), (Date::new(2026, 11, 2), Date::new(2026, 11, 3)), "nothing changes until the second day");
    // Meanwhile the span shows up to the day under the pointer.
    let tenth = told(&ui, "10 November 2026").rect;
    does(&mut ui, Event::Move(middle(tenth)), |ui| build(ui, &mut from, &mut to, &mut at));
    let shown: Vec<_> = told_as(&ui, Role::Row).into_iter().filter(|row| row.on == Some(true)).map(|row| row.of.unwrap()[0]).collect();
    assert_eq!(shown, (10..=20).collect::<Vec<_>>());
    click(&mut ui, middle(tenth), |ui| build(ui, &mut from, &mut to, &mut at));
    assert_eq!((from, to, told_as(&ui, Role::Row).len()), (Date::new(2026, 11, 10), Date::new(2026, 11, 20), 0));
    // By the keys: it opens on the first day; Enter, a week on, Enter.
    click(&mut ui, middle(at), |ui| build(ui, &mut from, &mut to, &mut at));
    for pressed in [Key::Right, Key::Enter, Key::Down] {
        key(&mut ui, pressed, Mods::NONE, |ui| build(ui, &mut from, &mut to, &mut at));
    }
    let picked: Vec<_> = told_as(&ui, Role::Row).into_iter().filter(|row| row.on == Some(true)).map(|row| row.of.unwrap()[0]).collect();
    assert_eq!(picked, (11..=18).collect::<Vec<_>>(), "the span to the day the keyboard is on");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut from, &mut to, &mut at));
    assert_eq!((from, to), (Date::new(2026, 11, 11), Date::new(2026, 11, 18)));
}

#[test]
fn a_date_and_a_time_are_set_in_one_box() {
    let mut ui = window(1.0);
    ui.locale = Some(GERMAN);
    ui.listen(true);
    let (mut day, mut at, mut frame) = (Date::new(2026, 11, 11), Time::new(14, 0, 0), Rect::EMPTY);
    let build = |ui: &mut Ui, day: &mut Date, at: &mut Time, frame: &mut Rect| {
        ui.row().pad(10.0).open();
        *frame = ui.date_time(day, at).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut day, &mut at, &mut frame));
    // The date opens the calendar under the whole box.
    let date = told_as(&ui, Role::Fold)[0].clone();
    assert_eq!(date.name, "11.11.2026");
    click(&mut ui, middle(date.rect), |ui| build(ui, &mut day, &mut at, &mut frame));
    let twelfth = told(&ui, "12 November 2026").rect;
    assert!(twelfth.y > frame.bottom(), "{twelfth:?} under {frame:?}");
    click(&mut ui, middle(twelfth), |ui| build(ui, &mut day, &mut at, &mut frame));
    assert_eq!(day, Date::new(2026, 11, 12));
    // The keyboard goes on from the date to the hours.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut day, &mut at, &mut frame));
    typed(&mut ui, "0815", |ui| build(ui, &mut day, &mut at, &mut frame));
    assert_eq!(at, Time::new(8, 15, 0));
    assert_eq!(told_as(&ui, Role::Fold)[0].name, "12.11.2026");
}

#[test]
fn a_number_has_its_limits_its_steps_and_its_unit_and_what_is_no_number_is_refused_kindly() {
    let mut ui = window(1.0);
    ui.locale = Some(GERMAN);
    ui.listen(true);
    let (mut weight, mut copies, mut boxes) = (12.5f32, 3u8, [Rect::EMPTY; 2]);
    let build = |ui: &mut Ui, weight: &mut f32, copies: &mut u8, boxes: &mut [Rect; 2]| {
        ui.column().pad(10.0).gap(40.0).open();
        boxes[0] = ui.number_in(weight, 0.0..=500.0, 0.5, "kg").rect;
        boxes[1] = ui.number_in(copies, 1..=9, 1, "").rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    let spins = told_as(&ui, Role::Spin);
    assert_eq!((spins[0].value.as_str(), spins[0].amount, spins[0].step), ("12,5", Some([12.5, 0.0, 500.0]), 0.5), "written with the person's comma");
    assert!(inked(&ui, Rect { x: boxes[0].right() - 30.0, w: 24.0, ..boxes[0] }) > 0, "the unit is written at the end of the box");
    // Typed with the person's comma, or a point.
    click(&mut ui, middle(boxes[0]), |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    typed(&mut ui, "7,25", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!(weight, 7.25);
    let under = Rect { y: boxes[0].bottom() + 1.0, h: 14.0, ..boxes[0] };
    assert_eq!(inked(&ui, under), 0, "nothing is said of a number");
    // What can never be a number: it changes nothing, a note under the box says so, and a
    // screen reader hears it. What may yet be one is waited for.
    typed(&mut ui, "x", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!((weight, ui.announced()), (7.25, &["Not a number".to_string()][..]));
    assert!(inked(&ui, under) > 0);
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    typed(&mut ui, "-", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!((inked(&ui, under), ui.announced().len()), (0, 0), "a sign alone may become a number");
    typed(&mut ui, "3", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!(ui.announced(), ["At least 0"]);
    assert_eq!(weight, 7.25, "below the least is refused");
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    typed(&mut ui, "1.000", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!((weight, ui.announced()), (7.25, &["At most 500".to_string()][..]), "a point between thousands, here");
    // Up and Down go a step, from a whole number of steps; Shift, Page Up and Page Down ten;
    // the ends hold. The text is written afresh.
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    typed(&mut ui, "7,3", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    for (pressed, mods, wanted) in [(Key::Up, Mods::NONE, 7.5), (Key::Up, Mods::SHIFT, 12.5), (Key::PageDown, Mods::NONE, 7.5), (Key::Down, Mods::NONE, 7.0)] {
        key(&mut ui, pressed, mods, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
        assert_eq!(weight, wanted, "after {pressed:?} with {mods:?}");
    }
    assert_eq!(told_as(&ui, Role::Spin)[0].value, "7");
    for _ in 0..3 {
        key(&mut ui, Key::PageDown, Mods::NONE, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    }
    assert_eq!(weight, 0.0, "the least holds");
    // The wheel steps the box the keyboard is in, while the pointer is over it; and only
    // then: over the other box, it is left to scroll.
    does(&mut ui, Event::Wheel(0.0, -96.0), |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!(weight, 1.0);
    does(&mut ui, Event::Move(middle(boxes[1])), |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    does(&mut ui, Event::Wheel(0.0, -48.0), |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!((weight, copies), (1.0, 3));
    // A whole number: what has a fraction is no number of its kind. Leaving the box writes
    // the number back.
    click(&mut ui, middle(boxes[1]), |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    typed(&mut ui, "2,5", |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!((copies, ui.announced()), (3, &["Not a number".to_string()][..]));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!(told_as(&ui, Role::Spin)[1].value, "3");
    assert_eq!(inked(&ui, Rect { y: boxes[1].bottom() + 1.0, h: 14.0, ..boxes[1] }), 0, "the note goes with the typing");
    // A screen reader sets it.
    let id = told_as(&ui, Role::Spin)[1].id;
    ui.set_for_reader(id, 12.0);
    ui.run(&mut |ui| build(ui, &mut weight, &mut copies, &mut boxes));
    assert_eq!(copies, 9, "as near as its most lets it");
}

#[test]
fn the_colour_picker_is_reached_by_the_keys_typed_as_numbers_and_remembers_what_was_picked() {
    let mut ui = window(1.0);
    ui.listen(true);
    let (mut color, mut swatch) = (Color::rgb(0x808080), Rect::EMPTY);
    let build = |ui: &mut Ui, color: &mut Color, swatch: &mut Rect| {
        ui.row().pad(10.0).open();
        *swatch = ui.color(color).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut color, &mut swatch));
    let said = told_as(&ui, Role::Select)[0].clone();
    assert_eq!((said.value.as_str(), said.on), ("#808080", Some(false)));
    click(&mut ui, middle(swatch), |ui| build(ui, &mut color, &mut swatch));
    // The picker opens with the keyboard on the square, whose arrows make the colour
    // stronger and darker.
    let square = told(&ui, "S");
    assert_eq!((square.role, square.focused, square.amount), (Role::Slider, true, Some([0.0, 0.0, 100.0])));
    key(&mut ui, Key::Right, Mods::SHIFT, |ui| build(ui, &mut color, &mut swatch));
    key(&mut ui, Key::Down, Mods::SHIFT, |ui| build(ui, &mut color, &mut swatch));
    let [hue, strong, light] = color.to_hsv();
    assert!((strong - 0.1).abs() < 0.01 && (light - (128.0 / 255.0 - 0.1)).abs() < 0.01, "{hue} {strong} {light}");
    // The strip of hues and the one of how solid it is, by Up and Down.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut color, &mut swatch));
    key(&mut ui, Key::Down, Mods::SHIFT, |ui| build(ui, &mut color, &mut swatch));
    assert_eq!(told(&ui, "H").amount.map(|amount| amount[0].round()), Some(10.0));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut color, &mut swatch));
    for _ in 0..5 {
        key(&mut ui, Key::Down, Mods::SHIFT, |ui| build(ui, &mut color, &mut swatch));
    }
    assert!((125..=130).contains(&color.a()), "half solid: {color}");
    // Red, green, blue and how solid, typed: red is the first box for a number, how solid
    // the fourth, in hundredths.
    let boxes = told_as(&ui, Role::Spin);
    assert_eq!(boxes.iter().map(|spin| spin.amount.unwrap()[2]).collect::<Vec<_>>(), [255.0, 255.0, 255.0, 100.0]);
    click(&mut ui, middle(boxes[0].rect), |ui| build(ui, &mut color, &mut swatch));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut color, &mut swatch));
    typed(&mut ui, "255", |ui| build(ui, &mut color, &mut swatch));
    assert_eq!(color.0 >> 16 & 0xff, 255);
    click(&mut ui, middle(boxes[3].rect), |ui| build(ui, &mut color, &mut swatch));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut color, &mut swatch));
    typed(&mut ui, "100", |ui| build(ui, &mut color, &mut swatch));
    assert_eq!(color.a(), 255);
    let picked = color;
    // Closed, the colour picked is kept, and offered the next time.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut color, &mut swatch));
    ui.run(&mut |ui| build(ui, &mut color, &mut swatch));
    color = Color::rgb(0x112233);
    ui.run(&mut |ui| build(ui, &mut color, &mut swatch));
    click(&mut ui, middle(swatch), |ui| build(ui, &mut color, &mut swatch));
    let used = told(&ui, &picked.to_string());
    assert_eq!(used.role, Role::Button);
    click(&mut ui, middle(used.rect), |ui| build(ui, &mut color, &mut swatch));
    assert_eq!(color, picked);
}

#[test]
fn a_slider_stops_at_steps_with_marks_and_another_goes_by_a_curve() {
    let mut ui = window(1.0);
    ui.locale = Some(GERMAN);
    ui.listen(true);
    let (mut stars, mut zoom, mut lines) = (3.0f32, 100.0f32, [Rect::EMPTY; 2]);
    let build = |ui: &mut Ui, stars: &mut f32, zoom: &mut f32, lines: &mut [Rect; 2]| {
        ui.column().pad(10.0).gap(10.0).open();
        lines[0] = ui.slider_marked(stars, 0.0..=10.0, 0.5, "").rect;
        lines[1] = ui.slider_curved(zoom, 25.0..=400.0, "%").rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    // The value is written beside each, in the person's way; marks lie under the first.
    assert_eq!([told(&ui, "3,0").role, told(&ui, "100 %").role], [Role::Text; 2]);
    let marks = Rect { y: lines[0].bottom(), h: 6.0, ..lines[0] };
    assert!(inked(&ui, marks) >= 21 * 5, "a mark at each of 21 steps, five high");
    // A press stops on the nearest step; the keys go a step, Page Up ten, Home and End.
    let at = |line: Rect, part: f32| pt(line.x + 7.0 + (line.w - 14.0) * part, line.y + line.h / 2.0);
    click(&mut ui, at(lines[0], 0.63), |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    assert_eq!(stars, 6.5);
    for (pressed, wanted) in [(Key::Right, 7.0), (Key::PageDown, 2.0), (Key::End, 10.0), (Key::Home, 0.0)] {
        key(&mut ui, pressed, Mods::NONE, |ui| build(ui, &mut stars, &mut zoom, &mut lines));
        assert_eq!(stars, wanted, "after {pressed:?}");
    }
    // Half way along the curve from 25 to 400 is 100, three quarters 200; a key goes a
    // twentieth of the way, which multiplies.
    click(&mut ui, at(lines[1], 0.75), |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    assert!((zoom - 200.0).abs() < 4.0, "{zoom}");
    key(&mut ui, Key::Home, Mods::NONE, |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    key(&mut ui, Key::Right, Mods::NONE, |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    assert!((zoom - 25.0 * 16f32.powf(0.05)).abs() < 0.01, "{zoom}");
    let said = told_as(&ui, Role::Slider)[1].clone();
    assert_eq!(said.amount.map(|amount| [amount[1], amount[2]]), Some([25.0, 400.0]));
    ui.set_for_reader(said.id, 300.0);
    ui.run(&mut |ui| build(ui, &mut stars, &mut zoom, &mut lines));
    assert_eq!(zoom, 300.0);
}
