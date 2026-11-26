//! Boxes that stay in view while what is around them scrolls: sticky columns, boxes sticking
//! in an outer scroll box, kept and sticky together, sticky both ways. Each step is held
//! against the window drawn afresh (the trial does that), and pixels against places worked out
//! by hand.
use taut::geom::pt;
use taut::testing::Trial;
use taut::{Color, Length, Role, Ui};

const STUCK: Color = Color::rgb(0xd03030);
/// The cells after a row's first, each its own colour.
const CELLS: [Color; 7] = [Color::rgb(0x30a030), Color::rgb(0x3030d0), Color::rgb(0xa0a030), Color::rgb(0x30a0a0), Color::rgb(0xa030a0), Color::rgb(0x606060), Color::rgb(0x905020)];

/// The pixel of the trial's window at `x`, `y` (in units, at a scale of 1).
fn pixel(trial: &Trial<impl Sized>, x: f32, y: f32) -> u32 {
    trial.screen.pixels[y as usize * trial.screen.width + x as usize] & 0x00ff_ffff
}
fn rgb(color: Color) -> u32 {
    color.0 & 0x00ff_ffff
}
/// Where the box a screen reader hears as `name` lies.
fn rect_of(trial: &Trial<impl Sized>, name: &str) -> taut::Rect {
    trial.access_nodes().iter().find(|node| node.name == name).map(|node| node.rect).unwrap_or_else(|| panic!("no {name}: {}", trial.dump()))
}

/// A row of 60-wide, 30-high cells: the first sticky sideways, the rest focusable and told by
/// name ("Cell 1" and on).
fn row_of_cells(ui: &mut Ui, row: usize) {
    ui.row().open();
    let first = ui.el().w(Length::Px(60.0)).h(Length::Px(30.0)).fill(STUCK).sticky_x().sense().show();
    ui.tell(&first, Role::Group, None, &format!("First {row}"));
    for (n, color) in CELLS.iter().enumerate() {
        let cell = ui.el().w(Length::Px(60.0)).h(Length::Px(30.0)).fill(*color).focusable().show();
        ui.tell(&cell, Role::Group, None, &format!("Cell {row}.{}", n + 1));
    }
    ui.close();
}

/// Three such rows in a 300 by 120 box at the window's corner that scrolls both ways.
fn table(_: &mut (), ui: &mut Ui) {
    ui.column().w(Length::Px(300.0)).h(Length::Px(120.0)).scroll().scroll_x().open();
    for row in 0..3 {
        row_of_cells(ui, row);
    }
    ui.close();
}

#[test]
fn a_sticky_column_stays_at_the_left_while_its_row_scrolls_sideways() {
    let mut trial = Trial::new((), table);
    assert_eq!((pixel(&trial, 30.0, 15.0), pixel(&trial, 90.0, 15.0)), (rgb(STUCK), rgb(CELLS[0])));
    // 90 units on: cell k (from 1) lies at 60k - 90. The first column stays at 0 to 60, and
    // what scrolls shows only right of it: cell 1 (-30 to 30) not at all, cell 2 (30 to 90)
    // from 60. (Drawn after it, cell 2 would cover 30 to 60 of it if it were not cut.)
    trial.wheel(pt(150.0, 50.0), 90.0, 0.0);
    assert_eq!(rect_of(&trial, "First 0").x, 0.0);
    assert_eq!(rect_of(&trial, "Cell 0.3").x, 90.0);
    assert_eq!([30.0, 45.0, 59.0, 61.0, 89.0, 91.0].map(|x| pixel(&trial, x, 15.0)), [STUCK, STUCK, STUCK, CELLS[1], CELLS[1], CELLS[2]].map(rgb));
    assert_eq!(pixel(&trial, 45.0, 75.0), rgb(STUCK), "every row's");
    // A cell focused while under the column is scrolled out from under it, not merely into
    // the box: cell 1 at 60, beside the column.
    trial.key("Tab");
    assert_eq!(trial.visible_access_nodes().iter().find(|node| node.focused).map(|node| (node.name.as_str(), node.rect.x)), Some(("Cell 0.1", 60.0)));
}

/// A list 900 wide (wider than the window, so the page scrolls sideways) that scrolls only
/// down, of rows whose first cells stick sideways.
fn wide_list(_: &mut (), ui: &mut Ui) {
    ui.column().w(Length::Px(900.0)).h(Length::Px(200.0)).scroll().open();
    for row in 0..12 {
        ui.row().open();
        let first = ui.el().w(Length::Px(100.0)).h(Length::Px(30.0)).fill(STUCK).sticky_x().sense().show();
        ui.tell(&first, Role::Group, None, &format!("First {row}"));
        ui.el().w(Length::Px(800.0)).h(Length::Px(30.0)).fill(CELLS[0]).show();
        ui.close();
    }
    ui.close();
}

#[test]
fn a_box_sticks_in_the_next_box_out_that_scrolls_its_way() {
    let mut trial = Trial::new((), wide_list);
    // The list scrolls only down: its first cells stick at the page's left as the page
    // scrolls sideways.
    trial.wheel(pt(300.0, 100.0), 150.0, 0.0);
    assert_eq!(rect_of(&trial, "First 0").x, 0.0);
    assert_eq!([50.0, 99.0, 101.0].map(|x| pixel(&trial, x, 15.0)), [STUCK, STUCK, CELLS[0]].map(rgb));
    // Down, the list scrolls itself; the cells still stick.
    trial.wheel(pt(300.0, 100.0), 0.0, 45.0);
    let second = rect_of(&trial, "First 2");
    assert_eq!((second.x, second.y), (0.0, 15.0));
}

/// 100 units of room, then a box 150 high scrolling down with a header kept at its top that
/// sticks too, and 20 rows of 30 under it; then 800 more, so the page scrolls down.
fn kept_and_sticky(_: &mut (), ui: &mut Ui) {
    ui.el().h(Length::Px(100.0)).show();
    ui.column().w(Length::Px(300.0)).h(Length::Px(150.0)).scroll().open();
    let header = ui.el().h(Length::Px(30.0)).fill(STUCK).frozen().sticky().sense().show();
    ui.tell(&header, Role::Group, None, "Header");
    for row in 0..20 {
        ui.el().h(Length::Px(30.0)).fill(CELLS[row % 2]).show();
    }
    ui.close();
    ui.el().h(Length::Px(800.0)).show();
}

#[test]
fn a_box_kept_in_view_that_sticks_too_sticks_in_the_next_box_out() {
    let mut trial = Trial::new((), kept_and_sticky);
    // Kept at the top of its box while that scrolls.
    trial.wheel(pt(150.0, 150.0), 0.0, 90.0);
    assert_eq!(rect_of(&trial, "Header").y, 100.0);
    assert_eq!(pixel(&trial, 10.0, 115.0), rgb(STUCK));
    // The page scrolled 150 (the pointer off the box): the box's top is 50 above the window,
    // and the header sticks at the window's top, what scrolls in the box passing under it.
    trial.wheel(pt(500.0, 300.0), 0.0, 150.0);
    assert_eq!(rect_of(&trial, "Header").y, 0.0);
    assert_eq!([1.0, 29.0].map(|y| pixel(&trial, 10.0, y)), [rgb(STUCK); 2]);
    assert_ne!(pixel(&trial, 10.0, 31.0), rgb(STUCK));
    // Its box's bottom pushes it out: at 250 the box ends at the window's top, the header 30
    // above it.
    trial.wheel(pt(500.0, 300.0), 0.0, 100.0);
    assert_eq!(rect_of(&trial, "Header").y, -30.0);
}

/// A 300 by 200 box scrolling both ways, of 800 by 800 in which a 50-square box at the corner
/// sticks both ways, and a box beside and one under it.
fn corner(_: &mut (), ui: &mut Ui) {
    ui.column().w(Length::Px(300.0)).h(Length::Px(200.0)).scroll().scroll_x().open();
    ui.el().flow(taut::Flow::Stack).w(Length::Px(800.0)).h(Length::Px(800.0)).open();
    let stuck = ui.el().w(Length::Px(50.0)).h(Length::Px(50.0)).fill(STUCK).sticky().sticky_x().sense().show();
    ui.tell(&stuck, Role::Group, None, "Corner");
    ui.el().offset(60.0, 0.0).w(Length::Px(400.0)).h(Length::Px(40.0)).fill(CELLS[0]).show();
    ui.el().offset(0.0, 60.0).w(Length::Px(40.0)).h(Length::Px(400.0)).fill(CELLS[1]).show();
    ui.close();
    ui.close();
}

#[test]
fn a_box_sticky_both_ways_stays_in_the_corner() {
    let mut trial = Trial::new((), corner);
    trial.wheel(pt(150.0, 100.0), 100.0, 0.0);
    trial.wheel(pt(150.0, 100.0), 0.0, 100.0);
    let at = rect_of(&trial, "Corner");
    assert_eq!((at.x, at.y), (0.0, 0.0));
    assert_eq!(pixel(&trial, 25.0, 25.0), rgb(STUCK));
}
