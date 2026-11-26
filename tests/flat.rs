//! The library's own code stays flat: nothing is nested more than two blocks deep inside a
//! function. This reads the source and fails at the first line that is.

/// The deepest a line of code may start: a method's body, and two blocks in it.
const DEEPEST: usize = 16;

#[test]
fn no_code_is_nested_deeper_than_two_blocks() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut too_deep = Vec::new();
    // (Data files, such as the word lists, are no code.)
    let code = |path: &std::path::PathBuf| path.extension().is_some_and(|extension| extension == "rs" || extension == "js");
    for file in std::fs::read_dir(&source).unwrap().map(|entry| entry.unwrap().path()).filter(code) {
        let text = std::fs::read_to_string(&file).unwrap();
        // (Tests at the end of a file may be laid out as they like, and so may comments.)
        let code = text.lines().enumerate().take_while(|(_, line)| !line.starts_with("#[cfg(test)]"));
        let deep = code.filter(|(_, line)| line.len() - line.trim_start().len() > DEEPEST && !line.trim_start().starts_with("//"));
        too_deep.extend(deep.map(|(number, line)| format!("{}:{}: {}", file.display(), number + 1, line.trim())));
    }
    assert!(too_deep.is_empty(), "nested too deep:\n{}", too_deep.join("\n"));
}

#[test]
fn the_source_is_one_flat_folder() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let folders: Vec<_> = std::fs::read_dir(source).unwrap().map(|entry| entry.unwrap().path()).filter(|path| path.is_dir()).collect();
    assert!(folders.is_empty(), "folders inside src: {folders:?}");
}
