//! Files on Android, where other apps' files are documents behind content addresses
//! (`content://…`), read through the `ContentResolver`, and the app's own files reach other
//! apps only through a provider.
//!
//! A document the person hands the app (picked in the file dialog, pasted, dropped) becomes a
//! copy in the app's own storage, named as the document is, so the app reads it as any file.
//! What the app then writes to the copy is written back to the document after the run that
//! wrote it. A file the app hands out (opened in another app, copied, dragged out) goes by an
//! address of the package's provider, `taut.Files`, readable by the app it is granted to.
use crate::android::{Part, activity, add_part, last_activity};
use crate::android_jni::{self as jni, Env, Obj, Value};
use crate::android_sys::log;
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`.
pub(crate) const GRANT_READ: i32 = 1;

/// A document copied in: its copy's folder (the copy is the newest file in it, whatever name
/// the app saved it under), its address (a global reference), and the copy's length and time
/// when last in step with the document.
struct Document {
    folder: PathBuf,
    address: Obj,
    in_step: Option<(u64, SystemTime)>,
}

thread_local! {
    /// The documents copied in so far.
    static DOCUMENTS: RefCell<Vec<Document>> = const { RefCell::new(Vec::new()) };
    /// Whether the package has the provider (asked once).
    static PROVIDED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// A document ready to copy: what reads it, and where its copy goes.
pub(crate) struct Opened {
    reader: File,
    copy: PathBuf,
}

/// Copies being made on a thread of their own: the copies' paths once all are made.
pub(crate) type Copying = Arc<Mutex<Option<Vec<PathBuf>>>>;

/// The app's content resolver.
pub(crate) fn resolver(env: Env, activity: Obj) -> Obj {
    env.call(activity, c"getContentResolver", c"()Landroid/content/ContentResolver;", &[]).object()
}

/// A document's file descriptor, opened in `mode` ("r", "wt"): ours to close.
fn descriptor(env: Env, activity: Obj, address: Obj, mode: &str) -> Option<OwnedFd> {
    let args = [Value::obj(address), Value::obj(env.string(mode))];
    let opened = env.call(resolver(env, activity), c"openFileDescriptor", c"(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;", &args).object();
    if opened.is_null() {
        return None;
    }
    let fd = env.call(opened, c"detachFd", c"()I", &[]).as_int();
    // SAFETY: a descriptor the parcel gave up: no one else closes it.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// A document's name, as its provider gives it (else its address's last part).
fn display_name(env: Env, activity: Obj, address: Obj) -> String {
    let columns = jni::new_objects(env, c"java/lang/String", &[env.string("_display_name")]);
    let null = Value::obj(std::ptr::null_mut());
    let args = [Value::obj(address), Value::obj(columns), null, null, null];
    let cursor = env.call(resolver(env, activity), c"query", c"(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;", &args).object();
    let named = env.call(cursor, c"moveToFirst", c"()Z", &[]).as_bool().then(|| env.text(env.call(cursor, c"getString", c"(I)Ljava/lang/String;", &[Value::int(0)]).object()));
    env.call(cursor, c"close", c"()V", &[]);
    let fallback = || env.text(env.call(address, c"getLastPathSegment", c"()Ljava/lang/String;", &[]).object());
    let name = named.flatten().or_else(fallback).unwrap_or_default();
    // (A name is one path part.)
    let name = name.replace(['/', '\0'], "_");
    if name.is_empty() || name == "." || name == ".." { "document".into() } else { name }
}

/// The folder copies go in, emptied of the copies an earlier run of the app left.
fn copies_folder() -> PathBuf {
    static EMPTIED: std::sync::Once = std::sync::Once::new();
    let folder = std::env::temp_dir().join("documents");
    EMPTIED.call_once(|| _ = std::fs::remove_dir_all(&folder));
    folder
}

/// A new copy for a document: an empty file of the document's name in a folder of its own,
/// kept in step with the document: as it is now if `filled`, else once copied in.
fn new_copy(env: Env, activity: Obj, address: Obj, filled: bool) -> Option<PathBuf> {
    let folder = copies_folder().join(DOCUMENTS.with_borrow(Vec::len).to_string());
    std::fs::create_dir_all(&folder).ok()?;
    let copy = folder.join(display_name(env, activity, address));
    File::create(&copy).ok()?;
    let in_step = if filled { state(&copy) } else { None };
    DOCUMENTS.with_borrow_mut(|documents| documents.push(Document { folder, address: env.global(address), in_step }));
    add_part(Part { name: "documents", before: |_| false, after: |_| write_back() });
    Some(copy)
}

/// Readies a document to be copied in: its copy, and the document opened to read, if it can
/// be.
pub(crate) fn open_document(env: Env, activity: Obj, address: Obj) -> Option<Opened> {
    let copy = new_copy(env, activity, address, false)?;
    let reader = File::from(descriptor(env, activity, address, "r")?);
    Some(Opened { reader, copy })
}

/// Readies a document that was just made, empty, to save into: its copy, empty too.
pub(crate) fn new_document(env: Env, activity: Obj, address: Obj) -> Option<PathBuf> {
    new_copy(env, activity, address, true)
}

/// Copies opened documents now: the copies made.
pub(crate) fn copy(opened: Vec<Opened>) -> Vec<PathBuf> {
    let copies = copy_all(opened);
    settle(&copies);
    copies
}

/// Copies opened documents on a thread of their own, waking the window when done.
pub(crate) fn copy_later(opened: Vec<Opened>) -> Copying {
    let done: Copying = Arc::new(Mutex::new(None));
    let theirs = done.clone();
    let copying = move || {
        *theirs.lock().unwrap_or_else(|held| held.into_inner()) = Some(copy_all(opened));
        crate::platform::Waker.wake();
    };
    if std::thread::Builder::new().name("taut documents".into()).spawn(copying).is_err() {
        log("taut: no thread to copy documents on.");
    }
    done
}

/// What copies made on a thread gave: none until all are made.
pub(crate) fn copied(copying: &Copying) -> Option<Vec<PathBuf>> {
    let copies = copying.lock().unwrap_or_else(|held| held.into_inner()).take()?;
    settle(&copies);
    Some(copies)
}

/// Copies opened documents: the copies read whole.
fn copy_all(opened: Vec<Opened>) -> Vec<PathBuf> {
    opened.into_iter().filter_map(copy_one).collect()
}

/// Copies one document: the copy, if it was read whole.
fn copy_one(mut opened: Opened) -> Option<PathBuf> {
    let mut copy = File::create(&opened.copy).ok()?;
    std::io::copy(&mut opened.reader, &mut copy).ok()?;
    Some(opened.copy)
}

/// Copies just made are in step with their documents.
fn settle(copies: &[PathBuf]) {
    DOCUMENTS.with_borrow_mut(|documents| {
        for document in documents.iter_mut().filter(|document| copies.iter().any(|copy| copy.starts_with(&document.folder))) {
            document.in_step = newest(&document.folder).as_deref().and_then(state);
        }
    });
}

/// A document's bytes, read now.
pub(crate) fn read(env: Env, activity: Obj, address: Obj) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::from(descriptor(env, activity, address, "r")?).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// A file's length and time of change.
fn state(file: &Path) -> Option<(u64, SystemTime)> {
    let about = std::fs::metadata(file).ok()?;
    Some((about.len(), about.modified().ok()?))
}

/// The copy in a document's folder: its newest file.
fn newest(folder: &Path) -> Option<PathBuf> {
    let files = std::fs::read_dir(folder).ok()?.filter_map(Result::ok).map(|entry| entry.path());
    files.max_by_key(|file| state(file).map(|(_, time)| time))
}

/// After a run: each copy the app changed since it was last in step is written back to its
/// document (one opened read only stays as it is).
fn write_back() {
    let (env, activity) = (jni::env(), activity());
    let Some(env) = env.filter(|_| !activity.is_null()) else { return };
    let changed: Vec<(PathBuf, Obj)> = DOCUMENTS.with_borrow_mut(|documents| documents.iter_mut().filter_map(changed_copy).collect());
    for (copy, address) in changed {
        let written = env.frame(|| descriptor(env, activity, address, "wt")).is_some_and(|writer| write_document(&copy, File::from(writer)));
        if !written {
            log(&format!("taut: {} was not written back to its document.", copy.display()));
        }
    }
}

/// A document's copy and address, if the copy changed since it was last in step (it is in step
/// from now on). None for a copy still being copied in.
fn changed_copy(document: &mut Document) -> Option<(PathBuf, Obj)> {
    let copy = newest(&document.folder)?;
    let now = state(&copy)?;
    let was = document.in_step?;
    document.in_step = Some(now);
    (was != now).then_some((copy, document.address))
}

/// Writes a copy into its document: whether it was written whole.
fn write_document(copy: &Path, mut document: File) -> bool {
    std::fs::read(copy).is_ok_and(|bytes| document.write_all(&bytes).is_ok())
}

// ---- the app's files, handed out ----

/// Whether the package has the provider `taut.Files` (see `examples/apk.rs`).
fn provided(env: Env, activity: Obj) -> bool {
    if let Some(known) = PROVIDED.get() {
        return known;
    }
    let manager = env.call(activity, c"getPackageManager", c"()Landroid/content/pm/PackageManager;", &[]).object();
    let found = env.call(manager, c"resolveContentProvider", c"(Ljava/lang/String;I)Landroid/content/pm/ProviderInfo;", &[Value::obj(env.string(&authority(env, activity))), Value::int(0)]).object();
    PROVIDED.set(Some(!found.is_null()));
    if found.is_null() {
        log("taut: the package has no provider taut.Files (see examples/apk.rs): its files are not handed out.");
    }
    !found.is_null()
}

/// The provider's authority: `<package>.taut.files`.
fn authority(env: Env, activity: Obj) -> String {
    let package = env.text(env.call(activity, c"getPackageName", c"()Ljava/lang/String;", &[]).object()).unwrap_or_default();
    format!("{package}.taut.files")
}

/// The address another app reads one of the app's files by: the provider's (none if the
/// package has no provider).
pub(crate) fn address_of(env: Env, activity: Obj, file: &Path) -> Obj {
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if !provided(env, activity) {
        return std::ptr::null_mut();
    }
    let builder = env.new_named(c"android/net/Uri$Builder", c"()V", &[]);
    for (name, value) in [(c"scheme", "content".to_string()), (c"authority", authority(env, activity)), (c"path", file.to_string_lossy().into_owned())] {
        env.call(builder, name, c"(Ljava/lang/String;)Landroid/net/Uri$Builder;", &[Value::obj(env.string(&value))]);
    }
    env.call(builder, c"build", c"()Landroid/net/Uri;", &[]).object()
}

/// A file's type by its ending, as the system knows it (`application/octet-stream` if not).
pub(crate) fn type_of(env: Env, file: &Path) -> String {
    let ending = file.extension().map(|ending| ending.to_string_lossy().to_lowercase()).unwrap_or_default();
    let types = env.call_static(c"android/webkit/MimeTypeMap", c"getSingleton", c"()Landroid/webkit/MimeTypeMap;", &[]).object();
    let found = env.call(types, c"getMimeTypeFromExtension", c"(Ljava/lang/String;)Ljava/lang/String;", &[Value::obj(env.string(&ending))]).object();
    env.text(found).unwrap_or_else(|| "application/octet-stream".into())
}

/// Opens one of the app's files with the app the system opens its type with, from any thread:
/// says whether one did.
pub(crate) fn open_file(file: &Path) -> bool {
    let activity = last_activity();
    let Some(env) = jni::thread_env().filter(|_| !activity.is_null()) else { return false };
    env.frame(|| {
        let address = address_of(env, activity, file);
        let intent = env.new_named(c"android/content/Intent", c"(Ljava/lang/String;)V", &[Value::obj(env.string("android.intent.action.VIEW"))]);
        let kind = type_of(env, file);
        env.call(intent, c"setDataAndType", c"(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;", &[Value::obj(address), Value::obj(env.string(&kind))]);
        env.call(intent, c"addFlags", c"(I)Landroid/content/Intent;", &[Value::int(GRANT_READ)]);
        !address.is_null() && jni::start_new_task(env, activity, intent)
    })
}
