# 8. Testing

A test can drive a `Ui` with no window: feed it input, then check the app's state, what a screen
reader would hear, or the pixels. Such tests run wherever `cargo test` runs, with no display, in
milliseconds.

## A trial

`taut::testing::Trial` is an app's window on no screen, driven as a user would, finding parts by
role and name as a screen reader does:

```rust
use taut::testing::Trial;
use taut::{Key, Mods, Role, Ui};

/// The app: a list of guests, and a box to add one.
struct Guests {
    names: Vec<String>,
    typed: String,
}

fn describe(guests: &mut Guests, ui: &mut Ui) {
    ui.row().pad(12.0).gap(8.0).open();
    let typed = ui.text_box(&mut guests.typed, "A name");
    let add = ui.button("Add").clicked || typed.focused && ui.key(Key::Enter, Mods::NONE);
    ui.close();
    if add && !guests.typed.is_empty() {
        guests.names.push(std::mem::take(&mut guests.typed));
    }
    ui.column().pad((12.0, 0.0)).open();
    for name in &guests.names {
        ui.label(name);
    }
    ui.close();
}

let mut trial = Trial::new(Guests { names: Vec::new(), typed: String::new() }, describe);
// Into the box (an empty box is called by its hint), a name, and the button by its words.
trial.fill("A name", "Ada");
trial.press(Role::Button, "Add");
assert_eq!(trial.app.names, ["Ada"]);
// Into the box again, and Enter this time.
trial.fill("A name", "Alan");
trial.key("Enter");
assert_eq!(trial.app.names, ["Ada", "Alan"]);
// The box is empty again at once: a key the app takes has the window described once more.
assert!(trial.visible_access_nodes().iter().any(|said| said.role == Role::TextBox && said.name == "A name" && said.value.is_empty()));
// And the window shows both: a screen reader would be told of them.
assert!(trial.has(Role::Text, "Ada") && trial.has(Role::Text, "Alan"));
```

- After every step, the partial redraw must equal a full redraw.
- `trial.looks_like("tests/looks/guests.png")` compares with a saved picture (saved on first
  run); built-in letters make it identical on every system.
- Put an app's tests in `mod tests` in `main.rs` and drive its own `describe` (hence chapter 4's
  file field). The examples `file_browser`, `editor`, `settings`, `chat`, `viewer`, `dashboard`,
  `node_editor` and `report` test themselves this way.

## Trial reference

| To | Call |
| --- | --- |
| Open the app's window, 640×480 units at scale 1 | `Trial::new(app, describe)` |
| Resize or rescale | `trial.resize(800.0, 600.0, 1.5)` |
| Use system letters instead of built-in ones | `trial.ui.set_text_system(taut::platform::system_text())` (pictures then vary by system) |
| Press a part by role and name | `trial.press(Role::Button, "Save")` |
| Type into a box by its hint | `trial.fill("Name", "Ada")` |
| Type at the focus, or press keys | `trial.type_text("Ada")`, `trial.key("Ctrl+S")` |
| Click at a point (right, double, with modifiers) | `trial.click(at)`, `trial.right_click(at)`, `trial.double_click(at)`, `trial.click_with(at, Button::Left, Mods::CTRL)` |
| Find a part's position | `trial.center_of(Role::Row, "notes.txt")` |
| Drag, scroll | `trial.drag(from, to)`, `trial.wheel(at, 0.0, 40.0)` |
| Drop files on it | `trial.drop_files(at, &paths)` |
| Let time pass | `trial.wait(Duration::from_secs(1))`; `trial.set_time(..)` passes the time to the app |
| Check for a part, or get it | `trial.has(Role::Text, "Saved")`, `trial.find(Role::CheckBox, "Loud")` |
| Read what a screen reader gets | `trial.visible_access_nodes()` (visible), `trial.access_nodes()` (all) |
| List everything, one line each | `trial.dump()` (printed when `find` fails) |
| Save a picture | `trial.save_png("shot.png")` |
| Compare with a saved picture | `trial.looks_like("tests/looks/main.png")` |
| Read or change app state | `trial.app` |

Parts are found by `Role`, which may not match other libraries' words:

| Usual term | Part | Role |
| --- | --- | --- |
| label, text | `ui.label`, `ui.note`, `ui.paragraph`, `ui.text` | `Role::Text` |
| button, toggle button | `ui.button`, `ui.primary_button`, `ui.icon_button`, `ui.toggle`, `ui.tool` | `Role::Button` (a toggle with `on`) |
| checkbox | `ui.checkbox` | `Role::CheckBox` |
| switch | `ui.switch` | `Role::Switch` |
| radio button | `ui.radio` | `Role::RadioButton` |
| slider, drag value, rating | `ui.slider`, `ui.range` (one per knob), `ui.drag_value`, `ui.rating`, `ui.splitter` | `Role::Slider` |
| spin box | `ui.number`, `ui.number_in`, each field of `ui.time` | `Role::SpinButton` |
| drop-down, combo box | `ui.select`, `ui.combo`, the button of `ui.color_picker` | `Role::ComboBox` |
| text field | `ui.text_box`, `ui.text_area`, `ui.search`, `ui.autocomplete` | `Role::TextBox` |
| password field | `ui.password_box` | `Role::PasswordBox` |
| collapsing header, date picker, menu button | `ui.fold`, `ui.date`, `ui.date_range`, `ui.menu_button`, a panel's heading | `Role::Fold` |
| progress bar, spinner | `ui.progress`, `ui.spinner` | `Role::Progress` |
| tab bar, segmented control, steps, pages | `ui.tabs`, `ui.segmented`, `ui.steps`, `ui.pagination` | `Role::Tabs`, each tab `Role::Tab` |
| link | `ui.link`, a place in `ui.breadcrumbs` | `Role::Link` |
| selectable item | `ui.selectable` | `Role::Row` |
| tree and items | `ui.tree_view` and `ui.tree_row`; `ui.tree` | `Role::Tree`, each item `Role::TreeItem` |
| menu bar, menu, items | `ui.menu_bar`, a menu, `ui.menu_item`, `ui.menu_checkbox`, `ui.menu_radio`, `ui.submenu` | `Role::MenuBar`, `Role::Menu`, `Role::MenuItem` (a check with `on`), `Role::MenuItemRadio`, `Role::Submenu` |
| toolbar | `ui.tool_bar` | `Role::ToolBar` |
| table | `ui.table`, its headings, rows, `ui.cell` | `Role::Table`, `Role::ColumnHeader`, `Role::TableRow`, `Role::Cell` |
| inner window, card, time picker | `ui.panel`, `ui.card`, `ui.time` | `Role::Group` |
| modal dialog | `ui.dialog` | `Role::Dialog` |
| document | `ui.markdown` | `Role::Document` |
| image | `ui.picture`, named by the app: `ui.tell(&shown, Role::Picture, None, "A map of the bay")` | `Role::Picture` |

## By hand

A program that runs the interface itself (chapter 9) does what a trial does:

```rust
use taut::geom::{Size, pt};
use taut::text::BuiltInFont;
use taut::{Button, Event, LocalClipboard, Mods, Surface, Ui};

fn describe(count: &mut u32, ui: &mut Ui) {
    ui.row().pad(12.0).open();
    if ui.button("Press me").clicked {
        *count += 1;
    }
    ui.close();
}

// A window of 200 by 60 units, a unit a pixel, with letters that need no system.
let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
ui.resize(Size { w: 200.0, h: 60.0 }, 1.0);
let mut count = 0;
ui.run(&mut |ui| describe(&mut count, ui));

// The person moves the pointer onto the button, presses, and lets go.
for event in [Event::Move(pt(40.0, 26.0)), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
    if ui.event(event) {
        ui.run(&mut |ui| describe(&mut count, ui));
    }
}
assert_eq!(count, 1);

// And what the window shows, as pixels: 200 by 60 of them.
let picture = Surface::draw(&mut ui, 64);
assert_eq!((picture.width, picture.height, picture.pixels.len()), (200, 60, 12_000));
```

- `BuiltInFont`: a fixed font (letters 12 units wide, lines 20 high) on every machine;
  `LocalClipboard` stays in-process.
- `ui.resize(size, scale)`; `ui.run(&mut describe)` returns the changed rect; `ui.event(event)`
  feeds one input and returns whether a run is needed; `Surface::draw(&mut ui, 64)` draws it all
  (`surface.to_png()` encodes it).
- Find parts by name, not coordinates: `ui.set_screen_reader(true)`, then `ui.visible_access_nodes()` (chapter 7).
- Run twice at the start (input is resolved against the previous layout), and after every event.
- Focus follows clicks: type into a box after clicking it or setting focus.
- Time stands still unless set: `ui.set_time(duration)`. `ui.wake_in()` says when the interface
  next wants to run (`None`: never).

## Time and pixels

The library's own tests check, after every step, that partial redraws equal a full redraw. An
app with custom drawing can do the same. This test also advances time while a switch slides:

```rust
use std::time::Duration;
use taut::geom::{Size, pt};
use taut::text::BuiltInFont;
use taut::{Button, Event, LocalClipboard, Mods, Surface, Ui};

fn describe(on: &mut bool, ui: &mut Ui) {
    ui.column().pad(12.0).gap(8.0).open();
    ui.switch(on, "Lights");
    ui.label(if *on { "They are on" } else { "They are off" });
    ui.close();
}

let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
ui.resize(Size { w: 240.0, h: 80.0 }, 1.0);
let mut on = false;
ui.run(&mut |ui| describe(&mut on, ui));
let mut screen = Surface::draw(&mut ui, 64);

/// Takes in what happened, if anything, runs the interface, and draws again only the part
/// that changed, as a window does.
fn step(ui: &mut Ui, screen: &mut Surface, on: &mut bool, event: Option<Event>) {
    if let Some(event) = event {
        ui.event(event);
    }
    let changed = ui.run(&mut |ui| describe(on, ui));
    screen.redraw(ui, changed);
}

// The switch is clicked.
for event in [Event::Move(pt(30.0, 22.0)), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
    step(&mut ui, &mut screen, &mut on, Some(event));
}
assert!(on);
// The knob is on its way over: the interface wants to run again a sixtieth of a second on.
assert_eq!(ui.wake_in(), Some(Duration::from_millis(16)));
ui.set_time(Duration::from_millis(500));
step(&mut ui, &mut screen, &mut on, None);
// It has arrived, and asks for nothing more.
assert_eq!(ui.wake_in(), None);
// And what the window shows is the same, to the pixel, as all of it drawn afresh.
assert!(screen.pixels == Surface::draw(&mut ui, 64).pixels);
```

To compare against a saved picture (`surface.to_png()` to write, `taut::png::decode` to read), use
`BuiltInFont`: system fonts differ between machines and versions.

## System letters

For pictures that look like the real window, use the system's text. The book's pictures are made
this way by `examples/book.rs`
(`cargo run -p taut --features emoji --example book -- some/folder`).

```rust
use taut::geom::Size;
use taut::{LocalClipboard, Ui};

// The system's letters, where the system has them to give.
let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
ui.resize(Size { w: 320.0, h: 120.0 }, 1.0);
ui.run(&mut |ui| drop(ui.label("In the system's own letters")));
```

`platform::system_text()` gives system letters on Windows and Linux, or the built-in ones where
the system has none (Linux without FreeType or Fontconfig). To know which,
`platform::SystemText::new()` returns `None` there.

## Not covered

The real window, the system clipboard, drags from file managers and real screen readers are
tested by the library itself, in real windows.

## Summary

- A `Trial` is a window on no screen: `press`, `fill`, `key`, `drag`, `wait`; `has`, `find`;
  `looks_like`.
- By hand: `Ui::new` with `BuiltInFont` and `LocalClipboard`; `event` then `run`. Run twice at
  start and after every event.
- The test owns time: `ui.set_time`, `ui.wake_in`.
- `Surface` gives pixels to compare or save.

Next: [inside another program](09-inside-another-program.md).
