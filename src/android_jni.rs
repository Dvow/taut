//! Android's Java, called through JNI: the classes `android_dex` writes, loaded and given their
//! native methods; and the system services an app reaches only from Java (the clipboard, the
//! soft keyboard, window insets, the person's display settings, opening a link). Everything here
//! runs on the main thread, where the activity's own `JNIEnv` is, but `open_link`.
use crate::android_dex::{Class, dex};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};

/// A Java object, class, method or field: a reference the VM hands out.
pub(crate) type Obj = *mut c_void;

/// One argument or result: whichever its type is.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union Value {
    pub z: u8,
    pub i: i32,
    pub j: i64,
    pub f: f32,
    pub l: Obj,
}

impl Value {
    pub(crate) fn int(i: i32) -> Value {
        Value { j: i64::from(i) & 0xffff_ffff }
    }
    pub(crate) fn obj(l: Obj) -> Value {
        Value { l }
    }
    pub(crate) fn bool(on: bool) -> Value {
        Value { j: i64::from(on) }
    }
    pub(crate) fn float(f: f32) -> Value {
        Value { j: i64::from(f.to_bits()) }
    }
    /// As an object (null for none).
    pub(crate) fn object(self) -> Obj {
        // SAFETY: every field is plain data, and a value made from another type reads as null
        // or a number, never used as an object but by the caller who asked for one.
        unsafe { self.l }
    }
    pub(crate) fn as_int(self) -> i32 {
        // SAFETY: plain data.
        unsafe { self.i }
    }
    pub(crate) fn as_bool(self) -> bool {
        // SAFETY: plain data.
        unsafe { self.z != 0 }
    }
    pub(crate) fn as_float(self) -> f32 {
        // SAFETY: plain data.
        unsafe { self.f }
    }
}

/// A native method handed to Java: name, descriptor, the function.
#[repr(C)]
pub(crate) struct Native {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub function: *const c_void,
}

/// JNI's table: the slots used, numbered as in `jni.h`.
const FIND_CLASS: usize = 6;
const EXCEPTION_DESCRIBE: usize = 16;
const EXCEPTION_CLEAR: usize = 17;
const PUSH_LOCAL_FRAME: usize = 19;
const POP_LOCAL_FRAME: usize = 20;
const NEW_GLOBAL_REF: usize = 21;
const DELETE_LOCAL_REF: usize = 23;
const NEW_OBJECT_A: usize = 30;
const GET_OBJECT_CLASS: usize = 31;
const GET_METHOD_ID: usize = 33;
/// The first `Call<Type>MethodA`, for objects; the others follow, three slots apart, in the
/// order of `TYPES`. Statics likewise.
const CALL_OBJECT_METHOD_A: usize = 36;
const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
const TYPES: &[u8] = b"LZBCSIJFDV";
const GET_FIELD_ID: usize = 94;
const GET_INT_FIELD: usize = 100;
const GET_FLOAT_FIELD: usize = 102;
const SET_INT_FIELD: usize = 109;
const GET_STATIC_METHOD_ID: usize = 113;
const GET_STATIC_FIELD_ID: usize = 144;
const GET_STATIC_INT_FIELD: usize = 150;
const NEW_STRING: usize = 163;
const GET_STRING_LENGTH: usize = 164;
const NEW_INT_ARRAY: usize = 179;
const GET_INT_ARRAY_REGION: usize = 203;
const REGISTER_NATIVES: usize = 215;
const GET_STRING_REGION: usize = 220;
const EXCEPTION_CHECK: usize = 228;
const NEW_DIRECT_BYTE_BUFFER: usize = 229;
/// The VM's own table: attach and detach a thread.
const ATTACH_CURRENT_THREAD: usize = 4;
const DETACH_CURRENT_THREAD: usize = 5;

/// A thread's JNI interface (`JNIEnv*`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Env(pub *mut *const *const c_void);

thread_local! {
    /// The main thread's interface, from the activity.
    static MAIN: Cell<Env> = const { Cell::new(Env(std::ptr::null_mut())) };
}

/// The VM, for threads of the library's own.
static VM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Takes in the main thread's interface and the VM.
pub(crate) fn set_main(env: Env, vm: *mut c_void) {
    MAIN.set(env);
    VM.store(vm, Ordering::SeqCst);
}

/// The main thread's interface, once an activity started on it.
pub(crate) fn env() -> Option<Env> {
    Some(MAIN.get()).filter(|env| !env.0.is_null())
}

impl Env {
    /// A function from the table.
    ///
    /// # Safety
    /// `F` is the slot's function type.
    unsafe fn slot<F: Copy>(self, n: usize) -> F {
        // SAFETY: the interface points at its table, which has every slot of `jni.h`; the
        // caller names the slot's type.
        unsafe { std::mem::transmute_copy(&*(*self.0).add(n)) }
    }

    /// Whether Java threw: then it is printed to the log and cleared, as nothing here expects
    /// one, and the call it came from answers nothing.
    pub(crate) fn threw(self) -> bool {
        // SAFETY: plain calls of the interface on its thread.
        unsafe {
            let thrown = self.slot::<extern "C" fn(Env) -> u8>(EXCEPTION_CHECK)(self) != 0;
            if thrown {
                self.slot::<extern "C" fn(Env)>(EXCEPTION_DESCRIBE)(self);
                self.slot::<extern "C" fn(Env)>(EXCEPTION_CLEAR)(self);
            }
            thrown
        }
    }

    /// A class by name, with slashes (`android/content/ClipData`).
    pub(crate) fn class(self, name: &CStr) -> Obj {
        // SAFETY: the name is a C string.
        let class = unsafe { self.slot::<extern "C" fn(Env, *const c_char) -> Obj>(FIND_CLASS)(self, name.as_ptr()) };
        if self.threw() { std::ptr::null_mut() } else { class }
    }

    /// Calls `name` with `signature` on an object (none: nothing happens).
    pub(crate) fn call(self, on: Obj, name: &CStr, signature: &CStr, args: &[Value]) -> Value {
        if on.is_null() {
            return Value { j: 0 };
        }
        // SAFETY: the object is alive; its class answers the method, or Java throws (cleared).
        let method = unsafe {
            let class = self.slot::<extern "C" fn(Env, Obj) -> Obj>(GET_OBJECT_CLASS)(self, on);
            let method = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr());
            self.drop_local(class);
            method
        };
        if self.threw() || method.is_null() {
            return Value { j: 0 };
        }
        self.invoke(CALL_OBJECT_METHOD_A, on, method, signature, args)
    }

    /// Calls a static method of a class.
    pub(crate) fn call_static(self, class: &CStr, name: &CStr, signature: &CStr, args: &[Value]) -> Value {
        let class = self.class(class);
        if class.is_null() {
            return Value { j: 0 };
        }
        // SAFETY: as in `call`.
        let method = unsafe { self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_STATIC_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr()) };
        let value = if self.threw() || method.is_null() { Value { j: 0 } } else { self.invoke(CALL_STATIC_OBJECT_METHOD_A, class, method, signature, args) };
        self.drop_local(class);
        value
    }

    /// Calls a method by its id, through the `Call<Type>MethodA` slot its return type has.
    fn invoke(self, first: usize, on: Obj, method: Obj, signature: &CStr, args: &[Value]) -> Value {
        let returns = signature.to_bytes().iter().rposition(|byte| *byte == b')').and_then(|at| signature.to_bytes().get(at + 1)).copied().unwrap_or(b'V');
        let kind = TYPES.iter().position(|letter| *letter == if returns == b'[' { b'L' } else { returns }).unwrap_or(9);
        let slot = first + kind * 3;
        // SAFETY: the slot is the one for this return type; each is read as the C type it
        // returns, widened into a value.
        let value = unsafe {
            let ptr = args.as_ptr();
            match returns {
                b'V' => (self.slot::<extern "C" fn(Env, Obj, Obj, *const Value)>(slot)(self, on, method, ptr), Value { j: 0 }).1,
                b'Z' | b'B' => Value { j: i64::from(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> u8>(slot)(self, on, method, ptr)) },
                b'C' | b'S' => Value { j: i64::from(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> u16>(slot)(self, on, method, ptr)) },
                b'I' => Value::int(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> i32>(slot)(self, on, method, ptr)),
                b'J' => Value { j: self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> i64>(slot)(self, on, method, ptr) },
                b'F' => Value::float(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> f32>(slot)(self, on, method, ptr)),
                b'D' => Value::float(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> f64>(slot)(self, on, method, ptr) as f32),
                _ => Value::obj(self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> Obj>(slot)(self, on, method, ptr)),
            }
        };
        if self.threw() { Value { j: 0 } } else { value }
    }

    /// A new object of `class`, made by its constructor with `signature`.
    pub(crate) fn new_object(self, class: Obj, signature: &CStr, args: &[Value]) -> Obj {
        if class.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: the class is alive; its constructor is asked for by signature.
        let object = unsafe {
            let made = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, c"<init>".as_ptr(), signature.as_ptr());
            if made.is_null() { made } else { self.slot::<extern "C" fn(Env, Obj, Obj, *const Value) -> Obj>(NEW_OBJECT_A)(self, class, made, args.as_ptr()) }
        };
        if self.threw() { std::ptr::null_mut() } else { object }
    }

    /// A new object of the class named.
    pub(crate) fn new_named(self, class: &CStr, signature: &CStr, args: &[Value]) -> Obj {
        let class = self.class(class);
        let object = self.new_object(class, signature, args);
        self.drop_local(class);
        object
    }

    /// An instance field's id, or null.
    fn field(self, on: Obj, name: &CStr, kind: &CStr) -> Obj {
        // SAFETY: the object is alive.
        let field = unsafe {
            let class = self.slot::<extern "C" fn(Env, Obj) -> Obj>(GET_OBJECT_CLASS)(self, on);
            let field = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_FIELD_ID)(self, class, name.as_ptr(), kind.as_ptr());
            self.drop_local(class);
            field
        };
        if self.threw() { std::ptr::null_mut() } else { field }
    }

    /// An `int` field of an object (0 without one).
    pub(crate) fn int_field(self, on: Obj, name: &CStr) -> i32 {
        let field = if on.is_null() { std::ptr::null_mut() } else { self.field(on, name, c"I") };
        // SAFETY: the field is the object's, of type int.
        if field.is_null() { 0 } else { unsafe { self.slot::<extern "C" fn(Env, Obj, Obj) -> i32>(GET_INT_FIELD)(self, on, field) } }
    }

    /// A `float` field of an object (0 without one).
    pub(crate) fn float_field(self, on: Obj, name: &CStr) -> f32 {
        let field = if on.is_null() { std::ptr::null_mut() } else { self.field(on, name, c"F") };
        // SAFETY: the field is the object's, of type float.
        if field.is_null() { 0.0 } else { unsafe { self.slot::<extern "C" fn(Env, Obj, Obj) -> f32>(GET_FLOAT_FIELD)(self, on, field) } }
    }

    /// Sets an `int` field of an object.
    pub(crate) fn set_int_field(self, on: Obj, name: &CStr, value: i32) {
        let field = if on.is_null() { std::ptr::null_mut() } else { self.field(on, name, c"I") };
        if !field.is_null() {
            // SAFETY: the field is the object's, of type int.
            unsafe { self.slot::<extern "C" fn(Env, Obj, Obj, i32)>(SET_INT_FIELD)(self, on, field, value) };
        }
    }

    /// A static `int` field of a class (0 without one).
    pub(crate) fn static_int(self, class: &CStr, name: &CStr) -> i32 {
        let class = self.class(class);
        if class.is_null() {
            return 0;
        }
        // SAFETY: the class is alive; its field is asked for by name and type.
        let value = unsafe {
            let field = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_STATIC_FIELD_ID)(self, class, name.as_ptr(), c"I".as_ptr());
            if field.is_null() { 0 } else { self.slot::<extern "C" fn(Env, Obj, Obj) -> i32>(GET_STATIC_INT_FIELD)(self, class, field) }
        };
        let value = if self.threw() { 0 } else { value };
        self.drop_local(class);
        value
    }

    /// A Java string of `text`.
    pub(crate) fn string(self, text: &str) -> Obj {
        let units: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the units are read for their length.
        let made = unsafe { self.slot::<extern "C" fn(Env, *const u16, i32) -> Obj>(NEW_STRING)(self, units.as_ptr(), units.len() as i32) };
        if self.threw() { std::ptr::null_mut() } else { made }
    }

    /// The text of a `CharSequence` (a string, or anything with `toString`).
    pub(crate) fn text(self, of: Obj) -> Option<String> {
        let string = self.call(of, c"toString", c"()Ljava/lang/String;", &[]).object();
        if string.is_null() {
            return None;
        }
        // SAFETY: the string is alive; its length is asked first and as many units read.
        let units = unsafe {
            let long = self.slot::<extern "C" fn(Env, Obj) -> i32>(GET_STRING_LENGTH)(self, string).max(0) as usize;
            let mut units = vec![0u16; long];
            self.slot::<extern "C" fn(Env, Obj, i32, i32, *mut u16)>(GET_STRING_REGION)(self, string, 0, long as i32, units.as_mut_ptr());
            units
        };
        self.drop_local(string);
        Some(String::from_utf16_lossy(&units))
    }

    /// Keeps an object past the native call that has it (never let go: the library's few).
    pub(crate) fn global(self, of: Obj) -> Obj {
        // SAFETY: a reference the VM keeps until deleted.
        if of.is_null() { of } else { unsafe { self.slot::<extern "C" fn(Env, Obj) -> Obj>(NEW_GLOBAL_REF)(self, of) } }
    }

    /// Lets go of a local reference.
    pub(crate) fn drop_local(self, of: Obj) {
        if !of.is_null() {
            // SAFETY: a local reference of this thread, not used after.
            unsafe { self.slot::<extern "C" fn(Env, Obj)>(DELETE_LOCAL_REF)(self, of) };
        }
    }

    /// Runs `work` with room for local references, all let go after (but its answer, which
    /// must be no reference).
    pub(crate) fn frame<T>(self, work: impl FnOnce() -> T) -> T {
        // SAFETY: a frame pushed and popped around the work, on this thread.
        let pushed = unsafe { self.slot::<extern "C" fn(Env, i32) -> i32>(PUSH_LOCAL_FRAME)(self, 128) } == 0;
        let answer = work();
        if pushed {
            // SAFETY: as above.
            unsafe { self.slot::<extern "C" fn(Env, Obj) -> Obj>(POP_LOCAL_FRAME)(self, std::ptr::null_mut()) };
        } else {
            self.threw();
        }
        answer
    }

    /// Gives a class its native methods.
    pub(crate) fn register(self, class: Obj, natives: &[Native]) -> bool {
        // SAFETY: the names and signatures are C strings that live for ever; each function has
        // the signature it is registered with.
        let said = unsafe { self.slot::<extern "C" fn(Env, Obj, *const Native, i32) -> i32>(REGISTER_NATIVES)(self, class, natives.as_ptr(), natives.len() as i32) };
        !self.threw() && said == 0
    }
}

/// A new Java `int[]` of `long` zeros.
pub(crate) fn new_ints(env: Env, long: i32) -> Obj {
    // SAFETY: a plain call of the interface.
    let made = unsafe { env.slot::<extern "C" fn(Env, i32) -> Obj>(NEW_INT_ARRAY)(env, long) };
    if env.threw() { std::ptr::null_mut() } else { made }
}

/// Reads the first numbers of a Java `int[]` into `into`.
pub(crate) fn ints(env: Env, of: Obj, into: &mut [i32]) {
    if !of.is_null() {
        // SAFETY: the array has at least as many numbers as asked for (it was made so).
        unsafe { env.slot::<extern "C" fn(Env, Obj, i32, i32, *mut i32)>(GET_INT_ARRAY_REGION)(env, of, 0, into.len() as i32, into.as_mut_ptr()) };
        env.threw();
    }
}

/// The library's Java classes: a view (accessibility host, input method target, insets), its
/// accessibility node provider, and its input connection.
pub(crate) const CLASSES: [Class; 3] = [
    Class {
        name: "taut/Host",
        parent: "android/view/View",
        made_with: &["Landroid/content/Context;"],
        natives: &[
            ("getAccessibilityNodeProvider", "()Landroid/view/accessibility/AccessibilityNodeProvider;"),
            ("onCreateInputConnection", "(Landroid/view/inputmethod/EditorInfo;)Landroid/view/inputmethod/InputConnection;"),
            ("onCheckIsTextEditor", "()Z"),
            ("onApplyWindowInsets", "(Landroid/view/WindowInsets;)Landroid/view/WindowInsets;"),
        ],
    },
    Class {
        name: "taut/Nodes",
        parent: "android/view/accessibility/AccessibilityNodeProvider",
        made_with: &[],
        natives: &[
            ("createAccessibilityNodeInfo", "(I)Landroid/view/accessibility/AccessibilityNodeInfo;"),
            ("performAction", "(IILandroid/os/Bundle;)Z"),
            ("findFocus", "(I)Landroid/view/accessibility/AccessibilityNodeInfo;"),
        ],
    },
    Class {
        name: "taut/Typing",
        parent: "android/view/inputmethod/BaseInputConnection",
        made_with: &["Landroid/view/View;", "Z"],
        natives: &[
            ("commitText", "(Ljava/lang/CharSequence;I)Z"),
            ("setComposingText", "(Ljava/lang/CharSequence;I)Z"),
            ("finishComposingText", "()Z"),
            ("deleteSurroundingText", "(II)Z"),
            ("performEditorAction", "(I)Z"),
        ],
    },
];

/// The file `CLASSES` are in, made once and kept for the VM, which reads it in place.
static DEX: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// Loads the library's classes beside the activity's own (`InMemoryDexClassLoader`, Android 8
/// and later). Each, by name, as a global reference; none if the VM refuses the file.
pub(crate) fn load_classes(env: Env, activity: Obj) -> Option<[Obj; 3]> {
    let file = DEX.get_or_init(|| dex(&CLASSES));
    env.frame(|| {
        // SAFETY: the buffer points at the file, which lives for ever and is never written.
        let buffer = unsafe { env.slot::<extern "C" fn(Env, *mut c_void, i64) -> Obj>(NEW_DIRECT_BYTE_BUFFER)(env, file.as_ptr().cast_mut().cast(), file.len() as i64) };
        let parent = env.call(activity, c"getClassLoader", c"()Ljava/lang/ClassLoader;", &[]).object();
        let loader = env.new_named(c"dalvik/system/InMemoryDexClassLoader", c"(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V", &[Value::obj(buffer), Value::obj(parent)]);
        let load = |name: &str| env.global(env.call(loader, c"loadClass", c"(Ljava/lang/String;)Ljava/lang/Class;", &[Value::obj(env.string(name))]).object());
        let classes = [load("taut.Host"), load("taut.Nodes"), load("taut.Typing")];
        classes.iter().all(|class| !class.is_null()).then_some(classes)
    })
}

/// A system service of the activity (`clipboard`, `input_method`, `accessibility`).
pub(crate) fn service(env: Env, activity: Obj, name: &str) -> Obj {
    env.call(activity, c"getSystemService", c"(Ljava/lang/String;)Ljava/lang/Object;", &[Value::obj(env.string(name))]).object()
}

/// The clipboard's text, if any.
pub(crate) fn clipboard_text(env: Env, activity: Obj) -> Option<String> {
    env.frame(|| {
        let clip = env.call(service(env, activity, "clipboard"), c"getPrimaryClip", c"()Landroid/content/ClipData;", &[]).object();
        if env.call(clip, c"getItemCount", c"()I", &[]).as_int() < 1 {
            return None;
        }
        let item = env.call(clip, c"getItemAt", c"(I)Landroid/content/ClipData$Item;", &[Value::int(0)]).object();
        env.text(env.call(item, c"coerceToText", c"(Landroid/content/Context;)Ljava/lang/CharSequence;", &[Value::obj(activity)]).object())
    })
}

/// Puts text on the clipboard.
pub(crate) fn set_clipboard_text(env: Env, activity: Obj, text: &str) {
    env.frame(|| {
        let args = [Value::obj(env.string("text")), Value::obj(env.string(text))];
        let clip = env.call_static(c"android/content/ClipData", c"newPlainText", c"(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;", &args).object();
        env.call(service(env, activity, "clipboard"), c"setPrimaryClip", c"(Landroid/content/ClipData;)V", &[Value::obj(clip)]);
    });
}

/// Shows or hides the soft keyboard for the library's view; `again` restarts its input (text
/// focus moved to another box).
pub(crate) fn soft_keyboard(env: Env, activity: Obj, view: Obj, show: bool, again: bool) {
    env.frame(|| {
        let methods = service(env, activity, "input_method");
        if again {
            env.call(methods, c"restartInput", c"(Landroid/view/View;)V", &[Value::obj(view)]);
        }
        if show {
            env.call(view, c"requestFocus", c"()Z", &[]);
            env.call(methods, c"showSoftInput", c"(Landroid/view/View;I)Z", &[Value::obj(view), Value::int(0)]);
        } else {
            let token = env.call(view, c"getWindowToken", c"()Landroid/os/IBinder;", &[]).object();
            env.call(methods, c"hideSoftInputFromWindow", c"(Landroid/os/IBinder;I)Z", &[Value::obj(token), Value::int(0)]);
        }
    });
}

/// What window insets keep clear, in pixels: system bars, display cutout and the soft keyboard
/// (`[left, top, right, bottom]`).
pub(crate) fn insets_of(env: Env, insets: Obj) -> [i32; 4] {
    env.frame(|| {
        let types = [c"systemBars", c"displayCutout", c"ime"].map(|name| env.call_static(c"android/view/WindowInsets$Type", name, c"()I", &[]).as_int());
        let kept = env.call(insets, c"getInsets", c"(I)Landroid/graphics/Insets;", &[Value::int(types.iter().fold(0, |all, one| all | one))]).object();
        [c"left", c"top", c"right", c"bottom"].map(|side| env.int_field(kept, side))
    })
}

/// What the person set for the display: pixels a unit, text scale, dark mode, the accent
/// colour (Android 12 and later) and whether animations are off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Display {
    pub density: f32,
    pub text_scale: f32,
    pub dark: bool,
    pub accent: Option<u32>,
    pub still: bool,
}

/// The display settings, as the activity's resources have them now.
pub(crate) fn display(env: Env, activity: Obj) -> Display {
    env.frame(|| {
        let resources = env.call(activity, c"getResources", c"()Landroid/content/res/Resources;", &[]).object();
        let metrics = env.call(resources, c"getDisplayMetrics", c"()Landroid/util/DisplayMetrics;", &[]).object();
        let config = env.call(resources, c"getConfiguration", c"()Landroid/content/res/Configuration;", &[]).object();
        let accent_id = env.static_int(c"android/R$color", c"system_accent1_500");
        let accent = (accent_id != 0).then(|| env.call(activity, c"getColor", c"(I)I", &[Value::int(accent_id)]).as_int() as u32);
        let resolver = env.call(activity, c"getContentResolver", c"()Landroid/content/ContentResolver;", &[]).object();
        let args = [Value::obj(resolver), Value::obj(env.string("animator_duration_scale")), Value::float(1.0)];
        let motion = env.call_static(c"android/provider/Settings$Global", c"getFloat", c"(Landroid/content/ContentResolver;Ljava/lang/String;F)F", &args).as_float();
        Display {
            density: env.float_field(metrics, c"density").max(0.5),
            text_scale: env.float_field(config, c"fontScale").max(0.5),
            dark: env.int_field(config, c"uiMode") & 0x30 == 0x20,
            accent,
            still: motion == 0.0,
        }
    })
}

/// Opens a link with the app the system opens it with (an `ACTION_VIEW` intent), from any
/// thread. Says whether an app took it.
pub(crate) fn open_link(activity: Obj, link: &str) -> bool {
    let vm = VM.load(Ordering::SeqCst);
    if vm.is_null() || activity.is_null() {
        return false;
    }
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: the VM's own table, slots 4 and 5: this thread is attached for the call and
    // detached after.
    let env = unsafe {
        let table = *(vm as *mut *const *const c_void);
        let attach: extern "C" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> i32 = std::mem::transmute_copy(&*table.add(ATTACH_CURRENT_THREAD));
        (attach(vm, &mut raw, std::ptr::null_mut()) == 0).then_some(Env(raw.cast()))
    };
    let Some(env) = env else { return false };
    let opened = env.frame(|| {
        let uri = env.call_static(c"android/net/Uri", c"parse", c"(Ljava/lang/String;)Landroid/net/Uri;", &[Value::obj(env.string(link))]).object();
        let intent = env.new_named(c"android/content/Intent", c"(Ljava/lang/String;Landroid/net/Uri;)V", &[Value::obj(env.string("android.intent.action.VIEW")), Value::obj(uri)]);
        env.call(intent, c"addFlags", c"(I)Landroid/content/Intent;", &[Value::int(0x1000_0000)]);
        env.call(activity, c"startActivity", c"(Landroid/content/Intent;)V", &[Value::obj(intent)]);
        !intent.is_null() && !env.threw()
    });
    // SAFETY: as above.
    unsafe {
        let table = *(vm as *mut *const *const c_void);
        let detach: extern "C" fn(*mut c_void) -> i32 = std::mem::transmute_copy(&*table.add(DETACH_CURRENT_THREAD));
        detach(vm);
    }
    opened
}
