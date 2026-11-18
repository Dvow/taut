//! The colour picker's recent colours, kept between runs as one line of text.
mod common;

use common::*;
use taut::{Color, RecentColors, Rect, Role, Ui};

fn picker(ui: &mut Ui, color: &mut Color, swatch: &mut Rect) {
    ui.row().pad(10.0).open();
    *swatch = ui.color_picker(color).rect;
    ui.close();
}

#[test]
fn recent_colours_are_written_as_a_line_and_offered_again_in_the_next_run() {
    // One run: two colours picked, one after the other.
    let (mut ui, mut color, mut swatch) = (window(1.0), Color::rgb(0x808080), Rect::EMPTY);
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| picker(ui, &mut color, &mut swatch));
    for picked in [Color::rgb(0xff8800), Color(0x8033aaff)] {
        click(&mut ui, middle(swatch), |ui| picker(ui, &mut color, &mut swatch));
        color = picked;
        key(&mut ui, taut::Key::Escape, taut::Mods::NONE, |ui| picker(ui, &mut color, &mut swatch));
        settle(&mut ui, |ui| picker(ui, &mut color, &mut swatch));
    }
    let kept = ui.recent_colors().to_string();
    assert_eq!(kept, "#33aaff80 #ff8800", "newest first");

    // The next run: the line read back, and the picker offers them.
    let (mut ui, mut color) = (window(1.0), Color::rgb(0x112233));
    ui.set_screen_reader(true);
    ui.set_recent_colors(kept.parse().expect("what was written reads back"));
    settle(&mut ui, |ui| picker(ui, &mut color, &mut swatch));
    click(&mut ui, middle(swatch), |ui| picker(ui, &mut color, &mut swatch));
    let offered: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::Button && node.name.starts_with('#')).map(|node| node.name.clone()).collect();
    assert_eq!(offered, ["#33aaff80", "#ff8800"]);
    let orange = expect_node(&ui, "#ff8800").rect;
    click(&mut ui, middle(orange), |ui| picker(ui, &mut color, &mut swatch));
    assert_eq!(color, Color::rgb(0xff8800));

    // Lines that were not written here give an error; an empty one, none.
    for wrong in ["#ff8800 red", "ff8800", "#ff88", &["#000000"; 9].join(" ")] {
        assert_eq!(wrong.parse::<RecentColors>(), Err(()), "{wrong}");
    }
    assert_eq!("".parse::<RecentColors>(), Ok(RecentColors::default()));
    // More than eight set: the oldest go.
    ui.set_recent_colors(RecentColors(vec![Color::rgb(0); 12]));
    assert_eq!(ui.recent_colors().0.len(), 8);
}
