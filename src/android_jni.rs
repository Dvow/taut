//! Android's Java, called through JNI: the classes `android_dex` writes, loaded and given their
//! native methods; and the system services an app reaches only from Java (the clipboard, the
//! soft keyboard, window insets, the person's display settings, opening a link). Everything here
//! runs on the main thread, where the activity's own `JNIEnv` is, but what takes a thread's own
//! interface (`thread_env`).
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
    pub(crate) fn long(j: i64) -> Value {
        Value { j }
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
    pub(crate) fn as_long(self) -> i64 {
        // SAFETY: plain data.
        unsafe { self.j }
    }
}

/// A native method handed to Java: name, descriptor, the function.
#[repr(C)]
pub(crate) struct Native {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub function: *const c_void,
}

impl Native {
    pub(crate) fn new(name: &'static CStr, signature: &'static CStr, function: *const c_void) -> Native {
        Native { name: name.as_ptr(), signature: signature.as_ptr(), function }
    }
}

/// JNI's table: the slots used, numbered as in `jni.h`.
const FIND_CLASS: usize = 6;
const EXCEPTION_DESCRIBE: usize = 16;
const EXCEPTION_CLEAR: usize = 17;
const PUSH_LOCAL_FRAME: usize = 19;
const POP_LOCAL_FRAME: usize = 20;
const NEW_GLOBAL_REF: usize = 21;
const DELETE_GLOBAL_REF: usize = 22;
const DELETE_LOCAL_REF: usize = 23;
const IS_SAME_OBJECT: usize = 24;
const NEW_OBJECT_A: usize = 30;
const GET_OBJECT_CLASS: usize = 31;
const GET_METHOD_ID: usize = 33;
/// `CallNonvirtualBooleanMethodA`.
const CALL_NONVIRTUAL_BOOLEAN_METHOD_A: usize = 69;
/// The first `Call<Type>MethodA`, for objects; the others follow, three slots apart, in the
/// order of `TYPES`. Statics likewise.
const CALL_OBJECT_METHOD_A: usize = 36;
const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
/// `CallVoidMethodA`.
const CALL_VOID_METHOD_A: usize = 63;
const TYPES: &[u8] = b"LZBCSIJFDV";
const GET_FIELD_ID: usize = 94;
const GET_OBJECT_FIELD: usize = 95;
const GET_INT_FIELD: usize = 100;
const GET_FLOAT_FIELD: usize = 102;
const SET_OBJECT_FIELD: usize = 104;
const SET_INT_FIELD: usize = 109;
const GET_STATIC_METHOD_ID: usize = 113;
const GET_STATIC_FIELD_ID: usize = 144;
const GET_STATIC_OBJECT_FIELD: usize = 145;
const GET_STATIC_INT_FIELD: usize = 150;
const NEW_STRING: usize = 163;
const GET_STRING_LENGTH: usize = 164;
const GET_ARRAY_LENGTH: usize = 171;
const NEW_OBJECT_ARRAY: usize = 172;
const GET_OBJECT_ARRAY_ELEMENT: usize = 173;
const SET_OBJECT_ARRAY_ELEMENT: usize = 174;
const NEW_INT_ARRAY: usize = 179;
const GET_INT_ARRAY_REGION: usize = 203;
const SET_INT_ARRAY_REGION: usize = 211;
const REGISTER_NATIVES: usize = 215;
const GET_STRING_REGION: usize = 220;
const EXCEPTION_CHECK: usize = 228;
const NEW_DIRECT_BYTE_BUFFER: usize = 229;
/// The VM's own table: attach and detach a thread, and a thread's interface.
const ATTACH_CURRENT_THREAD: usize = 4;
const DETACH_CURRENT_THREAD: usize = 5;
const GET_ENV: usize = 6;
/// `JNI_VERSION_1_6`.
const VERSION: i32 = 0x0001_0006;

/// A thread's JNI interface (`JNIEnv*`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Env(pub *mut *const *const c_void);

thread_local! {
    /// The main thread's interface, from the activity.
    static MAIN: Cell<Env> = const { Cell::new(Env(std::ptr::null_mut())) };
    /// Another thread's interface, once it was attached here (detached as the thread ends).
    static ATTACHED: Attached = const { Attached(Cell::new(Env(std::ptr::null_mut()))) };
}

/// A thread attached to the VM by the library, detached when the thread ends (the VM ends the
/// process if a thread ends attached).
struct Attached(Cell<Env>);

impl Drop for Attached {
    fn drop(&mut self) {
        if self.0.get().0.is_null() {
            return;
        }
        if let Some((vm, detach)) = vm_call::<extern "C" fn(*mut c_void) -> i32>(DETACH_CURRENT_THREAD) {
            detach(vm);
        }
    }
}

/// A function of the VM's own table, with the VM.
fn vm_call<F: Copy>(slot: usize) -> Option<(*mut c_void, F)> {
    let vm = VM.load(Ordering::SeqCst);
    // SAFETY: the VM's table has every slot of `jni.h`'s `JNIInvokeInterface`; the caller names
    // the slot's type.
    (!vm.is_null()).then(|| (vm, unsafe { std::mem::transmute_copy(&*(*(vm as *mut *const *const c_void)).add(slot)) }))
}

/// This thread's interface: the main thread's, or another's, attached the first time and for
/// as long as the thread lives. None before an activity started.
pub(crate) fn thread_env() -> Option<Env> {
    if let Some(env) = env() {
        return Some(env);
    }
    let attached = ATTACHED.with(|kept| kept.0.get());
    if !attached.0.is_null() {
        return Some(attached);
    }
    let mut raw: *mut c_void = std::ptr::null_mut();
    let (vm, get) = vm_call::<extern "C" fn(*mut c_void, *mut *mut c_void, i32) -> i32>(GET_ENV)?;
    if get(vm, &mut raw, VERSION) == 0 {
        return Some(Env(raw.cast()));
    }
    let (vm, attach) = vm_call::<extern "C" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> i32>(ATTACH_CURRENT_THREAD)?;
    let env = (attach(vm, &mut raw, std::ptr::null_mut()) == 0).then_some(Env(raw.cast()))?;
    ATTACHED.with(|kept| kept.0.set(env));
    Some(env)
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

    /// The id of an object's method `name` with `signature` (null for a null object, or none).
    fn method_of(self, on: Obj, name: &CStr, signature: &CStr) -> Obj {
        if on.is_null() {
            return on;
        }
        // SAFETY: the object is alive; its class answers the method, or Java throws (cleared).
        let method = unsafe {
            let class = self.slot::<extern "C" fn(Env, Obj) -> Obj>(GET_OBJECT_CLASS)(self, on);
            let method = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr());
            self.drop_local(class);
            method
        };
        if self.threw() { std::ptr::null_mut() } else { method }
    }

    /// Calls `name` with `signature` on an object (none: nothing happens).
    pub(crate) fn call(self, on: Obj, name: &CStr, signature: &CStr, args: &[Value]) -> Value {
        let method = self.method_of(on, name, signature);
        if method.is_null() {
            return Value { j: 0 };
        }
        self.invoke(CALL_OBJECT_METHOD_A, on, method, signature, args)
    }

    /// Calls a method that returns nothing: says whether it ran, without Java throwing.
    pub(crate) fn run(self, on: Obj, name: &CStr, signature: &CStr, args: &[Value]) -> bool {
        let method = self.method_of(on, name, signature);
        if method.is_null() {
            return false;
        }
        // SAFETY: a method of the object's class that returns nothing.
        unsafe { self.slot::<extern "C" fn(Env, Obj, Obj, *const Value)>(CALL_VOID_METHOD_A)(self, on, method, args.as_ptr()) };
        !self.threw()
    }

    /// Calls a `boolean` method of an object as `class` (with slashes) has it, not as the
    /// object's own class overrides it: what Java's `super.` calls.
    pub(crate) fn call_super(self, on: Obj, class: &CStr, name: &CStr, signature: &CStr, args: &[Value]) -> bool {
        let class = self.class(class);
        if class.is_null() {
            return false;
        }
        // SAFETY: the class is alive; its method is asked for by name and signature, and called
        // on an object of a class that extends it.
        let said = unsafe {
            let method = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr());
            !method.is_null() && self.slot::<extern "C" fn(Env, Obj, Obj, Obj, *const Value) -> u8>(CALL_NONVIRTUAL_BOOLEAN_METHOD_A)(self, on, class, method, args.as_ptr()) != 0
        };
        self.drop_local(class);
        !self.threw() && said
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

    /// An object field of an object, by name and type (null without one).
    pub(crate) fn object_field(self, on: Obj, name: &CStr, kind: &CStr) -> Obj {
        let field = if on.is_null() { std::ptr::null_mut() } else { self.field(on, name, kind) };
        // SAFETY: the field is the object's, of an object type.
        if field.is_null() { field } else { unsafe { self.slot::<extern "C" fn(Env, Obj, Obj) -> Obj>(GET_OBJECT_FIELD)(self, on, field) } }
    }

    /// Sets an object field of an object.
    pub(crate) fn set_object_field(self, on: Obj, name: &CStr, kind: &CStr, value: Obj) {
        let field = if on.is_null() { std::ptr::null_mut() } else { self.field(on, name, kind) };
        if !field.is_null() {
            // SAFETY: the field is the object's, of an object type the value has.
            unsafe { self.slot::<extern "C" fn(Env, Obj, Obj, Obj)>(SET_OBJECT_FIELD)(self, on, field, value) };
        }
    }

    /// A static object field of a class, by name and type (null without one).
    pub(crate) fn static_object(self, class: &CStr, name: &CStr, kind: &CStr) -> Obj {
        let class = self.class(class);
        if class.is_null() {
            return class;
        }
        // SAFETY: the class is alive; its field is asked for by name and type.
        let value = unsafe {
            let field = self.slot::<extern "C" fn(Env, Obj, *const c_char, *const c_char) -> Obj>(GET_STATIC_FIELD_ID)(self, class, name.as_ptr(), kind.as_ptr());
            if field.is_null() { field } else { self.slot::<extern "C" fn(Env, Obj, Obj) -> Obj>(GET_STATIC_OBJECT_FIELD)(self, class, field) }
        };
        let value = if self.threw() { std::ptr::null_mut() } else { value };
        self.drop_local(class);
        value
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

    /// Lets go of a global reference.
    pub(crate) fn drop_global(self, of: Obj) {
        if !of.is_null() {
            // SAFETY: a global reference the library made, not used after.
            unsafe { self.slot::<extern "C" fn(Env, Obj)>(DELETE_GLOBAL_REF)(self, of) };
        }
    }

    /// Whether two references are to the same object.
    pub(crate) fn same(self, one: Obj, other: Obj) -> bool {
        // SAFETY: two live references (or nulls).
        unsafe { self.slot::<extern "C" fn(Env, Obj, Obj) -> u8>(IS_SAME_OBJECT)(self, one, other) != 0 }
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

/// A new Java array of `long` nulls, for objects of the class named (with slashes).
pub(crate) fn new_object_array(env: Env, class: &CStr, long: i32) -> Obj {
    let class = env.class(class);
    if class.is_null() {
        return class;
    }
    // SAFETY: a plain call of the interface, with a live class.
    let made = unsafe { env.slot::<extern "C" fn(Env, i32, Obj, Obj) -> Obj>(NEW_OBJECT_ARRAY)(env, long, class, std::ptr::null_mut()) };
    env.drop_local(class);
    if env.threw() { std::ptr::null_mut() } else { made }
}

/// Puts an object into a Java array of objects, at `at`.
pub(crate) fn set_object(env: Env, array: Obj, at: i32, item: Obj) {
    if !array.is_null() {
        // SAFETY: the array is alive and of the item's class; Java throws (cleared) past its end.
        unsafe { env.slot::<extern "C" fn(Env, Obj, i32, Obj)>(SET_OBJECT_ARRAY_ELEMENT)(env, array, at, item) };
        env.threw();
    }
}

/// A new Java array of the class named (with slashes) holding `items`.
pub(crate) fn new_objects(env: Env, class: &CStr, items: &[Obj]) -> Obj {
    let made = new_object_array(env, class, items.len() as i32);
    for (at, item) in items.iter().enumerate() {
        set_object(env, made, at as i32, *item);
    }
    made
}

/// The objects in a Java array (local references; none for null).
pub(crate) fn objects(env: Env, of: Obj) -> Vec<Obj> {
    if of.is_null() {
        return Vec::new();
    }
    // SAFETY: the array is alive; each element up to its length is read.
    let long = unsafe { env.slot::<extern "C" fn(Env, Obj) -> i32>(GET_ARRAY_LENGTH)(env, of) };
    (0..long.max(0)).map(|at| unsafe { env.slot::<extern "C" fn(Env, Obj, i32) -> Obj>(GET_OBJECT_ARRAY_ELEMENT)(env, of, at) }).collect()
}

/// Reads the first numbers of a Java `int[]` into `into`.
pub(crate) fn ints(env: Env, of: Obj, into: &mut [i32]) {
    if !of.is_null() {
        // SAFETY: the array has at least as many numbers as asked for (it was made so).
        unsafe { env.slot::<extern "C" fn(Env, Obj, i32, i32, *mut i32)>(GET_INT_ARRAY_REGION)(env, of, 0, into.len() as i32, into.as_mut_ptr()) };
        env.threw();
    }
}

/// A bitmap of a picture: `width` pixels a row, `0xAARRGGBB` not premultiplied.
pub(crate) fn bitmap(env: Env, width: usize, pixels: &[u32]) -> Obj {
    let height = pixels.len() / width.max(1);
    let colours = new_ints(env, pixels.len() as i32);
    if colours.is_null() || height == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the array was made as long as the pixels, which are read whole.
    unsafe { env.slot::<extern "C" fn(Env, Obj, i32, i32, *const u32)>(SET_INT_ARRAY_REGION)(env, colours, 0, pixels.len() as i32, pixels.as_ptr()) };
    let config = env.static_object(c"android/graphics/Bitmap$Config", c"ARGB_8888", c"Landroid/graphics/Bitmap$Config;");
    let args = [Value::obj(colours), Value::int(width as i32), Value::int(height as i32), Value::obj(config)];
    env.call_static(c"android/graphics/Bitmap", c"createBitmap", c"([IIILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;", &args).object()
}

/// Whether the activity is the package's own `taut.Activity` (see `examples/apk.rs`), which
/// keeps what the system answers for the library.
pub(crate) fn is_app_activity(env: Env, activity: Obj) -> bool {
    let class = env.call(activity, c"getClass", c"()Ljava/lang/Class;", &[]).object();
    let name = env.text(env.call(class, c"getName", c"()Ljava/lang/String;", &[]).object());
    env.drop_local(class);
    name.as_deref() == Some("taut.Activity")
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
            ("addExtraDataToAccessibilityNodeInfo", "(ILandroid/view/accessibility/AccessibilityNodeInfo;Ljava/lang/String;Landroid/os/Bundle;)V"),
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
            ("deleteSurroundingTextInCodePoints", "(II)Z"),
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
/// focus came to a box).
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
    let Some(env) = thread_env().filter(|_| !activity.is_null()) else { return false };
    env.frame(|| {
        let uri = env.call_static(c"android/net/Uri", c"parse", c"(Ljava/lang/String;)Landroid/net/Uri;", &[Value::obj(env.string(link))]).object();
        let intent = env.new_named(c"android/content/Intent", c"(Ljava/lang/String;Landroid/net/Uri;)V", &[Value::obj(env.string("android.intent.action.VIEW")), Value::obj(uri)]);
        start_new_task(env, activity, intent)
    })
}

/// Starts an activity for `intent` in a task of its own: says whether one did.
pub(crate) fn start_new_task(env: Env, activity: Obj, intent: Obj) -> bool {
    env.call(intent, c"addFlags", c"(I)Landroid/content/Intent;", &[Value::int(NEW_TASK)]);
    env.run(activity, c"startActivity", c"(Landroid/content/Intent;)V", &[Value::obj(intent)])
}

/// `Intent.FLAG_ACTIVITY_NEW_TASK`.
pub(crate) const NEW_TASK: i32 = 0x1000_0000;

/// A class of the app's own package (its `classes.dex`, see `examples/apk.rs`), by name with
/// dots, as a local reference: null if the package lacks it.
pub(crate) fn app_class(env: Env, activity: Obj, name: &str) -> Obj {
    let (loader, name) = (env.call(activity, c"getClassLoader", c"()Ljava/lang/ClassLoader;", &[]).object(), env.string(name));
    let class = env.call(loader, c"loadClass", c"(Ljava/lang/String;)Ljava/lang/Class;", &[Value::obj(name)]).object();
    env.drop_local(name);
    env.drop_local(loader);
    class
}
