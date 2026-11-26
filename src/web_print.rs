//! Printing on the web: the browser's print dialog (`window.print`), with its preview, its
//! printers and "Save as PDF". Each page is drawn as [`Paper::render_page`] draws it, at 288
//! dots an inch, and printed alone on a sheet of its own size (CSS named pages); nothing else of
//! the page is printed. Pages laid out for the sheet ([`print_laid`](crate::print_laid)) are
//! laid out for Letter where the browser's language names a country using it, else for A4.
//!
//! The browser doesn't say whether the person printed or closed its dialog: once it closes, the
//! job is [`PrintOutcome::Sent`]. With no page to print in (no document, or no pages), it is
//! [`PrintOutcome::Unable`].
use crate::paper::Paper;
use crate::print::{Pages, PrintOutcome, tell};
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts, took};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn region() -> usize;
    fn print_page(rgba: *const u8, length: usize, width: u32, height: u32, page_width: f32, page_height: f32);
    fn print_pages(title: *const u8, length: usize, job: u32);
}

/// Pixels a unit the pages are drawn at: 288 dots an inch.
const SCALE: f32 = 3.0;

/// Countries whose paper is Letter (CLDR's `paperSize`).
const LETTER: [&str; 16] = ["US", "CA", "MX", "PR", "PH", "CL", "CO", "VE", "GT", "PA", "SV", "NI", "HN", "CR", "DO", "BZ"];

/// What the loader hears of printing, for job `number`: its dialog closed, or it can't print.
const CLOSED: u32 = 0;
const UNABLE: u32 = 1;

/// What came of a job, once known.
type Outcome = Arc<Mutex<Option<PrintOutcome>>>;

/// The jobs whose dialog is up, by number, and the last number given.
#[derive(Default)]
struct Jobs {
    open: Vec<(u32, Outcome)>,
    last: u32,
}

thread_local! {
    static JOBS: RefCell<Jobs> = RefCell::new(Jobs::default());
}

/// Shows the print dialog for the pages; says what came of it once it closes.
pub(crate) fn ask(mut pages: Pages, title: &str, outcome: Outcome) {
    add_part(parts::PRINT, Part { after: |_| {}, heard });
    let sheet = if uses_letter() { Paper::LETTER } else { Paper::A4 };
    let paper = pages.for_sheet(Some(sheet));
    (0..paper.pages()).for_each(|page| hand_page(&paper, page));
    let job = JOBS.with_borrow_mut(|jobs| {
        jobs.last += 1;
        jobs.open.push((jobs.last, outcome));
        jobs.last
    });
    // SAFETY: the loader reads `length` bytes; it answers through `heard`.
    unsafe { print_pages(title.as_ptr(), title.len(), job) };
}

/// Whether the browser's language names a country whose paper is Letter.
fn uses_letter() -> bool {
    // SAFETY: a plain question, answered with the length of the country's code.
    let length = unsafe { region() };
    let region = took(length);
    LETTER.iter().any(|country| country.as_bytes() == region)
}

/// Draws a page and hands its pixels to the loader, red first.
fn hand_page(paper: &Paper, page: usize) {
    let size = paper.size(page);
    let (width, mut pixels) = paper.render_page(page, SCALE);
    // 0xAARRGGBB becomes the bytes red, green, blue, alpha.
    for pixel in &mut pixels {
        *pixel = pixel.swap_bytes().rotate_right(8);
    }
    let height = pixels.len() / width.max(1);
    // SAFETY: four bytes a pixel, read at once.
    unsafe { print_page(pixels.as_ptr().cast(), pixels.len() * 4, width as u32, height as u32, size.w, size.h) };
}

/// A print dialog closed, or a job can't print.
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    let job = heard.number as u32;
    let outcome = match heard.what {
        CLOSED => PrintOutcome::Sent,
        UNABLE => PrintOutcome::Unable,
        _ => return (Then::Wait, 0),
    };
    let done = JOBS.with_borrow_mut(|jobs| jobs.open.iter().position(|(number, _)| *number == job).map(|at| jobs.open.remove(at).1));
    done.iter().for_each(|done| tell(done, outcome));
    (Then::Run, 0)
}
