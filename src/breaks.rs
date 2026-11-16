//! Line breaks in text with no spaces between words: Thai, Lao, Khmer and Burmese (Unicode line
//! breaking's "complex context").
//!
//! Word boundaries there need a dictionary per language, too large to carry for text a program
//! may never get. So:
//!
//! - Windows: Uniscribe's `ScriptBreak` (it has the dictionaries) breaks between words.
//! - Elsewhere no system service does (ICU and libthai are not always installed). A line breaks
//!   between syllables: between grapheme clusters (which keep Khmer and Burmese stacked
//!   consonants together), but never after a Thai or Lao vowel written before its consonant,
//!   before one of their vowels or signs that follows it, or before a Burmese final consonant
//!   (with asat, or stacked).
//!
//! Feature `brahmic`.

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

/// Character indices where a line may break between words, as the system says. Empty where it
/// cannot be asked.
#[cfg(not(windows))]
pub(crate) fn words(_text: &str) -> Vec<usize> {
    Vec::new()
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
    let letters: Vec<char> = text.chars().collect();
    let mut chances = Vec::new();
    let mut unit = 0;
    for (n, letter) in letters.iter().enumerate() {
        let between_words = n > 0 && unspaced(*letter) && unspaced(letters[n - 1]);
        if between_words && attributes.get(unit).is_some_and(|attribute| attribute & 1 != 0) {
            chances.push(n);
        }
        unit += letter.len_utf16();
    }
    chances
}

#[cfg(test)]
#[cfg(feature = "brahmic")]
mod tests {
    use super::*;

    #[test]
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
    #[cfg(windows)]
    fn windows_says_where_the_words_of_thai_end() {
        // ภาษาไทย ("the Thai language"): two words, the second from the fifth character. (From
        // Windows' dictionary: without it nothing is found, which is fine.)
        let found = words("\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22}");
        assert!(found.is_empty() || found == [4], "{found:?}");
        println!("Windows breaks ภาษาไทย before {found:?}");
    }
}
