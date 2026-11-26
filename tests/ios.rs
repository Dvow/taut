//! taut on iOS, in the simulator. One file, two programs:
//!
//! - Built for the simulator, it is an app checking itself (`phone`): the gallery, or a page it
//!   drives as a person and the system would, through the methods UIKit calls. `parts`: touches,
//!   keys, typed, corrected and composed text, the keyboard reading the box, the edit menu, a
//!   screen reader, rotation. `system`: the system's file dialog, a file opened, drops, a drag
//!   out, files on the clipboard. `notices`: a notification and a click on it. `windows`: a
//!   second window in a scene of its own (iPad), pressed, closed by the person and by the app.
//!   `dialog`: a dialog window on an iPhone, which shows none, keeping nothing from input. It
//!   writes what it saw into its temporary folder.
//! - On a Mac (`mac`), with `TAUT_IOS_SIMULATOR=1`, it builds that app and the gallery example
//!   as bundles (`examples/ios_bundle.rs`), boots the newest iPhone simulator, runs them, reads
//!   their notes, takes the screen (`simctl io screenshot`) and holds it against the same
//!   interface drawn on the Mac; switches the simulator to dark mode, and sends the app to the
//!   background and back; drives `dialog` there, then `windows` on the newest iPad simulator.
//!   Without that variable it does nothing (a simulator takes minutes).
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

#[cfg(target_os = "ios")]
mod phone {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CStr, c_char, c_void};
    use std::io::Write;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use taut::{DragData, DropEffect, FileDialog, Notification, Role, Ui, Window};

    type Obj = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
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
        fn class_getInstanceMethod(class: Obj, name: *const c_void) -> Obj;
        fn method_setImplementation(method: Obj, imp: *const c_void) -> *const c_void;
    }

    // The main queue (`dispatch_get_main_queue()` is this object's address).
    unsafe extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }

    /// Has work done on the main thread once the run going on is over, as UIKit's own calls
    /// come.
    fn after_this_run(work: Box<dyn FnOnce()>) {
        let work = Box::into_raw(Box::new(work));
        unsafe { dispatch_async_f((&raw const _dispatch_main_q).cast(), work.cast(), run_later) };
    }
    extern "C" fn run_later(work: *mut c_void) {
        let work = unsafe { Box::from_raw(work.cast::<Box<dyn FnOnce()>>()) };
        work();
    }

    fn class(name: &CStr) -> Obj {
        unsafe { objc_getClass(name.as_ptr()) }
    }
    fn sel(name: &CStr) -> *const c_void {
        unsafe { sel_registerName(name.as_ptr()) }
    }
    /// `objc_msgSend` as a method of no arguments, one, two, three.
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
    unsafe fn send3<R, A, B, C>(to: Obj, name: &CStr, a: A, b: B, c: C) -> R {
        let call: unsafe extern "C" fn(Obj, *const c_void, A, B, C) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { call(to, sel(name), a, b, c) }
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
    fn array(objects: &[Obj]) -> Obj {
        unsafe { send2(class(c"NSArray"), c"arrayWithObjects:count:", objects.as_ptr(), objects.len()) }
    }
    fn file_address(path: &std::path::Path) -> Obj {
        unsafe { send1(class(c"NSURL"), c"fileURLWithPath:", string(&path.to_string_lossy())) }
    }

    // ---- stand-ins for what UIKit makes: a finger's touch, a hardware key's press, a drag
    // session, a notification's answer, the keyboard listening ----

    thread_local! {
        /// Where each stand-in touch is, in the view, and its type (0 a finger, 2 a pencil, 3 a
        /// pointer); each stand-in key's code, flags, letters.
        static TOUCHES: RefCell<Vec<(usize, Point, isize)>> = const { RefCell::new(Vec::new()) };
        static KEYS: RefCell<Vec<(usize, isize, isize, &'static str)>> = const { RefCell::new(Vec::new()) };
        /// The stand-in drag session's items and place; what the stand-in keyboard heard; how
        /// often the system's edit menu was asked for, and its own method for that.
        static SESSION: Cell<(Obj, Point)> = const { Cell::new((std::ptr::null_mut(), Point { x: 0.0, y: 0.0 })) };
        static TOLD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static MENUS: Cell<(u32, *const c_void)> = const { Cell::new((0, std::ptr::null())) };
        /// What a drag lifted: its items' names, and whether a preview of the box lifts.
        static LIFTS: RefCell<(Vec<String>, bool)> = const { RefCell::new((Vec::new(), false)) };
        /// What the keyboard's places were, between runs; whether a long press may begin on the
        /// text box, and on a button.
        static PLACES: RefCell<Option<(bool, String)>> = const { RefCell::new(None) };
        static LONG_PRESSES: Cell<Option<(bool, bool)>> = const { Cell::new(None) };
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
    extern "C" fn session_items(_: Obj, _: *const c_void) -> Obj {
        SESSION.get().0
    }
    extern "C" fn session_place(_: Obj, _: *const c_void, _: Obj) -> Point {
        SESSION.get().1
    }
    extern "C" fn answer_itself(answer: Obj, _: *const c_void) -> Obj {
        answer
    }
    /// A stand-in gesture: begun (1), at the session's place.
    extern "C" fn gesture_began(_: Obj, _: *const c_void) -> isize {
        1
    }
    extern "C" fn gesture_place(_: Obj, _: *const c_void, _: Obj) -> Point {
        SESSION.get().1
    }
    /// A stand-in pointer event: the secondary button (2) down.
    extern "C" fn secondary_button(_: Obj, _: *const c_void) -> isize {
        2
    }
    extern "C" fn first_notification(_: Obj, _: *const c_void) -> Obj {
        string("1")
    }
    extern "C" fn told(_: Obj, name: *const c_void, _: Obj) {
        let name = ["textWillChange:", "textDidChange:", "selectionWillChange:", "selectionDidChange:"].into_iter().find(|said| sel(&std::ffi::CString::new(*said).unwrap()) == name);
        TOLD.with_borrow_mut(|told| told.push(name.unwrap_or("?").to_string()));
    }
    extern "C" fn edit_menu_shown(menu: Obj, name: *const c_void, configuration: Obj) {
        let (count, own) = MENUS.get();
        MENUS.set((count + 1, own));
        let own: extern "C" fn(Obj, *const c_void, Obj) = unsafe { std::mem::transmute(own) };
        own(menu, name, configuration);
    }

    /// A subclass of a class with these methods, made once.
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

    /// A drag session over the view: these items (`UIDragItem`s) at this place.
    fn session(dragged: &[Obj], at: Point) -> Obj {
        let class = stand_in(c"CheckSession", c"NSObject", &[(c"items", session_items as *const c_void, c"@@:"), (c"locationInView:", session_place as *const c_void, c"{CGPoint=dd}@:@")]);
        let dragged = unsafe { send0::<Obj>(array(dragged), c"retain") };
        SESSION.set((dragged, at));
        unsafe { send0(class, c"new") }
    }

    /// A drag item with an item provider made by `made` from one object.
    fn drag_item(made: &CStr, from: Obj) -> Obj {
        let provider: Obj = unsafe { send1(send0::<Obj>(class(c"NSItemProvider"), c"alloc"), made, from) };
        unsafe { send1(send0::<Obj>(class(c"UIDragItem"), c"alloc"), c"initWithItemProvider:", provider) }
    }

    /// A drag item of a file, named as the Files app names what it drags.
    fn file_item(path: &std::path::Path) -> Obj {
        let item = drag_item(c"initWithContentsOfURL:", file_address(path));
        let name = path.file_name().unwrap().to_string_lossy();
        unsafe { send1::<(), _>(send0::<Obj>(item, c"itemProvider"), c"setSuggestedName:", string(&name)) };
        item
    }

    /// The system lifting a drag at a place in the view: the names of the items it gets, and
    /// whether a preview of what lifts comes with them.
    fn lift_at(at: Point) -> (Vec<String>, bool) {
        let session = session(&[], at);
        let drags = interaction(c"UIDragInteraction");
        let lifted = items(unsafe { send2(delegate_of(drags), c"dragInteraction:itemsForBeginningSession:", drags, session) });
        let name = |item: Obj| text_of(unsafe { send0(send0::<Obj>(item, c"itemProvider"), c"suggestedName") });
        let preview = |item: &Obj| unsafe { send3::<Obj, _, _, _>(delegate_of(drags), c"dragInteraction:previewForLiftingItem:session:", drags, *item, session) };
        (lifted.iter().map(|item| name(*item)).collect(), lifted.first().is_some_and(|item| !preview(item).is_null()))
    }

    /// The person's answer to the program's first notification: a click.
    fn click_on_first_notification() -> Obj {
        let methods: [(&CStr, *const c_void, &CStr); 3] =
            [(c"notification", answer_itself as *const c_void, c"@@:"), (c"request", answer_itself as *const c_void, c"@@:"), (c"identifier", first_notification as *const c_void, c"@@:")];
        unsafe { send0(stand_in(c"CheckAnswer", c"NSObject", &methods), c"new") }
    }

    /// The keyboard's ear on the input view: what it is told, noted.
    fn listen_as_keyboard() {
        let names = [c"textWillChange:", c"textDidChange:", c"selectionWillChange:", c"selectionDidChange:"];
        let methods: Vec<(&CStr, *const c_void, &CStr)> = names.into_iter().map(|name| (name, told as *const c_void, c"v@:@")).collect();
        let keyboard: Obj = unsafe { send0(stand_in(c"CheckKeyboard", c"NSObject", &methods), c"new") };
        unsafe { send1::<(), _>(input(), c"setInputDelegate:", keyboard) };
    }

    /// Counts the system's edit menus asked for, from now on.
    fn count_edit_menus() {
        if !MENUS.get().1.is_null() {
            return;
        }
        let method = unsafe { class_getInstanceMethod(class(c"UIEditMenuInteraction"), sel(c"presentEditMenuWithConfiguration:")) };
        let own = unsafe { method_setImplementation(method, edit_menu_shown as *const c_void) };
        MENUS.set((0, own));
    }

    fn set_of(object: Obj) -> Obj {
        unsafe { send1(class(c"NSSet"), c"setWithObject:", object) }
    }

    // ---- the app's views ----

    /// The library's view: the root view of the first scene's window.
    fn view() -> Obj {
        unsafe { send0(root(), c"view") }
    }
    /// The first window's view controller (the scene of a second window is titled "Second").
    fn root() -> Obj {
        unsafe {
            let app: Obj = send0(class(c"UIApplication"), c"sharedApplication");
            let scenes = items(send0(send0::<Obj>(app, c"connectedScenes"), c"allObjects"));
            let first = scenes.into_iter().find(|scene| text_of(send0(*scene, c"title")) != "Second").unwrap_or(std::ptr::null_mut());
            let window = items(send0(first, c"windows")).first().copied().unwrap_or(std::ptr::null_mut());
            send0(window, c"rootViewController")
        }
    }
    /// What the window's controller presents (null for nothing).
    fn presented() -> Obj {
        unsafe { send0(root(), c"presentedViewController") }
    }
    /// A subview of the library's view, by class name.
    fn subview(name: &CStr) -> Obj {
        let views = items(unsafe { send0(view(), c"subviews") });
        views.into_iter().find(|one| unsafe { send1::<u8, _>(*one, c"isKindOfClass:", class(name)) } != 0).unwrap_or(std::ptr::null_mut())
    }
    fn input() -> Obj {
        subview(c"TautTextInput")
    }
    /// The view's interaction of a class (null for none).
    fn interaction(name: &CStr) -> Obj {
        let interactions = items(unsafe { send0(view(), c"interactions") });
        interactions.into_iter().find(|one| is_of(*one, name)).unwrap_or(std::ptr::null_mut())
    }
    /// An interaction's delegate: what UIKit calls, with the interaction.
    fn delegate_of(interaction: Obj) -> Obj {
        unsafe { send0(interaction, c"delegate") }
    }
    /// Where the page lies in the view: its top left.
    fn origin() -> Point {
        origin_in(view())
    }

    // ---- the keyboard's view of the text box ----

    /// A document position of the input view.
    fn position(at: usize) -> Obj {
        unsafe { send2(input(), c"positionFromPosition:offset:", send0::<Obj>(input(), c"beginningOfDocument"), at as isize) }
    }
    fn offset(position: Obj) -> usize {
        unsafe { send2::<isize, _, _>(input(), c"offsetFromPosition:toPosition:", send0::<Obj>(input(), c"beginningOfDocument"), position) as usize }
    }
    fn text_range(from: usize, to: usize) -> Obj {
        unsafe { send2(input(), c"textRangeFromPosition:toPosition:", position(from), position(to)) }
    }
    fn ends(range: Obj) -> Option<(usize, usize)> {
        (!range.is_null()).then(|| unsafe { (offset(send0(range, c"start")), offset(send0(range, c"end"))) })
    }
    /// The whole document, as the keyboard reads it.
    fn document() -> String {
        let length = offset(unsafe { send0(input(), c"endOfDocument") });
        text_of(unsafe { send1(input(), c"textInRange:", text_range(0, length)) })
    }
    fn selected() -> Option<(usize, usize)> {
        ends(unsafe { send0(input(), c"selectedTextRange") })
    }
    fn caret(at: usize) -> Rect {
        unsafe { send1(input(), c"caretRectForPosition:", position(at)) }
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
        match mode.as_str() {
            "reader" => drive(window, Reading::new()),
            "parts" => drive(window, Parts::default()),
            "system" => drive(window, System::new()),
            "notices" => drive(window, Notices::default()),
            "windows" => drive(window, Windows::default()),
            "dialog" => drive(window, PhoneDialog::default()),
            _ => {
                let mut app = super::gallery::app();
                let mut shown = Shown::default();
                taut::run(window, move |ui| {
                    super::gallery::describe(&mut app, ui);
                    shown.step(ui);
                });
            }
        }
    }

    /// Runs a page, driven by a script.
    fn drive<P: Page + 'static>(window: Window, page: P) {
        let state = Rc::new(RefCell::new((page, Script::default())));
        taut::run(window, move |ui| {
            let (page, script) = &mut *state.borrow_mut();
            page.describe(ui);
            script.step(page, ui);
        });
    }

    /// The gallery: once settled, its size for the Mac to draw it at, and what shows it; once
    /// the Mac has the screen, what it tells readers; asked, the window turned sideways and drawn
    /// again.
    #[derive(Default)]
    struct Shown {
        start: Option<Instant>,
        stage: u32,
        dark: Option<bool>,
        away: Option<bool>,
        /// Since when the window turns, and its size before.
        turning: Option<(Instant, taut::geom::Size)>,
    }

    impl Shown {
        fn step(&mut self, ui: &mut Ui) {
            let since = self.start.get_or_insert_with(Instant::now).elapsed();
            let at = origin();
            match self.stage {
                // (Asked for the GPU, once it shows the window: with `gpu-draw` its shaders
                // compile as it opens.)
                0 if since > Duration::from_secs(3) && (std::env::var_os("TAUT_RENDERER").is_none() || ui.renderer() != taut::Renderer::Cpu || since > Duration::from_secs(60)) => {
                    drawn_here(ui);
                    note("ready.txt", &page_said(ui, at));
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
            self.turn(ui, at);
        }

        /// Once the Mac asks (the file `turn`), the window turns sideways; once it has, and has
        /// settled, it is drawn here again and its page said (`turned.txt`).
        fn turn(&mut self, ui: &mut Ui, at: Point) {
            if self.stage >= 1 && self.turning.is_none() && folder().join("turn").exists() {
                turn(true);
                self.turning = Some((Instant::now(), ui.size()));
            }
            let Some((since, before)) = self.turning else { return };
            if ui.size() != before && since.elapsed() > Duration::from_secs(3) && !folder().join("turned.txt").exists() {
                drawn_here(ui);
                note("turned.txt", &page_said(ui, at));
            }
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

    /// The window's page as the Mac reads it: size, scale, where it lies, dark or light, the text
    /// scale; and what shows it.
    fn page_said(ui: &Ui, at: Point) -> String {
        let size = ui.size();
        format!("{} {} {} {} {} {} {} {:?}", size.w, size.h, ui.scale(), at.x, at.y, ui.system_dark() as u8, ui.text_scale(), ui.renderer())
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

    // ---- the script: steps acted, then checked ----

    /// A page the script drives: its steps (the last empty), the note it writes, what it shows,
    /// and how each step acts and shows it worked.
    trait Page: std::fmt::Debug {
        const STEPS: &'static [&'static str];
        const NOTE: &'static str;
        fn describe(&mut self, ui: &mut Ui);
        fn act(&mut self, script: &mut Script, name: &str, ui: &mut Ui);
        /// Whether the step's outcome shows: yes, or not yet.
        fn check(&mut self, name: &str, ui: &Ui) -> bool;
    }

    /// Where the script is: its step, since when, whether it acted; a touch held down, the view it
    /// is on, and until when.
    struct Script {
        step: usize,
        since: Option<Instant>,
        acted: bool,
        held: Obj,
        held_on: Obj,
        until: Instant,
    }

    impl Default for Script {
        fn default() -> Self {
            Script { step: 0, since: None, acted: false, held: std::ptr::null_mut(), held_on: std::ptr::null_mut(), until: Instant::now() }
        }
    }

    impl Script {
        fn step<P: Page>(&mut self, page: &mut P, ui: &mut Ui) {
            let since = *self.since.get_or_insert_with(Instant::now);
            if since.elapsed() < Duration::from_millis(500) && self.step == 0 {
                return;
            }
            let name = P::STEPS.get(self.step).copied().unwrap_or("");
            if name.is_empty() {
                return;
            }
            if !self.acted {
                self.acted = true;
                page.act(self, name, ui);
                return;
            }
            if self.lift() {
                return;
            }
            match page.check(name, ui) {
                true => note(P::NOTE, &format!("ok {name}")),
                false if since.elapsed() < Duration::from_secs(8) => return,
                false => note(P::NOTE, &format!("FAILED {name}: {page:?}; places {:?}, long presses {:?}, edit menus {}", PLACES.take(), LONG_PRESSES.get(), MENUS.get().0)),
            }
            (self.step, self.since, self.acted) = (self.step + 1, Some(Instant::now()), false);
            if P::STEPS[self.step].is_empty() {
                note(P::NOTE, "done");
            }
        }

        /// A touch held down lifts once its time is up. Says whether one is still down.
        fn lift(&mut self) -> bool {
            if self.held.is_null() {
                return false;
            }
            if Instant::now() < self.until {
                return true;
            }
            let touch = std::mem::replace(&mut self.held, std::ptr::null_mut());
            unsafe { send2::<(), _, _>(self.held_on, c"touchesEnded:withEvent:", set_of(touch), std::ptr::null_mut::<c_void>()) };
            true
        }

        /// A finger, pencil or pointer lands, then lifts (a run later).
        fn tap(&mut self, at: Point, kind: isize) {
            self.hold(at, kind, Duration::ZERO);
        }

        /// The same, lifting after a while.
        fn hold(&mut self, at: Point, kind: isize, held: Duration) {
            self.hold_on(view(), at, kind, held);
        }

        /// The same on a view of another window.
        fn hold_on(&mut self, on: Obj, at: Point, kind: isize, held: Duration) {
            let touch = touch_at(at, kind);
            unsafe { send2::<(), _, _>(on, c"touchesBegan:withEvent:", set_of(touch), std::ptr::null_mut::<c_void>()) };
            (self.held, self.held_on, self.until) = (touch, on, Instant::now() + held);
        }

        /// A pointer's secondary click (a trackpad's two-finger click): down, then up a run later.
        fn click_secondary(&mut self, at: Point) {
            let touch = touch_at(at, 3);
            let event: Obj = unsafe { send0(stand_in(c"CheckEvent", c"NSObject", &[(c"buttonMask", secondary_button as *const c_void, c"q@:")]), c"new") };
            unsafe { send2::<(), _, _>(view(), c"touchesBegan:withEvent:", set_of(touch), event) };
            (self.held, self.held_on, self.until) = (touch, view(), Instant::now());
        }

        fn key(code: isize, flags: isize, letters: &'static str) {
            let press = press_of(code, flags, letters);
            let target = if unsafe { send0::<u8>(input(), c"isFirstResponder") } != 0 { input() } else { view() };
            unsafe {
                send2::<(), _, _>(target, c"pressesBegan:withEvent:", set_of(press), std::ptr::null_mut::<c_void>());
                send2::<(), _, _>(target, c"pressesEnded:withEvent:", set_of(press), std::ptr::null_mut::<c_void>());
            }
        }
    }

    /// The middle of a part, in the view: the first of a role, or the one named.
    fn middle_of(ui: &Ui, role: Role, name: &str) -> Point {
        let found = ui.visible_access_nodes().iter().find(|node| node.role == role && (name.is_empty() || node.name == name));
        let node = found.map(|node| node.rect).unwrap_or_default();
        let at = origin();
        Point { x: at.x + f64::from(node.x + node.w / 2.0), y: at.y + f64::from(node.y + node.h / 2.0) }
    }

    // ---- parts: touches, keys, the keyboard, a reader ----

    /// The page of parts the app drives: a button counting presses, a text box, a tick, a slider.
    #[derive(Debug, Default)]
    struct Parts {
        presses: u32,
        text: String,
        tick: bool,
        amount: f32,
        /// The edit menus shown before the step.
        menus: u32,
        /// The app puts this text in the box in its next run.
        put: Option<String>,
    }

    impl Page for Parts {
        const NOTE: &'static str = "parts.txt";
        const STEPS: &'static [&'static str] = &[
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
            "keyboard reads the box",
            "keyboard corrects a word",
            "keyboard moves the cursor",
            "composing over a selection",
            "what was composed is committed",
            "places of the text",
            "the keyboard hears the app's change",
            "long press shows the system's edit menu",
            "a secondary click shows the system's edit menu",
            "reader ticks",
            "reader steps the slider",
            "turned sideways",
            "turned upright",
            "",
        ];

        fn describe(&mut self, ui: &mut Ui) {
            ui.theme = ui.system_theme();
            ui.column().pad(16.0).gap(12.0).open();
            ui.label("Parts");
            self.presses += ui.button("Press").clicked as u32;
            if let Some(put) = self.put.take() {
                self.text = put;
            }
            ui.text_box(&mut self.text, "Type here");
            ui.checkbox(&mut self.tick, "Tick");
            ui.slider(&mut self.amount, 0.0..=10.0);
            ui.label(&format!("Pressed {} times", self.presses));
            ui.close();
        }

        fn act(&mut self, script: &mut Script, name: &str, ui: &mut Ui) {
            let input = input();
            unsafe {
                match name {
                    "reader asked" => _ = elements(),
                    "finger taps the button" => script.tap(middle_of(ui, Role::Button, ""), 0),
                    "pointer clicks the button" => script.tap(middle_of(ui, Role::Button, ""), 3),
                    "pencil taps the button" => script.tap(middle_of(ui, Role::Button, ""), 2),
                    "finger taps the text box" => script.tap(middle_of(ui, Role::TextBox, ""), 0),
                    "typed text" => send1::<(), _>(input, c"insertText:", string("h\u{e9}llo")),
                    "composed text" => {
                        send2::<(), _, _>(input, c"setMarkedText:selectedRange:", string("\u{304b}"), Range { location: 1, length: 0 });
                        send0::<()>(input, c"unmarkText");
                    }
                    "keyboard's delete" => send0::<()>(input, c"deleteBackward"),
                    "hardware backspace" => Script::key(0x2a, 0, ""),
                    "select all and copy" => {
                        send1::<(), _>(input, c"selectAll:", std::ptr::null_mut::<c_void>());
                        send1::<(), _>(input, c"copy:", std::ptr::null_mut::<c_void>());
                    }
                    "command-a and backspace" => {
                        Script::key(0x04, 1 << 20, "a");
                        Script::key(0x2a, 0, "");
                    }
                    "paste" => send1::<(), _>(input, c"paste:", std::ptr::null_mut::<c_void>()),
                    "keyboard corrects a word" => send2::<(), _, _>(input, c"replaceRange:withText:", text_range(0, 4), string("Hello")),
                    "keyboard moves the cursor" => {
                        send1::<(), _>(input, c"setSelectedTextRange:", text_range(0, 0));
                        send1::<(), _>(input, c"insertText:", string("\u{a1}"));
                    }
                    "composing over a selection" => {
                        send1::<(), _>(input, c"setSelectedTextRange:", text_range(1, 6));
                        send2::<(), _, _>(input, c"setMarkedText:selectedRange:", string("\u{304b}"), Range { location: 1, length: 0 });
                    }
                    "what was composed is committed" => send0::<()>(input, c"unmarkText"),
                    // (UIKit asks for places between runs.)
                    "places of the text" => {
                        let field = ui.visible_access_nodes().iter().find(|node| node.role == Role::TextBox).map(|node| node.rect).unwrap_or_default();
                        after_this_run(Box::new(move || PLACES.set(Some(places_are_right(field)))));
                    }
                    "the keyboard hears the app's change" => {
                        listen_as_keyboard();
                        self.put = Some("new".into());
                    }
                    // A finger rests on the text box; UIKit's long press recognizer asks the view
                    // there, and on the button.
                    "long press shows the system's edit menu" => {
                        count_edit_menus();
                        let (field, button) = (middle_of(ui, Role::TextBox, ""), middle_of(ui, Role::Button, ""));
                        script.hold(field, 0, Duration::from_millis(600));
                        after_this_run(Box::new(move || LONG_PRESSES.set(Some((long_press_at(field), long_press_at(button))))));
                    }
                    "a secondary click shows the system's edit menu" => {
                        self.menus = MENUS.get().0;
                        script.click_secondary(middle_of(ui, Role::TextBox, ""));
                    }
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

        fn check(&mut self, name: &str, ui: &Ui) -> bool {
            let pasted = || text_of(unsafe { send0(send0::<Obj>(class(c"UIPasteboard"), c"generalPasteboard"), c"string") });
            let typing = || unsafe { send0::<u8>(input(), c"isFirstResponder") } != 0;
            let tick = || elements().iter().find(|(label, ..)| label == "Tick").map(|(_, _, traits, _)| *traits);
            let trait_of = |name: &CStr| unsafe { send0::<isize>(input(), name) };
            let size = ui.size();
            match name {
                "reader asked" => !ui.visible_access_nodes().is_empty() && !elements().is_empty(),
                "finger taps the button" => self.presses == 1,
                "pointer clicks the button" => self.presses == 2,
                "pencil taps the button" => self.presses == 3,
                "finger taps the text box" => typing() && ui.ime_area().is_some(),
                "typed text" => self.text == "h\u{e9}llo",
                "composed text" => self.text == "h\u{e9}llo\u{304b}",
                "keyboard's delete" => self.text == "h\u{e9}llo",
                "hardware backspace" => self.text == "h\u{e9}ll",
                "select all and copy" => pasted() == "h\u{e9}ll",
                "command-a and backspace" => self.text.is_empty(),
                "paste" => self.text == "h\u{e9}ll",
                // The system's choice of corrections (0), and capitals starting sentences (2).
                "keyboard reads the box" => document() == "h\u{e9}ll" && selected() == Some((4, 4)) && trait_of(c"autocorrectionType") == 0 && trait_of(c"autocapitalizationType") == 2,
                "keyboard corrects a word" => self.text == "Hello" && document() == "Hello" && selected() == Some((5, 5)),
                "keyboard moves the cursor" => self.text == "\u{a1}Hello" && selected() == Some((1, 1)),
                "composing over a selection" => self.text == "\u{a1}Hello" && document() == "\u{a1}\u{304b}" && ends(unsafe { send0(input(), c"markedTextRange") }) == Some((1, 2)),
                "what was composed is committed" => self.text == "\u{a1}\u{304b}" && document() == "\u{a1}\u{304b}" && selected() == Some((2, 2)),
                "places of the text" => PLACES.with_borrow(|places| places.as_ref().is_some_and(|(right, _)| *right)),
                "the keyboard hears the app's change" => self.text == "new" && document() == "new" && TOLD.with_borrow(|told| told.iter().any(|name| name == "textDidChange:")),
                "long press shows the system's edit menu" => {
                    LONG_PRESSES.get() == Some((true, false)) && MENUS.get().0 > 0 && !ui.visible_access_nodes().iter().any(|node| node.role == Role::MenuItem)
                }
                "a secondary click shows the system's edit menu" => MENUS.get().0 > self.menus && !ui.visible_access_nodes().iter().any(|node| node.role == Role::MenuItem),
                // Button (1) and selected (8).
                "reader ticks" => self.tick && tick().is_some_and(|traits| traits & 0x9 == 0x9),
                "reader steps the slider" => self.amount > 0.0,
                "turned sideways" => size.w > size.h,
                "turned upright" => size.w < size.h,
                _ => true,
            }
        }
    }

    /// The keyboard's places in "¡か", as UIKit asks between runs: carets inside the text box
    /// (`field`, on the page), left to right; a selection of both letters one piece; the
    /// position nearest the middle caret is 1. And what was found.
    fn places_are_right(field: taut::geom::Rect) -> (bool, String) {
        let (first, last) = (caret(0), caret(2));
        let inside = |at: Rect| at.origin.x >= f64::from(field.x) && at.origin.x <= f64::from(field.x + field.w) && at.origin.y >= f64::from(field.y) && at.origin.y < f64::from(field.y + field.h);
        let pieces = items(unsafe { send1(input(), c"selectionRectsForRange:", text_range(0, 2)) });
        let piece: Rect = pieces.first().map_or_else(Rect::default, |piece| unsafe { send0(*piece, c"rect") });
        let middle = caret(1);
        let near: Obj = unsafe { send1(input(), c"closestPositionToPoint:", Point { x: middle.origin.x, y: middle.origin.y + middle.size.y / 2.0 }) };
        let right = inside(first) && inside(last) && first.origin.x < last.origin.x && pieces.len() == 1 && piece.size.x > 0.0 && offset(near) == 1;
        (right, format!("box {field:?}, carets {first:?} {middle:?} {last:?}, pieces {} {piece:?}, nearest {}", pieces.len(), offset(near)))
    }

    /// The system's long press on a part (a stand-in of UIKit's recognizer): whether the view
    /// lets it begin; if it does, it is recognized.
    fn long_press_at(at: Point) -> bool {
        let methods: [(&CStr, *const c_void, &CStr); 2] = [(c"state", gesture_began as *const c_void, c"q@:"), (c"locationInView:", gesture_place as *const c_void, c"{CGPoint=dd}@:@")];
        let gesture: Obj = unsafe { send0(stand_in(c"CheckLongPress", c"TautLongPress", &methods), c"new") };
        SESSION.set((std::ptr::null_mut(), at));
        let begins = unsafe { send1::<u8, _>(view(), c"gestureRecognizerShouldBegin:", gesture) } != 0;
        if begins {
            unsafe { send1::<(), _>(view(), c"held:", gesture) };
        }
        begins
    }

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

    // ---- the system's parts: the file dialog, opening a file, drops, a drag out, the
    // clipboard's files ----

    /// What the app asks of the file dialog next.
    #[derive(Debug, Clone, Copy)]
    enum Ask {
        Several,
        One,
        Save,
    }

    /// The page of the system's parts: a file dialog, a box taking drops, a box handing a file
    /// out; and what each heard.
    #[derive(Debug)]
    struct System {
        files: FileDialog,
        sample: Vec<PathBuf>,
        ask: Option<Ask>,
        answers: Vec<PathBuf>,
        open: bool,
        hovered: bool,
        dropped: Vec<PathBuf>,
        dropped_text: String,
        dragged: Option<DropEffect>,
        copy: bool,
        read: bool,
        pasted: Vec<PathBuf>,
    }

    impl System {
        fn new() -> System {
            let sample = folder().join("sample.txt");
            _ = std::fs::write(&sample, "sample words");
            System {
                files: FileDialog::native(),
                sample: vec![sample],
                ask: None,
                answers: Vec::new(),
                open: false,
                hovered: false,
                dropped: Vec::new(),
                dropped_text: String::new(),
                dragged: None,
                copy: false,
                read: false,
                pasted: Vec::new(),
            }
        }
    }

    /// A small PNG: two pixels.
    fn png() -> Vec<u8> {
        taut::png::encode_rgba(2, &[0xff10_2030, 0xff40_5060]).unwrap()
    }

    impl Page for System {
        const NOTE: &'static str = "system.txt";
        const STEPS: &'static [&'static str] = &[
            "reader asked",
            "the system's dialog opens for several files",
            "two files are picked",
            "the system's dialog saves",
            "where it was saved is the answer",
            "the system's dialog opens for a file",
            "it is closed without a choice",
            "a file opens in the system's preview",
            "the preview is put away",
            "files come over the window",
            "files and a picture are dropped",
            "text is dropped",
            "a long press lifts the box's file",
            "the other app copies it",
            "files copied are read back",
            "files another app copied are pasted",
            "",
        ];

        fn describe(&mut self, ui: &mut Ui) {
            ui.theme = ui.system_theme();
            ui.column().pad(16.0).gap(12.0).open();
            let folder = folder();
            match self.ask.take() {
                Some(Ask::Several) => self.files.pick_many(&folder),
                Some(Ask::One) => self.files.pick(&folder),
                Some(Ask::Save) => {
                    self.files.only(&[".txt"]);
                    self.files.save(&folder, "notes");
                }
                None => {}
            }
            if let Some(answer) = ui.file_dialog(&mut self.files) {
                self.answers.push(answer);
            }
            if std::mem::take(&mut self.open) {
                taut::open_with_system(&self.sample[0]);
            }
            let target = ui.button("Drop here");
            self.hovered |= ui.files_hovered();
            self.dropped = ui.files_dropped(&target).unwrap_or(std::mem::take(&mut self.dropped));
            self.dropped_text = ui.text_dropped(&target).unwrap_or(std::mem::take(&mut self.dropped_text));
            let source = ui.button("Drag me");
            self.dragged = ui.drag_out(&source, DragData::Files(&self.sample)).or(self.dragged);
            if std::mem::take(&mut self.copy) {
                ui.copy_files(&self.sample);
            }
            if std::mem::take(&mut self.read) {
                self.pasted = ui.clipboard_files();
            }
            ui.close();
        }

        fn act(&mut self, _: &mut Script, name: &str, ui: &mut Ui) {
            let picker = || unsafe { send0::<Obj>(presented(), c"delegate") };
            let drop_here = || middle_of(ui, Role::Button, "Drop here");
            unsafe {
                match name {
                    "reader asked" => _ = elements(),
                    "the system's dialog opens for several files" => self.ask = Some(Ask::Several),
                    "two files are picked" => {
                        let two = [folder().join("one.txt"), folder().join("two.txt")];
                        two.iter().for_each(|file| _ = std::fs::write(file, "picked"));
                        let addresses = array(&[file_address(&two[0]), file_address(&two[1])]);
                        send2::<(), _, _>(picker(), c"documentPicker:didPickDocumentsAtURLs:", presented(), addresses);
                    }
                    "the system's dialog saves" => self.ask = Some(Ask::Save),
                    "where it was saved is the answer" => {
                        let moved = folder().join("moved").join("notes.txt");
                        _ = std::fs::create_dir_all(moved.parent().unwrap());
                        _ = std::fs::write(&moved, "");
                        send2::<(), _, _>(picker(), c"documentPicker:didPickDocumentsAtURLs:", presented(), array(&[file_address(&moved)]));
                    }
                    "the system's dialog opens for a file" => self.ask = Some(Ask::One),
                    "it is closed without a choice" => send1::<(), _>(picker(), c"documentPickerWasCancelled:", presented()),
                    "a file opens in the system's preview" => self.open = true,
                    "the preview is put away" => send2::<(), _, _>(presented(), c"dismissViewControllerAnimated:completion:", 0u8, std::ptr::null_mut::<c_void>()),
                    "files come over the window" => {
                        let session = session(&[file_item(&self.sample[0])], drop_here());
                        let drops = interaction(c"UIDropInteraction");
                        _ = send2::<u8, _, _>(delegate_of(drops), c"dropInteraction:canHandleSession:", drops, session);
                        send2::<(), _, _>(delegate_of(drops), c"dropInteraction:sessionDidEnter:", drops, session);
                    }
                    "files and a picture are dropped" => {
                        let picture: Obj = send1(class(c"UIImage"), c"imageWithData:", send2::<Obj, _, _>(class(c"NSData"), c"dataWithBytes:length:", png().as_ptr(), png().len()));
                        let dragged = [file_item(&self.sample[0]), drag_item(c"initWithObject:", picture)];
                        let session = session(&dragged, drop_here());
                        let drops = interaction(c"UIDropInteraction");
                        send2::<(), _, _>(delegate_of(drops), c"dropInteraction:performDrop:", drops, session);
                    }
                    "text is dropped" => {
                        let session = session(&[drag_item(c"initWithObject:", string("dropped words"))], drop_here());
                        let drops = interaction(c"UIDropInteraction");
                        send2::<(), _, _>(delegate_of(drops), c"dropInteraction:sessionDidEnter:", drops, session);
                        send2::<(), _, _>(delegate_of(drops), c"dropInteraction:performDrop:", drops, session);
                    }
                    // (UIKit lifts between runs: the box is run again while it does.)
                    "a long press lifts the box's file" => {
                        let at = middle_of(ui, Role::Button, "Drag me");
                        after_this_run(Box::new(move || LIFTS.set(lift_at(at))));
                    }
                    "the other app copies it" => {
                        let drags = interaction(c"UIDragInteraction");
                        send3::<(), _, _, _>(delegate_of(drags), c"dragInteraction:session:didEndWithOperation:", drags, std::ptr::null_mut::<c_void>(), 2isize);
                    }
                    "files copied are read back" => (self.copy, self.read) = (true, true),
                    "files another app copied are pasted" => {
                        let provider: Obj = send2(
                            send0::<Obj>(class(c"NSItemProvider"), c"alloc"),
                            c"initWithItem:typeIdentifier:",
                            send2::<Obj, _, _>(class(c"NSData"), c"dataWithBytes:length:", png().as_ptr(), png().len()),
                            string("public.png"),
                        );
                        send1::<(), _>(provider, c"setSuggestedName:", string("shot"));
                        send1::<(), _>(send0::<Obj>(class(c"UIPasteboard"), c"generalPasteboard"), c"setItemProviders:", array(&[provider]));
                        self.read = true;
                    }
                    _ => {}
                }
            }
        }

        fn check(&mut self, name: &str, ui: &Ui) -> bool {
            let picker = || unsafe { send1::<u8, _>(presented(), c"isKindOfClass:", class(c"UIDocumentPickerViewController")) } != 0;
            let several = || unsafe { send0::<u8>(presented(), c"allowsMultipleSelection") } != 0;
            let saving = || std::fs::read_dir(std::env::temp_dir().join("taut-save")).into_iter().flatten().flatten().any(|folder| folder.path().join("notes.txt").exists());
            let named = |files: &[PathBuf]| files.iter().map(|file| file.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>();
            let board = || unsafe { send0::<Obj>(class(c"UIPasteboard"), c"generalPasteboard") };
            match name {
                "reader asked" => !ui.visible_access_nodes().is_empty(),
                "the system's dialog opens for several files" => picker() && several(),
                "two files are picked" => self.answers == [folder().join("one.txt")] && self.files.chosen == [folder().join("one.txt"), folder().join("two.txt")] && presented().is_null(),
                "the system's dialog saves" => picker() && saving(),
                "where it was saved is the answer" => self.answers.last() == Some(&folder().join("moved").join("notes.txt")) && presented().is_null(),
                "the system's dialog opens for a file" => picker() && !several(),
                "it is closed without a choice" => !self.files.shown && self.answers.len() == 2 && presented().is_null(),
                "a file opens in the system's preview" => !presented().is_null() && !picker(),
                "the preview is put away" => presented().is_null(),
                "files come over the window" => self.hovered,
                "files and a picture are dropped" => {
                    let names = named(&self.dropped);
                    names.len() == 2 && names[0] == "sample.txt" && names[1].ends_with(".png") && std::fs::read_to_string(&self.dropped[0]).is_ok_and(|text| text == "sample words")
                }
                "text is dropped" => self.dropped_text == "dropped words",
                "a long press lifts the box's file" => LIFTS.with_borrow(|(names, previewed)| names == &["sample.txt"] && *previewed),
                "the other app copies it" => self.dragged == Some(DropEffect::Copied),
                "files copied are read back" => self.pasted == self.sample && items(unsafe { send0(board(), c"itemProviders") }).len() == 1,
                "files another app copied are pasted" => named(&self.pasted).len() == 1 && named(&self.pasted)[0].ends_with(".png") && std::fs::read(&self.pasted[0]).is_ok_and(|bytes| bytes == png()),
                _ => true,
            }
        }
    }

    // ---- a reader: lines (UIAccessibilityReadingContent), letters and selection (each
    // element's accessibilityTextInputResponder, a UITextInput), a long table as a data table ----

    const PARAGRAPH: &str = "A paragraph long enough to break onto several lines of a phone's window, which a screen reader reads line by line and letter by letter.";

    thread_local! {
        /// What the reader was given asking for row 400's first cell between turns: its label
        /// and the row it says it is in.
        static ROW_ASKED: RefCell<Option<(String, Range)>> = const { RefCell::new(None) };
    }

    /// The page a reader reads: a paragraph that wraps onto several lines, a text box, and a table
    /// of five hundred rows (those in view told).
    #[derive(Debug)]
    struct Reading {
        notes: String,
        table: taut::Table,
        /// What the last check found, when it was not what was wanted.
        found: String,
    }

    impl Reading {
        fn new() -> Self {
            Reading { notes: "héllo wörld".into(), table: taut::Table::default(), found: String::new() }
        }
    }

    impl Page for Reading {
        const NOTE: &'static str = "reader.txt";
        const STEPS: &'static [&'static str] = &[
            "reader asked",
            "lines as wrapped",
            "letters where drawn",
            "a reader selects in the text box",
            "a data table",
            "a row out of view comes at once",
            "a three-finger swipe pages the table on",
            "",
        ];

        fn describe(&mut self, ui: &mut Ui) {
            ui.theme = ui.system_theme();
            ui.column().pad(16.0).gap(12.0).grow().open();
            ui.paragraph(PARAGRAPH);
            ui.text_box(&mut self.notes, "Notes");
            let columns = [taut::Column::new("Name").px(160.0), taut::Column::new("Size").px(80.0)];
            ui.table(&mut self.table, &columns);
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

        fn act(&mut self, _: &mut Script, name: &str, _: &mut Ui) {
            match name {
                "reader asked" => _ = elements(),
                "a reader selects in the text box" => {
                    let reader: Obj = unsafe { send0(element("Notes"), c"accessibilityTextInputResponder") };
                    unsafe { send1::<(), _>(reader, c"setSelectedTextRange:", reader_range(reader, 1, 4)) };
                }
                "a row out of view comes at once" => after_this_run(Box::new(ask_for_row_400)),
                "a three-finger swipe pages the table on" => _ = unsafe { send1::<u8, _>(table(), c"accessibilityScroll:", 3isize) },
                _ => {}
            }
        }

        fn check(&mut self, name: &str, ui: &Ui) -> bool {
            let truth = |ok: bool, found: String| if ok { Ok(()) } else { Err(found) };
            let outcome = match name {
                "reader asked" => truth(!elements().is_empty() && !ui.visible_access_nodes().is_empty(), "no elements".into()),
                "lines as wrapped" => lines(),
                "letters where drawn" => letters(),
                "a reader selects in the text box" => {
                    let caret = ui.access_nodes().iter().find(|node| node.role == Role::TextBox).and_then(|node| node.caret);
                    let reader: Obj = unsafe { send0(element("Notes"), c"accessibilityTextInputResponder") };
                    let told = reader_ends(reader, unsafe { send0(reader, c"selectedTextRange") });
                    truth(caret == Some([1, 4]) && told == [1, 4], format!("the box's selection {caret:?}, the reader's {told:?}"))
                }
                "a data table" => data_table(),
                "a row out of view comes at once" => {
                    let asked = ROW_ASKED.with_borrow(Clone::clone);
                    truth(matches!(&asked, Some((label, Range { location: 400, length: 1 })) if label == "row 400"), format!("given {asked:?}"))
                }
                "a three-finger swipe pages the table on" => {
                    let first = ui.visible_access_nodes().iter().find(|node| node.role == Role::Cell).map(|node| node.name.clone());
                    truth(first.as_deref().is_some_and(|first| first != "row 0" && first != "row 400"), format!("the first cell in view is {first:?}"))
                }
                _ => Ok(()),
            };
            outcome.map_err(|found| self.found = found).is_ok()
        }
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
    fn reader_range(reader: Obj, from: isize, to: isize) -> Obj {
        unsafe {
            let start: Obj = send0(reader, c"beginningOfDocument");
            let [from, to]: [Obj; 2] = [from, to].map(|unit| send2(reader, c"positionFromPosition:offset:", start, unit));
            send2(reader, c"textRangeFromPosition:toPosition:", from, to)
        }
    }
    /// Where a range of a reader's text starts and ends, in UTF-16 units.
    fn reader_ends(reader: Obj, range: Obj) -> [isize; 2] {
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
    fn ask_for_row_400() {
        let cell = cell(400, 0);
        let said = unsafe { (text_of(send0(cell, c"accessibilityLabel")), send0::<Range>(cell, c"accessibilityRowRange")) };
        ROW_ASKED.with_borrow_mut(|asked| *asked = Some(said));
    }

    /// The paragraph by lines: its lines make its text, each lower than the one before, and a
    /// point on a line is on that line.
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
        let word = reader_range(reader, 2, 11);
        let (said, place) = unsafe { (text_of(send1(reader, c"textInRange:", word)), send1::<Rect, _>(reader, c"firstRectForRange:", word)) };
        let caret = unsafe { send1::<Rect, _>(reader, c"caretRectForPosition:", send0::<Obj>(word, c"start")) };
        let at = reader_ends(reader, unsafe { send1(reader, c"characterRangeAtPoint:", middle(place)) });
        let all = reader_range(reader, 0, PARAGRAPH.len() as isize);
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

    // ---- notifications ----

    /// The page of notifications: one sent, and the click heard.
    #[derive(Debug, Default)]
    struct Notices {
        send: bool,
        sent: Option<Notification>,
        clicked: Option<Notification>,
    }

    impl Page for Notices {
        const NOTE: &'static str = "notices.txt";
        const STEPS: &'static [&'static str] = &["a notification is sent", "a click on it is heard", ""];

        fn describe(&mut self, ui: &mut Ui) {
            ui.label("Notifications");
            if std::mem::take(&mut self.send) {
                self.sent = Some(ui.system_notification("Saved", "notes.txt is on the disk", None));
            }
            self.clicked = ui.notification_clicked().or(self.clicked);
        }

        fn act(&mut self, _: &mut Script, name: &str, _: &mut Ui) {
            let centre = || unsafe { send0::<Obj>(class(c"UNUserNotificationCenter"), c"currentNotificationCenter") };
            match name {
                "a notification is sent" => self.send = true,
                "a click on it is heard" => unsafe {
                    let delegate: Obj = send0(centre(), c"delegate");
                    let heard = c"userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:";
                    send3::<(), _, _, _>(delegate, heard, centre(), click_on_first_notification(), std::ptr::null_mut::<c_void>());
                },
                _ => {}
            }
        }

        fn check(&mut self, name: &str, _: &Ui) -> bool {
            let app_delegate = || unsafe { send0::<Obj>(send0::<Obj>(class(c"UIApplication"), c"sharedApplication"), c"delegate") };
            let centre_delegate = || unsafe { send0::<Obj>(send0::<Obj>(class(c"UNUserNotificationCenter"), c"currentNotificationCenter"), c"delegate") };
            match name {
                "a notification is sent" => self.sent.is_some() && !class(c"UNUserNotificationCenter").is_null() && centre_delegate() == app_delegate(),
                "a click on it is heard" => self.clicked.is_some() && self.clicked == self.sent,
                _ => true,
            }
        }
    }

    // ---- second windows (iPad) ----

    /// The page of second windows: one opened in a scene of its own, pressed, closed by the
    /// person (its app hears it), opened again, then closed by the app.
    #[derive(Debug, Default)]
    struct Windows {
        titled: bool,
        second: taut::SecondWindow,
        presses: u32,
        close_requests: u32,
    }

    impl Page for Windows {
        const NOTE: &'static str = "windows.txt";
        const STEPS: &'static [&'static str] = &[
            "a second window opens in a scene of its own",
            "a touch on it reaches its interface",
            "it shows its own page",
            "the person closing its scene is a close request",
            "it opens again",
            "the app closes it",
            "",
        ];

        fn describe(&mut self, ui: &mut Ui) {
            if !std::mem::replace(&mut self.titled, true) {
                ui.set_title("First");
            }
            ui.label("First window");
            if ui.window(&mut self.second, "Second") {
                self.presses += ui.button("Press").clicked as u32;
                ui.label(&format!("Pressed {}", self.presses));
                self.close_requests += ui.close_requested() as u32;
                ui.close();
            }
        }

        fn act(&mut self, script: &mut Script, name: &str, ui: &mut Ui) {
            match name {
                "a second window opens in a scene of its own" | "it opens again" => self.second.open = true,
                "a touch on it reaches its interface" => {
                    let Some(id) = ui.windows().find(|(_, title, _)| *title == "Second").map(|(id, ..)| id) else { return };
                    let button = ui.in_window(id, |ui| ui.visible_access_nodes().iter().find(|node| node.role == Role::Button).map(|node| node.rect));
                    let (view, rect) = (scene_view("Second"), button.flatten().unwrap_or_default());
                    let at = origin_in(view);
                    script.hold_on(view, Point { x: at.x + f64::from(rect.x + rect.w / 2.0), y: at.y + f64::from(rect.y + rect.h / 2.0) }, 0, Duration::ZERO);
                }
                "the person closing its scene is a close request" => unsafe {
                    let session: Obj = send0(scene_named("Second"), c"session");
                    let app: Obj = send0(class(c"UIApplication"), c"sharedApplication");
                    let nothing = std::ptr::null_mut::<c_void>();
                    send3::<(), _, _, _>(app, c"requestSceneSessionDestruction:options:errorHandler:", session, nothing, nothing);
                },
                "the app closes it" => self.second.open = false,
                _ => {}
            }
        }

        fn check(&mut self, name: &str, _: &Ui) -> bool {
            match name {
                "a second window opens in a scene of its own" | "it opens again" => scenes().len() == 2 && is_of(scene_view("Second"), c"TautView") && scene_view("Second") != view(),
                "a touch on it reaches its interface" => self.presses == 1,
                "it shows its own page" => unsafe {
                    let page: Obj = send0(send0::<Obj>(send0::<Obj>(scene_view("Second"), c"layer"), c"sublayers"), c"firstObject");
                    !send0::<Obj>(page, c"contents").is_null()
                },
                "the person closing its scene is a close request" => self.close_requests == 1 && !self.second.open && scenes().len() == 1,
                "the app closes it" => scenes().len() == 1,
                _ => true,
            }
        }
    }

    /// A dialog window on an iPhone, which shows one scene: it never shows, and keeps nothing
    /// from input.
    #[derive(Debug, Default)]
    struct PhoneDialog {
        dialog: taut::SecondWindow,
        presses: u32,
    }

    impl Page for PhoneDialog {
        const NOTE: &'static str = "dialog.txt";
        const STEPS: &'static [&'static str] = &["a dialog is asked for", "a touch on the first window still reaches it", ""];

        fn describe(&mut self, ui: &mut Ui) {
            self.presses += ui.button("Press").clicked as u32;
            if ui.window(&mut self.dialog, "Dialog") {
                ui.label("A question");
                ui.close();
            }
        }

        fn act(&mut self, script: &mut Script, name: &str, ui: &mut Ui) {
            match name {
                "a dialog is asked for" => self.dialog = taut::SecondWindow { kind: taut::WindowKind::Dialog, open: true, ..Default::default() },
                "a touch on the first window still reaches it" => script.tap(middle_of(ui, Role::Button, "Press"), 0),
                _ => {}
            }
        }

        fn check(&mut self, name: &str, ui: &Ui) -> bool {
            match name {
                "a dialog is asked for" => ui.windows().count() == 1 && scenes().len() == 1,
                "a touch on the first window still reaches it" => self.presses == 1,
                _ => true,
            }
        }
    }

    /// The app's connected scenes.
    fn scenes() -> Vec<Obj> {
        items(unsafe { send0(send0::<Obj>(send0::<Obj>(class(c"UIApplication"), c"sharedApplication"), c"connectedScenes"), c"allObjects") })
    }
    /// A connected scene, by its title.
    fn scene_named(title: &str) -> Obj {
        scenes().into_iter().find(|scene| text_of(unsafe { send0(*scene, c"title") }) == title).unwrap_or(std::ptr::null_mut())
    }
    /// The library's view in a scene, by its title (null where there is none).
    fn scene_view(title: &str) -> Obj {
        let windows = items(unsafe { send0(scene_named(title), c"windows") });
        windows.first().map_or(std::ptr::null_mut(), |window| unsafe { send0(send0::<Obj>(*window, c"rootViewController"), c"view") })
    }
    /// Whether an object is of a class (false for null).
    fn is_of(object: Obj, name: &CStr) -> bool {
        !object.is_null() && unsafe { send1::<u8, _>(object, c"isKindOfClass:", class(name)) } != 0
    }
    /// Where the page lies in a view: its top left.
    fn origin_in(view: Obj) -> Point {
        let insets: Insets = unsafe { send0(view, c"safeAreaInsets") };
        Point { x: insets.left, y: insets.top }
    }
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
    /// The check app built with `gpu-draw`.
    const GPU_ID: &str = "dev.taut.check.gpu";

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

    /// This file built for the simulator with `features`, as the app `id` (in `folder`).
    fn check_app(features: &str, id: &str, folder: &str) -> Result<PathBuf, String> {
        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.args(["build", "--test", "ios", "--features", features, "--target", "aarch64-apple-ios-sim", "--message-format=json"]).env("CARGO_TARGET_DIR", built());
        let said = ios_bundle::ran(&mut cargo)?;
        let program = said.lines().filter(|line| line.contains("\"executable\":\"") && line.contains("\"name\":\"ios\"")).find_map(|line| line.split("\"executable\":\"").nth(1)?.split('"').next());
        let program = PathBuf::from(program.ok_or("no test program built")?);
        let app = ios_bundle::bundle(&program, "check", id, false, &built().join(folder)).map_err(|error| error.to_string())?;
        ios_bundle::sign_for_simulator(&app)?;
        Ok(app)
    }

    fn simctl(args: &[&str]) -> Result<String, String> {
        ios_bundle::ran(Command::new("xcrun").arg("simctl").args(args))
    }

    /// Starts an app on the device in a mode, its output in files.
    fn launch(device: &str, id: &str, mode: &str) -> Result<(), String> {
        launch_with(device, id, mode, &[])
    }

    /// Starts an app on the device in a mode, with variables (`TAUT_RENDERER`, …).
    fn launch_with(device: &str, id: &str, mode: &str, variables: &[(&str, &str)]) -> Result<(), String> {
        let logs = built().join(format!("{id}-{mode}"));
        let (out, err) = (format!("--stdout={}.out", logs.display()), format!("--stderr={}.err", logs.display()));
        let mut command = Command::new("xcrun");
        command.args(["simctl", "launch", "--terminate-running-process", &out, &err, device, id]).env("SIMCTL_CHILD_TAUT_IOS_CHECK", mode);
        command.envs(variables.iter().map(|(name, value)| (format!("SIMCTL_CHILD_{name}"), *value)));
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

    /// Runs the check app on a page it drives (`mode`), and gives back the steps that failed.
    fn driven(device: &str, mode: &str) -> Result<Vec<String>, String> {
        launch(device, CHECK_ID, mode)?;
        let said = note(device, CHECK_ID, &format!("{mode}.txt"), |text| text.contains("done"), Duration::from_secs(240));
        let said = said.unwrap_or_else(|error| error);
        println!("{mode}:\n{said}");
        Ok(said.lines().filter(|line| line.starts_with("FAILED") || line.contains("after")).map(str::to_owned).collect())
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

    /// What app `id` drew of its window itself, as the Mac reads it (`drawn.png`).
    fn drawn_by_app(device: &str, id: &str) -> Result<Surface, String> {
        let container = simctl(&["get_app_container", device, id, "data"])?;
        let png = std::fs::read(Path::new(container.trim()).join("tmp").join("taut-check").join("drawn.png")).map_err(|error| error.to_string())?;
        let (width, pixels) = taut::png::decode(&png).ok_or("the app drew nothing")?;
        Ok(Surface { width, height: pixels.len() / width.max(1), pixels })
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

    /// How much of the screen's page differs from a drawing: the share of pixels more than
    /// `apart` levels apart in some channel, and the largest difference.
    fn compared(screen: &(usize, Vec<u32>), page: &Page, here: &Surface, name: &str, apart: u32) -> (f64, u32) {
        let (width, pixels) = screen;
        let (left, top) = ((page.at[0] * page.scale).round() as usize, (page.at[1] * page.scale).round() as usize);
        let (mut differ, mut most, mut diff) = (0usize, 0u32, vec![0u32; here.width * here.height]);
        for y in 0..here.height {
            for x in 0..here.width {
                let theirs = pixels.get((top + y) * width + left + x).copied().unwrap_or(0);
                let ours = here.pixels[y * here.width + x];
                let worst = (0..3).map(|channel| ((theirs >> (channel * 8)) & 0xff).abs_diff((ours >> (channel * 8)) & 0xff)).max().unwrap_or(0);
                most = most.max(worst);
                differ += usize::from(worst > apart);
                diff[y * here.width + x] = if worst > apart { 0xffff_0000 } else { 0xff00_0000 | (((ours & 0x00ff_ffff) / 2) & 0x007f_7f7f) };
            }
        }
        if let Some(folder) = shots() {
            let solid: Vec<u32> = here.pixels.iter().map(|pixel| pixel | 0xff00_0000).collect();
            _ = std::fs::write(folder.join(format!("{name}-mac.png")), taut::png::encode_rgba(here.width, &solid).unwrap_or_default());
            _ = std::fs::write(folder.join(format!("{name}-diff.png")), taut::png::encode_rgba(here.width, &diff).unwrap_or_default());
        }
        (differ as f64 / (here.width * here.height).max(1) as f64, most)
    }

    /// The library's GPU checks built for the simulator with `features` and run there
    /// (`gpu_ios`, `gpu_metal`, `gpu_draw_metal`, `layer_metal`); whether they passed.
    fn gpu_library(device: &str, features: &str) -> Result<bool, String> {
        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.args(["test", "--lib", "--no-run", "--features", features, "--target", "aarch64-apple-ios-sim", "--message-format=json"]).env("CARGO_TARGET_DIR", built().join("lib"));
        let said = ios_bundle::ran(&mut cargo)?;
        let program = said.lines().filter(|line| line.contains("\"test\":true") && line.contains("\"executable\":\"")).find_map(|line| line.split("\"executable\":\"").nth(1)?.split('"').next());
        let program = program.ok_or("no library tests built")?;
        let mut spawn = Command::new("xcrun");
        spawn.args(["simctl", "spawn", device, program, "gpu_ios", "gpu_metal", "gpu_draw_metal", "layer_metal", "--test-threads=1", "--nocapture"]);
        let done = spawn.output().map_err(|error| error.to_string())?;
        let ran = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
        println!("the library's GPU checks with {features}, in the simulator:\n{ran}");
        Ok(done.status.success() && ran.contains("test result: ok.") && !ran.contains(" 0 passed"))
    }

    /// The screen turned upright for a page lying sideways (the simulator's pictures keep the
    /// device's way up): which of the two quarter turns matches `here` best.
    fn upright(shot: (usize, Vec<u32>), page: &Page, here: &Surface) -> (usize, Vec<u32>) {
        let (width, pixels) = shot;
        let height = pixels.len() / width.max(1);
        if (page.size.w > page.size.h) == (width > height) {
            return (width, pixels);
        }
        let turned = |clockwise: bool| -> (usize, Vec<u32>) {
            let at = |x: usize, y: usize| if clockwise { pixels[(height - 1 - x) * width + y] } else { pixels[x * width + width - 1 - y] };
            (height, (0..width * height).map(|i| at(i % height, i / height)).collect())
        };
        let [one, other] = [turned(true), turned(false)];
        if compared(&one, page, here, "turned-a", 32).0 <= compared(&other, page, here, "turned-b", 32).0 { one } else { other }
    }

    /// The gallery shown by Metal in the check app built with `gpu-draw`: only shown (`TAUT_DRAW
    /// cpu`), then drawn too (`gpu`); its screen held against the app's own drawing by the CPU
    /// (the same pixels, or within a few levels where the GPU draws), first, after the
    /// background and back, and turned sideways. And the library's GPU checks in the simulator.
    fn gpu_checks(device: &str) -> Result<Vec<String>, String> {
        let mut failed = Vec::new();
        for features in ["gpu", "gpu-draw"] {
            failed.extend((!gpu_library(device, features)?).then(|| format!("the library's GPU checks with {features} failed in the simulator")));
        }
        let app = check_app("touch,gpu-draw", GPU_ID, "ios-gpu")?;
        simctl(&["install", device, &app.to_string_lossy()])?;
        for (draw, tolerance) in [("cpu", 0), ("gpu", 4)] {
            launch_with(device, GPU_ID, "gallery", &[("TAUT_RENDERER", "metal"), ("TAUT_DRAW", draw)])?;
            let ready = note(device, GPU_ID, "ready.txt", |text| text.ends_with('\n'), Duration::from_secs(120))?;
            if !ready.contains("Metal") {
                failed.push(format!("drawing on the {draw}: Metal does not show the gallery: {ready:?}"));
                continue;
            }
            let page = page_of(&ready)?;
            let mut check = |shot: (usize, Vec<u32>), page: &Page, here: &Surface, name: &str| {
                let (share, most) = compared(&shot, page, here, name, tolerance);
                println!("{name}: {:.3}% of pixels more than {tolerance} apart from the CPU's, at most {most}", share * 100.0);
                if share > 0.001 {
                    failed.push(format!("{name}: {:.3}% of pixels differ from the CPU's", share * 100.0));
                }
            };
            let phone = drawn_by_app(device, GPU_ID)?;
            check(screen(device, &format!("metal-{draw}"))?, &page, &phone, &format!("metal-{draw}"));
            simctl(&["launch", device, "com.apple.Preferences"])?;
            std::thread::sleep(Duration::from_secs(4));
            simctl(&["launch", device, GPU_ID])?;
            std::thread::sleep(Duration::from_secs(4));
            check(screen(device, &format!("metal-{draw}-back"))?, &page, &phone, &format!("metal-{draw}, back from the background"));
            signal(device, GPU_ID, "turn")?;
            let turned = note(device, GPU_ID, "turned.txt", |text| text.ends_with('\n'), Duration::from_secs(60))?;
            let (page, phone) = (page_of(&turned)?, drawn_by_app(device, GPU_ID)?);
            let shot = upright(screen(device, &format!("metal-{draw}-turned"))?, &page, &phone);
            check(shot, &page, &phone, &format!("metal-{draw}, turned"));
            _ = simctl(&["terminate", device, GPU_ID]);
        }
        Ok(failed)
    }

    /// Second windows on the newest iPad simulator, each a scene of its own.
    fn ipad_windows(check: &Path) -> Result<Vec<String>, String> {
        let device = ios_bundle::newest_simulator("iPad")?;
        println!("simulator {device}");
        ios_bundle::boot(&device)?;
        simctl(&["install", &device, &check.to_string_lossy()])?;
        let failed = driven(&device, "windows")?;
        _ = simctl(&["terminate", &device, CHECK_ID]);
        Ok(failed)
    }

    /// Everything, in order; the failures.
    fn checks() -> Result<Vec<String>, String> {
        let mut failed = Vec::new();
        let check = check_app("touch", CHECK_ID, "ios")?;
        let asked = ios_bundle::Asked { example: "gallery".into(), id: "dev.taut.gallery".into(), ..Default::default() };
        let gallery = ios_bundle::build(&asked, &built())?;
        let device = ios_bundle::newest_simulator("iPhone")?;
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
        let (share, most) = compared(&shown, &page, &here, "check-gallery", 32);
        println!("the screen against the Mac's drawing: {:.2}% of pixels apart, at most {most}", share * 100.0);
        if share > 0.03 {
            failed.push(format!("the screen differs from the Mac's drawing in {:.2}% of its pixels", share * 100.0));
        }
        // The same against the app's own drawing, made on the phone: the screen shows its pixels
        // as they are, where they belong.
        let phone = drawn_by_app(&device, CHECK_ID)?;
        let (share, most) = compared(&shown, &page, &phone, "check-gallery-phone", 32);
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

        // What a reader reads of lines, letters and a long table; the parts and the system's
        // parts, driven.
        failed.extend(driven(&device, "reader")?);
        failed.extend(driven(&device, "parts")?);
        failed.extend(driven(&device, "system")?);

        // The gallery example itself.
        launch(&device, "dev.taut.gallery", "")?;
        std::thread::sleep(Duration::from_secs(6));
        let shown = screen(&device, "gallery")?;
        let (share, most) = compared(&shown, &page, &here, "gallery", 32);
        println!("the gallery example against the Mac's drawing: {:.2}% of pixels apart, at most {most}", share * 100.0);
        if share > 0.03 {
            failed.push(format!("the gallery example differs from the Mac's drawing in {:.2}% of its pixels", share * 100.0));
        }
        // The GPU; then notifications last: the system asks the person's leave over the app (and
        // dims the screen while it asks).
        failed.extend(gpu_checks(&device)?);
        failed.extend(driven(&device, "notices")?);
        failed.extend(driven(&device, "dialog")?);
        failed.extend(ipad_windows(&check)?);
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
