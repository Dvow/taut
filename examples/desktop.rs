//! A whole screen drawn by this library alone: a desktop with a taskbar, a launcher, and
//! windows that move, resize, minimize, close and come to the front, each running a small
//! program. Nothing in it asks a system for anything. The interface renders into a screen-sized
//! pixel buffer, mouse and keyboard arrive as events, time is supplied, and the interface draws
//! the pointer itself: all a machine with a display, mouse and keyboard needs to provide for a
//! desktop.
//!
//!   cargo run -p taut --example desktop
//!       in a window, to try by hand (the window stands in for the screen)
//!   cargo run -p taut --example desktop -- --frames <folder>
//!       with no window at all: a short session plays on a screen that is only a pixel
//!       buffer, images of it are written to the folder, and each step's cost is reported
use std::path::Path;
use std::time::{Duration, Instant};
use taut::geom::{Point, Size, pt};
use taut::{Align, Button, Color, Event, Key, Length, LocalClipboard, Mods, Panel, Role, Surface, Theme, Ui, Window};

/// The programs of this desktop: each has a window of its own.
const PROGRAMS: [&str; 4] = ["Files", "Notes", "Monitor", "Settings"];
/// What the first of them lists.
const FILES: [(&str, &str); 6] = [("boot.cfg", "1 KB"), ("kernel.bin", "4,218 KB"), ("fonts", "folder"), ("notes.txt", "2 KB"), ("wallpaper.png", "812 KB"), ("users", "folder")];
/// The colours to choose from for what stands out.
const TINTS: [(&str, u32); 3] = [("Violet", 0x8b93ff), ("Green", 0x2fb38a), ("Amber", 0xe08a3c)];
/// How high the bar of tasks is.
const BAR: f32 = 44.0;

/// Everything the desktop keeps.
struct Desk {
    /// The window of each program: where it is, and whether it is open.
    windows: [Panel; 4],
    picked: usize,
    notes: String,
    light: bool,
    tint: usize,
    /// How long the machine has been up, as whoever runs the screen tells it.
    up: Duration,
    /// Nothing else draws a pointer on this screen: the desktop does.
    own_pointer: bool,
}

fn desk() -> Desk {
    let closed = |x: f32, y: f32, wide: f32, high: f32| Panel { open: false, ..Panel::new(x, y, wide, high) };
    let windows = [Panel::new(60.0, 50.0, 300.0, 300.0), closed(420.0, 90.0, 380.0, 280.0), closed(840.0, 60.0, 320.0, 210.0), closed(700.0, 380.0, 300.0, 170.0)];
    Desk { windows, picked: 1, notes: String::new(), light: false, tint: 0, up: Duration::ZERO, own_pointer: false }
}

/// The whole screen, as it is right now.
fn describe(desk: &mut Desk, ui: &mut Ui) {
    ui.theme = Theme { accent: Color::rgb(TINTS[desk.tint].1), ..if desk.light { Theme::light() } else { Theme::dark() } };
    let t = ui.theme;
    // The wall behind everything, in colours that go into each other. A right click on it
    // opens a menu of the programs.
    let far = if desk.light { Color::rgb(0xc9d3f2) } else { Color::rgb(0x1c2444) };
    let wall = ui.el().grow().gradient_along(150.0, &[(0.0, t.bg), (1.0, far)]).sense().show();
    if ui.context_menu(&wall) {
        launcher(desk, ui);
        ui.close();
    }
    // A window for each program that is open.
    for (number, name) in PROGRAMS.iter().enumerate() {
        if !ui.panel(&mut desk.windows[number], name) {
            continue;
        }
        match number {
            0 => files(desk, ui),
            1 => notes(desk, ui),
            2 => monitor(desk, ui),
            _ => settings(desk, ui),
        }
        ui.close();
    }
    tasks(desk, ui);
    if desk.own_pointer {
        ui.show_pointer();
    }
}

/// The rows of a menu that opens the programs.
fn launcher(desk: &mut Desk, ui: &mut Ui) {
    for (number, name) in PROGRAMS.iter().enumerate() {
        desk.windows[number].open |= ui.menu_item(name).clicked;
    }
}

/// The bar along the bottom of the screen, over everything: the launcher, a button for each
/// program (down while its window is open), and how long the machine has been up.
fn tasks(desk: &mut Desk, ui: &mut Ui) {
    let t = ui.theme;
    ui.pin(Align::Start, Align::End);
    let wide = ui.size().w;
    ui.row().w(Length::Px(wide)).h(Length::Px(BAR)).pad((8.0, 0.0)).gap(6.0).fill(t.panel).border(t.line, 1.0).open();
    let start = ui.primary_button("Start");
    if start.clicked {
        ui.open_popup(start.id, start.rect);
    }
    if ui.menu(start.id) {
        launcher(desk, ui);
        ui.close();
    }
    for (number, name) in PROGRAMS.iter().enumerate() {
        ui.toggle(&mut desk.windows[number].open, name);
    }
    ui.spacer();
    let seconds = desk.up.as_secs();
    ui.text(&format!("up {}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60), t.font, t.text_dim);
    // (The bar says the time again when the next second starts, and not before.)
    ui.wake_after(Duration::from_millis(1000 - desk.up.subsec_millis() as u64));
    ui.close();
}

fn files(desk: &mut Desk, ui: &mut Ui) {
    for (number, (name, _)) in FILES.iter().enumerate() {
        desk.picked = if ui.selectable(desk.picked == number, name).clicked { number } else { desk.picked };
    }
    let (name, size) = FILES[desk.picked];
    ui.note(&format!("{name}: {size}"));
}

fn notes(desk: &mut Desk, ui: &mut Ui) {
    ui.text_area(&mut desk.notes, "Write something", (8, 8));
    ui.note(&format!("{} letters", desk.notes.chars().count()));
}

/// A line that moves: drawn ten times a second for as long as its window is open, and at
/// no cost when it is not.
fn monitor(desk: &mut Desk, ui: &mut Ui) {
    // (Where the line stands goes by tenths of a second: between two of them it is the
    // same line, and nothing of it is drawn again.)
    let along = (desk.up.as_millis() / 100) as f32 * 0.3;
    let load: Vec<f32> = (0..64).map(|step| ((step as f32 * 0.2 + along).sin() * 0.35 + (step as f32 * 0.07 + along * 0.4).cos() * 0.25 + 0.5).clamp(0.0, 1.0)).collect();
    ui.plot_lines(&load, 296.0, 96.0);
    ui.progress(*load.last().unwrap_or(&0.0));
    ui.note("Drawn ten times a second while this window is open.");
    ui.wake_after(Duration::from_millis(100));
}

fn settings(desk: &mut Desk, ui: &mut Ui) {
    ui.switch(&mut desk.light, "Light colours");
    ui.segmented(&mut desk.tint, &TINTS.map(|(name, _)| name));
    ui.note("The whole screen takes these at once.");
}

/// The desktop in a window of the system this is run on: the window stands for the screen,
/// and the system draws the pointer.
fn in_a_window(mut desk: Desk) {
    let started = Instant::now();
    let screen = Window { title: "taut desktop", width: 1280.0, height: 720.0, min_width: 640.0, min_height: 400.0, ..Default::default() };
    taut::run(screen, move |ui| {
        desk.up = started.elapsed();
        describe(&mut desk, ui);
    });
}

/// A screen that is only a block of pixels, with the desktop on it: what a machine's own
/// code would keep, to show on its display.
struct Screen {
    ui: Ui,
    desk: Desk,
    pixels: Surface,
    /// Where the mouse is, and what the clock says.
    mouse: Point,
    clock: Duration,
    /// Each time the desktop was described and drawn: how many pixels were drawn, and how
    /// long both took.
    steps: Vec<(f32, Duration)>,
}

impl Screen {
    fn new(width: f32, height: f32) -> Screen {
        // (A machine with no system of its own would bring a font file, `OwnFonts`, or use the
        // letters built in.)
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: width, h: height }, 1.0);
        // (What a screen reader would be told is how this session finds its way: where the
        // button called "Start" is, where the window called "Notes".)
        ui.listen(true);
        let pixels = Surface::draw(&mut ui, 64);
        let mut screen = Screen { ui, desk: Desk { own_pointer: true, ..desk() }, pixels, mouse: pt(width / 2.0, height / 2.0), clock: Duration::ZERO, steps: Vec::new() };
        screen.does(Event::Move(screen.mouse));
        screen
    }

    /// Something happened: if the interface says that changes anything, the desktop is
    /// described again and the part of the screen that differs is drawn again.
    fn does(&mut self, event: Event) {
        self.clock += Duration::from_millis(16);
        self.ui.set_time(self.clock);
        self.desk.up = self.clock;
        let due = self.ui.wake_in() == Some(Duration::ZERO);
        if !self.ui.event(event) && !due && !self.steps.is_empty() {
            return;
        }
        let began = Instant::now();
        self.ui.run(&mut |ui| describe(&mut self.desk, ui));
        // Only the places that changed are drawn again: where the pointer was and where it
        // is now, not everything between the two.
        let places: Vec<taut::Rect> = self.ui.changes().collect();
        for place in &places {
            self.pixels.redraw(&mut self.ui, *place);
        }
        self.steps.push((places.iter().map(|place| place.w * place.h).sum(), began.elapsed()));
    }

    /// Where the thing of this kind that is called `name` lies on the screen.
    fn find(&self, role: Role, name: &str) -> Point {
        let said = self.ui.said().iter().find(|said| said.role == role && said.name == name).unwrap_or_else(|| panic!("nothing on the screen is called {name:?}"));
        pt(said.rect.x + said.rect.w / 2.0, said.rect.y + said.rect.h / 2.0)
    }

    /// Takes the mouse to a place, in a dozen steps as a hand would.
    fn mouse_to(&mut self, to: Point) {
        let from = self.mouse;
        for step in 1..=12 {
            let along = step as f32 / 12.0;
            self.does(Event::Move(pt((from.x + (to.x - from.x) * along).round(), (from.y + (to.y - from.y) * along).round())));
        }
        self.mouse = to;
    }
    fn click(&mut self, at: Point) {
        self.mouse_to(at);
        self.does(Event::Press(Button::Left, Mods::NONE));
        self.does(Event::Release(Button::Left));
    }
    /// Presses at one place, takes the mouse to another, and lets go there.
    fn drag(&mut self, from: Point, to: Point) {
        self.mouse_to(from);
        self.does(Event::Press(Button::Left, Mods::NONE));
        self.mouse_to(to);
        self.does(Event::Release(Button::Left));
    }
    /// Lets time pass, a sixtieth of a second at a time.
    fn wait(&mut self, time: Duration) {
        for _ in 0..time.as_millis() / 16 {
            self.does(Event::Focus(true));
        }
    }
    fn picture(&self, folder: &Path, name: &str) {
        std::fs::write(folder.join(format!("{name}.png")), self.pixels.png()).unwrap_or_else(|why| panic!("{name}.png could not be written: {why}"));
    }
}

/// Plays a short session on a screen that is only a block of pixels, writes pictures of it
/// into `folder`, and says what it cost.
fn play(folder: &Path) {
    std::fs::create_dir_all(folder).expect("the folder for the pictures could not be made");
    let mut screen = Screen::new(1280.0, 720.0);
    screen.picture(folder, "1-started");
    // Start, then Notes in the menu that opens: the window comes up, and takes what is typed.
    let start = screen.find(Role::Button, "Start");
    screen.click(start);
    screen.picture(folder, "2-launcher");
    let notes = screen.find(Role::MenuItem, "Notes");
    screen.click(notes);
    let page = screen.find(Role::TextBox, "Write something");
    screen.click(page);
    for line in ["Drawn by one library,", "with no window system under it."] {
        screen.does(Event::Text(line.into()));
        screen.does(Event::Key(Key::Enter, Mods::NONE));
    }
    screen.picture(folder, "3-notes");
    // The window is carried by its title, a little down and to the left.
    let title = screen.find(Role::Group, "Notes");
    let held = pt(title.x, title.y - 126.0);
    screen.drag(held, pt(held.x - 150.0, held.y + 60.0));
    // The monitor is opened from the bar, and left to run for a second; then the colours
    // of everything are changed in the settings.
    let monitor = screen.find(Role::Tick, "Monitor");
    screen.click(monitor);
    screen.wait(Duration::from_secs(1));
    screen.picture(folder, "4-monitor");
    let settings = screen.find(Role::Tick, "Settings");
    screen.click(settings);
    let light = screen.find(Role::Switch, "Light colours");
    screen.click(light);
    let green = screen.find(Role::Tab, "Green");
    screen.click(green);
    screen.wait(Duration::from_millis(300));
    screen.picture(folder, "5-light");

    // What was drawn piece by piece is what drawing all of it at once gives.
    let afresh = Surface::draw(&mut screen.ui, 64);
    assert!(afresh.pixels == screen.pixels.pixels, "drawing only what changed left the screen different from drawing all of it");
    // What a step cost as a rule (the middle one of them all: a move of the mouse), and
    // the dearest (all of the screen in other colours).
    let (all, count) = ((screen.pixels.width * screen.pixels.height) as f32, screen.steps.len());
    let share = |pixels: f32| pixels / all * 100.0;
    let millionths = |time: Duration| time.as_secs_f64() * 1e6;
    screen.steps.sort_by_key(|step| step.1);
    let (usual, dearest) = (screen.steps[count / 2], screen.steps[count - 1]);
    println!("the desktop was described and drawn {count} times");
    println!("as a rule {:.2}% of the screen was drawn again, in {:.0} millionths of a second", share(usual.0), millionths(usual.1));
    println!("at the most {:.0}% of it, in {:.0} millionths of a second", share(dearest.0), millionths(dearest.1));
    println!("pictures are in {}", folder.display());
}

fn main() {
    match std::env::args().skip_while(|arg| arg != "--frames").nth(1) {
        Some(folder) => play(Path::new(&folder)),
        None => in_a_window(desk()),
    }
}
