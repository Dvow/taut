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

    /// What the screen shows of a window's content, through CoreGraphics' window picture where
    /// the system still has it: its width and pixels, as `shown` gives them. (The picture has the
    /// title bar on top: only the rows of the view are kept.)
    fn on_screen(window: Obj) -> Option<(usize, Vec<u32>)> {
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
            let view = rect_of(view_of(window), c"bounds").size.height;
            let scale: f64 = send(window, c"backingScaleFactor", ());
            let rows = ((view * scale).round() as usize).min(height);
            Some((width, pixels[(height - rows) * width..].to_vec()))
        }
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

    /// Keys posted as a person's reach the focused text box, through the system's text input;
    /// an input method's text does too, and it is told where the text is.
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
                    let marked: u8 = send(view, c"hasMarkedText", ());
                    steps.check(marked == 1, || "an input method's text being composed is not noted".into());
                    let place: NSRect = send(view, c"firstRectForCharacterRange:actualRange:", (NSRange::default(), std::ptr::null_mut::<c_void>()));
                    let frame = rect_of(window, c"frame");
                    let inside = place.origin.x >= frame.origin.x
                        && place.origin.x <= frame.origin.x + frame.size.width
                        && place.origin.y >= frame.origin.y
                        && place.origin.y <= frame.origin.y + frame.size.height;
                    steps.check(inside && place.size.height > 0.0, || format!("the input method is told the text is at {place:?}, outside the window at {frame:?}"));
                    send::<()>(view, c"insertText:replacementRange:", (string("日本"), none));
                    steps.next();
                }
                3 if text.contains("日本") || steps.waited(2000) => {
                    steps.check(text == "hx日本i", || format!("an input method's text: the box holds {text:?}"));
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
                    send::<()>(sheet, c"ok:", (std::ptr::null_mut::<c_void>(),));
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

    /// The program's first window as a client reads it, from another thread (the window's
    /// thread answers): `None` where the system does not trust this program to read others.
    fn read_as_a_client() -> std::sync::mpsc::Receiver<Option<Vec<(String, String)>>> {
        let (tell, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if unsafe { AXIsProcessTrusted() } == 0 {
                let _ = tell.send(None);
                return;
            }
            let program = unsafe { AXUIElementCreateApplication(std::process::id() as i32) };
            let mut windows: Obj = std::ptr::null_mut();
            unsafe { AXUIElementCopyAttributeValue(program, string("AXWindows"), &mut windows) };
            let mut found = Vec::new();
            items(windows).first().into_iter().for_each(|window| told_to_clients(*window, 0, &mut found));
            _ = tell.send(Some(found));
        });
        heard
    }

    /// A screen reader reads what the window tells, through NSAccessibility: the text and the
    /// button, where they are; it presses the button, and hears the new count.
    fn a_reader_reads_the_window_and_presses_its_button() -> Vec<String> {
        let mut count = 0;
        let mut client = None;
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
                    client = Some(read_as_a_client());
                    steps.next();
                }
                2 if count == 1 && steps.waited(300) => {
                    told(view, 0, &mut all);
                    steps.check(all.iter().any(|(_, name)| name == "Pressed 1 times"), || format!("pressed by a reader, the window tells {all:?}"));
                    steps.next();
                }
                3 => match client.as_ref().map(|heard| heard.try_recv()) {
                    Some(Ok(None)) => {
                        println!("    the system does not trust this program to read others: not read as a client");
                        steps.done = true;
                    }
                    Some(Ok(Some(found))) => {
                        println!("    a client reads {} elements", found.len());
                        steps.check(found.iter().any(|(role, name)| role == "AXButton" && name.contains("Press")), || format!("a client reads {found:?}"));
                        steps.done = true;
                    }
                    _ => {}
                },
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
        let checks: [(&str, Check); 11] = [
            ("a window shows what the library draws, and a click redraws it", a_window_shows_what_the_library_draws_and_a_click_redraws_it),
            ("keys and an input method's text reach the focused box", keys_and_an_input_methods_text_reach_the_focused_box),
            ("the clipboard is the system's", the_clipboard_is_the_systems),
            ("a second window opens, and its close button closes it", a_second_window_opens_and_its_close_button_closes_it),
            ("a window opens where asked, and is drawn whole at a new size", a_window_opens_where_asked_and_is_drawn_whole_at_a_new_size),
            ("files let go on a window reach its app", files_let_go_on_a_window_reach_its_app),
            ("a see-through window is clear where nothing is drawn", a_see_through_window_is_clear_where_nothing_is_drawn),
            ("a window without a frame has no buttons", a_window_without_a_frame_has_no_buttons),
            ("a waker on another thread wakes the window", a_waker_on_another_thread_wakes_the_window),
            ("a reader reads the window and presses its button", a_reader_reads_the_window_and_presses_its_button),
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
