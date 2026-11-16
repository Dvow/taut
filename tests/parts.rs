//! The larger parts as an app uses them: tables and long lists, submenus, panels, numbers,
//! colours, and drag and drop between boxes.
mod common;

use common::*;
use std::time::Duration;
use taut::geom::{Size, pt, rect};
use taut::{Align, Button, Chart, Color, Column, Date, Dock, Event, Key, Length, Mods, Panel, Picks, Rect, Role, Second, Series, Surface, Table, Ui};

/// How many pixels of the window's picture inside `area` are not `ground`.
fn inked(ui: &Window, area: Rect, ground: Color) -> usize {
    let across = area.x as usize..area.right() as usize;
    let rows = (area.y as usize..area.bottom() as usize).map(|y| &ui.screen.pixels[y * ui.screen.width..][across.clone()]);
    rows.flatten().filter(|pixel| **pixel != ground.0).count()
}

const COLUMNS: [Column; 3] = [Column::new("Name"), Column::new("Size").px(80.0).align(Align::End), Column::new("Kind").px(100.0).unsorted()];

#[derive(Default)]
struct Listing {
    table: Table,
    picks: Picks,
    sizes: Vec<u32>,
    /// The rows that were described, the cells of the first of them, and how often the app
    /// was told to sort.
    seen: Vec<usize>,
    cells: [Rect; 3],
    sorted: u32,
}

fn files(app: &mut Listing, ui: &mut Ui) {
    let t = ui.theme;
    app.sorted += ui.table(&mut app.table, &COLUMNS).changed as u32;
    app.seen = ui.rows(app.sizes.len(), 20.0).collect();
    for row in app.seen.clone() {
        if ui.pick_row(row, app.picks.has(row)).clicked {
            app.picks.click(row, ui.mods());
        }
        let cells = [ui.el().text(&format!("file {row}"), t.font, t.text).sense().show().rect, ui.el().text(&app.sizes[row].to_string(), t.font, t.text).sense().show().rect, ui.label("text").rect];
        if row == app.seen[0] {
            app.cells = cells;
        }
        ui.close();
    }
    ui.close();
    ui.close();
}

#[test]
fn a_table_lines_cells_up_under_headings_that_sort_and_are_dragged() {
    let (mut app, mut ui) = (Listing { sizes: (0..500).map(|n| n % 10).collect(), ..Default::default() }, window(1.0));
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(ui.passes, 3, "the first time, the table is described once more with the room it turned out to have, and once more with the strip its bar runs in left free");
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(ui.passes, 1, "and after that, once");

    // 420 wide, of which 10 are kept free for the bar down the side: 8 at each end and 13
    // after each column leave 355, of which Size has 80 and Kind 100, and Name the rest.
    // The rows start under the headings (28) and a line (1).
    assert_eq!(app.cells[0], rect(8.0, 29.0, 175.0, 20.0));
    assert_eq!((app.cells[1].x, app.cells[1].w), (196.0, 80.0));
    assert_eq!(app.seen, (0..14).collect::<Vec<_>>(), "271 high, rows of 20: fourteen of the five hundred");
    // What is in the Size column sits at its far end: one letter, 12 wide.
    let ground = ui.theme.bg;
    assert_eq!(inked(&ui, rect(196.0, 29.0, 66.0, 20.0), ground), 0, "nothing at the near end of the cell");
    assert!(inked(&ui, rect(264.0, 29.0, 12.0, 20.0), ground) > 0, "the number is at the far end");

    // A click on a heading sorts by it, a second one turns it round; Shift keeps what it
    // was sorted by; and a heading that does not sort does nothing.
    let (name, size, kind) = (pt(60.0, 14.0), pt(236.0, 14.0), pt(339.0, 14.0));
    click(&mut ui, size, |ui| files(&mut app, ui));
    assert_eq!((app.sorted, &app.table.sort), (1, &vec![(1, false)]));
    click(&mut ui, size, |ui| files(&mut app, ui));
    assert_eq!((app.sorted, &app.table.sort), (2, &vec![(1, true)]));
    for event in [Event::Move(name), Event::Press(Button::Left, Mods::SHIFT), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    assert_eq!(app.table.sort, [(1, true), (0, false)]);
    click(&mut ui, kind, |ui| files(&mut app, ui));
    assert_eq!(app.sorted, 3, "a heading that does not sort");

    // The grip after Name, taken 30 to the right, makes Name 30 wider; the others move along.
    // (The table is wider than its room then, and gets a bar to scroll it sideways by: that
    // too takes no more goes.)
    for event in [Event::Move(pt(189.0, 14.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(219.0, 14.0))] {
        does(&mut ui, event, |ui| files(&mut app, ui));
        assert!(ui.passes <= 2, "dragging a column takes two goes at most: {}", ui.passes);
    }
    does(&mut ui, Event::Release(Button::Left), |ui| files(&mut app, ui));
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!((app.table.widths[0], app.cells[0].w, app.cells[1].x), (205.0, 205.0, 226.0));
    // Clicked twice, the grip gives the column back the width it asked for.
    for at in [10, 200] {
        ui.set_time(Duration::from_millis(at));
        click(&mut ui, pt(219.0, 14.0), |ui| files(&mut app, ui));
    }
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!((app.table.widths[0], app.cells[0].w), (0.0, 175.0));

    // Rows are picked by a click between two cells: one, one more with Control, a stretch
    // with Shift.
    let row = |number: usize| pt(189.0, 29.0 + number as f32 * 20.0 + 10.0);
    for (at, mods) in [(row(2), Mods::NONE), (row(4), Mods::CTRL), (row(7), Mods::SHIFT)] {
        ui.set_time(Duration::from_secs(at.y as u64));
        for event in [Event::Move(at), Event::Press(Button::Left, mods), Event::Release(Button::Left)] {
            does(&mut ui, event, |ui| files(&mut app, ui));
        }
    }
    assert_eq!(app.picks.iter().collect::<Vec<_>>(), [4, 5, 6, 7]);

    // The wheel scrolls the rows, and the headings stay.
    does(&mut ui, Event::Wheel(0.0, 100.0), |ui| files(&mut app, ui));
    assert_eq!((app.seen[0], app.seen.len()), (5, 14));
    // In a narrower window, the column that grows gives way.
    ui.resize(Size { w: 300.0, h: 300.0 }, 1.0);
    ui.screen = taut::Surface::draw(&mut ui.ui, 64);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(ui.passes, 2, "described once more, for the room it has now");
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(app.cells[0].w, 55.0);
}

#[test]
fn rows_in_view_are_found_wherever_they_are_in_a_box_that_scrolls() {
    let (mut ui, mut seen) = (window(1.0), Vec::new());
    let build = |ui: &mut Ui, seen: &mut Vec<usize>| {
        ui.column().grow().scroll().open();
        ui.el().h(Length::Px(100.0)).show();
        *seen = ui.rows(1000, 20.0).collect();
        for row in seen.iter() {
            ui.row().key(row).open();
            ui.label(&format!("row {row}"));
            ui.close();
        }
        ui.close();
        ui.label("what comes after the rows");
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut seen));
    assert_eq!(seen, (0..10).collect::<Vec<_>>(), "the rows start 100 down: ten of them show in 300");
    does(&mut ui, Event::Move(pt(200.0, 200.0)), |ui| build(ui, &mut seen));
    does(&mut ui, Event::Wheel(0.0, 150.0), |ui| build(ui, &mut seen));
    assert_eq!(seen, (2..18).collect::<Vec<_>>(), "150 further down");
    assert!(ui.passes <= 2, "scrolling takes two goes: one to hear of it, one to show it");
    // A taller window shows more rows at once, without waiting for anything else to happen.
    ui.resize(Size { w: 420.0, h: 400.0 }, 1.0);
    ui.screen = taut::Surface::draw(&mut ui.ui, 64);
    ui.run(&mut |ui| build(ui, &mut seen));
    assert_eq!(seen, (2..23).collect::<Vec<_>>());
}

#[derive(Default)]
struct Menus {
    opened: u32,
    recent: &'static str,
    wrap: bool,
    /// Which menus were open in the last run, and where their parts were.
    file: bool,
    edit: bool,
    inner: bool,
    open: Rect,
    more: Rect,
    two: Rect,
    tick: Rect,
}

fn menus(app: &mut Menus, ui: &mut Ui) {
    (app.file, app.edit, app.inner) = (false, false, false);
    ui.menu_bar();
    if ui.menu_button("File") {
        app.file = true;
        let open = ui.menu_item("Open\tCtrl+O");
        (app.open, app.opened) = (open.rect, app.opened + open.clicked as u32);
        if ui.submenu("Recent") {
            app.inner = true;
            for name in ["one", "two"] {
                let item = ui.menu_item(name);
                app.two = item.rect;
                if item.clicked {
                    app.recent = name;
                }
            }
            ui.close();
        }
        app.tick = ui.menu_check(&mut app.wrap, "Wrap lines").rect;
        ui.close();
    }
    if ui.menu_button("Edit") {
        app.edit = true;
        ui.menu_item("Undo");
        ui.close();
    }
    ui.close();
}

#[test]
fn a_bar_of_menus_opens_menus_and_menus_out_of_them() {
    let (mut app, mut ui) = (Menus::default(), window(1.0));
    settle(&mut ui, |ui| menus(&mut app, ui));
    // "File" and "Edit" are four letters each, 9 of room either side, 2 apart, 4 from the edge.
    let (file, edit) = (pt(20.0, 14.0), pt(100.0, 14.0));
    click(&mut ui, file, |ui| menus(&mut app, ui));
    ui.run(&mut |ui| menus(&mut app, ui));
    assert!(app.file && !app.edit && app.open.y > 28.0, "the menu hangs under the bar: {:?}", app.open);
    // With one menu open, the pointer opens the next just by coming over its name.
    does(&mut ui, Event::Move(edit), |ui| menus(&mut app, ui));
    assert!(app.edit && !app.file);
    does(&mut ui, Event::Move(file), |ui| menus(&mut app, ui));
    ui.run(&mut |ui| menus(&mut app, ui));
    assert!(app.file && !app.edit);

    // The pointer rests on "Recent": its menu opens beside it, and the first one stays.
    app.more = rect(app.open.x, app.open.bottom(), app.open.w, app.open.h);
    does(&mut ui, Event::Move(middle(app.more)), |ui| menus(&mut app, ui));
    assert!(!app.inner, "not at once");
    ui.set_time(Duration::from_millis(200));
    settle(&mut ui, |ui| menus(&mut app, ui));
    assert!(app.file && app.inner && app.two.x >= app.more.right() - 1.0, "beside its row: {:?} {:?}", app.two, app.more);
    // Resting on another row of the first menu closes it again.
    does(&mut ui, Event::Move(middle(app.open)), |ui| menus(&mut app, ui));
    ui.set_time(Duration::from_millis(400));
    settle(&mut ui, |ui| menus(&mut app, ui));
    assert!(app.file && !app.inner);
    // Picking from the inner menu closes both.
    does(&mut ui, Event::Move(middle(app.more)), |ui| menus(&mut app, ui));
    ui.set_time(Duration::from_millis(600));
    settle(&mut ui, |ui| menus(&mut app, ui));
    click(&mut ui, middle(app.two), |ui| menus(&mut app, ui));
    assert_eq!((app.recent, app.file, app.inner), ("two", false, false));

    // The same by keyboard: the keyboard goes into the menu as it opens, onto its first
    // row; down to "Recent", Right into its menu, Left back out, then down to the tick,
    // which Enter flips.
    click(&mut ui, file, |ui| menus(&mut app, ui));
    for (pressed, inner) in [(Key::Down, false), (Key::Right, true), (Key::Down, true), (Key::Left, false), (Key::Down, false)] {
        key(&mut ui, pressed, Mods::NONE, |ui| menus(&mut app, ui));
        assert_eq!(app.inner, inner, "after {pressed:?}");
    }
    key(&mut ui, Key::Enter, Mods::NONE, |ui| menus(&mut app, ui));
    assert!(app.wrap && !app.file, "the tick is set, and the menu is closed");
    assert_eq!(app.opened, 0);

    // With a menu of the bar open, Right opens the next menu and Left the one before; and
    // from the last one, Right comes round to the first.
    click(&mut ui, file, |ui| menus(&mut app, ui));
    for (pressed, edit) in [(Key::Right, true), (Key::Right, false), (Key::Left, true), (Key::Left, false)] {
        key(&mut ui, pressed, Mods::NONE, |ui| menus(&mut app, ui));
        assert_eq!((app.file, app.edit), (!edit, edit), "after {pressed:?}");
    }
    key(&mut ui, Key::Escape, Mods::NONE, |ui| menus(&mut app, ui));
    assert!(!app.file && !app.edit);
}

#[derive(Default)]
struct Desk {
    tools: Option<Panel>,
    notes: Option<Panel>,
    pressed: u32,
    button: Rect,
    shown: [bool; 2],
}

fn desk(app: &mut Desk, ui: &mut Ui) {
    let (tools, notes) = (app.tools.get_or_insert(Panel::new(40.0, 50.0, 200.0, 150.0)), app.notes.get_or_insert(Panel::new(60.0, 100.0, 200.0, 150.0)));
    ui.label("the page under the panels");
    app.shown = [false; 2];
    if ui.panel(tools, "Tools") {
        let button = ui.button("In A");
        (app.shown[0], app.button, app.pressed) = (true, button.rect, app.pressed + button.clicked as u32);
        ui.close();
    }
    if ui.panel(notes, "Notes") {
        app.shown[1] = true;
        ui.label("B");
        ui.close();
    }
}

#[test]
fn panels_are_moved_brought_forward_stretched_folded_and_closed() {
    let (mut app, mut ui) = (Desk::default(), window(1.0));
    settle(&mut ui, |ui| desk(&mut app, ui));
    assert_eq!(app.shown, [true, true]);
    // The button in Tools starts 10 inside it, under the title (28) and a line (1).
    assert_eq!((app.button.x, app.button.y), (50.0, 89.0));
    // Notes was described last, so it lies in front: a click where it covers the button
    // goes to Notes.
    let covered = pt(80.0, 110.0);
    click(&mut ui, covered, |ui| desk(&mut app, ui));
    assert_eq!(app.pressed, 0);
    // A press on Tools brings it forward, and then the same click reaches its button.
    click(&mut ui, pt(150.0, 64.0), |ui| desk(&mut app, ui));
    click(&mut ui, covered, |ui| desk(&mut app, ui));
    assert_eq!(app.pressed, 1);

    // Taken by its title, the panel goes where the pointer goes: and nothing is laid out
    // again for that, so it takes a single go.
    for event in [Event::Move(pt(150.0, 64.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(180.0, 84.0))] {
        does(&mut ui, event, |ui| desk(&mut app, ui));
    }
    assert_eq!((app.tools.unwrap().rect, ui.passes), (rect(70.0, 70.0, 200.0, 150.0), 1));
    does(&mut ui, Event::Release(Button::Left), |ui| desk(&mut app, ui));
    ui.run(&mut |ui| desk(&mut app, ui));
    assert_eq!(app.button.x, 80.0, "what is in it went along");

    // Just outside its right edge, the edge is taken; near the corner, both edges are.
    drag(&mut ui, pt(272.0, 140.0), pt(312.0, 150.0), |ui| desk(&mut app, ui));
    assert_eq!(app.tools.unwrap().rect, rect(70.0, 70.0, 240.0, 150.0));
    drag(&mut ui, pt(312.0, 222.0), pt(302.0, 242.0), |ui| desk(&mut app, ui));
    assert_eq!(app.tools.unwrap().rect, rect(70.0, 70.0, 230.0, 170.0));
    drag(&mut ui, pt(68.0, 140.0), pt(300.0, 140.0), |ui| desk(&mut app, ui));
    assert_eq!(app.tools.unwrap().rect, rect(160.0, 70.0, 140.0, 170.0), "never narrower than a panel may be");

    // The arrow by the title folds it, and the cross closes it.
    click(&mut ui, pt(160.0 + 16.0, 84.0), |ui| desk(&mut app, ui));
    ui.run(&mut |ui| desk(&mut app, ui));
    assert!(app.tools.unwrap().folded && !app.shown[0]);
    click(&mut ui, pt(300.0 - 16.0, 84.0), |ui| desk(&mut app, ui));
    ui.run(&mut |ui| desk(&mut app, ui));
    assert!(!app.tools.unwrap().open && app.notes.unwrap().open);
}

#[test]
fn a_number_is_typed_and_stepped_and_a_colour_is_picked() {
    let mut ui = window(1.0);
    let (mut count, mut share, mut color, mut boxes) = (5i32, 0.5f32, Color::rgb(0x808080), [Rect::EMPTY; 3]);
    let build = |ui: &mut Ui, count: &mut i32, share: &mut f32, color: &mut Color, boxes: &mut [Rect; 3]| {
        ui.column().pad(10.0).gap(8.0).open();
        *boxes = [ui.color(color).rect, ui.number(count, 1).rect, ui.number(share, 0.25).rect];
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert!(boxes[1].w <= 120.0, "a box for a number does not take a whole row: {:?}", boxes[1]);

    // Typed into, the number changes as soon as what is typed is one.
    click(&mut ui, middle(boxes[1]), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    typed(&mut ui, "4", |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    typed(&mut ui, "2", |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(count, 42);
    typed(&mut ui, "x", |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(count, 42, "what is no number changes nothing");
    key(&mut ui, Key::Up, Mods::NONE, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    key(&mut ui, Key::Up, Mods::NONE, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    key(&mut ui, Key::Down, Mods::NONE, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(count, 43);
    // A number with a fraction, the same way.
    click(&mut ui, middle(boxes[2]), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    key(&mut ui, Key::Up, Mods::NONE, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(share, 0.75);
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    typed(&mut ui, "-1.5", |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(share, -1.5);

    // The swatch opens the picker under it: the square starts 10 inside that, 4 below the
    // swatch. Its top right corner is the hue at its strongest and lightest: for a grey, red.
    click(&mut ui, middle(boxes[0]), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    let square = rect(boxes[0].x + 10.0, boxes[0].bottom() + 4.0 + 10.0, 168.0, 120.0);
    click(&mut ui, pt(square.right() - 0.5, square.y + 0.5), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert!(color.0 >> 16 & 255 > 250 && color.0 >> 8 & 255 < 8 && color.0 & 255 < 8, "red: {color}");
    // A third of the way down the strip of hues is green; half way down the last strip, the
    // colour is half solid.
    click(&mut ui, pt(square.right() + 16.0, square.y + 40.0), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert!(color.0 >> 8 & 255 > 250 && color.0 >> 16 & 255 < 16 && color.0 & 255 < 16, "green: {color}");
    click(&mut ui, pt(square.right() + 40.0, square.y + 60.0), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert!((120..136).contains(&color.a()), "half solid: {color}");
    // And it can be typed, the way a web page writes it.
    click(&mut ui, pt(square.x + 40.0, square.bottom() + 8.0 + 14.0), |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    typed(&mut ui, "#112233", |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    assert_eq!(color, Color::rgb(0x112233));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
    ui.run(&mut |ui| build(ui, &mut count, &mut share, &mut color, &mut boxes));
}

#[test]
fn something_is_carried_from_one_box_and_let_go_over_another() {
    let mut ui = window(1.0);
    let (mut boxes, mut carrying, mut got) = ([Rect::EMPTY; 2], None, None);
    let build = |ui: &mut Ui, boxes: &mut [Rect; 2], carrying: &mut Option<u64>, got: &mut Option<u64>| {
        ui.column().pad(10.0).gap(40.0).open();
        let file = ui.selectable(false, "report.txt");
        if ui.carried(&file, 7) {
            ui.label("report.txt");
            ui.close();
        }
        let folder = ui.selectable(false, "Archive");
        *got = got.or(ui.dropped(&folder));
        (*boxes, *carrying) = ([file.rect, folder.rect], ui.carrying());
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut boxes, &mut carrying, &mut got));
    let (from, to) = (middle(boxes[0]), middle(boxes[1]));
    for (event, carried) in [
        (Event::Move(from), None),
        (Event::Press(Button::Left, Mods::NONE), None),
        (Event::Move(pt(from.x + 2.0, from.y)), None),
        (Event::Move(pt(from.x + 20.0, from.y + 10.0)), Some(7)),
        (Event::Move(to), Some(7)),
    ] {
        does(&mut ui, event, |ui| build(ui, &mut boxes, &mut carrying, &mut got));
        assert_eq!((carrying, got), (carried, None), "a little way is no carrying yet");
    }
    does(&mut ui, Event::Release(Button::Left), |ui| build(ui, &mut boxes, &mut carrying, &mut got));
    assert_eq!((carrying, got), (None, Some(7)), "let go over the folder");
    // Let go over nothing, it is just put down.
    got = None;
    drag(&mut ui, from, pt(300.0, 250.0), |ui| build(ui, &mut boxes, &mut carrying, &mut got));
    assert_eq!((carrying, got), (None, None));
}

/// Letters twice as large as the built-in ones: another way of writing, for a test.
struct Large;
impl taut::TextSystem for Large {
    fn metrics(&mut self, font: taut::Font, scale: f32) -> taut::text::Metrics {
        taut::text::BuiltInFont.metrics(taut::Font { size: font.size * 2.0, ..font }, scale)
    }
    fn measure(&mut self, text: &str, font: taut::Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        taut::text::BuiltInFont.measure(text, taut::Font { size: font.size * 2.0, ..font }, scale, stops)
    }
    fn draw(&mut self, canvas: &mut taut::Canvas, text: &str, x: f32, y: f32, font: taut::Font, color: Color) {
        taut::text::BuiltInFont.draw(canvas, text, x, y, taut::Font { size: font.size * 2.0, ..font }, color)
    }
}

#[test]
fn picture_files_are_shown_and_text_is_written_with_what_the_app_brings() {
    let mut ui = window(1.0);
    let (mut pictures, mut sizes) = (None, [Rect::EMPTY; 3]);
    let build = |ui: &mut Ui, pictures: &mut Option<[taut::Picture; 2]>, sizes: &mut [Rect; 3]| {
        let t = ui.theme;
        let read = |ui: &mut Ui, file: &[u8]| ui.add_picture_file(file).expect("the picture is read");
        let [photo, drawing] = *pictures.get_or_insert_with(|| [read(ui, include_bytes!("pictures/plain.jpg")), read(ui, include_bytes!("pictures/grey.png"))]);
        *sizes = [ui.el().picture(photo).sense().show().rect, ui.el().picture(drawing).sense().show().rect, ui.el().place(Align::Start).text("ab", t.font, t.text).sense().show().rect];
    };
    settle(&mut ui, |ui| build(ui, &mut pictures, &mut sizes));
    // A JPEG and a PNG, each as large as it has pixels; and two letters, 12 wide and 20 high.
    assert_eq!((sizes[0].size(), sizes[2].size()), (taut::Size { w: 67.0, h: 45.0 }, taut::Size { w: 24.0, h: 20.0 }));
    assert!(sizes[1].w > 0.0 && ui.add_picture_file(b"neither of the two").is_none());
    // Written with other letters, everything is laid out again for them.
    ui.set_text(Box::new(Large));
    settle(&mut ui, |ui| build(ui, &mut pictures, &mut sizes));
    assert_eq!(sizes[2].size(), taut::Size { w: 36.0, h: 30.0 }, "at twice the size, a dot of the built-in letters is 3 pixels in place of 2");
}

/// A font file of the system this runs on, if there is one where fonts usually are.
fn a_font_of_the_system() -> Option<taut::FontFile> {
    let places = ["C:\\Windows\\Fonts\\arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf"];
    places.iter().find_map(|place| std::fs::read(place).ok()).and_then(taut::FontFile::read)
}

#[test]
fn a_window_is_written_with_a_font_file() {
    // (Where the system has none of the fonts looked for, there is nothing to try this with.)
    let Some(font) = a_font_of_the_system() else { return };
    let mut ui = window(1.5);
    ui.set_text(Box::new(taut::OwnFonts::new(font)));
    let (mut name, mut label) = (String::from("Typed with a font file"), [Rect::EMPTY; 2]);
    let build = |ui: &mut Ui, name: &mut String, label: &mut [Rect; 2]| {
        let t = ui.theme;
        ui.column().pad(10.0).gap(8.0).open();
        label[0] = ui.el().place(Align::Start).text("The quick brown fox", t.font, t.text).sense().show().rect;
        ui.button("A button");
        label[1] = ui.text_box(name, "").rect;
        ui.paragraph("A paragraph that is long enough to break into more than one line in a window this narrow, to see lines of it wrap.");
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut name, &mut label));
    let ground = ui.theme.bg;
    let inked = ui.screen.pixels.iter().filter(|pixel| **pixel != ground.0).count();
    assert!(label[0].w > 60.0 && label[0].w < 200.0 && inked > 2000, "the label is {:?}, and {inked} pixels are drawn on", label[0]);
    // Typed into and clicked like any other window; and what changed is all that is drawn again.
    let boxed = middle(label[1]);
    click(&mut ui, boxed, |ui| build(ui, &mut name, &mut label));
    key(&mut ui, Key::End, Mods::NONE, |ui| build(ui, &mut name, &mut label));
    typed(&mut ui, " and more", |ui| build(ui, &mut name, &mut label));
    assert!(name.ends_with(" and more"));
}

#[test]
fn tabs_are_closed_and_put_in_another_order() {
    let mut ui = window(1.0);
    let (mut papers, mut on) = (vec!["a", "b", "c"], 0);
    let build = |ui: &mut Ui, papers: &mut Vec<&'static str>, on: &mut usize| _ = ui.tabs_of(on, papers, |paper| paper);
    settle(&mut ui, |ui| build(ui, &mut papers, &mut on));
    // Each tab is its name (12 wide), a cross (12), and room around them: 48 from one to the next.
    click(&mut ui, pt(99.0, 14.0), |ui| build(ui, &mut papers, &mut on));
    assert_eq!((on, papers.len()), (2, 3), "c is picked");
    // The cross on b takes it out; c is still the one that is picked.
    click(&mut ui, pt(76.0, 14.0), |ui| build(ui, &mut papers, &mut on));
    ui.run(&mut |ui| build(ui, &mut papers, &mut on));
    assert_eq!((papers.clone(), on), (vec!["a", "c"], 1));
    // a, carried past the middle of c, changes places with it; and c is still picked.
    for event in [Event::Move(pt(15.0, 14.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(40.0, 14.0)), Event::Move(pt(80.0, 14.0)), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| build(ui, &mut papers, &mut on));
    }
    assert_eq!((papers.clone(), on), (vec!["c", "a"], 0));
}

#[test]
fn files_from_another_program_are_let_go_over_a_box() {
    let mut ui = window(1.0);
    let (mut over, mut got, mut at) = (false, None, Rect::EMPTY);
    let build = |ui: &mut Ui, over: &mut bool, got: &mut Option<Vec<std::path::PathBuf>>, at: &mut Rect| {
        ui.column().pad(10.0).gap(100.0).open();
        let target = ui.selectable(false, "Let files go here");
        (*over, *at) = (ui.files_over(), target.rect);
        *got = ui.files_dropped(&target).or(got.take());
        ui.label("and not here");
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut over, &mut got, &mut at));
    let files = vec![std::path::PathBuf::from("one.txt"), std::path::PathBuf::from("two.txt")];
    // Carried over the window, they are known of; let go beside the box, they are not kept.
    for event in [Event::Carry(true), Event::Move(pt(200.0, 200.0))] {
        does(&mut ui, event, |ui| build(ui, &mut over, &mut got, &mut at));
    }
    assert!(over && got.is_none());
    does(&mut ui, Event::Files(files.clone()), |ui| build(ui, &mut over, &mut got, &mut at));
    ui.run(&mut |ui| build(ui, &mut over, &mut got, &mut at));
    assert!(!over && got.is_none(), "let go over nothing that asks");
    // Let go over the box, they are handed to the app, once.
    for event in [Event::Carry(true), Event::Move(middle(at)), Event::Files(files.clone())] {
        does(&mut ui, event, |ui| build(ui, &mut over, &mut got, &mut at));
    }
    assert_eq!((over, got.take()), (false, Some(files)));
    ui.run(&mut |ui| build(ui, &mut over, &mut got, &mut at));
    assert!(got.is_none());
}

#[test]
fn text_sits_where_it_is_told_and_dragging_never_runs_away() {
    let mut ui = window(1.0);
    let (mut wide, mut amount, number) = (120.0f32, 10i64, std::cell::Cell::new(Rect::EMPTY));
    let build = |ui: &mut Ui, wide: &mut f32, amount: &mut i64| {
        let t = ui.theme;
        ui.el().w(Length::Px(200.0)).text("ab", t.font, t.text).text_align(Align::End).show();
        ui.el().w(Length::Px(200.0)).text("ab", t.font, t.text).text_align(Align::Center).show();
        ui.row().grow().open();
        ui.el().w(Length::Px(*wide)).h(Length::Grow(1.0)).show();
        ui.splitter(wide, 60.0, 300.0);
        ui.column().pad(10.0).open();
        number.set(ui.drag(amount, 1.0).rect);
        ui.close();
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut wide, &mut amount));
    // Two letters, 24 wide, in boxes of 200: at the far end of the first, in the middle of
    // the second.
    let ground = ui.theme.bg;
    assert_eq!(inked(&ui, rect(0.0, 0.0, 176.0, 20.0), ground), 0);
    assert!(inked(&ui, rect(176.0, 0.0, 24.0, 20.0), ground) > 0);
    assert_eq!(inked(&ui, rect(0.0, 20.0, 88.0, 20.0), ground) + inked(&ui, rect(112.0, 20.0, 88.0, 20.0), ground), 0);
    assert!(inked(&ui, rect(88.0, 20.0, 24.0, 20.0), ground) > 0);

    // The bar between the panes follows the pointer exactly, in two goes for each move.
    for (event, expected) in [(Event::Move(pt(122.0, 150.0)), 120.0), (Event::Press(Button::Left, Mods::NONE), 120.0), (Event::Move(pt(152.0, 150.0)), 150.0), (Event::Move(pt(137.0, 150.0)), 135.0)] {
        does(&mut ui, event, |ui| build(ui, &mut wide, &mut amount));
        assert_eq!((wide, ui.passes <= 2), (expected, true), "{} goes", ui.passes);
    }
    does(&mut ui, Event::Release(Button::Left), |ui| build(ui, &mut wide, &mut amount));
    // A whole number is dragged like any other: one for every unit the pointer goes.
    ui.run(&mut |ui| build(ui, &mut wide, &mut amount));
    let at = middle(number.get());
    drag(&mut ui, at, pt(at.x + 25.0, at.y), |ui| build(ui, &mut wide, &mut amount));
    assert_eq!(amount, 35);
}

#[test]
fn a_fill_goes_evenly_from_one_colour_into_another() {
    let mut ui = window(1.0);
    let build = |ui: &mut Ui| {
        ui.el().size(101.0, 20.0).gradient_across(Color::rgb(0xff0000), Color::rgb(0x0000ff)).show();
        ui.el().size(100.0, 21.0).radius(8.0).gradient(Color::WHITE, Color::BLACK).show();
    };
    settle(&mut ui, build);
    let pixel = |x: usize, y: usize| ui.screen.pixels[y * ui.screen.width + x];
    // From left to right: red, half way between, blue; and the same all the way down.
    assert_eq!([pixel(0, 0), pixel(50, 10), pixel(100, 19)], [0xffff_0000, 0xff7f_007f, 0xff00_00ff]);
    // From top to bottom: white, grey, black; and where the corner is round, the page.
    assert_eq!([pixel(50, 20), pixel(50, 30), pixel(50, 40)], [0xffff_ffff, 0xff7f_7f7f, 0xff00_0000]);
    assert_eq!(pixel(0, 20), ui.theme.bg.0, "outside the round corner");

    // Three colours along a line from the bottom left corner to the top right one: red in
    // the one corner, blue in the other, green all along the other slant, which is half way.
    let (red, green, blue) = (Color::rgb(0xff0000), Color::rgb(0x00ff00), Color::rgb(0x0000ff));
    let build = |ui: &mut Ui| {
        ui.el().size(41.0, 41.0).gradient_along(45.0, &[(0.0, red), (0.5, green), (1.0, blue)]).show();
        ui.el().size(41.0, 41.0).gradient_around(&[(0.0, Color::WHITE), (1.0, Color::BLACK)]).show();
    };
    settle(&mut ui, build);
    let pixel = |x: usize, y: usize| ui.screen.pixels[y * ui.screen.width + x];
    assert_eq!([pixel(0, 40), pixel(0, 0), pixel(20, 20), pixel(40, 40), pixel(40, 0)], [red.0, green.0, green.0, green.0, blue.0]);
    // And from the middle outwards: white there, black at the middle of each edge and in
    // the corners beyond, the same grey the same way out in every direction.
    assert_eq!([pixel(20, 61), pixel(0, 61), pixel(20, 41), pixel(40, 81), pixel(0, 41)], [0xffff_ffff, 0xff00_0000, 0xff00_0000, 0xff00_0000, 0xff00_0000]);
    assert_eq!([pixel(10, 61), pixel(30, 61), pixel(20, 51)], [pixel(20, 71); 3]);
}

#[test]
fn text_that_does_not_fit_a_box_that_clips_ends_in_dots() {
    let mut ui = window(1.0);
    let build = |ui: &mut Ui| {
        let t = ui.theme;
        // Ten letters, 12 wide each: in a box of 60 that clips, and in one they just fit.
        ui.el().w(Length::Px(60.0)).clip().text("abcdefghij", t.font, t.text).show();
        ui.el().w(Length::Px(120.0)).clip().text("abcdefghij", t.font, t.text).show();
    };
    settle(&mut ui, build);
    let ground = ui.theme.bg;
    // Four letters fit before the dots, which sit low in the fifth letter's place.
    assert!(inked(&ui, rect(0.0, 0.0, 48.0, 20.0), ground) > 0);
    assert_eq!(inked(&ui, rect(48.0, 0.0, 12.0, 12.0), ground), 0, "nothing above the dots");
    assert!(inked(&ui, rect(48.0, 12.0, 12.0, 8.0), ground) > 0, "the dots");
    assert_eq!(inked(&ui, rect(60.0, 0.0, 60.0, 20.0), ground), 0, "and nothing past the box");
    // Where all of it fits, all of it is there: the last letter too.
    assert!(inked(&ui, rect(108.0, 20.0, 12.0, 20.0), ground) > 0);
    assert!(inked(&ui, rect(48.0, 20.0, 12.0, 12.0), ground) > 0, "and no dots in place of the fifth");
}

#[test]
fn a_date_is_picked_from_a_calendar_with_the_keyboard() {
    let mut ui = window(1.0);
    let (mut date, mut at) = (Date::new(2026, 11, 11), Rect::EMPTY);
    let build = |ui: &mut Ui, date: &mut Date, at: &mut Rect| {
        ui.column().pad(10.0).open();
        *at = ui.date(date).rect;
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut date, &mut at));
    // The box opens its calendar with the keyboard on the day that is picked. Ten days
    // back is the first of the month; Enter picks it, and the calendar closes.
    click(&mut ui, middle(at), |ui| build(ui, &mut date, &mut at));
    for pressed in [Key::Left; 10].into_iter().chain([Key::Enter]) {
        key(&mut ui, pressed, Mods::NONE, |ui| build(ui, &mut date, &mut at));
    }
    assert_eq!(date, Date::new(2026, 11, 1));
    // Page Down goes to the same day of the next month; a week down from it is its eighth.
    click(&mut ui, middle(at), |ui| build(ui, &mut date, &mut at));
    for pressed in [Key::PageDown, Key::Down, Key::Enter] {
        key(&mut ui, pressed, Mods::NONE, |ui| build(ui, &mut date, &mut at));
    }
    assert_eq!(date, Date::new(2026, 12, 8));
    assert!(Date::today() > Date::new(2026, 1, 1), "the computer knows what day it is");
}

#[test]
fn the_columns_of_a_table_are_moved_and_hidden() {
    let (mut app, mut ui) = (Listing { sizes: (0..50).collect(), ..Default::default() }, window(1.0));
    settle(&mut ui, |ui| files(&mut app, ui));
    let across = |app: &Listing| [app.cells[0].x, app.cells[1].x];
    // (The table has a bar down its side, and its columns share what that leaves.)
    assert_eq!(across(&app), [8.0, 196.0], "Name, then Size, as the app gives them");

    // The heading of Name, taken past the middle of the heading of Size (which is at 236):
    // the two change places. And that was no click: nothing is sorted.
    for event in [Event::Move(pt(60.0, 14.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(150.0, 14.0)), Event::Move(pt(250.0, 14.0)), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!((&app.table.order, app.sorted), (&vec![1, 0, 2], 0));
    assert_eq!(across(&app), [101.0, 8.0], "Size, then Name");

    // A right click on a heading lists the columns, with the keyboard on the first; it
    // goes down to Size, and Enter hides it. Name takes the room it leaves, and nothing of
    // its cells shows.
    for event in [Event::Move(pt(350.0, 14.0)), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    for pressed in [Key::Down, Key::Enter] {
        key(&mut ui, pressed, Mods::NONE, |ui| files(&mut app, ui));
    }
    settle(&mut ui, |ui| files(&mut app, ui));
    assert_eq!(app.table.hidden, [1]);
    assert_eq!((app.cells[0], app.cells[1].w), (rect(8.0, 29.0, 268.0, 20.0), 0.0));
    // The last column that shows cannot be hidden; and a column shows again when asked.
    app.table.hidden = vec![1, 2];
    settle(&mut ui, |ui| files(&mut app, ui));
    for event in [Event::Move(pt(100.0, 14.0)), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    key(&mut ui, Key::Enter, Mods::NONE, |ui| files(&mut app, ui));
    assert_eq!(app.table.hidden, [1, 2], "Name is the only one left: it stays");
}

const WIDE: [Column; 4] = [Column::new("Name").px(150.0), Column::new("One").px(150.0), Column::new("Two").px(150.0), Column::new("Three").px(150.0)];

/// A table of four columns of 150: with the room around them 668 wide, in a window of 420.
/// It has `count` rows; what comes back is where the cells of the first row in view are,
/// and the last cell of the last.
fn wide(ui: &mut Ui, table: &mut Table, count: usize) -> ([Rect; 4], Rect) {
    let (t, mut first, mut last) = (ui.theme, None, Rect::EMPTY);
    ui.table(table, &WIDE);
    for row in ui.rows(count, 20.0) {
        ui.table_row().key(row).open();
        let cells = ["a", "XXXXXXXXXXXX", "c", "d"].map(|word| ui.el().text(word, t.font, t.text).sense().show().rect);
        (first, last) = (first.or(Some(cells)), cells[3]);
        ui.close();
    }
    ui.close();
    ui.close();
    (first.unwrap_or_default(), last)
}

#[test]
fn a_wide_table_is_scrolled_sideways_under_the_columns_it_keeps_in_view() {
    let (mut ui, mut table, mut cells) = (window(1.0), Table { kept: 1, ..Default::default() }, [Rect::EMPTY; 4]);
    let across = |cells: [Rect; 4]| cells.map(|cell| (cell.x, cell.w));
    settle(&mut ui, |ui| cells = wide(ui, &mut table, 3).0);
    // 8 before the first column, 13 between two; the window ends in the third, before the fourth.
    assert_eq!(across(cells), [(8.0, 150.0), (171.0, 150.0), (334.0, 86.0), (0.0, 0.0)]);

    // The wheel, sideways by 100. Name stays; One is 100 under it, and its other 50 show;
    // the rest has come 100 nearer.
    does(&mut ui, Event::Move(pt(200.0, 100.0)), |ui| cells = wide(ui, &mut table, 3).0);
    does(&mut ui, Event::Wheel(100.0, 0.0), |ui| cells = wide(ui, &mut table, 3).0);
    assert!(ui.passes <= 2, "scrolling takes two goes: one to hear of it, one to show it");
    ui.run(&mut |ui| cells = wide(ui, &mut table, 3).0);
    assert_eq!(across(cells), [(8.0, 150.0), (171.0, 50.0), (234.0, 150.0), (397.0, 23.0)]);
    // Of the row of X that starts under Name, nothing shows there: only after it. And the
    // same goes for the headings.
    let (ground, panel) = (ui.theme.bg, ui.theme.panel);
    assert_eq!(inked(&ui, rect(21.0, 29.0, 150.0, 20.0), ground), 0, "what lies under the column that is kept does not show");
    assert!(inked(&ui, rect(171.0, 29.0, 44.0, 20.0), ground) > 0, "what sticks out from under it does");
    assert_eq!(inked(&ui, rect(60.0, 4.0, 95.0, 20.0), panel), 0, "the heading of One is under the heading of Name");

    // The bar along the bottom of the rows, taken all the way to the right and back: the
    // table is 248 wider than its room. One is all under Name then, and Two 85 of its 150.
    for to in [pt(700.0, 295.0), pt(-300.0, 295.0), pt(700.0, 295.0)] {
        drag(&mut ui, pt(200.0, 295.0), to, |ui| cells = wide(ui, &mut table, 3).0);
        ui.run(&mut |ui| cells = wide(ui, &mut table, 3).0);
        let there = if to.x > 0.0 { [(8.0, 150.0), (0.0, 0.0), (171.0, 65.0), (249.0, 150.0)] } else { [(8.0, 150.0), (171.0, 150.0), (334.0, 86.0), (0.0, 0.0)] };
        assert_eq!(across(cells), there, "with the bar taken to {to:?}");
    }

    // The keyboard: Tab to the heading of Name changes nothing. On to the heading of One,
    // which is under Name: the table is scrolled back until all of One shows. On to Two,
    // whose end is out of the window: scrolled until it is not.
    for (shown, scrolled) in [(3, 248.0), (1, 0.0), (2, 72.0)] {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| cells = wide(ui, &mut table, 3).0);
        ui.run(&mut |ui| cells = wide(ui, &mut table, 3).0);
        let at = 8.0 + shown as f32 * 163.0 - scrolled;
        assert_eq!(cells[shown].x, at, "after Tab, the table is scrolled by {scrolled}");
    }

    // With the first column hidden, the next one is the one that is kept.
    table.hidden = vec![0];
    settle(&mut ui, |ui| cells = wide(ui, &mut table, 3).0);
    assert_eq!(across(cells), [(0.0, 0.0), (8.0, 150.0), (171.0, 78.0), (262.0, 150.0)]);
}

#[test]
fn an_icon_in_a_cell_keeps_its_size_at_the_side_its_column_says() {
    const SIDES: [Column; 4] = [Column::new("Start").px(60.0), Column::new("Middle").px(60.0).align(Align::Center), Column::new("End").px(40.0).align(Align::End), Column::new("Narrow").px(24.0)];
    const DASH: taut::Icon = taut::Icon::line("M2 8h12");
    let (mut ui, mut table, mut cells) = (window(1.0), Table::default(), [Rect::EMPTY; 4]);
    let build = |ui: &mut Ui, table: &mut Table, cells: &mut [Rect; 4]| {
        let ink = ui.theme.text;
        ui.table(table, &SIDES);
        ui.table_row().open();
        *cells = [16.0, 16.0, 16.0, 40.0].map(|size| ui.el().icon(DASH, size, ink).sense().show().rect);
        ui.close();
        ui.close();
    };
    settle(&mut ui, |ui| build(ui, &mut table, &mut cells));
    // The columns start at 8, 81, 154 and 207. An icon is not made as wide as its column:
    // it sits at the start of it, in its middle, or at its end. And one that is too wide
    // for its column shows as much of itself as the column has room for.
    assert_eq!(cells.map(|cell| (cell.x, cell.w, cell.h)), [(8.0, 16.0, 16.0), (103.0, 16.0, 16.0), (178.0, 16.0, 16.0), (207.0, 24.0, 40.0)]);
    let ground = ui.theme.bg;
    assert_eq!(inked(&ui, rect(231.0, 29.0, 40.0, 40.0), ground), 0, "nothing of the wide icon shows past its column");
    assert!(inked(&ui, rect(207.0, 29.0, 24.0, 40.0), ground) > 0);
}

#[test]
fn a_large_picture_shown_small_is_the_mean_of_all_of_it() {
    // 512 by 512, black and white pixel by pixel, shown 20 by 20 at a scale of one and a
    // half: 30 pixels a side, each standing for some seventeen by seventeen of the picture.
    // Picked out pixel by pixel it would come out in stripes; as the mean of what falls on
    // each pixel it is one even grey.
    let checks: Vec<u32> = (0..512 * 512).map(|i| if (i % 512 + i / 512) % 2 == 0 { 0xff00_0000 } else { 0xffff_ffff }).collect();
    let (mut ui, mut picture, mut at) = (window(1.5), None, Rect::EMPTY);
    settle(&mut ui, |ui| {
        let shown = *picture.get_or_insert_with(|| ui.add_picture(512, checks.clone()));
        at = ui.el().picture(shown).size(20.0, 20.0).sense().show().rect;
    });
    let rows = (0..30).map(|y| &ui.screen.pixels[y * ui.screen.width..][..30]);
    let greys: Vec<u32> = rows.flatten().map(|pixel| pixel & 255).collect();
    assert_eq!(at.size(), Size { w: 20.0, h: 20.0 });
    assert!(greys.iter().all(|grey| (0x7c..=0x83).contains(grey)), "{greys:x?}");
    // New pixels in its place show at once, at the same small size.
    ui.set_picture(picture.unwrap(), 512, vec![0xffff_ffff; 512 * 512]);
    ui.run(&mut |ui| at = ui.el().picture(picture.unwrap()).size(20.0, 20.0).sense().show().rect);
    assert!(ui.screen.pixels[..30].iter().all(|pixel| pixel & 0xff_ffff == 0xff_ffff));
}

#[test]
fn a_second_window_is_an_interface_of_its_own() {
    let (mut ui, mut prefs, mut dark, mut pressed) = (window(1.0), Second::new(200.0, 120.0).at(640, 480).least(150.0, 90.0), false, 0);
    let asked_for_dark = std::cell::Cell::new([false; 2]);
    let build = |ui: &mut Ui, prefs: &mut Second, dark: &mut bool, pressed: &mut u32| {
        *pressed += ui.button("In the first window").clicked as u32;
        let first = ui.system_dark();
        if ui.window(prefs, "Preferences") {
            asked_for_dark.set([first, ui.system_dark()]);
            ui.checkbox(dark, "Dark");
            ui.close();
        }
        ui.label("and the first window goes on");
    };
    // Whether the person asked the system for dark windows, the second window knows as
    // the first does.
    settle(&mut ui, |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!(asked_for_dark.get(), [false; 2]);
    ui.set_wishes(taut::Wishes { dark: true, ..Default::default() });
    settle(&mut ui, |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!(asked_for_dark.get(), [true; 2]);
    // One second window, known by its title and as big as the app asked. It is drawn by
    // itself; and the first window, 420 by 300, holds nothing of it.
    let found: Vec<(u64, String, Size)> = ui.windows().map(|(id, title, size)| (id, title.to_string(), size)).collect();
    let [(id, title, size)] = &found[..] else { panic!("{} second windows", found.len()) };
    assert_eq!((title.as_str(), *size), ("Preferences", Size { w: 200.0, h: 120.0 }));
    // Whoever shows it is told where its app wants it and how small it may be made; and
    // once it says where the person has put the window, the app hears of that. So does the
    // app of the first window, of its own.
    let opens = ui.window_opens(*id).unwrap();
    assert_eq!((opens.least, opens.at, opens.kind, opens.bare), (Size { w: 150.0, h: 90.0 }, Some((640, 480)), taut::WindowKind::Plain, false));
    let there = taut::Placed { at: (700, 500), width: 200.0, height: 120.0, maximized: false };
    assert_eq!((ui.in_window(*id, |window| window.set_placed(there)), ui.in_window(*id, |window| window.set_placed(there))), (Some(true), Some(false)));
    ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!((prefs.at, ui.placed()), (Some((700, 500)), None));
    assert!(ui.set_placed(taut::Placed { maximized: true, ..there }) && ui.placed().is_some_and(|place| place.maximized));
    let again = taut::Window { title: "Again", ..Default::default() }.placed(ui.placed());
    assert_eq!((again.at, again.width, again.height, again.maximized), (Some((700, 500)), 200.0, 120.0, true));
    let mut picture = ui.in_window(*id, |window| Surface::draw(window, 64)).unwrap();
    assert_eq!((picture.width, picture.height, ui.screen.width, ui.screen.height), (200, 120, 420, 300));

    // A click at the top left of the second window ticks its box, and the first window's
    // button, which is at the same place in its own window, is not pressed. Only what
    // changed in the second window is drawn again there, and nothing in the first.
    for event in [Event::Move(pt(12.0, 14.0)), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
        ui.in_window(*id, |window| window.event(event));
        let first = ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
        let changed = ui.in_window(*id, |window| window.changed()).unwrap();
        ui.in_window(*id, |window| picture.redraw(window, changed));
        let afresh = ui.in_window(*id, |window| Surface::draw(window, 64)).unwrap();
        assert!(first.is_empty() && picture.pixels == afresh.pixels, "the first window changed in {first:?}, the second in {changed:?}");
    }
    assert_eq!((dark, pressed), (true, 0));
    // And the other way round.
    click(&mut ui, pt(12.0, 14.0), |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!((dark, pressed), (true, 1));
    assert!(ui.in_window(*id, |window| window.changed()).unwrap().is_empty(), "nothing changed in the second window");

    // The person makes the second window larger: the app is told. Then closes it: the app
    // is told, and the window is gone. The app opens it again, and closes it itself.
    ui.in_window(*id, |window| window.resize(Size { w: 300.0, h: 150.0 }, 1.0));
    ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!((prefs.width, prefs.height, prefs.open), (300.0, 150.0, true));
    ui.in_window(*id, |window| window.ask_to_close());
    ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!((prefs.open, ui.windows().count()), (false, 0));
    prefs.open = true;
    ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!(ui.windows().map(|(known, ..)| known).collect::<Vec<_>>(), [*id], "open again, and known as before");
    prefs.open = false;
    ui.run(&mut |ui| build(ui, &mut prefs, &mut dark, &mut pressed));
    assert_eq!(ui.windows().count(), 0);

    // A window that closes itself is gone by the end of the run in which it said so.
    let (mut note, quits) = (Second::new(100.0, 50.0), std::cell::Cell::new(false));
    let closing = |ui: &mut Ui, note: &mut Second| {
        if ui.window(note, "Note") {
            if quits.replace(false) {
                ui.quit();
            }
            ui.close();
        }
    };
    settle(&mut ui, |ui| closing(ui, &mut note));
    assert_eq!((note.open, ui.windows().count()), (true, 1));
    quits.set(true);
    ui.run(&mut |ui| closing(ui, &mut note));
    assert_eq!((note.open, ui.windows().count()), (false, 0));
}

#[test]
fn a_text_box_shows_no_bar_however_long_its_text_is() {
    let (mut ui, mut text, mut at) = (window(1.0), " ".repeat(80), Rect::EMPTY);
    settle(&mut ui, |ui| at = ui.text_box(&mut text, "").rect);
    // Eighty spaces are 960 wide, in a box of 420: it scrolls sideways, and is too low for
    // a bar to be of use. Nothing is drawn on it but its edge, with its round corners.
    let panel = ui.theme.panel;
    assert_eq!(inked(&ui, rect(at.x + 4.0, at.y + 4.0, at.w - 8.0, at.h - 8.0), panel), 0);
}

#[test]
fn a_box_that_scrolls_both_ways_shows_its_end_clear_of_both_bars() {
    let (mut ui, mut table, mut last) = (window(1.0), Table::default(), Rect::EMPTY);
    settle(&mut ui, |ui| last = wide(ui, &mut table, 500).1);
    does(&mut ui, Event::Move(pt(200.0, 100.0)), |ui| last = wide(ui, &mut table, 500).1);
    does(&mut ui, Event::Wheel(1e6, 1e6), |ui| last = wide(ui, &mut table, 500).1);
    settle(&mut ui, |ui| last = wide(ui, &mut table, 500).1);
    // The rows are 271 high and 420 wide, with a bar of 10 along the bottom and one down
    // the side: the last of the five hundred rows ends where the bar along the bottom
    // starts, and its last cell 21 (what is between cells, and at the end of a row) before
    // the bar down the side.
    assert_eq!((last.right(), last.bottom()), (410.0 - 21.0, 290.0));
    // Each bar is taken back to its start by itself. (The fourth column starts at 497.)
    drag(&mut ui, pt(300.0, 295.0), pt(-300.0, 295.0), |ui| last = wide(ui, &mut table, 500).1);
    settle(&mut ui, |ui| last = wide(ui, &mut table, 500).1);
    assert_eq!((last, table.kept), (Rect::EMPTY, 0), "the last column is out of the window again");
    does(&mut ui, Event::Move(pt(200.0, 100.0)), |ui| last = wide(ui, &mut table, 500).1);
    does(&mut ui, Event::Wheel(1e6, 0.0), |ui| last = wide(ui, &mut table, 500).1);
    drag(&mut ui, pt(415.0, 280.0), pt(415.0, -300.0), |ui| last = wide(ui, &mut table, 500).1);
    settle(&mut ui, |ui| last = wide(ui, &mut table, 500).1);
    assert_eq!((last.right(), last.y), (389.0, 29.0 + 13.0 * 20.0), "back at the top, fourteen rows show and the last of them starts here");
}

/// A dock of three panels, the panels that show, and where what is in each of them is.
struct Bench {
    dock: Dock,
    shown: Vec<&'static str>,
    inside: [Rect; 3],
}

fn bench(app: &mut Bench, ui: &mut Ui) {
    app.shown.clear();
    ui.dock(&mut app.dock);
    for (number, name) in ["Files", "Editor", "Log"].into_iter().enumerate() {
        if ui.docked(&mut app.dock, name) {
            app.shown.push(name);
            app.inside[number] = ui.el().w(Length::Grow(1.0)).h(Length::Px(20.0)).sense().show().rect;
            ui.close();
        }
    }
    ui.close();
}

#[test]
fn panels_share_a_dock_and_are_carried_from_place_to_place() {
    let dock = Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Log"]), 0.25);
    let (mut app, mut ui) = (Bench { dock, shown: Vec::new(), inside: [Rect::EMPTY; 3] }, window(1.0));
    settle(&mut ui, |ui| bench(&mut app, ui));
    // A quarter of the width for Files, less the bar; the rest for Editor, with Log behind it.
    assert_eq!(app.shown, ["Files", "Editor"]);
    assert_eq!((app.inside[0], app.inside[1]), (rect(10.0, 40.0, 84.0, 20.0), rect(119.0, 40.0, 291.0, 20.0)));
    // A click on the tab of Log brings it in front of Editor.
    click(&mut ui, pt(235.0, 15.0), |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(app.shown, ["Files", "Log"]);
    // The bar between the two places, taken 50 to the right, gives Files that much more.
    drag(&mut ui, pt(106.0, 150.0), pt(156.0, 150.0), |ui| bench(&mut app, ui));
    ui.run(&mut |ui| bench(&mut app, ui));
    assert_eq!(app.inside[0].w, 134.0);

    // The tab of Log is carried to the mark at the right of the place of Files, and let
    // go: Log now lies between Files and Editor, and all three show.
    // (Its tab is 50 further right than it was, with the place it is in.)
    for event in [Event::Move(pt(285.0, 15.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(285.0, 60.0)), Event::Move(pt(113.0, 150.0)), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!(app.shown, ["Files", "Editor", "Log"]);
    assert!(app.inside[0].x < app.inside[2].x && app.inside[2].x < app.inside[1].x, "{:?}", app.inside);

    // The tab of Editor, let go on no mark, floats where it was let go: and the others
    // take the room it had.
    for event in [Event::Move(pt(209.0, 15.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(250.0, 100.0)), Event::Move(pt(300.0, 250.0)), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert_eq!(app.dock.floating.len(), 1);
    assert_eq!((app.shown.len(), app.inside[2].right()), (3, 410.0), "Log reaches the far side now");
    // Taken by its title and let go on the middle mark of the place of Files, it joins
    // the tabs there, in front.
    for event in [Event::Move(pt(300.0, 250.0)), Event::Press(Button::Left, Mods::NONE), Event::Move(pt(200.0, 200.0)), Event::Move(pt(104.0, 150.0)), Event::Release(Button::Left)] {
        does(&mut ui, event, |ui| bench(&mut app, ui));
    }
    settle(&mut ui, |ui| bench(&mut app, ui));
    assert!(app.dock.floating.is_empty());
    assert_eq!(app.shown, ["Editor", "Log"], "Files is behind Editor now");
}

/// A window with nothing in it but a box for a secret, after `secret` was typed into it:
/// its picture.
fn hidden(secret: &str) -> Vec<u32> {
    let (mut ui, mut text, mut at) = (window(1.0), String::new(), Rect::EMPTY);
    settle(&mut ui, |ui| at = ui.secret_box(&mut text, "").rect);
    click(&mut ui, middle(at), |ui| _ = ui.secret_box(&mut text, ""));
    typed(&mut ui, secret, |ui| _ = ui.secret_box(&mut text, ""));
    ui.screen.pixels.clone()
}

#[test]
fn a_box_for_a_secret_shows_dots_and_lets_nothing_out() {
    let (mut ui, mut text, mut at) = (window(1.0), String::new(), Rect::EMPTY);
    ui.listen(true);
    settle(&mut ui, |ui| at = ui.secret_box(&mut text, "Password").rect);
    click(&mut ui, middle(at), |ui| _ = ui.secret_box(&mut text, "Password"));
    // Characters of one byte, two, three and four: the app gets what was typed.
    typed(&mut ui, "pä€𝄞", |ui| _ = ui.secret_box(&mut text, "Password"));
    assert_eq!(text, "pä€𝄞");
    // A screen reader hears of four dots, not of what they stand for.
    let told = ui.said().iter().find(|said| said.role == Role::Secret).unwrap();
    assert_eq!((told.name.as_str(), told.value.as_str(), told.caret), ("Password", "••••", Some([4, 4])));
    // Neither copying nor cutting takes it out of the box.
    for letter in ['a', 'c', 'x'] {
        key(&mut ui, Key::Char(letter), Mods::CTRL, |ui| _ = ui.secret_box(&mut text, "Password"));
    }
    assert_eq!((text.as_str(), ui.clipboard()), ("pä€𝄞", None));
    // A press just past the second dot (the dots start 8 in, and are 12 wide) puts the
    // cursor after the second character, whatever bytes the two are.
    click(&mut ui, pt(at.x + 8.0 + 25.0, at.y + at.h / 2.0), |ui| _ = ui.secret_box(&mut text, "Password"));
    typed(&mut ui, "x", |ui| _ = ui.secret_box(&mut text, "Password"));
    assert_eq!(text, "päx€𝄞");
    // And the picture is of dots: the same for any characters, as many as there are.
    assert!(hidden("abcd") == hidden("WXY€"), "four characters look like any other four");
    assert!(hidden("abcd") != hidden("abc"), "but not like three");
}

/// A box 200 high that scrolls: a heading kept at its top, forty rows of 20 each in the
/// accent colour, and a foot kept at its bottom. Says where the heading and the foot are,
/// and each row (one that is out of view is nowhere), and notes a row that is clicked.
fn ledger(ui: &mut Ui, rows: &mut [Rect; 40], clicked: &mut Option<usize>) -> [Rect; 2] {
    let t = ui.theme;
    ui.column().key("ledger").h(Length::Px(200.0)).scroll().open();
    let head = ui.el().key("head").kept().h(Length::Px(30.0)).fill(t.panel).sense().show().rect;
    for (row, place) in rows.iter_mut().enumerate() {
        let state = ui.el().key(row).h(Length::Px(20.0)).fill(t.accent).sense().focusable().show();
        *place = state.rect;
        if state.clicked {
            *clicked = Some(row);
        }
    }
    let foot = ui.el().key("foot").kept().h(Length::Px(30.0)).fill(t.panel).sense().show().rect;
    ui.close();
    [head, foot]
}

#[test]
fn a_box_that_is_typed_into_has_a_menu_and_opens_with_its_text_selected() {
    // A box of one line with a name in it, and one of several lines.
    let (mut name, mut note, mut boxes) = ("old name".to_string(), "one".to_string(), [taut::Response::default(); 2]);
    let mut first = true;
    let form = |ui: &mut Ui, name: &mut String, note: &mut String, boxes: &mut [taut::Response; 2], first: &mut bool| {
        ui.column().grow().pad(10.0).gap(10.0).open();
        boxes[0] = ui.text_box(name, "A name");
        boxes[1] = ui.text_area(note, "A note", (2, 6));
        // The first time, the keyboard is put on the name, with all of it selected.
        if std::mem::take(first) {
            ui.set_focus(boxes[0].id);
            ui.select_all();
        }
        ui.close();
    };
    let mut ui = window(1.0);
    ui.listen(true);
    settle(&mut ui, |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    // What is typed takes the place of the name: it was selected.
    typed(&mut ui, "new", |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    assert_eq!(name, "new");

    // The right button on the box: a menu of four rows. With nothing selected there is
    // nothing to cut or copy, and "Select all" selects; then "Copy" copies, as its key does.
    let menu = |ui: &mut Window, name: &mut String, note: &mut String, boxes: &mut [taut::Response; 2], first: &mut bool, row: &str| {
        for event in [Event::Move(middle(boxes[0].rect)), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)] {
            does(ui, event, |ui| form(ui, name, note, boxes, first));
        }
        settle(ui, |ui| form(ui, name, note, boxes, first));
        let rows: Vec<String> = ui.said().iter().filter(|said| said.role == Role::MenuItem).map(|said| said.name.clone()).collect();
        assert_eq!(rows, ["Cut", "Copy", "Paste", "Select all"]);
        let at = ui.said().iter().find(|said| said.name == row).map(|said| middle(said.rect)).unwrap();
        click(ui, at, |ui| form(ui, name, note, boxes, first));
        settle(ui, |ui| form(ui, name, note, boxes, first));
        // (A row that is switched off takes no click, and leaves the menu open: Escape shuts it.)
        if ui.said().iter().any(|said| said.role == Role::MenuItem) {
            key(ui, Key::Escape, Mods::NONE, |ui| form(ui, name, note, boxes, first));
        }
    };
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Copy");
    assert_eq!(ui.clipboard(), None, "nothing was selected: nothing is copied");
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Select all");
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Copy");
    assert_eq!(ui.clipboard().as_deref(), Some("new"));
    // "Cut" takes it out; "Paste" puts it back.
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Select all");
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Cut");
    assert_eq!(name, "");
    menu(&mut ui, &mut name, &mut note, &mut boxes, &mut first, "Paste");
    assert_eq!(name, "new");

    // In the box of several lines, Enter starts a new line, and so does Enter with Shift.
    click(&mut ui, middle(boxes[1].rect), |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    key(&mut ui, Key::End, Mods::CTRL, |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    key(&mut ui, Key::Enter, Mods::SHIFT, |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    typed(&mut ui, "two", |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| form(ui, &mut name, &mut note, &mut boxes, &mut first));
    assert_eq!(note, "one\ntwo\n");
}

#[test]
fn text_is_written_with_a_line_through_it_or_under_it() {
    // Two letters with three spaces between them, 10 from the corner: each letter is 12
    // wide and the line 20 high, so from 24 to 54 across there is nothing but the spaces.
    let words = |ui: &mut Ui, font: taut::text::Font| {
        let ink = ui.theme.text;
        ui.column().grow().pad(10.0).open();
        ui.el().text("a   b", font, ink).show();
        ui.close();
    };
    let (mut ui, ground) = (window(1.0), Color::rgb(0));
    let (font, ground) = (ui.theme.font, [ui.theme.bg, ground][0]);
    // The rows of pixels, of the line's twenty, in which something is drawn across the spaces.
    let ruled = |ui: &Window| (10..30).filter(|row| inked(ui, rect(24.0, *row as f32, 30.0, 1.0), ground) == 30).collect::<Vec<_>>();
    settle(&mut ui, |ui| words(ui, font));
    assert_eq!(ruled(&ui), [0; 0], "plain text draws nothing between its words");
    // Struck through: one row, through the middle of the small letters (which stand on the
    // 16th row of the line and are half as tall). Underlined: one row, just under the letters.
    settle(&mut ui, |ui| words(ui, font.struck()));
    let through = ruled(&ui);
    settle(&mut ui, |ui| words(ui, font.underlined()));
    let under = ruled(&ui);
    assert_eq!((through.len(), under.len()), (1, 1), "{through:?} {under:?}");
    assert!((18..24).contains(&through[0]) && (26..30).contains(&under[0]), "{through:?} {under:?}");
    // And plain again leaves nothing of either behind.
    settle(&mut ui, |ui| words(ui, font));
    assert_eq!(ruled(&ui), [0; 0]);
}

#[test]
fn a_box_knows_the_pointer_is_over_it_unless_another_layer_lies_between() {
    // A row with a button on it; and a menu that opens over both when it is asked for.
    let form = |ui: &mut Ui, seen: &mut [taut::Response; 2], open: &mut bool| {
        ui.column().grow().pad(10.0).open();
        seen[0] = ui.row().key("row").w(Length::Grow(1.0)).h(Length::Px(40.0)).pad(6.0).sense().open();
        seen[1] = ui.button("On the row");
        ui.close();
        if std::mem::take(open) {
            ui.open_popup(seen[0].id, rect(seen[0].rect.x, seen[0].rect.y - 4.0, 0.0, 0.0));
        }
        if ui.menu(seen[0].id) {
            ui.menu_item("One");
            ui.menu_item("Two");
            ui.close();
        }
        ui.close();
    };
    let (mut ui, mut seen, mut open) = (window(1.0), [taut::Response::default(); 2], false);
    settle(&mut ui, |ui| form(ui, &mut seen, &mut open));
    // The pointer on the button: the button is what it is on, and it is over the row too.
    does(&mut ui, Event::Move(middle(seen[1].rect)), |ui| form(ui, &mut seen, &mut open));
    settle(&mut ui, |ui| form(ui, &mut seen, &mut open));
    assert_eq!((seen[0].hovered, seen[0].over, seen[1].hovered, seen[1].over), (false, true, true, true));
    // The menu opens over them, with the pointer where it was: now it is over neither.
    open = true;
    settle(&mut ui, |ui| form(ui, &mut seen, &mut open));
    settle(&mut ui, |ui| form(ui, &mut seen, &mut open));
    assert_eq!((seen[0].over, seen[1].hovered, seen[1].over), (false, false, false));
    // And with the menu gone, it is over them again.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| form(ui, &mut seen, &mut open));
    settle(&mut ui, |ui| form(ui, &mut seen, &mut open));
    assert_eq!((seen[0].over, seen[1].over), (true, true));
}

#[test]
fn the_page_keys_scroll_what_the_wheel_would() {
    // Fifty rows of 30 in a box 300 high: 1500, of which 300 show.
    let rows = |ui: &mut Ui, seen: &mut [Rect; 50]| {
        ui.column().grow().scroll().open();
        for (row, place) in seen.iter_mut().enumerate() {
            *place = ui.el().key(row).h(Length::Px(30.0)).sense().show().rect;
        }
        ui.close();
    };
    // How far the box is scrolled: by as much as a row that shows whole lies above where it
    // would with nothing scrolled.
    let (mut ui, mut first) = (window(1.0), [Rect::EMPTY; 50]);
    let offset = |ui: &mut Window, seen: &mut [Rect; 50]| {
        settle(ui, |ui| rows(ui, seen));
        seen.iter().enumerate().find(|(_, place)| place.h == 30.0).map(|(row, place)| row as f32 * 30.0 - place.y).unwrap()
    };
    assert_eq!(offset(&mut ui, &mut first), 0.0);
    // Page Down goes a page less two lines: 260. Twice, 520. Page Up, back to 260.
    for (pressed, to) in [(Key::PageDown, 260.0), (Key::PageDown, 520.0), (Key::PageUp, 260.0), (Key::End, 1200.0), (Key::PageDown, 1200.0), (Key::Home, 0.0), (Key::PageUp, 0.0)] {
        key(&mut ui, pressed, Mods::NONE, |ui| rows(ui, &mut first));
        assert_eq!(offset(&mut ui, &mut first), to, "after {pressed:?}");
    }
}

#[test]
fn what_a_box_that_scrolls_holds_keeps_clear_of_its_bar() {
    // Rows of 30 in a box that scrolls, 420 wide and 300 high. Where is the first of them?
    let rows = |ui: &mut Ui, count: usize, first: &mut Rect| {
        ui.column().grow().scroll().open();
        for row in 0..count {
            let state = ui.el().key(row).h(Length::Px(30.0)).sense().show();
            *first = if row == 0 { state.rect } else { *first };
        }
        ui.close();
    };
    let (mut ui, mut first) = (window(1.0), Rect::EMPTY);
    // Five rows fit: they are as wide as the box, and it has no bar. (Where a box is, it
    // says the next time it is described: so each time, once more to ask.)
    ui.run(&mut |ui| rows(ui, 5, &mut first));
    ui.run(&mut |ui| rows(ui, 5, &mut first));
    assert_eq!((first.w, ui.passes), (420.0, 1));
    // Twenty do not: the box gets a bar down its side, 10 wide, and the rows end where it
    // starts. That takes one more go than usual, and nothing is drawn in between.
    ui.run(&mut |ui| rows(ui, 20, &mut first));
    assert_eq!(ui.passes, 2);
    ui.run(&mut |ui| rows(ui, 20, &mut first));
    assert_eq!((first.w, ui.passes), (410.0, 1));
    // The bar is drawn in the strip that is left free: something is there that is not the page.
    let ground = ui.theme.bg;
    assert!(inked(&ui, rect(410.0, 0.0, 10.0, 300.0), ground) > 0);
    // And with the bar gone, the rows have all of the box again.
    ui.run(&mut |ui| rows(ui, 5, &mut first));
    assert_eq!(ui.passes, 2);
    ui.run(&mut |ui| rows(ui, 5, &mut first));
    assert_eq!((first.w, ui.passes), (420.0, 1));
}

#[test]
fn boxes_kept_in_view_stay_put_and_the_rest_scrolls_between_them() {
    let (mut ui, mut rows, mut clicked, mut ends) = (window(1.0), [Rect::EMPTY; 40], None, [Rect::EMPTY; 2]);
    // (The box is 420 wide, and keeps the 10 its bar runs in free of what it holds.)
    let kept = [rect(0.0, 0.0, 410.0, 30.0), rect(0.0, 170.0, 410.0, 30.0)];
    settle(&mut ui, |ui| ends = ledger(ui, &mut rows, &mut clicked));
    // The heading at the top of the 200 and the foot at their bottom; seven rows between.
    assert_eq!(ends, kept);
    assert_eq!((rows[0].y, rows[6].y, rows[7].is_empty()), (30.0, 150.0, true), "the eighth row is under the foot");

    // The wheel, down by 100: the two stay, and the sixth row is the first under the heading.
    does(&mut ui, Event::Move(pt(200.0, 100.0)), |ui| ends = ledger(ui, &mut rows, &mut clicked));
    does(&mut ui, Event::Wheel(0.0, 100.0), |ui| ends = ledger(ui, &mut rows, &mut clicked));
    ui.run(&mut |ui| ends = ledger(ui, &mut rows, &mut clicked));
    assert_eq!(ends, kept);
    assert_eq!((rows[4].is_empty(), rows[5].y, rows[11].y, rows[12].is_empty()), (true, 30.0, 150.0, true));
    // Nothing of a row is drawn on the heading or the foot, and between them nothing else
    // is (but where the bar runs, down the right, over all three).
    let (panel, accent) = (ui.theme.panel, ui.theme.accent);
    assert_eq!((inked(&ui, rect(0.0, 0.0, 400.0, 30.0), panel), inked(&ui, rect(0.0, 170.0, 400.0, 30.0), panel)), (0, 0));
    assert_eq!(inked(&ui, rect(0.0, 30.0, 400.0, 140.0), accent), 0);
    // A press on the heading reaches the heading and not the row under it; one just below
    // it reaches the row that shows there.
    click(&mut ui, pt(50.0, 15.0), |ui| ends = ledger(ui, &mut rows, &mut clicked));
    assert_eq!(clicked, None);
    click(&mut ui, pt(50.0, 35.0), |ui| ends = ledger(ui, &mut rows, &mut clicked));
    assert_eq!(clicked, Some(5));

    // The keyboard is on that row now. Ten rows on, it is on one that was out of view: the
    // rows are scrolled until that one is clear of the foot. Twelve back, until that one is
    // clear of the heading.
    for (steps, back, row, at) in [(10, false, 15, 150.0), (12, true, 3, 30.0)] {
        for _ in 0..steps {
            key(&mut ui, Key::Tab, Mods { shift: back, ..Mods::NONE }, |ui| ends = ledger(ui, &mut rows, &mut clicked));
        }
        ui.run(&mut |ui| ends = ledger(ui, &mut rows, &mut clicked));
        assert_eq!((rows[row].y, ends), (at, kept), "with the keyboard on row {row}");
    }

    // All the way down: the last row ends where the foot starts.
    does(&mut ui, Event::Wheel(0.0, 10_000.0), |ui| ends = ledger(ui, &mut rows, &mut clicked));
    ui.run(&mut |ui| ends = ledger(ui, &mut rows, &mut clicked));
    assert_eq!((rows[39].y, rows[32].is_empty(), ends), (150.0, true, kept));
}

/// The wide table with a row kept over its hundred rows and one kept under them. Says
/// where the cells of the two are, and of the first of the other rows in view.
fn summed(ui: &mut Ui, table: &mut Table) -> [[Rect; 4]; 3] {
    let (t, mut cells) = (ui.theme, [[Rect::EMPTY; 4]; 3]);
    let row = |ui: &mut Ui, words: [&str; 4]| words.map(|word| ui.el().text(word, t.font, t.text).sense().show().rect);
    ui.table(table, &WIDE);
    ui.table_row().key("first").kept().h(Length::Px(20.0)).fill(t.panel).open();
    cells[0] = row(ui, ["k", "XXXXXXXXXXXX", "c", "d"]);
    ui.close();
    for number in ui.rows(100, 20.0) {
        ui.table_row().key(number).open();
        let these = row(ui, ["a", "b", "c", "d"]);
        if cells[1] == [Rect::EMPTY; 4] && !these[0].is_empty() {
            cells[1] = these;
        }
        ui.close();
    }
    ui.close();
    ui.table_row().key("sums").kept().h(Length::Px(20.0)).fill(t.panel).open();
    cells[2] = row(ui, ["s", "XXXXXXXXXXXX", "c", "d"]);
    ui.close();
    ui.close();
    cells
}

#[test]
fn rows_of_a_table_are_kept_in_view_over_and_under_the_others() {
    let (mut ui, mut table, mut cells) = (window(1.0), Table { kept: 1, ..Default::default() }, [[Rect::EMPTY; 4]; 3]);
    let across = |cells: [Rect; 4]| cells.map(|cell| (cell.x, cell.w));
    let down = |cells: [[Rect; 4]; 3]| cells.map(|row| row[0].y);
    settle(&mut ui, |ui| cells = summed(ui, &mut table));
    // Under the headings (which end at 29) the row that is kept, then the others; and the
    // other one that is kept ends where the bar along the bottom starts, 10 from the end.
    assert_eq!(down(cells), [29.0, 49.0, 270.0]);

    // Down by 100: the two stay, and what was the sixth of the others is the first in view.
    does(&mut ui, Event::Move(pt(200.0, 100.0)), |ui| cells = summed(ui, &mut table));
    does(&mut ui, Event::Wheel(0.0, 100.0), |ui| cells = summed(ui, &mut table));
    ui.run(&mut |ui| cells = summed(ui, &mut table));
    assert_eq!(down(cells), [29.0, 49.0, 270.0]);
    // Sideways by 100: all three go, each under its first cell, which is of the column
    // that is kept; and the rows that are kept still lie where they did.
    does(&mut ui, Event::Wheel(100.0, 0.0), |ui| cells = summed(ui, &mut table));
    ui.run(&mut |ui| cells = summed(ui, &mut table));
    for row in cells {
        // (The last cell ends where the bar down the side starts, 10 from the edge.)
        assert_eq!(across(row), [(8.0, 150.0), (171.0, 50.0), (234.0, 150.0), (397.0, 13.0)]);
    }
    assert_eq!(down(cells), [29.0, 49.0, 270.0]);
    // Of the X that start under the first column, nothing shows there in either row.
    let panel = ui.theme.panel;
    assert_eq!((inked(&ui, rect(21.0, 29.0, 150.0, 20.0), panel), inked(&ui, rect(21.0, 270.0, 150.0, 20.0), panel)), (0, 0));
    assert!(inked(&ui, rect(171.0, 29.0, 44.0, 20.0), panel) > 0 && inked(&ui, rect(171.0, 270.0, 44.0, 20.0), panel) > 0);
}

/// A text system that counts how often it is asked where the characters of a text end:
/// which is what laying out a text that wraps asks.
struct Counted(std::rc::Rc<std::cell::Cell<usize>>);

impl taut::TextSystem for Counted {
    fn metrics(&mut self, font: taut::Font, scale: f32) -> taut::text::Metrics {
        taut::text::BuiltInFont.metrics(font, scale)
    }
    fn measure(&mut self, text: &str, font: taut::Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.0.set(self.0.get() + stops.is_some() as usize);
        taut::text::BuiltInFont.measure(text, font, scale, stops)
    }
    fn draw(&mut self, canvas: &mut taut::Canvas, text: &str, x: f32, y: f32, font: taut::Font, color: Color) {
        taut::text::BuiltInFont.draw(canvas, text, x, y, font, color)
    }
}

#[test]
fn a_window_is_laid_out_once_for_what_changed_and_for_its_new_size_together() {
    let asked = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut ui = Ui::new(Box::new(Counted(asked.clone())), Box::new(taut::LocalClipboard::default()));
    let long = "words upon words that do not fit on one line of a window as narrow as this one is";
    // The first time the window is described it is laid out: once.
    ui.resize(Size { w: 200.0, h: 200.0 }, 1.0);
    ui.run(&mut |ui| _ = ui.paragraph(long));
    assert_eq!(asked.replace(0), 1);
    // Described the same again, it is not laid out at all.
    ui.run(&mut |ui| _ = ui.paragraph(long));
    assert_eq!(asked.replace(0), 0);
    // Resized, with something else said in it: once for both, and not again after.
    ui.resize(Size { w: 180.0, h: 200.0 }, 1.0);
    ui.run(&mut |ui| _ = ui.paragraph(&long[6..]));
    assert_eq!(asked.replace(0), 1);
    ui.run(&mut |ui| _ = ui.paragraph(&long[6..]));
    assert_eq!(asked.replace(0), 0);
    // Resized with nothing else said: once more.
    ui.resize(Size { w: 160.0, h: 200.0 }, 1.0);
    ui.run(&mut |ui| _ = ui.paragraph(&long[6..]));
    assert_eq!(asked.replace(0), 1);
}

/// A square floor, ten units each way, lying flat with its middle at nothing: two triangles.
fn floor() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (vec![[-5.0, 0.0, -5.0], [5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [-5.0, 0.0, 5.0]], vec![[0, 1, 2], [0, 2, 3]])
}

#[test]
fn a_scene_shows_bodies_of_triangles_and_is_drawn_again_only_when_it_changes() {
    let (red, ground) = (Color::rgb(0xd03030), Color::rgb(0x101418));
    for scale in [1.0, 2.0] {
        let mut ui = window(scale);
        let (points, triangles) = floor();
        let body = ui.add_mesh(points, triangles);
        // Seen from straight above, as a plan that is twenty units from bottom to top, in a
        // box 300 by 200 at (40, 30): ten pixels to a unit, the floor a square of a hundred
        // in the middle of the box.
        let plan = taut::View::look([0.0, 50.0, 0.0], [0.0, 0.0, 0.0]).up([0.0, 0.0, 1.0]).flat(20.0);
        let says = |ui: &mut Ui, view: &taut::View, shown: &[taut::Shown]| {
            ui.row().pad((40.0, 30.0)).open();
            ui.el().size(300.0, 200.0).fill(ground).scene(view, shown).show();
            ui.close();
        };
        let (pixels, wide) = (|ui: &Window| ui.screen.pixels.clone(), |ui: &Window| ui.screen.width);
        ui.run(&mut |ui| says(ui, &plan, &[body.filled(red).unlit()]));
        let all = pixels(&ui);
        let at = |x: f32, y: f32| all[(y * scale) as usize * wide(&ui) + (x * scale) as usize];
        assert_eq!((at(140.0, 80.0), at(239.5, 179.5), at(139.0, 130.0), at(240.0, 130.0), at(190.0, 79.0), at(190.0, 180.0)), (red.0, red.0, ground.0, ground.0, ground.0, ground.0));
        assert_eq!(all.iter().filter(|pixel| **pixel == red.0).count(), (100.0 * scale * 100.0 * scale) as usize);
        // Described the same again, nothing is drawn.
        assert_eq!(ui.run(&mut |ui| says(ui, &plan, &[body.filled(red).unlit()])), Rect::EMPTY);
        // Seen from twice as far, the box is drawn again, and the floor is half as large.
        let further = plan.flat(40.0);
        assert_eq!(ui.run(&mut |ui| says(ui, &further, &[body.filled(red).unlit()])), rect(40.0, 30.0, 300.0, 200.0));
        assert_eq!(pixels(&ui).iter().filter(|pixel| **pixel == red.0).count(), (50.0 * scale * 50.0 * scale) as usize);
        // Another colour, another body in its place, a colour for each point: each is seen.
        let blue = Color::rgb(0x3050d0);
        assert_eq!(ui.run(&mut |ui| says(ui, &plan, &[body.filled(blue).unlit()])), rect(40.0, 30.0, 300.0, 200.0));
        ui.set_mesh(body, vec![[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0]], vec![[0, 1, 2]]);
        assert_eq!(ui.run(&mut |ui| says(ui, &plan, &[body.filled(blue).unlit()])), rect(40.0, 30.0, 300.0, 200.0));
        // (Half of a square of twenty: but for the pixels whose middles lie on its long edge,
        // which may go either way.)
        let half = pixels(&ui).iter().filter(|pixel| **pixel == blue.0).count();
        assert!(half.abs_diff((20.0 * scale * 20.0 * scale / 2.0) as usize) <= (10.0 * scale) as usize, "{half}");
        ui.color_mesh(body, vec![Color::rgb(0xff0000), Color::rgb(0x00ff00), Color::rgb(0x0000ff)]);
        assert_eq!(ui.run(&mut |ui| says(ui, &plan, &[body.filled(blue).unlit()])), rect(40.0, 30.0, 300.0, 200.0));
        assert_eq!(pixels(&ui).iter().filter(|pixel| **pixel == blue.0).count(), 0);
    }
}

#[test]
fn a_point_of_a_scene_is_found_in_the_window_and_outlines_run_where_the_edges_are() {
    let mut ui = window(1.0);
    let (points, triangles) = floor();
    let body = ui.add_mesh(points, triangles);
    let (white, panel) = (Color::WHITE, Color::rgb(0x203040));
    let plan = taut::View::look([0.0, 50.0, 0.0], [0.0, 0.0, 0.0]).up([0.0, 0.0, 1.0]).flat(20.0);
    // The box has no colour of its own and round corners: the scene is laid over the row's.
    let says = |ui: &mut Ui, view: &taut::View| {
        ui.row().pad((40.0, 30.0)).fill(panel).open();
        ui.el().size(300.0, 200.0).radius(8.0).scene(view, &[body.outlined(white)]).show();
        ui.close();
    };
    ui.run(&mut |ui| says(ui, &plan));
    let within = rect(40.0, 30.0, 300.0, 200.0);
    // Its corners are where the plan says: across the picture x runs to the left, seen from above with z up.
    let corner = |point: [f32; 3]| plan.spot(point, within).expect("in front of the eye");
    assert_eq!((corner([5.0, 0.0, 5.0]), corner([-5.0, 0.0, -5.0]), corner([0.0, 0.0, 0.0])), (pt(140.0, 80.0), pt(240.0, 180.0), pt(190.0, 130.0)));
    // The outline runs round the floor and along the edge its two triangles share, and
    // nowhere else: what lies between is the row's colour.
    let at = |x: usize, y: usize| ui.screen.pixels[y * ui.screen.width + x];
    let lit = |x: usize, y: usize| at(x, y) >> 8 & 255 > 0x60;
    assert!((142..238).all(|x| (lit(x, 79) || lit(x, 80)) && (lit(x, 179) || lit(x, 180))) && (82..178).all(|y| (lit(139, y) || lit(140, y)) && (lit(239, y) || lit(240, y))));
    assert!((145..235).all(|step| lit(step, step - 60) || lit(step, step - 61)));
    assert_eq!((at(170, 100), at(210, 160), at(100, 60)), (panel.0, panel.0, panel.0));
    // From where a person would stand on it, looking along it: the floor is under the
    // horizon, cut off at the eye, and what is behind the eye is found nowhere.
    let standing = taut::View::look([0.0, 1.7, 0.0], [0.0, 1.7, 4.0]);
    ui.run(&mut |ui| says(ui, &standing));
    let rows = |from: usize, to: usize| (from..to).map(|y| (40..340).filter(|x| ui.screen.pixels[y * ui.screen.width + x] != panel.0).count()).sum::<usize>();
    assert!(rows(30, 128) == 0 && rows(134, 230) > 300, "{} above, {} below", rows(30, 128), rows(134, 230));
    assert!(standing.spot([0.0, 0.0, -3.0], within).is_none() && standing.spot([0.0, 1.7, 5.0], within) == Some(pt(190.0, 130.0)));
}

#[test]
fn a_picture_and_a_body_that_are_let_go_show_nothing_and_their_places_are_used_again() {
    let mut ui = window(1.0);
    let (red, green, ground) = (Color::rgb(0xd03030), Color::rgb(0x30d030), Color::rgb(0x101418));
    ui.theme.bg = ground;
    // A picture of twenty by ten red pixels, shown at the top left; and under it a scene.
    let first = ui.add_picture(20, vec![red.0; 200]);
    let (points, triangles) = floor();
    let body = ui.add_mesh(points, triangles);
    let plan = taut::View::look([0.0, 50.0, 0.0], [0.0, 0.0, 0.0]).up([0.0, 0.0, 1.0]).flat(20.0);
    let says = |ui: &mut Ui, pictures: &[taut::Picture], bodies: &[taut::Mesh]| {
        let shown: Vec<taut::Shown> = bodies.iter().map(|body| body.filled(green).unlit()).collect();
        ui.row().size(100.0, 20.0).open();
        let sizes: Vec<Rect> = pictures.iter().map(|picture| ui.el().picture(*picture).sense().show().rect).collect();
        ui.close();
        ui.el().size(200.0, 200.0).scene(&plan, &shown).show();
        sizes
    };
    let (top, under) = (rect(0.0, 0.0, 100.0, 20.0), rect(0.0, 20.0, 200.0, 200.0));
    settle(&mut ui, |ui| drop(says(ui, &[first], &[body])));
    assert_eq!((inked(&ui, top, ground), inked(&ui, under, ground)), (200, 100 * 100));

    // Let go of, each shows nothing and takes no room, though the app still names it.
    ui.drop_picture(first);
    ui.drop_mesh(body);
    let mut sizes = Vec::new();
    settle(&mut ui, |ui| sizes = says(ui, &[first], &[body]));
    assert_eq!((inked(&ui, top, ground), inked(&ui, under, ground), sizes[0].w), (0, 0, 0.0));
    // Letting go twice, and putting something in a place that is gone, do nothing.
    ui.drop_picture(first);
    ui.drop_mesh(body);
    ui.set_picture(first, 1, vec![red.0]);

    // What is taken in next gets the place that was let go, under another number: the
    // number of what went still shows nothing.
    let second = ui.add_picture(10, vec![green.0; 50]);
    let (points, triangles) = floor();
    let other = ui.add_mesh(points, triangles);
    assert!(second != first && other != body);
    settle(&mut ui, |ui| sizes = says(ui, &[first, second], &[body, other]));
    assert_eq!((sizes[0].w, sizes[1].w, inked(&ui, top, ground), inked(&ui, under, ground)), (0.0, 10.0, 50, 100 * 100));
    // And things that come and go all day take up no more than they do at any one time:
    // the numbers given are those of two places, over and over.
    let mut places = std::collections::HashSet::new();
    for round in 0..600 {
        let passing = ui.add_picture(4, vec![round; 16]);
        places.insert(format!("{passing:?}"));
        ui.drop_picture(passing);
    }
    assert_eq!(places.len(), 256, "one place, given away 256 times before its numbers come round");
}

#[test]
fn a_row_that_wraps_goes_on_to_the_next_line_when_its_room_runs_out() {
    let mut ui = window(1.0);
    // Boxes 100 wide with 10 between them, in a window 420 wide: three fit on a line.
    let says = |ui: &mut Ui, boxes: &[(f32, f32)], sits: Align, grows: Option<usize>| {
        ui.wrapping_row().gap(10.0).align(sits, Align::Center).open();
        let mut places = Vec::new();
        for (number, (wide, high)) in boxes.iter().enumerate() {
            let size = if grows == Some(number) { ui.el().w(Length::Grow(1.0)).h(Length::Px(*high)) } else { ui.el().size(*wide, *high) };
            places.push(size.sense().show().rect);
        }
        ui.close();
        places.push(ui.el().size(30.0, 10.0).sense().show().rect);
        places
    };
    let even = [(100.0, 20.0); 5];
    let mut places = Vec::new();
    settle(&mut ui, |ui| places = says(ui, &even, Align::Start, None));
    let corner = |r: &Rect| (r.x, r.y);
    assert_eq!(
        places.iter().map(corner).collect::<Vec<_>>(),
        [(0.0, 0.0), (110.0, 0.0), (220.0, 0.0), (0.0, 30.0), (110.0, 30.0), (0.0, 50.0)],
        "three on the first line, two on the second, and what follows the row under both"
    );

    // What is spare on a line goes where the row says its boxes sit: at the end, in the
    // middle, or between them.
    settle(&mut ui, |ui| places = says(ui, &even, Align::End, None));
    assert_eq!(places[..5].iter().map(|r| r.x).collect::<Vec<_>>(), [100.0, 210.0, 320.0, 210.0, 320.0]);
    settle(&mut ui, |ui| places = says(ui, &even, Align::Center, None));
    assert_eq!(places[..5].iter().map(|r| r.x).collect::<Vec<_>>(), [50.0, 160.0, 270.0, 105.0, 215.0]);
    settle(&mut ui, |ui| places = says(ui, &even, Align::Spread, None));
    assert_eq!(places[..5].iter().map(|r| r.x).collect::<Vec<_>>(), [0.0, 160.0, 320.0, 0.0, 320.0]);

    // A line is as high as its highest box, and the others sit in the middle of that.
    let uneven = [(100.0, 20.0), (100.0, 60.0), (100.0, 20.0), (100.0, 30.0)];
    settle(&mut ui, |ui| places = says(ui, &uneven, Align::Start, None));
    assert_eq!(places.iter().map(corner).collect::<Vec<_>>(), [(0.0, 20.0), (110.0, 0.0), (220.0, 20.0), (0.0, 70.0), (0.0, 100.0)]);

    // A box that grows has a line to itself, as wide as the row; and one that is said to
    // be wider than the row keeps its width, alone on its line.
    settle(&mut ui, |ui| places = says(ui, &even, Align::Start, Some(1)));
    assert_eq!(places.iter().map(|r| (r.x, r.y, r.w)).collect::<Vec<_>>(), [(0.0, 0.0, 100.0), (0.0, 30.0, 420.0), (0.0, 60.0, 100.0), (110.0, 60.0, 100.0), (220.0, 60.0, 100.0), (0.0, 80.0, 30.0)]);
    settle(&mut ui, |ui| places = says(ui, &[(600.0, 20.0), (100.0, 20.0)], Align::Start, None));
    assert_eq!(places.iter().map(|r| (r.x, r.y, r.w)).collect::<Vec<_>>(), [(0.0, 0.0, 600.0), (0.0, 30.0, 100.0), (0.0, 50.0, 30.0)]);

    // In a narrower window the same boxes take more lines, and in one wide enough, one.
    ui.resize(Size { w: 215.0, h: 300.0 }, 1.0);
    settle(&mut ui, |ui| places = says(ui, &even, Align::Start, None));
    assert_eq!(places.iter().map(corner).collect::<Vec<_>>(), [(0.0, 0.0), (110.0, 0.0), (0.0, 30.0), (110.0, 30.0), (0.0, 60.0), (0.0, 80.0)]);
    ui.resize(Size { w: 600.0, h: 300.0 }, 1.0);
    settle(&mut ui, |ui| places = says(ui, &even, Align::Start, None));
    assert_eq!(places.iter().map(corner).collect::<Vec<_>>(), [(0.0, 0.0), (110.0, 0.0), (220.0, 0.0), (330.0, 0.0), (440.0, 0.0), (0.0, 20.0)]);
}

/// Where the thing a screen reader is told of under this name lies in the window.
fn told_at(ui: &Window, name: &str) -> Rect {
    ui.said().iter().find(|said| said.name == name).unwrap_or_else(|| panic!("nothing is told as {name:?}")).rect
}

#[test]
fn words_of_several_kinds_break_into_lines_together_and_a_link_among_them_is_pressed() {
    let mut ui = window(1.0);
    ui.listen(true);
    let mut followed = 0;
    // Words five letters wide with the space after them, sixty units each: seven to a line
    // of 420. The eighth, a link, starts the second line; and a line end ends a line.
    let says = |ui: &mut Ui, followed: &mut u32| {
        let t = ui.theme;
        ui.wrapping_row().open();
        ui.words("aaaa bbbb cccc ", t.font, t.text);
        ui.words("dddd eeee ffff gggg ", t.font, t.accent);
        let link = ui.link("here");
        *followed += link.clicked as u32;
        ui.words(" and\n", t.font, t.text_dim);
        ui.words("after", t.font.bold(), t.text);
        ui.close();
        link.rect
    };
    let mut link = Rect::EMPTY;
    settle(&mut ui, |ui| link = says(ui, &mut followed));
    let corner = |r: Rect| (r.x, r.y);
    // (The words of one call are told as one piece of text, where its first word is.)
    assert_eq!([corner(told_at(&ui, "aaaa bbbb cccc")), corner(told_at(&ui, "dddd eeee ffff gggg"))], [(0.0, 0.0), (180.0, 0.0)]);
    assert_eq!([corner(link), corner(told_at(&ui, "and")), corner(told_at(&ui, "after"))], [(0.0, 20.0), (48.0, 20.0), (0.0, 40.0)]);
    // A screen reader is told that it is a link, by its words.
    assert!(ui.said().iter().any(|said| said.role == Role::Link && said.name == "here"));
    // The link is pressed with the pointer, and with the keyboard once that is on it.
    click(&mut ui, middle(link), |ui| _ = says(ui, &mut followed));
    assert_eq!(followed, 1);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut followed));
    assert_eq!(followed, 2);
}

#[test]
fn a_button_stays_down_while_it_is_on_and_choices_in_one_frame_are_picked() {
    const STAR: taut::Icon = taut::Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");
    let mut ui = window(1.0);
    ui.listen(true);
    let (mut bold, mut starred, mut span, mut changes) = (false, false, 0usize, 0);
    let says = |ui: &mut Ui, bold: &mut bool, starred: &mut bool, span: &mut usize, changes: &mut u32| {
        ui.row().gap(8.0).open();
        let places = [ui.toggle(bold, "Bold").rect, ui.icon_toggle(starred, STAR, "Starred").rect];
        *changes += ui.segmented(span, &["Day", "Week", "Month"]).changed as u32;
        ui.close();
        places
    };
    let mut places = [Rect::EMPTY; 2];
    settle(&mut ui, |ui| places = says(ui, &mut bold, &mut starred, &mut span, &mut changes));
    let on = |ui: &Window, name: &str| ui.said().iter().find(|said| said.name == name).map(|said| (said.role, said.on));
    assert_eq!(
        [on(&ui, "Bold"), on(&ui, "Starred"), on(&ui, "Day"), on(&ui, "Week")],
        [Some((Role::Button, Some(false))), Some((Role::Button, Some(false))), Some((Role::Tab, Some(true))), Some((Role::Tab, Some(false)))]
    );

    // A click turns each on, and it stays so; another turns it off again.
    for place in places {
        click(&mut ui, middle(place), |ui| _ = says(ui, &mut bold, &mut starred, &mut span, &mut changes));
    }
    assert!(bold && starred);
    assert_eq!([on(&ui, "Bold"), on(&ui, "Starred")], [Some((Role::Button, Some(true))); 2]);
    click(&mut ui, middle(places[0]), |ui| _ = says(ui, &mut bold, &mut starred, &mut span, &mut changes));
    assert!(!bold && starred);

    // Of the choices in one frame, the one that is clicked is picked, and the app hears once.
    let month = told_at(&ui, "Month");
    click(&mut ui, middle(month), |ui| _ = says(ui, &mut bold, &mut starred, &mut span, &mut changes));
    assert_eq!((span, changes, on(&ui, "Month"), on(&ui, "Day")), (2, 1, Some((Role::Tab, Some(true))), Some((Role::Tab, Some(false)))));
}

#[test]
fn a_line_with_two_knobs_sets_both_ends_of_a_span() {
    let mut ui = window(1.0);
    let (mut low, mut high, mut changes) = (20.0f32, 80.0f32, 0);
    let says = |ui: &mut Ui, low: &mut f32, high: &mut f32, changes: &mut u32| *changes += ui.range(low, high, 0.0..=100.0).changed as u32;
    settle(&mut ui, |ui| says(ui, &mut low, &mut high, &mut changes));
    // The line is 420 wide and its knobs 14: a value lies 14 in, and a hundredth of 392 on
    // for each one more.
    let at = |value: f32| pt(14.0 + 3.92 * value, 10.0);
    let near = |value: f32, wanted: f32| (value - wanted).abs() < 0.3;
    // A press at 30 takes the low knob, which is nearer, and brings it there; dragged on
    // past the high one, it stops at that.
    for event in [Event::Move(at(30.0)), Event::Press(Button::Left, Mods::NONE)] {
        does(&mut ui, event, |ui| says(ui, &mut low, &mut high, &mut changes));
    }
    assert!(near(low, 30.0) && high == 80.0, "{low} to {high}");
    does(&mut ui, Event::Move(at(90.0)), |ui| says(ui, &mut low, &mut high, &mut changes));
    does(&mut ui, Event::Release(Button::Left), |ui| says(ui, &mut low, &mut high, &mut changes));
    assert_eq!((low, high), (80.0, 80.0));
    // With both in one place, a press beyond them takes the high one.
    drag(&mut ui, at(95.0), at(95.0), |ui| says(ui, &mut low, &mut high, &mut changes));
    assert!(low == 80.0 && near(high, 95.0), "{low} to {high}");
    // The knob that was taken has the keyboard: an arrow moves it a twentieth of the way.
    // Tab back goes to the other knob, which the arrows then move, and no further than the
    // first.
    key(&mut ui, Key::Left, Mods::NONE, |ui| says(ui, &mut low, &mut high, &mut changes));
    assert!(near(high, 90.0), "{high}");
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| says(ui, &mut low, &mut high, &mut changes));
    for _ in 0..4 {
        key(&mut ui, Key::Right, Mods::NONE, |ui| says(ui, &mut low, &mut high, &mut changes));
    }
    assert!(near(low, 90.0) && near(high, 90.0), "{low} to {high}");
    assert!(changes >= 5, "the app heard of {changes} changes");
}

#[test]
fn a_ring_turns_while_something_is_going_on() {
    let mut ui = window(1.0);
    ui.listen(true);
    settle(&mut ui, |ui| _ = ui.spinner());
    // It asks to be drawn again within a sixtieth of a second, and by then it has turned.
    assert!(ui.wake_in().is_some_and(|wait| wait <= Duration::from_millis(16)));
    assert!(ui.said().iter().any(|said| said.role == Role::Progress));
    let before = ui.screen.pixels.clone();
    ui.set_time(Duration::from_millis(250));
    let changed = ui.run(&mut |ui| _ = ui.spinner());
    assert!(ui.screen.pixels != before && changed.w <= 20.0 && changed.h <= 20.0, "it turned, and only its own place was drawn again: {changed:?}");
    // A second later it is where it was.
    ui.set_time(Duration::from_millis(1000));
    ui.run(&mut |ui| _ = ui.spinner());
    assert!(ui.screen.pixels == before);
}

#[test]
fn an_interface_that_is_the_whole_screen_draws_the_pointer_itself() {
    let mut ui = window(1.0);
    let ground = Color::rgb(0x101418);
    ui.theme.bg = ground;
    let (mut presses, mut name) = (0, String::new());
    let says = |ui: &mut Ui, presses: &mut u32, name: &mut String, shown: bool| {
        ui.row().pad(40.0).gap(20.0).open();
        let button = ui.button("Press");
        *presses += button.clicked as u32;
        let typed = ui.text_box(name, "Type");
        ui.close();
        if shown {
            ui.show_pointer();
        }
        [button.rect, typed.rect]
    };
    let mut places = [Rect::EMPTY; 2];
    settle(&mut ui, |ui| places = says(ui, &mut presses, &mut name, true));
    // The pointer is drawn where it is: a white arrow with its tip there, on a square of 24
    // units that starts 3 to the left of the tip and 2 above it.
    let square = |tip: taut::Point, hot: [f32; 2]| rect(tip.x - hot[0], tip.y - hot[1], 24.0, 24.0);
    let (first, second) = (pt(300.0, 200.0), pt(200.0, 250.0));
    assert!(ui.event(Event::Move(first)), "every move is something to show");
    ui.run(&mut |ui| _ = says(ui, &mut presses, &mut name, true));
    let pixel = |ui: &Window, x: usize, y: usize| ui.screen.pixels[y * ui.screen.width + x];
    assert!(inked(&ui, square(first, [3.0, 2.0]), ground) > 60 && pixel(&ui, 302, 208) == Color::WHITE.0);
    // It moves with the mouse and leaves nothing behind. (The window in these tests holds
    // drawing only what changed against drawing everything, after every step.)
    does(&mut ui, Event::Move(second), |ui| _ = says(ui, &mut presses, &mut name, true));
    assert!(inked(&ui, square(first, [3.0, 2.0]), ground) == 0 && inked(&ui, square(second, [3.0, 2.0]), ground) > 60);
    // Over a box that is typed into it is the shape for text, and not the arrow.
    let white = |ui: &Window, x: usize, y: usize| pixel(ui, x, y) == Color::WHITE.0;
    let block = |ui: &Window, of: Rect| (of.y as usize..of.bottom() as usize).flat_map(|y| (of.x as usize..of.right() as usize).map(move |x| white(ui, x, y))).collect::<Vec<bool>>();
    let arrow = block(&ui, square(second, [3.0, 2.0]));
    // (Its place is taken to whole units, as the pointer's place is when it is drawn.)
    let over_text = pt(middle(places[1]).x.round(), middle(places[1]).y.round());
    does(&mut ui, Event::Move(over_text), |ui| _ = says(ui, &mut presses, &mut name, true));
    let beam = block(&ui, square(over_text, [12.0, 12.0]));
    assert!(beam.iter().filter(|white| **white).count() > 20 && beam != arrow);
    // It takes nothing in: what is under it is pressed as ever.
    click(&mut ui, middle(places[0]), |ui| _ = says(ui, &mut presses, &mut name, true));
    assert_eq!(presses, 1);
    // When the pointer leaves, it is gone; and an interface that does not draw it hears of
    // a move over nothing new without running at all.
    does(&mut ui, Event::Move(first), |ui| _ = says(ui, &mut presses, &mut name, true));
    does(&mut ui, Event::Leave, |ui| _ = says(ui, &mut presses, &mut name, true));
    assert_eq!(inked(&ui, square(first, [3.0, 2.0]), ground), 0);
    settle(&mut ui, |ui| _ = says(ui, &mut presses, &mut name, false));
    does(&mut ui, Event::Move(first), |ui| _ = says(ui, &mut presses, &mut name, false));
    assert!(!ui.event(Event::Move(second)) && inked(&ui, square(second, [3.0, 2.0]), ground) == 0);
}

#[test]
fn a_box_pinned_to_an_edge_lies_over_the_page_and_the_panels() {
    let mut ui = window(1.0);
    let (mut tools, mut presses) = (Panel::new(100.0, 200.0, 200.0, 120.0), [0u32; 2]);
    // A panel that reaches down past the bottom of the window, and a bar pinned along the
    // bottom, as wide as the window, described after it.
    let says = |ui: &mut Ui, tools: &mut Panel, presses: &mut [u32; 2]| {
        let mut inside = Rect::EMPTY;
        if ui.panel(tools, "Tools") {
            let button = ui.button("In the panel");
            (presses[0], inside) = (presses[0] + button.clicked as u32, button.rect);
            ui.close();
        }
        ui.pin(Align::Start, Align::End);
        let wide = ui.size().w;
        ui.row().w(Length::Px(wide)).h(Length::Px(40.0)).pad((10.0, 0.0)).fill(Color::rgb(0x202838)).open();
        let start = ui.button("Start");
        presses[1] += start.clicked as u32;
        ui.close();
        [inside, start.rect]
    };
    let mut places = [Rect::EMPTY; 2];
    settle(&mut ui, |ui| places = says(ui, &mut tools, &mut presses));
    // The bar's button sits in the bar: in the bottom 40 units of the 420 by 300 window.
    assert!(places[1].y >= 260.0 && places[1].bottom() <= 300.0 && places[1].x == 10.0, "{:?}", places[1]);
    // Where the panel lies under the bar, the bar shows, and a press there is the bar's.
    let under = pt(250.0, 280.0);
    assert_eq!(ui.screen.pixels[280 * 420 + 250], 0xff20_2838);
    click(&mut ui, under, |ui| places = says(ui, &mut tools, &mut presses));
    click(&mut ui, middle(places[1]), |ui| places = says(ui, &mut tools, &mut presses));
    assert_eq!(presses, [0, 1]);
    // And the panel is still used as ever where the bar is not.
    click(&mut ui, middle(places[0]), |ui| places = says(ui, &mut tools, &mut presses));
    assert_eq!(presses, [1, 1]);
}

/// The names of the rows of the menu that is open, as a screen reader is told of them.
fn rows_told(ui: &Window) -> Vec<String> {
    ui.said().iter().filter(|said| said.role == Role::MenuItem).map(|said| said.name.clone()).collect()
}

#[test]
fn one_of_a_long_list_is_found_by_typing_and_picked_with_the_keys() {
    let mut ui = window(1.0);
    ui.listen(true);
    let names: Vec<String> = (0..300).map(|number| format!("Item {number}")).collect();
    let options: Vec<&str> = names.iter().map(String::as_str).collect();
    let (mut choice, mut changes) = (7usize, 0u32);
    let says = |ui: &mut Ui, choice: &mut usize, changes: &mut u32| {
        ui.column().w(Length::Px(240.0)).open();
        let combo = ui.combo(choice, &options);
        *changes += combo.changed as u32;
        ui.close();
        combo.rect
    };
    let mut place = Rect::EMPTY;
    settle(&mut ui, |ui| place = says(ui, &mut choice, &mut changes));
    let chosen = |ui: &Window| ui.said().iter().find(|said| said.role == Role::Select).map(|said| (said.value.clone(), said.on));
    assert_eq!(chosen(&ui), Some(("Item 7".to_string(), Some(false))));

    // It opens with eight of its options in view, the one that is picked among them, and
    // with the keyboard in the box at its top.
    click(&mut ui, middle(place), |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!(chosen(&ui), Some(("Item 7".to_string(), Some(true))));
    assert_eq!(rows_told(&ui), ["Item 0", "Item 1", "Item 2", "Item 3", "Item 4", "Item 5", "Item 6", "Item 7"]);
    assert_eq!(ui.said().iter().find(|said| said.name == "Item 7").and_then(|said| said.of), Some([8, 300]));
    // What is typed leaves the options that hold it: thirteen of the three hundred.
    typed(&mut ui, "29", |ui| _ = says(ui, &mut choice, &mut changes));
    let search = ui.said().iter().find(|said| said.role == Role::TextBox).map(|said| (said.name.clone(), said.value.clone()));
    assert_eq!(search, Some(("Search".to_string(), "29".to_string())));
    assert_eq!(rows_told(&ui)[..4], ["Item 29", "Item 129", "Item 229", "Item 290"]);
    assert_eq!(ui.said().iter().find(|said| said.name == "Item 229").and_then(|said| said.of), Some([3, 13]));
    // The arrows go down them (a screen reader says each), and Enter picks the lit one.
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!(ui.announced(), ["Item 129"]);
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!((choice, changes), (7, 0));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!((choice, changes, rows_told(&ui).len()), (229, 1, 0));
    assert_eq!(chosen(&ui), Some(("Item 229".to_string(), Some(false))));

    // Opened again, nothing is typed in it, and the list is scrolled to the one picked.
    click(&mut ui, middle(place), |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!((rows_told(&ui).first().cloned(), rows_told(&ui).last().cloned()), (Some("Item 222".to_string()), Some("Item 229".to_string())));
    // The light goes up past what is in view, and the list goes with it.
    for _ in 0..9 {
        key(&mut ui, Key::Up, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    }
    assert_eq!(rows_told(&ui).first().cloned(), Some("Item 220".to_string()));
    // Escape puts it away and picks nothing; what was typed and lit is forgotten.
    typed(&mut ui, "zz", |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!(rows_told(&ui).len(), 0);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!((choice, changes, chosen(&ui)), (229, 1, Some(("Item 229".to_string(), Some(false)))));
    // And an option is picked with the pointer.
    click(&mut ui, middle(place), |ui| _ = says(ui, &mut choice, &mut changes));
    let row = told_at(&ui, "Item 225");
    click(&mut ui, middle(row), |ui| _ = says(ui, &mut choice, &mut changes));
    assert_eq!((choice, changes, rows_told(&ui).len()), (225, 2, 0));
}

#[test]
fn what_a_box_might_become_is_listed_under_it_while_it_is_typed_into() {
    let mut ui = window(1.0);
    ui.listen(true);
    let cities = ["Berlin", "Bern", "Bergen", "Lisbon", "Lima"];
    let (mut text, mut changes) = (String::new(), 0u32);
    let says = |ui: &mut Ui, text: &mut String, changes: &mut u32| {
        ui.column().w(Length::Px(240.0)).gap(8.0).open();
        let city = ui.suggest(text, "City", &cities);
        *changes += city.changed as u32;
        let next = ui.button("Next");
        ui.close();
        [city.rect, next.rect]
    };
    let mut places = [Rect::EMPTY; 2];
    settle(&mut ui, |ui| places = says(ui, &mut text, &mut changes));
    // Nothing is listed until something is typed.
    click(&mut ui, middle(places[0]), |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!(rows_told(&ui).len(), 0);
    typed(&mut ui, "be", |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((rows_told(&ui), changes), (vec!["Berlin".to_string(), "Bern".to_string(), "Bergen".to_string()], 1));
    // The list lies under the box, over the button that comes after it.
    let first = told_at(&ui, "Berlin");
    assert!(first.y >= places[0].bottom() && first.y < places[1].bottom(), "{first:?}");
    // Enter picks nothing while nothing is lit: what was typed stays what it is.
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), changes, rows_told(&ui).len()), ("be", 1, 3));
    // The arrows light one, Enter takes it, and the keyboard is still in the box, after it.
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!(ui.announced(), ["Bern"]);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), changes, rows_told(&ui).len()), ("Bern", 2, 0));
    typed(&mut ui, "e", |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), rows_told(&ui).len()), ("Berne", 0));

    // One is taken with the pointer just as well, and the keyboard comes back to the box.
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| _ = says(ui, &mut text, &mut changes));
    typed(&mut ui, "LI", |ui| _ = says(ui, &mut text, &mut changes));
    // (Capitals count for nothing, and what begins with the text comes before what only holds it.)
    assert_eq!(rows_told(&ui), ["Lisbon", "Lima", "Berlin"]);
    let lima = told_at(&ui, "Lima");
    click(&mut ui, middle(lima), |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), rows_told(&ui).len()), ("Lima", 0));
    typed(&mut ui, "!", |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!(text, "Lima!");

    // Escape puts the list away and leaves the text; an arrow brings it back; and it goes
    // when the keyboard goes elsewhere.
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| _ = says(ui, &mut text, &mut changes));
    typed(&mut ui, "ber", |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!(rows_told(&ui).len(), 3);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), rows_told(&ui).len()), ("ber", 0));
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!(rows_told(&ui).len(), 3);
    key(&mut ui, Key::Tab, Mods::NONE, |ui| _ = says(ui, &mut text, &mut changes));
    assert_eq!((text.as_str(), rows_told(&ui).len()), ("ber", 0));
}

#[test]
fn a_chart_writes_its_axes_and_reads_off_what_stands_under_the_pointer_and_at_the_keys() {
    const DAYS: [&str; 4] = ["Mon", "Tue", "Wed", "Thu"];
    let mut ui = window(1.0);
    ui.listen(true);
    let (sold, back) = ([10.0, 40.0, 20.0, 30.0], [1.0, 4.0, 2.0, 3.0]);
    let (mut read, mut changes) = (None, 0u32);
    let says = |ui: &mut Ui, read: &mut Option<usize>, changes: &mut u32| {
        let rows = [Series::bars("Sold", &sold), Series::line("Back", &back)];
        let chart = ui.chart(&Chart::of(&rows).names(&DAYS), 400.0, 200.0);
        (*read, *changes) = (ui.chart_at(&chart), *changes + chart.changed as u32);
        chart.rect
    };
    let mut place = Rect::EMPTY;
    settle(&mut ui, |ui| place = says(ui, &mut read, &mut changes));
    // It is told as one thing named by its rows; the numbers up its side go from nought
    // (it has bars) to the most in steps good to read, and its places are named under it.
    let words = |ui: &Window| ui.said().iter().filter(|said| said.role == Role::Text).map(|said| said.name.clone()).collect::<Vec<_>>();
    assert!(ui.said().iter().any(|said| said.role == Role::Group && said.name == "Sold, Back"));
    assert_eq!(words(&ui), ["0", "10", "20", "30", "40", "Mon", "Tue", "Wed", "Thu", "Sold", "Back"]);
    let (mon, tue, top, ground) = (told_at(&ui, "Mon"), told_at(&ui, "Tue"), told_at(&ui, "40"), told_at(&ui, "0"));
    // The places are of one width, side by side; and the numbers stand evenly up the side.
    let slot = middle(tue).x - middle(mon).x;
    assert!((middle(told_at(&ui, "Thu")).x - middle(mon).x - 3.0 * slot).abs() <= 1.0, "the places are not of one width");
    assert_eq!(middle(ground).y - middle(told_at(&ui, "20")).y, (middle(ground).y - middle(top).y) / 2.0);
    // A bar stands in its place, on the ground, in the colour of its row: under Tuesday's
    // (the highest) there is its colour from the ground to the top, and none over Monday's.
    let bar = |day: Rect, up: f32| ui.screen.pixels[(middle(ground).y - up) as usize * 420 + (middle(day).x - slot * 0.175) as usize];
    let high = middle(ground).y - middle(top).y;
    assert_eq!([bar(tue, 3.0), bar(tue, high - 3.0), bar(mon, high * 0.2)], [ui.theme.accent.0; 3]);
    assert_ne!(bar(mon, high * 0.3), ui.theme.accent.0);
    assert_eq!((read, changes), (None, 0));

    // The pointer over a place reads it off: the place is named beside it, with what each
    // row has there. And every move of the pointer over the chart runs the interface.
    does(&mut ui, Event::Move(pt(middle(tue).x, place.y + 100.0)), |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, changes), (Some(1), 1));
    assert_eq!(words(&ui)[11..], ["Tue", "Sold", "40", "Back", "4"]);
    assert!(ui.event(Event::Move(pt(middle(tue).x + 2.0, place.y + 101.0))), "a move inside the chart went unheard");
    ui.run(&mut |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, changes), (Some(1), 1));
    does(&mut ui, Event::Move(pt(middle(mon).x + 3.0 * slot, place.y + 60.0)), |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, changes, &words(&ui)[11..]), (Some(3), 2, &["Thu", "Sold", "30", "Back", "3"].map(String::from)[..]));

    // With the keyboard on it, the arrows go along its places, and a screen reader says each.
    click(&mut ui, pt(middle(tue).x, place.y + 100.0), |ui| _ = says(ui, &mut read, &mut changes));
    key(&mut ui, Key::Right, Mods::NONE, |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, ui.announced()), (Some(2), &["Wed: Sold 20, Back 2".to_string()][..]));
    key(&mut ui, Key::End, Mods::NONE, |ui| _ = says(ui, &mut read, &mut changes));
    key(&mut ui, Key::Right, Mods::NONE, |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, ui.announced()), (Some(3), &["Thu: Sold 30, Back 3".to_string()][..]));
    key(&mut ui, Key::Home, Mods::NONE, |ui| _ = says(ui, &mut read, &mut changes));
    key(&mut ui, Key::Left, Mods::NONE, |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, &words(&ui)[11..]), (Some(0), &["Mon", "Sold", "10", "Back", "1"].map(String::from)[..]));
    // Nothing is read off once neither the pointer nor the keyboard is on it.
    does(&mut ui, Event::Leave, |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!(read, Some(0));
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| _ = says(ui, &mut read, &mut changes));
    click(&mut ui, pt(410.0, 290.0), |ui| _ = says(ui, &mut read, &mut changes));
    assert_eq!((read, words(&ui).len()), (None, 11));
}

#[test]
fn a_chart_of_numbers_over_numbers_marks_its_bottom_in_steps_and_leaves_gaps() {
    let mut ui = window(1.0);
    ui.listen(true);
    // Sixty-one values for a minute, one of them none; and dots that say where they stand.
    let load: Vec<f32> = (0..61).map(|second| if second == 30 { f32::NAN } else { second as f32 / 60.0 }).collect();
    let (at, dots) = ([-45.0, -15.0], [0.25, 0.75]);
    let says = |ui: &mut Ui| {
        let rows = [Series::line("", &load), Series::points("", &dots).at(&at)];
        ui.chart(&Chart::of(&rows).across(-60.0, 0.0).up(0.0, 1.0), 400.0, 200.0).rect
    };
    let mut place = Rect::EMPTY;
    settle(&mut ui, |ui| place = says(ui));
    // With no row named, nothing is written over it: only the numbers of its two axes.
    let words: Vec<String> = ui.said().iter().filter(|said| said.role == Role::Text).map(|said| said.name.clone()).collect();
    assert_eq!(words, ["0.0", "0.5", "1.0", "-60", "-50", "-40", "-30", "-20", "-10", "0"]);
    // The numbers along the bottom stand evenly, the first under the left end of the plot
    // and the last under its right end (as far as the chart has room for it).
    let (first, middle_one, last) = (told_at(&ui, "-60"), told_at(&ui, "-30"), told_at(&ui, "0"));
    assert!(last.right() <= place.right() && first.x >= place.x);
    assert!((middle(middle_one).x - (middle(first).x + last.right() - 5.0) / 2.0).abs() < 8.0, "{first:?} {middle_one:?} {last:?}");
    // Where the value is none, the place is read off without it; a row that says where its
    // values stand is read at the one nearest by.
    let plot = (told_at(&ui, "-60").x + told_at(&ui, "-60").w / 2.0, told_at(&ui, "0").x);
    let half = pt((plot.0 + plot.1) / 2.0 + 2.0, place.y + 80.0);
    does(&mut ui, Event::Move(half), |ui| _ = says(ui));
    let read: Vec<String> = ui.said().iter().filter(|said| said.role == Role::Text).map(|said| said.name.clone()).skip(10).collect();
    assert_eq!(read, ["-30", "0.25"]);
}

#[test]
fn a_tick_box_narrows_in_a_row_that_is_too_full_and_keeps_its_height_in_a_column_that_is() {
    let mut ui = window(1.0);
    let mut on = false;
    // A column 100 high that holds 160: a box, the tick box, a box. Then a row 150 wide
    // that holds 252: the same tick box (24 for its mark, 168 for its words) and a box.
    let says = |ui: &mut Ui, on: &mut bool| {
        ui.column().w(Length::Px(200.0)).h(Length::Px(100.0)).gap(10.0).open();
        ui.el().size(100.0, 60.0).show();
        let below = ui.checkbox(on, "Keep me");
        ui.el().size(100.0, 60.0).show();
        ui.close();
        ui.row().w(Length::Px(150.0)).open();
        let beside = ui.checkbox(on, "aaaa bbbb cccc");
        ui.el().size(60.0, 10.0).show();
        ui.close();
        [below.rect, beside.rect]
    };
    let mut places = [Rect::EMPTY; 2];
    settle(&mut ui, |ui| places = says(ui, &mut on));
    // In the column it is as high as its line and where it belongs: what is too much runs
    // out at the bottom. And it is still there to be ticked.
    assert_eq!((places[0].y, places[0].h), (70.0, 20.0));
    click(&mut ui, middle(places[0]), |ui| places = says(ui, &mut on));
    assert!(on);
    // In the row it gives the 102 that are too much, and its words go onto three lines.
    assert_eq!((places[1].w, places[1].h), (90.0, 60.0));
}

#[test]
fn what_an_app_says_in_a_corner_goes_by_itself_and_its_keys_are_asked_for_as_written() {
    let mut ui = window(1.0);
    ui.listen(true);
    let mut saved = 0u32;
    let says = |ui: &mut Ui, saved: &mut u32| {
        if ui.keys("Ctrl+S") {
            *saved += 1;
            ui.notice(&format!("Saved {saved} times"));
        }
        if ui.keys("f5") {
            ui.notice_for("Read again", Duration::from_secs(1));
        }
        ui.label("The page");
    };
    settle(&mut ui, |ui| says(ui, &mut saved));
    let there = |ui: &Window, words: &str| ui.said().iter().find(|said| said.name == words).map(|said| said.rect);
    assert_eq!((there(&ui, "Saved 1 times"), ui.wake_in()), (None, None));
    // The keys as a menu writes them: with Shift held as well they are other keys.
    key(&mut ui, Key::Char('s'), Mods { shift: true, ..Mods::CTRL }, |ui| says(ui, &mut saved));
    assert_eq!(saved, 0);
    key(&mut ui, Key::Char('s'), Mods::CTRL, |ui| says(ui, &mut saved));
    // What was said stands in the bottom right corner, over the page; a screen reader says
    // it once; and the interface asks to be run when it is to go, in four seconds.
    let first = there(&ui, "Saved 1 times").expect("what was said shows");
    assert!(first.right() <= 420.0 - 12.0 && first.right() > 380.0 && first.bottom() <= 300.0 - 12.0 && first.bottom() > 260.0, "{first:?}");
    assert_eq!((saved, ui.announced(), ui.wake_in()), (1, &["Saved 1 times".to_string()][..], Some(Duration::from_secs(4))));
    // The next thing said stands under it, and goes when its own time is up.
    ui.set_time(Duration::from_millis(500));
    key(&mut ui, Key::F(5), Mods::NONE, |ui| says(ui, &mut saved));
    let (moved, second) = (there(&ui, "Saved 1 times").unwrap(), there(&ui, "Read again").unwrap());
    assert!(moved.y < first.y && second.y > moved.y && ui.wake_in() == Some(Duration::from_secs(1)), "{moved:?} {second:?}");
    ui.set_time(Duration::from_millis(1600));
    ui.run(&mut |ui| says(ui, &mut saved));
    assert_eq!((there(&ui, "Read again"), there(&ui, "Saved 1 times"), ui.wake_in()), (None, Some(first), Some(Duration::from_millis(2400))));
    // A press on what is left puts it away at once; and nothing is waited for any more.
    click(&mut ui, middle(first), |ui| says(ui, &mut saved));
    assert_eq!((there(&ui, "Saved 1 times"), ui.wake_in(), saved), (None, None, 1));
    // Said again, it is gone by itself once its time is up.
    key(&mut ui, Key::Char('s'), Mods::CTRL, |ui| says(ui, &mut saved));
    assert!(there(&ui, "Saved 2 times").is_some());
    ui.set_time(Duration::from_secs(6));
    ui.run(&mut |ui| says(ui, &mut saved));
    assert_eq!((there(&ui, "Saved 2 times"), ui.wake_in()), (None, None));
}

#[test]
fn a_command_is_found_by_typing_in_a_box_over_the_window() {
    const COMMANDS: [&str; 5] = ["Open a file\tCtrl+O", "Save\tCtrl+S", "Save as", "Close the window", "Light colours"];
    let mut ui = window(1.0);
    ui.listen(true);
    let (mut finding, mut picked, mut name) = (false, Vec::new(), String::new());
    let says = |ui: &mut Ui, finding: &mut bool, picked: &mut Vec<usize>, name: &mut String| {
        if ui.key(Key::Char('k'), Mods::CTRL) {
            *finding = true;
        }
        let field = ui.text_box(name, "Name");
        picked.extend(ui.palette(finding, &COMMANDS));
        field.rect
    };
    let mut field = Rect::EMPTY;
    settle(&mut ui, |ui| field = says(ui, &mut finding, &mut picked, &mut name));
    let over = |ui: &Window| ui.said().iter().any(|said| said.role == Role::Dialog);
    click(&mut ui, middle(field), |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    typed(&mut ui, "x", |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert!(!over(&ui) && rows_told(&ui).is_empty());

    // Its key brings it up over the window with every command, each with its keys; and the
    // keyboard is in it: what is typed narrows the commands, and not the box under it.
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert!(finding && over(&ui));
    assert_eq!(rows_told(&ui), ["Open a file", "Save", "Save as", "Close the window", "Light colours"]);
    assert_eq!(ui.said().iter().find(|said| said.name == "Save").map(|said| said.keys.clone()), Some("Ctrl+S".to_string()));
    typed(&mut ui, "sa", |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((rows_told(&ui), name.as_str()), (vec!["Save".to_string(), "Save as".to_string()], "x"));
    // The arrows and Enter pick one: the app hears which, the box is gone, and the keyboard
    // is back where it was.
    key(&mut ui, Key::Down, Mods::NONE, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((picked.as_slice(), finding, over(&ui)), (&[2][..], false, false));
    typed(&mut ui, "y", |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!(name, "xy");

    // It comes up afresh each time. Escape puts it away with nothing picked; so does a
    // press beside it; and a press on a command picks that one.
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!(rows_told(&ui).len(), 5);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((picked.len(), finding, over(&ui)), (1, false, false));
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    click(&mut ui, pt(6.0, 294.0), |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((picked.len(), finding, over(&ui)), (1, false, false));
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    let close = told_at(&ui, "Close the window");
    click(&mut ui, middle(close), |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((picked.as_slice(), finding, over(&ui)), (&[2, 3][..], false, false));
    // Put away by the app itself in the middle of a word, it is empty the next time.
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    typed(&mut ui, "li", |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!(rows_told(&ui), ["Light colours"]);
    finding = false;
    settle(&mut ui, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    key(&mut ui, Key::Char('k'), Mods::CTRL, |ui| _ = says(ui, &mut finding, &mut picked, &mut name));
    assert_eq!((rows_told(&ui).len(), name.as_str()), (5, "xy"));
}

/// What a screen reader is told of a table that is built with the library's own parts: a
/// row of sums kept at the top, a long list of rows that are picked, a cell with a button
/// in it, and a column that is hidden.
fn accounts(ui: &mut Ui, table: &mut Table, picked: usize, pressed: &mut u32) {
    const LEDGER: [Column; 3] = [Column::new("Name"), Column::new("Size").px(80.0).align(Align::End), Column::new("Do").px(90.0).unsorted()];
    ui.table(table, &LEDGER);
    ui.table_row().kept().open();
    ui.label("All");
    ui.label("600");
    ui.label("");
    ui.close();
    for row in ui.rows(300, 24.0) {
        ui.pick_row(row, row == picked);
        ui.label(&format!("file {row}"));
        ui.label(&format!("{}", row * 2));
        *pressed += ui.button("Open").clicked as u32;
        ui.close();
    }
    ui.close();
    ui.close();
}

#[test]
fn a_table_is_told_to_a_screen_reader_as_rows_and_columns_under_headings() {
    let mut ui = window(1.0);
    ui.listen(true);
    let (mut table, mut pressed) = (Table { sort: vec![(1, true)], ..Default::default() }, 0);
    settle(&mut ui, |ui| accounts(ui, &mut table, 2, &mut pressed));
    let said = ui.said().to_vec();
    let place = |name: &str| said.iter().position(|said| said.name == name).unwrap_or_else(|| panic!("nothing is told as {name:?}"));
    // The table: 301 rows in all (the row of sums, and the 300 of the list, of which only
    // those in view are told), and three columns.
    assert_eq!((said[0].role, said[0].grid), (Role::Table, Some([301, 3])));
    // Its headings, in it: which of three each is, and which the rows are sorted by.
    let heads: Vec<_> = said.iter().filter(|said| said.role == Role::Heading).map(|said| (said.name.as_str(), said.of, said.sorted, said.within)).collect();
    assert_eq!(heads, [("Name", Some([1, 3]), None, Some(0)), ("Size", Some([2, 3]), Some(true), Some(0)), ("Do", Some([3, 3]), None, Some(0))]);
    // The row of sums is the first row, and cannot be picked; the third of the list is the
    // fourth row, and is picked. Each is called by what its cells say.
    let (sums, third) = (place("All, 600"), place("file 2, 4, Open"));
    assert_eq!([said[sums].role, said[third].role], [Role::TableRow; 2]);
    assert_eq!([(said[sums].of, said[sums].on), (said[third].of, said[third].on)], [(Some([1, 301]), None), (Some([4, 301]), Some(true))]);
    // Its cells are in it, by row and column; the one with a button holds the button.
    let cells: Vec<_> = said.iter().filter(|cell| cell.within == Some(third)).map(|cell| (cell.role, cell.name.as_str(), cell.grid)).collect();
    assert_eq!(cells, [(Role::Cell, "file 2", Some([3, 0])), (Role::Cell, "4", Some([3, 1])), (Role::Cell, "Open", Some([3, 2]))]);
    let open = said.iter().position(|cell| cell.within == Some(third) && cell.name == "Open").unwrap();
    assert_eq!(said.iter().filter(|inner| inner.within == Some(open)).map(|inner| inner.role).collect::<Vec<_>>(), [Role::Button]);

    // A column that is hidden is no column of it; one that is moved is told where it stands.
    table.hidden = vec![1];
    table.order = vec![2, 0, 1];
    settle(&mut ui, |ui| accounts(ui, &mut table, 2, &mut pressed));
    let said = ui.said().to_vec();
    assert_eq!(said[0].grid, Some([301, 2]));
    let heads: Vec<_> = said.iter().filter(|said| said.role == Role::Heading).map(|said| (said.name.as_str(), said.of)).collect();
    assert_eq!(heads, [("Do", Some([1, 2])), ("Name", Some([2, 2]))]);
    let third = said.iter().position(|said| said.role == Role::TableRow && said.name.starts_with("file 2")).unwrap();
    let cells: Vec<_> = said.iter().filter(|cell| cell.within == Some(third)).map(|cell| (cell.name.as_str(), cell.grid)).collect();
    assert_eq!(cells, [("file 2", Some([3, 1])), ("Open", Some([3, 0]))]);

    // Scrolled down, other rows are told, each still the row it is of all 301.
    ui.event(Event::Move(pt(200.0, 200.0)));
    does(&mut ui, Event::Wheel(0.0, 2400.0), |ui| accounts(ui, &mut table, 2, &mut pressed));
    settle(&mut ui, |ui| accounts(ui, &mut table, 2, &mut pressed));
    let said = ui.said().to_vec();
    let first = said.iter().find(|said| said.role == Role::TableRow && said.name.starts_with("file ")).unwrap();
    let number: usize = first.name["file ".len()..].split(',').next().unwrap().parse().unwrap();
    assert!(number >= 90, "{:?}", first.name);
    assert_eq!(first.of, Some([number as u32 + 2, 301]));
    assert!(said.iter().any(|said| said.name == "All, 600"), "the row of sums stays in view, and told");
}
