//! Parts familiar from web pages and phone apps, as an app uses them: breadcrumbs, steppers,
//! tags, ratings, pagination, a split button, a search box, an avatar, a card, a banner, key
//! caps, and skeleton placeholders.
mod common;

use common::*;
use std::time::Duration;
use taut::geom::{Size, pt};
use taut::{Event, Key, Mods, Role, Severity, Surface, Ui};

/// Clicks the middle of what is told to a screen reader by this name.
fn click_on(ui: &mut Window, name: &str, build: impl FnMut(&mut Ui)) {
    let at = middle(expect_node(ui, name).rect);
    click(ui, at, build);
}
/// What is told of the things of these kinds, one after another: each kind and name.
fn role_list(ui: &Window, of: &[Role]) -> String {
    let each = ui.visible_access_nodes().iter().filter(|node| of.contains(&node.role)).map(|node| format!("{:?} {}", node.role, node.name));
    each.collect::<Vec<_>>().join(", ")
}
/// The window made another size, and drawn afresh.
fn resized(ui: &mut Window, width: f32, height: f32) {
    ui.resize(Size { w: width, h: height }, 1.0);
    ui.screen = Surface::draw(&mut ui.ui, 64);
}
fn pixel(ui: &Window, x: usize, y: usize) -> u32 {
    ui.screen.pixels[y * ui.screen.width + x]
}

const PATH: [&str; 5] = ["Home", "Documents", "Taxes", "2026", "Receipts"];

#[test]
fn a_path_of_places_goes_back_along_it_and_folds_what_there_is_no_room_for() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let mut went = None;
    let says = |ui: &mut Ui, went: &mut Option<usize>| *went = ui.breadcrumbs(&PATH).or(*went);
    settle(&mut ui, |ui| says(ui, &mut went));
    // In the built-in letters, 12 wide, a place takes its letters, 6 on either side and 16
    // for its arrow: 500 in all, more than the window's 420. The second place folds into
    // the button that holds it, and the rest fits; the last is where the person is.
    let shown = [Role::Link, Role::Fold, Role::Text];
    assert_eq!(role_list(&ui, &shown), "Link Home, Fold More, Link Taxes, Link 2026, Text Receipts");
    click_on(&mut ui, "Taxes", |ui| says(ui, &mut went));
    assert_eq!(went, Some(2));
    // The folded place is selected from the menu its button opens.
    click_on(&mut ui, "More", |ui| says(ui, &mut went));
    assert_eq!(expect_node(&ui, "More").on, Some(true));
    click_on(&mut ui, "Documents", |ui| says(ui, &mut went));
    assert_eq!((went, expect_node(&ui, "More").on), (Some(1), Some(false)));
    // The keyboard is back on the button the menu came from; it goes to each place, and
    // Enter goes there.
    assert!(expect_node(&ui, "More").focused);
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| says(ui, &mut went));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| says(ui, &mut went));
    assert_eq!(went, Some(0));
    // With room for all of them, nothing folds; at the smallest a window is kept, all but
    // the first and the last do.
    resized(&mut ui, 640.0, 300.0);
    settle(&mut ui, |ui| says(ui, &mut went));
    assert_eq!(role_list(&ui, &shown), "Link Home, Link Documents, Link Taxes, Link 2026, Text Receipts");
    resized(&mut ui, 281.0, 220.0);
    settle(&mut ui, |ui| says(ui, &mut went));
    assert_eq!(role_list(&ui, &shown), "Link Home, Fold More, Text Receipts");
    assert!(expect_node(&ui, "Receipts").rect.right() <= 281.0);
}

#[test]
fn the_steps_of_a_task_go_back_to_one_that_is_done_and_never_on() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut step, mut changes) = (2usize, 0);
    let says = |ui: &mut Ui, step: &mut usize, changes: &mut u32| *changes += ui.steps(step, &["Account", "Address", "Payment", "Done"]).changed as u32;
    settle(&mut ui, |ui| says(ui, &mut step, &mut changes));
    let steps = |ui: &Window| {
        let each = ui.visible_access_nodes().iter().filter(|node| node.role == Role::Tab).map(|node| format!("{} {:?} {:?} {}", node.name, node.on, node.position, node.enabled));
        each.collect::<Vec<_>>()
    };
    assert_eq!(
        steps(&ui),
        ["Account, done Some(false) Some([1, 4]) true", "Address, done Some(false) Some([2, 4]) true", "Payment Some(true) Some([3, 4]) true", "Done Some(false) Some([4, 4]) false"]
    );
    // A step to come is not pressed.
    click_on(&mut ui, "Done", |ui| says(ui, &mut step, &mut changes));
    assert_eq!((step, changes), (2, 0));
    // Tab goes to the steps that are done, and Enter goes back to one.
    for _ in 0..2 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| says(ui, &mut step, &mut changes));
    }
    assert!(expect_node(&ui, "Address, done").focused);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| says(ui, &mut step, &mut changes));
    assert_eq!((step, changes), (1, 1));
    // At the smallest a window is kept, the names give way, the one at hand last.
    resized(&mut ui, 281.0, 220.0);
    settle(&mut ui, |ui| says(ui, &mut step, &mut changes));
    let [first, here] = [expect_node(&ui, "Account, done").rect, expect_node(&ui, "Address").rect];
    assert!(here.w > first.w && here.right() <= 281.0, "{first:?} {here:?}");
    click(&mut ui, middle(first), |ui| says(ui, &mut step, &mut changes));
    assert_eq!((step, changes), (0, 2));
}

#[test]
fn typed_words_become_tags_which_go_by_their_cross_and_by_backspace() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut tags, mut text, mut changes) = (vec!["rust".to_string()], String::new(), 0);
    let says = |ui: &mut Ui, tags: &mut Vec<String>, text: &mut String, changes: &mut u32| *changes += ui.tag_input(tags, text, "Topics").changed as u32;
    settle(&mut ui, |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!(role_list(&ui, &[Role::Group, Role::Button, Role::TextBox]), "Group Topics, Button Remove rust, TextBox Topics");
    let typing = ui.visible_access_nodes().iter().find(|node| node.role == Role::TextBox).unwrap().rect;
    click(&mut ui, middle(typing), |ui| says(ui, &mut tags, &mut text, &mut changes));
    // A comma makes a tag of what is before it; Enter of what is being typed; and a tag that
    // is there already is not made again.
    typed(&mut ui, "gui, fa", |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!((&tags[..], text.as_str(), changes), (&["rust".to_string(), "gui".to_string()][..], "fa", 1));
    typed(&mut ui, "st", |ui| says(ui, &mut tags, &mut text, &mut changes));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| says(ui, &mut tags, &mut text, &mut changes));
    typed(&mut ui, "rust,", |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!((tags.join(" "), text.as_str(), changes), ("rust gui fast".to_string(), "", 2));
    // Backspace in the empty box goes to the last tag; a second takes it away, and the
    // keyboard is back in the box.
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert!(expect_node(&ui, "Remove fast").focused);
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!(tags.join(" "), "rust gui");
    assert!(ui.visible_access_nodes().iter().any(|node| node.role == Role::TextBox && node.focused));
    // The cross of a tag takes it away.
    let gui = expect_node(&ui, "Remove gui").rect;
    click(&mut ui, pt(gui.right() - 12.0, gui.y + gui.h / 2.0), |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!(tags.join(" "), "rust");
    // A press on a tag itself only puts the keyboard there; Delete then takes it away.
    let rust = expect_node(&ui, "Remove rust").rect;
    click(&mut ui, pt(rust.x + 14.0, rust.y + rust.h / 2.0), |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!((tags.len(), expect_node(&ui, "Remove rust").focused), (1, true));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| says(ui, &mut tags, &mut text, &mut changes));
    assert_eq!((tags.len(), changes), (0, 5));
}

#[test]
fn stars_are_given_by_the_pointer_and_by_the_keys_in_halves() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut stars, mut changes) = (0.0f32, 0);
    let says = |ui: &mut Ui, stars: &mut f32, changes: &mut u32| *changes += ui.rating(stars, 5, 0.5).changed as u32;
    settle(&mut ui, |ui| says(ui, &mut stars, &mut changes));
    let none = ui.screen.pixels.clone();
    // Five stars, 20 wide each. Under the pointer, three and a half show what a press
    // would give, and nothing is given yet.
    does(&mut ui, Event::Move(pt(65.0, 10.0)), |ui| says(ui, &mut stars, &mut changes));
    assert!(ui.screen.pixels != none && stars == 0.0);
    let amber = ui.theme.amber.0;
    assert_eq!((pixel(&ui, 50, 10), pixel(&ui, 68, 10) == amber, pixel(&ui, 74, 10) == amber), (amber, true, false), "the half of the fourth star");
    click(&mut ui, pt(65.0, 10.0), |ui| says(ui, &mut stars, &mut changes));
    assert_eq!((stars, changes), (3.5, 1));
    // A press on what is given takes it back to none; on the last half, all of them.
    click(&mut ui, pt(65.0, 10.0), |ui| says(ui, &mut stars, &mut changes));
    assert_eq!(stars, 0.0);
    click(&mut ui, pt(99.0, 10.0), |ui| says(ui, &mut stars, &mut changes));
    assert_eq!(stars, 5.0);
    // The pointer gone, what shows is what is given.
    does(&mut ui, Event::Leave, |ui| says(ui, &mut stars, &mut changes));
    // The keys: a step, none, all.
    key(&mut ui, Key::Left, Mods::NONE, |ui| says(ui, &mut stars, &mut changes));
    assert_eq!(stars, 4.5);
    key(&mut ui, Key::Home, Mods::NONE, |ui| says(ui, &mut stars, &mut changes));
    assert_eq!(stars, 0.0);
    key(&mut ui, Key::End, Mods::NONE, |ui| says(ui, &mut stars, &mut changes));
    key(&mut ui, Key::Right, Mods::NONE, |ui| says(ui, &mut stars, &mut changes));
    assert_eq!(stars, 5.0);
    // A screen reader hears a slider, and can set it: to the nearest half.
    let slider = ui.visible_access_nodes().iter().find(|node| node.role == Role::Slider).unwrap().clone();
    assert_eq!((slider.value_range, slider.step), (Some([5.0, 0.0, 5.0]), 0.5));
    ui.set_for_reader(slider.id, 2.2);
    ui.run(&mut |ui| says(ui, &mut stars, &mut changes));
    assert_eq!((stars, changes), (2.0, 7));
}

#[test]
fn pages_of_results_are_gone_through_by_their_numbers_and_their_arrows() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut page, mut changes) = (0usize, 0);
    let says = |ui: &mut Ui, page: &mut usize, changes: &mut u32| *changes += ui.pagination(page, 20).changed as u32;
    settle(&mut ui, |ui| says(ui, &mut page, &mut changes));
    let numbers = |ui: &Window| ui.visible_access_nodes().iter().filter(|node| matches!(node.role, Role::Tab | Role::Text)).map(|node| node.name.clone()).collect::<Vec<_>>().join(" ");
    // Four buttons with arrows of 30 leave 300, for seven numbers of 38.
    assert_eq!(numbers(&ui), "1 2 3 4 5 \u{2026} 20");
    assert_eq!((expect_node(&ui, "1").on, expect_node(&ui, "1").position, expect_node(&ui, "Previous page").enabled), (Some(true), Some([1, 20]), false));
    click_on(&mut ui, "Next page", |ui| says(ui, &mut page, &mut changes));
    click_on(&mut ui, "5", |ui| says(ui, &mut page, &mut changes));
    assert_eq!((page, changes, numbers(&ui)), (4, 2, "1 \u{2026} 4 5 6 \u{2026} 20".to_string()));
    // The keyboard stays on the number it pressed, wherever that number goes.
    assert!(expect_node(&ui, "5").focused);
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| says(ui, &mut page, &mut changes));
    assert!(expect_node(&ui, "4").focused);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| says(ui, &mut page, &mut changes));
    assert_eq!((page, numbers(&ui)), (3, "1 2 3 4 5 \u{2026} 20".to_string()));
    assert!(expect_node(&ui, "4").focused);
    click_on(&mut ui, "Last page", |ui| says(ui, &mut page, &mut changes));
    assert_eq!((page, expect_node(&ui, "Next page").enabled, numbers(&ui)), (19, false, "1 \u{2026} 16 17 18 19 20".to_string()));
    // At the smallest a window is kept, three numbers.
    resized(&mut ui, 281.0, 220.0);
    settle(&mut ui, |ui| says(ui, &mut page, &mut changes));
    assert_eq!(numbers(&ui), "18 19 20");
    assert!(expect_node(&ui, "Last page").rect.right() <= 281.0);
}

#[test]
fn a_split_button_does_the_usual_thing_and_its_arrow_opens_the_other_ways() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut saved, mut other) = (0, 0);
    let says = |ui: &mut Ui, saved: &mut u32, other: &mut u32| {
        ui.row().pad(10.0).open();
        let save = ui.split_button("Save");
        *saved += save.clicked as u32;
        if ui.menu(save.id) {
            *other += ui.menu_item("Save as").clicked as u32;
            ui.close();
        }
        ui.close();
    };
    settle(&mut ui, |ui| says(ui, &mut saved, &mut other));
    assert_eq!(role_list(&ui, &[Role::Button, Role::Fold]), "Button Save, Fold More");
    click_on(&mut ui, "Save", |ui| says(ui, &mut saved, &mut other));
    assert_eq!((saved, expect_node(&ui, "More").on), (1, Some(false)));
    click_on(&mut ui, "More", |ui| says(ui, &mut saved, &mut other));
    let (more, menu) = (expect_node(&ui, "More").rect, expect_node(&ui, "Save as").rect);
    assert!(expect_node(&ui, "More").on == Some(true) && menu.y > more.bottom() && menu.x <= expect_node(&ui, "Save").rect.x + 5.0, "{menu:?}");
    click(&mut ui, middle(menu), |ui| says(ui, &mut saved, &mut other));
    assert_eq!((saved, other, expect_node(&ui, "More").on), (1, 1, Some(false)));
    // Alt and Down open it from the keyboard, Escape closes it.
    click_on(&mut ui, "Save", |ui| says(ui, &mut saved, &mut other));
    key(&mut ui, Key::Down, Mods { alt: true, ..Mods::NONE }, |ui| says(ui, &mut saved, &mut other));
    assert_eq!(expect_node(&ui, "More").on, Some(true));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| says(ui, &mut saved, &mut other));
    assert_eq!((saved, expect_node(&ui, "More").on), (2, Some(false)));
}

#[test]
fn a_box_to_search_with_is_emptied_by_its_cross_and_by_escape() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut text, mut changes, mut escapes) = (String::new(), 0, 0);
    let says = |ui: &mut Ui, text: &mut String, changes: &mut u32, escapes: &mut u32| {
        *changes += ui.search(text, "Find").changed as u32;
        *escapes += ui.key(Key::Escape, Mods::NONE) as u32;
    };
    settle(&mut ui, |ui| says(ui, &mut text, &mut changes, &mut escapes));
    let typing = expect_node(&ui, "Find").rect;
    click(&mut ui, middle(typing), |ui| says(ui, &mut text, &mut changes, &mut escapes));
    typed(&mut ui, "apples", |ui| says(ui, &mut text, &mut changes, &mut escapes));
    assert_eq!((text.as_str(), changes), ("apples", 1));
    // Escape empties it, and goes no further; in the empty box, it goes on to the app.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| says(ui, &mut text, &mut changes, &mut escapes));
    assert_eq!((text.as_str(), changes, escapes), ("", 2, 0));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| says(ui, &mut text, &mut changes, &mut escapes));
    assert_eq!(escapes, 1);
    // The cross shows while there is something, and empties it; the keyboard stays.
    typed(&mut ui, "pears", |ui| says(ui, &mut text, &mut changes, &mut escapes));
    click(&mut ui, pt(typing.right() - 15.0, typing.y + typing.h / 2.0), |ui| says(ui, &mut text, &mut changes, &mut escapes));
    assert_eq!((text.as_str(), changes, expect_node(&ui, "Find").focused), ("", 4, true));
    // What is typed starts after the looking glass, and never runs under the cross.
    typed(&mut ui, "a long long long long long long long long text", |ui| says(ui, &mut text, &mut changes, &mut escapes));
    let ground = ui.theme.panel.0;
    assert!((typing.x as usize + 2..typing.x as usize + 8).all(|x| pixel(&ui, x, 14) == ground), "nothing of the text before the glass");
}

#[test]
fn a_person_is_a_round_picture_or_their_first_letters_on_a_colour_of_their_own() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let face = ui.add_picture(4, vec![0xffff_0000; 16]);
    let says = |ui: &mut Ui| {
        ui.row().pad(10.0).gap(10.0).open();
        ui.avatar("Ada Lovelace", Some(face), 40.0);
        ui.avatar("Alan Turing", None, 40.0);
        ui.close();
    };
    settle(&mut ui, says);
    assert_eq!(role_list(&ui, &[Role::Text]), "Text Ada Lovelace, Text Alan Turing");
    // The picture fills a circle: red in the middle and at the middle of each edge, the
    // page in its corners.
    let ground = ui.theme.bg.0;
    assert_eq!([pixel(&ui, 30, 30), pixel(&ui, 11, 30), pixel(&ui, 30, 48)], [0xffff_0000; 3]);
    assert_eq!([pixel(&ui, 11, 11), pixel(&ui, 48, 48)], [ground; 2]);
    // The letters: on a colour, round as well.
    let letters = expect_node(&ui, "Alan Turing").rect;
    let (x, y) = (letters.x as usize, letters.y as usize);
    assert!(pixel(&ui, x + 2, y + 20) != ground && pixel(&ui, x + 1, y + 1) == ground);
}

#[test]
fn a_card_is_pressed_as_one_but_its_own_buttons_are_pressed_by_themselves() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut opened, mut liked) = (0, 0);
    let says = |ui: &mut Ui, opened: &mut u32, liked: &mut u32| {
        ui.row().pad(10.0).open();
        let card = ui.card();
        ui.label("Mountains");
        ui.note("A walk of four hours");
        *liked += ui.button("Like").clicked as u32;
        ui.close();
        *opened += card.clicked as u32;
        ui.close();
    };
    settle(&mut ui, |ui| says(ui, &mut opened, &mut liked));
    assert_eq!(role_list(&ui, &[Role::Group, Role::Button]), "Group Mountains, Button Like");
    click_on(&mut ui, "A walk of four hours", |ui| says(ui, &mut opened, &mut liked));
    assert_eq!((opened, liked), (1, 0));
    click_on(&mut ui, "Like", |ui| says(ui, &mut opened, &mut liked));
    assert_eq!((opened, liked), (1, 1));
    // The card is a stop of the keyboard, and Enter presses it.
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| says(ui, &mut opened, &mut liked));
    assert!(expect_node(&ui, "Mountains").focused);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| says(ui, &mut opened, &mut liked));
    assert_eq!((opened, liked), (2, 1));
}

#[test]
fn a_banner_is_said_once_holds_the_apps_buttons_and_is_closed_by_its_cross() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut shown, mut acted) = (false, 0);
    let says = |ui: &mut Ui, shown: &mut bool, acted: &mut u32| {
        if *shown {
            let banner = ui.banner(Severity::Warning, "The disk is nearly full");
            *acted += ui.button("Free some room").clicked as u32;
            ui.close();
            *shown &= !banner.clicked;
        }
        ui.label("The page");
    };
    settle(&mut ui, |ui| says(ui, &mut shown, &mut acted));
    let page = expect_node(&ui, "The page").rect;
    shown = true;
    ui.run(&mut |ui| says(ui, &mut shown, &mut acted));
    assert_eq!(ui.announced(), ["The disk is nearly full".to_string()]);
    ui.run(&mut |ui| says(ui, &mut shown, &mut acted));
    assert!(ui.announced().is_empty(), "said once, when it came");
    assert_eq!(role_list(&ui, &[Role::Group, Role::Button]), "Group The disk is nearly full, Button Close, Button Free some room");
    assert!(expect_node(&ui, "The page").rect.y > page.y);
    // In the colour of what wants attention: more red than blue in it.
    let strip = pixel(&ui, 2, expect_node(&ui, "The disk is nearly full").rect.y as usize + 6);
    assert!(strip != ui.theme.bg.0 && (strip >> 16 & 0xff) > (strip & 0xff) + 8, "{strip:x}");
    click_on(&mut ui, "Free some room", |ui| says(ui, &mut shown, &mut acted));
    assert_eq!((acted, shown), (1, true));
    click_on(&mut ui, "Close", |ui| says(ui, &mut shown, &mut acted));
    assert_eq!((shown, expect_node(&ui, "The page").rect), (false, page));
}

#[test]
fn keys_are_written_as_keys_and_told_as_they_are_written() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let says = |ui: &mut Ui| {
        ui.row().gap(10.0).open();
        ui.shortcut("Ctrl+Shift+S");
        ui.shortcut("Ctrl++");
        ui.shortcut("F2");
        ui.close();
    };
    settle(&mut ui, says);
    assert_eq!(role_list(&ui, &[Role::Text]), "Text Ctrl+Shift+S, Text Ctrl++, Text F2");
    let [three, two, one] = [expect_node(&ui, "Ctrl+Shift+S").rect, expect_node(&ui, "Ctrl++").rect, expect_node(&ui, "F2").rect];
    assert!(three.w > two.w && two.w > one.w && one.w >= 20.0, "{three:?} {two:?} {one:?}");
    // Each key is a box of its own: the page shows between them.
    let ground = ui.theme.bg.0;
    let gaps = (two.x as usize..two.right() as usize).filter(|x| pixel(&ui, *x, (two.y + two.h / 2.0) as usize) == ground).count();
    assert_eq!(gaps, 3);
}

#[test]
fn a_placeholder_has_a_band_of_light_go_over_it_and_only_it_is_drawn_again() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let says = |ui: &mut Ui| {
        ui.column().pad(10.0).gap(8.0).open();
        ui.skeleton().show();
        ui.skeleton().size(40.0, 40.0).radius(20.0).show();
        ui.close();
        ui.label("Below");
    };
    settle(&mut ui, says);
    // Nothing is told of it, and it asks to be drawn again within a sixtieth of a second.
    assert_eq!(role_list(&ui, &[Role::Progress, Role::Text]), "Text Below");
    assert!(ui.wake_in().is_some_and(|wait| wait <= Duration::from_millis(16)));
    let before = ui.screen.pixels.clone();
    ui.set_time(Duration::from_millis(500));
    let changed = ui.run(&mut |ui| says(ui));
    assert!(ui.screen.pixels != before && changed.bottom() <= 10.0 + 12.0 + 8.0 + 40.0, "{changed:?}");
    // A line of text: as wide as it may be, 12 high.
    let line = ui.screen.width - 10;
    assert!(pixel(&ui, line - 1, 15) != ui.theme.bg.0 && pixel(&ui, line + 1, 15) == ui.theme.bg.0);
}

#[test]
fn what_the_guide_has_an_app_write_itself_works_as_it_says() {
    const BELL: taut::Icon = taut::Icon::line("M8 2.5a3.5 3.5 0 0 1 3.5 3.5v2.5l1.2 2H3.3l1.2-2V6A3.5 3.5 0 0 1 8 2.5zM6.6 12.5a1.5 1.5 0 0 0 2.8 0");
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    let (mut styles, mut open) = ([false; 3], Some(0));
    let says = |ui: &mut Ui, styles: &mut [bool; 3], open: &mut Option<usize>| {
        let t = ui.theme;
        ui.row().pad(20.0).gap(20.0).open();
        let row = ui.row().place((taut::Align::Start, taut::Align::Center)).pad(2.0).gap(2.0).fill(t.panel).border(t.line, 1.0).radius(t.radius).open();
        ui.tell(&row, Role::Group, None, "Text style");
        for (on, name) in styles.iter_mut().zip(["Bold", "Italic", "Underline"]) {
            ui.toggle(on, name);
        }
        ui.close();
        ui.el().flow(taut::Flow::Stack).open();
        ui.icon_button(BELL, "Notices, 3 new");
        let badge = ui.el().place((taut::Align::End, taut::Align::Start)).offset(6.0, -6.0).min(16.0, 16.0).pad((4.0, 0.0)).radius(8.0).fill(t.red);
        badge.text("3", t.small_font, t.on_accent).text_align(taut::Align::Center).show();
        ui.close();
        ui.close();
        for (number, title) in ["One", "Two"].into_iter().enumerate() {
            let mut shows = *open == Some(number);
            if ui.fold(&mut shows, title) {
                ui.label(&format!("In {title}"));
            }
            *open = if shows { Some(number) } else { open.filter(|was| *was != number) };
        }
    };
    settle(&mut ui, |ui| says(ui, &mut styles, &mut open));
    assert_eq!(role_list(&ui, &[Role::Group, Role::Button]), "Group Text style, Button Bold, Button Italic, Button Underline, Button Notices, 3 new");
    // The count hangs over the button's corner, outside the button, and is drawn there.
    let bell = expect_node(&ui, "Notices, 3 new").rect;
    assert_eq!(pixel(&ui, bell.right() as usize + 3, bell.y as usize - 3), ui.theme.red.0);
    // Of the sections, one is open at a time.
    assert!(find_node(&ui, "In One").is_some());
    click_on(&mut ui, "Two", |ui| says(ui, &mut styles, &mut open));
    assert_eq!(open, Some(1));
    assert!(find_node(&ui, "In Two").is_some() && find_node(&ui, "In One").is_none());
    click_on(&mut ui, "Two", |ui| says(ui, &mut styles, &mut open));
    assert_eq!(open, None);
    click_on(&mut ui, "Italic", |ui| says(ui, &mut styles, &mut open));
    assert_eq!(styles, [false, true, false]);
}
