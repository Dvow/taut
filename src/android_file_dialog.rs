//! The system's file dialog on Android: the document picker of the Storage Access Framework,
//! over the app's activity. Opening (`ACTION_OPEN_DOCUMENT`, several documents for
//! `pick_many`), the picked documents come back as copies once copied (`android_files`);
//! saving (`ACTION_CREATE_DOCUMENT`), the new document comes back as its empty copy, written
//! back once the app writes it. The answer reaches the activity (`taut.Activity`, see
//! `examples/apk.rs`), which keeps it for the next run. (A folder has no path on Android: the
//! library's own dialog picks among the app's folders.)
use crate::android::{Part, activity, add_part, kick};
use crate::android_files::{self as files, Copying, GRANT_READ};
use crate::android_jni::{self as jni, Env, Obj, Value};
use crate::android_sys::log;
use crate::file_dialog::FileDialogRequest;
use crate::ui::Ui;
use std::cell::RefCell;
use std::path::PathBuf;

/// The request codes of the picker, opening and saving.
const OPENING: i32 = 0x7441;
const SAVING: i32 = 0x7442;
/// `Activity.RESULT_OK`.
const PICKED: i32 = -1;
/// `Intent.FLAG_GRANT_WRITE_URI_PERMISSION`.
const GRANT_WRITE: i32 = 2;

/// What was asked of the picker, and what came of it so far.
#[derive(Default)]
struct Asked {
    /// The picker's request code while it is up.
    code: i32,
    /// Several documents may be picked.
    many: bool,
    /// Documents picked, being copied.
    copying: Option<Copying>,
    /// Every copy of the last answer.
    chosen: Vec<PathBuf>,
    /// The answer is nothing, at the next run: no picker could be shown.
    refused: bool,
}

thread_local! {
    static ASKED: RefCell<Asked> = RefCell::new(Asked::default());
}

/// Has the windows ask the picker for the file dialog, from the next run on.
pub(crate) fn hand_file_dialog() {
    add_part(Part { name: "file dialog", before: take_answer, after: ask_if_asked });
}

/// Notes whether the dialog being asked for picks several files.
pub(crate) fn note_many(many: bool) {
    ASKED.with_borrow_mut(|asked| asked.many = many);
}

/// Every file of the answer whose first is `first`.
pub(crate) fn chosen(first: PathBuf) -> Vec<PathBuf> {
    let all = ASKED.with_borrow_mut(|asked| std::mem::take(&mut asked.chosen));
    if all.first() == Some(&first) { all } else { vec![first] }
}

/// After a run: shows the picker if the app asked for the dialog.
fn ask_if_asked(ui: &mut Ui) {
    let Some(request) = ui.take_file_dialog_request() else { return };
    let (env, activity) = (jni::env(), activity());
    let shown = env.is_some_and(|env| env.frame(|| show_picker(env, activity, &request)));
    if !shown {
        log("taut: the document picker needs the activity taut.Activity (see examples/apk.rs).");
        ASKED.with_borrow_mut(|asked| asked.refused = true);
        kick();
    }
}

/// Starts the picker for a request: says whether it showed.
fn show_picker(env: Env, activity: Obj, request: &FileDialogRequest) -> bool {
    if !jni::is_app_activity(env, activity) {
        return false;
    }
    let action = if request.saving { "android.intent.action.CREATE_DOCUMENT" } else { "android.intent.action.OPEN_DOCUMENT" };
    let intent = env.new_named(c"android/content/Intent", c"(Ljava/lang/String;)V", &[Value::obj(env.string(action))]);
    env.call(intent, c"addCategory", c"(Ljava/lang/String;)Landroid/content/Intent;", &[Value::obj(env.string("android.intent.category.OPENABLE"))]);
    let kinds: Vec<String> = request.endings.iter().map(|ending| files::type_of(env, &PathBuf::from(format!("x{ending}")))).collect();
    let kind = if kinds.len() == 1 { kinds[0].as_str() } else { "*/*" };
    env.call(intent, c"setType", c"(Ljava/lang/String;)Landroid/content/Intent;", &[Value::obj(env.string(kind))]);
    if kinds.len() > 1 {
        let listed: Vec<Obj> = kinds.iter().map(|kind| env.string(kind)).collect();
        put_extra(env, intent, "android.intent.extra.MIME_TYPES", c"(Ljava/lang/String;[Ljava/lang/String;)Landroid/content/Intent;", Value::obj(jni::new_objects(env, c"java/lang/String", &listed)));
    }
    let many = ASKED.with_borrow(|asked| asked.many) && !request.saving;
    put_extra(env, intent, "android.intent.extra.ALLOW_MULTIPLE", c"(Ljava/lang/String;Z)Landroid/content/Intent;", Value::bool(many));
    if request.saving {
        put_extra(env, intent, "android.intent.extra.TITLE", c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", Value::obj(env.string(&request.name)));
    }
    env.call(intent, c"addFlags", c"(I)Landroid/content/Intent;", &[Value::int(GRANT_READ | GRANT_WRITE)]);
    let code = if request.saving { SAVING } else { OPENING };
    env.set_int_field(activity, c"request", 0);
    let started = env.run(activity, c"startActivityForResult", c"(Landroid/content/Intent;I)V", &[Value::obj(intent), Value::int(code)]);
    ASKED.with_borrow_mut(|asked| asked.code = if started { code } else { 0 });
    started
}

/// Puts an extra into an intent, by `putExtra` of this signature.
fn put_extra(env: Env, intent: Obj, name: &str, signature: &std::ffi::CStr, value: Value) {
    env.call(intent, c"putExtra", signature, &[Value::obj(env.string(name)), value]);
}

/// Before a run: the picker's answer, once it came and its documents are copied. Says whether
/// the interface hears one.
fn take_answer(ui: &mut Ui) -> bool {
    if ASKED.with_borrow_mut(|asked| std::mem::take(&mut asked.refused)) {
        ui.file_dialog_answered(None);
        return true;
    }
    if let Some(copies) = ASKED.with_borrow(|asked| asked.copying.as_ref().and_then(files::copied)) {
        ASKED.with_borrow_mut(|asked| asked.copying = None);
        answer(ui, copies);
        return true;
    }
    let (env, activity, code) = (jni::env(), activity(), ASKED.with_borrow(|asked| asked.code));
    let Some(env) = env.filter(|_| code != 0 && !activity.is_null()) else { return false };
    if env.int_field(activity, c"request") != code {
        return false;
    }
    ASKED.with_borrow_mut(|asked| asked.code = 0);
    env.frame(|| take_result(env, activity, ui, code))
}

/// The picker's result, kept by the activity: the new document to save into, or the documents
/// picked, which are copied before the interface hears of them. Says whether it hears now.
fn take_result(env: Env, activity: Obj, ui: &mut Ui, code: i32) -> bool {
    let (result, data) = (env.int_field(activity, c"result"), env.object_field(activity, c"answer", c"Landroid/content/Intent;"));
    env.set_int_field(activity, c"request", 0);
    env.set_object_field(activity, c"answer", c"Landroid/content/Intent;", std::ptr::null_mut());
    if result != PICKED || data.is_null() {
        ui.file_dialog_answered(None);
        return true;
    }
    if code == SAVING {
        let address = env.call(data, c"getData", c"()Landroid/net/Uri;", &[]).object();
        answer(ui, files::new_document(env, activity, address).into_iter().collect());
        return true;
    }
    let opened = addresses(env, data).into_iter().filter_map(|address| files::open_document(env, activity, address)).collect();
    ASKED.with_borrow_mut(|asked| asked.copying = Some(files::copy_later(opened)));
    false
}

/// The documents an answer names: each of several, or the one.
fn addresses(env: Env, data: Obj) -> Vec<Obj> {
    let several = env.call(data, c"getClipData", c"()Landroid/content/ClipData;", &[]).object();
    let count = env.call(several, c"getItemCount", c"()I", &[]).as_int();
    if count > 0 {
        let item = |at: i32| env.call(several, c"getItemAt", c"(I)Landroid/content/ClipData$Item;", &[Value::int(at)]).object();
        return (0..count).map(|at| env.call(item(at), c"getUri", c"()Landroid/net/Uri;", &[]).object()).filter(|address| !address.is_null()).collect();
    }
    let one = env.call(data, c"getData", c"()Landroid/net/Uri;", &[]).object();
    if one.is_null() { Vec::new() } else { vec![one] }
}

/// Tells the interface the files chosen: the first, and all of them (see `chosen`).
fn answer(ui: &mut Ui, copies: Vec<PathBuf>) {
    let first = copies.first().cloned();
    ASKED.with_borrow_mut(|asked| asked.chosen = copies);
    ui.file_dialog_answered(first);
}
