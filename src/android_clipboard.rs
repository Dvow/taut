//! The clipboard's pictures and files on Android: clip items with content addresses. A picture
//! goes on it as a PNG file of the app's, by its provider address (`android_files`); files by
//! theirs. Off it, a picture is the first item whose type is an image, read and decoded; files
//! are the items' documents, copied in. Handed to the clipboard the first time an app copies or
//! asks for either (`platform::hand_pictures_and_files`).
use crate::android::{SystemClipboard, activity};
use crate::android_files as files;
use crate::android_jni::{self as jni, Env, Obj, Value};
use crate::android_sys::log;
use std::path::PathBuf;

/// The clip on the clipboard now (null for none).
fn clip(env: Env, activity: Obj) -> Obj {
    env.call(jni::service(env, activity, "clipboard"), c"getPrimaryClip", c"()Landroid/content/ClipData;", &[]).object()
}

/// The content addresses of a clip's items.
pub(crate) fn addresses(env: Env, clip: Obj) -> Vec<Obj> {
    let count = env.call(clip, c"getItemCount", c"()I", &[]).as_int();
    let item = |at: i32| env.call(clip, c"getItemAt", c"(I)Landroid/content/ClipData$Item;", &[Value::int(at)]).object();
    (0..count).map(|at| env.call(item(at), c"getUri", c"()Landroid/net/Uri;", &[]).object()).filter(|address| !address.is_null()).collect()
}

/// The type of what an address names, as its provider says.
fn type_at(env: Env, activity: Obj, address: Obj) -> String {
    env.text(env.call(files::resolver(env, activity), c"getType", c"(Landroid/net/Uri;)Ljava/lang/String;", &[Value::obj(address)]).object()).unwrap_or_default()
}

/// A clip of these addresses, labelled `label` (null for none).
pub(crate) fn clip_of(env: Env, activity: Obj, label: &str, addresses: &[Obj]) -> Obj {
    let Some((first, rest)) = addresses.split_first() else { return std::ptr::null_mut() };
    let resolver = files::resolver(env, activity);
    let args = [Value::obj(resolver), Value::obj(env.string(label)), Value::obj(*first)];
    let clip = env.call_static(c"android/content/ClipData", c"newUri", c"(Landroid/content/ContentResolver;Ljava/lang/CharSequence;Landroid/net/Uri;)Landroid/content/ClipData;", &args).object();
    for address in rest {
        let item = env.new_named(c"android/content/ClipData$Item", c"(Landroid/net/Uri;)V", &[Value::obj(*address)]);
        env.call(clip, c"addItem", c"(Landroid/content/ContentResolver;Landroid/content/ClipData$Item;)V", &[Value::obj(resolver), Value::obj(item)]);
    }
    clip
}

/// Puts a clip of these addresses on the clipboard, labelled `label`.
fn put(env: Env, activity: Obj, label: &str, addresses: &[Obj]) {
    let clip = clip_of(env, activity, label, addresses);
    if !clip.is_null() {
        env.call(jni::service(env, activity, "clipboard"), c"setPrimaryClip", c"(Landroid/content/ClipData;)V", &[Value::obj(clip)]);
    }
}

impl SystemClipboard {
    /// The first picture on the clipboard: an item whose type is an image the library reads.
    pub(crate) fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        let (env, activity) = (jni::env()?, activity());
        env.frame(|| {
            let pictures = addresses(env, clip(env, activity)).into_iter().filter(|address| type_at(env, activity, *address).starts_with("image/"));
            pictures.filter_map(|address| files::read(env, activity, address)).find_map(|bytes| crate::ui::read_picture(&bytes))
        })
    }

    /// Puts a picture on the clipboard, as a PNG file of the app's.
    pub(crate) fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some((env, png)) = jni::env().zip(crate::png::encode_rgba(width, pixels)) else { return };
        let Some(file) = picture_file().filter(|file| std::fs::write(file, png).is_ok()) else {
            return log("taut: no room for the copied picture.");
        };
        let activity = activity();
        env.frame(|| {
            let address = files::address_of(env, activity, &file);
            if !address.is_null() {
                put(env, activity, "picture", &[address]);
            }
        });
    }

    /// The files on the clipboard: each item's document, copied in.
    pub(crate) fn files_on(&mut self) -> Vec<PathBuf> {
        let Some(env) = jni::env() else { return Vec::new() };
        let activity = activity();
        let opened = env.frame(|| addresses(env, clip(env, activity)).into_iter().filter_map(|address| files::open_document(env, activity, address)).collect());
        files::copy(opened)
    }

    /// Puts files on the clipboard, by their provider addresses.
    pub(crate) fn put_files(&mut self, list: &[PathBuf]) {
        let Some(env) = jni::env() else { return };
        let activity = activity();
        env.frame(|| {
            let addresses: Vec<Obj> = list.iter().map(|file| files::address_of(env, activity, file)).filter(|address| !address.is_null()).collect();
            put(env, activity, "files", &addresses);
        });
    }
}

/// A new file for a copied picture: each its own, for an app that read an earlier one may
/// still be reading it.
pub(crate) fn picture_file() -> Option<PathBuf> {
    static COPIED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let folder = std::env::temp_dir().join("clipboard").join(COPIED.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string());
    std::fs::create_dir_all(&folder).ok()?;
    Some(folder.join("picture.png"))
}
