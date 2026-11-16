//! Fuzzer finds in fonts, each minimized. Regression tests: each failed before its fix.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

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

/// A font packed for the web (WOFF) of ten tables, each of 60,000 packed bytes that say they
/// unpack to 64 times as many (3.8 MB), which no font does but the reader lets by. Room for
/// all ten was taken before any was unpacked: 38 MB for a file of 0.6 MB (and 377 MB for one
/// of 6 MB, as found), only to find that the first table is no packed table at all. Room is
/// now made for a table once it has unpacked to as long as the file says.
#[test]
fn a_font_for_the_web_takes_no_room_for_what_it_only_says_it_holds() {
    let (count, packed) = (10usize, 60_000usize);
    let names = [b"CFF ", b"GPOS", b"cmap", b"glyf", b"head", b"hhea", b"hmtx", b"kern", b"loca", b"maxp"];
    let mut file = [&b"wOFF"[..], &[0, 1, 0, 0], &[0; 4], &(count as u16).to_be_bytes(), &[0; 30]].concat();
    for (n, name) in names.iter().enumerate() {
        let at = 44 + 20 * count + n * packed;
        file.extend([&name[..], &(at as u32).to_be_bytes(), &(packed as u32).to_be_bytes(), &(64 * packed as u32).to_be_bytes(), &[0; 4]].concat());
    }
    // (Each table's bytes start as a zlib stream does, and then say a kind of block there is not.)
    for _ in 0..count {
        file.extend([&[0x78, 0x01, 0x07][..], &vec![0; packed - 3]].concat());
    }
    let before = HELD.load(Relaxed);
    MOST.store(before, Relaxed);
    assert!(taut::FontFile::read(&file[..]).is_none());
    let most = MOST.load(Relaxed) - before;
    assert!(most < 8 << 20, "reading a font of {} bytes took {most} bytes", file.len());
}
