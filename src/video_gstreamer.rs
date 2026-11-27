//! Videos on Linux: GStreamer's `playbin`, opened with `dlopen`. Frames come from an `appsink`
//! as 32-bit BGRx, the file's captions from another as text; with no sound, audio goes to a
//! `fakesink`. Sound and caption tracks are `playbin`'s streams, named by their tags. Speed is a
//! seek at that rate, the pitch kept by `scaletempo` where it is installed. Captions in the
//! video stream (CEA-608, CEA-708) come from the caption meta parsers put on frames
//! (`h264parse`, `h265parse`), read by taut (`video_stream_captions`) through a probe on the
//! frame sink. A system without GStreamer, or without `playbin`, is told so.

use crate::dynamic_library::library;
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use crate::video_stream_captions::ClosedCaptions;
#[cfg(feature = "gpu-draw")]
use crate::video_yuv::Colours;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
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
    fn gst_buffer_get_meta(Object, usize) -> *const CaptionMeta;
    fn gst_element_get_static_pad(Object, *const c_char) -> Object;
    fn gst_pad_add_probe(Object, c_uint, *const c_void, *mut c_void, *const c_void) -> u64;
    fn gst_structure_get_string(Object, *const c_char) -> *const c_char;
});
// Dmabufs and the video meta that lays out their planes (`gpu-draw`; plugins-base's libraries).
// (A buffer's memories are GStreamer's own, found through the allocators', which links it.)
#[cfg(feature = "gpu-draw")]
library!(Dmabufs, ["libgstallocators-1.0.so.0"], {
    fn gst_is_dmabuf_memory(Object) -> c_int;
    fn gst_dmabuf_memory_get_fd(Object) -> c_int;
    fn gst_buffer_n_memory(Object) -> c_uint;
    fn gst_buffer_peek_memory(Object, c_uint) -> Object;
});
#[cfg(feature = "gpu-draw")]
library!(VideoMeta, ["libgstvideo-1.0.so.0"], {
    fn gst_buffer_get_video_meta(Object) -> *const PlanesMeta;
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
    fn g_type_from_name(*const c_char) -> usize;
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

/// `GstVideoMeta` up to its planes: its `GstMeta`, buffer, flags, format, id, width, height,
/// the number of planes and their offsets and strides.
#[cfg(feature = "gpu-draw")]
#[repr(C)]
struct PlanesMeta {
    meta: [usize; 2],
    buffer: Object,
    flags: c_uint,
    format: c_uint,
    id: c_int,
    width: c_uint,
    height: c_uint,
    planes: c_uint,
    offsets: [usize; 4],
    strides: [c_int; 4],
}

/// Where in a `GstMemory` its offset into what it holds is: after the mini object, allocator,
/// parent, maximum size and alignment.
#[cfg(feature = "gpu-draw")]
const MEMORY_OFFSET: usize = 96;

/// `GstVideoCaptionMeta`: its `GstMeta` (flags, info), the kind of its data, and the data.
#[repr(C)]
struct CaptionMeta {
    meta: [usize; 2],
    kind: c_uint,
    data: *const u8,
    size: usize,
}

/// Caption data as pairs of field 1 (`GST_VIDEO_CAPTION_TYPE_CEA608_RAW`), as triplets with
/// the field first (`..._CEA608_S334_1A`), as A/53 `cc_data` (`..._CEA708_RAW`), as SMPTE
/// 334-2 caption distribution packets (`..._CEA708_CDP`).
const CEA608_PAIRS: c_uint = 1;
const CEA608_FIELD_TRIPLETS: c_uint = 2;
const CC_DATA: c_uint = 3;
const CDP: c_uint = 4;
/// A probe of buffers, which lets each pass.
const PROBE_BUFFERS: c_uint = 1 << 4;
const PROBE_OK: c_int = 1;

/// The frames the frame sink takes: BGRx; with `gpu-draw` linear dmabufs of NV12, I420 or YV12
/// (as DRM formats, or as GStreamer's) first.
const BGRX: &CStr = c"video/x-raw,format=BGRx,pixel-aspect-ratio=1/1";
#[cfg(feature = "gpu-draw")]
const DMABUFS_OR_BGRX: &CStr = c"video/x-raw(memory:DMABuf),format=DMA_DRM,drm-format=(string){NV12,YU12,YV12},pixel-aspect-ratio=1/1;video/x-raw(memory:DMABuf),format=(string){NV12,I420,YV12},pixel-aspect-ratio=1/1;video/x-raw,format=BGRx,pixel-aspect-ratio=1/1";

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
/// loaded may still run). With `gpu-draw`, those that read dmabufs where they are installed.
struct Libraries {
    gst: Gst,
    app: App,
    gobject: GObject,
    glib: GLib,
    #[cfg(feature = "gpu-draw")]
    dmabufs: Option<(Dmabufs, VideoMeta)>,
}

fn libraries() -> Result<&'static Libraries, VideoError> {
    static OPENED: OnceLock<Option<Libraries>> = OnceLock::new();
    let opened = OPENED.get_or_init(|| {
        let libraries = Libraries {
            gst: Gst::open()?,
            app: App::open()?,
            gobject: GObject::open()?,
            glib: GLib::open()?,
            #[cfg(feature = "gpu-draw")]
            dmabufs: Dmabufs::open().zip(VideoMeta::open()),
        };
        // SAFETY: no arguments, no error asked for.
        let started = unsafe { (libraries.gst.gst_init_check)(null_mut(), null_mut(), null_mut()) };
        (started != 0).then_some(libraries)
    });
    opened.as_ref().ok_or_else(|| VideoError::NoPlayer("GStreamer is not installed".into()))
}

impl Libraries {
    /// How a sample's frame lies in `buffer`, its buffer: as its caps say, with `gpu-draw` its
    /// planes where the buffer's video meta says; none where it is of no format read here.
    fn layout(&self, sample: Object, buffer: Object) -> Option<Layout> {
        let layout = Layout::of(&self.caps_of(sample)?)?;
        #[cfg(feature = "gpu-draw")]
        let mut layout = layout;
        #[cfg(feature = "gpu-draw")]
        layout.place(self.planes_meta(buffer));
        #[cfg(not(feature = "gpu-draw"))]
        let _ = buffer;
        Some(layout)
    }
    /// What a sample's caps say of its frames.
    fn caps_of(&self, sample: Object) -> Option<Caps> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: the sample's caps are its own; the strings they lend are copied at once.
        unsafe {
            let structure = (self.gst.gst_caps_get_structure)((self.gst.gst_sample_get_caps)(sample), 0);
            (self.gst.gst_structure_get_int)(structure, c"width".as_ptr(), &mut width);
            (self.gst.gst_structure_get_int)(structure, c"height".as_ptr(), &mut height);
            let text = |name: &CStr| {
                let said = (self.gst.gst_structure_get_string)(structure, name.as_ptr());
                if said.is_null() { String::new() } else { CStr::from_ptr(said).to_string_lossy().into_owned() }
            };
            let size = (usize::try_from(width).ok().filter(|width| *width > 0)?, usize::try_from(height).ok().filter(|height| *height > 0)?);
            Some(Caps {
                size,
                format: text(c"format"),
                #[cfg(feature = "gpu-draw")]
                drm_format: text(c"drm-format"),
                #[cfg(feature = "gpu-draw")]
                colorimetry: text(c"colorimetry"),
            })
        }
    }
    /// What `read` makes of a buffer's bytes, mapped for reading; none where it can't be.
    fn mapped<T>(&self, buffer: Object, read: impl FnOnce(&[u8]) -> T) -> Option<T> {
        let mut map = MapInfo { memory: null_mut(), flags: 0, data: null_mut(), size: 0, rest: [0; 9] };
        // SAFETY: a live buffer, mapped for reading (1) while its bytes are read, then unmapped.
        unsafe {
            if (self.gst.gst_buffer_map)(buffer, &mut map, 1) == 0 {
                return None;
            }
            let read = read(std::slice::from_raw_parts(map.data, map.size));
            (self.gst.gst_buffer_unmap)(buffer, &mut map);
            Some(read)
        }
    }
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
    /// The captions in the video stream, read by the frame sink's probe (freed after the
    /// player), and which of their tracks is shown.
    in_stream: Box<InStream>,
    in_stream_shown: Option<usize>,
}

/// The captions read from the caption meta of each frame that reaches the frame sink.
struct InStream {
    lib: &'static Libraries,
    /// `GstVideoCaptionMetaAPI`'s type, once registered (by the first such meta).
    meta_kind: AtomicUsize,
    captions: Mutex<ClosedCaptions>,
}

impl InStream {
    fn captions(&self) -> MutexGuard<'_, ClosedCaptions> {
        self.captions.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `GstVideoCaptionMetaAPI`'s type, once registered; 0 before.
    fn meta_kind(&self) -> usize {
        let known = self.meta_kind.load(Ordering::Relaxed);
        if known != 0 {
            return known;
        }
        // SAFETY: a type name ending in a zero.
        let kind = unsafe { (self.lib.gobject.g_type_from_name)(c"GstVideoCaptionMetaAPI".as_ptr()) };
        self.meta_kind.store(kind, Ordering::Relaxed);
        kind
    }

    /// Reads a buffer's caption meta, if it has one.
    fn read(&self, buffer: Object) {
        let kind = self.meta_kind();
        if kind == 0 {
            return;
        }
        // SAFETY: a live buffer: its time follows the mini object; its meta, where it has one,
        // lives as long as it, and its data is as long as it says.
        let (time, meta) = unsafe { (*buffer.cast::<u8>().add(BUFFER_TIME).cast::<u64>(), (self.lib.gst.gst_buffer_get_meta)(buffer, kind).as_ref()) };
        let Some(meta) = meta.filter(|meta| time != u64::MAX && !meta.data.is_null()) else { return };
        // SAFETY: as above.
        let data = unsafe { std::slice::from_raw_parts(meta.data, meta.size) };
        let (mut captions, at) = (self.captions(), Duration::from_nanos(time));
        match meta.kind {
            CC_DATA => captions.read_cc_data(data, at),
            CDP => captions.read_cc_data(cc_data_of_cdp(data).unwrap_or_default(), at),
            CEA608_PAIRS => data.as_chunks::<2>().0.iter().for_each(|pair| captions.read_cea608_pair(*pair, at)),
            CEA608_FIELD_TRIPLETS => data.as_chunks::<3>().0.iter().filter(|three| three[0] & 0x80 != 0).for_each(|three| captions.read_cea608_pair([three[1], three[2]], at)),
            _ => {}
        }
    }
}

/// The `cc_data` triplets of a SMPTE 334-2 caption distribution packet: after its header
/// (`96 69`, length, frame rate, flags, sequence) and time code where it has one, a section
/// `72` of `cc_count` triplets; none where it has none.
fn cc_data_of_cdp(cdp: &[u8]) -> Option<&[u8]> {
    let [0x96, 0x69, _, _, flags, _, _, rest @ ..] = cdp else { return None };
    let rest = if flags & 0x80 != 0 { rest.get(5..)? } else { rest };
    let [0x72, count, triplets @ ..] = rest else { return None };
    (flags & 0x40 != 0).then(|| triplets.get(..(count & 0x1F) as usize * 3)).flatten()
}

/// A frame reached the frame sink: its captions are read.
unsafe extern "C" fn frame_came(_: Object, info: *mut c_void, data: *mut c_void) -> c_int {
    // SAFETY: `data` is the player's `InStream`, alive while the pipeline runs; a buffer
    // probe's info holds the buffer after its type and id.
    unsafe { (*data.cast::<InStream>()).read(*info.cast::<u8>().add(16).cast::<Object>()) };
    PROBE_OK
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
        let in_stream = Box::new(InStream { lib, meta_kind: AtomicUsize::new(0), captions: Mutex::default() });
        let mut player = Player {
            lib,
            playbin,
            sink,
            text_sink,
            bus,
            playing: false,
            pending: None,
            ready: false,
            last: None,
            bytes: None,
            speed: 1.0,
            captions: false,
            sound_wanted: None,
            in_stream,
            in_stream_shown: None,
        };
        player.read_captions_in_stream();
        player.set_sinks(sound, make(c"fakesink"), make(c"scaletempo"));
        player.set_source(source)?;
        // SAFETY: a live element.
        unsafe { (lib.gst.gst_element_set_state)(playbin, PAUSED) };
        Ok(player)
    }

    /// Frames as BGRx at square pixels (with `gpu-draw`, as linear dmabufs of YUV 4:2:0 first,
    /// where the decoder gives those) and captions as text, each as it is due; sound through
    /// `tempo` (where there is one), with no sound into `quiet`.
    fn set_sinks(&mut self, sound: bool, quiet: Object, tempo: Object) {
        let set = self.lib.gobject.g_object_set;
        let end = std::ptr::null::<c_char>();
        #[cfg(not(feature = "gpu-draw"))]
        let frames = BGRX;
        #[cfg(feature = "gpu-draw")]
        let frames = if self.lib.dmabufs.is_some() { DMABUFS_OR_BGRX } else { BGRX };
        // SAFETY: each property has the type passed, and the list ends in a null.
        unsafe {
            let caps = (self.lib.gst.gst_caps_from_string)(frames.as_ptr());
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

    /// Reads each frame's captions as it reaches the frame sink (a buffer probe on its pad).
    fn read_captions_in_stream(&mut self) {
        let data = (&*self.in_stream as *const InStream).cast_mut().cast();
        // SAFETY: the sink's pad, released once the probe is on it; the callback has a probe's
        // arguments, and the data outlives the pipeline.
        unsafe {
            let pad = (self.lib.gst.gst_element_get_static_pad)(self.sink, c"sink".as_ptr());
            if !pad.is_null() {
                (self.lib.gst.gst_pad_add_probe)(pad, PROBE_BUFFERS, frame_came as *const c_void, data, std::ptr::null());
                (self.lib.gst.gst_object_unref)(pad);
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
        self.in_stream.captions().sought(to);
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
        news.tracks_changed = self.ready && self.in_stream.captions().unlisted();
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
    /// A sample's frame made RGB, unless it is the last one given: its width and time, its
    /// pixels in `into`.
    fn read_sample(&mut self, sample: Object, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let (buffer, time) = self.new_buffer(sample)?;
        let layout = self.lib.layout(sample, buffer)?;
        if !self.lib.mapped(buffer, |bytes| layout.read(bytes, into))? {
            return None;
        }
        self.last = Some((buffer as usize, time));
        Some((layout.width, Duration::from_nanos(if time == u64::MAX { 0 } else { time })))
    }
    /// A sample's buffer and its time, unless it is the last one given.
    fn new_buffer(&self, sample: Object) -> Option<(Object, u64)> {
        // SAFETY: the sample's buffer is its own; its time follows the mini object.
        unsafe {
            let buffer = (self.lib.gst.gst_sample_get_buffer)(sample);
            let time = if buffer.is_null() { u64::MAX } else { *buffer.cast::<u8>().add(BUFFER_TIME).cast::<u64>() };
            (!buffer.is_null() && self.last != Some((buffer as usize, time))).then_some((buffer, time))
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
    /// The source's sound and caption tracks, once the player has read it; the captions in the
    /// video stream after the source's own, once they came.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        if !self.ready {
            return None;
        }
        let sound = self.streams(c"n-audio", c"get-audio-tags");
        let chosen = usize::try_from(self.number(c"current-audio")).ok().filter(|at| *at < sound.len());
        let mut captions = self.streams(c"n-text", c"get-text-tags");
        captions.extend(self.in_stream.captions().tracks());
        Some(Tracks { sound, captions, sound_chosen: chosen })
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
    /// Shows the `nth` caption stream (the tracks in the video stream after them), or none.
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        let streams = usize::try_from(self.number(c"n-text")).unwrap_or(0);
        self.in_stream_shown = nth.and_then(|nth| nth.checked_sub(streams));
        let nth = nth.filter(|nth| *nth < streams);
        self.captions = nth.is_some();
        if let Some(nth) = nth {
            // SAFETY: an int property, the list ending in a null.
            unsafe { (self.lib.gobject.g_object_set)(self.playbin, c"current-text".as_ptr(), nth as c_int, std::ptr::null::<c_char>()) };
        }
    }
    /// The caption on show: the text sink's last sample, while its span holds the position; or
    /// the captions in the video stream's at the position.
    pub(crate) fn caption(&mut self) -> Option<String> {
        if let Some(track) = self.in_stream_shown {
            let at = self.position();
            return self.in_stream.captions().text_at(track, at);
        }
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
            holds.then(|| self.lib.mapped(buffer, |bytes| crate::video_captions::plain(String::from_utf8_lossy(bytes).trim_end_matches('\0')))).flatten()
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

/// What a sample's caps say of its frames: their size, format (GStreamer's, `DMA_DRM` for a
/// DRM format) and, read with `gpu-draw`, DRM format and colorimetry (each empty where unsaid).
struct Caps {
    size: (usize, usize),
    format: String,
    #[cfg(feature = "gpu-draw")]
    drm_format: String,
    #[cfg(feature = "gpu-draw")]
    colorimetry: String,
}

/// How a frame lies in its buffer: its size; BGRx rows, or with `gpu-draw` YUV 4:2:0 planes and
/// how they become RGB.
struct Layout {
    width: usize,
    height: usize,
    #[cfg(feature = "gpu-draw")]
    yuv: Option<Yuv>,
}

/// A frame of YUV 4:2:0: how it becomes RGB, its planes (Y, U, V; or Y and UV, each pair U
/// first), and whether its buffer holds V before U (YV12).
#[cfg(feature = "gpu-draw")]
#[derive(Clone)]
struct Yuv {
    colours: Colours,
    planes: Vec<Plane>,
    v_first: bool,
}

/// A plane of a frame: where it starts in its buffer, and its bytes a row.
#[cfg(feature = "gpu-draw")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub(crate) offset: u64,
    pub(crate) pitch: u32,
}

impl Layout {
    /// As `caps` say: BGRx; with `gpu-draw` NV12, I420 or YV12 too, their planes where
    /// GStreamer puts them unless told (`place`). None for any other.
    fn of(caps: &Caps) -> Option<Layout> {
        let (width, height) = caps.size;
        match caps.format.as_str() {
            "BGRx" => Some(Layout {
                width,
                height,
                #[cfg(feature = "gpu-draw")]
                yuv: None,
            }),
            #[cfg(feature = "gpu-draw")]
            _ => Layout::of_yuv(caps),
            #[cfg(not(feature = "gpu-draw"))]
            _ => None,
        }
    }

    /// Reads its frame from its buffer's `bytes` into `into`, made RGB: whether it could.
    fn read(&self, bytes: &[u8], into: &mut Vec<u32>) -> bool {
        #[cfg(feature = "gpu-draw")]
        if let Some(yuv) = &self.yuv {
            return yuv.read(bytes, [self.width, self.height], into);
        }
        copy_rows(bytes, self.width, self.height, into);
        true
    }
}

#[cfg(feature = "gpu-draw")]
impl Layout {
    /// A YUV 4:2:0 frame as `caps` say, its planes where GStreamer lays them out by default.
    fn of_yuv(caps: &Caps) -> Option<Layout> {
        let (width, height) = caps.size;
        let format = match caps.format.as_str() {
            // (A DRM format named with a modifier is laid out its own way: not read here.)
            "DMA_DRM" => match caps.drm_format.as_str() {
                "NV12" => "NV12",
                "YU12" => "I420",
                "YV12" => "YV12",
                _ => return None,
            },
            format @ ("NV12" | "I420" | "YV12") => format,
            _ => return None,
        };
        let (pitch, rows) = (width.next_multiple_of(4) as u64, height.next_multiple_of(2) as u64);
        let chroma_pitch = (width.next_multiple_of(2) as u64 / 2).next_multiple_of(4);
        let planes = match format {
            "NV12" => vec![Plane { offset: 0, pitch: pitch as u32 }, Plane { offset: pitch * rows, pitch: pitch as u32 }],
            _ => vec![
                Plane { offset: 0, pitch: pitch as u32 },
                Plane { offset: pitch * rows, pitch: chroma_pitch as u32 },
                Plane { offset: pitch * rows + chroma_pitch * rows / 2, pitch: chroma_pitch as u32 },
            ],
        };
        let colours = colours_of(&caps.colorimetry, height);
        let mut yuv = Yuv { colours, planes, v_first: format == "YV12" };
        if yuv.v_first {
            yuv.planes.swap(1, 2);
        }
        Some(Layout { width, height, yuv: Some(yuv) })
    }

    /// Takes the planes where its buffer's video meta says (in the buffer's order), if it has
    /// one of as many planes.
    fn place(&mut self, meta: Option<Vec<Plane>>) {
        let (Some(yuv), Some(mut planes)) = (self.yuv.as_mut(), meta) else { return };
        if planes.len() != yuv.planes.len() {
            return;
        }
        if yuv.v_first {
            planes.swap(1, 2);
        }
        yuv.planes = planes;
    }
}

#[cfg(feature = "gpu-draw")]
impl Yuv {
    /// Reads a frame `size` big from its buffer's `bytes` into `into`, made RGB: whether all its
    /// planes lie in the bytes.
    fn read(&self, bytes: &[u8], [width, height]: [usize; 2], into: &mut Vec<u32>) -> bool {
        let rows = [height, height.div_ceil(2), height.div_ceil(2)];
        let within = self.planes.iter().zip(rows).all(|(plane, rows)| plane.offset as usize + plane.pitch as usize * rows <= bytes.len());
        if !within {
            return false;
        }
        let plane = |nth: usize, skip: usize, step: usize| {
            let Plane { offset, pitch } = self.planes[nth];
            crate::video_yuv::Plane { bytes: &bytes[offset as usize + skip..], row: pitch as usize, step }
        };
        let [y, u, v] = match self.planes.len() {
            2 => [plane(0, 0, 1), plane(1, 0, 2), plane(1, 1, 2)],
            _ => [plane(0, 0, 1), plane(1, 0, 1), plane(2, 0, 1)],
        };
        *into = self.colours.pixels([&y, &u, &v], [0, 0, width, height]);
        true
    }
}

/// How YUV becomes RGB as GStreamer's colorimetry says ("bt709", or range:matrix:… with range 1
/// full and matrix 3 BT.709, 4 BT.601, 6 BT.2020); unsaid, BT.601 below 720 lines and BT.709
/// from there, limited range.
#[cfg(feature = "gpu-draw")]
fn colours_of(colorimetry: &str, height: usize) -> Colours {
    use crate::video_yuv::{BT601, BT709, BT2020};
    let mut parts = colorimetry.split(':');
    let (range, matrix) = match (colorimetry, parts.next(), parts.next()) {
        ("bt709", ..) => ("2", "3"),
        ("bt601" | "smpte240m", ..) => ("2", "4"),
        ("bt2020" | "bt2100-pq" | "bt2100-hlg", ..) => ("2", "6"),
        ("jpeg", ..) => ("1", "4"),
        (_, Some(range), Some(matrix)) => (range, matrix),
        _ => ("2", if height >= 720 { "3" } else { "4" }),
    };
    let weights = match matrix {
        "3" => BT709,
        "6" => BT2020,
        _ => BT601,
    };
    Colours::new(weights, range == "1")
}

/// A frame's dmabuf: its file (the sample's, open while it is held), its size, and its planes
/// (Y, U, V or Y, UV), each where it starts in the file.
#[cfg(feature = "gpu-draw")]
pub(crate) struct Dmabuf {
    pub(crate) fd: c_int,
    pub(crate) size: u64,
    pub(crate) planes: Vec<Plane>,
}

/// A frame left in the decoder's dmabuf, with `gpu-draw`: its sample (held), the dmabuf the GPU
/// imports, how it lies and becomes RGB, and its size (visible, all of it: the same).
#[cfg(feature = "gpu-draw")]
pub(crate) struct FrameTexture {
    lib: &'static Libraries,
    sample: Object,
    pub(crate) dmabuf: Dmabuf,
    layout: Layout,
    pub(crate) size: (u32, u32),
    pub(crate) whole: (u32, u32),
}

// SAFETY: a sample may be read and released from any thread.
#[cfg(feature = "gpu-draw")]
unsafe impl Send for FrameTexture {}
// SAFETY: as above.
#[cfg(feature = "gpu-draw")]
unsafe impl Sync for FrameTexture {}

#[cfg(feature = "gpu-draw")]
impl FrameTexture {
    /// Reads its frame into `into`, made RGB: whether it could. (A linear dmabuf maps as the
    /// CPU's memory.)
    pub(crate) fn read(&self, into: &mut Vec<u32>) -> bool {
        #[cfg(test)]
        crate::video::READ_BACK.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the sample's buffer is its own.
        let buffer = unsafe { (self.lib.gst.gst_sample_get_buffer)(self.sample) };
        !buffer.is_null() && self.lib.mapped(buffer, |bytes| self.layout.read(bytes, into)) == Some(true)
    }

    /// How the shaders make its YUV RGB (`video_yuv::FROM_YUV`).
    pub(crate) fn for_shaders(&self) -> [f32; 8] {
        self.layout.yuv.as_ref().map_or([0.0; 8], |yuv| yuv.colours.for_shaders(self.whole))
    }

    /// What tells its dmabuf apart from the decoder's others: its file's inode.
    pub(crate) fn key(&self) -> Option<u64> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(format!("/proc/self/fd/{}", self.dmabuf.fd)).ok().map(|file| file.ino())
    }
}

#[cfg(feature = "gpu-draw")]
impl Drop for FrameTexture {
    fn drop(&mut self) {
        // SAFETY: the sample this holds.
        unsafe { (self.lib.gst.gst_mini_object_unref)(self.sample) };
    }
}

#[cfg(feature = "gpu-draw")]
impl Player {
    /// Whether frames may come as textures (`frame_texture`): always, with `gpu-draw` (those that
    /// are not in a dmabuf come as pixels).
    pub(crate) fn decodes_on_gpu(&self) -> bool {
        true
    }

    /// The frame on show where it is not the one given last and is a dmabuf of YUV 4:2:0, left
    /// there: it and its time. (Any other is left for `frame`.)
    pub(crate) fn frame_texture(&mut self) -> Option<(std::sync::Arc<FrameTexture>, Duration)> {
        let sample = self.last_sample(self.sink)?;
        let texture = self.texture_of(sample);
        if texture.is_none() {
            // SAFETY: ours, as `last_sample` gave it.
            unsafe { (self.lib.gst.gst_mini_object_unref)(sample) };
        }
        texture
    }

    /// `sample` (handed over where this gives a frame) as a frame left in its dmabuf: none where
    /// it is the last one given, or not in one dmabuf.
    fn texture_of(&mut self, sample: Object) -> Option<(std::sync::Arc<FrameTexture>, Duration)> {
        let (buffer, time) = self.new_buffer(sample)?;
        let texture = FrameTexture::of(self.lib, sample)?;
        self.last = Some((buffer as usize, time));
        Some((std::sync::Arc::new(texture), Duration::from_nanos(if time == u64::MAX { 0 } else { time })))
    }
}

#[cfg(feature = "gpu-draw")]
impl FrameTexture {
    /// `sample` (handed over where this gives a frame) as a frame left in its dmabuf: none where
    /// it is not of YUV 4:2:0 in one dmabuf.
    fn of(lib: &'static Libraries, sample: Object) -> Option<FrameTexture> {
        // SAFETY: the sample's buffer is its own.
        let buffer = unsafe { (lib.gst.gst_sample_get_buffer)(sample) };
        let layout = lib.layout(sample, buffer)?;
        let dmabuf = lib.dmabuf_of(buffer, &layout)?;
        let size = (layout.width as u32, layout.height as u32);
        Some(FrameTexture { lib, sample, dmabuf, layout, size, whole: size })
    }
}

#[cfg(feature = "gpu-draw")]
impl Libraries {
    /// The dmabuf a frame laid out as `layout` lies in: its buffer's one memory, where that is a
    /// dmabuf; the planes' places counted from the file's start.
    fn dmabuf_of(&self, buffer: Object, layout: &Layout) -> Option<Dmabuf> {
        let ((dmabufs, _), yuv) = (self.dmabufs.as_ref()?, layout.yuv.as_ref()?);
        // SAFETY: a live buffer; its memory lives as long as it, and its offset and size follow
        // its header.
        unsafe {
            if buffer.is_null() || (dmabufs.gst_buffer_n_memory)(buffer) != 1 {
                return None;
            }
            let memory = (dmabufs.gst_buffer_peek_memory)(buffer, 0);
            if memory.is_null() || (dmabufs.gst_is_dmabuf_memory)(memory) == 0 {
                return None;
            }
            let fd = (dmabufs.gst_dmabuf_memory_get_fd)(memory);
            let [offset, size] = [MEMORY_OFFSET, MEMORY_OFFSET + 8].map(|at| *memory.cast::<u8>().add(at).cast::<usize>() as u64);
            let planes = yuv.planes.iter().map(|plane| Plane { offset: plane.offset + offset, pitch: plane.pitch }).collect();
            (fd >= 0).then_some(Dmabuf { fd, size: offset + size, planes })
        }
    }

    /// The planes a buffer's video meta places, in its order; none where it has none.
    fn planes_meta(&self, buffer: Object) -> Option<Vec<Plane>> {
        let (_, meta) = self.dmabufs.as_ref()?;
        // SAFETY: a live buffer; its meta, where it has one, lives as long as it.
        let meta = unsafe { (meta.gst_buffer_get_video_meta)(buffer).as_ref()? };
        let count = (meta.planes as usize).min(4);
        Some((0..count).map(|nth| Plane { offset: meta.offsets[nth] as u64, pitch: meta.strides[nth].max(0) as u32 }).collect())
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
            self.lib.mapped(buffer, |bytes| first_channel(bytes, count, channels.max(1) as usize))
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

#[cfg(all(test, feature = "gpu-draw"))]
pub(crate) mod tests {
    use super::*;
    use crate::video_yuv::{BT601, BT709, BT2020};
    use std::os::fd::IntoRawFd;

    library!(Making, ["libgstallocators-1.0.so.0"], {
        fn gst_dmabuf_allocator_new() -> Object;
        fn gst_dmabuf_allocator_alloc(Object, c_int, usize) -> Object;
    });
    library!(MakingCore, ["libgstreamer-1.0.so.0"], {
        fn gst_buffer_new() -> Object;
        fn gst_buffer_append_memory(Object, Object);
        fn gst_sample_new(Object, Object, *const c_void, Object) -> Object;
    });
    library!(MakingMeta, ["libgstvideo-1.0.so.0"], {
        fn gst_buffer_add_video_meta_full(Object, c_uint, c_uint, c_uint, c_uint, c_uint, *const usize, *const c_int) -> *mut c_void;
    });

    unsafe extern "C" {
        fn memfd_create(name: *const u8, flags: u32) -> c_int;
    }

    /// The samples of the test frames: Y, U and V at each place (U and V of each 2 × 2 pixels).
    fn luma(x: usize, y: usize) -> u8 {
        ((x * 3 + y * 5) % 220 + 16) as u8
    }
    fn blue(x: usize, y: usize) -> u8 {
        ((x * 11 + y * 3) % 200 + 28) as u8
    }
    fn red(x: usize, y: usize) -> u8 {
        ((x * 5 + y * 13) % 200 + 28) as u8
    }

    /// A test frame `size` big of YUV 4:2:0, NV12 (two planes) or I420 (three), each plane's
    /// rows `pitch` bytes apart and 256 bytes between planes: its bytes and planes.
    pub(crate) fn yuv_bytes((width, height): (usize, usize), planes: usize, pitch: usize) -> (Vec<u8>, Vec<Plane>) {
        let (chroma, rows) = ((width.div_ceil(2), height.div_ceil(2)), height);
        let starts = [0, pitch * rows + 256, pitch * rows + 256 + pitch * chroma.1 + 256];
        let mut bytes = vec![0u8; starts[2] + pitch * chroma.1];
        for (y, x) in (0..height).flat_map(|y| (0..width).map(move |x| (y, x))) {
            bytes[y * pitch + x] = luma(x, y);
        }
        for (y, x) in (0..chroma.1).flat_map(|y| (0..chroma.0).map(move |x| (y, x))) {
            match planes {
                2 => [bytes[starts[1] + y * pitch + x * 2], bytes[starts[1] + y * pitch + x * 2 + 1]] = [blue(x, y), red(x, y)],
                _ => [bytes[starts[1] + y * pitch + x], bytes[starts[2] + y * pitch + x]] = [blue(x, y), red(x, y)],
            }
        }
        (bytes, starts[..planes].iter().map(|start| Plane { offset: *start as u64, pitch: pitch as u32 }).collect())
    }

    /// The test frame made RGB as the CPU does, by `colours`.
    pub(crate) fn rgb_of((width, height): (usize, usize), colours: Colours) -> Vec<u32> {
        (0..height).flat_map(|y| (0..width).map(move |x| colours.rgb(luma(x, y), blue(x / 2, y / 2), red(x / 2, y / 2)))).collect()
    }

    /// A sample of `bytes` in a dmabuf-like file (a memfd, as GStreamer's dmabuf memory), `caps`
    /// saying what it is, and a video meta of `planes` (GStreamer's `format`). None where
    /// GStreamer or its dmabuf allocator is missing.
    pub(crate) fn sample_of(bytes: &[u8], caps: &CStr, (format, size, planes): (c_uint, (usize, usize), &[Plane])) -> Option<Object> {
        let (lib, making, core, meta) = (libraries().ok()?, Making::open()?, MakingCore::open()?, MakingMeta::open()?);
        // SAFETY: a new memfd, written whole and handed to GStreamer (which closes it).
        let fd = unsafe { memfd_create(c"taut-frame".as_ptr().cast(), 0) };
        assert!(fd >= 0, "a memfd");
        let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        std::io::Write::write_all(&mut file, bytes).unwrap();
        let (offsets, strides): (Vec<usize>, Vec<c_int>) = (0..4).map(|nth| planes.get(nth).map_or((0, 0), |plane| (plane.offset as usize, plane.pitch as c_int))).unzip();
        // SAFETY: GStreamer's documented calls; what is made is handed on, the sample last.
        unsafe {
            let memory = (making.gst_dmabuf_allocator_alloc)((making.gst_dmabuf_allocator_new)(), file.into_raw_fd(), bytes.len());
            let buffer = (core.gst_buffer_new)();
            (core.gst_buffer_append_memory)(buffer, memory);
            (meta.gst_buffer_add_video_meta_full)(buffer, 0, format, size.0 as c_uint, size.1 as c_uint, planes.len() as c_uint, offsets.as_ptr(), strides.as_ptr());
            let caps = (lib.gst.gst_caps_from_string)(caps.as_ptr());
            let sample = (core.gst_sample_new)(buffer, caps, std::ptr::null(), null_mut());
            (lib.gst.gst_mini_object_unref)(buffer);
            (lib.gst.gst_mini_object_unref)(caps);
            Some(sample)
        }
    }

    /// The test frame, 64 × 48 NV12 in BT.709 limited range, its rows 80 bytes apart, as a
    /// frame left in its dmabuf; none where GStreamer or its dmabuf allocator is missing.
    pub(crate) fn dmabuf_frame() -> Option<std::sync::Arc<FrameTexture>> {
        let (bytes, planes) = yuv_bytes((64, 48), 2, 80);
        let caps = c"video/x-raw(memory:DMABuf),format=NV12,width=64,height=48,colorimetry=bt709";
        let sample = sample_of(&bytes, caps, (23, (64, 48), &planes))?;
        Some(std::sync::Arc::new(FrameTexture::of(libraries().ok()?, sample).expect("a frame of the sample")))
    }

    fn caps(format: &str, drm_format: &str, colorimetry: &str) -> Caps {
        Caps { size: (6, 5), format: format.into(), drm_format: drm_format.into(), colorimetry: colorimetry.into() }
    }

    #[test]
    fn a_yuv_frame_lies_where_gstreamer_lays_it_out_by_default() {
        let planes = |caps: &Caps| Layout::of(caps).and_then(|layout| layout.yuv).map(|yuv| yuv.planes.iter().map(|plane| (plane.offset, plane.pitch)).collect::<Vec<_>>());
        // (Rows of 8 bytes: 6 rounded up to 4; 6 rows: 5 rounded up to 2; chroma rows of 4.)
        assert_eq!(planes(&caps("NV12", "", "")), Some(vec![(0, 8), (48, 8)]));
        assert_eq!(planes(&caps("I420", "", "")), Some(vec![(0, 8), (48, 4), (60, 4)]));
        assert_eq!(planes(&caps("YV12", "", "")), Some(vec![(0, 8), (60, 4), (48, 4)]), "U and V found where YV12 puts them");
        assert_eq!(planes(&caps("DMA_DRM", "YU12", "")), planes(&caps("I420", "", "")));
        assert_eq!(planes(&caps("BGRx", "", "")), None, "BGRx is no YUV");
    }

    #[test]
    fn a_drm_format_with_a_modifier_or_another_format_is_not_read_here() {
        assert!(Layout::of(&caps("DMA_DRM", "NV12:0x0100000000000002", "")).is_none());
        assert!(Layout::of(&caps("P010_10LE", "", "")).is_none());
    }

    #[test]
    fn the_video_meta_s_planes_are_taken_in_the_buffer_s_order() {
        let mut layout = Layout::of(&caps("YV12", "", "")).expect("YV12");
        let at = |offset| Plane { offset, pitch: 16 };
        layout.place(Some(vec![at(0), at(100), at(200)]));
        assert_eq!(layout.yuv.as_ref().map(|yuv| yuv.planes.clone()), Some(vec![at(0), at(200), at(100)]), "V first in the buffer");
        layout.place(Some(vec![at(0), at(100)]));
        assert_eq!(layout.yuv.map(|yuv| yuv.planes.len()), Some(3), "a meta of other planes is not taken");
    }

    #[test]
    fn colorimetry_says_how_yuv_becomes_rgb() {
        assert_eq!(colours_of("bt709", 480), Colours::new(BT709, false));
        assert_eq!(colours_of("bt2020", 480), Colours::new(BT2020, false));
        assert_eq!(colours_of("1:4:0:0", 1080), Colours::new(BT601, true));
        assert_eq!(colours_of("2:3:5:1", 480), Colours::new(BT709, false));
        assert_eq!((colours_of("", 480), colours_of("", 720)), (Colours::new(BT601, false), Colours::new(BT709, false)));
    }

    #[test]
    fn nv12_i420_and_yv12_are_read_as_the_cpu_makes_their_samples_rgb() {
        let colours = Colours::new(BT709, false);
        let wanted = rgb_of((64, 48), colours);
        for (format, planes) in [("NV12", 2), ("I420", 3), ("YV12", 3)] {
            let (bytes, mut placed) = yuv_bytes((64, 48), planes, 72);
            if format == "YV12" {
                placed.swap(1, 2);
            }
            let mut layout = Layout::of(&Caps { size: (64, 48), format: format.into(), drm_format: String::new(), colorimetry: "bt709".into() }).expect(format);
            layout.place(Some(placed));
            let mut read = Vec::new();
            assert!(layout.read(&bytes, &mut read), "{format} read");
            assert!(read == wanted, "{format}: the pixels the CPU makes of its samples");
        }
    }

    #[test]
    fn a_dmabufs_frame_is_left_there_and_read_back_as_the_cpu_makes_it_rgb() {
        let Some(frame) = dmabuf_frame() else { return println!("no GStreamer or no dmabuf allocator here: skipped") };
        assert_eq!((frame.size, frame.whole), ((64, 48), (64, 48)));
        assert_eq!(frame.dmabuf.planes, [Plane { offset: 0, pitch: 80 }, Plane { offset: 80 * 48 + 256, pitch: 80 }], "the planes where the video meta says");
        assert!(frame.key().is_some(), "its file is known by its inode");
        let mut read = Vec::new();
        assert!(frame.read(&mut read));
        assert!(read == rgb_of((64, 48), Colours::new(BT709, false)), "read back as the CPU makes its samples RGB");
        assert_eq!(frame.for_shaders(), Colours::new(BT709, false).for_shaders((64, 48)));
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;

    #[test]
    fn a_caption_distribution_packets_cc_data_is_found_after_its_time_code() {
        let triplets = [0xFC, 0x94, 0x20, 0xFF, 0x02, 0x21];
        let with_time_code = [&[0x96, 0x69, 20, 0x4F, 0xC3, 0, 1, 0x71, 0, 0, 0, 0, 0x72, 0xE2][..], &triplets, &[0x74, 0, 1, 0]].concat();
        let without = [&[0x96, 0x69, 15, 0x4F, 0x43, 0, 1, 0x72, 0xE2][..], &triplets].concat();
        assert_eq!(cc_data_of_cdp(&with_time_code), Some(&triplets[..]));
        assert_eq!(cc_data_of_cdp(&without), Some(&triplets[..]));
        assert_eq!(cc_data_of_cdp(&[0x96, 0x69, 9, 0x4F, 0x03, 0, 1, 0x72, 0xE2]), None);
    }
}
