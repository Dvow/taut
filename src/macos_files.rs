//! macOS: files and text dragged in from other programs, and the system's file dialog. Each is
//! handed to the windows the first time an app asks for it (`platform::hand_drops`,
//! `platform::hand_file_dialog`), so a program that never does carries none of it.
//!
//! Drops: the view class gets AppKit's dragging methods, and each view takes file addresses and
//! plain text. A drag is accepted over a window whose app asked about drops of its kind; the
//! window's interface hears it come, move, leave, and what was let go.
//!
//! The file dialog is `NSOpenPanel` or `NSSavePanel`, shown as a sheet on the window that asked:
//! the windows are used and drawn meanwhile. Its answer comes back through a block, laid out
//! by hand as the compiler lays one out.
use crate::event::Event;
use crate::file_dialog::FileDialogRequest;
use crate::macos::{Heard, file_of, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::Cell;
use std::ffi::c_void;
use std::path::PathBuf;

/// `NSDragOperationCopy`, and none.
const COPY: usize = 1;
const REFUSED: usize = 0;
/// `NSModalResponseOK`.
const CHOSEN: isize = 1;

/// How a view takes drops: it is made to take them; after a run, it notes what its app takes.
#[derive(Clone, Copy)]
pub(crate) struct Drops {
    pub take: fn(Obj),
    pub note: fn(Obj, &Ui),
}

/// Shows the system's file dialog on a window (and its view) for a request.
pub(crate) type Asks = fn(Obj, Obj, FileDialogRequest);

thread_local! {
    /// What a window does about drops: nothing until an app asks (see `hand_drops`). A new
    /// view is made to take drops; after a run, it notes what its app takes.
    pub(crate) static DROPS: Cell<Option<Drops>> = const { Cell::new(None) };
    /// What a window does about the system's file dialog: nothing until an app asks.
    pub(crate) static DIALOG: Cell<Option<Asks>> = const { Cell::new(None) };
    /// What each view's app takes dropped, files and text, as of its last run (view, files,
    /// text).
    static TAKES: std::cell::RefCell<Vec<(Obj, bool, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Hands the windows their drop handling, on this thread: the view class learns AppKit's
/// dragging methods, and every view of it (each made from now on too) takes file addresses
/// and plain text.
pub(crate) fn hand_drops() {
    if DROPS.get().is_some() {
        return;
    }
    let methods: [(&std::ffi::CStr, *const c_void); 4] = [
        (c"draggingEntered:", entered as *const c_void),
        (c"draggingUpdated:", entered as *const c_void),
        (c"draggingExited:", exited as *const c_void),
        (c"performDragOperation:", dropped as *const c_void),
    ];
    let types = ["Q@:@", "Q@:@", "v@:@", &format!("{BOOL}@:@")];
    let listed: Vec<(&std::ffi::CStr, *const c_void, &str)> = methods.iter().zip(types).map(|((name, imp), types)| (*name, *imp, types)).collect();
    add_methods(crate::macos::view_class(), &listed);
    DROPS.set(Some(Drops { take: takes_drops, note: note_takes }));
    crate::macos::each_view(takes_drops);
}

/// A view takes file addresses and plain text dragged over it.
fn takes_drops(view: Obj) {
    // SAFETY: a view of the library's, registered for two pasteboard types.
    unsafe {
        let kinds = ns_array(&[NSPasteboardTypeFileURL, NSPasteboardTypeString]);
        send::<()>(view, c"registerForDraggedTypes:", (kinds,));
    }
}

/// After a run: what a view's app takes dropped.
fn note_takes(view: Obj, ui: &Ui) {
    let now = (view, ui.takes_files.get(), ui.crossing.takes_text.get());
    TAKES.with_borrow_mut(|takes| match takes.iter_mut().find(|(of, ..)| *of == view) {
        Some(kept) => *kept = now,
        None => takes.push(now),
    });
}

/// What a drag carries that the view's app takes: files (true) or text (false). Nothing where
/// it takes neither of what is offered.
fn carried(view: Obj, info: Obj) -> Option<bool> {
    let (files, text) = TAKES.with_borrow(|takes| takes.iter().find(|(of, ..)| *of == view).map(|(_, files, text)| (*files, *text)))?;
    // SAFETY: a drag's pasteboard answers which of these types it has.
    let has = |kind: Obj| unsafe {
        let board: Obj = send(info, c"draggingPasteboard", ());
        !send::<Obj>(board, c"availableTypeFromArray:", (ns_array(&[kind]),)).is_null()
    };
    // SAFETY: the pasteboard types are AppKit's constants.
    let (file_type, text_type) = unsafe { (NSPasteboardTypeFileURL, NSPasteboardTypeString) };
    match () {
        _ if files && has(file_type) => Some(true),
        _ if text && has(text_type) => Some(false),
        _ => None,
    }
}

/// Where a drag is over a view, in units from its top left.
fn place_of(view: Obj, info: Obj) -> crate::geom::Point {
    // SAFETY: the drag's place in the window, turned into the (flipped) view's.
    let at: NSPoint = unsafe { send(view, c"convertPoint:fromView:", (send::<NSPoint>(info, c"draggingLocation", ()), std::ptr::null_mut::<c_void>())) };
    crate::geom::pt(at.x as f32, at.y as f32)
}

/// A drag came over a view, or moved: copied if the app takes what it carries; the interface
/// hears it came, and where.
extern "C" fn entered(view: Obj, name: Sel, info: Obj) -> usize {
    let Some(files) = carried(view, info) else { return REFUSED };
    if name == sel(c"draggingEntered:") {
        heard(Heard::Event(view, if files { Event::FilesHovered(true) } else { Event::TextHovered(true) }));
    }
    heard(Heard::Event(view, Event::Move(place_of(view, info))));
    COPY
}

/// The drag left the view.
extern "C" fn exited(view: Obj, _: Sel, info: Obj) {
    let Some(files) = carried(view, info) else { return };
    heard(Heard::Event(view, if files { Event::FilesHovered(false) } else { Event::TextHovered(false) }));
    heard(Heard::Event(view, Event::Leave));
}

/// The drag was let go over the view: the interface gets the files or the text there.
extern "C" fn dropped(view: Obj, _: Sel, info: Obj) -> Bool {
    let Some(files) = carried(view, info) else { return 0 };
    // SAFETY: the drag's pasteboard and its items, read at once.
    let board: Obj = unsafe { send(info, c"draggingPasteboard", ()) };
    let event = match files {
        true => Event::FilesDropped(
            array_items(unsafe { send(board, c"pasteboardItems", ()) }).into_iter().filter_map(|item| file_of(unsafe { send(item, c"stringForType:", (NSPasteboardTypeFileURL,)) })).collect(),
        ),
        false => Event::TextDropped(rust_string(unsafe { send(board, c"stringForType:", (NSPasteboardTypeString,)) }).unwrap_or_default()),
    };
    heard(Heard::Event(view, if files { Event::FilesHovered(false) } else { Event::TextHovered(false) }));
    if matches!(&event, Event::FilesDropped(files) if files.is_empty()) || matches!(&event, Event::TextDropped(text) if text.is_empty()) {
        return 0;
    }
    heard(Heard::Event(view, Event::Move(place_of(view, info))));
    heard(Heard::Event(view, event));
    1
}

// ---- the system's file dialog ----

/// Hands the windows the system's file dialog, on this thread.
pub(crate) fn hand_file_dialog() {
    DIALOG.set(Some(ask));
}

/// A block's descriptor: its size.
#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

/// A completion block as the compiler lays one out, with what it holds after: the view whose
/// app asked, and the panel.
#[repr(C)]
struct Answering {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*const Answering, isize),
    descriptor: *const Descriptor,
    view: Obj,
    panel: Obj,
}

#[link(name = "System")]
unsafe extern "C" {
    /// The class of blocks that live for good (`BLOCK_IS_GLOBAL`), which copying leaves be.
    static _NSConcreteGlobalBlock: [usize; 0];
}

/// `BLOCK_IS_GLOBAL`.
const LIVES_ON: i32 = 1 << 28;
static DESCRIBED: Descriptor = Descriptor { reserved: 0, size: size_of::<Answering>() };

/// Shows the system's dialog for a request, as a sheet on the window: to open a file, pick a
/// folder, or save. The window's interface hears the answer when the person closes it.
fn ask(window: Obj, view: Obj, request: FileDialogRequest) {
    let _pool = Pool::new();
    // SAFETY: a panel made, set up and shown as a sheet on a window of the library's; it is
    // kept (retained) until answered. The block is made to live for good (a few bytes a
    // dialog): AppKit may touch it after it is called.
    unsafe {
        let panel: Obj = match request.saving {
            true => send(class(c"NSSavePanel"), c"savePanel", ()),
            false => send(class(c"NSOpenPanel"), c"openPanel", ()),
        };
        send::<Obj>(panel, c"retain", ());
        if !request.saving {
            send::<()>(panel, c"setCanChooseFiles:", (Bool::from(!request.folders),));
            send::<()>(panel, c"setCanChooseDirectories:", (Bool::from(request.folders),));
            send::<()>(panel, c"setAllowsMultipleSelection:", (0 as Bool,));
        }
        let folder: Obj = send(class(c"NSURL"), c"fileURLWithPath:isDirectory:", (ns_string(&request.folder.to_string_lossy()), 1 as Bool));
        send::<()>(panel, c"setDirectoryURL:", (folder,));
        send::<()>(panel, c"setMessage:", (ns_string(request.title),));
        if request.saving {
            send::<()>(panel, c"setNameFieldStringValue:", (ns_string(&request.name),));
        }
        let kinds: Vec<Obj> = request.endings.iter().map(|ending| ns_string(ending.rsplit('.').next().unwrap_or_default())).collect();
        if !kinds.is_empty() && !request.folders {
            send::<()>(panel, c"setAllowedFileTypes:", (ns_array(&kinds),));
        }
        let block = Box::leak(Box::new(Answering { isa: (&raw const _NSConcreteGlobalBlock).cast(), flags: LIVES_ON, reserved: 0, invoke: answered, descriptor: &DESCRIBED, view, panel }));
        send::<()>(panel, c"beginSheetModalForWindow:completionHandler:", (window, std::ptr::from_mut(block)));
    }
}

/// The dialog was closed: the window's interface gets the file or folder chosen, or none.
extern "C" fn answered(block: *const Answering, response: isize) {
    // SAFETY: the block is the one `ask` made, which lives for good.
    let Answering { view, panel, .. } = unsafe { &*block };
    let (view, panel) = (*view, *panel);
    // SAFETY: the panel answers its address; it is let go here, once.
    let chosen: Option<PathBuf> = (response == CHOSEN).then(|| rust_string(unsafe { send(send::<Obj>(panel, c"URL", ()), c"path", ()) }).map(PathBuf::from)).flatten();
    unsafe { send::<()>(panel, c"release", ()) };
    heard(Heard::Other(
        view,
        Box::new(move |ui: &mut Ui| {
            ui.file_dialog_answered(chosen);
            true
        }),
    ));
}
