//! Logic errors in the core, found by reading it: each test failed before its fix, and keeps
//! the fix in place.
mod common;

use common::*;
use std::time::Duration;
use taut::{Chart, Color, Column, Dock, DockNode, Event, Icon, Key, Length, Mods, Rect, Role, SecondWindow, Series, Surface, Table, Ui};

/// A star, as an app shows it: a line while it is not selected, filled while it is. (One path,
/// kept once, as an app keeps its icons.)
static STAR: &str = "M8 1.5 9.9 5.8 14.5 6.2 11 9.3 12.1 13.8 8 11.4 3.9 13.8 5 9.3 1.5 6.2 6.1 5.8z";

fn star(ui: &mut Ui, filled: bool) {
    let icon = if filled { Icon::solid(STAR) } else { Icon::line(STAR) };
    ui.el().icon(icon, 32.0, Color::WHITE).show();
}

#[test]
fn an_icon_that_turns_from_a_line_into_a_filled_shape_is_drawn_again() {
    let mut w = window(1.0);
    settle(&mut w, |ui| star(ui, false));
    // The window keeps what it showed and draws only what it is told changed: it panics
    // when that leaves it different from the whole window drawn afresh.
    w.run(&mut |ui| star(ui, true));
}

#[test]
fn the_panel_in_front_stays_in_front_when_a_tab_before_it_goes() {
    let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
    let mut dock = Dock::of(&["a", "b", "c"]);
    dock.nodes[0] = DockNode::Tabs { names: names(&["a", "b", "c"]), front: 1 };
    // "b" was in front; "a" goes (an app that no longer has it, or a tab carried away).
    let dock = dock.only(&["b", "c"]);
    assert_eq!(dock.nodes[0], DockNode::Tabs { names: names(&["b", "c"]), front: 0 }, "another panel came to the front");
}

#[test]
fn a_notice_that_is_to_stay_for_ever_does_not_overflow_the_clock() {
    let mut w = window(1.0);
    w.set_time(Duration::from_secs(5));
    let mut posted = false;
    w.run(&mut |ui| {
        if !std::mem::replace(&mut posted, true) {
            ui.notice_for("Kept until it is pressed", Duration::MAX);
        }
    });
}

#[test]
fn asking_to_run_again_never_does_not_overflow_the_clock() {
    let mut w = window(1.0);
    w.set_time(Duration::from_secs(5));
    w.run(&mut |ui| ui.wake_after(Duration::MAX));
}

/// How many pixels of the window are mostly of the colour (0xff00ff) the chart's line is in.
fn inked(w: &mut Window) -> usize {
    let near = |pixel: u32| (pixel >> 16 & 255) > 120 && (pixel & 255) > 120 && (pixel >> 8 & 255) < 80;
    Surface::draw(&mut w.ui, 64).pixels.iter().filter(|pixel| near(**pixel)).count()
}

#[test]
fn a_chart_of_a_value_that_never_changes_still_draws_its_line() {
    let ink = Color::rgb(0xff00ff);
    for (values, what) in [([50.0f32, 51.0, 50.0, 51.0, 50.0, 51.0], "a value that changes"), ([50.0; 6], "fifty all along"), ([21.5; 6], "21.5 all along"), ([1000.0; 6], "a thousand all along")] {
        let mut w = window(1.0);
        let rows = [Series::line("", &values).color(ink)];
        settle(&mut w, |ui| _ = ui.chart(&Chart::of(&rows), 400.0, 200.0));
        assert!(inked(&mut w) > 200, "no line was drawn for {what}");
    }
}

/// A long list of `count` rows, each `row` high and a box that knows where it is.
fn list_of(ui: &mut Ui, count: usize, row: f32, shown: &mut Vec<(usize, Rect)>) {
    shown.clear();
    for nth in ui.list(count, row) {
        let at = ui.el().key(nth).w(Length::Grow(1.0)).fill(Color::WHITE).sense().show();
        shown.push((nth, at.rect));
    }
    ui.close();
}

/// Such a list, scrolled to its end with the pointer over it: the window, and the rows in
/// view with where each is.
fn long_list(count: usize, row: f32) -> (Window, Vec<(usize, Rect)>) {
    let (mut w, mut shown) = (window(1.0), Vec::new());
    settle(&mut w, |ui| list_of(ui, count, row, &mut shown));
    does(&mut w, Event::Move(taut::geom::pt(100.0, 150.0)), |ui| list_of(ui, count, row, &mut shown));
    key(&mut w, Key::End, Mods::NONE, |ui| list_of(ui, count, row, &mut shown));
    settle(&mut w, |ui| list_of(ui, count, row, &mut shown));
    (w, shown)
}

#[test]
fn the_rows_of_a_long_list_stand_one_under_the_other_at_its_end() {
    // (The guide says a million rows cost what a screenful does.)
    for (count, row) in [(1_000_000, 25.0), (10_000_000, 24.0), (100_000_000, 24.0)] {
        let (_, shown) = long_list(count, row);
        // (The rows that show whole: the first one in view may be cut off at the top.)
        let whole: Vec<_> = shown.iter().filter(|(_, at)| at.h == row).collect();
        assert!(whole.len() > 5, "{count} rows: only {} rows show whole at the end: {shown:?}", whole.len());
        for pair in whole.windows(2) {
            let ((one, above), (two, below)) = (pair[0], pair[1]);
            assert_eq!((two - one, below.y - above.y), (1, row), "{count} rows: row {two} is not {row} under row {one}: {above:?} {below:?}");
        }
    }
}

#[test]
fn the_wheel_scrolls_a_long_list_at_its_end() {
    let (count, row) = (100_000_000, 24.0);
    let (mut w, mut shown) = long_list(count, row);
    let first = shown.first().map(|(nth, _)| *nth);
    // Two rows up: the first row in view is then the one two before.
    does(&mut w, Event::Wheel(0.0, -48.0), |ui| list_of(ui, count, row, &mut shown));
    settle(&mut w, |ui| list_of(ui, count, row, &mut shown));
    assert_eq!(shown.first().map(|(nth, _)| *nth), first.map(|nth| nth - 2), "the wheel moved nothing");
}

const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#ff0000"/></svg>"##;

#[test]
fn a_drawing_the_app_handed_over_shows_in_a_second_window_too() {
    let mut w = window(1.0);
    let drawing = w.add_drawing(SQUARE).expect("a drawing");
    let mut prefs = SecondWindow::new(200.0, 100.0);
    let (mut here, mut there) = (Rect::EMPTY, Rect::EMPTY);
    for _ in 0..3 {
        w.run(&mut |ui| {
            here = ui.el().drawing(drawing, Color::WHITE).sense().show().rect;
            if ui.window(&mut prefs, "Preferences") {
                there = ui.el().drawing(drawing, Color::WHITE).sense().show().rect;
                ui.close();
            }
        });
    }
    assert_eq!((here.w, here.h), (20.0, 20.0));
    assert_eq!((there.w, there.h), (20.0, 20.0), "the drawing takes no room in the second window, and shows nothing");
}

#[test]
fn a_glide_to_no_number_does_not_keep_the_window_running() {
    let mut w = window(1.0);
    // A bar that shows how far something has come, of nothing to do: 0 of 0 is no number.
    let (done, all) = (0.0f32, 0.0f32);
    for step in 0..20 {
        w.set_time(Duration::from_millis(step * 100));
        w.run(&mut |ui| {
            // (Written, not laid out: a size that is no number has a mistake of its own.)
            let wide = ui.animate(7, 200.0 * done / all, Duration::from_millis(150));
            ui.label(&format!("{wide}"));
        });
    }
    assert_eq!(w.wake_in(), None, "two seconds on, it still asks to be run again");
}

#[test]
fn a_box_whose_size_is_no_number_is_described_again_without_a_panic() {
    let mut w = window(1.0);
    settle(&mut w, |ui| _ = ui.el().w(Length::Px(f32::NAN)).h(Length::Px(8.0)).fill(Color::WHITE).show());
}

/// Runs out of goes: a button that opens a dialog with a table of many rows in it. Gives
/// how many goes the run of the click took, and whether a run after it with nothing new
/// still changed the window.
#[test]
fn a_click_that_brings_up_a_table_is_settled_by_the_run_that_sees_it() {
    let columns = [Column::new("Name"), Column::new("Size").px(80.0)];
    let mut w = window(1.0);
    let (mut open, button, mut table) = (false, std::cell::Cell::new(Rect::EMPTY), Table::default());
    let mut build = |ui: &mut Ui| {
        let b = ui.button("Open");
        (open, _) = (open || b.clicked, button.set(b.rect));
        if open {
            ui.dialog();
            ui.el().h(Length::Px(200.0)).w(Length::Px(300.0)).open();
            ui.table(&mut table, &columns);
            for row in ui.rows(500, 24.0) {
                ui.table_row().key(row).open();
                ui.label(&format!("row {row} with a name long enough to be cut"));
                ui.label("12 KB");
                ui.close();
            }
            ui.close();
            ui.close();
            ui.close();
            ui.close();
        }
    };
    settle(&mut w, &mut build);
    let at = middle(button.get());
    for event in [Event::Move(at), Event::Press(taut::Button::Left, Mods::NONE), Event::Release(taut::Button::Left)] {
        does(&mut w, event, &mut build);
    }
    let (goes, wake) = (w.passes, w.wake_in());
    let changed = w.run(&mut build);
    assert!(changed.is_empty() || wake.is_some(), "the click took {goes} goes, and the run after it still changed {changed:?} with nothing asking for it");
}

/// A box that suggests what its text might become, with a screen reader listening: gives
/// what it suggests.
fn suggested(w: &mut Window) -> Vec<String> {
    w.visible_access_nodes().iter().filter(|node| node.role == Role::MenuItem).map(|node| node.name.clone()).collect()
}

#[test]
fn what_is_suggested_follows_options_that_changed() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let (fruit, nuts) = (["apple", "apricot", "banana"], ["almond", "cashew", "pecan"]);
    let mut text = String::new();
    let mut build = |ui: &mut Ui, options: &[&str]| _ = ui.autocomplete(&mut text, "Food", options);
    settle(&mut w, |ui| build(ui, &fruit));
    key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &fruit));
    typed(&mut w, "ap", |ui| build(ui, &fruit));
    assert_eq!(suggested(&mut w), ["apple", "apricot"]);
    // The app's options are others now (as many as before): none of them holds "ap".
    settle(&mut w, |ui| build(ui, &nuts));
    assert_eq!(suggested(&mut w), [""; 0], "what was suggested holds nothing that was typed");
}

#[test]
fn a_table_of_many_columns_shows_each_column_once_whatever_order_was_kept() {
    let names: Vec<String> = (0..70).map(|nth| format!("c{nth}")).collect();
    let columns: Vec<Column> = names.iter().map(|name| Column::new(name).px(40.0)).collect();
    // An order kept from another day that names column 0 twice and column 1 not at all.
    let mut order: Vec<usize> = (0..70).collect();
    order[1] = 0;
    let mut state = Table { order, ..Table::default() };
    let mut w = window(1.0);
    w.set_screen_reader(true);
    settle(&mut w, |ui| {
        ui.table(&mut state, &columns);
        ui.close();
    });
    let headings: Vec<String> = w.visible_access_nodes().iter().filter(|node| node.role == Role::ColumnHeader).map(|node| node.name.clone()).collect();
    assert!(headings.iter().filter(|name| *name == "c0").count() == 1 && headings.contains(&"c1".to_string()), "{headings:?}");
}

#[test]
fn a_box_whose_room_inside_changes_is_drawn_again() {
    let mut w = window(1.0);
    let row = |ui: &mut Ui, indent: f32| {
        let font = ui.theme.font;
        _ = ui.el().w(Length::Px(200.0)).h(Length::Px(30.0)).pad((indent, 0.0, 0.0, 0.0)).text("Item", font, Color::WHITE).show();
    };
    settle(&mut w, |ui| row(ui, 4.0));
    // Moved in, as the row of a tree that is put one level deeper: the box stays where it
    // was and as big; its words move.
    w.run(&mut |ui| row(ui, 40.0));
}

#[test]
fn two_pieces_of_text_that_trade_a_letter_are_laid_out_again() {
    let mut w = window(1.0);
    let (mut first, mut second) = (Rect::EMPTY, Rect::EMPTY);
    let mut two = |ui: &mut Ui, a: &str, b: &str| {
        let font = ui.theme.font;
        ui.row().open();
        first = ui.el().text(a, font, Color::WHITE).sense().show().rect;
        second = ui.el().text(b, font, Color::WHITE).sense().show().rect;
        ui.close();
    };
    settle(&mut w, |ui| two(ui, "1", "23"));
    settle(&mut w, |ui| two(ui, "12", "3"));
    // (The built-in letters are each 12 wide.)
    assert_eq!((first.w, second.w), (24.0, 12.0), "the boxes kept the widths of the last texts");
}

#[test]
fn what_one_box_that_grows_cannot_take_goes_to_the_others() {
    let mut w = window(1.0);
    let mut last = Rect::EMPTY;
    settle(&mut w, |ui| {
        ui.row().open();
        // A search box that grows, but no wider than 100; then empty room that pushes what
        // follows to the far end; then a button 50 wide.
        ui.el().w(Length::Grow(1.0)).max(100.0, f32::INFINITY).h(Length::Px(20.0)).fill(Color::WHITE).show();
        ui.spacer();
        last = ui.el().w(Length::Px(50.0)).h(Length::Px(20.0)).fill(Color::WHITE).sense().show().rect;
        ui.close();
    });
    assert_eq!(last.right(), 420.0, "the button does not end at the far end of the window: {last:?}");
}

#[test]
fn what_an_app_tells_of_a_part_is_what_a_screen_reader_hears() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    // A part that tells of itself (a button called by its hint), and an app that says more
    // of it, right after it is made, as `tell` asks: it is a tick, and it is on.
    settle(&mut w, |ui| {
        let star = ui.icon_button(Icon::line(STAR), "Star");
        ui.tell(&star, Role::CheckBox, Some(true), "Starred");
    });
    let given: Vec<(Role, String, Option<bool>)> = w.visible_access_nodes().iter().map(|node| (node.role, node.name.clone(), node.on)).collect();
    assert!(given.contains(&(Role::CheckBox, "Starred".into(), Some(true))), "the app's word went unheard: {given:?}");
}
