//! Fuzzer finds in documents (`Ui::markdown`), each minimized. Regression tests: each failed
//! before its fix.
mod common;

use common::{settle, window};
use std::time::{Duration, Instant};
use taut::{Markdown, Role, Ui};

fn shown(ui: &mut common::Window, doc: &Markdown) {
    settle(ui, |ui: &mut Ui| {
        ui.column().grow().open();
        ui.markdown(doc);
        ui.close();
    });
}

/// An item of a list with nothing written in it ("1." alone, "- " at the end of a document
/// being written, an item whose only content is a list of empty items) is laid out with no
/// height: so its list has none, and the document ends where the list begins. Its mark
/// ("1.") is still drawn, a row high, below the end of the document's box: a window that
/// draws in strips cuts it off at the next strip, a document in a box that scrolls cannot
/// be scrolled down to it, and a screen reader is not told of the list at all. (The
/// document's height comes from `close_ended` in markdown_view.rs: a block that holds others is
/// as high as what is in it, and an empty item holds nothing.)
#[test]
fn an_empty_item_of_a_list_is_as_high_as_its_mark() {
    let mut ui = window(1.0);
    ui.set_screen_reader(true);
    shown(&mut ui, &Markdown::parse("Some words\n\n1."));
    let rect_of = |role: Role| ui.visible_access_nodes().iter().find(|node| node.role == role).map(|node| node.rect);
    let (document, words) = (rect_of(Role::Document).unwrap(), rect_of(Role::Paragraph).unwrap());
    assert!(rect_of(Role::List).is_some(), "the list was not told of");
    assert!(document.h >= words.h * 2.0, "the document is {} high, its first paragraph {}: the list after it has no height", document.h, words.h);
}

/// The same, as the fuzzer found it: a paragraph broken over eight rows, and under it a
/// list whose one item holds an empty list ("1. 0."). The marks lie below the end of the
/// document, across the line between two strips of 64 rows; the first run drew them
/// whole, and drawn afresh in strips, rows 256 and 257 of them are missing.
#[test]
fn a_list_of_empty_items_after_a_long_paragraph_is_drawn_the_same_in_strips_and_whole() {
    let text = format!("{}\n\n` {}\n1. 0.", "0".repeat(72), "0".repeat(211));
    let doc = Markdown::parse(&text);
    let mut ui = window(1.0);
    shown(&mut ui, &doc);
}

/// Lists in lists, as deep as a text can make them: `- - - - x`, 8,000 deep, in 16,000
/// bytes, every one read and laid out. (Found as 15,000 bytes of `- - -` that took 0.65 s,
/// when both grew with the square of the depth: each marker looked for a thematic break in
/// the rest of the line, and each list counted the lists open around it.)
#[test]
fn lists_nested_thousands_deep_are_read_and_laid_out_in_time() {
    let text = format!("{}x\n", "- ".repeat(8_000));
    let clock = Instant::now();
    let doc = Markdown::parse(&text);
    let mut ui = window(1.0);
    shown(&mut ui, &doc);
    let took = clock.elapsed();
    assert!(took < Duration::from_secs(1), "8,000 lists in lists took {took:?} to read and lay out");
}
