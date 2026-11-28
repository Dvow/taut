//! A stream's caption tracks on macOS and iOS (an address played as it comes: HLS, or a file
//! at an address): the item's legible media selection options (subtitle renditions, closed
//! captions in the video stream, a file's caption tracks; not forced-only ones), the one chosen
//! selected and the text it shows handed over a little ahead by an `AVPlayerItemLegibleOutput`,
//! placed as its text markup says.

use crate::macos_sys::{Block, Bool, NSRange, Obj, Pool, Sel, array_items, class, extra_of, made_class, new_with_extra, ns_array, ns_string, rust_string, send};
use crate::video::Track;
use crate::video_apple::{CMTime, Functions};
use crate::video_captions::{Caption, Cue};
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

#[link(name = "System")]
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

/// How far ahead of where it plays the output hands text over, in seconds: each caption is
/// then known as its time comes.
const LEAD: f64 = 2.0;
/// `AVKeyValueStatusLoaded`.
const LOADED: isize = 2;
/// The asset's key that holds its media selection options.
const OPTIONS_KEY: &str = "availableMediaCharacteristicsWithMediaSelectionOptions";

/// The cues each output handed over, by its captions' number: the output's delegate may be
/// called after its captions are gone, and then finds none.
static HANDED_OVER: Mutex<Vec<(u64, Vec<Cue>)>> = Mutex::new(Vec::new());
/// Numbers each stream's captions.
static NUMBERED: AtomicU64 = AtomicU64::new(0);

fn handed_over() -> MutexGuard<'static, Vec<(u64, Vec<Cue>)>> {
    HANDED_OVER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What `take` makes of the cues the output of captions `number` handed over; none where they
/// are gone.
fn with_cues<T>(number: u64, take: impl FnOnce(&mut Vec<Cue>) -> T) -> Option<T> {
    handed_over().iter_mut().find(|(of, _)| *of == number).map(|(_, cues)| take(cues))
}

/// These captions show from `at` on, in place of any before: cues from `at` on go (a seek back
/// hands them over again), those on show end there.
fn show_from(cues: &mut Vec<Cue>, at: Duration, captions: Vec<Caption>) {
    cues.retain(|cue| cue.from < at);
    cues.iter_mut().for_each(|cue| cue.to = cue.to.min(at));
    cues.extend(captions.into_iter().map(|caption| Cue { from: at, to: Duration::MAX, caption }));
}

/// A stream's caption tracks: its item's legible options, and the output that hands over the
/// text of the one chosen.
pub(crate) struct LegibleCaptions {
    number: u64,
    functions: &'static Functions,
    /// The item, the output and its delegate (each retained), and the queue it is called on.
    item: Obj,
    output: Obj,
    delegate: Obj,
    queue: *mut c_void,
    /// The legible options' group once loaded, and its options listed (each retained).
    group: Obj,
    options: Vec<Obj>,
    /// The options were listed once; their loading was asked for.
    listed: bool,
    asked: bool,
}

impl LegibleCaptions {
    /// Hands over the captions of `item` from now on (none chosen).
    pub(crate) fn new(functions: &'static Functions, item: Obj) -> LegibleCaptions {
        let number = NUMBERED.fetch_add(1, Ordering::Relaxed);
        handed_over().push((number, Vec::new()));
        // SAFETY: the classes answer these messages; what is kept is retained. The queue is
        // serial (no attributes).
        unsafe {
            let output: Obj = send(send::<Obj>(class(c"AVPlayerItemLegibleOutput"), c"alloc", ()), c"init", ());
            let delegate = new_with_extra(delegate_class(), number);
            let queue = dispatch_queue_create(c"taut captions".as_ptr(), std::ptr::null());
            send::<()>(output, c"setDelegate:queue:", (delegate, queue));
            send::<()>(output, c"setSuppressesPlayerRendering:", (1 as Bool,));
            send::<()>(output, c"setAdvanceIntervalForDelegateInvocation:", (LEAD,));
            send::<()>(item, c"addOutput:", (output,));
            send::<Obj>(item, c"retain", ());
            LegibleCaptions { number, functions, item, output, delegate, queue, group: std::ptr::null_mut(), options: Vec::new(), listed: false, asked: false }
        }
    }

    /// The legible options as tracks, now listed; none until the item's asset has loaded them.
    /// When first listed, none is chosen (the player may have chosen one by the system's
    /// settings).
    pub(crate) fn tracks(&mut self) -> Vec<Track> {
        let _pool = Pool::new();
        if !self.loaded() {
            self.ask_loading();
            return Vec::new();
        }
        if self.group.is_null() {
            // SAFETY: a live asset whose options are loaded; the group is retained.
            self.group = unsafe { send(send::<Obj>(send::<Obj>(self.item, c"asset", ()), c"mediaSelectionGroupForMediaCharacteristic:", (self.functions.legible,)), c"retain", ()) };
        }
        self.release_options();
        // SAFETY: a live group (or nil, which has none); each option kept is retained.
        let options = array_items(unsafe { send(self.group, c"options", ()) });
        self.options = options.into_iter().filter(|option| !self.forced_only(*option)).map(|option| unsafe { send(option, c"retain", ()) }).collect();
        if !std::mem::replace(&mut self.listed, true) {
            self.choose(None);
        }
        self.options.iter().map(|option| self.track_of(*option)).collect()
    }

    /// Whether options came since they were last listed.
    pub(crate) fn unlisted(&self) -> bool {
        !self.listed && self.loaded()
    }

    /// Whether the options are still to come.
    pub(crate) fn loading(&self) -> bool {
        !self.listed
    }

    /// Selects the `nth` option, or none; what was handed over before goes.
    pub(crate) fn choose(&mut self, nth: Option<usize>) {
        with_cues(self.number, Vec::clear);
        if self.group.is_null() {
            return;
        }
        let option = nth.and_then(|nth| self.options.get(nth)).copied().unwrap_or(std::ptr::null_mut());
        // SAFETY: a live item, its group and one of the group's options (or nil: none).
        unsafe { send::<()>(self.item, c"selectMediaOption:inMediaSelectionGroup:", (option, self.group)) };
    }

    /// The chosen option's captions on show at `at`.
    pub(crate) fn captions_at(&self, at: Duration) -> Vec<Caption> {
        with_cues(self.number, |cues| crate::video_captions::captions_at(cues, at)).unwrap_or_default()
    }

    /// Whether the item's asset has loaded its options.
    fn loaded(&self) -> bool {
        // SAFETY: a live item and its asset; the key is copied.
        unsafe { send::<isize>(send::<Obj>(self.item, c"asset", ()), c"statusOfValueForKey:error:", (ns_string(OPTIONS_KEY), std::ptr::null_mut::<Obj>())) == LOADED }
    }

    /// Asks the item's asset to load its options, once.
    fn ask_loading(&mut self) {
        if std::mem::replace(&mut self.asked, true) {
            return;
        }
        let done = Block::leaked(options_loaded as *const c_void, ());
        // SAFETY: a live asset; the keys are copied; the block lives for good.
        unsafe { send::<()>(send::<Obj>(self.item, c"asset", ()), c"loadValuesAsynchronouslyForKeys:completionHandler:", (ns_array(&[ns_string(OPTIONS_KEY)]), done as *const Block<()>)) };
    }

    /// Whether an option holds forced subtitles only (shown with another, not chosen alone).
    fn forced_only(&self, option: Obj) -> bool {
        // SAFETY: a live option; the characteristic is AVFoundation's constant.
        unsafe { send::<Bool>(option, c"hasMediaCharacteristic:", (self.functions.forced_only,)) != 0 }
    }

    /// An option as a track: its language (BCP 47) and its title (an HLS rendition's `NAME`).
    fn track_of(&self, option: Obj) -> Track {
        let f = self.functions;
        // SAFETY: a live option; the metadata items found are autoreleased; the strings copied.
        let (language, title) = unsafe {
            let titles: Obj = send(class(c"AVMetadataItem"), c"metadataItemsFromArray:withKey:keySpace:", (send::<Obj>(option, c"commonMetadata", ()), f.title_key, f.common_key_space));
            let title = array_items(titles).first().and_then(|item| rust_string(send(*item, c"stringValue", ())));
            (rust_string(send(option, c"extendedLanguageTag", ())), title)
        };
        Track { language: language.unwrap_or_default(), name: title.unwrap_or_default() }
    }

    fn release_options(&mut self) {
        for option in self.options.drain(..) {
            // SAFETY: retained once, by `tracks`.
            unsafe { send::<()>(option, c"release", ()) };
        }
    }
}

impl Drop for LegibleCaptions {
    fn drop(&mut self) {
        self.release_options();
        handed_over().retain(|(of, _)| *of != self.number);
        // SAFETY: the output calls the delegate no more; each object was retained once (the
        // group may be nil), the queue made once.
        unsafe {
            send::<()>(self.output, c"setDelegate:queue:", (std::ptr::null_mut::<c_void>(), std::ptr::null_mut::<c_void>()));
            for object in [self.output, self.delegate, self.group, self.item] {
                send::<()>(object, c"release", ());
            }
            dispatch_release(self.queue);
        }
    }
}

/// The asset loaded its options (nothing to do: they are asked for again).
extern "C" fn options_loaded(_block: *const Block<()>) {}

/// The class of the outputs' delegates: each keeps its captions' number.
fn delegate_class() -> Obj {
    let handed_over = text_handed_over as *const c_void;
    made_class(c"TautLegibleOutputDelegate", c"NSObject", &[(c"legibleOutput:didOutputAttributedStrings:nativeSampleBuffers:forItemTime:", handed_over, "v@:@@@{?=qiIq}")], &[c"AVPlayerItemLegibleOutputPushDelegate"])
}

/// The output handed over the texts shown from `time` on (none: nothing shows). The window
/// runs again: they come from a thread of their own, after a seek's frame, when a paused video
/// no longer asks.
extern "C" fn text_handed_over(delegate: Obj, _: Sel, _output: Obj, strings: Obj, _samples: Obj, time: CMTime) {
    let _pool = Pool::new();
    let Some(at) = time.known() else { return };
    let captions: Vec<Caption> = array_items(strings).into_iter().filter_map(caption_of).collect();
    // SAFETY: made by `new_with_extra` with its captions' number.
    let number: u64 = unsafe { extra_of(delegate) };
    with_cues(number, |cues| show_from(cues, at, captions));
    crate::platform::Waker.wake();
}

/// An attributed string handed over as a caption: its text, placed as its text markup says
/// (CoreMedia's `kCMTextMarkupAttribute_…` keys, as WebVTT's settings), italic where all of
/// it is; none where it has no text.
fn caption_of(string: Obj) -> Option<Caption> {
    // SAFETY: a live attributed string; its text is copied; the attributes it gives are
    // autoreleased, read at once.
    unsafe {
        let text = rust_string(send(string, c"string", ())).filter(|text| !text.trim().is_empty())?;
        let mut run = NSRange { location: 0, length: 0 };
        let attributes: Obj = send(string, c"attributesAtIndex:effectiveRange:", (0usize, &mut run as *mut NSRange));
        let value = |key: &str| -> Obj { send(attributes, c"objectForKey:", (ns_string(key),)) };
        let number = |key: &str| Some(value(key)).filter(|value| !value.is_null()).map(|value| send::<f64>(value, c"doubleValue", ()));
        let markup = TextMarkup {
            alignment: rust_string(value("CMWritingDirectionAlignment")),
            line: number("CMWritingDirectionOrthogonalLinePositionPercentage"),
            position: number("CMWritingDirectionTextPositionPercentage"),
            size: number("CMWritingDirectionSizePercentage"),
        };
        let all_of_it = run.length >= send::<usize>(string, c"length", ());
        let italic = number("CMItalicStyle").is_some_and(|italic| italic != 0.0) && all_of_it;
        Some(Caption { italic, ..crate::video_captions::caption_with_settings(&text, &markup.settings()) })
    }
}

/// Where a caption's text markup puts it: its alignment along the line (`CMWDA_start`,
/// `_middle`, `_end`, `_left`, `_right`), and in percent of the frame where its line sits down
/// it, where it sits along it, and how wide it is.
struct TextMarkup {
    alignment: Option<String>,
    line: Option<f64>,
    position: Option<f64>,
    size: Option<f64>,
}

impl TextMarkup {
    /// As WebVTT's cue settings, those at their usual values (the bottom middle, as wide as
    /// the frame) left out.
    fn settings(&self) -> String {
        let align = self.alignment.as_deref().and_then(|alignment| alignment.strip_prefix("CMWDA_")).filter(|align| *align != "middle");
        let mut settings = Vec::new();
        if let Some(align) = align {
            settings.push(format!("align:{align}"));
        }
        if let Some(line) = self.line.filter(|line| *line < 100.0) {
            settings.push(format!("line:{line}%"));
        }
        if let Some(position) = self.position.filter(|position| align.is_some() || *position != 50.0) {
            settings.push(format!("position:{position}%"));
        }
        if let Some(size) = self.size.filter(|size| *size < 100.0) {
            settings.push(format!("size:{size}%"));
        }
        settings.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn texts(cues: &[Cue], at: Duration) -> Vec<String> {
        crate::video_captions::captions_at(cues, at).into_iter().map(|caption| caption.text).collect()
    }

    #[test]
    fn text_handed_over_shows_until_the_next_and_a_seek_back_replaces_what_follows() {
        let mut cues = Vec::new();
        // (As it plays: one, then two, then nothing, each handed over ahead.)
        show_from(&mut cues, ms(0), vec![Caption::plain("one")]);
        show_from(&mut cues, ms(1000), vec![Caption::plain("two")]);
        show_from(&mut cues, ms(2000), Vec::new());
        assert_eq!((texts(&cues, ms(500)), texts(&cues, ms(1500)), texts(&cues, ms(2500))), (vec!["one".to_owned()], vec!["two".to_owned()], Vec::new()));
        // (A seek back into two: handed over again from its start.)
        show_from(&mut cues, ms(1000), vec![Caption::plain("two")]);
        assert_eq!((texts(&cues, ms(500)), texts(&cues, ms(1500)), texts(&cues, ms(2500))), (vec!["one".to_owned()], vec!["two".to_owned()], vec!["two".to_owned()]));
    }

    #[test]
    fn text_markup_is_webvtts_settings_the_usual_ones_left_out() {
        let markup = |alignment: &str, line: f64, position: f64, size: f64| TextMarkup { alignment: Some(alignment.into()), line: Some(line), position: Some(position), size: Some(size) }.settings();
        // (As AVFoundation hands over WebVTT at the usual place, WebVTT placed, and CEA-608's
        // row 15.)
        assert_eq!(markup("CMWDA_middle", 100.0, 50.0, 100.0), "");
        assert_eq!(markup("CMWDA_start", 10.0, 10.0, 100.0), "align:start line:10% position:10%");
        assert_eq!(markup("CMWDA_middle", 87.5, 23.75, 17.1875), "line:87.5% position:23.75% size:17.1875%");
        assert_eq!(TextMarkup { alignment: None, line: None, position: None, size: None }.settings(), "");
    }
}
