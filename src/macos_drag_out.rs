//! Dragging out of a window into another program, on macOS: an AppKit dragging session, the
//! view its source (`NSDraggingSource`). Handed to the windows the first time an app hands
//! something out (`platform::enable_drag_out`), so a program that never does carries none of it.
//!
//! What goes out is one dragging item per thing: files as file addresses, text as a string, a
//! link as an address, a picture as PNG. The session starts from the press that began the drag
//! and follows the pointer until the button comes up. Meanwhile the window's interface hears the
//! pointer while it is over the window, as for a drag inside it; at the end it hears what the
//! other program did (copied, moved, or nothing), and that the button came up, where it was.
use crate::drag_drop::{DragData, DropEffect};
use crate::event::{Button, Event};
use crate::macos::{Heard, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::Cell;
use std::ffi::c_void;

/// Drag operations: copy, link, generic, move.
const COPY: usize = 1;
const LINK: usize = 2;
const GENERIC: usize = 4;
const MOVE: usize = 16;
/// `NSDraggingContextOutsideApplication`.
const OUTSIDE: isize = 0;

/// After a run: starts a session for what a window's interface just handed out, from its view
/// and the press kept for it. Says whether the interface must run again.
pub(crate) type Outward = fn(&mut Ui, Obj, Obj) -> bool;

thread_local! {
    /// What a window does about dragging out: nothing until an app hands something out.
    pub(crate) static OUTWARD: Cell<Option<Outward>> = const { Cell::new(None) };
}

/// Hands the windows their drag-out code, on this thread: the view class learns to be a
/// dragging source.
pub(crate) fn enable() {
    if OUTWARD.get().is_some() {
        return;
    }
    let methods: [(&std::ffi::CStr, *const c_void, String); 3] = [
        (c"draggingSession:sourceOperationMaskForDraggingContext:", allowed as *const c_void, "Q@:@q".into()),
        (c"draggingSession:movedToPoint:", moved as *const c_void, format!("v@:@{POINT}")),
        (c"draggingSession:endedAtPoint:operation:", ended as *const c_void, format!("v@:@{POINT}Q")),
    ];
    let listed: Vec<(&std::ffi::CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    add_methods(crate::macos::view_class(), &listed);
    OUTWARD.set(Some(go_out));
}

/// Starts a session for what `ui` hands out, if anything, from `view` and the press `down`.
fn go_out(ui: &mut Ui, view: Obj, down: Obj) -> bool {
    let at = ui.pointer().unwrap_or_default();
    // (With no press to start from, nothing goes out, and the interface hears so.)
    if down.is_null() {
        let gone = ui.take_drag_out().is_some();
        if gone {
            ui.set_drop_effect(None);
        }
        return gone;
    }
    let Some(what) = ui.take_drag_out() else { return false };
    let _pool = Pool::new();
    let items = items(what, NSPoint { x: f64::from(at.x), y: f64::from(at.y) });
    // SAFETY: the items are dragging items; the press began this drag in this view, which
    // answers the source's methods (see `enable`). AppKit keeps the session.
    unsafe {
        let session: Obj = send(view, c"beginDraggingSessionWithItems:event:source:", (ns_array(&items), down, view));
        send::<()>(session, c"setAnimatesToStartingPositionsOnCancelOrFail:", (1 as Bool,));
    }
    false
}

/// The dragging items for what goes out (autoreleased), each shown at `at` in the view.
pub(crate) fn items(what: DragData, at: NSPoint) -> Vec<Obj> {
    // SAFETY: each writer is an object AppKit can put on a pasteboard: an address, a string, or
    // a pasteboard item holding PNG bytes.
    let writers: Vec<(Obj, Obj)> = unsafe {
        match what {
            DragData::Files(files) => files.iter().map(|file| file_writer(&file.to_string_lossy())).collect(),
            DragData::Text(text) => vec![(ns_string(text), std::ptr::null_mut())],
            DragData::Link(address) => vec![(send(class(c"NSURL"), c"URLWithString:", (ns_string(address),)), std::ptr::null_mut())],
            DragData::Picture(width, pixels) => picture_writer(width, pixels).into_iter().collect(),
        }
    };
    let item = |(writer, picture): (Obj, Obj)| -> Obj {
        // SAFETY: a dragging item made for the writer, autoreleased, shown as the picture (or as
        // nothing) in a square at the pointer.
        unsafe {
            let item: Obj = send(class(c"NSDraggingItem"), c"alloc", ());
            let item: Obj = send(item, c"initWithPasteboardWriter:", (writer,));
            send::<()>(item, c"setDraggingFrame:contents:", (NSRect::new(at.x - 16.0, at.y - 16.0, 32.0, 32.0), picture));
            send(item, c"autorelease", ())
        }
    };
    writers.into_iter().filter(|(writer, _)| !writer.is_null()).map(item).collect()
}

/// A file's address, and its icon as the file browser shows it.
///
/// # Safety
/// On the main thread, or any (both classes allow it).
unsafe fn file_writer(path: &str) -> (Obj, Obj) {
    // SAFETY: plain objects from a path.
    unsafe {
        let address: Obj = send(class(c"NSURL"), c"fileURLWithPath:", (ns_string(path),));
        let workspace: Obj = send(class(c"NSWorkspace"), c"sharedWorkspace", ());
        (address, send(workspace, c"iconForFile:", (ns_string(path),)))
    }
}

/// A picture as a pasteboard item holding PNG, and as an image to show.
///
/// # Safety
/// As `file_writer`.
unsafe fn picture_writer(width: usize, pixels: &[u32]) -> Option<(Obj, Obj)> {
    let png = crate::png::encode_rgba(width, pixels)?;
    // SAFETY: a pasteboard item given the bytes, and an image read from them; both autoreleased.
    unsafe {
        let item: Obj = send(made(c"NSPasteboardItem"), c"autorelease", ());
        send::<Bool>(item, c"setData:forType:", (ns_data(&png), NSPasteboardTypePNG));
        let image: Obj = send(class(c"NSImage"), c"alloc", ());
        let image: Obj = send(send::<Obj>(image, c"initWithData:", (ns_data(&png),)), c"autorelease", ());
        Some((item, image))
    }
}

/// What another program may do with it: copy, move, link (or anything, in this program).
extern "C" fn allowed(_: Obj, _: Sel, _: Obj, context: isize) -> usize {
    if context == OUTSIDE { COPY | MOVE | LINK | GENERIC } else { COPY | MOVE }
}

/// Where a place on the screen is in a view, in units from its top left; none outside it.
fn in_view(view: Obj, on_screen: NSPoint) -> Option<crate::geom::Point> {
    // SAFETY: the place turned from the screen's into the window's, then the (flipped) view's.
    unsafe {
        let window: Obj = send(view, c"window", ());
        let in_window = send_rect(window, c"convertRectFromScreen:", (NSRect::new(on_screen.x, on_screen.y, 0.0, 0.0),)).origin;
        let at: NSPoint = send(view, c"convertPoint:fromView:", (in_window, std::ptr::null_mut::<c_void>()));
        let bounds = send_rect(view, c"bounds", ());
        let inside = at.x >= 0.0 && at.y >= 0.0 && at.x < bounds.size.width && at.y < bounds.size.height;
        inside.then(|| crate::geom::pt(at.x as f32, at.y as f32))
    }
}

/// The pointer moved, dragging: over the window, its interface hears where.
extern "C" fn moved(view: Obj, _: Sel, _: Obj, at: NSPoint) {
    if let Some(at) = in_view(view, at) {
        heard(Heard::Event(view, Event::Move(at)));
    }
}

/// The session ended: the interface hears what became of it, then where the pointer is (if
/// over the window, where it drops as inside), and that the button came up.
extern "C" fn ended(view: Obj, _: Sel, _: Obj, at: NSPoint, operation: usize) {
    let taken = match operation {
        0 => None,
        _ if operation & MOVE != 0 => Some(DropEffect::Moved),
        _ => Some(DropEffect::Copied),
    };
    heard(Heard::Other(
        view,
        Box::new(move |ui: &mut Ui| {
            ui.set_drop_effect(taken);
            true
        }),
    ));
    heard(Heard::Event(view, in_view(view, at).map_or(Event::Leave, Event::Move)));
    heard(Heard::Event(view, Event::Release(Button::Left)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pasteboard types a dragging item's writer offers.
    fn offered(item: Obj) -> Vec<String> {
        // SAFETY: plain questions of an item and its writer.
        let writer: Obj = unsafe { send(item, c"item", ()) };
        let pasteboard: Obj = unsafe { send(class(c"NSPasteboard"), c"generalPasteboard", ()) };
        array_items(unsafe { send(writer, c"writableTypesForPasteboard:", (pasteboard,)) }).into_iter().filter_map(rust_string).collect()
    }

    #[test]
    fn what_goes_out_is_one_item_per_thing_in_the_types_other_programs_read() {
        let _pool = Pool::new();
        let at = NSPoint { x: 40.0, y: 30.0 };
        let files = [std::path::PathBuf::from("/tmp/a.txt"), std::path::PathBuf::from("/tmp/b c.png")];
        let files = items(DragData::Files(&files), at);
        assert_eq!(files.len(), 2);
        assert!(offered(files[1]).iter().any(|kind| kind == "public.file-url"), "{:?}", offered(files[1]));
        // SAFETY: the writer of a file's item is its address.
        let path = rust_string(unsafe { send(send::<Obj>(files[1], c"item", ()), c"path", ()) });
        assert_eq!(path.as_deref(), Some("/tmp/b c.png"));
        let frame = unsafe { send_rect(files[0], c"draggingFrame", ()) };
        assert_eq!((frame.origin.x, frame.origin.y, frame.size.width), (24.0, 14.0, 32.0));
        let text = items(DragData::Text("words"), at);
        assert!(offered(text[0]).iter().any(|kind| kind == "public.utf8-plain-text"), "{:?}", offered(text[0]));
        let link = items(DragData::Link("https://example.org/"), at);
        assert!(offered(link[0]).iter().any(|kind| kind == "public.url"), "{:?}", offered(link[0]));
        let picture = items(DragData::Picture(2, &[0xff10_2030, 0x8040_5060]), at);
        assert_eq!(offered(picture[0]), ["public.png"]);
        // SAFETY: the writer of a picture's item is a pasteboard item with PNG bytes.
        let png = bytes_of(unsafe { send(send::<Obj>(picture[0], c"item", ()), c"dataForType:", (NSPasteboardTypePNG,)) });
        assert_eq!(crate::png::decode(&png), Some((2, vec![0xff10_2030, 0x8040_5060])));
    }

    #[test]
    fn other_programs_may_copy_move_or_link_and_this_one_copy_or_move() {
        assert_eq!(allowed(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), OUTSIDE), COPY | MOVE | LINK | GENERIC);
        assert_eq!(allowed(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), 1), COPY | MOVE);
    }
}
