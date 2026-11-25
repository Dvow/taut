//! Bounds on what rendering an SVG drawing may cost: a file of a few hundred bytes reusing a
//! few elements, each a group composited as one, a clipped group, a shape as large as the
//! image, or a pattern fill, made rendering cover the whole image tens of thousands of times
//! (each such group is a layer the size of the image; each pattern tile one of up to
//! 1024×1024). The reader kept no more shapes than a file of its length could write, but each
//! could cost an image's worth. Now what is kept is bounded to as many image-covers as such a
//! file could produce (64 at least). Regression tests: each failed before its fix.
// (Measured by a thread's processor time, which Windows and Unix give.)
#![cfg(any(windows, unix))]
use std::time::Duration;

/// A drawing of 100 by 100 units in which `inner` is drawn 4^`levels` times, by groups that
/// each use the one before four times; `defs` are what it uses.
fn used_over_and_over(defs: &str, inner: &str, levels: usize) -> String {
    let mut svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><defs>{defs}<g id="z0">{inner}</g>"##);
    for level in 1..=levels {
        svg += &format!(r##"<g id="z{level}">{}</g>"##, format!(r##"<use href="#z{}"/>"##, level - 1).repeat(4));
    }
    svg + &format!(r##"</defs><use href="#z{levels}"/></svg>"##)
}

/// Draws a drawing `side` by `side` pixels, which must take less than four seconds of the
/// processor's time. (The drawings here take one to two in a debug build when the computer
/// is quiet, and more on one busy with other programs, which take some of each processor's
/// cache. Before drawings were held to their cost, the first of them took 25.)
fn drawn_in_time(svg: &str, side: usize) {
    let start = worked();
    let drawn = taut::svg::decode(svg, side, side);
    let took = worked() - start;
    assert!(drawn.is_some_and(|(wide, pixels)| wide == side && pixels.len() == side * side));
    assert!(took < Duration::from_secs(4), "a drawing of {} bytes took {took:?} to draw at {side} by {side}", svg.len());
}

/// How much of a processor's time this thread has had so far: what drawing costs, whatever
/// else the computer is doing meanwhile. (The clock on the wall counts the time other
/// programs take too: on a computer busy building other programs, these tests failed by it
/// though the drawing cost no more.) Nothing a drawing does is done on another thread.
fn worked() -> Duration {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn GetThreadTimes(thread: isize, made: *mut u64, ended: *mut u64, system: *mut u64, own: *mut u64) -> i32;
        }
        // (Each in hundreds of nanoseconds.)
        let ([mut made, mut ended], [mut system, mut own]) = ([0u64; 2], [0u64; 2]);
        assert_ne!(unsafe { GetThreadTimes(GetCurrentThread(), &mut made, &mut ended, &mut system, &mut own) }, 0);
        Duration::from_nanos((system + own) * 100)
    }
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Moment {
            seconds: i64,
            nanoseconds: i64,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: i32, at: *mut Moment) -> i32;
        }
        // (`CLOCK_THREAD_CPUTIME_ID`.)
        let mut at = Moment { seconds: 0, nanoseconds: 0 };
        assert_eq!(unsafe { clock_gettime(3, &mut at) }, 0);
        Duration::new(at.seconds as u64, at.nanoseconds as u32)
    }
}

/// A group that is less than solid is drawn on a block of its own, as large as the picture,
/// and laid down after: 16,384 of them in 600 bytes. (Live: 25 s at 256 by 256 in a debug
/// build; 0.8 ms each at 512 by 512 in a release build, and 31 ms at 2048 by 2048.)
#[test]
fn groups_laid_down_as_one_cost_no_more_than_the_file_could_write_out() {
    drawn_in_time(&used_over_and_over("", r#"<g opacity=".5"><rect width="1" height="1"/></g>"#, 7), 256);
    // Four such groups in one another, each a block, 4,096 times.
    let deep = r#"<g opacity=".9"><g opacity=".9"><g opacity=".9"><g opacity=".9"><rect width="1" height="1"/></g></g></g></g>"#;
    drawn_in_time(&used_over_and_over("", deep, 6), 256);
}

/// A group cut by a clip path that is no box is drawn on a block, and the clip path on
/// another; one cut by a mask, on three.
#[test]
fn groups_that_are_cut_cost_no_more_than_the_file_could_write_out() {
    let clip = r#"<clipPath id="c"><circle cx="5" cy="5" r="4"/></clipPath>"#;
    drawn_in_time(&used_over_and_over(clip, r#"<g clip-path="url(#c)"><rect width="10" height="10"/></g>"#, 7), 256);
    let mask = r#"<mask id="m"><rect width="10" height="10" fill="white"/></mask>"#;
    drawn_in_time(&used_over_and_over(mask, r#"<g mask="url(#m)"><rect width="10" height="10"/></g>"#, 7), 256);
    // A line in a drawing in the drawing, cut at its box: drawn on a block of its own.
    drawn_in_time(&used_over_and_over("", r#"<svg width="100" height="100"><path d="M0 0L1 1" stroke="red"/></svg>"#, 7), 256);
}

/// A shape as large as the picture is filled over all of it: 16,384 of them (of which the
/// 13,000 whose outlines a file of this length could write were kept).
#[test]
fn shapes_as_large_as_the_picture_cost_no_more_than_the_file_could_write_out() {
    drawn_in_time(&used_over_and_over("", r#"<rect width="100" height="100" fill="red"/>"#, 7), 256);
    // Filled with colours that go over into each other, each pixel's colour worked out.
    let blend = r#"<radialGradient id="g" fx=".2"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></radialGradient>"#;
    drawn_in_time(&used_over_and_over(blend, r#"<rect width="100" height="100" fill="url(#g)"/>"#, 6), 256);
}

/// A shape filled with a pattern is drawn after the pattern's tile is, on a block as large as
/// it shows, and each pixel of the shape is then worked out from the tile's: 4,096 shapes as
/// large as the picture, 18 s at 512 by 512 in a release build for 256 of them.
#[test]
fn shapes_filled_with_patterns_cost_no_more_than_the_file_could_write_out() {
    let pattern = r#"<pattern id="p" width="100" height="100" patternUnits="userSpaceOnUse"><rect width="50" height="50" fill="red"/></pattern>"#;
    drawn_in_time(&used_over_and_over(pattern, r#"<rect width="100" height="100" fill="url(#p)"/>"#, 6), 128);
}

/// A tile far larger than the picture is drawn on a block of 1024 by 1024 pixels, however
/// small the picture: 4,096 of them for a picture of 16 by 16 (4.5 ms each in a release
/// build). And patterns filled with patterns, forty deep, each tile 1024 by 1024: forty such
/// blocks held at once, and each filled pixel by pixel from the next (1.3 s and 20 MB for
/// eight of them, at 16 by 16).
#[test]
fn tiles_larger_than_the_picture_cost_no_more_than_the_file_could_write_out() {
    let huge = r#"<pattern id="p" width="10000" height="10000" patternUnits="userSpaceOnUse"><rect width="10000" height="10000" fill="red"/></pattern>"#;
    drawn_in_time(&used_over_and_over(huge, r#"<rect width="1" height="1" fill="url(#p)"/>"#, 6), 16);
    let mut nested = String::new();
    for n in 0..40 {
        nested += &format!(r##"<pattern id="p{n}" width="10000" height="10000" patternUnits="userSpaceOnUse"><rect width="10000" height="10000" fill="url(#p{})"/></pattern>"##, n + 1);
    }
    nested += r#"<pattern id="p40" width="10" height="10" patternUnits="userSpaceOnUse"><rect width="5" height="5" fill="red"/></pattern>"#;
    drawn_in_time(&format!(r##"<svg viewBox="0 0 100 100"><defs>{nested}</defs><rect width="100" height="100" fill="url(#p0)"/></svg>"##), 16);
}

/// A marker is drawn at every corner of a line: one that holds ten squares as large as the
/// picture, each less than solid and cut by a clip path (two blocks of pixels each), at each
/// of 4,000 corners written in 18,000 bytes. And one that holds four groups in one another,
/// each laid down as one.
#[test]
fn markers_at_every_corner_cost_no_more_than_the_file_could_write_out() {
    let corners = "l1 1l-1 0".repeat(2000);
    let cut = r#"<clipPath id="c"><circle cx="50" cy="50" r="40"/></clipPath><marker id="k" markerUnits="userSpaceOnUse">{}</marker>"#;
    let cut = cut.replace("{}", &r#"<rect width="100" height="100" opacity=".5" clip-path="url(#c)"/>"#.repeat(10));
    drawn_in_time(&format!(r##"<svg viewBox="0 0 100 100"><defs>{cut}</defs><path d="M0 0{corners}" stroke="red" marker-mid="url(#k)"/></svg>"##), 128);
    let grouped = r#"<marker id="k" markerUnits="userSpaceOnUse"><g opacity=".9"><g opacity=".9"><g opacity=".9"><g opacity=".9"><rect width="1" height="1"/></g></g></g></g></marker>"#;
    drawn_in_time(&format!(r##"<svg viewBox="0 0 100 100"><defs>{grouped}</defs><path d="M0 0{corners}" stroke="red" marker-mid="url(#k)"/></svg>"##), 96);
}

/// Reading costs too: each element that a `<use>` brings in is styled afresh, each time
/// going through every rule of the file's style sheets. Three hundred rules, and shapes that
/// draw nothing (which no bound counted), used a million times: 40 s in a release build.
#[test]
fn style_sheets_cost_no_more_to_read_than_the_file_could_write_out() {
    let sheet = format!("<style>{}</style>", "path{fill:red}".repeat(300));
    drawn_in_time(&used_over_and_over(&sheet, r#"<path d="M0 0"/>"#, 10), 16);
}

/// How much memory the thread that runs this has taken, and the most it has held.
mod held {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static HELD: Cell<isize> = const { Cell::new(0) };
        static MOST: Cell<isize> = const { Cell::new(0) };
    }
    fn more(bytes: isize) {
        let held = HELD.get() + bytes;
        HELD.set(held);
        MOST.set(MOST.get().max(held));
    }
    pub struct Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            more(layout.size() as isize);
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            more(layout.size() as isize);
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
            more(-(layout.size() as isize));
            unsafe { System.dealloc(at, layout) }
        }
        unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            more(size as isize - layout.size() as isize);
            unsafe { System.realloc(at, layout, size) }
        }
    }
    /// The most this thread held beyond what it held before, while `run` ran.
    pub fn most(run: impl FnOnce()) -> isize {
        let before = HELD.get();
        MOST.set(before);
        run();
        MOST.get() - before
    }
}
#[global_allocator]
static COUNTING: held::Counting = held::Counting;

/// Text is written in the system's fonts, and a letter the font of its family lacks in the
/// font the system stands in with: which was looked for, and kept, once for each such letter,
/// though it is the same font for all of them. Three hundred letters of the private area that
/// no font has took 330 MB (a few thousand, gigabytes), and as many searches through all of
/// the system's fonts (with the feature `fallback-fonts`, some 40 ms each on Windows); and
/// three hundred pieces of text, each naming a family of its own, as many fonts read.
#[test]
fn letters_no_font_has_cost_no_more_than_the_file_could_write_out() {
    taut::svg::enable_text();
    let letters: String = (0xE000u32..0xE000 + 300).filter_map(char::from_u32).collect();
    let most = held::most(|| drop(taut::svg::decode(&format!(r#"<svg viewBox="0 0 100 100"><text y="50">{letters}</text></svg>"#), 16, 16)));
    assert!(most < 32 << 20, "300 letters no font has took {most} bytes");
    let pieces: String = (0..300).map(|n| format!(r#"<tspan font-family="F{n}">x</tspan>"#)).collect();
    let most = held::most(|| drawn_in_time(&format!(r#"<svg viewBox="0 0 100 100"><text y="50">{pieces}</text></svg>"#), 16));
    assert!(most < 32 << 20, "300 families of fonts took {most} bytes");
}

/// A piece of outline costs more than a piece where its line is wide: each crosses every row
/// it covers, and goes round its corners in an arc of up to 256 points. A line of 8,000 pieces
/// with round corners, far wider than the picture, drawn four times: 2 million points held
/// for one of them (255 MB at the most, as found).
#[test]
fn wide_lines_with_round_corners_cost_no_more_than_the_file_could_write_out() {
    let line = format!(r#"<path d="M50 50{}" fill="none" stroke="red" stroke-width="1e6" stroke-linejoin="round"/>"#, "l1 1l-1 0".repeat(4000));
    let most = held::most(|| drawn_in_time(&used_over_and_over("", &line, 1), 64));
    assert!(most < 32 << 20, "a wide line took {most} bytes");
}
