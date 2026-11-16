//! What an app reaches for on its first day, as users of other libraries expect: a button with
//! a picture and label, a slider and progress bar showing their value, a number dragged within
//! a range, a combo box over app values, a striped grid, a translucent group, a hidden box
//! keeping its space. And what helps that day: an unclosed or doubly closed box reported at the
//! app's line, a scroll box without a height, a label naming the slider after it, a trial whose
//! describe closure holds what it needs. Each through `Trial`, which compares every step's
//! image with a fresh draw.
use std::cell::Cell;
use std::rc::Rc;
use taut::testing::Trial;
use taut::{Align, Color, Length, Locale, Picture, Role, Ui};

/// A trial of `describe`, with numbers written the plain way (a point before decimals), on
/// any system.
fn trial<A: 'static>(app: A, describe: impl FnMut(&mut A, &mut Ui) + 'static) -> Trial<A> {
    let mut trial = Trial::new(app, describe);
    trial.ui.locale = Some(Locale::PLAIN);
    trial.run();
    trial
}

/// The same on a page of white.
fn on_white<A: 'static>(app: A, describe: impl FnMut(&mut A, &mut Ui) + 'static) -> Trial<A> {
    let mut trial = trial(app, describe);
    trial.ui.theme.bg = Color::WHITE;
    trial.run();
    trial
}

/// The pixel of the trial's window at `x`, `y` (in units, at a scale of 1).
fn pixel(trial: &Trial<impl Sized>, x: f32, y: f32) -> u32 {
    trial.screen.pixels[y as usize * trial.screen.width + x as usize] & 0x00ff_ffff
}
/// Whether two colours are the same to within one in each part: a mix of two is worked out
/// in whole numbers.
fn near(one: u32, other: u32) -> bool {
    [16, 8, 0].iter().all(|shift| ((one >> shift & 0xff) as i32 - (other >> shift & 0xff) as i32).abs() <= 1)
}

#[derive(Default)]
struct Buttons {
    picture: Option<Picture>,
    clicked: [u32; 2],
}

fn buttons(app: &mut Buttons, ui: &mut Ui) {
    // (A picture of the app's own, 32 pixels wide and 16 high: taken in once.)
    let picture = *app.picture.get_or_insert_with(|| ui.add_picture(32, vec![0xff20_60c0; 32 * 16]));
    ui.column().pad(10.0).gap(8.0).align(Align::Start, Align::Start).open();
    app.clicked[0] += ui.button_with_icon(taut::Icon::line("M3 8h10"), "Save").clicked as u32;
    app.clicked[1] += ui.button_with_picture(picture, "Open").clicked as u32;
    ui.button("Plain");
    ui.close();
}

#[test]
fn a_button_with_an_icon_or_a_picture_has_it_before_its_words() {
    let mut trial = trial(Buttons::default(), buttons);
    trial.press(Role::Button, "Save");
    trial.press(Role::Button, "Open");
    assert_eq!(trial.app.clicked, [1, 1]);
    // Each is as wide as its words (12 a letter), its lead, the gap of 6 and its padding
    // of 11 a side: the icon 16 wide, the picture 32 (16 high, as wide as its shape says).
    let wide = |name: &str| trial.find(Role::Button, name).rect.w;
    assert_eq!([wide("Save"), wide("Open")], [4.0 * 12.0 + 16.0 + 6.0 + 22.0, 4.0 * 12.0 + 32.0 + 6.0 + 22.0]);
    // The picture is drawn there, in its own colour.
    let open = trial.find(Role::Button, "Open").rect;
    assert_eq!(pixel(&trial, open.x + 11.0 + 16.0, open.y + open.h / 2.0), 0x20_60c0);
}

struct Values {
    level: f32,
    done: f32,
    count: i32,
}

fn values(app: &mut Values, ui: &mut Ui) {
    ui.column().pad(10.0).gap(8.0).open();
    ui.label("Level");
    ui.slider_with_value(&mut app.level, 0.0..=100.0, "%");
    ui.label("Copied");
    ui.progress_with_percent(app.done);
    ui.label("Count");
    ui.drag_in(&mut app.count, 0..=10, 0.1);
    ui.close();
}

#[test]
fn a_slider_and_a_bar_write_their_value_and_a_dragged_number_keeps_to_its_range() {
    let mut trial = trial(Values { level: 50.0, done: 0.42, count: 8 }, values);
    // The slider's value is written beside it, with its unit, and changes as it moves.
    assert!(trial.has(Role::Text, "50.0 %"), "{}", trial.listed());
    let level = trial.spot(Role::Slider, "Level");
    trial.click(level);
    trial.key("End");
    assert_eq!(trial.app.level, 100.0);
    assert!(trial.has(Role::Text, "100 %"), "{}", trial.listed());
    // The bar says how much is done, in whole parts of a hundred.
    assert_eq!(trial.find(Role::Progress, "Copied").amount, Some([0.42, 0.0, 1.0]));
    assert!(trial.has(Role::Text, "42 %"));
    trial.app.done = 0.999;
    trial.run();
    assert!(trial.has(Role::Text, "100 %") && !trial.has(Role::Text, "42 %"));
    // The number goes up ten steps of 0.1 for each press of an arrow, and no further than
    // its range: a screen reader is told both ends.
    let count = trial.spot(Role::Slider, "Count");
    trial.click(count);
    trial.key("Right");
    assert_eq!(trial.app.count, 9);
    (0..3).for_each(|_| trial.key("Right"));
    assert_eq!(trial.app.count, 10);
    assert_eq!(trial.find(Role::Slider, "Count").amount, Some([10.0, 0.0, 10.0]));
    (0..12).for_each(|_| trial.key("Left"));
    assert_eq!(trial.app.count, 0);
    // Dragged a hundred units to the right, ten on: no further than 10.
    trial.app.count = 5;
    trial.run();
    trial.drag(count, taut::geom::pt(count.x + 100.0, count.y));
    assert_eq!(trial.app.count, 10);
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Size {
    Small,
    Medium,
    Large,
}

fn sizes(size: &mut Size, ui: &mut Ui) {
    ui.row().pad(10.0).gap(8.0).open();
    ui.label("Size");
    ui.select_value(size, &[(Size::Small, "Small"), (Size::Medium, "Medium"), (Size::Large, "Large")]);
    ui.close();
}

#[test]
fn a_select_picks_among_the_apps_own_values() {
    let mut trial = trial(Size::Medium, sizes);
    assert_eq!(trial.find(Role::Select, "Size").value, "Medium");
    trial.press(Role::Select, "Size");
    trial.press(Role::MenuItem, "Large");
    assert_eq!(trial.app, Size::Large);
    assert_eq!(trial.find(Role::Select, "Size").value, "Large");
    // By the keys too: the keyboard is back on it; Enter opens it with the keyboard on the
    // one picked, and Up goes to the one before.
    assert_eq!(trial.ui.focus(), trial.find(Role::Select, "Size").id);
    for key in ["Enter", "Up", "Enter"] {
        trial.key(key);
    }
    assert_eq!(trial.app, Size::Medium);
}

/// A grid of two columns and four rows of cells 20 high, 10 apart, striped in red; or a
/// column of the same rows; on white.
fn striped(grid: &mut bool, ui: &mut Ui) {
    let around = if *grid { ui.grid(2) } else { ui.column() };
    around.pad(10.0).gap(10.0).stripes(Color::rgb(0xff0000)).open();
    for _ in 0..if *grid { 8 } else { 4 } {
        ui.el().size(50.0, 20.0).show();
    }
    ui.close();
}

#[test]
fn every_other_row_of_a_grid_has_a_stripe() {
    for grid in [true, false] {
        let trial = on_white(grid, striped);
        // The rows stand at 10, 40, 70 and 100; the second and the fourth are striped,
        // and their stripes reach halfway into the gaps around them: 35 to 65, 95 to 125.
        let at = |y: f32| pixel(&trial, 30.0, y);
        assert_eq!([at(20.0), at(36.0), at(50.0), at(64.0), at(66.0), at(80.0), at(110.0), at(124.0)], [0xffffff, 0xff0000, 0xff0000, 0xff0000, 0xffffff, 0xffffff, 0xff0000, 0xff0000], "grid {grid}");
        // Across all of the box but its padding.
        assert_eq!([pixel(&trial, 5.0, 50.0), pixel(&trial, 11.0, 50.0)], [0xffffff, 0xff0000], "grid {grid}");
    }
    // A row's boxes lie side by side: it has no lines to stripe.
    let trial = on_white((), |_, ui| {
        ui.row().pad(10.0).gap(10.0).stripes(Color::rgb(0xff0000)).open();
        (0..4).for_each(|_| _ = ui.el().size(50.0, 20.0).show());
        ui.close();
    });
    assert!((0..300).all(|x| pixel(&trial, x as f32, 20.0) == 0xffffff));
}

/// A box filled black, with a white box in it, drawn half see-through on white; and a
/// button that is hidden, or not, before another.
struct Faded {
    solid: f32,
    hidden: bool,
    pressed: u32,
}

fn faded(app: &mut Faded, ui: &mut Ui) {
    ui.column().pad(10.0).gap(10.0).align(Align::Start, Align::Start).open();
    ui.el().size(100.0, 40.0).pad(10.0).fill(Color::BLACK).opacity(app.solid).open();
    ui.el().size(20.0, 20.0).fill(Color::rgb(0xff0000)).show();
    ui.close();
    ui.row().gap(8.0).open();
    ui.el().hidden(app.hidden).open();
    app.pressed += ui.button("Hidden").clicked as u32;
    ui.close();
    ui.button("After");
    ui.close();
    ui.close();
}

#[test]
fn a_group_is_drawn_see_through_as_one_picture() {
    let mut trial = on_white(Faded { solid: 0.5, hidden: false, pressed: 0 }, faded);
    // Black at half over white is grey, and so is the red in it: red at half, over the
    // white that was under the group (not over the black: the group is one picture).
    assert!(near(pixel(&trial, 15.0, 15.0), 0x808080) && near(pixel(&trial, 25.0, 25.0), 0xff8080), "{:06x} {:06x}", pixel(&trial, 15.0, 15.0), pixel(&trial, 25.0, 25.0));
    // Changed, it is drawn again (the trial holds that against all of the window drawn
    // afresh at every step); wholly solid, it is as it is.
    trial.app.solid = 1.0;
    trial.run();
    assert_eq!([pixel(&trial, 15.0, 15.0), pixel(&trial, 25.0, 25.0)], [0x000000, 0xff0000]);
    trial.app.solid = 0.25;
    trial.run();
    assert!(near(pixel(&trial, 15.0, 15.0), 0xbfbfbf), "{:06x}", pixel(&trial, 15.0, 15.0));
}

#[test]
fn a_hidden_box_keeps_its_room_and_nothing_reaches_it() {
    let mut trial = on_white(Faded { solid: 1.0, hidden: false, pressed: 0 }, faded);
    let (shown, after) = (trial.spot(Role::Button, "Hidden"), trial.find(Role::Button, "After").rect);
    trial.app.hidden = true;
    trial.run();
    // Not drawn, not told, not pressed; the button after it stays where it was.
    assert!(!trial.has(Role::Button, "Hidden"), "{}", trial.listed());
    assert_eq!(trial.find(Role::Button, "After").rect, after);
    assert_eq!(pixel(&trial, shown.x, shown.y), 0xffffff);
    trial.click(shown);
    assert_eq!(trial.app.pressed, 0);
    // Tab passes it by.
    trial.key("Tab");
    assert_eq!(trial.ui.focus(), trial.find(Role::Button, "After").id);
    // Shown again, it is all there again.
    trial.app.hidden = false;
    trial.run();
    trial.press(Role::Button, "Hidden");
    assert_eq!(trial.app.pressed, 1);
}

#[test]
fn a_link_before_a_slider_or_a_select_names_it() {
    let mut trial = trial((20.0f32, 0usize), |(level, size), ui| {
        ui.grid(2).pad(10.0).gap(8.0).open();
        ui.link("Speed");
        ui.slider(level, 0.0..=100.0);
        ui.link("Size");
        ui.select(size, &["Small", "Large"]);
        ui.close();
    });
    assert!(trial.has(Role::Slider, "Speed") && trial.has(Role::Select, "Size"), "{}", trial.listed());
    // A button before one does not name it: it has words of its own, for what it does.
    trial = self::trial((20.0f32, 0usize), |(level, _), ui| {
        ui.row().pad(10.0).open();
        ui.button("Reset");
        ui.slider(level, 0.0..=100.0);
        ui.close();
    });
    assert!(trial.has(Role::Slider, ""), "{}", trial.listed());
}

#[test]
fn a_box_that_scrolls_with_no_height_takes_the_height_it_is_given() {
    let trial = trial([taut::Rect::EMPTY; 2], |rects, ui| {
        ui.row().grow().open();
        // A list of three rows, in a row as high as the window: it starts at the top, as
        // high as the row, where it would have been as high as its rows and in the middle.
        rects[0] = ui.column().key("list").w(Length::Px(100.0)).scroll().sense().open().rect;
        for row in 0..3 {
            ui.label(&format!("Row {row}"));
        }
        ui.close();
        // One told the most it may be fits what is in it, up to that.
        rects[1] = ui.column().key("menu").w(Length::Px(100.0)).max(f32::INFINITY, 300.0).scroll().sense().open().rect;
        ui.label("Only");
        ui.close();
        ui.close();
    });
    let ([list, menu], room) = (trial.app, trial.ui.size());
    assert_eq!((list.y, list.h), (0.0, room.h));
    assert_eq!((menu.y, menu.h), ((room.h - 20.0) / 2.0, 20.0));
}

#[cfg(debug_assertions)]
#[test]
fn a_box_left_open_is_named_by_the_line_that_opened_it() {
    // (The panics below are expected; what they say is checked.)
    let said = |describe: fn(&mut Ui)| {
        let mut ui = Ui::new(Box::new(taut::text::BuiltInFont), Box::new(taut::LocalClipboard::default()));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ui.run(&mut |ui| describe(ui)))).expect_err("it panics");
        panic.downcast_ref::<String>().cloned().unwrap_or_default()
    };
    let opened = line!() + 1;
    let open = said(|ui| _ = ui.row().open());
    assert!(open.contains(&format!("the row opened at {}:{opened}:", file!())), "{open}");
    let closed = line!() + 4;
    let twice = said(|ui| {
        ui.column().open();
        ui.close();
        ui.close();
    });
    assert!(twice.contains(&format!("ui.close() at {}:{closed}:", file!())), "{twice}");
    // Parts that leave a box open for the app to close are named by the app's line too.
    let menu = line!() + 1;
    let left = said(|ui| _ = ui.card());
    assert!(left.contains(&format!("opened at {}:{menu}:", file!())), "{left}");
}

#[test]
fn a_trial_takes_a_closure_that_keeps_what_it_needs() {
    let presses = Rc::new(Cell::new(0));
    let counted = presses.clone();
    let mut trial = Trial::new((), move |_, ui| counted.set(counted.get() + ui.button("Press").clicked as u32));
    trial.press(Role::Button, "Press");
    trial.press(Role::Button, "Press");
    assert_eq!(presses.get(), 2);
    // What it says of itself is short, and names its interface.
    let said = format!("{trial:?}");
    assert!(said.starts_with("Trial { ui: Ui { size: Size { w: 640.0, h: 480.0 }, scale: 1.0, boxes: ") && said.len() < 400, "{said}");
}

#[test]
fn the_pictures_of_two_failures_are_named_apart() {
    // A trial that fails twice, as two tests that run at once would: each time its
    // pictures go under names of their own.
    let failed = || {
        let step = std::panic::catch_unwind(|| {
            // (What it shows changes after the part that shows it is described.)
            let mut trial = Trial::new(0u32, |count, ui| _ = (ui.label(&count.to_string()), *count += 1));
            trial.run();
        });
        let said = step.expect_err("a picture left stale fails").downcast_ref::<String>().cloned().unwrap_or_default();
        said.split("as shown: ").nth(1).and_then(|rest| rest.split(',').next()).unwrap_or_default().to_string()
    };
    let (first, second) = (failed(), failed());
    assert!(first.contains("taut-trial") && first.contains(&std::process::id().to_string()), "{first}");
    assert_ne!(first, second);
    assert!(std::path::Path::new(&first).is_file() && std::path::Path::new(&second).is_file());
    let _ = [first, second].map(std::fs::remove_file);
}

#[test]
fn the_systems_letters_come_in_a_box_that_any_interface_takes() {
    let mut ui = Ui::new(taut::platform::system_text(), Box::new(taut::LocalClipboard::default()));
    ui.set_text(taut::platform::system_text());
    ui.resize(taut::geom::Size { w: 200.0, h: 100.0 }, 1.0);
    (0..2).for_each(|_| _ = ui.run(&mut |ui| _ = ui.label("Letters")));
    // On Windows and on Linux with its fonts, they are the system's own: not the built-in
    // letters, which are each 12 units wide.
    if taut::platform::SystemText::new().is_some() {
        assert_ne!(ui.text_width("Letters", taut::Font::new(14.0)), 7.0 * 12.0);
    }
    assert!(format!("{ui:?}").starts_with("Ui { size: Size { w: 200.0, h: 100.0 }"));
}
