//! Every part, used by someone who cannot use a pointer and by someone who cannot see the
//! screen: each is operated with keys only, and what screen readers hear is compared with what
//! system controls report (the WAI-ARIA Authoring Practices for each kind of part are the
//! yardstick). Each part is tried at scales 1, 2 and 0.75, and with enlarged theme text.
//!
//! A test does not stop at the first failure: it continues, and reports everything wrong at the
//! end, with the scales.
mod common;

use common::*;
use std::cell::RefCell;
use taut::geom::Size;
use taut::{AccessNode, Chart, Color, Column, Date, Dock, FileDialog, Font, Icon, Key, Mods, Panel, Rect, Role, SecondWindow, Selection, Series, Table, Ui};

const STAR: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");

/// One way the window is shown: how many pixels a unit is, and how many times its own size
/// the theme's text is made. (The built-in letters grow in whole dots: at 2.5 times the
/// size they come out twice as large, 24 by 40 in place of 12 by 20.)
#[derive(Clone, Copy, Debug)]
struct Sizing {
    scale: f32,
    text: f32,
}

const SIZES: [Sizing; 4] = [Sizing { scale: 1.0, text: 1.0 }, Sizing { scale: 2.0, text: 1.0 }, Sizing { scale: 0.75, text: 1.0 }, Sizing { scale: 1.0, text: 2.5 }];

impl Sizing {
    fn describe(self) -> String {
        match (self.scale, self.text) {
            (scale, 1.0) => format!("at a scale of {scale}"),
            (_, _) => "with the text twice as large".to_string(),
        }
    }
}

/// What went wrong with one part, gathered over every size it was tried at, so that one run
/// says all of it: each thing once, with the sizes it went wrong at.
struct Audit {
    part: &'static str,
    wrong: Vec<(String, Vec<String>)>,
    sizing: Sizing,
}

impl Audit {
    fn new(part: &'static str) -> Self {
        Audit { part, wrong: Vec::new(), sizing: SIZES[0] }
    }
    /// Says that `what` should hold, and keeps it if it does not.
    fn check(&mut self, holds: bool, what: impl Into<String>) {
        if holds {
            return;
        }
        let (what, at) = (what.into(), self.sizing.describe());
        match self.wrong.iter_mut().find(|(known, _)| *known == what) {
            Some((_, sizes)) if sizes.last() == Some(&at) => {}
            Some((_, sizes)) => sizes.push(at),
            None => self.wrong.push((what, vec![at])),
        }
    }
    /// Runs `test` at every size, then fails with all that went wrong, if anything did. A
    /// size at which the test stopped (the window was drawn wrong, say) is one more thing
    /// that went wrong, and the other sizes are still tried.
    fn at_every_size(mut self, test: impl Fn(&mut Audit, Sizing)) {
        for sizing in SIZES {
            self.sizing = sizing;
            let mut audit = Audit { part: self.part, wrong: std::mem::take(&mut self.wrong), sizing };
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                test(&mut audit, sizing);
                audit.wrong.clone()
            }));
            match stopped {
                Ok(wrong) => self.wrong = wrong,
                Err(why) => {
                    let why = why.downcast_ref::<String>().cloned().or(why.downcast_ref::<&str>().map(|why| why.to_string())).unwrap_or_default();
                    self.wrong = audit.wrong;
                    self.check(false, format!("the test stopped: {}", why.split(" in pixels").next().unwrap_or("")));
                }
            }
        }
        let every = |sizes: &Vec<String>| if sizes.len() == SIZES.len() { "at every size".to_string() } else { sizes.join(", ") };
        let lines: Vec<String> = self.wrong.iter().map(|(what, sizes)| format!("- {what} ({})", every(sizes))).collect();
        assert!(lines.is_empty(), "{}:\n{}", self.part, lines.join("\n"));
    }
}

/// A window on no screen, shown this way, that a screen reader listens to.
fn sized(sizing: Sizing) -> Window {
    let mut ui = window(sizing.scale);
    ui.theme = ui.theme.text_scaled(sizing.text);
    ui.set_screen_reader(true);
    ui
}

/// What a screen reader is given of a node, but where it lies: for messages that are the
/// same at every size.
fn brief(node: &Option<AccessNode>) -> String {
    node.as_ref().map_or("nothing".into(), |node| {
        let mut text = format!("{:?} {:?}", node.role, node.name);
        let parts = [
            (!node.value.is_empty()).then(|| format!("value {:?}", node.value)),
            node.on.map(|on| format!("on {on}")),
            node.position.map(|of| format!("{} of {}", of[0], of[1])),
            node.value_range.map(|amount| format!("at {}", amount[0])),
        ];
        parts.into_iter().flatten().for_each(|part| text += &format!(", {part}"));
        text
    })
}

/// The name of the node the keyboard is on; "nothing" when there is none.
fn focused_name(ui: &Window) -> String {
    focused_node(ui).map_or("nothing".into(), |node| node.name)
}
/// The row of a table the keyboard is on: the row itself, or the one the cell it is on is
/// in (in a table the keyboard is on a cell, as in ARIA's grid).
fn focused_row(ui: &Window) -> Option<AccessNode> {
    let here = focused_node(ui)?;
    match here.role {
        Role::Cell => here.parent.map(|row| ui.visible_access_nodes()[row].clone()),
        Role::TableRow => Some(here),
        _ => None,
    }
}
fn press(ui: &mut Window, key_: Key, build: &mut impl FnMut(&mut Ui)) {
    key(ui, key_, Mods::NONE, &mut *build);
}

/// The pixels of a picture of the window (`picture`, `width` pixels across, at `scale`)
/// inside `area`, which is in units: row by row.
fn pixels(picture: &[u32], width: usize, scale: f32, area: Rect) -> Vec<u32> {
    let height = picture.len() / width.max(1);
    let left = ((area.x * scale).floor().max(0.0) as usize).min(width);
    let right = (((area.x + area.w) * scale).ceil().max(0.0) as usize).min(width);
    let top = ((area.y * scale).floor().max(0.0) as usize).min(height);
    let bottom = (((area.y + area.h) * scale).ceil().max(0.0) as usize).min(height);
    (top..bottom).flat_map(|y| picture[y * width + left..y * width + right].iter().copied()).collect()
}

/// Whether the box at `area` looks other in the window's picture than in `plain`, so that
/// it can be seen: as many pixels differ clearly (by a fifth or more in a colour) as half
/// of a ring round it would have.
fn stands_out(plain: &[u32], ui: &Window, area: Rect) -> bool {
    let (width, scale) = (ui.screen.width, ui.scale());
    let clearly = |a: u32, b: u32| (0..3).any(|byte| ((a >> (8 * byte)) as u8).abs_diff((b >> (8 * byte)) as u8) >= 51);
    let differ = pixels(plain, width, scale, area).into_iter().zip(pixels(&ui.screen.pixels, width, scale, area)).filter(|(a, b)| clearly(*a, *b)).count();
    differ as f32 >= (area.w + area.h) * scale
}

/// Goes round the window with Tab (or with Shift and Tab, `back`) from wherever the keyboard
/// is, until it comes back to where it started or has gone `most` steps: the things the
/// keyboard was on, as told. At each step it checks that the place of the keyboard is
/// shown: `focus_visible`, and the box looks other than it did before the keyboard came
/// (`plain`: the window's picture with the keyboard on nothing).
fn tab_round(audit: &mut Audit, ui: &mut Window, build: &mut impl FnMut(&mut Ui), back: bool, most: usize, plain: &[u32]) -> Vec<AccessNode> {
    let mut went: Vec<AccessNode> = Vec::new();
    for _ in 0..most {
        key(ui, Key::Tab, if back { Mods::SHIFT } else { Mods::NONE }, &mut *build);
        let Some(here) = focused_node(ui) else {
            audit.check(false, format!("Tab took the keyboard to something a screen reader is not told of, after {:?}", went.last().map(|node| node.name.clone())));
            break;
        };
        if went.first().is_some_and(|first| first.id == here.id) {
            break;
        }
        audit.check(ui.focus_visible(), format!("the keyboard is not shown as being on {:?} ({:?}), where Tab took it", here.name, here.role));
        // (What the keyboard is on is in view, all of it: the window's page scrolls it there
        // where it was cut off by the window's edge.)
        let room = ui.size();
        let whole = here.rect.x >= -0.5 && here.rect.y >= -0.5 && here.rect.right() <= room.w + 0.5 && here.rect.bottom() <= room.h + 0.5;
        audit.check(
            whole || matches!(here.role, Role::TableRow | Role::Cell | Role::Row),
            format!("{:?} ({:?}) lies at {:?}, out of the window, with the keyboard on it", here.name, here.role, here.rect),
        );
        let shown = plain.is_empty() || plain.len() == ui.screen.pixels.len() && stands_out(plain, ui, here.rect);
        audit.check(shown, format!("{:?} ({:?}) looks the same with the keyboard on it as without: the keyboard's place is not seen", here.name, here.role));
        went.push(here);
    }
    went
}

/// How wide so many of the built-in letters are, in units.
fn letters(count: usize, font: Font, sizing: Sizing) -> f32 {
    count as f32 * (font.size * sizing.scale / 8.0).round().max(1.0) * 6.0 / sizing.scale
}

/// What every part is checked for, wherever the keyboard can be: each thing it can be on
/// has a name; and each thing that holds words is high enough for a line of them (but for
/// the ones `unwritten`, whose names are not written in them: an icon called by its hint).
fn looked_over(audit: &mut Audit, ui: &Window, sizing: Sizing, unwritten: &[&str]) {
    let t = ui.theme;
    let line = |font: Font| ((font.size * sizing.scale / 8.0).round().max(1.0) * 10.0 / sizing.scale).floor();
    let room = ui.size();
    for node in ui.visible_access_nodes() {
        let keyed = !matches!(node.role, Role::Text | Role::Progress) && !node.role.is_container() || node.role == Role::TableRow && node.on.is_some();
        if keyed {
            audit.check(!node.name.trim().is_empty(), format!("a {:?} is told with no name (its value: {:?})", node.role, node.value));
        }
        // (Only where the name is written in the box: an icon button is called by its hint.)
        let font = if node.role == Role::ColumnHeader { t.small_font } else { t.font };
        let written = !node.name.is_empty() && !unwritten.contains(&node.name.as_str()) && node.rect.w >= letters(node.name.chars().count(), font, sizing);
        let worded = written
            && matches!(
                node.role,
                Role::Button | Role::Tab | Role::MenuItem | Role::TextBox | Role::PasswordBox | Role::ComboBox | Role::Fold | Role::Row | Role::Link | Role::CheckBox | Role::ColumnHeader
            );
        if worded {
            audit.check(node.rect.h + 0.5 >= line(font), format!("{:?} ({:?}) is {} high, and its words {}: they are cut off", node.name, node.role, node.rect.h, line(font)));
        }
        // (Or cut off by the window's top or bottom edge, where the window's page is taller
        // than the window: it scrolls into view as the keyboard gets there, which `tab_round`
        // holds it to.)
        let inside = node.rect.x >= -0.5 && node.rect.y < room.h && node.rect.right() <= room.w + 0.5 && node.rect.bottom() > 0.0;
        if keyed && !matches!(node.role, Role::TableRow | Role::Cell | Role::Row) {
            audit.check(inside, format!("{:?} ({:?}) lies at {:?}, out of the window", node.name, node.role, node.rect));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Things to press: buttons and their kinds, and links.

#[derive(Default)]
struct Presses {
    saved: u32,
    sent: u32,
    starred: u32,
    bold: bool,
    italic: bool,
    followed: u32,
}

fn presses(app: &mut Presses, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    app.saved += ui.button("Save").clicked as u32;
    app.sent += ui.primary_button("Send").clicked as u32;
    app.starred += ui.icon_button(STAR, "Add a star").clicked as u32;
    ui.toggle(&mut app.bold, "Bold");
    ui.icon_toggle(&mut app.italic, STAR, "Italic");
    app.followed += ui.link("Read the guide").clicked as u32;
    ui.set_enabled(false);
    ui.button("Not now");
    ui.set_enabled(true);
    ui.close();
}

#[test]
fn buttons_and_links_are_reached_pressed_and_told() {
    Audit::new("buttons, toggles and links").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Presses::default()), sized(sizing));
        let mut build = |ui: &mut Ui| presses(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &["Add a star", "Italic"]);
        let plain = ui.screen.pixels.clone();
        let names: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 12, &plain).into_iter().map(|node| node.name).collect();
        audit.check(names.starts_with(&["Save", "Send", "Add a star", "Bold", "Italic", "Read the guide"].map(String::from)), format!("Tab goes {names:?}"));
        let back: Vec<String> = tab_round(audit, &mut ui, &mut build, true, 12, &plain).into_iter().map(|node| node.name).collect();
        audit.check(back.iter().rev().cloned().collect::<Vec<_>>().ends_with(&names[..names.len().min(6)]) || back.len() == names.len(), format!("Shift and Tab go {back:?}"));

        // What each is reported as: the kind, the name, how it stands. (With the window's page
        // scrolled to its end: where it is taller than the window, the last is told only once
        // it is in view.)
        does(&mut ui, taut::Event::Move(taut::geom::pt(20.0, 20.0)), &mut build);
        does(&mut ui, taut::Event::Wheel(0.0, 10_000.0), &mut build);
        let kinds: Vec<(Role, String, Option<bool>, bool)> = ui.visible_access_nodes().iter().map(|node| (node.role, node.name.clone(), node.on, node.enabled)).collect();
        audit.check(kinds.contains(&(Role::Button, "Add a star".into(), None, true)), format!("an icon button is told by its hint: {kinds:?}"));
        audit.check(kinds.contains(&(Role::Link, "Read the guide".into(), None, true)), "a link is told as a link");
        audit.check(kinds.contains(&(Role::Button, "Not now".into(), None, false)), "a button that is switched off is told as not usable");
        // A button that stays down is a toggle button (ARIA: a button with aria-pressed;
        // Windows: a button with the Toggle pattern; AT-SPI: a toggle button), not a tick box.
        let bold = find_node_as(&ui, Role::CheckBox, "Bold").or(find_node_as(&ui, Role::Button, "Bold")).or(find_node_as(&ui, Role::Switch, "Bold"));
        audit.check(bold.as_ref().is_some_and(|node| node.role != Role::CheckBox), format!("a button that stays down is told as a tick box: {:?}", bold.map(|node| node.role)));

        // Enter and Space press each of them; the toggles flip, and say so.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..6 {
            press(&mut ui, Key::Tab, &mut build);
            press(&mut ui, Key::Enter, &mut build);
            press(&mut ui, Key::Space, &mut build);
        }
        audit.check(
            (app.borrow().saved, app.borrow().sent, app.borrow().starred, app.borrow().followed) == (2, 2, 2, 2),
            format!("Enter and Space each pressed once: {:?}", (app.borrow().saved, app.borrow().sent, app.borrow().starred, app.borrow().followed)),
        );
        audit.check(!app.borrow().bold && !app.borrow().italic, "a toggle flipped twice is off again");
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..4 {
            press(&mut ui, Key::Tab, &mut build);
        }
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().bold && focused_node(&ui).is_some_and(|node| node.on == Some(true)), "a toggle that is on is told as on");
    });
}

// ---------------------------------------------------------------------------------------
// Tick boxes, switches and radio buttons.

#[derive(Default)]
struct Choices {
    wrap: bool,
    dark: bool,
    size: u8,
}

fn choices(app: &mut Choices, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.checkbox(&mut app.wrap, "Wrap lines");
    ui.switch(&mut app.dark, "Dark");
    ui.label("Size of the letters");
    for (value, name) in ["Small", "Medium", "Large"].iter().enumerate() {
        ui.radio(&mut app.size, value as u8, name);
    }
    ui.close();
}

#[test]
fn tick_boxes_switches_and_radio_buttons_are_set_with_keys_and_told() {
    Audit::new("tick boxes, switches and radio buttons").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Choices::default()), sized(sizing));
        let mut build = |ui: &mut Ui| choices(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let names: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 12, &plain).into_iter().map(|node| node.name).collect();
        // (A group of radio buttons is one stop for Tab in ARIA and on Windows: the arrows
        // go between them.)
        audit.check(names == ["Wrap lines", "Dark", "Small"], format!("Tab goes {names:?}; ARIA has one stop for a group of radio buttons, on the one that is picked"));
        ui.set_focus(0);
        settle(&mut ui, &mut build);

        // Space ticks the tick box and flips the switch, and each says how it stands.
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().wrap && focused_node(&ui).is_some_and(|node| (node.role, node.on) == (Role::CheckBox, Some(true))), "Space ticks the tick box, and it is told as ticked");
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().dark && focused_node(&ui).is_some_and(|node| (node.role, node.on) == (Role::Switch, Some(true))), "Space flips the switch, and it is told as on");

        // The radio buttons: reported as one of three, the selected one as selected; the arrows
        // go from one to the next and pick it (ARIA, and the system's own).
        press(&mut ui, Key::Tab, &mut build);
        let small = focused_node(&ui);
        audit.check(small.as_ref().is_some_and(|node| (node.role, node.on) == (Role::RadioButton, Some(true))), format!("the picked radio button is told as picked: {small:?}"));
        audit.check(small.as_ref().is_some_and(|node| node.position == Some([1, 3])), format!("a radio button is told as the first of three: {:?}", small.as_ref().map(|node| node.position)));
        press(&mut ui, Key::Down, &mut build);
        audit.check(app.borrow().size == 1 && focused_name(&ui) == "Medium", format!("Down picks the next radio button (picked: {}, the keyboard on {})", app.borrow().size, focused_name(&ui)));
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().size == 1, "Space picks the radio button the keyboard is on");
        let medium = find_node_as(&ui, Role::RadioButton, "Medium");
        audit.check(medium.is_some_and(|node| node.on == Some(true)), "the radio button picked is told as picked");
        press(&mut ui, Key::Up, &mut build);
        audit.check(app.borrow().size == 0 && focused_name(&ui) == "Small", format!("Up picks the one before (picked: {}, the keyboard on {})", app.borrow().size, focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// Sliders, and the slider with two knobs.

#[derive(Default)]
struct Slides {
    volume: f32,
    from: f32,
    to: f32,
}

/// A slider and a span, each under a label; and with `named`, the app names the slider
/// itself, as it would any box of its own.
fn slides(app: &mut Slides, ui: &mut Ui, named: bool) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Volume");
    let volume = ui.slider(&mut app.volume, 0.0..=100.0);
    if named {
        ui.tell(&volume, Role::Slider, None, "Volume");
    }
    ui.label("Price");
    ui.range(&mut app.from, &mut app.to, 0.0..=100.0);
    ui.close();
}

#[test]
fn a_slider_and_a_span_are_moved_with_keys_and_told() {
    Audit::new("sliders and the span slider").at_every_size(|audit, sizing| {
        let (app, mut ui, named) = (RefCell::new(Slides { volume: 50.0, from: 20.0, to: 80.0 }), sized(sizing), std::cell::Cell::new(false));
        let mut build = |ui: &mut Ui| slides(&mut app.borrow_mut(), ui, named.get());
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went = tab_round(audit, &mut ui, &mut build, false, 8, &plain);
        audit.check(
            went.len() == 3 && went.iter().all(|node| node.role == Role::Slider),
            format!("Tab goes to the slider and to both knobs: {:?}", went.iter().map(|node| node.role).collect::<Vec<_>>()),
        );

        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        let told_as = focused_node(&ui);
        audit.check(
            told_as.as_ref().is_some_and(|node| node.value_range == Some([50.0, 0.0, 100.0]) && node.step == 5.0),
            format!("the slider is told where it stands, its ends and its step: {}", brief(&told_as)),
        );
        // The keys of a slider (ARIA): Right and Up a step more, Left and Down a step less,
        // Home and End to the ends, Page Up and Page Down a larger step.
        let at = |ui: &Window| focused_node(ui).and_then(|node| node.value_range).map_or(f32::NAN, |amount| amount[0]);
        press(&mut ui, Key::Right, &mut build);
        audit.check(at(&ui) == 55.0, format!("Right moves it a step on: {}", at(&ui)));
        press(&mut ui, Key::Left, &mut build);
        audit.check(at(&ui) == 50.0, format!("Left moves it a step back: {}", at(&ui)));
        press(&mut ui, Key::Up, &mut build);
        audit.check(at(&ui) == 55.0, format!("Up does not move it a step on (ARIA): it stands at {}, the keyboard on {}", at(&ui), brief(&focused_node(&ui))));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        let start = at(&ui);
        press(&mut ui, Key::Down, &mut build);
        audit.check(at(&ui) == start - 5.0, format!("Down does not move it a step back (ARIA): the keyboard goes on to {}", brief(&focused_node(&ui))));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::End, &mut build);
        audit.check(at(&ui) == 100.0, format!("End takes it to its end: {}", at(&ui)));
        press(&mut ui, Key::Home, &mut build);
        audit.check(at(&ui) == 0.0, format!("Home takes it to its start: {}", at(&ui)));
        let before = at(&ui);
        press(&mut ui, Key::PageUp, &mut build);
        audit.check(at(&ui) > before + 5.0, format!("Page Up moves it further than a step: from {before} to {}", at(&ui)));

        // A slider is named by the app, the way any part with no words in it is.
        let slider = find_node_as(&ui, Role::Slider, "");
        audit.check(slider.is_none(), "a slider has no name, and nothing to give it one");
        named.set(true);
        settle(&mut ui, &mut build);
        named.set(false);
        let named = ui.visible_access_nodes().iter().find(|node| node.role == Role::Slider).map(|node| node.name.clone());
        let kept = ui.visible_access_nodes().iter().find(|node| node.role == Role::Slider).map(|node| (node.value_range.is_some(), node.step));
        audit.check(kept == Some((true, 5.0)), format!("named by the app, the slider is no longer told where it stands, nor its step: {kept:?}"));
        audit.check(named.as_deref() == Some("Volume"), format!("`ui.tell` after a slider does not name it: it is told as {named:?}"));

        // The two knobs: each moves with the arrows, neither passes the other, and each is
        // told with its own name (ARIA: a slider with two thumbs, each labelled).
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..2 {
            press(&mut ui, Key::Tab, &mut build);
        }
        let low = focused_node(&ui);
        press(&mut ui, Key::Right, &mut build);
        let low_after = focused_node(&ui).and_then(|node| node.value_range);
        audit.check(low_after.is_some_and(|amount| amount[0] == 25.0), format!("Right moves the low knob a step: {low_after:?}"));
        for _ in 0..20 {
            press(&mut ui, Key::Right, &mut build);
        }
        let low_end = focused_node(&ui).and_then(|node| node.value_range);
        audit.check(low_end.is_some_and(|amount| amount[0] <= 80.0), format!("the low knob does not pass the high one: {low_end:?}"));
        audit.check(low.as_ref().is_some_and(|node| !node.name.is_empty()), "the low knob of a span is told with no name");
        press(&mut ui, Key::Tab, &mut build);
        audit.check(focused_node(&ui).is_some_and(|node| !node.name.is_empty()), "the high knob of a span is told with no name");
    });
}

// ---------------------------------------------------------------------------------------
// Numbers: one typed and stepped, one dragged.

#[derive(Default)]
struct Amounts {
    copies: i32,
    zoom: f32,
}

/// A number and a dragged number, each under a label; with `named`, the app names the
/// number box itself.
fn amounts(app: &mut Amounts, ui: &mut Ui, named: bool) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Copies");
    let copies = ui.number(&mut app.copies, 1);
    if named {
        ui.tell_text(&copies, "Copies", &app.copies.to_string());
    }
    ui.label("Zoom");
    ui.drag_value(&mut app.zoom, 0.1);
    ui.close();
}

#[test]
fn numbers_are_stepped_typed_and_told() {
    Audit::new("numbers").at_every_size(|audit, sizing| {
        let (app, mut ui, named) = (RefCell::new(Amounts { copies: 5, zoom: 1.0 }), sized(sizing), std::cell::Cell::new(false));
        let mut build = |ui: &mut Ui| amounts(&mut app.borrow_mut(), ui, named.get());
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went = tab_round(audit, &mut ui, &mut build, false, 6, &plain);
        audit.check(went.len() == 2, format!("Tab goes to both numbers: it goes to {:?}", went.iter().map(|node| (node.role, node.value.clone())).collect::<Vec<_>>()));
        // (Named by its label, as a slider is, not by the number written in it.)
        let dragged = ui.visible_access_nodes().iter().find(|node| node.name == "Zoom" && node.role != Role::Text).cloned();
        let set = dragged.as_ref().is_some_and(|node| node.role == Role::Slider && node.value_range.is_some_and(|amount| amount[0] == 1.0));
        audit.check(set, format!("a dragged number is told as words, or not named by its label: {:?}", dragged.map(|node| (node.role, node.value_range))));

        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        let told_as = focused_node(&ui);
        audit.check(told_as.as_ref().is_some_and(|node| node.value == "5"), format!("the number box is told what it holds: {}", brief(&told_as)));
        press(&mut ui, Key::Up, &mut build);
        audit.check(
            app.borrow().copies == 6 && focused_node(&ui).is_some_and(|node| node.value == "6"),
            format!("Up adds a step, and the box is told so: {:?}", focused_node(&ui).map(|node| node.value)),
        );
        press(&mut ui, Key::Down, &mut build);
        press(&mut ui, Key::Down, &mut build);
        audit.check(app.borrow().copies == 4, "Down takes a step away");
        key(&mut ui, Key::Char('a'), Mods::CTRL, &mut build);
        typed(&mut ui, "42", &mut build);
        audit.check(app.borrow().copies == 42, format!("what is typed is the number: {}", app.borrow().copies));
        named.set(true);
        settle(&mut ui, &mut build);
        audit.check(find_node_as(&ui, Role::SpinButton, "Copies").or(find_node_as(&ui, Role::TextBox, "Copies")).is_some(), "`ui.tell_text` after a number box does not name it");
        named.set(false);

        // The dragged number: reached by Tab, and moved by the arrows (as a slider is).
        press(&mut ui, Key::Tab, &mut build);
        let was = app.borrow().zoom;
        press(&mut ui, Key::Right, &mut build);
        audit.check(app.borrow().zoom != was, format!("the arrows change a dragged number: it stays {was}, the keyboard is on {:?}", focused_node(&ui).map(|node| node.role)));
    });
}

// ---------------------------------------------------------------------------------------
// Text boxes: one line, a password, several lines, and one under a label.

#[derive(Default)]
struct Typing {
    name: String,
    secret: String,
    notes: String,
    town: String,
}

fn typing(app: &mut Typing, ui: &mut Ui, named: bool) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.text_box(&mut app.name, "Your name");
    ui.password_box(&mut app.secret, "Password");
    ui.text_area(&mut app.notes, "Notes", (2, 4));
    ui.label("Town");
    let town = ui.text_box(&mut app.town, "");
    if named {
        ui.tell_text(&town, "Town", &app.town);
    }
    ui.close();
}

#[test]
fn text_boxes_are_typed_into_left_with_tab_and_told() {
    Audit::new("text boxes and the password box").at_every_size(|audit, sizing| {
        let (app, mut ui, named) = (RefCell::new(Typing::default()), sized(sizing), std::cell::Cell::new(false));
        let mut build = |ui: &mut Ui| typing(&mut app.borrow_mut(), ui, named.get());
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went: Vec<(Role, String)> = tab_round(audit, &mut ui, &mut build, false, 8, &plain).into_iter().map(|node| (node.role, node.name)).collect();
        let wanted = [(Role::TextBox, "Your name"), (Role::PasswordBox, "Password"), (Role::TextBox, "Notes"), (Role::TextBox, "Town")].map(|(role, name)| (role, name.to_string()));
        audit.check(went == wanted, format!("Tab goes {went:?}"));
        named.set(true);
        settle(&mut ui, &mut build);
        audit.check(find_node_as(&ui, Role::TextBox, "Town").is_some(), "a text box under a label has no name, and `ui.tell_text` after it does not give it one");
        named.set(false);

        // Typing, reported as the value and where the cursor is.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        typed(&mut ui, "Ada", &mut build);
        let name = focused_node(&ui);
        audit.check(name.as_ref().is_some_and(|node| node.value == "Ada" && node.caret == Some([3, 3])), format!("what is typed is told, and where the cursor is: {}", brief(&name)));
        // Up and Down mean nothing in one line (as on Windows and GTK): the keyboard stays.
        press(&mut ui, Key::Down, &mut build);
        audit.check(focused_name(&ui) == "Your name", format!("Down in a box of one line takes the keyboard out of it, to {:?}", focused_name(&ui)));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        // The menu of a box that is typed into opens from the keys too (Shift and F10, as
        // everywhere).
        key(&mut ui, Key::F(10), Mods::SHIFT, &mut build);
        let menu = ui.visible_access_nodes().iter().any(|node| node.role == Role::Menu);
        audit.check(menu, "Shift and F10 do not open the menu of a text box (cut, copy, paste): only the right button does");
        press(&mut ui, Key::Escape, &mut build);

        // The password: reported as one, by its dots only.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        typed(&mut ui, "pw", &mut build);
        let secret = focused_node(&ui);
        audit.check(
            secret.as_ref().is_some_and(|node| node.role == Role::PasswordBox && !node.value.contains('p') && node.value.chars().count() == 2),
            format!("a password is told by its dots: {}", brief(&secret)),
        );
        // Several lines: Enter starts a line, and Tab leaves (the keyboard is never stuck).
        press(&mut ui, Key::Tab, &mut build);
        typed(&mut ui, "one", &mut build);
        press(&mut ui, Key::Enter, &mut build);
        typed(&mut ui, "two", &mut build);
        audit.check(app.borrow().notes == "one\ntwo", format!("Enter starts a new line: {:?}", app.borrow().notes));
        press(&mut ui, Key::Tab, &mut build);
        audit.check(focused_node(&ui).is_some_and(|node| node.rect.y > 0.0 && node.value.is_empty()), "Tab leaves a box of several lines for the next one");
    });
}

// ---------------------------------------------------------------------------------------
// The select box.

#[derive(Default)]
struct Picking {
    size: usize,
}

fn picking(app: &mut Picking, ui: &mut Ui, named: bool) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Size");
    let size = ui.select(&mut app.size, &["Small", "Medium", "Large"]);
    if named {
        ui.tell(&size, Role::ComboBox, None, "Size");
    }
    ui.label("after it");
    ui.close();
}

#[test]
fn a_select_box_is_opened_picked_from_and_told() {
    Audit::new("the select box").at_every_size(|audit, sizing| {
        let (app, mut ui, named) = (RefCell::new(Picking::default()), sized(sizing), std::cell::Cell::new(false));
        let mut build = |ui: &mut Ui| picking(&mut app.borrow_mut(), ui, named.get());
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went = tab_round(audit, &mut ui, &mut build, false, 4, &plain);
        let closed = went.first().cloned();
        audit.check(
            closed.as_ref().is_some_and(|node| node.role == Role::ComboBox && node.value == "Small" && node.on == Some(false)),
            format!("the select box is told what is picked, and that it is shut: {}", brief(&closed)),
        );
        named.set(true);
        settle(&mut ui, &mut build);
        audit.check(find_node_as(&ui, Role::ComboBox, "Size").is_some(), "a select box has no name, and `ui.tell` after it does not give it one");
        let kept = ui.visible_access_nodes().iter().find(|node| node.role == Role::ComboBox).map(|node| (node.value.clone(), node.on));
        audit.check(kept == Some(("Small".into(), Some(false))), format!("named by the app, the select box is no longer told what is picked, nor whether it is open: {kept:?}"));
        named.set(false);
        settle(&mut ui, &mut build);

        // Enter opens it, with the keyboard on the option that is selected (ARIA).
        press(&mut ui, Key::Enter, &mut build);
        let open = ui.visible_access_nodes().iter().find(|node| node.role == Role::ComboBox).map(|node| node.on);
        audit.check(open == Some(Some(true)), format!("open, it is told as open: {open:?}"));
        audit.check(focused_name(&ui) == "Small", format!("as it opens the keyboard is on {:?}, not on the option that is picked", focused_name(&ui)));
        let options: Vec<(String, Option<[u32; 2]>, Option<bool>)> =
            ui.visible_access_nodes().iter().filter(|node| node.role == Role::MenuItem).map(|node| (node.name.clone(), node.position, node.on)).collect();
        audit.check(options.iter().all(|(_, of, _)| of.is_some()), format!("the options are not told which of how many they are: {options:?}"));
        audit.check(options.iter().any(|(name, _, on)| name == "Small" && *on == Some(true)), "the option that is picked is not told as picked");
        // Down to the next, Enter selects it: shut, the keyboard back on the box, which says
        // what is selected now.
        while !matches!(focused_name(&ui).as_str(), "Medium" | "Large" | "after it") {
            let was = focused_name(&ui);
            press(&mut ui, Key::Down, &mut build);
            if focused_name(&ui) == was {
                break;
            }
        }
        press(&mut ui, Key::Enter, &mut build);
        let shut = focused_node(&ui);
        audit.check(
            app.borrow().size == 1 && shut.as_ref().is_some_and(|node| node.role == Role::ComboBox && node.value == "Medium" && node.on == Some(false)),
            format!("Down and Enter pick Medium, and the keyboard is back on the box: {:?}, {shut:?}", app.borrow().size),
        );
        // Escape shuts it with nothing selected.
        press(&mut ui, Key::Space, &mut build);
        press(&mut ui, Key::Escape, &mut build);
        let shut = focused_node(&ui);
        audit.check(
            shut.as_ref().is_some_and(|node| node.role == Role::ComboBox && node.on == Some(false)) && app.borrow().size == 1,
            format!("Escape shuts it, the keyboard back on the box: {}", brief(&shut)),
        );
        // Down on the shut box opens it (ARIA), or steps the choice (Windows): it does not
        // take the keyboard somewhere else.
        press(&mut ui, Key::Down, &mut build);
        let after = focused_node(&ui).map(|node| (node.role, node.name));
        audit.check(
            after.as_ref().is_some_and(|(role, _)| matches!(role, Role::ComboBox | Role::MenuItem)) || app.borrow().size == 2,
            format!("Down on a shut select box takes the keyboard to {after:?}"),
        );
    });
}

// ---------------------------------------------------------------------------------------
// The combo, and the box that suggests.

const TOWNS: [&str; 12] = ["Aachen", "Basel", "Bergen", "Berlin", "Bern", "Bonn", "Bremen", "Brno", "Cork", "Dover", "Essen", "Graz"];

#[derive(Default)]
struct Finding {
    town: usize,
    city: String,
}

fn finding(app: &mut Finding, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Town");
    ui.combo(&mut app.town, &TOWNS);
    ui.autocomplete(&mut app.city, "City", &TOWNS);
    ui.label("after them");
    ui.close();
}

#[test]
fn a_combo_and_a_suggesting_box_are_used_with_keys_and_told() {
    Audit::new("the combo and the suggesting box").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Finding::default()), sized(sizing));
        let mut build = |ui: &mut Ui| finding(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went: Vec<Role> = tab_round(audit, &mut ui, &mut build, false, 4, &plain).into_iter().map(|node| node.role).collect();
        audit.check(went == [Role::ComboBox, Role::TextBox], format!("Tab goes {went:?}"));

        // The combo: Enter opens it with the keyboard in the box to type in; typing leaves
        // what holds it, Down lights one and a screen reader hears which, Enter selects it.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        audit.check(
            focused_node(&ui).is_some_and(|node| node.role == Role::TextBox && !node.name.is_empty()),
            format!("open, the keyboard is in a box to type in, with a name: {:?}", focused_node(&ui)),
        );
        looked_over(audit, &ui, sizing, &[]);
        typed(&mut ui, "br", &mut build);
        let left: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::MenuItem).map(|node| node.name.clone()).collect();
        audit.check(left == ["Bremen", "Brno"], format!("typing leaves what begins with it: {left:?}"));
        press(&mut ui, Key::Down, &mut build);
        let heard = ui.announced().to_vec();
        audit.check(heard == ["Brno"], format!("the option lit by Down is said: {heard:?}"));
        let lit = find_node_as(&ui, Role::MenuItem, "Brno");
        audit.check(lit.is_some_and(|node| node.on == Some(true) || node.focused), "the option that is lit is not told as such (only said once, as news)");
        press(&mut ui, Key::Enter, &mut build);
        let back = focused_node(&ui);
        audit.check(
            app.borrow().town == 7 && back.as_ref().is_some_and(|node| node.role == Role::ComboBox && node.value == "Brno"),
            format!("Enter picks it, and the keyboard is back on the combo: {}", brief(&back)),
        );
        press(&mut ui, Key::Enter, &mut build);
        press(&mut ui, Key::Escape, &mut build);
        audit.check(focused_node(&ui).is_some_and(|node| node.role == Role::ComboBox && node.on == Some(false)), "Escape shuts it, and the keyboard is back on it");

        // The box that suggests: typing lists, Down lights and is said, Enter takes it.
        press(&mut ui, Key::Tab, &mut build);
        typed(&mut ui, "ber", &mut build);
        let listed: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::MenuItem).map(|node| node.name.clone()).collect();
        audit.check(listed == ["Bergen", "Berlin", "Bern"], format!("typing lists: {listed:?}"));
        let city = focused_node(&ui);
        audit.check(city.as_ref().is_some_and(|node| node.on == Some(true)), format!("the box is not told that a list is open under it (ARIA: a combobox, expanded): {}", brief(&city)));
        press(&mut ui, Key::Down, &mut build);
        press(&mut ui, Key::Down, &mut build);
        let heard = ui.announced().to_vec();
        audit.check(heard == ["Berlin"], format!("the one lit is said: {heard:?}"));
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().city == "Berlin" && focused_name(&ui) == "City", format!("Enter takes it: {:?}, the keyboard on {:?}", app.borrow().city, focused_name(&ui)));
        typed(&mut ui, "x", &mut build);
        key(&mut ui, Key::Backspace, Mods::NONE, &mut build);
        press(&mut ui, Key::Escape, &mut build);
        audit.check(!ui.visible_access_nodes().iter().any(|node| node.role == Role::MenuItem) && focused_name(&ui) == "City", "Escape puts the list away, the keyboard stays");
        press(&mut ui, Key::Tab, &mut build);
        audit.check(focused_name(&ui) != "City", "Tab leaves the box");
    });
}

// ---------------------------------------------------------------------------------------
// The command palette.

const COMMANDS: [&str; 3] = ["Open a file\tCtrl+O", "Save\tCtrl+S", "Close\tCtrl+W"];

#[derive(Default)]
struct Commanding {
    finding: bool,
    ran: Option<usize>,
}

fn commanding(app: &mut Commanding, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.button("Before");
    ui.button("Run");
    ui.close();
    app.finding |= ui.key(Key::Char('k'), Mods::CTRL);
    if let Some(command) = ui.command_palette(&mut app.finding, &COMMANDS) {
        app.ran = Some(command);
    }
}

#[test]
fn a_command_is_found_and_run_with_keys_and_told() {
    Audit::new("the command palette").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Commanding::default()), sized(sizing));
        let mut build = |ui: &mut Ui| commanding(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        key(&mut ui, Key::Char('k'), Mods::CTRL, &mut build);
        let dialog = ui.visible_access_nodes().iter().find(|node| node.role == Role::Dialog).cloned();
        audit.check(dialog.as_ref().is_some_and(|node| !node.name.is_empty()), format!("the palette is told as a dialog with a name: {}", brief(&dialog)));
        audit.check(focused_node(&ui).is_some_and(|node| node.role == Role::TextBox), "the keyboard is in its box at once");
        audit.check(find_node(&ui, "Run").is_none(), "what is under it is not told meanwhile");
        looked_over(audit, &ui, sizing, &[]);
        let save = find_node_as(&ui, Role::MenuItem, "Save");
        audit.check(save.as_ref().is_some_and(|node| node.shortcut == "Ctrl+S" && node.position == Some([2, 3])), format!("a command is told with its keys and its place: {}", brief(&save)));
        // Tab stays in it (it is a dialog); Escape puts it away, the keyboard back on Run.
        for _ in 0..4 {
            press(&mut ui, Key::Tab, &mut build);
        }
        audit.check(ui.visible_access_nodes().iter().any(|node| node.role == Role::Dialog) && focused_node(&ui).is_some(), "Tab stays in the palette");
        press(&mut ui, Key::Escape, &mut build);
        audit.check(!app.borrow().finding && focused_name(&ui) == "Run", format!("Escape puts it away, and the keyboard is back on {:?}", focused_name(&ui)));
        // Found by typing, run with Enter.
        key(&mut ui, Key::Char('k'), Mods::CTRL, &mut build);
        typed(&mut ui, "sav", &mut build);
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().ran == Some(1) && focused_name(&ui) == "Run", format!("typing and Enter run Save: {:?}, the keyboard on {:?}", app.borrow().ran, focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// The colour picker.

fn colouring(tint: &mut Color, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Colour");
    ui.color_picker(tint);
    ui.label("after it");
    ui.close();
}

#[test]
fn a_colour_is_picked_with_keys_and_told() {
    Audit::new("the colour picker").at_every_size(|audit, sizing| {
        let (tint, mut ui) = (RefCell::new(Color::rgb(0x3366cc)), sized(sizing));
        let mut build = |ui: &mut Ui| colouring(&mut tint.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went = tab_round(audit, &mut ui, &mut build, false, 4, &plain);
        let swatch = went.first().cloned();
        audit.check(swatch.as_ref().is_some_and(|node| node.name.contains("3366cc") || node.value.contains("3366cc")), format!("the swatch is not told what colour it is: {}", brief(&swatch)));
        audit.check(swatch.as_ref().is_some_and(|node| node.on == Some(false)), "the swatch is not told that it opens a picker, nor whether it is open");
        // Enter opens the picker, with the keyboard in it.
        press(&mut ui, Key::Enter, &mut build);
        audit.check(focused_node(&ui).is_some(), format!("as the picker opens, the keyboard is on {:?}", focused_name(&ui)));
        looked_over(audit, &ui, sizing, &[]);
        let stops: Vec<(Role, String)> = tab_round(audit, &mut ui, &mut build, false, 8, &[]).into_iter().map(|node| (node.role, node.value)).collect();
        audit.check(stops.len() >= 3, format!("of the square, the hues, how solid and the written colour, the keyboard reaches only {stops:?}: the colour cannot be stepped with the arrows"));
        // The colour written out is typed over.
        while focused_node(&ui).is_some_and(|node| node.role != Role::TextBox) {
            press(&mut ui, Key::Tab, &mut build);
        }
        key(&mut ui, Key::Char('a'), Mods::CTRL, &mut build);
        typed(&mut ui, "#ff0000", &mut build);
        audit.check(*tint.borrow() == Color::rgb(0xff0000), format!("the colour typed is the colour: {:?}", tint.borrow()));
        press(&mut ui, Key::Escape, &mut build);
        audit.check(
            focused_node(&ui).is_some_and(|node| node.rect.w > 0.0) && !ui.visible_access_nodes().iter().any(|node| node.role == Role::TextBox),
            format!("Escape shuts it, the keyboard back on the swatch: {:?}", focused_node(&ui)),
        );
    });
}

// ---------------------------------------------------------------------------------------
// The date box and its calendar.

fn dating(day: &mut Date, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.label("Day");
    ui.date(day);
    ui.close();
}

#[test]
fn a_date_is_picked_in_the_calendar_with_keys_and_told() {
    Audit::new("the date box and its calendar").at_every_size(|audit, sizing| {
        let (day, mut ui) = (RefCell::new(Date::new(2026, 11, 13)), sized(sizing));
        let mut build = |ui: &mut Ui| dating(&mut day.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        press(&mut ui, Key::Tab, &mut build);
        let shut = focused_node(&ui);
        audit.check(shut.as_ref().is_some_and(|node| node.name.contains("2026") && node.name.contains("13")), format!("the date box is told its date: {}", brief(&shut)));
        audit.check(shut.as_ref().is_some_and(|node| node.on == Some(false)), "the date box is not told that it opens a calendar, nor whether it is open");
        // Enter opens the calendar with the keyboard on the date that is selected (ARIA's
        // date picker dialog).
        press(&mut ui, Key::Enter, &mut build);
        audit.check(focused_name(&ui).contains("13"), format!("as the calendar opens the keyboard is on {:?}, not on the day that is picked", focused_name(&ui)));
        looked_over(audit, &ui, sizing, &[]);
        let days: Vec<AccessNode> = ui.visible_access_nodes().iter().filter(|node| node.name.parse::<u8>().is_ok() || node.name.contains("November")).cloned().collect();
        let thirteenth = days.iter().find(|node| node.name.starts_with("13") || node.name.contains(" 13")).cloned();
        audit.check(
            thirteenth.as_ref().is_some_and(|node| node.name.contains("November") || node.name.contains("2026")),
            format!("a day is told by its number alone, with no month or year: {:?}", thirteenth.as_ref().map(|node| node.name.clone())),
        );
        audit.check(thirteenth.as_ref().is_some_and(|node| node.on == Some(true)), format!("the day that is picked is not told as picked: {:?}", thirteenth.as_ref().map(|node| (node.role, node.on))));
        // Get to the 13th with the arrows, from wherever the keyboard is.
        for _ in 0..40 {
            if focused_name(&ui) == "13" || focused_name(&ui).starts_with("13 ") {
                break;
            }
            press(&mut ui, Key::Right, &mut build);
        }
        let picked_at = focused_node(&ui).map(|node| node.rect);
        press(&mut ui, Key::Right, &mut build);
        audit.check(focused_name(&ui).starts_with("14"), format!("Right goes to the next day: {:?}", focused_name(&ui)));
        let elsewhere = ui.screen.pixels.clone();
        press(&mut ui, Key::Left, &mut build);
        if let Some(at) = picked_at {
            // (The 13th, with the keyboard on it, against the 13th with the keyboard on the 14th.)
            audit.check(stands_out(&elsewhere, &ui, at) || !ui.focus_visible(), "on the day that is picked the keyboard's place cannot be seen (its ring is the colour of its fill)");
        }
        press(&mut ui, Key::Down, &mut build);
        audit.check(focused_name(&ui).starts_with("20"), format!("Down goes a week on: {:?}", focused_name(&ui)));
        for _ in 0..3 {
            press(&mut ui, Key::Up, &mut build);
        }
        audit.check(focused_name(&ui).contains("30") || focused_name(&ui).contains("Oct"), format!("Up from the 6th goes back into October (ARIA), not to {:?}", focused_name(&ui)));
        // (From the 30th of October, where Up went, the next month is November.)
        let before = focused_name(&ui);
        press(&mut ui, Key::PageDown, &mut build);
        let after = focused_name(&ui);
        let month = before.contains("October") && after.contains("November") || before.contains("November") && after.contains("December");
        audit.check(month, format!("Page Down does not show the next month: from {before:?} to {after:?}"));
        press(&mut ui, Key::Escape, &mut build);
        audit.check(focused_node(&ui).is_some_and(|node| node.name.contains("2026")), format!("Escape shuts the calendar, the keyboard back on the date box: {:?}", focused_name(&ui)));
        // Selecting a day with Enter: the date box opens, the arrows go, Enter selects.
        press(&mut ui, Key::Enter, &mut build);
        for _ in 0..40 {
            if focused_name(&ui).starts_with("14") {
                break;
            }
            press(&mut ui, Key::Right, &mut build);
        }
        press(&mut ui, Key::Enter, &mut build);
        audit.check(*day.borrow() == Date::new(2026, 11, 14), format!("Enter picks the day the keyboard is on: {}", day.borrow()));
        audit.check(focused_node(&ui).is_some_and(|node| node.name.contains("2026")), format!("after a pick the keyboard is back on the date box: {:?}", focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// The file dialog.

struct Filing {
    files: FileDialog,
    folder: std::path::PathBuf,
    got: Option<std::path::PathBuf>,
}

fn filing(app: &mut Filing, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    if ui.button("Open a file").clicked {
        app.files.pick(&app.folder);
    }
    ui.button("Something else");
    ui.close();
    app.got = ui.file_dialog(&mut app.files).or(app.got.take());
}

#[test]
fn a_file_is_found_in_the_dialog_with_keys_and_told() {
    let folder = std::env::temp_dir().join(format!("taut-reach-files-{}", std::process::id()));
    std::fs::create_dir_all(folder.join("inner")).unwrap();
    std::fs::write(folder.join("notes.txt"), "hello").unwrap();
    Audit::new("the file dialog").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Filing { files: FileDialog::default(), folder: folder.clone(), got: None }), sized(sizing));
        let mut build = |ui: &mut Ui| filing(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        let dialog = ui.visible_access_nodes().iter().find(|node| node.role == Role::Dialog).cloned();
        audit.check(dialog.as_ref().is_some_and(|node| !node.name.is_empty()), format!("the dialog is told with a name: {}", brief(&dialog)));
        audit.check(focused_node(&ui).is_some_and(|node| node.role == Role::TextBox), format!("the keyboard is in the box for the name at once: {}", brief(&focused_node(&ui))));
        looked_over(audit, &ui, sizing, &[]);
        // Tab goes round what is in the dialog, and never out of it.
        let went = tab_round(audit, &mut ui, &mut build, false, 30, &[]);
        audit.check(!went.iter().any(|node| node.name == "Open a file" || node.name == "Something else"), "Tab leaves the dialog for what is under it");
        audit.check(went.len() >= 6, format!("Tab goes round only {:?}", went.iter().map(|node| node.name.clone()).collect::<Vec<_>>()));
        // A folder is reported as a folder, a file as a file (the picture beside the name is
        // the only thing that says so).
        let rows: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::TableRow).map(|node| node.name.clone()).collect();
        let folder_row = rows.iter().find(|name| name.starts_with("inner")).cloned().unwrap_or_default();
        audit.check(folder_row.to_lowercase().contains("folder"), format!("a folder is told as {folder_row:?}: nothing says it is a folder (rows: {rows:?})"));
        // The rows by the arrows, and Enter goes into the folder.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        while focused_node(&ui).is_some_and(|node| node.role != Role::TableRow) {
            press(&mut ui, Key::Tab, &mut build);
        }
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().files.folder == folder.join("inner"), format!("Enter on the first row, the folder, goes into it: {:?}", app.borrow().files.folder));
        // Escape closes it with no answer, and the keyboard is back where it was.
        press(&mut ui, Key::Escape, &mut build);
        audit.check(!app.borrow().files.shown && app.borrow().got.is_none(), "Escape closes the dialog with no answer");
        audit.check(focused_name(&ui) == "Open a file", format!("after the dialog the keyboard is on {:?}, not back on the button that opened it", focused_name(&ui)));
    });
    std::fs::remove_dir_all(folder).unwrap();
}

// ---------------------------------------------------------------------------------------
// Menus, and the bar of menus.

#[derive(Default)]
struct Barred {
    opened: u32,
    recent: &'static str,
    wrap: bool,
}

fn barred(app: &mut Barred, ui: &mut Ui) {
    ui.menu_bar();
    if ui.menu_button("File") {
        app.opened += ui.menu_item("Open\tCtrl+O").clicked as u32;
        if ui.submenu("Recent") {
            for name in ["one.txt", "two.txt"] {
                if ui.menu_item(name).clicked {
                    app.recent = name;
                }
            }
            ui.close();
        }
        ui.menu_checkbox(&mut app.wrap, "Wrap lines");
        ui.close();
    }
    if ui.menu_button("Edit") {
        ui.menu_item("Undo\tCtrl+Z");
        ui.menu_item("Redo\tCtrl+Y");
        ui.close();
    }
    ui.close();
    ui.label("The page");
}

#[test]
fn menus_and_the_bar_of_menus_are_used_with_keys_and_told() {
    Audit::new("menus and the bar of menus").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Barred::default()), sized(sizing));
        let mut build = |ui: &mut Ui| barred(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        audit.check(ui.visible_access_nodes().first().is_some_and(|node| node.role == Role::MenuBar), "the bar is told as a bar of menus");
        let plain = ui.screen.pixels.clone();
        let names: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 6, &plain).into_iter().map(|node| node.name).collect();
        audit.check(names == ["File", "Edit"], format!("Tab goes {names:?}"));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        let file = focused_node(&ui);
        audit.check(file.as_ref().is_some_and(|node| node.on == Some(false)), format!("the menu's button is told as shut: {}", brief(&file)));

        // Enter opens the menu, with the keyboard on its first row (ARIA: Enter, Space and
        // Down open a menu and go to its first item).
        press(&mut ui, Key::Enter, &mut build);
        audit.check(find_node_as(&ui, Role::Fold, "File").is_some_and(|node| node.on == Some(true)), "open, the menu's button is told as open");
        audit.check(focused_name(&ui) == "Open", format!("as the menu opens, the keyboard is on {:?}, not on its first row", focused_name(&ui)));
        looked_over(audit, &ui, sizing, &[]);
        if focused_name(&ui) != "Open" {
            press(&mut ui, Key::Down, &mut build);
        }
        let open = focused_node(&ui);
        audit.check(open.as_ref().is_some_and(|node| node.name == "Open" && node.shortcut == "Ctrl+O"), format!("a row is told by its name, with its keys apart: {}", brief(&open)));
        audit.check(open.as_ref().is_some_and(|node| node.position == Some([1, 3])), format!("a row of a menu is not told which of how many it is: {}", brief(&open)));
        // The row that opens another menu says so, and Right opens it with the keyboard
        // on its first row; Left goes back.
        press(&mut ui, Key::Down, &mut build);
        let recent = focused_node(&ui);
        audit.check(
            recent.as_ref().is_some_and(|node| node.name == "Recent" && node.on == Some(false)),
            format!("the row that opens a menu is not told that it does, nor whether it is open: {}", brief(&recent)),
        );
        press(&mut ui, Key::Right, &mut build);
        audit.check(find_node(&ui, "one.txt").is_some(), "Right opens the menu out of the row");
        audit.check(focused_name(&ui) == "one.txt", format!("as the inner menu opens, the keyboard is on {:?}, not on its first row", focused_name(&ui)));
        if focused_name(&ui) != "one.txt" {
            press(&mut ui, Key::Down, &mut build);
        }
        press(&mut ui, Key::Left, &mut build);
        audit.check(focused_name(&ui) == "Recent" && find_node(&ui, "one.txt").is_none(), format!("Left shuts the inner menu and the keyboard is back on {:?}", focused_name(&ui)));
        press(&mut ui, Key::Right, &mut build);
        if focused_name(&ui) != "one.txt" {
            press(&mut ui, Key::Down, &mut build);
        }
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().recent == "one.txt" && focused_name(&ui) == "File", format!("Enter picks the row, every menu shuts and the keyboard is back on {:?}", focused_name(&ui)));
        // Right in a menu of the bar opens the next menu, with the keyboard on its first row.
        press(&mut ui, Key::Enter, &mut build);
        if focused_name(&ui) != "Open" {
            press(&mut ui, Key::Down, &mut build);
        }
        press(&mut ui, Key::Right, &mut build);
        audit.check(find_node_as(&ui, Role::Fold, "Edit").is_some_and(|node| node.on == Some(true)), "Right goes on to the next menu of the bar");
        audit.check(focused_name(&ui) == "Undo", format!("in the next menu the keyboard is on {:?}, not on its first row", focused_name(&ui)));
        if focused_name(&ui) != "Undo" {
            press(&mut ui, Key::Down, &mut build);
        }
        press(&mut ui, Key::End, &mut build);
        audit.check(focused_name(&ui) == "Redo", format!("End goes to the last row (ARIA): the keyboard is on {:?}", focused_name(&ui)));
        press(&mut ui, Key::Escape, &mut build);
        audit.check(
            !ui.visible_access_nodes().iter().any(|node| node.role == Role::Menu) && focused_name(&ui) == "Edit",
            format!("after Right went on to the menu of Edit, Escape hands the keyboard back to {:?}, not to Edit", focused_name(&ui)),
        );
        // A row that ticks: reported as ticked or not, and flipped by Enter.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        for _ in 0..4 {
            if focused_name(&ui) == "Wrap lines" {
                break;
            }
            press(&mut ui, Key::Down, &mut build);
        }
        let wrap = focused_node(&ui);
        audit.check(wrap.as_ref().is_some_and(|node| node.on == Some(false)), format!("a row that ticks is told as not ticked: {}", brief(&wrap)));
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().wrap, "Enter ticks it");
    });
}

// ---------------------------------------------------------------------------------------
// Menus at the pointer, which have to open from the keyboard too.

fn pointed(renamed: &mut u32, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    let row = ui.selectable(false, "report.txt");
    if ui.context_menu(&row) {
        *renamed += ui.menu_item("Rename").clicked as u32;
        ui.menu_item("Delete");
        ui.close();
    }
    ui.close();
}

#[test]
fn a_context_menu_opens_from_the_keyboard() {
    Audit::new("context menus").at_every_size(|audit, sizing| {
        let (renamed, mut ui) = (RefCell::new(0), sized(sizing));
        let mut build = |ui: &mut Ui| pointed(&mut renamed.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        // Shift and F10 (and the key for a menu, which taut's keys do not have) open the
        // menu of what the keyboard is on, everywhere.
        key(&mut ui, Key::F(10), Mods::SHIFT, &mut build);
        let menu = ui.visible_access_nodes().iter().any(|node| node.role == Role::Menu);
        audit.check(menu, "Shift and F10 do not open the menu at the pointer: what is in it cannot be reached without a pointer");
        if menu {
            if focused_name(&ui) != "Rename" {
                press(&mut ui, Key::Down, &mut build);
            }
            press(&mut ui, Key::Enter, &mut build);
            audit.check(*renamed.borrow() == 1, "a row of it is picked with the keys");
        }
    });
}

// ---------------------------------------------------------------------------------------
// Tabs: a row of them, choices side by side, and tabs that are closed and moved.

struct Paging {
    page: usize,
    span: usize,
    paper: usize,
    papers: Vec<String>,
}

fn paging(app: &mut Paging, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.tabs(&mut app.page, &["One", "Two", "Three"]);
    ui.segmented(&mut app.span, &["Day", "Week"]);
    ui.tabs_of(&mut app.paper, &mut app.papers, |paper| paper.as_str());
    ui.label("after the tabs");
    ui.close();
}

#[test]
fn tabs_are_picked_closed_and_moved_with_keys_and_told() {
    Audit::new("tabs, also closable and moved").at_every_size(|audit, sizing| {
        let papers = ["a.txt", "b.txt", "c.txt"].map(String::from).to_vec();
        let (app, mut ui) = (RefCell::new(Paging { page: 0, span: 0, paper: 0, papers }), sized(sizing));
        let mut build = |ui: &mut Ui| paging(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let holders = ui.visible_access_nodes().iter().filter(|node| node.role == Role::Tabs).count();
        audit.check(holders == 3, format!("each row of tabs is told as one: {holders} told"));
        let tabs: Vec<(String, Option<bool>, Option<[u32; 2]>)> =
            ui.visible_access_nodes().iter().filter(|node| node.role == Role::Tab).map(|node| (node.name.clone(), node.on, node.position)).collect();
        audit.check(tabs.iter().all(|(_, on, of)| on.is_some() && of.is_some()), format!("each tab is told whether it is in front and which of how many: {tabs:?}"));
        let plain = ui.screen.pixels.clone();
        let went: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 12, &plain).into_iter().map(|node| node.name).collect();
        audit.check(went.len() == 3, format!("Tab stops once in each row of tabs (ARIA: the arrows go between them): it goes {went:?}"));

        // The arrows go along a row of tabs, Enter or Space brings one to the front.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Right, &mut build);
        audit.check(focused_name(&ui) == "Two", format!("Right goes to the next tab: {:?}", focused_name(&ui)));
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().page == 1 && focused_node(&ui).is_some_and(|node| node.on == Some(true)), "Space brings it to the front, and it is told so");
        press(&mut ui, Key::End, &mut build);
        audit.check(focused_name(&ui) == "Three", format!("End goes to the last tab (ARIA): {:?}", focused_name(&ui)));

        // A tab that can be closed: its cross is a button with a name, and Delete closes the
        // tab the keyboard is on (ARIA); one is moved with the keys too.
        let cross = ui.visible_access_nodes().iter().any(|node| node.role == Role::Button && node.name.contains("a.txt"));
        audit.check(cross, "the cross that closes a tab is not told of at all");
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..12 {
            press(&mut ui, Key::Tab, &mut build);
            if focused_name(&ui) == "b.txt" {
                break;
            }
        }
        press(&mut ui, Key::Delete, &mut build);
        audit.check(app.borrow().papers.len() == 2, "Delete does not close the tab the keyboard is on, and the cross cannot be reached: a tab cannot be closed without a pointer");
        let before = app.borrow().papers.clone();
        key(&mut ui, Key::Right, Mods { ctrl: true, shift: true, alt: false }, &mut build);
        audit.check(app.borrow().papers != before, "no key moves a tab: it is put in another order only by carrying it with the pointer");
    });
}

// ---------------------------------------------------------------------------------------
// Folds, and trees.

#[derive(Default)]
struct Folding {
    more: bool,
    root: bool,
    branch: bool,
}

fn folding(app: &mut Folding, ui: &mut Ui) {
    ui.column().pad(8.0).gap(4.0).open();
    if ui.fold(&mut app.more, "More") {
        ui.label("what was folded away");
    }
    ui.tree(&mut app.root, "Root");
    if app.root {
        ui.tree(&mut app.branch, "Branch");
        if app.branch {
            ui.label("Leaf");
            ui.close();
        }
        ui.close();
    }
    ui.close();
}

#[test]
fn folds_and_trees_open_and_shut_with_keys_and_are_told() {
    Audit::new("folds and trees").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Folding::default()), sized(sizing));
        let mut build = |ui: &mut Ui| folding(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 6, &plain).into_iter().map(|node| node.name).collect();
        audit.check(went == ["More", "Root"], format!("Tab goes {went:?}"));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        audit.check(focused_node(&ui).is_some_and(|node| node.on == Some(false)), "a fold is told as shut");
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().more && focused_node(&ui).is_some_and(|node| node.on == Some(true)), "Enter opens it, and it is told as open");
        press(&mut ui, Key::Space, &mut build);
        audit.check(!app.borrow().more, "Space shuts it");
        // A tree (ARIA): Right opens a branch, Left shuts it, Down goes to the next one; and
        // each is told how deep it is, and which of how many.
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Right, &mut build);
        audit.check(app.borrow().root, format!("Right does not open a branch of a tree: the keyboard went to {:?}", focused_name(&ui)));
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        if !app.borrow().root {
            press(&mut ui, Key::Enter, &mut build);
        }
        press(&mut ui, Key::Down, &mut build);
        let branch = focused_node(&ui);
        audit.check(branch.as_ref().is_some_and(|node| node.name == "Branch"), format!("Down goes to the branch under it: {}", brief(&branch)));
        // (Told one after another, each with how deep it is, as ARIA's aria-level has it: so a
        // long tree whose upper items are out of view is told the same way.)
        let level = |name: &str| ui.visible_access_nodes().iter().find(|node| node.name == name).map(|node| node.level);
        let told_deep = level("Branch") == Some(2) && level("Root") == Some(1);
        audit.check(told_deep, "a branch is not told how deep it is in the tree, nor what it is under (ARIA: a tree item's level)");
        press(&mut ui, Key::Left, &mut build);
        audit.check(focused_name(&ui) == "Root", format!("Left on a shut branch goes to the one it is under (ARIA): the keyboard went to {:?}", focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// Rows that are selected, in a long list.

#[derive(Default)]
struct Listing {
    picks: Selection,
}

fn listing(app: &mut Listing, ui: &mut Ui) {
    ui.column().grow().pad(8.0).open();
    ui.label("Items");
    for row in ui.list(500, 28.0) {
        if ui.selectable(app.picks.contains(row), &format!("Item {row}")).clicked {
            app.picks.click(row, ui.mods());
        }
    }
    ui.close();
    ui.close();
}

#[test]
fn rows_of_a_long_list_are_gone_through_and_picked_with_keys() {
    Audit::new("rows that are picked").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Listing::default()), sized(sizing));
        let mut build = |ui: &mut Ui| listing(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        press(&mut ui, Key::Tab, &mut build);
        let first = focused_node(&ui);
        audit.check(
            first.as_ref().is_some_and(|node| node.role == Role::Row && node.on == Some(false) && node.position == Some([1, 500])),
            format!("a row is told as one of 500, and whether it is picked: {}", brief(&first)),
        );
        for _ in 0..20 {
            press(&mut ui, Key::Down, &mut build);
        }
        let twentieth = focused_node(&ui);
        audit.check(twentieth.as_ref().is_some_and(|node| node.name == "Item 20" && node.position == Some([21, 500])), format!("Down goes down the rows, scrolling: {}", brief(&twentieth)));
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().picks.iter().collect::<Vec<_>>() == [20], "Space picks the row");
        audit.check(focused_node(&ui).is_some_and(|node| node.on == Some(true)), "a picked row is told as picked");
        // More than one (ARIA's list box of many): Shift and an arrow pick a stretch,
        // Control and Space one more.
        key(&mut ui, Key::Down, Mods::SHIFT, &mut build);
        key(&mut ui, Key::Down, Mods::SHIFT, &mut build);
        audit.check(app.borrow().picks.len() == 3, format!("Shift and Down do not pick a stretch: picked {:?}", app.borrow().picks.iter().collect::<Vec<_>>()));
        press(&mut ui, Key::Down, &mut build);
        key(&mut ui, Key::Space, Mods::CTRL, &mut build);
        audit.check(app.borrow().picks.len() >= 2, format!("Control and Space do not pick one more: picked {:?}", app.borrow().picks.iter().collect::<Vec<_>>()));
        // Page Down goes a page on, and End to the last (ARIA), and the keyboard stays
        // on a row that shows.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::PageDown, &mut build);
        audit.check(
            focused_node(&ui).is_some_and(|node| node.role == Role::Row) && ui.focus() != 0,
            format!("Page Down scrolls the list and the keyboard is dropped: it is on {} (focus {})", brief(&focused_node(&ui)), ui.focus()),
        );
        press(&mut ui, Key::End, &mut build);
        audit.check(focused_name(&ui) == "Item 499", format!("End scrolls to the end and the keyboard is dropped: it is on {:?}", focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// Tables: headings that sort, a long list of rows, a row kept in view, columns hidden and
// moved.

const COLUMNS: [Column; 3] = [Column::new("Name"), Column::new("Size").px(80.0), Column::new("Kind").px(80.0).unsorted()];

#[derive(Default)]
struct Tabled {
    table: Table,
    picks: Selection,
}

fn tabled(app: &mut Tabled, ui: &mut Ui) {
    ui.table(&mut app.table, &COLUMNS);
    ui.table_row().frozen().open();
    ui.label("All");
    ui.label("999");
    ui.label("");
    ui.close();
    for row in ui.rows(300, 24.0) {
        if ui.selectable_row(row, app.picks.contains(row)).clicked {
            app.picks.click(row, ui.mods());
        }
        ui.label(&format!("file {row}"));
        ui.label(&format!("{}", row * 3));
        ui.label("text");
        ui.close();
    }
    ui.close();
    ui.close();
    ui.button("After the table");
}

#[test]
fn a_table_is_sorted_gone_through_and_arranged_with_keys_and_told() {
    Audit::new("tables").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Tabled::default()), sized(sizing));
        let mut build = |ui: &mut Ui| tabled(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let table = ui.visible_access_nodes().first().cloned();
        audit.check(
            table.as_ref().is_some_and(|node| node.role == Role::Table && node.grid == Some([301, 3])),
            format!("the table is told with its rows and columns: {:?}", table.map(|node| (node.role, node.grid))),
        );
        // Tab: the headings that sort, then into the rows, and out after the table: ARIA has
        // a grid be one stop for Tab, the arrows going within it.
        let went: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 40, &[]).into_iter().map(|node| node.name).collect();
        let rows_stopped = went.iter().filter(|name| name.starts_with("file")).count();
        audit.check(rows_stopped <= 1, format!("Tab stops at every row in view ({rows_stopped} of them) on its way out of the table"));
        audit.check(went.iter().any(|name| name == "After the table"), format!("Tab gets out of the table: it goes {went:?}"));

        // Enter on a heading sorts; Shift and Enter sort by one more column.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().table.sort == [(0, false)] && focused_node(&ui).is_some_and(|node| node.sorted == Some(false)), "Enter on a heading sorts by it, and it is told so");
        press(&mut ui, Key::Tab, &mut build);
        key(&mut ui, Key::Enter, Mods::SHIFT, &mut build);
        audit.check(app.borrow().table.sort.len() == 2, format!("Shift and Enter do not sort by one more column (Shift and a click do): {:?}", app.borrow().table.sort));
        // The columns: hidden through the menu of a heading, moved, made wider.
        key(&mut ui, Key::F(10), Mods::SHIFT, &mut build);
        audit.check(ui.visible_access_nodes().iter().any(|node| node.role == Role::Menu), "the menu that hides and shows columns does not open from the keyboard");
        press(&mut ui, Key::Escape, &mut build);
        let order = app.borrow().table.to_string();
        key(&mut ui, Key::Right, Mods { ctrl: true, shift: true, alt: false }, &mut build);
        key(&mut ui, Key::Right, Mods { ctrl: false, shift: false, alt: true }, &mut build);
        audit.check(app.borrow().table.to_string() != order, "no key moves a column or makes it wider: only the pointer does");

        // The rows: Down goes down them past the ones in view, Space selects, and each says
        // which of all it is.
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..12 {
            press(&mut ui, Key::Tab, &mut build);
            if focused_row(&ui).is_some() {
                break;
            }
        }
        let first = focused_row(&ui);
        audit.check(
            first.as_ref().is_some_and(|node| node.position == Some([2, 301]) && node.on == Some(false)),
            format!("the first row is told which of all it is, and that it is not picked: {}", brief(&first)),
        );
        for _ in 0..25 {
            press(&mut ui, Key::Down, &mut build);
        }
        let deep = focused_row(&ui);
        audit.check(deep.as_ref().is_some_and(|node| node.name.starts_with("file 25")), format!("Down goes on past the rows in view: {}", brief(&deep)));
        press(&mut ui, Key::Space, &mut build);
        audit.check(app.borrow().picks.contains(25) && focused_row(&ui).is_some_and(|node| node.on == Some(true)), "Space picks the row, and it is told as picked");
        key(&mut ui, Key::Down, Mods::SHIFT, &mut build);
        audit.check(app.borrow().picks.len() == 2, "Shift and Down do not pick a stretch of rows");
        audit.check(ui.visible_access_nodes().iter().any(|node| node.name.starts_with("All, 999")), "the row kept in view is told");
    });
}

// ---------------------------------------------------------------------------------------
// Charts, and the small plots.

fn charting(ui: &mut Ui) {
    let (sold, back) = ([10.0, 40.0, 20.0], [1.0, 4.0, 2.0]);
    let rows = [Series::bars("Sold", &sold), Series::line("Back", &back)];
    ui.column().pad(8.0).gap(6.0).open();
    ui.chart(&Chart::of(&rows).categories(&["Mon", "Tue", "Wed"]), 300.0, 150.0);
    ui.plot_lines(&[1.0, 4.0, 2.0, 5.0], 200.0, 40.0);
    ui.button("After the chart");
    ui.close();
}

#[test]
fn a_chart_is_read_with_keys_and_told() {
    Audit::new("charts").at_every_size(|audit, sizing| {
        let mut ui = sized(sizing);
        let mut build = charting;
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let plain = ui.screen.pixels.clone();
        let went = tab_round(audit, &mut ui, &mut build, false, 4, &plain);
        let chart = went.first().cloned();
        audit.check(chart.as_ref().is_some_and(|node| node.name == "Sold, Back"), format!("the chart is reached by Tab and told by its rows: {}", brief(&chart)));
        // (Told as a group, which a screen reader hears the keyboard cannot be on: on
        // Windows `IsKeyboardFocusable` is false for it, and on AT-SPI it is not focusable.)
        audit.check(
            chart.as_ref().is_some_and(|node| !node.role.is_container()),
            format!("the chart is told as a {:?}, which the keyboard cannot be on, though it is", chart.as_ref().map(|node| node.role)),
        );
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        let mut heard = Vec::new();
        for key_ in [Key::Right, Key::Right, Key::End, Key::Home] {
            press(&mut ui, key_, &mut build);
            heard.extend(ui.announced().iter().cloned());
        }
        audit.check(heard == ["Mon: Sold 10, Back 1", "Tue: Sold 40, Back 4", "Wed: Sold 20, Back 2", "Mon: Sold 10, Back 1"], format!("the keys read the chart off: {heard:?}"));
        let plot = ui.visible_access_nodes().iter().any(|node| node.rect.h == 40.0 && node.rect.w == 200.0);
        audit.check(plot, "a small plot is told of not at all (it has no words, and the part takes no name)");
    });
}

// ---------------------------------------------------------------------------------------
// Docks and panels.

struct Docking {
    dock: Dock,
    tools: Panel,
    side: f32,
}

fn docking(app: &mut Docking, ui: &mut Ui) {
    ui.row().grow().open();
    ui.column().w(taut::Length::Px(app.side)).open();
    ui.label("A pane");
    ui.close();
    ui.splitter(&mut app.side, 60.0, 200.0);
    ui.dock(&mut app.dock);
    for name in ["Files", "Editor", "Notes"] {
        if ui.docked(&mut app.dock, name) {
            ui.button(&format!("in {name}"));
            ui.close();
        }
    }
    ui.close();
    ui.close();
    if ui.panel(&mut app.tools, "Tools") {
        ui.button("A tool");
        ui.close();
    }
}

/// Whether the thing is in the panel called "Tools".
fn in_tools(ui: &Window, node: &AccessNode) -> bool {
    node.parent.is_some_and(|within| ui.visible_access_nodes().get(within).is_some_and(|group| group.name == "Tools"))
}

#[test]
fn docks_and_panels_are_used_with_keys_and_told() {
    Audit::new("docks, panels and the bar between panes").at_every_size(|audit, sizing| {
        let dock = Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]), 0.4);
        let (app, mut ui) = (RefCell::new(Docking { dock, tools: Panel::new(150.0, 150.0, 200.0, 120.0), side: 80.0 }), sized(sizing));
        let mut build = |ui: &mut Ui| docking(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let panel = find_node_as(&ui, Role::Group, "Tools");
        audit.check(panel.is_some(), "a panel is told as a group by its title");
        let tabs: Vec<(String, Option<[u32; 2]>, bool)> = ui
            .visible_access_nodes()
            .iter()
            .filter(|node| node.role == Role::Tab)
            .map(|node| (node.name.clone(), node.position, node.parent.is_some_and(|within| ui.visible_access_nodes()[within].role == Role::Tabs)))
            .collect();
        audit.check(tabs.iter().all(|(_, of, within)| of.is_some() && *within), format!("the tabs of a dock are not told which of how many, nor as a row of tabs: {tabs:?}"));
        let went = tab_round(audit, &mut ui, &mut build, false, 20, &[]);
        let names: Vec<String> = went.iter().map(|node| node.name.clone()).collect();
        audit.check(went.iter().any(|node| node.role == Role::Slider), format!("the bar between two panes, and between the places of a dock, cannot be reached by keys: Tab goes {names:?}"));
        // The panel's own buttons: fold it, close it; told by name, and how it stands.
        audit.check(went.iter().any(|node| in_tools(&ui, node) && node.role == Role::Button && node.name != "A tool"), "the panel's buttons are not reached by Tab");
        ui.set_focus(0);
        settle(&mut ui, &mut build);
        for _ in 0..20 {
            press(&mut ui, Key::Tab, &mut build);
            if focused_node(&ui).is_some_and(|node| in_tools(&ui, &node)) {
                break;
            }
        }
        press(&mut ui, Key::Enter, &mut build);
        audit.check(app.borrow().tools.folded, "Enter on the first button of the panel folds it");
        audit.check(ui.visible_access_nodes().iter().any(|node| node.on == Some(false) && in_tools(&ui, node)), "a folded panel is not told as folded (its button says nothing of how it stands)");
        // Moving the panel, making it larger, putting a panel of the dock elsewhere: no key
        // does any of it.
        let at = app.borrow().tools.rect;
        for (key_, mods) in [(Key::Right, Mods { ctrl: false, shift: true, alt: true }), (Key::Down, Mods::CTRL)] {
            key(&mut ui, key_, mods, &mut build);
        }
        audit.check(app.borrow().tools.rect != at, "no key moves a panel or sizes it, nor puts a panel of a dock in another place");
    });
}

// ---------------------------------------------------------------------------------------
// Dialogs, as the guide writes one.

#[derive(Default)]
struct Asking {
    asking: bool,
    deleted: u32,
}

fn asking(app: &mut Asking, ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    app.asking |= ui.button("Delete the file").clicked;
    ui.button("Something else");
    ui.close();
    if app.asking {
        let beside = ui.dialog();
        ui.label("Delete the file?");
        let yes = ui.button("Delete").clicked;
        let no = ui.button("Keep").clicked;
        app.deleted += yes as u32;
        app.asking = !(yes || no || beside.clicked);
        ui.close();
    }
}

#[test]
fn a_dialog_takes_the_keyboard_keeps_it_and_hands_it_back() {
    Audit::new("dialogs").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Asking::default()), sized(sizing));
        let mut build = |ui: &mut Ui| asking(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        let dialog = ui.visible_access_nodes().iter().find(|node| node.role == Role::Dialog).cloned();
        audit.check(dialog.as_ref().is_some_and(|node| node.name == "Delete the file?"), format!("the dialog is told by what stands over it: {}", brief(&dialog)));
        audit.check(find_node(&ui, "Something else").is_none(), "what is under it is not told");
        looked_over(audit, &ui, sizing, &[]);
        // As it opens, the keyboard goes into it (ARIA: to its first thing).
        audit.check(focused_node(&ui).is_some(), format!("as the dialog opens the keyboard is on {:?}, not in the dialog", focused_name(&ui)));
        let went: Vec<String> = tab_round(audit, &mut ui, &mut build, false, 6, &[]).into_iter().map(|node| node.name).collect();
        audit.check(went.len() == 2 && went.iter().all(|name| name == "Delete" || name == "Keep"), format!("Tab goes round the dialog and stays in it: {went:?}"));
        // Escape is the way out of a dialog (ARIA), and the keyboard goes back to where it
        // was.
        press(&mut ui, Key::Escape, &mut build);
        audit.check(!app.borrow().asking, "Escape does not close a dialog: `ui.dialog()` gives the app no word of it, the way it does of a click beside it");
        if app.borrow().asking {
            ui.set_focus(0);
            settle(&mut ui, &mut build);
            press(&mut ui, Key::Tab, &mut build);
            press(&mut ui, Key::Tab, &mut build);
            press(&mut ui, Key::Enter, &mut build);
        }
        audit.check(!app.borrow().asking && app.borrow().deleted == 0, "Keep closes the dialog");
        audit.check(focused_name(&ui) == "Delete the file", format!("after the dialog the keyboard is on {:?}, not back on the button that opened it", focused_name(&ui)));
    });
}

// ---------------------------------------------------------------------------------------
// Notices.

fn noticing(ui: &mut Ui) {
    ui.column().pad(8.0).open();
    if ui.button("Save").clicked {
        ui.notice("Saved");
    }
    ui.close();
}

#[test]
fn a_notice_is_said_once_and_shown() {
    Audit::new("notices").at_every_size(|audit, sizing| {
        let mut ui = sized(sizing);
        let mut build = noticing;
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        audit.check(ui.announced() == ["Saved"], format!("the notice is said: {:?}", ui.announced()));
        audit.check(find_node_as(&ui, Role::Text, "Saved").is_some(), "the notice is told as words while it shows");
        audit.check(focused_name(&ui) == "Save", "the keyboard stays where it was");
        ui.run(&mut build);
        audit.check(ui.announced().is_empty(), "and it is said once only");
    });
}

// ---------------------------------------------------------------------------------------
// Second windows.

struct Seconded {
    prefs: SecondWindow,
    dark: bool,
}

fn seconded(app: &mut Seconded, ui: &mut Ui) {
    ui.button("In the first window");
    if ui.window(&mut app.prefs, "Preferences") {
        ui.column().pad(8.0).gap(6.0).open();
        ui.checkbox(&mut app.dark, "Dark");
        ui.button("Done");
        ui.close();
        ui.close();
    }
}

#[test]
fn a_second_window_is_used_with_keys_and_told() {
    Audit::new("second windows").at_every_size(|audit, sizing| {
        let (app, mut ui) = (RefCell::new(Seconded { prefs: SecondWindow::new(240.0, 160.0), dark: false }), sized(sizing));
        let mut build = |ui: &mut Ui| seconded(&mut app.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        let Some(id) = ui.windows().map(|(id, ..)| id).next() else {
            audit.check(false, "no second window");
            return;
        };
        let names = |ui: &mut Window| ui.in_window(id, |window| window.visible_access_nodes().iter().map(|node| (node.role, node.name.clone(), node.focused)).collect::<Vec<_>>()).unwrap_or_default();
        let told_there = names(&mut ui);
        audit.check(told_there.iter().any(|(role, name, _)| *role == Role::CheckBox && name == "Dark"), format!("the second window tells its own: {told_there:?}"));
        ui.in_window(id, |window| window.event(taut::Event::Key(Key::Tab, Mods::NONE)));
        ui.run(&mut build);
        let told_there = names(&mut ui);
        audit.check(told_there.iter().any(|(_, name, focused)| name == "Dark" && *focused), format!("Tab in the second window goes to its tick box: {told_there:?}"));
        audit.check(ui.in_window(id, |window| window.focus_visible()) == Some(true), "the keyboard's place is shown in the second window");
        ui.in_window(id, |window| window.event(taut::Event::Key(Key::Space, Mods::NONE)));
        ui.run(&mut build);
        audit.check(app.borrow().dark, "Space ticks it");
    });
}

// ---------------------------------------------------------------------------------------
// Links and words.

fn wording(followed: &mut u32, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().pad(8.0).gap(6.0).open();
    ui.wrapping_row().open();
    ui.words("Read the ", t.font, t.text);
    *followed += ui.link("guide").clicked as u32;
    ui.words(" before you start.", t.font, t.text);
    ui.close();
    ui.paragraph("A paragraph is one piece of text.");
    ui.close();
}

#[test]
fn words_and_links_are_read_and_followed() {
    Audit::new("links and words").at_every_size(|audit, sizing| {
        let (followed, mut ui) = (RefCell::new(0), sized(sizing));
        let mut build = |ui: &mut Ui| wording(&mut followed.borrow_mut(), ui);
        settle(&mut ui, &mut build);
        looked_over(audit, &ui, sizing, &[]);
        let texts: Vec<String> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::Text).map(|node| node.name.clone()).collect();
        audit.check(texts.len() <= 3, format!("words in a row that wraps are told one word at a time, {} pieces: {texts:?}", texts.len()));
        audit.check(find_node_as(&ui, Role::Link, "guide").is_some(), "the link is told as a link");
        press(&mut ui, Key::Tab, &mut build);
        press(&mut ui, Key::Enter, &mut build);
        audit.check(*followed.borrow() == 1 && focused_name(&ui) == "guide", "Tab reaches the link and Enter follows it");
    });
}

// ---------------------------------------------------------------------------------------
// Pictures, drawings and animations.

const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="0 0 4 4"><rect x="1" y="1" width="2" height="2" fill="#e11d48"/></svg>"##;

#[test]
fn pictures_drawings_and_films_are_told_as_the_app_names_them() {
    Audit::new("pictures, drawings and films").at_every_size(|audit, sizing| {
        let mut ui = sized(sizing);
        let picture = ui.add_picture(2, vec![0xffff_0000; 4]);
        let drawing = ui.add_drawing(SQUARE).unwrap();
        let film = ui.add_animation(include_bytes!("pictures/moving.gif")).unwrap();
        let mut build = |ui: &mut Ui| {
            let t = ui.theme;
            ui.column().pad(8.0).gap(6.0).open();
            let shown = ui.picture(picture);
            ui.tell(&shown, Role::Picture, None, "A red square");
            let drawn = ui.el().drawing(drawing, t.text).show();
            ui.tell(&drawn, Role::Picture, None, "A drawing");
            let moving = ui.el().animation(film).show();
            ui.tell(&moving, Role::Picture, None, "A film");
            let plot = ui.plot_lines(&[1.0, 3.0, 2.0], 60.0, 20.0);
            ui.tell(&plot, Role::Picture, None, "A plot");
            ui.close();
        };
        settle(&mut ui, &mut build);
        let told_as: Vec<(Role, String)> = ui.visible_access_nodes().iter().map(|node| (node.role, node.name.clone())).collect();
        audit.check(
            told_as.len() == 4,
            format!("`ui.tell` after `ui.picture`, `.drawing`, `.film` or a plot does nothing (their boxes have no id unless `.sense()` is said): a picture cannot be given a name; told: {told_as:?}"),
        );
        // (There is no kind for a picture: reported as words, a screen reader cannot say
        // "picture", as it does of an image on a page.)
        audit.check(told_as.iter().all(|(role, _)| *role != Role::Text), "a picture can only be told as words: there is no kind for a picture");
    });
}

// ---------------------------------------------------------------------------------------
// What the pointer brings up, and what shows how far something has come.

fn hinting(ui: &mut Ui) {
    ui.column().pad(8.0).gap(6.0).open();
    ui.icon_button(STAR, "Add a star");
    // (Named as the app names any part with no words of its own.)
    let bar = ui.progress(0.4);
    ui.tell(&bar, Role::Progress, None, "Copying");
    let ring = ui.spinner();
    ui.tell(&ring, Role::Progress, None, "Busy");
    ui.close();
}

#[test]
fn hints_show_for_the_keyboard_and_progress_is_named() {
    Audit::new("hints, progress and the ring that turns").at_every_size(|audit, sizing| {
        let mut ui = sized(sizing);
        let mut build = hinting;
        settle(&mut ui, &mut build);
        press(&mut ui, Key::Tab, &mut build);
        ui.set_time(std::time::Duration::from_secs(2));
        ui.run(&mut build);
        audit.check(
            find_node_as(&ui, Role::Text, "Add a star").is_some(),
            "the hint of an icon button shows only under a resting pointer, never for the keyboard: a sighted person on the keyboard does not learn what the button does",
        );
        let bars: Vec<(Option<[f32; 3]>, String)> = ui.visible_access_nodes().iter().filter(|node| node.role == Role::Progress).map(|node| (node.value_range, node.name.clone())).collect();
        audit.check(bars.first().is_some_and(|(amount, _)| *amount == Some([0.4, 0.0, 1.0])), format!("a bar is told how far it has come: {bars:?}"));
        audit.check(bars.iter().all(|(_, name)| !name.is_empty()), format!("a bar and a ring that turns are told with no name, and the parts take none: {bars:?}"));
    });
}

// ---------------------------------------------------------------------------------------
// Larger text, as an app asks for it (`text_scaled`) and as the person asks the system for it:
// the parts grow with their words.

#[test]
fn with_larger_text_and_taller_parts_nothing_is_cut_off() {
    let mut audit = Audit::new("larger text with taller parts");
    let sizing = Sizing { scale: 1.0, text: 2.5 };
    let mut ui = sized(sizing);
    ui.resize(Size { w: 420.0, h: 600.0 }, 1.0);
    ui.screen = taut::Surface::draw(&mut ui.ui, 64);
    let mut app = Presses::default();
    settle(&mut ui, |ui| presses(&mut app, ui));
    looked_over(&mut audit, &ui, sizing, &["Add a star", "Italic"]);
    let lines: Vec<String> = audit.wrong.iter().map(|(what, _)| what.clone()).collect();
    assert!(lines.is_empty(), "{lines:#?}");

    // The same, where the person asked the system for text that large.
    let mut ui = window(1.0);
    ui.set_preferences(taut::Preferences { text_scale: sizing.text, ..Default::default() });
    ui.set_screen_reader(true);
    ui.resize(Size { w: 420.0, h: 600.0 }, 1.0);
    ui.screen = taut::Surface::draw(&mut ui.ui, 64);
    let mut app = Presses::default();
    settle(&mut ui, |ui| {
        ui.theme = ui.system_theme();
        presses(&mut app, ui)
    });
    looked_over(&mut audit, &ui, sizing, &["Add a star", "Italic"]);
    let lines: Vec<String> = audit.wrong.iter().map(|(what, _)| what.clone()).collect();
    assert!(lines.is_empty(), "the system's larger text: {lines:#?}");
}

// ---------------------------------------------------------------------------------------
// Words taller than the part they are in.

/// A part is as high as `theme.control_height` whatever its words: with larger text one line of
/// them is taller than the part, and spills out above and below it. What spills out is not
/// in the part's drawn area, so when the words change colour (a toggle turned on) the
/// window is drawn again only within the part, and the spilt pixels stay as they were.
#[test]
fn words_taller_than_their_part_are_drawn_again_with_it() {
    let mut ui = window(1.0);
    ui.theme.font = Font::new(32.0);
    let mut bold = false;
    let mut build = |ui: &mut Ui| {
        ui.column().pad(20.0).open();
        ui.toggle(&mut bold, "Bold");
        ui.close();
    };
    settle(&mut ui, &mut build);
    press(&mut ui, Key::Tab, &mut build);
    press(&mut ui, Key::Space, &mut build);
}
