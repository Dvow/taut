//! Printing a [`Paper`]: the system's print dialog, where the person picks a printer, the pages
//! and the copies, then the pages go to that printer.
//!
//! On Windows the pages go to the printer's spooler as pixels (`print_windows.rs`); on Linux
//! the desktop portal shows its dialog and takes the pages as a PDF (`print_linux.rs`). Where
//! neither exists, nothing is printed.
//!
//! The call returns at once; [`PrintJob`] says what came of it later, and the window runs again
//! when it is known. A program that never calls `print` carries none of this.
use crate::paper::Paper;
use std::sync::{Arc, Mutex};

#[cfg(all(unix, not(target_os = "macos")))]
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
    ask(paper, title, printing.outcome.clone());
    printing
}

/// Says what came of printing, and has the window run again to hear it.
pub(crate) fn tell(outcome: &Mutex<Option<PrintOutcome>>, printed: PrintOutcome) {
    *outcome.lock().unwrap_or_else(|held| held.into_inner()) = Some(printed);
    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    crate::platform::Waker.wake();
}

/// No way of printing: nothing is printed.
#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
fn ask(paper: Paper, title: &str, outcome: Arc<Mutex<Option<PrintOutcome>>>) {
    let _ = (paper, title);
    tell(&outcome, PrintOutcome::Unable);
}
