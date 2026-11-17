//! taut under Direct3D 12. The program renders with Direct3D as usual, and the interface lies
//! over it: `taut::layer_d3d12::Layer`, made once from the program's device and command queue,
//! draws it with one call per frame, in the program's command list. Around it is the plainest
//! possible program: a window, a device, a swap chain, and the window's messages forwarded to the
//! interface.
//!   cargo run --example d3d12
//!
//! The Direct3D API used is declared here by hand, as the Windows SDK's d3d12.h and dxgi1_4.h
//! declare it: COM methods are called by vtable index, noted beside each call. (A program using a
//! Windows bindings crate calls them by name, and passes the layer its objects' raw pointers.)
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
    use taut::layer_d3d12::Layer;
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

    /// A COM interface's name, as four numbers.
    pub const fn named(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u32; 4] {
        [a, b as u32 | (c as u32) << 16, u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])]
    }

    pub const DEVICE: [u32; 4] = named(0x1898_19F1, 0x1DB6, 0x4B57, [0xBE, 0x54, 0x18, 0x21, 0x33, 0x9B, 0x85, 0xF7]);
    pub const QUEUE: [u32; 4] = named(0x0EC8_70A6, 0x5D7E, 0x4C22, [0x8C, 0xFC, 0x5B, 0xAA, 0xE0, 0x76, 0x16, 0xED]);
    pub const ALLOCATOR: [u32; 4] = named(0x6102_DEE4, 0xAF59, 0x4B09, [0xB9, 0x99, 0xB4, 0x4D, 0x73, 0xF0, 0x9B, 0x24]);
    pub const LIST: [u32; 4] = named(0x5B16_0D0F, 0xAC1B, 0x4185, [0x8B, 0xA8, 0xB3, 0xAE, 0x42, 0xA5, 0xA4, 0x55]);
    pub const FENCE: [u32; 4] = named(0x0A75_3DCF, 0xC4D8, 0x4B91, [0xAD, 0xF6, 0xBE, 0x5A, 0x60, 0xD9, 0x5A, 0x76]);
    pub const HEAP: [u32; 4] = named(0x8EFB_471D, 0x616C, 0x4F49, [0x90, 0xF7, 0x12, 0x7B, 0xB7, 0x63, 0xFA, 0x51]);
    pub const RESOURCE: [u32; 4] = named(0x6964_42BE, 0xA72E, 0x4059, [0xBC, 0x79, 0x5B, 0x5C, 0x98, 0x04, 0x0F, 0xAD]);
    const FACTORY: [u32; 4] = named(0x1BC6_EA02, 0xEF36, 0x464F, [0xBF, 0x0C, 0x21, 0xCA, 0x39, 0xE5, 0x16, 0x8A]);
    const ADAPTER: [u32; 4] = named(0x2411_E7E1, 0x12AC, 0x4CCF, [0xBD, 0x14, 0x97, 0x98, 0xE8, 0x53, 0x4D, 0xC0]);
    const SWAP_CHAIN: [u32; 4] = named(0x94D9_9BDB, 0xF1F8, 0x4AB0, [0xB2, 0x36, 0x7D, 0xA0, 0x17, 0x0E, 0xDA, 0xB1]);

    /// Resource states the swap chain's buffers pass through.
    pub const PRESENT: u32 = 0;
    pub const RENDER_TARGET: u32 = 0x4;

    #[link(name = "d3d12")]
    unsafe extern "system" {
        fn D3D12CreateDevice(adapter: Com, level: u32, interface: *const [u32; 4], device: *mut Com) -> i32;
    }
    #[link(name = "dxgi")]
    unsafe extern "system" {
        fn CreateDXGIFactory2(flags: u32, interface: *const [u32; 4], factory: *mut Com) -> i32;
    }

    /// `D3D12_RESOURCE_BARRIER` of the transition kind, for all of a resource.
    #[repr(C)]
    pub struct Barrier {
        pub kind: u32,
        pub flags: u32,
        pub resource: Com,
        pub subresource: u32,
        pub before: u32,
        pub after: u32,
    }

    /// The transition of all of `resource` from the state `before` to `after`.
    pub fn transition(resource: Com, before: u32, after: u32) -> Barrier {
        Barrier { kind: 0, flags: 0, resource, subresource: u32::MAX, before, after }
    }

    /// `DXGI_SWAP_CHAIN_DESC1`.
    #[repr(C)]
    struct DXGI_SWAP_CHAIN_DESC1 {
        width: u32,
        height: u32,
        format: u32,
        stereo: i32,
        sample_count: u32,
        sample_quality: u32,
        buffer_usage: u32,
        buffer_count: u32,
        scaling: u32,
        swap_effect: u32,
        alpha_mode: u32,
        flags: u32,
    }

    /// A Direct3D 12 device on the graphics card, or on Windows' own software renderer (WARP);
    /// a queue, a command list and a fence; and a swap chain on the window, its buffers as
    /// render targets.
    pub struct Device {
        /// The device (`ID3D12Device`), which makes things.
        pub device: Com,
        /// Its queue (`ID3D12CommandQueue`), which runs command lists.
        pub queue: Com,
        allocator: Com,
        /// The command list (`ID3D12GraphicsCommandList`) each frame is recorded in.
        pub list: Com,
        fence: Com,
        fenced: u64,
        /// The swap chain (`IDXGISwapChain3`), and its two buffers.
        pub swap: Com,
        pub buffers: [Com; 2],
        /// The buffers' render target views, in a heap of their own, this far apart.
        views: Com,
        step: usize,
        /// The buffers' size, in pixels.
        pub size: [u32; 2],
    }

    impl Device {
        /// The device on the graphics card, or on WARP where there is none or `software` says.
        pub fn new(window: isize, software: bool) -> Device {
            let mut made = Device {
                device: null_mut(),
                queue: null_mut(),
                allocator: null_mut(),
                list: null_mut(),
                fence: null_mut(),
                fenced: 0,
                swap: null_mut(),
                buffers: [null_mut(); 2],
                views: null_mut(),
                step: 0,
                size: [0; 2],
            };
            let (mut factory, mut adapter, mut chain) = (null_mut(), null_mut(), null_mut());
            // Pixels of B8G8R8A8_UNORM (87), drawn into (0x20), two buffers, flipped and let go
            // of (4).
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                width: 0,
                height: 0,
                format: 87,
                stereo: 0,
                sample_count: 1,
                sample_quality: 0,
                buffer_usage: 0x20,
                buffer_count: 2,
                scaling: 0,
                swap_effect: 4,
                alpha_mode: 0,
                flags: 0,
            };
            // A direct queue (0), and a heap of two render target views (2).
            let (queue, views) = ([0u32; 4], [2u32, 2, 0, 0]);
            // SAFETY: each description is whole; what is made is held once (factory 27 hands out
            // WARP; device 8 makes the queue, 9 the allocator, 12 the list, 36 the fence, 14 the
            // heap, 15 says how far apart its views are; factory 15 makes the swap chain).
            unsafe {
                check(CreateDXGIFactory2(0, &FACTORY, &mut factory), "making a DXGI factory");
                if software {
                    check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(factory, 27)(factory, &ADAPTER, &mut adapter), "finding WARP");
                }
                if D3D12CreateDevice(adapter, 0xB000, &DEVICE, &mut made.device) < 0 {
                    check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(factory, 27)(factory, &ADAPTER, &mut adapter), "finding WARP");
                    check(D3D12CreateDevice(adapter, 0xB000, &DEVICE, &mut made.device), "making a Direct3D 12 device");
                }
                let device = made.device;
                check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 8)(device, &queue, &QUEUE, &mut made.queue), "making the queue");
                check(function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(device, 9)(device, 0, &ALLOCATOR, &mut made.allocator), "making the command allocator");
                check(
                    function::<unsafe extern "system" fn(Com, u32, u32, Com, Com, *const [u32; 4], *mut Com) -> i32>(device, 12)(device, 0, 0, made.allocator, null_mut(), &LIST, &mut made.list),
                    "making the command list",
                );
                check(function::<unsafe extern "system" fn(Com) -> i32>(made.list, 9)(made.list), "closing the command list");
                check(function::<unsafe extern "system" fn(Com, u64, u32, *const [u32; 4], *mut Com) -> i32>(device, 36)(device, 0, 0, &FENCE, &mut made.fence), "making the fence");
                check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 14)(device, &views, &HEAP, &mut made.views), "making the views' heap");
                made.step = function::<unsafe extern "system" fn(Com, u32) -> u32>(device, 15)(device, 2) as usize;
                type MakeChain = unsafe extern "system" fn(Com, Com, isize, *const DXGI_SWAP_CHAIN_DESC1, *const c_void, Com, *mut Com) -> i32;
                check(function::<MakeChain>(factory, 15)(factory, made.queue, window, &desc, null(), null_mut(), &mut chain), "making the swap chain");
                check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(chain, 0)(chain, &SWAP_CHAIN, &mut made.swap), "asking for IDXGISwapChain3");
                [chain, adapter, factory].into_iter().for_each(|object| release(object));
            }
            made.resize(0, 0);
            made
        }

        /// Waits until the queue has done everything it was given.
        pub fn wait(&mut self) {
            self.fenced += 1;
            // SAFETY: the queue and fence are alive (queue 14 signals; fence 9 waits).
            unsafe {
                check(function::<unsafe extern "system" fn(Com, Com, u64) -> i32>(self.queue, 14)(self.queue, self.fence, self.fenced), "signalling");
                check(function::<unsafe extern "system" fn(Com, u64, isize) -> i32>(self.fence, 9)(self.fence, self.fenced, 0), "waiting");
            }
        }

        /// The swap chain's buffers at the window's new size (0 by 0: the size it has), and a
        /// render target view for each.
        pub fn resize(&mut self, width: u32, height: u32) {
            self.wait();
            let mut desc = [0u32; 14];
            // SAFETY: the buffers are let go of before they are made again (swap chain 13
            // resizes, 9 hands out a buffer; device 20 makes a view; heap 9 is where the views
            // start; resource 10 says what it is, after a pointer for the answer).
            unsafe {
                self.buffers.into_iter().for_each(|object| release(object));
                check(function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32) -> i32>(self.swap, 13)(self.swap, 0, width, height, 0, 0), "resizing the swap chain");
                for at in 0..2 {
                    check(function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(self.swap, 9)(self.swap, at as u32, &RESOURCE, &mut self.buffers[at]), "finding a buffer");
                    function::<unsafe extern "system" fn(Com, Com, *const c_void, usize)>(self.device, 20)(self.device, self.buffers[at], null(), self.view(at));
                }
                function::<unsafe extern "system" fn(Com, *mut [u32; 14]) -> *mut [u32; 14]>(self.buffers[0], 10)(self.buffers[0], &mut desc);
            }
            // (Width is the 64-bit number after the dimension and alignment; height follows.)
            self.size = [desc[4], desc[6]];
        }

        /// The render target view of buffer `at`.
        fn view(&self, at: usize) -> usize {
            let mut start = 0;
            // SAFETY: the heap is alive (9: where its views start).
            unsafe { function::<unsafe extern "system" fn(Com, *mut usize) -> *mut usize>(self.views, 9)(self.views, &mut start) };
            start + at * self.step
        }

        /// The buffer that the next frame is drawn into (swap chain 36 says which).
        pub fn current(&self) -> usize {
            // SAFETY: the swap chain is alive.
            unsafe { function::<unsafe extern "system" fn(Com) -> u32>(self.swap, 36)(self.swap) as usize }
        }

        /// Starts a frame: the command list recording, the buffer made a render target, bound,
        /// and filled with the program's own picture, which is here a colour.
        pub fn begin(&mut self, colour: [f32; 4]) {
            let (list, at) = (self.list, self.current());
            let viewport = [0.0, 0.0, self.size[0] as f32, self.size[1] as f32, 0.0, 1.0];
            // SAFETY: the queue is done with the list (each frame is waited for). (Allocator 8
            // and list 10 reset; list 26 places a barrier, 46 binds the target, 48 fills it, 21
            // sets the viewport.)
            unsafe {
                check(function::<unsafe extern "system" fn(Com) -> i32>(self.allocator, 8)(self.allocator), "resetting the allocator");
                check(function::<unsafe extern "system" fn(Com, Com, Com) -> i32>(list, 10)(list, self.allocator, null_mut()), "resetting the list");
                function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(list, 26)(list, 1, &transition(self.buffers[at], PRESENT, RENDER_TARGET));
                function::<unsafe extern "system" fn(Com, u32, *const usize, i32, *const usize)>(list, 46)(list, 1, &self.view(at), 0, null());
                function::<unsafe extern "system" fn(Com, usize, *const [f32; 4], u32, *const c_void)>(list, 48)(list, self.view(at), &colour, 0, null());
                function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(list, 21)(list, 1, &viewport);
            }
        }

        /// Ends the frame: the buffer made ready to be shown, and the list run (list 9 closes
        /// it; queue 10 runs it).
        pub fn finish(&mut self) {
            let (list, at) = (self.list, self.current());
            // SAFETY: the list is recording.
            unsafe {
                function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(list, 26)(list, 1, &transition(self.buffers[at], RENDER_TARGET, PRESENT));
                check(function::<unsafe extern "system" fn(Com) -> i32>(list, 9)(list), "closing the list");
                function::<unsafe extern "system" fn(Com, u32, *const Com)>(self.queue, 10)(self.queue, 1, &list);
            }
            self.wait();
        }

        /// Shows the frame, once the screen is ready for it (swap chain function 8).
        pub fn show(&self) {
            // SAFETY: the swap chain is alive.
            unsafe { function::<unsafe extern "system" fn(Com, u32, u32) -> i32>(self.swap, 8)(self.swap, 1, 0) };
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // (A device that failed while being made has no queue or fence to wait on.)
            if !self.queue.is_null() && !self.fence.is_null() {
                self.wait();
            }
            // SAFETY: each is held once, by this, and the queue is done with them.
            unsafe { [self.buffers[0], self.buffers[1], self.views, self.swap, self.fence, self.list, self.allocator, self.queue, self.device].into_iter().for_each(|object| release(object)) };
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
        ui.text("Over Direct3D 12", t.font.bold(), t.text);
        ui.switch(&mut app.still, "Hold the colour still");
        ui.slider(&mut app.speed, 0.0..=4.0);
        ui.text_box(&mut app.name, "Type here");
        ui.note(&format!("{} letters typed", app.name.chars().count()));
        ui.close();
    }

    pub fn run() {
        let (class, title): (Vec<u16>, Vec<u16>) = ("taut-d3d12\0".encode_utf16().collect(), "taut over Direct3D 12\0".encode_utf16().collect());
        // SAFETY: the class and the window are made once, with names that end in a zero; the
        // pixels of this program are the screen's own (-4: per monitor, the second way).
        let window = unsafe {
            SetProcessDpiAwarenessContext(-4);
            let instance = GetModuleHandleW(null());
            let arrow = LoadCursorW(0, 32512 as *const u16);
            RegisterClassW(&WNDCLASSW { style: 0, proc: keep, class_extra: 0, window_extra: 0, instance, icon: 0, cursor: arrow, background: 0, menu: null(), name: class.as_ptr() });
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0x10CF_0000, i32::MIN, i32::MIN, 900, 600, 0, 0, instance, null())
        };
        let mut device = Device::new(window, false);
        // SAFETY: the device and its queue are the program's own, and alive.
        let mut layer = unsafe { Layer::new(device.device, device.queue, 87) }.unwrap_or_else(|failed| panic!("{failed}"));
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
            device.begin([0.3 + 0.25 * hue.sin(), 0.3 + 0.25 * (hue + 2.1).sin(), 0.3 + 0.25 * (hue + 4.2).sin(), 1.0]);
            let area = [0, 0, device.size[0] as i32, device.size[1] as i32];
            // SAFETY: the list is the program's, recording, with the swap chain's buffer bound.
            unsafe { layer.draw(&mut ui, device.list, area) }.unwrap_or_else(|failed| panic!("{failed}"));
            device.finish();
            device.show();
        }
    }
}
