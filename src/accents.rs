//! Unicode normalization for shaping: a letter and its marks composed into the one character a
//! font has for them, and a character the font lacks decomposed into a letter and the marks it
//! has, as HarfBuzz does before lookups (`hb-ot-shape-normalize.cc`), for every script
//! `shape.rs` shapes. (Indic and Southeast Asian scripts do this their own way, in
//! `brahmic.rs`.)
//!
//! Unicode defines decompositions ("é" is "e" plus a combining acute; a Hangul syllable is its
//! jamo; a few characters are singletons, as most CJK compatibility ideographs) and the order
//! of marks after a letter by combining class (below before above, nearer before further). Text
//! goes through three passes: each letter with its marks is decomposed as far as the font has
//! the parts (a lone letter only if the font lacks it); each mark run is sorted by class; then
//! each mark composes with the preceding letter wherever the font has the pair as one and
//! nothing between them blocks it.
//!
//! A font lacking a sized space (em space, thin space) draws it with its own space, widened
//! later; lacking the non-breaking hyphen, with the plain hyphen.
//!
//! The tables are Unicode 17's, generated from `UnicodeData.txt` and
//! `CompositionExclusions.txt`: about 22 KB, only in programs that shape text with fonts
//! (right-to-left text, pages), not in ones that only draw emoji. Combining classes are
//! HarfBuzz's, which reorder a few Hebrew, Arabic, Telugu, Thai, Lao and Tibetan marks
//! differently from Unicode.
use crate::font::FontFile;

/// How a script composes and decomposes, as its HarfBuzz shaper does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Way {
    /// Decompose where the font lacks a character, compose where it has the whole.
    Usual,
    /// As usual, and among Arabic marks above or below, those that modify the letter itself
    /// (hamza, small high seen) first.
    Arabic,
    /// As usual, and patah or qamats before sheva or hiriq and meteg kept apart; and in a font
    /// without mark positioning, letters with points composed into the presentation forms older
    /// fonts have, which Unicode keeps decomposed.
    Hebrew { forms: bool },
    /// Hangul, as HarfBuzz's Hangul shaper passes it on (see `hangul`): jamo composed into
    /// syllables and syllables decomposed into jamo, as the font has them; otherwise only what
    /// the font lacks is decomposed, nothing composed.
    Hangul,
}

/// More marks than this after one letter keep their order, as in HarfBuzz: sorting them is
/// quadratic.
const MOST_MARKS: usize = 32;

/// A character's mark data: whether it is a mark (none, nonspacing, spacing or enclosing), and
/// its combining class.
fn found(letter: char) -> u32 {
    let code = letter as u32;
    let n = CLASSES.partition_point(|stretch| stretch >> 10 <= code);
    n.checked_sub(1).map_or(0, |n| CLASSES[n] & 0x3ff)
}

/// A mark's combining class: Unicode's, as HarfBuzz modifies a few; 0 for non-marks.
pub(crate) fn class(letter: char) -> u8 {
    found(letter) as u8
}

/// Whether a character is a mark (general category M: nonspacing, spacing or enclosing).
pub(crate) fn mark(letter: char) -> bool {
    found(letter) >> 8 != 0
}

/// The first Hangul syllable, the first jamo of each kind, and their counts.
const SYLLABLES: u32 = 0xac00;
const LEADING: u32 = 0x1100;
const VOWELS: u32 = 0x1161;
const TRAILING: u32 = 0x11a7;
const VOWEL_COUNT: u32 = 21;
const TRAILING_COUNT: u32 = 28;
const SYLLABLE_COUNT: u32 = 11_172;

/// A character's canonical decomposition: two characters (usually a letter and a mark), or one
/// (the second `None`).
pub(crate) fn split(letter: char) -> Option<(char, Option<char>)> {
    let code = letter as u32;
    // A Hangul syllable is its leading consonant and vowel, plus a trailing consonant.
    if let Some(syllable) = code.checked_sub(SYLLABLES).filter(|syllable| *syllable < SYLLABLE_COUNT) {
        let (first, second) = match syllable % TRAILING_COUNT {
            0 => (LEADING + syllable / (VOWEL_COUNT * TRAILING_COUNT), VOWELS + syllable % (VOWEL_COUNT * TRAILING_COUNT) / TRAILING_COUNT),
            trailing => (code - trailing, TRAILING + trailing),
        };
        return Some((char::from_u32(first)?, char::from_u32(second)));
    }
    let n = SPLITS.binary_search_by_key(&code, |split| (split >> 42) as u32).ok()?;
    let (first, second) = ((SPLITS[n] >> 21) as u32 & 0x1f_ffff, SPLITS[n] as u32 & 0x1f_ffff);
    Some((char::from_u32(first)?, char::from_u32(second).filter(|_| second != 0)))
}

/// The canonical composition of two characters, where Unicode allows it (not the composition
/// exclusions, kept apart for older encodings).
pub(crate) fn joined(first: char, second: char) -> Option<char> {
    let (one, other) = (first as u32, second as u32);
    let syllable = one.wrapping_sub(SYLLABLES);
    if syllable < SYLLABLE_COUNT && syllable % TRAILING_COUNT == 0 && other > TRAILING && other < TRAILING + TRAILING_COUNT {
        return char::from_u32(one + other - TRAILING);
    }
    if (LEADING..LEADING + 19).contains(&one) && (VOWELS..VOWELS + VOWEL_COUNT).contains(&other) {
        return char::from_u32(SYLLABLES + ((one - LEADING) * VOWEL_COUNT + other - VOWELS) * TRAILING_COUNT);
    }
    let parts = |n: &u16| (SPLITS[*n as usize] >> 21 & 0x1f_ffff, SPLITS[*n as usize] & 0x1f_ffff);
    let n = JOINS.binary_search_by(|n| parts(n).cmp(&(one as u64, other as u64))).ok()?;
    char::from_u32((SPLITS[JOINS[n] as usize] >> 42) as u32)
}

/// Hebrew presentation forms with points that older fonts have and Unicode keeps decomposed:
/// each letter and point with its form (HarfBuzz uses them for fonts without mark positioning).
#[rustfmt::skip]
const HEBREW: [(u16, u16, u16); 37] = [
    (0x5d9, 0x5b4, 0xfb1d), (0x5f2, 0x5b7, 0xfb1f), (0x5d0, 0x5b7, 0xfb2e), (0x5d0, 0x5b8, 0xfb2f), (0x5d5, 0x5b9, 0xfb4b),
    (0x5d0, 0x5bc, 0xfb30), (0x5d1, 0x5bc, 0xfb31), (0x5d2, 0x5bc, 0xfb32), (0x5d3, 0x5bc, 0xfb33), (0x5d4, 0x5bc, 0xfb34),
    (0x5d5, 0x5bc, 0xfb35), (0x5d6, 0x5bc, 0xfb36), (0x5d8, 0x5bc, 0xfb38), (0x5d9, 0x5bc, 0xfb39), (0x5da, 0x5bc, 0xfb3a),
    (0x5db, 0x5bc, 0xfb3b), (0x5dc, 0x5bc, 0xfb3c), (0x5de, 0x5bc, 0xfb3e), (0x5e0, 0x5bc, 0xfb40), (0x5e1, 0x5bc, 0xfb41),
    (0x5e3, 0x5bc, 0xfb43), (0x5e4, 0x5bc, 0xfb44), (0x5e6, 0x5bc, 0xfb46), (0x5e7, 0x5bc, 0xfb47), (0x5e8, 0x5bc, 0xfb48),
    (0x5e9, 0x5bc, 0xfb49), (0x5ea, 0x5bc, 0xfb4a), (0xfb2a, 0x5bc, 0xfb2c), (0xfb2b, 0x5bc, 0xfb2d), (0x5d1, 0x5bf, 0xfb4c),
    (0x5db, 0x5bf, 0xfb4d), (0x5e4, 0x5bf, 0xfb4e), (0x5e9, 0x5c1, 0xfb2a), (0xfb49, 0x5c1, 0xfb2c), (0x5e9, 0x5c2, 0xfb2b),
    (0xfb49, 0x5c2, 0xfb2d), (0, 0, 0),
];

/// The composition of two characters, under a script's rules.
fn put_together(way: Way, first: char, second: char) -> Option<char> {
    let whole = joined(first, second);
    if whole.is_some() || way != (Way::Hebrew { forms: true }) {
        return whole;
    }
    let found = HEBREW.iter().find(|(one, other, _)| *one as u32 == first as u32 && *other as u32 == second as u32 && *one != 0);
    found.and_then(|(.., form)| char::from_u32(*form as u32))
}

/// Arabic marks that modify their letter rather than its sound, which go nearest it.
const CHANGING: [u16; 14] = [0x654, 0x655, 0x658, 0x6dc, 0x6e3, 0x6e7, 0x6e8, 0x8ca, 0x8cb, 0x8cd, 0x8ce, 0x8cf, 0x8d3, 0x8f3];

/// Text as the font is to get it: each character (with its text position) decomposed or
/// composed as the font has it (see above); and the positions of combining grapheme joiners
/// (U+034F) the font's lookups skip, as HarfBuzz does: each that separates no marks that would
/// have been reordered (one before a class-0 character, or between marks already in order).
/// `variants` are the characters the font has its own glyph for with the variation selector
/// after them (by position): those, and a character followed by a selector the font didn't map,
/// are left as is, with their marks; one that composes loses its own glyph, as in HarfBuzz.
pub(crate) fn normalized(font: &FontFile, letters: &[(char, u32)], variants: &mut Vec<(u32, u16)>, way: Way) -> (Vec<(char, u32)>, Vec<u32>) {
    let syllables = if way == Way::Hangul { hangul(font, letters) } else { Vec::new() };
    let letters = if way == Way::Hangul { &syllables[..] } else { letters };
    let mut out: Vec<(char, u32)> = Vec::with_capacity(letters.len() + 8);
    let (mut simple, mut at) = (true, 0);
    let has = |letter: char| font.glyph(letter) != 0;
    while at < letters.len() {
        // A letter and its marks (or marks with no letter before).
        let end = at + 1 + letters[at + 1..].iter().take_while(|(letter, _)| mark(*letter)).count();
        let cluster = &letters[at..end];
        let selected = variants.iter().any(|(from, _)| *from == letters[at].1) || cluster.iter().any(|(letter, _)| selector(*letter));
        simple &= end == at + 1;
        if selected {
            out.extend_from_slice(cluster);
        } else {
            // A lone letter is decomposed only if the font lacks it; one with marks, as far as
            // the font has the parts, to be recomposed where it has the whole.
            let shortest = end == at + 1 || way == Way::Hangul;
            cluster.iter().for_each(|(letter, from)| taken_apart(&has, *letter, shortest, |part| out.push((part, *from))));
        }
        at = end;
    }
    let mut classes: Vec<u8> = out.iter().map(|(letter, _)| class(*letter)).collect();
    if !simple {
        in_order(&mut out, &mut classes, way);
    }
    let passed = (1..out.len().saturating_sub(1)).filter(|n| out[*n].0 == '\u{34f}' && (classes[n + 1] == 0 || classes[n - 1] <= classes[n + 1]));
    let passed = passed.map(|n| out[n].1).collect();
    if simple || way == Way::Hangul {
        return (out, passed);
    }
    (put_together_where_the_font_has_them(font, out, classes, variants, way), passed)
}

/// Hangul as HarfBuzz's Hangul shaper passes it on, first: a syllable's jamo (leading, vowel,
/// optional trailing) composed where the font has the syllable; a syllable the font lacks, or
/// one followed by a trailing jamo it doesn't have combined, decomposed where the font has all
/// its jamo; and a tone mark moved before the syllable it follows (unless nonspacing, drawn
/// over it), or given a dotted circle when it follows none.
fn hangul(font: &FontFile, letters: &[(char, u32)]) -> Vec<(char, u32)> {
    let has = |code: u32| char::from_u32(code).is_some_and(|letter| font.glyph(letter) != 0);
    let leading = |code: u32| matches!(code, 0x1100..=0x115f | 0xa960..=0xa97c);
    let vowel = |code: u32| matches!(code, 0x1160..=0x11a7 | 0xd7b0..=0xd7c6);
    let trailing = |code: u32| matches!(code, 0x11a8..=0x11ff | 0xd7cb..=0xd7fb);
    let joins = |code: u32| (TRAILING + 1..TRAILING + TRAILING_COUNT).contains(&code);
    let mut out: Vec<(char, u32)> = Vec::with_capacity(letters.len() + 4);
    // Where the last syllable is in the output, and where it ends.
    let (mut start, mut end, mut n) = (0, 0, 0);
    while n < letters.len() {
        let (letter, from) = letters[n];
        let code = letter as u32;
        let after = |k: usize| letters.get(n + k).map_or(0, |(letter, _)| *letter as u32);
        let (one, two) = (after(1), after(2));
        if matches!(code, 0x302e | 0x302f) {
            tone(font, &mut out, (letter, from), [start, end]);
            (start, end, n) = (out.len(), out.len(), n + 1);
            continue;
        }
        start = out.len();
        let syllable = code.wrapping_sub(SYLLABLES);
        // What the character (and the jamo it absorbs) becomes, and whether that is a syllable
        // (for a following tone mark).
        let (parts, whole): (Vec<u32>, bool) = match () {
            _ if leading(code) && vowel(one) => {
                let t = if trailing(two) { two } else { 0 };
                let made = SYLLABLES + ((code.wrapping_sub(LEADING) * VOWEL_COUNT + one.wrapping_sub(VOWELS)) * TRAILING_COUNT) + t.saturating_sub(TRAILING);
                let fits = (LEADING..LEADING + 19).contains(&code) && (VOWELS..VOWELS + VOWEL_COUNT).contains(&one) && (t == 0 || joins(t));
                n += 2 + (t != 0) as usize;
                let letters = [code, one, t].into_iter().filter(|part| *part != 0).collect();
                (if fits && has(made) { vec![made] } else { letters }, true)
            }
            _ if syllable < SYLLABLE_COUNT && syllable.is_multiple_of(TRAILING_COUNT) && joins(one) && has(code + one - TRAILING) => {
                n += 2;
                (vec![code + one - TRAILING], true)
            }
            _ if syllable < SYLLABLE_COUNT && (!has(code) || (syllable.is_multiple_of(TRAILING_COUNT) && trailing(one))) => {
                let t = syllable % TRAILING_COUNT;
                let split = [LEADING + syllable / (VOWEL_COUNT * TRAILING_COUNT), VOWELS + syllable % (VOWEL_COUNT * TRAILING_COUNT) / TRAILING_COUNT, TRAILING + t];
                let apart = has(split[0]) && has(split[1]) && (t == 0 || has(split[2]));
                // A syllable the font has, decomposed for a following trailing jamo, takes that
                // jamo along as is.
                let along = (apart && has(code) && t == 0) as usize;
                n += 1 + along;
                let parts = split.into_iter().take(2 + (t != 0) as usize).chain((along == 1).then_some(one)).collect();
                if apart { (parts, true) } else { (vec![code], has(code)) }
            }
            _ => {
                n += 1;
                (vec![code], syllable < SYLLABLE_COUNT && has(code))
            }
        };
        out.extend(parts.iter().filter_map(|part| char::from_u32(*part)).map(|part| (part, from)));
        end = if whole { out.len() } else { start };
    }
    out
}

/// Places a Hangul tone mark as HarfBuzz does: right after the syllable it follows (output
/// `start` to `end`) it moves before that syllable, unless nonspacing (a glyph drawn over the
/// syllable); after no syllable, a dotted circle goes with it, before it or (if nonspacing)
/// after it.
fn tone(font: &FontFile, out: &mut Vec<(char, u32)>, (letter, from): (char, u32), [start, end]: [usize; 2]) {
    let glyph = font.glyph(letter);
    let narrow = glyph != 0 && font.width(glyph) == 0.0;
    if start < end && end == out.len() {
        out.push((letter, from));
        if !narrow {
            out[start..].rotate_right(1);
        }
        return;
    }
    match (font.glyph('\u{25cc}') != 0, narrow) {
        (false, _) => out.push((letter, from)),
        (true, false) => out.extend([(letter, from), ('\u{25cc}', from)]),
        (true, true) => out.extend([('\u{25cc}', from), (letter, from)]),
    }
}

/// Whether a character is a variation selector (which HarfBuzz leaves to the font's lookups).
fn selector(letter: char) -> bool {
    matches!(letter, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
}

/// Emits a character's decomposition as far as the font `has` the parts: the deepest
/// decomposition whose second parts the font all has, and whose first it has (or, `shortest`,
/// the first such, where the font has the first part). The character itself if none. (What
/// happens to one the font lacks after all is `shape.rs`'s concern: a space as the font's own,
/// say; see `space`.)
fn taken_apart(has: &impl Fn(char) -> bool, letter: char, shortest: bool, mut part: impl FnMut(char)) {
    if shortest && has(letter) {
        return part(letter);
    }
    // Its decompositions, each a first part and an optional second, from the whole down; and
    // for each, whether decomposing from there works.
    let mut chain: Vec<(char, Option<char>)> = Vec::new();
    let mut whole = letter;
    while let Some(parts) = split(whole).filter(|_| chain.len() < 8) {
        chain.push(parts);
        whole = parts.0;
    }
    let mut works = vec![false; chain.len() + 1];
    for level in (0..chain.len()).rev() {
        let (first, second) = chain[level];
        works[level] = second.is_none_or(has) && (has(first) || works[level + 1]);
    }
    if works[0] {
        let mut depth = 0;
        while !(shortest && has(chain[depth].0)) && works[depth + 1] {
            depth += 1;
        }
        part(chain[depth].0);
        chain[..=depth].iter().rev().filter_map(|(_, second)| *second).for_each(&mut part);
        return;
    }
    part(letter)
}

/// A space's kind, where a font lacking it draws its own space widened as the kind says
/// (HarfBuzz's): 1 to 6, an em divided by that; 7, 1/16 em; 8, 4/18 em; 9, figure width; 10,
/// full stop width; 11, half the space; 12, the space as is. `None` for other characters.
pub(crate) fn space(letter: char) -> Option<u8> {
    Some(match letter {
        '\u{2001}' | '\u{2003}' | '\u{3000}' => 1,
        '\u{2000}' | '\u{2002}' => 2,
        '\u{2004}' => 3,
        '\u{2005}' => 4,
        '\u{2009}' => 5,
        '\u{2006}' => 6,
        '\u{200a}' => 7,
        '\u{205f}' => 8,
        '\u{2007}' => 9,
        '\u{2008}' => 10,
        '\u{202f}' => 11,
        '\u{20}' | '\u{a0}' => 12,
        _ => return None,
    })
}

/// Sorts each mark run after a letter by class (stable), except runs longer than `MOST_MARKS`;
/// then applies the script's extra rules.
fn in_order(letters: &mut [(char, u32)], classes: &mut [u8], way: Way) {
    let mut at = 0;
    while at < letters.len() {
        if classes[at] == 0 {
            at += 1;
            continue;
        }
        let end = at + classes[at..].iter().take_while(|class| **class != 0).count();
        if end - at <= MOST_MARKS {
            settle(&mut letters[at..end], &mut classes[at..end]);
            match way {
                Way::Arabic => changing_first(&mut letters[at..end], &mut classes[at..end]),
                Way::Hebrew { .. } => meteg_after(&mut letters[at..end], &classes[at..end]),
                _ => {}
            }
        }
        at = end;
    }
}

/// Stable insertion sort of marks by class: for a letter's few marks, all it takes.
fn settle(letters: &mut [(char, u32)], classes: &mut [u8]) {
    for n in 1..letters.len() {
        let (letter, class) = (letters[n], classes[n]);
        let mut at = n;
        while at > 0 && classes[at - 1] > class {
            (letters[at], classes[at]) = (letters[at - 1], classes[at - 1]);
            at -= 1;
        }
        (letters[at], classes[at]) = (letter, class);
    }
}

/// Among class-sorted Arabic marks, the modifying ones below (220) then above (230) go before
/// all others, and take classes that keep the run sorted (25 and 26, as HarfBuzz numbers them)
/// for what follows.
fn changing_first(marks: &mut [(char, u32)], classes: &mut [u8]) {
    let mut start = 0;
    let mut at = 0;
    for (place, renumbered) in [(220, 25), (230, 26)] {
        at += classes[at..].iter().take_while(|class| **class < place).count();
        if at == marks.len() {
            return;
        }
        if classes[at] > place {
            continue;
        }
        let changing = |n: usize| classes[n] == place && CHANGING.contains(&(marks[n].0 as u32 as u16)) && marks[n].0 < '\u{10000}';
        let end = at + (at..marks.len()).take_while(|n| changing(*n)).count();
        if end == at {
            continue;
        }
        marks[start..end].rotate_right(end - at);
        classes[start..end].rotate_right(end - at);
        classes[start..start + end - at].fill(renumbered);
        (start, at) = (start + end - at, end);
    }
}

/// Patah or qamats, then sheva or hiriq, then meteg (or any mark below): the last two swap, as
/// HarfBuzz does for Hebrew (once per run).
fn meteg_after(marks: &mut [(char, u32)], classes: &[u8]) {
    // HarfBuzz's classes for patah and qamats, sheva and hiriq, and meteg; 220 is below.
    let found = (2..marks.len()).find(|n| matches!(classes[n - 2], 20 | 21) && matches!(classes[n - 1], 22 | 23) && matches!(classes[*n], 25 | 220));
    if let Some(n) = found {
        marks.swap(n - 1, n);
    }
}

/// Composes each mark with the preceding letter where the font has the pair as one character
/// and no mark between blocks it (has a nonzero, lower class): the result then composes with
/// the following marks in turn.
fn put_together_where_the_font_has_them(font: &FontFile, letters: Vec<(char, u32)>, classes: Vec<u8>, variants: &mut Vec<(u32, u16)>, way: Way) -> Vec<(char, u32)> {
    let mut out: Vec<(char, u32)> = Vec::with_capacity(letters.len());
    let mut places: Vec<u8> = Vec::with_capacity(letters.len());
    let mut starter = 0;
    for ((letter, from), place) in letters.into_iter().zip(classes) {
        let open = !out.is_empty() && mark(letter) && (starter + 1 == out.len() || places.last().is_some_and(|last| *last < place));
        let whole = if open { put_together(way, out[starter].0, letter).filter(|whole| font.glyph(*whole) != 0) } else { None };
        if let Some(whole) = whole {
            variants.retain(|(at, _)| *at != out[starter].1);
            (out[starter].0, places[starter]) = (whole, class(whole));
            continue;
        }
        out.push((letter, from));
        places.push(place);
        if place == 0 {
            starter = out.len() - 1;
        }
    }
    out
}

/// Marks and their classes, in ranges: each range's first character (times 1024), its kind
/// (none, nonspacing, spacing or enclosing: times 256), and its class, as HarfBuzz numbers
/// them.
#[rustfmt::skip]
const CLASSES: [u32; 1145] = [
    0x0, 0xc01e6, 0xc55e8, 0xc59dc, 0xc69e8, 0xc6dd8, 0xc71dc, 0xc85ca, 0xc8ddc, 0xc9dca,
    0xca5dc, 0xcd101, 0xce5dc, 0xcf5e6, 0xd15f0, 0xd19e6, 0xd1ddc, 0xd29e6, 0xd35dc, 0xd3d00,
    0xd41e6, 0xd4ddc, 0xd5de6, 0xd61e8, 0xd65dc, 0xd6de6, 0xd71e9, 0xd75ea, 0xd7de9, 0xd81ea,
    0xd89e9, 0xd8de6, 0xdc000, 0x120de6, 0x122200, 0x122800, 0x1645dc, 0x1649e6, 0x1659dc, 0x165de6,
    0x1669de, 0x166ddc, 0x1671e6, 0x1689dc, 0x16a1e6, 0x16a9dc, 0x16ade6, 0x16b5de, 0x16b9e4, 0x16bde6,
    0x16c116, 0x16c50f, 0x16c910, 0x16cd11, 0x16d117, 0x16d512, 0x16d913, 0x16dd14, 0x16e115, 0x16e50e,
    0x16ed18, 0x16f10c, 0x16f519, 0x16f800, 0x16fd0d, 0x170000, 0x17050a, 0x17090b, 0x170c00, 0x1711e6,
    0x1715dc, 0x171800, 0x171d15, 0x172000, 0x1841e6, 0x18611f, 0x186520, 0x186921, 0x186c00, 0x192d1c,
    0x19311d, 0x19351e, 0x19391f, 0x193d20, 0x194121, 0x19451b, 0x194922, 0x194de6, 0x1955dc, 0x195de6,
    0x1971dc, 0x1975e6, 0x197ddc, 0x198000, 0x19c123, 0x19c400, 0x1b59e6, 0x1b7400, 0x1b7de6, 0x1b8ddc,
    0x1b91e6, 0x1b9400, 0x1b9de6, 0x1ba400, 0x1ba9dc, 0x1bade6, 0x1bb5dc, 0x1bb800, 0x1c4524, 0x1c4800,
    0x1cc1e6, 0x1cc5dc, 0x1cc9e6, 0x1cd1dc, 0x1cd5e6, 0x1cdddc, 0x1ce9e6, 0x1ceddc, 0x1cf5e6, 0x1cf9dc,
    0x1cfde6, 0x1d09dc, 0x1d0de6, 0x1d11dc, 0x1d15e6, 0x1d19dc, 0x1d1de6, 0x1d21dc, 0x1d25e6, 0x1d2c00,
    0x1e9900, 0x1ec400, 0x1fade6, 0x1fc9dc, 0x1fcde6, 0x1fd000, 0x1ff5dc, 0x1ff800, 0x2059e6, 0x206800,
    0x206de6, 0x209000, 0x2095e6, 0x20a000, 0x20a5e6, 0x20b800, 0x2165dc, 0x217000, 0x225de6, 0x2265dc,
    0x2271e6, 0x228000, 0x2329e6, 0x233ddc, 0x2351e6, 0x238800, 0x238ddc, 0x2391e6, 0x2399dc, 0x239de6,
    0x23a5dc, 0x23a9e6, 0x23b5dc, 0x23c11c, 0x23c51d, 0x23c91e, 0x23cde6, 0x23d9dc, 0x23dde6, 0x23e5dc,
    0x23ede6, 0x240100, 0x240e00, 0x241000, 0x24e900, 0x24ee00, 0x24f107, 0x24f400, 0x24fa00, 0x250500,
    0x252600, 0x253509, 0x253a00, 0x254000, 0x2545e6, 0x2549dc, 0x254de6, 0x255500, 0x256000, 0x258900,
    0x259000, 0x260500, 0x260a00, 0x261000, 0x26f107, 0x26f400, 0x26fa00, 0x270500, 0x271400, 0x271e00,
    0x272400, 0x272e00, 0x273509, 0x273800, 0x275e00, 0x276000, 0x278900, 0x279000, 0x27f9e6, 0x27fc00,
    0x280500, 0x280e00, 0x281000, 0x28f107, 0x28f400, 0x28fa00, 0x290500, 0x290c00, 0x291d00, 0x292400,
    0x292d00, 0x293509, 0x293800, 0x294500, 0x294800, 0x29c100, 0x29c800, 0x29d500, 0x29d800, 0x2a0500,
    0x2a0e00, 0x2a1000, 0x2af107, 0x2af400, 0x2afa00, 0x2b0500, 0x2b1800, 0x2b1d00, 0x2b2600, 0x2b2800,
    0x2b2e00, 0x2b3509, 0x2b3800, 0x2b8900, 0x2b9000, 0x2be900, 0x2c0000, 0x2c0500, 0x2c0a00, 0x2c1000,
    0x2cf107, 0x2cf400, 0x2cfa00, 0x2cfd00, 0x2d0200, 0x2d0500, 0x2d1400, 0x2d1e00, 0x2d2400, 0x2d2e00,
    0x2d3509, 0x2d3800, 0x2d5500, 0x2d5e00, 0x2d6000, 0x2d8900, 0x2d9000, 0x2e0900, 0x2e0c00, 0x2efa00,
    0x2f0100, 0x2f0600, 0x2f0c00, 0x2f1a00, 0x2f2400, 0x2f2a00, 0x2f3509, 0x2f3800, 0x2f5e00, 0x2f6000,
    0x300100, 0x300600, 0x301100, 0x301400, 0x30f107, 0x30f400, 0x30f900, 0x310600, 0x311400, 0x311900,
    0x312400, 0x312900, 0x313509, 0x313800, 0x315504, 0x315905, 0x315c00, 0x318900, 0x319000, 0x320500,
    0x320a00, 0x321000, 0x32f107, 0x32f400, 0x32fa00, 0x32fd00, 0x330200, 0x331400, 0x331900, 0x331e00,
    0x332400, 0x332a00, 0x333100, 0x333509, 0x333800, 0x335600, 0x335c00, 0x338900, 0x339000, 0x33ce00,
    0x33d000, 0x340100, 0x340a00, 0x341000, 0x34ed09, 0x34f400, 0x34fa00, 0x350500, 0x351400, 0x351a00,
    0x352400, 0x352a00, 0x353509, 0x353800, 0x355e00, 0x356000, 0x358900, 0x359000, 0x360500, 0x360a00,
    0x361000, 0x372909, 0x372c00, 0x373e00, 0x374900, 0x375400, 0x375900, 0x375c00, 0x376200, 0x378000,
    0x37ca00, 0x37d000, 0x38c500, 0x38c800, 0x38d100, 0x38e103, 0x38e909, 0x38ec00, 0x391d00, 0x39216b,
    0x393100, 0x393c00, 0x3ac500, 0x3ac800, 0x3ad100, 0x3ae176, 0x3ae909, 0x3aed00, 0x3af400, 0x3b217a,
    0x3b3100, 0x3b3c00, 0x3c61dc, 0x3c6800, 0x3cd5dc, 0x3cd800, 0x3cdddc, 0x3ce000, 0x3ce57f, 0x3ce800,
    0x3cfa00, 0x3d0000, 0x3dc581, 0x3dc984, 0x3dcd00, 0x3dd183, 0x3dd500, 0x3de984, 0x3df900, 0x3dfe00,
    0x3e0184, 0x3e0500, 0x3e09e6, 0x3e1109, 0x3e1400, 0x3e19e6, 0x3e2000, 0x3e3500, 0x3e6000, 0x3e6500,
    0x3ef400, 0x3f19fe, 0x3f1c00, 0x40ae00, 0x40b500, 0x40c600, 0x40c900, 0x40dd07, 0x40e200, 0x40e509,
    0x40ee00, 0x40f500, 0x40fc00, 0x415a00, 0x416100, 0x416800, 0x417900, 0x418400, 0x418a00, 0x419400,
    0x419e00, 0x41b800, 0x41c500, 0x41d400, 0x420900, 0x420e00, 0x421500, 0x421e00, 0x4235dc, 0x423800,
    0x423e00, 0x424000, 0x426a00, 0x427500, 0x427800, 0x4d75e6, 0x4d8000, 0x5c4900, 0x5c5109, 0x5c5609,
    0x5c5800, 0x5cc900, 0x5cd209, 0x5cd400, 0x5d4900, 0x5d5000, 0x5dc900, 0x5dd000, 0x5ed100, 0x5eda00,
    0x5edd00, 0x5efa00, 0x5f1900, 0x5f1e00, 0x5f2500, 0x5f4909, 0x5f4d00, 0x5f5000, 0x5f75e6, 0x5f7800,
    0x602d00, 0x603800, 0x603d00, 0x604000, 0x621500, 0x621c00, 0x62a5e4, 0x62a800, 0x648100, 0x648e00,
    0x649d00, 0x64a600, 0x64b000, 0x64c200, 0x64c900, 0x64ce00, 0x64e5de, 0x64e9e6, 0x64eddc, 0x64f000,
    0x685de6, 0x6861dc, 0x686600, 0x686d00, 0x687000, 0x695600, 0x695900, 0x695e00, 0x696100, 0x697c00,
    0x6981fe, 0x698600, 0x698900, 0x698e00, 0x699500, 0x69b600, 0x69cd00, 0x69d5e6, 0x69f400, 0x69fddc,
    0x6a0000, 0x6ac1e6, 0x6ad5dc, 0x6aede6, 0x6af5dc, 0x6afa00, 0x6afddc, 0x6b05e6, 0x6b0ddc, 0x6b15e6,
    0x6b29dc, 0x6b2de6, 0x6b75dc, 0x6b7800, 0x6b81e6, 0x6b99dc, 0x6b9de6, 0x6badea, 0x6bb000, 0x6c0100,
    0x6c1200, 0x6c1400, 0x6cd107, 0x6cd600, 0x6cd900, 0x6cee00, 0x6cf100, 0x6cf600, 0x6d0900, 0x6d0e00,
    0x6d1209, 0x6d1400, 0x6dade6, 0x6db1dc, 0x6db5e6, 0x6dd000, 0x6e0100, 0x6e0a00, 0x6e0c00, 0x6e8600,
    0x6e8900, 0x6e9a00, 0x6ea100, 0x6eaa09, 0x6ead09, 0x6eb100, 0x6eb800, 0x6f9907, 0x6f9e00, 0x6fa100,
    0x6faa00, 0x6fb500, 0x6fba00, 0x6fbd00, 0x6fca09, 0x6fd000, 0x709200, 0x70b100, 0x70d200, 0x70d900,
    0x70dd07, 0x70e000, 0x7341e6, 0x734c00, 0x735101, 0x7355dc, 0x7369e6, 0x7371dc, 0x7381e6, 0x738600,
    0x738901, 0x73a400, 0x73b5dc, 0x73b800, 0x73d1e6, 0x73d400, 0x73de00, 0x73e1e6, 0x73e800, 0x7701e6,
    0x7709dc, 0x770de6, 0x7729dc, 0x772de6, 0x7735ea, 0x7739d6, 0x773ddc, 0x7741ca, 0x7745e6, 0x77d9e8,
    0x77dde4, 0x77e5dc, 0x77e9da, 0x77ede6, 0x77f1e9, 0x77f5dc, 0x77f9e6, 0x77fddc, 0x780000, 0x8341e6,
    0x834901, 0x8351e6, 0x836101, 0x836de6, 0x837600, 0x8385e6, 0x838a00, 0x839501, 0x839de6, 0x83a1dc,
    0x83a5e6, 0x83a901, 0x83b1dc, 0x83c1e6, 0x83c400, 0xb3bde6, 0xb3c800, 0xb5fd09, 0xb60000, 0xb781e6,
    0xb80000, 0xc0a9da, 0xc0ade4, 0xc0b1e8, 0xc0b5de, 0xc0bae0, 0xc0c000, 0xc26508, 0xc26c00, 0x299bde6,
    0x299c200, 0x299cc00, 0x299d1e6, 0x299f800, 0x29a79e6, 0x29a8000, 0x29bc1e6, 0x29bc800, 0x2a00900, 0x2a00c00,
    0x2a01909, 0x2a01c00, 0x2a02d00, 0x2a03000, 0x2a08e00, 0x2a09500, 0x2a09e00, 0x2a0a000, 0x2a0b109, 0x2a0b400,
    0x2a20200, 0x2a20800, 0x2a2d200, 0x2a31109, 0x2a31500, 0x2a31800, 0x2a381e6, 0x2a3c800, 0x2a3fd00, 0x2a40000,
    0x2a49900, 0x2a4addc, 0x2a4b800, 0x2a51d00, 0x2a54a00, 0x2a54e09, 0x2a55000, 0x2a60100, 0x2a60e00, 0x2a61000,
    0x2a6cd07, 0x2a6d200, 0x2a6d900, 0x2a6ea00, 0x2a6f100, 0x2a6fa00, 0x2a70209, 0x2a70400, 0x2a79500, 0x2a79800,
    0x2a8a500, 0x2a8be00, 0x2a8c500, 0x2a8ce00, 0x2a8d500, 0x2a8dc00, 0x2a90d00, 0x2a91000, 0x2a93100, 0x2a93600,
    0x2a93800, 0x2a9ee00, 0x2a9f100, 0x2a9f600, 0x2a9f800, 0x2aac1e6, 0x2aac400, 0x2aac9e6, 0x2aad1dc, 0x2aad400,
    0x2aadde6, 0x2aae400, 0x2aaf9e6, 0x2ab0000, 0x2ab05e6, 0x2ab0800, 0x2abae00, 0x2abb100, 0x2abba00, 0x2abc000,
    0x2abd600, 0x2abd909, 0x2abdc00, 0x2af8e00, 0x2af9500, 0x2af9a00, 0x2afa100, 0x2afa600, 0x2afac00, 0x2afb200,
    0x2afb509, 0x2afb800, 0x3ec791a, 0x3ec7c00, 0x3f80100, 0x3f84000, 0x3f881e6, 0x3f89ddc, 0x3f8b9e6, 0x3f8c000,
    0x407f5dc, 0x407f800, 0x40b81dc, 0x40b8400, 0x40dd9e6, 0x40dec00, 0x4280500, 0x4281000, 0x4281500, 0x4281c00,
    0x4283100, 0x42835dc, 0x4283900, 0x4283de6, 0x4284000, 0x428e1e6, 0x428e501, 0x428e9dc, 0x428ec00, 0x428fd09,
    0x4290000, 0x42b95e6, 0x42b99dc, 0x42b9c00, 0x43491e6, 0x434a000, 0x435a5e6, 0x435b800, 0x43aade6, 0x43ab400,
    0x43be9dc, 0x43bf100, 0x43bf5dc, 0x43c0000, 0x43d19dc, 0x43d21e6, 0x43d2ddc, 0x43d31e6, 0x43d35dc, 0x43d4400,
    0x43e09e6, 0x43e0ddc, 0x43e11e6, 0x43e15dc, 0x43e1800, 0x4400200, 0x4400500, 0x4400a00, 0x4400c00, 0x440e100,
    0x4411909, 0x4411c00, 0x441c109, 0x441c400, 0x441cd00, 0x441d400, 0x441fd09, 0x4420100, 0x4420a00, 0x4420c00,
    0x442c200, 0x442cd00, 0x442de00, 0x442e509, 0x442e907, 0x442ec00, 0x4430900, 0x4430c00, 0x44401e6, 0x4440c00,
    0x4449d00, 0x444b200, 0x444b500, 0x444cd09, 0x444d400, 0x4451600, 0x4451c00, 0x445cd07, 0x445d000, 0x4460100,
    0x4460a00, 0x4460c00, 0x446ce00, 0x446d900, 0x446fe00, 0x4470209, 0x4470400, 0x4472500, 0x4472907, 0x4472d00,
    0x4473400, 0x4473a00, 0x4473d00, 0x4474000, 0x448b200, 0x448bd00, 0x448ca00, 0x448d100, 0x448d609, 0x448d907,
    0x448dd00, 0x448e000, 0x448f900, 0x448fc00, 0x4490500, 0x4490800, 0x44b7d00, 0x44b8200, 0x44b8d00, 0x44ba507,
    0x44ba909, 0x44bac00, 0x44c0100, 0x44c0a00, 0x44c1000, 0x44ced07, 0x44cf400, 0x44cfa00, 0x44d0100, 0x44d0600,
    0x44d1400, 0x44d1e00, 0x44d2400, 0x44d2e00, 0x44d3609, 0x44d3800, 0x44d5e00, 0x44d6000, 0x44d8a00, 0x44d9000,
    0x44d99e6, 0x44db400, 0x44dc1e6, 0x44dd400, 0x44ee200, 0x44eed00, 0x44f0400, 0x44f0a00, 0x44f0c00, 0x44f1600,
    0x44f1800, 0x44f1e00, 0x44f2c00, 0x44f3200, 0x44f3909, 0x44f3e09, 0x44f4109, 0x44f4400, 0x44f4900, 0x44f4c00,
    0x44f8500, 0x44f8c00, 0x450d600, 0x450e100, 0x4510200, 0x4510909, 0x4510d00, 0x4511600, 0x4511907, 0x4511c00,
    0x45179e6, 0x4517c00, 0x452c200, 0x452cd00, 0x452e600, 0x452e900, 0x452ee00, 0x452fd00, 0x4530600, 0x4530909,
    0x4530d07, 0x4531000, 0x456be00, 0x456c900, 0x456d800, 0x456e200, 0x456f100, 0x456fa00, 0x456fd09, 0x4570107,
    0x4570400, 0x4577100, 0x4577800, 0x458c200, 0x458cd00, 0x458ee00, 0x458f500, 0x458fa00, 0x458fd09, 0x4590100,
    0x4590400, 0x45aad00, 0x45ab200, 0x45ab500, 0x45aba00, 0x45ac100, 0x45ada09, 0x45add07, 0x45ae000, 0x45c7500,
    0x45c7a00, 0x45c7d00, 0x45c8200, 0x45c8900, 0x45c9a00, 0x45c9d00, 0x45cad09, 0x45cb000, 0x460b200, 0x460bd00,
    0x460e200, 0x460e509, 0x460e907, 0x460ec00, 0x464c200, 0x464d800, 0x464de00, 0x464e400, 0x464ed00, 0x464f609,
    0x464f909, 0x464fc00, 0x4650200, 0x4650400, 0x4650a00, 0x4650d07, 0x4651000, 0x4674600, 0x4675100, 0x4676000,
    0x4676900, 0x4677200, 0x4678109, 0x4678400, 0x4679200, 0x4679400, 0x4680500, 0x4682c00, 0x468cd00, 0x468d109,
    0x468d500, 0x468e600, 0x468e800, 0x468ed00, 0x468fc00, 0x4691d09, 0x4692000, 0x4694500, 0x4695e00, 0x4696500,
    0x4697000, 0x46a2900, 0x46a5e00, 0x46a6100, 0x46a6509, 0x46a6800, 0x46d8100, 0x46d8600, 0x46d8900, 0x46d9600,
    0x46d9900, 0x46d9e00, 0x46da000, 0x470be00, 0x470c100, 0x470dc00, 0x470e100, 0x470fa00, 0x470fd09, 0x4710000,
    0x4724900, 0x472a000, 0x472a600, 0x472a900, 0x472c600, 0x472c900, 0x472d200, 0x472d500, 0x472dc00, 0x474c500,
    0x474dc00, 0x474e900, 0x474ec00, 0x474f100, 0x474f800, 0x474fd00, 0x4750907, 0x4750d00, 0x4751109, 0x4751800,
    0x4751d00, 0x4752000, 0x4762a00, 0x4763c00, 0x4764100, 0x4764800, 0x4764e00, 0x4765500, 0x4765a00, 0x4765d09,
    0x4766000, 0x47bcd00, 0x47bd600, 0x47bdc00, 0x47c0100, 0x47c0800, 0x47c0e00, 0x47c1000, 0x47cd200, 0x47cd900,
    0x47cec00, 0x47cfa00, 0x47d0100, 0x47d0609, 0x47d0909, 0x47d0c00, 0x47d6900, 0x47d6c00, 0x4d10100, 0x4d10400,
    0x4d11d00, 0x4d15800, 0x5847900, 0x584aa00, 0x584b500, 0x584bd09, 0x584c000, 0x5abc101, 0x5abd400, 0x5acc1e6,
    0x5acdc00, 0x5bd3d00, 0x5bd4000, 0x5bd4600, 0x5be2000, 0x5be3d00, 0x5be4c00, 0x5bf9100, 0x5bf9400, 0x5bfc206,
    0x5bfc800, 0x6f27500, 0x6f27901, 0x6f27c00, 0x73c0100, 0x73cb800, 0x73cc100, 0x73d1c00, 0x74596d8, 0x7459d01,
    0x745a800, 0x745b6e2, 0x745bad8, 0x745cc00, 0x745eddc, 0x7460c00, 0x74615e6, 0x74629dc, 0x7463000, 0x746a9e6,
    0x746b800, 0x74909e6, 0x7491400, 0x7680100, 0x768dc00, 0x768ed00, 0x769b400, 0x769d500, 0x769d800, 0x76a1100,
    0x76a1400, 0x76a6d00, 0x76a8000, 0x76a8500, 0x76ac000, 0x78001e6, 0x7801c00, 0x78021e6, 0x7806400, 0x7806de6,
    0x7808800, 0x7808de6, 0x7809400, 0x78099e6, 0x780ac00, 0x7823de6, 0x7824000, 0x784c1e6, 0x784dc00, 0x78ab9e6,
    0x78abc00, 0x78bb1e6, 0x78bc000, 0x793b1e8, 0x793b9dc, 0x793bde6, 0x793c000, 0x797b9e6, 0x797bddc, 0x797c000,
    0x79b8de6, 0x79b9000, 0x79b99e6, 0x79b9c00, 0x79bb9e6, 0x79bc000, 0x79bd5e6, 0x79bd800, 0x7a341dc, 0x7a35c00,
    0x7a511e6, 0x7a52907, 0x7a52c00, 0x38040100, 0x3807c000,
];
/// Unicode's canonical decompositions, sorted: each character (times 2^42) with its first part
/// (times 2^21) and its second (0 for a singleton).
#[rustfmt::skip]
static SPLITS: [u64; 2081] = [
    0x3000008200300, 0x3040008200301, 0x3080008200302, 0x30c0008200303, 0x3100008200308, 0x314000820030a,
    0x31c0008600327, 0x3200008a00300, 0x3240008a00301, 0x3280008a00302, 0x32c0008a00308, 0x3300009200300,
    0x3340009200301, 0x3380009200302, 0x33c0009200308, 0x3440009c00303, 0x3480009e00300, 0x34c0009e00301,
    0x3500009e00302, 0x3540009e00303, 0x3580009e00308, 0x364000aa00300, 0x368000aa00301, 0x36c000aa00302,
    0x370000aa00308, 0x374000b200301, 0x380000c200300, 0x384000c200301, 0x388000c200302, 0x38c000c200303,
    0x390000c200308, 0x394000c20030a, 0x39c000c600327, 0x3a0000ca00300, 0x3a4000ca00301, 0x3a8000ca00302,
    0x3ac000ca00308, 0x3b0000d200300, 0x3b4000d200301, 0x3b8000d200302, 0x3bc000d200308, 0x3c4000dc00303,
    0x3c8000de00300, 0x3cc000de00301, 0x3d0000de00302, 0x3d4000de00303, 0x3d8000de00308, 0x3e4000ea00300,
    0x3e8000ea00301, 0x3ec000ea00302, 0x3f0000ea00308, 0x3f4000f200301, 0x3fc000f200308, 0x4000008200304,
    0x404000c200304, 0x4080008200306, 0x40c000c200306, 0x4100008200328, 0x414000c200328, 0x4180008600301,
    0x41c000c600301, 0x4200008600302, 0x424000c600302, 0x4280008600307, 0x42c000c600307, 0x430000860030c,
    0x434000c60030c, 0x438000880030c, 0x43c000c80030c, 0x4480008a00304, 0x44c000ca00304, 0x4500008a00306,
    0x454000ca00306, 0x4580008a00307, 0x45c000ca00307, 0x4600008a00328, 0x464000ca00328, 0x4680008a0030c,
    0x46c000ca0030c, 0x4700008e00302, 0x474000ce00302, 0x4780008e00306, 0x47c000ce00306, 0x4800008e00307,
    0x484000ce00307, 0x4880008e00327, 0x48c000ce00327, 0x4900009000302, 0x494000d000302, 0x4a00009200303,
    0x4a4000d200303, 0x4a80009200304, 0x4ac000d200304, 0x4b00009200306, 0x4b4000d200306, 0x4b80009200328,
    0x4bc000d200328, 0x4c00009200307, 0x4d00009400302, 0x4d4000d400302, 0x4d80009600327, 0x4dc000d600327,
    0x4e40009800301, 0x4e8000d800301, 0x4ec0009800327, 0x4f0000d800327, 0x4f4000980030c, 0x4f8000d80030c,
    0x50c0009c00301, 0x510000dc00301, 0x5140009c00327, 0x518000dc00327, 0x51c0009c0030c, 0x520000dc0030c,
    0x5300009e00304, 0x534000de00304, 0x5380009e00306, 0x53c000de00306, 0x5400009e0030b, 0x544000de0030b,
    0x550000a400301, 0x554000e400301, 0x558000a400327, 0x55c000e400327, 0x560000a40030c, 0x564000e40030c,
    0x568000a600301, 0x56c000e600301, 0x570000a600302, 0x574000e600302, 0x578000a600327, 0x57c000e600327,
    0x580000a60030c, 0x584000e60030c, 0x588000a800327, 0x58c000e800327, 0x590000a80030c, 0x594000e80030c,
    0x5a0000aa00303, 0x5a4000ea00303, 0x5a8000aa00304, 0x5ac000ea00304, 0x5b0000aa00306, 0x5b4000ea00306,
    0x5b8000aa0030a, 0x5bc000ea0030a, 0x5c0000aa0030b, 0x5c4000ea0030b, 0x5c8000aa00328, 0x5cc000ea00328,
    0x5d0000ae00302, 0x5d4000ee00302, 0x5d8000b200302, 0x5dc000f200302, 0x5e0000b200308, 0x5e4000b400301,
    0x5e8000f400301, 0x5ec000b400307, 0x5f0000f400307, 0x5f4000b40030c, 0x5f8000f40030c, 0x6800009e0031b,
    0x684000de0031b, 0x6bc000aa0031b, 0x6c0000ea0031b, 0x734000820030c, 0x738000c20030c, 0x73c000920030c,
    0x740000d20030c, 0x7440009e0030c, 0x748000de0030c, 0x74c000aa0030c, 0x750000ea0030c, 0x754001b800304,
    0x758001f800304, 0x75c001b800301, 0x760001f800301, 0x764001b80030c, 0x768001f80030c, 0x76c001b800300,
    0x770001f800300, 0x7780018800304, 0x77c001c800304, 0x7800044c00304, 0x7840044e00304, 0x7880018c00304,
    0x78c001cc00304, 0x7980008e0030c, 0x79c000ce0030c, 0x7a0000960030c, 0x7a4000d60030c, 0x7a80009e00328,
    0x7ac000de00328, 0x7b0003d400304, 0x7b4003d600304, 0x7b80036e0030c, 0x7bc005240030c, 0x7c0000d40030c,
    0x7d00008e00301, 0x7d4000ce00301, 0x7e00009c00300, 0x7e4000dc00300, 0x7e80018a00301, 0x7ec001ca00301,
    0x7f00018c00301, 0x7f4001cc00301, 0x7f8001b000301, 0x7fc001f000301, 0x800000820030f, 0x804000c20030f,
    0x8080008200311, 0x80c000c200311, 0x8100008a0030f, 0x814000ca0030f, 0x8180008a00311, 0x81c000ca00311,
    0x820000920030f, 0x824000d20030f, 0x8280009200311, 0x82c000d200311, 0x8300009e0030f, 0x834000de0030f,
    0x8380009e00311, 0x83c000de00311, 0x840000a40030f, 0x844000e40030f, 0x848000a400311, 0x84c000e400311,
    0x850000aa0030f, 0x854000ea0030f, 0x858000aa00311, 0x85c000ea00311, 0x860000a600326, 0x864000e600326,
    0x868000a800326, 0x86c000e800326, 0x878000900030c, 0x87c000d00030c, 0x8980008200307, 0x89c000c200307,
    0x8a00008a00327, 0x8a4000ca00327, 0x8a8001ac00304, 0x8ac001ec00304, 0x8b0001aa00304, 0x8b4001ea00304,
    0x8b80009e00307, 0x8bc000de00307, 0x8c00045c00304, 0x8c40045e00304, 0x8c8000b200304, 0x8cc000f200304,
    0xd000060000000, 0xd040060200000, 0xd0c0062600000, 0xd100061000301, 0xdd00057200000, 0xdf80007600000,
    0xe140015000301, 0xe180072200301, 0xe1c0016e00000, 0xe200072a00301, 0xe240072e00301, 0xe280073200301,
    0xe300073e00301, 0xe380074a00301, 0xe3c0075200301, 0xe400079400301, 0xea80073200308, 0xeac0074a00308,
    0xeb00076200301, 0xeb40076a00301, 0xeb80076e00301, 0xebc0077200301, 0xec00079600301, 0xf280077200308,
    0xf2c0078a00308, 0xf300077e00301, 0xf340078a00301, 0xf380079200301, 0xf4c007a400301, 0xf50007a400308,
    0x10000082a00300, 0x10040082a00308, 0x100c0082600301, 0x101c0080c00308, 0x10300083400301, 0x10340083000300,
    0x10380084600306, 0x10640083000306, 0x10e40087000306, 0x11400086a00300, 0x11440086a00308, 0x114c0086600301,
    0x115c008ac00308, 0x11700087400301, 0x11740087000300, 0x11780088600306, 0x11d8008e80030f, 0x11dc008ea0030f,
    0x13040082c00306, 0x13080086c00306, 0x13400082000306, 0x13440086000306, 0x13480082000308, 0x134c0086000308,
    0x13580082a00306, 0x135c0086a00306, 0x1368009b000308, 0x136c009b200308, 0x13700082c00308, 0x13740086c00308,
    0x13780082e00308, 0x137c0086e00308, 0x13880083000304, 0x138c0087000304, 0x13900083000308, 0x13940087000308,
    0x13980083c00308, 0x139c0087c00308, 0x13a8009d000308, 0x13ac009d200308, 0x13b00085a00308, 0x13b40089a00308,
    0x13b80084600304, 0x13bc0088600304, 0x13c00084600308, 0x13c40088600308, 0x13c8008460030b, 0x13cc008860030b,
    0x13d00084e00308, 0x13d40088e00308, 0x13e00085600308, 0x13e40089600308, 0x188800c4e00653, 0x188c00c4e00654,
    0x189000c9000654, 0x189400c4e00655, 0x189800c9400654, 0x1b0000daa00654, 0x1b0800d8200654, 0x1b4c00da400654,
    0x24a4012500093c, 0x24c4012600093c, 0x24d0012660093c, 0x25600122a0093c, 0x25640122c0093c, 0x25680122e0093c,
    0x256c012380093c, 0x2570012420093c, 0x2574012440093c, 0x2578012560093c, 0x257c0125e0093c, 0x272c0138e009be,
    0x27300138e009d7, 0x277001342009bc, 0x277401344009bc, 0x277c0135e009bc, 0x28cc0146400a3c, 0x28d80147000a3c,
    0x29640142c00a3c, 0x29680142e00a3c, 0x296c0143800a3c, 0x29780145600a3c, 0x2d200168e00b56, 0x2d2c0168e00b3e,
    0x2d300168e00b57, 0x2d700164200b3c, 0x2d740164400b3c, 0x2e500172400bd7, 0x2f280178c00bbe, 0x2f2c0178e00bbe,
    0x2f300178c00bd7, 0x31200188c00c56, 0x33000197e00cd5, 0x331c0198c00cd5, 0x33200198c00cd6, 0x33280198c00cc2,
    0x332c0199400cd5, 0x352801a8c00d3e, 0x352c01a8e00d3e, 0x353001a8c00d57, 0x376801bb200dca, 0x377001bb200dcf,
    0x377401bb800dca, 0x377801bb200ddf, 0x3d0c01e8400fb7, 0x3d3401e9800fb7, 0x3d4801ea200fb7, 0x3d5c01eac00fb7,
    0x3d7001eb600fb7, 0x3da401e8000fb5, 0x3dcc01ee200f72, 0x3dd401ee200f74, 0x3dd801f6400f80, 0x3de001f6600f80,
    0x3e0401ee200f80, 0x3e4c01f2400fb7, 0x3e7401f3800fb7, 0x3e8801f4200fb7, 0x3e9c01f4c00fb7, 0x3eb001f5600fb7,
    0x3ee401f2000fb5, 0x40980204a0102e, 0x6c180360a01b35, 0x6c200360e01b35, 0x6c280361201b35, 0x6c300361601b35,
    0x6c380361a01b35, 0x6c480362201b35, 0x6cec0367401b35, 0x6cf40367801b35, 0x6d000367c01b35, 0x6d040367e01b35,
    0x6d0c0368401b35, 0x78000008200325, 0x7804000c200325, 0x78080008400307, 0x780c000c400307, 0x78100008400323,
    0x7814000c400323, 0x78180008400331, 0x781c000c400331, 0x78200018e00301, 0x7824001ce00301, 0x78280008800307,
    0x782c000c800307, 0x78300008800323, 0x7834000c800323, 0x78380008800331, 0x783c000c800331, 0x78400008800327,
    0x7844000c800327, 0x7848000880032d, 0x784c000c80032d, 0x78500022400300, 0x78540022600300, 0x78580022400301,
    0x785c0022600301, 0x78600008a0032d, 0x7864000ca0032d, 0x78680008a00330, 0x786c000ca00330, 0x78700045000306,
    0x78740045200306, 0x78780008c00307, 0x787c000cc00307, 0x78800008e00304, 0x7884000ce00304, 0x78880009000307,
    0x788c000d000307, 0x78900009000323, 0x7894000d000323, 0x78980009000308, 0x789c000d000308, 0x78a00009000327,
    0x78a4000d000327, 0x78a8000900032e, 0x78ac000d00032e, 0x78b00009200330, 0x78b4000d200330, 0x78b80019e00301,
    0x78bc001de00301, 0x78c00009600301, 0x78c4000d600301, 0x78c80009600323, 0x78cc000d600323, 0x78d00009600331,
    0x78d4000d600331, 0x78d80009800323, 0x78dc000d800323, 0x78e003c6c00304, 0x78e403c6e00304, 0x78e80009800331,
    0x78ec000d800331, 0x78f0000980032d, 0x78f4000d80032d, 0x78f80009a00301, 0x78fc000da00301, 0x79000009a00307,
    0x7904000da00307, 0x79080009a00323, 0x790c000da00323, 0x79100009c00307, 0x7914000dc00307, 0x79180009c00323,
    0x791c000dc00323, 0x79200009c00331, 0x7924000dc00331, 0x79280009c0032d, 0x792c000dc0032d, 0x7930001aa00301,
    0x7934001ea00301, 0x7938001aa00308, 0x793c001ea00308, 0x79400029800300, 0x79440029a00300, 0x79480029800301,
    0x794c0029a00301, 0x7950000a000301, 0x7954000e000301, 0x7958000a000307, 0x795c000e000307, 0x7960000a400307,
    0x7964000e400307, 0x7968000a400323, 0x796c000e400323, 0x797003cb400304, 0x797403cb600304, 0x7978000a400331,
    0x797c000e400331, 0x7980000a600307, 0x7984000e600307, 0x7988000a600323, 0x798c000e600323, 0x7990002b400307,
    0x7994002b600307, 0x7998002c000307, 0x799c002c200307, 0x79a003cc400307, 0x79a403cc600307, 0x79a8000a800307,
    0x79ac000e800307, 0x79b0000a800323, 0x79b4000e800323, 0x79b8000a800331, 0x79bc000e800331, 0x79c0000a80032d,
    0x79c4000e80032d, 0x79c8000aa00324, 0x79cc000ea00324, 0x79d0000aa00330, 0x79d4000ea00330, 0x79d8000aa0032d,
    0x79dc000ea0032d, 0x79e0002d000301, 0x79e4002d200301, 0x79e8002d400308, 0x79ec002d600308, 0x79f0000ac00303,
    0x79f4000ec00303, 0x79f8000ac00323, 0x79fc000ec00323, 0x7a00000ae00300, 0x7a04000ee00300, 0x7a08000ae00301,
    0x7a0c000ee00301, 0x7a10000ae00308, 0x7a14000ee00308, 0x7a18000ae00307, 0x7a1c000ee00307, 0x7a20000ae00323,
    0x7a24000ee00323, 0x7a28000b000307, 0x7a2c000f000307, 0x7a30000b000308, 0x7a34000f000308, 0x7a38000b200307,
    0x7a3c000f200307, 0x7a40000b400302, 0x7a44000f400302, 0x7a48000b400323, 0x7a4c000f400323, 0x7a50000b400331,
    0x7a54000f400331, 0x7a58000d000331, 0x7a5c000e800308, 0x7a60000ee0030a, 0x7a64000f20030a, 0x7a6c002fe00307,
    0x7a800008200323, 0x7a84000c200323, 0x7a880008200309, 0x7a8c000c200309, 0x7a900018400301, 0x7a94001c400301,
    0x7a980018400300, 0x7a9c001c400300, 0x7aa00018400309, 0x7aa4001c400309, 0x7aa80018400303, 0x7aac001c400303,
    0x7ab003d4000302, 0x7ab403d4200302, 0x7ab80020400301, 0x7abc0020600301, 0x7ac00020400300, 0x7ac40020600300,
    0x7ac80020400309, 0x7acc0020600309, 0x7ad00020400303, 0x7ad40020600303, 0x7ad803d4000306, 0x7adc03d4200306,
    0x7ae00008a00323, 0x7ae4000ca00323, 0x7ae80008a00309, 0x7aec000ca00309, 0x7af00008a00303, 0x7af4000ca00303,
    0x7af80019400301, 0x7afc001d400301, 0x7b000019400300, 0x7b04001d400300, 0x7b080019400309, 0x7b0c001d400309,
    0x7b100019400303, 0x7b14001d400303, 0x7b1803d7000302, 0x7b1c03d7200302, 0x7b200009200309, 0x7b24000d200309,
    0x7b280009200323, 0x7b2c000d200323, 0x7b300009e00323, 0x7b34000de00323, 0x7b380009e00309, 0x7b3c000de00309,
    0x7b40001a800301, 0x7b44001e800301, 0x7b48001a800300, 0x7b4c001e800300, 0x7b50001a800309, 0x7b54001e800309,
    0x7b58001a800303, 0x7b5c001e800303, 0x7b6003d9800302, 0x7b6403d9a00302, 0x7b680034000301, 0x7b6c0034200301,
    0x7b700034000300, 0x7b740034200300, 0x7b780034000309, 0x7b7c0034200309, 0x7b800034000303, 0x7b840034200303,
    0x7b880034000323, 0x7b8c0034200323, 0x7b90000aa00323, 0x7b94000ea00323, 0x7b98000aa00309, 0x7b9c000ea00309,
    0x7ba00035e00301, 0x7ba40036000301, 0x7ba80035e00300, 0x7bac0036000300, 0x7bb00035e00309, 0x7bb40036000309,
    0x7bb80035e00303, 0x7bbc0036000303, 0x7bc00035e00323, 0x7bc40036000323, 0x7bc8000b200300, 0x7bcc000f200300,
    0x7bd0000b200323, 0x7bd4000f200323, 0x7bd8000b200309, 0x7bdc000f200309, 0x7be0000b200303, 0x7be4000f200303,
    0x7c000076200313, 0x7c040076200314, 0x7c0803e0000300, 0x7c0c03e0200300, 0x7c1003e0000301, 0x7c1403e0200301,
    0x7c1803e0000342, 0x7c1c03e0200342, 0x7c200072200313, 0x7c240072200314, 0x7c2803e1000300, 0x7c2c03e1200300,
    0x7c3003e1000301, 0x7c3403e1200301, 0x7c3803e1000342, 0x7c3c03e1200342, 0x7c400076a00313, 0x7c440076a00314,
    0x7c4803e2000300, 0x7c4c03e2200300, 0x7c5003e2000301, 0x7c5403e2200301, 0x7c600072a00313, 0x7c640072a00314,
    0x7c6803e3000300, 0x7c6c03e3200300, 0x7c7003e3000301, 0x7c7403e3200301, 0x7c800076e00313, 0x7c840076e00314,
    0x7c8803e4000300, 0x7c8c03e4200300, 0x7c9003e4000301, 0x7c9403e4200301, 0x7c9803e4000342, 0x7c9c03e4200342,
    0x7ca00072e00313, 0x7ca40072e00314, 0x7ca803e5000300, 0x7cac03e5200300, 0x7cb003e5000301, 0x7cb403e5200301,
    0x7cb803e5000342, 0x7cbc03e5200342, 0x7cc00077200313, 0x7cc40077200314, 0x7cc803e6000300, 0x7ccc03e6200300,
    0x7cd003e6000301, 0x7cd403e6200301, 0x7cd803e6000342, 0x7cdc03e6200342, 0x7ce00073200313, 0x7ce40073200314,
    0x7ce803e7000300, 0x7cec03e7200300, 0x7cf003e7000301, 0x7cf403e7200301, 0x7cf803e7000342, 0x7cfc03e7200342,
    0x7d000077e00313, 0x7d040077e00314, 0x7d0803e8000300, 0x7d0c03e8200300, 0x7d1003e8000301, 0x7d1403e8200301,
    0x7d200073e00313, 0x7d240073e00314, 0x7d2803e9000300, 0x7d2c03e9200300, 0x7d3003e9000301, 0x7d3403e9200301,
    0x7d400078a00313, 0x7d440078a00314, 0x7d4803ea000300, 0x7d4c03ea200300, 0x7d5003ea000301, 0x7d5403ea200301,
    0x7d5803ea000342, 0x7d5c03ea200342, 0x7d640074a00314, 0x7d6c03eb200300, 0x7d7403eb200301, 0x7d7c03eb200342,
    0x7d800079200313, 0x7d840079200314, 0x7d8803ec000300, 0x7d8c03ec200300, 0x7d9003ec000301, 0x7d9403ec200301,
    0x7d9803ec000342, 0x7d9c03ec200342, 0x7da00075200313, 0x7da40075200314, 0x7da803ed000300, 0x7dac03ed200300,
    0x7db003ed000301, 0x7db403ed200301, 0x7db803ed000342, 0x7dbc03ed200342, 0x7dc00076200300, 0x7dc40075800000,
    0x7dc80076a00300, 0x7dcc0075a00000, 0x7dd00076e00300, 0x7dd40075c00000, 0x7dd80077200300, 0x7ddc0075e00000,
    0x7de00077e00300, 0x7de40079800000, 0x7de80078a00300, 0x7dec0079a00000, 0x7df00079200300, 0x7df40079c00000,
    0x7e0003e0000345, 0x7e0403e0200345, 0x7e0803e0400345, 0x7e0c03e0600345, 0x7e1003e0800345, 0x7e1403e0a00345,
    0x7e1803e0c00345, 0x7e1c03e0e00345, 0x7e2003e1000345, 0x7e2403e1200345, 0x7e2803e1400345, 0x7e2c03e1600345,
    0x7e3003e1800345, 0x7e3403e1a00345, 0x7e3803e1c00345, 0x7e3c03e1e00345, 0x7e4003e4000345, 0x7e4403e4200345,
    0x7e4803e4400345, 0x7e4c03e4600345, 0x7e5003e4800345, 0x7e5403e4a00345, 0x7e5803e4c00345, 0x7e5c03e4e00345,
    0x7e6003e5000345, 0x7e6403e5200345, 0x7e6803e5400345, 0x7e6c03e5600345, 0x7e7003e5800345, 0x7e7403e5a00345,
    0x7e7803e5c00345, 0x7e7c03e5e00345, 0x7e8003ec000345, 0x7e8403ec200345, 0x7e8803ec400345, 0x7e8c03ec600345,
    0x7e9003ec800345, 0x7e9403eca00345, 0x7e9803ecc00345, 0x7e9c03ece00345, 0x7ea003ed000345, 0x7ea403ed200345,
    0x7ea803ed400345, 0x7eac03ed600345, 0x7eb003ed800345, 0x7eb403eda00345, 0x7eb803edc00345, 0x7ebc03ede00345,
    0x7ec00076200306, 0x7ec40076200304, 0x7ec803ee000345, 0x7ecc0076200345, 0x7ed00075800345, 0x7ed80076200342,
    0x7edc03f6c00345, 0x7ee00072200306, 0x7ee40072200304, 0x7ee80072200300, 0x7eec0070c00000, 0x7ef00072200345,
    0x7ef80077200000, 0x7f040015000342, 0x7f0803ee800345, 0x7f0c0076e00345, 0x7f100075c00345, 0x7f180076e00342,
    0x7f1c03f8c00345, 0x7f200072a00300, 0x7f240071000000, 0x7f280072e00300, 0x7f2c0071200000, 0x7f300072e00345,
    0x7f3403f7e00300, 0x7f3803f7e00301, 0x7f3c03f7e00342, 0x7f400077200306, 0x7f440077200304, 0x7f480079400300,
    0x7f4c0072000000, 0x7f580077200342, 0x7f5c0079400342, 0x7f600073200306, 0x7f640073200304, 0x7f680073200300,
    0x7f6c0071400000, 0x7f7403ffc00300, 0x7f7803ffc00301, 0x7f7c03ffc00342, 0x7f800078a00306, 0x7f840078a00304,
    0x7f880079600300, 0x7f8c0076000000, 0x7f900078200313, 0x7f940078200314, 0x7f980078a00342, 0x7f9c0079600342,
    0x7fa00074a00306, 0x7fa40074a00304, 0x7fa80074a00300, 0x7fac0071c00000, 0x7fb00074200314, 0x7fb40015000300,
    0x7fb80070a00000, 0x7fbc000c000000, 0x7fc803ef800345, 0x7fcc0079200345, 0x7fd00079c00345, 0x7fd80079200342,
    0x7fdc03fec00345, 0x7fe00073e00300, 0x7fe40071800000, 0x7fe80075200300, 0x7fec0071e00000, 0x7ff00075200345,
    0x7ff40016800000, 0x80000400400000, 0x80040400600000, 0x84980075200000, 0x84a80009600000, 0x84ac0018a00000,
    0x86680432000338, 0x866c0432400338, 0x86b80432800338, 0x8734043a000338, 0x8738043a800338, 0x873c043a400338,
    0x88100440600338, 0x88240441000338, 0x88300441600338, 0x88900444600338, 0x88980444a00338, 0x89040447800338,
    0x89100448600338, 0x891c0448a00338, 0x89240449000338, 0x89800007a00338, 0x8988044c200338, 0x89b40449a00338,
    0x89b80007800338, 0x89bc0007c00338, 0x89c0044c800338, 0x89c4044ca00338, 0x89d0044e400338, 0x89d4044e600338,
    0x89e0044ec00338, 0x89e4044ee00338, 0x8a00044f400338, 0x8a04044f600338, 0x8a100450400338, 0x8a140450600338,
    0x8a200450c00338, 0x8a240450e00338, 0x8ab00454400338, 0x8ab40455000338, 0x8ab80455200338, 0x8abc0455600338,
    0x8b80044f800338, 0x8b84044fa00338, 0x8b880452200338, 0x8b8c0452400338, 0x8ba80456400338, 0x8bac0456600338,
    0x8bb00456800338, 0x8bb40456a00338, 0x8ca40601000000, 0x8ca80601200000, 0xab70055ba00338, 0xc1300609603099,
    0xc1380609a03099, 0xc1400609e03099, 0xc148060a203099, 0xc150060a603099, 0xc158060aa03099, 0xc160060ae03099,
    0xc168060b203099, 0xc170060b603099, 0xc178060ba03099, 0xc180060be03099, 0xc188060c203099, 0xc194060c803099,
    0xc19c060cc03099, 0xc1a4060d003099, 0xc1c0060de03099, 0xc1c4060de0309a, 0xc1cc060e403099, 0xc1d0060e40309a,
    0xc1d8060ea03099, 0xc1dc060ea0309a, 0xc1e4060f003099, 0xc1e8060f00309a, 0xc1f0060f603099, 0xc1f4060f60309a,
    0xc2500608c03099, 0xc2780613a03099, 0xc2b00615603099, 0xc2b80615a03099, 0xc2c00615e03099, 0xc2c80616203099,
    0xc2d00616603099, 0xc2d80616a03099, 0xc2e00616e03099, 0xc2e80617203099, 0xc2f00617603099, 0xc2f80617a03099,
    0xc3000617e03099, 0xc3080618203099, 0xc3140618803099, 0xc31c0618c03099, 0xc3240619003099, 0xc3400619e03099,
    0xc3440619e0309a, 0xc34c061a403099, 0xc350061a40309a, 0xc358061aa03099, 0xc35c061aa0309a, 0xc364061b003099,
    0xc368061b00309a, 0xc370061b603099, 0xc374061b60309a, 0xc3d00614c03099, 0xc3dc061de03099, 0xc3e0061e003099,
    0xc3e4061e203099, 0xc3e8061e403099, 0xc3f8061fa03099, 0x3e4001189000000, 0x3e4040cde800000, 0x3e40811d9400000,
    0x3e40c1199000000, 0x3e4100dda200000, 0x3e41409c6400000, 0x3e4180a7ca00000, 0x3e41c13f3800000, 0x3e42013f3800000,
    0x3e4240b2a200000, 0x3e428123a200000, 0x3e42c0ab0e00000, 0x3e4300b29000000, 0x3e4340c3ec00000, 0x3e4380ecd200000,
    0x3e43c0ff0a00000, 0x3e44010c7e00000, 0x3e44410f7400000, 0x3e448111f000000, 0x3e44c1211e00000, 0x3e4500d40400000,
    0x3e4540da3600000, 0x3e4580e1b200000, 0x3e45c0e7bc00000, 0x3e4601087a00000, 0x3e464122d400000, 0x3e468133e200000,
    0x3e46c09d0400000, 0x3e4700a6ea00000, 0x3e4740d60800000, 0x3e4780e43600000, 0x3e47c10c5a00000, 0x3e48013c3c00000,
    0x3e4840baa000000, 0x3e4880dfd600000, 0x3e48c10b9a00000, 0x3e490112c800000, 0x3e4940c59200000, 0x3e498103b000000,
    0x3e49c1103e00000, 0x3e4a00bd9400000, 0x3e4a40ce2e00000, 0x3e4a80dad400000, 0x3e4ac0e5f800000, 0x3e4b01219c00000,
    0x3e4b409f0c00000, 0x3e4b80a36e00000, 0x3e4bc0a5bc00000, 0x3e4c00c98800000, 0x3e4c40d5a600000, 0x3e4c80e42000000,
    0x3e4cc0edce00000, 0x3e4d01000200000, 0x3e4d410c0c00000, 0x3e4d810cb800000, 0x3e4dc11bde00000, 0x3e4e012e6400000,
    0x3e4e4136de00000, 0x3e4e813bf400000, 0x3e4ec0f11800000, 0x3e4f00f2fe00000, 0x3e4f40fb4000000, 0x3e4f81079200000,
    0x3e4fc1260800000, 0x3e50013cfe00000, 0x3e504115ac00000, 0x3e5080b1be00000, 0x3e50c0be0800000, 0x3e5100f8c000000,
    0x3e514100fc00000, 0x3e5180e4c400000, 0x3e51c0f19400000, 0x3e5201198400000, 0x3e52412dee00000, 0x3e5280b1b000000,
    0x3e52c0b8c400000, 0x3e5300d42600000, 0x3e5340dbb400000, 0x3e5380de1e00000, 0x3e53c0fa5e00000, 0x3e5400fc6e00000,
    0x3e54412c9600000, 0x3e5480a5a400000, 0x3e54c1011600000, 0x3e5500a3b800000, 0x3e5540a39800000, 0x3e5580f43800000,
    0x3e55c0fb7c00000, 0x3e560107e200000, 0x3e56412cea00000, 0x3e5681170000000, 0x3e56c0c59e00000, 0x3e5700d40400000,
    0x3e574115fc00000, 0x3e57809c7200000, 0x3e57c0b7ce00000, 0x3e5800c02400000, 0x3e5840e70e00000, 0x3e5880eae000000,
    0x3e58c0a62e00000, 0x3e5900f1f600000, 0x3e59409f7e00000, 0x3e5980bf5200000, 0x3e59c09c1a00000, 0x3e5a00d99800000,
    0x3e5a40caf000000, 0x3e5a80fa4400000, 0x3e5ac0a78600000, 0x3e5b00b0bc00000, 0x3e5b40ee0200000, 0x3e5b81089200000,
    0x3e5bc1155400000, 0x3e5c00d77400000, 0x3e5c411f6000000, 0x3e5c80d91000000, 0x3e5cc0c5fc00000, 0x3e5d0105ca00000,
    0x3e5d40c74000000, 0x3e5d80eaca00000, 0x3e5dc09d5c00000, 0x3e5e00a2d200000, 0x3e5e40a39200000, 0x3e5e80d10200000,
    0x3e5ec0f9ce00000, 0x3e5f0104de00000, 0x3e5f4115a400000, 0x3e5f81239e00000, 0x3e5fc0a5ea00000, 0x3e6000a88400000,
    0x3e6040b2e600000, 0x3e6080bdd800000, 0x3e60c0cb8a00000, 0x3e6100dffc00000, 0x3e6140f25400000, 0x3e61812b5a00000,
    0x3e61c134d400000, 0x3e62013d2e00000, 0x3e62413d9c00000, 0x3e6280a53600000, 0x3e62c0cd8c00000, 0x3e6300d6ee00000,
    0x3e63411ec400000, 0x3e6380bce800000, 0x3e63c0c32000000, 0x3e6400c40000000, 0x3e6440c93400000, 0x3e6480de4600000,
    0x3e64c0e29200000, 0x3e6500e91200000, 0x3e6540f39400000, 0x3e6580fbe800000, 0x3e65c100de00000, 0x3e66011e4c00000,
    0x3e664109dc00000, 0x3e6681204600000, 0x3e66c1269400000, 0x3e6700a42e00000, 0x3e6740a54600000, 0x3e6780a97a00000,
    0x3e67c0e19000000, 0x3e6801118400000, 0x3e6841155400000, 0x3e6880bd9200000, 0x3e68c0bfea00000, 0x3e6900c6f600000,
    0x3e6940d75c00000, 0x3e6980f87c00000, 0x3e69c0e6ea00000, 0x3e6a009dc800000, 0x3e6a40adf200000, 0x3e6a80b7ce00000,
    0x3e6ac0bb7400000, 0x3e6b00c03800000, 0x3e6b40e76400000, 0x3e6b80e8d200000, 0x3e6bc0ff3400000, 0x3e6c01008c00000,
    0x3e6c41246800000, 0x3e6c812dec00000, 0x3e6cc12e9000000, 0x3e6d01303000000, 0x3e6d409f1600000, 0x3e6d80f35c00000,
    0x3e6dc1236800000, 0x3e6e012d7000000, 0x3e6e40c1c200000, 0x3e6e809d0c00000, 0x3e6ec0a1b400000, 0x3e6f00b7dc00000,
    0x3e6f40b87e00000, 0x3e6f80cb3200000, 0x3e6fc0d40400000, 0x3e7000e39c00000, 0x3e7040ec8400000, 0x3e708109f800000,
    0x3e70c120f800000, 0x3e71013f1a00000, 0x3e7140cd1000000, 0x3e71812c5c00000, 0x3e71c0a51200000, 0x3e7200cef600000,
    0x3e7240cfe600000, 0x3e7280da8200000, 0x3e72c0dd3800000, 0x3e7300e81200000, 0x3e7340eab200000, 0x3e7380f0d600000,
    0x3e73c0fa2000000, 0x3e740130bc00000, 0x3e7440a2da00000, 0x3e7480c45c00000, 0x3e74c12cf000000, 0x3e7500a05600000,
    0x3e7540ba3200000, 0x3e7580dbd400000, 0x3e75c11e5400000, 0x3e7600bf1600000, 0x3e7640c28800000, 0x3e7680d02e00000,
    0x3e76c0e70e00000, 0x3e77012d0c00000, 0x3e7740a45200000, 0x3e7780a81e00000, 0x3e77c0b8ca00000, 0x3e7800cc2600000,
    0x3e7840ce9c00000, 0x3e7880d15000000, 0x3e78c0d9ca00000, 0x3e7900e80c00000, 0x3e7940ebc400000, 0x3e7980fef200000,
    0x3e79c1119e00000, 0x3e7a0111c200000, 0x3e7a41239800000, 0x3e7a812dc400000, 0x3e7ac0a67e00000, 0x3e7b00dd7400000,
    0x3e7b40a83a00000, 0x3e7b80e3a000000, 0x3e7bc0e93000000, 0x3e7c010bf400000, 0x3e7c412d4600000, 0x3e7c8138ae00000,
    0x3e7cc13d3e00000, 0x3e7d00cf2e00000, 0x3e7d40db9600000, 0x3e7d8103d000000, 0x3e7dc0f59600000, 0x3e7e00f64000000,
    0x3e7e40f92400000, 0x3e7e80e58000000, 0x3e7ec0e13200000, 0x3e7f0116b000000, 0x3e7f409d8000000, 0x3e7f81066c00000,
    0x3e7fc0a47400000, 0x3e8000a40e00000, 0x3e8040bd4c00000, 0x3e8080c5a600000, 0x3e80c0f9ac00000, 0x3e8100b70a00000,
    0x3e8140da3c00000, 0x3e8180cd6800000, 0x3e81c11e7600000, 0x3e8201109800000, 0x3e82412c9a00000, 0x3e8281131600000,
    0x3e82c0bda600000, 0x3e8300a28000000, 0x3e8340ab8000000, 0x3e8400b0b400000, 0x3e8480cce800000, 0x3e8540a3bc00000,
    0x3e8580e65400000, 0x3e85c0ed9400000, 0x3e8600f27800000, 0x3e8640f2bc00000, 0x3e8680f2ca00000, 0x3e86c0f31e00000,
    0x3e87012eac00000, 0x3e8740f97c00000, 0x3e8780ff7a00000, 0x3e88010c2400000, 0x3e888115f000000, 0x3e8941207000000,
    0x3e898121fa00000, 0x3e8a8131de00000, 0x3e8ac131f800000, 0x3e8b01325000000, 0x3e8b413b6800000, 0x3e8b8121bc00000,
    0x3e8bc12d6e00000, 0x3e8c009f5c00000, 0x3e8c40a1ce00000, 0x3e8c80a29a00000, 0x3e8cc0a59200000, 0x3e8d00a5c800000,
    0x3e8d40a6a200000, 0x3e8d80ab3a00000, 0x3e8dc0ac0c00000, 0x3e8e00acd000000, 0x3e8e40b08000000, 0x3e8e80b15000000,
    0x3e8ec0b8c800000, 0x3e8f00b8dc00000, 0x3e8f40c12800000, 0x3e8f80c2d000000, 0x3e8fc0c31c00000, 0x3e9000c3e400000,
    0x3e9040ca9e00000, 0x3e9080cbc400000, 0x3e90c0cd2200000, 0x3e9100d10a00000, 0x3e9140daee00000, 0x3e9180dc3400000,
    0x3e91c0de4400000, 0x3e9200e2dc00000, 0x3e9240e45600000, 0x3e9280e84400000, 0x3e92c0f12200000, 0x3e9300f27c00000,
    0x3e9340f29200000, 0x3e9380f29000000, 0x3e93c0f2a000000, 0x3e9400f2ac00000, 0x3e9440f2ba00000, 0x3e9480f31a00000,
    0x3e94c0f31c00000, 0x3e9500f48000000, 0x3e9540f50200000, 0x3e9580f78000000, 0x3e95c0fbe800000, 0x3e9600fc1200000,
    0x3e9640fc8200000, 0x3e9680fee400000, 0x3e96c1000a00000, 0x3e970103da00000, 0x3e974104f200000, 0x3e978104f200000,
    0x3e97c108ae00000, 0x3e9801122000000, 0x3e9841132c00000, 0x3e9881160200000, 0x3e98c1167200000, 0x3e990119a600000,
    0x3e99411a1000000, 0x3e99811f6c00000, 0x3e99c1207000000, 0x3e9a012dc600000, 0x3e9a412ffe00000, 0x3e9a81307600000,
    0x3e9ac0c0ea00000, 0x3e9b0485dc00000, 0x3e9b41043000000, 0x3e9c009c4c00000, 0x3e9c40a36a00000, 0x3e9c80a2d000000,
    0x3e9cc09f0000000, 0x3e9d00a28a00000, 0x3e9d40a30000000, 0x3e9d80a58e00000, 0x3e9dc0a5f400000, 0x3e9e00ab3a00000,
    0x3e9e40aaaa00000, 0x3e9e80ab3200000, 0x3e9ec0abc400000, 0x3e9f00b0b400000, 0x3e9f40b16600000, 0x3e9f80b28800000,
    0x3e9fc0b2a800000, 0x3ea000b4c400000, 0x3ea040b65000000, 0x3ea080bda400000, 0x3ea0c0bdb200000, 0x3ea100bed200000,
    0x3ea140bf5a00000, 0x3ea180c1b000000, 0x3ea1c0c29c00000, 0x3ea200c21000000, 0x3ea240c31c00000, 0x3ea280c2c000000,
    0x3ea2c0c3e400000, 0x3ea300c46800000, 0x3ea340c78800000, 0x3ea380c83800000, 0x3ea3c0c8a400000, 0x3ea400caac00000,
    0x3ea440cce800000, 0x3ea480ce2e00000, 0x3ea4c0ce3600000, 0x3ea500ceac00000, 0x3ea540d6f200000, 0x3ea580d77400000,
    0x3ea5c0da8200000, 0x3ea600ddb600000, 0x3ea640dd9600000, 0x3ea680de4400000, 0x3ea6c0e03c00000, 0x3ea700e2dc00000,
    0x3ea740ef4e00000, 0x3ea780e46a00000, 0x3ea7c0e55e00000, 0x3ea800e65400000, 0x3ea840e8e200000, 0x3ea880ea0c00000,
    0x3ea8c0ea7600000, 0x3ea900ec3a00000, 0x3ea940ec3e00000, 0x3ea980ed9400000, 0x3ea9c0edb600000, 0x3eaa00ede800000,
    0x3eaa40ee9400000, 0x3eaa80ee8000000, 0x3eaac0f19800000, 0x3eab00f56200000, 0x3eab40f78000000, 0x3eab80f8f600000,
    0x3eabc0fab600000, 0x3eac00fbe800000, 0x3eac40fe7c00000, 0x3eac81000a00000, 0x3eacc106a400000, 0x3ead0107de00000,
    0x3ead410ef200000, 0x3ead81128200000, 0x3eadc1130c00000, 0x3eae01132c00000, 0x3eae41157e00000, 0x3eae8115f000000,
    0x3eaec1159600000, 0x3eaf01160200000, 0x3eaf4115fc00000, 0x3eaf8115da00000, 0x3eafc1167200000, 0x3eb001171400000,
    0x3eb0411a1000000, 0x3eb0811e7000000, 0x3eb0c120e400000, 0x3eb101233200000, 0x3eb14124ec00000, 0x3eb1812cf800000,
    0x3eb1c12dc600000, 0x3eb2012eac00000, 0x3eb2412fb600000, 0x3eb2812ffe00000, 0x3eb2c1301600000, 0x3eb301307600000,
    0x3eb341362400000, 0x3eb3813f3800000, 0x3eb3c4509400000, 0x3eb404508800000, 0x3eb44467aa00000, 0x3eb480773a00000,
    0x3eb4c0803000000, 0x3eb500807200000, 0x3eb544a49200000, 0x3eb584b9a000000, 0x3eb5c4fda600000, 0x3eb6013e8600000,
    0x3eb6413f1c00000, 0x3ec7400bb2005b4, 0x3ec7c00be4005b7, 0x3eca800bd2005c1, 0x3ecac00bd2005c2, 0x3ecb01f692005c1,
    0x3ecb41f692005c2, 0x3ecb800ba0005b7, 0x3ecbc00ba0005b8, 0x3ecc000ba0005bc, 0x3ecc400ba2005bc, 0x3ecc800ba4005bc,
    0x3eccc00ba6005bc, 0x3ecd000ba8005bc, 0x3ecd400baa005bc, 0x3ecd800bac005bc, 0x3ece000bb0005bc, 0x3ece400bb2005bc,
    0x3ece800bb4005bc, 0x3ecec00bb6005bc, 0x3ecf000bb8005bc, 0x3ecf800bbc005bc, 0x3ed0000bc0005bc, 0x3ed0400bc2005bc,
    0x3ed0c00bc6005bc, 0x3ed1000bc8005bc, 0x3ed1800bcc005bc, 0x3ed1c00bce005bc, 0x3ed2000bd0005bc, 0x3ed2400bd2005bc,
    0x3ed2800bd4005bc, 0x3ed2c00baa005b9, 0x3ed3000ba2005bf, 0x3ed3400bb6005bf, 0x3ed3800bc8005bf, 0x4172420ba400307,
    0x4179020bb400307, 0x4426822132110ba, 0x4427022136110ba, 0x442ac2214a110ba, 0x444b82226211127, 0x444bc2226411127,
    0x44d2c2268e1133e, 0x44d302268e11357, 0x44e0c22704113c9, 0x44e1422708113bb, 0x44e3822716113c2, 0x44e4422720113c9,
    0x44f1422784113c2, 0x44f1c22784113b8, 0x44f2022784113c9, 0x452ec22972114ba, 0x452f022972114b0, 0x452f822972114bd,
    0x456e822b70115af, 0x456ec22b72115af, 0x464e02326a11930, 0x584842c23c1611e, 0x584882c23c16129, 0x5848c2c23c1611f,
    0x584902c2521611f, 0x584942c23c16120, 0x584982c2421611f, 0x5849c2c2441611f, 0x584a02c24216120, 0x5b5a02dace16d67,
    0x5b5a42dac616d67, 0x5b5a82dad216d67, 0x745783a2ae1d165, 0x7457c3a2b01d165, 0x745803a2be1d16e, 0x745843a2be1d16f,
    0x745883a2be1d170, 0x7458c3a2be1d171, 0x745903a2be1d172, 0x746ec3a3721d165, 0x746f03a3741d165, 0x746f43a3761d16e,
    0x746f83a3781d16e, 0x746fc3a3761d16f, 0x747003a3781d16f, 0xbe00009c7a00000, 0xbe00409c7000000, 0xbe00809c8200000,
    0xbe00c4024400000, 0xbe01009ec000000, 0xbe01409f5c00000, 0xbe01809f7600000, 0xbe01c0a00400000, 0xbe0200a0f400000,
    0xbe0240a13200000, 0xbe0280a1ce00000, 0xbe02c0a19e00000, 0xbe0300693c00000, 0xbe03440c7400000, 0xbe0380a29a00000,
    0xbe03c0a2a800000, 0xbe0400a2c800000, 0xbe0440a2ee00000, 0xbe04840a3800000, 0xbe04c0697200000, 0xbe0500a2ce00000,
    0xbe0540a31a00000, 0xbe05840a9600000, 0xbe05c0a32e00000, 0xbe0600a34800000, 0xbe06409d9800000, 0xbe0680a35800000,
    0xbe06c0a36a00000, 0xbe070523be00000, 0xbe0740a3ea00000, 0xbe0780a40600000, 0xbe07c069be00000, 0xbe0800a47600000,
    0xbe0840a48c00000, 0xbe0880a4e400000, 0xbe08c0a4ee00000, 0xbe09006a2a00000, 0xbe0940a58e00000, 0xbe0980a59200000,
    0xbe09c0a5c800000, 0xbe0a00a5f400000, 0xbe0a40a60a00000, 0xbe0a80a60c00000, 0xbe0ac0a62e00000, 0xbe0b00a69200000,
    0xbe0b40a6a200000, 0xbe0b80a6b400000, 0xbe0bc0a6e600000, 0xbe0c00a6fa00000, 0xbe0c40a6fe00000, 0xbe0c80a6fe00000,
    0xbe0cc0a6fe00000, 0xbe0d04145800000, 0xbe0d40e0e000000, 0xbe0d80a79400000, 0xbe0dc0a7be00000, 0xbe0e0416c600000,
    0xbe0e40a7d600000, 0xbe0e80a7e200000, 0xbe0ec0a80c00000, 0xbe0f00a93c00000, 0xbe0f40a87000000, 0xbe0f80a89000000,
    0xbe0fc0a8d000000, 0xbe1000a94400000, 0xbe1040a9ec00000, 0xbe1080aa2000000, 0xbe10c0aaa600000, 0xbe1100aac600000,
    0xbe1140ab0800000, 0xbe1180ab0800000, 0xbe11c0ab3200000, 0xbe1200ab5600000, 0xbe1240ab6600000, 0xbe1280ab8400000,
    0xbe12c0ae2c00000, 0xbe1300ac0c00000, 0xbe1340ae2e00000, 0xbe1380aca200000, 0xbe13c0ace800000, 0xbe1400a40e00000,
    0xbe1440b1dc00000, 0xbe1480af9c00000, 0xbe14c0afe800000, 0xbe1500b01a00000, 0xbe1540af1600000, 0xbe1580b06400000,
    0xbe15c0b06200000, 0xbe1600b15800000, 0xbe164429c800000, 0xbe1680b1e400000, 0xbe16c0b1ee00000, 0xbe1700b20c00000,
    0xbe1740b23400000, 0xbe1780b24400000, 0xbe17c0b2c400000, 0xbe18042d5000000, 0xbe18442dd400000, 0xbe1880b3d800000,
    0xbe18c0b43600000, 0xbe1900b44e00000, 0xbe1940b3b000000, 0xbe1980b4cc00000, 0xbe19c06ddc00000, 0xbe1a006df800000,
    0xbe1a40b61000000, 0xbe1a80b67c00000, 0xbe1ac0b67c00000, 0xbe1b04339000000, 0xbe1b40b78600000, 0xbe1b80b7b000000,
    0xbe1bc0b7ce00000, 0xbe1c00b7e600000, 0xbe1c44363000000, 0xbe1c80b7fe00000, 0xbe1cc0b80c00000, 0xbe1d00bea600000,
    0xbe1d40b84400000, 0xbe1d806f0200000, 0xbe1dc0b8c000000, 0xbe1e00b8dc00000, 0xbe1e40b98000000, 0xbe1e80b91a00000,
    0xbe1ec43bc800000, 0xbe1f00ba8600000, 0xbe1f443bcc00000, 0xbe1f80badc00000, 0xbe1fc0bad600000, 0xbe2000baf800000,
    0xbe2040bbc200000, 0xbe2080bbc400000, 0xbe20c0705e00000, 0xbe2100bbfa00000, 0xbe2140bc5000000, 0xbe2180bc7a00000,
    0xbe21c0bcd200000, 0xbe220070c400000, 0xbe2244430600000, 0xbe228070f800000, 0xbe22c0bd6000000, 0xbe2300bd6600000,
    0xbe2340bd6c00000, 0xbe2380bd9400000, 0xbe23c5472400000, 0xbe2400bdfc00000, 0xbe2444466200000, 0xbe2484466200000,
    0xbe24c1040200000, 0xbe2500be4400000, 0xbe2540be4400000, 0xbe2580718e00000, 0xbe25c4657000000, 0xbe2604c3b400000,
    0xbe2640bec400000, 0xbe2680bed600000, 0xbe26c071c600000, 0xbe2700bf3400000, 0xbe2740bf9a00000, 0xbe2780bfae00000,
    0xbe27c0bff200000, 0xbe2800c10200000, 0xbe2840727400000, 0xbe2880723800000, 0xbe28c0c12800000, 0xbe29044da800000,
    0xbe2940c18e00000, 0xbe2980c29000000, 0xbe29c0c29800000, 0xbe2a00c29c00000, 0xbe2a40c29800000, 0xbe2a80c2f400000,
    0xbe2ac0c31c00000, 0xbe2b00c36400000, 0xbe2b40c34800000, 0xbe2b80c35e00000, 0xbe2bc0c3bc00000, 0xbe2c00c3e400000,
    0xbe2c40c3ec00000, 0xbe2c80c42000000, 0xbe2cc0c43600000, 0xbe2d00c4ba00000, 0xbe2d40c56200000, 0xbe2d80c5a800000,
    0xbe2dc0c6a000000, 0xbe2e04561800000, 0xbe2e40c67a00000, 0xbe2e80c5f800000, 0xbe2ec0c6d000000, 0xbe2f00c70600000,
    0xbe2f40c7c800000, 0xbe2f8457e200000, 0xbe2fc0c84400000, 0xbe3000c78a00000, 0xbe3040c75200000, 0xbe3080745c00000,
    0xbe30c0c8d200000, 0xbe3100c8fc00000, 0xbe3140c93a00000, 0xbe3180c8ee00000, 0xbe31c074d800000, 0xbe3200ca9e00000,
    0xbe3240cad800000, 0xbe3284601400000, 0xbe32c0cbc600000, 0xbe3300cdf000000, 0xbe3340cc9200000, 0xbe3380763200000,
    0xbe33c0cd2200000, 0xbe3400761000000, 0xbe344075c800000, 0xbe3480a32400000, 0xbe34c0a32a00000, 0xbe3500ce0000000,
    0xbe3540cd3800000, 0xbe3581015a00000, 0xbe35c087b200000, 0xbe3600ce2e00000, 0xbe3640ce3600000, 0xbe3680ce4200000,
    0xbe36c0cebc00000, 0xbe3700cea600000, 0xbe3744678600000, 0xbe3780769200000, 0xbe37c0cff400000, 0xbe3800cf0a00000,
    0xbe3840d0a400000, 0xbe3880d10a00000, 0xbe38c468da00000, 0xbe3900d11c00000, 0xbe3940d03e00000, 0xbe3980d22800000,
    0xbe39c0773a00000, 0xbe3a00d28400000, 0xbe3a40d34600000, 0xbe3a80d3d400000, 0xbe3ac0d55000000, 0xbe3b046d4600000,
    0xbe3b40d5b600000, 0xbe3b80783000000, 0xbe3bc0d64200000, 0xbe3c04714e00000, 0xbe3c40d6a800000, 0xbe3c80789c00000,
    0xbe3cc0d6e400000, 0xbe3d00d73e00000, 0xbe3d40d77400000, 0xbe3d80d77600000, 0xbe3dc4751a00000, 0xbe3e043a1600000,
    0xbe3e4475f400000, 0xbe3e80d89c00000, 0xbe3ec4797800000, 0xbe3f00d97e00000, 0xbe3f40d99a00000, 0xbe3f80d8ce00000,
    0xbe3fc0da2c00000, 0xbe4000da7c00000, 0xbe4040daee00000, 0xbe4080da8200000, 0xbe40c0dad200000, 0xbe4100daf000000,
    0xbe4140db0a00000, 0xbe41847a3c00000, 0xbe41c0da6800000, 0xbe4200dc5e00000, 0xbe4240dcdc00000, 0xbe42807a6600000,
    0xbe42c0dd9600000, 0xbe4300dd8e00000, 0xbe43447da200000, 0xbe4380dbf200000, 0xbe43c0dedc00000, 0xbe44047ebc00000,
    0xbe44447f1c00000, 0xbe4480df8c00000, 0xbe44c0e07200000, 0xbe4500e03c00000, 0xbe4540e03600000, 0xbe45807b2c00000,
    0xbe45c0e09400000, 0xbe4600e0fa00000, 0xbe4640e0ee00000, 0xbe4680e15a00000, 0xbe46c40a4a00000, 0xbe4700e28a00000,
    0xbe474484c600000, 0xbe4780e33800000, 0xbe47c4875600000, 0xbe4800e45000000, 0xbe4840e46a00000, 0xbe4880e4a000000,
    0xbe48c48c1000000, 0xbe4900e50000000, 0xbe4940e52a00000, 0xbe49848e6a00000, 0xbe49c4902800000, 0xbe4a00e6f400000,
    0xbe4a40e71600000, 0xbe4a807d5800000, 0xbe4ac0e74a00000, 0xbe4b007d7000000, 0xbe4b407d7000000, 0xbe4b80e88e00000,
    0xbe4bc0e8b800000, 0xbe4c00e8e200000, 0xbe4c40e90a00000, 0xbe4c80e99400000, 0xbe4cc07e3600000, 0xbe4d00ea4800000,
    0xbe4d44986c00000, 0xbe4d80ea7c00000, 0xbe4dc4992400000, 0xbe4e00eae000000, 0xbe4e44433e00000, 0xbe4e80ec2000000,
    0xbe4ec49f4200000, 0xbe4f049f7000000, 0xbe4f44a08800000, 0xbe4f807ff800000, 0xbe4fc0801000000, 0xbe5000ede800000,
    0xbe5044a1e600000, 0xbe5084a1e400000, 0xbe50c4a23200000, 0xbe5104a26600000, 0xbe5140ee3c00000, 0xbe5180ee3e00000,
    0xbe51c0ee3e00000, 0xbe5200ee9400000, 0xbe5240807200000, 0xbe5280ef1600000, 0xbe52c0808c00000, 0xbe5300812c00000,
    0xbe5344a83a00000, 0xbe5380f09c00000, 0xbe53c0f11800000, 0xbe5400f19800000, 0xbe544081c600000, 0xbe5484ac4c00000,
    0xbe54c0f2ac00000, 0xbe5504ad3400000, 0xbe5544ad8a00000, 0xbe5580f31e00000, 0xbe55c0f3d600000, 0xbe5600825e00000,
    0xbe5640f48000000, 0xbe5680f49400000, 0xbe56c0f49e00000, 0xbe5704b2f800000, 0xbe5744b54e00000, 0xbe5784b54e00000,
    0xbe57c0f5dc00000, 0xbe5800840400000, 0xbe5844b75600000, 0xbe5880f78c00000, 0xbe58c0f79200000, 0xbe5900844e00000,
    0xbe5944b90000000, 0xbe5980f9a400000, 0xbe59c0854000000, 0xbe5a00f9d000000, 0xbe5a40f9c600000, 0xbe5a80fa0000000,
    0xbe5ac4bf0c00000, 0xbe5b00fac600000, 0xbe5b40860200000, 0xbe5b80fb8e00000, 0xbe5bc0fc0400000, 0xbe5c00fc8a00000,
    0xbe5c40866800000, 0xbe5c84c45000000, 0xbe5cc4c48e00000, 0xbe5d0086b200000, 0xbe5d44c5b200000, 0xbe5d80fef400000,
    0xbe5dc4c67c00000, 0xbe5e00ff2a00000, 0xbe5e40fff400000, 0xbe5e81000a00000, 0xbe5ec4c9b400000, 0xbe5f04ca4600000,
    0xbe5f4100c000000, 0xbe5f84cb5000000, 0xbe5fc100e000000, 0xbe600466be00000, 0xbe604087aa00000, 0xbe6081016400000,
    0xbe60c1020600000, 0xbe6100881600000, 0xbe6141027c00000, 0xbe6180b56a00000, 0xbe61c4cf4e00000, 0xbe6204cf6a00000,
    0xbe6244672600000, 0xbe6284673800000, 0xbe62c1040200000, 0xbe6301040800000, 0xbe63411f3c00000, 0xbe638088d600000,
    0xbe63c1052200000, 0xbe6401051600000, 0xbe6441053a00000, 0xbe6480a56600000, 0xbe64c1056200000, 0xbe6501056600000,
    0xbe6541057a00000, 0xbe658105cc00000, 0xbe65c4d67800000, 0xbe660105ca00000, 0xbe6641063a00000, 0xbe668106c600000,
    0xbe66c1075a00000, 0xbe6701064600000, 0xbe6741077a00000, 0xbe678107ce00000, 0xbe67c108ae00000, 0xbe680106a600000,
    0xbe6841079400000, 0xbe6881079800000, 0xbe68c107b800000, 0xbe6904d86c00000, 0xbe6944dad600000, 0xbe6984d9aa00000,
    0xbe69c08a5600000, 0xbe6a0109e200000, 0xbe6a4109e600000, 0xbe6a810a2c00000, 0xbe6ac4e79400000, 0xbe6b010ac800000,
    0xbe6b44de5800000, 0xbe6b808aba00000, 0xbe6bc08ac200000, 0xbe6c04df6200000, 0xbe6c44e1a400000, 0xbe6c808ad600000,
    0xbe6cc10ca000000, 0xbe6d010cb800000, 0xbe6d410cce00000, 0xbe6d810cd200000, 0xbe6dc10d5200000, 0xbe6e010d1000000,
    0xbe6e410e1c00000, 0xbe6e810dc400000, 0xbe6ec10ef200000, 0xbe6f010e5000000, 0xbe6f410ed600000, 0xbe6f810f0c00000,
    0xbe6fc08bae00000, 0xbe70010fc200000, 0xbe7041100200000, 0xbe70808bf200000, 0xbe70c110c000000, 0xbe710110c600000,
    0xbe7144ecce00000, 0xbe718111ae00000, 0xbe71c111bc00000, 0xbe72008c6a00000, 0xbe724111f400000, 0xbe7280697600000,
    0xbe72c4f15c00000, 0xbe7304f2cc00000, 0xbe73408d7c00000, 0xbe73808d8e00000, 0xbe73c1154000000, 0xbe740115da00000,
    0xbe7441171400000, 0xbe748118aa00000, 0xbe74c4f95000000, 0xbe7501195600000, 0xbe7541198200000, 0xbe75811a3600000,
    0xbe75c11aee00000, 0xbe7604fe5e00000, 0xbe7644100800000, 0xbe76811b9600000, 0xbe76c11b7800000, 0xbe77011be000000,
    0xbe774411bc00000, 0xbe77811da800000, 0xbe77c11e7000000, 0xbe78050ba400000, 0xbe78450bda00000, 0xbe7881212800000,
    0xbe78c121e200000, 0xbe7901222200000, 0xbe79450e5c00000, 0xbe7981223600000, 0xbe79c1247000000, 0xbe7a0125ae00000,
    0xbe7a4125b000000, 0xbe7a8124f800000, 0xbe7ac127f200000, 0xbe7b01282a00000, 0xbe7b4517f400000, 0xbe7b812b1600000,
    0xbe7bc0932a00000, 0xbe7c012b6e00000, 0xbe7c451aee00000, 0xbe7c8093cc00000, 0xbe7cc12d8600000, 0xbe7d00bb6400000,
    0xbe7d412e4600000, 0xbe7d85228a00000, 0xbe7dc5243400000, 0xbe7e0094dc00000, 0xbe7e4094ec00000, 0xbe7e812fc000000,
    0xbe7ec5281400000, 0xbe7f00956400000, 0xbe7f45292c00000, 0xbe7f81301600000, 0xbe7fc1301600000, 0xbe8001305200000,
    0xbe80452b6c00000, 0xbe808131c400000, 0xbe80c0966600000, 0xbe8101325200000, 0xbe8141334e00000, 0xbe8181338400000,
    0xbe81c133fc00000, 0xbe8200979c00000, 0xbe8245366000000, 0xbe8281362400000, 0xbe82c1388000000, 0xbe830139fa00000,
    0xbe8340999c00000, 0xbe838099da00000, 0xbe83c13ace00000, 0xbe8405419c00000, 0xbe844099f000000, 0xbe8485420a00000,
    0xbe84c5441c00000, 0xbe8505452200000, 0xbe85413d7600000, 0xbe85809aac00000, 0xbe85c13df200000, 0xbe86013dfc00000,
    0xbe86413e0a00000, 0xbe86813e1e00000, 0xbe86c13e2c00000, 0xbe87013e7600000, 0xbe87454c0000000,
];
/// Which of those recompose (by index), sorted by their two parts: not composition exclusions,
/// nor non-starter decompositions.
#[rustfmt::skip]
const JOINS: [u16; 961] = [
    912, 909, 913, 0, 1, 2, 3, 53, 55, 238, 4, 572, 5, 165, 208, 210, 570, 415, 57, 417,
    419, 421, 59, 61, 63, 65, 6, 425, 67, 427, 431, 433, 429, 7, 8, 9, 598, 69, 71, 73,
    10, 596, 77, 212, 214, 594, 240, 75, 439, 441, 445, 198, 79, 447, 81, 83, 187, 85, 87, 449,
    453, 236, 451, 455, 457, 11, 12, 13, 89, 91, 93, 97, 14, 610, 167, 216, 218, 612, 95, 459,
    98, 463, 189, 465, 100, 467, 102, 106, 469, 104, 475, 473, 477, 479, 481, 200, 108, 15, 483, 112,
    485, 110, 489, 487, 16, 17, 18, 19, 114, 116, 246, 20, 616, 118, 169, 220, 222, 161, 614, 191,
    499, 501, 120, 503, 124, 224, 226, 505, 122, 509, 126, 128, 511, 132, 513, 232, 130, 521, 136, 523,
    234, 134, 527, 525, 21, 22, 23, 138, 140, 142, 24, 640, 144, 146, 171, 228, 230, 163, 638, 529,
    148, 533, 531, 539, 541, 543, 545, 150, 549, 547, 551, 553, 555, 652, 25, 152, 658, 250, 557, 154,
    656, 654, 155, 559, 157, 159, 561, 563, 26, 27, 28, 29, 54, 56, 239, 30, 573, 31, 166, 209,
    211, 571, 416, 58, 418, 420, 422, 60, 62, 64, 66, 32, 426, 68, 428, 432, 434, 430, 33, 34,
    35, 599, 70, 72, 74, 36, 597, 78, 213, 215, 595, 241, 76, 440, 442, 446, 199, 80, 448, 82,
    84, 188, 86, 88, 450, 454, 237, 452, 456, 458, 565, 37, 38, 39, 90, 92, 94, 40, 611, 168,
    217, 219, 613, 96, 460, 99, 197, 464, 190, 466, 101, 468, 103, 107, 470, 105, 476, 474, 478, 480,
    482, 201, 109, 41, 484, 113, 486, 111, 490, 488, 42, 43, 44, 45, 115, 117, 247, 46, 617, 119,
    170, 221, 223, 162, 615, 192, 500, 502, 121, 504, 125, 225, 227, 506, 123, 510, 127, 129, 512, 133,
    514, 233, 131, 522, 566, 137, 524, 235, 135, 528, 526, 47, 48, 49, 139, 141, 143, 50, 641, 145,
    147, 172, 229, 231, 164, 639, 530, 149, 534, 532, 540, 542, 544, 546, 151, 550, 548, 567, 552, 554,
    556, 653, 51, 153, 659, 251, 558, 52, 657, 568, 655, 156, 560, 158, 160, 562, 564, 875, 258, 835,
    576, 574, 580, 578, 181, 202, 204, 185, 423, 602, 600, 606, 604, 461, 620, 618, 624, 622, 491, 244,
    493, 242, 206, 179, 175, 173, 177, 577, 575, 581, 579, 182, 203, 205, 186, 424, 603, 601, 607, 605,
    462, 621, 619, 625, 623, 492, 245, 494, 243, 207, 180, 176, 174, 178, 586, 584, 590, 588, 587, 585,
    591, 589, 435, 437, 436, 438, 495, 497, 496, 498, 515, 516, 517, 518, 535, 536, 537, 538, 569, 630,
    628, 634, 632, 636, 631, 629, 635, 633, 637, 644, 642, 648, 646, 650, 645, 643, 649, 647, 651, 195,
    193, 194, 183, 184, 443, 444, 248, 249, 196, 831, 259, 830, 829, 668, 669, 833, 841, 261, 682, 683,
    843, 262, 696, 697, 845, 857, 263, 856, 855, 268, 712, 713, 883, 264, 726, 727, 874, 872, 265, 871,
    870, 269, 740, 885, 266, 752, 753, 887, 826, 838, 760, 270, 823, 822, 660, 661, 827, 825, 762, 271,
    676, 677, 764, 272, 688, 689, 839, 837, 766, 273, 850, 849, 275, 704, 705, 853, 768, 277, 720, 721,
    866, 867, 770, 278, 863, 862, 276, 732, 733, 868, 772, 279, 744, 745, 881, 879, 851, 267, 854, 864,
    274, 869, 880, 280, 281, 285, 302, 304, 284, 282, 306, 283, 300, 310, 312, 287, 314, 289, 316, 286,
    318, 324, 288, 326, 328, 330, 332, 322, 303, 305, 293, 291, 307, 292, 301, 311, 313, 296, 315, 290,
    317, 295, 319, 325, 297, 327, 329, 331, 333, 323, 294, 298, 299, 308, 309, 320, 321, 334, 335, 337,
    336, 338, 340, 341, 339, 342, 343, 344, 353, 354, 365, 364, 366, 369, 370, 372, 371, 373, 374, 377,
    375, 376, 378, 379, 381, 380, 382, 383, 385, 384, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412,
    413, 414, 471, 472, 507, 508, 519, 520, 582, 592, 583, 593, 608, 609, 626, 627, 662, 664, 666, 774,
    663, 665, 667, 775, 776, 777, 778, 779, 780, 781, 670, 672, 674, 782, 671, 673, 675, 783, 784, 785,
    786, 787, 788, 789, 678, 680, 679, 681, 684, 686, 685, 687, 690, 692, 694, 790, 691, 693, 695, 791,
    792, 793, 794, 795, 796, 797, 698, 700, 702, 798, 699, 701, 703, 799, 800, 801, 802, 803, 804, 805,
    706, 708, 710, 707, 709, 711, 714, 716, 718, 715, 717, 719, 722, 724, 723, 725, 728, 730, 729, 731,
    734, 736, 738, 735, 737, 739, 741, 742, 743, 746, 748, 750, 806, 747, 749, 751, 807, 808, 809, 810,
    811, 812, 813, 754, 756, 758, 814, 755, 757, 759, 815, 816, 817, 818, 819, 820, 821, 824, 836, 878,
    828, 846, 847, 848, 840, 882, 859, 860, 861, 894, 895, 896, 897, 899, 898, 900, 901, 902, 903, 904,
    905, 906, 907, 908, 911, 910, 914, 915, 916, 917, 918, 919, 920, 921, 930, 931, 922, 923, 924, 925,
    932, 933, 926, 927, 928, 929, 934, 935, 936, 937, 966, 941, 942, 943, 944, 945, 946, 947, 948, 949,
    950, 951, 952, 953, 954, 955, 956, 957, 958, 959, 960, 961, 962, 963, 964, 965, 967, 993, 968, 969,
    970, 971, 972, 973, 974, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 987, 988, 989,
    990, 991, 992, 994, 995, 996, 997, 998, 1493, 1494, 1495, 1496, 1497, 1498, 1499, 1500, 1501, 1502, 1503, 1504,
    1505, 1507, 1506, 1508, 1510, 1509, 1511, 1512, 1513, 1514, 1515, 1517, 1519, 1516, 1520, 1522, 1521, 1518, 1524, 1523,
    1525,
];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::font::tests::{file, glyph, longs, tables_of, with, words};

    /// A 1024-unit font with a 500-wide box for each of these characters (in order): glyph n +
    /// 1 for the nth.
    pub(crate) fn font_of(letters: &str) -> FontFile {
        let letters: Vec<char> = letters.chars().collect();
        assert!(letters.is_sorted(), "{letters:?}");
        let count = letters.len() as i32 + 1;
        let boxed = glyph(&[&[(0, 0, true), (0, 500, true), (400, 500, true), (400, 0, true)]]);
        let groups: Vec<u8> = letters.iter().enumerate().flat_map(|(n, letter)| longs(&[*letter as u32, *letter as u32, n as u32 + 1])).collect();
        let cmap = [words(&[0, 1, 3, 10]), longs(&[12]), words(&[12, 0]), longs(&[16 + groups.len() as u32, 0, letters.len() as u32]), groups].concat();
        let mut tables = with(tables_of(vec![boxed; count as usize]), b"cmap", cmap);
        tables = with(tables, b"maxp", [words(&[1, 0, count]), vec![0; 26]].concat());
        tables = with(tables, b"hhea", [words(&[1, 0, 832, -192]), vec![0; 26], words(&[count])].concat());
        tables = with(tables, b"hmtx", words(&(0..count).flat_map(|_| [500, 0]).collect::<Vec<_>>()));
        FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read")
    }

    /// What a font gets of some text, under a script's rules: the characters, in order.
    fn handed(font: &FontFile, text: &str, way: Way) -> String {
        let letters: Vec<(char, u32)> = text.chars().zip(0..).collect();
        normalized(font, &letters, &mut Vec::new(), way).0.iter().map(|(letter, _)| *letter).collect()
    }

    #[test]
    fn characters_are_made_of_what_unicode_says() {
        assert_eq!(split('é'), Some(('e', Some('\u{301}'))));
        assert_eq!(split('\u{1e09}'), Some(('ç', Some('\u{301}'))));
        assert_eq!(split('ç'), Some(('c', Some('\u{327}'))));
        // A Hangul syllable, with and without a trailing jamo; singletons (the angstrom sign, a
        // CJK compatibility ideograph); one excluded from recomposition (Devanagari qa) and one
        // made of two marks.
        assert_eq!((split('각'), split('가')), (Some(('가', Some('\u{11a8}'))), Some(('\u{1100}', Some('\u{1161}')))));
        assert_eq!((split('\u{212b}'), split('\u{f900}')), (Some(('Å', None)), Some(('\u{8c48}', None))));
        assert_eq!((split('\u{958}'), joined('\u{915}', '\u{93c}')), (Some(('\u{915}', Some('\u{93c}'))), None));
        assert_eq!((split('\u{344}'), joined('\u{308}', '\u{301}')), (Some(('\u{308}', Some('\u{301}'))), None));
        assert_eq!((split('a'), split('\u{10ffff}')), (None, None));
        assert_eq!((joined('e', '\u{301}'), joined('\u{1100}', '\u{1161}'), joined('가', '\u{11a8}')), (Some('é'), Some('가'), Some('각')));
        assert_eq!((joined('가', '\u{11a7}'), joined('e', 'e'), joined('\u{301}', 'e')), (None, None, None));
        // HarfBuzz's classes: Hebrew sheva and meteg, Arabic shadda, Telugu length mark, Tai
        // Tham sakot; Unicode's elsewhere.
        assert_eq!(['\u{301}', '\u{323}', '\u{5b0}', '\u{5bd}', '\u{651}', '\u{c55}', '\u{1a60}', 'a', '\u{903}'].map(class), [230, 220, 22, 25, 27, 4, 254, 0, 0]);
        assert_eq!(['\u{301}', '\u{903}', '\u{20dd}', 'a', '\u{34f}', ' '].map(mark), [true, true, true, false, true, false]);
    }

    #[test]
    fn every_character_put_together_is_taken_apart_into_the_same_two() {
        let (mut wholes, mut joins) = (0, 0);
        for whole in (0..0x110000).filter_map(char::from_u32) {
            let Some((first, second)) = split(whole) else { continue };
            wholes += 1;
            // Recomposition gives back the very character, and only a starter and a second part
            // compose.
            if let Some(again) = second.and_then(|second| joined(first, second)) {
                assert_eq!(again, whole, "{:x}", whole as u32);
                assert!(class(first) == 0 && class(whole) == 0, "{:x}", whole as u32);
                joins += 1;
            }
        }
        // Unicode 17: 2,081 decompositions besides Hangul's 11,172 syllables (all of which
        // recompose), 961 of them recomposing.
        assert_eq!((wholes, joins), (2081 + 11_172, 961 + 11_172));
    }

    #[test]
    fn a_letter_and_its_accents_are_the_one_character_the_font_has_for_them() {
        // The font has "é": "e" plus acute becomes it. It lacks it: "é" is decomposed.
        assert_eq!(handed(&font_of("e\u{e9}\u{301}"), "e\u{301}", Way::Usual), "\u{e9}");
        assert_eq!(handed(&font_of("e\u{301}"), "\u{e9}", Way::Usual), "e\u{301}");
        // It has neither the accent nor the "e": "é" stays, and "e" plus accent composes into
        // it.
        assert_eq!((handed(&font_of("\u{e9}"), "\u{e9}", Way::Usual), handed(&font_of("\u{e9}"), "e\u{301}", Way::Usual)), ("\u{e9}".into(), "\u{e9}".into()));
        // It has none of it: unchanged.
        assert_eq!(handed(&font_of("a"), "e\u{301}", Way::Usual), "e\u{301}");
        // Decomposed as far as it has the parts: "ḉ" is "ç" plus acute, and "ç" is "c" plus
        // cedilla.
        assert_eq!(handed(&font_of("c\u{e7}\u{301}"), "\u{1e09}", Way::Usual), "\u{e7}\u{301}");
        assert_eq!(handed(&font_of("c\u{301}\u{327}"), "\u{1e09}", Way::Usual), "c\u{327}\u{301}");
        // A lone letter the font has stays whole, whatever parts it also has.
        assert_eq!(handed(&font_of("c\u{e7}\u{301}\u{327}\u{1e09}"), "\u{1e09}", Way::Usual), "\u{1e09}");
    }

    #[test]
    fn marks_are_put_in_their_order_then_together_as_far_as_the_font_goes() {
        let font = font_of("a\u{302}\u{323}\u{1ea1}\u{1ead}");
        // A dot below sorts before a circumflex above, whichever came first, and the three
        // compose into one character the font has.
        assert_eq!(handed(&font, "a\u{302}\u{323}", Way::Usual), "\u{1ead}");
        assert_eq!(handed(&font, "a\u{323}\u{302}", Way::Usual), "\u{1ead}");
        // Two marks above keep their order: acute then circumflex is "á" plus circumflex, not
        // "ấ" ("â" plus acute); and without "â", not that either.
        let font = font_of("a\u{e1}\u{e2}\u{301}\u{302}\u{1ea5}");
        assert_eq!(handed(&font, "a\u{301}\u{302}", Way::Usual), "\u{e1}\u{302}");
        assert_eq!(handed(&font, "a\u{302}\u{301}", Way::Usual), "\u{1ea5}");
        assert_eq!(handed(&font_of("a\u{e1}\u{301}\u{302}\u{1ea5}"), "a\u{302}\u{301}", Way::Usual), "a\u{302}\u{301}");
        // A mark of lower class in between blocks another from its letter; one of the same
        // class does too.
        let font = font_of("a\u{e1}\u{301}\u{316}");
        assert_eq!(handed(&font, "a\u{316}\u{301}", Way::Usual), "\u{e1}\u{316}");
        assert_eq!(handed(&font, "a\u{301}\u{301}", Way::Usual), "\u{e1}\u{301}");
        // More than 32 marks keep their order.
        let many = format!("a{}\u{323}", "\u{302}".repeat(32));
        assert_eq!(handed(&font_of("a\u{302}\u{323}"), &many, Way::Usual), many);
    }

    #[test]
    fn what_the_font_lacks_is_taken_apart_into_what_it_has() {
        // The angstrom sign is "Å", or "A" plus ring.
        assert_eq!(handed(&font_of("A\u{c5}"), "\u{212b}", Way::Usual), "\u{c5}");
        assert_eq!(handed(&font_of("A\u{30a}"), "\u{212b}", Way::Usual), "A\u{30a}");
        // A CJK compatibility ideograph is its unified ideograph.
        assert_eq!(handed(&font_of("\u{8c48}"), "\u{f900}", Way::Usual), "\u{8c48}");
        // A Hangul syllable the font lacks is its jamo, and does not recompose.
        let letters = font_of("\u{1100}\u{1161}\u{11a8}");
        assert_eq!((handed(&letters, "각", Way::Hangul), handed(&letters, "\u{1100}\u{1161}", Way::Hangul)), ("\u{1100}\u{1161}\u{11a8}".into(), "\u{1100}\u{1161}".into()));
        // Its jamo become the syllable where the font has it (and with a trailing jamo, the
        // syllable with both); the syllable decomposes for a trailing jamo it lacks combined.
        let syllables = font_of("\u{1100}\u{1161}\u{11a8}\u{11a9}\u{ac00}\u{ac01}");
        assert_eq!(handed(&syllables, "\u{1100}\u{1161}", Way::Hangul), "\u{ac00}");
        assert_eq!(handed(&syllables, "\u{1100}\u{1161}\u{11a8}", Way::Hangul), "\u{ac01}");
        assert_eq!(handed(&syllables, "\u{ac00}\u{11a8}", Way::Hangul), "\u{ac01}");
        assert_eq!(handed(&syllables, "\u{ac00}\u{11a9}", Way::Hangul), "\u{1100}\u{1161}\u{11a9}");
        // A tone mark moves before the syllable it follows.
        assert_eq!(handed(&font_of("\u{302e}\u{ac00}"), "\u{ac00}\u{302e}", Way::Hangul), "\u{302e}\u{ac00}");
    }

    #[test]
    fn hebrew_and_arabic_marks_take_the_order_harfbuzz_gives_them() {
        // Alef with qamats, sheva and meteg: meteg before sheva, as Hebrew fonts expect.
        let hebrew = font_of("\u{5b0}\u{5b8}\u{5bd}\u{5d0}");
        assert_eq!(handed(&hebrew, "\u{5d0}\u{5b0}\u{5bd}\u{5b8}", Way::Hebrew { forms: false }), "\u{5d0}\u{5b8}\u{5bd}\u{5b0}");
        // Beh with fatha and hamza above: the hamza first, as it modifies the letter.
        let arabic = font_of("\u{628}\u{64e}\u{654}");
        assert_eq!(handed(&arabic, "\u{628}\u{64e}\u{654}", Way::Arabic), "\u{628}\u{654}\u{64e}");
        assert_eq!(handed(&arabic, "\u{628}\u{64e}\u{654}", Way::Usual), "\u{628}\u{64e}\u{654}");
        // In a font without mark positioning, shin with shin dot becomes the older presentation
        // form.
        let forms = font_of("\u{5c1}\u{5e9}\u{fb2a}");
        assert_eq!(handed(&forms, "\u{5e9}\u{5c1}", Way::Hebrew { forms: true }), "\u{fb2a}");
        assert_eq!(handed(&forms, "\u{5e9}\u{5c1}", Way::Hebrew { forms: false }), "\u{5e9}\u{5c1}");
    }

    #[test]
    fn a_letter_with_a_variation_selector_is_left_whole() {
        // The font lacks "é" with a selector after it: nothing in it is decomposed.
        let font = font_of("e\u{301}");
        assert_eq!(handed(&font, "\u{e9}\u{fe00}\u{301}", Way::Usual), "\u{e9}\u{fe00}\u{301}");
        // A letter with its own glyph for its selector keeps it, and is not decomposed;
        // composed with a following mark, it loses it, as in HarfBuzz.
        let mut variants = vec![(0, 7)];
        let (handed, _) = normalized(&font_of("e\u{301}"), &[('\u{e9}', 0)], &mut variants, Way::Usual);
        assert_eq!((handed, &variants), (vec![('\u{e9}', 0)], &vec![(0, 7)]));
        let (handed, _) = normalized(&font_of("e\u{e9}\u{301}"), &[('e', 0), ('\u{301}', 2)], &mut variants, Way::Usual);
        assert_eq!((handed, variants), (vec![('\u{e9}', 0)], vec![]));
    }

    #[test]
    fn spaces_the_font_lacks_are_its_own_space_as_wide_as_they_are() {
        use crate::shape::{Plan, Script};
        let font = font_of(" ,.0\u{2010}");
        let plan = Plan::of(&font, Script::None, false, crate::scripts::Language::NONE);
        let mut glyphs = Vec::new();
        // An em space, a thin space (1/5 em), a figure space (as wide as "0"), a narrow
        // no-break space (half the space), a no-break space (the space itself).
        let wide = font.shape(&plan, "\u{2003}\u{2009}\u{2007}\u{202f}\u{a0}", &mut glyphs);
        let space = font.glyph(' ');
        assert!(glyphs.iter().all(|laid| laid.glyph == space));
        assert_eq!(glyphs.iter().map(|laid| laid.advance).collect::<Vec<_>>(), [1024, 205, 500, 250, 500]);
        assert_eq!(wide, 2479);
        // The non-breaking hyphen becomes the plain one; what has no substitute is the font's
        // notdef box.
        font.shape(&plan, "\u{2011}\u{2028}", &mut glyphs);
        assert_eq!(glyphs.iter().map(|laid| laid.glyph).collect::<Vec<_>>(), [font.glyph('\u{2010}'), 0]);
    }
}

/// Compared with HarfBuzz, on Linux: words of letters and marks of every kind, in many fonts,
/// some built to lack a character or its parts.
#[cfg(all(test, target_os = "linux"))]
mod held {
    use super::tests::font_of;
    use super::*;

    /// A character fully decomposed (the first part recursively, the second parts after it),
    /// and each step between: from the whole down.
    fn ways(letter: char) -> Vec<String> {
        let mut ways = vec![letter.to_string()];
        let (mut first, mut seconds) = (letter, Vec::new());
        while let Some((part, second)) = split(first) {
            seconds.extend(second);
            first = part;
            ways.push(std::iter::once(first).chain(seconds.iter().rev().copied()).collect());
        }
        ways
    }

    /// The words: every decomposable character, as is and decomposed; marks of different
    /// classes in every order after letters of several scripts; Greek, Vietnamese, Hebrew and
    /// Arabic with their marks; Hangul; the non-breaking spaces and hyphen.
    fn words() -> Vec<String> {
        let mut words: Vec<String> = (0..0x110000).filter_map(char::from_u32).filter(|letter| split(*letter).is_some() && !('가'..='힣').contains(letter)).flat_map(ways).collect();
        words.extend(['가', '각', '힣', '뷁', '쀍'].into_iter().flat_map(ways));
        let marks = ['\u{301}', '\u{323}', '\u{328}', '\u{31b}', '\u{345}', '\u{313}', '\u{342}', '\u{306}', '\u{302}', '\u{308}', '\u{327}', '\u{331}', '\u{34f}', '\u{303}'];
        for base in ['a', 'e', 'o', 'u', 'i', 'A', 'O', 'α', 'ω', 'η', 'у', 'ı', 'n'] {
            for (n, one) in marks.iter().enumerate() {
                for other in marks.iter().skip(n % 3) {
                    words.push(format!("{base}{one}{other}"));
                    words.push(format!("{base}{one}{other}{}", marks[(n * 5 + 3) % marks.len()]));
                }
            }
        }
        let greek = ["\u{3b1}\u{314}\u{301}\u{345}", "\u{3b1}\u{345}\u{301}\u{314}", "\u{3c9}\u{342}\u{313}\u{345}", "\u{1f00}\u{301}", "\u{1f71}", "\u{3ae}\u{345}", "Ἀθῆναι", "ᾅδης"];
        let vietnamese = ["Tiếng Việt", "Tie\u{302}\u{301}ng Vie\u{323}\u{302}t", "Tie\u{301}\u{302}ng", "ươ\u{301}c", "u\u{31b}o\u{31b}\u{301}c", "Đạo đức"];
        let hebrew = [
            "\u{5e9}\u{5c1}\u{5bc}",
            "\u{5e9}\u{5bc}\u{5c1}",
            "\u{5d0}\u{5b8}\u{5b0}\u{5bd}",
            "\u{5d0}\u{5b0}\u{5bd}\u{5b8}",
            "\u{5d1}\u{5bc}\u{5b7}",
            "\u{5d1}\u{5bf}",
            "\u{5d9}\u{5b4}",
            "\u{5d5}\u{5b9}",
            "\u{5e9}\u{5c2}\u{5b8}",
            "\u{5d1}\u{5b7}\u{5b0}\u{591}",
            "שָׁלוֹם",
            "בְּרֵאשִׁית",
        ];
        let arabic = [
            "\u{627}\u{653}",
            "\u{627}\u{654}",
            "\u{627}\u{655}",
            "\u{648}\u{654}",
            "\u{64a}\u{654}",
            "\u{6c1}\u{654}",
            "\u{628}\u{64e}\u{654}",
            "\u{628}\u{654}\u{64e}",
            "\u{628}\u{651}\u{64e}",
            "\u{628}\u{64e}\u{651}",
            "\u{628}\u{650}\u{651}\u{655}",
            "\u{627}\u{670}\u{653}",
            "\u{628}\u{652}\u{658}\u{64f}",
            "كِتَابٌ",
            "مُحَمَّدٌ",
        ];
        words.extend(greek.iter().chain(&vietnamese).chain(&hebrew).chain(&arabic).map(|word| word.to_string()));
        for space in [
            '\u{a0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{202f}', '\u{205f}', '\u{3000}',
            '\u{2011}',
        ] {
            words.push(format!("a{space}b"));
        }
        words
    }

    /// The fonts: those `shape::held` tests with, Hebrew and Arabic ones, and fonts built here
    /// that lack what others have (written to files for HarfBuzz to read).
    fn fonts() -> Vec<(std::path::PathBuf, u32)> {
        let mut fonts = crate::shape::held::fonts();
        let more = ["noto/NotoSansArabic-Regular.ttf", "noto/NotoNaskhArabic-Regular.ttf", "noto/NotoSansHebrew-Regular.ttf", "noto/NotoSerifHebrew-Regular.ttf", "noto/NotoSerif-Italic.ttf"];
        fonts.extend(more.iter().map(|path| (std::path::PathBuf::from(format!("/usr/share/fonts/{path}")), 0)).filter(|(path, _)| path.exists()));
        let folder = std::env::temp_dir().join(format!("taut-accents-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let made = [
            ("letters", "ACEOacenou\u{301}\u{302}\u{303}\u{308}\u{323}\u{327}\u{328}\u{345}\u{3b1}\u{3c9}"),
            ("some-whole", "Aa\u{c1}\u{c5}\u{e1}\u{e2}\u{e9}\u{301}\u{302}\u{30a}\u{323}\u{1ea1}\u{1ea5}"),
            ("hangul", "\u{1100}\u{1161}\u{11a8}\u{ac00}"),
            ("hebrew", "\u{5b0}\u{5b4}\u{5b7}\u{5b8}\u{5b9}\u{5bc}\u{5bd}\u{5bf}\u{5c1}\u{5c2}\u{5d0}\u{5d1}\u{5d5}\u{5d9}\u{5e9}\u{fb1d}\u{fb2a}\u{fb2b}\u{fb2c}\u{fb31}\u{fb4b}\u{fb4c}"),
            ("spaces", " ,.0ab\u{2010}"),
        ];
        for (name, letters) in made {
            let path = folder.join(format!("{name}.ttf"));
            std::fs::write(&path, font_of(letters).bytes().0).unwrap();
            fonts.push((path, 0));
        }
        fonts
    }

    #[test]
    fn letters_and_accents_come_to_the_glyphs_harfbuzz_makes_of_them() {
        let words = words();
        let (mut tried, mut differ) = (0, Vec::new());
        for (path, face) in fonts() {
            let Some(font) = FontFile::open(&path).and_then(|font| if face == 0 { Some(font) } else { font.nth(face as usize) }) else { continue };
            let Some(theirs) = crate::shape::harfbuzz::harfbuzz_in(&path, face, &words, "") else { return eprintln!("no HarfBuzz on this system: not tested") };
            // Where the font has no mark positioning, or lacks a letter of the word, HarfBuzz
            // positions marks by class, which isn't done here: only glyphs are compared then.
            // Likewise where a combining grapheme joiner is removed, as the test asks HarfBuzz
            // to: it leaves the joiner's advance in place of a following mark.
            let placed = !font.table(b"GPOS").0.is_empty();
            let mut plans = Vec::new();
            let mut here = 0;
            for (word, theirs) in words.iter().zip(theirs) {
                // Right-to-left text and syllable-based scripts shape as HarfBuzz does only
                // with their features.
                let leftward = word.chars().any(|letter| ('\u{590}'..'\u{900}').contains(&letter) || ('\u{fb1d}'..'\u{fe00}').contains(&letter));
                let syllabic = word.chars().any(|letter| (1..=crate::scripts::BRAHMIC).contains(&(crate::scripts::of(letter) as usize)));
                if (leftward && !cfg!(feature = "right-to-left")) || (syllabic && !cfg!(feature = "brahmic")) {
                    continue;
                }
                let mine = crate::shape::held::ours(&font, &mut plans, word, crate::scripts::Language::NONE);
                let whole = placed && theirs.0.iter().all(|(glyph, _)| *glyph != 0) && !word.contains('\u{34f}');
                let same = if whole { mine == theirs } else { mine.0.iter().map(|(glyph, _)| *glyph).eq(theirs.0.iter().map(|(glyph, _)| *glyph)) };
                if !same {
                    here += 1;
                    if here <= 4 {
                        println!("  {word} ({}): {mine:?}, not {theirs:?}", word.escape_unicode());
                    }
                    differ.push(format!("{}#{face}: {word} ({}): {mine:?}, not {theirs:?}", path.display(), word.escape_unicode()));
                }
                tried += 1;
            }
            println!("{}#{face}: {} words, {here} differ", path.display(), words.len());
        }
        println!("{tried} words and fonts held, {} differ", differ.len());
        assert!(differ.is_empty(), "{} words differ", differ.len());
    }
}
