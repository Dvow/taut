//! Spell checking on macOS: the system's checker, `NSSpellChecker`, asked from the checking
//! thread (see `spell.rs`), with the user's dictionary.
//!
//! It names languages "en", "en_GB", "de": a name as the app gives it ("en-US") is matched
//! whole, then by its language alone.
use crate::macos_sys::*;
use crate::spell::{Check, OFFERED};

/// The checker for one language, by the name `NSSpellChecker` has for it.
pub(crate) struct Speller(String);

/// The shared checker.
fn checker() -> Obj {
    // SAFETY: a plain question; the checker lives as long as the program.
    unsafe { send(class(c"NSSpellChecker"), c"sharedSpellChecker", ()) }
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
        // SAFETY: the word and language are strings; no wrap, no document (0), no word count.
        let found: NSRange = unsafe {
            send(
                checker(),
                c"checkSpellingOfString:startingAt:language:wrap:inSpellDocumentWithTag:wordCount:",
                (ns_string(word), 0isize, ns_string(&self.0), 0 as Bool, 0isize, std::ptr::null_mut::<isize>()),
            )
        };
        found.location != NOT_FOUND && found.length > 0
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
mod probe {
    use super::*;
    use crate::spell::system;
    use std::ffi::c_void;
    use std::process::Command;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    type Answer = Arc<(Mutex<Option<(isize, usize)>>, Condvar)>;

    extern "C" fn answered(block: *const Block<Answer>, _: isize, results: Obj, _: Obj, count: isize) {
        let held = unsafe { &(*block).held };
        *held.0.lock().unwrap() = Some((count, array_items(results).len()));
        held.1.notify_all();
    }

    fn signal_spell_server(signal: &str) {
        let listed = Command::new("ps").args(["-axo", "pid,user,comm"]).output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        for line in listed.lines().filter(|line| line.to_lowercase().contains("spell")) {
            eprintln!("probe: {signal} {line}");
            let pid = line.split_whitespace().next().unwrap();
            if line.contains("AppleSpell") {
                let status = Command::new("kill").args([signal, pid]).status();
                eprintln!("probe: kill {signal} {pid}: {status:?}");
            }
        }
    }

    /// Stops every AppleSpell that appears until `hold` is over (polling), then lets them go.
    fn stop_relaunches(hold: Duration) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let (start, mut stopped) = (Instant::now(), Vec::<String>::new());
            while start.elapsed() < hold {
                let found = Command::new("pgrep").args(["-x", "AppleSpell"]).output().unwrap();
                for pid in String::from_utf8_lossy(&found.stdout).split_whitespace().map(str::to_owned) {
                    if !stopped.contains(&pid) {
                        let status = Command::new("kill").args(["-STOP", &pid]).status();
                        eprintln!("probe: relaunched AppleSpell {pid} stopped at {:?}: {status:?}", start.elapsed());
                        stopped.push(pid);
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            for pid in &stopped {
                _ = Command::new("kill").args(["-CONT", pid]).status();
            }
            eprintln!("probe: let go of {stopped:?} at {:?}", start.elapsed());
        })
    }

    fn ask(way: &str, checker: &dyn crate::spell::Check, word: &str) {
        let start = Instant::now();
        match way {
            "sync" => eprintln!("probe: sync says {word} wrong: {} after {:?}", checker.wrong(word), start.elapsed()),
            "checkstring" => {
                let _pool = Pool::new();
                let results: Obj = unsafe {
                    send(
                        super::checker(),
                        c"checkString:range:types:options:inSpellDocumentWithTag:orthography:wordCount:",
                        (ns_string(word), NSRange { location: 0, length: word.encode_utf16().count() }, 2u64, std::ptr::null_mut::<c_void>(), 0isize, std::ptr::null_mut::<Obj>(), std::ptr::null_mut::<isize>()),
                    )
                };
                eprintln!("probe: checkString {word} gave {} results after {:?}", array_items(results).len(), start.elapsed());
            }
            _ => {
                let answer: Answer = Arc::default();
                let block = Block::leaked(answered as *const c_void, answer.clone());
                let _pool = Pool::new();
                let sequence: isize = unsafe {
                    send(
                        super::checker(),
                        c"requestCheckingOfString:range:types:options:inSpellDocumentWithTag:completionHandler:",
                        (ns_string(word), NSRange { location: 0, length: word.encode_utf16().count() }, 2u64, std::ptr::null_mut::<c_void>(), 0isize, block as *const Block<Answer>),
                    )
                };
                let guard = answer.0.lock().unwrap();
                let (guard, waited) = answer.1.wait_timeout_while(guard, Duration::from_secs(60), |got| got.is_none()).unwrap();
                eprintln!("probe: async {word} (sequence {sequence}) answer {:?} (timed out {}) after {:?}", *guard, waited.timed_out(), start.elapsed());
            }
        }
    }

    /// (Probe) The spell server stopped or relaunching stopped: what each way of asking does.
    /// PROBE_WAY = sync, async, checkstring; PROBE_START = stop, relaunch, cold.
    #[test]
    #[ignore]
    fn probe_spell_server_stopped() {
        let way = std::env::var("PROBE_WAY").unwrap_or_default();
        let begin = std::env::var("PROBE_START").unwrap_or_default();
        std::thread::spawn(move || {
            let mut watcher = None;
            if begin == "cold" {
                signal_spell_server("-KILL");
                watcher = Some(stop_relaunches(Duration::from_secs(30)));
            }
            let checker = system("en-US").expect("a checker");
            if begin != "cold" {
                eprintln!("probe: warm, helo wrong: {}", checker.wrong("helo"));
            }
            match begin.as_str() {
                "relaunch" => {
                    signal_spell_server("-KILL");
                    watcher = Some(stop_relaunches(Duration::from_secs(30)));
                }
                "cold" => {}
                _ => signal_spell_server("-STOP"),
            }
            let start = Instant::now();
            for word in ["wrld", "recieve", "naïve", "teh", "hello", "speling", "colour", "thier"] {
                ask(&way, &*checker, word);
            }
            eprintln!("probe: {way} {begin} done after {:?}", start.elapsed());
            if let Some(watcher) = watcher {
                watcher.join().unwrap();
            }
            signal_spell_server("-CONT");
            eprintln!("probe: after, wrld wrong: {}", checker.wrong("wrld"));
        })
        .join()
        .unwrap();
    }
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
