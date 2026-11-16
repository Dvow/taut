//! The library as an app uses it, without a window: an interface is described, clicked, typed
//! into, scrolled, and rendered to memory.
mod common;

use common::*;
use std::time::Duration;
use taut::geom::{pt, rect};
use taut::{Align, Button, Color, Event, Flow, Icon, Key, Length, Mods, Path, Rect, Role, Surface, Ui};

const PLUS: Icon = Icon::line("M8 3.5v9M3.5 8h9");
const SPARK: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");

#[derive(Default)]
struct App {
    saves: u32,
    every: bool,
    button: Rect,
    tick: Rect,
}

/// A small window of a common shape: a bar across the top, and a list under it.
fn describe(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad((10.0, 8.0)).gap(8.0).fill(t.panel).open();
    ui.icon_button(PLUS, "Add notes");
    ui.label(&format!("{} notes saved", app.saves));
    ui.spacer();
    let b = ui.primary_button("Save all");
    (app.button, app.saves) = (b.rect, app.saves + b.clicked as u32);
    ui.close();
    ui.separator();
    ui.column().grow().scroll().pad(10.0).gap(6.0).open();
    app.tick = ui.checkbox(&mut app.every, "With nothing picked, Save takes every note").rect;
    for i in 0..40 {
        ui.row().gap(8.0).pad((8.0, 6.0)).fill(t.panel).radius(6.0).open();
        ui.icon(SPARK);
        ui.label(&format!("note {i}"));
        ui.spacer();
        ui.note("today");
        ui.close();
    }
    ui.paragraph("Every note you add gets a row here: when it was written, how long it is, and a button that opens it.");
    ui.close();
}

#[test]
fn a_click_reaches_the_app_and_only_what_changed_is_drawn_again() {
    let (mut app, mut ui) = (App::default(), window(1.0));
    let first = ui.run(&mut |ui| describe(&mut app, ui));
    assert_eq!(first, Rect { x: 0.0, y: 0.0, w: 420.0, h: 300.0 }, "the first time, the whole window is drawn");
    assert_eq!(ui.run(&mut |ui| describe(&mut app, ui)), Rect::EMPTY, "run again with nothing new: nothing to draw");

    assert!(app.button.w > 0.0 && app.button.right() == 410.0, "the button is at the far end of the bar: {:?}", app.button);
    let button = app.button;
    click(&mut ui, middle(button), |ui| describe(&mut app, ui));
    assert_eq!(app.saves, 1, "the click was seen once");
    click(&mut ui, middle(app.tick), |ui| describe(&mut app, ui));
    assert!(app.every, "the tick box was ticked");

    // Moving the pointer away changes one button's colour: only that part is drawn again.
    ui.event(Event::Move(pt(app.button.x + 5.0, app.button.y + 5.0)));
    ui.run(&mut |ui| describe(&mut app, ui));
    ui.event(Event::Move(pt(200.0, 150.0)));
    let changed = ui.run(&mut |ui| describe(&mut app, ui));
    assert_eq!(changed, button, "only the button is drawn again");
}

#[test]
fn the_page_takes_the_colour_of_the_theme_at_once() {
    // The app sets the theme while it describes the window: the page under everything is
    // in that theme's colour in the very run that set it, not one run later.
    let (mut ui, mut light) = (window(1.0), false);
    let build = |ui: &mut Ui, light: bool| {
        ui.theme = if light { taut::Theme::light() } else { taut::Theme::dark() };
        ui.label("words");
    };
    settle(&mut ui, |ui| build(ui, light));
    assert_eq!(ui.screen.pixels[299 * 420], taut::Theme::dark().bg.0);
    light = true;
    ui.run(&mut |ui| build(ui, light));
    assert_eq!(ui.screen.pixels[299 * 420], taut::Theme::light().bg.0, "light after one run");
}

#[test]
fn a_box_among_boxes_on_top_of_each_other_is_placed_across_and_down() {
    let mut ui = window(1.0);
    let mut found = [Rect::EMPTY; 5];
    let spots: [taut::layout::Spot; 5] = [Align::End.into(), (Align::End, Align::Center).into(), (Align::Start, Align::End).into(), Align::Center.into(), (Align::Center, Align::Start).into()];
    settle(&mut ui, |ui| {
        ui.el().flow(Flow::Layers).size(200.0, 100.0).open();
        for (spot, found) in spots.iter().zip(&mut found) {
            *found = ui.el().size(40.0, 20.0).place(*spot).sense().show().rect;
        }
        ui.close();
    });
    // The far corner; the right end, in the middle of the height; the left end, at the
    // bottom; the middle; the middle across, at the top.
    assert_eq!(found, [rect(160.0, 80.0, 40.0, 20.0), rect(160.0, 40.0, 40.0, 20.0), rect(0.0, 80.0, 40.0, 20.0), rect(80.0, 40.0, 40.0, 20.0), rect(80.0, 0.0, 40.0, 20.0)]);
    // In a row, a pair's second word is the one that counts: down is across the row.
    let mut low = Rect::EMPTY;
    settle(&mut ui, |ui| {
        ui.row().size(200.0, 100.0).open();
        low = ui.el().size(40.0, 20.0).place((Align::Start, Align::End)).sense().show().rect;
        ui.close();
    });
    assert_eq!(low, rect(0.0, 80.0, 40.0, 20.0));
}

#[test]
fn a_box_alone_in_its_window_hears_of_the_pointer_like_any_other() {
    // One box that senses the pointer, and nothing else that asks about anything: what it
    // is told is as of this run, not of the one before.
    let (mut ui, mut told) = (window(1.0), (Rect::EMPTY, false));
    let build = |ui: &mut Ui, told: &mut (Rect, bool)| {
        let only = ui.el().size(50.0, 40.0).sense().show();
        *told = (only.rect, only.hovered);
    };
    settle(&mut ui, |ui| build(ui, &mut told));
    assert_eq!(told, (rect(0.0, 0.0, 50.0, 40.0), false));
    does(&mut ui, Event::Move(pt(10.0, 10.0)), |ui| build(ui, &mut told));
    assert!(told.1, "the pointer is over it");
    does(&mut ui, Event::Move(pt(100.0, 10.0)), |ui| build(ui, &mut told));
    assert!(!told.1, "and has left it");
}

#[test]
fn a_screen_reader_is_told_what_is_in_the_window_and_works_it() {
    #[derive(Default)]
    struct Form {
        loud: bool,
        level: f32,
        name: String,
        saved: u32,
    }
    let form = |form: &mut Form, ui: &mut Ui| {
        ui.label("Sound");
        ui.checkbox(&mut form.loud, "Loud");
        ui.slider(&mut form.level, 0.0..=10.0);
        ui.text_box(&mut form.name, "Your name");
        if ui.button("Save").clicked {
            form.saved += 1;
            ui.announce("Saved");
        }
        ui.icon_button(PLUS, "Add one");
    };
    let (mut ui, mut app) = (window(1.0), Form { level: 4.0, ..Default::default() });
    // Nobody listens: nothing is kept.
    settle(&mut ui, |ui| form(&mut app, ui));
    assert!(ui.said().is_empty());

    // Somebody does: every thing in the window, with what it is, what it is called and how
    // it stands. (The text in a tick box is its name, not a thing of its own.)
    ui.listen(true);
    settle(&mut ui, |ui| form(&mut app, ui));
    let told = |ui: &Window| ui.said().iter().map(|said| (said.role, said.name.clone(), said.value.clone(), said.on, said.amount)).collect::<Vec<_>>();
    let none = String::new;
    let expected = vec![
        (Role::Text, "Sound".to_string(), none(), None, None),
        (Role::Tick, "Loud".to_string(), none(), Some(false), None),
        (Role::Slider, none(), none(), None, Some([4.0, 0.0, 10.0])),
        (Role::TextBox, "Your name".to_string(), none(), None, None),
        (Role::Button, "Save".to_string(), none(), None, None),
        (Role::Button, "Add one".to_string(), none(), None, None),
    ];
    assert_eq!(told(&ui), expected);
    assert!(ui.said().iter().all(|said| said.usable && !said.focused && said.rect.w > 0.0 && said.within.is_none()));

    // It presses the tick box, moves the slider on, types a name and presses the button,
    // each known by its id; and is told how things stand after.
    let ids: Vec<u64> = ui.said().iter().map(|said| said.id).collect();
    ui.press_for_reader(ids[1]);
    ui.run(&mut |ui| form(&mut app, ui));
    ui.step_for_reader(ids[2], true);
    ui.run(&mut |ui| form(&mut app, ui));
    ui.type_for_reader(ids[3], "Ada");
    ui.run(&mut |ui| form(&mut app, ui));
    ui.press_for_reader(ids[4]);
    ui.run(&mut |ui| form(&mut app, ui));
    assert_eq!((app.loud, app.level, app.name.as_str(), app.saved), (true, 4.5, "Ada", 1));
    // What the app had said about the press is there to be said after that run, and gone
    // after the next.
    assert_eq!(ui.announced(), ["Saved"]);
    ui.run(&mut |ui| form(&mut app, ui));
    assert!(ui.announced().is_empty());
    let now = told(&ui);
    assert_eq!((now[1].3, now[2].4, now[3].2.as_str()), (Some(true), Some([4.5, 0.0, 10.0]), "Ada"));
    assert!(ui.said()[4].focused, "the keyboard is on what was pressed last");
    assert_eq!(ui.said().iter().map(|said| said.id).collect::<Vec<_>>(), ids, "each thing is known as before");
    // It has the slider stand where it says, in one go: also between the steps the keyboard
    // takes it by, and no further than its end. The slider says how far a step takes it.
    for (asked, stands) in [(7.2, 7.2), (99.0, 10.0)] {
        ui.set_for_reader(ids[2], asked);
        ui.run(&mut |ui| form(&mut app, ui));
        assert_eq!((app.level, ui.said()[2].amount, ui.said()[2].step), (stands, Some([stands, 0.0, 10.0]), 0.5));
    }
}

#[test]
fn the_wheel_scrolls_the_list_under_the_pointer_and_stops_at_its_end() {
    let (mut app, mut ui) = (App::default(), window(1.0));
    ui.run(&mut |ui| describe(&mut app, ui));
    let before = Surface::draw(&mut ui, 64).pixels;
    ui.event(Event::Move(pt(200.0, 200.0)));
    ui.event(Event::Wheel(0.0, 90.0));
    ui.run(&mut |ui| describe(&mut app, ui));
    let scrolled = Surface::draw(&mut ui, 64).pixels;
    assert_ne!(before, scrolled, "the list moved");
    ui.event(Event::Wheel(0.0, 1e6));
    ui.run(&mut |ui| describe(&mut app, ui));
    let end = Surface::draw(&mut ui, 64).pixels;
    ui.event(Event::Wheel(0.0, 500.0));
    assert_eq!(ui.run(&mut |ui| describe(&mut app, ui)), Rect::EMPTY, "at the end, more wheel changes nothing");
    assert_eq!(end, Surface::draw(&mut ui, 64).pixels);
}

#[test]
fn strips_of_any_height_draw_the_same_window() {
    for scale in [1.0, 1.5] {
        let (mut app, mut ui) = (App::default(), window(scale));
        ui.run(&mut |ui| describe(&mut app, ui));
        let whole = Surface::draw(&mut ui, 4096);
        for strip in [1, 7, 64] {
            assert!(Surface::draw(&mut ui, strip).pixels == whole.pixels, "strips of {strip} rows at scale {scale}");
        }
        // Kept where it can be looked at: target/tmp/window-<scale>.png
        let file = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("window-{scale}.png"));
        std::fs::write(&file, whole.png()).unwrap();
        assert_eq!(whole.width, (420.0 * scale) as usize);
    }
}

/// A form: a box for one line, a box for several, and what they said happened to them.
#[derive(Default)]
struct Form {
    name: String,
    note: String,
    boxes: [Rect; 2],
    changes: u32,
    entered: bool,
}
fn form(app: &mut Form, ui: &mut Ui) {
    ui.column().pad(10.0).gap(8.0).open();
    let name = ui.text_box(&mut app.name, "Name");
    let note = ui.text_area(&mut app.note, "Message", (2, 4));
    app.boxes = [name.rect, note.rect];
    app.changes += name.changed as u32 + note.changed as u32;
    // Asked for after the boxes: a key one of them used is gone by now.
    app.entered |= ui.key(Key::Enter, Mods::NONE);
    ui.close();
}

#[test]
fn text_is_typed_selected_copied_pasted_and_undone() {
    let (mut app, mut ui) = (Form { note: "fix the arc".into(), ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| form(&mut app, ui));
    click(&mut ui, middle(app.boxes[0]), |ui| form(&mut app, ui));
    typed(&mut ui, "hello", |ui| form(&mut app, ui));
    assert_eq!((app.name.as_str(), app.changes), ("hello", 1));
    key(&mut ui, Key::Home, Mods::SHIFT, |ui| form(&mut app, ui));
    typed(&mut ui, "J", |ui| form(&mut app, ui));
    assert_eq!(app.name, "J", "what is typed takes the place of what is selected");
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| form(&mut app, ui));
    key(&mut ui, Key::Char('c'), Mods::CTRL, |ui| form(&mut app, ui));
    assert_eq!(ui.clipboard().as_deref(), Some("J"));
    // The clipboard holds one thing at a time, of whatever kind: text, a picture, files.
    ui.copy_picture(2, &[0xff10_2030, 0x8040_5060]);
    assert_eq!((ui.clipboard_picture(), ui.clipboard(), ui.clipboard_files().len()), (Some((2, vec![0xff10_2030, 0x8040_5060])), None, 0));
    ui.copy_files(&["notes.txt".into()]);
    assert_eq!((ui.clipboard_files(), ui.clipboard_picture()), (vec![std::path::PathBuf::from("notes.txt")], None));
    ui.copy("J");
    assert!(ui.clipboard_files().is_empty());
    key(&mut ui, Key::Enter, Mods::NONE, |ui| form(&mut app, ui));
    assert!(app.entered, "a box of one line leaves Enter to the app");

    // Tab goes on to the next box, with the cursor at the end of what is in it.
    ui.event(Event::Key(Key::Tab, Mods::NONE));
    typed(&mut ui, "!", |ui| form(&mut app, ui));
    assert_eq!(app.note, "fix the arc!");
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| form(&mut app, ui));
    assert_eq!(app.note, "fix the arc");
    app.entered = false;
    key(&mut ui, Key::Enter, Mods::NONE, |ui| form(&mut app, ui));
    assert_eq!((app.note.as_str(), app.entered), ("fix the arc\n", false), "a box of several lines takes Enter as a line end");
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| form(&mut app, ui));

    // A press puts the cursor at the letter under the pointer: the text starts 8 in and 6 down.
    let at = pt(app.boxes[1].x + 8.0 + 3.0 * 12.0 + 2.0, app.boxes[1].y + 6.0 + 5.0);
    click(&mut ui, at, |ui| form(&mut app, ui));
    typed(&mut ui, "X", |ui| form(&mut app, ui));
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| form(&mut app, ui));
    assert_eq!(app.note, "fixXJ the arc");
    // Two presses close together take the whole word.
    ui.set_time(Duration::from_secs(5));
    click(&mut ui, at, |ui| form(&mut app, ui));
    ui.set_time(Duration::from_millis(5100));
    click(&mut ui, at, |ui| form(&mut app, ui));
    typed(&mut ui, "fix", |ui| form(&mut app, ui));
    assert_eq!(app.note, "fix the arc");

    // The cursor blinks: the box asks to be run again, and then draws a little differently.
    assert!(ui.wake_in().is_some_and(|wait| wait <= Duration::from_millis(530)));
    ui.set_time(Duration::from_millis(5100 + 600));
    let blinked = ui.run(&mut |ui| form(&mut app, ui));
    assert!(!blinked.is_empty() && blinked.w < 20.0, "only the cursor is drawn again: {blinked:?}");
    // And a click elsewhere takes the keyboard away.
    click(&mut ui, pt(400.0, 280.0), |ui| form(&mut app, ui));
    typed(&mut ui, "lost", |ui| form(&mut app, ui));
    assert_eq!((app.note.as_str(), ui.wake_in()), ("fix the arc", None));
}

#[test]
fn a_long_line_scrolls_to_keep_the_cursor_in_view() {
    let (mut app, mut ui) = (Form::default(), window(1.0));
    settle(&mut ui, |ui| form(&mut app, ui));
    click(&mut ui, middle(app.boxes[0]), |ui| form(&mut app, ui));
    let start = Surface::draw(&mut ui, 64).pixels;
    // The box is 400 wide: fifty letters of 12 do not fit in it.
    typed(&mut ui, &"abcde".repeat(10), |ui| form(&mut app, ui));
    let end = Surface::draw(&mut ui, 64);
    let lit = |x: usize| (12..36).any(|y| end.pixels[y * end.width + x] == ui.theme.text.0);
    assert!((395..410).any(lit), "the end of the text, and the cursor after it, are in view");
    key(&mut ui, Key::Home, Mods::NONE, |ui| form(&mut app, ui));
    typed(&mut ui, "", |ui| form(&mut app, ui));
    assert_ne!(start, Surface::draw(&mut ui, 64).pixels);
}

#[derive(Default)]
struct Bar {
    clicks: [u32; 2],
    on: bool,
    picked: Option<&'static str>,
    more: Rect,
    other: Rect,
    item: Rect,
    asking: bool,
    answered: bool,
}
fn bar(app: &mut Bar, ui: &mut Ui) {
    ui.row().pad(10.0).gap(8.0).open();
    app.clicks[0] += ui.button("One").clicked as u32;
    let other = ui.button("Two");
    (app.other, app.clicks[1]) = (other.rect, app.clicks[1] + other.clicked as u32);
    ui.checkbox(&mut app.on, "On");
    let more = ui.icon_button(PLUS, "More things to do");
    app.more = more.rect;
    if more.clicked {
        ui.open_popup(more.id, more.rect);
    }
    if ui.menu(more.id) {
        if ui.menu_item("Rename").clicked {
            app.picked = Some("Rename");
        }
        let remove = ui.menu_item("Remove");
        app.item = remove.rect;
        if remove.clicked {
            app.picked = Some("Remove");
        }
        ui.close();
    }
    ui.close();
    if !app.asking {
        return;
    }
    let beside = ui.dialog();
    ui.label("Remove it?");
    app.answered |= ui.button("Remove").clicked;
    ui.close();
    app.asking = !(beside.clicked || app.answered);
}

#[test]
fn tab_moves_the_keyboard_and_space_presses_what_it_is_on() {
    let (mut app, mut ui) = (Bar::default(), window(1.0));
    settle(&mut ui, |ui| bar(&mut app, ui));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| bar(&mut app, ui));
    key(&mut ui, Key::Space, Mods::NONE, |ui| bar(&mut app, ui));
    assert_eq!(app.clicks, [1, 0], "the first Tab went to the first button");
    key(&mut ui, Key::Tab, Mods::NONE, |ui| bar(&mut app, ui));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| bar(&mut app, ui));
    key(&mut ui, Key::Space, Mods::NONE, |ui| bar(&mut app, ui));
    assert!(app.on, "the third went to the tick box");
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| bar(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| bar(&mut app, ui));
    assert_eq!(app.clicks, [1, 1], "Shift and Tab go back");
}

#[test]
fn the_arrow_keys_take_the_keyboard_to_the_nearest_thing_that_way() {
    // Two rows of three buttons. Tab comes to the first; the arrows go on from there, to
    // the one beside or under or over; and where there is nothing that way, nowhere.
    let (mut ui, mut on) = (window(1.0), 9);
    let grid = |ui: &mut Ui, on: &mut usize| {
        ui.grid(3).gap(8.0).open();
        let focused = ["a", "b", "c", "d", "e", "f"].map(|name| ui.button(name).focused);
        ui.close();
        *on = focused.iter().position(|focused| *focused).unwrap_or(9);
    };
    settle(&mut ui, |ui| grid(ui, &mut on));
    for (pressed, comes_to) in [(Key::Tab, 0), (Key::Right, 1), (Key::Down, 4), (Key::Left, 3), (Key::Up, 0), (Key::Up, 0), (Key::Left, 0), (Key::Right, 1), (Key::Right, 2), (Key::Right, 2)] {
        key(&mut ui, pressed, Mods::NONE, |ui| grid(ui, &mut on));
        assert_eq!(on, comes_to, "after {pressed:?}");
    }

    // In a long list only the rows in view are there. The keyboard goes down it all the
    // same: the list is scrolled on, a row at a time, and back.
    let (mut ui, mut on, mut seen) = (window(1.0), 0, 0..0);
    let list = |ui: &mut Ui, on: &mut usize, seen: &mut std::ops::Range<usize>| {
        *seen = ui.list(1000, 20.0);
        for row in seen.clone() {
            let state = ui.selectable(false, &format!("row {row}"));
            *on = if state.focused { row } else { *on };
        }
        ui.close();
        ui.button("under the list");
    };
    settle(&mut ui, |ui| list(ui, &mut on, &mut seen));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| list(ui, &mut on, &mut seen));
    for row in 1..=40 {
        key(&mut ui, Key::Down, Mods::NONE, |ui| list(ui, &mut on, &mut seen));
        assert!(on == row && seen.contains(&row), "after {row} times down, the keyboard is on row {on} and rows {seen:?} show");
        assert!(ui.passes <= 3, "which took {} goes", ui.passes);
    }
    for row in (0..40).rev() {
        key(&mut ui, Key::Up, Mods::NONE, |ui| list(ui, &mut on, &mut seen));
        assert!(on == row && seen.contains(&row), "back up to row {row}: on {on}, {seen:?} show");
    }
}

#[test]
fn a_menu_opens_is_picked_from_and_closes() {
    // At several sharpnesses of screen: what is drawn again has to land on whole pixels at each.
    for scale in [1.0, 1.25, 1.5] {
        let (mut app, mut ui) = (Bar::default(), window(scale));
        settle(&mut ui, |ui| bar(&mut app, ui));
        click(&mut ui, middle(app.more), |ui| bar(&mut app, ui));
        assert!(app.item.w >= 162.0 && app.item.y > app.more.bottom(), "the menu is open under its button: {:?}", app.item);
        click(&mut ui, middle(app.item), |ui| bar(&mut app, ui));
        assert_eq!(app.picked, Some("Remove"));

        // Open again: a press beside it closes it, and goes no further.
        click(&mut ui, middle(app.more), |ui| bar(&mut app, ui));
        click(&mut ui, middle(app.other), |ui| bar(&mut app, ui));
        assert_eq!(app.clicks[1], 0, "the button under the open menu was not clicked");
        click(&mut ui, middle(app.other), |ui| bar(&mut app, ui));
        assert_eq!(app.clicks[1], 1, "with the menu closed, it is");

        // Open again: the keyboard goes into it, on its first row; Enter picks, Escape closes.
        app.picked = None;
        click(&mut ui, middle(app.more), |ui| bar(&mut app, ui));
        key(&mut ui, Key::Enter, Mods::NONE, |ui| bar(&mut app, ui));
        assert_eq!(app.picked, Some("Rename"));
        click(&mut ui, middle(app.more), |ui| bar(&mut app, ui));
        let open = Surface::draw(&mut ui, 64).pixels;
        key(&mut ui, Key::Escape, Mods::NONE, |ui| bar(&mut app, ui));
        assert_ne!(open, Surface::draw(&mut ui, 64).pixels, "Escape closed it");
    }
}

#[test]
fn a_tool_tip_shows_once_the_pointer_has_rested() {
    // At several sharpnesses of screen: what is drawn again has to land on whole pixels at each.
    for scale in [1.0, 1.25, 1.5] {
        let (mut app, mut ui) = (Bar::default(), window(scale));
        settle(&mut ui, |ui| bar(&mut app, ui));
        ui.set_time(Duration::from_secs(1));
        ui.event(Event::Move(middle(app.more)));
        ui.run(&mut |ui| bar(&mut app, ui));
        assert_eq!(ui.wake_in(), Some(Duration::from_millis(500)), "it asks to be run when the tip is due");
        ui.set_time(Duration::from_millis(1500));
        let tip = ui.run(&mut |ui| bar(&mut app, ui));
        // (What is drawn again is the tip and the shadow around it, which reaches 8 further.)
        assert!(tip.y >= app.more.bottom() - 8.0 && tip.h > 20.0 && tip.right() <= 420.0, "the tip is drawn under the button: {tip:?}");
        assert_eq!(ui.wake_in(), None);
        ui.event(Event::Move(pt(200.0, 200.0)));
        assert_eq!(ui.run(&mut |ui| bar(&mut app, ui)).union(app.more), tip.union(app.more), "moving away takes it down again");
    }
}

#[test]
fn a_dialog_keeps_what_is_under_it_out_of_reach() {
    // At several sharpnesses of screen: what is drawn again has to land on whole pixels at each.
    for scale in [1.0, 1.25, 1.5] {
        let (mut app, mut ui) = (Bar { asking: true, ..Default::default() }, window(scale));
        ui.run(&mut |ui| bar(&mut app, ui));
        // Tab only goes round the dialog: its one button.
        key(&mut ui, Key::Tab, Mods::NONE, |ui| bar(&mut app, ui));
        key(&mut ui, Key::Tab, Mods::NONE, |ui| bar(&mut app, ui));
        key(&mut ui, Key::Space, Mods::NONE, |ui| bar(&mut app, ui));
        assert!(app.answered && app.clicks == [0, 0] && !app.asking);

        (app.asking, app.answered) = (true, false);
        settle(&mut ui, |ui| bar(&mut app, ui));
        click(&mut ui, middle(app.other), |ui| bar(&mut app, ui));
        assert!(app.clicks == [0, 0] && !app.asking, "a click beside the dialog closed it, and reached nothing under it");
        click(&mut ui, middle(app.other), |ui| bar(&mut app, ui));
        assert_eq!(app.clicks[1], 1);
    }
}

#[test]
fn a_long_list_describes_only_the_rows_in_view_and_its_bar_is_dragged() {
    let mut ui = window(1.0);
    let mut seen = Vec::new();
    let build = |ui: &mut Ui, seen: &mut Vec<usize>| {
        *seen = ui.list(100_000, 20.0).collect();
        for row in seen.iter() {
            ui.row().key(row).h(Length::Px(20.0)).open();
            ui.label(&format!("line {row}"));
            ui.close();
        }
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut seen));
    assert_eq!(seen, (0..15).collect::<Vec<_>>(), "300 high, rows of 20: fifteen of the hundred thousand");

    // The bar runs down the right edge. Caught at its top and taken half way down, it shows the middle.
    for event in [Event::Move(pt(415.0, 5.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(415.0, 5.0 + (300.0 - 24.0) / 2.0)), Event::Release(Button::Left)] {
        ui.event(event);
        ui.run(&mut |ui| build(ui, &mut seen));
    }
    assert!((49_900..50_100).contains(&seen[0]) && seen.len() <= 16, "{seen:?}");
}

#[test]
fn a_splitter_is_dragged_to_resize_what_is_beside_it() {
    let mut ui = window(1.0);
    let (mut wide, mut at) = (120.0, Rect::EMPTY);
    let build = |ui: &mut Ui, wide: &mut f32, at: &mut Rect| {
        ui.row().grow().open();
        ui.el().w(Length::Px(*wide)).h(Length::Grow(1.0)).show();
        *at = ui.splitter(wide, 60.0, 200.0).rect;
        ui.el().grow().show();
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut wide, &mut at));
    assert_eq!(at.x, 120.0);
    for (event, expected) in [(Event::Move(middle(at)), 120.0), (Event::Press(Button::Left, Mods::NONE), 120.0), (Event::Move(pt(152.5, 100.0)), 150.0), (Event::Move(pt(400.0, 100.0)), 200.0)] {
        ui.event(event);
        ui.run(&mut |ui| build(ui, &mut wide, &mut at));
        assert_eq!(wide, expected);
    }
}

/// One of each of the smaller parts, and where they ended up.
#[derive(Default)]
struct Parts {
    on: bool,
    pick: u8,
    level: f32,
    page: usize,
    open: bool,
    switch: Rect,
    radio: Rect,
    slider: Rect,
}
fn parts(app: &mut Parts, ui: &mut Ui) {
    ui.column().pad(10.0).gap(8.0).open();
    app.switch = ui.switch(&mut app.on, "On").rect;
    ui.radio(&mut app.pick, 1, "One");
    app.radio = ui.radio(&mut app.pick, 2, "Two").rect;
    app.slider = ui.slider(&mut app.level, 0.0..=10.0).rect;
    ui.tabs(&mut app.page, &["First", "Second"]);
    if ui.fold(&mut app.open, "More") {
        ui.label("what was folded away");
    }
    ui.progress(app.level / 10.0);
    ui.close();
}

#[test]
fn a_switch_a_radio_button_a_slider_tabs_and_a_fold_do_what_they_say() {
    let (mut app, mut ui) = (Parts { pick: 1, ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| parts(&mut app, ui));

    // The switch flips at once, and slides over for a little while after.
    ui.set_time(Duration::from_secs(1));
    click(&mut ui, middle(app.switch), |ui| parts(&mut app, ui));
    assert!(app.on && ui.wake_in().is_some(), "it is on, and still sliding");
    ui.set_time(Duration::from_millis(1060));
    assert!(!ui.run(&mut |ui| parts(&mut app, ui)).is_empty() && ui.wake_in().is_some(), "half way, it has moved and is not there yet");
    ui.set_time(Duration::from_millis(1500));
    ui.run(&mut |ui| parts(&mut app, ui));
    assert_eq!(ui.wake_in(), None, "there, nothing more is asked for");

    click(&mut ui, middle(app.radio), |ui| parts(&mut app, ui));
    assert_eq!(app.pick, 2);

    // A press in the middle of the slider puts it half way; an arrow key takes it a twentieth on.
    click(&mut ui, middle(app.slider), |ui| parts(&mut app, ui));
    assert!((app.level - 5.0).abs() < 0.1, "{}", app.level);
    let before = app.level;
    key(&mut ui, Key::Right, Mods::NONE, |ui| parts(&mut app, ui));
    assert!((app.level - before - 0.5).abs() < 1e-4);

    // The tabs are under the slider, the fold under them. Letters are 12 wide: "First" ends 80 in.
    let tabs = app.slider.bottom() + 8.0 + 14.0;
    click(&mut ui, pt(110.0, tabs), |ui| parts(&mut app, ui));
    assert_eq!(app.page, 1);
    let folded = Surface::draw(&mut ui, 64).pixels;
    click(&mut ui, pt(40.0, tabs + 19.0 + 8.0 + 14.0), |ui| parts(&mut app, ui));
    assert!(app.open && folded != Surface::draw(&mut ui, 64).pixels, "the fold opened, and shows what was in it");
}

#[test]
fn an_app_draws_its_own_shapes_and_pictures_where_it_likes_and_closes_its_window() {
    let mut ui = window(1.0);
    // A picture of four pixels: red and green over blue and white.
    let four = ui.add_picture(2, vec![0xffff0000, 0xff00ff00, 0xff0000ff, 0xffffffff]);
    let mut line = Path::new();
    line.move_to(10.0, 10.0).line_to(90.0, 10.0);
    let build = |ui: &mut Ui, line: &Path| {
        ui.el().flow(Flow::Layers).grow().open();
        ui.el().size(100.0, 40.0).at(50.0, 100.0).outline(line, 4.0, Color::rgb(0xff0000)).show();
        ui.el().picture(four).size(40.0, 40.0).at(200.0, 30.0).show();
        ui.close();
    };
    ui.run(&mut |ui| build(ui, &line));
    let shown = Surface::draw(&mut ui, 64);
    let at = |x: usize, y: usize| shown.pixels[y * shown.width + x];
    // The line starts 10 into its box, which is put at 50, 100: it runs along y 110, 4 thick.
    assert!(at(100, 110) >> 16 & 0xff >= 0xfa && at(100, 110) & 0xffff < 0x0800, "the line is red here: {:08x}", at(100, 110));
    assert_eq!((at(100, 105), at(155, 110)), (at(5, 5), at(5, 5)), "and not above it, nor past its round end");
    assert_eq!([at(200, 30), at(239, 30), at(200, 69), at(239, 69)], [0xffff0000, 0xff00ff00, 0xff0000ff, 0xffffffff], "the picture's corners are its four pixels");
    assert!(![0xffff0000, 0xff00ff00, 0xff0000ff, 0xffffffff].contains(&at(220, 50)), "and between them the colours run into each other");

    // The shape changes: what is drawn again is its box, and nothing is left behind.
    line.line_to(90.0, 30.0);
    let changed = ui.run(&mut |ui| build(ui, &line));
    assert_eq!(changed, Rect { x: 50.0, y: 100.0, w: 100.0, h: 40.0 });
    assert!(!ui.quitting());
    ui.quit();
    assert!(ui.quitting());
}

/// Parts that people coming from Dear ImGui look for, and where they ended up.
#[derive(Default)]
struct More {
    picked: usize,
    doubles: u32,
    removed: bool,
    open: bool,
    amount: f32,
    off: bool,
    clicks: u32,
    rows: [Rect; 3],
    item: Rect,
    leaf: Rect,
    number: Rect,
    button: Rect,
}
fn more(app: &mut More, ui: &mut Ui) {
    ui.column().pad(10.0).gap(6.0).open();
    for (i, name) in ["First", "Second", "Third"].iter().enumerate() {
        let row = ui.selectable(app.picked == i, name);
        (app.rows[i], app.doubles) = (row.rect, app.doubles + row.double_clicked as u32);
        if row.clicked {
            app.picked = i;
        }
        if ui.context_menu(&row) {
            let item = ui.menu_item("Remove");
            (app.item, app.removed) = (item.rect, app.removed || item.clicked);
            ui.close();
        }
    }
    ui.tree(&mut app.open, "Folder");
    if app.open {
        app.leaf = ui.selectable(false, "a file").rect;
        ui.close();
    }
    app.number = ui.drag(&mut app.amount, 0.5).rect;
    ui.plot_lines(&[1.0, 3.0, 2.0, 5.0], 120.0, 40.0);
    ui.plot_bars(&[1.0, 3.0, 2.0, 5.0], 120.0, 40.0);
    ui.set_enabled(!app.off);
    let button = ui.button("Go");
    (app.button, app.clicks) = (button.rect, app.clicks + button.clicked as u32);
    ui.set_enabled(true);
    ui.close();
}

#[test]
fn rows_are_picked_trees_open_numbers_are_dragged_and_parts_are_switched_off() {
    let (mut app, mut ui) = (More::default(), window(1.0));
    // (High enough for all of it with the tree open: the button at the bottom is pressed.)
    ui.resize(taut::geom::Size { w: 420.0, h: 360.0 }, 1.0);
    settle(&mut ui, |ui| more(&mut app, ui));
    click(&mut ui, middle(app.rows[1]), |ui| more(&mut app, ui));
    assert_eq!((app.picked, app.doubles), (1, 0));
    // Two clicks close together are a double click.
    ui.set_time(Duration::from_secs(5));
    click(&mut ui, middle(app.rows[2]), |ui| more(&mut app, ui));
    ui.set_time(Duration::from_millis(5100));
    click(&mut ui, middle(app.rows[2]), |ui| more(&mut app, ui));
    assert_eq!((app.picked, app.doubles), (2, 1));

    // The right button opens a menu where the pointer is; picking from it closes it.
    for event in [Event::Move(middle(app.rows[0])), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)] {
        ui.event(event);
        ui.run(&mut |ui| more(&mut app, ui));
    }
    assert!(app.item.y > middle(app.rows[0]).y && app.item.x >= middle(app.rows[0]).x, "the menu is by the pointer: {:?}", app.item);
    click(&mut ui, middle(app.item), |ui| more(&mut app, ui));
    assert!(app.removed);

    // The tree's heading is under the three rows (28 high, 6 apart); what is in it stands 16 in.
    click(&mut ui, pt(40.0, 10.0 + 3.0 * 34.0 + 14.0), |ui| more(&mut app, ui));
    settle(&mut ui, |ui| more(&mut app, ui));
    assert!(app.open && app.leaf.x == 26.0, "{:?}", app.leaf);

    // Dragged 20 to the right at half a unit each, the number is 10.
    for event in [Event::Move(middle(app.number)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(middle(app.number).x + 20.0, middle(app.number).y)), Event::Release(Button::Left)] {
        ui.event(event);
        ui.run(&mut |ui| more(&mut app, ui));
    }
    assert_eq!(app.amount, 10.0);

    // Switched off, a button takes no click, and Tab passes it by.
    app.off = true;
    settle(&mut ui, |ui| more(&mut app, ui));
    click(&mut ui, middle(app.button), |ui| more(&mut app, ui));
    assert_eq!(app.clicks, 0);
    app.off = false;
    settle(&mut ui, |ui| more(&mut app, ui));
    click(&mut ui, middle(app.button), |ui| more(&mut app, ui));
    assert_eq!(app.clicks, 1);
}

#[test]
fn a_dialog_that_opens_can_put_the_keyboard_in_its_text_box() {
    let mut ui = window(1.0);
    let (mut asking, mut name, mut opener) = (false, String::new(), Rect::EMPTY);
    let build = |ui: &mut Ui, asking: &mut bool, name: &mut String, opener: &mut Rect| {
        let open = ui.button("Rename");
        (*opener, *asking) = (open.rect, *asking || open.clicked);
        if !*asking {
            return;
        }
        ui.dialog();
        let name_box = ui.text_box(name, "Name");
        if ui.focus() == 0 {
            ui.set_focus(name_box.id);
        }
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut asking, &mut name, &mut opener));
    click(&mut ui, middle(opener), |ui| build(ui, &mut asking, &mut name, &mut opener));
    typed(&mut ui, "Taut", |ui| build(ui, &mut asking, &mut name, &mut opener));
    assert_eq!(name, "Taut", "what is typed after the dialog opens goes into its box");
}

/// Three buttons: one at the top of the window, one right under it, and one at the bottom.
fn far_apart(ui: &mut Ui) -> [Rect; 3] {
    ui.column().grow().pad(10.0).gap(4.0).align(taut::Align::Start, taut::Align::Start).open();
    let (top, under) = (ui.button("Top").rect, ui.button("Under it").rect);
    ui.spacer();
    let bottom = ui.button("Bottom").rect;
    ui.close();
    [top, under, bottom]
}

#[test]
fn changes_far_apart_are_drawn_each_by_itself() {
    let (mut ui, mut at) = (window(1.0), [Rect::EMPTY; 3]);
    settle(&mut ui, |ui| at = far_apart(ui));
    does(&mut ui, Event::Move(middle(at[0])), |ui| at = far_apart(ui));
    // From the button at the top straight to the one at the bottom: the one stops showing
    // that the pointer is over it and the other starts to, and nothing between them is
    // drawn again. The rectangle the run gives back is still the one around both.
    ui.event(Event::Move(middle(at[2])));
    let changed = ui.ui.run(&mut |ui| at = far_apart(ui));
    assert_eq!(changed, at[0].union(at[2]));
    assert_eq!(ui.ui.changes().collect::<Vec<_>>(), [at[0], at[2]]);
    ui.screen.redraw(&mut ui.ui, changed);
    // From there to the one at the top again, and on to the one right under it: those two
    // lie near each other, and are drawn in one go.
    does(&mut ui, Event::Move(middle(at[0])), |ui| at = far_apart(ui));
    ui.event(Event::Move(middle(at[1])));
    let changed = ui.ui.run(&mut |ui| at = far_apart(ui));
    assert_eq!(changed, at[0].union(at[1]));
    assert_eq!(ui.ui.changes().collect::<Vec<_>>(), [changed]);
    ui.screen.redraw(&mut ui.ui, changed);
    // A program that runs twice before it draws once is handed the places of both runs:
    // here the one at the top, which the first of the two runs changed, lies in the
    // rectangle the second gave back but is not one of the places that one changed.
    does(&mut ui, Event::Move(middle(at[0])), |ui| at = far_apart(ui));
    ui.event(Event::Move(pt(300.0, 150.0)));
    let first = ui.ui.run(&mut |ui| at = far_apart(ui));
    ui.event(Event::Move(middle(at[2])));
    let second = ui.ui.run(&mut |ui| at = far_apart(ui));
    assert_eq!((first, second), (at[0], at[2]));
    ui.screen.redraw(&mut ui.ui, first.union(second));
    assert!(ui.screen.pixels == Surface::draw(&mut ui.ui, 64).pixels, "both places were drawn");
    // And a run that changes nothing has nothing to draw.
    let changed = ui.run(&mut |ui| at = far_apart(ui));
    assert!(changed.is_empty() && ui.changes().next().is_none());
}

#[test]
fn a_screen_reader_is_told_each_part_as_the_kind_of_thing_it_is() {
    #[derive(Default)]
    struct Parts {
        loud: bool,
        dark: bool,
        tab: usize,
        size: usize,
        row: usize,
        password: String,
        asking: bool,
    }
    let parts = |app: &mut Parts, ui: &mut Ui| {
        ui.menu_bar();
        if ui.menu_button("File") {
            ui.menu_item("Open\tCtrl+O");
            ui.menu_check(&mut app.dark, "Dark");
            ui.close();
        }
        ui.close();
        ui.switch(&mut app.loud, "Loud");
        ui.tabs(&mut app.tab, &["One", "Two", "Three"]);
        ui.select(&mut app.size, &["Roomy", "Tight"]);
        ui.secret_box(&mut app.password, "Password");
        for row in ui.list(500, 20.0) {
            if ui.selectable(app.row == row, &format!("row {row}")).clicked {
                app.row = row;
            }
        }
        ui.close();
        if app.asking {
            ui.dialog();
            ui.label("Are you sure?");
            ui.button("Yes");
            ui.close();
        }
    };
    let (mut ui, mut app) = (window(1.0), Parts::default());
    ui.listen(true);
    settle(&mut ui, |ui| parts(&mut app, ui));
    // Each thing with its kind, its name, how it stands, and what it is in (by number).
    let told = |ui: &Window| ui.said().iter().map(|said| (said.role, said.name.clone(), said.on, said.within)).collect::<Vec<_>>();
    let page = told(&ui);
    let expected = [
        (Role::MenuBar, "", None, None),
        (Role::Fold, "File", Some(false), Some(0)),
        (Role::Switch, "Loud", Some(false), None),
        (Role::Tabs, "", None, None),
        (Role::Tab, "One", Some(true), Some(3)),
        (Role::Tab, "Two", Some(false), Some(3)),
        (Role::Tab, "Three", Some(false), Some(3)),
        (Role::Select, "", Some(false), None),
        (Role::Secret, "Password", None, None),
        (Role::Row, "row 0", Some(true), None),
        (Role::Row, "row 1", Some(false), None),
    ];
    assert_eq!(page[..11], expected.map(|(role, name, on, within)| (role, name.to_string(), on, within)));
    // A tab says which one of how many it is, and so does a row of a long list, of which
    // only the rows in view are there; and the box that opens a list says what is chosen.
    let of = |ui: &Window, name: &str| ui.said().iter().find(|said| said.name == name).and_then(|said| said.of);
    assert_eq!((of(&ui, "Two"), of(&ui, "row 1"), of(&ui, "Loud"), ui.said()[7].value.as_str()), (Some([2, 3]), Some([2, 500]), None, "Roomy"));
    assert!(ui.said().len() < 30, "of five hundred rows, the few in view are told");

    // The menu is opened: it is told as a menu, after everything in the window it lies
    // over, and its rows are called by their names, with the keys that do the same apart.
    // The button that opened it says that it is open, and everything else is told as
    // before, each thing still known as the same thing.
    let ids = |ui: &Window| ui.said().iter().filter(|said| !matches!(said.role, Role::Menu | Role::MenuItem)).map(|said| said.id).collect::<Vec<_>>();
    let (file, before) = (ui.said()[1].id, ids(&ui));
    ui.press_for_reader(file);
    settle(&mut ui, |ui| parts(&mut app, ui));
    let at = ui.said().iter().position(|said| said.role == Role::Menu).expect("a menu");
    let menu: Vec<_> = ui.said()[at..at + 3].iter().map(|said| (said.role, said.name.as_str(), said.keys.as_str(), said.on, said.within)).collect();
    assert_eq!(menu, [(Role::Menu, "", "", None, None), (Role::MenuItem, "Open", "Ctrl+O", None, Some(at)), (Role::MenuItem, "Dark", "", Some(false), Some(at))]);
    assert_eq!((ui.said()[1].on, ids(&ui)), (Some(true), before.clone()));
    // Escape closes it, and the window is told of as it was.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| parts(&mut app, ui));
    ui.run(&mut |ui| parts(&mut app, ui));
    assert_eq!((&told(&ui)[..9], ids(&ui)), (&page[..9], before));

    // What is typed into the box for a password is told as its dots, with where the cursor
    // is among them.
    let secret = ui.said()[8].id;
    ui.type_for_reader(secret, "abc");
    settle(&mut ui, |ui| parts(&mut app, ui));
    let secret = &ui.said()[8];
    assert_eq!((app.password.as_str(), secret.value.as_str(), secret.caret, secret.focused), ("abc", "•••", Some([3, 3]), true));

    // A dialog has to be answered first: nothing under it is told. It is called by what
    // stands at its top.
    app.asking = true;
    settle(&mut ui, |ui| parts(&mut app, ui));
    let dialog: Vec<_> = ui.said().iter().map(|said| (said.role, said.name.as_str(), said.within)).collect();
    assert_eq!(dialog, [(Role::Dialog, "Are you sure?", None), (Role::Text, "Are you sure?", Some(0)), (Role::Button, "Yes", Some(0))]);

    // Nobody listens any more: nothing is kept.
    ui.listen(false);
    assert!(ui.said().is_empty());
}

#[test]
fn a_box_that_is_typed_into_keeps_room_clear_at_its_ends_when_it_is_asked_to() {
    let mut ui = window(1.0);
    let (mut text, mut room, mut boxes) = (String::new(), [0.0f32; 2], [0, 0]);
    let build = |ui: &mut Ui, text: &mut String, room: [f32; 2], boxes: &mut [u64; 2]| {
        ui.column().pad(10.0).gap(10.0).open();
        ui.text_room(room[0], room[1]);
        boxes[0] = ui.text_area(text, "", (1, 4)).id;
        // (Asked for once, it is for one box: the next has none.)
        boxes[1] = ui.text_box(&mut String::new(), "").id;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut text, room, &mut boxes));
    ui.set_focus(boxes[0]);
    settle(&mut ui, |ui| build(ui, &mut text, room, &mut boxes));
    // The cursor of the empty box stands 8 units in from the box's edge, which is 10 in.
    assert_eq!(ui.typing_at().map(|at| at.x), Some(18.0));
    room = [20.0, 290.0];
    settle(&mut ui, |ui| build(ui, &mut text, room, &mut boxes));
    assert_eq!(ui.typing_at().map(|at| at.x), Some(38.0));
    // Of the 400 units the box is wide, 290 and 20 and the two eights are kept clear: of the 74 left,
    // six letters of 12 take 72, and the seventh starts the next line.
    typed(&mut ui, "abcdefg", |ui| build(ui, &mut text, room, &mut boxes));
    ui.run(&mut |ui| build(ui, &mut text, room, &mut boxes));
    let cursor = ui.typing_at().expect("the box is typed into");
    assert_eq!((cursor.x, cursor.y > 30.0), (38.0 + 12.0, true), "the seventh letter stands in the second line");
    // The box under it kept nothing clear.
    ui.set_focus(boxes[1]);
    settle(&mut ui, |ui| build(ui, &mut text, room, &mut boxes));
    assert_eq!(ui.typing_at().map(|at| at.x), Some(18.0));
}

#[test]
fn an_app_hears_of_its_window_and_asks_things_of_it() {
    let mut ui = window(1.0);
    // What the window says of itself: minimized, and back; the active one, or not; and
    // whether the person asked the system for dark windows.
    assert!(ui.window_active() && !ui.window_hidden() && !ui.system_dark());
    assert!(ui.event(Event::Hidden(true)) && ui.window_hidden());
    assert!(ui.event(Event::Focus(false)) && !ui.window_active());
    assert!(ui.event(Event::Hidden(false)) && !ui.window_hidden());
    let dark = taut::Wishes { dark: true, ..Default::default() };
    assert!(ui.set_wishes(dark) && ui.system_dark() && !ui.set_wishes(dark));
    // What the app asks of its window is handed on once.
    ui.run(&mut |ui| {
        ui.set_on_top(true);
        ui.come_forward();
    });
    assert_eq!((ui.take_on_top(), ui.take_forward()), (Some(true), true));
    assert_eq!((ui.take_on_top(), ui.take_forward()), (None, false));
    assert!(Color::rgb(0x0f1115).is_dark() && !Color::rgb(0xf5f6f8).is_dark() && Color::rgb(0x0000ff).is_dark() && !Color::rgb(0x00ff00).is_dark());
}

#[test]
fn notices_lie_in_a_corner_over_a_dialog_and_keep_only_their_own_place_from_it() {
    let mut ui = window(1.0);
    // What was clicked: the notice, the dialog's button, beside the dialog. And where the
    // notice and the dialog's button are.
    let mut seen = ([0u32; 3], Rect::EMPTY, Rect::EMPTY);
    let build = |ui: &mut Ui, seen: &mut ([u32; 3], Rect, Rect)| {
        ui.label("The page");
        let beside = ui.dialog();
        let fine = ui.button("Fine");
        ui.close();
        ui.notices(taut::Align::End, taut::Align::End);
        let undo = ui.button("Undo");
        ui.close();
        seen.0 = [seen.0[0] + undo.clicked as u32, seen.0[1] + fine.clicked as u32, seen.0[2] + beside.clicked as u32];
        (seen.1, seen.2) = (undo.rect, fine.rect);
    };
    settle(&mut ui, |ui| build(ui, &mut seen));
    // The notice ends 12 units from the right and the bottom edge of the 420 by 300 window.
    assert_eq!((seen.1.right(), seen.1.bottom()), (408.0, 288.0));
    // It is pressed though a dialog is up; the dialog's own button still is; and right
    // beside the notice, the press is one beside the dialog, as it would be without it.
    click(&mut ui, middle(seen.1), |ui| build(ui, &mut seen));
    assert_eq!(seen.0, [1, 0, 0]);
    click(&mut ui, middle(seen.2), |ui| build(ui, &mut seen));
    assert_eq!(seen.0, [1, 1, 0]);
    click(&mut ui, pt(seen.1.x - 6.0, seen.1.y + 4.0), |ui| build(ui, &mut seen));
    assert_eq!(seen.0, [1, 1, 1]);
}

/// What an overlay holds: a text box's text, whether its dialog is up, and where its button
/// and its text box were put.
#[derive(Default)]
struct Overlay {
    name: String,
    asking: bool,
    rects: [Rect; 2],
}

/// An overlay: a page that can be seen through, with a panel, a filled box and bare text.
fn overlay(state: &mut Overlay, ui: &mut Ui) {
    let t = ui.theme;
    ui.el().flow(Flow::Layers).grow().open();
    ui.column().place(Align::Start).at(16.0, 16.0).w(Length::Px(180.0)).pad(10.0).gap(8.0).fill(t.panel).open();
    state.rects = [ui.button("Press").rect, ui.text_box(&mut state.name, "Name").rect];
    ui.close();
    ui.el().place(Align::Start).at(260.0, 20.0).size(60.0, 30.0).fill(t.accent).show();
    ui.el().place(Align::Start).at(260.0, 120.0).text("bare text", t.font, t.text).show();
    ui.close();
    if state.asking {
        ui.dialog();
        state.asking = !ui.button("Fine").clicked;
        ui.close();
    }
}

/// A program that shares the pointer and the keyboard with the interface (a game under an
/// overlay, a DLL in another program's window) asks whose they are.
#[test]
fn a_host_is_told_whose_the_pointer_and_the_keyboard_are() {
    let (mut ui, mut state) = (window(1.0), Overlay::default());
    ui.theme.bg = Color::NONE;
    settle(&mut ui, |ui| overlay(&mut state, ui));
    // With the pointer nowhere, and then over the clear page, neither is the interface's.
    assert!(!ui.wants_pointer() && !ui.wants_keyboard());
    let nowhere = pt(380.0, 260.0);
    does(&mut ui, Event::Move(nowhere), |ui| overlay(&mut state, ui));
    assert!(!ui.wants_pointer());
    // Over the panel between its parts, over a part, and over a filled box that senses
    // nothing, the pointer is the interface's; over bare text on the clear page it is not.
    let ([button, name], in_panel, on_filled, on_text) = (state.rects, pt(20.0, 20.0), pt(290.0, 35.0), pt(275.0, 130.0));
    for (at, wanted) in [(in_panel, true), (middle(button), true), (on_filled, true), (on_text, false), (nowhere, false)] {
        does(&mut ui, Event::Move(at), |ui| overlay(&mut state, ui));
        assert_eq!(ui.wants_pointer(), wanted, "with the pointer at {at:?}");
    }
    // A box that is held keeps the pointer wherever it goes, until it is let go.
    for event in [Event::Move(middle(button)), Event::Press(Button::Left, Mods::NONE), Event::Move(nowhere)] {
        does(&mut ui, event, |ui| overlay(&mut state, ui));
    }
    assert!(ui.wants_pointer());
    does(&mut ui, Event::Release(Button::Left), |ui| overlay(&mut state, ui));
    assert!(!ui.wants_pointer());
    // The keyboard is the interface's while it is on one of its boxes: a press on the text
    // box puts it there, and a press beside every box takes it off again.
    click(&mut ui, middle(name), |ui| overlay(&mut state, ui));
    assert!(ui.wants_keyboard() && ui.wants_pointer());
    click(&mut ui, nowhere, |ui| overlay(&mut state, ui));
    assert!(!ui.wants_keyboard() && !ui.wants_pointer());
    // And a dialog keeps both, wherever the pointer is.
    state.asking = true;
    settle(&mut ui, |ui| overlay(&mut state, ui));
    does(&mut ui, Event::Move(nowhere), |ui| overlay(&mut state, ui));
    assert!(ui.wants_pointer() && ui.wants_keyboard());
}

/// A dialog that opens with a name in its box, to be typed over: the app puts the keyboard
/// there with all the text selected when nothing has the keyboard. Opened by a button, which
/// had it, the keyboard is taken into the dialog by itself: the text is selected all the same.
/// (It was not, for a while: the app's wish came too late, the keyboard being in the box
/// already.)
struct Renaming {
    name: String,
    renaming: Option<String>,
    field: u64,
}

fn renaming(app: &mut Renaming, ui: &mut Ui) {
    ui.column().pad(12.0).gap(8.0).open();
    ui.label(&app.name);
    if ui.button("Rename").clicked {
        app.renaming = Some(app.name.clone());
    }
    ui.close();
    let Some(name) = &mut app.renaming else { return };
    let _beside = ui.dialog();
    let field = ui.text_box(name, "What is to be done");
    app.field = field.id;
    if ui.focus() == 0 {
        ui.set_focus(field.id);
        ui.select_all();
    }
    ui.close();
}

/// A fresh interface of 400 by 300, with what is told to a screen reader kept.
fn bare_ui() -> Ui {
    let mut ui = Ui::new(Box::new(taut::text::BuiltInFont), Box::new(taut::LocalClipboard::default()));
    ui.resize(taut::geom::Size { w: 400.0, h: 300.0 }, 1.0);
    ui.listen(true);
    ui
}

/// Presses the button that is told by this name, running `describe` after each event.
fn press_named(ui: &mut Ui, name: &str, describe: &mut dyn FnMut(&mut Ui)) {
    let button = ui.said().iter().find(|said| said.name == name).expect("the button").rect;
    let at = pt(button.x + button.w / 2.0, button.y + button.h / 2.0);
    for event in [Event::Move(at), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
        ui.event(event);
        ui.run(describe);
    }
}

#[test]
fn a_dialog_opens_with_the_name_in_its_box_selected() {
    let (mut ui, mut app) = (bare_ui(), Renaming { name: "Buy bread".into(), renaming: None, field: 0 });
    (0..2).for_each(|_| _ = ui.run(&mut |ui| renaming(&mut app, ui)));
    press_named(&mut ui, "Rename", &mut |ui| renaming(&mut app, ui));
    (0..3).for_each(|_| _ = ui.run(&mut |ui| renaming(&mut app, ui)));
    assert_eq!(ui.focus(), app.field, "the keyboard is in the dialog's box");
    assert_eq!(ui.typing(app.field).map(|typing| typing.selected), Some([0, 9]), "all of the old name is selected");
}

#[test]
fn a_box_that_was_there_already_has_all_its_text_selected() {
    let (mut ui, mut name, mut field) = (bare_ui(), "Buy bread".to_string(), 0);
    let mut plain = |ui: &mut Ui| {
        let typed = ui.text_box(&mut name, "Name");
        field = typed.id;
        if ui.button("Select").clicked {
            ui.set_focus(typed.id);
            ui.select_all();
        }
    };
    (0..2).for_each(|_| _ = ui.run(&mut plain));
    press_named(&mut ui, "Select", &mut plain);
    ui.run(&mut plain);
    assert_eq!(ui.focus(), field);
    assert_eq!(ui.typing(field).map(|typing| typing.selected), Some([0, 9]));
}
