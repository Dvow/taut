//! Videos on macOS and iOS: AVFoundation's `AVPlayer`, its frames read through an
//! `AVPlayerItemVideoOutput` as 32-bit BGRA. The frameworks are opened at run time; their
//! classes are reached by name. Bytes are played from a file of their own, removed after. It
//! plays while the main thread's run loop turns (AppKit's or UIKit's).

use crate::macos_sys::{Args, Bool, Obj, Pool, class, ns_string, rust_string, sel, send};
use crate::video::{News, VideoError, VideoSource};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
}
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
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

/// CoreVideo's functions for reading a pixel buffer, and its key for their format.
struct Functions {
    lock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    unlock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    base: unsafe extern "C" fn(*mut c_void) -> *const u8,
    row: unsafe extern "C" fn(*mut c_void) -> usize,
    width: unsafe extern "C" fn(*mut c_void) -> usize,
    height: unsafe extern "C" fn(*mut c_void) -> usize,
    release: unsafe extern "C" fn(*mut c_void),
    format_key: Obj,
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
    let _av = open(c"/System/Library/Frameworks/AVFoundation.framework/AVFoundation")?;
    let video = open(c"/System/Library/Frameworks/CoreVideo.framework/CoreVideo")?;
    let found = |name: &CStr| {
        // SAFETY: a name ending in a zero, of an open library.
        let at = unsafe { dlsym(video, name.as_ptr()) };
        (!at.is_null()).then_some(at)
    };
    // SAFETY: each is the function or constant of that name, of the type its header gives.
    unsafe {
        Some(Functions {
            lock: function(found(c"CVPixelBufferLockBaseAddress")?),
            unlock: function(found(c"CVPixelBufferUnlockBaseAddress")?),
            base: function(found(c"CVPixelBufferGetBaseAddress")?),
            row: function(found(c"CVPixelBufferGetBytesPerRow")?),
            width: function(found(c"CVPixelBufferGetWidth")?),
            height: function(found(c"CVPixelBufferGetHeight")?),
            release: function(found(c"CVBufferRelease")?),
            format_key: *found(c"kCVPixelBufferPixelFormatTypeKey")?.cast::<Obj>(),
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

/// Sends a message whose result is a `CMTime`: through memory on x86-64.
///
/// # Safety
/// As `send`.
unsafe fn send_time(to: Obj, name: &CStr) -> CMTime {
    #[cfg(target_arch = "x86_64")]
    let via = objc_msgSend_stret;
    #[cfg(not(target_arch = "x86_64"))]
    let via = objc_msgSend;
    // SAFETY: as the caller promises; nil would answer garbage here, so it is not asked.
    if to.is_null() { CMTime::default() } else { unsafe { ().send(via, to, sel(name)) } }
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
    /// Seeks asked for; frames wait until the player has finished as many (`Seeked::done`):
    /// while it decodes its way to the time asked, it hands out frames on the way.
    seeks: u32,
    /// What the player calls when a seek is over.
    seeked: &'static Seeked,
}

#[link(name = "System")]
unsafe extern "C" {
    /// The class of blocks that live for good (`BLOCK_IS_GLOBAL`).
    static _NSConcreteGlobalBlock: [usize; 0];
}

/// A block's descriptor: its size.
#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

static DESCRIBED: Descriptor = Descriptor { reserved: 0, size: size_of::<Seeked>() };

/// The block a seek calls when it is over, finished or not, laid out as the compiler lays one
/// out, with the count it adds to. It lives for good: the player may call it after the
/// `Player` is gone (some 40 bytes for each video opened).
#[repr(C)]
struct Seeked {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*const Seeked, Bool),
    descriptor: *const Descriptor,
    done: AtomicU32,
}

// SAFETY: its count is atomic; the rest is never changed.
unsafe impl Sync for Seeked {}

impl Seeked {
    fn new() -> &'static Seeked {
        /// `BLOCK_IS_GLOBAL`.
        const LIVES_ON: i32 = 1 << 28;
        let isa = (&raw const _NSConcreteGlobalBlock).cast();
        Box::leak(Box::new(Seeked { isa, flags: LIVES_ON, reserved: 0, invoke: Seeked::over, descriptor: &DESCRIBED, done: AtomicU32::new(0) }))
    }
    /// The seek is over.
    unsafe extern "C" fn over(block: *const Seeked, _finished: Bool) {
        // SAFETY: the player calls the block with itself, which lives for good.
        unsafe { (*block).done.fetch_add(1, Ordering::Release) };
    }
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
            Ok(Player { functions, player, item, output, playing: false, ready: false, last: None, written, seeks: 0, seeked: Seeked::new() })
        }
    }

    pub(crate) fn play(&mut self) {
        self.playing = true;
        // SAFETY: a live player.
        unsafe { send::<()>(self.player, c"play", ()) };
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
        unsafe { send_time(self.player, c"currentTime") }.known().unwrap_or_default()
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        // SAFETY: a live item.
        unsafe { send_time(self.item, c"duration") }.known()
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
        if !self.ready || self.seeked.done.load(Ordering::Acquire) != self.seeks {
            return None;
        }
        let _pool = Pool::new();
        let mut shown = CMTime::default();
        // SAFETY: live objects; the buffer copied is ours and released.
        let buffer: *mut c_void = unsafe {
            let now = send_time(self.player, c"currentTime");
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
