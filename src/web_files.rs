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

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn save_file(bytes: *const u8, length: usize);
}

/// What the loader hears of the pickers: a file of several (its name and contents), more to
/// come or the last; that the files of a folder picked are all handed over; where the save
/// picker will write (the name chosen).
mod heard {
    pub const MORE: u32 = 0;
    pub const LAST: u32 = 1;
    pub const FOLDER: u32 = 2;
    pub const SAVED: u32 = 3;
}

/// Where files picked are kept.
const PICKED: &str = "/picked";

thread_local! {
    /// Files picked in the picker open now, until it answers.
    static INCOMING: RefCell<Vec<(PathBuf, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
    /// Every file the picker chose at its last answer, until the dialog takes them.
    static CHOSEN: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// Where the save picker chose to write last.
    static SAVING: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
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

/// Writes `bytes` into the file the save picker chose, if `file` is where it saved (or that
/// with an ending the file dialog added): whether it is. The browser writes after this returns.
pub(crate) fn save_chosen(file: &Path, bytes: &[u8]) -> bool {
    let chosen = SAVING.with_borrow(|saving| saving.as_deref().is_some_and(|saved| is_or_adds_ending(file, saved)));
    if chosen {
        // SAFETY: the loader reads the bytes at once.
        unsafe { save_file(bytes.as_ptr(), bytes.len()) };
    }
    chosen
}

/// Whether `file` is `saved`, or `saved` with an ending added.
fn is_or_adds_ending(file: &Path, saved: &Path) -> bool {
    let (file, saved) = (file.as_os_str().as_encoded_bytes(), saved.as_os_str().as_encoded_bytes());
    file.strip_prefix(saved).is_some_and(|rest| rest.first().is_none_or(|first| *first == b'.'))
}

/// A file of several, the folder picked, or where the save picker will write.
fn heard(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    let answer = match heard.what {
        heard::MORE | heard::LAST => {
            INCOMING.with_borrow_mut(|incoming| incoming.push((Path::new(PICKED).join(&heard.name), heard.bytes)));
            if heard.what == heard::MORE {
                return (Then::Wait, 0);
            }
            let chosen = INCOMING.with_borrow(|incoming| incoming.iter().map(|(path, _)| path.clone()).collect());
            keep_picked(chosen)
        }
        heard::FOLDER => {
            let folder = INCOMING.with_borrow(|incoming| incoming.first().map(|(file, _)| folder_of(file))).unwrap_or_default();
            // (Its files of an earlier pick are let go.)
            FILES.with_borrow_mut(|files| files.retain(|(path, _)| !path.starts_with(&folder)));
            keep_picked(vec![folder])
        }
        heard::SAVED => {
            let path = Path::new("/saved").join(String::from_utf8_lossy(&heard.bytes).as_ref());
            SAVING.set(Some(path.clone()));
            CHOSEN.set(vec![path.clone()]);
            path
        }
        _ => return (Then::Wait, 0),
    };
    ui.file_dialog_answered(Some(answer));
    (Then::Run, 0)
}

/// Keeps the files picked, and answers with `chosen`: the first of them.
fn keep_picked(chosen: Vec<PathBuf>) -> PathBuf {
    INCOMING.take().into_iter().for_each(|(path, bytes)| keep_file(path, bytes));
    let answer = chosen.first().cloned().unwrap_or_default();
    CHOSEN.set(chosen);
    answer
}

/// The folder picked that `file` is in: `/picked/<folder>` of `/picked/<folder>/<path in it>`.
fn folder_of(file: &Path) -> PathBuf {
    let inside = file.strip_prefix(PICKED).unwrap_or(file);
    Path::new(PICKED).join(inside.iter().next().unwrap_or_default())
}
