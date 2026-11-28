//! macOS's and iOS's own interfaces, declared by hand: the Objective-C runtime, and what the
//! windows, text and clipboard use of CoreFoundation and CoreGraphics. AppKit and UIKit are
//! reached through messages (`send`), their classes by name; nothing here is a crate.
//!
//! A message is sent through `objc_msgSend` cast to the method's own C signature, as the
//! compiler does. Methods returning a rectangle use `objc_msgSend_stret` on x86-64, where a
//! 32-byte value comes back through memory (`send_rect`). Classes of the library's own (a view,
//! a window, delegates) are made at run time, each method a plain `extern "C"` function.
use std::ffi::{CStr, c_char, c_void};

/// An Objective-C object (`id`); null is `nil`.
pub(crate) type Obj = *mut c_void;
/// A method's name (`SEL`).
pub(crate) type Sel = *const c_void;
/// `BOOL`: one byte, 0 or 1.
pub(crate) type Bool = u8;

/// A point, a size and a rectangle in points (`NSPoint`, `NSSize`, `NSRect`, and CoreGraphics'
/// same `CGPoint`, `CGSize`, `CGRect`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NSPoint {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NSSize {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NSRect {
    pub origin: NSPoint,
    pub size: NSSize,
}

impl NSRect {
    /// A rectangle from its corner nearest the origin and its size.
    pub(crate) const fn new(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect { origin: NSPoint { x, y }, size: NSSize { width, height } }
    }
}

/// A range of UTF-16 units (`NSRange`; `CFRange` is the same in signed numbers).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NSRange {
    pub location: usize,
    pub length: usize,
}

/// `NSNotFound`: a range that is none.
#[cfg_attr(target_os = "ios", allow(dead_code, reason = "iOS has no such part"))]
pub(crate) const NOT_FOUND: usize = isize::MAX as usize;

/// Type encodings of the methods the library's classes add: what the runtime says of them to
/// whoever asks (key-value coding, accessibility). `BOOL` is C's `bool` (`B`), except on an
/// Intel Mac (`signed char`, `c`).
#[cfg(any(target_arch = "aarch64", target_os = "ios"))]
pub(crate) const BOOL: &str = "B";
#[cfg(not(any(target_arch = "aarch64", target_os = "ios")))]
pub(crate) const BOOL: &str = "c";
pub(crate) const RECT: &str = "{CGRect={CGPoint=dd}{CGSize=dd}}";
pub(crate) const POINT: &str = "{CGPoint=dd}";
pub(crate) const RANGE: &str = "{_NSRange=QQ}";

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Obj;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
    fn objc_allocateClassPair(superclass: Obj, name: *const c_char, extra: usize) -> Obj;
    fn objc_registerClassPair(class: Obj);
    fn class_addMethod(class: Obj, name: Sel, imp: *const c_void, types: *const c_char) -> Bool;
    fn class_addProtocol(class: Obj, protocol: *const c_void) -> Bool;
    fn objc_getProtocol(name: *const c_char) -> *const c_void;
    pub(crate) fn objc_autoreleasePoolPush() -> *mut c_void;
    pub(crate) fn objc_autoreleasePoolPop(pool: *mut c_void);
}

// The frameworks, linked by name: their classes are found at run time, through the runtime.
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    /// Posted when the user's locale or its formats change; and a locale's key for its calendar.
    pub(crate) static NSCurrentLocaleDidChangeNotification: Obj;
    pub(crate) static NSLocaleCalendar: Obj;
}
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    /// How a layer shows its picture: at its top left, unscaled.
    pub(crate) static kCAGravityTopLeft: Obj;
}
#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// `NSApp`'s pasteboard type names, and the run loop mode events are taken in.
    pub(crate) static NSPasteboardTypeString: Obj;
    pub(crate) static NSPasteboardTypePNG: Obj;
    pub(crate) static NSPasteboardTypeTIFF: Obj;
    pub(crate) static NSPasteboardTypeFileURL: Obj;
    pub(crate) static NSDefaultRunLoopMode: Obj;
}

/// `CFRange`: a start and a length in UTF-16 units.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct CFRange {
    pub(crate) location: isize,
    pub(crate) length: isize,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFRelease(object: *const c_void);
    pub(crate) fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    pub(crate) fn CFStringCreateWithBytes(allocator: *const c_void, bytes: *const u8, length: isize, encoding: u32, external: Bool) -> *const c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CGColorSpaceCreateDeviceGray() -> *const c_void;
    pub(crate) fn CGColorSpaceCreateWithName(name: *const c_void) -> *const c_void;
    pub(crate) static kCGColorSpaceSRGB: *const c_void;
    #[cfg(all(target_os = "macos", feature = "hdr"))]
    pub(crate) static kCGColorSpaceExtendedLinearSRGB: *const c_void;
    pub(crate) fn CGColorSpaceRelease(space: *const c_void);
    pub(crate) fn CGDataProviderCreateWithCFData(data: *const c_void) -> *const c_void;
    pub(crate) fn CGDataProviderRelease(provider: *const c_void);
    pub(crate) fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *const c_void,
        info: u32,
        provider: *const c_void,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> *const c_void;
    pub(crate) fn CGImageRelease(image: *const c_void);
    pub(crate) fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bits: usize, bytes_per_row: usize, space: *const c_void, info: u32) -> *mut c_void;
    pub(crate) fn CGContextRelease(context: *mut c_void);
    pub(crate) fn CGContextSetGrayFillColor(context: *mut c_void, gray: f64, alpha: f64);
    pub(crate) fn CGContextSetShouldAntialias(context: *mut c_void, on: bool);
}

/// A CGImage's pixels as the library keeps them: 32 bits, `0xAARRGGBB` premultiplied in a
/// little-endian word (`kCGBitmapByteOrder32Little | kCGImageAlphaPremultipliedFirst`); and a
/// solid one's, the same without alpha (`kCGImageAlphaNoneSkipFirst`).
#[cfg_attr(target_os = "ios", allow(dead_code, reason = "iOS has no such part"))]
pub(crate) const PREMULTIPLIED: u32 = 2 << 12 | 2;
pub(crate) const SOLID: u32 = 2 << 12 | 6;
/// `kCFStringEncodingUTF8`.
pub(crate) const UTF8: u32 = 0x0800_0100;

/// A class by name; null if none is loaded.
pub(crate) fn class(name: &CStr) -> Obj {
    // SAFETY: the name ends in a zero; the runtime only looks it up.
    unsafe { objc_getClass(name.as_ptr()) }
}

/// A method name.
pub(crate) fn sel(name: &CStr) -> Sel {
    // SAFETY: the name ends in a zero; the runtime keeps it for the program's life.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// A message's arguments: a tuple of up to six, each as the method's C signature has it.
pub(crate) trait Args {
    /// Sends the message `name` to `to` with these arguments, through `objc_msgSend` cast to
    /// the signature they and `R` make. (Or `via`: `objc_msgSend_stret`, for a large result on
    /// x86-64.)
    ///
    /// # Safety
    /// `to` is an object (or nil) that answers `name`, with this signature.
    unsafe fn send<R>(self, via: unsafe extern "C" fn(), to: Obj, name: Sel) -> R;
}

macro_rules! args {
    ($($arg:ident),*) => {
        impl<$($arg),*> Args for ($($arg,)*) {
            #[allow(non_snake_case)]
            unsafe fn send<R>(self, via: unsafe extern "C" fn(), to: Obj, name: Sel) -> R {
                let ($($arg,)*) = self;
                // SAFETY: as the caller promises: the function is called as the method's own
                // signature, as the compiler calls it.
                let call = unsafe { std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Obj, Sel $(, $arg)*) -> R>(via) };
                unsafe { call(to, name $(, $arg)*) }
            }
        }
    };
}
args!();
args!(A);
args!(A, B);
args!(A, B, C);
args!(A, B, C, D);
args!(A, B, C, D, E);
args!(A, B, C, D, E, F);
args!(A, B, C, D, E, F, G);
args!(A, B, C, D, E, F, G, H);
args!(A, B, C, D, E, F, G, H, I);
args!(A, B, C, D, E, F, G, H, I, J);

/// Sends a message: `send::<R>(object, c"name:", (argument,))`.
///
/// # Safety
/// `to` is an object (or nil, which answers zero) that answers `name` with arguments and a
/// result of these types.
pub(crate) unsafe fn send<R>(to: Obj, name: &CStr, args: impl Args) -> R {
    // SAFETY: as the caller promises.
    unsafe { args.send(objc_msgSend, to, sel(name)) }
}

/// Sends a message whose result is a rectangle.
///
/// # Safety
/// As `send`.
pub(crate) unsafe fn send_rect(to: Obj, name: &CStr, args: impl Args) -> NSRect {
    // SAFETY: as the caller promises.
    unsafe { send_large(to, name, args) }
}

/// Sends a message whose result is a struct larger than 16 bytes (a rectangle, a `CMTime`):
/// on x86-64 it comes back through memory. Nil answers the default.
///
/// # Safety
/// As `send`.
pub(crate) unsafe fn send_large<R: Default>(to: Obj, name: &CStr, args: impl Args) -> R {
    #[cfg(target_arch = "x86_64")]
    let via = objc_msgSend_stret;
    #[cfg(not(target_arch = "x86_64"))]
    let via = objc_msgSend;
    // SAFETY: as the caller promises; nil would answer garbage here, so it is not asked.
    if to.is_null() { R::default() } else { unsafe { args.send(via, to, sel(name)) } }
}

/// A new class of the library's own, `name`, made from `superclass` with these methods (name,
/// function, type encoding) and protocols; or the one made before under that name.
pub(crate) fn made_class(name: &CStr, superclass: &CStr, methods: &[(&CStr, *const c_void, &str)], protocols: &[&CStr]) -> Obj {
    let known = class(name);
    if !known.is_null() {
        return known;
    }
    // SAFETY: the superclass is loaded; every method is a function of the signature its
    // encoding says, and the names end in zeros. The class is registered once complete.
    unsafe {
        let made = objc_allocateClassPair(class(superclass), name.as_ptr(), 0);
        for (name, imp, types) in methods {
            let types = std::ffi::CString::new(*types).unwrap_or_default();
            class_addMethod(made, sel(name), *imp, types.as_ptr());
        }
        for protocol in protocols {
            let protocol = objc_getProtocol(protocol.as_ptr());
            if !protocol.is_null() {
                class_addProtocol(made, protocol);
            }
        }
        objc_registerClassPair(made);
        made
    }
}

/// Adds methods to a class made before.
pub(crate) fn add_methods(class: Obj, methods: &[(&CStr, *const c_void, &str)]) {
    for (name, imp, types) in methods {
        let types = std::ffi::CString::new(*types).unwrap_or_default();
        // SAFETY: each method is a function of the signature its encoding says; the runtime
        // takes no method for no class.
        unsafe { class_addMethod(class, sel(name), *imp, types.as_ptr()) };
    }
}

/// A new object of a class, made with `init` (retained: the caller releases it).
pub(crate) fn made(class_name: &CStr) -> Obj {
    // SAFETY: every class answers `alloc` and `init`.
    unsafe { send(send::<Obj>(class(class_name), c"alloc", ()), c"init", ()) }
}

/// Text as an `NSString` (autoreleased). Text with a zero in it is cut there.
pub(crate) fn ns_string(text: &str) -> Obj {
    // SAFETY: `NSString` copies the bytes it is given, as UTF-8 (4).
    unsafe {
        let made: Obj = send(class(c"NSString"), c"alloc", ());
        let made: Obj = send(made, c"initWithBytes:length:encoding:", (text.as_ptr(), text.len(), 4usize));
        send(made, c"autorelease", ())
    }
}

/// An `NSString`'s text; `None` for nil.
pub(crate) fn rust_string(string: Obj) -> Option<String> {
    if string.is_null() {
        return None;
    }
    // SAFETY: `UTF8String` is valid while the string is: copied at once.
    let bytes: *const c_char = unsafe { send(string, c"UTF8String", ()) };
    (!bytes.is_null()).then(|| unsafe { CStr::from_ptr(bytes) }.to_string_lossy().into_owned())
}

/// An `NSData`'s bytes, copied; empty for nil.
pub(crate) fn bytes_of(data: Obj) -> Vec<u8> {
    if data.is_null() {
        return Vec::new();
    }
    // SAFETY: the bytes are valid while the data is: copied at once.
    unsafe {
        let (at, long): (*const u8, usize) = (send(data, c"bytes", ()), send(data, c"length", ()));
        if at.is_null() { Vec::new() } else { std::slice::from_raw_parts(at, long).to_vec() }
    }
}

/// Bytes as an `NSData` (autoreleased).
pub(crate) fn ns_data(bytes: &[u8]) -> Obj {
    // SAFETY: `NSData` copies the bytes.
    unsafe { send(class(c"NSData"), c"dataWithBytes:length:", (bytes.as_ptr(), bytes.len())) }
}

/// An `NSArray` of these objects (autoreleased).
pub(crate) fn ns_array(objects: &[Obj]) -> Obj {
    // SAFETY: the array retains the objects it is given.
    unsafe { send(class(c"NSArray"), c"arrayWithObjects:count:", (objects.as_ptr(), objects.len())) }
}

/// An `NSArray`'s objects.
pub(crate) fn array_items(array: Obj) -> Vec<Obj> {
    // SAFETY: nil answers 0; each index is below the count.
    let count: usize = unsafe { send(array, c"count", ()) };
    (0..count).map(|at| unsafe { send(array, c"objectAtIndex:", (at,)) }).collect()
}

#[link(name = "System")]
unsafe extern "C" {
    /// The class of blocks that live for good, which copying leaves be.
    static _NSConcreteGlobalBlock: [usize; 0];
}

/// A block's descriptor: its size.
#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// An Objective-C block as Clang lays one out, with what its function reads (`held`) after the
/// header: what the system calls back. Its function takes the block, then the call's arguments.
#[repr(C)]
pub(crate) struct Block<T> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    pub(crate) invoke: *const c_void,
    descriptor: *const BlockDescriptor,
    pub(crate) held: T,
}

impl<T> Block<T> {
    /// A block calling `invoke` with `held`. It lives for good (a few bytes each): the system
    /// may call or copy it after the caller is gone, and copying a global block keeps it.
    pub(crate) fn leaked(invoke: *const c_void, held: T) -> &'static Block<T> {
        /// `BLOCK_IS_GLOBAL`.
        const GLOBAL: i32 = 1 << 28;
        let descriptor = &const { BlockDescriptor { reserved: 0, size: size_of::<Block<T>>() } };
        let isa = (&raw const _NSConcreteGlobalBlock).cast();
        Box::leak(Box::new(Block { isa, flags: GLOBAL, reserved: 0, invoke, descriptor, held }))
    }
}

#[cfg(any(target_os = "ios", feature = "video"))]
#[link(name = "objc")]
unsafe extern "C" {
    fn class_createInstance(class: Obj, extra: usize) -> Obj;
    fn object_getIndexedIvars(object: Obj) -> *mut c_void;
}

/// A new object of a class of the library's with `extra` kept beside it, in its extra bytes
/// (retained: the caller releases or autoreleases it).
#[cfg(any(target_os = "ios", feature = "video"))]
pub(crate) fn new_with_extra<T: Copy>(class: Obj, extra: T) -> Obj {
    // SAFETY: an object with room for a `T` beside it, written once.
    unsafe {
        let made = class_createInstance(class, size_of::<T>());
        object_getIndexedIvars(made).cast::<T>().write(extra);
        made
    }
}

/// What an object made by `new_with_extra` keeps beside it.
///
/// # Safety
/// `object` was made by `new_with_extra` with a `T`.
#[cfg(any(target_os = "ios", feature = "video"))]
pub(crate) unsafe fn extra_of<T: Copy>(object: Obj) -> T {
    // SAFETY: as the caller promises.
    unsafe { object_getIndexedIvars(object).cast::<T>().read() }
}

/// An autorelease pool, drained where it ends.
pub(crate) struct Pool(*mut c_void);

impl Pool {
    /// A pool for what is made from now until this ends.
    pub(crate) fn new() -> Pool {
        // SAFETY: pools are pushed and popped in order on one thread.
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the pool this pushed.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}
