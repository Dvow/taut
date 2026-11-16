//! An app's frame: menus with icons, checks, radio items, shortcuts and access keys; tool bars;
//! popovers; docks with tabs moved, closed and floated; panels docked to edges.
mod common;
use common::*;
use std::time::Duration;
use taut::geom::{Size, pt, rect};
use taut::{Event, Icon, Key, Mods, Rect, Role, Ui};

const STAR: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");
const ALT: Mods = Mods { alt: true, ctrl: false, shift: false };

/// The columns of `area` that hold anything drawn on the ground at its left edge (in the
/// middle of its height): the first and the last, in units from the window's edge.
fn ink(ui: &Window, area: Rect) -> Option<[f32; 2]> {
    let (wide, at) = (ui.screen.width, |x: f32, y: f32| ui.screen.pixels[y as usize * ui.screen.width + x as usize]);
    let ground = at(area.x + 1.0, area.y + area.h / 2.0);
    let drawn = |x: usize| (area.y as usize..area.bottom() as usize).any(|y| ui.screen.pixels[y * wide + x] != ground);
    let columns: Vec<usize> = (area.x as usize..area.right() as usize).filter(|x| drawn(*x)).collect();
    Some([*columns.first()? as f32, *columns.last()? as f32])
}

/// What a screen reader is told of the thing called `name`.
fn told<'a>(ui: &'a Window, name: &str) -> &'a taut::Said {
    ui.said().iter().find(|said| said.name == name).unwrap_or_else(|| panic!("nothing called {name:?} in {:?}", ui.said().iter().map(|said| &said.name).collect::<Vec<_>>()))
}

#[derive(Default)]
struct Menus {
    file: bool,
    edit: bool,
    rows: [Rect; 6],
    tick: bool,
    view: u8,
    picked: Vec<&'static str>,
    bar: Rect,
    other: Rect,
    undo: Rect,
}

fn menus(app: &mut Menus, ui: &mut Ui) {
    (app.file, app.edit) = (false, false);
    app.bar = ui.menu_bar().rect;
    if ui.menu_button("&File") {
        app.file = true;
        let rows = [
            ui.icon_menu_item(STAR, "Show &all\tCtrl+S"),
            ui.menu_check(&mut app.tick, "Show &grid\tAlt+S"),
            ui.menu_radio(&mut app.view, 0, "Show as &list"),
            ui.menu_radio(&mut app.view, 1, "Show as &tiles\tCtrl+Shift+S"),
            ui.menu_item("Show &nothing\tShift+S"),
            ui.menu_item("Show && &hide"),
        ];
        for (number, row) in rows.iter().enumerate() {
            app.rows[number] = row.rect;
            if row.clicked {
                app.picked.push(["all", "grid", "list", "tiles", "nothing", "hide"][number]);
            }
        }
        ui.close();
    }
    if ui.menu_button("&Edit") {
        app.edit = true;
        app.undo = ui.menu_item("&Undo\tCtrl+Z").rect;
        ui.close();
    }
    ui.close();
    app.other = ui.button("Elsewhere").rect;
}

#[test]
fn rows_of_a_menu_line_up_their_words_marks_and_keys() {
    let (mut app, mut ui) = (Menus { view: 1, ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| menus(&mut app, ui));
    click(&mut ui, pt(20.0, 14.0), |ui| menus(&mut app, ui));
    settle(&mut ui, |ui| menus(&mut app, ui));
    assert!(app.file);
    // (Inside the round corners of a row that is lit: the first is, having the keyboard.)
    let inks: Vec<[f32; 2]> = app.rows.iter().map(|row| ink(&ui, rect(row.x + 4.0, row.y + 4.0, row.w - 8.0, row.h - 8.0)).unwrap()).collect();
    // The marks stand at the start of the rows (the icon, the dot of the choice that is
    // picked), and the words of every other row start where a mark's words do: after the
    // room kept for one, the same in every row.
    let start = app.rows[0].x + 10.0;
    assert!(inks[0][0] < start + 3.0 && inks[3][0] < start + 6.0, "{inks:?}");
    let words: Vec<f32> = [1, 2, 4].iter().map(|row| inks[*row][0]).collect();
    assert!(words.iter().all(|x| *x == words[0] && *x >= start + 24.0), "{words:?}");
    // The keys end where the row does, less its room, whatever the words before them.
    let ends: Vec<f32> = [0, 1, 3, 4].iter().map(|row| inks[*row][1]).collect();
    assert!(ends.iter().all(|x| *x == ends[0] && *x > app.rows[0].right() - 14.0), "{ends:?} in {:?}", app.rows[0]);
    // A menu whose rows have no mark keeps no room for one: Edit's words start at its edge.
    does(&mut ui, Event::Move(pt(85.0, 14.0)), |ui| menus(&mut app, ui));
    settle(&mut ui, |ui| menus(&mut app, ui));
    assert!(app.edit && ink(&ui, app.undo).unwrap()[0] < app.undo.x + 13.0);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| menus(&mut app, ui));

    // Ticked, the row shows its tick where the icon of the first is.
    click(&mut ui, pt(20.0, 14.0), |ui| menus(&mut app, ui));
    settle(&mut ui, |ui| menus(&mut app, ui));
    click(&mut ui, middle(app.rows[1]), |ui| menus(&mut app, ui));
    assert!(app.tick && !app.file);
    click(&mut ui, pt(20.0, 14.0), |ui| menus(&mut app, ui));
    settle(&mut ui, |ui| menus(&mut app, ui));
    assert!(ink(&ui, app.rows[1]).unwrap()[0] < start + 4.0);
}

#[test]
fn a_choice_in_a_menu_is_picked_and_told_as_one() {
    let (mut app, mut ui) = (Menus::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| menus(&mut app, ui));
    click(&mut ui, pt(20.0, 14.0), |ui| menus(&mut app, ui));
    settle(&mut ui, |ui| menus(&mut app, ui));
    let (list, tiles) = (told(&ui, "Show as list"), told(&ui, "Show as tiles"));
    assert_eq!((list.role, list.on, tiles.role, tiles.on, tiles.keys.as_str()), (Role::MenuChoice, Some(true), Role::MenuChoice, Some(false), "Ctrl+Shift+S"));
    // Told without the marks of their letters: "&&" is a "&".
    assert_eq!((told(&ui, "Show all").role, told(&ui, "Show & hide").role, told(&ui, "File").role), (Role::MenuItem, Role::MenuItem, Role::Fold));
    click(&mut ui, middle(app.rows[3]), |ui| menus(&mut app, ui));
    assert_eq!((app.view, app.file, app.picked.as_slice()), (1, false, &["tiles"][..]));
}

#[test]
fn letters_pick_menus_and_rows_and_are_underlined_while_alt_is_held() {
    let (mut app, mut ui) = (Menus::default(), window(1.0));
    settle(&mut ui, |ui| menus(&mut app, ui));
    let plain = ui.screen.pixels.clone();
    // Held, Alt underlines the letters of the bar: under the F and the E, and nowhere else.
    does(&mut ui, Event::Held(ALT), |ui| menus(&mut app, ui));
    let differs: Vec<usize> = (0..plain.len()).filter(|at| plain[*at] != ui.screen.pixels[*at]).collect();
    let (wide, rows) = (ui.screen.width, |at: &usize| at / ui.screen.width);
    assert!(!differs.is_empty() && differs.iter().all(|at| rows(at) > 14 && rows(at) < 26), "under the letters");
    let columns: Vec<usize> = differs.iter().map(|at| at % wide).collect();
    assert!(columns.iter().all(|x| (13..25).contains(x) || (81..93).contains(x)), "{:?}", (columns.iter().min(), columns.iter().max()));
    // Let go after a key was pressed with it (no Alt alone, which would take the keyboard to
    // the bar), the underlines go.
    key(&mut ui, Key::Char('x'), ALT, |ui| menus(&mut app, ui));
    does(&mut ui, Event::Held(Mods::NONE), |ui| menus(&mut app, ui));
    assert!(ui.screen.pixels == plain, "and away with them");

    // Alt with a letter opens its menu; there, the letter alone picks a row.
    does(&mut ui, Event::Held(ALT), |ui| menus(&mut app, ui));
    key(&mut ui, Key::Char('e'), ALT, |ui| menus(&mut app, ui));
    does(&mut ui, Event::Held(Mods::NONE), |ui| menus(&mut app, ui));
    assert!(app.edit && !app.file);
    key(&mut ui, Key::Char('f'), ALT, |ui| menus(&mut app, ui));
    assert!(app.file && !app.edit, "Alt with another letter opens that menu instead");
    key(&mut ui, Key::Char('t'), Mods::NONE, |ui| menus(&mut app, ui));
    assert_eq!((app.view, app.file), (1, false));
    key(&mut ui, Key::Char('f'), ALT, |ui| menus(&mut app, ui));
    key(&mut ui, Key::Char('h'), Mods::NONE, |ui| menus(&mut app, ui));
    assert_eq!(app.picked, ["tiles", "hide"]);
    // A letter no row has does nothing; one with no menu open, nothing either.
    key(&mut ui, Key::Char('f'), ALT, |ui| menus(&mut app, ui));
    key(&mut ui, Key::Char('q'), Mods::NONE, |ui| menus(&mut app, ui));
    assert!(app.file);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| menus(&mut app, ui));
    key(&mut ui, Key::Char('e'), Mods::NONE, |ui| menus(&mut app, ui));
    assert!(!app.file && !app.edit);
}

#[test]
fn alt_alone_puts_the_keyboard_on_the_bar_and_f10_opens_the_first_menu() {
    let (mut app, mut ui) = (Menus::default(), window(1.0));
    settle(&mut ui, |ui| menus(&mut app, ui));
    // The keyboard is on a button elsewhere. Alt pressed and let go takes it to the bar.
    click(&mut ui, middle(app.other), |ui| menus(&mut app, ui));
    let elsewhere = ui.focus();
    assert_ne!(elsewhere, 0);
    for held in [ALT, Mods::NONE] {
        does(&mut ui, Event::Held(held), |ui| menus(&mut app, ui));
    }
    ui.run(&mut |ui| menus(&mut app, ui));
    let on_file = ui.focus();
    assert!(on_file != elsewhere && !app.file);
    // There a letter alone opens a menu, Right goes to the next, and Down opens it with its
    // first row under the keyboard.
    key(&mut ui, Key::Right, Mods::NONE, |ui| menus(&mut app, ui));
    key(&mut ui, Key::Down, Mods::NONE, |ui| menus(&mut app, ui));
    ui.run(&mut |ui| menus(&mut app, ui));
    assert!(app.edit);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| menus(&mut app, ui));
    assert!(!app.edit, "Enter picked its first row, Undo");
    // Escape out of a menu goes back to the bar, and from the bar back to where it was.
    key(&mut ui, Key::Char('f'), ALT, |ui| menus(&mut app, ui));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| menus(&mut app, ui));
    assert!(!app.file);
    assert!(ui.focus() != elsewhere && ui.focus() != 0, "on the bar");
    // Alt alone again, while the keyboard is on the bar, takes it back where it was.
    for held in [ALT, Mods::NONE] {
        does(&mut ui, Event::Held(held), |ui| menus(&mut app, ui));
    }
    ui.run(&mut |ui| menus(&mut app, ui));
    assert_eq!(ui.focus(), elsewhere);
    // (Alt with anything pressed meanwhile is no Alt alone.)
    for event in [Event::Held(ALT), Event::Key(Key::Char('x'), ALT), Event::Held(Mods::NONE)] {
        does(&mut ui, event, |ui| menus(&mut app, ui));
    }
    assert_eq!(ui.focus(), elsewhere);
    // F10 opens the first menu, with its first row under the keyboard.
    key(&mut ui, Key::F(10), Mods::NONE, |ui| menus(&mut app, ui));
    ui.run(&mut |ui| menus(&mut app, ui));
    assert!(app.file);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| menus(&mut app, ui));
    assert_eq!(app.picked, ["all"]);
    let _ = on_file;
}

#[derive(Default)]
struct Rows {
    open: Option<usize>,
    picked: Vec<usize>,
    rows: [Rect; 3],
    first: Rect,
}

fn rows(app: &mut Rows, ui: &mut Ui) {
    app.open = None;
    ui.column().pad(10.0).open();
    for number in 0..3 {
        let row = ui.selectable(false, &format!("Row {number}"));
        app.rows[number] = row.rect;
        if ui.context_menu(&row) {
            app.open = Some(number);
            app.first = ui.menu_item("Open it").rect;
            if ui.menu_item("Rename it").clicked {
                app.picked.push(number);
            }
            ui.close();
        }
    }
    ui.close();
}

#[test]
fn shift_f10_and_the_menu_key_open_the_menu_of_what_the_keyboard_is_on() {
    let (mut app, mut ui) = (Rows::default(), window(1.0));
    settle(&mut ui, |ui| rows(&mut app, ui));
    // Nothing happens while the keyboard is on nothing.
    key(&mut ui, Key::F(10), Mods::SHIFT, |ui| rows(&mut app, ui));
    assert_eq!(app.open, None);
    for _ in 0..2 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| rows(&mut app, ui));
    }
    // Under the row the keyboard is on, with its first row under the keyboard.
    key(&mut ui, Key::F(10), Mods::SHIFT, |ui| rows(&mut app, ui));
    ui.run(&mut |ui| rows(&mut app, ui));
    assert!(app.open == Some(1) && app.first.y >= app.rows[1].bottom() && app.first.x >= app.rows[1].x, "{:?} under {:?}", app.first, app.rows[1]);
    key(&mut ui, Key::Down, Mods::NONE, |ui| rows(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| rows(&mut app, ui));
    assert_eq!((app.open, app.picked.as_slice()), (None, &[1][..]));
    // The menu key does the same, for the next row.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| rows(&mut app, ui));
    key(&mut ui, Key::Menu, Mods::NONE, |ui| rows(&mut app, ui));
    assert_eq!(app.open, Some(2));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| rows(&mut app, ui));
    assert_eq!(app.open, None);
}

#[derive(Default)]
struct Far {
    button: Rect,
    items: Vec<Rect>,
    open: bool,
    context: bool,
}

/// A button at the far right of the window with a menu of forty rows, and room under it
/// that has a menu at the pointer.
fn far(app: &mut Far, ui: &mut Ui) {
    (app.open, app.context) = (false, false);
    app.items.clear();
    ui.row().pad(4.0).open();
    ui.spacer();
    let button = ui.button("Far");
    app.button = button.rect;
    if button.clicked {
        ui.open_popup(button.id, button.rect);
    }
    if ui.menu(button.id) {
        app.open = true;
        for number in 0..40 {
            app.items.push(ui.menu_item(&format!("Item {number}")).rect);
        }
        ui.close();
    }
    ui.close();
    let room = ui.el().grow().sense().show();
    if ui.context_menu(&room) {
        app.context = true;
        ui.menu_item("Here");
        ui.menu_item("There");
        ui.close();
    }
}

/// Where the menu that is open lies, as a screen reader is told.
fn menu_rect(ui: &Window) -> Rect {
    ui.said().iter().rev().find(|said| said.role == Role::Menu).map(|said| said.rect).expect("a menu")
}

#[test]
fn a_menu_that_does_not_fit_goes_left_or_up_and_a_long_one_scrolls() {
    let (mut app, mut ui) = (Far::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| far(&mut app, ui));
    click(&mut ui, middle(app.button), |ui| far(&mut app, ui));
    settle(&mut ui, |ui| far(&mut app, ui));
    // Wider than its button and at the window's right edge: their right edges in line.
    let menu = menu_rect(&ui);
    assert!(app.open && menu.right() == app.button.right() && menu.w > app.button.w, "{menu:?} {:?}", app.button);
    // Higher than the window: as high as it, a little in from its edges, and scrolled.
    assert!(menu.y >= 4.0 && menu.bottom() <= 296.0 && menu.h >= 280.0, "{menu:?}");
    assert!(app.items[39].is_empty(), "the last row is out of view");
    // (The keyboard is on its first row as it opens: Down goes on from there, End to the last.)
    key(&mut ui, Key::Down, Mods::NONE, |ui| far(&mut app, ui));
    key(&mut ui, Key::End, Mods::NONE, |ui| far(&mut app, ui));
    ui.run(&mut |ui| far(&mut app, ui));
    assert!(app.items[39].bottom() <= menu.bottom() && app.items[39].y >= menu.y, "the keyboard took it into view: {:?}", app.items[39]);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| far(&mut app, ui));
    // At the pointer near the bottom right corner: to the left of it, and over it.
    for event in [Event::Move(pt(410.0, 290.0)), Event::Press(taut::Button::Right, Mods::NONE), Event::Release(taut::Button::Right)] {
        does(&mut ui, event, |ui| far(&mut app, ui));
    }
    settle(&mut ui, |ui| far(&mut app, ui));
    let menu = menu_rect(&ui);
    assert!(app.context && menu.right() <= 410.0 && menu.bottom() <= 290.0 && menu.x > 100.0, "{menu:?}");
}

#[derive(Default)]
struct Tools {
    pressed: Vec<usize>,
    bold: bool,
    shown: [Rect; 6],
}

fn tools(app: &mut Tools, ui: &mut Ui) {
    ui.tool_bar();
    for number in 0..6 {
        if number == 3 {
            ui.tool_group();
        }
        let hint = format!("Tool {number}\tCtrl+{number}");
        let tool = if number == 4 { ui.tool_toggle(&mut app.bold, STAR, "Bold\tCtrl+B") } else { ui.tool(STAR, &hint) };
        app.shown[number] = tool.rect;
        if tool.clicked {
            app.pressed.push(number);
        }
    }
    ui.close();
}

#[test]
fn a_tool_bar_puts_what_does_not_fit_into_a_menu_at_its_end() {
    let (mut app, mut ui) = (Tools::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| tools(&mut app, ui));
    // All of them fit: side by side, with a line between the groups, and no button for more.
    assert!(app.shown.iter().all(|shown| !shown.is_empty() && shown.w == 28.0), "{:?}", app.shown);
    assert_eq!(app.shown.map(|shown| shown.x), [4.0, 34.0, 64.0, 105.0, 135.0, 165.0]);
    assert!(ui.said().iter().all(|said| said.name != "More"));
    let bar = ui.said().iter().find(|said| said.role == Role::ToolBar).expect("told as a tool bar");
    let first = told(&ui, "Tool 0");
    assert_eq!((first.role, first.keys.as_str(), first.within), (Role::Button, "Ctrl+0", ui.said().iter().position(|said| said == bar)));
    assert_eq!((told(&ui, "Bold").role, told(&ui, "Bold").on), (Role::Button, Some(false)));
    // The hint shows when the pointer rests on a tool, with its keys.
    let under = rect(4.0, 40.0, 120.0, 30.0);
    assert!(ink(&ui, under).is_none());
    does(&mut ui, Event::Move(middle(app.shown[0])), |ui| tools(&mut app, ui));
    ui.set_time(Duration::from_millis(600));
    settle(&mut ui, |ui| tools(&mut app, ui));
    assert!(ink(&ui, under).is_some(), "a tool tip");
    does(&mut ui, Event::Leave, |ui| tools(&mut app, ui));

    // In a narrow window, what does not fit goes behind the button at the end: and the
    // line before the second group goes with it.
    ui.resize(Size { w: 150.0, h: 300.0 }, 1.0);
    settle(&mut ui, |ui| tools(&mut app, ui));
    assert_eq!(app.shown.map(|shown| shown.is_empty()), [false, false, false, true, true, true]);
    let more = told(&ui, "More").rect;
    assert_eq!((more.x, more.w), (94.0, 28.0));
    click(&mut ui, middle(more), |ui| tools(&mut app, ui));
    settle(&mut ui, |ui| tools(&mut app, ui));
    let rows: Vec<(String, Option<bool>, String)> = ui.said().iter().filter(|said| said.role == Role::MenuItem).map(|said| (said.name.clone(), said.on, said.keys.clone())).collect();
    let tool = |name: &str, on, keys: &str| (name.to_string(), on, keys.to_string());
    assert_eq!(rows, [tool("Tool 3", None, "Ctrl+3"), tool("Bold", Some(false), "Ctrl+B"), tool("Tool 5", None, "Ctrl+5")]);
    // Picked there, a tool is pressed; one that stays down is down from then on.
    let bold = middle(told(&ui, "Bold").rect);
    click(&mut ui, bold, |ui| tools(&mut app, ui));
    assert!(app.bold && app.pressed == [4]);
    click(&mut ui, middle(more), |ui| tools(&mut app, ui));
    settle(&mut ui, |ui| tools(&mut app, ui));
    assert_eq!(told(&ui, "Bold").on, Some(true));
    key(&mut ui, Key::End, Mods::NONE, |ui| tools(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| tools(&mut app, ui));
    assert_eq!(app.pressed, [4, 5], "the last row, with the keys");

    // Wide again: all of them are back in the bar.
    ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
    settle(&mut ui, |ui| tools(&mut app, ui));
    assert!(app.shown.iter().all(|shown| !shown.is_empty()) && ui.said().iter().all(|said| said.name != "More"));
}

#[derive(Default)]
struct Pop {
    /// Where the button is: at the top left of the window, or at its bottom right.
    far: bool,
    open: bool,
    button: Rect,
    inside: Rect,
}

fn pop(app: &mut Pop, ui: &mut Ui) {
    app.open = false;
    let corner = if app.far { taut::Align::End } else { taut::Align::Start };
    ui.pin(corner, corner);
    ui.row().pad(10.0).open();
    let button = ui.button("Tint");
    app.button = button.rect;
    if button.clicked {
        ui.open_popup(button.id, button.rect);
    }
    if ui.popover(button.id) {
        app.open = true;
        ui.label("A box of anything at all");
        app.inside = ui.button("Inside").rect;
        ui.close();
    }
    ui.close();
}

/// Where the popover that is open lies, as a screen reader is told.
fn popover_rect(ui: &Window) -> Rect {
    ui.said().iter().find(|said| said.role == Role::Group).map(|said| said.rect).expect("a popover")
}

#[test]
fn a_popover_points_at_its_box_and_stays_inside_the_window() {
    let (mut app, mut ui) = (Pop::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| pop(&mut app, ui));
    click(&mut ui, middle(app.button), |ui| pop(&mut app, ui));
    ui.run(&mut |ui| pop(&mut app, ui));
    // Under its button, a little away from it, and kept off the window's left edge.
    let lies = popover_rect(&ui);
    assert!(app.open && lies.y == app.button.bottom() + 8.0 && lies.x == 4.0 && lies.w > app.button.w, "{lies:?} {:?}", app.button);
    // The arrow between them points at the middle of the button: it stands out of the
    // shadow along the popover's edge there, and only there.
    let (middle_x, y) = (app.button.x + app.button.w / 2.0, app.button.bottom() + 6.0);
    let at = |x: f32| ui.screen.pixels[y as usize * ui.screen.width + x as usize];
    assert!(at(middle_x) != at(middle_x + 20.0) && at(middle_x - 20.0) == at(middle_x + 20.0), "an arrow at {middle_x}");
    // What is in it is used as anything else; a press beside it puts it away.
    click(&mut ui, middle(app.inside), |ui| pop(&mut app, ui));
    assert!(app.open);
    click(&mut ui, pt(300.0, 200.0), |ui| pop(&mut app, ui));
    assert!(!app.open);

    // At the bottom right of the window it stands over its button, and inside the window.
    app.far = true;
    settle(&mut ui, |ui| pop(&mut app, ui));
    click(&mut ui, middle(app.button), |ui| pop(&mut app, ui));
    ui.run(&mut |ui| pop(&mut app, ui));
    let lies = popover_rect(&ui);
    assert!(lies.bottom() == app.button.y - 8.0 && lies.right() <= 416.0 && lies.right() >= app.button.right() - 4.0, "{lies:?} {:?}", app.button);
    let (middle_x, y) = (app.button.x + app.button.w / 2.0, app.button.y - 6.0);
    let at = |x: f32| ui.screen.pixels[y as usize * ui.screen.width + x as usize];
    assert!(at(middle_x) != at(middle_x - 20.0), "its arrow points down at the button");
    key(&mut ui, Key::Escape, Mods::NONE, |ui| pop(&mut app, ui));
    assert!(!app.open);
}

struct Bench {
    dock: taut::Dock,
    shown: Vec<&'static str>,
    inside: [Rect; 4],
}

const PANELS: [&str; 4] = ["Files", "Editor", "Notes", "Log"];

/// A dock of four panels, of which the last two may be closed.
fn bench(app: &mut Bench, ui: &mut Ui) {
    app.shown.clear();
    ui.dock(&mut app.dock);
    for (number, name) in PANELS.into_iter().enumerate() {
        let shows = if number >= 2 { ui.docked_closable(&mut app.dock, name) } else { ui.docked(&mut app.dock, name) };
        if shows {
            app.shown.push(name);
            app.inside[number] = ui.button(&format!("In {name}")).rect;
            ui.close();
        }
    }
    ui.close();
}

fn bench_of() -> (Bench, Window) {
    let dock = taut::Dock::of(&["Files"]).beside(taut::Dock::of(&["Editor", "Notes", "Log"]), 0.25);
    let (mut app, mut ui) = (Bench { dock, shown: Vec::new(), inside: [Rect::EMPTY; 4] }, window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| bench(&mut app, ui));
    (app, ui)
}

/// The names of the tabs of the place the panel `name` is in, as a screen reader is told.
fn tabs_told(ui: &Window, name: &str) -> Vec<(String, Option<[u32; 2]>)> {
    let said = ui.said();
    let tab = said.iter().position(|said| said.role == Role::Tab && said.name == name).expect("its tab");
    said.iter().filter(|other| other.role == Role::Tab && other.within == said[tab].within).map(|tab| (tab.name.clone(), tab.of)).collect()
}

#[test]
fn tabs_of_a_dock_go_in_another_order_close_and_float_and_come_back() {
    let (mut app, mut ui) = bench_of();
    let place = |name: &str, nth: u32| (name.to_string(), Some([nth, 3]));
    assert_eq!(tabs_told(&ui, "Editor"), [place("Editor", 1), place("Notes", 2), place("Log", 3)]);
    assert_eq!(app.shown, ["Files", "Editor"]);
    // The tab of Log, carried left along its strip past the middle of Notes, goes before it;
    // and Editor stays in front.
    let log = told(&ui, "Log").rect;
    for event in [Event::Move(middle(log)), Event::Press(taut::Button::Left, Mods::NONE), Event::Move(pt(240.0, 16.0)), Event::Release(taut::Button::Left)] {
        does(&mut ui, event, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!(tabs_told(&ui, "Editor"), [place("Editor", 1), place("Log", 2), place("Notes", 3)]);
    assert!(app.dock.floating.is_empty() && app.shown == ["Files", "Editor"], "it carried no panel away");

    // The cross on a tab closes its panel, which shows nowhere until the app opens it again.
    let cross = ui.said().iter().find(|said| said.name == "Close" && said.rect.x > told(&ui, "Notes").rect.x).expect("Notes' cross").rect;
    assert_eq!(told(&ui, "Close").keys, "Ctrl+F4");
    click(&mut ui, middle(cross), |ui| bench(&mut app, ui));
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!((app.dock.closed.as_slice(), tabs_told(&ui, "Editor").len()), (&["Notes".to_string()][..], 2));
    app.dock.set_open("Notes", true);
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!((app.dock.is_open("Notes"), app.shown.as_slice()), (true, &["Files", "Notes"][..]), "back, in front");
    // The middle button closes one too; a panel the app does not let be closed has no cross.
    let log = told(&ui, "Log").rect;
    for event in [Event::Move(middle(log)), Event::Press(taut::Button::Middle, Mods::NONE), Event::Release(taut::Button::Middle)] {
        does(&mut ui, event, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!(app.dock.closed, ["Log"]);
    assert_eq!(ui.said().iter().filter(|said| said.name == "Close").count(), 1, "Notes' cross alone");

    // A double press on a tab has its panel float; one on the title of the panel, and it is
    // back where it was.
    let editor = middle(told(&ui, "Editor").rect);
    for time in [5000, 5100] {
        ui.set_time(Duration::from_millis(time));
        click(&mut ui, editor, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!(app.dock.floating.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["Editor"]);
    assert!(app.shown.contains(&"Editor") && app.shown.contains(&"Notes"), "{:?}", app.shown);
    let title = told(&ui, "Editor").rect;
    for time in [7000, 7100] {
        ui.set_time(Duration::from_millis(time));
        click(&mut ui, pt(title.x + title.w / 2.0, title.y + 14.0), |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert!(app.dock.floating.is_empty());
    assert_eq!(tabs_told(&ui, "Editor").iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["Notes", "Editor"]);
    assert_eq!(app.shown, ["Files", "Editor"]);
}

#[test]
fn a_place_fills_the_dock_and_the_keyboard_goes_from_place_to_place() {
    let (mut app, mut ui) = bench_of();
    // The button at the end of the strip of tabs has its place fill the dock, and back.
    let fills = |ui: &Window| ui.said().iter().filter(|said| said.name == "Fill the dock").map(|said| said.rect).collect::<Vec<_>>();
    let [_, right] = fills(&ui)[..] else { panic!("a button at the end of each strip") };
    click(&mut ui, middle(right), |ui| bench(&mut app, ui));
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!((app.dock.filled.as_deref(), app.shown.as_slice(), app.inside[1].x), (Some("Editor"), &["Editor"][..], 10.0));
    let back = told(&ui, "Back to its place").rect;
    click(&mut ui, middle(back), |ui| bench(&mut app, ui));
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!((app.dock.filled.as_deref(), app.shown.as_slice()), (None, &["Files", "Editor"][..]));

    // Control+F6 goes into the first place from outside, and into the next from there.
    let ctrl_shift = Mods { ctrl: true, shift: true, alt: false };
    let on = |ui: &Window| ui.said().iter().find(|said| said.focused).map(|said| said.name.clone()).unwrap_or_default();
    key(&mut ui, Key::F(6), Mods::CTRL, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(on(&ui), "In Files");
    key(&mut ui, Key::F(6), Mods::CTRL, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(on(&ui), "In Editor");
    // Control+Tab brings the next panel of the place to the front and goes into it; with
    // Shift, the one before.
    key(&mut ui, Key::Tab, Mods::CTRL, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!((on(&ui).as_str(), app.shown.as_slice()), ("In Notes", &["Files", "Notes"][..]));
    key(&mut ui, Key::Tab, ctrl_shift, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(on(&ui), "In Editor");
    key(&mut ui, Key::F(6), ctrl_shift, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(on(&ui), "In Files");
    // Control+F4 closes the panel the keyboard is in, where the app lets it be closed.
    key(&mut ui, Key::F(4), Mods::CTRL, |ui| bench(&mut app, ui));
    assert!(app.dock.closed.is_empty());
    key(&mut ui, Key::F(6), Mods::CTRL, |ui| bench(&mut app, ui));
    key(&mut ui, Key::Tab, Mods::CTRL, |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    key(&mut ui, Key::F(4), Mods::CTRL, |ui| bench(&mut app, ui));
    settle(&mut ui, |ui| bench(&mut app, ui));
    // (The tab after it comes to the front, as in other programs.)
    assert_eq!((app.dock.closed.as_slice(), app.shown.as_slice()), (&["Notes".to_string()][..], &["Files", "Log"][..]));
}

struct Desk {
    a: taut::Panel,
    b: taut::Panel,
}

fn desk(app: &mut Desk, ui: &mut Ui) {
    for (panel, title) in [(&mut app.a, "A"), (&mut app.b, "B")] {
        if ui.panel(panel, title) {
            ui.label(title);
            ui.close();
        }
    }
}

#[test]
fn panels_lie_against_edges_and_stay_inside_a_window_that_gets_smaller() {
    let (mut app, mut ui) = (Desk { a: taut::Panel::new(50.0, 50.0, 150.0, 100.0), b: taut::Panel::new(250.0, 50.0, 150.0, 100.0) }, window(1.0));
    settle(&mut ui, |ui| desk(&mut app, ui));
    // Taken by its title to within a few units of the window's edge, A lies against it.
    drag(&mut ui, pt(120.0, 64.0), pt(76.0, 74.0), |ui| desk(&mut app, ui));
    assert_eq!((app.a.rect.x, app.a.rect.y), (0.0, 60.0));
    // To within a few units of B, its right edge lies against B's left, and its top in line.
    drag(&mut ui, pt(60.0, 74.0), pt(154.0, 67.0), |ui| desk(&mut app, ui));
    assert_eq!((app.a.rect.x, app.a.rect.y), (100.0, 50.0));
    // Stretched by its bottom edge to near the window's, it reaches that.
    drag(&mut ui, pt(150.0, 152.0), pt(150.0, 297.0), |ui| desk(&mut app, ui));
    assert_eq!(app.a.rect.bottom(), 300.0);
    // The window gets narrower and lower: B, which lay inside it, is kept inside; A, as high
    // as the window was, is made as high as it is now.
    ui.resize(Size { w: 330.0, h: 240.0 }, 1.0);
    settle(&mut ui, |ui| desk(&mut app, ui));
    assert_eq!(app.b.rect, rect(180.0, 50.0, 150.0, 100.0));
    assert_eq!(app.a.rect, rect(100.0, 0.0, 150.0, 240.0));
}

#[test]
fn a_notice_that_is_pressed_goes_though_one_before_it_went_meanwhile() {
    let (mut ui, mut said) = (window(1.0), false);
    ui.listen(true);
    ui.set_time(Duration::from_secs(10));
    let mut build = |ui: &mut Ui| {
        if !std::mem::replace(&mut said, true) {
            ui.notice_for("one", Duration::from_secs(1));
            ui.notice("two");
            ui.notice("three");
        }
    };
    settle(&mut ui, &mut build);
    let two = told(&ui, "two").rect;
    for event in [Event::Move(middle(two)), Event::Press(taut::Button::Left, Mods::NONE)] {
        does(&mut ui, event, &mut build);
    }
    // While the button is down, the first goes by itself; the one pressed is let go of after.
    ui.set_time(Duration::from_millis(11_500));
    ui.run(&mut build);
    assert!(ui.said().iter().all(|said| said.name != "one") && told(&ui, "two").rect == two);
    does(&mut ui, Event::Release(taut::Button::Left), &mut build);
    settle(&mut ui, &mut build);
    let left: Vec<&str> = ui.said().iter().map(|said| said.name.as_str()).collect();
    assert_eq!(left, ["three"]);
}
