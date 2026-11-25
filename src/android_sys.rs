//! What the library takes from Android's NDK (`libandroid`, `liblog`) and from the C library:
//! the native activity, its window, input and looper. As declared in `android/*.h`.
use std::ffi::{c_char, c_int, c_void};

/// The activity, as the system hands it to native code (`ANativeActivity`).
#[repr(C)]
pub(crate) struct Activity {
    pub callbacks: *mut Callbacks,
    pub vm: *mut c_void,
    pub env: *mut *const *const c_void,
    /// The `NativeActivity` Java object.
    pub clazz: *mut c_void,
    pub internal_data_path: *const c_char,
    pub external_data_path: *const c_char,
    pub sdk_version: i32,
    pub instance: *mut c_void,
    pub asset_manager: *mut c_void,
    pub obb_path: *const c_char,
}

/// What the activity tells native code (`ANativeActivityCallbacks`).
#[repr(C)]
pub(crate) struct Callbacks {
    pub on_start: Option<extern "C" fn(*mut Activity)>,
    pub on_resume: Option<extern "C" fn(*mut Activity)>,
    pub on_save_instance_state: Option<extern "C" fn(*mut Activity, *mut usize) -> *mut c_void>,
    pub on_pause: Option<extern "C" fn(*mut Activity)>,
    pub on_stop: Option<extern "C" fn(*mut Activity)>,
    pub on_destroy: Option<extern "C" fn(*mut Activity)>,
    pub on_window_focus_changed: Option<extern "C" fn(*mut Activity, c_int)>,
    pub on_native_window_created: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_native_window_resized: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_native_window_redraw_needed: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_native_window_destroyed: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_input_queue_created: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_input_queue_destroyed: Option<extern "C" fn(*mut Activity, *mut c_void)>,
    pub on_content_rect_changed: Option<extern "C" fn(*mut Activity, *const [i32; 4])>,
    pub on_configuration_changed: Option<extern "C" fn(*mut Activity)>,
    pub on_low_memory: Option<extern "C" fn(*mut Activity)>,
}

/// A locked window buffer (`ANativeWindow_Buffer`).
#[repr(C)]
pub(crate) struct WindowBuffer {
    pub width: i32,
    pub height: i32,
    /// Pixels a row.
    pub stride: i32,
    pub format: i32,
    pub bits: *mut c_void,
    pub reserved: [u32; 6],
}

/// Window pixel format: 8-bit red, green, blue and alpha, in that byte order.
pub(crate) const RGBA_8888: i32 = 1;

/// Input event kinds, and motion actions (`AINPUT_EVENT_TYPE_*`, `AMOTION_EVENT_ACTION_*`).
pub(crate) const KEY_EVENT: i32 = 1;
pub(crate) const MOTION_EVENT: i32 = 2;
pub(crate) const DOWN: i32 = 0;
pub(crate) const UP: i32 = 1;
pub(crate) const MOVE: i32 = 2;
pub(crate) const CANCEL: i32 = 3;
pub(crate) const POINTER_DOWN: i32 = 5;
pub(crate) const POINTER_UP: i32 = 6;
pub(crate) const HOVER_MOVE: i32 = 7;
pub(crate) const SCROLL: i32 = 8;
pub(crate) const HOVER_ENTER: i32 = 9;
pub(crate) const HOVER_EXIT: i32 = 10;
/// Key actions, and repeats.
pub(crate) const KEY_DOWN: i32 = 0;
pub(crate) const KEY_UP: i32 = 1;
/// Tool kinds (`AMOTION_EVENT_TOOL_TYPE_*`).
pub(crate) const FINGER: i32 = 1;
pub(crate) const STYLUS: i32 = 2;
pub(crate) const MOUSE: i32 = 3;
pub(crate) const ERASER: i32 = 4;
/// Mouse buttons held (`AMOTION_EVENT_BUTTON_*`).
pub(crate) const PRIMARY: i32 = 1;
pub(crate) const SECONDARY: i32 = 2;
pub(crate) const TERTIARY: i32 = 4;
/// Axes: the scroll wheel's, a pen's pressure.
pub(crate) const AXIS_PRESSURE: i32 = 2;
pub(crate) const AXIS_VSCROLL: i32 = 9;
pub(crate) const AXIS_HSCROLL: i32 = 10;
/// Modifiers held (`AMETA_*`).
pub(crate) const META_SHIFT: i32 = 0x1;
pub(crate) const META_ALT: i32 = 0x2;
pub(crate) const META_CTRL: i32 = 0x1000;
/// The looper: run a callback, and its readable event.
pub(crate) const LOOPER_CALLBACK: c_int = -2;
pub(crate) const LOOPER_INPUT: c_int = 1;

/// A looper callback: file descriptor, events, its data. Answers 1 to go on being called.
pub(crate) type LooperCallback = extern "C" fn(c_int, c_int, *mut c_void) -> c_int;

#[link(name = "android")]
unsafe extern "C" {
    pub fn ANativeActivity_finish(activity: *mut Activity);
    pub fn ANativeWindow_acquire(window: *mut c_void);
    pub fn ANativeWindow_release(window: *mut c_void);
    pub fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    pub fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
    pub fn ANativeWindow_setBuffersGeometry(window: *mut c_void, width: i32, height: i32, format: i32) -> i32;
    pub fn ANativeWindow_lock(window: *mut c_void, buffer: *mut WindowBuffer, dirty: *mut [i32; 4]) -> i32;
    pub fn ANativeWindow_unlockAndPost(window: *mut c_void) -> i32;
    pub fn AInputQueue_attachLooper(queue: *mut c_void, looper: *mut c_void, ident: c_int, callback: LooperCallback, data: *mut c_void);
    pub fn AInputQueue_detachLooper(queue: *mut c_void);
    pub fn AInputQueue_getEvent(queue: *mut c_void, event: *mut *mut c_void) -> i32;
    pub fn AInputQueue_preDispatchEvent(queue: *mut c_void, event: *mut c_void) -> i32;
    pub fn AInputQueue_finishEvent(queue: *mut c_void, event: *mut c_void, handled: c_int);
    pub fn AInputEvent_getType(event: *const c_void) -> i32;
    pub fn AInputEvent_getDeviceId(event: *const c_void) -> i32;
    pub fn AKeyEvent_getAction(event: *const c_void) -> i32;
    pub fn AKeyEvent_getKeyCode(event: *const c_void) -> i32;
    pub fn AKeyEvent_getMetaState(event: *const c_void) -> i32;
    pub fn AMotionEvent_getAction(event: *const c_void) -> i32;
    pub fn AMotionEvent_getMetaState(event: *const c_void) -> i32;
    pub fn AMotionEvent_getButtonState(event: *const c_void) -> i32;
    pub fn AMotionEvent_getPointerCount(event: *const c_void) -> usize;
    pub fn AMotionEvent_getPointerId(event: *const c_void, index: usize) -> i32;
    pub fn AMotionEvent_getToolType(event: *const c_void, index: usize) -> i32;
    pub fn AMotionEvent_getX(event: *const c_void, index: usize) -> f32;
    pub fn AMotionEvent_getY(event: *const c_void, index: usize) -> f32;
    pub fn AMotionEvent_getAxisValue(event: *const c_void, axis: i32, index: usize) -> f32;
    pub fn ALooper_forThread() -> *mut c_void;
    pub fn ALooper_addFd(looper: *mut c_void, fd: c_int, ident: c_int, events: c_int, callback: LooperCallback, data: *mut c_void) -> c_int;
    pub fn AFontMatcher_create() -> *mut c_void;
    pub fn AFontMatcher_destroy(matcher: *mut c_void);
    pub fn AFontMatcher_setStyle(matcher: *mut c_void, weight: u16, italic: bool);
    pub fn AFontMatcher_match(matcher: *const c_void, family: *const c_char, text: *const u16, long: u32, run: *mut u32) -> *mut c_void;
    pub fn AFont_close(font: *mut c_void);
    pub fn AFont_getFontFilePath(font: *const c_void) -> *const c_char;
    pub fn AFont_getCollectionIndex(font: *const c_void) -> usize;
    pub fn AFont_getAxisCount(font: *const c_void) -> usize;
    pub fn AFont_getAxisTag(font: *const c_void, axis: u32) -> u32;
    pub fn AFont_getAxisValue(font: *const c_void, axis: u32) -> f32;
}

#[link(name = "log")]
unsafe extern "C" {
    pub fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// A time for `timerfd_settime`: an interval, and the first expiry.
#[repr(C)]
pub(crate) struct TimerSpec {
    pub interval: [i64; 2],
    pub value: [i64; 2],
}

unsafe extern "C" {
    pub fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
    pub fn timerfd_settime(fd: c_int, flags: c_int, new: *const TimerSpec, old: *mut TimerSpec) -> c_int;
    pub fn pipe2(ends: *mut [c_int; 2], flags: c_int) -> c_int;
    pub fn read(fd: c_int, into: *mut c_void, long: usize) -> isize;
    pub fn write(fd: c_int, from: *const c_void, long: usize) -> isize;
}

/// `CLOCK_MONOTONIC`, and file flags: non-blocking, close on exec.
pub(crate) const MONOTONIC: c_int = 1;
pub(crate) const NONBLOCK: c_int = 0o4000;
pub(crate) const CLOEXEC: c_int = 0o2000000;

/// Writes a line to the system log (logcat), under the tag `taut`.
pub(crate) fn log(text: &str) {
    let Ok(line) = std::ffi::CString::new(text.replace('\0', " ")) else { return };
    // SAFETY: both are C strings; 4 is "info".
    unsafe { __android_log_write(4, c"taut".as_ptr(), line.as_ptr()) };
}
