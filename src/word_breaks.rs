//! Where words end in Thai, Lao, Burmese and Khmer, which have no spaces between words: from
//! ICU's word lists, walked as ICU's dictionary break engines walk them (`dictbe.cpp`), so
//! lines break where browsers and the system's own programs break them. For systems that lend
//! no dictionary of their own (feature `word-lists`; Windows asks Uniscribe, Apple's systems
//! CoreFoundation): Linux, Android and the web.
//!
//! The lists are ICU 78's (26,383 Thai, 30,550 Lao, 41,120 Burmese, 81,028 Khmer words: 4.3 MB
//! as text), kept in `word_breaks.bin` as minimal automata, 804 KB in all.
//!
//! At each place the engine takes the longest word that a second and a third word follow, else
//! the one a second word follows, else the longest. Text no word starts is joined to the short
//! word before it, as far as a letter that may end a word comes before one that may start one
//! there. No word ends before a mark; and in Thai, paiyannoi (ฯ) and maiyamok (ๆ) end the word
//! before them.

/// The four lists: four little-endian u32 offsets, one per language (Thai, Lao, Burmese,
/// Khmer), of its part. A part: how many characters its words use, those characters (u16 each,
/// little-endian, ascending: a character's place there is its symbol), then its automaton.
///
/// The automaton is its states, the first one first, each before those it leads to. A state is
/// its edges, one after another. An edge is a label, then a code for its target. The label is
/// symbols, the last with its top bit set. The code's first byte has bit 7 set on a state's last
/// edge, bit 6 where a word ends after the label, and bits 5-4 its form: 0, no target (no word
/// goes on); 1, the target bits 3-0 bytes on; 2 and 3, the same with one or two bytes more
/// (high bits first). Distances count from the code's end.
const LISTS: &[u8] = include_bytes!("word_breaks.bin");

/// How many words in a row are good enough: three.
const LOOKAHEAD: usize = 3;
/// Text no word starts is not joined to a word this long or longer before it...
const ROOT_COMBINE: usize = 3;
/// ...nor where it starts this much of some word.
const PREFIX_COMBINE: usize = 3;
/// The fewest characters a stretch is broken in: two words of two.
const SHORTEST: usize = 4;
/// The most words that start at one place.
const MOST_WORDS: usize = 20;
/// Thai's paiyannoi and maiyamok.
const PAIYANNOI: char = '\u{e2f}';
const MAIYAMOK: char = '\u{e46}';

/// A language's way of finding words (ICU's `ThaiBreakEngine` and its siblings).
struct Engine {
    /// Its list's place in `LISTS`.
    list: usize,
    /// The characters it breaks (its script's letters of line break class SA), as ranges.
    letters: &'static [(u32, u32)],
    /// Those of them that are marks.
    marks: &'static [(u32, u32)],
    /// Those a word may start with, and those of `letters` it may not end with.
    starts: &'static [(u32, u32)],
    never_ends: &'static [(u32, u32)],
    /// Thai: paiyannoi and maiyamok end the word before them, and a stretch must be longer
    /// than `SHORTEST`.
    thai: bool,
}

/// The engines: Thai, Lao, Burmese, Khmer.
const ENGINES: [Engine; 4] = [
    Engine {
        list: 0,
        letters: &[(0xe01, 0xe3a), (0xe40, 0xe4e)],
        marks: &[(0xe31, 0xe31), (0xe34, 0xe3a), (0xe47, 0xe4e)],
        starts: &[(0xe01, 0xe2e), (0xe40, 0xe44)],
        never_ends: &[(0xe31, 0xe31), (0xe40, 0xe44)],
        thai: true,
    },
    Engine {
        list: 1,
        letters: &[(0xe81, 0xe82), (0xe84, 0xe84), (0xe86, 0xe8a), (0xe8c, 0xea3), (0xea5, 0xea5), (0xea7, 0xebd), (0xec0, 0xec4), (0xec6, 0xec6), (0xec8, 0xece), (0xedc, 0xedf)],
        marks: &[(0xeb1, 0xeb1), (0xeb4, 0xebc), (0xec8, 0xece)],
        starts: &[(0xe81, 0xeae), (0xec0, 0xec4), (0xedc, 0xedd)],
        never_ends: &[(0xec0, 0xec4)],
        thai: false,
    },
    Engine {
        list: 2,
        letters: &[(0x1000, 0x103f), (0x1050, 0x108f), (0x109a, 0x109f), (0xa9e0, 0xa9ef), (0xa9fa, 0xa9fe), (0xaa60, 0xaa7f)],
        marks: &[
            (0x102b, 0x103e),
            (0x1056, 0x1059),
            (0x105e, 0x1060),
            (0x1062, 0x1064),
            (0x1067, 0x106d),
            (0x1071, 0x1074),
            (0x1082, 0x108d),
            (0x108f, 0x108f),
            (0x109a, 0x109d),
            (0xa9e5, 0xa9e5),
            (0xaa7b, 0xaa7d),
        ],
        starts: &[(0x1000, 0x102a)],
        never_ends: &[],
        thai: false,
    },
    Engine {
        list: 3,
        letters: &[(0x1780, 0x17d3), (0x17d7, 0x17d7), (0x17dc, 0x17dd)],
        marks: &[(0x17b4, 0x17d3), (0x17dd, 0x17dd)],
        starts: &[(0x1780, 0x17b3)],
        never_ends: &[(0x17d2, 0x17d2)],
        thai: false,
    },
];

/// Whether a character lies in one of some ranges.
fn among(ranges: &[(u32, u32)], letter: char) -> bool {
    ranges.iter().any(|(first, last)| (*first..=*last).contains(&(letter as u32)))
}

/// Character indices where a line may break between words of Thai, Lao, Burmese or Khmer.
pub(crate) fn words(text: &str) -> Vec<usize> {
    let letters: Vec<char> = text.chars().collect();
    let mut breaks = Vec::new();
    let mut at = 0;
    while at < letters.len() {
        let Some(engine) = ENGINES.iter().find(|engine| among(engine.letters, letters[at])) else {
            at += 1;
            continue;
        };
        let end = at + letters[at..].iter().take_while(|letter| among(engine.letters, **letter)).count();
        Finder { engine, list: List::of(engine.list), letters: &letters, end, at }.find(&mut breaks);
        at = end;
    }
    breaks
}

/// One language's list in `LISTS`: its symbols, and its automaton.
#[derive(Clone, Copy)]
struct List {
    symbols: &'static [u8],
    automaton: &'static [u8],
}

impl List {
    /// The list of language `n`.
    fn of(n: usize) -> List {
        let at = LISTS.get(n * 4..n * 4 + 4).map_or(0, |offset| u32::from_le_bytes([offset[0], offset[1], offset[2], offset[3]]) as usize);
        let count = LISTS.get(at).copied().unwrap_or(0) as usize;
        let symbols = LISTS.get(at + 1..at + 1 + count * 2).unwrap_or(&[]);
        List { symbols, automaton: LISTS.get(at + 1 + count * 2..).unwrap_or(&[]) }
    }

    /// A character's symbol, if a word uses it.
    fn symbol(self, letter: char) -> Option<u8> {
        let (symbols, _) = self.symbols.as_chunks::<2>();
        let code = u16::try_from(letter as u32).ok()?;
        symbols.binary_search_by_key(&code, |pair| u16::from_le_bytes(*pair)).ok().map(|place| place as u8)
    }

    /// The edge of the state at `state` whose label starts with `symbol`.
    fn edge(self, state: usize, symbol: u8) -> Option<Edge> {
        let mut at = state;
        loop {
            let label = at;
            at += self.automaton.get(at..)?.iter().position(|byte| byte & 0x80 != 0)? + 1;
            let (code_at, code) = (at, *self.automaton.get(at)?);
            let form = (code >> 4 & 3) as usize;
            let more = self.automaton.get(at + 1..at + form.max(1))?;
            at += form.max(1);
            if self.automaton[label] & 0x7f == symbol {
                let distance = more.iter().fold((code & 15) as usize, |distance, byte| distance << 8 | *byte as usize);
                return Some(Edge { label: label + 1..code_at, ends: code & 0x40 != 0, target: (form != 0).then_some(at + distance) });
            }
            if code & 0x80 != 0 {
                return None;
            }
        }
    }

    /// The lengths of the words that `letters` start with, no longer than `most`, shortest
    /// first (`MOST_WORDS` at most); and how far they matched the start of any word, counting
    /// the first letter that matched none (as ICU counts).
    fn words(self, letters: &[char], most: usize) -> (Vec<usize>, usize) {
        let (mut found, mut matched) = (Vec::new(), 0);
        let mut walk = Walk { state: Some(0), label: 0..0, ends: false };
        for (n, letter) in letters.iter().enumerate() {
            matched += 1;
            if !walk.step(self, self.symbol(*letter)) {
                break;
            }
            let word = walk.label.is_empty() && walk.ends;
            if word && found.len() < MOST_WORDS {
                found.push(n + 1);
            }
            if (word && walk.state.is_none()) || n + 1 >= most {
                break;
            }
        }
        (found, matched)
    }
}

/// A walk through a list's automaton: the state it is at (none past a word no other goes on
/// from), the rest of the edge label it is on, and whether a word ends after that label.
struct Walk {
    state: Option<usize>,
    label: std::ops::Range<usize>,
    ends: bool,
}

impl Walk {
    /// Takes one more letter, by its symbol: whether some word goes on with it.
    fn step(&mut self, list: List, symbol: Option<u8>) -> bool {
        if !self.label.is_empty() {
            let next = symbol == Some(list.automaton[self.label.start] & 0x7f);
            self.label.start += 1;
            return next;
        }
        let Some(edge) = self.state.zip(symbol).and_then(|(state, symbol)| list.edge(state, symbol)) else { return false };
        (self.label, self.ends, self.state) = (edge.label, edge.ends, edge.target);
        true
    }
}

/// An edge: the rest of its label (after its first symbol), whether a word ends after it, and
/// its target state.
struct Edge {
    label: std::ops::Range<usize>,
    ends: bool,
    target: Option<usize>,
}

/// The words starting at one place, as an engine weighs them (ICU's `PossibleWord`).
#[derive(Default)]
struct Candidates {
    /// Where they start; their lengths, shortest first; how far they matched any word.
    start: Option<usize>,
    lengths: Vec<usize>,
    matched: usize,
    /// The one looked at, and the one liked best.
    current: usize,
    marked: usize,
}

/// Finding the words of one stretch of a language's letters.
struct Finder<'a> {
    engine: &'a Engine,
    list: List,
    letters: &'a [char],
    /// Where the stretch ends, and where the search stands.
    end: usize,
    at: usize,
}

impl Finder<'_> {
    /// Adds the word breaks of the stretch from `self.at` to `self.end` to `breaks`.
    fn find(&mut self, breaks: &mut Vec<usize>) {
        let (start, end) = (self.at, self.end);
        let too_short = if self.engine.thai { start + SHORTEST >= end } else { end - start < SHORTEST };
        if too_short {
            return;
        }
        let mut places: [Candidates; LOOKAHEAD] = Default::default();
        let mut found = 0;
        while self.at < end {
            let here = self.at;
            let mut long = self.best(&mut places, found);
            found += usize::from(long > 0);
            if self.at < end && long < ROOT_COMBINE {
                let joined = self.join_unknown(&mut places, found, here, long);
                // (Text no word starts counts as a word where it stands alone.)
                found += usize::from(long == 0 && joined > 0);
                long = joined;
            }
            // No word ends before a mark.
            while self.at < end && among(self.engine.marks, self.letters[self.at]) {
                (self.at, long) = (self.at + 1, long + 1);
            }
            if self.engine.thai && self.at < end && long > 0 {
                long = self.thai_suffix(&mut places[found % LOOKAHEAD], here, long);
            }
            if long > 0 {
                breaks.push(here + long);
            }
        }
        // (Not at the stretch's end.)
        if breaks.last().is_some_and(|last| *last >= end) {
            breaks.pop();
        }
    }

    /// The word taken at the place the search stands: its length (0 for none); the search
    /// moves past it.
    fn best(&mut self, places: &mut [Candidates; LOOKAHEAD], found: usize) -> usize {
        let count = self.candidates(&mut places[found % LOOKAHEAD]);
        if count == 0 {
            return 0;
        }
        // Of several, the longest a second and third word follow, else a second.
        if count > 1 && self.at < self.end {
            self.weigh(places, found);
        }
        let place = &places[found % LOOKAHEAD];
        let long = place.lengths[place.marked];
        self.at = place.start.unwrap_or(self.at) + long;
        long
    }

    /// Marks the word at the current place that the most words follow (see `best`).
    fn weigh(&mut self, places: &mut [Candidates; LOOKAHEAD], found: usize) {
        loop {
            let second = self.candidates(&mut places[(found + 1) % LOOKAHEAD]) > 0;
            if second {
                let first = &mut places[found % LOOKAHEAD];
                first.marked = first.current;
            }
            if second && (self.at >= self.end || self.third_follows(places, found)) {
                return;
            }
            if !self.back_up(&mut places[found % LOOKAHEAD]) {
                return;
            }
        }
    }

    /// Whether a third word follows one of the second words: then the first word is marked.
    fn third_follows(&mut self, places: &mut [Candidates; LOOKAHEAD], found: usize) -> bool {
        loop {
            if self.candidates(&mut places[(found + 2) % LOOKAHEAD]) > 0 {
                let first = &mut places[found % LOOKAHEAD];
                first.marked = first.current;
                return true;
            }
            if !self.back_up(&mut places[(found + 1) % LOOKAHEAD]) {
                return false;
            }
        }
    }

    /// Joins what follows a short word (or nothing) to it where no word starts there: up to a
    /// letter that may end a word before one that may start a word there. Returns the length
    /// taken from `here`.
    fn join_unknown(&mut self, places: &mut [Candidates; LOOKAHEAD], found: usize, here: usize, long: usize) -> usize {
        let next = self.candidates(&mut places[found % LOOKAHEAD]);
        let unknown = next == 0 && (long == 0 || places[found % LOOKAHEAD].matched < PREFIX_COMBINE);
        if !unknown {
            self.at = here + long;
            return long;
        }
        let mut passed = 0;
        loop {
            let before = self.letters[self.at];
            (self.at, passed) = (self.at + 1, passed + 1);
            if here + long + passed >= self.end {
                break;
            }
            let boundary = among(self.engine.letters, before) && !among(self.engine.never_ends, before) && among(self.engine.starts, self.letters[self.at]);
            if boundary && self.word_follows(&mut places[(found + 1) % LOOKAHEAD]) {
                break;
            }
        }
        long + passed
    }

    /// Whether a word starts where the search stands (which stays there).
    fn word_follows(&mut self, place: &mut Candidates) -> bool {
        let at = self.at;
        let words = self.candidates(place);
        self.at = at;
        words > 0
    }

    /// Thai: where no word follows, a paiyannoi, then a maiyamok, after the word belong to it
    /// (each but after another of them). Returns the word's length from `here`.
    fn thai_suffix(&mut self, place: &mut Candidates, here: usize, mut long: usize) -> usize {
        let at = |n: usize| self.letters.get(n).copied();
        if self.candidates(place) > 0 || !matches!(at(self.at), Some(PAIYANNOI | MAIYAMOK)) {
            self.at = here + long;
            return long;
        }
        if at(self.at) == Some(PAIYANNOI) && !matches!(at(self.at - 1), Some(PAIYANNOI | MAIYAMOK)) {
            (self.at, long) = (self.at + 1, long + 1);
        }
        if at(self.at) == Some(MAIYAMOK) && at(self.at - 1) != Some(MAIYAMOK) {
            (self.at, long) = (self.at + 1, long + 1);
        }
        long
    }

    /// Finds the words starting where the search stands (once per place), and moves the search
    /// past the longest: how many there are.
    fn candidates(&mut self, place: &mut Candidates) -> usize {
        if place.start != Some(self.at) {
            let (lengths, matched) = self.list.words(&self.letters[self.at..], self.end - self.at);
            (place.start, place.lengths, place.matched) = (Some(self.at), lengths, matched);
        }
        let count = place.lengths.len();
        if let Some(longest) = place.lengths.last() {
            self.at = place.start.unwrap_or(self.at) + longest;
        }
        (place.current, place.marked) = (count.saturating_sub(1), count.saturating_sub(1));
        count
    }

    /// Steps back to the next shorter word at a place, moving the search past it: whether
    /// there was one.
    fn back_up(&mut self, place: &mut Candidates) -> bool {
        if place.current == 0 {
            return false;
        }
        place.current -= 1;
        self.at = place.start.unwrap_or(self.at) + place.lengths[place.current];
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every word of a list, read out of its automaton.
    fn every_word(list: List) -> Vec<String> {
        let (symbols, _) = list.symbols.as_chunks::<2>();
        let letter = |symbol: u8| char::from_u32(u16::from_le_bytes(symbols[(symbol & 0x7f) as usize]) as u32).unwrap();
        let mut words = Vec::new();
        let mut stack = vec![(0usize, String::new())];
        while let Some((state, word)) = stack.pop() {
            let mut at = state;
            loop {
                let long = list.automaton[at..].iter().position(|byte| byte & 0x80 != 0).unwrap() + 1;
                let mut next = word.clone();
                next.extend(list.automaton[at..at + long].iter().map(|symbol| letter(*symbol)));
                at += long;
                let code = list.automaton[at];
                let form = (code >> 4 & 3) as usize;
                let distance = list.automaton[at + 1..at + form.max(1)].iter().fold((code & 15) as usize, |distance, byte| distance << 8 | *byte as usize);
                at += form.max(1);
                if code & 0x40 != 0 {
                    words.push(next.clone());
                }
                if form != 0 {
                    stack.push((at + distance, next));
                }
                if code & 0x80 != 0 {
                    break;
                }
            }
        }
        words
    }

    #[test]
    fn every_list_reads_back_whole() {
        // As many words as ICU's lists hold, each found as a word by the search.
        let counts = [26_383, 30_550, 41_120, 81_028];
        for (n, count) in counts.into_iter().enumerate() {
            let words = every_word(List::of(n));
            assert_eq!(words.len(), count, "list {n}");
            for word in words.iter().step_by(97) {
                let letters: Vec<char> = word.chars().collect();
                assert!(List::of(n).words(&letters, letters.len()).0.contains(&letters.len()), "{word}");
            }
        }
    }

    /// Where a text breaks, as words.
    fn split(text: &str) -> Vec<String> {
        let letters: Vec<char> = text.chars().collect();
        let mut starts = vec![0];
        starts.extend(words(text));
        starts.push(letters.len());
        starts.windows(2).map(|pair| letters[pair[0]..pair[1]].iter().collect()).collect()
    }

    #[test]
    fn thai_breaks_between_its_words() {
        // ภาษาไทยเป็นภาษาที่สวยงาม: "the Thai language is a beautiful language".
        assert_eq!(split("ภาษาไทยเป็นภาษาที่สวยงาม"), ["ภาษา", "ไทย", "เป็น", "ภาษา", "ที่", "สวยงาม"]);
        // A stretch of four letters is left whole in Thai; and nothing breaks outside the
        // scripts.
        assert_eq!(split("ไทยไ"), ["ไทยไ"]);
        assert_eq!(words("Thai ไทย text"), Vec::<usize>::new());
    }

    /// The system ICU's word boundaries in some texts (in a locale), where both sides are one
    /// engine's letters: character indices. Nothing without ICU.
    #[cfg(target_os = "linux")]
    fn icu(texts: &[String], locale: &std::ffi::CStr) -> Option<Vec<Vec<usize>>> {
        use std::ffi::{c_char, c_int, c_void};
        unsafe extern "C" {
            fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
            fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        // SAFETY: the library is opened by name, its functions found by theirs (with the
        // version suffix ICU gives them) and called as ICU declares them, on what they made.
        unsafe {
            let library = (60..100).rev().map(|version| format!("libicuuc.so.{version}\0")).map(|name| dlopen(name.as_ptr().cast(), 2)).find(|library| !library.is_null())?;
            let named = |name: &str| (60..100).rev().map(|version| dlsym(library, format!("{name}_{version}\0").as_ptr().cast())).find(|found| !found.is_null());
            let open: extern "C" fn(c_int, *const c_char, *const u16, i32, *mut c_int) -> *mut c_void = std::mem::transmute(named("ubrk_open")?);
            let next: extern "C" fn(*mut c_void) -> i32 = std::mem::transmute(named("ubrk_next")?);
            let close: extern "C" fn(*mut c_void) = std::mem::transmute(named("ubrk_close")?);
            let mut all = Vec::new();
            for text in texts {
                let units: Vec<u16> = text.encode_utf16().collect();
                let mut status = 0;
                let iterator = open(1, locale.as_ptr(), units.as_ptr(), units.len() as i32, &mut status);
                assert!(status <= 0 && !iterator.is_null(), "ICU opens no word iterator: {status}");
                let letters: Vec<char> = text.chars().collect();
                let inside = |at: usize| at > 0 && at < letters.len() && ENGINES.iter().any(|engine| among(engine.letters, letters[at - 1]) && among(engine.letters, letters[at]));
                let mut found = Vec::new();
                loop {
                    let unit = next(iterator);
                    if unit < 0 {
                        break;
                    }
                    let at = String::from_utf16_lossy(&units[..unit as usize]).chars().count();
                    found.extend(inside(at).then_some(at));
                }
                close(iterator);
                all.push(found);
            }
            Some(all)
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn words_break_where_icu_breaks_them() {
        // Per language: runs of two to seven words of its own list, picked at random (the same
        // each time); and with `TAUT_WORD_TEXTS` naming files (`:`-separated), their lines.
        let locales = [c"th", c"lo", c"my", c"km"];
        let more: Vec<String> = std::env::var("TAUT_WORD_TEXTS")
            .unwrap_or_default()
            .split(':')
            .filter(|path| !path.is_empty())
            .flat_map(|path| std::fs::read_to_string(path).expect("read").lines().map(String::from).collect::<Vec<_>>())
            .collect();
        let mut seed = 1u32;
        let mut random = |below: usize| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 8) as usize % below
        };
        let (mut held, mut differ) = (0, Vec::new());
        for (n, locale) in locales.iter().enumerate() {
            let list = every_word(List::of(n));
            let mut texts: Vec<String> = (0..4000).map(|_| (0..2 + random(6)).map(|_| list[random(list.len())].as_str()).collect()).collect();
            texts.extend(more.iter().filter(|line| line.chars().any(|letter| among(ENGINES[n].letters, letter))).cloned());
            let Some(theirs) = icu(&texts, locale) else { return eprintln!("no ICU here: not tested") };
            for (text, theirs) in texts.iter().zip(theirs) {
                let ours = words(text);
                if ours != theirs {
                    differ.push(format!("{}: {text}: {ours:?}, not {theirs:?}", locale.to_string_lossy()));
                }
            }
            held += texts.len();
        }
        println!("{held} texts held against ICU, {} differ", differ.len());
        differ.iter().take(20).for_each(|line| println!("  {line}"));
        assert!(differ.is_empty(), "{} texts differ", differ.len());
    }
}
