//! Right-to-left text (Hebrew, Arabic), alone and mixed with left-to-right text: the order
//! characters are shown in.
//!
//! Characters are kept in logical order, and a line is displayed by the Unicode Bidirectional
//! Algorithm (UAX #9). Each character has a bidi class: a strong letter of either direction, a
//! digit, a number separator or terminator, whitespace, a nonspacing mark, an embedding,
//! override or isolate control. From those every character gets an embedding level: even for
//! left-to-right, odd for right-to-left, higher when nested. A paragraph's level is its first
//! strong letter's, unless the app says. The line is then shown in runs of one level, odd ones
//! reversed, nested runs reversed with them; a bracket in right-to-left text is mirrored, so it
//! still opens toward its content.
//!
//! The whole algorithm is here: the classes, explicit levels from embeddings, overrides and
//! isolates, weak types around numbers, paired brackets, and neutrals. Classes are Unicode's,
//! stored as ranges; nonspacing marks of left-to-right scripts count as letters, which they
//! attach to anyway (and which keeps the table short).
//!
//! All of this is the feature `right-to-left`, on unless a program turns it off
//! (`default-features = false`) to save its size. Without it every line is shown as typed, left
//! to right: Hebrew and Arabic letters appear in their isolated forms, in logical order.
//!
//! Indic and Southeast Asian scripts run left to right, but their glyphs are not in typed
//! order: with the feature `brahmic`, their runs are shaped here too (`brahmic.rs`) from the
//! system font for them, and each syllable's characters share the width its glyphs take, which
//! the cursor steps over whole.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::FontFile;
use crate::scripts::Language;
use crate::shaping::{Laid, Plan, Script};
use crate::text::{Face, Font, Metrics, TextSystem};
use std::ops::Range;

/// A character's bidi class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Left-to-right strong (L).
    Left,
    /// Right-to-left strong: Hebrew and similar (R).
    Right,
    /// Arabic letter (AL).
    Arabic,
    /// European number (EN).
    Figure,
    /// European separator: plus or minus (ES).
    Sign,
    /// European terminator: currency, percent, degree (ET).
    Unit,
    /// Arabic number (AN).
    ArabicFigure,
    /// Common separator: comma, full stop, colon, slash (CS).
    Between,
    /// Nonspacing mark (NSM).
    Mark,
    /// Boundary neutral (BN).
    Unseen,
    /// Paragraph separator (B).
    Paragraph,
    /// Segment separator: tab (S).
    Tab,
    /// Whitespace (WS).
    Space,
    /// Other neutral: punctuation, symbols, emoji (ON).
    Other,
    /// Left-to-right embedding (LRE),
    LeftEmbedding,
    /// left-to-right override (LRO),
    LeftOverride,
    /// right-to-left embedding (RLE),
    RightEmbedding,
    /// right-to-left override (RLO),
    RightOverride,
    /// pop directional formatting (PDF).
    PopEmbedding,
    /// Left-to-right isolate (LRI),
    LeftIsolate,
    /// right-to-left isolate (RLI),
    RightIsolate,
    /// first strong isolate (FSI),
    FirstIsolate,
    /// pop directional isolate (PDI).
    PopIsolate,
}

/// The classes in the table's numbering.
const ALL: [Kind; 23] = [
    Kind::Left,
    Kind::Right,
    Kind::Arabic,
    Kind::Figure,
    Kind::Sign,
    Kind::Unit,
    Kind::ArabicFigure,
    Kind::Between,
    Kind::Mark,
    Kind::Unseen,
    Kind::Paragraph,
    Kind::Tab,
    Kind::Space,
    Kind::Other,
    Kind::LeftEmbedding,
    Kind::LeftOverride,
    Kind::RightEmbedding,
    Kind::RightOverride,
    Kind::PopEmbedding,
    Kind::LeftIsolate,
    Kind::RightIsolate,
    Kind::FirstIsolate,
    Kind::PopIsolate,
];

/// Every character's class, as ranges: first character (times 32) plus class number (below 32).
/// Unicode's data, except left-to-right nonspacing marks count as letters.
#[rustfmt::skip]
const KINDS: [u32; 662] = [
    0x9, 0x12b, 0x14a, 0x16b, 0x18c, 0x1aa, 0x1c9, 0x38a, 0x3eb, 0x40c, 0x42d, 0x465,
    0x4cd, 0x564, 0x587, 0x5a4, 0x5c7, 0x603, 0x747, 0x76d, 0x820, 0xb6d, 0xc20, 0xf6d,
    0xfe9, 0x10aa, 0x10c9, 0x1407, 0x142d, 0x1445, 0x14cd, 0x1540, 0x156d, 0x15a9, 0x15cd, 0x1605,
    0x1643, 0x168d, 0x16a0, 0x16cd, 0x1723, 0x1740, 0x176d, 0x1800, 0x1aed, 0x1b00, 0x1eed, 0x1f00,
    0x572d, 0x5760, 0x584d, 0x5a00, 0x5a4d, 0x5c00, 0x5cad, 0x5dc0, 0x5ded, 0x6008, 0x6e00, 0x6e8d,
    0x6ec0, 0x6fcd, 0x6fe0, 0x708d, 0x70c0, 0x70ed, 0x7100, 0x7ecd, 0x7ee0, 0x9068, 0x9140, 0xb14d,
    0xb160, 0xb1ad, 0xb1e5, 0xb201, 0xb228, 0xb7c1, 0xb7e8, 0xb801, 0xb828, 0xb861, 0xb888, 0xb8c1,
    0xb8e8, 0xb941, 0xc006, 0xc0cd, 0xc102, 0xc125, 0xc162, 0xc187, 0xc1a2, 0xc1cd, 0xc208, 0xc362,
    0xc968, 0xcc06, 0xcd45, 0xcd66, 0xcda2, 0xce08, 0xce22, 0xdac8, 0xdba6, 0xdbcd, 0xdbe8, 0xdca2,
    0xdce8, 0xdd2d, 0xdd48, 0xddc2, 0xde03, 0xdf42, 0xe228, 0xe242, 0xe608, 0xe962, 0xf4c8, 0xf622,
    0xf801, 0xfd68, 0xfe81, 0xfecd, 0xff41, 0xffa8, 0xffc1, 0x102c8, 0x10341, 0x10368, 0x10481, 0x104a8,
    0x10501, 0x10528, 0x105c1, 0x10b28, 0x10b81, 0x10c02, 0x11206, 0x11242, 0x112e8, 0x11402, 0x11948, 0x11c46,
    0x11c68, 0x12000, 0x13e45, 0x13e80, 0x13f65, 0x13f80, 0x15e25, 0x15e40, 0x17e6d, 0x17f25, 0x17f4d, 0x17f60,
    0x18f0d, 0x18fe0, 0x1c7e5, 0x1c800, 0x1e74d, 0x1e7c0, 0x2720d, 0x27340, 0x2800d, 0x28020, 0x2d00c, 0x2d020,
    0x2d36d, 0x2d3a0, 0x2fb65, 0x2fb80, 0x2fe0d, 0x2ff40, 0x3000d, 0x30160, 0x301c9, 0x301e0, 0x3280d, 0x32820,
    0x3288d, 0x328c0, 0x33bcd, 0x34000, 0x35608, 0x35e20, 0x3b808, 0x3c000, 0x3f7ad, 0x3f7c0, 0x3f7ed, 0x3f840,
    0x3f9ad, 0x3fa00, 0x3fbad, 0x3fc00, 0x3fdad, 0x3fe00, 0x3ffad, 0x3ffe0, 0x4000c, 0x40169, 0x401c0, 0x401e1,
    0x4020d, 0x4050c, 0x4052a, 0x4054e, 0x40570, 0x40592, 0x405af, 0x405d1, 0x405e7, 0x40605, 0x406ad, 0x40887,
    0x408ad, 0x40bec, 0x40c09, 0x40cd3, 0x40cf4, 0x40d15, 0x40d36, 0x40d49, 0x40e03, 0x40e20, 0x40e83, 0x40f44,
    0x40f8d, 0x40fe0, 0x41003, 0x41144, 0x4118d, 0x41200, 0x41405, 0x41a08, 0x41e20, 0x4200d, 0x42040, 0x4206d,
    0x420e0, 0x4210d, 0x42140, 0x4228d, 0x422a0, 0x422cd, 0x42320, 0x423cd, 0x42480, 0x424ad, 0x424c0, 0x424ed,
    0x42500, 0x4252d, 0x42540, 0x425c5, 0x425e0, 0x4274d, 0x42780, 0x4280d, 0x428a0, 0x4294d, 0x429c0, 0x42a0d,
    0x42c00, 0x4312d, 0x43180, 0x4320d, 0x44244, 0x44265, 0x4428d, 0x466c0, 0x46f6d, 0x472a0, 0x472cd, 0x48540,
    0x4880d, 0x48960, 0x48c0d, 0x49103, 0x49380, 0x49d4d, 0x4d580, 0x4d5ad, 0x50000, 0x5200d, 0x56e80, 0x56ecd,
    0x58000, 0x59cad, 0x59d60, 0x59f2d, 0x5a000, 0x5c00d, 0x5cbc0, 0x5cc0d, 0x5cc80, 0x5d00d, 0x5d340, 0x5d36d,
    0x5de80, 0x5e00d, 0x5fac0, 0x5fe0d, 0x6000c, 0x6002d, 0x600a0, 0x6010d, 0x60420, 0x6060d, 0x60620, 0x606cd,
    0x60700, 0x607ad, 0x60800, 0x6136d, 0x613a0, 0x6140d, 0x61420, 0x61f6d, 0x61f80, 0x6380d, 0x63cc0, 0x63ded,
    0x63e00, 0x643ad, 0x643e0, 0x64a0d, 0x64c00, 0x64f8d, 0x64fe0, 0x6562d, 0x65800, 0x6598d, 0x65a00, 0x66eed,
    0x66f60, 0x67bcd, 0x67c00, 0x67fed, 0x68000, 0x9b80d, 0x9c000, 0x14920d, 0x1498e0, 0x14c1ad, 0x14c200, 0x14ce6d,
    0x14ce80, 0x14cfcd, 0x14d000, 0x14e00d, 0x14e440, 0x14f10d, 0x14f120, 0x15050d, 0x150580, 0x150705, 0x150740, 0x150e8d,
    0x150f00, 0x156d4d, 0x156d80, 0x1f63a1, 0x1f63c8, 0x1f63e1, 0x1f6524, 0x1f6541, 0x1f6a02, 0x1f786d, 0x1f7a62, 0x1fa7cd,
    0x1faa02, 0x1fb20d, 0x1fb242, 0x1fb90d, 0x1fba09, 0x1fbe02, 0x1fbfad, 0x1fc008, 0x1fc20d, 0x1fc340, 0x1fc408, 0x1fc60d,
    0x1fca07, 0x1fca2d, 0x1fca47, 0x1fca60, 0x1fca8d, 0x1fcaa7, 0x1fcacd, 0x1fcbe5, 0x1fcc0d, 0x1fcc44, 0x1fcc8d, 0x1fcce0,
    0x1fcd0d, 0x1fcd25, 0x1fcd6d, 0x1fcd80, 0x1fce02, 0x1fdfe9, 0x1fe000, 0x1fe02d, 0x1fe065, 0x1fe0cd, 0x1fe164, 0x1fe187,
    0x1fe1a4, 0x1fe1c7, 0x1fe203, 0x1fe347, 0x1fe36d, 0x1fe420, 0x1fe76d, 0x1fe820, 0x1feb6d, 0x1fecc0, 0x1ffc05, 0x1ffc4d,
    0x1ffca5, 0x1ffce0, 0x1ffd0d, 0x1ffde0, 0x1ffe09, 0x1fff2d, 0x1fffc9, 0x200000, 0x20202d, 0x202040, 0x20280d, 0x2031a0,
    0x20320d, 0x2033a0, 0x20340d, 0x203420, 0x205c23, 0x205f80, 0x210001, 0x2123ed, 0x212401, 0x214028, 0x214081, 0x2140a8,
    0x2140e1, 0x214188, 0x214201, 0x214708, 0x214761, 0x2147e8, 0x214801, 0x215ca8, 0x215ce1, 0x21672d, 0x216801, 0x21a002,
    0x21a488, 0x21a502, 0x21a606, 0x21a742, 0x21a806, 0x21a941, 0x21ad28, 0x21adcd, 0x21ade1, 0x21cc06, 0x21cfe1, 0x21d568,
    0x21d5a1, 0x21d802, 0x21d968, 0x21da0d, 0x21db22, 0x21de08, 0x21e001, 0x21e602, 0x21e8c8, 0x21ea22, 0x21ee01, 0x21f048,
    0x21f0c1, 0x220000, 0x220a4d, 0x220cc0, 0x22cc0d, 0x22cda0, 0x23faad, 0x23fba5, 0x23fc2d, 0x23fe40, 0x2dfc4d, 0x2dfc60,
    0x379409, 0x379480, 0x39800d, 0x399ac0, 0x399e03, 0x399f4d, 0x399fa0, 0x39a00d, 0x39d680, 0x39d74d, 0x39da20, 0x39da4d,
    0x39daa0, 0x39dbad, 0x39dfc0, 0x3a2e69, 0x3a2f60, 0x3a3d2d, 0x3a3d60, 0x3a400d, 0x3a4840, 0x3a48ad, 0x3a48c0, 0x3a600d,
    0x3a6ae0, 0x3ad82d, 0x3ad840, 0x3adb6d, 0x3adb80, 0x3adf6d, 0x3adf80, 0x3ae2ad, 0x3ae2c0, 0x3ae6ad, 0x3ae6c0, 0x3ae9ed,
    0x3aea00, 0x3aeded, 0x3aee00, 0x3af12d, 0x3af140, 0x3af52d, 0x3af540, 0x3af86d, 0x3af880, 0x3af9c3, 0x3b0000, 0x3b600d,
    0x3b63a0, 0x3c5fe5, 0x3c6000, 0x3d0001, 0x3d1a08, 0x3d1ae1, 0x3d2888, 0x3d2961, 0x3d8e02, 0x3d9801, 0x3da002, 0x3daa01,
    0x3dc002, 0x3dde0d, 0x3dde42, 0x3de001, 0x3e000d, 0x3e0580, 0x3e060d, 0x3e1280, 0x3e140d, 0x3e15e0, 0x3e162d, 0x3e1800,
    0x3e182d, 0x3e1a00, 0x3e1a2d, 0x3e1ec0, 0x3e2003, 0x3e216d, 0x3e2200, 0x3e25ed, 0x3e2600, 0x3e2d4d, 0x3e2e00, 0x3e35ad,
    0x3e35c0, 0x3e4c0d, 0x3e4cc0, 0x3e600d, 0x3edb40, 0x3edb8d, 0x3edda0, 0x3ede0d, 0x3edfa0, 0x3ee00d, 0x3efb40, 0x3efb6d,
    0x3efb80, 0x3efc0d, 0x3efd80, 0x3efe0d, 0x3f0180, 0x3f020d, 0x3f0900, 0x3f0a0d, 0x3f0b40, 0x3f0c0d, 0x3f1100, 0x3f120d,
    0x3f15c0, 0x3f160d, 0x3f1780, 0x3f180d, 0x3f1840, 0x3f1a0d, 0x3f1b20, 0x3f200d, 0x3f4b00, 0x3f4c0d, 0x3f4dc0, 0x3f4e0d,
    0x3f4fa0, 0x3f500d, 0x3f58e0, 0x3f590d, 0x3f5920, 0x3f598d, 0x3f5bc0, 0x3f5bed, 0x3f5d80, 0x3f5ded, 0x3f5f60, 0x3f600d,
    0x3f7260, 0x3f728d, 0x3f7e03, 0x3f7f4d, 0x3f7f60, 0x3fffc9, 0x400000, 0x5fffc9, 0x600000, 0x7fffc9, 0x800000, 0x9fffc9,
    0xa00000, 0xbfffc9, 0xc00000, 0xdfffc9, 0xe00000, 0xffffc9, 0x1000000, 0x11fffc9, 0x1200000, 0x13fffc9, 0x1400000, 0x15fffc9,
    0x1600000, 0x17fffc9, 0x1800000, 0x19fffc9, 0x1a00000, 0x1bfffc9, 0x1c02008, 0x1c03e09, 0x1c20000, 0x1dfffc9, 0x1e00000, 0x1ffffc9,
    0x2000000, 0x21fffc9,
];

/// Mirroring pairs: consecutive pairs, as ranges (the first of the first pair and the first of
/// the last),
#[rustfmt::skip]
const NEIGHBOURS: [(u16, u16); 62] = [
    (0x28, 0x28), (0xf3a, 0xf3c), (0x169b, 0x169b), (0x2039, 0x2039), (0x2045, 0x2045), (0x207d, 0x207d), (0x208d, 0x208d), (0x223c, 0x223c), (0x2252, 0x2254), (0x2264, 0x226a),
    (0x226e, 0x228a), (0x228f, 0x2291), (0x22a2, 0x22a2), (0x22b0, 0x22b6), (0x22c9, 0x22cb), (0x22d0, 0x22d0), (0x22d6, 0x22ec), (0x22f0, 0x22f0), (0x2308, 0x230a), (0x2329, 0x2329),
    (0x2768, 0x2774), (0x27c3, 0x27c5), (0x27c8, 0x27c8), (0x27d5, 0x27d5), (0x27dd, 0x27dd), (0x27e2, 0x27ee), (0x2983, 0x298b), (0x298e, 0x298e), (0x2991, 0x2997), (0x29a4, 0x29a4),
    (0x29a8, 0x29ae), (0x29c0, 0x29c0), (0x29c4, 0x29c4), (0x29cf, 0x29d1), (0x29d4, 0x29d4), (0x29d8, 0x29da), (0x29e8, 0x29e8), (0x29f8, 0x29f8), (0x29fc, 0x29fc), (0x2a2b, 0x2a2d),
    (0x2a34, 0x2a34), (0x2a3c, 0x2a3c), (0x2a64, 0x2a64), (0x2a79, 0x2aa1), (0x2aa6, 0x2aac), (0x2aaf, 0x2ad5), (0x2aec, 0x2aec), (0x2af7, 0x2af9), (0x2e02, 0x2e04), (0x2e09, 0x2e09),
    (0x2e0c, 0x2e0c), (0x2e1c, 0x2e1c), (0x2e20, 0x2e28), (0x2e55, 0x2e5b), (0x2e62, 0x2e62), (0x3008, 0x3010), (0x3014, 0x301a), (0xfe59, 0xfe5d), (0xfe64, 0xfe64), (0xff08, 0xff08),
    (0xff5f, 0xff5f), (0xff62, 0xff62),
];
/// the other pairs,
#[rustfmt::skip]
const PAIRS: [(u16, u16); 31] = [
    (0x3c, 0x3e), (0x5b, 0x5d), (0x7b, 0x7d), (0xab, 0xbb), (0x2208, 0x220b), (0x2209, 0x220c), (0x220a, 0x220d), (0x2215, 0x29f5), (0x221f, 0x2bfe), (0x2220, 0x29a3),
    (0x2221, 0x299b), (0x2222, 0x29a0), (0x2224, 0x2aee), (0x2243, 0x22cd), (0x2245, 0x224c), (0x2298, 0x29b8), (0x22a6, 0x2ade), (0x22a8, 0x2ae4), (0x22a9, 0x2ae3), (0x22ab, 0x2ae5),
    (0x22b8, 0x27dc), (0x22f2, 0x22fa), (0x22f3, 0x22fb), (0x22f4, 0x22fc), (0x22f6, 0x22fd), (0x22f7, 0x22fe), (0x27cb, 0x27cd), (0x298d, 0x2990), (0xff1c, 0xff1e), (0xff3b, 0xff3d),
    (0xff5b, 0xff5d),
];
/// and the opening brackets (each closed by its mirror).
#[rustfmt::skip]
const OPENING: [u16; 65] = [
    0x28, 0x5b, 0x7b, 0xf3a, 0xf3c, 0x169b, 0x2045, 0x207d, 0x208d, 0x2308, 0x230a, 0x2329, 0x2768, 0x276a,
    0x276c, 0x276e, 0x2770, 0x2772, 0x2774, 0x27c5, 0x27e6, 0x27e8, 0x27ea, 0x27ec, 0x27ee, 0x2983, 0x2985, 0x2987,
    0x2989, 0x298b, 0x298d, 0x298f, 0x2991, 0x2993, 0x2995, 0x2997, 0x29d8, 0x29da, 0x29fc, 0x2e22, 0x2e24, 0x2e26,
    0x2e28, 0x2e55, 0x2e57, 0x2e59, 0x2e5b, 0x2e62, 0x3008, 0x300a, 0x300c, 0x300e, 0x3010, 0x3014, 0x3016, 0x3018,
    0x301a, 0xfe59, 0xfe5b, 0xfe5d, 0xff08, 0xff3b, 0xff5b, 0xff5f, 0xff62,
];

/// The deepest embedding level (max_depth).
const DEEPEST: u8 = 125;
/// How many brackets may be open in a sequence: no more pairs are sought after that.
const OPEN: usize = 63;

/// A character's bidi class.
pub(crate) fn kind(letter: char) -> Kind {
    let n = KINDS.partition_point(|entry| entry >> 5 <= letter as u32);
    ALL[(KINDS[n - 1] & 31) as usize]
}

/// A character's mirror, if it has one: a bracket facing the other way.
pub(crate) fn mirrored(letter: char) -> Option<char> {
    let code = u16::try_from(letter as u32).ok()?;
    // (In a consecutive pair, the first has its range start's parity, and the other is next to
    // it.)
    let near = NEIGHBOURS.partition_point(|(_, last)| *last + 1 < code);
    let other = match NEIGHBOURS.get(near) {
        Some((first, _)) if *first <= code && (code - first) % 2 == 0 => code + 1,
        Some((first, _)) if *first < code && (code - first) % 2 == 1 => code - 1,
        _ => PAIRS.iter().find_map(|(one, other)| [(*one == code).then_some(*other), (*other == code).then_some(*one)].into_iter().flatten().next())?,
    };
    char::from_u32(other as u32)
}

/// Whether a character starts an isolate.
fn isolates(kind: Kind) -> bool {
    matches!(kind, Kind::LeftIsolate | Kind::RightIsolate | Kind::FirstIsolate)
}

/// Whether a character is removed by rule X9: boundary neutrals, and embeddings and overrides,
/// done once levels are known.
fn removed(kind: Kind) -> bool {
    matches!(kind, Kind::Unseen | Kind::LeftEmbedding | Kind::LeftOverride | Kind::RightEmbedding | Kind::RightOverride | Kind::PopEmbedding)
}

/// The direction of some characters' first strong letter, skipping isolates within:
/// right-to-left (`Some(true)`), left-to-right, or none. Inside an isolate (`inside`), only up
/// to its end.
fn first_strong(kinds: &[Kind], inside: bool) -> Option<bool> {
    let mut depth = 0usize;
    for kind in kinds {
        match kind {
            Kind::LeftIsolate | Kind::RightIsolate | Kind::FirstIsolate => depth += 1,
            Kind::PopIsolate if depth == 0 && inside => return None,
            Kind::PopIsolate => depth = depth.saturating_sub(1),
            Kind::Paragraph => return None,
            _ if depth > 0 => {}
            Kind::Left => return Some(false),
            Kind::Right | Kind::Arabic => return Some(true),
            _ => {}
        }
    }
    None
}

/// Whether a paragraph starting with `text` is right-to-left: whether its first strong letter
/// is. (No letter means left-to-right; so does everything without `right-to-left`.)
pub(crate) fn leftward(text: &str) -> bool {
    if !cfg!(feature = "right-to-left") {
        return false;
    }
    let mut depth = 0usize;
    for kind in text.chars().map(kind) {
        match kind {
            Kind::LeftIsolate | Kind::RightIsolate | Kind::FirstIsolate => depth += 1,
            Kind::PopIsolate => depth = depth.saturating_sub(1),
            Kind::Paragraph => return false,
            _ if depth > 0 => {}
            Kind::Left => return false,
            Kind::Right | Kind::Arabic => return true,
            _ => {}
        }
    }
    false
}

/// The next level above `level`: the next odd for right-to-left, the next even for
/// left-to-right.
fn above(level: u8, right: bool) -> u8 {
    if right { (level + 1) | 1 } else { (level + 2) & !1 }
}

/// One line's resolved order: each character's level. Kept between lines only to reuse memory.
#[derive(Default)]
pub(crate) struct Order {
    /// The line's characters,
    letters: Vec<char>,
    /// their original classes,
    written: Vec<Kind>,
    /// their classes as being resolved,
    kinds: Vec<Kind>,
    /// and their final levels.
    pub levels: Vec<u8>,
    /// For an isolate initiator, its matching PDI; for a PDI, its initiator. (`usize::MAX` for
    /// none.)
    pairs: Vec<usize>,
    /// The directional status stack, innermost last: each level with its override class, if
    /// any, and whether it is an isolate.
    stack: Vec<(u8, Option<Kind>, bool)>,
    /// Overflow isolate and embedding counts, and the valid isolate count.
    over_isolates: usize,
    over_embeddings: usize,
    open_isolates: usize,
    /// The level runs (first and last character), and one isolating run sequence being
    /// resolved: its characters, in order.
    runs: Vec<(usize, usize)>,
    sequence: Vec<usize>,
    /// Open brackets, each with its closing character and sequence position; and the pairs
    /// found.
    brackets: Vec<(char, usize)>,
    found: Vec<(usize, usize)>,
}

impl Order {
    /// Resolves every character's level in a line: of a right-to-left paragraph (`Some(true)`),
    /// left-to-right, or by its first strong letter. Returns the paragraph level, leaving the
    /// characters' in `levels`.
    pub fn of(&mut self, text: &str, leftward: Option<bool>) -> u8 {
        self.letters.clear();
        self.letters.extend(text.chars());
        self.written.clear();
        self.written.extend(self.letters.iter().map(|letter| kind(*letter)));
        self.resolve(leftward)
    }

    /// The same, for characters and classes already in place.
    fn resolve(&mut self, leftward: Option<bool>) -> u8 {
        let paragraph = leftward.or_else(|| first_strong(&self.written, false)).unwrap_or(false) as u8;
        self.kinds.clear();
        self.kinds.extend_from_slice(&self.written);
        self.levels.clear();
        self.levels.resize(self.kinds.len(), paragraph);
        self.match_isolates();
        self.explicit(paragraph);
        self.level_runs();
        for run in 0..self.runs.len() {
            if self.sequence_from(run) {
                self.settle(paragraph);
            }
        }
        self.raise();
        self.untouched(paragraph);
        paragraph
    }

    /// Matches each isolate initiator with its PDI, and back.
    fn match_isolates(&mut self) {
        let Order { written, pairs, sequence: open, .. } = self;
        pairs.clear();
        pairs.resize(written.len(), usize::MAX);
        open.clear();
        for (at, kind) in written.iter().enumerate() {
            match kind {
                Kind::LeftIsolate | Kind::RightIsolate | Kind::FirstIsolate => open.push(at),
                Kind::PopIsolate => open.pop().into_iter().for_each(|start| (pairs[start], pairs[at]) = (at, start)),
                Kind::Paragraph => open.clear(),
                _ => {}
            }
        }
    }

    /// The innermost entry's level and override class.
    fn top(&self) -> (u8, Option<Kind>, bool) {
        self.stack[self.stack.len() - 1]
    }

    /// Gives character `at` the innermost entry's level, and its override class if any.
    fn take_top(&mut self, at: usize) {
        let (level, overriding, _) = self.top();
        self.levels[at] = level;
        if let Some(kind) = overriding {
            self.kinds[at] = kind;
        }
    }

    /// Explicit levels from embeddings, overrides and isolates (rules X1 to X8).
    fn explicit(&mut self, paragraph: u8) {
        self.stack.clear();
        self.stack.push((paragraph, None, false));
        (self.over_isolates, self.over_embeddings, self.open_isolates) = (0, 0, 0);
        for at in 0..self.kinds.len() {
            match self.kinds[at] {
                kind @ (Kind::LeftEmbedding | Kind::LeftOverride | Kind::RightEmbedding | Kind::RightOverride) => self.embed(at, kind),
                kind @ (Kind::LeftIsolate | Kind::RightIsolate | Kind::FirstIsolate) => self.isolate(at, kind),
                Kind::PopIsolate => self.pop_isolate(at),
                Kind::PopEmbedding => self.pop_embedding(at),
                Kind::Paragraph => self.levels[at] = paragraph,
                Kind::Unseen => self.levels[at] = self.top().0,
                _ => self.take_top(at),
            }
        }
    }

    /// An embedding or override starts: a level above, if there is room.
    fn embed(&mut self, at: usize, kind: Kind) {
        let level = self.top().0;
        self.levels[at] = level;
        let next = above(level, matches!(kind, Kind::RightEmbedding | Kind::RightOverride));
        let overriding = match kind {
            Kind::LeftOverride => Some(Kind::Left),
            Kind::RightOverride => Some(Kind::Right),
            _ => None,
        };
        match next <= DEEPEST && self.over_isolates == 0 && self.over_embeddings == 0 {
            true => self.stack.push((next, overriding, false)),
            false => self.over_embeddings += (self.over_isolates == 0) as usize,
        }
    }

    /// An isolate starts: the initiator keeps the outer level, its content a level above. (FSI
    /// looks at its first strong letter first.)
    fn isolate(&mut self, at: usize, kind: Kind) {
        self.take_top(at);
        let right = match kind {
            Kind::FirstIsolate => first_strong(&self.kinds[at + 1..], true) == Some(true),
            kind => kind == Kind::RightIsolate,
        };
        let next = above(self.levels[at], right);
        if next <= DEEPEST && self.over_isolates == 0 && self.over_embeddings == 0 {
            self.open_isolates += 1;
            self.stack.push((next, None, true));
        } else {
            self.over_isolates += 1;
        }
    }

    /// An isolate ends, and with it everything opened inside.
    fn pop_isolate(&mut self, at: usize) {
        if self.over_isolates > 0 {
            self.over_isolates -= 1;
        } else if self.open_isolates > 0 {
            self.over_embeddings = 0;
            while self.stack.pop().is_some_and(|(.., isolate)| !isolate) {}
            self.open_isolates -= 1;
        }
        self.take_top(at);
    }

    /// An embedding or override ends, unless the last opened is an isolate.
    fn pop_embedding(&mut self, at: usize) {
        self.levels[at] = self.top().0;
        if self.over_isolates > 0 {
            return;
        }
        if self.over_embeddings > 0 {
            self.over_embeddings -= 1;
        } else if !self.top().2 && self.stack.len() >= 2 {
            self.stack.pop();
        }
    }

    /// Splits the line into level runs, skipping characters removed by X9.
    fn level_runs(&mut self) {
        let Order { written, levels, runs, .. } = self;
        runs.clear();
        for at in (0..written.len()).filter(|at| !removed(written[*at])) {
            match runs.last_mut() {
                Some(run) if levels[run.1] == levels[at] => run.1 = at,
                _ => runs.push((at, at)),
            }
        }
    }

    /// Gathers the isolating run sequence starting with run `run`: continuing after each run
    /// that ends with an isolate initiator. Says whether there is one: a run starting with a
    /// matched PDI belongs to its initiator's sequence.
    fn sequence_from(&mut self, run: usize) -> bool {
        let Order { written, pairs, runs, sequence, .. } = self;
        let (mut first, mut last) = runs[run];
        if written[first] == Kind::PopIsolate && pairs[first] != usize::MAX {
            return false;
        }
        sequence.clear();
        loop {
            sequence.extend((first..=last).filter(|at| !removed(written[*at])));
            let end = pairs[last];
            if !isolates(written[last]) || end == usize::MAX {
                return true;
            }
            (first, last) = runs[runs.partition_point(|(start, _)| *start < end)];
        }
    }

    /// Resolves one sequence's classes, and from them its levels (rules W1 to W7, N0 to N2, I1
    /// and I2).
    fn settle(&mut self, paragraph: u8) {
        let Order { written, kinds, levels, sequence, .. } = self;
        let (first, last) = (sequence[0], sequence[sequence.len() - 1]);
        let level = levels[first];
        // The sos and eos: the higher of the sequence's level and that of the adjacent
        // character, or the paragraph's.
        let before = (0..first).rev().find(|at| !removed(written[*at])).map_or(paragraph, |at| levels[at]);
        let after = (last + 1..written.len()).find(|at| !removed(written[*at])).filter(|_| !isolates(written[last])).map_or(paragraph, |at| levels[at]);
        let [start, end] = [before, after].map(|other| if level.max(other) % 2 == 1 { Kind::Right } else { Kind::Left });
        weak(kinds, sequence, start);
        self.brackets(level, start);
        let Order { kinds, sequence, .. } = self;
        neutral(kinds, sequence, level, start, end);
    }

    /// Raises each character whose direction differs from its level's (rules I1 and I2). Waits
    /// until every sequence is resolved: sequences compare against the explicit levels.
    fn raise(&mut self) {
        let Order { written, kinds, levels, .. } = self;
        for at in (0..written.len()).filter(|at| !removed(written[*at])) {
            levels[at] += match (levels[at] % 2, kinds[at]) {
                (0, Kind::Right) => 1,
                (0, Kind::Figure | Kind::ArabicFigure) => 2,
                (1, Kind::Left | Kind::Figure | Kind::ArabicFigure) => 1,
                _ => 0,
            };
        }
    }

    /// Paired brackets take the direction of their content (rule N0).
    fn brackets(&mut self, level: u8, start: Kind) {
        let Order { letters, written, kinds, sequence, brackets, found, .. } = self;
        brackets.clear();
        found.clear();
        // The pairs: each closing bracket with the nearest open one it matches. (A bracket no
        // longer ON does not count.)
        for (n, at) in sequence.iter().copied().enumerate().filter(|(_, at)| kinds[*at] == Kind::Other) {
            let letter = same_bracket(letters[at]);
            let opening = OPENING.binary_search(&(letter as u32 as u16)).is_ok();
            if opening && brackets.len() == OPEN {
                break;
            }
            if opening {
                brackets.push((same_bracket(mirrored(letter).unwrap_or(letter)), n));
                continue;
            }
            if let Some(open) = brackets.iter().rposition(|(closing, _)| *closing == letter) {
                found.push((brackets[open].1, n));
                brackets.truncate(open);
            }
        }
        found.sort_unstable();
        let own = if level % 2 == 1 { Kind::Right } else { Kind::Left };
        for (open, close) in found.iter().copied() {
            let inside: Vec<Kind> = sequence[open + 1..close].iter().filter_map(|at| direction(kinds[*at])).collect();
            let other = if own == Kind::Left { Kind::Right } else { Kind::Left };
            let before = sequence[..open].iter().rev().find_map(|at| direction(kinds[*at])).unwrap_or(start);
            let taken = match (inside.contains(&own), inside.contains(&other)) {
                (true, _) => own,
                (false, true) if before == other => other,
                (false, true) => own,
                (false, false) => continue,
            };
            for n in [open, close] {
                kinds[sequence[n]] = taken;
                // (Marks originally after the bracket go with it.)
                sequence[n + 1..].iter().take_while(|at| written[**at] == Kind::Mark).for_each(|at| kinds[*at] = taken);
            }
        }
    }

    /// Gives removed characters the level of the one before; and resets trailing whitespace and
    /// whitespace before a tab to the paragraph level, with boundary neutrals among it (rule
    /// L1).
    fn untouched(&mut self, paragraph: u8) {
        let Order { written, levels, .. } = self;
        for at in 0..written.len() {
            if removed(written[at]) {
                levels[at] = if at == 0 { paragraph } else { levels[at - 1] };
            }
        }
        let mut trailing = true;
        for at in (0..written.len()).rev() {
            let kind = written[at];
            if matches!(kind, Kind::Tab | Kind::Paragraph) {
                (levels[at], trailing) = (paragraph, true);
            } else if trailing && (matches!(kind, Kind::Space | Kind::PopIsolate) || isolates(kind) || removed(kind)) {
                levels[at] = paragraph;
            } else {
                trailing = false;
            }
        }
    }

    /// The level runs the line is shown in, left to right: each one's bytes and level.
    /// (Odd-level runs read right to left.) Rule L2.
    pub fn shown(&self, text: &str, stretches: &mut Vec<(Range<usize>, u8)>) {
        stretches.clear();
        for ((at, letter), level) in text.char_indices().zip(self.levels.iter().copied()) {
            match stretches.last_mut() {
                Some((bytes, was)) if *was == level => bytes.end = at + letter.len_utf8(),
                _ => stretches.push((at..at + letter.len_utf8(), level)),
            }
        }
        let highest = stretches.iter().map(|(_, level)| *level).max().unwrap_or(0);
        let lowest = stretches.iter().map(|(_, level)| *level | 1).min().unwrap_or(1);
        for level in (lowest..=highest).rev() {
            turn(stretches, |(_, own)| *own >= level);
        }
    }
}

/// Reverses each run of items `within` accepts.
pub(crate) fn turn<T>(things: &mut [T], within: impl Fn(&T) -> bool) {
    let mut at = 0;
    while at < things.len() {
        let end = at + things[at..].iter().position(|thing| !within(thing)).unwrap_or(things.len() - at);
        things[at..end].reverse();
        at = end + 1;
    }
}

/// A class's direction for brackets and neutrals: numbers count as right-to-left there.
fn direction(kind: Kind) -> Option<Kind> {
    match kind {
        Kind::Left => Some(Kind::Left),
        Kind::Right | Kind::Arabic | Kind::Figure | Kind::ArabicFigure => Some(Kind::Right),
        _ => None,
    }
}

/// A bracket as compared with others: Unicode's two angle bracket pairs are equivalent.
fn same_bracket(letter: char) -> char {
    match letter {
        '\u{2329}' => '\u{3008}',
        '\u{232a}' => '\u{3009}',
        letter => letter,
    }
}

/// Weak types resolve from their neighbours (rules W1 to W7): a mark takes the class before it,
/// numbers after Arabic letters become Arabic numbers, separators between numbers and
/// terminators next to them join the numbers, and numbers after left-to-right letters become
/// left-to-right.
fn weak(kinds: &mut [Kind], sequence: &[usize], start: Kind) {
    let mut before = start;
    for at in sequence.iter().copied() {
        match kinds[at] {
            Kind::Mark => kinds[at] = before,
            kind if isolates(kind) || kind == Kind::PopIsolate => before = Kind::Other,
            kind => before = kind,
        }
    }
    let mut letter = start;
    for at in sequence.iter().copied() {
        match kinds[at] {
            kind @ (Kind::Left | Kind::Right | Kind::Arabic) => letter = kind,
            Kind::Figure if letter == Kind::Arabic => kinds[at] = Kind::ArabicFigure,
            _ => {}
        }
    }
    for at in sequence.iter().copied() {
        if kinds[at] == Kind::Arabic {
            kinds[at] = Kind::Right;
        }
    }
    for n in 1..sequence.len().saturating_sub(1) {
        let (left, here, right) = (kinds[sequence[n - 1]], kinds[sequence[n]], kinds[sequence[n + 1]]);
        kinds[sequence[n]] = match (left, here, right) {
            (Kind::Figure, Kind::Sign | Kind::Between, Kind::Figure) => Kind::Figure,
            (Kind::ArabicFigure, Kind::Between, Kind::ArabicFigure) => Kind::ArabicFigure,
            _ => here,
        };
    }
    units(kinds, sequence);
    for at in sequence.iter().copied() {
        if matches!(kinds[at], Kind::Sign | Kind::Unit | Kind::Between) {
            kinds[at] = Kind::Other;
        }
    }
    let mut letter = start;
    for at in sequence.iter().copied() {
        match kinds[at] {
            kind @ (Kind::Left | Kind::Right) => letter = kind,
            Kind::Figure if letter == Kind::Left => kinds[at] = Kind::Left,
            _ => {}
        }
    }
}

/// Terminators next to numbers join them (rule W5).
fn units(kinds: &mut [Kind], sequence: &[usize]) {
    let mut n = 0;
    while n < sequence.len() {
        if kinds[sequence[n]] != Kind::Unit {
            n += 1;
            continue;
        }
        let end = n + sequence[n..].iter().position(|at| kinds[*at] != Kind::Unit).unwrap_or(sequence.len() - n);
        let figures = (n > 0 && kinds[sequence[n - 1]] == Kind::Figure) || sequence.get(end).is_some_and(|at| kinds[*at] == Kind::Figure);
        if figures {
            sequence[n..end].iter().for_each(|at| kinds[*at] = Kind::Figure);
        }
        n = end;
    }
}

/// Neutrals take the direction of the strong text around them where both sides agree; else
/// their level's (rules N1 and N2).
fn neutral(kinds: &mut [Kind], sequence: &[usize], level: u8, start: Kind, end: Kind) {
    let own = if level % 2 == 1 { Kind::Right } else { Kind::Left };
    let mut n = 0;
    while n < sequence.len() {
        if direction(kinds[sequence[n]]).is_some() {
            n += 1;
            continue;
        }
        let stop = n + sequence[n..].iter().position(|at| direction(kinds[*at]).is_some()).unwrap_or(sequence.len() - n);
        let before = if n == 0 { start } else { direction(kinds[sequence[n - 1]]).unwrap_or(start) };
        let after = sequence.get(stop).and_then(|at| direction(kinds[*at])).unwrap_or(end);
        let taken = if before == after { before } else { own };
        sequence[n..stop].iter().for_each(|at| kinds[*at] = taken);
        n = stop;
    }
}

// ---- text, written in the order it is shown in ----

/// Whether a text has nothing right-to-left, judged by its bytes alone: no Hebrew, Arabic or
/// similar letter, and no character opening a right-to-left run. Such text all goes to the
/// inner text system, and nearly all text is such: so this is all it costs. (A few nearby
/// characters, Armenian and Latin ligatures, are checked more closely; they go there too.) A
/// UTF-8 lead byte roughly locates a character: those scripts start at lead bytes 0xd6 to 0xdf,
/// their presentation forms at 0xef; the directional controls at 0xe2. (Without
/// `right-to-left`, all text is plain.) With `brahmic`, text is also not plain if it has a
/// letter of a syllabic script: Indic and Southeast Asian ones start with 0xe0 (from 0xa4),
/// 0xe1, 0xea, 0xf0 0x91 and (Tai Yo) 0xf0 0x9e 0x9b; Mongolian with 0xe1 0xa0 to 0xa2;
/// Tifinagh with 0xe2 0xb4 or 0xb5; the rest of the left-to-right ones in the higher planes
/// with 0xf0 and a few more bytes.
pub(crate) fn plain(text: &str) -> bool {
    let (both_ways, syllables) = (cfg!(feature = "right-to-left"), cfg!(feature = "brahmic"));
    if !both_ways && !syllables {
        return true;
    }
    let bytes = text.as_bytes();
    // Eight bytes at a time first: in most text none reaches 0xd6. (A byte does if its top bit
    // is set and its rest is at least 0x56: adding 0x2a to the rest carries into the top bit
    // exactly then.)
    let (eights, rest) = bytes.as_chunks::<8>();
    let high = |eight: &[u8; 8]| {
        let eight = u64::from_ne_bytes(*eight);
        ((eight & 0x7f7f_7f7f_7f7f_7f7f) + 0x2a2a_2a2a_2a2a_2a2a) & eight & 0x8080_8080_8080_8080 != 0
    };
    if !eights.iter().any(high) && rest.iter().all(|byte| *byte < 0xd6) {
        return true;
    }
    let next = |at: usize| bytes.get(at).copied().unwrap_or(0);
    let leftward = |at: usize| match bytes[at] {
        0xd6..=0xdf => true,
        0xe0 => (0xa0..=0xa3).contains(&next(at + 1)),
        0xe2 => matches!((next(at + 1), next(at + 2)), (0x80, 0x8f | 0xab | 0xae) | (0x81, 0xa7 | 0xa8)),
        0xef => matches!(next(at + 1), 0xac..=0xb7 | 0xb9..=0xbb),
        0xf0 => matches!((next(at + 1), next(at + 2)), (0x90 | 0x9e, 0xa0..=0xbf)),
        _ => false,
    };
    let syllabic = |at: usize| {
        let (first, second, third) = (bytes[at], next(at + 1), next(at + 2));
        let below = matches!((first, second), (0xe0, 0xa4..=0xbf) | (0xe1, 0x80..=0x82 | 0x9c..=0xa2 | 0xa4..=0xb1 | 0xb3) | (0xe2, 0xb4 | 0xb5) | (0xea, 0xa0..=0xab | 0xaf));
        let above = matches!((second, third), (0x91 | 0x93 | 0x96, _) | (0x90, 0x95..=0x97) | (0x92, 0xbe | 0xbf) | (0x94, 0x80..=0x8f) | (0x98, 0xac..=0xb3) | (0x9b, 0xb0..=0xb2));
        let after = matches!((second, third), (0x9e, 0x84 | 0x85 | 0x8a | 0x8b | 0x93 | 0x97 | 0x9b));
        below || (first == 0xf0 && (above || after))
    };
    !(0..bytes.len()).any(|at| (both_ways && leftward(at)) || (syllables && syllabic(at)))
}

/// Roughly how many bytes of shaped text are cached. Past it, all are dropped and shaped again
/// as needed.
const CACHE_LIMIT: usize = 4 << 20;
/// A run of more pixels than this is not rasterized: no line is that long.
const LARGEST: usize = 1 << 22;

/// How one piece of a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Way {
    /// By the inner text system, as is: left-to-right text.
    Ahead,
    /// By the inner text system, reversed (each character with its marks) and brackets
    /// mirrored: right-to-left text that is no letter of a right-to-left script (emoji, other
    /// scripts' punctuation), or text no font here is found for. Its text is at these bytes of
    /// `turned`.
    Turned(usize, usize),
    /// Shaped and drawn here: the run cached as `shaped[n]`.
    Shaped(usize),
}

/// A piece of a line: its bytes, how it is drawn, and its width in whole pixels.
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    bytes: Range<usize>,
    way: Way,
    wide: f32,
}

/// A shaped and rasterized run: its width, each character's position (from its left edge, in
/// pixels), and once drawn, its coverage bitmap with its offset from the pen on the line.
struct Shaped {
    wide: f32,
    edges: Vec<[f32; 2]>,
    drawn: Option<([i32; 2], usize, Vec<u8>)>,
}

/// A text system wrapping another, which draws all left-to-right text, that shows text in
/// display order. A line without right-to-left letters (judged by its bytes) passes straight
/// through. Any other is reordered by the bidi algorithm (see `Order`) and split into pieces:
/// left-to-right text still goes to the inner system; Hebrew and Arabic letters, and what lies
/// between them, are shaped (`shaping.rs`) and drawn here, from the inner system's font for
/// them (an app font) or else the system's. A [`Ui`](crate::Ui) wraps its text system in one of
/// these itself.
pub struct BidiText {
    text: Box<dyn TextSystem>,
    order: Order,
    /// The level runs of the line being drawn, in display order; and its pieces, left to right.
    stretches: Vec<(Range<usize>, u8)>,
    pieces: Vec<Piece>,
    /// Reversed text for the inner system.
    turned: String,
    /// The system's fonts for these scripts, by face and script, as looked up (none where the
    /// system has none).
    fonts: Vec<(Face, Script, Option<FontFile>)>,
    /// The text language, and each font's plan per script in it, by font index.
    language: Language,
    plans: Vec<(u32, Script, Plan)>,
    /// The runs shaped so far, by font, pixel size and text, in that order; and roughly how
    /// many bytes they take.
    shaped: Vec<(u32, u32, Box<str>, Shaped)>,
    cache_bytes: usize,
    /// Kept only to reuse memory: glyphs, and character ends.
    glyphs: Vec<Laid>,
    stops: Vec<f32>,
    /// The language's forms in the system's fonts, for lines its text leaves without them (see
    /// `system_forms`).
    #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
    forms: crate::system_forms::Forms,
}

/// Whether lines the system's text leaves without their language's forms are shaped here: with
/// `shaping`, where the system's fonts are files read here (not on Android, whose text takes the
/// language itself, nor on the web).
const SHAPES_FORMS: bool = cfg!(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))));

/// The text system for all text: `text` with right-to-left text reordered around it, Indic and
/// Southeast Asian scripts shaped, and lines the system's text leaves without their language's
/// forms shaped with them ([`BidiText`]); or `text` alone without the features `right-to-left`,
/// `brahmic` and `shaping`.
pub(crate) fn ordered(text: Box<dyn TextSystem>) -> Box<dyn TextSystem> {
    match cfg!(feature = "right-to-left") || cfg!(feature = "brahmic") || SHAPES_FORMS {
        true => Box::new(BidiText::around(text)),
        false => text,
    }
}

impl std::fmt::Debug for BidiText {
    /// Its shaping language, and how many runs it caches.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BidiText").field("language", &self.language).field("shaped", &self.shaped.len()).finish_non_exhaustive()
    }
}

impl BidiText {
    /// Display-order text, drawn by `text` where left-to-right.
    pub fn around(text: Box<dyn TextSystem>) -> Self {
        BidiText {
            text,
            order: Order::default(),
            stretches: Vec::new(),
            pieces: Vec::new(),
            turned: String::new(),
            fonts: Vec::new(),
            language: Language::of_the_system(),
            plans: Vec::new(),
            shaped: Vec::new(),
            cache_bytes: 0,
            glyphs: Vec::new(),
            stops: Vec::new(),
            #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
            forms: crate::system_forms::Forms::new(),
        }
    }

    /// Whether a line goes to the inner system as it is: it has no right-to-left letters, none
    /// of scripts shaped here by syllable, and no forms of its language the inner system leaves
    /// out.
    fn passes(&mut self, text: &str, face: Face) -> bool {
        plain(text) && !self.forms_here(face, text)
    }

    /// Whether a line is shaped here for its language's forms (see `system_forms`): only where
    /// the inner system draws the system's fonts.
    #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
    fn forms_here(&mut self, face: Face, text: &str) -> bool {
        self.text.file(face, 'a').is_none() && self.forms.shaped_here(face, text)
    }
    /// None without `shaping`.
    #[cfg(not(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm")))))]
    fn forms_here(&mut self, _face: Face, _text: &str) -> bool {
        false
    }

    /// Splits a line into pieces in display order, each with its width. Returns the line's
    /// width in pixels. With `edges`, also each character's position (from the line's left, in
    /// pixels): what a cursor needs.
    fn lay(&mut self, text: &str, font: Font, scale: f32, leftward: Option<bool>, mut edges: Option<&mut Vec<[f32; 2]>>) -> f32 {
        if self.cache_bytes > CACHE_LIMIT {
            (self.shaped, self.cache_bytes) = (Vec::new(), 0);
        }
        // (Without `right-to-left` a line shows as typed: one run.)
        if cfg!(feature = "right-to-left") {
            self.order.of(text, leftward);
            self.order.shown(text, &mut self.stretches);
        } else {
            self.stretches = vec![(0..text.len(), 0)];
        }
        (self.pieces, self.turned) = (Vec::new(), String::new());
        for stretch in 0..self.stretches.len() {
            let (bytes, level) = self.stretches[stretch].clone();
            match level % 2 {
                1 if cfg!(feature = "right-to-left") => self.turned_stretch(text, bytes, font, scale),
                _ => self.ahead_stretch(text, bytes, font, scale),
            }
        }
        if let Some(edges) = edges.as_deref_mut() {
            edges.clear();
            edges.resize(text.chars().count(), [0.0; 2]);
        }
        let mut pen = 0.0;
        for n in 0..self.pieces.len() {
            let wide = self.piece(text, n, font, scale, pen, edges.as_deref_mut());
            self.pieces[n].wide = wide;
            pen += wide;
        }
        pen
    }

    /// Splits a left-to-right run into pieces, in order: runs of one Indic or Southeast Asian
    /// script (with signs and spaces between), each shaped for its font; and the rest, the
    /// inner system's as is. (Without `brahmic`, all of it is the inner system's.) In a line the
    /// inner system would leave without its language's forms, runs of scripts without shaping
    /// of their own are shaped here too, each for its font.
    fn ahead_stretch(&mut self, text: &str, bytes: Range<usize>, font: Font, scale: f32) {
        let stretch = &text[bytes.clone()];
        let forms = self.forms_here(font.face, stretch);
        // (Letters of right-to-left scripts stay the inner system's: here only without
        // `right-to-left`, which leaves them unreversed and unjoined.)
        let shaped = |letter: char| match Script::of(letter) {
            Some(Script::Brahmic(_)) => true,
            Some(Script::Plain(_)) => forms,
            _ => false,
        };
        if !stretch.chars().any(shaped) {
            return self.pieces.push(Piece { bytes, way: Way::Ahead, wide: 0.0 });
        }
        let mut at = 0;
        while at < stretch.len() {
            let next = stretch[at..].char_indices().find(|(_, letter)| shaped(*letter)).map_or(stretch.len(), |(n, _)| at + n);
            let (long, way) = if next > at { (next - at, None) } else { self.run(stretch, at, font, scale) };
            self.pieces.push(Piece { bytes: bytes.start + at..bytes.start + at + long, way: way.unwrap_or(Way::Ahead), wide: 0.0 });
            at += long;
        }
    }

    /// Splits a right-to-left run into pieces and adds them in display order: runs of one
    /// script's letters (with what lies between), each for its font; and what no such font has,
    /// reversed for the inner system.
    fn turned_stretch(&mut self, text: &str, bytes: Range<usize>, font: Font, scale: f32) {
        let first = self.pieces.len();
        let stretch = &text[bytes.clone()];
        // (A run with no letter of those scripts is only neutrals: the inner system's,
        // reversed.)
        let letter = stretch.chars().find(|letter| matches!(kind(*letter), Kind::Right | Kind::Arabic));
        let mut at = 0;
        while at < stretch.len() {
            let (long, way) = match letter {
                Some(_) => self.run(stretch, at, font, scale),
                None => (stretch.len(), None),
            };
            let piece = bytes.start + at..bytes.start + at + long;
            let way = way.unwrap_or_else(|| self.turn(&text[piece.clone()]));
            self.pieces.push(Piece { bytes: piece, way, wide: 0.0 });
            at += long;
        }
        self.pieces[first..].reverse();
    }

    /// The run starting at byte `at` of a level run: its length, and how it is shaped here, if
    /// it is. A run is one script's letters with what lies between, as far as one font has all
    /// of it; or what no font here has, as far as that goes.
    fn run(&mut self, stretch: &str, at: usize, font: Font, scale: f32) -> (usize, Option<Way>) {
        let rest = &stretch[at..];
        let script = rest.chars().find_map(Script::of).or_else(|| stretch[..at].chars().rev().find_map(Script::of)).unwrap_or(Script::None);
        let named = rest.chars().find(|letter| Script::of(*letter) == Some(script)).unwrap_or(' ');
        let Some(file) = self.file(font.face, script, named) else { return (rest.len(), None) };
        // As far as the font has every character (a mark or ignorable goes with the character
        // before), and no letter of another script comes. (An Indic or Southeast Asian run ends
        // at a letter of an unshaped script, a Latin one say, which the inner system draws;
        // Vedic signs belong to it.)
        let syllabic = cfg!(feature = "brahmic") && matches!(script, Script::Brahmic(_));
        let foreign = |letter: char| syllabic && letter.is_alphabetic() && Script::of(letter).is_none() && !('\u{1cd0}'..='\u{1cff}').contains(&letter);
        let mut long = 0;
        while long < rest.len() {
            let size = crate::text::cluster(&rest[long..]);
            let first = rest[long..].chars().next().unwrap_or(' ');
            let other = Script::of(first).is_some_and(|own| own != script) || foreign(first);
            if other || (file.glyph(first) == 0 && !first.is_whitespace()) {
                break;
            }
            long += size;
        }
        if long == 0 {
            let size = crate::text::cluster(rest);
            return (size, None);
        }
        let shaped = self.shape(font.face, script, named, &rest[..long], font.size * scale);
        (long, shaped.map(Way::Shaped))
    }

    /// The font for a character of a script in a face: the inner system's own if it has one (an
    /// app font); else the system's font for that script.
    fn file(&mut self, face: Face, script: Script, letter: char) -> Option<&FontFile> {
        if self.text.file(face, letter).is_some() {
            return self.text.file(face, letter);
        }
        if !self.fonts.iter().any(|(known, of, _)| *known == face && *of == script) {
            // (Letters of scripts without shaping of their own, shaped only for their forms, in
            // the font the system's text draws them in. Others come here only with
            // `right-to-left` or `brahmic`.)
            let file = match script {
                Script::Plain(_) if SHAPES_FORMS => crate::system_fonts::file_for(face, letter).filter(|file| file.glyph(letter) != 0),
                _ if cfg!(feature = "right-to-left") || cfg!(feature = "brahmic") => system(face, letter),
                _ => None,
            };
            self.fonts.push((face, script, file));
        }
        self.fonts.iter().find(|(known, of, _)| *known == face && *of == script).and_then(|(.., file)| file.as_ref())
    }

    /// Shapes a run in a font at a size and caches it: returns its cache index.
    fn shape(&mut self, face: Face, script: Script, letter: char, run: &str, pixels: f32) -> Option<usize> {
        let file = self.file(face, script, letter)?;
        let serial = file.serial();
        let key = (serial, pixels.to_bits());
        let found = self.shaped.binary_search_by(|(file, size, text, _)| (*file, *size, &**text).cmp(&(key.0, key.1, run)));
        let at = match found {
            Ok(at) => return Some(at),
            Err(at) => at,
        };
        if !(pixels > 0.0 && pixels < 4096.0) {
            return None;
        }
        // (The file is looked up again: its borrow ends where the plan is made. Scripts
        // `brahmic.rs` shapes go their own way, left to right as are scripts shaped only for
        // their forms; the rest here are right-to-left.)
        let (leftward, language) = (!matches!(script, Script::Brahmic(_) | Script::Plain(_)), self.language);
        if !self.plans.iter().any(|(of, known, _)| *of == serial && *known == script) {
            let plan = Plan::of(self.file(face, script, letter)?, script, leftward, language);
            self.plans.push((serial, script, plan));
        }
        let BidiText { text, fonts, plans, glyphs, .. } = self;
        let plan = &plans.iter().find(|(of, known, _)| *of == serial && *known == script)?.2;
        let file = match text.file(face, letter) {
            Some(file) => file,
            None => fonts.iter().find(|(known, of, _)| *known == face && *of == script)?.2.as_ref()?,
        };
        let total = file.shape(plan, run, glyphs);
        let unit = file.unit(pixels);
        let edges = if leftward { edges_of(glyphs, run, unit) } else { syllable_edges(glyphs, run, unit) };
        let shaped = Shaped { wide: (total as f32 * unit).round(), edges, drawn: None };
        self.cache_bytes += run.len() + shaped.edges.len() * 8 + size_of::<(u32, u32, Box<str>, Shaped)>();
        self.shaped.insert(at, (serial, pixels.to_bits(), run.into(), shaped));
        // (Runs of this line cached after it are one further on now.)
        for piece in &mut self.pieces {
            piece.way = match piece.way {
                Way::Shaped(n) if n >= at => Way::Shaped(n + 1),
                way => way,
            };
        }
        Some(at)
    }

    /// Reverses a piece for the inner system: characters in reverse, each with its marks,
    /// brackets mirrored.
    fn turn(&mut self, piece: &str) -> Way {
        let start = self.turned.len();
        let mut end = piece.len();
        while end > 0 {
            let from = crate::text::cluster_before(piece, end);
            let mut letters = piece[from..end].chars();
            let first = letters.next().unwrap_or(' ');
            self.turned.push(mirrored(first).unwrap_or(first));
            self.turned.extend(letters);
            end = from;
        }
        Way::Turned(start, self.turned.len())
    }

    /// Piece `n`'s width in whole pixels; and each character's position from the line's left
    /// (`pen` pixels in), if asked.
    fn piece(&mut self, text: &str, n: usize, font: Font, scale: f32, pen: f32, edges: Option<&mut Vec<[f32; 2]>>) -> f32 {
        let Piece { bytes, way, .. } = self.pieces[n].clone();
        let first = text[..bytes.start].chars().count();
        if let Way::Shaped(at) = way {
            let shaped = &self.shaped[at].3;
            let placed = shaped.edges.iter().enumerate();
            if let Some(edges) = edges {
                placed.for_each(|(m, [left, right])| edges[first + m] = [pen + left, pen + right]);
            }
            return shaped.wide;
        }
        let piece = match way {
            Way::Turned(start, end) => &self.turned[start..end],
            _ => &text[bytes.clone()],
        };
        self.stops.clear();
        let wide = (self.text.measure(piece, font, scale, Some(&mut self.stops)) * scale).round();
        let Some(edges) = edges else { return wide };
        // Where each character ends, by the inner system's count; reversed, the piece's last
        // character shows first.
        let count = self.stops.len();
        for m in 0..count {
            let [left, right] = [if m == 0 { 0.0 } else { self.stops[m - 1] * scale }, self.stops[m] * scale];
            let own = if let Way::Turned(..) = way { turned_back(&text[bytes.clone()], m) } else { m };
            edges[first + own] = [pen + left, pen + right];
        }
        wide
    }

    /// Draws a line, its pieces left to right, from (`x`, `y`).
    fn write(&mut self, canvas: &mut Canvas, text: &str, [x, y]: [f32; 2], font: Font, color: Color, leftward: Option<bool>) {
        let scale = canvas.scale();
        self.lay(text, font, scale, leftward, None);
        let ascent = self.text.metrics(font, scale).ascent;
        let (left, line) = ((x * scale).round(), ((y * scale).round() + (ascent * scale).round()) as i32);
        let mut pen = 0.0;
        for n in 0..self.pieces.len() {
            let Piece { bytes, way, wide } = self.pieces[n].clone();
            match way {
                Way::Ahead => self.text.draw(canvas, &text[bytes], (left + pen) / scale, y, font, color),
                Way::Turned(start, end) => self.text.draw(canvas, &self.turned[start..end], (left + pen) / scale, y, font, color),
                Way::Shaped(at) => self.lay_run(canvas, text, bytes, at, font, [(left + pen) as i32, line], color),
            }
            pen += wide;
        }
    }

    /// Draws a shaped run with its pen at `x` on line `line`, in pixels, rasterizing it first
    /// if never done: from the font it was shaped with.
    #[allow(clippy::too_many_arguments)]
    fn lay_run(&mut self, canvas: &mut Canvas, text: &str, bytes: Range<usize>, at: usize, font: Font, [x, line]: [i32; 2], color: Color) {
        let pixels = font.size * canvas.scale();
        if self.shaped[at].3.drawn.is_none() {
            let run = &text[bytes];
            let script = run.chars().find_map(Script::of).unwrap_or(Script::None);
            let letter = run.chars().find(|letter| Script::of(*letter) == Some(script)).unwrap_or(' ');
            let Some(block) = self.picture(font.face, script, letter, run, pixels) else { return };
            self.cache_bytes += block.2.len();
            self.shaped[at].3.drawn = Some(block);
        }
        let Some(([left, top], width, cover)) = &self.shaped[at].3.drawn else { return };
        if *width > 0 {
            canvas.cover(x + left, line + top, *width, cover, color);
        }
    }

    /// Rasterizes a run's glyphs into one coverage bitmap, overlaid, and its offset from the
    /// pen on the line.
    fn picture(&mut self, face: Face, script: Script, letter: char, run: &str, pixels: f32) -> Option<([i32; 2], usize, Vec<u8>)> {
        let serial = self.file(face, script, letter)?.serial();
        let BidiText { text, fonts, plans, glyphs, .. } = self;
        let plan = &plans.iter().find(|(of, known, _)| *of == serial && *known == script)?.2;
        let file = match text.file(face, letter) {
            Some(file) => file,
            None => fonts.iter().find(|(known, of, _)| *known == face && *of == script)?.2.as_ref()?,
        };
        file.shape(plan, run, glyphs);
        let unit = file.unit(pixels);
        let mut pen = 0.0;
        let mut parts = Vec::with_capacity(glyphs.len());
        for laid in glyphs.iter() {
            parts.push(file.cover(laid.glyph, unit, [(pen + laid.offset[0] as f32) * unit, laid.offset[1] as f32 * unit]));
            pen += laid.advance as f32;
        }
        Some(covered_all(&parts))
    }
}

/// Each character's position in a shaped run, from its left edge in pixels (its glyphs in
/// display order, each from pen to advance). A character without a glyph of its own (a ligature
/// component) shares the previous one's, split evenly by grapheme cluster from the right: the
/// run is right to left. Marks belong to their letters.
fn edges_of(glyphs: &[Laid], run: &str, unit: f32) -> Vec<[f32; 2]> {
    let count = run.chars().count();
    let mut edges: Vec<Option<[f32; 2]>> = vec![None; count];
    let mut pen = 0.0;
    for laid in glyphs {
        let (left, right) = (pen * unit, (pen + laid.advance as f32) * unit);
        pen += laid.advance as f32;
        let Some(edge) = edges.get_mut(laid.from as usize) else { continue };
        if right > left {
            *edge = Some(edge.map_or([left, right], |[l, r]| [l.min(left), r.max(right)]));
        }
    }
    // The grapheme clusters, each with its first `char`.
    let starts: Vec<usize> = std::iter::successors(Some(0usize), |at| Some(at + crate::text::cluster(&run[*at..])).filter(|next| *next < run.len())).collect();
    let firsts: Vec<usize> = starts.iter().map(|at| run[..*at].chars().count()).collect();
    let mut own = vec![[0.0f32; 2]; count];
    let mut n = 0;
    while n < firsts.len() {
        // The characters from this one to the next with its own glyph share it.
        let mut end = n + 1;
        while end < firsts.len() && (firsts[end]..firsts.get(end + 1).copied().unwrap_or(count)).all(|m| edges[m].is_none()) {
            end += 1;
        }
        let edge = (firsts[n]..firsts.get(end).copied().unwrap_or(count)).find_map(|m| edges[m]).unwrap_or([0.0; 2]);
        let share = (edge[1] - edge[0]) / (end - n) as f32;
        for (k, m) in (n..end).enumerate() {
            let right = edge[1] - share * k as f32;
            (firsts[m]..firsts.get(m + 1).copied().unwrap_or(count)).for_each(|c| own[c] = [right - share, right]);
        }
        n = end;
    }
    own
}

/// Each character's position in a left-to-right syllabic run, from its left edge in pixels. A
/// grapheme cluster ([`text::cluster`](crate::text::cluster)) takes its glyphs' width. Where
/// several clusters' glyphs are interleaved (a vowel sign before two consonants, a ligature of
/// two), they share all their glyphs' width evenly, in logical order; one without a glyph of
/// its own (a ligature component) shares the previous one's.
fn syllable_edges(glyphs: &[Laid], run: &str, unit: f32) -> Vec<[f32; 2]> {
    let count = run.chars().count();
    // Which cluster each `char` belongs to.
    let mut of = vec![0usize; count];
    let (mut at, mut n, mut clusters) = (0, 0, 0);
    while at < run.len() {
        let next = at + crate::text::cluster(&run[at..]).max(1);
        let chars = run[at..next].chars().count();
        of[n..(n + chars).min(count)].fill(clusters);
        (at, n, clusters) = (next, n + chars, clusters + 1);
    }
    // Which clusters interleave with the one before: those a syllable's glyphs span, and those
    // without glyphs.
    let (mut mixed, mut drawn) = (vec![false; clusters], vec![false; clusters]);
    let mut syllables: Vec<(u16, usize, usize)> = Vec::new();
    for laid in glyphs {
        let cluster = of.get(laid.from as usize).copied().unwrap_or(0);
        drawn[cluster] = true;
        match syllables.iter_mut().find(|(whole, ..)| *whole == laid.whole && laid.whole != 0) {
            Some(found) => (found.1, found.2) = (found.1.min(cluster), found.2.max(cluster)),
            None => syllables.push((laid.whole, cluster, cluster)),
        }
    }
    syllables.iter().filter(|(whole, ..)| *whole != 0).for_each(|(_, first, last)| mixed[first + 1..=*last].fill(true));
    (1..clusters).for_each(|cluster| mixed[cluster] |= !drawn[cluster]);
    // Each group's width: from its first glyph's left to its last glyph's right.
    let start: Vec<usize> = (0..clusters).scan(0, |start, cluster| Some(*(if mixed[cluster] { start } else { (*start = cluster, start).1 }))).collect();
    let mut room = vec![[f32::MAX, f32::MIN]; clusters];
    let mut pen = 0.0;
    for laid in glyphs {
        let (left, right) = (pen * unit, (pen + laid.advance as f32) * unit);
        pen += laid.advance as f32;
        let group = start[of.get(laid.from as usize).copied().unwrap_or(0)];
        room[group] = [room[group][0].min(left), room[group][1].max(right)];
    }
    // Shared among each group's clusters.
    let mut own = vec![[0.0f32; 2]; clusters];
    let (mut cluster, mut last) = (0, 0.0f32);
    while cluster < clusters {
        let end = cluster + 1 + mixed[cluster + 1..].iter().take_while(|mixed| **mixed).count();
        let [left, right] = if room[cluster][0] <= room[cluster][1] { room[cluster] } else { [last, last] };
        let share = (right - left) / (end - cluster) as f32;
        (cluster..end).enumerate().for_each(|(k, each)| own[each] = [left + share * k as f32, left + share * (k + 1) as f32]);
        (cluster, last) = (end, right);
    }
    of.iter().map(|cluster| own[*cluster]).collect()
}

/// Which character of a piece the `m`th character of its reversal is: characters, each with its
/// marks, in reverse order.
fn turned_back(piece: &str, m: usize) -> usize {
    let mut end = piece.len();
    let mut seen = 0;
    while end > 0 {
        let from = crate::text::cluster_before(piece, end);
        let size = piece[from..end].chars().count();
        if m < seen + size {
            return piece[..from].chars().count() + (m - seen);
        }
        seen += size;
        end = from;
    }
    m
}

/// Merges coverage bitmaps: each pixel covered as much as all of them together (what one
/// leaves, the next may cover).
fn covered_all(parts: &[([i32; 2], usize, Vec<u8>)]) -> ([i32; 2], usize, Vec<u8>) {
    let reach = |(at, width, cover): &([i32; 2], usize, Vec<u8>)| [at[0], at[1], at[0] + *width as i32, at[1] + (cover.len() / (*width).max(1)) as i32];
    let around = parts.iter().filter(|(_, width, _)| *width > 0).map(reach).reduce(|[l, t, r, b], [l2, t2, r2, b2]| [l.min(l2), t.min(t2), r.max(r2), b.max(b2)]);
    let Some([left, top, right, bottom]) = around else { return ([0; 2], 0, Vec::new()) };
    let (width, height) = ((right - left) as usize, (bottom - top) as usize);
    if width * height > LARGEST {
        return ([0; 2], 0, Vec::new());
    }
    let mut block = vec![0u8; width * height];
    for ([x, y], wide, cover) in parts.iter().filter(|(_, width, _)| *width > 0) {
        let (dx, dy) = ((x - left) as usize, (y - top) as usize);
        for (row, line) in cover.chunks(*wide).enumerate() {
            let into = &mut block[(dy + row) * width + dx..][..*wide];
            into.iter_mut().zip(line).for_each(|(into, own)| *into = (*into as u32 + *own as u32 - (*into as u32 * *own as u32 + 127) / 255) as u8);
        }
    }
    ([left, top], width, block)
}

/// The system font for a character in a face: on Windows, the interface font files (Segoe UI,
/// then Tahoma), the first with the character. With `brahmic`, then the fonts Windows links to
/// Segoe UI for Indic and Southeast Asian scripts: Nirmala UI (a collection, bold second),
/// Leelawadee UI, Myanmar Text, Microsoft Himalaya and Javanese Text.
#[cfg(windows)]
pub(crate) fn system(face: Face, letter: char) -> Option<FontFile> {
    let folder = std::path::Path::new(&std::env::var_os("WINDIR")?).join("Fonts");
    let names: &[&str] = match face {
        Face::Bold => &["segoeuib.ttf", "tahomabd.ttf", "segoeui.ttf"],
        Face::Mono => &["CascadiaMono.ttf", "cour.ttf", "segoeui.ttf"],
        Face::Regular => &["segoeui.ttf", "tahoma.ttf"],
        Face::Italic => &["segoeuii.ttf", "segoeui.ttf"],
        // (An app-named font has no known file here: the regular one stands in.)
        Face::Named(_) => &["segoeui.ttf", "tahoma.ttf"],
    };
    let found = names.iter().filter_map(|name| FontFile::open(folder.join(name))).find(|font| font.glyph(letter) != 0);
    if found.is_some() || Script::of(letter).is_none() {
        return found;
    }
    // (Windows' fonts for scripts those lack: Indic and Southeast Asian, Thai and neighbours,
    // Burmese, Tibetan, Javanese; N'Ko and Adlam; Mongolian; Phags-pa; Thaana; and historic
    // scripts.)
    let bold = face == Face::Bold;
    let syllabic = [
        ("Nirmala.ttc", bold as usize),
        ("LeelawUI.ttf", 0),
        ("mmrtext.ttf", 0),
        ("himalaya.ttf", 0),
        ("javatext.ttf", 0),
        ("ebrima.ttf", 0),
        ("monbaiti.ttf", 0),
        ("phagspa.ttf", 0),
        ("mvboli.ttf", 0),
        ("seguihis.ttf", 0),
    ];
    let bolder = |(name, nth): (&'static str, usize)| match (bold, name) {
        (true, "LeelawUI.ttf") => ("LeelaUIb.ttf", nth),
        (true, "mmrtext.ttf") => ("mmrtextb.ttf", nth),
        (true, "ebrima.ttf") => ("ebrimabd.ttf", nth),
        (true, "phagspa.ttf") => ("phagspab.ttf", nth),
        _ => (name, nth),
    };
    let opened = |(name, nth): (&str, usize)| FontFile::open(folder.join(name)).and_then(|font| if nth == 0 { Some(font) } else { font.nth(nth) });
    syllabic.into_iter().map(bolder).filter_map(opened).find(|font| font.glyph(letter) != 0)
}

/// The system font for a character in a face: elsewhere, the file Fontconfig picks, the
/// desktop's choice.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) fn system(face: Face, letter: char) -> Option<FontFile> {
    crate::x11::file_for(face, letter).and_then(FontFile::open).filter(|font| font.glyph(letter) != 0)
}

/// The same on Android: the first of the system's fonts that has it, in `fonts.xml`'s order.
#[cfg(target_os = "android")]
pub(crate) fn system(_face: Face, letter: char) -> Option<FontFile> {
    crate::android_text::file_for(letter)
}

/// The same on macOS: the file CoreText draws the character from.
#[cfg(target_vendor = "apple")]
pub(crate) fn system(face: Face, letter: char) -> Option<FontFile> {
    crate::core_text::file_for(face, letter).and_then(FontFile::open).filter(|font| font.glyph(letter) != 0)
}

/// The system font for a character: none known here.
#[cfg(not(any(windows, unix)))]
pub(crate) fn system(_face: Face, _letter: char) -> Option<FontFile> {
    None
}

impl TextSystem for BidiText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.text.metrics(font, scale)
    }

    /// A new language reshapes everything: cached runs and plans go.
    fn set_language(&mut self, language: Language) {
        if language != self.language {
            (self.language, self.plans, self.shaped, self.cache_bytes) = (language, Vec::new(), Vec::new(), 0);
        }
        #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
        self.forms.set(language);
        self.text.set_language(language);
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        if self.passes(text, font.face) {
            return self.text.measure(text, font, scale, stops);
        }
        let Some(stops) = stops else { return self.lay(text, font, scale, None, None) / scale };
        // Each character is as wide as shown, counted in logical order. (The `char`s of one
        // grapheme cluster share edges: they end where it ends, and only the first takes
        // width.)
        let mut edges = Vec::new();
        let wide = self.lay(text, font, scale, None, Some(&mut edges));
        let mut end = 0.0;
        for (n, [left, right]) in edges.iter().enumerate() {
            if n == 0 || edges[n - 1] != [*left, *right] {
                end += (right - left) / scale;
            }
            stops.push(end);
        }
        wide / scale
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        match self.passes(text, font.face) {
            true => self.text.draw(canvas, text, x, y, font, color),
            false => self.write(canvas, text, [x, y], font, color, None),
        }
    }

    fn places(&mut self, text: &str, font: Font, scale: f32, leftward: bool, places: &mut Vec<[f32; 2]>) -> f32 {
        if !leftward && self.passes(text, font.face) {
            places.clear();
            self.stops.clear();
            let wide = self.text.measure(text, font, scale, Some(&mut self.stops));
            places.extend(self.stops.iter().enumerate().map(|(n, stop)| [if n == 0 { 0.0 } else { self.stops[n - 1] }, *stop]));
            return wide;
        }
        let wide = self.lay(text, font, scale, Some(leftward), Some(places));
        places.iter_mut().for_each(|edge| *edge = edge.map(|x| x / scale));
        wide / scale
    }

    fn draw_line(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color, leftward: bool) {
        match !leftward && self.passes(text, font.face) {
            true => self.text.draw(canvas, text, x, y, font, color),
            false => self.write(canvas, text, [x, y], font, color, Some(leftward)),
        }
    }

    fn file(&mut self, face: Face, letter: char) -> Option<&FontFile> {
        self.text.file(face, letter)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The classes by Unicode's short names, as its tests write them.
    const NAMES: [&str; 23] = ["L", "R", "AL", "EN", "ES", "ET", "AN", "CS", "NSM", "BN", "B", "S", "WS", "ON", "LRE", "LRO", "RLE", "RLO", "PDF", "LRI", "RLI", "FSI", "PDI"];

    /// The display order of the characters not removed, left to right, by the levels (rule L2).
    fn visual(order: &Order) -> Vec<usize> {
        let mut shown: Vec<usize> = (0..order.levels.len()).filter(|at| !removed(order.written[*at])).collect();
        let highest = shown.iter().map(|at| order.levels[*at]).max().unwrap_or(0);
        let lowest = shown.iter().map(|at| order.levels[*at] | 1).min().unwrap_or(1);
        for level in (lowest..=highest).rev() {
            turn(&mut shown, |at| order.levels[*at] >= level);
        }
        shown
    }

    /// The levels as Unicode's tests write them: "x" for a removed character.
    fn written_levels(order: &Order) -> String {
        let each = (0..order.levels.len()).map(|at| if removed(order.written[at]) { "x".to_string() } else { order.levels[at].to_string() });
        each.collect::<Vec<_>>().join(" ")
    }

    /// The folder of Unicode's data files, if named (`TAUT_BIDI`).
    fn unicode() -> Option<std::path::PathBuf> {
        let folder = std::env::var_os("TAUT_BIDI").map(std::path::PathBuf::from);
        if folder.is_none() {
            eprintln!("TAUT_BIDI names no folder with Unicode's tests of the order: they are not run");
        }
        folder
    }

    /// The system's text, where available.
    #[cfg(any(feature = "right-to-left", feature = "brahmic"))]
    fn of_the_system() -> Option<Box<dyn TextSystem>> {
        crate::platform::SystemText::new().map(|text| Box::new(text) as Box<dyn TextSystem>)
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn an_interface_shows_text_that_runs_both_ways_and_draws_only_what_changed() {
        use crate::{Event, Key, Mods};
        let Some(system) = of_the_system() else { return eprintln!("no text of the system's own here: not tested") };
        let mut ui = crate::ui::Ui::new(system, Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 300.0, h: 330.0 }, 1.5);
        let mut screen = crate::surface::Surface::draw(&mut ui, 64);
        let (mut name, mut note, mut table, mut focused) = ("\u{5e9}\u{5dc}\u{5d5}\u{5dd} world".to_string(), String::new(), crate::table::Table::default(), false);
        note.push_str("\u{627}\u{644}\u{644}\u{63a}\u{629} \u{627}\u{644}\u{639}\u{631}\u{628}\u{64a}\u{629} \u{62c}\u{645}\u{64a}\u{644}\u{629} \u{62c}\u{62f}\u{627} (2026) \u{648}\u{647}\u{64a} \u{644}\u{63a}\u{629} Unicode.");
        let columns = [crate::table::Column::new("\u{5e9}\u{5dd}").unsorted(), crate::table::Column::new("Size").align(crate::layout::Align::End).unsorted()];
        let mut build = |ui: &mut crate::ui::Ui| {
            ui.column().pad(8.0).gap(6.0).open();
            ui.label("Hello, \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}! (123)");
            ui.label("\u{645}\u{631}\u{62d}\u{628}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}\u{60c} 2026");
            ui.row().gap(6.0).open();
            ui.button("\u{5e9}\u{5de}\u{5d5}\u{5e8}");
            ui.button("\u{62d}\u{641}\u{638} \u{627}\u{644}\u{645}\u{644}\u{641}");
            ui.close();
            let typed = ui.text_box(&mut name, "");
            if !std::mem::replace(&mut focused, true) {
                ui.set_focus(typed.id);
            }
            ui.text_area(&mut note, "", (3, 3));
            ui.table(&mut table, &columns);
            for (word, size) in [("\u{5e7}\u{5d5}\u{5d1}\u{5e5}.txt", "12 KB"), ("\u{645}\u{644}\u{641}", "3 MB")] {
                ui.table_row().open();
                ui.label(word);
                ui.label(size);
                ui.close();
            }
            ui.close();
            ui.close();
        };
        // After every run only what changed is redrawn, and the window is compared with a fresh
        // full draw.
        let mut step = |ui: &mut crate::ui::Ui, event: Option<Event>| {
            event.into_iter().for_each(|event| _ = ui.event(event));
            let changed = ui.run(&mut build);
            screen.redraw(ui, changed);
            let fresh = crate::surface::Surface::draw(ui, 64);
            assert!(screen.pixels == fresh.pixels, "drawing only what changed ({changed:?}) left the window different");
            fresh
        };
        for _ in 0..3 {
            step(&mut ui, None);
        }
        // The cursor moves through the Latin visually; a selection from inside it across the
        // space and the last Hebrew letter; a Hebrew letter typed over it.
        let keys = [Key::End, Key::Left, Key::Left];
        keys.into_iter().for_each(|key| _ = step(&mut ui, Some(Event::Key(key, Mods::NONE))));
        let picked = (0..4).map(|_| step(&mut ui, Some(Event::Key(Key::Right, Mods::SHIFT)))).last().unwrap();
        if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
            std::fs::write(std::path::Path::new(&folder).join(format!("both-{}-selected.png", std::env::consts::OS)), picked.to_png()).unwrap();
        }
        step(&mut ui, Some(Event::Text("\u{5d2}".into())));
        let last = step(&mut ui, Some(Event::Key(Key::End, Mods::NONE)));
        if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
            std::fs::write(std::path::Path::new(&folder).join(format!("both-{}.png", std::env::consts::OS)), last.to_png()).unwrap();
        }
        assert_eq!(name, "\u{5e9}\u{5dc}\u{5d5}\u{5d2}ld");
    }

    /// Bidirectional lines as pictures to inspect (with `TAUT_PICTURES` naming a folder): drawn
    /// by the system at twice unit size, each in a paragraph of its own direction, aligned to
    /// where such a paragraph starts.
    #[cfg(feature = "right-to-left")]
    #[test]
    fn lines_that_run_both_ways_are_drawn_where_they_are_seen() {
        let Some(system) = of_the_system() else { return eprintln!("no text of the system's own here: not tested") };
        let mut ordered = BidiText::around(system);
        let lines = [
            "Hello, \u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}! (123)",
            "\u{5e9}\u{5dc}\u{5d5}\u{5dd} world, 3.14 [x]",
            "\u{644}\u{63a}\u{629} Unicode.",
            "\u{628}\u{650}\u{633}\u{652}\u{645}\u{650} \u{627}\u{644}\u{644}\u{651}\u{64e}\u{647}\u{650} \u{627}\u{644}\u{631}\u{651}\u{64e}\u{62d}\u{652}\u{645}\u{64e}\u{670}\u{646}\u{650}",
            "\u{644}\u{627} \u{644}\u{623} \u{644}\u{622} \u{644}\u{625} \u{639}\u{640}\u{631}\u{628}\u{640}\u{64a} \u{661}\u{662}\u{663} 456",
            "\u{645}\u{6cc}\u{200c}\u{62e}\u{648}\u{627}\u{647}\u{645} \u{6a9}\u{62a}\u{627}\u{628} (\u{641}\u{627}\u{631}\u{633}\u{6cc})",
            "\u{5e9}\u{5b8}\u{5c1}\u{5dc}\u{5d5}\u{5b9}\u{5dd} \u{5d1}\u{5bc}\u{5b0}\u{5e8}\u{5b5}\u{5d0}\u{5e9}\u{5c1}\u{5b4}\u{5d9}\u{5ea}",
        ];
        let (font, scale) = (crate::text::Font::new(14.0), 2.0);
        let (width, line) = (420usize, (ordered.metrics(font, scale).height() * scale).ceil() as usize + 6);
        let mut pixels = Vec::new();
        for text in lines {
            let leftward = leftward(text);
            let wide = ordered.measure(text, font, scale, None);
            let x = if leftward { width as f32 / scale - 4.0 - wide } else { 4.0 };
            let mut row = vec![0xffff_ffffu32; width * line];
            ordered.draw_line(&mut Canvas::new(&mut row, width, line, (0, 0), scale), text, x, 1.0, font, Color::BLACK, leftward);
            // Every line is drawn within its measured width, and what its font's outlines reach
            // past their advances.
            let inked: Vec<usize> = row.iter().enumerate().filter(|(_, pixel)| **pixel != 0xffff_ffff).map(|(at, _)| at % width).collect();
            let (left, right) = (inked.iter().min().copied().unwrap_or(0) as f32 / scale, inked.iter().max().copied().unwrap_or(0) as f32 / scale);
            let [before, after] = overhang(text, font.size);
            assert!(left >= x - 2.0 - before && right <= x + wide + 2.0 + after, "{text:?}: ink from {left} to {right}, measured from {x} to {}", x + wide);
            pixels.extend(row);
        }
        if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
            let file = std::path::Path::new(&folder).join(format!("lines-{}.png", std::env::consts::OS));
            std::fs::write(file, crate::png::encode(width, &pixels).unwrap()).unwrap();
        }
    }

    /// How far the outlines of a line's Arabic and Hebrew words, in the system's font, reach past
    /// their advances on the left and right, in units at `size`. (Some fonts place a mark off its
    /// base: SF Arabic puts the kasra of a final noon 2.5 units at 14 left of the noon's start,
    /// as HarfBuzz does.)
    #[cfg(feature = "right-to-left")]
    fn overhang(text: &str, size: f32) -> [f32; 2] {
        let mut most = [0.0f32; 2];
        for word in text.split(' ') {
            let of_its_own = |letter: char| Script::of(letter).filter(|script| matches!(script, Script::Arabic | Script::Hebrew)).map(|script| (letter, script));
            let Some((letter, script)) = word.chars().find_map(of_its_own) else { continue };
            let Some(font) = system(Face::Regular, letter) else { continue };
            let mut glyphs = Vec::new();
            let total = font.shape(&Plan::of(&font, script, true, Language::NONE), word, &mut glyphs);
            let mut pen = 0;
            for laid in &glyphs {
                let reach = font.outline_through(laid.glyph, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).1;
                let at = (pen + laid.offset[0]) as f32;
                most[0] = most[0].max(-(at + reach[0]) * font.unit(size));
                most[1] = most[1].max((at + reach[2] - total as f32) * font.unit(size));
                pen += laid.advance;
            }
        }
        most
    }

    /// Lines in Indic and Southeast Asian scripts: Hindi, Bengali, Gurmukhi, Gujarati, Oriya,
    /// Tamil, Telugu, Kannada, Malayalam, Sinhala, Thai, Lao, Khmer, Burmese, Tibetan, and one
    /// with Latin among Hindi.
    #[cfg(feature = "brahmic")]
    const SYLLABIC: [&str; 16] = [
        "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947} \u{926}\u{941}\u{928}\u{93f}\u{92f}\u{93e} \u{915}\u{94d}\u{937}\u{92e}\u{93e} \u{905}\u{930}\u{94d}\u{925}",
        "\u{9ac}\u{9be}\u{982}\u{9b2}\u{9be} \u{9ad}\u{9be}\u{9b7}\u{9be} \u{995}\u{9cb}\u{9a8}",
        "\u{a2a}\u{a70}\u{a1c}\u{a3e}\u{a2c}\u{a40} \u{a38}\u{a4d}\u{a30}\u{a40}",
        "\u{a97}\u{ac1}\u{a9c}\u{ab0}\u{abe}\u{aa4}\u{ac0} \u{a95}\u{acd}\u{ab7}\u{aae}\u{abe}",
        "\u{b13}\u{b21}\u{b3c}\u{b3f}\u{b06} \u{b2d}\u{b3e}\u{b37}\u{b3e}",
        "\u{ba4}\u{bae}\u{bbf}\u{bb4}\u{bcd} \u{b95}\u{bca}\u{ba3}\u{bcd}\u{b9f}\u{bc1}",
        "\u{c24}\u{c46}\u{c32}\u{c41}\u{c17}\u{c41} \u{c36}\u{c4d}\u{c30}\u{c40}",
        "\u{c95}\u{ca8}\u{ccd}\u{ca8}\u{ca1} \u{ca6}\u{ccd}\u{cb5}\u{cbe}\u{cb0}",
        "\u{d2e}\u{d32}\u{d2f}\u{d3e}\u{d33}\u{d02} \u{d15}\u{d47}\u{d30}\u{d33}\u{d02}",
        "\u{dc3}\u{dd2}\u{d82}\u{dc4}\u{dbd} \u{dc1}\u{dca}\u{200d}\u{dbb}\u{dd3}",
        "\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22} \u{e19}\u{e49}\u{e33}",
        "\u{e9e}\u{eb2}\u{eaa}\u{eb2}\u{ea5}\u{eb2}\u{ea7}",
        "\u{1797}\u{17b6}\u{179f}\u{17b6}\u{1781}\u{17d2}\u{1798}\u{17c2}\u{179a} \u{179f}\u{17d2}\u{178f}\u{17d2}\u{179a}\u{17b8}",
        "\u{1019}\u{103c}\u{1014}\u{103a}\u{1019}\u{102c} \u{1019}\u{1004}\u{103a}\u{1039}\u{1002}\u{101c}\u{102c}",
        "\u{f56}\u{f7c}\u{f51}\u{f0b}\u{f61}\u{f72}\u{f42}",
        "Hello \u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947} world \u{915}\u{93f}",
    ];

    /// Indic and Southeast Asian lines as pictures to inspect (with `TAUT_PICTURES` naming a
    /// folder), each within its measured width, in the system fonts; and each line's grapheme
    /// clusters, in order, side by side.
    #[cfg(feature = "brahmic")]
    #[test]
    fn syllables_are_drawn_where_they_are_measured() {
        let Some(system) = of_the_system() else { return eprintln!("no text of the system's own here: not tested") };
        let mut ordered = BidiText::around(system);
        let (font, scale) = (crate::text::Font::new(14.0), 2.0);
        let (width, line) = (520usize, (ordered.metrics(font, scale).height() * scale).ceil() as usize + 16);
        let mut pixels = Vec::new();
        let mut shaped = 0;
        for text in SYLLABIC {
            let first = text.chars().find_map(Script::of);
            let Some(Script::Brahmic(_)) = first else { continue };
            let letter = text.chars().find(|letter| Script::of(*letter) == first).unwrap_or(' ');
            if super::system(Face::Regular, letter).is_none() {
                eprintln!("no font of the system's for {}: not drawn", letter.escape_unicode());
                continue;
            }
            shaped += 1;
            let (mut stops, mut places) = (Vec::new(), Vec::new());
            let wide = ordered.measure(text, font, scale, Some(&mut stops));
            ordered.places(text, font, scale, false, &mut places);
            let mut row = vec![0xffff_ffffu32; width * line];
            ordered.draw_line(&mut Canvas::new(&mut row, width, line, (0, 0), scale), text, 4.0, 4.0, font, Color::BLACK, false);
            let inked: Vec<usize> = row.iter().enumerate().filter(|(_, pixel)| **pixel != 0xffff_ffff).map(|(at, _)| at % width).collect();
            let (left, right) = (inked.iter().min().copied().unwrap_or(0) as f32 / scale, inked.iter().max().copied().unwrap_or(0) as f32 / scale);
            assert!(!inked.is_empty() && left >= 4.0 - 3.0 && right <= 4.0 + wide + 3.0, "{text:?}: ink from {left} to {right}, measured from 4 to {}", 4.0 + wide);
            // Character ends are in order, the last at the line's end; the `char`s of one
            // cluster end where it ends.
            assert!(stops.windows(2).all(|pair| pair[0] <= pair[1]) && (stops.last().copied().unwrap_or(0.0) - wide).abs() < 0.6, "{text:?}: {stops:?} in {wide}");
            let mut at = 0;
            while at < text.len() {
                let (first, next) = (text[..at].chars().count(), at + crate::text::cluster(&text[at..]));
                let last = text[..next].chars().count() - 1;
                assert!((first..=last).all(|n| places[n] == places[first]), "{text:?}: {:?}", &places[first..=last]);
                at = next;
            }
            pixels.extend(row);
        }
        if let (Some(folder), false) = (std::env::var_os("TAUT_PICTURES"), pixels.is_empty()) {
            let file = std::path::Path::new(&folder).join(format!("syllables-{}.png", std::env::consts::OS));
            std::fs::write(file, crate::png::encode(width, &pixels).unwrap()).unwrap();
        }
        println!("{shaped} lines shaped");
    }

    #[cfg(feature = "brahmic")]
    #[test]
    fn an_interface_shows_syllables_and_draws_only_what_changed() {
        use crate::{Event, Key, Mods};
        let Some(system) = of_the_system() else { return eprintln!("no text of the system's own here: not tested") };
        if system_has_none(&['\u{915}']) {
            return eprintln!("no font of the system's for Devanagari: not tested");
        }
        let mut ui = crate::ui::Ui::new(system, Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 300.0, h: 360.0 }, 1.5);
        let mut screen = crate::surface::Surface::draw(&mut ui, 64);
        let (mut name, mut note, mut focused) = ("\u{915}\u{94d}\u{937}\u{93f} \u{928}\u{92e}".to_string(), String::new(), false);
        note.push_str(SYLLABIC[10]);
        note.push(' ');
        note.push_str(SYLLABIC[12]);
        let mut build = |ui: &mut crate::ui::Ui| {
            ui.column().pad(8.0).gap(6.0).open();
            for line in &SYLLABIC[..6] {
                ui.label(line);
            }
            ui.row().gap(6.0).open();
            ui.button("\u{938}\u{939}\u{947}\u{91c}\u{947}\u{902}");
            ui.button("\u{e1a}\u{e31}\u{e19}\u{e17}\u{e36}\u{e01}");
            ui.close();
            let typed = ui.text_box(&mut name, "");
            if !std::mem::replace(&mut focused, true) {
                ui.set_focus(typed.id);
            }
            ui.text_area(&mut note, "", (3, 3));
            ui.close();
        };
        // After every run only what changed is redrawn, and the window is compared with a fresh
        // full draw.
        let mut step = |ui: &mut crate::ui::Ui, event: Option<Event>| {
            event.into_iter().for_each(|event| _ = ui.event(event));
            let changed = ui.run(&mut build);
            screen.redraw(ui, changed);
            let fresh = crate::surface::Surface::draw(ui, 64);
            assert!(screen.pixels == fresh.pixels, "drawing only what changed ({changed:?}) left the window different");
            fresh
        };
        for _ in 0..3 {
            step(&mut ui, None);
        }
        // The cursor steps over a whole syllable: from the start, one step right is after क्षि;
        // a selection of the next two characters (the space and न); typed over.
        step(&mut ui, Some(Event::Key(Key::Home, Mods::NONE)));
        step(&mut ui, Some(Event::Key(Key::Right, Mods::NONE)));
        step(&mut ui, Some(Event::Key(Key::Right, Mods::SHIFT)));
        let picked = step(&mut ui, Some(Event::Key(Key::Right, Mods::SHIFT)));
        if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
            std::fs::write(std::path::Path::new(&folder).join(format!("syllables-ui-{}-selected.png", std::env::consts::OS)), picked.to_png()).unwrap();
        }
        step(&mut ui, Some(Event::Text("\u{930}\u{94d}".into())));
        let last = step(&mut ui, Some(Event::Key(Key::End, Mods::NONE)));
        if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
            std::fs::write(std::path::Path::new(&folder).join(format!("syllables-ui-{}.png", std::env::consts::OS)), last.to_png()).unwrap();
        }
        assert_eq!(name, "\u{915}\u{94d}\u{937}\u{93f}\u{930}\u{94d}\u{92e}");
    }

    /// Whether the system lacks a font for all of some letters.
    #[cfg(feature = "brahmic")]
    fn system_has_none(letters: &[char]) -> bool {
        letters.iter().all(|letter| super::system(Face::Regular, *letter).is_none())
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn text_is_told_to_run_one_way_by_its_bytes_and_never_wrongly() {
        // Every character up to the last plane in use that is a right-to-left letter or opens
        // such a run, at every offset in eight bytes: the bytes never let it pass as plain.
        let asks = ['\u{200f}', '\u{202b}', '\u{202e}', '\u{2067}', '\u{2068}'];
        for letter in (0..0x20000).filter_map(char::from_u32).filter(|letter| matches!(kind(*letter), Kind::Right | Kind::Arabic) || asks.contains(letter)) {
            for before in 0..8 {
                let text = ["abcdefgh"[..before].to_string(), letter.to_string(), "xyz".into()].concat();
                assert!(!plain(&text), "{:x} is taken for plain text", letter as u32);
            }
        }
        // And plain text of many scripts is recognized.
        let texts = [
            "plain",
            "caf\u{e9} na\u{ef}ve",
            "\u{3b1}\u{3b2}\u{3b3} \u{431}\u{432}\u{433}",
            "\u{4e2d}\u{6587}\u{3001}",
            "\u{d55c}\u{ae00}",
            "\u{1f600} \u{2764}\u{fe0f}",
            "\u{2014}\u{201c}\u{201d}\u{2026}",
        ];
        assert!(texts.iter().all(|text| plain(text)));
        // Thai and Devanagari are plain too; but not with `brahmic`, which shapes them.
        assert!(["\u{e01}\u{e34}", "\u{915}\u{93f}"].iter().all(|text| plain(text) != cfg!(feature = "brahmic")));
    }

    #[cfg(feature = "brahmic")]
    #[test]
    fn text_of_india_and_south_east_asia_is_never_taken_for_plain() {
        // Every letter and sign of a syllabic script, at every offset in eight bytes.
        for letter in (0..0x20000).filter_map(char::from_u32).filter(|letter| matches!(Script::of(*letter), Some(Script::Brahmic(_)))) {
            for before in 0..8 {
                let text = ["abcdefgh"[..before].to_string(), letter.to_string(), "xyz".into()].concat();
                assert!(!plain(&text), "{:x} is taken for plain text", letter as u32);
            }
        }
    }

    /// An app font with a few Hebrew letters: alef is "A" (7 pixels wide at 16), gimel "C"
    /// (10), space 4; over the built-in font (12 wide), which draws what it lacks.
    pub(crate) fn hebrew_letters() -> crate::font_file::AppFonts {
        use crate::font_file::tests::{file, mapped, tables, with};
        let cmap = mapped(&[[0x20, 0x20, 4], [0x41, 0x43, 1], [0x5d0, 0x5d0, 1], [0x5d2, 0x5d2, 3]]);
        let font = FontFile::read(file(&[0, 1, 0, 0], &with(tables(), b"cmap", cmap))).unwrap();
        crate::font_file::AppFonts::new(font).fallback(Box::new(crate::text::BuiltInFont))
    }
    #[cfg(feature = "right-to-left")]
    pub(crate) fn hebrew() -> BidiText {
        BidiText::around(Box::new(hebrew_letters()))
    }

    /// Without `right-to-left` nothing is reordered: a line of Hebrew and Latin is drawn as
    /// typed, each letter after the one before, by the interface's text system (alef 7, gimel
    /// 10, space 4, Latin 12), right-to-left paragraph or not: the very system drawing the
    /// letters is asked where each goes, and it is drawn there.
    #[cfg(not(feature = "right-to-left"))]
    #[test]
    fn without_the_feature_a_line_is_written_as_it_was_typed() {
        let mut ui = crate::ui::Ui::new(Box::new(hebrew_letters()), Box::new(crate::platform::LocalClipboard::default()));
        ui.set_text_system(Box::new(hebrew_letters()));
        let (text, font) = ("\u{5d0}\u{5d2} cd", Font::new(16.0));
        assert!(!leftward(text) && plain(text) && plain("\u{627}\u{644}"));
        let typed = vec![[0.0, 7.0], [7.0, 17.0], [17.0, 21.0], [21.0, 33.0], [33.0, 45.0]];
        for leftward in [false, true] {
            let mut places = Vec::new();
            assert_eq!((ui.text.places(text, font, 1.0, leftward, &mut places), places), (45.0, typed.clone()));
        }
        let mut stops = Vec::new();
        assert_eq!((ui.text.measure(text, font, 1.0, Some(&mut stops)), stops), (45.0, vec![7.0, 17.0, 21.0, 33.0, 45.0]));
    }

    #[cfg(feature = "right-to-left")]
    #[test]
    fn a_line_is_shown_in_the_order_unicode_gives_it() {
        let mut ordered = hebrew();
        let font = Font::new(16.0);
        let places = |ordered: &mut BidiText, text: &str, leftward: bool| {
            let mut places = Vec::new();
            let wide = ordered.places(text, font, 1.0, leftward, &mut places);
            (wide, places)
        };
        // Alef and gimel, a space and two Latin letters. In a left-to-right paragraph the
        // Hebrew shows reversed at its start: gimel, then alef; the space follows the
        // paragraph, after them.
        let text = "\u{5d0}\u{5d2} cd";
        assert_eq!(places(&mut ordered, text, false), (45.0, vec![[10.0, 17.0], [0.0, 10.0], [17.0, 21.0], [21.0, 33.0], [33.0, 45.0]]));
        // In a right-to-left one the Latin comes first (it is at the paragraph's end), and the
        // space goes with the Hebrew.
        assert_eq!(places(&mut ordered, text, true), (45.0, vec![[38.0, 45.0], [28.0, 38.0], [24.0, 28.0], [0.0, 12.0], [12.0, 24.0]]));
        // Measured, each character is as wide as shown, counted in logical order; and the line
        // is as wide as drawn.
        let mut stops = Vec::new();
        assert_eq!((ordered.measure(text, font, 1.0, Some(&mut stops)), stops), (45.0, vec![7.0, 17.0, 21.0, 33.0, 45.0]));
        // Two runs of one line the font draws, with what it lacks between them: each is cached,
        // and the line measures the same the first time as after.
        let mut fresh = hebrew();
        let first = fresh.measure("\u{5d2}!\u{5d0}", font, 1.0, None);
        assert_eq!((first, fresh.measure("\u{5d2}!\u{5d0}", font, 1.0, None)), (29.0, 29.0));
        // A line with nothing right-to-left is the inner system's own.
        let mut alone = hebrew_letters();
        for text in ["", "plain AB", "caf\u{e9}"] {
            let mut stops = (Vec::new(), Vec::new());
            assert_eq!(ordered.measure(text, font, 1.0, Some(&mut stops.0)), alone.measure(text, font, 1.0, Some(&mut stops.1)));
            assert_eq!(stops.0, stops.1);
        }
    }

    #[test]
    fn every_case_of_unicodes_test_of_kinds_comes_out_as_it_says() {
        let Some(folder) = unicode() else { return };
        let text = std::fs::read_to_string(folder.join("BidiTest.txt")).unwrap();
        let (mut levels, mut reorder, mut order, mut cases, mut wrong) = (String::new(), String::new(), Order::default(), 0, Vec::new());
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            if let Some(said) = line.strip_prefix("@Levels:") {
                levels = said.split_whitespace().collect::<Vec<_>>().join(" ");
                continue;
            }
            if let Some(said) = line.strip_prefix("@Reorder:") {
                reorder = said.split_whitespace().collect::<Vec<_>>().join(" ");
                continue;
            }
            let (input, bits) = line.split_once(';').unwrap();
            let kinds: Vec<Kind> = input.split_whitespace().map(|name| ALL[NAMES.iter().position(|known| *known == name).unwrap()]).collect();
            let bits = u8::from_str_radix(bits.trim(), 16).unwrap();
            for (bit, leftward) in [(1, None), (2, Some(false)), (4, Some(true))].into_iter().filter(|(bit, _)| bits & bit != 0) {
                order.letters = vec!['a'; kinds.len()];
                order.written = kinds.clone();
                order.resolve(leftward);
                let shown = visual(&order).iter().map(|at| at.to_string()).collect::<Vec<_>>().join(" ");
                cases += 1;
                if written_levels(&order) != levels || shown != reorder {
                    wrong.push(format!("{input} ({bit}): {} / {shown}, not {levels} / {reorder}", written_levels(&order)));
                }
            }
        }
        println!("{cases} cases, {} wrong", wrong.len());
        assert!(wrong.is_empty(), "{} of {cases} wrong:\n{}", wrong.len(), wrong[..wrong.len().min(20)].join("\n"));
    }

    /// Unicode's classes of all characters, as its data lists them (with every script's marks).
    fn listed(folder: &std::path::Path) -> Vec<Kind> {
        let text = std::fs::read_to_string(folder.join("DerivedBidiClass.txt")).unwrap();
        let long = |name: &str| match name {
            "Left_To_Right" => "L",
            "Right_To_Left" => "R",
            "Arabic_Letter" => "AL",
            "European_Terminator" => "ET",
            other => panic!("{other}"),
        };
        let mut all = vec![Kind::Left; 0x110000];
        let span = |text: &str| {
            let mut parts = text.trim().split("..").map(|part| usize::from_str_radix(part.trim(), 16).unwrap());
            let first = parts.next().unwrap();
            first..=parts.next().unwrap_or(first)
        };
        for line in text.lines().filter_map(|line| line.strip_prefix("# @missing: ")) {
            let (range, name) = line.split_once(';').unwrap();
            all[span(range)].fill(ALL[NAMES.iter().position(|known| *known == long(name.trim())).unwrap()]);
        }
        for line in text.lines().filter(|line| !line.starts_with('#') && !line.trim().is_empty()) {
            let (range, rest) = line.split_once(';').unwrap();
            let name = rest.split('#').next().unwrap().trim();
            all[span(range)].fill(ALL[NAMES.iter().position(|known| *known == name).unwrap()]);
        }
        all
    }

    #[test]
    fn every_case_of_unicodes_test_of_characters_comes_out_as_it_says() {
        let Some(folder) = unicode() else { return };
        let exact = listed(&folder);
        // The classes here are Unicode's, except left-to-right nonspacing marks counted as
        // letters: all others match.
        let differ: Vec<usize> = (0..0x110000).filter(|code| char::from_u32(*code as u32).is_some_and(|letter| kind(letter) != exact[*code])).collect();
        assert!(differ.iter().all(|code| exact[*code] == Kind::Mark && kind(char::from_u32(*code as u32).unwrap()) == Kind::Left), "{:x?}", &differ[..differ.len().min(20)]);
        println!("{} marks of scripts that run from left to right are counted as letters", differ.len());
        let text = std::fs::read_to_string(folder.join("BidiCharacterTest.txt")).unwrap();
        let (mut order, mut cases, mut wrong, mut by_table) = (Order::default(), 0, Vec::new(), 0);
        for line in text.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
            let fields: Vec<&str> = line.split(';').collect();
            let letters: Vec<char> = fields[0].split_whitespace().map(|code| char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap()).collect();
            let leftward = match fields[1] {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
            let (paragraph, levels, reorder) = (fields[2].parse::<u8>().unwrap(), fields[3].trim(), fields[4].trim());
            cases += 1;
            // With the full class list, the order must match Unicode's to the character;
            order.letters = letters.clone();
            order.written = letters.iter().map(|letter| exact[*letter as usize]).collect();
            let level = order.resolve(leftward);
            let shown = visual(&order).iter().map(|at| at.to_string()).collect::<Vec<_>>().join(" ");
            if level != paragraph || written_levels(&order) != levels || shown != reorder {
                wrong.push(format!("{} ({}): {level} {} / {shown}, not {paragraph} {levels} / {reorder}", fields[0], fields[1], written_levels(&order)));
            }
            // with the classes as stored here, it may differ only where a left-to-right mark is
            // in the text.
            let kept = order.of(&letters.iter().collect::<String>(), leftward);
            let shown = visual(&order).iter().map(|at| at.to_string()).collect::<Vec<_>>().join(" ");
            if kept != paragraph || written_levels(&order) != levels || shown != reorder {
                by_table += 1;
                assert!(letters.iter().any(|letter| differ.binary_search(&(*letter as usize)).is_ok()), "{line}");
            }
        }
        println!("{cases} cases, {} wrong; {by_table} differ by the kinds kept here", wrong.len());
        assert!(wrong.is_empty(), "{} of {cases} wrong:\n{}", wrong.len(), wrong[..wrong.len().min(20)].join("\n"));
    }

    /// The glyphs of the runs a line is shaped in here, as drawn.
    #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
    fn shaped_glyphs(text: &mut BidiText, line: &str, font: Font) -> Vec<u16> {
        text.lay(line, font, 1.0, None, None);
        let mut all = Vec::new();
        for piece in text.pieces.clone() {
            let Way::Shaped(_) = piece.way else { continue };
            let run = &line[piece.bytes];
            let script = run.chars().find_map(Script::of).unwrap_or(Script::None);
            let letter = run.chars().find(|letter| Script::of(*letter) == Some(script)).unwrap_or(' ');
            let serial = text.file(font.face, script, letter).unwrap().serial();
            let plan = &text.plans.iter().find(|(of, known, _)| *of == serial && *known == script).unwrap().2;
            let file = text.fonts.iter().find(|(known, of, _)| *known == font.face && *of == script).unwrap().2.as_ref().unwrap();
            file.shape(plan, run, &mut text.glyphs);
            all.extend(text.glyphs.iter().map(|laid| laid.glyph));
        }
        all
    }

    #[test]
    #[cfg(all(feature = "shaping", not(any(target_os = "android", target_family = "wasm"))))]
    fn lines_the_systems_text_leaves_without_a_languages_forms_are_shaped_with_them() {
        // Families with forms of their own for a language, where installed, and words taking
        // them: a mark on the first letter, or a letter the family's font lacks after them, keeps
        // the system's text from swapping them in.
        let families = [
            ("Bahnschrift", "sr", "бгдпт"),
            ("Noto Sans", "bg", "вдгжзийклптцчшщъю"),
            ("DejaVu Sans", "sr", "бгдпт"),
            ("Arial", "ro", "şţ"),
            ("Times New Roman", "ro", "şţ"),
            ("Georgia", "ro", "şţ"),
            ("Verdana", "ro", "şţ"),
        ];
        let mut tried = 0;
        for (family, language, word) in families {
            let (face, language) = (Face::named(family), Language::named(language));
            let font = Font { face, ..Font::new(32.0) };
            // (Where the family is not installed, the system's own font stands in: one without
            // such forms, often.)
            let Some(file) = crate::system_fonts::file_for(face, 'a') else { continue };
            let forms: Vec<u16> = word.chars().map(|letter| crate::shaping::local(&file, letter, language)).filter(|form| !word.chars().any(|letter| file.glyph(letter) == *form)).collect();
            if forms.is_empty() {
                continue;
            }
            let other = ['α', 'ა', 'Ա', 'ሀ'].into_iter().find(|letter| file.glyph(*letter) == 0 && crate::system_fonts::file_for(face, *letter).is_some_and(|other| other.glyph(*letter) != 0));
            let mut letters = word.chars();
            let first = letters.next().unwrap();
            let lines = [Some(format!("{first}\u{301}{}", letters.as_str())), other.map(|other| format!("{word} {other}"))];
            for line in lines.iter().flatten() {
                let mut text = BidiText::around(Box::new(crate::platform::SystemText::new().expect("the system's text")));
                text.set_language(language);
                let glyphs = shaped_glyphs(&mut text, line, font);
                assert!(forms.iter().all(|form| glyphs.contains(form)), "{family}, {line}: {glyphs:?} lacks {forms:?}");
                // And none without the language.
                text.set_language(Language::NONE);
                assert!(shaped_glyphs(&mut text, line, font).is_empty(), "{family}, {line}");
            }
            // A line of simple script is the system's text's own, with the forms swapped in
            // (except on Apple's systems, whose text takes no forms itself).
            let mut text = BidiText::around(Box::new(crate::platform::SystemText::new().unwrap()));
            text.set_language(language);
            assert_eq!(shaped_glyphs(&mut text, word, font).is_empty(), cfg!(not(target_vendor = "apple")), "{family}");
            tried += 1;
            println!("{family}: forms {forms:?} in lines with a mark{}", if other.is_some() { " and with a letter of another font" } else { "" });
        }
        if tried == 0 {
            eprintln!("no system font with such forms: not tested");
        }
    }

    #[test]
    #[cfg(all(feature = "shaping", not(feature = "right-to-left"), not(any(target_os = "android", target_family = "wasm"))))]
    fn without_right_to_left_hebrew_in_a_line_shaped_for_its_forms_stays_the_systems() {
        // A Serbian word with a mark, then Hebrew: the word is shaped for its forms, the Hebrew
        // left to the system's text as typed (not reversed here).
        let line = "б\u{301}гдпт שלום";
        let hebrew = line.find('ש').unwrap();
        for family in ["Bahnschrift", "DejaVu Sans"] {
            let font = Font { face: Face::named(family), ..Font::new(32.0) };
            let mut text = BidiText::around(Box::new(crate::platform::SystemText::new().expect("the system's text")));
            text.set_language(Language::named("sr"));
            text.lay(line, font, 1.0, None, None);
            if !text.pieces.iter().any(|piece| matches!(piece.way, Way::Shaped(_))) {
                continue;
            }
            let shaped_hebrew = text.pieces.iter().any(|piece| piece.bytes.end > hebrew && !matches!(piece.way, Way::Ahead));
            assert!(!shaped_hebrew, "{family}: {:?}", text.pieces);
            return;
        }
        eprintln!("no system font with Serbian forms: not tested");
    }
}
