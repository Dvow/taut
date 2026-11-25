//! Drag and drop with other programs on the web, through the browser's drag events: files and
//! text dropped on the page, and what the app drags out of it.
//!
//! - **In:** once the app asks about drops, the loader takes the kinds it asks for (files, text)
//!   over the page, and hands each over with the pointer's place. Dropped files are read whole
//!   and kept as `/dropped/<name>` (read with [`web::picked`](crate::web::picked)).
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
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts};
use std::cell::RefCell;
use std::path::PathBuf;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn drops_taken(files: u32, text: u32);
    fn drag_offers(places: *const f32, count: usize);
    fn drag_item(kind: u32, name: *const u8, name_length: usize, bytes: *const u8, length: usize);
}

/// What the loader hears of drags coming in.
mod dropping {
    /// Over the page: `number` 1 for files, 2 for text. Answered with whether it is taken.
    pub const OVER: u32 = 0;
    /// Gone from the page.
    pub const LEFT: u32 = 1;
    /// A dropped file: `number` is its name's length, the bytes its name then its contents.
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
}

/// The kinds of item a drag out carries, as the loader takes them.
mod item {
    pub const TEXT: u32 = 0;
    pub const LINK: u32 = 1;
    pub const FILE: u32 = 2;
}

/// What the page does with drags.
#[derive(Default)]
struct Drags {
    /// What the app takes dropped, as the loader was told: files, text.
    taken: (bool, bool),
    /// The places of the boxes handing something out, as the loader was told.
    offered: Vec<f32>,
    /// Files dropped (kept already), until the drop is told.
    dropped: Vec<PathBuf>,
}

thread_local! {
    static DRAGS: RefCell<Drags> = RefCell::new(Drags::default());
}

/// Has the page take drops, of the kinds the app asks for: from the next run on.
pub(crate) fn hand_drops() {
    add_part(parts::DROPS, Part { after: tell_taken, heard: heard_dropping });
}

/// Has the page drag out what the app hands out: from the next run on.
pub(crate) fn enable_drag_out() {
    add_part(parts::DRAG_OUT, Part { after: hand_out, heard: heard_dragging });
}

/// After a run: tells the loader what the app takes, where it changed.
fn tell_taken(ui: &mut Ui) {
    let taken = (ui.takes_files.get(), ui.crossing.takes_text.get());
    if DRAGS.with_borrow_mut(|drags| std::mem::replace(&mut drags.taken, taken)) != taken {
        // SAFETY: a plain request.
        unsafe { drops_taken(taken.0 as u32, taken.1 as u32) };
    }
}

/// A drag from another program over the page, or dropped on it.
fn heard_dropping(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    let taken = DRAGS.with_borrow(|drags| drags.taken);
    match heard.what {
        dropping::OVER => {
            let files = heard.number == 1.0;
            let takes = if files { taken.0 } else { taken.1 };
            if takes {
                ui.event(if files { Event::FilesHovered(true) } else { Event::TextHovered(true) });
                ui.event(Event::Move(heard.at));
            }
            (Then::Run, takes as u32)
        }
        dropping::LEFT => {
            [Event::FilesHovered(false), Event::TextHovered(false), Event::Leave].into_iter().for_each(|event| _ = ui.event(event));
            (Then::Run, 0)
        }
        dropping::FILE => {
            let named = (heard.number as usize).min(heard.bytes.len());
            let name = String::from_utf8_lossy(&heard.bytes[..named]).into_owned();
            let path = PathBuf::from("/dropped").join(name);
            crate::web_host::keep_file(path.clone(), heard.bytes[named..].to_vec());
            DRAGS.with_borrow_mut(|drags| drags.dropped.push(path));
            (Then::Wait, 0)
        }
        dropping::FILES => {
            let dropped = DRAGS.with_borrow_mut(|drags| std::mem::take(&mut drags.dropped));
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

/// After a run: the loader learns where boxes hand something out; and what the lifted box
/// handed out becomes the drag's items.
fn hand_out(ui: &mut Ui) {
    let places: Vec<f32> = ui.crossing.offers.iter().flat_map(|(_, place)| [place.x, place.y, place.w, place.h]).collect();
    if DRAGS.with_borrow(|drags| drags.offered != places) {
        // SAFETY: four numbers a place, read at once.
        unsafe { drag_offers(places.as_ptr(), places.len() / 4) };
        DRAGS.with_borrow_mut(|drags| drags.offered = places);
    }
    let lifted = std::mem::take(&mut ui.crossing.lift) != 0;
    let Some(what) = ui.take_drag_out() else { return };
    if !lifted {
        return ui.set_drop_effect(None);
    }
    match what {
        DragData::Text(text) => item(item::TEXT, "", text.as_bytes()),
        DragData::Link(link) => item(item::LINK, "", link.as_bytes()),
        DragData::Files(files) => files.iter().for_each(|file| item(item::FILE, &name_of(file), &bytes_of(file))),
        DragData::Picture(width, pixels) => item(item::FILE, "picture.png", &crate::png::encode_rgba(width, pixels).unwrap_or_default()),
    }
}

/// Hands the loader one item of the drag out.
fn item(kind: u32, name: &str, bytes: &[u8]) {
    // SAFETY: the loader reads both at once.
    unsafe { drag_item(kind, name.as_ptr(), name.len(), bytes.as_ptr(), bytes.len()) };
}

/// A file's name, as it goes out.
fn name_of(file: &std::path::Path) -> String {
    file.file_name().map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned())
}

/// A file's bytes: one handed to the page, else one the app keeps in its storage.
fn bytes_of(file: &std::path::Path) -> Vec<u8> {
    crate::web_host::picked(file).or_else(|| crate::storage::read(file).map(String::into_bytes)).unwrap_or_default()
}

/// The app's own drag out, as the browser drags it.
fn heard_dragging(ui: &mut Ui, heard: Heard) -> (Then, u32) {
    match heard.what {
        dragging::START => {
            let offer = ui.crossing.offers.iter().rev().find(|(_, place)| place.contains(heard.at)).map(|(id, _)| *id);
            ui.crossing.lift = offer.unwrap_or(0);
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
