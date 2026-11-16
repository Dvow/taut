//! Printing on Windows: the system's print dialog (`PrintDlgExW`) over the window that asked,
//! then the pages sent to the chosen printer through its spooler.
//!
//! The dialog is shown between two of the window's messages, from a timer on the program's own
//! thread: shown during a run, it would pump the window's messages while the run holds the
//! interface. Pages go to the printer as pixels, drawn as [`Paper::draw`] draws them at the
//! printer's resolution, a band of rows at a time (a page at 600 dpi is some 35 million
//! pixels).
use crate::canvas::Canvas;
use crate::paper::Paper;
use crate::print::{Printed, tell};
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

/// A block of pixels for GDI: 4 bytes a pixel, rows from the top.
#[repr(C)]
struct Block {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bits: u16,
    packing: u32,
    bytes: u32,
    across: i32,
    down: i32,
    used: u32,
    important: u32,
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
    fn DeleteDC(dc: Handle) -> i32;
    fn GetDeviceCaps(dc: Handle, what: i32) -> i32;
    fn StretchDIBits(dc: Handle, x: i32, y: i32, w: i32, h: i32, from_x: i32, from_y: i32, from_w: i32, from_h: i32, bits: *const u32, block: *const Block, usage: u32, how: u32) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetTimer(window: Handle, id: usize, after: u32, then: usize) -> usize;
    fn KillTimer(window: Handle, id: usize) -> i32;
    fn GetActiveWindow() -> Handle;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalFree(memory: Handle) -> Handle;
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
/// `GetDeviceCaps` indexes: dots per inch, and the paper's size and unprintable margin in
/// device pixels.
const DOTS_X: i32 = 88;
const DOTS_Y: i32 = 90;
const PAPER_W: i32 = 110;
const PAPER_H: i32 = 111;
const EDGE_X: i32 = 112;
const EDGE_Y: i32 = 113;
/// Rows of a page drawn at a time.
const BAND: usize = 256;
/// How many page ranges the person may give in the dialog.
const STRETCHES: usize = 16;

/// What shows the dialog: Windows' own, or a stand-in in tests.
pub(crate) type Shows = unsafe extern "system" fn(*mut Asked) -> i32;

/// Pages to print, their title (UTF-16), and where the result is told.
type Asking = (Paper, Vec<u16>, Arc<Mutex<Option<Printed>>>);

thread_local! {
    /// Pages asked for and not printed yet.
    static ASKED: RefCell<Vec<Asking>> = const { RefCell::new(Vec::new()) };
    /// What shows the dialog on this thread.
    pub(crate) static SHOWS: Cell<Shows> = const { Cell::new(PrintDlgExW) };
}

/// Keeps the pages to print and has the thread's messages bring the dialog as soon as they are
/// taken (a zero-time timer on the thread).
pub(crate) fn ask(paper: Paper, title: &str, told: Arc<Mutex<Option<Printed>>>) {
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    ASKED.with_borrow_mut(|asked| asked.push((paper, title, told)));
    // SAFETY: a timer of this thread, which calls `shown` (a `TIMERPROC`) from its messages.
    unsafe { SetTimer(0, 0, 0, shown as *const () as usize) };
}

/// The timer: shows the dialog for each paper asked for, and prints it.
unsafe extern "system" fn shown(_: Handle, _: u32, timer: usize, _: u32) {
    // SAFETY: the timer is this thread's own and is done with.
    unsafe { KillTimer(0, timer) };
    while let Some((paper, title, told)) = ASKED.with_borrow_mut(Vec::pop) {
        // SAFETY: asks which of this thread's windows is active.
        let owner = unsafe { GetActiveWindow() };
        tell(&told, print_with(&paper, &title, owner, SHOWS.get()));
    }
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
/// printed.
pub(crate) fn print_with(paper: &Paper, title: &[u16], owner: Handle, shows: Shows) -> Printed {
    let mut ranges = [[0u32; 2]; STRETCHES];
    let mut asked = dialog(owner, paper.pages(), &mut ranges);
    // SAFETY: the arguments outlive the call; what the dialog hands back (the printer's
    // settings and device context) is freed below.
    let said = unsafe { shows(&mut asked) };
    let printed = match (said, asked.result) {
        (0, PRINT) if asked.dc != 0 => {
            let all = asked.flags & PAGE_NUMBERS == 0;
            let pages: Vec<usize> =
                (0..paper.pages()).filter(|page| all || ranges[..asked.ranges.min(STRETCHES as u32) as usize].iter().any(|[from, to]| (*from..=*to).contains(&(*page as u32 + 1)))).collect();
            spool(asked.dc, paper, title, &pages)
        }
        (0, _) => Printed::Closed,
        _ => Printed::Unable,
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

/// Sends pages to a printer's device context as a document of this title, each page drawn on
/// it.
pub(crate) fn spool(dc: Handle, paper: &Paper, title: &[u16], pages: &[usize]) -> Printed {
    let document = Document { size: size_of::<Document>() as i32, name: title.as_ptr(), output: std::ptr::null(), kind: std::ptr::null(), flags: 0 };
    // SAFETY: the device context is the printer's, and the title outlives the call.
    if unsafe { StartDocW(dc, &document) } <= 0 {
        return Printed::Unable;
    }
    for page in pages {
        // SAFETY: a page of the document begun above, drawn and ended.
        let drawn = unsafe { StartPage(dc) > 0 && draw_page(dc, paper, *page) && EndPage(dc) > 0 };
        if !drawn {
            // SAFETY: the document begun above is abandoned.
            unsafe { AbortDoc(dc) };
            return Printed::Unable;
        }
    }
    // SAFETY: the document begun above ends and goes to the printer.
    if unsafe { EndDoc(dc) } > 0 { Printed::Sent } else { Printed::Unable }
}

/// Draws a page on a device context from the paper's corner: at its own size where it fits,
/// scaled down where it does not. A band of rows at a time, drawn as [`Paper::draw`] draws it,
/// then handed over as pixels. Says whether all of it was taken.
///
/// # Safety
/// `dc` is a live device context.
pub(crate) unsafe fn draw_page(dc: Handle, paper: &Paper, page: usize) -> bool {
    // SAFETY: as the caller promises.
    let ask = |what: i32| unsafe { GetDeviceCaps(dc, what) } as f32;
    let (dots, size) = ([ask(DOTS_X).max(1.0), ask(DOTS_Y).max(1.0)], paper.size(page));
    let fits = (ask(PAPER_W) / (size.w * dots[0] / 96.0)).min(ask(PAPER_H) / (size.h * dots[1] / 96.0)).min(1.0);
    let scale = dots[0] / 96.0 * if fits > 0.0 { fits } else { 1.0 };
    let (width, height) = ((size.w * scale).round().max(1.0) as usize, (size.h * scale).round().max(1.0) as usize);
    let (edge_x, edge_y) = (ask(EDGE_X) as i32, ask(EDGE_Y) as i32);
    let mut band = vec![0u32; width * BAND];
    for top in (0..height).step_by(BAND) {
        let rows = BAND.min(height - top);
        band.fill(0xffff_ffff);
        paper.draw(page, &mut Canvas::new(&mut band[..width * rows], width, rows, (0, top as i32), scale));
        let block = Block { size: size_of::<Block>() as u32, width: width as i32, height: -(rows as i32), planes: 1, bits: 32, packing: 0, bytes: 0, across: 0, down: 0, used: 0, important: 0 };
        // SAFETY: the band holds as many pixels as the header says.
        let laid = unsafe { StretchDIBits(dc, -edge_x, top as i32 - edge_y, width as i32, rows as i32, 0, 0, width as i32, rows as i32, band.as_ptr(), &block, 0, 0x00CC_0020) };
        if laid == 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::geom::rect;
    use crate::paper::Paper;
    use crate::print::{Printed, print};
    use crate::windows::sys::{BITMAPINFO, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DispatchMessageW, MSG, SelectObject};
    use std::cell::RefCell;

    type Handle = isize;

    /// A page with shapes and a picture (no text, so installed fonts do not matter).
    fn page() -> Paper {
        let mut paper = Paper::new();
        paper.page(crate::geom::Size { w: 300.0, h: 200.0 });
        paper.rect(rect(10.0, 10.0, 120.0, 80.0), Color::rgb(0x2b6cb0));
        paper.rounded(rect(150.0, 20.0, 120.0, 160.0), 30.0, Color::rgb(0xe53e3e));
        let pixels: Vec<u32> = (0..16 * 16).map(|n| 0xff00_0000 | (n * 16) << 8).collect();
        paper.picture(16, &pixels, rect(20.0, 110.0, 64.0, 64.0));
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
    }

    #[test]
    fn a_page_goes_to_a_device_as_the_paper_draws_it() {
        let paper = page();
        let (dc, bits, picture) = memory(300, 200);
        // SAFETY: the context is alive; its 300 by 200 pixels are read once drawn.
        let drawn = unsafe { draw_page(dc, &paper, 0) };
        let got: Vec<u32> = unsafe { std::slice::from_raw_parts(bits, 300 * 200) }.iter().map(|pixel| pixel & 0xff_ffff).collect();
        unsafe {
            DeleteDC(dc);
            DeleteObject(picture);
        }
        let (_, own) = paper.picture_of(0, 1.0);
        assert!(drawn);
        assert_eq!(got, own.iter().map(|pixel| pixel & 0xff_ffff).collect::<Vec<_>>(), "the device has the pixels the paper draws");
    }

    thread_local! {
        /// What the stand-in dialog was asked, and how it answers: its result, what the person
        /// did, and the device context it hands back.
        static SEEN: RefCell<Vec<(u32, u32, u32)>> = const { RefCell::new(Vec::new()) };
        static ANSWER: RefCell<(i32, u32, Handle)> = const { RefCell::new((0, 0, 0)) };
    }

    /// The dialog as a test plays it: notes what it was asked, and answers as told.
    unsafe extern "system" fn stand_in(asked: *mut Asked) -> i32 {
        // SAFETY: the arguments the printing code made, alive while it waits.
        let asked = unsafe { &mut *asked };
        SEEN.with_borrow_mut(|seen| seen.push((asked.most_page, asked.flags, asked.start_page)));
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
    fn the_dialog_comes_between_messages_and_what_the_person_did_is_told() {
        // Never Windows' own dialog in a test: a stand-in for this thread.
        SHOWS.set(stand_in);
        ANSWER.set((0, 0, 0));
        let printing = print(page(), "A test");
        assert_eq!(printing.done(), None, "nothing is shown before the thread takes its messages");
        messages_until(|| printing.done().is_some());
        assert_eq!(printing.done(), Some(Printed::Closed), "the person closed the dialog");
        assert_eq!(SEEN.with_borrow(Clone::clone), [(1, 0x100 | 0x4_0000 | 0x4 | 0x80_0000, 0xFFFF_FFFF)]);
        // A dialog the system could not show: nothing printed.
        ANSWER.set((-2147024809, 0, 0));
        let printing = print(page(), "A test");
        messages_until(|| printing.done().is_some());
        assert_eq!(printing.done(), Some(Printed::Unable));
    }

    #[test]
    fn a_page_of_endless_size_goes_to_a_printer_drawn_to_fit() {
        let mut paper = Paper::new();
        paper.page(crate::geom::Size { w: f32::INFINITY, h: f32::INFINITY });
        paper.rect(rect(0.0, 0.0, 50.0, 50.0), Color::BLACK);
        let block = BITMAPINFO { size: 40, width: 200, height: -200, planes: 1, bits: 32, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        // SAFETY: a context and block of pixels of the test's own, freed after.
        unsafe {
            let dc = CreateCompatibleDC(0);
            let picture = CreateDIBSection(dc, &block, 0, &mut bits, 0, 0);
            SelectObject(dc, picture);
            // Printing draws in a timer on the windows' thread, where a panic ends the program.
            let drawn = std::panic::catch_unwind(|| draw_page(dc, &paper, 0));
            DeleteDC(dc);
            DeleteObject(picture);
            assert!(drawn.is_ok(), "drawing the page panicked");
        }
    }
}
