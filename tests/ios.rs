//! taut on iOS, in the simulator. One file, two programs:
//!
//! - Built for the simulator, it is an app checking itself (`phone`): the gallery, or a page of
//!   parts it drives as a person would, through the methods UIKit calls (touches, keys, typed
//!   and composed text, the edit menu's actions, a screen reader's), then rotated. It writes
//!   what it saw into its temporary folder.
//! - On a Mac (`mac`), with `TAUT_IOS_SIMULATOR=1`, it builds that app and the gallery example
//!   as bundles (`examples/ios_bundle.rs`), boots the newest iPhone simulator, runs them, reads
//!   their notes, takes the screen (`simctl io screenshot`) and holds it against the same
//!   interface drawn on the Mac; switches the simulator to dark mode, and sends the app to the
//!   background and back. Without that variable it does nothing (a simulator takes minutes).
//!
//! `TAUT_SHOTS` names a folder for the pictures: the screen, and the Mac's drawing.
//! Run without libtest's harness (`harness = false`): UIKit runs only on the main thread.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    phone::main();
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(dead_code, unused_attributes)]
#[path = "../examples/gallery.rs"]
mod gallery;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../examples/ios_bundle.rs"]
mod ios_bundle;

/// The page of parts the app drives: a button counting presses, a text box, a tick, a slider.
#[cfg(target_os = "ios")]
#[derive(Debug, Default)]
struct Parts {
    presses: u32,
    text: String,
    tick: bool,
    amount: f32,
}

#[cfg(target_os = "ios")]
fn parts(parts: &mut Parts, ui: &mut taut::Ui) {
    ui.theme = ui.system_theme();
    ui.column().pad(16.0).gap(12.0).open();
    ui.label("Parts");
    parts.presses += ui.button("Press").clicked as u32;
    ui.text_box(&mut parts.text, "Type here");
    ui.checkbox(&mut parts.tick, "Tick");
    ui.slider(&mut parts.amount, 0.0..=10.0);
    ui.label(&format!("Pressed {} times", parts.presses));
    ui.close();
}

/// The page a reader reads: a paragraph that wraps onto several lines, a text box, and a table
/// of five hundred rows (those in view told).
#[cfg(target_os = "ios")]
#[derive(Debug)]
struct Reading {
    notes: String,
    table: taut::Table,
}

#[cfg(target_os = "ios")]
const PARAGRAPH: &str = "A paragraph long enough to break onto several lines of a phone's window, which a screen reader reads line by line and letter by letter.";

#[cfg(target_os = "ios")]
fn reading(page: &mut Reading, ui: &mut taut::Ui) {
    ui.theme = ui.system_theme();
    ui.column().pad(16.0).gap(12.0).grow().open();
    ui.paragraph(PARAGRAPH);
    ui.text_box(&mut page.notes, "Notes");
    let columns = [taut::Column::new("Name").px(160.0), taut::Column::new("Size").px(80.0)];
    ui.table(&mut page.table, &columns);
    for row in ui.rows(500, 24.0) {
        ui.selectable_row(row, false);
        ui.cell(&format!("row {row}"));
        ui.cell("1 KB");
        ui.close();
    }
    ui.close();
    ui.close();
    ui.close();
}

#[cfg(target_os = "ios")]
mod phone {
    use super::{PARAGRAPH, Parts, Reading, parts, reading};
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char, c_void};
    use std::io::Write;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use taut::{Role, Ui, Window};

    type Obj = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Rect {
        origin: Point,
        size: Point,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Insets {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Range {
        location: usize,
        length: usize,
    }

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Obj;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
        fn objc_allocateClassPair(superclass: Obj, name: *const c_char, extra: usize) -> Obj;
        fn objc_registerClassPair(class: Obj);
        fn class_addMethod(class: Obj, name: *const c_void, imp: *const c_void, types: *const c_char) -> u8;
    }

    fn class(name: &CStr) -> Obj {
        unsafe { objc_getClass(name.as_ptr()) }
    }
    fn sel(name: &CStr) -> *const c_void {
        unsafe { sel_registerName(name.as_ptr()) }
    }
    /// `objc_msgSend` as a method of no arguments, one, two.
    unsafe fn send0<R>(to: Obj, name: &CStr) -> R {
        let call: unsafe extern "C" fn(Obj, *const c_void) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { call(to, sel(name)) }
    }
    unsafe fn send1<R, A>(to: Obj, name: &CStr, a: A) -> R {
        let call: unsafe extern "C" fn(Obj, *const c_void, A) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { call(to, sel(name), a) }
    }
    unsafe fn send2<R, A, B>(to: Obj, name: &CStr, a: A, b: B) -> R {
        let call: unsafe extern "C" fn(Obj, *const c_void, A, B) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { call(to, sel(name), a, b) }
    }
    fn string(text: &str) -> Obj {
        let text = std::ffi::CString::new(text).unwrap();
        unsafe { send1(class(c"NSString"), c"stringWithUTF8String:", text.as_ptr()) }
    }
    fn text_of(string: Obj) -> String {
        if string.is_null() {
            return String::new();
        }
        let bytes: *const c_char = unsafe { send0(string, c"UTF8String") };
        unsafe { CStr::from_ptr(bytes) }.to_string_lossy().into_owned()
    }
    fn items(array: Obj) -> Vec<Obj> {
        let count: usize = unsafe { send0(array, c"count") };
        (0..count).map(|at| unsafe { send1(array, c"objectAtIndex:", at) }).collect()
    }

    // ---- stand-ins for what UIKit makes: a finger's touch, a hardware key's press ----

    thread_local! {
        /// Where each stand-in touch is, in the view, and its type (0 a finger, 2 a pencil, 3 a
        /// pointer); each stand-in key's code, flags, letters.
        static TOUCHES: RefCell<Vec<(usize, Point, isize)>> = const { RefCell::new(Vec::new()) };
        static KEYS: RefCell<Vec<(usize, isize, isize, &'static str)>> = const { RefCell::new(Vec::new()) };
    }

    extern "C" fn touch_place(touch: Obj, _: *const c_void, _: Obj) -> Point {
        TOUCHES.with_borrow(|touches| touches.iter().find(|(of, ..)| *of == touch as usize).map(|(_, at, _)| *at).unwrap_or_default())
    }
    extern "C" fn touch_type(touch: Obj, _: *const c_void) -> isize {
        TOUCHES.with_borrow(|touches| touches.iter().find(|(of, ..)| *of == touch as usize).map_or(0, |(.., kind)| *kind))
    }
    extern "C" fn key_code(key: Obj, _: *const c_void) -> isize {
        KEYS.with_borrow(|keys| keys.iter().find(|(of, ..)| *of == key as usize).map_or(0, |(_, code, ..)| *code))
    }
    extern "C" fn key_flags(key: Obj, _: *const c_void) -> isize {
        KEYS.with_borrow(|keys| keys.iter().find(|(of, ..)| *of == key as usize).map_or(0, |(_, _, flags, _)| *flags))
    }
    extern "C" fn key_letters(key: Obj, _: *const c_void) -> Obj {
        string(KEYS.with_borrow(|keys| keys.iter().find(|(of, ..)| *of == key as usize).map_or("", |(.., letters)| letters)))
    }
    extern "C" fn press_key(press: Obj, _: *const c_void) -> Obj {
        // The press's key is kept as the press's own entry's twin: the key made with it.
        KEYS.with_borrow(|keys| keys.iter().find(|(of, ..)| *of == press as usize + 1).map_or(std::ptr::null_mut(), |(_, code, ..)| *code as Obj))
    }

    /// A subclass of a UIKit class with these methods, made once.
    fn stand_in(name: &CStr, of: &CStr, methods: &[(&CStr, *const c_void, &CStr)]) -> Obj {
        let known = class(name);
        if !known.is_null() {
            return known;
        }
        unsafe {
            let made = objc_allocateClassPair(class(of), name.as_ptr(), 0);
            for (method, imp, types) in methods {
                class_addMethod(made, sel(method), *imp, types.as_ptr());
            }
            objc_registerClassPair(made);
            made
        }
    }

    /// A touch of a type at a place in the view.
    fn touch_at(at: Point, kind: isize) -> Obj {
        let class = stand_in(c"CheckTouch", c"UITouch", &[(c"locationInView:", touch_place as *const c_void, c"{CGPoint=dd}@:@"), (c"type", touch_type as *const c_void, c"q@:")]);
        let touch: Obj = unsafe { send0(send0::<Obj>(class, c"alloc"), c"init") };
        TOUCHES.with_borrow_mut(|touches| touches.push((touch as usize, at, kind)));
        touch
    }

    /// A hardware key's press: its HID usage, modifier flags and letters.
    fn press_of(code: isize, flags: isize, letters: &'static str) -> Obj {
        let key_class = stand_in(
            c"CheckKey",
            c"UIKey",
            &[(c"keyCode", key_code as *const c_void, c"q@:"), (c"modifierFlags", key_flags as *const c_void, c"q@:"), (c"charactersIgnoringModifiers", key_letters as *const c_void, c"@@:")],
        );
        let press_class = stand_in(c"CheckPress", c"UIPress", &[(c"key", press_key as *const c_void, c"@@:")]);
        let key: Obj = unsafe { send0(key_class, c"alloc") };
        let press: Obj = unsafe { send0(send0::<Obj>(press_class, c"alloc"), c"init") };
        KEYS.with_borrow_mut(|keys| {
            keys.push((key as usize, code, flags, letters));
            keys.push((press as usize + 1, key as isize, 0, ""));
        });
        press
    }

    fn set_of(object: Obj) -> Obj {
        unsafe { send1(class(c"NSSet"), c"setWithObject:", object) }
    }

    // ---- the app's views ----

    /// The library's view: the root view of the first scene's window.
    fn view() -> Obj {
        unsafe {
            let app: Obj = send0(class(c"UIApplication"), c"sharedApplication");
            let scene: Obj = send0(send0::<Obj>(app, c"connectedScenes"), c"anyObject");
            let window = items(send0(scene, c"windows")).first().copied().unwrap_or(std::ptr::null_mut());
            send0(send0::<Obj>(window, c"rootViewController"), c"view")
        }
    }
    /// A subview of the library's view, by class name.
    fn subview(name: &CStr) -> Obj {
        let views = items(unsafe { send0(view(), c"subviews") });
        views.into_iter().find(|one| unsafe { send1::<u8, _>(*one, c"isKindOfClass:", class(name)) } != 0).unwrap_or(std::ptr::null_mut())
    }
    fn input() -> Obj {
        subview(c"TautTextInput")
    }
    /// Where the page lies in the view: its top left.
    fn origin() -> Point {
        let insets: Insets = unsafe { send0(view(), c"safeAreaInsets") };
        Point { x: insets.left, y: insets.top }
    }

    // ---- notes for the Mac ----

    fn folder() -> PathBuf {
        let folder = std::env::temp_dir().join("taut-check");
        _ = std::fs::create_dir_all(&folder);
        folder
    }
    /// A line added to a note.
    fn note(file: &str, line: &str) {
        eprintln!("{file}: {line}");
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(folder().join(file)).unwrap();
        writeln!(file, "{line}").unwrap();
    }

    /// What the reader is told, read back through UIKit's accessibility methods: each element's
    /// label, value, traits and frame.
    fn elements() -> Vec<(String, String, u64, Rect)> {
        let elements: Obj = unsafe { send0(view(), c"accessibilityElements") };
        let read = |element: Obj| unsafe {
            (
                text_of(send0(element, c"accessibilityLabel")),
                text_of(send0(element, c"accessibilityValue")),
                send0::<u64>(element, c"accessibilityTraits"),
                send0::<Rect>(element, c"accessibilityFrame"),
            )
        };
        items(elements).into_iter().map(read).collect()
    }
    fn element(label: &str) -> Obj {
        let elements: Obj = unsafe { send0(view(), c"accessibilityElements") };
        items(elements).into_iter().find(|element| text_of(unsafe { send0(*element, c"accessibilityLabel") }) == label).unwrap_or(std::ptr::null_mut())
    }

    pub fn main() {
        let mode = std::env::var("TAUT_IOS_CHECK").unwrap_or_default();
        _ = std::fs::remove_dir_all(folder());
        // A turn a tenth of a second, for the checks to go on with nothing happening.
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                taut::platform::Waker.wake();
            }
        });
        let window = Window { title: "taut check", ..Default::default() };
        if mode == "reader" {
            let state = Rc::new(RefCell::new((Reading { notes: "héllo wörld".into(), table: taut::Table::default() }, ReaderScript::default())));
            taut::run(window, move |ui| {
                let (page, script) = &mut *state.borrow_mut();
                reading(page, ui);
                script.step(ui);
            });
        } else if mode == "parts" {
            let state = Rc::new(RefCell::new((Parts::default(), Script::default())));
            taut::run(window, move |ui| {
                let (parts_now, script) = &mut *state.borrow_mut();
                parts(parts_now, ui);
                script.step(parts_now, ui);
            });
        } else {
            let mut app = super::gallery::app();
            let mut shown = Shown::default();
            taut::run(window, move |ui| {
                super::gallery::describe(&mut app, ui);
                shown.step(ui);
            });
        }
    }

    /// The gallery: once settled, its size for the Mac to draw it at; once the Mac has the
    /// screen, what it tells readers.
    #[derive(Default)]
    struct Shown {
        start: Option<Instant>,
        stage: u32,
        dark: Option<bool>,
        away: Option<bool>,
    }

    impl Shown {
        fn step(&mut self, ui: &mut Ui) {
            let since = self.start.get_or_insert_with(Instant::now).elapsed();
            let at = origin();
            match self.stage {
                0 if since > Duration::from_secs(3) => {
                    let size = ui.size();
                    drawn_here(ui);
                    note("ready.txt", &format!("{} {} {} {} {} {} {}", size.w, size.h, ui.scale(), at.x, at.y, ui.system_dark() as u8, ui.text_scale()));
                    self.stage = 1;
                }
                1 if folder().join("shot").exists() => {
                    _ = elements();
                    self.stage = 2;
                }
                2 if !ui.visible_access_nodes().is_empty() => self.stage = 3,
                3 => {
                    for (label, value, traits, frame) in elements() {
                        note("elements.txt", &format!("{label:?} {value:?} {traits:#x} {:.0},{:.0} {:.0}x{:.0}", frame.origin.x, frame.origin.y, frame.size.x, frame.size.y));
                    }
                    note("elements.txt", "done");
                    self.stage = 4;
                }
                _ => {}
            }
            self.looks(ui);
        }

        /// The system's dark mode, and going to the background and back, noted as heard.
        fn looks(&mut self, ui: &mut Ui) {
            if self.stage < 4 {
                return;
            }
            if self.dark.replace(ui.system_dark()) != Some(ui.system_dark()) {
                note("looks.txt", &format!("dark {}", ui.system_dark()));
            }
            if self.away.replace(ui.window_minimized()) != Some(ui.window_minimized()) {
                note("looks.txt", &format!("away {}", ui.window_minimized()));
            }
        }
    }

    /// The gallery drawn afresh here, at the window's size, scale and looks, as the Mac draws it:
    /// the pixels the screen should show.
    fn drawn_here(shown: &Ui) {
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(taut::LocalClipboard::default()));
        ui.resize(shown.size(), shown.scale());
        let looks = taut::Preferences { dark: shown.system_dark(), text_scale: shown.text_scale(), ..taut::Preferences::default() };
        ui.set_preferences(looks);
        let mut app = super::gallery::app();
        let mut surface = taut::Surface::default();
        for run in 0..4 {
            ui.set_time(Duration::from_millis(run * 500));
            ui.run(&mut |ui| super::gallery::describe(&mut app, ui));
            surface.update(&mut ui);
            ui.changed();
        }
        let solid: Vec<u32> = surface.pixels.iter().map(|pixel| pixel | 0xff00_0000).collect();
        _ = std::fs::write(folder().join("drawn.png"), taut::png::encode_rgba(surface.width, &solid).unwrap_or_default());
    }

    /// The parts page, driven: each step acts, then waits (at most a few seconds) for the
    /// interface to show what it should.
    #[derive(Default)]
    struct Script {
        step: usize,
        since: Option<Instant>,
        acted: bool,
        held: Obj,
    }

    impl Script {
        fn step(&mut self, parts: &mut Parts, ui: &mut Ui) {
            let since = *self.since.get_or_insert_with(Instant::now);
            if since.elapsed() < Duration::from_millis(500) && self.step == 0 {
                return;
            }
            let name = STEPS.get(self.step).copied().unwrap_or("");
            if name.is_empty() {
                return;
            }
            if !self.acted {
                self.acted = true;
                self.act(name, ui);
                return;
            }
            let passed = self.check(name, parts, ui);
            match passed {
                Some(true) => note("parts.txt", &format!("ok {name}")),
                None if since.elapsed() < Duration::from_secs(8) => return,
                _ => note("parts.txt", &format!("FAILED {name}: {parts:?}")),
            }
            (self.step, self.since, self.acted) = (self.step + 1, Some(Instant::now()), false);
            if STEPS[self.step].is_empty() {
                note("parts.txt", "done");
            }
        }

        /// The middle of a part, in the view.
        fn middle_of(ui: &Ui, role: Role) -> Point {
            let node = ui.visible_access_nodes().iter().find(|node| node.role == role).map(|node| node.rect).unwrap_or_default();
            let at = origin();
            Point { x: at.x + f64::from(node.x + node.w / 2.0), y: at.y + f64::from(node.y + node.h / 2.0) }
        }

        /// A finger, pencil or pointer lands, then lifts (a run later).
        fn tap(&mut self, at: Point, kind: isize) {
            let touch = touch_at(at, kind);
            unsafe { send2::<(), _, _>(view(), c"touchesBegan:withEvent:", set_of(touch), std::ptr::null_mut::<c_void>()) };
            self.held = touch;
        }

        fn key(code: isize, flags: isize, letters: &'static str) {
            let press = press_of(code, flags, letters);
            let target = if unsafe { send0::<u8>(input(), c"isFirstResponder") } != 0 { input() } else { view() };
            unsafe {
                send2::<(), _, _>(target, c"pressesBegan:withEvent:", set_of(press), std::ptr::null_mut::<c_void>());
                send2::<(), _, _>(target, c"pressesEnded:withEvent:", set_of(press), std::ptr::null_mut::<c_void>());
            }
        }

        fn act(&mut self, name: &str, ui: &mut Ui) {
            let input = input();
            unsafe {
                match name {
                    "reader asked" => _ = elements(),
                    "finger taps the button" => self.tap(Self::middle_of(ui, Role::Button), 0),
                    "pointer clicks the button" => self.tap(Self::middle_of(ui, Role::Button), 3),
                    "pencil taps the button" => self.tap(Self::middle_of(ui, Role::Button), 2),
                    "finger taps the text box" => self.tap(Self::middle_of(ui, Role::TextBox), 0),
                    "typed text" => send1::<(), _>(input, c"insertText:", string("h\u{e9}llo")),
                    "composed text" => {
                        send2::<(), _, _>(input, c"setMarkedText:selectedRange:", string("\u{304b}"), Range { location: 1, length: 0 });
                        send0::<()>(input, c"unmarkText");
                    }
                    "keyboard's delete" => send0::<()>(input, c"deleteBackward"),
                    "hardware backspace" => Self::key(0x2a, 0, ""),
                    "select all and copy" => {
                        send1::<(), _>(input, c"selectAll:", std::ptr::null_mut::<c_void>());
                        send1::<(), _>(input, c"copy:", std::ptr::null_mut::<c_void>());
                    }
                    "command-a and backspace" => {
                        Self::key(0x04, 1 << 20, "a");
                        Self::key(0x2a, 0, "");
                    }
                    "paste" => send1::<(), _>(input, c"paste:", std::ptr::null_mut::<c_void>()),
                    "reader ticks" => _ = send0::<u8>(element("Tick"), c"accessibilityActivate"),
                    "reader steps the slider" => {
                        let slider = elements().iter().position(|(.., traits, _)| traits & 0x1000 != 0);
                        let elements: Obj = send0(view(), c"accessibilityElements");
                        let slider = slider.map(|at| items(elements)[at]).unwrap_or(std::ptr::null_mut());
                        send0::<()>(slider, c"accessibilityIncrement");
                    }
                    "turned sideways" => turn(true),
                    "turned upright" => turn(false),
                    _ => {}
                }
            }
        }

        /// Whether the step's outcome shows: yes, no, or not yet.
        fn check(&mut self, name: &str, parts: &Parts, ui: &Ui) -> Option<bool> {
            if !self.held.is_null() {
                let touch = std::mem::replace(&mut self.held, std::ptr::null_mut());
                unsafe { send2::<(), _, _>(view(), c"touchesEnded:withEvent:", set_of(touch), std::ptr::null_mut::<c_void>()) };
                return None;
            }
            let pasted = || text_of(unsafe { send0(send0::<Obj>(class(c"UIPasteboard"), c"generalPasteboard"), c"string") });
            let typing = || unsafe { send0::<u8>(input(), c"isFirstResponder") } != 0;
            let tick = || elements().iter().find(|(label, ..)| label == "Tick").map(|(_, _, traits, _)| *traits);
            let size = ui.size();
            let shows = match name {
                "reader asked" => !ui.visible_access_nodes().is_empty() && !elements().is_empty(),
                "finger taps the button" => parts.presses == 1,
                "pointer clicks the button" => parts.presses == 2,
                "pencil taps the button" => parts.presses == 3,
                "finger taps the text box" => typing() && ui.ime_area().is_some(),
                "typed text" => parts.text == "h\u{e9}llo",
                "composed text" => parts.text == "h\u{e9}llo\u{304b}",
                "keyboard's delete" => parts.text == "h\u{e9}llo",
                "hardware backspace" => parts.text == "h\u{e9}ll",
                "select all and copy" => pasted() == "h\u{e9}ll",
                "command-a and backspace" => parts.text.is_empty(),
                "paste" => parts.text == "h\u{e9}ll",
                // Button (1) and selected (8).
                "reader ticks" => parts.tick && tick().is_some_and(|traits| traits & 0x9 == 0x9),
                "reader steps the slider" => parts.amount > 0.0,
                "turned sideways" => size.w > size.h,
                "turned upright" => size.w < size.h,
                _ => true,
            };
            shows.then_some(true)
        }
    }

    const STEPS: [&str; 17] = [
        "reader asked",
        "finger taps the button",
        "pointer clicks the button",
        "pencil taps the button",
        "finger taps the text box",
        "typed text",
        "composed text",
        "keyboard's delete",
        "hardware backspace",
        "select all and copy",
        "command-a and backspace",
        "paste",
        "reader ticks",
        "reader steps the slider",
        "turned sideways",
        "turned upright",
        "",
    ];

    /// Asks the scene to turn sideways (landscape) or upright (portrait), as a person turning
    /// the phone does.
    fn turn(sideways: bool) {
        let orientations: usize = if sideways { 1 << 3 | 1 << 4 } else { 1 << 1 };
        unsafe {
            let app: Obj = send0(class(c"UIApplication"), c"sharedApplication");
            let scene: Obj = send0(send0::<Obj>(app, c"connectedScenes"), c"anyObject");
            let wanted: Obj = send1(send0::<Obj>(class(c"UIWindowSceneGeometryPreferencesIOS"), c"alloc"), c"initWithInterfaceOrientations:", orientations);
            send2::<(), _, _>(scene, c"requestGeometryUpdateWithPreferences:errorHandler:", wanted, std::ptr::null_mut::<c_void>());
        }
    }

    // ---- the reader page, read through the methods UIKit calls of its elements: lines
    // (UIAccessibilityReadingContent), letters and selection (each element's
    // accessibilityTextInputResponder, a UITextInput), and the table as a data table ----

    unsafe extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }

    thread_local! {
        /// What the reader was given asking for row 400's first cell between turns: its label
        /// and the row it says it is in.
        static ROW_ASKED: RefCell<Option<(String, Range)>> = const { RefCell::new(None) };
    }

    /// The table: the top element that is a data table (container type 1).
    fn table() -> Obj {
        let elements: Obj = unsafe { send0(view(), c"accessibilityElements") };
        items(elements).into_iter().find(|element| unsafe { send0::<isize>(*element, c"accessibilityContainerType") } == 1).unwrap_or(std::ptr::null_mut())
    }
    /// An element's lines, as the reader reads them one by one.
    fn lines_of(element: Obj) -> Vec<String> {
        let line = |nth: isize| unsafe { send1::<Obj, _>(element, c"accessibilityContentForLineNumber:", nth) };
        (0..).map(line).take_while(|line| !line.is_null()).map(text_of).collect()
    }
    /// The middle of a rectangle.
    fn middle(rect: Rect) -> Point {
        Point { x: rect.origin.x + rect.size.x / 2.0, y: rect.origin.y + rect.size.y / 2.0 }
    }
    /// A range of a reader's text, from one UTF-16 unit to another.
    fn units(reader: Obj, from: isize, to: isize) -> Obj {
        unsafe {
            let start: Obj = send0(reader, c"beginningOfDocument");
            let [from, to]: [Obj; 2] = [from, to].map(|unit| send2(reader, c"positionFromPosition:offset:", start, unit));
            send2(reader, c"textRangeFromPosition:toPosition:", from, to)
        }
    }
    /// Where a range of a reader's text starts and ends, in UTF-16 units.
    fn ends(reader: Obj, range: Obj) -> [isize; 2] {
        unsafe {
            let start: Obj = send0(reader, c"beginningOfDocument");
            [c"start", c"end"].map(|end| send2(reader, c"offsetFromPosition:toPosition:", start, send0::<Obj>(range, end)))
        }
    }
    /// A cell of the table, through the data table's own method.
    fn cell(row: usize, column: usize) -> Obj {
        unsafe { send2(table(), c"accessibilityDataTableCellElementForRow:column:", row, column) }
    }
    /// Asked between the window's turns, as VoiceOver asks: row 400's first cell.
    extern "C" fn ask_for_row_400(_: *mut c_void) {
        let cell = cell(400, 0);
        let said = unsafe { (text_of(send0(cell, c"accessibilityLabel")), send0::<Range>(cell, c"accessibilityRowRange")) };
        ROW_ASKED.with_borrow_mut(|asked| *asked = Some(said));
    }

    /// The reader page, driven a step at a time as the parts page is.
    #[derive(Default)]
    struct ReaderScript {
        step: usize,
        since: Option<Instant>,
        acted: bool,
    }

    impl ReaderScript {
        fn step(&mut self, ui: &mut Ui) {
            let since = *self.since.get_or_insert_with(Instant::now);
            let Some(name) = READER_STEPS.get(self.step).copied().filter(|name| !name.is_empty()) else { return };
            if since.elapsed() < Duration::from_millis(500) {
                return;
            }
            if !self.acted {
                self.acted = true;
                return Self::act(name);
            }
            match Self::check(name, ui) {
                Ok(()) => note("reader.txt", &format!("ok {name}")),
                Err(_) if since.elapsed() < Duration::from_secs(8) => return,
                Err(why) => note("reader.txt", &format!("FAILED {name}: {why}")),
            }
            (self.step, self.since, self.acted) = (self.step + 1, Some(Instant::now()), false);
            if READER_STEPS[self.step].is_empty() {
                note("reader.txt", "done");
            }
        }

        fn act(name: &str) {
            match name {
                "reader asked" => _ = elements(),
                "a reader selects in the text box" => {
                    let reader: Obj = unsafe { send0(element("Notes"), c"accessibilityTextInputResponder") };
                    unsafe { send1::<(), _>(reader, c"setSelectedTextRange:", units(reader, 1, 4)) };
                }
                "a row out of view comes at once" => unsafe { dispatch_async_f((&raw const _dispatch_main_q).cast(), std::ptr::null_mut(), ask_for_row_400) },
                "a three-finger swipe pages the table on" => _ = unsafe { send1::<u8, _>(table(), c"accessibilityScroll:", 3isize) },
                _ => {}
            }
        }

        /// Whether the step's outcome shows: if not, what does.
        fn check(name: &str, ui: &Ui) -> Result<(), String> {
            let truth = |ok: bool, said: String| if ok { Ok(()) } else { Err(said) };
            match name {
                "reader asked" => truth(!elements().is_empty() && !ui.visible_access_nodes().is_empty(), "no elements".into()),
                "lines as wrapped" => Self::lines(),
                "letters where drawn" => Self::letters(),
                "a reader selects in the text box" => {
                    let caret = ui.access_nodes().iter().find(|node| node.role == Role::TextBox).and_then(|node| node.caret);
                    let reader: Obj = unsafe { send0(element("Notes"), c"accessibilityTextInputResponder") };
                    let told = ends(reader, unsafe { send0(reader, c"selectedTextRange") });
                    truth(caret == Some([1, 4]) && told == [1, 4], format!("the box's selection {caret:?}, the reader's {told:?}"))
                }
                "a data table" => Self::data_table(),
                "a row out of view comes at once" => {
                    let asked = ROW_ASKED.with_borrow(Clone::clone);
                    truth(matches!(&asked, Some((label, Range { location: 400, length: 1 })) if label == "row 400"), format!("given {asked:?}"))
                }
                "a three-finger swipe pages the table on" => {
                    let first = ui.visible_access_nodes().iter().find(|node| node.role == Role::Cell).map(|node| node.name.clone());
                    truth(first.as_deref().is_some_and(|first| first != "row 0" && first != "row 400"), format!("the first cell in view is {first:?}"))
                }
                _ => Ok(()),
            }
        }

        /// The paragraph by lines: its lines make its text, each lower than the one before, and
        /// a point on a line is on that line.
        fn lines() -> Result<(), String> {
            let paragraph = element(PARAGRAPH);
            let page = text_of(unsafe { send0(paragraph, c"accessibilityPageContent") });
            let lines = lines_of(paragraph);
            if page != PARAGRAPH || lines.len() < 2 || lines.concat() != PARAGRAPH {
                return Err(format!("page {page:?}, lines {lines:?}"));
            }
            let frame = |nth: isize| unsafe { send1::<Rect, _>(paragraph, c"accessibilityFrameForLineNumber:", nth) };
            let (first, second) = (frame(0), frame(1));
            let found = unsafe { send1::<isize, _>(paragraph, c"accessibilityLineNumberForPoint:", middle(second)) };
            let ok = second.origin.y >= first.origin.y + first.size.y - 1.0 && found == 1;
            if ok { Ok(()) } else { Err(format!("lines at {first:?} and {second:?}; the second's middle is on line {found}")) }
        }

        /// The paragraph by letters: a range's text and place, the cursor's place, the letter at a
        /// point, a rectangle per line, and the system's tokenizer over it.
        fn letters() -> Result<(), String> {
            let reader: Obj = unsafe { send0(element(PARAGRAPH), c"accessibilityTextInputResponder") };
            if reader.is_null() {
                return Err("no reader".into());
            }
            let word = units(reader, 2, 11);
            let (said, place) = unsafe { (text_of(send1(reader, c"textInRange:", word)), send1::<Rect, _>(reader, c"firstRectForRange:", word)) };
            let caret = unsafe { send1::<Rect, _>(reader, c"caretRectForPosition:", send0::<Obj>(word, c"start")) };
            let at = ends(reader, unsafe { send1(reader, c"characterRangeAtPoint:", middle(place)) });
            let all = units(reader, 0, PARAGRAPH.len() as isize);
            let rects = items(unsafe { send1(reader, c"selectionRectsForRange:", all) }).len();
            let tokenizer: Obj = unsafe { send0(reader, c"tokenizer") };
            let lines = lines_of(element(PARAGRAPH)).len();
            let ok = said == "paragraph" && (caret.origin.x - place.origin.x).abs() < 0.5 && (2..11).contains(&at[0]) && rects == lines && !tokenizer.is_null();
            if ok { Ok(()) } else { Err(format!("{said:?} at {place:?}, the cursor at {caret:?}, the letter at its middle {at:?}, {rects} rectangles for {lines} lines")) }
        }

        /// The table as a data table: all its rows, its columns, a cell's row, a column's heading.
        fn data_table() -> Result<(), String> {
            let table = table();
            let (rows, columns) = unsafe { (send0::<usize>(table, c"accessibilityRowCount"), send0::<usize>(table, c"accessibilityColumnCount")) };
            let first = cell(0, 0);
            let (label, row) = unsafe { (text_of(send0(first, c"accessibilityLabel")), send0::<Range>(first, c"accessibilityRowRange")) };
            let headings = items(unsafe { send1(table, c"accessibilityHeaderElementsForColumn:", 1usize) });
            let heading = headings.first().map(|heading| text_of(unsafe { send0(*heading, c"accessibilityLabel") }));
            let ok = (rows, columns) == (500, 2) && label == "row 0" && (row.location, row.length) == (0, 1) && heading.as_deref() == Some("Size");
            if ok { Ok(()) } else { Err(format!("{rows} rows, {columns} columns; the first cell {label:?} in {row:?}; heading {heading:?}")) }
        }
    }

    const READER_STEPS: [&str; 8] = [
        "reader asked",
        "lines as wrapped",
        "letters where drawn",
        "a reader selects in the text box",
        "a data table",
        "a row out of view comes at once",
        "a three-finger swipe pages the table on",
        "",
    ];
}

#[cfg(target_os = "macos")]
mod mac {
    use super::ios_bundle;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};
    use taut::geom::Size;
    use taut::{LocalClipboard, Preferences, Surface, Ui};

    const CHECK_ID: &str = "dev.taut.check";

    pub fn main() {
        if std::env::var_os("TAUT_IOS_SIMULATOR").is_none() {
            return println!("The iOS checks run with TAUT_IOS_SIMULATOR=1 (they boot a simulator).");
        }
        let failed = match checks() {
            Ok(failed) => failed,
            Err(error) => vec![error],
        };
        if !failed.is_empty() {
            eprintln!("FAILED:\n{}", failed.join("\n"));
            std::process::exit(1);
        }
        println!("all iOS checks passed");
    }

    /// Where the bundles are built: a folder of their own, so this test's own build is not
    /// waited for.
    fn built() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("ios-check")
    }

    fn shots() -> Option<PathBuf> {
        let folder = PathBuf::from(std::env::var_os("TAUT_SHOTS")?);
        std::fs::create_dir_all(&folder).ok()?;
        Some(folder)
    }

    /// This file built for the simulator, as an app.
    fn check_app() -> Result<PathBuf, String> {
        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.args(["build", "--test", "ios", "--features", "touch", "--target", "aarch64-apple-ios-sim", "--message-format=json"]).env("CARGO_TARGET_DIR", built());
        let said = ios_bundle::ran(&mut cargo)?;
        let program = said.lines().filter(|line| line.contains("\"executable\":\"") && line.contains("\"name\":\"ios\"")).find_map(|line| line.split("\"executable\":\"").nth(1)?.split('"').next());
        let program = PathBuf::from(program.ok_or("no test program built")?);
        let app = ios_bundle::bundle(&program, "check", CHECK_ID, false, &built().join("ios")).map_err(|error| error.to_string())?;
        ios_bundle::sign_for_simulator(&app)?;
        Ok(app)
    }

    fn simctl(args: &[&str]) -> Result<String, String> {
        ios_bundle::ran(Command::new("xcrun").arg("simctl").args(args))
    }

    /// Starts an app on the device in a mode, its output in files.
    fn launch(device: &str, id: &str, mode: &str) -> Result<(), String> {
        let logs = built().join(format!("{id}-{mode}"));
        let (out, err) = (format!("--stdout={}.out", logs.display()), format!("--stderr={}.err", logs.display()));
        let mut command = Command::new("xcrun");
        command.args(["simctl", "launch", "--terminate-running-process", &out, &err, device, id]).env("SIMCTL_CHILD_TAUT_IOS_CHECK", mode);
        ios_bundle::ran(&mut command).map(drop)
    }

    /// A note the app wrote, once it holds `until` (or the wait is over).
    fn note(device: &str, id: &str, file: &str, until: impl Fn(&str) -> bool, wait: Duration) -> Result<String, String> {
        let since = Instant::now();
        let container = simctl(&["get_app_container", device, id, "data"])?;
        let path = Path::new(container.trim()).join("tmp").join("taut-check").join(file);
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if until(&text) {
                return Ok(text);
            }
            if since.elapsed() > wait {
                return Err(format!("{file} after {wait:?}: {text:?}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn signal(device: &str, id: &str, file: &str) -> Result<(), String> {
        let container = simctl(&["get_app_container", device, id, "data"])?;
        std::fs::write(Path::new(container.trim()).join("tmp").join("taut-check").join(file), "").map_err(|error| error.to_string())
    }

    /// The screen, as pixels.
    fn screen(device: &str, name: &str) -> Result<(usize, Vec<u32>), String> {
        let file = built().join(format!("{name}.png"));
        simctl(&["io", device, "screenshot", &file.to_string_lossy()])?;
        if let Some(folder) = shots() {
            _ = std::fs::copy(&file, folder.join(format!("{name}.png")));
        }
        taut::png::decode(&std::fs::read(&file).map_err(|error| error.to_string())?).ok_or_else(|| "the screenshot is no PNG".into())
    }

    /// What the app said of its window: size and scale, where the page lies, dark or light,
    /// the text scale.
    struct Page {
        size: Size,
        scale: f32,
        at: [f32; 2],
        preferences: Preferences,
    }

    fn page_of(ready: &str) -> Result<Page, String> {
        let numbers: Vec<f32> = ready.split_whitespace().filter_map(|word| word.parse().ok()).collect();
        let [w, h, scale, x, y, dark, text_scale] = numbers[..] else { return Err(format!("ready.txt: {ready:?}")) };
        let preferences = Preferences { dark: dark != 0.0, text_scale, ..Preferences::default() };
        Ok(Page { size: Size { w, h }, scale, at: [x, y], preferences })
    }

    /// The gallery drawn on the Mac, at the page's size and scale, with CoreText.
    fn drawn_here(page: &Page) -> Surface {
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
        ui.resize(page.size, page.scale);
        ui.set_preferences(page.preferences);
        let mut app = super::gallery::app();
        let mut surface = Surface::default();
        for run in 0..4 {
            ui.set_time(Duration::from_millis(run * 500));
            ui.run(&mut |ui| super::gallery::describe(&mut app, ui));
            surface.update(&mut ui);
            ui.changed();
        }
        surface
    }

    /// How much of the screen's page differs from the Mac's drawing: the share of pixels more
    /// than 32 levels apart in some channel, and the largest difference.
    fn compared(screen: &(usize, Vec<u32>), page: &Page, here: &Surface, name: &str) -> (f64, u32) {
        let (width, pixels) = screen;
        let (left, top) = ((page.at[0] * page.scale).round() as usize, (page.at[1] * page.scale).round() as usize);
        let (mut apart, mut most, mut diff) = (0usize, 0u32, vec![0u32; here.width * here.height]);
        for y in 0..here.height {
            for x in 0..here.width {
                let theirs = pixels.get((top + y) * width + left + x).copied().unwrap_or(0);
                let ours = here.pixels[y * here.width + x];
                let worst = (0..3).map(|channel| ((theirs >> (channel * 8)) & 0xff).abs_diff((ours >> (channel * 8)) & 0xff)).max().unwrap_or(0);
                most = most.max(worst);
                apart += usize::from(worst > 32);
                diff[y * here.width + x] = if worst > 32 { 0xffff_0000 } else { 0xff00_0000 | (((ours & 0x00ff_ffff) / 2) & 0x007f_7f7f) };
            }
        }
        if let Some(folder) = shots() {
            let solid: Vec<u32> = here.pixels.iter().map(|pixel| pixel | 0xff00_0000).collect();
            _ = std::fs::write(folder.join(format!("{name}-mac.png")), taut::png::encode_rgba(here.width, &solid).unwrap_or_default());
            _ = std::fs::write(folder.join(format!("{name}-diff.png")), taut::png::encode_rgba(here.width, &diff).unwrap_or_default());
        }
        (apart as f64 / (here.width * here.height).max(1) as f64, most)
    }

    /// Everything, in order; the failures.
    fn checks() -> Result<Vec<String>, String> {
        let mut failed = Vec::new();
        let check = check_app()?;
        let asked = ios_bundle::Asked { example: "gallery".into(), id: "dev.taut.gallery".into(), ..Default::default() };
        let gallery = ios_bundle::build(&asked, &built())?;
        let device = ios_bundle::newest_iphone()?;
        println!("simulator {device}");
        ios_bundle::boot(&device)?;
        simctl(&["ui", &device, "appearance", "light"])?;
        simctl(&["install", &device, &check.to_string_lossy()])?;
        simctl(&["install", &device, &gallery.to_string_lossy()])?;

        // The gallery in the check app: the screen against the Mac's drawing; what readers hear.
        launch(&device, CHECK_ID, "gallery")?;
        let ready = note(&device, CHECK_ID, "ready.txt", |text| text.ends_with('\n'), Duration::from_secs(120))?;
        let page = page_of(&ready)?;
        println!("page {}x{} at {:?}, scale {}", page.size.w, page.size.h, page.at, page.scale);
        let here = drawn_here(&page);
        let shown = screen(&device, "check-gallery")?;
        let (share, most) = compared(&shown, &page, &here, "check-gallery");
        println!("the screen against the Mac's drawing: {:.2}% of pixels apart, at most {most}", share * 100.0);
        if share > 0.03 {
            failed.push(format!("the screen differs from the Mac's drawing in {:.2}% of its pixels", share * 100.0));
        }
        // The same against the app's own drawing, made on the phone: the screen shows its pixels
        // as they are, where they belong.
        let container = simctl(&["get_app_container", &device, CHECK_ID, "data"])?;
        let drawn = std::fs::read(Path::new(container.trim()).join("tmp/taut-check/drawn.png")).unwrap_or_default();
        let (width, pixels) = taut::png::decode(&drawn).ok_or("the app drew nothing")?;
        let phone = Surface { width, height: pixels.len() / width.max(1), pixels };
        let (share, most) = compared(&shown, &page, &phone, "check-gallery-phone");
        println!("the screen against the phone's own drawing: {:.3}% of pixels apart, at most {most}", share * 100.0);
        if share > 0.001 {
            failed.push(format!("the screen differs from the phone's own drawing in {:.3}% of its pixels", share * 100.0));
        }
        signal(&device, CHECK_ID, "shot")?;
        let told = note(&device, CHECK_ID, "elements.txt", |text| text.contains("done"), Duration::from_secs(60))?;
        let count = told.lines().count() - 1;
        println!("{count} elements told to readers; the first:\n{}", told.lines().take(25).collect::<Vec<_>>().join("\n"));
        if count < 10 {
            failed.push(format!("only {count} elements told to readers"));
        }

        // Dark mode, and the background and back.
        simctl(&["ui", &device, "appearance", "dark"])?;
        let dark = note(&device, CHECK_ID, "looks.txt", |text| text.contains("dark true"), Duration::from_secs(30));
        failed.extend(dark.err().map(|error| format!("dark mode not heard: {error}")));
        simctl(&["ui", &device, "appearance", "light"])?;
        simctl(&["launch", &device, "com.apple.Preferences"])?;
        let away = note(&device, CHECK_ID, "looks.txt", |text| text.contains("away true"), Duration::from_secs(30));
        failed.extend(away.err().map(|error| format!("the background not heard: {error}")));
        simctl(&["launch", &device, CHECK_ID])?;
        let back = note(&device, CHECK_ID, "looks.txt", |text| text.contains("away false"), Duration::from_secs(30));
        failed.extend(back.err().map(|error| format!("coming back not heard: {error}")));
        if let Ok(looks) = note(&device, CHECK_ID, "looks.txt", |_| true, Duration::ZERO) {
            println!("looks:\n{looks}");
        }

        // What a reader reads of lines, letters and a long table.
        launch(&device, CHECK_ID, "reader")?;
        let read = note(&device, CHECK_ID, "reader.txt", |text| text.contains("done"), Duration::from_secs(120));
        let read = read.unwrap_or_else(|error| error);
        println!(
            "reader:
{read}"
        );
        failed.extend(read.lines().filter(|line| line.starts_with("FAILED") || line.contains("after")).map(str::to_owned));

        // The parts, driven.
        launch(&device, CHECK_ID, "parts")?;
        let parts = note(&device, CHECK_ID, "parts.txt", |text| text.contains("done"), Duration::from_secs(240));
        let parts = parts.unwrap_or_else(|error| error);
        println!("parts:\n{parts}");
        failed.extend(parts.lines().filter(|line| line.starts_with("FAILED") || line.contains("after")).map(str::to_owned));

        // The gallery example itself.
        launch(&device, "dev.taut.gallery", "")?;
        std::thread::sleep(Duration::from_secs(6));
        let shown = screen(&device, "gallery")?;
        let (share, most) = compared(&shown, &page, &here, "gallery");
        println!("the gallery example against the Mac's drawing: {:.2}% of pixels apart, at most {most}", share * 100.0);
        if share > 0.03 {
            failed.push(format!("the gallery example differs from the Mac's drawing in {:.2}% of its pixels", share * 100.0));
        }
        for log in std::fs::read_dir(built()).map_err(|error| error.to_string())?.flatten() {
            let path = log.path();
            if path.extension().is_some_and(|ending| ending == "err" || ending == "out") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let tail: Vec<&str> = text.lines().rev().take(15).collect();
                println!("--- {}:\n{}", path.display(), tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
            }
        }
        _ = simctl(&["terminate", &device, CHECK_ID]);
        _ = simctl(&["terminate", &device, "dev.taut.gallery"]);
        Ok(failed)
    }
}
