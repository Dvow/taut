//! Renders a small interface into a PNG file with system text, without opening a window: what a
//! program presenting the interface through its own renderer gets.
//!   cargo run -p taut --example picture -- out.png [scale]
use taut::geom::Size;
use taut::{Icon, LocalClipboard, Surface, Ui};

const SPARK: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");

fn describe(on: &mut bool, level: &mut f32, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad((10.0, 8.0)).gap(8.0).fill(t.panel).open();
    ui.label("A picture of an interface");
    ui.spacer();
    ui.button("A button");
    ui.primary_button("The main one");
    ui.close();
    ui.separator();
    ui.column().grow().pad(10.0).gap(8.0).open();
    ui.checkbox(on, "A tick box");
    ui.paragraph("Letters come from the system's own fonts. Each is drawn once as shades of grey and laid down from then on, so they keep their place on any block of pixels.");
    ui.slider(level, 0.0..=10.0);
    for name in ["One row", "Another", "A third"] {
        ui.row().gap(8.0).pad((10.0, 5.0)).fill(t.panel).radius(6.0).open();
        ui.icon(SPARK);
        ui.label(name);
        ui.spacer();
        ui.note("a note");
        ui.close();
    }
    ui.close();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().unwrap_or_else(|| "taut.png".into());
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 520.0, h: 300.0 }, scale);
    let (mut on, mut level) = (true, 6.0);
    ui.run(&mut |ui| describe(&mut on, &mut level, ui));
    std::fs::write(&file, Surface::draw(&mut ui, 64).png()).expect("the picture could not be saved");
    println!("saved {file}");
}
