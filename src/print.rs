//! Printing a [`Paper`]: the system's print dialog, where the person picks a printer, the pages
//! and the copies, then the pages go to that printer.
//!
//! On Windows the pages go to the printer's spooler as shapes and text, or as pixels where the
//! printer can't blend (`print_windows.rs`); on Linux the desktop portal shows its dialog and
//! takes the pages as a PDF (`print_linux.rs`), as Android's print dialog does
//! (`android_print.rs`). Elsewhere nothing is printed.
//!
//! Pages can be laid out for the paper the person picks ([`print_laid`]): the app's layout runs
//! again for the sheet's size once it is known.
//!
//! The call returns at once; [`PrintJob`] says what came of it later, and the window runs again
//! when it is known. A program that never calls `print` carries none of this.
use crate::geom::Size;
use crate::paper::Paper;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "android")]
use crate::android_print::ask;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
use crate::print_linux::ask;
#[cfg(windows)]
use crate::print_windows::ask;

/// What came of printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrintOutcome {
    /// The pages went to the printer the person picked.
    Sent,
    /// The person closed the dialog without printing.
    Closed,
    /// Printing is not possible here: no printer, no dialog, or the system refused the pages.
    Unable,
}

/// Printing that was asked for, and what came of it once known.
#[derive(Clone, Debug)]
pub struct PrintJob {
    outcome: Arc<Mutex<Option<PrintOutcome>>>,
}

impl PrintJob {
    /// What came of it: nothing while the dialog is up and the pages are on their way. The
    /// window runs again once it is known.
    pub fn outcome(&self) -> Option<PrintOutcome> {
        *self.outcome.lock().unwrap_or_else(|held| held.into_inner())
    }
}

/// Prints the pages of `paper`: shows the system's print dialog (printer, pages, copies) and
/// sends the pages there, listed as `title` in the printer's queue. Returns at once;
/// [`PrintJob::outcome`] says what came of it. On Windows the dialog shows over the window in
/// use once the thread's messages are taken: at once under a taut window, else from the
/// program's own message loop.
///
/// ```no_run
/// # use taut::{Color, Font, Paper, PrintJob, Ui};
/// # fn describe(ui: &mut Ui, printing: &mut Option<PrintJob>) {
/// if ui.button("Print").clicked {
///     let mut paper = Paper::new();
///     paper.page(Paper::A4);
///     paper.text("Hello, printer", 72.0, 72.0, Font::new(14.0), Color::BLACK);
///     *printing = Some(taut::print(paper, "A letter"));
/// }
/// if printing.as_ref().and_then(PrintJob::outcome).is_some() {
///     *printing = None; // done: printed, or the dialog closed
/// }
/// # }
/// ```
pub fn print(paper: Paper, title: &str) -> PrintJob {
    let printing = PrintJob { outcome: Arc::new(Mutex::new(None)) };
    ask(Pages::Ready(Box::new(paper)), title, printing.outcome.clone());
    printing
}

/// Prints pages laid out for the paper the person picks: shows the dialog as [`print()`] does,
/// then calls `lay` with the sheet's size in units (A4 or Letter, upright or across, or any
/// other) and prints the pages it gives. (Before the dialog, it is called once for A4, to count
/// the pages the dialog offers.)
///
/// ```no_run
/// # use taut::{Markdown, Paper};
/// let report = Markdown::parse("# Report\n\nWords.");
/// taut::print_laid("Report", move |sheet| {
///     let mut paper = Paper::new();
///     paper.markdown(&report, sheet, 72.0);
///     paper
/// });
/// ```
pub fn print_laid(title: &str, lay: impl FnMut(Size) -> Paper + Send + 'static) -> PrintJob {
    let printing = PrintJob { outcome: Arc::new(Mutex::new(None)) };
    ask(Pages::Laid(Box::new(lay)), title, printing.outcome.clone());
    printing
}

/// Pages to print: laid out already, or laid out for the sheet once it is known.
#[cfg_attr(target_vendor = "apple", allow(dead_code, reason = "Apple systems print nothing yet"))]
pub(crate) enum Pages {
    Ready(Box<Paper>),
    Laid(Box<dyn FnMut(Size) -> Paper + Send>),
}

#[cfg_attr(target_vendor = "apple", allow(dead_code, reason = "Apple systems print nothing yet"))]
impl Pages {
    /// The pages for a sheet of `size` units (where known): laid out for it, or as they are.
    pub(crate) fn for_sheet(&mut self, size: Option<Size>) -> Paper {
        match self {
            Pages::Ready(paper) => std::mem::take(&mut **paper),
            Pages::Laid(lay) => lay(size.unwrap_or(Paper::A4)),
        }
    }
    /// How many pages there are, laid out for A4 where not yet laid out. (What the Windows
    /// dialog offers.)
    #[cfg(windows)]
    pub(crate) fn count(&mut self) -> usize {
        match self {
            Pages::Ready(paper) => paper.pages(),
            Pages::Laid(lay) => lay(Paper::A4).pages(),
        }
    }
}

/// The window print dialogs show over, as the desktop portal names it ("x11:" and its id, or
/// "wayland:" and its exported handle): the program's first window, once it has one.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
static PARENT: Mutex<String> = Mutex::new(String::new());

/// Notes the window print dialogs show over (from the windows' host).
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) fn set_parent(window: String) {
    *PARENT.lock().unwrap_or_else(|held| held.into_inner()) = window;
}

/// The window print dialogs show over ("" for none).
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) fn parent() -> String {
    PARENT.lock().unwrap_or_else(|held| held.into_inner()).clone()
}

/// Says what came of printing, and has the window run again to hear it.
pub(crate) fn tell(outcome: &Mutex<Option<PrintOutcome>>, printed: PrintOutcome) {
    *outcome.lock().unwrap_or_else(|held| held.into_inner()) = Some(printed);
    #[cfg(any(windows, all(unix, not(target_vendor = "apple"))))]
    crate::platform::Waker.wake();
}

/// No way of printing: nothing is printed.
#[cfg(not(any(windows, all(unix, not(target_vendor = "apple")))))]
fn ask(pages: Pages, title: &str, outcome: Arc<Mutex<Option<PrintOutcome>>>) {
    let _ = (pages, title);
    tell(&outcome, PrintOutcome::Unable);
}
