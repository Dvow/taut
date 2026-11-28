//! A stream's caption tracks on macOS and iOS (an address played as it comes, as HLS): the
//! item's legible media selection options (subtitle renditions, closed captions), the one chosen
//! selected and its cues handed over a little ahead by an `AVPlayerItemLegibleOutput`. WebVTT
//! and CEA-608 come as their samples, read by taut (placed as they say); anything else as the
//! text shown.

use crate::macos_sys::{Block, Bool, Obj, Pool, Sel, array_items, class, extra_of, made_class, new_with_extra, ns_array, ns_string, rust_string, send};
use crate::video::Track;
use crate::video_apple::{CMTime, Functions, sample_bytes};
use crate::video_captions::{Caption, Cue};
use crate::video_stream_captions::{ClosedCaptions, cea608_pairs};
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

#[link(name = "System")]
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

/// How far ahead of where it plays the output hands cues over, in seconds.
const LEAD: f64 = 2.0;
/// The sample formats handed over as samples: WebVTT, CEA-608 and CEA-708.
const WEBVTT: u32 = u32::from_be_bytes(*b"wvtt");
const CEA608: u32 = u32::from_be_bytes(*b"c608");
const CEA708: u32 = u32::from_be_bytes(*b"c708");
/// `AVKeyValueStatusLoaded`.
const LOADED: isize = 2;
/// The asset's key that holds its media selection options.
const SELECTION_KEY: &str = "availableMediaCharacteristicsWithMediaSelectionOptions";

/// What the legible outputs handed over, by their captions' number: the output's delegate
/// may be called after its captions are gone, and then finds none.
static HANDED_OVER: Mutex<Vec<(u64, HandedOver)>> = Mutex::new(Vec::new());
/// Numbers each stream's captions.
static NUMBERED: AtomicU64 = AtomicU64::new(0);

fn handed_over() -> MutexGuard<'static, Vec<(u64, HandedOver)>> {
    HANDED_OVER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What `take` makes of what the output of captions `number` handed over; none where they are
/// gone.
fn with_handed_over<T>(number: u64, take: impl FnOnce(&mut HandedOver) -> T) -> Option<T> {
    handed_over().iter_mut().find(|(of, _)| *of == number).map(|(_, handed)| take(handed))
}

/// The chosen option's cues handed over so far: WebVTT's and the text shown, and CEA-608's
/// services.
#[derive(Default)]
struct HandedOver {
    cues: Vec<Cue>,
    closed: ClosedCaptions,
}

impl HandedOver {
    /// Takes in a sample of WebVTT or CEA-608.
    fn take(&mut self, sample: Sample) {
        match sample.kind {
            WEBVTT => self.take_webvtt(sample),
            CEA608 => cea608_pairs(&sample.bytes).for_each(|(field, pair)| self.closed.read_cea608_pair(field, pair, sample.from)),
            _ => eprintln!("probe: sample {} at {:?} to {:?}: {:02x?}", fourcc(sample.kind), sample.from, sample.to, &sample.bytes[..sample.bytes.len().min(96)]),
        }
    }

    /// A WebVTT sample's cues, unless its time's were taken.
    fn take_webvtt(&mut self, sample: Sample) {
        let at = self.cues.partition_point(|cue| cue.from < sample.from);
        if self.cues.get(at).is_some_and(|cue| cue.from == sample.from) {
            return;
        }
        let to = sample.to.unwrap_or(Duration::MAX);
        let new: Vec<Cue> = crate::video_mp4::webvtt_cues(&sample.bytes).into_iter().map(|caption| Cue { from: sample.from, to, caption }).collect();
        self.cues.splice(at..at, new);
    }

    /// These texts show from `at` on, in place of any before.
    fn show_texts(&mut self, at: Duration, texts: Vec<String>) {
        self.cues.retain(|cue| cue.from < at);
        self.cues.iter_mut().for_each(|cue| cue.to = cue.to.min(at));
        self.cues.extend(texts.into_iter().map(|text| Cue { from: at, to: Duration::MAX, caption: Caption::plain(text) }));
    }
}

/// A sample handed over: its format, when it shows and until when, and its bytes.
struct Sample {
    kind: u32,
    from: Duration,
    to: Option<Duration>,
    bytes: Vec<u8>,
}

/// A stream's caption tracks: its item's legible options, and the output that hands over the
/// chosen one's cues.
pub(crate) struct LegibleCaptions {
    number: u64,
    functions: &'static Functions,
    /// The item, the output and its delegate (retained), and the queue it is called on.
    item: Obj,
    output: Obj,
    delegate: Obj,
    queue: *mut c_void,
    /// The legible options' group once loaded, and its options (retained).
    group: Obj,
    options: Vec<Obj>,
    /// The options were listed; their loading was asked for.
    listed: bool,
    asked: bool,
}

impl LegibleCaptions {
    /// Hands over the captions of `item` from now on (none chosen).
    pub(crate) fn new(functions: &'static Functions, item: Obj) -> LegibleCaptions {
        let number = NUMBERED.fetch_add(1, Ordering::Relaxed);
        handed_over().push((number, HandedOver::default()));
        let native = std::env::var("TAUT_PROBE_NATIVE").as_deref() != Ok("0");
        // SAFETY: the classes answer these messages; what is kept is retained, the rest
        // autoreleased into the pool. The queue is serial (no attributes).
        unsafe {
            let _pool = Pool::new();
            let kinds: Vec<Obj> = [WEBVTT, CEA608, CEA708].iter().filter(|_| native).map(|kind| send(class(c"NSNumber"), c"numberWithUnsignedInt:", (*kind,))).collect();
            let output: Obj = send(send::<Obj>(class(c"AVPlayerItemLegibleOutput"), c"alloc", ()), c"initWithMediaSubtypesForNativeRepresentation:", (ns_array(&kinds),));
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
    /// When first listed none is chosen (the player may have chosen one by the system's
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
        // SAFETY: a live group (or nil, which has none); each option is retained.
        self.options = array_items(unsafe { send(self.group, c"options", ()) }).into_iter().map(|option| unsafe { send(option, c"retain", ()) }).collect();
        if !std::mem::replace(&mut self.listed, true) {
            self.choose(None);
        }
        self.options.iter().map(|option| option_track(*option)).collect()
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
        with_handed_over(self.number, |handed| *handed = HandedOver::default());
        if self.group.is_null() {
            return;
        }
        let option = nth.and_then(|nth| self.options.get(nth)).copied().unwrap_or(std::ptr::null_mut());
        eprintln!("probe: choose {nth:?}");
        // SAFETY: a live item, its group and one of the group's options (or nil: none).
        unsafe { send::<()>(self.item, c"selectMediaOption:inMediaSelectionGroup:", (option, self.group)) };
    }

    /// The chosen option's captions on show at `at`.
    pub(crate) fn captions_at(&self, at: Duration) -> Vec<Caption> {
        let shown = with_handed_over(self.number, |handed| {
            let mut shown = crate::video_captions::captions_at(&handed.cues, at);
            shown.extend(handed.closed.captions_at(0, at));
            shown
        });
        shown.unwrap_or_default()
    }

    /// It went to `to`: CEA-608 is read again from there.
    pub(crate) fn sought(&self, to: Duration) {
        with_handed_over(self.number, |handed| handed.closed.sought(to));
    }

    /// Whether the item's asset has loaded its options.
    fn loaded(&self) -> bool {
        // SAFETY: a live item and its asset; the key is copied.
        unsafe { send::<isize>(send::<Obj>(self.item, c"asset", ()), c"statusOfValueForKey:error:", (ns_string(SELECTION_KEY), std::ptr::null_mut::<Obj>())) == LOADED }
    }

    /// Asks the item's asset to load its options, once.
    fn ask_loading(&mut self) {
        if std::mem::replace(&mut self.asked, true) {
            return;
        }
        let done = Block::leaked(loaded as *const c_void, ());
        // SAFETY: a live asset; the keys are copied; the block lives for good.
        unsafe { send::<()>(send::<Obj>(self.item, c"asset", ()), c"loadValuesAsynchronouslyForKeys:completionHandler:", (ns_array(&[ns_string(SELECTION_KEY)]), done as *const Block<()>)) };
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
        // SAFETY: the output calls the delegate no more; each object was retained once, the
        // queue made once.
        unsafe {
            send::<()>(self.output, c"setDelegate:queue:", (std::ptr::null_mut::<c_void>(), std::ptr::null_mut::<c_void>()));
            for object in [self.output, self.delegate, self.group, self.item] {
                send::<()>(object, c"release", ());
            }
            dispatch_release(self.queue);
        }
    }
}

/// The loading of options is over (nothing to do: they are asked for again).
extern "C" fn loaded(_block: *const Block<()>) {}

/// A legible option as a track: its language (BCP 47) and name.
fn option_track(option: Obj) -> Track {
    // SAFETY: a live option; the strings are copied.
    let (language, name, kind, more) = unsafe {
        (
            rust_string(send(option, c"extendedLanguageTag", ())),
            rust_string(send(option, c"displayName", ())),
            rust_string(send(option, c"mediaType", ())),
            rust_string(send(send::<Obj>(option, c"description", ()), c"description", ())),
        )
    };
    // SAFETY: as above.
    let list = unsafe { rust_string(send(send::<Obj>(option, c"propertyList", ()), c"description", ())) };
    // SAFETY: as above.
    let subtypes = unsafe { rust_string(send(send::<Obj>(option, c"mediaSubTypes", ()), c"description", ())) };
    eprintln!("probe: option {language:?} {name:?} {kind:?} subtypes {subtypes:?} list {list:?} described {more:?}");
    Track { language: language.unwrap_or_default(), name: name.unwrap_or_default() }
}

/// The class of the outputs' delegates: each keeps its captions' number.
fn delegate_class() -> Obj {
    let handed = handed as *const c_void;
    made_class(c"TautLegibleOutputDelegate", c"NSObject", &[(c"legibleOutput:didOutputAttributedStrings:nativeSampleBuffers:forItemTime:", handed, "v@:@@@{?=qiIq}")], &[c"AVPlayerItemLegibleOutputPushDelegate"])
}

/// The output handed over what shows from `time`: as samples, or as the texts shown.
extern "C" fn handed(delegate: Obj, _: Sel, _output: Obj, strings: Obj, samples: Obj, time: CMTime) {
    let _pool = Pool::new();
    let Some(functions) = crate::video_apple::functions() else { return };
    // SAFETY: made by `new_with_extra` with its captions' number.
    let number: u64 = unsafe { extra_of(delegate) };
    let samples: Vec<Sample> = array_items(samples).into_iter().filter_map(|sample| sample_of(functions, sample)).collect();
    // SAFETY: each is a live attributed string; its text is copied.
    let texts: Vec<String> = array_items(strings).into_iter().filter_map(|string| unsafe { rust_string(send(string, c"string", ())) }).collect();
    for string in array_items(strings) {
        // SAFETY: as above.
        let described = unsafe { rust_string(send(string, c"description", ())) };
        eprintln!("probe: {number} at {:?} string {described:?}", time.known());
    }
    eprintln!("probe: {number} at {:?}: {} strings, {} samples", time.known(), texts.len(), samples.len());
    let at = time.known().unwrap_or_default();
    with_handed_over(number, |handed| match samples.is_empty() {
        true => handed.show_texts(at, texts),
        false => samples.into_iter().for_each(|sample| handed.take(sample)),
    });
}

/// A sample buffer handed over: its format, times and bytes; none where it has no time.
fn sample_of(f: &Functions, sample: Obj) -> Option<Sample> {
    // SAFETY: a live sample buffer, and its format description.
    let (kind, from, length) = unsafe { ((f.media_subtype)((f.sample_format)(sample)), (f.sample_time)(sample).known()?, (f.sample_duration)(sample).known()) };
    let bytes = sample_bytes(f, sample).unwrap_or_default();
    eprintln!("probe: sample {} at {from:?} for {length:?}: {:02x?}", fourcc(kind), &bytes[..bytes.len().min(96)]);
    Some(Sample { kind, from, to: length.map(|length| from + length), bytes })
}

fn fourcc(kind: u32) -> String {
    String::from_utf8_lossy(&kind.to_be_bytes()).into_owned()
}
