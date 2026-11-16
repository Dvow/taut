//! Fuzzer finds in the library's own Wayland host, as a compositor might send them: the window
//! runs for real (`taut::run`) against a compositor played here at the other end of a socket
//! passed as `WAYLAND_SOCKET`. Regression tests: each failed before its fix.
#![cfg(all(unix, not(target_os = "macos")))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// Every allocation of these tests, counted; and refused beyond a gigabyte held at once (so
/// that a test that fails does so at once, and does not take all of the computer's memory).
struct Counting;
static HELD: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);
const REFUSED: usize = 1 << 30;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let held = HELD.fetch_add(layout.size(), Relaxed) + layout.size();
        if held > REFUSED {
            HELD.fetch_sub(layout.size(), Relaxed);
            return std::ptr::null_mut();
        }
        MOST.fetch_max(held, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(at, layout) }
    }
}

#[global_allocator]
static COUNTED: Counting = Counting;

/// What the compositor offers: what a window needs, and a scale finer than whole pixels.
const OFFERED: [(&str, u32); 5] = [("wl_compositor", 6), ("wl_shm", 1), ("xdg_wm_base", 6), ("wp_viewporter", 1), ("wp_fractional_scale_manager_v1", 1)];

/// The compositor's end of the socket: what each of the program's objects is, by the
/// requests that made them.
struct Compositor {
    socket: UnixStream,
    inbox: Vec<u8>,
    objects: HashMap<u32, &'static str>,
    configured: bool,
    drawn: bool,
    gone: bool,
    /// The largest size the program set its window to be shown at, in units.
    largest: [u32; 2],
}

impl Compositor {
    fn send(&mut self, object: u32, opcode: u16, words: &[u32]) {
        let mut message = Vec::new();
        message.extend_from_slice(&object.to_ne_bytes());
        message.extend_from_slice(&((8 + 4 * words.len() as u32) << 16 | opcode as u32).to_ne_bytes());
        words.iter().for_each(|word| message.extend_from_slice(&word.to_ne_bytes()));
        self.gone |= self.socket.write_all(&message).is_err();
    }

    /// Text as the protocol writes it: how long, then the bytes and a nought, in words.
    fn text(words: &mut Vec<u32>, text: &str) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        words.push(bytes.len() as u32);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        words.extend(bytes.chunks(4).map(|word| u32::from_ne_bytes(word.try_into().unwrap())));
    }

    fn of(&self, interface: &str) -> u32 {
        self.objects.iter().find(|(_, of)| **of == interface).map_or(0, |(id, _)| *id)
    }

    /// Reads what the program said for up to `wait`, and answers it.
    fn listen(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        let mut room = [0u8; 65536];
        while !self.gone && Instant::now() < until {
            let _ = self.socket.set_read_timeout(Some(until.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))));
            match self.socket.read(&mut room) {
                Ok(0) => self.gone = true,
                Ok(got) => self.inbox.extend_from_slice(&room[..got]),
                Err(_) => break,
            }
            while self.inbox.len() >= 8 {
                let size = u32::from_ne_bytes(self.inbox[4..8].try_into().unwrap());
                let long = (size >> 16) as usize;
                if long < 8 || self.inbox.len() < long {
                    break;
                }
                let object = u32::from_ne_bytes(self.inbox[..4].try_into().unwrap());
                let words: Vec<u32> = self.inbox[8..long].chunks(4).map(|word| u32::from_ne_bytes(word.try_into().unwrap())).collect();
                self.inbox.drain(..long);
                self.request(object, size as u16, &words);
            }
        }
    }

    fn request(&mut self, object: u32, opcode: u16, words: &[u32]) {
        let at = |n: usize| words.get(n).copied().unwrap_or(0);
        let of = if object == 1 { "wl_display" } else { self.objects.get(&object).copied().unwrap_or("") };
        let made = match (of, opcode) {
            ("wl_display", 1) => {
                for (n, (name, version)) in OFFERED.iter().enumerate() {
                    let mut said = vec![n as u32 + 1];
                    Compositor::text(&mut said, name);
                    said.push(*version);
                    self.send(at(0), 0, &said);
                }
                Some("wl_registry")
            }
            ("wl_display", 0) => {
                // A round trip: the window is shown first, the first time it is asked.
                let toplevel = self.of("xdg_toplevel");
                if !self.configured && toplevel != 0 {
                    self.configured = true;
                    self.send(toplevel, 0, &[0, 0, 0]);
                    self.send(self.of("xdg_surface"), 0, &[7]);
                }
                self.send(at(0), 0, &[1]);
                self.send(1, 1, &[at(0)]);
                None
            }
            ("wl_registry", 0) => {
                let name = at(0) as usize;
                let id = at(1 + at(1) as usize / 4 + usize::from(at(1) % 4 != 0) + 2);
                self.objects.insert(id, OFFERED[name - 1].0);
                None
            }
            ("wl_compositor", 0) => Some("wl_surface"),
            ("wl_surface", 3) => {
                self.send(at(0), 0, &[1]);
                None
            }
            ("wl_surface", 6) => {
                self.drawn |= self.configured;
                None
            }
            ("wl_shm", 0) => Some("wl_shm_pool"),
            ("wl_shm_pool", 0) => Some("wl_buffer"),
            ("xdg_wm_base", 2) => Some("xdg_surface"),
            ("xdg_surface", 1) => Some("xdg_toplevel"),
            ("wp_viewporter", 1) => Some("wp_viewport"),
            ("wp_viewport", 2) => {
                self.largest = [self.largest[0].max(at(0)), self.largest[1].max(at(1))];
                None
            }
            ("wp_fractional_scale_manager_v1", 1) => Some("wp_fractional_scale_v1"),
            _ => None,
        };
        if let Some(interface) = made {
            self.objects.insert(at(0), interface);
        }
    }
}

/// What a run against the compositor came to: the most memory held at once while it ran,
/// and the largest size, in units, the window was set to be shown at.
struct Ran {
    most: usize,
    largest: [u32; 2],
}

unsafe extern "C" {
    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
}

/// Holds this program to what memory it has now and three gigabytes more (`RLIMIT_AS`, 9):
/// a window that asks for more than that is not made, rather than the computer's memory
/// filling up as a failing test draws it.
fn hold_memory() {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let now = status.lines().find_map(|line| line.strip_prefix("VmSize:")).and_then(|kb| kb.trim().trim_end_matches("kB").trim().parse::<u64>().ok()).unwrap_or(0) << 10;
    let limit = [now + (3 << 30); 2];
    // SAFETY: the limit is two numbers, as the C library has them.
    unsafe { setrlimit(9, &limit) };
}

/// Runs a window with a label and a box to type into, against a compositor that shows it
/// and then says `then` (each a message to the latest object of an interface: its opcode
/// and its words), and goes.
fn run_against(then: &'static [(&'static str, u16, &'static [u32])]) -> Ran {
    hold_memory();
    let (ours, theirs) = UnixStream::pair().unwrap();
    // SAFETY: each run is a program of its own (see `alone`), whose one thread of tests is
    // the only one that reads or changes the environment; and neither an X server nor a real compositor is to be
    // reached by these tests.
    unsafe {
        std::env::set_var("WAYLAND_SOCKET", ours.into_raw_fd().to_string());
        std::env::set_var("TAUT_BACKEND", "wayland");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
    }
    let compositor = std::thread::spawn(move || {
        let mut compositor = Compositor { socket: theirs, inbox: Vec::new(), objects: HashMap::new(), configured: false, drawn: false, gone: false, largest: [0; 2] };
        let until = Instant::now() + Duration::from_secs(10);
        while !compositor.drawn && !compositor.gone && Instant::now() < until {
            compositor.listen(Duration::from_millis(20));
        }
        assert!(compositor.drawn, "the window was not shown");
        for (interface, opcode, words) in then {
            let object = compositor.of(interface);
            compositor.send(object, *opcode, words);
        }
        compositor.listen(Duration::from_millis(200));
        let _ = compositor.socket.shutdown(std::net::Shutdown::Write);
        compositor.listen(Duration::from_secs(10));
        compositor.largest
    });
    let before = HELD.load(Relaxed);
    MOST.store(before, Relaxed);
    let mut typed = String::from("Some words");
    taut::run(taut::Window { title: "Played", width: 420.0, height: 300.0, ..Default::default() }, move |ui| {
        ui.label("A window whose compositor is a test");
        ui.text_box(&mut typed, "Type here");
    });
    let largest = compositor.join().unwrap();
    Ran { most: MOST.load(Relaxed) - before, largest }
}

/// Runs one of the tests below by itself, in a program of its own (a program runs its
/// window once): whether it passed, and what it said.
fn alone(name: &str) -> (bool, String) {
    let out = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", name, "--ignored", "--nocapture", "--test-threads=1"]).output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The compositor says the window's best scale is 600 (a fractional scale of 72,000
/// 120ths): the window's interface is run at it, and its letters are made 600 times their
/// size as it is measured, half a gigabyte for a label and a box to type into, and seconds.
/// (Found with 46 bytes of input as a scale of 33 million, 0xEC02050F 120ths: there,
/// Arch's FreeType made glyphs of hundreds of megabytes each and Ubuntu's refused them.)
/// Pixels of a window's buffer are held to 32,767 a side, but the scale its interface is
/// run and its text drawn at is held to nothing. A compositor that says this is mistaken,
/// but the program should not take all the computer's memory for it.
#[test]
fn a_scale_that_no_screen_has_does_not_take_all_the_memory() {
    let (passed, said) = alone("a_scale_that_no_screen_has_by_itself");
    assert!(passed, "{said}");
}

#[test]
#[ignore = "run by itself by the test above"]
fn a_scale_that_no_screen_has_by_itself() {
    let ran = run_against(&[("wp_fractional_scale_v1", 0, &[600 * 120])]);
    assert!(ran.most < 256 << 20, "{} MB held at once", ran.most >> 20);
}

/// The compositor says the window is to be 76,800 by 16,303,872 units: the window takes
/// that size, and its buffer is held to 32,767 by 32,767 pixels, four gigabytes, all of
/// them drawn (found with 635 bytes of input, seven to nine seconds a run). No screen, nor
/// row of screens, is near that: a window can be held to 16,384 units a side.
#[test]
fn a_window_larger_than_any_screen_is_not_made_so() {
    let (passed, said) = alone("a_window_larger_than_any_screen_by_itself");
    assert!(passed, "{said}");
}

#[test]
#[ignore = "run by itself by the test above"]
fn a_window_larger_than_any_screen_by_itself() {
    let ran = run_against(&[("xdg_toplevel", 0, &[76_800, 16_303_872, 0]), ("xdg_surface", 0, &[8])]);
    assert!(ran.largest[0] <= 16_384 && ran.largest[1] <= 16_384, "the window was set to {:?} units", ran.largest);
}
