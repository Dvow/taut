//! Videos on macOS and iOS: AVFoundation's `AVPlayer`, its frames read through an
//! `AVPlayerItemVideoOutput` as 32-bit BGRA. The frameworks are opened at run time; their
//! classes are reached by name. Bytes are played from a file of their own, removed after. It
//! plays while the main thread's run loop turns (AppKit's or UIKit's).
//!
//! Sound tracks are the item's audio tracks, one enabled at a time. Caption tracks are the
//! asset's subtitle and text tracks, their cues read once by an `AVAssetReader` (3GPP timed
//! text). Speed is the player's rate, the pitch kept by the spectral time-pitch algorithm.

use crate::color::Color;
use crate::macos_sys::{Block, Bool, Obj, Pool, array_items, class, ns_string, rust_string, send, send_large};
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use crate::video_captions::Cue;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// A time as CoreMedia keeps it: `value / timescale` seconds, valid where `flags & 1`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    /// Its time, if a known, finite one (valid, not indefinite or infinite).
    fn known(self) -> Option<Duration> {
        let finite = self.flags & 1 != 0 && self.flags & 0x1C == 0 && self.timescale > 0;
        finite.then(|| Duration::from_secs_f64((self.value as f64 / self.timescale as f64).max(0.0)))
    }
    fn of(time: Duration) -> CMTime {
        CMTime { value: time.as_micros().min(i64::MAX as u128) as i64, timescale: 1_000_000, flags: 1, epoch: 0 }
    }
}

/// CoreVideo's functions for reading a pixel buffer, and its key for their format; CoreMedia's
/// for reading a sample buffer; AVFoundation's name of the time-pitch algorithm that keeps the
/// pitch at any rate.
struct Functions {
    lock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    unlock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    base: unsafe extern "C" fn(*mut c_void) -> *const u8,
    row: unsafe extern "C" fn(*mut c_void) -> usize,
    width: unsafe extern "C" fn(*mut c_void) -> usize,
    height: unsafe extern "C" fn(*mut c_void) -> usize,
    release: unsafe extern "C" fn(*mut c_void),
    format_key: Obj,
    sample_time: unsafe extern "C" fn(*mut c_void) -> CMTime,
    sample_duration: unsafe extern "C" fn(*mut c_void) -> CMTime,
    sample_data: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    data_length: unsafe extern "C" fn(*mut c_void) -> usize,
    copy_data: unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8) -> i32,
    spectral: Obj,
}
// SAFETY: functions, and a constant string object that lives as long as the program.
unsafe impl Send for Functions {}
// SAFETY: as above.
unsafe impl Sync for Functions {}

/// The frameworks, opened once; none where AVFoundation is missing.
fn functions() -> Option<&'static Functions> {
    static LOADED: OnceLock<Option<Functions>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref()
}

fn load() -> Option<Functions> {
    let open = |path: &CStr| {
        // SAFETY: a path ending in a zero; 1 is `RTLD_LAZY`.
        let library = unsafe { dlopen(path.as_ptr(), 1) };
        (!library.is_null()).then_some(library)
    };
    let av = open(c"/System/Library/Frameworks/AVFoundation.framework/AVFoundation")?;
    let video = open(c"/System/Library/Frameworks/CoreVideo.framework/CoreVideo")?;
    let media = open(c"/System/Library/Frameworks/CoreMedia.framework/CoreMedia")?;
    let found = |library: *mut c_void, name: &CStr| {
        // SAFETY: a name ending in a zero, of an open library.
        let at = unsafe { dlsym(library, name.as_ptr()) };
        (!at.is_null()).then_some(at)
    };
    // SAFETY: each is the function or constant of that name, of the type its header gives.
    unsafe {
        Some(Functions {
            lock: function(found(video, c"CVPixelBufferLockBaseAddress")?),
            unlock: function(found(video, c"CVPixelBufferUnlockBaseAddress")?),
            base: function(found(video, c"CVPixelBufferGetBaseAddress")?),
            row: function(found(video, c"CVPixelBufferGetBytesPerRow")?),
            width: function(found(video, c"CVPixelBufferGetWidth")?),
            height: function(found(video, c"CVPixelBufferGetHeight")?),
            release: function(found(video, c"CVBufferRelease")?),
            format_key: *found(video, c"kCVPixelBufferPixelFormatTypeKey")?.cast::<Obj>(),
            sample_time: function(found(media, c"CMSampleBufferGetPresentationTimeStamp")?),
            sample_duration: function(found(media, c"CMSampleBufferGetDuration")?),
            sample_data: function(found(media, c"CMSampleBufferGetDataBuffer")?),
            data_length: function(found(media, c"CMBlockBufferGetDataLength")?),
            copy_data: function(found(media, c"CMBlockBufferCopyDataBytes")?),
            spectral: *found(av, c"AVAudioTimePitchAlgorithmSpectral")?.cast::<Obj>(),
        })
    }
}

/// A function at this address, as type `F`.
///
/// # Safety
/// The address is such a function.
unsafe fn function<F: Copy>(at: *mut c_void) -> F {
    // SAFETY: as the caller promises; a function pointer is one pointer, as `F` is.
    unsafe { std::mem::transmute_copy(&at) }
}

/// An `AVPlayer` with one item and its video output.
pub(crate) struct Player {
    functions: &'static Functions,
    player: Obj,
    item: Obj,
    output: Obj,
    playing: bool,
    /// The item was seen ready to play: its first frame was asked for.
    ready: bool,
    /// The last frame's time.
    last: Option<i64>,
    /// The file bytes were written to.
    written: Option<PathBuf>,
    /// Seeks asked for; frames wait until the player has finished as many (`seeked`):
    /// while it decodes its way to the time asked, it hands out frames on the way.
    seeks: u32,
    /// What the player calls when a seek is over.
    seeked: &'static Seeked,
    /// The rate it plays at.
    speed: f32,
    /// The caption tracks' cues, read once the item is ready; the one chosen.
    cues: Vec<Vec<Cue>>,
    captions: Option<usize>,
}

/// The block a seek calls when it is over, finished or not, with the count of seeks over. It
/// lives for good: the player may call it after the `Player` is gone (some 40 bytes for each
/// video opened).
type Seeked = Block<AtomicU32>;

/// A new block counting seeks over.
fn seeked() -> &'static Seeked {
    Block::leaked(seek_over as *const c_void, AtomicU32::new(0))
}

/// A seek is over.
extern "C" fn seek_over(block: *const Seeked, _finished: Bool) {
    // SAFETY: the player calls the block with itself, which lives for good.
    unsafe { (*block).held.fetch_add(1, Ordering::Release) };
}

impl Player {
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let functions = functions().ok_or_else(|| VideoError::NoPlayer("AVFoundation is missing".into()))?;
        let _pool = Pool::new();
        let (url, written) = url_of(source)?;
        // SAFETY: each class answers these messages; what is kept is retained, the rest
        // autoreleased into the pool. 'BGRA' is the pixel format asked.
        unsafe {
            let item: Obj = send(class(c"AVPlayerItem"), c"playerItemWithURL:", (url,));
            if item.is_null() {
                return Err(VideoError::Refused("AVFoundation made no item of it".into()));
            }
            let format: Obj = send(class(c"NSNumber"), c"numberWithUnsignedInt:", (0x4247_5241u32,));
            let wanted: Obj = send(class(c"NSDictionary"), c"dictionaryWithObject:forKey:", (format, functions.format_key));
            let output: Obj = send(send::<Obj>(class(c"AVPlayerItemVideoOutput"), c"alloc", ()), c"initWithPixelBufferAttributes:", (wanted,));
            send::<()>(item, c"addOutput:", (output,));
            let player: Obj = send(send::<Obj>(class(c"AVPlayer"), c"alloc", ()), c"initWithPlayerItem:", (item,));
            send::<Obj>(item, c"retain", ());
            send::<()>(player, c"setMuted:", (!sound as Bool,));
            #[cfg(target_os = "ios")]
            if sound {
                plays_sound();
            }
            send::<()>(item, c"setAudioTimePitchAlgorithm:", (functions.spectral,));
            Ok(Player { functions, player, item, output, playing: false, ready: false, last: None, written, seeks: 0, seeked: seeked(), speed: 1.0, cues: Vec::new(), captions: None })
        }
    }

    /// Plays at its speed (a rate of 0 is paused).
    pub(crate) fn play(&mut self) {
        self.playing = true;
        // SAFETY: a live player.
        unsafe { send::<()>(self.player, c"setRate:", (self.speed,)) };
    }
    pub(crate) fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        if self.playing {
            self.play();
        }
    }
    pub(crate) fn pause(&mut self) {
        self.playing = false;
        // SAFETY: a live player.
        unsafe { send::<()>(self.player, c"pause", ()) };
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        (self.last, self.seeks) = (None, self.seeks.wrapping_add(1));
        let exact = CMTime { value: 0, timescale: 1, flags: 1, epoch: 0 };
        let over: *const Seeked = self.seeked;
        let _pool = Pool::new();
        // SAFETY: a live player; the times are passed as the method takes them, and the block
        // lives for good.
        unsafe { send::<()>(self.player, c"seekToTime:toleranceBefore:toleranceAfter:completionHandler:", (CMTime::of(to), exact, exact, over)) };
    }
    pub(crate) fn position(&mut self) -> Duration {
        // SAFETY: a live player.
        unsafe { send_large::<CMTime>(self.player, c"currentTime", ()) }.known().unwrap_or_default()
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        // SAFETY: a live item.
        unsafe { send_large::<CMTime>(self.item, c"duration", ()) }.known()
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        // SAFETY: a live player.
        unsafe { send::<()>(self.player, c"setVolume:", (volume,)) };
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        // SAFETY: a live player.
        unsafe { send::<()>(self.player, c"setMuted:", (muted as Bool,)) };
    }

    /// The end, as playing stopped at it; an error, as the item failed. Ready, the first
    /// frame is asked for by a seek where it stands.
    pub(crate) fn news(&mut self) -> News {
        let _pool = Pool::new();
        // SAFETY: live objects; the error's words are copied.
        let (status, rate): (isize, f32) = unsafe { (send(self.item, c"status", ()), send(self.player, c"rate", ())) };
        let error = (status == 2).then(|| {
            // SAFETY: as above.
            let words = unsafe { rust_string(send(send::<Obj>(self.item, c"error", ()), c"localizedDescription", ())) };
            format!("the player stopped: {}", words.unwrap_or_else(|| "an error".into()))
        });
        if status == 1 && !self.ready {
            self.ready = true;
            let here = self.position();
            self.seek(here);
        }
        let at_end = self.length().is_some_and(|length| self.position() + Duration::from_millis(1) >= length);
        let ended = self.playing && rate == 0.0 && at_end;
        self.playing &= !ended;
        News { ended, error, clears_on_play: false }
    }

    /// The newest frame where there is one not given yet: its width and time, its pixels in
    /// `into`. None before the item is ready (its length known) and its first seek is over.
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        if !self.ready || self.seeked.held.load(Ordering::Acquire) != self.seeks {
            return None;
        }
        let _pool = Pool::new();
        let mut shown = CMTime::default();
        // SAFETY: live objects; the buffer copied is ours and released.
        let buffer: *mut c_void = unsafe {
            let now: CMTime = send_large(self.player, c"currentTime", ());
            if send::<Bool>(self.output, c"hasNewPixelBufferForItemTime:", (now,)) == 0 {
                return None;
            }
            send(self.output, c"copyPixelBufferForItemTime:itemTimeForDisplay:", (now, &mut shown as *mut CMTime))
        };
        if buffer.is_null() {
            return None;
        }
        let width = self.read(buffer, into);
        // SAFETY: copied, so ours.
        unsafe { (self.functions.release)(buffer) };
        let at = shown.known().unwrap_or_default();
        if width == 0 || self.last == Some(shown.value) {
            return None;
        }
        self.last = Some(shown.value);
        Some((width, at))
    }
    /// A BGRA pixel buffer's pixels, opaque, into `into`; its width.
    fn read(&self, buffer: *mut c_void, into: &mut Vec<u32>) -> usize {
        let f = self.functions;
        // SAFETY: the buffer is locked for reading while its rows are copied.
        unsafe {
            if (f.lock)(buffer, 1) != 0 {
                return 0;
            }
            let (base, row, width, height) = ((f.base)(buffer), (f.row)(buffer), (f.width)(buffer), (f.height)(buffer));
            into.clear();
            if !base.is_null() && row >= width * 4 {
                let bytes = std::slice::from_raw_parts(base, row * height);
                into.extend(bytes.chunks(row).flat_map(|line| line[..width * 4].as_chunks::<4>().0).map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0xFF])));
            }
            (f.unlock)(buffer, 1);
            if into.is_empty() { 0 } else { width }
        }
    }

    // ---- tracks ----

    /// The item's tracks of these media types (`AVPlayerItemTrack`s, autoreleased).
    fn item_tracks(&self, kinds: &[&str]) -> Vec<Obj> {
        // SAFETY: a live item; each track answers `assetTrack`, which answers `mediaType`.
        let kind = |track: &Obj| unsafe { rust_string(send(send::<Obj>(*track, c"assetTrack", ()), c"mediaType", ())) };
        // SAFETY: as above.
        let all = array_items(unsafe { send(self.item, c"tracks", ()) });
        all.into_iter().filter(|track| kind(track).is_some_and(|kind| kinds.contains(&kind.as_str()))).collect()
    }

    /// The item's sound and caption tracks, once it is ready; the caption tracks' cues are read
    /// then.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        if !self.ready {
            return None;
        }
        let _pool = Pool::new();
        let sound = self.item_tracks(&["soun"]);
        let captions = self.item_tracks(&["sbtl", "text"]);
        // SAFETY: live tracks.
        let chosen = sound.iter().position(|track| unsafe { send::<Bool>(*track, c"isEnabled", ()) } != 0);
        self.cues = captions.iter().map(|track| self.cues_of(*track)).collect();
        Some(Tracks { sound: sound.iter().map(|track| track_of(*track)).collect(), captions: captions.iter().map(|track| track_of(*track)).collect(), sound_chosen: chosen })
    }

    /// Plays the `nth` sound track, and no other.
    pub(crate) fn choose_sound(&mut self, nth: usize) {
        let _pool = Pool::new();
        for (at, track) in self.item_tracks(&["soun"]).into_iter().enumerate() {
            // SAFETY: a live track.
            unsafe { send::<()>(track, c"setEnabled:", ((at == nth) as Bool,)) };
        }
    }
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        self.captions = nth;
    }
    /// The chosen caption track's text on show.
    pub(crate) fn caption(&mut self) -> Option<String> {
        let at = self.position();
        crate::video_captions::text_at(self.cues.get(self.captions?)?, at)
    }

    /// A caption track's cues, read from its 3GPP timed text samples; none where they can't be
    /// read (a stream).
    fn cues_of(&self, track: Obj) -> Vec<Cue> {
        let mut cues = Vec::new();
        let Some(output) = self.samples_of(track) else { return cues };
        loop {
            // SAFETY: a reading output; the sample buffer it copies is ours, read and released.
            let sample: *mut c_void = unsafe { send(output, c"copyNextSampleBuffer", ()) };
            if sample.is_null() {
                break;
            }
            cues.extend(self.cue_of(sample));
            // SAFETY: as above.
            unsafe { crate::macos_sys::CFRelease(sample) };
        }
        cues.sort_by_key(|cue| cue.from);
        cues
    }

    /// An `AVAssetReader`'s output of a track's samples as stored (no settings), reading
    /// (autoreleased, with its reader); none where they can't be read.
    fn samples_of(&self, track: Obj) -> Option<Obj> {
        // SAFETY: live objects; what is made is autoreleased into the caller's pool.
        unsafe {
            let (asset, asset_track): (Obj, Obj) = (send(self.item, c"asset", ()), send(track, c"assetTrack", ()));
            let reader: Obj = send(class(c"AVAssetReader"), c"assetReaderWithAsset:error:", (asset, std::ptr::null_mut::<Obj>()));
            let output: Obj = send(class(c"AVAssetReaderTrackOutput"), c"assetReaderTrackOutputWithTrack:outputSettings:", (asset_track, std::ptr::null_mut::<c_void>()));
            if reader.is_null() || output.is_null() {
                return None;
            }
            send::<()>(reader, c"addOutput:", (output,));
            (send::<Bool>(reader, c"startReading", ()) != 0).then_some(output)
        }
    }

    /// A 3GPP timed text sample buffer's cue: its text, at its time, for its duration; none
    /// where it is empty.
    fn cue_of(&self, sample: *mut c_void) -> Option<Cue> {
        let f = self.functions;
        // SAFETY: a live sample buffer; its data is copied within its length.
        unsafe {
            let (from, length) = ((f.sample_time)(sample).known()?, (f.sample_duration)(sample).known());
            let data = (f.sample_data)(sample);
            let mut bytes = vec![0; if data.is_null() { 0 } else { (f.data_length)(data) }];
            if data.is_null() || (f.copy_data)(data, 0, bytes.len(), bytes.as_mut_ptr()) != 0 {
                return None;
            }
            Some(Cue { from, to: from + length.unwrap_or(Duration::MAX - from), text: timed_text(&bytes)? })
        }
    }
}

/// A live `AVPlayerItemTrack` as the app sees it: its language (BCP 47, else ISO 639).
fn track_of(track: Obj) -> Track {
    // SAFETY: a live track; the strings are copied.
    let language = unsafe {
        let asset_track: Obj = send(track, c"assetTrack", ());
        rust_string(send(asset_track, c"extendedLanguageTag", ())).or_else(|| rust_string(send(asset_track, c"languageCode", ())))
    };
    Track { language: language.filter(|language| language != "und").unwrap_or_default(), name: String::new() }
}

/// A 3GPP timed text sample's text, after its length; none where it is empty.
fn timed_text(sample: &[u8]) -> Option<String> {
    let [high, low, rest @ ..] = sample else { return None };
    let text = &rest[..(usize::from(*high) << 8 | usize::from(*low)).min(rest.len())];
    Some(crate::video_captions::plain(String::from_utf8_lossy(text).trim())).filter(|text| !text.is_empty())
}

/// How captions look, as the system's caption settings say (MediaAccessibility's caption
/// appearance, the user's): colours, their opacity, the size against the usual.
pub(crate) fn caption_style() -> CaptionStyle {
    caption_appearance().unwrap_or_default()
}

fn caption_appearance() -> Option<CaptionStyle> {
    type Colour = unsafe extern "C" fn(isize, *mut isize) -> *const c_void;
    type Amount = unsafe extern "C" fn(isize, *mut isize) -> f64;
    let open = |path: &CStr| {
        // SAFETY: a path ending in a zero; 1 is `RTLD_LAZY`.
        unsafe { dlopen(path.as_ptr(), 1) }
    };
    let library = open(c"/System/Library/Frameworks/MediaAccessibility.framework/MediaAccessibility");
    let graphics = open(c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics");
    let found = |library: *mut c_void, name: &CStr| {
        // SAFETY: a name ending in a zero, of an open library (or none, which finds nothing).
        let at = unsafe { dlsym(library, name.as_ptr()) };
        (!library.is_null() && !at.is_null()).then_some(at)
    };
    // SAFETY: each is the function of that name, of the type its header gives; the colours
    // copied are read and released; 1 is `kMACaptionAppearanceDomainUser`.
    unsafe {
        let (text, background): (Colour, Colour) = (function(found(library, c"MACaptionAppearanceCopyForegroundColor")?), function(found(library, c"MACaptionAppearanceCopyBackgroundColor")?));
        let (text_alpha, background_alpha, size): (Amount, Amount, Amount) = (
            function(found(library, c"MACaptionAppearanceGetForegroundOpacity")?),
            function(found(library, c"MACaptionAppearanceGetBackgroundOpacity")?),
            function(found(library, c"MACaptionAppearanceGetRelativeCharacterSize")?),
        );
        let count: unsafe extern "C" fn(*const c_void) -> usize = function(found(graphics, c"CGColorGetNumberOfComponents")?);
        let parts: unsafe extern "C" fn(*const c_void) -> *const f64 = function(found(graphics, c"CGColorGetComponents")?);
        let colour = |made: *const c_void, alpha: f64| {
            let channels = if made.is_null() { &[][..] } else { std::slice::from_raw_parts(parts(made), count(made)) };
            let grey = |at: usize| channels.get(at).or(channels.first()).copied().unwrap_or(0.0);
            let [red, green, blue] = (if channels.len() >= 4 { [grey(0), grey(1), grey(2)] } else { [grey(0); 3] }).map(|part| (part.clamp(0.0, 1.0) * 255.0).round() as u32);
            if !made.is_null() {
                crate::macos_sys::CFRelease(made);
            }
            Color::rgb(red << 16 | green << 8 | blue).with_alpha(alpha as f32)
        };
        let null = std::ptr::null_mut();
        Some(CaptionStyle { text: colour(text(1, null), text_alpha(1, null)), background: colour(background(1, null), background_alpha(1, null)), scale: size(1, null) as f32 })
    }
}

/// iOS: a video with sound plays as video apps do (the audio session's category `Playback`:
/// heard with the ring switch on silent), unless the app chose a category of its own.
#[cfg(target_os = "ios")]
fn plays_sound() {
    // SAFETY: the shared audio session answers these; the strings are constants.
    unsafe {
        let session: Obj = send(class(c"AVAudioSession"), c"sharedInstance", ());
        let category = rust_string(send(session, c"category", ()));
        if category.as_deref() == Some("AVAudioSessionCategorySoloAmbient") {
            send::<Bool>(session, c"setCategory:error:", (ns_string("AVAudioSessionCategoryPlayback"), std::ptr::null_mut::<Obj>()));
        }
    }
}

/// A source as an `NSURL` (autoreleased): bytes are first written to a file of their own.
fn url_of(source: VideoSource) -> Result<(Obj, Option<PathBuf>), VideoError> {
    let (text, file, written) = match source {
        VideoSource::File(path) => (std::path::absolute(&path).unwrap_or(path).display().to_string(), true, None),
        VideoSource::Url(url) => (url, false, None),
        VideoSource::Bytes(bytes) => {
            let path = std::env::temp_dir().join(format!("taut-video-{}-{}.{}", std::process::id(), bytes.len(), crate::video::kind_of(&bytes)));
            std::fs::write(&path, &bytes).map_err(|why| VideoError::Refused(why.to_string()))?;
            (path.display().to_string(), true, Some(path))
        }
    };
    let made = if file { c"fileURLWithPath:" } else { c"URLWithString:" };
    // SAFETY: `NSURL` answers both with an autoreleased URL, or nil.
    let url: Obj = unsafe { send(class(c"NSURL"), made, (ns_string(&text),)) };
    match url.is_null() {
        true => Err(VideoError::Refused(format!("{text} is no address"))),
        false => Ok((url, written)),
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // (What AVFoundation autoreleases here goes at once, not when the run loop next turns.)
        let _pool = Pool::new();
        // SAFETY: the player stops and lets its item go; all three are released once.
        unsafe {
            send::<()>(self.player, c"pause", ());
            send::<()>(self.player, c"replaceCurrentItemWithPlayerItem:", (std::ptr::null_mut::<c_void>(),));
            for object in [self.output, self.item, self.player] {
                send::<()>(object, c"release", ());
            }
        }
        if let Some(path) = self.written.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}
