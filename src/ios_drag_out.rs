//! Dragging out of a window into another app, on iOS: a `UIDragInteraction` on each window's
//! view. Handed to the window the first time an app hands something out
//! (`platform::enable_drag_out`), so a program that never does carries none of it.
//!
//! The system lifts a drag with a long press. The box under the finger that hands something out
//! ([`Ui::drag_out`]) is then run once more as the one lifted, and what it hands out goes, one
//! item per thing: files as themselves, text as a string, a link as an address, a picture as
//! PNG. The box itself lifts, as its picture. At the end its app hears what the other app did
//! (copied, moved, or nothing).
use crate::drag_drop::{DragData, DropEffect};
use crate::geom::Rect;
use crate::ios::{Heard, heard_in};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};

/// `UIDropOperation`: copied, moved.
const COPY: isize = 2;
const MOVE: isize = 3;

thread_local! {
    /// The items of what the lifted box handed out (retained), until the system takes them;
    /// and where the box is, on the page.
    static ITEMS: RefCell<Vec<Obj>> = const { RefCell::new(Vec::new()) };
    static LIFTED: Cell<Option<Rect>> = const { Cell::new(None) };
    static DELEGATE: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
}

/// Hands the window its drag-out code: each window's view lifts drags from now on.
pub(crate) fn enable() {
    if crate::ios::DRAG_OUT.get().is_some() {
        return;
    }
    crate::ios::DRAG_OUT.set(Some(follow));
    crate::ios::on_every_view(lift_drags);
}

/// Has a view lift drags.
fn lift_drags(view: Obj) {
    // SAFETY: an interaction made for the delegate (made once, kept for good), enabled (an
    // iPhone's are not by themselves), handed to the view, which keeps it.
    unsafe {
        let interaction: Obj = send(send::<Obj>(class(c"UIDragInteraction"), c"alloc", ()), c"initWithDelegate:", (delegate(),));
        send::<()>(interaction, c"setEnabled:", (1 as Bool,));
        send::<()>(view, c"addInteraction:", (interaction,));
        send::<()>(interaction, c"release", ());
    }
}

/// The interactions' delegate, made the first time.
fn delegate() -> Obj {
    if DELEGATE.get().is_null() {
        // SAFETY: an object of the library's class, kept for good.
        DELEGATE.set(unsafe { send(delegate_class(), c"new", ()) });
    }
    DELEGATE.get()
}

/// The delegate's class.
fn delegate_class() -> Obj {
    let methods: [(&CStr, *const c_void, &str); 3] = [
        (c"dragInteraction:itemsForBeginningSession:", items_to_lift as *const c_void, "@@:@@"),
        (c"dragInteraction:previewForLiftingItem:session:", lifted_preview as *const c_void, "@@:@@@"),
        (c"dragInteraction:session:didEndWithOperation:", ended as *const c_void, "v@:@@q"),
    ];
    made_class(c"TautDragOut", c"NSObject", &methods, &[c"UIDragInteractionDelegate"])
}

/// After a run of a window: what the lifted box handed out becomes the drag's items. What leaves
/// the window otherwise (a pointer dragging past its edge) cannot go: the system starts no drag
/// for it.
fn follow(ui: &mut Ui, _: Obj) {
    let lifted = std::mem::take(&mut ui.crossing.lift) != 0;
    let Some(what) = ui.take_drag_out() else { return };
    let items = if lifted { items(what) } else { Vec::new() };
    if items.is_empty() {
        ui.set_drop_effect(None);
    }
    ITEMS.set(items);
}

/// The drag items for what goes out (retained).
fn items(what: DragData) -> Vec<Obj> {
    let providers: Vec<Obj> = match what {
        DragData::Files(files) => files.iter().map(|file| crate::ios_files::file_provider(file)).collect(),
        DragData::Text(text) => vec![provider(c"initWithObject:", ns_string(text))],
        DragData::Link(address) => vec![provider(c"initWithObject:", link(address))],
        DragData::Picture(width, pixels) => crate::png::encode_rgba(width, pixels).map(|png| picture_provider(&png)).into_iter().collect(),
    };
    providers.into_iter().filter(|made| !made.is_null()).map(drag_item).collect()
}

/// An item provider made by `made` from one object (retained; nil for nil: a link that is
/// no address).
fn provider(made: &CStr, from: Obj) -> Obj {
    if from.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: a provider made with one object, as `made` takes it.
    unsafe { send(send::<Obj>(class(c"NSItemProvider"), c"alloc", ()), made, (from,)) }
}

/// A provider of PNG bytes (retained).
fn picture_provider(png: &[u8]) -> Obj {
    // SAFETY: a provider made with the bytes, as PNG.
    unsafe { send(send::<Obj>(class(c"NSItemProvider"), c"alloc", ()), c"initWithItem:typeIdentifier:", (ns_data(png), ns_string("public.png"))) }
}

fn link(address: &str) -> Obj {
    // SAFETY: an address made from text (autoreleased; nil if it is none).
    unsafe { send(class(c"NSURL"), c"URLWithString:", (ns_string(address),)) }
}

/// A drag item for a provider, which it takes over (retained).
fn drag_item(provider: Obj) -> Obj {
    // SAFETY: an item made for the provider, which it keeps; the provider is let go here.
    unsafe {
        let item: Obj = send(send::<Obj>(class(c"UIDragItem"), c"alloc", ()), c"initWithItemProvider:", (provider,));
        send::<()>(provider, c"release", ());
        item
    }
}

/// The system lifts a drag where the finger is: the box there that hands something out runs
/// once as the one lifted, and what it hands out is the drag's. None where no box does.
extern "C" fn items_to_lift(_: Obj, _: Sel, interaction: Obj, session: Obj) -> Obj {
    let view = crate::ios::view_of(interaction);
    let at = crate::ios::place_of(view, session);
    let lifted = crate::ios::with_ui_of(view, |ui| {
        let (id, rect) = ui.crossing.offers.iter().rev().find(|(_, rect)| rect.contains(at)).copied()?;
        ui.crossing.lift = id;
        Some(rect)
    });
    LIFTED.set(lifted.flatten());
    if LIFTED.get().is_some() {
        heard_in(view, Heard::Other(Box::new(|_| true)));
        crate::ios::turn();
    }
    let items = ITEMS.take();
    // SAFETY: the items are handed back autoreleased, in an array.
    items.iter().for_each(|item| _ = unsafe { send::<Obj>(*item, c"autorelease", ()) });
    ns_array(&items)
}

/// What lifts: the box, as a picture of the view where it is.
extern "C" fn lifted_preview(_: Obj, _: Sel, interaction: Obj, _: Obj, _: Obj) -> Obj {
    let (Some(lifted), view) = (LIFTED.get(), crate::ios::view_of(interaction)) else { return std::ptr::null_mut() };
    let origin = crate::ios::origin_of(view);
    let area = NSRect::new(f64::from(lifted.x) + origin.x, f64::from(lifted.y) + origin.y, f64::from(lifted.w), f64::from(lifted.h));
    let middle = NSPoint { x: area.origin.x + area.size.width / 2.0, y: area.origin.y + area.size.height / 2.0 };
    // SAFETY: a picture of part of the view, shown where it was, in a preview handed back
    // autoreleased.
    unsafe {
        let picture: Obj = send(view, c"resizableSnapshotViewFromRect:afterScreenUpdates:withCapInsets:", (area, 0 as Bool, crate::ios::Insets::default()));
        let target: Obj = send(send::<Obj>(class(c"UIDragPreviewTarget"), c"alloc", ()), c"initWithContainer:center:", (view, middle));
        let shape: Obj = send(made(c"UIDragPreviewParameters"), c"autorelease", ());
        let preview: Obj = send(send::<Obj>(class(c"UITargetedDragPreview"), c"alloc", ()), c"initWithView:parameters:target:", (picture, shape, target));
        send::<()>(target, c"release", ());
        send(preview, c"autorelease", ())
    }
}

/// The drag ended: the box's app hears what became of it.
extern "C" fn ended(_: Obj, _: Sel, interaction: Obj, _: Obj, operation: isize) {
    let taken = match operation {
        MOVE => Some(DropEffect::Moved),
        COPY => Some(DropEffect::Copied),
        _ => None,
    };
    LIFTED.set(None);
    heard_in(
        crate::ios::view_of(interaction),
        Heard::Other(Box::new(move |ui: &mut Ui| {
            ui.set_drop_effect(taken);
            true
        })),
    );
}
