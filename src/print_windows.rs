//! Printing on Windows: the system's print dialog (`PrintDlgExW`), then the pages sent to the
//! chosen printer through its spooler.
//!
//! The dialog is shown between two of the program's messages, from a timer on its own thread:
//! shown during a run, it would pump the window's messages while the run holds the interface.
//! It shows over the active window, else a window of the thread, else a hidden window made for
//! it (the dialog needs an owner).
//!
//! Pages go to the printer as GDI shapes and text: paths filled and stroked with pens of the
//! page's caps, joins and dashes, words as glyphs of the paper's own fonts (lent to GDI for the
//! job), pictures as pixels. What GDI can't draw (gradients, see-through colours, colour emoji)
//! is drawn by taut and blended in, where the printer blends; on a printer that can't, the page
//! goes as pixels, drawn as [`Paper::draw`] draws it, a band of rows at a time (a page at 600
//! dpi is some 35 million pixels).
//!
//! A page prints at its own size from the sheet's corner. One larger than the sheet goes on as
//! many sheets as it takes, unless the app asked for it to be shrunk to fit
//! ([`Paper::shrink_to_fit`]).
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::FontFile;
use crate::geom::{Point, Rect, Size, rect};
use crate::paper::{Drawn, Mark, PagePaint, Paper, Picture, Words};
use crate::path::Step;
use crate::pen::{Cap, Join};
use crate::print::{Pages, PrintOutcome, tell};
use crate::windows::sys::{BITMAPINFO, CreateCompatibleDC, CreateDIBSection, CreateWindowExW, DeleteDC, DeleteObject, DestroyWindow, IsWindowVisible, POINT, SelectObject};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

type Handle = isize;

/// `PRINTDLGEXW`, as Windows lays it out.
#[repr(C)]
pub(crate) struct Asked {
    pub size: u32,
    pub owner: Handle,
    pub dev_mode: Handle,
    pub dev_names: Handle,
    pub dc: Handle,
    pub flags: u32,
    pub flags2: u32,
    pub excluded: u32,
    pub ranges: u32,
    pub most_ranges: u32,
    pub range_list: *mut [u32; 2],
    pub least_page: u32,
    pub most_page: u32,
    pub copies: u32,
    pub instance: Handle,
    pub template: *const u16,
    pub callback: *mut std::ffi::c_void,
    pub page_count: u32,
    pub pages_list: *mut Handle,
    pub start_page: u32,
    pub result: u32,
}

/// `DOCINFOW`.
#[repr(C)]
struct Document {
    size: i32,
    name: *const u16,
    output: *const u16,
    kind: *const u16,
    flags: u32,
}

/// `XFORM`: a point `x`, `y` goes to `m11 x + m21 y + dx`, `m12 x + m22 y + dy`.
#[repr(C)]
struct Transform {
    m11: f32,
    m12: f32,
    m21: f32,
    m22: f32,
    dx: f32,
    dy: f32,
}

/// `LOGBRUSH`: a solid brush (style 0) of a colour.
#[repr(C)]
struct Brush {
    style: u32,
    color: u32,
    hatch: usize,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn PrintDlgExW(asked: *mut Asked) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn StartDocW(dc: Handle, document: *const Document) -> i32;
    fn StartPage(dc: Handle) -> i32;
    fn EndPage(dc: Handle) -> i32;
    fn EndDoc(dc: Handle) -> i32;
    fn AbortDoc(dc: Handle) -> i32;
    fn GetDeviceCaps(dc: Handle, what: i32) -> i32;
    fn StretchDIBits(dc: Handle, x: i32, y: i32, w: i32, h: i32, from_x: i32, from_y: i32, from_w: i32, from_h: i32, bits: *const u32, block: *const BITMAPINFO, usage: u32, how: u32) -> i32;
    fn SaveDC(dc: Handle) -> i32;
    fn RestoreDC(dc: Handle, saved: i32) -> i32;
    fn SetGraphicsMode(dc: Handle, mode: i32) -> i32;
    fn SetWorldTransform(dc: Handle, transform: *const Transform) -> i32;
    fn SetBkMode(dc: Handle, mode: i32) -> i32;
    fn SetPolyFillMode(dc: Handle, mode: i32) -> i32;
    fn SetTextAlign(dc: Handle, how: u32) -> u32;
    fn SetTextColor(dc: Handle, color: u32) -> u32;
    fn SetStretchBltMode(dc: Handle, mode: i32) -> i32;
    fn SetMiterLimit(dc: Handle, limit: f32, old: *mut f32) -> i32;
    fn BeginPath(dc: Handle) -> i32;
    fn EndPath(dc: Handle) -> i32;
    fn MoveToEx(dc: Handle, x: i32, y: i32, old: *mut POINT) -> i32;
    fn LineTo(dc: Handle, x: i32, y: i32) -> i32;
    fn PolyBezierTo(dc: Handle, points: *const POINT, count: u32) -> i32;
    fn CloseFigure(dc: Handle) -> i32;
    fn FillPath(dc: Handle) -> i32;
    fn StrokePath(dc: Handle) -> i32;
    fn SelectClipPath(dc: Handle, mode: i32) -> i32;
    fn CreateSolidBrush(color: u32) -> Handle;
    fn ExtCreatePen(style: u32, width: u32, brush: *const Brush, count: u32, dashes: *const u32) -> Handle;
    fn AddFontMemResourceEx(file: *const u8, size: u32, reserved: *const u8, fonts: *mut u32) -> Handle;
    fn RemoveFontMemResourceEx(fonts: Handle) -> i32;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        precision: u32,
        clip: u32,
        quality: u32,
        family: u32,
        face: *const u16,
    ) -> Handle;
    fn GetFontData(dc: Handle, table: u32, offset: u32, buffer: *mut u8, size: u32) -> u32;
    fn ExtTextOutW(dc: Handle, x: i32, y: i32, how: u32, clip: *const core::ffi::c_void, glyphs: *const u16, count: u32, apart: *const i32) -> i32;
    fn GdiAlphaBlend(dc: Handle, x: i32, y: i32, w: i32, h: i32, from: Handle, from_x: i32, from_y: i32, from_w: i32, from_h: i32, blend: u32) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetTimer(window: Handle, id: usize, after: u32, then: usize) -> usize;
    fn KillTimer(window: Handle, id: usize) -> i32;
    fn GetActiveWindow() -> Handle;
    fn EnumThreadWindows(thread: u32, each: unsafe extern "system" fn(Handle, isize) -> i32, with: isize) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalFree(memory: Handle) -> Handle;
    fn GetCurrentThreadId() -> u32;
}

/// Dialog flags: return the printer's device context (`PD_RETURNDC`), let the printer make the
/// copies (`PD_USEDEVMODECOPIESANDCOLLATE`), offer neither "selection" nor "current page"; and
/// the results: printed, or not.
const RETURN_DC: u32 = 0x100;
const PRINTER_COPIES: u32 = 0x4_0000;
const NO_SELECTION: u32 = 0x4;
const NO_CURRENT_PAGE: u32 = 0x80_0000;
const PAGE_NUMBERS: u32 = 0x2;
const GENERAL_PAGE: u32 = 0xFFFF_FFFF;
const PRINT: u32 = 1;
/// `GetDeviceCaps` indexes: the printable area in device pixels, dots per inch, the paper's
/// size and unprintable margin in device pixels, and how it blends.
const AREA_X: i32 = 8;
const AREA_Y: i32 = 10;
const DOTS_X: i32 = 88;
const DOTS_Y: i32 = 90;
const PAPER_W: i32 = 110;
const PAPER_H: i32 = 111;
const EDGE_X: i32 = 112;
const EDGE_Y: i32 = 113;
const BLENDS: i32 = 120;
/// `SB_PIXEL_ALPHA`: the device blends pixels by their own alpha.
const PIXEL_ALPHA: i32 = 2;
/// Rows of a page drawn at a time.
const BAND: usize = 256;
/// How many page ranges the person may give in the dialog.
const STRETCHES: usize = 16;
/// GDI takes whole numbers: page units go to it in sixteenths.
const FINER: f32 = 16.0;

/// What shows the dialog: Windows' own, or a stand-in in tests.
pub(crate) type Shows = unsafe extern "system" fn(*mut Asked) -> i32;

/// Pages to print, their title (UTF-16), and where the outcome goes.
type Asking = (Pages, Vec<u16>, Arc<Mutex<Option<PrintOutcome>>>);

thread_local! {
    /// Pages asked for and not printed yet.
    static ASKED: RefCell<Vec<Asking>> = const { RefCell::new(Vec::new()) };
    /// What shows the dialog on this thread.
    pub(crate) static SHOWS: Cell<Shows> = const { Cell::new(PrintDlgExW) };
}

/// Keeps the pages to print and has the thread's messages bring the dialog as soon as they are
/// taken (a zero-time timer on the thread).
pub(crate) fn ask(pages: Pages, title: &str, outcome: Arc<Mutex<Option<PrintOutcome>>>) {
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    ASKED.with_borrow_mut(|asked| asked.push((pages, title, outcome)));
    // SAFETY: a timer of this thread, which calls `shown` (a `TIMERPROC`) from its messages.
    unsafe { SetTimer(0, 0, 0, shown as *const () as usize) };
}

/// The timer: shows the dialog for each paper asked for, and prints it.
unsafe extern "system" fn shown(_: Handle, _: u32, timer: usize, _: u32) {
    // SAFETY: the timer is this thread's own and is done with.
    unsafe { KillTimer(0, timer) };
    while let Some((pages, title, outcome)) = ASKED.with_borrow_mut(Vec::pop) {
        let owner = Owner::find();
        tell(&outcome, print_with(pages, &title, owner.window, SHOWS.get()));
    }
}

/// The window the dialog shows over: the active one, else a visible one of this thread, else
/// one of its own, never shown (destroyed with this).
pub(crate) struct Owner {
    pub window: Handle,
    own: bool,
}

impl Owner {
    /// Finds or makes the owner.
    pub(crate) fn find() -> Owner {
        // SAFETY: asks which of this thread's windows is active.
        let active = unsafe { GetActiveWindow() };
        if active != 0 {
            return Owner { window: active, own: false };
        }
        let mut found: Handle = 0;
        // SAFETY: lists this thread's windows to a callback that writes into `found`, alive
        // throughout.
        unsafe { EnumThreadWindows(GetCurrentThreadId(), visible, &raw mut found as isize) };
        if found != 0 {
            return Owner { window: found, own: false };
        }
        let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
        // SAFETY: a popup window of the system's static class, without `WS_VISIBLE`: never shown.
        let made = unsafe { CreateWindowExW(0, class.as_ptr(), [0u16].as_ptr(), 0x8000_0000, 0, 0, 0, 0, 0, 0, 0, std::ptr::null_mut()) };
        Owner { window: made, own: made != 0 }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.own {
            // SAFETY: the window is this one's own.
            unsafe { DestroyWindow(self.window) };
        }
    }
}

/// `EnumThreadWindows`' callback: keeps the first visible window and stops.
unsafe extern "system" fn visible(window: Handle, found: isize) -> i32 {
    // SAFETY: `found` points at the handle `Owner::find` waits for.
    unsafe {
        if IsWindowVisible(window) == 0 {
            return 1;
        }
        *(found as *mut Handle) = window;
    }
    0
}

/// The dialog's arguments for `pages` pages over `owner`: room for the page ranges, all pages
/// to begin with.
pub(crate) fn dialog(owner: Handle, pages: usize, ranges: &mut [[u32; 2]; STRETCHES]) -> Asked {
    let pages = pages.clamp(1, u32::MAX as usize) as u32;
    ranges[0] = [1, pages];
    Asked {
        size: size_of::<Asked>() as u32,
        owner,
        dev_mode: 0,
        dev_names: 0,
        dc: 0,
        flags: RETURN_DC | PRINTER_COPIES | NO_SELECTION | NO_CURRENT_PAGE,
        flags2: 0,
        excluded: 0,
        ranges: 1,
        most_ranges: STRETCHES as u32,
        range_list: ranges.as_mut_ptr(),
        least_page: 1,
        most_page: pages,
        copies: 1,
        instance: 0,
        template: std::ptr::null(),
        callback: std::ptr::null_mut(),
        page_count: 0,
        pages_list: std::ptr::null_mut(),
        start_page: GENERAL_PAGE,
        result: 0,
    }
}

/// Shows the dialog with `shows` over `owner`, and prints the chosen pages if the person
/// printed: laid out for the printer's sheet, where they are laid out on asking.
pub(crate) fn print_with(mut pages: Pages, title: &[u16], owner: Handle, shows: Shows) -> PrintOutcome {
    let mut ranges = [[0u32; 2]; STRETCHES];
    let mut asked = dialog(owner, pages.count(), &mut ranges);
    // SAFETY: the arguments outlive the call; what the dialog hands back (the printer's
    // settings and device context) is freed below.
    let said = unsafe { shows(&mut asked) };
    let printed = match (said, asked.result) {
        (0, PRINT) if asked.dc != 0 => {
            // SAFETY: the device context the dialog handed back.
            let paper = pages.for_sheet(unsafe { sheet_of(asked.dc) });
            let all = asked.flags & PAGE_NUMBERS == 0;
            let chosen = |page: &usize| all || ranges[..asked.ranges.min(STRETCHES as u32) as usize].iter().any(|[from, to]| (*from..=*to).contains(&(*page as u32 + 1)));
            let pages: Vec<usize> = (0..paper.pages()).filter(chosen).collect();
            spool(asked.dc, &paper, title, &pages)
        }
        (0, _) => PrintOutcome::Closed,
        _ => PrintOutcome::Unable,
    };
    // SAFETY: what the dialog handed back is the program's to free, once.
    unsafe {
        if asked.dc != 0 {
            DeleteDC(asked.dc);
        }
        [asked.dev_mode, asked.dev_names].into_iter().filter(|held| *held != 0).for_each(|held| _ = GlobalFree(held));
    }
    printed
}

/// What a device is: its dots per unit across and down, its sheet's size and unprintable
/// margin in its pixels, and whether it blends pixels by their alpha.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Device {
    pub dots: [f32; 2],
    pub sheet: [f32; 2],
    pub edge: [f32; 2],
    pub blends: bool,
}

impl Device {
    /// A device context's. (A device that is no printer has no sheet: its printable area is.)
    ///
    /// # Safety
    /// `dc` is a live device context.
    pub(crate) unsafe fn of(dc: Handle) -> Device {
        // SAFETY: as the caller promises.
        let ask = |what: i32| unsafe { GetDeviceCaps(dc, what) } as f32;
        let dots = [ask(DOTS_X).max(1.0) / 96.0, ask(DOTS_Y).max(1.0) / 96.0];
        let sheet = [if ask(PAPER_W) > 0.0 { ask(PAPER_W) } else { ask(AREA_X) }, if ask(PAPER_H) > 0.0 { ask(PAPER_H) } else { ask(AREA_Y) }];
        Device { dots, sheet: sheet.map(|side| side.max(1.0)), edge: [ask(EDGE_X), ask(EDGE_Y)], blends: ask(BLENDS) as i32 & PIXEL_ALPHA != 0 }
    }
}

/// A printer's sheet in units.
///
/// # Safety
/// `dc` is a live device context.
pub(crate) unsafe fn sheet_of(dc: Handle) -> Option<Size> {
    // SAFETY: as the caller promises.
    let device = unsafe { Device::of(dc) };
    Some(Size { w: device.sheet[0] / device.dots[0], h: device.sheet[1] / device.dots[1] })
}

/// How a page of `size` units goes on a device's sheets: its scale (device pixels per unit,
/// across and down) and the corner of each sheet's part of it, in units. At its own size, on as
/// many sheets as it takes; or with `shrink`, one, scaled down to fit where larger.
pub(crate) fn placing(device: &Device, size: Size, shrink: bool) -> ([f32; 2], Vec<Point>) {
    let fits = (device.sheet[0] / (size.w * device.dots[0])).min(device.sheet[1] / (size.h * device.dots[1]));
    if shrink && fits < 1.0 && fits > 0.0 {
        return (device.dots.map(|dots| dots * fits), vec![Point::default()]);
    }
    let step = [device.sheet[0] / device.dots[0], device.sheet[1] / device.dots[1]];
    // (A sliver of a unit over is rounding, not another sheet.)
    let across = ((size.w - 0.5) / step[0]).ceil().clamp(1.0, 64.0) as usize;
    let down = ((size.h - 0.5) / step[1]).ceil().clamp(1.0, 64.0) as usize;
    let corners = (0..down).flat_map(|row| (0..across).map(move |column| Point { x: column as f32 * step[0], y: row as f32 * step[1] })).collect();
    (device.dots, corners)
}

/// Sends pages to a printer's device context as a document of this title, each page on as many
/// sheets as it takes.
pub(crate) fn spool(dc: Handle, paper: &Paper, title: &[u16], pages: &[usize]) -> PrintOutcome {
    let document = Document { size: size_of::<Document>() as i32, name: title.as_ptr(), output: std::ptr::null(), kind: std::ptr::null(), flags: 0 };
    // SAFETY: the device context is the printer's, and the title outlives the call.
    if unsafe { StartDocW(dc, &document) } <= 0 {
        return PrintOutcome::Unable;
    }
    // SAFETY: as above.
    let device = unsafe { Device::of(dc) };
    for page in pages {
        let (scale, corners) = placing(&device, paper.size(*page), paper.shrink_to_fit);
        for corner in corners {
            // SAFETY: a page of the document begun above, drawn and ended.
            let drawn = unsafe { StartPage(dc) > 0 && draw_page(dc, paper, *page, (scale, corner)) && EndPage(dc) > 0 };
            if !drawn {
                // SAFETY: the document begun above is abandoned.
                unsafe { AbortDoc(dc) };
                return PrintOutcome::Unable;
            }
        }
    }
    // SAFETY: the document begun above ends and goes to the printer.
    if unsafe { EndDoc(dc) } > 0 { PrintOutcome::Sent } else { PrintOutcome::Unable }
}

/// Draws a page's part from `corner` (units) on a device context, `scale` device pixels per
/// unit: as shapes and text, blending in what GDI can't draw where the device blends; else as
/// pixels. Says whether all of it was taken.
///
/// # Safety
/// `dc` is a live device context.
pub(crate) unsafe fn draw_page(dc: Handle, paper: &Paper, page: usize, (scale, corner): ([f32; 2], Point)) -> bool {
    // SAFETY: as the caller promises.
    let device = unsafe { Device::of(dc) };
    let origin = [-device.edge[0] - corner.x * scale[0], -device.edge[1] - corner.y * scale[1]];
    let blended = marks_of(paper, page).any(|mark| !plain(paper, mark));
    // SAFETY: as the caller promises.
    unsafe {
        match blended && !device.blends {
            true => draw_pixels(dc, paper, page, scale, origin),
            false => draw_vectors(dc, paper, page, scale, origin),
        }
    }
}

/// A page's marks, a field's as it looks in their place.
fn marks_of(paper: &Paper, page: usize) -> impl Iterator<Item = &Mark> {
    paper.pages.get(page).into_iter().flat_map(|sheet| &sheet.marks).flat_map(as_shown)
}

/// A mark as it shows: a field as its marks in the state it shows, any other as itself.
fn as_shown(mark: &Mark) -> Vec<&Mark> {
    match mark {
        Mark::Field(field) => field.looks[field.shown.min(field.looks.len() - 1)].iter().collect(),
        _ => vec![mark],
    }
}

/// Whether GDI draws a mark as taut does: a shape of one solid colour, opaque words in a font
/// without colour glyphs, an opaque picture; clips, and what draws nothing.
fn plain(paper: &Paper, mark: &Mark) -> bool {
    match mark {
        Mark::Shape(drawn) => matches!(drawn.paint, PagePaint::Color(color) if color.a() == 255) && drawn.pen.is_none_or(|pen| pen.offset == 0.0),
        Mark::Words(words) => words.color.a() == 255 && !(cfg!(feature = "emoji") && paper.files[words.font as usize].pictured()),
        Mark::Picture(at, _) => match &paper.pictures[*at as usize] {
            Picture::Pixels { pixels, .. } => pixels.iter().all(|pixel| pixel >> 24 == 255),
            Picture::Jpeg { .. } => true,
        },
        _ => true,
    }
}

/// A colour as GDI takes it: `0x00BBGGRR`.
fn colour_ref(color: Color) -> u32 {
    (color.0 & 0xff) << 16 | (color.0 & 0xff00) | (color.0 >> 16 & 0xff)
}

/// A page unit as GDI takes it, in sixteenths.
fn finer(value: f32) -> i32 {
    (value * FINER).round().clamp(-1e9, 1e9) as i32
}

/// Draws a page as GDI shapes, text and pictures, in page units scaled by a world transform;
/// what GDI can't draw, blended in.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn draw_vectors(dc: Handle, paper: &Paper, page: usize, scale: [f32; 2], origin: [f32; 2]) -> bool {
    let mut lent = Lent::default();
    // SAFETY: as the caller promises; the state saved here is restored at the end.
    let ok = unsafe {
        let saved = SaveDC(dc);
        SetGraphicsMode(dc, 2);
        let world = Transform { m11: scale[0] / FINER, m12: 0.0, m21: 0.0, m22: scale[1] / FINER, dx: origin[0], dy: origin[1] };
        let mut ok = SetWorldTransform(dc, &world) != 0;
        SetBkMode(dc, 1);
        SetPolyFillMode(dc, 2);
        SetTextAlign(dc, 24);
        SetStretchBltMode(dc, 4);
        for mark in marks_of(paper, page) {
            ok &= draw_mark(dc, paper, mark, &mut lent, (scale, origin));
        }
        RestoreDC(dc, saved);
        ok
    };
    lent.give_back();
    ok
}

/// Draws one mark with GDI (blending in what it can't draw). Says whether it was taken.
///
/// # Safety
/// `dc` is a live device context, in page units scaled by the world transform.
unsafe fn draw_mark(dc: Handle, paper: &Paper, mark: &Mark, lent: &mut Lent, place: ([f32; 2], [f32; 2])) -> bool {
    // SAFETY: as the caller promises.
    unsafe {
        match mark {
            _ if !plain(paper, mark) => blend(dc, paper, mark, place),
            Mark::Shape(drawn) => shape(dc, drawn),
            Mark::Words(words) => words_drawn(dc, paper, words, lent),
            Mark::Picture(at, r) => picture(dc, paper, *at, *r),
            Mark::Clip(r) => {
                SaveDC(dc);
                BeginPath(dc);
                rectangle(dc, *r);
                EndPath(dc);
                SelectClipPath(dc, 1) != 0
            }
            Mark::Unclip => RestoreDC(dc, -1) != 0,
            Mark::Link(..) | Mark::Field(_) => true,
        }
    }
}

/// A rectangle as a path's figure.
///
/// # Safety
/// `dc` is a live device context with a path begun.
unsafe fn rectangle(dc: Handle, r: Rect) {
    // SAFETY: as the caller promises.
    unsafe {
        MoveToEx(dc, finer(r.x), finer(r.y), std::ptr::null_mut());
        LineTo(dc, finer(r.right()), finer(r.y));
        LineTo(dc, finer(r.right()), finer(r.bottom()));
        LineTo(dc, finer(r.x), finer(r.bottom()));
        CloseFigure(dc);
    }
}

/// A path as GDI's current path (a quadratic as the equivalent cubic).
///
/// # Safety
/// `dc` is a live device context.
unsafe fn path(dc: Handle, path: &crate::path::Path) {
    let point = |p: Point| POINT { x: finer(p.x), y: finer(p.y) };
    let mut at = Point::default();
    // SAFETY: as the caller promises; each call's points outlive it.
    unsafe {
        BeginPath(dc);
        for step in path.steps() {
            match *step {
                Step::Move(p) => _ = MoveToEx(dc, finer(p.x), finer(p.y), std::ptr::null_mut()),
                Step::Line(p) => _ = LineTo(dc, finer(p.x), finer(p.y)),
                Step::Quad(c, p) => _ = PolyBezierTo(dc, cubic(at, c, p).map(point).as_ptr(), 3),
                Step::Cubic(a, b, p) => _ = PolyBezierTo(dc, [point(a), point(b), point(p)].as_ptr(), 3),
                Step::Close => _ = CloseFigure(dc),
            }
            at = match *step {
                Step::Move(p) | Step::Line(p) | Step::Quad(_, p) | Step::Cubic(_, _, p) => p,
                Step::Close => at,
            };
        }
        EndPath(dc);
    }
}

/// A quadratic curve from `at` as the equivalent cubic: its two control points and its end.
fn cubic(at: Point, c: Point, p: Point) -> [Point; 3] {
    let third = |from: Point| Point { x: from.x + (c.x - from.x) * 2.0 / 3.0, y: from.y + (c.y - from.y) * 2.0 / 3.0 };
    [third(at), third(p), p]
}

/// A shape of one colour: filled, or stroked with a pen of its width, caps, joins and dashes.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn shape(dc: Handle, drawn: &Drawn) -> bool {
    let PagePaint::Color(color) = drawn.paint else { return false };
    // SAFETY: as the caller promises; what is made is selected, used, and deleted.
    unsafe {
        path(dc, &drawn.path);
        let Some(pen) = drawn.pen else {
            let brush = CreateSolidBrush(colour_ref(color));
            let was = SelectObject(dc, brush);
            let filled = FillPath(dc) != 0;
            SelectObject(dc, was);
            DeleteObject(brush);
            return filled;
        };
        let cap = match pen.cap {
            Cap::Round => 0,
            Cap::Square => 0x100,
            Cap::Flat => 0x200,
        };
        let (join, limit) = match pen.join {
            Join::Round => (0, 10.0),
            Join::Bevel => (0x1000, 10.0),
            Join::Mitre(limit) => (0x2000, limit.max(1.0)),
        };
        let dashes: Vec<u32> = drawn.dashes.iter().map(|dash| finer(dash.max(0.0)).max(1) as u32).collect();
        let dashed = drawn.dashes.iter().sum::<f32>() > 0.0;
        let style = 0x1_0000 | cap | join | if dashed { 7 } else { 0 };
        let brush = Brush { style: 0, color: colour_ref(color), hatch: 0 };
        let made = ExtCreatePen(style, finer(pen.width.max(0.0)).max(1) as u32, &brush, if dashed { dashes.len() as u32 } else { 0 }, if dashed { dashes.as_ptr() } else { std::ptr::null() });
        SetMiterLimit(dc, limit, std::ptr::null_mut());
        let was = SelectObject(dc, made);
        let stroked = StrokePath(dc) != 0;
        SelectObject(dc, was);
        DeleteObject(made);
        stroked
    }
}

/// A picture over `r`, as pixels GDI stretches.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn picture(dc: Handle, paper: &Paper, at: u32, r: Rect) -> bool {
    let unpacked = match &paper.pictures[at as usize] {
        Picture::Pixels { width, pixels } => Some((*width, std::borrow::Cow::Borrowed(pixels.as_slice()))),
        Picture::Jpeg { file, .. } => paper.unpack.and_then(|unpack| unpack(file)).map(|(width, pixels)| (width, std::borrow::Cow::Owned(pixels))),
    };
    let Some((width, pixels)) = unpacked.filter(|(width, pixels)| *width > 0 && pixels.len() >= *width) else { return true };
    let height = pixels.len() / width;
    let block = BITMAPINFO { size: 40, width: width as i32, height: -(height as i32), planes: 1, bits: 32, ..Default::default() };
    let [x, y, right, bottom] = [r.x, r.y, r.right(), r.bottom()].map(finer);
    // SAFETY: as the caller promises; the pixels are as many as the header says.
    unsafe { StretchDIBits(dc, x, y, right - x, bottom - y, 0, 0, width as i32, height as i32, pixels.as_ptr(), &block, 0, 0x00CC_0020) != 0 }
}

/// Fonts lent to GDI for printing: each font file added once (by the paper's index), and each
/// font made of it by size; or none where GDI doesn't pick the very file.
#[derive(Default)]
struct Lent {
    files: Vec<(u32, Handle)>,
    made: Vec<((u32, u32), Option<Handle>)>,
}

impl Lent {
    /// The GDI font for a paper font at a size: the file lent to GDI, a font asked of it by its
    /// family, weight and slant, and checked to be that file (its `head` table, unique to each
    /// font). None where GDI picks another.
    ///
    /// # Safety
    /// `dc` is a live device context.
    unsafe fn font(&mut self, dc: Handle, paper: &Paper, at: u32, size: f32) -> Option<Handle> {
        let key = (at, size.to_bits());
        if let Some((_, made)) = self.made.iter().find(|(known, _)| *known == key) {
            return *made;
        }
        let file = &paper.files[at as usize];
        if !self.files.iter().any(|(known, _)| *known == at) {
            let bytes = file.bytes().0;
            let mut count = 0;
            // SAFETY: GDI copies the font's bytes.
            let held = unsafe { AddFontMemResourceEx(bytes.as_ptr(), bytes.len() as u32, std::ptr::null(), &mut count) };
            self.files.push((at, held));
        }
        // SAFETY: as the caller promises.
        let made = unsafe { asked_font(dc, file, size) };
        self.made.push((key, made));
        made
    }

    /// Takes the fonts back from GDI.
    fn give_back(&mut self) {
        // SAFETY: each was made or added here, once.
        unsafe {
            self.made.iter().filter_map(|(_, made)| *made).for_each(|made| _ = DeleteObject(made));
            self.files.iter().filter(|(_, held)| *held != 0).for_each(|(_, held)| _ = RemoveFontMemResourceEx(*held));
        }
        (self.made, self.files) = (Vec::new(), Vec::new());
    }
}

/// A GDI font of a font file's family, weight and slant at `size` units (in sixteenths): if GDI
/// picks that very file.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn asked_font(dc: Handle, file: &FontFile, size: f32) -> Option<Handle> {
    let (family, weight, italic) = family_of(file)?;
    let face: Vec<u16> = family.encode_utf16().take(31).chain([0]).collect();
    let head = crate::font_file::listed(file.bytes(), file.start, b"head")?;
    let own = file.bytes().0.get(head)?;
    // SAFETY: as the caller promises; the font is selected to be read, then put back.
    unsafe {
        let made = CreateFontW(-finer(size), 0, 0, 0, weight, italic as u32, 0, 0, 1, 7, 0, 0, 0, face.as_ptr());
        let was = SelectObject(dc, made);
        let mut theirs = vec![0u8; own.len()];
        let read = GetFontData(dc, u32::from_le_bytes(*b"head"), 0, theirs.as_mut_ptr(), theirs.len() as u32);
        SelectObject(dc, was);
        if read as usize == own.len() && theirs == own && file.varied.is_none() {
            return Some(made);
        }
        DeleteObject(made);
    }
    None
}

/// A font's family name (`name` id 1, from Windows' entries), weight (`OS/2`) and whether it is
/// italic.
fn family_of(file: &FontFile) -> Option<(String, i32, bool)> {
    let all = file.bytes();
    let names = all.within(&crate::font_file::listed(all, file.start, b"name")?);
    let os2 = crate::font_file::listed(all, file.start, b"OS/2").map(|table| all.within(&table));
    let (count, strings) = (names.u16(2)? as usize, names.u16(4)? as usize);
    let entry = (0..count).map(|n| 6 + 12 * n).find(|at| names.u16(*at) == Some(3) && names.u16(at + 6) == Some(1))?;
    let bytes = names.part(strings + names.u16(entry + 10)? as usize, names.u16(entry + 8)? as usize)?.0;
    let family = String::from_utf16_lossy(&bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect::<Vec<u16>>());
    let weight = os2.and_then(|os2| os2.u16(4)).unwrap_or(400) as i32;
    let italic = os2.and_then(|os2| os2.u16(62)).is_some_and(|selection| selection & 1 != 0);
    Some((family, weight, italic))
}

/// Words as GDI text: the paper's glyphs, each where the paper put it, in the paper's own font
/// lent to GDI; or where GDI picks another font, as their outlines.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn words_drawn(dc: Handle, paper: &Paper, words: &Words, lent: &mut Lent) -> bool {
    let Some(first) = words.glyphs.first() else { return true };
    // SAFETY: as the caller promises.
    let Some(font) = (unsafe { lent.font(dc, paper, words.font, words.size) }) else { return unsafe { outlines(dc, paper, words) } };
    let ids: Vec<u16> = words.glyphs.iter().map(|glyph| glyph.id).collect();
    // Each glyph's step to the next, across and down, in sixteenths (from each one's rounded
    // place, so none drifts).
    let places: Vec<[i32; 2]> = words.glyphs.iter().map(|glyph| [finer(glyph.at.x), finer(glyph.at.y)]).collect();
    let steps: Vec<i32> = (0..places.len()).flat_map(|at| places.get(at + 1).map_or([0, 0], |next| [next[0] - places[at][0], next[1] - places[at][1]])).collect();
    // SAFETY: as the caller promises; the glyphs and steps outlive the call.
    unsafe {
        let was = SelectObject(dc, font);
        SetTextColor(dc, colour_ref(words.color));
        let drawn = ExtTextOutW(dc, finer(first.at.x), finer(first.at.y), 0x10 | 0x2000, std::ptr::null(), ids.as_ptr(), ids.len() as u32, steps.as_ptr()) != 0;
        SelectObject(dc, was);
        drawn
    }
}

/// Words as their glyphs' outlines, filled.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn outlines(dc: Handle, paper: &Paper, words: &Words) -> bool {
    let file = &paper.files[words.font as usize];
    let unit = file.unit(words.size);
    let mut all = crate::path::Path::new();
    for glyph in &words.glyphs {
        all.add(&file.outline_through(glyph.id, [unit, 0.0, 0.0, -unit, glyph.at.x, glyph.at.y]).0);
    }
    let drawn = Drawn { path: all, pen: None, dashes: Vec::new(), paint: PagePaint::Color(words.color), full: None };
    // SAFETY: as the caller promises.
    unsafe { shape(dc, &drawn) }
}

/// What GDI can't draw, drawn by taut over its bounds and blended in by its alpha.
///
/// # Safety
/// `dc` is a live device context, in page units scaled by the world transform.
unsafe fn blend(dc: Handle, paper: &Paper, mark: &Mark, (scale, origin): ([f32; 2], [f32; 2])) -> bool {
    let around = match mark {
        Mark::Shape(drawn) => {
            let (r, grow) = (drawn.path.bounds(), drawn.pen.map_or(1.0, |pen| pen.width * 4.0));
            rect(r.x - grow, r.y - grow, r.w + 2.0 * grow, r.h + 2.0 * grow)
        }
        Mark::Words(words) => words.glyphs.iter().fold(Rect::EMPTY, |all, glyph| all.union(rect(glyph.at.x - words.size, glyph.at.y - words.size * 1.5, words.size * 3.0, words.size * 2.5))),
        Mark::Picture(_, r) => *r,
        _ => return true,
    };
    // (In device pixels, whole, within the sheet's reach.)
    let [left, top] = [(around.x * scale[0] + origin[0]).floor().max(-1e6), (around.y * scale[1] + origin[1]).floor().max(-1e6)];
    let [right, bottom] = [(around.right() * scale[0] + origin[0]).ceil().min(1e6), (around.bottom() * scale[1] + origin[1]).ceil().min(1e6)];
    let (width, height) = ((right - left).max(0.0) as usize, (bottom - top).max(0.0) as usize);
    if width == 0 || height == 0 || width * height > 1 << 26 {
        return true;
    }
    let block = BITMAPINFO { size: 40, width: width as i32, height: -(height as i32), planes: 1, bits: 32, ..Default::default() };
    let mut bits = std::ptr::null_mut();
    // SAFETY: a context and block of pixels of this function's own, freed after; the canvas
    // draws within the block's pixels.
    unsafe {
        let memory = CreateCompatibleDC(dc);
        let pixels = CreateDIBSection(memory, &block, 0, &mut bits, 0, 0);
        if pixels == 0 || bits.is_null() {
            DeleteDC(memory);
            return false;
        }
        let was = SelectObject(memory, pixels);
        let mut canvas = Canvas::new(std::slice::from_raw_parts_mut(bits, width * height), width, height, ((left - origin[0]) as i32, (top - origin[1]) as i32), scale[0]);
        match mark {
            Mark::Shape(drawn) => drawn.draw(&mut canvas),
            Mark::Words(words) => paper.draw_words(words, &mut canvas),
            Mark::Picture(at, r) => paper.draw_picture(*at, *r, &mut canvas),
            _ => {}
        }
        // (Back to page units: the world transform scales them.)
        let [x, y] = [(left - origin[0]) / scale[0], (top - origin[1]) / scale[1]];
        let [w, h] = [width as f32 / scale[0], height as f32 / scale[1]];
        let blended = GdiAlphaBlend(dc, finer(x), finer(y), finer(w), finer(h), memory, 0, 0, width as i32, height as i32, 0x01FF_0000) != 0;
        SelectObject(memory, was);
        DeleteObject(pixels);
        DeleteDC(memory);
        blended
    }
}

/// Draws a page as pixels, a band of rows at a time, as [`Paper::draw`] draws it.
///
/// # Safety
/// `dc` is a live device context.
unsafe fn draw_pixels(dc: Handle, paper: &Paper, page: usize, scale: [f32; 2], origin: [f32; 2]) -> bool {
    let size = paper.size(page);
    let (width, height) = ((size.w * scale[0]).round().max(1.0) as usize, (size.h * scale[1]).round().max(1.0) as usize);
    let mut band = vec![0u32; width * BAND];
    for top in (0..height).step_by(BAND) {
        let rows = BAND.min(height - top);
        band.fill(0xffff_ffff);
        paper.draw(page, &mut Canvas::new(&mut band[..width * rows], width, rows, (0, top as i32), scale[0]));
        let block = BITMAPINFO { size: 40, width: width as i32, height: -(rows as i32), planes: 1, bits: 32, ..Default::default() };
        // SAFETY: the band holds as many pixels as the header says.
        let laid = unsafe { StretchDIBits(dc, origin[0] as i32, top as i32 + origin[1] as i32, width as i32, rows as i32, 0, 0, width as i32, rows as i32, band.as_ptr(), &block, 0, 0x00CC_0020) };
        if laid == 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::{PrintOutcome, print, print_laid};
    use crate::text::{Face, Font};
    use crate::windows::sys::{DispatchMessageW, MSG};

    type Handle = isize;

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateEnhMetaFileW(reference: Handle, name: *const u16, bounds: *const [i32; 4], about: *const u16) -> Handle;
        fn CloseEnhMetaFile(dc: Handle) -> Handle;
        fn GetEnhMetaFileBits(file: Handle, size: u32, bytes: *mut u8) -> u32;
        fn PlayEnhMetaFile(dc: Handle, file: Handle, bounds: *const [i32; 4]) -> i32;
        fn DeleteEnhMetaFile(file: Handle) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn IsWindow(window: Handle) -> i32;
    }

    /// A page with shapes and a picture (no text, so installed fonts do not matter).
    fn page() -> Paper {
        let mut paper = Paper::new();
        paper.page(Size { w: 300.0, h: 200.0 });
        paper.rect(rect(10.0, 10.0, 120.0, 80.0), Color::rgb(0x2b6cb0));
        paper.rounded(rect(150.0, 20.0, 120.0, 160.0), 30.0, Color::rgb(0xe53e3e));
        let pixels: Vec<u32> = (0..16 * 16).map(|n| 0xff00_0000 | (n * 16) << 8).collect();
        paper.picture(16, &pixels, rect(20.0, 110.0, 64.0, 64.0));
        paper
    }

    /// A page of words, shapes and strokes: what a printer gets as GDI calls.
    fn drawn() -> Paper {
        let mut paper = page();
        if let Some(font) = crate::pdf::tests::latin() {
            paper.font(Face::Regular, font);
        }
        paper.text("Printed as text: AV To fi", 20.0, 186.0 - 24.0, Font::new(14.0), Color::BLACK);
        let mut wave = crate::path::Path::new();
        wave.move_to(150.0, 190.0).cubic_to(190.0, 150.0, 230.0, 230.0, 290.0, 190.0);
        paper.outline(&wave, crate::pen::Pen::new(3.0).cap(Cap::Flat).join(Join::Mitre(4.0)).dashes(&[8.0, 4.0], 0.0), Color::BLACK);
        paper
    }

    /// A memory device context and its pixels: the context, the pixels, and what to free.
    fn memory(width: usize, height: usize) -> (Handle, *mut u32, Handle) {
        let block = BITMAPINFO { size: 40, width: width as i32, height: -(height as i32), planes: 1, bits: 32, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        // SAFETY: a context and block of pixels of the test's own.
        unsafe {
            let dc = CreateCompatibleDC(0);
            let picture = CreateDIBSection(dc, &block, 0, &mut bits, 0, 0);
            SelectObject(dc, picture);
            (dc, bits, picture)
        }
    }

    /// The pixels a memory context of that size holds after `draw`, white first.
    fn pixels_of(width: usize, height: usize, draw: impl FnOnce(Handle)) -> Vec<u32> {
        let (dc, bits, picture) = memory(width, height);
        // SAFETY: the context is alive; its pixels are read once drawn, then freed.
        unsafe {
            std::slice::from_raw_parts_mut(bits, width * height).fill(0xffff_ffff);
            draw(dc);
            let got = std::slice::from_raw_parts(bits, width * height).iter().map(|pixel| pixel & 0xff_ffff).collect();
            DeleteDC(dc);
            DeleteObject(picture);
            got
        }
    }

    /// A page drawn into a metafile, as a printer is sent it: the kinds of its records, and the
    /// pixels it plays back as, at 96 dpi.
    fn recorded(paper: &Paper, page: usize, blends: Option<bool>) -> (Vec<u32>, Vec<u32>) {
        let size = paper.size(page);
        let (width, height) = (size.w as usize, size.h as usize);
        // SAFETY: a metafile of the test's own, read, played and deleted.
        unsafe {
            let dc = CreateEnhMetaFileW(0, std::ptr::null(), std::ptr::null(), std::ptr::null());
            let mut device = Device::of(dc);
            device.blends = blends.unwrap_or(device.blends);
            device.dots = [1.0; 2];
            let blended = marks_of(paper, page).any(|mark| !plain(paper, mark));
            let ok = match blended && !device.blends {
                true => draw_pixels(dc, paper, page, device.dots, [0.0; 2]),
                false => draw_vectors(dc, paper, page, device.dots, [0.0; 2]),
            };
            assert!(ok, "every mark was taken");
            let file = CloseEnhMetaFile(dc);
            let mut bytes = vec![0u8; GetEnhMetaFileBits(file, 0, std::ptr::null_mut()) as usize];
            GetEnhMetaFileBits(file, bytes.len() as u32, bytes.as_mut_ptr());
            let mut kinds = Vec::new();
            let mut at = 0;
            while at + 8 <= bytes.len() {
                let [kind, long] = [0, 4].map(|n| u32::from_le_bytes(bytes[at + n..at + n + 4].try_into().unwrap()));
                kinds.push(kind);
                at += (long as usize).max(8);
            }
            // (Played over the page's own size, as recorded.)
            let header: [i32; 4] = std::array::from_fn(|n| i32::from_le_bytes(bytes[8 + 4 * n..12 + 4 * n].try_into().unwrap()));
            let pixels = pixels_of(width, height, |into| _ = PlayEnhMetaFile(into, file, &[header[0], header[1], header[2] + 1, header[3] + 1]));
            DeleteEnhMetaFile(file);
            (kinds, pixels)
        }
    }

    /// How far apart two pictures are: the mean difference of their colours, 0 to 255.
    fn apart(a: &[u32], b: &[u32]) -> f32 {
        let d = |a: u32, b: u32| [16, 8, 0].map(|shift| ((a >> shift & 0xff) as i32 - (b >> shift & 0xff) as i32).unsigned_abs()).into_iter().max().unwrap_or(0);
        a.iter().zip(b).map(|(a, b)| d(*a, *b) as f32).sum::<f32>() / a.len() as f32
    }

    #[test]
    fn the_dialog_is_asked_for_every_page() {
        let mut ranges = [[0u32; 2]; 16];
        let asked = dialog(0x1234, 5, &mut ranges);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<Asked>(), 136, "laid out as commdlg.h lays out PRINTDLGEXW");
        assert_eq!((asked.size as usize, asked.owner, asked.least_page, asked.most_page, asked.copies), (size_of::<Asked>(), 0x1234, 1, 5, 1));
        assert_eq!((asked.ranges, asked.most_ranges, ranges[0], asked.start_page), (1, 16, [1, 5], 0xFFFF_FFFF));
        // A printer context comes back; the printer makes the copies; no "selection" and no
        // "current page".
        assert_eq!(asked.flags, 0x100 | 0x4_0000 | 0x4 | 0x80_0000);
        assert_eq!((size_of::<Transform>(), size_of::<Brush>()), (24, 16), "as wingdi.h lays out XFORM and LOGBRUSH");
    }

    #[test]
    fn a_page_goes_to_a_printer_as_shapes_and_text() {
        let paper = drawn();
        let (kinds, pixels) = recorded(&paper, 0, None);
        let has = |kind: u32| kinds.contains(&kind);
        assert!(has(62) && has(64) && has(88) | has(5), "filled and stroked paths, with curves: {kinds:?}");
        if crate::pdf::tests::latin().is_some() {
            assert!(has(84), "words as text: {kinds:?}");
        }
        assert_eq!(kinds.iter().filter(|kind| **kind == 81).count(), 1, "pixels only for the picture");
        let (_, own) = paper.render_page(0, 1.0);
        let own: Vec<u32> = own.iter().map(|pixel| pixel & 0xff_ffff).collect();
        let apart = apart(&pixels, &own);
        assert!(apart < 6.0, "GDI draws what taut draws, within its edges: {apart}");
    }

    #[test]
    fn what_gdi_cant_draw_is_blended_in_or_the_page_goes_as_pixels() {
        let mut paper = page();
        paper.rect(rect(60.0, 60.0, 120.0, 80.0), Color::rgb(0x00aa00).with_alpha(0.5));
        paper.gradient(rect(200.0, 150.0, 90.0, 40.0), 0.0, &[(0.0, Color::BLACK), (1.0, Color::WHITE)], Some(90.0));
        let (kinds, pixels) = recorded(&paper, 0, Some(true));
        assert_eq!(kinds.iter().filter(|kind| **kind == 114).count(), 2, "two blended in: {kinds:?}");
        let (_, own) = paper.render_page(0, 1.0);
        let own: Vec<u32> = own.iter().map(|pixel| pixel & 0xff_ffff).collect();
        assert!(apart(&pixels, &own) < 6.0);
        // A printer that can't blend gets pixels.
        let (kinds, pixels) = recorded(&paper, 0, Some(false));
        assert!(!kinds.contains(&62) && !kinds.contains(&114) && kinds.contains(&81), "{kinds:?}");
        let apart = apart(&pixels, &own);
        assert!(apart < 6.0, "{apart}");
    }

    #[test]
    fn a_page_larger_than_the_sheet_goes_on_several_or_shrinks_when_asked() {
        // A Letter sheet at 600 dpi.
        let device = Device { dots: [6.25; 2], sheet: [5100.0, 6600.0], edge: [0.0; 2], blends: true };
        let (scale, corners) = placing(&device, Paper::LETTER, false);
        assert_eq!((scale, corners.len()), ([6.25; 2], 1), "a page its size fits");
        let (_, corners) = placing(&device, Size { w: 816.0 * 2.0, h: 1056.0 * 1.5 }, false);
        let expected = [[0.0, 0.0], [816.0, 0.0], [0.0, 1056.0], [816.0, 1056.0]];
        assert_eq!(corners.iter().map(|corner| [corner.x, corner.y]).collect::<Vec<_>>(), expected, "at its size, on four sheets");
        let (scale, corners) = placing(&device, Size { w: 816.0 * 2.0, h: 1056.0 * 1.5 }, true);
        assert_eq!((scale, corners.len()), ([3.125; 2], 1), "shrunk to fit, when the app asks");
        let (_, corners) = placing(&device, Size { w: f32::INFINITY, h: 1e30 }, false);
        assert!(corners.len() <= 64 * 64);
    }

    #[test]
    fn a_page_of_endless_size_goes_to_a_printer() {
        let mut paper = Paper::new();
        paper.page(Size { w: f32::INFINITY, h: f32::INFINITY });
        paper.rect(rect(0.0, 0.0, 50.0, 50.0), Color::BLACK.with_alpha(0.5));
        // Printing draws in a timer on the windows' thread, where a panic ends the program.
        let drawn = std::panic::catch_unwind(|| {
            pixels_of(200, 200, |dc| {
                // SAFETY: the test's own memory context.
                unsafe { draw_page(dc, &paper, 0, ([0.01; 2], Point::default())) };
            })
        });
        assert!(drawn.is_ok(), "drawing the page panicked");
    }

    thread_local! {
        /// What the stand-in dialog was asked, and how it answers: its result, what the person
        /// did, and the device context it hands back. And the owners it was shown over.
        static SEEN: RefCell<Vec<(u32, u32, u32)>> = const { RefCell::new(Vec::new()) };
        static OWNERS: RefCell<Vec<(Handle, bool, bool)>> = const { RefCell::new(Vec::new()) };
        static ANSWER: RefCell<(i32, u32, Handle)> = const { RefCell::new((0, 0, 0)) };
    }

    /// The dialog as a test plays it: notes what it was asked, and answers as told.
    unsafe extern "system" fn stand_in(asked: *mut Asked) -> i32 {
        // SAFETY: the arguments the printing code made, alive while it waits; the owner is
        // only asked about.
        let asked = unsafe { &mut *asked };
        SEEN.with_borrow_mut(|seen| seen.push((asked.most_page, asked.flags, asked.start_page)));
        let owner = unsafe { (asked.owner, IsWindow(asked.owner) != 0, IsWindowVisible(asked.owner) != 0) };
        OWNERS.with_borrow_mut(|owners| owners.push(owner));
        let (said, result, dc) = ANSWER.with_borrow(|answer| *answer);
        (asked.result, asked.dc) = (result, dc);
        said
    }

    /// Takes the thread's messages until `done`, or five seconds pass.
    fn messages_until(done: impl Fn() -> bool) {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn PeekMessageW(msg: *mut MSG, hwnd: Handle, min: u32, max: u32, how: u32) -> i32;
        }
        let since = std::time::Instant::now();
        while !done() && since.elapsed() < std::time::Duration::from_secs(5) {
            let mut msg = MSG::default();
            // SAFETY: the thread's own messages, each dispatched.
            while unsafe { PeekMessageW(&mut msg, 0, 0, 0, 1) } != 0 {
                unsafe { DispatchMessageW(&msg) };
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn the_dialog_comes_between_messages_over_a_window_and_what_the_person_did_is_told() {
        // Never Windows' own dialog in a test: a stand-in for this thread.
        SHOWS.set(stand_in);
        ANSWER.set((0, 0, 0));
        let printing = print(page(), "A test");
        assert_eq!(printing.outcome(), None, "nothing is shown before the thread takes its messages");
        messages_until(|| printing.outcome().is_some());
        assert_eq!(printing.outcome(), Some(PrintOutcome::Closed), "the person closed the dialog");
        assert_eq!(SEEN.with_borrow(Clone::clone), [(1, 0x100 | 0x4_0000 | 0x4 | 0x80_0000, 0xFFFF_FFFF)]);
        // (This thread has no window, let alone an active one: the dialog had one of its own,
        // never shown, gone after.)
        let (owner, was, shown) = OWNERS.with_borrow(|owners| owners[0]);
        assert!(owner != 0 && was && !shown, "an owner, hidden");
        // SAFETY: only asked about.
        assert_eq!(unsafe { IsWindow(owner) }, 0, "and gone");
        // A dialog the system could not show: nothing printed.
        ANSWER.set((-2147024809, 0, 0));
        let printing = print(page(), "A test");
        messages_until(|| printing.outcome().is_some());
        assert_eq!(printing.outcome(), Some(PrintOutcome::Unable));
    }

    #[test]
    fn pages_are_laid_out_for_the_printers_sheet() {
        SHOWS.set(stand_in);
        // The stand-in hands back a metafile context: the printer's sheet is its area. (It
        // can't start a document, so nothing is sent.)
        // SAFETY: a context of the test's own, freed by the printing code as a printer's is.
        let dc = unsafe { CreateEnhMetaFileW(0, std::ptr::null(), std::ptr::null(), std::ptr::null()) };
        // SAFETY: as above.
        let expected = unsafe { sheet_of(dc) };
        ANSWER.set((0, PRINT, dc));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let heard = asked.clone();
        let printing = print_laid("Laid", move |sheet| {
            heard.lock().unwrap().push(sheet);
            let mut paper = Paper::new();
            paper.page(sheet);
            paper
        });
        messages_until(|| printing.outcome().is_some());
        assert_eq!(printing.outcome(), Some(PrintOutcome::Unable), "a metafile is no printer");
        assert_eq!(*asked.lock().unwrap(), [Paper::A4, expected.unwrap()], "counted on A4, laid out for the sheet");
    }
}
