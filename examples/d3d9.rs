//! taut under Direct3D 9. The program renders with Direct3D as usual, and the interface lies
//! over it: `taut::layer_d3d9::Layer`, made once from the program's device, draws it with one
//! call per frame, inside the program's scene. Around it is the plainest possible program: a
//! window, a Direct3D 9Ex device, and the window's messages forwarded to the interface.
//!   cargo run --example d3d9
//!
//! The Direct3D API used is declared here by hand, as the Windows SDK's d3d9.h declares it: COM
//! methods are called by vtable index, noted beside each call. (A program using a Windows
//! bindings crate calls them by name, and passes the layer its device's raw pointer.)
#[cfg(not(windows))]
fn main() {
    println!("Direct3D is Windows' own: there is nothing for this example to show here.");
}

#[cfg(windows)]
fn main() {
    direct3d::run();
}

#[cfg(windows)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
pub mod direct3d {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};
    use taut::geom::Size;
    use taut::layer_d3d9::Layer;
    use taut::platform::{Messages, SystemClipboard};
    use taut::{Color, Length, Ui};

    /// An object of Direct3D's: a pointer to a pointer to its table of functions.
    pub type Com = *mut c_void;

    /// Function number `at` of a COM object's table, as a function of the kind `F`.
    ///
    /// # Safety
    /// The object is alive, and its function of that number is of that kind.
    pub unsafe fn function<F: Copy>(object: Com, at: usize) -> F {
        // SAFETY: as the caller promises; a function is known by where it is, one number.
        unsafe { std::mem::transmute_copy(&*(*(object as *const *const usize)).add(at)) }
    }

    /// Lets go of a COM object (function 2, of every one), if there is one.
    ///
    /// # Safety
    /// The object is held once here, and not used after this.
    pub unsafe fn release(object: Com) {
        if !object.is_null() {
            unsafe { function::<unsafe extern "system" fn(Com) -> u32>(object, 2)(object) };
        }
    }

    /// Stops the program with what Direct3D said, where it said something went wrong.
    pub fn check(result: i32, what: &str) {
        assert!(result >= 0, "{what} failed: {:#010x}", result as u32);
    }

    #[link(name = "d3d9")]
    unsafe extern "system" {
        fn Direct3DCreate9(version: u32) -> Com;
        fn Direct3DCreate9Ex(version: u32, made: *mut Com) -> i32;
    }

    /// `D3DPRESENT_PARAMETERS`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct D3DPRESENT_PARAMETERS {
        pub width: u32,
        pub height: u32,
        pub format: u32,
        pub count: u32,
        pub multisample: u32,
        pub quality: u32,
        pub swap_effect: u32,
        pub window: isize,
        pub windowed: i32,
        pub depth: i32,
        pub depth_format: u32,
        pub flags: u32,
        pub refresh: u32,
        pub interval: u32,
    }

    /// A Direct3D 9 device on the graphics card (9Ex, or the first kind where `ex` is false),
    /// showing its frames in the window.
    pub struct Device {
        d3d: Com,
        /// The device (`IDirect3DDevice9Ex`, or `IDirect3DDevice9`).
        pub device: Com,
        /// It is a 9Ex device.
        pub ex: bool,
        /// How it shows its frames: its back buffer's size among them.
        pub shown: D3DPRESENT_PARAMETERS,
    }

    impl Device {
        pub fn new(window: isize, ex: bool) -> Device {
            // A back buffer of A8R8G8B8 (21) the window's size, its frames let go of once shown
            // (1), in a window, shown at the screen's refresh (0).
            let shown = D3DPRESENT_PARAMETERS {
                width: 0,
                height: 0,
                format: 21,
                count: 1,
                multisample: 0,
                quality: 0,
                swap_effect: 1,
                window,
                windowed: 1,
                depth: 0,
                depth_format: 0,
                flags: 0,
                refresh: 0,
                interval: 0,
            };
            let mut made = Device { d3d: null_mut(), device: null_mut(), ex, shown };
            // Vertices made by the graphics card (0x40), the window left as it is (0x800), the
            // processor's floating point left as it is (0x2).
            let flags = 0x40 | 0x800 | 0x2;
            // SAFETY: the version is the SDK's (32); the description is whole. (IDirect3D9Ex 20:
            // CreateDeviceEx; IDirect3D9 16: CreateDevice; on the first adapter (0), the
            // graphics card (1).)
            unsafe {
                if ex {
                    check(Direct3DCreate9Ex(32, &mut made.d3d), "making Direct3D 9Ex");
                    type Make = unsafe extern "system" fn(Com, u32, u32, isize, u32, *mut D3DPRESENT_PARAMETERS, *const c_void, *mut Com) -> i32;
                    check(function::<Make>(made.d3d, 20)(made.d3d, 0, 1, window, flags, &mut made.shown, null(), &mut made.device), "making a Direct3D 9Ex device");
                } else {
                    made.d3d = Direct3DCreate9(32);
                    assert!(!made.d3d.is_null(), "making Direct3D 9");
                    type Make = unsafe extern "system" fn(Com, u32, u32, isize, u32, *mut D3DPRESENT_PARAMETERS, *mut Com) -> i32;
                    check(function::<Make>(made.d3d, 16)(made.d3d, 0, 1, window, flags, &mut made.shown, &mut made.device), "making a Direct3D 9 device");
                }
            }
            made
        }

        /// The back buffer at the window's new size (0 by 0: the size it has). A device of the
        /// first kind lets go of everything in its own memory first: so the layer does, before.
        pub fn resize(&mut self, width: u32, height: u32) -> i32 {
            (self.shown.width, self.shown.height) = (width, height);
            // SAFETY: the device is alive (IDirect3DDevice9Ex 132: ResetEx; IDirect3DDevice9 16:
            // Reset).
            unsafe {
                match self.ex {
                    true => function::<unsafe extern "system" fn(Com, *mut D3DPRESENT_PARAMETERS, *const c_void) -> i32>(self.device, 132)(self.device, &mut self.shown, null()),
                    false => function::<unsafe extern "system" fn(Com, *mut D3DPRESENT_PARAMETERS) -> i32>(self.device, 16)(self.device, &mut self.shown),
                }
            }
        }

        /// Starts a frame: the back buffer filled with the program's own picture, which is
        /// here a colour (`0xAARRGGBB`), and the scene begun (device 43 clears, 41 begins).
        pub fn begin(&self, colour: u32) {
            // SAFETY: the device is alive.
            unsafe {
                function::<unsafe extern "system" fn(Com, u32, *const c_void, u32, u32, f32, u32) -> i32>(self.device, 43)(self.device, 0, null(), 1, colour, 1.0, 0);
                check(function::<unsafe extern "system" fn(Com) -> i32>(self.device, 41)(self.device), "beginning the scene");
            }
        }

        /// Ends the scene (device 42), and shows the frame (17).
        pub fn show(&self) {
            // SAFETY: the device is alive.
            unsafe {
                function::<unsafe extern "system" fn(Com) -> i32>(self.device, 42)(self.device);
                function::<unsafe extern "system" fn(Com, *const c_void, *const c_void, isize, *const c_void) -> i32>(self.device, 17)(self.device, null(), null(), 0, null());
            }
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: each is held once, by this.
            unsafe { [self.device, self.d3d].into_iter().for_each(|object| release(object)) };
        }
    }

    // Everything from here on is the program's own: its window, what it shows, its loop.

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
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }
    const WM_DESTROY: u32 = 0x0002;
    const WM_SIZE: u32 = 0x0005;
    const WM_QUIT: u32 = 0x0012;
    const WM_MOUSEMOVE: u32 = 0x0200;

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

    /// What the program's own parts are set to.
    struct App {
        still: bool,
        speed: f32,
        name: String,
    }

    fn describe(app: &mut App, ui: &mut Ui) {
        let t = ui.theme;
        ui.column().w(Length::Px(260.0)).offset(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.with_alpha(0.9)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
        ui.text("Over Direct3D 9", t.font.bold(), t.text);
        ui.switch(&mut app.still, "Hold the colour still");
        ui.slider(&mut app.speed, 0.0..=4.0);
        ui.text_box(&mut app.name, "Type here");
        ui.note(&format!("{} letters typed", app.name.chars().count()));
        ui.close();
    }

    pub fn run() {
        let (class, title): (Vec<u16>, Vec<u16>) = ("taut-d3d9\0".encode_utf16().collect(), "taut over Direct3D 9\0".encode_utf16().collect());
        // SAFETY: the class and the window are made once, with names that end in a zero; the
        // pixels of this program are the screen's own (-4: per monitor, the second way).
        let window = unsafe {
            SetProcessDpiAwarenessContext(-4);
            let instance = GetModuleHandleW(null());
            let arrow = LoadCursorW(0, 32512 as *const u16);
            RegisterClassW(&WNDCLASSW { style: 0, proc: keep, class_extra: 0, window_extra: 0, instance, icon: 0, cursor: arrow, background: 0, menu: null(), name: class.as_ptr() });
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0x10CF_0000, i32::MIN, i32::MIN, 900, 600, 0, 0, instance, null())
        };
        let mut device = Device::new(window, true);
        // SAFETY: the device is the program's own, and alive.
        let mut layer = unsafe { Layer::new(device.device) }.unwrap_or_else(|failed| panic!("{failed}"));
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(SystemClipboard));
        ui.theme.bg = Color::NONE;
        let mut app = App { still: false, speed: 1.0, name: String::new() };
        let (mut messages, scale) = (Messages::of(window), unsafe { GetDpiForWindow(window) } as f32 / 96.0);
        let (mut waiting, started, mut hue) = (true, Instant::now(), 0.0f32);
        while pump() {
            for (msg, w, l) in HEARD.take() {
                let (width, height) = (l as u32 & 0xffff, l as u32 >> 16 & 0xffff);
                if msg == WM_SIZE && width > 0 && height > 0 {
                    layer.before_reset();
                    check(device.resize(width, height), "resizing the back buffer");
                    ui.resize(Size { w: width as f32 / scale, h: height as f32 / scale }, scale);
                    waiting = true;
                }
                // SAFETY: the message comes straight from the window procedure, with what
                // Windows sent along with it.
                unsafe { messages.events(msg, w, l, scale, |event| waiting |= ui.event(event)) };
            }
            // The interface runs when something happened, or something in it moves.
            ui.set_time(started.elapsed());
            if std::mem::take(&mut waiting) || ui.wake_in() == Some(Duration::ZERO) {
                ui.run(&mut |ui| describe(&mut app, ui));
            }
            hue += if app.still { 0.0 } else { app.speed / 120.0 };
            let channel = |at: f32| ((0.3 + 0.25 * (hue + at).sin()) * 255.0) as u32;
            device.begin(0xff00_0000 | channel(0.0) << 16 | channel(2.1) << 8 | channel(4.2));
            layer.draw(&mut ui).unwrap_or_else(|failed| panic!("{failed}"));
            device.show();
        }
    }
}
