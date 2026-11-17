//! macOS: AppKit windows, spoken to through the Objective-C runtime (`macos_sys`). Text is
//! CoreText's (`core_text`).
//!
//! Each window is an `NSWindow` of the library's own class whose content is a view of its own
//! (`TautView`). The CPU draws every pixel into a block in memory, as on the other systems;
//! what changed is drawn again there, and the block is handed to the view's layer as its
//! picture (a `CGImage`), shown at the screen's scale (two pixels a point on Retina screens).
//!
//! One loop runs all windows, as on Linux: it takes AppKit's events from the queue itself
//! (`nextEventMatchingMask:`) and hands each on (`sendEvent:`); the view and window methods
//! AppKit then calls note what happened (`Heard`), and the loop's turn passes that to the
//! interface. So nothing of the interface runs inside AppKit's calls, but for one case: while a
//! person drags a window's edge, AppKit keeps the events to itself until they let go, and the
//! window is run and drawn from inside its resize notice (`LIVE`).
//!
//! Keys come as key codes, which name the keys whatever the layout; a letter key is the letter
//! it types without modifiers. Typed text goes through the system's text input (the view is an
//! `NSTextInputClient`), so input methods and dead keys work; what an input method is still
//! composing is shown by its own window, not in the text box. Command and Control are both
//! `ctrl` (Command+C copies), Option is `alt`.
//!
//! Places on the desktop are in points from the top left of the main screen, as CoreGraphics
//! has them: the desktop's own pixels here are points.
use crate::color::Color;
use crate::core_text::SystemText;
use crate::event::{Button, Cursor, Event, Key, Mods};
use crate::geom::{Point, Rect, Size, pt};
use crate::id::Id;
use crate::macos_sys::*;
use crate::platform::{Clipboard, PicturesAndFiles, Window, WindowPlacement};
use crate::preferences::Preferences;
use crate::screens::{OpenAt, Screen, window_origin};
use crate::second_window::{WindowKind, WindowOpening};
use crate::surface::Surface;
use crate::theme::ContrastColors;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};

/// Window style bits: a title bar, a close button, minimizing, resizing; content under the
/// title bar.
const TITLED: usize = 1;
const CLOSABLE: usize = 2;
const MINIATURIZABLE: usize = 4;
const RESIZABLE: usize = 8;
const FULL_SIZE_CONTENT: usize = 1 << 15;
/// Window levels: as usual, and above other programs' windows.
const NORMAL_LEVEL: isize = 0;
const FLOATING_LEVEL: isize = 3;

/// Event types, as `NSEvent` numbers them.
const LEFT_DOWN: usize = 1;
const LEFT_UP: usize = 2;
const RIGHT_DOWN: usize = 3;
const RIGHT_UP: usize = 4;
const EXITED: usize = 9;
const APPLICATION: usize = 15;
const OTHER_DOWN: usize = 25;
const OTHER_UP: usize = 26;

/// Modifier bits: Shift, Control, Option, Command.
const SHIFT: usize = 1 << 17;
const CONTROL: usize = 1 << 18;
const OPTION: usize = 1 << 19;
const COMMAND: usize = 1 << 20;

/// What a [`Waker`] posts: an application event of this subtype.
const WAKE: i16 = 0x7461;

/// The scroll distance of a wheel notch, in units (as on Linux).
const NOTCH: f32 = 48.0;

/// What the library's views, windows and application delegate heard from AppKit, for the
/// loop's next turn. A view or window is named by its view.
pub(crate) enum Heard {
    /// Input in a view.
    Event(Obj, Event),
    /// A window's close button.
    Close(Obj),
    /// A window's size or scale changed.
    Resized(Obj),
    /// A window moved, or went full screen or back.
    Moved(Obj),
    /// A window was minimized or restored.
    Minimized(Obj, bool),
    /// A window gained or lost the keyboard.
    Focused(Obj, bool),
    /// The system's looks changed (dark or light, accent, contrast, motion), or its screens.
    Looks,
    /// The program was asked to quit (its menu, logging out).
    Quit,
    /// The desktop part heard something for the app: its tray icon, a hotkey.
    Desktop,
    /// Something only a handed-in part knows of (drops, the file dialog).
    Other(Obj, Box<dyn FnOnce(&mut Ui) -> bool>),
}

/// What a view keeps between AppKit's calls: its cursor, the text cursor's place (for input
/// methods), what an input method composes, the modifiers held, and what a key typed.
struct ViewState {
    view: Obj,
    cursor: Cursor,
    ime: Option<Rect>,
    marked: bool,
    typed: Option<String>,
    held: Mods,
    /// The last press, kept (retained) for moving a window by the app's own title bar.
    down: Obj,
}

thread_local! {
    static HEARD: RefCell<Vec<Heard>> = const { RefCell::new(Vec::new()) };
    static VIEWS: RefCell<Vec<ViewState>> = const { RefCell::new(Vec::new()) };
    /// The host, while the loop waits for events: what a window being resized runs (see
    /// `live_turn`). And whether a turn is running now.
    static LIVE: Cell<Option<*mut dyn Turns>> = const { Cell::new(None) };
    static TURNING: Cell<bool> = const { Cell::new(false) };
}

impl Heard {
    /// The view it happened in; null for what is the program's.
    fn view(&self) -> Obj {
        match self {
            Heard::Event(view, _) | Heard::Close(view) | Heard::Resized(view) | Heard::Moved(view) | Heard::Minimized(view, _) | Heard::Focused(view, _) | Heard::Other(view, _) => *view,
            Heard::Looks | Heard::Quit | Heard::Desktop => std::ptr::null_mut(),
        }
    }
}

/// Notes what AppKit said, for the next turn.
pub(crate) fn heard(what: Heard) {
    HEARD.with_borrow_mut(|heard| heard.push(what));
}

/// Does something to every view of the library's.
pub(crate) fn each_view(work: fn(Obj)) {
    let views: Vec<Obj> = VIEWS.with_borrow(|views| views.iter().map(|state| state.view).collect());
    views.into_iter().for_each(work);
}

/// Works with a view's state, if it is one of the library's.
fn with_view<T>(view: Obj, work: impl FnOnce(&mut ViewState) -> T) -> Option<T> {
    VIEWS.with_borrow_mut(|views| views.iter_mut().find(|state| state.view == view).map(work))
}

/// The application (`NSApp`), once a window was opened: for wakers on other threads.
static APP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// A wake is posted that the loop has not taken yet: another posts nothing.
static WOKEN: AtomicBool = AtomicBool::new(false);

/// Asks the window to run its interface again; can be sent from any thread. (An app learning
/// something in the background, say a changed file, wakes the window with this.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;

impl Waker {
    /// Has the interface run once more, soon, on its own thread, from whatever thread calls
    /// this.
    pub fn wake(&self) {
        let app = APP.load(Ordering::SeqCst);
        if app.is_null() || WOKEN.swap(true, Ordering::SeqCst) {
            return;
        }
        let _pool = Pool::new();
        // SAFETY: AppKit lets any thread post an event to the application's queue.
        unsafe {
            let event: Obj = send(
                class(c"NSEvent"),
                c"otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
                (APPLICATION, NSPoint::default(), 0usize, 0.0f64, 0isize, std::ptr::null_mut::<c_void>(), WAKE, 0isize, 0isize),
            );
            send::<()>(app, c"postEvent:atStart:", (event, 0 as Bool));
        }
    }
}

// ---- the application ----

/// The application, made ready once: a regular program with a menu bar of its own (whose Quit
/// asks the app, as closing the window does), and a delegate hearing what the system says.
fn application() -> Obj {
    // SAFETY: made on the main thread, the one AppKit runs on.
    let app: Obj = unsafe { send(class(c"NSApplication"), c"sharedApplication", ()) };
    if APP.swap(app, Ordering::SeqCst).is_null() {
        prepare(app);
    }
    app
}

/// Readies the application the first time: its policy, delegate, menu, and the notices of
/// changed looks and screens.
fn prepare(app: Obj) {
    let methods: [(&CStr, *const c_void, &str); 2] = [(c"applicationShouldTerminate:", should_terminate as *const c_void, "Q@:@"), (c"looksChanged:", looks_changed as *const c_void, "v@:@")];
    made_class(c"TautApplication", c"NSObject", &methods, &[c"NSApplicationDelegate"]);
    let delegate = made(c"TautApplication");
    // SAFETY: plain setup of the shared application; the delegate is kept for good.
    unsafe {
        send::<Bool>(app, c"setActivationPolicy:", (0isize,));
        send::<()>(app, c"setDelegate:", (delegate,));
        send::<()>(app, c"setMainMenu:", (menu(),));
        send::<()>(app, c"finishLaunching", ());
        let centre: Obj = send(class(c"NSNotificationCenter"), c"defaultCenter", ());
        let workspace: Obj = send(send::<Obj>(class(c"NSWorkspace"), c"sharedWorkspace", ()), c"notificationCenter", ());
        let notices = [
            (centre, c"NSApplicationDidChangeScreenParametersNotification"),
            (centre, c"NSSystemColorsDidChangeNotification"),
            (workspace, c"NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification"),
        ];
        for (centre, name) in notices {
            let name = ns_string(name.to_str().unwrap_or_default());
            send::<()>(centre, c"addObserver:selector:name:object:", (delegate, sel(c"looksChanged:"), name, std::ptr::null_mut::<c_void>()));
        }
    }
}

/// The menu bar: the program's menu, with Hide and Quit.
fn menu() -> Obj {
    let name = std::env::current_exe().ok().and_then(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned())).unwrap_or_default();
    // SAFETY: menus and items made and handed to the application, which keeps them.
    unsafe {
        let bar: Obj = send(made(c"NSMenu"), c"autorelease", ());
        let own: Obj = send(made(c"NSMenu"), c"autorelease", ());
        let holder: Obj = send(made(c"NSMenuItem"), c"autorelease", ());
        for (title, action, key) in [(format!("Hide {name}"), c"hide:", "h"), (format!("Quit {name}"), c"terminate:", "q")] {
            let item: Obj = send(class(c"NSMenuItem"), c"alloc", ());
            let item: Obj = send(item, c"initWithTitle:action:keyEquivalent:", (ns_string(&title), sel(action), ns_string(key)));
            send::<()>(own, c"addItem:", (item,));
            send::<()>(item, c"release", ());
        }
        send::<()>(holder, c"setSubmenu:", (own,));
        send::<()>(bar, c"addItem:", (holder,));
        bar
    }
}

/// The system asks the program to quit (its menu's Quit, logging out): the app hears it as
/// its first window's close; the system is told not now.
extern "C" fn should_terminate(_: Obj, _: Sel, _: Obj) -> usize {
    heard(Heard::Quit);
    0
}

/// The system's looks or screens changed.
extern "C" fn looks_changed(_: Obj, _: Sel, _: Obj) {
    heard(Heard::Looks);
}

/// What the person asked the system for: dark or light, the accent colour, more contrast, less
/// motion.
fn preferences() -> Preferences {
    // SAFETY: plain questions of the application, the colour and the workspace.
    unsafe {
        let app: Obj = send(class(c"NSApplication"), c"sharedApplication", ());
        let looks: Obj = send(app, c"effectiveAppearance", ());
        let dark = rust_string(send(looks, c"name", ())).is_some_and(|name| name.contains("Dark"));
        let srgb: Obj = send(class(c"NSColorSpace"), c"sRGBColorSpace", ());
        let accent: Obj = send(send::<Obj>(class(c"NSColor"), c"controlAccentColor", ()), c"colorUsingColorSpace:", (srgb,));
        let channel = |name: &CStr| (send::<f64>(accent, name, ()).clamp(0.0, 1.0) * 255.0).round() as u32;
        let accent = (!accent.is_null()).then(|| Color::rgb(channel(c"redComponent") << 16 | channel(c"greenComponent") << 8 | channel(c"blueComponent")));
        let workspace: Obj = send(class(c"NSWorkspace"), c"sharedWorkspace", ());
        let contrast = send::<Bool>(workspace, c"accessibilityDisplayShouldIncreaseContrast", ()) != 0;
        let still = send::<Bool>(workspace, c"accessibilityDisplayShouldReduceMotion", ()) != 0;
        let high_contrast = contrast.then_some(if dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
        Preferences { dark, accent, high_contrast, reduced_motion: still, ..Preferences::default() }
    }
}

/// The height of the main screen, in points: what turns AppKit's places (from the bottom
/// left, upward) into the desktop's (from the top left, downward).
fn desk_height() -> f64 {
    // SAFETY: plain questions; the first screen is the main one.
    unsafe {
        let screens: Obj = send(class(c"NSScreen"), c"screens", ());
        send_rect(array_items(screens).first().copied().unwrap_or(std::ptr::null_mut()), c"frame", ()).size.height
    }
}

/// An AppKit rectangle on the desktop as the library has it: from the top left, downward.
fn on_desk(frame: NSRect, height: f64) -> Rect {
    let top = height - frame.origin.y - frame.size.height;
    Rect { x: frame.origin.x as f32, y: top as f32, w: frame.size.width as f32, h: frame.size.height as f32 }
}

/// The screens, main one first: where each lies, its work area (without the menu bar and the
/// Dock), and its scale.
fn screens() -> Vec<Screen> {
    let height = desk_height();
    // SAFETY: plain questions of each screen.
    unsafe {
        let screens: Obj = send(class(c"NSScreen"), c"screens", ());
        let screen = |of: Obj| Screen {
            rect: on_desk(send_rect(of, c"frame", ()), height),
            work_area: on_desk(send_rect(of, c"visibleFrame", ()), height),
            scale: send::<f64>(of, c"backingScaleFactor", ()) as f32,
        };
        array_items(screens).into_iter().map(screen).collect()
    }
}

// ---- the view ----

/// The library's view class: input, text input, and the window's delegate.
pub(crate) fn view_class() -> Obj {
    let (event, plain, yes) = ("v@:@".to_string(), "v@:".to_string(), format!("{BOOL}@:"));
    let pointer = [
        c"mouseDown:",
        c"mouseUp:",
        c"rightMouseDown:",
        c"rightMouseUp:",
        c"otherMouseDown:",
        c"otherMouseUp:",
        c"mouseMoved:",
        c"mouseDragged:",
        c"rightMouseDragged:",
        c"otherMouseDragged:",
        c"mouseExited:",
        c"mouseEntered:",
    ];
    let mut methods: Vec<(&CStr, *const c_void, String)> = pointer.into_iter().map(|name| (name, pointed as *const c_void, event.clone())).collect();
    let heard_of: [(&CStr, *const c_void); 18] = [
        (c"scrollWheel:", scrolled as *const c_void),
        (c"magnifyWithEvent:", magnified as *const c_void),
        (c"keyDown:", key_down as *const c_void),
        (c"keyUp:", key_up as *const c_void),
        (c"flagsChanged:", flags_changed as *const c_void),
        (c"cursorUpdate:", cursor_update as *const c_void),
        (c"windowDidResize:", window_resized as *const c_void),
        (c"windowDidChangeBackingProperties:", window_rescaled as *const c_void),
        (c"windowDidMove:", window_moved as *const c_void),
        (c"windowDidEnterFullScreen:", window_moved as *const c_void),
        (c"windowDidExitFullScreen:", window_moved as *const c_void),
        (c"windowDidBecomeKey:", window_focused as *const c_void),
        (c"windowDidResignKey:", window_unfocused as *const c_void),
        (c"windowDidMiniaturize:", window_minimized as *const c_void),
        (c"windowDidDeminiaturize:", window_restored as *const c_void),
        (c"insertText:", inserted_alone as *const c_void),
        (c"viewDidChangeBackingProperties", view_rescaled as *const c_void),
        (c"viewDidChangeEffectiveAppearance", view_looks as *const c_void),
    ];
    methods.extend(heard_of.into_iter().map(|(name, imp)| (name, imp, if name.to_bytes().ends_with(b":") { event.clone() } else { plain.clone() })));
    methods.extend([
        (c"isFlipped", yes_please as *const c_void, yes.clone()),
        (c"acceptsFirstResponder", yes_please as *const c_void, yes.clone()),
        (c"canBecomeKeyView", yes_please as *const c_void, yes.clone()),
        (c"acceptsFirstMouse:", accepts_first_mouse as *const c_void, format!("{BOOL}@:@")),
        (c"windowShouldClose:", should_close as *const c_void, format!("{BOOL}@:@")),
        (c"unmarkText", unmarked as *const c_void, plain.clone()),
        (c"doCommandBySelector:", commanded as *const c_void, "v@::".to_string()),
        (c"insertText:replacementRange:", inserted as *const c_void, format!("v@:@{RANGE}")),
        (c"setMarkedText:selectedRange:replacementRange:", marked as *const c_void, format!("v@:@{RANGE}{RANGE}")),
        (c"hasMarkedText", has_marked as *const c_void, yes.clone()),
        (c"markedRange", marked_range as *const c_void, format!("{RANGE}@:")),
        (c"selectedRange", selected_range as *const c_void, format!("{RANGE}@:")),
        (c"validAttributesForMarkedText", no_attributes as *const c_void, "@@:".to_string()),
        (c"attributedSubstringForProposedRange:actualRange:", no_substring as *const c_void, format!("@@:{RANGE}^{RANGE}")),
        (c"characterIndexForPoint:", no_index as *const c_void, format!("Q@:{POINT}")),
        (c"firstRectForCharacterRange:actualRange:", first_rect as *const c_void, format!("{RECT}@:{RANGE}^{RANGE}")),
    ]);
    methods.extend(crate::ns_accessibility::view_methods());
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautView", c"NSView", &listed, &[c"NSTextInputClient", c"NSWindowDelegate"])
}

/// The library's window class: one that takes the keyboard even without a title bar.
fn window_class() -> Obj {
    let yes = yes_please as extern "C" fn(Obj, Sel) -> Bool as *const c_void;
    let types = format!("{BOOL}@:");
    made_class(c"TautWindow", c"NSWindow", &[(c"canBecomeKeyWindow", yes, &types), (c"canBecomeMainWindow", yes, &types)], &[])
}

extern "C" fn yes_please(_: Obj, _: Sel) -> Bool {
    1
}

extern "C" fn accepts_first_mouse(_: Obj, _: Sel, _: Obj) -> Bool {
    1
}

/// The modifiers held, as the library has them: Command and Control are both `ctrl`.
fn mods_of(flags: usize) -> Mods {
    Mods { ctrl: flags & (CONTROL | COMMAND) != 0, shift: flags & SHIFT != 0, alt: flags & OPTION != 0 }
}

/// An event's modifiers.
fn mods(event: Obj) -> Mods {
    // SAFETY: an event answers `modifierFlags`.
    mods_of(unsafe { send::<usize>(event, c"modifierFlags", ()) })
}

/// Where an event happened in a view, in units from its top left.
fn place_in(view: Obj, event: Obj) -> Point {
    // SAFETY: the event's place in its window, turned into the (flipped) view's.
    let at: NSPoint = unsafe { send(view, c"convertPoint:fromView:", (send::<NSPoint>(event, c"locationInWindow", ()), std::ptr::null_mut::<c_void>())) };
    pt(at.x as f32, at.y as f32)
}

/// Pointer events: a press or release of a button (with the place first), a move, the pointer
/// leaving.
extern "C" fn pointed(view: Obj, _: Sel, event: Obj) {
    // SAFETY: an event answers its type and button.
    let (kind, button) = unsafe { (send::<usize>(event, c"type", ()), send::<isize>(event, c"buttonNumber", ())) };
    if kind == EXITED {
        return heard(Heard::Event(view, Event::Leave));
    }
    heard(Heard::Event(view, Event::Move(place_in(view, event))));
    let which = match (kind, button) {
        (LEFT_DOWN | LEFT_UP, _) => Button::Left,
        (RIGHT_DOWN | RIGHT_UP, _) => Button::Right,
        (OTHER_DOWN | OTHER_UP, 2) => Button::Middle,
        _ => return,
    };
    if matches!(kind, LEFT_DOWN | RIGHT_DOWN | OTHER_DOWN) {
        keep_press(view, event);
        heard(Heard::Event(view, Event::Press(which, mods(event))));
    } else {
        heard(Heard::Event(view, Event::Release(which)));
    }
}

/// Keeps a press, for moving the window by the app's own title bar (see `Host::grip`).
fn keep_press(view: Obj, event: Obj) {
    // SAFETY: the event is retained while kept, and the one before released.
    let kept: Obj = unsafe { send(event, c"retain", ()) };
    let before = with_view(view, |state| std::mem::replace(&mut state.down, kept)).unwrap_or(kept);
    unsafe { send::<()>(before, c"release", ()) };
}

/// The wheel or a touchpad: precise distances in points, or lines of a wheel's notches. With
/// Command or Control a wheel zooms.
extern "C" fn scrolled(view: Obj, _: Sel, event: Obj) {
    // SAFETY: an event answers these.
    let (precise, x, y) = unsafe { (send::<Bool>(event, c"hasPreciseScrollingDeltas", ()) != 0, send::<f64>(event, c"scrollingDeltaX", ()), send::<f64>(event, c"scrollingDeltaY", ())) };
    let per = if precise { 1.0 } else { NOTCH };
    let (right, down) = (-x as f32 * per, -y as f32 * per);
    if (right, down) == (0.0, 0.0) {
        return;
    }
    heard(Heard::Event(view, Event::Move(place_in(view, event))));
    let zoom = mods(event).ctrl && !precise && down != 0.0;
    heard(Heard::Event(view, if zoom { Event::Zoom(crate::touch::wheel_zoom(-down / NOTCH)) } else { Event::Wheel(right, down) }));
}

/// A touchpad pinch.
extern "C" fn magnified(view: Obj, _: Sel, event: Obj) {
    // SAFETY: a magnify event answers this.
    let by = unsafe { send::<f64>(event, c"magnification", ()) } as f32;
    heard(Heard::Event(view, Event::Move(place_in(view, event))));
    heard(Heard::Event(view, Event::Zoom((1.0 + by).max(0.1))));
}

/// The key a key code names, whatever the layout; for another key, the letter it types
/// without modifiers.
fn key_of(event: Obj) -> Option<Key> {
    // SAFETY: a key event answers these.
    let code = unsafe { send::<u16>(event, c"keyCode", ()) };
    let named = match code {
        0x24 | 0x4c => Key::Enter,
        0x35 => Key::Escape,
        0x30 => Key::Tab,
        0x33 => Key::Backspace,
        0x75 => Key::Delete,
        0x7b => Key::Left,
        0x7c => Key::Right,
        0x7e => Key::Up,
        0x7d => Key::Down,
        0x73 => Key::Home,
        0x77 => Key::End,
        0x74 => Key::PageUp,
        0x79 => Key::PageDown,
        0x31 => Key::Space,
        0x6e => Key::Menu,
        _ => return function_key(code).or_else(|| letter_of(event)),
    };
    Some(named)
}

/// F1 to F12, by key code.
fn function_key(code: u16) -> Option<Key> {
    const CODES: [u16; 12] = [0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f];
    CODES.iter().position(|of| *of == code).map(|at| Key::F(at as u8 + 1))
}

/// The letter a key types without modifiers (but Shift, which is taken off), lower case.
fn letter_of(event: Obj) -> Option<Key> {
    // SAFETY: a key event answers this.
    let typed = rust_string(unsafe { send(event, c"charactersIgnoringModifiers", ()) })?;
    let letter = typed.chars().next()?.to_lowercase().next()?;
    // AppKit gives the arrows and such as private-use characters: those are named above.
    (!letter.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&letter)).then_some(Key::Char(letter))
}

/// A key went down: the key, then what it typed through the system's text input. With Command
/// or Control held, only the key. While an input method composes, keys are its own.
extern "C" fn key_down(view: Obj, _: Sel, event: Obj) {
    let (held, key) = (mods(event), key_of(event));
    // SAFETY: an event answers its flags.
    let command = unsafe { send::<usize>(event, c"modifierFlags", ()) } & (COMMAND | CONTROL) != 0;
    let was_marked = with_view(view, |state| {
        state.typed = Some(String::new());
        state.marked
    });
    if !command {
        // SAFETY: the view is an `NSTextInputClient`; AppKit calls back into it from here.
        unsafe { send::<()>(view, c"interpretKeyEvents:", (ns_array(&[event]),)) };
    }
    let (typed, marked) = with_view(view, |state| (state.typed.take().unwrap_or_default(), state.marked)).unwrap_or_default();
    if let Some(key) = key.filter(|_| was_marked == Some(false) && !marked) {
        heard(Heard::Event(view, Event::Key(key, held)));
    }
    if !typed.is_empty() {
        heard(Heard::Event(view, Event::Text(typed)));
    }
}

/// A key came up.
extern "C" fn key_up(view: Obj, _: Sel, event: Obj) {
    if let Some(key) = key_of(event) {
        heard(Heard::Event(view, Event::KeyUp(key)));
    }
}

/// Shift, Control, Option or Command went down or up.
extern "C" fn flags_changed(view: Obj, _: Sel, event: Obj) {
    let now = mods(event);
    if with_view(view, |state| std::mem::replace(&mut state.held, now) != now) == Some(true) {
        heard(Heard::Event(view, Event::ModifiersChanged(now)));
    }
}

/// The pointer came over the view: its cursor is shown.
extern "C" fn cursor_update(view: Obj, _: Sel, _: Obj) {
    if let Some(cursor) = with_view(view, |state| state.cursor) {
        show_cursor(cursor);
    }
}

/// Shows a cursor shape.
fn show_cursor(cursor: Cursor) {
    let name = match cursor {
        Cursor::Arrow => c"arrowCursor",
        Cursor::Hand => c"pointingHandCursor",
        Cursor::Text => c"IBeamCursor",
        Cursor::ResizeX => c"resizeLeftRightCursor",
        Cursor::ResizeY => c"resizeUpDownCursor",
        Cursor::ResizeDown => c"_windowResizeNorthWestSouthEastCursor",
        Cursor::ResizeUp => c"_windowResizeNorthEastSouthWestCursor",
        Cursor::Move => c"openHandCursor",
        Cursor::No => c"operationNotAllowedCursor",
    };
    // SAFETY: class methods of `NSCursor`; a diagonal one a system lacks is a crosshair.
    unsafe {
        let has = send::<Bool>(class(c"NSCursor"), c"respondsToSelector:", (sel(name),)) != 0;
        let shape: Obj = send(class(c"NSCursor"), if has { name } else { c"crosshairCursor" }, ());
        send::<()>(shape, c"set", ());
    }
}

extern "C" fn view_rescaled(view: Obj, _: Sel) {
    heard(Heard::Resized(view));
}

extern "C" fn view_looks(_: Obj, _: Sel) {
    heard(Heard::Looks);
}

/// The window's size changed: noted, and while the person drags its edge, it is run and drawn
/// at once.
extern "C" fn window_resized(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Resized(view));
    // SAFETY: a view answers this.
    if unsafe { send::<Bool>(view, c"inLiveResize", ()) } != 0 {
        live_turn();
    }
}

extern "C" fn window_rescaled(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Resized(view));
}

extern "C" fn window_moved(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Moved(view));
}

extern "C" fn window_focused(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Focused(view, true));
}

extern "C" fn window_unfocused(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Focused(view, false));
}

extern "C" fn window_minimized(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Minimized(view, true));
}

extern "C" fn window_restored(view: Obj, _: Sel, _: Obj) {
    heard(Heard::Minimized(view, false));
}

/// The close button: the app is asked; the window stays until it agrees.
extern "C" fn should_close(view: Obj, _: Sel, _: Obj) -> Bool {
    heard(Heard::Close(view));
    0
}

// ---- text input (`NSTextInputClient`) ----

/// The text in a plain or attributed string.
fn text_of(string: Obj) -> String {
    // SAFETY: both kinds answer `isKindOfClass:`; an attributed one answers `string`.
    let attributed = unsafe { send::<Bool>(string, c"isKindOfClass:", (class(c"NSAttributedString"),)) } != 0;
    let plain = if attributed { unsafe { send(string, c"string", ()) } } else { string };
    rust_string(plain).unwrap_or_default()
}

/// Text typed or committed by an input method: kept for the key that typed it, or heard now.
fn typed(view: Obj, text: String) {
    let kept = with_view(view, |state| {
        state.marked = false;
        state.typed.as_mut().map(|typed| typed.push_str(&text)).is_some()
    });
    if kept == Some(false) && !text.is_empty() {
        heard(Heard::Event(view, Event::Text(text)));
    }
}

extern "C" fn inserted(view: Obj, _: Sel, string: Obj, _: NSRange) {
    typed(view, text_of(string));
}

extern "C" fn inserted_alone(view: Obj, _: Sel, string: Obj) {
    typed(view, text_of(string));
}

/// An input method composes: shown by itself; the view only notes that it does.
extern "C" fn marked(view: Obj, _: Sel, string: Obj, _: NSRange, _: NSRange) {
    let composing = !text_of(string).is_empty();
    with_view(view, |state| state.marked = composing);
}

extern "C" fn unmarked(view: Obj, _: Sel) {
    with_view(view, |state| state.marked = false);
}

extern "C" fn has_marked(view: Obj, _: Sel) -> Bool {
    with_view(view, |state| state.marked).unwrap_or(false) as Bool
}

extern "C" fn marked_range(view: Obj, _: Sel) -> NSRange {
    match with_view(view, |state| state.marked) {
        Some(true) => NSRange { location: 0, length: 1 },
        _ => NSRange { location: NOT_FOUND, length: 0 },
    }
}

extern "C" fn selected_range(_: Obj, _: Sel) -> NSRange {
    NSRange { location: 0, length: 0 }
}

extern "C" fn no_attributes(_: Obj, _: Sel) -> Obj {
    ns_array(&[])
}

extern "C" fn no_substring(_: Obj, _: Sel, _: NSRange, _: *mut NSRange) -> Obj {
    std::ptr::null_mut()
}

extern "C" fn no_index(_: Obj, _: Sel, _: NSPoint) -> usize {
    NOT_FOUND
}

/// Keys the text system turns into commands (arrows, Return): already heard as keys.
extern "C" fn commanded(_: Obj, _: Sel, _: Sel) {}

/// Where the text being typed is, on the screen: where the input method's window goes.
extern "C" fn first_rect(view: Obj, _: Sel, _: NSRange, _: *mut NSRange) -> NSRect {
    let place = with_view(view, |state| state.ime).flatten().unwrap_or_default();
    let rect = NSRect::new(f64::from(place.x), f64::from(place.y), f64::from(place.w.max(1.0)), f64::from(place.h));
    // SAFETY: the view's rectangle turned into its window's, then the screen's.
    unsafe {
        let window: Obj = send(view, c"window", ());
        let in_window = send_rect(view, c"convertRect:toView:", (rect, std::ptr::null_mut::<c_void>()));
        send_rect(window, c"convertRectToScreen:", (in_window,))
    }
}

// ---- the loop ----

/// What a window being resized runs from inside AppKit's resize (see `live_turn`).
trait Turns {
    /// One turn of the loop.
    fn turn(&mut self);
}

/// While the person drags a window's edge, the loop's turn runs from inside AppKit's own: the
/// window is redrawn at each size. Not while a turn is running already.
fn live_turn() {
    let Some(host) = LIVE.get() else { return };
    if TURNING.get() {
        return;
    }
    // SAFETY: the host lent itself for the length of its wait for events (`Host::wait`), and
    // touches nothing of itself meanwhile.
    unsafe { (*host).turn() }
}

/// One window, and what keeps an interface shown in it.
struct Shown {
    /// The first window's interface's id for it, if a second window.
    known: Option<Id>,
    window: Obj,
    view: Obj,
    title: String,
    /// Its size in units and its scale; its pixels.
    size: Size,
    scale: f32,
    surface: Surface,
    see_through: bool,
    frameless: bool,
    kind: WindowKind,
    /// It is minimized now.
    minimized: bool,
}

/// What a window opens with.
struct Opening<'a> {
    known: Option<Id>,
    title: &'a str,
    size: Size,
    min_size: Size,
    open_at: OpenAt,
    frameless: bool,
    transparent: bool,
    kind: WindowKind,
}

impl Shown {
    /// Opens a window (not yet shown).
    fn open(opening: Opening) -> Shown {
        let (view_class, window_class) = (view_class(), window_class());
        let frame = NSRect::new(0.0, 0.0, f64::from(opening.size.w.max(1.0)), f64::from(opening.size.h.max(1.0)));
        let style = match (opening.transparent, opening.frameless) {
            (true, true) => RESIZABLE,
            (true, false) => 0,
            (false, true) => TITLED | CLOSABLE | MINIATURIZABLE | RESIZABLE | FULL_SIZE_CONTENT,
            (false, false) => TITLED | CLOSABLE | MINIATURIZABLE | RESIZABLE,
        };
        // SAFETY: a window and view of the library's classes, made and set up on the main
        // thread; the window keeps the view, and is released when it closes (`close`).
        unsafe {
            let window: Obj = send(class_alloc(window_class), c"initWithContentRect:styleMask:backing:defer:", (frame, style, 2usize, 0 as Bool));
            let view: Obj = send(class_alloc(view_class), c"initWithFrame:", (frame,));
            send::<()>(window, c"setReleasedWhenClosed:", (0 as Bool,));
            send::<()>(window, c"setTitle:", (ns_string(opening.title),));
            send::<()>(window, c"setContentMinSize:", (NSSize { width: f64::from(opening.min_size.w), height: f64::from(opening.min_size.h) },));
            send::<()>(window, c"setContentView:", (view,));
            send::<()>(window, c"setDelegate:", (view,));
            send::<()>(window, c"setAcceptsMouseMovedEvents:", (1 as Bool,));
            send::<Bool>(window, c"makeFirstResponder:", (view,));
            send::<()>(view, c"release", ());
            prepare_view(view, opening.transparent);
            Shown::dressed(window, opening.frameless, opening.transparent, opening.kind);
            Shown::placed(window, opening.open_at);
            let scale = send::<f64>(window, c"backingScaleFactor", ()) as f32;
            let size = Shown::content_size(view);
            VIEWS.with_borrow_mut(|views| views.push(ViewState { view, cursor: Cursor::Arrow, ime: None, marked: false, typed: None, held: Mods::NONE, down: std::ptr::null_mut() }));
            if let Some(drops) = crate::macos_files::DROPS.get() {
                (drops.take)(view);
            }
            let (frameless, kind) = (opening.frameless, opening.kind);
            Shown { known: opening.known, window, view, title: opening.title.into(), size, scale, surface: Surface::default(), see_through: opening.transparent, frameless, kind, minimized: false }
        }
    }

    /// How a window looks: frameless (its content under a hidden title bar, which still moves
    /// it), see-through, and how it stacks.
    fn dressed(window: Obj, frameless: bool, transparent: bool, kind: WindowKind) {
        // SAFETY: plain setup of a window of the library's.
        unsafe {
            if frameless && !transparent {
                send::<()>(window, c"setTitlebarAppearsTransparent:", (1 as Bool,));
                send::<()>(window, c"setTitleVisibility:", (1isize,));
                (0..3usize).for_each(|button| send::<()>(send::<Obj>(window, c"standardWindowButton:", (button,)), c"setHidden:", (1 as Bool,)));
            }
            if transparent {
                send::<()>(window, c"setOpaque:", (0 as Bool,));
                send::<()>(window, c"setBackgroundColor:", (send::<Obj>(class(c"NSColor"), c"clearColor", ()),));
                send::<()>(window, c"setHasShadow:", (0 as Bool,));
            }
            let above = matches!(kind, WindowKind::Tool | WindowKind::Dialog | WindowKind::OnTop);
            send::<()>(window, c"setLevel:", (if above { FLOATING_LEVEL } else { NORMAL_LEVEL },));
        }
    }

    /// Puts a window where its app asked (see `screens::spot`): its frame's top left at a
    /// desktop place, on the screen there or the nearest; in the middle of a screen, or of the
    /// window it belongs to. Else AppKit puts it in the middle of the main screen.
    fn placed(window: Obj, asked: OpenAt) {
        // SAFETY: plain questions and a move of a window of the library's.
        unsafe {
            let frame = send_rect(window, c"frame", ());
            let size = [frame.size.width as f32, frame.size.height as f32];
            let Some((_, at)) = window_origin(&screens(), asked, |_| size) else { return send::<()>(window, c"center", ()) };
            send::<()>(window, c"setFrameTopLeftPoint:", (NSPoint { x: f64::from(at.x), y: desk_height() - f64::from(at.y) },));
        }
    }

    /// A view's size, in units.
    fn content_size(view: Obj) -> Size {
        // SAFETY: a view answers its bounds.
        let bounds = unsafe { send_rect(view, c"bounds", ()) };
        Size { w: bounds.size.width as f32, h: bounds.size.height as f32 }
    }

    /// Takes in the window's size and scale now; says whether either changed.
    fn measure(&mut self) -> bool {
        // SAFETY: a window answers its scale.
        let (size, scale) = (Shown::content_size(self.view), unsafe { send::<f64>(self.window, c"backingScaleFactor", ()) } as f32);
        let changed = (size, scale) != (self.size, self.scale);
        (self.size, self.scale) = (size, scale);
        changed
    }

    /// Where the window is, as its app keeps it: its frame's top left, its content size, and
    /// whether it fills its screen (zoomed). And whether it is full screen.
    fn placement(&self) -> (WindowPlacement, bool) {
        // SAFETY: plain questions of a window.
        unsafe {
            let frame = send_rect(self.window, c"frame", ());
            let at = on_desk(frame, desk_height());
            let whole = send::<usize>(self.window, c"styleMask", ()) & (1 << 14) != 0;
            let zoomed = send::<Bool>(self.window, c"isZoomed", ()) != 0 && !whole;
            (WindowPlacement { position: (at.x.round() as i32, at.y.round() as i32), width: self.size.w, height: self.size.h, maximized: zoomed }, whole)
        }
    }

    /// Hands the window's pixels to its layer, whole, after `changed` places of them were drawn
    /// again.
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
            let info = if self.see_through { PREMULTIPLIED } else { SOLID };
            let picture = CGImageCreate(width, height, 8, 32, width * 4, space, info, provider, std::ptr::null(), false, 0);
            let layer: Obj = send(self.view, c"layer", ());
            send::<()>(class(c"CATransaction"), c"begin", ());
            send::<()>(class(c"CATransaction"), c"setDisableActions:", (1 as Bool,));
            send::<()>(layer, c"setContentsScale:", (f64::from(self.scale),));
            send::<()>(layer, c"setContents:", (picture,));
            send::<()>(class(c"CATransaction"), c"commit", ());
            CGImageRelease(picture);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
            CFRelease(bytes);
        }
    }

    /// Shows the window, at the front.
    fn show(&self, front: bool) {
        // SAFETY: a window of the library's.
        unsafe { send::<()>(self.window, if front { c"makeKeyAndOrderFront:" } else { c"orderFront:" }, (std::ptr::null_mut::<c_void>(),)) };
    }

    /// Closes the window and lets it go.
    fn close(self) {
        VIEWS.with_borrow_mut(|views| views.retain(|state| state.view != self.view));
        crate::ns_accessibility::forget(self.view);
        // SAFETY: the window is the library's, made with a count of one, which it gives up
        // here; its delegate is unset first, so it says nothing more.
        unsafe {
            send::<()>(self.window, c"setDelegate:", (std::ptr::null_mut::<c_void>(),));
            send::<()>(self.window, c"orderOut:", (std::ptr::null_mut::<c_void>(),));
            send::<()>(self.window, c"close", ());
            send::<()>(self.window, c"release", ());
        }
    }
}

/// A class's `alloc`.
///
/// # Safety
/// `class` is a class.
unsafe fn class_alloc(class: Obj) -> Obj {
    // SAFETY: as the caller promises.
    unsafe { send(class, c"alloc", ()) }
}

/// A view's layer, which shows its pixels: at its top left, unscaled, never drawn by AppKit; a
/// pointer tracking area over all of it.
fn prepare_view(view: Obj, transparent: bool) {
    // SAFETY: plain setup of a view of the library's; the tracking area is kept by the view.
    unsafe {
        send::<()>(view, c"setWantsLayer:", (1 as Bool,));
        send::<()>(view, c"setLayerContentsRedrawPolicy:", (0isize,));
        let layer: Obj = send(view, c"layer", ());
        send::<()>(layer, c"setContentsGravity:", (kCAGravityTopLeft,));
        send::<()>(layer, c"setOpaque:", (Bool::from(!transparent),));
        // Entered and exited, moves, cursor updates, always, over the visible rectangle.
        let options = 0x01usize | 0x02 | 0x04 | 0x80 | 0x200;
        let area: Obj = send(class_alloc(class(c"NSTrackingArea")), c"initWithRect:options:owner:userInfo:", (NSRect::default(), options, view, std::ptr::null_mut::<c_void>()));
        send::<()>(view, c"addTrackingArea:", (area,));
        send::<()>(area, c"release", ());
    }
}

/// Runs work in the interface of a window: the first's, or a second one's.
fn within<T>(first: &mut Ui, known: Option<Id>, work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    match known {
        Some(known) => first.in_window(known, work),
        None => Some(work(first)),
    }
}

/// The windows and their interfaces.
struct Host<App> {
    app: App,
    /// The first window's interface, which holds the second windows' interfaces.
    ui: Ui,
    windows: Vec<Shown>,
    rerun: bool,
    done: bool,
    start: Instant,
    /// The window the pointer is over, if any.
    over: Option<Obj>,
}

impl<App: FnMut(&mut Ui)> Host<App> {
    /// Opens the first window and shows it.
    fn open(text: SystemText, window: Window, app: App) -> Host<App> {
        let application = application();
        let opening = Opening {
            known: None,
            title: window.title,
            size: Size { w: window.width, h: window.height },
            min_size: Size { w: window.min_width, h: window.min_height },
            open_at: window.position.map(|(x, y)| OpenAt::At(x, y)).or(window.screen.map(OpenAt::Middle)).unwrap_or_default(),
            frameless: window.frameless,
            transparent: window.transparent,
            kind: WindowKind::Plain,
        };
        let first = Shown::open(opening);
        set_icon(application, window.icons);
        let mut ui = Ui::new(Box::new(text), Box::new(SystemClipboard));
        if window.transparent {
            ui.theme.bg = Color::NONE;
        }
        ui.set_preferences(preferences());
        ui.set_screens(screens());
        ui.set_native_file_dialog(true);
        let mut host = Host { app, ui, windows: vec![first], rerun: true, done: false, start: Instant::now(), over: None };
        host.turn();
        let shown = &host.windows[0];
        // SAFETY: plain requests of the application and the window.
        unsafe {
            if window.maximized {
                send::<()>(shown.window, c"zoom:", (std::ptr::null_mut::<c_void>(),));
            }
            shown.show(!window.minimized);
            if window.minimized {
                send::<()>(shown.window, c"miniaturize:", (std::ptr::null_mut::<c_void>(),));
            } else {
                send::<()>(application, c"activateIgnoringOtherApps:", (1 as Bool,));
            }
        }
        host
    }

    /// The window a view is in.
    fn window_of(&self, view: Obj) -> Option<usize> {
        self.windows.iter().position(|shown| shown.view == view)
    }

    /// Takes in what AppKit said since the last turn.
    fn take_heard(&mut self) {
        for what in HEARD.with_borrow_mut(std::mem::take) {
            self.take(what);
        }
    }

    /// One thing AppKit said.
    fn take(&mut self, what: Heard) {
        let at = self.window_of(what.view());
        match (what, at) {
            (Heard::Looks, _) => self.rerun |= self.ui.set_preferences(preferences()) | self.ui.set_screens(screens()),
            (Heard::Quit, _) => self.closed(0),
            (Heard::Desktop, _) => self.rerun = true,
            (_, None) => {}
            (Heard::Event(_, event), Some(at)) => self.hear(at, event),
            (Heard::Close(_), Some(at)) => self.closed(at),
            (Heard::Resized(_), Some(at)) => self.rerun |= self.windows[at].measure(),
            (Heard::Moved(_), Some(at)) => self.place(at),
            (Heard::Minimized(_, down), Some(at)) => self.minimized(at, down),
            (Heard::Focused(_, on), Some(at)) => self.hear(at, Event::Focused(on)),
            (Heard::Other(_, work), Some(at)) => self.rerun |= within(&mut self.ui, self.windows[at].known, work) == Some(true),
        }
    }

    /// Passes what the user did in a window to its interface, noting whether it must run. A
    /// window a dialog keeps from input hears none; a press on it brings the dialog forward.
    fn hear(&mut self, at: usize, event: Event) {
        let blocked = self.ui.window_blocked(self.windows[at].known.unwrap_or(0));
        let input = !matches!(event, Event::Focused(_) | Event::Minimized(_) | Event::Leave);
        if blocked && input {
            let last = self.windows.iter().rposition(|shown| shown.kind == WindowKind::Dialog);
            return last.into_iter().for_each(|dialog| self.windows[dialog].show(matches!(event, Event::Press(..))));
        }
        match event {
            Event::Move(_) => self.over = Some(self.windows[at].view),
            Event::Leave => self.over = None,
            Event::Press(Button::Left, _) if self.grip(at) => return,
            _ => {}
        }
        self.rerun |= within(&mut self.ui, self.windows[at].known, |ui| ui.event(event)) == Some(true);
    }

    /// A press on the app's own title bar in a window without a frame moves the window, as the
    /// system's would. Says whether it was one.
    fn grip(&mut self, at: usize) -> bool {
        let shown = &self.windows[at];
        if !shown.frameless {
            return false;
        }
        let (view, window) = (shown.view, shown.window);
        let Some(down) = with_view(view, |state| state.down).filter(|down| !down.is_null()) else { return false };
        let place = place_in(view, down);
        let hit = within(&mut self.ui, shown.known, |ui| ui.hit_test(place, false));
        if hit != Some(crate::frameless::HitTest::TitleBar) {
            return false;
        }
        // SAFETY: the window moves with the pointer until the button comes up.
        unsafe { send::<()>(window, c"performWindowDragWithEvent:", (down,)) };
        true
    }

    /// The person closed a window. Its app hears in the next run: after it the first window
    /// goes and the program ends, unless the app says no; a second one goes when the app stops
    /// describing it.
    fn closed(&mut self, at: usize) {
        let Some(known) = self.windows[at].known else {
            self.ui.request_close();
            return self.rerun = true;
        };
        self.rerun |= self.ui.in_window(known, Ui::request_close).is_some();
    }

    /// A window was minimized or restored: its interface hears.
    fn minimized(&mut self, at: usize, down: bool) {
        self.windows[at].minimized = down;
        self.hear(at, Event::Minimized(down));
        self.place(at);
    }

    /// Tells a window's interface where the window is.
    fn place(&mut self, at: usize) {
        let (now, whole) = self.windows[at].placement();
        self.rerun |= within(&mut self.ui, self.windows[at].known, |ui| ui.set_window_placement(now) | ui.full_screen_now(whole)) == Some(true);
    }

    /// Runs the interface: each window's is told its size first; after, the windows are
    /// brought in line with what was described.
    fn describe(&mut self) {
        for shown in &self.windows {
            within(&mut self.ui, shown.known, |ui| ui.resize(shown.size, shown.scale));
        }
        self.ui.run(&mut self.app);
        let closes = self.ui.take_closing() == Some(Ok(()));
        (self.rerun, self.done) = (false, self.done || closes || self.ui.quitting());
        if let Some(stood) = crate::macos_shell::DESKTOP.get() {
            self.rerun |= stood(&mut self.ui, self.windows[0].window, self.windows[0].view);
        }
        self.keep_up();
        (0..self.windows.len()).for_each(|at| self.follow(at));
        if self.ui.listening {
            self.tell_readers();
        }
    }

    /// An assistive program asked: every window tells from now on (see `ns_accessibility`).
    fn listen(&mut self) {
        if self.ui.listening || !crate::ns_accessibility::listening() {
            return;
        }
        for shown in &self.windows {
            within(&mut self.ui, shown.known, |ui| ui.set_screen_reader(true));
        }
        self.rerun = true;
    }

    /// After a run an assistive program reads: each window's elements follow its nodes.
    fn tell_readers(&mut self) {
        for shown in &self.windows {
            within(&mut self.ui, shown.known, |ui| crate::ns_accessibility::keep(shown.view, ui));
        }
    }

    /// Closes the second windows the app stopped describing, and opens those described for the
    /// first time.
    fn keep_up(&mut self) {
        for at in (1..self.windows.len()).rev() {
            if !self.ui.windows().any(|(known, ..)| self.windows[at].known == Some(known)) {
                self.windows.remove(at).close();
            }
        }
        let new: Vec<(Id, String, Size)> =
            self.ui.windows().filter(|(known, ..)| self.windows.iter().all(|shown| shown.known != Some(*known))).map(|(known, title, size)| (known, title.into(), size)).collect();
        for (known, title, size) in new {
            let opens: WindowOpening = self.ui.window_opening(known).unwrap_or_default();
            // A dialog goes over the first window, unless its app says where.
            let over = || on_desk(unsafe { send_rect(self.windows[0].window, c"frame", ()) }, desk_height());
            let open_at = match (opens.position, opens.screen, opens.kind) {
                (Some((x, y)), ..) => OpenAt::At(x, y),
                (None, Some(nth), _) => OpenAt::Middle(nth),
                (None, None, WindowKind::Dialog) => OpenAt::Over(over()),
                _ => OpenAt::Any,
            };
            let opening = Opening { known: Some(known), title: &title, size, min_size: opens.min_size, open_at, frameless: opens.frameless, transparent: false, kind: opens.kind };
            let shown = Shown::open(opening);
            if self.ui.listening {
                self.ui.in_window(known, |ui| ui.set_screen_reader(true));
            }
            shown.show(true);
            self.windows.push(shown);
            self.rerun = true;
        }
    }

    /// After a run, does what a window's interface asks of the window: a title, a cursor, the
    /// text cursor's place for input methods, minimize, maximize, full screen, stay on top,
    /// come forward.
    fn follow(&mut self, at: usize) {
        let (view, drops) = (self.windows[at].view, crate::macos_files::DROPS.get());
        let asks = |ui: &mut Ui| {
            drops.inspect(|drops| (drops.note)(view, ui));
            (ui.take_title(), ui.cursor(), ui.ime_area(), ui.take_minimize(), ui.take_maximized(), ui.take_full_screen(), ui.take_on_top(), ui.take_bring_to_front(), ui.take_file_dialog_request())
        };
        let Some((title, cursor, ime, minimize, fill, whole, on_top, forward, files)) = within(&mut self.ui, self.windows[at].known, asks) else { return };
        if let Some((ask, files)) = crate::macos_files::DIALOG.get().zip(files) {
            ask(self.windows[at].window, view, files);
        }
        let (shown, over) = (&mut self.windows[at], self.over);
        with_view(shown.view, |state| (state.cursor, state.ime) = (cursor, ime));
        if over == Some(shown.view) {
            show_cursor(cursor);
        }
        let window = shown.window;
        // SAFETY: plain requests of a window of the library's.
        unsafe {
            if let Some(title) = title {
                send::<()>(window, c"setTitle:", (ns_string(&title),));
                shown.title = title;
            }
            if minimize {
                send::<()>(window, c"miniaturize:", (std::ptr::null_mut::<c_void>(),));
            }
            let (zoomed, full) = (send::<Bool>(window, c"isZoomed", ()) != 0, send::<usize>(window, c"styleMask", ()) & (1 << 14) != 0);
            if fill.is_some_and(|fill| fill != zoomed) {
                send::<()>(window, c"zoom:", (std::ptr::null_mut::<c_void>(),));
            }
            if whole.is_some_and(|whole| whole != full) {
                send::<()>(window, c"toggleFullScreen:", (std::ptr::null_mut::<c_void>(),));
            }
            if let Some(on_top) = on_top {
                send::<()>(window, c"setLevel:", (if on_top { FLOATING_LEVEL } else { NORMAL_LEVEL },));
            }
            if forward {
                send::<()>(APP.load(Ordering::SeqCst), c"activateIgnoringOtherApps:", (1 as Bool,));
                shown.show(true);
            }
        }
    }

    /// Draws what changed in a window and hands its pixels to the window.
    fn draw(&mut self, at: usize) {
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

    /// How long the loop may wait for events: until the interface's next timer, or not at all
    /// when a run is due.
    fn patience(&self) -> Option<Duration> {
        if self.rerun || !HEARD.with_borrow(Vec::is_empty) { Some(Duration::ZERO) } else { self.ui.wake_in() }
    }
}

impl<App: FnMut(&mut Ui)> Turns for Host<App> {
    /// One turn: takes in what happened, runs the interface if needed, and draws what changed.
    fn turn(&mut self) {
        TURNING.set(true);
        self.ui.set_time(self.start.elapsed());
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO);
        self.take_heard();
        self.listen();
        if self.rerun {
            self.describe();
        }
        (0..self.windows.len()).for_each(|at| self.draw(at));
        TURNING.set(false);
    }
}

impl<App> Drop for Host<App> {
    fn drop(&mut self) {
        self.windows.drain(..).for_each(Shown::close);
        HEARD.with_borrow_mut(Vec::clear);
    }
}

/// Waits for events, at most `patience` (for ever without), and hands each one on to AppKit.
/// Meanwhile a window being resized may run `host`'s turn (see `live_turn`). Says whether a
/// [`Waker`] woke the loop.
fn wait(app: Obj, patience: Option<Duration>, host: *mut dyn Turns) -> bool {
    LIVE.set(Some(host));
    let mut woken = false;
    // SAFETY: events are taken from the main thread's queue and handed back to AppKit.
    unsafe {
        let until: Obj = match patience {
            Some(wait) => send(class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", (wait.as_secs_f64(),)),
            None => send(class(c"NSDate"), c"distantFuture", ()),
        };
        let mut next: Obj = send(app, c"nextEventMatchingMask:untilDate:inMode:dequeue:", (usize::MAX, until, NSDefaultRunLoopMode, 1 as Bool));
        while !next.is_null() {
            woken |= is_wake(next);
            send::<()>(app, c"sendEvent:", (next,));
            let now: Obj = send(class(c"NSDate"), c"distantPast", ());
            next = send(app, c"nextEventMatchingMask:untilDate:inMode:dequeue:", (usize::MAX, now, NSDefaultRunLoopMode, 1 as Bool));
        }
    }
    LIVE.set(None);
    woken
}

/// Whether an event is a [`Waker`]'s: then the next wake posts again.
fn is_wake(event: Obj) -> bool {
    // SAFETY: an event answers its type, and an application event its subtype.
    let wake = unsafe { send::<usize>(event, c"type", ()) == APPLICATION && send::<i16>(event, c"subtype", ()) == WAKE };
    if wake {
        WOKEN.store(false, Ordering::SeqCst);
    }
    wake
}

/// The program's icon in the Dock: the largest picture given.
fn set_icon(app: Obj, icons: &[(usize, &[u32])]) {
    let Some((width, pixels)) = icons.iter().max_by_key(|(width, _)| *width) else { return };
    let height = pixels.len() / (*width).max(1);
    let Some(png) = crate::png::encode_rgba(*width, &pixels[..width * height]) else { return };
    // SAFETY: an image read from PNG bytes, handed to the application, which keeps it.
    unsafe {
        let image: Obj = send(class_alloc(class(c"NSImage")), c"initWithData:", (ns_data(&png),));
        send::<()>(app, c"setApplicationIconImage:", (image,));
        send::<()>(image, c"release", ());
    }
}

/// Opens the window and runs the app in it until the window is closed.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    let _pool = Pool::new();
    // SAFETY: a plain question.
    if unsafe { send::<Bool>(class(c"NSThread"), c"isMainThread", ()) } == 0 {
        return eprintln!("taut: macOS shows windows only from the program's main thread.");
    }
    let Some(text) = SystemText::new() else {
        return eprintln!("taut: CoreText has no system font.");
    };
    let mut host = Host::open(text, window, app);
    let application = application();
    while !host.done {
        let _pool = Pool::new();
        let patience = host.patience();
        let lent: *mut dyn Turns = &mut host;
        host.rerun |= wait(application, patience, lent);
        host.turn();
    }
}

// ---- the clipboard ----

/// The system clipboard (the general pasteboard), for an interface.
struct SystemClipboard;

/// The general pasteboard.
fn pasteboard() -> Obj {
    // SAFETY: a plain question.
    unsafe { send(class(c"NSPasteboard"), c"generalPasteboard", ()) }
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        let _pool = Pool::new();
        // SAFETY: a plain question of the pasteboard.
        rust_string(unsafe { send(pasteboard(), c"stringForType:", (NSPasteboardTypeString,)) })
    }
    fn set(&mut self, text: &str) {
        let _pool = Pool::new();
        // SAFETY: the pasteboard is cleared, then given the text.
        unsafe {
            send::<isize>(pasteboard(), c"clearContents", ());
            send::<Bool>(pasteboard(), c"setString:forType:", (ns_string(text), NSPasteboardTypeString));
        }
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

impl SystemClipboard {
    /// A picture, as PNG; or as TIFF (what many programs copy), turned into PNG by AppKit.
    fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        let _pool = Pool::new();
        let board = pasteboard();
        // SAFETY: plain questions of the pasteboard; a TIFF is read by an image rep, which
        // writes it again as PNG (4) with no properties.
        unsafe {
            let png: Obj = send(board, c"dataForType:", (NSPasteboardTypePNG,));
            if !png.is_null() {
                return crate::png::decode(&bytes_of(png));
            }
            let tiff: Obj = send(board, c"dataForType:", (NSPasteboardTypeTIFF,));
            let rep: Obj = send(class(c"NSBitmapImageRep"), c"imageRepWithData:", (tiff,));
            let properties: Obj = send(class(c"NSDictionary"), c"dictionary", ());
            let png: Obj = send(rep, c"representationUsingType:properties:", (4usize, properties));
            crate::png::decode(&bytes_of(png))
        }
    }
    /// Puts a picture on the clipboard as PNG.
    fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
        let _pool = Pool::new();
        // SAFETY: the pasteboard is cleared, then given the bytes.
        unsafe {
            send::<isize>(pasteboard(), c"clearContents", ());
            send::<Bool>(pasteboard(), c"setData:forType:", (ns_data(&png), NSPasteboardTypePNG));
        }
    }
    /// The files on the clipboard: each item's file address.
    fn files_on(&mut self) -> Vec<PathBuf> {
        let _pool = Pool::new();
        // SAFETY: plain questions of the pasteboard's items.
        let items = array_items(unsafe { send(pasteboard(), c"pasteboardItems", ()) });
        items.into_iter().filter_map(|item| file_of(unsafe { send(item, c"stringForType:", (NSPasteboardTypeFileURL,)) })).collect()
    }
    /// Puts files on the clipboard, as file addresses.
    fn put_files(&mut self, files: &[PathBuf]) {
        let _pool = Pool::new();
        // SAFETY: the pasteboard is cleared, then given the addresses.
        unsafe {
            let urls: Vec<Obj> = files.iter().map(|file| send(class(c"NSURL"), c"fileURLWithPath:", (ns_string(&file.to_string_lossy()),))).collect();
            send::<isize>(pasteboard(), c"clearContents", ());
            send::<Bool>(pasteboard(), c"writeObjects:", (ns_array(&urls),));
        }
    }
}

/// The file a file address (`file:///...`) names.
pub(crate) fn file_of(address: Obj) -> Option<PathBuf> {
    // SAFETY: an address made from the string, asked for its path.
    let url: Obj = unsafe { send(class(c"NSURL"), c"URLWithString:", (address,)) };
    rust_string(unsafe { send(url, c"path", ()) }).map(PathBuf::from)
}
