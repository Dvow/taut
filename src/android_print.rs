//! Printing on Android: the system's print dialog (`PrintManager`), with its preview, its
//! printers and "Save as PDF". The pages go to it as a PDF, through the package's adapter class
//! `taut.Pages` (see `examples/apk.rs`), given its natives here the first time an app prints:
//! the dialog says the sheet (pages laid out for it, with `print_laid`), then asks for the
//! pages, for its preview and, once the person confirms, for the printer (or the PDF file).
use crate::android::activity;
use crate::android_jni::{self as jni, Env, Native, Obj, Value};
use crate::android_sys::log;
use crate::geom::Size;
use crate::paper::Paper;
use crate::print::{Pages, PrintOutcome, tell};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::io::Write;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::{Arc, Mutex};

/// Units an inch, as a paper's size counts them; the dialog counts thousandths of an inch.
const UNITS_AN_INCH: f32 = 96.0;
const MILS_AN_INCH: f32 = 1000.0;

/// A print job while the dialog is up.
struct Job {
    /// The adapter the dialog asks (a global reference).
    adapter: Obj,
    title: String,
    pages: Pages,
    /// The pages as laid out for the sheet last said.
    laid: Paper,
    /// The person confirmed (the dialog laid the pages out for printing, not its preview), and
    /// whether the pages went to it then.
    confirmed: bool,
    sent: bool,
    outcome: Arc<Mutex<Option<PrintOutcome>>>,
}

thread_local! {
    static JOBS: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
    /// `taut.Pages` has its natives.
    static READY: Cell<bool> = const { Cell::new(false) };
}

/// Shows the print dialog for the pages; says what came of it once done. (On the main thread
/// only, where the activity is.)
pub(crate) fn ask(pages: Pages, title: &str, outcome: Arc<Mutex<Option<PrintOutcome>>>) {
    let (env, activity) = (jni::env(), activity());
    let Some(env) = env.filter(|_| !activity.is_null()) else { return tell(&outcome, PrintOutcome::Unable) };
    let started = env.frame(|| start(env, activity, title));
    let Some(adapter) = started else {
        log("taut: printing needs the package's class taut.Pages (see examples/apk.rs), on the main thread.");
        return tell(&outcome, PrintOutcome::Unable);
    };
    JOBS.with_borrow_mut(|jobs| jobs.push(Job { adapter, title: title.into(), pages, laid: Paper::new(), confirmed: false, sent: false, outcome }));
}

/// Shows the dialog with a new adapter: the adapter (a global reference).
fn start(env: Env, activity: Obj, title: &str) -> Option<Obj> {
    let class = jni::app_class(env, activity, "taut.Pages");
    if class.is_null() || !READY.get() && !register(env, class) {
        return None;
    }
    READY.set(true);
    let adapter = env.global(env.new_object(class, c"()V", &[]));
    let args = [Value::obj(env.string(title)), Value::obj(adapter), Value::obj(std::ptr::null_mut())];
    let job =
        env.call(jni::service(env, activity, "print"), c"print", c"(Ljava/lang/String;Landroid/print/PrintDocumentAdapter;Landroid/print/PrintAttributes;)Landroid/print/PrintJob;", &args).object();
    if adapter.is_null() || job.is_null() {
        env.drop_global(adapter);
        return None;
    }
    Some(adapter)
}

/// Gives `taut.Pages` its natives.
fn register(env: Env, class: Obj) -> bool {
    env.register(
        class,
        &[
            Native::new(
                c"onLayout",
                c"(Landroid/print/PrintAttributes;Landroid/print/PrintAttributes;Landroid/os/CancellationSignal;Landroid/print/PrintDocumentAdapter$LayoutResultCallback;Landroid/os/Bundle;)V",
                lay_out as *const c_void,
            ),
            Native::new(
                c"onWrite",
                c"([Landroid/print/PageRange;Landroid/os/ParcelFileDescriptor;Landroid/os/CancellationSignal;Landroid/print/PrintDocumentAdapter$WriteResultCallback;)V",
                write as *const c_void,
            ),
            Native::new(c"onFinish", c"()V", finish as *const c_void),
        ],
    )
}

/// Runs `work` on the job of an adapter, if it is one.
fn with_job<T>(env: Env, adapter: Obj, work: impl FnOnce(&mut Job) -> T) -> Option<T> {
    JOBS.with_borrow_mut(|jobs| jobs.iter_mut().find(|job| env.same(job.adapter, adapter)).map(work))
}

/// The sheet the dialog says, in units.
fn sheet(env: Env, attributes: Obj) -> Option<Size> {
    let media = env.call(attributes, c"getMediaSize", c"()Landroid/print/PrintAttributes$MediaSize;", &[]).object();
    let [width, height] = [c"getWidthMils", c"getHeightMils"].map(|name| env.call(media, name, c"()I", &[]).as_int() as f32 * UNITS_AN_INCH / MILS_AN_INCH);
    (width > 0.0 && height > 0.0).then_some(Size { w: width, h: height })
}

/// The dialog says the sheet, for its preview or for printing: the pages are laid out for
/// it, and counted.
extern "C" fn lay_out(env: Env, adapter: Obj, _: Obj, attributes: Obj, _: Obj, callback: Obj, extras: Obj) {
    let size = sheet(env, attributes);
    let preview = env.call(extras, c"getBoolean", c"(Ljava/lang/String;Z)Z", &[Value::obj(env.string("EXTRA_PRINT_PREVIEW")), Value::bool(true)]).as_bool();
    let Some((title, count)) = with_job(env, adapter, |job| {
        job.confirmed |= !preview;
        job.laid = job.pages.for_sheet(size);
        (job.title.clone(), job.laid.pages())
    }) else {
        return;
    };
    env.frame(|| {
        let builder = env.new_named(c"android/print/PrintDocumentInfo$Builder", c"(Ljava/lang/String;)V", &[Value::obj(env.string(&title))]);
        env.call(builder, c"setContentType", c"(I)Landroid/print/PrintDocumentInfo$Builder;", &[Value::int(0)]);
        env.call(builder, c"setPageCount", c"(I)Landroid/print/PrintDocumentInfo$Builder;", &[Value::int(count as i32)]);
        let info = env.call(builder, c"build", c"()Landroid/print/PrintDocumentInfo;", &[]).object();
        env.call(callback, c"onLayoutFinished", c"(Landroid/print/PrintDocumentInfo;Z)V", &[Value::obj(info), Value::bool(true)]);
    });
}

/// The dialog asks for the pages: all of them go, as a PDF.
extern "C" fn write(env: Env, adapter: Obj, _: Obj, file: Obj, _: Obj, callback: Obj) {
    let pdf = with_job(env, adapter, |job| job.laid.to_pdf()).unwrap_or_default();
    let fd = env.call(file, c"getFd", c"()I", &[]).as_int();
    let written = fd >= 0 && !pdf.is_empty() && write_pdf(fd, &pdf);
    with_job(env, adapter, |job| job.sent = job.confirmed && written);
    if written {
        env.frame(|| {
            let all = env.static_object(c"android/print/PageRange", c"ALL_PAGES", c"Landroid/print/PageRange;");
            let ranges = jni::new_objects(env, c"android/print/PageRange", &[all]);
            env.call(callback, c"onWriteFinished", c"([Landroid/print/PageRange;)V", &[Value::obj(ranges)]);
        });
    } else {
        env.frame(|| env.call(callback, c"onWriteFailed", c"(Ljava/lang/CharSequence;)V", &[Value::obj(env.string("The pages could not be written."))]));
    }
}

/// Writes the PDF into the dialog's file (`fd`, the dialog's: written through a copy of it).
fn write_pdf(fd: i32, pdf: &[u8]) -> bool {
    // SAFETY: the file descriptor is open for the length of the call that handed it.
    let own: Option<OwnedFd> = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned().ok();
    own.is_some_and(|own| std::fs::File::from(own).write_all(pdf).is_ok())
}

/// The dialog is done: the app hears whether the pages went to the printer the person
/// confirmed, could not, or the dialog was closed.
extern "C" fn finish(env: Env, adapter: Obj) {
    let taken = JOBS.with_borrow_mut(|jobs| jobs.iter().position(|job| env.same(job.adapter, adapter)).map(|at| jobs.remove(at)));
    let Some(job) = taken else { return };
    let outcome = match (job.confirmed, job.sent) {
        (false, _) => PrintOutcome::Closed,
        (true, true) => PrintOutcome::Sent,
        (true, false) => PrintOutcome::Unable,
    };
    tell(&job.outcome, outcome);
    env.drop_global(job.adapter);
}
