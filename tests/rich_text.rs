//! Rich text and documents, through a headless window: after every step the incrementally
//! redrawn window is compared with a full fresh draw. (Built-in glyphs are 12 wide and 20 high
//! in the normal font, 14 wide in bold.)
mod common;
use common::*;
use std::time::Duration;
use taut::geom::pt;
use taut::{Event, Key, Length, Markdown, Mods, Role, Span, Ui};

/// What a screen reader hears of a window, as kinds and names.
fn roles_and_names(ui: &Window) -> Vec<(Role, String)> {
    ui.visible_access_nodes().iter().map(|node| (node.role, node.name.clone())).collect()
}

/// A window with nothing but a document in it.
fn build(doc: &Markdown) -> impl FnMut(&mut Ui) + '_ {
    move |ui: &mut Ui| _ = ui.markdown(doc)
}

#[test]
fn pieces_of_rich_text_break_into_rows_together_and_its_links_are_pressed() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let (mut pressed, mut wide) = (Vec::new(), 240.0);
    let build = |ui: &mut Ui, pressed: &mut Vec<usize>, wide: f32| {
        let t = ui.theme;
        ui.column().w(Length::Px(wide)).open();
        let pieces = [Span::text("Read the "), Span::text("guide").link(), Span::text(" before "), Span::text("the rest").bold().color(t.green), Span::text(" of it.")];
        pressed.extend(ui.rich(&pieces));
        ui.close();
    };
    settle(&mut w, |ui| build(ui, &mut pressed, wide));
    // In 240: "Read the guide" (168), then "before the rest of" (84 + 112 in bold + 36),
    // then "it.": three rows of 20. It is reported as one text, and its link as a link.
    let text = expect_node_as(&w, Role::Text, "Read the guide before the rest of it.").rect;
    assert_eq!((text.w, text.h), (240.0, 60.0));
    let link = expect_node_as(&w, Role::Link, "guide").rect;
    assert_eq!((link.x, link.y, link.w, link.h), (108.0, 0.0, 60.0, 20.0));
    // Pressed by the pointer, and by Enter once Tab has gone to it.
    click(&mut w, middle(link), |ui| build(ui, &mut pressed, wide));
    assert_eq!(pressed, [1]);
    click(&mut w, pt(400.0, 280.0), |ui| build(ui, &mut pressed, wide));
    key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &mut pressed, wide));
    assert!(expect_node_as(&w, Role::Link, "guide").focused);
    key(&mut w, Key::Enter, Mods::NONE, |ui| build(ui, &mut pressed, wide));
    assert_eq!(pressed, [1, 1]);
    // Narrower, it has more rows; wider, fewer. (The run after the change is the one
    // that knows the new width: it describes the paragraph once more, by itself.)
    for (room, rows) in [(120.0, 5.0), (420.0, 2.0)] {
        wide = room;
        w.run(&mut |ui| build(ui, &mut pressed, wide));
        assert_eq!(expect_node_as(&w, Role::Text, "Read the guide before the rest of it.").rect.h, rows * 20.0, "in {room}");
        assert!(w.passes <= 2);
    }
}

#[test]
fn icons_and_pictures_stand_on_the_line_among_the_words() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let picture = w.add_picture(2, vec![0xffff_0000; 4]);
    let build = |ui: &mut Ui| {
        let t = ui.theme;
        let star = taut::Icon::solid("M8 1 10 6h5l-4 3 2 6-5-4-5 4 2-6-4-3h5z");
        ui.rich(&[Span::text("A star "), Span::icon(star), Span::text(" and a picture "), Span::picture(picture, 30.0, 30.0), Span::text(" in "), Span::text("code").code().color(t.amber)]);
    };
    settle(&mut w, build);
    // One row of 420: the picture, 30 high over the line the letters stand on, makes it
    // 30 + 4 under the line. Its pixels are there, and the word of code is on its ground.
    let text = expect_node_as(&w, Role::Text, "A star  and a picture  in code").rect;
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
    w.set_screen_reader(true);
    let doc = Markdown::parse(EVERY_BLOCK);
    let build = |ui: &mut Ui| {
        ui.column().grow().scroll().open();
        ui.markdown(&doc);
        ui.close();
    };
    settle(&mut w, build);
    let given = roles_and_names(&w);
    let words = |role: Role, name: &str| (role, name.to_string());
    let expected = [
        words(Role::Document, "Title"),
        words(Role::Heading, "Title"),
        words(Role::Paragraph, "A paragraph with a link in it."),
        words(Role::List, ""),
        words(Role::ListItem, ""),
        words(Role::Paragraph, "one"),
        words(Role::ListItem, ""),
        words(Role::Paragraph, "two"),
        words(Role::List, ""),
        words(Role::ListItem, ""),
        words(Role::Paragraph, "inner"),
        words(Role::Quote, ""),
        words(Role::Paragraph, "quoted"),
        words(Role::Code, "fn main() {}"),
        words(Role::Table, ""),
        words(Role::ColumnHeader, "a"),
        words(Role::ColumnHeader, "b"),
        words(Role::TableRow, "1, 2"),
        words(Role::Cell, "1"),
        words(Role::Cell, "2"),
        words(Role::Link, "link"),
    ];
    assert_eq!(given, expected);
    // The heading says its level; each item which of how many it is; and everything is in
    // the document.
    let nodes = w.visible_access_nodes();
    assert_eq!(nodes.iter().find(|node| node.role == Role::Heading).map(|node| node.level), Some(1));
    let items: Vec<_> = nodes.iter().filter(|node| node.role == Role::ListItem).map(|node| node.position).collect();
    assert_eq!(items, [Some([1, 2]), Some([2, 2]), Some([1, 1])]);
    assert!(nodes.iter().skip(1).all(|node| node.parent.is_some()));
}

#[test]
fn a_document_is_selected_by_the_pointer_and_the_keys_and_copied() {
    let mut w = window(1.0);
    let doc = Markdown::parse("Hello brave new world\n\nA second *block* of words.\n");
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
    w.set_screen_reader(true);
    let filler: String = (0..30).map(|n| format!("Paragraph {n}.\n\n")).collect();
    let doc = Markdown::parse(&format!("Go [down](#far-away) or [out](https://example.com/out).\n\n{filler}## Far away\n\nThe [end](https://example.com/end).\n\n{filler}"));
    let mut followed = Vec::new();
    let build = |ui: &mut Ui, followed: &mut Vec<String>| {
        ui.column().grow().scroll().open();
        followed.extend(ui.markdown(&doc).map(str::to_string));
        ui.close();
    };
    settle(&mut w, |ui| build(ui, &mut followed));
    assert!(!roles_and_names(&w).contains(&(Role::Heading, "Far away".into())));
    // Tab goes to the document, then to each link in turn, also to one out of view, which
    // the document is scrolled to; Enter follows it.
    for _ in 0..4 {
        key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &mut followed));
    }
    settle(&mut w, |ui| build(ui, &mut followed));
    assert!(expect_node_as(&w, Role::Link, "end").focused);
    key(&mut w, Key::Enter, Mods::NONE, |ui| build(ui, &mut followed));
    assert_eq!(followed, ["https://example.com/end"]);
    // Back at the top, a link to a heading of its own scrolls the document to it; the
    // app hears nothing of it.
    does(&mut w, Event::Move(pt(200.0, 150.0)), |ui| build(ui, &mut followed));
    does(&mut w, Event::Wheel(0.0, -10_000.0), |ui| build(ui, &mut followed));
    settle(&mut w, |ui| build(ui, &mut followed));
    let down = middle(expect_node_as(&w, Role::Link, "down").rect);
    click(&mut w, down, |ui| build(ui, &mut followed));
    settle(&mut w, |ui| build(ui, &mut followed));
    assert_eq!(followed.len(), 1);
    assert!(expect_node_as(&w, Role::Heading, "Far away").rect.y < 40.0);
}

#[test]
fn code_is_coloured_by_its_language_and_pictures_are_given_by_name() {
    let mut w = window(1.0);
    let mut doc = Markdown::parse("```rust\nfn main() {}\n```\n\n![a red square](red.png)\n");
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

/// A document in a scroll box filling the window.
fn scrolled(doc: &Markdown) -> impl FnMut(&mut Ui) + '_ {
    move |ui: &mut Ui| {
        ui.column().grow().scroll().open();
        ui.markdown(doc);
        ui.close();
    }
}

#[test]
fn a_documents_table_is_told_as_rows_and_cells_under_its_headings() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse("| Name | Size |\n|---|--:|\n| a | 1 |\n| b | 2 |\n");
    settle(&mut w, build(&doc));
    let nodes = w.visible_access_nodes().to_vec();
    let table = nodes.iter().position(|node| node.role == Role::Table).expect("a table");
    assert_eq!(nodes[table].grid, Some([2, 2]), "two rows of two columns, the headings apart");
    let under = |role: Role| nodes.iter().filter(move |node| node.role == role);
    let heads: Vec<_> = under(Role::ColumnHeader).map(|node| (node.name.as_str(), node.position, node.parent)).collect();
    assert_eq!(heads, [("Name", Some([1, 2]), Some(table)), ("Size", Some([2, 2]), Some(table))]);
    let rows: Vec<_> = under(Role::TableRow).map(|node| (node.name.as_str(), node.position)).collect();
    assert_eq!(rows, [("a, 1", Some([1, 2])), ("b, 2", Some([2, 2]))]);
    let cells: Vec<_> = under(Role::Cell).map(|node| (node.name.as_str(), node.grid)).collect();
    assert_eq!(cells, [("a", Some([0, 0])), ("1", Some([0, 1])), ("b", Some([1, 0])), ("2", Some([1, 1]))]);
}

#[test]
fn ctrl_c_on_a_document_with_nothing_selected_is_left_to_the_app() {
    let mut w = window(1.0);
    let doc = Markdown::parse("Some words.\n");
    let mut heard = 0;
    let build = |ui: &mut Ui, heard: &mut i32| {
        ui.markdown(&doc);
        *heard += ui.key(Key::Char('c'), Mods::CTRL) as i32;
    };
    settle(&mut w, |ui| build(ui, &mut heard));
    key(&mut w, Key::Tab, Mods::NONE, |ui| build(ui, &mut heard));
    key(&mut w, Key::Char('c'), Mods::CTRL, |ui| build(ui, &mut heard));
    assert_eq!(heard, 1);
    // Selected, it copies.
    key(&mut w, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut heard));
    key(&mut w, Key::Char('c'), Mods::CTRL, |ui| build(ui, &mut heard));
    assert_eq!((heard, w.clipboard().as_deref()), (1, Some("Some words.")));
}

#[test]
fn a_documents_table_rows_keep_their_places_when_laid_out_at_another_width() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse("| Name | Size |\n|---|--:|\n| a | 1 |\n| b | 2 |\n");
    settle(&mut w, build(&doc));
    w.resize(taut::geom::Size { w: 400.0, h: 300.0 }, 1.0);
    w.screen = taut::Surface::draw(&mut w.ui, 64);
    settle(&mut w, build(&doc));
    let rows: Vec<_> = w.visible_access_nodes().iter().filter(|node| node.role == Role::TableRow).map(|node| node.position).collect();
    assert_eq!(rows, [Some([1, 2]), Some([2, 2])]);
}

#[test]
fn the_selection_in_a_document_is_told_in_each_block_it_covers() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse("Hello brave new world\n\nA second *block* of words.\n");
    settle(&mut w, build(&doc));
    assert!(w.visible_access_nodes().iter().all(|node| node.caret.is_none()));
    // From "brave" into the second block: each block is told its part, in characters, from
    // where the selection was started to where it was taken.
    drag(&mut w, pt(74.0, 10.0), pt(26.0, 41.0), build(&doc));
    assert_eq!(expect_node_as(&w, Role::Paragraph, "Hello brave new world").caret, Some([6, 21]));
    assert_eq!(expect_node_as(&w, Role::Paragraph, "A second block of words.").caret, Some([0, 2]));
}

#[test]
fn a_links_title_shows_while_it_is_hovered_and_is_told() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse("See [the guide](https://example.com \"How it all works\") first.\n");
    settle(&mut w, build(&doc));
    let link = expect_node_as(&w, Role::Link, "the guide");
    assert_eq!(link.description, "How it all works");
    assert!(find_node(&w, "How it all works").is_none());
    // Once the pointer rests on it, its title shows by it.
    does(&mut w, Event::Move(middle(link.rect)), build(&doc));
    w.set_time(Duration::from_secs(1));
    settle(&mut w, build(&doc));
    assert!(find_node(&w, "How it all works").is_some_and(|tip| tip.rect.y >= link.rect.bottom()));
}

#[test]
fn a_documents_context_menu_copies_selects_all_and_copies_a_link() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse("Some [words](https://example.com/w) here.\n");
    settle(&mut w, build(&doc));
    let right_click = |w: &mut Window, at| {
        for event in [Event::Move(at), Event::Press(taut::Button::Right, Mods::NONE), Event::Release(taut::Button::Right)] {
            does(w, event, build(&doc));
        }
        w.run(&mut build(&doc));
    };
    // On its text: copy (nothing to copy yet) and select all; no link to copy.
    right_click(&mut w, pt(10.0, 10.0));
    assert!(!expect_node_as(&w, Role::MenuItem, "Copy").enabled);
    assert!(find_node_as(&w, Role::MenuItem, "Copy link").is_none());
    let at = middle(expect_node_as(&w, Role::MenuItem, "Select all").rect);
    click(&mut w, at, build(&doc));
    right_click(&mut w, pt(10.0, 10.0));
    let at = middle(expect_node_as(&w, Role::MenuItem, "Copy").rect);
    click(&mut w, at, build(&doc));
    assert_eq!(w.clipboard().as_deref(), Some("Some words here."));
    // On the link: its address too.
    let at = middle(expect_node_as(&w, Role::Link, "words").rect);
    right_click(&mut w, at);
    let at = middle(expect_node_as(&w, Role::MenuItem, "Copy link").rect);
    click(&mut w, at, build(&doc));
    assert_eq!(w.clipboard().as_deref(), Some("https://example.com/w"));
}

#[test]
fn a_selection_dragged_past_the_views_edge_scrolls_it() {
    let mut w = window(1.0);
    let text: String = (0..60).map(|n| format!("Paragraph {n}.\n\n")).collect();
    let doc = Markdown::parse(&text);
    settle(&mut w, scrolled(&doc));
    for event in [Event::Move(pt(1.0, 10.0)), Event::Press(taut::Button::Left, Mods::NONE), Event::Move(pt(30.0, 298.0))] {
        does(&mut w, event, scrolled(&doc));
    }
    // Held at the bottom edge, time passes: the view goes on down, and the selection with it.
    for step in 1..=120 {
        w.set_time(Duration::from_millis(16 * step));
        w.run(&mut scrolled(&doc));
    }
    does(&mut w, Event::Release(taut::Button::Left), scrolled(&doc));
    assert!(find_node(&w, "Paragraph 0.").is_none_or(|node| !node.visible));
    key(&mut w, Key::Char('c'), Mods::CTRL, scrolled(&doc));
    let copied = w.clipboard().unwrap_or_default();
    assert!(copied.starts_with("Paragraph 0.\nParagraph 1.") && copied.contains("Paragraph 15."), "{copied:?}");
}

#[test]
fn a_wide_code_block_scrolls_sideways_by_its_keys() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse(&format!("```\nstart{}end\n```\n", " ".repeat(60)));
    settle(&mut w, build(&doc));
    let code = expect_node_as(&w, Role::Code, &format!("start{}end", " ".repeat(60)));
    let row = taut::Rect { y: code.rect.y + 2.0, h: 30.0, ..code.rect };
    let (left, right) = (taut::Rect { w: 40.0, ..row }, taut::Rect { x: row.right() - 60.0, w: 50.0, ..row });
    let background = w.theme.panel;
    assert!(painted_pixels(&w, left, background) > 0 && painted_pixels(&w, right, background) == 0, "a row, not wrapped");
    // Tab reaches the document, then the code; End takes it to its far end, Home back.
    for _ in 0..2 {
        key(&mut w, Key::Tab, Mods::NONE, build(&doc));
    }
    assert_eq!(focused_node(&w).map(|node| node.role), Some(Role::Code));
    key(&mut w, Key::End, Mods::NONE, build(&doc));
    settle(&mut w, build(&doc));
    assert!(painted_pixels(&w, right, background) > 0);
    key(&mut w, Key::Home, Mods::NONE, build(&doc));
    settle(&mut w, build(&doc));
    assert!(painted_pixels(&w, right, background) == 0);
}

#[test]
fn a_list_nested_thousands_deep_shows_its_text_wide_enough_to_read() {
    let mut w = window(1.0);
    w.set_screen_reader(true);
    let doc = Markdown::parse(&("- ".repeat(3000) + "deep"));
    settle(&mut w, scrolled(&doc));
    let deep = expect_node_as(&w, Role::Paragraph, "deep");
    assert!(deep.rect.w >= 100.0 && deep.rect.right() <= 420.0, "{:?}", deep.rect);
}
