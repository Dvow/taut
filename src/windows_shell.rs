//! The Windows shell beyond the program's windows: its notification area icon (`tray`),
//! notifications (`notifications`), global hotkeys (`hotkeys`), and later launches
//! (`single_instance`). All go through a hidden window of the program's own, never shown: the
//! shell reports icon and notification interaction to it, the system reports hotkeys, and a
//! later launch hands it its arguments. It is a top-level window, not a message-only one: the
//! taskbar broadcasts `TaskbarCreated` (after Explorer restarts) only to those, and the icon is
//! then re-added.
//!
//! It is created on first need, on the windows' thread: by the first run that asks for any of
//! this, or by `taut::single_instance`. After every run the first window applies what the app
//! said (`after`): the icon is added, modified or deleted, requested notifications shown, and
//! hotkeys registered and unregistered. When the first window goes, so does the icon (`ended`).
//!
//! The icon uses shell version 4 (Vista on): a click, or Enter on the icon, is one message
//! (`NIN_SELECT`), and the context menu is one message whether by mouse or keyboard
//! (`WM_CONTEXTMENU`). The menu is a system menu built from the app's last rows when requested.
//! A notification is an icon balloon (`NIF_INFO`), which Windows 10 and 11 show as toasts: from
//! the app's tray icon where it has one, else from one lent while the notification shows, as
//! Windows requires an icon.
use crate::hotkeys::{ALT, CTRL, HOTKEYS, LOGO, Registration, SHIFT};
use crate::notifications::NOTICES;
use crate::single_instance::{self, PATIENCE};
use crate::tray::{self, Did, HINT, PICTURES, Row, RowKind, TRAY};
use crate::windows::sys::{CreateWindowExW, DefWindowProcW, GetModuleHandleW, GetSystemMetrics, HANDLE, PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowTextW, WNDCLASSW};
use crate::windows::{icon_made, wide};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::time::{Duration, Instant};

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterHotKey(hwnd: HANDLE, id: i32, held: u32, key: u32) -> i32;
    fn UnregisterHotKey(hwnd: HANDLE, id: i32) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> HANDLE;
    fn SendMessageTimeoutW(hwnd: HANDLE, msg: u32, w: usize, l: isize, how: u32, millis: u32, result: *mut usize) -> isize;
    fn AllowSetForegroundWindow(process: u32) -> i32;
    fn GetWindowThreadProcessId(hwnd: HANDLE, process: *mut u32) -> u32;
    fn CreatePopupMenu() -> HANDLE;
    fn AppendMenuW(menu: HANDLE, flags: u32, id: usize, text: *const u16) -> i32;
    fn CheckMenuRadioItem(menu: HANDLE, first: u32, last: u32, check: u32, how: u32) -> i32;
    fn TrackPopupMenuEx(menu: HANDLE, flags: u32, x: i32, y: i32, hwnd: HANDLE, more: *const c_void) -> i32;
    fn DestroyMenu(menu: HANDLE) -> i32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn ChangeWindowMessageFilterEx(hwnd: HANDLE, msg: u32, how: u32, more: *mut c_void) -> i32;
    fn DestroyIcon(icon: HANDLE) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(what: u32, data: *const IconData) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(security: *const c_void, owned: i32, name: *const u16) -> HANDLE;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: usize) -> i32;
}

/// An icon's data for the shell, laid out as shellapi.h's `NOTIFYICONDATAW`.
#[repr(C)]
pub(crate) struct IconData {
    size: u32,
    hwnd: HANDLE,
    id: u32,
    flags: u32,
    callback: u32,
    icon: HANDLE,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u32; 4],
    balloon_icon: HANDLE,
}

/// `NIM_ADD`, `NIM_MODIFY`, `NIM_DELETE`, `NIM_SETVERSION`.
const ADD: u32 = 0;
const MODIFY: u32 = 1;
const DELETE: u32 = 2;
const SET_VERSION: u32 = 4;
/// `NIF_MESSAGE`, `NIF_ICON`, `NIF_TIP`, `NIF_INFO`, and `NIF_SHOWTIP` (which version 4
/// otherwise leaves to the program).
const MESSAGE: u32 = 0x01;
const PICTURE: u32 = 0x02;
const TIP: u32 = 0x04;
const INFO: u32 = 0x10;
const SHOW_TIP: u32 = 0x80;
/// `NIIF_USER | NIIF_LARGE_ICON`: the notification uses the program's own large icon; and
/// `NIIF_RESPECT_QUIET_TIME`, quiet in a new account's first hour, as Windows asks.
const OWN_PICTURE: u32 = 0x04;
const LARGE: u32 = 0x20;
const QUIET_TIME: u32 = 0x80;
/// The icon's callback events, in the low word of `lParam`: `NIN_SELECT` (click or Enter),
/// `NIN_KEYSELECT`, `NIN_BALLOONSHOW`, `NIN_BALLOONHIDE`, `NIN_BALLOONTIMEOUT`,
/// `NIN_BALLOONUSERCLICK`; `WM_CONTEXTMENU`; `WM_LBUTTONDBLCLK`.
const SELECT: u32 = 0x400;
const KEY_SELECT: u32 = 0x401;
const BALLOON_HIDE: u32 = 0x403;
const BALLOON_TIMEOUT: u32 = 0x404;
const BALLOON_CLICK: u32 = 0x405;
const CONTEXT_MENU: u32 = 0x007B;
const DOUBLE_CLICK: u32 = 0x0203;
/// The program's icon ids: its own tray icon, and the one lent to a notification while it has
/// none in the tray.
const OWN: u32 = 1;
const LENT: u32 = 2;
/// The callback message the icons send the hidden window; a hotkey; a forwarded launch.
const CALLBACK: u32 = 0x8010;
const WM_HOTKEY: u32 = 0x0312;
const WM_COPYDATA: u32 = 0x004A;
/// The `dwData` tag of a launch this library forwards ("taut").
const STARTS: usize = 0x7461_7574;
/// The hidden windows' class name.
const CLASS: &str = "taut desktop";

/// The hidden window and the icons' state, on this thread.
#[derive(Default)]
struct Shell {
    hwnd: HANDLE,
    /// The program's icon handle while in the tray; whether the shell accepted it, or refused
    /// it (then not retried until `TaskbarCreated`); and whether an icon is lent to a
    /// notification, with its image.
    icon: HANDLE,
    added: bool,
    refused: bool,
    lending: bool,
    lent: HANDLE,
    /// The notification each of the two icons showed last (0 for none).
    showing: [u32; 2],
}

thread_local! {
    static SHELL: RefCell<Shell> = RefCell::new(Shell::default());
    /// The `TaskbarCreated` message number.
    static ANEW: Cell<u32> = const { Cell::new(0) };
    /// This is the program's single running instance (`single_instance`): the hidden window
    /// accepts later launches' arguments. (Any other accepts none: they would come from no
    /// launch of its own.)
    static FIRST: Cell<bool> = const { Cell::new(false) };
}

/// What the first window should do after a run, for the app's desktop requests: hide, and come
/// forward (a later launch arrived).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Asked {
    pub hide: bool,
    pub forward: bool,
}

/// The hidden window, created on first request. (0 where Windows creates none.)
fn window() -> HANDLE {
    let made = SHELL.with_borrow(|shell| shell.hwnd);
    if made != 0 {
        return made;
    }
    let class = wide(CLASS);
    // SAFETY: the hidden window class is registered and one window created: drawing nothing,
    // without a taskbar button (`WS_EX_TOOLWINDOW`, 0x80), never shown. `TaskbarCreated` may
    // come from a lower-integrity process (Explorer, when this runs elevated: `MSGFLT_ALLOW`,
    // 1). A forwarded launch is not let through so: a less privileged program must not tell a
    // more privileged one what to open. Every string is NUL-terminated.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        RegisterClassW(&WNDCLASSW { style: 0, proc: Some(heard), class_extra: 0, window_extra: 0, instance, icon: 0, cursor: 0, background: 0, menu: std::ptr::null(), name: class.as_ptr() });
        let hwnd = CreateWindowExW(0x80, class.as_ptr(), class.as_ptr(), 0x8000_0000, 0, 0, 0, 0, 0, 0, instance, std::ptr::null_mut());
        ANEW.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
        ChangeWindowMessageFilterEx(hwnd, ANEW.get(), 1, std::ptr::null_mut());
        hwnd
    };
    SHELL.with_borrow_mut(|shell| shell.hwnd = hwnd);
    hwnd
}

/// After a run: the app's desktop requests go to Windows, returning what the first window
/// should do. (Nothing on this thread is borrowed while Windows is called: a shell call waits
/// for its answer, and the windows receive messages meanwhile.)
pub(crate) fn after() -> Asked {
    let hwnd = window();
    let (described, changed, hide) = TRAY.with_borrow_mut(|tray| {
        let (described, changed) = tray.settle();
        (described, changed, std::mem::take(&mut tray.hide))
    });
    let (added, refused) = SHELL.with_borrow(|shell| (shell.added, shell.refused));
    match (described, added) {
        (true, false) if !refused => put_in(hwnd),
        (true, true) if changed & (PICTURES | HINT) != 0 => change(hwnd),
        (false, true) => take_out(hwnd, OWN),
        _ => {}
    }
    let out = NOTICES.with_borrow_mut(|notices| {
        notices.clicked.clear();
        std::mem::take(&mut notices.out)
    });
    out.into_iter().for_each(|out| show(hwnd, out));
    // SAFETY: this window's hotkeys are unregistered and registered; neither sends a message.
    // (A refused key has the app run again, to hear of it.)
    let refused = HOTKEYS.with_borrow_mut(|hotkeys| {
        hotkeys.settle(|wanted| _ = (wanted.state == Registration::Registered).then(|| unsafe { UnregisterHotKey(hwnd, wanted.number as i32) }));
        let mut refused = false;
        for wanted in hotkeys.keys.iter_mut().filter(|wanted| wanted.state == Registration::New) {
            let (held, key) = (held_bits(wanted.combo.modifiers), wanted.combo.key.virtual_key());
            let had = unsafe { RegisterHotKey(hwnd, wanted.number as i32, held, key) } != 0;
            wanted.state = if had { Registration::Registered } else { Registration::Refused };
            refused |= !had;
        }
        refused
    });
    if refused {
        crate::platform::Waker.wake();
    }
    Asked { hide, forward: single_instance::forward().is_some() }
}

/// The first window is gone, and the program with it: its icons leave the tray (else they would
/// linger until hovered), and so does the hidden window, so no later launch hands anything to
/// an ending program.
pub(crate) fn ended() {
    let hwnd = SHELL.with_borrow(|shell| shell.hwnd);
    if hwnd == 0 {
        return;
    }
    take_out(hwnd, OWN);
    take_out(hwnd, LENT);
    // SAFETY: the hidden window is this thread's, and nothing uses it afterwards.
    unsafe { crate::windows::sys::DestroyWindow(hwnd) };
    SHELL.with_borrow_mut(|shell| shell.hwnd = 0);
}

/// An icon's base data: its id, its window, and its callback message; nothing else set yet.
pub(crate) fn icon_data(hwnd: HANDLE, id: u32) -> IconData {
    IconData {
        size: size_of::<IconData>() as u32,
        hwnd,
        id,
        flags: MESSAGE,
        callback: CALLBACK,
        icon: 0,
        tip: [0; 128],
        state: 0,
        state_mask: 0,
        info: [0; 256],
        version: 0,
        info_title: [0; 64],
        info_flags: 0,
        guid: [0; 4],
        balloon_icon: 0,
    }
}

/// Writes a string into room for so many UTF-16 units, NUL-terminated: truncated where room
/// ends (at a whole character).
pub(crate) fn written(room: &mut [u16], text: &str) {
    let mut at = 0;
    for letter in text.chars() {
        let mut units = [0u16; 2];
        let units = letter.encode_utf16(&mut units);
        if at + units.len() >= room.len() {
            break;
        }
        room[at..at + units.len()].copy_from_slice(units);
        at += units.len();
    }
    room[at..].fill(0);
}

/// A hotkey's modifiers as Windows has them: `MOD_ALT` 1, `MOD_CONTROL` 2, `MOD_SHIFT` 4,
/// `MOD_WIN` 8; and `MOD_NOREPEAT`, 0x4000.
pub(crate) fn held_bits(held: u8) -> u32 {
    [(ALT, 1), (CTRL, 2), (SHIFT, 4), (LOGO, 8)].iter().fold(0x4000, |bits, (ours, theirs)| if held & ours != 0 { bits | theirs } else { bits })
}

/// The program's icon as the app last set it, at the tray's icon size; and its shell data, with
/// its tooltip. (Its image is created afresh each time: the caller destroys the previous one.)
fn tray_icon(hwnd: HANDLE) -> (IconData, HANDLE) {
    let side = unsafe { GetSystemMetrics(49) }.max(1) as usize;
    let (picture, hint) = TRAY.with_borrow(|tray| (tray::fitting(&tray.pictures, side), tray.hint.clone()));
    let icon = picture.map_or(0, |pixels| icon_made(side, &pixels));
    let mut data = icon_data(hwnd, OWN);
    (data.flags, data.icon) = (MESSAGE | PICTURE | TIP | SHOW_TIP, icon);
    written(&mut data.tip, &hint);
    (data, icon)
}

/// Adds the program's icon to the tray, as version 4. (Refused, the taskbar is missing: it is
/// added on `TaskbarCreated`.)
fn put_in(hwnd: HANDLE) {
    let (mut data, icon) = tray_icon(hwnd);
    // SAFETY: the data is laid out as the shell reads it, and outlives the calls.
    let added = unsafe { Shell_NotifyIconW(ADD, &data) } != 0;
    data.version = 4;
    if added {
        unsafe { Shell_NotifyIconW(SET_VERSION, &data) };
    }
    let was = SHELL.with_borrow_mut(|shell| {
        (shell.added, shell.refused) = (added, !added);
        std::mem::replace(&mut shell.icon, icon)
    });
    give_back(was);
}

/// Updates the icon's image and tooltip.
fn change(hwnd: HANDLE) {
    let (data, icon) = tray_icon(hwnd);
    // SAFETY: as in `put_in`.
    unsafe { Shell_NotifyIconW(MODIFY, &data) };
    give_back(SHELL.with_borrow_mut(|shell| std::mem::replace(&mut shell.icon, icon)));
}

/// Removes one of the program's icons from the tray, where present.
fn take_out(hwnd: HANDLE, id: u32) {
    let was = SHELL.with_borrow_mut(|shell| match id {
        OWN if shell.added => {
            (shell.added, shell.showing[0]) = (false, 0);
            Some(std::mem::take(&mut shell.icon))
        }
        LENT if shell.lending => {
            (shell.lending, shell.showing[1]) = (false, 0);
            Some(std::mem::take(&mut shell.lent))
        }
        _ => None,
    });
    let Some(was) = was else { return };
    // SAFETY: as in `put_in`.
    unsafe { Shell_NotifyIconW(DELETE, &icon_data(hwnd, id)) };
    give_back(was);
}

/// Destroys an icon this library created (0 is none).
fn give_back(icon: HANDLE) {
    if icon != 0 {
        // SAFETY: the icon came from `icon_made`, and nothing shows it any more.
        unsafe { DestroyIcon(icon) };
    }
}

/// Shell data showing a notification from one of the program's icons: its title and text, with
/// its image (or the icon's own, where none).
pub(crate) fn balloon(hwnd: HANDLE, id: u32, title: &str, text: &str, picture: HANDLE) -> IconData {
    let mut data = icon_data(hwnd, id);
    data.flags = MESSAGE | INFO;
    written(&mut data.info_title, title);
    // (The shell drops a notification without text: it gets at least a space.)
    written(&mut data.info, if text.is_empty() { " " } else { text });
    (data.info_flags, data.balloon_icon) = (OWN_PICTURE | QUIET_TIME | if picture != 0 { LARGE } else { 0 }, picture);
    data
}

/// Shows a notification: from the program's tray icon, or one lent to it (added the first
/// time). Its image is made exactly the size Windows wants.
fn show(hwnd: HANDLE, out: crate::notifications::Out) {
    let side = unsafe { GetSystemMetrics(11) }.max(1) as usize;
    let picture = out.picture.and_then(|(width, pixels)| tray::fitting(&[(width, pixels)], side)).map_or(0, |pixels| icon_made(side, &pixels));
    let own = SHELL.with_borrow(|shell| shell.added);
    let id = if own { OWN } else { LENT };
    if !own && !SHELL.with_borrow(|shell| shell.lending) {
        // (The lent icon is the program's own, as the app set it, or the system's.)
        let (mut data, icon) = tray_icon(hwnd);
        data.id = LENT;
        let lent = unsafe { Shell_NotifyIconW(ADD, &data) } != 0;
        data.version = 4;
        if !lent {
            give_back(icon);
            give_back(picture);
            return;
        }
        unsafe { Shell_NotifyIconW(SET_VERSION, &data) };
        SHELL.with_borrow_mut(|shell| (shell.lending, shell.lent) = (true, icon));
    }
    // SAFETY: as in `put_in`. (Windows has copied the image by the time the call returns.)
    let shown = unsafe { Shell_NotifyIconW(MODIFY, &balloon(hwnd, id, &out.title, &out.text, picture)) } != 0;
    give_back(picture);
    if shown {
        SHELL.with_borrow_mut(|shell| shell.showing[(id - 1) as usize] = out.number);
    }
}

/// The icon's context menu as a Windows menu: an item per app row, numbered from 1 by position,
/// checked, radio-dotted and greyed as the app said; a separator for a separator.
pub(crate) fn menu_of(rows: &[Row]) -> HANDLE {
    // SAFETY: a menu of this thread's is created and filled; every string is NUL-terminated and
    // copied by Windows.
    unsafe {
        let menu = CreatePopupMenu();
        for (at, row) in rows.iter().enumerate() {
            let id = at + 1;
            let pale = if row.enabled { 0 } else { 1 };
            match row.kind {
                RowKind::Line => _ = AppendMenuW(menu, 0x800, 0, std::ptr::null()),
                RowKind::Tick(on) => _ = AppendMenuW(menu, pale | if on { 8 } else { 0 }, id, wide(&row.label).as_ptr()),
                RowKind::Item | RowKind::Choice(_) => _ = AppendMenuW(menu, pale, id, wide(&row.label).as_ptr()),
            }
            if row.kind == RowKind::Choice(true) {
                CheckMenuRadioItem(menu, id as u32, id as u32, id as u32, 0);
            }
        }
        menu
    }
}

/// Shows the icon's menu at a screen point and returns the chosen row (none if dismissed). The
/// hidden window is brought to the foreground first, as Windows requires for tray menus:
/// otherwise the menu does not close on a click elsewhere.
fn pop_up(hwnd: HANDLE, x: i32, y: i32) -> Option<usize> {
    let rows = TRAY.with_borrow(|tray| tray.rows.clone());
    if rows.is_empty() {
        return None;
    }
    // SAFETY: the menu is this thread's, tracked over this thread's window, and destroyed
    // afterwards. (`TPM_RIGHTBUTTON`, 2; aligned as drop-down menus are, 40 and 8;
    // `TPM_BOTTOMALIGN`, 0x20; the choice returned rather than posted,
    // `TPM_RETURNCMD | TPM_NONOTIFY`, 0x100 and 0x80.) The message after it is the one Windows
    // requires so the menu closes properly.
    let picked = unsafe {
        let menu = menu_of(&rows);
        SetForegroundWindow(hwnd);
        let align = if GetSystemMetrics(40) != 0 { 8 } else { 0 };
        let picked = TrackPopupMenuEx(menu, 0x100 | 0x80 | 0x20 | 2 | align, x, y, hwnd, std::ptr::null());
        PostMessageW(hwnd, 0, 0, 0);
        DestroyMenu(menu);
        picked
    };
    (picked > 0).then(|| picked as usize - 1)
}

/// The user did something with the icon: the next run hears of it.
fn did(did: Did) {
    TRAY.with_borrow_mut(|tray| tray.news.push(did));
    crate::platform::Waker.wake();
}

/// The hidden window's procedure: icon callbacks, hotkeys, forwarded launches, and
/// `TaskbarCreated`. (Everything else goes to Windows.)
unsafe extern "system" fn heard(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize {
    match msg {
        CALLBACK => from_icon(hwnd, w, l),
        WM_HOTKEY => {
            if HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.pressed(w as u32)) {
                crate::platform::Waker.wake();
            }
        }
        // SAFETY: Windows passes the launch data as sent, valid while this message is handled.
        WM_COPYDATA => return unsafe { copied(l) },
        _ if msg == ANEW.get() && msg != 0 => anew(hwnd),
        // SAFETY: unhandled messages go to Windows as they came.
        _ => return unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
    0
}

/// An icon callback: the event (low word of `l`), the icon (high word), and for its menu the
/// position (`w`, x and y in screen pixels).
fn from_icon(hwnd: HANDLE, w: usize, l: isize) {
    let (what, id) = ((l & 0xffff) as u32, (l >> 16 & 0xffff) as u32);
    let (x, y) = ((w & 0xffff) as i16 as i32, (w >> 16 & 0xffff) as i16 as i32);
    match (what, id) {
        (SELECT | KEY_SELECT, OWN) => did(Did::Clicked),
        (DOUBLE_CLICK, OWN) => did(Did::Twice),
        (CONTEXT_MENU, OWN) => pop_up(hwnd, x, y).into_iter().for_each(|at| did(Did::Picked(at))),
        (BALLOON_CLICK, OWN | LENT) => {
            let number = SHELL.with_borrow(|shell| shell.showing[(id - 1) as usize]);
            if number != 0 {
                NOTICES.with_borrow_mut(|notices| notices.clicked.push(number));
                crate::platform::Waker.wake();
            }
            if id == LENT {
                take_out(hwnd, LENT);
            }
        }
        (BALLOON_HIDE | BALLOON_TIMEOUT, LENT) => take_out(hwnd, LENT),
        _ => {}
    }
}

/// The taskbar was recreated (Explorer restarted): the program's icon is re-added as it was.
/// (One lent to a showing notification went with the notification.)
fn anew(hwnd: HANDLE) {
    let (added, refused, lent) = SHELL.with_borrow_mut(|shell| {
        let was = (shell.added, shell.refused, std::mem::take(&mut shell.lent));
        (shell.added, shell.refused, shell.lending, shell.showing) = (false, false, false, [0; 2]);
        was
    });
    give_back(lent);
    if added || refused {
        put_in(hwnd);
    }
}

/// A later launch forwarded its arguments: the next run hears of them, and the first window
/// comes forward. Returns whether it was one (1) or not (0).
///
/// # Safety
/// `l` is a `WM_COPYDATA`'s `lParam` as Windows passes it.
unsafe fn copied(l: isize) -> isize {
    // SAFETY: as the caller promises: a tag, a byte count, and a pointer.
    let data = unsafe { &*(l as *const CopyData) };
    if data.data != STARTS || data.bytes.is_null() || !FIRST.get() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
    let Some((tokens, start)) = single_instance::unpacked(bytes) else { return 0 };
    single_instance::came(tokens, start);
    1
}

/// `COPYDATASTRUCT`: a tag, a byte count, and a pointer.
#[repr(C)]
struct CopyData {
    data: usize,
    size: u32,
    bytes: *const u8,
}

/// Makes this the single running instance of the program named `name` (see
/// `single_instance::first_copy`): the first instance holds a session mutex of that name and
/// gives its hidden window the name; a later one finds that window, lets it take the
/// foreground, and hands it `packed`. Returns whether this is the first instance. (While the
/// first is still starting or ending, the later one retries for a while; where it cannot be
/// reached at all, this runs as a first instance.)
pub(crate) fn first_copy(name: &str, packed: &[u8]) -> bool {
    let (mutex, title) = (wide(&format!("Local\\taut {}", name.replace('\\', "/"))), wide(&format!("taut {name}")));
    let since = Instant::now();
    while since.elapsed() < PATIENCE {
        // SAFETY: a session mutex by name; one that already existed is closed again (183:
        // `ERROR_ALREADY_EXISTS`). The first instance keeps its own until it ends.
        let (held, there) = unsafe { (CreateMutexW(std::ptr::null(), 0, mutex.as_ptr()), GetLastError() == 183) };
        if held == 0 {
            return true;
        }
        if !there {
            unsafe { SetWindowTextW(window(), title.as_ptr()) };
            FIRST.set(true);
            return true;
        }
        unsafe { CloseHandle(held as usize) };
        match hand_over(&title, packed) {
            Some(true) => return false,
            // (A first instance with higher privileges than this launch: this runs on its own.)
            None => return true,
            Some(false) => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Forwards a launch to the first instance's hidden window, titled `title`, and lets it take
/// the foreground (a program the user just started may, and may grant it). Returns whether the
/// first instance accepted (one not answering within five seconds, or apparently hung, has
/// not); none where Windows blocks this launch from reaching it (it runs elevated, this one
/// not).
fn hand_over(title: &[u16], packed: &[u8]) -> Option<bool> {
    let data = CopyData { data: STARTS, size: packed.len() as u32, bytes: packed.as_ptr() };
    let mut taken = 0;
    // SAFETY: a window found by class and title; the data outlives the call, which waits for
    // the answer (`SMTO_ABORTIFHUNG`, 2).
    unsafe {
        let first = FindWindowW(wide(CLASS).as_ptr(), title.as_ptr());
        if first == 0 {
            return Some(false);
        }
        let mut process = 0;
        GetWindowThreadProcessId(first, &mut process);
        AllowSetForegroundWindow(process);
        // (5: `ERROR_ACCESS_DENIED`, UIPI blocked the message.)
        let sent = SendMessageTimeoutW(first, WM_COPYDATA, 0, &data as *const CopyData as isize, 2, 5000, &mut taken) != 0;
        (sent || GetLastError() != 5).then_some(sent && taken == 1)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn what_the_shell_is_told_of_an_icon_is_laid_out_as_shellapi_h_has_it() {
        // NOTIFYICONDATAW with 64-bit handles: 976 bytes, and each field's offset.
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(size_of::<IconData>(), 976);
            let at = [std::mem::offset_of!(IconData, icon), std::mem::offset_of!(IconData, tip), std::mem::offset_of!(IconData, state), std::mem::offset_of!(IconData, info)];
            assert_eq!(at, [32, 40, 296, 304]);
            let at = [
                std::mem::offset_of!(IconData, version),
                std::mem::offset_of!(IconData, info_title),
                std::mem::offset_of!(IconData, info_flags),
                std::mem::offset_of!(IconData, guid),
                std::mem::offset_of!(IconData, balloon_icon),
            ];
            assert_eq!(at, [816, 820, 948, 952, 968]);
        }
        #[cfg(target_pointer_width = "32")]
        assert_eq!(size_of::<IconData>(), 956);
    }

    #[test]
    fn texts_are_cut_to_the_room_the_shell_has_for_them_at_a_whole_character() {
        let mut room = [7u16; 6];
        written(&mut room, "abc");
        assert_eq!(room, [97, 98, 99, 0, 0, 0]);
        written(&mut room, "abcdefgh");
        assert_eq!(room, [97, 98, 99, 100, 101, 0], "five letters and the zero");
        // A surrogate pair that would not fit whole is left out.
        written(&mut room, "abcd\u{1F600}");
        assert_eq!(room, [97, 98, 99, 100, 0, 0]);
        let data = balloon(0, LENT, &"T".repeat(100), "", 0);
        assert_eq!((data.info_title[62], data.info_title[63], data.info[0], data.info[1]), (b'T' as u16, 0, b' ' as u16, 0));
        assert_eq!((data.flags, data.info_flags, data.id), (MESSAGE | INFO, OWN_PICTURE | QUIET_TIME, LENT));
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetMenuItemCount(menu: HANDLE) -> i32;
        fn GetMenuStringW(menu: HANDLE, item: u32, text: *mut u16, room: i32, how: u32) -> i32;
        fn GetMenuState(menu: HANDLE, item: u32, how: u32) -> u32;
    }

    #[test]
    fn the_menu_is_a_menu_of_the_systems_with_the_rows_the_app_said() {
        let row = |label: &str, kind: RowKind, enabled: bool| Row { label: label.into(), kind, enabled };
        let rows = [
            row("&Show", RowKind::Item, true),
            row("&Quiet", RowKind::Tick(true), true),
            row("", RowKind::Line, true),
            row("One", RowKind::Choice(true), true),
            row("Two", RowKind::Choice(false), false),
        ];
        let menu = menu_of(&rows);
        // (Created, never shown: a menu shows only when tracked.)
        let text = |at: u32| {
            let mut text = [0u16; 32];
            let len = unsafe { GetMenuStringW(menu, at, text.as_mut_ptr(), 32, 0x400) };
            String::from_utf16_lossy(&text[..len.max(0) as usize])
        };
        // By position (`MF_BYPOSITION`, 0x400): its text, checked (8), separator (0x800), radio
        // (0x200, with 8), greyed (1).
        let state = |at: u32| unsafe { GetMenuState(menu, at, 0x400) };
        assert_eq!(unsafe { GetMenuItemCount(menu) }, 5);
        assert_eq!([text(0), text(1), text(3), text(4)], ["&Show", "&Quiet", "One", "Two"]);
        assert_eq!([state(0), state(1) & 8, state(2) & 0x800, state(3) & 0x208, state(4) & 1], [0, 8, 0x800, 0x208, 1]);
        // Rows are numbered by position, from 1: what a choice returns.
        assert_eq!(unsafe { GetMenuState(menu, 4, 0) } & 0x208, 0x208, "the fourth row is the dotted one");
        unsafe { DestroyMenu(menu) };
    }

    // ---- the shell, played on a desktop of the test's own ----

    use crate::notifications::Notification;
    use crate::windows::sys::{DestroyWindow, IsIconic, IsWindowVisible, KillTimer, SendMessageW, SetTimer, ShowWindow};
    use crate::windows::tests::{ALONE, is_on_its_own_desktop, on_a_desktop_of_its_own, pump};
    use crate::{Ui, Window};
    use std::rc::Rc;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EndMenu() -> i32;
        fn SetKeyboardState(keys: *const [u8; 256]) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(of: usize, millis: u32) -> u32;
        fn ReleaseMutex(mutex: HANDLE) -> i32;
    }

    /// What the shell was told, as the test's own taskbar heard it: the request (`ADD` and the
    /// rest), the icon, the flags (`MESSAGE` and the rest), its tooltip, its notification's
    /// title and text, and the version requested.
    #[derive(Clone, Debug, PartialEq)]
    struct Heard {
        what: u32,
        id: u32,
        flags: u32,
        tip: String,
        title: String,
        info: String,
        version: u32,
    }

    thread_local! {
        static HEARD: RefCell<Vec<Heard>> = const { RefCell::new(Vec::new()) };
        static REFUSES: Cell<bool> = const { Cell::new(false) };
        /// The menu as the test worked it (see `Worked`).
        static WORKED: Cell<Worked> = const { Cell::new(Worked { keys: 0, ended: None }) };
    }

    /// Reads a number and a string from what the shell passed the taskbar.
    fn number_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }
    fn text_at(bytes: &[u8], at: usize, units: usize) -> String {
        let units: Vec<u16> = bytes[at..at + 2 * units].chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).take_while(|unit| *unit != 0).collect();
        String::from_utf16_lossy(&units)
    }

    /// The test's own taskbar: the window `Shell_NotifyIconW` sends its requests to
    /// (`WM_COPYDATA`, tag 1: a signature, the request, then the icon's NOTIFYICONDATAW, in
    /// whichever layout its first field indicates). It accepts everything unless told to
    /// refuse.
    unsafe extern "system" fn task_bar(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize {
        if msg != WM_COPYDATA {
            return unsafe { DefWindowProcW(hwnd, msg, w, l) };
        }
        let data = unsafe { &*(l as *const CopyData) };
        let bytes = unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
        assert_eq!((data.data, number_at(bytes, 0)), (1, 0x3475_3423), "what the shell hands its task bar");
        let (what, icon) = (number_at(bytes, 4), &bytes[8..]);
        // (64-bit handles, as this program has them; or 32-bit, as any program can send.)
        let [id, flags, tip, info, version, title] = match number_at(icon, 0) {
            976 => [16, 20, 40, 304, 816, 820],
            956 => [8, 12, 24, 288, 800, 804],
            other => panic!("the shell handed the task bar an icon's data of {other} bytes"),
        };
        let heard = Heard {
            what,
            id: number_at(icon, id),
            flags: number_at(icon, flags),
            tip: text_at(icon, tip, 128),
            title: text_at(icon, title, 64),
            info: text_at(icon, info, 256),
            version: number_at(icon, version),
        };
        HEARD.with_borrow_mut(|told| told.push(heard));
        (!REFUSES.get()) as isize
    }

    /// The shell requests since last asked, each as request and icon.
    fn heard() -> Vec<(u32, u32)> {
        HEARD.with_borrow_mut(std::mem::take).iter().map(|heard| (heard.what, heard.id)).collect()
    }

    /// What the app in the test window keeps and hears.
    #[derive(Default)]
    struct Seen {
        tray: bool,
        quiet: bool,
        send: Option<&'static str>,
        sent: Vec<Notification>,
        clicked: Vec<Notification>,
        heard: Vec<&'static str>,
        hide: bool,
        forward: bool,
        hidden: bool,
        taken: bool,
    }

    /// The test icon: 16×16, red.
    const RED: [u32; 256] = [0xffff_0000; 256];
    /// Keys no keyboard has: F23 and F24 with all four modifiers.
    const KEY: &str = "Ctrl+Alt+Shift+Win+F24";
    const OTHER_KEY: &str = "Ctrl+Alt+Shift+Win+F23";

    fn describe(seen: &RefCell<Seen>, ui: &mut Ui) {
        let mut seen = seen.borrow_mut();
        if seen.tray {
            let icon = ui.tray(&[(16, &RED)], "Notes");
            if icon.clicked {
                seen.heard.push("clicked");
            }
            if icon.double_clicked {
                seen.heard.push("twice");
            }
            if ui.tray_item("&Show") {
                seen.heard.push("show");
            }
            let mut quiet = seen.quiet;
            if ui.tray_checkbox(&mut quiet, "&Quiet") {
                seen.heard.push("quiet");
            }
            seen.quiet = quiet;
            ui.tray_separator();
            ui.tray_item("&Quit");
        }
        if ui.hotkey(KEY) {
            seen.heard.push("key");
        }
        ui.hotkey(OTHER_KEY);
        seen.taken = ui.hotkey_refused(OTHER_KEY);
        if let Some(text) = seen.send.take() {
            let sent = ui.system_notification("Saved", text, None);
            seen.sent.push(sent);
        }
        let clicked = ui.notification_clicked();
        seen.clicked.extend(clicked);
        if std::mem::take(&mut seen.hide) {
            ui.hide_window();
        }
        if std::mem::take(&mut seen.forward) {
            ui.bring_to_front();
        }
        seen.hidden = ui.window_minimized();
        ui.label("One");
    }

    /// How the test worked the icon's menu: the menu window the keys went to (0: it never came
    /// up), and the row lit when the test had to end it (-1: none).
    #[derive(Clone, Copy, Debug)]
    struct Worked {
        keys: HANDLE,
        ended: Option<i32>,
    }

    /// How long the menu may stay up before the test ends it: far longer than three keys take.
    const MENU_PATIENCE: u32 = 60_000;

    /// Shows the icon's menu at a point and, once it is up, works it by keys (Down, Down, Enter:
    /// the second row).
    fn work_the_menu(shell: HANDLE) -> Worked {
        // (Timers fire in a message loop: here, the menu's own.)
        unsafe extern "system" fn keys(_: HANDLE, _: u32, timer: usize, _: u32) {
            let menu = unsafe { FindWindowW(wide("#32768").as_ptr(), std::ptr::null()) };
            if menu == 0 || unsafe { IsWindowVisible(menu) } == 0 {
                return;
            }
            unsafe { KillTimer(0, timer) };
            for key in [0x28, 0x28, 0x0D] {
                unsafe { PostMessageW(menu, 0x0100, key, 0) };
            }
            WORKED.set(Worked { keys: menu, ..WORKED.get() });
        }
        unsafe extern "system" fn enough(_: HANDLE, _: u32, timer: usize, _: u32) {
            // The lit row: `MN_GETHMENU` (0x01E1), then `MF_HILITE` (0x80) by position.
            let menu = unsafe { FindWindowW(wide("#32768").as_ptr(), std::ptr::null()) };
            let rows = unsafe { SendMessageW(menu, 0x01E1, 0, 0) };
            let lit = (0..unsafe { GetMenuItemCount(rows) }).find(|at| unsafe { GetMenuState(rows, *at as u32, 0x400) } & 0x80 != 0);
            WORKED.set(Worked { ended: Some(lit.unwrap_or(-1)), ..WORKED.get() });
            unsafe { (KillTimer(0, timer), EndMenu()) };
        }
        // A thread's key state starts as the session's, and this desktop gets no input: a mouse
        // button held as the test started stays held for this thread. A menu opened with its
        // button held takes no keys. So every key is up first.
        unsafe { SetKeyboardState(&[0; 256]) };
        WORKED.set(Worked { keys: 0, ended: None });
        let (first, last) = unsafe { (SetTimer(0, 0, 10, keys as *const () as usize), SetTimer(0, 0, MENU_PATIENCE, enough as *const () as usize)) };
        unsafe { SendMessageW(shell, CALLBACK, 300 | 400 << 16, (OWN << 16 | CONTEXT_MENU) as isize) };
        unsafe { (KillTimer(0, first), KillTimer(0, last)) };
        WORKED.get()
    }

    #[test]
    fn the_icon_its_menu_notifications_and_keys_reach_the_shell_and_what_it_says_comes_back() {
        // (The test plays the shell's taskbar, on a desktop without one: nothing can reach the
        // user's.)
        on_a_desktop_of_its_own("windows_shell::tests::on_a_desktop_with_a_task_bar_of_the_tests_own_the_tray_works");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_with_a_task_bar_of_the_tests_own_the_tray_works() {
        if std::env::var_os(ALONE).is_some() {
            the_tray_works();
        }
    }

    #[test]
    fn the_tray_menu_takes_its_keys_whatever_button_was_held_as_the_test_began() {
        on_a_desktop_of_its_own("windows_shell::tests::on_its_own_desktop_with_the_right_button_held_the_tray_still_works");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_its_own_desktop_with_the_right_button_held_the_tray_still_works() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        // This thread's key state as when the person held the right button as the test began.
        let mut held = [0u8; 256];
        held[2] = 0x80;
        unsafe { SetKeyboardState(&held) };
        the_tray_works();
    }

    /// The tray test, on a desktop of its own.
    fn the_tray_works() {
        // Hotkeys are the session's, not the desktop's: one tray test at a time, in whatever
        // process. (One that ended holding the lock gives it up: `WAIT_ABANDONED`, 0x80.)
        let lock = unsafe { CreateMutexW(std::ptr::null(), 0, wide("Local\\taut tests tray").as_ptr()) };
        let waited = unsafe { WaitForSingleObject(lock as usize, 100_000) };
        assert!(lock != 0 && (waited == 0 || waited == 0x80), "another tray test held the lock for 100 s ({waited:#x})");
        assert!(is_on_its_own_desktop(), "this is the desktop the person works in: nothing here may reach its task bar");
        assert_eq!(unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null()) }, 0, "this desktop has a task bar of its own");
        // The test's taskbar, under the class name the shell looks for.
        let class = wide("Shell_TrayWnd");
        let task_bar = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            RegisterClassW(&WNDCLASSW { style: 0, proc: Some(task_bar), class_extra: 0, window_extra: 0, instance, icon: 0, cursor: 0, background: 0, menu: std::ptr::null(), name: class.as_ptr() });
            CreateWindowExW(0x80, class.as_ptr(), class.as_ptr(), 0x8000_0000, 0, 0, 0, 0, 0, 0, instance, std::ptr::null_mut())
        };
        // A key another program holds: the app will be refused it.
        let other = combo_bits(OTHER_KEY);
        assert!(unsafe { RegisterHotKey(task_bar, 1, other.0, other.1) } != 0, "{OTHER_KEY} was free");

        let seen = Rc::new(RefCell::new(Seen { tray: true, ..Seen::default() }));
        let app = seen.clone();
        let hwnd = crate::windows::open(Window { position: Some((50, 50)), width: 300.0, height: 200.0, ..Window::default() }, Box::new(move |ui| describe(&app, ui)));
        unsafe { ShowWindow(hwnd, 4) };
        pump();
        let shell = SHELL.with_borrow(|shell| shell.hwnd);
        let wake = || {
            unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
            pump();
        };

        // The icon was added, as version 4, with its tooltip and image.
        let told = HEARD.with_borrow(|told| told.clone());
        assert_eq!(told.iter().map(|heard| (heard.what, heard.id)).collect::<Vec<_>>(), [(ADD, OWN), (SET_VERSION, OWN)]);
        assert_eq!((told[0].flags, told[0].tip.as_str(), told[1].version), (MESSAGE | PICTURE | TIP | SHOW_TIP, "Notes", 4));
        heard();
        // The key no keyboard has is the app's; the one another program holds is refused.
        assert!(seen.borrow().taken);
        let number = HOTKEYS.with_borrow(|hotkeys| hotkeys.keys.iter().find(|wanted| wanted.state == Registration::Registered).map(|wanted| wanted.number));
        let number = number.expect("the app has the key no keyboard has");

        // A click, a double click, the key: each heard once, in the next run.
        unsafe { SendMessageW(shell, CALLBACK, 0, (OWN << 16 | SELECT) as isize) };
        pump();
        unsafe { SendMessageW(shell, CALLBACK, 0, (OWN << 16 | DOUBLE_CLICK) as isize) };
        unsafe { SendMessageW(shell, WM_HOTKEY, number as usize, 0) };
        pump();
        wake();
        assert_eq!(std::mem::take(&mut seen.borrow_mut().heard), ["clicked", "twice", "key"]);

        // The menu: a system menu operated by keys. The second row is the check item.
        let worked = work_the_menu(shell);
        pump();
        assert!(worked.keys != 0 && worked.ended.is_none(), "the menu took the keys: {worked:?}");
        assert_eq!(std::mem::take(&mut seen.borrow_mut().heard), ["quiet"]);
        assert!(seen.borrow().quiet);
        // (A changed menu is no news to the shell: it is rebuilt on each request.)
        assert!(heard().is_empty());

        // A notification comes from the icon; clicked, the app hears which.
        seen.borrow_mut().send = Some("notes.txt is on the disk");
        wake();
        let told = HEARD.with_borrow_mut(std::mem::take);
        assert_eq!(told.iter().map(|heard| (heard.what, heard.id, heard.flags)).collect::<Vec<_>>(), [(MODIFY, OWN, MESSAGE | INFO)]);
        assert_eq!((told[0].title.as_str(), told[0].info.as_str()), ("Saved", "notes.txt is on the disk"));
        unsafe { SendMessageW(shell, CALLBACK, 0, (OWN << 16 | BALLOON_CLICK) as isize) };
        pump();
        let sent = seen.borrow().sent.clone();
        assert_eq!(seen.borrow().clicked, sent);

        // Without its own tray icon, a notification comes from one lent while it shows.
        seen.borrow_mut().tray = false;
        wake();
        assert_eq!(heard(), [(DELETE, OWN)]);
        seen.borrow_mut().send = Some("again");
        wake();
        assert_eq!(heard(), [(ADD, LENT), (SET_VERSION, LENT), (MODIFY, LENT)]);
        unsafe { SendMessageW(shell, CALLBACK, 0, (LENT << 16 | BALLOON_TIMEOUT) as isize) };
        assert_eq!(heard(), [(DELETE, LENT)]);

        // A taskbar refusing the icon is not asked again until `TaskbarCreated`; then the icon
        // is re-added.
        REFUSES.set(true);
        seen.borrow_mut().tray = true;
        wake();
        wake();
        assert_eq!(heard(), [(ADD, OWN)]);
        REFUSES.set(false);
        unsafe { SendMessageW(shell, ANEW.get(), 0, 0) };
        assert_eq!(heard(), [(ADD, OWN), (SET_VERSION, OWN)]);
        unsafe { SendMessageW(shell, ANEW.get(), 0, 0) };
        assert_eq!(heard(), [(ADD, OWN), (SET_VERSION, OWN)], "and again, each time the task bar is there anew");

        // The window hides to the tray, with its taskbar button; and comes back.
        seen.borrow_mut().hide = true;
        wake();
        wake();
        assert!(unsafe { IsWindowVisible(hwnd) } == 0 && seen.borrow().hidden);
        seen.borrow_mut().forward = true;
        wake();
        wake();
        assert!(unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { IsIconic(hwnd) } == 0 && !seen.borrow().hidden);

        // The hotkeys are released with the window, and the icon leaves the tray.
        unsafe { DestroyWindow(hwnd) };
        pump();
        assert_eq!(heard(), [(DELETE, OWN)]);
        assert!(unsafe { RegisterHotKey(task_bar, 2, combo_bits(KEY).0, combo_bits(KEY).1) } != 0, "{KEY} was given back");
        unsafe { (UnregisterHotKey(task_bar, 1), UnregisterHotKey(task_bar, 2)) };

        // An app living in the tray that hides its window in its very first run (which happens
        // as `run` shows the window) is never seen.
        let seen = Rc::new(RefCell::new(Seen { tray: true, hide: true, ..Seen::default() }));
        let app = seen.clone();
        let hwnd = crate::windows::open(Window { position: Some((50, 50)), width: 300.0, height: 200.0, ..Window::default() }, Box::new(move |ui| describe(&app, ui)));
        unsafe { ShowWindow(hwnd, 1) };
        pump();
        assert!(unsafe { IsWindowVisible(hwnd) } == 0 && seen.borrow().hidden);
        assert_eq!(heard(), [(ADD, OWN), (SET_VERSION, OWN)]);
        unsafe { (DestroyWindow(hwnd), DestroyWindow(task_bar)) };
        unsafe { (ReleaseMutex(lock), CloseHandle(lock as usize)) };
    }

    /// Tells a test process it is a program's later launch, and the program's name.
    const LATER: &str = "TAUT_TEST_LATER_START";

    #[test]
    fn a_later_start_hands_what_it_was_given_to_the_first_which_comes_forward() {
        // (The first instance's window comes to the foreground: on its own desktop.)
        on_a_desktop_of_its_own("windows_shell::tests::on_a_desktop_of_its_own_a_later_start_reaches_the_first");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_of_its_own_a_later_start_reaches_the_first() {
        if std::env::var_os(ALONE).is_none() || std::env::var_os(LATER).is_some() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works in: no window comes forward on it");
        let name = format!("org.example.Tests {}", std::process::id());
        assert!(crate::single_instance::single_instance(&name), "nothing else runs by that name");
        let starts = Rc::new(RefCell::new(Vec::new()));
        let heard = starts.clone();
        let hwnd = crate::windows::open(Window { position: Some((50, 50)), width: 300.0, height: 200.0, ..Window::default() }, Box::new(move |ui| heard.borrow_mut().extend(ui.relaunches())));
        // (Minimized and not in front: the launch should restore it.)
        unsafe { ShowWindow(hwnd, 7) };
        pump();
        assert!(unsafe { IsIconic(hwnd) } != 0);
        // A later launch of the program, in its own folder, with a file to open.
        let folder = std::env::temp_dir();
        let later = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "windows_shell::tests::a_later_start_of_the_tests", "--ignored", "--nocapture", "notes of mine.txt"])
            .env(LATER, &name)
            .current_dir(&folder)
            .spawn()
            .unwrap();
        let since = Instant::now();
        while starts.borrow().is_empty() && since.elapsed() < Duration::from_secs(20) {
            pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        let ran = later.wait_with_output().unwrap();
        assert!(ran.status.success(), "the later start ended with {:?}", ran.status);
        let starts = starts.borrow();
        assert_eq!(starts.len(), 1, "the later start was heard once");
        assert_eq!((starts[0].working_dir.as_path(), starts[0].args.last().and_then(|arg| arg.to_str())), (folder.as_path(), Some("notes of mine.txt")));
        // The first instance's window came forward for it.
        assert!(unsafe { IsIconic(hwnd) } == 0, "the first copy's window is back");
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    #[ignore = "a later start of the program, started by the test before it"]
    fn a_later_start_of_the_tests() {
        let Some(name) = std::env::var_os(LATER) else { return };
        assert!(!crate::single_instance::single_instance(&name.to_string_lossy()), "the first copy took the start");
    }

    /// A hotkey as Windows takes it: modifiers and virtual key.
    fn combo_bits(keys: &str) -> (u32, u32) {
        let combo = crate::hotkeys::combo(keys).unwrap();
        (held_bits(combo.modifiers), combo.key.virtual_key())
    }

    #[test]
    fn starts_are_taken_only_by_a_first_copy_and_not_kept_without_end() {
        on_a_desktop_of_its_own("windows_shell::tests::on_a_desktop_of_its_own_starts_are_taken_by_a_first_copy_alone");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_of_its_own_starts_are_taken_by_a_first_copy_alone() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop());
        // A tray icon, hotkeys or notifications create the hidden window without claiming
        // single-instance.
        after();
        pump();
        let hidden = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(CLASS).as_ptr()) };
        assert!(hidden != 0, "the hidden window is there");
        let start = single_instance::Relaunch { args: vec!["--erase-everything".into()], working_dir: "C:\\".into() };
        let bytes = single_instance::packed(&single_instance::Tokens::default(), &start);
        let data = CopyData { data: 0x7461_7574, size: bytes.len() as u32, bytes: bytes.as_ptr() };
        // Another program forwards it a launch, as a later launch of a single-instance program
        // does.
        let took = unsafe { SendMessageW(hidden, WM_COPYDATA, 0, &data as *const CopyData as isize) };
        assert_eq!(took, 0, "a program that is not the one copy of anything takes no starts");
        // A first instance accepts them, but keeps at most 64 no run has taken.
        assert!(single_instance::single_instance(&format!("org.example.Starts {}", std::process::id())));
        let first = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(&format!("taut org.example.Starts {}", std::process::id())).as_ptr()) };
        assert!(first != 0);
        let took: isize = (0..1000).map(|_| unsafe { SendMessageW(first, WM_COPYDATA, 0, &data as *const CopyData as isize) }).sum();
        assert!(took > 0, "a first copy takes starts");
        let mut trial = crate::testing::Trial::new(0usize, |heard: &mut usize, ui| *heard += ui.relaunches().len());
        trial.run();
        assert!(trial.app <= 64, "{} starts were kept for the app", trial.app);
    }
}
