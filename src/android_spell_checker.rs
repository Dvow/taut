//! Spell checking on Android: the system's spell checker (`SpellCheckerSession`; on most
//! phones the keyboard's), asked from the checking thread (see `spell.rs`). It answers on a
//! thread of its own (a `HandlerThread`) through the package's class `taut.Spelling` (see
//! `examples/apk.rs`), given its natives here the first time a box asks to be checked. Adding
//! a word opens the system's form for the person's dictionary, as Android's own boxes do.
use crate::android::last_activity;
use crate::android_jni::{self as jni, Env, Native, Obj, Value};
use crate::spell::{Check, OFFERED};
use std::ffi::c_void;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// `SuggestionsInfo.RESULT_ATTR_LOOKS_LIKE_TYPO`.
const TYPO: i32 = 2;
/// How long the checker's answer is waited for.
const PATIENCE: Duration = Duration::from_secs(2);

/// What the checking thread and the checker's thread hand each other.
struct Exchange {
    /// A session asked for (its language), and the session made: a global reference, null
    /// where the system has no checker for the language.
    opening: Option<String>,
    opened: Option<usize>,
    /// The question asked last, and its answer: whether the word looks wrong, and suggestions.
    asked: i32,
    answer: Option<(bool, Vec<String>)>,
}

static EXCHANGE: Mutex<Exchange> = Mutex::new(Exchange { opening: None, opened: None, asked: 0, answer: None });
static ANSWERED: Condvar = Condvar::new();
/// The listener and the handler of the checker's thread (global references), made once.
static LISTENING: OnceLock<Option<(usize, usize)>> = OnceLock::new();

/// The exchange, locked (it stays consistent through a panic: each field is set whole).
fn exchange() -> MutexGuard<'static, Exchange> {
    EXCHANGE.lock().unwrap_or_else(|held| held.into_inner())
}

/// Waits until `take` takes what was asked for from the exchange, for `PATIENCE` at most.
fn wait_for<T>(mut take: impl FnMut(&mut Exchange) -> Option<T>) -> Option<T> {
    let until = Instant::now() + PATIENCE;
    let mut held = exchange();
    loop {
        if let Some(taken) = take(&mut held) {
            return Some(taken);
        }
        let left = until.checked_duration_since(Instant::now())?;
        held = ANSWERED.wait_timeout(held, left).unwrap_or_else(|held| held.into_inner()).0;
    }
}

/// The listener and the handler of the checker's thread, made the first time.
fn listening(env: Env) -> Option<(Obj, Obj)> {
    let made = LISTENING.get_or_init(|| env.frame(|| listen(env)));
    made.map(|(listener, handler)| (listener as Obj, handler as Obj))
}

/// Gives `taut.Spelling` its natives, and makes a listener and a thread for the checker.
fn listen(env: Env) -> Option<(usize, usize)> {
    let class = jni::app_class(env, last_activity(), "taut.Spelling");
    let natives = [
        Native::new(c"onGetSuggestions", c"([Landroid/view/textservice/SuggestionsInfo;)V", on_words as *const c_void),
        Native::new(c"onGetSentenceSuggestions", c"([Landroid/view/textservice/SentenceSuggestionsInfo;)V", on_sentences as *const c_void),
        Native::new(c"run", c"()V", open_here as *const c_void),
    ];
    if class.is_null() || !env.register(class, &natives) {
        crate::android_sys::log("taut: spell checking needs the package's class taut.Spelling (see examples/apk.rs).");
        return None;
    }
    let listener = env.global(env.new_object(class, c"()V", &[]));
    let thread = env.new_named(c"android/os/HandlerThread", c"(Ljava/lang/String;)V", &[Value::obj(env.string("taut spelling"))]);
    env.call(thread, c"start", c"()V", &[]);
    let looper = env.call(thread, c"getLooper", c"()Landroid/os/Looper;", &[]).object();
    let handler = env.global(env.new_named(c"android/os/Handler", c"(Landroid/os/Looper;)V", &[Value::obj(looper)]));
    (!listener.is_null() && !handler.is_null()).then_some((listener as usize, handler as usize))
}

/// The checker for one language, by the session the system opened for it.
pub(crate) struct Speller(Obj);

impl Speller {
    /// The checker for the language of this name ("en-US", "de_DE", "fr"), if the system has
    /// one: its session, opened on the checker's thread.
    pub(crate) fn open(name: &str) -> Option<Speller> {
        let env = jni::thread_env()?;
        let (listener, handler) = listening(env)?;
        *exchange() = Exchange { opening: Some(name.replace('_', "-")), opened: None, asked: 0, answer: None };
        env.call(handler, c"post", c"(Ljava/lang/Runnable;)Z", &[Value::obj(listener)]);
        let session = wait_for(|now| now.opened.take())? as Obj;
        (!session.is_null()).then_some(Speller(session))
    }

    /// The checker's answer for `word`: whether it looks wrong, and up to `OFFERED`
    /// suggestions.
    fn ask(&self, word: &str) -> Option<(bool, Vec<String>)> {
        let env = jni::thread_env()?;
        let asked = {
            let mut now = exchange();
            (now.asked, now.answer) = (now.asked.wrapping_add(1).max(1), None);
            now.asked
        };
        env.frame(|| {
            let info = env.new_named(c"android/view/textservice/TextInfo", c"(Ljava/lang/String;II)V", &[Value::obj(env.string(word)), Value::int(0), Value::int(asked)]);
            let infos = jni::new_objects(env, c"android/view/textservice/TextInfo", &[info]);
            env.call(self.0, c"getSentenceSuggestions", c"([Landroid/view/textservice/TextInfo;I)V", &[Value::obj(infos), Value::int(OFFERED as i32)]);
        });
        wait_for(|now| now.answer.take())
    }
}

impl Check for Speller {
    fn wrong(&self, word: &str) -> bool {
        self.ask(word).is_some_and(|(wrong, _)| wrong)
    }
    fn offer(&mut self, word: &str) -> Vec<String> {
        self.ask(word).map(|(_, offers)| offers).unwrap_or_default()
    }
    /// Opens the system's form to add the word to the person's dictionary.
    fn add(&mut self, word: &str) {
        let Some(env) = jni::thread_env() else { return };
        env.frame(|| {
            let intent = env.new_named(c"android/content/Intent", c"(Ljava/lang/String;)V", &[Value::obj(env.string("com.android.settings.USER_DICTIONARY_INSERT"))]);
            env.call(intent, c"putExtra", c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[Value::obj(env.string("word")), Value::obj(env.string(word))]);
            jni::start_new_task(env, last_activity(), intent);
        });
    }
}

impl Drop for Speller {
    fn drop(&mut self) {
        if let Some(env) = jni::thread_env() {
            env.call(self.0, c"close", c"()V", &[]);
            env.drop_global(self.0);
        }
    }
}

/// The person's own language, as Android names it ("en-US").
pub(crate) fn own_language() -> String {
    let Some(env) = jni::thread_env() else { return String::new() };
    env.frame(|| {
        let locale = env.call_static(c"java/util/Locale", c"getDefault", c"()Ljava/util/Locale;", &[]).object();
        env.text(env.call(locale, c"toLanguageTag", c"()Ljava/lang/String;", &[]).object()).unwrap_or_default()
    })
}

/// On the checker's thread: opens the session asked for, this listener hearing its answers.
extern "C" fn open_here(env: Env, listener: Obj) {
    let Some(name) = exchange().opening.take() else { return };
    let session = env.frame(|| {
        let locale = env.call_static(c"java/util/Locale", c"forLanguageTag", c"(Ljava/lang/String;)Ljava/util/Locale;", &[Value::obj(env.string(&name))]).object();
        let services = jni::service(env, last_activity(), "textservices");
        let args = [Value::obj(std::ptr::null_mut()), Value::obj(locale), Value::obj(listener), Value::bool(false)];
        let session = env
            .call(
                services,
                c"newSpellCheckerSession",
                c"(Landroid/os/Bundle;Ljava/util/Locale;Landroid/view/textservice/SpellCheckerSession$SpellCheckerSessionListener;Z)Landroid/view/textservice/SpellCheckerSession;",
                &args,
            )
            .object();
        env.global(session)
    });
    exchange().opened = Some(session as usize);
    ANSWERED.notify_all();
}

/// A word's answer, as the checker gives it: the question it answers, whether the word looks
/// wrong, and suggestions.
fn answer_of(env: Env, info: Obj) -> (i32, bool, Vec<String>) {
    let looks = env.call(info, c"getSuggestionsAttributes", c"()I", &[]).as_int() & TYPO != 0;
    let count = env.call(info, c"getSuggestionsCount", c"()I", &[]).as_int();
    let offers = (0..count).filter_map(|at| env.text(env.call(info, c"getSuggestionAt", c"(I)Ljava/lang/String;", &[Value::int(at)]).object())).collect();
    (env.call(info, c"getSequence", c"()I", &[]).as_int(), looks, offers)
}

/// Takes in the words' answers to the question asked last: none means every word is right.
fn answered(answers: Vec<(i32, bool, Vec<String>)>) {
    let mut now = exchange();
    let (asked, any) = (now.asked, !answers.is_empty());
    let mine: Vec<(bool, Vec<String>)> = answers.into_iter().filter(|(of, ..)| *of == asked).map(|(_, wrong, offers)| (wrong, offers)).collect();
    // (Answers to an earlier question, come late, are not this one's.)
    if any && mine.is_empty() {
        return;
    }
    let wrong = mine.iter().any(|(wrong, _)| *wrong);
    now.answer = Some((wrong, mine.into_iter().flat_map(|(_, offers)| offers).take(OFFERED).collect()));
    drop(now);
    ANSWERED.notify_all();
}

/// The checker's answer, word by word.
extern "C" fn on_words(env: Env, _: Obj, infos: Obj) {
    let answers = env.frame(|| jni::objects(env, infos).into_iter().map(|info| answer_of(env, info)).collect());
    answered(answers);
}

/// The checker's answer, by sentence: each sentence's words.
extern "C" fn on_sentences(env: Env, _: Obj, sentences: Obj) {
    let answers = env.frame(|| {
        let words = |sentence: Obj| {
            let count = env.call(sentence, c"getSuggestionsCount", c"()I", &[]).as_int();
            (0..count).map(move |at| env.call(sentence, c"getSuggestionsInfoAt", c"(I)Landroid/view/textservice/SuggestionsInfo;", &[Value::int(at)]).object())
        };
        jni::objects(env, sentences).into_iter().flat_map(words).map(|info| answer_of(env, info)).collect()
    });
    answered(answers);
}
