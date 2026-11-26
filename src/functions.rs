//! Tables of functions found by name at run time, as Vulkan's and OpenGL's loaders give them
//! (`vkGetInstanceProcAddr`, `eglGetProcAddress`, `wglGetProcAddress`): nothing links to either.
//! A system library's table (`dynamic_library::library!`) is filled by the same loop (`found_all`).
// Every function here is `#[inline]`: only a program that calls one compiles it.
use std::ffi::{CStr, c_void};

/// Declares a table of functions, each found by name (an entry may carry an attribute, as
/// `#[cfg(test)]`). The table is laid out as the list of their addresses: one loop finds them.
macro_rules! functions {
    ($set:ident { $($(#[$only:meta])? $name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)* }) => {
        /// Functions found by name.
        #[allow(non_snake_case)] // (Their own names.)
        #[repr(C)]
        pub(crate) struct $set {
            $($(#[$only])? pub(crate) $name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)*
        }
        impl $set {
            /// Each function, found through `find`; or the name of the first one missing.
            #[inline]
            pub(crate) fn load(find: &mut dyn FnMut(&std::ffi::CStr) -> *const std::ffi::c_void) -> Result<$set, &'static str> {
                const COUNT: usize = [$($(#[$only])? stringify!($name),)*].len();
                let names: [&'static str; COUNT] = [$($(#[$only])? concat!(stringify!($name), "\0"),)*];
                let mut found = [std::ptr::null::<std::ffi::c_void>(); COUNT];
                $crate::functions::found_all(find, &mut names.iter().copied(), &mut found)?;
                // SAFETY: the table is `repr(C)`, a function pointer for each name in order, each
                // declared as its header declares it; every address found is one (not null).
                Ok(unsafe { std::mem::transmute::<[*const std::ffi::c_void; COUNT], $set>(found) })
            }
        }
    };
}
pub(crate) use functions;

/// The functions `names` (each ending in a zero) as `find` gives them, into `found`; or the name
/// of the first one it gives none for.
#[inline]
pub(crate) fn found_all(find: &mut dyn FnMut(&CStr) -> *const c_void, names: &mut dyn Iterator<Item = &'static str>, found: &mut [*const c_void]) -> Result<(), &'static str> {
    for (name, slot) in names.zip(found) {
        *slot = self::found(find, name)?;
    }
    Ok(())
}

/// The function `name` (ending in a zero) as `find` gives it; or its name, if it gives none.
#[inline]
pub(crate) fn found(find: &mut dyn FnMut(&CStr) -> *const c_void, name: &'static str) -> Result<*const c_void, &'static str> {
    let found = find(CStr::from_bytes_with_nul(name.as_bytes()).unwrap_or_default());
    if found.is_null() { Err(name.trim_end_matches('\0')) } else { Ok(found) }
}

#[cfg(test)]
mod tests {
    use super::*;

    functions!(Pair {
        first: fn() -> u32;
        #[cfg(test)]
        second: fn() -> u32;
    });

    extern "system" fn one() -> u32 {
        1
    }

    extern "system" fn two() -> u32 {
        2
    }

    #[test]
    fn a_table_holds_each_function_found_by_its_name_and_says_which_one_is_missing() {
        let mut find = |name: &CStr| match name.to_bytes() {
            b"first" => one as *const c_void,
            b"second" => two as *const c_void,
            _ => std::ptr::null(),
        };
        let pair = Pair::load(&mut find).expect("both found");
        // SAFETY: each is the function its name says.
        assert_eq!(unsafe { ((pair.first)(), (pair.second)()) }, (1, 2));
        let mut lacking = |name: &CStr| if name == c"second" { std::ptr::null() } else { one as *const c_void };
        assert_eq!(Pair::load(&mut lacking).err(), Some("second"));
    }
}
