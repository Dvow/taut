//! Shaping for the scripts of South and Southeast Asia: Devanagari, Bengali, Gurmukhi,
//! Gujarati, Oriya, Tamil, Telugu, Kannada and Malayalam; Sinhala; Thai and Lao; Khmer;
//! Myanmar; Tibetan, Balinese, Javanese and the rest of the Brahmic family (hence the feature
//! name `brahmic`).
//!
//! These scripts are written in syllables: a consonant, or a cluster joined by viramas (the
//! sign that removes a consonant's inherent vowel), with vowel signs and modifiers. Characters
//! are typed in spoken order; glyphs are drawn in another. A vowel sign spoken after a
//! consonant is written before it (ि in कि); a cluster-initial "r" becomes a hook over the last
//! letter (र्क is written क with the hook); and cluster consonants take half, below or
//! post-base forms. So, as HarfBuzz does: characters are normalized, each gets its category
//! (consonant, pre-base vowel sign, virama…) from HarfBuzz's tables, text is split into
//! syllables by the script's grammar, and between the stages of the font's features (run by
//! `shaping.rs`) each syllable is reordered into drawing order and its glyphs masked for their
//! features. Text forming no syllable (a lone vowel sign) is shown on a dotted circle, as a
//! typist expects.
//!
//! Each script uses HarfBuzz's shaper for it: the Indic shaper for the nine Indian scripts,
//! with both old (`deva`) and new (`dev2`) font tables; dedicated Khmer and Myanmar shapers;
//! Thai and Lao, whose only reordering is a two-part vowel; and the Universal Shaping Engine
//! for the rest, which HarfBuzz also uses for other families: Mongolian and Phags-pa, which
//! join like Arabic; N'Ko, Adlam, Hanifi Rohingya, Mandaic, Manichaean, Sogdian and similar,
//! right-to-left and joining (so shaped only with the feature `right-to-left` too); Kharoshthi,
//! Yezidi, Garay and other right-to-left ones; Egyptian hieroglyphs and the rest of HarfBuzz's
//! list. A font made for none of these (tables only for `DFLT` or `latn`) is shaped as
//! script-less text.
//!
//! Not done: Mongolian's vertical forms.
use crate::font_file::{Asked, Bytes, FontFile, lookups};
use crate::shaping::{ALL, DENOMINATOR, FRACTION, Laid, NUMERATOR, Plan, Step, Zeroing, chosen, merged, unicode};

// ---- the scripts ----

/// A script's shaping model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Way {
    /// Script-less: the font has no tables for it (only `DFLT` or `latn`).
    Plain,
    /// Indic: syllables, reph, pre-base vowel signs.
    Indic,
    /// Khmer.
    Khmer,
    /// Myanmar.
    Myanmar,
    /// Thai and Lao.
    Thai,
    /// The Universal Shaping Engine: the family's scripts without a dedicated shaper.
    Universal,
}

/// Whether script `n` of `WAYS` (the first scripts of `scripts.rs`'s list) is shaped here: with
/// the feature `brahmic`, and a right-to-left one only with `right-to-left` too (without it, no
/// text is reversed and no letter joins to its right).
pub(crate) fn shaped(n: u8) -> bool {
    cfg!(feature = "brahmic") && (WAYS[n as usize].1 & 1 == 0 || cfg!(feature = "right-to-left"))
}

/// The script tags a font may use, in lookup order: for Indic scripts the USE tag (`dev3`), the
/// new (`dev2`) and the old (`deva`); for Myanmar the new (`mym2`) and the old; then the
/// generic tags.
pub(crate) fn names(n: u8) -> Vec<&'static [u8; 4]> {
    let (tag, way) = (crate::scripts::tag(n + 1), &WAYS[n as usize].0);
    let mut names: Vec<&'static [u8; 4]> = match (way, n) {
        ('I', _) => vec![&NEWER[n as usize][1], &NEWER[n as usize][0], tag],
        ('M', _) => vec![b"mym2", tag],
        _ => vec![tag],
    };
    names.extend([b"DFLT", b"dflt", b"latn"]);
    names
}

/// The newer Indic script tags: `dev2` (the second-generation font spec) and `dev3` (for the
/// Universal Shaping Engine).
const NEWER: [[[u8; 4]; 2]; 9] = [
    [*b"dev2", *b"dev3"],
    [*b"bng2", *b"bng3"],
    [*b"gur2", *b"gur3"],
    [*b"gjr2", *b"gjr3"],
    [*b"ory2", *b"ory3"],
    [*b"tml2", *b"tml3"],
    [*b"tel2", *b"tel3"],
    [*b"knd2", *b"knd3"],
    [*b"mlm2", *b"mlm3"],
];

// ---- what each character is ----

/// The Indic, Khmer and Myanmar shaper categories the code refers to, by HarfBuzz's values (the
/// grammars name them all, below): consonant, independent vowel, nukta, virama (Khmer's coeng
/// counts as one), the joiners, matra, syllable modifier, Vedic sign, placeholder (no-break
/// space, dash), dotted circle, register shifter, post-base vowel sign, repha, ra, medial
/// consonant, consonant with stacker; Khmer and Myanmar below and pre-base vowels; and
/// Myanmar's asat, medial ra and variation selectors.
const CONSONANT: u8 = 1;
const VOWEL: u8 = 2;
const NUKTA: u8 = 3;
const VIRAMA: u8 = 4;
const NON_JOINER: u8 = 5;
const JOINER: u8 = 6;
const MATRA: u8 = 7;
const MODIFIER: u8 = 8;
const VEDIC: u8 = 9;
const PLACEHOLDER: u8 = 10;
const DOTTED: u8 = 11;
const SHIFTER: u8 = 12;
const MATRA_AFTER: u8 = 13;
const REPHA: u8 = 14;
const RA: u8 = 15;
const MEDIAL: u8 = 16;
const STACKER: u8 = 18;
const BELOW: u8 = 21;
const BEFORE: u8 = 22;
const ASAT: u8 = 32;
const MEDIAL_RA: u8 = 36;
const SELECTOR: u8 = 40;

/// The Universal Shaping Engine categories the code refers to (the grammar names them all,
/// below): other, base, number, other base, grapheme joiner, halant, ZWNJ, repha, pre-base
/// vowels and vowel modifiers, and a halant that may also be a vowel modifier.
const U_OTHER: u8 = 0;
const U_BASE: u8 = 1;
const U_CGJ: u8 = 6;
const U_HALANT: u8 = 12;
const U_NON_JOINER: u8 = 14;
const U_REPHA: u8 = 18;
const U_VOWEL_BEFORE: u8 = 22;
const U_MODIFIER_BEFORE: u8 = 23;
const U_HALANT_OR_MODIFIER: u8 = 44;
const U_HALANT_MODIFIER: u8 = 53;

/// Indic syllable positions, by HarfBuzz's values: start; ra becoming reph; pre-base matra;
/// pre-base consonant; base; after main; above-base; before, at and after sub-joined; before,
/// at and after post-base; syllable-final modifiers; end.
const P_START: u8 = 0;
const P_REPH: u8 = 1;
const P_MATRA_BEFORE: u8 = 2;
const P_BEFORE: u8 = 3;
const P_BASE: u8 = 4;
const P_AFTER_MAIN: u8 = 5;
const P_BEFORE_SUB: u8 = 7;
const P_BELOW: u8 = 8;
const P_AFTER_SUB: u8 = 9;
const P_POST: u8 = 11;
const P_AFTER_POST: u8 = 12;
const P_SIGNS: u8 = 13;
const P_END: u8 = 14;

/// A character's Indic, Khmer and Myanmar category and position.
fn indic(letter: char) -> (u8, u8) {
    let code = letter as u32;
    let n = INDIC.partition_point(|entry| entry >> 6 <= code);
    let value = VALUES[n.checked_sub(1).map_or(0, |n| INDIC[n] & 63) as usize];
    ((value >> 8) as u8, value as u8)
}

/// A character's Universal Shaping Engine category.
fn universal(letter: char) -> u8 {
    let code = letter as u32;
    let n = UNIVERSAL.partition_point(|entry| entry >> 6 <= code);
    n.checked_sub(1).map_or(U_OTHER, |n| (UNIVERSAL[n] & 63) as u8)
}

/// A mark's combining class (Unicode's, as HarfBuzz modifies a few): 0 for non-marks.
fn class_of(letter: char) -> u8 {
    let code = letter as u32;
    let n = CLASSES.partition_point(|entry| entry >> 8 <= code);
    n.checked_sub(1).map_or(0, |n| CLASSES[n] as u8)
}

/// Whether a character of these scripts is a nonspacing mark: what joining letters join across.
pub(crate) fn passed_over(letter: char) -> bool {
    cfg!(feature = "brahmic") && marked(letter) == 1
}

/// A character's mark kind: none (0), nonspacing (1), spacing (2).
fn marked(letter: char) -> u32 {
    let n = MARKS.partition_point(|entry| entry >> 2 <= letter as u32);
    n.checked_sub(1).map_or(0, |n| MARKS[n] & 3)
}

/// Whether a character is a mark: a vowel sign, virama, or sign above or below a letter.
fn mark(letter: char) -> bool {
    marked(letter) != 0
}

/// A character's canonical decomposition into two, and whether it recomposes when the font has
/// it (composition exclusions don't).
fn made_of(letter: char) -> Option<(char, char, bool)> {
    let at = MADE_OF.binary_search_by_key(&(letter as u32), |(whole, ..)| *whole).ok()?;
    let (_, first, second, again) = MADE_OF[at];
    Some((char::from_u32(first)?, char::from_u32(second)?, again))
}

// ---- syllables ----

/// Syllable grammars, as HarfBuzz writes them: names for categories and for composite parts,
/// then the rules, each with its syllable type. Where several rules match, the longest match
/// wins; among equals, the first. `|` separates alternatives, `?` marks optional, `*` any
/// number, `+` at least one.
type Grammar = (&'static [(&'static str, u8)], &'static [(&'static str, &'static str)], &'static [(&'static str, u8)]);

/// The Indic syllable grammar.
const INDIC_GRAMMAR: Grammar = (
    &[
        ("X", 0),
        ("C", 1),
        ("V", 2),
        ("N", 3),
        ("H", 4),
        ("ZWNJ", 5),
        ("ZWJ", 6),
        ("M", 7),
        ("SM", 8),
        ("A", 9),
        ("PLACEHOLDER", 10),
        ("DOTTEDCIRCLE", 11),
        ("RS", 12),
        ("MPst", 13),
        ("Repha", 14),
        ("Ra", 15),
        ("CM", 16),
        ("Symbol", 17),
        ("CS", 18),
        ("SMPst", 57),
    ],
    &[
        ("c", "C | Ra"),
        ("n", "(ZWNJ? RS)? (N N?)?"),
        ("z", "ZWJ | ZWNJ"),
        ("reph", "Ra H | Repha"),
        ("sm", "SM | SMPst"),
        ("cn", "c ZWJ? n?"),
        ("symbol", "Symbol N?"),
        ("matra_group", "z* (M | sm? MPst) N? H?"),
        ("syllable_tail", "(z? sm sm? ZWNJ?)? A*"),
        ("halant_group", "z? H (ZWJ N?)?"),
        ("final_halant_group", "halant_group | H ZWNJ"),
        ("halant_or_matra_group", "final_halant_group | matra_group*"),
        ("complex_syllable_tail", "(halant_group cn)* CM? halant_or_matra_group syllable_tail"),
    ],
    &[
        ("(Repha | CS)? cn complex_syllable_tail", 0),
        ("reph? V n? (ZWJ | complex_syllable_tail)", 1),
        ("((Repha | CS)? PLACEHOLDER | reph? DOTTEDCIRCLE) n? complex_syllable_tail", 2),
        ("symbol syllable_tail", 3),
        ("SMPst", 5),
        ("reph? n? complex_syllable_tail", 4),
    ],
);

/// The Khmer syllable grammar.
const KHMER_GRAMMAR: Grammar = (
    &[
        ("C", 1),
        ("V", 2),
        ("H", 4),
        ("ZWNJ", 5),
        ("ZWJ", 6),
        ("PLACEHOLDER", 10),
        ("DOTTEDCIRCLE", 11),
        ("Ra", 15),
        ("VAbv", 20),
        ("VBlw", 21),
        ("VPre", 22),
        ("VPst", 23),
        ("Robatic", 25),
        ("Xgroup", 26),
        ("Ygroup", 27),
    ],
    &[
        ("c", "C | Ra | V"),
        ("cn", "c ((ZWJ | ZWNJ)? Robatic)?"),
        ("joiner", "ZWJ | ZWNJ"),
        ("xgroup", "(joiner* Xgroup)*"),
        ("ygroup", "Ygroup*"),
        ("matra_group", "VPre? xgroup VBlw? xgroup (joiner? VAbv)? xgroup VPst?"),
        ("syllable_tail", "xgroup matra_group xgroup (H c)? ygroup"),
        ("broken_cluster", "Robatic? (H cn)* (H | syllable_tail)"),
    ],
    &[("(cn | PLACEHOLDER | DOTTEDCIRCLE) broken_cluster", 0), ("broken_cluster", 1)],
);

/// The Myanmar syllable grammar.
const MYANMAR_GRAMMAR: Grammar = (
    &[
        ("C", 1),
        ("IV", 2),
        ("DB", 3),
        ("H", 4),
        ("ZWNJ", 5),
        ("ZWJ", 6),
        ("SM", 8),
        ("GB", 10),
        ("DOTTEDCIRCLE", 11),
        ("A", 9),
        ("Ra", 15),
        ("CS", 18),
        ("SMPst", 57),
        ("VAbv", 20),
        ("VBlw", 21),
        ("VPre", 22),
        ("VPst", 23),
        ("As", 32),
        ("MH", 35),
        ("MR", 36),
        ("MW", 37),
        ("MY", 38),
        ("PT", 39),
        ("VS", 40),
        ("ML", 41),
    ],
    &[
        ("j", "ZWJ | ZWNJ"),
        ("k", "Ra As H"),
        ("sm", "SM | SMPst"),
        ("c", "C | Ra"),
        ("medial_group", "MY? As? MR? ((MW MH? ML? | MH ML? | ML) As?)?"),
        ("main_vowel_group", "(VPre VS?)* VAbv* VBlw* A* (DB As?)?"),
        ("post_vowel_group", "VPst MH? ML? As* VAbv* A* (DB As?)?"),
        ("tone_group", "sm | PT A* DB? As?"),
        ("complex_syllable_tail", "As* medial_group main_vowel_group post_vowel_group* tone_group* j?"),
        ("syllable_tail", "(H (c | IV) VS?)* (H | complex_syllable_tail)"),
    ],
    &[("(k | CS)? (c | IV | GB | DOTTEDCIRCLE) VS? syllable_tail", 0), ("j | SMPst", 3), ("k? VS? syllable_tail", 2)],
);

/// The Universal Shaping Engine cluster grammar.
const UNIVERSAL_GRAMMAR: Grammar = (
    &[
        ("O", 0),
        ("B", 1),
        ("N", 4),
        ("GB", 5),
        ("CGJ", 6),
        ("SUB", 11),
        ("H", 12),
        ("HN", 13),
        ("ZWNJ", 14),
        ("WJ", 16),
        ("R", 18),
        ("CS", 43),
        ("IS", 44),
        ("Sk", 48),
        ("G", 49),
        ("J", 50),
        ("SB", 51),
        ("SE", 52),
        ("HVM", 53),
        ("HM", 54),
        ("HR", 55),
        ("RK", 56),
        ("FAbv", 24),
        ("FBlw", 25),
        ("FPst", 26),
        ("MAbv", 27),
        ("MBlw", 28),
        ("MPst", 29),
        ("MPre", 30),
        ("CMAbv", 31),
        ("CMBlw", 32),
        ("VAbv", 33),
        ("VBlw", 34),
        ("VPst", 35),
        ("VPre", 22),
        ("VMAbv", 37),
        ("VMBlw", 38),
        ("VMPst", 39),
        ("VMPre", 23),
        ("SMAbv", 41),
        ("SMBlw", 42),
        ("FMAbv", 45),
        ("FMBlw", 46),
        ("FMPst", 47),
    ],
    &[
        ("h", "H | HVM | IS | Sk"),
        ("consonant_modifiers", "CMAbv* CMBlw* ((h B | SUB) CMAbv* CMBlw*)*"),
        ("medial_consonants", "MPre? MAbv? MBlw? MPst?"),
        ("dependent_vowels", "VPre* VAbv* VBlw* VPst* | H"),
        ("vowel_modifiers", "HVM? VMPre* VMAbv* VMBlw* VMPst*"),
        ("final_consonants", "FAbv* FBlw* FPst*"),
        ("final_modifiers", "FMAbv* FMBlw* | FMPst?"),
        ("complex_syllable_start", "(R | CS)? (B | GB)"),
        ("complex_syllable_middle", "consonant_modifiers medial_consonants dependent_vowels vowel_modifiers (Sk B)*"),
        ("complex_syllable_tail", "complex_syllable_middle final_consonants final_modifiers"),
        ("number_joiner_terminated_cluster_tail", "(HN N)* HN"),
        ("numeral_cluster_tail", "(HN N)+"),
        ("symbol_cluster_tail", "SMAbv+ SMBlw* | SMBlw+"),
        ("virama_terminated_cluster_tail", "consonant_modifiers (IS | RK)"),
        ("sakot_terminated_cluster_tail", "complex_syllable_middle Sk"),
        ("tail", "complex_syllable_tail | sakot_terminated_cluster_tail | symbol_cluster_tail | virama_terminated_cluster_tail"),
    ],
    &[
        ("complex_syllable_start virama_terminated_cluster_tail ZWNJ?", 1),
        ("complex_syllable_start sakot_terminated_cluster_tail ZWNJ?", 2),
        ("complex_syllable_start complex_syllable_tail ZWNJ?", 3),
        ("N number_joiner_terminated_cluster_tail ZWNJ?", 4),
        ("N numeral_cluster_tail? ZWNJ?", 5),
        ("(O | GB | SB) tail? ZWNJ?", 6),
        ("SB* G HR? HM? SE* (J SB* (G HR? HM? SE*)?)* ZWNJ?", 7),
        ("FMPst", 9),
        ("R? (tail | number_joiner_terminated_cluster_tail | numeral_cluster_tail) ZWNJ?", 8),
    ],
);

/// The broken syllable type per shaper: one lacking what its signs attach to, shown on a dotted
/// circle; and the non-syllable type.
const BROKEN: [u8; 4] = [4, 1, 2, 8];
const NONE: [u8; 4] = [5, 2, 3, 9];

/// One state of a syllable-matching automaton: consume a character of one of the `takes`
/// categories (a bit each) and go to another; or go to two others without consuming; or to one;
/// or accept rule `rule`.
#[derive(Clone, Copy, Debug)]
enum State {
    Takes(u64, usize),
    Either(usize, usize),
    Then(usize),
    Ends(u8),
}

/// A script's syllable automaton: its states, with the start; and each rule's syllable type.
#[derive(Debug, Default)]
pub(crate) struct Machine {
    states: Vec<State>,
    start: usize,
    kinds: Vec<u8>,
    other: u8,
}

/// An unpatched transition.
const OPEN: usize = usize::MAX;

impl Machine {
    /// The automaton for a grammar. Each part name is expanded in place, then the rules are
    /// compiled as patterns (to an NFA, by Thompson's construction).
    fn of((categories, parts, rules): Grammar, other: u8) -> Machine {
        let mut written: Vec<(&str, String)> = Vec::new();
        for (name, part) in parts {
            let text = spelled(part, &written);
            written.push((name, text));
        }
        let mut machine = Machine { other, ..Machine::default() };
        let mut starts = Vec::new();
        for (rule, kind) in rules {
            let tokens = spelled(rule, &written);
            let (start, end) = machine.pattern(&tokens, categories);
            machine.states[end] = State::Then(machine.states.len());
            machine.states.push(State::Ends(machine.kinds.len() as u8));
            machine.kinds.push(*kind);
            starts.push(start);
        }
        // The rules start in parallel: each in turn, via split states.
        machine.start = starts.pop().unwrap_or(0);
        while let Some(start) = starts.pop() {
            machine.states.push(State::Either(start, machine.start));
            machine.start = machine.states.len() - 1;
        }
        machine
    }

    /// Adds a state; returns its index.
    fn add(&mut self, state: State) -> usize {
        self.states.push(state);
        self.states.len() - 1
    }

    /// A pattern's states, from space-separated words: its start and its end (dangling to
    /// `OPEN`). The words are converted to postfix order first (each operator after its
    /// operand, a sequence's pieces before the `|` joining them), then built with a fragment
    /// stack.
    fn pattern(&mut self, text: &str, categories: &[(&str, u8)]) -> (usize, usize) {
        let mut pieces: Vec<(usize, usize)> = Vec::new();
        for word in postfix(text) {
            let piece = self.piece(word, &mut pieces, categories);
            pieces.push(piece);
        }
        let mut whole = pieces.pop().unwrap_or_else(|| self.empty());
        while let Some(before) = pieces.pop() {
            whole = self.join(before, whole, false);
        }
        whole
    }

    /// The fragment a pattern word makes from the fragments before it: a repetition operator
    /// wraps the last; `.` or `|` joins the last two; a name consumes a character of its
    /// category.
    fn piece(&mut self, word: &str, pieces: &mut Vec<(usize, usize)>, categories: &[(&str, u8)]) -> (usize, usize) {
        let mut last = || pieces.pop();
        match word {
            "?" | "*" | "+" => {
                let inner = last().unwrap_or_else(|| self.empty());
                self.repeat(inner, word)
            }
            "." | "|" => {
                let (second, first) = (last(), last());
                let (second, first) = (second.unwrap_or_else(|| self.empty()), first.unwrap_or_else(|| self.empty()));
                self.join(first, second, word == "|")
            }
            name => {
                let bit = categories.iter().find(|(known, _)| *known == name).map_or(0, |(_, n)| 1u64 << n);
                let end = self.add(State::Then(OPEN));
                (self.add(State::Takes(bit, end)), end)
            }
        }
    }

    /// A fragment consuming nothing.
    fn empty(&mut self) -> (usize, usize) {
        let end = self.add(State::Then(OPEN));
        (end, end)
    }

    /// A fragment made optional (`?`), repeated any number of times (`*`), or at least once
    /// (`+`).
    fn repeat(&mut self, (start, end): (usize, usize), how: &str) -> (usize, usize) {
        let after = self.add(State::Then(OPEN));
        let choice = self.add(State::Either(start, after));
        self.states[end] = State::Then(if how == "?" { after } else { choice });
        (if how == "+" { start } else { choice }, after)
    }

    /// Two fragments in sequence, or (`either`) as alternatives.
    fn join(&mut self, (first, first_end): (usize, usize), (second, second_end): (usize, usize), either: bool) -> (usize, usize) {
        if !either {
            self.states[first_end] = State::Then(second);
            return (first, second_end);
        }
        let after = self.add(State::Then(OPEN));
        self.states[first_end] = State::Then(after);
        self.states[second_end] = State::Then(after);
        (self.add(State::Either(first, second)), after)
    }

    /// How many `categories` from `from` the longest rule matches (at least one: an unmatched
    /// character is a syllable of its own), and which rule.
    fn longest(&self, categories: &[u8], from: usize, scratch: &mut Scratch) -> (usize, u8) {
        let mut best = (1, u8::MAX);
        scratch.now.clear();
        scratch.now.push(self.start);
        self.close(scratch);
        let mut at = from;
        loop {
            let ended = scratch.now.iter().filter_map(|state| match self.states[*state] {
                State::Ends(rule) => Some(rule),
                _ => None,
            });
            if let (true, Some(rule)) = (at > from, ended.min()) {
                best = (at - from, rule);
            }
            let Some(category) = categories.get(at) else { break };
            let next: Vec<usize> = scratch.now.iter().filter_map(|state| self.taken(*state, *category)).collect();
            if next.is_empty() {
                break;
            }
            scratch.now = next;
            self.close(scratch);
            at += 1;
        }
        best
    }

    /// Where a state goes on consuming a category, if it consumes one.
    fn taken(&self, state: usize, category: u8) -> Option<usize> {
        match self.states[state] {
            State::Takes(bits, next) if category < 64 && bits & (1 << category) != 0 => Some(next),
            _ => None,
        }
    }

    /// Adds to the current states all those reachable without consuming, each once.
    fn close(&self, scratch: &mut Scratch) {
        scratch.seen.clear();
        scratch.seen.resize(self.states.len(), false);
        let mut waiting = std::mem::take(&mut scratch.now);
        while let Some(state) = waiting.pop() {
            if state == OPEN || std::mem::replace(&mut scratch.seen[state], true) {
                continue;
            }
            match self.states[state] {
                State::Either(one, other) => waiting.extend([one, other]),
                State::Then(next) => waiting.push(next),
                _ => scratch.now.push(state),
            }
        }
    }

    /// Splits characters of these categories into syllables: each gets its syllable's number (1
    /// to 15, cycling) times 16, plus its type. `kept` marks the characters the grammar sees:
    /// the others belong to the syllable around them. Returns whether any syllable was broken.
    fn cut(&self, categories: &[u8], kept: &[bool], syllables: &mut [u8], broken: u8) -> bool {
        let seen: Vec<usize> = (0..categories.len()).filter(|n| kept[*n]).collect();
        let shown: Vec<u8> = seen.iter().map(|n| categories[*n]).collect();
        let (mut scratch, mut serial, mut any) = (Scratch::default(), 1u8, false);
        let mut at = 0;
        while at < shown.len() {
            let (long, rule) = self.longest(&shown, at, &mut scratch);
            let kind = self.kinds.get(rule as usize).copied().unwrap_or(self.other);
            let first = seen[at];
            let end = seen.get(at + long).copied().unwrap_or(categories.len());
            syllables[first..end].fill(serial << 4 | (kind & 15));
            any |= kind == broken;
            serial = if serial == 15 { 1 } else { serial + 1 };
            at += long;
        }
        any
    }
}

/// The automaton's current and visited states, kept to reuse their memory.
#[derive(Default)]
struct Scratch {
    now: Vec<usize>,
    seen: Vec<bool>,
}

/// A pattern with part names expanded, as space-separated words.
fn spelled(text: &str, parts: &[(&str, String)]) -> String {
    let spaced = text.replace('(', " ( ").replace(')', " ) ").replace('|', " | ").replace('?', " ? ").replace('*', " * ").replace('+', " + ");
    let words = spaced.split_whitespace().map(|word| match parts.iter().rev().find(|(name, _)| *name == word) {
        Some((_, part)) => format!("( {part} )"),
        None => word.to_string(),
    });
    words.collect::<Vec<_>>().join(" ")
}

/// A pattern's words in postfix order: each piece after its operands, `.` for sequence and `|`
/// for alternation (`.` binding tighter). Parentheses are dropped.
fn postfix(text: &str) -> Vec<&str> {
    let (mut out, mut held): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let mut piece_before = false;
    for word in text.split_whitespace() {
        let starts_piece = word != ")" && word != "|" && !matches!(word, "?" | "*" | "+");
        if starts_piece && piece_before {
            pop_while(&mut out, &mut held, |top| top == ".");
            held.push(".");
        }
        match word {
            "(" => held.push("("),
            ")" => {
                pop_while(&mut out, &mut held, |top| top != "(");
                held.pop();
            }
            "|" => {
                pop_while(&mut out, &mut held, |top| top == "." || top == "|");
                held.push("|");
            }
            _ => out.push(word),
        }
        piece_before = !matches!(word, "(" | "|");
    }
    pop_while(&mut out, &mut held, |_| true);
    out
}

/// Pops operators onto `out` from the stack's top while `moves` holds for them.
fn pop_while<'a>(out: &mut Vec<&'a str>, held: &mut Vec<&'a str>, moves: impl Fn(&str) -> bool) {
    while let Some(top) = held.last().filter(|top| moves(top)) {
        out.push(top);
        held.pop();
    }
}

// ---- the plan ----

/// Masks of the features applied to some glyphs only (the rest apply to all): consonant forms
/// (reph, pre-base, below-base, above-base, half, post-base), the word-initial matra, and the
/// consonants after a pre-base ra.
const RPHF: u32 = 64;
const PREF: u32 = 128;
const BLWF: u32 = 256;
const ABVF: u32 = 512;
const HALF: u32 = 1024;
const PSTF: u32 = 2048;
const INIT: u32 = 4096;
const CFAR: u32 = 8192;
/// Universal Shaping Engine joining forms: isolated, initial, medial, final (counted as
/// Arabic's are in `shaping.rs`).
const ISOL: u32 = 2;
const FINA: u32 = 4;
const MEDI: u32 = 8;
const INIT_FORM: u32 = 16;

/// How a feature applies: whether a ZWJ or ZWNJ between glyphs blocks its rules, and whether
/// its rules stay within one syllable.
const BY_JOINER: u8 = 1;
const BY_NON_JOINER: u8 = 2;
const BY_JOINERS: u8 = 3;
const IN_SYLLABLE: u8 = 4;

/// What happens between two stages of a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pause {
    /// Nothing.
    None,
    /// Syllable segmentation (and for the Universal Shaping Engine, joining forms).
    Syllables,
    /// The initial Indic reordering: each syllable's base found, its glyphs reordered and
    /// masked for their features.
    Initial,
    /// The final Indic reordering: pre-base matras, reph and pre-base ra moved to where the
    /// font's forms want them.
    Final,
    /// Khmer's reordering.
    Khmer,
    /// Myanmar's reordering.
    Myanmar,
    /// Restores what replaced default-ignorables (Universal Shaping Engine).
    Forget,
    /// Glyphs that became a reph, or a pre-base ra, are recorded as such.
    Reph,
    Prebase,
    /// The Universal Shaping Engine's reordering.
    Universal,
    /// Syllables are no longer needed.
    Done,
}

/// The Indic shaper's settings for one of the nine scripts: its virama, reph position, reph
/// encoding (ra plus virama, ra plus virama plus ZWJ, or its own character), and whether
/// below-base forms apply to pre-base consonants too.
#[derive(Clone, Copy, Debug, Default)]
struct Config {
    virama: char,
    reph: u8,
    written: u8,
    below_before: bool,
}

/// Reph positions: after main, before sub-joined, after sub-joined, before post-base, after
/// post-base.
const AFTER_MAIN: u8 = P_AFTER_MAIN;
const BEFORE_SUB: u8 = P_BEFORE_SUB;
const AFTER_SUB: u8 = P_AFTER_SUB;
const BEFORE_POST: u8 = 10;
const AFTER_POST: u8 = P_AFTER_POST;
/// Reph encodings.
const IMPLICIT: u8 = 0;
const EXPLICIT: u8 = 1;
const LETTER: u8 = 2;

/// The settings of the nine Indic scripts, in `TAGS` order.
const CONFIGS: [Config; 9] = [
    Config { virama: '\u{94d}', reph: BEFORE_POST, written: IMPLICIT, below_before: true },
    Config { virama: '\u{9cd}', reph: AFTER_SUB, written: IMPLICIT, below_before: true },
    Config { virama: '\u{a4d}', reph: BEFORE_SUB, written: IMPLICIT, below_before: true },
    Config { virama: '\u{acd}', reph: BEFORE_POST, written: IMPLICIT, below_before: true },
    Config { virama: '\u{b4d}', reph: AFTER_MAIN, written: IMPLICIT, below_before: true },
    Config { virama: '\u{bcd}', reph: AFTER_POST, written: IMPLICIT, below_before: true },
    Config { virama: '\u{c4d}', reph: AFTER_POST, written: EXPLICIT, below_before: false },
    Config { virama: '\u{ccd}', reph: AFTER_POST, written: IMPLICIT, below_before: false },
    Config { virama: '\u{d4d}', reph: AFTER_MAIN, written: LETTER, below_before: true },
];

/// A plan's extras for these scripts besides its lookups: shaping model and script,
/// right-to-left and Arabic-like joining; what happens between stages; masks of the font's
/// glyph-specific features; for the Indic shaper, its settings, whether the font is old-style,
/// and the consonant form features' lookups (reph, pre-base, below, post-base, below with ra),
/// checked for whether they would apply; whether the font has no substitutions for the script;
/// and the syllable automaton.
#[derive(Debug, Default)]
pub(crate) struct Syllabic {
    pub way: Option<Way>,
    script: u8,
    leftward: bool,
    joins: bool,
    pub pauses: Vec<Pause>,
    masks: u32,
    config: Config,
    old: bool,
    would: [Vec<u16>; 5],
    zero_context: bool,
    unlisted: bool,
    machine: Machine,
}

/// A plan feature: its tag, the glyphs it applies to, and how it applies.
type Feature = (&'static [u8; 4], u32, u8);

/// Each shaper's stages: their features, and what happens after each.
const INDIC_STAGES: &[(&[Feature], Pause)] = &[
    (&[(b"ltra", ALL, 0), (b"ltrm", ALL, 0)], Pause::Syllables),
    (&[(b"locl", ALL, IN_SYLLABLE), (b"ccmp", ALL, IN_SYLLABLE)], Pause::Initial),
    (&[(b"nukt", ALL, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"akhn", ALL, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"rphf", RPHF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"rkrf", ALL, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"pref", PREF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"blwf", BLWF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"abvf", ABVF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"half", HALF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"pstf", PSTF, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"vatu", ALL, BY_JOINERS | IN_SYLLABLE)], Pause::None),
    (&[(b"cjct", ALL, BY_JOINERS | IN_SYLLABLE)], Pause::Final),
    (
        &[
            (b"init", INIT, BY_JOINERS | IN_SYLLABLE),
            (b"pres", ALL, BY_JOINERS | IN_SYLLABLE),
            (b"abvs", ALL, BY_JOINERS | IN_SYLLABLE),
            (b"blws", ALL, BY_JOINERS | IN_SYLLABLE),
            (b"psts", ALL, BY_JOINERS | IN_SYLLABLE),
            (b"haln", ALL, BY_JOINERS | IN_SYLLABLE),
            (b"rlig", ALL, 0),
            (b"calt", ALL, 0),
            (b"clig", ALL, 0),
            (b"rclt", ALL, 0),
        ],
        Pause::Done,
    ),
];
/// Khmer's stages.
const KHMER_STAGES: &[(&[Feature], Pause)] = &[
    (&[(b"ltra", ALL, 0), (b"ltrm", ALL, 0)], Pause::Syllables),
    (&[], Pause::Khmer),
    (
        &[
            (b"locl", ALL, IN_SYLLABLE),
            (b"ccmp", ALL, IN_SYLLABLE),
            (b"pref", PREF, BY_JOINERS | IN_SYLLABLE),
            (b"blwf", BLWF, BY_JOINERS | IN_SYLLABLE),
            (b"abvf", ABVF, BY_JOINERS | IN_SYLLABLE),
            (b"pstf", PSTF, BY_JOINERS | IN_SYLLABLE),
            (b"cfar", CFAR, BY_JOINERS | IN_SYLLABLE),
        ],
        Pause::Done,
    ),
    (
        &[(b"pres", ALL, BY_JOINERS), (b"abvs", ALL, BY_JOINERS), (b"blws", ALL, BY_JOINERS), (b"psts", ALL, BY_JOINERS), (b"rlig", ALL, 0), (b"calt", ALL, 0), (b"clig", ALL, 0), (b"rclt", ALL, 0)],
        Pause::None,
    ),
];
/// Myanmar's stages.
const MYANMAR_STAGES: &[(&[Feature], Pause)] = &[
    (&[(b"ltra", ALL, 0), (b"ltrm", ALL, 0)], Pause::Syllables),
    (&[(b"locl", ALL, IN_SYLLABLE), (b"ccmp", ALL, IN_SYLLABLE)], Pause::Myanmar),
    (&[(b"rphf", ALL, BY_JOINER | IN_SYLLABLE)], Pause::None),
    (&[(b"pref", ALL, BY_JOINER | IN_SYLLABLE)], Pause::None),
    (&[(b"blwf", ALL, BY_JOINER | IN_SYLLABLE)], Pause::None),
    (&[(b"pstf", ALL, BY_JOINER | IN_SYLLABLE)], Pause::Done),
    (
        &[
            (b"pres", ALL, BY_JOINER),
            (b"abvs", ALL, BY_JOINER),
            (b"blws", ALL, BY_JOINER),
            (b"psts", ALL, BY_JOINER),
            (b"rlig", ALL, 0),
            (b"calt", ALL, 0),
            (b"clig", ALL, 0),
            (b"liga", ALL, 0),
            (b"rclt", ALL, 0),
        ],
        Pause::None,
    ),
];
/// The Universal Shaping Engine's stages.
const UNIVERSAL_STAGES: &[(&[Feature], Pause)] = &[
    (&[(b"ltra", ALL, 0), (b"ltrm", ALL, 0)], Pause::Syllables),
    (&[(b"locl", ALL, IN_SYLLABLE), (b"ccmp", ALL, IN_SYLLABLE), (b"nukt", ALL, IN_SYLLABLE), (b"akhn", ALL, BY_JOINER | IN_SYLLABLE)], Pause::Forget),
    (&[(b"rphf", RPHF, BY_JOINER | IN_SYLLABLE)], Pause::Reph),
    (&[(b"pref", ALL, BY_JOINER | IN_SYLLABLE)], Pause::Prebase),
    (
        &[
            (b"rkrf", ALL, BY_JOINER | IN_SYLLABLE),
            (b"abvf", ALL, BY_JOINER | IN_SYLLABLE),
            (b"blwf", ALL, BY_JOINER | IN_SYLLABLE),
            (b"half", ALL, BY_JOINER | IN_SYLLABLE),
            (b"pstf", ALL, BY_JOINER | IN_SYLLABLE),
            (b"vatu", ALL, BY_JOINER | IN_SYLLABLE),
            (b"cjct", ALL, BY_JOINER | IN_SYLLABLE),
        ],
        Pause::Universal,
    ),
    (&[(b"isol", ISOL, 0), (b"init", INIT_FORM, 0), (b"medi", MEDI, 0), (b"fina", FINA, 0)], Pause::None),
    (
        &[
            (b"abvs", ALL, BY_JOINER),
            (b"blws", ALL, BY_JOINER),
            (b"haln", ALL, BY_JOINER),
            (b"pres", ALL, BY_JOINER),
            (b"psts", ALL, BY_JOINER),
            (b"rlig", ALL, 0),
            (b"calt", ALL, 0),
            (b"clig", ALL, 0),
            (b"liga", ALL, 0),
            (b"rclt", ALL, 0),
        ],
        Pause::None,
    ),
];
/// The stage of Thai, Lao, and script-less text: the common features.
const PLAIN_STAGES: &[(&[Feature], Pause)] =
    &[(&[(b"ltra", ALL, 0), (b"ltrm", ALL, 0), (b"ccmp", ALL, 0), (b"locl", ALL, 0), (b"rlig", ALL, 0), (b"calt", ALL, 0), (b"clig", ALL, 0), (b"liga", ALL, 0), (b"rclt", ALL, 0)], Pause::None)];
/// The `GPOS` features, in one stage.
const MOVES: &[Feature] =
    &[(b"ltra", ALL, 0), (b"ltrm", ALL, 0), (b"abvm", ALL, 0), (b"blwm", ALL, 0), (b"mark", ALL, BY_JOINERS), (b"mkmk", ALL, BY_JOINERS), (b"curs", ALL, 0), (b"dist", ALL, 0), (b"kern", ALL, 0)];

/// The first stage's features besides the direction ones: variable font substitutions, and
/// fractions.
const FIRST: [Feature; 4] = [(b"rvrn", ALL, 0), (b"frac", FRACTION, 0), (b"numr", NUMERATOR, 0), (b"dnom", DENOMINATOR, 0)];
/// The right-to-left direction features (instead of `ltra` and `ltrm`): the second only on a
/// bracket the font has no mirrored glyph for.
const LEFTWARD: [Feature; 2] = [(b"rtla", ALL, 0), (b"rtlm", crate::shaping::MIRROR, 0)];

/// The lookups some features list for a script, language and table, as steps.
fn steps(table: Bytes, asked: Asked, features: &[Feature]) -> Vec<Step> {
    let mut steps = Vec::new();
    for (name, mask, how) in features {
        let found = lookups(table, asked, &[*name]).unwrap_or_default();
        let step = |lookup| Step { lookup, mask: *mask, joiners: how & BY_JOINER == 0, non_joiners: how & BY_NON_JOINER == 0, syllable: how & IN_SYLLABLE != 0 };
        steps.extend(found.into_iter().map(step));
    }
    merged(steps)
}

/// Whether a font's chosen script tag is script-less: `DFLT` or `latn`.
fn plain_tag(tag: Option<[u8; 4]>) -> bool {
    matches!(&tag, Some(name) if name == b"DFLT" || name == b"latn")
}

impl Way {
    /// The model for script `n` in a font that chose script tag `tag`.
    fn of(n: u8, tag: Option<[u8; 4]>) -> Way {
        match WAYS[n as usize].0 {
            _ if plain_tag(tag) => Way::Plain,
            'I' if tag.is_some_and(|name| name[3] == b'3') => Way::Universal,
            'I' => Way::Indic,
            'K' => Way::Khmer,
            'M' if tag.is_some_and(|name| &name == b"mymr") => Way::Plain,
            'M' => Way::Myanmar,
            'T' => Way::Thai,
            _ => Way::Universal,
        }
    }

    /// Its shaper's stages.
    fn stages(self) -> &'static [(&'static [Feature], Pause)] {
        match self {
            Way::Indic => INDIC_STAGES,
            Way::Khmer => KHMER_STAGES,
            Way::Myanmar => MYANMAR_STAGES,
            Way::Universal => UNIVERSAL_STAGES,
            Way::Plain | Way::Thai => PLAIN_STAGES,
        }
    }

    /// Its syllable automaton.
    fn machine(self) -> Machine {
        match self {
            Way::Indic => Machine::of(INDIC_GRAMMAR, NONE[0]),
            Way::Khmer => Machine::of(KHMER_GRAMMAR, NONE[1]),
            Way::Myanmar => Machine::of(MYANMAR_GRAMMAR, NONE[2]),
            Way::Universal => Machine::of(UNIVERSAL_GRAMMAR, NONE[3]),
            Way::Plain | Way::Thai => Machine::default(),
        }
    }

    /// When marks' advances are zeroed.
    fn zeroing(self) -> Zeroing {
        match self {
            Way::Indic | Way::Khmer => Zeroing::Never,
            Way::Myanmar | Way::Universal => Zeroing::Early,
            Way::Plain | Way::Thai => Zeroing::Late,
        }
    }

    /// The masks of the glyph-specific features the font has (`has`).
    fn feature_masks(self, has: impl Fn(&[u8; 4]) -> bool) -> u32 {
        let init = if self == Way::Universal { INIT_FORM } else { INIT };
        let masks = [(b"rphf", RPHF), (b"pref", PREF), (b"blwf", BLWF), (b"abvf", ABVF), (b"half", HALF), (b"pstf", PSTF), (b"init", init), (b"cfar", CFAR)];
        let forms = [(b"isol", ISOL), (b"medi", MEDI), (b"fina", FINA)].into_iter().filter(|_| self == Way::Universal);
        masks.into_iter().chain(forms).filter(|(name, _)| has(name)).fold(0, |all, (_, mask)| all | mask)
    }
}

/// The plan for script `n` of these, for a font and language. (A right-to-left one in a
/// script-less font is shaped as `shaping.rs` shapes such text.)
pub(crate) fn plan(font: &FontFile, n: u8, language: crate::scripts::Language) -> Plan {
    let (gsub, gpos) = (font.table(b"GSUB"), font.table(b"GPOS"));
    let languages: &[[u8; 4]] = &language.systems().copied().collect::<Vec<_>>();
    let names = names(n);
    let tag = chosen(gsub, &names);
    let (leftward, joins) = (WAYS[n as usize].1 & 1 != 0, WAYS[n as usize].1 & 2 != 0);
    if plain_tag(tag) && leftward {
        return Plan::of(font, crate::shaping::Script::Plain(n + 1), true, language);
    }
    let way = Way::of(n, tag);
    let asked: Asked = (&names, languages, font.place());
    let has = |name: &[u8; 4]| lookups(gsub, asked, &[name]).is_some() || lookups(gpos, asked, &[name]).is_some();
    let old = way == Way::Indic && tag.is_none_or(|name| name[3] != b'2');
    let lookups_of = |name: &[u8; 4]| lookups(gsub, asked, &[name]).unwrap_or_default();
    let syllabic = Syllabic {
        way: Some(way),
        script: n,
        leftward,
        joins: joins && way == Way::Universal,
        pauses: way.stages().iter().map(|(_, pause)| *pause).collect(),
        masks: way.feature_masks(has),
        config: CONFIGS.get(n as usize).copied().filter(|_| way == Way::Indic).unwrap_or_default(),
        old,
        would: [b"rphf", b"pref", b"blwf", b"pstf", b"vatu"].map(lookups_of),
        // HarfBuzz's rule, after Windows: a new-style font's lookups get zero context, but an
        // old-style one, and Malayalam either way, may look around.
        zero_context: !old && n != 8,
        unlisted: way == Way::Thai && chosen(gsub, &names[..1]).is_none(),
        machine: way.machine(),
    };
    // Each shaper's first stage has the direction features and those every script gets.
    let first = |features: &[Feature]| [if leftward { &LEFTWARD[..] } else { features }, &FIRST].concat();
    let stage = |(at, (features, _)): (usize, &(&[Feature], Pause))| steps(gsub, asked, &if at == 0 { first(features) } else { features.to_vec() });
    let moves = [MOVES, &FIRST[1..]].concat();
    Plan {
        swaps: way.stages().iter().enumerate().map(stage).collect(),
        moves: steps(gpos, asked, &moves),
        kerned: lookups(gpos, asked, &[b"kern"]).is_some(),
        leftward,
        joins: false,
        zeroing: way.zeroing(),
        syllabic: Some(Box::new(syllabic)),
        ..Plan::default()
    }
}

// ---- the characters ----

/// A run's glyphs in the order HarfBuzz passes them to the font's lookups: dotted circles
/// inserted where a sequence would read as another letter, Thai sara am decomposed, characters
/// normalized as the font has them; each with its category and syllable position.
pub(crate) fn characters(font: &FontFile, plan: &Syllabic, text: &str, glyphs: &mut Vec<Laid>) {
    let mut letters: Vec<(char, u32)> = text.chars().zip(0..).take(crate::shaping::LONGEST).collect();
    let variants = crate::shaping::variants(font, &mut letters);
    let way = plan.way.unwrap_or(Way::Plain);
    if matches!(way, Way::Indic | Way::Universal) && font.glyph('\u{25cc}') != 0 {
        letters = circled(letters);
    }
    if way == Way::Thai {
        sara_am(&mut letters);
    }
    if way == Way::Thai && plan.unlisted {
        older_thai(font, &mut letters);
    }
    let letters = normalized(font, way, letters);
    // Joining scripts take their forms here, per letter as HarfBuzz does, not per syllable; and
    // fraction digits are marked.
    let mut forms = vec![ALL; letters.len()];
    if plan.joins {
        crate::shaping::joined(letters.iter().map(|(letter, _)| *letter), &mut forms);
    }
    crate::shaping::fractions(letters.iter().map(|(letter, _)| *letter), plan.leftward, &mut forms);
    for (n, (letter, from)) in letters.into_iter().enumerate() {
        let mirror = if plan.leftward { crate::bidi::mirrored(letter) } else { None };
        let shown = mirror.filter(|mirror| font.glyph(*mirror) != 0).unwrap_or(letter);
        let mirrors = if mirror.is_some() && shown == letter { crate::shaping::MIRROR } else { 0 };
        let glyph = variants.iter().find(|(at, _)| *at == from).map_or_else(|| font.glyph(shown), |(_, glyph)| *glyph);
        let (category, position) = match way {
            Way::Indic | Way::Khmer | Way::Myanmar => indic(letter),
            Way::Universal => (universal(letter), 0),
            Way::Plain | Way::Thai => (0, 0),
        };
        let unicode = unicode(letter);
        // What a font without a `GDEF` class takes a glyph for, as in HarfBuzz: a mark if its
        // character is nonspacing, and not default-ignorable.
        let props = if marked(letter) == 1 && unicode & 1 == 0 { crate::shaping::MARK } else { crate::shaping::BASE };
        let mut laid = Laid::default();
        (laid.glyph, laid.from, laid.mask, laid.unicode, laid.props) = (glyph, from, forms[n] | mirrors | ALL, unicode, props);
        (laid.letter, laid.category, laid.position) = (letter, category, position);
        glyphs.push(laid);
    }
    let letters: Vec<char> = glyphs.iter().map(|laid| laid.letter).collect();
    crate::shaping::unhidden(&letters, glyphs, class_of);
}

/// Character pairs that would read as another letter, separated by a dotted circle (HarfBuzz's
/// list, from the Universal Shaping Engine's): an independent vowel and a vowel sign that would
/// make it look like another vowel.
#[rustfmt::skip]
const LOOKALIKES: [(u32, u32); 99] = [
    (0x905, 0x93a), (0x905, 0x93b), (0x905, 0x93e), (0x905, 0x945), (0x905, 0x946), (0x905, 0x949), (0x905, 0x94a), (0x905, 0x94b),
    (0x905, 0x94c), (0x905, 0x94f), (0x905, 0x956), (0x905, 0x957), (0x906, 0x93a), (0x906, 0x945), (0x906, 0x946), (0x906, 0x947),
    (0x906, 0x948), (0x909, 0x941), (0x90f, 0x945), (0x90f, 0x946), (0x90f, 0x947), (0x985, 0x9be), (0x98b, 0x9c3), (0x98c, 0x9e2),
    (0xa05, 0xa3e), (0xa05, 0xa48), (0xa05, 0xa4c), (0xa72, 0xa3f), (0xa72, 0xa40), (0xa72, 0xa47), (0xa73, 0xa41), (0xa73, 0xa42),
    (0xa73, 0xa4b), (0xa85, 0xabe), (0xa85, 0xac5), (0xa85, 0xac7), (0xa85, 0xac8), (0xa85, 0xac9), (0xa85, 0xacb), (0xa85, 0xacc),
    (0xac5, 0xabe), (0xb05, 0xb3e), (0xb0f, 0xb57), (0xb13, 0xb57), (0xb85, 0xbc2), (0xc12, 0xc4c), (0xc12, 0xc55), (0xc3f, 0xc55),
    (0xc46, 0xc55), (0xc4a, 0xc55), (0xc89, 0xcbe), (0xc8b, 0xcbe), (0xc92, 0xccc), (0xd07, 0xd57), (0xd09, 0xd57), (0xd0e, 0xd46),
    (0xd12, 0xd3e), (0xd12, 0xd57), (0xd85, 0xdcf), (0xd85, 0xdd0), (0xd85, 0xdd1), (0xd8b, 0xddf), (0xd8d, 0xdd8), (0xd8f, 0xddf),
    (0xd91, 0xdca), (0xd91, 0xdd9), (0xd91, 0xdda), (0xd91, 0xddc), (0xd91, 0xddd), (0xd91, 0xdde), (0xd94, 0xddf), (0x11005, 0x11038),
    (0x1100b, 0x1103e), (0x1100f, 0x11042), (0x11200, 0x1122c), (0x11200, 0x11231), (0x11200, 0x11233), (0x11206, 0x1122c), (0x1122c, 0x11230), (0x1122c, 0x11231),
    (0x11240, 0x1122e), (0x112b0, 0x112e0), (0x112b0, 0x112e5), (0x112b0, 0x112e6), (0x112b0, 0x112e7), (0x112b0, 0x112e8), (0x11481, 0x114b0), (0x1148b, 0x114ba),
    (0x1148d, 0x114ba), (0x114aa, 0x114b5), (0x114aa, 0x114b6), (0x11600, 0x11639), (0x11600, 0x1163a), (0x11601, 0x11639), (0x11601, 0x1163a), (0x11680, 0x116ad),
    (0x11680, 0x116b4), (0x11680, 0x116b5), (0x11686, 0x116b2),
];

/// Characters with a dotted circle inserted between each lookalike pair (and in Devanagari,
/// between ra plus virama and vowel i, which would read as a reph on it).
fn circled(letters: Vec<(char, u32)>) -> Vec<(char, u32)> {
    let mut out = Vec::with_capacity(letters.len() + 1);
    let mut n = 0;
    while n < letters.len() {
        out.push(letters[n]);
        let next = |k: usize| letters.get(n + k).map_or(0, |(letter, _)| *letter as u32);
        if letters[n].0 == '\u{930}' && next(1) == 0x94d && next(2) == 0x907 {
            out.push(letters[n + 1]);
            out.push(('\u{25cc}', letters[n + 2].1));
            n += 2;
            continue;
        }
        let pair = (letters[n].0 as u32, next(1));
        if n + 1 < letters.len() && LOOKALIKES.binary_search(&pair).is_ok() {
            out.push(('\u{25cc}', letters[n + 1].1));
        }
        n += 1;
    }
    out
}

/// Thai and Lao sara am, which is a nikhahit over the previous letter plus sara aa, written as
/// those two: the nikhahit before other marks on that letter.
fn sara_am(letters: &mut Vec<(char, u32)>) {
    let above = |letter: char| matches!(letter as u32 & !0x80, 0xe31 | 0xe34..=0xe37 | 0xe3b | 0xe47..=0xe4e);
    let mut n = 0;
    while n < letters.len() {
        let (letter, from) = letters[n];
        if letter as u32 & !0x80 != 0xe33 {
            n += 1;
            continue;
        }
        let code = letter as u32;
        let (nikhahit, aa) = (char::from_u32(code - 0xe33 + 0xe4d).unwrap_or(letter), char::from_u32(code - 1).unwrap_or(letter));
        letters[n] = (aa, from);
        let start = n - letters[..n].iter().rev().take_while(|(letter, _)| above(*letter)).count();
        letters.insert(start, (nikhahit, from));
        n += 2;
    }
}

/// Thai marks shifted where a font without Thai tables has glyphs for them (in its private-use
/// area, at Windows' or the Mac's code points): a tone mark lower where no vowel sign is above
/// the letter, further left over a letter with a tall right stroke; a vowel sign below a letter
/// with a descender, which then loses it.
fn older_thai(font: &FontFile, letters: &mut [(char, u32)]) {
    let (mut above, mut below, mut base) = (3usize, 2usize, 0);
    for n in 0..letters.len() {
        let code = letters[n].0 as u32;
        let mark = match code {
            0xe31 | 0xe34..=0xe37 | 0xe47 | 0xe4d..=0xe4e => 0,
            0xe38..=0xe3a => 1,
            0xe48..=0xe4c => 2,
            _ => 3,
        };
        if mark == 3 {
            let consonant = match code {
                0xe1b | 0xe1d | 0xe1f => 1,
                0xe0d | 0xe10 => 2,
                0xe0e | 0xe0f => 3,
                0xe01..=0xe2e => 0,
                _ => 4,
            };
            (above, below, base) = ([0, 1, 0, 0, 3][consonant], [0, 0, 1, 2, 2][consonant], n);
            continue;
        }
        let (above_action, above_next) = THAI_ABOVE[above][mark];
        let (below_action, below_next) = THAI_BELOW[below][mark];
        (above, below) = (above_next, below_next);
        let action = if above_action != 0 { above_action } else { below_action };
        let at = if action == RD { base } else { n };
        letters[at].0 = moved(font, letters[at].0, action);
    }
}

/// What is done to a Thai mark or letter, per a pre-OpenType Thai font: nothing, shift down,
/// left, down and left, or remove a descender.
const SD: u8 = 1;
const SL: u8 = 2;
const SDL: u8 = 3;
const RD: u8 = 4;
/// What happens above a letter, by what is there (nothing, a tall stroke, a left-shifted mark,
/// a full slot) and what comes (vowel above, vowel below, tone): the action, and the new state.
const THAI_ABOVE: [[(u8, usize); 3]; 4] = [[(0, 3), (0, 0), (SD, 3)], [(SL, 2), (0, 1), (SDL, 2)], [(0, 3), (0, 2), (SL, 3)], [(0, 3), (0, 3), (0, 3)]];
/// What happens below a letter (no descender, a removable one, a fixed one).
const THAI_BELOW: [[(u8, usize); 3]; 3] = [[(0, 0), (0, 2), (0, 0)], [(0, 1), (RD, 2), (0, 1)], [(0, 2), (SD, 2), (0, 2)]];
/// The private-use code points a font keeps for shifted forms: each with Windows' and the
/// Mac's.
#[rustfmt::skip]
const THAI_MOVED: [(u8, u16, [u16; 2]); 27] = [
    (SD, 0xe48, [0xf70a, 0xf88b]), (SD, 0xe49, [0xf70b, 0xf88e]), (SD, 0xe4a, [0xf70c, 0xf891]), (SD, 0xe4b, [0xf70d, 0xf894]),
    (SD, 0xe4c, [0xf70e, 0xf897]), (SD, 0xe38, [0xf718, 0xf89b]), (SD, 0xe39, [0xf719, 0xf89c]), (SD, 0xe3a, [0xf71a, 0xf89d]),
    (SDL, 0xe48, [0xf705, 0xf88c]), (SDL, 0xe49, [0xf706, 0xf88f]), (SDL, 0xe4a, [0xf707, 0xf892]), (SDL, 0xe4b, [0xf708, 0xf895]),
    (SDL, 0xe4c, [0xf709, 0xf898]), (SL, 0xe48, [0xf713, 0xf88a]), (SL, 0xe49, [0xf714, 0xf88d]), (SL, 0xe4a, [0xf715, 0xf890]),
    (SL, 0xe4b, [0xf716, 0xf893]), (SL, 0xe4c, [0xf717, 0xf896]), (SL, 0xe31, [0xf710, 0xf884]), (SL, 0xe34, [0xf701, 0xf885]),
    (SL, 0xe35, [0xf702, 0xf886]), (SL, 0xe36, [0xf703, 0xf887]), (SL, 0xe37, [0xf704, 0xf888]), (SL, 0xe47, [0xf712, 0xf889]),
    (SL, 0xe4d, [0xf711, 0xf899]), (RD, 0xe0d, [0xf70f, 0xf89a]), (RD, 0xe10, [0xf700, 0xf89e]),
];

/// The private-use character a font has for a Thai character after `action`, if any.
fn moved(font: &FontFile, letter: char, action: u8) -> char {
    let found = THAI_MOVED.iter().find(|(done, of, _)| *done == action && *of as u32 == letter as u32);
    let places = found.map_or([].as_slice(), |(.., places)| places.as_slice());
    let kept = places.iter().filter_map(|place| char::from_u32(*place as u32)).find(|place| font.glyph(*place) != 0);
    kept.unwrap_or(letter)
}

/// Characters as HarfBuzz passes them to a font: each decomposed where the font has all the
/// parts (for Thai, Lao and script-less text, only one the font lacks, or one among marks),
/// marks after each letter sorted by class, then recomposed where the font has the whole.
fn normalized(font: &FontFile, way: Way, letters: Vec<(char, u32)>) -> Vec<(char, u32)> {
    let short = matches!(way, Way::Thai | Way::Plain);
    let mut out: Vec<(char, u32)> = Vec::with_capacity(letters.len() + 4);
    for (n, (letter, from)) in letters.iter().copied().enumerate() {
        // A letter followed by a mark is fully decomposed, as the marks are.
        let marked = (n > 0 && mark(letter)) || letters.get(n + 1).is_some_and(|(next, _)| mark(*next));
        let shortest = short && !marked && font.glyph(letter) != 0;
        if shortest {
            out.push((letter, from));
            continue;
        }
        let parts = taken_apart(font, way, letter, short && !marked);
        out.extend(parts.into_iter().map(|part| (part, from)));
    }
    if letters.iter().skip(1).all(|(letter, _)| !mark(*letter)) {
        return out;
    }
    in_class_order(&mut out);
    composed(font, way, out)
}

/// A character's decomposition as far as the font has the parts: the deepest whose second parts
/// the font all has, and whose first it has (or, `shortest`, the first such). Itself if none.
fn taken_apart(font: &FontFile, way: Way, letter: char, shortest: bool) -> Vec<char> {
    let mut chain = Vec::new();
    let mut whole = letter;
    while let Some((first, second)) = split(way, whole).filter(|_| chain.len() < 4) {
        chain.push((first, second));
        whole = first;
    }
    // Whether decomposing from each level down works: its second part is in the font, and so is
    // its first, or that decomposes in turn.
    let has = |letter: char| font.glyph(letter) != 0;
    let mut works = vec![false; chain.len() + 1];
    for level in (0..chain.len()).rev() {
        let (first, second) = chain[level];
        works[level] = has(second) && (works[level + 1] && !(shortest && has(first)) || has(first));
    }
    if !works.first().copied().unwrap_or(false) {
        return vec![letter];
    }
    let mut depth = 0;
    while depth + 1 < chain.len() && works[depth + 1] && !(shortest && has(chain[depth].0)) {
        depth += 1;
    }
    let mut parts = vec![chain[depth].0];
    parts.extend(chain[..=depth].iter().rev().map(|(_, second)| *second));
    parts
}

/// A character's decomposition under a shaper.
fn split(way: Way, letter: char) -> Option<(char, char)> {
    match (way, letter as u32) {
        (Way::Indic, 0x931 | 0x9dc | 0x9dd | 0xb94) => None,
        (Way::Khmer, 0x17be | 0x17bf | 0x17c0 | 0x17c4 | 0x17c5) => Some(('\u{17c1}', letter)),
        _ => made_of(letter).map(|(first, second, _)| (first, second)),
    }
}

/// The composition of two characters under a shaper, if any.
fn joined(way: Way, first: char, second: char) -> Option<char> {
    if matches!(way, Way::Indic | Way::Khmer | Way::Universal) && mark(first) {
        return None;
    }
    if way == Way::Indic && first == '\u{9af}' && second == '\u{9bc}' {
        return Some('\u{9df}');
    }
    let found = MADE_OF.iter().find(|(_, one, other, again)| *again && *one == first as u32 && *other == second as u32);
    found.and_then(|(whole, ..)| char::from_u32(*whole))
}

/// Sorts each mark run after a letter by combining class (stable), as Unicode does, except runs
/// longer than 32.
fn in_class_order(letters: &mut [(char, u32)]) {
    let mut at = 0;
    while at < letters.len() {
        if class_of(letters[at].0) == 0 {
            at += 1;
            continue;
        }
        let end = at + letters[at..].iter().take_while(|(letter, _)| class_of(*letter) != 0).count();
        if end - at <= 32 {
            settle(&mut letters[at..end], |(letter, _)| class_of(*letter));
        }
        at = end + 1;
    }
}

/// Characters with each mark composed with its preceding letter where the font has the pair as
/// one character and nothing between blocks it in class order.
fn composed(font: &FontFile, way: Way, letters: Vec<(char, u32)>) -> Vec<(char, u32)> {
    let mut out: Vec<(char, u32)> = Vec::with_capacity(letters.len());
    let mut starter = 0;
    for (n, (letter, from)) in letters.into_iter().enumerate() {
        let open = n > 0 && mark(letter) && (starter + 1 == out.len() || out.last().is_some_and(|(last, _)| class_of(*last) < class_of(letter)));
        let whole = if open { joined(way, out[starter].0, letter).filter(|whole| font.glyph(*whole) != 0) } else { None };
        if let Some(whole) = whole {
            out[starter].0 = whole;
            continue;
        }
        out.push((letter, from));
        if class_of(letter) == 0 {
            starter = out.len() - 1;
        }
    }
    out
}

/// A stable insertion sort by key, as HarfBuzz does: for a syllable's few items, all it takes.
fn settle<T: Copy>(things: &mut [T], key: impl Fn(&T) -> u8) {
    for n in 1..things.len() {
        let thing = things[n];
        let mut at = n;
        while at > 0 && key(&things[at - 1]) > key(&thing) {
            things[at] = things[at - 1];
            at -= 1;
        }
        things[at] = thing;
    }
}

// ---- between the stages ----

/// Does what a plan says happens after stage `stage`.
pub(crate) fn pause(font: &FontFile, plan: &Syllabic, stage: usize, glyphs: &mut Vec<Laid>) {
    match plan.pauses.get(stage).copied().unwrap_or(Pause::None) {
        Pause::None => {}
        Pause::Syllables => syllables(plan, glyphs),
        Pause::Initial => initial(font, plan, glyphs),
        Pause::Final => each_syllable(glyphs, |glyphs, start, end| finally(font, plan, glyphs, start, end)),
        Pause::Khmer => khmer(font, plan, glyphs),
        Pause::Myanmar => myanmar(font, glyphs),
        Pause::Forget => glyphs.iter_mut().for_each(|laid| laid.props &= !crate::shaping::SUBSTITUTED),
        Pause::Reph => {
            each_syllable(glyphs, |glyphs, start, end| reph_recorded(plan, &mut glyphs[start..end]));
            glyphs.iter_mut().for_each(|laid| laid.props &= !crate::shaping::SUBSTITUTED);
        }
        Pause::Prebase => each_syllable(glyphs, |glyphs, start, end| prebase_recorded(&mut glyphs[start..end])),
        Pause::Universal => {
            universal_reordered(font, glyphs);
            glyphs.iter_mut().for_each(|laid| laid.syllable = 0);
        }
        Pause::Done => glyphs.iter_mut().for_each(|laid| laid.syllable = 0),
    }
}

/// Calls `each` with every syllable's start and end.
fn each_syllable(glyphs: &mut Vec<Laid>, mut each: impl FnMut(&mut Vec<Laid>, usize, usize)) {
    let mut start = 0;
    while start < glyphs.len() {
        let syllable = glyphs[start].syllable;
        let end = start + glyphs[start..].iter().take_while(|laid| laid.syllable == syllable).count();
        each(glyphs, start, end);
        start = end.max(start + 1);
    }
}

/// Splits glyphs into syllables by their script's grammar. (The Universal Shaping Engine's
/// grammar ignores grapheme joiners, and ZWNJ before a mark; and its glyphs get joining forms
/// where the font has them.)
fn syllables(plan: &Syllabic, glyphs: &mut [Laid]) {
    let categories: Vec<u8> = glyphs.iter().map(|laid| laid.category).collect();
    let universal = plan.way == Some(Way::Universal);
    let seen = |n: usize| glyphs[n].category != U_CGJ;
    let kept: Vec<bool> = (0..glyphs.len())
        .map(|n| match (universal, glyphs[n].category) {
            (false, _) => true,
            (true, U_CGJ) => false,
            (true, U_NON_JOINER) => (n + 1..glyphs.len()).find(|m| seen(*m)).is_none_or(|m| !mark(glyphs[m].letter)),
            _ => true,
        })
        .collect();
    let way = match plan.way {
        Some(Way::Khmer) => 1,
        Some(Way::Myanmar) => 2,
        Some(Way::Universal) => 3,
        _ => 0,
    };
    let mut syllables = vec![0u8; glyphs.len()];
    let broken = plan.machine.cut(&categories, &kept, &mut syllables, BROKEN[way]);
    glyphs.iter_mut().zip(&syllables).for_each(|(laid, syllable)| laid.syllable = *syllable);
    // Broken syllables are found again where dotted circles are inserted.
    let _ = broken;
    let mut serial = 0u16;
    let mut last = None;
    for laid in glyphs.iter_mut() {
        if last != Some(laid.syllable) {
            (serial, last) = (serial + 1, Some(laid.syllable));
        }
        laid.whole = serial;
    }
    if universal {
        universal_masks(plan, glyphs);
    }
}

/// A glyph's syllable type.
fn kind(laid: &Laid) -> u8 {
    laid.syllable & 15
}

/// Inserts a dotted circle into each broken syllable (after a leading repha), if the font has
/// one: as category `category`, at position `position`.
fn dotted_circles(font: &FontFile, glyphs: &mut Vec<Laid>, broken: u8, category: u8, repha: Option<u8>, position: u8) {
    let circle = font.glyph('\u{25cc}');
    if circle == 0 || !glyphs.iter().any(|laid| kind(laid) == broken) {
        return;
    }
    let mut out = Vec::with_capacity(glyphs.len() + 4);
    let mut last = 0;
    let mut n = 0;
    while n < glyphs.len() {
        let laid = glyphs[n];
        if last == laid.syllable || kind(&laid) != broken {
            out.push(laid);
            n += 1;
            continue;
        }
        last = laid.syllable;
        while n < glyphs.len() && glyphs[n].syllable == last && Some(glyphs[n].category) == repha {
            out.push(glyphs[n]);
            n += 1;
        }
        let mut dotted = Laid::default();
        (dotted.glyph, dotted.from, dotted.mask, dotted.syllable, dotted.whole) = (circle, laid.from, laid.mask, laid.syllable, laid.whole);
        (dotted.letter, dotted.category, dotted.position) = ('\u{25cc}', category, position);
        out.push(dotted);
    }
    *glyphs = out;
}

// ---- the scripts of India ----

/// Whether a glyph is of one of some categories (a bit each); never one that ligated.
fn one_of(laid: &Laid, categories: u64) -> bool {
    laid.props & crate::shaping::LIGATED == 0 && laid.category < 64 && categories & (1 << laid.category) != 0
}

/// A bitmask of categories.
const fn bits(categories: &[u8]) -> u64 {
    let mut all = 0;
    let mut n = 0;
    while n < categories.len() {
        all |= 1 << categories[n];
        n += 1;
    }
    all
}

/// What counts as a consonant in a syllable (vowels and placeholders stand in for one).
const CONSONANTS: u64 = bits(&[CONSONANT, STACKER, RA, MEDIAL, VOWEL, PLACEHOLDER, DOTTED]);
const JOINERS: u64 = bits(&[JOINER, NON_JOINER]);
const MATRAS: u64 = bits(&[MATRA, MATRA_AFTER]);

/// Whether a glyph counts as a consonant (and didn't ligate).
fn consonant(laid: &Laid) -> bool {
    one_of(laid, CONSONANTS)
}
/// Whether a glyph is ZWJ or ZWNJ (and didn't ligate).
fn joiner(laid: &Laid) -> bool {
    one_of(laid, JOINERS)
}
/// Whether a glyph is a virama (and didn't ligate).
fn virama(laid: &Laid) -> bool {
    one_of(laid, bits(&[VIRAMA]))
}

impl Syllabic {
    /// Whether one of the consonant form features' lookups (reph, pre-base, below, post-base,
    /// below with ra) would substitute these glyphs.
    fn would(&self, font: &FontFile, feature: usize, glyphs: &[u16]) -> bool {
        let table = font.table(b"GSUB");
        self.would[feature].iter().any(|lookup| crate::shaping::would_apply(table, *lookup, glyphs, self.zero_context))
    }
    /// Whether the font has a glyph-specific feature.
    fn has(&self, mask: u32) -> bool {
        self.masks & mask != 0
    }
    /// A glyph-specific feature's mask, if the font has it.
    fn mask(&self, mask: u32) -> u32 {
        self.masks & mask
    }
    /// The script's first tag.
    fn tag(&self) -> &'static [u8; 4] {
        crate::scripts::tag(self.script + 1)
    }
}

/// The initial Indic reordering, before the consonant form features.
fn initial(font: &FontFile, plan: &Syllabic, glyphs: &mut Vec<Laid>) {
    let virama = font.glyph(plan.config.virama);
    if virama != 0 {
        for laid in glyphs.iter_mut().filter(|laid| laid.position == P_BASE) {
            laid.position = placed(font, plan, laid.glyph, virama);
        }
    }
    dotted_circles(font, glyphs, BROKEN[0], DOTTED, Some(REPHA), P_END);
    each_syllable(glyphs, |glyphs, start, end| {
        if matches!(kind(&glyphs[start]), 0 | 1 | 2 | 4) {
            reordered(font, plan, &mut glyphs[start..end]);
        }
    });
}

/// A consonant's position from what the font does with it after a virama: below-base,
/// post-base, or (if neither) a possible base. (Both orders are tried, for fonts that copied
/// the old lookup order into the new.)
fn placed(font: &FontFile, plan: &Syllabic, consonant: u16, virama: u16) -> u8 {
    let either = |feature: usize| plan.would(font, feature, &[virama, consonant]) || plan.would(font, feature, &[consonant, virama]);
    match () {
        _ if either(2) || either(4) => P_BELOW,
        _ if either(3) || either(1) => P_POST,
        _ => P_BASE,
    }
}

/// Reorders one Indic syllable into formation order and masks its glyphs for their features
/// (Microsoft's rules, as HarfBuzz implements them).
fn reordered(font: &FontFile, plan: &Syllabic, syllable: &mut [Laid]) {
    let end = syllable.len();
    // Kannada ra, virama, ZWJ act as ra, ZWJ, virama, as older text has it.
    if plan.tag() == b"knda" && end >= 3 && one_of(&syllable[0], bits(&[RA])) && one_of(&syllable[1], bits(&[VIRAMA])) && one_of(&syllable[2], bits(&[JOINER])) {
        syllable.swap(1, 2);
    }
    let (mut base, has_reph) = base_of(font, plan, syllable);
    for laid in &mut syllable[..base] {
        laid.position = laid.position.min(P_BEFORE);
    }
    if base < end {
        syllable[base].position = P_BASE;
    }
    if has_reph {
        syllable[0].position = P_REPH;
    }
    if plan.old {
        halant_moved(plan, syllable, base);
    }
    marks_follow(syllable);
    owned_by_post_base(syllable, base);
    settle(syllable, |laid| laid.position);
    base = syllable.iter().position(|laid| laid.position == P_BASE).unwrap_or(end);
    flip_left_matras(syllable, base);
    masked(font, plan, syllable, base);
}

/// A syllable's base consonant, and whether it starts with a reph. The base is found from the
/// end: the last consonant without a below-base or post-base form (post-base forms may only
/// follow below-base ones), or the first consonant. A leading ra plus virama (or a reph
/// character) becomes a reph where the font forms one and another consonant follows.
fn base_of(font: &FontFile, plan: &Syllabic, syllable: &[Laid]) -> (usize, bool) {
    let end = syllable.len();
    let (mut base, mut has_reph, mut limit) = (end, false, 0);
    let config = plan.config;
    let by_ra = end >= 3 && ((config.written == IMPLICIT && !joiner(&syllable[2])) || (config.written == EXPLICIT && syllable[2].category == JOINER));
    if plan.has(RPHF) && by_ra {
        let two = [syllable[0].glyph, syllable[1].glyph];
        let three = [syllable[0].glyph, syllable[1].glyph, syllable[2].glyph];
        if plan.would(font, 0, &two) || (config.written == EXPLICIT && plan.would(font, 0, &three)) {
            limit = 2 + syllable[2..].iter().take_while(|laid| joiner(laid)).count();
            (base, has_reph) = (0, true);
        }
    } else if config.written == LETTER && syllable[0].category == REPHA {
        limit = 1 + syllable[1..].iter().take_while(|laid| joiner(laid)).count();
        (base, has_reph) = (0, true);
    }
    let mut seen_below = false;
    let mut n = end;
    while n > 0 {
        n -= 1;
        let laid = &syllable[n];
        if consonant(laid) {
            if laid.position != P_BELOW && (laid.position != P_POST || seen_below) {
                base = n;
                break;
            }
            seen_below |= laid.position == P_BELOW;
            base = n;
        } else if n > 0 && laid.category == JOINER && syllable[n - 1].category == VIRAMA {
            // ZWJ after a virama requests a half form, and the base search stops.
            break;
        }
        if n <= limit {
            break;
        }
    }
    // Where the reph's ra is the only consonant, it is the base and forms no reph.
    if has_reph && base == 0 && limit <= 2 {
        has_reph = false;
    }
    (base, has_reph)
}

/// In old-style fonts, the first virama after the base moves after the last consonant (in
/// Kannada, not past another virama).
fn halant_moved(plan: &Syllabic, syllable: &mut [Laid], base: usize) {
    let end = syllable.len();
    let Some(n) = (base + 1..end).find(|n| syllable[*n].category == VIRAMA) else { return };
    let kannada = plan.tag() == b"knda";
    let mut last = end - 1;
    while last > n && !(consonant(&syllable[last]) || (kannada && syllable[last].category == VIRAMA)) {
        last -= 1;
    }
    if syllable[last].category != VIRAMA && last > n {
        syllable[n..=last].rotate_left(1);
    }
}

/// Signs belonging to the previous glyph (joiners, nuktas, a virama, a medial) take its
/// position, to move with it. (A virama after a pre-base matra keeps the position before that,
/// as Windows does.)
fn marks_follow(syllable: &mut [Laid]) {
    let mut last = P_START;
    let following = bits(&[JOINER, NON_JOINER, NUKTA, SHIFTER, MEDIAL, VIRAMA]);
    for n in 0..syllable.len() {
        let laid = syllable[n];
        if laid.category < 64 && following & (1 << laid.category) != 0 {
            syllable[n].position = last;
            if laid.category == VIRAMA && last == P_MATRA_BEFORE {
                let before = (1..=n).rev().find(|m| syllable[m - 1].position != P_MATRA_BEFORE);
                syllable[n].position = before.map_or(last, |m| syllable[m - 1].position);
            }
        } else if laid.position != P_SIGNS {
            if laid.category == MATRA_AFTER && n > 0 && syllable[n - 1].category == MODIFIER {
                syllable[n - 1].position = laid.position;
            }
            last = laid.position;
        }
    }
}

/// A post-base consonant owns what lies before it since the last consonant or matra.
fn owned_by_post_base(syllable: &mut [Laid], base: usize) {
    let mut last = base;
    for n in base + 1..syllable.len() {
        if consonant(&syllable[n]) {
            let position = syllable[n].position;
            syllable[last + 1..n].iter_mut().filter(|laid| laid.position < P_SIGNS).for_each(|laid| laid.position = position);
            last = n;
        } else if MATRAS & (1u64 << syllable[n].category.min(63)) != 0 {
            last = n;
        }
    }
}

/// Several pre-base matras are drawn in reverse order (each with its signs).
fn flip_left_matras(syllable: &mut [Laid], base: usize) {
    let lefts: Vec<usize> = (0..base.min(syllable.len())).filter(|n| syllable[*n].position == P_MATRA_BEFORE).collect();
    let (Some(first), Some(last)) = (lefts.first().copied(), lefts.last().copied()) else { return };
    if first >= last {
        return;
    }
    syllable[first..=last].reverse();
    let mut from = first;
    for n in first..=last {
        if MATRAS & (1u64 << syllable[n].category.min(63)) != 0 {
            syllable[from..=n].reverse();
            from = n + 1;
        }
    }
}

/// Masks a syllable's glyphs for the consonant form features: reph; half and (new-style)
/// below-base before the base; below, above and post-base after it; in old Devanagari the
/// eyelash ra below a half form; a virama plus ra after the base that the font forms pre-base;
/// and no half form before ZWNJ.
fn masked(font: &FontFile, plan: &Syllabic, syllable: &mut [Laid], base: usize) {
    let end = syllable.len();
    for laid in syllable.iter_mut().take_while(|laid| laid.position == P_REPH) {
        laid.mask |= plan.mask(RPHF);
    }
    let before = plan.mask(HALF) | if !plan.old && plan.config.below_before { plan.mask(BLWF) } else { 0 };
    syllable[..base.min(end)].iter_mut().for_each(|laid| laid.mask |= before);
    let after = plan.mask(BLWF) | plan.mask(ABVF) | plan.mask(PSTF);
    syllable.iter_mut().skip(base + 1).for_each(|laid| laid.mask |= after);
    if plan.old && plan.tag() == b"deva" {
        for n in 0..base.saturating_sub(1) {
            let eyelash = syllable[n].category == RA && syllable[n + 1].category == VIRAMA && (n + 2 == base || syllable[n + 2].category != JOINER);
            if eyelash {
                syllable[n].mask |= plan.mask(BLWF);
                syllable[n + 1].mask |= plan.mask(BLWF);
            }
        }
    }
    if plan.has(PREF) && base + 2 < end {
        let found = (base + 1..end - 1).find(|n| plan.would(font, 1, &[syllable[*n].glyph, syllable[n + 1].glyph]));
        if let Some(n) = found {
            syllable[n].mask |= plan.mask(PREF);
            syllable[n + 1].mask |= plan.mask(PREF);
        }
    }
    for n in 1..end {
        if syllable[n].category != NON_JOINER || syllable[n].props & crate::shaping::LIGATED != 0 {
            continue;
        }
        let mut m = n;
        loop {
            m -= 1;
            syllable[m].mask &= !plan.mask(HALF);
            if m == 0 || consonant(&syllable[m]) {
                break;
            }
        }
    }
}

/// The final Indic reordering of a syllable, after the consonant form features: a pre-base
/// matra moves after the half forms the font made; a reph to its script's position; a pre-base
/// ra the font formed before the base; and a word-initial pre-base matra gets `init`.
fn finally(font: &FontFile, plan: &Syllabic, glyphs: &mut [Laid], start: usize, end: usize) {
    let virama = font.glyph(plan.config.virama);
    for laid in glyphs[start..end].iter_mut().filter(|laid| virama != 0 && laid.glyph == virama) {
        if laid.props & crate::shaping::LIGATED != 0 && laid.props & crate::shaping::MULTIPLIED != 0 {
            laid.category = VIRAMA;
            laid.props &= !(crate::shaping::LIGATED | crate::shaping::MULTIPLIED);
        }
    }
    let (mut base, try_pref) = final_base(plan, &mut glyphs[start..end]);
    base += start;
    if start + 1 < end && start < base {
        base = matra_moved(plan, glyphs, start, end, base);
    }
    let reph_moves = start + 1 < end && glyphs[start].position == P_REPH && ((glyphs[start].category == REPHA) ^ ligated_only(&glyphs[start]));
    if reph_moves {
        let to = reph_place(plan, &glyphs[start..end], base - start) + start;
        glyphs[start..=to].rotate_left(1);
        if start < base && base <= to {
            base -= 1;
        }
    }
    if try_pref && base + 1 < end {
        prebase_moved(plan, glyphs, start, end, base);
    }
    if glyphs[start].position == P_MATRA_BEFORE {
        let word_start = start == 0 || !letter_or_mark(glyphs[start - 1].letter);
        if word_start {
            glyphs[start].mask |= plan.mask(INIT);
        }
    }
}

/// Whether a glyph is a ligature and not a multiple substitution's component.
fn ligated_only(laid: &Laid) -> bool {
    laid.props & crate::shaping::LIGATED != 0 && laid.props & crate::shaping::MULTIPLIED == 0
}

/// Whether a character is a letter, mark, format character or unassigned: what a pre-base matra
/// doesn't start a word after.
fn letter_or_mark(letter: char) -> bool {
    letter.is_alphabetic() || mark(letter) || matches!(letter as u32, 0xad | 0x600..=0x605 | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0xfeff | 0xe000..=0xf8ff)
}

/// The syllable base again, after the consonant form features: where glyph positions reach the
/// base's; but an unformed pre-base ra leaves the base after it; in Malayalam, below-base
/// consonants that formed nothing become the base; and the base is never a nukta or virama.
/// Returns whether a pre-base ra is still to be sought.
fn final_base(plan: &Syllabic, syllable: &mut [Laid]) -> (usize, bool) {
    let end = syllable.len();
    let mut try_pref = plan.has(PREF);
    let mut base = syllable.iter().position(|laid| laid.position >= P_BASE).unwrap_or(end);
    if base < end {
        let unformed = if try_pref && base + 1 < end { unformed_prebase(plan, syllable, base) } else { None };
        if let Some(after) = unformed {
            (base, try_pref) = (after, false);
        }
        if plan.tag() == b"mlym" {
            base = malayalam_base(syllable, base);
        }
        if base > 0 && base < end && syllable[base].position > P_BASE {
            base -= 1;
        }
    }
    if base == end && base > 0 && one_of(&syllable[base - 1], bits(&[JOINER])) {
        base -= 1;
    }
    if base < end {
        while base > 0 && one_of(&syllable[base], bits(&[NUKTA, VIRAMA])) {
            base -= 1;
        }
    }
    (base, try_pref)
}

/// The base after a ra the font was asked to form pre-base (`pref`) but didn't: the glyph after
/// it (and after the viramas there), which becomes the base. `None` if there is no such ra
/// after the base, or the font formed it.
fn unformed_prebase(plan: &Syllabic, syllable: &mut [Laid], base: usize) -> Option<usize> {
    let end = syllable.len();
    let n = (base + 1..end).find(|n| syllable[*n].mask & plan.mask(PREF) != 0)?;
    if syllable[n].props & crate::shaping::SUBSTITUTED != 0 && ligated_only(&syllable[n]) {
        return None;
    }
    let after = n + syllable[n..].iter().take_while(|laid| virama(laid)).count();
    if after < end {
        syllable[after].position = P_BASE;
    }
    Some(after)
}

/// In Malayalam, the base moves on over below-base consonants that formed nothing (but not over
/// post-base ones).
fn malayalam_base(syllable: &mut [Laid], mut base: usize) -> usize {
    let end = syllable.len();
    let mut n = base + 1;
    while n < end {
        n += syllable[n..].iter().take_while(|laid| joiner(laid)).count();
        if n == end || !virama(&syllable[n]) {
            break;
        }
        n += 1;
        n += syllable[n..].iter().take_while(|laid| joiner(laid)).count();
        if n < end && consonant(&syllable[n]) && syllable[n].position == P_BELOW {
            base = n;
            syllable[base].position = P_BASE;
        }
        n += 1;
    }
    base
}

/// Moves a pre-base matra (placed first by the initial reordering) to just before the base, or
/// after the last virama before it that formed no half form (one followed by a joiner doesn't
/// count). Returns the new base position.
fn matra_moved(plan: &Syllabic, glyphs: &mut [Laid], start: usize, end: usize, mut base: usize) -> usize {
    let mut place = if base == end { base - 2 } else { base - 1 };
    let looks = plan.tag() != b"mlym" && plan.tag() != b"taml";
    if looks {
        place = matra_place(glyphs, start, end, place);
    }
    if start < place && glyphs[place].position != P_MATRA_BEFORE {
        let mut n = place;
        while n > start {
            if glyphs[n - 1].position == P_MATRA_BEFORE {
                let old = n - 1;
                base -= (old < base && base <= place) as usize;
                glyphs[old..=place].rotate_left(1);
                place -= 1;
            }
            n -= 1;
        }
    }
    base
}

/// Where a pre-base matra moves, looking back from `place`: after the last virama not its own
/// (one followed by a joiner doesn't count); with none, it stays (at the start).
fn matra_place(glyphs: &[Laid], start: usize, end: usize, mut place: usize) -> usize {
    loop {
        while place > start && !one_of(&glyphs[place], bits(&[MATRA, MATRA_AFTER, VIRAMA])) {
            place -= 1;
        }
        if !(virama(&glyphs[place]) && glyphs[place].position != P_MATRA_BEFORE) {
            return start;
        }
        if place + 1 < end && glyphs[place + 1].category == JOINER && place > start {
            place -= 1;
            continue;
        }
        return place;
    }
}

/// The reph's position in a syllable with base `base`: after the first virama before the base
/// (and a following joiner), unless its script puts it after the base and below-base
/// consonants; failing all, at the end, before syllable-final modifiers.
fn reph_place(plan: &Syllabic, syllable: &[Laid], base: usize) -> usize {
    let end = syllable.len();
    let reph = plan.config.reph;
    let after_halant = || {
        let n = 1 + syllable[1..base.max(1)].iter().take_while(|laid| !virama(laid)).count();
        (n < base && virama(&syllable[n])).then(|| if n + 1 < base && joiner(&syllable[n + 1]) { n + 1 } else { n })
    };
    if reph != AFTER_POST {
        if let Some(n) = after_halant() {
            return n;
        }
        // After the base only if there is one: a baseless syllable continues below.
        let along = |stops: &dyn Fn(&Laid) -> bool| (base < end).then(|| base + syllable[base + 1..].iter().take_while(|laid| !stops(laid)).count());
        if let (AFTER_MAIN, Some(n)) = (reph, along(&|laid: &Laid| laid.position > P_AFTER_MAIN)) {
            return n;
        }
        if let (AFTER_SUB, Some(n)) = (reph, along(&|laid: &Laid| matches!(laid.position, P_POST | P_AFTER_POST | P_SIGNS))) {
            return n;
        }
    }
    if let Some(n) = after_halant() {
        return n;
    }
    let mut n = end - 1;
    while n > 0 && syllable[n].position == P_SIGNS {
        n -= 1;
    }
    // After a matra and virama, the reph goes before the virama, to meet the matra: one
    // position back per post-base matra, counted while before the reph.
    if virama(&syllable[n]) {
        let mut m = base + 1;
        while m < n {
            n -= (MATRAS & (1u64 << syllable[m].category.min(63)) != 0) as usize;
            m += 1;
        }
    }
    n
}

/// Moves a ra the font formed pre-base (`pref`) before the base: after the last virama before
/// it (and a following joiner), where a matra would go.
fn prebase_moved(plan: &Syllabic, glyphs: &mut [Laid], start: usize, end: usize, base: usize) {
    let Some(old) = (base + 1..end).find(|n| glyphs[*n].mask & plan.mask(PREF) != 0) else { return };
    if !ligated_only(&glyphs[old]) {
        return;
    }
    let mut place = base;
    if plan.tag() != b"mlym" && plan.tag() != b"taml" {
        while place > start && !one_of(&glyphs[place - 1], bits(&[MATRA, MATRA_AFTER, VIRAMA])) {
            place -= 1;
        }
    }
    if place > start && virama(&glyphs[place - 1]) && place < end && joiner(&glyphs[place]) {
        place += 1;
    }
    glyphs[place..=old].rotate_right(1);
}

// ---- Khmer, Myanmar, and the Universal Shaping Engine ----

/// Khmer reordering: in each syllable, coeng plus ro (the first of up to two coengs) moves to
/// its start and gets `pref`, what follows gets `cfar`; a pre-base vowel moves to the start;
/// all after the first glyph gets the below, above and post-base forms.
fn khmer(font: &FontFile, plan: &Syllabic, glyphs: &mut Vec<Laid>) {
    dotted_circles(font, glyphs, BROKEN[1], DOTTED, Some(REPHA), 0);
    each_syllable(glyphs, |glyphs, start, end| {
        if matches!(kind(&glyphs[start]), 0 | 1) {
            khmer_syllable(plan, &mut glyphs[start..end]);
        }
    });
}

/// Khmer reordering of one syllable (see `khmer`).
fn khmer_syllable(plan: &Syllabic, syllable: &mut [Laid]) {
    let end = syllable.len();
    let forms = plan.mask(BLWF) | plan.mask(ABVF) | plan.mask(PSTF);
    syllable[1..].iter_mut().for_each(|laid| laid.mask |= forms);
    let mut coengs = 0;
    for n in 1..end {
        if syllable[n].category == VIRAMA && coengs <= 2 && n + 1 < end {
            coengs += 1;
            if syllable[n + 1].category != RA {
                continue;
            }
            syllable[n].mask |= plan.mask(PREF);
            syllable[n + 1].mask |= plan.mask(PREF);
            syllable[..n + 2].rotate_right(2);
            syllable[n + 2..].iter_mut().for_each(|laid| laid.mask |= plan.mask(CFAR));
            coengs = 2;
        } else if syllable[n].category == BEFORE {
            syllable[..=n].rotate_right(1);
        }
    }
}

/// Myanmar reordering: each syllable's positions assigned (kinzi after the base; pre-base,
/// base, then medial ra and pre-base vowels; below-base signs, then post-base), and its glyphs
/// sorted.
fn myanmar(font: &FontFile, glyphs: &mut Vec<Laid>) {
    dotted_circles(font, glyphs, BROKEN[2], DOTTED, None, 0);
    each_syllable(glyphs, |glyphs, start, end| {
        if matches!(kind(&glyphs[start]), 0 | 2) {
            myanmar_syllable(&mut glyphs[start..end]);
        }
    });
}

/// Myanmar reordering of one syllable (see `myanmar`).
fn myanmar_syllable(syllable: &mut [Laid]) {
    let end = syllable.len();
    let kinzi = end >= 3 && syllable[0].category == RA && syllable[1].category == ASAT && syllable[2].category == VIRAMA;
    let limit = if kinzi { 3 } else { 0 };
    let base = (limit..end).find(|n| consonant(&syllable[*n])).unwrap_or(if kinzi { 0 } else { limit });
    let mut n = 0;
    while n < limit {
        syllable[n].position = P_AFTER_MAIN;
        n += 1;
    }
    while n < base {
        syllable[n].position = P_BEFORE;
        n += 1;
    }
    if n < end {
        syllable[n].position = P_BASE;
        n += 1;
    }
    let mut place = P_AFTER_MAIN;
    for m in n..end {
        let category = syllable[m].category;
        syllable[m].position = match category {
            MEDIAL_RA => P_BEFORE,
            BEFORE => P_MATRA_BEFORE,
            SELECTOR => syllable[m - 1].position,
            BELOW if place == P_AFTER_MAIN => {
                place = P_BELOW;
                place
            }
            VEDIC if place == P_BELOW => P_BEFORE_SUB,
            BELOW if place == P_BELOW => place,
            _ if place == P_BELOW => {
                place = P_AFTER_SUB;
                place
            }
            _ => place,
        };
    }
    settle(syllable, |laid| laid.position);
    let lefts: Vec<usize> = (0..end).filter(|n| syllable[*n].position == P_MATRA_BEFORE).collect();
    let (Some(first), Some(last)) = (lefts.first().copied(), lefts.last().copied()) else { return };
    if first < last {
        syllable[first..=last].reverse();
        let mut from = first;
        for m in first..=last {
            if syllable[m].category == BEFORE {
                syllable[from..=m].reverse();
                from = m + 1;
            }
        }
    }
}

/// The Universal Shaping Engine's reph candidates: each syllable's first glyph if it starts
/// with a repha, else its first three. And where the font has joining forms, a word's syllables
/// take them, as Arabic letters do.
fn universal_masks(plan: &Syllabic, glyphs: &mut [Laid]) {
    let reph = plan.mask(RPHF);
    let mut start = 0;
    while reph != 0 && start < glyphs.len() {
        let end = start + glyphs[start..].iter().take_while(|laid| laid.syllable == glyphs[start].syllable).count();
        let limit = if glyphs[start].category == U_REPHA { 1 } else { 3.min(end - start) };
        glyphs[start..start + limit].iter_mut().for_each(|laid| laid.mask |= reph);
        start = end;
    }
    // A joining script already took forms per letter.
    let forms = plan.masks & (ISOL | FINA | MEDI | INIT_FORM);
    if forms == 0 || plan.joins {
        return;
    }
    let (mut last_start, mut last_form, mut start) = (0, None, 0);
    while start < glyphs.len() {
        let end = start + glyphs[start..].iter().take_while(|laid| laid.syllable == glyphs[start].syllable).count();
        if matches!(kind(&glyphs[start]), 7 | 9) {
            (last_form, last_start, start) = (None, start, end);
            continue;
        }
        let join = matches!(last_form, Some(FINA | ISOL));
        if join {
            let form = if last_form == Some(FINA) { MEDI } else { INIT_FORM };
            glyphs[last_start..start].iter_mut().for_each(|laid| laid.mask = (laid.mask & !forms) | (form & plan.masks));
        }
        let form = if join { FINA } else { ISOL };
        last_form = Some(form);
        glyphs[start..end].iter_mut().for_each(|laid| laid.mask = (laid.mask & !forms) | (form & plan.masks));
        (last_start, start) = (start, end);
    }
}

/// A glyph the font made a reph of (`rphf`) is recorded as a repha.
fn reph_recorded(plan: &Syllabic, syllable: &mut [Laid]) {
    let found = syllable.iter_mut().take_while(|laid| laid.mask & plan.mask(RPHF) != 0).find(|laid| laid.props & crate::shaping::SUBSTITUTED != 0);
    if let Some(laid) = found {
        laid.category = U_REPHA;
    }
}

/// A glyph the font substituted by `pref` is recorded as a pre-base vowel.
fn prebase_recorded(syllable: &mut [Laid]) {
    if let Some(laid) = syllable.iter_mut().find(|laid| laid.props & crate::shaping::SUBSTITUTED != 0) {
        laid.category = U_VOWEL_BEFORE;
    }
}

/// The categories a reph goes after: post-base ones.
const POST_BASE: u64 = (1 << 24)
    | (1 << 25)
    | (1 << 26)
    | (1 << 45)
    | (1 << 46)
    | (1 << 47)
    | (1 << 27)
    | (1 << 28)
    | (1 << 29)
    | (1 << 30)
    | (1 << 33)
    | (1 << 34)
    | (1 << 35)
    | (1 << 22)
    | (1 << 37)
    | (1 << 38)
    | (1 << 39)
    | (1 << 23);

/// The Universal Shaping Engine's reordering: dotted circles into broken clusters; in each
/// cluster a repha moves before the post-base part (or to the end), and a pre-base vowel or
/// modifier to just after the last halant before it.
fn universal_reordered(font: &FontFile, glyphs: &mut Vec<Laid>) {
    dotted_circles(font, glyphs, BROKEN[3], U_BASE, Some(U_REPHA), 0);
    each_syllable(glyphs, |glyphs, start, end| {
        if matches!(kind(&glyphs[start]), 1 | 2 | 3 | 8) {
            universal_syllable(&mut glyphs[start..end]);
        }
    });
}

/// Whether a glyph is a Universal Shaping Engine halant (and didn't ligate).
fn halant(laid: &Laid) -> bool {
    matches!(laid.category, U_HALANT | U_HALANT_MODIFIER | U_HALANT_OR_MODIFIER) && laid.props & crate::shaping::LIGATED == 0
}

/// The Universal Shaping Engine's reordering of one cluster (see `universal_reordered`).
fn universal_syllable(syllable: &mut [Laid]) {
    let end = syllable.len();
    if syllable[0].category == U_REPHA && end > 1 {
        for n in 1..end {
            let post = (syllable[n].category < 64 && POST_BASE & (1 << syllable[n].category) != 0) || halant(&syllable[n]);
            if post || n == end - 1 {
                let to = if post { n - 1 } else { n };
                syllable[..=to].rotate_left(1);
                break;
            }
        }
    }
    let mut after = 0;
    for n in 0..end {
        let category = syllable[n].category;
        if halant(&syllable[n]) {
            after = n + 1;
        } else if matches!(category, U_VOWEL_BEFORE | U_MODIFIER_BEFORE) && syllable[n].component() == 0 && after < n {
            syllable[after..=n].rotate_right(1);
        }
    }
}

// ---- tables ----

/// Each script's shaper (I Indic, K Khmer, M Myanmar, T Thai and Lao, U Universal Shaping
/// Engine), and whether right-to-left (1) and Arabic-like joining (2): by index in
/// `scripts.rs`'s list.
#[rustfmt::skip]
const WAYS: [(char, u8); 101] = [
    ('I', 0) /* Devanagari */, ('I', 0) /* Bengali */, ('I', 0) /* Gurmukhi */, ('I', 0) /* Gujarati */,
    ('I', 0) /* Oriya */, ('I', 0) /* Tamil */, ('I', 0) /* Telugu */, ('I', 0) /* Kannada */,
    ('I', 0) /* Malayalam */, ('U', 0) /* Sinhala */, ('T', 0) /* Thai */, ('T', 0) /* Lao */,
    ('U', 0) /* Tibetan */, ('M', 0) /* Myanmar */, ('K', 0) /* Khmer */, ('U', 0) /* Balinese */,
    ('U', 0) /* Javanese */, ('U', 0) /* Sundanese */, ('U', 0) /* Buginese */, ('U', 0) /* Tai_Tham */,
    ('U', 0) /* Cham */, ('U', 0) /* Batak */, ('U', 0) /* Lepcha */, ('U', 0) /* Limbu */,
    ('U', 0) /* Tai_Le */, ('U', 0) /* Tagalog */, ('U', 0) /* Hanunoo */, ('U', 0) /* Buhid */,
    ('U', 0) /* Tagbanwa */, ('U', 0) /* Syloti_Nagri */, ('U', 0) /* Saurashtra */, ('U', 0) /* Kayah_Li */,
    ('U', 0) /* Rejang */, ('U', 0) /* Meetei_Mayek */, ('U', 0) /* Tai_Viet */, ('U', 0) /* Brahmi */,
    ('U', 0) /* Kaithi */, ('U', 0) /* Chakma */, ('U', 0) /* Sharada */, ('U', 0) /* Takri */,
    ('U', 0) /* Grantha */, ('U', 0) /* Khojki */, ('U', 0) /* Khudawadi */, ('U', 0) /* Mahajani */,
    ('U', 0) /* Modi */, ('U', 0) /* Siddham */, ('U', 0) /* Tirhuta */, ('U', 0) /* Ahom */,
    ('U', 0) /* Multani */, ('U', 0) /* Bhaiksuki */, ('U', 0) /* Marchen */, ('U', 0) /* Newa */,
    ('U', 0) /* Masaram_Gondi */, ('U', 0) /* Soyombo */, ('U', 0) /* Zanabazar_Square */, ('U', 0) /* Dogra */,
    ('U', 0) /* Gunjala_Gondi */, ('U', 0) /* Makasar */, ('U', 0) /* Nandinagari */, ('U', 0) /* Dives_Akuru */,
    ('U', 0) /* Kawi */, ('U', 0) /* Tulu_Tigalari */, ('U', 0) /* Tai_Yo */, ('U', 2) /* Mongolian */,
    ('U', 1) /* Kharoshthi */, ('U', 0) /* Tifinagh */, ('U', 3) /* Nko */, ('U', 2) /* Phags_Pa */,
    ('U', 0) /* Egyptian_Hieroglyphs */, ('U', 3) /* Mandaic */, ('U', 0) /* Miao */, ('U', 0) /* Duployan */,
    ('U', 3) /* Manichaean */, ('U', 0) /* Pahawh_Hmong */, ('U', 3) /* Psalter_Pahlavi */, ('U', 3) /* Adlam */,
    ('U', 3) /* Hanifi_Rohingya */, ('U', 0) /* Medefaidrin */, ('U', 1) /* Old_Sogdian */, ('U', 3) /* Sogdian */,
    ('U', 1) /* Elymaic */, ('U', 0) /* Nyiakeng_Puachue_Hmong */, ('U', 0) /* Wancho */, ('U', 3) /* Chorasmian */,
    ('U', 0) /* Khitan_Small_Script */, ('U', 1) /* Yezidi */, ('U', 0) /* Cypro_Minoan */, ('U', 3) /* Old_Uyghur */,
    ('U', 0) /* Tangsa */, ('U', 0) /* Toto */, ('U', 0) /* Vithkuqi */, ('U', 0) /* Nag_Mundari */,
    ('U', 1) /* Garay */, ('U', 0) /* Gurung_Khema */, ('U', 0) /* Kirat_Rai */, ('U', 0) /* Ol_Onal */,
    ('U', 0) /* Sunuwar */, ('U', 0) /* Todhri */, ('U', 0) /* Beria_Erfe */, ('U', 1) /* Sidetic */,
    ('U', 0) /* Tolong_Siki */,
];
/// `INDIC`'s values: category (times 256) and position.
#[rustfmt::skip]
const VALUES: [u16; 42] = [
    0x000e, 0x0a04, 0x390d, 0x080d, 0x0204, 0x0104, 0x0f04, 0x0709, 0x030e, 0x110d, 0x0702, 0x0408,
    0x090d, 0x070c, 0x0d0c, 0x0708, 0x1004, 0x0807, 0x0705, 0x0406, 0x0707, 0x1204, 0x0e0e, 0x170b,
    0x1406, 0x1508, 0x1603, 0x040e, 0x200e, 0x260e, 0x240e, 0x250e, 0x230e, 0x290e, 0x270e, 0x1a0e,
    0x1b0e, 0x190e, 0x050e, 0x060e, 0x0b04, 0x280e,
];
/// Indic, Khmer and Myanmar categories and positions, in ranges: each range's first character
/// (times 64) with its value's index in `VALUES`. HarfBuzz's table, from Unicode 17.
#[rustfmt::skip]
const INDIC: [u32; 487] = [
    0xb41, 0xb80, 0xc01, 0xe80, 0x2801, 0x2840, 0x2c82, 0x2d00, 0x35c1, 0x3600, 0x24003, 0x24104,
    0x24545, 0x24c06, 0x24c45, 0x24e87, 0x24f08, 0x24f49, 0x24f87, 0x24fca, 0x25007, 0x2534b, 0x2538a, 0x253c7,
    0x25400, 0x2544c, 0x254c3, 0x25547, 0x25605, 0x25804, 0x25887, 0x25900, 0x25981, 0x25c00, 0x25c84, 0x25e05,
    0x26001, 0x26043, 0x26100, 0x26144, 0x26340, 0x263c4, 0x26440, 0x264c4, 0x26545, 0x26a40, 0x26a85, 0x26c06,
    0x26c40, 0x26c85, 0x26cc0, 0x26d85, 0x26e80, 0x26f08, 0x26f49, 0x26f8d, 0x26fca, 0x2700d, 0x27047, 0x27140,
    0x271ca, 0x27240, 0x272cd, 0x2734b, 0x27385, 0x273c0, 0x275cd, 0x27600, 0x27705, 0x27780, 0x277c5, 0x27804,
    0x27887, 0x27900, 0x27981, 0x27c06, 0x27c45, 0x27c80, 0x27f01, 0x27f40, 0x27f83, 0x27fc0, 0x28043, 0x28100,
    0x28144, 0x282c0, 0x283c4, 0x28440, 0x284c4, 0x28545, 0x28a40, 0x28a85, 0x28c06, 0x28c40, 0x28c85, 0x28d00,
    0x28d45, 0x28dc0, 0x28e05, 0x28e80, 0x28f08, 0x28f40, 0x28f8d, 0x28fca, 0x2900e, 0x2904d, 0x290c0, 0x291cd,
    0x29240, 0x292cd, 0x2934b, 0x29380, 0x2944f, 0x29480, 0x29645, 0x29740, 0x29785, 0x297c0, 0x29981, 0x29c03,
    0x29c85, 0x29d00, 0x29d50, 0x29d80, 0x2a043, 0x2a100, 0x2a144, 0x2a380, 0x2a3c4, 0x2a480, 0x2a4c4, 0x2a545,
    0x2aa40, 0x2aa85, 0x2ac06, 0x2ac40, 0x2ac85, 0x2ad00, 0x2ad45, 0x2ae80, 0x2af08, 0x2af49, 0x2af8d, 0x2afca,
    0x2b00d, 0x2b147, 0x2b180, 0x2b1c7, 0x2b24d, 0x2b280, 0x2b2cd, 0x2b34b, 0x2b380, 0x2b804, 0x2b88d, 0x2b900,
    0x2b981, 0x2bc00, 0x2be45, 0x2be8c, 0x2bec8, 0x2bf0c, 0x2bf48, 0x2c000, 0x2c051, 0x2c083, 0x2c100, 0x2c144,
    0x2c340, 0x2c3c4, 0x2c440, 0x2c4c4, 0x2c545, 0x2ca40, 0x2ca85, 0x2cc06, 0x2cc40, 0x2cc85, 0x2cd00, 0x2cd45,
    0x2ce80, 0x2cf08, 0x2cf49, 0x2cf8d, 0x2cfd2, 0x2d00d, 0x2d047, 0x2d140, 0x2d1ca, 0x2d212, 0x2d240, 0x2d2cd,
    0x2d34b, 0x2d380, 0x2d548, 0x2d592, 0x2d5cd, 0x2d600, 0x2d705, 0x2d780, 0x2d7c5, 0x2d804, 0x2d887, 0x2d900,
    0x2d981, 0x2dc00, 0x2dc45, 0x2dc80, 0x2e083, 0x2e0c0, 0x2e144, 0x2e2c0, 0x2e384, 0x2e440, 0x2e484, 0x2e545,
    0x2e580, 0x2e645, 0x2e6c0, 0x2e705, 0x2e740, 0x2e785, 0x2e800, 0x2e8c5, 0x2e940, 0x2ea05, 0x2eac0, 0x2eb85,
    0x2ec06, 0x2ec45, 0x2ee80, 0x2ef8d, 0x2f007, 0x2f04d, 0x2f0c0, 0x2f18a, 0x2f240, 0x2f28d, 0x2f353, 0x2f380,
    0x2f5cd, 0x2f600, 0x2f981, 0x2fc00, 0x30003, 0x30144, 0x30340, 0x30384, 0x30440, 0x30484, 0x30545, 0x30a40,
    0x30a85, 0x30c06, 0x30c45, 0x30e80, 0x30f08, 0x30f49, 0x30f94, 0x310c7, 0x31140, 0x31194, 0x31240, 0x31294,
    0x31353, 0x31380, 0x31554, 0x315c0, 0x31605, 0x316c0, 0x31745, 0x31780, 0x31804, 0x31894, 0x31900, 0x31981,
    0x31c00, 0x32001, 0x32043, 0x32100, 0x32144, 0x32340, 0x32384, 0x32440, 0x32484, 0x32545, 0x32a40, 0x32a85,
    0x32c06, 0x32c45, 0x32d00, 0x32d45, 0x32e80, 0x32f08, 0x32f49, 0x32f94, 0x330c7, 0x33140, 0x33194, 0x331c7,
    0x33240, 0x33287, 0x33314, 0x33353, 0x33380, 0x33547, 0x335c0, 0x33745, 0x337c0, 0x33804, 0x33894, 0x33900,
    0x33981, 0x33c00, 0x33c55, 0x33cc3, 0x33d00, 0x34003, 0x34101, 0x34144, 0x34340, 0x34384, 0x34440, 0x34484,
    0x34545, 0x34c06, 0x34c45, 0x34ec7, 0x34f49, 0x34f8d, 0x35140, 0x3518a, 0x35240, 0x3528d, 0x35353, 0x35396,
    0x353c0, 0x35505, 0x355cd, 0x35600, 0x357c4, 0x3588d, 0x35900, 0x35981, 0x35c00, 0x35e85, 0x36000, 0x40005,
    0x40106, 0x40145, 0x406c6, 0x40705, 0x40844, 0x40ad7, 0x40b58, 0x40bd9, 0x40c5a, 0x40c8c, 0x40cd8, 0x40d8c,
    0x40dc8, 0x40e03, 0x40e5b, 0x40e9c, 0x40edd, 0x40f1e, 0x40f5f, 0x40fa0, 0x40fc5, 0x41001, 0x41300, 0x41385,
    0x413c0, 0x41405, 0x41484, 0x41597, 0x41619, 0x41686, 0x416c5, 0x4179d, 0x41821, 0x41845, 0x41897, 0x418e2,
    0x41945, 0x419d7, 0x41a62, 0x41b85, 0x41c58, 0x41d45, 0x4209f, 0x420d7, 0x4211a, 0x42158, 0x421c3, 0x42385,
    0x423c3, 0x42401, 0x42683, 0x42758, 0x42780, 0x5e005, 0x5e686, 0x5e6c5, 0x5e8c4, 0x5ed00, 0x5ed97, 0x5edd8,
    0x5eed9, 0x5ef98, 0x5efd7, 0x5f05a, 0x5f117, 0x5f1a3, 0x5f1e4, 0x5f265, 0x5f2e3, 0x5f325, 0x5f363, 0x5f49b,
    0x5f4e4, 0x5f500, 0x5f641, 0x5f680, 0x5f709, 0x5f764, 0x5f780, 0x5f801, 0x5fa80, 0x7340c, 0x734c0, 0x7350c,
    0x73a49, 0x73b4c, 0x73b89, 0x73c85, 0x73d0c, 0x73d55, 0x73dcc, 0x73e81, 0x73ec0, 0x80326, 0x80367, 0x80380,
    0x80401, 0x80580, 0x80881, 0x808c0, 0x81d02, 0x81d40, 0x82082, 0x82140, 0x97328, 0x97340, 0x97ec1, 0x97fc0,
    0x2a380c, 0x2a3c89, 0x2a3e00, 0x2a3f84, 0x2a3fc7, 0x2a4000, 0x2a7805, 0x2a7958, 0x2a7980, 0x2a79c5, 0x2a7c01, 0x2a7e85,
    0x2a7fc0, 0x2a9805, 0x2a9c00, 0x2a9c45, 0x2a9d01, 0x2a9dc0, 0x2a9e85, 0x2a9ee2, 0x2a9f08, 0x2a9f85, 0x2aa000, 0x3f8029,
    0x3f8400, 0x44c043, 0x44c100, 0x44cec8, 0x44cf40, 0x45b401, 0x45b900,
];
/// Universal Shaping Engine categories, in ranges: each range's first character (times 64) with
/// its category. HarfBuzz's table, from Unicode 17.
#[rustfmt::skip]
const UNIVERSAL: [u32; 1373] = [
    0xb45, 0xb80, 0xc01, 0xe80, 0x16f3, 0x1700, 0x1774, 0x1780, 0x1ef3, 0x1f00, 0x1f74, 0x1f80,
    0x2805, 0x2840, 0x2b50, 0x2b80, 0x2caf, 0x2d00, 0x35c5, 0x3600, 0xd3c6, 0xd400, 0x19001, 0x19040,
    0x1f281, 0x1fae5, 0x1fd00, 0x1fe81, 0x1fec0, 0x1ff65, 0x1ff80, 0x21001, 0x21660, 0x21700, 0x24025, 0x240e7,
    0x24101, 0x24ea1, 0x24ee3, 0x24f20, 0x24f41, 0x24fa3, 0x24fd6, 0x25023, 0x25062, 0x25161, 0x25263, 0x2534c,
    0x25396, 0x253e3, 0x25400, 0x25465, 0x254a6, 0x254c0, 0x25561, 0x255a2, 0x25601, 0x258a2, 0x25900, 0x25981,
    0x25c00, 0x25c81, 0x26005, 0x26065, 0x260a7, 0x26100, 0x26141, 0x26340, 0x263c1, 0x26440, 0x264c1, 0x26a40,
    0x26a81, 0x26c40, 0x26c81, 0x26cc0, 0x26d81, 0x26e80, 0x26f20, 0x26f41, 0x26fa3, 0x26fd6, 0x27023, 0x27062,
    0x27140, 0x271d6, 0x27240, 0x272d6, 0x2734c, 0x27380, 0x275e3, 0x27600, 0x27701, 0x27780, 0x277c1, 0x278a2,
    0x27900, 0x27981, 0x27c80, 0x27f01, 0x27f40, 0x27fad, 0x27fc0, 0x28065, 0x280e7, 0x28100, 0x28141, 0x282c0,
    0x283c1, 0x28440, 0x284c1, 0x28a40, 0x28a81, 0x28c40, 0x28c81, 0x28d00, 0x28d41, 0x28dc0, 0x28e01, 0x28e80,
    0x28f20, 0x28f40, 0x28fa3, 0x28fd6, 0x29023, 0x29062, 0x290c0, 0x291e1, 0x29240, 0x292e1, 0x2934c, 0x29380,
    0x29466, 0x29480, 0x29641, 0x29740, 0x29781, 0x297c0, 0x29981, 0x29c25, 0x29c5f, 0x29c85, 0x29d00, 0x29d5c,
    0x29d80, 0x2a065, 0x2a0e7, 0x2a100, 0x2a141, 0x2a380, 0x2a3c1, 0x2a480, 0x2a4c1, 0x2aa40, 0x2aa81, 0x2ac40,
    0x2ac81, 0x2ad00, 0x2ad41, 0x2ae80, 0x2af20, 0x2af41, 0x2afa3, 0x2afd6, 0x2b023, 0x2b062, 0x2b161, 0x2b180,
    0x2b1e1, 0x2b280, 0x2b2e3, 0x2b34c, 0x2b380, 0x2b801, 0x2b8a2, 0x2b900, 0x2b981, 0x2bc00, 0x2be41, 0x2bea5,
    0x2bedf, 0x2bf25, 0x2bf5f, 0x2c000, 0x2c065, 0x2c0a7, 0x2c100, 0x2c141, 0x2c340, 0x2c3c1, 0x2c440, 0x2c4c1,
    0x2ca40, 0x2ca81, 0x2cc40, 0x2cc81, 0x2cd00, 0x2cd41, 0x2ce80, 0x2cf20, 0x2cf41, 0x2cfa3, 0x2cfe1, 0x2d023,
    0x2d062, 0x2d140, 0x2d1d6, 0x2d240, 0x2d2d6, 0x2d34c, 0x2d380, 0x2d561, 0x2d600, 0x2d701, 0x2d780, 0x2d7c1,
    0x2d8a2, 0x2d900, 0x2d981, 0x2dc00, 0x2dc41, 0x2dc80, 0x2e0a5, 0x2e0c0, 0x2e141, 0x2e2c0, 0x2e381, 0x2e440,
    0x2e481, 0x2e580, 0x2e641, 0x2e6c0, 0x2e701, 0x2e740, 0x2e781, 0x2e800, 0x2e8c1, 0x2e940, 0x2ea01, 0x2eac0,
    0x2eb81, 0x2ee80, 0x2efa3, 0x2f021, 0x2f063, 0x2f0c0, 0x2f196, 0x2f240, 0x2f296, 0x2f34c, 0x2f380, 0x2f5e3,
    0x2f600, 0x2f981, 0x2fc00, 0x30025, 0x30067, 0x30125, 0x30141, 0x30340, 0x30381, 0x30440, 0x30481, 0x30a40,
    0x30a81, 0x30e80, 0x30f20, 0x30f41, 0x30fa1, 0x31063, 0x31140, 0x311a1, 0x31240, 0x312a1, 0x3134c, 0x31380,
    0x31561, 0x315a2, 0x315c0, 0x31601, 0x316c0, 0x31801, 0x318a2, 0x31900, 0x31981, 0x31c00, 0x32001, 0x32065,
    0x320a7, 0x32100, 0x32141, 0x32340, 0x32381, 0x32440, 0x32481, 0x32a40, 0x32a81, 0x32d00, 0x32d41, 0x32e80,
    0x32f20, 0x32f41, 0x32fa3, 0x32fe1, 0x33063, 0x33140, 0x331a1, 0x33240, 0x332a1, 0x3334c, 0x33380, 0x33563,
    0x335c0, 0x33781, 0x337c0, 0x33801, 0x338a2, 0x33900, 0x33981, 0x33c00, 0x33c6b, 0x33ce7, 0x33d00, 0x34025,
    0x340a7, 0x34101, 0x34340, 0x34381, 0x34440, 0x34481, 0x34ee1, 0x34f41, 0x34fa3, 0x35062, 0x35140, 0x35196,
    0x35240, 0x35296, 0x3534c, 0x35392, 0x353c0, 0x355e3, 0x35600, 0x357c1, 0x358a2, 0x35900, 0x35981, 0x35c00,
    0x36065, 0x360a7, 0x36100, 0x36141, 0x365c0, 0x36681, 0x36c80, 0x36cc1, 0x36f00, 0x36f41, 0x36f80, 0x37001,
    0x371c0, 0x372b5, 0x372c0, 0x373e3, 0x374a1, 0x37522, 0x37540, 0x375a2, 0x375c0, 0x37623, 0x37656, 0x377e3,
    0x37800, 0x37981, 0x37c00, 0x37ca3, 0x37d00, 0x3c001, 0x3c080, 0x3c101, 0x3c1c0, 0x3c622, 0x3c680, 0x3c801,
    0x3cd00, 0x3cd6e, 0x3cd80, 0x3cdee, 0x3ce00, 0x3ce5f, 0x3ce80, 0x3cfa3, 0x3cfd6, 0x3d001, 0x3d200, 0x3d241,
    0x3db40, 0x3dc60, 0x3dca2, 0x3dce1, 0x3dd62, 0x3dda1, 0x3dea2, 0x3dfa5, 0x3dfc0, 0x3e022, 0x3e061, 0x3e0a5,
    0x3e122, 0x3e140, 0x3e1a5, 0x3e201, 0x3e34b, 0x3e600, 0x3e64b, 0x3ef40, 0x3f1ae, 0x3f1c0, 0x5c001, 0x5c4a1,
    0x5c4e2, 0x5c563, 0x5c580, 0x5c7c1, 0x5cca1, 0x5cce2, 0x5cd23, 0x5cd40, 0x5d001, 0x5d4a1, 0x5d4e2, 0x5d500,
    0x5d801, 0x5db40, 0x5db81, 0x5dc40, 0x5dca1, 0x5dce2, 0x5dd00, 0x60001, 0x60040, 0x601c1, 0x60200, 0x60281,
    0x602c6, 0x60390, 0x603c6, 0x60400, 0x60801, 0x61e40, 0x62005, 0x6215f, 0x621c1, 0x62a60, 0x62a81, 0x62ac0,
    0x64001, 0x647c0, 0x64821, 0x648a2, 0x648e3, 0x64961, 0x64a4b, 0x64b00, 0x64c1a, 0x64ca6, 0x64cda, 0x64e59,
    0x64ea5, 0x64eee, 0x64f00, 0x65181, 0x65b80, 0x65c01, 0x65d40, 0x68001, 0x685e1, 0x68656, 0x686a3, 0x686e1,
    0x68700, 0x68801, 0x6955e, 0x6959c, 0x695cb, 0x69618, 0x6969b, 0x696cb, 0x697c0, 0x69830, 0x69863, 0x698a1,
    0x698e3, 0x69961, 0x69a62, 0x69ae1, 0x69b22, 0x69b63, 0x69b96, 0x69ce1, 0x69d25, 0x69ea1, 0x69ee5, 0x69f40,
    0x69fe6, 0x6a001, 0x6a280, 0x6a401, 0x6a680, 0x6c025, 0x6c0d8, 0x6c127, 0x6c141, 0x6cd1f, 0x6cd63, 0x6cda1,
    0x6ce22, 0x6cf21, 0x6cf96, 0x6d0a1, 0x6d10c, 0x6d141, 0x6d340, 0x6d401, 0x6d680, 0x6dae9, 0x6dd00, 0x6e025,
    0x6e058, 0x6e0a7, 0x6e0c1, 0x6e84b, 0x6e921, 0x6e962, 0x6e996, 0x6e9e3, 0x6ea21, 0x6eaa3, 0x6eaec, 0x6eb0b,
    0x6eb81, 0x6f99f, 0x6f9e3, 0x6fa21, 0x6faa3, 0x6fb61, 0x6fba3, 0x6fbe1, 0x6fc18, 0x6fcb8, 0x6fd00, 0x70001,
    0x7090b, 0x709a3, 0x709d6, 0x70aa3, 0x70b22, 0x70b58, 0x70d17, 0x70dad, 0x70de0, 0x70e00, 0x71001, 0x71280,
    0x71341, 0x71400, 0x73425, 0x734c0, 0x73526, 0x736a5, 0x73726, 0x73825, 0x73867, 0x738a6, 0x73a40, 0x73b66,
    0x73b80, 0x73d25, 0x73d6b, 0x73de7, 0x73e25, 0x73e85, 0x73ec0, 0x77eed, 0x77f00, 0x802d0, 0x8030e, 0x80346,
    0x80390, 0x80405, 0x80540, 0x80a90, 0x80bc0, 0x81810, 0x81940, 0x81990, 0x81c00, 0x81d2f, 0x81d40, 0x820af,
    0x82140, 0x83c25, 0x83c40, 0x97301, 0x97340, 0x9f9b3, 0x9f9f4, 0x9fa33, 0x9fa74, 0x9fa80, 0xb4c01, 0xb5a00,
    0xb5bc1, 0xb5c00, 0xb5fcc, 0xb6000, 0xb88b3, 0xb88f4, 0xb8933, 0xb8974, 0xb8980, 0x2a0001, 0x2a00a1, 0x2a00c1,
    0x2a018c, 0x2a01c1, 0x2a02e5, 0x2a0301, 0x2a08e3, 0x2a0962, 0x2a09a1, 0x2a09e3, 0x2a0a00, 0x2a0b22, 0x2a0b40, 0x2a1001,
    0x2a1d00, 0x2a2027, 0x2a2081, 0x2a2d1d, 0x2a2d63, 0x2a310c, 0x2a3165, 0x2a3180, 0x2a3401, 0x2a3680, 0x2a3825, 0x2a3c81,
    0x2a3d00, 0x2a3f81, 0x2a3fe1, 0x2a4001, 0x2a49a1, 0x2a4ae6, 0x2a4b80, 0x2a4c01, 0x2a51e2, 0x2a52a1, 0x2a52e2, 0x2a53d8,
    0x2a549a, 0x2a54e3, 0x2a5500, 0x2a6025, 0x2a6098, 0x2a60e7, 0x2a6101, 0x2a6cdf, 0x2a6d23, 0x2a6da1, 0x2a6e22, 0x2a6e96,
    0x2a6f21, 0x2a6f5c, 0x2a6f9d, 0x2a6fdc, 0x2a700c, 0x2a7040, 0x2a7401, 0x2a7680, 0x2a8001, 0x2a8a65, 0x2a8aa1, 0x2a8b62,
    0x2a8ba1, 0x2a8bd6, 0x2a8c61, 0x2a8ca2, 0x2a8cdd, 0x2a8d1e, 0x2a8d5b, 0x2a8d9c, 0x2a8dc0, 0x2a9001, 0x2a90d8, 0x2a9101,
    0x2a9318, 0x2a935a, 0x2a9380, 0x2a9401, 0x2a9680, 0x2aa001, 0x2aac22, 0x2aac41, 0x2aaca2, 0x2aad21, 0x2aad41, 0x2aade2,
    0x2aae41, 0x2aafa2, 0x2aafe5, 0x2ab001, 0x2ab065, 0x2ab081, 0x2ab0c0, 0x2ab801, 0x2abad6, 0x2abb22, 0x2abb61, 0x2abb96,
    0x2abbe3, 0x2abc00, 0x2abd67, 0x2abdac, 0x2abdc0, 0x2af001, 0x2af8e3, 0x2af961, 0x2af9a3, 0x2afa22, 0x2afa63, 0x2afac0,
    0x2afb27, 0x2afb62, 0x2afb80, 0x2afc01, 0x2afe80, 0x3f8006, 0x3f8400, 0x3fbfd0, 0x3fc000, 0x415c01, 0x415ec0, 0x415f01,
    0x4162c0, 0x416301, 0x4164c0, 0x416501, 0x416580, 0x4165c1, 0x416880, 0x4168c1, 0x416c80, 0x416cc1, 0x416e80, 0x416ec1,
    0x416f40, 0x428001, 0x428062, 0x428100, 0x428161, 0x4281a2, 0x4281c0, 0x428323, 0x428366, 0x4283e5, 0x428401, 0x428500,
    0x428541, 0x428600, 0x428641, 0x428d80, 0x428e20, 0x428ec0, 0x428fec, 0x429001, 0x429240, 0x42b001, 0x42b200, 0x42b241,
    0x42b960, 0x42b9c0, 0x42bac1, 0x42bc00, 0x42e001, 0x42e480, 0x42ea41, 0x42ebc0, 0x434001, 0x434925, 0x4349df, 0x434a00,
    0x434c01, 0x434e80, 0x435281, 0x435980, 0x435a61, 0x435b80, 0x435bc1, 0x436180, 0x43a001, 0x43aa80, 0x43aae1, 0x43ab40,
    0x43ac01, 0x43ac80, 0x43cc01, 0x43d1a6, 0x43d441, 0x43d540, 0x43dc01, 0x43e0a0, 0x43e180, 0x43ec01, 0x43ec40, 0x43ec81,
    0x43edc0, 0x43ee01, 0x43f000, 0x43f041, 0x43f140, 0x43f241, 0x43f300, 0x440027, 0x440065, 0x4400a7, 0x4400eb, 0x440141,
    0x440e21, 0x440f22, 0x4410a1, 0x44118c, 0x4411c0, 0x441484, 0x441981, 0x441c21, 0x441c41, 0x441ce1, 0x441d41, 0x441d80,
    0x441fcd, 0x442025, 0x4420a7, 0x4420c1, 0x442c23, 0x442c56, 0x442ca3, 0x442ce2, 0x442d61, 0x442de3, 0x442e4c, 0x442ea0,
    0x442ec0, 0x4430a2, 0x4430c0, 0x444025, 0x4440c1, 0x4449e2, 0x444aa1, 0x444b16, 0x444b62, 0x444ba1, 0x444c22, 0x444c61,
    0x444cec, 0x444d1f, 0x444d40, 0x444d81, 0x445000, 0x445101, 0x445163, 0x4451c1, 0x445200, 0x445401, 0x445ce0, 0x445d00,
    0x446025, 0x4460a7, 0x4460c1, 0x446ce3, 0x446d16, 0x446d63, 0x446da2, 0x446f21, 0x44700c, 0x447041, 0x447092, 0x447100,
    0x44726e, 0x4472a0, 0x4472e1, 0x447322, 0x447340, 0x447396, 0x4473e5, 0x447401, 0x4476c0, 0x447841, 0x447d40, 0x448001,
    0x448480, 0x4484c1, 0x448b23, 0x448be2, 0x448c21, 0x448d25, 0x448d4c, 0x448d9f, 0x448e00, 0x448fa5, 0x448fc1, 0x449062,
    0x449080, 0x44a001, 0x44a1c0, 0x44a201, 0x44a240, 0x44a281, 0x44a380, 0x44a3c1, 0x44a780, 0x44a7c1, 0x44aa40, 0x44ac01,
    0x44b7e5, 0x44b823, 0x44b856, 0x44b8a3, 0x44b8e2, 0x44b961, 0x44ba60, 0x44baa2, 0x44bac0, 0x44bc01, 0x44be80, 0x44c025,
    0x44c100, 0x44c141, 0x44c340, 0x44c3c1, 0x44c440, 0x44c4c1, 0x44ca40, 0x44ca81, 0x44cc40, 0x44cc81, 0x44cd00, 0x44cd41,
    0x44ce80, 0x44cee0, 0x44cf41, 0x44cfa3, 0x44d021, 0x44d063, 0x44d140, 0x44d1d6, 0x44d240, 0x44d2d6, 0x44d34c, 0x44d380,
    0x44d5e3, 0x44d600, 0x44d781, 0x44d8a3, 0x44d900, 0x44d9a5, 0x44db40, 0x44dc25, 0x44dd40, 0x44e001, 0x44e280, 0x44e2c1,
    0x44e300, 0x44e381, 0x44e3c0, 0x44e401, 0x44ed80, 0x44edc1, 0x44ee23, 0x44ee61, 0x44eee2, 0x44f040, 0x44f096, 0x44f0c0,
    0x44f156, 0x44f180, 0x44f1d6, 0x44f263, 0x44f2a7, 0x44f2c0, 0x44f327, 0x44f3a5, 0x44f3e0, 0x44f42c, 0x44f452, 0x44f4a0,
    0x44f4c0, 0x44f865, 0x44f8a6, 0x44f8c0, 0x450001, 0x450d63, 0x450d96, 0x450de3, 0x450e22, 0x450fa1, 0x451023, 0x45108c,
    0x4510e5, 0x451167, 0x4511a0, 0x4511c1, 0x451200, 0x451401, 0x451680, 0x4517ad, 0x4517c1, 0x45182b, 0x451880, 0x452041,
    0x452c23, 0x452c56, 0x452ca3, 0x452ce2, 0x452e56, 0x452ea1, 0x452ed6, 0x452f63, 0x452f96, 0x452fe5, 0x45308c, 0x4530e0,
    0x453101, 0x453140, 0x453401, 0x453680, 0x456001, 0x456be3, 0x456c16, 0x456c63, 0x456ca2, 0x456d80, 0x456e16, 0x456f25,
    0x456fa7, 0x456fcc, 0x457020, 0x457040, 0x457601, 0x457722, 0x457780, 0x458001, 0x458c23, 0x458ce2, 0x458e61, 0x458ee3,
    0x458f65, 0x458fa7, 0x458fcc, 0x459021, 0x459040, 0x459401, 0x459680, 0x45a001, 0x45aae5, 0x45ab27, 0x45ab61, 0x45ab96,
    0x45abe3, 0x45ac22, 0x45aca1, 0x45ad8c, 0x45ade0, 0x45ae01, 0x45ae40, 0x45b001, 0x45b280, 0x45c001, 0x45c6c0, 0x45c75c,
    0x45c79e, 0x45c7db, 0x45c823, 0x45c8a1, 0x45c922, 0x45c996, 0x45c9e1, 0x45ca22, 0x45ca61, 0x45cb00, 0x45cc01, 0x45cf00,
    0x45d001, 0x45d1c0, 0x460001, 0x460b23, 0x460b56, 0x460ba3, 0x460be2, 0x460ce1, 0x460de5, 0x460e27, 0x460e4c, 0x460ea0,
    0x460ec0, 0x464001, 0x4641c0, 0x464241, 0x464280, 0x464301, 0x464500, 0x464541, 0x4645c0, 0x464601, 0x464c23, 0x464d56,
    0x464d80, 0x464dd6, 0x464e40, 0x464ee5, 0x464f63, 0x464fac, 0x464fd2, 0x46501d, 0x465052, 0x46509d, 0x4650e0, 0x465100,
    0x465401, 0x465680, 0x466801, 0x466a00, 0x466a81, 0x467463, 0x467496, 0x4674e3, 0x467522, 0x467600, 0x4676a1, 0x467723,
    0x4677a7, 0x46780c, 0x467841, 0x467880, 0x467916, 0x467940, 0x468001, 0x468061, 0x4680a2, 0x468121, 0x4682a2, 0x4682c1,
    0x468cee, 0x468d22, 0x468d65, 0x468e67, 0x468eab, 0x468ecb, 0x468fc5, 0x469000, 0x469145, 0x469180, 0x4691ec, 0x469200,
    0x469401, 0x469461, 0x4694a2, 0x469521, 0x4695e3, 0x469662, 0x469701, 0x46a112, 0x46a299, 0x46a5a5, 0x46a5e7, 0x46a61f,
    0x46a66c, 0x46a680, 0x46a741, 0x46a780, 0x46d821, 0x46d863, 0x46d8a2, 0x46d921, 0x46d963, 0x46d9a1, 0x46d9e3, 0x46da00,
    0x470001, 0x470240, 0x470281, 0x470be3, 0x470c21, 0x470ca2, 0x470dc0, 0x470e21, 0x470f25, 0x470fa7, 0x470fcc, 0x471001,
    0x471040, 0x471401, 0x471b40, 0x471c81, 0x472400, 0x47248b, 0x472a00, 0x472a4b, 0x472c22, 0x472c56, 0x472ca2, 0x472ce1,
    0x472d23, 0x472d65, 0x472dc0, 0x474001, 0x4741c0, 0x474201, 0x474280, 0x4742c1, 0x474c61, 0x474da2, 0x474dc0, 0x474ea1,
    0x474ec0, 0x474f21, 0x474f80, 0x474fe1, 0x475025, 0x4750a0, 0x4750e1, 0x475122, 0x47516c, 0x475192, 0x4751dc, 0x475200,
    0x475401, 0x475680, 0x475801, 0x475980, 0x4759c1, 0x475a40, 0x475a81, 0x4762a3, 0x4763c0, 0x476421, 0x476480, 0x4764e3,
    0x476565, 0x4765a7, 0x4765ec, 0x476600, 0x476801, 0x476a80, 0x47b801, 0x47bc85, 0x47bce1, 0x47bd22, 0x47bd56, 0x47bda3,
    0x47bdc0, 0x47c025, 0x47c092, 0x47c0e7, 0x47c101, 0x47c440, 0x47c481, 0x47cd23, 0x47cda1, 0x47ce22, 0x47cec0, 0x47cf96,
    0x47d021, 0x47d063, 0x47d0ac, 0x47d0c0, 0x47d401, 0x47d69f, 0x47d6c0, 0x4c0031, 0x4d0c32, 0x4d0df3, 0x4d0e34, 0x4d0e72,
    0x4d0f31, 0x4d1037, 0x4d1071, 0x4d11f6, 0x4d1580, 0x4d1831, 0x50fec0, 0x584001, 0x5847a1, 0x584a9e, 0x584b1d, 0x584b65,
    0x584b9c, 0x584be2, 0x584c01, 0x584e80, 0x5ab001, 0x5ab280, 0x5ac001, 0x5acc25, 0x5acdc0, 0x5b5027, 0x5b50c1, 0x5b5ae3,
    0x5b5b40, 0x5b5c01, 0x5b5e80, 0x5bc001, 0x5bd2c0, 0x5bd3e0, 0x5bd400, 0x5bd462, 0x5be200, 0x5be3e6, 0x5be4c0, 0x5bf901,
    0x5bf940, 0x62c001, 0x633580, 0x633fc1, 0x634000, 0x6f0001, 0x6f1ac0, 0x6f1c01, 0x6f1f40, 0x6f2001, 0x6f2240, 0x6f2401,
    0x6f2680, 0x6f2760, 0x6f27c0, 0x745cd0, 0x745ec0, 0x784001, 0x784b40, 0x784c25, 0x784dc1, 0x784f80, 0x785001, 0x785280,
    0x785381, 0x785400, 0x78a401, 0x78aba5, 0x78abc0, 0x78b001, 0x78bb25, 0x78bc01, 0x78be80, 0x793401, 0x793b21, 0x793c01,
    0x793e80, 0x797401, 0x797ba2, 0x797c01, 0x797ec0, 0x7a4001, 0x7a511f, 0x7a52c1, 0x7a5300, 0x7a5401, 0x7a5680, 0x3800050,
    0x3800080, 0x3800810, 0x3802000, 0x3804006, 0x3807c00,
];
/// Combining classes (Unicode's, as HarfBuzz modifies a few) of these scripts' characters, in
/// ranges: each range's first character (times 256) with its class.
#[rustfmt::skip]
const CLASSES: [u32; 385] = [
    0x300e6, 0x315e8, 0x316dc, 0x31ae8, 0x31bd8, 0x31cdc, 0x321ca, 0x323dc, 0x327ca, 0x329dc, 0x33401, 0x339dc,
    0x33de6, 0x345f0, 0x346e6, 0x347dc, 0x34ae6, 0x34ddc, 0x34f00, 0x350e6, 0x353dc, 0x357e6, 0x358e8, 0x359dc,
    0x35be6, 0x35ce9, 0x35dea, 0x35fe9, 0x360ea, 0x362e9, 0x363e6, 0x37000, 0x7ebe6, 0x7f2dc, 0x7f3e6, 0x7f400,
    0x7fddc, 0x7fe00, 0x859dc, 0x85c00, 0x93c07, 0x93d00, 0x94d09, 0x94e00, 0x951e6, 0x952dc, 0x953e6, 0x95500,
    0x9bc07, 0x9bd00, 0x9cd09, 0x9ce00, 0x9fee6, 0x9ff00, 0xa3c07, 0xa3d00, 0xa4d09, 0xa4e00, 0xabc07, 0xabd00,
    0xacd09, 0xace00, 0xb3c07, 0xb3d00, 0xb4d09, 0xb4e00, 0xbcd09, 0xbce00, 0xc3c07, 0xc3d00, 0xc4d09, 0xc4e00,
    0xcbc07, 0xcbd00, 0xccd09, 0xcce00, 0xd3b09, 0xd3d00, 0xd4d09, 0xd4e00, 0xdca09, 0xdcb00, 0xe3803, 0xe3a09,
    0xe3b00, 0xe486b, 0xe4c00, 0xeb876, 0xeba09, 0xebb00, 0xec87a, 0xecc00, 0xf18dc, 0xf1a00, 0xf35dc, 0xf3600,
    0xf37dc, 0xf3800, 0xf397f, 0xf3a00, 0xf7181, 0xf7284, 0xf7300, 0xf7483, 0xf7500, 0xf7a84, 0xf7e00, 0xf8084,
    0xf8100, 0xf82e6, 0xf8409, 0xf8500, 0xf86e6, 0xf8800, 0xfc6fe, 0xfc700, 0x103707, 0x103800, 0x103909, 0x103b00,
    0x108ddc, 0x108e00, 0x171409, 0x171600, 0x173409, 0x173500, 0x17d209, 0x17d300, 0x17dde6, 0x17de00, 0x18a9e4, 0x18aa00,
    0x1939de, 0x193ae6, 0x193bdc, 0x193c00, 0x1a17e6, 0x1a18dc, 0x1a1900, 0x1a60fe, 0x1a6100, 0x1a75e6, 0x1a7d00, 0x1a7fdc,
    0x1a8000, 0x1b3407, 0x1b3500, 0x1b4409, 0x1b4500, 0x1b6be6, 0x1b6cdc, 0x1b6de6, 0x1b7400, 0x1baa09, 0x1bac00, 0x1be607,
    0x1be700, 0x1bf209, 0x1bf400, 0x1c3707, 0x1c3800, 0x1cd0e6, 0x1cd300, 0x1cd401, 0x1cd5dc, 0x1cdae6, 0x1cdcdc, 0x1ce0e6,
    0x1ce100, 0x1ce201, 0x1ce900, 0x1ceddc, 0x1cee00, 0x1cf4e6, 0x1cf500, 0x1cf8e6, 0x1cfa00, 0x1dc0e6, 0x1dc2dc, 0x1dc3e6,
    0x1dcadc, 0x1dcbe6, 0x1dcdea, 0x1dced6, 0x1dcfdc, 0x1dd0ca, 0x1dd1e6, 0x1df6e8, 0x1df7e4, 0x1df9dc, 0x1dfada, 0x1dfbe6,
    0x1dfce9, 0x1dfddc, 0x1dfee6, 0x1dffdc, 0x1e0000, 0x20d0e6, 0x20d201, 0x20d4e6, 0x20d801, 0x20dbe6, 0x20dd00, 0x20e1e6,
    0x20e200, 0x20e501, 0x20e7e6, 0x20e8dc, 0x20e9e6, 0x20ea01, 0x20ecdc, 0x20f0e6, 0x20f100, 0x2d7f09, 0x2d8000, 0xa80609,
    0xa80700, 0xa82c09, 0xa82d00, 0xa8c409, 0xa8c500, 0xa8e0e6, 0xa8f200, 0xa92bdc, 0xa92e00, 0xa95309, 0xa95400, 0xa9b307,
    0xa9b400, 0xa9c009, 0xa9c100, 0xaab0e6, 0xaab100, 0xaab2e6, 0xaab4dc, 0xaab500, 0xaab7e6, 0xaab900, 0xaabee6, 0xaac000,
    0xaac1e6, 0xaac200, 0xaaf609, 0xaaf700, 0xabed09, 0xabee00, 0x10a0ddc, 0x10a0e00, 0x10a0fe6, 0x10a1000, 0x10a38e6, 0x10a3901,
    0x10a3adc, 0x10a3b00, 0x10a3f09, 0x10a4000, 0x10ae5e6, 0x10ae6dc, 0x10ae700, 0x10d24e6, 0x10d2800, 0x10d69e6, 0x10d6e00, 0x10eabe6,
    0x10ead00, 0x10f46dc, 0x10f48e6, 0x10f4bdc, 0x10f4ce6, 0x10f4ddc, 0x10f5100, 0x10f82e6, 0x10f83dc, 0x10f84e6, 0x10f85dc, 0x10f8600,
    0x1104609, 0x1104700, 0x1107009, 0x1107100, 0x1107f09, 0x1108000, 0x110b909, 0x110ba07, 0x110bb00, 0x11100e6, 0x1110300, 0x1113309,
    0x1113500, 0x1117307, 0x1117400, 0x111c009, 0x111c100, 0x111ca07, 0x111cb00, 0x1123509, 0x1123607, 0x1123700, 0x112e907, 0x112ea09,
    0x112eb00, 0x1133c07, 0x1133d00, 0x1134d09, 0x1134e00, 0x11366e6, 0x1136d00, 0x11370e6, 0x1137500, 0x113ce09, 0x113d100, 0x1144209,
    0x1144300, 0x1144607, 0x1144700, 0x1145ee6, 0x1145f00, 0x114c209, 0x114c307, 0x114c400, 0x115bf09, 0x115c007, 0x115c100, 0x1163f09,
    0x1164000, 0x116b609, 0x116b707, 0x116b800, 0x1172b09, 0x1172c00, 0x1183909, 0x1183a07, 0x1183b00, 0x1193d09, 0x1193f00, 0x1194307,
    0x1194400, 0x119e009, 0x119e100, 0x11a3409, 0x11a3500, 0x11a4709, 0x11a4800, 0x11a9909, 0x11a9a00, 0x11c3f09, 0x11c4000, 0x11d4207,
    0x11d4300, 0x11d4409, 0x11d4600, 0x11d9709, 0x11d9800, 0x11f4109, 0x11f4300, 0x1612f09, 0x1613000, 0x16b30e6, 0x16b3700, 0x1bc9e01,
    0x1bc9f00, 0x1e130e6, 0x1e13700, 0x1e2aee6, 0x1e2af00, 0x1e2ece6, 0x1e2f000, 0x1e4ece8, 0x1e4eedc, 0x1e4efe6, 0x1e4f000, 0x1e5eee6,
    0x1e5efdc, 0x1e5f000, 0x1e6e3e6, 0x1e6e400, 0x1e6e6e6, 0x1e6e700, 0x1e6eee6, 0x1e6f000, 0x1e6f5e6, 0x1e6f600, 0x1e944e6, 0x1e94a07,
    0x1e94b00,
];
/// Mark kinds of these scripts' characters, in ranges: each range's first character (times 4)
/// with its kind: none (0), nonspacing (1), spacing (2).
#[rustfmt::skip]
const MARKS: [u32; 756] = [
    0xc01, 0xdc0, 0x1fad, 0x1fd0, 0x1ff5, 0x1ff8, 0x2165, 0x2170, 0x2401, 0x240e, 0x2410, 0x24e9,
    0x24ee, 0x24f1, 0x24f4, 0x24fa, 0x2505, 0x2526, 0x2535, 0x253a, 0x2540, 0x2545, 0x2560, 0x2589,
    0x2590, 0x2605, 0x260a, 0x2610, 0x26f1, 0x26f4, 0x26fa, 0x2705, 0x2714, 0x271e, 0x2724, 0x272e,
    0x2735, 0x2738, 0x275e, 0x2760, 0x2789, 0x2790, 0x27f9, 0x27fc, 0x2805, 0x280e, 0x2810, 0x28f1,
    0x28f4, 0x28fa, 0x2905, 0x290c, 0x291d, 0x2924, 0x292d, 0x2938, 0x2945, 0x2948, 0x29c1, 0x29c8,
    0x29d5, 0x29d8, 0x2a05, 0x2a0e, 0x2a10, 0x2af1, 0x2af4, 0x2afa, 0x2b05, 0x2b18, 0x2b1d, 0x2b26,
    0x2b28, 0x2b2e, 0x2b35, 0x2b38, 0x2b89, 0x2b90, 0x2be9, 0x2c00, 0x2c05, 0x2c0a, 0x2c10, 0x2cf1,
    0x2cf4, 0x2cfa, 0x2cfd, 0x2d02, 0x2d05, 0x2d14, 0x2d1e, 0x2d24, 0x2d2e, 0x2d35, 0x2d38, 0x2d55,
    0x2d5e, 0x2d60, 0x2d89, 0x2d90, 0x2e09, 0x2e0c, 0x2efa, 0x2f01, 0x2f06, 0x2f0c, 0x2f1a, 0x2f24,
    0x2f2a, 0x2f35, 0x2f38, 0x2f5e, 0x2f60, 0x3001, 0x3006, 0x3011, 0x3014, 0x30f1, 0x30f4, 0x30f9,
    0x3106, 0x3114, 0x3119, 0x3124, 0x3129, 0x3138, 0x3155, 0x315c, 0x3189, 0x3190, 0x3205, 0x320a,
    0x3210, 0x32f1, 0x32f4, 0x32fa, 0x32fd, 0x3302, 0x3314, 0x3319, 0x331e, 0x3324, 0x332a, 0x3331,
    0x3338, 0x3356, 0x335c, 0x3389, 0x3390, 0x33ce, 0x33d0, 0x3401, 0x340a, 0x3410, 0x34ed, 0x34f4,
    0x34fa, 0x3505, 0x3514, 0x351a, 0x3524, 0x352a, 0x3535, 0x3538, 0x355e, 0x3560, 0x3589, 0x3590,
    0x3605, 0x360a, 0x3610, 0x3729, 0x372c, 0x373e, 0x3749, 0x3754, 0x3759, 0x375c, 0x3762, 0x3780,
    0x37ca, 0x37d0, 0x38c5, 0x38c8, 0x38d1, 0x38ec, 0x391d, 0x393c, 0x3ac5, 0x3ac8, 0x3ad1, 0x3af4,
    0x3b21, 0x3b3c, 0x3c61, 0x3c68, 0x3cd5, 0x3cd8, 0x3cdd, 0x3ce0, 0x3ce5, 0x3ce8, 0x3cfa, 0x3d00,
    0x3dc5, 0x3dfe, 0x3e01, 0x3e14, 0x3e19, 0x3e20, 0x3e35, 0x3e60, 0x3e65, 0x3ef4, 0x3f19, 0x3f1c,
    0x40ae, 0x40b5, 0x40c6, 0x40c9, 0x40e2, 0x40e5, 0x40ee, 0x40f5, 0x40fc, 0x415a, 0x4161, 0x4168,
    0x4179, 0x4184, 0x418a, 0x4194, 0x419e, 0x41b8, 0x41c5, 0x41d4, 0x4209, 0x420e, 0x4215, 0x421e,
    0x4235, 0x4238, 0x423e, 0x4240, 0x426a, 0x4275, 0x4278, 0x5c49, 0x5c56, 0x5c58, 0x5cc9, 0x5cd2,
    0x5cd4, 0x5d49, 0x5d50, 0x5dc9, 0x5dd0, 0x5ed1, 0x5eda, 0x5edd, 0x5efa, 0x5f19, 0x5f1e, 0x5f25,
    0x5f50, 0x5f75, 0x5f78, 0x602d, 0x6038, 0x603d, 0x6040, 0x6215, 0x621c, 0x62a5, 0x62a8, 0x6481,
    0x648e, 0x649d, 0x64a6, 0x64b0, 0x64c2, 0x64c9, 0x64ce, 0x64e5, 0x64f0, 0x685d, 0x6866, 0x686d,
    0x6870, 0x6956, 0x6959, 0x695e, 0x6961, 0x697c, 0x6981, 0x6986, 0x6989, 0x698e, 0x6995, 0x69b6,
    0x69cd, 0x69f4, 0x69fd, 0x6a00, 0x6c01, 0x6c12, 0x6c14, 0x6cd1, 0x6cd6, 0x6cd9, 0x6cee, 0x6cf1,
    0x6cf6, 0x6d09, 0x6d0e, 0x6d14, 0x6dad, 0x6dd0, 0x6e01, 0x6e0a, 0x6e0c, 0x6e86, 0x6e89, 0x6e9a,
    0x6ea1, 0x6eaa, 0x6ead, 0x6eb8, 0x6f99, 0x6f9e, 0x6fa1, 0x6faa, 0x6fb5, 0x6fba, 0x6fbd, 0x6fca,
    0x6fd0, 0x7092, 0x70b1, 0x70d2, 0x70d9, 0x70e0, 0x7341, 0x734c, 0x7351, 0x7386, 0x7389, 0x73a4,
    0x73b5, 0x73b8, 0x73d1, 0x73d4, 0x73de, 0x73e1, 0x73e8, 0x7701, 0x7800, 0x8341, 0x8376, 0x8385,
    0x838a, 0x8395, 0x83c4, 0xb5fd, 0xb600, 0x2a009, 0x2a00c, 0x2a019, 0x2a01c, 0x2a02d, 0x2a030, 0x2a08e,
    0x2a095, 0x2a09e, 0x2a0a0, 0x2a0b1, 0x2a0b4, 0x2a202, 0x2a208, 0x2a2d2, 0x2a311, 0x2a318, 0x2a381, 0x2a3c8,
    0x2a3fd, 0x2a400, 0x2a499, 0x2a4b8, 0x2a51d, 0x2a54a, 0x2a550, 0x2a601, 0x2a60e, 0x2a610, 0x2a6cd, 0x2a6d2,
    0x2a6d9, 0x2a6ea, 0x2a6f1, 0x2a6fa, 0x2a704, 0x2a795, 0x2a798, 0x2a8a5, 0x2a8be, 0x2a8c5, 0x2a8ce, 0x2a8d5,
    0x2a8dc, 0x2a90d, 0x2a910, 0x2a931, 0x2a936, 0x2a938, 0x2a9ee, 0x2a9f1, 0x2a9f6, 0x2a9f8, 0x2aac1, 0x2aac4,
    0x2aac9, 0x2aad4, 0x2aadd, 0x2aae4, 0x2aaf9, 0x2ab00, 0x2ab05, 0x2ab08, 0x2abae, 0x2abb1, 0x2abba, 0x2abc0,
    0x2abd6, 0x2abd9, 0x2abdc, 0x2af8e, 0x2af95, 0x2af9a, 0x2afa1, 0x2afa6, 0x2afac, 0x2afb2, 0x2afb5, 0x2afb8,
    0x42805, 0x42810, 0x42815, 0x4281c, 0x42831, 0x42840, 0x428e1, 0x428ec, 0x428fd, 0x42900, 0x42b95, 0x42b9c,
    0x43491, 0x434a0, 0x435a5, 0x435b8, 0x43aad, 0x43ab4, 0x43d19, 0x43d44, 0x43e09, 0x43e18, 0x44002, 0x44005,
    0x4400a, 0x4400c, 0x440e1, 0x4411c, 0x441c1, 0x441c4, 0x441cd, 0x441d4, 0x441fd, 0x4420a, 0x4420c, 0x442c2,
    0x442cd, 0x442de, 0x442e5, 0x442ec, 0x44309, 0x4430c, 0x44401, 0x4440c, 0x4449d, 0x444b2, 0x444b5, 0x444d4,
    0x44516, 0x4451c, 0x445cd, 0x445d0, 0x44601, 0x4460a, 0x4460c, 0x446ce, 0x446d9, 0x446fe, 0x44704, 0x44725,
    0x44734, 0x4473a, 0x4473d, 0x44740, 0x448b2, 0x448bd, 0x448ca, 0x448d1, 0x448d6, 0x448d9, 0x448e0, 0x448f9,
    0x448fc, 0x44905, 0x44908, 0x44b7d, 0x44b82, 0x44b8d, 0x44bac, 0x44c01, 0x44c0a, 0x44c10, 0x44cf1, 0x44cf4,
    0x44cfa, 0x44d01, 0x44d06, 0x44d14, 0x44d1e, 0x44d24, 0x44d2e, 0x44d38, 0x44d5e, 0x44d60, 0x44d8a, 0x44d90,
    0x44d99, 0x44db4, 0x44dc1, 0x44dd4, 0x44ee2, 0x44eed, 0x44f04, 0x44f0a, 0x44f0c, 0x44f16, 0x44f18, 0x44f1e,
    0x44f2c, 0x44f32, 0x44f39, 0x44f3e, 0x44f41, 0x44f44, 0x44f49, 0x44f4c, 0x44f85, 0x44f8c, 0x450d6, 0x450e1,
    0x45102, 0x45109, 0x45116, 0x45119, 0x4511c, 0x45179, 0x4517c, 0x452c2, 0x452cd, 0x452e6, 0x452e9, 0x452ee,
    0x452fd, 0x45306, 0x45309, 0x45310, 0x456be, 0x456c9, 0x456d8, 0x456e2, 0x456f1, 0x456fa, 0x456fd, 0x45704,
    0x45771, 0x45778, 0x458c2, 0x458cd, 0x458ee, 0x458f5, 0x458fa, 0x458fd, 0x45904, 0x45aad, 0x45ab2, 0x45ab5,
    0x45aba, 0x45ac1, 0x45ada, 0x45add, 0x45ae0, 0x45c75, 0x45c7a, 0x45c7d, 0x45c82, 0x45c89, 0x45c9a, 0x45c9d,
    0x45cb0, 0x460b2, 0x460bd, 0x460e2, 0x460e5, 0x460ec, 0x464c2, 0x464d8, 0x464de, 0x464e4, 0x464ed, 0x464f6,
    0x464f9, 0x464fc, 0x46502, 0x46504, 0x4650a, 0x4650d, 0x46510, 0x46746, 0x46751, 0x46760, 0x46769, 0x46772,
    0x46781, 0x46784, 0x46792, 0x46794, 0x46805, 0x4682c, 0x468cd, 0x468e6, 0x468e8, 0x468ed, 0x468fc, 0x4691d,
    0x46920, 0x46945, 0x4695e, 0x46965, 0x46970, 0x46a29, 0x46a5e, 0x46a61, 0x46a68, 0x46d81, 0x46d86, 0x46d89,
    0x46d96, 0x46d99, 0x46d9e, 0x46da0, 0x470be, 0x470c1, 0x470dc, 0x470e1, 0x470fa, 0x470fd, 0x47100, 0x47249,
    0x472a0, 0x472a6, 0x472a9, 0x472c6, 0x472c9, 0x472d2, 0x472d5, 0x472dc, 0x474c5, 0x474dc, 0x474e9, 0x474ec,
    0x474f1, 0x474f8, 0x474fd, 0x47518, 0x4751d, 0x47520, 0x4762a, 0x4763c, 0x47641, 0x47648, 0x4764e, 0x47655,
    0x4765a, 0x4765d, 0x47660, 0x47bcd, 0x47bd6, 0x47bdc, 0x47c01, 0x47c08, 0x47c0e, 0x47c10, 0x47cd2, 0x47cd9,
    0x47cec, 0x47cfa, 0x47d01, 0x47d06, 0x47d09, 0x47d0c, 0x47d69, 0x47d6c, 0x4d101, 0x4d104, 0x4d11d, 0x4d158,
    0x58479, 0x584aa, 0x584b5, 0x584c0, 0x5acc1, 0x5acdc, 0x5bd3d, 0x5bd40, 0x5bd46, 0x5be20, 0x5be3d, 0x5be4c,
    0x5bf91, 0x5bf94, 0x6f275, 0x6f27c, 0x784c1, 0x784dc, 0x78ab9, 0x78abc, 0x78bb1, 0x78bc0, 0x793b1, 0x793c0,
    0x797b9, 0x797c0, 0x79b8d, 0x79b90, 0x79b99, 0x79b9c, 0x79bb9, 0x79bc0, 0x79bd5, 0x79bd8, 0x7a511, 0x7a52c,
];
/// These scripts' characters with a canonical decomposition into two: each with the two, and
/// whether it recomposes (exclusions don't). Sorted.
#[rustfmt::skip]
const MADE_OF: [(u32, u32, u32, bool); 107] = [
    (0x344, 0x308, 0x301, false), (0x929, 0x928, 0x93c, true), (0x931, 0x930, 0x93c, true), (0x934, 0x933, 0x93c, true),
    (0x958, 0x915, 0x93c, false), (0x959, 0x916, 0x93c, false), (0x95a, 0x917, 0x93c, false), (0x95b, 0x91c, 0x93c, false),
    (0x95c, 0x921, 0x93c, false), (0x95d, 0x922, 0x93c, false), (0x95e, 0x92b, 0x93c, false), (0x95f, 0x92f, 0x93c, false),
    (0x9cb, 0x9c7, 0x9be, true), (0x9cc, 0x9c7, 0x9d7, true), (0x9dc, 0x9a1, 0x9bc, false), (0x9dd, 0x9a2, 0x9bc, false),
    (0x9df, 0x9af, 0x9bc, false), (0xa33, 0xa32, 0xa3c, false), (0xa36, 0xa38, 0xa3c, false), (0xa59, 0xa16, 0xa3c, false),
    (0xa5a, 0xa17, 0xa3c, false), (0xa5b, 0xa1c, 0xa3c, false), (0xa5e, 0xa2b, 0xa3c, false), (0xb48, 0xb47, 0xb56, true),
    (0xb4b, 0xb47, 0xb3e, true), (0xb4c, 0xb47, 0xb57, true), (0xb5c, 0xb21, 0xb3c, false), (0xb5d, 0xb22, 0xb3c, false),
    (0xb94, 0xb92, 0xbd7, true), (0xbca, 0xbc6, 0xbbe, true), (0xbcb, 0xbc7, 0xbbe, true), (0xbcc, 0xbc6, 0xbd7, true),
    (0xc48, 0xc46, 0xc56, true), (0xcc0, 0xcbf, 0xcd5, true), (0xcc7, 0xcc6, 0xcd5, true), (0xcc8, 0xcc6, 0xcd6, true),
    (0xcca, 0xcc6, 0xcc2, true), (0xccb, 0xcca, 0xcd5, true), (0xd4a, 0xd46, 0xd3e, true), (0xd4b, 0xd47, 0xd3e, true),
    (0xd4c, 0xd46, 0xd57, true), (0xdda, 0xdd9, 0xdca, true), (0xddc, 0xdd9, 0xdcf, true), (0xddd, 0xddc, 0xdca, true),
    (0xdde, 0xdd9, 0xddf, true), (0xf43, 0xf42, 0xfb7, false), (0xf4d, 0xf4c, 0xfb7, false), (0xf52, 0xf51, 0xfb7, false),
    (0xf57, 0xf56, 0xfb7, false), (0xf5c, 0xf5b, 0xfb7, false), (0xf69, 0xf40, 0xfb5, false), (0xf73, 0xf71, 0xf72, false),
    (0xf75, 0xf71, 0xf74, false), (0xf76, 0xfb2, 0xf80, false), (0xf78, 0xfb3, 0xf80, false), (0xf81, 0xf71, 0xf80, false),
    (0xf93, 0xf92, 0xfb7, false), (0xf9d, 0xf9c, 0xfb7, false), (0xfa2, 0xfa1, 0xfb7, false), (0xfa7, 0xfa6, 0xfb7, false),
    (0xfac, 0xfab, 0xfb7, false), (0xfb9, 0xf90, 0xfb5, false), (0x1026, 0x1025, 0x102e, true), (0x1b06, 0x1b05, 0x1b35, true),
    (0x1b08, 0x1b07, 0x1b35, true), (0x1b0a, 0x1b09, 0x1b35, true), (0x1b0c, 0x1b0b, 0x1b35, true), (0x1b0e, 0x1b0d, 0x1b35, true),
    (0x1b12, 0x1b11, 0x1b35, true), (0x1b3b, 0x1b3a, 0x1b35, true), (0x1b3d, 0x1b3c, 0x1b35, true), (0x1b40, 0x1b3e, 0x1b35, true),
    (0x1b41, 0x1b3f, 0x1b35, true), (0x1b43, 0x1b42, 0x1b35, true), (0x105c9, 0x105d2, 0x307, true), (0x105e4, 0x105da, 0x307, true),
    (0x1109a, 0x11099, 0x110ba, true), (0x1109c, 0x1109b, 0x110ba, true), (0x110ab, 0x110a5, 0x110ba, true), (0x1112e, 0x11131, 0x11127, true),
    (0x1112f, 0x11132, 0x11127, true), (0x1134b, 0x11347, 0x1133e, true), (0x1134c, 0x11347, 0x11357, true), (0x11383, 0x11382, 0x113c9, true),
    (0x11385, 0x11384, 0x113bb, true), (0x1138e, 0x1138b, 0x113c2, true), (0x11391, 0x11390, 0x113c9, true), (0x113c5, 0x113c2, 0x113c2, true),
    (0x113c7, 0x113c2, 0x113b8, true), (0x113c8, 0x113c2, 0x113c9, true), (0x114bb, 0x114b9, 0x114ba, true), (0x114bc, 0x114b9, 0x114b0, true),
    (0x114be, 0x114b9, 0x114bd, true), (0x115ba, 0x115b8, 0x115af, true), (0x115bb, 0x115b9, 0x115af, true), (0x11938, 0x11935, 0x11930, true),
    (0x16121, 0x1611e, 0x1611e, true), (0x16122, 0x1611e, 0x16129, true), (0x16123, 0x1611e, 0x1611f, true), (0x16124, 0x16129, 0x1611f, true),
    (0x16125, 0x1611e, 0x16120, true), (0x16126, 0x16121, 0x1611f, true), (0x16127, 0x16122, 0x1611f, true), (0x16128, 0x16121, 0x16120, true),
    (0x16d68, 0x16d67, 0x16d67, true), (0x16d69, 0x16d63, 0x16d67, true), (0x16d6a, 0x16d69, 0x16d67, true),
];

#[cfg(test)]
#[cfg(feature = "brahmic")]
mod tests {
    use super::*;
    use crate::scripts::Language;
    use crate::shaping::Script;
    use crate::shaping::harfbuzz::Shaped;

    /// A character's script, as an index in `WAYS`; `None` if not one of these.
    fn script(letter: char) -> Option<u8> {
        crate::scripts::of(letter).checked_sub(1).filter(|n| (*n as usize) < crate::scripts::BRAHMIC)
    }

    /// A font by file and index in it.
    type Named = (String, u32);

    /// What this makes of a word: shaped per its script, left to right.
    fn ours(font: &FontFile, plans: &mut Vec<(u8, Plan)>, word: &str) -> Shaped {
        let n = word.chars().find_map(script).unwrap_or(0);
        if !plans.iter().any(|(known, _)| *known == n) {
            plans.push((n, Plan::of(font, Script::Brahmic(n), false, Language::NONE)));
        }
        let plan = &plans.iter().find(|(known, _)| *known == n).expect("a plan").1;
        let mut glyphs = Vec::new();
        let total = font.shape(plan, word, &mut glyphs);
        let mut pen = 0;
        let placed = glyphs.iter().map(|laid| (laid.glyph, [(pen, pen += laid.advance).0 + laid.offset[0], laid.offset[1]])).collect();
        (placed, total)
    }

    /// The character index each of a word's glyphs stands for.
    fn froms(font: &FontFile, word: &str) -> Vec<u32> {
        let n = word.chars().find_map(script).expect("a script");
        let mut glyphs = Vec::new();
        font.shape(&Plan::of(font, Script::Brahmic(n), false, Language::NONE), word, &mut glyphs);
        glyphs.iter().map(|laid| laid.from).collect()
    }

    /// A font file, or the `face`th font of a collection.
    fn opened(path: &str, face: u32) -> Option<FontFile> {
        let font = FontFile::open(path)?;
        if face == 0 { Some(font) } else { font.nth(face as usize) }
    }

    /// Compares words with HarfBuzz's shaping of each in one font: returns the count per script
    /// and how many matched, and the words that differ.
    fn held(font: &FontFile, words: &[(String, Shaped)], tally: &mut Vec<(String, usize, usize)>) -> Vec<String> {
        let (mut plans, mut differ) = (Vec::new(), Vec::new());
        // Text without these scripts' letters isn't shaped here: a sign Vedic and Devanagari
        // share, alone, is script-less.
        for (word, theirs) in words.iter().filter(|(word, _)| word.chars().any(|letter| script(letter).is_some())) {
            let mine = ours(font, &mut plans, word);
            let tag = word.chars().find_map(script).map_or("none".to_string(), |n| String::from_utf8_lossy(crate::scripts::tag(n + 1)).into_owned());
            let same = mine == *theirs;
            match tally.iter_mut().find(|(known, ..)| *known == tag) {
                Some(entry) => (entry.1, entry.2) = (entry.1 + 1, entry.2 + same as usize),
                None => tally.push((tag, 1, same as usize)),
            }
            differ.extend((!same).then(|| format!("{word} ({}): {mine:?}, not {theirs:?}", word.escape_unicode())));
        }
        differ
    }

    /// Reads words from files (each space-separated piece of a line; lines starting with `#`
    /// are comments).
    #[cfg(target_os = "linux")]
    fn words_of(files: &str) -> Vec<String> {
        let all = files.split(if cfg!(windows) { ';' } else { ':' }).filter(|path| !path.is_empty());
        let mut words: Vec<String> = all
            .flat_map(|path| {
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| !line.starts_with('#'))
                    .flat_map(|line| line.split_whitespace().map(String::from).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            })
            .collect();
        words.sort();
        words.dedup();
        words
    }

    /// Prints the results, and fails if any word differs.
    fn report(all_differ: Vec<String>, tally: &[(String, usize, usize)]) {
        for (tag, count, same) in tally {
            println!("{tag}: {count} words, {same} the same");
        }
        assert!(all_differ.is_empty(), "{} words differ", all_differ.len());
    }

    #[test]
    fn words_come_to_the_glyphs_harfbuzz_makes_of_them() {
        // HarfBuzz's shaping of the words (or HarfRust's, its Rust port, where HarfBuzz is
        // missing), read from `TAUT_EXPECTED`: a line per word per font.
        let Some(path) = std::env::var_os("TAUT_EXPECTED") else { return eprintln!("no TAUT_EXPECTED: not tested") };
        let text = std::fs::read_to_string(path).expect("the file of what HarfBuzz made");
        let mut fonts: Vec<(Named, Vec<(String, Shaped)>)> = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let glyphs = fields[3].split(',').filter(|glyph| !glyph.is_empty()).map(|glyph| glyph.split(':').map(|n| n.parse::<i32>().expect("a number")).collect::<Vec<_>>());
            let shaped: Shaped = (glyphs.map(|n| (n[0] as u16, [n[1], n[2]])).collect(), fields[4].parse().expect("a number"));
            let key = (fields[0].to_string(), fields[1].parse().expect("a number"));
            match fonts.iter_mut().find(|(known, _)| *known == key) {
                Some((_, words)) => words.push((fields[2].to_string(), shaped)),
                None => fonts.push((key, vec![(fields[2].to_string(), shaped)])),
            }
        }
        let (mut all_differ, mut tally) = (Vec::new(), Vec::new());
        for ((path, face), words) in fonts {
            let font = opened(&path, face).expect("the font is read");
            let differ = held(&font, &words, &mut tally);
            println!("{path}#{face}: {} words, {} differ", words.len(), differ.len());
            differ.iter().take(8).for_each(|line| println!("  {line}"));
            all_differ.extend(differ);
        }
        report(all_differ, &tally);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn words_come_to_the_glyphs_the_systems_harfbuzz_makes_of_them() {
        // The words of the files `TAUT_SCRIPT_WORDS` names, in the fonts `TAUT_SCRIPT_FONTS`
        // names (paths separated by `:`, each with `#` and its index in a collection): each
        // word the font has all glyphs for, per HarfBuzz.
        let words = words_of(&std::env::var("TAUT_SCRIPT_WORDS").unwrap_or_default());
        let fonts = std::env::var("TAUT_SCRIPT_FONTS").unwrap_or_default();
        if words.is_empty() || fonts.is_empty() {
            return eprintln!("no TAUT_SCRIPT_WORDS or TAUT_SCRIPT_FONTS: not tested");
        }
        let (mut all_differ, mut tally) = (Vec::new(), Vec::new());
        for named in fonts.split(':').filter(|named| !named.is_empty()) {
            let (path, face) = named.split_once('#').map_or((named, 0), |(path, face)| (path, face.parse().expect("a number")));
            let Some(font) = opened(path, face) else { continue };
            let Some(theirs) = crate::shaping::harfbuzz::harfbuzz(std::path::Path::new(path), face, &words) else { return eprintln!("no HarfBuzz on this system: not tested") };
            let kept: Vec<(String, Shaped)> = words.iter().cloned().zip(theirs).filter(|(_, (glyphs, _))| glyphs.iter().all(|(glyph, _)| *glyph != 0)).collect();
            let differ = held(&font, &kept, &mut tally);
            println!("{named}: {} words, {} differ", kept.len(), differ.len());
            differ.iter().take(8).for_each(|line| println!("  {line}"));
            all_differ.extend(differ);
        }
        report(all_differ, &tally);
    }

    /// The system fonts for these scripts a test can rely on: Windows' own.
    fn systems() -> Vec<(char, FontFile)> {
        let letters = ['\u{915}', '\u{995}', '\u{b95}', '\u{d9a}', '\u{e01}', '\u{1780}', '\u{1000}'];
        letters.into_iter().filter_map(|letter| crate::bidi::system(crate::text::Face::Regular, letter).map(|font| (letter, font))).collect()
    }

    #[test]
    fn syllables_are_put_in_the_order_their_glyphs_are_drawn_in() {
        // By the characters each glyph stands for: a pre-base matra comes first; a reph last; a
        // two-part vowel is decomposed, the pre-base part first; Myanmar medial ra goes before
        // its consonant; Thai sara am is a nikhahit, put before the tone mark on the same
        // letter, plus sara aa.
        let cases: [(&str, &[u32]); 9] = [
            ("\u{915}\u{93f}", &[1, 0]),
            ("\u{930}\u{94d}\u{915}", &[2, 0]),
            ("\u{995}\u{9cb}", &[1, 0, 1]),
            ("\u{b95}\u{bca}", &[1, 0, 1]),
            ("\u{d9a}\u{ddc}", &[1, 0, 1]),
            ("\u{e19}\u{e49}\u{e33}", &[0, 2, 1, 2]),
            ("\u{1780}\u{17be}", &[1, 0, 1]),
            ("\u{1000}\u{103c}", &[1, 0]),
            ("\u{915}\u{94d}\u{937}\u{93f}", &[3, 0]),
        ];
        let fonts = systems();
        if fonts.is_empty() {
            return eprintln!("no system font for these scripts: not tested");
        }
        for (word, order) in cases {
            let first = word.chars().next().expect("a letter");
            let Some((_, font)) = fonts.iter().find(|(letter, _)| script(*letter) == script(first)) else { continue };
            let found = froms(font, word);
            assert!(found.starts_with(order), "{}: {found:?}", word.escape_unicode());
        }
    }

    #[test]
    fn any_run_of_characters_is_shaped_without_failing() {
        // Runs of one to eight characters from each script's block, with joiners, a virama, a
        // no-break space and the dotted circle among them, chosen by a fixed pseudo-random
        // sequence: in any order, shaping panics nowhere and yields glyphs.
        let mut seed = 0x2545_f491_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let extra = ['\u{200c}', '\u{200d}', '\u{a0}', '\u{25cc}', '\u{94d}'];
        for (letter, font) in systems() {
            let block = (letter as u32) & !0x7f;
            let mut plans = Vec::new();
            for _ in 0..1500 {
                let long = 1 + next() as usize % 8;
                let word: String = (0..long).map(|_| if next() % 5 == 0 { extra[next() as usize % extra.len()] } else { char::from_u32(block + next() % 0x80).unwrap_or(letter) }).collect();
                if !word.chars().any(|letter| script(letter).is_some()) {
                    continue;
                }
                let (glyphs, _) = ours(&font, &mut plans, &word);
                assert!(!glyphs.is_empty() || word.chars().all(|letter| unicode(letter) != 0), "{}", word.escape_unicode());
            }
        }
    }

    #[test]
    fn a_sign_by_itself_is_shown_on_a_dotted_circle() {
        for (_, font) in systems() {
            let circle = font.glyph('\u{25cc}');
            if circle == 0 {
                continue;
            }
            let mut plans = Vec::new();
            for word in ["\u{93f}", "\u{94d}", "\u{9cb}", "\u{17d2}\u{1780}", "\u{102d}"] {
                if font.glyph(word.chars().next().expect("a sign")) == 0 {
                    continue;
                }
                let (glyphs, _) = ours(&font, &mut plans, word);
                assert!(glyphs.iter().any(|(glyph, _)| *glyph == circle), "{}: {glyphs:?}", word.escape_unicode());
            }
        }
    }

    #[test]
    fn grammars_find_the_syllables_harfbuzz_finds() {
        // Devanagari categories: ra, virama, consonant, matra (a reph on a syllable with a
        // matra); a consonant and an orphan sign; a lone matra.
        let machine = Machine::of(INDIC_GRAMMAR, NONE[0]);
        let cut = |categories: &[u8]| {
            let mut syllables = vec![0; categories.len()];
            machine.cut(categories, &vec![true; categories.len()], &mut syllables, BROKEN[0]);
            syllables
        };
        assert_eq!(cut(&[RA, VIRAMA, CONSONANT, MATRA, CONSONANT]), [0x10, 0x10, 0x10, 0x10, 0x20]);
        assert_eq!(cut(&[MATRA]), [0x14]);
        assert_eq!(cut(&[CONSONANT, VIRAMA, JOINER, CONSONANT, NUKTA, MATRA, MODIFIER]), [0x10; 7]);
        assert_eq!(cut(&[0, CONSONANT, 0]), [0x15, 0x20, 0x35]);
        assert_eq!(postfix("a ( b | c ) * d ?"), ["a", "b", "c", "|", "*", ".", "d", "?", "."]);
    }
}
