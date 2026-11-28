//! Spell checking in text boxes: misspellings found by the system checker (so the user's own
//! dictionary and languages apply), underlined with a wavy line in the theme's red, and
//! corrected from the box's context menu, which offers suggestions first, then adding to the
//! dictionary or ignoring. A box may be checked in several languages at once (a word is right
//! if one of them has it). A word written twice in a row ("the the") is marked too; its menu
//! offers the word once.
//!
//! A box asks for it while described ([`Ui::check_spelling`]). After each layout the words of
//! its visible lines are looked up in what is known, and unknown words are sent to the checker:
//! their marks come with the answer. Only visible text is checked and answers are cached, so a
//! box of a hundred thousand lines costs what its visible lines do. The word at the cursor is
//! not marked while being typed: only once ended, by what follows or by the cursor leaving.
//!
//! A code box ([`Ui::code_editor`]) that asks has its comments and strings checked, as code
//! editors do: the pieces its language's rules ([`Syntax`](crate::Syntax)) colour as such, each
//! a box of its own, checked as part of the code.
//!
//! The checker is Windows' own (`ISpellChecker` through COM, Windows 8 on), or on Linux Enchant
//! (`libenchant-2`, opened by name on first use, as FreeType is), which asks Hunspell, Nuspell
//! or Aspell. GNOME programs check with Enchant, and its personal dictionary is the user's
//! there. (KDE programs use Sonnet, which needs Qt; its plugins read the same Hunspell
//! dictionaries.) Either takes tens of microseconds per word, and Windows' several times that
//! on a window's thread, so checking runs on a thread of its own, which wakes the window when
//! it has answers. Where there is no checker, or none for the language, nothing is marked,
//! silently.
//!
//! A program whose boxes never ask carries none of this: the first
//! [`check_spelling`](Ui::check_spelling) hands the rest over (`Hooks`), as `paint::Shows` does
//! for drawings.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::{Point, Rect, rect};
use crate::id::{Id, fold, hash};
use crate::layout::{Align, Content};
use crate::path::Path;
use crate::text::{Font, Line, Paragraph};
use crate::text_edit::{Edit, in_word, step};
use crate::ui::{Response, Ui};
use std::any::Any;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The most suggestions a menu offers.
pub(crate) const OFFERED: usize = 5;
/// The longest word sent to a checker, in bytes: anything longer is no word.
const LONGEST: usize = 64;
/// How many words per language are cached before the cache is cleared.
const REMEMBERED: usize = 50_000;
/// How long a menu opening on a misspelling waits for suggestions. (Windows takes a few
/// milliseconds.)
const OFFER_WAIT: Duration = Duration::from_millis(150);
/// A line longer than this, in bytes, has only its horizontally visible words checked.
const LONG_LINE: usize = 2000;
/// While a screen reader listens, a box's text up to this many bytes is checked whole; a longer
/// one, as far as it shows.
const TOLD_WHOLE: usize = 65_536;
/// The wavelength and height of the wavy underline, in units.
const WAVE: f32 = 4.0;
const RISE: f32 = 1.0;

/// A window's spell checking state: nothing until a box asks. Then its hooks, and all it keeps
/// (a `Kept` behind a type-erased pointer, so a program that never asks has no code to drop
/// one).
#[derive(Default)]
pub(crate) struct Spelling {
    hooks: Option<&'static Hooks>,
    kept: Option<Box<dyn Any>>,
}

impl Spelling {
    /// The spell checking hooks, once a box has asked.
    pub fn hooks(&self) -> Option<&'static Hooks> {
        self.hooks
    }
}

/// The spell checking hooks, handed over by the first box that asks.
pub(crate) struct Hooks {
    /// A text box was described, its text at this tree index (none for a box showing no text of
    /// its own: a password box, or one showing its placeholder): it is checked if it asked.
    pub noted: fn(&mut Ui, &Response, Option<u32>),
    /// The rows a box's context menu starts with, when open on a misspelling: suggestions, add
    /// to dictionary, ignore.
    pub rows: fn(&mut Ui, &Response),
    /// In the box being typed into, once its keys are taken (cursor, anchor and change count as
    /// before them): whether the word at the cursor is being written, and inserting a
    /// correction chosen from the menu.
    pub typed: fn(&mut Spelling, &mut Edit, &mut String, Id, (usize, usize, u32)),
    /// After layout: visible words are found, looked up, sent, and marked.
    pub settle: fn(&mut Ui),
    /// Draws one box's wavy lines, in a colour.
    pub wavy: Wavy,
    /// The misspellings in the box of an id, as byte ranges of its text, in order: for screen
    /// readers. All of them while one listens; else those visible. (A code box's: in the
    /// comments and strings described.)
    pub wrong: fn(&Spelling, Id) -> Vec<Range<usize>>,
    /// A code box takes the language it was asked to be checked in, if any.
    pub asked: fn(&mut Spelling) -> Option<usize>,
    /// A comment or string of a code box was described: the box's id, the piece's tree index,
    /// where its text starts in the code, and the language. It is checked.
    pub part: fn(&mut Spelling, Id, u32, usize, usize),
    /// The correction chosen from a code box's menu, for the box to make: the byte range of
    /// its code it replaces, the old text, and the new.
    pub correction: fn(&mut Spelling, Id) -> Option<Correction>,
}

/// A correction chosen from a menu: the byte range it replaces, the old text, and the new.
pub(crate) type Correction = (Range<usize>, String, String);

/// Draws the wavy lines under misspellings in tree box `at`.
pub(crate) type Wavy = fn(&mut Canvas, &Spelling, u32);

static HOOKS: Hooks = Hooks { noted, rows, typed, settle, wavy, wrong, asked, part, correction };

/// The misspellings found in box `id` when it was last processed (see `Hooks::wrong`): all,
/// while a screen reader listens; else the visible ones. (A code box's from each of its pieces
/// checked.)
fn wrong(spelling: &Spelling, id: Id) -> Vec<Range<usize>> {
    let Some(kept) = spelling.kept.as_ref().and_then(|kept| kept.downcast_ref::<Kept>()) else { return Vec::new() };
    let mut words: Vec<Range<usize>> = kept.told.iter().filter(|told| told.0 == id).flat_map(|(.., all)| all.iter().cloned()).collect();
    if !kept.told.iter().any(|told| told.0 == id) {
        words.extend(kept.found.iter().filter(|found| found.id == id).flat_map(|found| found.spots.iter().map(|(bytes, _)| bytes.clone())));
    }
    words.sort_by_key(|word| word.start);
    words
}

/// A code box takes the language it asked for (see `Hooks::asked`).
fn asked(spelling: &mut Spelling) -> Option<usize> {
    kept(spelling)?.next.take()
}

/// A code box's comment or string was described (see `Hooks::part`).
fn part(spelling: &mut Spelling, id: Id, at: u32, offset: usize, language: usize) {
    if let Some(kept) = kept(spelling) {
        kept.boxes.push(Checked { at, id, language, offset });
    }
}

/// The correction a code box's menu chose (see `Hooks::correction`).
fn correction(spelling: &mut Spelling, id: Id) -> Option<Correction> {
    kept(spelling)?.replacing.take_if(|(of, ..)| *of == id).map(|(_, range, word, with)| (range, word, with))
}

/// A checker for one language: the system's (or, in tests, a stand-in).
pub(crate) trait Check {
    /// Whether `word` is misspelled.
    fn wrong(&self, word: &str) -> bool;
    /// Suggestions for a misspelled word, likeliest first.
    fn offer(&mut self, word: &str) -> Vec<String>;
    /// Adds `word` to the user's dictionary, permanently.
    fn add(&mut self, word: &str);
}

/// Opens a language's checker by the app's name for it ("" for the user's own): none where
/// there is none.
pub(crate) type Opens = fn(&str) -> Option<Box<dyn Check>>;

/// What is known of a word.
#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    /// Sent to the checker, which has not answered.
    Asked,
    Right,
    Wrong,
}

/// A language boxes are checked in, and what is known of its words.
struct Language {
    /// Its name as the app gave it ("" for the user's own).
    name: String,
    known: HashMap<Box<str>, Verdict>,
    /// It has no checker: nothing is sent, nothing is misspelled.
    none: bool,
    /// Counts changes to what is known of its words.
    version: u64,
}

/// A box checked in this pass: its tree index, id and language; and where its text starts in
/// the text the box stands for (a code box's comment or string: in all its code).
struct Checked {
    at: u32,
    id: Id,
    language: usize,
    offset: usize,
}

/// What was found in a visible box: a hash of its inputs (unchanged while nothing it depends on
/// changes), and its misspellings, each as byte range and where it shows (a word may show in
/// pieces in bidirectional text), with their wavy lines.
struct Found {
    id: Id,
    key: u64,
    spots: Vec<(Range<usize>, Rect)>,
    waves: Vec<Rect>,
}

/// A box's context menu, open on a misspelling: the box, the word (byte range, text, language)
/// and its suggestions; and whether the menu was described this pass.
struct Menu {
    of: Id,
    word: Option<(Range<usize>, String, usize)>,
    offered: Vec<String>,
    seen: bool,
    /// For a word written twice: the word once (`word` is then both).
    single: Option<String>,
}

/// Who answers checker requests.
enum Checking {
    /// The interface's own thread: each language's checker, once opened (none where there is
    /// none).
    Here(Opens, Vec<Option<Option<Box<dyn Check>>>>),
    /// A thread of its own.
    Away(Arc<Shared>),
}

/// All that is kept for checking a window's boxes.
struct Kept {
    /// The language the next described box is checked in, once it asked.
    next: Option<usize>,
    languages: Vec<Language>,
    /// The boxes checked this pass, and what was found in the visible ones last time.
    boxes: Vec<Checked>,
    found: Vec<Found>,
    menu: Option<Menu>,
    /// A correction chosen from a menu, until the box takes it: the box, the byte range it
    /// replaces, the old text, and the new.
    replacing: Option<(Id, Range<usize>, String, String)>,
    /// The words the user ignored, until the program ends; and how many times.
    ignored: Vec<Box<str>>,
    ignores: u64,
    /// The words to send to a checker from this pass, not yet sent: each with its language.
    asking: Vec<(usize, String)>,
    /// While a screen reader listens: each checked box's misspellings in all its text (see
    /// `told_in`), with a hash of what they were found from.
    told: Vec<(Id, u64, Vec<Range<usize>>)>,
    /// This run's wavy lines in window coordinates, each with the box whose text it underlines,
    /// in description order; and their colour, the theme's red.
    wrong: Vec<(u32, Rect)>,
    red: Color,
    checking: Checking,
}

/// State shared by the interface and the checking thread: work and results, and the condition
/// signalling a change on either side.
struct Shared {
    queue: Mutex<Queue>,
    signal: Condvar,
}

/// The checking thread's work, and its results.
#[derive(Default)]
struct Queue {
    /// The languages, by index, as the app named them.
    languages: Vec<String>,
    /// Words to check, each with its language; and to add to the dictionary.
    asked: Vec<(usize, String)>,
    added: Vec<(usize, String)>,
    /// A word to suggest for, until taken up; and the suggestions for one.
    offer: Option<(usize, String)>,
    offered: Option<(usize, String, Vec<String>)>,
    /// Results: whether each checked word is misspelled.
    answers: Vec<(usize, String, bool)>,
    /// The languages found to have no checker.
    missing: Vec<usize>,
    /// The window is gone: the thread ends.
    closed: bool,
}

impl Language {
    fn new(name: &str) -> Self {
        Language { name: name.to_owned(), known: HashMap::new(), none: false, version: 0 }
    }
    /// Records what is now known of a word.
    fn learn(&mut self, word: &str, verdict: Verdict) {
        if self.known.len() >= REMEMBERED {
            self.known.clear();
        }
        self.known.insert(word.into(), verdict);
        // (A word only sent shows as before: no box needs another look for it.)
        self.version += (verdict != Verdict::Asked) as u64;
    }
}

impl Kept {
    fn new(opens: Opens, here: bool) -> Self {
        let checking = if here { Checking::Here(opens, Vec::new()) } else { Checking::away(opens) };
        Kept {
            next: None,
            languages: Vec::new(),
            boxes: Vec::new(),
            found: Vec::new(),
            menu: None,
            replacing: None,
            ignored: Vec::new(),
            ignores: 0,
            asking: Vec::new(),
            told: Vec::new(),
            wrong: Vec::new(),
            red: Color::NONE,
            checking,
        }
    }

    /// The index of the language of this name, added if new.
    fn language(&mut self, name: &str) -> usize {
        if let Some(at) = self.languages.iter().position(|language| language.name == name) {
            return at;
        }
        self.languages.push(Language::new(name));
        if let Checking::Away(shared) = &self.checking {
            shared.lock().languages.push(name.to_owned());
        }
        self.languages.len() - 1
    }

    /// Whether `word` is misspelled in language `at`, as far as known. An unknown word is
    /// checked: here at once, or by the checking thread, which answers later.
    fn verdict(&mut self, at: usize, word: &str) -> Verdict {
        let language = &self.languages[at];
        if language.none || self.ignored.iter().any(|ignored| **ignored == *word) {
            return Verdict::Right;
        }
        if let Some(verdict) = language.known.get(word) {
            return *verdict;
        }
        // (Here: none where the language has no checker.)
        let checked = match &mut self.checking {
            Checking::Here(opens, checkers) => Some(Checking::here(*opens, checkers, &self.languages[at].name, at).map(|checker| checker.wrong(word))),
            Checking::Away(_) => None,
        };
        let verdict = match checked {
            None => Verdict::Asked,
            Some(None) => Verdict::Right,
            Some(Some(wrong)) => [Verdict::Right, Verdict::Wrong][wrong as usize],
        };
        if verdict == Verdict::Asked {
            self.asking.push((at, word.to_owned()));
        }
        self.languages[at].none |= checked == Some(None);
        self.languages[at].learn(word, verdict);
        verdict
    }

    /// Hands this pass's words to the checking thread, and takes in its answers since the last
    /// pass.
    fn send_and_take(&mut self) {
        let Checking::Away(shared) = &self.checking else { return };
        let mut queue = shared.lock();
        let (answers, missing) = (std::mem::take(&mut queue.answers), std::mem::take(&mut queue.missing));
        if !self.asking.is_empty() {
            queue.asked.append(&mut self.asking);
            shared.signal.notify_all();
        }
        drop(queue);
        for (at, word, wrong) in answers {
            self.languages[at].learn(&word, [Verdict::Right, Verdict::Wrong][wrong as usize]);
        }
        for at in missing {
            (self.languages[at].none, self.languages[at].version) = (true, self.languages[at].version + 1);
        }
    }

    /// Language `at`'s suggestions for `word`. (From the checking thread they are waited for
    /// briefly: a menu that grew rows after opening would shift under the pointer.)
    fn offer(&mut self, at: usize, word: &str) -> Vec<String> {
        let shared = match &mut self.checking {
            Checking::Here(opens, checkers) => return Checking::here(*opens, checkers, &self.languages[at].name, at).map_or(Vec::new(), |checker| checker.offer(word)),
            Checking::Away(shared) => shared.clone(),
        };
        let mut queue = shared.lock();
        (queue.offer, queue.offered) = (Some((at, word.to_owned())), None);
        shared.signal.notify_all();
        let waited = shared.signal.wait_timeout_while(queue, OFFER_WAIT, |queue| queue.offered.is_none() && !queue.closed);
        let mut queue = waited.map_or_else(|poisoned| poisoned.into_inner().0, |(queue, _)| queue);
        queue.offer = None;
        queue.offered.take().filter(|(language, offered, _)| *language == at && offered == word).map_or(Vec::new(), |(.., offers)| offers)
    }

    /// Adds `word` to the user's dictionary for language `at`: correct from now on.
    fn add(&mut self, at: usize, word: &str) {
        match &mut self.checking {
            Checking::Here(opens, checkers) => Checking::here(*opens, checkers, &self.languages[at].name, at).into_iter().for_each(|checker| checker.add(word)),
            Checking::Away(shared) => {
                shared.lock().added.push((at, word.to_owned()));
                shared.signal.notify_all();
            }
        }
        self.languages[at].learn(word, Verdict::Right);
    }

    /// Treats `word` as correct everywhere, in any language, until the program ends.
    fn ignore(&mut self, word: &str) {
        self.ignored.push(word.into());
        self.ignores += 1;
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        // (The checking thread ends with the window, dropping its checkers.)
        if let Checking::Away(shared) = &self.checking {
            shared.lock().closed = true;
            shared.signal.notify_all();
        }
    }
}

impl Checking {
    /// A checking thread that opens checkers with `opens` and answers requests. (Without a
    /// thread, nothing is checked: as with no checker.)
    fn away(opens: Opens) -> Self {
        let shared = Arc::new(Shared { queue: Mutex::new(Queue::default()), signal: Condvar::new() });
        let theirs = shared.clone();
        match std::thread::Builder::new().name("taut spelling".into()).spawn(move || work(&theirs, opens)) {
            Ok(_) => Checking::Away(shared),
            Err(_) => Checking::Here(|_| None, Vec::new()),
        }
    }

    /// Language `at`'s checker, named `name`, among `checkers`: opened on first use.
    fn here<'a>(opens: Opens, checkers: &'a mut Vec<Option<Option<Box<dyn Check>>>>, name: &str, at: usize) -> Option<&'a mut Box<dyn Check>> {
        if checkers.len() <= at {
            checkers.resize_with(at + 1, || None);
        }
        checkers[at].get_or_insert_with(|| opens(name)).as_mut()
    }
}

impl Shared {
    /// The queue, locked. (Poisoned by a panic, it is still consistent: it is only appended to
    /// and taken whole.)
    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The next work, waited for: language names, words to check, to add, and to suggest for.
    /// Nothing once the window is gone.
    fn next(&self) -> Option<Work> {
        let busy = |queue: &mut Queue| queue.asked.is_empty() && queue.added.is_empty() && queue.offer.is_none() && !queue.closed;
        let mut queue = self.signal.wait_while(self.lock(), busy).unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.closed {
            return None;
        }
        Some(Work { languages: queue.languages.clone(), asked: std::mem::take(&mut queue.asked), added: std::mem::take(&mut queue.added), offer: queue.offer.clone() })
    }
}

/// One batch of the checking thread's work.
struct Work {
    languages: Vec<String>,
    asked: Vec<(usize, String)>,
    added: Vec<(usize, String)>,
    offer: Option<(usize, String)>,
}

/// The checking thread: opens each language's checker on first use, answers until the window
/// goes, and wakes the window after each answer.
fn work(shared: &Shared, opens: Opens) {
    let mut checkers = Vec::new();
    while let Some(work) = shared.next() {
        let mut answers = Vec::with_capacity(work.asked.len());
        for (at, word) in work.added {
            Checking::here(opens, &mut checkers, &work.languages[at], at).into_iter().for_each(|checker| checker.add(&word));
        }
        let offered = work.offer.map(|(at, word)| {
            let offers = Checking::here(opens, &mut checkers, &work.languages[at], at).map_or(Vec::new(), |checker| checker.offer(&word));
            (at, word, offers)
        });
        for (at, word) in work.asked {
            let wrong = Checking::here(opens, &mut checkers, &work.languages[at], at).map(|checker| checker.wrong(&word));
            answers.push((at, word, wrong));
        }
        let mut queue = shared.lock();
        queue.offered = offered.or(queue.offered.take());
        queue.missing.extend(answers.iter().filter(|(.., wrong)| wrong.is_none()).map(|(at, ..)| *at));
        queue.answers.extend(answers.into_iter().map(|(at, word, wrong)| (at, word, wrong == Some(true))));
        drop(queue);
        shared.signal.notify_all();
        crate::platform::Waker.wake();
    }
}

/// A line's words to check, as byte ranges: runs of letters, with following combining marks,
/// and an apostrophe between two letters. A run with a digit or `_` (a code identifier, a part
/// number), or part of an address (an address character just before or after: `@`, `/`, ``, or
/// `.` or `:` followed by more), is skipped; so are single letters and runs longer than
/// `LONGEST`.
fn words(line: &str, into: &mut Vec<Range<usize>>) {
    let mut at = 0;
    while at < line.len() {
        if !in_word(line, at) {
            at = step(line, at, true);
            continue;
        }
        let (start, mut plain) = (at, true);
        while at < line.len() && (in_word(line, at) || joins(line, at)) {
            plain &= !line[at..].starts_with(|c: char| c.is_numeric() || c == '_');
            at = step(line, at, true);
        }
        if plain && at - start <= LONGEST && line[start..at].chars().nth(1).is_some() && !address(line, start..at) {
            into.push(start..at);
        }
    }
}

/// Whether the character at `at` is an apostrophe between two letters, joining one word
/// ("don't", "l'eau").
fn joins(line: &str, at: usize) -> bool {
    let next = step(line, at, true);
    line[at..].starts_with(['\'', '\u{2019}']) && at > 0 && in_word(line, step(line, at, false)) && next < line.len() && in_word(line, next)
}

/// Whether the letter run `word` of `line` is part of an address, path or code identifier, by
/// the characters around it.
fn address(line: &str, word: Range<usize>) -> bool {
    let mut before = line[..word.start].chars().rev();
    let mut after = line[word.end..].chars();
    let ([before, earlier], [next, beyond]) = ([before.next(), before.next()], [after.next(), after.next()]);
    let more = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    matches!(before, Some('@' | '/' | '\\')) || before == Some('.') && more(earlier) || matches!(next, Some('@' | '/' | '\\')) || matches!(next, Some('.' | ':')) && more(beyond)
}

/// The spell checking state, if a box has asked.
fn kept(spelling: &mut Spelling) -> Option<&mut Kept> {
    spelling.kept.as_mut()?.downcast_mut::<Kept>()
}

/// Runs `work` with the kept state taken out of the interface meanwhile, so the interface can
/// be described.
fn with_kept(ui: &mut Ui, work: impl FnOnce(&mut Ui, &mut Kept)) {
    let Some(mut held) = ui.asks.spelling.kept.take() else { return };
    if let Some(kept) = held.downcast_mut::<Kept>() {
        work(ui, kept);
    }
    ui.asks.spelling.kept = Some(held);
}

/// A text box was described (see `Hooks::noted`).
fn noted(ui: &mut Ui, state: &Response, at: Option<u32>) {
    let Some(kept) = kept(&mut ui.asks.spelling) else { return };
    let (Some(language), Some(at)) = (kept.next.take(), at) else { return };
    kept.boxes.push(Checked { at, id: state.id, language, offset: 0 });
}

/// The box being typed into took its keys (see `Hooks::typed`).
fn typed(spelling: &mut Spelling, edit: &mut Edit, text: &mut String, id: Id, was: (usize, usize, u32)) {
    // The word at the cursor is being written when the last change was to the text; once the
    // cursor moves by itself, it is not.
    if was != (edit.cursor, edit.anchor, edit.version) {
        edit.writing = was.2 != edit.version;
    }
    let Some(kept) = kept(spelling) else { return };
    let Some((_, range, word, with)) = kept.replacing.take_if(|(of, ..)| *of == id) else { return };
    // (Unless the app changed the text meanwhile.)
    if text.get(range.clone()) == Some(word.as_str()) {
        edit.replace(text, range, &with, false);
        edit.writing = false;
    }
}

/// A box menu's first rows (see `Hooks::rows`).
fn rows(ui: &mut Ui, state: &Response) {
    with_kept(ui, |ui, kept| kept.rows(ui, state));
}

/// After layout (see `Hooks::settle`).
fn settle(ui: &mut Ui) {
    with_kept(ui, |ui, kept| kept.settle(ui));
}

impl Kept {
    /// A box menu's first rows, when open on a misspelling; and which word, decided as it
    /// opens: the one under the pointer, or (opened by key) at the cursor.
    fn rows(&mut self, ui: &mut Ui, state: &Response) {
        let Some(language) = self.boxes.iter().find(|checked| checked.id == state.id).map(|checked| checked.language) else { return };
        if self.menu.as_ref().is_none_or(|menu| menu.of != state.id) {
            let mut word = self.word_for_menu(ui, state, language);
            let twice = word.as_ref().and_then(|(range, ..)| self.twice_in_box(ui, state.id, range.clone()));
            if let Some((both, _)) = &twice {
                word = self.spelt(ui, state.id, both.clone()).map(|spelt| (both.clone(), spelt, language));
            }
            let offered = match (&word, &twice) {
                (_, Some((_, single))) => vec![single.clone()],
                (Some((_, word, _)), None) => self.offer(language, word),
                (None, None) => Vec::new(),
            };
            self.menu = Some(Menu { of: state.id, word, offered, seen: false, single: twice.map(|(.., single)| single) });
        }
        let Some(menu) = self.menu.as_mut() else { return };
        menu.seen = true;
        let Some((range, word, _)) = menu.word.clone() else { return };
        let mut picked = None;
        for offer in &menu.offered {
            // (A "&" in a word would mark its access key: doubled, it is itself.)
            if ui.menu_item(&offer.replace('&', "&&")).clicked {
                picked = Some(offer.clone());
            }
        }
        // (Not added to the dictionary: a word written twice is no misspelling.)
        let add = menu.single.is_none() && ui.menu_item(ui.edit_words[4]).clicked;
        let ignore = ui.menu_item(ui.edit_words[5]).clicked;
        let single = menu.single.clone();
        ui.separator();
        if let Some(with) = picked {
            self.replacing = Some((state.id, range, word.clone(), with));
        }
        if add {
            self.add(language, &word);
        }
        if ignore {
            self.ignore(single.as_deref().unwrap_or(&word));
        }
        if add || ignore || self.replacing.is_some() {
            // (Focus returns to the box, where the correction goes in next pass.)
            ui.set_focus(state.id);
            ui.rerun();
        }
    }

    /// The misspelling a box's menu opens on, if any: its byte range, text, and language.
    fn word_for_menu(&self, ui: &Ui, state: &Response, language: usize) -> Option<(Range<usize>, String, usize)> {
        let mut spots = self.found.iter().filter(|found| found.id == state.id).flat_map(|found| found.spots.iter());
        let pointer = ui.pointer().filter(|_| state.right_clicked);
        let cursor = (ui.edit.id == state.id).then_some(ui.edit.cursor);
        let on = |(range, spot): &&(Range<usize>, Rect)| match pointer {
            Some(p) => spot.contains(p),
            None => cursor.is_some_and(|at| range.contains(&at) || range.end == at),
        };
        let (range, _) = spots.find(on)?;
        Some((range.clone(), self.spelt(ui, state.id, range.clone())?, language))
    }

    /// The word at byte range `range` of box `id`'s text, as in the tree.
    fn spelt(&self, ui: &Ui, id: Id, range: Range<usize>) -> Option<String> {
        let (text, offset) = self.text_holding(ui, id, &range)?;
        text.get(range.start - offset..range.end - offset).map(str::to_owned)
    }

    /// The text of a box checked as box `id` that holds byte range `range` of the text `id`
    /// stands for (in a code box, the comment or string holding it), and where it starts there.
    fn text_holding<'a>(&self, ui: &'a Ui, id: Id, range: &Range<usize>) -> Option<(&'a str, usize)> {
        self.boxes.iter().filter(|checked| checked.id == id).find_map(|checked| {
            let Content::Text { start, end, .. } = ui.tree.specs.get(checked.at as usize)?.content else { return None };
            let text = ui.tree.strings.get(start as usize..end as usize)?;
            (range.start >= checked.offset && range.end <= checked.offset + text.len()).then_some((text, checked.offset))
        })
    }

    /// Where the same word (in any case) ends just before `word`, a byte range of `text` whose box
    /// starts at `start`, with only spaces between: a word written twice. Its start, unless the
    /// user ignored the word.
    fn twice(&self, text: &str, start: usize, word: Range<usize>) -> Option<usize> {
        let spelt = &text[word.clone()];
        let earlier = text[start..word.start].trim_end_matches([' ', '\t', '\u{a0}']);
        let end = start + earlier.len();
        let from = end.checked_sub(spelt.len()).filter(|from| *from >= start && text.is_char_boundary(*from) && end < word.start)?;
        let alone = from == start || !in_word(text, step(text, from, false));
        let ignored = self.ignored.iter().any(|ignored| ignored.to_lowercase() == spelt.to_lowercase());
        (alone && !ignored && text[from..end].to_lowercase() == spelt.to_lowercase()).then_some(from)
    }

    /// For a word written twice at byte range `range` of box `id`'s text: the range of both,
    /// and the word once (as written first).
    fn twice_in_box(&self, ui: &Ui, id: Id, range: Range<usize>) -> Option<(Range<usize>, String)> {
        let (text, offset) = self.text_holding(ui, id, &range)?;
        let from = self.twice(text, 0, range.start - offset..range.end - offset)?;
        Some((from + offset..range.end, text[from..from + range.len()].to_owned()))
    }

    /// After layout: each box checked this pass has its visible words found, looked up, sent
    /// and marked. A box whose inputs are unchanged keeps what was found last time.
    fn settle(&mut self, ui: &mut Ui) {
        self.send_and_take();
        // (A menu not described last pass has closed.)
        if self.menu.as_mut().is_some_and(|menu| !std::mem::take(&mut menu.seen)) {
            self.menu = None;
        }
        self.red = ui.theme.red;
        self.wrong.clear();
        let (boxes, mut was_told) = (std::mem::take(&mut self.boxes), std::mem::take(&mut self.told));
        let mut found = Vec::with_capacity(boxes.len());
        for checked in &boxes {
            if ui.listening && Self::checked_whole(ui, checked) {
                let told = self.told_in(ui, checked, &mut was_told);
                self.told.push(told);
            }
            let Some((area, now)) = self.find(ui, checked) else { continue };
            self.wrong.extend(now.waves.iter().map(|wave| (checked.at, *wave)));
            // (The lines are a mark of their own: so they redraw when misspellings come or go
            // though the text is unchanged.)
            let look = now.waves.iter().fold(hash(checked.at), |look, wave| fold(fold(look, wave.x.to_bits() as u64), (wave.y.to_bits() as u64) << 32 | wave.w.to_bits() as u64));
            ui.marks.push((look, area));
            found.push(now);
        }
        (self.found, self.boxes, self.next) = (found, boxes, None);
        self.boxes.clear();
        self.send_and_take();
    }

    /// Whether a box's text is short enough to be checked whole (see `TOLD_WHOLE`).
    fn checked_whole(ui: &Ui, checked: &Checked) -> bool {
        matches!(ui.tree.specs[checked.at as usize].content, Content::Text { start, end, .. } if (end - start) as usize <= TOLD_WHOLE)
    }
    /// While a screen reader listens: the misspellings in all of a checked box's text, seen or
    /// not, as byte ranges of the text it stands for (as far as known, unknown words sent). Not
    /// the word being written. Taken from `was` where its text and what is known are unchanged.
    fn told_in(&mut self, ui: &mut Ui, checked: &Checked, was: &mut Vec<(Id, u64, Vec<Range<usize>>)>) -> (Id, u64, Vec<Range<usize>>) {
        let Content::Text { start, end, .. } = ui.tree.specs[checked.at as usize].content else { return (checked.id, 0, Vec::new()) };
        let (start, end) = (start as usize, end as usize);
        let typing = ui.focus == checked.id && ui.edit.id == checked.id && ui.edit.writing && ui.edit.cursor == ui.edit.anchor;
        let writing = typing.then(|| (start + ui.edit.cursor).checked_sub(checked.offset)).flatten();
        let key = hash((&ui.tree.strings[start..end], self.languages[checked.language].version, self.ignores, writing, checked.offset));
        if let Some(at) = was.iter().position(|(id, made, _)| *id == checked.id && *made == key) {
            return was.swap_remove(at);
        }
        let (mut found, mut wrong) = (Vec::new(), Vec::new());
        words(&ui.tree.strings[start..end], &mut found);
        for word in found.into_iter().map(|word| start + word.start..start + word.end) {
            if writing.is_some_and(|at| word.start <= at && at <= word.end) {
                continue;
            }
            let twice = !self.languages[checked.language].none && self.twice(&ui.tree.strings, start, word.clone()).is_some();
            if twice || self.verdict(checked.language, &ui.tree.strings[word.clone()]) == Verdict::Wrong {
                wrong.push(word.start - start + checked.offset..word.end - start + checked.offset);
            }
        }
        (checked.id, key, wrong)
    }

    /// A checked box's visible content and the window area it covers, its misspellings as byte
    /// ranges of the text it stands for: reused where nothing it depends on changed. Nothing
    /// for a box out of view.
    fn find(&mut self, ui: &mut Ui, checked: &Checked) -> Option<(Rect, Found)> {
        let seen = ui.visible_boxes.boxes.iter().find(|seen| seen.at == checked.at && !seen.leaving)?;
        let (r, area) = (seen.rect, seen.area);
        let view = View::of(ui, checked.at, r, area)?;
        // The word being written, if in this box: unchecked until ended.
        let typing = ui.focus == checked.id && ui.edit.id == checked.id && ui.edit.writing && ui.edit.cursor == ui.edit.anchor;
        let writing = typing.then(|| (view.start + ui.edit.cursor).checked_sub(checked.offset)).flatten();
        let language = &self.languages[checked.language];
        let shown = &ui.tree.strings[view.lines.first()?.start..view.lines.last()?.end];
        let key =
            hash((r.x.to_bits(), r.y.to_bits(), area.y.to_bits(), area.h.to_bits(), view.font.size.to_bits(), ui.scale.to_bits(), shown, (language.version, self.ignores, writing, checked.offset)));
        if let Some(at) = self.found.iter().position(|found| found.id == checked.id && found.key == key) {
            return Some((area, self.found.swap_remove(at)));
        }
        let mut now = Found { id: checked.id, key, spots: Vec::new(), waves: Vec::new() };
        let mut wrong = Vec::new();
        for (row, line) in view.lines.iter().enumerate() {
            self.wrong_in(ui, &view, line, writing, checked.language, &mut wrong);
            view.spots(ui, line, view.first + row, &wrong, &mut now);
        }
        // (A code box's piece is placed in its code.)
        now.spots.iter_mut().for_each(|(bytes, _)| *bytes = bytes.start + checked.offset..bytes.end + checked.offset);
        Some((area, now))
    }

    /// One line's misspellings, as byte ranges of the window's text: as far as known, unknown
    /// words sent. Not the word at `writing`, being written.
    fn wrong_in(&mut self, ui: &mut Ui, view: &View, line: &Line, writing: Option<usize>, language: usize, wrong: &mut Vec<Range<usize>>) {
        wrong.clear();
        let mut found = Vec::new();
        words(&ui.tree.strings[line.start..line.end], &mut found);
        // (Of a very long line, only horizontally visible words.)
        let seen = if line.end - line.start > LONG_LINE { view.across(ui, line) } else { None };
        for word in found.into_iter().map(|word| line.start + word.start..line.start + word.end) {
            let out = seen.as_ref().is_some_and(|seen| word.end <= seen.start || word.start >= seen.end);
            if out || writing.is_some_and(|at| word.start <= at && at <= word.end) {
                continue;
            }
            let twice = !self.languages[language].none && self.twice(&ui.tree.strings, view.start, word.clone()).is_some();
            if twice || self.verdict(language, &ui.tree.strings[word.clone()]) == Verdict::Wrong {
                wrong.push(word);
            }
        }
    }
}

/// The visible part of a checked box: its text range in the window's text, its font, its
/// visible lines (the first being line `first`), the first one's position, line height and
/// baseline lift, the available width, and alignment.
struct View {
    start: usize,
    end: usize,
    font: Font,
    lines: Vec<Line>,
    first: usize,
    inside: Rect,
    top: f32,
    line_height: f32,
    lift: f32,
    side: f32,
    area: Rect,
}

impl View {
    /// What is visible of tree box `at`, at `r` in the window and visible in `area`: as
    /// `Painter::words` draws it.
    fn of(ui: &Ui, at: u32, r: Rect, area: Rect) -> Option<View> {
        let (tree, scale) = (&ui.tree, ui.scale);
        let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
        let Content::Text { start, end, font, wrap, side, .. } = spec.content else { return None };
        let inside = r.inset(spec.frame.pad);
        let asked = place.fit[0] - spec.frame.pad.horizontal();
        let wide = if side == Align::Start { asked } else { tree.text_width(at, scale).unwrap_or(asked) };
        let whole = [Line { start: start as usize, end: end as usize, width: wide }];
        let lines = if wrap { &tree.lines[place.lines.0 as usize..place.lines.1 as usize] } else { &whole[..] };
        let (high, top) = (place.line_height.max(1.0), inside.y + if wrap { 0.0 } else { ((inside.h - place.line_height) / 2.0).max(0.0) });
        // (The visible lines, by position.)
        let first = (((area.y - top) / high).floor().max(0.0) as usize).min(lines.len());
        let last = (((area.bottom() - top) / high).ceil().max(0.0) as usize).clamp(first, lines.len());
        let side = [0.0, 0.5, 1.0, 0.0][side as usize];
        Some(View { start: start as usize, end: end as usize, font, lines: lines[first..last].to_vec(), first, inside, top, line_height: place.line_height, lift: place.lift, side, area })
    }

    /// The paragraph of line `line` (see `Paragraph`), in the window's text `strings`.
    fn paragraph<'a>(&self, strings: &'a str, line: &Line) -> Paragraph<'a> {
        Paragraph::of(&strings[self.start..self.end], line.start - self.start..line.end - self.start)
    }

    /// Line `line`'s horizontal start, in a paragraph running right to left or not: as
    /// `Painter::words` has it.
    fn line_start(&self, line: &Line, right_to_left: bool) -> f32 {
        self.inside.x + ((self.inside.w - line.width).max(0.0) * if right_to_left { 1.0 - self.side } else { self.side }).round()
    }

    /// The horizontally visible byte range of a very long line, rounded to whole characters.
    fn across(&self, ui: &mut Ui, line: &Line) -> Option<Range<usize>> {
        let (text, paragraph) = (&ui.tree.strings[line.start..line.end], self.paragraph(&ui.tree.strings, line));
        if !paragraph.plain(text) {
            return None;
        }
        let x = self.line_start(line, paragraph.right_to_left);
        let mut stops = Vec::new();
        ui.text.measure(text, self.font, ui.scale, Some(&mut stops));
        let (from, to) = (self.area.x - x, self.area.right() - x);
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(at, _)| at);
        Some(line.start + byte(stops.partition_point(|stop| *stop <= from))..line.start + byte(stops.partition_point(|stop| *stop < to) + 1))
    }

    /// Where line `row`'s misspellings (`wrong`, byte ranges of the window's text) show, and
    /// their wavy lines.
    fn spots(&self, ui: &mut Ui, line: &Line, row: usize, wrong: &[Range<usize>], found: &mut Found) {
        if wrong.is_empty() {
            return;
        }
        let paragraph = self.paragraph(&ui.tree.strings, line);
        let (x, y) = (self.line_start(line, paragraph.right_to_left), self.top + row as f32 * self.line_height);
        let metrics = ui.text.metrics(self.font, ui.scale);
        // (Just under the baseline, never below the line's own height.)
        let middle = (y - self.lift + metrics.ascent + RISE + 1.0).min(y + self.line_height - RISE - 0.5);
        // (A line shown as typed is measured once, each character after the previous. Other
        // text a word at a time, as displayed.)
        let text = &ui.tree.strings[line.start..line.end];
        let (plain, mut stops, mut pieces) = (paragraph.plain(text), Vec::new(), Vec::new());
        if plain {
            ui.text.measure(text, self.font, ui.scale, Some(&mut stops));
        }
        let mut counted = (line.start, 0);
        for word in wrong {
            match plain {
                true => (pieces.clear(), pieces.push([at_byte(text, &stops, line.start, word.start, &mut counted), at_byte(text, &stops, line.start, word.end, &mut counted)])).1,
                false => _ = crate::text::marks(ui.text.as_mut(), text, line.start, line.width, self.font, ui.scale, paragraph, [word.start, word.end], None, false, &mut pieces),
            }
            let bytes = word.start - self.start..word.end - self.start;
            found.spots.extend(pieces.iter().map(|[left, right]| (bytes.clone(), rect(x + left, y, right - left, self.line_height))));
            found.waves.extend(pieces.iter().map(|[left, right]| rect(x + left, middle - RISE, right - left, 2.0 * RISE)));
        }
    }
}

/// How far into a plain line (starting at byte `start` of the window's text, characters ending
/// at `stops`) byte `at` lies: counted on from `counted` (a byte and the characters before it),
/// which is advanced there.
fn at_byte(text: &str, stops: &[f32], start: usize, at: usize, counted: &mut (usize, usize)) -> f32 {
    let (from, before) = *counted;
    *counted = (at, before + text[from - start..at - start].chars().count());
    counted.1.checked_sub(1).and_then(|last| stops.get(last)).copied().unwrap_or(0.0)
}

/// Draws box `at`'s wavy lines in the theme's red (see `Hooks::wavy`): a wave every `WAVE`
/// units, `RISE` high, across each word.
fn wavy(canvas: &mut Canvas, spelling: &Spelling, at: u32) {
    let Some(Kept { wrong, red, .. }) = spelling.kept.as_ref().and_then(|kept| kept.downcast_ref::<Kept>()) else { return };
    let from = wrong.partition_point(|(of, _)| *of < at);
    for (_, wave) in wrong[from..].iter().take_while(|(of, _)| *of == at) {
        let (middle, mut path, mut x) = (wave.y + wave.h / 2.0, Path::new(), wave.x);
        path.move_to(x, middle);
        let mut up = true;
        while x < wave.right() - 0.01 {
            let half = (WAVE / 2.0).min(wave.right() - x);
            path.quad_to(x + half / 2.0, middle + if up { -2.0 * RISE } else { 2.0 * RISE }, x + half, middle);
            (x, up) = (x + half, !up);
        }
        canvas.stroke(&path, Point::default(), 1.0, 1.0, *red);
    }
}

/// Spell checking in text boxes.
impl Ui {
    /// Has the next text box underline its misspelled words with a wavy line in the theme's
    /// red, as system text boxes do. Its context menu (right-click on a marked word, or the
    /// menu key with the cursor on it) then offers suggestions first, and adding the word to
    /// the user's dictionary or ignoring it ([`edit_words`](Self::edit_words) gives those in
    /// the app's language). Words are checked by the system checker with the user's dictionary,
    /// in `language` as the system names languages ("en-US", "de_DE", "fr"); or with none, the
    /// user's own language. Only visible text is checked, and marks appear as the checker
    /// answers, from its own thread: a box of any length costs what it shows. Where the system
    /// has no checker for the language, nothing is marked. A password box is never checked; a
    /// code box ([`code_editor`](Self::code_editor)) has its comments and strings checked.
    pub fn check_spelling(&mut self, language: Option<&str>) {
        self.spell_with(language, system, false);
    }

    /// The same, with checkers from `opens`, called on this thread (`here`) or their own. (The
    /// first box that asks decides: a stand-in, in tests.)
    pub(crate) fn spell_with(&mut self, language: Option<&str>, opens: Opens, here: bool) {
        let spelling = &mut self.asks.spelling;
        if spelling.kept.is_none() {
            (spelling.hooks, spelling.kept) = (Some(&HOOKS), Some(Box::new(Kept::new(opens, here)) as Box<dyn Any>));
        }
        let Some(kept) = kept(spelling) else { return };
        kept.next = Some(kept.language(language.unwrap_or("")));
    }
}

// ---- the system's checkers ----

/// The system checker for the language of this name ("" for the user's own), if any: on Windows
/// its own; on Linux Enchant's, for whatever dictionary Enchant finds; on macOS
/// `NSSpellChecker` (`ns_spell_checker.rs`); on Android its spell checker
/// (`android_spell_checker.rs`); elsewhere none.
pub(crate) fn system(language: &str) -> Option<Box<dyn Check>> {
    several(language, |name| Speller::open(name).map(|speller| Box::new(speller) as Box<dyn Check>))
}

/// The checkers `one` opens for each of `languages` (comma-separated; "" for the user's own),
/// as one: none where none has one.
fn several(languages: &str, one: Opens) -> Option<Box<dyn Check>> {
    let named = |name: &str| if name.is_empty() { own_language() } else { name.to_owned() };
    let mut checkers: Vec<Box<dyn Check>> = languages.split(',').filter_map(|name| one(&named(name.trim()))).collect();
    match checkers.len() {
        0 | 1 => checkers.pop(),
        _ => Some(Box::new(Several(checkers))),
    }
}

/// Checkers for several languages as one: a word is right if one of them has it.
struct Several(Vec<Box<dyn Check>>);

impl Check for Several {
    fn wrong(&self, word: &str) -> bool {
        self.0.iter().all(|checker| checker.wrong(word))
    }
    /// Each language's suggestions in turn, first ones first, each once.
    fn offer(&mut self, word: &str) -> Vec<String> {
        let lists: Vec<Vec<String>> = self.0.iter_mut().map(|checker| checker.offer(word)).collect();
        let mut offers: Vec<String> = Vec::new();
        for offer in (0..lists.iter().map(Vec::len).max().unwrap_or(0)).flat_map(|nth| lists.iter().filter_map(move |list| list.get(nth))) {
            if offers.len() < OFFERED && !offers.contains(offer) {
                offers.push(offer.clone());
            }
        }
        offers
    }
    /// To the first language's dictionary.
    fn add(&mut self, word: &str) {
        self.0.iter_mut().take(1).for_each(|checker| checker.add(word));
    }
}

#[cfg(target_os = "android")]
use crate::android_spell_checker::{Speller, own_language};
#[cfg(target_os = "macos")]
use crate::ns_spell_checker::{Speller, own_language};

#[cfg(any(target_os = "ios", not(any(windows, unix))))]
struct Speller;
#[cfg(any(target_os = "ios", not(any(windows, unix))))]
impl Speller {
    fn open(_: &str) -> Option<Speller> {
        None
    }
}
#[cfg(any(target_os = "ios", not(any(windows, unix))))]
impl Check for Speller {
    fn wrong(&self, _: &str) -> bool {
        false
    }
    fn offer(&mut self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn add(&mut self, _: &str) {}
}
#[cfg(any(target_os = "ios", not(any(windows, unix))))]
fn own_language() -> String {
    String::new()
}

#[cfg(windows)]
use crate::windows::{Com, named, wide};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetUserDefaultLocaleName(name: *mut u16, room: i32) -> i32;
}

/// The user's language as Windows names it: their locale's (as
/// [`Locale::of_the_system`](crate::Locale::of_the_system) reads it, "de-AT"); English where
/// none.
#[cfg(windows)]
fn own_language() -> String {
    let mut name = [0u16; 85];
    // SAFETY: Windows writes at most the given room, NUL-terminated, and returns the count
    // including that.
    let wrote = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    let name = String::from_utf16_lossy(&name[..(wrote.max(1) - 1) as usize]);
    if name.is_empty() { "en-US".to_owned() } else { name }
}

/// Windows' checker for one language: an `ISpellChecker`. Called by vtable index: check (4),
/// suggest (5), add (6). Of check results, the next error (3); of a string list, the next
/// string (3).
#[cfg(windows)]
struct Speller(*mut Com);

#[cfg(windows)]
impl Speller {
    /// Windows' checker for the language of this name, from the spell checker factory, if it
    /// has one. (COM is initialized multithreaded on this thread: it is the checking thread,
    /// which has no window.)
    fn open(language: &str) -> Option<Speller> {
        const FACTORY: [u32; 4] = named(0x7AB3_6653, 0x1796, 0x484B, [0xBD, 0xFA, 0xE7, 0x4F, 0x1D, 0xB7, 0xC1, 0xDC]);
        const A_FACTORY: [u32; 4] = named(0x8E01_8A9D, 0x2415, 0x4677, [0xBF, 0x08, 0x79, 0x4E, 0xA6, 0x1F, 0x94, 0xBB]);
        let (name, mut factory, mut checker, mut known) = (wide(&language.replace('_', "-")), std::ptr::null_mut(), std::ptr::null_mut(), 0i32);
        // SAFETY: the factory is created, asked whether it supports the language (4) and for
        // its checker (5), and released. The name is NUL-terminated and outlives the calls.
        unsafe {
            crate::windows::sys::CoInitializeEx(std::ptr::null_mut(), 0);
            if crate::windows::sys::CoCreateInstance(&FACTORY, std::ptr::null_mut(), 1, &A_FACTORY, &mut factory) < 0 || factory.is_null() {
                return None;
            }
            Com::function::<unsafe extern "system" fn(*mut Com, *const u16, *mut i32) -> i32>(factory, 4)(factory, name.as_ptr(), &mut known);
            if known != 0 {
                Com::function::<unsafe extern "system" fn(*mut Com, *const u16, *mut *mut Com) -> i32>(factory, 5)(factory, name.as_ptr(), &mut checker);
            }
            Com::release(factory);
        }
        (!checker.is_null()).then(|| Speller(checker))
    }
}

#[cfg(windows)]
impl Check for Speller {
    fn wrong(&self, word: &str) -> bool {
        let (word, mut found, mut first) = (wide(word), std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: the checker is alive; its results are asked for their first error, and both
        // are released.
        unsafe {
            if Com::function::<unsafe extern "system" fn(*mut Com, *const u16, *mut *mut Com) -> i32>(self.0, 4)(self.0, word.as_ptr(), &mut found) < 0 || found.is_null() {
                return false;
            }
            let more = Com::function::<unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32>(found, 3)(found, &mut first);
            if !first.is_null() {
                Com::release(first);
            }
            Com::release(found);
            more == 0 && !first.is_null()
        }
    }

    fn offer(&mut self, word: &str) -> Vec<String> {
        let (word, mut list, mut offers) = (wide(word), std::ptr::null_mut(), Vec::new());
        // SAFETY: the checker is alive; its list is read a string at a time (each
        // NUL-terminated, in memory this function frees) and released.
        unsafe {
            if Com::function::<unsafe extern "system" fn(*mut Com, *const u16, *mut *mut Com) -> i32>(self.0, 5)(self.0, word.as_ptr(), &mut list) < 0 || list.is_null() {
                return offers;
            }
            while let Some(offer) = (offers.len() < OFFERED).then(|| next_of(list)).flatten() {
                offers.push(offer);
            }
            Com::release(list);
        }
        offers
    }

    fn add(&mut self, word: &str) {
        let word = wide(word);
        // SAFETY: the checker is alive, and the word is NUL-terminated.
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com, *const u16) -> i32>(self.0, 6)(self.0, word.as_ptr()) };
    }
}

/// The next string of an `IEnumString`; none at its end.
///
/// # Safety
/// `list` is such a list, and alive.
#[cfg(windows)]
unsafe fn next_of(list: *mut Com) -> Option<String> {
    let (mut one, mut got) = (std::ptr::null_mut::<u16>(), 0u32);
    // SAFETY: as the caller promises; the string comes NUL-terminated, in memory this function
    // frees.
    unsafe {
        if Com::function::<unsafe extern "system" fn(*mut Com, u32, *mut *mut u16, *mut u32) -> i32>(list, 3)(list, 1, &mut one, &mut got) != 0 || got == 0 || one.is_null() {
            return None;
        }
        let length = (0..).take_while(|at| *one.add(*at) != 0).count();
        let word = String::from_utf16_lossy(std::slice::from_raw_parts(one, length));
        crate::windows::sys::CoTaskMemFree(one.cast());
        Some(word)
    }
}

#[cfg(windows)]
impl Drop for Speller {
    fn drop(&mut self) {
        // SAFETY: the checker was referenced once and is not used after.
        unsafe { Com::release(self.0) };
    }
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
use crate::dynamic_library::library;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
use std::ffi::{c_char, c_int, c_void};

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
library!(Enchant, ["libenchant-2.so.2", "libenchant-2.so"], {
    fn enchant_broker_init() -> *mut c_void;
    fn enchant_broker_free(*mut c_void);
    fn enchant_broker_request_dict(*mut c_void, *const c_char) -> *mut c_void;
    fn enchant_broker_free_dict(*mut c_void, *mut c_void);
    fn enchant_dict_check(*mut c_void, *const c_char, isize) -> c_int;
    fn enchant_dict_suggest(*mut c_void, *const c_char, isize, *mut usize) -> *mut *mut c_char;
    fn enchant_dict_free_string_list(*mut c_void, *mut *mut c_char);
    fn enchant_dict_add(*mut c_void, *const c_char, isize);
});

/// The user's language as the environment names the messages locale (glibc's lookup), in
/// Enchant's form ("de_AT"); English where none, or "C".
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn own_language() -> String {
    let named = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|variable| std::env::var(variable).ok().filter(|name| !name.is_empty()));
    let name = named.as_deref().and_then(|name| name.split(':').next()).and_then(|name| name.split(['.', '@']).next()).unwrap_or("");
    if matches!(name, "" | "C" | "POSIX") { "en_US".to_owned() } else { name.to_owned() }
}

/// Enchant, loaded on the first checker request and never unloaded: loading registers GObject
/// types, and GLib, which stays, cannot register them twice (the program would abort). None
/// where missing. (Enchant is called from any thread: each checker has its own broker, used
/// only by the checking thread.)
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn enchant() -> Option<&'static Enchant> {
    static LOADED: std::sync::OnceLock<Option<Enchant>> = std::sync::OnceLock::new();
    LOADED.get_or_init(Enchant::open).as_ref()
}

/// Enchant's checker for one language: its broker, and the dictionary it found.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
struct Speller {
    enchant: &'static Enchant,
    broker: *mut c_void,
    dictionary: *mut c_void,
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
impl Speller {
    /// Enchant's checker for the language of this name, if Enchant exists and finds a
    /// dictionary.
    fn open(language: &str) -> Option<Speller> {
        let enchant = enchant()?;
        let name = std::ffi::CString::new(language.replace('-', "_")).ok()?;
        // SAFETY: a broker is created and asked for a dictionary by a NUL-terminated name;
        // freed again where it has none.
        unsafe {
            let broker = (enchant.enchant_broker_init)();
            let dictionary = if broker.is_null() { std::ptr::null_mut() } else { (enchant.enchant_broker_request_dict)(broker, name.as_ptr()) };
            if dictionary.is_null() && !broker.is_null() {
                (enchant.enchant_broker_free)(broker);
            }
            (!dictionary.is_null()).then(|| Speller { enchant, broker, dictionary })
        }
    }
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
impl Check for Speller {
    fn wrong(&self, word: &str) -> bool {
        // SAFETY: the dictionary is alive, and given the word's length.
        unsafe { (self.enchant.enchant_dict_check)(self.dictionary, word.as_ptr().cast(), word.len() as isize) > 0 }
    }

    fn offer(&mut self, word: &str) -> Vec<String> {
        let mut count = 0;
        // SAFETY: the dictionary is alive and given the word's length; it returns so many
        // NUL-terminated suggestions, freed back to it.
        unsafe {
            let list = (self.enchant.enchant_dict_suggest)(self.dictionary, word.as_ptr().cast(), word.len() as isize, &mut count);
            if list.is_null() {
                return Vec::new();
            }
            let offers = (0..count.min(OFFERED)).map(|at| std::ffi::CStr::from_ptr(*list.add(at)).to_string_lossy().into_owned()).collect();
            (self.enchant.enchant_dict_free_string_list)(self.dictionary, list);
            offers
        }
    }

    fn add(&mut self, word: &str) {
        // SAFETY: the dictionary is alive, and given the word's length.
        unsafe { (self.enchant.enchant_dict_add)(self.dictionary, word.as_ptr().cast(), word.len() as isize) };
    }
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
impl Drop for Speller {
    fn drop(&mut self) {
        // SAFETY: both came from this Enchant, which stays loaded, and are freed once.
        unsafe {
            (self.enchant.enchant_broker_free_dict)(self.broker, self.dictionary);
            (self.enchant.enchant_broker_free)(self.broker);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::accessibility::{AccessNode, Role};
    use crate::geom::pt;
    use crate::testing::Trial;
    use crate::{LocalClipboard, Size, Surface, text::BuiltInFont};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The words the stand-in checker knows: any other is misspelled.
    const KNOWN: [&str; 16] = ["the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "teh", "quiet", "quilt", "don't", "line", "word", "words", "tea"];
    /// Words added to the stand-in's dictionary, by any test.
    static ADDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// A checker knowing `KNOWN` and what was added, suggesting known words sharing the first
    /// two letters.
    struct StandIn;
    impl Check for StandIn {
        fn wrong(&self, word: &str) -> bool {
            !KNOWN.contains(&word) && !ADDED.lock().unwrap().iter().any(|added| added == word)
        }
        fn offer(&mut self, word: &str) -> Vec<String> {
            KNOWN.iter().filter(|known| known.get(..2) == word.get(..2)).map(|known| known.to_string()).collect()
        }
        fn add(&mut self, word: &str) {
            ADDED.lock().unwrap().push(word.to_owned());
        }
    }
    /// The stand-in, for any language but "none". (But "teh" is correct only in "en-teh".)
    pub(crate) fn stand_in(language: &str) -> Option<Box<dyn Check>> {
        (language != "none").then(|| Box::new(StandIn) as Box<dyn Check>)
    }

    /// A page of text, checked or not, on the interface's thread.
    struct Page {
        text: String,
        checked: bool,
    }
    fn page(app: &mut Page, ui: &mut Ui) {
        if app.checked {
            ui.spell_with(None, stand_in, true);
        }
        ui.text_area(&mut app.text, "Notes", (4, 8));
    }
    fn trial(text: &str) -> Trial<Page> {
        Trial::new(Page { text: text.into(), checked: true }, page)
    }

    /// Where the "Notes" box's text starts: 8 in, 6 down (built-in letters are 12 wide, lines
    /// 20 high).
    fn letters(trial: &Trial<Page>) -> Point {
        let r = trial.find(Role::TextBox, "Notes").rect;
        pt(r.x + 8.0, r.y + 6.0)
    }
    /// The window's wavy lines, each from its start to its end across, and its line index from
    /// 0.
    fn waves(trial: &Trial<Page>) -> Vec<[f32; 3]> {
        let at = letters(trial);
        wrong(&trial.ui).iter().map(|(_, wave)| [wave.x - at.x, wave.right() - at.x, ((wave.y - at.y) / 20.0).floor()]).collect()
    }
    /// The window's wavy lines, as the window keeps them.
    fn wrong(ui: &Ui) -> Vec<(u32, Rect)> {
        ui.asks.spelling.kept.as_ref().and_then(|kept| kept.downcast_ref::<Kept>()).map_or(Vec::new(), |kept| kept.wrong.clone())
    }
    /// Whether the pixel at `p` is reddish: drawn in the theme's red, at least partly.
    fn reddish(screen: &Surface, p: Point) -> bool {
        let pixel = screen.pixels[p.y as usize * screen.width + p.x as usize];
        let [r, g, b] = [pixel >> 16 & 0xff, pixel >> 8 & 0xff, pixel & 0xff].map(|channel| channel as i32);
        r - g > 40 && r - b > 30
    }
    /// The bounding rectangle of the window's reddish pixels, if any.
    fn red(screen: &Surface) -> Option<Rect> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..screen.pixels.len() / screen.width {
            for x in (0..screen.width).filter(|x| reddish(screen, pt(*x as f32, y as f32))) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        (x0 < x1).then(|| rect(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32))
    }

    #[test]
    fn words_spelt_wrong_are_marked_with_a_wavy_line_in_red() {
        let trial = trial("the quikc brown fox\njumps ovr the lazy dog");
        // "quikc" is the fifth to ninth letters of the first line, "ovr" the seventh to ninth
        // of the second: each 12 units wide.
        assert_eq!(waves(&trial), [[48.0, 108.0, 0.0], [72.0, 108.0, 1.0]]);
        // And they are drawn: in red, under those letters only.
        let at = letters(&trial);
        let marked = red(&trial.screen).expect("nothing is drawn in red");
        assert!(marked.x >= at.x + 47.0 && marked.right() <= at.x + 109.0, "{marked:?}");
        assert!(marked.y >= at.y + 15.0 && marked.bottom() <= at.y + 40.0, "under the letters of both lines: {marked:?}");
        for x in [48.0, 60.0, 100.0] {
            assert!((0..4).any(|down| reddish(&trial.screen, pt(at.x + x, at.y + 17.0 + down as f32))), "no wave at {x}");
        }
        // Unchecked, nothing is red.
        let mut plain = trial;
        plain.app.checked = false;
        plain.run();
        assert_eq!((red(&plain.screen), wrong(&plain.ui).len()), (None, 0));
    }

    #[test]
    fn a_screen_reader_is_told_the_words_spelt_wrong() {
        // The same text as a screen reader is told: each misspelling by character offsets
        // across both lines ("ovr" is 27th to 29th).
        let mut trial = trial("the quikc brown fox\njumps ovr the lazy dog");
        assert_eq!(trial.find(Role::TextBox, "Notes").misspelled, [[4, 9], [26, 29]]);
        // Corrected, nothing is; and in an unchecked box, nothing ever is.
        trial.app.text = "the quick brown fox".into();
        trial.run();
        assert!(trial.find(Role::TextBox, "Notes").misspelled.is_empty());
        trial.app = Page { text: "the quikc brown fox".into(), checked: false };
        trial.run();
        assert!(trial.find(Role::TextBox, "Notes").misspelled.is_empty());
        // In a box of twenty lines, of which eight show, a word spelt wrong on the last is
        // told too, though not marked.
        let mut text = "the quick brown fox\n".repeat(19);
        text.push_str("the lazy dgo");
        let trial = self::trial(&text);
        let start = text.chars().count() as u32 - 3;
        assert_eq!((&trial.find(Role::TextBox, "Notes").misspelled[..], waves(&trial)), (&[[start, start + 3]][..], vec![]));
    }

    #[test]
    fn what_is_and_is_not_a_word() {
        let mut found = Vec::new();
        let line = "Don't e.g. x2 snake_case me@here.org /usr/bin l'eau café\u{301} ok. end:";
        words(line, &mut found);
        let found: Vec<&str> = found.into_iter().map(|word| &line[word]).collect();
        assert_eq!(found, ["Don't", "l'eau", "café\u{301}", "ok", "end"]);
    }

    #[test]
    fn the_word_being_typed_is_marked_once_it_is_ended() {
        let mut trial = trial("");
        trial.press(Role::TextBox, "Notes");
        trial.type_text("the");
        trial.type_text(" ");
        trial.type_text("qu");
        trial.type_text("ikc");
        assert!(waves(&trial).is_empty(), "being typed");
        trial.type_text(" ");
        assert_eq!(waves(&trial), [[48.0, 108.0, 0.0]], "ended by a space");
        // Ended by leaving, too; and one corrected while typing is not marked.
        trial.type_text("browm");
        assert_eq!(waves(&trial).len(), 1);
        trial.key("Home");
        assert_eq!(waves(&trial), [[48.0, 108.0, 0.0], [120.0, 180.0, 0.0]], "ended by going home");
        trial.key("End");
        trial.key("Backspace");
        assert_eq!(waves(&trial).len(), 1, "being written again");
        trial.type_text("n");
        trial.key("Left");
        assert_eq!(waves(&trial), [[48.0, 108.0, 0.0]], "put right");
        // A box no longer being typed in has all its words checked.
        trial.type_text("x");
        assert_eq!(waves(&trial).len(), 1);
        trial.click(pt(320.0, 400.0));
        assert_eq!(waves(&trial).len(), 2);
    }

    /// The open menu's rows, as a screen reader is told.
    fn menu(trial: &Trial<Page>) -> Vec<String> {
        trial.visible_access_nodes().iter().filter(|said| said.role == Role::MenuItem).map(|said| said.name.clone()).collect()
    }

    #[test]
    fn the_menu_offers_the_right_words_first_and_puts_one_in() {
        let mut trial = trial("the quikc fox");
        // Right-click on the misspelling: suggestions, then add and ignore, then the usual
        // rows.
        let at = letters(&trial);
        trial.right_click(pt(at.x + 70.0, at.y + 10.0));
        assert_eq!(menu(&trial), ["quick", "quiet", "quilt", "Add to dictionary", "Ignore", "Cut", "Copy", "Paste", "Select all"]);
        trial.press(Role::MenuItem, "quiet");
        assert_eq!((trial.app.text.as_str(), waves(&trial).len()), ("the quiet fox", 0));
        // As one change, undone as one.
        trial.key("Ctrl+Z");
        assert_eq!(trial.app.text, "the quikc fox");
        // Over a correct word, the menu is as always.
        trial.key("Escape");
        trial.right_click(pt(at.x + 20.0, at.y + 10.0));
        assert_eq!(menu(&trial), ["Cut", "Copy", "Paste", "Select all"]);
        trial.key("Escape");
        // By the menu key, the word at the cursor.
        trial.press(Role::TextBox, "Notes");
        trial.key("Home");
        trial.key("Ctrl+Right");
        trial.key("Right");
        trial.key("Menu");
        assert_eq!(menu(&trial)[..3], ["quick", "quiet", "quilt"]);
        trial.key("Enter");
        assert_eq!(trial.app.text, "the quick fox", "the first row has the keyboard");
    }

    /// Two text boxes, each checked.
    struct Two {
        one: String,
        other: String,
    }
    fn two(app: &mut Two, ui: &mut Ui) {
        ui.spell_with(None, stand_in, true);
        ui.text_area(&mut app.one, "One", (2, 2));
        ui.spell_with(None, stand_in, true);
        ui.text_box(&mut app.other, "Other");
    }

    #[test]
    fn a_word_ignored_or_added_is_spelt_right_everywhere() {
        let mut trial = Trial::new(Two { one: "the frobnicate fox".into(), other: "frobnicate the zorkmid".into() }, two);
        let marked = |trial: &Trial<Two>| wrong(&trial.ui).len();
        assert_eq!(marked(&trial), 3);
        let at = trial.find(Role::TextBox, "One").rect;
        trial.right_click(pt(at.x + 8.0 + 60.0, at.y + 16.0));
        trial.press(Role::MenuItem, "Ignore");
        assert_eq!(marked(&trial), 1, "ignored in both boxes");
        let at = trial.find(Role::TextBox, "Other").rect;
        trial.right_click(pt(at.x + 8.0 + 16.0 * 12.0, at.y + at.h / 2.0));
        trial.press(Role::MenuItem, "Add to dictionary");
        assert_eq!(marked(&trial), 0);
        assert!(ADDED.lock().unwrap().iter().any(|added| added == "zorkmid"), "added to the checker's dictionary");
        assert_eq!((trial.app.one.as_str(), trial.app.other.as_str()), ("the frobnicate fox", "frobnicate the zorkmid"));
    }

    #[test]
    fn a_box_that_hides_its_text_or_shows_its_hint_is_not_checked_nor_is_a_language_with_no_checker() {
        struct Boxes(String, String, String);
        let mut trial = Trial::new(Boxes(String::new(), "hunter2 zorkmid".into(), "zorkmid".into()), |app, ui| {
            ui.spell_with(None, stand_in, true);
            ui.text_box(&mut app.0, "Hint wrods");
            ui.spell_with(None, stand_in, true);
            ui.password_box(&mut app.1, "Secret");
            ui.spell_with(Some("none"), stand_in, true);
            ui.text_box(&mut app.2, "Elsewhere");
        });
        trial.run();
        assert_eq!(wrong(&trial.ui).len(), 0);
    }

    #[test]
    fn a_program_that_never_asks_carries_nothing_of_it() {
        let mut trial = Trial::new(Page { text: "the quikc fox".into(), checked: false }, page);
        trial.right_click(trial.center_of(Role::TextBox, "Notes"));
        assert!(trial.ui.asks.spelling.hooks().is_none());
        assert_eq!(menu(&trial), ["Cut", "Copy", "Paste", "Select all"]);
    }

    /// How many words the counting checker was asked.
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    /// A checker that counts requests and calls every word misspelled.
    struct Counting;
    impl Check for Counting {
        fn wrong(&self, _: &str) -> bool {
            ASKED.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn offer(&mut self, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn add(&mut self, _: &str) {}
    }

    /// A box of a hundred thousand lines, each with three words of its own ("ab ab ab" for the
    /// first, "ac ac ac", and so on): line `n`'s words in letters.
    fn long_text() -> String {
        let word = |n: usize| (0..4).map(|place| char::from(b'a' + (n / 26usize.pow(place) % 26) as u8)).collect::<String>();
        (0..100_000).map(|n| format!("{0} {0} {0}\n", word(n))).collect()
    }
    struct Long {
        text: String,
        checked: bool,
    }
    fn long(app: &mut Long, ui: &mut Ui) {
        if app.checked {
            ui.spell_with(None, |_| Some(Box::new(Counting)), true);
        }
        ui.text_page(&mut app.text, "Long");
    }

    #[test]
    fn only_the_words_in_view_are_checked_and_each_only_once() {
        let mut trial = Trial::new(Long { text: long_text(), checked: true }, long);
        // The page shows 23 lines (and part of one more): one word each, the rest repeats.
        let asked = ASKED.load(Ordering::SeqCst);
        assert!((23..=25).contains(&asked), "{asked} words were asked about");
        assert_eq!(wrong(&trial.ui).len(), asked * 3);
        // Scrolled a little, only the lines that came into view are checked.
        trial.wheel(trial.center_of(Role::TextBox, "Long"), 0.0, 200.0);
        let more = ASKED.load(Ordering::SeqCst) - asked;
        assert!((9..=11).contains(&more), "{more} more words were asked about");
        // And back up, nothing.
        trial.wheel(trial.center_of(Role::TextBox, "Long"), 0.0, -200.0);
        assert_eq!(ASKED.load(Ordering::SeqCst) - asked, more);
    }

    #[test]
    fn on_a_thread_of_its_own_the_marks_come_when_the_checker_has_answered() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 400.0, h: 200.0 }, 1.0);
        let mut text = String::from("the quikc fox");
        let mut describe = |ui: &mut Ui| {
            ui.spell_with(None, stand_in, false);
            ui.text_area(&mut text, "", (2, 2));
        };
        ui.run(&mut describe);
        assert_eq!(wrong(&ui).len(), 0, "not answered yet");
        let mut tries = 0;
        while wrong(&ui).is_empty() && tries < 200 {
            std::thread::sleep(Duration::from_millis(5));
            ui.run(&mut describe);
            tries += 1;
        }
        assert_eq!(wrong(&ui).len(), 1);
        // The thread ends with the window.
        let Some(Checking::Away(shared)) = kept(&mut ui.asks.spelling).map(|kept| &kept.checking) else { panic!("checked here") };
        let thread = Arc::downgrade(shared);
        drop(ui);
        let mut tries = 0;
        while thread.strong_count() > 0 && tries < 200 {
            std::thread::sleep(Duration::from_millis(5));
            tries += 1;
        }
        assert_eq!(thread.strong_count(), 0, "the thread still holds what it shared");
    }

    /// A box of a hundred thousand lines, checked or not, by the stand-in on its own thread:
    /// marks arrive as in a window.
    fn long_away(app: &mut Long, ui: &mut Ui) {
        if app.checked {
            ui.spell_with(None, stand_in, false);
        }
        ui.text_page(&mut app.text, "Long");
    }

    /// How long typing a letter takes in a box of a hundred thousand lines, unchecked and
    /// checked: alternating, a letter at a time (so other load falls on both alike), each the
    /// minimum of many tries, in microseconds.
    fn typing_takes() -> [f64; 2] {
        let mut boxes = [false, true].map(|checked| (Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default())), Long { text: long_text(), checked }));
        for (ui, app) in &mut boxes {
            ui.resize(Size { w: 640.0, h: 480.0 }, 1.0);
            ui.run(&mut |ui| long_away(app, ui));
            ui.set_focus(ui.visible_access_nodes().iter().find(|said| said.role == Role::TextBox).map_or(0, |said| said.id));
            ui.run(&mut |ui| long_away(app, ui));
        }
        let mut least = [f64::MAX; 2];
        let letters = if cfg!(debug_assertions) { "ab" } else { "abcdefghijklmnopqrstuvwxyzabcdefghijklmn" };
        for letter in letters.chars() {
            for (at, (ui, app)) in boxes.iter_mut().enumerate() {
                std::thread::sleep(Duration::from_millis(2));
                ui.event(crate::Event::Text(letter.to_string()));
                let started = std::time::Instant::now();
                ui.run(&mut |ui| long_away(app, ui));
                least[at] = least[at].min(started.elapsed().as_secs_f64() * 1e6);
            }
        }
        least
    }

    #[test]
    fn how_long_typing_takes_in_a_long_box_checked_and_not() {
        // (In a release build this measures; in a test build it only runs both.)
        let [off, on] = typing_takes();
        println!("typing a letter in a box of 100,000 lines: {off:.0} us unchecked, {on:.0} us checked");
        if !cfg!(debug_assertions) {
            assert!(on < off * 1.1 + 50.0, "{on:.0} us checked against {off:.0} us");
        }
    }

    /// Misspelled English words by the system checker, if it has this language: its verdicts,
    /// and its suggestions for the first.
    #[cfg_attr(any(target_vendor = "apple", target_os = "android"), allow(dead_code))]
    fn system_says(language: &str, words: &[&str]) -> Option<(Vec<bool>, Vec<String>)> {
        let mut checker = system(language)?;
        let wrong = words.iter().map(|word| checker.wrong(word)).collect();
        Some((wrong, checker.offer(words[0])))
    }

    #[cfg(windows)]
    #[test]
    fn windows_checks_english_and_offers_the_right_words() {
        // (On a thread with COM initialized as the checking thread has it.)
        std::thread::spawn(|| {
            let (wrong, offered) = system_says("en-US", &["helo", "hello", "recieve", "don't", "colour", "naïve"]).expect("Windows has a checker for English");
            assert_eq!(wrong, [true, false, true, false, true, false]);
            assert!(offered.contains(&"hello".to_owned()) && offered.len() <= OFFERED, "{offered:?}");
            // Another installed language, in which "colour" is correct; and one not installed.
            let (wrong, _) = system_says("en_CA", &["colour", "helo"]).expect("Windows has a checker for Canadian English");
            assert_eq!(wrong, [false, true]);
            assert!(system("xx-YY").is_none());
            // And the user's own, whatever it is.
            assert!(!own_language().is_empty());
        })
        .join()
        .unwrap();
    }

    /// Where Enchant is not installed but unpacked into a folder (`TAUT_ENCHANT_PREFIX`): it is
    /// told to find its modules there, and kept open so it remembers.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn enchant_brought() {
        use std::os::unix::ffi::OsStrExt;
        let Some(prefix) = std::env::var_os("TAUT_ENCHANT_PREFIX") else { return };
        library!(Prefix, ["libenchant-2.so.2"], {
            fn enchant_set_prefix_dir(*const c_char);
        });
        let prefix = std::ffi::CString::new(prefix.as_bytes()).unwrap();
        let Some(enchant) = Prefix::open() else { return };
        // SAFETY: as declared, with a NUL-terminated name.
        unsafe { (enchant.enchant_set_prefix_dir)(prefix.as_ptr()) };
        std::mem::forget(enchant);
    }

    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn enchant_checks_english_and_offers_the_right_words() {
        enchant_brought();
        let Some((wrong, offered)) = system_says("en_US", &["helo", "hello", "recieve", "don't", "colour"]) else {
            return eprintln!("(no Enchant here, or no English dictionary for it: nothing to check)");
        };
        assert_eq!(wrong, [true, false, true, false, true]);
        assert!(offered.contains(&"hello".to_owned()) && offered.len() <= OFFERED, "{offered:?}");
        assert!(system("xx_YY").is_none());
        assert!(!own_language().contains('.'));
        match system_says("de-DE", &["Haus", "Huas", "Straße"]) {
            Some((wrong, offered)) => {
                assert_eq!(wrong, [false, true, false]);
                assert!(offered.iter().any(|offer| offer.starts_with("Ha")), "{offered:?}");
            }
            None => eprintln!("(no German dictionary for Enchant here)"),
        }
    }

    /// Adds a word to Enchant's personal dictionary: in a test-only folder Enchant takes as the
    /// user's (`ENCHANT_CONFIG_DIR`), in a separate process. Never the user's real one.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn a_word_added_goes_to_the_persons_own_dictionary() {
        if std::env::var_os("TAUT_SPELL_FOLDER").is_some() {
            return;
        }
        let folder = std::env::temp_dir().join(format!("taut-spell-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // (With the dictionaries given to Enchant for this run, if any.)
        let given = std::env::var_os("ENCHANT_CONFIG_DIR").map(|given| std::path::PathBuf::from(given).join("hunspell"));
        if let Some(Ok(files)) = given.map(std::fs::read_dir) {
            std::fs::create_dir_all(folder.join("hunspell")).unwrap();
            files.flatten().for_each(|file| _ = std::fs::copy(file.path(), folder.join("hunspell").join(file.file_name())));
        }
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "spell::tests::adding_a_word_in_a_folder_of_its_own", "--ignored", "--nocapture"])
            .env("TAUT_SPELL_FOLDER", &folder)
            .env("ENCHANT_CONFIG_DIR", &folder)
            .env("XDG_CONFIG_HOME", &folder)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&child.stdout).into_owned() + &String::from_utf8_lossy(&child.stderr);
        let added = std::fs::read_to_string(folder.join("en_US.dic")).unwrap_or_default();
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(child.status.success(), "{said}");
        if !said.contains("no Enchant") {
            assert!(added.lines().any(|line| line == "zorkmidian"), "the dictionary has: {added:?}\n{said}");
        }
    }
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    #[ignore]
    fn adding_a_word_in_a_folder_of_its_own() {
        let Some(folder) = std::env::var_os("TAUT_SPELL_FOLDER") else { return };
        assert_eq!(std::env::var_os("ENCHANT_CONFIG_DIR"), Some(folder));
        enchant_brought();
        let Some(mut checker) = system("en_US") else { return eprintln!("(no Enchant here)") };
        assert!(checker.wrong("zorkmidian"));
        checker.add("zorkmidian");
        assert!(!checker.wrong("zorkmidian"));
        drop(checker);
        let again = system("en_US").unwrap();
        assert!(!again.wrong("zorkmidian"), "kept for good");
    }

    /// The system's text, where available.
    fn system_text() -> Option<Box<dyn crate::TextSystem>> {
        crate::platform::SystemText::new().map(|text| Box::new(text) as Box<dyn crate::TextSystem>)
    }

    /// A window of system text checked by the system checker, as a user sees it: rendered to a
    /// picture, saved where `TAUT_SPELL_SHOTS` says, at each scale. (Run by hand, to look at.)
    #[test]
    #[ignore]
    fn a_picture_of_the_marks() {
        let Some(folder) = std::env::var_os("TAUT_SPELL_SHOTS") else { return };
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        enchant_brought();
        for (scale, name) in [(1.0, "marks-100.png"), (1.5, "marks-150.png"), (2.0, "marks-200.png")] {
            let Some(text) = system_text() else { return };
            let mut ui = Ui::new(text, Box::new(LocalClipboard::default()));
            ui.theme = crate::Theme::light();
            ui.resize(Size { w: 420.0, h: 150.0 }, scale);
            let (mut letter, mut name_box) = (String::from("Thsi is a smiple tset of the speling checker.\nIt marks words spelt wrnog, and leaves the right ones be."), String::from("Recieve"));
            let mut describe = |ui: &mut Ui| {
                ui.column().pad(10.0).gap(8.0).open();
                ui.check_spelling(Some("en-US"));
                ui.text_box(&mut name_box, "Name");
                ui.check_spelling(Some("en-US"));
                ui.text_area(&mut letter, "Letter", (3, 3));
                ui.close();
            };
            ui.run(&mut describe);
            for _ in 0..400 {
                if wrong(&ui).len() >= 6 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
                ui.run(&mut describe);
            }
            let screen = Surface::draw(&mut ui, 64);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(std::path::Path::new(&folder).join(name), screen.to_png()).unwrap();
            println!("{name}: {} words marked", wrong(&ui).len());
        }
    }

    /// A German stand-in, for "de": knows only "der", "Hund", "und".
    struct German;
    impl Check for German {
        fn wrong(&self, word: &str) -> bool {
            !["der", "Hund", "und"].contains(&word)
        }
        fn offer(&mut self, word: &str) -> Vec<String> {
            if word.starts_with('H') { vec!["Hund".to_owned()] } else { Vec::new() }
        }
        fn add(&mut self, _: &str) {}
    }
    /// The stand-ins by language: German for "de", none for "none", English for any other.
    fn by_language(language: &str) -> Option<Box<dyn Check>> {
        match language {
            "de" => Some(Box::new(German)),
            _ => stand_in(language),
        }
    }

    #[test]
    fn a_box_is_checked_in_several_languages_at_once() {
        let mut trial = Trial::new(Page { text: "the Hund und the fox frobnicate".into(), checked: true }, |app, ui| {
            ui.spell_with(Some("en, de"), |languages| several(languages, by_language), true);
            ui.text_area(&mut app.text, "Notes", (4, 8));
        });
        trial.run();
        // Only the word neither language has.
        assert_eq!(trial.find(Role::TextBox, "Notes").misspelled, [[21, 31]]);
        // Suggestions from each language in turn, each once.
        let mut both = several("de,en", by_language).unwrap();
        assert_eq!(both.offer("Hunf"), ["Hund"]);
        assert_eq!(both.offer("qux"), ["quick", "quiet", "quilt"]);
        assert!(!both.wrong("der") && !both.wrong("fox") && both.wrong("xyz"));
        // A language with no checker counts for nothing; with none at all, nothing is checked.
        assert!(several("none,de", by_language).is_some_and(|one| one.wrong("fox") && !one.wrong("Hund")));
        assert!(several("none, none", by_language).is_none());
    }

    #[test]
    fn a_word_written_twice_is_marked_and_its_menu_offers_it_once() {
        let mut trial = trial("the the fox. the the dog, fox fox2 dog\ndog");
        let kept = Kept::new(stand_in, true);
        let twice = |text: &str, word: Range<usize>| kept.twice(text, 0, word);
        assert_eq!([twice("The the", 4..7), twice("lathe the", 6..9), twice("the\tthe", 4..7), twice("the.the", 4..7), twice("the", 0..3)], [Some(0), None, Some(0), None, None]);
        // "the the" twice, not across a line or a number, nor a word ended by a
        // point.
        assert_eq!(waves(&trial), [[48.0, 84.0, 0.0], [204.0, 240.0, 0.0]]);
        let at = letters(&trial);
        trial.right_click(pt(at.x + 60.0, at.y + 10.0));
        assert_eq!(menu(&trial), ["the", "Ignore", "Cut", "Copy", "Paste", "Select all"], "no adding a right word to the dictionary");
        trial.press(Role::MenuItem, "the");
        assert_eq!((trial.app.text.as_str(), waves(&trial)), ("the fox. the the dog, fox fox2 dog\ndog", vec![[156.0, 192.0, 0.0]]));
        trial.key("Ctrl+Z");
        assert_eq!(trial.app.text, "the the fox. the the dog, fox fox2 dog\ndog", "undone as one");
        // Ignored, it is no longer marked written twice.
        trial.key("Escape");
        trial.right_click(pt(at.x + 210.0, at.y + 10.0));
        trial.press(Role::MenuItem, "Ignore");
        assert!(waves(&trial).is_empty());
        // A box in a language with no checker marks nothing, twice or not.
        let mut none = Trial::new(Page { text: "the the".into(), checked: false }, |app, ui| {
            ui.spell_with(Some("none"), stand_in, true);
            ui.text_area(&mut app.text, "Notes", (4, 8));
        });
        none.run();
        assert!(waves(&none).is_empty());
    }

    /// Windows' own checkers as one: "colour" is right in Canadian English, "color" in American.
    #[cfg(windows)]
    #[test]
    fn windows_checks_two_languages_as_one() {
        std::thread::spawn(|| {
            let (wrong, _) = system_says("en-US,en-CA", &["colour", "color", "helo"]).expect("Windows has checkers for both");
            assert_eq!(wrong, [false, false, true]);
            let (wrong, _) = system_says("en-US, xx-YY", &["colour", "color"]).expect("the one there is");
            assert_eq!(wrong, [true, false]);
        })
        .join()
        .unwrap();
    }

    /// Enchant's checkers as one, where it has English and German dictionaries.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn enchant_checks_two_languages_as_one() {
        enchant_brought();
        let Some((wrong, offered)) = system_says("en_US,de_DE", &["Hund", "dog", "Huas", "helo"]) else { return eprintln!("(no Enchant here)") };
        if system("de_DE").is_none() {
            return eprintln!("(no German dictionary for Enchant here)");
        }
        assert_eq!(wrong, [false, false, true, true]);
        assert!(offered.len() <= OFFERED);
    }

    /// A code box, checked or not, in the stand-in or the system checker's `language`.
    struct Code {
        code: crate::CodeEditor,
        checked: Option<Opens>,
        language: Option<&'static str>,
        here: bool,
    }
    fn code(app: &mut Code, ui: &mut Ui) {
        if let Some(opens) = app.checked {
            ui.spell_with(app.language, opens, app.here);
        }
        ui.code_editor(&mut app.code, "Source");
    }
    fn code_trial(text: &str, rules: &'static crate::Syntax) -> Trial<Code> {
        Trial::new(Code { code: crate::CodeEditor::new(text, rules), checked: Some(stand_in), language: None, here: true }, code)
    }
    /// The misspellings a screen reader is told of in the code box, as the words.
    fn told_wrong(trial: &Trial<Code>) -> Vec<String> {
        let source = trial.find(Role::TextBox, "Source");
        let letters: Vec<char> = source.value.chars().collect();
        source.misspelled.iter().map(|[from, to]| letters[*from as usize..*to as usize].iter().collect()).collect()
    }

    #[test]
    fn a_code_box_has_its_comments_and_strings_checked_by_its_languages_rules() {
        // Rust: a line comment, a block comment and a string are checked; names, keywords and
        // what lies between are not, misspelt or not.
        let mut trial = code_trial("// the quikc fox\nlet quikc = \"the lazy dgo\";\n/* jumps ovr */\n", &crate::Syntax::RUST);
        assert_eq!(told_wrong(&trial), ["quikc", "dgo", "ovr"]);
        assert_eq!(wrong(&trial.ui).len(), 3, "three wavy lines");
        assert!(red(&trial.screen).is_some(), "drawn");
        // Python's comments start with `#`; its strings may be single-quoted.
        let mut python = code_trial("# a dgo\nx = 'quikc' # ovr\n", &crate::Syntax::PYTHON);
        python.run();
        assert_eq!(told_wrong(&python), ["dgo", "quikc", "ovr"]);
        // Not asked, nothing is checked.
        trial.app.checked = None;
        trial.run();
        assert_eq!((told_wrong(&trial).len(), wrong(&trial.ui).len()), (0, 0));
    }

    #[test]
    fn a_code_boxs_menu_puts_a_right_word_into_a_comment() {
        let mut trial = code_trial("let x = 1; // the quikc fox\n", &crate::Syntax::RUST);
        let (_, wave) = wrong(&trial.ui)[0];
        trial.right_click(pt(wave.x + wave.w / 2.0, wave.y - 6.0));
        let offered: Vec<String> = trial.visible_access_nodes().iter().filter(|said| said.role == Role::MenuItem).map(|said| said.name.clone()).collect();
        assert_eq!(offered[..5], ["quick", "quiet", "quilt", "Add to dictionary", "Ignore"]);
        trial.press(Role::MenuItem, "quiet");
        assert_eq!((trial.app.code.text(), wrong(&trial.ui).len()), ("let x = 1; // the quiet fox\n", 0));
        // As one change, undone as one.
        trial.key("Ctrl+Z");
        assert_eq!(trial.app.code.text(), "let x = 1; // the quikc fox\n");
    }

    #[test]
    fn a_word_being_typed_in_a_comment_is_marked_once_it_is_ended() {
        let mut trial = code_trial("", &crate::Syntax::RUST);
        trial.press(Role::TextBox, "Source");
        trial.type_text("// the quikc");
        assert!(told_wrong(&trial).is_empty(), "still being written");
        trial.type_text(" ");
        assert_eq!(told_wrong(&trial), ["quikc"]);
        // A word moved off by the cursor alone is ended too.
        trial.type_text("ovr");
        trial.key("Home");
        assert_eq!(told_wrong(&trial), ["quikc", "ovr"]);
    }

    /// The misspellings the system checker for `language` finds in a code box, once it has
    /// answered (on its own thread, as in a window, which it wakes: here, runs until then).
    #[cfg_attr(any(target_vendor = "apple", target_os = "android"), allow(dead_code))]
    fn system_marks_in_code(language: &'static str) -> Vec<String> {
        let mut app = Code { code: crate::CodeEditor::new("// helo world\nlet helo = \"recieve it\";\n", &crate::Syntax::RUST), checked: Some(system), language: Some(language), here: false };
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 400.0, h: 200.0 }, 1.0);
        ui.set_screen_reader(true);
        let told = |ui: &Ui| {
            let source = ui.access_nodes().iter().find(|node| node.role == Role::TextBox).cloned().unwrap_or_else(|| AccessNode::plain(0, Role::TextBox, String::new(), Rect::EMPTY, None));
            let letters: Vec<char> = source.value.chars().collect();
            source.misspelled.iter().map(|[from, to]| letters[*from as usize..*to as usize].iter().collect::<String>()).collect::<Vec<_>>()
        };
        let start = std::time::Instant::now();
        let mut runs = 0;
        for _ in 0..400 {
            runs += 1;
            ui.run(&mut |ui| code(&mut app, ui));
            if told(&ui).len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let line = format!("probe: {language} marks {:?} after {:?} in {runs} runs\n", told(&ui), start.elapsed());
        eprint!("{line}");
        if let Ok(path) = std::env::var("TAUT_PROBE") {
            use std::io::Write;
            _ = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut file| file.write_all(line.as_bytes()));
        }
        told(&ui)
    }

    #[cfg(windows)]
    #[test]
    fn windows_checks_a_code_boxs_comments_and_strings() {
        let start = std::time::Instant::now();
        let marks = system_marks_in_code("en-US");
        if marks != ["helo", "recieve"] {
            let checker = system("en-US");
            let now = checker.as_ref().map(|checker| (checker.wrong("helo"), checker.wrong("recieve"), checker.wrong("world")));
            panic!("marks {marks:?} after {:?}; a checker opened now: {}, says helo/recieve/world wrong: {now:?}", start.elapsed(), checker.is_some());
        }
    }

    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn enchant_checks_a_code_boxs_comments_and_strings() {
        enchant_brought();
        if system("en_US").is_none() {
            return eprintln!("(no Enchant here, or no English dictionary for it: nothing to check)");
        }
        assert_eq!(system_marks_in_code("en_US"), ["helo", "recieve"]);
    }
}
