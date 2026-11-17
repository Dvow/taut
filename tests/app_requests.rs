//! Requests from apps built on the library, tested from outside: a key whose effect shows at
//! once, a page taller than its window, scroll boxes scrolled as the app says (to the end, a
//! box or row into view, sticking to the end), long lists of varying row heights, the wheel
//! over an app's own box, a box's available room, an app's Edit menu reaching a text box, a
//! page of text, Enter left to the app, pictures queried for size and fitted, and what a tree
//! heading reports. Every step goes through `common`'s window, which compares incremental
//! redraws with full ones.
mod common;

use common::*;
use taut::geom::{Size, pt, rect};
use taut::{Color, Event, Id, Key, Length, Mods, Rect, Role, ScrollState, TextCommand, Ui};

/// Runs the window once more with nothing new: it must change nothing.
fn assert_settled(ui: &mut Window, build: impl FnMut(&mut Ui)) {
    let mut build = build;
    let changed = ui.run(&mut build);
    assert!(changed.is_empty() && ui.passes == 1, "a run with nothing new changed {changed:?} in {} passes", ui.passes);
}

// ---------------------------------------------------------------------------------------
// A key shows what it changed at once.

#[derive(Default)]
struct Notes {
    count: u32,
    shown: String,
    draft: String,
    list: Vec<String>,
    typed: String,
}

fn notes(app: &mut Notes, ui: &mut Ui) {
    app.shown = format!("{} new", app.count);
    ui.label(&app.shown);
    let typing = ui.text_box(&mut app.draft, "Something to do");
    // Asked for after the label and the box that show what it changes.
    if typing.focused && ui.key(Key::Enter, Mods::NONE) {
        app.list.push(std::mem::take(&mut app.draft));
    }
    if ui.keys("Ctrl+N") {
        app.count += 1;
    }
    app.typed += &ui.typed();
}

#[test]
fn a_key_that_is_taken_runs_the_window_once_more_as_a_click_does() {
    let (mut app, mut ui) = (Notes::default(), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| notes(&mut app, ui));
    key(&mut ui, Key::Char('n'), Mods::CTRL, |ui| notes(&mut app, ui));
    assert_eq!((app.shown.as_str(), ui.passes), ("1 new", 2), "the label described before the key was asked for shows what it did");
    assert!(find_node(&ui, "1 new").is_some());
    assert_settled(&mut ui, |ui| notes(&mut app, ui));

    // Enter in a box of one line is the app's: the box shows what the app left in it.
    let box_at = ui.visible_access_nodes().iter().find(|node| node.role == Role::TextBox).unwrap().rect;
    click(&mut ui, middle(box_at), |ui| notes(&mut app, ui));
    typed(&mut ui, "Buy milk", |ui| notes(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| notes(&mut app, ui));
    let value = ui.visible_access_nodes().iter().find(|node| node.role == Role::TextBox).unwrap().value.clone();
    assert_eq!((app.list.as_slice(), value.as_str()), (&["Buy milk".to_string()][..], ""), "added, and the box is empty in the same run");
    assert_settled(&mut ui, |ui| notes(&mut app, ui));

    // A key nobody asks for runs nothing more; text no box takes is the app's, and shows too.
    key(&mut ui, Key::F(9), Mods::NONE, |ui| notes(&mut app, ui));
    assert_eq!(ui.passes, 1);
    ui.set_focus(0);
    settle(&mut ui, |ui| notes(&mut app, ui));
    typed(&mut ui, "x", |ui| notes(&mut app, ui));
    assert_eq!((app.typed.as_str(), ui.passes), ("x", 2));
}

// ---------------------------------------------------------------------------------------
// A page taller than its window scrolls; one that fits is as it was.

#[derive(Default)]
struct Page {
    rows: usize,
    pressed: Vec<usize>,
    lies: Vec<Rect>,
    grown: Rect,
}

fn page(app: &mut Page, ui: &mut Ui) {
    let t = ui.theme;
    app.lies.clear();
    for row in 0..app.rows {
        let state = ui.row().key(row).w(Length::Grow(1.0)).h(Length::Px(30.0)).fill(t.panel).sense().focusable().open();
        ui.label(&format!("Row {row}"));
        ui.close();
        if state.clicked {
            app.pressed.push(row);
        }
        app.lies.push(state.rect);
    }
    // What grows shares what is left of the window, as on any page that does not scroll.
    app.grown = ui.el().grow().min(0.0, 20.0).fill(t.accent).sense().show().rect;
}

#[test]
fn a_page_taller_than_its_window_scrolls_and_one_that_fits_is_as_it_was() {
    let (mut app, mut ui) = (Page { rows: 4, ..Default::default() }, window(1.0));
    ui.set_screen_reader(true);
    let ground = ui.theme.bg;
    settle(&mut ui, |ui| page(&mut app, ui));
    // Four rows of 30 and what grows under them fill the window, and nothing scrolls.
    assert_eq!((app.lies[3], app.grown), (rect(0.0, 90.0, 420.0, 30.0), rect(0.0, 120.0, 420.0, 180.0)));
    assert_settled(&mut ui, |ui| page(&mut app, ui));
    ui.event(Event::Move(pt(200.0, 100.0)));
    does(&mut ui, Event::Wheel(0.0, 300.0), |ui| page(&mut app, ui));
    assert_eq!(app.lies[0].y, 0.0, "the wheel moves nothing");

    // Twenty rows are taller than the window: the page has a bar down its side, and what is
    // in it keeps clear of the bar.
    app.rows = 20;
    settle(&mut ui, |ui| page(&mut app, ui));
    assert_eq!(app.lies[0], rect(0.0, 0.0, 410.0, 30.0));
    assert!(painted_pixels(&ui, rect(411.0, 0.0, 9.0, 300.0), ground) > 0, "a bar is drawn");
    assert_settled(&mut ui, |ui| page(&mut app, ui));
    // Only what is in view is told, and can be pressed.
    let rows: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.name.starts_with("Row ")).map(|node| node.name.clone()).collect();
    assert_eq!(rows.len(), 10, "rows 0 to 9 are in view: {rows:?}");

    // The wheel scrolls it; a press lands on what shows there.
    does(&mut ui, Event::Wheel(0.0, 100.0), |ui| page(&mut app, ui));
    assert_settled(&mut ui, |ui| page(&mut app, ui));
    assert_eq!(app.lies[4].y, 20.0);
    click(&mut ui, pt(100.0, 30.0), |ui| page(&mut app, ui));
    assert_eq!(app.pressed, [4]);
    // The keyboard brings what it goes to into view: Tab from the last row in view on.
    for _ in 0..8 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| page(&mut app, ui));
    }
    let on = focused_node(&ui).unwrap();
    assert!(on.name == "Row 12" && on.rect.bottom() <= 300.0 && on.rect.y >= 0.0, "{} at {:?}", on.name, on.rect);
    // End and Home go to its ends.
    ui.set_focus(0);
    settle(&mut ui, |ui| page(&mut app, ui));
    key(&mut ui, Key::End, Mods::NONE, |ui| page(&mut app, ui));
    assert_settled(&mut ui, |ui| page(&mut app, ui));
    assert_eq!(app.grown, rect(0.0, 280.0, 410.0, 20.0), "at the end, the last of it (as small as it may be) is at the bottom of the window");
    key(&mut ui, Key::Home, Mods::NONE, |ui| page(&mut app, ui));
    assert_settled(&mut ui, |ui| page(&mut app, ui));
    assert_eq!(app.lies[0].y, 0.0);

    // A window made higher shows all of it again, with no bar.
    ui.resize(Size { w: 420.0, h: 700.0 }, 1.0);
    settle(&mut ui, |ui| page(&mut app, ui));
    assert_eq!((app.lies[19], app.grown), (rect(0.0, 570.0, 420.0, 30.0), rect(0.0, 600.0, 420.0, 100.0)));
}

// ---------------------------------------------------------------------------------------
// A box that scrolls as the app says.

#[derive(Default)]
struct Log {
    lines: usize,
    follow: bool,
    to: Option<[f32; 2]>,
    into_view: Option<usize>,
    scrolled: ScrollState,
    lies: Vec<(usize, Rect)>,
    ids: Vec<Id>,
}

fn log(app: &mut Log, ui: &mut Ui) {
    let t = ui.theme;
    app.lies.clear();
    app.ids.clear();
    let list = ui.column().grow().scroll().open();
    if app.follow {
        ui.stick_to_bottom(&list);
    }
    for line in 0..app.lines {
        let state = ui.el().key(line).h(Length::Px(20.0)).text(&format!("line {line}"), t.font, t.text).sense().show();
        app.lies.push((line, state.rect));
        app.ids.push(state.id);
    }
    ui.close();
    app.scrolled = ui.scroll_state(&list);
    if let Some(to) = app.to.take() {
        ui.scroll_to(&list, to);
    }
    if let Some(line) = app.into_view.take() {
        ui.scroll_into_view(app.ids[line]);
    }
}

#[test]
fn a_box_scrolls_where_the_app_says_and_stays_at_its_end_while_it_is_there() {
    let (mut app, mut ui) = (Log { lines: 30, follow: true, ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| log(&mut app, ui));
    // Thirty lines of 20 in 300: it starts at its end, and says how far it can go.
    assert_eq!(app.scrolled, ScrollState { offset: [0.0, 300.0], max_offset: [0.0, 300.0] });
    assert!(app.scrolled.at_end() && app.lies[29].1.bottom() == 300.0);
    assert_settled(&mut ui, |ui| log(&mut app, ui));
    // What comes in at the end pushes the rest up, at once.
    app.lines = 35;
    ui.run(&mut |ui| log(&mut app, ui));
    assert_eq!(app.scrolled.offset[1], 400.0);
    assert_settled(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[34].1.bottom(), 300.0);

    // Scrolled back by the person, it stays where it is as more comes.
    ui.event(Event::Move(pt(100.0, 100.0)));
    does(&mut ui, Event::Wheel(0.0, -100.0), |ui| log(&mut app, ui));
    assert!(!app.scrolled.at_end() && app.scrolled.offset[1] == 300.0);
    app.lines = 40;
    settle(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.scrolled.offset[1], 300.0, "it stays where it was put");
    // Taken back to its end by the app, it follows again.
    app.to = Some([f32::NAN, f32::INFINITY]);
    ui.run(&mut |ui| log(&mut app, ui));
    settle(&mut ui, |ui| log(&mut app, ui));
    assert!(app.scrolled.at_end() && app.lies[39].1.bottom() == 300.0);
    app.lines = 41;
    settle(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[40].1.bottom(), 300.0);

    // Anywhere, and to the top; past the end is the end.
    app.follow = false;
    app.to = Some([0.0, 120.0]);
    ui.run(&mut |ui| log(&mut app, ui));
    assert_eq!(app.scrolled.offset[1], 120.0, "in the run that asked");
    assert_settled(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[6].1.y, 0.0);
    app.to = Some([f32::NAN, 1e9]);
    settle(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.scrolled.offset[1], 520.0);
    app.to = Some([f32::NAN, 0.0]);
    settle(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[0].1.y, 0.0);
    // A box brought into view: from below, it comes in at the bottom; from above, at the top.
    app.into_view = Some(25);
    ui.run(&mut |ui| log(&mut app, ui));
    assert_settled(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[25].1.bottom(), 300.0);
    app.into_view = Some(3);
    ui.run(&mut |ui| log(&mut app, ui));
    assert_settled(&mut ui, |ui| log(&mut app, ui));
    assert_eq!(app.lies[3].1.y, 0.0);
}

// ---------------------------------------------------------------------------------------
// A long list whose rows are each as high as what is in them.

#[derive(Default)]
struct Chat {
    texts: Vec<String>,
    show: Option<usize>,
    follow: bool,
    described: Vec<(usize, Rect)>,
    scrolled: ScrollState,
}

fn chat(app: &mut Chat, ui: &mut Ui) {
    let t = ui.theme;
    app.described.clear();
    let list = ui.column().grow().scroll().open();
    if app.follow {
        ui.stick_to_bottom(&list);
    }
    if let Some(row) = app.show.take() {
        ui.scroll_to_row(row);
    }
    for row in ui.variable_rows(app.texts.len(), 20.0) {
        let lies = ui.el().key(row).w(Length::Grow(1.0)).text(&app.texts[row], t.font, t.text).wrap().sense().show().rect;
        app.described.push((row, lies));
    }
    ui.close();
    ui.close();
    app.scrolled = ui.scroll_state(&list);
}

/// A message of row `row`: of one, two or three lines in a window 420 wide (34 letters to a
/// line of the built-in ones, beside the bar).
fn message(row: usize) -> String {
    let words = "word ".repeat([3, 10, 17][row % 3]);
    format!("{row} {}", words.trim_end())
}

#[test]
fn a_long_list_of_rows_of_different_heights_describes_only_what_is_in_view() {
    let (mut app, mut ui) = (Chat { texts: (0..3000).map(message).collect(), ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| chat(&mut app, ui));
    // Each row is as high as its lines, one under the other, and only the ones in view are
    // described: with heights of 20, 40 and 60, rows 0 to 7 fill 300.
    let rows: Vec<usize> = app.described.iter().map(|(row, _)| *row).collect();
    assert_eq!(rows, (0..8).collect::<Vec<_>>());
    let heights: Vec<f32> = app.described.iter().map(|(_, lies)| lies.h).collect();
    assert_eq!(&heights[..6], [20.0, 40.0, 60.0, 20.0, 40.0, 60.0]);
    assert!(app.described.windows(2).all(|pair| pair[1].1.y == pair[0].1.bottom()), "no gaps, nothing over another: {:?}", app.described);
    assert_settled(&mut ui, |ui| chat(&mut app, ui));

    // A row far down is brought into view, as high as it is.
    app.show = Some(2000);
    settle(&mut ui, |ui| chat(&mut app, ui));
    let (_, lies) = *app.described.iter().find(|(row, _)| *row == 2000).expect("row 2000 is described");
    assert!(lies.bottom() <= 300.0 && lies.y >= 0.0 && lies.h == 60.0, "{lies:?}");
    assert!(app.described.len() < 20 && app.described.windows(2).all(|pair| pair[1].1.y == pair[0].1.bottom()));
    assert_settled(&mut ui, |ui| chat(&mut app, ui));

    // Scrolling up into rows never laid out keeps what shows where it is: the row at the
    // top of the window stays there as the ones over it are measured.
    let (top, at) = (app.described[1].0, app.described[1].1.y);
    ui.event(Event::Move(pt(100.0, 100.0)));
    does(&mut ui, Event::Wheel(0.0, -10.0), |ui| chat(&mut app, ui));
    settle(&mut ui, |ui| chat(&mut app, ui));
    let now = app.described.iter().find(|(row, _)| *row == top).unwrap().1.y;
    assert_eq!(now, at + 10.0, "row {top} went down by the wheel's ten, and no more");

    // At the end, it stays there as rows come; the last is at the bottom.
    app.follow = true;
    app.show = Some(2999);
    settle(&mut ui, |ui| chat(&mut app, ui));
    settle(&mut ui, |ui| chat(&mut app, ui));
    assert_eq!(app.described.last().map(|(row, lies)| (*row, lies.bottom())), Some((2999, 300.0)));
    app.texts.push("one more".into());
    ui.run(&mut |ui| chat(&mut app, ui));
    assert_eq!(app.described.last().map(|(row, lies)| (*row, lies.bottom())), Some((3000, 300.0)), "it came, and is in view at once");
    assert!(app.scrolled.at_end());
    assert_settled(&mut ui, |ui| chat(&mut app, ui));
}

// ---------------------------------------------------------------------------------------
// The wheel over a box of the app's own, and the room a box has.

#[derive(Default)]
struct Map {
    zoom: f32,
    room: Size,
    fitted: Rect,
    list: ScrollState,
}

fn map(app: &mut Map, ui: &mut Ui) {
    let t = ui.theme;
    let list = ui.column().grow().scroll().open();
    let ground = ui.el().size(300.0, 200.0).fill(t.panel).sense().show();
    if let Some([_, down]) = ui.wheel_delta(&ground) {
        app.zoom += down;
    }
    for line in 0..20 {
        ui.label(&format!("line {line}"));
    }
    ui.close();
    app.list = ui.scroll_state(&list);
    // A box fitted to the room it has: half of it across, a quarter of it down.
    ui.row().h(Length::Px(60.0)).pad(5.0).open();
    app.room = ui.available_size();
    app.fitted = ui.el().size(app.room.w / 2.0, app.room.h / 4.0).fill(t.accent).sense().show().rect;
    ui.close();
}

#[test]
fn the_wheel_over_a_box_is_the_apps_and_a_box_is_fitted_to_its_room_at_once() {
    let (mut app, mut ui) = (Map::default(), window(1.0));
    ui.run(&mut |ui| map(&mut app, ui));
    // The very first run fits the box to the room it turned out to have: the window was
    // described once more for that, and it is what shows.
    assert_eq!((app.room, ui.passes), (Size { w: 410.0, h: 50.0 }, 2));
    assert_settled(&mut ui, |ui| map(&mut app, ui));
    assert_eq!(app.fitted, rect(5.0, 263.75, 205.0, 12.5));
    // Over the app's box, the wheel is the app's, and nothing scrolls; beside it, the list scrolls.
    ui.event(Event::Move(pt(100.0, 100.0)));
    does(&mut ui, Event::Wheel(0.0, 48.0), |ui| map(&mut app, ui));
    assert_eq!((app.zoom, app.list.offset[1], ui.passes), (48.0, 0.0, 2));
    ui.event(Event::Move(pt(350.0, 100.0)));
    does(&mut ui, Event::Wheel(0.0, 48.0), |ui| map(&mut app, ui));
    assert_eq!((app.zoom, app.list.offset[1]), (48.0, 48.0));
    // A window of another size: the box is fitted to its new room in the same run.
    ui.resize(Size { w: 300.0, h: 300.0 }, 1.0);
    ui.run(&mut |ui| map(&mut app, ui));
    assert_eq!(app.room.w, 290.0);
    assert_settled(&mut ui, |ui| map(&mut app, ui));
    assert_eq!(app.fitted.w, 145.0);
}

// ---------------------------------------------------------------------------------------
// An app's own Edit menu and a page of text; Enter left to the app.

#[derive(Default)]
struct Editor {
    text: String,
    message: String,
    sent: Vec<String>,
    box_id: Id,
    command: Option<TextCommand>,
    select: Option<[usize; 2]>,
    typing: Option<taut::TextEditState>,
    page: Rect,
}

fn editor(app: &mut Editor, ui: &mut Ui) {
    ui.column().grow().gap(4.0).open();
    let page = ui.text_page(&mut app.text, "Write here");
    (app.box_id, app.page) = (page.id, page.rect);
    ui.enter_submits();
    let message = ui.text_area(&mut app.message, "Message", (1, 3));
    if message.focused && ui.key(Key::Enter, Mods::NONE) {
        app.sent.push(std::mem::take(&mut app.message));
    }
    ui.close();
    if let Some(command) = app.command.take() {
        ui.text_command(app.box_id, command);
    }
    if let Some(ends) = app.select.take() {
        ui.select_text(app.box_id, ends);
    }
    app.typing = ui.text_edit_state(app.box_id);
}

#[test]
fn an_apps_own_edit_menu_reaches_its_text_box_and_a_page_of_text_fills_its_room() {
    let (mut app, mut ui) = (Editor { text: "one two\nthree four\nfive".into(), ..Default::default() }, window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| editor(&mut app, ui));
    // The page fills what the box for a message leaves of the window (that one is a line
    // and its room, 32 high, and 4 apart).
    assert_eq!(app.page, rect(0.0, 0.0, 420.0, 264.0));
    assert!(app.typing.is_none(), "nothing is typed into yet");
    // A press under the last line puts the keyboard in it, at the end.
    click(&mut ui, pt(200.0, 200.0), |ui| editor(&mut app, ui));
    let typing = app.typing.expect("typed into");
    assert_eq!((typing.cursor(), typing.line_column(&app.text), typing.can_undo), (23, (3, 5), false));

    // A find that selects what it found: "three", on the second line.
    app.select = Some([8, 13]);
    settle(&mut ui, |ui| editor(&mut app, ui));
    let typing = app.typing.unwrap();
    assert_eq!((typing.range(), typing.line_column(&app.text)), (8..13, (2, 6)));
    let focused = focused_node(&ui).unwrap();
    assert_eq!(focused.caret, Some([8, 13]), "and a screen reader is told so");

    // Cut, then paste at the end, then undo and redo: as the menu of an app asks.
    app.command = Some(TextCommand::Cut);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(app.text, "one two\n four\nfive");
    assert!(app.typing.unwrap().can_undo);
    app.select = Some([app.text.len(), app.text.len()]);
    settle(&mut ui, |ui| editor(&mut app, ui));
    app.command = Some(TextCommand::Paste);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(app.text, "one two\n four\nfivethree");
    app.command = Some(TextCommand::Undo);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(app.text, "one two\n four\nfive");
    assert!(app.typing.unwrap().can_redo);
    app.command = Some(TextCommand::Redo);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(app.text, "one two\n four\nfivethree");
    app.command = Some(TextCommand::SelectAll);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(app.typing.unwrap().range(), 0..app.text.len());
    app.command = Some(TextCommand::Copy);
    settle(&mut ui, |ui| editor(&mut app, ui));
    assert_eq!(ui.clipboard().as_deref(), Some(app.text.as_str()));
    assert_settled(&mut ui, |ui| editor(&mut app, ui));

    // In the box for a message, Enter is the app's (it sends), and Shift and Enter start a
    // new line; the box is empty again at once.
    let message = expect_node(&ui, "Message").rect;
    click(&mut ui, middle(message), |ui| editor(&mut app, ui));
    typed(&mut ui, "Hello", |ui| editor(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::SHIFT, |ui| editor(&mut app, ui));
    typed(&mut ui, "there", |ui| editor(&mut app, ui));
    assert_eq!(app.message, "Hello\nthere");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| editor(&mut app, ui));
    assert_eq!((app.sent.as_slice(), app.message.as_str()), (&["Hello\nthere".to_string()][..], ""));
    assert_eq!(expect_node(&ui, "Message").value, "");
    assert_settled(&mut ui, |ui| editor(&mut app, ui));
}

// ---------------------------------------------------------------------------------------
// Pictures asked their size, and fitted to their room.

#[test]
fn pictures_are_asked_their_size_and_fitted_to_their_room_keeping_their_shape() {
    let mut ui = window(1.0);
    let (red, ground) = (Color::rgb(0xff0000), ui.theme.bg);
    // A picture twice as wide as it is high.
    let picture = ui.add_picture(40, vec![red.0 | 0xff00_0000; 40 * 20]);
    let pictures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
    let drawing = ui.add_drawing(&std::fs::read_to_string(pictures.join("logo.svg")).unwrap()).unwrap();
    let animation = ui.add_animation(&std::fs::read(pictures.join("moving.gif")).unwrap()).unwrap();
    assert_eq!(ui.picture_size(picture), Some((40, 20)));
    assert!(ui.drawing_size(drawing).is_some_and(|[w, h]| w > 0.0 && h > 0.0));
    let (wide, high) = ui.animation_size(animation).unwrap();
    assert_eq!(Some((wide, high)), taut::gif::decode(&std::fs::read(pictures.join("moving.gif")).unwrap()).map(|(width, pixels)| (width, pixels.len() / width)));

    // In a box 200 by 200 it is 200 by 100, in the middle; stretched, all of the box.
    let mut keep_aspect = true;
    let build = |ui: &mut Ui, keep_aspect: bool| {
        let shown = ui.el().picture(picture).size(200.0, 200.0);
        if keep_aspect {
            shown.keep_aspect().show()
        } else {
            shown.show()
        };
    };
    settle(&mut ui, |ui| build(ui, keep_aspect));
    assert_eq!((painted_pixels(&ui, rect(0.0, 0.0, 200.0, 50.0), ground), painted_pixels(&ui, rect(0.0, 150.0, 200.0, 50.0), ground)), (0, 0));
    assert_eq!(painted_pixels(&ui, rect(0.0, 50.0, 200.0, 100.0), ground), 200 * 100);
    keep_aspect = false;
    ui.run(&mut |ui| build(ui, keep_aspect));
    assert_eq!(painted_pixels(&ui, rect(0.0, 0.0, 200.0, 200.0), ground), 200 * 200);
    // Let go of, it has no size.
    ui.drop_picture(picture);
    assert_eq!(ui.picture_size(picture), None);
}

// ---------------------------------------------------------------------------------------
// What a tree's heading says it did.

#[derive(Default)]
struct Tree {
    open: [bool; 2],
    selected: usize,
    clicked: Vec<usize>,
    changed: Vec<usize>,
    heads: [Rect; 2],
}

fn tree(app: &mut Tree, ui: &mut Ui) {
    ui.column().pad(8.0).open();
    for (number, name) in ["Music", "Photos"].into_iter().enumerate() {
        let heading = ui.selectable_tree(&mut app.open[number], name, app.selected == number);
        app.heads[number] = heading.rect;
        if heading.clicked {
            (app.selected, app.clicked) = (number, [app.clicked.as_slice(), &[number]].concat());
        }
        if heading.changed {
            app.changed.push(number);
        }
        if app.open[number] {
            ui.label("a file");
            ui.close();
        }
    }
    ui.close();
}

#[test]
fn a_trees_heading_says_whether_it_was_pressed_and_whether_it_folded() {
    let (mut app, mut ui) = (Tree::default(), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| tree(&mut app, ui));
    // A press selects it and opens it: both are said.
    click(&mut ui, middle(app.heads[1]), |ui| tree(&mut app, ui));
    assert_eq!((app.clicked.as_slice(), app.changed.as_slice(), app.open, app.selected), (&[1][..], &[1][..], [false, true], 1));
    let photos = expect_node(&ui, "Photos");
    assert_eq!((photos.role, photos.on, photos.expanded), (Role::TreeItem, Some(true), Some(true)));
    // The arrows fold it and open it: changed, not pressed.
    key(&mut ui, Key::Left, Mods::NONE, |ui| tree(&mut app, ui));
    key(&mut ui, Key::Right, Mods::NONE, |ui| tree(&mut app, ui));
    assert_eq!((app.clicked.as_slice(), app.changed.as_slice(), app.open), (&[1][..], &[1, 1, 1][..], [false, true]));
    assert_settled(&mut ui, |ui| tree(&mut app, ui));
}
