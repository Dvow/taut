//! How long the library takes, for windows of a hundred, a thousand and ten thousand rows. No
//! window opens: the interface runs and renders into memory, as a window would.
//!   cargo run -p taut --example bench --release
use std::hint::black_box;
use std::time::{Duration, Instant};
use taut::geom::{Size, pt};
use taut::text::BuiltInFont;
use taut::{Button, Camera, Color, Event, Icon, LocalClipboard, MeshInstance, Mods, Rect, Surface, Ui};

const STAR: Icon = Icon::line("M8 2.2 9.8 5.9 13.8 6.4 10.9 9.2 11.6 13.2 8 11.3 4.4 13.2 5.1 9.2 2.2 6.4 6.2 5.9z");

/// A bar and a list of rows, each row seven boxes: the shape of a real window.
fn describe(ui: &mut Ui, rows: usize) {
    let t = ui.theme;
    ui.row().pad((10.0, 8.0)).gap(8.0).fill(t.panel).open();
    ui.label("Notes");
    ui.spacer();
    ui.primary_button("Save all");
    ui.close();
    ui.column().grow().scroll().pad(10.0).gap(6.0).open();
    for i in 0..rows {
        ui.row().gap(8.0).pad((10.0, 5.0)).fill(t.panel).radius(6.0).open();
        ui.icon(STAR);
        ui.label("a note with a name");
        ui.note("kb");
        ui.spacer();
        ui.note("today");
        ui.button(if i % 2 == 0 { "Open" } else { "Edit" });
        ui.close();
    }
    ui.close();
}

/// The time one go of `work` takes, as the quickest of several: in millionths of a second.
fn micros(times: usize, mut work: impl FnMut()) -> f64 {
    let quickest = (0..times).map(|_| (Instant::now(), work()).0.elapsed()).min().unwrap_or(Duration::ZERO);
    quickest.as_secs_f64() * 1e6
}

/// A hilly sheet of so many squares each way, two triangles to a square.
fn hills(across: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut points, mut triangles) = (Vec::new(), Vec::new());
    for at in 0..(across + 1) * (across + 1) {
        let (x, z) = ((at % (across + 1)) as f32 / across as f32 - 0.5, (at / (across + 1)) as f32 / across as f32 - 0.5);
        points.push([x * 100.0, ((x * 23.0).sin() + (z * 17.0).cos()) * 3.5, z * 100.0]);
    }
    for at in (0..(across + 1) * across).filter(|at| at % (across + 1) != across) {
        triangles.extend([[at, at + 1, at + across + 1], [at + 1, at + across + 2, at + across + 1]]);
    }
    (points, triangles)
}

/// A scene of a million triangles that fills a window of 1920 by 1080, seen from a place
/// that moves a little with every frame: how long a frame takes, from describing the window
/// to its pixels being there. With TAUT_BENCH_SHOT set to a file's name, the last picture
/// is written there as a PNG.
fn scenes() {
    let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 1920.0, h: 1080.0 }, 1.0);
    let (points, triangles) = hills(708);
    println!(
        "
a scene of {} triangles in a window of 1920 by 1080, the eye moving",
        triangles.len()
    );
    let body = ui.add_mesh(points, triangles);
    let (ground, fill, line) = (Color::rgb(0x10141a), Color::rgb(0x4080c0), Color::rgb(0xd8e4f0));
    let looks = [("filled, in light", body.filled(fill)), ("filled, with outlines", body.filled(fill).outlined(line.with_alpha(0.5))), ("outlines alone", body.outlined(line))];
    for (name, shown) in looks {
        let mut frame = 0;
        let describe = |ui: &mut Ui, frame: i32| {
            let view = Camera::look_at([frame as f32 * 0.25 - 4.0, 40.0, -90.0], [0.0, 0.0, 0.0]);
            ui.el().grow().fill(ground).scene(&view, &[shown as MeshInstance]).show();
        };
        ui.run(&mut |ui| describe(ui, 0));
        let mut screen = Surface::draw(&mut ui, 64);
        let took = micros(40, || {
            frame += 1;
            let changed = ui.run(&mut |ui| describe(ui, frame % 32));
            screen.redraw(&mut ui, changed);
        });
        println!("{name:>24} {:>7.2} ms a frame", took / 1000.0);
        if let Some(file) = std::env::var_os("TAUT_BENCH_SHOT").filter(|_| name.contains("with")) {
            std::fs::write(file, screen.to_png()).expect("the picture could not be written");
        }
    }
}

fn main() {
    println!("{:>7} {:>7} {:>12} {:>13} {:>14} {:>12} {:>14} {:>16}", "rows", "boxes", "run, same", "run, pointer", "draw a change", "draw it all", "a quiet move", "press to pixels");
    for rows in [100, 1000, 10_000] {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 1200.0, h: 800.0 }, 1.0);
        ui.run(&mut |ui| describe(ui, rows));
        let mut screen = Surface::draw(&mut ui, 64);
        // Nothing happened: the interface is run again and finds nothing to draw.
        let same = micros(50, || assert_eq!(ui.run(&mut |ui| describe(ui, rows)), Rect::EMPTY));
        // The pointer moves from one row's button to the next row's: two buttons change.
        let mut at = 0;
        let mut changed = Rect::EMPTY;
        let moved = micros(50, || {
            at = (at + 1) % 8;
            ui.event(Event::Move(pt(1150.0, 80.0 + at as f32 * 44.0)));
            changed = ui.run(&mut |ui| describe(ui, rows));
        });
        let part = micros(50, || screen.redraw(&mut ui, changed));
        let all = micros(10, || screen.redraw(&mut ui, Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 }));
        // What the person waits for. The pointer moves without coming over anything new: the
        // interface hears of it, and has nothing to do. (In thousand-millionths of a second.)
        let moves = 200_000;
        let start = Instant::now();
        for step in 0..moves {
            black_box(ui.event(Event::Move(pt(300.0 + (step % 2) as f32, 20.0))));
        }
        let quiet = start.elapsed().as_secs_f64() * 1e9 / moves as f64;
        // A button is pressed: from the press to the pixels of the pressed button being there.
        ui.event(Event::Move(pt(1150.0, 124.0)));
        ui.run(&mut |ui| describe(ui, rows));
        let pressed = micros(50, || {
            ui.event(Event::Press(Button::Left, Mods::NONE));
            let changed = ui.run(&mut |ui| describe(ui, rows));
            screen.redraw(&mut ui, changed);
            ui.event(Event::Release(Button::Left));
            let changed = ui.run(&mut |ui| describe(ui, rows));
            screen.redraw(&mut ui, changed);
        }) / 2.0;
        println!("{rows:>7} {:>7} {same:>9.0} µs {moved:>10.0} µs {part:>11.0} µs {all:>9.0} µs {quiet:>11.0} ns {pressed:>13.0} µs", rows * 7 + 6);
    }
    scenes();
}
