//! Spell checking on macOS: the system's checker, `NSSpellChecker`, asked from the checking
//! thread (see `spell.rs`), with the user's dictionary.
//!
//! It names languages "en", "en_GB", "de": a name as the app gives it ("en-US") is matched
//! whole, then by its language alone.
//!
//! Words are checked by request, as AppKit's own text views check as you type: the checker
//! answers on a queue of its own, finding nothing where its server does not answer. (Its
//! synchronous check can throw `NSXPCSpellServerTimeoutException` when the server's connection
//! times out, which past Rust's frames ends the program.)
use crate::macos_sys::*;
use crate::spell::{Check, OFFERED};
use std::ffi::c_void;
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// The text checking option giving the orthography (script and language) to check in.
    static NSTextCheckingOrthographyKey: Obj;
}

/// `NSTextCheckingTypeSpelling`.
const SPELLING: u64 = 1 << 1;
/// How long a request's answer is waited for. (The checker gives up on a server that does not
/// answer after about a second, and answers that it found nothing.)
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The checker's answers by request number: whether it found a misspelling.
static ANSWERS: Mutex<Vec<(isize, bool)>> = Mutex::new(Vec::new());
static ANSWERED: Condvar = Condvar::new();

/// The checker for one language, by the name `NSSpellChecker` has for it.
pub(crate) struct Speller(String);

/// The shared checker.
fn checker() -> Obj {
    // SAFETY: a plain question; the checker lives as long as the program.
    unsafe { send(class(c"NSSpellChecker"), c"sharedSpellChecker", ()) }
}

/// The checker answered request `sequence` with these results.
extern "C" fn answered(_: *const Block<()>, sequence: isize, results: Obj, _: Obj, _: isize) {
    let found = !array_items(results).is_empty();
    ANSWERS.lock().unwrap_or_else(PoisonError::into_inner).push((sequence, found));
    ANSWERED.notify_all();
}

/// The block every request is answered through, made once.
fn answer_block() -> *const Block<()> {
    static MADE: OnceLock<usize> = OnceLock::new();
    *MADE.get_or_init(|| Block::leaked(answered as *const c_void, ()) as *const Block<()> as usize) as *const Block<()>
}

/// The answer to request `sequence`: whether it found a misspelling; none if it did not answer
/// in `ANSWER_WAIT`. Answers to earlier requests, given up on, are dropped.
fn answer_to(sequence: isize) -> Option<bool> {
    let answers = ANSWERS.lock().unwrap_or_else(PoisonError::into_inner);
    let waited = ANSWERED.wait_timeout_while(answers, ANSWER_WAIT, |answers| answers.iter().all(|(of, _)| *of != sequence));
    let (mut answers, _) = waited.unwrap_or_else(PoisonError::into_inner);
    let found = answers.iter().find(|(of, _)| *of == sequence).map(|(_, found)| *found);
    answers.retain(|(of, _)| *of > sequence);
    found
}

impl Speller {
    /// The checker for the language of this name ("en-US", "de_DE", "fr"), if the system has
    /// one: the name whole, else its language.
    pub(crate) fn open(name: &str) -> Option<Speller> {
        let _pool = Pool::new();
        // SAFETY: a plain question; an array of strings comes back.
        let known: Vec<String> = array_items(unsafe { send(checker(), c"availableLanguages", ()) }).into_iter().filter_map(rust_string).collect();
        let name = name.replace('-', "_");
        let language = name.split('_').next().unwrap_or_default();
        let found = known.iter().find(|known| **known == name).or_else(|| known.iter().find(|known| *known == language))?;
        Some(Speller(found.clone()))
    }
}

impl Check for Speller {
    fn wrong(&self, word: &str) -> bool {
        let _pool = Pool::new();
        let whole = NSRange { location: 0, length: word.encode_utf16().count() };
        // SAFETY: the language's orthography in a dictionary of options, both autoreleased; the
        // key is AppKit's.
        let options: Obj = unsafe {
            let orthography: Obj = send(class(c"NSOrthography"), c"defaultOrthographyForLanguage:", (ns_string(&self.0),));
            send(class(c"NSDictionary"), c"dictionaryWithObject:forKey:", (orthography, NSTextCheckingOrthographyKey))
        };
        // SAFETY: the word, its whole range in UTF-16 units, spelling only, the options, no
        // document (0), and a block that lives for good.
        let sequence: isize =
            unsafe { send(checker(), c"requestCheckingOfString:range:types:options:inSpellDocumentWithTag:completionHandler:", (ns_string(word), whole, SPELLING, options, 0isize, answer_block())) };
        answer_to(sequence).unwrap_or(false)
    }

    fn offer(&mut self, word: &str) -> Vec<String> {
        let _pool = Pool::new();
        let whole = NSRange { location: 0, length: word.encode_utf16().count() };
        // SAFETY: the range covers the word's UTF-16 units; an array of strings (or nil) comes
        // back.
        let offers: Obj = unsafe { send(checker(), c"guessesForWordRange:inString:language:inSpellDocumentWithTag:", (whole, ns_string(word), ns_string(&self.0), 0isize)) };
        array_items(offers).into_iter().filter_map(rust_string).take(OFFERED).collect()
    }

    fn add(&mut self, word: &str) {
        let _pool = Pool::new();
        // SAFETY: the word is a string; the checker keeps it in the user's dictionary.
        unsafe { send::<()>(checker(), c"learnWord:", (ns_string(word),)) };
    }
}

/// The user's language: the first of their preferred languages ("en-US"); English where none.
pub(crate) fn own_language() -> String {
    let _pool = Pool::new();
    // SAFETY: a plain question; an array of strings comes back.
    let first = array_items(unsafe { send(class(c"NSLocale"), c"preferredLanguages", ()) }).first().copied().and_then(rust_string);
    first.filter(|name| !name.is_empty()).unwrap_or_else(|| "en-US".to_owned())
}

#[cfg(test)]
mod tests {
    use crate::spell::{OFFERED, system};

    #[test]
    fn apples_checker_checks_english_and_offers_the_right_words() {
        // (From a thread of its own, as the checking thread asks.)
        std::thread::spawn(|| {
            let mut checker = system("en-US").expect("macOS has a checker for English");
            let wrong: Vec<bool> = ["helo", "hello", "recieve", "don't", "naïve"].iter().map(|word| checker.wrong(word)).collect();
            assert_eq!(wrong, [true, false, true, false, false]);
            let offered = checker.offer("recieve");
            assert!(offered.contains(&"receive".to_owned()) && offered.len() <= OFFERED, "{offered:?}");
            // British English, in which "colour" is right; German; and a language there is none for.
            let british = system("en-GB").expect("macOS has a checker for British English");
            assert!(!british.wrong("colour") && british.wrong("helo"));
            if let Some(mut german) = system("de-DE") {
                assert!(!german.wrong("Straße") && german.wrong("Huas"));
                assert!(german.offer("Huas").iter().any(|offer| offer.starts_with("Ha")));
            }
            assert!(system("xx-YY").is_none());
            assert!(!super::own_language().is_empty());
        })
        .join()
        .unwrap();
    }
}
