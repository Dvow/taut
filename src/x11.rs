//! X11 windows (on Wayland desktops, through XWayland), with text drawn by FreeType and
//! Fontconfig.
//!
//! One loop on one X connection runs all of a program's windows. Whatever window something
//! happens in, the first window's interface runs and describes the others too; then each window
//! redraws where it changed, through one shared strip of pixels. The loop also waits on the
//! session bus, where the desktop portal is asked for its file dialog and dark-mode preference
//! (`portal`; else read from the desktop's theme), and on the screen readers' buses (`atspi`).
//!
//! A window's place, for its app, is the top left of its frame on screen in pixels: of the
//! window manager's frame, as far as the manager states its extents (else, and with no window
//! manager, the window's own corner). Windows open at, and report, places by that one rule, so
//! a place kept one day is where the window opens the next.
//!
//! A frameless window asks the window manager for no decorations; its app's move strip and
//! edges are handled here (layout in `frameless`): a press hands the window to the window
//! manager to move or resize (`_NET_WM_MOVERESIZE`), and with no manager, or one that refuses,
//! the window moves and resizes itself by the pointer. A see-through window uses a 32-bit ARGB
//! visual, so a compositing manager shows what is behind through its alpha.
//!
//! Nothing is linked at build time: X11, Fontconfig, FreeType (and Xrandr where present) are
//! opened by name when the window is, so the program builds without their headers and starts on
//! any desktop that has them.
use crate::atspi::{Bus, Message, Readers};
use crate::canvas::Canvas;
use crate::color::Color;
use crate::drag_drop::{DragData, DropEffect};
use crate::dynamic_library::library;
use crate::event::{Button, Cursor, Event, Key, Mods};
use crate::file_dialog::FileDialogRequest;
use crate::frameless::HitTest;
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::id::Id;
use crate::platform::{Clipboard, PicturesAndFiles, Window, WindowPlacement};
use crate::portal::Portal;
use crate::preferences::{Preferences, of_x_settings};
use crate::screens::{OpenAt, Screen, window_origin};
use crate::second_window::{WindowKind, WindowOpening};
use crate::text::{Face, Font, Metrics, TextSystem};
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString, OsString, c_char, c_int, c_long, c_short, c_uchar, c_uint, c_ulong, c_ushort, c_void};
use std::fmt::Write;
use std::mem::zeroed;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    pub(crate) fn pipe(ends: *mut c_int) -> c_int;
    pub(crate) fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int;
    pub(crate) fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}
#[repr(C)]
pub(crate) struct PollFd {
    pub(crate) fd: c_int,
    pub(crate) events: c_short,
    pub(crate) revents: c_short,
}

type Display = *mut c_void;
type XWindow = c_ulong;
type Atom = c_ulong;

library!(Xlib, ["libX11.so.6", "libX11.so"], {
    fn XOpenDisplay(*const c_char) -> Display;
    fn XCloseDisplay(Display) -> c_int;
    fn XSetErrorHandler(Handler) -> Handler;
    fn XDefaultScreen(Display) -> c_int;
    fn XRootWindow(Display, c_int) -> XWindow;
    fn XDefaultVisual(Display, c_int) -> *mut c_void;
    fn XDefaultDepth(Display, c_int) -> c_int;
    fn XDefaultGC(Display, c_int) -> *mut c_void;
    fn XCreateWindow(Display, XWindow, c_int, c_int, c_uint, c_uint, c_uint, c_int, c_uint, *mut c_void, c_ulong, *mut Attributes) -> XWindow;
    fn XMatchVisualInfo(Display, c_int, c_int, c_int, *mut VisualInfo) -> c_int;
    fn XCreateColormap(Display, XWindow, *mut c_void, c_int) -> c_ulong;
    fn XCreateGC(Display, XWindow, c_ulong, *mut c_void) -> *mut c_void;
    fn XMoveResizeWindow(Display, XWindow, c_int, c_int, c_uint, c_uint) -> c_int;
    fn XRaiseWindow(Display, XWindow) -> c_int;
    fn XUngrabPointer(Display, c_ulong) -> c_int;
    fn XSelectInput(Display, XWindow, c_long) -> c_int;
    fn XMapWindow(Display, XWindow) -> c_int;
    fn XDestroyWindow(Display, XWindow) -> c_int;
    fn XStoreName(Display, XWindow, *const c_char) -> c_int;
    fn XInternAtom(Display, *const c_char, c_int) -> Atom;
    fn XSetWMProtocols(Display, XWindow, *mut Atom, c_int) -> c_int;
    fn XChangeProperty(Display, XWindow, Atom, Atom, c_int, c_int, *const c_uchar, c_int) -> c_int;
    fn XSetWMNormalHints(Display, XWindow, *mut SizeHints) -> ();
    fn XNextEvent(Display, *mut XEvent) -> c_int;
    fn XPending(Display) -> c_int;
    fn XFilterEvent(*mut XEvent, XWindow) -> c_int;
    fn XCreateImage(Display, *mut c_void, c_uint, c_int, c_int, *mut c_char, c_uint, c_uint, c_int, c_int) -> *mut c_void;
    fn XPutImage(Display, XWindow, *mut c_void, *mut c_void, c_int, c_int, c_int, c_int, c_uint, c_uint) -> c_int;
    fn XFlush(Display) -> c_int;
    fn XFree(*mut c_void) -> c_int;
    fn XConnectionNumber(Display) -> c_int;
    fn XMaxRequestSize(Display) -> c_long;
    fn XResourceManagerString(Display) -> *const c_char;
    fn XCreateFontCursor(Display, c_uint) -> c_ulong;
    fn XDefineCursor(Display, XWindow, c_ulong) -> c_int;
    fn XFreeCursor(Display, c_ulong) -> c_int;
    fn XSetLocaleModifiers(*const c_char) -> *mut c_char;
    fn XOpenIM(Display, *mut c_void, *mut c_char, *mut c_char) -> *mut c_void;
    fn XCloseIM(*mut c_void) -> c_int;
    fn XCreateIC(*mut c_void, ...) -> *mut c_void;
    fn XDestroyIC(*mut c_void) -> ();
    fn XSetICFocus(*mut c_void) -> ();
    fn XUnsetICFocus(*mut c_void) -> ();
    fn XVaCreateNestedList(c_int, ...) -> *mut c_void;
    fn XSetICValues(*mut c_void, ...) -> *mut c_char;
    fn Xutf8LookupString(*mut c_void, *mut XEvent, *mut c_char, c_int, *mut c_ulong, *mut c_int) -> c_int;
    fn XLookupKeysym(*mut XEvent, c_int) -> c_ulong;
    fn XkbSetDetectableAutoRepeat(Display, c_int, *mut c_int) -> c_int;
    fn XSetSelectionOwner(Display, Atom, XWindow, c_ulong) -> c_int;
    fn XGetSelectionOwner(Display, Atom) -> XWindow;
    fn XConvertSelection(Display, Atom, Atom, Atom, XWindow, c_ulong) -> c_int;
    fn XCheckIfEvent(Display, *mut XEvent, unsafe extern "C" fn(Display, *mut XEvent, *mut c_char) -> c_int, *mut c_char) -> c_int;
    fn XGetWindowProperty(Display, XWindow, Atom, c_long, c_long, c_int, Atom, *mut Atom, *mut c_int, *mut c_ulong, *mut c_ulong, *mut *mut c_uchar) -> c_int;
    fn XSendEvent(Display, XWindow, c_int, c_long, *mut XEvent) -> c_int;
    fn XTranslateCoordinates(Display, XWindow, XWindow, c_int, c_int, *mut c_int, *mut c_int, *mut XWindow) -> c_int;
    fn XGetGeometry(Display, XWindow, *mut XWindow, *mut c_int, *mut c_int, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_int;
    fn XGrabKeyboard(Display, XWindow, c_int, c_int, c_int, c_ulong) -> c_int;
    fn XUngrabKeyboard(Display, c_ulong) -> c_int;
});

// RandR 1.5's monitors, where the server has it.
library!(Xrandr, ["libXrandr.so.2", "libXrandr.so"], {
    fn XRRQueryExtension(Display, *mut c_int, *mut c_int) -> c_int;
    fn XRRSelectInput(Display, XWindow, c_int) -> ();
    fn XRRGetMonitors(Display, XWindow, c_int, *mut c_int) -> *mut Monitor;
    fn XRRFreeMonitors(*mut Monitor) -> ();
});

/// `XRRMonitorInfo`.
#[repr(C)]
struct Monitor {
    name: Atom,
    main: c_int,
    made: c_int,
    outputs: c_int,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    width_mm: c_int,
    height_mm: c_int,
    shown_by: *mut c_ulong,
}

library!(FreeType, ["libfreetype.so.6", "libfreetype.so"], {
    fn FT_Init_FreeType(*mut *mut c_void) -> c_int;
    fn FT_Done_FreeType(*mut c_void) -> c_int;
    fn FT_New_Face(*mut c_void, *const c_char, c_long, *mut *mut FaceRec) -> c_int;
    fn FT_Set_Pixel_Sizes(*mut FaceRec, c_uint, c_uint) -> c_int;
    fn FT_Get_Char_Index(*mut FaceRec, c_ulong) -> c_uint;
    fn FT_Load_Char(*mut FaceRec, c_ulong, c_int) -> c_int;
});

library!(Fontconfig, ["libfontconfig.so.1", "libfontconfig.so"], {
    fn FcInitLoadConfigAndFonts() -> *mut c_void;
    fn FcNameParse(*const c_uchar) -> *mut c_void;
    fn FcConfigSubstitute(*mut c_void, *mut c_void, c_int) -> c_int;
    fn FcDefaultSubstitute(*mut c_void) -> ();
    fn FcFontMatch(*mut c_void, *mut c_void, *mut c_int) -> *mut c_void;
    fn FcFontSort(*mut c_void, *mut c_void, c_int, *mut *mut c_void, *mut c_int) -> *mut FontSet;
    fn FcFontSetDestroy(*mut FontSet) -> ();
    fn FcPatternGetString(*mut c_void, *const c_char, c_int, *mut *const c_uchar) -> c_int;
    fn FcPatternGetInteger(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int;
    fn FcPatternGetBool(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int;
    fn FcPatternGetCharSet(*mut c_void, *const c_char, c_int, *mut *mut c_void) -> c_int;
    fn FcCharSetFirstPage(*mut c_void, *mut u32, *mut u32) -> u32;
    fn FcCharSetNextPage(*mut c_void, *mut u32, *mut u32) -> u32;
    fn FcPatternDestroy(*mut c_void) -> ();
    fn FcConfigDestroy(*mut c_void) -> ();
    fn FcFontList(*mut c_void, *mut c_void, *mut c_void) -> *mut FontSet;
    fn FcObjectSetCreate() -> *mut c_void;
    fn FcObjectSetAdd(*mut c_void, *const c_char) -> c_int;
    fn FcObjectSetDestroy(*mut c_void) -> ();
});

// The fields of FreeType's and Fontconfig's records read here, laid out as their headers have
// them.
#[repr(C)]
struct FaceRec {
    counts: [c_long; 5],
    names: [*mut c_char; 2],
    fixed_sizes: c_int,
    available_sizes: *mut c_void,
    charmap_count: c_int,
    charmaps: *mut c_void,
    generic: [*mut c_void; 2],
    bbox: [c_long; 4],
    units_per_em: c_ushort,
    design: [c_short; 7],
    glyph: *mut GlyphSlot,
    size: *mut SizeRec,
}
#[repr(C)]
struct GlyphSlot {
    owners: [*mut c_void; 3],
    index: c_uint,
    generic: [*mut c_void; 2],
    metrics: [c_long; 8],
    linear_advance: [c_long; 2],
    advance: [c_long; 2],
    format: c_int,
    bitmap: Bitmap,
    left: c_int,
    top: c_int,
}
#[repr(C)]
struct Bitmap {
    rows: c_uint,
    width: c_uint,
    pitch: c_int,
    buffer: *mut u8,
    grays: c_ushort,
    pixel_mode: c_uchar,
    palette_mode: c_uchar,
    palette: *mut c_void,
}
#[repr(C)]
struct SizeRec {
    face: *mut c_void,
    generic: [*mut c_void; 2],
    ppem: [c_ushort; 2],
    scale: [c_long; 2],
    ascender: c_long,
    descender: c_long,
    height: c_long,
    max_advance: c_long,
}
/// `FcFontSet`.
#[repr(C)]
struct FontSet {
    count: c_int,
    room: c_int,
    fonts: *mut *mut c_void,
}

/// Fontconfig's name for a face: the desktop's choice for "sans" and "monospace", or the family
/// an app named, in its style (see `system_fonts`).
fn name_of(face: Face) -> String {
    match face {
        Face::Regular => "sans".into(),
        Face::Bold => "sans:bold".into(),
        Face::Mono => "monospace".into(),
        Face::Italic => "sans:italic".into(),
        Face::Named(_) => crate::system_fonts::pattern(face),
    }
}

/// The scalable fonts Fontconfig knows, each by its family's first name and its style: all, or
/// one family's (named as Fontconfig reads it).
pub(crate) fn listed(family: Option<&str>) -> Vec<(String, crate::system_fonts::Style)> {
    Catalog::open().map(|catalog| catalog.listed(family.unwrap_or_default())).unwrap_or_default()
}

/// The desktop's emoji font file, as Fontconfig has it (including a person's own choice).
/// Looked up while Fontconfig is open anyway for the system's text, and kept only where found:
/// opening Fontconfig again would cost a megabyte or more the program never gets back. (Nothing
/// where system text is never drawn; on a machine with no emoji font, some other font.)
pub(crate) fn emoji_file() -> Option<PathBuf> {
    EMOJI_FILE.get().cloned().flatten()
}
static EMOJI_FILE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Fontconfig with its full font list loaded: what knows which file a font is in. The list
/// costs a megabyte or two and some tens of ms, so this is opened, asked, and closed.
struct Catalog {
    fc: Fontconfig,
    config: *mut c_void,
}

impl Catalog {
    fn open() -> Option<Self> {
        let fc = Fontconfig::open()?;
        // SAFETY: Fontconfig's documented entry call.
        let config = unsafe { (fc.FcInitLoadConfigAndFonts)() };
        if config.is_null() {
            return None;
        }
        Some(Catalog { fc, config })
    }

    /// The pattern Fontconfig matches a font name against, with the desktop's settings
    /// substituted. (The caller destroys it.)
    fn pattern(&self, name: &str) -> *mut c_void {
        let name = CString::new(name).unwrap_or_default();
        // SAFETY: the name ends in a zero, and these are Fontconfig's documented calls for
        // building a pattern.
        unsafe {
            let pattern = (self.fc.FcNameParse)(name.as_ptr().cast());
            (self.fc.FcConfigSubstitute)(self.config, pattern, 0);
            (self.fc.FcDefaultSubstitute)(pattern);
            pattern
        }
    }

    /// Where one of Fontconfig's fonts is: its file and index in it.
    ///
    /// # Safety
    /// `font` is a font Fontconfig handed out and has not destroyed.
    unsafe fn place(&self, font: *mut c_void) -> Option<(CString, c_long)> {
        let (mut file, mut index) = (null(), 0);
        // SAFETY: Fontconfig fills these in; the name it hands out ends in a zero and is
        // copied.
        unsafe {
            if (self.fc.FcPatternGetString)(font, c"file".as_ptr(), 0, &mut file) != 0 {
                return None;
            }
            (self.fc.FcPatternGetInteger)(font, c"index".as_ptr(), 0, &mut index);
            Some((CStr::from_ptr(file.cast()).to_owned(), index as c_long))
        }
    }

    /// The file and index of the desktop's font called `name`.
    fn file_of(&self, name: &str) -> Option<(CString, c_long)> {
        let (wanted, mut result) = (self.pattern(name), 0);
        // SAFETY: Fontconfig's documented font-match calls; what it hands out is copied before
        // it is freed.
        unsafe {
            let found = (self.fc.FcFontMatch)(self.config, wanted, &mut result);
            (self.fc.FcPatternDestroy)(wanted);
            if found.is_null() {
                return None;
            }
            let place = self.place(found);
            (self.fc.FcPatternDestroy)(found);
            place
        }
    }

    /// The fallback fonts for the font `name` where it lacks a character: those the desktop
    /// would turn to, in order, each with its coverage. Only fonts covering something the
    /// earlier ones lack are kept.
    fn spares(&self, name: &str) -> Spares {
        // Bitmap and colour fonts are asked for last: neither is drawn here, so neither may
        // shadow one that is.
        let wanted = self.pattern(&format!("{name}:scalable=true:color=false"));
        let (mut result, mut spares) = (0, Spares::default());
        // SAFETY: Fontconfig's set holds `count` fonts until destroyed.
        unsafe {
            let sorted = (self.fc.FcFontSort)(self.config, wanted, 1, null_mut(), &mut result);
            (self.fc.FcPatternDestroy)(wanted);
            if sorted.is_null() {
                return spares;
            }
            for at in 0..(*sorted).count as usize {
                self.list(*(*sorted).fonts.add(at), &mut spares);
            }
            (self.fc.FcFontSetDestroy)(sorted);
        }
        // Fonts with the same page keep their listing order, which is their fallback order. A
        // heap sorts them with next to no code.
        spares.pages = std::collections::BinaryHeap::from(std::mem::take(&mut spares.pages)).into_sorted_vec();
        spares.pages.shrink_to_fit();
        spares
    }

    /// The scalable fonts matching a pattern, each by family and style.
    fn listed(&self, family: &str) -> Vec<(String, crate::system_fonts::Style)> {
        let (wanted, mut list) = (CString::new(format!("{family}:scalable=true")).unwrap_or_default(), Vec::new());
        // SAFETY: Fontconfig's documented listing calls; what it hands out is read and copied
        // before it is destroyed.
        unsafe {
            let (pattern, asked) = ((self.fc.FcNameParse)(wanted.as_ptr().cast()), (self.fc.FcObjectSetCreate)());
            [c"family", c"weight", c"slant"].iter().for_each(|name| _ = (self.fc.FcObjectSetAdd)(asked, name.as_ptr()));
            let fonts = (self.fc.FcFontList)(self.config, pattern, asked);
            (self.fc.FcObjectSetDestroy)(asked);
            (self.fc.FcPatternDestroy)(pattern);
            if fonts.is_null() {
                return list;
            }
            list.extend((0..(*fonts).count as usize).filter_map(|at| self.style_of(*(*fonts).fonts.add(at))));
            (self.fc.FcFontSetDestroy)(fonts);
        }
        list
    }

    /// A Fontconfig font's family and style.
    ///
    /// # Safety
    /// `font` is a font Fontconfig handed out and has not destroyed.
    unsafe fn style_of(&self, font: *mut c_void) -> Option<(String, crate::system_fonts::Style)> {
        let (mut family, mut weight, mut slant) = (null(), 80, 0);
        // SAFETY: Fontconfig fills these in; the name it hands out ends in a zero and is
        // copied.
        unsafe {
            if (self.fc.FcPatternGetString)(font, c"family".as_ptr(), 0, &mut family) != 0 {
                return None;
            }
            (self.fc.FcPatternGetInteger)(font, c"weight".as_ptr(), 0, &mut weight);
            (self.fc.FcPatternGetInteger)(font, c"slant".as_ptr(), 0, &mut slant);
            let style = crate::system_fonts::Style { weight: crate::system_fonts::weight_of(weight), italic: slant != 0 };
            Some((CStr::from_ptr(family.cast()).to_string_lossy().into_owned(), style))
        }
    }

    /// Adds a Fontconfig font to the fallbacks: its place and its characters. One without
    /// outlines is skipped: only outlines draw at any size.
    ///
    /// # Safety
    /// `font` is a font Fontconfig handed out and has not destroyed.
    unsafe fn list(&self, font: *mut c_void, spares: &mut Spares) {
        let (mut outlines, mut has, mut bits, mut next) = (0, null_mut(), [0; 8], 0);
        // SAFETY: Fontconfig fills these in: the charset 256 characters at a time, until it
        // says that was all (all bits set).
        unsafe {
            (self.fc.FcPatternGetBool)(font, c"outline".as_ptr(), 0, &mut outlines);
            (self.fc.FcPatternGetCharSet)(font, c"charset".as_ptr(), 0, &mut has);
            let Some(place) = self.place(font).filter(|_| outlines == 1 && !has.is_null()) else { return };
            let mut first = (self.fc.FcCharSetFirstPage)(has, bits.as_mut_ptr(), &mut next);
            while first != u32::MAX {
                spares.pages.push(Page { first, font: spares.files.len() as u32, bits });
                first = (self.fc.FcCharSetNextPage)(has, bits.as_mut_ptr(), &mut next);
            }
            spares.files.push(place);
        }
    }
}

impl Drop for Catalog {
    fn drop(&mut self) {
        // SAFETY: the font list loaded for this is freed; nothing it handed out is kept.
        // (`FcFini` is never called: a host program may be using Fontconfig too.)
        unsafe { (self.fc.FcConfigDestroy)(self.config) };
    }
}

/// The font file the system draws a face's character with: the face's own font if it has it,
/// else the first fallback that does, as [`SystemText`] draws it. (Opens and closes
/// Fontconfig.) Nothing where no font has it.
pub(crate) fn file_for(face: Face, letter: char) -> Option<PathBuf> {
    let spares = Catalog::open()?.spares(&name_of(face));
    let (file, _) = spares.with(letter).next()?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(file.as_bytes())))
}

/// 256 consecutive characters and which of them a font has. (Ordered by first character, then
/// font.)
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Page {
    /// The first of the characters.
    first: u32,
    /// Its index among the fallbacks.
    font: u32,
    /// A bit for each character the font has.
    bits: [u32; 8],
}

/// The fallback fonts for a face, preferred first. (180 fonts of every script take some 50 KB
/// this way.)
#[derive(Default)]
struct Spares {
    /// Each one's file and index in it.
    files: Vec<(CString, c_long)>,
    /// Their coverage: by character, then by font.
    pages: Vec<Page>,
}

impl Spares {
    /// The fonts with a character, preferred first.
    fn with(&self, c: char) -> impl Iterator<Item = &(CString, c_long)> {
        let (first, bit) = (c as u32 & !0xff, c as usize & 0xff);
        let from = self.pages.partition_point(|page| page.first < first);
        let having = self.pages[from..].iter().take_while(move |page| page.first == first).filter(move |page| page.bits[bit / 32] >> (bit % 32) & 1 != 0);
        having.map(|page| &self.files[page.font as usize])
    }
}

/// A font file FreeType has open, and its current pixel size.
struct Opened {
    file: CString,
    index: c_long,
    face: *mut FaceRec,
    px: u32,
}

/// A glyph rasterized once at one size.
struct Glyph {
    width: usize,
    left: i32,
    top: i32,
    advance: f32,
    cover: Vec<u8>,
}

/// FreeType, its open fonts, and each face's fallbacks.
struct Fonts {
    ft: FreeType,
    library: *mut c_void,
    /// Fonts opened so far. A font is read when something is first drawn in it.
    open: Vec<Opened>,
    /// Each face's fallbacks, asked of Fontconfig the first time the face's font lacks a
    /// character.
    spares: HashMap<Face, Spares>,
}

impl Fonts {
    /// Opens the font in a file if not yet open. Says which open font it is; nothing if
    /// FreeType cannot read it.
    fn read(&mut self, file: &CStr, index: c_long) -> Option<usize> {
        if let Some(at) = self.open.iter().position(|font| font.file.as_c_str() == file && font.index == index) {
            return Some(at);
        }
        let mut face = null_mut();
        // SAFETY: the file name ends in a zero; FreeType hands back the face it opened.
        if unsafe { (self.ft.FT_New_Face)(self.library, file.as_ptr(), index, &mut face) } != 0 {
            return None;
        }
        self.open.push(Opened { file: file.to_owned(), index, face, px: 0 });
        Some(self.open.len() - 1)
    }

    /// An open font, set to a pixel size.
    fn sized(&mut self, at: usize, px: u32) -> *mut FaceRec {
        let font = &mut self.open[at];
        if std::mem::replace(&mut font.px, px) != px {
            // SAFETY: the face is open, and stays so while this lives.
            unsafe { (self.ft.FT_Set_Pixel_Sizes)(font.face, 0, px) };
        }
        font.face
    }

    /// Whether an open font has a character.
    fn has(&self, at: usize, c: char) -> bool {
        // SAFETY: the face is open, and stays so while this lives.
        unsafe { (self.ft.FT_Get_Char_Index)(self.open[at].face, c as c_ulong) != 0 }
    }

    /// The font drawing a character `face`'s font lacks: the first fallback that has it.
    /// Nothing where none does.
    fn stand_in(&mut self, face: Face, c: char) -> Option<usize> {
        // Fontconfig is opened again to list them, then closed.
        let listed = || Catalog::open().map(|catalog| catalog.spares(&name_of(face))).unwrap_or_default();
        let spares = self.spares.remove(&face).unwrap_or_else(listed);
        let found = spares.with(c).find_map(|(file, index)| self.read(file, *index).filter(|at| self.has(*at, c)));
        self.spares.insert(face, spares);
        found
    }

    /// A glyph of `face` in font `at`, `px` pixels high: drawn by FreeType from that font, or
    /// the fallback where it lacks the character. (Where none has it, the font's .notdef,
    /// usually a box. Line ends, tabs and the like are no font's characters and not looked
    /// for.) Or, with `form` not 0, that glyph of the font: the character's form in a language.
    fn draw(&mut self, face: Face, at: usize, px: u32, c: char, #[cfg(feature = "shaping")] form: u16) -> Glyph {
        let from = if self.has(at, c) || c.is_control() { at } else { self.stand_in(face, c).unwrap_or(at) };
        if cfg!(feature = "fallback-fonts") && from != at {
            return self.draw_fallback(at, from, px, c);
        }
        let font = self.sized(from, px);
        // SAFETY: FreeType renders into the face's own glyph slot, read here before anything
        // else is drawn. `FT_LOAD_RENDER` (4) with `FT_LOAD_NO_BITMAP` (8): rows of one byte
        // per pixel.
        unsafe {
            #[cfg(feature = "shaping")]
            let failed = if form != 0 { self.load_glyph(font, form) } else { (self.ft.FT_Load_Char)(font, c as c_ulong, 4 | 8) != 0 };
            #[cfg(not(feature = "shaping"))]
            let failed = (self.ft.FT_Load_Char)(font, c as c_ulong, 4 | 8) != 0;
            rendered(font, failed, px)
        }
    }

    /// A character's glyph from fallback font `from` for a face drawn in font `at` (feature
    /// `fallback-fonts`): at a size that fits its line into the face's, as Windows sizes linked
    /// fonts, and kept within that line, so nothing it overhangs is left behind on redraw.
    fn draw_fallback(&mut self, at: usize, from: usize, px: u32, c: char) -> Glyph {
        let ([ascent, descent], [above, below]) = (self.reach(at, px), self.reach(from, px));
        let fitted = (px as f32 * crate::system_fonts::fit((ascent + descent) as f32, (above + below) as f32)).round().max(1.0);
        let font = self.sized(from, fitted as u32);
        // SAFETY: as in `draw`.
        let glyph = unsafe { rendered(font, (self.ft.FT_Load_Char)(font, c as c_ulong, 4 | 8) != 0, px) };
        let (top, cover) = crate::system_fonts::within_line(glyph.top, glyph.width, &glyph.cover, [ascent, descent]);
        Glyph { top, cover: cover.to_vec(), ..glyph }
    }

    /// How far a font's line reaches above and below the baseline at a pixel size, in whole
    /// pixels.
    fn reach(&mut self, at: usize, px: u32) -> [i32; 2] {
        // SAFETY: a sized face has its size record. (Heights are in 26.6 fixed point.)
        let size = unsafe { &*(*self.sized(at, px)).size };
        [size.ascender, -size.descender].map(|reach| (reach as f32 / 64.0).ceil() as i32)
    }
}

/// The glyph FreeType rendered into a font's slot; where loading it `failed`, a blank one
/// advancing half of `px`.
///
/// # Safety
/// The font is open, and its slot holds what was loaded last.
unsafe fn rendered(font: *mut FaceRec, failed: bool, px: u32) -> Glyph {
    if failed {
        return Glyph { width: 0, left: 0, top: 0, advance: px as f32 / 2.0, cover: Vec::new() };
    }
    // SAFETY: as the caller promises.
    unsafe {
        let slot = &*(*font).glyph;
        Glyph { width: slot.bitmap.width as usize, left: slot.left, top: slot.top, advance: slot.advance[0] as f32 / 64.0, cover: slot.bitmap.cover() }
    }
}

#[cfg(feature = "shaping")]
impl Fonts {
    /// Renders glyph `glyph` of a font as `draw` does a character (with `FT_Load_Glyph`,
    /// found when first needed): whether that failed.
    ///
    /// # Safety
    /// The font is open and sized.
    unsafe fn load_glyph(&self, font: *mut FaceRec, glyph: u16) -> bool {
        type Load = unsafe extern "C" fn(*mut FaceRec, c_uint, c_int) -> c_int;
        let Some(load) = self.ft.library.symbol(c"FT_Load_Glyph") else { return true };
        // SAFETY: the function is FreeType's, declared as it documents it; the font is as the
        // caller promises.
        unsafe { std::mem::transmute::<*const c_void, Load>(load)(font, glyph as c_uint, 4 | 8) != 0 }
    }

    /// The open font drawing a character of `face` (font `at`, or its fallback), if its glyph
    /// for the character is `own`: the font text shaped with that glyph was shaped in.
    fn shaped_in(&mut self, face: Face, at: usize, (letter, own): (char, u16)) -> Option<usize> {
        let from = if self.has(at, letter) { at } else { self.stand_in(face, letter)? };
        // SAFETY: the font is open, and stays so while this lives.
        let glyph = unsafe { (self.ft.FT_Get_Char_Index)(self.open[from].face, letter as c_ulong) };
        (glyph == c_uint::from(own)).then_some(from)
    }

    /// Glyph `glyph` of font `at`, `px` pixels high.
    fn draw_glyph(&mut self, at: usize, px: u32, glyph: u16) -> Glyph {
        let font = self.sized(at, px);
        // SAFETY: the font is open and sized; FreeType renders into its slot, read at once.
        unsafe { rendered(font, self.load_glyph(font, glyph), px) }
    }
}

impl Bitmap {
    /// The coverage of each pixel of a glyph FreeType rendered, row by row. A bitmap-only font
    /// returns its bitmaps anyway, a bit per pixel: its glyphs keep their width and stay empty.
    /// A zero-width glyph, which may still have rows, has no bitmap to read.
    ///
    /// # Safety
    /// The bitmap is as FreeType rendered it, and still there.
    unsafe fn cover(&self) -> Vec<u8> {
        let drawn = self.pixel_mode == 2 && !self.buffer.is_null() && self.width > 0;
        let (rows, width, pitch) = (if drawn { self.rows as isize } else { 0 }, self.width as usize, self.pitch as isize);
        // SAFETY: as the caller promises: each row starts `pitch` bytes after the previous
        // (before it, if negative) and is `width` bytes long.
        (0..rows).flat_map(|row| unsafe { std::slice::from_raw_parts(self.buffer.offset(row * pitch), width) }).copied().collect()
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: FreeType frees its open faces with itself; none is used after this.
        unsafe { (self.ft.FT_Done_FreeType)(self.library) };
    }
}

/// Text drawn by FreeType with the fonts Fontconfig picks: the desktop's choice for "sans" and
/// "monospace", and for characters they lack, the next font it would use.
pub struct SystemText {
    fonts: Fonts,
    /// Each face's font file and index, until opened.
    files: HashMap<Face, (CString, c_long)>,
    /// The open font each face uses.
    faces: HashMap<Face, usize>,
    glyphs: HashMap<Pictured, Glyph>,
    /// The language's forms in these fonts (see `system_forms`); and glyphs drawn by id (see
    /// `TextSystem::draw_glyphs`), by open font, pixel size and glyph.
    #[cfg(feature = "shaping")]
    forms: crate::system_forms::Forms,
    #[cfg(feature = "shaping")]
    by_id: HashMap<(usize, u32, u16), Glyph>,
}

/// A glyph's key: face, size, character; and with `shaping`, the glyph of its language form (0
/// for none).
#[cfg(feature = "shaping")]
type Pictured = (Face, u32, char, u16);
#[cfg(not(feature = "shaping"))]
type Pictured = (Face, u32, char);

impl SystemText {
    /// The system's text, as Fontconfig names the fonts and FreeType draws them. Nothing where
    /// the machine has neither.
    pub fn new() -> Option<Self> {
        let (ft, catalog) = (FreeType::open()?, Catalog::open()?);
        let mut library = null_mut();
        // SAFETY: FreeType hands back the library it starts.
        unsafe { (ft.FT_Init_FreeType)(&mut library) };
        // Fontconfig gives the font files; its full list is not kept, and neither is Fontconfig
        // (closed where `catalog` ends).
        let files = [Face::Regular, Face::Bold, Face::Mono, Face::Italic].into_iter().filter_map(|face| Some((face, catalog.file_of(&name_of(face))?))).collect();
        // And the emoji font file, for whatever draws emoji: see `emoji_file`.
        EMOJI_FILE.get_or_init(|| catalog.file_of("emoji").map(|(file, _)| PathBuf::from(std::ffi::OsStr::from_bytes(file.as_bytes()))));
        let fonts = Fonts { ft, library, open: Vec::new(), spares: HashMap::new() };
        let mut text = SystemText {
            fonts,
            files,
            faces: HashMap::new(),
            glyphs: HashMap::new(),
            #[cfg(feature = "shaping")]
            forms: crate::system_forms::Forms::new(),
            #[cfg(feature = "shaping")]
            by_id: HashMap::new(),
        };
        text.open(Face::Regular).then_some(text)
    }

    /// Opens a face's font if not yet open. Says whether it is open now.
    fn open(&mut self, face: Face) -> bool {
        if self.faces.contains_key(&face) {
            return true;
        }
        // A font FreeType cannot read is not tried again; a face the app named is looked up the
        // first time it is drawn.
        let named = || Catalog::open()?.file_of(&name_of(face));
        let Some(at) = self.files.remove(&face).or_else(named).and_then(|(file, index)| self.fonts.read(&file, index)) else { return false };
        self.faces.insert(face, at);
        true
    }

    /// What a font is drawn with: the face (the regular one, where the one asked for has no
    /// font of its own), its open font, and its pixel size.
    fn face(&mut self, font: Font, scale: f32) -> (Face, usize, u32) {
        let which = if self.open(font.face) { font.face } else { Face::Regular };
        (which, self.faces[&which], (font.size * scale).round().max(1.0) as u32)
    }

    /// A glyph of what `face` gave: rasterized when first asked for, and kept.
    #[cfg(not(feature = "shaping"))]
    fn glyph(&mut self, (which, at, px): (Face, usize, u32), c: char) -> &Glyph {
        let fonts = &mut self.fonts;
        self.glyphs.entry((which, px, c)).or_insert_with(|| fonts.draw(which, at, px, c))
    }
    /// The same, or that of its language form `form` (where not 0).
    #[cfg(feature = "shaping")]
    fn glyph(&mut self, (which, at, px): (Face, usize, u32), c: char, form: u16) -> &Glyph {
        let fonts = &mut self.fonts;
        self.glyphs.entry((which, px, c, form)).or_insert_with(|| fonts.draw(which, at, px, c, form))
    }

    /// Each character's form in a line, where it takes some (0 where none): see
    /// `system_forms`.
    #[cfg(feature = "shaping")]
    fn forms(&mut self, face: Face, text: &str) -> impl Iterator<Item = u16> + use<> {
        let glyphs = self.forms.glyphs(face, text).unwrap_or_default();
        glyphs.into_iter().map(|[own, form]| if form == own { 0 } else { form })
    }
}

impl std::fmt::Debug for SystemText {
    /// How many fonts are open and glyphs kept.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemText").field("fonts", &self.fonts.open.len()).field("letters", &self.glyphs.len()).finish()
    }
}

impl TextSystem for SystemText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        let (which, at, px) = self.face(font, scale);
        // SAFETY: a sized face has its size record. (Heights are in 26.6 fixed point.)
        let size = unsafe { &*(*self.fonts.sized(at, px)).size };
        let (ascent, descent) = ((size.ascender as f32 / 64.0).ceil() / scale, (-size.descender as f32 / 64.0).ceil() / scale);
        // Cap height and x-height: how far "H" and "x" reach above the baseline.
        #[cfg(feature = "shaping")]
        let [cap, small] = ['H', 'x'].map(|letter| self.glyph((which, at, px), letter, 0).top.max(0) as f32 / scale);
        #[cfg(not(feature = "shaping"))]
        let [cap, small] = ['H', 'x'].map(|letter| self.glyph((which, at, px), letter).top.max(0) as f32 / scale);
        Metrics { ascent, descent, cap_height: cap, x_height: small }
    }

    #[cfg(feature = "shaping")]
    fn set_language(&mut self, language: crate::scripts::Language) {
        self.forms.set(language);
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        let (with, mut x) = (self.face(font, scale), 0.0);
        #[cfg(feature = "shaping")]
        let mut forms = self.forms(with.0, text);
        for c in text.chars() {
            #[cfg(feature = "shaping")]
            let g = self.glyph(with, c, forms.next().unwrap_or(0));
            #[cfg(not(feature = "shaping"))]
            let g = self.glyph(with, c);
            x += g.advance;
            if let Some(stops) = stops.as_deref_mut() {
                stops.push(x / scale);
            }
        }
        x / scale
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let ascent = (self.metrics(font, scale).ascent * scale).round() as i32;
        let (with, mut pen, top) = (self.face(font, scale), (x * scale).round(), (y * scale).round() as i32 + ascent);
        #[cfg(feature = "shaping")]
        let mut forms = self.forms(with.0, text);
        for c in text.chars() {
            #[cfg(feature = "shaping")]
            let g = self.glyph(with, c, forms.next().unwrap_or(0));
            #[cfg(not(feature = "shaping"))]
            let g = self.glyph(with, c);
            canvas.cover(pen as i32 + g.left, top - g.top, g.width, &g.cover, color);
            pen += g.advance;
        }
    }

    #[cfg(feature = "shaping")]
    fn draw_glyphs(&mut self, canvas: &mut Canvas, font: Font, letter: (char, u16), glyphs: &[(u16, [f32; 2])], [x, line]: [i32; 2], color: Color) -> bool {
        let (which, at, px) = self.face(font, canvas.scale());
        let Some(from) = self.fonts.shaped_in(which, at, letter) else { return false };
        for (id, [right, down]) in glyphs {
            let fonts = &mut self.fonts;
            let g = self.by_id.entry((from, px, *id)).or_insert_with(|| fonts.draw_glyph(from, px, *id));
            canvas.cover(x + right.round() as i32 + g.left, line + down.round() as i32 - g.top, g.width, &g.cover, color);
        }
        true
    }
}

// `XEvent`, as far as read here (24 longs).
#[repr(C)]
union XEvent {
    kind: c_int,
    input: InputEvent,
    configure: ConfigureEvent,
    expose: ExposeEvent,
    client: ClientEvent,
    property: PropertyEvent,
    request: RequestEvent,
    selection: SelectionEvent,
    /// A generic event (type 35): XInput 2, for fingers, pens and the pointer.
    #[cfg(feature = "touch")]
    cookie: crate::xinput::Cookie,
    size: [c_long; 24],
}
/// Keys, buttons and motion share this layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    window: XWindow,
    root: XWindow,
    child: XWindow,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    /// The button, or the keycode.
    detail: c_uint,
}

#[cfg(feature = "touch")]
impl InputEvent {
    /// The core pointer event X would have sent for what XInput 2 reported (see `xinput`).
    fn of(p: crate::xinput::Pointer, display: Display) -> InputEvent {
        let ([x, y], [x_root, y_root]) = (p.at, p.root);
        InputEvent { kind: p.kind, serial: 0, sent: 0, display, window: p.window, root: 0, child: 0, time: p.time, x, y, x_root, y_root, state: p.state, detail: p.detail }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ConfigureEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    event: XWindow,
    window: XWindow,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ExposeEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    window: XWindow,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
}
/// `XClientMessageEvent`: five numbers under an agreed atom.
#[repr(C)]
#[derive(Clone, Copy)]
struct ClientEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    window: XWindow,
    message: Atom,
    format: c_int,
    data: [c_long; 5],
}
/// `XPropertyEvent`: a property written (`state` 0) or deleted (1). `XDestroyWindowEvent`
/// matches it up to `window`.
#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    window: XWindow,
    atom: Atom,
    time: c_ulong,
    state: c_int,
}
/// `XSelectionRequestEvent`: another program asks for what was copied here.
#[repr(C)]
#[derive(Clone, Copy)]
struct RequestEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    owner: XWindow,
    requestor: XWindow,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: c_ulong,
}
/// `XSelectionEvent`: the reply; the data is in the requester's `property`, 0 for none.
#[repr(C)]
#[derive(Clone, Copy)]
struct SelectionEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: Display,
    requestor: XWindow,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: c_ulong,
}
/// `XSizeHints`, as far as set here: which fields are given, and the least size.
#[repr(C)]
struct SizeHints {
    flags: c_long,
    place: [c_int; 4],
    min: [c_int; 2],
    rest: [c_int; 12],
}

/// The size hints a window gives the window manager: least size `least` (`PMinSize`), and the
/// same as max if `fixed` (`PMaxSize`); and if `placed`, `USPosition` (1): its place is the
/// user's choice, as when an app restores where the window was left. Every window manager
/// honours that; a mere `PPosition` many ignore.
fn hints(least: [c_int; 2], fixed: bool, placed: bool) -> SizeHints {
    let most = if fixed { least } else { [0; 2] };
    SizeHints { flags: 1 << 4 | (fixed as c_long) << 5 | placed as c_long, place: [0; 4], min: least, rest: [most[0], most[1], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] }
}

/// `XSetWindowAttributes`, of which three are set: background pixel, border pixel, colormap.
#[repr(C)]
struct Attributes {
    ground: [c_ulong; 2],
    border: [c_ulong; 2],
    gravity: [c_int; 3],
    backing: [c_ulong; 2],
    save_under: c_int,
    masks: [c_long; 2],
    unmanaged: c_int,
    colors: c_ulong,
    cursor: c_ulong,
}

/// `XVisualInfo`.
#[repr(C)]
struct VisualInfo {
    visual: *mut c_void,
    id: c_ulong,
    screen: c_int,
    depth: c_int,
    kind: c_int,
    masks: [c_ulong; 3],
    colors: c_int,
    bits: c_int,
}

/// The `_NET_WM_STATE` atoms for maximized (horizontally and vertically) and fullscreen.
const FILLING: [&CStr; 2] = [c"_NET_WM_STATE_MAXIMIZED_HORZ", c"_NET_WM_STATE_MAXIMIZED_VERT"];
const WHOLE: &CStr = c"_NET_WM_STATE_FULLSCREEN";

/// How long a window must stay put before that place is the one it restores to from maximized.
/// A window manager maximizing a window reports it in several steps, in its own order, with
/// gaps: a new frame or size shows some ms before the state does. (No two steps are this far
/// apart; a person moving a window by hand and then maximizing it takes longer.)
const SETTLED: Duration = Duration::from_millis(250);

/// Two presses on a window's move strip within this many ms are a double press.
const DOUBLE_PRESS: c_ulong = 500;

/// A window's state from its `_NET_WM_STATE` list: maximized (both horizontally and
/// vertically), and fullscreen. (The atoms are `FILLING` and `WHOLE`.)
fn stands(state: &[Atom], [wide, high, whole]: [Atom; 3]) -> [bool; 2] {
    [state.contains(&wide) && state.contains(&high), state.contains(&whole)]
}

/// Where a window held by its strip or some edges lies after the pointer moved `by` since
/// grabbing it: it `was` at (left, top), (wide, high), with least size `least`. Held by no
/// edge, all of it moves; else each held edge (-1 the first on an axis, 1 the last) moves and
/// the opposite one stays.
fn dragged([across, down]: [i8; 2], [left, top, wide, high]: [c_int; 4], by: [c_int; 2], least: [c_int; 2]) -> [c_int; 4] {
    let moved = (across, down) == (0, 0);
    let along = |edge: i8, start: c_int, long: c_int, by: c_int, least: c_int| match edge {
        -1 => (start + long - (long - by).max(least), (long - by).max(least)),
        1 => (start, (long + by).max(least)),
        _ => (start + if moved { by } else { 0 }, long),
    };
    let ((left, wide), (top, high)) = (along(across, left, wide, by[0], least[0]), along(down, top, high, by[1], least[1]));
    [left, top, wide, high]
}

/// The modifiers held once modifier key `sym` went down or up (Shift, Control, Alt, left or
/// right; some keyboards call Alt "Meta"), from the state X gave before it.
fn held_after(sym: c_ulong, state: c_uint, down: bool) -> Option<Mods> {
    let mut held = held(state);
    match sym {
        0xffe1 | 0xffe2 => held.shift = down,
        0xffe3 | 0xffe4 => held.ctrl = down,
        0xffe7..=0xffea => held.alt = down,
        _ => return None,
    }
    Some(held)
}

pub(crate) fn key(sym: c_ulong) -> Option<Key> {
    Some(match sym {
        0xff08 => Key::Backspace,
        0xff09 => Key::Tab,
        0xff0d | 0xff8d => Key::Enter,
        0xff1b => Key::Escape,
        0x0020 => Key::Space,
        0xff50 => Key::Home,
        0xff51 => Key::Left,
        0xff52 => Key::Up,
        0xff53 => Key::Right,
        0xff54 => Key::Down,
        0xff55 => Key::PageUp,
        0xff56 => Key::PageDown,
        0xff57 => Key::End,
        0xff67 => Key::Menu,
        0xffff => Key::Delete,
        0xffbe..=0xffc9 => Key::F((sym - 0xffbd) as u8),
        0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a => Key::Char((sym as u8 as char).to_ascii_lowercase()),
        // The zoom keys: "=" ("+" with Shift) and "-", and the two on the number pad.
        0x3d => Key::Char('='),
        0x2d | 0xffad => Key::Char('-'),
        0x2b | 0xffab => Key::Char('+'),
        _ => return None,
    })
}

/// X's error handler, set to do nothing: a request naming another program's window fails when
/// that window is gone, and X's default handler would exit the program.
unsafe extern "C" fn shrug(_: Display, _: *mut c_void) -> c_int {
    0
}

/// An X error handler as a program sets it: a function, or none for X's default. One per
/// program.
type Handler = Option<unsafe extern "C" fn(Display, *mut c_void) -> c_int>;

/// Whether that is `shrug`.
fn shrugs(handler: Handler) -> bool {
    handler.is_some_and(|handler| std::ptr::fn_addr_eq(handler, shrug as unsafe extern "C" fn(Display, *mut c_void) -> c_int))
}

/// How many connections are open, and the error handler before the first set `shrug`: restored
/// when the last closes. (A host program may have its own.)
static CONNECTIONS: std::sync::Mutex<(usize, Handler)> = std::sync::Mutex::new((0, None));

/// The most bytes taken of what another program hands over.
pub(crate) const MOST: usize = 1 << 26;

/// A copy too large for one reply, going to another program in `INCR` pieces through a property
/// of its window, each when the last was taken.
struct Going {
    to: XWindow,
    property: Atom,
    /// Its type and bytes.
    kind: Atom,
    bytes: Rc<[u8]>,
    /// How many bytes have been sent so far.
    left: usize,
}

/// What `Connection::heard` waits for: an event of one type about one window, and for property
/// news, that this property was written.
#[repr(C)]
struct Awaited {
    kind: c_int,
    window: XWindow,
    property: Atom,
}

/// Whether an event is the awaited one: X's predicate for `XIfEvent`. Events passed over stay
/// queued for the windows' loop.
unsafe extern "C" fn awaited(_: Display, event: *mut XEvent, wanted: *mut c_char) -> c_int {
    // SAFETY: X hands over one of its events (plain numbers), and what `heard` passed in, which
    // outlives the call.
    let (news, wanted) = unsafe { ((*event).property, &*wanted.cast::<Awaited>()) };
    (news.kind == wanted.kind && news.window == wanted.window && (news.kind != 28 || (news.atom, news.state) == (wanted.property, 0))) as c_int
}

/// The MIME types X programs exchange: a PNG picture; files as a URI list; the same list as
/// file managers pass it, under a first line saying copy or cut; and UTF-8 text by its MIME
/// name (`UTF8_STRING` is the older one).
const PICTURE: &CStr = c"image/png";
const FILES: &CStr = c"text/uri-list";
const COPIED_FILES: &CStr = c"x-special/gnome-copied-files";
const TEXT: &CStr = c"text/plain;charset=utf-8";

/// Something handed to other programs as each type they may ask for, with its bytes as that.
type Kinds = Vec<(Atom, Rc<[u8]>)>;

/// The X clipboard: whoever copied last owns the selection and hands it to any program asking,
/// in the type asked, as long as it runs. (Drag and drop hands over data the same way, so it is
/// read through this too.)
struct Connection {
    x: Xlib,
    display: Display,
    /// The window owning what is copied here, which asks for what is pasted.
    window: XWindow,
    /// The atoms of the clipboard, `UTF8_STRING`, `TARGETS`, and the property replies are left
    /// in.
    selection: Atom,
    utf8: Atom,
    targets: Atom,
    mailbox: Atom,
    /// What was copied here, while this window owns the selection: each type it is offered as,
    /// with its bytes. (Empty while another program owns it, or none does.)
    held: RefCell<Kinds>,
    /// The same for what one of the program's windows drags out to other programs, under
    /// `XdndSelection`, with the window dragging it, which owns it.
    dnd: Atom,
    dragged_out: RefCell<(XWindow, Kinds)>,
    /// Copies going to other programs in pieces.
    going: RefCell<Vec<Going>>,
    /// The program's icon as X keeps it on a window (see `pictured`).
    icons: Vec<c_ulong>,
    /// RandR's monitor queries, where X has RandR. (Dropped after the display closes: X holds a
    /// function of it until then.)
    randr: Option<Xrandr>,
    /// XInput 2 for the pointer, fingers, pens and touchpad pinches, where X has it and the
    /// program takes touch. (Dropped after the display closes, as RandR is.)
    #[cfg(feature = "touch")]
    inputs: Option<RefCell<crate::xinput::Inputs>>,
    /// The windows ask the GPU to show them (feature `gpu`): X is to fill no background under
    /// them.
    #[cfg(feature = "gpu")]
    unfilled: bool,
}

/// The program's icons as `_NET_WM_ICON` holds them: each as width, height, then its pixels,
/// every number a C long.
fn pictured(icons: &[(usize, &[u32])]) -> Vec<c_ulong> {
    let mut numbers = Vec::new();
    for (width, pixels) in icons {
        numbers.extend([*width, pixels.len() / (*width).max(1)].map(|number| number as c_ulong));
        numbers.extend(pixels.iter().map(|pixel| *pixel as c_ulong));
    }
    numbers
}

impl Connection {
    /// The clipboard `selection`, for a window.
    ///
    /// # Safety
    /// `display` was opened with `x` for this alone, and is closed when this ends; `window` is
    /// alive while this is used and has selected `PropertyChangeMask` (a paste in pieces
    /// arrives by that).
    unsafe fn new(x: Xlib, display: Display, window: XWindow, selection: &CStr) -> Self {
        // SAFETY: every name ends in a zero, and `shrug` is X's error handler.
        let atom = |name: &CStr| unsafe { (x.XInternAtom)(display, name.as_ptr(), 0) };
        let mut conns = CONNECTIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = unsafe { (x.XSetErrorHandler)(Some(shrug)) };
        conns.1 = if shrugs(before) { conns.1 } else { before };
        conns.0 += 1;
        let (selection, utf8, targets, mailbox) = (atom(selection), atom(c"UTF8_STRING"), atom(c"TARGETS"), atom(c"TAUT_CLIPBOARD"));
        let (held, dnd, dragged_out) = (RefCell::new(Vec::new()), atom(c"XdndSelection"), RefCell::new((0, Vec::new())));
        Connection {
            x,
            display,
            window,
            selection,
            utf8,
            targets,
            mailbox,
            held,
            dnd,
            dragged_out,
            going: RefCell::new(Vec::new()),
            icons: Vec::new(),
            #[cfg(feature = "gpu")]
            unfilled: false,
            randr: None,
            #[cfg(feature = "touch")]
            inputs: None,
        }
    }

    /// Gives new window `id` its title, its frame or none, maximized or not, the program's
    /// class and its icons, before it is mapped.
    fn set_window_properties(&self, id: XWindow, title: &str, frameless: bool, maximized: bool) {
        self.name(id, title);
        if frameless {
            let frame = self.atom(c"_MOTIF_WM_HINTS");
            self.leave(id, frame, frame, &[2 as c_long, 0, 0, 0, 0]);
        }
        if maximized {
            self.leave(id, self.atom(c"_NET_WM_STATE"), 4, &[self.atom(FILLING[0]), self.atom(FILLING[1])]);
        }
        // `WM_CLASS`, for task bars and docks grouping a program's windows: the program's name
        // twice (instance and class), each zero-terminated. (Type STRING, 31.)
        let program = std::env::current_exe().ok().and_then(|path| path.file_stem().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default();
        self.leave(id, self.atom(c"WM_CLASS"), 31, format!("{program}\0{program}\0").as_bytes());
        // The program's icons, if any: task bars and window lists show the one that fits. (Type
        // CARDINAL, 6.)
        if !self.icons.is_empty() {
            self.leave(id, self.atom(c"_NET_WM_ICON"), 6, &self.icons);
        }
    }

    /// An atom by name. (Xlib caches them: only the first lookup goes to X.)
    fn atom(&self, name: &CStr) -> Atom {
        // SAFETY: the name ends in a zero, and the display is open.
        unsafe { (self.x.XInternAtom)(self.display, name.as_ptr(), 0) }
    }

    /// Sends a client message of five numbers under an atom both programs know, to window `to`
    /// about window `about` (the same, or one `to` stands for).
    fn tell(&self, to: XWindow, about: XWindow, name: &CStr, data: [c_long; 5]) {
        // SAFETY: the message is an X event; if the window is gone meanwhile, X tells `shrug`.
        unsafe {
            let mut event: XEvent = zeroed();
            event.client = ClientEvent { kind: 33, serial: 0, sent: 1, display: self.display, window: about, message: self.atom(name), format: 32, data };
            (self.x.XSendEvent)(self.display, to, 0, 0, &mut event);
            (self.x.XFlush)(self.display);
        }
    }

    /// Puts something on the clipboard, replacing everything: in each type another program may
    /// ask for, with its bytes as that. (X only learns that this window owns the clipboard; the
    /// bytes stay here until asked for.)
    fn set(&self, kinds: Kinds) {
        *self.held.borrow_mut() = kinds;
        // SAFETY: the display and window are alive (see `new`).
        unsafe {
            (self.x.XSetSelectionOwner)(self.display, self.selection, self.window, 0);
            (self.x.XFlush)(self.display);
        }
    }

    /// X says another program copied (or now drags: `selection` says which): it owns it now,
    /// and what was held here is dropped. (Unless something was copied here again before the
    /// news was read: then one of these windows owns it again, and the news is stale.)
    fn lost(&self, selection: Atom) {
        // SAFETY: the display is open.
        let owner = unsafe { (self.x.XGetSelectionOwner)(self.display, selection) };
        let mut dragged_out = self.dragged_out.borrow_mut();
        match selection {
            _ if selection == self.selection && owner != self.window => self.held.borrow_mut().clear(),
            _ if selection == self.dnd && owner != dragged_out.0 => *dragged_out = (0, Vec::new()),
            _ => {}
        }
    }

    /// Holds what window `from` drags out, in each type other programs may ask for, owned by
    /// that window from `time`; replacing what was dragged before. (With nothing and no window,
    /// nothing is dragged any more.)
    fn own_drag_out(&self, kinds: Kinds, from: XWindow, time: c_ulong) {
        *self.dragged_out.borrow_mut() = (from, kinds);
        // SAFETY: the display is open, and the window, if named, is the program's.
        unsafe {
            (self.x.XSetSelectionOwner)(self.display, self.dnd, from, time);
            (self.x.XFlush)(self.display);
        }
    }

    /// What one of the program's windows drags out, in one type, while it does: at hand for
    /// another of its windows it is dropped on, which could not ask X and wait for the reply.
    /// (Nothing while nothing is dragged here.)
    fn dragged_out_as(&self, kind: Atom) -> Option<Vec<u8>> {
        self.dragged_out.borrow().1.iter().find(|(dragged_out_as, _)| *dragged_out_as == kind).map(|(_, bytes)| bytes.to_vec())
    }

    /// What something dragged out of a window is to X programs, in each type with its bytes:
    /// files as a URI list; text in UTF-8 under both names; a picture as PNG; a link as a
    /// one-entry URI list and as text.
    fn kinds_of(&self, what: DragData) -> Kinds {
        let text = |text: &str| {
            let bytes: Rc<[u8]> = text.as_bytes().into();
            [(self.atom(TEXT), bytes.clone()), (self.utf8, bytes)]
        };
        match what {
            DragData::Files(files) => vec![(self.atom(FILES), lists_of(files)[0].clone().into())],
            DragData::Text(words) => text(words).to_vec(),
            DragData::Picture(width, pixels) => crate::png::encode_rgba(width, pixels).map(|png| (self.atom(PICTURE), png.into())).into_iter().collect(),
            DragData::Link(address) => [vec![(self.atom(FILES), format!("{address}\r\n").into_bytes().into())], text(address).to_vec()].concat(),
        }
    }

    /// What is on the clipboard in one type; nothing if not offered so. (What was copied here
    /// is at hand; X is not asked.)
    fn get(&self, kind: Atom) -> Option<Vec<u8>> {
        let held = self.held.borrow();
        if held.is_empty() {
            return self.read(self.window, self.selection, kind, 0);
        }
        held.iter().find(|(held_as, _)| *held_as == kind).map(|(_, bytes)| bytes.to_vec())
    }

    /// Asks the owner of a selection (the clipboard, or what another program drags) for it in
    /// one type, on behalf of one of this program's windows, and waits for all of it. Nothing
    /// if nobody owns it, or not in that type.
    fn read(&self, window: XWindow, selection: Atom, kind: Atom, time: c_ulong) -> Option<Vec<u8>> {
        let asked = Instant::now();
        // SAFETY: the display is open, and the window is the program's.
        unsafe {
            (self.x.XConvertSelection)(self.display, selection, kind, self.mailbox, window, time);
            (self.x.XFlush)(self.display);
        }
        // The owner replies with a `SelectionNotify` (31) naming the property holding the data.
        let mut answer = self.heard(window, 31, asked)?;
        // SAFETY: an event's fields are plain numbers, and a type-31 event has this layout.
        while unsafe { answer.selection.selection != selection || answer.selection.target != kind } {
            answer = self.heard(window, 31, asked)?; // a late reply to an earlier request
        }
        if unsafe { answer.selection.property } != self.mailbox {
            return None;
        }
        let (first, bits, mut all) = self.property(window, self.mailbox, true)?;
        if first != self.atom(c"INCR") {
            return (first == kind && bits == 8).then_some(all);
        }
        // Too long for one reply: it comes `INCR`, each piece left in the property when the
        // last was taken, an empty one ending it. Data of another type than asked, or too much,
        // is no answer: its pieces are still taken to the last and dropped. (The owner waits
        // for each to be taken and would otherwise leave the rest in the property mid-way
        // through the next request.)
        let mut good = true;
        all.clear();
        loop {
            self.heard(window, 28, asked)?;
            match self.property(window, self.mailbox, true) {
                Some((_, _, bytes)) if bytes.is_empty() => return good.then_some(all),
                Some((piece, 8, bytes)) if good && piece == kind && all.len() + bytes.len() <= MOST => all.extend(bytes),
                Some(_) => (good, all) = (false, Vec::new()),
                None => {} // news of a piece already taken
            }
        }
    }

    /// Waits for an event of one type about a window and returns it: a `SelectionNotify` (31)
    /// or a `PropertyNotify` (28). Other events stay queued for the windows' loop. A live
    /// program sends its next piece within 0.5 s and all of it within 5 s of being `asked`: the
    /// windows stand still meanwhile, no longer than that, however much a program keeps
    /// sending.
    fn heard(&self, window: XWindow, kind: c_int, asked: Instant) -> Option<XEvent> {
        let until = (Instant::now() + Duration::from_millis(500)).min(asked + Duration::from_secs(5));
        let mut wanted = Awaited { kind, window, property: self.mailbox };
        // SAFETY: the display is open; X fills in the event and calls `awaited` with what is
        // wanted, alive for the call. With nothing queued, X waits on the display's connection.
        let mut event: XEvent = unsafe { zeroed() };
        while Instant::now() <= until {
            if unsafe { (self.x.XCheckIfEvent)(self.display, &mut event, awaited, (&raw mut wanted).cast()) } != 0 {
                return Some(event);
            }
            unsafe { poll(&mut PollFd { fd: (self.x.XConnectionNumber)(self.display), events: 1, revents: 0 }, 1, 20) };
        }
        None
    }

    /// A window property: its type, format (bits per item) and items as bytes (32-bit items
    /// come as C longs, whatever their size). With `take`, it is also deleted, which is how a
    /// sender learns it arrived. Nothing for no such property or window.
    fn property(&self, window: XWindow, name: Atom, take: bool) -> Option<(Atom, c_int, Vec<u8>)> {
        let (mut kind, mut bits, mut count, mut left, mut data) = (0, 0, 0, 0, null_mut());
        // SAFETY: X fills these in and returns `count` items, copied before they are freed.
        unsafe {
            let failed = (self.x.XGetWindowProperty)(self.display, window, name, 0, (MOST / 4) as c_long, take as c_int, 0, &mut kind, &mut bits, &mut count, &mut left, &mut data);
            if failed != 0 || data.is_null() {
                return None;
            }
            let size = if bits == 32 { size_of::<c_long>() } else { bits as usize / 8 };
            let held = std::slice::from_raw_parts(data, count as usize * size).to_vec();
            (self.x.XFree)(data.cast());
            Some((kind, bits, held))
        }
    }

    /// The numbers a window property holds (atoms are numbers too); none where it holds none.
    fn numbers(&self, window: XWindow, name: &CStr) -> Vec<c_ulong> {
        let Some((_, 32, bytes)) = self.property(window, self.atom(name), false) else { return Vec::new() };
        bytes.as_chunks().0.iter().map(|number| c_ulong::from_ne_bytes(*number)).collect()
    }

    /// Writes a window property of one type: bytes, or 32-bit numbers (which X wants as C
    /// longs).
    fn leave<Piece>(&self, window: XWindow, property: Atom, kind: Atom, pieces: &[Piece]) {
        let bits = if size_of::<Piece>() == 1 { 8 } else { 32 };
        // SAFETY: the property is written from as many items as passed.
        unsafe { (self.x.XChangeProperty)(self.display, window, property, kind, bits, 0, pieces.as_ptr().cast(), pieces.len() as c_int) };
    }

    /// Where a window's corner is on screen, in pixels: the window's own, not its frame's.
    fn place(&self, window: XWindow) -> [c_int; 2] {
        let (mut place, mut child) = ([0; 2], 0);
        // SAFETY: the display is open, and X fills these in. (If the window is gone, X tells
        // `shrug` and the place stays as it was.)
        unsafe { (self.x.XTranslateCoordinates)(self.display, window, root(&self.x, self.display), 0, 0, &mut place[0], &mut place[1], &mut child) };
        place
    }

    /// Asks the window manager something about a window: a client message under an agreed atom,
    /// sent to the root window, where a window manager listens
    /// (`SubstructureRedirectMask | SubstructureNotifyMask`). With no window manager, nobody
    /// hears it.
    fn ask(&self, window: XWindow, name: &CStr, data: [c_long; 5]) {
        // SAFETY: the message is an X event.
        unsafe {
            let mut event: XEvent = zeroed();
            event.client = ClientEvent { kind: 33, serial: 0, sent: 1, display: self.display, window, message: self.atom(name), format: 32, data };
            (self.x.XSendEvent)(self.display, root(&self.x, self.display), 0, 1 << 19 | 1 << 20, &mut event);
            (self.x.XFlush)(self.display);
        }
    }

    /// The XSETTINGS manager's window, if one runs: it owns `_XSETTINGS_S<screen>`. (0 where
    /// none does.)
    fn settings(&self) -> XWindow {
        // SAFETY: plain queries on an open display; the name ends in a zero.
        let name = CString::new(format!("_XSETTINGS_S{}", unsafe { (self.x.XDefaultScreen)(self.display) })).unwrap_or_default();
        unsafe { (self.x.XGetSelectionOwner)(self.display, self.atom(&name)) }
    }

    /// What those settings, as they stand on that window now, say the person asked for: what
    /// windows go by where the portal says nothing. (Nothing asked for where there is no such
    /// window.)
    fn desk(&self, settings: XWindow) -> Preferences {
        let kept = (settings != 0).then(|| self.property(settings, self.atom(c"_XSETTINGS_SETTINGS"), false)).flatten();
        kept.and_then(|(_, _, bytes)| of_x_settings(&bytes)).unwrap_or_default()
    }

    /// Whether an EWMH window manager runs (it says so on the root window,
    /// `_NET_SUPPORTING_WM_CHECK`).
    fn managed(&self) -> bool {
        !self.numbers(root(&self.x, self.display), c"_NET_SUPPORTING_WM_CHECK").is_empty()
    }

    /// Whether the window manager supports this atom (`_NET_SUPPORTED` on the root window). Not
    /// where there is none.
    fn offers(&self, name: &CStr) -> bool {
        self.numbers(root(&self.x, self.display), c"_NET_SUPPORTED").contains(&self.atom(name))
    }

    /// Sets a window's title: as UTF-8 (`_NET_WM_NAME`), and again for tools that only know
    /// `WM_NAME`.
    fn name(&self, window: XWindow, title: &str) {
        self.leave(window, self.atom(c"_NET_WM_NAME"), self.utf8, title.as_bytes());
        // SAFETY: the display is open, and the title ends in a zero.
        unsafe { (self.x.XStoreName)(self.display, window, CString::new(title).unwrap_or_default().as_ptr()) };
    }

    /// Another program asks for what was copied here: hands it over, or says there is nothing.
    ///
    /// # Safety
    /// `asked` is a request X delivered on this display.
    unsafe fn answer(&self, asked: RequestEvent) {
        let (dragged_out, copied) = (self.dragged_out.borrow(), self.held.borrow());
        let held = if asked.selection == self.dnd && self.dnd != self.selection { &dragged_out.1 } else { &*copied };
        // An ICCCM-obsolete requester leaves the owner to pick the property.
        let mut property = if asked.property == 0 { asked.target } else { asked.property };
        // What is asked: the list of types offered (`TARGETS`, as atoms, type 4), or the data
        // in one of them.
        if let Some((kind, bytes)) = held.iter().find(|(kind, _)| *kind == asked.target) {
            self.hand_over(asked.requestor, property, *kind, bytes);
        } else if asked.target == self.targets && !held.is_empty() {
            let kinds: Vec<Atom> = std::iter::once(self.targets).chain(held.iter().map(|(kind, _)| *kind)).collect();
            self.leave(asked.requestor, property, 4, &kinds);
        } else {
            property = 0;
        }
        // SAFETY: the reply is an X event, sent to the requester.
        unsafe {
            let mut reply: XEvent = zeroed();
            reply.selection =
                SelectionEvent { kind: 31, serial: 0, sent: 1, display: self.display, requestor: asked.requestor, selection: asked.selection, target: asked.target, property, time: asked.time };
            (self.x.XSendEvent)(self.display, asked.requestor, 0, 0, &mut reply);
            (self.x.XFlush)(self.display);
        }
    }

    /// The most bytes of a copy per X request: the max request size (in four-byte words, at
    /// least 4096), less the request's own few words.
    fn piece(&self) -> usize {
        // SAFETY: the display is open.
        (unsafe { (self.x.XMaxRequestSize)(self.display) } as usize - 16) * 4
    }

    /// Leaves what was copied for the requesting window in the type it asked: all of it if one
    /// request carries it; else its size as type `INCR`, the pieces following in `hand_on`,
    /// each when the window took the last (X is asked to report that).
    fn hand_over(&self, to: XWindow, property: Atom, kind: Atom, bytes: &Rc<[u8]>) {
        if bytes.len() <= self.piece() {
            return self.leave(to, property, kind, bytes);
        }
        // SAFETY: the display is open. (Asks for the other window's property changes and
        // destruction.)
        unsafe { (self.x.XSelectInput)(self.display, to, 1 << 17 | 1 << 22) };
        self.leave(to, property, self.atom(c"INCR"), &[bytes.len() as c_ulong]);
        self.going.borrow_mut().push(Going { to, property, kind, bytes: bytes.clone(), left: 0 });
    }

    /// X has news of a window a copy goes to in pieces: it is `gone`, and the copy forgotten;
    /// or it took the last piece, and the next is left. An empty piece ends it.
    fn hand_on(&self, news: PropertyEvent, gone: bool) {
        let mut going = self.going.borrow_mut();
        if gone {
            return going.retain(|copy| copy.to != news.window);
        }
        let Some(at) = going.iter().position(|copy| copy.to == news.window && copy.property == news.atom && news.state == 1) else { return };
        let (copy, piece) = (&mut going[at], self.piece());
        let next = &copy.bytes[copy.left..(copy.left + piece).min(copy.bytes.len())];
        self.leave(copy.to, copy.property, copy.kind, next);
        copy.left += next.len();
        if next.is_empty() {
            going.remove(at);
        }
        // Once nothing more goes to the window, nothing more is asked of it.
        let heard = if going.iter().any(|copy| copy.to == news.window) { 1 << 17 | 1 << 22 } else { 0 };
        // SAFETY: the display is open.
        unsafe {
            (self.x.XSelectInput)(self.display, news.window, heard);
            (self.x.XFlush)(self.display);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: the display was opened for this (see `new`) and nothing uses it after. X
        // frees what was made on it: windows and selections. (Errors meanwhile still go to
        // `shrug`.)
        unsafe { (self.x.XCloseDisplay)(self.display) };
        // The last connection to close restores the error handler from before the first, unless
        // something set its own since, which then stays.
        let mut conns = CONNECTIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        conns.0 -= 1;
        if conns.0 == 0 {
            // SAFETY: what is restored is what X returned when it was replaced.
            let now = unsafe { (self.x.XSetErrorHandler)(conns.1) };
            if !shrugs(now) {
                unsafe { (self.x.XSetErrorHandler)(now) };
            }
        }
    }
}

struct SystemClipboard(Rc<Connection>);

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        let text = self.0.get(self.0.utf8)?;
        Some(String::from_utf8(text).unwrap_or_else(|other| String::from_utf8_lossy(other.as_bytes()).into_owned()))
    }
    fn set(&mut self, text: &str) {
        self.0.set(vec![(self.0.utf8, text.as_bytes().into())]);
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
    /// The windows' drag-out code: none until an interface on this thread hands something out
    /// (see `Outward`).
    static OUTWARD: Cell<Option<Outward>> = const { Cell::new(None) };
}

/// Hands the clipboard its picture and file code, on this thread (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    let (picture, set_picture) = (SystemClipboard::picture_on, SystemClipboard::put_picture);
    PICTURES_AND_FILES.set(Some(PicturesAndFiles { picture, set_picture, files: SystemClipboard::files_on, set_files: SystemClipboard::put_files }));
}

impl SystemClipboard {
    /// A picture, as PNG.
    fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        crate::png::decode(&self.0.get(self.0.atom(PICTURE))?)
    }
    /// Puts a picture on the clipboard as PNG.
    fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
        self.0.set(vec![(self.0.atom(PICTURE), png.into())]);
    }
    /// The files as a URI list; or, from a file manager offering nothing else, its own list
    /// format.
    fn files_on(&mut self) -> Vec<PathBuf> {
        let list = [FILES, COPIED_FILES].into_iter().find_map(|kind| self.0.get(self.0.atom(kind)));
        list.map_or(Vec::new(), |list| files_in(&list))
    }
    /// Puts files on the clipboard both ways: some file managers paste only the second.
    fn put_files(&mut self, files: &[PathBuf]) {
        let [list, copied] = lists_of(files);
        self.0.set(vec![(self.0.atom(FILES), list.into()), (self.0.atom(COPIED_FILES), copied.into())]);
    }
}

/// Files as the two lists put on the clipboard: a URI list with CRLF line ends; and for file
/// managers, "copy" (not "cut") then a URI per line.
pub(crate) fn lists_of(files: &[PathBuf]) -> [Vec<u8>; 2] {
    let (mut list, mut copied) = (String::new(), String::from("copy"));
    for address in files.iter().filter_map(|file| address_of(file)) {
        list = list + &address + "\r\n";
        copied = copied + "\n" + &address;
    }
    [list.into_bytes(), copied.into_bytes()]
}

/// A file's `file://` URI: its absolute path, every byte but letters, digits and a few signs
/// percent-encoded. Nothing for an empty path.
fn address_of(file: &Path) -> Option<String> {
    let (path, mut address) = (std::path::absolute(file).ok()?, String::from("file://"));
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'-' | b'.' | b'_' | b'~' | b'/' => address.push(*byte as char),
            byte if byte.is_ascii_alphanumeric() => address.push(*byte as char),
            _ => _ = write!(address, "%{byte:02X}"),
        }
    }
    Some(address)
}

/// The files a `text/uri-list` names, one per line, as file managers drag or copy them. URIs of
/// anything but local files are skipped, as are comment lines (`#`) and anything else that is
/// no URI (the first line of the file managers' own list).
pub(crate) fn files_in(list: &[u8]) -> Vec<PathBuf> {
    list.split(|byte| matches!(byte, b'\r' | b'\n' | 0)).filter_map(path_of).collect()
}

/// The path a `file:` URI names; nothing for other URIs. A host name before the path
/// (`file://host/path`) is dropped, and `%41` becomes the byte it encodes.
pub(crate) fn path_of(address: &[u8]) -> Option<PathBuf> {
    let rest = address.split_at_checked(5).filter(|(kind, _)| kind.eq_ignore_ascii_case(b"file:"))?.1;
    let from = if rest.starts_with(b"//") { 2 + rest[2..].iter().position(|byte| *byte == b'/')? } else { 0 };
    let (coded, mut path) = (&rest[from..], Vec::new());
    let digit = |at: usize| coded.get(at).and_then(|byte| (*byte as char).to_digit(16));
    let mut at = 0;
    while at < coded.len() {
        let byte = if coded[at] == b'%' { digit(at + 1).zip(digit(at + 2)).map(|(high, low)| (high * 16 + low) as u8) } else { None };
        path.push(byte.unwrap_or(coded[at]));
        at += if byte.is_some() { 3 } else { 1 };
    }
    // Every path starts with `/`, and none holds a zero.
    (path.starts_with(b"/") && !path.contains(&0)).then(|| OsString::from_vec(path).into())
}

/// Pixels per unit: the desktop's `Xft.dpi` over 96.
fn scale_of(x: &Xlib, display: Display) -> f32 {
    if let Some(forced) = std::env::var("TAUT_SCALE").ok().and_then(|v| decimal(&v)) {
        return forced;
    }
    // SAFETY: the string, if any, belongs to the display and ends in a zero.
    let settings = unsafe { (x.XResourceManagerString)(display) };
    if settings.is_null() {
        return 1.0;
    }
    let settings = unsafe { CStr::from_ptr(settings) }.to_string_lossy();
    settings.lines().find_map(|l| l.strip_prefix("Xft.dpi:")).and_then(|v| decimal(v.trim())).map_or(1.0, |dpi| dpi / 96.0)
}

/// A decimal number as X resources and people write a scale ("96", "144.5", "1.25"); nothing
/// for anything else. (Parsed here, not by `f32::from_str`, whose tables every program would
/// carry for a few digits.)
pub(crate) fn decimal(text: &str) -> Option<f32> {
    let (whole, part) = text.split_once('.').unwrap_or((text, ""));
    let digits = |side: &str| side.bytes().all(|byte| byte.is_ascii_digit());
    if whole.len() + part.len() == 0 || !digits(whole) || !digits(part) {
        return None;
    }
    let whole = whole.bytes().fold(0.0, |value, digit| value * 10.0 + f64::from(digit - b'0'));
    let part = part.bytes().rev().fold(0.0, |value, digit| (value + f64::from(digit - b'0')) / 10.0);
    Some((whole + part) as f32)
}

/// The modifier state of a key event.
fn event_state(event: &XEvent) -> c_uint {
    // SAFETY: the event is a key event, whose layout has the state where every pointer and key
    // event has it.
    unsafe { event.input.state }
}

/// The modifiers held, as X reports them.
fn held(state: c_uint) -> Mods {
    Mods { shift: state & 1 != 0, ctrl: state & 4 != 0, alt: state & 8 != 0 }
}

/// A size in units as whole pixels: at least one each way (X's smallest window), and no more
/// than X can count.
fn pixels(width: f32, height: f32, scale: f32) -> [c_int; 2] {
    [((width * scale) as c_int).clamp(1, 32_767), ((height * scale) as c_int).clamp(1, 32_767)]
}

/// The root window.
fn root(x: &Xlib, display: Display) -> XWindow {
    // SAFETY: plain queries on an open display.
    unsafe { (x.XRootWindow)(display, (x.XDefaultScreen)(display)) }
}

/// The person's screens as X has them now, main one first, all at `scale` (X desktops have one
/// scale): each RandR monitor where RandR exists, else the whole display as one; each with its
/// work area (`work_on`).
fn screens(conn: &Connection, scale: f32) -> Vec<Screen> {
    let (x, display, randr, mut count) = (&conn.x, conn.display, conn.randr.as_ref(), 0);
    // SAFETY: RandR returns `count` monitors, read and then freed.
    let monitors = randr.map_or(null_mut(), |randr| unsafe { (randr.XRRGetMonitors)(display, root(x, display), 1, &mut count) });
    let each = |nth: usize| unsafe { &*monitors.add(nth) };
    let mut found: Vec<_> = (0..count.max(0) as usize).map(each).map(|one| (one.main != 0, rect(one.x as f32, one.y as f32, one.width as f32, one.height as f32))).collect();
    if let Some(randr) = randr.filter(|_| !monitors.is_null()) {
        unsafe { (randr.XRRFreeMonitors)(monitors) };
    }
    if found.is_empty() {
        let [wide, high] = screen_size(x, display);
        found.push((true, rect(0.0, 0.0, wide as f32, high as f32)));
    }
    if let Some(main) = found.iter().position(|(main, _)| *main) {
        found[..=main].rotate_right(1);
    }
    let work = work_areas(conn);
    found.into_iter().map(|(_, all)| Screen { rect: all, work_area: work_on(&work, all), scale }).collect()
}

/// The desktop's work areas, as the window manager states them: one per monitor
/// (`_GTK_WORKAREAS_D<n>` for the current desktop, where given), else one for all
/// (`_NET_WORKAREA`, four numbers per desktop). None with no window manager.
fn work_areas(conn: &Connection) -> Vec<Rect> {
    let top = root(&conn.x, conn.display);
    let desktop = conn.numbers(top, c"_NET_CURRENT_DESKTOP").first().map_or(0, |nth| *nth as usize);
    let named = CString::new(format!("_GTK_WORKAREAS_D{desktop}")).unwrap_or_default();
    let (each, all) = (conn.numbers(top, &named), conn.numbers(top, c"_NET_WORKAREA"));
    let areas = if each.is_empty() { all.get(desktop * 4..desktop * 4 + 4).unwrap_or_default() } else { &each };
    areas.as_chunks::<4>().0.iter().map(|area| rect(area[0] as f32, area[1] as f32, area[2] as f32, area[3] as f32)).collect()
}

/// A monitor's work area: of the window manager's areas, the one overlapping it most, clipped
/// to it; all of it where none does.
fn work_on(areas: &[Rect], all: Rect) -> Rect {
    let most = areas.iter().map(|area| area.intersect(all)).max_by(|one, other| (one.w * one.h).total_cmp(&(other.w * other.h)));
    most.filter(|part| !part.is_empty()).unwrap_or(all)
}

/// Tells the window manager, before showing a second window, how it relates to the first: a
/// tool window or dialog is transient for the first (`WM_TRANSIENT_FOR`, type WINDOW 33), kept
/// over it and minimized with it; and by EWMH type (ATOM 4), a tool window is "utility" (off
/// the task bar), a dialog "modal", and one over all others "above".
fn tell_kind(conn: &Connection, id: XWindow, first: XWindow, kind: WindowKind) {
    let (named, states): (&CStr, &[&CStr]) = match kind {
        WindowKind::Plain => return,
        WindowKind::Tool => (c"_NET_WM_WINDOW_TYPE_UTILITY", &[c"_NET_WM_STATE_SKIP_TASKBAR"]),
        WindowKind::Dialog => (c"_NET_WM_WINDOW_TYPE_DIALOG", &[c"_NET_WM_STATE_MODAL", c"_NET_WM_STATE_SKIP_TASKBAR"]),
        WindowKind::OnTop => (c"_NET_WM_WINDOW_TYPE_NORMAL", &[c"_NET_WM_STATE_ABOVE"]),
    };
    if kind != WindowKind::OnTop {
        conn.leave(id, conn.atom(c"WM_TRANSIENT_FOR"), 33, &[first]);
    }
    conn.leave(id, conn.atom(c"_NET_WM_WINDOW_TYPE"), 4, &[conn.atom(named)]);
    conn.leave(id, conn.atom(c"_NET_WM_STATE"), 4, &states.iter().map(|name| conn.atom(name)).collect::<Vec<_>>());
}

/// The screen's size in pixels: all monitors as one area.
fn screen_size(x: &Xlib, display: Display) -> [c_uint; 2] {
    let (mut parent, mut corner, mut size, mut rest) = (0, [0; 2], [0; 2], [0; 2]);
    // SAFETY: X fills these in for the root window.
    unsafe { (x.XGetGeometry)(display, root(x, display), &mut parent, &mut corner[0], &mut corner[1], &mut size[0], &mut size[1], &mut rest[0], &mut rest[1]) };
    size
}

/// A window's kind, fixed when X created it.
struct Kind {
    /// No window-manager frame; the person resizes it by its own edges.
    frameless: bool,
    edges: bool,
    /// See-through; and, where the screen supports it, its 32-bit visual and a graphics context
    /// for it.
    transparent: bool,
    deep: Option<(*mut c_void, *mut c_void)>,
    /// The least size, in pixels.
    min_size: [c_int; 2],
}

/// Has X create an unmapped top-level window `size` pixels big, at least `least`, with its
/// corner `at` a place if given. (Per ICCCM, with a window manager that is the corner of its
/// frame; with none, there is no frame and it is the window's own.) `edges`: no frame, resized
/// by its own edges. `clear`: see-through, so also frameless, created in the screen's 32-bit
/// TrueColor visual where it has one (class 4), with its own colormap as X requires. A
/// see-through window not resized by its edges keeps its size.
///
/// # Safety
/// `display` is open.
unsafe fn new_window(x: &Xlib, display: Display, size: [c_int; 2], least: [c_int; 2], at: Option<(i32, i32)>, [edges, clear]: [bool; 2]) -> (XWindow, Kind) {
    let ((left, top), fixed, among) = (at.unwrap_or_default(), clear && !edges, root(x, display));
    let (least, [wide, high]) = (if fixed { size } else { least }, size.map(|side| side as c_uint));
    let mut hints = hints(least, fixed, at.is_some());
    // SAFETY: Xlib's documented calls to create a window and set its window-manager hints. What
    // is pointed at is read or filled during the calls: plain numbers and a pointer, starting
    // null. (Attributes set: background pixel and border pixel, `CWBackPixel` and
    // `CWBorderPixel`, both 0; and the colormap, `CWColormap`, where it has its own. A
    // see-through window's colormap and GC live with the connection: only a first window can be
    // one, and it lives as long.)
    unsafe {
        let (mut found, mut with): (VisualInfo, Attributes) = (zeroed(), zeroed());
        let deep = clear && (x.XMatchVisualInfo)(display, (x.XDefaultScreen)(display), 32, 4, &mut found) != 0;
        if deep {
            with.colors = (x.XCreateColormap)(display, among, found.visual, 0);
        }
        let (depth, said) = (if deep { 32 } else { 0 }, 1 << 1 | 1 << 3 | (deep as c_ulong) << 13);
        let id = (x.XCreateWindow)(display, among, left, top, wide, high, 0, depth, 1, found.visual, said, &mut with);
        (x.XSetWMNormalHints)(display, id, &mut hints);
        let deep = deep.then(|| (found.visual, (x.XCreateGC)(display, id, 0, null_mut())));
        (id, Kind { frameless: edges || clear, edges, transparent: clear, deep, min_size: least })
    }
}

/// One window on the display, and what keeps an interface shown in it.
struct Shown {
    /// X's id for the window; and for a second window, the first window's interface's id for
    /// it.
    id: XWindow,
    known: Option<Id>,
    /// Its title, the name screen readers get.
    title: String,
    /// Its size in pixels.
    size: [c_int; 2],
    /// Its place and size while not maximized: as last seen and when; and what it restores to
    /// from maximized.
    seen: (WindowPlacement, Instant),
    normal: WindowPlacement,
    /// Whether it is maximized now; and whether minimized.
    filling: bool,
    iconic: bool,
    /// Its kind.
    kind: Kind,
    /// Whether it is fullscreen now; and the place it restores to (left, top, width, height),
    /// when it made itself fullscreen.
    whole: bool,
    back: [c_int; 4],
    /// Something of it is the window's, not the interface's, as of the last run: a resize edge
    /// or a move strip; and which of them the pointer is over.
    grips: bool,
    over: HitTest,
    /// While it moves or resizes itself by the pointer: the held edges (none for its strip),
    /// where on screen the grab started, and where the window lay then.
    held: Option<([i8; 2], [c_int; 2], [c_int; 4])>,
    /// When (X server time) and where on screen its strip was last pressed: a press soon after,
    /// close by, is a double press.
    pressed: Option<(c_ulong, [c_int; 2])>,
    /// How far the window manager's frame corner lies left and up of the window's own, in
    /// pixels.
    frame: [c_int; 2],
    /// X or the window manager reported a place its interface has not been told yet.
    moved: bool,
    /// Its input context, turning key presses into text in the person's language; and the
    /// cursor spot it was last told, in pixels.
    typing: *mut c_void,
    spot: [c_short; 2],
    /// The input method is kept off the window: what is typed now must not be seen.
    hidden: bool,
    /// It announced it accepts drops (see `Drops`); the window dragging something over it (0
    /// for none), where it was last, and what it is taken as (files or text).
    takes: bool,
    drag_over: (XWindow, Point, Atom),
    cursor: Cursor,
    /// Its app hid it entirely to live in the tray ([`Ui::hide_window`]): withdrawn until it
    /// comes forward.
    withdrawn: bool,
    /// What must be redrawn: the part uncovered, and the places the last run changed.
    uncovered: Rect,
    changed: Vec<Rect>,
    /// How it is shown: by X, or through the GPU (feature `gpu`).
    #[cfg(feature = "gpu")]
    way: crate::gpu_x11::Way,
}

/// A window's input context from an input method, with on-the-spot preedit left to the method
/// (`XIMPreeditNothing | XIMStatusNothing`, 0x0408). Nothing where there is no method or it
/// makes none.
///
/// # Safety
/// `method` is an input method of `id`'s display, or null.
unsafe fn typing_by(x: &Xlib, method: *mut c_void, id: XWindow) -> *mut c_void {
    if method.is_null() {
        return null_mut();
    }
    // SAFETY: Xlib's documented call; the names and the list end in zeros.
    unsafe { (x.XCreateIC)(method, c"inputStyle".as_ptr(), 0x0408 as c_long, c"clientWindow".as_ptr(), id, null::<c_void>()) }
}

impl Shown {
    /// Makes a window X just created one of this program's, and maps it: it selects its events,
    /// takes `WM_DELETE_WINDOW`, gets its title and an input context. (It announces drops once
    /// its app asks: see `Drops`.) `opens` is its place and size in units, and whether it is
    /// maximized with that place and size to restore to, set as `_NET_WM_STATE` before mapping.
    /// A frameless `kind` also sets `_MOTIF_WM_HINTS`: flags 2 (decorations given), decorations
    /// 0.
    ///
    /// # Safety
    /// `id` is a window of the connection's display, `size` pixels big; `method` is that
    /// display's input method, or null.
    unsafe fn new(conn: &Connection, method: *mut c_void, (id, kind): (XWindow, Kind), known: Option<Id>, title: &str, size: [c_int; 2], opens: WindowPlacement) -> Self {
        let (x, display, mut close) = (&conn.x, conn.display, conn.atom(c"WM_DELETE_WINDOW"));
        conn.set_window_properties(id, title, kind.frameless, opens.maximized);
        // SAFETY: Xlib's documented calls; every pointer outlives its call.
        let typing = unsafe {
            // Keys, buttons, enter, leave, motion, exposure, structure, focus, property
            // changes.
            (x.XSelectInput)(display, id, 1 | 1 << 1 | 1 << 2 | 1 << 3 | 1 << 4 | 1 << 5 | 1 << 6 | 1 << 15 | 1 << 17 | 1 << 21 | 1 << 22);
            // A program taking touch gets the pointer through XInput 2, with fingers and
            // pinches: see `Host::extended`.
            #[cfg(feature = "touch")]
            conn.inputs.iter().for_each(|inputs| inputs.borrow().select(id));
            (x.XSetWMProtocols)(display, id, &mut close, 1);
            // Each window has its own input context: preedit and the method's candidate window
            // are kept apart per window. (It has focus, as its interface assumes, until X says
            // otherwise.)
            let typing = typing_by(x, method, id);
            if !typing.is_null() {
                (x.XSetICFocus)(typing);
            }
            #[cfg(feature = "gpu")]
            if conn.unfilled && !kind.transparent {
                crate::gpu_x11::unfilled(display, id);
            }
            (x.XMapWindow)(display, id);
            typing
        };
        let (normal, drag_over) = (WindowPlacement { maximized: false, ..opens }, (0, Point::default(), 0));
        Shown {
            id,
            known,
            title: title.into(),
            size,
            seen: (normal, Instant::now()),
            normal,
            filling: opens.maximized,
            iconic: false,
            kind,
            whole: false,
            back: [0; 4],
            grips: false,
            over: HitTest::Client,
            held: None,
            pressed: None,
            frame: [0; 2],
            moved: false,
            typing,
            spot: [0; 2],
            hidden: false,
            takes: false,
            drag_over,
            cursor: Cursor::Arrow,
            withdrawn: false,
            uncovered: Rect::EMPTY,
            changed: Vec::new(),
            #[cfg(feature = "gpu")]
            way: crate::gpu_x11::Way::Unasked,
        }
    }

    /// A second window with the app's title and options (least size, kind, frame): `size`
    /// units, its frame corner `at` a place if given, else where the window manager puts it.
    /// The two declare one group led by the first, and the window manager is told its kind
    /// (`tell_kind`). (A plain one is like the first: moved, resized and stacked as the person
    /// likes.)
    ///
    /// # Safety
    /// `first` is a window of the connection's display, and `method` that display's input
    /// method, or null.
    unsafe fn second(conn: &Connection, method: *mut c_void, first: XWindow, known: Id, title: &str, scale: f32, (size, at, opening): (Size, Option<(i32, i32)>, WindowOpening)) -> Self {
        let (hints, least) = (conn.atom(c"WM_HINTS"), opening.min_size);
        let (big, small) = (pixels(size.w, size.h, scale), pixels(least.w, least.h, scale));
        // SAFETY: the display is open while the connection lives.
        let (id, kind) = unsafe { new_window(&conn.x, conn.display, big, small, at, [opening.frameless, false]) };
        tell_kind(conn, id, first, opening.kind);
        // Of `WM_HINTS`' nine numbers this sets the last, flagged in the first: the window
        // group leader.
        for member in [first, id] {
            conn.leave(member, hints, hints, &[1 << 6, 0, 0, 0, 0, 0, 0, 0, first as c_long]);
        }
        let opens = WindowPlacement { position: at.unwrap_or_default(), width: size.w, height: size.h, maximized: false };
        // SAFETY: the window was just created, this big, on the connection's display.
        unsafe { Shown::new(conn, method, (id, kind), Some(known), title, big, opens) }
    }

    /// Makes the window fullscreen by itself, above all others, or restores it: on a display
    /// with no window manager to do it. (Its current place is what it restores to.)
    fn lie_over(&mut self, conn: &Connection, on: bool) {
        if on == self.whole {
            return;
        }
        let ([left, top], [wide, high]) = (conn.place(self.id), screen_size(&conn.x, conn.display));
        let to = if on { [0, 0, wide as c_int, high as c_int] } else { self.back };
        (self.back, self.whole, self.moved) = ([left, top, self.size[0], self.size[1]], on, true);
        self.normal = if on { self.seen.0 } else { self.normal };
        // SAFETY: the window is alive, on the connection's display.
        unsafe {
            (conn.x.XMoveResizeWindow)(conn.display, self.id, to[0], to[1], to[2] as c_uint, to[3] as c_uint);
            (conn.x.XRaiseWindow)(conn.display, self.id);
        }
    }

    /// Closes the window: its input context, then the window and everything X kept for it
    /// (properties, cursor, selections).
    fn close(self, conn: &Connection) {
        // SAFETY: both were created in `new`, and neither is used after this.
        unsafe {
            if !self.typing.is_null() {
                (conn.x.XDestroyIC)(self.typing);
            }
            (conn.x.XDestroyWindow)(conn.display, self.id);
        }
    }

    /// X says part of the window was uncovered: it must be redrawn.
    fn exposed(&mut self, part: ExposeEvent, scale: f32) {
        self.uncovered = self.uncovered.union(rect(part.x as f32 / scale, part.y as f32 / scale, part.width as f32 / scale, part.height as f32 / scale));
    }

    /// Does something with the interface shown in this window: the first window's own, or a
    /// second window's, lent out by the first meanwhile. (Nothing for a second window the app
    /// stopped describing.)
    fn with<T>(&self, first: &mut Ui, work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
        match self.known {
            Some(known) => first.in_window(known, work),
            None => Some(work(first)),
        }
    }

    /// Tells the input context where the text cursor is, when it moved: an input method puts
    /// its CJK candidate window right under it. (One without such a window takes the spot and
    /// does nothing more.)
    fn follow_typing(&mut self, x: &Xlib, at: Option<Rect>, scale: f32) {
        let Some(at) = at.filter(|_| !self.typing.is_null()) else { return };
        let spot = [(at.x * scale) as c_short, (at.bottom() * scale) as c_short];
        if std::mem::replace(&mut self.spot, spot) == spot {
            return;
        }
        // SAFETY: the list names the spot, which is read before the call taking the list
        // returns; then the list is freed.
        unsafe {
            let list = (x.XVaCreateNestedList)(0, c"spotLocation".as_ptr(), spot.as_ptr(), null::<c_void>());
            (x.XSetICValues)(self.typing, c"preeditAttributes".as_ptr(), list, null::<c_void>());
            (x.XFree)(list);
        }
    }

    /// Changes the pointer's shape over the window when the interface asks.
    fn point(&mut self, conn: &Connection, cursor: Cursor) {
        if std::mem::replace(&mut self.cursor, cursor) == cursor {
            return;
        }
        let shape = match cursor {
            Cursor::Arrow => 68,
            Cursor::Hand => 60,
            Cursor::Text => 152,
            Cursor::ResizeX => 108,
            Cursor::ResizeY => 116,
            Cursor::ResizeDown => 14,
            Cursor::ResizeUp => 12,
            Cursor::Move => 52,
            Cursor::No => 0,
        };
        // SAFETY: one of X's font cursors by number. The window keeps it; the program frees its
        // own reference at once, or X would keep one per change.
        unsafe {
            let pointer = (conn.x.XCreateFontCursor)(conn.display, shape);
            (conn.x.XDefineCursor)(conn.display, self.id, pointer);
            (conn.x.XFreeCursor)(conn.display, pointer);
        }
    }
}

/// The program's D-Bus session: the session bus, the desktop portal reached over it, and the
/// screen readers (whether any listens is said there).
pub(crate) struct Session {
    pub(crate) bus: Option<Bus>,
    pub(crate) portal: Portal,
    pub(crate) readers: Readers,
}

impl Session {
    /// Starts on the session bus, or none: what there is to ask at start is asked at once,
    /// nothing waited for.
    pub(crate) fn on(mut bus: Option<Bus>) -> Self {
        let (portal, readers) = (Portal::new(bus.as_mut()), Readers::new(bus.as_mut()));
        Session { bus, portal, readers }
    }
}

/// All of a window `size` pixels big, in units.
#[cfg(feature = "gpu")]
fn whole(size: [c_int; 2], scale: f32) -> Rect {
    rect(0.0, 0.0, size[0] as f32 / scale, size[1] as f32 / scale)
}

/// A program's windows on an X display, and everything keeping them shown.
struct Host<App> {
    conn: Rc<Connection>,
    app: App,
    /// The first window's interface, which holds the second windows' interfaces: it runs
    /// whichever window something happened in.
    ui: Ui,
    scale: f32,
    /// The open windows: the first, then the second ones.
    windows: Vec<Shown>,
    /// The strip the windows are drawn through in turn; then the visual, depth and GC X draws
    /// it with.
    strip: Vec<u32>,
    screen: (*mut c_void, c_int, *mut c_void),
    /// The input method making each window's input context. Null where X has none for the
    /// person's language.
    method: *mut c_void,
    /// The null input method: Xlib's own, showing and keeping nothing. A window types through
    /// it while what is typed must not be seen. Null until first needed.
    plain: *mut c_void,
    /// Something happened that the interface must run for.
    rerun: bool,
    done: bool,
    /// The pipe end other threads wake the windows through.
    wake: c_int,
    start: Instant,
    /// The D-Bus session; and whether one of its buses has something to say, or room again for
    /// what waits to be sent.
    session: Session,
    heard: bool,
    /// The XSETTINGS manager's window (0 if none runs), and what its settings say the person
    /// asked for.
    settings: XWindow,
    desk: Preferences,
    /// RandR's event base for monitor changes (-1 where there is none).
    monitors_changed: c_int,
    /// What is being dragged out of a window, if anything; the pointer's last place on screen,
    /// time and modifiers; and the windows something was just dropped on, with its source
    /// window, which learn whether it was taken once the interface has run.
    out: Option<Out>,
    last: ([c_int; 2], c_ulong, c_uint),
    finishing: Vec<(XWindow, XWindow)>,
    /// The GPU the windows are shown through (feature `gpu`), where one is asked for and
    /// works; and what to ask of it: nothing once X is to show them.
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu_x11::Gpu>,
    #[cfg(feature = "gpu")]
    asks: Option<crate::gpu_x11::Asked>,
}

/// Something dragged from a window to whatever lies under the pointer, by XDND, as its source
/// keeps it.
struct Out {
    /// The source window, and the types offered.
    from: XWindow,
    kinds: Vec<Atom>,
    /// The XDND-aware window under it now (0 for none), the window its messages go to (itself
    /// or a proxy), and the protocol version both speak. And whether it is over its own window.
    over: XWindow,
    to: XWindow,
    version: c_long,
    home: bool,
    /// What that window said it would do on a drop; nothing if it refused.
    takes: Option<Atom>,
    /// That window was sent a position and has not replied; where the pointer went meanwhile,
    /// sent once it has; and when the button came up, if it has.
    asked: bool,
    next: Option<([c_int; 2], c_ulong)>,
    let_go: Option<c_ulong>,
    /// Dropped over that window, which has yet to report what it did.
    dropped: bool,
    /// Until when a reply is awaited.
    until: Option<Instant>,
    /// Shift was held as it left: a move, not a copy.
    moves: bool,
}

impl Out {
    /// The pointer's shape while this is dragged from window `id` over another: move arrows
    /// over one that accepts it, the no-drop shape over any other. (Nothing while over its own
    /// window, where its interface decides, or when dragged from another.)
    fn cursor(&self, id: XWindow) -> Option<Cursor> {
        let over = self.from == id && !self.home && self.let_go.is_none();
        over.then_some(if self.takes.is_some() { Cursor::Move } else { Cursor::No })
    }
}

impl<App: FnMut(&mut Ui)> Host<App> {
    /// Opens the first window; nothing if there is no display.
    fn open(x: Xlib, text: SystemText, session: Session, window: Window, app: App) -> Option<Self> {
        // SAFETY: Xlib's documented calls to create a window; every pointer outlives its call.
        unsafe {
            let display = (x.XOpenDisplay)(null());
            if display.is_null() {
                return None;
            }
            // A held key then repeats only its press, its release coming once at the end, so
            // the interface tells a held key from released ones.
            (x.XkbSetDetectableAutoRepeat)(display, 1, null_mut());
            let (number, scale) = ((x.XDefaultScreen)(display), scale_of(&x, display));
            let (size, least) = (pixels(window.width, window.height, scale), pixels(window.min_width, window.min_height, scale));
            // What is copied in any window is the first window's to hand out: it lives as long
            // as the program. (Created below.)
            let mut conn = Connection::new(x, display, 0, c"CLIPBOARD");
            #[cfg(feature = "gpu")]
            let asks = crate::gpu_x11::asked(window.renderer);
            #[cfg(feature = "gpu")]
            {
                conn.unfilled = asks.is_some();
            }
            // Monitors and their changes: RandR's screen, output and CRTC change events (1 | 2
            // | 4); and the root window's `StructureNotify` (1 << 17), reconfigured when they
            // change, and `PropertyChange` (1 << 22), where work areas are given.
            let (mut news, mut failed) = (-1, 0);
            conn.randr = Xrandr::open().filter(|randr| (randr.XRRQueryExtension)(display, &mut news, &mut failed) != 0);
            conn.randr.iter().for_each(|randr| (randr.XRRSelectInput)(display, root(&conn.x, display), 1 | 2 | 4));
            (conn.x.XSelectInput)(display, root(&conn.x, display), 1 << 17 | 1 << 22);
            // In a program taking touch, the pointer, fingers, pens and touchpad pinches
            // through XInput 2, with the atoms of a pen's axes (pressure, tilt).
            #[cfg(feature = "touch")]
            {
                let labels = [c"Abs Pressure", c"Abs Tilt X", c"Abs Tilt Y"].map(|name| conn.atom(name));
                conn.inputs = crate::xinput::Inputs::open(display, root(&conn.x, display), labels).map(RefCell::new);
            }
            // Where it goes: where its app says (on the screen with that place, else moved onto
            // the nearest), or centred on the screen it names; else where the window manager
            // puts it.
            let found = screens(&conn, scale);
            let asked = window.position.map(|(x, y)| OpenAt::At(x, y)).or(window.screen.map(OpenAt::Middle)).unwrap_or_default();
            let at = window_origin(&found, asked, |_| size.map(|side| side as f32)).map(|(_, at)| (at.x as i32, at.y as i32));
            let (id, kind) = new_window(&conn.x, display, size, least, at, [window.frameless, window.transparent]);
            (conn.window, conn.icons) = (id, pictured(window.icons));
            crate::print::set_parent(format!("x11:{id:x}"));
            let conn = Rc::new(conn);
            let x = &conn.x;
            // Only a window manager maximizes a window: with none, the window opens at its size
            // and is not reported maximized.
            let opens = WindowPlacement { position: at.unwrap_or_default(), width: window.width, height: window.height, maximized: window.maximized && conn.managed() };
            // A window opening minimized says so before mapping: `WM_HINTS`' third number is
            // the initial state (3: iconic), flagged in the first.
            if window.minimized {
                conn.leave(id, conn.atom(c"WM_HINTS"), conn.atom(c"WM_HINTS"), &[2 as c_long, 0, 3, 0, 0, 0, 0, 0, 0]);
            }
            // The root window's `PropertyChange` (1 << 22) reports changes to XSETTINGS.
            let settings = conn.settings();
            if settings != 0 {
                (x.XSelectInput)(display, settings, 1 << 22);
            }
            let desk = conn.desk(settings);
            let method = open_method(x, display, c"");
            let (windows, start) = (vec![Shown::new(&conn, method, (id, kind), None, window.title, size, opens)], Instant::now());
            let screen = ((x.XDefaultVisual)(display, number), (x.XDefaultDepth)(display, number), (x.XDefaultGC)(display, number));
            let [wake, _] = pipe_ends();
            // The interface is made last, with nothing left to ask, so it is made in place and
            // not moved there piece by piece.
            let mut ui = Ui::new(Box::new(text), Box::new(SystemClipboard(conn.clone())));
            // A see-through window opens on a page of no colour: what is behind shows wherever
            // its app draws nothing.
            if window.transparent {
                ui.theme.bg = Color::NONE;
            }
            ui.set_screens(found);
            // What the session bus said by the first turn is taken in before the interface
            // first runs, so `heard` starts true.
            let monitors_changed = if conn.randr.is_some() { news } else { -1 };
            Some(Host {
                conn,
                app,
                ui,
                scale,
                windows,
                strip: Vec::new(),
                screen,
                method,
                plain: null_mut(),
                rerun: true,
                done: false,
                wake,
                start,
                session,
                heard: true,
                settings,
                desk,
                monitors_changed,
                out: None,
                last: ([0; 2], 0, 0),
                finishing: Vec::new(),
                #[cfg(feature = "gpu")]
                gpu: None,
                #[cfg(feature = "gpu")]
                asks,
            })
        }
    }

    /// One turn: takes in what happened, runs the interface if needed, and draws what changed.
    fn turn(&mut self) {
        self.ui.set_time(self.start.elapsed());
        self.rerun |= self.ui.wake_in() == Some(Duration::ZERO); // a blinking cursor, a tooltip that is due
        // The portal is told the time too: one that never answers a file-dialog request is
        // given up on, and the window gets the library's own dialog.
        let Session { bus, portal, .. } = &mut self.session;
        portal.now = self.start.elapsed();
        if portal.give_up(bus.as_mut()) {
            self.heed();
        }
        // Everything queued is taken in before the interface runs once for all of it.
        // SAFETY: the display is open while the host lives.
        while unsafe { (self.conn.x.XPending)(self.conn.display) } > 0 {
            self.take();
        }
        self.out_waited();
        if std::mem::take(&mut self.heard) {
            self.hear_session();
            self.hear_readers();
        }
        (0..self.windows.len()).for_each(|at| self.place(at));
        if self.rerun {
            self.describe();
        }
        if let Some(drops) = DROPS.get() {
            (drops.ran)(self.taking());
        }
        #[cfg(feature = "gpu")]
        self.mend();
        (0..self.windows.len()).for_each(|at| self.draw(at));
        // A device lost while drawing: the windows are drawn again, whole, through a new one or
        // by X.
        #[cfg(feature = "gpu")]
        if self.mend() {
            (0..self.windows.len()).for_each(|at| self.draw(at));
        }
    }

    /// Takes in one X event.
    fn take(&mut self) {
        let (conn, scale) = (self.conn.clone(), self.scale);
        // SAFETY: X fills the event in. Its fields are plain numbers, so reading any variant is
        // harmless; its type says which means something.
        let mut event: XEvent = unsafe { zeroed() };
        let composing = unsafe { (conn.x.XNextEvent)(conn.display, &mut event) + (conn.x.XFilterEvent)(&mut event, 0) } != 0;
        #[cfg(feature = "touch")]
        if self.extended(&mut event) {
            return;
        }
        let (kind, input, expose, resized, news) = unsafe { (event.kind, event.input, event.expose, event.configure, event.property) };
        // Which window it is about: every event names it in the same place. (None for another
        // program's window, or a second window closed since.)
        let at = self.windows.iter().position(|shown| shown.id == news.window);
        match (kind, at) {
            _ if composing => {} // an input method is composing a character
            // A window blocked by a dialog hears neither keys nor pointer.
            (2 | 4 | 5 | 6 | 7, Some(at)) if self.blocked(at) => self.kept_from(at, kind == 4),
            (2, Some(at)) => self.key(at, &mut event),
            (3, Some(at)) => self.key_up(at, &mut event),
            (4 | 5, Some(at)) => self.button(at, input, kind == 4),
            // The pointer is also where it is when a window comes under it without it moving:
            // when the window over this one just closed.
            (6 | 7, Some(at)) => self.pointer(at, input),
            (8, Some(at)) => self.left(at),
            (9 | 10, Some(at)) => self.focus(at, kind == 9),
            (12, Some(at)) => self.windows[at].exposed(expose, scale),
            // Mapped (19) or reparented into a frame by the window manager (21): only now is it
            // where it stays.
            (19 | 21, Some(at)) => self.windows[at].moved = true,
            (22, Some(at)) => self.sized(at, [resized.width.max(1), resized.height.max(1)]),
            (28, Some(at)) => self.noted(at, news.atom),
            // SAFETY: as above: a type-33 event is a client message, a type-30 one (below) a
            // selection request, delivered on this display.
            (33, Some(at)) => self.client_message(at, unsafe { event.client }),
            // The XSETTINGS changed: the theme may have.
            (28, None) if news.window == self.settings => self.settings_changed(),
            // The work areas are given on the root window, per desktop.
            (28, None) if news.window == root(&conn.x, conn.display) && [c"_NET_WORKAREA", c"_NET_CURRENT_DESKTOP"].map(|name| conn.atom(name)).contains(&news.atom) => self.screens_changed(),
            // RandR reports the screen and its outputs; and the root window is reconfigured
            // when its monitors change.
            (kind, _) if (self.monitors_changed..self.monitors_changed + 2).contains(&kind) => self.screens_changed(),
            (22, None) if news.window == root(&conn.x, conn.display) => self.screens_changed(),
            // X also reports on a window a copy goes to in pieces: it took a piece, or is gone.
            (17 | 28, _) => conn.hand_on(news, kind == 17),
            (29, _) => conn.lost(news.atom),
            (30, _) => unsafe { conn.answer(event.request) },
            // A global hotkey: X delivers it on the root window.
            (2, None) => self.rerun |= DESKTOP.get().is_some_and(|desktop| (desktop.pressed)(&conn, &input)),
            _ => {}
        }
    }

    /// An XInput 2 event (generic event type 35, see `xinput`), in a program taking touch. A
    /// pointer event is converted to X's core layout once its pen axes are read; finger and
    /// pinch events go to the window they are in. (A window blocked by a dialog hears neither:
    /// a finger landing on it raises the dialog, as a press does.) Says whether that was all
    /// there is to the event.
    #[cfg(feature = "touch")]
    fn extended(&mut self, event: &mut XEvent) -> bool {
        let conn = self.conn.clone();
        // SAFETY: every event starts with its type; a type-35 one has a cookie's layout and was
        // delivered on this display.
        let heard = match (unsafe { event.kind }, &conn.inputs) {
            (35, Some(inputs)) => unsafe { inputs.borrow_mut().read(&mut event.cookie, self.scale) },
            _ => return false,
        };
        let (window, said, taken) = match heard {
            crate::xinput::Heard::Nothing => return true,
            crate::xinput::Heard::Said(window, said) => (window, Some(said), true),
            crate::xinput::Heard::Pointer(news, p) => {
                event.input = InputEvent::of(p, conn.display);
                (p.window, news, false)
            }
        };
        let Some(at) = self.windows.iter().position(|shown| shown.id == window) else { return taken };
        match said {
            Some(said) if !self.blocked(at) => self.hear(at, said),
            Some(Event::Touch(crate::touch::Touch { phase: crate::touch::TouchPhase::Down, .. })) => self.kept_from(at, true),
            _ => {}
        }
        taken
    }

    /// X says a window was moved or resized, by the person or the window manager: its size in
    /// pixels now. (Its place is queried once all news is in: the one given here is relative to
    /// the parent, right only for a frameless window.)
    fn sized(&mut self, at: usize, size: [c_int; 2]) {
        let shown = &mut self.windows[at];
        (shown.moved, self.rerun) = (true, self.rerun || size != shown.size);
        shown.size = size;
    }

    /// A property was written to or deleted from a window. The window manager's frame extents
    /// (`_NET_FRAME_EXTENTS`) or the window state change where the window is for its interface;
    /// a state that says minimized ("hidden"), or no longer does, is passed to the interface.
    fn noted(&mut self, at: usize, name: Atom) {
        let (conn, shown) = (&self.conn, &mut self.windows[at]);
        let mut iconic = shown.iconic;
        if name == conn.atom(c"_NET_FRAME_EXTENTS") {
            let frame = conn.numbers(shown.id, c"_NET_FRAME_EXTENTS");
            shown.frame = [0, 2].map(|side| frame.get(side).map_or(0, |wide| *wide as c_int));
        } else if name == conn.atom(c"_NET_WM_STATE") {
            let state = conn.numbers(shown.id, c"_NET_WM_STATE");
            [shown.filling, shown.whole] = stands(&state, [FILLING[0], FILLING[1], WHOLE].map(|name| conn.atom(name)));
            iconic = state.contains(&conn.atom(c"_NET_WM_STATE_HIDDEN"));
        } else {
            return;
        }
        shown.moved = true;
        if std::mem::replace(&mut shown.iconic, iconic) != iconic {
            self.hear(at, Event::Minimized(iconic));
        }
    }

    /// Takes in where a window is now, if X or the window manager reported a change since its
    /// interface was last told, and tells the interface: its frame corner on screen (its own
    /// corner without a frame), its client size, and whether maximized; a maximized one with
    /// the place and size it restores to. If any of that is news, the interface runs for the
    /// app to hear.
    fn place(&mut self, at: usize) {
        let (conn, scale, shown) = (&self.conn, self.scale, &mut self.windows[at]);
        if !std::mem::take(&mut shown.moved) {
            return;
        }
        let ([x, y], [left, up], (seen, since)) = (conn.place(shown.id), shown.frame, shown.seen);
        let here = WindowPlacement { position: (x.saturating_sub(left), y.saturating_sub(up)), width: shown.size[0] as f32 / scale, height: shown.size[1] as f32 / scale, maximized: false };
        // Where it was last seen while not maximized is what it restores to, once it has stayed
        // there a while (see `SETTLED`); and what was seen just before it was maximized is
        // forgotten.
        if since.elapsed() > SETTLED {
            shown.normal = seen;
        }
        // Seen again where it was, it has stood there since it was first seen there.
        let (away, whole, stood) = (shown.filling || shown.whole, shown.whole, if here == seen { since } else { Instant::now() });
        shown.seen = if away { (shown.normal, since) } else { (here, stood) };
        let now = if away { WindowPlacement { maximized: shown.filling, ..shown.normal } } else { here };
        self.rerun |= self.windows[at].with(&mut self.ui, |ui| ui.set_window_placement(now) | ui.full_screen_now(whole)) == Some(true);
    }

    /// Passes what the person did in a window to its interface, noting whether it must run.
    fn hear(&mut self, at: usize, event: Event) {
        self.rerun |= self.windows[at].with(&mut self.ui, |ui| ui.event(event)) == Some(true);
    }

    /// What of a window the pointer grabs at a place: the interface is asked if the window has
    /// parts of its own to grab. (No edge resizes a maximized window.)
    fn grip(&mut self, at: usize, place: Point) -> HitTest {
        let shown = &self.windows[at];
        let sized = shown.kind.edges && !shown.filling;
        if !shown.grips {
            return HitTest::Client;
        }
        shown.with(&mut self.ui, |ui| ui.hit_test(place, sized)).unwrap_or_default()
    }

    /// The pointer is somewhere in a window. A window moving or resizing itself follows it.
    /// Else its interface hears where the pointer is; but on the window's strip or a resize
    /// edge, the pointer has left the interface and takes that edge's shape.
    fn pointer(&mut self, at: usize, e: InputEvent) {
        if self.windows[at].held.is_some() {
            return self.drag(at, e);
        }
        self.last = ([e.x_root, e.y_root], e.time, e.state);
        let place = pt(e.x as f32 / self.scale, e.y as f32 / self.scale);
        let grip = self.grip(at, place);
        // While what the pointer drags is outside the window, or leaves as soon as the pointer
        // does, the pointer over something else has left it.
        let away = OUTWARD.get().is_some_and(|outward| (outward.moved)(self.outgoing(), at, [e.x_root, e.y_root], e.time));
        self.hear(at, if grip == HitTest::Client && !away { Event::Move(place) } else { Event::Leave });
        if std::mem::replace(&mut self.windows[at].over, grip) != grip {
            self.shape(at);
        }
    }

    /// The pointer left a window: its interface hears, and no edge has the pointer. (A nearby
    /// pen left with it: X says nothing more of it.)
    fn left(&mut self, at: usize) {
        #[cfg(feature = "touch")]
        if let Some(gone) = self.conn.inputs.as_ref().and_then(|inputs| inputs.borrow_mut().pen_gone()) {
            self.hear(at, gone);
        }
        self.hear(at, Event::Leave);
        if std::mem::take(&mut self.windows[at].over) != HitTest::Client {
            self.shape(at);
        }
    }

    /// Gives the pointer its shape over a window now: its edge's, or the one the interface
    /// asked for.
    fn shape(&mut self, at: usize) {
        let asked = self.windows[at].with(&mut self.ui, |ui| ui.cursor()).unwrap_or_default();
        let (conn, shown) = (&self.conn, &mut self.windows[at]);
        shown.point(conn, shown.over.cursor().unwrap_or(asked));
    }

    /// The left button went down in a window. On its strip or a resize edge it grabs the
    /// window, and this says so: the press is not the interface's.
    ///
    /// The window manager then gets the window to move or resize by the pointer
    /// (`_NET_WM_MOVERESIZE`), the implicit pointer grab released for that: given the press's
    /// screen place, the direction (the eight edges and corners clockwise from the top left, or
    /// 8 to move), button 1 and source 1 (a person). With no window manager, or one that does
    /// not support it, the window follows the pointer itself (see `drag`) from where it lies.
    ///
    /// A press on the strip soon after another, close by, is a double press: the window manager
    /// is asked to maximize or restore the window. (Not for a window keeping its creation
    /// size.)
    fn take_hold(&mut self, at: usize, e: InputEvent) -> bool {
        let grip = self.grip(at, pt(e.x as f32 / self.scale, e.y as f32 / self.scale));
        let (conn, shown) = (self.conn.clone(), &mut self.windows[at]);
        let [across, down] = match grip {
            HitTest::Client => return false,
            HitTest::TitleBar => [0, 0],
            HitTest::Edge(across, down) => [across, down],
        };
        let (before, [wide, high]) = (shown.pressed.replace((e.time, [e.x_root, e.y_root])), FILLING.map(|name| conn.atom(name) as c_long));
        let again = grip == HitTest::TitleBar && before.is_some_and(|(then, [x, y])| e.time.wrapping_sub(then) < DOUBLE_PRESS && (e.x_root - x).abs() < 4 && (e.y_root - y).abs() < 4);
        if again && (shown.kind.edges || !shown.kind.transparent) {
            conn.ask(shown.id, c"_NET_WM_STATE", [!shown.filling as c_long, wide, high, 1, 0]);
        }
        if again {
            shown.pressed = None;
            return true;
        }
        if conn.offers(c"_NET_WM_MOVERESIZE") {
            let way = [0, 1, 2, 7, 8, 3, 6, 5, 4][(down * 3 + across + 4) as usize];
            // SAFETY: the display is open. (0: `CurrentTime`.)
            unsafe { (conn.x.XUngrabPointer)(conn.display, 0) };
            conn.ask(shown.id, c"_NET_WM_MOVERESIZE", [e.x_root as c_long, e.y_root as c_long, way, 1, 1]);
        } else {
            let [left, top] = conn.place(shown.id);
            shown.held = Some(([across, down], [e.x_root, e.y_root], [left, top, shown.size[0], shown.size[1]]));
        }
        true
    }

    /// The pointer moved while a window is held and follows it itself: the window is moved
    /// there.
    fn drag(&mut self, at: usize, e: InputEvent) {
        let (conn, shown) = (&self.conn, &self.windows[at]);
        let Some((edges, from, was)) = shown.held else { return };
        let [left, top, wide, high] = dragged(edges, was, [e.x_root - from[0], e.y_root - from[1]], shown.kind.min_size);
        // SAFETY: the window is the host's own, on its display.
        unsafe { (conn.x.XMoveResizeWindow)(conn.display, shown.id, left, top, wide as c_uint, high as c_uint) };
    }

    /// A mouse button went down or up where the pointer is. (The wheel comes as buttons 4 to 7;
    /// with Shift it scrolls sideways.) The left button down on a window's strip or edge grabs
    /// the window, and up releases a self-moving window: neither is the interface's. Up while
    /// dragging something out of the window drops it there: over another window, the interface
    /// hears the pointer is not over its own.
    fn button(&mut self, at: usize, e: InputEvent, down: bool) {
        if e.detail == 1 && if down { self.take_hold(at, e) } else { self.windows[at].held.take().is_some() } {
            return;
        }
        self.last = ([e.x_root, e.y_root], e.time, e.state);
        let away = self.out.as_ref().is_some_and(|out| out.from == self.windows[at].id && !out.home);
        self.hear(at, if away { Event::Leave } else { Event::Move(pt(e.x as f32 / self.scale, e.y as f32 / self.scale)) });
        if let Some(outward) = OUTWARD.get().filter(|_| e.detail == 1 && !down) {
            self.rerun |= (outward.released)(self.outgoing(), at, e.time);
        }
        let mods = held(e.state);
        let event = match (e.detail, down) {
            (1, true) => Event::Press(Button::Left, mods),
            (2, true) => Event::Press(Button::Middle, mods),
            (3, true) => Event::Press(Button::Right, mods),
            (1, false) => Event::Release(Button::Left),
            (2, false) => Event::Release(Button::Middle),
            (3, false) => Event::Release(Button::Right),
            // With Ctrl it zooms, a quarter per notch.
            (4 | 5, true) if mods.ctrl => Event::Zoom(crate::touch::wheel_zoom(if e.detail == 4 { 1.0 } else { -1.0 })),
            (4 | 5, true) if !mods.shift => Event::Wheel(0.0, if e.detail == 4 { -48.0 } else { 48.0 }),
            (4..=7, true) => Event::Wheel(if e.detail.is_multiple_of(2) { -48.0 } else { 48.0 }, 0.0),
            _ => return,
        };
        self.hear(at, event);
    }

    /// A key went down in a window: which key, and what it types. (Escape while dragging
    /// something out with the button held cancels it: the interface hears the button came up
    /// nowhere.)
    fn key(&mut self, at: usize, event: &mut XEvent) {
        // SAFETY: X reads the event it delivered.
        let (mods, name) = unsafe { (held(event.input.state), (self.conn.x.XLookupKeysym)(event, 0)) };
        if key(name) == Some(Key::Escape) && self.out.as_ref().is_some_and(|out| out.from == self.windows[at].id && out.let_go.is_none()) {
            self.give_up_out();
            self.hear(at, Event::Leave);
            return self.hear(at, Event::Release(Button::Left));
        }
        // What an input method commits at once can exceed any buffer: it then says how long,
        // and is asked again.
        let mut room = vec![0; 64];
        let mut len = self.typed(at, event, &mut room);
        if len > room.len() {
            room.resize(len, 0);
            len = self.typed(at, event, &mut room);
        }
        if let Some(held) = held_after(name, event_state(event), true) {
            self.hear(at, Event::ModifiersChanged(held));
        }
        if let Some(key) = key(name) {
            self.hear(at, Event::Key(key, mods));
        }
        let text = std::str::from_utf8(room.get(..len).unwrap_or_default()).unwrap_or_default();
        if !text.is_empty() && !mods.ctrl && text.chars().all(|c| !c.is_control()) {
            self.hear(at, Event::Text(text.to_string()));
        }
    }

    /// A key came up in a window: for modifiers, what is held now; for any other, which it was.
    fn key_up(&mut self, at: usize, event: &mut XEvent) {
        // SAFETY: X reads the event it delivered.
        let name = unsafe { (self.conn.x.XLookupKeysym)(event, 0) };
        if let Some(held) = held_after(name, event_state(event), false) {
            self.hear(at, Event::ModifiersChanged(held));
        }
        if let Some(key) = key(name) {
            self.hear(at, Event::KeyUp(key));
        }
    }

    /// Has X write what a key press types into `room`. Gives back its length in bytes: more
    /// than `room` when it did not fit and nothing was written.
    fn typed(&self, at: usize, event: &mut XEvent, room: &mut [u8]) -> usize {
        let (typing, mut sym, mut status) = (self.windows[at].typing, 0, 0);
        if typing.is_null() {
            return 0;
        }
        // SAFETY: X reads the event it delivered and writes no more than the room given.
        unsafe { (self.conn.x.Xutf8LookupString)(typing, event, room.as_mut_ptr().cast(), room.len() as c_int, &mut sym, &mut status).max(0) as usize }
    }

    /// A window gained or lost focus: its interface is told, and so is its input context, as X
    /// requires. (An input method shows its own windows by it.)
    fn focus(&mut self, at: usize, on: bool) {
        self.hear(at, Event::Focused(on));
        let (x, typing) = (&self.conn.x, self.windows[at].typing);
        if typing.is_null() {
            return;
        }
        let tell = if on { x.XSetICFocus } else { x.XUnsetICFocus };
        // SAFETY: the input context was made for this window and lives as long.
        unsafe { tell(typing) };
    }

    /// Another program's client message to a window: the window manager's `WM_DELETE_WINDOW`;
    /// XDND replies from a program something dragged from here is over (status, finished); or
    /// XDND messages from a program dragging something here (enter, position, leave, drop),
    /// which a window hears once its app asked about drops (see `Drops`).
    fn client_message(&mut self, at: usize, told: ClientEvent) {
        let conn = self.conn.clone();
        // The first number says what to do, or which window it comes from.
        let (named, from) = (|name: &CStr| told.message == conn.atom(name), told.data[0] as XWindow);
        if told.format != 32 || from == 0 {
            return;
        }
        if named(c"WM_PROTOCOLS") && from == conn.atom(c"WM_DELETE_WINDOW") {
            self.closed(at);
        } else if let Some(outward) = OUTWARD.get().filter(|_| named(c"XdndStatus") || named(c"XdndFinished")) {
            self.rerun |= (outward.reply)(self.outgoing(), from, named(c"XdndFinished"), told.data);
        } else if let Some(drops) = DROPS.get() {
            self.rerun |= (drops.xdnd_message)(self.taking(), at, &told);
        }
    }

    /// What taking in drops works with (see `Taking`).
    fn taking(&mut self) -> Taking<'_> {
        Taking { conn: &self.conn, ui: &mut self.ui, windows: &mut self.windows, scale: self.scale, finishing: &mut self.finishing, rerun: false }
    }

    /// The person closed a window. Its app hears in the next run, due now: after it the first
    /// window goes and the program ends, unless the app says no; a second one goes when the app
    /// stops describing it, unless it says no.
    fn closed(&mut self, at: usize) {
        let Some(known) = self.windows[at].known else {
            self.ui.request_close();
            return self.rerun = true;
        };
        self.rerun |= self.ui.in_window(known, Ui::request_close).is_some();
    }

    /// Whether window `at` takes neither keys nor pointer now: a dialog opened after it is
    /// open.
    fn blocked(&self, at: usize) -> bool {
        self.ui.window_blocked(self.windows[at].known.unwrap_or(0))
    }

    /// Pointer or key input in a window blocked by a dialog: its interface hears the pointer
    /// left, and a `press` raises the dialog (asking the window manager, or itself where there
    /// is none).
    fn kept_from(&mut self, at: usize, press: bool) {
        self.hear(at, Event::Leave);
        let opens = |shown: &&Shown| shown.known.and_then(|known| self.ui.window_opening(known));
        let dialog = self.windows.iter().rev().find(|shown| opens(shown).is_some_and(|opening| opening.kind == WindowKind::Dialog));
        let Some(dialog) = dialog.map(|shown| shown.id).filter(|_| press) else { return };
        self.conn.ask(dialog, c"_NET_ACTIVE_WINDOW", [1, 0, 0, 0, 0]);
        // SAFETY: the window is the host's own, on its display.
        unsafe { (self.conn.x.XRaiseWindow)(self.conn.display, dialog) };
    }

    /// The monitors or a work area changed: the interface is told, and runs for the app to hear
    /// if that is news.
    fn screens_changed(&mut self) {
        self.rerun |= self.ui.set_screens(screens(&self.conn, self.scale));
    }

    /// Where a new second window's frame corner goes, its client being `size`: at its place or
    /// centred on a screen if the app says; a dialog centred on the first window. Nothing where
    /// the window manager places it. (A frame's width is unknown until the window manager adds
    /// it: the first window's is assumed.)
    fn place_for(&self, size: Size, opening: WindowOpening) -> Option<(i32, i32)> {
        let (first, scale) = (&self.windows[0], self.scale);
        let ([left, top], [x, y]) = (first.frame, self.conn.place(first.id));
        let over = rect((x - left) as f32, (y - top) as f32, (first.size[0] + 2 * left) as f32, (first.size[1] + top + left) as f32);
        let asked = match (opening.position, opening.screen, opening.kind) {
            (Some((x, y)), ..) => OpenAt::At(x, y),
            (None, Some(nth), _) => OpenAt::Middle(nth),
            (None, None, WindowKind::Dialog) => OpenAt::Over(over),
            _ => return None,
        };
        let [left, top] = if opening.frameless { [0, 0] } else { first.frame };
        let big = [size.w * scale + (2 * left) as f32, size.h * scale + (top + left) as f32];
        window_origin(self.ui.screens(), asked, |_| big).map(|(_, at)| (at.x as i32, at.y as i32))
    }

    // ---- dragging out of a window (how: `Outward`) ----

    /// What dragging out of the windows works with (see `Outgoing`).
    fn outgoing(&mut self) -> Outgoing<'_> {
        Outgoing { conn: &self.conn, ui: &mut self.ui, windows: &mut self.windows, out: &mut self.out, last: self.last, rerun: false }
    }

    /// Cancels what is being dragged out of a window, if anything.
    fn give_up_out(&mut self) {
        if let Some(outward) = OUTWARD.get() {
            self.rerun |= (outward.given_up)(self.outgoing());
        }
    }

    /// Cancels the drag once the reply timeout passes.
    fn out_waited(&mut self) {
        if self.out.as_ref().and_then(|out| out.until).is_some_and(|until| Instant::now() >= until) {
            self.give_up_out();
        }
    }

    /// Runs the interface: the first window's, which describes the second ones too. Each is
    /// told its window size first; after, the windows are brought in line with what was
    /// described.
    fn describe(&mut self) {
        let scale = self.scale;
        for shown in &self.windows {
            let size = Size { w: shown.size[0] as f32 / scale, h: shown.size[1] as f32 / scale };
            shown.with(&mut self.ui, |ui| ui.resize(size, scale));
        }
        self.ui.run(&mut self.app);
        // The person asked the first window to close, and the app did not object.
        let closes = self.ui.take_closing() == Some(Ok(()));
        (self.rerun, self.done) = (false, self.done || closes || self.ui.quitting());
        // What the app asked of the desktop beyond its windows, once it asked any.
        if let Some(desktop) = DESKTOP.get() {
            self.rerun |= (desktop.stood)(&self.conn, &mut self.windows[0], &mut self.ui, &mut self.session);
        }
        self.keep_up();
        (0..self.windows.len()).for_each(|at| self.follow(at));
        if let Some(outward) = OUTWARD.get() {
            self.rerun |= (outward.ran)(self.outgoing());
        }
        if self.ui.listening {
            self.tell_readers();
        }
    }

    /// The session bus has messages: each goes to the portal and to the screen readers, each
    /// knowing its own. A bus that is gone is dropped, and the portal with it. Then what the
    /// portal said is taken in.
    fn hear_session(&mut self) {
        let (Session { bus, portal, readers }, desktop, rerun) = (&mut self.session, DESKTOP.get(), &mut self.rerun);
        let hear = &mut |bus: &mut Bus, message: &Message| {
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

    /// XSETTINGS changed: what they say the person asked for is read again, and the windows
    /// told what follows.
    fn settings_changed(&mut self) {
        self.desk = self.conn.desk(self.settings);
        self.heed();
    }

    /// Takes in what the desktop said: the first window's interface gets the person's
    /// preferences (from the portal, else XSETTINGS) and whether the desktop has a file dialog;
    /// each window whose file dialog ended gets the answer. The interface runs for what is
    /// news.
    fn heed(&mut self) {
        let portal = &mut self.session.portal;
        self.rerun |= self.ui.set_preferences(portal.preferences(self.desk));
        self.rerun |= crate::locale::set_desktop_clock(portal.twelve_hour_clock);
        self.ui.set_native_file_dialog(portal.files);
        for (known, file) in std::mem::take(&mut portal.answers) {
            let shown = self.windows.iter().find(|shown| shown.known.unwrap_or(0) == known);
            self.rerun |= shown.and_then(|shown| shown.with(&mut self.ui, |ui| ui.file_dialog_answered(file))).is_some();
        }
    }

    /// A window's app asked for the system file dialog: the portal is asked to show it over the
    /// window. The answer comes over the bus once the person gives one; meanwhile the window
    /// works and draws as usual. (If the portal vanished since the interface last heard of it,
    /// the request is answered at once.)
    fn ask_files(&mut self, at: usize, ask: FileDialogRequest) {
        let (shown, Session { bus, portal, .. }) = (&self.windows[at], &mut self.session);
        portal.ask_for_files(bus.as_mut(), &format!("x11:{:x}", shown.id), shown.known.unwrap_or(0), &ask);
        self.heed();
    }

    /// After a run a screen reader listens to: each window's content is handed on, and change
    /// events go out. (A slider a screen reader set moves a step per run: for that, the
    /// interface runs again.)
    fn tell_readers(&mut self) {
        for shown in &self.windows {
            let told = shown.with(&mut self.ui, |ui| self.session.readers.keep(ui, shown.known.unwrap_or(0), &shown.title, shown.size, self.scale));
            self.rerun |= told == Some(true);
        }
        self.session.readers.settle();
    }

    /// The screen readers' bus has messages, or whether any listens was just said on the
    /// session bus. Queries about the windows are answered; actions a screen reader asked for
    /// are done to that window's interface; and when one starts or stops listening, the
    /// interfaces start or stop keeping its tree.
    fn hear_readers(&mut self) {
        let (conn, readers) = (self.conn.clone(), &mut self.session.readers);
        let place = |known: Id| self.windows.iter().find(|shown| shown.known.unwrap_or(0) == known).map_or([0; 2], |shown| conn.place(shown.id));
        for deed in readers.turn(&place) {
            let shown = self.windows.iter().find(|shown| shown.known.unwrap_or(0) == deed.window);
            shown.and_then(|shown| shown.with(&mut self.ui, |ui| readers.act(ui, deed)));
            self.rerun = true;
        }
        let listening = readers.listening();
        if listening != self.ui.listening {
            for shown in &self.windows {
                shown.with(&mut self.ui, |ui| ui.set_screen_reader(listening));
            }
            self.rerun = true;
        }
    }

    /// After a run, the windows are made to match the interface: a second window the app
    /// stopped describing is closed (with any portal file dialog up for it), and one described
    /// for the first time is opened, with its app's options.
    fn keep_up(&mut self) {
        let (scale, first, Session { bus, portal, .. }) = (self.scale, self.windows[0].id, &mut self.session);
        for at in (1..self.windows.len()).rev() {
            if !self.ui.windows().any(|(known, ..)| self.windows[at].known == Some(known)) {
                portal.forget(bus.as_mut(), self.windows[at].known.unwrap_or(0));
                #[cfg(feature = "gpu")]
                crate::gpu_x11::unshow(&mut self.gpu, &mut self.windows[at].way);
                self.windows.remove(at).close(&self.conn);
            }
        }
        for (known, title, size) in self.ui.windows() {
            if self.windows.iter().all(|shown| shown.known != Some(known)) {
                let opening = self.ui.window_opening(known).unwrap_or_default();
                let at = self.place_for(size, opening);
                // SAFETY: the first window and the input method are the display's, and live as
                // long as the host.
                let opened = unsafe { Shown::second(&self.conn, self.method, first, known, title, scale, (size, at, opening)) };
                self.windows.push(opened);
            }
        }
    }

    /// After a run, does what a window's interface asks of the window: redraw where it changed,
    /// a title, a cursor shape, stay on top or stop, come to the front, the system file dialog;
    /// and its input context hears the text cursor's spot and whether input may be shown. (A
    /// second window's interface keeps the rectangle of all changes until asked: it is asked,
    /// and its places are drawn.)
    fn follow(&mut self, at: usize) {
        let mut changed = std::mem::take(&mut self.windows[at].changed);
        let asks = |ui: &mut Ui| {
            changed.extend(ui.changes());
            ui.changed();
            let window_state = (ui.window_state.title_bar != 0, ui.take_minimize(), ui.take_maximized(), ui.take_full_screen());
            (ui.take_title(), ui.cursor(), ui.ime_area(), ui.password_focused(), ui.window_focused, ui.take_on_top(), ui.take_bring_to_front(), ui.take_file_dialog_request(), window_state)
        };
        let asked = self.windows[at].with(&mut self.ui, asks);
        self.windows[at].changed = changed;
        let Some((title, cursor, ime_area, hidden, focused, top, forward, files, (title_bar, minimize, fill, whole))) = asked else { return };
        files.into_iter().for_each(|ask| self.ask_files(at, ask));
        self.hide_typing(at, hidden, focused);
        self.apply_state(at, minimize, fill, whole);
        // On a resize edge the pointer keeps the edge's shape; over another window, a drag out
        // of this one has its own shape.
        let cursor = self.windows[at].over.cursor().or(self.out.as_ref().and_then(|out| out.cursor(self.windows[at].id))).unwrap_or(cursor);
        let (conn, shown) = (&self.conn, &mut self.windows[at]);
        shown.follow_typing(&conn.x, ime_area, self.scale);
        shown.grips = shown.kind.edges || title_bar;
        shown.point(conn, cursor);
        if let Some(title) = title {
            conn.name(shown.id, &title);
            shown.title = title;
        }
        // Both are the window manager's to do, asked with source 1 (a normal application): add
        // or remove "above" (1, 0); and activate the window, which also restores it from
        // minimized.
        if let Some(on) = top {
            conn.ask(shown.id, c"_NET_WM_STATE", [on as c_long, conn.atom(c"_NET_WM_STATE_ABOVE") as c_long, 0, 1, 0]);
        }
        if forward {
            // SAFETY: the window is the host's own, on its display. (Mapped again, an iconic
            // window is iconic no more.)
            unsafe { (conn.x.XMapWindow)(conn.display, shown.id) };
            conn.ask(shown.id, c"_NET_ACTIVE_WINDOW", [1, 0, 0, 0, 0]);
        }
    }

    /// Does what the app asked of a window's state, all through the window manager (source 1):
    /// add or remove the two maximized states (not asked of a fullscreen window); the same for
    /// fullscreen; and iconify (`WM_CHANGE_STATE` 3). With no window manager, fullscreen is the
    /// window's own to do and the rest nobody's.
    fn apply_state(&mut self, at: usize, minimize: bool, fill: Option<bool>, whole: Option<bool>) {
        if !minimize && fill.is_none() && whole.is_none() {
            return;
        }
        let (conn, shown) = (self.conn.clone(), &mut self.windows[at]);
        let [wide, high, all] = [FILLING[0], FILLING[1], WHOLE].map(|name| conn.atom(name) as c_long);
        if let Some(on) = fill.filter(|_| !shown.whole) {
            conn.ask(shown.id, c"_NET_WM_STATE", [on as c_long, wide, high, 1, 0]);
        }
        match whole {
            Some(on) if conn.managed() => conn.ask(shown.id, c"_NET_WM_STATE", [on as c_long, all, 0, 1, 0]),
            Some(on) => shown.lie_over(&conn, on),
            None => {}
        }
        if minimize {
            conn.ask(shown.id, c"WM_CHANGE_STATE", [3, 0, 0, 0, 0]);
        }
    }

    /// Keeps the input method off a window while what is typed must not be seen, and lets it
    /// back after. (A method shows typed text in its own window and offers it again later: not
    /// for a password.) The window's input context is remade: by the null method meanwhile, so
    /// every key still types what the keymap says, and by the person's method after. The old
    /// context is destroyed, with what the method knew of the window. (Where the method to
    /// switch to is unavailable, the window keeps its context.)
    fn hide_typing(&mut self, at: usize, hidden: bool, focused: bool) {
        if std::mem::replace(&mut self.windows[at].hidden, hidden) == hidden {
            return;
        }
        let method = if hidden { self.plain() } else { self.method };
        let (x, shown) = (&self.conn.x, &mut self.windows[at]);
        // SAFETY: the method is the display's, or null; the window is alive. The destroyed
        // context was made for this window and is not used again.
        unsafe {
            let typing = typing_by(x, method, shown.id);
            if typing.is_null() {
                return;
            }
            if !shown.typing.is_null() {
                (x.XDestroyIC)(shown.typing);
            }
            if focused {
                (x.XSetICFocus)(typing);
            }
            (shown.typing, shown.spot) = (typing, [0; 2]);
        }
    }

    /// The null input method, opened when first asked: Xlib's locale modifiers are set to it
    /// for the open, then back to the person's. (Null where Xlib has none.)
    fn plain(&mut self) -> *mut c_void {
        let (x, display) = (&self.conn.x, self.conn.display);
        if self.plain.is_null() {
            // SAFETY: Xlib's documented calls on an open display; the names end in zeros.
            unsafe { self.plain = open_method(x, display, c"@im=none") };
        }
        self.plain
    }

    /// What a window is drawn with: visual, depth and GC. The screen's own; or for a
    /// see-through window, the 32-bit visual and its GC.
    fn draws_with(&self, at: usize) -> (*mut c_void, c_int, *mut c_void) {
        self.windows[at].kind.deep.map_or(self.screen, |(way, context)| (way, 32, context))
    }

    /// Redraws what a window needs, if anything: its uncovered part, and each place the last
    /// run changed. Two small changes far apart are two small areas to draw and put, not all
    /// between them. (A place inside the uncovered part is drawn with it.) The windows share
    /// the strip all of it is drawn through, used as wide as the window and as tall as keeps it
    /// near a quarter of a megabyte.
    fn draw(&mut self, at: usize) {
        #[cfg(feature = "gpu")]
        self.fit(at);
        let (conn, shown) = (self.conn.clone(), &mut self.windows[at]);
        let (width, bare, mut changed) = (shown.size[0], std::mem::replace(&mut shown.uncovered, Rect::EMPTY), std::mem::take(&mut shown.changed));
        if bare.is_empty() && changed.is_empty() {
            return;
        }
        let rows = (65_536 / width).clamp(16, 256);
        if self.strip.len() < (width * rows) as usize {
            self.strip = vec![0; (width * rows) as usize];
        }
        // SAFETY: X's description of the strip as rows of this window's width. The strip holds
        // that many rows and stays put until the description is freed below.
        let (way, depth, _) = self.draws_with(at);
        let image = unsafe { (conn.x.XCreateImage)(conn.display, way, depth as c_uint, 2, 0, self.strip.as_mut_ptr().cast(), width as c_uint, rows as c_uint, 32, 0) };
        let within = |place: &Rect| place.x >= bare.x && place.y >= bare.y && place.right() <= bare.right() && place.bottom() <= bare.bottom();
        let apart = changed.drain(..).filter(|place| !within(place));
        std::iter::once(bare).chain(apart).for_each(|area| self.put(at, image, rows, area));
        // The list of places is kept empty for its capacity: the next run fills it again.
        self.windows[at].changed = changed;
        // SAFETY: only X's description is freed: the strip is the host's.
        unsafe { (conn.x.XFree)(image) };
        #[cfg(feature = "gpu")]
        self.show(at);
    }

    /// Draws one area of a window a strip at a time and has X put each on screen: `image`
    /// describes the strip as `rows` rows of the window's width. (A see-through window is drawn
    /// on a clear background: the strip is cleared first.)
    fn put(&mut self, at: usize, image: *mut c_void, rows: c_int, area: Rect) {
        let (conn, scale, shown) = (self.conn.clone(), self.scale, &self.windows[at]);
        let (id, [width, height], clear, context) = (shown.id, shown.size, shown.kind.transparent, self.draws_with(at).2);
        let (left, right) = (((area.x * scale).floor() as c_int).max(0), ((area.right() * scale).ceil() as c_int).min(width));
        let (mut top, bottom) = (((area.y * scale).floor() as c_int).max(0), ((area.bottom() * scale).ceil() as c_int).min(height));
        #[cfg(feature = "gpu-draw")]
        if !clear && self.drawn_by_gpu(at, area, [left, top, right, bottom]) {
            return;
        }
        while top < bottom && left < right {
            let high = rows.min(bottom - top);
            let mut canvas = Canvas::new(&mut self.strip[..(width * high) as usize], width as usize, high as usize, (0, top), scale);
            canvas.clip(area.snap_to_pixels(scale));
            if clear {
                canvas.erase();
            }
            self.windows[at].with(&mut self.ui, |ui| ui.paint(&mut canvas));
            #[cfg(feature = "gpu")]
            if self.texture(at, [left, top, right, top + high]) {
                top += high;
                continue;
            }
            // SAFETY: the image describes the strip, which holds at least `high` rows.
            unsafe { (conn.x.XPutImage)(conn.display, id, context, image, left, 0, left, top, (right - left) as c_uint, high as c_uint) };
            top += high;
        }
    }

    /// Puts a painted strip of a window into its texture, where the GPU shows the window
    /// (`place`: left, top, right, bottom in pixels). Says whether it did; if not, X is to put
    /// it.
    #[cfg(feature = "gpu")]
    fn texture(&mut self, at: usize, place: [c_int; 4]) -> bool {
        let (Some(gpu), shown) = (self.gpu.as_mut(), &mut self.windows[at]) else { return false };
        let crate::gpu_x11::Way::Gpu(shows) = &mut shown.way else { return false };
        gpu.put(shows, &self.strip, shown.size[0], place);
        true
    }

    /// Draws `area` of a window (`place` in pixels: left, top, right, bottom) on the GPU, where
    /// the GPU shows the window and draws a part that big itself (see `gpu_draw`). Says whether
    /// it did; if not, the CPU is to.
    #[cfg(feature = "gpu-draw")]
    fn drawn_by_gpu(&mut self, at: usize, area: Rect, place: [c_int; 4]) -> bool {
        let (scale, size) = (self.scale, self.windows[at].size);
        let (Some(gpu), crate::gpu_x11::Way::Gpu(_)) = (self.gpu.as_mut(), &self.windows[at].way) else { return false };
        let Some(list) = gpu.drawing(place) else { return false };
        let mut list = std::mem::take(list);
        list.begin(size);
        let mut canvas = Canvas::recording(&mut list, size[0] as usize, size[1] as usize, (0, 0), scale);
        canvas.clip(area.snap_to_pixels(scale));
        self.windows[at].with(&mut self.ui, |ui| ui.paint(&mut canvas));
        drop(canvas);
        let (Some(gpu), crate::gpu_x11::Way::Gpu(shows)) = (self.gpu.as_mut(), &mut self.windows[at].way) else { return false };
        gpu.draw(shows, list, place)
    }

    /// Shows what was put into a window's texture, where the GPU shows the window.
    #[cfg(feature = "gpu")]
    fn show(&mut self, at: usize) {
        if let (Some(gpu), crate::gpu_x11::Way::Gpu(shows)) = (self.gpu.as_mut(), &mut self.windows[at].way) {
            gpu.show(shows);
        }
    }

    /// Gives a window's texture the window's size, where the GPU shows the window; all of it is
    /// then drawn. A window too big for the device goes to X.
    #[cfg(feature = "gpu")]
    fn fit(&mut self, at: usize) {
        let (Some(gpu), shown) = (self.gpu.as_mut(), &mut self.windows[at]) else { return };
        let crate::gpu_x11::Way::Gpu(shows) = &mut shown.way else { return };
        if shows.size() == shown.size {
            return;
        }
        shown.uncovered = whole(shown.size, self.scale);
        if !gpu.fit(shows, shown.size) {
            crate::gpu_x11::unshow(&mut self.gpu, &mut shown.way);
        }
    }

    /// Brings the GPU in line with the windows (feature `gpu`): opens it when first asked for;
    /// when lost, lets it go and opens it again, or leaves the windows to X for good; and shows
    /// each window not yet asked through it. Says whether a lost device was let go.
    #[cfg(feature = "gpu")]
    fn mend(&mut self) -> bool {
        let lost = self.gpu.as_ref().is_some_and(crate::gpu_x11::Gpu::lost);
        if lost {
            self.let_go();
        }
        if self.gpu.is_none() && self.asks.is_some() {
            self.open_gpu();
        }
        (0..self.windows.len()).for_each(|at| self.ask_gpu(at));
        self.ui.renderer = self.gpu.as_ref().map_or(crate::platform::Renderer::Cpu, crate::gpu_x11::Gpu::renderer);
        lost
    }

    /// Opens the GPU on a window it may show (one not see-through), for all of them: the first
    /// renderer asked for that works. Where none does, X shows them for good.
    #[cfg(feature = "gpu")]
    fn open_gpu(&mut self) {
        let (Some(asks), scale) = (self.asks, self.scale) else { return };
        let Some(shown) = self.windows.iter_mut().find(|shown| !shown.kind.transparent && matches!(shown.way, crate::gpu_x11::Way::Unasked)) else { return };
        // SAFETY: the display is open, and the window alive, of the screen's own visual.
        let Some((gpu, shows)) = (unsafe { crate::gpu_x11::Gpu::open(self.conn.display, self.screen.0, shown.id, shown.size, asks) }) else {
            self.asks = None;
            return;
        };
        (shown.way, shown.uncovered, self.gpu) = (crate::gpu_x11::Way::Gpu(shows), whole(shown.size, scale), Some(gpu));
    }

    /// Shows a window not yet asked through the GPU, where there is one and the window is not
    /// see-through (it is then drawn whole); else by X.
    #[cfg(feature = "gpu")]
    fn ask_gpu(&mut self, at: usize) {
        let (scale, shown) = (self.scale, &mut self.windows[at]);
        if !matches!(shown.way, crate::gpu_x11::Way::Unasked) {
            return;
        }
        let made = self.gpu.as_mut().filter(|_| !shown.kind.transparent).and_then(|gpu| gpu.window(shown.id, shown.size));
        shown.way = made.map_or(crate::gpu_x11::Way::X, crate::gpu_x11::Way::Gpu);
        if matches!(shown.way, crate::gpu_x11::Way::Gpu(_)) {
            shown.uncovered = whole(shown.size, scale);
        }
    }

    /// Sleeps until X has events, another thread asks for a run, the session bus or a screen
    /// reader has messages, or the time comes that the interface asked to be woken at, or a
    /// silent portal is given up on. (Not at all when a run is due.)
    fn sleep(&mut self) {
        let (x, display) = (&self.conn.x, self.conn.display);
        let buses = [self.session.bus.as_ref().map_or((-1, 0), Bus::waits), self.session.readers.waits()];
        let [session, desk] = buses.map(|(fd, events)| PollFd { fd, events, revents: 0 });
        let mut waiting = [PollFd { fd: -1, events: 1, revents: 0 }, PollFd { fd: self.wake, events: 1, revents: 0 }, session, desk];
        // Not past the time a silent portal is given up on, or a drop target that never
        // answers.
        let patience = self.session.portal.patience_ends().map(|ends| ends.saturating_sub(self.start.elapsed()));
        let answer = self.out.as_ref().and_then(|out| out.until).map(|until| until.saturating_duration_since(Instant::now()));
        // SAFETY: the display and pipe are open; `read` drains what woke the windows, there
        // when `poll` says so: two bytes at most (see `WOKEN`).
        unsafe {
            (x.XFlush)(display);
            if self.rerun || (x.XPending)(display) > 0 {
                return;
            }
            waiting[0].fd = (x.XConnectionNumber)(display);
            poll(waiting.as_mut_ptr(), 4, [self.ui.wake_in(), patience, answer].into_iter().flatten().min().map_or(-1, |wait| wait.as_millis() as c_int + 1));
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

// ---- taking in drops (XDND, as the target) ----

/// How the windows take drops of files and text from other programs, by XDND version 5: a
/// window announces `XdndAware` once its app asked about drops, and from then on receives them.
/// Set by the first call asking about drops ([`Ui::files_hovered`], [`Ui::files_dropped`],
/// [`Ui::text_hovered`], [`Ui::text_dropped`], through `platform::hand_drops`); reached no
/// other way, so a program that never asks is built without it, and none of its windows accepts
/// drops.
#[derive(Clone, Copy)]
struct Drops {
    /// An XDND message to window `at` from a drag source. Says whether an interface must run.
    xdnd_message: fn(Taking<'_>, usize, &ClientEvent) -> bool,
    /// After a run (see `Taking::ran`).
    ran: fn(Taking<'_>),
}

/// Hands the windows their drop handling, on this thread (see `Drops`).
pub(crate) fn hand_drops() {
    DROPS.set(Some(Drops { xdnd_message: |taking, at, told| taking.xdnd_message(at, told), ran: |taking| taking.ran() }));
}

/// What taking in drops works with: the connection, the first window's interface (lending out
/// the others'), the windows, the scale, and the windows something was just dropped on with its
/// source window, which learn whether it was taken once the interface ran. And whether an
/// interface must run.
struct Taking<'a> {
    conn: &'a Connection,
    ui: &'a mut Ui,
    windows: &'a mut [Shown],
    scale: f32,
    finishing: &'a mut Vec<(XWindow, XWindow)>,
    rerun: bool,
}

impl Taking<'_> {
    /// An XDND message to window `at` from a drag source: enter, position, leave or drop. Says
    /// whether an interface must run.
    fn xdnd_message(mut self, at: usize, told: &ClientEvent) -> bool {
        let conn = self.conn;
        let (named, from) = (|name: &CStr| told.message == conn.atom(name), told.data[0] as XWindow);
        if named(c"XdndEnter") {
            let kind = self.offered(at, told);
            self.note_drag_over(at, if kind != 0 { from } else { 0 }, kind);
        } else if named(c"XdndPosition") {
            self.drag_moved_to(at, from, told.data[2]);
        } else if named(c"XdndLeave") && from == self.windows[at].drag_over.0 {
            self.note_drag_over(at, 0, 0);
            self.hear(at, Event::Leave); // the pointer left with it
        } else if named(c"XdndDrop") {
            self.let_go(at, from, told.data[2] as c_ulong);
        }
        self.rerun
    }

    /// After a run: each window whose app asked about drops announces `XdndAware` if it has not
    /// (then keeps doing so while open); and each source whose drop landed on one of the
    /// windows hears whether a box there took it, now that the interface has run.
    fn ran(self) {
        let conn = self.conn;
        for shown in self.windows.iter_mut().filter(|shown| !shown.takes) {
            shown.takes = shown.with(self.ui, |ui| ui.takes_files.get() || ui.crossing.takes_text.get()) == Some(true);
            if shown.takes {
                conn.leave(shown.id, conn.atom(c"XdndAware"), 4, &[5 as Atom]);
            }
        }
        let copies = conn.atom(c"XdndActionCopy") as c_long;
        for (id, from) in std::mem::take(self.finishing) {
            let shown = self.windows.iter().find(|shown| shown.id == id);
            let taken = shown.and_then(|shown| shown.with(self.ui, Ui::take_drop_accepted)) == Some(true);
            conn.tell(from, from, c"XdndFinished", [id as c_long, taken as c_long, if taken { copies } else { 0 }, 0, 0]);
        }
    }

    /// Passes what is dragged over a window to its interface, noting whether it must run.
    fn hear(&mut self, at: usize, event: Event) {
        self.rerun |= self.windows[at].with(self.ui, |ui| ui.event(event)) == Some(true);
    }

    /// What a window takes of what is dragged over it: files as a URI list, where its app asks
    /// about files and they are offered; else text, likewise, under either UTF-8 text name. (0
    /// for nothing.) `XdndEnter` lists up to three types; a source offering more keeps the list
    /// on its own window and says so (lowest bit of the second number).
    fn offered(&mut self, at: usize, told: &ClientEvent) -> Atom {
        let conn = self.conn;
        let kinds = if told.data[1] & 1 == 0 { told.data[2..].iter().map(|kind| *kind as Atom).collect() } else { conn.numbers(told.data[0] as XWindow, c"XdndTypeList") };
        let [files, text] = self.windows[at].with(self.ui, |ui| [ui.takes_files.get(), ui.crossing.takes_text.get()]).unwrap_or_default();
        let wanted = [(files, FILES), (text, TEXT), (text, c"UTF8_STRING")].into_iter().filter(|(takes, _)| *takes);
        wanted.map(|(_, kind)| conn.atom(kind)).find(|kind| kinds.contains(kind)).unwrap_or(0)
    }

    /// Notes which window drags something over a window and as what (0 for none), and tells its
    /// interface when it comes and goes.
    fn note_drag_over(&mut self, at: usize, from: XWindow, kind: Atom) {
        let (was, text) = (self.windows[at].drag_over, kind != self.conn.atom(FILES));
        if (from != 0) != (was.0 != 0) {
            let text = if from != 0 { text } else { was.2 != self.conn.atom(FILES) };
            self.hear(at, if text { Event::TextHovered(from != 0) } else { Event::FilesHovered(from != 0) });
        }
        self.windows[at].drag_over = (from, was.1, kind);
    }

    /// The drag reached a screen position (its halves are x and y). If the window takes it, its
    /// interface hears where on the window that is; and the source hears whether it may be
    /// dropped here. It may, as a copy.
    fn drag_moved_to(&mut self, at: usize, from: XWindow, place: c_long) {
        let conn = self.conn;
        let (x, display, id) = (&conn.x, conn.display, self.windows[at].id);
        let (takes, mut on, mut child) = (from == self.windows[at].drag_over.0, [0; 2], 0);
        if takes {
            // SAFETY: X fills in where on this window the screen position is.
            unsafe { (x.XTranslateCoordinates)(display, root(x, display), id, (place >> 16) as i16 as c_int, place as i16 as c_int, &mut on[0], &mut on[1], &mut child) };
            self.windows[at].drag_over.1 = pt(on[0] as f32 / self.scale, on[1] as f32 / self.scale);
            self.hear(at, Event::Move(self.windows[at].drag_over.1));
        }
        let copies = if takes { conn.atom(c"XdndActionCopy") } else { 0 };
        // No rectangle is given in which the reply would stay the same: every move is to be
        // reported.
        conn.tell(from, from, c"XdndStatus", [id as c_long, takes as c_long * 3, 0, 0, copies as c_long]);
    }

    /// The drag was dropped over a window. If the window takes it, the source is asked for it
    /// on behalf of that window (at hand, where the source is this program), and that window's
    /// interface gets it where it was dropped. The source hears whether a box of the window
    /// took it once the interface has run.
    fn let_go(&mut self, at: usize, from: XWindow, time: c_ulong) {
        let (conn, (source, _, kind), id) = (self.conn, self.windows[at].drag_over, self.windows[at].id);
        let got = (from == source && from != 0).then(|| conn.dragged_out_as(kind).or_else(|| conn.read(id, conn.dnd, kind, time))).flatten();
        if from == source {
            self.note_drag_over(at, 0, kind);
        }
        let Some(got) = got.filter(|got| !got.is_empty()) else { return conn.tell(from, from, c"XdndFinished", [id as c_long, 0, 0, 0, 0]) };
        self.hear(at, Event::Move(self.windows[at].drag_over.1));
        let files = kind == conn.atom(FILES);
        self.hear(at, if files { Event::FilesDropped(files_in(&got)) } else { Event::TextDropped(String::from_utf8_lossy(&got).into_owned()) });
        self.finishing.push((id, from));
    }
}

// ---- dragging out of a window (XDND, as the source) ----

/// How the windows drag things out to other programs by XDND. Set by the first call handing
/// something out ([`Ui::drag_out`], through `platform::enable_drag_out`); reached no other way,
/// so a program that never does is built without it (including a PNG writer).
#[derive(Clone, Copy)]
struct Outward {
    /// After a run: what the person just dragged out of a window goes out. Says whether an
    /// interface must run (see the others too).
    ran: fn(Outgoing<'_>) -> bool,
    /// The pointer is at a screen position at a time, in window `at`: says whether what that
    /// window drags is out of it there.
    moved: fn(Outgoing<'_>, usize, [c_int; 2], c_ulong) -> bool,
    /// The button came up in window `at`, at a time.
    released: fn(Outgoing<'_>, usize, c_ulong) -> bool,
    /// The drag out is cancelled.
    given_up: fn(Outgoing<'_>) -> bool,
    /// The window it is over (named by the first number) says what it would do with it; or
    /// (with `true`) what it did after the drop.
    reply: fn(Outgoing<'_>, XWindow, bool, [c_long; 5]) -> bool,
}

/// Hands the windows their drag-out code, on this thread (see `Outward`).
pub(crate) fn enable_drag_out() {
    OUTWARD.set(Some(Outward {
        ran: |outgoing| outgoing.ran(),
        moved: |mut outgoing, at, root, time| outgoing.out_at(at, root, time),
        released: |mut outgoing, at, time| {
            outgoing.out_released(at, time);
            outgoing.rerun
        },
        given_up: |mut outgoing| {
            outgoing.give_up_out();
            outgoing.rerun
        },
        reply: |mut outgoing, from, done, data| {
            match done {
                true => outgoing.finished(from, data),
                false => outgoing.answered(from, data),
            }
            outgoing.rerun
        },
    }));
}

/// What dragging out works with: the connection, the first window's interface (lending out the
/// others'), the windows, the current drag if any, and the pointer's last screen position, time
/// and modifiers. And whether an interface must run.
struct Outgoing<'a> {
    conn: &'a Connection,
    ui: &'a mut Ui,
    windows: &'a mut [Shown],
    out: &'a mut Option<Out>,
    last: ([c_int; 2], c_ulong, c_uint),
    rerun: bool,
}

impl Outgoing<'_> {
    /// After a run: whatever the person just dragged out of a window goes out (see
    /// `start_drag_out`). Says whether an interface must run.
    fn ran(mut self) -> bool {
        for at in 0..self.windows.len() {
            self.start_drag_out(at);
        }
        self.rerun
    }

    /// If the person just dragged something out of window `at`, the XDND drag starts: the
    /// window owns `XdndSelection` in each type offered, keeping the type list on itself where
    /// there are more than three, and grabs the keyboard while dragging, so Escape cancels it
    /// wherever the pointer is. Then the window under the pointer hears of it.
    fn start_drag_out(&mut self, at: usize) {
        let conn = self.conn;
        let Some(kinds) = self.windows[at].with(self.ui, |ui| ui.take_drag_out().map(|what| conn.kinds_of(what))).flatten() else { return };
        self.give_up_out();
        let (from, ([x, y], time, state)) = (self.windows[at].id, self.last);
        let names: Vec<Atom> = kinds.iter().map(|(kind, _)| *kind).collect();
        conn.own_drag_out(kinds, from, time);
        if names.len() > 3 {
            conn.leave(from, conn.atom(c"XdndTypeList"), 4, &names);
        }
        // SAFETY: the window is the host's own, on its display. (`GrabModeAsync` for both.)
        unsafe { (conn.x.XGrabKeyboard)(conn.display, from, 0, 1, 1, time) };
        *self.out = Some(Out { from, kinds: names, over: 0, to: 0, version: 0, home: true, takes: None, asked: false, next: None, let_go: None, dropped: false, until: None, moves: state & 1 != 0 });
        self.out_at(at, [x, y], time);
    }

    /// The pointer is at a screen position at a time, window `at` having the pointer: says
    /// whether its drag is out of the window there, over something else. While it is, the
    /// XDND-aware window under the pointer gets positions (and one it left, a leave). A drag
    /// not yet out goes out as soon as the pointer is over another window, even one on top of
    /// this one: the window's interface hears the pointer left.
    fn out_at(&mut self, at: usize, root: [c_int; 2], time: c_ulong) -> bool {
        let id = self.windows[at].id;
        let going = self.out.as_ref().is_some_and(|out| out.from == id && out.let_go.is_none());
        if !going {
            return self.windows[at].with(self.ui, |ui| ui.drag_leaves_window()) == Some(true) && self.aware_at(root).0 != id;
        }
        let (under, to, version) = self.aware_at(root);
        let Some(out) = self.out.as_mut() else { return false };
        let over = if under == id { 0 } else { under };
        out.home = under == id;
        if over != out.over {
            let (was, from, kinds, more) = ((out.to, out.over), out.from, out.kinds.clone(), out.kinds.len() > 3);
            (out.over, out.to, out.version, out.takes, out.asked, out.next) = (over, to, version, None, false, None);
            if was.1 != 0 {
                self.conn.tell(was.0, was.1, c"XdndLeave", [from as c_long, 0, 0, 0, 0]);
            }
            let first = |at: usize| kinds.get(at).map_or(0, |kind| *kind as c_long);
            if over != 0 {
                self.conn.tell(to, over, c"XdndEnter", [from as c_long, version << 24 | more as c_long, first(0), first(1), first(2)]);
            }
        }
        self.shape_out(at);
        if over != 0 {
            self.position(root, time);
        }
        under != id
    }

    /// The XDND-aware window at a screen position, found top-down through the windows
    /// containing it (a window manager frame, then its client): that window, the window its
    /// messages go to (itself or a proxy, which also says so), and the protocol version both
    /// speak. (0s where there is none, or it speaks a version older than 3.)
    fn aware_at(&self, [x, y]: [c_int; 2]) -> (XWindow, XWindow, c_long) {
        let (conn, mut under) = (self.conn, root(&self.conn.x, self.conn.display));
        for _ in 0..8 {
            let (mut on, mut child) = ([0; 2], 0);
            // SAFETY: X fills in which child of this window contains the screen position.
            let found = unsafe { (conn.x.XTranslateCoordinates)(conn.display, root(&conn.x, conn.display), under, x, y, &mut on[0], &mut on[1], &mut child) };
            if found == 0 || child == 0 {
                break;
            }
            under = child;
            let proxy = conn.numbers(under, c"XdndProxy").first().map(|proxy| *proxy as XWindow);
            let to = proxy.filter(|proxy| conn.numbers(*proxy, c"XdndProxy").first() == Some(&(*proxy as c_ulong))).unwrap_or(under);
            if let Some(version) = conn.numbers(to, c"XdndAware").first() {
                return if *version >= 3 { (under, to, (*version).min(5) as c_long) } else { (0, 0, 0) };
            }
        }
        (0, 0, 0)
    }

    /// Sends the target an `XdndPosition` with the pointer's place and the action asked (copy,
    /// or move with Shift): now, or once it answered the last one, which is waited for.
    fn position(&mut self, [x, y]: [c_int; 2], time: c_ulong) {
        let conn = self.conn;
        let Some(out) = self.out.as_mut() else { return };
        if std::mem::replace(&mut out.asked, true) {
            return out.next = Some(([x, y], time));
        }
        let asked = conn.atom(if out.moves { c"XdndActionMove" } else { c"XdndActionCopy" }) as c_long;
        let (to, over, from) = (out.to, out.over, out.from as c_long);
        conn.tell(to, over, c"XdndPosition", [from, 0, (x as c_long) << 16 | (y as c_long & 0xffff), time as c_long, asked]);
    }

    /// The target answered (`from` names it): whether it accepts and what it would do. It gets
    /// the position the pointer reached meanwhile; or, if the button came up meanwhile, the
    /// drop now.
    fn answered(&mut self, from: XWindow, data: [c_long; 5]) {
        let Some(out) = self.out.as_mut().filter(|out| out.over == from && out.asked) else { return };
        out.asked = false;
        out.takes = (data[1] & 1 != 0).then_some(data[4] as Atom);
        let (next, let_go, id) = (out.next.take(), out.let_go, out.from);
        if let Some(at) = self.windows.iter().position(|shown| shown.id == id) {
            self.shape_out(at);
        }
        match (next, let_go) {
            (_, Some(time)) => self.let_out_go(time),
            (Some((root, time)), None) => self.position(root, time),
            (None, None) => {}
        }
    }

    /// The dragging button came up at a time. Over an XDND target, the drop goes there once it
    /// answered the last position (with a second to do so). Over its own window or nothing that
    /// accepts it, the drag ends (and in its own window, it drops as any internal drag).
    fn out_released(&mut self, at: usize, time: c_ulong) {
        let Some(out) = self.out.as_mut().filter(|out| out.from == self.windows[at].id && out.let_go.is_none()) else { return };
        // The keyboard is released: what the person types next goes wherever they type.
        // SAFETY: the display is open. (0: `CurrentTime`.)
        unsafe { (self.conn.x.XUngrabKeyboard)(self.conn.display, 0) };
        if out.over == 0 {
            return self.end_out(None);
        }
        out.let_go = Some(time);
        if out.asked {
            return out.until = Some(Instant::now() + Duration::from_secs(1));
        }
        self.let_out_go(time);
    }

    /// Drops on the target window if it said it accepts, then waits for `XdndFinished` (ten
    /// seconds at most). If it did not, it just gets `XdndLeave`.
    fn let_out_go(&mut self, time: c_ulong) {
        let Some(out) = self.out.as_mut() else { return };
        let (to, over, from) = (out.to, out.over, out.from as c_long);
        if out.takes.is_none() {
            self.conn.tell(to, over, c"XdndLeave", [from, 0, 0, 0, 0]);
            return self.end_out(None);
        }
        (out.dropped, out.until) = (true, Some(Instant::now() + Duration::from_secs(10)));
        self.conn.tell(to, over, c"XdndDrop", [from, 0, time as c_long, 0, 0]);
    }

    /// The drop target says it is done (`from` names it): whether it took it and what it did.
    /// (A version older than 5 says neither: it took it as it said it would.)
    fn finished(&mut self, from: XWindow, data: [c_long; 5]) {
        let Some(out) = self.out.as_ref().filter(|out| out.dropped && out.over == from) else { return };
        let (took, did) = if out.version >= 5 { (data[1] & 1 != 0, data[2] as Atom) } else { (true, out.takes.unwrap_or(0)) };
        let moved = did == self.conn.atom(c"XdndActionMove");
        self.end_out(took.then_some(if moved { DropEffect::Moved } else { DropEffect::Copied }))
    }

    /// Cancels the drag out, if any: the window it is over gets `XdndLeave`, unless the drop
    /// happened already.
    fn give_up_out(&mut self) {
        let Some(out) = self.out.as_ref() else { return };
        if out.over != 0 && !out.dropped {
            self.conn.tell(out.to, out.over, c"XdndLeave", [out.from as c_long, 0, 0, 0, 0]);
        }
        self.end_out(None)
    }

    /// The drag out is over: the keyboard is released, the selection dropped, and the source
    /// window hears what became of it.
    fn end_out(&mut self, taken: Option<DropEffect>) {
        let Some(out) = self.out.take() else { return };
        let conn = self.conn;
        // SAFETY: the display is open. (0: `CurrentTime`.)
        unsafe { (conn.x.XUngrabKeyboard)(conn.display, 0) };
        conn.own_drag_out(Vec::new(), 0, 0);
        let Some(at) = self.windows.iter().position(|shown| shown.id == out.from) else { return };
        self.rerun |= self.windows[at].with(self.ui, |ui| ui.set_drop_effect(taken)).is_some();
    }

    /// Gives the pointer its shape while something is dragged out of window `at`, where it has
    /// one (see `Out::cursor`). (While the button is held, X shows the cursor of the window the
    /// button went down in wherever the pointer is.)
    fn shape_out(&mut self, at: usize) {
        let (conn, shown) = (self.conn, &mut self.windows[at]);
        let Some(cursor) = self.out.as_ref().and_then(|out| out.cursor(shown.id)) else { return };
        shown.point(conn, cursor);
    }
}

#[cfg(feature = "gpu")]
impl<App> Host<App> {
    /// Lets go of the GPU, each window's surface first. Each window is to be asked again, and
    /// drawn whole.
    #[cfg(feature = "gpu")]
    fn let_go(&mut self) {
        let Some(mut gpu) = self.gpu.take() else { return };
        for shown in &mut self.windows {
            if let crate::gpu_x11::Way::Gpu(shows) = std::mem::replace(&mut shown.way, crate::gpu_x11::Way::Unasked) {
                gpu.forget(shows);
            }
            shown.uncovered = whole(shown.size, self.scale);
        }
        self.ui.renderer = crate::platform::Renderer::Cpu;
    }
}

impl<App> Drop for Host<App> {
    fn drop(&mut self) {
        // The GPU goes first, while the windows and the display are there.
        #[cfg(feature = "gpu")]
        self.let_go();
        // Every window goes, with its input context. (The display closes when the connection
        // ends, once the interface has dropped it too.)
        self.windows.drain(..).for_each(|shown| shown.close(&self.conn));
        // SAFETY: the input methods were opened in `open` and `plain`, and nothing made from
        // either is left.
        [self.method, self.plain].into_iter().filter(|method| !method.is_null()).for_each(|method| _ = unsafe { (self.conn.x.XCloseIM)(method) });
    }
}

// ---- what an app asks of the desktop beyond its windows ----

/// What the windows do for an app's desktop requests (tray icon, notifications, global hotkeys,
/// a hidden window, single instance): session bus messages for it go to `freedesktop` (from
/// Wayland's windows too); after a run, the app's requests go to the desktop and the first
/// window does what they ask; and X delivers the global hotkeys, grabbed here. Set by the first
/// call asking for any of it (through `platform::hand_desktop`); reached no other way, so a
/// program that never asks is built without it.
#[derive(Clone, Copy)]
pub(crate) struct Desktop {
    pub(crate) heard: fn(&mut Bus, &Message) -> bool,
    stood: fn(&Connection, &mut Shown, &mut Ui, &mut Session) -> bool,
    pressed: fn(&Connection, &InputEvent) -> bool,
}

thread_local! {
    /// What the windows do for the desktop: nothing until an app on this thread asks (see
    /// `Desktop`).
    pub(crate) static DESKTOP: Cell<Option<Desktop>> = const { Cell::new(None) };
    /// The few X calls only global hotkeys and a hidden window need, opened the first time.
    static GRABS: std::cell::OnceCell<Option<Grabs>> = const { std::cell::OnceCell::new() };
}

/// Hands the windows their desktop integration, on this thread (see `Desktop`).
pub(crate) fn hand_desktop() {
    DESKTOP.set(Some(Desktop { heard: crate::freedesktop::heard, stood, pressed }));
}

library!(Grabs, ["libX11.so.6", "libX11.so"], {
    fn XGrabKey(Display, c_int, c_uint, XWindow, c_int, c_int, c_int) -> c_int;
    fn XUngrabKey(Display, c_int, c_uint, XWindow) -> c_int;
    fn XKeysymToKeycode(Display, c_ulong) -> c_uchar;
    fn XSync(Display, c_int) -> c_int;
    fn XWithdrawWindow(Display, XWindow, c_int) -> c_int;
});

/// X's modifier masks: Shift 1, Control 4, Alt (`Mod1`) 8, Super (`Mod4`) 64. And the lock
/// masks, Caps Lock 2 and Num Lock (`Mod2`) 16, with which each key is grabbed too, so it works
/// with either on.
const X_HELD: [(u8, c_uint); 4] = [(crate::hotkeys::SHIFT, 1), (crate::hotkeys::CTRL, 4), (crate::hotkeys::ALT, 8), (crate::hotkeys::LOGO, 64)];
const LOCKS: [c_uint; 4] = [0, 2, 16, 18];

/// A hotkey's modifiers as X has them.
fn x_held(held: u8) -> c_uint {
    X_HELD.iter().filter(|(ours, _)| held & ours != 0).map(|(_, theirs)| theirs).sum()
}

/// X refused a grab since this was last checked: another program has the key.
static REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// X's error handler while grabbing keys: notes a refused grab (request code 33). The request
/// code is a byte after four pointer-sized fields (the error code is the byte before).
unsafe extern "C" fn refused(_: Display, error: *mut c_void) -> c_int {
    // SAFETY: X passes the error as `XErrorEvent` lays it out.
    if unsafe { *error.cast::<u8>().add(4 * size_of::<c_ulong>() + 1) } == 33 {
        REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// After a run under X: the app's requests go to the desktop over the session bus; global
/// hotkeys it says are grabbed, those it no longer says released; and the first window is
/// withdrawn (the window manager removes it from the screen and its lists), or mapped and
/// raised: for a later start of the program, with that start's timestamp, or where its app
/// asked it back. Says whether its interface must run.
fn stood(conn: &Connection, first: &mut Shown, ui: &mut Ui, session: &mut Session) -> bool {
    let asked = crate::freedesktop::ran(session.bus.as_mut(), false, "");
    let Some((mut news, withdraw)) = GRABS.with(|grabs| grabs.get_or_init(Grabs::open).as_ref().map(|grabs| (grab(conn, grabs), grabs.XWithdrawWindow))) else { return false };
    if asked.hide && !first.withdrawn {
        // SAFETY: the window is the host's own, on its display and screen.
        unsafe { withdraw(conn.display, first.id, (conn.x.XDefaultScreen)(conn.display)) };
        first.withdrawn = true;
        news |= ui.event(Event::Minimized(true));
    }
    let back = first.withdrawn && ui.take_bring_to_front();
    if asked.forward || back {
        // A start's timestamp is in its startup id, after "_TIME".
        let time = asked.token.rsplit_once("_TIME").and_then(|(_, time)| time.parse::<c_long>().ok()).unwrap_or(0);
        // SAFETY: as above.
        unsafe { (conn.x.XMapWindow)(conn.display, first.id) };
        conn.ask(first.id, c"_NET_ACTIVE_WINDOW", [1, time, 0, 0, 0]);
        news |= std::mem::take(&mut first.withdrawn) && ui.event(Event::Minimized(false));
    }
    news
}

/// Grabs on the root window the global hotkeys the app wants and lacks, and releases those it
/// no longer wants. A key the keyboard lacks, or another program grabbed, is refused. Says
/// whether one was.
fn grab(conn: &Connection, grabs: &Grabs) -> bool {
    let (display, root) = (conn.display, root(&conn.x, conn.display));
    // SAFETY: plain queries about the display's keyboard.
    let code = |keysym: u32| unsafe { (grabs.XKeysymToKeycode)(display, keysym as c_ulong) } as c_int;
    crate::hotkeys::HOTKEYS.with_borrow_mut(|hotkeys| {
        // SAFETY: keys are ungrabbed on the display's root, as they were grabbed.
        let let_go = |held: c_uint, code: c_int| LOCKS.iter().for_each(|lock| _ = unsafe { (grabs.XUngrabKey)(display, code, held | lock, root) });
        hotkeys.settle(|wanted| _ = (wanted.state == crate::hotkeys::Registration::Registered).then(|| let_go(x_held(wanted.combo.modifiers), code(wanted.combo.key.keysym().0))));
        let mut refused_any = false;
        for wanted in hotkeys.keys.iter_mut().filter(|wanted| wanted.state == crate::hotkeys::Registration::New) {
            let (held, code) = (x_held(wanted.combo.modifiers), code(wanted.combo.key.keysym().0));
            let had = code != 0 && !switches_console(wanted.combo) && grabbed(conn, grabs, held, code);
            wanted.state = if had { crate::hotkeys::Registration::Registered } else { crate::hotkeys::Registration::Refused };
            refused_any |= !had;
        }
        refused_any
    })
}

/// Whether X keeps a key for itself: Ctrl+Alt+F1 to F12 (without Shift) switch virtual consoles
/// in every X keymap and never reach a program, grabbed or not.
fn switches_console(combo: crate::hotkeys::Combo) -> bool {
    use crate::hotkeys::{ALT, CTRL, SHIFT};
    matches!(combo.key, crate::hotkeys::Named::F(1..=12)) && combo.modifiers & (CTRL | ALT | SHIFT) == CTRL | ALT
}

/// Grabs one key with its modifiers, delivered to this program whatever has focus: once per
/// lock combination. Whether X refused it is known once X answered all of that: errors
/// meanwhile are noted, not shrugged off. Says whether the program has it (a refused one is
/// released again).
fn grabbed(conn: &Connection, grabs: &Grabs, held: c_uint, code: c_int) -> bool {
    let (x, display, root) = (&conn.x, conn.display, root(&conn.x, conn.display));
    REFUSED.store(false, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: the display is open; X's error handler is restored once X answered (`XSync`).
    // Grabbed with `GrabModeAsync` for both (1, 1).
    unsafe {
        let was = (x.XSetErrorHandler)(Some(refused));
        LOCKS.iter().for_each(|lock| _ = (grabs.XGrabKey)(display, code, held | lock, root, 0, 1, 1));
        (grabs.XSync)(display, 0);
        (x.XSetErrorHandler)(was);
        let had = !REFUSED.load(std::sync::atomic::Ordering::Relaxed);
        if !had {
            LOCKS.iter().for_each(|lock| _ = (grabs.XUngrabKey)(display, code, held | lock, root));
        }
        had
    }
}

/// X delivered a key pressed on the root window: if it is one of the program's global hotkeys,
/// the next run hears of it. (Lock keys do not count.)
fn pressed(conn: &Connection, key: &InputEvent) -> bool {
    let Some(Some(grabs)) = GRABS.with(|grabs| grabs.get().map(|grabs| grabs.as_ref().map(|grabs| grabs.XKeysymToKeycode))) else { return false };
    let held = key.state & x_held(0x0f);
    crate::hotkeys::HOTKEYS.with_borrow_mut(|hotkeys| {
        // SAFETY: a plain query about the display's keyboard.
        let same = |wanted: &crate::hotkeys::Wanted| x_held(wanted.combo.modifiers) == held && unsafe { grabs(conn.display, wanted.combo.key.keysym().0 as c_ulong) } as c_uint == key.detail;
        let number = hotkeys.keys.iter().filter(|wanted| wanted.state == crate::hotkeys::Registration::Registered).find(|wanted| same(wanted)).map(|wanted| wanted.number);
        number.is_some_and(|number| hotkeys.pressed(number))
    })
}

/// Opens the window and runs the app in it until the window is closed.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    // The session bus is asked first: its replies arrive while fonts are found and the window
    // made, and as a rule are there by the first run.
    let session = Session::on(Bus::session());
    let (Some(x), Some(text)) = (Xlib::open(), SystemText::new()) else {
        return eprintln!("taut: this needs libX11, libfreetype and libfontconfig, and a font.");
    };
    let Some(mut host) = Host::open(x, text, session, window, app) else {
        return eprintln!("taut: no display to open a window on.");
    };
    while !host.done {
        host.sleep();
        host.turn();
    }
}

/// The pipe other threads wake the windows through: one per program, created when its first
/// window opens and never closed (a [`Waker`] may be kept and used after every window is gone).
/// Its ends are close-on-exec. (Where no pipe could be made, its ends are -1: nothing waits on
/// or writes to them.)
static PIPE: std::sync::OnceLock<[c_int; 2]> = std::sync::OnceLock::new();
/// A wake is in the pipe that the windows have not drained. While one is, another writes
/// nothing, so the pipe holds at most two bytes: it never fills, and a [`Waker`] never blocks
/// on it, window or not.
pub(crate) static WOKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The pipe's two ends, made when first asked: read, then write. (Each set close-on-exec:
/// `F_SETFD` 2, `FD_CLOEXEC` 1.)
pub(crate) fn pipe_ends() -> [c_int; 2] {
    *PIPE.get_or_init(|| {
        let mut ends = [-1; 2];
        // SAFETY: the system fills in the two ends, the program's from then on.
        if unsafe { pipe(ends.as_mut_ptr()) } == 0 {
            ends.iter().for_each(|end| _ = unsafe { fcntl(*end, 2, 1 as c_int) });
        }
        ends
    })
}

/// Opens the input method the locale `modifiers` name (none: the person's own). Xlib's input
/// methods follow `LC_CTYPE` at open: it is the person's (from the environment) for that.
/// (Null where X has no method for it.)
///
/// # Safety
/// `display` is open.
unsafe fn open_method(x: &Xlib, display: Display, modifiers: &CStr) -> *mut c_void {
    // SAFETY: Xlib's documented calls on an open display; the names end in zeros.
    in_locale(c"", || unsafe {
        (x.XSetLocaleModifiers)(modifiers.as_ptr());
        let method = (x.XOpenIM)(display, null_mut(), null_mut(), null_mut());
        (x.XSetLocaleModifiers)(c"".as_ptr());
        method
    })
}

/// Held while the program's `LC_CTYPE` is changed for a moment. Windows open on several threads
/// at once: unheld, one thread copied another's passing locale and put that back for good.
static LOCALE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `then` with `LC_CTYPE` set to `name` (empty: the person's, from the environment), then
/// puts the program's back: a host program may have set its own, and its other threads follow
/// it. (`setlocale` is not thread-safe: the library's own changes take turns; the host's own
/// must not come meanwhile.)
fn in_locale<T>(name: &CStr, then: impl FnOnce() -> T) -> T {
    let _turn = LOCALE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: the locale is read and copied before it changes; the names end in zeros.
    let was = unsafe {
        let was = setlocale(0, null());
        let was = (!was.is_null()).then(|| CStr::from_ptr(was).to_owned());
        setlocale(0, name.as_ptr());
        was
    };
    let done = then();
    if let Some(was) = was {
        // SAFETY: a name `setlocale` gave.
        unsafe { setlocale(0, was.as_ptr()) };
    }
    done
}

/// Asks the window to run its interface again; can be sent from any thread. (An app learning
/// something in the background, say a changed file, wakes the window with this.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;
impl Waker {
    /// Has the interface run once more, soon, on its own thread, from whatever thread calls
    /// this.
    pub fn wake(&self) {
        let Some([_, end]) = PIPE.get().copied() else { return };
        // A wake already in the pipe makes the interface run after this one too.
        if WOKEN.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // SAFETY: one byte into the program's own pipe, never closed, which has room.
        if unsafe { write(end, [1u8].as_ptr().cast(), 1) } != 1 {
            WOKEN.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::portal::tests::{Ends, OwnBus, StandIn};
    use crate::preferences::tests::{Kept, settings_of};
    use crate::{FileDialog, SecondWindow, Theme};
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

    /// An unmapped window with the clipboard `selection`; nothing where there is no display to
    /// test on.
    fn conn(selection: &CStr) -> Option<Connection> {
        // SAFETY: the window is made on a display that stays open for the rest of the test, and
        // selects property changes.
        unsafe {
            let x = Xlib::open()?;
            let display = (x.XOpenDisplay)(null());
            if display.is_null() {
                return None;
            }
            let (id, _) = new_window(&x, display, [1, 1], [1, 1], None, [false; 2]);
            (x.XSelectInput)(display, id, 1 << 22);
            Some(Connection::new(x, display, id, selection))
        }
    }

    /// The same as an interface has it: with its picture and file code, handed over on the
    /// first request for one.
    fn clipboard(selection: &CStr) -> Option<SystemClipboard> {
        hand_pictures_and_files();
        Some(SystemClipboard(Rc::new(conn(selection)?)))
    }

    /// Does for a connection what a window's loop does: waits for the next X event and serves
    /// selection requests and `INCR` progress.
    fn serve(conn: &Connection) {
        // SAFETY: the events are read from the connection's own display.
        unsafe {
            let mut event: XEvent = zeroed();
            (conn.x.XNextEvent)(conn.display, &mut event);
            match event.kind {
                17 | 28 => conn.hand_on(event.property, event.kind == 17),
                29 => conn.lost(event.property.atom),
                30 => conn.answer(event.request),
                _ => {}
            }
        }
    }

    /// Megabytes of text, every line different, with multi-byte letters.
    fn long_text() -> String {
        (0..300_000).map(|line| format!("{line} \u{449}\u{443}\u{43a}\u{430}\n")).collect()
    }

    /// A `width`-square picture that does not compress: some pixels clear, some half clear with
    /// a colour of their own.
    fn noise(width: usize) -> Vec<u32> {
        let mut seed = 0x2545_f491u32;
        let pixel = |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        (0..width * width).map(pixel).collect()
    }

    /// Another program's clipboard on a thread of its own: a window copying something to
    /// clipboard `selection` and serving it as long as the test runs. Returns once X has heard
    /// of the copy; then sends whether a copy is going somewhere in pieces.
    fn copied_elsewhere(selection: &'static CStr, copy: impl FnOnce(&mut SystemClipboard) + Send + 'static) -> std::sync::mpsc::Receiver<bool> {
        let (ready, copied) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut owner = clipboard(selection).unwrap();
            copy(&mut owner);
            let owner = owner.0;
            // X has registered the copy by the time it answers a query sent after it.
            owner.property(owner.window, owner.mailbox, false);
            ready.send(false).unwrap();
            loop {
                serve(&owner);
                ready.send(!owner.going.borrow().is_empty()).ok();
            }
        });
        copied.recv().unwrap();
        copied
    }

    /// The types a connection's clipboard owner offers, by name.
    fn kinds_offered(reader: &Connection) -> Vec<String> {
        let asked = Instant::now();
        // SAFETY: the display is open, the window the connection's own; the names X returns end
        // in zeros and are copied before they are freed.
        unsafe {
            (reader.x.XConvertSelection)(reader.display, reader.selection, reader.targets, reader.mailbox, reader.window, 0);
            (reader.x.XFlush)(reader.display);
            reader.heard(reader.window, 31, asked).expect("an answer");
            let naming = Naming::open().unwrap();
            let name = |kind: c_ulong| {
                let name = (naming.XGetAtomName)(reader.display, kind);
                let text = CStr::from_ptr(name).to_string_lossy().into_owned();
                (reader.x.XFree)(name.cast());
                text
            };
            let (_, _, kinds) = reader.property(reader.window, reader.mailbox, true).unwrap_or_default();
            kinds.as_chunks().0.iter().map(|kind| name(c_ulong::from_ne_bytes(*kind))).collect()
        }
    }

    library!(Naming, ["libX11.so.6", "libX11.so"], {
        fn XGetAtomName(Display, Atom) -> *mut c_char;
    });

    #[test]
    fn keys_held_with_others_are_news_when_they_go_down_and_come_up() {
        // Left Alt goes down with Control held (X reports the state before it); then it comes
        // up, with itself held.
        let ctrl_alt = Mods { ctrl: true, alt: true, shift: false };
        assert_eq!(held_after(0xffe9, 4, true), Some(ctrl_alt));
        assert_eq!(held_after(0xffe9, 4 | 8, false), Some(Mods::CTRL));
        assert_eq!((held_after(0xffe2, 0, true), held_after(0xffe7, 8, false)), (Some(Mods::SHIFT), Some(Mods::NONE)));
        // Any other key is no such news; the menu key is a key of its own.
        assert_eq!((held_after(0x61, 8, true), key(0xff67)), (None, Some(Key::Menu)));
        // The zoom keys, on the main keyboard and the number pad.
        assert_eq!([key(0x3d), key(0x2d), key(0xffad), key(0x2b), key(0xffab)], [Some(Key::Char('=')), Some(Key::Char('-')), Some(Key::Char('-')), Some(Key::Char('+')), Some(Key::Char('+'))]);
    }

    /// Two clipboards on one display: one copies, the other pastes. (Under a private clipboard
    /// name, leaving the real one alone.)
    #[test]
    fn text_copied_in_one_window_is_pasted_in_another() {
        let Some(mut reader) = clipboard(c"TAUT_TEST") else { return eprintln!("no display: not tested") };
        assert_eq!(reader.get(), None, "nobody has copied anything yet");
        copied_elsewhere(c"TAUT_TEST", |owner| owner.set("copied: \u{17c}\u{f3}\u{142}w \u{1f44d}"));
        assert_eq!(reader.get().as_deref(), Some("copied: \u{17c}\u{f3}\u{142}w \u{1f44d}"));
        assert_eq!((kinds_offered(&reader.0), reader.picture(), reader.files()), (vec!["TARGETS".to_string(), "UTF8_STRING".into()], None, vec![]), "it is text, and nothing else");
    }

    /// A copy too large for one reply goes between windows in pieces and arrives whole. (Under
    /// a private clipboard name.)
    #[test]
    fn a_copy_too_large_for_one_answer_arrives_in_pieces() {
        let Some(mut reader) = clipboard(c"TAUT_TEST_LARGE") else { return eprintln!("no display: not tested") };
        let in_pieces = copied_elsewhere(c"TAUT_TEST_LARGE", |owner| owner.set(&long_text()));
        assert!(reader.get().is_some_and(|pasted| pasted == long_text()), "what was pasted is what was copied");
        assert!(in_pieces.try_iter().any(|it_was| it_was), "it was too large to go in one piece");
    }

    /// A program asked for one type hands over another in pieces (xclip does: it serves what it
    /// has, whatever is asked). That is no answer; but its pieces are taken to the last, so it
    /// is not left mid-transfer, and the next thing asked arrives whole.
    #[test]
    fn pieces_of_another_kind_than_was_asked_for_are_taken_to_the_last() {
        let Some(mut reader) = clipboard(c"TAUT_TEST_OTHER") else { return eprintln!("no display: not tested") };
        let (tell, on_its_way) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let owner = conn(c"TAUT_TEST_OTHER").unwrap();
            let text: Rc<[u8]> = long_text().into_bytes().into();
            owner.set(vec![(owner.utf8, text.clone())]);
            owner.property(owner.window, owner.mailbox, false);
            tell.send(false).unwrap();
            loop {
                // SAFETY: the events come from the owner's own display; a type-30 one is a
                // request, and the reply has X's event layout.
                unsafe {
                    let (mut event, mut reply): (XEvent, XEvent) = (zeroed(), zeroed());
                    (owner.x.XNextEvent)(owner.display, &mut event);
                    let asked = event.request;
                    if matches!(event.kind, 17 | 28) {
                        owner.hand_on(event.property, event.kind == 17);
                    }
                    if event.kind == 30 {
                        owner.hand_over(asked.requestor, asked.property, owner.utf8, &text);
                        reply.selection = SelectionEvent {
                            kind: 31,
                            serial: 0,
                            sent: 1,
                            display: owner.display,
                            requestor: asked.requestor,
                            selection: asked.selection,
                            target: asked.target,
                            property: asked.property,
                            time: asked.time,
                        };
                        (owner.x.XSendEvent)(owner.display, asked.requestor, 0, 0, &mut reply);
                        (owner.x.XFlush)(owner.display);
                    }
                }
                // After each action the owner says whether a copy is still going.
                tell.send(!owner.going.borrow().is_empty()).ok();
            }
        });
        on_its_way.recv().unwrap();
        assert_eq!(reader.picture(), None, "megabytes of text are no picture");
        let mut still = true;
        while let Ok(now) = on_its_way.recv_timeout(Duration::from_millis(200)) {
            still = now;
        }
        assert!(!still, "its owner is not left in the middle of handing it over");
        assert!(reader.get().is_some_and(|pasted| pasted == long_text()), "and what is asked of it next arrives whole");
    }

    /// A picture copied in one window pastes in another unchanged, clear pixels and all: a
    /// small one in one reply, a large one in pieces. And only the picture is there.
    #[test]
    fn a_picture_copied_in_one_window_is_pasted_in_another() {
        let Some(mut reader) = clipboard(c"TAUT_TEST_PICTURE") else { return eprintln!("no display: not tested") };
        let small = vec![0xffff_0000, 0x8000_ff00, 0x0000_00ff, 0x0000_0000, 0xff12_3456, 0x01ff_ffff];
        let copy = small.clone();
        let in_pieces = copied_elsewhere(c"TAUT_TEST_PICTURE", move |owner| owner.set_picture(3, &copy));
        assert_eq!(reader.picture(), Some((3, small)));
        assert_eq!((kinds_offered(&reader.0), reader.get(), reader.files()), (vec!["TARGETS".to_string(), "image/png".into()], None, vec![]), "it is a picture, and nothing else");
        assert!(in_pieces.try_iter().all(|it_was| !it_was), "it went in one piece");

        let Some(mut reader) = clipboard(c"TAUT_TEST_PICTURE_LARGE") else { return };
        let in_pieces = copied_elsewhere(c"TAUT_TEST_PICTURE_LARGE", |owner| owner.set_picture(400, &noise(400)));
        assert!(reader.picture() == Some((400, noise(400))), "what was pasted is what was copied");
        assert!(in_pieces.try_iter().any(|it_was| it_was), "it was too large to go in one piece");
    }

    /// Files copied in one window paste in another, whatever their names; both as a URI list
    /// and as file managers pass them.
    #[test]
    fn files_copied_in_one_window_are_pasted_in_another() {
        let Some(mut reader) = clipboard(c"TAUT_TEST_FILES") else { return eprintln!("no display: not tested") };
        let files = ["/tmp/one.txt", "/home/me/two words/\u{17c}\u{f3}\u{142}w #1 100%.png", "/tmp/new\nline"].map(PathBuf::from);
        let copy = files.clone();
        copied_elsewhere(c"TAUT_TEST_FILES", move |owner| owner.set_files(&copy));
        assert_eq!(reader.files(), files);
        let kinds = ["TARGETS", "text/uri-list", "x-special/gnome-copied-files"].map(String::from);
        assert_eq!((kinds_offered(&reader.0), reader.get(), reader.picture()), (kinds.to_vec(), None, None), "they are files, and nothing else");
        // The file managers' type holds the same files, under a line saying they are copied.
        let copied = reader.0.get(reader.0.atom(COPIED_FILES)).unwrap();
        assert_eq!((copied.starts_with(b"copy\nfile:///tmp/one.txt\n"), files_in(&copied)), (true, files.to_vec()));
    }

    /// What is put on the clipboard replaces everything on it, of every type: for the copying
    /// window (which does not ask X about what it holds) and for every other. And when another
    /// program copies, what was copied here is gone.
    #[test]
    fn what_is_copied_takes_the_place_of_everything_on_the_clipboard() {
        let Some(mut ours) = clipboard(c"TAUT_TEST_PLACE") else { return eprintln!("no display: not tested") };
        ours.set_picture(1, &[0xff00_00ff]);
        assert_eq!((ours.get(), ours.picture(), ours.files()), (None, Some((1, vec![0xff00_00ff])), vec![]));
        ours.set_files(&[PathBuf::from("/tmp/one.txt")]);
        assert_eq!((ours.get(), ours.picture(), ours.files()), (None, None, vec![PathBuf::from("/tmp/one.txt")]));
        ours.set("text");
        assert_eq!((ours.get().as_deref(), ours.picture(), ours.files()), (Some("text"), None, vec![]));
        ours.set_files(&[]);
        assert_eq!((ours.get(), ours.picture(), ours.files()), (None, None, vec![]), "no files are still not text");
        // Another program copies: X says so, and what was held here is dropped. (X may say
        // other things first: that the keyboard changed, for one.)
        copied_elsewhere(c"TAUT_TEST_PLACE", |owner| owner.set("theirs"));
        let asked = Instant::now();
        while !ours.0.held.borrow().is_empty() && asked.elapsed() < Duration::from_secs(2) {
            serve(&ours.0);
        }
        let held = ours.0.held.borrow().len();
        assert_eq!((held, ours.get().as_deref(), ours.files()), (0, Some("theirs"), vec![]));
    }

    /// Has xclip copy something to an X selection as one type, and says whether what is then
    /// pasted here is `it`. (xclip hands ownership to a background process, which takes a
    /// moment to claim it: until then the old content remains. And that another program owns
    /// the selection is news the clipboard here must read, as a window's loop does.)
    fn copied_by_xclip(ours: &mut SystemClipboard, selection: &str, kind: &str, bytes: &[u8], mut it: impl FnMut(&mut SystemClipboard) -> bool) -> bool {
        let mut theirs = Command::new("xclip").args(["-selection", selection, "-t", kind, "-i"]).stdin(Stdio::piped()).spawn().expect("xclip");
        theirs.stdin.take().unwrap().write_all(bytes).unwrap();
        theirs.wait().unwrap();
        let asked = Instant::now();
        loop {
            // SAFETY: the display is the connection's own.
            while unsafe { (ours.0.x.XPending)(ours.0.display) } > 0 {
                serve(&ours.0);
            }
            let is_it = it(ours);
            if is_it || asked.elapsed() > Duration::from_secs(10) {
                return is_it;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// What xclip pastes from an X selection as one type while a connection owns it; and
    /// whether any of it went in pieces.
    fn pasted_by_xclip(ours: &Connection, selection: &str, kind: &str) -> (Vec<u8>, bool) {
        let paste = Command::new("xclip").args(["-selection", selection, "-t", kind, "-o"]).stdout(Stdio::piped()).spawn().expect("xclip");
        let (done, pasted) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(paste.wait_with_output().unwrap().stdout));
        let mut in_pieces = false;
        loop {
            // SAFETY: the display is the connection's own.
            while unsafe { (ours.x.XPending)(ours.display) } > 0 {
                serve(ours);
                in_pieces |= !ours.going.borrow().is_empty();
            }
            if let Ok(pasted) = pasted.recv_timeout(Duration::from_millis(5)) {
                return (pasted, in_pieces);
            }
        }
    }

    /// The real clipboard against another program (xclip): what it copies pastes here, and what
    /// is copied here it pastes. It writes the clipboard, so it runs only when asked, best on a
    /// display of its own: `xvfb-run cargo test -p taut -- --ignored`
    #[test]
    #[ignore]
    fn the_clipboard_is_shared_with_other_programs() {
        let mut ours = clipboard(c"CLIPBOARD").expect("a display");
        assert!(copied_by_xclip(&mut ours, "clipboard", "UTF8_STRING", b"from xclip", |ours| ours.get().as_deref() == Some("from xclip")));
        ours.set("from taut");
        assert_eq!(pasted_by_xclip(&ours.0, "clipboard", "UTF8_STRING").0, b"from taut");

        // A picture both ways: xclip gets the PNG of what was copied here, and the PNG it
        // copies pastes as the picture here.
        let (picture, theirs) = (vec![0xffff_0000, 0x8000_ff00, 0x0000_00ff, 0x0000_0000], vec![0xff00_ff00, 0x4012_3456]);
        ours.set_picture(2, &picture);
        let (png, _) = pasted_by_xclip(&ours.0, "clipboard", "image/png");
        assert_eq!((crate::png::decode(&png), pasted_by_xclip(&ours.0, "clipboard", "TARGETS").0), (Some((2, picture)), b"TARGETS\nimage/png\n".to_vec()));
        assert!(copied_by_xclip(&mut ours, "clipboard", "image/png", &crate::png::encode_rgba(2, &theirs).unwrap(), |ours| ours.picture() == Some((2, theirs.clone()))));
        assert_eq!((ours.get(), ours.files()), (None, vec![]), "xclip has a picture, and nothing else");

        // Files both ways, in both list formats.
        let files = ["/tmp/one.txt", "/tmp/two words/\u{17c}\u{f3}\u{142}w.png"].map(PathBuf::from);
        ours.set_files(&files);
        assert_eq!(pasted_by_xclip(&ours.0, "clipboard", "text/uri-list").0, b"file:///tmp/one.txt\r\nfile:///tmp/two%20words/%C5%BC%C3%B3%C5%82w.png\r\n");
        assert_eq!(pasted_by_xclip(&ours.0, "clipboard", "x-special/gnome-copied-files").0, b"copy\nfile:///tmp/one.txt\nfile:///tmp/two%20words/%C5%BC%C3%B3%C5%82w.png");
        assert_eq!(pasted_by_xclip(&ours.0, "clipboard", "TARGETS").0, b"TARGETS\ntext/uri-list\nx-special/gnome-copied-files\n");
        let listed = b"file:///tmp/three.txt\r\nfile://localhost/tmp/four%20words/%C5%BC%C3%B3%C5%82w.png\r\n";
        let theirs = ["/tmp/three.txt", "/tmp/four words/\u{17c}\u{f3}\u{142}w.png"].map(PathBuf::from);
        assert!(copied_by_xclip(&mut ours, "clipboard", "text/uri-list", listed, |ours| ours.files() == theirs), "{:?}", ours.files());
        let alone = copied_by_xclip(&mut ours, "clipboard", "x-special/gnome-copied-files", b"cut\nfile:///tmp/five.txt", |ours| ours.files() == [PathBuf::from("/tmp/five.txt")]);
        assert!(alone, "as a file manager alone says them: {:?}", ours.files());
        assert_eq!((ours.get(), ours.picture()), (None, None), "xclip has files, and nothing else");
    }

    /// Megabytes from another program, sent in pieces, paste here whole: text and a picture. It
    /// writes the PRIMARY selection (so the tests beside it can run at once), so it runs only
    /// when asked, on a display of its own: `xvfb-run cargo test -p taut -- --ignored`
    #[test]
    #[ignore]
    fn a_large_copy_from_another_program_is_pasted() {
        let mut ours = clipboard(c"PRIMARY").expect("a display");
        let text = long_text();
        assert!(copied_by_xclip(&mut ours, "primary", "UTF8_STRING", text.as_bytes(), |ours| ours.get().is_some_and(|pasted| pasted == text)), "what was pasted is what xclip copied");
        let png = crate::png::encode_rgba(400, &noise(400)).unwrap();
        assert!(png.len() > ours.0.piece(), "{} bytes go in one piece", png.len());
        assert!(copied_by_xclip(&mut ours, "primary", "image/png", &png, |ours| ours.picture() == Some((400, noise(400)))), "the picture xclip copied is the one pasted");
    }

    /// Megabytes copied here paste whole in another program, sent in pieces: text and a
    /// picture. It writes the SECONDARY selection: only on a display of its own.
    #[test]
    #[ignore]
    fn a_large_copy_is_pasted_by_another_program() {
        let mut ours = clipboard(c"SECONDARY").expect("a display");
        ours.set(&long_text());
        let (pasted, in_pieces) = pasted_by_xclip(&ours.0, "secondary", "UTF8_STRING");
        assert!(pasted == long_text().as_bytes(), "what xclip pasted is what was copied");
        assert!(in_pieces, "it was too large to go in one piece");
        ours.set_picture(400, &noise(400));
        let (pasted, in_pieces) = pasted_by_xclip(&ours.0, "secondary", "image/png");
        assert!(crate::png::decode(&pasted) == Some((400, noise(400))), "the picture xclip pasted is the one copied: {} bytes of it", pasted.len());
        assert!(in_pieces, "it was too large to go in one piece");
    }

    #[test]
    fn the_files_in_a_list_of_addresses_are_found() {
        let list = b"# a note\r\nfile:///home/me/a.txt\r\nfile://machine/tmp/two%20words/%C5%BC%C3%B3%C5%82w.png\r\nhttp://example.org/x\r\n\r\nfile:/short\nFILE:///loud%2fpath\r\nfile:///100%/5%2\r\nfile:///no%00thing\r\nfile://nothing\r\nfile:relative\0";
        let files = ["/home/me/a.txt", "/tmp/two words/\u{17c}\u{f3}\u{142}w.png", "/short", "/loud/path", "/100%/5%2"];
        assert_eq!(files_in(list), files.map(PathBuf::from));
        assert_eq!(files_in(b""), Vec::<PathBuf>::new());
        // As file managers pass them: a first line that is no URI.
        assert_eq!(files_in(b"copy\nfile:///tmp/a%20b\nfile:///tmp/c"), ["/tmp/a b", "/tmp/c"].map(PathBuf::from));
        assert_eq!((files_in(b"cut\nfile:///tmp/a"), files_in(b"copy")), (vec![PathBuf::from("/tmp/a")], vec![]));
    }

    /// A decimal scale reads as Rust reads it, to the last bit, from whole dpi to four decimal
    /// places; anything else is no scale.
    #[test]
    fn a_scale_written_in_decimals_is_read_as_rust_reads_it() {
        for written in (0..20_000).map(|n| format!("{}.{:02}", n / 100, n % 100)).chain((0..1000).map(|n| format!("{n}"))).chain((0..10_000).map(|n| format!("1.{n:04}"))) {
            assert_eq!(decimal(&written).map(f32::to_bits), written.parse::<f32>().ok().map(f32::to_bits), "{written}");
        }
        assert_eq!([".5", "2.", "096"].map(decimal), [Some(0.5), Some(2.0), Some(96.0)]);
        assert_eq!(["", ".", "-1", "+1", "1e3", "1.5.2", " 96", "96 ", "inf", "NaN", "1,5"].map(decimal), [None; 11]);
    }

    /// Files become the two clipboard lists, whatever their names, and the lists become the
    /// same files again.
    #[test]
    fn files_become_lists_of_addresses_and_back() {
        let here = std::env::current_dir().unwrap();
        let odd = OsString::from_vec(b"/tmp/not \xff utf-8".to_vec());
        let files = [
            PathBuf::from("/tmp/a b/c#d?e%f.txt"),
            PathBuf::from("/home/\u{17c}\u{f3}\u{142}w/\u{1f44d}"),
            PathBuf::from("/tmp/-._~/new\nline"),
            odd.into(),
            PathBuf::from("beside/./here.txt"),
            PathBuf::new(),
        ];
        let [list, copied] = lists_of(&files).map(|list| String::from_utf8(list).unwrap());
        let addresses = ["file:///tmp/a%20b/c%23d%3Fe%25f.txt", "file:///home/%C5%BC%C3%B3%C5%82w/%F0%9F%91%8D", "file:///tmp/-._~/new%0Aline", "file:///tmp/not%20%FF%20utf-8"];
        // A relative path is resolved from the working folder; an empty one is no file and is
        // left out.
        let beside = address_of(&here.join("beside/here.txt")).unwrap();
        let all: Vec<&str> = addresses.into_iter().chain([beside.as_str()]).collect();
        assert_eq!((list, copied), (all.iter().map(|address| format!("{address}\r\n")).collect::<String>(), format!("copy\n{}", all.join("\n"))));
        let back: Vec<PathBuf> = files[..4].iter().cloned().chain([here.join("beside/here.txt")]).collect();
        assert_eq!(lists_of(&files).map(|list| files_in(&list)), [back.clone(), back]);
        assert_eq!(lists_of(&[]), [b"".to_vec(), b"copy".to_vec()]);
    }

    /// Size hints always give a least size; a max size the same, when the window keeps its
    /// size; and a position only when its app gives one, as the user's choice.
    #[test]
    fn a_window_says_how_small_it_may_be_and_where_it_opens() {
        let (free, placed, fixed) = (hints([200, 120], false, false), hints([150, 90], false, true), hints([64, 48], true, false));
        assert_eq!((free.flags, free.min, placed.flags, placed.min), (1 << 4, [200, 120], 1 << 4 | 1, [150, 90]));
        assert_eq!((free.rest, fixed.flags, fixed.min, fixed.rest), ([0; 12], 1 << 4 | 1 << 5, [64, 48], [64, 48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        // No smaller than Xlib takes it to be: a long and 17 ints.
        assert!(size_of::<SizeHints>() >= size_of::<c_long>() + 17 * size_of::<c_int>());
    }

    /// Maximized only when both horizontally and vertically, not one; fullscreen by its own
    /// atom, maximized or not.
    #[test]
    fn a_window_fills_its_screen_by_the_names_of_its_state() {
        let [wide, high, whole, other] = [11, 12, 13, 14];
        let stands = |state: &[Atom]| stands(state, [wide, high, whole]);
        assert_eq!([stands(&[other, high, wide]), stands(&[whole]), stands(&[wide, whole, high])], [[true, false], [false, true], [true, true]]);
        assert_eq!([stands(&[]), stands(&[wide]), stands(&[high, other]), stands(&[wide, whole])], [[false, false], [false, false], [false, false], [false, true]]);
    }

    /// A window following the pointer itself: held by its strip all of it moves; held by an
    /// edge or corner, the held part moves and the opposite stays, never below the least size.
    #[test]
    fn a_window_held_by_its_strip_or_an_edge_goes_by_how_far_the_pointer_went() {
        let (was, least) = ([100, 80, 300, 200], [120, 90]);
        let goes = |edges: [i8; 2], by: [c_int; 2]| dragged(edges, was, by, least);
        assert_eq!([goes([0, 0], [40, -30]), goes([0, 0], [-500, 7])], [[140, 50, 300, 200], [-400, 87, 300, 200]]);
        // The right and bottom edges: it grows and shrinks from its corner.
        assert_eq!([goes([1, 0], [50, 999]), goes([0, 1], [999, -20]), goes([1, 1], [-30, 40])], [[100, 80, 350, 200], [100, 80, 300, 180], [100, 80, 270, 240]]);
        // The left and top edges: the far edges stay.
        assert_eq!([goes([-1, 0], [50, 999]), goes([0, -1], [999, -20]), goes([-1, -1], [-30, 40])], [[150, 80, 250, 200], [100, 60, 300, 220], [70, 120, 330, 160]]);
        assert_eq!([goes([-1, 1], [10, 10]), goes([1, -1], [10, 10])], [[110, 80, 290, 210], [100, 90, 310, 190]]);
        // Never below the least size, whichever edge is pushed in: the edge stops there.
        assert_eq!([goes([1, 1], [-900, -900]), goes([-1, -1], [900, 900])], [[100, 80, 120, 90], [280, 190, 120, 90]]);
    }

    #[test]
    fn a_character_is_found_in_the_first_font_that_has_it() {
        let place = |name: &str| (CString::new(name).unwrap(), 0);
        let page = |first, font, c: char| Page { first, font, bits: std::array::from_fn(|word| if word == (c as usize & 0xff) / 32 { 1 << (c as u32 % 32) } else { 0 }) };
        let pages = vec![page(0, 0, 'a'), page(0x300, 1, '\u{3b1}'), page(0x300, 2, '\u{3b1}'), page(0x300, 2, '\u{3b2}'), page(0x2100, 1, '\u{2192}')];
        let spares = Spares { files: vec![place("latin"), place("greek"), place("more")], pages };
        let having = |c| spares.with(c).map(|(file, _)| file.to_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(having('a'), ["latin"]);
        assert_eq!(having('\u{3b1}'), ["greek", "more"], "the one listed first comes first");
        assert_eq!(having('\u{3b2}'), ["more"]);
        assert_eq!(having('\u{2192}'), ["greek"]);
        assert_eq!(having('\u{2193}'), [""; 0], "on a page that is listed, but not on it");
        assert_eq!(having('\u{4e2d}'), [""; 0], "on no page at all");
    }

    /// With the installed fonts: a character the desktop's font lacks and another font has is
    /// drawn from that one, not as the missing-glyph box; and a line holding it is as wide as
    /// its glyphs.
    #[test]
    fn a_character_the_font_lacks_is_drawn_from_another_font() {
        let _alone = alone();
        let Some(mut text) = SystemText::new() else { return eprintln!("no fonts: not tested") };
        let (font, with) = (Font::new(14.0), text.face(Font::new(14.0), 1.0));
        let lacking: Vec<char> = ['\u{2192}', '\u{2603}', '\u{5d0}', '\u{e01}', '\u{4e2d}', '\u{e0ff}'].into_iter().filter(|c| !text.fonts.has(with.1, *c)).collect();
        let Some(c) = lacking.into_iter().find(|c| text.fonts.stand_in(with.0, *c).is_some()) else { return eprintln!("the desktop's font lacks nothing another font has: not tested") };
        #[cfg(feature = "shaping")]
        let (found, missing) = (text.glyph(with, c, 0).cover.clone(), text.glyph(with, '\u{10ffff}', 0).cover.clone());
        #[cfg(not(feature = "shaping"))]
        let (found, missing) = (text.glyph(with, c).cover.clone(), text.glyph(with, '\u{10ffff}').cover.clone());
        assert!(found.iter().any(|cover| *cover > 0) && found != missing, "{c:?} is drawn, and not as a box");
        #[cfg(feature = "shaping")]
        let (advance, mut stops) = (text.glyph(with, c, 0).advance, Vec::new());
        #[cfg(not(feature = "shaping"))]
        let (advance, mut stops) = (text.glyph(with, c).advance, Vec::new());
        let (a, wide) = (text.measure("a", font, 1.0, None), text.measure(&format!("a{c}a"), font, 1.0, Some(&mut stops)));
        assert_eq!((wide, stops), (a + advance + a, vec![a, a + advance, a + advance + a]));
    }

    // What only the tests ask of X: resize or move a window, a letter's keycode, fill a window
    // with one colour, and read the screen.
    library!(Asking, ["libX11.so.6", "libX11.so"], {
        fn XResizeWindow(Display, XWindow, c_uint, c_uint) -> c_int;
        fn XMoveWindow(Display, XWindow, c_int, c_int) -> c_int;
        fn XMoveResizeWindow(Display, XWindow, c_int, c_int, c_uint, c_uint) -> c_int;
        fn XKeysymToKeycode(Display, c_ulong) -> c_uchar;
        fn XSetWindowBackground(Display, XWindow, c_ulong) -> c_int;
        fn XClearWindow(Display, XWindow) -> c_int;
        fn XGetImage(Display, XWindow, c_int, c_int, c_uint, c_uint, c_ulong, c_int) -> *mut Picture;
        fn XGetPixel(*mut Picture, c_int, c_int) -> c_ulong;
    });

    /// The start of `XImage`: its size. (The rest is read through X's own function.)
    #[repr(C)]
    struct Picture {
        width: c_int,
        height: c_int,
    }

    /// The two-window test app's state, and what it noted in its last run.
    struct Two {
        prefs: SecondWindow,
        dark: bool,
        pressed: u32,
        typed: [String; 2],
        /// The files dropped in each window.
        dropped: [Vec<PathBuf>; 2],
        /// A new title for the second window, asked once.
        rename: Option<&'static str>,
        /// Where things are: the first window's button and text box, then the second window's
        /// check box and text box.
        at: [Rect; 4],
    }

    /// A first window with a button and a text box; and while open, a second with a check box
    /// and a text box, lying where the first's button and text box are.
    fn two(app: &mut Two, ui: &mut Ui) {
        let button = ui.button("In the first window");
        app.pressed += button.clicked as u32;
        app.dropped[0].extend(ui.files_dropped(&button).unwrap_or_default());
        (app.at[0], app.at[1]) = (button.rect, ui.text_box(&mut app.typed[0], "").rect);
        if ui.window(&mut app.prefs, "Preferences") {
            let tick = ui.checkbox(&mut app.dark, "Dark");
            app.dropped[1].extend(ui.files_dropped(&tick).unwrap_or_default());
            (app.at[2], app.at[3]) = (tick.rect, ui.text_box(&mut app.typed[1], "").rect);
            if let Some(title) = app.rename.take() {
                ui.set_title(title);
            }
            ui.close();
        }
    }

    /// Serializes tests that open windows or ask Fontconfig for fonts: Xlib's input methods and
    /// Fontconfig keep process-wide state, and neither may be started by one thread while
    /// another tears it down. (A program opens its windows on one thread; only tests do not.)
    pub(crate) fn alone() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ONE.lock().unwrap_or_else(|failed| failed.into_inner())
    }

    /// A test app's windows, whatever app; and with them the guard keeping the test alone. (The
    /// guard comes first, so it drops last: after the windows.)
    type Windows = Host<Box<dyn FnMut(&mut Ui)>>;
    type Alone = (std::sync::MutexGuard<'static, ()>, Windows);
    type TwoWindows = (std::sync::MutexGuard<'static, ()>, Windows, Rc<RefCell<Two>>);

    /// Opens a window for an app on the test display and runs the app twice, so it knows where
    /// its boxes are. Nothing without a display or font. (On no bus: nothing a test window asks
    /// reaches the desktop.)
    fn host(window: Window, app: impl FnMut(&mut Ui) + 'static) -> Option<Alone> {
        host_on(alone(), Session::on(None), window, app)
    }

    /// The same in a session a test started on a private bus, for a test already holding the
    /// guard.
    fn host_on(alone: std::sync::MutexGuard<'static, ()>, session: Session, window: Window, app: impl FnMut(&mut Ui) + 'static) -> Option<Alone> {
        let mut host = Host::open(Xlib::open()?, SystemText::new()?, session, window, Box::new(app) as Box<dyn FnMut(&mut Ui)>)?;
        host.turn();
        host.rerun = true;
        host.turn();
        Some((alone, host))
    }

    /// These tests' two windows and their app's state.
    fn two_windows() -> Option<TwoWindows> {
        let prefs = SecondWindow::new(200.0, 120.0);
        let app = Rc::new(RefCell::new(Two { prefs, dark: false, pressed: 0, typed: Default::default(), dropped: Default::default(), rename: None, at: [Rect::EMPTY; 4] }));
        let described = app.clone();
        let (alone, host) = host(Window { title: "taut test", width: 320.0, height: 200.0, ..Default::default() }, move |ui| two(&mut described.borrow_mut(), ui))?;
        Some((alone, host, app))
    }

    /// Has X deliver an event to a window as if it happened there, and the host take a turn.
    fn send<App: FnMut(&mut Ui)>(host: &mut Host<App>, to: XWindow, mut event: XEvent) {
        let conn = host.conn.clone();
        // SAFETY: the event has an X event layout, and the display is the host's.
        unsafe { (conn.x.XSendEvent)(conn.display, to, 0, 0, &mut event) };
        // X has delivered it by the time it answers a later query.
        conn.property(to, conn.mailbox, false);
        host.turn();
    }

    /// An input event at a place in a window: `KeyPress` 2, `ButtonPress` 4, `ButtonRelease` 5,
    /// `MotionNotify` 6, `EnterNotify` 7, `LeaveNotify` 8; and `FocusOut` 10, which names the
    /// window in the same place. (`detail`: button or keycode.)
    fn input(to: XWindow, kind: c_int, [x, y]: [c_int; 2], detail: c_uint) -> XEvent {
        // SAFETY: an event is plain numbers; these types have this layout.
        let mut event: XEvent = unsafe { zeroed() };
        event.input = InputEvent { kind, serial: 0, sent: 1, display: null_mut(), window: to, root: 0, child: 0, time: 0, x, y, x_root: 0, y_root: 0, state: 0, detail };
        event
    }

    /// A client message of five numbers under an atom, as another program sends one.
    fn message(to: XWindow, name: Atom, data: [c_long; 5]) -> XEvent {
        // SAFETY: as above; a client message has this layout.
        let mut event: XEvent = unsafe { zeroed() };
        event.client = ClientEvent { kind: 33, serial: 0, sent: 1, display: null_mut(), window: to, message: name, format: 32, data };
        event
    }

    /// A box's middle, in its window's pixels.
    fn middle(of: Rect, scale: f32) -> [c_int; 2] {
        [((of.x + of.w / 2.0) * scale) as c_int, ((of.y + of.h / 2.0) * scale) as c_int]
    }

    /// A left click at a place in a window.
    fn click<App: FnMut(&mut Ui)>(host: &mut Host<App>, to: XWindow, at: [c_int; 2]) {
        for (kind, detail) in [(6, 0), (4, 1), (5, 1)] {
            send(host, to, input(to, kind, at, detail));
        }
    }

    /// A window's size in pixels; nothing if X has no such window.
    fn size_at_x(conn: &Connection, window: XWindow) -> Option<[c_uint; 2]> {
        let (mut parent, mut place, mut size, mut rest) = (0, [0; 2], [0; 2], [0; 2]);
        // SAFETY: X fills these in, or reports to `shrug` that the window does not exist.
        let there = unsafe { (conn.x.XGetGeometry)(conn.display, window, &mut parent, &mut place[0], &mut place[1], &mut size[0], &mut size[1], &mut rest[0], &mut rest[1]) };
        (there != 0).then_some(size)
    }

    /// The text in a window property.
    fn text_at_x(conn: &Connection, window: XWindow, name: &CStr) -> Option<String> {
        conn.property(window, conn.atom(name), false).map(|(_, _, bytes)| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The program's icons are on its window for task bars and window lists: each width,
    /// height, then pixels.
    #[test]
    fn a_window_holds_the_programs_pictures_of_itself() {
        let (small, big) = ([0xff10_2030u32; 4], [0x8040_5060u32; 6]);
        let icons = [(2, &small[..]), (3, &big[..])];
        let asked = Window { title: "taut pictured", width: 320.0, height: 200.0, icons: &icons, ..Default::default() };
        let Some((_alone, host)) = host(asked, |ui: &mut Ui| _ = ui.label("One")) else { return eprintln!("no display: not tested") };
        // X keeps 32 bits of each number and returns them as C longs.
        let kept: Vec<u32> = host.conn.numbers(host.windows[0].id, c"_NET_WM_ICON").into_iter().map(|number| number as u32).collect();
        assert_eq!(kept, [2, 2, 0xff10_2030, 0xff10_2030, 0xff10_2030, 0xff10_2030, 3, 2, 0x8040_5060, 0x8040_5060, 0x8040_5060, 0x8040_5060, 0x8040_5060, 0x8040_5060]);
    }

    /// A second window is an X window like the first: its own title, the size asked, mapped,
    /// and in the first one's group.
    #[test]
    fn a_second_window_is_opened_with_its_title_and_size() {
        let Some((_alone, host, _app)) = two_windows() else { return eprintln!("no display: not tested") };
        let (conn, scale) = (host.conn.clone(), host.scale);
        let [first, second] = [host.windows[0].id, host.windows[1].id];
        assert_eq!((host.windows.len(), host.windows[1].known), (2, host.ui.windows().next().map(|(known, ..)| known)));
        assert_eq!((text_at_x(&conn, first, c"_NET_WM_NAME").as_deref(), text_at_x(&conn, second, c"_NET_WM_NAME").as_deref()), (Some("taut test"), Some("Preferences")));
        assert_eq!(text_at_x(&conn, second, c"WM_NAME").as_deref(), Some("Preferences"), "and under the old kind of name");
        // Both carry `WM_CLASS`, as task bars want.
        let program = std::env::current_exe().unwrap().file_stem().unwrap().to_string_lossy().into_owned();
        assert_eq!([first, second].map(|window| text_at_x(&conn, window, c"WM_CLASS")), [(); 2].map(|_| Some(format!("{program}\0{program}\0"))));
        assert_eq!(size_at_x(&conn, first), Some([(320.0 * scale) as c_uint, (200.0 * scale) as c_uint]));
        assert_eq!(size_at_x(&conn, second), Some([(200.0 * scale) as c_uint, (120.0 * scale) as c_uint]));
        // It accepts drops, takes `WM_DELETE_WINDOW`, and shares the first's window group.
        assert_eq!(conn.numbers(second, c"XdndAware"), [5]);
        assert_eq!(conn.numbers(second, c"WM_PROTOCOLS"), [conn.atom(c"WM_DELETE_WINDOW")]);
        assert_eq!([first, second].map(|window| conn.numbers(window, c"WM_HINTS")), [[1 << 6, 0, 0, 0, 0, 0, 0, 0, first]; 2]);
    }

    /// Pointer input in the second window goes to its interface, not the first's, and the
    /// reverse; each window shows the cursor its own interface asks for.
    #[test]
    fn a_click_in_a_second_window_is_heard_there_and_not_in_the_first() {
        let Some((_alone, mut host, app)) = two_windows() else { return eprintln!("no display: not tested") };
        let [first, second] = [host.windows[0].id, host.windows[1].id];
        let (at, scale) = (app.borrow().at, host.scale);
        // The second window's check box lies where the first's button does.
        let place = middle(at[2], scale);
        assert!(at[0].contains(pt(place[0] as f32 / scale, place[1] as f32 / scale)), "{at:?}");
        click(&mut host, second, place);
        assert_eq!((app.borrow().dark, app.borrow().pressed), (true, 0));
        click(&mut host, first, place);
        assert_eq!((app.borrow().dark, app.borrow().pressed), (true, 1));
        // The pointer leaves the first window and enters the second's text box: the text cursor
        // there, in that window only. Re-entering the first, the pointer is where it entered,
        // though it has not moved since.
        send(&mut host, first, input(first, 8, [0, 0], 0));
        send(&mut host, second, input(second, 6, middle(at[3], scale), 0));
        assert_eq!((host.windows[0].cursor, host.windows[1].cursor, host.ui.pointer()), (Cursor::Arrow, Cursor::Text, None));
        send(&mut host, first, input(first, 7, [30, 40], 0));
        assert_eq!(host.ui.pointer(), Some(pt(30.0 / scale, 40.0 / scale)));
    }

    /// The person closes the second window through the window manager: the app hears, stops
    /// describing it, and it goes. The first stays until closed the same way: then the program
    /// ends.
    #[test]
    fn a_second_window_the_person_closes_is_closed_by_the_app() {
        let Some((_alone, mut host, app)) = two_windows() else { return eprintln!("no display: not tested") };
        let (conn, [first, second]) = (host.conn.clone(), [host.windows[0].id, host.windows[1].id]);
        let closing = |window| message(window, conn.atom(c"WM_PROTOCOLS"), [conn.atom(c"WM_DELETE_WINDOW") as c_long, 0, 0, 0, 0]);
        send(&mut host, second, closing(second));
        assert_eq!((app.borrow().prefs.open, host.windows.len(), host.ui.windows().count(), host.done), (false, 1, 0, false));
        assert_eq!((size_at_x(&conn, second), size_at_x(&conn, first).is_some()), (None, true), "X has the first window, and no second one");
        send(&mut host, first, closing(first));
        assert!(host.done);
    }

    /// The app stops describing the second window: it closes. Described again, it reopens as a
    /// new window (X is asked for 1×1 if the app gives no size). When the person resizes it the
    /// app is told; when the app retitles it, X is.
    #[test]
    fn a_second_window_follows_what_the_app_describes() {
        let Some((_alone, mut host, app)) = two_windows() else { return eprintln!("no display: not tested") };
        let (conn, scale, second) = (host.conn.clone(), host.scale, host.windows[1].id);
        for open in [false, true, false] {
            (app.borrow_mut().prefs, host.rerun) = (SecondWindow { open, ..SecondWindow::new(0.0, 0.0) }, true);
            host.turn();
            let (shown, asked) = (host.windows.get(1).map_or(second, |shown| shown.id), host.windows.get(1).map(|shown| shown.size));
            assert_eq!((host.windows.len(), size_at_x(&conn, shown).is_some(), asked), (1 + open as usize, open, open.then_some([1, 1])));
        }
        (app.borrow_mut().prefs, host.rerun) = (SecondWindow::new(200.0, 120.0), true);
        host.turn();
        let again = host.windows[1].id;
        assert_eq!((host.windows.len(), size_at_x(&conn, again)), (2, Some([(200.0 * scale) as c_uint, (120.0 * scale) as c_uint])));

        // X is asked to resize, as a window manager does when the person drags an edge; the app
        // is told once X confirms.
        let (asking, asked) = (Asking::open().unwrap(), Instant::now());
        // SAFETY: the window is the host's own, on its display.
        unsafe { (asking.XResizeWindow)(conn.display, again, (300.0 * scale) as c_uint, (150.0 * scale) as c_uint) };
        while app.borrow().prefs.width.round() != 300.0 && asked.elapsed() < Duration::from_secs(2) {
            conn.property(again, conn.mailbox, false);
            host.turn();
        }
        assert_eq!((app.borrow().prefs.width.round(), app.borrow().prefs.height.round()), (300.0, 150.0));
        (app.borrow_mut().rename, host.rerun) = (Some("Renamed"), true);
        host.turn();
        assert_eq!([c"_NET_WM_NAME", c"WM_NAME"].map(|name| text_at_x(&conn, again, name)), [Some("Renamed".into()), Some("Renamed".into())]);
        assert_eq!(text_at_x(&conn, host.windows[0].id, c"_NET_WM_NAME").as_deref(), Some("taut test"), "the first window keeps its own");
    }

    /// When the windows close, nothing of them stays with X: seen from another connection, both
    /// exist while the host does and are gone with it.
    #[test]
    fn nothing_is_left_at_x_when_the_windows_are_gone() {
        let Some((_alone, host, _app)) = two_windows() else { return eprintln!("no display: not tested") };
        let (watcher, windows) = (conn(c"TAUT_TEST_WINDOWS").unwrap(), [host.windows[0].id, host.windows[1].id]);
        // X has created them by the time it answers a later query.
        host.conn.property(windows[0], host.conn.mailbox, false);
        assert!(windows.iter().all(|window| size_at_x(&watcher, *window).is_some()));
        drop(host);
        assert_eq!(windows.map(|window| size_at_x(&watcher, window)), [None, None]);
    }

    /// Typing in the second window goes to its text box, not the first's, and the reverse; with
    /// the second gone, the first types as before. (Keys are sent, not pressed, but on a
    /// display in use a real key press could land here: so it runs only when asked, on a
    /// display of its own.)
    #[test]
    #[ignore]
    fn each_window_is_typed_into_by_itself() {
        let (_alone, mut host, app) = two_windows().expect("a display");
        if host.conn.managed() {
            return eprintln!("a window manager is there, and hands the keyboard from window to window itself: not tested");
        }
        let (conn, scale, at, [first, second]) = (host.conn.clone(), host.scale, app.borrow().at, [host.windows[0].id, host.windows[1].id]);
        let asking = Asking::open().unwrap();
        // SAFETY: the display is the host's own.
        let key = |letter: char| unsafe { (asking.XKeysymToKeycode)(conn.display, letter as c_ulong) } as c_uint;
        click(&mut host, second, middle(at[3], scale));
        "two".chars().for_each(|letter| send(&mut host, second, input(second, 2, [0, 0], key(letter))));
        assert_eq!(app.borrow().typed, ["", "two"]);
        click(&mut host, first, middle(at[1], scale));
        "one".chars().for_each(|letter| send(&mut host, first, input(first, 2, [0, 0], key(letter))));
        assert_eq!(app.borrow().typed, ["one", "two"]);
        // Focus leaves the second window: its interface hears, the first's does not.
        send(&mut host, second, input(second, 10, [0, 0], 0));
        assert_eq!((host.ui.window_focused, host.ui.in_window(host.windows[1].known.unwrap(), |window| window.window_focused)), (true, Some(false)));
        (app.borrow_mut().prefs.open, host.rerun) = (false, true);
        host.turn();
        send(&mut host, first, input(first, 2, [0, 0], key('s')));
        assert_eq!((host.windows.len(), app.borrow().typed[0].as_str()), (1, "ones"));
    }

    /// Another program dragging files, on a thread of its own: a window owning `XdndSelection`
    /// that serves `list`. Returns that window once X has registered it; then sends which
    /// window requested the list, and each client message received (atom and numbers).
    fn files_dragged_from_elsewhere(list: &'static str) -> (XWindow, std::sync::mpsc::Receiver<(Atom, [c_long; 5])>) {
        let (tell, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let source = conn(c"XdndSelection").unwrap();
            source.set(vec![(source.atom(FILES), list.as_bytes().into())]);
            source.property(source.window, source.mailbox, false);
            tell.send((source.window, [0; 5])).unwrap();
            loop {
                // SAFETY: the events come from the source's own display; type 30 is a request,
                // type 33 a client message. The reply has X's event layout.
                unsafe {
                    let (mut event, mut reply): (XEvent, XEvent) = (zeroed(), zeroed());
                    (source.x.XNextEvent)(source.display, &mut event);
                    let asked = event.request;
                    if event.kind == 33 {
                        tell.send((event.client.message, event.client.data)).ok();
                    }
                    if event.kind != 30 {
                        continue;
                    }
                    source.leave(asked.requestor, asked.property, asked.target, list.as_bytes());
                    reply.selection = SelectionEvent {
                        kind: 31,
                        serial: 0,
                        sent: 1,
                        display: source.display,
                        requestor: asked.requestor,
                        selection: asked.selection,
                        target: asked.target,
                        property: asked.property,
                        time: asked.time,
                    };
                    (source.x.XSendEvent)(source.display, asked.requestor, 0, 0, &mut reply);
                    (source.x.XFlush)(source.display);
                    tell.send((0, [asked.requestor as c_long, 0, 0, 0, 0])).ok();
                }
            }
        });
        (heard.recv().unwrap().0, heard)
    }

    /// Files dragged from another program and dropped on the second window reach the second
    /// window's interface, where dropped, not the first's; the source deals with the drop
    /// window throughout. (One XDND selection per display: only when asked, on a display of its
    /// own.)
    #[test]
    #[ignore]
    fn files_let_go_over_a_second_window_reach_its_interface() {
        let (_alone, mut host, app) = two_windows().expect("a display");
        // A window manager places windows in its own time: the check box's screen position is
        // asked after that.
        settle(&mut host);
        let (conn, second) = (host.conn.clone(), host.windows[1].id);
        let (from, heard) = files_dragged_from_elsewhere("file:///tmp/one.txt\r\nfile:///tmp/two%20words.png\r\n");
        let (named, wait) = (|name: &CStr| conn.atom(name), Duration::from_secs(2));
        // The second window's check box on screen.
        let (at, mut place, mut child) = (middle(app.borrow().at[2], host.scale), [0; 2], 0);
        // SAFETY: X fills in where on screen the window point is.
        unsafe { (conn.x.XTranslateCoordinates)(conn.display, second, root(&conn.x, conn.display), at[0], at[1], &mut place[0], &mut place[1], &mut child) };
        send(&mut host, second, message(second, named(c"XdndEnter"), [from as c_long, 5 << 24, named(FILES) as c_long, 0, 0]));
        send(&mut host, second, message(second, named(c"XdndPosition"), [from as c_long, 0, (place[0] << 16 | place[1]) as c_long, 0, named(c"XdndActionCopy") as c_long]));
        assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndStatus"), [second as c_long, 3, 0, 0, named(c"XdndActionCopy") as c_long])), "they may be let go here");
        assert_eq!((host.ui.in_window(host.windows[1].known.unwrap(), |window| window.files_hovered()), host.ui.files_hovered()), (Some(true), false));
        send(&mut host, second, message(second, named(c"XdndDrop"), [from as c_long, 0, 0, 0, 0]));
        assert_eq!(heard.recv_timeout(wait), Ok((0, [second as c_long, 0, 0, 0, 0])), "the list was asked for by the window they were let go on");
        assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndFinished"), [second as c_long, 1, named(c"XdndActionCopy") as c_long, 0, 0])), "and they were taken");
        assert_eq!(app.borrow().dropped, [vec![], vec![PathBuf::from("/tmp/one.txt"), PathBuf::from("/tmp/two words.png")]]);
    }

    /// A window announces drops only once its app asked about them: before, it announces
    /// nothing, and drags over it go unanswered and never reach its interface. From the run its
    /// app first asks, it does, and dropped files reach the app. (One XDND selection per
    /// display: only when asked, on a display of its own.)
    #[test]
    #[ignore]
    fn a_window_takes_what_other_programs_drag_in_only_once_its_app_asks() {
        let (asks, got) = (Rc::new(Cell::new(false)), Rc::new(RefCell::new((Vec::new(), Rect::EMPTY))));
        let (app_asks, kept) = (asks.clone(), got.clone());
        let window = Window { title: "taut test", width: 320.0, height: 200.0, ..Default::default() };
        let (_alone, mut host) = host(window, move |ui| {
            let button = ui.button("Let files go here");
            kept.borrow_mut().1 = button.rect;
            let files = if app_asks.get() { ui.files_dropped(&button) } else { None };
            kept.borrow_mut().0.extend(files.into_iter().flatten());
        })
        .expect("a display");
        settle(&mut host);
        let (conn, id) = (host.conn.clone(), host.windows[0].id);
        let (named, wait) = (|name: &CStr| conn.atom(name), Duration::from_secs(2));
        assert!(DROPS.get().is_none() && OUTWARD.get().is_none(), "no way of taking or dragging out has been handed over");
        assert_eq!(conn.numbers(id, c"XdndAware"), [], "it says nothing while its app has not asked");
        let (from, heard) = files_dragged_from_elsewhere("file:///tmp/one.txt\r\n");
        let at = spot(&host, 0, got.borrow().1);
        let dragged_in = || {
            [
                message(id, named(c"XdndEnter"), [from as c_long, 5 << 24, named(FILES) as c_long, 0, 0]),
                message(id, named(c"XdndPosition"), [from as c_long, 0, (at[0] << 16 | at[1]) as c_long, 0, named(c"XdndActionCopy") as c_long]),
            ]
        };
        dragged_in().into_iter().for_each(|event| send(&mut host, id, event));
        assert!(heard.recv_timeout(Duration::from_millis(300)).is_err() && !host.ui.files_over, "nothing answered, nothing heard");
        send(&mut host, id, message(id, named(c"XdndLeave"), [from as c_long, 0, 0, 0, 0]));
        // The app asks: from that run the window announces XDND 5, and drops reach the app.
        asks.set(true);
        host.rerun = true;
        host.turn();
        assert_eq!(conn.numbers(id, c"XdndAware"), [5], "it says so once its app asks");
        dragged_in().into_iter().for_each(|event| send(&mut host, id, event));
        assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndStatus"), [id as c_long, 3, 0, 0, named(c"XdndActionCopy") as c_long])), "they may be let go here");
        assert!(host.ui.files_over);
        send(&mut host, id, message(id, named(c"XdndDrop"), [from as c_long, 0, 0, 0, 0]));
        assert_eq!(heard.recv_timeout(wait), Ok((0, [id as c_long, 0, 0, 0, 0])), "the list was asked for");
        assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndFinished"), [id as c_long, 1, named(c"XdndActionCopy") as c_long, 0, 0])), "and the files were taken");
        assert_eq!(got.borrow().0, [PathBuf::from("/tmp/one.txt")]);
    }

    /// Has a host take in X events until something holds, or two seconds pass. Says whether it
    /// came to hold. (A window manager acts in its own time.)
    fn wait<App: FnMut(&mut Ui)>(host: &mut Host<App>, so: impl Fn(&Host<App>) -> bool) -> bool {
        let (conn, asked) = (host.conn.clone(), Instant::now());
        while !so(host) && asked.elapsed() < Duration::from_secs(2) {
            // X has done what it was asked by the time it answers a later query.
            conn.property(conn.window, conn.mailbox, false);
            host.turn();
        }
        so(host)
    }

    /// Has a host take in whatever X still has to say, for 200 ms.
    fn settle<App: FnMut(&mut Ui)>(host: &mut Host<App>) {
        let (conn, asked) = (host.conn.clone(), Instant::now());
        while asked.elapsed() < Duration::from_millis(200) {
            conn.property(conn.window, conn.mailbox, false);
            host.turn();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Each pixel of a window as on screen now, in rows.
    fn on_screen(conn: &Connection, window: XWindow, [width, height]: [c_int; 2]) -> Vec<Vec<c_ulong>> {
        let asking = Asking::open().unwrap();
        // SAFETY: X returns an image of the window this size (all planes, `ZPixmap` 2), read
        // pixel by pixel through X's own function.
        unsafe {
            let picture = (asking.XGetImage)(conn.display, window, 0, 0, width as c_uint, height as c_uint, !0, 2);
            assert!(!picture.is_null() && ((*picture).width, (*picture).height) == (width, height), "X has no picture of the window");
            (0..height).map(|y| (0..width).map(|x| (asking.XGetPixel)(picture, x, y)).collect()).collect()
        }
    }

    /// Two small changes far apart in one run: each place is drawn and put by itself, nothing
    /// between. (First, X itself fills the window with a colour nothing draws: whatever differs
    /// afterwards was put on screen.) It reads the screen, so it runs only when asked, on a
    /// display of its own.
    #[test]
    #[ignore]
    fn two_changes_far_apart_are_put_on_the_screen_as_two_places() {
        const MARK: c_ulong = 0xff00ff;
        let kept = Rc::new(RefCell::new(([false; 2], [Rect::EMPTY; 2])));
        let app = kept.clone();
        let describe = move |ui: &mut Ui| {
            let (ticks, at) = &mut *app.borrow_mut();
            at[0] = ui.checkbox(&mut ticks[0], "Above").rect;
            ui.spacer();
            at[1] = ui.checkbox(&mut ticks[1], "Below").rect;
        };
        let (_alone, mut host) = host(Window { title: "taut places", width: 320.0, height: 200.0, ..Default::default() }, describe).expect("a display");
        let (conn, id, size, scale, asking) = (host.conn.clone(), host.windows[0].id, host.windows[0].size, host.scale, Asking::open().unwrap());
        // Whatever X still has to say about the window being mapped is taken in first.
        settle(&mut host);
        // SAFETY: the window is the host's own, on its display.
        let fill = || unsafe { ((asking.XSetWindowBackground)(conn.display, id, MARK), (asking.XClearWindow)(conn.display, id)) };
        // What was put on screen: how many pixels, and the rows of the first and last.
        let put = |conn: &Connection| {
            let rows = on_screen(conn, id, size);
            let drawn: Vec<usize> = rows.iter().enumerate().filter(|(_, row)| row.iter().any(|pixel| *pixel != MARK)).map(|(y, _)| y).collect();
            (rows.iter().flatten().filter(|pixel| **pixel != MARK).count(), drawn)
        };
        // (Through the GPU, a driver may put a frame on the screen a moment after it is shown:
        // what is wanted is waited for, a second at most.)
        let waited = |wanted: &dyn Fn(usize, &[usize]) -> bool| {
            let (asked, mut seen) = (Instant::now(), put(&conn));
            while !wanted(seen.0, &seen.1) && asked.elapsed() < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(10));
                seen = put(&conn);
            }
            seen
        };
        fill();
        assert_eq!(put(&conn), (0, vec![]), "nothing is put on the screen while nothing changes");
        host.turn();
        assert_eq!(put(&conn).0, 0);

        // The app checks both boxes itself, at once.
        (kept.borrow_mut().0, host.rerun) = ([true; 2], true);
        host.turn();
        let at = kept.borrow().1;
        let lines = |of: Rect| ((of.y * scale).floor() as usize, (of.bottom() * scale).ceil() as usize);
        let ((above, above_ends), (below, below_ends)) = (lines(at[0]), lines(at[1]));
        let both = |_: usize, rows: &[usize]| rows.iter().any(|y| (above..above_ends).contains(y)) && rows.iter().any(|y| (below..below_ends).contains(y));
        let (pixels, rows) = waited(&both);
        assert!(below > above_ends + 100, "the boxes are far apart: {at:?}");
        assert!(both(pixels, &rows), "both places are put on the screen: {rows:?}");
        // (Through the GPU, the driver decides what reaches X: see `through_the_gpu`.)
        let by_x = host.ui.renderer() == crate::platform::Renderer::Cpu;
        assert!(!by_x || rows.iter().all(|y| (above..above_ends).contains(y) || (below..below_ends).contains(y)), "and nothing between them, or around them: {rows:?} for {at:?}");
        assert!(!by_x || pixels <= ((at[0].w + at[1].w) * scale) as usize * rows.len(), "{pixels} pixels");

        // What X reports uncovered is redrawn whole, with whatever changed in it meanwhile.
        fill();
        (kept.borrow_mut().0, host.rerun) = ([false; 2], true);
        host.windows[0].exposed(ExposeEvent { kind: 12, serial: 0, sent: 0, display: null_mut(), window: id, x: 0, y: 0, width: size[0], height: size[1] }, scale);
        host.turn();
        let whole: (usize, Vec<usize>) = ((size[0] * size[1]) as usize, (0..size[1] as usize).collect());
        assert_eq!(waited(&|pixels, rows| pixels == whole.0 && rows == whole.1), whole, "all of the window");
    }

    /// A window's frame corner as X and the window manager have it: the window's own corner on
    /// screen, less the frame extents left and top.
    fn corner_at_x(conn: &Connection, window: XWindow) -> (i32, i32) {
        let ([x, y], frame) = (conn.place(window), conn.numbers(window, c"_NET_FRAME_EXTENTS"));
        (x - frame.first().map_or(0, |left| *left as i32), y - frame.get(2).map_or(0, |up| *up as i32))
    }

    /// The placement tests' app state: a second window and a third.
    #[derive(Clone, Copy, Debug)]
    struct Places {
        prefs: SecondWindow,
        log: SecondWindow,
    }

    /// A first window opened as `window` says, describing the two second windows.
    fn placed(window: Window, places: Places) -> (Alone, Rc<RefCell<Places>>) {
        let kept = Rc::new(RefCell::new(places));
        let app = kept.clone();
        let describe = move |ui: &mut Ui| {
            let Places { prefs, log } = &mut *app.borrow_mut();
            ui.label("One");
            if ui.window(prefs, "Preferences") {
                ui.label("Two");
                ui.close();
            }
            if ui.window(log, "Log") {
                ui.label("Three");
                ui.close();
            }
        };
        (host(window, describe).expect("a display"), kept)
    }

    /// A window opens where its app says, and its app hears where it is and where the person
    /// moves it; a place kept one day is where it opens the next. With a window manager that is
    /// the frame's corner, without one the window's own. (It opens and moves windows on screen,
    /// so it runs only when asked, on a display of its own; again with a window manager where
    /// there is one.)
    #[test]
    #[ignore]
    fn a_window_opens_where_its_app_says_and_its_app_is_told_where_it_is() {
        let asking = Asking::open().unwrap();
        let places = Places { prefs: SecondWindow::new(200.0, 120.0).position(540, 280).min_size(120.0, 60.0), log: SecondWindow { open: false, ..SecondWindow::new(200.0, 120.0) } };
        let ((alone, mut host), kept) = placed(Window { title: "taut placed", position: Some((300, 200)), width: 400.0, height: 240.0, ..Default::default() }, places);
        let (conn, scale, first) = (host.conn.clone(), host.scale, host.windows[0].id);
        let there = |at| WindowPlacement { position: at, width: (400.0 * scale).floor() / scale, height: (240.0 * scale).floor() / scale, maximized: false };
        assert!(wait(&mut host, |host| host.ui.window_placement() == Some(there((300, 200)))), "the window is told {:?}", host.ui.window_placement());
        assert_eq!(corner_at_x(&conn, first), (300, 200), "and that is where X has it");
        // Moved as the person would (asking the window manager, where there is one); its
        // interface is told.
        unsafe { (asking.XMoveWindow)(conn.display, first, 420, 310) };
        assert!(wait(&mut host, |host| host.ui.window_placement() == Some(there((420, 310)))), "moved, the window is told {:?}", host.ui.window_placement());
        assert_eq!(corner_at_x(&conn, first), (420, 310));

        // A second window: where its app says, no smaller than it says. Its app hears where it
        // is and where it moves.
        let second = host.windows[1].id;
        assert!(wait(&mut host, |_| kept.borrow().prefs.position == Some((540, 280)) && corner_at_x(&conn, second) == (540, 280)), "the second window is told {:?}", kept.borrow().prefs.position);
        let asks = conn.numbers(second, c"WM_NORMAL_HINTS");
        assert_eq!((asks[0], asks[5], asks[6]), (1 << 4 | 1, (120.0 * scale) as c_ulong, (60.0 * scale) as c_ulong), "its place is one the person chose, and it has a smallest size");
        unsafe { (asking.XMoveWindow)(conn.display, second, 600, 330) };
        assert!(wait(&mut host, |_| kept.borrow().prefs.position == Some((600, 330))), "moved, the second window is told {:?}", kept.borrow().prefs.position);
        assert_eq!(corner_at_x(&conn, second), (600, 330));
        // One with no place given is placed by the window manager (by X where there is none),
        // and its app hears where.
        (kept.borrow_mut().log.open, host.rerun) = (true, true);
        host.turn();
        let third = host.windows[2].id;
        assert_eq!(conn.numbers(third, c"WM_NORMAL_HINTS")[0], 1 << 4, "it asks for no place");
        // A window manager may map a window before placing it.
        settle(&mut host);
        assert!(wait(&mut host, |_| kept.borrow().log.position == Some(corner_at_x(&conn, third))), "the third window is told {:?}", kept.borrow().log.position);

        // The next day: the app opens its windows with what it kept, and they are where they
        // were.
        let (first_kept, places) = (host.ui.window_placement(), *kept.borrow());
        drop((host, alone));
        let ((_alone, mut host), kept) = placed(Window { title: "taut placed", ..Default::default() }.with_placement(first_kept), places);
        let (conn, windows) = (host.conn.clone(), [0, 1, 2].map(|at| host.windows[at].id));
        let wanted = [first_kept.map(|place| place.position), places.prefs.position, places.log.position];
        let told = |host: &Windows| [host.ui.window_placement().map(|place| place.position), kept.borrow().prefs.position, kept.borrow().log.position];
        let there = || windows.map(|window| Some(corner_at_x(&conn, window)));
        // Again the window manager gets time to place each.
        settle(&mut host);
        assert!(wait(&mut host, |host| told(host) == wanted && there() == wanted), "the next day the windows are told {:?} and are at {:?}, and had kept {wanted:?}", told(&host), there());
        assert_eq!(host.ui.window_placement(), first_kept);
    }

    /// A place on no screen (one was removed) is moved onto the nearest screen, for a first and
    /// a second window: just enough for the whole window to lie in its work area, at least its
    /// title.
    #[test]
    #[ignore]
    fn a_place_on_no_screen_is_moved_onto_the_nearest() {
        let places = Places { prefs: SecondWindow::new(200.0, 120.0).position(20_000, 100), log: SecondWindow::new(200.0, 120.0).position(100, -500) };
        let ((_alone, mut host), kept) = placed(Window { title: "taut lost", position: Some((-30_000, -30_000)), width: 300.0, height: 200.0, ..Default::default() }, places);
        let (conn, windows, work) = (host.conn.clone(), [0, 1, 2].map(|at| host.windows[at].id), host.ui.screens()[0].work_area);
        assert_eq!(windows.map(|window| conn.numbers(window, c"WM_NORMAL_HINTS")[0] & 1), [1; 3], "each asks for the place it was moved to");
        // A window manager may map a window before placing it.
        settle(&mut host);
        let told = |host: &Windows| [host.ui.window_placement().map(|place| place.position), kept.borrow().prefs.position, kept.borrow().log.position];
        let there = || windows.map(|window| Some(corner_at_x(&conn, window)));
        assert!(wait(&mut host, |host| told(host) == there()), "each is told where it was put: {:?}, and they are at {:?}", told(&host), there());
        let on = |(x, y): (i32, i32)| x as f32 >= work.x && y as f32 >= work.y && (x + 160) as f32 <= work.right() && (y + 30) as f32 <= work.bottom();
        assert!(there().into_iter().flatten().all(on), "{:?} are not all on {work:?}", there());
        let [first, prefs, log] = there().map(Option::unwrap);
        assert_eq!((first, prefs.1, log), ((work.x as i32, work.y as i32), 100, (100, work.y as i32)), "each is moved no further than it takes");
        assert!(prefs.0 > 400, "the one beyond the right edge is at the right edge: {prefs:?}");
    }

    /// Asks the window manager to change a window's state, as a task bar does: maximize, or
    /// restore.
    fn ask_to_fill(conn: &Connection, window: XWindow, fill: bool) {
        let data = [fill as c_long, conn.atom(FILLING[0]) as c_long, conn.atom(FILLING[1]) as c_long, 2, 0];
        let mut event = message(window, conn.atom(c"_NET_WM_STATE"), data);
        // SAFETY: the client message has X's layout and goes to the window manager
        // (`SubstructureRedirectMask | SubstructureNotifyMask`).
        unsafe { (conn.x.XSendEvent)(conn.display, root(&conn.x, conn.display), 0, 1 << 19 | 1 << 20, &mut event) };
    }

    /// A window opening minimized tells its app, comes forward when its app asks, and stays
    /// above other programs' windows while asked. All of it is the window manager's: with none,
    /// the window opens normally, says how it would have started, and its app is told nothing.
    /// (On a display of its own.)
    #[test]
    #[ignore]
    fn a_window_opens_minimized_comes_forward_and_stays_over_the_others() {
        let asks = Rc::new(std::cell::Cell::new((None::<bool>, false)));
        let app = asks.clone();
        let describe = move |ui: &mut Ui| {
            let (top, forward) = app.take();
            top.into_iter().for_each(|on| ui.set_on_top(on));
            if forward {
                ui.bring_to_front();
            }
            ui.label("One");
        };
        let (alone, mut host) = host(Window { title: "taut minimized", width: 300.0, height: 200.0, minimized: true, ..Default::default() }, describe).expect("a display");
        let (conn, first) = (host.conn.clone(), host.windows[0].id);
        let state = |name: &CStr| conn.numbers(first, c"_NET_WM_STATE").contains(&conn.atom(name));
        assert_eq!(conn.numbers(first, c"WM_HINTS").get(..3), Some(&[2, 0, 3][..]), "it said that it starts as an icon");
        if !conn.managed() {
            assert!(!host.ui.window_minimized());
            return eprintln!("no window manager: a window that opens minimized not tested");
        }
        assert!(wait(&mut host, |host| host.ui.window_minimized()), "minimized, its interface is told so");
        asks.set((None, true));
        host.rerun = true;
        assert!(wait(&mut host, |host| !host.ui.window_minimized()), "asked to come forward, it is minimized no longer");
        asks.set((Some(true), false));
        host.rerun = true;
        assert!(wait(&mut host, |_| state(c"_NET_WM_STATE_ABOVE")), "asked to, it stays over the others");
        asks.set((Some(false), false));
        host.rerun = true;
        assert!(wait(&mut host, |_| !state(c"_NET_WM_STATE_ABOVE")), "and stops when it is asked to");
        drop((host, alone));
    }

    /// A maximized window is told so with the place and size it restores to; one opening
    /// maximized is told the place and size it opened with. Only a window manager maximizes:
    /// without one, a window asking for it opens at its size and is not told it is maximized.
    /// (On a display of its own.)
    #[test]
    #[ignore]
    fn a_window_that_fills_its_screen_is_told_the_place_it_goes_back_to() {
        let none = Places { prefs: SecondWindow { open: false, ..SecondWindow::new(200.0, 120.0) }, log: SecondWindow { open: false, ..SecondWindow::new(200.0, 120.0) } };
        let ((alone, mut host), _) = placed(Window { title: "taut filling", position: Some((200, 150)), width: 400.0, height: 240.0, maximized: true, ..Default::default() }, none);
        let (conn, scale, first) = (host.conn.clone(), host.scale, host.windows[0].id);
        let small = [(400.0 * scale) as c_int, (240.0 * scale) as c_int];
        let there = |at, maximized| WindowPlacement { position: at, width: small[0] as f32 / scale, height: small[1] as f32 / scale, maximized };
        if !conn.managed() {
            assert!(wait(&mut host, |host| host.ui.window_placement() == Some(there((200, 150), false))), "with no window manager the window is told {:?}", host.ui.window_placement());
            assert_eq!((conn.numbers(first, c"_NET_WM_STATE"), size_at_x(&conn, first)), (vec![], Some(small.map(|side| side as c_uint))));
            return eprintln!("no window manager: a window that fills its screen not tested");
        }
        // Whether a window that opened maximized is back at its restore place and is told where
        // it is. (A window manager may place it some pixels off: xfwm4 by its frame border.)
        let back = |host: &mut Windows, to: (i32, i32)| {
            let told = |host: &Windows| host.ui.window_placement().filter(|place| !place.maximized && place.position == corner_at_x(&host.conn, host.windows[0].id));
            wait(host, |host| host.windows[0].size == small && told(host).is_some());
            let at = told(host).map(|place| place.position);
            assert!(at.is_some_and(|at| (at.0 - to.0).abs() <= 8 && (at.1 - to.1).abs() <= 8), "back at {to:?}, the window is told {:?}", host.ui.window_placement());
        };
        // It opens maximized, told the place and size it opened with.
        assert!(wait(&mut host, |host| host.ui.window_placement() == Some(there((200, 150), true)) && host.windows[0].size[0] > small[0]), "the window is told {:?}", host.ui.window_placement());
        let filled = host.windows[0].size;
        assert!(wait(&mut host, |host| host.ui.size().w == filled[0] as f32 / scale), "its interface is as wide as the screen has room");
        // The person restores it: it goes back, told where it is.
        ask_to_fill(&conn, first, false);
        back(&mut host, (200, 150));
        // Moved, then maximized again: it is told so, the new place being the one to restore
        // to. And there it goes.
        // SAFETY: the window is the host's own, on its display.
        unsafe { (Asking::open().unwrap().XMoveWindow)(conn.display, first, 260, 180) };
        assert!(wait(&mut host, |host| host.ui.window_placement() == Some(there((260, 180), false))), "moved, the window is told {:?}", host.ui.window_placement());
        // The person lets go, and a moment later maximizes it.
        std::thread::sleep(SETTLED * 2);
        for fill in [true, false, true] {
            ask_to_fill(&conn, first, fill);
            let size = if fill { filled } else { small };
            assert!(
                wait(&mut host, |host| host.ui.window_placement() == Some(there((260, 180), fill)) && host.windows[0].size == size),
                "filling: {fill}, the window is told {:?}",
                host.ui.window_placement()
            );
        }
        // The next day it opens maximized, with the same restore place.
        let kept = host.ui.window_placement();
        drop((host, alone));
        let ((_alone, mut host), _) = placed(Window { title: "taut filling", ..Default::default() }.with_placement(kept), none);
        let (conn, first) = (host.conn.clone(), host.windows[0].id);
        assert!(
            wait(&mut host, |host| host.ui.window_placement() == kept && host.windows[0].size == filled),
            "the next day the window is told {:?}, and had kept {kept:?}",
            host.ui.window_placement()
        );
        ask_to_fill(&conn, first, false);
        back(&mut host, (260, 180));
    }

    /// A window manager reports maximizing in steps, in its own order: here the test is the
    /// window manager, taking the steps one at a time in the worst order, the windows' loop
    /// taking each in alone. The restore place is where it stood before, whatever was seen
    /// meanwhile; the frame corner follows from the stated frame extents. (On a display of its
    /// own, with no window manager: there the test decides.)
    #[test]
    #[ignore]
    fn a_window_seen_on_its_way_to_filling_its_screen_goes_back_to_where_it_stood() {
        let none = Places { prefs: SecondWindow { open: false, ..SecondWindow::new(200.0, 120.0) }, log: SecondWindow { open: false, ..SecondWindow::new(200.0, 120.0) } };
        let ((_alone, mut host), _) = placed(Window { title: "taut steps", position: Some((200, 150)), width: 400.0, height: 240.0, ..Default::default() }, none);
        let (conn, scale, id, asking) = (host.conn.clone(), host.scale, host.windows[0].id, Asking::open().unwrap());
        if conn.managed() {
            return eprintln!("a window manager is there: its steps not played");
        }
        let small = [(400.0 * scale) as c_int, (240.0 * scale) as c_int];
        let there = |at, [width, height]: [c_int; 2], maximized| Some(WindowPlacement { position: at, width: width as f32 / scale, height: height as f32 / scale, maximized });
        // SAFETY: the window is the host's own, on its display.
        let lies = |[x, y, width, height]: [c_int; 4]| unsafe { (asking.XMoveResizeWindow)(conn.display, id, x, y, width as c_uint, height as c_uint) };
        let state = |names: &[&CStr]| conn.leave(id, conn.atom(c"_NET_WM_STATE"), 4, &names.iter().map(|name| conn.atom(name)).collect::<Vec<Atom>>());
        let frame = |left: c_long, up: c_long| conn.leave(id, conn.atom(c"_NET_FRAME_EXTENTS"), 6, &[left, 7, up, 9]);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((200, 150), small, false)), "the window is told {:?}", host.ui.window_placement());
        // A frame 6 pixels wide and 30 above: its corner that far from the window's own.
        frame(6, 30);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((194, 120), small, false)), "in a frame, the window is told {:?}", host.ui.window_placement());
        std::thread::sleep(SETTLED * 2);

        // A maximized frame, then a maximized size, and only then the state: each seen as it
        // is, none kept.
        frame(0, 24);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((200, 126), small, false)), "with another frame, the window is told {:?}", host.ui.window_placement());
        lies([0, 24, 900, 576]);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((0, 0), [900, 576], false)), "at another size, the window is told {:?}", host.ui.window_placement());
        state(&FILLING);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((194, 120), small, true)), "filling its screen, the window is told {:?}", host.ui.window_placement());
        // Back, the state first and the rest after: it is where it was.
        state(&[]);
        frame(6, 30);
        lies([200, 150, small[0], small[1]]);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((194, 120), small, false)), "back, the window is told {:?}", host.ui.window_placement());
        // Fullscreen at once, the state first this time: nothing seen since stayed long enough
        // to count, and the restore place is the same. (Fullscreen is not maximized.)
        state(&[WHOLE]);
        lies([0, 0, 900, 600]);
        frame(0, 0);
        assert!(
            wait(&mut host, |host| host.windows[0].size == [900, 600] && host.ui.window_placement() == there((194, 120), small, false) && host.ui.full_screen()),
            "over all the screen, the window is told {:?}",
            host.ui.window_placement()
        );
        // Back, moved by the person, it stays a while. Then the maximized state comes first:
        // where it stood is its restore place.
        state(&[]);
        frame(6, 30);
        lies([300, 200, small[0], small[1]]);
        assert!(wait(&mut host, |host| !host.ui.full_screen()), "back from lying over the screen, the window is told so");
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((294, 170), small, false)), "moved, the window is told {:?}", host.ui.window_placement());
        std::thread::sleep(SETTLED * 2);
        state(&FILLING);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((294, 170), small, true)), "filling its screen from there, the window is told {:?}", host.ui.window_placement());
        // Only one of the two maximized states is not maximized: the window is told as it is.
        state(&FILLING[..1]);
        assert!(wait(&mut host, |host| host.ui.window_placement() == there((294, 170), small, false)), "as wide as the screen only, the window is told {:?}", host.ui.window_placement());
    }

    /// While input must not be seen, the window types through the null input method and every
    /// key still types what the keymap says; afterwards it types through the person's method
    /// again. (Keys are sent to the window: on a display of its own.)
    #[test]
    #[ignore]
    fn what_is_typed_in_a_secret_box_goes_past_the_input_method() {
        let kept = Rc::new(RefCell::new(([String::new(), String::new()], [Rect::EMPTY; 2])));
        let app = kept.clone();
        let describe = move |ui: &mut Ui| {
            let (typed, at) = &mut *app.borrow_mut();
            let [seen, secret] = typed;
            *at = [ui.text_box(seen, "").rect, ui.password_box(secret, "").rect];
        };
        let (_alone, mut host) = host(Window { title: "taut secret", width: 320.0, height: 200.0, ..Default::default() }, describe).expect("a display");
        let (conn, scale, id, at, asking) = (host.conn.clone(), host.scale, host.windows[0].id, kept.borrow().1, Asking::open().unwrap());
        // SAFETY: the display is the host's own.
        let key = |letter: char| unsafe { (asking.XKeysymToKeycode)(conn.display, letter as c_ulong) } as c_uint;
        let typed = |host: &mut Windows, text: &str| text.chars().for_each(|letter| send(host, id, input(id, 2, [0, 0], key(letter))));
        click(&mut host, id, middle(at[0], scale));
        typed(&mut host, "ab");
        assert!(!host.windows[0].typing.is_null() && !host.windows[0].hidden && host.plain.is_null(), "the window types through the person's input method");
        click(&mut host, id, middle(at[1], scale));
        let plain = host.windows[0].typing;
        assert!(host.windows[0].hidden && !host.plain.is_null() && !plain.is_null(), "the window types through the method that is none");
        typed(&mut host, "pw");
        assert_eq!((host.windows[0].typing, kept.borrow().0.clone()), (plain, ["ab".to_string(), "pw".into()]), "what is typed still arrives, by the same way for every key");
        click(&mut host, id, middle(at[0], scale));
        assert!(!host.windows[0].hidden && !host.windows[0].typing.is_null(), "the person's input method is back");
        typed(&mut host, "c");
        assert_eq!(kept.borrow().0, ["abc", "pw"]);
    }

    /// The portal tests' app state: it follows the desktop's dark mode and asks for the system
    /// file dialog in its first window and a second one.
    struct Follows {
        /// How often it was described; and the desktop's dark state each time it changed.
        runs: u32,
        dark: Vec<bool>,
        /// How often its button was pressed, and where the button is.
        presses: u32,
        button: Rect,
        /// The first window's file dialog and its answers; then the second window's.
        files: FileDialog,
        picked: Vec<PathBuf>,
        second: SecondWindow,
        files_there: FileDialog,
        picked_there: Vec<PathBuf>,
    }

    fn follows(app: &mut Follows, ui: &mut Ui) {
        let dark = ui.system_dark();
        (app.runs, ui.theme) = (app.runs + 1, if dark { Theme::dark() } else { Theme::light() });
        if app.dark.last() != Some(&dark) {
            app.dark.push(dark);
        }
        let button = ui.button("Press");
        (app.presses, app.button) = (app.presses + button.clicked as u32, button.rect);
        ui.label(&format!("{} presses", app.presses));
        app.picked.extend(ui.file_dialog(&mut app.files));
        if ui.window(&mut app.second, "Second") {
            app.picked_there.extend(ui.file_dialog(&mut app.files_there));
            ui.close();
        }
    }

    /// That app's window in a test-started session, and its state.
    type Following = (std::sync::MutexGuard<'static, ()>, Windows, Rc<RefCell<Follows>>);
    fn following(session: Session) -> Option<Following> {
        following_alone(alone(), session)
    }
    fn following_alone(alone: std::sync::MutexGuard<'static, ()>, session: Session) -> Option<Following> {
        let (second, files) = (SecondWindow { open: false, ..SecondWindow::new(240.0, 160.0) }, [FileDialog::native(), FileDialog::native()]);
        let [files, files_there] = files;
        let app = Follows { runs: 0, dark: Vec::new(), presses: 0, button: Rect::EMPTY, files, picked: Vec::new(), second, files_there, picked_there: Vec::new() };
        let app = Rc::new(RefCell::new(app));
        let described = app.clone();
        let (alone, host) = host_on(alone, session, Window { title: "taut portal", width: 320.0, height: 200.0, ..Default::default() }, move |ui| follows(&mut described.borrow_mut(), ui))?;
        Some((alone, host, app))
    }

    /// Has a host take turns, reading its buses each time, until something holds or five
    /// seconds pass. Says whether it came to hold. (A bus peer answers in its own time.)
    fn hears(host: &mut Windows, so: impl Fn(&Windows) -> bool) -> bool {
        let asked = Instant::now();
        while !so(host) && asked.elapsed() < Duration::from_secs(5) {
            host.heard = true;
            host.turn();
            std::thread::sleep(Duration::from_millis(2));
        }
        so(host)
    }

    /// Has a host take turns for 200 ms, reading its buses: whatever peers said by then is
    /// taken in.
    fn heard_out(host: &mut Windows) {
        let asked = Instant::now();
        hears(host, |_| asked.elapsed() >= Duration::from_millis(200));
    }

    /// Has a host sleep as its loop does, and says how long it slept. A watch wakes it after
    /// `most`, if nothing else does.
    fn slept(host: &mut Windows, most: Duration) -> Duration {
        let (awake, waits) = std::sync::mpsc::channel::<()>();
        let watch = std::thread::spawn(move || {
            if waits.recv_timeout(most).is_err() {
                Waker.wake();
            }
        });
        let since = Instant::now();
        host.sleep();
        let slept = since.elapsed();
        awake.send(()).ok();
        watch.join().unwrap();
        slept
    }

    /// Has the app change its state, and its window take a turn for that.
    fn has(host: &mut Windows, app: &Rc<RefCell<Follows>>, does: impl FnOnce(&mut Follows)) {
        does(&mut app.borrow_mut());
        host.rerun = true;
        host.turn();
    }

    /// A window hears the person's dark-mode preference by its first run, where the portal has
    /// answered by then; its interface runs again whenever the person changes it, and for
    /// nothing else changing on the desktop. While nothing is said, nothing runs.
    #[test]
    fn a_window_follows_whether_the_desktop_is_dark() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let stand = StandIn::start(&own.address, true, Ends::Closed);
        // The portal is asked as the session starts, and given the time a program spends
        // finding fonts: its answer is there by the first turn.
        let session = Session::on(Bus::open(&own.address));
        std::thread::sleep(Duration::from_millis(100));
        let Some((_alone, mut host, app)) = following(session) else { return eprintln!("no display: not tested") };
        assert_eq!((host.ui.system_dark(), app.borrow().dark.clone()), (true, vec![true]), "the app never saw the desktop as not dark");

        // The person asks for light, then dark again: the app hears each. Another desktop
        // setting changes: nothing runs.
        for (wish, dark) in [(2, false), (1, true)] {
            let runs = app.borrow().runs;
            stand.say(wish);
            assert!(hears(&mut host, |host| host.ui.system_dark() == dark));
            assert!(app.borrow().dark.last() == Some(&dark) && app.borrow().runs > runs, "wish {wish}");
        }
        let runs = app.borrow().runs;
        stand.say_other();
        heard_out(&mut host);
        assert_eq!((app.borrow().runs, app.borrow().dark.clone()), (runs, vec![true, false, true]));

        // While nothing is said the window sleeps: only the watch set here wakes it. And a
        // preference change wakes it.
        assert!(!host.rerun && host.ui.wake_in().is_none(), "nothing is due");
        assert!(slept(&mut host, Duration::from_millis(400)) >= Duration::from_millis(350) && !host.heard);
        host.turn();
        stand.say(2);
        assert!(slept(&mut host, Duration::from_secs(5)) < Duration::from_secs(4) && host.heard);
        assert!(hears(&mut host, |host| !host.ui.system_dark()) && app.borrow().dark.last() == Some(&false));
    }

    /// An app asks for the system file dialog: the portal is asked to show it over the window
    /// with the app's options, and the file picked there is what the app's dialog answers. None
    /// of the library's own dialog is described meanwhile.
    #[test]
    fn the_systems_file_dialog_is_asked_of_the_portal_and_answers_the_app() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        // A file name with spaces, a percent sign, an ampersand and a two-byte letter, as the
        // portal writes its URI.
        let stand = StandIn::start(&own.address, false, Ends::Picked(vec!["file:///tmp/taut%20portal/100%25%20gr%C3%BCn%20%26%20blau.png".into()]));
        let Some((_alone, mut host, app)) = following(Session::on(Bus::open(&own.address))) else { return eprintln!("no display: not tested") };
        assert!(hears(&mut host, |host| host.ui.system_files), "the portal has a file dialog: the window can show the system's");
        let (id, asked, picked) = (host.windows[0].id, || stand.asked.lock().unwrap().clone(), |ends: &str| stand.how.lock().unwrap().ends = Ends::Picked(vec![ends.into()]));

        // A picture to open.
        has(&mut host, &app, |app| (app.files.only(&[".png", ".txt"]), app.files.pick("/tmp/taut portal")).0);
        assert!(host.ui.file_asked && host.ui.focus() == 0 && app.borrow().files.shown, "the system's dialog is up, and nothing of the library's own is described");
        assert!(hears(&mut host, |_| !app.borrow().picked.is_empty()));
        assert_eq!(app.borrow().picked, [PathBuf::from("/tmp/taut portal/100% gr\u{fc}n & blau.png")]);
        assert_eq!((app.borrow().files.shown, app.borrow().files.folder.clone(), host.ui.file_asked), (false, PathBuf::from("/tmp/taut portal"), false));
        let kinds = "filters=*.png *.txt:*.[pP][nN][gG],*.[tT][xX][tT]";
        assert_eq!(asked(), [format!("OpenFile x11:{id:x} | Open a file | handle_token=taut1 | current_folder=/tmp/taut portal\0 | {kinds}")]);

        // Where to save, with a starting name.
        picked("file:///tmp/taut%20portal/new%20name.txt");
        has(&mut host, &app, |app| app.files.save("/tmp/taut portal", "new name.txt"));
        assert!(hears(&mut host, |_| app.borrow().picked.len() == 2));
        assert_eq!(app.borrow().picked[1], PathBuf::from("/tmp/taut portal/new name.txt"));
        assert_eq!(asked()[1], format!("SaveFile x11:{id:x} | Save as | handle_token=taut2 | current_folder=/tmp/taut portal\0 | current_name=new name.txt | {kinds}"));

        // A folder. (The dialog next starts where the person went: in the folder picked.)
        picked("file:///tmp/taut%20portal");
        has(&mut host, &app, |app| app.files.pick_folder("/tmp"));
        assert!(hears(&mut host, |_| app.borrow().picked.len() == 3));
        assert_eq!((app.borrow().picked[2].clone(), app.borrow().files.folder.clone()), (PathBuf::from("/tmp/taut portal"), PathBuf::from("/tmp/taut portal")));
        assert_eq!(asked()[2], format!("OpenFile x11:{id:x} | Pick a folder | handle_token=taut3 | current_folder=/tmp\0 | directory=1"));

        // The person cancels by its button, or closes it by a key: either way the app's dialog
        // ends with no file, and the next one is the system's again.
        for (ends, asked_so_far) in [(Ends::Closed, 4), (Ends::Otherwise, 5)] {
            stand.how.lock().unwrap().ends = ends;
            has(&mut host, &app, |app| app.files.pick("/tmp"));
            assert!(hears(&mut host, |_| !app.borrow().files.shown));
            assert_eq!((app.borrow().picked.len(), asked().len(), host.ui.system_files, host.ui.file_asked, host.ui.focus()), (3, asked_so_far, true, false, 0));
        }
    }

    /// While the system file dialog is up (the portal takes two seconds here), the window works
    /// and draws as usual: the dialog is another program's, and the window waits for nothing.
    #[test]
    #[ignore]
    fn a_window_is_used_and_drawn_while_the_systems_file_dialog_is_up() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let stand = StandIn::start(&own.address, false, Ends::Picked(vec!["file:///tmp/late.txt".into()]));
        stand.how.lock().unwrap().after = Duration::from_secs(2);
        let (_alone, mut host, app) = following(Session::on(Bus::open(&own.address))).expect("a display");
        assert!(hears(&mut host, |host| host.ui.system_files));
        let (conn, scale, id, size) = (host.conn.clone(), host.scale, host.windows[0].id, host.windows[0].size);
        has(&mut host, &app, |app| app.files.pick("/tmp"));
        let (asked, before, at) = (Instant::now(), on_screen(&conn, id, size), middle(app.borrow().button, scale));
        // The button is pressed three times: each is counted, and shows.
        for presses in 1..=3 {
            click(&mut host, id, at);
            assert_eq!(app.borrow().presses, presses);
        }
        assert_ne!(on_screen(&conn, id, size), before, "what the presses changed is on the screen");
        assert!(app.borrow().picked.is_empty() && app.borrow().files.shown && asked.elapsed() < Duration::from_secs(2), "all of it while the dialog was up");
        // Then the person picks a file.
        assert!(hears(&mut host, |_| !app.borrow().picked.is_empty()));
        assert!(asked.elapsed() >= Duration::from_secs(2) && app.borrow().picked == [PathBuf::from("/tmp/late.txt")]);
    }

    /// Where the desktop has no file dialog, or it fails, the library's own is shown instead,
    /// as everywhere else, with its name box focused.
    #[test]
    fn where_the_portal_shows_no_file_dialog_the_librarys_own_is_shown() {
        let own_shown = |host: &Windows, app: &Rc<RefCell<Follows>>| app.borrow().files.shown && host.ui.focus() != 0 && !host.ui.file_asked && !host.ui.system_files;
        // No bus; a bus with no portal; a portal with no file dialog.
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        for case in ["no bus", "no portal", "no dialog"] {
            let how = crate::portal::tests::Stand { scheme: Some(1), version: None, ends: Ends::Never, ..Default::default() };
            let _stand = (case == "no dialog").then(|| StandIn::of(&own.address, how));
            let session = Session::on(Bus::open(&own.address).filter(|_| case != "no bus"));
            let Some((_alone, mut host, app)) = following(session) else { return eprintln!("no display: not tested") };
            // The portal present says the desktop is dark: what it lacks is a file dialog.
            heard_out(&mut host);
            assert_eq!(host.ui.system_dark(), case == "no dialog", "{case}");
            has(&mut host, &app, |app| app.files.pick("/tmp"));
            assert!(own_shown(&host, &app) && app.borrow().picked.is_empty(), "{case}");
        }
        // A portal refusing the request, and one leaving the bus while its dialog is up. Each
        // time the system dialog is asked for, and the library's own shown when that fails.
        for ends in [Ends::Refused, Ends::Never] {
            let mut stand = Some(StandIn::start(&own.address, false, ends.clone()));
            let (_alone, mut host, app) = following(Session::on(Bus::open(&own.address))).expect("a display");
            assert!(hears(&mut host, |host| host.ui.system_files));
            has(&mut host, &app, |app| app.files.pick("/tmp"));
            assert!(host.ui.file_asked && host.ui.focus() == 0, "the system's dialog is asked for");
            assert!(hears(&mut host, |_| stand.iter().any(|stand| stand.asked.lock().unwrap().len() == 1)));
            if matches!(ends, Ends::Never) {
                stand = None;
            }
            assert!(hears(&mut host, |host| own_shown(host, &app)) && app.borrow().picked.is_empty());
            // And the next dialog is the library's own, without asking.
            has(&mut host, &app, |app| (app.files.shown = false, app.files.pick("/tmp")).1);
            assert!(own_shown(&host, &app) && stand.iter().all(|stand| stand.asked.lock().unwrap().len() == 1));
        }
    }

    /// A portal that never answers a file-dialog request is given up on after five seconds, not
    /// before: the library's own dialog shows, the portal is told to close its own should it
    /// come after all, and the next dialog is the library's own with nothing asked.
    #[test]
    fn a_portal_that_says_nothing_is_given_up_on_and_the_librarys_own_dialog_shows() {
        let own_shown = |host: &Windows, app: &Rc<RefCell<Follows>>| app.borrow().files.shown && host.ui.focus() != 0 && !host.ui.file_asked && !host.ui.system_files;
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let stand = StandIn::start(&own.address, false, Ends::Silent);
        let Some((_alone, mut host, app)) = following(Session::on(Bus::open(&own.address))) else { return eprintln!("no display: not tested") };
        assert!(hears(&mut host, |host| host.ui.system_files));
        let asked = || stand.asked.lock().unwrap().clone();
        let since = Instant::now();
        has(&mut host, &app, |app| app.files.pick("/tmp"));
        assert!(hears(&mut host, |_| asked().len() == 1) && host.ui.file_asked);
        // Four seconds on, the app still waits for the system dialog.
        hears(&mut host, |_| since.elapsed() >= Duration::from_secs(4));
        assert!(host.ui.file_asked && !own_shown(&host, &app), "given up on before its time");
        // Just over five, it has the library's own; and the portal was told to close the dialog
        // it never reported.
        assert!(hears(&mut host, |host| own_shown(host, &app)) && app.borrow().picked.is_empty());
        assert!(since.elapsed() >= Duration::from_secs(5) && since.elapsed() < Duration::from_secs(7), "{:?}", since.elapsed());
        assert!(hears(&mut host, |_| asked().len() == 2) && asked()[1].starts_with("Close /org/freedesktop/portal/desktop/request/") && asked()[1].ends_with("/taut1"), "{:?}", asked());
        has(&mut host, &app, |app| (app.files.shown = false, app.files.pick("/tmp")).1);
        assert!(own_shown(&host, &app) && asked().len() == 2);
    }

    /// A second window's file dialog is shown over that window and answers that window's part
    /// of the app. When the app closes a window whose dialog is up, the portal is told to close
    /// it.
    #[test]
    fn a_second_windows_file_dialog_is_its_own() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let stand = StandIn::start(&own.address, false, Ends::Picked(vec!["file:///tmp/there.txt".into()]));
        let Some((_alone, mut host, app)) = following(Session::on(Bus::open(&own.address))) else { return eprintln!("no display: not tested") };
        assert!(hears(&mut host, |host| host.ui.system_files));
        has(&mut host, &app, |app| app.second.open = true);
        let asked = || stand.asked.lock().unwrap().clone();
        let (first, second) = (host.windows[0].id, host.windows[1].id);
        has(&mut host, &app, |app| app.files_there.pick("/tmp"));
        assert!(hears(&mut host, |_| !app.borrow().picked_there.is_empty()));
        assert_eq!((app.borrow().picked_there.clone(), app.borrow().picked.len()), (vec![PathBuf::from("/tmp/there.txt")], 0));
        assert!(asked()[0].starts_with(&format!("OpenFile x11:{second:x} | Open a file | ")) && first != second, "{:?}", asked());

        // A dialog that stays up, in the second window; then the app closes that window.
        stand.how.lock().unwrap().ends = Ends::Never;
        has(&mut host, &app, |app| app.files_there.pick("/tmp"));
        assert!(hears(&mut host, |_| asked().len() == 2));
        has(&mut host, &app, |app| app.second.open = false);
        assert!(hears(&mut host, |_| asked().len() == 3) && host.windows.len() == 1);
        assert!(asked()[2].starts_with("Close /org/freedesktop/portal/desktop/request/") && asked()[2].ends_with("/taut2"), "{:?}", asked());
    }

    /// Where the portal has no preference, or there is no portal, a window follows the desktop
    /// theme from XSETTINGS, and follows the person's theme changes. A portal preference comes
    /// before the theme.
    #[test]
    fn a_window_goes_by_the_desktops_theme_where_the_portal_knows_of_no_wish() {
        let alone = alone();
        let Some(desktop) = conn(c"_XSETTINGS_S0") else { return eprintln!("no display: not tested") };
        // A stand-in XSETTINGS manager: it owns the selection settings are found by and keeps
        // them on its window. (X has them by the time it answers a later query.)
        let theme = |name: &str| {
            let kept = desktop.atom(c"_XSETTINGS_SETTINGS");
            desktop.leave(desktop.window, kept, kept, &settings_of(true, &[("Xft/DPI", Kept::Number(98_304)), ("Net/ThemeName", Kept::Text(name))]));
            desktop.property(desktop.window, desktop.mailbox, false);
        };
        desktop.set(Vec::new());
        theme("Adwaita-dark");

        // On no bus the theme is all there is: dark from the first run, then light and dark
        // again as the person switches themes.
        let (alone, mut host, app) = following_alone(alone, Session::on(None)).expect("a display");
        assert_eq!((host.settings, host.ui.system_dark(), app.borrow().dark.clone()), (desktop.window, true, vec![true]));
        for (name, dark) in [("Adwaita", false), ("Yaru-dark", true)] {
            theme(name);
            assert!(wait(&mut host, |host| host.ui.system_dark() == dark) && app.borrow().dark.last() == Some(&dark), "{name}");
        }
        drop((host, app));

        // With a portal saying the person prefers light, that comes before the dark theme. With
        // no preference there, the theme counts again; and a dark preference comes before a
        // light theme.
        let Some(own) = OwnBus::start() else { return drop((desktop, eprintln!("no dbus-daemon to start a bus with: not tested further"))) };
        let stand = StandIn::start(&own.address, false, Ends::Closed);
        let (alone, mut host, app) = following_alone(alone, Session::on(Bus::open(&own.address))).expect("a display");
        assert!(hears(&mut host, |host| !host.ui.system_dark()), "the person's wish goes before the theme");
        stand.say(0);
        assert!(hears(&mut host, |host| host.ui.system_dark()), "no wish: the theme counts");
        theme("Adwaita");
        assert!(hears(&mut host, |host| !host.ui.system_dark()));
        stand.say(1);
        assert!(hears(&mut host, |host| host.ui.system_dark()) && app.borrow().dark.last() == Some(&true));
        // The settings leave the display before another test opens a window on it.
        drop((host, app, stand));
        drop(desktop);
        drop(alone);
    }

    /// A window hears the person's other preferences: high contrast and reduced motion from
    /// XSETTINGS where no portal says them, and from the portal along with the accent colour.
    /// Each is news, and the interface runs for it.
    #[test]
    fn a_window_follows_high_contrast_less_movement_and_the_colour_picked() {
        use crate::theme::ContrastColors;
        let alone = alone();
        let Some(desktop) = conn(c"_XSETTINGS_S0") else { return eprintln!("no display: not tested") };
        let settings = |list: &[(&str, Kept)]| {
            let kept = desktop.atom(c"_XSETTINGS_SETTINGS");
            desktop.leave(desktop.window, kept, kept, &settings_of(true, list));
            desktop.property(desktop.window, desktop.mailbox, false);
        };
        desktop.set(Vec::new());
        settings(&[("Net/ThemeName", Kept::Text("HighContrast")), ("Gtk/EnableAnimations", Kept::Number(0))]);

        // On no bus XSETTINGS is all there is: high contrast in the library's light colours and
        // reduced motion from the first run; then dark high contrast, then neither.
        let (alone, mut host, app) = following_alone(alone, Session::on(None)).expect("a display");
        assert_eq!(host.ui.preferences.current, Preferences { high_contrast: Some(ContrastColors::LIGHT), reduced_motion: true, ..Preferences::default() });
        settings(&[("Net/ThemeName", Kept::Text("HighContrastInverse"))]);
        assert!(wait(&mut host, |host| host.ui.preferences.current == Preferences { dark: true, high_contrast: Some(ContrastColors::DARK), ..Preferences::default() }));
        settings(&[("Net/ThemeName", Kept::Text("Adwaita"))]);
        assert!(wait(&mut host, |host| host.ui.preferences.current == Preferences::default()));
        drop((host, app));

        // With a portal: the person asks for more contrast, for reduced motion, and picks an
        // accent; the interface runs for each.
        let Some(own) = OwnBus::start() else { return drop((desktop, eprintln!("no dbus-daemon to start a bus with: not tested further"))) };
        let stand = StandIn::start(&own.address, false, Ends::Closed);
        let (alone, mut host, app) = following_alone(alone, Session::on(Bus::open(&own.address))).expect("a display");
        heard_out(&mut host);
        assert_eq!(host.ui.preferences.current, Preferences::default());
        let mut runs = app.borrow().runs;
        stand.change("contrast", |how| how.contrast = Some(1));
        assert!(hears(&mut host, |host| host.ui.high_contrast()) && app.borrow().runs > runs);
        runs = app.borrow().runs;
        stand.change("reduced-motion", |how| how.motion = Some(1));
        assert!(hears(&mut host, |host| host.ui.reduced_motion()) && app.borrow().runs > runs);
        runs = app.borrow().runs;
        stand.change("accent-color", |how| how.accent = Some([0.0, 0.5, 1.0]));
        assert!(hears(&mut host, |host| host.ui.system_accent() == Some(Color::rgb(0x0080ff))) && app.borrow().runs > runs);
        assert_eq!(host.ui.preferences.current, Preferences { accent: Some(Color::rgb(0x0080ff)), high_contrast: Some(ContrastColors::LIGHT), reduced_motion: true, ..Preferences::default() });
        // A setting of another group changes: nothing runs.
        (runs, _) = (app.borrow().runs, stand.say_other());
        heard_out(&mut host);
        assert_eq!(app.borrow().runs, runs);
        drop((host, app, stand));
        drop(desktop);
        drop(alone);
    }

    /// The window-kind tests' app state: where its move strip is; how often the strip's button
    /// was pressed, and whether the strip itself ever was, as the interface heard it; what to
    /// ask of its window next run, once each; and what the window reported last run.
    #[derive(Default)]
    struct Kinds {
        strip: Rect,
        button: Rect,
        pressed: u32,
        held: bool,
        /// To ask: minimize, come forward, maximize or restore, fullscreen or not.
        minimize: bool,
        forward: bool,
        fill: Option<bool>,
        over_all: Option<bool>,
    }

    /// A window with the app's own title strip across the top, a button at its end, and a page
    /// under it.
    fn kinds(app: &mut Kinds, ui: &mut Ui) {
        let strip = ui.row().h(crate::Length::Px(30.0)).sense().open();
        ui.label("A title");
        ui.spacer();
        let button = ui.button("Close");
        ui.close();
        ui.set_title_bar(&strip);
        if std::mem::take(&mut app.minimize) {
            ui.minimize();
        }
        if std::mem::take(&mut app.forward) {
            ui.bring_to_front();
        }
        app.fill.take().into_iter().for_each(|on| ui.set_maximized(on));
        app.over_all.take().into_iter().for_each(|on| ui.set_full_screen(on));
        (app.strip, app.button, app.pressed, app.held) = (strip.rect, button.rect, app.pressed + button.clicked as u32, app.held || strip.pressed);
        ui.label("The page");
    }

    /// Such a window of the kind `asked` says, on the test display.
    fn of_a_kind(asked: Window) -> Option<(Alone, Rc<RefCell<Kinds>>)> {
        let app = Rc::new(RefCell::new(Kinds::default()));
        let described = app.clone();
        Some((host(asked, move |ui| kinds(&mut described.borrow_mut(), ui))?, app))
    }

    /// A pointer or button event with what `input` leaves out: the screen position and time.
    fn pointing(to: XWindow, kind: c_int, [x, y]: [c_int; 2], [x_root, y_root]: [c_int; 2], time: c_ulong, detail: c_uint) -> XEvent {
        // SAFETY: an event is plain numbers; these types have this layout.
        let mut event: XEvent = unsafe { zeroed() };
        event.input = InputEvent { kind, serial: 0, sent: 1, display: null_mut(), window: to, root: 0, child: 0, time, x, y, x_root, y_root, state: 0, detail };
        event
    }

    /// Where X has a window on screen: left, top, width, height.
    fn lies_at_x(conn: &Connection, window: XWindow) -> [c_int; 4] {
        let ([x, y], size) = (conn.place(window), size_at_x(conn, window).unwrap_or_default());
        [x, y, size[0] as c_int, size[1] as c_int]
    }

    /// A frameless window asks the window manager for no frame. With no window manager it is
    /// still moved by its strip and resized by its edges and corners: it follows the pointer
    /// since the grab, down to its least size. The pointer takes the shape of the edge under
    /// it, and what it does on the strip and edges is not the interface's. (Pointer input is
    /// sent as events with the screen places it would have: nothing is pressed for real. The
    /// window stays clear of the screen's middle, where an unused display has its pointer.)
    #[test]
    fn a_window_with_no_frame_says_so_and_goes_by_the_pointer_itself_where_nobody_moves_it() {
        let asked = Window { title: "taut bare", position: Some((40, 30)), width: 300.0, height: 200.0, min_width: 150.0, min_height: 90.0, frameless: true, ..Default::default() };
        // The pointer-driving library opens before the window so it closes after: X holds it
        // while the window's connection is open.
        let hands = hands();
        let Some(((_alone, mut host), app)) = of_a_kind(asked) else { return eprintln!("no display: not tested") };
        let (conn, scale, id) = (host.conn.clone(), host.scale, host.windows[0].id);
        // Where the test owns the display's pointer, it is moved to a corner first: what it
        // would report over the window is not what is sent here.
        if let Some(hands) = &hands {
            unsafe { (hands.XTestFakeMotionEvent)(conn.display, -1, 899, 599, 0) };
            lets(&mut host, 60);
        }
        assert_eq!(conn.numbers(id, c"_MOTIF_WM_HINTS"), [2, 0, 0, 0, 0], "it asks for no frame");
        assert!(host.windows[0].kind.edges && host.windows[0].grips && !host.windows[0].kind.transparent);
        if conn.managed() {
            return eprintln!("a window manager is there, and moves the window itself: not tested further");
        }
        let px = |unit: f32| (unit * scale) as c_int;
        let ([wide, high], least) = ([px(300.0), px(200.0)], [px(150.0), px(90.0)]);
        assert!(wait(&mut host, |host| lies_at_x(&host.conn, id) == [40, 30, wide, high]), "it lies at {:?}", lies_at_x(&conn, id));
        // A pointer event at a place in the window, wherever it lies: button down (4) or up
        // (5), or motion (6), `by` this far from there.
        let does = |host: &mut Windows, kind: c_int, [x, y]: [c_int; 2], by: [c_int; 2], time: c_ulong| {
            let [left, top, ..] = lies_at_x(&host.conn, id);
            let detail = if kind == 6 { 0 } else { 1 };
            send(host, id, pointing(id, kind, [x + by[0], y + by[1]], [left + x + by[0], top + y + by[1]], time, detail));
        };
        let goes_to = |host: &mut Windows, to: [c_int; 4]| {
            let there = |host: &Windows| lies_at_x(&host.conn, id) == to && host.windows[0].size == [to[2], to[3]] && host.ui.window_placement().is_some_and(|place| place.position == (to[0], to[1]));
            assert!(wait(host, there), "the window lies at {:?} and is told {:?}, and was to go to {to:?}", lies_at_x(&host.conn, id), host.ui.window_placement());
        };

        // Pressed on its strip and moved: all of it follows, each time from the grab point.
        // Released, it stays, and the pointer is the interface's again.
        let on_strip = middle(app.borrow().strip, scale);
        does(&mut host, 4, on_strip, [0, 0], 1000);
        assert!(host.windows[0].held.is_some() && !app.borrow().held, "the strip is at {on_strip:?}, and the pointer has hold of {:?}", host.windows[0].over);
        does(&mut host, 6, on_strip, [40, 30], 1010);
        goes_to(&mut host, [80, 60, wide, high]);
        // The window moved under the pointer by now: the screen position is what counts.
        does(&mut host, 6, on_strip, [-50, -25], 1020);
        goes_to(&mut host, [30, 35, wide, high]);
        does(&mut host, 5, on_strip, [0, 0], 1030);
        assert!(host.windows[0].held.is_none());
        does(&mut host, 6, [px(150.0), px(100.0)], [0, 0], 1040);
        assert!(host.ui.pointer().is_some() && lies_at_x(&conn, id) == [30, 35, wide, high]);

        // By its right edge it gets wider, nothing else; by its bottom right corner wider and
        // taller.
        does(&mut host, 4, [wide - 2, high / 2], [0, 0], 3000);
        does(&mut host, 6, [wide - 2, high / 2], [50, 400], 3010);
        goes_to(&mut host, [30, 35, wide + 50, high]);
        does(&mut host, 5, [wide + 48, high / 2], [0, 0], 3020);
        does(&mut host, 4, [wide + 48, high - 2], [0, 0], 4000);
        does(&mut host, 6, [wide + 48, high - 2], [-50, 20], 4010);
        goes_to(&mut host, [30, 35, wide, high + 20]);
        does(&mut host, 5, [wide - 2, high + 18], [0, 0], 4020);
        // By its top left corner, that corner follows and the opposite one stays; pushed past
        // the least size, it stops.
        does(&mut host, 4, [1, 1], [0, 0], 5000);
        does(&mut host, 6, [1, 1], [20, 10], 5010);
        goes_to(&mut host, [50, 45, wide - 20, high + 10]);
        does(&mut host, 6, [1, 1], [4000, 4000], 5020);
        goes_to(&mut host, [30 + wide - least[0], 35 + high + 20 - least[1], least[0], least[1]]);
        does(&mut host, 5, [1, 1], [0, 0], 5030);
        assert!(host.windows[0].held.is_none() && !app.borrow().held && app.borrow().pressed == 0, "none of it was the interface's to hear");

        // Over an edge the pointer has that edge's shape and has left the interface; back
        // inside it is the interface's, with the shape it asks for.
        let over = |host: &mut Windows, at: [c_int; 2]| {
            does(host, 6, at, [0, 0], 6000);
            (host.windows[0].over, host.windows[0].cursor, host.ui.pointer().is_some())
        };
        assert_eq!(over(&mut host, [1, least[1] / 2]), (HitTest::Edge(-1, 0), Cursor::ResizeX, false));
        assert_eq!(over(&mut host, [least[0] / 2, least[1] / 2 + 20]), (HitTest::Client, Cursor::Arrow, true));
        assert_eq!(over(&mut host, [least[0] - 2, least[1] - 2]), (HitTest::Edge(1, 1), Cursor::ResizeDown, false));
        assert_eq!(over(&mut host, [least[0] - 2, 1]), (HitTest::Edge(1, -1), Cursor::ResizeUp, false));
        // The interface runs meanwhile, and the pointer keeps the edge's shape.
        host.rerun = true;
        host.turn();
        assert_eq!(host.windows[0].cursor, Cursor::ResizeUp);
        send(&mut host, id, input(id, 8, [0, 0], 0));
        assert_eq!((host.windows[0].over, host.windows[0].cursor), (HitTest::Client, Cursor::Arrow));

        // The strip's button works as ever. A double press on the strip moves nothing: it asks
        // a window manager, and there is none.
        let on_button = middle(app.borrow().button, scale);
        for (kind, time) in [(6, 7000), (4, 7010), (5, 7020)] {
            does(&mut host, kind, on_button, [0, 0], time);
        }
        assert_eq!((app.borrow().pressed, host.windows[0].held.is_some()), (1, false));
        let on_strip = middle(app.borrow().strip, scale);
        for (kind, time, held) in [(4, 8000, true), (5, 8050, false), (4, 8200, false), (5, 8250, false)] {
            does(&mut host, kind, on_strip, [0, 0], time);
            assert_eq!(host.windows[0].held.is_some(), held, "at {time}");
        }
    }

    /// A second window is frameless where its app asks, while the first keeps its frame; a
    /// see-through window also asks for no frame and keeps its size.
    #[test]
    fn a_second_window_and_a_see_through_one_ask_for_no_frame() {
        let kept = Rc::new(RefCell::new(SecondWindow::new(200.0, 120.0)));
        let app = kept.clone();
        let describe = move |ui: &mut Ui| {
            ui.label("The first window");
            if ui.frameless_window(&mut app.borrow_mut(), "Tools") {
                let strip = ui.row().h(crate::Length::Px(24.0)).sense().open();
                ui.label("Tools");
                ui.close();
                ui.set_title_bar(&strip);
                ui.close();
            }
        };
        let Some((alone, host)) = host(Window { title: "taut framed", width: 320.0, height: 200.0, ..Default::default() }, describe) else { return eprintln!("no display: not tested") };
        let (conn, [first, second]) = (host.conn.clone(), [host.windows[0].id, host.windows[1].id]);
        assert_eq!([first, second].map(|window| conn.numbers(window, c"_MOTIF_WM_HINTS")), [vec![], vec![2, 0, 0, 0, 0]]);
        assert!(host.windows[1].kind.edges && host.windows[1].grips && !host.windows[0].kind.frameless && !host.windows[0].grips);
        drop((host, alone));

        let Some(((_alone, host), _)) = of_a_kind(Window { title: "taut clear", width: 160.0, height: 100.0, transparent: true, ..Default::default() }) else { return };
        let (conn, id, [wide, high]) = (host.conn.clone(), host.windows[0].id, host.windows[0].size.map(|side| side as c_ulong));
        let asks = conn.numbers(id, c"WM_NORMAL_HINTS");
        assert_eq!((conn.numbers(id, c"_MOTIF_WM_HINTS"), asks[0], &asks[5..9]), (vec![2, 0, 0, 0, 0], 1 << 4 | 1 << 5, &[wide, high, wide, high][..]));
        assert!(host.windows[0].kind.transparent && !host.windows[0].kind.edges && host.ui.theme.bg == Color::NONE);
    }

    library!(Hands, ["libXtst.so.6", "libXtst.so"], {
        fn XTestFakeMotionEvent(Display, c_int, c_int, c_int, c_ulong) -> c_int;
        fn XTestFakeButtonEvent(Display, c_uint, c_int, c_ulong) -> c_int;
    });

    /// Real pointer control of the display, as a hand would: only on a display made for tests
    /// that nobody looks at (made by `xvfb-run`, which says so in its auth file's path).
    /// Anywhere else the pointer is somebody's and is left alone.
    fn hands() -> Option<Hands> {
        Hands::open().filter(|_| std::env::var("XAUTHORITY").is_ok_and(|key| key.contains("xvfb-run")))
    }

    /// Has a host take in X events for so many ms.
    fn lets<App: FnMut(&mut Ui)>(host: &mut Host<App>, millis: u64) {
        let (conn, asked) = (host.conn.clone(), Instant::now());
        while asked.elapsed() < Duration::from_millis(millis) {
            conn.property(conn.window, conn.mailbox, false);
            host.turn();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A person grabs a frameless window with the display's real pointer: by the strip it
    /// moves, by an edge it resizes, and a double press on the strip maximizes and restores it.
    /// The app's own buttons minimize, maximize and make it fullscreen. With a window manager
    /// all of that is the manager's doing at the window's request; with none, the window moves,
    /// resizes and goes fullscreen itself, and the rest is unavailable. (On a display of its
    /// own; once without a window manager and once with each available.)
    #[test]
    #[ignore]
    fn a_person_moves_and_sizes_a_window_with_no_frame_and_its_app_has_it_stand_as_asked() {
        let Some(hands) = hands() else { return eprintln!("no display of the tests' own to work the pointer of: not tested") };
        let asked = Window { title: "taut bare", position: Some((200, 150)), width: 300.0, height: 200.0, min_width: 150.0, min_height: 90.0, frameless: true, ..Default::default() };
        let ((_alone, mut host), app) = of_a_kind(asked).expect("a display");
        let (conn, scale, id, screen) = (host.conn.clone(), host.scale, host.windows[0].id, screen_size(&host.conn.x, host.conn.display).map(|side| side as c_int));
        let px = |unit: f32| (unit * scale) as c_int;
        let ([wide, high], managed) = ([px(300.0), px(200.0)], conn.managed());
        eprintln!("with a window manager: {managed}; it moves and sizes windows when asked: {}", conn.offers(c"_NET_WM_MOVERESIZE"));
        // A window manager maps a window before placing it.
        settle(&mut host);
        assert!(
            wait(&mut host, |host| lies_at_x(&host.conn, id) == [200, 150, wide, high] && host.ui.window_placement().is_some_and(|place| place.position == (200, 150))),
            "it lies at {:?}, and is told {:?}",
            lies_at_x(&conn, id),
            host.ui.window_placement()
        );
        assert_eq!(conn.numbers(id, c"_NET_FRAME_EXTENTS").iter().sum::<c_ulong>(), 0, "nothing was put around it");
        // The pointer goes to a screen position; and its left button goes down or up.
        let goes = |host: &mut Windows, [x, y]: [c_int; 2]| {
            unsafe { (hands.XTestFakeMotionEvent)(host.conn.display, -1, x, y, 0) };
            lets(host, 60);
        };
        let button = |host: &mut Windows, down: bool| {
            unsafe { (hands.XTestFakeButtonEvent)(host.conn.display, 1, down as c_int, 0) };
            lets(host, 60);
        };
        let lies = |host: &mut Windows, to: [c_int; 4], what: &str| {
            let there = |host: &Windows| {
                lies_at_x(&host.conn, id) == to && host.ui.window_placement().is_some_and(|place| place.position == (to[0], to[1]) && !place.maximized) && host.ui.size().w == to[2] as f32 / scale
            };
            assert!(wait(host, there), "{what}: the window lies at {:?} and is told {:?}, and was to lie at {to:?}", lies_at_x(&host.conn, id), host.ui.window_placement());
        };

        // By its strip: pressed there, the pointer moves off, and the window with it.
        let on_strip = middle(app.borrow().strip, scale);
        goes(&mut host, [200 + on_strip[0], 150 + on_strip[1]]);
        button(&mut host, true);
        for by in [[8, 6], [30, 20], [60, 40]] {
            goes(&mut host, [200 + on_strip[0] + by[0], 150 + on_strip[1] + by[1]]);
        }
        button(&mut host, false);
        lies(&mut host, [260, 190, wide, high], "moved by its strip");
        // By its right edge: wider. By its top left corner: that corner follows.
        goes(&mut host, [260 + wide - 2, 190 + high / 2]);
        assert_eq!((host.windows[0].over, host.windows[0].cursor), (HitTest::Edge(1, 0), Cursor::ResizeX));
        button(&mut host, true);
        for by in [8, 30, 50] {
            goes(&mut host, [260 + wide - 2 + by, 190 + high / 2 + 3]);
        }
        button(&mut host, false);
        lies(&mut host, [260, 190, wide + 50, high], "made wider by its right edge");
        goes(&mut host, [261, 191]);
        button(&mut host, true);
        for by in [[-8, -6], [-30, -20], [-40, -30]] {
            goes(&mut host, [261 + by[0], 191 + by[1]]);
        }
        button(&mut host, false);
        lies(&mut host, [220, 160, wide + 90, high + 30], "made larger by its top left corner");
        assert!(!app.borrow().held && app.borrow().pressed == 0, "none of that was the interface's to hear");

        // A double press on the strip: with a window manager it maximizes, and another
        // restores; with none, nothing happens. (On the strip where it is now, wherever the
        // window lies.)
        let double = |host: &mut Windows| {
            let ([left, top, ..], on_strip) = (lies_at_x(&host.conn, id), middle(app.borrow().strip, scale));
            goes(host, [left + on_strip[0], top + on_strip[1]]);
            [true, false, true, false].into_iter().for_each(|down| button(host, down));
        };
        // The person releases the corner, and a moment later double-presses the strip.
        lets(&mut host, 2 * SETTLED.as_millis() as u64);
        double(&mut host);
        assert_eq!(wait(&mut host, |host| host.ui.maximized()), managed, "after a double press on its strip, the window is told {:?}", host.ui.window_placement());
        assert!(
            !managed || wait(&mut host, |host| host.windows[0].size[0] > wide + 90 && host.ui.window_placement().is_some_and(|place| place.position == (220, 160))),
            "filling the screen, it is told {:?}",
            host.ui.window_placement()
        );
        // The next double press is a separate one, not four presses in a row.
        lets(&mut host, 700);
        double(&mut host);
        lies(&mut host, [220, 160, wide + 90, high + 30], "back from filling the screen");

        // What its app asks: fullscreen, and back. The place it reports is its restore place
        // throughout.
        let asks = |host: &mut Windows, ask: &dyn Fn(&mut Kinds)| {
            ask(&mut app.borrow_mut());
            host.rerun = true;
            lets(host, 100);
        };
        asks(&mut host, &|app| app.over_all = Some(true));
        let over_all = |host: &Windows| host.ui.full_screen() && host.windows[0].size == screen && lies_at_x(&host.conn, id) == [0, 0, screen[0], screen[1]];
        assert!(wait(&mut host, over_all), "over all of the screen, the window lies at {:?}", lies_at_x(&conn, id));
        assert!(
            wait(&mut host, |host| host.ui.size().w == screen[0] as f32 / scale
                && host.ui.window_placement().is_some_and(|place| place.position == (220, 160) && place.width == (wide + 90) as f32 / scale && !place.maximized)),
            "it is told {:?}",
            host.ui.window_placement()
        );
        // Nothing is grabbed meanwhile: a press on its strip is the interface's.
        goes(&mut host, [on_strip[0], on_strip[1]]);
        assert_eq!(host.windows[0].over, HitTest::Client);
        asks(&mut host, &|app| app.over_all = Some(false));
        lies(&mut host, [220, 160, wide + 90, high + 30], "back from lying over the screen");
        assert!(!host.ui.full_screen());

        // Maximize and restore, minimize and come forward: a window manager's to do.
        asks(&mut host, &|app| app.fill = Some(true));
        assert_eq!(wait(&mut host, |host| host.ui.maximized() && host.windows[0].size[0] > wide + 90), managed, "asked to fill the screen, the window is told {:?}", host.ui.window_placement());
        asks(&mut host, &|app| app.fill = Some(false));
        lies(&mut host, [220, 160, wide + 90, high + 30], "back from filling the screen at its app's asking");
        asks(&mut host, &|app| app.minimize = true);
        assert_eq!(wait(&mut host, |host| host.ui.window_minimized()), managed, "asked to be minimized");
        asks(&mut host, &|app| app.forward = true);
        assert!(wait(&mut host, |host| !host.ui.window_minimized()));
        lies(&mut host, [220, 160, wide + 90, high + 30], "back from minimized");
        // The pointer is left in a screen corner, out of the other tests' way.
        goes(&mut host, [screen[0] - 1, screen[1] - 1]);
    }

    /// A see-through window has a 32-bit visual, and X gets each pixel's alpha as drawn: opaque
    /// where something opaque is, half where half, zero where nothing is drawn. With a
    /// compositing manager, what is behind then shows through by that: here another program's
    /// green window, read off the screen. (On a display of its own; with no compositing
    /// manager, only what X got is checked.)
    #[test]
    #[ignore]
    fn a_see_through_window_hands_x_how_solid_each_pixel_is_and_lets_what_is_behind_it_show() {
        // It waits for the other window tests before it shows its first.
        let (alone, asking, behind) = (alone(), Asking::open().unwrap(), conn(c"TAUT_TEST_BEHIND").expect("a display"));
        // The window behind: plain green, large enough to lie under the other.
        // SAFETY: a window of the test's own, on its own connection.
        let green = unsafe {
            let (green, _) = new_window(&behind.x, behind.display, [420, 320], [1, 1], Some((240, 140)), [false; 2]);
            (asking.XSetWindowBackground)(behind.display, green, 0x00ff00);
            (behind.x.XMapWindow)(behind.display, green);
            (behind.x.XFlush)(behind.display);
            green
        };
        std::thread::sleep(Duration::from_millis(500));
        let describe = |ui: &mut Ui| {
            ui.el().size(40.0, 40.0).fill(Color::rgb(0xff0000)).show();
            ui.el().size(40.0, 40.0).fill(Color::rgb(0x0000ff).with_alpha(0.5)).show();
            ui.label("Written");
        };
        let (_alone, mut host) =
            host_on(alone, Session::on(None), Window { title: "taut clear", position: Some((330, 220)), width: 200.0, height: 160.0, transparent: true, ..Default::default() }, describe)
                .expect("a display");
        let (conn, scale, id, size) = (host.conn.clone(), host.scale, host.windows[0].id, host.windows[0].size);
        if host.windows[0].kind.deep.is_none() {
            return eprintln!("the display has no way of showing 32 bits to a pixel: not tested");
        }
        settle(&mut host);
        lets(&mut host, 300);
        // What X has of the window itself: each pixel with its alpha.
        let (rows, px) = (on_screen(&conn, id, size), |unit: f32| (unit * scale) as usize);
        let at = |x: f32, y: f32| rows[px(y)][px(x)] as u32;
        assert_eq!((at(20.0, 20.0), at(150.0, 20.0), at(150.0, 150.0)), (0xffff_0000, 0, 0));
        let half = at(20.0, 60.0);
        assert!((0x7f..=0x80).contains(&(half >> 24)) && half & 0xff == half >> 24 && half & 0x00ff_ff00 == 0, "{half:08x}");
        let lettered = rows[px(80.0)..px(100.0)].iter().flatten().filter(|pixel| (1..255).contains(&(**pixel as u32 >> 24))).count();
        assert!(
            lettered > 0 && rows.iter().flatten().all(|pixel| [16, 8, 0].iter().all(|shift| pixel >> shift & 0xff <= pixel >> 24 & 0xff)),
            "letters are written with how solid they are, and no colour is more than that"
        );
        // What shows on screen, where a compositing manager runs.
        let name = CString::new(format!("_NET_WM_CM_S{}", unsafe { (conn.x.XDefaultScreen)(conn.display) })).unwrap();
        if unsafe { (conn.x.XGetSelectionOwner)(conn.display, conn.atom(&name)) } == 0 {
            return eprintln!("no compositing manager: what shows through the window not seen");
        }
        let [left, top, ..] = lies_at_x(&conn, id);
        let screen = on_screen(&conn, root(&conn.x, conn.display), [900, 600]);
        let shows = |x: f32, y: f32| screen[top as usize + px(y)][left as usize + px(x)] as u32 & 0x00ff_ffff;
        let mixed = shows(20.0, 60.0);
        eprintln!(
            "on the screen: solid {:06x}, half {mixed:06x}, clear {:06x} and {:06x}; the windows lie at {:?} and {:?}",
            shows(20.0, 20.0),
            shows(150.0, 20.0),
            shows(150.0, 150.0),
            lies_at_x(&conn, id),
            lies_at_x(&behind, green)
        );
        assert_eq!((shows(20.0, 20.0), shows(150.0, 20.0), shows(150.0, 150.0)), (0xff0000, 0x00ff00, 0x00ff00), "solid where it is solid, and the window behind where it is clear");
        assert!(mixed >> 16 == 0 && (0x7e..=0x81).contains(&(mixed >> 8 & 0xff)) && (0x7e..=0x81).contains(&(mixed & 0xff)), "half of each where it lets half through: {mixed:06x}");
    }

    // ---- screens, second-window kinds, and closing ----

    // What only these tests ask: monitors of their own, and the stacking order.
    library!(Monitors, ["libXrandr.so.2", "libXrandr.so"], {
        fn XRRSetMonitor(Display, XWindow, *mut Monitor) -> ();
        fn XRRDeleteMonitor(Display, XWindow, Atom) -> ();
        fn XRRGetMonitors(Display, XWindow, c_int, *mut c_int) -> *mut Monitor;
        fn XRRFreeMonitors(*mut Monitor) -> ();
    });
    library!(Stacking, ["libX11.so.6", "libX11.so"], {
        fn XQueryTree(Display, XWindow, *mut XWindow, *mut XWindow, *mut *mut XWindow, *mut c_uint) -> c_int;
    });

    /// Two monitors on the test display while this lives: its left half, the main one showing
    /// X's own output, and the right half, two thirds as tall. (An Xvfb display has one output,
    /// of which X makes one monitor; one showing that output replaces it.) Made on a connection
    /// of its own, which RandR keeps them for.
    struct Desk {
        x: Xlib,
        display: Display,
        set: Monitors,
        names: [Atom; 2],
        all: [Rect; 2],
    }

    fn desk() -> Option<Desk> {
        let (x, set) = (Xlib::open()?, Monitors::open()?);
        let display = unsafe { (x.XOpenDisplay)(null()) };
        if display.is_null() {
            return None;
        }
        let (top, [wide, high], mut count) = (root(&x, display), screen_size(&x, display).map(|side| side as c_int), 0);
        let made = unsafe { (set.XRRGetMonitors)(display, top, 1, &mut count) };
        let mut output = unsafe { (count > 0 && (*made).outputs > 0).then(|| *(*made).shown_by) };
        unsafe { (set.XRRFreeMonitors)(made) };
        let names = [c"TAUT_LEFT", c"TAUT_RIGHT"].map(|name| unsafe { (x.XInternAtom)(display, name.as_ptr(), 0) });
        let half = wide / 2;
        let shown_by = output.as_mut().map_or(null_mut(), |output| output as *mut c_ulong);
        let mut left = Monitor { name: names[0], main: 1, made: 0, outputs: output.is_some() as c_int, x: 0, y: 0, width: half, height: high, width_mm: 300, height_mm: 200, shown_by };
        let mut right = Monitor { name: names[1], main: 0, made: 0, outputs: 0, x: half, y: 0, width: wide - half, height: high * 2 / 3, width_mm: 300, height_mm: 130, shown_by: null_mut() };
        unsafe { ((set.XRRSetMonitor)(display, top, &mut left), (set.XRRSetMonitor)(display, top, &mut right), (x.XFlush)(display)) };
        // A query X answers after what it was asked before.
        screen_size(&x, display);
        let all = [left, right].map(|one| rect(one.x as f32, one.y as f32, one.width as f32, one.height as f32));
        Some(Desk { x, display, set, names, all })
    }

    impl Drop for Desk {
        fn drop(&mut self) {
            // The libraries are dropped after the display closes.
            let top = root(&self.x, self.display);
            unsafe {
                self.names.iter().for_each(|name| (self.set.XRRDeleteMonitor)(self.display, top, *name));
                (self.x.XCloseDisplay)(self.display);
            }
        }
    }

    /// The windows on screen bottom to top: as X has them with no window manager, as the window
    /// manager says otherwise.
    fn stacking(conn: &Connection) -> Vec<XWindow> {
        if conn.managed() {
            return conn.numbers(root(&conn.x, conn.display), c"_NET_CLIENT_LIST_STACKING");
        }
        let (asking, (mut top, mut parent, mut children, mut count)) = (Stacking::open().unwrap(), (0, 0, null_mut(), 0));
        unsafe {
            (asking.XQueryTree)(conn.display, root(&conn.x, conn.display), &mut top, &mut parent, &mut children, &mut count);
            let found = std::slice::from_raw_parts(children, count as usize).to_vec();
            (conn.x.XFree)(children.cast());
            found
        }
    }

    /// These tests' app state: its second windows, whether it has unsaved changes, what it
    /// heard, how often the first window's button was pressed, and what was typed there.
    #[derive(Default)]
    struct Owned {
        windows: Vec<(SecondWindow, &'static str)>,
        unsaved: bool,
        asked: Vec<&'static str>,
        pressed: u32,
        typed: String,
        button: Rect,
        boxed: Rect,
    }

    /// A first window as `window` says, with a button and a text box, whose app describes its
    /// second windows; with unsaved changes, each window refuses to close.
    fn owned(alone: std::sync::MutexGuard<'static, ()>, window: Window, app: &Rc<RefCell<Owned>>) -> Alone {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            let mut hear = |ui: &mut Ui, name: &'static str| {
                if ui.close_requested() && !app.asked.contains(&name) {
                    app.asked.push(name);
                }
                if app.unsaved {
                    ui.cancel_close("There are unsaved changes");
                }
            };
            hear(ui, "first");
            let button = ui.button("Press");
            app.pressed += button.clicked as u32;
            (app.button, app.boxed) = (button.rect, ui.text_box(&mut app.typed, "").rect);
            for (state, title) in app.windows.iter_mut() {
                if ui.window(state, title) {
                    hear(ui, title);
                    ui.label(title);
                    ui.close();
                }
            }
        };
        host_on(alone, Session::on(None), window, describe).expect("a display")
    }

    /// Writes CARDINAL (6) numbers on the root window under an atom, as a window manager does;
    /// or deletes them, for none.
    fn write_on_root(conn: &Connection, name: &CStr, numbers: &[f32]) {
        let (top, name) = (root(&conn.x, conn.display), conn.atom(name));
        match numbers {
            [] => _ = conn.property(top, name, true),
            _ => conn.leave(top, name, 6, &numbers.iter().map(|number| *number as c_ulong).collect::<Vec<_>>()),
        }
        unsafe { (conn.x.XFlush)(conn.display) };
    }

    /// The screens are RandR's monitors, main one first, at the display's scale, each with its
    /// work area as a window manager states it; a window hears again when any of it changes.
    /// (It makes monitors, so it runs only when asked, on a display of its own.)
    #[test]
    #[ignore]
    fn the_screens_are_the_monitors_randr_has_and_a_window_hears_when_they_change() {
        let app = Rc::new(RefCell::new(Owned::default()));
        let (alone, mut host) = owned(alone(), Window { title: "taut screens", width: 300.0, height: 200.0, position: Some((10, 10)), ..Default::default() }, &app);
        let (conn, scale) = (host.conn.clone(), host.scale);
        let [wide, high] = screen_size(&conn.x, conn.display).map(|side| side as f32);
        // One output, one monitor: one screen, the whole display.
        let all = |host: &Windows| host.ui.screens().iter().map(|screen| screen.rect).collect::<Vec<_>>();
        assert_eq!((all(&host), host.ui.screens()[0].scale), (vec![rect(0.0, 0.0, wide, high)], scale));
        // Two monitors: the window hears, main one first, and that it is on that one.
        let desk = desk().expect("RandR");
        assert!(wait(&mut host, |host| host.ui.screens().len() == 2), "the window was not told: {:?}", host.ui.screens());
        assert_eq!((all(&host), host.ui.screen()), (desk.all.to_vec(), Some(0)));
        assert!(host.ui.screens().iter().all(|screen| screen.scale == scale));
        let works = |host: &Windows| host.ui.screens().iter().map(|screen| screen.work_area).collect::<Vec<_>>();
        // The work area a window manager states (or the test, where none does): one for all
        // monitors, each getting what overlaps it; then one per monitor.
        if !conn.managed() {
            let [left, right] = desk.all;
            assert_eq!(works(&host), desk.all.to_vec());
            write_on_root(&conn, c"_NET_WORKAREA", &[0.0, 30.0, wide, high - 30.0]);
            let cut = [rect(0.0, 30.0, left.w, left.h - 30.0), rect(right.x, 30.0, right.w, right.h - 30.0)];
            assert!(wait(&mut host, |host| works(host) == cut), "{:?}", works(&host));
            write_on_root(&conn, c"_GTK_WORKAREAS_D0", &[0.0, 30.0, left.w, left.h - 30.0, right.x, 0.0, right.w, right.h]);
            write_on_root(&conn, c"_NET_WORKAREA", &[0.0, 30.0, wide, high - 30.0, 0.0, 0.0, wide, high]);
            assert!(wait(&mut host, |host| works(host) == [cut[0], right]), "{:?}", works(&host));
            write_on_root(&conn, c"_GTK_WORKAREAS_D0", &[]);
            write_on_root(&conn, c"_NET_WORKAREA", &[]);
            assert!(wait(&mut host, |host| works(host) == desk.all), "{:?}", works(&host));
        }
        // The monitors go: one screen again.
        drop(desk);
        assert!(wait(&mut host, |host| all(host) == [rect(0.0, 0.0, wide, high)]), "{:?}", all(&host));
        drop((host, alone));
    }

    /// On two monitors: a window opens centred on the screen its app names, a second one too;
    /// one placed on no screen moves onto the nearest; a dialog opens centred on the first
    /// window. (It makes monitors and places windows, so it runs only when asked, on a display
    /// of its own.)
    #[test]
    #[ignore]
    fn a_window_opens_on_the_screen_its_app_names_and_a_dialog_over_the_first() {
        let alone = alone();
        let desk = desk().expect("RandR");
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![
            (SecondWindow::new(200.0, 120.0).on_screen(0), "On the left"),
            (SecondWindow::new(160.0, 100.0).position(20_000, 50), "Lost"),
            (SecondWindow { open: false, ..SecondWindow::new(180.0, 90.0).dialog() }, "Save?"),
        ];
        let (_alone, mut host) = owned(alone, Window { title: "taut on the right", screen: Some(1), width: 300.0, height: 200.0, ..Default::default() }, &app);
        let (conn, scale, works) = (host.conn.clone(), host.scale, host.ui.screens().iter().map(|screen| screen.work_area).collect::<Vec<_>>());
        settle(&mut host);
        // A window's middle, frame included, and where it lies, as X has it.
        let middle = |window: XWindow| {
            let ((x, y), [left, right, up, down]) = (corner_at_x(&conn, window), [0, 1, 2, 3].map(|side| conn.numbers(window, c"_NET_FRAME_EXTENTS").get(side).map_or(0, |wide| *wide as i32)));
            let size = size_at_x(&conn, window).unwrap().map(|side| side as i32);
            pt((x + (size[0] + left + right) / 2) as f32, (y + (size[1] + up + down) / 2) as f32)
        };
        let near = |at: Point, to: Point| (at.x - to.x).abs() <= 16.0 && (at.y - to.y).abs() <= 16.0;
        let centre = |work: Rect| pt(work.x + work.w / 2.0, work.y + work.h / 2.0);
        let [first, left, lost] = [0, 1, 2].map(|at| host.windows[at].id);
        // Without a window manager to the pixel; with one, as near as framing a window it had
        // not yet framed allows.
        assert!(near(middle(first), centre(works[1])) && near(middle(left), centre(works[0])), "{:?} {:?} in {works:?}", middle(first), middle(left));
        assert!(wait(&mut host, |host| host.ui.screen() == Some(1)), "{:?}", host.ui.window_placement());
        // Beyond both right edges: onto the right monitor, at its right edge.
        let ((x, y), [wide, _]) = (corner_at_x(&conn, lost), size_at_x(&conn, lost).unwrap());
        assert!(x as f32 >= works[1].x && (x + wide as i32) as f32 <= works[1].right() + 16.0 && (x + wide as i32) as f32 >= works[1].right() - 16.0 && y == 50, "{x} {y} on {works:?}");
        // A dialog: centred on the first window.
        (app.borrow_mut().windows[2].0.open, host.rerun) = (true, true);
        host.turn();
        settle(&mut host);
        let dialog = host.windows[3].id;
        assert!(near(middle(dialog), middle(first)), "{:?} over {:?}", middle(dialog), middle(first));
        assert!((180.0 * scale) as c_uint == size_at_x(&conn, dialog).unwrap()[0]);
        drop(desk);
    }

    /// Each kind of second window declares itself to the window manager before mapping: a tool
    /// window and a dialog are transient for the first; a tool window is "utility" (off the
    /// task bar), a dialog "modal", one over all others "above". With a window manager, the
    /// transient ones stay above the first. (Only when asked, on a display of its own, with or
    /// without a window manager.)
    #[test]
    #[ignore]
    fn second_windows_of_each_kind_say_to_the_window_manager_what_they_are() {
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![
            (SecondWindow::new(200.0, 120.0), "Plain"),
            (SecondWindow::new(160.0, 200.0).tool(), "Tools"),
            (SecondWindow::new(140.0, 80.0).on_top(), "Clock"),
            (SecondWindow::new(220.0, 100.0).dialog(), "Save?"),
        ];
        let (_alone, mut host) = owned(alone(), Window { title: "taut kinds", width: 360.0, height: 240.0, ..Default::default() }, &app);
        let conn = host.conn.clone();
        settle(&mut host);
        let [first, plain, tools, clock, dialog] = [0, 1, 2, 3, 4].map(|at| host.windows[at].id);
        let names = |names: &[&CStr]| names.iter().map(|name| conn.atom(name)).collect::<Vec<_>>();
        let (owner, kind) = (|window| conn.numbers(window, c"WM_TRANSIENT_FOR"), |window| conn.numbers(window, c"_NET_WM_WINDOW_TYPE"));
        let has = |window, states: &[&CStr]| names(states).iter().all(|state| conn.numbers(window, c"_NET_WM_STATE").contains(state));
        assert_eq!([plain, tools, clock, dialog].map(owner), [vec![], vec![first], vec![], vec![first]]);
        assert_eq!([tools, clock, dialog].map(kind), [names(&[c"_NET_WM_WINDOW_TYPE_UTILITY"]), names(&[c"_NET_WM_WINDOW_TYPE_NORMAL"]), names(&[c"_NET_WM_WINDOW_TYPE_DIALOG"])]);
        assert!(
            has(tools, &[c"_NET_WM_STATE_SKIP_TASKBAR"]) && has(clock, &[c"_NET_WM_STATE_ABOVE"]) && has(dialog, &[c"_NET_WM_STATE_MODAL"]),
            "{:?}",
            [tools, clock, dialog].map(|window| conn.numbers(window, c"_NET_WM_STATE"))
        );
        if !conn.managed() {
            return eprintln!("no window manager: what one does with the kinds of window is not seen");
        }
        // A window manager keeps the tool window and dialog above the first, also when the
        // first is activated; and the always-on-top one above all.
        conn.ask(first, c"_NET_ACTIVE_WINDOW", [1, 0, 0, 0, 0]);
        settle(&mut host);
        let order = stacking(&conn);
        let at = |window| order.iter().position(|one| *one == window).unwrap_or_else(|| panic!("{window} is not in {order:?}"));
        assert!(at(tools) > at(first) && at(dialog) > at(first) && at(clock) > at(first) && at(clock) > at(plain), "{order:?}: first {first}, tools {tools}, dialog {dialog}, clock {clock}");
        // Minimized, the first window takes the tool window and dialog with it, not the plain
        // one.
        conn.ask(first, c"WM_CHANGE_STATE", [3, 0, 0, 0, 0]);
        let hidden = |window| conn.numbers(window, c"WM_STATE").first() == Some(&3);
        assert!(wait(&mut host, |_| hidden(first) && hidden(tools)), "{:?}", [first, tools, dialog, plain].map(|window| conn.numbers(window, c"WM_STATE")));
        assert!(!hidden(plain));
    }

    /// While a dialog is open, the windows opened before it get neither pointer nor keyboard,
    /// and a press on one raises the dialog; once it closes, they get both again.
    #[test]
    #[ignore]
    fn a_dialog_keeps_the_windows_before_it_from_the_keyboard_and_the_pointer() {
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![(SecondWindow { open: false, ..SecondWindow::new(220.0, 100.0).dialog() }, "Save?")];
        let (_alone, mut host) = owned(alone(), Window { title: "taut asks", width: 320.0, height: 200.0, ..Default::default() }, &app);
        let (conn, scale, first) = (host.conn.clone(), host.scale, host.windows[0].id);
        let (button, boxed) = (middle(app.borrow().button, scale), middle(app.borrow().boxed, scale));
        click(&mut host, first, button);
        assert_eq!(app.borrow().pressed, 1);
        (app.borrow_mut().windows[0].0.open, host.rerun) = (true, true);
        host.turn();
        let dialog = host.windows[1].id;
        // The button is not pressed, nothing is typed, and the dialog is raised over the first
        // window (a window manager already keeping it there is asked to activate it).
        click(&mut host, first, button);
        click(&mut host, first, boxed);
        let key = unsafe { (Asking::open().unwrap().XKeysymToKeycode)(conn.display, 'a' as c_ulong) };
        send(&mut host, first, input(first, 2, boxed, key as c_uint));
        assert_eq!((app.borrow().pressed, app.borrow().typed.as_str()), (1, ""));
        let order = stacking(&conn);
        let at = |window| order.iter().position(|one| *one == window);
        assert!(conn.managed() || at(dialog) > at(first), "{order:?}");
        // It is answered and closed: the first window gets the pointer again.
        (app.borrow_mut().windows[0].0.open, host.rerun) = (false, true);
        host.turn();
        click(&mut host, first, button);
        assert_eq!(app.borrow().pressed, 2);
    }

    /// The person closes a window through the window manager: its app is asked; with unsaved
    /// changes it refuses and the window stays; without, it goes, and for the first window the
    /// program ends.
    #[test]
    #[ignore]
    fn the_app_is_asked_before_a_window_closes_and_can_say_no() {
        let app = Rc::new(RefCell::new(Owned::default()));
        (app.borrow_mut().windows, app.borrow_mut().unsaved) = (vec![(SecondWindow::new(200.0, 120.0), "Notes")], true);
        let (_alone, mut host) = owned(alone(), Window { title: "taut closing", width: 320.0, height: 200.0, ..Default::default() }, &app);
        let (conn, [first, notes]) = (host.conn.clone(), [0, 1].map(|at| host.windows[at].id));
        let closing = |window| message(window, conn.atom(c"WM_PROTOCOLS"), [conn.atom(c"WM_DELETE_WINDOW") as c_long, 0, 0, 0, 0]);
        send(&mut host, notes, closing(notes));
        send(&mut host, first, closing(first));
        assert_eq!((app.borrow().asked.clone(), host.windows.len(), host.done), (vec!["Notes", "first"], 2, false));
        assert!(size_at_x(&conn, notes).is_some() && app.borrow().windows[0].0.open);
        app.borrow_mut().unsaved = false;
        send(&mut host, notes, closing(notes));
        assert_eq!((host.windows.len(), app.borrow().windows[0].0.open, size_at_x(&conn, notes)), (1, false, None));
        send(&mut host, first, closing(first));
        assert!(host.done);
    }

    /// The drag-out tests' app state. Its first window has a drag source box (the number 7
    /// inside the window, `what` outside it) and a drop box that also takes files and text from
    /// other programs; its second window, while open, a box taking files and text.
    #[derive(Default)]
    struct DragSourceApp {
        what: Option<DragData<'static>>,
        from: Rect,
        on: Rect,
        /// What became of what was dragged out; what was dropped on the box, as a number, files
        /// and text.
        taken: Vec<DropEffect>,
        dropped: Option<u64>,
        files: Vec<PathBuf>,
        text: Option<String>,
        /// The second window, its box, and what was dropped on it.
        takes: SecondWindow,
        took: Rect,
        took_text: Option<String>,
    }

    fn drag_source_app(app: &mut DragSourceApp, ui: &mut Ui) {
        ui.column().pad(10.0).gap(10.0).open();
        let from = ui.button("Drag me");
        if ui.drag_source(&from, 7) {
            ui.label("seven");
            ui.close();
        }
        app.taken.extend(app.what.and_then(|what| ui.drag_out(&from, what)));
        let on = ui.button("Let go here");
        app.dropped = app.dropped.or(ui.dropped(&on));
        app.files.extend(ui.files_dropped(&on).into_iter().flatten());
        app.text = app.text.take().or(ui.text_dropped(&on));
        (app.from, app.on) = (from.rect, on.rect);
        ui.close();
        if ui.window(&mut app.takes, "Takes") {
            let took = ui.button("Let go in here");
            app.took_text = app.took_text.take().or(ui.text_dropped(&took));
            app.took = took.rect;
            ui.close();
        }
    }

    /// That app's windows, the first with its corner at `at` on screen.
    fn drag_source_window(what: Option<DragData<'static>>, takes: SecondWindow, at: (i32, i32)) -> Option<(Alone, Rc<RefCell<DragSourceApp>>)> {
        let app = Rc::new(RefCell::new(DragSourceApp { what, takes, ..Default::default() }));
        let described = app.clone();
        let window = Window { title: "taut dragging", position: Some(at), width: 300.0, height: 200.0, ..Default::default() };
        Some((host(window, move |ui| drag_source_app(&mut described.borrow_mut(), ui))?, app))
    }

    /// A window box's middle on screen.
    fn spot(host: &Windows, at: usize, of: Rect) -> [c_int; 2] {
        let ([x, y], scale) = (host.conn.place(host.windows[at].id), host.scale);
        [x + ((of.x + of.w / 2.0) * scale) as c_int, y + ((of.y + of.h / 2.0) * scale) as c_int]
    }

    library!(Keys, ["libXtst.so.6", "libXtst.so"], {
        fn XTestFakeKeyEvent(Display, c_uint, c_int, c_ulong) -> c_int;
    });
    library!(Codes, ["libX11.so.6", "libX11.so"], {
        fn XKeysymToKeycode(Display, c_ulong) -> c_uchar;
    });

    /// The display's pointer goes to a screen position, as a hand moves it; then the host takes
    /// in what follows.
    fn goes(hands: &Hands, host: &mut Windows, [x, y]: [c_int; 2]) {
        unsafe { (hands.XTestFakeMotionEvent)(host.conn.display, -1, x, y, 0) };
        lets(host, 50);
    }
    /// The left button goes down or up.
    fn presses(hands: &Hands, host: &mut Windows, down: bool) {
        unsafe { (hands.XTestFakeButtonEvent)(host.conn.display, 1, down as c_int, 0) };
        lets(host, 50);
    }

    /// Sends a window a client message of five numbers under an atom, as another program does.
    fn say(conn: &Connection, to: XWindow, name: &CStr, data: [c_long; 5]) {
        unsafe {
            let mut event: XEvent = zeroed();
            event.client = ClientEvent { kind: 33, serial: 0, sent: 1, display: conn.display, window: to, message: conn.atom(name), format: 32, data };
            (conn.x.XSendEvent)(conn.display, to, 0, 0, &mut event);
            (conn.x.XFlush)(conn.display);
        }
    }

    /// Another program accepting XDND file drops, as a file manager does, on a thread of its
    /// own: a 200×200 window at (500, 100) taking a URI list and reporting it did `does` with a
    /// drop. It sends on what it hears: each message's atom, the types offered, and what it
    /// read on a drop. A "TAUT_STOP" message ends it.
    fn takes_elsewhere(does: &'static CStr) -> (XWindow, std::sync::mpsc::Receiver<String>) {
        let (tell, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (conn, naming) = (conn(c"TAUT_TEST_TARGET").unwrap(), Naming::open().unwrap());
            let (x, display) = (&conn.x, conn.display);
            let id = unsafe { new_window(x, display, [200, 200], [1, 1], Some((500, 100)), [false; 2]).0 };
            conn.leave(id, conn.atom(c"XdndAware"), 4, &[5 as Atom]);
            unsafe { (x.XMapWindow)(display, id) };
            conn.property(id, conn.mailbox, false);
            tell.send(id.to_string()).unwrap();
            let name = |atom: c_long| unsafe {
                let name = (naming.XGetAtomName)(display, atom as Atom);
                let text = if name.is_null() { String::new() } else { CStr::from_ptr(name).to_string_lossy().into_owned() };
                (x.XFree)(name.cast());
                text
            };
            let mut from = 0;
            loop {
                let mut event: XEvent = unsafe { zeroed() };
                unsafe { (x.XNextEvent)(display, &mut event) };
                let (kind, message, data) = unsafe { (event.kind, event.client.message, event.client.data) };
                if kind != 33 {
                    continue;
                }
                let said = name(message as c_long);
                match said.as_str() {
                    "TAUT_STOP" => return,
                    "XdndEnter" => {
                        from = data[0] as XWindow;
                        tell.send(format!("XdndEnter {} {}", data[1] >> 24, data[2..].iter().filter(|kind| **kind != 0).map(|kind| name(*kind)).collect::<Vec<_>>().join(" "))).ok();
                    }
                    "XdndPosition" => {
                        tell.send(format!("XdndPosition {}", name(data[4]))).ok();
                        say(&conn, from, c"XdndStatus", [id as c_long, 1, 0, 0, conn.atom(does) as c_long]);
                    }
                    "XdndDrop" => {
                        let got = conn.read(id, conn.atom(c"XdndSelection"), conn.atom(FILES), data[2] as c_ulong).unwrap_or_default();
                        tell.send(format!("read {}", String::from_utf8_lossy(&got))).ok();
                        say(&conn, from, c"XdndFinished", [id as c_long, 1, conn.atom(does) as c_long, 0, 0]);
                    }
                    _ => _ = tell.send(said).ok(),
                }
            }
        });
        let id = heard.recv().unwrap().parse().unwrap();
        (id, heard)
    }

    /// Everything a thread sent so far (waiting a little for it).
    fn sent(heard: &std::sync::mpsc::Receiver<String>) -> Vec<String> {
        let mut all: Vec<String> = heard.recv_timeout(Duration::from_secs(1)).into_iter().collect();
        all.extend(heard.try_iter());
        all
    }

    /// Files dragged out of a window to another XDND program, with the display's real pointer,
    /// on a display of its own. Grabbed and moved inside the window, the drag is the window's;
    /// past its edge it goes out, and the program under the pointer gets enter and positions;
    /// dropped there, it reads them and reports a move (or copy), which the app hears. Back
    /// over its own window the drag is the window's again and drops there as ever; Escape
    /// cancels it anywhere.
    #[test]
    #[ignore = "works the pointer of a display of its own (xvfb-run)"]
    fn files_dragged_out_go_to_another_program_the_xdnd_way_and_come_back_home() {
        let Some(hands) = hands() else { return eprintln!("no display of the tests' own to work the pointer of: not tested") };
        let (keys, codes) = (Keys::open().unwrap(), Codes::open().unwrap());
        let paths: &'static [PathBuf] = Box::leak(Box::new([PathBuf::from("/tmp/one.txt"), PathBuf::from("/tmp/two words.png")]));
        let ((_alone, mut host), app) = drag_source_window(Some(DragData::Files(paths)), SecondWindow::default(), (50, 50)).expect("a display");
        let (target, heard) = takes_elsewhere(c"XdndActionMove");
        settle(&mut host);
        let (from, on) = (spot(&host, 0, app.borrow().from), spot(&host, 0, app.borrow().on));
        let id = host.windows[0].id;
        // Grabbed and moved a step: inside the window it is the window's.
        goes(&hands, &mut host, from);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [from[0] + 10, from[1]]);
        assert!(host.ui.drag_payload() == Some(7) && host.ui.drag_leaves_window() && host.out.is_none());
        // Past the window's edge it goes out; over the other program's window, that gets enter
        // and position, asked to copy.
        goes(&hands, &mut host, [420, 150]);
        assert!(host.out.as_ref().is_some_and(|out| out.from == id && out.over == 0) && !host.ui.drag_leaves_window());
        assert_eq!(host.conn.dragged_out_as(host.conn.atom(FILES)).as_deref(), Some(&b"file:///tmp/one.txt\r\nfile:///tmp/two%20words.png\r\n"[..]));
        goes(&hands, &mut host, [550, 150]);
        goes(&hands, &mut host, [560, 160]);
        assert_eq!(sent(&heard)[..2], ["XdndEnter 5 text/uri-list", "XdndPosition XdndActionCopy"]);
        assert!(host.out.as_ref().is_some_and(|out| out.over == target && out.takes.is_some()));
        assert_eq!(host.windows[0].cursor, Cursor::Move, "it may be let go there");
        // Dropped there, it is read and moved, the program says; the app hears.
        presses(&hands, &mut host, false);
        assert!(wait(&mut host, |host| host.out.is_none()), "the other program said it was done");
        lets(&mut host, 50);
        assert_eq!(sent(&heard).iter().filter(|said| said.starts_with("read")).collect::<Vec<_>>(), ["read file:///tmp/one.txt\r\nfile:///tmp/two%20words.png\r\n"]);
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped), (vec![DropEffect::Moved], None));
        assert!(host.conn.dragged_out_as(host.conn.atom(FILES)).is_none(), "nothing is dragged any more");
        // Dragged out again and back over its own window: the other program gets a leave;
        // dropped on the box, it is the number again.
        goes(&hands, &mut host, from);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [from[0] + 10, from[1]]);
        goes(&hands, &mut host, [420, 150]);
        goes(&hands, &mut host, [550, 150]);
        goes(&hands, &mut host, on);
        assert!(host.out.as_ref().is_some_and(|out| out.home) && host.ui.pointer().is_some());
        presses(&hands, &mut host, false);
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped), (vec![DropEffect::Moved], Some(7)));
        assert!(host.out.is_none() && sent(&heard).contains(&"XdndLeave".to_string()));
        // And cancelled with Escape over the other program: it gets a leave, and the app hears
        // nothing was taken. (The button is still down: it comes up after.)
        app.borrow_mut().dropped = None;
        goes(&hands, &mut host, from);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [from[0] + 10, from[1]]);
        goes(&hands, &mut host, [420, 150]);
        goes(&hands, &mut host, [550, 150]);
        let escape = unsafe { (codes.XKeysymToKeycode)(host.conn.display, 0xff1b) } as c_uint;
        unsafe { ((keys.XTestFakeKeyEvent)(host.conn.display, escape, 1, 0), (keys.XTestFakeKeyEvent)(host.conn.display, escape, 0, 0)) };
        lets(&mut host, 100);
        assert!(host.out.is_none() && host.ui.drag_payload().is_none());
        assert!(sent(&heard).contains(&"XdndLeave".to_string()));
        presses(&hands, &mut host, false);
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped), (vec![DropEffect::Moved], None));
        say(&host.conn, target, c"TAUT_STOP", [0; 5]);
    }

    /// Text dragged from another program and dropped on a window reaches the box under it, and
    /// the source hears it was taken; dropped beside the box, it hears it was not. (One XDND
    /// selection per display: only when asked, on a display of its own.)
    #[test]
    #[ignore]
    fn text_from_another_program_reaches_the_box_it_is_let_go_on() {
        let ((_alone, mut host), app) = drag_source_window(None, SecondWindow::default(), (50, 50)).expect("a display");
        settle(&mut host);
        let (conn, id) = (host.conn.clone(), host.windows[0].id);
        let (named, wait) = (|name: &CStr| conn.atom(name), Duration::from_secs(2));
        let on = app.borrow().on;
        for (place, taken) in [(on, true), (Rect { y: 180.0, h: 10.0, ..on }, false)] {
            let (from, heard) = files_dragged_from_elsewhere("words, ünïcode 𝄞");
            let at = spot(&host, 0, place);
            send(&mut host, id, message(id, named(c"XdndEnter"), [from as c_long, 5 << 24, named(c"UTF8_STRING") as c_long, named(TEXT) as c_long, 0]));
            send(&mut host, id, message(id, named(c"XdndPosition"), [from as c_long, 0, (at[0] << 16 | at[1]) as c_long, 0, named(c"XdndActionCopy") as c_long]));
            assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndStatus"), [id as c_long, 3, 0, 0, named(c"XdndActionCopy") as c_long])), "it may be let go here");
            assert!(host.ui.text_hovered() && !host.ui.files_hovered());
            send(&mut host, id, message(id, named(c"XdndDrop"), [from as c_long, 0, 0, 0, 0]));
            assert_eq!(heard.recv_timeout(wait), Ok((0, [id as c_long, 0, 0, 0, 0])), "the text was asked for");
            let finished = [id as c_long, taken as c_long, if taken { named(c"XdndActionCopy") as c_long } else { 0 }, 0, 0];
            assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndFinished"), finished)));
            assert_eq!(app.borrow_mut().text.take().as_deref(), if taken { Some("words, ünïcode 𝄞") } else { None });
        }
        // What is neither files nor text is not taken.
        let (from, heard) = files_dragged_from_elsewhere("");
        send(&mut host, id, message(id, named(c"XdndEnter"), [from as c_long, 5 << 24, named(PICTURE) as c_long, 0, 0]));
        let at = spot(&host, 0, app.borrow().on);
        send(&mut host, id, message(id, named(c"XdndPosition"), [from as c_long, 0, (at[0] << 16 | at[1]) as c_long, 0, named(c"XdndActionCopy") as c_long]));
        assert_eq!(heard.recv_timeout(wait), Ok((named(c"XdndStatus"), [id as c_long, 0, 0, 0, 0])));
        assert!(!host.ui.text_hovered() && !host.ui.files_hovered());
    }

    /// Text dragged from one window of a program to another: they talk XDND through X, as two
    /// programs do, and the data is read where it is at hand. (On a display of its own, with
    /// its pointer.)
    #[test]
    #[ignore = "works the pointer of a display of its own (xvfb-run)"]
    fn what_one_window_drags_out_another_window_of_the_program_takes() {
        let Some(hands) = hands() else { return eprintln!("no display of the tests' own to work the pointer of: not tested") };
        let ((_alone, mut host), app) = drag_source_window(Some(DragData::Text("über alles")), SecondWindow::new(200.0, 120.0).position(500, 100), (50, 50)).expect("a display");
        settle(&mut host);
        assert_eq!(host.windows.len(), 2);
        let (from, took) = (spot(&host, 0, app.borrow().from), spot(&host, 1, app.borrow().took));
        goes(&hands, &mut host, from);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [from[0] + 10, from[1]]);
        goes(&hands, &mut host, [420, 150]);
        goes(&hands, &mut host, took);
        assert!(host.out.as_ref().is_some_and(|out| out.over == host.windows[1].id && out.takes.is_some()));
        presses(&hands, &mut host, false);
        assert!(wait(&mut host, |host| host.out.is_none()));
        lets(&mut host, 50);
        assert_eq!((app.borrow().took_text.as_deref(), app.borrow().taken.clone()), (Some("über alles"), vec![DropEffect::Copied]));
    }

    /// What the program started by the test below does: a window at (500, 100) with a box
    /// taking files and text, and one handing out a file. It reports where both are, then what
    /// it got and what became of what it handed out, until it has both or 20 seconds pass.
    #[test]
    #[ignore = "run by the test below, in a program of its own"]
    fn a_program_that_takes_and_hands_out() {
        if std::env::var_os("TAUT_TEST_OTHER").is_none() {
            return;
        }
        let paths: &'static [PathBuf] = Box::leak(Box::new([PathBuf::from("/tmp/from the other.txt")]));
        let ((_alone, mut host), app) = drag_source_window(Some(DragData::Files(paths)), SecondWindow::default(), (500, 100)).expect("a display");
        settle(&mut host);
        let (on, from) = (spot(&host, 0, app.borrow().on), spot(&host, 0, app.borrow().from));
        println!("ready {} {} {} {}", on[0], on[1], from[0], from[1]);
        let (mut got, until) = ((false, false), Instant::now() + Duration::from_secs(20));
        while got != (true, true) && Instant::now() < until {
            lets(&mut host, 20);
            if let Some(text) = app.borrow_mut().text.take() {
                println!("got text: {text}");
                got.0 = true;
            }
            if let Some(taken) = app.borrow_mut().taken.pop() {
                println!("taken: {taken:?}");
                got.1 = true;
            }
        }
    }

    /// Two programs drag to each other by XDND: text from this one into the other's window, and
    /// a file from the other's window into this one. (The other is this test binary, run again
    /// for the test above; on a display of its own, with its pointer.)
    #[test]
    #[ignore = "works the pointer of a display of its own (xvfb-run)"]
    fn two_programs_drag_to_each_other() {
        let Some(hands) = hands() else { return eprintln!("no display of the tests' own to work the pointer of: not tested") };
        let ((_alone, mut host), app) = drag_source_window(Some(DragData::Text("from the first program")), SecondWindow::default(), (50, 50)).expect("a display");
        let mut other = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "x11::tests::a_program_that_takes_and_hands_out", "--ignored", "--nocapture", "--test-threads=1"])
            .env("TAUT_TEST_OTHER", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (tell, said) = std::sync::mpsc::channel();
        let out = other.stdout.take().unwrap();
        std::thread::spawn(move || std::io::BufRead::lines(std::io::BufReader::new(out)).map_while(Result::ok).for_each(|line| _ = tell.send(line)));
        // The other program's output starts on the line naming its test.
        let next = |word: &str| std::iter::from_fn(|| said.recv_timeout(Duration::from_secs(30)).ok()).find_map(|line| line.find(word).map(|at| line[at..].to_string()));
        let ready = next("ready ").expect("the other program is ready");
        let numbers: Vec<c_int> = ready.split(' ').skip(1).map(|number| number.parse().unwrap()).collect();
        settle(&mut host);
        // This program's text, dropped on the other's box.
        let from = spot(&host, 0, app.borrow().from);
        goes(&hands, &mut host, from);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [from[0] + 10, from[1]]);
        goes(&hands, &mut host, [420, 150]);
        goes(&hands, &mut host, [numbers[0], numbers[1]]);
        presses(&hands, &mut host, false);
        assert!(wait(&mut host, |host| host.out.is_none()));
        lets(&mut host, 50);
        assert_eq!(app.borrow().taken, [DropEffect::Copied]);
        // The other program's file, dropped on this one's box.
        let on = spot(&host, 0, app.borrow().on);
        goes(&hands, &mut host, [numbers[2], numbers[3]]);
        presses(&hands, &mut host, true);
        goes(&hands, &mut host, [numbers[2] + 10, numbers[3]]);
        goes(&hands, &mut host, [420, 150]);
        goes(&hands, &mut host, on);
        presses(&hands, &mut host, false);
        assert!(wait(&mut host, |_| !app.borrow().files.is_empty()));
        assert_eq!(app.borrow().files, [PathBuf::from("/tmp/from the other.txt")]);
        assert_eq!((next("got "), next("taken: ")), (Some("got text: from the first program".into()), Some("taken: Copied".into())));
        _ = other.wait();
    }

    #[test]
    fn a_letter_freetype_draws_no_picture_of_has_no_rows() {
        let bitmap = |rows, width, pitch, buffer: *mut u8, pixel_mode| Bitmap { rows, width, pitch, buffer, grays: 256, pixel_mode, palette_mode: 0, palette: null_mut() };
        let mut pixels = [1u8, 2, 9, 3, 4, 9];
        unsafe {
            assert_eq!(bitmap(2, 2, 3, pixels.as_mut_ptr(), 2).cover(), [1, 2, 3, 4]);
            // Zero rows, no rows, and a 1-bit bitmap.
            assert_eq!(bitmap(3, 0, 0, null_mut(), 2).cover(), []);
            assert_eq!(bitmap(3, 2, 3, null_mut(), 2).cover(), []);
            assert_eq!(bitmap(2, 2, 3, pixels.as_mut_ptr(), 1).cover(), []);
        }
    }

    /// The wake pipe is one per program and stays: a later window uses it too, and no window
    /// leaves descriptors open. An undrained wake covers all after it, so waking never blocks,
    /// window or not. (On a display of its own.)
    #[test]
    #[ignore = "needs a display of its own (xvfb-run)"]
    fn waking_never_waits_and_one_pipe_wakes_every_window() {
        let _alone = alone();
        // The program's open descriptors with their targets: other tests open and close their
        // own meanwhile, so only what appears and stays counts.
        let open = || -> Vec<(String, std::path::PathBuf)> {
            let fds = std::fs::read_dir("/proc/self/fd").map(|fds| fds.flatten().collect::<Vec<_>>()).unwrap_or_default();
            fds.iter().map(|fd| (fd.file_name().to_string_lossy().into_owned(), std::fs::read_link(fd.path()).unwrap_or_default())).collect()
        };
        crate::run(Window { title: "taut woken", width: 120.0, height: 80.0, ..Default::default() }, |ui| ui.quit());
        let (pipe, files) = (PIPE.get().copied(), open());
        assert!(pipe.is_some_and(|[read, write]| read >= 0 && write >= 0));
        // A window that quits once another thread wakes it (else after five seconds by itself).
        let (woken, since) = (Arc::new(AtomicBool::new(false)), Instant::now());
        let seen = woken.clone();
        let waker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            woken.store(true, Relaxed);
            Waker.wake();
        });
        crate::run(Window { title: "taut woken", width: 120.0, height: 80.0, ..Default::default() }, move |ui| match seen.load(Relaxed) || since.elapsed() > Duration::from_secs(5) {
            true => ui.quit(),
            false => ui.wake_after(Duration::from_secs(5)),
        });
        _ = waker.join();
        assert!(since.elapsed() < Duration::from_secs(4), "the window was woken");
        assert_eq!(PIPE.get().copied(), pipe, "the same pipe");
        let left = || open().into_iter().filter(|file| !files.contains(file)).collect::<Vec<_>>();
        let waited = Instant::now();
        while !left().is_empty() && waited.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(left(), [], "nothing left open");
        // With no window, a thread wakes as often as it likes.
        let (done, wakes) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            (0..100_000).for_each(|_| Waker.wake());
            _ = done.send(());
        });
        assert!(wakes.recv_timeout(Duration::from_secs(10)).is_ok(), "a wake waited");
    }

    /// X's error handler is the program's own again once the last window connection closes;
    /// while one is open, errors are shrugged off. (In a process of its own: other tests open
    /// connections meanwhile.)
    #[test]
    fn xs_way_with_failures_is_put_back_once_the_windows_are_gone() {
        let ran = Command::new(std::env::current_exe().unwrap()).args(["--exact", "x11::tests::xs_way_with_failures_alone", "--ignored", "--nocapture"]).env("TAUT_TEST_ALONE", "1").output().unwrap();
        let said = String::from_utf8_lossy(&ran.stdout);
        assert!(ran.status.success() && said.contains("1 passed"), "{said}\n{}", String::from_utf8_lossy(&ran.stderr));
    }

    unsafe extern "C" fn own_way(_: Display, _: *mut c_void) -> c_int {
        0
    }

    #[test]
    #[ignore = "run by the test before it, in a program of its own"]
    fn xs_way_with_failures_alone() {
        if std::env::var_os("TAUT_TEST_ALONE").is_none() {
            return;
        }
        let Some(x) = Xlib::open() else { return };
        let own: unsafe extern "C" fn(Display, *mut c_void) -> c_int = own_way;
        // The current handler is read by setting another and restoring it.
        let now = || unsafe {
            let was = (x.XSetErrorHandler)(None);
            (x.XSetErrorHandler)(was);
            was
        };
        unsafe { (x.XSetErrorHandler)(Some(own)) };
        let Some(one) = conn(c"TAUT_TEST_ONE") else { return eprintln!("no display: not tested") };
        assert!(shrugs(now()));
        let two = conn(c"TAUT_TEST_TWO").unwrap();
        drop(one);
        assert!(shrugs(now()), "one connection is still open");
        drop(two);
        assert!(now().is_some_and(|way| std::ptr::fn_addr_eq(way, own)));
    }

    /// The program's locale is as it was before its window opened: Xlib's input methods only
    /// read it at open. (On a display of its own.)
    #[test]
    #[ignore = "needs a display of its own (xvfb-run)"]
    fn the_programs_letters_are_set_as_they_were_once_its_window_is_open() {
        let before = program_locale();
        let Some((_alone, mut host)) = host(Window { title: "taut letters", width: 120.0, height: 80.0, ..Default::default() }, |ui: &mut Ui| _ = ui.label("Ünïcode")) else { return };
        lets(&mut host, 20);
        assert_eq!(program_locale(), before);
        assert!(!host.method.is_null() || std::env::var("LANG").is_err(), "an input method was opened");
    }

    /// The program's `LC_CTYPE`, read between the library's changes of it (other tests open
    /// windows meanwhile).
    fn program_locale() -> CString {
        let _turn = LOCALE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe { CStr::from_ptr(setlocale(0, null())).to_owned() }
    }

    /// Windows opening on many threads at once leave the program's locale as it was. (Each
    /// changes it for a moment; unheld, one put back another's passing locale for good.)
    #[test]
    fn the_locale_is_put_back_when_windows_open_on_many_threads_at_once() {
        let before = program_locale();
        let threads: Vec<_> = (0..8).map(|_| std::thread::spawn(|| (0..500).for_each(|_| in_locale(c"C.UTF-8", || std::hint::black_box(()))))).collect();
        threads.into_iter().for_each(|thread| thread.join().unwrap());
        assert_eq!(program_locale(), before);
        let passing = in_locale(c"C.UTF-8", || unsafe { CStr::from_ptr(setlocale(0, null())).to_owned() });
        assert_eq!((passing.as_c_str(), program_locale()), (c"C.UTF-8", before), "set for the moment, then put back");
    }

    /// `XWindowAttributes`: only `map_state` is read (0 unmapped, 2 viewable).
    #[repr(C)]
    struct Standing {
        numbers: [c_int; 6],
        visual: *mut c_void,
        root: XWindow,
        more: [c_int; 4],
        planes: [c_ulong; 2],
        save_under: c_int,
        colormap: c_ulong,
        installed: c_int,
        shown: c_int,
        rest: [c_long; 3],
        override_redirect: c_int,
        screen: *mut c_void,
    }
    library!(Looks, ["libX11.so.6", "libX11.so"], {
        fn XGetWindowAttributes(Display, XWindow, *mut Standing) -> c_int;
    });

    /// Whether X shows a window now (mapped, and every ancestor mapped).
    fn shown_at_x(conn: &Connection, window: XWindow) -> bool {
        let mut standing: Standing = unsafe { zeroed() };
        unsafe { (Looks::open().unwrap().XGetWindowAttributes)(conn.display, window, &mut standing) };
        standing.shown == 2
    }

    /// Another program on the display, closed when the test ends, however it ends.
    struct Other {
        x: Xlib,
        display: Display,
    }
    impl Drop for Other {
        fn drop(&mut self) {
            unsafe { (self.x.XCloseDisplay)(self.display) };
        }
    }

    /// Global hotkeys are grabbed and reach the app whatever has focus; one another program
    /// grabbed is refused, as is one X keeps; one the app no longer wants is released. A window
    /// hidden for the tray is withdrawn, and shown again when its app asks or a later start
    /// comes. It grabs and presses keys: only on a display of its own (`xvfb-run`).
    #[test]
    #[ignore = "needs a display of its own (xvfb-run)"]
    fn on_a_display_of_its_own_keys_of_the_whole_system_reach_the_app_and_a_hidden_window_comes_back() {
        #[derive(Default)]
        struct App {
            wants: bool,
            pressed: u32,
            taken: [bool; 2],
            hide: bool,
            forward: bool,
            hidden: bool,
            starts: Vec<crate::Relaunch>,
        }
        let _starts = crate::single_instance::tests::starts_alone();
        // Another program has Ctrl+Shift+F11.
        let grabs = Grabs::open().expect("libX11");
        let x = Xlib::open().unwrap();
        let display = unsafe { (x.XOpenDisplay)(null()) };
        if display.is_null() {
            return eprintln!("no display: not tested");
        }
        let other = Other { x, display };
        let (f11, f12) = unsafe { ((grabs.XKeysymToKeycode)(display, 0xffc8) as c_int, (grabs.XKeysymToKeycode)(display, 0xffc9) as c_int) };
        unsafe { ((grabs.XGrabKey)(display, f11, 1 | 4, root(&other.x, display), 0, 1, 1), (grabs.XSync)(display, 0)) };

        // XTest opens before the window so it still exists when the window's display closes: X
        // calls into it then.
        let (keys, codes) = (Keys::open().expect("libXtst"), Codes::open().unwrap());
        let app = Rc::new(RefCell::new(App { wants: true, ..App::default() }));
        let described = app.clone();
        let Some((_alone, mut host)) = host(Window { title: "taut keys", width: 200.0, height: 100.0, ..Default::default() }, move |ui: &mut Ui| {
            let mut app = described.borrow_mut();
            if app.wants {
                app.pressed += ui.hotkey("Ctrl+Shift+F12") as u32;
                ui.hotkey("Ctrl+Shift+F11");
                ui.hotkey("Ctrl+Alt+F12");
                app.taken = [ui.hotkey_refused("Ctrl+Shift+F11"), ui.hotkey_refused("Ctrl+Alt+F12")];
            }
            if std::mem::take(&mut app.hide) {
                ui.hide_window();
            }
            if std::mem::take(&mut app.forward) {
                ui.bring_to_front();
            }
            app.hidden = ui.window_minimized();
            let starts = ui.relaunches();
            app.starts.extend(starts);
            ui.label("One");
        }) else {
            return eprintln!("no display: not tested");
        };
        lets(&mut host, 100);
        assert_eq!(app.borrow().taken, [true, true], "the key another program has is refused, and so is one that switches to a console");
        let window = host.windows[0].id;
        assert!(shown_at_x(&host.conn, window));

        // The person presses Ctrl+Shift+F12, on none of the program's windows.
        let ours = host.conn.display;
        let code = |keysym: c_ulong| unsafe { (codes.XKeysymToKeycode)(ours, keysym) } as c_uint;
        for (keysym, down) in [(0xffe3, 1), (0xffe1, 1), (0xffc9, 1), (0xffc9, 0), (0xffe1, 0), (0xffe3, 0)] {
            unsafe { (keys.XTestFakeKeyEvent)(ours, code(keysym), down, 0) };
        }
        lets(&mut host, 200);
        assert_eq!(app.borrow().pressed, 1, "pressed once, heard once");

        // Hidden for the tray, the window is withdrawn; asked back, it shows again.
        let stand = |host: &mut Windows, app: &Rc<RefCell<App>>, hide: bool| {
            let mut asks = app.borrow_mut();
            (asks.hide, asks.forward, host.rerun) = (hide, !hide, true);
            drop(asks);
            lets(host, 200);
        };
        stand(&mut host, &app, true);
        assert!(!shown_at_x(&host.conn, window) && app.borrow().hidden);
        stand(&mut host, &app, false);
        assert!(shown_at_x(&host.conn, window) && !app.borrow().hidden);

        // Hidden again, it comes back for a later start of the program, which the app hears.
        stand(&mut host, &app, true);
        let later = crate::Relaunch { args: vec!["notes.txt".into()], working_dir: "/home/someone".into() };
        crate::single_instance::came(crate::single_instance::Tokens { wayland: String::new(), x: "launcher-7_TIME1234".into() }, later.clone());
        host.rerun = true;
        lets(&mut host, 200);
        assert!(shown_at_x(&host.conn, window) && !app.borrow().hidden);
        assert_eq!(app.borrow().starts, [later]);

        // No longer wanted, the key is released: the other program can grab it now.
        app.borrow_mut().wants = false;
        host.rerun = true;
        lets(&mut host, 100);
        REFUSED.store(false, Relaxed);
        unsafe {
            let was = (other.x.XSetErrorHandler)(Some(refused));
            (grabs.XGrabKey)(display, f12, 1 | 4, root(&other.x, display), 0, 1, 1);
            (grabs.XSync)(display, 0);
            (other.x.XSetErrorHandler)(was);
        }
        assert!(!REFUSED.load(Relaxed), "Control, Shift and F12 were given back");
    }

    // ---- fingers, pens and pinches from the X server's test input devices ----
    //
    // These run on an X server whose devices tests drive (Xorg with the `inputtest` driver: see
    // `xinput::tests::Device`), started by the test scripts and named in `TAUT_TEST_INPUT`. On
    // any other display they test nothing.
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    /// One of the test X server's `inputtest` devices, driven over its socket. The test scripts
    /// start such a server and name its socket folder in `TAUT_TEST_INPUT`; on any other server
    /// there is none.
    struct Device(UnixStream);

    /// The device connections, each made once per test run: the driver accepts one connection
    /// per device, and none after it ends.
    static CONNECTED: std::sync::Mutex<Vec<(String, UnixStream)>> = std::sync::Mutex::new(Vec::new());

    /// The driver's message types: version, wait for the server to process everything, motion,
    /// proximity, button, key, touch, touchpad gesture.
    const VERSION: u32 = 0;
    const SYNC: u32 = 1;
    const MOVED: u32 = 2;
    const NEAR: u32 = 3;
    const BUTTON: u32 = 4;
    const KEY: u32 = 5;
    const FINGER: u32 = 6;
    const PINCH: u32 = 7;

    impl Device {
        /// The device of that name (`touch`, `pen`, `eraser`, `mouse`, `touchpad`, `keyboard`);
        /// nothing on a server without it.
        fn open(name: &str) -> Option<Device> {
            let folder = std::env::var_os("TAUT_TEST_INPUT")?;
            let mut connected = CONNECTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((_, stream)) = connected.iter().find(|(known, _)| known == name) {
                return Some(Device(stream.try_clone().unwrap()));
            }
            let stream = UnixStream::connect(std::path::Path::new(&folder).join(format!("{name}.sock"))).ok()?;
            // An unanswering driver fails the test instead of hanging it.
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut device = Device(stream);
            device.send(VERSION, &[1, 0, 1, 0]);
            let mut answer = [0; 12];
            device.0.read_exact(&mut answer).expect("the test driver answers");
            connected.push((name.into(), device.0.try_clone().unwrap()));
            Some(device)
        }

        /// Sends one message: its length and type, then the rest.
        fn send(&mut self, kind: u32, rest: &[u8]) {
            let mut bytes = ((8 + rest.len()) as u32).to_le_bytes().to_vec();
            bytes.extend(kind.to_le_bytes());
            bytes.extend(rest);
            self.0.write_all(&bytes).unwrap();
        }

        /// Waits until the server processed everything sent to the device: X has then sent the
        /// windows what it made of it, before answering later queries.
        fn sync(&mut self) {
            self.send(SYNC, &[]);
            let mut answer = [0; 8];
            self.0.read_exact(&mut answer).expect("the test driver has the server take in what it was told");
        }

        /// The device's axes take these values, absolute (or relative).
        fn moved(&mut self, absolute: bool, axes: &[(usize, f64)]) {
            self.send(MOVED, &[&u64::from(absolute).to_le_bytes()[..], &axes_of(axes)].concat());
        }

        /// A pen comes into or out of proximity, where its axes say.
        fn near(&mut self, near: bool, axes: &[(usize, f64)]) {
            self.send(NEAR, &[&u64::from(near).to_le_bytes()[..], &axes_of(axes)].concat());
        }

        /// A button goes down or up, at absolute axis values.
        fn button(&mut self, button: i32, down: bool, axes: &[(usize, f64)]) {
            let head = [1i32.to_le_bytes(), button.to_le_bytes(), i32::from(down).to_le_bytes(), [0; 4]].concat();
            self.send(BUTTON, &[&head[..], &axes_of(axes)].concat());
        }

        /// A key goes down or up, by keycode.
        fn key(&mut self, code: i32, down: bool) {
            self.send(KEY, &[code.to_le_bytes(), i32::from(down).to_le_bytes()].concat());
        }

        /// A finger lands (18), moves (19) or lifts (20), where its axes say.
        fn finger(&mut self, finger: u32, kind: c_int, axes: &[(usize, f64)]) {
            self.send(FINGER, &[&finger.to_le_bytes()[..], &(kind as u32).to_le_bytes(), &axes_of(axes)].concat());
        }

        /// A two-finger touchpad pinch begins (27), updates (28) or ends (29), scaling what it
        /// pinches by `scale`.
        fn pinch(&mut self, kind: c_int, scale: f64) {
            let head = [(kind as u16).to_le_bytes(), 2u16.to_le_bytes()].concat();
            let numbers: Vec<u8> = [0.0, 0.0, 0.0, 0.0, scale, 0.0].iter().flat_map(|number: &f64| number.to_le_bytes()).collect();
            self.send(PINCH, &[&head[..], &[0; 4], &numbers].concat());
        }
    }

    /// Axis values as the driver takes them: whether any also come unaccelerated (none), a bit
    /// per axis given, and a value for each of 64 axes, twice.
    fn axes_of(axes: &[(usize, f64)]) -> Vec<u8> {
        let (mut bits, mut values) = ([0u8; 8], [0.0f64; 64]);
        for (axis, value) in axes {
            bits[axis / 8] |= 1 << (axis % 8);
            values[*axis] = *value;
        }
        let values: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        [&[0; 4][..], &bits, &[0; 4], &values, &[0; 512]].concat()
    }

    /// Each message to the test driver is as long as its header says: motion, button, key,
    /// touch, pinch.
    #[test]
    fn what_the_test_driver_is_told_is_as_long_as_it_expects() {
        assert_eq!(axes_of(&[(4, 1.0)]).len(), 1040);
        let (told, mut heard) = UnixStream::pair().unwrap();
        let mut device = Device(told);
        device.moved(true, &[(0, 1.0)]);
        device.near(true, &[]);
        device.button(1, true, &[]);
        device.key(37, true);
        device.finger(1, 18, &[]);
        device.pinch(27, 1.0);
        device.0.shutdown(std::net::Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        heard.read_to_end(&mut bytes).unwrap();
        let mut lengths = Vec::new();
        while bytes.len() >= 8 {
            let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            lengths.push((length, u32::from_le_bytes(bytes[4..8].try_into().unwrap())));
            bytes.drain(..length.min(bytes.len()));
        }
        assert_eq!(lengths, [(1056, MOVED), (1056, NEAR), (1064, BUTTON), (16, KEY), (1056, FINGER), (64, PINCH)]);
    }

    /// The real-touch tests' app: a scrolling list (where each row is), a zoom box (its scale),
    /// a drawing box (what the pen reported each run it was down on it) and a button (how often
    /// clicked).
    struct Drawn {
        rows: Vec<Rect>,
        map: Rect,
        zoomed: f32,
        paper: Rect,
        strokes: Vec<crate::touch::Stylus>,
        button: Rect,
        clicks: u32,
    }

    fn drawn(app: &mut Drawn, ui: &mut Ui) {
        ui.row().gap(10.0).open();
        ui.column().key("list").w(crate::Length::Px(150.0)).h(crate::Length::Px(300.0)).scroll().gap(4.0).open();
        app.rows = (0..30).map(|row| ui.button(&format!("Row {row}")).rect).collect();
        ui.close();
        let map = ui.el().size(200.0, 200.0).fill(Color::rgb(0x224422)).sense().show();
        app.zoomed *= ui.zoom(&map).map_or(1.0, |zoom| zoom.factor);
        let paper = ui.el().size(200.0, 200.0).fill(Color::rgb(0xeeeeee)).sense().show();
        if paper.pressed {
            app.strokes.extend(ui.stylus());
        }
        let button = ui.button("Press");
        (app.map, app.paper, app.button, app.clicks) = (map.rect, paper.rect, button.rect, app.clicks + button.clicked as u32);
        ui.close();
    }

    /// That app's window on the test-input display, with the named devices; nothing on any
    /// other display.
    #[allow(clippy::type_complexity)]
    fn drawn_on<const N: usize>(devices: [&str; N]) -> Option<(Alone, Rc<RefCell<Drawn>>, [Device; N])> {
        let alone = alone();
        let devices = devices.map(Device::open);
        if devices.iter().any(Option::is_none) {
            return None;
        }
        let app = Rc::new(RefCell::new(Drawn { rows: Vec::new(), map: Rect::EMPTY, zoomed: 1.0, paper: Rect::EMPTY, strokes: Vec::new(), button: Rect::EMPTY, clicks: 0 }));
        let described = app.clone();
        let asked = Window { title: "taut drawn", position: Some((0, 0)), width: 700.0, height: 360.0, ..Default::default() };
        let (alone, host) = host_on(alone, Session::on(None), asked, move |ui| drawn(&mut described.borrow_mut(), ui))?;
        Some(((alone, host), app, devices.map(Option::unwrap)))
    }

    /// Where on a test device's axes (0 to 65,535 across the screen each way, as X counts) a
    /// place in the first window is: its pixel's middle.
    fn on_axes(host: &Windows, at: Point) -> [(usize, f64); 2] {
        let ([left, top], [wide, high], scale) = (host.conn.place(host.windows[0].id), screen_size(&host.conn.x, host.conn.display), host.scale);
        let axis = |window: f32, corner: c_int, across: c_uint| (f64::from(window * scale) + f64::from(corner) + 0.5) * 65536.0 / f64::from(across);
        [(0, axis(at.x, left, wide)), (1, axis(at.y, top, high))]
    }

    /// The device delivered all it was told, and the window took in what X made of it.
    fn taken(host: &mut Windows, device: &mut Device) {
        device.sync();
        let conn = host.conn.clone();
        conn.property(conn.window, conn.mailbox, false);
        host.turn();
    }

    /// A box's middle, moved by so much.
    fn beside(of: Rect, by: [f32; 2]) -> Point {
        pt(of.x + of.w / 2.0 + by[0], of.y + of.h / 2.0 + by[1])
    }

    /// How far the list scrolled down, by a row fully visible before and after.
    #[cfg(feature = "touch")]
    fn scrolled(app: &Drawn, before: &[Rect]) -> f32 {
        before[5].y - app.rows[5].y
    }

    /// Two real touch fingers on the zoom box, spread from 60 to 120 pixels apart, double it;
    /// on the list and dragged up 100 pixels together, they scroll it 100 down and click
    /// nothing.
    #[cfg(feature = "touch")]
    #[test]
    fn two_real_fingers_pinch_what_zooms_and_scroll_a_list() {
        let Some(((_alone, mut host), app, [mut touch])) = drawn_on(["touch"]) else { return eprintln!("no test input devices: not tested") };
        let map = app.borrow().map;
        let fingers = |host: &Windows, touch: &mut Device, kind: c_int, places: [Point; 2]| {
            for (finger, at) in places.into_iter().enumerate() {
                touch.finger(finger as u32 + 1, kind, &on_axes(host, at));
            }
        };
        fingers(&host, &mut touch, 18, [beside(map, [-30.0, 0.0]), beside(map, [30.0, 0.0])]);
        taken(&mut host, &mut touch);
        for apart in [40.0, 50.0, 60.0] {
            fingers(&host, &mut touch, 19, [beside(map, [-apart, 0.0]), beside(map, [apart, 0.0])]);
            taken(&mut host, &mut touch);
        }
        fingers(&host, &mut touch, 20, [beside(map, [-60.0, 0.0]), beside(map, [60.0, 0.0])]);
        taken(&mut host, &mut touch);
        let zoomed = app.borrow().zoomed;
        assert!((zoomed - 2.0).abs() < 0.02, "zoomed {zoomed} times");

        let (before, list) = (app.borrow().rows.clone(), app.borrow().rows[4]);
        fingers(&host, &mut touch, 18, [beside(list, [-20.0, 0.0]), beside(list, [20.0, 0.0])]);
        taken(&mut host, &mut touch);
        for up in [20.0, 40.0, 60.0, 80.0, 100.0] {
            fingers(&host, &mut touch, 19, [beside(list, [-20.0, -up]), beside(list, [20.0, -up])]);
            taken(&mut host, &mut touch);
        }
        // Held still before lifting, they do not fling it further.
        std::thread::sleep(Duration::from_millis(150));
        host.rerun = true;
        host.turn();
        fingers(&host, &mut touch, 20, [beside(list, [-20.0, -100.0]), beside(list, [20.0, -100.0])]);
        taken(&mut host, &mut touch);
        settle(&mut host);
        let (by, clicks) = (scrolled(&app.borrow(), &before), app.borrow().clicks);
        assert!((by - 100.0).abs() < 1.5 && clicks == 0, "scrolled {by}, clicked {clicks}");
    }

    /// A real finger tapping the button clicks it once and reports the press as a finger's.
    #[cfg(feature = "touch")]
    #[test]
    fn a_real_finger_tapped_on_a_button_clicks_it() {
        let Some(((_alone, mut host), app, [mut touch])) = drawn_on(["touch"]) else { return eprintln!("no test input devices: not tested") };
        let on = on_axes(&host, beside(app.borrow().button, [0.0, 0.0]));
        for kind in [18, 20] {
            touch.finger(1, kind, &on);
            taken(&mut host, &mut touch);
        }
        settle(&mut host);
        assert_eq!((app.borrow().clicks, host.ui.is_touch()), (1, true));
    }

    /// A real pen drawn on the box with rising pressure: the app hears the pressure every run
    /// while the pen holds the box down; its other end erases. Once the mouse moves the
    /// pointer, no pen is near.
    #[cfg(feature = "touch")]
    #[test]
    fn a_real_pen_draws_with_the_pressure_it_presses_with_and_its_other_end_rubs_out() {
        let Some(((_alone, mut host), app, [mut pen, mut eraser, mut mouse])) = drawn_on(["pen", "eraser", "mouse"]) else {
            return eprintln!("no test input devices: not tested");
        };
        let paper = app.borrow().paper;
        let at = |host: &Windows, by: f32, pressure: f64| [on_axes(host, beside(paper, [by, by]))[0], on_axes(host, beside(paper, [by, by]))[1], (4, pressure)];
        pen.near(true, &at(&host, -40.0, 0.0));
        pen.moved(true, &at(&host, -40.0, 0.0));
        taken(&mut host, &mut pen);
        let hovering = host.ui.stylus();
        assert!(hovering.is_some_and(|pen| !pen.touching && pen.pressure == 0.0 && !pen.eraser), "{hovering:?}");
        pen.button(1, true, &at(&host, -40.0, 200.0));
        taken(&mut host, &mut pen);
        for (by, pressure) in [(-30.0, 400.0), (-20.0, 600.0), (-10.0, 800.0)] {
            pen.moved(true, &at(&host, by, pressure));
            taken(&mut host, &mut pen);
        }
        pen.button(1, false, &at(&host, -10.0, 0.0));
        taken(&mut host, &mut pen);
        let mut pressures: Vec<i32> = app.borrow().strokes.iter().map(|stroke| (stroke.pressure * 100.0).round() as i32).collect();
        pressures.dedup();
        assert_eq!(pressures, [20, 40, 60, 80]);
        assert!(app.borrow().strokes.iter().all(|stroke| stroke.touching && !stroke.eraser));
        assert!(host.ui.stylus().is_some_and(|pen| !pen.touching && pen.pressure == 0.0), "lifted, it hovers: {:?}", host.ui.stylus());

        // The eraser, a device of its own.
        app.borrow_mut().strokes.clear();
        eraser.near(true, &at(&host, 20.0, 0.0));
        eraser.button(1, true, &at(&host, 20.0, 500.0));
        taken(&mut host, &mut eraser);
        eraser.button(1, false, &at(&host, 20.0, 0.0));
        taken(&mut host, &mut eraser);
        let strokes = app.borrow().strokes.clone();
        assert!(!strokes.is_empty() && strokes.iter().all(|stroke| stroke.eraser && stroke.pressure == 0.5), "{strokes:?}");

        // The mouse moves the pointer: the pen is gone.
        mouse.moved(false, &[(0, 5.0), (1, 5.0)]);
        taken(&mut host, &mut mouse);
        assert_eq!(host.ui.stylus(), None);
    }

    /// A real touchpad pinch with the pointer over the zoom box scales it as the pinch says.
    #[cfg(feature = "touch")]
    #[test]
    fn a_real_touchpads_pinch_zooms_what_is_under_the_pointer() {
        let Some(((_alone, mut host), app, [mut touchpad, mut pen])) = drawn_on(["touchpad", "pen"]) else { return eprintln!("no test input devices: not tested") };
        // The pen puts the pointer over the box: the touchpad only moves it relatively.
        let over = on_axes(&host, beside(app.borrow().map, [0.0, 0.0]));
        pen.moved(true, &over);
        taken(&mut host, &mut pen);
        for (kind, scale) in [(27, 1.0), (28, 1.5), (28, 2.5), (29, 2.5)] {
            touchpad.pinch(kind, scale);
            taken(&mut host, &mut touchpad);
        }
        let zoomed = app.borrow().zoomed;
        assert!((zoomed - 2.5).abs() < 0.01, "zoomed {zoomed} times");
    }

    /// With touch and pens through XInput 2, a real mouse works as before: it clicks the
    /// button, its wheel scrolls the list a notch, and with Control the wheel zooms.
    #[cfg(feature = "touch")]
    #[test]
    fn a_real_mouse_clicks_scrolls_and_zooms_as_it_did() {
        let Some(((_alone, mut host), app, [mut mouse, mut pen, mut keyboard])) = drawn_on(["mouse", "pen", "keyboard"]) else {
            return eprintln!("no test input devices: not tested");
        };
        let (button, list, map, before) = (app.borrow().button, app.borrow().rows[4], app.borrow().map, app.borrow().rows.clone());
        // The pen puts the pointer where wanted; the mouse moves it on a pixel.
        let to = |host: &mut Windows, pen: &mut Device, mouse: &mut Device, at: Rect| {
            pen.moved(true, &on_axes(host, beside(at, [0.0, 0.0])));
            taken(host, pen);
            mouse.moved(false, &[(0, 1.0), (1, 0.0)]);
            taken(host, mouse);
        };
        to(&mut host, &mut pen, &mut mouse, button);
        for down in [true, false] {
            mouse.button(1, down, &[]);
            taken(&mut host, &mut mouse);
        }
        settle(&mut host);
        assert_eq!((app.borrow().clicks, host.ui.is_touch(), host.ui.stylus()), (1, false, None));
        to(&mut host, &mut pen, &mut mouse, list);
        for down in [true, false] {
            mouse.button(5, down, &[]);
            taken(&mut host, &mut mouse);
        }
        settle(&mut host);
        assert_eq!(scrolled(&app.borrow(), &before), 48.0);
        to(&mut host, &mut pen, &mut mouse, map);
        // Control is keycode 37 on an evdev keyboard.
        keyboard.key(37, true);
        taken(&mut host, &mut keyboard);
        for down in [true, false] {
            mouse.button(4, down, &[]);
            taken(&mut host, &mut mouse);
        }
        keyboard.key(37, false);
        taken(&mut host, &mut keyboard);
        assert_eq!(app.borrow().zoomed, 1.25);
    }

    /// Without the feature `touch`, nothing asks XInput 2: a real finger is X's emulated mouse,
    /// clicking what it taps; two fingers pinch nothing; a pen is a mouse with no pressure.
    #[cfg(not(feature = "touch"))]
    #[test]
    fn without_the_feature_a_real_finger_is_the_mouse_x_makes_of_it() {
        let Some(((_alone, mut host), app, [mut touch, mut pen])) = drawn_on(["touch", "pen"]) else { return eprintln!("no test input devices: not tested") };
        let on = on_axes(&host, beside(app.borrow().button, [0.0, 0.0]));
        for kind in [18, 20] {
            touch.finger(1, kind, &on);
            taken(&mut host, &mut touch);
        }
        settle(&mut host);
        assert_eq!((app.borrow().clicks, host.ui.is_touch()), (1, false));
        let map = app.borrow().map;
        for (kind, apart) in [(18, 30.0), (19, 60.0), (20, 60.0)] {
            touch.finger(1, kind, &on_axes(&host, beside(map, [-apart, 0.0])));
            touch.finger(2, kind, &on_axes(&host, beside(map, [apart, 0.0])));
            taken(&mut host, &mut touch);
        }
        assert_eq!(app.borrow().zoomed, 1.0);
        let paper = app.borrow().paper;
        let at = on_axes(&host, beside(paper, [0.0, 0.0]));
        pen.button(1, true, &[at[0], at[1], (4, 700.0)]);
        taken(&mut host, &mut pen);
        pen.button(1, false, &[at[0], at[1], (4, 0.0)]);
        taken(&mut host, &mut pen);
        assert_eq!((app.borrow().strokes.len(), host.ui.stylus()), (0, None));
    }

    /// Windows shown through the GPU (feature `gpu`), by OpenGL or Vulkan: the very pixels X
    /// shows, through every kind of change, a lost device, a missing renderer, and none at all. They read the screen, so they run only
    /// when asked, on a display of their own.
    #[cfg(feature = "gpu")]
    mod through_the_gpu {
        use super::*;
        use crate::platform::Renderer;

        /// The scene's state: two check boxes far apart, words, a text box; where the boxes are.
        #[derive(Default)]
        struct Scene {
            ticks: [bool; 2],
            words: String,
            typed: String,
            at: [Rect; 2],
        }

        fn scene(app: &mut Scene, ui: &mut Ui) {
            app.at[0] = ui.checkbox(&mut app.ticks[0], "Above").rect;
            ui.label(&app.words);
            ui.text_box(&mut app.typed, "Type here");
            ui.spacer();
            app.at[1] = ui.checkbox(&mut app.ticks[1], "Below").rect;
        }

        /// A window of the scene, shown as `renderer` asks.
        fn opened(renderer: Renderer) -> Option<(Alone, Rc<RefCell<Scene>>)> {
            let app = Rc::new(RefCell::new(Scene { words: "Words".into(), ..Default::default() }));
            let described = app.clone();
            let window = Window { title: "taut gpu", width: 320.0, height: 240.0, renderer, ..Default::default() };
            Some((host(window, move |ui| scene(&mut described.borrow_mut(), ui))?, app))
        }

        /// What a window's interface paints, drawn whole: what the window must show.
        fn painted(host: &mut Windows, at: usize) -> Vec<Vec<c_ulong>> {
            let ([width, height], scale) = (host.windows[at].size, host.scale);
            let mut pixels = vec![0; (width * height) as usize];
            let mut canvas = Canvas::new(&mut pixels, width as usize, height as usize, (0, 0), scale);
            host.windows[at].with(&mut host.ui, |ui| ui.paint(&mut canvas));
            pixels.chunks(width as usize).map(|row| row.iter().map(|pixel| (pixel & 0xff_ffff) as c_ulong).collect()).collect()
        }

        /// What a window shows, and what it must: once they match, or a second has passed.
        fn shown(host: &mut Windows, at: usize) -> (Vec<Vec<c_ulong>>, Vec<Vec<c_ulong>>) {
            let (must, conn, asked) = (painted(host, at), host.conn.clone(), Instant::now());
            let (id, size) = (host.windows[at].id, host.windows[at].size);
            let mut seen = on_screen(&conn, id, size);
            while seen != must && asked.elapsed() < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(10));
                seen = on_screen(&conn, id, size);
            }
            (seen, must)
        }

        /// How many pixels differ between two pictures of one size; the first that does, its
        /// place and both values; and the area around all that do (left, top, right, bottom).
        type Differ = (usize, Option<(usize, usize, c_ulong, c_ulong)>, Option<[usize; 4]>);

        /// See `Differ`.
        fn differ(seen: &[Vec<c_ulong>], must: &[Vec<c_ulong>]) -> Differ {
            let all = seen.iter().zip(must).enumerate().flat_map(|(y, (a, b))| a.iter().zip(b).enumerate().map(move |(x, (a, b))| (x, y, *a, *b)));
            let different: Vec<_> = all.filter(|(_, _, a, b)| a != b).collect();
            let around =
                different.iter().fold(None, |area: Option<[usize; 4]>, (x, y, ..)| Some(area.map_or([*x, *y, *x + 1, *y + 1], |[l, t, r, b]| [l.min(*x), t.min(*y), r.max(x + 1), b.max(y + 1)])));
            (different.len(), different.first().copied(), around)
        }

        /// Asserts that a window shows exactly what its interface paints.
        fn same(host: &mut Windows, at: usize, step: &str) -> Vec<Vec<c_ulong>> {
            let (seen, must) = shown(host, at);
            assert_eq!((seen.len(), seen.first().map(Vec::len)), (must.len(), must.first().map(Vec::len)), "{step}: the window's size");
            assert_eq!(differ(&seen, &must), (0, None, None), "{step}: pixels shown that differ from those painted (x, y, shown, painted)");
            seen
        }

        /// The steps the scene goes through: a change by its app, by X, or by the person. Says
        /// what the step was.
        fn step(host: &mut Windows, app: &Rc<RefCell<Scene>>, nth: usize) -> &'static str {
            let (conn, id, scale) = (host.conn.clone(), host.windows[0].id, host.scale);
            let said = match nth {
                0 => "opened",
                1 => {
                    app.borrow_mut().ticks[0] = true;
                    "the app ticks the box above"
                }
                2 => {
                    app.borrow_mut().ticks = [false, true];
                    "the app changes both boxes, far apart"
                }
                3 => {
                    app.borrow_mut().words = "Other words, and longer ones".into();
                    "the words change"
                }
                4 => {
                    let size = host.windows[0].size;
                    host.windows[0].exposed(ExposeEvent { kind: 12, serial: 0, sent: 0, display: null_mut(), window: id, x: 0, y: 0, width: size[0], height: size[1] }, scale);
                    "X has it drawn whole"
                }
                5 => {
                    let at = app.borrow().at[0];
                    click(host, id, middle(at, scale));
                    "the person clicks the box above"
                }
                6 => {
                    let asking = Asking::open().unwrap();
                    // SAFETY: the window is the host's own, on its display.
                    unsafe { (asking.XResizeWindow)(conn.display, id, 400, 300) };
                    assert!(wait(host, |host| host.windows[0].size == [400, 300]), "X resized the window");
                    "resized"
                }
                _ => {
                    app.borrow_mut().typed = "typed".into();
                    "text in the box"
                }
            };
            host.rerun = true;
            host.turn();
            said
        }

        /// Whether `TAUT_RENDERER` overrides what a window asks for.
        fn overridden() -> bool {
            std::env::var_os("TAUT_RENDERER").is_some()
        }

        /// The renderers a window can be shown through here.
        const GPUS: [Renderer; 2] = [Renderer::OpenGL, Renderer::Vulkan];

        /// A window of the scene shown through `renderer`, settled; nothing (said) where that
        /// renderer is missing.
        fn through(renderer: Renderer) -> Option<(Alone, Rc<RefCell<Scene>>)> {
            let ((alone, mut host), app) = opened(renderer).expect("a display");
            settle(&mut host);
            let by = host.gpu.as_ref().map(crate::gpu_x11::Gpu::renderer);
            if by != Some(renderer) {
                eprintln!("{renderer:?} missing (shown by {by:?}): not tested");
                return None;
            }
            eprintln!("{renderer:?}: shown through {}", host.gpu.as_ref().unwrap().name());
            Some(((alone, host), app))
        }

        /// Two small changes far apart go into the GPU's pixels, and are drawn and named to the
        /// driver, as two places, nothing between. (A driver presenting by damage, as DRI3 does,
        /// puts only those on the screen; Mesa's software one under Xvfb puts the area around
        /// them.)
        #[test]
        #[ignore]
        fn two_changes_far_apart_are_named_as_two_places() {
            for renderer in GPUS {
                let Some(((_alone, mut host), app)) = through(renderer) else { continue };
                app.borrow_mut().ticks = [true; 2];
                host.rerun = true;
                host.turn();
                let crate::gpu_x11::Way::Gpu(shows) = &host.windows[0].way else { panic!("{renderer:?}: shown by X") };
                let (at, scale) = (app.borrow().at, host.scale);
                let rows = |of: Rect| (of.y * scale).floor() as c_int..(of.bottom() * scale).ceil() as c_int;
                let named: Vec<[c_int; 4]> = crate::gpu_x11::Gpu::named(shows).to_vec();
                assert_eq!(named.len(), 2, "{renderer:?}: two places: {named:?} for {at:?}");
                let within = |place: &[c_int; 4], of: Rect| rows(of).contains(&place[1]) && rows(of).contains(&(place[3] - 1));
                assert!(named.iter().any(|place| within(place, at[0])) && named.iter().any(|place| within(place, at[1])), "{renderer:?}: {named:?} for {at:?}");
            }
        }

        /// Through either GPU renderer a window shows what it shows through X, byte for byte, at
        /// every step: and each is what its interface paints.
        #[test]
        #[ignore]
        fn the_gpu_shows_what_x_shows_byte_for_byte() {
            let mut pictures: [Vec<Vec<Vec<c_ulong>>>; 3] = Default::default();
            for (way, renderer) in [Renderer::Cpu, Renderer::OpenGL, Renderer::Vulkan].into_iter().enumerate() {
                let ((_alone, mut host), app) = opened(renderer).expect("a display");
                settle(&mut host);
                if !overridden() {
                    assert_eq!(host.ui.renderer(), renderer, "shown as asked");
                }
                for nth in 0..8 {
                    let said = step(&mut host, &app, nth);
                    pictures[way].push(same(&mut host, 0, &format!("{renderer:?}, {said}")));
                }
                if let Some(gpu) = &host.gpu {
                    eprintln!("{renderer:?}: shown through {}", gpu.name());
                }
            }
            for (renderer, shown) in GPUS.iter().zip(&pictures[1..]) {
                for (nth, (x, gpu)) in pictures[0].iter().zip(shown).enumerate() {
                    assert_eq!(differ(x, gpu), (0, None, None), "step {nth}: X and {renderer:?} show different pixels");
                }
            }
        }

        /// Where the GPU draws too (`gpu-draw`), it draws every kind of thing as the CPU does:
        /// forced to, as on a device that is the CPU at work. Says whether `host` has a GPU path.
        #[cfg(feature = "gpu-draw")]
        fn always_drawn(host: &mut Windows) -> bool {
            match host.gpu.as_mut() {
                Some(crate::gpu_x11::Gpu::OpenGL(gpu)) => gpu.drawing = crate::gpu_draw::Drawing::Always,
                Some(crate::gpu_x11::Gpu::Vulkan(gpu)) => gpu.drawing = crate::gpu_draw::Drawing::Always,
                None => return false,
            }
            true
        }

        /// Whether the GPU drew `host`'s windows itself.
        #[cfg(feature = "gpu-draw")]
        fn drawn_by_gpu(host: &Windows) -> bool {
            match host.gpu.as_ref() {
                Some(crate::gpu_x11::Gpu::OpenGL(gpu)) => gpu.has_drawn(),
                Some(crate::gpu_x11::Gpu::Vulkan(gpu)) => gpu.has_drawn(),
                None => false,
            }
        }

        /// Where the GPU draws too (`gpu-draw`), each GPU path's window shows what its interface
        /// paints at every step within a few levels (see `gpu_draw`), drawn by the GPU.
        #[cfg(feature = "gpu-draw")]
        #[test]
        #[ignore]
        fn the_gpu_draws_what_the_cpu_paints_within_a_few_levels() {
            for renderer in GPUS {
                let Some(((_alone, mut host), app)) = through(renderer) else { continue };
                assert!(always_drawn(&mut host));
                let off = |a: &c_ulong, b: &c_ulong| [16, 8, 0].map(|shift| (a >> shift & 255).abs_diff(b >> shift & 255)).into_iter().max().unwrap_or(0);
                for nth in 0..8 {
                    let said = step(&mut host, &app, nth);
                    let (seen, must) = shown(&mut host, 0);
                    let most = seen.iter().flatten().zip(must.iter().flatten()).map(|(a, b)| off(a, b)).max().unwrap_or(0);
                    assert!(most <= crate::gpu_draw::tests::TOLERANCE as c_ulong, "{renderer:?}, {said}: a pixel {most} levels off what was painted");
                }
                assert!(drawn_by_gpu(&host), "{renderer:?}: drawn by the GPU");
            }
        }

        /// Where the GPU draws too (`gpu-draw`), each GPU path draws every kind of thing as the
        /// CPU paints it, within a few levels: boxes, borders, a shadow, an icon, pictures, text,
        /// and what the CPU puts in patches.
        #[cfg(feature = "gpu-draw")]
        #[test]
        #[ignore]
        fn the_gpu_draws_every_kind_of_thing_as_the_cpu_paints_it() {
            for renderer in GPUS {
                let mut pictures = None;
                let window = Window { title: "taut gpu", width: 360.0, height: 210.0, renderer, ..Default::default() };
                let (_alone, mut host) = host(window, move |ui| crate::gpu_draw::tests::every_kind(ui, &mut pictures)).expect("a display");
                settle(&mut host);
                if host.gpu.as_ref().map(crate::gpu_x11::Gpu::renderer) != Some(renderer) || !always_drawn(&mut host) {
                    eprintln!("{renderer:?} missing: not tested");
                    continue;
                }
                host.windows[0].uncovered = whole(host.windows[0].size, host.scale);
                host.draw(0);
                let (seen, must) = shown(&mut host, 0);
                let off = |a: &c_ulong, b: &c_ulong| [16, 8, 0].map(|shift| (a >> shift & 255).abs_diff(b >> shift & 255)).into_iter().max().unwrap_or(0);
                let most = seen.iter().flatten().zip(must.iter().flatten()).map(|(a, b)| off(a, b)).max().unwrap_or(0);
                let differ = seen.iter().flatten().zip(must.iter().flatten()).filter(|(a, b)| a != b).count();
                eprintln!("{renderer:?}: {differ} pixels differ, at most by {most}");
                assert!(most <= crate::gpu_draw::tests::TOLERANCE as c_ulong, "{renderer:?}: a pixel {most} levels off what was painted");
                assert!(drawn_by_gpu(&host), "{renderer:?}: drawn by the GPU");
            }
        }

        /// A lost device is made again, and the window drawn whole through it; lost with none to
        /// be had, X shows the window from then on.
        #[test]
        #[ignore]
        fn a_lost_device_is_made_again_else_x_shows_the_window() {
            for renderer in GPUS {
                let Some(((_alone, mut host), app)) = through(renderer) else { continue };
                host.gpu.as_mut().unwrap().lose();
                app.borrow_mut().ticks[0] = true;
                host.rerun = true;
                host.turn();
                assert!(host.gpu.as_ref().is_some_and(|gpu| !gpu.lost()), "{renderer:?}: a new device");
                assert_eq!(host.ui.renderer(), renderer);
                same(&mut host, 0, &format!("{renderer:?}, made again"));

                crate::gpu_x11::NO_DEVICE.set(true);
                host.gpu.as_mut().unwrap().lose();
                app.borrow_mut().ticks[1] = true;
                host.rerun = true;
                host.turn();
                crate::gpu_x11::NO_DEVICE.set(false);
                assert!(host.gpu.is_none() && host.asks.is_none(), "{renderer:?}: X shows it for good");
                assert_eq!(host.ui.renderer(), Renderer::Cpu);
                same(&mut host, 0, &format!("{renderer:?}, then by X"));
                for nth in [3, 5] {
                    let said = step(&mut host, &app, nth);
                    same(&mut host, 0, said);
                }
            }
        }

        /// A renderer asked for but missing falls back along the order (OpenGL, then Vulkan),
        /// at the start and after a lost device; Direct3D is missing here. `Ui::renderer` says
        /// which one shows the window, and it shows what it paints.
        #[test]
        #[ignore]
        fn a_missing_renderer_falls_back_along_the_order() {
            let (gl, vk) = (Renderer::OpenGL, Renderer::Vulkan);
            let cases = [(Renderer::Gpu, Some(gl), vk), (Renderer::Vulkan, Some(vk), gl), (Renderer::Direct3D11, None, gl), (Renderer::Direct3D12, Some(gl), vk), (gl, Some(gl), vk)];
            for (asked, missing, wanted) in cases {
                crate::gpu_x11::MISSING.set(missing);
                let ((_alone, mut host), app) = opened(asked).expect("a display");
                settle(&mut host);
                crate::gpu_x11::MISSING.set(None);
                if host.gpu.is_none() || overridden() {
                    eprintln!("{asked:?} without {missing:?}: no device, not tested");
                    continue;
                }
                assert_eq!(host.ui.renderer(), wanted, "{asked:?} without {missing:?}");
                same(&mut host, 0, &format!("{asked:?} without {missing:?}"));
                // Lost, and its own renderer gone: the next one in the order shows the window.
                let other = if wanted == gl { vk } else { gl };
                crate::gpu_x11::MISSING.set(Some(wanted));
                host.gpu.as_mut().unwrap().lose();
                app.borrow_mut().ticks[0] ^= true;
                host.rerun = true;
                host.turn();
                crate::gpu_x11::MISSING.set(None);
                assert_eq!(host.ui.renderer(), other, "{asked:?}: after {wanted:?} was lost and gone");
                same(&mut host, 0, &format!("{asked:?}, after {wanted:?} was lost"));
            }
        }

        /// With no driver at all for one renderer, or for both, the next in the order shows the
        /// windows of an app asking for the GPU, else X, from the start. (Each in a program of
        /// its own, started without EGL's drivers, Vulkan's, or either.)
        #[test]
        #[ignore]
        fn with_a_driver_missing_the_next_renderer_shows_the_windows() {
            let name = "x11::tests::through_the_gpu::with_a_driver_missing_the_next_renderer_shows_the_windows";
            let (none, egl, vulkan) = ("/nonexistent/none.json", "__EGL_VENDOR_LIBRARY_FILENAMES", ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"]);
            if let Ok(missing) = std::env::var("TAUT_TEST_MISSING") {
                let ((_alone, mut host), app) = opened(Renderer::Gpu).expect("a display");
                settle(&mut host);
                let wanted = match missing.as_str() {
                    "opengl" => Renderer::Vulkan,
                    "vulkan" => Renderer::OpenGL,
                    _ => Renderer::Cpu,
                };
                assert_eq!(host.ui.renderer(), wanted, "without {missing}");
                assert_eq!((host.gpu.is_none(), host.asks.is_none()), (wanted == Renderer::Cpu, wanted == Renderer::Cpu));
                for nth in 0..3 {
                    let said = step(&mut host, &app, nth);
                    same(&mut host, 0, said);
                }
                return;
            }
            // No other window of this program meanwhile: the child's opens where it does.
            let _alone = alone();
            for missing in ["opengl", "vulkan", "both"] {
                let mut child = Command::new(std::env::current_exe().unwrap());
                child.args(["--exact", name, "--ignored", "--nocapture"]).env("TAUT_TEST_MISSING", missing).env_remove("TAUT_RENDERER");
                if missing != "vulkan" {
                    child.env(egl, none);
                }
                if missing != "opengl" {
                    vulkan.iter().for_each(|variable| _ = child.env(variable, none));
                }
                let ran = child.output().unwrap();
                let said = String::from_utf8_lossy(&ran.stdout);
                assert!(ran.status.success() && said.contains("1 passed"), "without {missing}: {said}\n{}", String::from_utf8_lossy(&ran.stderr));
            }
        }

        /// `Auto` takes the first device in the order that shows frames directly: a hardware
        /// device, with DRI3 where its driver is Mesa's. (Under Xvfb, none: it has no DRI3, and
        /// Mesa copies every frame through X.)
        #[test]
        #[ignore]
        fn auto_takes_only_a_device_showing_frames_directly() {
            let mut direct = Vec::new();
            for renderer in GPUS {
                let Some(((alone, host), _)) = through(renderer) else { continue };
                // SAFETY: the host's display is open.
                direct.extend(host.gpu.as_ref().filter(|gpu| unsafe { gpu.direct(host.conn.display) }).map(crate::gpu_x11::Gpu::renderer));
                drop((host, alone));
            }
            let ((_alone, host), _) = opened(Renderer::Auto).expect("a display");
            let wanted = direct.first().copied().unwrap_or(Renderer::Cpu);
            if !overridden() {
                assert_eq!((host.ui.renderer(), host.gpu.is_some()), (wanted, wanted != Renderer::Cpu), "direct: {direct:?}");
            }
        }

        /// A second window is shown through the GPU too, and shows what it paints; closed, it
        /// lets go of its surface, and the first goes on.
        #[test]
        #[ignore]
        fn second_windows_are_shown_through_it_too() {
            for renderer in GPUS {
                let prefs = SecondWindow::new(200.0, 120.0);
                let app = Rc::new(RefCell::new(Two { prefs, dark: false, pressed: 0, typed: Default::default(), dropped: Default::default(), rename: None, at: [Rect::EMPTY; 4] }));
                let described = app.clone();
                let window = Window { title: "taut test", width: 320.0, height: 200.0, renderer, ..Default::default() };
                let (_alone, mut host) = host(window, move |ui| two(&mut described.borrow_mut(), ui)).expect("a display");
                settle(&mut host);
                if host.gpu.as_ref().map(crate::gpu_x11::Gpu::renderer) != Some(renderer) {
                    eprintln!("{renderer:?} missing: not tested");
                    continue;
                }
                assert!(host.windows.iter().all(|shown| matches!(shown.way, crate::gpu_x11::Way::Gpu(_))), "{renderer:?}: both through the GPU");
                same(&mut host, 1, &format!("{renderer:?}, the second window"));
                (app.borrow_mut().dark, host.rerun) = (true, true);
                host.turn();
                same(&mut host, 1, &format!("{renderer:?}, its box ticked"));
                (app.borrow_mut().prefs.open, host.rerun) = (false, true);
                host.turn();
                assert_eq!(host.windows.len(), 1);
                (app.borrow_mut().typed[0], host.rerun) = ("first".into(), true);
                host.turn();
                same(&mut host, 0, &format!("{renderer:?}, the first window, after the second closed"));
            }
        }

        /// This program's processor time so far: all its threads (the software device's too).
        fn processor() -> Duration {
            unsafe extern "C" {
                fn clock_gettime(clock: c_int, time: *mut [i64; 2]) -> c_int;
            }
            let mut time = [0; 2];
            // SAFETY: the clock fills in seconds and nanoseconds (`CLOCK_PROCESS_CPUTIME_ID` 2).
            unsafe { clock_gettime(2, &mut time) };
            Duration::new(time[0] as u64, time[1] as u32)
        }

        /// The X server's processor time so far, where `TAUT_XVFB_PID` names it: its user and
        /// system ticks (100 a second) from `/proc`.
        fn servers() -> Duration {
            let stat = std::env::var("TAUT_XVFB_PID").ok().and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()).unwrap_or_default();
            let fields: Vec<u64> = stat.rsplit(')').next().unwrap_or("").split_whitespace().filter_map(|field| field.parse().ok()).collect();
            Duration::from_millis(10 * (fields.get(10).unwrap_or(&0) + fields.get(11).unwrap_or(&0)))
        }

        /// This program's memory in use (resident), in kB.
        fn resident() -> u64 {
            let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            let line = status.lines().find(|line| line.starts_with("VmRSS:")).unwrap_or("");
            line.split_whitespace().nth(1).and_then(|kb| kb.parse().ok()).unwrap_or(0)
        }

        /// How long a frame takes through each renderer, at the size `TAUT_MEASURE` names (as
        /// "1200x800", the default), with the gallery: the start to the first frame shown, a
        /// whole frame, and a small change. Processor time of this program and of the X server,
        /// and memory. Prints a table row each. (`TAUT_MEASURE_ONLY=cpu|opengl|vulkan` measures one, in a
        /// program of its own, so neither pays for what the other set up.)
        #[test]
        #[ignore]
        fn how_long_frames_take_through_each_renderer() {
            let asked = std::env::var("TAUT_MEASURE").unwrap_or("1200x800".into());
            let [width, height] = [0, 1].map(|side| asked.split('x').nth(side).and_then(|side| side.parse::<f32>().ok()).unwrap_or(800.0));
            let only = std::env::var("TAUT_MEASURE_ONLY").unwrap_or_default();
            for renderer in [Renderer::Cpu, Renderer::OpenGL, Renderer::Vulkan].into_iter().filter(|renderer| only.is_empty() || format!("{renderer:?}").eq_ignore_ascii_case(&only)) {
                let (memory, time, started) = (resident(), processor(), Instant::now());
                let mut kept = super::super::gallery::app();
                let window = Window { title: "taut measure", width, height, renderer, ..Default::default() };
                let (_alone, mut host) = host(window, move |ui| super::super::gallery::describe(&mut kept, ui)).expect("a display");
                let start = (started.elapsed(), processor() - time);
                settle(&mut host);
                let (id, size, scale) = (host.windows[0].id, host.windows[0].size, host.scale);
                let mut frames = |area: [c_int; 4], count: u32| {
                    let (time, server, wall) = (processor(), servers(), Instant::now());
                    for _ in 0..count {
                        host.windows[0].exposed(ExposeEvent { kind: 12, serial: 0, sent: 0, display: null_mut(), window: id, x: area[0], y: area[1], width: area[2], height: area[3] }, scale);
                        host.turn();
                    }
                    // X has done all it was asked by the time it answers a later query.
                    host.conn.property(host.conn.window, host.conn.mailbox, false);
                    [processor() - time, servers() - server, wall.elapsed()].map(|spent| spent.as_secs_f64() * 1000.0 / count as f64)
                };
                let whole = frames([0, 0, size[0], size[1]], 30);
                let small = frames([10, 10, 120, 24], 200);
                let device = host.gpu.as_ref().map_or("X".into(), crate::gpu_x11::Gpu::name);
                let grown = resident().saturating_sub(memory) / 1024;
                let [ms, cpu] = [start.0, start.1].map(|spent| spent.as_secs_f64() * 1000.0);
                println!(
                    "| {:?} | {} | {}x{} | start {ms:.0} ms ({cpu:.0} ms CPU) | whole {:.2} + X {:.2} ms ({:.2} ms) | small {:.3} + X {:.3} ms ({:.3} ms) | +{grown} MB |",
                    host.ui.renderer(),
                    device,
                    size[0],
                    size[1],
                    whole[0],
                    whole[1],
                    whole[2],
                    small[0],
                    small[1],
                    small[2]
                );
            }
        }
    }
}

// The gallery example, measured through each renderer by the tests above.
#[cfg(all(test, feature = "gpu"))]
#[allow(dead_code, unused_attributes, unused_imports, clippy::all)]
#[path = "../examples/gallery.rs"]
mod gallery;
