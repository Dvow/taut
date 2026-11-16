//! The code editor, through a headless window: after every step the incrementally redrawn
//! window is compared with a full fresh draw.
mod common;
use common::*;
use taut::geom::pt;
use taut::{Code, Event, Key, Mods, Role, Rules, Ui};

/// A window with nothing but a code box in it.
fn build(code: &mut Code) -> impl FnMut(&mut Ui) + '_ {
    move |ui: &mut Ui| {
        ui.code_box(code, "Source");
    }
}

#[test]
fn a_large_file_is_typed_into_and_scrolled_where_it_is() {
    let text: String = (0..100_000).map(|n| format!("fn f{n}() {{ let x = {n}; }} // line {n}\n")).collect();
    let mut code = Code::new(text, &Rules::RUST);
    let mut w = window(1.0);
    settle(&mut w, build(&mut code));
    // A press puts the keyboard and the cursor in it; the end of the text is a key away.
    click(&mut w, pt(200.0, 30.0), build(&mut code));
    key(&mut w, Key::End, Mods::CTRL, build(&mut code));
    assert_eq!(code.cursor(), (100_001, 1));
    typed(&mut w, "// the end", build(&mut code));
    assert!(code.text().ends_with("line 99999\n// the end"));
    // What is in view was scrolled to the cursor, and the box was told so.
    let said = w.said().iter().any(|said| said.role == Role::TextBox);
    assert!(!said, "nothing is told while no screen reader listens");
    w.listen(true);
    w.run(&mut build(&mut code));
    let told = w.said().iter().find(|said| said.role == Role::TextBox).expect("told as a text box").clone();
    assert_eq!((told.name.as_str(), told.of, told.focused), ("Source", Some([100_001, 100_001]), true));
    assert_eq!(told.value.len(), code.text().len());
    // Undone at once, and the first line is a key away again.
    key(&mut w, Key::Char('z'), Mods::CTRL, build(&mut code));
    key(&mut w, Key::Home, Mods::CTRL, build(&mut code));
    assert_eq!(code.cursor(), (1, 1));
    assert!(code.text().ends_with("line 99999\n"));
}

#[test]
fn tab_indents_and_escape_then_tab_goes_on() {
    let mut code = Code::new("fn main() {}", &Rules::RUST);
    let mut other = String::new();
    let mut w = window(1.0);
    let mut both = |ui: &mut Ui| {
        ui.code_box(&mut code, "Source");
        ui.text_box(&mut other, "After it");
    };
    settle(&mut w, &mut both);
    click(&mut w, pt(300.0, 14.0), &mut both);
    let code_box = w.focus();
    assert_ne!(code_box, 0);
    key(&mut w, Key::Tab, Mods::NONE, &mut both);
    assert_eq!(w.focus(), code_box, "Tab stays in the box");
    key(&mut w, Key::Escape, Mods::NONE, &mut both);
    key(&mut w, Key::Tab, Mods::NONE, &mut both);
    assert_ne!(w.focus(), code_box, "after Escape, Tab goes on");
    assert_eq!(code.text(), "fn main() {}    ");
}

#[test]
fn find_opens_its_bar_and_selects_what_it_finds() {
    let mut code = Code::new("one two\nthree two\n", &Rules::PLAIN);
    let mut w = window(1.0);
    settle(&mut w, build(&mut code));
    click(&mut w, pt(300.0, 14.0), build(&mut code));
    key(&mut w, Key::Char('f'), Mods::CTRL, build(&mut code));
    w.run(&mut build(&mut code));
    typed(&mut w, "two", build(&mut code));
    w.run(&mut build(&mut code));
    assert_eq!(code.cursor(), (2, 10), "found on from the cursor, which the press put at the end of line 1");
    key(&mut w, Key::Enter, Mods::NONE, build(&mut code));
    assert_eq!(code.cursor(), (1, 8), "Enter finds the next, going round the end");
    key(&mut w, Key::Escape, Mods::NONE, build(&mut code));
    // Typing again goes in place of what was found last.
    typed(&mut w, "2", build(&mut code));
    assert_eq!(code.text(), "one 2\nthree two\n");
}

#[test]
fn wrapping_and_folding_are_drawn_as_they_change() {
    let text = "fn a() {\n    one two three four five six seven eight nine ten eleven twelve\n}\n";
    let mut code = Code::new(text, &Rules::RUST);
    code.wrap = true;
    let mut w = window(1.0);
    settle(&mut w, build(&mut code));
    click(&mut w, pt(300.0, 14.0), build(&mut code));
    key(&mut w, Key::Char('f'), Mods { ctrl: true, alt: true, shift: false }, build(&mut code));
    key(&mut w, Key::Down, Mods::NONE, build(&mut code));
    assert_eq!(code.cursor().0, 3, "Down goes past the folded lines");
    w.event(Event::Wheel(0.0, 40.0));
    w.run(&mut build(&mut code));
}
