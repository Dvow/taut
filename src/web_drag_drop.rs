//! Drag and drop with other programs on the web, through the browser's drag events: files and
//! text dropped on the page, and what the app drags out of it.
//!
//! - **In:** once the app asks about drops, the page takes the kinds it asks for (files, text),
//!   each handed over with the pointer's place. Dropped files are read whole and kept as
//!   `/dropped/<name>` (read with [`web::picked`](crate::web::picked)).
//! - **Out:** a box that hands something out ([`Ui::drag_out`]) is draggable where it is drawn.
//!   As the browser starts a drag there, that box runs once as the one lifted, as on iOS, and
//!   what it hands out becomes the drag's data: text, a link (`text/uri-list`), files and
//!   pictures as files (a picture as PNG; Chrome also saves the first file where it is
//!   dropped on the desktop, by `DownloadURL`). While the browser drags it over the page the
//!   interface hears it as the pointer, so let go there it drops as any drag inside does. At
//!   its end the app hears what the other side did (copied, moved, or nothing).
//!
//! Handed to the page the first time an app asks (`platform::hand_drops`,
//! `platform::enable_drag_out`): a program that never does carries none of it.
use crate::drag_drop::{DragData, DropEffect};
use crate::event::{Button, Event};
use crate::geom::{Point, Rect};
use crate::id::Id;
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn drag_from(left: f32, top: f32, width: f32, height: f32);
    fn drag_item(kind: u32, name: *const u8, name_length: usize, bytes: *const u8, length: usize);
}

/// What the loader hears of drags coming in.
mod dropping {
    /// Over the page: `number` says what the drag holds (1 files, 2 text, 3 both). Answered with
    /// what the app takes of it: 1 files, 2 text, 0 nothing.
    pub const OVER: u32 = 0;
    /// Gone from the page.
    pub const LEFT: u32 = 1;
    /// A dropped file: its name and contents.
    pub const FILE: u32 = 2;
    /// The files just handed over were dropped, here.
    pub const FILES: u32 = 3;
    /// Text dropped, here.
    pub const TEXT: u32 = 4;
}

/// What the loader hears of the app's own drag out.
mod dragging {
    /// The browser starts a drag here: answered with whether a box hands something out here.
    pub const START: u32 = 0;
    /// Over the page.
    pub const OVER: u32 = 1;
    /// Let go on the page.
    pub const DROP: u32 = 2;
    /// Ended: `number` 1 copied, 2 moved, else nothing taken.
    pub const END: u32 = 3;
    /// Gone from the page.
    pub const LEFT: u32 = 4;
    /// The pointer is here with no button down: answered with whether a box hands something
    /// out here (the browser may start a drag from it).
    pub const OFFERED: u32 = 5;
}

/// The kinds of item a drag out carries, as the loader takes them.
mod item {
    pub const TEXT: u32 = 0;
    pub const LINK: u32 = 1;
    pub const FILE: u32 = 2;
    /// A PNG file.
    pub const PICTURE: u32 = 3;
}

thread_local! {
    /// Files dropped (kept already), until the drop is told.
    static DROPPED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Has the page take drops, of the kinds the app asks for: from the next run on.
pub(crate) fn hand_drops() {
    add_part(parts::DROPS, Part { after: |_| {}, heard: heard_dropping });
}

/// Has the page drag out what the app hands out: from the next run on.
pub(crate) fn enable_drag_out() {
    add_part(parts::DRAG_OUT, Part { after: hand_out, heard: heard_dragging });
}

/// A drag from another program over the page, or dropped on it.
fn heard_dropping(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    match heard.what {
        dropping::OVER => {
            let taken = taken_of(ui, heard.number as u32);
            match taken {
                1 => ui.event(Event::FilesHovered(true)),
                2 => ui.event(Event::TextHovered(true)),
                _ => return (Then::Wait, 0),
            };
            ui.event(Event::Move(heard.at));
            (Then::Run, taken)
        }
        dropping::LEFT => {
            [Event::FilesHovered(false), Event::TextHovered(false), Event::Leave].into_iter().for_each(|event| _ = ui.event(event));
            (Then::Run, 0)
        }
        dropping::FILE => {
            let path = PathBuf::from("/dropped").join(&heard.name);
            crate::web_host::keep_file(path.clone(), heard.bytes);
            DROPPED.with_borrow_mut(|dropped| dropped.push(path));
            (Then::Wait, 0)
        }
        dropping::FILES => {
            let dropped = DROPPED.take();
            [Event::Move(heard.at), Event::FilesDropped(dropped)].into_iter().for_each(|event| _ = ui.event(event));
            (Then::Run, 0)
        }
        dropping::TEXT => {
            let text = String::from_utf8_lossy(&heard.bytes).into_owned();
            [Event::Move(heard.at), Event::TextDropped(text)].into_iter().for_each(|event| _ = ui.event(event));
            (Then::Run, 0)
        }
        _ => (Then::Wait, 0),
    }
}

/// What the app takes of a drag holding `holds` (1 files, 2 text, 3 both): 1 files, 2 text, 0
/// nothing. (Files and text are files to an app taking files.)
fn taken_of(ui: &Ui, holds: u32) -> u32 {
    if holds & 1 != 0 && ui.takes_files.get() {
        return 1;
    }
    if holds & 2 != 0 && ui.crossing.takes_text.get() { 2 } else { 0 }
}

/// After a run: what the lifted box handed out becomes the drag's items.
fn hand_out(ui: &mut Ui) {
    let lifted = std::mem::take(&mut ui.crossing.lift) != 0;
    let Some(what) = ui.take_drag_out() else { return };
    if !lifted {
        return ui.set_drop_effect(None);
    }
    match what {
        DragData::Text(text) => item(item::TEXT, "", text.as_bytes()),
        DragData::Link(link) => item(item::LINK, "", link.as_bytes()),
        DragData::Files(files) => files.iter().for_each(|file| file_item(file)),
        DragData::Picture(width, pixels) => item(item::PICTURE, "picture.png", &crate::png::encode_rgba(width, pixels).unwrap_or_default()),
    }
}

/// Hands the loader one item of the drag out.
fn item(kind: u32, name: &str, bytes: &[u8]) {
    // SAFETY: the loader reads both at once.
    unsafe { drag_item(kind, name.as_ptr(), name.len(), bytes.as_ptr(), bytes.len()) };
}

/// Hands the loader a file of the drag out, by its name: one handed to the page, else one the
/// app keeps in its storage.
fn file_item(file: &Path) {
    let name = file.file_name().map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let bytes = crate::web_host::picked(file).or_else(|| crate::storage::read(file).map(String::into_bytes)).unwrap_or_default();
    item(if name.ends_with(".png") { item::PICTURE } else { item::FILE }, &name, &bytes);
}

/// The app's own drag out, as the browser drags it.
fn heard_dragging(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    match heard.what {
        dragging::OFFERED => (Then::Wait, offer_at(ui, heard.at).is_some() as u32),
        dragging::START => {
            let offer = offer_at(ui, heard.at);
            if let Some((_, place)) = offer {
                // SAFETY: a plain request.
                unsafe { drag_from(place.x, place.y, place.w, place.h) };
            }
            ui.crossing.lift = offer.map_or(0, |(id, _)| id);
            ui.event(Event::Move(heard.at));
            (if offer.is_some() { Then::RunNow } else { Then::Wait }, offer.is_some() as u32)
        }
        // (Run at once: a drop that follows has the drag under way.)
        dragging::OVER => {
            ui.event(Event::Move(heard.at));
            (Then::RunNow, 0)
        }
        dragging::LEFT => {
            ui.event(Event::Leave);
            (Then::Run, 0)
        }
        dragging::DROP => {
            [Event::Move(heard.at), Event::Release(Button::Left)].into_iter().for_each(|event| _ = ui.event(event));
            ui.set_drop_effect(None);
            (Then::Run, 0)
        }
        dragging::END => {
            let taken = match heard.number as u32 {
                1 => Some(DropEffect::Copied),
                2 => Some(DropEffect::Moved),
                _ => None,
            };
            [Event::Leave, Event::Release(Button::Left)].into_iter().for_each(|event| _ = ui.event(event));
            ui.set_drop_effect(taken);
            (Then::Run, 0)
        }
        _ => (Then::Wait, 0),
    }
}

/// The box handing something out at `at` (the last drawn there), and its place, as the last run
/// laid them out.
fn offer_at(ui: &Ui, at: Point) -> Option<(Id, Rect)> {
    ui.crossing.offers.iter().rev().find(|(_, place)| place.contains(at)).copied()
}
