//! The macOS windows, on a Mac: real windows, driven by events posted to AppKit's queue as the
//! system posts a person's, their pixels read back from the window and held against the same
//! interface drawn afresh. Run without libtest's harness (`harness = false` in `Cargo.toml`):
//! AppKit opens windows only on a program's main thread, where libtest runs no test.
//!
//! `TAUT_SHOTS` names a folder for pictures of the windows (as the window shows them, and as
//! the screen does), to look at afterwards.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char, c_void};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use taut::geom::{Point, Size, pt};
    use taut::{Event, FileDialog, LocalClipboard, Mods, Rect, SecondWindow, Surface, Ui, Window};

    type Obj = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct NSSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct NSRange {
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
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        static NSPasteboardTypeString: Obj;
        static NSPasteboardTypeFileURL: Obj;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(image: Obj) -> usize;
        fn CGImageGetHeight(image: Obj) -> usize;
        fn CGImageGetBytesPerRow(image: Obj) -> usize;
        fn CGImageGetAlphaInfo(image: Obj) -> u32;
        fn CGImageGetDataProvider(image: Obj) -> Obj;
        fn CGDataProviderCopyData(provider: Obj) -> Obj;
        fn CGImageRelease(image: Obj);
        fn CGColorSpaceCreateDeviceRGB() -> Obj;
        fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bits: usize, row: usize, space: Obj, info: u32) -> Obj;
        fn CGContextDrawImage(context: Obj, rect: NSRect, image: Obj);
        fn CGContextRelease(context: Obj);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetBytePtr(data: Obj) -> *const u8;
        fn CFDataGetLength(data: Obj) -> isize;
        fn CFRelease(object: Obj);
    }
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    trait Args {
        unsafe fn send<R>(self, to: Obj, name: *const c_void) -> R;
    }
    macro_rules! args {
        ($($arg:ident),*) => {
            impl<$($arg),*> Args for ($($arg,)*) {
                #[allow(non_snake_case)]
                unsafe fn send<R>(self, to: Obj, name: *const c_void) -> R {
                    let ($($arg,)*) = self;
                    let call = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Obj, *const c_void $(, $arg)*) -> R>(objc_msgSend) };
                    unsafe { call(to, name $(, $arg)*) }
                }
            }
        };
    }
    args!();
    args!(A);
    args!(A, B);
    args!(A, B, C);
    args!(A, B, C, D);
    args!(A, B, C, D, E, F, G, H, I);
    args!(A, B, C, D, E, F, G, H, I, J);

    fn class(name: &CStr) -> Obj {
        unsafe { objc_getClass(name.as_ptr()) }
    }
    fn sel(name: &CStr) -> *const c_void {
        unsafe { sel_registerName(name.as_ptr()) }
    }
    fn send<R>(to: Obj, name: &CStr, args: impl Args) -> R {
        unsafe { args.send(to, sel(name)) }
    }
    fn string(text: &str) -> Obj {
        let made: Obj = send(class(c"NSString"), c"alloc", ());
        let made: Obj = send(made, c"initWithBytes:length:encoding:", (text.as_ptr(), text.len(), 4usize));
        send(made, c"autorelease", ())
    }
    fn text_of(string: Obj) -> Option<String> {
        if string.is_null() {
            return None;
        }
        let bytes: *const c_char = send(string, c"UTF8String", ());
        Some(unsafe { CStr::from_ptr(bytes) }.to_string_lossy().into_owned())
    }
    fn array(objects: &[Obj]) -> Obj {
        send(class(c"NSArray"), c"arrayWithObjects:count:", (objects.as_ptr(), objects.len()))
    }
    fn items(array: Obj) -> Vec<Obj> {
        let count: usize = send(array, c"count", ());
        (0..count).map(|at| send(array, c"objectAtIndex:", (at,))).collect()
    }
    fn app() -> Obj {
        send(class(c"NSApplication"), c"sharedApplication", ())
    }
    fn rect_of(object: Obj, name: &CStr) -> NSRect {
        send(object, name, ())
    }

    /// The program's windows of the library's that show, in the order AppKit lists them.
    fn windows() -> Vec<Obj> {
        let all = items(send(app(), c"windows", ()));
        all.into_iter()
            .filter(|window| send::<u8>(*window, c"isVisible", ()) == 1)
            .filter(|window| text_of(send(send::<Obj>(send::<Obj>(*window, c"contentView", ()), c"className", ()), c"description", ())).as_deref() == Some("TautView"))
            .collect()
    }
    /// The window titled so.
    fn window_titled(title: &str) -> Option<Obj> {
        windows().into_iter().find(|window| text_of(send(*window, c"title", ())).as_deref() == Some(title))
    }
    fn view_of(window: Obj) -> Obj {
        send(window, c"contentView", ())
    }

    /// What a window's layer shows: its width, and its pixels, `0xAARRGGBB`.
    fn shown(window: Obj) -> Option<(usize, Vec<u32>, u32)> {
        let layer: Obj = send(view_of(window), c"layer", ());
        let image: Obj = send(layer, c"contents", ());
        if image.is_null() {
            return None;
        }
        unsafe {
            let (width, height, row) = (CGImageGetWidth(image), CGImageGetHeight(image), CGImageGetBytesPerRow(image));
            let data = CGDataProviderCopyData(CGImageGetDataProvider(image));
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize);
            let pixels = (0..height).flat_map(|y| (0..width).map(move |x| (y, x))).map(|(y, x)| u32::from_le_bytes(bytes[y * row + x * 4..y * row + x * 4 + 4].try_into().unwrap())).collect();
            CFRelease(data);
            Some((width, pixels, CGImageGetAlphaInfo(image)))
        }
    }

    /// What the screen shows of a window's content (see `window_picture`): the picture has the
    /// title bar on top, so only the rows of the view are kept.
    fn on_screen(window: Obj) -> Option<(usize, Vec<u32>)> {
        let (width, pixels) = window_picture(window)?;
        let height = pixels.len() / width.max(1);
        let view = rect_of(view_of(window), c"bounds").size.height;
        let scale: f64 = send(window, c"backingScaleFactor", ());
        let rows = ((view * scale).round() as usize).min(height);
        Some((width, pixels[(height - rows) * width..].to_vec()))
    }

    /// Saves a picture to the pictures folder, if there is one.
    fn save(name: &str, width: usize, pixels: &[u32]) {
        let Some(folder) = std::env::var_os("TAUT_SHOTS") else { return };
        let _ = std::fs::create_dir_all(&folder);
        let surface = Surface { width, height: pixels.len() / width.max(1), pixels: pixels.to_vec() };
        let _ = std::fs::write(PathBuf::from(folder).join(name), surface.to_png());
    }

    /// The same interface drawn afresh in a window of nobody's: the window's size and scale,
    /// the same events, the same app.
    fn afresh(size: Size, scale: f32, events: &[Event], mut describe: impl FnMut(&mut Ui)) -> Surface {
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
        ui.resize(size, scale);
        ui.set_time(Duration::from_secs(60));
        ui.run(&mut describe);
        for event in events {
            ui.event(event.clone());
            ui.run(&mut describe);
        }
        ui.set_time(Duration::from_secs(120));
        ui.run(&mut describe);
        Surface::draw(&mut ui, 64)
    }

    /// Where two pictures differ: how many pixels, and the largest difference of a channel.
    fn differ(a: &[u32], b: &[u32]) -> (usize, u32) {
        let channel = |a: u32, b: u32, shift: u32| ((a >> shift & 0xff) as i32 - (b >> shift & 0xff) as i32).unsigned_abs();
        let most = |a: u32, b: u32| [0, 8, 16].map(|shift| channel(a, b, shift)).into_iter().max().unwrap_or(0);
        let differing = a.iter().zip(b).filter(|(a, b)| (**a & 0xff_ffff) != (**b & 0xff_ffff));
        (differing.count(), a.iter().zip(b).map(|(a, b)| most(*a, *b)).max().unwrap_or(0))
    }

    fn uptime() -> f64 {
        send(send::<Obj>(class(c"NSProcessInfo"), c"processInfo", ()), c"systemUptime", ())
    }

    /// Posts a press or release of the left button at a place in a window's view (in units
    /// from its top left), as AppKit posts a person's.
    fn post_mouse(window: Obj, at: Point, kind: usize) {
        let height = rect_of(view_of(window), c"bounds").size.height;
        let place = NSPoint { x: f64::from(at.x), y: height - f64::from(at.y) };
        let number: isize = send(window, c"windowNumber", ());
        let pressure: f32 = if kind == 1 { 1.0 } else { 0.0 };
        let event: Obj = send(
            class(c"NSEvent"),
            c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
            (kind, place, 0usize, uptime(), number, std::ptr::null_mut::<c_void>(), 0isize, 1isize, pressure),
        );
        send::<()>(app(), c"postEvent:atStart:", (event, 0u8));
    }

    /// Posts a key going down (10) or up (11): its key code, what it types, its modifiers.
    fn post_key(window: Obj, code: u16, typed: &str, flags: usize, kind: usize) {
        let number: isize = send(window, c"windowNumber", ());
        let event: Obj = send(
            class(c"NSEvent"),
            c"keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
            (kind, NSPoint::default(), flags, uptime(), number, std::ptr::null_mut::<c_void>(), string(typed), string(typed), 0u8, code),
        );
        send::<()>(app(), c"postEvent:atStart:", (event, 0u8));
    }

    /// A check's state as its app runs: the step it is at and since when, and what went wrong.
    /// While `idle`, nothing but events wakes its window.
    struct Steps {
        step: u32,
        since: Instant,
        failed: Vec<String>,
        done: bool,
        idle: bool,
    }

    impl Steps {
        fn new() -> Self {
            Steps { step: 0, since: Instant::now(), failed: Vec::new(), done: false, idle: false }
        }
        /// On to the next step.
        fn next(&mut self) {
            (self.step, self.since) = (self.step + 1, Instant::now());
        }
        /// The step has waited this long, in ms.
        fn waited(&self, long: u64) -> bool {
            self.since.elapsed() >= Duration::from_millis(long)
        }
        /// Notes a failure where `ok` is false.
        fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
            if !ok {
                self.failed.push(what());
            }
        }
        /// Ends the check, as failed if a step waited past ten seconds; else has the window run
        /// again soon.
        fn keep_time(&mut self, ui: &mut Ui) {
            if self.since.elapsed() > Duration::from_secs(10) {
                self.failed.push(format!("step {} waited too long", self.step));
                self.done = true;
            }
            if self.done {
                ui.quit();
            } else if !self.idle {
                ui.wake_after(Duration::from_millis(50));
            }
        }
    }

    /// Runs a check's app in a window until it is done; gives back what failed.
    fn check(window: Window, mut app: impl FnMut(&mut Steps, &mut Ui) + 'static) -> Vec<String> {
        let steps = Rc::new(RefCell::new(Steps::new()));
        let kept = steps.clone();
        taut::run(window, move |ui| {
            let mut steps = kept.borrow_mut();
            app(&mut steps, ui);
            steps.keep_time(ui);
        });
        let mut steps = steps.borrow_mut();
        if !steps.done {
            steps.failed.push("the window closed before the check ended".into());
        }
        std::mem::take(&mut steps.failed)
    }

    /// The interface of the first check: a count and a button, and text in other scripts.
    fn counter(count: &mut u32, ui: &mut Ui) -> Rect {
        ui.column().pad(16.0).gap(10.0).open();
        ui.label(&format!("Pressed {count} times"));
        let button = ui.button("Press");
        *count += button.clicked as u32;
        ui.label("Ünïcödé — Ελληνικά — 日本語");
        ui.close();
        button.rect
    }

    /// Compares a window with its interface drawn afresh, and keeps pictures of both.
    fn compare(steps: &mut Steps, ui: &Ui, window: Obj, name: &str, events: &[Event], describe: impl FnMut(&mut Ui)) {
        let Some((width, pixels, alpha)) = shown(window) else { return steps.check(false, || format!("{name}: the window shows no picture")) };
        let fresh = afresh(ui.size(), ui.scale(), events, describe);
        save(&format!("{name}-window.png"), width, &pixels);
        save(&format!("{name}-afresh.png"), fresh.width, &fresh.pixels);
        let height = pixels.len() / width.max(1);
        steps.check((width, height) == (fresh.width, fresh.height), || format!("{name}: the window is {width} by {height}, afresh {} by {}", fresh.width, fresh.height));
        let (count, most) = differ(&pixels, &fresh.pixels);
        steps.check(count == 0, || format!("{name}: {count} pixels differ from the interface drawn afresh, by up to {most}"));
        steps.check(alpha == 6, || format!("{name}: a solid window's picture has alpha kind {alpha}, not 6"));
        match on_screen(window) {
            Some((width, screen)) => {
                save(&format!("{name}-screen.png"), width, &screen);
                let (count, most) = if screen.len() == fresh.pixels.len() { differ(&screen, &fresh.pixels) } else { (usize::MAX, 0) };
                println!("    {name}: the screen shows {width} by {}; {count} pixels differ from afresh, by up to {most}", screen.len() / width.max(1));
            }
            None => println!("    {name}: no picture of the screen to be had"),
        }
    }

    /// A window opens with what the library draws; a click posted as a person's presses its
    /// button, and the window then shows the same as the interface drawn afresh after it.
    fn a_window_shows_what_the_library_draws_and_a_click_redraws_it() -> Vec<String> {
        let mut count = 0;
        let window = Window { title: "Counter", width: 360.0, height: 200.0, ..Default::default() };
        check(window, move |steps, ui| {
            let button = counter(&mut count, ui);
            let middle = pt(button.x + button.w / 2.0, button.y + button.h / 2.0);
            let Some(window) = window_titled("Counter") else { return };
            match steps.step {
                0 if steps.waited(500) => {
                    compare(steps, ui, window, "first", &[], |ui| _ = counter(&mut 0, ui));
                    post_mouse(window, middle, 1);
                    post_mouse(window, middle, 2);
                    steps.next();
                }
                1 if count == 1 => steps.next(),
                2 if steps.waited(500) => {
                    let events = [Event::Move(middle), Event::Press(taut::Button::Left, Mods::NONE), Event::Release(taut::Button::Left)];
                    let mut fresh = 0;
                    compare(steps, ui, window, "clicked", &events, |ui| _ = counter(&mut fresh, ui));
                    steps.check(count == 1, || format!("one click counted {count}"));
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// The window's child windows that show: what an input method composes shows in one.
    fn shown_children(window: Obj) -> Vec<Obj> {
        items(send(window, c"childWindows", ())).into_iter().filter(|child| send::<u8>(*child, c"isVisible", ()) == 1).collect()
    }

    /// What the screen shows of a window, through CoreGraphics' window picture where the system
    /// still has it: its width and pixels, as `shown` gives them.
    fn window_picture(window: Obj) -> Option<(usize, Vec<u32>)> {
        type Pictures = unsafe extern "C" fn(NSRect, u32, u32, u32) -> Obj;
        let found = unsafe { dlsym(-2isize as *mut c_void, c"CGWindowListCreateImage".as_ptr()) };
        if found.is_null() {
            return None;
        }
        let picture = unsafe { std::mem::transmute::<*mut c_void, Pictures>(found) };
        let number: isize = send(window, c"windowNumber", ());
        // The rectangle that is none (`CGRectNull`): the window's own bounds. Just this window
        // (8), without its frame (1), at its best resolution (8).
        let null = NSRect { origin: NSPoint { x: f64::INFINITY, y: f64::INFINITY }, size: NSSize::default() };
        let image = unsafe { picture(null, 8, number as u32, 1 | 8) };
        if image.is_null() {
            return None;
        }
        unsafe {
            let (width, height) = (CGImageGetWidth(image), CGImageGetHeight(image));
            let mut pixels = vec![0u32; width * height];
            let space = CGColorSpaceCreateDeviceRGB();
            let context = CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, width * 4, space, 2 << 12 | 2);
            CGContextDrawImage(context, NSRect { origin: NSPoint::default(), size: NSSize { width: width as f64, height: height as f64 } }, image);
            CGContextRelease(context);
            CFRelease(space);
            CGImageRelease(image);
            Some((width, pixels))
        }
    }

    /// Keys posted as a person's reach the focused text box, through the system's text input;
    /// an input method's text does too: what it composes shows at the text cursor until
    /// committed, and it is told where the text is.
    fn keys_and_an_input_methods_text_reach_the_focused_box() -> Vec<String> {
        let mut text = String::new();
        let window = Window { title: "Typing", width: 360.0, height: 160.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.column().pad(16.0).open();
            let typed = ui.text_box(&mut text, "Type here");
            ui.close();
            let Some(window) = window_titled("Typing") else { return };
            match steps.step {
                0 => {
                    ui.set_focus(typed.id);
                    send::<()>(window, c"makeKeyAndOrderFront:", (std::ptr::null_mut::<c_void>(),));
                    send::<()>(app(), c"activateIgnoringOtherApps:", (1u8,));
                    steps.next();
                }
                1 if steps.waited(300) => {
                    let key: Obj = send(app(), c"keyWindow", ());
                    steps.check(key == window, || "the window is not the key window: keys go elsewhere".into());
                    for (code, typed) in [(0x04, "h"), (0x22, "i"), (0x7b, "\u{f702}"), (0x07, "x")] {
                        post_key(window, code, typed, 0, 10);
                        post_key(window, code, typed, 0, 11);
                    }
                    steps.next();
                }
                2 if text == "hxi" || steps.waited(2000) => {
                    steps.check(text == "hxi", || format!("typed h, i, Left, x: the box holds {text:?}"));
                    let view = view_of(window);
                    let none = NSRange { location: usize::MAX >> 1, length: 0 };
                    send::<()>(view, c"setMarkedText:selectedRange:replacementRange:", (string("にほ"), NSRange { location: 2, length: 0 }, none));
                    send::<()>(view, c"setMarkedText:selectedRange:replacementRange:", (string("日本ご"), NSRange { location: 0, length: 2 }, none));
                    let marked: u8 = send(view, c"hasMarkedText", ());
                    steps.check(marked == 1, || "an input method's text being composed is not noted".into());
                    let (range, picked): (NSRange, NSRange) = (send(view, c"markedRange", ()), send(view, c"selectedRange", ()));
                    steps.check((range.location, range.length, picked.location, picked.length) == (0, 3, 0, 2), || format!("composing: marked {range:?}, selected {picked:?}"));
                    steps.next();
                }
                3 if steps.waited(300) => {
                    let view = view_of(window);
                    let none = NSRange { location: usize::MAX >> 1, length: 0 };
                    let children = shown_children(window);
                    steps.check(children.len() == 1, || format!("what is composed shows in {} windows of its own", children.len()));
                    steps.check(text == "hxi", || format!("what is composed is not yet in the box, which holds {text:?}"));
                    let place: NSRect = send(view, c"firstRectForCharacterRange:actualRange:", (NSRange::default(), std::ptr::null_mut::<c_void>()));
                    let frame = rect_of(window, c"frame");
                    let inside = place.origin.x >= frame.origin.x
                        && place.origin.x <= frame.origin.x + frame.size.width
                        && place.origin.y >= frame.origin.y
                        && place.origin.y <= frame.origin.y + frame.size.height;
                    steps.check(inside && place.size.height > 0.0, || format!("the input method is told the text is at {place:?}, outside the window at {frame:?}"));
                    if let Some(panel) = children.first().copied() {
                        // At the text cursor: its top where the cursor's is, its left edge there.
                        let shows = rect_of(panel, c"frame");
                        let top = (shows.origin.y + shows.size.height - (place.origin.y + place.size.height)).abs();
                        steps.check((shows.origin.x - place.origin.x).abs() <= 2.0 && top <= 2.0, || format!("what is composed shows at {shows:?}, the cursor is at {place:?}"));
                        match window_picture(panel) {
                            Some((width, pixels)) => {
                                save("composing.png", width, &pixels);
                                let dark = pixels.iter().filter(|pixel| (**pixel & 0xff) + (**pixel >> 8 & 0xff) + (**pixel >> 16 & 0xff) < 200).count();
                                steps.check(dark > 20, || format!("what is composed shows {dark} dark pixels"));
                            }
                            None => println!("    no picture of what is composed to be had"),
                        }
                    }
                    send::<()>(view, c"insertText:replacementRange:", (string("日本"), none));
                    steps.next();
                }
                4 if text.contains("日本") || steps.waited(2000) => {
                    steps.check(text == "hx日本i", || format!("an input method's text: the box holds {text:?}"));
                    let left = shown_children(window).len();
                    steps.check(left == 0, || format!("committed, what was composed still shows in {left} windows"));
                    let marked: u8 = send(view_of(window), c"hasMarkedText", ());
                    steps.check(marked == 0, || "committed, the text is still noted as being composed".into());
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// What the app copies is on the system's clipboard, and what is there the app pastes: text,
    /// a picture, files.
    fn the_clipboard_is_the_systems() -> Vec<String> {
        let window = Window { title: "Clipboard", width: 240.0, height: 120.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("The clipboard");
            if steps.step != 0 || !steps.waited(300) {
                return;
            }
            let board: Obj = send(class(c"NSPasteboard"), c"generalPasteboard", ());
            let (plain, file_url) = unsafe { (NSPasteboardTypeString, NSPasteboardTypeFileURL) };
            ui.copy("taut on a Mac");
            let there = text_of(send(board, c"stringForType:", (plain,)));
            steps.check(there.as_deref() == Some("taut on a Mac"), || format!("copied text, the system has {there:?}"));
            send::<isize>(board, c"clearContents", ());
            send::<u8>(board, c"setString:forType:", (string("from AppKit"), plain));
            let pasted = ui.clipboard();
            steps.check(pasted.as_deref() == Some("from AppKit"), || format!("the system's text, pasted: {pasted:?}"));
            let picture = vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0x80ff_ffff];
            ui.copy_picture(2, &picture);
            let back = ui.clipboard_picture();
            steps.check(back == Some((2, picture.clone())), || format!("a picture copied and pasted came back as {back:?}"));
            let files = vec![PathBuf::from("/tmp/taut one.txt"), PathBuf::from("/tmp/two.txt")];
            ui.copy_files(&files);
            let back = ui.clipboard_files();
            steps.check(back == files, || format!("files copied and pasted came back as {back:?}"));
            let listed = items(send(board, c"pasteboardItems", ()));
            let first = listed.first().and_then(|item| text_of(send(*item, c"stringForType:", (file_url,))));
            steps.check(first.as_deref() == Some("file:///tmp/taut%20one.txt"), || format!("the system has the first file as {first:?}"));
            steps.done = true;
        })
    }

    /// A second window opens with its title; its close button asks its app, which closes it.
    fn a_second_window_opens_and_its_close_button_closes_it() -> Vec<String> {
        let mut second = SecondWindow::new(300.0, 160.0);
        let window = Window { title: "First", width: 300.0, height: 160.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("The first window");
            if ui.window(&mut second, "Second") {
                ui.label("The second window");
                ui.close();
            }
            match steps.step {
                0 if steps.waited(500) => {
                    let Some(other) = window_titled("Second") else { return steps.check(false, || "no second window opened".into()) };
                    steps.check(windows().len() == 2, || format!("{} windows are open, not 2", windows().len()));
                    send::<()>(other, c"performClose:", (std::ptr::null_mut::<c_void>(),));
                    steps.next();
                }
                1 if !second.open && steps.waited(300) => {
                    steps.check(window_titled("Second").is_none(), || "the second window stays after its app closed it".into());
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// Six lines of text, to be drawn at any size.
    fn lines(ui: &mut Ui) {
        ui.column().pad(12.0).gap(6.0).open();
        (0..6).for_each(|line| _ = ui.label(&format!("Line {line} of a window that grows")));
        ui.close();
    }

    /// A window opens where its app says, and its app is told where it is, which screens there
    /// are, and whether the system is dark; resized, it is drawn whole at its new size.
    fn a_window_opens_where_asked_and_is_drawn_whole_at_a_new_size() -> Vec<String> {
        let window = Window { title: "Placed", width: 300.0, height: 180.0, position: Some((140, 160)), ..Default::default() };
        check(window, move |steps, ui| {
            lines(ui);
            let Some(window) = window_titled("Placed") else { return };
            match steps.step {
                0 if steps.waited(500) => {
                    let place = ui.window_placement();
                    steps.check(place.is_some_and(|place| place.position == (140, 160)), || format!("asked to open at (140, 160), it says {place:?}"));
                    let screens = ui.screens().to_vec();
                    steps.check(!screens.is_empty() && screens[0].scale >= 1.0, || format!("the screens: {screens:?}"));
                    let looks: Obj = send(app(), c"effectiveAppearance", ());
                    let dark = text_of(send(looks, c"name", ())).is_some_and(|name| name.contains("Dark"));
                    steps.check(ui.system_dark() == dark, || format!("the system is dark: {dark}; the app is told {}", ui.system_dark()));
                    send::<()>(window, c"setContentSize:", (NSSize { width: 420.0, height: 260.0 },));
                    steps.next();
                }
                1 if steps.waited(500) => {
                    steps.check(ui.size() == Size { w: 420.0, h: 260.0 }, || format!("resized to 420 by 260, it is {:?}", ui.size()));
                    compare(steps, ui, window, "resized", &[], lines);
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    thread_local! {
        /// What the stand-in drag says: its pasteboard and its place in the window.
        static DRAG: std::cell::Cell<(Obj, NSPoint)> = const { std::cell::Cell::new((std::ptr::null_mut(), NSPoint { x: 0.0, y: 0.0 })) };
    }
    extern "C" fn drag_board(_: Obj, _: *const c_void) -> Obj {
        DRAG.get().0
    }
    extern "C" fn drag_place(_: Obj, _: *const c_void) -> NSPoint {
        DRAG.get().1
    }
    extern "C" fn drag_mask(_: Obj, _: *const c_void) -> usize {
        1
    }

    /// A drag of files from another program, as AppKit hands one to a view: an object answering
    /// what the drag carries and where it is.
    fn stand_in_drag(files: &[&str], at: NSPoint) -> Obj {
        let mut made = class(c"TestDrag");
        if made.is_null() {
            unsafe {
                made = objc_allocateClassPair(class(c"NSObject"), c"TestDrag".as_ptr(), 0);
                class_addMethod(made, sel(c"draggingPasteboard"), drag_board as *const c_void, c"@@:".as_ptr());
                class_addMethod(made, sel(c"draggingLocation"), drag_place as *const c_void, c"{CGPoint=dd}@:".as_ptr());
                class_addMethod(made, sel(c"draggingSourceOperationMask"), drag_mask as *const c_void, c"Q@:".as_ptr());
                objc_registerClassPair(made);
            }
        }
        let board: Obj = send(class(c"NSPasteboard"), c"pasteboardWithUniqueName", ());
        let urls: Vec<Obj> = files.iter().map(|file| send(class(c"NSURL"), c"fileURLWithPath:", (string(file),))).collect();
        send::<isize>(board, c"clearContents", ());
        send::<u8>(board, c"writeObjects:", (array(&urls),));
        DRAG.set((board, at));
        send(send::<Obj>(made, c"alloc", ()), c"init", ())
    }

    /// Files dragged over a window and let go reach the box they were let go on.
    fn files_let_go_on_a_window_reach_its_app() -> Vec<String> {
        let mut got: Option<Vec<PathBuf>> = None;
        let mut hovered = false;
        let window = Window { title: "Drops", width: 300.0, height: 160.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.column().pad(16.0).open();
            let target = ui.button("Let files go here");
            ui.close();
            hovered |= ui.files_hovered();
            if let Some(files) = ui.files_dropped(&target) {
                got = Some(files);
            }
            let Some(window) = window_titled("Drops") else { return };
            let view = view_of(window);
            let height = rect_of(view, c"bounds").size.height;
            let at = NSPoint { x: f64::from(target.rect.x + target.rect.w / 2.0), y: height - f64::from(target.rect.y + target.rect.h / 2.0) };
            match steps.step {
                0 if steps.waited(300) => {
                    let drag = stand_in_drag(&["/tmp/dropped one.txt", "/tmp/two.txt"], at);
                    let taken: usize = send(view, c"draggingEntered:", (drag,));
                    steps.check(taken == 1, || format!("a drag of files is taken as {taken}, not as a copy"));
                    steps.next();
                }
                1 if steps.waited(200) => {
                    steps.check(hovered, || "the app did not hear files come over the window".into());
                    let drag = stand_in_drag(&["/tmp/dropped one.txt", "/tmp/two.txt"], at);
                    let done: u8 = send(view, c"performDragOperation:", (drag,));
                    steps.check(done == 1, || "the drop was refused".into());
                    steps.next();
                }
                2 if got.is_some() || steps.waited(2000) => {
                    let want = vec![PathBuf::from("/tmp/dropped one.txt"), PathBuf::from("/tmp/two.txt")];
                    steps.check(got.as_ref() == Some(&want), || format!("the box got {got:?}"));
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// The app asks for the system's file dialog: it comes as a sheet on the window, and what
    /// it is given back reaches the app.
    fn the_systems_file_dialog_comes_as_a_sheet_and_answers() -> Vec<String> {
        let mut files = FileDialog::native();
        let mut got: Option<PathBuf> = None;
        let name = format!("answer-{}.txt", std::process::id());
        let folder = std::env::temp_dir().canonicalize().unwrap_or(std::env::temp_dir());
        let want = folder.join(&name);
        let window = Window { title: "Dialog", width: 360.0, height: 200.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("Saving");
            if steps.step == 0 {
                files.save(&folder, &name);
                steps.next();
            }
            if let Some(path) = ui.file_dialog(&mut files) {
                got = Some(path);
            }
            let Some(window) = window_titled("Dialog") else { return };
            match steps.step {
                1 if steps.waited(1500) => {
                    let sheet: Obj = send(window, c"attachedSheet", ());
                    steps.check(!sheet.is_null(), || "no sheet on the window".into());
                    // (The panel runs in another process: `ok:` is not there; ending the sheet with
                    // OK is what its button does.)
                    send::<()>(window, c"endSheet:returnCode:", (sheet, 1isize));
                    steps.next();
                }
                2 if got.is_some() || steps.waited(3000) => {
                    let found = got.as_ref().map(|path| path.parent().and_then(|folder| folder.canonicalize().ok()).unwrap_or_default().join(path.file_name().unwrap_or_default()));
                    steps.check(found.as_ref() == Some(&want), || format!("saving as {want:?}, the app got {got:?}"));
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// A see-through window hands AppKit pixels with alpha; where nothing is drawn, none.
    fn a_see_through_window_is_clear_where_nothing_is_drawn() -> Vec<String> {
        let window = Window { title: "Clear", width: 200.0, height: 120.0, transparent: true, ..Default::default() };
        check(window, move |steps, ui| {
            ui.column().pad(30.0).open();
            ui.label("See through");
            ui.close();
            let Some(window) = window_titled("Clear") else { return };
            if steps.step == 0 && steps.waited(500) {
                let Some((width, pixels, alpha)) = shown(window) else { return steps.check(false, || "the window shows no picture".into()) };
                save("clear-window.png", width, &pixels);
                steps.check(alpha == 2, || format!("a see-through window's picture has alpha kind {alpha}, not 2"));
                steps.check(pixels.first().is_some_and(|corner| corner >> 24 == 0), || format!("its corner is {:08x}, not clear", pixels[0]));
                steps.check(pixels.iter().any(|pixel| pixel >> 24 == 0xff), || "nothing in it is solid".into());
                let opaque: u8 = send(window, c"isOpaque", ());
                steps.check(opaque == 0, || "AppKit takes the window as opaque".into());
                steps.done = true;
            }
        })
    }

    /// A window without a frame shows no title bar buttons.
    fn a_window_without_a_frame_has_no_buttons() -> Vec<String> {
        let window = Window { title: "Bare", width: 240.0, height: 140.0, frameless: true, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("No frame");
            let Some(window) = window_titled("Bare") else { return };
            if steps.step == 0 && steps.waited(300) {
                for button in 0..3usize {
                    let hidden: u8 = send(send::<Obj>(window, c"standardWindowButton:", (button,)), c"isHidden", ());
                    steps.check(hidden == 1, || format!("title bar button {button} shows"));
                }
                steps.done = true;
            }
        })
    }

    /// A waker on another thread has the window run, with nothing else to wake it.
    fn a_waker_on_another_thread_wakes_the_window() -> Vec<String> {
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let window = Window { title: "Woken", width: 200.0, height: 100.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("Waiting");
            match steps.step {
                0 if steps.waited(300) => {
                    steps.idle = true;
                    let woken = woken.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(300));
                        woken.store(true, std::sync::atomic::Ordering::SeqCst);
                        taut::Waker.wake();
                    });
                    steps.next();
                }
                1 if woken.load(std::sync::atomic::Ordering::SeqCst) => {
                    steps.idle = false;
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> Obj;
        fn AXUIElementCopyAttributeValue(element: Obj, attribute: Obj, value: *mut Obj) -> i32;
        fn AXUIElementCopyParameterizedAttributeValue(element: Obj, attribute: Obj, parameter: Obj, value: *mut Obj) -> i32;
        fn AXUIElementSetAttributeValue(element: Obj, attribute: Obj, value: Obj) -> i32;
        fn AXValueCreate(kind: u32, value: *const c_void) -> Obj;
        fn AXValueGetValue(value: Obj, kind: u32, into: *mut c_void) -> u8;
    }

    /// `kAXValueCFRangeType`.
    const RANGE_VALUE: u32 = 4;

    /// An attribute of an element as a client reads it.
    fn ask(element: Obj, name: &str) -> Option<Obj> {
        let mut value: Obj = std::ptr::null_mut();
        let failed = unsafe { AXUIElementCopyAttributeValue(element, string(name), &mut value) };
        (failed == 0 && !value.is_null()).then_some(value)
    }
    /// The same, given something (a range, a place).
    fn ask_of(element: Obj, name: &str, given: Obj) -> Option<Obj> {
        let mut value: Obj = std::ptr::null_mut();
        let failed = unsafe { AXUIElementCopyParameterizedAttributeValue(element, string(name), given, &mut value) };
        (failed == 0 && !value.is_null()).then_some(value)
    }
    fn number_of(value: Option<Obj>) -> Option<i64> {
        value.map(|value| send::<i64>(value, c"longLongValue", ()))
    }
    fn range_value(location: isize, length: isize) -> Obj {
        unsafe { AXValueCreate(RANGE_VALUE, [location, length].as_ptr().cast()) }
    }
    fn range_of(value: Option<Obj>) -> Option<[isize; 2]> {
        let mut range = [0isize; 2];
        let read = value.is_some_and(|value| unsafe { AXValueGetValue(value, RANGE_VALUE, range.as_mut_ptr().cast()) } != 0);
        read.then_some(range)
    }

    /// Every element under one, as a client reads them, with its role and title (or value).
    fn all_under(element: Obj, depth: usize, into: &mut Vec<(Obj, String, String)>) {
        let text = |name: &str| ask(element, name).and_then(|value| text_of(send(value, c"description", ()))).unwrap_or_default();
        into.push((element, text("AXRole"), [text("AXTitle"), text("AXDescription")].concat()));
        if let Some(children) = ask(element, "AXChildren").filter(|_| depth < 10) {
            items(children).into_iter().for_each(|child| all_under(child, depth + 1, into));
        }
    }

    /// What an element tells, as a reader hears it: role and name, then its children's, in
    /// order, through the view's own answers (what the system passes on).
    fn told(element: Obj, depth: usize, into: &mut Vec<(String, String)>) {
        let role = text_of(send(element, c"accessibilityRole", ())).unwrap_or_default();
        let name = text_of(send(element, c"accessibilityLabel", ())).unwrap_or_default();
        into.push((role, name));
        if depth < 8 {
            items(send(element, c"accessibilityChildren", ())).into_iter().for_each(|child| told(child, depth + 1, into));
        }
    }

    /// The same through the system's client interface, as another program reads it: roles and
    /// titles or descriptions of the program's first window and all in it.
    fn told_to_clients(element: Obj, depth: usize, into: &mut Vec<(String, String)>) {
        let attribute = |name: &str| {
            let mut value: Obj = std::ptr::null_mut();
            let failed = unsafe { AXUIElementCopyAttributeValue(element, string(name), &mut value) };
            (failed == 0).then_some(value).filter(|value| !value.is_null())
        };
        let text = |name: &str| attribute(name).and_then(|value| text_of(send(value, c"description", ()))).unwrap_or_default();
        into.push((text("AXRole"), [text("AXTitle"), text("AXDescription")].concat()));
        if let Some(children) = attribute("AXChildren").filter(|_| depth < 8) {
            items(children).into_iter().for_each(|child| told_to_clients(child, depth + 1, into));
        }
    }

    /// The program's first window as a client reads it: `None` where the system does not trust
    /// this program to read others. (Asked of this program, the client interface answers on the
    /// asking thread, and AppKit answers only on the window's: so it is asked here.)
    fn read_as_a_client() -> Option<Vec<(String, String)>> {
        if unsafe { AXIsProcessTrusted() } == 0 {
            return None;
        }
        let program = unsafe { AXUIElementCreateApplication(std::process::id() as i32) };
        let mut windows: Obj = std::ptr::null_mut();
        unsafe { AXUIElementCopyAttributeValue(program, string("AXWindows"), &mut windows) };
        let mut found = Vec::new();
        items(windows).first().into_iter().for_each(|window| told_to_clients(*window, 0, &mut found));
        Some(found)
    }

    /// A screen reader reads what the window tells, through NSAccessibility: the text and the
    /// button, where they are; it presses the button, and hears the new count.
    fn a_reader_reads_the_window_and_presses_its_button() -> Vec<String> {
        let mut count = 0;
        let window = Window { title: "Readers", width: 360.0, height: 200.0, ..Default::default() };
        check(window, move |steps, ui| {
            counter(&mut count, ui);
            let Some(window) = window_titled("Readers") else { return };
            let view = view_of(window);
            let mut all = Vec::new();
            match steps.step {
                0 if steps.waited(300) => {
                    told(view, 0, &mut all);
                    steps.next();
                }
                1 if steps.waited(500) => {
                    told(view, 0, &mut all);
                    let has = |role: &str, name: &str| all.iter().any(|(of, called)| of == role && called == name);
                    steps.check(has("AXStaticText", "Pressed 0 times") && has("AXButton", "Press"), || format!("a reader is told {all:?}"));
                    let elements = items(send(view, c"accessibilityChildren", ()));
                    let button = elements.iter().copied().find(|element| text_of(send(*element, c"accessibilityLabel", ())).as_deref() == Some("Press"));
                    let Some(button) = button else { return steps.check(false, || "no button element".into()) };
                    let place: NSRect = send(button, c"accessibilityFrame", ());
                    let frame = rect_of(window, c"frame");
                    let inside = place.origin.x >= frame.origin.x && place.origin.y >= frame.origin.y && place.origin.y + place.size.height <= frame.origin.y + frame.size.height;
                    steps.check(inside && place.size.width > 0.0, || format!("the button is said to be at {place:?}, the window at {frame:?}"));
                    let pressed: u8 = send(button, c"accessibilityPerformPress", ());
                    steps.check(pressed == 1, || "the button would not be pressed".into());
                    steps.next();
                }
                2 if count == 1 && steps.waited(300) => {
                    told(view, 0, &mut all);
                    steps.check(all.iter().any(|(_, name)| name == "Pressed 1 times"), || format!("pressed by a reader, the window tells {all:?}"));
                    steps.next();
                }
                3 => {
                    match read_as_a_client() {
                        None => println!("    the system does not trust this program to read others: not read as a client"),
                        Some(found) => {
                            println!("    a client reads {} elements", found.len());
                            steps.check(found.iter().any(|(role, name)| role == "AXButton" && name.contains("Press")), || format!("a client reads {found:?}"));
                        }
                    }
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// The window of the next check: a text box with a misspelling, a table of three rows (the
    /// second selected), and a tree of a folder (with one thing in it) and a file.
    fn structures(text: &mut String, table: &mut taut::Table, tree: &mut taut::TreeView, ui: &mut Ui) {
        ui.column().pad(10.0).gap(8.0).grow().open();
        ui.check_spelling(Some("en-US"));
        ui.text_box(text, "Words");
        let columns = [taut::Column::new("Name").px(120.0), taut::Column::new("Size").px(80.0)];
        ui.table(table, &columns);
        for row in ui.rows(3, 22.0) {
            ui.selectable_row(row, row == 1);
            ui.cell(["one", "two", "three"][row]);
            ui.cell(["1 KB", "2 KB", "3 KB"][row]);
            ui.close();
        }
        ui.close();
        ui.close();
        let under = |of: Option<usize>| match of {
            None => vec![taut::TreeItem::with_children(1, "Folder"), taut::TreeItem::new(2, "File")],
            Some(1) => vec![taut::TreeItem::new(3, "Inner")],
            Some(_) => Vec::new(),
        };
        for row in ui.tree_view(tree, 20.0, under) {
            ui.tree_row(tree, row);
            ui.close();
        }
        ui.close();
        ui.close();
    }

    /// A client reads a text box by ranges (its length, selection, pieces, a misspelling) and
    /// selects in it; reads a table by rows, columns, headings and cells; and a tree by levels,
    /// opening a folder in it.
    fn a_reader_reads_text_by_ranges_and_tables_and_trees_by_rows() -> Vec<String> {
        let (mut text, mut table, mut tree) = ("Hello wrold".to_string(), taut::Table::default(), taut::TreeView::default());
        let window = Window { title: "Structures", width: 420.0, height: 420.0, ..Default::default() };
        check(window, move |steps, ui| {
            structures(&mut text, &mut table, &mut tree, ui);
            let Some(window) = window_titled("Structures") else { return };
            let all = || {
                let program = unsafe { AXUIElementCreateApplication(std::process::id() as i32) };
                let windows = ask(program, "AXWindows").map(items).unwrap_or_default();
                let mut found = Vec::new();
                let ours = windows.into_iter().find(|one| ask(*one, "AXTitle").and_then(text_of).as_deref() == Some("Structures"));
                ours.into_iter().for_each(|one| all_under(one, 0, &mut found));
                found
            };
            let find = |found: &[(Obj, String, String)], role: &str| found.iter().find(|(_, of, _)| of == role).map(|(element, ..)| *element);
            match steps.step {
                0 if steps.waited(300) => {
                    if unsafe { AXIsProcessTrusted() } == 0 {
                        println!("    the system does not trust this program to read others: not read");
                        steps.done = true;
                        return;
                    }
                    told(view_of(window), 0, &mut Vec::new());
                    steps.next();
                }
                // (Spelling is checked on a thread of its own: its marks come a little later.)
                1 if steps.waited(1500) => {
                    let found = all();
                    let Some(field) = find(&found, "AXTextField") else { return steps.check(false, || format!("no text field among {} elements", found.len())) };
                    let count = number_of(ask(field, "AXNumberOfCharacters"));
                    steps.check(count == Some(11), || format!("the text box has {count:?} characters"));
                    let piece = ask_of(field, "AXStringForRange", range_value(0, 5)).and_then(text_of);
                    steps.check(piece.as_deref() == Some("Hello"), || format!("its first five characters are {piece:?}"));
                    let marked = ask_of(field, "AXAttributedStringForRange", range_value(0, 11));
                    let wrong = marked.map(|marked| send::<Obj>(marked, c"attribute:atIndex:effectiveRange:", (string("AXMarkedMisspelled"), 7usize, std::ptr::null_mut::<c_void>())));
                    steps.check(wrong.is_some_and(|wrong| !wrong.is_null()), || "the misspelled word is not marked so".into());
                    let set = unsafe { AXUIElementSetAttributeValue(field, string("AXSelectedTextRange"), range_value(6, 5)) };
                    steps.check(set == 0, || format!("setting the selection failed: {set}"));
                    let table = find(&found, "AXTable");
                    let counts = table.map(|table| (number_of(ask(table, "AXRowCount")), number_of(ask(table, "AXColumnCount"))));
                    steps.check(counts == Some((Some(3), Some(2))), || format!("the table has rows and columns {counts:?}"));
                    let rows = table.and_then(|table| ask(table, "AXRows")).map(items).unwrap_or_default();
                    let picked = table.and_then(|table| ask(table, "AXSelectedRows")).map(items).unwrap_or_default();
                    let heads = table.and_then(|table| ask(table, "AXColumnHeaderUIElements")).map(items).unwrap_or_default();
                    steps.check((rows.len(), picked.len(), heads.len()) == (3, 1, 2), || format!("rows {}, selected {}, headings {}", rows.len(), picked.len(), heads.len()));
                    steps.check(number_of(picked.first().and_then(|row| ask(*row, "AXIndex"))) == Some(1), || "the selected row is not the second".into());
                    let numbers: Vec<Obj> = [1i64, 2].iter().map(|n| send(class(c"NSNumber"), c"numberWithLongLong:", (*n,))).collect();
                    let cell = table.and_then(|table| ask_of(table, "AXCellForColumnAndRow", array(&numbers)));
                    let cell_row = range_of(cell.and_then(|cell| ask(cell, "AXRowIndexRange")));
                    steps.check(cell_row == Some([2, 1]), || format!("the cell at column 1, row 2 says it is in rows {cell_row:?}"));
                    let outline = find(&found, "AXOutline");
                    let items_now = outline.and_then(|outline| ask(outline, "AXRows")).map(items).unwrap_or_default();
                    steps.check(items_now.len() == 2, || format!("the tree has {} rows", items_now.len()));
                    if let Some(folder) = items_now.first() {
                        let (level, open) = (number_of(ask(*folder, "AXDisclosureLevel")), number_of(ask(*folder, "AXDisclosing")));
                        steps.check((level, open) == (Some(0), Some(0)), || format!("the folder is at level {level:?}, open {open:?}"));
                        let yes: Obj = send(class(c"NSNumber"), c"numberWithBool:", (1u8,));
                        let opened = unsafe { AXUIElementSetAttributeValue(*folder, string("AXDisclosing"), yes) };
                        steps.check(opened == 0, || format!("opening the folder failed: {opened}"));
                    }
                    steps.next();
                }
                2 if steps.waited(500) => {
                    let found = all();
                    if let Some(field) = find(&found, "AXTextField") {
                        let picked = range_of(ask(field, "AXSelectedTextRange"));
                        let said = ask(field, "AXSelectedText").and_then(text_of);
                        steps.check(picked == Some([6, 5]) && said.as_deref() == Some("wrold"), || format!("selected by a reader: {picked:?}, {said:?}"));
                    }
                    let outline = find(&found, "AXOutline");
                    let items_now = outline.and_then(|outline| ask(outline, "AXRows")).map(items).unwrap_or_default();
                    steps.check(items_now.len() == 3, || format!("opened, the tree has {} rows", items_now.len()));
                    if let [folder, inner, ..] = items_now[..] {
                        let level = number_of(ask(inner, "AXDisclosureLevel"));
                        let by = ask(inner, "AXDisclosedByRow");
                        let under = ask(folder, "AXDisclosedRows").map(items).unwrap_or_default();
                        let same = |a: Obj, b: Obj| send::<u8>(a, c"isEqual:", (b,)) == 1;
                        steps.check(level == Some(1) && by.is_some_and(|by| same(by, folder)), || format!("the inner item is at level {level:?}, under {by:?}"));
                        steps.check(under.len() == 1 && same(under[0], inner), || format!("the folder holds {} items", under.len()));
                    }
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// A view and all within it, as AppKit has them, that is of a class.
    fn views_of(view: Obj, class_name: &str, depth: usize, into: &mut Vec<Obj>) {
        if text_of(send(send::<Obj>(view, c"className", ()), c"description", ())).as_deref() == Some(class_name) {
            into.push(view);
        }
        if depth < 6 {
            items(send(view, c"subviews", ())).into_iter().for_each(|under| views_of(under, class_name, depth + 1, into));
        }
    }

    /// The program's status item buttons, in the menu bar.
    fn status_buttons() -> Vec<Obj> {
        let mut found = Vec::new();
        for window in items(send(app(), c"windows", ())).into_iter().filter(|window| send::<u8>(*window, c"isVisible", ()) == 1) {
            views_of(send(window, c"contentView", ()), "NSStatusBarButton", 0, &mut found);
        }
        found
    }

    /// The tray icon is a status item in the menu bar: its picture and hint as the app says, a
    /// click heard as one, its menu's rows picked; gone when the app stops saying it.
    fn the_tray_icon_is_a_status_item_with_a_menu() -> Vec<String> {
        let (mut heard, mut quiet, mut shown) = (Vec::new(), false, true);
        let icon: Vec<u32> = vec![0xff20_60e0; 16 * 16];
        let window = Window { title: "Tray", width: 240.0, height: 120.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("A tray icon");
            if shown {
                if ui.tray(&[(16, &icon)], "Taut in the tray").clicked {
                    heard.push("clicked");
                }
                if ui.tray_item("&Show") {
                    heard.push("show");
                }
                ui.tray_checkbox(&mut quiet, "&Quiet");
            }
            match steps.step {
                0 if steps.waited(500) => {
                    let buttons = status_buttons();
                    let Some(button) = buttons.first().copied() else { return steps.check(false, || "no status item in the menu bar".into()) };
                    let hint = text_of(send(button, c"toolTip", ()));
                    steps.check(hint.as_deref() == Some("Taut in the tray"), || format!("its hint is {hint:?}"));
                    steps.check(!send::<Obj>(button, c"image", ()).is_null(), || "it has no picture".into());
                    let menu: Obj = send(button, c"menu", ());
                    let rows: Vec<String> = items(send(menu, c"itemArray", ())).into_iter().filter_map(|item| text_of(send(item, c"title", ()))).collect();
                    steps.check(rows == ["Show", "Quiet"], || format!("its menu has {rows:?}"));
                    // As a person's: between runs, from AppKit's loop (what a run does not take
                    // is dropped).
                    let later = |to: Obj, name: &CStr, with: Obj| send::<()>(to, c"performSelector:withObject:afterDelay:", (sel(name), with, 0.0f64));
                    later(button, c"performClick:", std::ptr::null_mut());
                    let rows = items(send(menu, c"itemArray", ()));
                    rows.iter().for_each(|row| later(send(*row, c"target", ()), c"trayPicked:", *row));
                    steps.next();
                }
                1 if steps.waited(500) => {
                    steps.check(heard == ["clicked", "show"] && quiet, || format!("clicked and picked, the app heard {heard:?}, quiet: {quiet}"));
                    shown = false;
                    steps.next();
                }
                2 if steps.waited(500) => {
                    steps.check(status_buttons().is_empty(), || "the status item stays after the app stopped saying it".into());
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(source: Obj, code: u16, down: bool) -> Obj;
        fn CGEventSetFlags(event: Obj, flags: u64);
        fn CGEventPost(tap: u32, event: Obj);
        fn CGEventCreateMouseEvent(source: Obj, kind: u32, at: NSPoint, button: u32) -> Obj;
    }

    /// A global hotkey is registered with the system; where this program may post keys as a
    /// person's, a press of it is heard.
    fn a_global_hotkey_is_registered_and_heard() -> Vec<String> {
        let mut pressed = 0;
        let window = Window { title: "Hotkey", width: 240.0, height: 120.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("A hotkey");
            pressed += ui.hotkey("Ctrl+Alt+F9") as u32;
            pressed += ui.hotkey("Ctrl+Alt+K") as u32 * 10;
            match steps.step {
                0 if steps.waited(500) => {
                    steps.check(!ui.hotkey_refused("Ctrl+Alt+F9"), || "the system refused Ctrl+Alt+F9 (Command+Option+F9)".into());
                    // F9 (code 0x65) and K (0x28) with Command and Option, at the system's own input.
                    // (A function key comes with the Fn flag, as from a Mac's keyboard.)
                    for (code, down) in [(0x65, true), (0x65, false), (0x28, true), (0x28, false)] {
                        let key = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, down) };
                        let function = if code == 0x65 { 0x0080_0000 } else { 0 };
                        unsafe { CGEventSetFlags(key, 0x0010_0000 | 0x0008_0000 | function) };
                        unsafe { CGEventPost(0, key) };
                        unsafe { CFRelease(key) };
                    }
                    steps.next();
                }
                1 if pressed >= 11 || steps.waited(2000) => {
                    match (pressed, unsafe { AXIsProcessTrusted() } != 0) {
                        (11, _) => {}
                        (_, true) => steps.check(false, || format!("a trusted program posted both hotkeys: F9 heard {} times, K {}", pressed % 10, pressed / 10)),
                        (0, false) => println!("    the system does not take keys this program posts: the press is not tried"),
                        _ => println!("    heard: F9 {} times, K {} times", pressed % 10, pressed / 10),
                    }
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    thread_local! {
        /// What the stand-in for another program's window took dropped.
        static TAKEN: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    extern "C" fn target_takes(_: Obj, _: *const c_void, _: Obj) -> usize {
        1
    }

    extern "C" fn target_dropped(_: Obj, _: *const c_void, info: Obj) -> u8 {
        let board: Obj = send(info, c"draggingPasteboard", ());
        // (Where it holds no text, what it holds instead, to say.)
        let text = text_of(send(board, c"stringForType:", (unsafe { NSPasteboardTypeString },)));
        TAKEN.set(text.or_else(|| text_of(send(send::<Obj>(board, c"types", ()), c"description", ())).map(|kinds| format!("no text, but {kinds}"))));
        1
    }

    /// A window that stands in for another program's: it takes text dropped on it, as a copy.
    fn drop_target(frame: NSRect) -> Obj {
        let mut made = class(c"TestDropTarget");
        if made.is_null() {
            unsafe {
                made = objc_allocateClassPair(class(c"NSView"), c"TestDropTarget".as_ptr(), 0);
                class_addMethod(made, sel(c"draggingEntered:"), target_takes as *const c_void, c"Q@:@".as_ptr());
                class_addMethod(made, sel(c"draggingUpdated:"), target_takes as *const c_void, c"Q@:@".as_ptr());
                class_addMethod(made, sel(c"performDragOperation:"), target_dropped as *const c_void, c"c@:@".as_ptr());
                objc_registerClassPair(made);
            }
        }
        let window: Obj = send(class(c"NSWindow"), c"alloc", ());
        let window: Obj = send(window, c"initWithContentRect:styleMask:backing:defer:", (frame, 1usize, 2usize, 0u8));
        let view: Obj = send(send::<Obj>(made, c"alloc", ()), c"initWithFrame:", (frame,));
        send::<()>(view, c"registerForDraggedTypes:", (array(&[unsafe { NSPasteboardTypeString }]),));
        send::<()>(window, c"setReleasedWhenClosed:", (0u8,));
        send::<()>(window, c"setContentView:", (view,));
        send::<()>(window, c"orderFront:", (std::ptr::null_mut::<c_void>(),));
        window
    }

    /// The middle of a rectangle in a window's view, where the screen has it for posted mouse
    /// events: from the top left of the main screen, downward.
    fn on_the_screen(window: Obj, rect: Rect) -> NSPoint {
        let view = view_of(window);
        let middle = NSRect { origin: NSPoint { x: f64::from(rect.x + rect.w / 2.0), y: f64::from(rect.y + rect.h / 2.0) }, size: NSSize::default() };
        let in_window: NSRect = send(view, c"convertRect:toView:", (middle, std::ptr::null_mut::<c_void>()));
        let on_screen: NSRect = send(window, c"convertRectToScreen:", (in_window,));
        let main: Obj = items(send(class(c"NSScreen"), c"screens", ()))[0];
        NSPoint { x: on_screen.origin.x, y: rect_of(main, c"frame").size.height - on_screen.origin.y }
    }

    /// Posts a person's drag from one place on the screen to another (pressed, moved in steps,
    /// let go), from a thread of its own: the system's drag runs meanwhile.
    fn post_drag(from: NSPoint, to: NSPoint) {
        let post = |kind: u32, at: NSPoint| {
            let event = unsafe { CGEventCreateMouseEvent(std::ptr::null_mut(), kind, at, 0) };
            unsafe { CGEventPost(0, event) };
            unsafe { CFRelease(event) };
            std::thread::sleep(Duration::from_millis(40));
        };
        std::thread::spawn(move || {
            let at = |part: f64| NSPoint { x: from.x + (to.x - from.x) * part, y: from.y + (to.y - from.y) * part };
            post(5, at(0.0));
            post(1, at(0.0));
            (1..=20).for_each(|step| post(6, at(f64::from(step) / 20.0)));
            post(6, at(1.0));
            post(2, at(1.0));
        });
    }

    /// Dragged past the window's edge, what a box hands out goes to the system's drag: let go
    /// on another window that takes text, that window has it, and the box hears it was copied.
    fn a_drag_out_of_the_window_reaches_another_program() -> Vec<String> {
        let mut outcome = None;
        let mut target: Obj = std::ptr::null_mut();
        let window = Window { title: "Drag out", width: 240.0, height: 140.0, position: Some((80, 120)), ..Default::default() };
        check(window, move |steps, ui| {
            ui.column().pad(16.0).open();
            let source = ui.button("Drag me out");
            ui.close();
            if let Some(effect) = ui.drag_out(&source, taut::DragData::Text("from taut")) {
                outcome = Some(effect);
            }
            let Some(window) = window_titled("Drag out") else { return };
            match steps.step {
                0 if steps.waited(500) => {
                    if unsafe { AXIsProcessTrusted() } == 0 {
                        println!("    the system does not take mouse events this program posts: not tried");
                        steps.done = true;
                        return;
                    }
                    target = drop_target(NSRect { origin: NSPoint { x: 480.0, y: 200.0 }, size: NSSize { width: 220.0, height: 160.0 } });
                    let from = on_the_screen(window, source.rect);
                    let frame = rect_of(target, c"frame");
                    let main: Obj = items(send(class(c"NSScreen"), c"screens", ()))[0];
                    let to = NSPoint { x: frame.origin.x + 110.0, y: rect_of(main, c"frame").size.height - frame.origin.y - 80.0 };
                    println!("    dragged from {from:?} to {to:?}");
                    post_drag(from, to);
                    steps.next();
                }
                1 if outcome.is_some() || steps.waited(6000) => {
                    let taken = TAKEN.take();
                    steps.check(taken.as_deref() == Some("from taut"), || format!("the other window took {taken:?}"));
                    steps.check(outcome == Some(taut::DropEffect::Copied), || format!("the box heard {outcome:?}"));
                    steps.check(!source.pressed, || "the button is still held after the drop".into());
                    // (Only put away: the system's drag may still hold on to it.)
                    send::<()>(target, c"orderOut:", (std::ptr::null_mut::<c_void>(),));
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    #[link(name = "System")]
    unsafe extern "C" {
        static _NSConcreteGlobalBlock: [usize; 0];
    }

    /// A block as the compiler lays one out, holding nothing.
    #[repr(C)]
    struct Block {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: *const c_void,
        descriptor: *const [usize; 2],
    }

    /// A block calling `invoke`, made to live for good.
    fn block(invoke: *const c_void) -> *const Block {
        static DESCRIBED: [usize; 2] = [0, size_of::<Block>()];
        Box::leak(Box::new(Block { isa: (&raw const _NSConcreteGlobalBlock).cast(), flags: 1 << 28, reserved: 0, invoke, descriptor: &DESCRIBED }))
    }

    /// What the notification centre answered, from its own threads: the permission (+1 once
    /// answered), how many notifications it shows (+1), and whether the click was answered.
    static PERMISSION: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    static SHOWING: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    static CLICK_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    extern "C" fn settings_came(_: *const Block, settings: Obj) {
        PERMISSION.store(send::<i64>(settings, c"authorizationStatus", ()) + 1, std::sync::atomic::Ordering::SeqCst);
    }
    extern "C" fn showing_came(_: *const Block, shown: Obj) {
        SHOWING.store(items(shown).len() as i64 + 1, std::sync::atomic::Ordering::SeqCst);
    }
    extern "C" fn click_done(_: *const Block) {
        CLICK_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    extern "C" fn this_one(this: Obj, _: *const c_void) -> Obj {
        this
    }
    extern "C" fn first_one(_: Obj, _: *const c_void) -> Obj {
        string("1")
    }

    /// A stand-in for the system's answer to a click on notification 1: its notification, that
    /// one's request, and its identifier, all as the system's answer them.
    fn stand_in_click() -> Obj {
        let mut made = class(c"TestResponse");
        if made.is_null() {
            unsafe {
                made = objc_allocateClassPair(class(c"NSObject"), c"TestResponse".as_ptr(), 0);
                class_addMethod(made, sel(c"notification"), this_one as *const c_void, c"@@:".as_ptr());
                class_addMethod(made, sel(c"request"), this_one as *const c_void, c"@@:".as_ptr());
                class_addMethod(made, sel(c"identifier"), first_one as *const c_void, c"@@:".as_ptr());
                objc_registerClassPair(made);
            }
        }
        send(send(send::<Obj>(made, c"alloc", ()), c"init", ()), c"autorelease", ())
    }

    /// Makes an app bundle of this program in a folder of its own, signed for this machine.
    fn bundled_copy() -> std::io::Result<PathBuf> {
        let app = std::env::temp_dir().join(format!("taut-check-{}", std::process::id())).join("TautCheck.app");
        std::fs::create_dir_all(app.join("Contents/MacOS"))?;
        std::fs::copy(std::env::current_exe()?, app.join("Contents/MacOS/TautCheck"))?;
        let keys = [("CFBundleIdentifier", "org.taut.check"), ("CFBundleExecutable", "TautCheck"), ("CFBundleName", "TautCheck"), ("CFBundlePackageType", "APPL"), ("CFBundleVersion", "1")];
        let listed: String = keys.iter().map(|(key, value)| format!("<key>{key}</key><string>{value}</string>")).collect();
        let head = r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#;
        std::fs::write(app.join("Contents/Info.plist"), format!("{head}<plist version=\"1.0\"><dict>{listed}</dict></plist>"))?;
        let _ = std::process::Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app).output();
        Ok(app.join("Contents/MacOS/TautCheck"))
    }

    /// Notifications go to the system's centre from an app bundle: this program is copied into
    /// one and runs this check there. It shows one with a picture; the centre has it where the
    /// system permits; a click reported by the centre reaches the app.
    fn a_notification_from_an_app_bundle_reaches_the_system() -> Vec<String> {
        if std::env::var_os("TAUT_BUNDLED").is_none() {
            let ran = bundled_copy().and_then(|program| std::process::Command::new(program).arg("notification").env("TAUT_BUNDLED", "1").env_remove("TAUT_SHOTS").output());
            return match ran {
                Ok(done) if done.status.success() => {
                    String::from_utf8_lossy(&done.stdout).lines().filter(|line| line.starts_with("    ")).for_each(|line| println!("{line}"));
                    Vec::new()
                }
                Ok(done) => vec![format!("in a bundle: {}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr))],
                Err(error) => vec![format!("no bundle to run in: {error}")],
            };
        }
        let (mut shown, mut heard) = (None, None);
        let window = Window { title: "Notices", width: 240.0, height: 120.0, ..Default::default() };
        check(window, move |steps, ui| {
            ui.label("Notifications");
            heard = heard.or(ui.notification_clicked());
            match steps.step {
                0 if steps.waited(300) => {
                    shown = Some(ui.system_notification("Hello", "From taut", Some((2, &[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff]))));
                    steps.next();
                }
                1 if steps.waited(1500) => {
                    let centre: Obj = send(class(c"UNUserNotificationCenter"), c"currentNotificationCenter", ());
                    let delegate: Obj = send(centre, c"delegate", ());
                    steps.check(!delegate.is_null(), || "the centre has no delegate: clicks would not be heard".into());
                    send::<()>(centre, c"getNotificationSettingsWithCompletionHandler:", (block(settings_came as *const c_void),));
                    send::<()>(centre, c"getDeliveredNotificationsWithCompletionHandler:", (block(showing_came as *const c_void),));
                    let selector = c"userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:";
                    send::<()>(delegate, selector, (centre, stand_in_click(), block(click_done as *const c_void)));
                    steps.next();
                }
                2 if steps.waited(1000) => {
                    let ordering = std::sync::atomic::Ordering::SeqCst;
                    let (permission, showing) = (PERMISSION.load(ordering) - 1, SHOWING.load(ordering) - 1);
                    println!("    in a bundle: permission {permission} (2 given, 1 refused, 0 not asked yet), {showing} notifications shown");
                    steps.check(permission >= 0 && showing >= 0, || "the centre did not answer".into());
                    steps.check(!matches!(permission, 2 | 3) || showing >= 1, || "permitted, the centre shows no notification".into());
                    steps.check(heard == shown && shown.is_some(), || format!("clicked, the app heard {heard:?}, not {shown:?}"));
                    steps.check(CLICK_DONE.load(ordering), || "the click was not answered to the centre".into());
                    steps.done = true;
                }
                _ => {}
            }
        })
    }

    /// Says what an Objective-C exception was, as it is thrown: past Rust's frames it can only
    /// end the program.
    extern "C" fn thrown(exception: Obj) -> Obj {
        let said = |name: &CStr| text_of(send(send::<Obj>(exception, name, ()), c"description", ())).unwrap_or_default();
        eprintln!("Objective-C exception: {}: {}\n{}", said(c"name"), said(c"reason"), said(c"callStackSymbols"));
        exception
    }

    unsafe extern "C" {
        fn objc_setExceptionPreprocessor(preprocessor: extern "C" fn(Obj) -> Obj) -> *const c_void;
    }

    pub fn main() {
        unsafe { objc_setExceptionPreprocessor(thrown) };
        type Check = fn() -> Vec<String>;
        let checks: [(&str, Check); 16] = [
            ("a window shows what the library draws, and a click redraws it", a_window_shows_what_the_library_draws_and_a_click_redraws_it),
            ("keys and an input method's text reach the focused box", keys_and_an_input_methods_text_reach_the_focused_box),
            ("the clipboard is the system's", the_clipboard_is_the_systems),
            ("a second window opens, and its close button closes it", a_second_window_opens_and_its_close_button_closes_it),
            ("a window opens where asked, and is drawn whole at a new size", a_window_opens_where_asked_and_is_drawn_whole_at_a_new_size),
            ("files let go on a window reach its app", files_let_go_on_a_window_reach_its_app),
            ("a drag out of the window reaches another program", a_drag_out_of_the_window_reaches_another_program),
            ("a see-through window is clear where nothing is drawn", a_see_through_window_is_clear_where_nothing_is_drawn),
            ("a window without a frame has no buttons", a_window_without_a_frame_has_no_buttons),
            ("a waker on another thread wakes the window", a_waker_on_another_thread_wakes_the_window),
            ("a reader reads the window and presses its button", a_reader_reads_the_window_and_presses_its_button),
            ("a reader reads text by ranges, and tables and trees by rows", a_reader_reads_text_by_ranges_and_tables_and_trees_by_rows),
            ("the tray icon is a status item with a menu", the_tray_icon_is_a_status_item_with_a_menu),
            ("a notification from an app bundle reaches the system", a_notification_from_an_app_bundle_reaches_the_system),
            ("a global hotkey is registered and heard", a_global_hotkey_is_registered_and_heard),
            ("the system's file dialog comes as a sheet and answers", the_systems_file_dialog_comes_as_a_sheet_and_answers),
        ];
        let only = std::env::args().nth(1).filter(|word| !word.starts_with('-'));
        let mut failed = 0;
        for (name, check) in checks.into_iter().filter(|(name, _)| only.as_deref().is_none_or(|only| name.contains(only))) {
            let started = Instant::now();
            let found = check();
            println!("{} {name} ({} ms)", if found.is_empty() { "ok    " } else { "FAILED" }, started.elapsed().as_millis());
            found.iter().for_each(|what| println!("    {what}"));
            failed += !found.is_empty() as usize;
        }
        if failed > 0 {
            eprintln!("{failed} checks failed");
            std::process::exit(1);
        }
    }
}
