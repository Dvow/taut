//! The page scrolls sideways too, where what is on it is wider than the window: by the wheel
//! (sideways, or Shift with it, as the systems send it), its bar, Left and Right with nothing
//! focused, and for a screen reader that asks to see something. Each step is held against the
//! window drawn afresh.
use taut::geom::pt;
use taut::testing::Trial;
use taut::{Length, Role, Ui};

/// Where the box a screen reader hears as `name` lies, and whether any of it shows.
fn lies(trial: &Trial<impl Sized>, name: &str) -> (f32, bool) {
    let node = trial.access_nodes().iter().find(|node| node.name == name).unwrap_or_else(|| panic!("no {name}: {}", trial.dump()));
    (node.rect.x, node.visible)
}

/// A box told as `name`, `wide` units wide and 30 high.
fn named(ui: &mut Ui, name: &str, wide: f32) {
    let panel = ui.theme.panel;
    let shown = ui.el().w(Length::Px(wide)).h(Length::Px(30.0)).fill(panel).sense().show();
    ui.tell(&shown, Role::Group, None, name);
}

/// A row of three 300-wide boxes in a column of no width of its own: 900 on a page 640 wide.
fn wide(_: &mut (), ui: &mut Ui) {
    ui.column().open();
    ui.row().open();
    for name in ["Near", "Middle", "Far"] {
        named(ui, name, 300.0);
    }
    ui.close();
    ui.close();
}

#[test]
fn the_page_scrolls_sideways_where_its_content_is_wider_than_the_window() {
    let mut trial = Trial::new((), wide);
    assert_eq!((lies(&trial, "Far"), lies(&trial, "Middle").0), ((600.0, true), 300.0));
    // The wheel sideways; no further than the content's end, 900 - 640.
    trial.wheel(pt(320.0, 15.0), 100.0, 0.0);
    assert_eq!(lies(&trial, "Middle").0, 200.0);
    trial.wheel(pt(320.0, 15.0), 1000.0, 0.0);
    assert_eq!(lies(&trial, "Middle").0, 40.0);
    // Left with nothing focused: back a line.
    trial.key("Left");
    assert_eq!(lies(&trial, "Middle").0, 56.0);
    trial.wheel(pt(320.0, 15.0), -1000.0, 0.0);
    // Its bar along the bottom: a thumb of 640 x 640 / 900 at the left. Dragged 92.4 (all it
    // can go, 640 - 455.1, times 130 / 260), it scrolls 130.
    let thumb = 640.0 * 640.0 / 900.0;
    trial.drag(pt(100.0, 475.0), pt(100.0 + (640.0 - thumb) / 2.0, 475.0));
    assert!((lies(&trial, "Middle").0 - 170.0).abs() < 0.01, "{:?}", lies(&trial, "Middle"));
}

/// Text that wraps, a box wider than the window inside a box that scrolls, and a narrow row.
fn fits(_: &mut (), ui: &mut Ui) {
    let t = ui.theme;
    ui.el().text(&"words that wrap ".repeat(40), t.font, t.text).wrap().show();
    ui.column().w(Length::Px(300.0)).h(Length::Px(100.0)).scroll().scroll_x().open();
    named(ui, "Inside", 2000.0);
    ui.close();
    named(ui, "Last", 100.0);
}

#[test]
fn what_fits_or_scrolls_in_a_box_of_its_own_does_not_scroll_the_page() {
    let mut trial = Trial::new((), fits);
    trial.wheel(pt(500.0, 400.0), 300.0, 0.0);
    trial.key("Right");
    assert_eq!(lies(&trial, "Last").0, 0.0);
}

/// A box 50 wide right of 1200 of room, in a page that is otherwise narrow.
fn far_away(_: &mut (), ui: &mut Ui) {
    ui.row().open();
    ui.el().w(Length::Px(1200.0)).h(Length::Px(30.0)).show();
    named(ui, "Away", 50.0);
    ui.close();
}

#[test]
fn a_screen_reader_brings_what_lies_right_of_the_window_into_view() {
    let mut trial = Trial::new((), far_away);
    // Told, as out of view.
    assert_eq!(lies(&trial, "Away"), (1200.0, false));
    let id = trial.access_nodes().iter().find(|node| node.name == "Away").map_or(0, |node| node.id);
    trial.ui.show_for_reader(id);
    trial.run();
    // Scrolled just far enough: its right edge at the window's.
    assert_eq!(lies(&trial, "Away"), (590.0, true));
}
