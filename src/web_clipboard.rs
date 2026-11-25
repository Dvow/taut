//! The clipboard's pictures and files on the web, once a program asks for one
//! (`platform::hand_pictures_and_files`): a program that never does carries none of it.
//!
//! - A picture copied goes to the system clipboard as PNG (`navigator.clipboard.write`, which
//!   the browser allows after a press or a key).
//! - The picture on the clipboard is the last one copied in the page, pasted into it (with
//!   Control+V), or read from the system clipboard as the page gains the keyboard, where the
//!   person let the page read it. It stands while the clipboard holds no text of the page's: a
//!   copy of text replaces it, as on the desktops.
//! - Files pasted with Control+V (copied in a file manager) are read whole and kept as
//!   `/pasted/<name>`. A page can't put files on the system clipboard: `copy_files` does
//!   nothing.
use crate::platform::PicturesAndFiles;
use crate::ui::Ui;
use crate::web_host::{CLIPBOARD, Heard, PageClipboard, Part, Then, add_part, parts};
use std::cell::RefCell;
use std::path::PathBuf;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn clipboard_watch();
    fn clipboard_picture_set(png: *const u8, length: usize);
}

/// What the loader hears of the clipboard.
mod heard {
    /// The picture on the clipboard (PNG bytes; none for no picture).
    pub const PICTURE: u32 = 0;
    /// A pasted file: its name and contents.
    pub const FILE: u32 = 1;
    /// The files just handed over were pasted (none: the paste held none).
    pub const FILES: u32 = 2;
}

/// The clipboard's picture and files as the page knows them.
#[derive(Default)]
struct Held {
    picture: Option<(usize, Vec<u32>)>,
    files: Vec<PathBuf>,
    /// Files of the paste under way (kept already).
    incoming: Vec<PathBuf>,
}

thread_local! {
    static HELD: RefCell<Held> = RefCell::new(Held::default());
}

/// The page clipboard's picture and file code; the loader reads pictures and files from now on.
pub(crate) fn pictures_and_files() -> PicturesAndFiles<PageClipboard> {
    add_part(parts::CLIPBOARD, Part { after: |_| {}, heard });
    // SAFETY: a plain request.
    unsafe { clipboard_watch() };
    PicturesAndFiles { picture, set_picture, files, set_files }
}

/// The picture, while the clipboard holds no text of the page's.
fn picture(_: &mut PageClipboard) -> Option<(usize, Vec<u32>)> {
    let text = CLIPBOARD.with_borrow(String::is_empty);
    text.then(|| HELD.with_borrow(|held| held.picture.clone())).flatten()
}

/// Copies a picture: kept here, and on the system clipboard as PNG.
fn set_picture(_: &mut PageClipboard, width: usize, pixels: &[u32]) {
    CLIPBOARD.with_borrow_mut(String::clear);
    HELD.with_borrow_mut(|held| (held.picture, held.files) = (Some((width, pixels.to_vec())), Vec::new()));
    let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
    // SAFETY: the loader reads the bytes at once.
    unsafe { clipboard_picture_set(png.as_ptr(), png.len()) };
}

/// The files last pasted.
fn files(_: &mut PageClipboard) -> Vec<PathBuf> {
    HELD.with_borrow(|held| held.files.clone())
}

/// A page can't put files on the system clipboard.
fn set_files(_: &mut PageClipboard, _: &[PathBuf]) {}

/// A picture or files from the system clipboard.
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    match heard.what {
        heard::PICTURE => {
            let picture = crate::png::decode(&heard.bytes);
            HELD.with_borrow_mut(|held| held.picture = picture);
        }
        heard::FILE => {
            let path = PathBuf::from("/pasted").join(&heard.name);
            crate::web_host::keep_file(path.clone(), heard.bytes);
            HELD.with_borrow_mut(|held| held.incoming.push(path));
        }
        heard::FILES => {
            HELD.with_borrow_mut(|held| held.files = std::mem::take(&mut held.incoming));
        }
        _ => {}
    }
    (Then::Wait, 0)
}
