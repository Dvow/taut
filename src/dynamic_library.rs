//! System libraries opened by name at run time (`dlopen`), on Linux, the BSDs and Android: a
//! program builds without their headers and starts where one is missing.
//!
//! `library!` declares a library's functions as a struct, found by name (`functions::found_all`)
//! when the library opens:
//!
//! ```ignore
//! library!(Xkb, ["libxkbcommon.so.0", "libxkbcommon.so"], {
//!     fn xkb_context_new(c_int) -> *mut c_void;
//! });
//! let xkb = Xkb::open()?;
//! ```
//!
//! A library closes when its struct is dropped. One that must never be unloaded (a GPU driver,
//! a library registering GLib types) is kept in a `static`.
use std::ffi::{CStr, c_char, c_int, c_void};

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(library: *mut c_void) -> c_int;
}

/// `RTLD_NOW`: every function is found as the library opens.
const NOW: c_int = 2;

/// An open library, closed when dropped. Its functions are called through what `symbol` found:
/// those stay valid while it is open.
pub(crate) struct Library(*mut c_void);

// SAFETY: `dlsym` and `dlclose` may be called from any thread.
unsafe impl Send for Library {}
// SAFETY: as above; the handle itself is never changed.
unsafe impl Sync for Library {}

impl Library {
    /// The first of `files` that opens (each name ending in a zero, one after another).
    pub(crate) fn open(files: &str) -> Option<Library> {
        let mut names = files.split_inclusive('\0').filter_map(|file| CStr::from_bytes_with_nul(file.as_bytes()).ok());
        // SAFETY: a name ending in a zero.
        names.find_map(|name| Some(unsafe { dlopen(name.as_ptr(), NOW) }).filter(|library| !library.is_null())).map(Library)
    }

    /// One of its functions by name; nothing where it has none.
    pub(crate) fn symbol(&self, name: &CStr) -> Option<*const c_void> {
        // SAFETY: an open library, and a name ending in a zero.
        let found = unsafe { dlsym(self.0, name.as_ptr()) };
        (!found.is_null()).then_some(found.cast_const())
    }

    /// Finds `names` (each ending in a zero, one after another) into `found`, in order;
    /// nothing where one is missing.
    pub(crate) fn find_all(&self, names: &'static str, found: &mut [*const c_void]) -> Option<()> {
        crate::functions::found_all(&mut |name| self.symbol(name).unwrap_or(std::ptr::null()), &mut names.split_inclusive('\0'), found).ok()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: opened by `open`; nothing found in it is called after this.
        unsafe { dlclose(self.0) };
    }
}

/// Declares a system library as a struct of its functions (`vis` its visibility and theirs),
/// found by name when it is opened (`open`; none where the library or one of its functions is
/// missing). The library closes when the struct is dropped. `...` stands for variadic
/// arguments.
macro_rules! library {
    ($vis:vis $name:ident, [$($file:literal),+], { $(fn $function:ident $arguments:tt $(-> $result:ty)?;)* }) => {
        /// The library's functions, and the library, open while they are.
        #[allow(non_snake_case)] // (The library's own names.)
        $vis struct $name {
            $($vis $function: library!(@fn $arguments $(-> $result)?),)*
            #[allow(dead_code, reason = "kept open while its functions are called")]
            library: $crate::dynamic_library::Library,
        }
        impl $name {
            /// The library opened, and its functions; nothing where it or one of them is missing.
            #[allow(non_snake_case)] // (The library's own names.)
            $vis fn open() -> Option<$name> {
                const COUNT: usize = [$(stringify!($function)),*].len();
                let library = $crate::dynamic_library::Library::open(concat!($($file, "\0"),+))?;
                let mut found = [std::ptr::null(); COUNT];
                library.find_all(concat!($(stringify!($function), "\0"),*), &mut found)?;
                let [$($function),*] = found;
                // SAFETY: each function was found (not null) and is declared as its library's
                // header declares it.
                Some($name { $($function: unsafe { std::mem::transmute::<*const std::ffi::c_void, library!(@fn $arguments $(-> $result)?)>($function) },)* library })
            }
        }
    };
    // One function's type.
    (@fn ($($argument:ty),*) $(-> $result:ty)?) => { unsafe extern "C" fn($($argument),*) $(-> $result)? };
    (@fn ($($argument:ty),*, ...) $(-> $result:ty)?) => { unsafe extern "C" fn($($argument),*, ...) $(-> $result)? };
}
pub(crate) use library;

#[cfg(test)]
mod tests {
    use std::ffi::c_char;

    // The C library's own `strlen` and `snprintf`, through a library opened the same way as any
    // other.
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
