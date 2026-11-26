//! Videos on Linux: GStreamer's `playbin`, opened with `dlopen`. Frames come from an `appsink`
//! as 32-bit BGRx, the file's captions from another as text; with no sound, audio goes to a
//! `fakesink`. Sound and caption tracks are `playbin`'s streams, named by their tags. Speed is a
//! seek at that rate, the pitch kept by `scaletempo` where it is installed. A system without
//! GStreamer, or without `playbin`, is told so.

use crate::dynamic_library::library;
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

type Object = *mut c_void;

library!(Gst, ["libgstreamer-1.0.so.0"], {
    fn gst_init_check(*mut c_int, *mut c_void, *mut *mut GError) -> c_int;
    fn gst_element_factory_make(*const c_char, *const c_char) -> Object;
    fn gst_element_set_state(Object, c_int) -> c_int;
    fn gst_element_query_position(Object, c_int, *mut i64) -> c_int;
    fn gst_element_query_duration(Object, c_int, *mut i64) -> c_int;
    fn gst_element_seek(Object, f64, c_int, c_int, c_int, i64, c_int, i64) -> c_int;
    fn gst_element_get_bus(Object) -> Object;
    fn gst_bus_pop_filtered(Object, c_uint) -> Object;
    fn gst_message_parse_error(Object, *mut *mut GError, *mut *mut c_char);
    fn gst_mini_object_unref(Object);
    fn gst_object_unref(Object);
    fn gst_sample_get_buffer(Object) -> Object;
    fn gst_sample_get_caps(Object) -> Object;
    fn gst_caps_get_structure(Object, c_uint) -> Object;
    fn gst_structure_get_int(Object, *const c_char, *mut c_int) -> c_int;
    fn gst_caps_from_string(*const c_char) -> Object;
    fn gst_buffer_map(Object, *mut MapInfo, c_int) -> c_int;
    fn gst_buffer_unmap(Object, *mut MapInfo);
    fn gst_buffer_new_memdup(*const c_void, usize) -> Object;
    fn gst_filename_to_uri(*const c_char, *mut *mut GError) -> *mut c_char;
    fn gst_tag_list_get_string(Object, *const c_char, *mut *mut c_char) -> c_int;
});
library!(App, ["libgstapp-1.0.so.0"], {
    fn gst_app_src_push_buffer(Object, Object) -> c_int;
    fn gst_app_src_end_of_stream(Object) -> c_int;
});
library!(GObject, ["libgobject-2.0.so.0"], {
    fn g_object_set(Object, *const c_char, ...);
    fn g_object_get(Object, *const c_char, ...);
    fn g_signal_connect_data(Object, *const c_char, *const c_void, *mut c_void, *const c_void, c_int) -> u64;
    fn g_signal_emit_by_name(Object, *const c_char, ...);
});
library!(GLib, ["libglib-2.0.so.0"], {
    fn g_free(*mut c_void);
    fn g_error_free(*mut GError);
});

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

/// `GstMapInfo`: its data and size; the rest is GStreamer's.
#[repr(C)]
struct MapInfo {
    memory: Object,
    flags: c_int,
    data: *mut u8,
    size: usize,
    rest: [usize; 9],
}

/// Where in a `GstMessage` its type is, and in a `GstBuffer` its time and duration: after the
/// `GstMiniObject` (64 bytes), and its pool.
const MESSAGE_TYPE: usize = 64;
const BUFFER_TIME: usize = 72;
const BUFFER_DURATION: usize = 88;
/// Messages: the end, an error, a finished state change.
const EOS: c_uint = 1;
const ERROR: c_uint = 2;
const ASYNC_DONE: c_uint = 1 << 21;
const TIME: c_int = 3;
const PAUSED: c_int = 3;
const PLAYING: c_int = 4;
/// A seek that flushes and lands on the exact frame; from a time (`GST_SEEK_TYPE_SET`), to
/// the end (`GST_SEEK_TYPE_NONE`).
const FLUSH_ACCURATE: c_int = 3;
const FROM: c_int = 1;
const ON: c_int = 0;

/// The libraries, opened and GStreamer started once, kept for the program's life (plugins it
/// loaded may still run).
struct Libraries {
    gst: Gst,
    app: App,
    gobject: GObject,
    glib: GLib,
}
// SAFETY: the libraries' functions may be called from any thread.
unsafe impl Send for Libraries {}
// SAFETY: as above; nothing in it changes after it is made.
unsafe impl Sync for Libraries {}

fn libraries() -> Result<&'static Libraries, VideoError> {
    static OPENED: OnceLock<Option<Libraries>> = OnceLock::new();
    let opened = OPENED.get_or_init(|| {
        let libraries = Libraries { gst: Gst::open()?, app: App::open()?, gobject: GObject::open()?, glib: GLib::open()? };
        // SAFETY: no arguments, no error asked for.
        let started = unsafe { (libraries.gst.gst_init_check)(null_mut(), null_mut(), null_mut()) };
        (started != 0).then_some(libraries)
    });
    opened.as_ref().ok_or_else(|| VideoError::NoPlayer("GStreamer is not installed".into()))
}

/// Bytes handed to an `appsrc`, and where it reads.
struct Bytes {
    bytes: Vec<u8>,
    at: AtomicU64,
}

/// A `playbin` playing one source, and its frame and caption sinks.
pub(crate) struct Player {
    lib: &'static Libraries,
    playbin: Object,
    sink: Object,
    text_sink: Object,
    bus: Object,
    playing: bool,
    /// A seek asked before the player was ready, in ns.
    pending: Option<i64>,
    ready: bool,
    /// The last frame given, by its buffer and time.
    last: Option<(usize, u64)>,
    /// Bytes being played, freed after the player.
    bytes: Option<Box<Bytes>>,
    /// The speed it plays at, and whether captions are shown.
    speed: f64,
    captions: bool,
    /// A sound stream chosen while paused, played from the next play. (Switched while paused,
    /// a flushing seek after hangs in `playbin`'s input selector.)
    sound_wanted: Option<usize>,
}

impl Player {
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let lib = libraries()?;
        let make = |kind: &CStr| {
            // SAFETY: a factory name ending in a zero, no element name.
            unsafe { (lib.gst.gst_element_factory_make)(kind.as_ptr(), std::ptr::null()) }
        };
        let (playbin, sink, text_sink) = (make(c"playbin"), make(c"appsink"), make(c"appsink"));
        if playbin.is_null() || sink.is_null() || text_sink.is_null() {
            return Err(VideoError::NoPlayer("GStreamer's playbin or appsink is missing".into()));
        }
        // SAFETY: the bus of a live element, held until the player is dropped.
        let bus = unsafe { (lib.gst.gst_element_get_bus)(playbin) };
        let mut player = Player { lib, playbin, sink, text_sink, bus, playing: false, pending: None, ready: false, last: None, bytes: None, speed: 1.0, captions: false, sound_wanted: None };
        player.set_sinks(sound, make(c"fakesink"), make(c"scaletempo"));
        player.set_source(source)?;
        // SAFETY: a live element.
        unsafe { (lib.gst.gst_element_set_state)(playbin, PAUSED) };
        Ok(player)
    }

    /// Frames as BGRx at square pixels and captions as text, each as it is due; sound through
    /// `tempo` (where there is one), with no sound into `quiet`.
    fn set_sinks(&mut self, sound: bool, quiet: Object, tempo: Object) {
        let set = self.lib.gobject.g_object_set;
        let end = std::ptr::null::<c_char>();
        // SAFETY: each property has the type passed, and the list ends in a null.
        unsafe {
            let caps = (self.lib.gst.gst_caps_from_string)(c"video/x-raw,format=BGRx,pixel-aspect-ratio=1/1".as_ptr());
            set(self.sink, c"caps".as_ptr(), caps, c"max-buffers".as_ptr(), 1 as c_uint, c"drop".as_ptr(), 1 as c_int, c"wait-on-eos".as_ptr(), 0 as c_int, end);
            (self.lib.gst.gst_mini_object_unref)(caps);
            let caps = (self.lib.gst.gst_caps_from_string)(c"text/x-raw".as_ptr());
            set(self.text_sink, c"caps".as_ptr(), caps, c"max-buffers".as_ptr(), 1 as c_uint, c"drop".as_ptr(), 1 as c_int, c"wait-on-eos".as_ptr(), 0 as c_int, end);
            (self.lib.gst.gst_mini_object_unref)(caps);
            set(self.playbin, c"video-sink".as_ptr(), self.sink, c"text-sink".as_ptr(), self.text_sink, end);
            if !tempo.is_null() {
                set(self.playbin, c"audio-filter".as_ptr(), tempo, end);
            }
            if !sound && !quiet.is_null() {
                set(quiet, c"sync".as_ptr(), 1 as c_int, end);
                set(self.playbin, c"audio-sink".as_ptr(), quiet, c"mute".as_ptr(), 1 as c_int, end);
            }
        }
    }

    /// A file as its URI, an address as is, bytes through an `appsrc`.
    fn set_source(&mut self, source: VideoSource) -> Result<(), VideoError> {
        let uri = match source {
            VideoSource::File(path) => self.file_uri(&path)?,
            VideoSource::Url(url) => CString::new(url).map_err(|_| VideoError::Refused("a zero in the address".into()))?,
            VideoSource::Bytes(bytes) => self.hand_bytes(bytes),
        };
        // SAFETY: a string property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"uri".as_ptr(), uri.as_ptr(), std::ptr::null::<c_char>()) };
        Ok(())
    }
    fn file_uri(&self, path: &std::path::Path) -> Result<CString, VideoError> {
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| VideoError::Refused("a zero in the path".into()))?;
        // SAFETY: a path ending in a zero; the URI is copied and freed.
        unsafe {
            let uri = (self.lib.gst.gst_filename_to_uri)(name.as_ptr(), null_mut());
            if uri.is_null() {
                return Err(VideoError::Refused(format!("{} has no URI", path.display())));
            }
            let owned = CStr::from_ptr(uri).to_owned();
            (self.lib.glib.g_free)(uri.cast());
            Ok(owned)
        }
    }
    /// Plays bytes: `playbin` makes an `appsrc`, which reads them at any place.
    fn hand_bytes(&mut self, bytes: Vec<u8>) -> CString {
        let bytes = Box::new(Bytes { bytes, at: AtomicU64::new(0) });
        let data = (&*bytes as *const Bytes).cast_mut().cast();
        self.bytes = Some(bytes);
        // SAFETY: the callback has the signal's arguments; the bytes outlive the player.
        unsafe { (self.lib.gobject.g_signal_connect_data)(self.playbin, c"source-setup".as_ptr(), source_setup as *const c_void, data, std::ptr::null(), 0) };
        c"appsrc://".to_owned()
    }

    pub(crate) fn play(&mut self) {
        self.playing = true;
        // SAFETY: a live element.
        unsafe { (self.lib.gst.gst_element_set_state)(self.playbin, PLAYING) };
        if let Some(nth) = self.sound_wanted.take() {
            self.choose_sound(nth);
        }
    }
    pub(crate) fn pause(&mut self) {
        self.playing = false;
        // SAFETY: a live element.
        unsafe { (self.lib.gst.gst_element_set_state)(self.playbin, PAUSED) };
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        // (At the very end there is no frame to show: just before it, the last.)
        let to = self.length().map_or(to, |length| to.min(length.saturating_sub(Duration::from_millis(1))));
        let to = to.as_nanos().min(i64::MAX as u128) as i64;
        self.last = None;
        // SAFETY: a live element; flushing, to the exact frame, at the speed it plays.
        let done = self.ready && unsafe { (self.lib.gst.gst_element_seek)(self.playbin, self.speed, TIME, FLUSH_ACCURATE, FROM, to, ON, -1) } != 0;
        self.pending = (!done).then_some(to);
    }
    pub(crate) fn position(&mut self) -> Duration {
        let mut at = 0i64;
        // SAFETY: a live element, writing a time in ns.
        unsafe { (self.lib.gst.gst_element_query_position)(self.playbin, TIME, &mut at) };
        Duration::from_nanos(at.max(0) as u64)
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        let mut length = -1i64;
        // SAFETY: as above.
        let known = unsafe { (self.lib.gst.gst_element_query_duration)(self.playbin, TIME, &mut length) } != 0;
        (known && length >= 0).then(|| Duration::from_nanos(length as u64))
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        // SAFETY: a double property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"volume".as_ptr(), volume as f64, std::ptr::null::<c_char>()) };
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        // SAFETY: a boolean property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"mute".as_ptr(), muted as c_int, std::ptr::null::<c_char>()) };
    }
    /// Plays at this speed from where it is (or from a seek not yet done): a seek there at that
    /// rate.
    pub(crate) fn set_speed(&mut self, speed: f32) {
        self.speed = speed as f64;
        let here = self.pending.map_or_else(|| self.position(), |to| Duration::from_nanos(to as u64));
        self.seek(here);
    }

    pub(crate) fn news(&mut self) -> News {
        let mut news = News::default();
        // SAFETY: each message popped is read by its type and released.
        while let Some(message) = Some(unsafe { (self.lib.gst.gst_bus_pop_filtered)(self.bus, EOS | ERROR | ASYNC_DONE) }).filter(|m| !m.is_null()) {
            // SAFETY: the type follows the mini object.
            let kind = unsafe { *message.cast::<u8>().add(MESSAGE_TYPE).cast::<c_uint>() };
            match kind {
                EOS => news.ended = true,
                ERROR => news.error = Some(self.error_of(message)),
                _ => self.now_ready(),
            }
            // SAFETY: popped, so ours.
            unsafe { (self.lib.gst.gst_mini_object_unref)(message) };
        }
        news
    }
    /// The player finished a change: a seek asked before may go now.
    fn now_ready(&mut self) {
        self.ready = true;
        if let Some(to) = self.pending.take() {
            self.seek(Duration::from_nanos(to as u64));
        }
    }
    /// An error message's words.
    fn error_of(&self, message: Object) -> String {
        let (mut error, mut debug) = (null_mut::<GError>(), null_mut::<c_char>());
        // SAFETY: an error message; its error and debug text are read and freed.
        unsafe {
            (self.lib.gst.gst_message_parse_error)(message, &mut error, &mut debug);
            let words = if error.is_null() || (*error).message.is_null() { "an error".into() } else { CStr::from_ptr((*error).message).to_string_lossy().into_owned() };
            if !error.is_null() {
                (self.lib.glib.g_error_free)(error);
            }
            (self.lib.glib.g_free)(debug.cast());
            format!("the player stopped: {words}")
        }
    }

    /// The frame on show where it is not the one given last: its width and time, its pixels in
    /// `into`. (The sink's last sample: playing, the newest due; paused, the one it stands on.)
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let sample = self.last_sample(self.sink)?;
        let read = self.read_sample(sample, into);
        // SAFETY: ours, as `last_sample` gave it.
        unsafe { (self.lib.gst.gst_mini_object_unref)(sample) };
        read
    }
    /// The sample a sink took last (a new reference), if any.
    fn last_sample(&self, sink: Object) -> Option<Object> {
        let mut sample: Object = null_mut();
        // SAFETY: an object property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_get)(sink, c"last-sample".as_ptr(), &mut sample, std::ptr::null::<c_char>()) };
        (!sample.is_null()).then_some(sample)
    }
    /// A sample's frame, unless it is the last one given.
    fn read_sample(&mut self, sample: Object, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: the sample's buffer and caps are its own.
        unsafe {
            let buffer = (self.lib.gst.gst_sample_get_buffer)(sample);
            let structure = (self.lib.gst.gst_caps_get_structure)((self.lib.gst.gst_sample_get_caps)(sample), 0);
            (self.lib.gst.gst_structure_get_int)(structure, c"width".as_ptr(), &mut width);
            (self.lib.gst.gst_structure_get_int)(structure, c"height".as_ptr(), &mut height);
            if buffer.is_null() || width <= 0 || height <= 0 {
                return None;
            }
            let time = *buffer.cast::<u8>().add(BUFFER_TIME).cast::<u64>();
            if self.last == Some((buffer as usize, time)) {
                return None;
            }
            self.mapped(buffer, |rows| copy_rows(rows, width as usize, height as usize, into))?;
            self.last = Some((buffer as usize, time));
            Some((width as usize, Duration::from_nanos(if time == u64::MAX { 0 } else { time })))
        }
    }
    /// What `read` makes of a buffer's bytes, mapped for reading; none where it can't be.
    fn mapped<T>(&self, buffer: Object, read: impl FnOnce(&[u8]) -> T) -> Option<T> {
        let mut map = MapInfo { memory: null_mut(), flags: 0, data: null_mut(), size: 0, rest: [0; 9] };
        // SAFETY: a live buffer, mapped for reading (1) while its bytes are read, then unmapped.
        unsafe {
            if (self.lib.gst.gst_buffer_map)(buffer, &mut map, 1) == 0 {
                return None;
            }
            let read = read(std::slice::from_raw_parts(map.data, map.size));
            (self.lib.gst.gst_buffer_unmap)(buffer, &mut map);
            Some(read)
        }
    }

    // ---- tracks ----

    /// A number property of `playbin`.
    fn number(&self, name: &CStr) -> c_int {
        let mut value: c_int = 0;
        // SAFETY: an int property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_get)(self.playbin, name.as_ptr(), &mut value, std::ptr::null::<c_char>()) };
        value
    }
    /// The source's sound and caption tracks, once the player has read it.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        if !self.ready {
            return None;
        }
        let sound = self.streams(c"n-audio", c"get-audio-tags");
        let chosen = usize::try_from(self.number(c"current-audio")).ok().filter(|at| *at < sound.len());
        Some(Tracks { sound, captions: self.streams(c"n-text", c"get-text-tags"), sound_chosen: chosen })
    }
    /// Each stream of a kind, named by its tags.
    fn streams(&self, count: &CStr, tags_of: &CStr) -> Vec<Track> {
        (0..self.number(count)).map(|at| self.stream(tags_of, at)).collect()
    }
    /// A stream as its tags (`language-code`, `title`) name it, asked by an action signal.
    fn stream(&self, tags_of: &CStr, at: c_int) -> Track {
        let mut tags: Object = null_mut();
        // SAFETY: the signal takes the stream's number and gives its tag list (a new
        // reference, or null).
        unsafe { (self.lib.gobject.g_signal_emit_by_name)(self.playbin, tags_of.as_ptr(), at, &mut tags) };
        let track = Track { language: self.tag(tags, c"language-code"), name: self.tag(tags, c"title") };
        if !tags.is_null() {
            // SAFETY: ours, as above.
            unsafe { (self.lib.gst.gst_mini_object_unref)(tags) };
        }
        track
    }
    /// A text tag of a tag list, empty where it has none.
    fn tag(&self, tags: Object, name: &CStr) -> String {
        let mut value: *mut c_char = null_mut();
        // SAFETY: a live tag list (or none); the string it gives is copied and freed.
        unsafe {
            if tags.is_null() || (self.lib.gst.gst_tag_list_get_string)(tags, name.as_ptr(), &mut value) == 0 || value.is_null() {
                return String::new();
            }
            let text = CStr::from_ptr(value).to_string_lossy().into_owned();
            (self.lib.glib.g_free)(value.cast());
            text
        }
    }

    /// Plays the `nth` sound stream: now while playing, else from the next play.
    pub(crate) fn choose_sound(&mut self, nth: usize) {
        if !self.playing {
            self.sound_wanted = Some(nth);
            return;
        }
        // SAFETY: an int property, the list ending in a null.
        unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"current-audio".as_ptr(), nth as c_int, std::ptr::null::<c_char>()) };
    }
    /// Shows the `nth` caption stream, or none.
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        self.captions = nth.is_some();
        if let Some(nth) = nth {
            // SAFETY: an int property, the list ending in a null.
            unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"current-text".as_ptr(), nth as c_int, std::ptr::null::<c_char>()) };
        }
    }
    /// The caption on show: the text sink's last sample, while its span holds the position.
    pub(crate) fn caption(&mut self) -> Option<String> {
        if !self.captions {
            return None;
        }
        let sample = self.last_sample(self.text_sink)?;
        let at = self.position().as_nanos() as u64;
        // SAFETY: the sample's buffer is its own; its time and duration follow the mini object.
        let shown = unsafe {
            let buffer = (self.lib.gst.gst_sample_get_buffer)(sample);
            let (from, length) = match buffer.is_null() {
                true => (u64::MAX, 0),
                false => (*buffer.cast::<u8>().add(BUFFER_TIME).cast::<u64>(), *buffer.cast::<u8>().add(BUFFER_DURATION).cast::<u64>()),
            };
            let holds = from != u64::MAX && from <= at && (length == u64::MAX || at < from.saturating_add(length));
            holds.then(|| self.mapped(buffer, |bytes| crate::video_captions::plain(String::from_utf8_lossy(bytes).trim_end_matches('\0')))).flatten()
        };
        // SAFETY: ours, as `last_sample` gave it.
        unsafe { (self.lib.gst.gst_mini_object_unref)(sample) };
        shown.filter(|text| !text.trim().is_empty())
    }
}

/// How captions look: Linux desktops keep no caption settings.
pub(crate) fn caption_style() -> CaptionStyle {
    CaptionStyle::default()
}

/// BGRx rows (each `bytes.len() / height` long) into pixels, opaque.
fn copy_rows(bytes: &[u8], width: usize, height: usize, into: &mut Vec<u32>) {
    let stride = bytes.len() / height.max(1);
    into.clear();
    for row in bytes.chunks(stride.max(1)).take(height) {
        into.extend(row.as_chunks::<4>().0.iter().take(width).map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0xFF])));
    }
}

/// `playbin` made its `appsrc`: it is told the bytes' size and that it may read anywhere.
unsafe extern "C" fn source_setup(_: Object, source: Object, data: *mut c_void) {
    let Ok(lib) = libraries() else { return };
    // SAFETY: `data` is the player's bytes, alive while it plays; each property has the
    // type passed, and each callback the signal's arguments.
    unsafe {
        let bytes = &*data.cast::<Bytes>();
        let set = lib.gobject.g_object_set;
        set(source, c"stream-type".as_ptr(), 2 as c_int, c"size".as_ptr(), bytes.bytes.len() as i64, c"format".as_ptr(), 2 as c_int, std::ptr::null::<c_char>());
        (lib.gobject.g_signal_connect_data)(source, c"need-data".as_ptr(), need_data as *const c_void, data, std::ptr::null(), 0);
        (lib.gobject.g_signal_connect_data)(source, c"seek-data".as_ptr(), seek_data as *const c_void, data, std::ptr::null(), 0);
    }
}

/// The `appsrc` wants `length` bytes from where it reads.
unsafe extern "C" fn need_data(source: Object, length: c_uint, data: *mut c_void) {
    let Ok(lib) = libraries() else { return };
    // SAFETY: as in `source_setup`; the buffer is a copy, handed over.
    unsafe {
        let bytes = &*data.cast::<Bytes>();
        let at = (bytes.at.load(Ordering::Acquire) as usize).min(bytes.bytes.len());
        let end = (at + length.max(4096) as usize).min(bytes.bytes.len());
        if at == end {
            (lib.app.gst_app_src_end_of_stream)(source);
            return;
        }
        let buffer = (lib.gst.gst_buffer_new_memdup)(bytes.bytes[at..end].as_ptr().cast(), end - at);
        bytes.at.store(end as u64, Ordering::Release);
        (lib.app.gst_app_src_push_buffer)(source, buffer);
    }
}

/// The `appsrc` reads from `offset` on.
unsafe extern "C" fn seek_data(_: Object, offset: u64, data: *mut c_void) -> c_int {
    // SAFETY: as in `source_setup`.
    let bytes = unsafe { &*data.cast::<Bytes>() };
    bytes.at.store(offset, Ordering::Release);
    1
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: the pipeline is stopped (which waits for its threads), then released with
        // its bus; the bytes after.
        unsafe {
            (self.lib.gst.gst_element_set_state)(self.playbin, 1);
            (self.lib.gst.gst_object_unref)(self.bus);
            (self.lib.gst.gst_object_unref)(self.playbin);
        }
    }
}

#[cfg(test)]
/// Whether GStreamer here has an element of this kind.
pub(crate) fn has_element(kind: &CStr) -> bool {
    let Ok(lib) = libraries() else { return false };
    // SAFETY: a factory name ending in a zero; the element made is released.
    unsafe {
        let element = (lib.gst.gst_element_factory_make)(kind.as_ptr(), std::ptr::null());
        if !element.is_null() {
            (lib.gst.gst_object_unref)(element);
        }
        !element.is_null()
    }
}

#[cfg(test)]
impl Player {
    /// The sound last played into the quiet sink, its first channel, and its rate; none before
    /// any. (16-bit or float samples, told apart by their bytes against the buffer's span.)
    pub(crate) fn heard(&mut self) -> Option<(Vec<f32>, u32)> {
        let mut quiet: Object = null_mut();
        // SAFETY: an object property, the list ending in a null; released below.
        unsafe { (self.lib.gobject.g_object_get)(self.playbin, c"audio-sink".as_ptr(), &mut quiet, std::ptr::null::<c_char>()) };
        if quiet.is_null() {
            return None;
        }
        let sample = self.last_sample(quiet);
        // SAFETY: ours, as `g_object_get` gave it.
        unsafe { (self.lib.gst.gst_object_unref)(quiet) };
        let sample = sample?;
        let (mut rate, mut channels) = (0, 1);
        // SAFETY: the sample's caps and buffer are its own; its duration follows the mini object.
        let samples = unsafe {
            let structure = (self.lib.gst.gst_caps_get_structure)((self.lib.gst.gst_sample_get_caps)(sample), 0);
            (self.lib.gst.gst_structure_get_int)(structure, c"rate".as_ptr(), &mut rate);
            (self.lib.gst.gst_structure_get_int)(structure, c"channels".as_ptr(), &mut channels);
            let buffer = (self.lib.gst.gst_sample_get_buffer)(sample);
            let span = *buffer.cast::<u8>().add(BUFFER_DURATION).cast::<u64>();
            let count = (span as f64 * rate as f64 / 1e9).round() as usize * channels.max(1) as usize;
            self.mapped(buffer, |bytes| first_channel(bytes, count, channels.max(1) as usize))
        };
        // SAFETY: ours, as `last_sample` gave it.
        unsafe { (self.lib.gst.gst_mini_object_unref)(sample) };
        Some((samples.flatten()?, rate as u32))
    }
}

/// The first channel of `count` samples, 16-bit or float as their bytes say.
#[cfg(test)]
fn first_channel(bytes: &[u8], count: usize, channels: usize) -> Option<Vec<f32>> {
    match bytes.len() / count.max(1) {
        2 => Some(bytes.as_chunks::<2>().0.iter().step_by(channels).map(|s| i16::from_le_bytes(*s) as f32 / 32768.0).collect()),
        4 => Some(bytes.as_chunks::<4>().0.iter().step_by(channels).map(|s| f32::from_le_bytes(*s)).collect()),
        _ => None,
    }
}
