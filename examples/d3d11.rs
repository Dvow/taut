//! taut under Direct3D 11. The program renders with Direct3D as usual, and the interface lies
//! over it: `taut::layer_d3d11::Layer`, created once from the program's device and context,
//! draws it with one call per frame. Around it is the plainest possible program: a window, a
//! device, a swap chain, and the window's messages forwarded to the interface.
//!   cargo run -p taut --example d3d11
//!
//! The Direct3D API used is declared here by hand, as the Windows SDK's d3d11.h and dxgi.h
//! declare it: COM methods are called by vtable index, noted beside each call. (A program using
//! a Windows bindings crate calls them by name, and passes the layer its device's and context's
//! raw pointers.)
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
    use taut::layer_d3d11::Layer;
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

    #[repr(C)]
    struct DXGI_SWAP_CHAIN_DESC {
        width: u32,
        height: u32,
        refresh_rate: [u32; 2],
        format: u32,
        scanline_ordering: u32,
        scaling: u32,
        sample_count: u32,
        sample_quality: u32,
        buffer_usage: u32,
        buffer_count: u32,
        window: isize,
        windowed: i32,
        swap_effect: u32,
        flags: u32,
    }
    #[repr(C)]
    pub struct D3D11_VIEWPORT {
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub min_depth: f32,
        pub max_depth: f32,
    }
    #[link(name = "d3d11")]
    unsafe extern "system" {
        fn D3D11CreateDeviceAndSwapChain(
            adapter: Com,
            driver: u32,
            software: isize,
            flags: u32,
            levels: *const u32,
            count: u32,
            version: u32,
            desc: *const DXGI_SWAP_CHAIN_DESC,
            swap: *mut Com,
            device: *mut Com,
            level: *mut u32,
            context: *mut Com,
        ) -> i32;
    }

    /// A device and a swap chain on the window: on the graphics card, or on Windows' own
    /// software renderer where there is none. And the swap chain's buffer as a render target.
    pub struct Device {
        /// The device (`ID3D11Device`), which makes things.
        pub device: Com,
        /// Its context (`ID3D11DeviceContext`), which draws and copies.
        pub context: Com,
        /// The swap chain (`IDXGISwapChain`).
        pub swap: Com,
        target: Com,
        /// The buffers' size, in pixels.
        size: [u32; 2],
    }

    impl Device {
        pub fn new(window: isize) -> Device {
            // Pixels of B8G8R8A8_UNORM (87), drawn into (0x20), two buffers, flipped (4).
            let desc = DXGI_SWAP_CHAIN_DESC {
                width: 0,
                height: 0,
                refresh_rate: [0, 0],
                format: 87,
                scanline_ordering: 0,
                scaling: 0,
                sample_count: 1,
                sample_quality: 0,
                buffer_usage: 0x20,
                buffer_count: 2,
                window,
                windowed: 1,
                swap_effect: 4,
                flags: 0,
            };
            let mut made = Device { device: null_mut(), context: null_mut(), swap: null_mut(), target: null_mut(), size: [0; 2] };
            // The graphics card (1), or else Windows' software renderer (5); for B8G8R8A8 (0x20).
            // SAFETY: the description is whole, and the rest is room for what is made.
            let result = [1, 5]
                .into_iter()
                .map(|driver| unsafe { D3D11CreateDeviceAndSwapChain(null_mut(), driver, 0, 0x20, null(), 0, 7, &desc, &mut made.swap, &mut made.device, null_mut(), &mut made.context) })
                .find(|result| *result >= 0);
            check(result.unwrap_or(-1), "making a Direct3D 11 device");
            made.resize(0, 0);
            made
        }

        /// The swap chain's buffers at the window's new size (0 by 0: the size it has), and a
        /// render target for them.
        pub fn resize(&mut self, width: u32, height: u32) {
            const TEXTURE_2D: [u32; 4] = [0x6f15_aaf2, 0x4e89_d208, 0x9548_b49a, 0x9c4f_d335];
            let (mut buffer, mut desc) = (null_mut(), [0u32; 11]);
            // SAFETY: the target is let go of before the buffers are made again (swap chain 13
            // resizes, 9 hands out a buffer; device 9 makes the render target; texture 10 says
            // how large it is).
            unsafe {
                release(std::mem::replace(&mut self.target, null_mut()));
                check(function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32) -> i32>(self.swap, 13)(self.swap, 0, width, height, 0, 0), "resizing the swap chain");
                check(function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(self.swap, 9)(self.swap, 0, &TEXTURE_2D, &mut buffer), "finding the swap chain's buffer");
                check(function::<unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32>(self.device, 9)(self.device, buffer, null(), &mut self.target), "making the render target");
                function::<unsafe extern "system" fn(Com, *mut [u32; 11])>(buffer, 10)(buffer, &mut desc);
                release(buffer);
            }
            self.size = [desc[0], desc[1]];
        }

        /// The program's own picture, which is here a colour, in the whole window: the target
        /// bound, its viewport set, and filled.
        pub fn clear(&self, colour: [f32; 4]) {
            let viewport = D3D11_VIEWPORT { x: 0.0, y: 0.0, width: self.size[0] as f32, height: self.size[1] as f32, min_depth: 0.0, max_depth: 1.0 };
            // SAFETY: the context is alive (33 binds the target, 44 sets the viewport, 50 fills).
            unsafe {
                function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(self.context, 33)(self.context, 1, &self.target, null_mut());
                function::<unsafe extern "system" fn(Com, u32, *const D3D11_VIEWPORT)>(self.context, 44)(self.context, 1, &viewport);
                function::<unsafe extern "system" fn(Com, Com, *const [f32; 4])>(self.context, 50)(self.context, self.target, &colour);
            }
        }

        /// Shows the frame, once the screen is ready for it (swap chain function 8).
        pub fn show(&self) {
            // SAFETY: the swap chain is alive.
            unsafe { function::<unsafe extern "system" fn(Com, u32, u32) -> i32>(self.swap, 8)(self.swap, 1, 0) };
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: each is held once, by this.
            unsafe { [self.target, self.swap, self.context, self.device].into_iter().for_each(|object| release(object)) };
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
        ui.text("Over Direct3D 11", t.font.bold(), t.text);
        ui.switch(&mut app.still, "Hold the colour still");
        ui.slider(&mut app.speed, 0.0..=4.0);
        ui.text_box(&mut app.name, "Type here");
        ui.note(&format!("{} letters typed", app.name.chars().count()));
        ui.close();
    }

    pub fn run() {
        let (class, title): (Vec<u16>, Vec<u16>) = ("taut-d3d11\0".encode_utf16().collect(), "taut over Direct3D 11\0".encode_utf16().collect());
        // SAFETY: the class and the window are made once, with names that end in a zero; the
        // pixels of this program are the screen's own (-4: per monitor, the second way).
        let window = unsafe {
            SetProcessDpiAwarenessContext(-4);
            let instance = GetModuleHandleW(null());
            let arrow = LoadCursorW(0, 32512 as *const u16);
            RegisterClassW(&WNDCLASSW { style: 0, proc: keep, class_extra: 0, window_extra: 0, instance, icon: 0, cursor: arrow, background: 0, menu: null(), name: class.as_ptr() });
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0x10CF_0000, i32::MIN, i32::MIN, 900, 600, 0, 0, instance, null())
        };
        let mut device = Device::new(window);
        // SAFETY: the device and its context are the program's own, and alive.
        let mut layer = unsafe { Layer::new(device.device, device.context) }.unwrap_or_else(|failed| panic!("{failed}"));
        let mut ui = Ui::new(taut::platform::system_text(), Box::new(SystemClipboard));
        ui.theme.bg = Color::NONE;
        let mut app = App { still: false, speed: 1.0, name: String::new() };
        let (mut messages, scale) = (Messages::of(window), unsafe { GetDpiForWindow(window) } as f32 / 96.0);
        let (mut waiting, started, mut hue) = (true, Instant::now(), 0.0f32);
        while pump() {
            for (msg, w, l) in HEARD.take() {
                let (width, height) = (l as u32 & 0xffff, l as u32 >> 16 & 0xffff);
                if msg == WM_SIZE && width > 0 && height > 0 {
                    device.resize(width, height);
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
            device.clear([0.3 + 0.25 * hue.sin(), 0.3 + 0.25 * (hue + 2.1).sin(), 0.3 + 0.25 * (hue + 4.2).sin(), 1.0]);
            layer.draw(&mut ui).unwrap_or_else(|failed| panic!("{failed}"));
            device.show();
        }
    }
}
