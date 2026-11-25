//! System notifications on macOS and iOS: the user notification centre
//! (`UNUserNotificationCenter`). It serves only app bundles: a program run as a bare executable
//! has no bundle identifier, and the centre will not take it (it ends the program). So outside
//! an app bundle notifications are dropped; inside one they show as every app's do.
//!
//! The first notification loads the framework, asks the person's permission (the system asks
//! once per app), and makes the application's delegate the centre's: it hears clicks, and lets
//! a notification show while the app is in front. A picture goes as an attachment, written to a
//! PNG file the system takes over. (On iOS a click that starts the program comes before any
//! notification was asked for: it is not heard.)
#[cfg(target_os = "ios")]
use crate::ios::{Heard, heard};
#[cfg(target_os = "macos")]
use crate::macos::{Heard, heard};
use crate::macos_sys::*;
use crate::notifications::{NOTICES, Out};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};

/// Permission asked: alerts and sounds.
const ALERTS_AND_SOUNDS: usize = 1 << 2 | 1 << 1;
/// How a notification shows while the app is in front: a banner, in the list, with its sound.
const IN_FRONT: usize = 1 << 4 | 1 << 3 | 1 << 1;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
}

#[link(name = "System")]
unsafe extern "C" {
    /// The class of blocks that live for good (`BLOCK_IS_GLOBAL`).
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

/// A block as the compiler lays one out, holding nothing: what the system calls back.
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: *const Descriptor,
}

static DESCRIBED: Descriptor = Descriptor { reserved: 0, size: size_of::<Block>() };

thread_local! {
    /// The centre, once asked: null outside an app bundle.
    static CENTRE: Cell<Option<Obj>> = const { Cell::new(None) };
}

/// A block calling `invoke` (made to live for good: the system may keep it).
fn block(invoke: *const c_void) -> *const Block {
    let isa = (&raw const _NSConcreteGlobalBlock).cast();
    Box::leak(Box::new(Block { isa, flags: LIVES_ON, reserved: 0, invoke, descriptor: &DESCRIBED }))
}

/// Whether the program is an app bundle: then it has an identifier.
fn bundled() -> bool {
    // SAFETY: plain questions of the main bundle.
    unsafe { !send::<Obj>(send::<Obj>(class(c"NSBundle"), c"mainBundle", ()), c"bundleIdentifier", ()).is_null() }
}

/// The notification centre, made ready the first time; null outside an app bundle.
fn centre() -> Obj {
    if let Some(known) = CENTRE.get() {
        return known;
    }
    let centre = if bundled() { opened() } else { std::ptr::null_mut() };
    CENTRE.set(Some(centre));
    centre
}

/// Loads the framework, and readies its centre: the delegate hears clicks; permission is asked.
fn opened() -> Obj {
    let path = c"/System/Library/Frameworks/UserNotifications.framework/UserNotifications";
    // SAFETY: a system framework loaded (and kept) by its path, lazily (1).
    if unsafe { dlopen(path.as_ptr(), 1) }.is_null() || class(c"UNUserNotificationCenter").is_null() {
        return std::ptr::null_mut();
    }
    let methods: [(&CStr, *const c_void, &str); 2] = [
        (c"userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:", clicked as *const c_void, "v@:@@@?"),
        (c"userNotificationCenter:willPresentNotification:withCompletionHandler:", in_front as *const c_void, "v@:@@@?"),
    ];
    // The application's delegate's class, and the application's.
    #[cfg(target_os = "macos")]
    let (delegate, application) = (c"TautApplication", c"NSApplication");
    #[cfg(target_os = "ios")]
    let (delegate, application) = (c"TautAppDelegate", c"UIApplication");
    add_methods(class(delegate), &methods);
    // SAFETY: the centre lives for the program; the delegate is the application's own, which
    // answers the methods above; the block answers the permission (and ignores it).
    unsafe {
        let centre: Obj = send(class(c"UNUserNotificationCenter"), c"currentNotificationCenter", ());
        let app: Obj = send(class(application), c"sharedApplication", ());
        send::<()>(centre, c"setDelegate:", (send::<Obj>(app, c"delegate", ()),));
        let answered = block(permitted as *const c_void);
        send::<()>(centre, c"requestAuthorizationWithOptions:completionHandler:", (ALERTS_AND_SOUNDS, answered));
        centre
    }
}

/// The person's answer to the permission: the system keeps it.
extern "C" fn permitted(_: *const Block, _: Bool, _: Obj) {}

/// Shows the notifications an app asked for: each with its number as its identifier. None
/// outside an app bundle.
pub(crate) fn show(notices: Vec<Out>) {
    if notices.is_empty() || centre().is_null() {
        return;
    }
    let _pool = Pool::new();
    let centre = centre();
    for out in notices {
        // SAFETY: a content object made, filled and handed to a request, which the centre
        // takes; all autoreleased.
        unsafe {
            let content: Obj = send(made(c"UNMutableNotificationContent"), c"autorelease", ());
            send::<()>(content, c"setTitle:", (ns_string(&out.title),));
            send::<()>(content, c"setBody:", (ns_string(&out.text),));
            if let Some(attached) = out.picture.and_then(|(width, pixels)| attachment(out.number, width, &pixels)) {
                send::<()>(content, c"setAttachments:", (ns_array(&[attached]),));
            }
            let name = ns_string(&out.number.to_string());
            let request: Obj = send(class(c"UNNotificationRequest"), c"requestWithIdentifier:content:trigger:", (name, content, std::ptr::null_mut::<c_void>()));
            send::<()>(centre, c"addNotificationRequest:withCompletionHandler:", (request, std::ptr::null_mut::<c_void>()));
        }
    }
}

/// A picture as an attachment: written as PNG to a file the system then moves into its own
/// store. None where it cannot be written.
fn attachment(number: u32, width: usize, pixels: &[u32]) -> Option<Obj> {
    let png = crate::png::encode_rgba(width, pixels)?;
    let file = std::env::temp_dir().join(format!("taut-notification-{}-{number}.png", std::process::id()));
    std::fs::write(&file, png).ok()?;
    // SAFETY: an address of the file, and an attachment made from it (or nil).
    let attached: Obj = unsafe {
        let address: Obj = send(class(c"NSURL"), c"fileURLWithPath:", (ns_string(&file.to_string_lossy()),));
        send(class(c"UNNotificationAttachment"), c"attachmentWithIdentifier:URL:options:error:", (ns_string(""), address, std::ptr::null_mut::<c_void>(), std::ptr::null_mut::<Obj>()))
    };
    (!attached.is_null()).then_some(attached)
}

/// Calls a block the system handed over, with one number (or none it reads).
///
/// # Safety
/// `done` is a block taking that number.
unsafe fn call(done: *const Block, number: usize) {
    // SAFETY: as the caller promises: a block's function takes the block, then its arguments.
    unsafe {
        let invoke: extern "C" fn(*const Block, usize) = std::mem::transmute((*done).invoke);
        invoke(done, number);
    }
}

/// The person clicked a notification: the app hears which, in its next run.
extern "C" fn clicked(_: Obj, _: Sel, _: Obj, response: Obj, done: *const Block) {
    // SAFETY: a response answers its notification, that its request, that its identifier.
    let named = unsafe { send::<Obj>(send::<Obj>(send::<Obj>(response, c"notification", ()), c"request", ()), c"identifier", ()) };
    if let Some(number) = rust_string(named).and_then(|named| named.parse().ok()) {
        NOTICES.with_borrow_mut(|notices| notices.clicked.push(number));
        #[cfg(target_os = "macos")]
        heard(Heard::Desktop);
        #[cfg(target_os = "ios")]
        heard(Heard::Other(Box::new(|_| true)));
    }
    // SAFETY: the system's block takes nothing (the number is not read).
    if !done.is_null() {
        unsafe { call(done, 0) };
    }
}

/// A notification comes while the app is in front: it shows anyway.
extern "C" fn in_front(_: Obj, _: Sel, _: Obj, _: Obj, done: *const Block) {
    // SAFETY: the system's block takes how to show it.
    if !done.is_null() {
        unsafe { call(done, IN_FRONT) };
    }
}
