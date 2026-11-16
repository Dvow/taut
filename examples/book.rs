//! Draws every picture of the book (the chapters in `docs/`) into a folder, with no window
//! anywhere: each is a small window described by the code its chapter shows, used the way a
//! person would use it, and drawn into a `Surface` (with the feature `emoji`, for the emoji in
//! colour of the sixth chapter).
//!   cargo run -p taut --features emoji --example book -- docs/pictures [a part of a name: only those pictures]
//!
//! Each module below holds, before the line that says "The picture of it", the very code of a
//! block in the chapter named over it (the book's test holds the two against each other); after
//! that line, what the person does before the picture is taken.
#![allow(dead_code)] // the chapters' programs have a `main`, which is never called here
use std::path::{Path, PathBuf};
use std::time::Duration;
use taut::geom::{Point, Size, pt};
use taut::{Button, Event, Key, LocalClipboard, Mods, Rect, Surface, Ui};

/// One thing the person does before a picture is taken. A thing is found by its name, the
/// way a screen reader finds it: the words on a button, the hint in an empty text box.
#[derive(Clone, Copy)]
enum Step {
    /// The pointer goes to the thing, and the left button is pressed and let go there.
    Click(&'static str),
    /// The left button is pressed and let go at this place, in units.
    ClickAt(f32, f32),
    /// The same with the right button.
    RightClick(&'static str),
    /// The pointer goes to the thing, and stays there.
    Hover(&'static str),
    /// Text is typed into whatever the keyboard is on.
    Type(&'static str),
    /// A key is pressed.
    Press(Key, Mods),
    /// The wheel is turned over the thing, this far down.
    Wheel(&'static str, f32),
}

/// A picture of the book: its name (the file is `<name>.png`), how big the window is in units,
/// what the person does first, and the window's description.
struct Shot {
    name: &'static str,
    size: [f32; 2],
    steps: Vec<Step>,
    describe: Box<dyn FnMut(&mut Ui)>,
    /// The part of the window the last step had drawn again is marked on the picture.
    marked: bool,
    /// How many pixels a unit is.
    scale: f32,
    /// The picture is laid over one of a game's, as a texture is.
    over_a_game: bool,
}

impl Shot {
    fn new(name: &'static str, size: [f32; 2], describe: impl FnMut(&mut Ui) + 'static) -> Shot {
        Shot { name, size, steps: Vec::new(), describe: Box::new(describe), marked: false, scale: 1.0, over_a_game: false }
    }
    fn steps(self, steps: &[Step]) -> Shot {
        Shot { steps: steps.to_vec(), ..self }
    }
    fn marked(self) -> Shot {
        Shot { marked: true, ..self }
    }
    fn scale(self, scale: f32) -> Shot {
        Shot { scale, ..self }
    }
    fn over_a_game(self) -> Shot {
        Shot { over_a_game: true, ..self }
    }
}

/// Where the thing called `name` is in the window, as a screen reader is told.
fn find(ui: &Ui, name: &str) -> Rect {
    let found = ui.said().iter().find(|said| said.name == name);
    found.unwrap_or_else(|| panic!("nothing called {name:?} is in the window")).rect
}

fn middle(of: Rect) -> Point {
    pt(of.x + of.w / 2.0, of.y + of.h / 2.0)
}

/// Takes in one thing that happened, runs the interface, and gives back what it drew again.
fn happens(ui: &mut Ui, describe: &mut dyn FnMut(&mut Ui), screen: &mut Surface, event: Event) -> Rect {
    ui.event(event);
    let changed = ui.run(describe);
    screen.redraw(ui, changed);
    changed
}

/// Does one step, and gives back what the interface drew again for it.
fn take(ui: &mut Ui, describe: &mut dyn FnMut(&mut Ui), screen: &mut Surface, step: Step) -> Rect {
    let events = match step {
        Step::Click(name) => vec![Event::Move(middle(find(ui, name))), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)],
        Step::ClickAt(x, y) => vec![Event::Move(pt(x, y)), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)],
        Step::RightClick(name) => vec![Event::Move(middle(find(ui, name))), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)],
        Step::Hover(name) => vec![Event::Move(middle(find(ui, name)))],
        Step::Type(text) => vec![Event::Text(text.into())],
        Step::Press(key, mods) => vec![Event::Key(key, mods)],
        Step::Wheel(name, down) => vec![Event::Move(middle(find(ui, name))), Event::Wheel(0.0, down)],
    };
    let mut changed = Rect::EMPTY;
    for event in events {
        changed = changed.union(happens(ui, describe, screen, event));
    }
    changed
}

/// Takes the picture: the window is described, the steps are taken one after another (a
/// second apart, so that what slides has come to rest), and it is written as a PNG file.
fn shoot(folder: &Path, mut shot: Shot) {
    let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: shot.size[0], h: shot.size[1] }, shot.scale);
    ui.listen(true);
    ui.run(&mut shot.describe);
    ui.run(&mut shot.describe);
    let mut screen = Surface::draw(&mut ui, 64);
    let mut changed = Rect::EMPTY;
    for (at, step) in shot.steps.iter().enumerate() {
        ui.set_time(Duration::from_secs(at as u64 + 1));
        changed = take(&mut ui, &mut shot.describe, &mut screen, *step);
    }
    ui.set_time(Duration::from_secs(shot.steps.len() as u64 + 2));
    let rest = ui.run(&mut shot.describe);
    screen.redraw(&mut ui, rest);
    if shot.marked {
        mark(&mut screen, changed.on_pixels(shot.scale));
    }
    if shot.over_a_game {
        lay_over_a_game(&mut screen);
    }
    let file = folder.join(format!("{}.png", shot.name));
    std::fs::write(&file, screen.png()).expect("the picture could not be written");
    println!("{} ({} by {})", file.display(), screen.width, screen.height);
}

/// Draws a line two pixels thick around `area`, in a red that stands out on either theme.
fn mark(screen: &mut Surface, area: Rect) {
    let [left, top] = [area.x.max(0.0) as usize, area.y.max(0.0) as usize];
    let [right, bottom] = [(area.right() as usize).min(screen.width), (area.bottom() as usize).min(screen.height)];
    for y in top..bottom {
        for x in left..right {
            if x < left + 2 || x + 2 >= right || y < top + 2 || y + 2 >= bottom {
                screen.pixels[y * screen.width + x] = 0xffe0_3c3c;
            }
        }
    }
}

/// Lays the picture over one a game might show (a sky, and hills under it), the way a
/// renderer lays a texture whose colours are multiplied by how solid they are over what it
/// drew: each colour, plus what is under it as far as the picture lets it through.
fn lay_over_a_game(screen: &mut Surface) {
    let (wide, high) = (screen.width as f32, screen.height as f32);
    for (at, pixel) in screen.pixels.iter_mut().enumerate() {
        let (x, y) = ((at % screen.width) as f32, (at / screen.width) as f32);
        let hill = high * 0.62 + (x / wide * 9.0).sin() * high * 0.08 + (x / wide * 23.0).cos() * high * 0.03;
        let under = if y > hill { [40.0 + y / high * 30.0, 110.0 - y / high * 40.0, 60.0] } else { [90.0 + y / high * 80.0, 150.0 + y / high * 60.0, 225.0] };
        let let_through = 255 - (*pixel >> 24);
        let channel = |shift: u32, game: f32| ((*pixel >> shift & 0xff) + (game as u32 * let_through) / 255).min(255) << shift;
        *pixel = 0xff00_0000 | channel(16, under[0]) | channel(8, under[1]) | channel(0, under[2]);
    }
}

fn main() {
    let folder = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "docs/pictures".into()));
    std::fs::create_dir_all(&folder).expect("the folder could not be made");
    let wanted = std::env::args().nth(2).unwrap_or_default();
    let shots = [
        first_window::shot(),
        first_window::drawn(),
        a_row::shot(),
        a_column::shot(),
        three_sizes::shot(),
        sharing::shot(),
        aligned::shot(),
        placed::shot(),
        a_grid::shot(),
        wrapping::shot(),
        giving_way::shot(),
        scrolling::shot(),
        layered::shot(),
        chips::shot(),
        a_list::shot(),
        a_table::shot(),
        a_long_list::shot(),
        a_tree::shot(),
        menus::shot(),
        a_dock::shot(),
        a_chart::shot(),
        a_panel::shot(),
        a_palette::shot(),
        a_notice::shot(),
        kinds_of_text::shot(),
        emoji::shot(),
        pictures::shot(),
        drawing::shot(),
        a_scene::shot(),
        an_overlay::shot(),
        a_whole_screen::shot(),
    ];
    for shot in shots.into_iter().chain(todo::shots()).chain(a_form::shots()).filter(|shot| shot.name.contains(&wanted)) {
        shoot(&folder, shot);
    }
}

// docs/01-a-first-window.md
mod first_window {
    use taut::{Ui, Window};

    struct App {
        count: u32,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        ui.row().pad(16.0).gap(12.0).open();
        ui.label(&format!("Pressed {} times", app.count));
        if ui.button("Press me").clicked {
            app.count += 1;
        }
        ui.close();
    }

    fn main() {
        let mut app = App { count: 0 };
        taut::run(Window { title: "A first window", width: 320.0, height: 120.0, ..Default::default() }, move |ui| describe(&mut app, ui));
    }

    // The picture of it:
    use super::{Shot, Step::Click};
    pub(super) fn shot() -> Shot {
        let mut app = App { count: 0 };
        Shot::new("01-first-window", [320.0, 120.0], move |ui| describe(&mut app, ui)).steps(&[Click("Press me"), Click("Press me"), Click("Press me")])
    }
    pub(super) fn drawn() -> Shot {
        let mut app = App { count: 0 };
        Shot::new("01-drawn-again", [320.0, 120.0], move |ui| describe(&mut app, ui)).steps(&[Click("Press me"), Click("Press me"), Click("Press me")]).marked()
    }
}

// docs/02-boxes-and-layout.md
mod a_row {
    use taut::Ui;

    fn describe(ui: &mut Ui) {
        ui.row().pad(12.0).gap(8.0).open();
        ui.button("One");
        ui.button("Two");
        ui.button("Three");
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-a-row", [360.0, 52.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod a_column {
    use taut::Ui;

    fn describe(ui: &mut Ui) {
        ui.column().pad(12.0).gap(8.0).open();
        ui.button("One");
        ui.button("Two");
        ui.button("Three");
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-a-column", [360.0, 124.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod three_sizes {
    use taut::{Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.row().pad(12.0).gap(8.0).open();
        ui.el().text("Fit", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.el().w(Length::Px(120.0)).text("Px(120.0)", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.el().w(Length::Grow(1.0)).text("Grow(1.0)", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-three-sizes", [420.0, 58.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod sharing {
    use taut::{Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.column().pad(12.0).gap(8.0).open();
        ui.row().gap(8.0).open();
        ui.el().w(Length::Grow(1.0)).text("Grow(1.0)", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.el().w(Length::Grow(2.0)).text("Grow(2.0)", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.close();
        ui.row().gap(8.0).open();
        ui.button("Back");
        ui.spacer();
        ui.button("Cancel");
        ui.primary_button("Save");
        ui.close();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-sharing", [420.0, 102.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod aligned {
    use taut::{Align, Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.column().pad(12.0).gap(8.0).open();
        for (name, along) in [("Start", Align::Start), ("Center", Align::Center), ("End", Align::End), ("Spread", Align::Spread)] {
            ui.row().h(Length::Px(40.0)).pad(4.0).gap(4.0).align(along, Align::Center).fill(t.panel).border(t.line, 1.0).open();
            ui.button(name);
            ui.button("and");
            ui.button("more");
            ui.close();
        }
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-aligned", [420.0, 216.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod placed {
    use taut::{Align, Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.column().pad(12.0).gap(8.0).open();
        ui.button("Stretched across, as a column has its boxes");
        ui.el().place(Align::End).text("At the end", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.row().h(Length::Px(56.0)).gap(8.0).fill(t.panel).border(t.line, 1.0).open();
        ui.button("In the middle of the height");
        ui.el().place(Align::Start).text("At the top", t.font, t.text).pad(8.0).fill(t.raise).border(t.line_strong, 1.0).show();
        ui.close();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-placed", [420.0, 160.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod a_grid {
    use taut::{Align, Ui};

    struct Form {
        name: String,
        place: String,
        rows: usize,
    }

    fn describe(form: &mut Form, ui: &mut Ui) {
        ui.grid(2).pad(12.0).gap(8.0).align(Align::Start, Align::Center).open();
        ui.label("Name");
        ui.text_box(&mut form.name, "Your name");
        ui.label("Where it is kept");
        ui.text_box(&mut form.place, "A folder");
        ui.label("Rows");
        ui.select(&mut form.rows, &["Roomy", "Tight"]);
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut form = Form { name: "Ada".into(), place: String::new(), rows: 0 };
        super::Shot::new("02-a-grid", [420.0, 132.0], move |ui| describe(&mut form, ui))
    }
}

// docs/02-boxes-and-layout.md
mod wrapping {
    use taut::Ui;

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.wrapping_row().pad(12.0).gap(6.0).open();
        for tag in ["layout", "drawing", "text", "tables", "menus", "docks", "scenes", "screen readers", "fonts", "pictures"] {
            ui.el().text(tag, t.small, t.text_dim).pad((8.0, 3.0)).fill(t.raise).border(t.line, 1.0).radius(10.0).show();
        }
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-wrapping", [300.0, 100.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod giving_way {
    use taut::{Length, Ui};

    fn file(ui: &mut Ui, name: &str, folder: &str) {
        let t = ui.theme;
        ui.row().gap(8.0).pad((10.0, 6.0)).fill(t.panel).radius(6.0).open();
        ui.el().text(name, t.font, t.text).gives_way(1).show();
        ui.el().text(folder, t.small, t.text_dim).gives_way(2).show();
        ui.spacer();
        ui.button("Open");
        ui.close();
    }

    fn describe(ui: &mut Ui) {
        ui.column().pad(12.0).gap(8.0).open();
        for width in [420.0, 300.0, 200.0] {
            ui.column().w(Length::Px(width)).open();
            file(ui, "a report with a long name.txt", "D:\\Work\\Reports\\2026\\November");
            ui.close();
        }
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-giving-way", [444.0, 148.0], describe)
    }
}

// docs/02-boxes-and-layout.md
mod scrolling {
    use taut::{Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.column().h(Length::Px(160.0)).scroll().pad(12.0).gap(4.0).open();
        ui.el().text("Kept at the top", t.font.bold(), t.text).pad((0.0, 4.0)).fill(t.bg).kept().show();
        for row in 1..=30 {
            ui.row().pad((10.0, 6.0)).fill(t.panel).radius(4.0).open();
            ui.label(&format!("Row {row}"));
            ui.close();
        }
        ui.close();
    }

    // The picture of it:
    use super::Step::Wheel;
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-scrolling", [300.0, 160.0], describe).steps(&[Wheel("Row 2", 130.0)])
    }
}

// docs/02-boxes-and-layout.md
mod layered {
    use taut::{Align, Flow, Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.row().pad(16.0).open();
        ui.el().flow(Flow::Layers).size(220.0, 90.0).open();
        ui.column().grow().pad(12.0).gap(4.0).fill(t.panel).border(t.line, 1.0).radius(8.0).open();
        ui.text("Inbox", t.font.bold(), t.text);
        ui.note("Everything that came today");
        ui.close();
        ui.el().place((Align::End, Align::Start)).at(6.0, -6.0).text("12", t.small, t.on_accent).pad((7.0, 2.0)).fill(t.red).radius(9.0).show();
        ui.el().place(Align::End).h(Length::Px(4.0)).w(Length::Px(60.0)).at(-12.0, -12.0).fill(t.accent).radius(2.0).show();
        ui.close();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("02-layered", [300.0, 130.0], describe)
    }
}

// docs/03-parts-and-state.md
mod chips {
    use taut::{Cursor, Response, Role, Ui};

    /// A small rounded word that is on or off: a tag to pick.
    fn chip(ui: &mut Ui, on: &mut bool, name: &str) -> Response {
        let t = ui.theme;
        let mut state = ui.press(ui.next_id());
        if state.clicked {
            (*on, state.changed) = (!*on, true);
        }
        if state.hovered {
            ui.set_cursor(Cursor::Hand);
        }
        let (fill, ink) = match (*on, state.hovered) {
            (true, _) => (t.accent, t.on_accent),
            (false, true) => (t.hover, t.text),
            (false, false) => (t.raise, t.text_dim),
        };
        ui.el().text(name, t.font, ink).pad((12.0, 5.0)).fill(fill).border(t.line, 1.0).radius(14.0).sense().focusable().show();
        ui.tell(&state, Role::Tick, Some(*on), "");
        state
    }

    struct Shopping {
        wanted: [bool; 6],
    }

    fn describe(shopping: &mut Shopping, ui: &mut Ui) {
        ui.wrapping_row().pad(12.0).gap(6.0).open();
        for (on, name) in shopping.wanted.iter_mut().zip(["Fruit", "Bread", "Cheese", "Tea", "Rice", "Soap"]) {
            chip(ui, on, name);
        }
        ui.close();
    }

    // The picture of it:
    use super::Step::{Click, Hover};
    pub(super) fn shot() -> super::Shot {
        let mut shopping = Shopping { wanted: [false; 6] };
        super::Shot::new("03-chips", [360.0, 92.0], move |ui| describe(&mut shopping, ui)).steps(&[Click("Bread"), Click("Tea"), Hover("Rice")])
    }
}

// docs/04-a-real-small-app.md
mod a_list {
    use taut::{Ui, Window};

    struct Task {
        text: String,
        done: bool,
    }

    fn list(tasks: &mut [Task], ui: &mut Ui) {
        let t = ui.theme;
        ui.column().grow().scroll().pad(12.0).gap(4.0).open();
        for task in tasks.iter_mut() {
            ui.row().pad((10.0, 4.0)).fill(t.panel).radius(6.0).open();
            ui.checkbox(&mut task.done, &task.text);
            ui.close();
        }
        ui.close();
    }

    fn main() {
        let task = |text: &str, done| Task { text: text.into(), done };
        let mut tasks = vec![task("Buy bread", true), task("Write to Ada", false), task("Fix the gate", false)];
        taut::run(Window { title: "To do", width: 400.0, height: 200.0, ..Default::default() }, move |ui| list(&mut tasks, ui));
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let task = |text: &str, done| Task { text: text.into(), done };
        let mut tasks = vec![task("Buy bread", true), task("Write to Ada", false), task("Fix the gate", false)];
        super::Shot::new("04-a-list", [400.0, 200.0], move |ui| list(&mut tasks, ui))
    }
}

// docs/04-a-real-small-app.md
mod todo {
    //! A list of things to do: the app the fourth chapter of the book builds, step by step
    //! (`docs/04-a-real-small-app.md`). Tasks are added, ticked, renamed and taken away; the list
    //! shows all of them, those left, or those done; and it is kept in a file between runs,
    //! together with where the window was.
    //!   cargo run -p taut --example todo
    use std::path::PathBuf;
    use taut::{Align, Icon, Key, Mods, Placed, Theme, Ui, Window};

    const PEN: Icon = Icon::line("M3.5 12.5 4 10 10.5 3.5 12.5 5.5 6 12zM9 5l2 2");
    const CROSS: Icon = Icon::line("M4.5 4.5 11.5 11.5M11.5 4.5 4.5 11.5");
    /// Which tasks the list shows.
    const SHOWN: [&str; 3] = ["All", "To do", "Done"];

    /// One thing to do.
    struct Task {
        /// What it is known by, whatever its place in the list.
        id: u64,
        text: String,
        done: bool,
    }

    /// Everything the app keeps.
    struct App {
        tasks: Vec<Task>,
        /// What is typed into the box for a new task, so far.
        draft: String,
        /// Which tasks show: the number of one of `SHOWN`.
        shown: usize,
        /// The task being renamed, and its new name so far.
        renaming: Option<(u64, String)>,
        /// The number the next new task is known by.
        next: u64,
        /// Where the window is, as it last said.
        placed: Option<Placed>,
        /// The file the list is kept in.
        file: PathBuf,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        ui.theme = if ui.system_dark() { Theme::dark() } else { Theme::light() };
        app.placed = ui.placed().or(app.placed);
        let mut changed = adding(app, ui);
        showing(app, ui);
        changed |= list(app, ui);
        changed |= renaming(app, ui);
        if changed && app.save().is_err() {
            ui.notice("The list could not be saved");
        }
    }

    /// The box a new task is typed into, and the button that adds it. Says whether one was added.
    fn adding(app: &mut App, ui: &mut Ui) -> bool {
        ui.row().pad(12.0).gap(8.0).open();
        let typed = ui.text_box(&mut app.draft, "Something to do");
        let add = ui.primary_button("Add").clicked || typed.focused && ui.key(Key::Enter, Mods::NONE);
        ui.close();
        // When the keyboard is on nothing (as the app starts), it goes to this box.
        if ui.focus() == 0 && app.renaming.is_none() {
            ui.set_focus(typed.id);
        }
        if !add || app.draft.trim().is_empty() {
            return false;
        }
        app.tasks.push(Task { id: app.next, text: app.draft.trim().into(), done: false });
        app.next += 1;
        app.draft.clear();
        true
    }

    /// Which tasks show, and how many are left to do.
    fn showing(app: &mut App, ui: &mut Ui) {
        let left = app.tasks.iter().filter(|task| !task.done).count();
        ui.row().pad((12.0, 0.0)).gap(8.0).open();
        ui.segmented(&mut app.shown, &SHOWN);
        ui.spacer();
        ui.note(&format!("{left} left to do"));
        ui.close();
    }

    /// The tasks that show, each with a tick box and buttons to rename it and to take it away.
    /// Says whether one was ticked or taken away.
    fn list(app: &mut App, ui: &mut Ui) -> bool {
        let t = ui.theme;
        let (mut changed, mut gone, mut none) = (false, None, true);
        ui.column().grow().scroll().pad(12.0).gap(4.0).open();
        for task in app.tasks.iter_mut().filter(|task| [true, !task.done, task.done][app.shown]) {
            none = false;
            ui.row().key(task.id).pad((10.0, 4.0)).gap(4.0).fill(t.panel).radius(6.0).open();
            changed |= ui.checkbox(&mut task.done, &task.text).changed;
            ui.spacer();
            if ui.icon_button(PEN, "Rename").clicked {
                app.renaming = Some((task.id, task.text.clone()));
            }
            if ui.icon_button(CROSS, "Take away").clicked {
                gone = Some(task.id);
            }
            ui.close();
        }
        if none {
            ui.note(["Nothing to do yet.", "Nothing left to do.", "Nothing done yet."][app.shown]);
        }
        ui.close();
        app.tasks.retain(|task| Some(task.id) != gone);
        changed || gone.is_some()
    }

    /// The dialog a task is renamed in, while one is. Says whether a task got a new name.
    fn renaming(app: &mut App, ui: &mut Ui) -> bool {
        let Some((id, name)) = &mut app.renaming else { return false };
        let t = ui.theme;
        let beside = ui.dialog();
        ui.text("Rename the task", t.font.bold(), t.text);
        let field = ui.text_box(name, "What is to be done");
        // (As the dialog opens, the keyboard goes into its box, with the old name selected.)
        if ui.focus() == 0 {
            ui.set_focus(field.id);
            ui.select_all();
        }
        ui.row().gap(8.0).align(Align::End, Align::Center).open();
        let cancel = ui.button("Cancel").clicked || ui.key(Key::Escape, Mods::NONE) || beside.clicked;
        let keep = ui.primary_button("Rename").clicked || ui.key(Key::Enter, Mods::NONE);
        ui.close();
        ui.close();
        let renamed = keep && !name.trim().is_empty();
        if let Some(task) = app.tasks.iter_mut().find(|task| task.id == *id).filter(|_| renamed) {
            task.text = name.trim().into();
        }
        if keep || cancel {
            app.renaming = None;
        }
        renamed
    }

    impl App {
        /// The list as it was left in `file`, or an empty one the first time.
        fn load(file: PathBuf) -> App {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let mut lines = text.lines();
            let placed = lines.next().and_then(|line| line.parse().ok());
            let tasks: Vec<Task> = lines.enumerate().map(|(at, line)| Task { id: at as u64, done: line.starts_with("x "), text: line.get(2..).unwrap_or("").into() }).collect();
            App { next: tasks.len() as u64, tasks, draft: String::new(), shown: 0, renaming: None, placed, file }
        }

        /// Writes the list into its file: where the window is on the first line, then a line for
        /// each task, with `x` before one that is done and `-` before one that is not.
        fn save(&self) -> std::io::Result<()> {
            let mut text = self.placed.map_or(String::new(), |placed| placed.to_string());
            for task in &self.tasks {
                text += &format!("\n{} {}", if task.done { "x" } else { "-" }, task.text);
            }
            std::fs::write(&self.file, text)
        }
    }

    /// When the window closes, the app goes with it, and the list is written once more: now with
    /// where the window was left. (Nobody is left to tell if that fails.)
    impl Drop for App {
        fn drop(&mut self) {
            let _ = self.save();
        }
    }

    /// Where the list is kept: in the folder the system keeps the settings of programs in.
    fn file() -> PathBuf {
        let folder = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")).map(PathBuf::from);
        let home = || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"));
        folder.or_else(home).unwrap_or_default().join("taut-todo.txt")
    }

    fn main() {
        let mut app = App::load(file());
        let window = Window { title: "To do", width: 400.0, height: 480.0, min_width: 280.0, min_height: 220.0, ..Default::default() };
        taut::run(window.placed(app.placed), move |ui| describe(&mut app, ui));
    }

    // The picture of it:
    use super::Step::{Click, Type};

    /// The app with a few tasks in it, kept in a file of the book's own.
    fn sample() -> App {
        let tasks = [("Buy bread", true), ("Write to Ada", false), ("Fix the gate", false), ("Book the train", true), ("Plan the garden", false)];
        let tasks = tasks.iter().enumerate().map(|(id, (text, done))| Task { id: id as u64, text: text.to_string(), done: *done }).collect();
        App { tasks, draft: String::new(), shown: 0, renaming: None, next: 5, placed: None, file: std::env::temp_dir().join("taut-book-todo.txt") }
    }

    pub(super) fn shots() -> Vec<super::Shot> {
        let (mut first, mut second, mut third) = (sample(), sample(), sample());
        vec![
            super::Shot::new("04-the-app", [400.0, 480.0], move |ui| describe(&mut first, ui)).steps(&[Type("Water the plants")]),
            super::Shot::new("04-to-do", [400.0, 480.0], move |ui| describe(&mut second, ui)).steps(&[Click("To do")]),
            super::Shot::new("04-renaming", [400.0, 480.0], move |ui| (ui.set_wishes(taut::Wishes { dark: true, ..Default::default() }), describe(&mut third, ui)).1).steps(&[Click("Rename")]),
        ]
    }
}

// docs/05-larger-parts.md
mod a_table {
    use std::cmp::Ordering;
    use taut::{Align, Column, Picks, Table, Ui};

    const COLUMNS: [Column; 3] = [Column::new("Name"), Column::new("Size").px(90.0).align(Align::End), Column::new("Kind").px(80.0)];

    struct File {
        name: String,
        size: u64,
        kind: &'static str,
    }

    struct App {
        files: Vec<File>,
        table: Table,
        picks: Picks,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        if ui.table(&mut app.table, &COLUMNS).changed {
            let by = app.table.sort.clone();
            app.files.sort_by(|a, b| order(a, b, &by));
            app.picks.clear();
        }
        for row in ui.rows(app.files.len(), 24.0) {
            if ui.pick_row(row, app.picks.has(row)).clicked {
                app.picks.click(row, ui.mods());
            }
            let file = &app.files[row];
            ui.label(&file.name);
            ui.label(&format!("{} KB", file.size));
            ui.label(file.kind);
            ui.close();
        }
        ui.close();
        ui.close();
    }

    /// How two files compare in the order the headings ask for: by the column that counts
    /// most, and where that is the same, by the next.
    fn order(a: &File, b: &File, by: &[(usize, bool)]) -> Ordering {
        for &(column, large_first) in by {
            let (a, b) = if large_first { (b, a) } else { (a, b) };
            let order = match column {
                0 => a.name.cmp(&b.name),
                1 => a.size.cmp(&b.size),
                _ => a.kind.cmp(b.kind),
            };
            if order.is_ne() {
                return order;
            }
        }
        Ordering::Equal
    }

    // The picture of it:
    use super::Step::{Click, ClickAt};
    pub(super) fn shot() -> super::Shot {
        let kinds = ["text", "picture", "sound", "folder"];
        let files = (0..500u64).map(|n| File { name: format!("file {n:03}"), size: n * 7919 % 4096 + 1, kind: kinds[n as usize * 5 % 4] }).collect();
        let mut app = App { files, table: Table::default(), picks: Picks::default() };
        super::Shot::new("05-a-table", [420.0, 220.0], move |ui| describe(&mut app, ui)).steps(&[Click("Size"), Click("Size"), ClickAt(200.0, 116.0)])
    }
}

// docs/05-larger-parts.md
mod a_long_list {
    use taut::Ui;

    fn describe(ui: &mut Ui) {
        for row in ui.list(100_000, 24.0) {
            ui.row().pad((12.0, 0.0)).open();
            ui.label(&format!("Row {} of 100000", row + 1));
            ui.close();
        }
        ui.close();
    }

    // The picture of it:
    use taut::{Key, Mods};
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("05-a-long-list", [300.0, 130.0], describe).steps(&[super::Step::Press(Key::End, Mods::NONE)])
    }
}

// docs/05-larger-parts.md
mod a_tree {
    use taut::{Length, Ui};

    struct Folders {
        open: [bool; 3],
        picked: usize,
    }

    fn describe(app: &mut Folders, ui: &mut Ui) {
        ui.column().w(Length::Px(260.0)).pad(12.0).gap(2.0).open();
        ui.tree(&mut app.open[0], "Pictures");
        if app.open[0] {
            ui.tree(&mut app.open[1], "Holidays");
            if app.open[1] {
                leaf(app, ui, 0, "Beach.jpg");
                leaf(app, ui, 1, "Hills.jpg");
                ui.close();
            }
            ui.tree(&mut app.open[2], "Family");
            if app.open[2] {
                leaf(app, ui, 2, "Ada.png");
                ui.close();
            }
            leaf(app, ui, 3, "Cat.png");
            ui.close();
        }
        ui.close();
    }

    fn leaf(app: &mut Folders, ui: &mut Ui, number: usize, name: &str) {
        if ui.selectable(app.picked == number, name).clicked {
            app.picked = number;
        }
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut app = Folders { open: [true, true, false], picked: 1 };
        super::Shot::new("05-a-tree", [300.0, 210.0], move |ui| describe(&mut app, ui))
    }
}

// docs/05-larger-parts.md
mod menus {
    use taut::Ui;

    struct Editor {
        wrap: bool,
        said: String,
    }

    fn describe(app: &mut Editor, ui: &mut Ui) {
        ui.menu_bar();
        if ui.menu_button("File") {
            if ui.menu_item("New\tCtrl+N").clicked {
                app.said = "A new file".into();
            }
            if ui.submenu("Open again") {
                for name in ["notes.txt", "plan.md"] {
                    if ui.menu_item(name).clicked {
                        app.said = format!("Opened {name}");
                    }
                }
                ui.close();
            }
            ui.separator();
            ui.menu_check(&mut app.wrap, "Wrap long lines");
            ui.separator();
            if ui.menu_item("Quit\tCtrl+Q").clicked {
                ui.quit();
            }
            ui.close();
        }
        if ui.menu_button("Help") {
            ui.menu_item("About");
            ui.close();
        }
        ui.close();
        // The keys the menu shows are the app's to ask for.
        if ui.keys("Ctrl+N") {
            app.said = "A new file".into();
        }
        if ui.keys("Ctrl+Q") {
            ui.quit();
        }
        ui.column().pad(12.0).open();
        ui.label(&app.said);
        ui.close();
    }

    // The picture of it:
    use super::Step::{Click, Hover};
    pub(super) fn shot() -> super::Shot {
        let mut app = Editor { wrap: true, said: "Nothing yet".into() };
        super::Shot::new("05-menus", [420.0, 220.0], move |ui| describe(&mut app, ui)).steps(&[Click("File"), Hover("Open again")])
    }
}

// docs/05-larger-parts.md
mod a_dock {
    use taut::{Dock, Ui};

    /// How the panels lie the first time: the files at the left, a quarter of the width; the
    /// editor and the notes behind tabs, over the log.
    fn first() -> Dock {
        Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]).above(Dock::of(&["Log"]), 0.7), 0.25)
    }

    fn describe(dock: &mut Dock, ui: &mut Ui) {
        ui.dock(dock);
        for name in ["Files", "Editor", "Notes", "Log"] {
            if ui.docked(dock, name) {
                ui.paragraph(&format!("What the {name} panel shows"));
                ui.close();
            }
        }
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut dock = first();
        super::Shot::new("05-a-dock", [560.0, 300.0], move |ui| describe(&mut dock, ui))
    }
}

// docs/05-larger-parts.md
mod a_chart {
    use taut::{Chart, Series, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        let sold = [12.0, 18.0, 9.0, 22.0, 30.0, 26.0, 14.0];
        let back = [2.0, 3.0, 1.0, 4.0, 6.0, 3.0, 2.0];
        let rows = [Series::bars("Sold", &sold), Series::line("Brought back", &back).color(t.red)];
        let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        ui.column().pad(12.0).open();
        ui.chart(&Chart::of(&rows).names(&days), 420.0, 200.0);
        ui.close();
    }

    // The picture of it:
    use super::Step::Hover;
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("05-a-chart", [444.0, 224.0], describe).steps(&[Hover("Sold, Brought back")])
    }
}

// docs/05-larger-parts.md
mod a_panel {
    use taut::{Panel, Ui};

    struct App {
        tools: Panel,
        size: f32,
        grid: bool,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        ui.column().grow().pad(12.0).open();
        ui.paragraph("The page, under the panel. The panel is moved by its title, stretched by its edges and corners, folded by its arrow and closed by its cross.");
        ui.close();
        if ui.panel(&mut app.tools, "Tools") {
            ui.label("Brush size");
            ui.slider(&mut app.size, 1.0..=10.0);
            ui.checkbox(&mut app.grid, "Show the grid");
            ui.close();
        }
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut app = App { tools: Panel::new(150.0, 70.0, 240.0, 150.0), size: 4.0, grid: true };
        super::Shot::new("05-a-panel", [420.0, 240.0], move |ui| describe(&mut app, ui))
    }
}

// docs/05-larger-parts.md
mod a_palette {
    use taut::Ui;

    const COMMANDS: [&str; 6] = ["New file\tCtrl+N", "Open a file\tCtrl+O", "Save\tCtrl+S", "Save as\u{2026}", "Search the files\tCtrl+Shift+F", "Settings"];

    struct App {
        finding: bool,
        said: String,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        app.finding |= ui.keys("Ctrl+K");
        if let Some(command) = ui.palette(&mut app.finding, &COMMANDS) {
            app.said = format!("Picked: {}", COMMANDS[command].split('\t').next().unwrap_or(""));
        }
        ui.column().pad(12.0).open();
        ui.label(&app.said);
        ui.close();
    }

    // The picture of it:
    use super::Step::Type;
    pub(super) fn shot() -> super::Shot {
        let mut app = App { finding: true, said: "Ctrl+K finds a command".into() };
        super::Shot::new("05-a-palette", [460.0, 260.0], move |ui| describe(&mut app, ui)).steps(&[Type("sa")])
    }
}

// docs/05-larger-parts.md
mod a_notice {
    use taut::Ui;

    fn describe(ui: &mut Ui) {
        ui.row().pad(12.0).open();
        if ui.button("Save").clicked {
            ui.notice("Saved");
        }
        ui.close();
    }

    // The picture of it:
    use super::Step::Click;
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("05-a-notice", [320.0, 130.0], describe).steps(&[Click("Save")])
    }
}

// docs/06-text-and-pictures.md
mod kinds_of_text {
    use taut::{Font, Length, Ui};

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        ui.column().pad(12.0).gap(6.0).open();
        ui.label("A label: one line, in the theme's font and colour");
        ui.note("A note: smaller and quieter");
        ui.text("Bold, in the colour that stands out", t.font.bold(), t.accent);
        ui.text("Letters of one width, for code: let x = 1;", t.font.mono(), t.text);
        ui.text("Struck through: no longer so", t.font.struck(), t.text_dim);
        ui.text("Larger", Font::new(20.0), t.text);
        ui.paragraph("A paragraph breaks into as many lines as the width it is given needs, between its words.");
        ui.el().w(Length::Px(220.0)).clip().text("Text that is too long for its box ends in dots", t.font, t.text).show();
        ui.wrapping_row().open();
        ui.words("Words of ", t.font, t.text);
        ui.words("several kinds", t.font.bold(), t.text);
        ui.words(" in one paragraph, with a ", t.font, t.text);
        ui.link("link");
        ui.words(" among them.", t.font, t.text);
        ui.close();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("06-kinds-of-text", [420.0, 270.0], describe)
    }
}

// docs/06-text-and-pictures.md
mod emoji {
    use taut::Ui;

    fn describe(note: &mut String, ui: &mut Ui) {
        ui.column().pad(12.0).gap(8.0).open();
        ui.label("In colour, at the size of the letters: 🙂 👍🏽 👩‍🚀 ❤️ 🇳🇴");
        ui.button("🎉 Done");
        ui.text_box(note, "Typed into a box, too");
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut note = "Pizza 🍕 on Friday".to_string();
        super::Shot::new("06-emoji", [420.0, 124.0], move |ui| describe(&mut note, ui))
    }
}

// docs/06-text-and-pictures.md
mod pictures {
    use taut::{Drawing, Picture, Ui};

    /// A sun, as an SVG file says it: its rays take the colour they are shown in.
    const SUN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
      <circle cx="12" cy="12" r="5" fill="#f5b400"/>
      <path d="M12 1.5v3M12 19.5v3M1.5 12h3M19.5 12h3M4.6 4.6l2.1 2.1M17.3 17.3l2.1 2.1M4.6 19.4l2.1-2.1M17.3 6.7l2.1-2.1"
        stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
    </svg>"##;

    struct App {
        pixels: Option<Picture>,
        sun: Option<Drawing>,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        let t = ui.theme;
        // Handed over once, kept by the library from then on.
        if app.pixels.is_none() {
            // Sixteen by sixteen pixels: red grows to the right, green downwards.
            let pixels = (0..256u32).map(|i| 0xff00_0080 | (i % 16 * 17) << 16 | (i / 16 * 17) << 8).collect();
            app.pixels = Some(ui.add_picture(16, pixels));
            app.sun = ui.add_drawing(SUN);
        }
        ui.row().pad(12.0).gap(16.0).open();
        if let Some(pixels) = app.pixels {
            ui.picture(pixels);
            ui.el().picture(pixels).size(96.0, 96.0).show();
        }
        if let Some(sun) = app.sun {
            ui.el().drawing(sun, t.text).size(24.0, 24.0).show();
            ui.el().drawing(sun, t.accent).size(96.0, 96.0).show();
        }
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut app = App { pixels: None, sun: None };
        super::Shot::new("06-pictures", [300.0, 120.0], move |ui| describe(&mut app, ui))
    }
}

// docs/06-text-and-pictures.md
mod drawing {
    use taut::{Flow, Icon, Path, Ui};

    const STAR: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");

    fn describe(ui: &mut Ui) {
        let t = ui.theme;
        // A wave, as a line and as the area under it.
        let (wide, high) = (300.0, 90.0);
        let (mut line, mut area) = (Path::new(), Path::new());
        area.move_to(0.0, high);
        for step in 0..=60 {
            let (x, y) = (step as f32 * wide / 60.0, high / 2.0 - (step as f32 / 6.0).sin() * 28.0);
            area.line_to(x, y);
            match step {
                0 => line.move_to(x, y),
                _ => line.line_to(x, y),
            };
        }
        area.line_to(wide, high).close();
        ui.column().pad(12.0).gap(12.0).open();
        ui.el().flow(Flow::Layers).size(wide, high).fill(t.panel).radius(6.0).clip().open();
        ui.el().size(wide, high).shape(&area, t.accent.alpha(0.25)).show();
        ui.el().size(wide, high).outline(&line, 2.0, t.accent).show();
        ui.close();
        // Fills that go from colour to colour, and icons drawn from SVG path data.
        ui.row().gap(12.0).open();
        ui.el().size(90.0, 36.0).radius(8.0).gradient_across(t.accent, t.green).show();
        ui.el().size(90.0, 36.0).radius(8.0).gradient_along(60.0, &[(0.0, t.red), (0.5, t.amber), (1.0, t.green)]).show();
        ui.el().size(36.0, 36.0).radius(18.0).gradient_around(&[(0.0, t.text), (0.6, t.accent), (1.0, t.accent.alpha(0.0))]).show();
        ui.icon(STAR);
        ui.el().icon(STAR, 32.0, t.amber).show();
        ui.close();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        super::Shot::new("06-drawing", [324.0, 172.0], describe)
    }
}

// docs/06-text-and-pictures.md
mod a_scene {
    use taut::{Mesh, Ui, View};

    /// A body with four sides and a square floor: five points, and four triangles, each
    /// said by the numbers of its three corners.
    fn pyramid(ui: &mut Ui) -> Mesh {
        let points = vec![[-5.0, 0.0, -5.0], [5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [-5.0, 0.0, 5.0], [0.0, 6.0, 0.0]];
        ui.add_mesh(points, vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]])
    }

    fn describe(body: &mut Option<Mesh>, ui: &mut Ui) {
        let t = ui.theme;
        let pyramid = *body.get_or_insert_with(|| pyramid(ui));
        let view = View::look([13.0, 9.0, 17.0], [0.0, 2.0, 0.0]);
        ui.row().pad(12.0).open();
        ui.el().size(300.0, 160.0).fill(t.panel).radius(6.0).scene(&view, &[pyramid.filled(t.accent).outlined(t.text)]).show();
        ui.close();
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut body = None;
        super::Shot::new("06-a-scene", [324.0, 184.0], move |ui| describe(&mut body, ui))
    }
}

// docs/07-everyone-can-use-it.md
mod a_form {
    use taut::{Align, Ui};

    struct Form {
        name: String,
        letters: bool,
    }

    fn describe(form: &mut Form, ui: &mut Ui) {
        ui.column().pad(12.0).gap(10.0).open();
        ui.text_box(&mut form.name, "Your name");
        ui.checkbox(&mut form.letters, "Send me letters");
        ui.row().gap(8.0).align(Align::End, Align::Center).open();
        ui.button("Cancel");
        ui.primary_button("Save");
        ui.close();
        ui.close();
    }

    // The picture of it:
    use super::Step::{Press, Type};
    use taut::{Key, Mods, Theme};
    pub(super) fn shots() -> Vec<super::Shot> {
        let mut first = Form { name: String::new(), letters: false };
        let mut second = Form { name: String::new(), letters: true };
        let tab = Press(Key::Tab, Mods::NONE);
        vec![
            super::Shot::new("07-by-the-keyboard", [360.0, 130.0], move |ui| describe(&mut first, ui)).steps(&[tab, Type("Ada"), tab, Press(Key::Space, Mods::NONE), tab]),
            super::Shot::new("07-light-and-larger", [360.0, 130.0], move |ui| (ui.theme = Theme::light(), describe(&mut second, ui)).1).scale(1.5),
        ]
    }
}

// docs/09-inside-another-program.md
mod an_overlay {
    use std::time::{Duration, Instant};
    use taut::geom::{Size, pt};
    use taut::text::BuiltInFont;
    use taut::{Button, Color, Event, Length, LocalClipboard, Mods, Surface, Ui};

    /// What the overlay's own parts are set to.
    struct Settings {
        names: bool,
        range: f32,
    }

    fn describe(settings: &mut Settings, ui: &mut Ui) {
        let t = ui.theme;
        ui.column().w(Length::Px(220.0)).at(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.alpha(0.92)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
        ui.text("Overlay", t.font.bold(), t.text);
        ui.switch(&mut settings.names, "Show names");
        ui.slider(&mut settings.range, 0.0..=100.0);
        ui.note(&format!("Seen as far as {:.0} steps", settings.range));
        ui.close();
    }

    /// The interface over a game: what it keeps, and its pixels.
    struct Overlay {
        ui: Ui,
        pixels: Surface,
        settings: Settings,
        /// Something happened that the pixels do not show yet.
        waiting: bool,
        started: Instant,
    }

    impl Overlay {
        /// An overlay as large as the game's window, `width` by `height` pixels, on a screen
        /// that shows `scale` pixels to a unit.
        fn new(width: u32, height: u32, scale: f32) -> Overlay {
            let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            ui.theme.bg = Color::NONE;
            ui.resize(Size { w: width as f32 / scale, h: height as f32 / scale }, scale);
            let pixels = Surface::default(); // (drawn whole by the first `frame`)
            Overlay { ui, pixels, settings: Settings { names: true, range: 60.0 }, waiting: true, started: Instant::now() }
        }

        /// The game's window says the pointer moved here, in its pixels.
        fn pointer(&mut self, x: f32, y: f32) {
            let scale = self.ui.scale();
            self.waiting |= self.ui.event(Event::Move(pt(x / scale, y / scale)));
        }

        /// The game's window says the left button went down, or came up.
        fn button(&mut self, down: bool) {
            let event = if down { Event::Press(Button::Left, Mods::NONE) } else { Event::Release(Button::Left) };
            self.waiting |= self.ui.event(event);
        }

        /// Once a frame. The places of `self.pixels` that changed, in pixels (left, top, right,
        /// bottom), to copy into the texture; none, when nothing did, which is most frames.
        fn frame(&mut self) -> Vec<[usize; 4]> {
            self.ui.set_time(self.started.elapsed());
            let due = self.ui.wake_in() == Some(Duration::ZERO);
            if !std::mem::take(&mut self.waiting) && !due {
                return Vec::new();
            }
            let settings = &mut self.settings;
            self.ui.run(&mut |ui| describe(settings, ui));
            self.pixels.update(&mut self.ui)
        }

        /// Whether what the mouse does now is the overlay's, or the game's own.
        fn wants_pointer(&self) -> bool {
            self.ui.wants_pointer()
        }
    }

    // The picture of it:
    pub(super) fn shot() -> super::Shot {
        let mut settings = Settings { names: true, range: 60.0 };
        super::Shot::new("09-an-overlay", [480.0, 240.0], move |ui| (ui.theme.bg = Color::NONE, describe(&mut settings, ui)).1).over_a_game()
    }
}

// docs/09-inside-another-program.md
mod a_whole_screen {
    use taut::{Align, Length, Panel, Ui};

    struct Desk {
        notes: Panel,
        text: String,
    }

    fn describe(desk: &mut Desk, ui: &mut Ui) {
        let t = ui.theme;
        // The wall behind everything.
        ui.el().grow().gradient_along(150.0, &[(0.0, t.bg), (1.0, t.selected)]).show();
        // A window of a program, which the person moves, sizes and closes.
        if ui.panel(&mut desk.notes, "Notes") {
            ui.text_area(&mut desk.text, "Write something", (3, 8));
            ui.close();
        }
        // A bar along the bottom of the screen, over everything.
        ui.pin(Align::Start, Align::End);
        let wide = ui.size().w;
        ui.row().w(Length::Px(wide)).h(Length::Px(40.0)).pad((8.0, 0.0)).gap(8.0).fill(t.panel).border(t.line, 1.0).open();
        ui.toggle(&mut desk.notes.open, "Notes");
        ui.spacer();
        ui.note("12:00");
        ui.close();
        // Nothing else on this screen draws a pointer: the interface does, last of all.
        ui.show_pointer();
    }

    // The picture of it:
    use super::Step::Hover;
    pub(super) fn shot() -> super::Shot {
        let mut desk = Desk { notes: Panel::new(40.0, 30.0, 260.0, 150.0), text: "Bread, cheese, tea".into() };
        super::Shot::new("09-a-whole-screen", [480.0, 260.0], move |ui| describe(&mut desk, ui)).steps(&[Hover("Notes")])
    }
}
