//! Starts the demo and uses it as a person would. Each thing it presses is found by what the
//! window tells screen readers, each step is checked the same way (only what shows in a window
//! counts), and a picture is saved after each step. Prints the demo's memory and processor
//! time. Ends with 1 if a check failed, a picture is missing or blank, or the demo does not end
//! when closed.
//!
//!   cargo build -p taut --example gallery --release
//!   cargo run -p taut --example walk --release -- [<folder for pictures>] [<program>]
//!
//! Pictures go to `taut-shots` in the temporary folder unless a folder is given; the program is
//! the demo built beside the walk unless one is given. `TAUT_WALK_TELL=1` prints what the window
//! tells before each picture. Memory after use includes what was kept for the screen reader.
//!
//! - **Windows**: posts the messages a mouse and keyboard make, and asks UI Automation where
//!   things are. The window shows but is never made active: run it on a desktop nobody uses.
//!   Keys with Control are not sent (Windows reads Control from the keyboard itself). Pictures
//!   come from `PrintWindow`, which redraws the whole window, so they cannot show stale drawing.
//! - **Linux (X11)**: moves the pointer and types through XTest, asks AT-SPI where things are,
//!   and takes pictures of the screen. After each picture every window redraws in full, and any
//!   difference (something left stale) fails the walk. Run it on a display of its own:
//!   `xvfb-run -a -s "-screen 0 900x600x24" target/release/examples/walk`. Needs libXtst, xclip,
//!   libatspi, at-spi2-core, gdbus and dbus; it starts a session bus of its own when none is set.

#![cfg_attr(not(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android"))))), allow(dead_code))]
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What a thing is, as far as the walk tells kinds apart.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Any,
    Button,
    Entry,
    MenuItem,
    Menu,
    Tab,
    Heading,
    Cell,
    Slider,
    Spin,
    Text,
    Window,
    Dialog,
    Group,
    Other,
}

/// A place on the screen, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    /// The point at this fraction of the way across and down.
    fn at(&self, across: f64, down: f64) -> (f64, f64) {
        (self.x + self.w * across, self.y + self.h * down)
    }

    fn mid(&self) -> (f64, f64) {
        self.at(0.5, 0.5)
    }

    fn holds(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// One node of what a window tells screen readers.
#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    name: String,
    at: Rect,
    /// Ticked, switched on.
    on: bool,
}

/// A key the walk presses.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Enter,
    Escape,
    Up,
    Backspace,
    Letter(char),
}

/// What each system does for the walk: its input, pictures and accessibility tree.
trait Driver {
    /// The nodes of the program's windows that show, in tree order.
    fn nodes(&mut self) -> Vec<Node>;
    /// How many windows the program has open.
    fn windows(&mut self) -> usize;
    fn point(&mut self, at: (f64, f64));
    fn click(&mut self, at: (f64, f64));
    fn drag(&mut self, from: (f64, f64), to: (f64, f64));
    fn wheel(&mut self, at: (f64, f64), notches: u32);
    fn key(&mut self, key: Key, control: bool);
    fn typed(&mut self, text: &str);
    /// Saves a picture of the program's topmost window, or says why there is none (no
    /// window, or a picture of one colour only).
    fn shot(&mut self, name: &str) -> Result<(), String>;
    fn memory(&mut self, when: &str);
    /// Milliseconds of processor the program uses in this many seconds.
    fn processor(&mut self, seconds: u64) -> f64;
    /// What another program reads from the clipboard, where the walk can ask.
    fn clipboard(&mut self) -> Option<String>;
    /// Ends the program; gives back what is left to say, as a failure where pictures were
    /// wrong.
    fn end(&mut self) -> Result<String, String>;
}

/// Waits so many milliseconds: time for the demo to take in what was done.
fn rest(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

/// The demo built beside this program.
fn the_demo_beside() -> PathBuf {
    let this = std::env::current_exe().expect("this program does not know where it is");
    this.with_file_name(format!("gallery{}", std::env::consts::EXE_SUFFIX))
}

/// The walk as it goes: the system's driver, what failed, and how many checks held.
struct Walk<D: Driver> {
    driver: D,
    failed: Vec<String>,
    held: usize,
    tell: bool,
}

/// Whether a node lies in one of the windows told, so a picture of the window shows it.
fn in_view(nodes: &[Node], node: &Node) -> bool {
    nodes.iter().any(|window| window.kind == Kind::Window && window.at.holds(node.at.mid()))
}

impl<D: Driver> Walk<D> {
    /// The first thing that shows with this name (and kind, unless `Any`), waiting up to two
    /// seconds for it. A failure is noted with what the window told instead.
    fn find(&mut self, kind: Kind, name: &str) -> Option<Node> {
        self.find_nth(kind, name, 0)
    }

    /// The `nth` such thing (from 0; `usize::MAX`: the last), as `find`.
    fn find_nth(&mut self, kind: Kind, name: &str, nth: usize) -> Option<Node> {
        let since = Instant::now();
        loop {
            let nodes = self.driver.nodes();
            let mut fits = nodes.iter().filter(|thing| thing.name == name && (kind == Kind::Any || thing.kind == kind));
            let found = if nth == usize::MAX { fits.next_back() } else { fits.nth(nth) };
            if let Some(found) = found {
                return Some(found.clone());
            }
            if since.elapsed() > Duration::from_secs(2) {
                let names: Vec<String> = nodes.iter().filter(|thing| !thing.name.is_empty()).map(|thing| format!("{:?} {:?}", thing.kind, thing.name)).collect();
                self.failed.push(format!("not found: {kind:?} {name:?}; told: {}", names.join(", ")));
                return None;
            }
            rest(100);
        }
    }

    /// The node right after the first with this kind and name.
    fn after(&mut self, kind: Kind, name: &str) -> Option<Node> {
        let nodes = self.driver.nodes();
        let at = nodes.iter().position(|thing| thing.name == name && thing.kind == kind)?;
        nodes.get(at + 1).cloned()
    }

    /// Whether a thing with this name shows in a window, now.
    fn shows(&mut self, name: &str) -> bool {
        let nodes = self.driver.nodes();
        nodes.iter().any(|thing| thing.name == name && in_view(&nodes, thing))
    }

    /// Whether a thing whose name starts so shows in a window, now.
    fn shows_start(&mut self, start: &str) -> bool {
        let nodes = self.driver.nodes();
        nodes.iter().any(|thing| thing.name.starts_with(start) && in_view(&nodes, thing))
    }

    /// Clicks at a fraction of the way across and down a thing found by name.
    fn press_at(&mut self, kind: Kind, name: &str, across: f64, down: f64) -> Option<Node> {
        let found = self.find(kind, name)?;
        self.driver.click(found.at.at(across, down));
        if kind == Kind::Tab {
            // (A new page glides in.)
            rest(300);
        }
        Some(found)
    }

    fn press(&mut self, kind: Kind, name: &str) -> Option<Node> {
        self.press_at(kind, name, 0.5, 0.5)
    }

    fn point_to(&mut self, kind: Kind, name: &str) {
        if let Some(found) = self.find(kind, name) {
            self.driver.point(found.at.mid());
        }
    }

    /// Notes whether what a picture is named for holds.
    fn holds(&mut self, picture: &str, what: &str, holds: bool) {
        match holds {
            true => self.held += 1,
            false => self.failed.push(format!("{picture}: {what} does not hold")),
        }
    }

    /// Saves a picture, after printing what the window tells if asked to. A missing or blank
    /// picture fails the walk.
    fn shot(&mut self, name: &str) {
        if self.tell {
            println!("-- {name}");
            for thing in self.driver.nodes() {
                println!("   {:?} {:?} at {:.0},{:.0} {:.0}x{:.0}{}", thing.kind, thing.name, thing.at.x, thing.at.y, thing.at.w, thing.at.h, if thing.on { " on" } else { "" });
            }
        }
        if let Err(why) = self.driver.shot(name) {
            self.failed.push(format!("{name}: no picture: {why}"));
        }
    }

    /// The numbers of the cells under a heading that show in a window, top to bottom.
    fn column(&mut self, heading: &str) -> Vec<f64> {
        let nodes = self.driver.nodes();
        let Some(head) = nodes.iter().find(|thing| thing.kind == Kind::Heading && thing.name == heading) else { return Vec::new() };
        let under = |thing: &&Node| thing.kind == Kind::Cell && thing.at.y > head.at.y && (head.at.x..head.at.x + head.at.w).contains(&thing.at.mid().0) && in_view(&nodes, thing);
        nodes.iter().filter(under).filter_map(|cell| cell.name.split(' ').next()?.parse().ok()).collect()
    }
}

fn main() {
    let mut given = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = given.next().unwrap_or_else(|| std::env::temp_dir().join("taut-shots"));
    let program = given.next().unwrap_or_else(the_demo_beside);
    if !program.is_file() {
        eprintln!("{} is not there. Build the demo first: cargo build -p taut --example gallery --release", program.display());
        std::process::exit(1);
    }
    std::fs::create_dir_all(&folder).expect("the folder for the pictures could not be made");
    #[cfg(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
    let tell = std::env::var_os("TAUT_WALK_TELL").is_some();
    #[cfg(windows)]
    let driver = windows::start(&folder, &program);
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    let driver = x11::start(&folder, &program);
    #[cfg(not(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android"))))))]
    return println!("there is no walk for this system yet");
    #[cfg(any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
    {
        let mut walk = Walk { driver, failed: Vec::new(), held: 0, tell };
        steps(&mut walk);
        match walk.driver.end() {
            Ok(said) => println!("{said}"),
            Err(said) => walk.failed.push(said),
        }
        println!("pictures are in {}", folder.display());
        if !walk.failed.is_empty() {
            println!("FAILED:\n  {}", walk.failed.join("\n  "));
            std::process::exit(1);
        }
        println!("every step held ({} checks)", walk.held);
    }
}

/// The steps, each checked against what the window tells.
fn steps<D: Driver>(walk: &mut Walk<D>) {
    walk.driver.memory("at start");
    let list = walk.find(Kind::Entry, "Filter items").is_some() && walk.shows("item 0") && walk.shows("item 2");
    walk.holds("01-start", "the list page shows", list);
    walk.shot("01-start");
    println!("{:.0} ms of processor in 3 s with nothing happening", walk.driver.processor(3));

    // The filter box: the list keeps the items that hold what is typed.
    walk.press(Kind::Entry, "Filter items");
    walk.driver.typed("item 1");
    let filtered = walk.shows("item 12") && !walk.shows("item 2");
    walk.holds("02-typed", "the list is filtered", filtered);
    walk.shot("02-typed");
    rest(530);
    walk.shot("02-typed-b");
    println!("{:.0} ms of processor in 3 s with the cursor blinking", walk.driver.processor(3));
    // Selected by dragging across the box, then copied.
    if let Some(filter) = walk.find(Kind::Entry, "Filter items") {
        walk.driver.drag(filter.at.at(0.9, 0.5), (filter.at.x + 2.0, filter.at.mid().1));
    }
    walk.driver.key(Key::Letter('c'), true);
    walk.shot("03-selected");
    match walk.driver.clipboard() {
        Some(read) => walk.holds("03-selected", &format!("another program reads what was copied ({read:?})"), read == "item 1"),
        None => println!("03-selected: not checked here (keys with Control are not sent)"),
    }

    // The menu of the second "More" (the first is the tool bar's), and its dialog.
    if let Some(more) = walk.find_nth(Kind::Button, "More", 1) {
        walk.driver.click(more.at.mid());
    }
    walk.point_to(Kind::MenuItem, "Rename this window…");
    let menu = walk.shows("Rename this window…");
    walk.holds("04-menu", "the menu is open", menu);
    walk.shot("04-menu");
    walk.press(Kind::MenuItem, "Rename this window…");
    walk.driver.typed("Taut");
    let dialog = walk.find(Kind::Dialog, "Rename this window").is_some();
    walk.holds("05-dialog", "the dialog is open", dialog);
    walk.shot("05-dialog");
    walk.driver.key(Key::Enter, false);

    // The menu bar: with one menu open, pointing at another opens that one.
    walk.press(Kind::Any, "File");
    walk.point_to(Kind::Any, "View");
    let view = walk.shows("The Tools panel") && !walk.shows("Go to page") && walk.find(Kind::Window, "Taut").is_some();
    walk.holds("06-view-menu", "the window is renamed and the View menu is open", view);
    walk.shot("06-view-menu");
    walk.point_to(Kind::Any, "File");
    // (Clicked, not pointed at: Windows tells a window the pointer left at once when the real
    // pointer is elsewhere, so a posted hover never opens it.)
    walk.press(Kind::Any, "Go to page");
    rest(400);
    let inner = walk.find(Kind::MenuItem, "Writing").is_some();
    walk.holds("07-menu-in-menu", "the menu of pages is open", inner);
    walk.shot("07-menu-in-menu");
    walk.press(Kind::MenuItem, "Table");
    let table = walk.find(Kind::Heading, "Size").is_some();
    walk.holds("08-table", "the table shows", table);
    walk.shot("08-table");

    // The table: sorted by "Size" one way and the other; "Name" made wider by the grip after
    // its heading; a row selected, and the rows scrolled.
    walk.press(Kind::Heading, "Size");
    let up = walk.column("Size");
    walk.press(Kind::Heading, "Size");
    let down = walk.column("Size");
    let sorted = up.len() > 3 && up.is_sorted() && down.len() > 3 && down.iter().rev().is_sorted();
    walk.holds("09-table", &format!("the sizes are sorted both ways ({up:?}, {down:?})"), sorted);
    if let (Some(name), Some(size)) = (walk.find(Kind::Heading, "Name"), walk.find(Kind::Heading, "Size")) {
        let grip = ((name.at.x + name.at.w + size.at.x) / 2.0, name.at.mid().1);
        walk.driver.drag(grip, (grip.0 + 100.0, grip.1));
        let wider = walk.find(Kind::Heading, "Name").is_some_and(|after| after.at.w > name.at.w + 50.0);
        walk.holds("09-table", "the column \"Name\" is wider", wider);
    }
    let first_file = |nodes: Vec<Node>| nodes.into_iter().find(|thing| thing.kind == Kind::Cell && thing.name.starts_with("file "));
    let top = first_file(walk.driver.nodes());
    if let Some(cell) = &top {
        walk.driver.click(cell.at.mid());
        walk.driver.wheel(cell.at.mid(), 3);
    }
    let now = first_file(walk.driver.nodes());
    let picked = walk.shows_start("5000 files, 1 picked") && top.is_some() && now.map(|cell| cell.name) != top.map(|cell| cell.name);
    walk.holds("09-table", "a row is picked and the rows are scrolled", picked);
    walk.shot("09-table");

    // The parts: a switch flipped (reported as wide as its row, it is clicked at its start), a slider moved.
    walk.press(Kind::Tab, "Parts");
    let switch = "A switch slides over when it is flipped";
    let was = walk.find(Kind::Any, switch);
    if let Some(found) = &was {
        walk.driver.click((found.at.x + 16.0, found.at.mid().1));
    }
    let before = walk.after(Kind::Slider, "A slider").map(|nodes| nodes.name);
    if let Some(slider) = walk.find(Kind::Slider, "A slider") {
        walk.driver.click(slider.at.at(0.85, 0.5));
    }
    rest(300);
    let flipped = was.is_some_and(|was| walk.find(Kind::Any, switch).is_some_and(|now| now.on != was.on));
    let after = walk.after(Kind::Slider, "A slider").map(|nodes| nodes.name);
    walk.holds("10-parts", &format!("the switch flipped and the slider moved ({before:?} to {after:?})"), flipped && before.is_some() && after != before);
    walk.shot("10-parts");
    walk.press(Kind::Tab, "Drawing");
    let drawing = walk.shows_start("A chart drawn by the app");
    walk.holds("11-drawing", "the page of drawings shows", drawing);
    walk.shot("11-drawing");

    // More parts: a number stepped up, a colour and a day selected, a thing dragged.
    walk.press(Kind::Tab, "More");
    walk.press(Kind::Spin, "A number to type");
    for _ in 0..3 {
        walk.driver.key(Key::Up, false);
    }
    let colour = walk.after(Kind::Text, "A colour to pick");
    if let Some(swatch) = &colour {
        walk.driver.click(swatch.at.mid());
    }
    if let (Some(square), Some(hues)) = (walk.find(Kind::Slider, "S"), walk.find(Kind::Slider, "H")) {
        walk.driver.click(square.at.at(0.7, 0.3));
        walk.driver.click(hues.at.at(0.5, 0.6));
    }
    let picked = walk.after(Kind::Text, "A colour to pick").map(|swatch| swatch.name);
    let changed = colour.is_some_and(|colour| picked.is_some_and(|picked| picked != colour.name && picked.starts_with('#')));
    let picker = walk.shows("#rrggbb");
    walk.holds("12-colour", "a colour is picked", picker && changed);
    walk.shot("12-colour");
    walk.driver.key(Key::Escape, false);
    if let Some(day) = walk.after(Kind::Text, "A day to pick") {
        walk.driver.click(day.at.mid());
    }
    let calendar = walk.shows("The month after");
    walk.holds("12b-calendar", "the calendar is open", calendar);
    walk.shot("12b-calendar");
    walk.driver.key(Key::Escape, false);
    if let (Some(apples), Some(bread)) = (walk.find(Kind::Any, "Apples"), walk.find(Kind::Any, "Bread")) {
        walk.driver.drag(apples.at.mid(), (bread.at.mid().0, bread.at.y + bread.at.h * 1.5));
        let dragged = walk.find(Kind::Any, "Apples").is_some_and(|now| (now.at.x - bread.at.x).abs() < 2.0);
        walk.holds("13-dragged", "the apples are in the second box", dragged);
    }
    walk.shot("13-dragged");

    // The Tools panel: moved by its title, stretched by its corner, then closed.
    walk.press(Kind::Any, "View");
    walk.press(Kind::Any, "The Tools panel");
    if let Some(panel) = walk.find(Kind::Group, "Tools") {
        let title = (panel.at.x + panel.at.w / 2.0, panel.at.y + 14.0);
        walk.driver.drag(title, (title.0 - 220.0, title.1 + 76.0));
        // (The grip of a corner lies just outside it.)
        let corner = (panel.at.x + panel.at.w - 220.0 + 2.0, panel.at.y + panel.at.h + 76.0 + 2.0);
        walk.driver.drag(corner, (corner.0 + 58.0, corner.1 + 52.0));
        let moved = walk.find(Kind::Group, "Tools").is_some_and(|now| (now.at.x - (panel.at.x - 220.0)).abs() < 2.0 && now.at.w > panel.at.w + 40.0);
        walk.holds("14-panel", "the panel is moved and stretched", moved);
    }
    walk.shot("14-panel");
    let nodes = walk.driver.nodes();
    let panel = nodes.iter().find(|thing| thing.kind == Kind::Group && thing.name == "Tools").map(|panel| panel.at);
    if let Some(close) = nodes.iter().find(|thing| thing.name == "Close" && panel.is_some_and(|panel| panel.holds(thing.at.mid()))) {
        walk.driver.click(close.at.mid());
    }

    // The dock: a tab dragged onto the middle mark of another place.
    walk.press(Kind::Tab, "Dock");
    let dock = walk.find(Kind::Slider, "Files, Editor");
    let files = walk.shows("Folder 000");
    walk.holds("14a-dock", "the dock shows", dock.is_some() && files);
    walk.shot("14a-dock");
    // A tab dragged by itself onto the middle of the other place.
    if let (Some(tab), Some(folder), Some(files)) = (walk.find(Kind::Tab, "Preview"), walk.find(Kind::Any, "Folder 000"), dock.clone()) {
        walk.driver.drag(tab.at.mid(), (folder.at.mid().0, files.at.mid().1));
    }
    let moved = dock.is_some_and(|dock| walk.find(Kind::Text, "Preview").is_some_and(|preview| preview.at.x < dock.at.x)) && !walk.shows("Folder 000");
    walk.holds("14b-dock-moved", "the tab is in front in the other place", moved);
    walk.shot("14b-dock-moved");

    // The dialog that selects a file.
    walk.press(Kind::Any, "File");
    walk.press(Kind::Any, "Open a file…");
    rest(400);
    let files = walk.find(Kind::Dialog, "Open a file").is_some();
    walk.holds("15-files", "the file dialog is open", files);
    walk.shot("15-files");
    walk.driver.key(Key::Escape, false);

    // Back to the list: the filter emptied, what was copied pasted into the notes, the page scrolled.
    walk.press(Kind::Tab, "List");
    walk.press_at(Kind::Entry, "Filter items", 0.95, 0.5);
    for _ in 0.."item 1".len() {
        walk.driver.key(Key::Backspace, false);
    }
    walk.press(Kind::Entry, "Notes");
    walk.driver.key(Key::Letter('v'), true);
    walk.driver.typed(" pasted");
    if let Some(list) = walk.find(Kind::Text, "item 0") {
        walk.driver.wheel(list.at.mid(), 3);
    }
    let scrolled = walk.shows("item 9") && !walk.shows("Notes");
    walk.holds("16-scrolled", "the page is scrolled", scrolled);
    walk.shot("16-scrolled");

    // A second window: its switch and slider used, then closed by its button.
    walk.press(Kind::Any, "View");
    walk.press(Kind::Any, "A second window");
    rest(400);
    let two = walk.driver.windows() == 2;
    walk.holds("17-second-window", "a second window is open", two);
    walk.shot("17-second-window");
    if let Some(light) = walk.find_nth(Kind::Any, "Light colours", usize::MAX) {
        walk.driver.click((light.at.x + 16.0, light.at.mid().1));
    }
    if let Some(slider) = walk.find_nth(Kind::Slider, "", usize::MAX) {
        walk.driver.drag(slider.at.at(0.2, 0.5), slider.at.at(0.6, 0.5));
    }
    let light = walk.find_nth(Kind::Any, "Light colours", usize::MAX).is_some_and(|nodes| nodes.on);
    walk.holds("17b-second-used", "light colours are on", light);
    walk.shot("17b-second-used");
    walk.press(Kind::Any, "Close this window");
    rest(300);
    let one = walk.driver.windows() == 1;
    walk.holds("18-second-closed", "the second window is gone", one);
    walk.shot("18-second-closed");
    walk.driver.memory("after use");
}

/// Windows: posted messages, UI Automation, `PrintWindow`.
#[cfg(windows)]
mod windows {
    use super::{Driver, Key, Kind, Node, Rect, rest};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};

    type Handle = isize;

    #[repr(C)]
    struct StartupInfo {
        size: u32,
        names: [*mut u16; 3],
        place: [i32; 2],
        console: [u32; 5],
        flags: u32,
        show: u16,
        more_size: u16,
        more: *mut u8,
        handles: [Handle; 3],
    }
    #[repr(C)]
    struct Started {
        process: Handle,
        thread: Handle,
        process_id: u32,
        thread_id: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    /// `PROCESS_MEMORY_COUNTERS_EX`.
    #[repr(C)]
    struct Memory {
        size: u32,
        page_faults: u32,
        most_in_use: usize,
        in_use: usize,
        pools: [usize; 4],
        in_page_file: usize,
        most_in_page_file: usize,
        private: usize,
    }
    /// `PROCESSENTRY32W`.
    #[repr(C)]
    struct Running {
        size: u32,
        usage: u32,
        process_id: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        file: [u16; 260],
    }
    /// `BITMAPINFOHEADER`.
    #[repr(C)]
    struct BitmapInfo {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bits: u16,
        rest: [u32; 7],
    }
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(
            program: *const u16,
            command: *mut u16,
            process_rights: *const c_void,
            thread_rights: *const c_void,
            inherit: i32,
            flags: u32,
            environment: *const c_void,
            folder: *const u16,
            how: *const StartupInfo,
            started: *mut Started,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetProcessTimes(process: Handle, started: *mut u64, ended: *mut u64, in_system: *mut u64, in_program: *mut u64) -> i32;
        fn K32GetProcessMemoryInfo(process: Handle, memory: *mut Memory, size: u32) -> i32;
        fn CreateToolhelp32Snapshot(of: u32, process: u32) -> Handle;
        fn Process32FirstW(list: Handle, running: *mut Running) -> i32;
        fn Process32NextW(list: Handle, running: *mut Running) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn TerminateProcess(process: Handle, code: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(each: unsafe extern "system" fn(Handle, isize) -> i32, with: isize) -> i32;
        fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
        fn IsWindowVisible(window: Handle) -> i32;
        fn GetWindow(window: Handle, which: u32) -> Handle;
        fn GetWindowRect(window: Handle, rect: *mut WinRect) -> i32;
        fn GetClientRect(window: Handle, rect: *mut WinRect) -> i32;
        fn PrintWindow(window: Handle, dc: Handle, how: u32) -> i32;
        fn PostMessageW(window: Handle, message: u32, w: usize, l: isize) -> i32;
        fn GetDpiForWindow(window: Handle) -> u32;
        fn ScreenToClient(window: Handle, point: *mut Point) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(dc: Handle) -> Handle;
        fn CreateDIBSection(dc: Handle, info: *const BitmapInfo, usage: u32, pixels: *mut *mut u32, section: Handle, offset: u32) -> Handle;
        fn SelectObject(dc: Handle, object: Handle) -> Handle;
        fn DeleteObject(object: Handle) -> i32;
        fn DeleteDC(dc: Handle) -> i32;
        fn GdiFlush() -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *const c_void, how: u32) -> i32;
        fn CoCreateInstance(class: *const Guid, outer: *mut c_void, context: u32, interface: *const Guid, made: *mut *mut c_void) -> i32;
    }
    #[link(name = "oleaut32")]
    unsafe extern "system" {
        fn SysStringLen(text: *const u16) -> u32;
        fn SysFreeString(text: *const u16);
    }

    const WM_SETFOCUS: u32 = 0x0007;
    const WM_CLOSE: u32 = 0x0010;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_CHAR: u32 = 0x0102;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_MOUSEWHEEL: u32 = 0x020A;

    /// `CUIAutomation` and `IUIAutomation` (UIAutomationClient.h).
    const CUIAUTOMATION: Guid = Guid(0xff48dba4, 0x60ef, 0x4201, [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e]);
    const IUIAUTOMATION: Guid = Guid(0x30cbe57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);
    /// Slots of the interfaces' tables (UIAutomationClient.h).
    const RELEASE: usize = 2;
    const ELEMENT_FROM_HANDLE: usize = 6;
    const CREATE_TRUE_CONDITION: usize = 21;
    const FIND_ALL: usize = 6;
    const GET_PROPERTY: usize = 10;
    const LENGTH: usize = 3;
    const GET_ELEMENT: usize = 4;
    const CONTROL_TYPE: usize = 21;
    const NAME: usize = 23;
    const IS_OFFSCREEN: usize = 38;
    const BOUNDING_RECTANGLE: usize = 43;
    /// `TreeScope_Subtree`.
    const SUBTREE: i32 = 7;
    /// `UIA_ToggleToggleStatePropertyId`.
    const TOGGLE_STATE: i32 = 30086;

    /// The function in `slot` of a COM object's table.
    ///
    /// # Safety
    /// `object` is a live COM object whose table has a function of type `F` in that slot.
    unsafe fn slot<F: Copy>(object: *mut c_void, slot: usize) -> F {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        // SAFETY: a COM object starts with a pointer to its table of functions.
        unsafe { std::mem::transmute_copy(&*(*(object as *const *const *const c_void)).add(slot)) }
    }

    /// Lets go of a COM object.
    fn release(object: *mut c_void) {
        if !object.is_null() {
            // SAFETY: slot 2 of every COM object is `Release`.
            unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(object, RELEASE)(object) };
        }
    }

    /// The kind of a UI Automation control type.
    fn kind(control: i32) -> Kind {
        match control {
            50000 => Kind::Button,
            50004 => Kind::Entry,
            50011 => Kind::MenuItem,
            50009 => Kind::Menu,
            50019 => Kind::Tab,
            50035 => Kind::Heading,
            50029 => Kind::Cell,
            50015 => Kind::Slider,
            50016 => Kind::Spin,
            50020 => Kind::Text,
            50032 => Kind::Window,
            50033 => Kind::Dialog,
            50026 => Kind::Group,
            _ => Kind::Other,
        }
    }

    /// A number with its thousands set apart.
    fn grouped(number: f64, decimals: usize) -> String {
        let plain = format!("{number:.decimals$}");
        let whole = plain.find('.').unwrap_or(plain.len());
        let mut grouped = String::new();
        for (at, digit) in plain[..whole].chars().enumerate() {
            grouped.extend((at > 0 && (whole - at).is_multiple_of(3)).then_some(','));
            grouped.push(digit);
        }
        grouped + &plain[whole..]
    }

    /// Starts a program shown but not activated.
    fn launch(program: &Path) -> (Handle, u32) {
        let name: Vec<u16> = program.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: all zeros is a valid `STARTUPINFOW`.
        let mut how: StartupInfo = unsafe { std::mem::zeroed() };
        // STARTF_USESHOWWINDOW, SW_SHOWNOACTIVATE.
        (how.size, how.flags, how.show) = (size_of::<StartupInfo>() as u32, 0x1, 4);
        let mut started = Started { process: 0, thread: 0, process_id: 0, thread_id: 0 };
        // SAFETY: the name ends in a zero; `how` and `started` are laid out as Windows has them.
        let fine = unsafe { CreateProcessW(name.as_ptr(), null_mut(), null(), null(), 0, 0, null(), null(), &how, &mut started) };
        assert!(fine != 0, "the program could not be started: {}", std::io::Error::last_os_error());
        // SAFETY: the thread's handle is not used.
        unsafe { CloseHandle(started.thread) };
        (started.process, started.process_id)
    }

    /// Notes a window of the program asked about, if it shows. Called by `EnumWindows`.
    unsafe extern "system" fn note_window(window: Handle, search: isize) -> i32 {
        // SAFETY: `search` is what `windows_of` handed to `EnumWindows`.
        let (process, found) = unsafe { &mut *(search as *mut (u32, Vec<Handle>)) };
        let mut of = 0;
        // SAFETY: any window handle may be asked about.
        let shows = unsafe { GetWindowThreadProcessId(window, &mut of) != 0 && IsWindowVisible(window) != 0 };
        found.extend((shows && of == *process).then_some(window));
        1
    }

    /// The program's windows that show and belong to no other window, topmost first.
    fn windows_of(process: u32) -> Vec<Handle> {
        let mut search = (process, Vec::new());
        // SAFETY: `note_window` gets this pointer only while `EnumWindows` runs.
        unsafe { EnumWindows(note_window, &raw mut search as isize) };
        // SAFETY: any window handle may be asked for its owner (4).
        search.1.into_iter().filter(|window| unsafe { GetWindow(*window, 4) } == 0).collect()
    }

    /// A picture of a window as it draws all of itself, and its width (nothing for none).
    fn picture_of(window: Handle) -> (Vec<u32>, usize) {
        let mut frame = WinRect::default();
        // SAFETY: `frame` is room for the window's place.
        unsafe { GetWindowRect(window, &mut frame) };
        let (wide, high) = (frame.right - frame.left, frame.bottom - frame.top);
        if wide <= 0 || high <= 0 {
            return (Vec::new(), 0);
        }
        let info = BitmapInfo { size: 40, width: wide, height: -high, planes: 1, bits: 32, rest: [0; 7] };
        let mut pixels = null_mut();
        // SAFETY: a top-down block of 32-bit pixels is made, the window draws all of itself
        // into it (2: as for the screen), and it is copied and given back.
        let picture = unsafe {
            let dc = CreateCompatibleDC(0);
            let block = CreateDIBSection(dc, &info, 0, &mut pixels, 0, 0);
            let before = SelectObject(dc, block);
            PrintWindow(window, dc, 2);
            GdiFlush();
            let picture = if pixels.is_null() { Vec::new() } else { std::slice::from_raw_parts(pixels, (wide * high) as usize).to_vec() };
            SelectObject(dc, before);
            DeleteObject(block);
            DeleteDC(dc);
            picture
        };
        (picture, wide as usize)
    }

    /// The walk's side on Windows: the demo, UI Automation, and where pictures go.
    pub struct Demo {
        process: Handle,
        id: u32,
        automation: *mut c_void,
        folder: PathBuf,
        shots: Vec<String>,
    }

    pub fn start(folder: &Path, program: &Path) -> Demo {
        let program = std::path::absolute(program).expect("the program to walk has no place");
        let (process, id) = launch(&program);
        for _ in 0..100 {
            if !windows_of(id).is_empty() {
                break;
            }
            rest(100);
        }
        rest(2000);
        let mut automation = null_mut();
        // SAFETY: COM is started for this thread, and UI Automation's object made in it.
        unsafe {
            CoInitializeEx(null(), 0);
            CoCreateInstance(&CUIAUTOMATION, null_mut(), 1, &IUIAUTOMATION, &mut automation);
        }
        assert!(!automation.is_null(), "UI Automation could not be started");
        let demo = Demo { process, id, automation, folder: folder.to_path_buf(), shots: Vec::new() };
        let size = std::fs::metadata(&program).map_or(0, |file| file.len());
        let window = windows_of(id).first().copied().unwrap_or(0);
        let mut inside = WinRect::default();
        // SAFETY: any window handle may be asked; `inside` is room for its size. Text boxes
        // show their cursor only in a window that has the keyboard: it is told it has.
        let scale = unsafe {
            GetClientRect(window, &mut inside);
            PostMessageW(window, WM_SETFOCUS, 0, 0);
            GetDpiForWindow(window) as f64 / 96.0
        };
        println!("program: {} KB, window {}x{} at scale {scale}", grouped(size as f64 / 1024.0, 0), inside.right, inside.bottom);
        demo
    }

    impl Demo {
        /// The topmost window of the demo under a point of the screen, and the point in it.
        fn under(&self, (x, y): (f64, f64)) -> Option<(Handle, isize)> {
            for window in windows_of(self.id) {
                let mut frame = WinRect::default();
                // SAFETY: `frame` is room for the window's place.
                unsafe { GetWindowRect(window, &mut frame) };
                let (x, y) = (x.round() as i32, y.round() as i32);
                if x < frame.left || y < frame.top || x >= frame.right || y >= frame.bottom {
                    continue;
                }
                let mut point = Point { x, y };
                // SAFETY: `point` is a place on the screen, made one in the window.
                unsafe { ScreenToClient(window, &mut point) };
                return Some((window, ((point.y << 16) | (point.x & 0xFFFF)) as isize));
            }
            None
        }

        fn post_at(&self, at: (f64, f64), message: u32, w: usize) {
            if let Some((window, place)) = self.under(at) {
                // SAFETY: a message can be posted to any handle; these carry numbers only.
                unsafe { PostMessageW(window, message, w, place) };
            }
        }

        /// Posts to the demo's topmost window.
        fn post(&self, message: u32, w: usize, l: isize) {
            if let Some(window) = windows_of(self.id).first() {
                // SAFETY: as in `post_at`.
                unsafe { PostMessageW(*window, message, w, l) };
            }
        }

        /// What one window tells: every thing under it that is on the screen.
        fn told_by(&self, window: Handle, nodes: &mut Vec<Node>) {
            let (mut element, mut condition, mut found) = (null_mut(), null_mut(), null_mut());
            type Get = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32;
            type Make = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
            type FindAll = unsafe extern "system" fn(*mut c_void, i32, *mut c_void, *mut *mut c_void) -> i32;
            // SAFETY: each call is in the slot UIAutomationClient.h gives it, on an object
            // UI Automation made; what comes back is let go of below.
            unsafe {
                slot::<Get>(self.automation, ELEMENT_FROM_HANDLE)(self.automation, window as *mut c_void, &mut element);
                slot::<Make>(self.automation, CREATE_TRUE_CONDITION)(self.automation, &mut condition);
                if !element.is_null() && !condition.is_null() {
                    slot::<FindAll>(element, FIND_ALL)(element, SUBTREE, condition, &mut found);
                }
            }
            if !found.is_null() {
                self.read_all(found, nodes);
            }
            [found, condition, element].into_iter().for_each(release);
        }

        /// Reads each element of an array UI Automation found.
        fn read_all(&self, found: *mut c_void, nodes: &mut Vec<Node>) {
            let mut count = 0;
            type Length = unsafe extern "system" fn(*mut c_void, *mut i32) -> i32;
            type Element = unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> i32;
            // SAFETY: as in `told_by`.
            unsafe { slot::<Length>(found, LENGTH)(found, &mut count) };
            for at in 0..count {
                let mut element = null_mut();
                // SAFETY: as in `told_by`; the element is let go of after it is read.
                unsafe { slot::<Element>(found, GET_ELEMENT)(found, at, &mut element) };
                if !element.is_null() {
                    nodes.extend(read(element));
                    release(element);
                }
            }
        }
    }

    /// An element's kind, name and place, if it is on the screen.
    fn read(element: *mut c_void) -> Option<Node> {
        let (mut control, mut offscreen, mut name, mut place, mut toggle) = (0, 0, null_mut::<u16>(), [0i32; 4], [0u64; 3]);
        type Number = unsafe extern "system" fn(*mut c_void, *mut i32) -> i32;
        type Text = unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32;
        type Place = unsafe extern "system" fn(*mut c_void, *mut [i32; 4]) -> i32;
        type Property = unsafe extern "system" fn(*mut c_void, i32, *mut [u64; 3]) -> i32;
        // SAFETY: as in `told_by`. The name is a BSTR, freed after it is read. The toggle
        // state is a VARIANT (24 bytes), a VT_I4 or VT_EMPTY that needs no clearing.
        let name = unsafe {
            slot::<Number>(element, CONTROL_TYPE)(element, &mut control);
            slot::<Number>(element, IS_OFFSCREEN)(element, &mut offscreen);
            slot::<Place>(element, BOUNDING_RECTANGLE)(element, &mut place);
            slot::<Property>(element, GET_PROPERTY)(element, TOGGLE_STATE, &mut toggle);
            slot::<Text>(element, NAME)(element, &mut name);
            let read = if name.is_null() { String::new() } else { String::from_utf16_lossy(std::slice::from_raw_parts(name, SysStringLen(name) as usize)) };
            SysFreeString(name);
            read
        };
        // A RECT: left, top, right, bottom.
        let at = Rect { x: place[0] as f64, y: place[1] as f64, w: (place[2] - place[0]) as f64, h: (place[3] - place[1]) as f64 };
        // VT_I4 (3), ToggleState_On (1).
        let on = toggle[0] as u16 == 3 && toggle[1] as i32 == 1;
        (offscreen == 0 && at.w > 0.0 && at.h > 0.0).then_some(Node { kind: kind(control), name, at, on })
    }

    impl Driver for Demo {
        fn nodes(&mut self) -> Vec<Node> {
            let mut nodes = Vec::new();
            for window in windows_of(self.id) {
                self.told_by(window, &mut nodes);
            }
            nodes
        }

        fn windows(&mut self) -> usize {
            windows_of(self.id).len()
        }

        fn point(&mut self, at: (f64, f64)) {
            self.post_at(at, WM_MOUSEMOVE, 0);
            rest(80);
        }

        fn click(&mut self, at: (f64, f64)) {
            self.point(at);
            self.post_at(at, WM_LBUTTONDOWN, 1);
            rest(60);
            self.post_at(at, WM_LBUTTONUP, 0);
            rest(150);
        }

        fn drag(&mut self, from: (f64, f64), to: (f64, f64)) {
            self.point(from);
            self.post_at(from, WM_LBUTTONDOWN, 1);
            rest(60);
            for step in 1..=5 {
                let step = step as f64 / 5.0;
                self.post_at((from.0 + (to.0 - from.0) * step, from.1 + (to.1 - from.1) * step), WM_MOUSEMOVE, 1);
                rest(40);
            }
            self.post_at(to, WM_LBUTTONUP, 0);
            rest(150);
        }

        fn wheel(&mut self, at: (f64, f64), notches: u32) {
            let Some((window, _)) = self.under(at) else { return };
            // The wheel's place is on the screen. One notch towards the person is -120.
            let place = (((at.1 as i32) << 16) | (at.0 as i32 & 0xFFFF)) as isize;
            for _ in 0..notches {
                // SAFETY: as in `post_at`.
                unsafe { PostMessageW(window, WM_MOUSEWHEEL, (-120i16 as u16 as usize) << 16, place) };
                rest(60);
            }
            rest(300);
        }

        fn key(&mut self, key: Key, control: bool) {
            // Windows reads Control from the keyboard itself: a posted one would not count.
            if control {
                return;
            }
            let code = match key {
                Key::Enter => 0x0D,
                Key::Escape => 0x1B,
                Key::Up => 0x26,
                Key::Backspace => 0x08,
                Key::Letter(letter) => letter.to_ascii_uppercase() as usize,
            };
            self.post(WM_KEYDOWN, code, 0);
            self.post(WM_KEYUP, code, 0);
            rest(150);
        }

        fn typed(&mut self, text: &str) {
            for letter in text.encode_utf16() {
                self.post(WM_CHAR, letter as usize, 0);
                rest(15);
            }
            rest(150);
        }

        /// The first `PrintWindow` of a window just shown can come back blank: a blank picture
        /// is taken again once.
        fn shot(&mut self, name: &str) -> Result<(), String> {
            let Some(window) = windows_of(self.id).first().copied() else { return Err("no window".into()) };
            let (mut picture, mut wide) = picture_of(window);
            if picture.iter().all(|pixel| Some(pixel) == picture.first()) {
                rest(200);
                (picture, wide) = picture_of(window);
            }
            if picture.is_empty() {
                return Err("the window gave none".into());
            }
            self.shots.push(name.to_string());
            std::fs::write(self.folder.join(format!("{name}.png")), taut::png::encode(wide, &picture).unwrap()).expect("the picture could not be saved");
            match picture.iter().all(|pixel| *pixel == picture[0]) {
                true => Err("all one colour".into()),
                false => Ok(()),
            }
        }

        fn memory(&mut self, when: &str) {
            // SAFETY: zeros are a valid start; `size` says how big it is.
            let mut memory = Memory { size: size_of::<Memory>() as u32, ..unsafe { std::mem::zeroed() } };
            // SAFETY: `memory` is room for what Windows says.
            unsafe { K32GetProcessMemoryInfo(self.process, &mut memory, memory.size) };
            let megabytes = |bytes: usize| grouped(bytes as f64 / 1_048_576.0, 1);
            println!("{when}: {} MB private, {} MB in use, {} threads", megabytes(memory.private), megabytes(memory.in_use), threads(self.id));
        }

        fn processor(&mut self, seconds: u64) -> f64 {
            let used = |process| {
                let (mut started, mut ended, mut in_system, mut in_program) = (0, 0, 0, 0);
                // SAFETY: each is room for a time in units of 100 ns.
                unsafe { GetProcessTimes(process, &mut started, &mut ended, &mut in_system, &mut in_program) };
                (in_system + in_program) as f64 / 10_000.0
            };
            let before = used(self.process);
            rest(seconds * 1000);
            used(self.process) - before
        }

        fn clipboard(&mut self) -> Option<String> {
            None
        }

        fn end(&mut self) -> Result<String, String> {
            let first = windows_of(self.id).first().copied();
            if let Some(window) = first {
                // SAFETY: a message can be posted to any handle.
                unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
            }
            release(self.automation);
            // SAFETY: the process is the walk's own; its handle is closed last.
            let closed = unsafe {
                let closed = WaitForSingleObject(self.process, 4000) == 0;
                if !closed {
                    TerminateProcess(self.process, 1);
                }
                CloseHandle(self.process);
                closed
            };
            match closed {
                true => Ok(format!("{} pictures", self.shots.len())),
                false => Err("the demo did not end when its window was closed".into()),
            }
        }
    }

    /// How many threads a program has.
    fn threads(id: u32) -> u32 {
        // SAFETY: zeros are a valid entry; `size` says how big it is.
        let mut running = Running { size: size_of::<Running>() as u32, ..unsafe { std::mem::zeroed() } };
        // SAFETY: the list of processes (2) is read entry by entry until the program is found, then closed.
        unsafe {
            let list = CreateToolhelp32Snapshot(2, 0);
            let mut more = Process32FirstW(list, &mut running);
            while more != 0 && running.process_id != id {
                more = Process32NextW(list, &mut running);
            }
            CloseHandle(list);
            if more != 0 { running.threads } else { 0 }
        }
    }
}

/// Linux: XTest, AT-SPI, and pictures of the screen.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod x11 {
    use super::{Driver, Key, Kind, Node, Rect, rest};
    use std::ffi::{CStr, c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        fn kill(process: c_int, signal: c_int) -> c_int;
    }

    type Display = *mut c_void;
    type XWindow = c_ulong;
    /// An `AtspiAccessible`.
    type Thing = *mut c_void;

    /// `XImage`: the fields read here.
    #[repr(C)]
    struct Image {
        size: [c_int; 4],
        data: *const u8,
        layout: [c_int; 5],
        bytes_per_line: c_int,
        bits_per_pixel: c_int,
        masks: [c_ulong; 3],
        made_by: [*mut c_void; 2],
        destroy: unsafe extern "C" fn(*mut Image) -> c_int,
    }
    /// `XWindowAttributes`.
    #[repr(C)]
    struct Attributes {
        place: [c_int; 2],
        width: c_int,
        height: c_int,
        border: [c_int; 2],
        visual: *mut c_void,
        root: XWindow,
        kind: [c_int; 4],
        backing: [c_ulong; 2],
        save_under: c_int,
        colormap: c_ulong,
        map_installed: c_int,
        map_state: c_int,
        rest: [c_long; 8],
    }
    /// `XExposeEvent`, padded to the size of any event.
    #[repr(C)]
    struct Uncovered {
        kind: c_int,
        serial: c_ulong,
        sent: c_int,
        display: Display,
        window: XWindow,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        count: c_int,
        rest: [c_long; 24],
    }
    /// `AtspiRect`.
    #[repr(C)]
    struct Place {
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
    }

    type Tells<T> = unsafe extern "C" fn(Thing, *mut *mut c_void) -> T;
    type TellsOf<T> = unsafe extern "C" fn(Thing, c_int, *mut *mut c_void) -> T;

    /// What the walk takes from Xlib, XTest, libatspi and glib, opened by name at the start.
    #[allow(non_snake_case)]
    struct X {
        XOpenDisplay: unsafe extern "C" fn(*const c_char) -> Display,
        XSync: unsafe extern "C" fn(Display, c_int) -> c_int,
        XKeysymToKeycode: unsafe extern "C" fn(Display, c_ulong) -> c_uchar,
        XDisplayWidth: unsafe extern "C" fn(Display, c_int) -> c_int,
        XDisplayHeight: unsafe extern "C" fn(Display, c_int) -> c_int,
        XDefaultRootWindow: unsafe extern "C" fn(Display) -> XWindow,
        XGetImage: unsafe extern "C" fn(Display, XWindow, c_int, c_int, c_uint, c_uint, c_ulong, c_int) -> *mut Image,
        XQueryTree: unsafe extern "C" fn(Display, XWindow, *mut XWindow, *mut XWindow, *mut *mut XWindow, *mut c_uint) -> c_int,
        XGetWindowAttributes: unsafe extern "C" fn(Display, XWindow, *mut Attributes) -> c_int,
        XSendEvent: unsafe extern "C" fn(Display, XWindow, c_int, c_long, *mut Uncovered) -> c_int,
        XFree: unsafe extern "C" fn(*mut c_void) -> c_int,
        XTestFakeMotionEvent: unsafe extern "C" fn(Display, c_int, c_int, c_int, c_ulong) -> c_int,
        XTestFakeButtonEvent: unsafe extern "C" fn(Display, c_uint, c_int, c_ulong) -> c_int,
        XTestFakeKeyEvent: unsafe extern "C" fn(Display, c_uint, c_int, c_ulong) -> c_int,
        atspi_init: unsafe extern "C" fn() -> c_int,
        atspi_get_desktop: unsafe extern "C" fn(c_int) -> Thing,
        atspi_accessible_set_cache_mask: unsafe extern "C" fn(Thing, c_int),
        atspi_accessible_get_child_count: Tells<c_int>,
        atspi_accessible_get_child_at_index: TellsOf<Thing>,
        atspi_accessible_get_name: Tells<*mut c_char>,
        atspi_accessible_get_role: Tells<c_int>,
        atspi_accessible_get_state_set: unsafe extern "C" fn(Thing) -> *mut c_void,
        atspi_state_set_contains: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        atspi_component_get_extents: TellsOf<*mut Place>,
        g_free: unsafe extern "C" fn(*mut c_void),
        g_error_free: unsafe extern "C" fn(*mut c_void),
        g_object_unref: unsafe extern "C" fn(*mut c_void),
    }

    fn library(name: &CStr) -> *mut c_void {
        // SAFETY: the name ends in a zero. (2: RTLD_NOW.)
        let library = unsafe { dlopen(name.as_ptr(), 2) };
        assert!(!library.is_null(), "{} is not on this machine", name.to_string_lossy());
        library
    }

    /// # Safety
    /// `F` is the type the library's header gives the function of that name.
    unsafe fn function<F: Copy>(library: *mut c_void, name: &CStr) -> F {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        // SAFETY: the library is open, and the name ends in a zero.
        let found = unsafe { dlsym(library, name.as_ptr()) };
        assert!(!found.is_null(), "{} is not in its library", name.to_string_lossy());
        // SAFETY: the caller says which type the function has.
        unsafe { std::mem::transmute_copy(&found) }
    }

    fn open() -> X {
        let (xlib, xtst) = (library(c"libX11.so.6"), library(c"libXtst.so.6"));
        let (atspi, glib, gobject) = (library(c"libatspi.so.0"), library(c"libglib-2.0.so.0"), library(c"libgobject-2.0.so.0"));
        // SAFETY: each function is declared in `X` as its header declares it.
        unsafe {
            X {
                XOpenDisplay: function(xlib, c"XOpenDisplay"),
                XSync: function(xlib, c"XSync"),
                XKeysymToKeycode: function(xlib, c"XKeysymToKeycode"),
                XDisplayWidth: function(xlib, c"XDisplayWidth"),
                XDisplayHeight: function(xlib, c"XDisplayHeight"),
                XDefaultRootWindow: function(xlib, c"XDefaultRootWindow"),
                XGetImage: function(xlib, c"XGetImage"),
                XQueryTree: function(xlib, c"XQueryTree"),
                XGetWindowAttributes: function(xlib, c"XGetWindowAttributes"),
                XSendEvent: function(xlib, c"XSendEvent"),
                XFree: function(xlib, c"XFree"),
                XTestFakeMotionEvent: function(xtst, c"XTestFakeMotionEvent"),
                XTestFakeButtonEvent: function(xtst, c"XTestFakeButtonEvent"),
                XTestFakeKeyEvent: function(xtst, c"XTestFakeKeyEvent"),
                atspi_init: function(atspi, c"atspi_init"),
                atspi_get_desktop: function(atspi, c"atspi_get_desktop"),
                atspi_accessible_set_cache_mask: function(atspi, c"atspi_accessible_set_cache_mask"),
                atspi_accessible_get_child_count: function(atspi, c"atspi_accessible_get_child_count"),
                atspi_accessible_get_child_at_index: function(atspi, c"atspi_accessible_get_child_at_index"),
                atspi_accessible_get_name: function(atspi, c"atspi_accessible_get_name"),
                atspi_accessible_get_role: function(atspi, c"atspi_accessible_get_role"),
                atspi_accessible_get_state_set: function(atspi, c"atspi_accessible_get_state_set"),
                atspi_state_set_contains: function(atspi, c"atspi_state_set_contains"),
                atspi_component_get_extents: function(atspi, c"atspi_component_get_extents"),
                g_free: function(glib, c"g_free"),
                g_error_free: function(glib, c"g_error_free"),
                g_object_unref: function(gobject, c"g_object_unref"),
            }
        }
    }

    /// Keysyms.
    const SHIFT: c_ulong = 0xFFE1;
    const CONTROL: c_ulong = 0xFFE3;
    /// AT-SPI states (atspi-constants.h).
    const CHECKED: c_int = 4;
    const SHOWING: c_int = 25;

    /// The kind of an AT-SPI role (atspi-constants.h).
    fn kind(role: c_int) -> Kind {
        match role {
            // (Switches too, as UI Automation has them.)
            43 | 62 | 130 => Kind::Button,
            79 | 40 => Kind::Entry,
            35 | 8 | 45 => Kind::MenuItem,
            33 => Kind::Menu,
            37 => Kind::Tab,
            10 => Kind::Heading,
            56 => Kind::Cell,
            51 => Kind::Slider,
            52 => Kind::Spin,
            29 => Kind::Text,
            23 => Kind::Window,
            16 => Kind::Dialog,
            39 => Kind::Group,
            _ => Kind::Other,
        }
    }

    /// What another program reads from the clipboard: `xclip`, given five seconds.
    fn read_by_another_program() -> String {
        let mut xclip = Command::new("xclip").args(["-selection", "clipboard", "-o"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("xclip could not be started");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && matches!(xclip.try_wait(), Ok(None)) {
            rest(10);
        }
        let _ = xclip.kill();
        let read = xclip.wait_with_output().expect("xclip could not be heard");
        String::from_utf8_lossy(&read.stdout).into_owned()
    }

    /// The screen at one moment: its width, and its pixels `0x__RRGGBB` row by row.
    struct Screen {
        wide: usize,
        pixels: Vec<u32>,
    }

    /// Asks the session bus's `org.a11y.Bus` service something.
    fn a11y(method: &str, with: &[&str]) -> String {
        let asked = Command::new("gdbus").args(["call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus", "--method", method]).args(with).stderr(Stdio::null()).output();
        asked.map(|answer| String::from_utf8_lossy(&answer.stdout).into_owned()).unwrap_or_default()
    }

    /// Says whether a screen reader listens, as Orca does when it starts and ends.
    fn listening(on: bool) {
        a11y("org.freedesktop.DBus.Properties.Set", &["org.a11y.Status", "IsEnabled", if on { "<true>" } else { "<false>" }]);
    }

    /// Passes on what the walk writes to stderr, leaving out the log lines of the bus and of the
    /// services it starts.
    fn forward(said: std::process::ChildStderr) {
        for line in std::io::BufRead::lines(std::io::BufReader::new(said)).map_while(Result::ok) {
            if !(line.is_empty() || line.starts_with("dbus-daemon[") || line.starts_with("(/")) {
                eprintln!("{line}");
            }
        }
    }

    /// The processes a process started, and theirs, deepest last.
    fn descendants(of: u32) -> Vec<u32> {
        let parent = |process: u32| {
            let nodes = std::fs::read_to_string(format!("/proc/{process}/stat")).unwrap_or_default();
            nodes.rsplit_once(')').and_then(|(_, after)| after.split_whitespace().nth(1)?.parse::<u32>().ok())
        };
        let all: Vec<u32> = std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str()?.parse().ok()).collect();
        let mut found = vec![of];
        let mut at = 0;
        while at < found.len() {
            let under = found[at];
            found.extend(all.iter().filter(|process| parent(**process) == Some(under)));
            at += 1;
        }
        found.remove(0);
        found
    }

    /// The walk's side on Linux: X, AT-SPI, the demo and the bus it is told over.
    pub struct Walk {
        x: X,
        display: Display,
        demo: Child,
        launcher: Child,
        program: String,
        folder: PathBuf,
        differing: Vec<String>,
        shots: usize,
    }

    /// Starts the screen readers' bus (nobody listening yet), then the demo. Without a session
    /// bus, runs this program again under `dbus-run-session`.
    pub fn start(folder: &Path, program: &Path) -> Walk {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|bus| bus.is_empty()) {
            let this = std::env::current_exe().expect("this program does not know where it is");
            let mut again = Command::new("dbus-run-session").arg("--").arg(this).args(std::env::args_os().skip(1)).stderr(Stdio::piped()).spawn().expect("dbus-run-session could not be started");
            let said = again.stderr.take().expect("its errors are piped");
            // Not joined: a service the bus started may hold the pipe open after the walk ends.
            std::thread::spawn(move || forward(said));
            std::process::exit(again.wait().map_or(1, |status| status.code().unwrap_or(1)));
        }
        let at_spi = ["/usr/libexec", "/usr/lib", "/usr/lib/at-spi2-core"].iter().map(|folder| Path::new(folder).join("at-spi-bus-launcher")).find(|file| file.exists());
        let launcher = Command::new(at_spi.expect("at-spi-bus-launcher is not on this machine"))
            .env("GSETTINGS_BACKEND", "memory")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("at-spi-bus-launcher could not be started");
        for _ in 0..100 {
            if a11y("org.freedesktop.DBus.Properties.GetAll", &["org.a11y.Status"]).contains("IsEnabled") {
                break;
            }
            rest(50);
        }
        listening(false);
        let demo = Command::new(program).spawn().expect("the program could not be started");
        rest(1500);
        let x = open();
        // SAFETY: libatspi is started once, before it is asked anything.
        unsafe { (x.atspi_init)() };
        // SAFETY: no name asks for the display the environment names.
        let display = unsafe { (x.XOpenDisplay)(null()) };
        assert!(!display.is_null(), "there is no display to walk on");
        let program = program.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let walk = Walk { x, display, demo, launcher, program, folder: folder.to_path_buf(), differing: Vec::new(), shots: 0 };
        walk.sync(500);
        walk
    }

    impl Walk {
        /// Waits until X has taken in what was asked, then `wait` ms for the demo.
        fn sync(&self, wait: u64) {
            // SAFETY: the display is open.
            unsafe { (self.x.XSync)(self.display, 0) };
            rest(wait);
        }

        /// A mouse button goes down or up where the pointer is.
        fn button(&self, button: c_uint, down: bool) {
            // SAFETY: the display is open.
            unsafe { (self.x.XTestFakeButtonEvent)(self.display, button, down as c_int, 0) };
        }

        fn key_for(&self, sym: c_ulong) -> c_uint {
            // SAFETY: the display is open; an unknown keysym gives 0.
            unsafe { (self.x.XKeysymToKeycode)(self.display, sym) as c_uint }
        }

        fn key_goes(&self, key: c_uint, down: bool) {
            // SAFETY: the display is open.
            unsafe { (self.x.XTestFakeKeyEvent)(self.display, key, down as c_int, 0) };
        }

        /// Presses a key while others are held.
        fn press(&self, sym: c_ulong, held: &[c_ulong]) {
            let (held, key): (Vec<c_uint>, c_uint) = (held.iter().map(|sym| self.key_for(*sym)).collect(), self.key_for(sym));
            held.iter().for_each(|held| self.key_goes(*held, true));
            self.key_goes(key, true);
            self.key_goes(key, false);
            held.iter().for_each(|held| self.key_goes(*held, false));
            self.sync(30);
        }

        fn screen(&self) -> Screen {
            // SAFETY: the display is open; all of screen 0 is asked for as ZPixmap (2).
            let (wide, high, image) = unsafe {
                let (wide, high) = ((self.x.XDisplayWidth)(self.display, 0), (self.x.XDisplayHeight)(self.display, 0));
                (wide as usize, high as usize, (self.x.XGetImage)(self.display, (self.x.XDefaultRootWindow)(self.display), 0, 0, wide as c_uint, high as c_uint, 0xFFFF_FFFF, 2))
            };
            assert!(!image.is_null(), "X gave no picture of the screen");
            // SAFETY: the image holds `high` rows of `bytes_per_line` bytes, 4 to a pixel.
            let (bytes, in_a_row) = unsafe { (std::slice::from_raw_parts((*image).data, (*image).bytes_per_line as usize * high), (*image).bytes_per_line as usize) };
            let pixels = bytes.chunks_exact(in_a_row).flat_map(|row| row[..wide * 4].as_chunks::<4>().0).map(|pixel| u32::from_le_bytes(*pixel)).collect();
            // SAFETY: the image is given back through its own function, and not read after.
            unsafe { ((*image).destroy)(image) };
            Screen { wide, pixels }
        }

        /// The screen once it has stopped changing: two pictures alike 50 ms apart, or the
        /// last of 3 s of them.
        fn still_screen(&self) -> Screen {
            let mut last = self.screen();
            for _ in 0..60 {
                rest(50);
                let now = self.screen();
                if now.pixels == last.pixels {
                    return now;
                }
                last = now;
            }
            last
        }

        /// Whether the pixels at `places` go back to how `drawn` had them within about a
        /// second: a blinking cursor. Something left stale never does.
        fn blinks(&self, drawn: &Screen, places: &[usize]) -> bool {
            let mut back = vec![false; places.len()];
            for _ in 0..25 {
                rest(50);
                let now = self.screen();
                back.iter_mut().zip(places).for_each(|(back, at)| *back |= now.pixels[*at] == drawn.pixels[*at]);
                if back.iter().all(|back| *back) {
                    return true;
                }
            }
            false
        }

        fn save(&self, name: &str, screen: &Screen) {
            let file = self.folder.join(format!("{name}.png"));
            std::fs::write(file, taut::png::encode(screen.wide, &screen.pixels).unwrap()).expect("the picture could not be saved");
        }

        /// Tells a window that shows that all of it was uncovered.
        fn uncover(&self, window: XWindow) {
            // SAFETY: zeros are a valid `XWindowAttributes`, which X fills in.
            let mut about: Attributes = unsafe { std::mem::zeroed() };
            // SAFETY: the display is open, and `about` is as big as X writes.
            let shows = unsafe { (self.x.XGetWindowAttributes)(self.display, window, &mut about) } != 0 && about.map_state == 2;
            if !shows {
                return;
            }
            let mut news = Uncovered { kind: 12, serial: 0, sent: 1, display: null_mut(), window, x: 0, y: 0, width: about.width, height: about.height, count: 0, rest: [0; 24] };
            // SAFETY: the event is as big as any, sent to windows that asked for ExposureMask (1 << 15).
            unsafe { (self.x.XSendEvent)(self.display, window, 0, 1 << 15, &mut news) };
        }

        /// Tells every top-level window that shows to draw all of itself.
        fn draw_afresh(&self) {
            let (mut root, mut parent, mut children, mut count) = (0, 0, null_mut(), 0);
            // SAFETY: the display is open; each is room for what X answers.
            unsafe { (self.x.XQueryTree)(self.display, (self.x.XDefaultRootWindow)(self.display), &mut root, &mut parent, &mut children, &mut count) };
            if !children.is_null() {
                // SAFETY: X handed over a list of `count` windows, freed after.
                unsafe { std::slice::from_raw_parts(children, count as usize) }.iter().for_each(|window| self.uncover(*window));
                // SAFETY: the list is X's to free, and not read after.
                unsafe { (self.x.XFree)(children.cast()) };
            }
            self.sync(100);
        }

        /// Milliseconds of processor the demo has used, in itself and the system (counted in
        /// hundredths of a second, fields 14 and 15 of /proc/<pid>/stat).
        fn processor_so_far(&self) -> u64 {
            let nodes = std::fs::read_to_string(format!("/proc/{}/stat", self.demo.id())).unwrap_or_default();
            let counted = nodes.rsplit_once(')').map_or("", |(_, counted)| counted);
            counted.split_whitespace().skip(11).take(2).filter_map(|ticks| ticks.parse::<u64>().ok()).sum::<u64>() * 10
        }

        /// A name libatspi handed over, freed after it is read.
        fn words(&self, words: *mut c_char) -> String {
            if words.is_null() {
                return String::new();
            }
            // SAFETY: the text ends in a zero, and is the caller's to free.
            let read = unsafe { CStr::from_ptr(words) }.to_string_lossy().into_owned();
            // SAFETY: as above; not read after.
            unsafe { (self.x.g_free)(words.cast()) };
            read
        }

        /// What libatspi answered, or nothing where it could not get an answer.
        fn answered<T>(&self, answer: T, problem: *mut c_void) -> Option<T> {
            if problem.is_null() {
                return Some(answer);
            }
            // SAFETY: a `GError` of the caller's, not read after.
            unsafe { (self.x.g_error_free)(problem) };
            None
        }

        fn ask<T>(&self, asked: Tells<T>, thing: Thing) -> Option<T> {
            let mut problem = null_mut();
            // SAFETY: the function is libatspi's, the thing one it handed out.
            let answer = unsafe { asked(thing, &mut problem) };
            self.answered(answer, problem)
        }

        fn ask_of<T>(&self, asked: TellsOf<T>, thing: Thing, of: c_int) -> Option<T> {
            let mut problem = null_mut();
            // SAFETY: as in `ask`.
            let answer = unsafe { asked(thing, of, &mut problem) };
            self.answered(answer, problem)
        }

        fn children(&self, thing: Thing) -> Vec<Thing> {
            let count = self.ask(self.x.atspi_accessible_get_child_count, thing).unwrap_or(0);
            (0..count).filter_map(|at| self.ask_of(self.x.atspi_accessible_get_child_at_index, thing, at)).filter(|child| !child.is_null()).collect()
        }

        /// Whether a thing is in a state.
        fn is(&self, thing: Thing, state: c_int) -> bool {
            // SAFETY: the set is libatspi's, let go of after it is read.
            unsafe {
                let states = (self.x.atspi_accessible_get_state_set)(thing);
                let shows = !states.is_null() && (self.x.atspi_state_set_contains)(states, state) != 0;
                if !states.is_null() {
                    (self.x.g_object_unref)(states);
                }
                shows
            }
        }

        /// A thing's place on the screen (0: counted from the screen's corner).
        fn place(&self, thing: Thing) -> Rect {
            let Some(place) = self.ask_of(self.x.atspi_component_get_extents, thing, 0).filter(|place| !place.is_null()) else { return Rect::default() };
            // SAFETY: libatspi made the rect; it is freed after it is read.
            let (x, y, w, h) = unsafe { ((*place).x, (*place).y, (*place).w, (*place).h) };
            // SAFETY: as above.
            unsafe { (self.x.g_free)(place.cast()) };
            Rect { x: x as f64, y: y as f64, w: w as f64, h: h as f64 }
        }

        /// The demo among the programs of the desktop, if it is there.
        fn app(&self) -> Option<Thing> {
            // SAFETY: libatspi was started; desktop 0 is the only one.
            let desktop = unsafe { (self.x.atspi_get_desktop)(0) };
            self.children(desktop).into_iter().find(|app| self.ask(self.x.atspi_accessible_get_name, *app).map(|name| self.words(name)).as_deref() == Some(self.program.as_str()))
        }

        /// The demo's windows that show.
        fn frames(&self) -> Vec<Thing> {
            self.app().map_or(Vec::new(), |app| self.children(app).into_iter().filter(|frame| self.is(*frame, SHOWING)).collect())
        }
    }

    impl Driver for Walk {
        fn nodes(&mut self) -> Vec<Node> {
            let mut todo: Vec<Thing> = self.frames().into_iter().rev().collect();
            let mut nodes = Vec::new();
            while let Some(thing) = todo.pop() {
                let name = self.ask(self.x.atspi_accessible_get_name, thing).map(|name| self.words(name)).unwrap_or_default();
                let role = self.ask(self.x.atspi_accessible_get_role, thing).unwrap_or(0);
                nodes.push(Node { kind: kind(role), name, at: self.place(thing), on: self.is(thing, CHECKED) });
                todo.extend(self.children(thing).into_iter().filter(|child| self.is(*child, SHOWING)).rev());
            }
            nodes
        }

        fn windows(&mut self) -> usize {
            self.frames().len()
        }

        fn point(&mut self, (x, y): (f64, f64)) {
            // SAFETY: the display is open; screen 0.
            unsafe { (self.x.XTestFakeMotionEvent)(self.display, 0, x as c_int, y as c_int, 0) };
            self.sync(80);
        }

        fn click(&mut self, at: (f64, f64)) {
            self.point(at);
            self.button(1, true);
            self.sync(50);
            self.button(1, false);
            self.sync(150);
        }

        fn drag(&mut self, from: (f64, f64), to: (f64, f64)) {
            self.point(from);
            self.button(1, true);
            self.sync(50);
            for step in 1..=5 {
                let step = step as f64 / 5.0;
                self.point((from.0 + (to.0 - from.0) * step, from.1 + (to.1 - from.1) * step));
            }
            self.button(1, false);
            self.sync(150);
        }

        fn wheel(&mut self, at: (f64, f64), notches: u32) {
            self.point(at);
            // Each notch towards the person is button 5.
            for _ in 0..notches {
                self.button(5, true);
                self.button(5, false);
                self.sync(50);
            }
        }

        fn key(&mut self, key: Key, control: bool) {
            let sym = match key {
                Key::Enter => 0xFF0D,
                Key::Escape => 0xFF1B,
                Key::Up => 0xFF52,
                Key::Backspace => 0xFF08,
                Key::Letter(letter) => letter as c_ulong,
            };
            self.press(sym, if control { &[CONTROL] } else { &[] });
            self.sync(120);
        }

        fn typed(&mut self, text: &str) {
            for letter in text.chars() {
                self.press(letter.to_ascii_lowercase() as c_ulong, if letter.is_uppercase() { &[SHIFT] } else { &[] });
            }
            self.sync(150);
        }

        /// Saves the screen as drawn, then has everything redrawn and notes any difference.
        fn shot(&mut self, name: &str) -> Result<(), String> {
            self.shots += 1;
            let drawn = self.still_screen();
            self.draw_afresh();
            let afresh = self.still_screen();
            self.save(name, &drawn);
            if drawn.pixels.iter().all(|pixel| Some(pixel) == drawn.pixels.first()) {
                return Err("the screen is all one colour".into());
            }
            if drawn.pixels == afresh.pixels {
                return Ok(());
            }
            let differing: Vec<usize> = (0..drawn.pixels.len()).filter(|at| drawn.pixels[*at] != afresh.pixels[*at]).collect();
            if self.blinks(&drawn, &differing) {
                return Ok(());
            }
            let places: Vec<(usize, usize)> = differing.iter().map(|at| (at % drawn.wide, at / drawn.wide)).collect();
            let (left, right) = (places.iter().map(|at| at.0).min().unwrap_or(0), places.iter().map(|at| at.0).max().unwrap_or(0));
            let (top, bottom) = (places.iter().map(|at| at.1).min().unwrap_or(0), places.iter().map(|at| at.1).max().unwrap_or(0));
            self.differing.push(format!("{name}: {} pixels, in {} by {} at {left},{top}", places.len(), right - left + 1, bottom - top + 1));
            self.save(&format!("{name}-afresh"), &afresh);
            Ok(())
        }

        fn memory(&mut self, when: &str) {
            // Nobody listens while memory is measured: what is kept for screen readers is let go.
            listening(false);
            rest(500);
            let nodes = std::fs::read_to_string(format!("/proc/{}/smaps_rollup", self.demo.id())).unwrap_or_default();
            let megabytes = |kind: &str| {
                let line = nodes.lines().find(|line| line.split(':').next() == Some(kind));
                line.and_then(|line| line.split_whitespace().nth(1)).and_then(|kilobytes| kilobytes.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
            };
            println!("{when}: {:.1} MB in use, {:.1} MB of its own", megabytes("Rss"), megabytes("Private_Dirty"));
            self.listen();
        }

        fn processor(&mut self, seconds: u64) -> f64 {
            let before = self.processor_so_far();
            rest(seconds * 1000);
            (self.processor_so_far() - before) as f64
        }

        fn clipboard(&mut self) -> Option<String> {
            Some(read_by_another_program())
        }

        fn end(&mut self) -> Result<String, String> {
            // SAFETY: the demo is the walk's own child, asked to end (SIGTERM).
            unsafe { kill(self.demo.id() as c_int, 15) };
            let _ = self.demo.wait();
            for process in descendants(self.launcher.id()).into_iter().rev() {
                // SAFETY: a process of the walk's own bus, ended at once (SIGKILL).
                unsafe { kill(process as c_int, 9) };
            }
            let _ = self.launcher.kill();
            let _ = self.launcher.wait();
            match self.differing.is_empty() {
                true => Ok(format!("{} pictures; drawn afresh, each is the same", self.shots)),
                false => Err(format!("{} pictures; drawn afresh, these differ (saved beside them):\n  {}", self.shots, self.differing.join("\n  "))),
            }
        }
    }

    impl Walk {
        /// Says a screen reader listens, and waits until the demo is among the programs.
        fn listen(&mut self) {
            listening(true);
            let since = Instant::now();
            while self.app().is_none() && since.elapsed() < Duration::from_secs(5) {
                rest(50);
            }
            if let Some(app) = self.app() {
                // SAFETY: an accessible libatspi handed out; 0: nothing is cached.
                unsafe { (self.x.atspi_accessible_set_cache_mask)(app, 0) };
            }
        }
    }
}
