//! iOS and iPadOS: UIKit, spoken to through the Objective-C runtime (`macos_sys`). Text is
//! CoreText's (`core_text`), as on macOS.
//!
//! `run` hands the program to UIKit (`UIApplicationMain`, which never returns), with an
//! application delegate of the library's own made at run time. The window comes with the first
//! scene (`UIApplicationSceneManifest` in the bundle's `Info.plist`), or at launch without one.
//! Its content is a view of the library's (`TautView`) holding one layer (the *page*) over the
//! safe area: the CPU draws every pixel into a block in memory, as everywhere, and what changed
//! is handed to that layer as its picture. Around the safe area (the status bar, the home
//! indicator, the keyboard) shows the theme's background.
//!
//! UIKit runs the loop. What its calls tell the view is noted (`Heard`), and a turn of the
//! library's is queued on the main queue: it takes in what was noted, runs the interface if
//! needed, and draws what changed. Nothing of the interface runs inside UIKit's calls, but for
//! three: a new layout (rotation, a new size in Split View or Stage Manager) and going to the
//! background, after which the system may stop the program at any time.
//!
//! Fingers are [`Event::Touch`] with the feature `touch`; without it the first finger is the
//! mouse, as under Wayland, and a long press is a right click. A pencil is the mouse with
//! [`Event::Stylus`] (`touch`); a pointer (a trackpad or mouse) is the mouse, its scrolling the
//! wheel, its pinch a zoom. Keys of a hardware keyboard come by their HID usage; Command and
//! Control are both `ctrl`. Typed text, from either keyboard and input methods, goes through
//! `ios_text_input`, which takes the keyboard while a text box has focus. Screen readers read
//! `ui_accessibility`.
//!
//! The window fills its scene: of [`Window`], only the title (it names the scene in the iPad's
//! app switcher) and the renderer are used. The CPU draws and Core Animation shows; with the
//! feature `gpu`, Metal shows (`gpu_ios`).
//!
//! On iPad each second window is a scene of its own: asked for as the app first describes it
//! (`requestSceneSessionActivation` with an activity naming the window), shown as it connects,
//! closed as the app stops describing it. The person closing its scene is a close request
//! ([`Ui::close_requested`]). A scene the person opens from the system has nothing to show and
//! closes. What UIKit says of a view goes to its window (`heard_in`); the keyboard, typing and
//! dialogs are the window in front's (the key window: `view`, `heard`). An iPhone shows one
//! scene: second windows do nothing there.
//!
//! The app hears the program go to the background as its window minimized
//! ([`Event::Minimized`], [`Ui::window_minimized`]): the moment to save its state. The system's
//! file dialog, drops and files opened are `ios_files`'; dragging out is `ios_drag_out`'s;
//! notifications are the user notification centre's (`macos_notifications`). Each is handed
//! to the window the first time an app asks for it. Tray icons and global hotkeys have no iOS
//! equivalent: they do nothing.
use crate::color::Color;
use crate::core_text::SystemText;
use crate::event::{Button, Cursor, Event, Key, Mods};
use crate::geom::{Point, Rect, Size, pt};
use crate::id::Id;
use crate::macos_sys::*;
use crate::platform::{Clipboard, PicturesAndFiles, Window};
use crate::preferences::Preferences;
use crate::screens::Screen;
use crate::second_window::{WindowKind, within};
use crate::surface::Surface;
use crate::theme::ContrastColors;
#[cfg(feature = "touch")]
use crate::touch::Stylus;
use crate::touch::TouchPhase;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// UIKit's edge insets (`UIEdgeInsets`): top, left, bottom, right.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Insets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIApplicationMain(argc: i32, argv: *mut *mut c_char, principal: Obj, delegate: Obj) -> i32;
    fn UIAccessibilityIsReduceMotionEnabled() -> bool;
    fn UIAccessibilityDarkerSystemColorsEnabled() -> bool;
    fn UIImagePNGRepresentation(image: Obj) -> Obj;
    static UIApplicationDidEnterBackgroundNotification: Obj;
    static UIApplicationWillEnterForegroundNotification: Obj;
    static UIApplicationDidBecomeActiveNotification: Obj;
    static UIApplicationWillResignActiveNotification: Obj;
    static UIApplicationWillTerminateNotification: Obj;
    static UIContentSizeCategoryDidChangeNotification: Obj;
    static UIAccessibilityReduceMotionStatusDidChangeNotification: Obj;
    static UIAccessibilityDarkerSystemColorsStatusDidChangeNotification: Obj;
    static UIKeyboardWillChangeFrameNotification: Obj;
    static UIKeyboardWillHideNotification: Obj;
    static UIKeyboardFrameEndUserInfoKey: Obj;
    static UIWindowDidBecomeKeyNotification: Obj;
}

// The main queue (`dispatch_get_main_queue()` is this object's address), in the system library.
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_after_f(when: u64, queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn exit(code: i32) -> !;
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_msgSendSuper();
}

/// `UITouchType`: a finger, a finger seen from afar, a pencil, a pointer.
const PENCIL: isize = 2;
const POINTER: isize = 3;
/// `UIGestureRecognizerState`: began, changed, ended, cancelled.
pub(crate) const BEGAN: isize = 1;
const CHANGED: isize = 2;
/// Modifier bits (`UIKeyModifierFlags`, as AppKit's): Shift, Control, Option, Command.
const SHIFT: isize = 1 << 17;
const CONTROL: isize = 1 << 18;
const OPTION: isize = 1 << 19;
const COMMAND: isize = 1 << 20;
/// The scroll distance of a wheel notch, in units (as on Linux and macOS).
const NOTCH: f32 = 48.0;
/// A held key repeats after this, then this often.
const REPEAT_AFTER: Duration = Duration::from_millis(500);
const REPEAT_EVERY: Duration = Duration::from_millis(40);

/// What UIKit said, for the next turn.
pub(crate) enum Heard {
    /// Input.
    Event(Event),
    /// The view's size, safe area or scale changed.
    Resized,
    /// The system's looks changed: dark or light, contrast, motion, text size.
    Looks,
    /// The user's locale or its formats changed: everything is written again.
    Locale,
    /// The keyboard covers this much of the view's bottom now, in points.
    Keyboard(f64),
    /// The program went to the background (`true`) or came back.
    Background(bool),
    /// The program became active (`true`) or stopped being.
    Active(bool),
    /// The system ends the program.
    Ending,
    /// Something only a part knows of (a reader's request).
    Other(Box<dyn FnOnce(&mut Ui) -> bool>),
    /// A scene connected after the first (iPad): its window and session, and the second window it
    /// was asked for, if any.
    SceneConnected(Obj, Obj, Option<Id>),
    /// A scene went (the person closed it, or the app): its session, and its window.
    SceneGone(Obj, Obj),
}

/// What UIKit's calls ask of a window while a turn may be running: its window, view and
/// controller, where the page lies in the view, whether the page is dark, the pointer's shape
/// over it, and which window it is (none: the first).
#[derive(Clone, Copy)]
struct Seen {
    window: Obj,
    view: Obj,
    controller: Obj,
    origin: NSPoint,
    dark: bool,
    cursor: Cursor,
    known: Option<Id>,
}

/// What `run` hands the window: the app's title, what is to show it, and the app.
type Starting = (String, crate::platform::Renderer, Box<dyn FnMut(&mut Ui)>);

/// What the view keeps between UIKit's calls: the fingers down (each touch and the id told the
/// interface), the one that is the mouse, the keys held, and the key repeating.
#[derive(Default)]
struct Input {
    fingers: Vec<(Obj, u32)>,
    next_finger: u32,
    mouse: Option<Obj>,
    pointer_button: Option<Button>,
    held: Mods,
    keys: Vec<isize>,
    /// The keys whose presses went on to the text system: their ends go there too.
    passed: Vec<isize>,
    repeating: Option<(Key, Mods, Instant)>,
}

thread_local! {
    /// What UIKit said, and of which window (by its view).
    static HEARD: RefCell<Vec<(Obj, Heard)>> = const { RefCell::new(Vec::new()) };
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    /// What `run` was given, until the window opens.
    static STARTING: RefCell<Option<Starting>> = const { RefCell::new(None) };
    static INPUT: RefCell<Input> = RefCell::new(Input::default());
    /// Each scene delegate and its window (UIKit asks a delegate for its own).
    static SCENE_WINDOWS: RefCell<Vec<(Obj, Obj)>> = const { RefCell::new(Vec::new()) };
    /// A turn is queued; the time the timer queued last fires at, and its number.
    static QUEUED: Cell<bool> = const { Cell::new(false) };
    static TIMER: Cell<(Option<Instant>, usize)> = const { Cell::new((None, 0)) };
    /// Text pasted by the system's Paste (its edit menu, the keyboard's bar): read at once, as
    /// the system allows without asking the person, and taken by the next paste.
    static PASTED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// What UIKit's calls ask of each window, the first window's first; and the view of the
    /// window in front (the key window: the keyboard's and the dialogs').
    static SEEN: RefCell<Vec<Seen>> = const { RefCell::new(Vec::new()) };
    static FRONT: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    /// What parts handed to the window put on each view (drops, drags out, the long press in
    /// text boxes): on every window's as it opens.
    static VIEW_PARTS: RefCell<Vec<fn(Obj)>> = const { RefCell::new(Vec::new()) };
}

/// What a window does after each run for a part an app asked for (see `follow_parts`), with its
/// view; and for typing, with the front window's page's place in its view.
pub(crate) type AfterRun = fn(&mut Ui, Obj);
pub(crate) type Typing = fn(&mut Ui, NSRect);

thread_local! {
    /// The parts handed to the window, each the first time an app asks for it: typing (see
    /// `ios_text_input`, which follows each run with the window's page), the system's
    /// file dialog, drops, dragging out, notifications. Nothing until then.
    pub(crate) static TYPING: Cell<Option<Typing>> = const { Cell::new(None) };
    pub(crate) static FILE_DIALOG: Cell<Option<AfterRun>> = const { Cell::new(None) };
    pub(crate) static DROPS: Cell<Option<AfterRun>> = const { Cell::new(None) };
    pub(crate) static DRAG_OUT: Cell<Option<AfterRun>> = const { Cell::new(None) };
    static NOTIFICATIONS: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Hands the window its notifications: shown after each run from now on.
pub(crate) fn hand_desktop() {
    NOTIFICATIONS.set(Some(show_notifications));
}

/// Shows the notifications the app asked for.
fn show_notifications() {
    let out = crate::notifications::NOTICES.with_borrow_mut(|notices| std::mem::take(&mut notices.out));
    crate::macos_notifications::show(out);
}

/// The program is running: a waker may queue a turn.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// A wake is queued that has not run yet: another queues nothing.
static WOKEN: AtomicBool = AtomicBool::new(false);

/// Notes what UIKit said of the window in front, and queues a turn.
pub(crate) fn heard(what: Heard) {
    heard_in(FRONT.get(), what);
}

/// Notes what UIKit said of the window of `view` (the front window for another view), and
/// queues a turn.
pub(crate) fn heard_in(view: Obj, what: Heard) {
    HEARD.with_borrow_mut(|heard| heard.push((view, what)));
    queue_turn();
}

/// What is kept of the window of `view`.
fn seen(view: Obj) -> Option<Seen> {
    SEEN.with_borrow(|seen| seen.iter().find(|seen| seen.view == view).copied())
}

/// Where the page lies in the front window's view, in points.
pub(crate) fn origin() -> NSPoint {
    origin_of(FRONT.get())
}

/// Where the page lies in `view`, one of the library's, in points.
pub(crate) fn origin_of(view: Obj) -> NSPoint {
    seen(view).map_or(NSPoint::default(), |seen| seen.origin)
}

/// The library's view in the front window: null until the window opens.
pub(crate) fn view() -> Obj {
    FRONT.get()
}

/// The library's view an interaction (a drop's, a drag's) is on.
pub(crate) fn view_of(interaction: Obj) -> Obj {
    // SAFETY: an interaction answers its view.
    unsafe { send(interaction, c"view", ()) }
}

/// Puts a part on the view of every window: those open, and each opening from now on.
pub(crate) fn on_every_view(add: fn(Obj)) {
    SEEN.with_borrow(|seen| seen.iter().for_each(|seen| add(seen.view)));
    VIEW_PARTS.with_borrow_mut(|parts| parts.push(add));
}

/// Does work with the front window's interface, unless a turn is running (then none).
pub(crate) fn with_ui<T>(work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    with_ui_of(FRONT.get(), work)
}

/// Does work with the interface of the window of `view`, unless a turn is running (then none).
pub(crate) fn with_ui_of<T>(view: Obj, work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    let known = seen(view)?.known;
    HOST.with(|host| within(&mut host.try_borrow_mut().ok()?.as_mut()?.ui, known, work))
}

/// The view controller showing what is in front of the front window: its own, or what it
/// presents (a dialog over a dialog). Null until the window opens.
pub(crate) fn front_controller() -> Obj {
    front_controller_of(FRONT.get())
}

/// The view controller showing what is in front of the window of `view`.
pub(crate) fn front_controller_of(view: Obj) -> Obj {
    let mut front = seen(view).map_or(std::ptr::null_mut(), |seen| seen.controller);
    loop {
        // SAFETY: a controller answers what it presents (nil for nothing; nil answers nil).
        let over: Obj = unsafe { send(front, c"presentedViewController", ()) };
        if over.is_null() {
            return front;
        }
        front = over;
    }
}

/// Has work done on the main thread, from any thread.
pub(crate) fn on_main(work: Box<dyn FnOnce() + Send>) {
    let work = Box::into_raw(Box::new(work));
    // SAFETY: the box is handed to the main queue, which hands it back to `do_work`.
    unsafe { dispatch_async_f(main_queue(), work.cast(), do_work) };
}

extern "C" fn do_work(work: *mut c_void) {
    // SAFETY: the box `on_main` made.
    let work = unsafe { Box::from_raw(work.cast::<Box<dyn FnOnce() + Send>>()) };
    let _pool = Pool::new();
    work();
}

/// The main queue.
fn main_queue() -> *const c_void {
    // (The address of the system's main queue object, which lives for good.)
    (&raw const _dispatch_main_q).cast()
}

/// Queues one turn on the main queue, unless one is.
fn queue_turn() {
    if !QUEUED.replace(true) {
        // SAFETY: a plain function queued with no context.
        unsafe { dispatch_async_f(main_queue(), std::ptr::null_mut(), queued) };
    }
}

extern "C" fn queued(_: *mut c_void) {
    QUEUED.set(false);
    turn();
}

/// Has the timer run a turn at `at`, unless one is due sooner.
fn wake_at(at: Instant) {
    let (due, number) = TIMER.get();
    if due.is_some_and(|due| due <= at) {
        return;
    }
    TIMER.set((Some(at), number + 1));
    let wait = at.saturating_duration_since(Instant::now()).as_nanos().min(i64::MAX as u128) as i64;
    // SAFETY: a plain function queued with its number as context; DISPATCH_TIME_NOW is 0.
    unsafe { dispatch_after_f(dispatch_time(0, wait), main_queue(), (number + 1) as *mut c_void, timed) };
}

/// A timer fired: a turn, unless a later one replaced it.
extern "C" fn timed(number: *mut c_void) {
    let (_, latest) = TIMER.get();
    if number as usize == latest {
        TIMER.set((None, latest));
        turn();
    }
}

/// Asks the window to run its interface again; can be sent from any thread. (An app learning
/// something in the background, say a finished download, wakes the window with this.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;

impl Waker {
    /// Has the interface run once more, soon, on its own thread, from whatever thread calls
    /// this.
    pub fn wake(&self) {
        if RUNNING.load(Ordering::SeqCst) && !WOKEN.swap(true, Ordering::SeqCst) {
            // SAFETY: the main queue takes work from any thread.
            unsafe { dispatch_async_f(main_queue(), std::ptr::null_mut(), woken) };
        }
    }
}

extern "C" fn woken(_: *mut c_void) {
    WOKEN.store(false, Ordering::SeqCst);
    heard(Heard::Other(Box::new(|_| true)));
}

// ---- the application and its scene ----

/// The application delegate's class: launching, a scene's configuration, and the system's
/// notices.
fn delegate_class() -> Obj {
    let notice = "v@:@";
    let methods: [(&CStr, *const c_void, &str); 11] = [
        (c"application:didFinishLaunchingWithOptions:", launched as *const c_void, "B@:@@"),
        (c"windowBecameKey:", window_became_key as *const c_void, notice),
        (c"application:configurationForConnectingSceneSession:options:", scene_configuration as *const c_void, "@@:@@@"),
        (c"enteredBackground:", entered_background as *const c_void, notice),
        (c"enteringForeground:", entering_foreground as *const c_void, notice),
        (c"becameActive:", became_active as *const c_void, notice),
        (c"resigningActive:", resigning_active as *const c_void, notice),
        (c"terminating:", terminating as *const c_void, notice),
        (c"looksChanged:", looks_changed as *const c_void, notice),
        (c"localeChanged:", locale_changed as *const c_void, notice),
        (c"keyboardMoved:", keyboard_moved as *const c_void, notice),
    ];
    made_class(c"TautAppDelegate", c"UIResponder", &methods, &[c"UIApplicationDelegate"])
}

/// The scene delegate's class: the window, made as the scene connects; the scene going.
fn scene_class() -> Obj {
    let methods: [(&CStr, *const c_void, &str); 4] = [
        (c"scene:willConnectToSession:options:", scene_connected as *const c_void, "v@:@@@"),
        (c"sceneDidDisconnect:", scene_gone as *const c_void, "v@:@"),
        (c"window", scene_window as *const c_void, "@@:"),
        (c"setWindow:", set_scene_window as *const c_void, "v@:@"),
    ];
    made_class(c"TautSceneDelegate", c"UIResponder", &methods, &[c"UIWindowSceneDelegate"])
}

/// The program launched: the delegate hears the system's notices; without scenes, the window
/// opens now.
extern "C" fn launched(delegate: Obj, _: Sel, _: Obj, _: Obj) -> Bool {
    let _pool = Pool::new();
    // SAFETY: plain setup of the notices, on the main thread; the delegate lives for good.
    unsafe {
        let centre: Obj = send(class(c"NSNotificationCenter"), c"defaultCenter", ());
        let notices = [
            (UIApplicationDidEnterBackgroundNotification, c"enteredBackground:"),
            (UIApplicationWillEnterForegroundNotification, c"enteringForeground:"),
            (UIApplicationDidBecomeActiveNotification, c"becameActive:"),
            (UIApplicationWillResignActiveNotification, c"resigningActive:"),
            (UIApplicationWillTerminateNotification, c"terminating:"),
            (UIContentSizeCategoryDidChangeNotification, c"looksChanged:"),
            (UIAccessibilityReduceMotionStatusDidChangeNotification, c"looksChanged:"),
            (UIAccessibilityDarkerSystemColorsStatusDidChangeNotification, c"looksChanged:"),
            (crate::macos_sys::NSCurrentLocaleDidChangeNotification, c"localeChanged:"),
            (UIKeyboardWillChangeFrameNotification, c"keyboardMoved:"),
            (UIKeyboardWillHideNotification, c"keyboardMoved:"),
            (UIWindowDidBecomeKeyNotification, c"windowBecameKey:"),
        ];
        for (name, action) in notices {
            send::<()>(centre, c"addObserver:selector:name:object:", (delegate, sel(action), name, std::ptr::null_mut::<c_void>()));
        }
        let bundle: Obj = send(class(c"NSBundle"), c"mainBundle", ());
        let scenes: Obj = send(bundle, c"objectForInfoDictionaryKey:", (ns_string("UIApplicationSceneManifest"),));
        if scenes.is_null() {
            let screen: Obj = send(class(c"UIScreen"), c"mainScreen", ());
            let window: Obj = send(send::<Obj>(class(c"UIWindow"), c"alloc", ()), c"initWithFrame:", (send_rect(screen, c"bounds", ()),));
            open(window);
        }
    }
    1
}

/// A scene's configuration: the library's scene delegate.
extern "C" fn scene_configuration(_: Obj, _: Sel, _: Obj, session: Obj, _: Obj) -> Obj {
    // SAFETY: a configuration made for the session's role, handed back autoreleased.
    unsafe {
        let role: Obj = send(session, c"role", ());
        let made: Obj = send(class(c"UISceneConfiguration"), c"alloc", ());
        let made: Obj = send(made, c"initWithName:sessionRole:", (std::ptr::null_mut::<c_void>(), role));
        send::<()>(made, c"setDelegateClass:", (scene_class(),));
        send(made, c"autorelease", ())
    }
}

/// The kind of activity a second window's scene is asked for with.
const SECOND_WINDOW: &str = "dev.taut.window";

/// A scene connects: the first opens the program's window; a later one is a second window's
/// (iPad), the window it was asked for being in its activity.
extern "C" fn scene_connected(delegate: Obj, _: Sel, scene: Obj, session: Obj, options: Obj) {
    // SAFETY: a window made for the scene: the first window's is kept for good, a second
    // window's until it closes.
    let window: Obj = unsafe { send(send::<Obj>(class(c"UIWindow"), c"alloc", ()), c"initWithWindowScene:", (scene,)) };
    SCENE_WINDOWS.with_borrow_mut(|windows| windows.push((delegate, window)));
    if STARTING.with_borrow(Option::is_some) {
        return open(window);
    }
    heard(Heard::SceneConnected(window, session, asked_window(options)));
}

/// The second window a scene was asked for, by the activity it connects with.
fn asked_window(options: Obj) -> Option<Id> {
    // SAFETY: plain questions of the options, their activities, and an activity's information.
    unsafe {
        let activities: Obj = send(send::<Obj>(options, c"userActivities", ()), c"allObjects", ());
        let ours = array_items(activities).into_iter().find(|activity| rust_string(send(*activity, c"activityType", ())).as_deref() == Some(SECOND_WINDOW))?;
        let number: Obj = send(send::<Obj>(ours, c"userInfo", ()), c"objectForKey:", (ns_string("window"),));
        (!number.is_null()).then(|| send::<u64>(number, c"unsignedLongLongValue", ()))
    }
}

/// A scene went: a second window's closes, and the scene's window is let go of (in the next
/// turn).
extern "C" fn scene_gone(delegate: Obj, _: Sel, scene: Obj) {
    let window = SCENE_WINDOWS.with_borrow_mut(|windows| windows.iter().position(|(of, _)| *of == delegate).map(|at| windows.remove(at).1));
    // SAFETY: a scene answers its session.
    let session: Obj = unsafe { send(scene, c"session", ()) };
    if let Some(window) = window {
        heard(Heard::SceneGone(session, window));
    }
}

extern "C" fn scene_window(delegate: Obj, _: Sel) -> Obj {
    SCENE_WINDOWS.with_borrow(|windows| windows.iter().find(|(of, _)| *of == delegate).map_or(std::ptr::null_mut(), |(_, window)| *window))
}

/// (UIKit may set a delegate's window: each keeps the one it made.)
extern "C" fn set_scene_window(_: Obj, _: Sel, _: Obj) {}

/// A window became the key window: the one in front, whose keyboard and dialogs these are; it
/// has the focus, the one before loses it.
extern "C" fn window_became_key(_: Obj, _: Sel, notice: Obj) {
    // SAFETY: a notice answers its object: here, the window.
    let window: Obj = unsafe { send(notice, c"object", ()) };
    let Some(now) = SEEN.with_borrow(|seen| seen.iter().find(|seen| seen.window == window).map(|seen| seen.view)) else { return };
    let was = FRONT.replace(now);
    if was != now {
        heard_in(was, Heard::Event(Event::Focused(false)));
        heard_in(now, Heard::Event(Event::Focused(true)));
    }
}

/// The program went to the background: the app hears it at once, as its window minimized, so
/// it can save its state before the system stops the program.
extern "C" fn entered_background(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Background(true));
    turn();
}

extern "C" fn entering_foreground(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Background(false));
}

extern "C" fn became_active(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Active(true));
}

extern "C" fn resigning_active(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Active(false));
}

/// The system ends the program: the app hears it as its window's close, at once.
extern "C" fn terminating(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Ending);
    turn();
}

extern "C" fn looks_changed(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Looks);
}

/// The user's locale or its formats changed: they are read again, and the window runs.
extern "C" fn locale_changed(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Locale);
}

/// The keyboard moves: how much of the view's bottom it will cover.
extern "C" fn keyboard_moved(_: Obj, _: Sel, notice: Obj) {
    let view = FRONT.get();
    // SAFETY: the notice's end frame, in the screen's coordinates, turned into the view's.
    let covered = unsafe {
        let name: Obj = send(notice, c"name", ());
        let hides = send::<Bool>(name, c"isEqualToString:", (UIKeyboardWillHideNotification,)) != 0;
        let info: Obj = send(notice, c"userInfo", ());
        let value: Obj = send(info, c"objectForKey:", (UIKeyboardFrameEndUserInfoKey,));
        if hides || value.is_null() || view.is_null() {
            0.0
        } else {
            let screen: Obj = send(send::<Obj>(class(c"UIScreen"), c"mainScreen", ()), c"coordinateSpace", ());
            let frame = send_rect(view, c"convertRect:fromCoordinateSpace:", (send_rect(value, c"CGRectValue", ()), screen));
            let bounds = send_rect(view, c"bounds", ());
            (bounds.size.height - frame.origin.y).clamp(0.0, bounds.size.height)
        }
    };
    heard(Heard::Keyboard(covered));
}

/// What the person asked the system for: dark or light, more contrast, less motion, larger
/// text (Dynamic Type, as the body text grows).
fn preferences(view: Obj) -> Preferences {
    // SAFETY: plain questions of the view's traits and the system.
    unsafe {
        let traits: Obj = send(view, c"traitCollection", ());
        let dark = send::<isize>(traits, c"userInterfaceStyle", ()) == 2;
        let metrics: Obj = send(class(c"UIFontMetrics"), c"defaultMetrics", ());
        let body = 17.0;
        let scaled = if metrics.is_null() { body } else { send::<f64>(metrics, c"scaledValueForValue:compatibleWithTraitCollection:", (body, traits)) };
        let contrast = UIAccessibilityDarkerSystemColorsEnabled();
        let high_contrast = contrast.then_some(if dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
        let text_scale = (scaled / body).clamp(0.5, 4.0) as f32;
        Preferences { dark, accent: None, high_contrast, reduced_motion: UIAccessibilityIsReduceMotionEnabled(), text_scale }
    }
}

/// The screen: where it lies, and its scale. (Its work area is all of it.)
fn screens() -> Vec<Screen> {
    // SAFETY: plain questions of the main screen.
    unsafe {
        let screen: Obj = send(class(c"UIScreen"), c"mainScreen", ());
        let bounds = send_rect(screen, c"bounds", ());
        let rect = Rect { x: 0.0, y: 0.0, w: bounds.size.width as f32, h: bounds.size.height as f32 };
        vec![Screen { rect, work_area: rect, scale: send::<f64>(screen, c"scale", ()) as f32 }]
    }
}

// ---- the view ----

/// The view controller's class: the status bar's text dark or light, as the page is.
fn controller_class() -> Obj {
    made_class(c"TautViewController", c"UIViewController", &[(c"preferredStatusBarStyle", status_bar_style as *const c_void, "q@:")], &[])
}

/// Light text (1) on a dark page, dark text (3) on a light one.
extern "C" fn status_bar_style(controller: Obj, _: Sel) -> isize {
    let dark = SEEN.with_borrow(|seen| seen.iter().any(|seen| seen.controller == controller && seen.dark));
    if dark { 1 } else { 3 }
}

/// The library's view class: touches, presses, layout, looks, and what readers ask.
pub(crate) fn view_class() -> Obj {
    let (touches, plain, yes) = ("v@:@@".to_string(), "v@:".to_string(), format!("{BOOL}@:"));
    let mut methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"touchesBegan:withEvent:", touches_began as *const c_void, touches.clone()),
        (c"touchesMoved:withEvent:", touches_moved as *const c_void, touches.clone()),
        (c"touchesEnded:withEvent:", touches_ended as *const c_void, touches.clone()),
        (c"touchesCancelled:withEvent:", touches_cancelled as *const c_void, touches.clone()),
        (c"pressesBegan:withEvent:", presses_began as *const c_void, touches.clone()),
        (c"pressesEnded:withEvent:", presses_ended as *const c_void, touches.clone()),
        (c"pressesCancelled:withEvent:", presses_ended as *const c_void, touches),
        (c"layoutSubviews", laid_out as *const c_void, plain.clone()),
        (c"safeAreaInsetsDidChange", insets_changed as *const c_void, plain),
        (c"traitCollectionDidChange:", traits_changed as *const c_void, "v@:@".into()),
        (c"canBecomeFirstResponder", yes_please as *const c_void, yes),
        (c"hovered:", hovered as *const c_void, "v@:@".into()),
        (c"scrolled:", scrolled as *const c_void, "v@:@".into()),
        (c"pinched:", pinched as *const c_void, "v@:@".into()),
        (c"pointerInteraction:regionForRequest:defaultRegion:", pointer_region as *const c_void, "@@:@@@".into()),
        (c"pointerInteraction:styleForRegion:", pointer_style as *const c_void, "@@:@@".into()),
    ];
    methods.extend(crate::ui_accessibility::view_methods());
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautView", c"UIView", &listed, &[c"UIPointerInteractionDelegate"])
}

extern "C" fn yes_please(_: Obj, _: Sel) -> Bool {
    1
}

/// Calls the superclass's (`UIView`'s, or `of`) own method on `to`.
///
/// # Safety
/// As `send`: `to` answers `name` with these arguments.
pub(crate) unsafe fn send_super<R>(to: Obj, of: &CStr, name: &CStr, args: impl Args) -> R {
    #[repr(C)]
    struct Super {
        receiver: Obj,
        class: Obj,
    }
    let asked = Super { receiver: to, class: class(of) };
    // SAFETY: `objc_msgSendSuper` takes the receiver and class in place of the receiver.
    unsafe { args.send(objc_msgSendSuper, (&raw const asked).cast_mut().cast(), sel(name)) }
}

/// A new layout (rotation, Split View, Stage Manager): the window is run and drawn at once,
/// within the system's animation.
extern "C" fn laid_out(view: Obj, _: Sel) {
    // SAFETY: UIView's own layout first.
    unsafe { send_super::<()>(view, c"UIView", c"layoutSubviews", ()) };
    heard_in(view, Heard::Resized);
    turn();
}

extern "C" fn insets_changed(view: Obj, _: Sel) {
    heard_in(view, Heard::Resized);
}

extern "C" fn traits_changed(view: Obj, _: Sel, _: Obj) {
    heard_in(view, Heard::Looks);
    heard_in(view, Heard::Resized);
}

/// Where a touch (or gesture) is, in units from the page's top left.
pub(crate) fn place_of(view: Obj, touch: Obj) -> Point {
    // SAFETY: a touch or gesture answers its place in a view.
    let at: NSPoint = unsafe { send(touch, c"locationInView:", (view,)) };
    let origin = origin_of(view);
    pt((at.x - origin.x) as f32, (at.y - origin.y) as f32)
}

/// The modifiers in UIKit's flags, as the library has them: Command and Control are both
/// `ctrl`.
fn mods_of(flags: isize) -> Mods {
    Mods { ctrl: flags & (CONTROL | COMMAND) != 0, shift: flags & SHIFT != 0, alt: flags & OPTION != 0 }
}

/// The touches of a set.
fn touches_in(set: Obj) -> Vec<Obj> {
    // SAFETY: a set answers its objects.
    array_items(unsafe { send(set, c"allObjects", ()) })
}

extern "C" fn touches_began(view: Obj, _: Sel, set: Obj, event: Obj) {
    touched(view, set, event, TouchPhase::Down);
}

extern "C" fn touches_moved(view: Obj, _: Sel, set: Obj, event: Obj) {
    touched(view, set, event, TouchPhase::Move);
}

extern "C" fn touches_ended(view: Obj, _: Sel, set: Obj, event: Obj) {
    touched(view, set, event, TouchPhase::Up);
}

extern "C" fn touches_cancelled(view: Obj, _: Sel, set: Obj, event: Obj) {
    touched(view, set, event, TouchPhase::Cancel);
}

/// Each touch of a set: a pointer's, a pencil's or a finger's.
fn touched(view: Obj, set: Obj, event: Obj, phase: TouchPhase) {
    for touch in touches_in(set) {
        // SAFETY: a touch answers its type.
        match unsafe { send::<isize>(touch, c"type", ()) } {
            POINTER => pointed(view, touch, event, phase),
            PENCIL => penned(view, touch, phase),
            _ => fingered(view, touch, phase),
        }
    }
}

/// A pointer's button, as its event says: the primary one, or the secondary (a right click).
fn pointer_button(event: Obj) -> Button {
    // SAFETY: an event answers its buttons (iOS 13.4); nil answers 0.
    let mask = unsafe { send::<isize>(event, c"buttonMask", ()) };
    if mask & 2 != 0 { Button::Right } else { Button::Left }
}

/// A trackpad's or mouse's click: the mouse.
fn pointed(view: Obj, touch: Obj, event: Obj, phase: TouchPhase) {
    let place = place_of(view, touch);
    heard_in(view, Heard::Event(Event::Move(place)));
    let held = INPUT.with_borrow(|input| input.held);
    let pressed = INPUT.with_borrow_mut(|input| match phase {
        TouchPhase::Down => Some(Event::Press(*input.pointer_button.insert(pointer_button(event)), held)),
        TouchPhase::Move => None,
        TouchPhase::Up | TouchPhase::Cancel => input.pointer_button.take().map(Event::Release),
    });
    pressed.into_iter().for_each(|event| heard_in(view, Heard::Event(event)));
}

/// What a pencil says beyond the mouse: pressure, tilt, touching.
#[cfg(feature = "touch")]
fn stylus_of(view: Obj, touch: Obj, touching: bool) -> Stylus {
    // SAFETY: a pencil's touch answers these.
    let (force, most, altitude, azimuth) =
        unsafe { (send::<f64>(touch, c"force", ()), send::<f64>(touch, c"maximumPossibleForce", ()), send::<f64>(touch, c"altitudeAngle", ()), send::<f64>(touch, c"azimuthAngleInView:", (view,))) };
    let pressure = if !touching {
        0.0
    } else if most > 0.0 {
        (force / most).clamp(0.0, 1.0) as f32
    } else {
        0.5
    };
    // Tilt to the right and toward the person from the angle above the screen and its bearing.
    let lean = altitude.tan().max(1e-6);
    let tilt = [(azimuth.cos() / lean).atan().to_degrees() as f32, (azimuth.sin() / lean).atan().to_degrees() as f32];
    Stylus { pressure, tilt, twist: 0.0, barrel: false, eraser: false, touching }
}

/// A pencil: the mouse, told first what the pencil does (with the feature `touch`).
fn penned(view: Obj, touch: Obj, phase: TouchPhase) {
    #[cfg(feature = "touch")]
    heard_in(view, Heard::Event(Event::Stylus(Some(stylus_of(view, touch, phase == TouchPhase::Down || phase == TouchPhase::Move)))));
    as_mouse(view, place_of(view, touch), phase);
    #[cfg(feature = "touch")]
    if matches!(phase, TouchPhase::Up | TouchPhase::Cancel) {
        heard_in(view, Heard::Event(Event::Stylus(None)));
    }
}

/// A finger: a touch (with the feature `touch`), or else the mouse, the first finger only.
fn fingered(view: Obj, touch: Obj, phase: TouchPhase) {
    let place = place_of(view, touch);
    let id = INPUT.with_borrow_mut(|input| finger_id(input, touch, phase));
    if cfg!(feature = "touch") {
        return heard_in(view, Heard::Event(Event::Touch(crate::touch::Touch { id, position: place, phase })));
    }
    let mouse = INPUT.with_borrow_mut(|input| {
        input.mouse = input.mouse.or((phase == TouchPhase::Down).then_some(touch));
        let mine = input.mouse == Some(touch);
        input.mouse = input.mouse.filter(|_| matches!(phase, TouchPhase::Down | TouchPhase::Move));
        mine
    });
    if mouse {
        as_mouse(view, place, phase);
    }
}

/// The interface's id for a touch: given as it lands, kept until it lifts.
fn finger_id(input: &mut Input, touch: Obj, phase: TouchPhase) -> u32 {
    let known = input.fingers.iter().position(|(of, _)| *of == touch);
    let id = match known {
        Some(at) => input.fingers[at].1,
        None => {
            input.next_finger = input.next_finger.wrapping_add(1);
            input.fingers.push((touch, input.next_finger));
            input.next_finger
        }
    };
    if matches!(phase, TouchPhase::Up | TouchPhase::Cancel) {
        input.fingers.retain(|(of, _)| *of != touch);
    }
    id
}

/// A finger or pencil as the mouse's left button, in the window of `view`: down where it lands,
/// moves, up as it lifts (and leaves with it).
fn as_mouse(view: Obj, place: Point, phase: TouchPhase) {
    let held = INPUT.with_borrow(|input| input.held);
    let events = match phase {
        TouchPhase::Down => [Some(Event::Move(place)), Some(Event::Press(Button::Left, held))],
        TouchPhase::Move => [Some(Event::Move(place)), None],
        TouchPhase::Up | TouchPhase::Cancel => [Some(Event::Release(Button::Left)), Some(Event::Leave)],
    };
    events.into_iter().flatten().for_each(|event| heard_in(view, Heard::Event(event)));
}

/// A gesture recognizer's state.
pub(crate) fn state_of(gesture: Obj) -> isize {
    // SAFETY: a gesture recognizer answers its state.
    unsafe { send(gesture, c"state", ()) }
}

/// A pointer or hovering pencil over the view: it moves; it leaves.
extern "C" fn hovered(view: Obj, _: Sel, gesture: Obj) {
    let event = if matches!(state_of(gesture), BEGAN | CHANGED) { Event::Move(place_of(view, gesture)) } else { Event::Leave };
    heard_in(view, Heard::Event(event));
}

/// A trackpad's or wheel's scrolling: the wheel, by the distance since the last. With Command
/// or Control held, it zooms, as Control with a wheel does elsewhere.
extern "C" fn scrolled(view: Obj, _: Sel, gesture: Obj) {
    if !matches!(state_of(gesture), BEGAN | CHANGED) {
        return;
    }
    // SAFETY: a pan gesture answers its distance, which is set back to nothing.
    let by: NSPoint = unsafe { send(gesture, c"translationInView:", (view,)) };
    unsafe { send::<()>(gesture, c"setTranslation:inView:", (NSPoint::default(), view)) };
    heard_in(view, Heard::Event(Event::Move(place_of(view, gesture))));
    let (right, down) = (-by.x as f32, -by.y as f32);
    let zoom = INPUT.with_borrow(|input| input.held.ctrl) && down != 0.0;
    heard_in(view, Heard::Event(if zoom { Event::Zoom(crate::touch::wheel_zoom(-down / NOTCH)) } else { Event::Wheel(right, down) }));
}

/// A trackpad's pinch: a zoom by the change since the last.
extern "C" fn pinched(view: Obj, _: Sel, gesture: Obj) {
    if !matches!(state_of(gesture), BEGAN | CHANGED) {
        return;
    }
    // SAFETY: a pinch answers its scale, which is set back to 1.
    let by = unsafe { send::<f64>(gesture, c"scale", ()) } as f32;
    unsafe { send::<()>(gesture, c"setScale:", (1.0f64,)) };
    heard_in(view, Heard::Event(Event::Move(place_of(view, gesture))));
    heard_in(view, Heard::Event(Event::Zoom(by.max(0.1))));
}

/// The pointer's shape: one region, the whole view.
extern "C" fn pointer_region(_: Obj, _: Sel, _: Obj, _: Obj, region: Obj) -> Obj {
    region
}

/// A text beam over text; else the system's pointer.
extern "C" fn pointer_style(_: Obj, _: Sel, interaction: Obj, _: Obj) -> Obj {
    if seen(view_of(interaction)).is_none_or(|seen| seen.cursor != Cursor::Text) {
        return std::ptr::null_mut();
    }
    // SAFETY: a beam shape of a usual length, vertical (2), in a style constrained to nothing.
    unsafe {
        let shape: Obj = send(class(c"UIPointerShape"), c"beamWithPreferredLength:axis:", (20.0f64, 2usize));
        send(class(c"UIPointerStyle"), c"styleWithShape:constrainedAxes:", (shape, 0usize))
    }
}

// ---- keys ----

/// The key a HID usage names; for another key, the letter it types without modifiers.
fn key_of(code: isize, letters: Obj) -> Option<Key> {
    let named = match code {
        0x28 | 0x58 => Key::Enter,
        0x29 => Key::Escape,
        0x2b => Key::Tab,
        0x2a => Key::Backspace,
        0x4c => Key::Delete,
        0x50 => Key::Left,
        0x4f => Key::Right,
        0x52 => Key::Up,
        0x51 => Key::Down,
        0x4a => Key::Home,
        0x4d => Key::End,
        0x4b => Key::PageUp,
        0x4e => Key::PageDown,
        0x2c => Key::Space,
        0x65 => Key::Menu,
        0x3a..=0x45 => Key::F((code - 0x3a + 1) as u8),
        _ => return letter_of(letters),
    };
    Some(named)
}

/// The letter a key types without modifiers, lower case. (UIKit gives arrows and such as
/// private-use characters; those are named above.)
fn letter_of(letters: Obj) -> Option<Key> {
    let letter = rust_string(letters)?.chars().next()?.to_lowercase().next()?;
    (!letter.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&letter)).then_some(Key::Char(letter))
}

/// The modifier a HID usage is: Control and Command, Shift, Option (left and right).
fn modifier(code: isize) -> Option<fn(&mut Mods) -> &mut bool> {
    match code {
        0xe0 | 0xe3 | 0xe4 | 0xe7 => Some(|mods| &mut mods.ctrl),
        0xe1 | 0xe5 => Some(|mods| &mut mods.shift),
        0xe2 | 0xe6 => Some(|mods| &mut mods.alt),
        _ => None,
    }
}

/// Keys that repeat while held, and those typing nothing (taken here, not by the text system).
fn repeats(key: Key) -> bool {
    matches!(key, Key::Backspace | Key::Delete | Key::Left | Key::Right | Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Enter | Key::Tab)
}

/// What a press is: its HID usage, its modifiers, and what it types without them. `None` for a
/// press without a key (a remote's).
fn press_of(press: Obj) -> Option<(isize, isize, Obj)> {
    // SAFETY: a press answers its key (iOS 13.4); a key answers these.
    unsafe {
        let key: Obj = send(press, c"key", ());
        (!key.is_null()).then(|| (send::<isize>(key, c"keyCode", ()), send::<isize>(key, c"modifierFlags", ()), send::<Obj>(key, c"charactersIgnoringModifiers", ())))
    }
}

/// Keys went down: each is heard as a key. Those that type text are left to the text system
/// (the superclass's), which hands what they type to the text box; the rest are taken here.
/// Called on the view and on the text input view.
pub(crate) extern "C" fn presses_began(view: Obj, _: Sel, presses: Obj, event: Obj) {
    let mut rest = Vec::new();
    for press in touches_in(presses) {
        let Some((code, flags, letters)) = press_of(press) else {
            rest.push(press);
            continue;
        };
        if pressed(view, code, flags, letters) {
            INPUT.with_borrow_mut(|input| input.passed.push(code));
            rest.push(press);
        }
    }
    pass_on(view, c"pressesBegan:withEvent:", &rest, event);
}

/// One key went down, in the window of `view`: heard as the key, or the modifiers held. Says
/// whether the text system should have it too (it types text).
fn pressed(view: Obj, code: isize, flags: isize, letters: Obj) -> bool {
    if let Some(bit) = modifier(code) {
        let held = INPUT.with_borrow_mut(|input| {
            *bit(&mut input.held) = true;
            input.held
        });
        heard_in(view, Heard::Event(Event::ModifiersChanged(held)));
        return false;
    }
    let held = mods_of(flags);
    let Some(key) = key_of(code, letters) else { return true };
    INPUT.with_borrow_mut(|input| {
        input.keys.push(code);
        input.repeating = repeats(key).then(|| (key, held, Instant::now() + REPEAT_AFTER));
    });
    heard_in(view, Heard::Event(Event::Key(key, held)));
    if repeats(key) {
        wake_at(Instant::now() + REPEAT_AFTER);
    }
    !repeats(key) && !held.ctrl
}

/// Keys came up.
pub(crate) extern "C" fn presses_ended(view: Obj, name: Sel, presses: Obj, event: Obj) {
    let mut rest = Vec::new();
    for press in touches_in(presses) {
        let Some((code, _, letters)) = press_of(press) else {
            rest.push(press);
            continue;
        };
        released(view, code, letters);
        let passed = INPUT.with_borrow_mut(|input| input.passed.iter().position(|of| *of == code).map(|at| input.passed.remove(at)));
        if passed.is_some() {
            rest.push(press);
        }
    }
    let ended = std::ptr::eq(name, sel(c"pressesEnded:withEvent:"));
    pass_on(view, if ended { c"pressesEnded:withEvent:" } else { c"pressesCancelled:withEvent:" }, &rest, event);
}

/// One key came up, in the window of `view`.
fn released(view: Obj, code: isize, letters: Obj) {
    if let Some(bit) = modifier(code) {
        let held = INPUT.with_borrow_mut(|input| {
            *bit(&mut input.held) = false;
            input.held
        });
        return heard_in(view, Heard::Event(Event::ModifiersChanged(held)));
    }
    INPUT.with_borrow_mut(|input| {
        input.keys.retain(|held| *held != code);
        input.repeating = input.repeating.filter(|_| !input.keys.is_empty());
    });
    if let Some(key) = key_of(code, letters) {
        heard_in(view, Heard::Event(Event::KeyUp(key)));
    }
}

/// Hands presses on to the superclass (the text system, then the responders above).
fn pass_on(view: Obj, name: &CStr, presses: &[Obj], event: Obj) {
    if presses.is_empty() {
        return;
    }
    // SAFETY: a set of the presses UIKit handed in, passed to UIView's own method.
    unsafe {
        let set: Obj = send(class(c"NSSet"), c"setWithArray:", (ns_array(presses),));
        send_super::<()>(view, c"UIView", name, (set, event));
    }
}

/// Whether a key that types nothing is held now: the text system's own deletion or line break
/// for it is not heard again.
pub(crate) fn key_held(key: Key) -> bool {
    INPUT.with_borrow(|input| input.repeating.is_some_and(|(held, ..)| held == key) || input.keys.iter().any(|code| key_of(*code, std::ptr::null_mut()) == Some(key)))
}

/// A held key repeats, as its time comes.
fn repeat_keys() {
    let due = INPUT.with_borrow_mut(|input| {
        let (key, held, at) = input.repeating?;
        let now = Instant::now();
        if at > now {
            return Some((None, at));
        }
        input.repeating = Some((key, held, now + REPEAT_EVERY));
        Some((Some(Event::Key(key, held)), now + REPEAT_EVERY))
    });
    let Some((event, next)) = due else { return };
    if let Some(event) = event {
        HEARD.with_borrow_mut(|heard| heard.push((FRONT.get(), Heard::Event(event))));
    }
    wake_at(next);
}

// ---- the windows ----

/// The windows and the interface shown in them: the first window's, which holds the second
/// windows' (each in a scene of its own, on iPad).
struct Host {
    app: Box<dyn FnMut(&mut Ui)>,
    ui: Ui,
    windows: Vec<Shown>,
    /// What is to show each window.
    renderer: crate::platform::Renderer,
    /// Second windows a scene was asked for, not yet connected.
    asked: Vec<Id>,
    rerun: bool,
    start: Instant,
}

/// A window: its view and page, and what its interface last asked of it.
struct Shown {
    /// The second window it shows (none: the first).
    known: Option<Id>,
    window: Obj,
    view: Obj,
    controller: Obj,
    /// Its scene's session (a second window's): what it is closed by.
    session: Obj,
    /// The layer showing the pixels, and where it lies in the view: the safe area above the
    /// keyboard, in points.
    page: Obj,
    area: NSRect,
    keyboard: f64,
    scale: f32,
    surface: Surface,
    /// The page's background, as last set.
    background: Color,
    pointer: Obj,
    /// Its title, as last asked (shown in the app switcher on iPad).
    title: String,
    /// What Metal keeps of the window (feature `gpu`).
    #[cfg(feature = "gpu")]
    screen: crate::gpu_ios::Screen,
}

/// Opens the program's window in `window` (its first scene's, or the screen's): the interface,
/// run once.
fn open(window: Obj) {
    let Some((title, renderer, app)) = STARTING.with_borrow_mut(Option::take) else { return };
    let Some(text) = SystemText::new() else {
        return eprintln!("taut: CoreText has no system font.");
    };
    let first = Shown::open(window, None, std::ptr::null_mut(), renderer);
    let mut ui = Ui::new(Box::new(text), Box::new(SystemClipboard));
    ui.set_preferences(preferences(first.view));
    ui.set_screens(screens());
    ui.set_native_file_dialog(true);
    crate::ui_accessibility::listen_if_running(first.view);
    let shown = Shown { title, ..first };
    HOST.set(Some(Host { app, ui, windows: vec![shown], renderer, asked: Vec::new(), rerun: true, start: Instant::now() }));
    RUNNING.store(true, Ordering::SeqCst);
    turn();
}

/// What hears a pointer and a pencil hovering, a trackpad's scrolling and pinching, and shapes
/// the pointer. Gives back the pointer's interaction (null before iOS 13.4).
fn listen_to_pointer(view: Obj) -> Obj {
    // SAFETY: gesture recognizers and an interaction made with the view as target and delegate,
    // handed to the view, which keeps them.
    unsafe {
        let made = |name: &CStr, action: &CStr| -> Obj {
            let gesture: Obj = send(send::<Obj>(class(name), c"alloc", ()), c"initWithTarget:action:", (view, sel(action)));
            send::<()>(gesture, c"setCancelsTouchesInView:", (0 as Bool,));
            send::<()>(view, c"addGestureRecognizer:", (gesture,));
            send::<()>(gesture, c"release", ());
            gesture
        };
        made(c"UIHoverGestureRecognizer", c"hovered:");
        let scrolling = made(c"UIPanGestureRecognizer", c"scrolled:");
        send::<()>(scrolling, c"setAllowedScrollTypesMask:", (3isize,));
        send::<()>(scrolling, c"setMaximumNumberOfTouches:", (0usize,));
        let pinching = made(c"UIPinchGestureRecognizer", c"pinched:");
        let pointer_only: Obj = send(class(c"NSNumber"), c"numberWithInteger:", (POINTER,));
        send::<()>(pinching, c"setAllowedTouchTypes:", (ns_array(&[pointer_only]),));
        if class(c"UIPointerInteraction").is_null() {
            return std::ptr::null_mut();
        }
        let interaction: Obj = send(send::<Obj>(class(c"UIPointerInteraction"), c"alloc", ()), c"initWithDelegate:", (view,));
        send::<()>(view, c"addInteraction:", (interaction,));
        send::<()>(interaction, c"release", ());
        interaction
    }
}

/// Changes what is kept of the window of `view`.
fn change_seen(view: Obj, change: impl FnOnce(&mut Seen)) {
    SEEN.with_borrow_mut(|seen| seen.iter_mut().find(|seen| seen.view == view).map(change));
}

/// Whether the system shows more than one scene of an app (iPad; not iPhone).
fn multiple_scenes() -> bool {
    // SAFETY: a plain question of the application.
    unsafe { send::<Bool>(send::<Obj>(class(c"UIApplication"), c"sharedApplication", ()), c"supportsMultipleScenes", ()) != 0 }
}

/// Asks the system for a scene for second window `id`, beside the scene of `beside`: it
/// connects with an activity naming the window (`asked_window`).
fn ask_for_scene(id: Id, title: &str, beside: Obj) {
    // SAFETY: an activity and options made, handed to the application, and released.
    unsafe {
        let activity: Obj = send(send::<Obj>(class(c"NSUserActivity"), c"alloc", ()), c"initWithActivityType:", (ns_string(SECOND_WINDOW),));
        send::<()>(activity, c"setTitle:", (ns_string(title),));
        let number: Obj = send(class(c"NSNumber"), c"numberWithUnsignedLongLong:", (id,));
        send::<()>(activity, c"setUserInfo:", (send::<Obj>(class(c"NSDictionary"), c"dictionaryWithObject:forKey:", (number, ns_string("window"))),));
        let options: Obj = send(class(c"UISceneActivationRequestOptions"), c"new", ());
        send::<()>(options, c"setRequestingScene:", (send::<Obj>(beside, c"windowScene", ()),));
        let application: Obj = send(class(c"UIApplication"), c"sharedApplication", ());
        let nothing = std::ptr::null_mut::<c_void>();
        send::<()>(application, c"requestSceneSessionActivation:userActivity:options:errorHandler:", (nothing, activity, options, nothing));
        send::<()>(options, c"release", ());
        send::<()>(activity, c"release", ());
    }
}

/// Brings a scene forward, by its session.
fn bring_scene_forward(session: Obj) {
    // SAFETY: the application activates the session's scene.
    unsafe {
        let application: Obj = send(class(c"UIApplication"), c"sharedApplication", ());
        let nothing = std::ptr::null_mut::<c_void>();
        send::<()>(application, c"requestSceneSessionActivation:userActivity:options:errorHandler:", (session, nothing, nothing, nothing));
    }
}

/// Has the system close a scene, by its session; it goes (`scene_gone`) once closed.
fn close_scene(session: Obj) {
    // SAFETY: the application destroys the session (and disconnects its scene).
    unsafe {
        let application: Obj = send(class(c"UIApplication"), c"sharedApplication", ());
        let nothing = std::ptr::null_mut::<c_void>();
        send::<()>(application, c"requestSceneSessionDestruction:options:errorHandler:", (session, nothing, nothing));
    }
}

impl Shown {
    /// Shows a window in `window`: a view controller, the view and its page, what hears the
    /// pointer, and the parts put on every view. It comes forward, and is measured in the next
    /// turn.
    fn open(window: Obj, known: Option<Id>, session: Obj, renderer: crate::platform::Renderer) -> Shown {
        // SAFETY: objects of UIKit and the library's classes made and set up on the main thread;
        // the controller keeps the view, the window the controller.
        let (view, controller, page) = unsafe {
            let bounds = send_rect(window, c"bounds", ());
            let view: Obj = send(send::<Obj>(view_class(), c"alloc", ()), c"initWithFrame:", (bounds,));
            send::<()>(view, c"setMultipleTouchEnabled:", (1 as Bool,));
            let controller: Obj = send(send::<Obj>(controller_class(), c"alloc", ()), c"init", ());
            send::<()>(controller, c"setView:", (view,));
            send::<()>(view, c"release", ());
            send::<()>(window, c"setRootViewController:", (controller,));
            send::<()>(controller, c"release", ());
            let page: Obj = send(class(c"CALayer"), c"layer", ());
            send::<()>(page, c"setContentsGravity:", (kCAGravityTopLeft,));
            send::<()>(page, c"setOpaque:", (1 as Bool,));
            send::<()>(send::<Obj>(view, c"layer", ()), c"addSublayer:", (page,));
            (view, controller, page)
        };
        let pointer = listen_to_pointer(view);
        SEEN.with_borrow_mut(|seen| seen.push(Seen { window, view, controller, origin: NSPoint::default(), dark: false, cursor: Cursor::Arrow, known }));
        if FRONT.get().is_null() {
            FRONT.set(view);
        }
        VIEW_PARTS.with_borrow(|parts| parts.iter().for_each(|add| add(view)));
        // SAFETY: the window shows and takes the keys (made key, it is the front window); the
        // view takes the keys of a hardware keyboard.
        unsafe {
            send::<()>(window, c"makeKeyAndVisible", ());
            send::<Bool>(view, c"becomeFirstResponder", ());
        }
        heard_in(view, Heard::Resized);
        #[cfg(not(feature = "gpu"))]
        let _ = renderer;
        #[rustfmt::skip]
        return Shown {
            known, window, view, controller, session, page, area: NSRect::default(), keyboard: 0.0, scale: 1.0, surface: Surface::default(),
            background: Color::NONE, pointer, title: String::new(),
            #[cfg(feature = "gpu")]
            screen: crate::gpu_ios::Screen::new(renderer),
        };
    }

    /// Takes in the view's size, safe area and scale now; says whether the page changed.
    fn measure(&mut self) -> bool {
        // SAFETY: plain questions of the view.
        let (bounds, insets, scale) = unsafe {
            let traits: Obj = send(self.view, c"traitCollection", ());
            (send_rect(self.view, c"bounds", ()), send::<Insets>(self.view, c"safeAreaInsets", ()), send::<f64>(traits, c"displayScale", ()))
        };
        let bottom = insets.bottom.max(self.keyboard);
        let width = (bounds.size.width - insets.left - insets.right).max(1.0);
        let area = NSRect::new(insets.left, insets.top, width, (bounds.size.height - insets.top - bottom).max(1.0));
        let scale = if scale > 0.0 { scale as f32 } else { self.scale };
        let changed = (area, scale) != (self.area, self.scale);
        (self.area, self.scale) = (area, scale);
        change_seen(self.view, |seen| seen.origin = area.origin);
        changed
    }

    /// The page's size, in points.
    fn size(&self) -> Size {
        Size { w: self.area.size.width as f32, h: self.area.size.height as f32 }
    }

    /// Does what the interface asked of the window: a title, a pointer shape, the page's
    /// background and the status bar's text.
    fn follow(&mut self, title: Option<String>, cursor: Cursor, background: Color) {
        if let Some(title) = title {
            self.set_title(title);
        }
        let mut was = cursor;
        change_seen(self.view, |seen| was = std::mem::replace(&mut seen.cursor, cursor));
        if was != cursor && !self.pointer.is_null() {
            // SAFETY: the interaction asks its delegate for the style again.
            unsafe { send::<()>(self.pointer, c"invalidate", ()) };
        }
        if std::mem::replace(&mut self.background, background) != background {
            self.paint_background(background);
        }
    }

    /// Names its scene (shown in the app switcher on iPad).
    fn set_title(&mut self, title: String) {
        // SAFETY: a window answers its scene (nil without scenes); a scene takes a title.
        unsafe { send::<()>(send::<Obj>(self.window, c"windowScene", ()), c"setTitle:", (ns_string(&title),)) };
        self.title = title;
    }

    /// The view's background, around the page; the status bar's text dark or light on it.
    fn paint_background(&mut self, background: Color) {
        let channel = |shift: u32| f64::from((background.0 >> shift) & 0xff) / 255.0;
        let (red, green, blue) = (channel(16), channel(8), channel(0));
        let dark = 0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5;
        change_seen(self.view, |seen| seen.dark = dark);
        // SAFETY: a colour made and handed to the view, which keeps it.
        unsafe {
            let color: Obj = send(class(c"UIColor"), c"colorWithRed:green:blue:alpha:", (red, green, blue, 1.0f64));
            send::<()>(self.view, c"setBackgroundColor:", (color,));
            send::<()>(self.controller, c"setNeedsStatusBarAppearanceUpdate", ());
        }
    }

    /// Hands the page its pixels, whole, and puts it over the safe area.
    fn present(&self) {
        let (width, height) = (self.surface.width, self.surface.height);
        if width == 0 || height == 0 {
            return;
        }
        // SAFETY: the bytes are copied into the data the picture keeps; every object made here
        // is released once handed on (the layer keeps the picture).
        unsafe {
            let bytes = CFDataCreate(std::ptr::null(), self.surface.pixels.as_ptr().cast(), (width * height * 4) as isize);
            let provider = CGDataProviderCreateWithCFData(bytes);
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            let picture = CGImageCreate(width, height, 8, 32, width * 4, space, SOLID, provider, std::ptr::null(), false, 0);
            send::<()>(class(c"CATransaction"), c"begin", ());
            send::<()>(class(c"CATransaction"), c"setDisableActions:", (1 as Bool,));
            send::<()>(self.page, c"setFrame:", (self.area,));
            send::<()>(self.page, c"setContentsScale:", (f64::from(self.scale),));
            send::<()>(self.page, c"setContents:", (picture,));
            send::<()>(class(c"CATransaction"), c"commit", ());
            CGImageRelease(picture);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
            CFRelease(bytes);
        }
    }

    /// A second window's view is gone: nothing more is kept of it, and the first window is in
    /// front where this one was. (Its window is let go of as its scene goes.)
    fn close(self) {
        SEEN.with_borrow_mut(|seen| seen.retain(|seen| seen.view != self.view));
        crate::ui_accessibility::forget(self.view);
        if FRONT.get() == self.view {
            FRONT.set(SEEN.with_borrow(|seen| seen.first().map_or(std::ptr::null_mut(), |first| first.view)));
        }
    }
}

impl Host {
    /// The window in front.
    fn front(&self) -> usize {
        self.windows.iter().position(|shown| shown.view == FRONT.get()).unwrap_or(0)
    }

    /// One thing UIKit said of the window of `view` (or of the front window).
    fn take(&mut self, view: Obj, what: Heard) {
        let at = self.windows.iter().position(|shown| shown.view == view).unwrap_or_else(|| self.front());
        match what {
            Heard::Event(event) => self.hear(at, event),
            Heard::Resized => self.rerun |= self.windows[at].measure(),
            Heard::Looks => self.rerun |= self.ui.set_preferences(preferences(self.windows[0].view)) | self.ui.set_screens(screens()),
            Heard::Locale => {
                crate::locale::changed();
                self.rerun = true;
            }
            Heard::Keyboard(covered) => {
                self.windows[at].keyboard = covered;
                self.rerun |= self.windows[at].measure();
            }
            Heard::Background(away) => self.background_changed(away),
            Heard::Active(on) => self.hear(at, Event::Focused(on)),
            Heard::Ending => {
                self.ui.request_close();
                self.rerun = true;
            }
            Heard::Other(work) => self.rerun |= within(&mut self.ui, self.windows[at].known, work) == Some(true),
            Heard::SceneConnected(window, session, known) => self.connected(window, session, known),
            Heard::SceneGone(session, window) => self.gone(session, window),
        }
    }

    /// Passes what the person did in a window to its interface, noting whether it must run. A
    /// window a dialog keeps from input hears none; a press on it brings the dialog forward.
    fn hear(&mut self, at: usize, event: Event) {
        let blocked = self.ui.window_blocked(self.windows[at].known.unwrap_or(0));
        if blocked && !matches!(event, Event::Focused(_) | Event::Minimized(_) | Event::Leave) {
            let dialog = |shown: &&Shown| shown.known.and_then(|id| self.ui.window_opening(id)).is_some_and(|opening| opening.kind == WindowKind::Dialog);
            if let (Event::Press(..), Some(dialog)) = (&event, self.windows.iter().rev().find(dialog)) {
                bring_scene_forward(dialog.session);
            }
            return;
        }
        self.rerun |= within(&mut self.ui, self.windows[at].known, |ui| ui.event(event)) == Some(true);
    }

    /// The program went to the background, or came back: every window's interface hears it as
    /// its window minimized, or restored.
    fn background_changed(&mut self, away: bool) {
        for shown in &mut self.windows {
            #[cfg(feature = "gpu")]
            shown.screen.set_away(away);
            self.rerun |= within(&mut self.ui, shown.known, |ui| ui.event(Event::Minimized(away))) == Some(true);
        }
    }

    /// A scene connected: a second window asked for, still described and not shown, shows in
    /// it. Another (one the person made, or one the system brought back from an earlier run)
    /// has nothing to show, and closes.
    fn connected(&mut self, window: Obj, session: Obj, known: Option<Id>) {
        let described = |id: &Id| self.ui.windows().any(|(of, ..)| of == *id) && self.windows.iter().all(|shown| shown.known != Some(*id));
        let Some(id) = known.filter(described) else { return close_scene(session) };
        self.asked.retain(|asked| *asked != id);
        if self.ui.listening {
            self.ui.in_window(id, |ui| ui.set_screen_reader(true));
        }
        let mut shown = Shown::open(window, Some(id), session, self.renderer);
        shown.measure();
        if let Some((_, title, _)) = self.ui.windows().find(|(of, ..)| *of == id) {
            shown.set_title(title.to_string());
        }
        self.windows.push(shown);
        self.rerun = true;
    }

    /// A scene went: a second window's interface hears its window was closed (if its app keeps
    /// it, a scene is asked for again). The scene's window is let go of, unless it is the
    /// first's, kept for good.
    fn gone(&mut self, session: Obj, window: Obj) {
        if let Some(at) = self.windows.iter().skip(1).position(|shown| shown.session == session).map(|at| at + 1) {
            let known = self.windows[at].known;
            self.rerun |= known.and_then(|id| self.ui.in_window(id, Ui::request_close)).is_some();
            self.windows.remove(at).close();
        }
        if window != self.windows[0].window {
            // SAFETY: the window made as the scene connected, released once, after it went.
            unsafe { send::<()>(window, c"release", ()) };
        }
    }

    /// Runs the interface, then does what it asks of the windows.
    fn describe(&mut self) {
        for shown in &self.windows {
            within(&mut self.ui, shown.known, |ui| ui.resize(shown.size(), shown.scale));
        }
        self.ui.run(&mut self.app);
        self.rerun = false;
        if self.ui.take_closing() == Some(Ok(())) || self.ui.quitting() {
            // SAFETY: the program ends, as the app asked.
            unsafe { exit(0) }
        }
        self.keep_up();
        (0..self.windows.len()).for_each(|at| self.follow(at));
        if let Some(typing) = TYPING.get() {
            let front = &self.windows[self.front()];
            within(&mut self.ui, front.known, |ui| typing(ui, front.area));
        }
        (0..self.windows.len()).for_each(|at| self.follow_parts(at));
        if let Some(show) = NOTIFICATIONS.get() {
            show();
        }
        if self.ui.listening {
            self.tell_readers();
        }
    }

    /// Closes the scenes of second windows the app stopped describing, and asks for a scene for
    /// each one described the first time (where the system shows more than one).
    fn keep_up(&mut self) {
        for at in (1..self.windows.len()).rev() {
            if !self.ui.windows().any(|(id, ..)| Some(id) == self.windows[at].known) {
                close_scene(self.windows[at].session);
                self.windows.remove(at).close();
            }
        }
        self.asked.retain(|id| self.ui.windows().any(|(of, ..)| of == *id));
        if !multiple_scenes() {
            return;
        }
        let shown = |id: &Id| self.asked.contains(id) || self.windows.iter().any(|shown| shown.known == Some(*id));
        let new: Vec<(Id, String)> = self.ui.windows().filter(|(id, ..)| !shown(id)).map(|(id, title, _)| (id, title.to_string())).collect();
        for (id, title) in new {
            ask_for_scene(id, &title, self.windows[0].window);
            self.asked.push(id);
        }
    }

    /// Does what a window's interface asks of the window.
    fn follow(&mut self, at: usize) {
        let asks = |ui: &mut Ui| (ui.take_title(), ui.cursor(), ui.theme.bg.solid());
        if let Some((title, cursor, background)) = within(&mut self.ui, self.windows[at].known, asks) {
            self.windows[at].follow(title, cursor, background);
        }
    }

    /// Does what a window's interface asks of the parts handed to the window: the system's file
    /// dialog, drops, dragging out.
    fn follow_parts(&mut self, at: usize) {
        let view = self.windows[at].view;
        for part in [FILE_DIALOG.get(), DROPS.get(), DRAG_OUT.get()].into_iter().flatten() {
            within(&mut self.ui, self.windows[at].known, |ui| part(ui, view));
        }
    }

    /// Once a reader asked: each window's interface tells. Says whether that is news, for the
    /// interface to run.
    fn listen(&mut self) -> bool {
        let mut news = false;
        for shown in &self.windows {
            news |= within(&mut self.ui, shown.known, crate::ui_accessibility::listen) == Some(true);
        }
        news
    }

    /// After a run a reader reads: each window's elements follow its nodes.
    fn tell_readers(&mut self) {
        for shown in &self.windows {
            let origin = (shown.area.origin.x, shown.area.origin.y);
            within(&mut self.ui, shown.known, |ui| crate::ui_accessibility::keep(shown.view, origin, ui));
        }
    }

    /// Draws what changed in a window and shows it: through Metal (feature `gpu`), else by
    /// handing the pixels to its page.
    fn draw(&mut self, at: usize) {
        #[cfg(feature = "gpu")]
        if self.through_gpu(at) {
            return;
        }
        let shown = &mut self.windows[at];
        let surface = &mut shown.surface;
        let changed = within(&mut self.ui, shown.known, |ui| {
            let changed = surface.update(ui);
            ui.changed();
            changed
        });
        if changed.is_some_and(|changed| !changed.is_empty()) {
            shown.present();
        }
    }

    /// One turn: takes in what happened, runs the interface if needed, draws what changed, and
    /// has the timer bring the next turn the interface asks for.
    fn turn(&mut self) {
        self.ui.set_time(self.start.elapsed());
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO);
        for (view, what) in HEARD.with_borrow_mut(std::mem::take) {
            self.take(view, what);
        }
        self.rerun |= self.listen();
        if self.rerun {
            self.describe();
        }
        (0..self.windows.len()).for_each(|at| self.draw(at));
        if let Some(wait) = self.ui.wake_in() {
            wake_at(Instant::now() + wait);
        }
    }
}

/// The windows shown through Metal (feature `gpu`).
#[cfg(feature = "gpu")]
impl Host {
    /// Draws what changed in a window through Metal, where it shows the window. Says whether it
    /// did; if not, the CPU is to (all of the window, where Metal stopped showing it).
    fn through_gpu(&mut self, at: usize) -> bool {
        let shown = &mut self.windows[at];
        // SAFETY: a view answers its layer.
        let under: Obj = unsafe { send(shown.view, c"layer", ()) };
        let (screen, surface, area, scale) = (&mut shown.screen, &mut shown.surface, shown.area, shown.scale);
        let drawn = within(&mut self.ui, shown.known, |ui| {
            let drawn = screen.draw(ui, surface, under, area, scale);
            ui.renderer = screen.renderer();
            drawn
        });
        if drawn == Some(crate::platform::ShownBy::CpuAfresh) {
            shown.surface = Surface::default();
        }
        drawn == Some(crate::platform::ShownBy::Gpu)
    }
}

/// One turn of the window, unless one is running (then the queued one comes after it).
pub(crate) fn turn() {
    repeat_keys();
    let _pool = Pool::new();
    HOST.with(|host| {
        let Ok(mut host) = host.try_borrow_mut() else { return queue_turn() };
        if let Some(host) = host.as_mut() {
            host.turn();
        }
    });
}

/// Opens the window and runs the app in it, for the program's life: UIKit runs the program
/// from here, and never returns. A run whose app asks to close the window ends the program.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    let _pool = Pool::new();
    // SAFETY: a plain question.
    if unsafe { send::<Bool>(class(c"NSThread"), c"isMainThread", ()) } == 0 {
        return eprintln!("taut: iOS shows windows only from the program's main thread.");
    }
    STARTING.set(Some((window.title.to_string(), window.renderer, Box::new(app))));
    let delegate = delegate_class();
    // The program's arguments, kept for its life as UIKit asks.
    let arguments: Vec<*mut c_char> = std::env::args().filter_map(|argument| std::ffi::CString::new(argument).ok()).map(std::ffi::CString::into_raw).collect();
    let arguments = Box::leak(arguments.into_boxed_slice());
    // SAFETY: UIKit takes the program's arguments and the delegate class's name, and runs the
    // program from here on.
    unsafe {
        let name: Obj = send(delegate, c"description", ());
        UIApplicationMain(arguments.len() as i32, arguments.as_mut_ptr(), std::ptr::null_mut(), name);
    }
}

/// Opens a link with the app the system opens it with, from the main thread. Says it was
/// handed on.
pub(crate) fn open_link(link: &str) -> bool {
    let link = link.to_string();
    on_main(Box::new(move || open_now(&link)));
    true
}

fn open_now(link: &str) {
    // SAFETY: a URL made from the text (nil if it is none), handed to the application.
    unsafe {
        let url: Obj = send(class(c"NSURL"), c"URLWithString:", (ns_string(link),));
        let app: Obj = send(class(c"UIApplication"), c"sharedApplication", ());
        if !url.is_null() {
            let options: Obj = send(class(c"NSDictionary"), c"dictionary", ());
            send::<()>(app, c"openURL:options:completionHandler:", (url, options, std::ptr::null_mut::<c_void>()));
        }
    }
}

// ---- the clipboard ----

/// The system clipboard (the general pasteboard), for an interface. Reading what another app
/// put there asks the person first (iOS 16), unless they pasted through the system's own
/// Paste; what the program put there itself is read back without asking.
struct SystemClipboard;

/// What the program last copied of one kind, and the pasteboard's change count right after:
/// while the count holds, the pasteboard holds that.
type Copied<T> = &'static std::thread::LocalKey<RefCell<Option<(isize, T)>>>;

/// A picture: its width, and its pixels.
type Picture = (usize, Vec<u32>);

thread_local! {
    static COPIED_TEXT: RefCell<Option<(isize, String)>> = const { RefCell::new(None) };
    static COPIED_PICTURE: RefCell<Option<(isize, Picture)>> = const { RefCell::new(None) };
    static COPIED_FILES: RefCell<Option<(isize, Vec<PathBuf>)>> = const { RefCell::new(None) };
}

/// The general pasteboard.
pub(crate) fn pasteboard() -> Obj {
    // SAFETY: a plain question.
    unsafe { send(class(c"UIPasteboard"), c"generalPasteboard", ()) }
}

/// The pasteboard's change count: asking it asks the person nothing.
fn change_count() -> isize {
    // SAFETY: a plain question of the pasteboard.
    unsafe { send(pasteboard(), c"changeCount", ()) }
}

/// Notes what the program just put on the pasteboard.
fn keep_copied<T>(kind: Copied<T>, what: T) {
    kind.set(Some((change_count(), what)));
}

/// What the program put on the pasteboard, while it is still there.
fn read_copied<T: Clone>(kind: Copied<T>) -> Option<T> {
    let count = change_count();
    kind.with_borrow(|copied| copied.as_ref().filter(|(at, _)| *at == count).map(|(_, what)| what.clone()))
}

/// The system's Paste: the pasteboard's text read now, for the paste that follows.
pub(crate) fn paste_now() {
    let _pool = Pool::new();
    // SAFETY: a plain question of the pasteboard.
    let text = rust_string(unsafe { send(pasteboard(), c"string", ()) });
    PASTED.set(text);
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        if let Some(text) = PASTED.take().or_else(|| read_copied(&COPIED_TEXT)) {
            return Some(text);
        }
        let _pool = Pool::new();
        // SAFETY: a plain question of the pasteboard.
        rust_string(unsafe { send(pasteboard(), c"string", ()) })
    }
    fn set(&mut self, text: &str) {
        let _pool = Pool::new();
        // SAFETY: the pasteboard takes the text, in place of what it held.
        unsafe { send::<()>(pasteboard(), c"setString:", (ns_string(text),)) };
        keep_copied(&COPIED_TEXT, text.into());
    }
    /// Pictures and files, once an interface asked for either: see `PicturesAndFiles`.
    fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
        (PICTURES.get()?.picture)(self)
    }
    fn set_picture(&mut self, width: usize, pixels: &[u32]) {
        PICTURES.get().map_or((), |its| (its.set_picture)(self, width, pixels))
    }
    fn files(&mut self) -> Vec<PathBuf> {
        PICTURES.get().map_or(Vec::new(), |its| (its.files)(self))
    }
    fn set_files(&mut self, files: &[PathBuf]) {
        PICTURES.get().map_or((), |its| (its.set_files)(self, files))
    }
}

thread_local! {
    /// The clipboard's picture and file code: none until an interface on this thread asks for
    /// one (see `PicturesAndFiles`).
    static PICTURES: Cell<Option<PicturesAndFiles<SystemClipboard>>> = const { Cell::new(None) };
}

/// Hands the clipboard its picture and file code, on this thread (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    let (picture, set_picture, files, set_files) = (SystemClipboard::picture_on, SystemClipboard::put_picture, SystemClipboard::files_on, SystemClipboard::put_files);
    PICTURES.set(Some(PicturesAndFiles { picture, set_picture, files, set_files }));
}

impl SystemClipboard {
    /// The picture on the pasteboard, as PNG.
    fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        if let Some(own) = read_copied(&COPIED_PICTURE) {
            return Some(own);
        }
        let _pool = Pool::new();
        // SAFETY: plain questions of the pasteboard and its image.
        let png = unsafe { UIImagePNGRepresentation(send(pasteboard(), c"image", ())) };
        crate::png::decode(&bytes_of(png))
    }
    /// Puts a picture on the pasteboard as PNG.
    fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
        let _pool = Pool::new();
        // SAFETY: the pasteboard takes the bytes as a PNG, in place of what it held.
        unsafe { send::<()>(pasteboard(), c"setData:forPasteboardType:", (ns_data(&png), ns_string("public.png"))) };
        keep_copied(&COPIED_PICTURE, (width, pixels.to_vec()));
    }
    /// The files on the pasteboard (see `ios_files::pasted_files`).
    fn files_on(&mut self) -> Vec<PathBuf> {
        if let Some(own) = read_copied(&COPIED_FILES) {
            return own;
        }
        let _pool = Pool::new();
        crate::ios_files::pasted_files(pasteboard())
    }
    /// Puts files on the pasteboard, for another app to paste.
    fn put_files(&mut self, files: &[PathBuf]) {
        let _pool = Pool::new();
        crate::ios_files::put_files(pasteboard(), files);
        keep_copied(&COPIED_FILES, files.to_vec());
    }
}
