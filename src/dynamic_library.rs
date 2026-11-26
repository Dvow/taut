//! System libraries opened by name at run time (`dlopen`), on Linux, the BSDs and Android: a
//! program builds without their headers and starts where one is missing.
//!
//! `library!` declares a library's functions as fields of a struct, each found by its own name
//! when the library opens:
//!
//! ```ignore
//! library!(Xkb, ["libxkbcommon.so.0", "libxkbcommon.so"], {
//!     fn xkb_context_new(c_int) -> *mut c_void;
//! });
//! let xkb = Xkb::open()?;
//! ```
use std::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    pub(crate) fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(library: *mut c_void) -> c_int;
}

/// `RTLD_NOW`: every function is found as the library opens.
pub(crate) const NOW: c_int = 2;

/// One of a library's functions by name (ending in a zero).
pub(crate) fn symbol(library: *mut c_void, name: &str) -> Option<*mut c_void> {
    debug_assert!(name.ends_with('\0'), "a name ending in a zero");
    // SAFETY: an open library, and a name ending in a zero.
    let found = unsafe { dlsym(library, name.as_ptr().cast()) };
    (!found.is_null()).then_some(found)
}

/// An open library, closed when dropped. Its functions are called through what `symbol` found:
/// those stay valid while it is open.
pub(crate) struct Library(*mut c_void);

// SAFETY: `dlsym` and `dlclose` may be called from any thread.
unsafe impl Send for Library {}
// SAFETY: as above; the handle itself is never changed.
unsafe impl Sync for Library {}

impl Library {
    /// The first of these names (each ending in a zero) that opens.
    pub(crate) fn open(names: &[&str]) -> Option<Library> {
        // SAFETY: each name ends in a zero.
        let opened = names.iter().map(|name| unsafe { dlopen(name.as_ptr().cast(), NOW) });
        opened.into_iter().find(|library| !library.is_null()).map(Library)
    }

    /// One of its functions by name (ending in a zero).
    pub(crate) fn symbol(&self, name: &str) -> Option<*mut c_void> {
        symbol(self.0, name)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: opened by `open`; nothing found in it is called after this.
        unsafe { dlclose(self.0) };
    }
}

/// Declares a system library as a struct of its functions, found by name when it is opened
/// (`open`, none where the library or one of its functions is missing). The library closes
/// when the struct is dropped. `...` stands for variadic arguments.
macro_rules! library {
    ($name:ident, [$($file:literal),+], { $(fn $f:ident $args:tt $(-> $r:ty)?;)* }) => {
        #[allow(non_snake_case)]
        struct $name {
            $($f: library!(@fn $args $(-> $r)?),)*
            #[allow(dead_code, reason = "kept open while its functions are called")]
            library: $crate::dynamic_library::Library,
        }
        impl $name {
            #[allow(non_snake_case)]
            fn open() -> Option<Self> {
                let library = $crate::dynamic_library::Library::open(&[$(concat!($file, "\0")),+])?;
                // SAFETY: each function is declared as its library documents it.
                $(let $f = unsafe { std::mem::transmute::<*mut std::ffi::c_void, library!(@fn $args $(-> $r)?)>(library.symbol(concat!(stringify!($f), "\0"))?) };)*
                Some($name { $($f,)* library })
            }
        }
    };
    // One function's type.
    (@fn ($($t:ty),*) $(-> $r:ty)?) => { unsafe extern "C" fn($($t),*) $(-> $r)? };
    (@fn ($($t:ty),*, ...) $(-> $r:ty)?) => { unsafe extern "C" fn($($t),*, ...) $(-> $r)? };
}
pub(crate) use library;

#[cfg(test)]
mod tests {
    use std::ffi::c_char;

    // The C library's own `strlen`, through a library opened the same way as any other.
    library!(C, ["libc.so.6", "libc.so"], {
        fn strlen(*const c_char) -> usize;
        fn snprintf(*mut c_char, usize, *const c_char, ...) -> i32;
    });

    library!(Missing, ["libtaut-none.so.1"], {
        fn strlen(*const c_char) -> usize;
    });

    library!(MissingFunction, ["libc.so.6", "libc.so"], {
        fn taut_no_such_function() -> i32;
    });

    #[test]
    fn a_library_opens_and_its_functions_are_called() {
        let c = C::open().expect("the C library");
        // SAFETY: C strings, and room for what is written.
        unsafe {
            assert_eq!((c.strlen)(c"four".as_ptr()), 4);
            let mut written = [0 as c_char; 16];
            (c.snprintf)(written.as_mut_ptr(), written.len(), c"%d-%s".as_ptr(), 7, c"x".as_ptr());
            assert_eq!(std::ffi::CStr::from_ptr(written.as_ptr()), c"7-x");
        }
    }

    #[test]
    fn a_missing_library_or_function_opens_nothing() {
        assert!(Missing::open().map(|opened| opened.strlen).is_none());
        assert!(MissingFunction::open().map(|opened| opened.taut_no_such_function).is_none());
    }
}
