//! The browser's file pickers beyond one file: several files, a folder, and the save picker.
//! Handed to the page by the first call that may need them (`FileDialog::pick_many`,
//! `pick_folder`, `save`): a program that never makes one carries none of it. (One file picked
//! comes through `taut_file`.)
//!
//! - **Several files** come one by one, each read whole, and are answered together: the first
//!   is the answer, all are in [`FileDialog::chosen`](crate::FileDialog::chosen).
//! - **A folder** (`webkitdirectory`): each of its files is kept as `/picked/<folder>/<path in
//!   it>`; the answer is `/picked/<folder>`, its files listed by
//!   [`web::files_in`](crate::web::files_in).
//! - **Saving**, where the browser has a save picker (Chrome, Edge): the answer is
//!   `/saved/<name chosen>`, and what the app writes there ([`web::save`](crate::web::save),
//!   [`storage::write`](crate::storage::write)) goes into the file chosen.
use crate::ui::Ui;
use crate::web_host::{FILES, Heard, Part, Then, add_part, keep_file, parts};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// What the loader hears of the pickers: a file of several (`number` is its name's length, the
/// bytes its name then its contents), more to come or the last; the folder picked, after its
/// files (the bytes its name); where the save picker will write (the bytes the name chosen).
mod heard {
    pub const MORE: u32 = 0;
    pub const LAST: u32 = 1;
    pub const FOLDER: u32 = 2;
    pub const SAVED: u32 = 3;
}

thread_local! {
    /// Files picked in the picker open now, until it answers.
    static INCOMING: RefCell<Vec<(PathBuf, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
    /// Every file the picker chose at its last answer, until the dialog takes them.
    static CHOSEN: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Has the page hear the pickers' answers beyond one file.
pub(crate) fn hand_pickers() {
    add_part(parts::FILES, Part { after: |_| {}, heard });
}

/// Every file the picker chose at its last answer, if `first` is the first of them (see
/// `FileDialog::chosen`).
pub(crate) fn chosen(first: PathBuf) -> Vec<PathBuf> {
    let all = CHOSEN.take();
    if all.first() == Some(&first) { all } else { vec![first] }
}

/// The files the page holds inside `folder`, by path (see [`web::files_in`](crate::web::files_in)).
pub(crate) fn files_in(folder: &Path) -> Vec<PathBuf> {
    FILES.with_borrow(|files| files.iter().filter(|(path, _)| path.starts_with(folder) && path != folder).map(|(path, _)| path.clone()).collect())
}

/// A file of several, the folder picked, or where the save picker will write.
fn heard(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    let answer = match heard.what {
        heard::MORE | heard::LAST => {
            let named = (heard.number as usize).min(heard.bytes.len());
            let path = PathBuf::from("/picked").join(String::from_utf8_lossy(&heard.bytes[..named]).as_ref());
            INCOMING.with_borrow_mut(|incoming| incoming.push((path, heard.bytes[named..].to_vec())));
            if heard.what == heard::MORE {
                return (Then::Wait, 0);
            }
            answer_with(None)
        }
        heard::FOLDER => answer_with(Some(PathBuf::from("/picked").join(String::from_utf8_lossy(&heard.bytes).as_ref()))),
        heard::SAVED => {
            let path = PathBuf::from("/saved").join(String::from_utf8_lossy(&heard.bytes).as_ref());
            CHOSEN.set(vec![path.clone()]);
            path
        }
        _ => return (Then::Wait, 0),
    };
    ui.file_dialog_answered(Some(answer));
    (Then::Run, 0)
}

/// Keeps the files picked, and answers with them: the first, or the folder they are in (once
/// its files of an earlier pick are let go).
fn answer_with(folder: Option<PathBuf>) -> PathBuf {
    let incoming = INCOMING.take();
    let chosen: Vec<PathBuf> = match &folder {
        Some(folder) => vec![folder.clone()],
        None => incoming.iter().map(|(path, _)| path.clone()).collect(),
    };
    if let Some(folder) = &folder {
        FILES.with_borrow_mut(|files| files.retain(|(path, _)| !path.starts_with(folder)));
    }
    incoming.into_iter().for_each(|(path, bytes)| keep_file(path, bytes));
    let answer = chosen.first().cloned().unwrap_or_default();
    CHOSEN.set(chosen);
    answer
}
