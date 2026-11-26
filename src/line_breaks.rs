//! Line breaks in text with no spaces between words.
//!
//! **Chinese, Japanese and Korean** (with Latin, digits and punctuation among them), for
//! vertical text: Unicode's line breaking (UAX #14) for the classes such text holds. A line
//! breaks between ideographs and kana, never before closing punctuation, iteration marks or
//! ellipses, nor after opening brackets (kinsoku); Latin words and numbers break only at
//! spaces. Small kana and the long vowel mark may start a line, as CSS `line-break: normal`
//! has it (Chrome's default).
//!
//! **Thai, Lao, Khmer and Burmese** (Unicode line breaking's "complex context"). Word
//! boundaries there need a dictionary per language. A line breaks between words, as:
//!
//! - Windows: Uniscribe's `ScriptBreak` says.
//! - macOS and iOS: CoreFoundation's tokenizer says (with Apple's dictionaries).
//! - Linux, Android and the web: ICU's word lists, carried here (`word_breaks.rs`, 804 KB; feature
//!   `word-lists`; without it, between syllables).
//!
//! A word longer than a line breaks between syllables: between grapheme clusters (which keep
//! Khmer and Burmese stacked consonants together), but never after a Thai or Lao vowel written
//! before its consonant, before one of their vowels or signs that follows it, or before a
//! Burmese final consonant (with asat, or stacked).
//!
//! Feature `brahmic`.

/// A character's line breaking class (UAX #14), as far as CJK text with Latin among it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BreakClass {
    /// A space (SP).
    Space,
    /// No break either side: no-break space (GL), word joiner (WJ).
    Glue,
    /// Zero width space (ZW): a break after it.
    ZeroWidth,
    /// Opening punctuation (OP): no break after.
    Open,
    /// Closing punctuation (CL, CP), `!` and `?` (EX), `,` `.` `:` `;` (IS), `/` (SY): no break
    /// before.
    Close,
    /// Iteration marks, middle dots and other non-starters (NS): no break before.
    Nonstarter,
    /// Ellipses (IN): no break before.
    Inseparable,
    /// Quotation marks (QU): no break either side.
    Quote,
    /// Hyphens and breaking spaces (BA, HY): a break after, not before.
    After,
    /// Breaks before, not after (BB).
    Before,
    /// Currency and other signs before a number (PR).
    Prefix,
    /// Percent and other signs after a number (PO).
    Postfix,
    /// Digits (NU).
    Number,
    /// Letters and symbols of alphabets (AL): no break between them.
    Letter,
    /// Ideographs, kana (small ones too: CJ), Hangul, fullwidth forms, emoji (ID): a break
    /// either side.
    Ideograph,
}

/// The classes, in the order `CLASSES` numbers them.
const BY_NUMBER: [BreakClass; 15] = {
    use BreakClass::*;
    [Space, Glue, ZeroWidth, Open, Close, Nonstarter, Inseparable, Quote, After, Before, Prefix, Postfix, Number, Letter, Ideograph]
};

/// The class of a character: from `CLASSES`, else ideographic in the CJK blocks and emoji,
/// else a letter.
pub(crate) fn break_class(letter: char) -> BreakClass {
    let code = letter as u32;
    let n = CLASSES.partition_point(|entry| entry >> 4 <= code);
    if let Some(class) = n.checked_sub(1).and_then(|n| BY_NUMBER.get((CLASSES[n] & 15) as usize)) {
        return *class;
    }
    let ideographic = matches!(code, 0x1100..=0x115f | 0x2e80..=0x2fff | 0x3000..=0x30ff | 0x3105..=0x4dbf | 0x4e00..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f);
    match ideographic || matches!(code, 0xff00..=0xffef | 0x1f000..=0x1faff | 0x20000..=0x3fffd) {
        true => BreakClass::Ideograph,
        false => BreakClass::Letter,
    }
}

/// Whether a line may break before character `after`, the last character before it that was
/// no space being `before`, with spaces between them or not.
fn may_break(before: char, spaced: bool, after: char) -> bool {
    use BreakClass::*;
    // (Brackets that are not East Asian: fullwidth and CJK ones lie from U+2E80.)
    let narrow = |letter: char| (letter as u32) < 0x2e80;
    match (break_class(before), spaced, break_class(after)) {
        (_, _, Space) => false,                                                          // LB7
        (ZeroWidth, ..) => true,                                                         // LB8
        (Glue, false, _) | (_, false, Glue) => false,                                    // LB11, LB12
        (_, _, Close) => false,                                                          // LB13
        (Open, ..) => false,                                                             // LB14
        (Quote, _, Open) => false,                                                       // LB15
        (Close, _, Nonstarter) => false,                                                 // LB16
        (_, true, _) => true,                                                            // LB18
        (Quote, ..) | (_, _, Quote) => false,                                            // LB19
        (Before, ..) | (_, _, After | Nonstarter | Inseparable) => false,                // LB21, LB22
        (Letter | Number, _, Letter | Number) => false,                                  // LB23, LB28, LB29
        (Prefix, _, Ideograph | Number | Letter) | (Ideograph, _, Postfix) => false,     // LB23a, LB24, LB25
        (Postfix, _, Number | Letter) | (Number | Letter, _, Prefix | Postfix) => false, // LB24, LB25
        (After, _, Number) => false,                                                     // LB25 (a hyphen before digits)
        (Letter | Number, _, Open) if narrow(after) => false,                            // LB30: "f(x"
        (Close, _, Letter | Number) if matches!(before, ')' | ']') => false,             // LB30: "(s)he"
        _ => true,                                                                       // LB31
    }
}

/// The byte offsets in `text` (one paragraph, without its line end) where a line may break:
/// between grapheme clusters, each taking the class of its first character.
pub(crate) fn break_chances(text: &str, chances: &mut Vec<usize>) {
    chances.clear();
    let (mut at, mut before, mut spaced) = (0, None, false);
    while at < text.len() {
        let letter = text[at..].chars().next().unwrap_or(' ');
        if before.is_some_and(|before| may_break(before, spaced, letter)) {
            chances.push(at);
        }
        match break_class(letter) {
            BreakClass::Space => spaced = true,
            _ => (before, spaced) = (Some(letter), false),
        }
        at += crate::text::cluster(&text[at..]);
    }
}

/// The characters whose class is other than their block's (`break_class`): each change of
/// class as its first character times 16, plus the class (its place in `BreakClass`; 15 for the
/// block's). Unicode 17 `LineBreak.txt`, for Latin, punctuation and symbols, the CJK blocks and
/// forms, and emoji (made by `breaks` in the vertical text tools).
#[rustfmt::skip]
const CLASSES: [u32; 549] = [
    0x98, 0xaf, 0x200, 0x214, 0x227, 0x23f, 0x24a, 0x25b, 0x26f, 0x277, 0x283, 0x294,
    0x2af, 0x2ba, 0x2c4, 0x2d8, 0x2e4, 0x30c, 0x3a4, 0x3cf, 0x3f4, 0x40f, 0x5b3, 0x5ca,
    0x5d4, 0x5ef, 0x7b3, 0x7c8, 0x7d4, 0x7ef, 0xa01, 0xa13, 0xa2b, 0xa3a, 0xa6f, 0xab7,
    0xacf, 0xad8, 0xaef, 0xb0b, 0xb1a, 0xb2f, 0xb49, 0xb5f, 0xbb7, 0xbcf, 0xbf3, 0xc0f,
    0x2c89, 0x2c9f, 0x2cc9, 0x2cdf, 0x2df9, 0x2e0f, 0x35c1, 0x363f, 0x20008, 0x20071, 0x20088, 0x200b2,
    0x200cf, 0x20111, 0x2012f, 0x2014e, 0x2015f, 0x20187, 0x201a3, 0x201b7, 0x201e3, 0x201f7, 0x2020f, 0x20246,
    0x20278, 0x2028f, 0x202f1, 0x2030b, 0x2038f, 0x20397, 0x203bf, 0x203c5, 0x203ef, 0x20444, 0x20453, 0x20464,
    0x20475, 0x204af, 0x20568, 0x2057b, 0x20588, 0x205cf, 0x205d8, 0x20601, 0x2061f, 0x207d3, 0x207e4, 0x207ff,
    0x208d3, 0x208e4, 0x208ff, 0x20a0a, 0x20a7b, 0x20a8a, 0x20b6b, 0x20b7a, 0x20bbb, 0x20bca, 0x20beb, 0x20bfa,
    0x20c0b, 0x20c1a, 0x20d0f, 0x2103b, 0x2104f, 0x2109b, 0x210af, 0x2116a, 0x2117f, 0x2212a, 0x2214f, 0x22ef6,
    0x22f0f, 0x23083, 0x23094, 0x230a3, 0x230b4, 0x230cf, 0x231ae, 0x231cf, 0x23293, 0x232a4, 0x232bf, 0x23f0e,
    0x23f4f, 0x2600e, 0x2604f, 0x2614e, 0x2616f, 0x2618e, 0x2619f, 0x261ae, 0x2620f, 0x2639e, 0x263cf, 0x2668e,
    0x2669f, 0x267fe, 0x2680f, 0x26bde, 0x26c9f, 0x26cde, 0x26cef, 0x26cfe, 0x26d2f, 0x26d3e, 0x26d5f, 0x26d8e,
    0x26daf, 0x26dce, 0x26ddf, 0x26dfe, 0x26e2f, 0x26eae, 0x26ebf, 0x26f1e, 0x26f6f, 0x26f7e, 0x26fbf, 0x26fde,
    0x2705f, 0x2708e, 0x270ef, 0x275b7, 0x2761f, 0x27624, 0x2764e, 0x2765f, 0x27683, 0x27694, 0x276a3, 0x276b4,
    0x276c3, 0x276d4, 0x276e3, 0x276f4, 0x27703, 0x27714, 0x27723, 0x27734, 0x27743, 0x27754, 0x2776f, 0x27c53,
    0x27c64, 0x27c7f, 0x27e63, 0x27e74, 0x27e83, 0x27e94, 0x27ea3, 0x27eb4, 0x27ec3, 0x27ed4, 0x27ee3, 0x27ef4,
    0x27f0f, 0x28008, 0x2801f, 0x29833, 0x29844, 0x29853, 0x29864, 0x29873, 0x29884, 0x29893, 0x298a4, 0x298b3,
    0x298c4, 0x298d3, 0x298e4, 0x298f3, 0x29904, 0x29913, 0x29924, 0x29933, 0x29944, 0x29953, 0x29964, 0x29973,
    0x29984, 0x2999f, 0x29d83, 0x29d94, 0x29da3, 0x29db4, 0x29dcf, 0x29fc3, 0x29fd4, 0x29fef, 0x2e007, 0x2e0e8,
    0x2e16f, 0x2e183, 0x2e198, 0x2e1af, 0x2e1c7, 0x2e1ef, 0x2e207, 0x2e223, 0x2e234, 0x2e243, 0x2e254, 0x2e263,
    0x2e274, 0x2e283, 0x2e294, 0x2e2a8, 0x2e2e4, 0x2e2ff, 0x2e308, 0x2e32f, 0x2e338, 0x2e35f, 0x2e3ae, 0x2e3c8,
    0x2e3ff, 0x2e418, 0x2e423, 0x2e438, 0x2e4bf, 0x2e4c8, 0x2e4df, 0x2e4e8, 0x2e50f, 0x2e534, 0x2e553, 0x2e564,
    0x2e573, 0x2e584, 0x2e593, 0x2e5a4, 0x2e5b3, 0x2e5c4, 0x2e5df, 0x30008, 0x30014, 0x3003f, 0x30055, 0x3006f,
    0x30083, 0x30094, 0x300a3, 0x300b4, 0x300c3, 0x300d4, 0x300e3, 0x300f4, 0x30103, 0x30114, 0x3012f, 0x30143,
    0x30154, 0x30163, 0x30174, 0x30183, 0x30194, 0x301a3, 0x301b4, 0x301c5, 0x301d3, 0x301e4, 0x3020f, 0x302ad,
    0x3030f, 0x3035d, 0x3036f, 0x303b5, 0x303df, 0x3099d, 0x309b5, 0x309ff, 0x30a05, 0x30a1f, 0x30fb5, 0x30fcf,
    0x30fd5, 0x30fff, 0x3248d, 0x3250f, 0xa0155, 0xa016f, 0xa4fe8, 0xa500f, 0xa60d8, 0xa60e4, 0xa60f8, 0xa610f,
    0xa620c, 0xa62af, 0xa6f38, 0xa6f8f, 0xa838b, 0xa839f, 0xa8749, 0xa8764, 0xa878f, 0xa8ce8, 0xa8d0c, 0xa8daf,
    0xa8fc9, 0xa8fdf, 0xa900c, 0xa90af, 0xa92e8, 0xa930f, 0xa960e, 0xa97df, 0xa9c1e, 0xa9c78, 0xa9cae, 0xa9cef,
    0xa9cf8, 0xa9d0f, 0xa9dee, 0xa9e0f, 0xa9f0c, 0xa9faf, 0xaa408, 0xaa43f, 0xaa448, 0xaa4cf, 0xaa5ce, 0xaa5d8,
    0xaa60f, 0xaaf08, 0xaaf2f, 0xabeb8, 0xabecf, 0xabf0c, 0xabfaf, 0xd7b0e, 0xd7c7f, 0xd7cbe, 0xd7fcf, 0xfd3e4,
    0xfd3f3, 0xfd40f, 0xfdfcb, 0xfdfdf, 0xfe104, 0xfe135, 0xfe154, 0xfe173, 0xfe184, 0xfe196, 0xfe1af, 0xfe201,
    0xfe21f, 0xfe221, 0xfe23f, 0xfe241, 0xfe25f, 0xfe261, 0xfe28f, 0xfe291, 0xfe2af, 0xfe2b1, 0xfe2cf, 0xfe2d1,
    0xfe2ff, 0xfe353, 0xfe364, 0xfe373, 0xfe384, 0xfe393, 0xfe3a4, 0xfe3b3, 0xfe3c4, 0xfe3d3, 0xfe3e4, 0xfe3f3,
    0xfe404, 0xfe413, 0xfe424, 0xfe433, 0xfe444, 0xfe45f, 0xfe473, 0xfe484, 0xfe49f, 0xfe504, 0xfe51e, 0xfe524,
    0xfe53f, 0xfe545, 0xfe564, 0xfe58e, 0xfe593, 0xfe5a4, 0xfe5b3, 0xfe5c4, 0xfe5d3, 0xfe5e4, 0xfe5fe, 0xfe67f,
    0xfe68e, 0xfe69a, 0xfe6ab, 0xfe6be, 0xfe6cf, 0xfeff1, 0xff00f, 0xff014, 0xff02f, 0xff04a, 0xff05b, 0xff06f,
    0xff083, 0xff094, 0xff0af, 0xff0c4, 0xff0df, 0xff0e4, 0xff0ff, 0xff1a5, 0xff1cf, 0xff1f4, 0xff20f, 0xff3b3,
    0xff3cf, 0xff3d4, 0xff3ef, 0xff5b3, 0xff5cf, 0xff5d4, 0xff5ef, 0xff5f3, 0xff604, 0xff623, 0xff634, 0xff655,
    0xff66f, 0xff9e5, 0xffa0f, 0xffe0b, 0xffe1a, 0xffe2f, 0xffe5a, 0xffe7f, 0xffe8d, 0xffeff, 0x1f100d, 0x1f1aef,
    0x1f39cd, 0x1f39ef, 0x1f3b5d, 0x1f3b7f, 0x1f3bcd, 0x1f3bdf, 0x1f4a0d, 0x1f4a1f, 0x1f4a2d, 0x1f4a3f, 0x1f4a4d, 0x1f4a5f,
    0x1f4afd, 0x1f4b0f, 0x1f4b1d, 0x1f4b3f, 0x1f500d, 0x1f507f, 0x1f517d, 0x1f525f, 0x1f532d, 0x1f54af, 0x1f5d4d, 0x1f5dcf,
    0x1f5f4d, 0x1f5faf, 0x1f650d, 0x1f6767, 0x1f6795, 0x1f67cd, 0x1f680f, 0x1f700d, 0x1f774f, 0x1f777d, 0x1f77bf, 0x1f780d,
    0x1f7d5f, 0x1f800d, 0x1f80cf, 0x1f810d, 0x1f848f, 0x1f850d, 0x1f85af, 0x1f860d, 0x1f888f, 0x1f890d, 0x1f8aef, 0x1f8b0d,
    0x1f8bcf, 0x1f8c0d, 0x1f8c2f, 0x1f8d0d, 0x1f8d9f, 0x1f900d, 0x1f90cf, 0x1fa00d, 0x1fa58f,
];

/// Whether a character's script has no spaces between words: Thai, Lao, Myanmar (and its
/// extensions), Khmer, and their symbols.
pub(crate) fn unspaced(letter: char) -> bool {
    matches!(letter as u32, 0xe00..=0xeff | 0x1000..=0x109f | 0x1780..=0x17ff | 0x19e0..=0x19ff | 0xa9e0..=0xa9ff | 0xaa60..=0xaa7f)
}

/// Whether a line may break before `letters[n]`: not after a Thai or Lao leading vowel (เ แ โ ใ
/// ไ), not before a following vowel or sign (sara a, sara aa, sara am, lakkhangyao, mai yamok,
/// paiyannoi), not before a Burmese consonant with asat or a stacked one.
pub(crate) fn between(letters: &[char], n: usize) -> bool {
    let (Some(before), Some(after)) = (n.checked_sub(1).and_then(|before| letters.get(before)), letters.get(n)) else { return true };
    let next = letters.get(n + 1).map_or(0, |next| *next as u32);
    let leading = matches!(*before as u32, 0xe40..=0xe44 | 0xec0..=0xec4);
    let following = matches!(*after as u32, 0xe2f | 0xe30 | 0xe32 | 0xe33 | 0xe45 | 0xe46 | 0xeaf | 0xeb0 | 0xeb2 | 0xeb3 | 0xec6);
    let ending = matches!(*after as u32, 0x1000..=0x1021) && matches!(next, 0x103a | 0x1039);
    !(leading || following || ending)
}

/// Character indices where a line may break between words, from ICU's word lists (see
/// `word_breaks.rs`): where no system dictionary is asked.
#[cfg(all(feature = "word-lists", not(any(windows, target_vendor = "apple"))))]
pub(crate) use crate::word_breaks::words;

/// None without `word-lists`: lines break between syllables.
#[cfg(not(any(windows, target_vendor = "apple", feature = "word-lists")))]
pub(crate) fn words(_text: &str) -> Vec<usize> {
    Vec::new()
}

/// `CFRange`: a start and a length in UTF-16 units.
#[cfg(target_vendor = "apple")]
#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    location: isize,
    length: isize,
}

#[cfg(target_vendor = "apple")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringTokenizerCreate(allocator: *const std::ffi::c_void, text: *const std::ffi::c_void, range: Range, unit: usize, locale: *const std::ffi::c_void) -> *const std::ffi::c_void;
    fn CFStringTokenizerAdvanceToNextToken(tokenizer: *const std::ffi::c_void) -> usize;
    fn CFStringTokenizerGetCurrentTokenRange(tokenizer: *const std::ffi::c_void) -> Range;
}

/// `kCFStringTokenizerUnitWordBoundary`: tokens from word boundary to word boundary.
#[cfg(target_vendor = "apple")]
const WORD_BOUNDARIES: usize = 4;

/// Character indices where a line may break between words, as CoreFoundation says: where its
/// tokenizer's tokens between word boundaries start.
#[cfg(target_vendor = "apple")]
pub(crate) fn words(text: &str) -> Vec<usize> {
    use crate::macos_sys::{CFRelease, CFStringCreateWithBytes, UTF8};
    let units = text.encode_utf16().count() as isize;
    let mut starts = Vec::new();
    // SAFETY: the string is made from the text's bytes and released, as is the tokenizer made
    // over all of it.
    unsafe {
        let string = CFStringCreateWithBytes(std::ptr::null(), text.as_ptr(), text.len() as isize, UTF8, 0);
        if string.is_null() {
            return Vec::new();
        }
        let tokenizer = CFStringTokenizerCreate(std::ptr::null(), string, Range { location: 0, length: units }, WORD_BOUNDARIES, std::ptr::null());
        while !tokenizer.is_null() && CFStringTokenizerAdvanceToNextToken(tokenizer) != 0 {
            starts.push(CFStringTokenizerGetCurrentTokenRange(tokenizer).location.max(0) as usize);
        }
        if !tokenizer.is_null() {
            CFRelease(tokenizer);
        }
        CFRelease(string);
    }
    between_words(text, |unit| starts.binary_search(&unit).is_ok())
}

/// The character indices between two letters of unspaced scripts, not inside a syllable (see
/// `between`), where `starts` holds for the UTF-16 unit there: a word starts.
#[cfg(any(windows, target_vendor = "apple"))]
fn between_words(text: &str, starts: impl Fn(usize) -> bool) -> Vec<usize> {
    let letters: Vec<char> = text.chars().collect();
    let mut chances = Vec::new();
    let mut unit = 0;
    for (n, letter) in letters.iter().enumerate() {
        if n > 0 && unspaced(*letter) && unspaced(letters[n - 1]) && starts(unit) && between(&letters, n) {
            chances.push(n);
        }
        unit += letter.len_utf16();
    }
    chances
}

/// `SCRIPT_ITEM` (usp10.h): start in UTF-16 units, and the analysis, only passed back.
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Item {
    start: i32,
    analysis: [u16; 2],
}

#[cfg(windows)]
#[link(name = "usp10")]
unsafe extern "system" {
    fn ScriptItemize(text: *const u16, count: i32, most: i32, control: *const std::ffi::c_void, state: *const std::ffi::c_void, items: *mut Item, found: *mut i32) -> i32;
    fn ScriptBreak(text: *const u16, count: i32, analysis: *const [u16; 2], attributes: *mut u8) -> i32;
}

/// Character indices where a line may break between words, as Uniscribe says: it splits the
/// text into items and flags each UTF-16 unit that may start a line (`SCRIPT_LOGATTR` bit 0).
#[cfg(windows)]
pub(crate) fn words(text: &str) -> Vec<usize> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut items = vec![Item::default(); units.len() + 2];
    let mut found = 0;
    // SAFETY: the text is `units.len()` units long; there is room for one item more than the
    // most asked for, as Uniscribe requires; it writes the count into `found`.
    let itemized = unsafe { ScriptItemize(units.as_ptr(), units.len() as i32, units.len() as i32 + 1, std::ptr::null(), std::ptr::null(), items.as_mut_ptr(), &mut found) };
    if itemized != 0 || units.is_empty() {
        return Vec::new();
    }
    let mut attributes = vec![0u8; units.len()];
    for item in 0..(found.max(0) as usize).min(units.len()) {
        let (from, to) = (items[item].start.max(0) as usize, (items[item + 1].start.max(0) as usize).min(units.len()));
        if from >= to {
            continue;
        }
        // SAFETY: the item's units lie in the text, and there is an attribute for each.
        unsafe { ScriptBreak(units[from..].as_ptr(), (to - from) as i32, &items[item].analysis, attributes[from..].as_mut_ptr()) };
    }
    between_words(text, |unit| attributes.get(unit).is_some_and(|attribute| attribute & 1 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `text` may break.
    fn chances(text: &str) -> Vec<usize> {
        let mut found = Vec::new();
        break_chances(text, &mut found);
        found
    }

    #[test]
    fn cjk_text_breaks_between_ideographs_but_not_before_closing_marks_or_after_opening_ones() {
        assert_eq!(chances("あい。「う」"), [3, 9]);
        // Small kana and the long vowel mark may start a line, as CSS `line-break: normal` has it.
        assert_eq!(chances("ちょっとー"), [3, 6, 9, 12]);
    }

    #[test]
    fn letters_keep_to_narrow_brackets_beside_them_but_not_to_fullwidth_ones() {
        assert_eq!(chances("f(x)あ"), [4]);
        assert_eq!(chances("(s)he"), []);
        assert_eq!(chances("A（注）"), [1]);
    }

    #[test]
    #[cfg(feature = "brahmic")]
    fn syllables_of_thai_lao_and_burmese_are_not_broken() {
        let letters = |text: &str| text.chars().collect::<Vec<_>>();
        // เป็น: not after เ. น้ำ: not before the sara am. ไทย: before ไ, yes.
        let thai = letters("\u{e40}\u{e1b}\u{e47}\u{e19}\u{e19}\u{e49}\u{e33}\u{e44}\u{e17}\u{e22}");
        assert_eq!((1..thai.len()).filter(|n| between(&thai, *n)).collect::<Vec<_>>(), [2, 3, 4, 5, 7, 9]);
        // မင်း: not before င, which has asat over it.
        let burmese = letters("\u{1019}\u{1004}\u{103a}\u{1038}");
        assert!(!between(&burmese, 1) && between(&burmese, 2));
    }

    #[test]
    #[cfg(all(windows, feature = "brahmic"))]
    fn windows_says_where_the_words_of_thai_end() {
        // ภาษาไทย ("the Thai language"): two words, the second from the fifth character. (From
        // Windows' dictionary: without it nothing is found, which is fine.)
        let found = words("\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22}");
        assert!(found.is_empty() || found == [4], "{found:?}");
        println!("Windows breaks ภาษาไทย before {found:?}");
    }

    #[test]
    #[cfg(any(all(target_vendor = "apple", feature = "brahmic"), all(not(windows), feature = "word-lists")))]
    fn words_of_thai_lao_burmese_and_khmer_end_where_the_dictionary_ends_them() {
        // As ICU 78 splits them, from its word lists (Linux, Android, the web). Apple's systems
        // say with dictionaries of their own (ภาษาไทย is one word there): each sentence breaks
        // between words, never inside a syllable.
        let split = |text: &str| {
            let letters: Vec<char> = text.chars().collect();
            let starts: Vec<usize> = [0].into_iter().chain(words(text)).chain([letters.len()]).collect();
            starts.windows(2).map(|pair| letters[pair[0]..pair[1]].iter().collect::<String>()).collect::<Vec<_>>()
        };
        let all = [
            ("ภาษาไทยเป็นภาษาที่สวยงาม", &["ภาษา", "ไทย", "เป็น", "ภาษา", "ที่", "สวยงาม"][..]),
            ("เด็กๆเล่นกันอยู่ในสวนฯลฯ", &["เด็กๆ", "เล่น", "กัน", "อยู่", "ใน", "สวน", "ฯลฯ"]),
            ("ພາສາລາວເປັນພາສາທີ່ສວຍງາມ", &["ພາສາ", "ລາວ", "ເປັນ", "ພາສາ", "ທີ່", "ສວຍງາມ"]),
            ("အပြည်ပြည်ဆိုင်ရာလူ့အခွင့်အရေးကြေညာစာတမ်း", &["အပြည်", "ပြည်", "ဆိုင်ရာ", "လူ့", "အခွင့်အရေး", "ကြေညာစာတမ်း"]),
            ("ប្រទេសកម្ពុជាមានប្រជាជនច្រើន", &["ប្រទេស", "កម្ពុជា", "មាន", "ប្រជាជន", "ច្រើន"]),
        ];
        for (text, words_of_icu) in all {
            let found = split(text);
            println!("{text}: {found:?}");
            if cfg!(target_vendor = "apple") {
                let letters: Vec<char> = text.chars().collect();
                let breaks = words(text);
                assert!(breaks.len() >= 2 && breaks.iter().all(|at| between(&letters, *at)), "{text}: {found:?}");
            } else {
                assert_eq!(found, words_of_icu, "{text}");
            }
        }
    }
}
