//! Fuzzer finds from driving the library's example apps with random events, each minimized.
//! Regression tests: each failed before its fix.
mod common;

#[allow(dead_code, unused_attributes)]
#[path = "../examples/gallery.rs"]
mod gallery;

use common::{Window, click, middle};
use std::time::Duration;
use taut::geom::Size;
use taut::text::BuiltInFont;
use taut::{LocalClipboard, Role, Surface, Ui};

/// The gallery in a window on no screen, of this size and scale, with a screen reader
/// listening (so that its parts can be found by what they are told as).
fn gallery_window(size: Size, scale: f32) -> Window {
    let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(size, scale);
    ui.listen(true);
    let screen = Surface::draw(&mut ui, 64);
    Window { ui, screen, passes: 0 }
}

/// Where a part is that a screen reader is told of as `role`, the `nth` of its kind (or by
/// a name it is told by).
fn told(ui: &Window, role: Role, name: Option<&str>, nth: usize) -> taut::Rect {
    let mut found = ui.said().iter().filter(|said| said.role == role && name.is_none_or(|name| said.name == name));
    found.nth(nth).map(|said| said.rect).expect("the part is there")
}

/// The gallery's page of more parts, at 1200 by 800 at a scale of 1.5: the box to type a
/// number of copies in is clicked, and the time goes on past the first blink of its cursor.
/// The cursor goes out, the run says nothing changed (an empty rectangle), and a window
/// that draws only what changed keeps showing it: the two pixels across and 21 down that
/// are the cursor differ from the window drawn afresh. (The same box alone in a window
/// blinked as it should: below it on the page lies a code box, which took the place of the
/// box typed into, whose cursor is watched. See `fuzzed_code.rs`.)
#[test]
fn the_cursor_of_the_gallery_number_box_blinks_where_it_is_drawn() {
    let mut app = gallery::app();
    let mut ui = gallery_window(Size { w: 1200.0, h: 800.0 }, 1.5);
    let mut build = |ui: &mut Ui| gallery::describe(&mut app, ui);
    ui.run(&mut build);
    ui.run(&mut build);
    let page = told(&ui, Role::Tab, None, 4);
    click(&mut ui, middle(page), &mut build);
    let number = told(&ui, Role::Spin, Some("A number to type"), 0);
    click(&mut ui, middle(number), &mut build);
    for millis in [100, 300, 600] {
        ui.ui.set_time(Duration::from_millis(millis));
        ui.run(&mut build);
    }
}
