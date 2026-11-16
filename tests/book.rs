//! The book (the chapters in `docs/`) keeps its promises. Every picture it shows was drawn by
//! `examples/book.rs` from code the chapter shows: each module of that example, up to the line
//! "The picture of it", is a code block of the chapter named above it, character for character.
//! Every picture a chapter shows is in `docs/pictures`, and every picture there is shown. And
//! the to-do app the fourth chapter builds is `examples/todo.rs`, whole.
use std::path::Path;

/// The crate's folder, where `docs` and `examples` are.
fn here() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(file: &str) -> String {
    std::fs::read_to_string(here().join(file)).unwrap_or_else(|_| panic!("{file} could not be read")).replace("\r\n", "\n")
}

/// The chapters: each one's file name, and what is in it.
fn chapters() -> Vec<(String, String)> {
    let folder = std::fs::read_dir(here().join("docs")).expect("there is no docs folder");
    let mut names: Vec<String> = folder.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).filter(|name| name.ends_with(".md")).collect();
    names.sort();
    names.into_iter().map(|name| (name.clone(), read(&format!("docs/{name}")))).collect()
}

/// The blocks of Rust in a chapter, each as one text.
fn blocks(chapter: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = chapter.lines();
    while let Some(line) = lines.next() {
        if line.starts_with("```rust") {
            blocks.push(lines.by_ref().take_while(|line| *line != "```").collect::<Vec<_>>().join("\n"));
        }
    }
    blocks
}

/// The modules of the picture program that come from a chapter: the chapter's file name,
/// the module's name, and its code up to the line where the book's own part starts, as it
/// would stand outside the module.
fn shown_code() -> Vec<(String, String, String)> {
    let program = read("examples/book.rs");
    let lines: Vec<&str> = program.lines().collect();
    let mut found = Vec::new();
    for at in 1..lines.len() {
        let (Some(chapter), Some(module)) = (lines[at - 1].strip_prefix("// docs/"), lines[at].strip_prefix("mod ").and_then(|rest| rest.strip_suffix(" {"))) else { continue };
        let body = lines[at + 1..].iter().take_while(|line| line.trim() != "// The picture of it:");
        let code: Vec<&str> = body.map(|line| line.strip_prefix("    ").unwrap_or(line)).collect();
        found.push((chapter.to_string(), module.to_string(), code.join("\n").trim_end().to_string()));
    }
    found
}

#[test]
fn every_picture_is_drawn_from_code_its_chapter_shows() {
    let chapters = chapters();
    let shown = shown_code();
    assert!(shown.len() > 10, "the picture program has hardly any modules from the chapters");
    for (chapter, module, code) in shown {
        let Some((_, text)) = chapters.iter().find(|(name, _)| *name == chapter) else { panic!("module {module} names {chapter}, which is not in docs") };
        assert!(blocks(text).contains(&code), "module {module} of examples/book.rs is not a block of code in docs/{chapter}, as it is there:\n{code}");
    }
}

#[test]
fn every_picture_shown_is_there_and_every_one_there_is_shown() {
    let named: Vec<String> = chapters().iter().flat_map(|(_, text)| text.split("](pictures/").skip(1).map(|rest| rest.split(')').next().unwrap_or("").to_string()).collect::<Vec<_>>()).collect();
    let folder = std::fs::read_dir(here().join("docs/pictures")).expect("there is no docs/pictures folder");
    let there: Vec<String> = folder.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
    for name in &named {
        assert!(there.contains(name), "a chapter shows pictures/{name}, which is not there");
    }
    for name in &there {
        assert!(named.contains(name), "docs/pictures/{name} is in no chapter");
    }
    // And the picture program draws each of them.
    let program = read("examples/book.rs");
    for name in &there {
        let drawn = format!("\"{}\"", name.trim_end_matches(".png"));
        assert!(program.contains(&drawn), "examples/book.rs does not draw {name}");
    }
}

#[test]
fn the_app_the_fourth_chapter_builds_is_the_example() {
    let app = read("examples/todo.rs");
    let (_, chapter) = chapters().into_iter().find(|(name, _)| name.starts_with("04-")).expect("there is no fourth chapter");
    assert!(blocks(&chapter).contains(&app.trim_end().to_string()), "examples/todo.rs is not a block of code in the fourth chapter, whole");
}
