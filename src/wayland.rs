//! Linux under Wayland: windows the compositor shows, spoken to over its socket in the
//! protocol's bytes (`wayland_wire`). Text is as under X (FreeType with Fontconfig's fonts,
//! `x11::SystemText`, which needs no X server), and so are screen readers and the desktop
//! portal over D-Bus (`atspi`, `portal`).
//!
//! Choice: Wayland where `WAYLAND_DISPLAY` names a compositor that answers and shows windows;
//! else X (also shown on Wayland desktops through XWayland). `TAUT_BACKEND=x11` or
//! `TAUT_BACKEND=wayland` overrides, as `GDK_BACKEND` does for GTK.
//!
//! One loop on one connection runs all windows, as under X: the first window's interface runs
//! whatever window something happens in, and describes the others. A window is an `xdg-shell`
//! toplevel. Its pixels go into shared memory; the compositor is told which places changed, and
//! a window redraws only when something changed, at most once per compositor frame (a frame
//! callback gates the next commit). A window has one buffer, and a second only while the
//! compositor still holds the first when something must be drawn: that one is brought up to
//! date by redrawing what changed since it was last committed. (Under Wayland the program keeps
//! its pixels, where X keeps them for it: so windows draw straight into the buffer, with no
//! title bar.)
//!
//! Decorations are the compositor's where it offers them (`xdg-decoration`); else the library
//! draws its own title bar over the window (`decorations`), as a subsurface. Bare and
//! see-through windows have neither.
//!
//! Optional protocols are used where offered: cursor shapes by name (`cursor-shape`, else the
//! desktop cursor theme via `xcursor`), fractional scale (`fractional-scale` with
//! `viewporter`), activation (`xdg-activation`), the window icon (`xdg-toplevel-icon`), an
//! exported handle for portal dialogs (`xdg-foreign`), input methods (`text-input` v3); and
//! with the feature `touch`, touchpad pinches (`pointer-gestures`) and pens (`tablet` v2).
//! Touch (`wl_touch`) reaches every program: no compositor emulates a mouse from it, as X and
//! Windows do. So without the feature, the first finger is made the mouse here; with it,
//! fingers are taken as such (`touch`).
//!
//! Compositor differences are handled as they come: one that draws no decorations (Mutter,
//! Weston) gets the library's title bar; one that reports a window's output only once it
//! shows (Mutter, Sway, Weston) gets it first drawn at the outputs' largest scale; one that
//! puts a new window behind the active one (Mutter) is asked to activate a second window as it
//! maps, and the first with the startup token the desktop gave; one that leaves a window to
//! pick its size when unmaximized (Weston) gets the size it had.
//!
//! What no program can do under Wayland, by design: know or set its window's screen position,
//! stay above other programs, or come to the front unless the compositor agrees (it is asked,
//! with the last input serial as reason). So a window's place for its app
//! ([`Ui::window_placement`]) is where the app asked it to open, unchanged: a place kept one
//! day still works on a day it runs under X.
use crate::atspi::{Bus, Message as BusMessage};
use crate::canvas::Canvas;
use crate::color::Color;
use crate::decorations::{self, Asked, TitleBar};
use crate::event::{Button, Cursor, Event, Mods};
use crate::file_dialog::FileDialogRequest;
use crate::frameless::HitTest;
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::id::Id;
use crate::keymap::Keyboard;
use crate::platform::{Clipboard, PicturesAndFiles, Window, WindowPlacement};
use crate::preferences::Preferences;
use crate::text::{Font, Metrics, TextSystem};
use crate::touch::{Stylus, Touch, TouchPhase};
use crate::ui::Ui;
use crate::wayland_wire::{Arg, Interface, Message, Shared, Wire};
use crate::x11::{MOST, PollFd, Session, SystemText, WOKEN, decimal, files_in, lists_of, pipe_ends, poll, read};
use std::cell::{Cell, RefCell};
use std::ffi::c_int;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// MIME types text is offered and asked for as, the preferred first.
const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];
/// A picture as PNG; files as a URI list; the same list as file managers pass it.
const PICTURE: &str = "image/png";
const FILES: &str = "text/uri-list";
const COPIED_FILES: &str = "x-special/gnome-copied-files";

/// The `wl_shm` formats every compositor supports: ARGB8888 and XRGB8888.
const SEE_THROUGH: u32 = 0;
const SOLID: u32 = 1;

/// Mouse buttons, by their kernel codes.
const LEFT: u32 = 0x110;
const RIGHT: u32 = 0x111;
const MIDDLE: u32 = 0x112;

/// The eraser tool type; and the pen's side buttons, by kernel codes.
const RUBS_OUT: u32 = 0x141;
const SIDE: [u32; 3] = [0x14b, 0x14c, 0x149];

/// The scroll distance of a wheel notch, in units (as under X); and what the compositor counts
/// a notch as in value120 steps.
const NOTCH: f32 = 48.0;
const STEPS: f32 = 120.0;

/// Two presses on a window's title bar within this many ms are a double press.
const DOUBLE_PRESS: u32 = 500;

/// `xdg_toplevel` states: maximized, fullscreen, activated, suspended (as when minimized: the
/// compositor says no more).
const FILLING: u32 = 1;
const WHOLE: u32 = 2;
const ACTIVE: u32 = 4;
const OUT_OF_SIGHT: u32 = 9;

/// Decoration modes: client-side (1) or server-side (2).
const BY_THE_WINDOW: u32 = 1;
const BY_THE_COMPOSITOR: u32 = 2;

/// The globals the compositor offers that this uses: the program's object for each (0 where not
/// offered), and some versions.
#[derive(Default)]
struct Has {
    registry: u32,
    compositor: u32,
    compositor_version: u32,
    subcompositor: u32,
    shm: u32,
    shell: u32,
    seat: u32,
    seat_version: u32,
    data: u32,
    data_version: u32,
    decorations: u32,
    shapes: u32,
    viewporter: u32,
    scales: u32,
    activation: u32,
    exporter: u32,
    icons: u32,
    text_inputs: u32,
    gestures: u32,
    tablets: u32,
    /// `zwp_linux_dmabuf_v1`: its global name and version, bound only for a window the GPU
    /// shows (feature `gpu`).
    #[cfg(feature = "gpu")]
    dmabuf: (u32, u32),
    /// `wp_color_manager_v1` (feature `hdr`).
    #[cfg(feature = "hdr")]
    colors: u32,
}

/// What all windows and the clipboard share: the connection, the globals, the clipboard, and
/// the last input serial (which the compositor wants whenever the program asks for something
/// only a person should start: setting the selection, moving a window, activation). And the
/// GPU, where it shows windows (feature `gpu`).
struct Link {
    wire: Wire,
    has: Has,
    clipboard: RefCell<ClipboardState>,
    serial: Cell<u32>,
    #[cfg(feature = "gpu")]
    gpu: RefCell<Option<gpu_wayland::Gpu>>,
}

/// The Wayland clipboard: whoever copied last offers it in some MIME types and hands it to any
/// program that asks.
#[derive(Default)]
struct ClipboardState {
    /// The seat's data device (0: none).
    device: u32,
    /// What was copied here while it is the selection: each type it is offered as with its
    /// bytes; and its data source.
    held: Vec<(&'static str, Rc<[u8]>)>,
    source: u32,
    /// Other programs' data offers, as announced: each offer and its MIME types.
    offers: Vec<(u32, Vec<String>)>,
    /// The offer that is the selection now (0: none).
    selection: u32,
}

/// Which window system a program's windows use: the Wayland compositor that answered (and what
/// it answered), X, or neither (Wayland asked for, none answers).
enum Chosen<T> {
    Wayland(T),
    X,
    Neither,
}

/// Chooses by `TAUT_BACKEND` (if set) and whether a compositor answers (asked only where it
/// matters).
fn choose<T>(asked: Option<&str>, answer: impl FnOnce() -> Option<T>) -> Chosen<T> {
    let asked = asked.map(str::to_ascii_lowercase);
    match (asked.as_deref(), asked.as_deref() == Some("x11")) {
        (_, true) => Chosen::X,
        (Some("wayland"), _) => answer().map_or(Chosen::Neither, Chosen::Wayland),
        _ => answer().map_or(Chosen::X, Chosen::Wayland),
    }
}

/// Waits up to so many ms for a file to become readable.
fn readable(fd: c_int, millis: c_int) -> bool {
    let mut waiting = PollFd { fd, events: 1, revents: 0 };
    // SAFETY: one file to wait on, for no longer than said.
    unsafe { poll(&mut waiting, 1, millis) > 0 }
}

/// A connection to a compositor that shows windows, and its outputs: each the program's object
/// and the compositor's global name.
type Linked = (Link, Vec<(u32, u32)>);

impl Link {
    /// Connects to the compositor the environment names and binds its globals. Nothing where
    /// none answers, or it shows no windows. (With the outputs, as global name and the
    /// program's object.)
    fn new() -> Option<Linked> {
        Link::over(Wire::connect()?)
    }

    /// The same over an existing connection.
    fn over(wire: Wire) -> Option<Linked> {
        let registry = wire.make(Interface::Registry);
        wire.send(1, 1, &[Arg::Uint(registry)]);
        let offered = Link::offered(&wire)?;
        let (has, outputs) = bind(&wire, registry, &offered);
        if has.compositor == 0 || has.shm == 0 || has.shell == 0 {
            return None;
        }
        let link = Link {
            wire,
            has,
            clipboard: RefCell::default(),
            serial: Cell::new(0),
            #[cfg(feature = "gpu")]
            gpu: RefCell::default(),
        };
        Some((link, outputs))
    }

    /// The registry's globals: name, interface and version each. Nothing where it says nothing
    /// within five seconds, or reports an error.
    fn offered(wire: &Wire) -> Option<Vec<(u32, String, u32)>> {
        let (synced, until, mut offered) = (wire.make(Interface::Callback), Instant::now() + Duration::from_secs(5), Vec::new());
        wire.send(1, 0, &[Arg::Uint(synced)]);
        wire.flush();
        while Instant::now() < until {
            if readable(wire.fd(), 100) && !wire.read() {
                return None;
            }
            let ended = std::iter::from_fn(|| wire.next()).find_map(|message| listed(wire, message, synced, &mut offered));
            if let Some(fine) = ended {
                return fine.then_some(offered);
            }
        }
        None
    }

    /// The last round trip, as the program hangs up: a `sync`, and whatever the compositor says
    /// until it answers, taken in and let be (the windows are gone), for no longer than `most`.
    /// A compositor that finds a client's socket closed with words of its own still unread sees
    /// the connection reset, and logs that as an error; one that finds them read sees the
    /// program hang up. (The callback's `delete_id` comes in the same write as its answer, and
    /// is read with it.)
    fn part(&self, most: Duration) {
        let wire = &self.wire;
        if wire.broken.get() {
            return;
        }
        let (synced, until) = (wire.make(Interface::Callback), Instant::now() + most);
        wire.send(1, 0, &[Arg::Uint(synced)]);
        wire.flush();
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if !readable(wire.fd(), left.as_millis().clamp(1, 1000) as c_int) {
                continue;
            }
            if !wire.read() {
                return;
            }
            // (All that came is off the socket now, which is what the compositor sees; the
            // answer may be among it.)
            if std::iter::from_fn(|| wire.next()).any(|message| message.object == synced && message.opcode == 0) {
                return;
            }
        }
    }

    /// The clipboard content in the first of `kinds` it offers; nothing if none. What was
    /// copied here is at hand, and nobody is asked.
    fn get(&self, kinds: &[&str]) -> Option<Vec<u8>> {
        let clipboard = self.clipboard.borrow();
        if !clipboard.held.is_empty() {
            return kinds.iter().find_map(|kind| clipboard.held.iter().find(|(held, _)| held == kind)).map(|(_, bytes)| bytes.to_vec());
        }
        let offer = clipboard.selection;
        let kind = kinds.iter().find(|kind| clipboard.offers.iter().any(|(id, has)| *id == offer && has.iter().any(|has| has == *kind)))?;
        drop(clipboard);
        self.receive(offer, kind)
    }

    /// Asks the offering program for one MIME type through a pipe and reads it all. Five
    /// seconds for that, as under X; more than `MOST` is no answer.
    fn receive(&self, offer: u32, kind: &str) -> Option<Vec<u8>> {
        let (out, into) = crate::wayland_wire::pipe()?;
        self.wire.send(offer, 1, &[Arg::Text(kind), Arg::File(into.as_raw_fd())]);
        drop(into);
        let (until, mut file, mut all) = (Instant::now() + Duration::from_secs(5), std::fs::File::from(out), Vec::new());
        let mut room = vec![0; 65536];
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if !readable(file.as_raw_fd(), left.as_millis() as c_int + 1) {
                return None;
            }
            match std::io::Read::read(&mut file, &mut room) {
                Ok(0) => return Some(all),
                Ok(got) if all.len() + got <= MOST => all.extend_from_slice(&room[..got]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                _ => return None,
            }
        }
        None
    }

    /// Puts something on the clipboard, replacing everything: in each type offered, with its
    /// bytes. The compositor only hears the types; the bytes stay here until asked for. (The
    /// previous data source goes when the compositor cancels it, as for every copy.)
    fn set(&self, kinds: Vec<(&'static str, Rc<[u8]>)>) {
        let mut clipboard = self.clipboard.borrow_mut();
        let (wire, device) = (&self.wire, clipboard.device);
        (clipboard.source, clipboard.held) = (0, kinds);
        if device == 0 {
            return; // no seat: the copy stays inside the program
        }
        let source = wire.make(Interface::DataSource);
        wire.send(self.has.data, 0, &[Arg::Uint(source)]);
        clipboard.held.iter().for_each(|(kind, _)| wire.send(source, 0, &[Arg::Text(kind)]));
        wire.send(device, 1, &[Arg::Object(source), Arg::Uint(self.serial.get())]);
        wire.flush();
        clipboard.source = source;
    }

    /// Another program asks for what was copied here, in one type, through a pipe: a thread of
    /// its own writes it and ends when done (or the reader stops), so no window waits.
    fn hand_over(&self, kind: &str, file: Option<OwnedFd>) {
        let bytes = self.clipboard.borrow().held.iter().find(|(held, _)| *held == kind).map(|(_, bytes)| bytes.to_vec());
        let (Some(file), Some(bytes)) = (file, bytes) else { return };
        std::thread::spawn(move || {
            crate::wayland_wire::no_signal_for_pipes();
            std::io::Write::write_all(&mut std::fs::File::from(file), &bytes)
        });
    }

    /// The compositor says this program's data source is no longer the selection: another copy
    /// replaced it (another program's, or a later one of this program's).
    fn cancelled(&self, source: u32) {
        let mut clipboard = self.clipboard.borrow_mut();
        self.wire.send(source, 1, &[]);
        if clipboard.source == source {
            (clipboard.source, clipboard.held) = (0, Vec::new());
        }
    }

    /// Whether an offer has a MIME type.
    fn offers(&self, offer: u32, kind: &str) -> bool {
        self.clipboard.borrow().offers.iter().any(|(id, has)| *id == offer && has.iter().any(|has| has == kind))
    }

    /// Destroys an offer and forgets it.
    fn forget(&self, offer: u32) {
        let mut clipboard = self.clipboard.borrow_mut();
        if offer != 0 && clipboard.offers.iter().any(|(id, _)| *id == offer) {
            self.wire.send(offer, 2, &[]);
            self.wire.note(offer, Interface::Free);
            clipboard.offers.retain(|(id, _)| *id != offer);
        }
    }
}

/// One registry event while listing globals: a global (kept), the `sync` callback `synced` that
/// ends the list (true), a protocol error (false), or something else (nothing).
fn listed(wire: &Wire, mut message: Message, synced: u32, offered: &mut Vec<(u32, String, u32)>) -> Option<bool> {
    match (message.of, message.opcode) {
        (Interface::Registry, 0) => offered.push((message.uint(), message.text(), message.uint())),
        (Interface::Callback, 0) if message.object == synced => return Some(true),
        (Interface::Display, 1) => wire.deleted(message.uint()),
        (Interface::Display, 0) => return Some(false),
        _ => {}
    }
    None
}

/// Binds the globals this uses, each at the newest version both support up to the one this was
/// written for: one of each, and every output. Says what it has, and the outputs (object and
/// global name each).
fn bind(wire: &Wire, registry: u32, offered: &[(u32, String, u32)]) -> (Has, Vec<(u32, u32)>) {
    let (mut has, mut outputs) = (Has { registry, ..Has::default() }, Vec::new());
    for (name, interface, version) in offered {
        let bind = |most: u32, of: Interface| wire.bind(registry, *name, interface, (*version).min(most), of);
        match interface.as_str() {
            "wl_compositor" if has.compositor == 0 => (has.compositor, has.compositor_version) = (bind(6, Interface::Compositor), (*version).min(6)),
            "wl_subcompositor" if has.subcompositor == 0 => has.subcompositor = bind(1, Interface::Subcompositor),
            "wl_shm" if has.shm == 0 => has.shm = bind(1, Interface::Shm),
            "xdg_wm_base" if has.shell == 0 => has.shell = bind(6, Interface::WmBase),
            "wl_seat" if has.seat == 0 => (has.seat, has.seat_version) = (bind(8, Interface::Seat), (*version).min(8)),
            "wl_data_device_manager" if has.data == 0 => (has.data, has.data_version) = (bind(3, Interface::DataManager), (*version).min(3)),
            "zxdg_decoration_manager_v1" if has.decorations == 0 => has.decorations = bind(1, Interface::Decorations),
            "wp_cursor_shape_manager_v1" if has.shapes == 0 => has.shapes = bind(1, Interface::Shapes),
            "wp_viewporter" if has.viewporter == 0 => has.viewporter = bind(1, Interface::Viewporter),
            "wp_fractional_scale_manager_v1" if has.scales == 0 => has.scales = bind(1, Interface::Scales),
            "xdg_activation_v1" if has.activation == 0 => has.activation = bind(1, Interface::Activation),
            "zxdg_exporter_v2" if has.exporter == 0 => has.exporter = bind(1, Interface::Exporter),
            "xdg_toplevel_icon_manager_v1" if has.icons == 0 => has.icons = bind(1, Interface::Icons),
            "zwp_text_input_manager_v3" if has.text_inputs == 0 => has.text_inputs = bind(1, Interface::TextInputs),
            // Pinches and pens only for a program taking them as such, with the feature
            // `touch`: for any other, the compositor emulates a mouse from a pen.
            "zwp_pointer_gestures_v1" if cfg!(feature = "touch") && has.gestures == 0 => has.gestures = bind(1, Interface::Gestures),
            "zwp_tablet_manager_v2" if cfg!(feature = "touch") && has.tablets == 0 => has.tablets = bind(1, Interface::Tablets),
            "wl_output" if *version >= 2 => outputs.push((bind(4, Interface::Output), *name)),
            #[cfg(feature = "gpu")]
            "zwp_linux_dmabuf_v1" => has.dmabuf = (*name, *version),
            #[cfg(feature = "hdr")]
            "wp_color_manager_v1" if has.colors == 0 => has.colors = bind(1, Interface::ColorManager),
            _ => {}
        }
    }
    // Fractional scale needs both: the scale, and a viewport to show a surface at another size
    // than its pixels.
    if has.viewporter == 0 {
        has.scales = 0;
    }
    (has, outputs)
}

/// The Wayland clipboard, for an interface.
struct SystemClipboard(Rc<Link>);

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        let text = self.0.get(&TEXT)?;
        Some(String::from_utf8(text).unwrap_or_else(|other| String::from_utf8_lossy(other.as_bytes()).into_owned()))
    }
    fn set(&mut self, text: &str) {
        let bytes: Rc<[u8]> = text.as_bytes().into();
        self.0.set(TEXT.iter().map(|kind| (*kind, bytes.clone())).collect());
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
    /// The windows' drop handling: none until an interface on this thread asks about drops (see
    /// `Drops`).
    static DROPS: Cell<Option<Drops>> = const { Cell::new(None) };
}

/// Hands the clipboard its picture and file code, on this thread (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    let (picture, set_picture) = (SystemClipboard::picture_on, SystemClipboard::put_picture);
    PICTURES_AND_FILES.set(Some(PicturesAndFiles { picture, set_picture, files: SystemClipboard::files_on, set_files: SystemClipboard::put_files }));
}

impl SystemClipboard {
    /// A picture, as PNG.
    fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        crate::png::decode(&self.0.get(&[PICTURE])?)
    }
    /// Puts a picture on the clipboard as PNG.
    fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
        self.0.set(vec![(PICTURE, png.into())]);
    }
    /// The files as a URI list; or, from a file manager offering nothing else, its own list.
    fn files_on(&mut self) -> Vec<PathBuf> {
        self.0.get(&[FILES, COPIED_FILES]).map_or(Vec::new(), |list| files_in(&list))
    }
    /// Puts files on the clipboard both ways: some file managers paste only the second.
    fn put_files(&mut self, files: &[PathBuf]) {
        let [list, copied] = lists_of(files);
        self.0.set(vec![(FILES, list.into()), (COPIED_FILES, copied.into())]);
    }
}

/// The system text, shared by every interface of the program, the windows' and their title
/// strips'. (Fonts open once; each glyph rasterized once.)
#[derive(Clone)]
struct SharedText(Rc<RefCell<SystemText>>);

impl TextSystem for SharedText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.0.borrow_mut().metrics(font, scale)
    }
    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.0.borrow_mut().measure(text, font, scale, stops)
    }
    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        self.0.borrow_mut().draw(canvas, text, x, y, font, color)
    }
    #[cfg(feature = "shaping")]
    fn set_language(&mut self, language: crate::scripts::Language) {
        self.0.borrow_mut().set_language(language)
    }
    #[cfg(feature = "shaping")]
    fn draw_glyphs(&mut self, canvas: &mut Canvas, font: Font, letter: (char, u16), glyphs: &[(u16, [f32; 2])], pen: [i32; 2], color: Color) -> bool {
        self.0.borrow_mut().draw_glyphs(canvas, font, letter, glyphs, pen, color)
    }
}

/// A `wl_shm` buffer of pixels, so many wide and high. The compositor holds it from commit
/// until it releases it (`busy`); the places changed since it was last drawn into are what it
/// lacks.
struct Buffer {
    id: u32,
    pool: u32,
    memory: Shared,
    size: [i32; 2],
    busy: bool,
    missing: Vec<Rect>,
}

impl Buffer {
    /// A buffer so many pixels big, all of it lacking. Nothing where no memory is to be had.
    fn new(link: &Link, size: [i32; 2], clear: bool) -> Option<Self> {
        let len = size[0] as usize * size[1] as usize * 4;
        let memory = Shared::new(len)?;
        let (wire, pool, id) = (&link.wire, link.wire.make(Interface::Pool), link.wire.make(Interface::Buffer));
        wire.send(link.has.shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(len as i32)]);
        let format = if clear { SEE_THROUGH } else { SOLID };
        wire.send(pool, 0, &[Arg::Uint(id), Arg::Int(0), Arg::Int(size[0]), Arg::Int(size[1]), Arg::Int(size[0] * 4), Arg::Uint(format)]);
        let all = rect(0.0, 0.0, f32::MAX, f32::MAX);
        Some(Buffer { id, pool, memory, size, busy: false, missing: vec![all] })
    }

    /// Destroys the buffer and frees its memory.
    fn destroy(self, link: &Link) {
        link.wire.send(self.id, 0, &[]);
        link.wire.send(self.pool, 1, &[]);
    }
}

/// A surface the library draws: a window's, or its title bar's. It knows its buffers, its
/// size, what changed since the last commit, and whether the compositor has shown it yet.
struct Sheet {
    surface: u32,
    /// Its viewport, where the scale is fractional (0: none).
    viewport: u32,
    buffers: Vec<Buffer>,
    /// Its size in units and in pixels; and the integer scale it was last given.
    units: [i32; 2],
    pixels: [i32; 2],
    whole: i32,
    /// The pending frame callback (0: none).
    waiting: u32,
    /// The places to redraw, in units.
    changed: Vec<Rect>,
    /// The next draw commits at once, without waiting for the frame callback: the compositor
    /// waits for a resized window. And a commit is owed whether or not anything changed: the
    /// compositor waits for one after a configure.
    hurry: bool,
    owed: bool,
    /// See-through: drawn on a clear background.
    clear: bool,
    /// Shown through the GPU, or by shared memory (feature `gpu`).
    #[cfg(feature = "gpu")]
    way: gpu_wayland::Way,
    /// Its HDR (feature `hdr`).
    #[cfg(feature = "hdr")]
    lit: hdr_wayland::Lit,
}

impl Sheet {
    /// A new, empty surface.
    fn new(link: &Link, clear: bool) -> Self {
        let surface = link.wire.make(Interface::Surface);
        link.wire.send(link.has.compositor, 0, &[Arg::Uint(surface)]);
        Sheet {
            surface,
            viewport: 0,
            buffers: Vec::new(),
            units: [0; 2],
            pixels: [0; 2],
            whole: 1,
            waiting: 0,
            changed: Vec::new(),
            hurry: false,
            owed: false,
            clear,
            #[cfg(feature = "gpu")]
            way: gpu_wayland::Way::Unasked,
            #[cfg(feature = "hdr")]
            lit: Default::default(),
        }
    }

    /// Shows it at a fractional scale from now on.
    fn view(&mut self, link: &Link) {
        if link.has.viewporter != 0 && self.viewport == 0 {
            self.viewport = link.wire.make(Interface::Viewport);
            link.wire.send(link.has.viewporter, 1, &[Arg::Uint(self.viewport), Arg::Uint(self.surface)]);
        }
    }

    /// Gives it a size in units at a scale. Buffers of another size go as the compositor
    /// releases them; all of it is to be drawn and committed at once.
    fn size(&mut self, link: &Link, units: [i32; 2], scale: f32) {
        let pixels = units.map(|side| ((side as f32 * scale).round() as i32).clamp(1, 32_767));
        if (units, pixels) == (self.units, self.pixels) {
            return;
        }
        (self.units, self.pixels, self.hurry) = (units, pixels, true);
        self.changed = vec![rect(0.0, 0.0, units[0] as f32, units[1] as f32)];
        // An integer scale is the surface's buffer scale; a fractional one is set by the
        // viewport's destination size in units, whatever its pixels.
        if self.viewport != 0 {
            link.wire.send(self.viewport, 2, &[Arg::Int(units[0]), Arg::Int(units[1])]);
        } else if std::mem::replace(&mut self.whole, scale as i32) != scale as i32 && link.has.compositor_version >= 3 {
            link.wire.send(self.surface, 8, &[Arg::Int(scale as i32)]);
        }
        let (old, kept): (Vec<Buffer>, Vec<Buffer>) = std::mem::take(&mut self.buffers).into_iter().partition(|buffer| !buffer.busy && buffer.size != pixels);
        old.into_iter().for_each(|buffer| buffer.destroy(link));
        self.buffers = kept;
    }

    /// The compositor released one of the buffers. Says whether it was this surface's. (One of
    /// a size the surface no longer has is destroyed.)
    fn released(&mut self, link: &Link, id: u32) -> bool {
        #[cfg(feature = "hdr")]
        if self.lit.released(link, id, self.pixels) {
            return true;
        }
        let Some(at) = self.buffers.iter().position(|buffer| buffer.id == id) else { return false };
        self.buffers[at].busy = false;
        if self.buffers[at].size != self.pixels {
            self.buffers.remove(at).destroy(link);
        }
        true
    }

    /// A buffer to draw into: one not held by the compositor, of the surface's size, or a new
    /// one if there are fewer than two. Nothing where the surface must wait.
    fn free(&mut self, link: &Link) -> Option<usize> {
        if let Some(at) = self.buffers.iter().position(|buffer| !buffer.busy && buffer.size == self.pixels) {
            return Some(at);
        }
        if self.buffers.len() >= 2 {
            return None;
        }
        self.buffers.push(Buffer::new(link, self.pixels, self.clear)?);
        Some(self.buffers.len() - 1)
    }

    /// Draws what changed, if anything did and the compositor is ready, and commits: a buffer
    /// is brought up to date with `paint` where it lacks, damage is reported since what the
    /// compositor shows, and a frame callback asked. Says whether anything was committed.
    fn draw(&mut self, link: &Link, scale: f32, paint: &mut dyn FnMut(&mut Canvas)) -> bool {
        if self.changed.is_empty() || (self.waiting != 0 && !self.hurry) {
            return self.pay(link);
        }
        #[cfg(feature = "hdr")]
        if let Some(drawn) = self.draw_lit(link, scale, paint) {
            return drawn;
        }
        #[cfg(feature = "gpu")]
        if let Some(drawn) = self.draw_by_gpu(link, scale, paint) {
            return drawn;
        }
        let Some(at) = self.free(link) else { return self.pay(link) };
        let (changed, clear) = (std::mem::take(&mut self.changed), self.clear);
        let buffer = &mut self.buffers[at];
        let mut areas = std::mem::take(&mut buffer.missing);
        areas.extend_from_slice(&changed);
        if areas.len() > 16 {
            areas = vec![areas.iter().fold(Rect::EMPTY, |all, area| all.union(*area))];
        }
        let units = rect(0.0, 0.0, self.units[0] as f32, self.units[1] as f32);
        areas.iter().map(|area| area.intersect(units)).filter(|area| !area.is_empty()).for_each(|area| paint_area(buffer, area, scale, clear, paint));
        let wire = &link.wire;
        wire.send(self.surface, 1, &[Arg::Object(buffer.id), Arg::Int(0), Arg::Int(0)]);
        for area in &changed {
            let [left, top, right, bottom] = on_pixels(area.intersect(units), scale, buffer.size);
            wire.send(self.surface, 9, &[Arg::Int(left), Arg::Int(top), Arg::Int(right - left), Arg::Int(bottom - top)]);
        }
        buffer.busy = true;
        self.waiting = wire.make(Interface::Callback);
        wire.send(self.surface, 3, &[Arg::Uint(self.waiting)]);
        wire.send(self.surface, 6, &[]);
        (self.hurry, self.owed) = (false, false);
        // Every other buffer lacks what changed now.
        let others = self.buffers.iter_mut().enumerate().filter(|(other, _)| *other != at);
        others.for_each(|(_, buffer)| buffer.missing.extend_from_slice(&changed));
        true
    }

    /// Commits with nothing new where a commit is owed. Says whether it did.
    fn pay(&mut self, link: &Link) -> bool {
        if std::mem::take(&mut self.owed) {
            link.wire.send(self.surface, 6, &[]);
        }
        false
    }

    /// Destroys the surface, its buffers and its viewport.
    fn destroy(self, link: &Link) {
        #[cfg(feature = "gpu")]
        Sheet::forget_gpu(link, self.way);
        #[cfg(feature = "hdr")]
        self.lit.destroy(link);
        if self.viewport != 0 {
            link.wire.send(self.viewport, 0, &[]);
        }
        link.wire.send(self.surface, 0, &[]);
        self.buffers.into_iter().for_each(|buffer| buffer.destroy(link));
    }
}

/// An area in units as whole buffer pixels: left, top, right, bottom.
fn on_pixels(area: Rect, scale: f32, [width, height]: [i32; 2]) -> [i32; 4] {
    let start = |at: f32, most: i32| ((at * scale).floor() as i32).clamp(0, most);
    let end = |at: f32, most: i32| ((at * scale).ceil() as i32).clamp(0, most);
    [start(area.x, width), start(area.y, height), end(area.right(), width), end(area.bottom(), height)]
}

/// Draws one area into a buffer: a canvas over the rows it covers.
fn paint_area(buffer: &mut Buffer, area: Rect, scale: f32, clear: bool, paint: &mut dyn FnMut(&mut Canvas)) {
    let [_, top, _, bottom] = on_pixels(area, scale, buffer.size);
    let width = buffer.size[0];
    if top >= bottom {
        return;
    }
    let rows = &mut buffer.memory.pixels()[(top * width) as usize..(bottom * width) as usize];
    let mut canvas = Canvas::new(rows, width as usize, (bottom - top) as usize, (0, top), scale);
    canvas.clip(area.snap_to_pixels(scale));
    if clear {
        canvas.erase();
    }
    paint(&mut canvas);
}

/// A window's kind, fixed when it was made.
struct Kind {
    /// Frameless: the app draws its own title bar and it is resized by its edges.
    frameless: bool,
    /// See-through; and it keeps its creation size unless also frameless.
    transparent: bool,
}

/// The library's own title bar for a window: the title bar, its surface, and the subsurface
/// placing it over the window.
struct Framed {
    title_bar: TitleBar,
    sheet: Sheet,
    sub: u32,
}

/// One window, and what keeps an interface shown in it.
struct Shown {
    /// The first window's interface's id for it, if a second window; and its title, the name
    /// screen readers get.
    known: Option<Id>,
    title: String,
    /// Its surface and the objects making it a window: `xdg_surface`, `xdg_toplevel`, the
    /// decoration, the fractional scale, and the exported handle (with its handle string, once
    /// given).
    sheet: Sheet,
    xdg: u32,
    toplevel: u32,
    decoration: u32,
    fraction: u32,
    exported: u32,
    handle: String,
    kind: Kind,
    /// Its size in units: now, and the last while not maximized (what it restores to). And
    /// where its app asked it to open.
    size: [i32; 2],
    normal: [i32; 2],
    at: (i32, i32),
    /// Its least size (its size, where it keeps it).
    least: [i32; 2],
    /// Pixels per unit: as finely as the compositor says (fractional), else its integer buffer
    /// scale, else from its outputs (by object).
    scale: f32,
    finer: Option<f32>,
    whole: Option<i32>,
    on: Vec<u32>,
    /// The pending configure until `configure` ends it: a size (zeros: the window picks) and
    /// its states.
    asked: ([i32; 2], Vec<u32>),
    /// The compositor configured it at least once.
    configured: bool,
    /// Whether it is maximized, fullscreen, activated, and suspended.
    filling: bool,
    lying_over: bool,
    active: bool,
    hidden: bool,
    /// Its decorations are to be the library's own (unless fullscreen); and that title bar,
    /// while it shows.
    framed: bool,
    frame: Option<Framed>,
    /// Something of it is the window's, not the interface's, as of the last run (a resize edge,
    /// a title bar); and which the pointer is over.
    grips: bool,
    over: HitTest,
    /// When and where its title bar was last pressed: a press soon after, close by, is a double
    /// press.
    pressed: Option<(u32, Point)>,
    cursor: Cursor,
    /// It is to minimize as soon as it maps (it opens so). It is to activate as soon as it
    /// maps: a second window, which a compositor may otherwise put behind the active one
    /// (Mutter does), and the first where the desktop gave it a token.
    minimize: bool,
    come: bool,
    /// Its icon, until the compositor has taken it.
    icon: Option<(Shared, u32, Vec<u32>)>,
    /// Its app hid it entirely to live in the tray ([`Ui::hide_window`]): unmapped until it
    /// comes forward.
    withdrawn: bool,
}

/// What a window opens as.
struct Opening<'a> {
    known: Option<Id>,
    title: &'a str,
    size: [i32; 2],
    min_size: [i32; 2],
    position: (i32, i32),
    frameless: bool,
    transparent: bool,
    maximized: bool,
    minimized: bool,
    icons: &'a [(usize, Vec<u32>)],
}

/// The program's name, by which the desktop groups its windows (the app id).
fn program() -> String {
    std::env::current_exe().ok().and_then(|path| path.file_stem().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default()
}

/// A title as long as one message takes, cut at a character.
fn titled(title: &str) -> &str {
    let mut end = title.len().min(crate::wayland_wire::LONGEST - 64);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    &title[..end]
}

impl Shown {
    /// Creates a window and asks the compositor to show it: title, app id, least size,
    /// decorations, and whether it opens maximized. Nothing is drawn until the compositor
    /// configures it.
    fn open(link: &Link, text: &SharedText, opening: Opening) -> Self {
        let (wire, has) = (&link.wire, &link.has);
        let mut sheet = Sheet::new(link, opening.transparent);
        let fraction = if has.scales != 0 { wire.make(Interface::Scale) } else { 0 };
        if fraction != 0 {
            wire.send(has.scales, 1, &[Arg::Uint(fraction), Arg::Uint(sheet.surface)]);
            sheet.view(link);
        }
        // A see-through window that is not frameless keeps its creation size.
        let (size, kind) = (opening.size, Kind { frameless: opening.frameless, transparent: opening.transparent });
        let least = if opening.transparent && !opening.frameless { opening.size } else { opening.min_size };
        let mut shown = Shown {
            known: opening.known,
            title: opening.title.into(),
            sheet,
            xdg: 0,
            toplevel: 0,
            decoration: 0,
            fraction,
            exported: 0,
            handle: String::new(),
            kind,
            size,
            normal: size,
            least,
            at: opening.position,
            scale: 1.0,
            finer: None,
            whole: None,
            on: Vec::new(),
            asked: ([0; 2], Vec::new()),
            configured: false,
            filling: false,
            lying_over: false,
            active: false,
            hidden: false,
            framed: false,
            frame: None,
            grips: false,
            over: HitTest::Client,
            pressed: None,
            cursor: Cursor::Arrow,
            minimize: opening.minimized,
            come: opening.known.is_some() && link.has.activation != 0,
            icon: None,
            withdrawn: false,
        };
        shown.role(link);
        shown.framed = !opening.frameless && !opening.transparent && shown.decoration == 0;
        shown.icon = pictured(link, shown.toplevel, opening.icons);
        if opening.maximized {
            wire.send(shown.toplevel, 9, &[]);
        }
        shown.frame_up(link, text);
        wire.send(shown.sheet.surface, 6, &[]);
        shown
    }

    /// Gives the surface its toplevel role: `xdg_surface` and `xdg_toplevel`, with title, app
    /// id, least size (and max where it keeps its size), decoration mode, and the exported
    /// handle. (Recreated as a tray-hidden window comes back: see `withdraw`.)
    fn role(&mut self, link: &Link) {
        let (wire, has) = (&link.wire, &link.has);
        let (xdg, toplevel) = (wire.make(Interface::XdgSurface), wire.make(Interface::Toplevel));
        wire.send(has.shell, 2, &[Arg::Uint(xdg), Arg::Uint(self.sheet.surface)]);
        wire.send(xdg, 1, &[Arg::Uint(toplevel)]);
        wire.send(toplevel, 2, &[Arg::Text(titled(&self.title))]);
        wire.send(toplevel, 3, &[Arg::Text(&program())]);
        let least = self.least;
        wire.send(toplevel, 8, &[Arg::Int(least[0]), Arg::Int(least[1])]);
        if self.kind.transparent && !self.kind.frameless {
            wire.send(toplevel, 7, &[Arg::Int(least[0]), Arg::Int(least[1])]);
        }
        let framed = !self.kind.frameless && !self.kind.transparent;
        let decoration = if has.decorations != 0 { wire.make(Interface::Decoration) } else { 0 };
        if decoration != 0 {
            wire.send(has.decorations, 1, &[Arg::Uint(decoration), Arg::Uint(toplevel)]);
            wire.send(decoration, 1, &[Arg::Uint(if framed { BY_THE_COMPOSITOR } else { BY_THE_WINDOW })]);
        }
        let exported = if has.exporter != 0 { wire.make(Interface::Exported) } else { 0 };
        if exported != 0 {
            wire.send(has.exporter, 1, &[Arg::Uint(exported), Arg::Uint(self.sheet.surface)]);
        }
        (self.xdg, self.toplevel, self.decoration, self.exported, self.configured) = (xdg, toplevel, decoration, exported, false);
    }

    /// Takes the window off the screen for the tray ([`Ui::hide_window`]): its role objects are
    /// destroyed and its buffer detached, as other toolkits do. (A window a compositor was only
    /// given a null buffer for, it may never configure again: Weston does not.)
    fn withdraw(&mut self, link: &Link) {
        for object in [self.exported, self.decoration, self.toplevel, self.xdg].into_iter().filter(|object| *object != 0) {
            link.wire.send(object, 0, &[]);
        }
        (self.exported, self.decoration, self.toplevel, self.xdg) = (0, 0, 0, 0);
        (self.handle, self.configured, self.withdrawn) = (String::new(), false, true);
        link.wire.send(self.sheet.surface, 1, &[Arg::Nothing, Arg::Int(0), Arg::Int(0)]);
        link.wire.send(self.sheet.surface, 6, &[]);
    }

    /// Adds or removes the library's title bar: it shows while the decorations are the
    /// library's and the window is not fullscreen. The window geometry includes the title bar.
    fn frame_up(&mut self, link: &Link, text: &SharedText) {
        let wanted = self.framed && !self.lying_over;
        if wanted == self.frame.is_some() || link.has.subcompositor == 0 {
            return;
        }
        let wire = &link.wire;
        if let Some(framed) = self.frame.take() {
            wire.send(framed.sub, 0, &[]);
            framed.sheet.destroy(link);
            wire.send(self.xdg, 3, &[Arg::Int(0), Arg::Int(0), Arg::Int(self.size[0]), Arg::Int(self.size[1])]);
            return;
        }
        // Each new object is created as soon as numbered: the compositor takes no id out of
        // order.
        let mut sheet = Sheet::new(link, false);
        if self.fraction != 0 {
            sheet.view(link);
        }
        let sub = wire.make(Interface::Subsurface);
        wire.send(link.has.subcompositor, 1, &[Arg::Uint(sub), Arg::Uint(sheet.surface), Arg::Uint(self.sheet.surface)]);
        wire.send(sub, 1, &[Arg::Int(0), Arg::Int(-decorations::HIGH)]);
        let mut title_bar = TitleBar::new(Box::new(text.clone()));
        (title_bar.title, title_bar.filling, title_bar.active) = (self.title.clone(), self.filling, self.active);
        self.frame = Some(Framed { title_bar, sheet, sub });
        // Before the first configure nothing of the window has pixels, and a window geometry
        // over nothing is invalid: Mutter says so. It is set with the first pixels.
        if self.configured {
            self.bounds(link);
        }
    }

    /// Tells the compositor the window geometry: its own surface, and the title bar over it
    /// where it has one.
    fn bounds(&self, link: &Link) {
        let high = if self.frame.is_some() { decorations::HIGH } else { 0 };
        link.wire.send(self.xdg, 3, &[Arg::Int(0), Arg::Int(-high), Arg::Int(self.size[0]), Arg::Int(self.size[1] + high)]);
    }

    /// Sizes and scales the window's surfaces as the window is now.
    fn fit(&mut self, link: &Link) {
        self.sheet.size(link, self.size, self.scale);
        if let Some(framed) = &mut self.frame {
            framed.sheet.size(link, [self.size[0], decorations::HIGH], self.scale);
            framed.title_bar.ui.resize(Size { w: self.size[0] as f32, h: decorations::HIGH as f32 }, self.scale);
        }
    }

    /// Applies the configure the compositor sent, once it says it is done. A size of zeros is
    /// the window's to pick: it keeps its size, or restores the one from before it was
    /// maximized or fullscreen (as Weston leaves it to). The library's title bar is part of
    /// the size given.
    fn configure(&mut self, link: &Link, text: &SharedText) {
        let ([wide, high], states) = std::mem::take(&mut self.asked);
        let [filling, lying_over, active, hidden] = [FILLING, WHOLE, ACTIVE, OUT_OF_SIGHT].map(|state| states.contains(&state));
        if (self.filling || self.lying_over) && !filling && !lying_over {
            self.size = self.normal;
        }
        (self.filling, self.active, self.hidden) = (filling, active, hidden);
        if std::mem::replace(&mut self.lying_over, lying_over) != lying_over {
            self.frame_up(link, text);
        }
        let title_bar = if self.frame.is_some() { decorations::HIGH } else { 0 };
        // No screen or row of screens is near 16,384 units a side: a compositor saying more is
        // mistaken, and a buffer that size would be gigabytes.
        if wide > 0 && high > 0 {
            self.size = [wide.min(16_384), (high - title_bar).clamp(1, 16_384)];
        }
        if !filling && !lying_over {
            self.normal = self.size;
        }
        if let Some(framed) = &mut self.frame {
            (framed.title_bar.filling, framed.title_bar.active) = (filling, active);
        }
        self.bounds(link);
        self.fit(link);
        self.configured = true;
    }

    /// The scale to draw at now: `TAUT_SCALE` if set, else the finest the compositor says.
    /// Before the compositor says which output the window is on (most say so only once it
    /// shows), the largest output's, so a window is not first drawn blurred and then again. (A
    /// fractional scale is rounded up where the window cannot show one.)
    fn scale_now(&self, forced: Option<f32>, outputs: &[(u32, u32, i32)]) -> f32 {
        let on = self.on.iter().filter_map(|output| outputs.iter().find(|(id, ..)| id == output)).map(|(.., scale)| *scale).max();
        let on = on.or(outputs.iter().map(|(.., scale)| *scale).max());
        // No screen has more than eight pixels per unit: a larger scale is the compositor's
        // mistake, and text and pictures drawn at it would take all the memory there is.
        let scale = forced.or(self.finer).unwrap_or(self.whole.or(on).unwrap_or(1) as f32).clamp(0.25, 8.0);
        if self.sheet.viewport == 0 { scale.ceil() } else { scale }
    }

    /// Destroys the window and everything of it.
    fn close(self, link: &Link) {
        let wire = &link.wire;
        if let Some(framed) = self.frame {
            wire.send(framed.sub, 0, &[]);
            framed.sheet.destroy(link);
        }
        // A tray-hidden window has no role objects: see `withdraw`.
        for object in [self.exported, self.fraction, self.decoration, self.toplevel, self.xdg].into_iter().filter(|object| *object != 0) {
            wire.send(object, 0, &[]);
        }
        self.sheet.destroy(link);
        if let Some((_, pool, buffers)) = self.icon {
            buffers.into_iter().for_each(|buffer| wire.send(buffer, 0, &[]));
            wire.send(pool, 1, &[]);
        }
    }

    /// Frees the window's icon once the compositor has it.
    fn icon_taken(&mut self, link: &Link) {
        if let Some((_, pool, buffers)) = self.icon.take() {
            buffers.into_iter().for_each(|buffer| link.wire.send(buffer, 0, &[]));
            link.wire.send(pool, 1, &[]);
        }
    }
}

/// Hands the compositor a window's icon where it takes one: each square picture as a buffer in
/// one shared memory block, premultiplied as the compositor wants. They apply from the next
/// commit (the memory can go after that). Nothing where there are none or no room for them.
fn pictured(link: &Link, toplevel: u32, icons: &[(usize, Vec<u32>)]) -> Option<(Shared, u32, Vec<u32>)> {
    let square: Vec<&(usize, Vec<u32>)> = icons.iter().filter(|(width, pixels)| *width > 0 && pixels.len() == width * width).collect();
    if link.has.icons == 0 || square.is_empty() {
        return None;
    }
    let mut memory = Shared::new(square.iter().map(|(_, pixels)| pixels.len() * 4).sum())?;
    let (wire, pool, icon) = (&link.wire, link.wire.make(Interface::Pool), link.wire.make(Interface::Icon));
    wire.send(link.has.shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(memory.len as i32)]);
    wire.send(link.has.icons, 1, &[Arg::Uint(icon)]);
    let (mut from, mut buffers) = (0, Vec::new());
    for (width, pixels) in square {
        let place = &mut memory.pixels()[from..from + pixels.len()];
        place.iter_mut().zip(pixels).for_each(|(into, pixel)| *into = Color(*pixel).over(0, 255));
        let buffer = wire.make(Interface::Buffer);
        wire.send(pool, 0, &[Arg::Uint(buffer), Arg::Int(from as i32 * 4), Arg::Int(*width as i32), Arg::Int(*width as i32), Arg::Int(*width as i32 * 4), Arg::Uint(SEE_THROUGH)]);
        wire.send(icon, 2, &[Arg::Uint(buffer), Arg::Int(1)]);
        (from, buffers) = (from + pixels.len(), [buffers, vec![buffer]].concat());
    }
    wire.send(link.has.icons, 2, &[Arg::Uint(toplevel), Arg::Object(icon)]);
    wire.send(icon, 0, &[]);
    Some((memory, pool, buffers))
}

/// Does something with a window's interface: the first's own, or a second window's, lent by the
/// first meanwhile. (Nothing for a second window the app stopped describing.)
fn within<T>(first: &mut Ui, known: Option<Id>, work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    match known {
        Some(known) => first.in_window(known, work),
        None => Some(work(first)),
    }
}

/// The cursor shape by its `cursor-shape` name.
fn shape_number(cursor: Cursor) -> u32 {
    match cursor {
        Cursor::Arrow => 1,
        Cursor::Hand => 4,
        Cursor::Text => 9,
        Cursor::Move => 13,
        Cursor::No => 15,
        Cursor::ResizeX => 26,
        Cursor::ResizeY => 27,
        Cursor::ResizeUp => 28,
        Cursor::ResizeDown => 29,
    }
}

/// Resize edges as the compositor numbers them: top 1, bottom 2, left 4, right 8, a corner the
/// sum.
fn edges_number(across: i8, down: i8) -> u32 {
    [4, 0, 8][(across.clamp(-1, 1) + 1) as usize] | [1, 0, 2][(down.clamp(-1, 1) + 1) as usize]
}

/// The pointer, as the compositor reports it.
#[derive(Default)]
struct Pointer {
    /// The program's `wl_pointer` (0: the seat has none), and its cursor-shape device (0:
    /// none).
    id: u32,
    shapes: u32,
    /// The surface it is over (0: none), where on it in units, and the enter serial (setting
    /// the cursor needs it).
    over: u32,
    at: Point,
    entered: u32,
    /// The shape set last over that surface.
    shape: Option<Cursor>,
    /// What the wheel turned since the last `frame` event: in notches (or fractions) each way,
    /// and in units each way; notches take precedence where the compositor sends them.
    notches: [f32; 2],
    slid: [f32; 2],
    /// Its cursor theme pictures, where the compositor has no shape names.
    theme: Option<crate::xcursor::Pointers>,
}

/// The keyboard, as the compositor reports it.
#[derive(Default)]
struct Keys {
    id: u32,
    /// Its keymap once the compositor sent it (nothing where there is no libxkbcommon to read
    /// it).
    map: Option<Keyboard>,
    /// The focused surface (0: none).
    focus: u32,
    /// The key held for repeat, and when it repeats next.
    held: Option<(u32, Duration)>,
}

/// The input methods (`text-input`).
#[derive(Default)]
struct Typing {
    id: u32,
    /// The surface they are in (0: none), whether enabled there, and the last state sent:
    /// cursor rectangle and whether input is hidden.
    on: u32,
    enabled: bool,
    told: Option<(Rect, bool)>,
    /// Committed text still to hand on at `done`.
    typed: String,
}

/// A drag over a window from another program: its offer, the window's surface, its last
/// position, and what it is taken as (files or text).
#[derive(Default)]
struct DragOver {
    offer: u32,
    surface: u32,
    at: Point,
    kind: &'static str,
}

/// Touch points (`wl_touch`).
#[derive(Default)]
struct Fingers {
    id: u32,
    /// Each finger down: the compositor's id, the surface it landed on, and its position there.
    down: Vec<(i32, u32, Point)>,
    /// The finger emulating the mouse (see `Host::finger`), and the one grabbing its window to
    /// move or resize it, which the window hears nothing more of.
    mouse: Option<i32>,
    holds: Option<i32>,
    /// The window (and whether on its title bar) whose left button a cancelled finger held:
    /// released after the run that heard the finger leave, so what it pressed is not clicked.
    lifted: Option<(usize, bool)>,
}

/// A touchpad pinch (`pointer-gestures`): the program's object, the surface it began over, and
/// its scale since it began.
#[derive(Default)]
struct Pinch {
    id: u32,
    on: u32,
    spread: f32,
}

/// Pens near the screen (`tablet`): the seat's tablet object, and each tool announced.
#[derive(Default)]
struct Pens {
    seat: u32,
    pens: Vec<Pen>,
}

/// One pen tool, reported a frame at a time: its type and capabilities, the surface it is near
/// and where, its axes, and what happened this frame.
#[derive(Default)]
struct Pen {
    id: u32,
    /// It is an eraser; it reports pressure.
    rubs: bool,
    says_pressure: bool,
    /// The surface it is near (0: none), the proximity serial (setting its cursor needs it),
    /// and its cursor-shape device (0: none yet).
    surface: u32,
    entered: u32,
    shapes: u32,
    at: Point,
    /// Its axes: pressure (0 to 1, as reported), tilt and rotation, its side button, and
    /// whether it touches.
    stylus: Stylus,
    /// This frame it came in or out of proximity, touched or lifted. And the button its tip
    /// pressed with, while touching.
    came: bool,
    went: bool,
    touched: Option<bool>,
    button: Option<Button>,
}

/// A program's windows on a Wayland compositor, and everything keeping them shown.
struct Host<App> {
    link: Rc<Link>,
    app: App,
    /// The first window's interface, which holds the second windows' interfaces: it runs
    /// whichever window something happened in.
    ui: Ui,
    text: SharedText,
    windows: Vec<Shown>,
    /// The outputs: object, global name, and integer scale each.
    outputs: Vec<(u32, u32, i32)>,
    pointer: Pointer,
    keys: Keys,
    typing: Typing,
    drag_over: DragOver,
    fingers: Fingers,
    pinch: Pinch,
    pens: Pens,
    /// The program's icon pictures, for each window it opens.
    icons: Vec<(usize, Vec<u32>)>,
    /// `TAUT_SCALE`: the user's scale, whatever the compositor says.
    forced: Option<f32>,
    /// The pending `sync` callback (0: none).
    synced: u32,
    /// Activation tokens requested and not yet given, each with the window's surface. And the
    /// startup token the desktop gave (`XDG_ACTIVATION_TOKEN`), until used.
    tokens: Vec<(u32, u32)>,
    token: String,
    /// Something happened that the interface must run for.
    rerun: bool,
    done: bool,
    /// The pipe end other threads wake the windows through.
    wake: c_int,
    start: Instant,
    session: Session,
    heard: bool,
    /// The compositor's colour management and screens (feature `hdr`).
    #[cfg(feature = "hdr")]
    colors: hdr_wayland::Colors,
}

impl<App: FnMut(&mut Ui)> Host<App> {
    /// Opens the first window and waits for its first configure. Nothing if none comes.
    fn open(link: Link, outputs: Vec<(u32, u32)>, text: SystemText, session: Session, window: Window, app: App) -> Option<Self> {
        let link = Rc::new(link);
        let text = SharedText(Rc::new(RefCell::new(text)));
        let forced = std::env::var("TAUT_SCALE").ok().and_then(|value| decimal(&value)).filter(|scale| *scale > 0.0);
        let icons: Vec<(usize, Vec<u32>)> = window.icons.iter().map(|(width, pixels)| (*width, pixels.to_vec())).collect();
        let size = [window.width, window.height].map(|side| side.round().max(1.0) as i32);
        let min_size = [window.min_width, window.min_height].map(|side| side.round().max(1.0) as i32);
        let opening = Opening {
            known: None,
            title: window.title,
            size,
            min_size,
            position: window.position.unwrap_or_default(),
            frameless: window.frameless,
            transparent: window.transparent,
            maximized: window.maximized,
            minimized: window.minimized,
            icons: &icons,
        };
        let first = Shown::open(&link, &text, opening);
        // The pipe a `Waker` uses is the program's, as under X.
        let wake = pipe_ends();
        let mut ui = Ui::new(Box::new(text.clone()), Box::new(SystemClipboard(link.clone())));
        if window.transparent {
            ui.theme.bg = Color::NONE;
        }
        let outputs = outputs.into_iter().map(|(id, name)| (id, name, 1)).collect();
        let (keys, pointer, typing, drag_over) = (Keys::default(), Pointer::default(), Typing::default(), DragOver::default());
        let mut host = Host {
            link,
            app,
            ui,
            text,
            windows: vec![first],
            outputs,
            pointer,
            keys,
            typing,
            drag_over,
            fingers: Fingers::default(),
            pinch: Pinch::default(),
            pens: Pens::default(),
            icons,
            forced,
            synced: 0,
            tokens: Vec::new(),
            token: std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default(),
            rerun: true,
            done: false,
            wake: wake[0],
            start: Instant::now(),
            session,
            heard: true,
            #[cfg(feature = "hdr")]
            colors: Default::default(),
        };
        // A token comes from another program: one longer than a desktop gives is none.
        let token = !host.token.is_empty() && host.token.len() <= crate::freedesktop::TOKEN;
        host.windows[0].come = token && !window.minimized && host.link.has.activation != 0;
        #[cfg(feature = "gpu")]
        host.ask_gpu(window.renderer);
        host.seat_up();
        host.settle(Duration::from_secs(5));
        host.windows[0].configured.then_some(host)
    }

    /// Creates the seat objects that need no reply: the data device, the text-input object, and
    /// the tablet seat.
    fn seat_up(&mut self) {
        let (link, seat) = (self.link.clone(), self.link.has.seat);
        if seat == 0 {
            return;
        }
        if link.has.data != 0 {
            let device = link.wire.make(Interface::DataDevice);
            link.wire.send(link.has.data, 1, &[Arg::Uint(device), Arg::Uint(seat)]);
            link.clipboard.borrow_mut().device = device;
        }
        if link.has.text_inputs != 0 {
            self.typing.id = link.wire.make(Interface::TextInput);
            link.wire.send(link.has.text_inputs, 1, &[Arg::Uint(self.typing.id), Arg::Uint(seat)]);
        }
        if cfg!(feature = "touch") && link.has.tablets != 0 {
            self.pens.seat = link.wire.make(Interface::TabletSeat);
            link.wire.send(link.has.tablets, 0, &[Arg::Uint(self.pens.seat), Arg::Uint(seat)]);
        }
    }

    /// A round trip: asks for a `sync` callback and takes in events until it fires, for no
    /// longer than `most`.
    fn settle(&mut self, most: Duration) {
        let until = Instant::now() + most;
        self.synced = self.link.wire.make(Interface::Callback);
        self.link.wire.send(1, 0, &[Arg::Uint(self.synced)]);
        self.link.wire.flush();
        while self.synced != 0 && !self.done && Instant::now() < until {
            if readable(self.link.wire.fd(), 50) {
                self.take_all();
            }
        }
    }

    /// Takes in all the compositor sent.
    fn take_all(&mut self) {
        if !self.link.wire.read() {
            self.done = true;
        }
        while let Some(message) = self.link.wire.next() {
            self.take(message);
        }
        self.link.wire.flush();
    }

    /// One turn: takes in what happened, runs the interface if needed, and draws what changed.
    fn turn(&mut self) {
        self.ui.set_time(self.start.elapsed());
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO);
        let Session { bus, portal, .. } = &mut self.session;
        portal.now = self.start.elapsed();
        if portal.give_up(bus.as_mut()) {
            self.heed();
        }
        self.take_all();
        self.repeat();
        if std::mem::take(&mut self.heard) {
            self.hear_session();
            self.hear_readers();
        }
        if self.rerun {
            self.describe();
        }
        if let Some((at, title_bar)) = self.fingers.lifted.take().filter(|(at, _)| *at < self.windows.len()) {
            self.tell(at, title_bar, Event::Release(Button::Left));
        }
        (0..self.windows.len()).for_each(|at| self.draw(at));
        self.link.wire.flush();
        self.done |= self.link.wire.broken.get();
    }

    /// Dispatches one event to its object's handler.
    fn take(&mut self, message: Message) {
        match message.of {
            Interface::Display => self.displayed(message),
            Interface::Registry => self.listed(message),
            Interface::Callback => self.called(message.object),
            Interface::WmBase => self.pinged(message),
            Interface::XdgSurface => self.configured(message),
            Interface::Toplevel => self.toplevel_event(message),
            Interface::Decoration => self.decorated(message),
            Interface::Surface => self.surfaced(message),
            Interface::Scale => self.scaled(message),
            Interface::Buffer => self.released(message.object),
            Interface::Output => self.output(message),
            Interface::Seat => self.seated(message),
            Interface::Pointer => self.pointed(message),
            Interface::Keyboard => self.keyed(message),
            Interface::DataDevice => self.data_device_event(message),
            Interface::DataOffer => self.offered(message),
            Interface::DataSource => self.sourced(message),
            Interface::Token => self.activate(message),
            Interface::Exported => self.exported(message),
            Interface::TextInput => self.typed(message),
            Interface::Touch => self.touched(message),
            // Pinches and pens only for a program taking them as such: see `bind`.
            Interface::Pinch if cfg!(feature = "touch") => self.pinched(message),
            Interface::TabletSeat if cfg!(feature = "touch") => self.tablets(message),
            Interface::Tablet if cfg!(feature = "touch") => self.tablet(message),
            Interface::TabletTool if cfg!(feature = "touch") => self.pen(message),
            #[cfg(feature = "gpu")]
            Interface::Dmabuf | Interface::DmabufFeedback | Interface::DmabufParams => self.heard_by_gpu(message),
            #[cfg(feature = "hdr")]
            Interface::Shm | Interface::ColorManager | Interface::ColorOutput | Interface::Description | Interface::DescriptionInfo => self.colored(message),
            _ => {}
        }
    }

    /// `xdg_wm_base.ping`: answered with pong.
    fn pinged(&mut self, mut message: Message) {
        self.link.wire.send(message.object, 3, &[Arg::Uint(message.uint())]);
    }

    /// `wl_display`: a protocol error (fatal), or an object id freed.
    fn displayed(&mut self, mut message: Message) {
        if message.opcode == 1 {
            return self.link.wire.deleted(message.uint());
        }
        let (object, code, said) = (message.uint(), message.uint(), message.text());
        eprintln!("taut: the compositor ended the connection over object {object} ({code}): {said}");
        self.link.wire.broken.set(true);
        self.done = true;
    }

    /// A global added (an output plugged in) or removed (one unplugged). Only outputs come and
    /// go here.
    fn listed(&mut self, mut message: Message) {
        let (name, link) = (message.uint(), self.link.clone());
        if message.opcode == 1 {
            return self.outputs.retain(|(_, named, _)| *named != name);
        }
        let (interface, version) = (message.text(), message.uint());
        if interface == "wl_output" && version >= 2 {
            let id = link.wire.bind(link.has.registry, name, &interface, version.min(4), Interface::Output);
            self.outputs.push((id, name, 1));
        }
    }

    /// A callback fired: the round trip, or a surface's frame (it may draw again).
    fn called(&mut self, id: u32) {
        if id == self.synced {
            self.synced = 0;
        }
        for shown in &mut self.windows {
            let frames = shown.frame.iter_mut().map(|framed| &mut framed.sheet);
            for sheet in std::iter::once(&mut shown.sheet).chain(frames).filter(|sheet| sheet.waiting == id) {
                sheet.waiting = 0;
            }
        }
    }

    /// A buffer released: its surface may draw into it.
    fn released(&mut self, id: u32) {
        let link = self.link.clone();
        #[cfg(feature = "gpu")]
        if self.released_by_gpu(id) {
            return;
        }
        for shown in &mut self.windows {
            let frames = shown.frame.iter_mut().map(|framed| &mut framed.sheet);
            if std::iter::once(&mut shown.sheet).chain(frames).any(|sheet| sheet.released(&link, id)) {
                return;
            }
        }
        if let Some(theme) = &mut self.pointer.theme {
            theme.released(&link.wire, id);
        }
    }

    /// The window a surface belongs to, if theirs, and whether it is the window's title bar.
    fn window_of(&self, surface: u32) -> Option<(usize, bool)> {
        let own = self.windows.iter().position(|shown| shown.sheet.surface == surface).map(|at| (at, false));
        own.or_else(|| self.windows.iter().position(|shown| shown.frame.as_ref().is_some_and(|framed| framed.sheet.surface == surface)).map(|at| (at, true)))
    }

    /// `xdg_surface.configure`: the window acks it (its next commit follows it) and applies it.
    /// Its interface hears what is news.
    fn configured(&mut self, mut message: Message) {
        let Some(at) = self.windows.iter().position(|shown| shown.xdg == message.object) else { return };
        let (link, text) = (self.link.clone(), self.text.clone());
        link.wire.send(message.object, 4, &[Arg::Uint(message.uint())]);
        let shown = &mut self.windows[at];
        let was = (shown.size, shown.filling, shown.lying_over, shown.configured, shown.hidden);
        shown.configure(&link, &text);
        shown.sheet.owed = true;
        let now = (shown.size, shown.filling, shown.lying_over, true, shown.hidden);
        if now.4 != was.4 {
            self.hear(at, Event::Minimized(now.4));
        }
        self.rerun |= now != was;
        self.place(at);
    }

    /// `xdg_toplevel` events: configure (applied at `xdg_surface.configure`), close, and
    /// capabilities.
    fn toplevel_event(&mut self, mut message: Message) {
        let Some(at) = self.windows.iter().position(|shown| shown.toplevel == message.object) else { return };
        match message.opcode {
            0 => {
                let size = [message.int(), message.int()];
                let states = message.bytes().as_chunks::<4>().0.iter().map(|state| u32::from_ne_bytes(*state)).collect();
                self.windows[at].asked = (size, states);
            }
            1 => self.closed(at),
            _ => {}
        }
    }

    /// Who draws a window's decorations: the compositor, or the window, which then draws the
    /// library's own (unless it asked for none).
    fn decorated(&mut self, mut message: Message) {
        let Some(at) = self.windows.iter().position(|shown| shown.decoration == message.object) else { return };
        let (link, text, mode) = (self.link.clone(), self.text.clone(), message.uint());
        let shown = &mut self.windows[at];
        shown.framed = mode == BY_THE_WINDOW && !shown.kind.frameless && !shown.kind.transparent;
        shown.frame_up(&link, &text);
        shown.fit(&link);
        self.rerun = true;
    }

    /// A surface entered or left an output, or the compositor states its preferred integer
    /// scale: a window's scale may change.
    fn surfaced(&mut self, mut message: Message) {
        let Some((at, false)) = self.window_of(message.object) else { return };
        let (shown, said) = (&mut self.windows[at], message.uint());
        match message.opcode {
            0 => shown.on.push(said),
            1 => shown.on.retain(|output| *output != said),
            2 => shown.whole = Some(said as i32),
            _ => return,
        }
        self.rescale(at);
    }

    /// The compositor's preferred fractional scale for a window.
    fn scaled(&mut self, mut message: Message) {
        let Some(at) = self.windows.iter().position(|shown| shown.fraction == message.object) else { return };
        self.windows[at].finer = Some(message.uint() as f32 / 120.0);
        self.rescale(at);
    }

    /// An output's integer scale. Windows on it may redraw at a new scale.
    fn output(&mut self, mut message: Message) {
        let Some(output) = self.outputs.iter_mut().find(|(id, ..)| *id == message.object) else { return };
        if message.opcode == 3 {
            output.2 = message.int().max(1);
            (0..self.windows.len()).for_each(|at| self.rescale(at));
        }
    }

    /// Recomputes a window's scale. If it changed, everything in the window is redrawn and its
    /// interface runs at it.
    fn rescale(&mut self, at: usize) {
        let link = self.link.clone();
        let shown = &mut self.windows[at];
        let scale = shown.scale_now(self.forced, &self.outputs);
        if scale != shown.scale {
            shown.scale = scale;
            shown.fit(&link);
            self.rerun = true;
        }
    }

    /// The seat's capabilities: pointer (with touchpad pinches), keyboard, touch. Objects are
    /// created for each.
    fn seated(&mut self, mut message: Message) {
        if message.opcode != 0 {
            return;
        }
        let (link, has) = (self.link.clone(), message.uint());
        if has & 1 != 0 && self.pointer.id == 0 {
            self.pointer.id = link.wire.make(Interface::Pointer);
            link.wire.send(link.has.seat, 0, &[Arg::Uint(self.pointer.id)]);
        }
        if has & 1 != 0 && link.has.shapes != 0 && self.pointer.shapes == 0 {
            self.pointer.shapes = link.wire.make(Interface::ShapeDevice);
            link.wire.send(link.has.shapes, 1, &[Arg::Uint(self.pointer.shapes), Arg::Uint(self.pointer.id)]);
        }
        if cfg!(feature = "touch") && has & 1 != 0 && link.has.gestures != 0 && self.pinch.id == 0 {
            self.pinch.id = link.wire.make(Interface::Pinch);
            link.wire.send(link.has.gestures, 1, &[Arg::Uint(self.pinch.id), Arg::Uint(self.pointer.id)]);
        }
        if has & 4 != 0 && self.fingers.id == 0 {
            self.fingers.id = link.wire.make(Interface::Touch);
            link.wire.send(link.has.seat, 2, &[Arg::Uint(self.fingers.id)]);
        }
        if has & 2 != 0 && self.keys.id == 0 {
            self.keys.id = link.wire.make(Interface::Keyboard);
            link.wire.send(link.has.seat, 1, &[Arg::Uint(self.keys.id)]);
            self.keys.map = Keyboard::new();
        }
    }

    /// Pointer events: enter, leave, motion, button, axis (in parts, then `frame`).
    fn pointed(&mut self, mut message: Message) {
        match message.opcode {
            0 => {
                let (serial, surface, x, y) = (message.uint(), message.uint(), message.fixed(), message.fixed());
                (self.pointer.over, self.pointer.entered, self.pointer.shape) = (surface, serial, None);
                self.moved(pt(x, y));
            }
            1 => self.pointer_left(),
            2 => {
                let (_, x, y) = (message.uint(), message.fixed(), message.fixed());
                self.moved(pt(x, y));
            }
            3 => {
                let (serial, time, button, state) = (message.uint(), message.uint(), message.uint(), message.uint());
                self.link.serial.set(serial);
                self.button(time, button, state == 1);
            }
            4 => {
                let (_, axis, value) = (message.uint(), message.uint() as usize, message.fixed());
                self.pointer.slid[axis.min(1)] += value;
                // A compositor without `frame` events: each part is one.
                (self.link.has.seat_version < 5).then(|| self.wheel());
            }
            5 => self.wheel(),
            8 => {
                let (axis, notches) = (message.uint() as usize, message.int());
                self.pointer.notches[axis.min(1)] += notches as f32;
            }
            9 => {
                let (axis, steps) = (message.uint() as usize, message.int());
                self.pointer.notches[axis.min(1)] += steps as f32 / STEPS;
            }
            _ => {}
        }
    }

    /// The pointer at a place on its surface. Over the library's title bar, the title bar hears
    /// it; over the window, as under X: a held window follows it, else its interface hears
    /// where the pointer is, unless it is on the window's title bar or a resize edge, which
    /// take it. (While fingers are down, the window hears nothing of the pointer: a finger is
    /// its pointer then, and Mutter sends a leave as a finger lands.)
    fn moved(&mut self, place: Point) {
        self.pointer.at = place;
        let Some((at, title_bar)) = self.window_of(self.pointer.over).filter(|_| self.fingers.down.is_empty()) else { return };
        let hit = self.hit_test(at, title_bar, place);
        self.tell(at, title_bar, if hit == HitTest::Client { Event::Move(place) } else { Event::Leave });
        self.windows[at].over = hit;
        self.shape();
    }

    /// The pointer left its surface. (While fingers are down the window does not hear it: see
    /// `moved`.)
    fn pointer_left(&mut self) {
        let over = std::mem::take(&mut self.pointer.over);
        if let Some((at, title_bar)) = self.window_of(over).filter(|_| self.fingers.down.is_empty()) {
            self.windows[at].over = HitTest::Client;
            self.tell(at, title_bar, Event::Leave);
        }
    }

    /// What of a window the pointer grabs at a place: on the window itself, its interface is
    /// asked where it has parts of its own to grab (a window with the library's title bar
    /// resizes by its other edges; its top edge is the title bar's); on the title bar, the
    /// title bar is asked. (A maximized window has no resize edges.)
    fn hit_test(&mut self, at: usize, title_bar: bool, place: Point) -> HitTest {
        let shown = &mut self.windows[at];
        let sized = (shown.kind.frameless || shown.frame.is_some()) && !shown.filling && !shown.lying_over;
        // The title bar's bottom edge lies on the window and is not a window edge; nor is the
        // window's top edge under a title bar.
        if title_bar {
            let hit = shown.frame.as_ref().map_or(HitTest::Client, |framed| framed.title_bar.ui.hit_test(place, sized));
            return match hit {
                HitTest::Edge(0, 1) => HitTest::TitleBar,
                HitTest::Edge(across, 1) => HitTest::Edge(across, 0),
                _ => hit,
            };
        }
        if !shown.grips && shown.frame.is_none() {
            return HitTest::Client;
        }
        let hit = within(&mut self.ui, shown.known, |ui| ui.hit_test(place, sized)).unwrap_or_default();
        match hit {
            HitTest::Edge(0, -1) if shown.frame.is_some() => HitTest::Client,
            HitTest::Edge(across, -1) if shown.frame.is_some() => HitTest::Edge(across, 0),
            _ => hit,
        }
    }

    /// Sets the pointer's shape for what it is over: the edge's, else what the window's
    /// interface (or its title bar) asked for. Only a change is sent.
    fn shape(&mut self) {
        let Some((at, title_bar)) = self.window_of(self.pointer.over) else { return };
        let shown = &self.windows[at];
        let asked = match title_bar {
            true => shown.frame.as_ref().map_or(Cursor::Arrow, |framed| framed.title_bar.ui.cursor()),
            false => shown.cursor,
        };
        let cursor = shown.over.cursor().unwrap_or(asked);
        if self.pointer.shape.replace(cursor) == Some(cursor) {
            return;
        }
        let (link, scale) = (self.link.clone(), shown.scale);
        if self.pointer.shapes != 0 {
            return link.wire.send(self.pointer.shapes, 1, &[Arg::Uint(self.pointer.entered), Arg::Uint(shape_number(cursor))]);
        }
        let theme = self.pointer.theme.get_or_insert_with(|| crate::xcursor::Pointers::new(&link.wire, link.has.compositor));
        theme.show(&link.wire, link.has.shm, self.pointer.id, self.pointer.entered, cursor, scale);
    }

    /// A mouse button went down or up over a window or its title bar. As under X, a left
    /// press on a title bar or edge grabs the window and the interface does not hear it. A
    /// right press on a title bar opens the compositor's window menu.
    fn button(&mut self, time: u32, button: u32, down: bool) {
        let Some((at, title_bar)) = self.window_of(self.pointer.over) else { return };
        let hit = self.hit_test(at, title_bar, self.pointer.at);
        if down && button == LEFT && hit != HitTest::Client {
            return self.take_hold(at, hit, time, self.pointer.at);
        }
        if down && button == RIGHT && hit == HitTest::TitleBar {
            let (link, shown) = (&self.link, &self.windows[at]);
            let y = self.pointer.at.y as i32 - if title_bar { decorations::HIGH } else { 0 };
            let place = [Arg::Uint(link.has.seat), Arg::Uint(link.serial.get()), Arg::Int(self.pointer.at.x as i32), Arg::Int(y)];
            return link.wire.send(shown.toplevel, 4, &place);
        }
        let Some(which) = [(LEFT, Button::Left), (RIGHT, Button::Right), (MIDDLE, Button::Middle)].into_iter().find(|(number, _)| *number == button).map(|(_, which)| which) else { return };
        let mods = self.keys.map.as_ref().map(Keyboard::held).unwrap_or_default();
        self.tell(at, title_bar, if down { Event::Press(which, mods) } else { Event::Release(which) });
    }

    /// The left button (or a finger or pen) went down on a window's title bar or edge: the
    /// compositor is asked to move the window, or resize it by those edges, while it stays
    /// down. A press on the title bar soon after another, close by, is a double press: the
    /// window asks to maximize or restore. (Not a window keeping its creation size.)
    fn take_hold(&mut self, at: usize, hit: HitTest, time: u32, place: Point) {
        let link = self.link.clone();
        let shown = &mut self.windows[at];
        let before = shown.pressed.replace((time, place));
        let again = hit == HitTest::TitleBar && before.is_some_and(|(then, was)| time.wrapping_sub(then) < DOUBLE_PRESS && (place.x - was.x).abs() < 4.0 && (place.y - was.y).abs() < 4.0);
        let (seat, serial) = (Arg::Uint(link.has.seat), Arg::Uint(link.serial.get()));
        match hit {
            HitTest::TitleBar if again && (shown.kind.frameless || !shown.kind.transparent) => {
                shown.pressed = None;
                link.wire.send(shown.toplevel, if shown.filling { 10 } else { 9 }, &[]);
            }
            HitTest::TitleBar if again => shown.pressed = None,
            HitTest::TitleBar => link.wire.send(shown.toplevel, 5, &[seat, serial]),
            HitTest::Edge(across, down) => link.wire.send(shown.toplevel, 6, &[seat, serial, Arg::Uint(edges_number(across, down))]),
            HitTest::Client => {}
        }
    }

    /// A wheel `frame`: what it turned goes where the pointer is, in notches where counted,
    /// else as far as it slid.
    fn wheel(&mut self) {
        let (notches, slid) = (std::mem::take(&mut self.pointer.notches), std::mem::take(&mut self.pointer.slid));
        let way = |at: usize| if notches[at] != 0.0 { notches[at] * NOTCH } else { slid[at] };
        let [down, right] = [way(0), way(1)];
        if (down, right) == (0.0, 0.0) {
            return;
        }
        if let Some((at, false)) = self.window_of(self.pointer.over) {
            // With Control it zooms, a quarter per notch, as under X.
            let ctrl = self.keys.map.as_ref().is_some_and(|map| map.held().ctrl);
            let event = if ctrl && down != 0.0 { Event::Zoom(crate::touch::wheel_zoom(-down / NOTCH)) } else { Event::Wheel(right, down) };
            self.hear(at, event);
        }
    }

    /// Keyboard events: keymap, enter, leave, key, modifiers, repeat info.
    fn keyed(&mut self, mut message: Message) {
        match message.opcode {
            0 => {
                let (format, size, file) = (message.uint(), message.uint(), message.file.take());
                let text = file.filter(|_| format == 1).and_then(|file| crate::wayland_wire::read_file(&file, size as usize));
                self.keys.map.as_mut().zip(text).map(|(map, text)| map.take_map(&text));
            }
            1 => {
                let (serial, surface) = (message.uint(), message.uint());
                self.link.serial.set(serial);
                self.keys.focus = surface;
                self.focus(surface, true);
            }
            2 => {
                let (_, surface) = (message.uint(), message.uint());
                (self.keys.focus, self.keys.held) = (0, None);
                self.focus(surface, false);
            }
            3 => {
                let (serial, _, key, state) = (message.uint(), message.uint(), message.uint(), message.uint());
                self.link.serial.set(serial);
                self.key(key, state == 1);
            }
            4 => {
                let (_, held, latched, locked, group) = (message.uint(), message.uint(), message.uint(), message.uint(), message.uint());
                let was = self.keys.map.as_ref().map(Keyboard::held);
                self.keys.map.iter_mut().for_each(|map| map.hold(held, latched, locked, group));
                self.held_changed(was);
            }
            5 => {
                let (rate, delay) = (message.int(), message.int());
                let repeat = (rate > 0).then(|| (Duration::from_millis(delay.max(0) as u64), Duration::from_micros(1_000_000 / rate as u64)));
                self.keys.map.iter_mut().for_each(|map| map.repeat = repeat);
            }
            _ => {}
        }
    }

    /// Focus came to or left a window: its interface is told, and the library's title bar
    /// shows it.
    fn focus(&mut self, surface: u32, on: bool) {
        let Some((at, _)) = self.window_of(surface) else { return };
        self.hear(at, Event::Focused(on));
    }

    /// A key went down or up in the focused window: the interface hears which and what it
    /// types, as under X. A held key repeats after the delay, then at the compositor's rate.
    fn key(&mut self, code: u32, down: bool) {
        if !down {
            self.keys.held = self.keys.held.filter(|(held, _)| *held != code);
            return self.key_up(code);
        }
        let Some(map) = self.keys.map.as_mut() else { return };
        let repeat = map.repeat.filter(|_| map.repeats(code));
        self.keys.held = repeat.map(|(delay, _)| (code, self.start.elapsed() + delay));
        self.press(code);
    }

    /// A key came up in the focused window: which. (For Shift, Control and Alt the compositor
    /// reports modifiers instead: see `held_changed`.)
    fn key_up(&mut self, code: u32) {
        let Some((at, _)) = self.window_of(self.keys.focus) else { return };
        if let Some(key) = self.keys.map.as_ref().and_then(|map| map.which(code)) {
            self.hear(at, Event::KeyUp(key));
        }
    }

    /// The modifiers changed: where that changes Shift, Control or Alt, the focused window
    /// hears it, as under X.
    fn held_changed(&mut self, was: Option<Mods>) {
        let now = self.keys.map.as_ref().map(Keyboard::held);
        let Some((at, _)) = self.window_of(self.keys.focus).filter(|_| now != was) else { return };
        self.hear(at, Event::ModifiersChanged(now.unwrap_or_default()));
    }

    /// Hands the focused window a key that went down: which key, and what it types.
    fn press(&mut self, code: u32) {
        let Some((at, _)) = self.window_of(self.keys.focus) else { return };
        let Some(map) = self.keys.map.as_mut() else { return };
        let ((key, text), mods) = (map.press(code), map.held());
        if let Some(key) = key {
            self.hear(at, Event::Key(key, mods));
        }
        if !text.is_empty() && !mods.ctrl && text.chars().all(|c| !c.is_control()) {
            self.hear(at, Event::Text(text));
        }
    }

    /// A held key repeats, when due.
    fn repeat(&mut self) {
        let now = self.start.elapsed();
        let Some((code, due)) = self.keys.held.filter(|(_, due)| *due <= now) else { return };
        let every = self.keys.map.as_ref().and_then(|map| map.repeat).map_or(Duration::from_millis(40), |(_, every)| every);
        self.keys.held = Some((code, due.max(now.saturating_sub(every)) + every));
        self.press(code);
    }

    /// Data device events: an offer came (its types follow), the selection changed, or a drag
    /// entered, moved, left or dropped on a window, which a window takes once its app asked
    /// about drops (see `Drops`).
    fn data_device_event(&mut self, mut message: Message) {
        let link = self.link.clone();
        match (message.opcode, DROPS.get()) {
            (0, _) => {
                let offer = message.uint();
                link.wire.note(offer, Interface::DataOffer);
                link.clipboard.borrow_mut().offers.push((offer, Vec::new()));
            }
            (5, _) => {
                let offer = message.uint();
                let old = std::mem::replace(&mut link.clipboard.borrow_mut().selection, offer);
                // The previous selection offer is destroyed, unless it is the same or is being
                // dragged.
                link.forget(if old != offer && old != self.drag_over.offer { old } else { 0 });
            }
            (1..=4, Some(drops)) => {
                let taking = Taking { link: &self.link, ui: &mut self.ui, windows: &self.windows, drag_over: &mut self.drag_over, rerun: false };
                self.rerun |= (drops.drag_event)(taking, message);
            }
            // Where no window accepts drops, a drag over one is refused as it comes.
            (1, None) => {
                let (_, _, _, _, offer) = (message.uint(), message.uint(), message.fixed(), message.fixed(), message.uint());
                link.forget(offer);
            }
            _ => {}
        }
    }

    /// An offer announces a MIME type.
    fn offered(&mut self, mut message: Message) {
        if message.opcode != 0 {
            return;
        }
        let kind = message.text();
        let mut clipboard = self.link.clipboard.borrow_mut();
        if let Some((_, kinds)) = clipboard.offers.iter_mut().find(|(id, _)| *id == message.object) {
            kinds.push(kind);
        }
    }

    /// This program's data source: another program asks for it (`send`), or it is cancelled.
    fn sourced(&mut self, mut message: Message) {
        match message.opcode {
            1 => {
                let (kind, file) = (message.text(), message.file.take());
                self.link.hand_over(&kind, file);
            }
            2 => self.link.cancelled(message.object),
            _ => {}
        }
    }

    /// The compositor gave an activation token: the window is activated, if the compositor
    /// agrees.
    fn activate(&mut self, mut message: Message) {
        let Some(at) = self.tokens.iter().position(|(token, _)| *token == message.object) else { return };
        let ((token, surface), link) = (self.tokens.remove(at), &self.link);
        link.wire.send(link.has.activation, 2, &[Arg::Text(&message.text()), Arg::Uint(surface)]);
        link.wire.send(token, 4, &[]);
    }

    /// The compositor gave a window an exported handle: portal dialogs are parented to the
    /// window by it.
    fn exported(&mut self, mut message: Message) {
        if let Some(shown) = self.windows.iter_mut().find(|shown| shown.exported == message.object) {
            shown.handle = format!("wayland:{}", message.text());
            crate::print::set_parent(shown.handle.clone());
        }
    }

    /// Input method events: enter, leave, commit, done (the committed text then goes to the
    /// window).
    fn typed(&mut self, mut message: Message) {
        match message.opcode {
            0 => (self.typing.on, self.typing.told, self.typing.enabled) = (message.uint(), None, false),
            1 => (self.typing.on, self.typing.enabled) = (0, false),
            3 => self.typing.typed.push_str(&message.text()),
            5 => self.typing_done(),
            _ => {}
        }
    }

    /// The input method's `done`: its committed text goes to the window it is in.
    fn typing_done(&mut self) {
        let text = std::mem::take(&mut self.typing.typed);
        if let Some((at, false)) = self.window_of(self.typing.on).filter(|_| !text.is_empty()) {
            self.hear(at, Event::Text(text));
        }
    }

    /// A finger down, motion or up; or cancel (another program or a compositor gesture has them
    /// now).
    fn touched(&mut self, mut message: Message) {
        match message.opcode {
            0 => {
                let (serial, time, surface, finger, x, y) = (message.uint(), message.uint(), message.uint(), message.int(), message.fixed(), message.fixed());
                self.link.serial.set(serial);
                self.fingers.down.push((finger, surface, pt(x, y)));
                self.finger(finger, TouchPhase::Down, time);
            }
            1 => {
                let (serial, time, finger) = (message.uint(), message.uint(), message.int());
                self.link.serial.set(serial);
                self.finger(finger, TouchPhase::Up, time);
                self.fingers.down.retain(|(down, ..)| *down != finger);
            }
            2 => {
                let (time, finger, at) = (message.uint(), message.int(), pt(message.fixed(), message.fixed()));
                self.fingers.down.iter_mut().filter(|(down, ..)| *down == finger).for_each(|down| down.2 = at);
                self.finger(finger, TouchPhase::Move, time);
            }
            4 => {
                let down: Vec<i32> = self.fingers.down.iter().map(|(finger, ..)| *finger).collect();
                down.into_iter().for_each(|finger| self.finger(finger, TouchPhase::Cancel, 0));
                self.fingers.down.clear();
            }
            _ => {}
        }
    }

    /// What a finger did on a window. Landing on its title bar or a resize edge, it grabs the
    /// window, as the mouse does, and the window hears no more of it. Else on the library's
    /// title bar it is the mouse; on the window, its interface takes it as a finger (with the
    /// feature `touch`) or as the mouse, first finger only (without: so a finger works as on X
    /// and Windows, where the system emulates the mouse).
    fn finger(&mut self, finger: i32, phase: TouchPhase, time: u32) {
        let Some(&(_, surface, place)) = self.fingers.down.iter().find(|(down, ..)| *down == finger) else { return };
        let Some((at, title_bar)) = self.window_of(surface) else { return };
        if self.fingers.holds == Some(finger) {
            self.fingers.holds = self.fingers.holds.filter(|_| phase == TouchPhase::Move);
            return;
        }
        let hit = if phase == TouchPhase::Down { self.hit_test(at, title_bar, place) } else { HitTest::Client };
        if hit != HitTest::Client {
            self.fingers.holds = Some(finger);
            return self.take_hold(at, hit, time, place);
        }
        if !title_bar && cfg!(feature = "touch") {
            return self.hear(at, Event::Touch(Touch { id: finger as u32, position: place, phase }));
        }
        let fingers = &mut self.fingers;
        fingers.mouse = fingers.mouse.or((phase == TouchPhase::Down).then_some(finger));
        if fingers.mouse != Some(finger) {
            return;
        }
        fingers.mouse = fingers.mouse.filter(|_| matches!(phase, TouchPhase::Down | TouchPhase::Move));
        self.as_mouse(at, title_bar, place, phase);
    }

    /// A finger or pen emulating the mouse's left button: down where it lands, moves, up as it
    /// lifts (and leaves with it). Cancelled, it leaves, and its button comes up after the next
    /// run (`Fingers::lifted`).
    fn as_mouse(&mut self, at: usize, title_bar: bool, place: Point, phase: TouchPhase) {
        let mods = self.keys.map.as_ref().map(Keyboard::held).unwrap_or_default();
        let events = match phase {
            TouchPhase::Down => [Some(Event::Move(place)), Some(Event::Press(Button::Left, mods))],
            TouchPhase::Move => [Some(Event::Move(place)), None],
            TouchPhase::Up => [Some(Event::Release(Button::Left)), Some(Event::Leave)],
            TouchPhase::Cancel => [Some(Event::Leave), None],
        };
        if phase == TouchPhase::Cancel {
            self.fingers.lifted = Some((at, title_bar));
        }
        events.into_iter().flatten().for_each(|event| self.tell(at, title_bar, event));
    }

    /// A touchpad pinch began over a surface, updated (scale since begin), or ended: a zoom at
    /// the pointer, as Control with the wheel.
    fn pinched(&mut self, mut message: Message) {
        let mut by = 1.0;
        match message.opcode {
            0 => {
                let (_, _, surface) = (message.uint(), message.uint(), message.uint());
                (self.pinch.on, self.pinch.spread) = (surface, 1.0);
            }
            1 => {
                let (_, _, _, spread) = (message.uint(), message.fixed(), message.fixed(), message.fixed());
                by = spread / std::mem::replace(&mut self.pinch.spread, spread);
            }
            _ => self.pinch.on = 0,
        }
        let Some((at, false)) = self.window_of(self.pinch.on).filter(|_| by.is_finite() && by > 0.0 && by != 1.0) else { return };
        self.hear(at, Event::Zoom(by));
    }

    /// The tablet seat: a tablet or a tool was added, reported from then on. (Not pads: their
    /// buttons and rings are an app's own to assign, and none asks.)
    fn tablets(&mut self, mut message: Message) {
        let (opcode, new) = (message.opcode, message.uint());
        match opcode {
            0 => self.link.wire.note(new, Interface::Tablet),
            1 => self.link.wire.note(new, Interface::TabletTool),
            _ => {}
        }
        if opcode == 1 {
            self.pens.pens.push(Pen { id: new, ..Pen::default() });
        }
    }

    /// A tablet was removed: destroyed.
    fn tablet(&mut self, message: Message) {
        if message.opcode == 4 {
            self.link.wire.send(message.object, 0, &[]);
            self.link.wire.note(message.object, Interface::Free);
        }
    }

    /// A tool event: type and capabilities, removed (destroyed), proximity in or out, down or
    /// up, motion, pressure, tilt, rotation, buttons; or `frame`.
    fn pen(&mut self, mut message: Message) {
        let Some(at) = self.pens.pens.iter().position(|pen| pen.id == message.object) else { return };
        if message.opcode == 5 {
            self.link.wire.send(message.object, 1, &[]);
            self.link.wire.note(message.object, Interface::Free);
            self.pens.pens.remove(at);
            return;
        }
        let (link, pen) = (&self.link, &mut self.pens.pens[at]);
        match message.opcode {
            0 => pen.rubs = message.uint() == RUBS_OUT,
            3 => pen.says_pressure |= message.uint() == 2,
            6 => {
                let (serial, _, surface) = (message.uint(), message.uint(), message.uint());
                (pen.entered, pen.surface, pen.came) = (serial, surface, true);
            }
            7 => pen.went = true,
            8 => {
                link.serial.set(message.uint());
                pen.touched = Some(true);
            }
            9 => pen.touched = Some(false),
            10 => pen.at = pt(message.fixed(), message.fixed()),
            11 => pen.stylus.pressure = message.uint().min(65535) as f32 / 65535.0,
            13 => pen.stylus.tilt = [message.fixed(), message.fixed()].map(|tilt| tilt.clamp(-90.0, 90.0)),
            14 => pen.stylus.twist = message.fixed().rem_euclid(360.0),
            17 => {
                let (_, button, state) = (message.uint(), message.uint(), message.uint());
                pen.stylus.barrel = if SIDE.contains(&button) { state == 1 } else { pen.stylus.barrel };
            }
            18 => self.pen_frame(at, message.uint()),
            _ => {}
        }
    }

    /// A tool's frame is done. The window it is near hears its axes (pressure only while
    /// touching, half where not reported), position, and what its tip pressed or released: left
    /// button, or right with the side button held. A window it left hears it gone, and the
    /// pointer with it. Touching a title bar or resize edge, it grabs the window; on the
    /// library's title bar, it is the mouse.
    fn pen_frame(&mut self, at: usize, time: u32) {
        let pen = &mut self.pens.pens[at];
        let (came, went, touched) = (std::mem::take(&mut pen.came), std::mem::take(&mut pen.went), pen.touched.take());
        pen.stylus.touching = touched.unwrap_or(pen.stylus.touching);
        let pressure = [[0.0, 0.0], [0.5, pen.stylus.pressure]][pen.stylus.touching as usize][pen.says_pressure as usize];
        let (place, stylus, surface) = (pen.at, Stylus { pressure, eraser: pen.rubs, ..pen.stylus }, pen.surface);
        let Some((window, title_bar)) = self.window_of(surface) else { return };
        if came {
            self.pen_shape(at, window);
        }
        let hit = if touched == Some(true) { self.hit_test(window, title_bar, place) } else { HitTest::Client };
        if hit != HitTest::Client {
            return self.take_hold(window, hit, time, place);
        }
        if title_bar {
            let phase = [(went, TouchPhase::Cancel), (touched == Some(true), TouchPhase::Down), (touched == Some(false), TouchPhase::Up)].into_iter().find(|(so, _)| *so);
            return self.as_mouse(window, true, place, phase.map_or(TouchPhase::Move, |(_, phase)| phase));
        }
        let mods = self.keys.map.as_ref().map(Keyboard::held).unwrap_or_default();
        let pen = &mut self.pens.pens[at];
        let button = match touched {
            Some(true) => Some(Event::Press(*pen.button.insert(if stylus.barrel { Button::Right } else { Button::Left }), mods)),
            _ if touched == Some(false) || went => pen.button.take().map(Event::Release),
            _ => None,
        };
        let events = match went {
            true => [Some(Event::Stylus(None)), Some(Event::Leave), button],
            false => [Some(Event::Stylus(Some(stylus))), Some(Event::Move(place)), button],
        };
        events.into_iter().flatten().for_each(|event| self.hear(window, event));
    }

    /// A tool came near a window: it gets the pointer's shape there, where the compositor has
    /// shape names.
    fn pen_shape(&mut self, at: usize, window: usize) {
        let (link, pen) = (self.link.clone(), &mut self.pens.pens[at]);
        if link.has.shapes == 0 {
            return;
        }
        if pen.shapes == 0 {
            pen.shapes = link.wire.make(Interface::ShapeDevice);
            link.wire.send(link.has.shapes, 2, &[Arg::Uint(pen.shapes), Arg::Uint(pen.id)]);
        }
        link.wire.send(pen.shapes, 1, &[Arg::Uint(pen.entered), Arg::Uint(shape_number(self.windows[window].cursor))]);
    }

    /// Tells a window what the user did on it: its library title bar, or else its interface.
    fn tell(&mut self, at: usize, title_bar: bool, event: Event) {
        match title_bar {
            true => self.title_bar_event(at, event),
            false => self.hear(at, event),
        }
    }

    /// Passes what the user did in a window to its interface, noting whether it must run.
    fn hear(&mut self, at: usize, event: Event) {
        self.rerun |= within(&mut self.ui, self.windows[at].known, |ui| ui.event(event)) == Some(true);
    }

    /// A window's title bar hears what the user did there and runs if needed; its buttons'
    /// requests are carried out.
    fn title_bar_event(&mut self, at: usize, event: Event) {
        let theme = within(&mut self.ui, self.windows[at].known, |ui| ui.theme).unwrap_or(self.ui.theme);
        let Some(framed) = self.windows[at].frame.as_mut() else { return };
        if !framed.title_bar.ui.event(event) {
            return;
        }
        let asked = framed.title_bar.run(theme);
        framed.sheet.changed.extend(framed.title_bar.ui.changes());
        framed.title_bar.ui.changed();
        let (link, shown) = (self.link.clone(), &self.windows[at]);
        match asked {
            Some(Asked::Minimize) => link.wire.send(shown.toplevel, 13, &[]),
            Some(Asked::Fill) => link.wire.send(shown.toplevel, if shown.filling { 10 } else { 9 }, &[]),
            Some(Asked::Close) => self.closed(at),
            None => {}
        }
        self.shape();
    }

    /// The user closed a window. With the first the program ends. A second one's app hears in
    /// the next run and stops describing it: only then does the window go.
    fn closed(&mut self, at: usize) {
        let Some(known) = self.windows[at].known else { return self.done = true };
        self.rerun |= self.ui.in_window(known, Ui::request_close).is_some();
    }

    /// Tells a window's interface what Wayland reveals of its place: where its app asked it to
    /// open, its client size (its restore size while maximized), and whether maximized or
    /// fullscreen.
    fn place(&mut self, at: usize) {
        let shown = &self.windows[at];
        let away = shown.filling || shown.lying_over;
        let size = if away { shown.normal } else { shown.size };
        let now = WindowPlacement { position: shown.at, width: size[0] as f32, height: size[1] as f32, maximized: shown.filling };
        let whole = shown.lying_over;
        self.rerun |= within(&mut self.ui, shown.known, |ui| ui.set_window_placement(now) | ui.full_screen_now(whole)) == Some(true);
    }

    /// Runs the interface, as under X: each window's interface is told its size, the first one
    /// runs, and the windows are brought in line with what was described.
    fn describe(&mut self) {
        for shown in &self.windows {
            let size = Size { w: shown.size[0] as f32, h: shown.size[1] as f32 };
            within(&mut self.ui, shown.known, |ui| ui.resize(size, shown.scale));
        }
        self.ui.run(&mut self.app);
        (self.rerun, self.done) = (false, self.done || self.ui.quitting());
        // What the app asked of the desktop beyond its windows, once it asked any.
        if let Some(stood) = DESKTOP.get() {
            self.rerun |= stood(&self.link, &mut self.windows[0], &mut self.ui, &mut self.session);
        }
        self.keep_up();
        (0..self.windows.len()).for_each(|at| self.follow(at));
        #[cfg(feature = "hdr")]
        (0..self.windows.len()).for_each(|at| self.light(at));
        if self.ui.listening {
            self.tell_readers();
        }
    }

    /// After a run, the windows are made to match the interface: a second window the app
    /// stopped describing is closed (with any portal dialog up for it), and one described for
    /// the first time is opened, with its app's options.
    fn keep_up(&mut self) {
        let (link, text, Session { bus, portal, .. }) = (self.link.clone(), self.text.clone(), &mut self.session);
        for at in (1..self.windows.len()).rev() {
            if !self.ui.windows().any(|(known, ..)| self.windows[at].known == Some(known)) {
                portal.forget(bus.as_mut(), self.windows[at].known.unwrap_or(0));
                self.windows.remove(at).close(&link);
            }
        }
        let new: Vec<(Id, String, Size)> =
            self.ui.windows().filter(|(known, ..)| self.windows.iter().all(|shown| shown.known != Some(*known))).map(|(known, title, size)| (known, title.into(), size)).collect();
        for (known, title, size) in new {
            // Where it goes, and on which output, the compositor decides, not the app.
            let opens = self.ui.window_opening(known).unwrap_or_default();
            let [size, min_size] = [size, opens.min_size].map(|size| [size.w, size.h].map(|side| side.round().max(1.0) as i32));
            let position = opens.position.unwrap_or_default();
            let opening =
                Opening { known: Some(known), title: &title, size, min_size, position, frameless: opens.frameless, transparent: false, maximized: false, minimized: false, icons: &self.icons };
            self.windows.push(Shown::open(&link, &text, opening));
            self.rerun = true;
        }
    }

    /// After a run, does what a window's interface asks of the window: redraw where it changed,
    /// a title, a cursor shape, minimize, maximize, fullscreen, activate, the portal file
    /// dialog; and the input methods hear the text cursor's place. (Staying above other
    /// programs is not possible under Wayland.)
    fn follow(&mut self, at: usize) {
        let mut changed = std::mem::take(&mut self.windows[at].sheet.changed);
        let asks = |ui: &mut Ui| {
            changed.extend(ui.changes());
            ui.changed();
            let window_state = (ui.window_state.title_bar != 0, ui.take_minimize(), ui.take_maximized(), ui.take_full_screen());
            (ui.take_title(), ui.cursor(), ui.ime_area(), ui.password_focused(), ui.take_on_top(), ui.take_bring_to_front(), ui.take_file_dialog_request(), ui.theme, window_state)
        };
        let asked = within(&mut self.ui, self.windows[at].known, asks);
        self.windows[at].sheet.changed = changed;
        let Some((title, cursor, ime_area, hidden, _, forward, files, theme, (title_bar, minimize, fill, whole))) = asked else { return };
        files.into_iter().for_each(|ask| self.ask_files(at, ask));
        self.apply_state(at, minimize, fill, whole);
        self.tell_typing(at, ime_area, hidden);
        if forward {
            self.forward(at);
        }
        let (link, shown) = (self.link.clone(), &mut self.windows[at]);
        (shown.grips, shown.cursor) = (shown.kind.frameless || title_bar, cursor);
        // A tray-hidden window has no role objects: it gets its title as it comes back.
        if let Some(title) = title {
            if shown.toplevel != 0 {
                link.wire.send(shown.toplevel, 2, &[Arg::Text(titled(&title))]);
            }
            shown.title = title;
        }
        if let Some(framed) = &mut shown.frame {
            framed.title_bar.title.clone_from(&shown.title);
            framed.title_bar.run(theme);
            framed.sheet.changed.extend(framed.title_bar.ui.changes());
            framed.title_bar.ui.changed();
        }
        if self.window_of(self.pointer.over).is_some_and(|(window, _)| window == at) {
            self.shape();
        }
    }

    /// Does what the app asked of a window's state, all through the compositor: minimize,
    /// maximize or restore (not while fullscreen), fullscreen or not.
    fn apply_state(&mut self, at: usize, minimize: bool, fill: Option<bool>, whole: Option<bool>) {
        let (wire, shown) = (&self.link.wire, &self.windows[at]);
        if shown.toplevel == 0 {
            return;
        }
        if let Some(on) = fill.filter(|_| !shown.lying_over) {
            wire.send(shown.toplevel, if on { 9 } else { 10 }, &[]);
        }
        match whole {
            Some(true) => wire.send(shown.toplevel, 11, &[Arg::Nothing]),
            Some(false) => wire.send(shown.toplevel, 12, &[]),
            None => {}
        }
        if minimize {
            wire.send(shown.toplevel, 13, &[]);
        }
    }

    /// Asks the compositor to activate a window: requests a token made with the last input
    /// serial and the focused surface (whose request the compositor judges); the window is
    /// activated when the token comes.
    fn forward(&mut self, at: usize) {
        let link = &self.link;
        if link.has.activation == 0 {
            return;
        }
        let token = link.wire.make(Interface::Token);
        link.wire.send(link.has.activation, 1, &[Arg::Uint(token)]);
        if link.serial.get() != 0 && link.has.seat != 0 {
            link.wire.send(token, 0, &[Arg::Uint(link.serial.get()), Arg::Uint(link.has.seat)]);
        }
        let asking = self.window_of(self.keys.focus).map_or(self.windows[at].sheet.surface, |(by, _)| self.windows[by].sheet.surface);
        link.wire.send(token, 2, &[Arg::Uint(asking)]);
        link.wire.send(token, 3, &[]);
        self.tokens.push((token, self.windows[at].sheet.surface));
    }

    /// Tells the input methods the text cursor's place in the focused window and whether its
    /// input is hidden: enabled while something is being typed into, disabled while nothing is.
    fn tell_typing(&mut self, at: usize, ime_area: Option<Rect>, hidden: bool) {
        let (wire, typing, shown) = (&self.link.wire, &mut self.typing, &self.windows[at]);
        if typing.id == 0 || typing.on != shown.sheet.surface {
            return;
        }
        let now = ime_area.map(|place| (place, hidden));
        if now == typing.told {
            return;
        }
        typing.told = now;
        match now {
            Some((place, hidden)) => {
                (!std::mem::replace(&mut typing.enabled, true)).then(|| wire.send(typing.id, 1, &[]));
                // A password: sensitive and hidden (0x40, 0x80), purpose password (8); else
                // plain text.
                let (hints, purpose) = if hidden { (0xc0, 8) } else { (0, 0) };
                wire.send(typing.id, 5, &[Arg::Uint(hints), Arg::Uint(purpose)]);
                let [x, y, w, h] = [place.x, place.y, place.w.max(1.0), place.h].map(|value| value as i32);
                wire.send(typing.id, 6, &[Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]);
            }
            None => {
                typing.enabled = false;
                wire.send(typing.id, 2, &[]);
            }
        }
        wire.send(typing.id, 7, &[]);
    }

    /// A window's app asked for the system file dialog: the portal is asked to show it over the
    /// window, by its exported handle.
    fn ask_files(&mut self, at: usize, ask: FileDialogRequest) {
        let (shown, Session { bus, portal, .. }) = (&self.windows[at], &mut self.session);
        portal.ask_for_files(bus.as_mut(), &shown.handle, shown.known.unwrap_or(0), &ask);
        self.heed();
    }

    /// Draws what changed in a window and its title bar where the compositor is ready. A window
    /// that was to minimize, or activate, as soon as it mapped does so once it has.
    fn draw(&mut self, at: usize) {
        let (link, ui) = (self.link.clone(), &mut self.ui);
        let shown = &mut self.windows[at];
        if !shown.configured {
            return;
        }
        let (scale, known) = (shown.scale, shown.known);
        let mut title_bar_drawn = false;
        if let Some(Framed { title_bar, sheet, .. }) = &mut shown.frame {
            title_bar_drawn = sheet.draw(&link, scale, &mut |canvas| title_bar.ui.paint(canvas));
        }
        shown.sheet.owed |= title_bar_drawn;
        let drawn = shown.sheet.draw(&link, scale, &mut |canvas| _ = within(ui, known, |ui| ui.paint(canvas)));
        if drawn {
            shown.icon_taken(&link);
        }
        if drawn && std::mem::take(&mut shown.minimize) {
            link.wire.send(shown.toplevel, 13, &[]);
        }
        let (come, surface) = (drawn && std::mem::take(&mut shown.come), shown.sheet.surface);
        if come && at == 0 {
            let token = std::mem::take(&mut self.token);
            link.wire.send(link.has.activation, 2, &[Arg::Text(&token), Arg::Uint(surface)]);
        } else if come {
            self.forward(at);
        }
        #[cfg(feature = "gpu")]
        if at == 0 {
            self.tell_renderer();
        }
    }

    /// The session bus has messages, as under X: the portal and screen readers each take
    /// theirs.
    fn hear_session(&mut self) {
        let (Session { bus, portal, readers }, desktop, rerun) = (&mut self.session, crate::x11::DESKTOP.get(), &mut self.rerun);
        let hear = &mut |bus: &mut Bus, message: &BusMessage| {
            portal.hear(bus, message);
            readers.hear_session(bus, message);
            *rerun |= desktop.is_some_and(|desktop| (desktop.heard)(bus, message));
        };
        bus.iter_mut().for_each(|bus| bus.turn(hear));
        if bus.take_if(|bus| bus.dead).is_some() {
            portal.gone();
        }
        self.heed();
    }

    /// Takes in what the desktop said: the user's preferences (dark, contrast, reduced motion)
    /// from the portal; there are no XSETTINGS here, so what it does not say is not asked for.
    /// And whether it has a file dialog, and the answers of those that ended.
    fn heed(&mut self) {
        let portal = &mut self.session.portal;
        self.rerun |= self.ui.set_preferences(portal.preferences(Preferences::default()));
        self.rerun |= crate::locale::set_desktop_clock(portal.twelve_hour_clock);
        self.ui.set_native_file_dialog(portal.files);
        for (known, file) in std::mem::take(&mut portal.answers) {
            let shown = self.windows.iter().find(|shown| shown.known.unwrap_or(0) == known);
            self.rerun |= shown.and_then(|shown| within(&mut self.ui, shown.known, |ui| ui.file_dialog_answered(file))).is_some();
        }
    }

    /// After a run a screen reader listens to: each window's content is handed on, as under X.
    /// (A window's screen position is unknown here: its corner is reported as the screen's.)
    fn tell_readers(&mut self) {
        for shown in &self.windows {
            let told = within(&mut self.ui, shown.known, |ui| self.session.readers.keep(ui, shown.known.unwrap_or(0), &shown.title, shown.sheet.pixels, shown.scale));
            self.rerun |= told == Some(true);
        }
        self.session.readers.settle();
    }

    /// The screen readers' bus has messages, as under X.
    fn hear_readers(&mut self) {
        let readers = &mut self.session.readers;
        for deed in readers.turn(&|_| [0; 2]) {
            let shown = self.windows.iter().find(|shown| shown.known.unwrap_or(0) == deed.window);
            shown.and_then(|shown| within(&mut self.ui, shown.known, |ui| readers.act(ui, deed)));
            self.rerun = true;
        }
        let listening = readers.listening();
        if listening != self.ui.listening {
            for shown in &self.windows {
                within(&mut self.ui, shown.known, |ui| ui.set_screen_reader(listening));
            }
            self.rerun = true;
        }
    }

    /// Sleeps until the compositor has events, another thread asks for a run, the session bus
    /// or a screen reader has messages, a held key is due to repeat, or the time comes that the
    /// interface asked to be woken at, or a silent portal is given up on. (Not at all when a
    /// run is due.)
    fn sleep(&mut self) {
        self.link.wire.flush();
        if self.rerun {
            return;
        }
        let buses = [self.session.bus.as_ref().map_or((-1, 0), Bus::waits), self.session.readers.waits()];
        let [session, desk] = buses.map(|(fd, events)| PollFd { fd, events, revents: 0 });
        let mut waiting = [PollFd { fd: self.link.wire.fd(), events: 1, revents: 0 }, PollFd { fd: self.wake, events: 1, revents: 0 }, session, desk];
        let now = self.start.elapsed();
        let patience = self.session.portal.patience_ends().map(|ends| ends.saturating_sub(now));
        let held = self.keys.held.map(|(_, due)| due.saturating_sub(now));
        let wait = [self.ui.wake_in(), patience, held].into_iter().flatten().min().map_or(-1, |wait| wait.as_millis() as c_int + 1);
        // SAFETY: the files are open; `read` drains what woke the windows, there when `poll`
        // says so: two bytes at most (see `x11::WOKEN`).
        unsafe {
            poll(waiting.as_mut_ptr(), 4, wait);
            // The wake is noted first, then drained: a wake arriving between is drained with
            // it, and the run comes after both.
            if waiting[1].revents != 0 {
                WOKEN.store(false, std::sync::atomic::Ordering::SeqCst);
                read(self.wake, [0u8; 64].as_mut_ptr().cast(), 64);
            }
        }
        (self.rerun, self.heard) = (self.rerun || waiting[1].revents != 0, waiting[2].revents | waiting[3].revents != 0);
    }
}

/// How the windows take drops of files and text: a drag is accepted over a window whose app
/// asked about drops, and that window receives it. Set by the first call asking about drops
/// ([`Ui::files_hovered`], [`Ui::files_dropped`], [`Ui::text_hovered`], [`Ui::text_dropped`],
/// through `platform::hand_drops`); reached no other way, so a program that never asks is built
/// without it and refuses drags over its windows.
#[derive(Clone, Copy)]
struct Drops {
    /// A data device drag event over a window: enter, leave, motion or drop. Says whether an
    /// interface must run.
    drag_event: fn(Taking<'_>, Message) -> bool,
}

/// After a run, what a Wayland window does for the app's desktop requests (see `x11::Desktop`,
/// whose session-bus handling is shared): set by the first call asking for any (through
/// `platform::hand_desktop`); reached no other way, so a program that never asks is built
/// without it.
type DesktopRequests = fn(&Link, &mut Shown, &mut Ui, &mut Session) -> bool;

thread_local! {
    /// What the windows do for the desktop after a run: nothing until an app on this thread
    /// asks (see `DesktopRequests`).
    static DESKTOP: Cell<Option<DesktopRequests>> = const { Cell::new(None) };
}

/// Hands the windows their desktop integration, on this thread (see `DesktopRequests`).
pub(crate) fn hand_desktop() {
    DESKTOP.set(Some(stood));
}

/// After a run under Wayland: the app's requests go to the desktop over the session bus (global
/// shortcuts to the portal, which shows its prompts over the first window); and the first
/// window is hidden (unmapped: the compositor removes it) or shown again and activated: for a
/// later start of the program, or where its app asked. It activates with the token the desktop
/// gave along with what caused it (a tray or notification click, a hotkey, a later start): the
/// user's own request. Without one, the window asks as it does by itself (`Host::forward`).
/// Says whether its interface must run.
fn stood(link: &Link, first: &mut Shown, ui: &mut Ui, session: &mut Session) -> bool {
    let asked = crate::freedesktop::ran(session.bus.as_mut(), true, &first.handle);
    let mut news = false;
    if asked.hide && !first.withdrawn {
        first.withdraw(link);
        news |= ui.event(Event::Minimized(true));
    }
    let forward = asked.forward || ui.take_bring_to_front();
    if forward && first.withdrawn {
        // Shown again as a window is first shown: its role recreated, a commit without a
        // buffer, then the compositor configures it and all of it is drawn.
        first.role(link);
        first.withdrawn = false;
        first.sheet.waiting = 0;
        first.sheet.hurry = true;
        first.sheet.changed = vec![rect(0.0, 0.0, first.size[0] as f32, first.size[1] as f32)];
        link.wire.send(first.sheet.surface, 6, &[]);
        news |= ui.event(Event::Minimized(false));
    }
    // A token comes from another program: one longer than a desktop gives is none.
    let tokenless = asked.token.is_empty() || asked.token.len() > crate::freedesktop::TOKEN;
    match (forward, tokenless || link.has.activation == 0) {
        (true, false) => link.wire.send(link.has.activation, 2, &[Arg::Text(&asked.token), Arg::Uint(first.sheet.surface)]),
        (true, true) => ui.bring_to_front(),
        _ => {}
    }
    news
}

/// Hands the windows their drop handling, on this thread (see `Drops`).
pub(crate) fn hand_drops() {
    DROPS.set(Some(Drops { drag_event: |taking, message| taking.drag_event(message) }));
}

/// What taking in drops works with: the connection, the first window's interface (lending out
/// the others'), the windows, and the drag over one of them; and whether an interface must run.
struct Taking<'a> {
    link: &'a Link,
    ui: &'a mut Ui,
    windows: &'a [Shown],
    drag_over: &'a mut DragOver,
    rerun: bool,
}

impl Taking<'_> {
    /// A data device drag event over a window: enter, leave, motion or drop. Says whether an
    /// interface must run.
    fn drag_event(mut self, mut message: Message) -> bool {
        match message.opcode {
            1 => {
                let (serial, surface, x, y, offer) = (message.uint(), message.uint(), message.fixed(), message.fixed(), message.uint());
                self.came(serial, surface, pt(x, y), offer);
            }
            2 => self.left(),
            3 => {
                let (_, x, y) = (message.uint(), message.fixed(), message.fixed());
                self.moved(pt(x, y));
            }
            4 => self.let_go(),
            _ => {}
        }
        self.rerun
    }

    /// The window whose client surface this is, if theirs. (Over a title bar nothing is
    /// taken.)
    fn window_of(&self, surface: u32) -> Option<usize> {
        self.windows.iter().position(|shown| shown.sheet.surface == surface)
    }

    /// Passes what is dragged over a window to its interface, noting whether it must run.
    fn hear(&mut self, at: usize, event: Event) {
        self.rerun |= within(self.ui, self.windows[at].known, |ui| ui.event(event)) == Some(true);
    }

    /// A drag from another program entered a window. If the window's app asks about files and
    /// they are offered, or else about text and that is offered (UTF-8, under either name), the
    /// source is told it may drop here, as a copy, and the window's interface hears it came and
    /// where. Anything else is refused.
    fn came(&mut self, serial: u32, surface: u32, at: Point, offer: u32) {
        let link = self.link;
        let window = self.window_of(surface);
        let asks = window.and_then(|window| within(self.ui, self.windows[window].known, |ui| [ui.takes_files.get(), ui.crossing.takes_text.get()]));
        let [files, text] = asks.unwrap_or_default();
        let wanted = [(files, FILES), (text, TEXT[0]), (text, TEXT[1])].into_iter().filter(|(takes, _)| *takes && offer != 0);
        let kind = wanted.map(|(_, kind)| kind).find(|kind| link.offers(offer, kind));
        if offer != 0 {
            link.wire.send(offer, 0, &[Arg::Uint(serial), kind.map_or(Arg::Nothing, Arg::Text)]);
        }
        if offer != 0 && link.has.data_version >= 3 {
            let copies = kind.is_some() as u32;
            link.wire.send(offer, 4, &[Arg::Uint(copies), Arg::Uint(copies)]);
        }
        let (Some(window), Some(kind)) = (window, kind) else { return link.forget(offer) };
        *self.drag_over = DragOver { offer, surface, at, kind };
        self.hear(window, if kind == FILES { Event::FilesHovered(true) } else { Event::TextHovered(true) });
        self.hear(window, Event::Move(at));
    }

    /// The drag moved over the window: its interface hears where.
    fn moved(&mut self, at: Point) {
        self.drag_over.at = at;
        if let Some(window) = self.window_of(self.drag_over.surface).filter(|_| self.drag_over.offer != 0) {
            self.hear(window, Event::Move(at));
        }
    }

    /// The drag left the window: its interface hears it gone, and the pointer with it. The
    /// offer is destroyed.
    fn left(&mut self) {
        let DragOver { offer, surface, kind, .. } = std::mem::take(self.drag_over);
        if let Some(window) = self.window_of(surface).filter(|_| offer != 0) {
            self.hear(window, if kind == FILES { Event::FilesHovered(false) } else { Event::TextHovered(false) });
            self.hear(window, Event::Leave);
        }
        self.link.forget(offer);
    }

    /// The drag was dropped over the window: it is read from the source, the window's interface
    /// gets it at the drop point, and the source hears it is finished.
    fn let_go(&mut self) {
        let (link, DragOver { offer, surface, at, kind }) = (self.link, std::mem::take(self.drag_over));
        let Some(window) = self.window_of(surface).filter(|_| offer != 0) else { return };
        let got = link.receive(offer, kind).unwrap_or_default();
        if link.has.data_version >= 3 {
            link.wire.send(offer, 3, &[]);
        }
        link.forget(offer);
        let files = kind == FILES;
        self.hear(window, if files { Event::FilesHovered(false) } else { Event::TextHovered(false) });
        let event = if files { Event::FilesDropped(files_in(&got)) } else { Event::TextDropped(String::from_utf8_lossy(&got).into_owned()) };
        // An empty transfer, or a file list with no file, is nothing to take.
        if !got.is_empty() && !matches!(&event, Event::FilesDropped(files) if files.is_empty()) {
            self.hear(window, Event::Move(at));
            self.hear(window, event);
        }
    }
}

/// How long a program that ends waits for the compositor's last answer (see `Link::part`): a
/// compositor answers in a few ms; one that hangs holds the program up no longer than this.
const PARTING: Duration = Duration::from_millis(250);

impl<App> Drop for Host<App> {
    fn drop(&mut self) {
        let link = self.link.clone();
        self.windows.drain(..).for_each(|shown| shown.close(&link));
        link.part(PARTING);
    }
}

/// Opens the window and runs the app in it until it is closed: under Wayland where a compositor
/// answers, else under X (see this module's top for how).
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    let asked = std::env::var("TAUT_BACKEND").ok();
    let (link, outputs) = match choose(asked.as_deref(), Link::new) {
        Chosen::Wayland(link) => link,
        Chosen::X => return crate::x11::run(window, app),
        Chosen::Neither => return eprintln!("taut: Wayland was asked for, and no compositor answers where WAYLAND_DISPLAY says."),
    };
    // The session bus is asked first, as under X.
    let session = Session::on(Bus::session());
    let Some(text) = SystemText::new() else {
        return eprintln!("taut: this needs libfreetype and libfontconfig, and a font.");
    };
    let Some(mut host) = Host::open(link, outputs, text, session, window, app) else {
        return eprintln!("taut: the compositor did not show the window.");
    };
    while !host.done {
        host.sleep();
        host.turn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositors::tests::{Desk, found};
    use crate::platform::LocalClipboard;
    use crate::surface::Surface;
    use crate::text::BuiltInFont;
    use crate::wayland_wire::Named;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, MutexGuard, mpsc};

    /// The globals a window-showing compositor must offer, each at a version.
    const SHOWS_WINDOWS: [(&str, u32); 3] = [("wl_compositor", 7), ("wl_shm", 1), ("xdg_wm_base", 6)];

    /// A test compositor at the other end of a socket offering these globals, answering the
    /// program's first round trip, then only listening. With what the program made of it.
    fn offering(offers: &[(&str, u32)]) -> (Wire, Option<Linked>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let compositor = Wire::over(theirs);
        // The program's registry is object 2, its first callback 3.
        for (name, (interface, version)) in offers.iter().enumerate() {
            compositor.send(2, 0, &[Arg::Uint(name as u32 + 1), Arg::Text(interface), Arg::Uint(*version)]);
        }
        compositor.send(3, 0, &[Arg::Uint(0)]);
        compositor.flush();
        (compositor, Link::over(Wire::over(ours)))
    }

    /// What the program sent the compositor since last asked: each message as object, opcode
    /// and its numbers.
    fn said(compositor: &Wire, link: &Link) -> Vec<(u32, u16, Vec<i32>)> {
        link.wire.flush();
        compositor.read();
        std::iter::from_fn(|| compositor.next()).map(|message| (message.object, message.opcode, message.words())).collect()
    }

    #[test]
    fn x_is_chosen_unless_a_compositor_answers_or_wayland_is_asked_for() {
        let answering = || Some(1);
        let silent = || None::<u8>;
        assert!(matches!(choose(None, answering), Chosen::Wayland(1)));
        assert!(matches!(choose(None, silent), Chosen::X));
        assert!(matches!(choose(Some("x11"), || -> Option<u8> { panic!("a compositor is not asked for when X is") }), Chosen::X));
        assert!(matches!(choose(Some("X11"), answering), Chosen::X));
        assert!(matches!(choose(Some("wayland"), silent), Chosen::Neither));
        assert!(matches!(choose(Some("Wayland"), answering), Chosen::Wayland(1)));
        assert!(matches!(choose(Some("something else"), silent), Chosen::X), "anything else: as the environment has it");
        // Where nobody listens on the named socket, no compositor answers.
        let nobody = std::env::temp_dir().join(format!("taut-nobody-{}", std::process::id()));
        assert!(matches!(choose(None, || Wire::connect_to(Named::At(nobody)).and_then(Link::over)), Chosen::X));
    }

    #[test]
    fn a_compositor_is_taken_where_it_shows_windows_and_spoken_to_in_versions_both_know() {
        let (compositor, link) = offering(&SHOWS_WINDOWS);
        let (link, outputs) = link.expect("a compositor that shows windows");
        assert_eq!([link.has.compositor, link.has.shm, link.has.shell, link.has.seat], [4, 5, 6, 0]);
        assert!(outputs.is_empty());
        // Each bound at the newest version both support: the compositor offers 7, this speaks
        // 6.
        let binds: Vec<(u32, Vec<i32>)> = said(&compositor, &link).into_iter().filter(|(object, opcode, _)| (*object, *opcode) == (2, 0)).map(|(_, _, words)| (words[0] as u32, words)).collect();
        assert_eq!(binds.len(), 3);
        assert_eq!(binds[0].1[binds[0].1.len() - 2..], [6, 4], "wl_compositor in version 6, as object 4");
        let (_, none) = offering(&SHOWS_WINDOWS[..2]);
        assert!(none.is_none(), "with no xdg_wm_base, no window can be shown");
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        assert!(Link::over(Wire::over(ours)).is_none(), "one that hangs up answers nothing");
    }

    /// Lists the globals the named compositor offers with versions, for comparing compositors
    /// (run with `--nocapture`).
    #[test]
    #[ignore]
    fn what_the_compositor_offers() {
        let Some(wire) = Wire::connect() else { return eprintln!("no compositor: not tested") };
        let registry = wire.make(Interface::Registry);
        wire.send(1, 1, &[Arg::Uint(registry)]);
        let mut offered = Link::offered(&wire).expect("the compositor lists what it offers");
        offered.sort_by(|a, b| a.1.cmp(&b.1));
        let desk = std::env::var("TAUT_DESK").unwrap_or_default();
        offered.iter().for_each(|(_, interface, version)| println!("offers {desk} {interface} {version}"));
        let desk = Desk::new().expect("the compositor answers again");
        println!("screen {} {:?} pixels at {}, {:?} units; hands: {}, fingers: {}", desk.name, desk.pixels, desk.scale, desk.units(), desk.has_hands(), desk.has_fingers());
    }

    #[test]
    fn edges_and_shapes_have_the_compositors_numbers() {
        let edges = [(-1, -1, 5), (0, -1, 1), (1, -1, 9), (-1, 0, 4), (1, 0, 8), (-1, 1, 6), (0, 1, 2), (1, 1, 10)];
        for (across, down, number) in edges {
            assert_eq!(edges_number(across, down), number, "{across}, {down}");
        }
        assert_eq!([Cursor::Arrow, Cursor::Hand, Cursor::Text, Cursor::ResizeDown].map(shape_number), [1, 4, 9, 29]);
    }

    /// A label with a count and a button, as the test surfaces show.
    fn counted(count: &u32, ui: &mut Ui) {
        ui.column().pad(10.0).gap(8.0).open();
        ui.label(&format!("{count} pressed"));
        ui.button("Press");
        ui.close();
    }

    /// An interface with the built-in font, this big, run once with a count.
    fn interface(size: Size, scale: f32, count: u32) -> Ui {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(size, scale);
        ui.run(&mut |ui| counted(&count, ui));
        ui
    }

    /// A surface's damage, in pixels: left, top, right, bottom.
    fn damaged(said: &[(u32, u16, Vec<i32>)], surface: u32) -> Vec<[i32; 4]> {
        let told = said.iter().filter(|(object, opcode, _)| (*object, *opcode) == (surface, 9));
        told.map(|(_, _, words)| [words[0], words[1], words[0] + words[2], words[1] + words[3]]).collect()
    }

    #[test]
    fn a_surface_is_drawn_into_shared_memory_and_only_what_changed_is_handed_over() {
        let (compositor, link) = offering(&[SHOWS_WINDOWS[0], SHOWS_WINDOWS[1], SHOWS_WINDOWS[2], ("wp_viewporter", 1)]);
        let (link, _) = link.unwrap();
        let size = Size { w: 200.0, h: 120.0 };
        // At an integer scale, and a fractional one shown by viewport size.
        for (scale, finer) in [(1.0, false), (2.0, false), (1.5, true)] {
            let mut ui = interface(size, scale, 0);
            let mut sheet = Sheet::new(&link, false);
            if finer {
                sheet.view(&link);
            }
            sheet.size(&link, [200, 120], scale);
            ui.changes();
            assert!(sheet.draw(&link, scale, &mut |canvas| ui.paint(canvas)));
            let afresh = Surface::draw(&mut ui, 64);
            assert_eq!(sheet.buffers[0].size, [afresh.width as i32, afresh.height as i32]);
            assert!(sheet.buffers[0].memory.pixels() == afresh.pixels, "at {scale}: the buffer holds the interface as it is drawn afresh");
            let first = said(&compositor, &link);
            assert_eq!(damaged(&first, sheet.surface), [[0, 0, afresh.width as i32, afresh.height as i32]], "all of it is new");
            let asked = |opcode: u16| first.iter().any(|(object, said, _)| (*object, *said) == (sheet.surface, opcode));
            assert!(asked(1) && asked(3) && asked(6), "the buffer is handed over, the compositor asked to say when it shows, and it is committed");
            if finer {
                assert!(first.iter().any(|(object, opcode, words)| (*object, *opcode, &words[..]) == (sheet.viewport, 2, &[200, 120][..])), "shown at its size in units");
            }
            // The count rises before the compositor showed the first frame: nothing is drawn
            // until it has.
            let mut count = 5;
            ui.run(&mut |ui| counted(&count, ui));
            sheet.changed.extend(ui.changes());
            assert!(!sheet.draw(&link, scale, &mut |canvas| ui.paint(canvas)));
            // It has; it still holds the first buffer, so a second is made, lacking everything;
            // but the compositor is told only what changed.
            sheet.waiting = 0;
            let changed = sheet.changed.clone();
            assert!(sheet.draw(&link, scale, &mut |canvas| ui.paint(canvas)));
            assert_eq!(sheet.buffers.len(), 2);
            assert!(sheet.buffers[1].memory.pixels() == Surface::draw(&mut ui, 64).pixels);
            let told = damaged(&said(&compositor, &link), sheet.surface);
            assert_eq!(told, changed.iter().map(|area| on_pixels(*area, scale, sheet.pixels)).collect::<Vec<_>>());
            assert!(told.iter().all(|[left, top, right, bottom]| (right - left) * (bottom - top) < afresh.width as i32 * afresh.height as i32 / 4), "a small part of it: {told:?}");
            // The compositor releases the first buffer; the next change is drawn into it, with
            // what it lacks since its last commit.
            let first_buffer = sheet.buffers[0].id;
            assert!(sheet.released(&link, first_buffer));
            (sheet.waiting, count) = (0, 77);
            ui.run(&mut |ui| counted(&count, ui));
            sheet.changed.extend(ui.changes());
            assert!(sheet.draw(&link, scale, &mut |canvas| ui.paint(canvas)));
            assert_eq!(sheet.buffers.len(), 2, "no third");
            assert!(sheet.buffers[0].busy && sheet.buffers[0].memory.pixels() == Surface::draw(&mut ui, 64).pixels, "brought up to date");
            sheet.destroy(&link);
            said(&compositor, &link);
        }
    }

    #[test]
    fn a_surface_of_another_size_lets_go_of_its_buffers_as_the_compositor_does() {
        let (compositor, link) = offering(&SHOWS_WINDOWS);
        let (link, _) = link.unwrap();
        let mut ui = interface(Size { w: 100.0, h: 80.0 }, 1.0, 0);
        let mut sheet = Sheet::new(&link, false);
        sheet.size(&link, [100, 80], 1.0);
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        let held = sheet.buffers[0].id;
        // Enlarged while the compositor holds the buffer: it stays until released.
        ui.resize(Size { w: 140.0, h: 90.0 }, 1.0);
        ui.run(&mut |ui| counted(&3, ui));
        sheet.size(&link, [140, 90], 1.0);
        assert!(sheet.hurry, "a window of a new size is handed over without waiting for the last frame");
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        assert_eq!(sheet.buffers.iter().map(|buffer| buffer.size).collect::<Vec<_>>(), [[100, 80], [140, 90]]);
        assert!(sheet.released(&link, held));
        assert_eq!(sheet.buffers.len(), 1, "the old one went when it was let go");
        let gone = said(&compositor, &link).iter().any(|(object, opcode, _)| (*object, *opcode) == (held, 0));
        assert!(gone, "and the compositor was told");
    }

    // ---- fingers, pens and pinches from a test-played compositor ----

    /// The objects a test-played compositor tracks, by the program's ids: each bound global (by
    /// interface), the window's surface and role objects, and the seat and tablet objects.
    #[derive(Default, Debug)]
    struct Played {
        bound: Vec<(String, u32)>,
        surface: u32,
        xdg: u32,
        toplevel: u32,
        pointer: u32,
        touch: u32,
        tablets: u32,
        pinch: u32,
        configured: bool,
    }

    impl Played {
        /// The program's object for a bound global.
        fn of(&self, interface: &str) -> u32 {
            self.bound.iter().find(|(name, _)| name == interface).map_or(0, |(_, id)| *id)
        }

        /// Takes in what the program sent, as the played compositor: notes created objects,
        /// sends the seat's capabilities (`seat`: pointer 1, touch 4), and answers a round trip
        /// after first configuring the window. Says whether it answered one after the window
        /// was shown.
        fn heard(&mut self, compositor: &Wire, offers: &[(&str, u32)], seat: u32, object: u32, opcode: u16, words: &[i32]) -> bool {
            let first = words.first().copied().unwrap_or(0) as u32;
            match (object, opcode) {
                (2, 0) => {
                    let interface = offers.get(first as usize - 1).map_or("", |(interface, _)| interface);
                    let id = *words.last().unwrap() as u32;
                    self.bound.push((interface.to_string(), id));
                    if interface == "wl_seat" {
                        compositor.send(id, 0, &[Arg::Uint(seat)]);
                    }
                }
                (1, 0) => {
                    if self.toplevel != 0 && !self.configured {
                        compositor.send(self.toplevel, 0, &[Arg::Int(0), Arg::Int(0), Arg::Nothing]);
                        compositor.send(self.xdg, 0, &[Arg::Uint(7)]);
                        self.configured = true;
                    }
                    compositor.send(first, 0, &[Arg::Uint(0)]);
                    compositor.flush();
                    return self.configured;
                }
                _ if object == self.of("wl_compositor") && opcode == 0 && self.surface == 0 => self.surface = first,
                _ if object == self.of("xdg_wm_base") && opcode == 2 => self.xdg = first,
                _ if object == self.xdg && opcode == 1 => self.toplevel = first,
                _ if object == self.of("wl_seat") && opcode == 0 => self.pointer = first,
                _ if object == self.of("wl_seat") && opcode == 2 => self.touch = first,
                _ if object == self.of("zwp_tablet_manager_v2") && opcode == 0 => self.tablets = first,
                _ if object == self.of("zwp_pointer_gestures_v1") && opcode == 1 => self.pinch = first,
                _ => {}
            }
            compositor.flush();
            false
        }
    }

    /// The played compositor's globals: a window, a seat, touchpad pinches and tablets.
    const PLAYED: [(&str, u32); 6] = [("wl_compositor", 6), ("wl_shm", 1), ("xdg_wm_base", 6), ("wl_seat", 8), ("zwp_pointer_gestures_v1", 3), ("zwp_tablet_manager_v2", 2)];

    /// An app's window shown by a test-played compositor (on a thread until the window shows),
    /// its seat having `seat`. With the compositor's socket end, its notes, and the windows.
    fn played(seat: u32, app: impl FnMut(&mut Ui) + 'static) -> Option<(Wire, Played, Windows)> {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let compositor = std::thread::spawn(move || {
            let (compositor, mut played) = (Wire::over(theirs), Played::default());
            // The program's registry is object 2, its first callback 3.
            for (name, (interface, version)) in PLAYED.iter().enumerate() {
                compositor.send(2, 0, &[Arg::Uint(name as u32 + 1), Arg::Text(interface), Arg::Uint(*version)]);
            }
            compositor.send(3, 0, &[Arg::Uint(0)]);
            compositor.flush();
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until && compositor.read() {
                let said: Vec<(u32, u16, Vec<i32>)> = std::iter::from_fn(|| compositor.next()).map(|message| (message.object, message.opcode, message.words())).collect();
                if said.iter().fold(false, |shown, (object, opcode, words)| played.heard(&compositor, &PLAYED, seat, *object, *opcode, words) || shown) {
                    break;
                }
                readable(compositor.fd(), 20);
            }
            (compositor, played)
        });
        let (link, outputs) = Link::over(Wire::over(ours))?;
        let window = Window { title: "taut touched", width: 300.0, height: 300.0, ..Default::default() };
        let mut host = Host::open(link, outputs, SystemText::new()?, Session::on(None), window, Box::new(app) as Box<dyn FnMut(&mut Ui)>)?;
        let (compositor, mut played) = compositor.join().unwrap();
        // What the program requested once it heard the seat's capabilities.
        host.turn();
        played_said(&compositor, &mut played, seat);
        Some((compositor, played, host))
    }

    /// The played compositor takes in what the program sent since.
    fn played_said(compositor: &Wire, played: &mut Played, seat: u32) {
        readable(compositor.fd(), 50);
        compositor.read();
        let said: Vec<(u32, u16, Vec<i32>)> = std::iter::from_fn(|| compositor.next()).map(|message| (message.object, message.opcode, message.words())).collect();
        said.iter().for_each(|(object, opcode, words)| _ = played.heard(compositor, &PLAYED, seat, *object, *opcode, words));
    }

    /// The played compositor sends events, and the windows take them in and run.
    fn plays(compositor: &Wire, host: &mut Windows, said: &[(u32, u16, &[Arg])]) {
        said.iter().for_each(|(object, opcode, args)| compositor.send(*object, *opcode, args));
        compositor.flush();
        readable(host.link.wire.fd(), 200);
        host.turn();
        host.turn();
    }

    /// The touch tests' app state: button presses, list scroll, the zoom box's events, and what
    /// the pen reported each run, with whether the box was held down.
    #[derive(Default)]
    struct Fingered {
        clicked: u32,
        at: [Rect; 2],
        list: Id,
        zooms: Vec<f32>,
        pens: Vec<(Option<Stylus>, bool)>,
    }

    /// A button, a zoom box, and a scrolling list.
    fn fingered(kept: &mut Fingered, ui: &mut Ui) {
        ui.column().grow().open();
        let button = ui.button("Tap");
        let clipboard = ui.el().w(crate::Length::Grow(1.0)).h(crate::Length::Px(60.0)).sense().show();
        kept.zooms.extend(ui.zoom(&clipboard).map(|zoom| zoom.factor));
        kept.pens.push((ui.stylus(), clipboard.pressed));
        let list = ui.column().grow().scroll().open();
        (0..80).for_each(|row| _ = ui.label(&format!("Row {row}")));
        ui.close();
        ui.close();
        (kept.clicked, kept.at, kept.list) = (kept.clicked + button.clicked as u32, [button.rect, clipboard.rect], list.id);
    }

    /// The test app shown by a played compositor whose seat has `seat`.
    fn fingered_shown(seat: u32) -> Option<(Wire, Played, Windows, Rc<RefCell<Fingered>>)> {
        let kept = Rc::new(RefCell::new(Fingered::default()));
        let described = kept.clone();
        let (compositor, played, host) = played(seat, move |ui| fingered(&mut described.borrow_mut(), ui))?;
        Some((compositor, played, host, kept))
    }

    /// A finger taps the button: it is pressed (as an emulated mouse without the feature
    /// `touch`, as a tap with it). Dragged up the list, with the feature it scrolls; without,
    /// it is a mouse drag over words and nothing scrolls. Without the feature a second finger
    /// is nothing; a touch cancel clicks nothing.
    #[test]
    fn a_finger_on_the_screen_taps_and_scrolls_as_the_feature_says() {
        let _alone = crate::x11::tests::alone();
        let Some((compositor, played, mut host, kept)) = fingered_shown(1 | 4) else { return eprintln!("no font: not tested") };
        assert!(played.touch != 0 && host.fingers.id == played.touch, "the touch screen was asked for: {played:?}");
        let (touch, surface) = (played.touch, played.surface);
        let finger = |opcode: u16, finger: i32, at: Point| -> Vec<Arg<'static>> {
            match opcode {
                0 => vec![Arg::Uint(7), Arg::Uint(1), Arg::Uint(surface), Arg::Int(finger), Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)],
                1 => vec![Arg::Uint(8), Arg::Uint(2), Arg::Int(finger)],
                _ => vec![Arg::Uint(3), Arg::Int(finger), Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)],
            }
        };
        let button = middle(kept.borrow().at[0]);
        plays(&compositor, &mut host, &[(touch, 0, &finger(0, 5, button)), (touch, 3, &[])]);
        plays(&compositor, &mut host, &[(touch, 1, &finger(1, 5, button)), (touch, 3, &[])]);
        lets(&mut host, 100);
        assert_eq!(kept.borrow().clicked, 1, "a tap presses the button");
        // Dragged up the list, in steps.
        let list = kept.borrow().list;
        let from = pt(100.0, 280.0);
        plays(&compositor, &mut host, &[(touch, 0, &finger(0, 6, from)), (touch, 3, &[])]);
        for step in 1..=8 {
            std::thread::sleep(Duration::from_millis(16));
            plays(&compositor, &mut host, &[(touch, 2, &finger(2, 6, pt(from.x, from.y - 20.0 * step as f32))), (touch, 3, &[])]);
        }
        plays(&compositor, &mut host, &[(touch, 1, &finger(1, 6, from)), (touch, 3, &[])]);
        lets(&mut host, 100);
        let scrolled = host.ui.scrolls.get(&list).map_or(0.0, |scrolled| scrolled[1]);
        assert_eq!(scrolled > 100.0, cfg!(feature = "touch"), "the list scrolled {scrolled} (the feature: {})", cfg!(feature = "touch"));
        // A finger lands and the compositor cancels it: nothing is clicked.
        plays(&compositor, &mut host, &[(touch, 0, &finger(0, 7, button)), (touch, 3, &[])]);
        plays(&compositor, &mut host, &[(touch, 4, &[])]);
        lets(&mut host, 600);
        assert_eq!(kept.borrow().clicked, 1, "a finger taken away clicks nothing");
        assert!(host.fingers.down.is_empty() && host.fingers.mouse.is_none());
    }

    /// A pen near the window reports pressure (only while touching), tilt and rotation, its
    /// button, and whether it is the eraser; its tip presses the box under it, then it leaves.
    /// (Pens are taken as such with the feature `touch`.)
    #[cfg(feature = "touch")]
    #[test]
    fn a_pen_says_how_hard_it_presses_how_it_leans_and_whether_it_rubs_out() {
        let _alone = crate::x11::tests::alone();
        let Some((compositor, played, mut host, kept)) = fingered_shown(1) else { return eprintln!("no font: not tested") };
        assert!(played.tablets != 0 && host.pens.seat == played.tablets, "the seat's tablets were asked for: {played:?}");
        // A tablet and a pen arrive: one reporting pressure and tilt.
        let (tablet, pen) = (0xff00_0000, 0xff00_0001);
        let at = middle(kept.borrow().at[1]);
        let near = [Arg::Uint(9), Arg::Uint(tablet), Arg::Uint(played.surface)];
        let place = [Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)];
        plays(&compositor, &mut host, &[(played.tablets, 0, &[Arg::Uint(tablet)]), (played.tablets, 1, &[Arg::Uint(pen)])]);
        plays(&compositor, &mut host, &[(pen, 0, &[Arg::Uint(0x140)]), (pen, 3, &[Arg::Uint(2)]), (pen, 3, &[Arg::Uint(1)]), (pen, 4, &[])]);
        // It comes into proximity, tilted, pressing nothing.
        let leaning = [Arg::Fixed(30.0), Arg::Fixed(-15.0)];
        plays(&compositor, &mut host, &[(pen, 6, &near), (pen, 10, &place), (pen, 13, &leaning), (pen, 11, &[Arg::Uint(40_000)]), (pen, 18, &[Arg::Uint(1)])]);
        let hovers = Stylus { pressure: 0.0, tilt: [30.0, -15.0], twist: 0.0, barrel: false, eraser: false, touching: false };
        assert_eq!(kept.borrow().pens.last(), Some(&(Some(hovers), false)), "near, and touching nothing");
        // It touches at three-quarter pressure, rotated: the box is pressed.
        let turned = [Arg::Fixed(-90.0)];
        plays(&compositor, &mut host, &[(pen, 8, &[Arg::Uint(10)]), (pen, 11, &[Arg::Uint(49_151)]), (pen, 14, &turned), (pen, 18, &[Arg::Uint(2)])]);
        let (stylus, pressed) = kept.borrow().pens.last().copied().unwrap();
        let stylus = stylus.expect("the pen is near");
        assert!(pressed && stylus.touching && (stylus.pressure - 0.75).abs() < 0.001 && stylus.twist == 270.0, "{stylus:?}, pressed: {pressed}");
        // Its side button, then it lifts and leaves.
        plays(&compositor, &mut host, &[(pen, 17, &[Arg::Uint(11), Arg::Uint(0x14b), Arg::Uint(1)]), (pen, 18, &[Arg::Uint(3)])]);
        assert!(kept.borrow().pens.last().is_some_and(|(stylus, _)| stylus.is_some_and(|stylus| stylus.barrel)));
        plays(&compositor, &mut host, &[(pen, 9, &[]), (pen, 18, &[Arg::Uint(4)])]);
        plays(&compositor, &mut host, &[(pen, 7, &[]), (pen, 18, &[Arg::Uint(5)])]);
        assert_eq!(kept.borrow().pens.last(), Some(&(None, false)), "gone");
        // An eraser with no pressure axis: half while touching.
        let rubber = 0xff00_0002;
        plays(&compositor, &mut host, &[(played.tablets, 1, &[Arg::Uint(rubber)])]);
        plays(&compositor, &mut host, &[(rubber, 0, &[Arg::Uint(0x141)]), (rubber, 4, &[])]);
        plays(&compositor, &mut host, &[(rubber, 6, &near), (rubber, 10, &place), (rubber, 8, &[Arg::Uint(12)]), (rubber, 18, &[Arg::Uint(6)])]);
        let (stylus, _) = kept.borrow().pens.last().copied().unwrap();
        assert!(stylus.is_some_and(|stylus| stylus.eraser && stylus.touching && stylus.pressure == 0.5), "{stylus:?}");
        // It is removed and destroyed.
        plays(&compositor, &mut host, &[(rubber, 5, &[])]);
        assert_eq!(host.pens.pens.len(), 1);
        let said = said(&compositor, &host.link);
        assert!(said.iter().any(|(object, opcode, _)| (*object, *opcode) == (rubber, 1)), "the pen that went was let go of");
    }

    /// A touchpad pinch over the zoom box zooms it by the scale since the last update. (With
    /// the feature `touch`.)
    #[cfg(feature = "touch")]
    #[test]
    fn a_touchpads_pinch_zooms_the_box_under_the_pointer() {
        let _alone = crate::x11::tests::alone();
        let Some((compositor, played, mut host, kept)) = fingered_shown(1) else { return eprintln!("no font: not tested") };
        assert!(played.pinch != 0 && host.pinch.id == played.pinch, "the pinch was asked for: {played:?}");
        let at = middle(kept.borrow().at[1]);
        let pointer = [Arg::Uint(1), Arg::Uint(played.surface), Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)];
        plays(&compositor, &mut host, &[(played.pointer, 0, &pointer)]);
        let spread = |by: f64| [Arg::Uint(1), Arg::Fixed(0.0), Arg::Fixed(0.0), Arg::Fixed(by), Arg::Fixed(0.0)];
        plays(&compositor, &mut host, &[(played.pinch, 0, &[Arg::Uint(2), Arg::Uint(1), Arg::Uint(played.surface), Arg::Uint(2)])]);
        plays(&compositor, &mut host, &[(played.pinch, 1, &spread(1.5))]);
        plays(&compositor, &mut host, &[(played.pinch, 1, &spread(3.0))]);
        plays(&compositor, &mut host, &[(played.pinch, 2, &[Arg::Uint(3), Arg::Uint(2), Arg::Int(0)])]);
        assert_eq!(kept.borrow().zooms, [1.5, 2.0], "a half larger, then twice as large again");
    }

    /// Hands the played compositor's end of the socket to a thread that answers the program's
    /// `sync` requests as a compositor does (the callback's `done` and its `delete_id`, in one
    /// write) until the program hangs up. Gives how the socket ended: closed, or an error (a
    /// reset, where the program left words of the compositor's unread).
    fn answering(compositor: Wire) -> std::thread::JoinHandle<std::io::Result<()>> {
        std::thread::spawn(move || {
            use std::io::Read;
            use std::os::fd::FromRawFd;
            // SAFETY: the compositor's end, open while `compositor` (kept here) lives; it is
            // read here, and closed only by `compositor`.
            let mut socket = std::mem::ManuallyDrop::new(unsafe { UnixStream::from_raw_fd(compositor.fd()) });
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            let (mut heard, mut room) = (Vec::new(), [0u8; 4096]);
            loop {
                let got = socket.read(&mut room)?;
                if got == 0 {
                    return Ok(());
                }
                heard.extend_from_slice(&room[..got]);
                while heard.len() >= 8 {
                    let word = |at: usize| u32::from_ne_bytes([heard[at], heard[at + 1], heard[at + 2], heard[at + 3]]);
                    let (object, size) = (word(0), word(4));
                    let len = (size >> 16) as usize;
                    if len < 8 || heard.len() < len {
                        break;
                    }
                    if object == 1 && size & 0xffff == 0 {
                        let callback = word(8);
                        compositor.send(callback, 0, &[Arg::Uint(0)]);
                        compositor.send(1, 1, &[Arg::Uint(callback)]);
                        compositor.flush();
                    }
                    heard.drain(..len);
                }
            }
        })
    }

    /// A program that ends hears the compositor out before it hangs up: what the compositor said
    /// last is read, so the compositor finds the socket closed, not reset with its words unread
    /// (which compositors log as an error).
    #[test]
    fn a_program_that_ends_hears_the_compositor_out_before_it_hangs_up() {
        let _alone = crate::x11::tests::alone();
        let Some((compositor, played, host)) = played(1, |ui| _ = ui.label("Ending")) else { return eprintln!("no font: not tested") };
        assert!(played.pointer != 0, "the pointer was asked for: {played:?}");
        // The pointer moves as the program ends: words it has not read when it hangs up.
        compositor.send(played.pointer, 2, &[Arg::Uint(1), Arg::Fixed(10.0), Arg::Fixed(10.0)]);
        compositor.flush();
        let ended = answering(compositor);
        drop(host);
        assert_eq!(ended.join().unwrap().map_err(|error| error.kind()), Ok(()), "the compositor finds the program hung up");
    }

    /// A compositor that does not answer holds a program that ends up for a moment only.
    #[test]
    fn a_compositor_that_does_not_answer_holds_an_ending_program_up_a_moment_only() {
        let _alone = crate::x11::tests::alone();
        let Some((_compositor, _played, host)) = played(1, |ui| _ = ui.label("Ending")) else { return eprintln!("no font: not tested") };
        let started = Instant::now();
        drop(host);
        let took = started.elapsed();
        assert!(took >= PARTING && took < PARTING + Duration::from_secs(2), "waited {took:?} for an answer that never came");
    }

    // ---- on a compositor ----
    //
    // The tests below open real windows on the compositor `WAYLAND_DISPLAY` names, driving
    // pointer and keys through whatever test input it offers (see `compositors`). They run on
    // headless compositors (KWin, Mutter, Sway, Weston, each started so), never on a user's
    // desktop: so only when asked, as the tests on an X display.

    /// A test app's windows; with the guard keeping a test alone, first so it drops last.
    pub(super) type Windows = Host<Box<dyn FnMut(&mut Ui)>>;
    pub(super) type Alone = (MutexGuard<'static, ()>, Windows);

    /// Opens a window for an app on the named compositor, on no bus, and waits until it is
    /// drawn. Nothing where no compositor answers or there is no font. `own_frame`: the window
    /// draws its own decorations, as on a compositor that will not.
    pub(super) fn shown(window: Window, own_frame: bool, app: impl FnMut(&mut Ui) + 'static) -> Option<Alone> {
        let alone = crate::x11::tests::alone();
        let (mut link, outputs) = Link::new()?;
        if own_frame {
            link.has.decorations = 0;
        }
        let mut host = Host::open(link, outputs, SystemText::new()?, Session::on(None), window, Box::new(app) as Box<dyn FnMut(&mut Ui)>)?;
        host.turn();
        assert!(waits(&mut host, |host| drawn(&host.windows[0])), "the window is drawn");
        Some((alone, host))
    }

    /// Whether a window is drawn as it is now: configured, one buffer holding all of it,
    /// nothing changed since. (Whether the compositor still holds the buffer is no sign: Mutter
    /// releases one as soon as it has copied it.)
    fn drawn(shown: &Shown) -> bool {
        shown.configured && shown.sheet.buffers.iter().any(|buffer| buffer.missing.is_empty()) && shown.sheet.changed.is_empty()
    }

    /// Takes turns for so many ms, taking in what comes.
    fn lets(host: &mut Windows, millis: u64) {
        let until = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < until {
            readable(host.link.wire.fd(), 10);
            host.turn();
        }
    }

    /// Takes turns until something holds, up to five seconds. Says whether it came to hold.
    pub(super) fn waits(host: &mut Windows, so: impl Fn(&Windows) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while !so(host) && Instant::now() < until {
            readable(host.link.wire.fd(), 10);
            host.turn();
        }
        so(host)
    }

    /// Keys, by kernel code.
    const KEY_A: u32 = 30;
    const KEY_H: u32 = 35;
    const KEY_I: u32 = 23;
    const KEY_C: u32 = 46;
    const KEY_V: u32 = 47;
    const SHIFT: u32 = 42;
    const CONTROL: u32 = 29;

    /// The compositor's test input, where it has any.
    fn hands() -> Option<Desk> {
        Desk::new().filter(Desk::has_hands)
    }

    /// A window's client corner on screen (see `compositors::found`).
    fn corner(host: &mut Windows, desk: &mut Desk, at: usize) -> Option<Point> {
        let surface = host.windows[at].sheet.surface;
        found(desk, &mut |_| {
            lets(host, 10);
            (host.pointer.over == surface).then_some(host.pointer.at)
        })
    }

    /// A box's middle.
    fn middle(of: Rect) -> Point {
        pt(of.x + of.w / 2.0, of.y + of.h / 2.0)
    }

    /// A left click at a point in a window (in units from its corner).
    fn click(host: &mut Windows, desk: &mut Desk, corner: Point, at: Point) {
        desk.point(pt(corner.x + at.x, corner.y + at.y));
        lets(host, 60);
        desk.button(true);
        lets(host, 60);
        desk.button(false);
        lets(host, 100);
    }

    /// A key pressed while others are held.
    fn press(host: &mut Windows, desk: &mut Desk, key: u32, held: &[u32]) {
        desk.press(key, held);
        lets(host, 80);
    }

    /// The compositor tests' app state.
    #[derive(Default)]
    struct App {
        pressed: u32,
        typed: String,
        /// Where its button and text box are, and the scroll box's name (under them).
        at: [Rect; 2],
        list: Id,
        fill: bool,
    }

    /// A button, a text box and a scrolling list; the window maximizes or restores when the app
    /// says.
    fn app(app: &mut App, ui: &mut Ui) {
        ui.column().pad(10.0).gap(8.0).grow().open();
        let button = ui.button("Press");
        app.pressed += button.clicked as u32;
        let typed = ui.text_box(&mut app.typed, "");
        app.at = [button.rect, typed.rect];
        let list = ui.column().grow().scroll().open();
        (0..60).for_each(|row| _ = ui.label(&format!("Row {row}")));
        ui.close();
        app.list = list.id;
        ui.close();
        if std::mem::take(&mut app.fill) {
            let fills = ui.maximized();
            ui.set_maximized(!fills);
        }
    }

    /// The test app in a window.
    fn app_shown(own_frame: bool) -> Option<(Alone, Rc<RefCell<App>>)> {
        let kept = Rc::new(RefCell::new(App::default()));
        let described = kept.clone();
        let window = Window { title: "taut on Wayland", width: 320.0, height: 240.0, ..Default::default() };
        let shown = shown(window, own_frame, move |ui| app(&mut described.borrow_mut(), ui))?;
        Some((shown, kept))
    }

    #[test]
    #[ignore]
    fn a_window_on_the_compositor_shows_what_its_app_describes_at_the_screens_scale() {
        let Some(((_alone, mut host), _)) = app_shown(false) else { return eprintln!("no compositor: not tested") };
        // The test compositor's starter states its screen's scale in `TAUT_SCREEN_SCALE`. A
        // compositor may report a fractional scale finer than the integer one only once the
        // window shows: Sway does.
        if let Some(screen) = std::env::var("TAUT_SCREEN_SCALE").ok().and_then(|scale| scale.parse::<f32>().ok()) {
            let first = host.windows[0].scale;
            assert!(first == screen || first == screen.ceil(), "the window is first drawn at its screen's scale, or the whole scale above it: {first}");
            assert!(waits(&mut host, |host| host.windows[0].scale == screen && drawn(&host.windows[0])), "the window is drawn at its screen's scale");
        }
        let shown = &host.windows[0];
        let pixels = [320, 240].map(|side| (side as f32 * shown.scale).round() as i32);
        assert_eq!((shown.size, shown.sheet.pixels), ([320, 240], pixels), "at {}", shown.scale);
        assert_eq!(shown.frame.is_some(), host.link.has.decorations == 0, "the library's own title title_bar, where the compositor draws no frame");
        let held = shown.sheet.buffers.iter().position(|buffer| buffer.missing.is_empty()).unwrap();
        let drawn = host.windows[0].sheet.buffers[held].memory.pixels().to_vec();
        assert!(drawn == Surface::draw(&mut host.ui, 64).pixels, "what was handed over is the interface, as drawn afresh");
        // Nothing happens: nothing drawn, nothing to wake for.
        lets(&mut host, 300);
        assert_eq!(host.ui.wake_in(), None);
        assert!(host.windows[0].sheet.changed.is_empty() && host.windows[0].sheet.waiting == 0);
    }

    #[test]
    #[ignore]
    fn the_pointer_and_the_keyboard_reach_the_interface() {
        let Some(((_alone, mut host), app)) = app_shown(false) else { return eprintln!("no compositor: not tested") };
        let Some(mut desk) = hands() else { return eprintln!("no fake input: not tested") };
        let corner = corner(&mut host, &mut desk, 0).expect("the pointer comes over the window");
        let [button, text] = app.borrow().at;
        click(&mut host, &mut desk, corner, middle(button));
        assert_eq!(app.borrow().pressed, 1);
        click(&mut host, &mut desk, corner, middle(text));
        // Over a text box the cursor is the text shape: by name where the compositor has names,
        // else from the cursor theme (Weston).
        assert_eq!(host.pointer.shape, Some(Cursor::Text), "the pointer's shape over a text box");
        assert_eq!(host.pointer.theme.is_some(), host.link.has.shapes == 0, "the theme's pointers only where there are no shapes by name");
        for key in [KEY_H, KEY_I] {
            press(&mut host, &mut desk, key, &[]);
        }
        press(&mut host, &mut desk, KEY_A, &[SHIFT]);
        assert_eq!(app.borrow().typed, "hiA");
        // Held, a key repeats at the compositor's rate.
        desk.key(KEY_A, true);
        lets(&mut host, 1200);
        desk.key(KEY_A, false);
        lets(&mut host, 100);
        let typed = app.borrow().typed.clone();
        assert!(typed.starts_with("hiAa") && typed.len() >= 3 + 4 && typed[3..].chars().all(|c| c == 'a'), "{typed:?}");
        // Copied and pasted within the program.
        press(&mut host, &mut desk, KEY_A, &[CONTROL]);
        press(&mut host, &mut desk, KEY_C, &[CONTROL]);
        press(&mut host, &mut desk, KEY_V, &[CONTROL]);
        press(&mut host, &mut desk, KEY_V, &[CONTROL]);
        assert_eq!(app.borrow().typed, typed.repeat(2));
        // The wheel scrolls the list under the pointer.
        let list = app.borrow().list;
        desk.point(pt(corner.x + text.x + 20.0, corner.y + text.bottom() + 50.0));
        lets(&mut host, 60);
        desk.wheel(2);
        assert!(waits(&mut host, |host| host.ui.scrolls.get(&list).is_some_and(|scrolled| scrolled[1] > 0.0)), "the list scrolled");
    }

    /// The left button goes down at a screen point, the pointer moves so far in steps, and the
    /// button comes up once the compositor took in the last step (over XTest, Weston could
    /// release the window a step short).
    fn drag(host: &mut Windows, desk: &mut Desk, from: Point, by: Point) {
        desk.point(from);
        lets(host, 80);
        desk.button(true);
        lets(host, 120);
        for step in 1..=4 {
            desk.point(pt(from.x + by.x * step as f32 / 4.0, from.y + by.y * step as f32 / 4.0));
            lets(host, 60);
        }
        desk.point(pt(from.x + by.x, from.y + by.y));
        lets(host, 200);
        desk.button(false);
        lets(host, 300);
    }

    /// The library's title bar over a window whose compositor draws no decorations (or is
    /// told it does not): drawn; dragged, it moves the window; the window's corner resizes it;
    /// a double click on it, and its middle button, maximize the window (where the compositor
    /// does that: Sway does not for a floating window) and it restores; its last button closes
    /// it.
    #[test]
    #[ignore]
    fn a_window_whose_frame_is_its_own_is_moved_sized_filled_and_closed_by_its_strip() {
        let Some(((_alone, mut host), app)) = app_shown(true) else { return eprintln!("no compositor: not tested") };
        let Some(mut desk) = hands() else { return eprintln!("no fake input: not tested") };
        assert!(host.windows[0].frame.is_some(), "the library's own title_bar is over the window");
        let title_bar = host.windows[0].frame.as_ref().is_some_and(|framed| framed.sheet.buffers.iter().any(|buffer| buffer.missing.is_empty()));
        assert!(title_bar, "and drawn");
        let mid_strip = -(decorations::HIGH as f32) / 2.0;
        let corner = corner(&mut host, &mut desk, 0).expect("the pointer comes over the window");
        // Dragged by its title bar, it follows the pointer, toward the screen's middle so it
        // stays fully on screen. (Weston places a new window anywhere.)
        let [wide, high] = desk.units();
        let by = pt(if corner.x + 160.0 > wide / 2.0 { -40.0 } else { 40.0 }, if corner.y + 120.0 > high / 2.0 { -30.0 } else { 30.0 });
        drag(&mut host, &mut desk, pt(corner.x + 60.0, corner.y + mid_strip), by);
        let moved = self::corner(&mut host, &mut desk, 0).expect("the pointer comes over the window again");
        assert_eq!(pt(moved.x - corner.x, moved.y - corner.y), by, "moved as far as the pointer, from {corner:?} to {moved:?}");
        // Dragged by its corner, it shrinks as far. (Shrinks: on a small screen a compositor
        // keeps a growing window within it.)
        drag(&mut host, &mut desk, pt(moved.x + 318.0, moved.y + 238.0), pt(-30.0, -20.0));
        // To a unit: Sway rounds its own way, depending on where the window lies.
        let near = |size: [i32; 2]| (size[0] - 290).abs() <= 1 && (size[1] - 220).abs() <= 1;
        assert!(waits(&mut host, |host| near(host.windows[0].size)), "sized by its corner at {moved:?}: {:?}", host.windows[0].size);
        let sized = host.windows[0].size;
        let wide = sized[0] as f32;
        let fills = !desk.name.starts_with("sway");
        if fills {
            // A double click on the title bar maximizes (client and title bar as tall as the
            // screen, or the room a panel leaves); the app restores it.
            let place = pt(moved.x + 60.0, moved.y + mid_strip);
            desk.point(place);
            lets(&mut host, 80);
            (0..2).for_each(|_| click(&mut host, &mut desk, pt(0.0, 0.0), place));
            assert!(waits(&mut host, |host| host.windows[0].filling), "a double press on the title_bar fills the screen");
            assert!(host.ui.maximized(), "and its app hears");
            let high = desk.units()[1].round() as i32;
            let filled = host.windows[0].size[1] + decorations::HIGH;
            assert!(filled <= high && filled > sized[1] + decorations::HIGH, "inside and title_bar, {filled} high on a screen {high} high");
            app.borrow_mut().fill = true;
            host.rerun = true;
            assert!(waits(&mut host, |host| !host.windows[0].filling));
            assert_eq!(host.windows[0].size, sized, "as big as it was");
            // Its middle button maximizes it too (once the compositor has restored it).
            lets(&mut host, 500);
            let corner = self::corner(&mut host, &mut desk, 0).expect("the pointer comes over the window again");
            click(&mut host, &mut desk, corner, pt(wide - 44.0 * 1.5, mid_strip));
            let filled = waits(&mut host, |host| host.windows[0].filling);
            if !filled {
                desk.shoot("fill-button");
            }
            assert!(filled, "the title_bar's middle button has the window fill its screen");
            app.borrow_mut().fill = true;
            host.rerun = true;
            assert!(waits(&mut host, |host| !host.windows[0].filling));
        }
        // Its last button closes it.
        let corner = self::corner(&mut host, &mut desk, 0).expect("the pointer comes over the window again");
        click(&mut host, &mut desk, corner, pt(wide - 22.0, mid_strip));
        assert!(host.done, "the first window closed, and with it the program");
    }

    /// Fingers on the compositor (KWin's fake input and Mutter's remote desktop have them;
    /// Sway's and Weston's do not): a tap presses the button; dragged up the list it scrolls
    /// with the feature `touch` (without, a mouse drag over words); and dragged by the
    /// library's title bar the window follows.
    #[test]
    #[ignore]
    fn fingers_on_the_compositor_tap_scroll_and_move_the_window() {
        let Some(((_alone, mut host), app)) = app_shown(true) else { return eprintln!("no compositor: not tested") };
        let Some(mut desk) = hands().filter(Desk::has_fingers) else { return eprintln!("no fingers to lend: not tested") };
        let corner = corner(&mut host, &mut desk, 0).expect("the pointer comes over the window");
        let [button, text] = app.borrow().at;
        let tap = pt(corner.x + middle(button).x, corner.y + middle(button).y);
        desk.touch(1, tap, TouchPhase::Down);
        lets(&mut host, 80);
        desk.touch(1, tap, TouchPhase::Up);
        lets(&mut host, 400);
        assert_eq!(app.borrow().pressed, 1, "a tap presses the button");
        // Dragged up the list, in steps.
        let (list, from) = (app.borrow().list, pt(corner.x + 40.0, corner.y + text.bottom() + 100.0));
        desk.touch(2, from, TouchPhase::Down);
        for step in 1..=8 {
            lets(&mut host, 16);
            desk.touch(2, pt(from.x, from.y - 12.0 * step as f32), TouchPhase::Move);
        }
        desk.touch(2, pt(from.x, from.y - 96.0), TouchPhase::Up);
        lets(&mut host, 400);
        let scrolled = host.ui.scrolls.get(&list).map_or(0.0, |scrolled| scrolled[1]);
        assert_eq!(scrolled > 50.0, cfg!(feature = "touch"), "the list scrolled {scrolled} (the feature: {})", cfg!(feature = "touch"));
        // Dragged by the title bar, the window follows the finger.
        let title_bar = pt(corner.x + 60.0, corner.y - decorations::HIGH as f32 / 2.0);
        desk.touch(3, title_bar, TouchPhase::Down);
        lets(&mut host, 120);
        for step in 1..=4 {
            desk.touch(3, pt(title_bar.x + 10.0 * step as f32, title_bar.y + 7.5 * step as f32), TouchPhase::Move);
            lets(&mut host, 60);
        }
        desk.touch(3, pt(title_bar.x + 40.0, title_bar.y + 30.0), TouchPhase::Up);
        lets(&mut host, 300);
        let moved = self::corner(&mut host, &mut desk, 0).expect("the pointer comes over the window again");
        assert_eq!(pt(moved.x - corner.x, moved.y - corner.y), pt(40.0, 30.0), "moved as far as the finger, from {corner:?} to {moved:?}");
    }

    /// A test input method (where the compositor allows a program to be one: Sway) is enabled
    /// as the text box is clicked, and what it commits goes into the box.
    #[test]
    #[ignore]
    fn an_input_method_types_into_the_text_box() {
        let Some(((_alone, mut host), app)) = app_shown(false) else { return eprintln!("no compositor: not tested") };
        let Some(mut desk) = hands() else { return eprintln!("no fake input: not tested") };
        let Some(method) = desk.input_method() else { return eprintln!("no input method can be the tests' own: not tested") };
        let corner = corner(&mut host, &mut desk, 0).expect("the pointer comes over the window");
        let text = app.borrow().at[1];
        click(&mut host, &mut desk, corner, middle(text));
        let (mut on, until) = (false, Instant::now() + Duration::from_secs(5));
        while !on && Instant::now() < until {
            lets(&mut host, 20);
            on = desk.method_on(method, 20);
        }
        assert!(on && host.typing.enabled, "the input method is switched on in the text box");
        desk.method_types(method, "\u{65e5}\u{672c}\u{8a9e}");
        assert!(waits(&mut host, |_| app.borrow().typed == "\u{65e5}\u{672c}\u{8a9e}"), "typed: {:?}", app.borrow().typed);
    }

    /// The two-window test app: a button in the first opens the second, which counts presses.
    #[derive(Default)]
    struct Two {
        second: crate::SecondWindow,
        pressed: u32,
        at: [Rect; 2],
    }

    /// A second window opens as its app says when a user presses a button for it (as is usual):
    /// it comes in front of the first, where the compositor would put it behind (Mutter puts an
    /// unactivated new window behind the active one), a click in it is heard there, and closed
    /// by the user, it goes.
    #[test]
    #[ignore]
    fn a_second_window_opens_and_closes_as_its_app_says() {
        let mut second = crate::SecondWindow::new(260.0, 160.0);
        second.open = false;
        let kept = Rc::new(RefCell::new(Two { second, ..Default::default() }));
        let described = kept.clone();
        let describe = move |ui: &mut Ui| {
            let mut two = described.borrow_mut();
            let opens = ui.button("Open the second");
            (two.second.open, two.at[0]) = (two.second.open || opens.clicked, opens.rect);
            if ui.window(&mut two.second, "The second") {
                let button = ui.button("In the second");
                (two.pressed, two.at[1]) = (two.pressed + button.clicked as u32, button.rect);
                ui.close();
            }
        };
        let window = Window { title: "taut first", width: 200.0, height: 100.0, ..Default::default() };
        let Some((_alone, mut host)) = shown(window, false, describe) else { return eprintln!("no compositor: not tested") };
        let Some(mut desk) = hands() else {
            // Where no user can press the button, the app opens the window itself.
            kept.borrow_mut().second.open = true;
            host.rerun = true;
            assert!(waits(&mut host, |host| host.windows.len() == 2 && drawn(&host.windows[1])), "the second window is drawn");
            return eprintln!("no fake input: only opening the second window tested");
        };
        let first = corner(&mut host, &mut desk, 0).expect("the pointer comes over the first window");
        let opens = middle(kept.borrow().at[0]);
        click(&mut host, &mut desk, first, opens);
        let drawn = waits(&mut host, |host| host.windows.len() == 2 && drawn(&host.windows[1]));
        if !drawn {
            desk.shoot("second-not-drawn");
        }
        assert!(drawn, "the second window is drawn (asked for: {}, at {first:?} + {opens:?})", kept.borrow().second.open);
        assert_eq!((host.windows[1].title.as_str(), host.windows[1].size), ("The second", [260, 160]));
        let found = self::corner(&mut host, &mut desk, 1);
        if found.is_none() {
            desk.shoot("second-not-found");
        }
        let (corner, at) = (found.expect("the pointer comes over the second window"), middle(kept.borrow().at[1]));
        click(&mut host, &mut desk, corner, at);
        assert_eq!(kept.borrow().pressed, 1, "the click is heard in the second window");
        // In front of the first: where they overlap, the second is under the pointer.
        let both = rect(first.x, first.y, 200.0, 100.0).intersect(rect(corner.x, corner.y, 260.0, 160.0));
        if !both.is_empty() {
            desk.point(middle(both));
            assert!(waits(&mut host, |host| host.pointer.over == host.windows[1].sheet.surface), "the second window lies in front");
        }
        // The user closes it: its app hears, stops describing it, and it goes.
        host.closed(1);
        assert!(waits(&mut host, |host| host.windows.len() == 1));
        assert!(!kept.borrow().second.open);
    }

    /// What the tests' other program copies: text, a picture (3×2, some pixels half
    /// transparent), and files.
    const COPIED: &str = "copied in another program: \u{17c}\u{f3}\u{142}w \u{1f44d}";
    const COPIED_PICTURE: [u32; 6] = [0xff10_2030, 0x8040_5060, 0x00ff_ffff, 0xffff_0000, 0x4000_ff00, 0xff00_00ff];
    fn copied_files() -> Vec<PathBuf> {
        vec!["/tmp/taut a b.txt".into(), "/tmp/\u{fc}ber".into()]
    }

    #[test]
    #[ignore]
    fn what_one_program_copies_another_pastes() {
        let _alone = crate::x11::tests::alone();
        let Some(_desk) = Desk::new() else { return eprintln!("no compositor: not tested") };
        if !_desk.offers("wl_seat") {
            return eprintln!("no seat, and so no clipboard: not tested");
        }
        // Sway's and Mutter's seat has a keyboard only while test input exists: `_desk`
        // provides it. The copying program, on a thread of its own: it copies one type when
        // asked, as soon as it has keyboard focus (needed to set the selection).
        let (ask, asked) = mpsc::channel::<u8>();
        let (tell, told) = mpsc::channel::<u8>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let other = std::thread::spawn(move || {
            let wanted = Rc::new(Cell::new(None::<u8>));
            let kept = wanted.clone();
            let app = move |ui: &mut Ui| {
                ui.label("The program that copies");
                let Some(kind) = kept.take() else { return };
                match kind {
                    0 => ui.copy(COPIED),
                    1 => ui.copy_picture(3, &COPIED_PICTURE),
                    _ => ui.copy_files(&copied_files()),
                }
                tell.send(kind).unwrap();
            };
            let (link, outputs) = Link::new().unwrap();
            let window = Window { title: "taut copies", width: 300.0, height: 200.0, ..Default::default() };
            let mut host = Host::open(link, outputs, SystemText::new().unwrap(), Session::on(None), window, app).unwrap();
            while !stopped.load(Ordering::Relaxed) {
                readable(host.link.wire.fd(), 20);
                if host.keys.focus == host.windows[0].sheet.surface {
                    wanted.set(wanted.get().or(asked.try_recv().ok()));
                    host.rerun |= wanted.get().is_some();
                }
                host.turn();
            }
        });
        // For each type a pasting program opens; it gets focus and with it the selection; it
        // reads it, closes, and focus returns.
        for kind in 0..3 {
            ask.send(kind).unwrap();
            assert_eq!(told.recv_timeout(Duration::from_secs(10)), Ok(kind), "the other program copied");
            let (link, outputs) = Link::new().unwrap();
            let window = Window { title: "taut pastes", width: 240.0, height: 160.0, ..Default::default() };
            let mut host = Host::open(link, outputs, SystemText::new().unwrap(), Session::on(None), window, Box::new(|ui: &mut Ui| _ = ui.label("Pastes")) as Box<dyn FnMut(&mut Ui)>).unwrap();
            assert!(waits(&mut host, |host| host.keys.focus == host.windows[0].sheet.surface && host.link.clipboard.borrow().selection != 0), "it has the keyboard, and the clipboard is offered");
            match kind {
                0 => assert_eq!(host.ui.clipboard().as_deref(), Some(COPIED)),
                1 => assert_eq!(host.ui.clipboard_picture(), Some((3, COPIED_PICTURE.to_vec()))),
                _ => assert_eq!(host.ui.clipboard_files(), copied_files()),
            }
        }
        stop.store(true, Ordering::Relaxed);
        other.join().unwrap();
    }

    /// A test program speaking the wire protocol itself: a single-colour window that starts a
    /// drag of one `kind` (`list`, a file manager's file list, or text) when pressed, and
    /// serves it until told to stop. It reports when its window shows (nothing), the pointer's
    /// position on it as it enters and moves, that it left (nothing), and when it is asked for
    /// data.
    fn drag_source_program(kind: &'static str, list: &'static str, shows: mpsc::Sender<Option<Point>>, asked: mpsc::Sender<()>, stop: Arc<AtomicBool>) {
        let wire = Wire::connect().unwrap();
        let registry = wire.make(Interface::Registry);
        wire.send(1, 1, &[Arg::Uint(registry)]);
        let offered = Link::offered(&wire).unwrap();
        let bind = |interface: &str, version: u32, of: Interface| {
            let (name, ..) = offered.iter().find(|(_, offered, _)| offered == interface).unwrap();
            wire.bind(registry, *name, interface, version, of)
        };
        let (compositor, shm, shell, seat, data) = (
            bind("wl_compositor", 4, Interface::Compositor),
            bind("wl_shm", 1, Interface::Shm),
            bind("xdg_wm_base", 1, Interface::WmBase),
            bind("wl_seat", 5, Interface::Seat),
            bind("wl_data_device_manager", 3, Interface::DataManager),
        );
        let (surface, xdg, toplevel) = (wire.make(Interface::Surface), wire.make(Interface::XdgSurface), wire.make(Interface::Toplevel));
        wire.send(compositor, 0, &[Arg::Uint(surface)]);
        wire.send(shell, 2, &[Arg::Uint(xdg), Arg::Uint(surface)]);
        wire.send(xdg, 1, &[Arg::Uint(toplevel)]);
        wire.send(toplevel, 2, &[Arg::Text("taut drags")]);
        wire.send(surface, 6, &[]);
        let pointer = wire.make(Interface::Pointer);
        wire.send(seat, 0, &[Arg::Uint(pointer)]);
        let device = wire.make(Interface::DataDevice);
        wire.send(data, 1, &[Arg::Uint(device), Arg::Uint(seat)]);
        wire.flush();
        let mut memory = Shared::new(120 * 120 * 4).unwrap();
        memory.pixels().fill(0xff20_8040);
        while !stop.load(Ordering::Relaxed) && wire.read() {
            while let Some(mut message) = wire.next() {
                match (message.of, message.opcode) {
                    (Interface::WmBase, 0) => wire.send(shell, 3, &[Arg::Uint(message.uint())]),
                    (Interface::XdgSurface, 0) => {
                        wire.send(xdg, 4, &[Arg::Uint(message.uint())]);
                        let (pool, buffer) = (wire.make(Interface::Pool), wire.make(Interface::Buffer));
                        wire.send(shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(120 * 120 * 4)]);
                        wire.send(pool, 0, &[Arg::Uint(buffer), Arg::Int(0), Arg::Int(120), Arg::Int(120), Arg::Int(480), Arg::Uint(SOLID)]);
                        wire.send(surface, 1, &[Arg::Object(buffer), Arg::Int(0), Arg::Int(0)]);
                        wire.send(surface, 2, &[Arg::Int(0), Arg::Int(0), Arg::Int(120), Arg::Int(120)]);
                        wire.send(surface, 6, &[]);
                        let _ = shows.send(None);
                    }
                    (Interface::Pointer, 1) => _ = shows.send(None),
                    (Interface::Pointer, 0 | 2) => {
                        // The position follows the serial and surface on enter, the time on
                        // motion.
                        (0..2 - message.opcode / 2).for_each(|_| _ = message.uint());
                        let _ = shows.send(Some(pt(message.fixed(), message.fixed())));
                    }
                    (Interface::Pointer, 3) => {
                        let (serial, _, _, state) = (message.uint(), message.uint(), message.uint(), message.uint());
                        if state == 1 {
                            let source = wire.make(Interface::DataSource);
                            wire.send(data, 0, &[Arg::Uint(source)]);
                            wire.send(source, 0, &[Arg::Text(kind)]);
                            wire.send(source, 2, &[Arg::Uint(1)]);
                            wire.send(device, 0, &[Arg::Object(source), Arg::Uint(surface), Arg::Nothing, Arg::Uint(serial)]);
                        }
                    }
                    (Interface::DataSource, 1) => {
                        let file = message.file.take().unwrap();
                        std::io::Write::write_all(&mut std::fs::File::from(file), list.as_bytes()).unwrap();
                        let _ = asked.send(());
                    }
                    _ => {}
                }
            }
            wire.flush();
            readable(wire.fd(), 20);
        }
    }

    /// The drop tests' app state: whether it asks about files and about text; whether a drag
    /// was over its box; and what was dropped there.
    #[derive(Default)]
    struct Taker {
        asks: [bool; 2],
        over: [bool; 2],
        files: Vec<PathBuf>,
        text: Option<String>,
    }

    /// A window with a drop box, of an app asking about files or text as `asks` says.
    fn taker(asks: [bool; 2]) -> Option<(Alone, Rc<RefCell<Taker>>)> {
        let kept = Rc::new(RefCell::new(Taker { asks, ..Default::default() }));
        let described = kept.clone();
        let describe = move |ui: &mut Ui| {
            let mut taker = described.borrow_mut();
            let target = ui.column().grow().sense().open();
            ui.label("Let it go here");
            ui.close();
            if taker.asks[0] {
                taker.over[0] |= ui.files_hovered();
                taker.files.extend(ui.files_dropped(&target).into_iter().flatten());
            }
            if taker.asks[1] {
                taker.over[1] |= ui.text_hovered();
                taker.text = taker.text.take().or(ui.text_dropped(&target));
            }
        };
        let window = Window { title: "taut takes", width: 320.0, height: 240.0, ..Default::default() };
        Some((shown(window, false, describe)?, kept))
    }

    /// Another program (see `drag_source_program`) drags one `kind` over this window's left
    /// side and drops it there: its window opens over this one's middle, is pressed there, and
    /// the pointer moves over in steps. Returns whether that program was asked for its data.
    /// (Nothing without test input.)
    fn drag_over_window(host: &mut Windows, kind: &'static str, list: &'static str) -> Option<bool> {
        let mut desk = hands()?;
        let corner = corner(host, &mut desk, 0).expect("the pointer comes over the window");
        let (stop, (says, shows), (asks, asked)) = (Arc::new(AtomicBool::new(false)), mpsc::channel(), mpsc::channel());
        let stopped = stop.clone();
        let other = std::thread::spawn(move || drag_source_program(kind, list, says, asks, stopped));
        assert_eq!(shows.recv_timeout(Duration::from_secs(10)), Ok(None), "the other program's window shows");
        lets(host, 300);
        // The other program reports where its window is, as this one does; the drop point is
        // one in this window the other does not cover.
        let mut over = None;
        let from = found(&mut desk, &mut |_| {
            lets(host, 10);
            over = shows.try_iter().last().unwrap_or(over);
            over
        });
        let from = from.expect("the pointer comes over the other program's window");
        let free = |at: &Point| !rect(from.x - 4.0, from.y - 4.0, 128.0, 128.0).contains(pt(corner.x + at.x, corner.y + at.y));
        let to = [pt(30.0, 120.0), pt(290.0, 120.0), pt(160.0, 30.0), pt(160.0, 220.0)].into_iter().find(free).unwrap();
        let (start, end) = (pt(from.x + 60.0, from.y + 60.0), pt(corner.x + to.x, corner.y + to.y));
        desk.point(start);
        lets(host, 100);
        desk.button(true);
        lets(host, 200);
        for step in 1..=6 {
            let along = step as f32 / 6.0;
            desk.point(pt(start.x + (end.x - start.x) * along, start.y + (end.y - start.y) * along));
            lets(host, 60);
        }
        desk.button(false);
        lets(host, 400);
        let was_asked = asked.recv_timeout(Duration::from_secs(1)).is_ok();
        stop.store(true, Ordering::Relaxed);
        other.join().unwrap();
        Some(was_asked)
    }

    /// Files dragged from another program drop on a box whose app asks about files: the box
    /// heard them over it, and gets the files in the list and nothing that is no file.
    #[test]
    #[ignore]
    fn files_dragged_from_another_program_are_dropped_on_a_box() {
        let Some(((_alone, mut host), kept)) = taker([true, false]) else { return eprintln!("no compositor: not tested") };
        let Some(asked) = drag_over_window(&mut host, FILES, "# carried\r\nfile:///tmp/taut%20carried.txt\r\nhttps://example.org/no\r\n") else { return eprintln!("no fake input: not tested") };
        assert!(asked && kept.borrow().over[0], "the box heard that files are over it, and the list was asked for");
        assert!(waits(&mut host, |_| !kept.borrow().files.is_empty()), "the files were let go");
        assert_eq!(kept.borrow().files, [PathBuf::from("/tmp/taut carried.txt")], "the file, and nothing that is no file");
    }

    /// Text dragged from another program drops on a box whose app asks about text.
    #[test]
    #[ignore]
    fn text_dragged_from_another_program_is_dropped_on_a_box() {
        let Some(((_alone, mut host), kept)) = taker([false, true]) else { return eprintln!("no compositor: not tested") };
        let Some(asked) = drag_over_window(&mut host, TEXT[0], "words, ünïcode 𝄞") else { return eprintln!("no fake input: not tested") };
        assert!(asked && kept.borrow().over[1], "the box heard that text is over it, and the text was asked for");
        assert!(waits(&mut host, |_| kept.borrow().text.is_some()), "the text was let go");
        assert_eq!(kept.borrow().text.as_deref(), Some("words, ünïcode 𝄞"));
    }

    /// A window whose app never asks about drops takes nothing: drags over it are refused, the
    /// source is never asked, and nothing reaches its interface. (Nor is the drop code in the
    /// program: the first request hands it over.)
    #[test]
    #[ignore]
    fn a_window_whose_app_never_asks_takes_nothing_dragged_in() {
        let Some(((_alone, mut host), _)) = taker([false, false]) else { return eprintln!("no compositor: not tested") };
        let Some(asked) = drag_over_window(&mut host, FILES, "file:///tmp/taut%20carried.txt\r\n") else { return eprintln!("no fake input: not tested") };
        assert!(!asked, "the other program was never asked for what it dragged");
        assert!(!host.ui.files_over && host.ui.files.is_empty() && DROPS.get().is_none(), "nothing of it reached the interface");
    }

    /// A tray-hidden window shows nothing, so the compositor removes it: the pointer finds
    /// nothing where it was, and its interface hears it is hidden. Asked back, it shows as at
    /// first, fully drawn, activates with the desktop's token, and is under the pointer again.
    #[test]
    fn a_window_hidden_for_the_tray_is_taken_away_and_shown_again() {
        let asks = Rc::new(RefCell::new([false; 3]));
        let seen = asks.clone();
        let app = move |ui: &mut Ui| {
            let mut asks = seen.borrow_mut();
            if std::mem::take(&mut asks[0]) {
                ui.hide_window();
            }
            if std::mem::take(&mut asks[1]) {
                ui.bring_to_front();
            }
            asks[2] = ui.window_minimized();
            ui.label("One");
        };
        let Some((_alone, mut host)) = shown(Window { title: "taut tray", width: 300.0, height: 200.0, ..Window::default() }, false, app) else {
            return eprintln!("no compositor: not tested");
        };
        let Some(mut desk) = hands() else { return eprintln!("no fake input: not tested") };
        assert!(corner(&mut host, &mut desk, 0).is_some(), "the window is under the pointer");
        // Hidden: nothing of it is under the pointer any more.
        (asks.borrow_mut()[0], host.rerun) = (true, true);
        lets(&mut host, 300);
        assert!(host.windows[0].withdrawn && !host.windows[0].configured && asks.borrow()[2]);
        assert!(corner(&mut host, &mut desk, 0).is_none(), "the pointer finds nothing of a hidden window");
        // Asked back by its app: fully drawn and there again.
        let shown_again = |host: &Windows| drawn(&host.windows[0]);
        (asks.borrow_mut()[1], host.rerun) = (true, true);
        assert!(waits(&mut host, shown_again) && !host.windows[0].withdrawn, "shown again");
        lets(&mut host, 100);
        assert!(!asks.borrow()[2], "its interface hears it is back");
        assert!(corner(&mut host, &mut desk, 0).is_some(), "the window is under the pointer again");
        // Hidden again, it comes back for a later start of the program, with that start's
        // token.
        let _starts = crate::single_instance::tests::starts_alone();
        (asks.borrow_mut()[0], host.rerun) = (true, true);
        lets(&mut host, 300);
        assert!(host.windows[0].withdrawn && asks.borrow()[2]);
        let later = crate::Relaunch { args: Vec::new(), working_dir: "/wayland test".into() };
        crate::single_instance::came(crate::single_instance::Tokens { wayland: "token-of-a-later-start".into(), x: String::new() }, later);
        host.rerun = true;
        assert!(waits(&mut host, shown_again) && !host.windows[0].withdrawn, "shown again for the later start");
        lets(&mut host, 100);
        assert!(!asks.borrow()[2] && corner(&mut host, &mut desk, 0).is_some());
        drop(std::mem::take(&mut *crate::single_instance::tests::queued()));
    }

    // ---- the gallery on the compositor ----

    use super::gallery;

    /// A gallery part's rectangle, found by what screen readers are told: the first whose name
    /// contains `name`, of `role` (any, for none).
    fn part(host: &Windows, role: Option<crate::Role>, name: &str) -> Option<Rect> {
        host.ui.visible_access_nodes().iter().find(|said| role.is_none_or(|role| said.role == role) && said.name.contains(name)).map(|said| said.rect)
    }

    /// Types a word on a US keyboard layout.
    fn types(host: &mut Windows, desk: &mut Desk, word: &str) {
        let code = |letter: char| match letter {
            'a' => 30,
            'd' => 32,
            'l' => 38,
            'n' => 49,
            'w' => 17,
            'y' => 21,
            _ => 57,
        };
        word.chars().for_each(|letter| press(host, desk, code(letter), &[]));
    }

    /// The gallery on the compositor: it opens (with the library's title bar where the
    /// compositor draws no decorations), its button is pressed twice, a word typed into its
    /// filter, its View menu and a dropdown opened; each captured as a screenshot into the
    /// folder `TAUT_SHOTS` names. And where the compositor shows a window as drawn (integer
    /// scale), the window's part of the screenshot is its own pixels.
    #[test]
    #[ignore]
    fn the_gallery_is_used_on_the_compositor() {
        let _alone = crate::x11::tests::alone();
        let Some(mut desk) = Desk::new() else { return eprintln!("no compositor: not tested") };
        let (link, outputs) = Link::new().unwrap();
        // As large as the screen allows, with its title bar.
        let [wide, high] = desk.units().map(|side| (side - 60.0).min(900.0));
        let window = Window { title: "taut gallery", width: wide, height: high - decorations::HIGH as f32, min_width: 280.0, min_height: 220.0, ..Default::default() };
        let mut kept = gallery::app();
        let app = Box::new(move |ui: &mut Ui| gallery::describe(&mut kept, ui)) as Box<dyn FnMut(&mut Ui)>;
        let mut host = Host::open(link, outputs, SystemText::new().unwrap(), Session::on(None), window, app).unwrap();
        // Parts are found by what screen readers are told: as if one listened, once the host
        // found none does.
        host.turn();
        host.ui.set_screen_reader(true);
        host.rerun = true;
        let shows = |host: &Windows| host.windows[0].sheet.buffers.iter().any(|buffer| buffer.missing.is_empty()) && part(host, None, "Count one").is_some();
        assert!(waits(&mut host, shows), "the gallery is drawn");
        lets(&mut host, 300);
        if !desk.has_hands() {
            desk.shoot("gallery-opened").expect("a picture of the screen");
            return eprintln!("no fake input: only opening the gallery tested");
        }
        let corner = corner(&mut host, &mut desk, 0).expect("the pointer comes over the gallery");
        let scale = host.windows[0].scale;
        shown_as_drawn(&mut host, &mut desk, "gallery-opened", corner);
        // Its button, twice.
        let count = part(&host, Some(crate::Role::Button), "Count one").expect("the button");
        (0..2).for_each(|_| click(&mut host, &mut desk, corner, middle(count)));
        assert!(waits(&mut host, |host| part(host, None, "2 presses counted").is_some()), "the button was pressed twice");
        // A word in its filter.
        let filter = part(&host, Some(crate::Role::TextBox), "Filter").expect("the filter");
        click(&mut host, &mut desk, corner, middle(filter));
        types(&mut host, &mut desk, "wayland");
        let typed = host.ui.visible_access_nodes().iter().any(|said| said.role == crate::Role::TextBox && said.value == "wayland");
        assert!(typed, "the word was typed into the filter");
        desk.shoot("gallery-typed");
        // Its View menu and its dropdown: each opens where it belongs, under its opener.
        let view = part(&host, None, "View").expect("the View menu");
        click(&mut host, &mut desk, corner, middle(view));
        assert!(waits(&mut host, |host| part(host, Some(crate::Role::Menu), "").is_some()), "the menu opened");
        let menu = part(&host, Some(crate::Role::Menu), "").unwrap();
        assert!((menu.y - view.bottom()).abs() < 8.0 && (menu.x - view.x).abs() < 8.0, "the menu opens under its button: {menu:?} under {view:?}");
        desk.shoot("gallery-menu");
        press(&mut host, &mut desk, 1, &[]);
        let select = part(&host, Some(crate::Role::ComboBox), "").expect("the list to choose from");
        click(&mut host, &mut desk, corner, middle(select));
        lets(&mut host, 300);
        shown_as_drawn(&mut host, &mut desk, "gallery-list", corner);
        press(&mut host, &mut desk, 1, &[]);
        let _ = scale;
    }

    /// Takes a screenshot and writes it under this name. At an integer scale, the window's part
    /// is its own pixels as last drawn: retaken until it is, for up to three seconds (a
    /// compositor shows what it was given at its next composite: Mutter shows a new window's
    /// client a while after its title bar). Where the compositor gives no screenshots (KWin
    /// without a backend), nothing.
    fn shown_as_drawn(host: &mut Windows, desk: &mut Desk, name: &str, corner: Point) {
        let (scale, until) = (host.windows[0].scale, Instant::now() + Duration::from_secs(3));
        loop {
            let Some(picture) = desk.picture() else { return eprintln!("no picture of the screen: not compared") };
            let differ = if scale.fract() == 0.0 { same(host, &picture, corner, scale) } else { 0 };
            if differ == 0 || Instant::now() > until {
                let shot = desk.shoot(name);
                assert!(differ == 0 || shot.is_some_and(|shot| same(host, &shot, corner, scale) == 0), "{differ} pixels differ from what the window drew");
                return;
            }
            lets(host, 300);
        }
    }

    /// How many pixels of the window's part of a screenshot differ from its own last-drawn
    /// pixels (its corner, in units, at that screen place): none but a few where the pointer
    /// lies over the window (some compositors include it).
    fn same(host: &mut Windows, (width, pixels): &(usize, Vec<u32>), corner: Point, scale: f32) -> usize {
        let sheet = &mut host.windows[0].sheet;
        let Some(buffer) = sheet.buffers.iter_mut().find(|buffer| buffer.missing.is_empty()) else { return 0 };
        let ([wide, high], drawn) = (buffer.size.map(|side| side as usize), buffer.memory.pixels().to_vec());
        let (left, top) = ((corner.x * scale).round() as usize, (corner.y * scale).round() as usize);
        let inside = (0..high).flat_map(|row| (0..wide).map(move |column| (row, column)));
        let on_screen = inside.filter(|(row, column)| top + row < pixels.len() / width && left + column < *width);
        let differ = on_screen.filter(|(row, column)| pixels[(top + row) * width + left + column] & 0xff_ffff != drawn[row * wide + column] & 0xff_ffff).count();
        if differ < 32 * 32 { 0 } else { differ }
    }

    /// A program that asks for a copy and stops reading before it is all written does not end
    /// this one, even where a write to such a pipe would (as in a C host program). (It did:
    /// SIGPIPE, signal 13.)
    #[test]
    fn a_program_that_stops_reading_a_copy_does_not_end_this_one() {
        crate::wayland_wire::tests::by_itself("wayland::tests::a_program_that_stops_reading_a_copy_alone");
    }

    #[test]
    #[ignore = "run by the test before it, in a program of its own"]
    fn a_program_that_stops_reading_a_copy_alone() {
        if std::env::var_os(crate::wayland_wire::tests::ALONE).is_none() {
            return;
        }
        crate::wayland_wire::tests::as_in_c();
        let (_compositor, link) = offering(&SHOWS_WINDOWS);
        let (link, _) = link.unwrap();
        link.set(vec![(TEXT[0], vec![b'x'; 1 << 20].into())]);
        let (out, into) = crate::wayland_wire::pipe().unwrap();
        drop(out);
        link.hand_over(TEXT[0], Some(into));
        // The copy is written by its own thread at once: it has long failed by then.
        std::thread::sleep(Duration::from_secs(1));
    }
}

// The gallery example, used on a compositor by the tests above.
#[cfg(test)]
#[allow(dead_code, unused_attributes, unused_imports, clippy::all)]
#[path = "../examples/gallery.rs"]
mod gallery;

// Windows shown through Vulkan (feature `gpu`): this host's half of what `gpu_x11` does for X.
#[cfg(feature = "gpu")]
#[path = "gpu_wayland.rs"]
mod gpu_wayland;

// Windows lit on an HDR screen handed over in PQ (feature `hdr`).
#[cfg(feature = "hdr")]
#[path = "hdr_wayland.rs"]
mod hdr_wayland;
