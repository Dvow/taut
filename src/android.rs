//! taut on Android: one window, the activity's, through `NativeActivity`. The app is a shared
//! library the activity loads; the library exports `ANativeActivity_onCreate`, which calls the
//! app's own `android_main` (it calls `taut::run`, as `main` does elsewhere):
//!
//! ```ignore
//! #[cfg(target_os = "android")]
//! #[unsafe(no_mangle)]
//! extern "C" fn android_main() {
//!     main()
//! }
//! ```
//!
//! Everything runs on the main thread, in the activity's callbacks and the main looper's: `run`
//! returns at once, and the interface runs when input, a timer or a [`Waker`] says. Frames are
//! drawn by the CPU into the window's buffer (`ANativeWindow_lock`), inside the system bars,
//! the display cutout and the soft keyboard (window insets), with the theme's background
//! under them; with the feature `gpu`, shown through Vulkan or OpenGL ES instead
//! (`gpu_android`). The interface outlives its activity: rotated, or recreated by the system, the
//! new activity shows the same interface. Java is reached through JNI (`android_jni`), and the
//! few classes the system's Java interfaces need are written at run time (`android_dex`): a
//! view that takes the soft keyboard's text and window insets, and the node provider screen
//! readers read (`android_access`). The soft keyboard's typing and what it composes are
//! `android_input_method`'s, handed over by the first text box.
//!
//! The system parts (the file dialog, notifications, the clipboard's pictures and files, drops
//! and drags, spelling, printing) each join the interface's runs the first time an app uses one
//! (`add_part`): a program that never does carries none of them. They need the package's own
//! classes, which `examples/apk.rs` writes.
use crate::android_input_method::INPUT_METHOD;
use crate::android_jni::{self as jni, Env, Native, Obj, Value};
use crate::android_sys::*;
use crate::android_text::SystemText;
use crate::event::{Button, Event, Key, Mods};
use crate::geom::{Size, pt};
use crate::platform::{Clipboard, PicturesAndFiles, Window};
use crate::preferences::Preferences;
use crate::surface::Surface;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Key codes (`AKEYCODE_*`).
const BACK: i32 = 4;
const DEL: i32 = 67;
const FORWARD_DEL: i32 = 112;

/// What the activity and its window said, for the interface's next turn.
pub(crate) enum Heard {
    Event(Event),
    /// Work on the interface (a screen reader's request); says whether it must run.
    Other(Box<dyn FnOnce(&mut Ui) -> bool>),
    /// The back button: closes a popup, else asks the app to close.
    Back,
    /// The window or its insets changed size, or the display settings changed.
    Looks,
    /// The user's locale or its formats may have changed: read again, and if they did,
    /// everything is written again.
    Locale,
    /// The window must be shown whole again.
    Redraw,
}

/// What the library keeps of the activity, on the main thread.
struct Shell {
    activity: *mut Activity,
    /// The window, while there is one.
    window: *mut c_void,
    queue: *mut c_void,
    /// The library's view (`taut.Host`), and its classes.
    view: Obj,
    classes: [Obj; 3],
    /// What window insets keep clear, in pixels: left, top, right, bottom.
    insets: [i32; 4],
    /// The activity is in the foreground.
    resumed: bool,
    /// A text box has focus, and what it holds.
    typing: bool,
    typed: Typed,
    /// The mouse buttons held, and the finger acting as the mouse (without the feature
    /// `touch`).
    buttons: i32,
    finger: Option<i32>,
    /// The system drags what the app dragged out (`android_drag_drop`): the touch it began
    /// with is the system's, and its cancelling is not the pointer coming up.
    dragging_out: bool,
    /// Pixels a unit.
    scale: f32,
}

thread_local! {
    static SHELL: RefCell<Shell> = const { RefCell::new(Shell {
        activity: std::ptr::null_mut(),
        window: std::ptr::null_mut(),
        queue: std::ptr::null_mut(),
        view: std::ptr::null_mut(),
        classes: [std::ptr::null_mut(); 3],
        insets: [0; 4],
        resumed: false,
        typing: false,
        typed: Typed::Text,
        buttons: 0,
        finger: None,
        dragging_out: false,
        scale: 1.0,
    }) };
    static HEARD: RefCell<Vec<Heard>> = const { RefCell::new(Vec::new()) };
    /// The interface, once `run` opened it.
    static HOST: RefCell<Option<Box<dyn Turns>>> = const { RefCell::new(None) };
    /// A turn is running: what is heard meanwhile waits for the next.
    static TURNING: Cell<bool> = const { Cell::new(false) };
}

/// The system parts in use: what each does around a run. (Parts join from any thread: a face
/// is named on any.)
static PARTS: std::sync::Mutex<Vec<Part>> = std::sync::Mutex::new(Vec::new());

/// The parts in use, now.
fn parts() -> Vec<Part> {
    PARTS.lock().unwrap_or_else(|held| held.into_inner()).clone()
}

/// A system part's work around the interface's runs (see the module).
#[derive(Clone, Copy)]
pub(crate) struct Part {
    /// Which part: each joins once.
    pub name: &'static str,
    /// Before a run: takes in what the system answered meanwhile; says whether to run.
    pub before: fn(&mut Ui) -> bool,
    /// After a run: does what the run asked of the system.
    pub after: fn(&mut Ui),
}

/// The parts in use, as one part, once one joined: reached no other way, so a program using none
/// carries none of it.
static ALL_PARTS: std::sync::OnceLock<Part> = std::sync::OnceLock::new();

/// Has a system part join the interface's runs, from the next on.
pub(crate) fn add_part(part: Part) {
    ALL_PARTS.get_or_init(|| Part { name: "all", before: parts_before, after: parts_after });
    let mut parts = PARTS.lock().unwrap_or_else(|held| held.into_inner());
    if !parts.iter().any(|known| known.name == part.name) {
        parts.push(part);
    }
}

/// The parts' work before a run: whether any says to run.
fn parts_before(ui: &mut Ui) -> bool {
    parts().iter().fold(false, |run, part| (part.before)(ui) | run)
}

/// The parts' work after a run.
fn parts_after(ui: &mut Ui) {
    parts().iter().for_each(|part| (part.after)(ui));
}

/// The looper's timer, and the waker's pipe (its writing end).
static TIMER: AtomicI32 = AtomicI32::new(-1);
static PIPE: AtomicI32 = AtomicI32::new(-1);
static WOKEN: AtomicBool = AtomicBool::new(false);
/// The last activity's Java object, for threads of the library's own (`open_link`).
static LAST_ACTIVITY: std::sync::atomic::AtomicPtr<c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// Asks the window to run its interface again; can be sent from any thread. (An app learning
/// something in the background, say a changed file, wakes the window with this.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;

impl Waker {
    /// Has the interface run once more, soon, on its own thread, from whatever thread calls
    /// this.
    pub fn wake(&self) {
        let end = PIPE.load(Ordering::SeqCst);
        if end < 0 || WOKEN.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: one byte into the library's own pipe, never closed.
        if unsafe { write(end, [1u8].as_ptr().cast(), 1) } != 1 {
            WOKEN.store(false, Ordering::SeqCst);
        }
    }
}

/// Notes what was heard, for the next turn.
pub(crate) fn heard(what: Heard) {
    HEARD.with_borrow_mut(|heard| heard.push(what));
}

/// Runs a turn now, unless one is running (it then runs again as soon as it ends).
pub(crate) fn kick() {
    if TURNING.get() {
        return arm(Some(Duration::ZERO));
    }
    TURNING.set(true);
    let patience = HOST.with_borrow_mut(|host| host.as_mut().map(|host| host.turn()));
    TURNING.set(false);
    arm(patience.flatten());
}

/// Runs the looper's timer once after `wait`, or stops it.
fn arm(wait: Option<Duration>) {
    let timer = TIMER.load(Ordering::SeqCst);
    let wait = if HEARD.with_borrow(|heard| heard.is_empty()) { wait } else { Some(Duration::ZERO) };
    let value = wait.map_or([0, 0], |wait| [wait.as_secs() as i64, i64::from(wait.subsec_nanos().max(u32::from(wait.is_zero())))]);
    // SAFETY: the library's own timer, set once, never repeating.
    unsafe { timerfd_settime(timer, 0, &TimerSpec { interval: [0, 0], value }, std::ptr::null_mut()) };
}

/// The activity's main thread's interface to Java.
fn env() -> Env {
    jni::env().unwrap_or(Env(std::ptr::null_mut()))
}

/// The activity's Java object (null once it is gone).
pub(crate) fn activity() -> Obj {
    SHELL.with_borrow(|shell| if shell.activity.is_null() { std::ptr::null_mut() } else { unsafe { (*shell.activity).clazz } })
}

/// The library's classes: `taut.Host`, `taut.Nodes`, `taut.Typing`.
pub(crate) fn classes() -> [Obj; 3] {
    SHELL.with_borrow(|shell| shell.classes)
}

/// The library's view, and pixels a unit, and the insets.
pub(crate) fn view() -> (Obj, f32, [i32; 4]) {
    SHELL.with_borrow(|shell| (shell.view, shell.scale, shell.insets))
}

// ---- the activity ----

/// Where the system starts native code in a `NativeActivity`: the callbacks are set, the
/// library's view and classes made, and the app's `android_main` called (unless its interface
/// is still open from an activity before this one: then it shows here).
///
/// # Safety
/// The system calls it on the main thread with its activity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ANativeActivity_onCreate(activity: *mut Activity, _saved: *mut c_void, _saved_size: usize) {
    // SAFETY: the activity is alive until `on_destroy`; its callbacks are ours to set.
    let callbacks = unsafe {
        jni::set_main(Env((*activity).env), (*activity).vm);
        &mut *(*activity).callbacks
    };
    callbacks.on_resume = Some(on_resume);
    callbacks.on_pause = Some(on_pause);
    callbacks.on_destroy = Some(on_destroy);
    callbacks.on_window_focus_changed = Some(on_focus);
    callbacks.on_native_window_created = Some(on_window_created);
    callbacks.on_native_window_resized = Some(on_window_resized);
    callbacks.on_native_window_redraw_needed = Some(on_redraw_needed);
    callbacks.on_native_window_destroyed = Some(on_window_destroyed);
    callbacks.on_input_queue_created = Some(on_queue_created);
    callbacks.on_input_queue_destroyed = Some(on_queue_destroyed);
    callbacks.on_configuration_changed = Some(on_configuration_changed);
    callbacks.on_save_instance_state = Some(on_save);
    SHELL.with_borrow_mut(|shell| shell.activity = activity);
    // SAFETY: as above.
    let own = unsafe { (*activity).clazz };
    LAST_ACTIVITY.store(env().global(own), Ordering::SeqCst);
    start_once(activity);
    make_view();
    let open = HOST.with_borrow(|host| host.as_ref().is_some_and(|host| !host.done()));
    if open {
        heard(Heard::Looks);
        return kick();
    }
    HOST.set(None);
    call_android_main();
}

/// What a process does once: its folders (`HOME`, `XDG_CONFIG_HOME` and `TMPDIR` become the
/// app's own, for `storage`), the timer and the waker's pipe on the main looper.
fn start_once(activity: *mut Activity) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: the activity's paths are C strings; the variables are set before any thread of
    // the library's starts. The timer and pipe are the library's for the life of the process.
    unsafe {
        let own = CStr::from_ptr((*activity).internal_data_path).to_string_lossy().into_owned();
        std::env::set_var("HOME", &own);
        std::env::set_var("XDG_CONFIG_HOME", &own);
        std::env::set_var("TMPDIR", std::path::Path::new(&own).join("tmp"));
        let _ = std::fs::create_dir_all(std::path::Path::new(&own).join("tmp"));
        let timer = timerfd_create(MONOTONIC, NONBLOCK | CLOEXEC);
        let mut ends = [-1; 2];
        pipe2(&mut ends, NONBLOCK | CLOEXEC);
        TIMER.store(timer, Ordering::SeqCst);
        PIPE.store(ends[1], Ordering::SeqCst);
        let looper = ALooper_forThread();
        ALooper_addFd(looper, timer, LOOPER_CALLBACK, LOOPER_INPUT, on_readable, std::ptr::null_mut());
        ALooper_addFd(looper, ends[0], LOOPER_CALLBACK, LOOPER_INPUT, on_readable, std::ptr::null_mut());
    }
}

/// The timer went off, or a waker wrote: what was written is read, and a turn runs.
extern "C" fn on_readable(fd: c_int, _events: c_int, _: *mut c_void) -> c_int {
    let mut bytes = [0u8; 64];
    // SAFETY: the library's own descriptors, non-blocking.
    while unsafe { read(fd, bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    if fd != TIMER.load(Ordering::SeqCst) {
        WOKEN.store(false, Ordering::SeqCst);
        heard(Heard::Other(Box::new(|_| true)));
    }
    kick();
    1
}

/// The app's `android_main`, found in the library that holds this code, called.
fn call_android_main() {
    // SAFETY: the library is loaded (this code is in it): it is opened again only to look the
    // name up, and a function of that name takes nothing.
    unsafe {
        let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
        let here = call_android_main as *const c_void;
        let library = if dladdr(here, &mut info) != 0 { dlopen(info.fname, 2 | 4) } else { std::ptr::null_mut() };
        let main = if library.is_null() { std::ptr::null_mut() } else { dlsym(library, c"android_main".as_ptr()) };
        if main.is_null() {
            return log("taut: the app has no `android_main` (see the module `android`).");
        }
        let main: extern "C" fn() = std::mem::transmute(main);
        main();
    }
}

/// `Dl_info`.
#[repr(C)]
struct DlInfo {
    fname: *const c_char,
    fbase: *mut c_void,
    sname: *const c_char,
    saddr: *mut c_void,
}

unsafe extern "C" {
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    #[cfg(feature = "gpu")]
    fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
}

/// A system property (as `getprop` gives it), where it is set.
#[cfg(feature = "gpu")]
pub(crate) fn property(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    // (`PROP_VALUE_MAX`: the longest value, its zero included.)
    let mut value = [0u8; 92];
    // SAFETY: the name is a C string; the value has room for the longest one.
    let length = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
    (length > 0).then(|| String::from_utf8_lossy(&value[..length.min(91) as usize]).into_owned())
}

/// The library's view, made and put over the activity's own (which shows nothing: the window's
/// pixels are drawn natively). It takes focus, so the soft keyboard types into it; its classes
/// are loaded the first time.
fn make_view() {
    let (env, activity) = (env(), activity());
    let classes = SHELL.with_borrow(|shell| shell.classes);
    let classes = if classes[0].is_null() { jni::load_classes(env, activity).filter(|classes| register(env, classes)) } else { Some(classes) };
    let Some(classes) = classes else { return log("taut: Android refused the library's classes: no soft keyboard, insets or screen readers.") };
    let view = env.frame(|| {
        let view = env.new_object(classes[0], c"(Landroid/content/Context;)V", &[Value::obj(activity)]);
        env.call(view, c"setFocusable", c"(Z)V", &[Value::bool(true)]);
        env.call(view, c"setFocusableInTouchMode", c"(Z)V", &[Value::bool(true)]);
        env.call(view, c"setImportantForAccessibility", c"(I)V", &[Value::int(1)]);
        let place = env.new_named(c"android/view/ViewGroup$LayoutParams", c"(II)V", &[Value::int(-1), Value::int(-1)]);
        env.call(activity, c"addContentView", c"(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V", &[Value::obj(view), Value::obj(place)]);
        env.call(view, c"requestFocus", c"()Z", &[]);
        env.global(view)
    });
    SHELL.with_borrow_mut(|shell| (shell.view, shell.classes) = (view, classes));
}

/// Gives the library's classes their native methods.
fn register(env: Env, classes: &[Obj; 3]) -> bool {
    let host = [
        Native::new(c"getAccessibilityNodeProvider", c"()Landroid/view/accessibility/AccessibilityNodeProvider;", crate::android_access::provider as *const c_void),
        Native::new(c"onCreateInputConnection", c"(Landroid/view/inputmethod/EditorInfo;)Landroid/view/inputmethod/InputConnection;", connection as *const c_void),
        Native::new(c"onCheckIsTextEditor", c"()Z", is_text_editor as *const c_void),
        Native::new(c"onApplyWindowInsets", c"(Landroid/view/WindowInsets;)Landroid/view/WindowInsets;", insets_applied as *const c_void),
    ];
    env.register(classes[0], &host) && env.register(classes[1], &crate::android_access::natives())
}

/// The activity comes back: the 24-hour setting may have changed meanwhile (it is no configuration
/// change), so the formats are read again.
extern "C" fn on_resume(_: *mut Activity) {
    SHELL.with_borrow_mut(|shell| shell.resumed = true);
    heard(Heard::Locale);
    heard(Heard::Event(Event::Minimized(false)));
    heard(Heard::Redraw);
    kick();
}

extern "C" fn on_pause(_: *mut Activity) {
    SHELL.with_borrow_mut(|shell| shell.resumed = false);
    heard(Heard::Event(Event::Minimized(true)));
    kick();
}

extern "C" fn on_focus(_: *mut Activity, focused: c_int) {
    heard(Heard::Event(Event::Focused(focused != 0)));
    kick();
}

/// The activity goes. Its interface stays, for the next activity, unless it was closed.
extern "C" fn on_destroy(_: *mut Activity) {
    let done = HOST.with_borrow(|host| host.as_ref().is_none_or(|host| host.done()));
    if done && !TURNING.get() {
        HOST.set(None);
    }
    SHELL.with_borrow_mut(|shell| (shell.activity, shell.view, shell.typing) = (std::ptr::null_mut(), std::ptr::null_mut(), false));
    drop_composed();
    crate::android_access::forget();
}

/// Drops what the input method composes, once a text box handed it over.
fn drop_composed() {
    if let Some(typing) = INPUT_METHOD.get() {
        (typing.drop)();
    }
}

/// Nothing is kept in the activity's saved state: the interface itself outlives the activity.
extern "C" fn on_save(_: *mut Activity, size: *mut usize) -> *mut c_void {
    // SAFETY: the system hands a place for the size.
    unsafe { *size = 0 };
    std::ptr::null_mut()
}

extern "C" fn on_window_created(_: *mut Activity, window: *mut c_void) {
    // SAFETY: the window is alive until `on_window_destroyed`; it is held meanwhile.
    unsafe {
        ANativeWindow_acquire(window);
        ANativeWindow_setBuffersGeometry(window, 0, 0, RGBA_8888);
    }
    SHELL.with_borrow_mut(|shell| shell.window = window);
    heard(Heard::Looks);
    heard(Heard::Redraw);
    kick();
}

extern "C" fn on_window_resized(_: *mut Activity, _: *mut c_void) {
    heard(Heard::Looks);
    kick();
}

/// The system waits for the window to be drawn whole before this returns.
extern "C" fn on_redraw_needed(_: *mut Activity, _: *mut c_void) {
    heard(Heard::Looks);
    heard(Heard::Redraw);
    kick();
}

extern "C" fn on_window_destroyed(_: *mut Activity, window: *mut c_void) {
    SHELL.with_borrow_mut(|shell| shell.window = std::ptr::null_mut());
    // What the GPU made on the window goes before it.
    #[cfg(feature = "gpu")]
    HOST.with(|host| host.try_borrow_mut().ok()?.as_mut().map(|host| host.window_gone()));
    // SAFETY: held since `on_window_created`.
    unsafe { ANativeWindow_release(window) };
}

extern "C" fn on_queue_created(_: *mut Activity, queue: *mut c_void) {
    // SAFETY: the queue is alive until `on_queue_destroyed`; its events are read on this
    // thread's looper.
    unsafe { AInputQueue_attachLooper(queue, ALooper_forThread(), 1, on_input, queue) };
    SHELL.with_borrow_mut(|shell| shell.queue = queue);
}

extern "C" fn on_queue_destroyed(_: *mut Activity, queue: *mut c_void) {
    // SAFETY: attached in `on_queue_created`.
    unsafe { AInputQueue_detachLooper(queue) };
    SHELL.with_borrow_mut(|shell| shell.queue = std::ptr::null_mut());
}

/// The configuration changed: looks, and the locale (the manifest asks for both).
extern "C" fn on_configuration_changed(_: *mut Activity) {
    heard(Heard::Looks);
    heard(Heard::Locale);
    kick();
}

// ---- input ----

/// Input waits: each event is taken (unless the input method took it first) and finished,
/// then the interface runs.
extern "C" fn on_input(_: c_int, _: c_int, queue: *mut c_void) -> c_int {
    let mut event = std::ptr::null_mut();
    // SAFETY: events are taken from the queue and each handed back once.
    while unsafe { AInputQueue_getEvent(queue, &mut event) } >= 0 {
        if unsafe { AInputQueue_preDispatchEvent(queue, event) } != 0 {
            continue;
        }
        let handled = input(event);
        unsafe { AInputQueue_finishEvent(queue, event, c_int::from(handled)) };
    }
    kick();
    1
}

/// One input event: says whether the library took it (else the system does what it does with
/// it: a volume key changes the volume).
fn input(event: *mut c_void) -> bool {
    // SAFETY: the event is the queue's, alive until finished.
    match unsafe { AInputEvent_getType(event) } {
        KEY_EVENT => key(event),
        MOTION_EVENT => motion(event),
        _ => false,
    }
}

/// The modifiers in a meta state.
fn mods_of(meta: i32) -> Mods {
    Mods { ctrl: meta & META_CTRL != 0, shift: meta & META_SHIFT != 0, alt: meta & META_ALT != 0 }
}

/// A key code's key.
fn key_of(code: i32) -> Option<Key> {
    Some(match code {
        66 | 160 => Key::Enter,
        111 => Key::Escape,
        61 => Key::Tab,
        DEL => Key::Backspace,
        FORWARD_DEL => Key::Delete,
        21 => Key::Left,
        22 => Key::Right,
        19 => Key::Up,
        20 => Key::Down,
        122 => Key::Home,
        123 => Key::End,
        92 => Key::PageUp,
        93 => Key::PageDown,
        62 => Key::Space,
        82 => Key::Menu,
        131..=142 => Key::F((code - 130) as u8),
        29..=54 => Key::Char((b'a' + (code - 29) as u8) as char),
        7..=16 => Key::Char((b'0' + (code - 7) as u8) as char),
        _ => return None,
    })
}

/// What a key types, as its keyboard's map says (none for a key that types nothing).
fn typed_by(event: *const c_void, code: i32, meta: i32) -> Option<char> {
    let env = env();
    // SAFETY: the event is alive.
    let device = unsafe { AInputEvent_getDeviceId(event) };
    let typed = env.frame(|| {
        let map = env.call_static(c"android/view/KeyCharacterMap", c"load", c"(I)Landroid/view/KeyCharacterMap;", &[Value::int(device)]).object();
        env.call(map, c"get", c"(II)I", &[Value::int(code), Value::int(meta)]).as_int()
    });
    char::from_u32(typed as u32 & 0x7fff_ffff).filter(|letter| *letter != '\0' && !letter.is_control())
}

/// A key went down or up. The back key goes to the interface as `Back` (as it comes up); keys
/// the library has no use for (volume, camera) go to the system.
fn key(event: *mut c_void) -> bool {
    // SAFETY: the event is a key event, alive.
    let (action, code, meta) = unsafe { (AKeyEvent_getAction(event), AKeyEvent_getKeyCode(event), AKeyEvent_getMetaState(event)) };
    if code == BACK {
        if action == KEY_UP {
            heard(Heard::Back);
        }
        return true;
    }
    let (key, mods) = (key_of(code), mods_of(meta));
    if action == KEY_UP {
        key.into_iter().for_each(|key| heard(Heard::Event(Event::KeyUp(key))));
        return key.is_some();
    }
    let text = if action == KEY_DOWN && !mods.ctrl { typed_by(event, code, meta) } else { None };
    if action == KEY_DOWN {
        key.into_iter().for_each(|key| heard(Heard::Event(Event::Key(key, mods))));
        text.into_iter().for_each(|text| heard(Heard::Event(Event::Text(text.to_string()))));
    }
    key.is_some() || text.is_some()
}

/// Where pointer `n` of a motion event is, in units of the interface (inside the insets).
fn place_of(event: *const c_void, n: usize) -> crate::geom::Point {
    let (scale, insets) = SHELL.with_borrow(|shell| (shell.scale, shell.insets));
    // SAFETY: the event is alive and has the pointer.
    let (x, y) = unsafe { (AMotionEvent_getX(event, n), AMotionEvent_getY(event, n)) };
    pt((x - insets[0] as f32) / scale, (y - insets[1] as f32) / scale)
}

/// Notes whether the system drags what the app dragged out.
pub(crate) fn set_dragging_out(dragging: bool) {
    SHELL.with_borrow_mut(|shell| (shell.dragging_out, shell.finger) = (dragging, None));
}

/// A motion: the mouse's, a pen's, a finger's, or a screen reader's exploring finger (hover
/// events from a touch screen). (A touch the system took for a drag out is cancelled: the drag
/// says where it ends.)
fn motion(event: *mut c_void) -> bool {
    // SAFETY: the event is a motion event, alive.
    let (action, tool, buttons) = unsafe { (AMotionEvent_getAction(event), AMotionEvent_getToolType(event, 0), AMotionEvent_getButtonState(event)) };
    let (what, nth) = (action & 0xff, (action >> 8 & 0xff) as usize);
    if what == CANCEL && SHELL.with_borrow(|shell| shell.dragging_out) {
        return true;
    }
    match tool {
        MOUSE => mouse(event, what, buttons),
        FINGER if matches!(what, HOVER_ENTER | HOVER_MOVE | HOVER_EXIT) => crate::android_access::hover(place_of(event, 0), what == HOVER_EXIT),
        STYLUS | ERASER => pen(event, what, tool == ERASER),
        _ if cfg!(feature = "touch") => fingers(event, what, nth),
        _ => finger_as_mouse(event, what, nth),
    }
    true
}

/// The mouse: where it is, its buttons as they change, its wheel.
fn mouse(event: *mut c_void, what: i32, buttons: i32) {
    let place = place_of(event, 0);
    match what {
        HOVER_EXIT => return heard(Heard::Event(Event::Leave)),
        // SAFETY: the event is alive.
        SCROLL => unsafe {
            let (across, down) = (AMotionEvent_getAxisValue(event, AXIS_HSCROLL, 0), AMotionEvent_getAxisValue(event, AXIS_VSCROLL, 0));
            heard(Heard::Event(Event::Move(place)));
            // Control and the wheel zoom, as elsewhere.
            if mods_of(AMotionEvent_getMetaState(event)).ctrl {
                return heard(Heard::Event(Event::Zoom(crate::touch::wheel_zoom(down))));
            }
            return heard(Heard::Event(Event::Wheel(across * 48.0, -down * 48.0)));
        },
        _ => heard(Heard::Event(Event::Move(place))),
    }
    let mods = mods_of(unsafe { AMotionEvent_getMetaState(event) });
    let was = SHELL.with_borrow_mut(|shell| std::mem::replace(&mut shell.buttons, buttons));
    for (bit, button) in [(PRIMARY, Button::Left), (SECONDARY, Button::Right), (TERTIARY, Button::Middle)] {
        match (was & bit != 0, buttons & bit != 0) {
            (false, true) => heard(Heard::Event(Event::Press(button, mods))),
            (true, false) => heard(Heard::Event(Event::Release(button))),
            _ => {}
        }
    }
}

/// A pen: the mouse's left button while it touches, and (with the feature `touch`) what it is
/// beyond that.
fn pen(event: *mut c_void, what: i32, eraser: bool) {
    let place = place_of(event, 0);
    heard(Heard::Event(Event::Move(place)));
    if cfg!(feature = "touch") {
        // SAFETY: the event is alive.
        let pressure = unsafe { AMotionEvent_getAxisValue(event, AXIS_PRESSURE, 0) };
        let touching = matches!(what, DOWN | MOVE);
        let stylus = crate::touch::Stylus { pressure: if touching { pressure.clamp(0.0, 1.0) } else { 0.0 }, eraser, touching, ..Default::default() };
        heard(Heard::Event(Event::Stylus(Some(stylus))));
    }
    match what {
        DOWN => heard(Heard::Event(Event::Press(Button::Left, Mods::NONE))),
        UP | CANCEL => heard(Heard::Event(Event::Release(Button::Left))),
        HOVER_EXIT => heard(Heard::Event(Event::Leave)),
        _ => {}
    }
}

/// Fingers, each as itself (the feature `touch`): taps, scrolling, long presses, pinches.
fn fingers(event: *mut c_void, what: i32, nth: usize) {
    use crate::touch::{Touch, TouchPhase};
    // SAFETY: the event is alive, with this many pointers.
    let count = unsafe { AMotionEvent_getPointerCount(event) };
    let id = |n: usize| unsafe { AMotionEvent_getPointerId(event, n) } as u32;
    let one = |n: usize, phase| heard(Heard::Event(Event::Touch(Touch { id: id(n), position: place_of(event, n), phase })));
    match what {
        DOWN | POINTER_DOWN => one(nth, TouchPhase::Down),
        UP | POINTER_UP => one(nth, TouchPhase::Up),
        MOVE => (0..count).for_each(|n| one(n, TouchPhase::Move)),
        CANCEL => (0..count).for_each(|n| one(n, TouchPhase::Cancel)),
        _ => {}
    }
}

/// The first finger down as the mouse's left button (without the feature `touch`), leaving as
/// it lifts.
fn finger_as_mouse(event: *mut c_void, what: i32, nth: usize) {
    // SAFETY: the event is alive.
    let id = unsafe { AMotionEvent_getPointerId(event, nth) };
    let mouse = SHELL.with_borrow_mut(|shell| {
        shell.finger = shell.finger.or((what == DOWN).then_some(id));
        shell.finger
    });
    let Some(mouse) = mouse else { return };
    // SAFETY: as above: the finger's index now.
    let at = (0..unsafe { AMotionEvent_getPointerCount(event) }).find(|n| unsafe { AMotionEvent_getPointerId(event, *n) } == mouse).unwrap_or(0);
    heard(Heard::Event(Event::Move(place_of(event, at))));
    match what {
        DOWN => heard(Heard::Event(Event::Press(Button::Left, Mods::NONE))),
        UP | CANCEL => {
            heard(Heard::Event(Event::Release(Button::Left)));
            heard(Heard::Event(Event::Leave));
            SHELL.with_borrow_mut(|shell| shell.finger = None);
        }
        POINTER_UP if id == mouse => {
            heard(Heard::Event(Event::Release(Button::Left)));
            SHELL.with_borrow_mut(|shell| shell.finger = None);
        }
        _ => {}
    }
}

// ---- the library's view: soft keyboard and insets ----

/// The view is asked whether it edits text: while a text box has focus.
extern "C" fn is_text_editor(_: Env, _: Obj) -> u8 {
    SHELL.with_borrow(|shell| u8::from(shell.typing))
}

/// An input method connects: through `android_input_method`'s connection (none before a text
/// box was described).
extern "C" fn connection(env: Env, view: Obj, info: Obj) -> Obj {
    INPUT_METHOD.get().map_or(std::ptr::null_mut(), |typing| (typing.connect)(env, view, info))
}

/// What a text box holds, as its input method is told.
#[derive(Clone, Copy)]
pub(crate) enum Typed {
    Text,
    Password,
    Code,
}

/// What the focused text box holds.
pub(crate) fn typed() -> Typed {
    SHELL.with_borrow(|shell| shell.typed)
}

/// The insets changed: what the system bars, the cutout and the keyboard cover now.
extern "C" fn insets_applied(env: Env, view: Obj, insets: Obj) -> Obj {
    let root = env.call(view, c"getRootWindowInsets", c"()Landroid/view/WindowInsets;", &[]).object();
    let kept = jni::insets_of(env, if root.is_null() { insets } else { root });
    env.drop_local(root);
    let changed = SHELL.with_borrow_mut(|shell| std::mem::replace(&mut shell.insets, kept) != kept);
    if changed {
        heard(Heard::Looks);
        heard(Heard::Redraw);
        kick();
    }
    insets
}

// ---- the loop ----

/// What the callbacks drive: the interface.
trait Turns {
    /// One turn; says how long the looper may wait before the next (for ever: none).
    fn turn(&mut self) -> Option<Duration>;
    /// Whether the app closed.
    fn done(&self) -> bool;
    /// The window goes: what the GPU made on it goes first.
    #[cfg(feature = "gpu")]
    fn window_gone(&mut self);
}

/// The interface and what shows it.
struct Host<App> {
    app: App,
    ui: Ui,
    surface: Surface,
    rerun: bool,
    done: bool,
    /// The window must be shown whole at the next draw.
    whole: bool,
    start: Instant,
    /// The window's size in pixels, as last measured.
    pixels: [i32; 2],
    /// The text box the soft keyboard types into, and whether the person pressed since the last
    /// run.
    typed_in: Option<crate::id::Id>,
    pressed: bool,
    /// What the input method composes, as last drawn.
    composed: Option<crate::android_input_method::Panel>,
    /// What the GPU keeps of the window (feature `gpu`).
    #[cfg(feature = "gpu")]
    screen: crate::gpu_android::Screen,
}

impl<App: FnMut(&mut Ui)> Turns for Host<App> {
    fn turn(&mut self) -> Option<Duration> {
        self.ui.set_time(self.start.elapsed());
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO);
        for what in HEARD.with_borrow_mut(std::mem::take) {
            self.take(what);
        }
        self.listen();
        self.rerun |= ALL_PARTS.get().is_some_and(|parts| (parts.before)(&mut self.ui));
        if self.rerun && !self.done && self.pixels[0] > 0 {
            self.describe();
        }
        self.draw();
        if self.rerun { Some(Duration::ZERO) } else { self.ui.wake_in() }
    }
    fn done(&self) -> bool {
        self.done
    }
    #[cfg(feature = "gpu")]
    fn window_gone(&mut self) {
        self.screen.window_gone();
    }
}

impl<App: FnMut(&mut Ui)> Host<App> {
    /// One thing heard.
    fn take(&mut self, what: Heard) {
        match what {
            Heard::Event(event) => {
                self.pressed |= matches!(event, Event::Press(..) | Event::Touch(crate::touch::Touch { phase: crate::touch::TouchPhase::Up, .. }));
                self.rerun |= self.ui.event(event);
            }
            Heard::Other(work) => self.rerun |= work(&mut self.ui),
            Heard::Looks => self.measure(),
            Heard::Locale => self.rerun |= crate::locale::read_again(),
            Heard::Redraw => self.whole = true,
            Heard::Back if self.ui.popup_anchor().is_some() => self.rerun |= self.ui.event(Event::Key(Key::Escape, Mods::NONE)),
            Heard::Back => {
                self.ui.request_close();
                self.rerun = true;
            }
        }
    }

    /// Takes in the window's size, the insets and the display settings: the interface runs if
    /// any changed.
    fn measure(&mut self) {
        let (window, insets) = SHELL.with_borrow(|shell| (shell.window, shell.insets));
        if activity().is_null() {
            return;
        }
        let display = jni::display(env(), activity());
        let scale = if display.density > 0.0 { display.density } else { 1.0 };
        SHELL.with_borrow_mut(|shell| shell.scale = scale);
        let preferences = Preferences { dark: display.dark, accent: display.accent.map(crate::color::Color), reduced_motion: display.still, text_scale: display.text_scale, ..Preferences::default() };
        self.rerun |= self.ui.set_preferences(preferences);
        if window.is_null() {
            return;
        }
        // SAFETY: the window is held while the shell has it.
        self.pixels = unsafe { [ANativeWindow_getWidth(window), ANativeWindow_getHeight(window)] };
        let inner = [(self.pixels[0] - insets[0] - insets[2]).max(1), (self.pixels[1] - insets[1] - insets[3]).max(1)];
        let size = Size { w: inner[0] as f32 / scale, h: inner[1] as f32 / scale };
        if (size, scale) != (self.ui.size(), self.ui.scale()) {
            self.ui.resize(size, scale);
            (self.rerun, self.whole) = (true, true);
            // What a test draws the same frame by (see `android_text`'s tests).
            let [width, height] = self.pixels;
            let accent = display.accent.unwrap_or(0);
            log(&format!(
                "window {width} {height} insets {} {} {} {} scale {scale} dark {} accent {accent:08x} text {} still {}",
                insets[0],
                insets[1],
                insets[2],
                insets[3],
                u8::from(display.dark),
                display.text_scale,
                u8::from(display.still)
            ));
        }
    }

    /// A screen reader asked for the window's nodes: the interface tells from now on.
    fn listen(&mut self) {
        if !self.ui.listening && crate::android_access::asked() {
            self.ui.set_screen_reader(true);
            self.rerun = true;
        }
    }

    /// Runs the interface, then does what it asked: the soft keyboard for a focused text box,
    /// closing, readers told.
    fn describe(&mut self) {
        self.ui.run(&mut self.app);
        let closes = self.ui.take_closing() == Some(Ok(()));
        (self.rerun, self.done) = (false, self.done || closes || self.ui.quitting());
        let (_, _, _) = (self.ui.take_minimize(), self.ui.take_maximized(), self.ui.take_full_screen());
        if let Some(parts) = ALL_PARTS.get() {
            (parts.after)(&mut self.ui);
        }
        self.keyboard();
        if self.ui.listening {
            crate::android_access::keep(&self.ui);
        }
        let activity = SHELL.with_borrow(|shell| shell.activity);
        if self.done && !activity.is_null() {
            // SAFETY: the activity is alive until `on_destroy`.
            unsafe { ANativeActivity_finish(activity) };
        }
    }

    /// The soft keyboard shows while a text box has focus (again as it is pressed, should the
    /// person have put it away), and goes with it. Focus coming to a box starts its input
    /// afresh (the input method learns what it holds), dropping what was composed.
    fn keyboard(&mut self) {
        let now = self.ui.ime_area().map(|_| self.ui.focus());
        let (was, pressed) = (std::mem::replace(&mut self.typed_in, now), std::mem::take(&mut self.pressed));
        let typed = match (self.ui.password_focused(), self.ui.code_focused()) {
            (true, _) => Typed::Password,
            (_, true) => Typed::Code,
            _ => Typed::Text,
        };
        let view = SHELL.with_borrow_mut(|shell| {
            (shell.typing, shell.typed) = (now.is_some(), typed);
            shell.view
        });
        if was != now {
            drop_composed();
        }
        if (was != now || pressed && now.is_some()) && !view.is_null() {
            jni::soft_keyboard(env(), activity(), view, now.is_some(), now.is_some() && was != now);
        }
    }

    /// Draws what changed and shows it: copied into the window's buffer, inside the insets, with
    /// what the input method composes over it.
    fn draw(&mut self) {
        let (window, resumed, insets) = SHELL.with_borrow(|shell| (shell.window, shell.resumed, shell.insets));
        if self.ui.size().w <= 0.0 {
            return;
        }
        #[cfg(feature = "gpu")]
        if self.through_gpu(if resumed { window } else { std::ptr::null_mut() }, insets) {
            return;
        }
        let changed = self.surface.update(&mut self.ui);
        self.ui.changed();
        let typing = INPUT_METHOD.get();
        let panel = typing.and_then(|typing| (typing.panel)(&mut self.ui, [insets[0], insets[1]]));
        let was = std::mem::replace(&mut self.composed, panel);
        if window.is_null() || !resumed || (changed.is_empty() && !self.whole && was == self.composed) {
            return;
        }
        let whole = std::mem::take(&mut self.whole);
        let around = [was.as_ref(), self.composed.as_ref()].into_iter().flatten().map(|panel| panel.place).fold(bounds(&changed, insets), union);
        let mut dirty = if whole { [0, 0, self.pixels[0], self.pixels[1]] } else { around };
        let mut buffer = WindowBuffer { width: 0, height: 0, stride: 0, format: 0, bits: std::ptr::null_mut(), reserved: [0; 6] };
        // SAFETY: the window is held; the buffer is written within its size and posted.
        unsafe {
            if ANativeWindow_lock(window, &mut buffer, &mut dirty) != 0 || buffer.bits.is_null() {
                return;
            }
            let rows = std::slice::from_raw_parts_mut(buffer.bits.cast::<u32>(), (buffer.stride * buffer.height) as usize);
            copy_out(&self.surface, rows, &buffer, dirty, insets, self.ui.theme.bg.0);
            if let Some((panel, typing)) = self.composed.as_ref().zip(typing) {
                (typing.show)(panel, rows, &buffer);
            }
            ANativeWindow_unlockAndPost(window);
        }
    }
}

/// The window shown through the GPU (feature `gpu`).
#[cfg(feature = "gpu")]
impl<App: FnMut(&mut Ui)> Host<App> {
    /// Draws what changed through the GPU, where it shows the window (`window`, null while there
    /// is none to show), with what the input method composes over it. Says whether it did; if
    /// not, the CPU is to.
    fn through_gpu(&mut self, window: *mut c_void, insets: [i32; 4]) -> bool {
        let panel = INPUT_METHOD.get().and_then(|typing| (typing.panel)(&mut self.ui, [insets[0], insets[1]]));
        let shown = {
            let panels = [self.composed.as_ref(), panel.as_ref()];
            let window = crate::gpu_android::Window { native: window, size: self.pixels, insets, again: self.whole, panels };
            self.screen.draw(&mut self.ui, &mut self.surface, &window)
        };
        let renderer = self.screen.renderer();
        if std::mem::replace(&mut self.ui.renderer, renderer) != renderer {
            // What a test checks the window is shown by (see `tests/android_gpu.rs`).
            log(&format!("shown by {renderer:?}"));
        }
        match shown {
            crate::platform::ShownBy::Gpu => (self.composed, self.whole) = (panel, false),
            crate::platform::ShownBy::Cpu => {}
            crate::platform::ShownBy::CpuAfresh => (self.surface, self.whole) = (Surface::default(), true),
        }
        shown == crate::platform::ShownBy::Gpu
    }
}

/// The pixels changed places cover in the window: left, top, right, bottom (all 0 for none).
fn bounds(changed: &[[usize; 4]], insets: [i32; 4]) -> [i32; 4] {
    if changed.is_empty() {
        return [0; 4];
    }
    let start = [i32::MAX, i32::MAX, 0, 0];
    let [left, top, right, bottom] = changed.iter().fold(start, |all, [l, t, r, b]| [all[0].min(*l as i32), all[1].min(*t as i32), all[2].max(*r as i32), all[3].max(*b as i32)]);
    [left + insets[0], top + insets[1], right + insets[0], bottom + insets[1]]
}

/// Two pixel rectangles' bounds (left, top, right, bottom); an empty one adds nothing.
fn union(one: [i32; 4], other: [i32; 4]) -> [i32; 4] {
    match (one[0] < one[2] && one[1] < one[3], other[0] < other[2] && other[1] < other[3]) {
        (_, false) => one,
        (false, true) => other,
        (true, true) => [one[0].min(other[0]), one[1].min(other[1]), one[2].max(other[2]), one[3].max(other[3])],
    }
}

/// A pixel as the window's buffer has it: red and blue trade places (the buffer is RGBA in
/// memory), and it is opaque.
pub(crate) fn swap(pixel: u32) -> u32 {
    0xff00_0000 | pixel & 0x0000_ff00 | pixel >> 16 & 0xff | (pixel & 0xff) << 16
}

/// Copies the surface into the window's buffer over `dirty` (as the lock left it): the
/// interface inside the insets, the background under them.
fn copy_out(surface: &Surface, rows: &mut [u32], buffer: &WindowBuffer, dirty: [i32; 4], insets: [i32; 4], bg: u32) {
    let [left, top] = [dirty[0].max(0), dirty[1].max(0)];
    let [right, bottom] = [dirty[2].min(buffer.width), dirty[3].min(buffer.height)];
    for y in top..bottom {
        let row = &mut rows[(y * buffer.stride) as usize..][..buffer.width as usize];
        let inside = (y - insets[1]) as usize;
        let from = surface.pixels.get(inside.wrapping_mul(surface.width)..).filter(|_| y >= insets[1] && inside < surface.height).unwrap_or_default();
        for x in left..right {
            let column = (x - insets[0]) as usize;
            let pixel = if x >= insets[0] && column < surface.width { from.get(column).copied().unwrap_or(bg) } else { bg };
            row[x as usize] = swap(pixel);
        }
    }
}

/// Opens the interface in the activity's window and returns: it runs on the main looper from
/// then on, while the activity and its successors live (see the module). Called from the app's
/// `android_main`; the window's size and place are the activity's: of `window`, only the
/// renderer is used.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    if activity().is_null() {
        return log("taut: on Android, `run` is called from `android_main`, on the main thread.");
    }
    let text: Box<dyn crate::text::TextSystem> = match SystemText::new() {
        Some(text) => Box::new(text),
        None => Box::new(crate::text::BuiltInFont),
    };
    let mut ui = Ui::new(text, Box::new(SystemClipboard));
    ui.set_native_file_dialog(true);
    #[rustfmt::skip]
    let host = Host {
        app, ui, surface: Surface::default(), rerun: true, done: false, whole: true, start: Instant::now(), pixels: [0, 0], typed_in: None, pressed: false, composed: None,
        #[cfg(feature = "gpu")]
        screen: crate::gpu_android::Screen::new(window.renderer),
    };
    #[cfg(not(feature = "gpu"))]
    let _ = window;
    HOST.set(Some(Box::new(host)));
    heard(Heard::Looks);
    kick();
}

// ---- the clipboard ----

/// The system clipboard, text only, for an interface.
pub(crate) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        jni::clipboard_text(env(), activity())
    }
    fn set(&mut self, text: &str) {
        jni::set_clipboard_text(env(), activity(), text);
    }
    /// Pictures and files, once an interface asked for one: see `PicturesAndFiles`.
    fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
        (PICTURES_AND_FILES.get()?.picture)(self)
    }
    fn set_picture(&mut self, width: usize, pixels: &[u32]) {
        PICTURES_AND_FILES.get().map_or((), |its| (its.set_picture)(self, width, pixels))
    }
    fn files(&mut self) -> Vec<PathBuf> {
        PICTURES_AND_FILES.get().map_or(Vec::new(), |its| (its.files)(self))
    }
    fn set_files(&mut self, files: &[PathBuf]) {
        PICTURES_AND_FILES.get().map_or((), |its| (its.set_files)(self, files))
    }
}

thread_local! {
    /// The clipboard's picture and file code: none until an interface on this thread asks for
    /// one (see `PicturesAndFiles`).
    static PICTURES_AND_FILES: Cell<Option<PicturesAndFiles<SystemClipboard>>> = const { Cell::new(None) };
}

/// Hands the clipboard its picture and file code, on this thread (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    let (picture, set_picture) = (SystemClipboard::picture_on, SystemClipboard::put_picture);
    PICTURES_AND_FILES.set(Some(PicturesAndFiles { picture, set_picture, files: SystemClipboard::files_on, set_files: SystemClipboard::put_files }));
}

/// Opens a link with the app the system opens it with (`open_with_system`): from any thread.
/// (Files: see `android_files::open_file`.)
pub(crate) fn open_link(link: &str) -> bool {
    jni::open_link(last_activity(), link)
}

/// The last activity's Java object, for any thread (a global reference; null before the first).
pub(crate) fn last_activity() -> Obj {
    LAST_ACTIVITY.load(Ordering::SeqCst)
}
