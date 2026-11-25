//! The library inside a DLL, used by a program knowing only a few C functions: the `overlay`
//! example is loaded as any program loads a DLL, driven, and inspected.
#![cfg(any(windows, unix))]
use std::ffi::{CString, c_void};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(file: *const u16) -> *mut c_void;
    fn GetProcAddress(library: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}
#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(file: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

/// Loads the library in `file`.
fn load(file: &std::path::Path) -> *mut c_void {
    #[cfg(windows)]
    let library = unsafe { LoadLibraryW(file.to_string_lossy().encode_utf16().chain([0]).collect::<Vec<u16>>().as_ptr()) };
    #[cfg(unix)]
    let library = unsafe { dlopen(CString::new(file.to_string_lossy().as_bytes()).unwrap().as_ptr(), 2) };
    assert!(!library.is_null(), "{} could not be loaded: build it with `cargo build -p taut --example overlay`", file.display());
    library
}

/// One of the library's functions, by name.
fn function(library: *mut c_void, name: &str) -> *mut c_void {
    let name = CString::new(name).unwrap();
    #[cfg(windows)]
    let found = unsafe { GetProcAddress(library, name.as_ptr()) };
    #[cfg(unix)]
    let found = unsafe { dlsym(library, name.as_ptr()) };
    assert!(!found.is_null(), "the library has no function called {name:?}");
    found
}

#[test]
fn a_program_drives_an_interface_that_lives_in_a_dll() {
    // The example is built in `<target>/<profile>/examples`, some levels above this test's own
    // binary (how many depends on cargo's build directory layout).
    let name = if cfg!(windows) { "overlay.dll" } else { "liboverlay.so" };
    let exe = std::env::current_exe().unwrap();
    let file = exe.ancestors().map(|dir| dir.join("examples").join(name)).find(|file| file.exists()).unwrap_or_else(|| exe.with_file_name(name));
    let library = load(&file);
    // SAFETY: each function is declared as the example declares it.
    type Asks = unsafe extern "C" fn(*const c_void) -> u32;
    let (open, frame, key, presses, close) = unsafe {
        (
            std::mem::transmute::<*mut c_void, extern "C" fn(u32, u32, f32) -> *mut c_void>(function(library, "overlay_open")),
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, *mut [i32; 4]) -> *const u32>(function(library, "overlay_frame")),
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, u32)>(function(library, "overlay_key")),
            std::mem::transmute::<*mut c_void, Asks>(function(library, "overlay_presses")),
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(function(library, "overlay_close")),
        )
    };
    let (pointer, wants_pointer, wants_keyboard) = unsafe {
        (
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, f32, f32)>(function(library, "overlay_pointer")),
            std::mem::transmute::<*mut c_void, Asks>(function(library, "overlay_wants_pointer")),
            std::mem::transmute::<*mut c_void, Asks>(function(library, "overlay_wants_keyboard")),
        )
    };
    let (wide, high) = (300usize, 240usize);
    let overlay = open(wide as u32, high as u32, 1.0);
    let mut changed = [0; 4];
    // SAFETY: the overlay is open, and its picture is `wide` by `high` pixels.
    unsafe {
        let picture = std::slice::from_raw_parts(frame(overlay, &mut changed), wide * high);
        assert!(changed[0] <= 16 && changed[2] >= 236 && changed[2] < wide as i32, "the first picture is new where the panel and its shadow are, and nowhere else: {changed:?}");
        assert_eq!(picture[2 * wide + 2], 0, "outside its panel the overlay is clear: what is under it shows");
        assert!(picture[40 * wide + 120] >> 24 > 0xd0, "its panel is nearly solid: {:08x}", picture[40 * wide + 120]);
        assert!(picture.iter().any(|p| *p >> 24 == 0xff), "and its letters are solid");

        frame(overlay, &mut changed);
        assert_eq!(changed, [0; 4], "a frame in which nothing happened costs nothing and changes nothing");

        // The program asks whose the mouse is: the overlay's over its panel, the program's
        // own beside it. And the keyboard is the program's until it is on a part.
        assert_eq!((wants_pointer(overlay), wants_keyboard(overlay)), (0, 0));
        for (x, y, wanted) in [(120.0, 40.0, 1), (280.0, 220.0, 0)] {
            pointer(overlay, x, y);
            frame(overlay, &mut changed);
            assert_eq!(wants_pointer(overlay), wanted, "with the mouse at {x}, {y}");
        }

        // Tab goes to the switch, the slider, the button; Space presses the button.
        for pressed in [0, 0, 0, 1] {
            key(overlay, pressed);
            frame(overlay, &mut changed);
        }
        assert_eq!((presses(overlay), wants_keyboard(overlay)), (1, 1));
        assert!(changed[2] > changed[0] && changed[3] > changed[1] && changed[2] - changed[0] < wide as i32, "only part of the picture changed: {changed:?}");
        close(overlay);
    }
}
