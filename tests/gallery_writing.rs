//! The gallery's writing page with the feature `brahmic`: a line in each Indic and Southeast
//! Asian script, shaped. Drawn headless, compared with a fresh draw, and saved as an image to
//! inspect (with `TAUT_PICTURES` naming a folder).
#![cfg(feature = "brahmic")]

mod common;

#[allow(dead_code, unused_attributes)]
#[path = "../examples/gallery.rs"]
mod gallery;

use common::{Window, click, middle};
use taut::geom::Size;
use taut::text::BuiltInFont;
use taut::{LocalClipboard, Role, Surface, Ui};

#[test]
fn the_writing_page_shows_a_line_of_each_script() {
    let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 760.0, h: 1500.0 }, 1.0);
    ui.set_screen_reader(true);
    let screen = Surface::draw(&mut ui, 64);
    let mut window = Window { ui, screen, passes: 0 };
    let mut app = gallery::app();
    let mut build = |ui: &mut Ui| gallery::describe(&mut app, ui);
    window.run(&mut build);
    window.run(&mut build);
    let tab = window.ui.visible_access_nodes().iter().filter(|node| node.role == Role::Tab).nth(8).map(|node| node.rect).expect("the ninth tab");
    click(&mut window, middle(tab), &mut build);
    window.run(&mut build);
    let fresh = Surface::draw(&mut window.ui, 64);
    assert!(window.screen.pixels == fresh.pixels, "drawing only what changed left the window different");
    if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
        std::fs::write(std::path::Path::new(&folder).join(format!("gallery-writing-{}.png", std::env::consts::OS)), fresh.to_png()).unwrap();
    }
}
