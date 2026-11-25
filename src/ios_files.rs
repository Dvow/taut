//! iOS: the system's file dialog (`UIDocumentPickerViewController`), files, pictures and text
//! dropped on the window (`UIDropInteraction`), files opened with the system
//! (`UIDocumentInteractionController`), and files on the pasteboard. Each is handed to the
//! window the first time an app asks for it, so a program that never does carries none of it.
//!
//! The file dialog opens copies of the files picked (several where asked), or a folder in place;
//! saving moves a new, empty file of the name asked for to where the person puts it, for the app
//! to write. What the person picks stays open to the program while it runs.
//!
//! Dropped files and pictures are copied into the program's temporary folder as they arrive
//! (the system takes its own copies back), and reach the app as files; text comes as text. A
//! drag is taken while the app takes what it carries, as on the desktops.
use crate::event::Event;
use crate::file_dialog::FileDialogRequest;
use crate::geom::{Point, pt};
use crate::ios::{Heard, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// `UIDropOperation`: refused, copied.
const REFUSED: usize = 1;
const COPY: usize = 2;

thread_local! {
    /// Every file the system's dialog answered with last; the file being saved, while asked.
    static PICKED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static SAVING: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// What the app takes dropped, files and text, as of its last run; and what is over the
    /// window now (files or text), if anything.
    static TAKES: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    static OVER: Cell<Option<bool>> = const { Cell::new(None) };
    /// The delegates (made once), the file being opened, and how many folders were made for
    /// files coming in.
    static PICKER: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static DROPS: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static OPENER: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static OPENING: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static FOLDERS: Cell<u32> = const { Cell::new(0) };
}

/// An object of a class of the library's, made the first time and kept in `kept`.
fn made_once(kept: &'static std::thread::LocalKey<Cell<Obj>>, made: fn() -> Obj) -> Obj {
    if kept.get().is_null() {
        // SAFETY: an object of a class of the library's, made and kept for good.
        kept.set(unsafe { send(made(), c"new", ()) });
    }
    kept.get()
}

/// A folder of its own in the program's temporary folder, for files coming in.
fn fresh_folder(kind: &str) -> PathBuf {
    FOLDERS.set(FOLDERS.get() + 1);
    std::env::temp_dir().join(kind).join(FOLDERS.get().to_string())
}

// ---- types of files ----

/// Whether a type (by identifier) is of another.
fn conforms(kind: Obj, to: &str) -> bool {
    // SAFETY: two types made from identifiers (nil for an unknown one, which answers no).
    unsafe {
        let made: Obj = send(class(c"UTType"), c"typeWithIdentifier:", (kind,));
        send::<Bool>(made, c"conformsToType:", (send::<Obj>(class(c"UTType"), c"typeWithIdentifier:", (ns_string(to),)),)) != 0
    }
}

/// The type a provider gives a file as: data, and not text or a link alone unless it names a
/// file; a picture as PNG or JPEG where it can be (pictures the library reads). None for text.
fn file_type(provider: Obj) -> Option<Obj> {
    // SAFETY: plain questions of an item provider.
    let (named, types) = unsafe { (!send::<Obj>(provider, c"suggestedName", ()).is_null(), array_items(send(provider, c"registeredTypeIdentifiers", ()))) };
    let files: Vec<Obj> = types.into_iter().filter(|kind| conforms(*kind, "public.data") && (named || !(conforms(*kind, "public.text") || conforms(*kind, "public.url")))).collect();
    let read = ["public.png", "public.jpeg"].into_iter().find_map(|picture| files.iter().copied().find(|kind| rust_string(*kind).as_deref() == Some(picture)));
    read.or(files.first().copied())
}

/// Names a provider's file as the file it is made from.
pub(crate) fn name_as(provider: Obj, file: &Path) {
    let name = file.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    // SAFETY: a provider takes a name (nil answers nothing).
    unsafe { send::<()>(provider, c"setSuggestedName:", (ns_string(&name),)) };
}

/// The name for a file a provider gives as a type: its own, with the type's ending if it has
/// none.
fn file_name(provider: Obj, kind: Obj) -> String {
    // SAFETY: plain questions of an item provider and a type.
    let (named, ending) = unsafe {
        let ending: Obj = send(send::<Obj>(class(c"UTType"), c"typeWithIdentifier:", (kind,)), c"preferredFilenameExtension", ());
        (rust_string(send(provider, c"suggestedName", ())), rust_string(ending))
    };
    let name = named.filter(|name| !name.is_empty()).unwrap_or_else(|| "file".into());
    match ending {
        Some(ending) if Path::new(&name).extension().is_none() => format!("{name}.{ending}"),
        _ => name,
    }
}

// ---- the system's file dialog ----

/// Hands the window the system's file dialog.
pub(crate) fn hand_file_dialog() {
    crate::ios::FILE_DIALOG.set(Some(ask_if_asked));
}

/// After a run: shows the system's dialog if the app just asked for it.
fn ask_if_asked(ui: &mut Ui) {
    if let Some(asked) = ui.take_file_dialog_request() {
        ask(&asked);
    }
}

/// Every file the system's dialog answered with last (the first was the answer).
pub(crate) fn all_picked() -> Vec<PathBuf> {
    PICKED.take()
}

/// The picker's delegate's class.
fn picker_class() -> Obj {
    let methods: [(&CStr, *const c_void, &str); 2] =
        [(c"documentPicker:didPickDocumentsAtURLs:", picked as *const c_void, "v@:@@"), (c"documentPickerWasCancelled:", cancelled as *const c_void, "v@:@")];
    made_class(c"TautFilePicker", c"NSObject", &methods, &[c"UIDocumentPickerDelegate"])
}

/// Shows the system's dialog over what is in front: to open files or a folder, or to save.
fn ask(request: &FileDialogRequest) {
    let _pool = Pool::new();
    let picker = if request.saving { exporting(request) } else { opening(request) };
    if picker.is_null() {
        return answered(Vec::new());
    }
    // SAFETY: a picker made here, set up and presented by the controller in front, which keeps
    // it; the delegate is kept for good.
    unsafe {
        send::<()>(picker, c"setDelegate:", (made_once(&PICKER, picker_class),));
        send::<()>(picker, c"setAllowsMultipleSelection:", (Bool::from(request.many),));
        if request.folder.is_dir() {
            send::<()>(picker, c"setDirectoryURL:", (file_address(&request.folder),));
        }
        send::<()>(crate::ios::front_controller(), c"presentViewController:animated:completion:", (picker, 1 as Bool, std::ptr::null_mut::<c_void>()));
        send::<()>(picker, c"release", ());
    }
}

/// A file's address (autoreleased).
fn file_address(path: &Path) -> Obj {
    // SAFETY: an address made from a path.
    unsafe { send(class(c"NSURL"), c"fileURLWithPath:", (ns_string(&path.to_string_lossy()),)) }
}

/// A picker opening copies of files of the endings asked for, or a folder in place (retained).
fn opening(request: &FileDialogRequest) -> Obj {
    // SAFETY: types made from identifiers and endings (nil for one the system does not know).
    let named = |identifier: &str| unsafe { send::<Obj>(class(c"UTType"), c"typeWithIdentifier:", (ns_string(identifier),)) };
    let by_ending = |ending: &String| unsafe { send::<Obj>(class(c"UTType"), c"typeWithFilenameExtension:", (ns_string(ending.trim_start_matches('.')),)) };
    let mut types: Vec<Obj> = request.endings.iter().map(by_ending).filter(|kind| !kind.is_null()).collect();
    if request.folders {
        types = vec![named("public.folder")];
    } else if types.is_empty() {
        types.push(named("public.item"));
    }
    // SAFETY: a picker made for the types, as copies unless a folder.
    unsafe {
        let picker: Obj = send(class(c"UIDocumentPickerViewController"), c"alloc", ());
        send(picker, c"initForOpeningContentTypes:asCopy:", (ns_array(&types), Bool::from(!request.folders)))
    }
}

/// A picker saving: a new, empty file of the name asked for (with the first ending asked for, if
/// it has none of them), moved where the person puts it (retained). Null if it cannot be made.
fn exporting(request: &FileDialogRequest) -> Obj {
    let name = if request.name.is_empty() { "Untitled" } else { request.name.as_str() };
    let ended = request.endings.iter().any(|ending| name.to_lowercase().ends_with(ending.as_str()));
    let name = match request.endings.first() {
        Some(ending) if !ended => format!("{name}{ending}"),
        _ => name.to_string(),
    };
    let folder = fresh_folder("taut-save");
    let file = folder.join(name);
    if std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&file, [])).is_err() {
        return std::ptr::null_mut();
    }
    SAVING.set(Some(file.clone()));
    // SAFETY: a picker made to move the file.
    unsafe {
        let picker: Obj = send(class(c"UIDocumentPickerViewController"), c"alloc", ());
        send(picker, c"initForExportingURLs:asCopy:", (ns_array(&[file_address(&file)]), 0 as Bool))
    }
}

/// The person picked: the files (or the folder, or where the saved file went), kept open to
/// the program.
extern "C" fn picked(_: Obj, _: Sel, picker: Obj, addresses: Obj) {
    let opened = |address: Obj| -> Option<PathBuf> {
        // SAFETY: an address the picker gave, opened to the program for good, and its path.
        unsafe {
            send::<Bool>(address, c"startAccessingSecurityScopedResource", ());
            rust_string(send(address, c"path", ())).map(PathBuf::from)
        }
    };
    let files = array_items(addresses).into_iter().filter_map(opened).collect();
    SAVING.set(None);
    put_away(picker);
    answered(files);
}

/// The person closed the dialog: no answer; a file made for saving goes.
extern "C" fn cancelled(_: Obj, _: Sel, picker: Obj) {
    if let Some(file) = SAVING.take() {
        _ = std::fs::remove_file(file);
    }
    put_away(picker);
    answered(Vec::new());
}

/// Puts a picker away, unless it went by itself.
fn put_away(picker: Obj) {
    // SAFETY: a presented controller is dismissed; one presented by nothing is left be.
    unsafe {
        if !send::<Obj>(picker, c"presentingViewController", ()).is_null() {
            send::<()>(picker, c"dismissViewControllerAnimated:completion:", (1 as Bool, std::ptr::null_mut::<c_void>()));
        }
    }
}

/// The app hears the answer in its next run: the first file (none if closed), all of them in
/// `FileDialog::chosen`.
fn answered(files: Vec<PathBuf>) {
    let first = files.first().cloned();
    PICKED.set(files);
    heard(Heard::Other(Box::new(move |ui: &mut Ui| {
        ui.file_dialog_answered(first);
        true
    })));
}

// ---- drops ----

/// Hands the window its drop handling: the view takes drops from now on.
pub(crate) fn hand_drops() {
    if crate::ios::DROPS.get().is_some() {
        return;
    }
    crate::ios::DROPS.set(Some(note_takes));
    // SAFETY: an interaction made for the drop delegate (kept for good), handed to the
    // library's view, which keeps it.
    unsafe {
        let interaction: Obj = send(send::<Obj>(class(c"UIDropInteraction"), c"alloc", ()), c"initWithDelegate:", (made_once(&DROPS, drops_class),));
        send::<()>(crate::ios::view(), c"addInteraction:", (interaction,));
        send::<()>(interaction, c"release", ());
    }
}

/// After a run: what the app takes dropped.
fn note_takes(ui: &mut Ui) {
    TAKES.set((ui.takes_files.get(), ui.crossing.takes_text.get()));
}

/// The drop delegate's class.
fn drops_class() -> Obj {
    let methods: [(&CStr, *const c_void, String); 6] = [
        (c"dropInteraction:canHandleSession:", can_take as *const c_void, format!("{BOOL}@:@@")),
        (c"dropInteraction:sessionDidEnter:", entered as *const c_void, "v@:@@".into()),
        (c"dropInteraction:sessionDidUpdate:", moved as *const c_void, "@@:@@".into()),
        (c"dropInteraction:sessionDidExit:", exited as *const c_void, "v@:@@".into()),
        (c"dropInteraction:performDrop:", dropped as *const c_void, "v@:@@".into()),
        (c"dropInteraction:sessionDidEnd:", exited as *const c_void, "v@:@@".into()),
    ];
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautDrops", c"NSObject", &listed, &[c"UIDropInteractionDelegate"])
}

/// The item providers of a drop session's items.
fn providers(session: Obj) -> Vec<Obj> {
    // SAFETY: a session answers its items, each its provider.
    array_items(unsafe { send(session, c"items", ()) }).into_iter().map(|item| unsafe { send(item, c"itemProvider", ()) }).collect()
}

/// Whether a provider gives text.
fn gives_text(provider: Obj) -> bool {
    // SAFETY: a plain question of a provider.
    unsafe { send::<Bool>(provider, c"canLoadObjectOfClass:", (class(c"NSString"),)) != 0 }
}

/// What a drag carries that the app takes: files (true) or text (false). Nothing where it
/// takes neither of what is offered.
fn carried(session: Obj) -> Option<bool> {
    let (files, text) = TAKES.get();
    let providers = providers(session);
    match () {
        _ if files && providers.iter().any(|provider| file_type(*provider).is_some()) => Some(true),
        _ if text && providers.iter().any(|provider| gives_text(*provider)) => Some(false),
        _ => None,
    }
}

/// Where a drag is over the page.
fn place_of(session: Obj) -> Point {
    // SAFETY: a session answers its place in a view.
    let at: NSPoint = unsafe { send(session, c"locationInView:", (crate::ios::view(),)) };
    let origin = crate::ios::origin();
    pt((at.x - origin.x) as f32, (at.y - origin.y) as f32)
}

/// The interface hears files or text come over the window, or go.
fn hover(what: bool, over: bool) {
    OVER.set(over.then_some(what));
    heard(Heard::Event(if what { Event::FilesHovered(over) } else { Event::TextHovered(over) }));
}

extern "C" fn can_take(_: Obj, _: Sel, _: Obj, session: Obj) -> Bool {
    Bool::from(carried(session).is_some())
}

/// A drag came over the window: the interface hears it, and where.
extern "C" fn entered(_: Obj, _: Sel, _: Obj, session: Obj) {
    if let Some(files) = carried(session) {
        hover(files, true);
        heard(Heard::Event(Event::Move(place_of(session))));
    }
}

/// The drag moved: copied if the app takes what it carries.
extern "C" fn moved(_: Obj, _: Sel, _: Obj, session: Obj) -> Obj {
    let taken = carried(session).is_some();
    if taken {
        heard(Heard::Event(Event::Move(place_of(session))));
    }
    // SAFETY: a proposal made and handed back autoreleased.
    unsafe {
        let proposal: Obj = send(class(c"UIDropProposal"), c"alloc", ());
        let proposal: Obj = send(proposal, c"initWithDropOperation:", (if taken { COPY } else { REFUSED },));
        send(proposal, c"autorelease", ())
    }
}

/// The drag left the window, or ended over it without a drop.
extern "C" fn exited(_: Obj, _: Sel, _: Obj, _: Obj) {
    if let Some(files) = OVER.get() {
        hover(files, false);
        heard(Heard::Event(Event::Leave));
    }
}

/// The drag was let go over the window: what it carries comes, then the interface gets it
/// where it was let go.
extern "C" fn dropped(_: Obj, _: Sel, _: Obj, session: Obj) {
    let Some(files) = carried(session) else { return };
    hover(files, false);
    let at = place_of(session);
    if files {
        land_files(&providers(session), at);
    } else {
        land_text(&providers(session), at);
    }
}

/// What comes from a drop as it arrives, on threads of the system's: each item's (none for
/// one that failed), how many are still to come, and where the drop was.
struct Landing<T> {
    arrived: Vec<Option<T>>,
    left: usize,
    at: Point,
    /// What the app gets once all have come.
    deliver: fn(Vec<T>) -> Event,
}

/// One item's part of a landing, held by the block the system calls when it arrives.
struct Arriving<T> {
    landing: Arc<Mutex<Landing<T>>>,
    nth: usize,
}

/// An item arrived: once all have, the interface gets them where the drop was (and the drag
/// is gone after).
fn arrive<T: Send + 'static>(arriving: Arriving<T>, value: Option<T>) {
    let mut landing = arriving.landing.lock().unwrap_or_else(PoisonError::into_inner);
    landing.arrived[arriving.nth] = value;
    landing.left -= 1;
    if landing.left > 0 {
        return;
    }
    let (at, deliver, arrived) = (landing.at, landing.deliver, std::mem::take(&mut landing.arrived));
    crate::ios::on_main(Box::new(move || {
        heard(Heard::Event(Event::Move(at)));
        heard(Heard::Event(deliver(arrived.into_iter().flatten().collect())));
        crate::ios::on_main(Box::new(|| heard(Heard::Event(Event::Leave))));
    }));
}

/// Starts a landing of `count` items.
fn landing<T>(count: usize, at: Point, deliver: fn(Vec<T>) -> Event) -> Arc<Mutex<Landing<T>>> {
    Arc::new(Mutex::new(Landing { arrived: (0..count).map(|_| None).collect(), left: count, at, deliver }))
}

/// Files and pictures dropped: each copied into a folder of the program's as it arrives.
fn land_files(providers: &[Obj], at: Point) {
    let files: Vec<(Obj, Obj)> = providers.iter().filter_map(|provider| Some((*provider, file_type(*provider)?))).collect();
    let (landing, folder) = (landing(files.len(), at, Event::FilesDropped), fresh_folder("taut-drops"));
    for (nth, (provider, kind)) in files.into_iter().enumerate() {
        let into = folder.join(file_name(provider, kind));
        let held = (Arriving { landing: landing.clone(), nth }, into);
        // SAFETY: the provider writes the file and calls the block, which lives for good.
        unsafe { send::<Obj>(provider, c"loadFileRepresentationForTypeIdentifier:completionHandler:", (kind, block(file_arrived as *const c_void, held))) };
    }
}

/// Text dropped: each item's, a line each.
fn land_text(providers: &[Obj], at: Point) {
    let texts: Vec<Obj> = providers.iter().copied().filter(|provider| gives_text(*provider)).collect();
    let landing = landing(texts.len(), at, |texts: Vec<String>| Event::TextDropped(texts.join("\n")));
    for (nth, provider) in texts.into_iter().enumerate() {
        let held = Arriving { landing: landing.clone(), nth };
        // SAFETY: the provider reads the text and calls the block, which lives for good.
        unsafe { send::<Obj>(provider, c"loadObjectOfClass:completionHandler:", (class(c"NSString"), block(text_arrived as *const c_void, held))) };
    }
}

/// A dropped file arrived, in a temporary place the system takes back after: it is copied where
/// it was to go.
extern "C" fn file_arrived(block: *const Block<(Arriving<PathBuf>, PathBuf)>, address: Obj, _: Obj) {
    let _pool = Pool::new();
    // SAFETY: the block `land_files` made, called once; the address is a file's (or nil).
    let Some((arriving, into)) = (unsafe { (*block).held.take() }) else { return };
    let from = rust_string(unsafe { send(address, c"path", ()) });
    let copied = from.and_then(|from| copy_in(Path::new(&from), &into));
    arrive(arriving, copied);
}

/// Copies a file to `into`, making its folder.
fn copy_in(from: &Path, into: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(into.parent()?).ok()?;
    std::fs::copy(from, into).ok()?;
    Some(into.to_path_buf())
}

/// Dropped text arrived.
extern "C" fn text_arrived(block: *const Block<Arriving<String>>, text: Obj, _: Obj) {
    let _pool = Pool::new();
    // SAFETY: the block `land_text` made, called once; the object is a string (or nil).
    let Some(arriving) = (unsafe { (*block).held.take() }) else { return };
    arrive(arriving, rust_string(text));
}

// ---- completion blocks ----

#[link(name = "System")]
unsafe extern "C" {
    /// The class of blocks that live for good (`BLOCK_IS_GLOBAL`), which copying leaves be.
    static _NSConcreteGlobalBlock: [usize; 0];
}

/// `BLOCK_IS_GLOBAL`.
const LIVES_ON: i32 = 1 << 28;

/// A block's descriptor: its size.
#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

/// A block as the compiler lays one out, holding what its function takes once called.
#[repr(C)]
struct Block<T> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: *const Descriptor,
    held: Cell<Option<T>>,
}

/// A block calling `invoke` with what it holds. It lives for good (a few bytes each), as the
/// system may touch it after calling it.
fn block<T>(invoke: *const c_void, held: T) -> *const Block<T> {
    let descriptor = Box::leak(Box::new(Descriptor { reserved: 0, size: size_of::<Block<T>>() }));
    let isa = (&raw const _NSConcreteGlobalBlock).cast();
    Box::leak(Box::new(Block { isa, flags: LIVES_ON, reserved: 0, invoke, descriptor, held: Cell::new(Some(held)) }))
}

// ---- opening files ----

/// Opens a file as the system does: shown in its preview, or else offered to the apps that open
/// it. Says it was handed on.
pub(crate) fn open_file(path: &Path) -> bool {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    crate::ios::on_main(Box::new(move || show_file(&path)));
    true
}

/// The opener's delegate's class: the preview is shown by the controller in front.
fn opener_class() -> Obj {
    let methods: [(&CStr, *const c_void, &str); 1] = [(c"documentInteractionControllerViewControllerForPreview:", previewed_by as *const c_void, "@@:@")];
    made_class(c"TautFileOpener", c"NSObject", &methods, &[c"UIDocumentInteractionControllerDelegate"])
}

extern "C" fn previewed_by(_: Obj, _: Sel, _: Obj) -> Obj {
    crate::ios::front_controller()
}

/// Shows a file in the system's preview; where it has none, the apps that open it.
fn show_file(path: &Path) {
    let view = crate::ios::view();
    // SAFETY: an interaction controller for the file, kept (retained) until the next file; the
    // delegate is kept for good.
    unsafe {
        let opener: Obj = send(class(c"UIDocumentInteractionController"), c"interactionControllerWithURL:", (file_address(path),));
        send::<Obj>(opener, c"retain", ());
        send::<()>(OPENING.replace(opener), c"release", ());
        send::<()>(opener, c"setDelegate:", (made_once(&OPENER, opener_class),));
        if send::<Bool>(opener, c"presentPreviewAnimated:", (1 as Bool,)) == 0 {
            let bounds = send_rect(view, c"bounds", ());
            let middle = NSRect::new(bounds.size.width / 2.0, bounds.size.height / 2.0, 1.0, 1.0);
            send::<Bool>(opener, c"presentOptionsMenuFromRect:inView:animated:", (middle, view, 1 as Bool));
        }
    }
}

// ---- files on the pasteboard ----

/// The files on the pasteboard, each written into a folder of the program's: what another app
/// copied as files, pictures among them.
pub(crate) fn pasted_files(board: Obj) -> Vec<PathBuf> {
    let folder = fresh_folder("taut-pasted");
    // SAFETY: a plain question of the pasteboard.
    let providers = array_items(unsafe { send(board, c"itemProviders", ()) });
    providers.into_iter().enumerate().filter_map(|(nth, provider)| pasted_file(board, provider, nth, &folder)).collect()
}

/// The pasteboard's `nth` item as a file in `folder`, if it is one.
fn pasted_file(board: Obj, provider: Obj, nth: usize, folder: &Path) -> Option<PathBuf> {
    let kind = file_type(provider)?;
    // SAFETY: the item's data of its type, read from the pasteboard.
    let data = unsafe {
        let item: Obj = send(class(c"NSIndexSet"), c"indexSetWithIndex:", (nth,));
        array_items(send(board, c"dataForPasteboardType:inItemSet:", (kind, item))).first().copied()?
    };
    let file = folder.join(file_name(provider, kind));
    std::fs::create_dir_all(folder).ok()?;
    std::fs::write(&file, bytes_of(data)).ok()?;
    Some(file)
}

/// Puts files on the pasteboard, each as itself, for another app to paste.
pub(crate) fn put_files(board: Obj, files: &[PathBuf]) {
    let provider = |file: &PathBuf| -> Obj {
        // SAFETY: a provider of the file (nil if it cannot be read), autoreleased.
        let made: Obj = unsafe { send(send::<Obj>(send::<Obj>(class(c"NSItemProvider"), c"alloc", ()), c"initWithContentsOfURL:", (file_address(file),)), c"autorelease", ()) };
        name_as(made, file);
        made
    };
    let providers: Vec<Obj> = files.iter().map(provider).filter(|made| !made.is_null()).collect();
    // SAFETY: the pasteboard takes the providers, in place of what it held.
    unsafe { send::<()>(board, c"setItemProviders:", (ns_array(&providers),)) };
}
