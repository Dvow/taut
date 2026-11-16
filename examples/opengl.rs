//! taut under OpenGL. The program renders with OpenGL as usual, and the interface lies over it:
//! `taut::layer_opengl::Layer`, created once in the program's context, draws it with one call
//! per frame, on any OpenGL 3.3 or newer. Around it is the plainest possible program: a window
//! with a core profile context (WGL on Windows, EGL on X11 on Linux), and the window's events
//! forwarded to the interface.
//!   cargo run -p taut --example opengl
//!
//! Only system libraries, with OpenGL functions loaded by name: each declared here as
//! glcorearb.h declares it. (A program using a windowing library gives the layer that library's
//! loader function.)
use std::ffi::{CStr, c_void};
use std::time::{Duration, Instant};
use taut::geom::Size;
use taut::layer_opengl::Layer;
use taut::{Color, Event, Length, Ui};

/// Declares a set of OpenGL's functions, which are found by name once a context is current.
macro_rules! functions {
    ($set:ident { $($name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)* }) => {
        /// Functions of OpenGL, found by name.
        #[allow(non_snake_case)]
        pub struct $set {
            $(pub $name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)*
        }
        impl $set {
            /// Finds each function with `find`, which is handed its name.
            pub fn load(find: &mut dyn FnMut(&std::ffi::CStr) -> *const c_void) -> $set {
                let mut found = |name: &str| Some(find(std::ffi::CStr::from_bytes_with_nul(name.as_bytes()).unwrap())).filter(|found| !found.is_null()).unwrap_or_else(|| panic!("this OpenGL has no {name}"));
                // SAFETY: each function is declared as OpenGL declares it.
                $set { $($name: unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn($($arg),*) $(-> $out)?>(found(concat!(stringify!($name), "\0"))) },)* }
            }
        }
    };
}

// What the program draws of its own: a colour.
functions!(Gl {
    glViewport: fn(i32, i32, i32, i32);
    glClearColor: fn(f32, f32, f32, f32);
    glClear: fn(u32);
});

/// What the program's own parts are set to.
struct App {
    still: bool,
    speed: f32,
    name: String,
}

fn describe(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().w(Length::Px(260.0)).at(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.alpha(0.9)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
    ui.text("Over OpenGL", t.font.bold(), t.text);
    ui.switch(&mut app.still, "Hold the colour still");
    ui.slider(&mut app.speed, 0.0..=4.0);
    ui.text_box(&mut app.name, "Type here");
    ui.note(&format!("{} letters typed", app.name.chars().count()));
    ui.close();
}

/// The program: its picture, a colour that moves, and its interface over it.
pub struct Program {
    pub ui: Ui,
    pub layer: Layer,
    gl: Gl,
    app: App,
    /// The window's size, in pixels.
    size: [i32; 2],
    /// Something happened that the interface has not run for.
    waiting: bool,
    started: Instant,
    hue: f32,
}

impl Program {
    /// The program, in the context that is current, whose functions `find` gives.
    pub fn new(find: &mut dyn FnMut(&CStr) -> *const c_void, mut ui: Ui) -> Program {
        ui.theme.bg = Color::NONE;
        // SAFETY: the program's context is current, and `find` is its way to its functions.
        let layer = unsafe { Layer::new(find) }.unwrap_or_else(|failed| panic!("{failed}"));
        let app = App { still: false, speed: 1.0, name: String::new() };
        Program { ui, layer, gl: Gl::load(find), app, size: [1, 1], waiting: true, started: Instant::now(), hue: 0.0 }
    }

    /// The window is now `width` by `height` pixels, with `scale` pixels to a unit.
    pub fn resize(&mut self, width: usize, height: usize, scale: f32) {
        self.ui.resize(Size { w: width as f32 / scale, h: height as f32 / scale }, scale);
        (self.size, self.waiting) = ([width as i32, height as i32], true);
    }

    /// Something the person did in the window.
    pub fn hear(&mut self, event: Event) {
        self.waiting |= self.ui.event(event);
    }

    /// One frame: the interface runs if something happened (or something in it moves); then
    /// the program's own picture, and the interface over it, into the framebuffer that is bound.
    pub fn frame(&mut self) {
        self.ui.set_time(self.started.elapsed());
        if std::mem::take(&mut self.waiting) || self.ui.wake_in() == Some(Duration::ZERO) {
            let app = &mut self.app;
            self.ui.run(&mut |ui| describe(app, ui));
        }
        self.hue += if self.app.still { 0.0 } else { self.app.speed / 120.0 };
        let (gl, hue) = (&self.gl, self.hue);
        // SAFETY: the context the layer was made in is current: this program has only the one.
        // (0x4000: the colour of every pixel.)
        let drawn = unsafe {
            (gl.glViewport)(0, 0, self.size[0], self.size[1]);
            (gl.glClearColor)(0.3 + 0.25 * hue.sin(), 0.3 + 0.25 * (hue + 2.1).sin(), 0.3 + 0.25 * (hue + 4.2).sin(), 1.0);
            (gl.glClear)(0x4000);
            self.layer.draw(&mut self.ui)
        };
        drawn.unwrap_or_else(|failed| panic!("{failed}"));
    }
}

fn main() {
    #[cfg(windows)]
    wgl::run();
    #[cfg(all(unix, not(target_os = "macos")))]
    egl::run();
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    println!("this example opens its window through WGL on Windows and through EGL on X11: there is neither here");
}

/// The program on Windows: a window, and a context on it through WGL.
#[cfg(windows)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
pub mod wgl {
    use super::Program;
    use std::cell::RefCell;
    use std::ffi::{CStr, c_void};
    use std::ptr::null;
    use taut::Ui;
    use taut::platform::{Messages, SystemClipboard};

    #[repr(C)]
    struct PIXELFORMATDESCRIPTOR {
        size: u16,
        version: u16,
        flags: u32,
        pixel_type: u8,
        colour_bits: u8,
        /// Bits and shifts of red, green, blue and alpha, the accumulation buffer's, depth,
        /// stencil, auxiliary buffers, the layer, one byte left over.
        bits: [u8; 18],
        masks: [u32; 3],
    }
    #[repr(C)]
    struct WNDCLASSW {
        style: u32,
        proc: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
        class_extra: i32,
        window_extra: i32,
        instance: isize,
        icon: isize,
        cursor: isize,
        background: isize,
        menu: *const u16,
        name: *const u16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct MSG {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
    }
    #[repr(C)]
    struct TRACKMOUSEEVENT {
        size: u32,
        flags: u32,
        hwnd: isize,
        hover_time: u32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WNDCLASSW) -> u16;
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize, instance: isize, param: *const c_void) -> isize;
        fn DefWindowProcW(hwnd: isize, msg: u32, w: usize, l: isize) -> isize;
        fn PeekMessageW(msg: *mut MSG, hwnd: isize, first: u32, last: u32, remove: u32) -> i32;
        fn TranslateMessage(msg: *const MSG) -> i32;
        fn DispatchMessageW(msg: *const MSG) -> isize;
        fn PostQuitMessage(code: i32);
        fn LoadCursorW(instance: isize, name: *const u16) -> isize;
        fn TrackMouseEvent(track: *mut TRACKMOUSEEVENT) -> i32;
        fn SetProcessDpiAwarenessContext(context: isize) -> i32;
        fn GetDpiForWindow(hwnd: isize) -> u32;
        fn GetDC(hwnd: isize) -> isize;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn ChoosePixelFormat(dc: isize, format: *const PIXELFORMATDESCRIPTOR) -> i32;
        fn SetPixelFormat(dc: isize, format: i32, described: *const PIXELFORMATDESCRIPTOR) -> i32;
        fn SwapBuffers(dc: isize) -> i32;
    }
    #[link(name = "opengl32")]
    unsafe extern "system" {
        fn wglCreateContext(dc: isize) -> isize;
        fn wglMakeCurrent(dc: isize, context: isize) -> i32;
        fn wglDeleteContext(context: isize) -> i32;
        fn wglGetProcAddress(name: *const u8) -> *const c_void;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(library: isize, name: *const u8) -> *const c_void;
    }
    const WM_DESTROY: u32 = 0x0002;
    const WM_SIZE: u32 = 0x0005;
    const WM_QUIT: u32 = 0x0012;
    const WM_MOUSEMOVE: u32 = 0x0200;

    /// An OpenGL function by name: the newer ones from the driver, those of version 1.1 from
    /// opengl32.dll itself (for which the driver gives nothing, or 1, 2, 3 or -1).
    pub fn find(name: &CStr) -> *const c_void {
        // SAFETY: the name ends in a zero, and a context is current.
        let found = unsafe { wglGetProcAddress(name.as_ptr().cast()) };
        if !matches!(found as isize, -1..=3) {
            return found;
        }
        let library: Vec<u16> = "opengl32.dll\0".encode_utf16().collect();
        unsafe { GetProcAddress(GetModuleHandleW(library.as_ptr()), name.as_ptr().cast()) }
    }

    /// A context of OpenGL 3.3's core profile on the window, current on this thread; and the
    /// window's device context, which is what is shown with `SwapBuffers`. A context of the
    /// old kind is made first: only through one can the newer kind be asked for.
    pub fn context(window: isize) -> isize {
        let mut bits = [0u8; 18];
        bits[6] = 8; // alpha
        let format = PIXELFORMATDESCRIPTOR { size: 40, version: 1, flags: 0x25, pixel_type: 0, colour_bits: 32, bits, masks: [0; 3] };
        // The newer kind: version 3.3 (0x2091, 0x2092), the core profile (0x9126: 1).
        let asked = [0x2091, 3, 0x2092, 3, 0x9126, 1, 0];
        type Make = unsafe extern "system" fn(isize, isize, *const i32) -> isize;
        // SAFETY: the window is alive; each context is made current before it is used.
        unsafe {
            let dc = GetDC(window);
            assert!(SetPixelFormat(dc, ChoosePixelFormat(dc, &format), &format) != 0, "the window takes no pixels for OpenGL");
            let old = wglCreateContext(dc);
            wglMakeCurrent(dc, old);
            let make = find(c"wglCreateContextAttribsARB");
            assert!(!make.is_null(), "this OpenGL cannot make a context of the core profile");
            let context = std::mem::transmute::<*const c_void, Make>(make)(dc, 0, asked.as_ptr());
            assert!(context != 0, "this OpenGL has no version 3.3 of the core profile");
            wglMakeCurrent(dc, context);
            wglDeleteContext(old);
            dc
        }
    }

    thread_local! {
        /// The messages the window got since they were last looked at.
        static HEARD: RefCell<Vec<(u32, usize, isize)>> = const { RefCell::new(Vec::new()) };
    }

    /// What Windows calls with each message for the window: it is kept, for the loop below.
    unsafe extern "system" fn keep(hwnd: isize, msg: u32, w: usize, l: isize) -> isize {
        HEARD.with_borrow_mut(|heard| heard.push((msg, w, l)));
        // (Windows says when the pointer leaves the window only to a window that asks.)
        let mut leave = TRACKMOUSEEVENT { size: size_of::<TRACKMOUSEEVENT>() as u32, flags: 2, hwnd, hover_time: 0 };
        // SAFETY: the window is this one; the record is filled in as Windows wants it.
        match msg {
            WM_DESTROY => unsafe { PostQuitMessage(0) },
            WM_MOUSEMOVE => _ = unsafe { TrackMouseEvent(&mut leave) },
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    /// Hands the window every message that is waiting for it; false once the program is to end.
    fn pump() -> bool {
        let mut message = MSG::default();
        // SAFETY: messages of this thread's window, handed back to Windows to be sent on.
        while unsafe { PeekMessageW(&mut message, 0, 0, 0, 1) } != 0 {
            if message.message == WM_QUIT {
                return false;
            }
            unsafe { (TranslateMessage(&message), DispatchMessageW(&message)) };
        }
        true
    }

    pub fn run() {
        let (class, title): (Vec<u16>, Vec<u16>) = ("taut-opengl\0".encode_utf16().collect(), "taut over OpenGL\0".encode_utf16().collect());
        // SAFETY: the class and the window are made once, with names that end in a zero; the
        // pixels of this program are the screen's own (-4: per monitor, the second way). The
        // window's device context is its own (0x20), as OpenGL wants it.
        let window = unsafe {
            SetProcessDpiAwarenessContext(-4);
            let instance = GetModuleHandleW(null());
            let arrow = LoadCursorW(0, 32512 as *const u16);
            RegisterClassW(&WNDCLASSW { style: 0x20, proc: keep, class_extra: 0, window_extra: 0, instance, icon: 0, cursor: arrow, background: 0, menu: null(), name: class.as_ptr() });
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0x10CF_0000, i32::MIN, i32::MIN, 900, 600, 0, 0, instance, null())
        };
        let dc = context(window);
        let scale = unsafe { GetDpiForWindow(window) } as f32 / 96.0;
        let mut program = Program::new(&mut |name| find(name), Ui::new(taut::platform::system_text(), Box::new(SystemClipboard)));
        let mut messages = Messages::of(window);
        while pump() {
            for (msg, w, l) in HEARD.take() {
                let (width, height) = (l as usize & 0xffff, l as usize >> 16 & 0xffff);
                if msg == WM_SIZE && width > 0 && height > 0 {
                    program.resize(width, height, scale);
                }
                // SAFETY: the message comes straight from the window procedure, with what
                // Windows sent along with it.
                unsafe { messages.events(msg, w, l, scale, |event| program.hear(event)) };
            }
            program.frame();
            unsafe { SwapBuffers(dc) };
        }
    }
}

/// Declares a system library's functions, found by name when it is opened.
#[cfg(all(unix, not(target_os = "macos")))]
macro_rules! library {
    ($set:ident, $file:literal, { $($name:ident: fn($($arg:ty),*) -> $out:ty;)* }) => {
        /// Functions of a system library, found by name.
        #[allow(non_snake_case)]
        pub struct $set {
            $(pub $name: unsafe extern "C" fn($($arg),*) -> $out,)*
        }
        impl $set {
            /// The library, if the system has it. (It stays open while the program runs.)
            pub fn open() -> Option<$set> {
                // SAFETY: the names end in a zero, and each function is declared as its
                // header declares it.
                let library = unsafe { dlopen(concat!($file, "\0").as_ptr().cast(), 2) };
                (!library.is_null()).then_some(())?;
                let found = |name: &str| Some(unsafe { dlsym(library, name.as_ptr().cast()) }).filter(|found| !found.is_null());
                Some($set { $($name: unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> $out>(found(concat!(stringify!($name), "\0"))?) },)* })
            }
        }
    };
}

/// The program on Linux: a window of X's, and a context on it through EGL. Both libraries
/// are opened by name, and declared as EGL/egl.h and X11/Xlib.h declare them.
#[cfg(all(unix, not(target_os = "macos")))]
pub mod egl {
    use super::Program;
    use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
    use std::ptr::{null, null_mut};
    use taut::geom::pt;
    use taut::{Button, Event, Key, LocalClipboard, Mods, Ui};

    unsafe extern "C" {
        pub fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
        pub fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    library!(Egl, "libEGL.so.1", {
        eglGetPlatformDisplay: fn(c_uint, *mut c_void, *const isize) -> *mut c_void;
        eglInitialize: fn(*mut c_void, *mut i32, *mut i32) -> c_uint;
        eglBindAPI: fn(c_uint) -> c_uint;
        eglChooseConfig: fn(*mut c_void, *const i32, *mut *mut c_void, i32, *mut i32) -> c_uint;
        eglCreatePlatformWindowSurface: fn(*mut c_void, *mut c_void, *mut c_void, *const isize) -> *mut c_void;
        eglCreateContext: fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void;
        eglMakeCurrent: fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> c_uint;
        eglSwapBuffers: fn(*mut c_void, *mut c_void) -> c_uint;
        eglGetProcAddress: fn(*const c_char) -> *const c_void;
    });

    library!(Xlib, "libX11.so.6", {
        XOpenDisplay: fn(*const c_char) -> *mut c_void;
        XDefaultRootWindow: fn(*mut c_void) -> c_ulong;
        XCreateSimpleWindow: fn(*mut c_void, c_ulong, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong) -> c_ulong;
        XSelectInput: fn(*mut c_void, c_ulong, c_long) -> c_int;
        XStoreName: fn(*mut c_void, c_ulong, *const c_char) -> c_int;
        XInternAtom: fn(*mut c_void, *const c_char, c_int) -> c_ulong;
        XSetWMProtocols: fn(*mut c_void, c_ulong, *mut c_ulong, c_int) -> c_int;
        XMapWindow: fn(*mut c_void, c_ulong) -> c_int;
        XPending: fn(*mut c_void) -> c_int;
        XNextEvent: fn(*mut c_void, *mut XEvent) -> c_int;
        XLookupString: fn(*mut XEvent, *mut c_char, c_int, *mut c_ulong, *mut c_void) -> c_int;
    });

    /// X's display, and what EGL says of it (EGL/eglext.h: EGL_PLATFORM_X11_KHR).
    pub const PLATFORM_X11: c_uint = 0x31D5;
    /// No display at all: Mesa's, which draws into what the program makes for itself
    /// (EGL_PLATFORM_SURFACELESS_MESA).
    pub const PLATFORM_SURFACELESS: c_uint = 0x31DD;

    /// An OpenGL context of the core profile, version 3.3, current on this thread.
    pub struct Context {
        pub egl: Egl,
        pub display: *mut c_void,
        /// What is drawn into and shown: the window's (nothing, where there is no window).
        pub surface: *mut c_void,
    }

    impl Context {
        /// An OpenGL function by name.
        pub fn find(&self, name: &CStr) -> *const c_void {
            // SAFETY: the name ends in a zero.
            unsafe { (self.egl.eglGetProcAddress)(name.as_ptr()) }
        }
    }

    /// A context on `platform`, with its display (X's, or none), and on the window, where
    /// there is one. Nothing, where EGL has none to give.
    // (The display handed in is the program's own, handed on to EGL as the program would.)
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn context(platform: c_uint, native: *mut c_void, window: Option<c_ulong>) -> Option<Context> {
        let egl = Egl::open()?;
        // Pixels of 8 bits a colour (0x3024, 0x3023, 0x3022), for OpenGL (0x3040: 8), for a
        // window (0x3033: 4) or for none (1). Version 3.3 (0x3098, 0x30FB), of the core
        // profile (0x30FD: 1). Every list ends with 0x3038.
        let wanted = [0x3033, if window.is_some() { 4 } else { 1 }, 0x3040, 8, 0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3038];
        let asked = [0x3098, 3, 0x30FB, 3, 0x30FD, 1, 0x3038];
        let (mut config, mut configs, mut window) = (null_mut(), 0, window);
        // SAFETY: what is handed back is checked before it is used; the window is X's.
        unsafe {
            let display = (egl.eglGetPlatformDisplay)(platform, native, null());
            let ready = !display.is_null() && (egl.eglInitialize)(display, null_mut(), null_mut()) != 0 && (egl.eglBindAPI)(0x30A2) != 0;
            (ready && (egl.eglChooseConfig)(display, wanted.as_ptr(), &mut config, 1, &mut configs) != 0 && configs == 1).then_some(())?;
            let surface = window.as_mut().map_or(null_mut(), |window| (egl.eglCreatePlatformWindowSurface)(display, config, (window as *mut c_ulong).cast(), null()));
            let context = (egl.eglCreateContext)(display, config, null_mut(), asked.as_ptr());
            (!context.is_null() && (window.is_none() || !surface.is_null()) && (egl.eglMakeCurrent)(display, surface, surface, context) != 0).then_some(())?;
            Some(Context { egl, display, surface })
        }
    }

    /// Keys, buttons and pointer moves share this shape.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Input {
        kind: c_int,
        serial: c_ulong,
        sent: c_int,
        display: *mut c_void,
        window: c_ulong,
        root: c_ulong,
        child: c_ulong,
        time: c_ulong,
        x: c_int,
        y: c_int,
        x_root: c_int,
        y_root: c_int,
        state: c_uint,
        detail: c_uint,
    }
    /// A window's new place and size.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Configure {
        kind: c_int,
        serial: c_ulong,
        sent: c_int,
        display: *mut c_void,
        event: c_ulong,
        window: c_ulong,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
    }
    /// Any event of X's: room for the largest.
    #[repr(C)]
    pub union XEvent {
        pub kind: c_int,
        pub input: Input,
        pub configure: Configure,
        pub room: [c_long; 24],
    }

    /// A key, by the name X gives it.
    fn key(name: c_ulong) -> Option<Key> {
        Some(match name {
            0xff08 => Key::Backspace,
            0xff09 | 0xfe20 => Key::Tab,
            0xff0d | 0xff8d => Key::Enter,
            0xff1b => Key::Escape,
            0x0020 => Key::Space,
            0xff50 => Key::Home,
            0xff51 => Key::Left,
            0xff52 => Key::Up,
            0xff53 => Key::Right,
            0xff54 => Key::Down,
            0xff55 => Key::PageUp,
            0xff56 => Key::PageDown,
            0xff57 => Key::End,
            0xffff => Key::Delete,
            0xffbe..=0xffc9 => Key::F((name - 0xffbd) as u8),
            0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a => Key::Char((name as u8 as char).to_ascii_lowercase()),
            _ => return None,
        })
    }

    /// What an event of the window means for the interface: `each` gets every event it comes
    /// to. (A key types what X's own table says, which is Latin-1: an input method, for other
    /// writing, is the program's to bring, as in any program on X.)
    pub fn events(x: &Xlib, event: &mut XEvent, scale: f32, mut each: impl FnMut(Event)) {
        // SAFETY: the event is one X filled in; the fields read are those of its kind.
        let (kind, input) = unsafe { (event.kind, event.input) };
        let mods = Mods { shift: input.state & 1 != 0, ctrl: input.state & 4 != 0, alt: input.state & 8 != 0 };
        let at = Event::Move(pt(input.x as f32 / scale, input.y as f32 / scale));
        let button = [Button::Left, Button::Middle, Button::Right].get(input.detail.wrapping_sub(1) as usize).copied();
        // The wheel is buttons 4 and 5 (up, down: sideways with Shift), and 6 and 7 (left, right).
        let step = if input.detail % 2 == 0 { -48.0 } else { 48.0 };
        let wheel = if input.detail <= 5 && !mods.shift { Event::Wheel(0.0, step) } else { Event::Wheel(step, 0.0) };
        match (kind, button) {
            (4, Some(button)) => [at, Event::Press(button, mods)].into_iter().for_each(&mut each),
            (5, Some(button)) => [at, Event::Release(button)].into_iter().for_each(&mut each),
            (4, None) if (4..=7).contains(&input.detail) => [at, wheel].into_iter().for_each(&mut each),
            (6, _) => each(at),
            (8, _) => each(Event::Leave),
            (9 | 10, _) => each(Event::Focus(kind == 9)),
            _ => {}
        }
        let (mut typed, mut name) = ([0u8; 16], 0);
        // SAFETY: a key's event; the room is as long as is said.
        let length = if kind == 2 { unsafe { (x.XLookupString)(event, typed.as_mut_ptr().cast(), typed.len() as c_int, &mut name, null_mut()) } } else { 0 };
        if let Some(key) = key(name).filter(|_| kind == 2) {
            each(Event::Key(key, mods));
        }
        let text: String = typed[..length.max(0) as usize].iter().map(|byte| *byte as char).filter(|c| !c.is_control()).collect();
        if !text.is_empty() && !mods.ctrl {
            each(Event::Text(text));
        }
    }

    /// Hands the program every event that is waiting for the window; false once the window
    /// manager has asked for the window to close (`closing`).
    fn pump(x: &Xlib, display: *mut c_void, closing: c_ulong, program: &mut Program) -> bool {
        // SAFETY: each event is read into room for any event there is, and read as its kind.
        while unsafe { (x.XPending)(display) } > 0 {
            let mut event = XEvent { room: [0; 24] };
            unsafe { (x.XNextEvent)(display, &mut event) };
            let (kind, configure, said) = unsafe { (event.kind, event.configure, event.room[7] as c_ulong) };
            if kind == 33 && said == closing {
                return false;
            }
            if kind == 22 {
                program.resize(configure.width as usize, configure.height as usize, 1.0);
            }
            events(x, &mut event, 1.0, |event| program.hear(event));
        }
        true
    }

    /// The X display, a window on it `width` by `height` (shown), and the word the window
    /// manager says when the window is to close. The window hears keys, buttons, moves, the
    /// pointer leaving, focus, and its own size and showing.
    pub fn window(x: &Xlib, width: c_uint, height: c_uint) -> (*mut c_void, c_ulong, c_ulong) {
        // SAFETY: the display is opened once and the window made on it; both stay while the
        // program runs.
        unsafe {
            let display = (x.XOpenDisplay)(null());
            assert!(!display.is_null(), "there is no X display to open a window on");
            let window = (x.XCreateSimpleWindow)(display, (x.XDefaultRootWindow)(display), 0, 0, width, height, 0, 0, 0);
            (x.XSelectInput)(display, window, 0x1 | 0x4 | 0x8 | 0x40 | 0x20 | 0x20_0000 | 0x2_0000);
            (x.XStoreName)(display, window, c"taut over OpenGL".as_ptr());
            let mut closing = (x.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            (x.XSetWMProtocols)(display, window, &mut closing, 1);
            (x.XMapWindow)(display, window);
            (display, window, closing)
        }
    }

    pub fn run() {
        let Some(x) = Xlib::open() else { return println!("there is no libX11 to open a window with") };
        let (display, window, closing) = self::window(&x, 900, 600);
        let context = context(PLATFORM_X11, display, Some(window)).expect("EGL gives no OpenGL 3.3 of the core profile for this window");
        let letters = taut::platform::system_text();
        let mut program = Program::new(&mut |name| context.find(name), Ui::new(letters, Box::new(LocalClipboard::default())));
        program.resize(900, 600, 1.0);
        while pump(&x, display, closing, &mut program) {
            program.frame();
            unsafe { (context.egl.eglSwapBuffers)(context.display, context.surface) };
        }
    }
}
