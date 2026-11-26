//! Videos on Android: the NDK's media C API (`libmediandk.so`), opened at run time. An
//! extractor reads the source; the system's decoder decodes its video into an image reader
//! (YUV 4:2:0, made RGB here) and, where sound is asked for, its sound for AAudio
//! (`libaaudio.so`). Video and sound each run on a thread of their own, kept to one clock: the
//! sound's while it plays. No Java: it plays in an app and in a command-line program alike
//! (addresses only in an app, where Android's network stack is).

use crate::color::Color;
use crate::video::{CaptionStyle, News, Tracks, VideoError, VideoSource};
use crate::video_captions::Cue;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Object = *mut c_void;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    fn clock_gettime(clock: c_int, time: *mut [c_long; 2]) -> c_int;
}

/// Declares the functions taken from a library, each found by its own name when the library is
/// first asked for; none where it or one of them is missing.
macro_rules! library {
    ($name:ident, $file:literal, { $(fn $f:ident($($t:ty),*) $(-> $r:ty)?;)* }) => {
        #[allow(non_snake_case)]
        struct $name { $($f: unsafe extern "C" fn($($t),*) $(-> $r)?,)* }
        impl $name {
            /// The library's functions, opened once and kept for the program's life.
            fn get() -> Option<&'static $name> {
                static OPENED: OnceLock<Option<$name>> = OnceLock::new();
                OPENED.get_or_init($name::open).as_ref()
            }
            fn open() -> Option<$name> {
                let library = open_library($file)?;
                // SAFETY: each function is declared as its header declares it.
                unsafe { Some($name { $($f: function(library, concat!(stringify!($f), "\0"))?,)* }) }
            }
        }
    };
}

/// A system library, opened for good.
fn open_library(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: a name ending in a zero; 2 is `RTLD_NOW`.
    let library = unsafe { dlopen(name.as_ptr(), 2) };
    (!library.is_null()).then_some(library)
}

/// A library's function by name (ending in a zero), as type `F`.
///
/// # Safety
/// `F` is the function's type, as its header declares it.
unsafe fn function<F: Copy>(library: *mut c_void, name: &str) -> Option<F> {
    // SAFETY: an open library, a name ending in a zero.
    let found = unsafe { dlsym(library, name.as_ptr().cast()) };
    // SAFETY: as the caller promises; a function pointer is one pointer, as `F` is.
    (!found.is_null()).then(|| unsafe { std::mem::transmute_copy(&found) })
}

library!(Media, c"libmediandk.so", {
    fn AMediaExtractor_new() -> Object;
    fn AMediaExtractor_delete(Object) -> i32;
    fn AMediaExtractor_setDataSourceFd(Object, c_int, i64, i64) -> i32;
    fn AMediaExtractor_setDataSource(Object, *const c_char) -> i32;
    fn AMediaExtractor_setDataSourceCustom(Object, Object) -> i32;
    fn AMediaExtractor_getTrackCount(Object) -> usize;
    fn AMediaExtractor_getTrackFormat(Object, usize) -> Object;
    fn AMediaExtractor_selectTrack(Object, usize) -> i32;
    fn AMediaExtractor_readSampleData(Object, *mut u8, usize) -> isize;
    fn AMediaExtractor_getSampleTime(Object) -> i64;
    fn AMediaExtractor_advance(Object) -> bool;
    fn AMediaExtractor_seekTo(Object, i64, c_int) -> i32;
    fn AMediaFormat_delete(Object) -> i32;
    fn AMediaFormat_getInt32(Object, *const c_char, *mut i32) -> bool;
    fn AMediaFormat_getInt64(Object, *const c_char, *mut i64) -> bool;
    fn AMediaFormat_getString(Object, *const c_char, *mut *const c_char) -> bool;
    fn AMediaCodec_createDecoderByType(*const c_char) -> Object;
    fn AMediaCodec_configure(Object, Object, Object, Object, u32) -> i32;
    fn AMediaCodec_start(Object) -> i32;
    fn AMediaCodec_stop(Object) -> i32;
    fn AMediaCodec_flush(Object) -> i32;
    fn AMediaCodec_delete(Object) -> i32;
    fn AMediaCodec_dequeueInputBuffer(Object, i64) -> isize;
    fn AMediaCodec_getInputBuffer(Object, usize, *mut usize) -> *mut u8;
    fn AMediaCodec_queueInputBuffer(Object, usize, c_long, usize, u64, u32) -> i32;
    fn AMediaCodec_dequeueOutputBuffer(Object, *mut BufferInfo, i64) -> isize;
    fn AMediaCodec_getOutputBuffer(Object, usize, *mut usize) -> *mut u8;
    fn AMediaCodec_getOutputFormat(Object) -> Object;
    fn AMediaCodec_releaseOutputBuffer(Object, usize, bool) -> i32;
    fn AMediaDataSource_new() -> Object;
    fn AMediaDataSource_delete(Object);
    fn AMediaDataSource_setUserdata(Object, *mut c_void);
    fn AMediaDataSource_setReadAt(Object, unsafe extern "C" fn(*mut c_void, i64, *mut c_void, usize) -> isize);
    fn AMediaDataSource_setGetSize(Object, unsafe extern "C" fn(*mut c_void) -> isize);
    fn AImageReader_new(i32, i32, i32, i32, *mut Object) -> i32;
    fn AImageReader_delete(Object);
    fn AImageReader_getWindow(Object, *mut Object) -> i32;
    fn AImageReader_acquireNextImage(Object, *mut Object) -> i32;
    fn AImage_delete(Object);
    fn AImage_getTimestamp(Object, *mut i64) -> i32;
    fn AImage_getWidth(Object, *mut i32) -> i32;
    fn AImage_getHeight(Object, *mut i32) -> i32;
    fn AImage_getCropRect(Object, *mut [i32; 4]) -> i32;
    fn AImage_getPlaneRowStride(Object, i32, *mut i32) -> i32;
    fn AImage_getPlanePixelStride(Object, i32, *mut i32) -> i32;
    fn AImage_getPlaneData(Object, i32, *mut *mut u8, *mut i32) -> i32;
});

library!(Audio, c"libaaudio.so", {
    fn AAudio_createStreamBuilder(*mut Object) -> i32;
    fn AAudioStreamBuilder_setFormat(Object, i32);
    fn AAudioStreamBuilder_setChannelCount(Object, i32);
    fn AAudioStreamBuilder_setSampleRate(Object, i32);
    fn AAudioStreamBuilder_setUsage(Object, i32);
    fn AAudioStreamBuilder_setContentType(Object, i32);
    fn AAudioStreamBuilder_openStream(Object, *mut Object) -> i32;
    fn AAudioStreamBuilder_delete(Object) -> i32;
    fn AAudioStream_requestStart(Object) -> i32;
    fn AAudioStream_requestPause(Object) -> i32;
    fn AAudioStream_requestFlush(Object) -> i32;
    fn AAudioStream_close(Object) -> i32;
    fn AAudioStream_write(Object, *const c_void, i32, i64) -> i32;
    fn AAudioStream_getTimestamp(Object, c_int, *mut i64, *mut i64) -> i32;
    fn AAudioStream_getFramesWritten(Object) -> i64;
});

/// `AMediaCodecBufferInfo`.
#[repr(C)]
#[derive(Default)]
struct BufferInfo {
    offset: i32,
    size: i32,
    time_us: i64,
    flags: u32,
}

/// From the NDK's headers: a decoder's last buffer, and its answers "nothing yet", "a new
/// format" and "new buffers" (`NdkMediaCodec.h`); the extractor's seek to the key frame before
/// (`NdkMediaExtractor.h`); the image reader's YUV 4:2:0 (`NdkImage.h`); AAudio's 16-bit samples,
/// media use and film sound (`AAudio.h`); float samples (`AMEDIAFORMAT_KEY_PCM_ENCODING`);
/// `CLOCK_MONOTONIC`.
const END_OF_STREAM: u32 = 4;
const TRY_AGAIN_LATER: isize = -1;
const OUTPUT_FORMAT_CHANGED: isize = -2;
const OUTPUT_BUFFERS_CHANGED: isize = -3;
const SEEK_PREVIOUS_SYNC: c_int = 0;
const YUV_420_888: i32 = 0x23;
const PCM_16: i32 = 1;
const USAGE_MEDIA: i32 = 1;
const CONTENT_MOVIE: i32 = 3;
const PCM_FLOAT: i32 = 4;
const MONOTONIC: c_int = 1;
/// The caption tracks it reads: 3GPP timed text (MP4), SRT (Matroska), WebVTT (WebM).
const CAPTIONS: [&str; 3] = ["text/3gpp-tt", "application/x-subrip", "text/vtt"];

fn micros(time: Duration) -> i64 {
    time.as_micros().min(i64::MAX as u128) as i64
}

// ---- reading and decoding a track ----

/// Where the media comes from. Each extractor opens it anew (a file's offset is its own).
#[derive(Clone)]
enum Origin {
    File(PathBuf),
    Address(CString),
    Bytes(Arc<Vec<u8>>),
}

/// What keeps an extractor's source alive while it reads: the open file, or the data source
/// and the bytes it reads.
#[derive(Default)]
struct Held {
    _file: Option<std::fs::File>,
    source: Option<Object>,
    _bytes: Option<Arc<Vec<u8>>>,
}

/// A track of a source: its number, type and language.
#[derive(Clone)]
struct Found {
    at: usize,
    mime: String,
    language: String,
}

/// What a source holds: its first video track (number, type, size), its sound and caption
/// tracks, and the longest track's length.
struct Contents {
    video: Option<(usize, String, [i32; 2])>,
    sound: Vec<Found>,
    captions: Vec<Found>,
    length: Option<Duration>,
}

impl Contents {
    /// The tracks as the app sees them, the first sound track playing.
    fn tracks(&self) -> Tracks {
        let track = |found: &Found| crate::video::Track { language: found.language.clone(), name: String::new() };
        let sound: Vec<_> = self.sound.iter().map(track).collect();
        Tracks { sound_chosen: (!sound.is_empty()).then_some(0), sound, captions: self.captions.iter().map(track).collect() }
    }
}

/// An extractor of a source, and the decoder of one of its tracks.
struct Track {
    media: &'static Media,
    extractor: Object,
    held: Held,
    codec: Object,
    /// The samples ran out, and the decoder was told.
    fed: bool,
}

// SAFETY: the NDK's extractors and decoders may be used from any thread; a track is used by
// one thread at a time (made on one, then moved to its own).
unsafe impl Send for Track {}

impl Track {
    /// An extractor of the source, no track decoded yet, and what it holds.
    fn read(media: &'static Media, origin: &Origin) -> Result<(Track, Contents), String> {
        // SAFETY: a new extractor, deleted with the track.
        let extractor = unsafe { (media.AMediaExtractor_new)() };
        if extractor.is_null() {
            return Err("no extractor".into());
        }
        let mut track = Track { media, extractor, held: Held::default(), codec: null_mut(), fed: false };
        track.held = open_source(media, extractor, origin)?;
        Ok((track, contents(media, extractor)))
    }

    /// Decodes the track at `at`, of type `mime`: a video into `window`, sound into buffers.
    fn decode(&mut self, at: usize, mime: &str, window: Object) -> Result<(), String> {
        let m = self.media;
        let named = CString::new(mime).map_err(|_| format!("a track of type {mime:?}"))?;
        // SAFETY: a live extractor; the decoder is the track's; the format is deleted once the
        // decoder has it; the window outlives the decoder (the track drops first).
        unsafe {
            (m.AMediaExtractor_selectTrack)(self.extractor, at);
            self.codec = (m.AMediaCodec_createDecoderByType)(named.as_ptr());
            if self.codec.is_null() {
                return Err(format!("no decoder for {mime}"));
            }
            let format = (m.AMediaExtractor_getTrackFormat)(self.extractor, at);
            let configured = (m.AMediaCodec_configure)(self.codec, format, window, null_mut(), 0);
            (m.AMediaFormat_delete)(format);
            if configured != 0 || (m.AMediaCodec_start)(self.codec) != 0 {
                return Err(format!("the decoder for {mime} refused it ({configured})"));
            }
        }
        Ok(())
    }

    /// Goes to the key frame at or before `to`; what the decoder held is dropped.
    fn seek(&mut self, to: Duration) {
        self.fed = false;
        // SAFETY: a live extractor and a started decoder.
        unsafe {
            (self.media.AMediaExtractor_seekTo)(self.extractor, micros(to), SEEK_PREVIOUS_SYNC);
            (self.media.AMediaCodec_flush)(self.codec);
        }
    }

    /// Hands the decoder samples while it has room; once they run out, the end.
    fn feed(&mut self) {
        while !self.fed && self.feed_one() {}
    }
    fn feed_one(&mut self) -> bool {
        let m = self.media;
        // SAFETY: a started decoder, whose lent buffer is written within its room.
        unsafe {
            let Ok(at) = usize::try_from((m.AMediaCodec_dequeueInputBuffer)(self.codec, 0)) else { return false };
            let mut room = 0;
            let buffer = (m.AMediaCodec_getInputBuffer)(self.codec, at, &mut room);
            let size = (m.AMediaExtractor_readSampleData)(self.extractor, buffer, room);
            let time = (m.AMediaExtractor_getSampleTime)(self.extractor);
            self.fed = size < 0;
            let flags = if self.fed { END_OF_STREAM } else { 0 };
            (m.AMediaCodec_queueInputBuffer)(self.codec, at, 0, size.max(0) as usize, time.max(0) as u64, flags);
            (m.AMediaExtractor_advance)(self.extractor);
        }
        true
    }

    /// The decoder's next output, waiting up to `wait`: its buffer's number and what it holds;
    /// none yet.
    fn output(&mut self, wait: Duration) -> Result<Option<(usize, BufferInfo)>, String> {
        let mut info = BufferInfo::default();
        // SAFETY: a started decoder, writing the info.
        let at = unsafe { (self.media.AMediaCodec_dequeueOutputBuffer)(self.codec, &mut info, micros(wait)) };
        match at {
            0.. => Ok(Some((at as usize, info))),
            // (A new format or new buffers are read when used.)
            TRY_AGAIN_LATER | OUTPUT_FORMAT_CHANGED | OUTPUT_BUFFERS_CHANGED => Ok(None),
            failed => Err(format!("the decoder failed ({failed})")),
        }
    }

    /// The bytes of an output buffer.
    fn bytes(&self, at: usize, info: &BufferInfo) -> &[u8] {
        let mut size = 0;
        // SAFETY: a buffer the decoder lent and that is not given back yet, read within its size.
        unsafe {
            let base = (self.media.AMediaCodec_getOutputBuffer)(self.codec, at, &mut size);
            let (from, count) = (info.offset.max(0) as usize, info.size.max(0) as usize);
            if base.is_null() || from + count > size {
                return &[];
            }
            std::slice::from_raw_parts(base.add(from), count)
        }
    }

    /// Gives an output buffer back, shown (into the window) or not.
    fn release(&self, at: usize, show: bool) {
        // SAFETY: a buffer the decoder lent and that is not given back yet.
        unsafe { (self.media.AMediaCodec_releaseOutputBuffer)(self.codec, at, show) };
    }

    /// A number of the decoder's output format.
    fn output_number(&self, key: &CStr) -> Option<i32> {
        // SAFETY: a started decoder; its format is deleted once read.
        unsafe {
            let format = (self.media.AMediaCodec_getOutputFormat)(self.codec);
            if format.is_null() {
                return None;
            }
            let found = number(self.media, format, key);
            (self.media.AMediaFormat_delete)(format);
            found
        }
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        let m = self.media;
        // SAFETY: the decoder and extractor are the track's, used no more; a data source
        // outlives its extractor.
        unsafe {
            if !self.codec.is_null() {
                (m.AMediaCodec_stop)(self.codec);
                (m.AMediaCodec_delete)(self.codec);
            }
            (m.AMediaExtractor_delete)(self.extractor);
            if let Some(source) = self.held.source {
                (m.AMediaDataSource_delete)(source);
            }
        }
    }
}

/// Points the extractor at the source: what keeps the source alive.
fn open_source(media: &'static Media, extractor: Object, origin: &Origin) -> Result<Held, String> {
    match origin {
        Origin::File(path) => {
            let file = std::fs::File::open(path).map_err(|why| format!("{}: {why}", path.display()))?;
            let length = file.metadata().map_or(0, |metadata| metadata.len() as i64);
            // SAFETY: a live extractor and an open file, kept open as long as the extractor.
            let status = unsafe { (media.AMediaExtractor_setDataSourceFd)(extractor, file.as_raw_fd(), 0, length) };
            read_or_not(status, Held { _file: Some(file), ..Held::default() })
        }
        Origin::Address(address) => {
            // SAFETY: a live extractor and an address ending in a zero.
            let status = unsafe { (media.AMediaExtractor_setDataSource)(extractor, address.as_ptr()) };
            read_or_not(status, Held::default())
        }
        Origin::Bytes(bytes) => bytes_source(media, extractor, bytes),
    }
}

/// The extractor reads bytes in memory through a data source of its own.
fn bytes_source(media: &'static Media, extractor: Object, bytes: &Arc<Vec<u8>>) -> Result<Held, String> {
    // SAFETY: a new data source whose callbacks read the bytes; both are kept in `Held` until
    // the extractor is gone.
    unsafe {
        let source = (media.AMediaDataSource_new)();
        if source.is_null() {
            return Err("no data source".into());
        }
        (media.AMediaDataSource_setUserdata)(source, Arc::as_ptr(bytes).cast_mut().cast());
        (media.AMediaDataSource_setReadAt)(source, read_at);
        (media.AMediaDataSource_setGetSize)(source, size_of_bytes);
        let held = Held { source: Some(source), _bytes: Some(Arc::clone(bytes)), ..Held::default() };
        let status = (media.AMediaExtractor_setDataSourceCustom)(extractor, source);
        if status != 0 {
            (media.AMediaDataSource_delete)(source);
        }
        read_or_not(status, held)
    }
}

/// A data source's read: up to `size` bytes from `offset`; 0 at the end, -1 past it. (The
/// header says -1 at the end, but Android 15's extractor takes that as a failed read and plays
/// nothing: 0 is what it reads to the end by.)
unsafe extern "C" fn read_at(bytes: *mut c_void, offset: i64, into: *mut c_void, size: usize) -> isize {
    // SAFETY: the user data is the bytes, alive while the source is.
    let bytes: &Vec<u8> = unsafe { &*bytes.cast() };
    let Some(part) = usize::try_from(offset).ok().and_then(|from| bytes.get(from..)) else { return -1 };
    let count = part.len().min(size);
    // SAFETY: the extractor's buffer has room for `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(part.as_ptr(), into.cast(), count) };
    count as isize
}

unsafe extern "C" fn size_of_bytes(bytes: *mut c_void) -> isize {
    // SAFETY: as in `read_at`.
    unsafe { (*bytes.cast::<Vec<u8>>()).len() as isize }
}

/// The extractor's answer: `held` where it read the source.
fn read_or_not(status: i32, held: Held) -> Result<Held, String> {
    match status {
        0 => Ok(held),
        status => Err(format!("the extractor can't read it ({status})")),
    }
}

/// What the extractor found.
fn contents(media: &'static Media, extractor: Object) -> Contents {
    let mut found = Contents { video: None, sound: Vec::new(), captions: Vec::new(), length: None };
    // SAFETY: a live extractor.
    let count = unsafe { (media.AMediaExtractor_getTrackCount)(extractor) };
    for at in 0..count {
        // SAFETY: a track it has; its format is deleted once read.
        let format = unsafe { (media.AMediaExtractor_getTrackFormat)(extractor, at) };
        let length = long(media, format, c"durationUs").and_then(|us| u64::try_from(us).ok()).map(Duration::from_micros);
        found.length = found.length.max(length);
        let size = [c"width", c"height"].map(|key| number(media, format, key).unwrap_or(0));
        let language = string(media, format, c"language").filter(|language| language != "und").unwrap_or_default();
        match string(media, format, c"mime") {
            Some(mime) if mime.starts_with("video/") && found.video.is_none() => found.video = Some((at, mime, size)),
            Some(mime) if mime.starts_with("audio/") => found.sound.push(Found { at, mime, language }),
            Some(mime) if CAPTIONS.contains(&mime.as_str()) => found.captions.push(Found { at, mime, language }),
            _ => {}
        }
        // SAFETY: read, used no more.
        unsafe { (media.AMediaFormat_delete)(format) };
    }
    found
}

fn string(media: &'static Media, format: Object, key: &CStr) -> Option<String> {
    let mut value: *const c_char = std::ptr::null();
    // SAFETY: a live format; the string it lends is copied at once.
    let found = unsafe { (media.AMediaFormat_getString)(format, key.as_ptr(), &mut value) };
    // SAFETY: as above.
    (found && !value.is_null()).then(|| unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned())
}
fn number(media: &'static Media, format: Object, key: &CStr) -> Option<i32> {
    let mut value = 0;
    // SAFETY: a live format.
    unsafe { (media.AMediaFormat_getInt32)(format, key.as_ptr(), &mut value) }.then_some(value)
}
fn long(media: &'static Media, format: Object, key: &CStr) -> Option<i64> {
    let mut value = 0;
    // SAFETY: a live format.
    unsafe { (media.AMediaFormat_getInt64)(format, key.as_ptr(), &mut value) }.then_some(value)
}

// ---- what the threads share ----

/// The media's time: where it was at `since`, moving on from there at `speed` while it runs.
#[derive(Clone, Copy)]
struct Clock {
    at: Duration,
    since: Instant,
    running: bool,
    speed: f64,
}

impl Clock {
    fn now(&self) -> Duration {
        if self.running { self.at + self.since.elapsed().mul_f64(self.speed) } else { self.at }
    }
    fn set_running(&mut self, running: bool) {
        (self.at, self.since, self.running) = (self.now(), Instant::now(), running);
    }
    fn set(&mut self, at: Duration, since: Instant) {
        (self.at, self.since) = (at, since);
    }
    fn set_speed(&mut self, speed: f64) {
        (self.at, self.since, self.speed) = (self.now(), Instant::now(), speed);
    }
}

/// A frame made RGB: its width, pixels and time.
struct Frame {
    width: usize,
    pixels: Vec<u32>,
    at: Duration,
}

/// What the app's side and the threads share.
struct State {
    playing: bool,
    /// The last seek's time, and how many there were: each thread follows them.
    seek_to: Duration,
    seeks: u32,
    clock: Clock,
    length: Option<Duration>,
    volume: f32,
    muted: bool,
    /// The source's tracks once read, the sound track chosen, and each caption track's cues
    /// once read.
    tracks: Option<Tracks>,
    sound_track: usize,
    cues: Vec<Vec<Cue>>,
    /// The newest frame not given yet.
    frame: Option<Frame>,
    /// The sound played its last (or there is none).
    sound_done: bool,
    /// A sound thread plays.
    sounding: bool,
    ended: bool,
    error: Option<String>,
    stop: bool,
    /// (Tests: the sound last handed to the speaker, its first channel, and its rate.)
    #[cfg(test)]
    heard: (Vec<f32>, u32),
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Waits up to `time`, or until the app's side changes something.
    fn wait(&self, time: Duration) {
        let state = self.lock();
        if !state.stop {
            drop(self.changed.wait_timeout(state, time));
        }
    }
    /// The video's thread failed: playing stops, and the error is told.
    fn failed(&self, why: String) {
        let mut state = self.lock();
        state.error = Some(format!("the player stopped: {why}"));
        state.playing = false;
        state.clock.set_running(false);
    }
    /// Lets the thread rest until the player is dropped.
    fn rest(&self) {
        while !self.lock().stop {
            self.wait(Duration::from_millis(200));
        }
    }
}

/// What a thread acts on, as the app's side left it.
#[derive(Clone, Copy)]
struct Asked {
    playing: bool,
    seeks: u32,
    seek_to: Duration,
    now: Duration,
    volume: f32,
    speed: f64,
    sound_track: usize,
    stop: bool,
}

impl Shared {
    fn asked(&self) -> Asked {
        let state = self.lock();
        let volume = if state.muted { 0.0 } else { state.volume };
        let (playing, seeks, seek_to, now, speed) = (state.playing, state.seeks, state.seek_to, state.clock.now(), state.clock.speed);
        Asked { playing, seeks, seek_to, now, volume, speed, sound_track: state.sound_track, stop: state.stop }
    }
}

// ---- the player ----

/// A source played by the video's thread (which also keeps the time and tells the end), where
/// asked and there is sound, the sound's, and where there are caption tracks, a thread that reads
/// their cues.
pub(crate) struct Player {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    /// The caption track chosen.
    captions: Option<usize>,
}

impl Player {
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let media = Media::get().ok_or_else(|| VideoError::NoPlayer("the NDK's media library is missing".into()))?;
        let origin = match source {
            VideoSource::File(path) => Origin::File(path),
            VideoSource::Url(url) => Origin::Address(CString::new(url).map_err(|_| VideoError::Refused("an address with a zero in it".into()))?),
            VideoSource::Bytes(bytes) => Origin::Bytes(Arc::new(bytes)),
        };
        // A file or bytes are read at once, an address on the thread (it may take a while).
        let read = match origin {
            Origin::Address(_) => None,
            _ => Some(Track::read(media, &origin).map_err(VideoError::Refused)?),
        };
        let sounding = sound && read.as_ref().is_none_or(|(_, contents)| !contents.sound.is_empty());
        let captioned = read.as_ref().is_none_or(|(_, contents)| !contents.captions.is_empty());
        let mut state = State::new(sounding);
        state.tracks = read.as_ref().map(|(_, contents)| contents.tracks());
        let shared = Arc::new(Shared { state: Mutex::new(state), changed: Condvar::new() });
        let (video_origin, caption_origin) = (origin.clone(), origin.clone());
        let mut threads = vec![spawn(&shared, move |shared| video_thread(media, &video_origin, read, shared))];
        if sounding {
            threads.push(spawn(&shared, move |shared| sound_thread(media, &origin, shared)));
        }
        if captioned {
            threads.push(spawn(&shared, move |shared| caption_thread(media, &caption_origin, shared)));
        }
        Ok(Player { shared, threads, captions: None })
    }

    /// Changes what the threads share, and wakes them.
    fn change(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.shared.lock());
        self.shared.changed.notify_all();
    }

    pub(crate) fn play(&mut self) {
        self.change(|state| {
            state.playing = true;
            state.clock.set_running(true);
        });
    }
    pub(crate) fn pause(&mut self) {
        self.change(|state| {
            state.playing = false;
            state.clock.set_running(false);
        });
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        self.change(|state| {
            (state.seek_to, state.seeks, state.frame, state.ended) = (to, state.seeks.wrapping_add(1), None, false);
            state.sound_done = !state.sounding;
            state.clock.set(to, Instant::now());
        });
    }
    pub(crate) fn position(&mut self) -> Duration {
        let state = self.shared.lock();
        let now = state.clock.now();
        state.length.map_or(now, |length| now.min(length))
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        self.shared.lock().length
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        self.change(|state| state.volume = volume);
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        self.change(|state| state.muted = muted);
    }
    /// Plays at this speed: the clock moves so, and the sound is stretched to it.
    pub(crate) fn set_speed(&mut self, speed: f32) {
        self.change(|state| state.clock.set_speed(speed as f64));
    }
    pub(crate) fn news(&mut self) -> News {
        let mut state = self.shared.lock();
        News { ended: std::mem::take(&mut state.ended), error: state.error.take(), clears_on_play: false }
    }
    /// The newest frame not given yet: its width and time, its pixels in `into`.
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let frame = self.shared.lock().frame.take()?;
        *into = frame.pixels;
        Some((frame.width, frame.at))
    }

    /// The source's tracks, once read.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        self.shared.lock().tracks.take()
    }
    /// Plays the `nth` sound track: the sound's thread follows.
    pub(crate) fn choose_sound(&mut self, nth: usize) {
        self.change(|state| state.sound_track = nth);
    }
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        self.captions = nth;
    }
    /// The chosen caption track's text on show, once its cues are read.
    pub(crate) fn caption(&mut self) -> Option<String> {
        let at = self.position();
        let state = self.shared.lock();
        crate::video_captions::text_at(state.cues.get(self.captions?)?, at)
    }
}

impl State {
    fn new(sounding: bool) -> State {
        State {
            playing: false,
            seek_to: Duration::ZERO,
            seeks: 0,
            clock: Clock { at: Duration::ZERO, since: Instant::now(), running: false, speed: 1.0 },
            length: None,
            volume: 1.0,
            muted: false,
            tracks: None,
            sound_track: 0,
            cues: Vec::new(),
            frame: None,
            sound_done: !sounding,
            sounding,
            ended: false,
            error: None,
            stop: false,
            #[cfg(test)]
            heard: (Vec::new(), 0),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.change(|state| state.stop = true);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Starts one of the player's threads.
fn spawn(shared: &Arc<Shared>, run: impl FnOnce(&Shared) + Send + 'static) -> JoinHandle<()> {
    let shared = Arc::clone(shared);
    std::thread::spawn(move || run(&shared))
}

/// How captions look: as the system's caption settings say (Settings, Accessibility, Caption
/// preferences, through `CaptioningManager`) in an app; the usual in a command-line program.
pub(crate) fn caption_style() -> CaptionStyle {
    use crate::android_jni::service;
    let (Some(env), activity) = (crate::android_jni::env(), crate::android::activity()) else { return CaptionStyle::default() };
    if activity.is_null() {
        return CaptionStyle::default();
    }
    let colour = |argb: i32| Color::rgb(argb as u32 & 0xFF_FFFF).with_alpha((argb as u32 >> 24) as f32 / 255.0);
    env.frame(|| {
        let manager = service(env, activity, "captioning");
        if manager.is_null() {
            return CaptionStyle::default();
        }
        let style = env.call(manager, c"getUserStyle", c"()Landroid/view/accessibility/CaptioningManager$CaptionStyle;", &[]).object();
        let scale = env.call(manager, c"getFontScale", c"()F", &[]).as_float();
        CaptionStyle { text: colour(env.int_field(style, c"foregroundColor")), background: colour(env.int_field(style, c"backgroundColor")), scale }
    })
}

// ---- the captions' thread ----

/// Reads each caption track's cues (the source's text samples), in turn, into what the threads
/// share; for an address, finds the tracks first.
fn caption_thread(media: &'static Media, origin: &Origin, shared: &Shared) {
    let Ok((_, contents)) = Track::read(media, origin) else { return };
    for found in &contents.captions {
        let cues = Track::read(media, origin).map(|(track, _)| track.cues(found)).unwrap_or_default();
        let mut state = shared.lock();
        if state.stop {
            return;
        }
        state.cues.push(cues);
    }
}

impl Track {
    /// The cues of a caption track: each text sample until the next.
    fn cues(&self, found: &Found) -> Vec<Cue> {
        // SAFETY: a live extractor: only this track is read.
        unsafe { (self.media.AMediaExtractor_selectTrack)(self.extractor, found.at) };
        let (mut buffer, mut samples) = (vec![0u8; 64 << 10], Vec::new());
        while let Some((time, size)) = self.next_sample(&mut buffer) {
            samples.push((time, cue_text(&found.mime, &buffer[..size])));
        }
        let ends = samples.iter().skip(1).map(|(time, _)| *time).chain([Duration::MAX]);
        samples.iter().zip(ends).filter(|((_, text), _)| !text.is_empty()).map(|((from, text), to)| Cue { from: *from, to, text: text.clone() }).collect()
    }

    /// The next sample, into `buffer`: its time and size; none after the last.
    fn next_sample(&self, buffer: &mut [u8]) -> Option<(Duration, usize)> {
        let m = self.media;
        // SAFETY: a live extractor, writing within the buffer's room.
        unsafe {
            let size = usize::try_from((m.AMediaExtractor_readSampleData)(self.extractor, buffer.as_mut_ptr(), buffer.len())).ok()?;
            let time = Duration::from_micros((m.AMediaExtractor_getSampleTime)(self.extractor).max(0) as u64);
            (m.AMediaExtractor_advance)(self.extractor);
            Some((time, size))
        }
    }
}

/// A caption sample's text: 3GPP timed text's after its length (styles after it dropped); SRT's
/// and WebVTT's as they are, without their markup.
fn cue_text(mime: &str, sample: &[u8]) -> String {
    let text = match (mime, sample) {
        ("text/3gpp-tt", [high, low, rest @ ..]) => &rest[..(usize::from(*high) << 8 | usize::from(*low)).min(rest.len())],
        ("text/3gpp-tt", _) => &[],
        _ => sample,
    };
    crate::video_captions::plain(String::from_utf8_lossy(text).trim_end_matches('\0').trim())
}

// ---- the video's thread ----

/// Reads the source (where `open` did not), decodes the video and shows each frame at its
/// time: after a seek, the frame at the time asked. Keeps the time where there is no video, and
/// tells the end.
fn video_thread(media: &'static Media, origin: &Origin, read: Option<(Track, Contents)>, shared: &Shared) {
    let tracks_known = read.is_some();
    let played = read.map_or_else(|| Track::read(media, origin), Ok).and_then(|(track, contents)| {
        let mut state = shared.lock();
        state.length = contents.length;
        if !tracks_known {
            state.tracks = Some(contents.tracks());
        }
        drop(state);
        let mut pictures = Pictures::open(track, contents.video)?;
        play_pictures(pictures.as_mut(), shared)
    });
    if let Err(why) = played {
        shared.failed(why);
        shared.rest();
    }
}

/// Plays the video until the player is dropped; where all its frames are shown (or there are
/// none), keeps the time.
fn play_pictures(mut pictures: Option<&mut Pictures>, shared: &Shared) -> Result<(), String> {
    loop {
        let asked = shared.asked();
        if asked.stop {
            return Ok(());
        }
        let shown_all = match &mut pictures {
            Some(pictures) => pictures.step(asked, shared)?,
            None => true,
        };
        if shown_all {
            keep_time(asked, shared);
        }
    }
}

/// All frames shown: the end comes once the sound is done too and the time has reached the
/// length; until then it waits.
fn keep_time(asked: Asked, shared: &Shared) {
    let mut state = shared.lock();
    let at_end = state.length.is_none_or(|length| asked.now >= length);
    if !(asked.playing && state.sound_done && at_end) {
        drop(state);
        return shared.wait(Duration::from_millis(10));
    }
    let end = state.length.unwrap_or(asked.now);
    (state.playing, state.ended) = (false, true);
    state.clock = Clock { at: end, since: Instant::now(), running: false, ..state.clock };
}

/// The video's track, the image reader it decodes into, and where it is.
struct Pictures {
    track: Track,
    reader: Reader,
    /// The seeks followed so far; the time a seek asked, until its frame is shown.
    seeks: u32,
    target: Option<Duration>,
    /// Decoded frames waiting: the last one at or before the target, the next to show.
    before: Option<(usize, Duration)>,
    next: Option<(usize, Duration)>,
    /// The decoder gave its last frame.
    drained: bool,
}

impl Pictures {
    /// Decodes the video track (number, type, size) into a reader of its size; none where there
    /// is no video.
    fn open(mut track: Track, video: Option<(usize, String, [i32; 2])>) -> Result<Option<Pictures>, String> {
        let Some((at, mime, size)) = video else { return Ok(None) };
        let reader = Reader::new(track.media, size)?;
        track.decode(at, &mime, reader.window)?;
        Ok(Some(Pictures { track, reader, seeks: 0, target: Some(Duration::ZERO), before: None, next: None, drained: false }))
    }

    /// One step: follows a seek, decodes, shows a frame that is due. Whether all is shown.
    fn step(&mut self, asked: Asked, shared: &Shared) -> Result<bool, String> {
        if asked.seeks != self.seeks {
            self.seek(asked.seeks, asked.seek_to);
        }
        if self.next.is_none() && !self.drained {
            self.decode()?;
        }
        if let Some(target) = self.target {
            self.land(target, shared);
            return Ok(false);
        }
        let Some(next @ (_, at)) = self.next else { return Ok(self.drained) };
        if asked.playing && asked.now >= at {
            self.next = None;
            self.show(next, shared);
            return Ok(false);
        }
        let rest = if asked.playing { (at - asked.now).min(Duration::from_millis(20)) } else { Duration::from_millis(50) };
        shared.wait(rest);
        Ok(false)
    }

    /// Goes to the key frame before `to`; frames already decoded are dropped (a flushed decoder
    /// takes none back).
    fn seek(&mut self, seeks: u32, to: Duration) {
        (self.seeks, self.target, self.before, self.next, self.drained) = (seeks, Some(to), None, None, false);
        self.track.seek(to);
    }

    /// Feeds the decoder and takes its next frame, if it has one: as `next`.
    fn decode(&mut self) -> Result<(), String> {
        self.track.feed();
        let Some((at, info)) = self.track.output(Duration::from_millis(10))? else { return Ok(()) };
        if info.flags & END_OF_STREAM != 0 {
            self.track.release(at, false);
            self.drained = true;
            return Ok(());
        }
        self.next = Some((at, Duration::from_micros(info.time_us.max(0) as u64)));
        Ok(())
    }

    /// While seeking: keeps the last frame at or before the time asked, and shows it once the
    /// next is past that time or there is no next (else the first after it).
    fn land(&mut self, target: Duration, shared: &Shared) {
        match self.next {
            Some(next) if next.1 <= target => self.keep_before(next),
            None if !self.drained => {}
            _ => self.show_landed(shared),
        }
    }

    /// The seek is over: shows the frame kept, else the first after the time asked.
    fn show_landed(&mut self, shared: &Shared) {
        self.target = None;
        if let Some(landed) = self.before.take().or_else(|| self.next.take()) {
            self.show(landed, shared);
        }
    }

    /// Keeps a frame at or before a seek's time in place of the one kept before.
    fn keep_before(&mut self, frame: (usize, Duration)) {
        if let Some((dropped, _)) = self.before.replace(frame) {
            self.track.release(dropped, false);
        }
        self.next = None;
    }

    /// Shows a decoded frame: into the reader, then made RGB for the app's side, unless a seek
    /// came meanwhile.
    fn show(&mut self, (at, time): (usize, Duration), shared: &Shared) {
        self.track.release(at, true);
        let colours = Colours::of(&self.track);
        let Some((width, pixels)) = self.reader.take(time, colours) else { return };
        let mut state = shared.lock();
        if state.seeks == self.seeks {
            state.frame = Some(Frame { width, pixels, at: time });
        }
    }
}

/// An image reader a decoder draws into.
struct Reader {
    media: &'static Media,
    reader: Object,
    window: Object,
}

impl Reader {
    fn new(media: &'static Media, [width, height]: [i32; 2]) -> Result<Reader, String> {
        let (mut reader, mut window) = (null_mut(), null_mut());
        // SAFETY: a new reader, deleted with this; its window is its own.
        unsafe {
            if (media.AImageReader_new)(width.max(1), height.max(1), YUV_420_888, 4, &mut reader) != 0 {
                return Err(format!("no image reader of {width}×{height}"));
            }
            (media.AImageReader_getWindow)(reader, &mut window);
        }
        Ok(Reader { media, reader, window })
    }

    /// The image drawn for the frame at `time`, made RGB: its width and pixels. Images of
    /// other times are dropped; it waits up to half a second.
    fn take(&self, time: Duration, colours: Colours) -> Option<(usize, Vec<u32>)> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            let mut image = null_mut();
            // SAFETY: a live reader; an image it gives is deleted once read.
            if unsafe { (self.media.AImageReader_acquireNextImage)(self.reader, &mut image) } != 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            let mut stamp = 0;
            // SAFETY: as above.
            unsafe { (self.media.AImage_getTimestamp)(image, &mut stamp) };
            let pixels = (stamp == time.as_nanos() as i64).then(|| rgb_of(self.media, image, colours)).flatten();
            // SAFETY: as above.
            unsafe { (self.media.AImage_delete)(image) };
            if pixels.is_some() {
                return pixels;
            }
        }
        None
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: the reader is this one's; its decoder is gone (the track drops first).
        unsafe { (self.media.AImageReader_delete)(self.reader) };
    }
}

/// One plane of an image: its bytes, and the steps between rows and between samples.
struct Plane<'a> {
    bytes: &'a [u8],
    row: usize,
    step: usize,
}

impl Plane<'_> {
    fn at(&self, x: usize, y: usize) -> u8 {
        self.bytes.get(y * self.row + x * self.step).copied().unwrap_or(0)
    }
}

/// A plane of a live image, borrowed for as long as `image` is (which the caller deletes only
/// after).
fn plane<'a>(media: &'static Media, image: &'a Object, number: i32) -> Option<Plane<'a>> {
    let (mut row, mut step, mut data, mut length) = (0, 0, null_mut(), 0);
    // SAFETY: a live image; its plane's bytes are read only while it is borrowed, before it is
    // deleted.
    unsafe {
        let image = *image;
        let read = (media.AImage_getPlaneRowStride)(image, number, &mut row) | (media.AImage_getPlanePixelStride)(image, number, &mut step);
        if read != 0 || (media.AImage_getPlaneData)(image, number, &mut data, &mut length) != 0 || data.is_null() {
            return None;
        }
        Some(Plane { bytes: std::slice::from_raw_parts(data, length.max(0) as usize), row: row.max(0) as usize, step: step.max(1) as usize })
    }
}

/// A YUV 4:2:0 image's visible part, made RGB: its width and pixels.
fn rgb_of(media: &'static Media, image: Object, colours: Colours) -> Option<(usize, Vec<u32>)> {
    let (mut width, mut height, mut crop) = (0, 0, [0; 4]);
    // SAFETY: a live image.
    unsafe {
        (media.AImage_getWidth)(image, &mut width);
        (media.AImage_getHeight)(image, &mut height);
        (media.AImage_getCropRect)(image, &mut crop);
    }
    let [left, top, right, bottom] = match crop {
        [left, top, right, bottom] if right > left && bottom > top => [left, top, right.min(width), bottom.min(height)],
        _ => [0, 0, width, height],
    }
    .map(|edge| edge.max(0) as usize);
    let [Some(y), Some(u), Some(v)] = [0, 1, 2].map(|number| plane(media, &image, number)) else { return None };
    let rows = (top..bottom).flat_map(|row| (left..right).map(move |column| (column, row)));
    let pixels = rows.map(|(x, row)| colours.rgb(y.at(x, row), u.at(x / 2, row / 2), v.at(x / 2, row / 2))).collect();
    Some((right - left, pixels))
}

/// How YUV becomes RGB: by BT.601, BT.709 or BT.2020, in limited or full range, in 14-bit
/// fixed point.
#[derive(Clone, Copy)]
struct Colours {
    black: i32,
    luma: i32,
    red_v: i32,
    green_u: i32,
    green_v: i32,
    blue_u: i32,
}

impl Colours {
    /// As the decoder's output format says (`color-standard`, `color-range`); else BT.601 below
    /// 720 lines and BT.709 from there, limited range.
    fn of(track: &Track) -> Colours {
        let height = track.output_number(c"height").unwrap_or(0);
        let (kr, kb) = match track.output_number(c"color-standard") {
            Some(1) => (0.2126, 0.0722),
            Some(6) => (0.2627, 0.0593),
            Some(2 | 4) => (0.299, 0.114),
            _ if height >= 720 => (0.2126, 0.0722),
            _ => (0.299, 0.114),
        };
        let full = track.output_number(c"color-range") == Some(1);
        let (luma, chroma) = if full { (1.0, 1.0) } else { (255.0 / 219.0, 255.0 / 224.0) };
        let (fixed, kg) = (|value: f32| (value * 16384.0).round() as i32, 1.0 - kr - kb);
        Colours {
            black: if full { 0 } else { 16 },
            luma: fixed(luma),
            red_v: fixed(2.0 * (1.0 - kr) * chroma),
            green_u: fixed(2.0 * (1.0 - kb) * kb / kg * chroma),
            green_v: fixed(2.0 * (1.0 - kr) * kr / kg * chroma),
            blue_u: fixed(2.0 * (1.0 - kb) * chroma),
        }
    }

    fn rgb(&self, y: u8, u: u8, v: u8) -> u32 {
        let y = (i32::from(y) - self.black) * self.luma;
        let (u, v) = (i32::from(u) - 128, i32::from(v) - 128);
        let channel = |value: i32| ((value + 8192) >> 14).clamp(0, 255) as u32;
        0xFF00_0000 | channel(y + self.red_v * v) << 16 | channel(y - self.green_u * u - self.green_v * v) << 8 | channel(y + self.blue_u * u)
    }
}

// ---- the sound's thread ----

/// Decodes the chosen sound track and plays it through AAudio while the video plays, from where
/// it is, stretched to its speed; moves the clock to the sound's own as it is heard. Where there
/// is no AAudio, no stream or no decoder, the video plays on without sound.
fn sound_thread(media: &'static Media, origin: &Origin, shared: &Shared) {
    let opened = Audio::get().ok_or_else(String::new).and_then(|audio| Speaker::open(audio, media, origin, shared.asked().sound_track));
    let played = opened.and_then(|mut speaker| speaker.play(shared));
    if played.is_err() {
        shared.lock().sound_done = true;
        shared.rest();
    }
}

/// The sound's track, its stream once its format is known, and what waits to be written.
struct Speaker {
    audio: &'static Audio,
    media: &'static Media,
    origin: Origin,
    /// The sound track it decodes, by its place among the source's.
    sound_track: usize,
    track: Track,
    stream: Option<Stream>,
    seeks: u32,
    /// Samples before this time are not played (decoded on the way to a seek's time).
    skip_to: Duration,
    /// Samples decoded, stretched and not yet written.
    waiting: Vec<i16>,
    /// The time of the first sample written since the stream started or was emptied, and the
    /// stream's count of frames written then.
    heard_from: Option<(Duration, i64)>,
    /// The speed it plays at, and its stretch (made with the stream).
    speed: f64,
    stretch: Option<Stretch>,
}

/// An AAudio stream: its rate and channels, and whether it plays.
struct Stream {
    audio: &'static Audio,
    stream: Object,
    rate: i32,
    channels: usize,
    started: bool,
}

impl Speaker {
    /// The source's `nth` sound track (its first where there is no such), decoding.
    fn open(audio: &'static Audio, media: &'static Media, origin: &Origin, nth: usize) -> Result<Speaker, String> {
        let (mut track, contents) = Track::read(media, origin)?;
        let found = contents.sound.get(nth).or(contents.sound.first()).ok_or_else(String::new)?;
        track.decode(found.at, &found.mime, null_mut())?;
        Ok(Speaker { audio, media, origin: origin.clone(), sound_track: nth, track, stream: None, seeks: 0, skip_to: Duration::ZERO, waiting: Vec::new(), heard_from: None, speed: 1.0, stretch: None })
    }

    /// Plays until the player is dropped; an error where no stream opens.
    fn play(&mut self, shared: &Shared) -> Result<(), String> {
        loop {
            let asked = shared.asked();
            if asked.stop {
                return Ok(());
            }
            self.step(asked, shared)?;
        }
    }

    fn step(&mut self, asked: Asked, shared: &Shared) -> Result<(), String> {
        if asked.sound_track != self.sound_track {
            // (Another track: decoded from where the media is.)
            *self = Speaker::open(self.audio, self.media, &self.origin, asked.sound_track)?;
            self.seek(asked.seeks, asked.now);
        }
        if asked.seeks != self.seeks {
            self.seek(asked.seeks, asked.seek_to);
        }
        if asked.speed != self.speed {
            (self.speed, self.heard_from) = (asked.speed, None);
            self.stretch = self.stream.as_ref().map(|stream| Stretch::new(stream.channels, stream.rate));
        }
        if !asked.playing {
            if let Some(stream) = &mut self.stream {
                stream.set_playing(false);
            }
            shared.wait(Duration::from_millis(50));
            return Ok(());
        }
        let Some(stream) = self.stream.as_mut().filter(|_| !self.waiting.is_empty()) else { return self.decode(asked.volume, shared) };
        stream.set_playing(true);
        let written = stream.write(&self.waiting);
        self.waiting.drain(..written * stream.channels);
        if let Some(heard) = self.heard_from.and_then(|from| stream.heard(from, self.speed)) {
            follow(heard, shared);
        }
        Ok(())
    }

    fn seek(&mut self, seeks: u32, to: Duration) {
        (self.seeks, self.skip_to, self.heard_from) = (seeks, to, None);
        self.waiting.clear();
        self.track.seek(to);
        if let Some(stream) = &mut self.stream {
            stream.empty();
            self.stretch = Some(Stretch::new(stream.channels, stream.rate));
        }
    }

    /// Decodes the next samples into `waiting`, at the volume asked, stretched to the speed;
    /// opens the stream for the first.
    fn decode(&mut self, volume: f32, shared: &Shared) -> Result<(), String> {
        self.track.feed();
        let Some((at, info)) = self.track.output(Duration::from_millis(10))? else { return Ok(()) };
        if info.flags & END_OF_STREAM != 0 {
            self.track.release(at, false);
            shared.lock().sound_done = true;
            shared.wait(Duration::from_millis(50));
            return Ok(());
        }
        if self.stream.is_none() {
            let stream = Stream::open(self.audio, &self.track).ok_or("no sound stream")?;
            self.stretch = Some(Stretch::new(stream.channels, stream.rate));
            self.stream = Some(stream);
        }
        let time = Duration::from_micros(info.time_us.max(0) as u64);
        let float = self.track.output_number(c"pcm-encoding") == Some(PCM_FLOAT);
        let samples = samples(self.track.bytes(at, &info), float);
        self.track.release(at, false);
        let (Some(stream), Some(stretch)) = (&self.stream, &mut self.stretch) else { return Ok(()) };
        let skipped = self.skip_to.saturating_sub(time).as_secs_f64() * f64::from(stream.rate);
        let skipped = (skipped as usize * stream.channels).min(samples.len());
        if skipped < samples.len() && self.heard_from.is_none() {
            let first = time + Duration::from_secs_f64(skipped as f64 / stream.channels as f64 / f64::from(stream.rate));
            self.heard_from = Some((first, stream.written()));
        }
        self.waiting = stretch.apply(&samples[skipped..], self.speed);
        #[cfg(test)]
        {
            let first: Vec<f32> = self.waiting.iter().step_by(stream.channels).map(|sample| f32::from(*sample) / 32768.0).collect();
            shared.lock().heard = (first, stream.rate as u32);
        }
        // (The volume last: a muted video is still heard here in tests.)
        self.waiting.iter_mut().for_each(|sample| *sample = (f32::from(*sample) * volume) as i16);
        Ok(())
    }
}

/// 16-bit samples from a decoder's bytes (16-bit, or float).
fn samples(bytes: &[u8], float: bool) -> Vec<i16> {
    if float {
        bytes.as_chunks::<4>().0.iter().map(|sample| (f32::from_le_bytes(*sample) * 32767.0).clamp(-32768.0, 32767.0) as i16).collect()
    } else {
        bytes.as_chunks::<2>().0.iter().map(|sample| i16::from_le_bytes(*sample)).collect()
    }
}

/// Moves the clock to the sound's time where they are more than 40 ms apart.
fn follow((heard, at): (Duration, Instant), shared: &Shared) {
    let mut state = shared.lock();
    let clock = Clock { at: heard, since: at, ..state.clock };
    if state.clock.running && state.clock.now().abs_diff(clock.now()) > Duration::from_millis(40) {
        state.clock.set(heard, at);
    }
}

/// Sound made faster or slower at the same pitch (WSOLA): windows of 30 ms, each laid half over
/// the last, each taken from the input near where it is due, where it best continues the last.
struct Stretch {
    channels: usize,
    /// A window's frames.
    window: usize,
    /// Samples not yet used, and where in them the next window is due, in frames.
    input: Vec<f32>,
    due: f64,
    /// The last window's second half, to add to the next's first.
    tail: Vec<f32>,
    /// What followed the last window's first half in the input (first channel), which the next
    /// window should continue.
    follows: Vec<f32>,
}

impl Stretch {
    fn new(channels: usize, rate: i32) -> Stretch {
        let window = ((rate.max(8000) as usize * 3 / 100) & !1).max(32);
        Stretch { channels: channels.max(1), window, input: Vec::new(), due: 0.0, tail: vec![0.0; window / 2 * channels.max(1)], follows: Vec::new() }
    }

    /// The stretched sound of these samples (what is due of them so far: the rest waits for the
    /// next); as they are at the usual speed.
    fn apply(&mut self, samples: &[i16], speed: f64) -> Vec<i16> {
        if speed == 1.0 && self.input.is_empty() {
            return samples.to_vec();
        }
        self.input.extend(samples.iter().map(|sample| f32::from(*sample)));
        let (half, reach, channels) = (self.window / 2, self.window / 4, self.channels);
        let mut out = Vec::new();
        while self.due as usize + reach + self.window <= self.input.len() / channels {
            let start = self.best(self.due as usize, reach);
            self.lay(start, &mut out);
            self.due += half as f64 * speed;
        }
        let used = (self.due as usize).saturating_sub(reach);
        self.input.drain(..used * channels);
        self.due -= used as f64;
        out.into_iter().map(|sample| sample.round().clamp(-32768.0, 32767.0) as i16).collect()
    }

    /// Where near `due` a window best continues the last: the most alike to what followed it.
    fn best(&self, due: usize, reach: usize) -> usize {
        if self.follows.is_empty() {
            return due;
        }
        let alike = |start: usize| -> f32 { self.follows.iter().step_by(4).enumerate().map(|(at, follows)| self.input[(start + at * 4) * self.channels] * follows).sum() };
        (due.saturating_sub(reach)..=due + reach).step_by(2).max_by(|one, other| alike(*one).total_cmp(&alike(*other))).unwrap_or(due)
    }

    /// Lays the window from `start` (Hann-shaped, so two halves sum to the whole): its first
    /// half added to the last's second, into `out`; its second half kept.
    fn lay(&mut self, start: usize, out: &mut Vec<f32>) {
        let (half, channels) = (self.window / 2, self.channels);
        let shape = |at: usize| 0.5 - 0.5 * (std::f32::consts::TAU * at as f32 / self.window as f32).cos();
        let sample = |at: usize, channel: usize| self.input[(start + at) * channels + channel] * shape(at);
        out.extend((0..half * channels).map(|at| self.tail[at] + sample(at / channels, at % channels)));
        self.tail = (0..half * channels).map(|at| sample(half + at / channels, at % channels)).collect();
        self.follows = (0..half).map(|at| self.input[(start + half + at) * channels]).collect();
    }
}

impl Stream {
    /// A stream for the decoder's output: 16-bit, its rate and channels, for films.
    fn open(audio: &'static Audio, track: &Track) -> Option<Stream> {
        let rate = track.output_number(c"sample-rate")?;
        let channels = track.output_number(c"channel-count")?;
        let (mut builder, mut stream) = (null_mut(), null_mut());
        // SAFETY: a new builder, deleted once it made the stream, which is closed with this.
        unsafe {
            if (audio.AAudio_createStreamBuilder)(&mut builder) != 0 {
                return None;
            }
            (audio.AAudioStreamBuilder_setFormat)(builder, PCM_16);
            (audio.AAudioStreamBuilder_setChannelCount)(builder, channels);
            (audio.AAudioStreamBuilder_setSampleRate)(builder, rate);
            (audio.AAudioStreamBuilder_setUsage)(builder, USAGE_MEDIA);
            (audio.AAudioStreamBuilder_setContentType)(builder, CONTENT_MOVIE);
            let opened = (audio.AAudioStreamBuilder_openStream)(builder, &mut stream);
            (audio.AAudioStreamBuilder_delete)(builder);
            (opened == 0).then_some(Stream { audio, stream, rate, channels: channels.max(1) as usize, started: false })
        }
    }

    fn set_playing(&mut self, playing: bool) {
        if self.started != playing {
            self.started = playing;
            // SAFETY: a live stream.
            unsafe { if playing { (self.audio.AAudioStream_requestStart)(self.stream) } else { (self.audio.AAudioStream_requestPause)(self.stream) } };
        }
    }

    /// Drops what it holds (after a seek); paused.
    fn empty(&mut self) {
        self.set_playing(false);
        // SAFETY: a live, paused stream.
        unsafe { (self.audio.AAudioStream_requestFlush)(self.stream) };
    }

    /// Writes as many frames as it takes within 20 ms: how many.
    fn write(&mut self, samples: &[i16]) -> usize {
        let frames = (samples.len() / self.channels) as i32;
        // SAFETY: a live stream, reading `frames` frames of the samples.
        let written = unsafe { (self.audio.AAudioStream_write)(self.stream, samples.as_ptr().cast(), frames, 20_000_000) };
        written.max(0) as usize
    }

    fn written(&self) -> i64 {
        // SAFETY: a live stream.
        unsafe { (self.audio.AAudioStream_getFramesWritten)(self.stream) }
    }

    /// The media time heard at an instant, played at `speed`: from the frame the stream last
    /// played, and when.
    fn heard(&self, (first, written): (Duration, i64), speed: f64) -> Option<(Duration, Instant)> {
        let (mut frame, mut nanos, mut now) = (0, 0, [0; 2]);
        // SAFETY: a live stream; a clock this system has.
        let known = unsafe { (self.audio.AAudioStream_getTimestamp)(self.stream, MONOTONIC, &mut frame, &mut nanos) == 0 && clock_gettime(MONOTONIC, &mut now) == 0 };
        if !known || frame <= written {
            return None;
        }
        let ago = Duration::new(now[0] as u64, now[1] as u32).saturating_sub(Duration::from_nanos(nanos as u64));
        let heard = first + Duration::from_secs_f64((frame - written) as f64 * speed / f64::from(self.rate));
        Some((heard, Instant::now().checked_sub(ago)?))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream is this one's.
        unsafe { (self.audio.AAudioStream_close)(self.stream) };
    }
}

#[cfg(test)]
impl Player {
    /// The sound last handed to the speaker, its first channel, and its rate; none before any
    /// (or with no sound device).
    pub(crate) fn heard(&mut self) -> Option<(Vec<f32>, u32)> {
        let heard = self.shared.lock().heard.clone();
        (!heard.0.is_empty()).then_some(heard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many times a second a sound at 8 kHz crosses zero, halved: a tone's Hz.
    fn hertz(samples: &[i16]) -> f32 {
        let crossings = samples.windows(2).filter(|two| (two[0] < 0) != (two[1] < 0)).count();
        crossings as f32 * 8000.0 / samples.len() as f32 / 2.0
    }

    #[test]
    fn stretched_sound_keeps_its_pitch_and_takes_the_time_its_speed_says() {
        let tone: Vec<i16> = (0..16_000).map(|at| ((at as f32 * 400.0 / 8000.0 * std::f32::consts::TAU).sin() * 8000.0) as i16).collect();
        for speed in [2.0, 0.5, 1.5] {
            let mut stretch = Stretch::new(1, 8000);
            let out: Vec<i16> = tone.chunks(800).flat_map(|chunk| stretch.apply(chunk, speed)).collect();
            let expected = tone.len() as f64 / speed;
            // (The last window and its reach wait for more.)
            assert!((out.len() as f64 - expected).abs() < 2.0 * stretch.window as f64 / speed, "at {speed}×: {} samples, not about {expected}", out.len());
            let heard = hertz(&out[stretch.window..]);
            assert!((heard - 400.0).abs() < 20.0, "at {speed}× the tone is {heard} Hz");
        }
    }
}
