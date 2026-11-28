//! Videos on macOS and iOS: AVFoundation's `AVPlayer`, its frames read through an
//! `AVPlayerItemVideoOutput` as 32-bit BGRA. The frameworks are opened at run time; their
//! classes are reached by name. Bytes are played from a file of their own, removed after. It
//! plays while the main thread's run loop turns (AppKit's or UIKit's).
//!
//! Sound tracks are the item's audio tracks, one enabled at a time. Caption tracks are a file's
//! own, read by taut as it plays (`video_mp4`), or a stream's, as AVFoundation hands them over
//! (`video_apple_legible`); then the captions in a file's video stream (CEA-608 and CEA-708 in
//! H.264's or HEVC's SEI), read by an `AVAssetReader` as it plays (`video_stream_captions`).
//! Speed is the player's rate, the pitch kept by the spectral time-pitch algorithm.

use crate::color::Color;
use crate::macos_sys::{Block, Bool, Obj, Pool, array_items, class, ns_string, rust_string, send, send_large};
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use crate::video_apple_legible::LegibleCaptions;
use crate::video_captions::Caption;
use crate::video_mp4::FileCaptions;
use crate::video_read_ahead::{ReadAhead, SeiReading, Step};
use crate::video_sei::Codec;
use crate::video_stream_captions::ClosedCaptions;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// A time as CoreMedia keeps it: `value / timescale` seconds, valid where `flags & 1`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    /// Its time, if a known, finite one (valid, not indefinite or infinite).
    pub(crate) fn known(self) -> Option<Duration> {
        let finite = self.flags & 1 != 0 && self.flags & 0x1C == 0 && self.timescale > 0;
        finite.then(|| Duration::from_secs_f64((self.value as f64 / self.timescale as f64).max(0.0)))
    }
    fn of(time: Duration) -> CMTime {
        CMTime { value: time.as_micros().min(i64::MAX as u128) as i64, timescale: 1_000_000, flags: 1, epoch: 0 }
    }
}

/// CoreVideo's functions for reading a pixel buffer, and its key for their format; CoreMedia's
/// for reading a sample buffer; AVFoundation's name of the time-pitch algorithm that keeps the
/// pitch at any rate, and of the characteristic of caption tracks. With `gpu-draw`, CoreVideo's
/// keys asking for buffers Metal reads and its cache of Metal textures of them.
pub(crate) struct Functions {
    lock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    unlock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    base: unsafe extern "C" fn(*mut c_void) -> *const u8,
    row: unsafe extern "C" fn(*mut c_void) -> usize,
    width: unsafe extern "C" fn(*mut c_void) -> usize,
    height: unsafe extern "C" fn(*mut c_void) -> usize,
    release: unsafe extern "C" fn(*mut c_void),
    format_key: Obj,
    pub(crate) sample_time: unsafe extern "C" fn(*mut c_void) -> CMTime,
    decode_time: unsafe extern "C" fn(*mut c_void) -> CMTime,
    pub(crate) sample_duration: unsafe extern "C" fn(*mut c_void) -> CMTime,
    pub(crate) sample_format: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    sample_data: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    data_length: unsafe extern "C" fn(*mut c_void) -> usize,
    copy_data: unsafe extern "C" fn(*mut c_void, usize, usize, *mut u8) -> i32,
    pub(crate) media_subtype: unsafe extern "C" fn(*mut c_void) -> u32,
    spectral: Obj,
    pub(crate) legible: Obj,
    #[cfg(feature = "gpu-draw")]
    metal: MetalTextures,
}

/// `kCVPixelBufferMetalCompatibilityKey` and `kCVPixelBufferIOSurfacePropertiesKey`;
/// `CVMetalTextureCacheCreate`, `CVMetalTextureCacheCreateTextureFromImage`,
/// `CVMetalTextureGetTexture` and `CVMetalTextureCacheFlush`.
#[cfg(feature = "gpu-draw")]
struct MetalTextures {
    compatible_key: Obj,
    surface_key: Obj,
    new_cache: unsafe extern "C" fn(*const c_void, *const c_void, Obj, *const c_void, *mut *mut c_void) -> i32,
    new_texture: unsafe extern "C" fn(*const c_void, *mut c_void, *mut c_void, *const c_void, usize, usize, usize, usize, *mut *mut c_void) -> i32,
    texture_of: unsafe extern "C" fn(*mut c_void) -> Obj,
    flush: unsafe extern "C" fn(*mut c_void, u64),
}
// SAFETY: functions, and a constant string object that lives as long as the program.
unsafe impl Send for Functions {}
// SAFETY: as above.
unsafe impl Sync for Functions {}

/// The frameworks, opened once; none where AVFoundation is missing.
pub(crate) fn functions() -> Option<&'static Functions> {
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
            decode_time: function(found(media, c"CMSampleBufferGetDecodeTimeStamp")?),
            sample_duration: function(found(media, c"CMSampleBufferGetDuration")?),
            sample_format: function(found(media, c"CMSampleBufferGetFormatDescription")?),
            sample_data: function(found(media, c"CMSampleBufferGetDataBuffer")?),
            data_length: function(found(media, c"CMBlockBufferGetDataLength")?),
            copy_data: function(found(media, c"CMBlockBufferCopyDataBytes")?),
            media_subtype: function(found(media, c"CMFormatDescriptionGetMediaSubType")?),
            spectral: *found(av, c"AVAudioTimePitchAlgorithmSpectral")?.cast::<Obj>(),
            legible: *found(av, c"AVMediaCharacteristicLegible")?.cast::<Obj>(),
            #[cfg(feature = "gpu-draw")]
            metal: MetalTextures {
                compatible_key: *found(video, c"kCVPixelBufferMetalCompatibilityKey")?.cast::<Obj>(),
                surface_key: *found(video, c"kCVPixelBufferIOSurfacePropertiesKey")?.cast::<Obj>(),
                new_cache: function(found(video, c"CVMetalTextureCacheCreate")?),
                new_texture: function(found(video, c"CVMetalTextureCacheCreateTextureFromImage")?),
                texture_of: function(found(video, c"CVMetalTextureGetTexture")?),
                flush: function(found(video, c"CVMetalTextureCacheFlush")?),
            },
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
    /// The last seek is to where it stood, for the first frame: it hands out no frames on the
    /// way, and the player may say it is over seconds after its frame (8 s on the iOS simulator).
    in_place: bool,
    /// What the player calls when a seek is over.
    seeked: &'static Seeked,
    /// The rate it plays at.
    speed: f32,
    /// The source's own caption tracks, and how many of them were listed (before those of the
    /// video stream); the one chosen.
    own: OwnCaptions,
    own_listed: usize,
    captions: Option<usize>,
    /// The captions in the video stream of a file, read by a thread of their own.
    in_stream: Arc<ReadAhead<SeiReading>>,
}

/// Where a source's own caption tracks come from.
enum OwnCaptions {
    /// A file's, read by taut as it plays.
    File(Arc<ReadAhead<FileCaptions>>),
    /// A stream's, as AVFoundation hands them over.
    Stream(LegibleCaptions),
}

impl OwnCaptions {
    /// A file's caption tracks, read from now on by a thread of their own.
    fn of_file(path: PathBuf) -> OwnCaptions {
        let captions = Arc::new(ReadAhead::new());
        captions.spawn(move |captions| {
            if let Ok(file) = std::fs::File::open(&path) {
                crate::video_mp4::read_paced(file, captions);
            }
        });
        OwnCaptions::File(captions)
    }

    /// The tracks, now listed.
    fn tracks(&mut self) -> Vec<Track> {
        match self {
            OwnCaptions::File(captions) => captions.with(FileCaptions::tracks),
            OwnCaptions::Stream(captions) => captions.tracks(),
        }
    }

    /// Whether tracks came since they were last listed.
    fn unlisted(&self) -> bool {
        match self {
            OwnCaptions::File(captions) => captions.with(|captions| captions.unlisted()),
            OwnCaptions::Stream(captions) => captions.unlisted(),
        }
    }

    /// Whether tracks may still come though the video stands still.
    fn reading(&self) -> bool {
        match self {
            OwnCaptions::File(captions) => captions.reading(),
            OwnCaptions::Stream(captions) => captions.loading(),
        }
    }

    /// Shows the `nth` track from now on, or none.
    fn choose(&mut self, nth: Option<usize>) {
        if let OwnCaptions::Stream(captions) = self {
            captions.choose(nth);
        }
    }

    /// The captions the `nth` track shows at `at`.
    fn captions_at(&self, nth: usize, at: Duration) -> Vec<Caption> {
        match self {
            OwnCaptions::File(captions) => captions.with(|captions| captions.captions_at(nth, at)),
            OwnCaptions::Stream(captions) => captions.captions_at(at),
        }
    }

    /// It plays at `position`.
    fn played_to(&self, position: Duration) {
        if let OwnCaptions::File(captions) = self {
            captions.played_to(position);
        }
    }

    /// It went to `to`.
    fn seek(&self, to: Duration) {
        match self {
            OwnCaptions::File(captions) => captions.seek(to),
            OwnCaptions::Stream(captions) => captions.sought(to),
        }
    }
}

impl Drop for OwnCaptions {
    fn drop(&mut self) {
        if let OwnCaptions::File(captions) = self {
            captions.stop();
        }
    }
}

/// A source's own caption tracks: a file's, read by taut, and the captions in its video stream
/// read into `in_stream`; or a stream's, as the item hands them over.
fn own_captions(functions: &'static Functions, url: Obj, item: Obj, in_stream: &Arc<ReadAhead<SeiReading>>) -> OwnCaptions {
    let Some(path) = file_of(url) else { return OwnCaptions::Stream(LegibleCaptions::new(functions, item)) };
    let url = Retained::of(url);
    in_stream.spawn(move |in_stream| read_captions_in_stream(functions, &url, in_stream));
    OwnCaptions::of_file(path)
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
        // autoreleased into the pool.
        unsafe {
            let item: Obj = send(class(c"AVPlayerItem"), c"playerItemWithURL:", (url,));
            if item.is_null() {
                return Err(VideoError::Refused("AVFoundation made no item of it".into()));
            }
            let output: Obj = send(send::<Obj>(class(c"AVPlayerItemVideoOutput"), c"alloc", ()), c"initWithPixelBufferAttributes:", (buffers_wanted(functions),));
            send::<()>(item, c"addOutput:", (output,));
            let player: Obj = send(send::<Obj>(class(c"AVPlayer"), c"alloc", ()), c"initWithPlayerItem:", (item,));
            send::<Obj>(item, c"retain", ());
            send::<()>(player, c"setMuted:", (!sound as Bool,));
            #[cfg(target_os = "ios")]
            if sound {
                plays_sound();
            }
            send::<()>(item, c"setAudioTimePitchAlgorithm:", (functions.spectral,));
            let in_stream = Arc::new(ReadAhead::new());
            let own = own_captions(functions, url, item, &in_stream);
            Ok(Player {
                functions,
                player,
                item,
                output,
                playing: false,
                ready: false,
                last: None,
                written,
                seeks: 0,
                in_place: false,
                seeked: seeked(),
                speed: 1.0,
                own,
                own_listed: 0,
                captions: None,
                in_stream,
            })
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
        (self.last, self.seeks, self.in_place) = (None, self.seeks.wrapping_add(1), false);
        let exact = CMTime { value: 0, timescale: 1, flags: 1, epoch: 0 };
        let over: *const Seeked = self.seeked;
        let _pool = Pool::new();
        // SAFETY: a live player; the times are passed as the method takes them, and the block
        // lives for good.
        unsafe { send::<()>(self.player, c"seekToTime:toleranceBefore:toleranceAfter:completionHandler:", (CMTime::of(to), exact, exact, over)) };
        self.in_stream.seek(to);
        self.own.seek(to);
    }
    /// Where it plays (the captions' readers read on from there).
    pub(crate) fn position(&mut self) -> Duration {
        // SAFETY: a live player.
        let position = unsafe { send_large::<CMTime>(self.player, c"currentTime", ()) }.known().unwrap_or_default();
        self.in_stream.played_to(position);
        self.own.played_to(position);
        position
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
            self.in_place = true;
        }
        let at_end = self.length().is_some_and(|length| self.position() + Duration::from_millis(1) >= length);
        let ended = self.playing && rate == 0.0 && at_end;
        self.playing &= !ended;
        let tracks_changed = self.ready && (self.own.unlisted() || self.in_stream.with_tracks(|tracks| tracks.unlisted()));
        News { ended, error, clears_on_play: false, tracks_changed, reading_captions: self.own.reading() || self.in_stream.reading() }
    }

    /// The newest frame where there is one not given yet: its width and time, its pixels in
    /// `into`. None before the item is ready (its length known), nor while a seek goes elsewhere.
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let (buffer, shown) = self.new_buffer()?;
        let width = read(self.functions, buffer, into);
        // SAFETY: copied, so ours.
        unsafe { (self.functions.release)(buffer) };
        if width == 0 {
            return None;
        }
        self.last = Some(shown.value);
        Some((width, shown.known().unwrap_or_default()))
    }

    /// Whether frames are given as textures (`frame_texture`): always, with `gpu-draw`.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn decodes_on_gpu(&self) -> bool {
        true
    }
    /// The newest frame where there is one not given yet, left where AVFoundation decoded it:
    /// its pixel buffer and time.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn frame_texture(&mut self) -> Option<(std::sync::Arc<FrameTexture>, Duration)> {
        let (buffer, shown) = self.new_buffer()?;
        let f = self.functions;
        // SAFETY: a live pixel buffer, copied, so ours: the frame holds it.
        let size = unsafe { ((f.width)(buffer) as u32, (f.height)(buffer) as u32) };
        self.last = Some(shown.value);
        Some((std::sync::Arc::new(FrameTexture { functions: f, buffer, size }), shown.known().unwrap_or_default()))
    }

    /// The newest pixel buffer not given yet (+1), and its time; none before the item is ready
    /// (its length known), nor while a seek goes elsewhere.
    fn new_buffer(&mut self) -> Option<(*mut c_void, CMTime)> {
        if !self.ready || (!self.in_place && self.seeked.held.load(Ordering::Acquire) != self.seeks) {
            return None;
        }
        let _pool = Pool::new();
        let mut shown = CMTime::default();
        // SAFETY: live objects; the buffer copied is ours.
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
        if self.last == Some(shown.value) {
            // SAFETY: copied, so ours.
            unsafe { (self.functions.release)(buffer) };
            return None;
        }
        Some((buffer, shown))
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

    /// The item's sound tracks, once it is ready; the source's own caption tracks found so far
    /// (a file's closed-caption track gives a track for each CEA-608 service it holds), then
    /// those of its video stream.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        if !self.ready {
            return None;
        }
        let _pool = Pool::new();
        let sound = self.item_tracks(&["soun"]);
        // SAFETY: live tracks.
        let chosen = sound.iter().position(|track| unsafe { send::<Bool>(*track, c"isEnabled", ()) } != 0);
        let mut captions = self.own.tracks();
        self.own_listed = captions.len();
        captions.extend(self.in_stream.with_tracks(ClosedCaptions::tracks));
        Some(Tracks { sound: sound.iter().map(|track| track_of(*track)).collect(), captions, sound_chosen: chosen })
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
        self.own.choose(nth.filter(|nth| *nth < self.own_listed));
    }
    /// The chosen caption track's captions on show: of the source's own tracks, or of the video
    /// stream's.
    pub(crate) fn captions(&mut self) -> Vec<Caption> {
        let (at, Some(nth)) = (self.position(), self.captions) else { return Vec::new() };
        match nth.checked_sub(self.own_listed) {
            None => self.own.captions_at(nth, at),
            Some(in_stream) => self.in_stream.with_tracks(|tracks| tracks.captions_at(in_stream, at)),
        }
    }
}

/// An `AVAssetReader` reading a track's samples as stored (no settings) from `from`, and its
/// output (both autoreleased); none where they can't be read.
fn stored_samples(asset: Obj, track: Obj, from: Duration) -> Option<(Obj, Obj)> {
    // SAFETY: live objects; what is made is autoreleased into the caller's pool. The time
    // range runs from `from` for good (an infinite duration: flags valid and positive infinity).
    unsafe {
        let reader: Obj = send(class(c"AVAssetReader"), c"assetReaderWithAsset:error:", (asset, std::ptr::null_mut::<Obj>()));
        let output: Obj = send(class(c"AVAssetReaderTrackOutput"), c"assetReaderTrackOutputWithTrack:outputSettings:", (track, std::ptr::null_mut::<c_void>()));
        if reader.is_null() || output.is_null() {
            return None;
        }
        if !from.is_zero() {
            let forever = CMTime { value: 0, timescale: 0, flags: 5, epoch: 0 };
            send::<()>(reader, c"setTimeRange:", (CMTimeRange { start: CMTime::of(from), duration: forever },));
        }
        send::<()>(reader, c"addOutput:", (output,));
        (send::<Bool>(reader, c"startReading", ()) != 0).then_some((reader, output))
    }
}

/// A sample buffer's bytes; none where it has none.
pub(crate) fn sample_bytes(f: &Functions, sample: *mut c_void) -> Option<Vec<u8>> {
    // SAFETY: a live sample buffer; its data is copied within its length.
    unsafe {
        let data = (f.sample_data)(sample);
        if data.is_null() {
            return None;
        }
        let mut bytes = vec![0; (f.data_length)(data)];
        ((f.copy_data)(data, 0, bytes.len(), bytes.as_mut_ptr()) == 0).then_some(bytes)
    }
}

/// A time range as CoreMedia keeps it.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTimeRange {
    start: CMTime,
    duration: CMTime,
}

// ---- the captions in the video stream ----

/// An object retained, moved to the captions' thread, released there.
struct Retained(Obj);

impl Retained {
    fn of(object: Obj) -> Retained {
        // SAFETY: a live object.
        unsafe { send::<Obj>(object, c"retain", ()) };
        Retained(object)
    }
}

// SAFETY: it holds an `NSURL`, which is immutable and may be used from any thread.
unsafe impl Send for Retained {}

impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: retained once, by `of`.
        unsafe { send::<()>(self.0, c"release", ()) };
    }
}

/// The captions' thread: reads the samples of the file's video track as stored (an
/// `AVAssetReader`, no decoder), where it is H.264 or HEVC, a little ahead of where it plays,
/// until the player goes.
fn read_captions_in_stream(functions: &Functions, url: &Retained, in_stream: &ReadAhead<SeiReading>) {
    let _pool = Pool::new();
    // SAFETY: a live URL; the asset and its tracks are autoreleased into the pool.
    let asset: Obj = unsafe { send(class(c"AVURLAsset"), c"URLAssetWithURL:options:", (url.0, std::ptr::null_mut::<c_void>())) };
    // SAFETY: as above.
    let Some(&track) = array_items(unsafe { send(asset, c"tracksWithMediaType:", (ns_string("vide"),)) }).first() else { return };
    let Some(codec) = codec_of(functions, track) else { return };
    let (mut reading, mut read_to) = (TrackReading::start(asset, track, Duration::ZERO), Duration::ZERO);
    loop {
        match in_stream.next(read_to) {
            Step::Stop => return,
            Step::Seek(to) => (reading, read_to) = (TrackReading::start(asset, track, to), to),
            Step::Read => read_to = read_next(reading.as_ref(), functions, codec, in_stream),
        }
    }
}

/// The codec of a video track, where its SEI is read: `avc1` or `avc3`, `hvc1` or `hev1`.
fn codec_of(f: &Functions, track: Obj) -> Option<Codec> {
    // SAFETY: a live track; its format descriptions are autoreleased into the caller's pool.
    let format = *array_items(unsafe { send(track, c"formatDescriptions", ()) }).first()?;
    // SAFETY: a live format description.
    match unsafe { (f.media_subtype)(format) }.to_be_bytes() {
        [b'a', b'v', b'c', b'1' | b'3'] => Some(Codec::H264),
        [b'h', b'v', b'c', b'1'] | [b'h', b'e', b'v', b'1'] => Some(Codec::Hevc),
        _ => None,
    }
}

/// An `AVAssetReader` (retained) reading a track's samples as stored, and its output.
struct TrackReading {
    reader: Obj,
    output: Obj,
}

impl TrackReading {
    /// Starts reading at `from`; none where it can't.
    fn start(asset: Obj, track: Obj, from: Duration) -> Option<TrackReading> {
        let _pool = Pool::new();
        let (reader, output) = stored_samples(asset, track, from)?;
        // SAFETY: a live reader, which keeps its output.
        unsafe { send::<Obj>(reader, c"retain", ()) };
        Some(TrackReading { reader, output })
    }

    /// Reads the next sample into the captions: its time; at the end, `Duration::MAX`, the
    /// captions waiting read.
    fn read_sample(&self, f: &Functions, codec: Codec, in_stream: &ReadAhead<SeiReading>) -> Duration {
        let _pool = Pool::new();
        // SAFETY: a reading output; the sample buffer it copies is ours, read and released.
        let sample: *mut c_void = unsafe { send(self.output, c"copyNextSampleBuffer", ()) };
        if sample.is_null() {
            in_stream.ended();
            return Duration::MAX;
        }
        // SAFETY: as above. (A sample decoded as it is shown has no decode time of its own.)
        let (at, decoded) = unsafe { ((f.sample_time)(sample).known().unwrap_or_default(), (f.decode_time)(sample).known()) };
        if let Some(bytes) = sample_bytes(f, sample) {
            in_stream.read(&bytes, codec, at, Some(decoded.unwrap_or(at)));
        }
        // SAFETY: as above.
        unsafe { crate::macos_sys::CFRelease(sample) };
        at
    }
}

impl Drop for TrackReading {
    fn drop(&mut self) {
        // SAFETY: retained once, by `start`.
        unsafe {
            send::<()>(self.reader, c"cancelReading", ());
            send::<()>(self.reader, c"release", ());
        }
    }
}

/// A BGRA pixel buffer's pixels, opaque, into `into`; its width (0 where it can't be read).
fn read(f: &Functions, buffer: *mut c_void, into: &mut Vec<u32>) -> usize {
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

/// The pixel buffers asked of the video output (autoreleased): 32-bit BGRA ('BGRA'); with
/// `gpu-draw` on an IOSurface, so Metal reads them where they are.
fn buffers_wanted(f: &Functions) -> Obj {
    // SAFETY: the classes answer these messages with autoreleased objects; the keys are
    // CoreVideo's constants.
    unsafe {
        let format: Obj = send(class(c"NSNumber"), c"numberWithUnsignedInt:", (0x4247_5241u32,));
        #[cfg(not(feature = "gpu-draw"))]
        let (values, keys) = ([format], [f.format_key]);
        #[cfg(feature = "gpu-draw")]
        let (values, keys) = {
            let (yes, none): (Obj, Obj) = (send(class(c"NSNumber"), c"numberWithBool:", (1 as Bool,)), send(class(c"NSDictionary"), c"dictionary", ()));
            ([format, yes, none], [f.format_key, f.metal.compatible_key, f.metal.surface_key])
        };
        send(class(c"NSDictionary"), c"dictionaryWithObjects:forKeys:count:", (values.as_ptr(), keys.as_ptr(), keys.len()))
    }
}

/// A frame as AVFoundation decoded it, with `gpu-draw`: its BGRA pixel buffer on an IOSurface
/// (held until dropped), which Metal draws from (`metal_texture`), and its size.
#[cfg(feature = "gpu-draw")]
pub(crate) struct FrameTexture {
    functions: &'static Functions,
    buffer: *mut c_void,
    pub(crate) size: (u32, u32),
}

// SAFETY: a pixel buffer may be retained, released, locked and read from any thread.
#[cfg(feature = "gpu-draw")]
unsafe impl Send for FrameTexture {}
// SAFETY: as above.
#[cfg(feature = "gpu-draw")]
unsafe impl Sync for FrameTexture {}

#[cfg(feature = "gpu-draw")]
impl FrameTexture {
    /// Reads its pixels into `into`, opaque: whether it could.
    pub(crate) fn read(&self, into: &mut Vec<u32>) -> bool {
        #[cfg(test)]
        crate::video::READ_BACK.fetch_add(1, Ordering::Relaxed);
        read(self.functions, self.buffer, into) != 0
    }

    /// A Metal texture of its pixels, BGRA, through `cache` (see `metal_texture_cache`): the
    /// CoreVideo texture holding them (+1; release it once the GPU is done) and the Metal
    /// texture it gives. None where it can't be made.
    pub(crate) fn metal_texture(&self, cache: *mut c_void) -> Option<(*mut c_void, Obj)> {
        let (m, mut made) = (&self.functions.metal, std::ptr::null_mut());
        let [width, height] = [self.size.0 as usize, self.size.1 as usize];
        // SAFETY: a live cache and pixel buffer; plane 0 of a BGRA buffer, as big as it.
        unsafe {
            let done = (m.new_texture)(std::ptr::null(), cache, self.buffer, std::ptr::null(), crate::gpu_metal::BGRA8, width, height, 0, &mut made);
            if done != 0 || made.is_null() {
                return None;
            }
            let texture = (m.texture_of)(made);
            if texture.is_null() {
                crate::macos_sys::CFRelease(made);
                return None;
            }
            Some((made, texture))
        }
    }
}

#[cfg(feature = "gpu-draw")]
impl Drop for FrameTexture {
    fn drop(&mut self) {
        // SAFETY: copied for this frame, so its own.
        unsafe { (self.functions.release)(self.buffer) };
    }
}

/// CoreVideo's cache of Metal textures of pixel buffers, on `device` (+1, `CFRelease` it);
/// none where AVFoundation is missing or it can't be made.
#[cfg(feature = "gpu-draw")]
pub(crate) fn metal_texture_cache(device: Obj) -> Option<*mut c_void> {
    let (m, mut cache) = (&functions()?.metal, std::ptr::null_mut());
    // SAFETY: a live device; no attributes.
    let made = unsafe { (m.new_cache)(std::ptr::null(), std::ptr::null(), device, std::ptr::null(), &mut cache) };
    (made == 0 && !cache.is_null()).then_some(cache)
}

/// Lets `cache` free the textures no longer used.
#[cfg(feature = "gpu-draw")]
pub(crate) fn flush_metal_textures(cache: *mut c_void) {
    if let Some(f) = functions() {
        // SAFETY: a live cache; no options.
        unsafe { (f.metal.flush)(cache, 0) };
    }
}

/// Reads the next sample, where there is a reading: as `TrackReading::read_sample`.
fn read_next(reading: Option<&TrackReading>, f: &Functions, codec: Codec, in_stream: &ReadAhead<SeiReading>) -> Duration {
    let Some(reading) = reading else {
        in_stream.ended();
        return Duration::MAX;
    };
    reading.read_sample(f, codec, in_stream)
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

/// The file a URL names, where it is a file's.
fn file_of(url: Obj) -> Option<PathBuf> {
    // SAFETY: a live URL; its path is copied.
    let path = unsafe { (send::<Bool>(url, c"isFileURL", ()) != 0).then(|| rust_string(send(url, c"path", ()))) };
    path.flatten().map(PathBuf::from)
}

impl Drop for Player {
    fn drop(&mut self) {
        self.in_stream.stop();
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

#[cfg(all(test, feature = "gpu-draw"))]
pub(crate) mod tests {
    use super::*;

    /// A frame of `pixels` (`width` a row, `0xAARRGGBB`) in a BGRA pixel buffer on an IOSurface,
    /// as the video output gives them; none where CoreVideo is missing.
    pub(crate) fn frame_of(width: usize, pixels: &[u32]) -> Option<std::sync::Arc<FrameTexture>> {
        let (f, height, _pool) = (functions()?, pixels.len() / width, Pool::new());
        let mut buffer = std::ptr::null_mut();
        unsafe {
            let video = dlopen(c"/System/Library/Frameworks/CoreVideo.framework/CoreVideo".as_ptr(), 1);
            let create: unsafe extern "C" fn(*const c_void, usize, usize, u32, Obj, *mut *mut c_void) -> i32 = function(dlsym(video, c"CVPixelBufferCreate".as_ptr()));
            assert_eq!(create(std::ptr::null(), width, height, 0x4247_5241, buffers_wanted(f), &mut buffer), 0, "a pixel buffer");
            assert_eq!((f.lock)(buffer, 0), 0);
            let (base, row) = ((f.base)(buffer).cast_mut(), (f.row)(buffer));
            for (y, line) in pixels.chunks(width).enumerate() {
                let to = std::slice::from_raw_parts_mut(base.add(y * row), width * 4);
                to.copy_from_slice(&line.iter().flat_map(|pixel| pixel.to_le_bytes()).collect::<Vec<u8>>());
            }
            (f.unlock)(buffer, 0);
        }
        Some(std::sync::Arc::new(FrameTexture { functions: f, buffer, size: (width as u32, height as u32) }))
    }
}
