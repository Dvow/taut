//! macOS: what an app asks of the desktop beyond its windows. A tray icon is a status item in
//! the menu bar (`NSStatusItem`): its picture, its hint, its menu on a right click (or a click
//! with Control), and a plain click heard as a click. Global hotkeys are the system's own
//! (`RegisterEventHotKey`, which needs no permission), reported through the application's event
//! target. A hidden window is ordered out; a later start of the program (single instance)
//! brings the first window forward.
//!
//! Handed to the windows the first time an app asks for any of it (`platform::hand_desktop`),
//! so a program that never does carries none of it. System notifications are in
//! `macos_notifications`.
use crate::hotkeys::{ALT, CTRL, HOTKEYS, LOGO, Named, Registration, SHIFT};
use crate::macos::{Heard, heard};
use crate::macos_sys::*;
use crate::tray::{Did, HINT, PICTURES, ROWS, RowKind, TRAY};
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};

/// The status item's width: as wide as it is high (`NSSquareStatusItemLength`); its picture's
/// side, in points.
const SQUARE: f64 = -2.0;
const SIDE: f64 = 18.0;

/// Carbon: the keyboard event class, a hotkey pressed, the event's direct parameter as a
/// hotkey id; the library's signature on its hotkeys ("taut").
const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const HOTKEY_PRESSED: u32 = 5;
const DIRECT: u32 = u32::from_be_bytes(*b"----");
const HOTKEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const SIGNED: u32 = u32::from_be_bytes(*b"taut");
/// Carbon's modifier bits: Command, Shift, Option, Control.
const COMMAND_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(target: *mut c_void, handler: Handler, count: usize, types: *const EventTypeSpec, data: *mut c_void, made: *mut *mut c_void) -> i32;
    fn RegisterEventHotKey(code: u32, modifiers: u32, id: EventHotKeyID, target: *mut c_void, options: u32, made: *mut *mut c_void) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn GetEventParameter(event: *mut c_void, name: u32, kind: u32, actual: *mut u32, size: usize, given: *mut usize, data: *mut c_void) -> i32;
}

/// What the desktop part does after each run: carries out the app's requests, given the
/// first window (and its view). Says whether the interface must run again.
pub(crate) type Stood = fn(&mut Ui, Obj, Obj) -> bool;

thread_local! {
    /// What runs after each run once an app asked (see `hand_desktop`).
    pub(crate) static DESKTOP: Cell<Option<Stood>> = const { Cell::new(None) };
    /// The status item, while there is one.
    static ITEM: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    /// Each registered hotkey, by its number.
    static HELD: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new());
    /// The first window is hidden now.
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
}

/// Hands the windows their desktop part, on this thread: the application's delegate learns the
/// status item's actions, and hotkeys are heard.
pub(crate) fn hand_desktop() {
    if DESKTOP.get().is_some() {
        return;
    }
    let methods: [(&CStr, *const c_void, &str); 2] = [(c"trayClicked:", clicked as *const c_void, "v@:@"), (c"trayPicked:", picked as *const c_void, "v@:@")];
    add_methods(class(c"TautApplication"), &methods);
    let kinds = [EventTypeSpec { class: KEYBOARD, kind: HOTKEY_PRESSED }];
    // SAFETY: a handler of Carbon's signature on the application's target, kept for good.
    unsafe { InstallEventHandler(GetApplicationEventTarget(), hotkey_pressed, 1, kinds.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut()) };
    DESKTOP.set(Some(stood));
}

/// After a run: the tray icon follows what the app said; hotkeys are registered or released;
/// notifications show; the first window hides, or comes back, or comes forward for a later
/// start.
fn stood(ui: &mut Ui, window: Obj, _view: Obj) -> bool {
    let (described, changed, hide) = TRAY.with_borrow_mut(|tray| {
        let (described, changed) = tray.settle();
        (described, changed, std::mem::take(&mut tray.hide))
    });
    keep_icon(described, changed);
    let mut news = keep_hotkeys(ui);
    let out = crate::notifications::NOTICES.with_borrow_mut(|notices| std::mem::take(&mut notices.out));
    crate::macos_notifications::show(out);
    // SAFETY: plain requests of a window of the library's, and of the application.
    unsafe {
        if hide && !HIDDEN.replace(true) {
            send::<()>(window, c"orderOut:", (std::ptr::null_mut::<c_void>(),));
            news |= ui.event(crate::event::Event::Minimized(true));
        }
        let back = HIDDEN.get() && ui.take_bring_to_front();
        if crate::single_instance::forward().is_some() || back {
            send::<()>(send::<Obj>(class(c"NSApplication"), c"sharedApplication", ()), c"activateIgnoringOtherApps:", (1 as Bool,));
            send::<()>(window, c"makeKeyAndOrderFront:", (std::ptr::null_mut::<c_void>(),));
            news |= HIDDEN.replace(false) && ui.event(crate::event::Event::Minimized(false));
        }
    }
    news
}

/// The status item made, changed, or taken away, as the app described it this run.
fn keep_icon(described: bool, changed: u8) {
    let _pool = Pool::new();
    let item = ITEM.get();
    // SAFETY: the status bar's own item, kept (retained) while the app describes it.
    unsafe {
        let bar: Obj = send(class(c"NSStatusBar"), c"systemStatusBar", ());
        if !described {
            if !item.is_null() {
                send::<()>(bar, c"removeStatusItem:", (item,));
                send::<()>(item, c"release", ());
                ITEM.set(std::ptr::null_mut());
            }
            return;
        }
        let item = if item.is_null() { made_item(bar) } else { item };
        let button: Obj = send(item, c"button", ());
        TRAY.with_borrow(|tray| {
            if changed & PICTURES != 0 {
                send::<()>(button, c"setImage:", (picture(&tray.pictures),));
            }
            if changed & HINT != 0 {
                send::<()>(button, c"setToolTip:", (ns_string(&tray.hint),));
            }
            if changed & ROWS != 0 {
                send::<()>(button, c"setMenu:", (menu(&tray.rows),));
            }
        });
    }
}

/// A new status item, its button heard by the application's delegate.
///
/// # Safety
/// `bar` is the system status bar.
unsafe fn made_item(bar: Obj) -> Obj {
    // SAFETY: as the caller promises; the item is retained until taken away.
    unsafe {
        let item: Obj = send(send::<Obj>(bar, c"statusItemWithLength:", (SQUARE,)), c"retain", ());
        let button: Obj = send(item, c"button", ());
        let delegate: Obj = send(send::<Obj>(class(c"NSApplication"), c"sharedApplication", ()), c"delegate", ());
        send::<()>(button, c"setTarget:", (delegate,));
        send::<()>(button, c"setAction:", (sel(c"trayClicked:"),));
        ITEM.set(item);
        item
    }
}

/// The icon's picture: the one given that fits two pixels a point best, as an image of the
/// status bar's size (autoreleased; nil for none).
fn picture(pictures: &[(usize, Vec<u32>)]) -> Obj {
    let side = (SIDE * 2.0) as usize;
    let Some(png) = crate::tray::fitting(pictures, side).and_then(|pixels| crate::png::encode_rgba(side, &pixels)) else { return std::ptr::null_mut() };
    // SAFETY: an image read from PNG bytes, sized in points.
    unsafe {
        let image: Obj = send(send::<Obj>(class(c"NSImage"), c"alloc", ()), c"initWithData:", (ns_data(&png),));
        send::<()>(image, c"setSize:", (NSSize { width: SIDE, height: SIDE },));
        send(image, c"autorelease", ())
    }
}

/// The icon's menu, from the rows the app described: each picked one tells its index.
fn menu(rows: &[crate::tray::Row]) -> Obj {
    // SAFETY: a menu and its items, made and handed to the button, which keeps them.
    unsafe {
        let menu: Obj = send(made(c"NSMenu"), c"autorelease", ());
        send::<()>(menu, c"setAutoenablesItems:", (0 as Bool,));
        let delegate: Obj = send(send::<Obj>(class(c"NSApplication"), c"sharedApplication", ()), c"delegate", ());
        for (at, row) in rows.iter().enumerate() {
            let item: Obj = match row.kind {
                RowKind::Line => send(class(c"NSMenuItem"), c"separatorItem", ()),
                _ => send(made(c"NSMenuItem"), c"autorelease", ()),
            };
            send::<()>(menu, c"addItem:", (item,));
            if row.kind == RowKind::Line {
                continue;
            }
            let on = matches!(row.kind, RowKind::Tick(true) | RowKind::Choice(true));
            send::<()>(item, c"setTitle:", (ns_string(&row.label.replace('&', "")),));
            send::<()>(item, c"setTarget:", (delegate,));
            send::<()>(item, c"setAction:", (sel(c"trayPicked:"),));
            send::<()>(item, c"setTag:", (at as isize,));
            send::<()>(item, c"setState:", (isize::from(on),));
            send::<()>(item, c"setEnabled:", (Bool::from(row.enabled),));
        }
        menu
    }
}

/// The icon was clicked.
extern "C" fn clicked(_: Obj, _: Sel, _: Obj) {
    TRAY.with_borrow_mut(|tray| tray.news.push(Did::Clicked));
    heard(Heard::Desktop);
}

/// A row of the icon's menu was picked.
extern "C" fn picked(_: Obj, _: Sel, item: Obj) {
    // SAFETY: a menu item answers its tag.
    let at = unsafe { send::<isize>(item, c"tag", ()) };
    TRAY.with_borrow_mut(|tray| tray.news.push(Did::Picked(at.max(0) as usize)));
    heard(Heard::Desktop);
}

/// A key's macOS key code, where the keyboard has one.
fn key_code(key: Named) -> Option<u32> {
    // Key codes 0 to 31 by the letter or digit they type (a zero where none), then the rest.
    const LETTERS: &[u8; 32] = b"asdfhgzxcv\0bqweryt123465\097\080\0o";
    const MORE: [(u8, u32); 8] = [(b'u', 0x20), (b'i', 0x22), (b'p', 0x23), (b'l', 0x25), (b'j', 0x26), (b'k', 0x28), (b'n', 0x2d), (b'm', 0x2e)];
    const F: [u32; 20] = [0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f, 0x69, 0x6b, 0x71, 0x6a, 0x40, 0x4f, 0x50, 0x5a];
    let code = match key {
        Named::Char(letter) => {
            let letter = letter.to_ascii_lowercase();
            let found = MORE.iter().find(|(of, _)| *of == letter).map(|(_, code)| *code);
            found.or_else(|| LETTERS.iter().position(|of| *of == letter).map(|at| at as u32))?
        }
        Named::F(number) => *F.get(usize::from(number).checked_sub(1)?)?,
        Named::Space => 0x31,
        Named::Enter => 0x24,
        Named::Tab => 0x30,
        Named::Escape => 0x35,
        Named::Backspace => 0x33,
        Named::Delete => 0x75,
        Named::Insert => 0x72,
        Named::Home => 0x73,
        Named::End => 0x77,
        Named::PageUp => 0x74,
        Named::PageDown => 0x79,
        Named::Left => 0x7b,
        Named::Right => 0x7c,
        Named::Up => 0x7e,
        Named::Down => 0x7d,
        _ => return None,
    };
    Some(code)
}

/// The library's modifiers as Carbon's: Ctrl is Command (as in the windows), Super is Control.
fn carbon_modifiers(modifiers: u8) -> u32 {
    let bits = [(CTRL, COMMAND_KEY), (ALT, OPTION_KEY), (SHIFT, SHIFT_KEY), (LOGO, CONTROL_KEY)];
    bits.iter().filter(|(own, _)| modifiers & own != 0).map(|(_, carbon)| carbon).sum()
}

/// After a run: hotkeys the app no longer names are released, new ones registered (or
/// refused, where the keyboard has no such key or another program has it). Says whether one
/// was refused.
fn keep_hotkeys(ui: &mut Ui) -> bool {
    let refused = HOTKEYS.with_borrow_mut(|hotkeys| {
        // SAFETY: each hotkey released is one this registered.
        hotkeys.settle(|wanted| HELD.with_borrow_mut(|held| held.remove(&wanted.number).into_iter().for_each(|hotkey| _ = unsafe { UnregisterEventHotKey(hotkey as *mut c_void) })));
        let mut refused = false;
        for wanted in hotkeys.keys.iter_mut().filter(|wanted| wanted.state == Registration::New) {
            let made = key_code(wanted.combo.key).and_then(|code| registered(code, carbon_modifiers(wanted.combo.modifiers), wanted.number));
            wanted.state = if made.is_some() { Registration::Registered } else { Registration::Refused };
            refused |= made.is_none();
        }
        refused
    });
    refused && {
        ui.rerun();
        true
    }
}

/// Registers one hotkey under its number; the system's handle for it, if it took it.
fn registered(code: u32, modifiers: u32, number: u32) -> Option<usize> {
    let mut made: *mut c_void = std::ptr::null_mut();
    // SAFETY: a hotkey on the application's target, released when the app stops naming it.
    let failed = unsafe { RegisterEventHotKey(code, modifiers, EventHotKeyID { signature: SIGNED, id: number }, GetApplicationEventTarget(), 0, &mut made) };
    (failed == 0 && !made.is_null()).then(|| {
        HELD.with_borrow_mut(|held| held.insert(number, made as usize));
        made as usize
    })
}

/// Carbon reports a hotkey pressed: the next run that asks hears it.
extern "C" fn hotkey_pressed(_: *mut c_void, event: *mut c_void, _: *mut c_void) -> i32 {
    let mut id = EventHotKeyID::default();
    // SAFETY: the event's direct parameter is its hotkey's id, of this size.
    let failed = unsafe { GetEventParameter(event, DIRECT, HOTKEY_ID, std::ptr::null_mut(), size_of::<EventHotKeyID>(), std::ptr::null_mut(), (&raw mut id).cast()) };
    if failed == 0 && id.signature == SIGNED && HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.pressed(id.id)) {
        heard(Heard::Desktop);
    }
    0
}
