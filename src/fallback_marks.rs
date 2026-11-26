//! Marks placed on their letters in fonts without `GPOS`, as HarfBuzz's fallback positioning
//! places them (`hb-ot-shape-fallback.cc`): by the mark's combining class around the box of
//! the glyph before it (above, below, left or right; attached or a gap of 1/16 em away), each
//! class stacked on the marks of that class before it, and on a ligature over the component it
//! belongs to. Only `shaping.rs`'s plans for fonts without `GPOS` hand this over, for scripts
//! HarfBuzz shapes so (not those shaped by syllable).
use crate::font_file::{Bytes, FontFile, Placing};
use crate::shaping::Laid;

/// A box in font units, as HarfBuzz's glyph extents: left and top (up from the baseline), width
/// and height (downward: negative).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Extents {
    left: i64,
    top: i64,
    width: i64,
    height: i64,
}

/// Combining classes (Unicode's names).
const ATTACHED_BELOW_LEFT: u8 = 200;
const ATTACHED_BELOW: u8 = 202;
const ATTACHED_ABOVE: u8 = 214;
const ATTACHED_ABOVE_RIGHT: u8 = 216;
const BELOW_LEFT: u8 = 218;
const BELOW: u8 = 220;
const BELOW_RIGHT: u8 = 222;
const ABOVE_LEFT: u8 = 228;
const ABOVE: u8 = 230;
const ABOVE_RIGHT: u8 = 232;
const DOUBLE_BELOW: u8 = 233;
const DOUBLE_ABOVE: u8 = 234;

/// The class a mark character is placed by: Unicode's combining class as HarfBuzz modifies it
/// (`normalization::class`), and for a nonspacing mark (category Mn) as its fallback sorts the
/// classes of Hebrew, Arabic, Syriac, Thai, Lao and Tibetan points into above and below. None
/// for a character that is no mark.
pub(crate) fn class(letter: char) -> Option<u8> {
    if !crate::normalization::mark(letter) {
        return None;
    }
    let class = crate::normalization::class(letter);
    if !crate::normalization::nonspacing(letter) {
        return Some(class);
    }
    Some(placed(letter as u32, class))
}

/// A nonspacing mark's class as HarfBuzz's fallback takes it (`recategorize_combining_class`).
fn placed(code: u32, class: u8) -> u8 {
    if class >= 200 {
        return class;
    }
    // Thai and Lao marks of class 0 (and Thai's virama).
    let thai = match (code & !0xff == 0xe00, class, code) {
        (true, 0, 0xe31 | 0xe34..=0xe37 | 0xe47 | 0xe4c..=0xe4e) => ABOVE_RIGHT,
        (true, 0, 0xeb1 | 0xeb4..=0xeb7 | 0xebb | 0xecc | 0xecd) => ABOVE,
        (true, 0, 0xebc) => BELOW,
        (true, _, 0xe3a) => BELOW_RIGHT,
        _ => class,
    };
    // By HarfBuzz's own classes (see `normalization`): Hebrew points 10 to 26 (shin dot 10, sin
    // dot 11, dagesh 12, rafe 13, holam 14), Arabic and Syriac 27 to 36 (kasratan 30, kasra 33),
    // Thai 3 and 107, Lao 118 and 122, Tibetan 129, 131 and 132.
    match thai {
        15..=25 | 30 | 33 | 118 | 129 | 131 => BELOW,
        13 => ATTACHED_ABOVE,
        10 | 107 => ABOVE_RIGHT,
        11 | 14 => ABOVE_LEFT,
        26..=29 | 31 | 32 | 34..=36 | 122 | 132 => ABOVE,
        3 => BELOW_RIGHT,
        class => class,
    }
}

/// Which way a run's glyphs follow each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    LeftToRight,
    RightToLeft,
    TopToBottom,
}

impl Direction {
    /// Down a column (`downward`), else right to left (`leftward`) or left to right.
    pub(crate) fn of(downward: bool, leftward: bool) -> Direction {
        match (downward, leftward) {
            (true, _) => Direction::TopToBottom,
            (false, true) => Direction::RightToLeft,
            (false, false) => Direction::LeftToRight,
        }
    }
}

/// Places the marks of a run (glyphs in logical order, already shaped, mark advances zeroed)
/// on the glyphs before them.
pub(crate) fn place(font: &FontFile, glyphs: &mut [Laid], direction: Direction) {
    let mut start = 0;
    for n in 1..=glyphs.len() {
        if n == glyphs.len() || glyphs[n].combining.is_none() {
            place_cluster(font, &mut glyphs[start..n], direction);
            start = n;
        }
    }
}

/// Places the marks after each base of a cluster: a base and the marks after it.
fn place_cluster(font: &FontFile, glyphs: &mut [Laid], direction: Direction) {
    if glyphs.len() < 2 {
        return;
    }
    // (Marks before the first base stay as they are.)
    let Some(base) = glyphs.iter().position(|laid| laid.combining.is_none()) else { return };
    place_around(font, &mut glyphs[base..], direction);
}

/// Places the marks after a base glyph (`glyphs[0]`) around its box: its height, and its
/// advance for its width. (In vertical text too, the box is the horizontal one: HarfBuzz's
/// fallback knows no other.)
fn place_around(font: &FontFile, glyphs: &mut [Laid], direction: Direction) {
    let base = glyphs[0];
    let mut own = extents(font, base.glyph);
    own.top += base.offset[1] as i64;
    (own.left, own.width) = (0, font.width(base.glyph) as i64);
    let parts = base.parts() as i64;
    // How far each mark's pen is from the base's: back over the base where text runs forward
    // (in vertical text, up).
    let back = |laid: &Laid| match direction {
        Direction::LeftToRight => [-(laid.advance as i64), 0],
        Direction::RightToLeft => [laid.advance as i64, 0],
        Direction::TopToBottom => [0, laid.advance as i64],
    };
    let mut pen = if direction == Direction::RightToLeft { [0; 2] } else { back(&base) };
    let (mut part_box, mut stack, mut last_part, mut last_class) = (own, own, None, None);
    for laid in &mut glyphs[1..] {
        let Some(class) = laid.combining.filter(|class| *class != 0) else {
            let [x, y] = back(laid);
            pen = [pen[0] + x, pen[1] + y];
            continue;
        };
        if parts > 1 {
            // A mark of another ligature, or with no component, goes on the last one.
            let own_part = laid.component() as i64 - 1;
            let part = if base.ligature() == 0 || base.ligature() != laid.ligature() || !(0..parts).contains(&own_part) { parts - 1 } else { own_part };
            if last_part != Some(part) {
                let from = if direction == Direction::RightToLeft { parts - 1 - part } else { part };
                part_box = Extents { left: own.left + from * own.width / parts, width: own.width / parts, ..own };
                (last_part, last_class) = (Some(part), None);
            }
        }
        if last_class != Some(class) {
            (stack, last_class) = (part_box, Some(class));
        }
        let placed = mark_place(&mut stack, extents(font, laid.glyph), class, direction, font.units() as i64 / 16);
        laid.advance = 0;
        laid.offset = [0, 1].map(|axis| (placed[axis] + pen[axis]).clamp(i32::MIN as i64, i32::MAX as i64) as i32);
    }
}

/// Where a mark of a class goes from the pen, on a box (`stack`), which grows by the mark: as
/// `position_mark` has it. `gap` is the space between a mark and the box it is not attached to.
fn mark_place(stack: &mut Extents, mark: Extents, class: u8, direction: Direction, gap: i64) -> [i64; 2] {
    let x = match (class, direction) {
        (DOUBLE_BELOW | DOUBLE_ABOVE, Direction::LeftToRight) => stack.left + stack.width - mark.width / 2 - mark.left,
        (DOUBLE_BELOW | DOUBLE_ABOVE, Direction::RightToLeft) => stack.left - mark.width / 2 - mark.left,
        (ATTACHED_BELOW_LEFT | BELOW_LEFT | ABOVE_LEFT, _) => stack.left - mark.left,
        (ATTACHED_ABOVE_RIGHT | BELOW_RIGHT | ABOVE_RIGHT, _) => stack.left + stack.width - mark.width - mark.left,
        _ => stack.left + (stack.width - mark.width) / 2 - mark.left,
    };
    let y = match class {
        DOUBLE_BELOW | BELOW_LEFT | BELOW | BELOW_RIGHT | ATTACHED_BELOW_LEFT | ATTACHED_BELOW => below(stack, mark, class, gap),
        DOUBLE_ABOVE | ABOVE_LEFT | ABOVE | ABOVE_RIGHT | ATTACHED_ABOVE | ATTACHED_ABOVE_RIGHT => above(stack, mark, class, gap),
        _ => 0,
    };
    [x, y]
}

/// How far up a mark below the box goes (never up), and the box grown down by it.
fn below(stack: &mut Extents, mark: Extents, class: u8, gap: i64) -> i64 {
    if !matches!(class, ATTACHED_BELOW_LEFT | ATTACHED_BELOW) {
        stack.height -= gap;
    }
    let mut y = stack.top + stack.height - mark.top;
    if (gap > 0) == (y > 0) {
        (stack.height, y) = (stack.height - y, 0);
    }
    stack.height += mark.height;
    y
}

/// How far up a mark above the box goes (not far down), and the box grown up by it.
fn above(stack: &mut Extents, mark: Extents, class: u8, gap: i64) -> i64 {
    if !matches!(class, ATTACHED_ABOVE | ATTACHED_ABOVE_RIGHT) {
        (stack.top, stack.height) = (stack.top + gap, stack.height - gap);
    }
    let mut y = stack.top - mark.top - mark.height;
    if (gap > 0) != (y > 0) {
        let half = -(y / 2);
        (stack.top, stack.height, y) = (stack.top + half, stack.height - half, y + half);
    }
    (stack.top, stack.height) = (stack.top - mark.height, stack.height + mark.height);
    y
}

/// A glyph's box as HarfBuzz reads it: for a `glyf` glyph at the font's default, its header's
/// box with the `hmtx` left side bearing for its left; else its outline's points' bounds,
/// rounded. (An empty glyph's box is empty.)
fn extents(font: &FontFile, glyph: u16) -> Extents {
    if let Some(header) = font.points(glyph).filter(|_| font.varied.is_none()) {
        let [low_x, low_y, high_x, high_y] = [2, 4, 6, 8].map(|at| header.i16(at).unwrap_or(0) as i64);
        let left = font.edge(glyph).map_or(low_x.min(high_x), |edge| edge as i64);
        return Extents { left, top: low_y.max(high_y), width: (high_x - low_x).abs(), height: low_y.min(high_y) - low_y.max(high_y) };
    }
    let [left, low, right, high] = font.sketch(glyph, Placing([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])).reach;
    let (left, width) = if left < right { (left.round(), (right - left.round()).round()) } else { (0.0, 0.0) };
    let (top, height) = if low < high { (high.round(), (low - high.round()).round()) } else { (0.0, 0.0) };
    Extents { left: left as i64, top: top as i64, width: width as i64, height: height as i64 }
}

/// Whether a font's `kern` table kerns across the line (moving glyphs up and down): HarfBuzz
/// then leaves marks as the table puts them.
pub(crate) fn kerns_across(kern: Bytes) -> bool {
    match kern.u16(0) {
        // OpenType's: subtables of a version, length and coverage (cross-stream bit 2).
        Some(0) => {
            let mut at = 4;
            (0..kern.u16(2).unwrap_or(0).min(64)).any(|_| {
                let (long, coverage) = (kern.u16(at + 2).unwrap_or(0) as usize, kern.u16(at + 4).unwrap_or(0));
                at += long.max(6);
                coverage & 4 != 0
            })
        }
        // Apple's: a length of four bytes, then coverage (cross-stream bit 14).
        Some(1) => {
            let mut at = 8;
            (0..kern.u32(4).unwrap_or(0).min(64)).any(|_| {
                let (long, coverage) = (kern.u32(at).unwrap_or(0) as usize, kern.u16(at + 4).unwrap_or(0));
                at += long.max(8);
                coverage & 0x4000 != 0
            })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_sorted_above_and_below_as_harfbuzz_sorts_them() {
        // Acute above, dot below, horn attached above right, ogonek attached below; Hebrew
        // sheva and shin dot, Arabic fatha and kasra, Thai mai ek and sara u; a spacing mark
        // keeps its class; letters have none.
        let letters = ['\u{301}', '\u{323}', '\u{31b}', '\u{328}', '\u{5b0}', '\u{5c1}', '\u{64e}', '\u{650}', '\u{e48}', '\u{e38}', '\u{903}', 'a'];
        let classes =
            [Some(ABOVE), Some(BELOW), Some(ATTACHED_ABOVE_RIGHT), Some(ATTACHED_BELOW), Some(BELOW), Some(ABOVE_RIGHT), Some(ABOVE), Some(BELOW), Some(ABOVE_RIGHT), Some(BELOW_RIGHT), Some(0), None];
        assert_eq!(letters.map(class), classes);
    }

    #[test]
    fn a_mark_above_sits_a_sixteenth_of_an_em_over_its_letter_and_the_next_over_it() {
        // A letter 500 wide reaching from 0 to 700, two marks 100 wide from 0 to 50 above it.
        let mut stack = Extents { left: 0, top: 700, width: 500, height: -700 };
        let mark = Extents { left: 10, top: 50, width: 100, height: -50 };
        let first = mark_place(&mut stack, mark, ABOVE, Direction::LeftToRight, 64);
        let second = mark_place(&mut stack, mark, ABOVE, Direction::LeftToRight, 64);
        // Centred: (500 - 100) / 2 - 10. Up: 700 + 64 - 50 + 50 = 764; then 50 higher, and the gap
        // again.
        assert_eq!([first, second], [[190, 764], [190, 878]]);
        // Below, never up: from the letter's bottom less the gap.
        let mut stack = Extents { left: 0, top: 700, width: 500, height: -700 };
        assert_eq!(mark_place(&mut stack, mark, BELOW, Direction::LeftToRight, 64), [190, -114]);
    }

    /// A font's file without its `GPOS` table.
    #[cfg(target_os = "linux")]
    fn without_positions(file: &[u8], start: usize) -> Vec<u8> {
        let all = Bytes(file);
        let count = all.u16(start + 4).unwrap_or(0) as usize;
        let mut tables = Vec::new();
        for n in 0..count {
            let record = start + 12 + n * 16;
            let name: [u8; 4] = file[record..record + 4].try_into().unwrap();
            let (at, long) = (all.u32(record + 8).unwrap() as usize, all.u32(record + 12).unwrap() as usize);
            if &name != b"GPOS" {
                tables.push((name, file[at..at + long].to_vec()));
            }
        }
        crate::font_file::tests::file(&file[start..start + 4].try_into().unwrap(), &tables)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn marks_in_fonts_without_positions_come_where_harfbuzz_puts_them() {
        use crate::scripts::Language;
        // Letters with marks of every class, alone, stacked, in ligatures and in kerned pairs.
        let mut words: Vec<String> = [
            "q\u{301}",
            "x\u{323}",
            "w\u{30c}\u{323}",
            "q\u{307}\u{323}\u{301}\u{330}",
            "n\u{303}\u{303}\u{303}",
            "y\u{31b}\u{301}",
            "j\u{328}",
            "g\u{334}",
            "k\u{35c}l",
            "m\u{361}n",
            "z\u{345}",
            "A\u{30a}\u{301}",
            "VA\u{301}W\u{323}",
            "f\u{301}i",
            "fi\u{323}",
            "ffi\u{301}\u{302}\u{303}",
            "T\u{30f}o",
            "\u{301}a",
            "i\u{31b}\u{328}\u{324}",
            "\u{436}\u{301}\u{328}",
            "\u{3b1}\u{313}\u{301}\u{345}",
            "\u{3c9}\u{342}\u{345}",
            "b\u{20dd}\u{301}",
            "c\u{489}",
            "p\u{1dc4}\u{1dca}",
        ]
        .map(String::from)
        .to_vec();
        if cfg!(feature = "right-to-left") {
            words.extend(["\u{5e9}\u{5c1}\u{5b8}", "\u{5d1}\u{5bc}\u{5b0}", "\u{5dc}\u{5b9}", "\u{628}\u{64e}\u{651}", "\u{628}\u{650}\u{628}", "\u{644}\u{627}\u{64b}"].map(String::from));
        }
        let folder = std::env::temp_dir().join(format!("taut-fallback-marks-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (mut held, mut differ) = (0, Vec::new());
        for (path, face) in crate::shaping::held::fonts() {
            let Some(font) = FontFile::open(&path).and_then(|font| if face == 0 { Some(font) } else { font.nth(face as usize) }) else { continue };
            let stripped = folder.join(format!("{}-{face}.ttf", path.file_stem().unwrap().to_string_lossy()));
            std::fs::write(&stripped, without_positions(font.bytes().0, font.start)).unwrap();
            let Some(font) = FontFile::open(&stripped) else { continue };
            // (Only words whose letters the font has: HarfBuzz 12 and 14 give .notdef different boxes.)
            let has = |word: &&String| word.chars().all(|letter| font.glyph(letter) != 0);
            let these: Vec<String> = words.iter().filter(has).cloned().collect();
            let Some(theirs) = crate::shaping::harfbuzz::harfbuzz_in(&stripped, 0, &these, "") else { return eprintln!("no HarfBuzz here: not tested") };
            let mut plans = Vec::new();
            for (word, theirs) in these.iter().zip(theirs) {
                let ours = crate::shaping::held::ours(&font, &mut plans, word, Language::NONE);
                if ours != theirs {
                    differ.push(format!("{}: {word} ({}): {ours:?}, not {theirs:?}", path.display(), word.escape_unicode()));
                }
            }
            held += these.len();
        }
        let _ = std::fs::remove_dir_all(&folder);
        println!("{held} words held against HarfBuzz, {} differ", differ.len());
        differ.iter().take(12).for_each(|line| println!("  {line}"));
        assert!(held > 0 && differ.is_empty(), "{} words differ", differ.len());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn marks_in_vertical_text_in_fonts_without_positions_come_where_harfbuzz_puts_them() {
        use crate::scripts::Language;
        use crate::shaping::{Plan, Script};
        // Kana with voicing marks, and letters with marks above, below and stacked, down a
        // column: each glyph's place from the first's pen (up), and the column's length.
        let words: Vec<String> = ["\u{304b}\u{3099}", "\u{306f}\u{309a}\u{3068}", "q\u{301}", "x\u{323}", "w\u{30c}\u{323}", "n\u{303}\u{303}", "j\u{328}", "A\u{30a}\u{301}", "\u{436}\u{301}\u{328}"]
            .map(String::from)
            .to_vec();
        let east_asian = [
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/mnt/c/Windows/Fonts/YuGothR.ttc",
            "/mnt/c/Windows/Fonts/meiryo.ttc",
            "/mnt/c/Windows/Fonts/msgothic.ttc",
        ];
        let fonts = crate::shaping::held::fonts().into_iter().chain(east_asian.map(|path| (std::path::PathBuf::from(path), 0)));
        let folder = std::env::temp_dir().join(format!("taut-fallback-marks-down-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (mut held, mut differ) = (0, Vec::new());
        for (path, face) in fonts {
            let Some(font) = FontFile::open(&path).and_then(|font| if face == 0 { Some(font) } else { font.nth(face as usize) }) else { continue };
            let stripped = folder.join(format!("{}-{face}.ttf", path.file_stem().unwrap().to_string_lossy()));
            std::fs::write(&stripped, without_positions(font.bytes().0, font.start)).unwrap();
            let Some(font) = FontFile::open(&stripped) else { continue };
            let these: Vec<String> = words.iter().filter(|word| word.chars().all(|letter| font.glyph(letter) != 0)).cloned().collect();
            let Some(theirs) = crate::shaping::harfbuzz::harfbuzz_downward(&stripped, 0, &these) else { return eprintln!("no HarfBuzz here: not tested") };
            for (word, theirs) in these.iter().zip(theirs) {
                let script = word.chars().find_map(Script::of).unwrap_or(Script::None);
                let mut glyphs = Vec::new();
                let total = font.shape(&Plan::downward(&font, script, Language::NONE), word, &mut glyphs);
                let mut pen = 0;
                let ours: Vec<(u16, [i32; 2])> = glyphs.iter().map(|laid| (laid.glyph, [laid.offset[0], (pen, pen -= laid.advance).0 + laid.offset[1]])).collect();
                if (ours.clone(), -total) != theirs {
                    differ.push(format!("{}: {word} ({}): {ours:?} {}, not {theirs:?}", path.display(), word.escape_unicode(), -total));
                }
            }
            held += these.len();
        }
        let _ = std::fs::remove_dir_all(&folder);
        println!("{held} words held against HarfBuzz, {} differ", differ.len());
        differ.iter().take(12).for_each(|line| println!("  {line}"));
        assert!(held > 0 && differ.is_empty(), "{} words differ", differ.len());
    }
}
