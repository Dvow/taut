//! Rich text and documents, through a headless window: after every step the incrementally
//! redrawn window is compared with a full fresh draw. (Built-in glyphs are 12 wide and 20 high
//! in the normal font, 14 wide in bold.)
mod common;
use common::*;
use std::time::Duration;
use taut::geom::pt;
use taut::{Event, Key, Length, Markdown, Mods, Piece, Role, Said, Ui};

/// What a screen reader is told of a window, as kinds and names.
fn told(ui: &Window) -> Vec<(Role, String)> {
    ui.said().iter().map(|said| (said.role, said.name.clone())).collect()
}

/// A window with nothing but a document in it.
fn build(doc: &Markdown) -> impl FnMut(&mut Ui) + '_ {
    move |ui: &mut Ui| _ = ui.markdown(doc)
}

fn find<'a>(ui: &'a Window, role: Role, name: &str) -> &'a Said {
    ui.said().iter().find(|said| said.role == role && said.name == name).unwrap_or_else(|| panic!("no {role:?} {name:?} in {:?}", told(ui)))
}

#[test]
fn pieces_of_rich_text_break_into_rows_together_and_its_links_are_pressed() {
    let mut w = window(1.0);
    w.listen(true);
    let (mut pressed, mut wide) = (Vec::new(), 240.0);
    let build = |ui: &mut Ui, pressed: &mut Vec<usize>, wide: f32| {
        let t = ui.theme;
        ui.column().w(Length::Px(wide)).open();
        let pieces = [Piece::words("Read the "), Piece::words("guide").link(), Piece::words(" before "), Piece::words("the rest").bold().color(t.green), Piece::words(" of it.")];
        pressed.extend(ui.rich(&pieces));
        ui.close();
    };
    settle(&mut w, |ui| build(ui, &mut pressed, wide));
    // In 240: "Read the guide" (168), then "before the rest of" (84 + 112 in bold + 36),
    // then "it.": three rows of 20. It is told as one text, and its link as a link.
    let text = find(&w, Role::Text, "Read the guide before the rest of it.").rect;
    assert_eq!((text.w, text.h), (240.0, 60.0));
    let link = find(&w, Role::Link, "guide").rect;
    assert_eq!((link.x, link.y, link.w, link.h), (108.0, 0.0, 60.0, 20.0));
    // Pressed by the pointer, and by Enter once Tab has gone to it.
    click(&mut w, middle(link), |ui| build(ui, &mut pressed, wide));
    assert_eq!(pressed, [1]);
    click(&mut w, pt(400.0, 280.0), |ui| build(ui, &mut pressed, wide));
    key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &mut pressed, wide));
    assert!(find(&w, Role::Link, "guide").focused);
    key(&mut w, Key::Enter, Mods::NONE, |ui| build(ui, &mut pressed, wide));
    assert_eq!(pressed, [1, 1]);
    // Narrower, it has more rows; wider, fewer. (The run after the change is the one
    // that knows the new width: it describes the paragraph once more, by itself.)
    for (room, rows) in [(120.0, 5.0), (420.0, 2.0)] {
        wide = room;
        w.run(&mut |ui| build(ui, &mut pressed, wide));
        assert_eq!(find(&w, Role::Text, "Read the guide before the rest of it.").rect.h, rows * 20.0, "in {room}");
        assert!(w.passes <= 2);
    }
}

#[test]
fn icons_and_pictures_stand_on_the_line_among_the_words() {
    let mut w = window(1.0);
    w.listen(true);
    let picture = w.add_picture(2, vec![0xffff_0000; 4]);
    let build = |ui: &mut Ui| {
        let t = ui.theme;
        let star = taut::Icon::solid("M8 1 10 6h5l-4 3 2 6-5-4-5 4 2-6-4-3h5z");
        ui.rich(&[Piece::words("A star "), Piece::icon(star), Piece::words(" and a picture "), Piece::picture(picture, 30.0, 30.0), Piece::words(" in "), Piece::words("code").code().color(t.amber)]);
    };
    settle(&mut w, build);
    // One row of 420: the picture, 30 high over the line the letters stand on, makes it
    // 30 + 4 under the line. Its pixels are there, and the word of code is on its ground.
    let text = find(&w, Role::Text, "A star  and a picture  in code").rect;
    assert_eq!(text.h, 34.0);
    let red = w.screen.pixels.iter().filter(|pixel| **pixel & 0xffffff == 0xff0000).count();
    assert_eq!(red, 30 * 30);
}

/// A document of every kind of block.
const EVERY_BLOCK: &str = "# Title

A paragraph with a [link](https://example.com) in it.

- one
- two
  1. inner

> quoted

```rust
fn main() {}
```

| a | b |
|---|---|
| 1 | 2 |
";

#[test]
fn a_document_is_told_as_a_document() {
    let mut w = window(1.0);
    w.listen(true);
    let doc = Markdown::read(EVERY_BLOCK);
    let build = |ui: &mut Ui| {
        ui.column().grow().scroll().open();
        ui.markdown(&doc);
        ui.close();
    };
    settle(&mut w, build);
    let told = told(&w);
    let words = |role: Role, name: &str| (role, name.to_string());
    let expected = [
        words(Role::Document, "Title"),
        words(Role::Title, "Title"),
        words(Role::Paragraph, "A paragraph with a link in it."),
        words(Role::List, ""),
        words(Role::Item, ""),
        words(Role::Paragraph, "one"),
        words(Role::Item, ""),
        words(Role::Paragraph, "two"),
        words(Role::List, ""),
        words(Role::Item, ""),
        words(Role::Paragraph, "inner"),
        words(Role::Quote, ""),
        words(Role::Paragraph, "quoted"),
        words(Role::Code, "fn main() {}"),
        words(Role::Paragraph, "a"),
        words(Role::Paragraph, "b"),
        words(Role::Paragraph, "1"),
        words(Role::Paragraph, "2"),
        words(Role::Link, "link"),
    ];
    assert_eq!(told, expected);
    // The heading says its level; each item which of how many it is; and everything is in
    // the document.
    let said = w.said();
    assert_eq!(said.iter().find(|said| said.role == Role::Title).map(|said| said.level), Some(1));
    let items: Vec<_> = said.iter().filter(|said| said.role == Role::Item).map(|said| said.of).collect();
    assert_eq!(items, [Some([1, 2]), Some([2, 2]), Some([1, 1])]);
    assert!(said.iter().skip(1).all(|said| said.within.is_some()));
}

#[test]
fn a_document_is_selected_by_the_pointer_and_the_keys_and_copied() {
    let mut w = window(1.0);
    let doc = Markdown::read("Hello brave new world\n\nA second *block* of words.\n");
    let build = |ui: &mut Ui| _ = ui.markdown(&doc);
    settle(&mut w, build);
    let copied = |w: &mut Window| {
        key(w, Key::Char('c'), Mods::CTRL, build);
        w.clipboard().unwrap_or_default()
    };
    // Dragged over the first word (each letter 12 wide): it is selected, and copied.
    drag(&mut w, pt(2.0, 10.0), pt(62.0, 10.0), build);
    assert_eq!(copied(&mut w), "Hello");
    // Two presses in a row take a word; three, the block.
    w.set_time(Duration::from_millis(100));
    click(&mut w, pt(100.0, 10.0), build);
    w.set_time(Duration::from_millis(200));
    click(&mut w, pt(100.0, 10.0), build);
    assert_eq!(copied(&mut w), "brave");
    w.set_time(Duration::from_millis(300));
    click(&mut w, pt(100.0, 10.0), build);
    assert_eq!(copied(&mut w), "Hello brave new world");
    // Shift with an arrow takes the selection on from where it was taken to; down a row
    // takes it into the next block (31 below: a row of 20 and the room between blocks).
    w.set_time(Duration::from_secs(2));
    click(&mut w, pt(2.0, 10.0), build);
    for _ in 0..3 {
        key(&mut w, Key::Right, Mods::SHIFT, build);
    }
    assert_eq!(copied(&mut w), "Hel");
    key(&mut w, Key::Down, Mods::SHIFT, build);
    assert_eq!(copied(&mut w), "Hello brave new world\nA s");
    key(&mut w, Key::End, Mods::SHIFT, build);
    assert_eq!(copied(&mut w), "Hello brave new world\nA second block of words.");
    // Control+A takes all of it.
    key(&mut w, Key::Char('a'), Mods::CTRL, build);
    assert_eq!(copied(&mut w), "Hello brave new world\nA second block of words.");
    // A drag that starts in one block and ends in another, the second row of the window.
    drag(&mut w, pt(74.0, 10.0), pt(26.0, 41.0), build);
    assert_eq!(copied(&mut w), "brave new world\nA ");
}

#[test]
fn links_in_a_document_are_reached_by_tab_and_followed() {
    let mut w = window(1.0);
    w.listen(true);
    let filler: String = (0..30).map(|n| format!("Paragraph {n}.\n\n")).collect();
    let doc = Markdown::read(&format!("Go [down](#far-away) or [out](https://example.com/out).\n\n{filler}## Far away\n\nThe [end](https://example.com/end).\n\n{filler}"));
    let mut followed = Vec::new();
    let build = |ui: &mut Ui, followed: &mut Vec<String>| {
        ui.column().grow().scroll().open();
        followed.extend(ui.markdown(&doc).map(str::to_string));
        ui.close();
    };
    settle(&mut w, |ui| build(ui, &mut followed));
    assert!(!told(&w).contains(&(Role::Title, "Far away".into())));
    // Tab goes to the document, then to each link in turn, also to one out of view, which
    // the document is scrolled to; Enter follows it.
    for _ in 0..4 {
        key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &mut followed));
    }
    settle(&mut w, |ui| build(ui, &mut followed));
    assert!(find(&w, Role::Link, "end").focused);
    key(&mut w, Key::Enter, Mods::NONE, |ui| build(ui, &mut followed));
    assert_eq!(followed, ["https://example.com/end"]);
    // Back at the top, a link to a heading of its own scrolls the document to it; the
    // app hears nothing of it.
    does(&mut w, Event::Move(pt(200.0, 150.0)), |ui| build(ui, &mut followed));
    does(&mut w, Event::Wheel(0.0, -10_000.0), |ui| build(ui, &mut followed));
    settle(&mut w, |ui| build(ui, &mut followed));
    let down = middle(find(&w, Role::Link, "down").rect);
    click(&mut w, down, |ui| build(ui, &mut followed));
    settle(&mut w, |ui| build(ui, &mut followed));
    assert_eq!(followed.len(), 1);
    assert!(find(&w, Role::Title, "Far away").rect.y < 40.0);
}

#[test]
fn code_is_coloured_by_its_language_and_pictures_are_given_by_name() {
    let mut w = window(1.0);
    let mut doc = Markdown::read("```rust\nfn main() {}\n```\n\n![a red square](red.png)\n");
    settle(&mut w, build(&doc));
    // "fn" is a word Rust keeps: in the colour that stands out.
    let accent = w.theme.accent;
    assert!(w.screen.pixels.contains(&accent.0));
    // The picture's name, until the app gives a picture for it: then the picture.
    assert_eq!(doc.pictures(), ["red.png"]);
    let red = w.add_picture(1, vec![0xffff_0000]);
    doc.set_picture("red.png", red);
    w.run(&mut build(&doc));
    w.run(&mut build(&doc));
    assert_eq!(w.screen.pixels.iter().filter(|pixel| **pixel == 0xffff_0000).count(), 1);
}
