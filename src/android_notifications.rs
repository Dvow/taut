//! System notifications on Android: the `NotificationManager`, on a channel of the library's.
//! The first notification asks the person's permission where Android 13 and later want it
//! (`POST_NOTIFICATIONS`): notifications wait until the answer, and go if it is no. A click
//! brings the activity forward with an intent naming the notification; the activity
//! (`taut.Activity`, see `examples/apk.rs`) takes it as its own, and the next run hears which
//! was clicked. (Started by the click after the system ended the app, its first run hears it.)
use crate::android::{Part, activity, add_part};
use crate::android_jni::{self as jni, Env, Obj, Value};
use crate::android_sys::log;
use crate::notifications::{NOTICES, Out};
use crate::ui::Ui;
use std::cell::{Cell, RefCell};

/// The library's channel.
const CHANNEL: &str = "taut";
/// The permission Android 13 and later ask for, and the request code it is asked with.
const PERMISSION: &str = "android.permission.POST_NOTIFICATIONS";
const ASKING: i32 = 0x7443;
/// `PackageManager.PERMISSION_GRANTED`; `NotificationManager.IMPORTANCE_DEFAULT`.
const GRANTED: i32 = 0;
const DEFAULT_IMPORTANCE: i32 = 3;
/// The extras a click's intent carries: the notification's number, and when it was posted
/// (what tells one click from the same intent seen again).
const NUMBER: &str = "taut.notification";
const POSTED: &str = "taut.posted";
/// `Intent.FLAG_ACTIVITY_SINGLE_TOP`; `PendingIntent.FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT`.
const SINGLE_TOP: i32 = 0x2000_0000;
const PENDING_FLAGS: i32 = 0x0400_0000 | 0x0800_0000;

thread_local! {
    /// Notifications waiting for the person's permission.
    static WAITING: RefCell<Vec<Out>> = const { RefCell::new(Vec::new()) };
    /// The permission was asked for, and no answer has come.
    static ASKED: Cell<bool> = const { Cell::new(false) };
    /// The clicks heard so far, by when their notification was posted.
    static HEARD: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

/// Has the windows post notifications, from the next run on.
pub(crate) fn hand_desktop() {
    add_part(Part { name: "notifications", before: take_permission, after: post_new });
}

/// After a run: the notifications it asked for are posted, or wait for the permission.
fn post_new(_: &mut Ui) {
    let out = NOTICES.with_borrow_mut(|notices| std::mem::take(&mut notices.out));
    let (env, activity) = (jni::env(), activity());
    let Some(env) = env.filter(|_| !out.is_empty() && !activity.is_null()) else { return };
    if !permitted(env, activity) {
        WAITING.with_borrow_mut(|waiting| waiting.extend(out));
        return ask_permission(env, activity);
    }
    out.iter().for_each(|one| env.frame(|| post(env, activity, one)));
}

/// Whether the app may post notifications (always before Android 13, which has no such
/// permission).
fn permitted(env: Env, activity: Obj) -> bool {
    let asks = env.static_int(c"android/os/Build$VERSION", c"SDK_INT") >= 33;
    !asks || env.call(activity, c"checkSelfPermission", c"(Ljava/lang/String;)I", &[Value::obj(env.string(PERMISSION))]).as_int() == GRANTED
}

/// Asks the person's permission, once until answered: the activity keeps the answer.
fn ask_permission(env: Env, activity: Obj) {
    if ASKED.replace(true) {
        return;
    }
    if !jni::is_app_activity(env, activity) {
        log("taut: asking to post notifications needs the activity taut.Activity (see examples/apk.rs).");
        WAITING.with_borrow_mut(Vec::clear);
        return ASKED.set(false);
    }
    env.frame(|| {
        env.set_int_field(activity, c"permitted", 0);
        let asked = jni::new_objects(env, c"java/lang/String", &[env.string(PERMISSION)]);
        env.call(activity, c"requestPermissions", c"([Ljava/lang/String;I)V", &[Value::obj(asked), Value::int(ASKING)]);
    });
}

/// Before a run: once the person answered, the waiting notifications go out if allowed, else
/// they are dropped.
fn take_permission(_: &mut Ui) -> bool {
    let (env, activity) = (jni::env(), activity());
    let Some(env) = env.filter(|_| ASKED.get() && !activity.is_null()) else { return false };
    if env.int_field(activity, c"permitted") != ASKING {
        return false;
    }
    env.set_int_field(activity, c"permitted", 0);
    ASKED.set(false);
    let waiting = WAITING.with_borrow_mut(std::mem::take);
    if permitted(env, activity) {
        waiting.iter().for_each(|one| env.frame(|| post(env, activity, one)));
    } else {
        log(&format!("taut: the person did not allow notifications: {} dropped.", waiting.len()));
    }
    false
}

/// The library's channel, made (or found) under the app's name.
fn channel(env: Env, manager: Obj, activity: Obj) {
    let packages = env.call(activity, c"getPackageManager", c"()Landroid/content/pm/PackageManager;", &[]).object();
    let app = env.call(activity, c"getApplicationInfo", c"()Landroid/content/pm/ApplicationInfo;", &[]).object();
    let label = env.call(app, c"loadLabel", c"(Landroid/content/pm/PackageManager;)Ljava/lang/CharSequence;", &[Value::obj(packages)]).object();
    let made =
        env.new_named(c"android/app/NotificationChannel", c"(Ljava/lang/String;Ljava/lang/CharSequence;I)V", &[Value::obj(env.string(CHANNEL)), Value::obj(label), Value::int(DEFAULT_IMPORTANCE)]);
    env.call(manager, c"createNotificationChannel", c"(Landroid/app/NotificationChannel;)V", &[Value::obj(made)]);
}

/// Posts a notification: its title, text and picture; its click brings the activity forward.
fn post(env: Env, activity: Obj, one: &Out) {
    let manager = jni::service(env, activity, "notification");
    channel(env, manager, activity);
    let builder = env.new_named(c"android/app/Notification$Builder", c"(Landroid/content/Context;Ljava/lang/String;)V", &[Value::obj(activity), Value::obj(env.string(CHANNEL))]);
    let set_text = |name: &std::ffi::CStr, text: &str| env.call(builder, name, c"(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;", &[Value::obj(env.string(text))]);
    set_text(c"setContentTitle", &one.title);
    set_text(c"setContentText", &one.text);
    let icon = env.static_int(c"android/R$drawable", c"ic_dialog_info");
    let small = env
        .call_static(c"android/graphics/drawable/Icon", c"createWithResource", c"(Ljava/lang/String;I)Landroid/graphics/drawable/Icon;", &[Value::obj(env.string("android")), Value::int(icon)])
        .object();
    env.call(builder, c"setSmallIcon", c"(Landroid/graphics/drawable/Icon;)Landroid/app/Notification$Builder;", &[Value::obj(small)]);
    if let Some((width, pixels)) = &one.picture {
        let large = env
            .call_static(c"android/graphics/drawable/Icon", c"createWithBitmap", c"(Landroid/graphics/Bitmap;)Landroid/graphics/drawable/Icon;", &[Value::obj(jni::bitmap(env, *width, pixels))])
            .object();
        env.call(builder, c"setLargeIcon", c"(Landroid/graphics/drawable/Icon;)Landroid/app/Notification$Builder;", &[Value::obj(large)]);
    }
    env.call(builder, c"setAutoCancel", c"(Z)Landroid/app/Notification$Builder;", &[Value::bool(true)]);
    env.call(builder, c"setContentIntent", c"(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;", &[Value::obj(on_click(env, activity, one.number))]);
    let notification = env.call(builder, c"build", c"()Landroid/app/Notification;", &[]).object();
    env.call(manager, c"notify", c"(ILandroid/app/Notification;)V", &[Value::int(one.number as i32), Value::obj(notification)]);
}

/// What a click does: brings the activity forward (the same one, if it is in front of its
/// task) with an intent naming the notification.
fn on_click(env: Env, activity: Obj, number: u32) -> Obj {
    let class = env.call(activity, c"getClass", c"()Ljava/lang/Class;", &[]).object();
    let intent = env.new_named(c"android/content/Intent", c"(Landroid/content/Context;Ljava/lang/Class;)V", &[Value::obj(activity), Value::obj(class)]);
    env.call(intent, c"putExtra", c"(Ljava/lang/String;I)Landroid/content/Intent;", &[Value::obj(env.string(NUMBER)), Value::int(number as i32)]);
    let posted = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos() as i64);
    env.call(intent, c"putExtra", c"(Ljava/lang/String;J)Landroid/content/Intent;", &[Value::obj(env.string(POSTED)), Value::long(posted)]);
    env.call(intent, c"addFlags", c"(I)Landroid/content/Intent;", &[Value::int(jni::NEW_TASK | SINGLE_TOP)]);
    let args = [Value::obj(activity), Value::int(number as i32), Value::obj(intent), Value::int(PENDING_FLAGS)];
    env.call_static(c"android/app/PendingIntent", c"getActivity", c"(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;", &args).object()
}

/// The notification the activity's intent says was clicked, once: then it is with those
/// `notification_clicked` gives.
pub(crate) fn hear_click() {
    let (env, activity) = (jni::env(), activity());
    let Some(env) = env.filter(|_| !activity.is_null()) else { return };
    let clicked = env.frame(|| {
        let intent = env.call(activity, c"getIntent", c"()Landroid/content/Intent;", &[]).object();
        let number = env.call(intent, c"getIntExtra", c"(Ljava/lang/String;I)I", &[Value::obj(env.string(NUMBER)), Value::int(0)]).as_int();
        let posted = env.call(intent, c"getLongExtra", c"(Ljava/lang/String;J)J", &[Value::obj(env.string(POSTED)), Value::long(0)]).as_long();
        (number > 0).then_some((number as u32, posted))
    });
    let Some((number, posted)) = clicked.filter(|(_, posted)| !HEARD.with_borrow(|heard| heard.contains(posted))) else { return };
    HEARD.with_borrow_mut(|heard| heard.push(posted));
    NOTICES.with_borrow_mut(|notices| notices.clicked.push(number));
}
