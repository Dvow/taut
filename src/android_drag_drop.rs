//! Drag and drop on Android, through the system's drags (`View.startDragAndDrop`): files and
//! text dropped on the window by other apps, and what the app drags out of the window. The
//! library's view listens for the system's drag events (`taut.Drops`), and a drag out shows a
//! shadow of its own (`taut.Shadow`); both are the package's classes (see `examples/apk.rs`),
//! given their natives here the first time an app asks about drops or drags something out.
//!
//! Dropped files are documents of the app that dragged them: they are copied in
//! (`android_files`), and the window hears the drop once they are. A drag out leaves the window
//! as the finger does (past its edge, or into another app's window beside it, where Android
//! shows two); while the system drags it, the window hears it as the pointer when it is over
//! the window, so dropped back there it drops as any drag inside does.
use crate::android::{Heard, Part, activity, add_part, heard, kick, set_dragging_out, view};
use crate::android_clipboard as clipboard;
use crate::android_files::{self as files, Copying};
use crate::android_jni::{self as jni, Env, Native, Obj, Value};
use crate::android_sys::log;
use crate::drag_drop::{DragData, DropEffect};
use crate::event::{Button, Event};
use crate::geom::{Point, pt};
use crate::ui::Ui;
use std::cell::RefCell;
use std::ffi::c_void;

/// `DragEvent` actions.
const STARTED: i32 = 1;
const LOCATION: i32 = 2;
const DROP: i32 = 3;
const ENDED: i32 = 4;
const ENTERED: i32 = 5;
const EXITED: i32 = 6;
/// `View.DRAG_FLAG_GLOBAL | DRAG_FLAG_GLOBAL_URI_READ`: to other apps, which may read files.
const TO_OTHER_APPS: i32 = 0x100 | 0x1;
/// The largest side of a shadow, in pixels.
const SHADOW: usize = 160;

/// What the window does with drags.
struct Drags {
    /// The listener (a global reference), and the view it listens to.
    listener: Obj,
    on: Obj,
    /// What the app takes dropped: files, text.
    takes: (bool, bool),
    /// A drag over the window brings files (not text).
    files_over: bool,
    /// The app's own drag is out, and whether it was dropped back on the window.
    out: bool,
    dropped_back: bool,
    /// Files dropped, being copied, and where they were dropped.
    copying: Option<(Copying, Point)>,
    /// The shadow of the drag out: its width and pixels.
    shadow: (usize, Vec<u32>),
}

thread_local! {
    static DRAGS: RefCell<Option<Drags>> = const { RefCell::new(None) };
}

/// Runs `work` on what the window does with drags, once it does any.
fn with_drags<T>(work: impl FnOnce(&mut Drags) -> T) -> Option<T> {
    DRAGS.with_borrow_mut(|drags| drags.as_mut().map(work))
}

/// Has the window take drops (and drag out): from the next run on.
pub(crate) fn hand_drops() {
    if DRAGS.with_borrow(Option::is_some) {
        return;
    }
    let (Some(env), activity) = (jni::env(), activity()) else { return };
    let Some(listener) = env.frame(|| listener(env, activity)) else {
        return log("taut: drops and drags out need the package's class taut.Drops (see examples/apk.rs).");
    };
    let drags = Drags { listener, on: std::ptr::null_mut(), takes: (false, false), files_over: false, out: false, dropped_back: false, copying: None, shadow: (0, Vec::new()) };
    DRAGS.set(Some(drags));
    add_part(Part { name: "drags", before: take_copies, after: after_run });
}

/// The listener, its class and the shadow's given their natives: a global reference.
fn listener(env: Env, activity: Obj) -> Option<Obj> {
    let drops = jni::app_class(env, activity, "taut.Drops");
    let shadow = jni::app_class(env, activity, "taut.Shadow");
    let shadows = [
        Native::new(c"onProvideShadowMetrics", c"(Landroid/graphics/Point;Landroid/graphics/Point;)V", shadow_size as *const c_void),
        Native::new(c"onDrawShadow", c"(Landroid/graphics/Canvas;)V", draw_shadow as *const c_void),
    ];
    let listens = env.register(drops, &[Native::new(c"onDrag", c"(Landroid/view/View;Landroid/view/DragEvent;)Z", on_drag as *const c_void)]);
    (listens && env.register(shadow, &shadows)).then(|| env.global(env.new_object(drops, c"()V", &[])))
}

/// After a run: the listener listens to the window's view (a new one too), what the app takes
/// is noted, and what it just dragged out goes to the system.
fn after_run(ui: &mut Ui) {
    let now = view().0;
    let listener = DRAGS.with_borrow_mut(|drags| {
        let drags = drags.as_mut()?;
        drags.takes = (ui.takes_files.get(), ui.crossing.takes_text.get());
        (drags.on != now).then(|| {
            drags.on = now;
            drags.listener
        })
    });
    let env = jni::env();
    if let Some((env, listener)) = env.zip(listener) {
        env.call(now, c"setOnDragListener", c"(Landroid/view/View$OnDragListener;)V", &[Value::obj(listener)]);
    }
    let accent = ui.theme.accent.0;
    let Some(what) = ui.take_drag_out() else { return };
    let started = env.is_some_and(|env| env.frame(|| drag_out(env, now, what, accent)));
    if !started {
        ui.set_drop_effect(None);
    }
}

/// Starts the system's drag of what was dragged out of the window: says whether it started.
fn drag_out(env: Env, view: Obj, what: DragData, accent: u32) -> bool {
    let activity = activity();
    let clip = clip_of(env, activity, what);
    let shadow = jni::app_class(env, activity, "taut.Shadow");
    let shadow = env.new_object(shadow, c"()V", &[]);
    with_drags(|drags| drags.shadow = shadow_of(what, accent));
    let args = [Value::obj(clip), Value::obj(shadow), Value::obj(std::ptr::null_mut()), Value::int(TO_OTHER_APPS)];
    let started = !clip.is_null() && env.call(view, c"startDragAndDrop", c"(Landroid/content/ClipData;Landroid/view/View$DragShadowBuilder;Ljava/lang/Object;I)Z", &args).as_bool();
    with_drags(|drags| (drags.out, drags.dropped_back) = (started, false));
    set_dragging_out(started);
    started
}

/// What goes out, as a clip: files and a picture by their addresses, text as text, a link as
/// an address.
fn clip_of(env: Env, activity: Obj, what: DragData) -> Obj {
    let label = Value::obj(env.string("taut"));
    let made = |method: &std::ffi::CStr, signature: &std::ffi::CStr, args: &[Value]| env.call_static(c"android/content/ClipData", method, signature, args).object();
    match what {
        DragData::Text(text) => made(c"newPlainText", c"(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;", &[label, Value::obj(env.string(text))]),
        DragData::Link(link) => {
            let address = env.call_static(c"android/net/Uri", c"parse", c"(Ljava/lang/String;)Landroid/net/Uri;", &[Value::obj(env.string(link))]).object();
            made(c"newRawUri", c"(Ljava/lang/CharSequence;Landroid/net/Uri;)Landroid/content/ClipData;", &[label, Value::obj(address)])
        }
        DragData::Files(list) => clip_of_files(env, activity, list),
        DragData::Picture(width, pixels) => {
            let file = clipboard::picture_file().filter(|file| crate::png::encode_rgba(width, pixels).is_some_and(|png| std::fs::write(file, png).is_ok()));
            file.map_or(std::ptr::null_mut(), |file| clip_of_files(env, activity, &[file]))
        }
    }
}

/// A clip of files, by their addresses (null if none can be handed out).
fn clip_of_files(env: Env, activity: Obj, list: &[std::path::PathBuf]) -> Obj {
    let addresses: Vec<Obj> = list.iter().map(|file| files::address_of(env, activity, file)).filter(|address| !address.is_null()).collect();
    clipboard::clip_of(env, activity, "files", &addresses)
}

/// The shadow of what goes out: a picture as itself, made small; anything else a square of
/// the theme's accent colour, half see-through.
fn shadow_of(what: DragData, accent: u32) -> (usize, Vec<u32>) {
    let DragData::Picture(width, pixels) = what else { return (SHADOW / 2, vec![0x8000_0000 | accent & 0xff_ffff; SHADOW * SHADOW / 4]) };
    let height = pixels.len() / width.max(1);
    let step = width.max(height).div_ceil(SHADOW).max(1);
    let (small_width, small_height) = (width.div_ceil(step), height.div_ceil(step));
    let small = (0..small_height).flat_map(|y| (0..small_width).map(move |x| pixels[y * step * width + x * step])).collect();
    (small_width, small)
}

/// The shadow's size, and where the finger holds it: its middle.
extern "C" fn shadow_size(env: Env, _: Obj, size: Obj, finger: Obj) {
    let (width, height) = DRAGS.with_borrow(|drags| drags.as_ref().map_or((1, 1), |drags| (drags.shadow.0, drags.shadow.1.len() / drags.shadow.0.max(1))));
    let set = |point: Obj, x: usize, y: usize| env.call(point, c"set", c"(II)V", &[Value::int(x as i32), Value::int(y as i32)]);
    set(size, width.max(1), height.max(1));
    set(finger, width / 2, height / 2);
}

/// Draws the shadow.
extern "C" fn draw_shadow(env: Env, _: Obj, canvas: Obj) {
    let bitmap = DRAGS.with_borrow(|drags| drags.as_ref().map(|drags| jni::bitmap(env, drags.shadow.0, &drags.shadow.1)));
    let args = [Value::obj(bitmap.unwrap_or(std::ptr::null_mut())), Value::float(0.0), Value::float(0.0), Value::obj(std::ptr::null_mut())];
    env.call(canvas, c"drawBitmap", c"(Landroid/graphics/Bitmap;FFLandroid/graphics/Paint;)V", &args);
}

/// Where a drag event is, in units of the interface.
fn place(env: Env, event: Obj) -> Point {
    let (_, scale, insets) = view();
    let [x, y] = [c"getX", c"getY"].map(|name| env.call(event, name, c"()F", &[]).as_float());
    pt((x - insets[0] as f32) / scale, (y - insets[1] as f32) / scale)
}

/// What a drag offers: files (anything but text), text.
fn offers(env: Env, event: Obj) -> (bool, bool) {
    let description = env.call(event, c"getClipDescription", c"()Landroid/content/ClipDescription;", &[]).object();
    let count = env.call(description, c"getMimeTypeCount", c"()I", &[]).as_int();
    let kinds: Vec<String> = (0..count).filter_map(|at| env.text(env.call(description, c"getMimeType", c"(I)Ljava/lang/String;", &[Value::int(at)]).object())).collect();
    (kinds.iter().any(|kind| !kind.starts_with("text/")), kinds.iter().any(|kind| kind.starts_with("text/")))
}

/// Has the interface hear `event`: taken.
fn hear(event: Event) -> bool {
    heard(Heard::Event(event));
    true
}

/// The event of a drag of files or text coming over the window (`true`) or leaving it.
fn hovered(files: bool, over: bool) -> Event {
    if files { Event::FilesHovered(over) } else { Event::TextHovered(over) }
}

/// The system's drag event, on the view: says whether the window takes the drag (at its start)
/// or the drop.
extern "C" fn on_drag(env: Env, _: Obj, _: Obj, event: Obj) -> u8 {
    let action = env.call(event, c"getAction", c"()I", &[]).as_int();
    let Some((takes, files_over, out)) = DRAGS.with_borrow(|drags| drags.as_ref().map(|drags| (drags.takes, drags.files_over, drags.out))) else { return 0 };
    let taken = match action {
        STARTED => started(env, event, takes, out),
        ENTERED | EXITED if out => true,
        ENTERED => hear(hovered(files_over, true)),
        EXITED => hear(hovered(files_over, false)) && hear(Event::Leave),
        LOCATION => hear(Event::Move(place(env, event))),
        DROP if out => dropped_back(env, event),
        DROP => env.frame(|| dropped(env, event, takes, files_over)),
        ENDED if out => ended(env, event),
        _ => true,
    };
    kick();
    u8::from(taken)
}

/// A drag started somewhere: the window takes it if it is its own, or offers what the app
/// takes. (A text file's type is text: what comes over is files unless it looks like text the
/// app takes; the drop says which it was.)
fn started(env: Env, event: Obj, (files, text): (bool, bool), out: bool) -> bool {
    let (has_files, has_text) = offers(env, event);
    let files_over = !out && files && (has_files || !text);
    with_drags(|drags| drags.files_over = files_over);
    out || files && (has_files || has_text) || text && has_text
}

/// The app's own drag, dropped back on the window: the pointer comes up there.
fn dropped_back(env: Env, event: Obj) -> bool {
    with_drags(|drags| drags.dropped_back = true);
    heard(Heard::Event(Event::Move(place(env, event))));
    heard(Heard::Event(Event::Release(Button::Left)));
    true
}

/// Another app's drag dropped on the window: files (items with addresses) once copied in,
/// text at once.
fn dropped(env: Env, event: Obj, (files, text): (bool, bool), files_over: bool) -> bool {
    let (at, activity) = (place(env, event), activity());
    heard(Heard::Event(hovered(files_over, false)));
    let clip = env.call(event, c"getClipData", c"()Landroid/content/ClipData;", &[]).object();
    let addresses = clipboard::addresses(env, clip);
    if files && !addresses.is_empty() {
        // (The dragging app's documents may be read for as long as the activity lives.)
        env.call(activity, c"requestDragAndDropPermissions", c"(Landroid/view/DragEvent;)Landroid/view/DragAndDropPermissions;", &[Value::obj(event)]);
        let opened = addresses.into_iter().filter_map(|address| files::open_document(env, activity, address)).collect();
        with_drags(|drags| drags.copying = Some((files::copy_later(opened), at)));
        return true;
    }
    let item = env.call(clip, c"getItemAt", c"(I)Landroid/content/ClipData$Item;", &[Value::int(0)]).object();
    let dropped = env.text(env.call(item, c"coerceToText", c"(Landroid/content/Context;)Ljava/lang/CharSequence;", &[Value::obj(activity)]).object());
    text && hear(Event::Move(at)) && hear(Event::TextDropped(dropped.unwrap_or_default()))
}

/// The app's own drag ended: its box hears whether another app took it; the pointer comes up
/// where it is, unless it came up on the window already.
fn ended(env: Env, event: Obj) -> bool {
    let taken = env.call(event, c"getResult", c"()Z", &[]).as_bool();
    let back = DRAGS.with_borrow_mut(|drags| drags.as_mut().is_some_and(|drags| std::mem::replace(&mut drags.out, false) && drags.dropped_back));
    set_dragging_out(false);
    let effect = (taken && !back).then_some(DropEffect::Copied);
    heard(Heard::Other(Box::new(move |ui: &mut Ui| {
        ui.set_drop_effect(effect);
        true
    })));
    if !back {
        heard(Heard::Event(Event::Release(Button::Left)));
        heard(Heard::Event(Event::Leave));
    }
    true
}

/// Before a run: dropped files, once copied in, are dropped where they were let go.
fn take_copies(ui: &mut Ui) -> bool {
    let done = DRAGS.with_borrow_mut(|drags| {
        let drags = drags.as_mut()?;
        let copies = files::copied(&drags.copying.as_ref()?.0)?;
        drags.copying.take().map(|(_, at)| (copies, at))
    });
    let Some((copies, at)) = done.filter(|(copies, _)| !copies.is_empty()) else { return false };
    ui.event(Event::Move(at));
    ui.event(Event::FilesDropped(copies));
    true
}
