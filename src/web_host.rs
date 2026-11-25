//! The web: the interface in a page's `<canvas>`, built for `wasm32-unknown-unknown` with no
//! crates. The loader (`web.js`, [`crate::web::LOADER`]) makes the page's elements, hands each
//! DOM event to a function exported here (`taut_*`), and does what the functions imported here
//! ask (in the module `taut`): show pixels, set the cursor, measure and draw text, keep the
//! clipboard, storage and the reader's elements.
//!
//! The CPU draws every pixel, as everywhere; what changed goes to the canvas through
//! `putImageData`. Nothing runs while nothing happens: a frame is asked for
//! (`requestAnimationFrame`) only when something changed or the interface asked for a timer.
//! Input that may need the person's own gesture (a copy, a file picker, a link) is run at once,
//! inside the DOM event; the rest waits for the next frame.
use crate::color::Color;
use crate::event::{Cursor, Event, Key};
use crate::geom::{Point, Rect, Size, pt};
use crate::platform::{Clipboard, PicturesAndFiles, Window};
use crate::preferences::Preferences;
use crate::surface::Surface;
use crate::theme::ContrastColors;
use crate::ui::Ui;
use crate::web_input::{Did, Pointer};
use crate::web_mirror::Mirror;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Duration;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn present(pixels: *const u8, x: u32, y: u32, width: u32, height: u32);
    fn wake(after_ms: f64);
    fn set_cursor(shape: u32);
    fn set_title(text: *const u8, length: usize);
    fn text_field(x: f32, y: f32, width: f32, height: f32, on: u32);
    fn clipboard_set(text: *const u8, length: usize);
    fn mirror(json: *const u8, length: usize);
    fn log(text: *const u8, length: usize);
    fn now() -> f64;
    fn clock() -> f64;
    fn zone() -> f64;
    fn storage_get(key: *const u8, length: usize) -> i32;
    fn storage_set(key: *const u8, key_length: usize, value: *const u8, value_length: usize) -> u32;
    fn open_link(link: *const u8, length: usize);
    fn ask_file(accept: *const u8, length: usize, name: *const u8, name_length: usize, kind: u32) -> u32;
    fn download(name: *const u8, name_length: usize, bytes: *const u8, length: usize);
    fn save_file(path: *const u8, path_length: usize, bytes: *const u8, length: usize) -> u32;
    fn font_file(nth: u32, face: *mut u32) -> usize;
    fn take(into: *mut u8);
    pub(crate) fn text_metrics(font: *const u8, length: usize, sizes: *mut f32);
    pub(crate) fn text_measure(font: *const u8, length: usize, text: *const u8, text_length: usize, stops: *mut f32, count: usize) -> f32;
    pub(crate) fn text_draw(font: *const u8, length: usize, text: *const u8, text_length: usize, place: *mut f32) -> usize;
}

/// The bytes the loader made last (a line's pixels, a stored value, a font), `length` of them.
pub(crate) fn took(length: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; length];
    // SAFETY: the loader writes exactly the `length` bytes it said it had.
    unsafe { take(bytes.as_mut_ptr()) };
    bytes
}

/// Says something on the browser's console.
pub(crate) fn say(text: &str) {
    // SAFETY: the loader reads `length` bytes of UTF-8.
    unsafe { log(text.as_ptr(), text.len()) };
}

/// Milliseconds since 1970, UTC, by the browser's clock.
pub(crate) fn unix_ms() -> f64 {
    // SAFETY: a plain question.
    unsafe { clock() }
}

/// Seconds since 1970, UTC, by the browser's clock.
pub(crate) fn unix_seconds() -> u64 {
    (unix_ms() / 1000.0).max(0.0) as u64
}

/// How far the person's time zone is ahead of UTC, in seconds.
pub(crate) fn zone_ahead() -> i64 {
    // SAFETY: a plain question.
    (unsafe { zone() } * 60.0) as i64
}

/// The value kept under `key` in the page's `localStorage`.
pub(crate) fn stored(key: &str) -> Option<String> {
    // SAFETY: the loader reads `length` bytes, and answers with the length of what it has.
    let length = unsafe { storage_get(key.as_ptr(), key.len()) };
    let length = usize::try_from(length).ok()?;
    String::from_utf8(took(length)).ok()
}

/// Keeps `value` under `key` in `localStorage`; false if the browser refused (full, or
/// storage off).
pub(crate) fn store(key: &str, value: &str) -> bool {
    // SAFETY: the loader reads both strings.
    unsafe { storage_set(key.as_ptr(), key.len(), value.as_ptr(), value.len()) != 0 }
}

/// Opens a link in a new tab (from a press, or the browser may block it).
pub(crate) fn open(link: &str) {
    // SAFETY: the loader reads `length` bytes.
    unsafe { open_link(link.as_ptr(), link.len()) };
}

/// Has the browser save `bytes` as a download named `name`.
pub(crate) fn save_as(name: &str, bytes: &[u8]) {
    // SAFETY: the loader reads both.
    unsafe { download(name.as_ptr(), name.len(), bytes.as_ptr(), bytes.len()) };
}

/// Writes `bytes` into the file the browser's save picker chose for `path` (`/saved/<name>`):
/// whether it chose one.
pub(crate) fn save_to(path: &std::path::Path, bytes: &[u8]) -> bool {
    let path = path.to_string_lossy();
    // SAFETY: the loader reads both at once.
    unsafe { save_file(path.as_ptr(), path.len(), bytes.as_ptr(), bytes.len()) != 0 }
}

/// The font files the page gave the loader (`fonts`), each with the face it is for: 0
/// regular, 1 bold, 2 mono, 3 italic.
fn page_fonts() -> Vec<(u32, Vec<u8>)> {
    let mut fonts = Vec::new();
    for nth in 0..4 {
        let mut face = 0;
        // SAFETY: the loader writes the face and answers with the file's length.
        let length = unsafe { font_file(nth, &mut face) };
        if length > 0 {
            fonts.push((face, took(length)));
        }
    }
    fonts
}

/// The text renderer of a page: the page's font files where it gave the loader some (the same
/// letters as on every system; the browser's for what they lack), else the browser's.
pub(crate) fn system_text() -> Box<dyn crate::text::TextSystem> {
    let mut files = page_fonts().into_iter().filter_map(|(face, bytes)| crate::font_file::FontFile::read(bytes).map(|file| (face, file)));
    let browser = Box::new(crate::web_text::SystemText::default());
    let Some((_, regular)) = files.next() else { return browser };
    let mut fonts = crate::font_file::AppFonts::new(regular);
    for (face, file) in files {
        fonts = match face {
            1 => fonts.bold(file),
            2 => fonts.mono(file),
            _ => fonts.italic(file),
        };
    }
    Box::new(fonts.fallback(browser))
}

// ---- parts a program uses only when it asks ----

/// The parts of a page a program carries only once it calls for them: each is handed over by
/// that call (`add_part`), and the loader reaches it through `taut_heard` by its number.
pub(crate) mod parts {
    /// Files and text dropped on the page (`web_drag_drop`).
    pub const DROPS: u32 = 0;
    /// What is dragged out of the page (`web_drag_drop`).
    pub const DRAG_OUT: u32 = 1;
    /// The clipboard's pictures and files (`web_clipboard`).
    pub const CLIPBOARD: u32 = 2;
    /// Notifications and one running copy (`web_desktop`).
    pub const DESKTOP: u32 = 3;
    /// Printing (`web_print`).
    pub const PRINT: u32 = 4;
    /// The person's fonts (`web_fonts`).
    pub const FONTS: u32 = 5;
    /// WebGL2 (`web_gpu`).
    pub const GPU: u32 = 6;
    /// The file pickers' answers beyond one file (`web_files`).
    pub const FILES: u32 = 7;
    /// How many there are.
    pub const COUNT: usize = 8;
}

/// What the loader heard for a part: which kind of thing, where (CSS pixels), a number, and
/// bytes (text, or a file).
pub(crate) struct Heard {
    pub what: u32,
    pub at: Point,
    pub number: f64,
    pub bytes: Vec<u8>,
}

/// What the interface does once a part heard something.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Then {
    /// Nothing.
    Wait,
    /// It runs at the next frame.
    Run,
    /// It runs at once, inside the DOM event (which may need to be the person's gesture, or
    /// need the run's answer).
    RunNow,
}

/// A part's work: after each run, and when the loader heard something for it (answering it
/// with a number).
#[derive(Clone, Copy)]
pub(crate) struct Part {
    pub after: fn(&mut Ui),
    pub heard: fn(&mut Ui, Heard) -> (Then, u32),
}

/// Has a part join the interface's runs, from the next on.
pub(crate) fn add_part(number: u32, part: Part) {
    let mut all = PARTS.get();
    all[number as usize] = Some(part);
    PARTS.set(all);
}

// ---- the clipboard ----

/// The clipboard as a page has it: what was copied or pasted last (`CLIPBOARD`). A copy goes to
/// the system clipboard too (the loader hands it on in the browser's copy event, or through
/// `navigator.clipboard`); a paste with Control+V brings the system's text first (`taut_paste`).
/// Pictures and files, once a program asks for one (see `web_clipboard`).
pub(crate) struct PageClipboard;

impl Clipboard for PageClipboard {
    fn get(&mut self) -> Option<String> {
        CLIPBOARD.with_borrow(|text| (!text.is_empty()).then(|| text.clone()))
    }
    fn set(&mut self, text: &str) {
        CLIPBOARD.set(text.into());
        // SAFETY: the loader reads `length` bytes.
        unsafe { clipboard_set(text.as_ptr(), text.len()) };
    }
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

/// Hands the clipboard its picture and file code (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    if PICTURES_AND_FILES.get().is_none() {
        PICTURES_AND_FILES.set(Some(crate::web_clipboard::pictures_and_files()));
    }
}

thread_local! {
    /// The page's interface, once `run` made it.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    /// The clipboard's text: what was copied or pasted last.
    pub(crate) static CLIPBOARD: RefCell<String> = const { RefCell::new(String::new()) };
    /// The clipboard's pictures and files, once an interface asked for one.
    static PICTURES_AND_FILES: Cell<Option<PicturesAndFiles<PageClipboard>>> = const { Cell::new(None) };
    /// The parts in use, by number.
    static PARTS: Cell<[Option<Part>; parts::COUNT]> = const { Cell::new([None; parts::COUNT]) };
    /// Files the person handed the page (picked, dropped, pasted), by the path they are given
    /// (`/picked/<name>`, ...), and their bytes.
    pub(crate) static FILES: RefCell<Vec<(PathBuf, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// The bytes of a file the person handed the page (see [`crate::web::picked`]).
pub(crate) fn picked(path: &std::path::Path) -> Option<Vec<u8>> {
    FILES.with_borrow(|files| files.iter().find(|(name, _)| name == path).map(|(_, bytes)| bytes.clone()))
}

/// Keeps a file handed to the page, in place of one of the same path.
pub(crate) fn keep_file(path: PathBuf, bytes: Vec<u8>) {
    FILES.with_borrow_mut(|files| {
        files.retain(|(kept, _)| *kept != path);
        files.push((path, bytes));
    });
}

/// The page and what keeps the interface shown in it.
struct Host {
    app: Box<dyn FnMut(&mut Ui)>,
    ui: Ui,
    surface: Surface,
    /// The pixels of a changed place, as `putImageData` takes them.
    rgba: Vec<u8>,
    size: Size,
    scale: f32,
    /// The interface must run, and draw after.
    rerun: bool,
    drawn: bool,
    started: f64,
    mirror: Mirror,
    /// Which nodes the mirror last heard of (`access_version`): the same, none are looked at.
    told: u64,
    cursor: Cursor,
    ime: Option<Rect>,
    title: String,
    /// The console was told that second windows don't show.
    told_windows: bool,
    /// WebGL2, where the page is shown through it (feature `gpu`).
    #[cfg(feature = "gpu")]
    gpu: Option<crate::web_gpu::Gpu>,
}

impl Host {
    /// Tells the interface the time, by the page's clock.
    fn tick(&mut self) {
        // SAFETY: a plain question.
        let ms = (unsafe { now() } - self.started).max(0.0);
        self.ui.set_time(Duration::from_secs_f64(ms / 1000.0));
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO);
    }

    /// Runs the interface if it must, and does what it asked of the page.
    fn describe(&mut self) {
        self.tick();
        if !self.rerun || self.size.w <= 0.0 {
            return;
        }
        self.ui.resize(self.size, self.scale);
        self.ui.run(&mut self.app);
        let _ = self.ui.take_closing();
        (self.rerun, self.drawn) = (false, false);
        if !self.told_windows && self.ui.windows().next().is_some() {
            say("taut: a page shows one window; second windows are not shown on the web");
            self.told_windows = true;
        }
        self.follow();
        PARTS.get().iter().flatten().for_each(|part| (part.after)(&mut self.ui));
        let (version, announced) = (self.ui.access_version(), self.ui.announced());
        if version == self.told && announced.is_empty() {
            return;
        }
        self.told = version;
        if let Some(json) = self.mirror.update(self.ui.access_nodes(), announced) {
            // SAFETY: the loader reads `length` bytes of UTF-8.
            unsafe { mirror(json.as_ptr(), json.len()) };
        }
    }

    /// After a run: the title, the cursor, where text is typed, a file asked for.
    fn follow(&mut self) {
        if let Some(title) = self.ui.take_title().filter(|title| *title != self.title) {
            // SAFETY: the loader reads `length` bytes.
            unsafe { set_title(title.as_ptr(), title.len()) };
            self.title = title;
        }
        if std::mem::replace(&mut self.cursor, self.ui.cursor()) != self.cursor {
            // SAFETY: a plain request.
            unsafe { set_cursor(self.cursor as u32) };
        }
        let ime = self.ui.ime_area();
        if ime != self.ime || ime.is_some() {
            let place = ime.unwrap_or_default();
            // SAFETY: a plain request.
            unsafe { text_field(place.x, place.y, place.w, place.h, ime.is_some() as u32 + self.ui.password_focused() as u32) };
        }
        self.ime = ime;
        if let Some(asked) = self.ui.take_file_dialog_request() {
            let accept = asked.endings.join(",");
            let kind = asked.saving as u32 | (asked.many as u32) << 1 | (asked.folders as u32) << 2;
            // SAFETY: the loader reads both, and says whether it answers a save itself (with the
            // browser's save picker).
            let answers = unsafe { ask_file(accept.as_ptr(), accept.len(), asked.name.as_ptr(), asked.name.len(), kind) } != 0;
            if asked.saving && !answers {
                self.ui.file_dialog_answered(Some(PathBuf::from("/downloads").join(asked.name)));
                self.rerun = true;
            }
        }
    }

    /// Draws what changed and hands it to the canvas.
    fn draw(&mut self) {
        if self.drawn {
            return;
        }
        self.drawn = true;
        #[cfg(feature = "gpu")]
        if let Some(gpu) = &mut self.gpu {
            if gpu.draw(&mut self.ui, &mut self.surface) {
                let _ = self.ui.changed();
                return;
            }
            // Lost, or failed: the CPU shows the page from now on, all of it drawn afresh.
            (self.gpu, self.surface) = (None, Surface::default());
        }
        for [left, top, right, bottom] in self.surface.update(&mut self.ui) {
            let (width, height) = (right - left, bottom - top);
            self.rgba.clear();
            for row in top..bottom {
                let from = row * self.surface.width;
                self.rgba.extend(self.surface.pixels[from + left..from + right].iter().flat_map(|pixel| straight(*pixel)));
            }
            // SAFETY: `width * height * 4` bytes, read at once.
            unsafe { present(self.rgba.as_ptr(), left as u32, top as u32, width as u32, height as u32) };
        }
        let _ = self.ui.changed();
    }

    /// Asks for the next frame: now if the interface must run, when its timer is due, or none
    /// (nor while the page has given no size yet).
    fn schedule(&self) {
        let after = match (self.rerun || !self.drawn, self.ui.wake_in()) {
            _ if self.size.w <= 0.0 => -1.0,
            (true, _) => 0.0,
            (false, Some(wait)) => wait.as_secs_f64() * 1000.0,
            (false, None) => -1.0,
        };
        // SAFETY: a plain request.
        unsafe { wake(after) };
    }

    /// Hands an input event to the interface.
    fn hear(&mut self, event: Event) {
        self.rerun |= self.ui.event(event);
    }
}

/// A pixel of the interface (`0xAARRGGBB`, premultiplied) as a canvas takes it: red, green,
/// blue, alpha, straight.
fn straight(pixel: u32) -> [u8; 4] {
    let alpha = pixel >> 24;
    let [blue, green, red, _] = pixel.to_le_bytes();
    if alpha == 255 || alpha == 0 {
        return [red, green, blue, alpha as u8];
    }
    let part = |channel: u8| ((u32::from(channel) * 255 + alpha / 2) / alpha).min(255) as u8;
    [part(red), part(green), part(blue), alpha as u8]
}

/// Works with the page's interface, if there is one and it is not busy (an event from inside a
/// call is dropped: the loader holds such events back).
fn with_host(work: impl FnOnce(&mut Host)) {
    HOST.with(|host| {
        let Ok(mut host) = host.try_borrow_mut() else { return };
        if let Some(host) = host.as_mut() {
            work(host);
        }
    });
}

/// Runs the interface at once (for input that may need the person's gesture to be the
/// browser's: a copy, a picker, a link), then asks for a frame to draw it.
fn now_and_later(host: &mut Host) {
    host.describe();
    host.schedule();
}

/// Opens the page's interface: it runs `app` from now on, each time something happens. The
/// window is the element the loader was given (the whole page by default): `window`'s size and
/// place are not used, its title is the page's. Returns at once: the page keeps the interface,
/// and the browser calls it.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    std::panic::set_hook(Box::new(|told| say(&format!("taut: {told}"))));
    let mut ui = Ui::new(system_text(), Box::new(PageClipboard));
    if window.transparent {
        ui.theme.bg = Color::NONE;
    }
    ui.set_screen_reader(true);
    ui.set_native_file_dialog(true);
    // SAFETY: plain requests and questions.
    let started = unsafe { now() };
    unsafe { set_title(window.title.as_ptr(), window.title.len()) };
    let host = Host {
        app: Box::new(app),
        ui,
        surface: Surface::default(),
        rgba: Vec::new(),
        size: Size { w: 0.0, h: 0.0 },
        scale: 1.0,
        rerun: true,
        drawn: false,
        started,
        mirror: Mirror::default(),
        told: 0,
        cursor: Cursor::Arrow,
        ime: None,
        title: window.title.into(),
        told_windows: false,
        #[cfg(feature = "gpu")]
        gpu: crate::web_gpu::Gpu::open(window.renderer),
    };
    HOST.set(Some(host));
    // SAFETY: a plain request.
    unsafe { wake(0.0) };
}

/// Wakes the interface: it runs again at the next frame. (The page has one thread: this is
/// for a callback the app set up, as a [`Waker`] on another system is for a thread.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;

impl Waker {
    /// Has the interface run once more, soon.
    pub fn wake(&self) {
        with_host(|host| {
            host.rerun = true;
            host.schedule();
        });
    }
}

// ---- what the loader calls ----

/// Room for `length` bytes the loader writes (a string, a file): handed back with the call
/// that uses them, which frees them.
#[unsafe(no_mangle)]
pub extern "C" fn taut_alloc(length: usize) -> *mut u8 {
    let mut room = std::mem::ManuallyDrop::new(Vec::<u8>::with_capacity(length.max(1)));
    room.as_mut_ptr()
}

/// The bytes at `at` that `taut_alloc` made room for, taken back.
fn given(at: *mut u8, length: usize) -> Vec<u8> {
    // SAFETY: `at` came from `taut_alloc(length)`, and the loader wrote `length` bytes there.
    unsafe { Vec::from_raw_parts(at, length, length.max(1)) }
}

/// Text the loader wrote at `at`.
fn given_text(at: *mut u8, length: usize) -> String {
    String::from_utf8(given(at, length)).unwrap_or_default()
}

/// The page's element is this size, in CSS pixels, at this many device pixels to one. (No
/// size: the canvas lost its pixels, and the next size is drawn whole.)
#[unsafe(no_mangle)]
pub extern "C" fn taut_size(width: f32, height: f32, scale: f32) {
    with_host(|host| {
        if width <= 0.0 {
            host.surface = Surface::default();
        }
        (host.size, host.scale, host.rerun) = (Size { w: width.max(0.0), h: height.max(0.0) }, scale.max(0.25), true);
        host.schedule();
    });
}

/// A frame: the interface runs if it must, and what changed is drawn.
#[unsafe(no_mangle)]
pub extern "C" fn taut_frame() {
    with_host(|host| {
        host.describe();
        host.draw();
        host.schedule();
    });
}

/// What the person asked the browser for: dark, more contrast (2 forced colours), less motion,
/// and an accent colour (`0xRRGGBB`, or -1 for none).
#[unsafe(no_mangle)]
pub extern "C" fn taut_looks(dark: u32, contrast: u32, still: u32, accent: i32) {
    let dark = dark != 0;
    let high_contrast = (contrast != 0).then_some(if dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
    let accent = u32::try_from(accent).ok().map(Color::rgb);
    let preferences = Preferences { dark, accent, high_contrast, reduced_motion: still != 0, ..Preferences::default() };
    with_host(|host| {
        host.rerun |= host.ui.set_preferences(preferences);
        host.schedule();
    });
}

/// A pointer event (see `web_input::Pointer`; places in CSS pixels).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn taut_pointer(did: u32, kind: u32, id: u32, primary: u32, x: f32, y: f32, button: i32, buttons: u32, pressure: f32, tilt_x: f32, tilt_y: f32, twist: f32, mods: u32) {
    let did = Did::of(did);
    let event = Pointer { did, kind, id, primary: primary != 0, at: pt(x, y), button, buttons, pressure, tilt: [tilt_x, tilt_y], twist, mods: crate::web_input::mods(mods) };
    with_host(|host| {
        crate::web_input::pointer(event, cfg!(feature = "touch")).into_iter().for_each(|event| host.hear(event));
        match did {
            Did::Down | Did::Up => now_and_later(host),
            _ => host.schedule(),
        }
    });
}

/// The wheel turned (`delta_mode` as the DOM counts it), or a touchpad pinched (Control held).
#[unsafe(no_mangle)]
pub extern "C" fn taut_wheel(x: f32, y: f32, dx: f32, dy: f32, delta_mode: u32, mods: u32) {
    with_host(|host| {
        let (right, down) = crate::web_input::wheel(dx, dy, delta_mode, [host.size.w, host.size.h]);
        host.hear(Event::Move(pt(x, y)));
        let zoom = mods & crate::web_input::CTRL != 0;
        host.hear(if zoom { Event::Zoom(crate::touch::wheel_zoom(-down / crate::web_input::NOTCH)) } else { Event::Wheel(right, down) });
        host.schedule();
    });
}

/// A key went down (`down`, or repeats) or up, by its DOM `key` and `code`; `field` says the
/// text field has the keyboard (its input events carry what is typed). Answers 1 where the
/// browser should not act on it too.
#[unsafe(no_mangle)]
pub extern "C" fn taut_key(down: u32, key: *mut u8, key_length: usize, code: *mut u8, code_length: usize, mods: u32, field: u32) -> u32 {
    let (name, code) = (given_text(key, key_length), given_text(code, code_length));
    let held = crate::web_input::mods(mods);
    let key = crate::web_input::key(&name, &code, held);
    let mut stops = false;
    with_host(|host| {
        host.hear(Event::ModifiersChanged(held));
        let Some(key) = key else { return };
        if down == 0 {
            host.hear(Event::KeyUp(key));
            return host.schedule();
        }
        host.hear(Event::Key(key, held));
        if let Some(text) = crate::web_input::typed(&name, held).filter(|_| field == 0) {
            host.hear(Event::Text(text.into()));
        }
        stops = crate::web_input::stops(Some(key), held);
        now_and_later(host);
    });
    stops as u32
}

/// Text typed into the text field, or committed by an input method.
#[unsafe(no_mangle)]
pub extern "C" fn taut_text(text: *mut u8, length: usize) {
    let text = given_text(text, length);
    with_host(|host| {
        if !text.is_empty() {
            host.hear(Event::Text(text));
        }
        now_and_later(host);
    });
}

/// The system clipboard's text, from the browser's paste event: kept as the clipboard's, then
/// Control+V is pressed.
#[unsafe(no_mangle)]
pub extern "C" fn taut_paste(text: *mut u8, length: usize) {
    CLIPBOARD.set(given_text(text, length));
    with_host(|host| {
        host.hear(Event::Key(Key::Char('v'), crate::event::Mods::CTRL));
        now_and_later(host);
    });
}

/// The system clipboard's text, read as the page gained the keyboard (where the browser lets the
/// page read it without asking): what a menu's Paste pastes.
#[unsafe(no_mangle)]
pub extern "C" fn taut_clipboard(text: *mut u8, length: usize) {
    CLIPBOARD.set(given_text(text, length));
}

/// The page gained (1) or lost the keyboard.
#[unsafe(no_mangle)]
pub extern "C" fn taut_focus(on: u32) {
    with_host(|host| {
        host.hear(Event::Focused(on != 0));
        host.schedule();
    });
}

/// A screen reader acted on a node's element (its key as the mirror wrote it): 0 pressed it,
/// 1 moved to it (it is shown, and takes the keyboard where it can).
#[unsafe(no_mangle)]
pub extern "C" fn taut_reader(key: *mut u8, length: usize, action: u32) {
    let Some(id) = crate::web_mirror::id_of(&given_text(key, length)) else { return };
    with_host(|host| {
        match action {
            0 => host.ui.press_for_reader(id),
            _ => host.ui.show_for_reader(id),
        }
        host.rerun = true;
        now_and_later(host);
    });
}

/// A file the person picked (its name and bytes), or none (`length` of the name 0: the picker
/// was closed). (Several files, a folder or where to save come through `web_files`.)
#[unsafe(no_mangle)]
pub extern "C" fn taut_file(name: *mut u8, name_length: usize, bytes: *mut u8, length: usize) {
    let name = given_text(name, name_length);
    let bytes = given(bytes, length);
    let file = (!name.is_empty()).then(|| PathBuf::from("/picked").join(name));
    if let Some(file) = &file {
        keep_file(file.clone(), bytes);
    }
    with_host(|host| {
        host.ui.file_dialog_answered(file);
        host.rerun = true;
        host.schedule();
    });
}

/// The loader heard something for a part (see `parts`): what kind of thing, where, a number and
/// bytes. Answers the part's number (0 for a part not in use).
#[unsafe(no_mangle)]
pub extern "C" fn taut_heard(part: u32, what: u32, x: f32, y: f32, number: f64, bytes: *mut u8, length: usize) -> u32 {
    let bytes = given(bytes, length);
    let Some(part) = PARTS.get().get(part as usize).copied().flatten() else { return 0 };
    let mut answer = 0;
    with_host(|host| {
        let (then, said) = (part.heard)(&mut host.ui, Heard { what, at: pt(x, y), number, bytes });
        (answer, host.rerun) = (said, host.rerun || then != Then::Wait);
        match then {
            Then::RunNow => now_and_later(host),
            _ => host.schedule(),
        }
    });
    answer
}

/// The page closes: the app hears it as its window's close (and may keep what it must).
#[unsafe(no_mangle)]
pub extern "C" fn taut_close() {
    with_host(|host| {
        host.ui.request_close();
        host.rerun = true;
        host.describe();
    });
}
