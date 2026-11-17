//! Bounds on what reading compressed data and naming fonts may cost, each of which failed
//! before its fix: a file claiming to hold a great deal while holding nothing like it gets no
//! memory for the claim; and fonts named by an app or document, however many and however long
//! their names, take little.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard};

/// Counts the memory the program holds, and the most it held.
struct Counting;
static HELD: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        MOST.fetch_max(HELD.fetch_add(layout.size(), Relaxed) + layout.size(), Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        MOST.fetch_max(HELD.fetch_add(layout.size(), Relaxed) + layout.size(), Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(at, layout) }
    }
    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if size > layout.size() {
            MOST.fetch_max(HELD.fetch_add(size - layout.size(), Relaxed) + size - layout.size(), Relaxed);
        } else {
            HELD.fetch_sub(layout.size() - size, Relaxed);
        }
        unsafe { System.realloc(at, layout, size) }
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;

/// The tests take turns: each counts the memory of the whole program, so none may make or
/// let go of any while another counts.
static TURN: Mutex<()> = Mutex::new(());

/// A test's turn, for as long as it is held.
fn turn() -> MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What `read` gives, and the most memory taken while it ran beyond what was held before.
fn taken<T>(read: impl FnOnce() -> T) -> (T, usize) {
    let before = HELD.load(Relaxed);
    MOST.store(before, Relaxed);
    let read = read();
    (read, MOST.load(Relaxed) - before)
}

/// A zlib stream `long` bytes long that starts as one does, and then says a kind of block
/// there is not.
fn damaged(long: usize) -> Vec<u8> {
    [&[0x78, 0x01, 0x07][..], &vec![0; long - 3]].concat()
}

/// A chunk of a PNG file. (Its check is not read.)
fn chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&(body.len() as u32).to_be_bytes()[..], name, body, &[0; 4]].concat()
}

/// A picture of 8,192 by 8,192 pixels, as its header says, whose two megabytes of packed rows
/// are damaged from their first byte. Room for 128 MB was taken before they were unpacked (as
/// much as 64 times the packed bytes, of the 268 MB the header lets them come to), for
/// nothing; the same for bytes an app unpacks, with as much room as it likes. Room is now
/// taken as the bytes unpack.
#[test]
fn packed_bytes_take_no_room_for_what_they_only_say_they_hold() {
    let _turn = turn();
    let side = 8192u32.to_be_bytes();
    let header = [&side[..], &side, &[8, 6, 0, 0, 0]].concat();
    let png = [&b"\x89PNG\r\n\x1a\n"[..], &chunk(b"IHDR", &header), &chunk(b"IDAT", &damaged(2 << 20)), &chunk(b"IEND", &[])].concat();
    let (read, most) = taken(|| taut::png::decode(&png));
    assert!(read.is_none() && most < 8 * png.len(), "reading a picture of {} bytes took {most} bytes", png.len());
    let packed = damaged(2 << 20);
    let (read, most) = taken(|| taut::png::zlib_decompress(&packed, usize::MAX));
    assert!(read.is_none() && most < 8 * packed.len(), "unpacking {} bytes took {most} bytes", packed.len());
}

/// A font packed for the web of one table of two megabytes that says it unpacks to 63 times
/// as many (which the reader lets by: no more than 64 times the file), and is damaged from its
/// first byte. Room for all of the 126 MB was taken before it was unpacked.
#[test]
fn a_font_for_the_web_takes_no_room_for_a_table_it_only_says_it_holds() {
    let _turn = turn();
    let packed = 2 << 20;
    let mut file = [&b"wOFF"[..], &[0, 1, 0, 0], &[0; 4], &1u16.to_be_bytes(), &[0; 30]].concat();
    file.extend([&b"glyf"[..], &64u32.to_be_bytes(), &(packed as u32).to_be_bytes(), &(63 * packed as u32).to_be_bytes(), &[0; 4]].concat());
    file.extend(damaged(packed));
    let (read, most) = taken(|| taut::FontFile::read(&file[..]));
    assert!(read.is_none() && most < 8 * file.len(), "reading a font of {} bytes took {most} bytes", file.len());
}

/// Faces of fonts by name: an app or a document names as many as it likes, each with a name
/// as long as it likes. Each was kept with its own copy of its name for as long as the
/// program ran: two thousand names of eight kilobytes, in two styles each, held 32 MB. A name
/// is now kept once, and only where it can be a family's; past as many families as any
/// system has, a new one is written in the system's own font. Every face named before stays
/// the face it was.
#[test]
fn naming_fonts_holds_little_however_many_and_however_long_the_names() {
    use taut::Face;
    let _turn = turn();
    let (georgia, before) = (Face::named("Georgia").italic(), HELD.load(Relaxed));
    for n in 0..2000 {
        let _ = Face::named(&format!("{n:05}{}", "x".repeat(8192))).italic();
    }
    for n in 0..10_000 {
        let _ = Face::named(&format!("Family {n}")).weight(700);
    }
    let held = HELD.load(Relaxed).saturating_sub(before);
    assert!(held < 1 << 20, "naming fonts holds {held} bytes");
    assert_eq!(Face::named("Georgia").italic(), georgia, "a face named before is the same face");
    assert_eq!(Face::named("Not Named Before"), Face::named("Nor This One"), "past as many as are kept, the system's own font");
    assert_ne!(Face::named("Georgia"), Face::named("Not Named Before"));
}

/// How much memory a page of Chinese, Japanese and Korean text in an SVG drawing takes, with
/// the system's fonts on Windows: the most the program held while it was drawn, and the most
/// the system gave it. (A measure, printed: `--ignored --nocapture`. It needs the feature
/// `fallback-fonts`, without which the system's fonts for those letters are not found.)
#[test]
#[ignore]
#[cfg(windows)]
fn how_much_a_page_of_east_asian_text_takes() {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        size: u32,
        faults: u32,
        sizes: [usize; 6],
        given: usize,
        most_given: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let given = || {
        let mut counters = Counters { size: size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: the counters are as long as they say they are.
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.size) };
        counters
    };
    let _turn = turn();
    taut::svg::enable_text();
    let scripts = [(0x4e00, 600), (0x3041, 86), (0x30a1, 90), (0xac00, 400)];
    let letters: Vec<char> = scripts.iter().flat_map(|(first, count)| (*first..first + count).filter_map(char::from_u32)).collect();
    let lines: Vec<String> = letters.chunks(40).enumerate().map(|(n, line)| format!("<text x=\"10\" y=\"{}\" font-size=\"18\">{}</text>", 24 + 22 * n, line.iter().collect::<String>())).collect();
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"{}\">{}</svg>", 40 + 22 * lines.len(), lines.concat());
    let before = given();
    let start = std::time::Instant::now();
    let (drawn, most) = taken(|| taut::svg::decode(&svg, 800, 40 + 22 * lines.len()));
    let (time, after) = (start.elapsed(), given());
    let inked = drawn.as_ref().map_or(0, |(_, pixels)| pixels.iter().filter(|pixel| **pixel >> 24 != 0).count());
    let sum = drawn.map_or(0, |(_, pixels)| pixels.iter().fold(0u64, |sum, pixel| sum.wrapping_mul(31).wrapping_add(u64::from(*pixel))));
    eprintln!("{} letters in {} lines: {inked} pixels inked (sum {sum:x}), in {time:?}", letters.len(), lines.len());
    eprintln!("most held while drawing: {:.1} MB; held after: {:.1} MB", most as f64 / 1e6, HELD.load(Relaxed) as f64 / 1e6);
    eprintln!("given by the system: {:.1} MB before, {:.1} MB after, at most {:.1} MB", before.given as f64 / 1e6, after.given as f64 / 1e6, after.most_given as f64 / 1e6);
}
