//! The Wayland compositors the tests run on, and how the tests drive them: synthetic pointer,
//! keyboard and touch input, and screenshots. Each compositor has its own protocols for these,
//! offered only to trusted clients; the tests start each headless, on its own bus:
//!
//! - KWin (`kwin_wayland --virtual`, with `KWIN_WAYLAND_NO_PERMISSION_CHECKS`): fake input
//!   (`org_kde_kwin_fake_input`), touch included; screenshots from its screencast
//!   (`zkde_screencast_unstable_v1`), frames over PipeWire, composited with OpenGL. Fully
//!   headless it doesn't composite and casts nothing: shown as a window on the tests' own X
//!   display (`--x11-display`, on Xvfb), it does, and screenshots come from that display.
//! - Mutter (`mutter --headless --virtual-monitor`): its remote desktop, touch included, and
//!   screencast, both over the session bus (`org.gnome.Mutter.RemoteDesktop`,
//!   `org.gnome.Mutter.ScreenCast`); frames over PipeWire.
//! - Sway and other wlroots compositors (`WLR_BACKENDS=headless`): virtual pointer and keyboard
//!   (`zwlr_virtual_pointer_manager_v1`, `zwp_virtual_keyboard_manager_v1`), and screencopy
//!   (`zwlr_screencopy_manager_v1`). No touch.
//! - Weston (`weston --debug`): screen capture (`weston_capture_v1`). Without a backend it has
//!   no seat; shown in a window on the tests' own X display (`--backend=x11`, on Xvfb), it is
//!   driven through that display (XTest). No touch.
//!
//! PipeWire frames are grabbed by GStreamer (`gst-launch-1.0` with its PipeWire element): the
//! only external program the tests run.
#[cfg(test)]
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) mod tests {
    use crate::atspi::{ANSWER, Bus, FAILED, NEWS, PROPERTIES, Pack, Value};
    use crate::geom::{Point, pt};
    use crate::touch::Phase;
    use crate::wire::{Arg, Interface, Message, Shared, Wire};
    use crate::x11::{PollFd, dlclose, dlopen, library, poll, symbol};
    use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    /// The left button's evdev code.
    pub(crate) const LEFT: u32 = 0x110;

    /// The tests' own objects the compositor talks to: input and capture. (Their events are
    /// read by opcode, not by interface.)
    const OWN: Interface = Interface::FakeInput;

    /// A compositor's screen, as a user at it: input where it allows it, and screenshots.
    pub(crate) struct Desk {
        /// The compositor, as its starter named it (`TAUT_DESK`): kwin, mutter, sway, weston,
        /// weston-x11; and its scale (`TAUT_SCREEN_SCALE`).
        pub(crate) name: String,
        pub(crate) scale: f32,
        /// The screen size, in pixels.
        pub(crate) pixels: [i32; 2],
        wire: Wire,
        registry: u32,
        offered: Vec<(u32, String, u32)>,
        output: u32,
        shm: u32,
        seat: u32,
        hands: Hands,
        /// The compositor's events for the tests' input method: `done` count, whether it was
        /// activated then, and whether it is active at the next `done`.
        method: (u32, bool, bool),
        /// The X display the compositor's screen is shown in, if any.
        x: Option<Xtest>,
        start: Instant,
        /// The left button is down.
        down: bool,
    }

    /// How one compositor's pointer and keys are driven.
    enum Hands {
        None,
        Kwin(u32),
        Wlr { pointer: u32, keyboard: u32, held: u32, _keymap: Shared },
        Mutter(Remote),
        X,
    }

    /// Mutter's remote desktop: the session bus, the session, its screencast stream (whose
    /// screen position pointer coordinates are relative to), and that stream's PipeWire node.
    struct Remote {
        bus: Bus,
        session: String,
        stream: String,
        node: u32,
    }

    library!(Xlib, ["libX11.so.6", "libX11.so"], {
        fn XOpenDisplay(*const c_char) -> *mut c_void;
        fn XFlush(*mut c_void) -> c_int;
        fn XCloseDisplay(*mut c_void) -> c_int;
        fn XDefaultRootWindow(*mut c_void) -> c_ulong;
        fn XDisplayWidth(*mut c_void, c_int) -> c_int;
        fn XDisplayHeight(*mut c_void, c_int) -> c_int;
        fn XGetImage(*mut c_void, c_ulong, c_int, c_int, c_uint, c_uint, c_ulong, c_int) -> *mut c_int;
        fn XGetPixel(*mut c_int, c_int, c_int) -> c_ulong;
    });
    library!(Xtst, ["libXtst.so.6", "libXtst.so"], {
        fn XTestFakeMotionEvent(*mut c_void, c_int, c_int, c_int, c_ulong) -> c_int;
        fn XTestFakeButtonEvent(*mut c_void, c_uint, c_int, c_ulong) -> c_int;
        fn XTestFakeKeyEvent(*mut c_void, c_uint, c_int, c_ulong) -> c_int;
    });

    /// The X display Weston shows in, with XTest. (The display closes before the libraries: X
    /// holds XTest's functions until then.)
    struct Xtest {
        display: *mut c_void,
        xlib: Xlib,
        xtst: Xtst,
    }

    impl Drop for Xtest {
        fn drop(&mut self) {
            // SAFETY: the display was opened here and nothing uses it after.
            unsafe { (self.xlib.XCloseDisplay)(self.display) };
        }
    }

    impl Xtest {
        /// XTest on the named display, only if the compositor's starter says it is the tests'
        /// own (`TAUT_XVFB`).
        fn new() -> Option<Self> {
            let name = std::ffi::CString::new(std::env::var("TAUT_XVFB").ok()?).ok()?;
            let (xlib, xtst) = (Xlib::open()?, Xtst::open()?);
            // SAFETY: a NUL-terminated display name.
            let display = unsafe { (xlib.XOpenDisplay)(name.as_ptr()) };
            (!display.is_null()).then_some(Xtest { display, xlib, xtst })
        }
        fn flush(&self) {
            // SAFETY: the display is open.
            unsafe { (self.xlib.XFlush)(self.display) };
        }

        /// The whole display: the compositor's screen, in its window there.
        fn picture(&self) -> Option<(usize, Vec<u32>)> {
            // SAFETY: the display is open; X returns an image of the whole root window (all
            // planes, ZPixmap 2: a value per pixel), freed through X's own function.
            unsafe {
                let (xlib, display) = (&self.xlib, self.display);
                let (width, height) = ((xlib.XDisplayWidth)(display, 0), (xlib.XDisplayHeight)(display, 0));
                let picture = (xlib.XGetImage)(display, (xlib.XDefaultRootWindow)(display), 0, 0, width as c_uint, height as c_uint, !0, 2);
                if picture.is_null() {
                    return None;
                }
                let pixels = (0..height).flat_map(|y| (0..width).map(move |x| (xlib.XGetPixel)(picture, x, y) as u32 | 0xff00_0000)).collect();
                Some((width as usize, pixels))
            }
        }
    }

    /// The compositor's globals: each name, interface and version.
    pub(crate) fn offered(wire: &Wire, registry: u32) -> Option<Vec<(u32, String, u32)>> {
        let synced = wire.make(Interface::Callback);
        wire.send(1, 1, &[Arg::Uint(registry)]);
        wire.send(1, 0, &[Arg::Uint(synced)]);
        wire.flush();
        let (until, mut offered) = (Instant::now() + Duration::from_secs(5), Vec::new());
        while Instant::now() < until {
            readable(wire.fd(), 100);
            if !wire.read() {
                return None;
            }
            while let Some(mut message) = wire.next() {
                match (message.of, message.opcode) {
                    (Interface::Registry, 0) => offered.push((message.uint(), message.text(), message.uint())),
                    (Interface::Callback, 0) if message.object == synced => return Some(offered),
                    _ => {}
                }
            }
        }
        None
    }

    /// Waits up to `millis` ms for input on a file descriptor.
    fn readable(fd: c_int, millis: c_int) -> bool {
        let mut waiting = PollFd { fd, events: 1, revents: 0 };
        // SAFETY: one descriptor, waited on no longer than stated.
        unsafe { poll(&mut waiting, 1, millis) > 0 }
    }

    impl Desk {
        /// The compositor the environment names, with whatever input it allows. `None` if none
        /// answers.
        pub(crate) fn new() -> Option<Self> {
            let wire = Wire::connect()?;
            let registry = wire.make(Interface::Registry);
            let offered = offered(&wire, registry)?;
            let name = std::env::var("TAUT_DESK").unwrap_or_default();
            let scale = std::env::var("TAUT_SCREEN_SCALE").ok().and_then(|scale| scale.parse().ok()).unwrap_or(1.0);
            let mut desk = Desk {
                name,
                scale,
                pixels: [0; 2],
                wire,
                registry,
                offered,
                output: 0,
                shm: 0,
                seat: 0,
                hands: Hands::None,
                method: (0, false, false),
                x: Xtest::new(),
                start: Instant::now(),
                down: false,
            };
            desk.output = desk.bind("wl_output", 2, Interface::Output);
            desk.shm = desk.bind("wl_shm", 1, Interface::Shm);
            desk.seat = desk.bind("wl_seat", 1, Interface::Seat);
            desk.screen();
            desk.hands = desk.hands(desk.seat);
            // Mutter's remote desktop keyboard loses the first key: it is sent one that types
            // nothing.
            desk.press(42, &[]);
            // Nor has Mutter's seat touch until a first touch: one lands in an empty corner,
            // and windows hear the seat has touch.
            let corner = pt(1.0, desk.units()[1] - 1.0);
            if desk.touch(9, corner, Phase::Down) {
                desk.touch(9, corner, Phase::Up);
            }
            desk.sync();
            Some(desk)
        }

        /// Binds a global, at most version `most` (0: not offered).
        fn bind(&self, interface: &str, most: u32, of: Interface) -> u32 {
            let Some((name, _, version)) = self.offered.iter().find(|(_, offered, _)| offered == interface) else { return 0 };
            self.wire.bind(self.registry, *name, interface, (*version).min(most), of)
        }

        /// Whether the compositor offers a global.
        pub(crate) fn offers(&self, interface: &str) -> bool {
            self.offered.iter().any(|(_, offered, _)| offered == interface)
        }

        /// Reads the compositor's events, passing each to `heard`, until it returns `true`, or
        /// `millis` ms pass. Returns which.
        fn hear(&self, millis: u64, heard: &mut dyn FnMut(&mut Message) -> bool) -> bool {
            let until = Instant::now() + Duration::from_millis(millis);
            self.wire.flush();
            while Instant::now() < until {
                readable(self.wire.fd(), 20);
                if !self.wire.read() {
                    return false;
                }
                while let Some(mut message) = self.wire.next() {
                    if message.of == Interface::Display && message.opcode == 0 {
                        let (object, code, said) = (message.uint(), message.uint(), message.text());
                        eprintln!("the compositor refused the tests' hands: object {object} ({code}): {said}");
                        return false;
                    }
                    if heard(&mut message) {
                        return true;
                    }
                }
                self.wire.flush();
            }
            false
        }

        /// A roundtrip: everything sent so far was processed.
        fn sync(&self) {
            let synced = self.wire.make(Interface::Callback);
            self.wire.send(1, 0, &[Arg::Uint(synced)]);
            self.hear(5000, &mut |message| message.object == synced);
        }

        /// The screen size in pixels, from its output.
        fn screen(&mut self) {
            let (output, mut pixels) = (self.output, [0; 2]);
            let synced = self.wire.make(Interface::Callback);
            self.wire.send(1, 0, &[Arg::Uint(synced)]);
            self.hear(5000, &mut |message| {
                if (message.object, message.opcode) == (output, 1) && message.uint() & 1 != 0 {
                    pixels = [message.int(), message.int()];
                }
                message.object == synced
            });
            self.pixels = pixels;
        }

        /// The screen in units: its pixels over its scale.
        pub(crate) fn units(&self) -> [f32; 2] {
            self.pixels.map(|side| side as f32 / self.scale)
        }

        /// Milliseconds since the input was set up, as the compositor takes timestamps.
        fn time(&self) -> u32 {
            self.start.elapsed().as_millis() as u32
        }

        /// The input the compositor allows, of those the tests know: KWin's, wlroots', XTest on
        /// Weston's display, Mutter's over the bus. None if it allows none.
        fn hands(&mut self, seat: u32) -> Hands {
            let fake = self.bind("org_kde_kwin_fake_input", 4, OWN);
            if fake != 0 {
                self.wire.send(fake, 0, &[Arg::Text("taut tests"), Arg::Text("to use the windows of a test")]);
                return Hands::Kwin(fake);
            }
            let (pointers, keyboards) = (self.bind("zwlr_virtual_pointer_manager_v1", 1, OWN), self.bind("zwp_virtual_keyboard_manager_v1", 1, OWN));
            if pointers != 0 && keyboards != 0 && seat != 0 {
                return self.wlr(pointers, keyboards, seat).unwrap_or(Hands::None);
            }
            if self.x.is_some() {
                return Hands::X;
            }
            if self.offers("gtk_shell1") {
                return Remote::new().map_or(Hands::None, Hands::Mutter);
            }
            Hands::None
        }

        /// wlroots' virtual pointer and keyboard, the keyboard with a US keymap (the compositor
        /// reads it from a file descriptor it is sent, as it sends its own).
        fn wlr(&self, pointers: u32, keyboards: u32, seat: u32) -> Option<Hands> {
            let wire = &self.wire;
            // Each object is created as soon as it is numbered: the compositor takes no id out
            // of order.
            let pointer = wire.make(OWN);
            wire.send(pointers, 0, &[Arg::Uint(seat), Arg::Uint(pointer)]);
            let keyboard = wire.make(OWN);
            wire.send(keyboards, 0, &[Arg::Uint(seat), Arg::Uint(keyboard)]);
            let map = b"xkb_keymap { xkb_keycodes { include \"evdev+aliases(qwerty)\" }; xkb_types { include \"complete\" }; xkb_compat { include \"complete\" }; xkb_symbols { include \"pc+us+inet(evdev)\" }; };\0";
            let keymap = Shared::new(map.len())?;
            std::os::unix::fs::FileExt::write_all_at(&std::fs::File::from(keymap.file.try_clone().ok()?), map, 0).ok()?;
            wire.send(keyboard, 0, &[Arg::Uint(1), Arg::File(keymap.file.as_raw_fd()), Arg::Uint(map.len() as u32)]);
            self.sync();
            Some(Hands::Wlr { pointer, keyboard, held: 0, _keymap: keymap })
        }

        /// Whether the compositor allows input at all.
        pub(crate) fn has_hands(&self) -> bool {
            !matches!(self.hands, Hands::None)
        }

        /// Whether the input can touch.
        pub(crate) fn has_fingers(&self) -> bool {
            matches!(self.hands, Hands::Kwin(_) | Hands::Mutter(_))
        }

        /// Moves the pointer to a screen point, in units.
        pub(crate) fn point(&mut self, at: Point) {
            let (time, [wide, high], scale) = (self.time(), self.units(), self.scale);
            match &mut self.hands {
                Hands::Kwin(fake) => self.wire.send(*fake, 9, &[Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)]),
                Hands::Wlr { pointer, .. } => {
                    let [x, y, w, h] = [at.x, at.y, wide, high].map(|value| (value * 256.0).round() as u32);
                    self.wire.send(*pointer, 1, &[Arg::Uint(time), Arg::Uint(x), Arg::Uint(y), Arg::Uint(w), Arg::Uint(h)]);
                    self.wire.send(*pointer, 4, &[]);
                }
                Hands::Mutter(remote) => remote.notify("NotifyPointerMotionAbsolute", "sdd", &[Said::Stream, Said::Real(at.x * scale), Said::Real(at.y * scale)]),
                Hands::X => {
                    let x = self.x.as_ref().unwrap();
                    // SAFETY: the display is open.
                    unsafe { (x.xtst.XTestFakeMotionEvent)(x.display, -1, (at.x * scale) as c_int, (at.y * scale) as c_int, 0) };
                    x.flush();
                }
                Hands::None => {}
            }
            self.wire.flush();
        }

        /// The left button goes down or up.
        pub(crate) fn button(&mut self, down: bool) {
            let time = self.time();
            self.down = down;
            match &mut self.hands {
                Hands::Kwin(fake) => self.wire.send(*fake, 2, &[Arg::Uint(LEFT), Arg::Uint(down as u32)]),
                Hands::Wlr { pointer, .. } => {
                    self.wire.send(*pointer, 2, &[Arg::Uint(time), Arg::Uint(LEFT), Arg::Uint(down as u32)]);
                    self.wire.send(*pointer, 4, &[]);
                }
                Hands::Mutter(remote) => remote.notify("NotifyPointerButton", "ib", &[Said::Number(LEFT), Said::Number(down as u32)]),
                Hands::X => {
                    let x = self.x.as_ref().unwrap();
                    // SAFETY: the display is open.
                    unsafe { (x.xtst.XTestFakeButtonEvent)(x.display, 1, down as c_int, 0) };
                    x.flush();
                }
                Hands::None => {}
            }
            self.wire.flush();
        }

        /// A key goes down or up, by evdev code.
        pub(crate) fn key(&mut self, code: u32, down: bool) {
            let time = self.time();
            match &mut self.hands {
                Hands::Kwin(fake) => self.wire.send(*fake, 10, &[Arg::Uint(code), Arg::Uint(down as u32)]),
                Hands::Wlr { keyboard, held, .. } => {
                    self.wire.send(*keyboard, 1, &[Arg::Uint(time), Arg::Uint(code), Arg::Uint(down as u32)]);
                    // A virtual keyboard reports its modifiers itself: Shift 1, Control 4, Alt
                    // 8, as the keymap names them.
                    let mask = match code {
                        42 | 54 => 1,
                        29 | 97 => 4,
                        56 | 100 => 8,
                        _ => 0,
                    };
                    if mask != 0 {
                        *held = if down { *held | mask } else { *held & !mask };
                        self.wire.send(*keyboard, 2, &[Arg::Uint(*held), Arg::Uint(0), Arg::Uint(0), Arg::Uint(0)]);
                    }
                }
                Hands::Mutter(remote) => remote.notify("NotifyKeyboardKeycode", "ub", &[Said::Number(code), Said::Number(down as u32)]),
                Hands::X => {
                    let x = self.x.as_ref().unwrap();
                    // SAFETY: the display is open; X key codes are evdev's plus 8.
                    unsafe { (x.xtst.XTestFakeKeyEvent)(x.display, code + 8, down as c_int, 0) };
                    x.flush();
                }
                Hands::None => {}
            }
            self.wire.flush();
        }

        /// The wheel turns `notches` towards the user.
        pub(crate) fn wheel(&mut self, notches: i32) {
            let time = self.time();
            match &mut self.hands {
                Hands::Kwin(fake) => self.wire.send(*fake, 3, &[Arg::Uint(0), Arg::Fixed(notches as f64 * 15.0)]),
                Hands::Wlr { pointer, .. } => {
                    self.wire.send(*pointer, 5, &[Arg::Uint(0)]);
                    self.wire.send(*pointer, 7, &[Arg::Uint(time), Arg::Uint(0), Arg::Fixed(notches as f64 * 15.0), Arg::Int(notches)]);
                    self.wire.send(*pointer, 4, &[]);
                }
                Hands::Mutter(remote) => remote.notify("NotifyPointerAxisDiscrete", "ui", &[Said::Number(0), Said::Number(notches as u32)]),
                Hands::X => {
                    let x = self.x.as_ref().unwrap();
                    let button = if notches > 0 { 5 } else { 4 };
                    for down in (0..notches.unsigned_abs()).flat_map(|_| [1, 0]) {
                        // SAFETY: the display is open.
                        unsafe { (x.xtst.XTestFakeButtonEvent)(x.display, button, down, 0) };
                    }
                    x.flush();
                }
                Hands::None => {}
            }
            self.wire.flush();
        }

        /// A finger lands, moves or lifts at a screen point, in units. Returns whether input
        /// can touch.
        pub(crate) fn touch(&mut self, finger: u32, at: Point, phase: Phase) -> bool {
            let scale = self.scale;
            match &mut self.hands {
                Hands::Kwin(fake) => {
                    let place = [Arg::Uint(finger), Arg::Fixed(at.x as f64), Arg::Fixed(at.y as f64)];
                    match phase {
                        Phase::Down => self.wire.send(*fake, 4, &place),
                        Phase::Move => self.wire.send(*fake, 5, &place),
                        Phase::Up => self.wire.send(*fake, 6, &[Arg::Uint(finger)]),
                        Phase::Cancel => self.wire.send(*fake, 7, &[]),
                    }
                    self.wire.send(*fake, 8, &[]);
                }
                Hands::Mutter(remote) => {
                    let place = [Said::Stream, Said::Number(finger), Said::Real(at.x * scale), Said::Real(at.y * scale)];
                    match phase {
                        Phase::Down => remote.notify("NotifyTouchDown", "sudd", &place),
                        Phase::Move => remote.notify("NotifyTouchMotion", "sudd", &place),
                        Phase::Up | Phase::Cancel => remote.notify("NotifyTouchUp", "u", &[Said::Number(finger)]),
                    }
                }
                _ => return false,
            }
            self.wire.flush();
            true
        }

        /// Presses a key while others are held.
        pub(crate) fn press(&mut self, key: u32, held: &[u32]) {
            held.iter().for_each(|held| self.key(*held, true));
            self.key(key, true);
            self.key(key, false);
            held.iter().rev().for_each(|held| self.key(*held, false));
        }

        // ---- an input method ----

        /// The tests' own input method, where the compositor allows one
        /// (`zwp_input_method_manager_v2`: Sway; numbered as wlroots numbers it, a protocol not
        /// installed system-wide): its object. `None` if there is none.
        pub(crate) fn input_method(&mut self) -> Option<u32> {
            let manager = self.bind("zwp_input_method_manager_v2", 1, OWN);
            if manager == 0 || self.seat == 0 {
                return None;
            }
            let method = self.wire.make(OWN);
            self.wire.send(manager, 0, &[Arg::Uint(self.seat), Arg::Uint(method)]);
            self.sync();
            Some(method)
        }

        /// Reads the compositor's input method events for `millis` ms: a text box activating or
        /// deactivating it, and `done`. Returns whether it is active.
        pub(crate) fn method_on(&mut self, method: u32, millis: u64) -> bool {
            let mut said = self.method;
            self.hear(millis, &mut |message| {
                match (message.object == method, message.opcode) {
                    (true, 0) => said.2 = true,
                    (true, 1) => said.2 = false,
                    (true, 5) => (said.0, said.1) = (said.0 + 1, said.2),
                    _ => {}
                }
                false
            });
            self.method = said;
            said.1
        }

        /// The input method commits text into its text box, with the compositor's `done` count.
        pub(crate) fn method_types(&mut self, method: u32, text: &str) {
            self.wire.send(method, 0, &[Arg::Text(text)]);
            self.wire.send(method, 3, &[Arg::Uint(self.method.0)]);
            self.wire.flush();
        }

        // ---- pictures ----

        /// A screenshot of the whole screen, as `0xffRRGGBB` pixels, by whatever the compositor
        /// offers. `None` if nothing.
        pub(crate) fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
            if self.offers("zwlr_screencopy_manager_v1") {
                return self.screencopy();
            }
            if self.offers("weston_capture_v1") {
                return self.captured();
            }
            if let Some(x) = &self.x {
                return x.picture();
            }
            if self.offers("zkde_screencast_unstable_v1") {
                return self.kwin_cast();
            }
            match &self.hands {
                Hands::Mutter(remote) => frame_of(remote.node, self.pixels),
                _ => None,
            }
        }

        /// The same, saved as PNG in the folder `TAUT_SHOTS` names (if set), as
        /// `<compositor>-<scale>-<name>.png`.
        pub(crate) fn shoot(&mut self, name: &str) -> Option<(usize, Vec<u32>)> {
            let picture = self.picture();
            let folder = std::env::var("TAUT_SHOTS").ok();
            if let (Some(folder), Some((width, pixels))) = (folder, &picture) {
                let file = std::path::Path::new(&folder).join(format!("{}-{}-{name}.png", self.name, self.scale));
                std::fs::write(&file, crate::png::encode(*width, pixels).unwrap()).unwrap();
                println!("picture: {}", file.display());
            }
            picture
        }

        /// A shared memory buffer for the compositor to write a screenshot into: memory, pool
        /// and buffer.
        fn buffer(&self, [width, height, stride]: [i32; 3], format: u32) -> Option<(Shared, u32, u32)> {
            let memory = Shared::new((stride * height) as usize)?;
            let (pool, buffer) = (self.wire.make(Interface::Pool), self.wire.make(Interface::Buffer));
            self.wire.send(self.shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(stride * height)]);
            self.wire.send(pool, 0, &[Arg::Uint(buffer), Arg::Int(0), Arg::Int(width), Arg::Int(height), Arg::Int(stride), Arg::Uint(format)]);
            Some((memory, pool, buffer))
        }

        /// Releases such a buffer and reads the screenshot from it: flipped if the compositor
        /// says so, red and blue swapped where its format puts red first.
        fn read_out(&self, (mut memory, pool, buffer): (Shared, u32, u32), [width, height, stride]: [i32; 3], format: u32, upside_down: bool) -> (usize, Vec<u32>) {
            self.wire.send(buffer, 0, &[]);
            self.wire.send(pool, 1, &[]);
            let all = memory.pixels();
            let rows = (0..height as usize).map(|row| if upside_down { height as usize - 1 - row } else { row });
            let mut pixels: Vec<u32> = rows.flat_map(|row| all[row * stride as usize / 4..][..width as usize].to_vec()).collect();
            // XBGR8888 and ABGR8888, in DRM's names: red in the low byte.
            if format == 0x3432_4258 || format == 0x3432_4241 {
                pixels.iter_mut().for_each(|pixel| *pixel = (*pixel & 0xff00_ff00) | (*pixel >> 16 & 0xff) | (*pixel & 0xff) << 16);
            }
            pixels.iter_mut().for_each(|pixel| *pixel |= 0xff00_0000);
            (width as usize, pixels)
        }

        /// The screen, by wlroots screencopy.
        fn screencopy(&mut self) -> Option<(usize, Vec<u32>)> {
            let (manager, frame) = (self.bind("zwlr_screencopy_manager_v1", 1, OWN), self.wire.make(OWN));
            self.wire.send(manager, 0, &[Arg::Uint(frame), Arg::Int(0), Arg::Uint(self.output)]);
            let mut asked = None;
            self.hear(5000, &mut |message| {
                if (message.object, message.opcode) == (frame, 0) {
                    asked = Some((message.uint(), [message.uint(), message.uint(), message.uint()].map(|value| value as i32)));
                }
                asked.is_some()
            });
            let (format, size) = asked?;
            let buffer = self.buffer(size, format)?;
            self.wire.send(frame, 0, &[Arg::Object(buffer.2)]);
            let (mut flags, mut ready) = (0, false);
            self.hear(5000, &mut |message| match (message.object == frame, message.opcode) {
                (true, 1) => (flags = message.uint(), false).1,
                (true, 2) => (ready = true, true).1,
                (true, 3) => true,
                _ => false,
            });
            self.wire.send(frame, 1, &[]);
            self.wire.send(manager, 2, &[]);
            let picture = self.read_out(buffer, size, format, flags & 1 != 0);
            ready.then_some(picture)
        }

        /// The screen, by Weston's capture (started with `--debug`, which enables it).
        fn captured(&mut self) -> Option<(usize, Vec<u32>)> {
            let (capture, source) = (self.bind("weston_capture_v1", 1, OWN), self.wire.make(OWN));
            // The composited framebuffer: source 1.
            self.wire.send(capture, 1, &[Arg::Object(self.output), Arg::Uint(1), Arg::Uint(source)]);
            let (mut format, mut size) = (0, [0; 2]);
            self.hear(5000, &mut |message| {
                match (message.object == source, message.opcode) {
                    (true, 0) => format = message.uint(),
                    (true, 1) => size = [message.int(), message.int()],
                    _ => {}
                }
                format != 0 && size[0] > 0
            });
            // Weston gives a DRM format; wl_shm numbers the two common ones its own way.
            let shm_format = match format {
                0x3432_5258 => 1,
                0x3432_5241 => 0,
                other => other,
            };
            let size = [size[0], size[1], size[0] * 4];
            let buffer = self.buffer(size, shm_format)?;
            self.wire.send(source, 1, &[Arg::Object(buffer.2)]);
            let mut complete = false;
            self.hear(5000, &mut |message| match (message.object == source, message.opcode) {
                (true, 2) => (complete = true, true).1,
                (true, 3 | 4) => true,
                _ => false,
            });
            self.wire.send(source, 0, &[]);
            self.wire.send(capture, 0, &[]);
            let picture = self.read_out(buffer, size, format, false);
            complete.then_some(picture)
        }

        /// The screen, by KWin's screencast to PipeWire, first frame.
        fn kwin_cast(&mut self) -> Option<(usize, Vec<u32>)> {
            let (cast, stream) = (self.bind("zkde_screencast_unstable_v1", 6, OWN), self.wire.make(OWN));
            // Without the cursor: 1.
            self.wire.send(cast, 0, &[Arg::Uint(stream), Arg::Object(self.output), Arg::Uint(1)]);
            let mut node = None;
            self.hear(5000, &mut |message| match (message.object == stream, message.opcode) {
                (true, 1) => (node = Some(message.uint()), true).1,
                (true, 0 | 2) => true,
                _ => false,
            });
            let Some(node) = node else {
                eprintln!("KWin made no stream of its screen");
                return None;
            };
            let picture = frame_of(node, self.pixels);
            self.wire.send(stream, 0, &[]);
            self.wire.send(cast, 2, &[]);
            self.wire.flush();
            picture
        }
    }

    impl Drop for Desk {
        fn drop(&mut self) {
            // A test that failed with the button down leaves it down for the next, on a display
            // that outlives the test: it is released.
            if self.down {
                self.button(false);
            }
            if let Hands::Wlr { pointer, keyboard, .. } = self.hands {
                self.wire.send(pointer, 8, &[]);
                self.wire.send(keyboard, 3, &[]);
            }
            if let Hands::Mutter(remote) = &mut self.hands {
                let session = remote.session.clone();
                remote.ask(&session, "org.gnome.Mutter.RemoteDesktop.Session", "Stop", "", &Pack::default());
            }
            self.wire.flush();
        }
    }

    /// A PipeWire node's first frame, grabbed by GStreamer as 4-byte pixels, blue first; the
    /// screen is `width` pixels wide. `None` if none arrives within 15 seconds.
    fn frame_of(node: u32, [width, _]: [i32; 2]) -> Option<(usize, Vec<u32>)> {
        let file = std::env::temp_dir().join(format!("taut-frame-{}-{node}.raw", std::process::id()));
        let caps = "video/x-raw,format=BGRx";
        let pipeline = [&format!("path={node}"), "num-buffers=1", "always-copy=true", "!", "videoconvert", "!", caps, "!", "filesink", &format!("location={}", file.display())];
        let mut gst = std::process::Command::new("gst-launch-1.0").arg("-q").arg("pipewiresrc").args(pipeline).spawn().ok()?;
        let until = Instant::now() + Duration::from_secs(15);
        while gst.try_wait().ok()?.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = gst.kill();
        let Ok(bytes) = std::fs::read(&file) else {
            eprintln!("no frame came from PipeWire node {node}");
            return None;
        };
        let _ = std::fs::remove_file(&file);
        let pixels: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|pixel| u32::from_le_bytes(*pixel) | 0xff00_0000).collect();
        (!pixels.is_empty() && pixels.len().is_multiple_of(width as usize)).then_some((width as usize, pixels))
    }

    /// One value sent to Mutter: the stream path, an integer, or a double.
    enum Said {
        Stream,
        Number(u32),
        Real(f32),
    }

    /// A bus call's reply: whether it failed, and the first string in it (an object path, or a
    /// string variant).
    struct Answer {
        failed: bool,
        text: String,
    }

    const REMOTE: &str = "org.gnome.Mutter.RemoteDesktop";
    const CAST: &str = "org.gnome.Mutter.ScreenCast";

    impl Remote {
        /// A started Mutter remote desktop session, with a screencast of its monitor (relative
        /// to which pointer and touch coordinates are given). `None` if Mutter is not on the
        /// session bus.
        fn new() -> Option<Self> {
            let mut remote = Remote { bus: Bus::session()?, session: String::new(), stream: String::new(), node: 0 };
            let created = remote.ask("/org/gnome/Mutter/RemoteDesktop", REMOTE, "CreateSession", "", &Pack::default());
            remote.session = created.filter(|answer| !answer.failed)?.text;
            let mut asked = Pack::default();
            asked.text("org.gnome.Mutter.RemoteDesktop.Session").text("SessionId");
            let session = remote.session.clone();
            let id = remote.ask(&session, PROPERTIES, "Get", "ss", &asked)?.text;
            let mut options = Pack::default();
            options.open(8).pad(8).text("remote-desktop-session-id").kinds("s").text(&id).close();
            let cast = remote.ask("/org/gnome/Mutter/ScreenCast", CAST, "CreateSession", "a{sv}", &options).filter(|answer| !answer.failed)?.text;
            remote.bus.ask("AddMatch", "type='signal',interface='org.gnome.Mutter.ScreenCast.Stream',member='PipeWireStreamAdded'");
            for connector in ["", "Meta-0"] {
                let mut record = Pack::default();
                record.text(connector).open(8).close();
                let stream = remote.ask(&cast, "org.gnome.Mutter.ScreenCast.Session", "RecordMonitor", "sa{sv}", &record);
                if let Some(stream) = stream.filter(|answer| !answer.failed) {
                    remote.stream = stream.text;
                    break;
                }
            }
            let started = remote.ask(&session, "org.gnome.Mutter.RemoteDesktop.Session", "Start", "", &Pack::default());
            if remote.stream.is_empty() || started.is_none_or(|answer| answer.failed) {
                return None;
            }
            let until = Instant::now() + Duration::from_secs(5);
            while remote.node == 0 && Instant::now() < until {
                remote.turn();
            }
            Some(remote)
        }

        /// Reads incoming bus messages: the stream's PipeWire node, when announced.
        fn turn(&mut self) -> Vec<(u32, bool, String)> {
            readable(self.bus.waits().0, 20);
            let (mut answers, mut node) = (Vec::new(), self.node);
            self.bus.turn(&mut |_, message| {
                let mut body = message.body;
                match message.kind {
                    NEWS if message.member == "PipeWireStreamAdded" => node = body.u32().unwrap_or(0),
                    ANSWER => {
                        // An object path; or, for a property, a variant with its signature.
                        let mut value = message.body;
                        let text = body.text().or(match value.value() {
                            Some(Value::Text(text)) => Some(text),
                            _ => None,
                        });
                        answers.push((message.answers, false, text.unwrap_or_default().to_string()));
                    }
                    FAILED => answers.push((message.answers, true, format!("{}: {}", message.failure, body.text().unwrap_or_default()))),
                    _ => {}
                }
            });
            self.node = node;
            answers
        }

        /// Calls a Mutter method and waits up to five seconds for the reply.
        fn ask(&mut self, path: &str, interface: &str, member: &str, kinds: &str, values: &Pack) -> Option<Answer> {
            let to = if interface.starts_with(CAST) { CAST } else { REMOTE };
            let serial = self.bus.call(to, path, interface, member, kinds, values);
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                if let Some((_, failed, text)) = self.turn().into_iter().find(|(answers, ..)| *answers == serial) {
                    if failed {
                        eprintln!("Mutter refused {member}: {text}");
                    }
                    return Some(Answer { failed, text });
                }
            }
            None
        }

        /// Sends a session input event and waits until Mutter processed it.
        fn notify(&mut self, member: &str, kinds: &str, values: &[Said]) {
            let mut pack = Pack::default();
            for value in values {
                match value {
                    Said::Stream => pack.text(&self.stream),
                    Said::Number(number) => pack.u32(*number),
                    Said::Real(real) => pack.f64(*real as f64),
                };
            }
            let session = self.session.clone();
            self.ask(&session, "org.gnome.Mutter.RemoteDesktop.Session", member, kinds, &pack);
        }
    }

    /// A window's client area origin on screen, found the only way a client can: the pointer is
    /// moved over the screen, from its centre outward, until the window says it is over it, and
    /// where it says it is gives its origin. `over` returns the pointer's surface position, as
    /// the window last heard.
    pub(crate) fn found(desk: &mut Desk, over: &mut dyn FnMut(&mut Desk) -> Option<Point>) -> Option<Point> {
        let [wide, high] = desk.units();
        // The centre first, where most compositors put new windows; then a grid, a tenth of the
        // screen apart.
        let grid = (1..10).flat_map(|down| (1..10).map(move |across| (across as f32 / 10.0, down as f32 / 10.0)));
        let debug = std::env::var_os("TAUT_DESK_DEBUG").is_some();
        for (across, down) in std::iter::once((0.5, 0.5)).chain(grid) {
            let at = pt((wide * across).round(), (high * down).round());
            desk.point(at);
            let until = Instant::now() + Duration::from_millis(150);
            let mut on = None;
            while Instant::now() < until {
                on = over(desk);
            }
            if debug {
                eprintln!("pointer at {at:?}: the window says {on:?}");
            }
            // Confirmed with the pointer moved a little further: what the window said may be
            // stale, from before the pointer or window moved.
            let Some(on) = on else { continue };
            let further = pt(at.x + 3.0, at.y + 2.0);
            desk.point(further);
            let until = Instant::now() + Duration::from_millis(150);
            let mut then = None;
            while Instant::now() < until && then != Some(pt(on.x + 3.0, on.y + 2.0)) {
                then = over(desk);
            }
            if debug {
                eprintln!("  and a little further on: {then:?}");
            }
            if then == Some(pt(on.x + 3.0, on.y + 2.0)) {
                return Some(pt(at.x - on.x, at.y - on.y));
            }
        }
        if debug {
            desk.shoot("not-found");
        }
        None
    }
}
