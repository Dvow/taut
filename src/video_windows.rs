//! Videos on Windows: Media Foundation's Media Engine, loaded at run time, in frame-server mode
//! (no window of its own).
//!
//! - Decoding: where the machine has a GPU, the engine is handed a Direct3D 11 device (one for
//!   the program) and decodes there (DXVA) what the GPU decodes; each new frame goes into a
//!   texture and is copied to one the CPU reads. Elsewhere it decodes in software into a WIC
//!   bitmap.
//! - Sound tracks are the engine's audio streams; caption tracks the file's own text tracks, as
//!   its timed text gives them.
//!
//! Functions are called by vtable index, as the SDK's headers number them.

use crate::color::Color;
use crate::layer_d3d11::{B8G8R8A8_UNORM, Pixels, TextureDescription};
use crate::layout::Align;
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use crate::video_captions::{Caption, Cue, Down, Place};
use crate::video_mp4::{FileCaptions, Source};
use crate::video_read_ahead::{ReadAhead, SeiReading, Step};
use crate::video_sei::Codec;
use crate::video_stream_captions::ClosedCaptions;
use crate::video_windows_download::{Download, byte_stream};
use crate::windows::{Com, function_in, named, system_library, wide};
use std::ffi::{CStr, c_void};
use std::ptr::{null, null_mut};
#[cfg(feature = "gpu-draw")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

#[link(name = "ole32")]
unsafe extern "system" {
    fn PropVariantClear(value: *mut PropVariant) -> i32;
    fn CoTaskMemFree(memory: *mut c_void);
}
#[cfg(feature = "gpu-draw")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CloseHandle(handle: usize) -> i32;
}

const ENGINE_FACTORY: [u32; 4] = named(0xB443_92DA, 0x499B, 0x446B, [0xA4, 0xCB, 0x00, 0x5F, 0xEA, 0xD0, 0xE6, 0xD5]);
const AN_ENGINE_FACTORY: [u32; 4] = named(0x4D64_5ACE, 0x26AA, 0x4688, [0x9B, 0xE1, 0xDF, 0x35, 0x16, 0x99, 0x0B, 0x93]);
const AN_ENGINE_EX: [u32; 4] = named(0x8301_5EAD, 0xB1E6, 0x40D0, [0xA9, 0x8A, 0x37, 0x14, 0x5F, 0xFE, 0x1A, 0xD1]);
/// `IMFMediaEngineNotify` and `IMFTimedTextNotify`.
const A_NOTIFY: [u32; 4] = named(0xFEE7_C112, 0xE776, 0x42B5, [0x9B, 0xBF, 0x00, 0x48, 0x52, 0x4E, 0x2B, 0xD5]);
const A_CUE_NOTIFY: [u32; 4] = named(0xDF6B_87B6, 0xCE12, 0x45DB, [0xAB, 0xA7, 0x43, 0x2F, 0xE0, 0x54, 0xE5, 0x7D]);
const AN_UNKNOWN: [u32; 4] = named(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
/// `MF_MEDIA_ENGINE_CALLBACK`, `MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT`,
/// `MF_MEDIA_ENGINE_DXGI_MANAGER`.
const CALLBACK: [u32; 4] = named(0xC603_81B8, 0x83A4, 0x41F8, [0xA3, 0xD0, 0xDE, 0x05, 0x07, 0x68, 0x49, 0xA9]);
const OUTPUT_FORMAT: [u32; 4] = named(0x5066_893C, 0x8CF9, 0x42BC, [0x8B, 0x8A, 0x47, 0x22, 0x12, 0xE5, 0x27, 0x26]);
const DXGI_MANAGER: [u32; 4] = named(0x0657_02DA, 0x1094, 0x486D, [0x86, 0x17, 0xEE, 0x7C, 0xC4, 0xEE, 0x46, 0x48]);
const WIC_FACTORY: [u32; 4] = named(0xCACA_F262, 0x9370, 0x4615, [0xA1, 0x3B, 0x9F, 0x55, 0x39, 0xDA, 0x4C, 0x0A]);
const A_WIC_FACTORY: [u32; 4] = named(0xEC5E_C8A9, 0xC395, 0x4314, [0x9C, 0x77, 0x54, 0xD7, 0xA9, 0x35, 0xFF, 0x70]);
const BGRA: [u32; 4] = named(0x6FDD_C324, 0x4E03, 0x4BFE, [0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9, 0x0F]);
/// `ID3D10Multithread`.
const A_MULTITHREAD: [u32; 4] = named(0x9B7E_4E00, 0x342C, 0x4106, [0xA1, 0x9F, 0x4F, 0x27, 0x04, 0xF6, 0x89, 0xF0]);
/// A stream's `MF_MT_MAJOR_TYPE`, `MF_SD_LANGUAGE` and `MF_SD_STREAM_NAME`; `MFMediaType_Audio`.
const MAJOR_TYPE: [u32; 4] = named(0x48EB_A18E, 0xF8C9, 0x4687, [0xBF, 0x11, 0x0A, 0x74, 0xC9, 0xF9, 0x6A, 0x8F]);
const LANGUAGE: [u32; 4] = named(0x00AF_2180, 0xBDC2, 0x423C, [0xAB, 0xCA, 0xF5, 0x03, 0x59, 0x3B, 0xC1, 0x21]);
const STREAM_NAME: [u32; 4] = named(0x4F1B_099D, 0xD314, 0x41E5, [0xA7, 0x81, 0x7F, 0xEF, 0xAA, 0x4C, 0x50, 0x1F]);
const AUDIO: [u32; 4] = named(0x7364_7561, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
/// `IMFGetService`, the engine's `MF_MEDIA_ENGINE_TIMEDTEXT` service and `IMFTimedText`.
const A_SERVICE_GETTER: [u32; 4] = named(0xFA99_3888, 0x4383, 0x415A, [0xA9, 0x30, 0xDD, 0x47, 0x2A, 0x8C, 0xF6, 0xF7]);
const TIMED_TEXT: [u32; 4] = named(0x805E_A411, 0x92E0, 0x4E59, [0x9B, 0x6E, 0x5C, 0x7D, 0x79, 0x15, 0xE6, 0x4F]);
const A_TIMED_TEXT: [u32; 4] = named(0x1F2A_94C9, 0xA3DF, 0x430D, [0x9D, 0x0F, 0xAC, 0xD8, 0x5D, 0xDC, 0x29, 0xAF]);
/// A media type's `MF_MT_SUBTYPE`; `MFVideoFormat_H264` and `MFVideoFormat_HEVC`.
const SUBTYPE: [u32; 4] = named(0xF7E3_4C9A, 0x42E8, 0x4714, [0xB7, 0x4B, 0xCB, 0x29, 0xD7, 0x2C, 0x35, 0xE5]);
const H264: [u32; 4] = named(0x3436_3248, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
const HEVC: [u32; 4] = named(0x4356_4548, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
/// `MFSubtitleFormat_ATSC`: the data format of the timed text's own track of the captions in
/// the video stream.
const ATSC: [u32; 4] = named(0x7FA7_FAA3, 0xFEAE, 0x4E16, [0xAE, 0xDF, 0x36, 0xB9, 0xAC, 0xFB, 0xB0, 0x99]);
/// A sample's `MFSampleExtension_DecodeTimestamp`.
const DECODE_TIME: [u32; 4] = named(0x73A9_54D4, 0x09E2, 0x4861, [0xBE, 0xFC, 0x94, 0xBD, 0x97, 0xC0, 0x8E, 0x6E]);
/// A source reader's first video stream, and all its streams.
const FIRST_VIDEO: u32 = 0xFFFF_FFFC;
const ALL_STREAMS: u32 = 0xFFFF_FFFE;

/// The functions of Media Foundation and its helpers that are not methods.
pub(crate) struct Functions {
    startup: unsafe extern "system" fn(u32, u32) -> i32,
    attributes: unsafe extern "system" fn(*mut *mut Com, u32) -> i32,
    on_stream: unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32,
    device_manager: unsafe extern "system" fn(*mut u32, *mut *mut Com) -> i32,
    memory_stream: unsafe extern "system" fn(*const u8, u32) -> *mut Com,
    text: unsafe extern "system" fn(*const u16) -> *mut u16,
    free_text: unsafe extern "system" fn(*mut u16),
    /// `MFCreateSourceReaderFromURL` and `MFCreateSourceReaderFromByteStream`: the captions in a
    /// video stream are read through a reader of their own.
    reader_from_url: unsafe extern "system" fn(*const u16, *mut Com, *mut *mut Com) -> i32,
    reader_from_stream: unsafe extern "system" fn(*mut Com, *mut Com, *mut *mut Com) -> i32,
    /// `MFCreateSourceResolver`, `MFCreateAsyncResult` and `MFInvokeCallback`: an address is
    /// downloaded once for all its readers (`video_windows_download`).
    pub(crate) source_resolver: unsafe extern "system" fn(*mut *mut Com) -> i32,
    pub(crate) async_result: unsafe extern "system" fn(*mut Com, *mut Com, *mut Com, *mut *mut Com) -> i32,
    pub(crate) invoke: unsafe extern "system" fn(*mut Com) -> i32,
    /// `D3D11CreateDevice`; none without Direct3D 11.
    make_device: Option<MakeDevice>,
}

type MakeDevice = unsafe extern "system" fn(*mut c_void, u32, isize, u32, *const u32, u32, u32, *mut *mut Com, *mut u32, *mut *mut Com) -> i32;

/// The functions, loaded once from the system folder; none where Media Foundation is missing
/// (Windows N editions without the Media Feature Pack).
fn functions() -> Option<&'static Functions> {
    static LOADED: OnceLock<Option<Functions>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref()
}

/// Starts Media Foundation once, for the program's life: its `HRESULT`. (Started and shut down
/// per video, opening took twice as long: 40 opens 17 s, now 8.5 s.)
fn media_foundation_started(functions: &Functions) -> i32 {
    static STARTED: OnceLock<i32> = OnceLock::new();
    // SAFETY: `MFStartup` with the version of the SDK used (MF_VERSION), full start.
    *STARTED.get_or_init(|| unsafe { (functions.startup)(0x0002_0070, 0) })
}

/// The functions, with Media Foundation started: none where it is missing or does not start.
#[cfg(test)]
pub(crate) fn started_functions() -> Option<&'static Functions> {
    functions().filter(|functions| media_foundation_started(functions) >= 0)
}

fn load() -> Option<Functions> {
    let (mf, ole, shell, d3d11) = (system_library("mfplat.dll"), system_library("oleaut32.dll"), system_library("shlwapi.dll"), system_library("d3d11.dll"));
    let reader = system_library("mfreadwrite.dll");
    let found = |library: isize, name: &CStr| {
        let at = function_in(library, name);
        (library != 0 && at != 0).then_some(at)
    };
    // SAFETY: each is the function of that name, of the type its header gives.
    unsafe {
        Some(Functions {
            startup: function(found(mf, c"MFStartup")?),
            attributes: function(found(mf, c"MFCreateAttributes")?),
            on_stream: function(found(mf, c"MFCreateMFByteStreamOnStream")?),
            device_manager: function(found(mf, c"MFCreateDXGIDeviceManager")?),
            memory_stream: function(found(shell, c"SHCreateMemStream")?),
            text: function(found(ole, c"SysAllocString")?),
            free_text: function(found(ole, c"SysFreeString")?),
            reader_from_url: function(found(reader, c"MFCreateSourceReaderFromURL")?),
            reader_from_stream: function(found(reader, c"MFCreateSourceReaderFromByteStream")?),
            source_resolver: function(found(mf, c"MFCreateSourceResolver")?),
            async_result: function(found(mf, c"MFCreateAsyncResult")?),
            invoke: function(found(mf, c"MFInvokeCallback")?),
            make_device: found(d3d11, c"D3D11CreateDevice").map(|at| function(at)),
        })
    }
}

/// A function at this address, as type `F`.
///
/// # Safety
/// The address is such a function.
unsafe fn function<F: Copy>(at: usize) -> F {
    // SAFETY: as the caller promises; a function pointer is one pointer, as `F` is.
    unsafe { std::mem::transmute_copy(&at) }
}

/// Calls method `at` of a live COM object with these arguments, as type `F`.
macro_rules! call {
    ($object:expr, $at:expr, $kind:ty $(, $argument:expr)*) => {
        Com::function::<$kind>($object, $at)($object $(, $argument)*)
    };
}
pub(crate) use call;

// ---- the GPU the engines decode on ----

/// The Direct3D 11 device videos' engines decode with, its immediate context, and the DXGI
/// device manager that hands it to Media Foundation. One for the program, made again once lost.
struct Gpu {
    device: *mut Com,
    context: *mut Com,
    manager: *mut Com,
    /// The context as `ID3D11DeviceContext4`, which signals fences; null before Windows 10 1703.
    #[cfg(feature = "gpu-draw")]
    signaller: *mut Com,
}

// SAFETY: the device is multithread protected (`ID3D10Multithread`): Direct3D takes a lock
// around each call on it and its context, from any thread.
unsafe impl Send for Gpu {}
// SAFETY: as above.
unsafe impl Sync for Gpu {}

impl Gpu {
    /// Whether the device is gone: removed, reset or hung (`GetDeviceRemovedReason`, 39).
    fn lost(&self) -> bool {
        // SAFETY: a plain query of a live device.
        unsafe { call!(self.device, 39, unsafe extern "system" fn(*mut Com) -> i32) != 0 }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: each is held once, by this.
        unsafe {
            #[cfg(feature = "gpu-draw")]
            release_all(&[self.signaller]);
            release_all(&[self.manager, self.context, self.device]);
        }
    }
}

/// The device, made at first use and again after a loss (the GPU reset, its driver updated):
/// one lost would fail every video after. None where the machine has no GPU (or no
/// Direct3D 11), which is not asked again.
fn gpu(functions: &Functions) -> Option<Arc<Gpu>> {
    static MADE: Mutex<Option<Option<Arc<Gpu>>>> = Mutex::new(None);
    let mut made = MADE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if made.as_ref().is_none_or(|known| known.as_ref().is_some_and(|gpu| gpu.lost())) {
        *made = Some(make_gpu(functions).map(Arc::new));
    }
    made.clone().flatten()
}

fn make_gpu(functions: &Functions) -> Option<Gpu> {
    let make = functions.make_device?;
    let (mut device, mut context, mut manager, mut multithread, mut token) = (null_mut(), null_mut(), null_mut(), null_mut::<Com>(), 0u32);
    // SAFETY: a hardware device with video and BGRA support (1: D3D_DRIVER_TYPE_HARDWARE;
    // 0x800 | 0x20; 7: the SDK version); it is made multithread protected
    // (`ID3D10Multithread::SetMultithreadProtected`, 5) and handed to a new device manager
    // (`IMFDXGIDeviceManager::ResetDevice`, 7). What fails is released.
    unsafe {
        if make(null_mut(), 1, 0, 0x820, null(), 0, 7, &mut device, null_mut(), &mut context) < 0 || device.is_null() {
            return None;
        }
        call!(device, 0, Ask, &A_MULTITHREAD, &mut multithread);
        let made = !multithread.is_null() && (functions.device_manager)(&mut token, &mut manager) >= 0;
        if made {
            call!(multithread, 5, unsafe extern "system" fn(*mut Com, i32) -> i32, 1);
        }
        let reset = made && call!(manager, 7, unsafe extern "system" fn(*mut Com, *mut Com, u32) -> i32, device, token) >= 0;
        release_all(&[multithread]);
        if !reset {
            release_all(&[manager, context, device]);
            return None;
        }
    }
    #[cfg(not(feature = "gpu-draw"))]
    return Some(Gpu { device, context, manager });
    #[cfg(feature = "gpu-draw")]
    {
        let mut signaller = null_mut();
        // SAFETY: the context is alive; what it gives is held by the `Gpu`.
        unsafe { call!(context, 0, Ask, &A_SIGNALLING_CONTEXT, &mut signaller) };
        Some(Gpu { device, context, manager, signaller })
    }
}

/// `QueryInterface`.
type Ask = unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut *mut Com) -> i32;

// ---- what the engine tells, from its own threads ----

/// A COM object of ours that the engine tells things to: its vtable, the one interface it has
/// (besides `IUnknown`), its references, and what it was told.
#[repr(C)]
pub(crate) struct Listener<S: 'static> {
    table: &'static ListenerTable<S>,
    interface: &'static [u32; 4],
    references: AtomicU32,
    pub(crate) told: S,
}

/// `IUnknown`'s three methods, then the interface's own, of any shape (each table below is
/// one of these with them after).
#[repr(C)]
pub(crate) struct ListenerTable<S: 'static> {
    pub(crate) query: unsafe extern "system" fn(*mut Listener<S>, *const [u32; 4], *mut *mut c_void) -> i32,
    pub(crate) add: unsafe extern "system" fn(*mut Listener<S>) -> u32,
    pub(crate) release: unsafe extern "system" fn(*mut Listener<S>) -> u32,
}

impl<S> Listener<S> {
    /// A new one, held once by the caller.
    pub(crate) fn boxed(table: &'static ListenerTable<S>, interface: &'static [u32; 4], told: S) -> *mut Listener<S> {
        Box::into_raw(Box::new(Listener { table, interface, references: AtomicU32::new(1), told }))
    }
}

pub(crate) unsafe extern "system" fn query<S>(this: *mut Listener<S>, of: *const [u32; 4], out: *mut *mut c_void) -> i32 {
    // SAFETY: COM passes a live object, a valid interface id and a place for the answer.
    unsafe {
        let ours = *of == AN_UNKNOWN || *of == *(*this).interface;
        *out = if ours { this.cast() } else { null_mut() };
        if !ours {
            return 0x8000_4002_u32 as i32;
        }
        add(this);
    }
    0
}

pub(crate) unsafe extern "system" fn add<S>(this: *mut Listener<S>) -> u32 {
    // SAFETY: the caller holds a reference, so it is alive.
    unsafe { (*this).references.fetch_add(1, Ordering::AcqRel) + 1 }
}

pub(crate) unsafe extern "system" fn release<S>(this: *mut Listener<S>) -> u32 {
    // SAFETY: the caller holds a reference; the last one frees it, as it was boxed.
    unsafe {
        let left = (*this).references.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(this));
        }
        left
    }
}

/// What the engine said (`IMFMediaEngineNotify`), as bits, and the `HRESULT` of its last error.
struct Said {
    bits: AtomicU32,
    code: AtomicI32,
}

const LOADED: u32 = 1;
const ENDED: u32 = 2;
const FAILED: u32 = 4;

#[repr(C)]
struct SaidTable {
    unknown: ListenerTable<Said>,
    event: unsafe extern "system" fn(*mut Listener<Said>, u32, usize, u32) -> i32,
}

static SAID: SaidTable = SaidTable { unknown: ListenerTable { query, add, release }, event };

unsafe extern "system" fn event(this: *mut Listener<Said>, event: u32, _: usize, code: u32) -> i32 {
    // (10: metadata loaded, 19: ended, 5: an error, with its HRESULT second.)
    let bit = match event {
        10 => LOADED,
        19 => ENDED,
        5 => FAILED,
        _ => return 0,
    };
    // SAFETY: the engine holds a reference while it calls.
    let said = unsafe { &(*this).told };
    if bit == FAILED {
        said.code.store(code as i32, Ordering::Release);
    }
    said.bits.fetch_or(bit, Ordering::AcqRel);
    0
}

/// What the timed text told (`IMFTimedTextNotify`): the cues it shows now, each with its id
/// and track's; whether its tracks changed since asked (captions in the video stream, CEA-608,
/// are a track added once their first data plays); and how many it has, as its notices count
/// them (a track may be told before its list holds it).
struct Showing {
    cues: Mutex<Vec<(u32, u32, Cue)>>,
    tracks_changed: AtomicBool,
    tracks: AtomicU32,
}

impl Showing {
    fn cues(&self) -> std::sync::MutexGuard<'_, Vec<(u32, u32, Cue)>> {
        self.cues.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[repr(C)]
struct ShowingTable {
    unknown: ListenerTable<Showing>,
    track_added: unsafe extern "system" fn(*mut Listener<Showing>, u32),
    track_removed: unsafe extern "system" fn(*mut Listener<Showing>, u32),
    track_selected: unsafe extern "system" fn(*mut Listener<Showing>, u32, i32),
    track_ready: unsafe extern "system" fn(*mut Listener<Showing>, u32),
    error: unsafe extern "system" fn(*mut Listener<Showing>, u32, i32, u32),
    cue: unsafe extern "system" fn(*mut Listener<Showing>, u32, f64, *mut Com),
    reset: unsafe extern "system" fn(*mut Listener<Showing>),
}

static SHOWING: ShowingTable =
    ShowingTable { unknown: ListenerTable { query, add, release }, track_added, track_removed, track_selected: ignore_selected, track_ready: ignore_track, error: ignore_error, cue, reset };

unsafe extern "system" fn ignore_track(_: *mut Listener<Showing>, _: u32) {}
unsafe extern "system" fn ignore_selected(_: *mut Listener<Showing>, _: u32, _: i32) {}
unsafe extern "system" fn ignore_error(_: *mut Listener<Showing>, _: u32, _: i32, _: u32) {}

unsafe extern "system" fn track_added(this: *mut Listener<Showing>, _: u32) {
    // SAFETY: the timed text holds a reference while it calls.
    let told = unsafe { &(*this).told };
    told.tracks.fetch_add(1, Ordering::AcqRel);
    told.tracks_changed.store(true, Ordering::Release);
    crate::video::wake_window();
}

unsafe extern "system" fn track_removed(this: *mut Listener<Showing>, _: u32) {
    // SAFETY: the timed text holds a reference while it calls.
    let told = unsafe { &(*this).told };
    told.tracks.fetch_sub(1, Ordering::AcqRel);
    told.tracks_changed.store(true, Ordering::Release);
    crate::video::wake_window();
}

/// A cue came on show (0), went (1), or all went (2). (Of the cue: 3: GetId, 8: GetTrackId.)
unsafe extern "system" fn cue(this: *mut Listener<Showing>, happened: u32, _: f64, cue: *mut Com) {
    // SAFETY: the timed text holds a reference while it calls, and lends a live cue (or
    // none, to clear).
    let (showing, id, track) = unsafe {
        let ids = (!cue.is_null()).then(|| (call!(cue, 3, unsafe extern "system" fn(*mut Com) -> u32), call!(cue, 8, unsafe extern "system" fn(*mut Com) -> u32)));
        (&(*this).told, ids.map(|ids| ids.0), ids.map(|ids| ids.1))
    };
    let mut shown = showing.cues();
    match (happened, id, track) {
        // SAFETY: as above.
        (0, Some(id), Some(track)) => shown.push((id, track, unsafe { cue_of(cue) })),
        (1, Some(id), _) => shown.retain(|(of, _, _)| *of != id),
        (2, ..) => shown.clear(),
        _ => {}
    }
}

unsafe extern "system" fn reset(this: *mut Listener<Showing>) {
    // SAFETY: the timed text holds a reference while it calls.
    unsafe { (*this).told.cues().clear() };
}

// ---- the player ----

/// A Media Engine playing one source, what its frames are copied into, and what it found in
/// the source.
pub(crate) struct Player {
    functions: &'static Functions,
    engine: *mut Com,
    /// Its extended interface (`IMFMediaEngineEx`).
    ex: *mut Com,
    /// What the engine tells, and the cues its timed text shows.
    said: *mut Listener<Said>,
    showing: *mut Listener<Showing>,
    frames: Frames,
    /// The last frame's time, in 100 ns.
    last: Option<i64>,
    /// A frame was put in the target since the source was given.
    framed: bool,
    /// The frame of the last seek: waited for, or fetched again.
    sought: Sought,
    /// The source's metadata was read: its tracks are known.
    loaded: bool,
    /// The engine's streams that are sound tracks, in order.
    sound_streams: Vec<u32>,
    /// The engine's timed text (`IMFTimedText`), how many tracks its list held when last read,
    /// its caption tracks' ids, and the one chosen.
    timed_text: *mut Com,
    timed_tracks: u32,
    caption_ids: Vec<u32>,
    captions: Option<usize>,
    /// The caption tracks of an MP4 or QuickTime file, read by taut (listed after the timed
    /// text's tracks), and how many of them were listed last.
    file_captions: Arc<ReadAhead<FileCaptions>>,
    file_listed: usize,
    /// The captions in the video stream, read by a thread of their own (listed after the
    /// file's), unless the source is one taut does not read (the engine's own `CC1` is then
    /// listed).
    in_stream: Arc<ReadAhead<SeiReading>>,
    reads_in_stream: bool,
    /// An address being downloaded, until its first bytes come and the engine is given it.
    pending: Option<Pending>,
    /// (Tests: the effect that hears what plays, or null.)
    #[cfg(test)]
    hearing: *mut Listener<Mutex<hearing::Hearing>>,
    /// (Tests: the next seek's frame is taken as lost.)
    #[cfg(test)]
    lose: bool,
}

/// The frame of the last seek, while it is due.
#[derive(Clone, Copy)]
enum Sought {
    /// None is due: it came.
    Done,
    /// The frame at `at` s is due, fetched again `tries` times.
    Due { at: f64, tries: u8 },
    /// Seeking elsewhere, to seek back to `at` s and fetch its frame again.
    Away { at: f64, tries: u8 },
}

/// How many times a seek's lost frame is fetched again, at most.
const FETCHES: u8 = 2;
/// The time `OnVideoStreamTick` gives while the engine holds no frame.
const NO_FRAME: i64 = i64::MIN;
/// (Tests: how many lost frames were fetched again.)
#[cfg(test)]
pub(crate) static FETCHED: AtomicU32 = AtomicU32::new(0);

impl Player {
    /// Opens the source in a new engine; with no sound, muted. Decoded on the GPU where there is
    /// one.
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let mut player = Player::new(sound, true)?;
        player.set_source(source)?;
        Ok(player)
    }

    /// A new engine with no source yet: decoding on the GPU where `on_gpu` and there is one.
    fn new(sound: bool, on_gpu: bool) -> Result<Player, VideoError> {
        let mut player = Player::started()?;
        let gpu = if on_gpu { gpu(player.functions) } else { None };
        player.engine = player.make_engine(sound, gpu.as_deref())?;
        player.frames = Frames::new(gpu)?;
        // SAFETY: the engine is alive (0: QueryInterface); the extended interface is released
        // with it.
        unsafe { call!(player.engine, 0, Ask, &AN_ENGINE_EX, &mut player.ex) };
        if player.ex.is_null() {
            return Err(VideoError::NoPlayer("no IMFMediaEngineEx".into()));
        }
        player.timed_text = player.make_timed_text();
        Ok(player)
    }

    /// Media Foundation started, and what the engine will tell: a player with no engine yet.
    fn started() -> Result<Player, VideoError> {
        let functions = functions().ok_or_else(|| VideoError::NoPlayer("Media Foundation is not installed".into()))?;
        // SAFETY: COM is started for this thread (single-threaded, as OLE wants it; a thread
        // already started either way keeps its kind).
        unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 2) };
        let started = media_foundation_started(functions);
        if started < 0 {
            return Err(VideoError::NoPlayer(format!("Media Foundation did not start ({:#010x})", started as u32)));
        }
        Ok(Player {
            functions,
            engine: null_mut(),
            ex: null_mut(),
            said: Listener::boxed(&SAID.unknown, &A_NOTIFY, Said { bits: AtomicU32::new(0), code: AtomicI32::new(0) }),
            showing: Listener::boxed(&SHOWING.unknown, &A_CUE_NOTIFY, Showing { cues: Mutex::new(Vec::new()), tracks_changed: AtomicBool::new(false), tracks: AtomicU32::new(0) }),
            frames: Frames::Bitmap { wic: null_mut(), bitmap: null_mut(), size: (0, 0) },
            last: None,
            framed: false,
            sought: Sought::Done,
            loaded: false,
            sound_streams: Vec::new(),
            timed_text: null_mut(),
            timed_tracks: 0,
            caption_ids: Vec::new(),
            captions: None,
            file_captions: Arc::new(ReadAhead::new()),
            file_listed: 0,
            in_stream: Arc::new(ReadAhead::new()),
            reads_in_stream: true,
            pending: None,
            #[cfg(test)]
            hearing: null_mut(),
            #[cfg(test)]
            lose: false,
        })
    }

    /// A new engine telling `said`, giving frames as 32-bit BGRA, decoding on `gpu` if any.
    fn make_engine(&mut self, sound: bool, gpu: Option<&Gpu>) -> Result<*mut Com, VideoError> {
        let (mut attributes, mut factory, mut engine) = (null_mut(), null_mut(), null_mut());
        // SAFETY: attributes are made and set (27: SetUnknown, 21: SetUINT32), the factory is
        // made and asked for an engine (3: CreateInstance; 4 forces mute), and both released.
        unsafe {
            if (self.functions.attributes)(&mut attributes, 3) < 0 || attributes.is_null() {
                return Err(VideoError::NoPlayer("no attributes".into()));
            }
            call!(attributes, 27, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut c_void) -> i32, &CALLBACK, self.said.cast());
            call!(attributes, 21, unsafe extern "system" fn(*mut Com, *const [u32; 4], u32) -> i32, &OUTPUT_FORMAT, B8G8R8A8_UNORM);
            if let Some(gpu) = gpu {
                call!(attributes, 27, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut c_void) -> i32, &DXGI_MANAGER, gpu.manager.cast());
            }
            let made = crate::windows::sys::CoCreateInstance(&ENGINE_FACTORY, null_mut(), 1, &AN_ENGINE_FACTORY, &mut factory);
            if made >= 0 && !factory.is_null() {
                call!(factory, 3, unsafe extern "system" fn(*mut Com, u32, *mut Com, *mut *mut Com) -> i32, if sound { 0 } else { 4 }, attributes, &mut engine);
                Com::release(factory);
            }
            Com::release(attributes);
        }
        match engine.is_null() {
            true => Err(VideoError::NoPlayer("no Media Engine".into())),
            false => Ok(engine),
        }
    }

    /// The engine's timed text, with the file's own text tracks read (`IMFGetService`,
    /// 3: GetService; of the timed text, 15: SetInBandEnabled, 3: RegisterNotifications);
    /// none where it has none.
    fn make_timed_text(&self) -> *mut Com {
        let (mut getter, mut timed_text) = (null_mut(), null_mut());
        // SAFETY: the engine is alive; what it gives is ours to release.
        unsafe {
            call!(self.engine, 0, Ask, &A_SERVICE_GETTER, &mut getter);
            if getter.is_null() {
                return null_mut();
            }
            call!(getter, 3, unsafe extern "system" fn(*mut Com, *const [u32; 4], *const [u32; 4], *mut *mut Com) -> i32, &TIMED_TEXT, &A_TIMED_TEXT, &mut timed_text);
            Com::release(getter);
            if !timed_text.is_null() {
                call!(timed_text, 15, unsafe extern "system" fn(*mut Com, i32) -> i32, 1);
                call!(timed_text, 3, unsafe extern "system" fn(*mut Com, *mut c_void) -> i32, self.showing.cast());
            }
        }
        timed_text
    }

    /// Gives the engine its source and starts taut's caption readers on it. An `http` or
    /// `https` address is downloaded once for the engine and the readers (`download`).
    fn set_source(&mut self, source: VideoSource) -> Result<(), VideoError> {
        match source {
            VideoSource::Url(url) if url.starts_with("http://") || url.starts_with("https://") => {
                self.download(url);
                Ok(())
            }
            VideoSource::Url(url) => self.set_address(&url),
            VideoSource::Bytes(bytes) => self.set_bytes(&bytes),
            VideoSource::File(path) => {
                let path = std::path::absolute(&path).unwrap_or(path);
                self.set_address(&path.display().to_string())?;
                self.read_file_captions(move || std::fs::File::open(&path).ok());
                Ok(())
            }
        }
    }

    /// Gives the engine a path or address; the captions in its video stream are read through a
    /// reader of their own.
    fn set_address(&mut self, address: &str) -> Result<(), VideoError> {
        let address = wide(address);
        self.give_address(&address)?;
        self.read_in_stream(ReaderSource::Address(address));
        Ok(())
    }

    /// Gives the engine a path or address ending in a zero (6: SetSource).
    fn give_address(&mut self, address: &[u16]) -> Result<(), VideoError> {
        // SAFETY: the text is made and freed around the call.
        refused(unsafe {
            let text = (self.functions.text)(address.as_ptr());
            let done = call!(self.engine, 6, unsafe extern "system" fn(*mut Com, *mut u16) -> i32, text);
            (self.functions.free_text)(text);
            done
        })
    }

    /// Gives the engine a byte stream, named `name` for its kind (45:
    /// `IMFMediaEngineEx::SetSourceFromByteStream`; the engine takes its own reference).
    fn give_byte_stream(&mut self, stream: *mut Com, name: &str) -> Result<(), VideoError> {
        let name = wide(name);
        // SAFETY: a live byte stream; the text is made and freed around the call.
        refused(unsafe {
            let text = (self.functions.text)(name.as_ptr());
            let done = call!(self.ex, 45, unsafe extern "system" fn(*mut Com, *mut Com, *mut u16) -> i32, stream, text);
            (self.functions.free_text)(text);
            done
        })
    }

    /// Plays bytes: the engine and each caption reader read a clone of one memory stream over
    /// a copy of them (13: Clone, sharing the bytes, reading on its own).
    fn set_bytes(&mut self, bytes: &[u8]) -> Result<(), VideoError> {
        let mut stream = null_mut();
        // SAFETY: the memory stream and the byte stream over it are ours, released once the
        // engine and the readers took theirs.
        unsafe {
            let memory = (self.functions.memory_stream)(bytes.as_ptr(), bytes.len() as u32);
            if memory.is_null() {
                return Err(VideoError::Refused("no memory for the bytes".into()));
            }
            let made = (self.functions.on_stream)(memory, &mut stream);
            let given = match made < 0 || stream.is_null() {
                true => refused(made.min(-1)),
                false => self.give_byte_stream(stream, &format!("memory.{}", crate::video::kind_of(bytes))),
            };
            if given.is_ok() {
                if let Some(copy) = cloned(memory) {
                    self.read_in_stream(ReaderSource::Stream(copy));
                }
                if let Some(copy) = cloned(memory).map(MemoryStream) {
                    self.read_file_captions(move || Some(copy));
                }
            }
            release_all(&[stream, memory]);
            given
        }
    }

    /// Starts downloading an address: its first bytes are read on a thread of their own (an
    /// address may take a while), and the engine is given the download once they came
    /// (`take_download`).
    fn download(&mut self, address: String) {
        let download = Download::new(self.functions, &address);
        let (sent, first) = mpsc::channel();
        let reading = Arc::clone(&download);
        std::thread::spawn(move || {
            // SAFETY: COM for this thread, multithreaded (0), ended with it.
            let com = unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 0) } >= 0;
            let mut head = [0u8; 16];
            let read = reading.read(0, &mut head);
            let _ = sent.send(head[..read].to_vec());
            if com {
                // SAFETY: started above.
                unsafe { crate::windows::sys::CoUninitialize() };
            }
        });
        self.pending = Some(Pending { address, download, first });
    }

    /// Once an address's first bytes came: the engine is given a byte stream of the download,
    /// and so are taut's caption readers; an error where nothing came. A playlist (HLS, DASH)
    /// the engine reads itself: its captions in the stream are then the engine's own `CC1`.
    fn take_download(&mut self) -> Option<String> {
        let first = self.pending.as_ref()?.first.try_recv().ok()?;
        let Pending { address, download, .. } = self.pending.take()?;
        if first.is_empty() {
            return Some(format!("{address} can't be read"));
        }
        if first.starts_with(b"#EXTM3U") || first.starts_with(b"<") {
            self.reads_in_stream = false;
            return self.give_address(&wide(&address)).err().map(|error| error.to_string());
        }
        let stream = byte_stream(&download);
        let given = self.give_byte_stream(stream, &format!("download.{}", kind_of_address(&address, &first)));
        // SAFETY: held once; the engine holds its own.
        unsafe { Com::release(stream) };
        if let Err(error) = given {
            return Some(error.to_string());
        }
        self.read_in_stream(ReaderSource::ByteStream(byte_stream(&download)));
        self.read_file_captions(move || Some(download));
        None
    }

    /// Starts the thread that reads the captions in the source's video stream.
    fn read_in_stream(&self, source: ReaderSource) {
        let functions = self.functions;
        self.in_stream.spawn(move |in_stream| read_captions_in_stream(functions, source, in_stream));
    }

    /// Starts the thread that reads the caption tracks of an MP4 or QuickTime file from what
    /// `open` gives (on that thread).
    fn read_file_captions<S: Source + 'static>(&self, open: impl FnOnce() -> Option<S> + Send + 'static) {
        self.file_captions.spawn(move |file_captions| {
            // SAFETY: COM for this thread, multithreaded (0), ended with it.
            let com = unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 0) } >= 0;
            if let Some(source) = open() {
                crate::video_mp4::read_paced(source, file_captions);
            }
            if com {
                // SAFETY: started above.
                unsafe { crate::windows::sys::CoUninitialize() };
            }
        });
    }

    /// Calls a method that takes nothing more and returns `R`.
    fn call<R>(&self, at: usize) -> R {
        // SAFETY: the engine is alive, and its method at that index takes only itself.
        unsafe { call!(self.engine, at, unsafe extern "system" fn(*mut Com) -> R) }
    }
    /// Calls a method that takes one number.
    fn set<T>(&self, at: usize, value: T) {
        // SAFETY: the engine is alive, and its method at that index takes one such number.
        unsafe { call!(self.engine, at, unsafe extern "system" fn(*mut Com, T) -> i32, value) };
    }

    pub(crate) fn play(&mut self) {
        self.call::<i32>(32);
    }
    pub(crate) fn pause(&mut self) {
        self.call::<i32>(33);
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        (self.last, self.sought) = (None, Sought::Due { at: to.as_secs_f64(), tries: 0 });
        self.set(17, to.as_secs_f64());
        self.in_stream.seek(to);
        self.file_captions.seek(to);
    }
    /// Where it plays (the captions' readers read on from there): where it was sought while
    /// its frame is fetched again.
    pub(crate) fn position(&mut self) -> Duration {
        let seconds = match self.sought {
            Sought::Away { at, .. } => at,
            _ => self.call::<f64>(16),
        };
        let position = Duration::try_from_secs_f64(seconds).unwrap_or_default();
        self.in_stream.played_to(position);
        self.file_captions.played_to(position);
        position
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        // (Not a number until known; infinite for a live stream.)
        Duration::try_from_secs_f64(self.call::<f64>(19)).ok()
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        self.set(37, volume as f64);
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        self.set(35, muted as i32);
    }
    /// Plays at this speed (22: SetDefaultPlaybackRate, which a new play keeps; 24:
    /// SetPlaybackRate).
    pub(crate) fn set_speed(&mut self, speed: f32) {
        self.set(22, speed as f64);
        self.set(24, speed as f64);
    }

    /// What the engine and the caption readers said since last asked (an address's download
    /// given to the engine once its first bytes came).
    pub(crate) fn news(&mut self) -> News {
        let unread = self.take_download();
        // SAFETY: ours is one of its references.
        let told = unsafe { &(*self.said).told };
        let (said, code) = (told.bits.swap(0, Ordering::AcqRel), told.code.load(Ordering::Acquire));
        self.loaded |= said & LOADED != 0;
        let error = (said & FAILED != 0).then(|| format!("the player stopped: {} ({:#010x})", self.error_words(), code as u32));
        // SAFETY: as above.
        let showing = unsafe { &(*self.showing).told };
        // (A track told before the list held it is listed again until the list does, asked for
        // meanwhile though paused.)
        let unlisted = showing.tracks.load(Ordering::Acquire) > self.timed_tracks;
        let timed_text = showing.tracks_changed.swap(false, Ordering::AcqRel) || unlisted;
        let tracks_changed = timed_text || self.in_stream.with_tracks(|tracks| tracks.unlisted()) || self.file_captions.with(|captions| captions.unlisted());
        let reading_captions = unlisted || self.in_stream.reading() || self.file_captions.reading();
        News { ended: said & ENDED != 0, error: unread.or(error), clears_on_play: false, tracks_changed, reading_captions }
    }
    /// The engine's error as words (3: GetError; of the error, 3: GetErrorCode).
    fn error_words(&self) -> &'static str {
        let mut error = null_mut();
        // SAFETY: the engine is alive; its error is read and released.
        let code = unsafe {
            call!(self.engine, 3, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut error);
            if error.is_null() {
                return "an error";
            }
            let code = call!(error, 3, unsafe extern "system" fn(*mut Com) -> u16);
            Com::release(error);
            code
        };
        ["an error", "aborted", "a network error", "a decoding error", "a format it can't play", "encrypted"][(code as usize).min(5)]
    }

    /// The newest frame where there is one not given yet: its width and time, its pixels in
    /// `into`.
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let (at, width) = self.transferred()?;
        self.frames.read(into).then_some((width, at))
    }

    /// Whether frames are decoded on the GPU, and so given as textures (`frame_texture`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn decodes_on_gpu(&self) -> bool {
        matches!(self.frames, Frames::Gpu { .. })
    }
    /// The newest frame where there is one not given yet, left on the GPU: its texture and
    /// time.
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn frame_texture(&mut self) -> Option<(Arc<FrameTexture>, Duration)> {
        let (at, _) = self.transferred()?;
        match &self.frames {
            Frames::Gpu { texture: Some(texture), .. } => Some((texture.clone(), at)),
            _ => None,
        }
    }

    /// Has the engine put its newest frame into the target where there is one not given yet:
    /// its time and width. (44: OnVideoStreamTick, 40: GetNativeVideoSize.)
    fn transferred(&mut self) -> Option<(Duration, usize)> {
        let (mut at, mut width, mut height) = (0i64, 0u32, 0u32);
        // SAFETY: the engine is alive, and writes the time.
        let tick = unsafe { call!(self.engine, 44, unsafe extern "system" fn(*mut Com, *mut i64) -> i32, &mut at) };
        if self.fetching_again(tick, at) || tick != 0 || self.last == Some(at) {
            return None;
        }
        // SAFETY: as above; it writes the size.
        unsafe { call!(self.engine, 40, unsafe extern "system" fn(*mut Com, *mut u32, *mut u32) -> i32, &mut width, &mut height) };
        let whole = [0i32, 0, width as i32, height as i32];
        if width == 0 || height == 0 || !self.frames.sized(width, height) || !self.frames.transfer(self.engine, &whole) {
            return None;
        }
        (self.last, self.framed, self.sought) = (Some(at), true, Sought::Done);
        Some((Duration::from_nanos(at.max(0) as u64 * 100), width as usize))
    }

    /// Fetches a paused seek's frame again where the engine lost it: whether this tick is part
    /// of that, and gives no frame. A seek made as the video pauses may end with no frame, and
    /// none comes after (one in eight on a runner with no GPU), while a seek to where the engine
    /// stands is done at once with none. So it seeks elsewhere, then back; the frame from
    /// elsewhere is never given.
    fn fetching_again(&mut self, tick: i32, at: i64) -> bool {
        if let Sought::Away { at: back, tries } = self.sought {
            if !self.seeking() && (tick == 0 || at == NO_FRAME) {
                (self.last, self.sought) = ((tick == 0).then_some(at), Sought::Due { at: back, tries });
                self.set(17, back);
            }
            return true;
        }
        let Sought::Due { at: time, tries } = self.sought else { return false };
        if tries >= FETCHES || !(self.lost(tick, at) || self.faked_loss()) {
            return false;
        }
        let Some(away) = self.elsewhere(time) else { return false };
        #[cfg(test)]
        FETCHED.fetch_add(1, Ordering::Relaxed);
        self.sought = Sought::Away { at: time, tries: tries + 1 };
        self.set(17, away);
        true
    }
    /// Whether the engine lost a seek's frame: paused, the seek done, a frame shown before, and
    /// it holds none (20: IsPaused; 14: GetReadyState, `HAVE_CURRENT_DATA` at least).
    fn lost(&self, tick: i32, at: i64) -> bool {
        tick != 0 && at == NO_FRAME && self.framed && !self.seeking() && self.call::<i32>(20) != 0 && self.call::<u16>(14) >= 2
    }
    /// Whether the engine still seeks (15: IsSeeking).
    fn seeking(&self) -> bool {
        self.call::<i32>(15) != 0
    }
    /// A time a quarter of the length or more from `time`, never the end: a seek there is one
    /// the engine makes (19: GetDuration). None for a live stream.
    fn elsewhere(&self, time: f64) -> Option<f64> {
        let length = self.call::<f64>(19);
        (length.is_finite() && length > 0.0).then(|| if time > length / 2.0 { time / 2.0 } else { (time + length) / 2.0 })
    }
    /// (Tests: a lost frame faked once.)
    fn faked_loss(&mut self) -> bool {
        #[cfg(test)]
        return std::mem::take(&mut self.lose);
        #[cfg(not(test))]
        false
    }

    // ---- tracks ----

    /// The source's sound and caption tracks, once its metadata was read.
    pub(crate) fn tracks(&mut self) -> Option<Tracks> {
        if !self.loaded {
            return None;
        }
        let mut tracks = Tracks::default();
        let mut count = 0u32;
        // SAFETY: the engine's extended interface is alive (54: GetNumberOfStreams).
        unsafe { call!(self.ex, 54, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut count) };

        self.sound_streams = (0..count).filter(|stream| self.stream_guid(*stream, &MAJOR_TYPE) == Some(AUDIO)).collect();
        for &stream in &self.sound_streams {
            if self.stream_chosen(stream) && tracks.sound_chosen.is_none() {
                tracks.sound_chosen = Some(tracks.sound.len());
            }
            let text = |of| self.stream_text(stream, of).unwrap_or_default();
            tracks.sound.push(Track { language: text(&LANGUAGE), name: text(&STREAM_NAME) });
        }
        (self.caption_ids, tracks.captions) = self.caption_tracks();
        let file = self.file_captions.with(FileCaptions::tracks);
        self.file_listed = file.len();
        tracks.captions.extend(file);
        tracks.captions.extend(self.in_stream.with_tracks(ClosedCaptions::tracks));
        Some(tracks)
    }

    /// Plays the `nth` sound track, and no other (57: SetStreamSelection, 58:
    /// ApplyStreamSelections).
    pub(crate) fn choose_sound(&mut self, nth: usize) {
        for (at, &stream) in self.sound_streams.iter().enumerate() {
            // SAFETY: the engine's extended interface is alive, and the stream one of its.
            unsafe { call!(self.ex, 57, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, stream, (at == nth) as i32) };
        }
        // SAFETY: as above.
        unsafe { call!(self.ex, 58, unsafe extern "system" fn(*mut Com) -> i32) };
    }

    /// A stream attribute's value: a `PROPVARIANT` read by `read` and cleared (55:
    /// GetStreamAttribute).
    fn stream_value<T>(&self, stream: u32, of: &[u32; 4], read: impl FnOnce(&PropVariant) -> Option<T>) -> Option<T> {
        let mut value = PropVariant { kind: 0, reserved: [0; 3], data: null_mut(), more: 0 };
        // SAFETY: the engine's extended interface is alive; the value it writes is cleared.
        unsafe {
            let got = call!(self.ex, 55, unsafe extern "system" fn(*mut Com, u32, *const [u32; 4], *mut PropVariant) -> i32, stream, of, &mut value);
            let read = (got >= 0).then(|| read(&value)).flatten();
            PropVariantClear(&mut value);
            read
        }
    }
    /// A stream's GUID attribute (`VT_CLSID`, 72).
    fn stream_guid(&self, stream: u32, of: &[u32; 4]) -> Option<[u32; 4]> {
        // SAFETY: a `VT_CLSID` value points at its GUID.
        self.stream_value(stream, of, |value| (value.kind == 72 && !value.data.is_null()).then(|| unsafe { *value.data.cast::<[u32; 4]>() }))
    }
    /// A stream's text attribute (`VT_LPWSTR`, 31).
    fn stream_text(&self, stream: u32, of: &[u32; 4]) -> Option<String> {
        // SAFETY: a `VT_LPWSTR` value points at text ending in a zero.
        self.stream_value(stream, of, |value| (value.kind == 31).then(|| unsafe { text_at(value.data.cast()) }))
    }
    /// Whether a stream plays (56: GetStreamSelection).
    fn stream_chosen(&self, stream: u32) -> bool {
        let mut chosen = 0i32;
        // SAFETY: the engine's extended interface is alive.
        unsafe { call!(self.ex, 56, unsafe extern "system" fn(*mut Com, u32, *mut i32) -> i32, stream, &mut chosen) };
        chosen != 0
    }

    /// The timed text's subtitle and caption tracks: their ids and what they are (of the timed
    /// text, 13: GetTextTracks; of the list, 3: GetLength).
    fn caption_tracks(&mut self) -> (Vec<u32>, Vec<Track>) {
        let mut list = null_mut();
        // SAFETY: the timed text is alive; it gives a list of its own.
        let listed = !self.timed_text.is_null() && unsafe { call!(self.timed_text, 13, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut list) } >= 0;
        if !listed || list.is_null() {
            return (Vec::new(), Vec::new());
        }
        // SAFETY: a live list.
        let count = unsafe { call!(list, 3, unsafe extern "system" fn(*mut Com) -> u32) };
        self.timed_tracks = count;
        let found = (0..count).filter_map(|at| caption_track(list, at, self.reads_in_stream)).unzip();
        // SAFETY: held once.
        unsafe { Com::release(list) };
        found
    }

    /// Shows the `nth` caption track (of the timed text, 4: SelectTrack; or of the video
    /// stream, after them), or none.
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        if let Some(id) = self.captions.and_then(|at| self.caption_ids.get(at)) {
            // SAFETY: the timed text is alive, and the id one of its tracks'.
            unsafe { call!(self.timed_text, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, *id, 0) };
        }
        self.captions = nth;
        if let Some(id) = nth.and_then(|at| self.caption_ids.get(at)) {
            // SAFETY: as above.
            unsafe { call!(self.timed_text, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, *id, 1) };
        }
    }

    /// The chosen caption track's captions on show now: its cues the timed text says are on
    /// show, or those read in the file's caption tracks or the video stream.
    pub(crate) fn captions(&mut self) -> Vec<Caption> {
        let Some(nth) = self.captions else { return Vec::new() };
        let Some(&id) = self.caption_ids.get(nth) else {
            let (at, file) = (self.position(), nth - self.caption_ids.len());
            return match file.checked_sub(self.file_listed) {
                None => self.file_captions.with(|captions| captions.captions_at(file, at)),
                Some(in_stream) => self.in_stream.with_tracks(|tracks| tracks.captions_at(in_stream, at)),
            };
        };
        // SAFETY: ours is one of its references.
        let shown = unsafe { (*self.showing).told.cues() };
        let mut cues: Vec<&Cue> = shown.iter().filter(|(_, track, _)| *track == id).map(|(_, _, cue)| cue).collect();
        cues.sort_by_key(|cue| cue.from);
        cues.into_iter().map(|cue| cue.caption.clone()).collect()
    }
}

/// The `at`th track of a timed text's track list, unless it holds metadata (kind 3) or is the
/// engine's own reading of the captions in the video stream (data format
/// `MFSubtitleFormat_ATSC`) where taut reads those itself: its id, and what it is (of the list,
/// 4: GetTrack; of the track, 3: GetId, 4: GetLabel, 6: GetLanguage, 7: GetTrackKind,
/// 13: GetDataFormat). The engine's own is named `CC1`.
fn caption_track(list: *mut Com, at: u32, taut_reads_in_stream: bool) -> Option<(u32, Track)> {
    let (mut track, mut format) = (null_mut(), [0u32; 4]);
    // SAFETY: a live list; the track it gives is read and released.
    unsafe {
        if call!(list, 4, unsafe extern "system" fn(*mut Com, u32, *mut *mut Com) -> i32, at, &mut track) < 0 || track.is_null() {
            return None;
        }
        call!(track, 13, unsafe extern "system" fn(*mut Com, *mut [u32; 4]) -> i32, &mut format);
        let in_stream = format == ATSC;
        let captions = call!(track, 7, unsafe extern "system" fn(*mut Com) -> u32) != 3 && !(in_stream && taut_reads_in_stream);
        let name = |label: String| if in_stream && label.is_empty() { "CC1".to_owned() } else { label };
        let found = captions.then(|| (call!(track, 3, unsafe extern "system" fn(*mut Com) -> u32), Track { language: owned_text(track, 6), name: name(owned_text(track, 4)) }));
        Com::release(track);
        found
    }
}

/// A cue's times, lines, place and italics (6: GetStartTime, 7: GetDuration, 12: GetLineCount,
/// 13: GetLine; of a line, 3: GetText): placed by its region where it has one, else by SRT's
/// `{\anN}` left in its text.
///
/// # Safety
/// `cue` is a live `IMFTimedTextCue`.
unsafe fn cue_of(cue: *mut Com) -> Cue {
    let seconds = |at| {
        // SAFETY: as the caller promises.
        let value = unsafe { call!(cue, at, unsafe extern "system" fn(*mut Com) -> f64) };
        Duration::try_from_secs_f64(value).unwrap_or_default()
    };
    let (from, length) = (seconds(6), seconds(7));
    let mut lines = Vec::new();
    // SAFETY: as the caller promises; each line is released once read.
    unsafe {
        for at in 0..call!(cue, 12, unsafe extern "system" fn(*mut Com) -> u32) {
            let mut line = null_mut();
            call!(cue, 13, unsafe extern "system" fn(*mut Com, u32, *mut *mut Com) -> i32, at, &mut line);
            if !line.is_null() {
                lines.push(owned_text(line, 3));
                Com::release(line);
            }
        }
    }
    let caption = crate::video_captions::caption_of(&lines.join("\n"));
    // SAFETY: as the caller promises.
    let (alignment, italic) = unsafe { cue_style(cue) };
    // SAFETY: as the caller promises.
    let place = unsafe { region_place(cue, alignment) }.or(caption.place);
    Cue { from, to: from + length, caption: Caption { place, italic: caption.italic || italic, ..caption } }
}

/// A cue's object of the method at `at` (10: GetRegion, 11: GetStyle), held: none where it has
/// none.
///
/// # Safety
/// `cue` is a live `IMFTimedTextCue`.
unsafe fn cue_part(cue: *mut Com, at: usize) -> Option<*mut Com> {
    let mut part = null_mut();
    // SAFETY: as the caller promises; the part given is ours.
    let got = unsafe { call!(cue, at, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut part) };
    (got >= 0 && !part.is_null()).then_some(part)
}

/// How a cue's style lines up its text (13: GetTextAlignment: 0 start, 1 end, 2 centre) and
/// whether it is italic (10: GetFontStyle: 1 oblique, 2 italic).
///
/// # Safety
/// `cue` is a live `IMFTimedTextCue`.
unsafe fn cue_style(cue: *mut Com) -> (Align, bool) {
    // SAFETY: as the caller promises; the style is read and released.
    let Some(style) = (unsafe { cue_part(cue, 11) }) else { return (Align::Center, false) };
    let (mut alignment, mut font) = (2u32, 0u32);
    // SAFETY: a live style.
    unsafe {
        call!(style, 13, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut alignment);
        call!(style, 10, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut font);
        Com::release(style);
    }
    let alignment = match alignment {
        0 => Align::Start,
        1 => Align::End,
        _ => Align::Center,
    };
    (alignment, matches!(font, 1 | 2))
}

/// The place of a cue's region, where it gives one in percent (4: GetPosition, 5: GetExtent,
/// 8: GetDisplayAlignment: 0 its top, 1 its bottom, 2 its middle), its lines lined up as
/// `lines`. (Matroska's SSA comes so: its alignment, margins and `\pos` as regions.)
///
/// # Safety
/// `cue` is a live `IMFTimedTextCue`.
unsafe fn region_place(cue: *mut Com, lines: Align) -> Option<Place> {
    // SAFETY: as the caller promises; the region is read and released.
    let region = unsafe { cue_part(cue, 10) }?;
    let ([mut x, mut y, mut width, mut height], [mut at_unit, mut size_unit], mut display) = ([0f64; 4], [0u32; 2], 0u32);
    // SAFETY: a live region.
    unsafe {
        call!(region, 4, unsafe extern "system" fn(*mut Com, *mut f64, *mut f64, *mut u32) -> i32, &mut x, &mut y, &mut at_unit);
        call!(region, 5, unsafe extern "system" fn(*mut Com, *mut f64, *mut f64, *mut u32) -> i32, &mut width, &mut height, &mut size_unit);
        call!(region, 8, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut display);
        Com::release(region);
    }
    // (1: MF_TIMED_TEXT_UNIT_TYPE_PERCENTAGE.)
    if (at_unit, size_unit) != (1, 1) || width.is_nan() || width <= 0.0 {
        return None;
    }
    let [x, y, width, height] = [x, y, width, height].map(|value| (value / 100.0).clamp(0.0, 1.0) as f32);
    let across = match lines {
        Align::Start => x,
        Align::End => x + width,
        _ => x + width / 2.0,
    };
    let down = match display {
        1 => Down::At(y + height, Align::End),
        2 => Down::At(y + height / 2.0, Align::Center),
        _ => Down::At(y, Align::Start),
    };
    Some(Place { x: across, across: lines, down, width, lines })
}

/// Text a method gives as a `LPWSTR` the caller frees, empty where none.
///
/// # Safety
/// `object` is alive, and its method `at` writes such text.
unsafe fn owned_text(object: *mut Com, at: usize) -> String {
    let mut text = null_mut::<u16>();
    // SAFETY: as the caller promises; the text is copied and freed.
    unsafe {
        call!(object, at, unsafe extern "system" fn(*mut Com, *mut *mut u16) -> i32, &mut text);
        let owned = if text.is_null() { String::new() } else { text_at(text) };
        CoTaskMemFree(text.cast());
        owned
    }
}

/// Text ending in a zero.
///
/// # Safety
/// `text` points at UTF-16 text ending in a zero.
unsafe fn text_at(text: *const u16) -> String {
    // SAFETY: as the caller promises.
    unsafe {
        let length = (0..).take_while(|at| *text.add(*at) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
    }
}

// ---- the captions in the video stream ----

/// What the captions' reader reads: the source's address (or path, ending in a zero), a clone
/// of the bytes' stream, or a byte stream of an address's download.
enum ReaderSource {
    Address(Vec<u16>),
    Stream(*mut Com),
    ByteStream(*mut Com),
}

// SAFETY: a memory stream (`SHCreateMemStream`) and a download's byte stream may be used from
// any thread; each moves to the reader's thread, which alone uses it.
unsafe impl Send for ReaderSource {}

impl Drop for ReaderSource {
    fn drop(&mut self) {
        if let ReaderSource::Stream(stream) | ReaderSource::ByteStream(stream) = *self {
            // SAFETY: held once.
            unsafe { Com::release(stream) };
        }
    }
}

/// An address being downloaded: the download, and its first bytes once read (none where it
/// can't be).
struct Pending {
    address: String,
    download: Arc<Download>,
    first: mpsc::Receiver<Vec<u8>>,
}

/// A clone of a memory stream, read by a caption reader (3: Read, 5: Seek).
struct MemoryStream(*mut Com);

// SAFETY: a memory stream's clone may be used from any thread; it moves to the reader's thread,
// which alone uses it.
unsafe impl Send for MemoryStream {}

impl Source for MemoryStream {
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
        let (mut moved, mut read) = (0u64, 0u32);
        // SAFETY: a live stream, read within the room given.
        unsafe {
            if call!(self.0, 5, unsafe extern "system" fn(*mut Com, i64, u32, *mut u64) -> i32, at as i64, 0, &mut moved) < 0 {
                return 0;
            }
            call!(self.0, 3, unsafe extern "system" fn(*mut Com, *mut u8, u32, *mut u32) -> i32, into.as_mut_ptr(), into.len().min(u32::MAX as usize) as u32, &mut read);
        }
        read as usize
    }
}

impl Drop for MemoryStream {
    fn drop(&mut self) {
        // SAFETY: held once.
        unsafe { Com::release(self.0) };
    }
}

/// A clone of a memory stream (13: Clone), held; none where it can't be made.
///
/// # Safety
/// `memory` is a live `IStream`.
unsafe fn cloned(memory: *mut Com) -> Option<*mut Com> {
    let mut copy = null_mut();
    // SAFETY: as the caller promises; the clone is ours.
    let made = unsafe { call!(memory, 13, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut copy) };
    (made >= 0 && !copy.is_null()).then_some(copy)
}

/// An error where an `HRESULT` says the engine refused its source.
fn refused(done: i32) -> Result<(), VideoError> {
    match done < 0 {
        true => Err(VideoError::Refused(format!("the source was refused ({:#010x})", done as u32))),
        false => Ok(()),
    }
}

/// The kind of file at an address, as the engine knows it: its file name's extension (before
/// any query), else the kind its first bytes show.
fn kind_of_address(address: &str, first: &[u8]) -> String {
    let path = address.split(['?', '#']).next().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    let extension = name.rsplit_once('.').map(|(_, extension)| extension).filter(|extension| (1..=5).contains(&extension.len()) && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    extension.map_or_else(|| crate::video::kind_of(first).to_owned(), str::to_ascii_lowercase)
}

/// The captions' thread: reads the samples of the source's video stream as stored (an
/// `IMFSourceReader`, no decoder), where it is H.264 or HEVC, a little ahead of where it plays,
/// until the player goes.
fn read_captions_in_stream(functions: &Functions, source: ReaderSource, in_stream: &ReadAhead<SeiReading>) {
    // SAFETY: COM for this thread, multithreaded (0), ended with it where it started.
    let com = unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 0) } >= 0;
    if let Some(reader) = source_reader(functions, &source) {
        if let Some(codec) = video_codec(reader) {
            read_samples(reader, codec, in_stream);
        }
        // SAFETY: held once.
        unsafe { Com::release(reader) };
    }
    drop(source);
    if com {
        // SAFETY: started above.
        unsafe { crate::windows::sys::CoUninitialize() };
    }
}

/// A source reader of `source`, with only its first video stream chosen (4:
/// SetStreamSelection); none where it can't be read.
fn source_reader(functions: &Functions, source: &ReaderSource) -> Option<*mut Com> {
    let mut reader = null_mut();
    // SAFETY: the address ends in a zero, the stream is alive; the reader made is ours.
    unsafe {
        let made = match *source {
            ReaderSource::Address(ref address) => (functions.reader_from_url)(address.as_ptr(), null_mut(), &mut reader),
            ReaderSource::Stream(stream) => stream_reader(functions, stream, &mut reader),
            ReaderSource::ByteStream(stream) => (functions.reader_from_stream)(stream, null_mut(), &mut reader),
        };
        if made < 0 || reader.is_null() {
            return None;
        }
        call!(reader, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, ALL_STREAMS, 0);
        call!(reader, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, FIRST_VIDEO, 1);
    }
    Some(reader)
}

/// Makes a source reader of a stream, through a byte stream over it (released once the
/// reader holds it): its `HRESULT`.
///
/// # Safety
/// `stream` is a live `IStream`.
unsafe fn stream_reader(functions: &Functions, stream: *mut Com, reader: &mut *mut Com) -> i32 {
    let mut bytes = null_mut();
    // SAFETY: as the caller promises; the byte stream is ours.
    unsafe {
        let made = (functions.on_stream)(stream, &mut bytes);
        if made < 0 || bytes.is_null() {
            return -1;
        }
        let made = (functions.reader_from_stream)(bytes, null_mut(), reader);
        Com::release(bytes);
        made
    }
}

/// The codec of the reader's video stream, where its SEI is read (6: GetCurrentMediaType; of
/// the type, 10: GetGUID).
fn video_codec(reader: *mut Com) -> Option<Codec> {
    let (mut kind, mut subtype) = (null_mut(), [0u32; 4]);
    // SAFETY: a live reader; the type it gives is read and released.
    unsafe {
        if call!(reader, 6, unsafe extern "system" fn(*mut Com, u32, *mut *mut Com) -> i32, FIRST_VIDEO, &mut kind) < 0 || kind.is_null() {
            return None;
        }
        call!(kind, 10, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut [u32; 4]) -> i32, &SUBTYPE, &mut subtype);
        Com::release(kind);
    }
    match subtype {
        H264 => Some(Codec::H264),
        HEVC => Some(Codec::Hevc),
        _ => None,
    }
}

/// Reads the video stream's samples as the player's side lets it, following its seeks.
fn read_samples(reader: *mut Com, codec: Codec, in_stream: &ReadAhead<SeiReading>) {
    let mut read_to = Duration::ZERO;
    loop {
        match in_stream.next(read_to) {
            Step::Stop => return,
            Step::Seek(to) => read_to = seek_reader(reader, to),
            Step::Read => read_to = read_sample(reader, codec, in_stream),
        }
    }
}

/// Goes to the key frame at or before `to` (8: SetCurrentPosition, in 100 ns as `VT_I8`).
fn seek_reader(reader: *mut Com, to: Duration) -> Duration {
    let position = PropVariant { kind: 20, reserved: [0; 3], data: (to.as_nanos() / 100).min(i64::MAX as u128) as i64 as *mut c_void, more: 0 };
    // SAFETY: a live reader; the time format is GUID_NULL, 100 ns.
    unsafe { call!(reader, 8, unsafe extern "system" fn(*mut Com, *const [u32; 4], *const PropVariant) -> i32, &[0; 4], &position) };
    to
}

/// A sample's decode time, where the source gives one (`MFSampleExtension_DecodeTimestamp`,
/// in 100 ns; 8: GetUINT64).
///
/// # Safety
/// `sample` is a live `IMFSample`.
unsafe fn decode_time(sample: *mut Com) -> Option<Duration> {
    let mut time = 0u64;
    // SAFETY: as the caller promises.
    let got = unsafe { call!(sample, 8, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut u64) -> i32, &DECODE_TIME, &mut time) };
    (got >= 0).then(|| Duration::from_nanos(time.saturating_mul(100)))
}

/// Reads the next sample into the captions (9: ReadSample; of the sample, 41:
/// ConvertToContiguousBuffer): its time; at the end (or a failure), `Duration::MAX`, the
/// captions waiting read.
fn read_sample(reader: *mut Com, codec: Codec, in_stream: &ReadAhead<SeiReading>) -> Duration {
    let (mut flags, mut time, mut sample, mut buffer) = (0u32, 0i64, null_mut(), null_mut());
    // SAFETY: a live reader; the sample and its buffer are read and released.
    unsafe {
        let read = call!(reader, 9, unsafe extern "system" fn(*mut Com, u32, u32, *mut u32, *mut u32, *mut i64, *mut *mut Com) -> i32, FIRST_VIDEO, 0, null_mut(), &mut flags, &mut time, &mut sample);
        // (1: an error, 2: the end of the stream.)
        if read < 0 || flags & 3 != 0 {
            release_all(&[sample]);
            in_stream.ended();
            return Duration::MAX;
        }
        let at = Duration::from_nanos(time.max(0) as u64 * 100);
        if !sample.is_null() && call!(sample, 41, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut buffer) >= 0 && !buffer.is_null() {
            let decoded = decode_time(sample);
            locked(buffer, |bytes| in_stream.read(bytes, codec, at, decoded));
        }
        release_all(&[buffer, sample]);
        at
    }
}

/// What `read` makes of a media buffer's bytes, locked while it reads them (3: Lock, 4: Unlock).
///
/// # Safety
/// `buffer` is a live `IMFMediaBuffer`.
unsafe fn locked(buffer: *mut Com, read: impl FnOnce(&[u8])) {
    let (mut bytes, mut length) = (null_mut::<u8>(), 0u32);
    // SAFETY: as the caller promises; the bytes are read within their length while locked.
    unsafe {
        if call!(buffer, 3, unsafe extern "system" fn(*mut Com, *mut *mut u8, *mut u32, *mut u32) -> i32, &mut bytes, null_mut(), &mut length) < 0 || bytes.is_null() {
            return;
        }
        read(std::slice::from_raw_parts(bytes, length as usize));
        call!(buffer, 4, unsafe extern "system" fn(*mut Com) -> i32);
    }
}

/// `PROPVARIANT`: its type, and the value or a pointer to it.
#[repr(C)]
struct PropVariant {
    kind: u16,
    reserved: [u16; 3],
    data: *mut c_void,
    more: usize,
}

/// `TransferVideoFrame`, `CopyPixels`, `Map` and `CreateTexture2D`.
type Transfer = unsafe extern "system" fn(*mut Com, *mut Com, *const c_void, *const [i32; 4], *const c_void) -> i32;
type CopyPixels = unsafe extern "system" fn(*mut Com, *const c_void, u32, u32, *mut u32) -> i32;
type Map = unsafe extern "system" fn(*mut Com, *mut Com, u32, u32, u32, *mut Pixels) -> i32;
type MakeTexture = unsafe extern "system" fn(*mut Com, *const TextureDescription, *const c_void, *mut *mut Com) -> i32;

/// Where the engine puts each new frame.
enum Frames {
    /// On the GPU, in a texture of the device it decodes on (made at the first frame, again
    /// when the size changes).
    Gpu { gpu: Arc<Gpu>, texture: Option<Arc<FrameTexture>> },
    /// A WIC bitmap, made by WIC's factory.
    Bitmap { wic: *mut Com, bitmap: *mut Com, size: (u32, u32) },
}

impl Frames {
    /// Frames on `gpu`, or in a bitmap where none.
    fn new(gpu: Option<Arc<Gpu>>) -> Result<Frames, VideoError> {
        if let Some(gpu) = gpu {
            return Ok(Frames::Gpu { gpu, texture: None });
        }
        let mut wic = null_mut();
        // SAFETY: the factory is made with its interface id.
        let made = unsafe { crate::windows::sys::CoCreateInstance(&WIC_FACTORY, null_mut(), 1, &A_WIC_FACTORY, &mut wic) };
        match made >= 0 && !wic.is_null() {
            true => Ok(Frames::Bitmap { wic, bitmap: null_mut(), size: (0, 0) }),
            false => Err(VideoError::NoPlayer("no Windows Imaging Component".into())),
        }
    }

    /// Makes the target this size where it is not: whether it is.
    fn sized(&mut self, width: u32, height: u32) -> bool {
        match self {
            Frames::Gpu { texture: Some(texture), .. } if texture.size == (width, height) => true,
            Frames::Gpu { gpu, texture } => {
                *texture = FrameTexture::new(gpu, (width, height)).map(Arc::new);
                texture.is_some()
            }
            Frames::Bitmap { size, bitmap, .. } if *size == (width, height) && !bitmap.is_null() => true,
            Frames::Bitmap { wic, bitmap, size } => {
                *size = (width, height);
                remade_bitmap(*wic, bitmap, *size)
            }
        }
    }

    /// Has the engine transfer its frame into the target, `whole` its rectangle (43:
    /// TransferVideoFrame): whether it did. Not while a window's device holds the texture: the
    /// engine keeps the frame (`OnVideoStreamTick` gives it again), so the next tick puts it in.
    /// A texture whose keyed mutex was abandoned (a device lost while holding it) is dropped, to
    /// be made again.
    fn transfer(&mut self, engine: *mut Com, whole: &[i32; 4]) -> bool {
        // SAFETY: the engine is alive; the target is a live texture or bitmap of that size,
        // the texture held by this device while drawn into.
        let sent = unsafe {
            match self {
                Frames::Gpu { texture: Some(texture), .. } => texture.held(0, || texture.filled(call!(engine, 43, Transfer, texture.drawn, null(), whole, null()) >= 0)),
                Frames::Gpu { texture: None, .. } => Ok(false),
                Frames::Bitmap { bitmap, .. } => Ok(call!(engine, 43, Transfer, *bitmap, null(), whole, null()) >= 0),
            }
        };
        match (sent, self) {
            (Ok(sent), _) => sent,
            (Err(taken), Frames::Gpu { texture, .. }) if taken != WAIT_TIMEOUT => {
                *texture = None;
                false
            }
            // (Timed out: the next tick tries again.)
            (Err(_), _) => false,
        }
    }

    /// Reads the frame transferred into the target: whether it did.
    fn read(&self, into: &mut Vec<u32>) -> bool {
        match self {
            Frames::Gpu { texture: Some(texture), .. } => texture.read(into),
            Frames::Gpu { texture: None, .. } => false,
            Frames::Bitmap { bitmap, size, .. } => read_bitmap(*bitmap, *size, into),
        }
    }
}

/// A frame on the GPU: the texture the engine draws it into, and a copy the CPU reads it from.
/// With `gpu-draw` the texture is shared with windows' devices, which draw the frame from it:
/// a keyed mutex orders the devices' work on it; Direct3D 12, which has none, waits for a
/// fence the decoder's device signals once each frame is in.
pub(crate) struct FrameTexture {
    gpu: Arc<Gpu>,
    drawn: *mut Com,
    readable: *mut Com,
    /// Its keyed mutex (`IDXGIKeyedMutex`), and the NT handle other devices open it by: null and
    /// 0 where the device could not share it.
    #[cfg(feature = "gpu-draw")]
    mutex: *mut Com,
    #[cfg(feature = "gpu-draw")]
    pub(crate) handle: isize,
    /// The fence (`ID3D11Fence`), its NT handle, and the value it is signalled to once the
    /// newest frame is in: null and 0 where there is none.
    #[cfg(feature = "gpu-draw")]
    fence: *mut Com,
    #[cfg(feature = "gpu-draw")]
    pub(crate) fence_handle: isize,
    #[cfg(feature = "gpu-draw")]
    pub(crate) fence_value: AtomicU64,
    /// Its number among the textures made, which no other has (a handle's may be used again).
    #[cfg(feature = "gpu-draw")]
    serial: u64,
    /// Its width and height.
    pub(crate) size: (u32, u32),
}

// SAFETY: its device is multithread protected (see `Gpu`); the textures are used under its
// lock, the shared one under its keyed mutex.
unsafe impl Send for FrameTexture {}
// SAFETY: as above.
unsafe impl Sync for FrameTexture {}

/// How long a device waits for another's work on a frame's texture, in ms (`AcquireSync`), and
/// what it gives when that passes. (The decoder's transfer never waits.)
pub(crate) const FRAME_WAIT: u32 = 100;
const WAIT_TIMEOUT: i32 = 0x102;
/// How many frame textures were made (each one's `serial`).
#[cfg(feature = "gpu-draw")]
static MADE: AtomicU64 = AtomicU64::new(1);

/// `IDXGIKeyedMutex`, `IDXGIResource1`, `ID3D11Device5`, `ID3D11DeviceContext4` and
/// `ID3D11Fence`.
#[cfg(feature = "gpu-draw")]
pub(crate) const A_KEYED_MUTEX: [u32; 4] = named(0x9D8E_1289, 0xD7B3, 0x465F, [0x81, 0x26, 0x25, 0x0E, 0x34, 0x9A, 0xF8, 0x5D]);
#[cfg(feature = "gpu-draw")]
const A_DXGI_RESOURCE: [u32; 4] = named(0x3096_1379, 0x4609, 0x4A41, [0x99, 0x8E, 0x54, 0xFE, 0x56, 0x7E, 0xE0, 0xC1]);
#[cfg(feature = "gpu-draw")]
const A_FENCE_MAKER: [u32; 4] = named(0x8FFD_E202, 0xA0E7, 0x45DF, [0x9E, 0x01, 0xE8, 0x37, 0x80, 0x1B, 0x5E, 0xA0]);
#[cfg(feature = "gpu-draw")]
const A_SIGNALLING_CONTEXT: [u32; 4] = named(0x9176_00DA, 0xF58C, 0x4C33, [0x98, 0xD8, 0x3E, 0x15, 0xB3, 0x90, 0xFA, 0x24]);
#[cfg(feature = "gpu-draw")]
const A_FENCE: [u32; 4] = named(0xAFFD_E9D1, 0x1DF7, 0x4BB7, [0x8A, 0x34, 0x0F, 0x46, 0x25, 0x1D, 0xAB, 0x80]);

/// `CreateSharedHandle`: an NT handle to open by from any device of this process, read and
/// written (`DXGI_SHARED_RESOURCE_READ | WRITE`; `GENERIC_ALL` for a fence).
#[cfg(feature = "gpu-draw")]
type MakeHandle = unsafe extern "system" fn(*mut Com, *const c_void, u32, *const u16, *mut isize) -> i32;

impl FrameTexture {
    /// The two textures, this size, on `gpu`; none where they can't be made. (0x28: a render
    /// and shader target, with `gpu-draw` shared by an NT handle with a keyed mutex, 0x900, where
    /// the device can; 3, 0x20000: staging, read by the CPU.)
    fn new(gpu: &Arc<Gpu>, (width, height): (u32, u32)) -> Option<FrameTexture> {
        let shared = if cfg!(feature = "gpu-draw") { 0x900 } else { 0 };
        // SAFETY: the device is alive.
        let (mut drawn, readable) = unsafe { (texture(gpu, width, height, [0, 0x28, 0, shared]), texture(gpu, width, height, [3, 0, 0x20000, 0])) };
        if drawn.is_null() {
            // SAFETY: as above.
            drawn = unsafe { texture(gpu, width, height, [0, 0x28, 0, 0]) };
        }
        #[cfg(not(feature = "gpu-draw"))]
        let made = FrameTexture { gpu: gpu.clone(), drawn, readable, size: (width, height) };
        #[cfg(feature = "gpu-draw")]
        let made = FrameTexture {
            gpu: gpu.clone(),
            drawn,
            readable,
            mutex: null_mut(),
            handle: 0,
            fence: null_mut(),
            fence_handle: 0,
            fence_value: AtomicU64::new(0),
            serial: MADE.fetch_add(1, Ordering::Relaxed),
            size: (width, height),
        }
        .shared();
        (!made.drawn.is_null() && !made.readable.is_null()).then_some(made)
    }

    /// The same with its keyed mutex and NT handle (of `IDXGIResource1`, 13), where it is
    /// shared, and its fence: else windows' devices can't open it, and its frames are read back.
    #[cfg(feature = "gpu-draw")]
    fn shared(mut self) -> FrameTexture {
        if self.drawn.is_null() {
            return self;
        }
        let mut resource = null_mut();
        // SAFETY: the texture is alive; what it gives is released (the mutex with it, if any).
        unsafe {
            call!(self.drawn, 0, Ask, &A_KEYED_MUTEX, &mut self.mutex);
            call!(self.drawn, 0, Ask, &A_DXGI_RESOURCE, &mut resource);
            if !resource.is_null() {
                call!(resource, 13, MakeHandle, null(), 0x8000_0001, null(), &mut self.handle);
                Com::release(resource);
            }
        }
        if self.handle != 0 && !self.gpu.signaller.is_null() {
            self.with_fence();
        }
        self
    }

    /// Makes the fence and its NT handle (`ID3D11Device5::CreateFence`, 68, shared: 0x2; the
    /// fence's CreateSharedHandle, 7).
    #[cfg(feature = "gpu-draw")]
    fn with_fence(&mut self) {
        let mut maker = null_mut();
        // SAFETY: the device is alive; what it gives is released, the fence with this.
        unsafe {
            call!(self.gpu.device, 0, Ask, &A_FENCE_MAKER, &mut maker);
            if maker.is_null() {
                return;
            }
            call!(maker, 68, unsafe extern "system" fn(*mut Com, u64, u32, *const [u32; 4], *mut *mut Com) -> i32, 0, 0x2, &A_FENCE, &mut self.fence);
            Com::release(maker);
            if !self.fence.is_null() {
                call!(self.fence, 7, MakeHandle, null(), 0x1000_0000, null(), &mut self.fence_handle);
            }
        }
    }

    /// After a frame was put in (`sent`), has the fence signalled once that is done, after all
    /// the device was given (`ID3D11DeviceContext4::Signal`, 147). Gives `sent`.
    fn filled(&self, sent: bool) -> bool {
        #[cfg(feature = "gpu-draw")]
        if sent && self.fence_handle != 0 {
            let next = self.fence_value.load(Ordering::Relaxed) + 1;
            // SAFETY: the context and fence are alive; the context is multithread protected.
            let signalled = unsafe { call!(self.gpu.signaller, 147, unsafe extern "system" fn(*mut Com, *mut Com, u64) -> i32, self.fence, next) } >= 0;
            if signalled {
                self.fence_value.store(next, Ordering::Release);
            }
        }
        sent
    }

    /// Does `work` on the texture drawn into, holding its keyed mutex (8: AcquireSync, 9:
    /// ReleaseSync): other devices' work on it is done first, and theirs waits for this. Not
    /// where the mutex can't be taken within `wait` ms: what `AcquireSync` gave instead.
    fn held<R>(&self, wait: u32, work: impl FnOnce() -> R) -> Result<R, i32> {
        #[cfg(feature = "gpu-draw")]
        if !self.mutex.is_null() {
            // SAFETY: the mutex is alive; key 0 is the one every device takes (`S_OK` only: a
            // time-out and an abandoned mutex are success codes).
            let taken = unsafe { call!(self.mutex, 8, unsafe extern "system" fn(*mut Com, u64, u32) -> i32, 0, wait) };
            if taken != 0 {
                return Err(taken);
            }
            let done = work();
            // SAFETY: taken above.
            unsafe { call!(self.mutex, 9, unsafe extern "system" fn(*mut Com, u64) -> i32, 0) };
            return Ok(done);
        }
        #[cfg(not(feature = "gpu-draw"))]
        let _ = wait;
        Ok(work())
    }

    /// Copies the frame to the readable texture and reads its rows into `into`, opaque:
    /// whether it could (of the device's context, 47: CopyResource, 14: Map, 15: Unmap).
    pub(crate) fn read(&self, into: &mut Vec<u32>) -> bool {
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        into.resize(width * height, 0);
        let mut mapped = Pixels { first: null(), row_pitch: 0, slice_pitch: 0 };
        // SAFETY: two live textures of one size and format; the copy is mapped for reading.
        let mapped_now = unsafe {
            self.held(FRAME_WAIT, || call!(self.gpu.context, 47, unsafe extern "system" fn(*mut Com, *mut Com, *mut Com), self.readable, self.drawn)).is_ok()
                && call!(self.gpu.context, 14, Map, self.readable, 0, 1, 0, &mut mapped) >= 0
                && !mapped.first.is_null()
        };
        if !mapped_now {
            return false;
        }
        for (row, line) in into.chunks_exact_mut(width).enumerate() {
            // SAFETY: each mapped row holds `width` pixels, at its pitch from the first.
            let from = unsafe { std::slice::from_raw_parts(mapped.first.cast::<u8>().add(row * mapped.row_pitch as usize).cast::<u32>(), width) };
            line.iter_mut().zip(from).for_each(|(pixel, from)| *pixel = from | 0xFF00_0000);
        }
        // SAFETY: mapped above.
        unsafe { call!(self.gpu.context, 15, unsafe extern "system" fn(*mut Com, *mut Com, u32), self.readable, 0) };
        #[cfg(test)]
        crate::video::READ_BACK.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[cfg(feature = "gpu-draw")]
impl crate::gpu_frames::Frame for FrameTexture {
    /// Its number among the textures made.
    fn key(&self) -> Option<u64> {
        Some(self.serial)
    }
    /// The texture itself: the decoder puts each frame into it while the size holds.
    fn source(this: &Arc<Self>) -> std::sync::Weak<dyn Send + Sync> {
        Arc::downgrade(this) as std::sync::Weak<dyn Send + Sync>
    }
}

impl Drop for FrameTexture {
    fn drop(&mut self) {
        // SAFETY: each is held once, or null; the handles are this texture's own.
        unsafe {
            #[cfg(feature = "gpu-draw")]
            {
                release_all(&[self.mutex, self.fence]);
                [self.handle, self.fence_handle].into_iter().filter(|handle| *handle != 0).for_each(|handle| _ = CloseHandle(handle as usize));
            }
            release_all(&[self.drawn, self.readable]);
        }
    }
}

/// Makes a bitmap this size in place of the old: whether it was made (17:
/// `IWICImagingFactory::CreateBitmap`, 1: its own memory).
fn remade_bitmap(wic: *mut Com, bitmap: &mut *mut Com, (width, height): (u32, u32)) -> bool {
    // SAFETY: the old bitmap is released, a new one made by the live factory.
    unsafe {
        release_all(&[*bitmap]);
        *bitmap = null_mut();
        call!(wic, 17, unsafe extern "system" fn(*mut Com, u32, u32, *const [u32; 4], u32, *mut *mut Com) -> i32, width, height, &BGRA, 1, bitmap);
    }
    !bitmap.is_null()
}

/// Reads a bitmap's pixels into `into` (7: CopyPixels), opaque: whether it could.
fn read_bitmap(bitmap: *mut Com, (width, height): (u32, u32), into: &mut Vec<u32>) -> bool {
    into.resize(width as usize * height as usize, 0);
    // SAFETY: a live bitmap of that size, and `into` holds all its pixels.
    let read = unsafe { call!(bitmap, 7, CopyPixels, null(), width * 4, width * height * 4, into.as_mut_ptr()) >= 0 };
    into.iter_mut().for_each(|pixel| *pixel |= 0xFF00_0000);
    read
}

/// A BGRA texture on the GPU: its usage, bind flags, CPU access and other flags (5:
/// CreateTexture2D).
///
/// # Safety
/// The device is alive.
unsafe fn texture(gpu: &Gpu, width: u32, height: u32, [usage, bind_flags, cpu_access_flags, misc_flags]: [u32; 4]) -> *mut Com {
    let described = TextureDescription { width, height, mip_levels: 1, array_size: 1, format: B8G8R8A8_UNORM, sample_count: 1, sample_quality: 0, usage, bind_flags, cpu_access_flags, misc_flags };
    let mut made = null_mut();
    // SAFETY: as the caller promises.
    unsafe { call!(gpu.device, 5, MakeTexture, &described, null(), &mut made) };
    made
}

// ---- the system's caption settings ----

/// `IClosedCaptionPropertiesStatics`.
const CAPTION_SETTINGS: [u32; 4] = named(0x10AA_1F84, 0xCC30, 0x4141, [0xB5, 0x03, 0x52, 0x72, 0x28, 0x9E, 0x0C, 0x20]);

/// How captions look, as Windows' caption settings say (WinRT's `ClosedCaptionProperties`);
/// the usual where they can't be read. (COM is started on this thread: a video was opened.)
pub(crate) fn caption_style() -> CaptionStyle {
    let settings = caption_settings();
    if settings.is_null() {
        return CaptionStyle::default();
    }
    // SAFETY: the statics are alive (after `IInspectable`'s six: 7 ComputedFontColor, 8
    // FontOpacity, 9 FontSize, 13 ComputedBackgroundColor, 14 BackgroundOpacity); released
    // once read.
    unsafe {
        let number = |at| {
            let mut value = 0i32;
            call!(settings, at, unsafe extern "system" fn(*mut Com, *mut i32) -> i32, &mut value);
            value
        };
        let colour = |at, opacity: i32, usual: f32| {
            let mut argb = [0u8; 4];
            call!(settings, at, unsafe extern "system" fn(*mut Com, *mut [u8; 4]) -> i32, &mut argb);
            let [_, red, green, blue] = argb;
            let alpha = [usual, 1.0, 0.75, 0.25, 0.0][opacity.clamp(0, 4) as usize];
            Color::rgb(u32::from_be_bytes([0, red, green, blue])).with_alpha(alpha)
        };
        let style = CaptionStyle { text: colour(7, number(8), 1.0), background: colour(13, number(14), 0.8), scale: [1.0, 0.5, 1.0, 1.5, 2.0][number(9).clamp(0, 4) as usize] };
        Com::release(settings);
        style
    }
}

/// The statics of `Windows.Media.ClosedCaptioning.ClosedCaptionProperties`, from combase's
/// `RoGetActivationFactory`; null where there are none.
fn caption_settings() -> *mut Com {
    let library = system_library("combase.dll");
    let found = |name: &CStr| function_in(library, name);
    // SAFETY: the functions found have the types combase's headers give; the class name is made
    // and deleted around the call.
    unsafe {
        let (make, factory, delete) = (found(c"WindowsCreateString"), found(c"RoGetActivationFactory"), found(c"WindowsDeleteString"));
        if library == 0 || make == 0 || factory == 0 || delete == 0 {
            return null_mut();
        }
        let make: unsafe extern "system" fn(*const u16, u32, *mut *mut c_void) -> i32 = function(make);
        let factory: unsafe extern "system" fn(*mut c_void, *const [u32; 4], *mut *mut Com) -> i32 = function(factory);
        let delete: unsafe extern "system" fn(*mut c_void) -> i32 = function(delete);
        let class = wide("Windows.Media.ClosedCaptioning.ClosedCaptionProperties");
        let (mut name, mut statics) = (null_mut(), null_mut());
        if make(class.as_ptr(), class.len() as u32 - 1, &mut name) < 0 {
            return null_mut();
        }
        factory(name, &CAPTION_SETTINGS, &mut statics);
        delete(name);
        statics
    }
}

/// Releases each object that is there.
///
/// # Safety
/// Each is a live COM object held once, or null.
unsafe fn release_all(objects: &[*mut Com]) {
    for object in objects.iter().filter(|object| !object.is_null()) {
        // SAFETY: as the caller promises.
        unsafe { Com::release(*object) };
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        if let Frames::Bitmap { wic, bitmap, .. } = *self {
            // SAFETY: each is held once, or null.
            unsafe { release_all(&[bitmap, wic]) };
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.in_stream.stop();
        self.file_captions.stop();
        // SAFETY: the engine, where it was made, is shut down (42) and released with its
        // interfaces; then our references to what tells us.
        unsafe {
            release_all(&[self.timed_text, self.ex]);
            if !self.engine.is_null() {
                self.call::<i32>(42);
                Com::release(self.engine);
            }
            release(self.said);
            release(self.showing);
            #[cfg(test)]
            if !self.hearing.is_null() {
                release(self.hearing);
            }
        }
    }
}

#[cfg(test)]
impl Player {
    /// A player of `source` with sound, an effect before its sound renderer keeping what it
    /// hears (inserted before the source: 61, `IMFMediaEngineEx::InsertAudioEffect`).
    pub(crate) fn listening(source: VideoSource) -> Result<Player, VideoError> {
        let mut player = Player::new(true, true)?;
        player.hearing = hearing::listener();
        // SAFETY: the engine's extended interface is alive; it takes its own reference.
        let inserted = unsafe { call!(player.ex, 61, unsafe extern "system" fn(*mut Com, *mut c_void, i32) -> i32, player.hearing.cast(), 0) };
        assert!(inserted >= 0, "the effect was refused ({:#010x})", inserted as u32);
        player.set_source(source)?;
        Ok(player)
    }
    /// What the effect heard last: its first channel, and its rate.
    pub(crate) fn heard(&mut self) -> Option<(Vec<f32>, u32)> {
        (!self.hearing.is_null()).then(|| hearing::heard(self.hearing)).flatten()
    }
    /// A player of `source` decoding in software, with no sound.
    pub(crate) fn in_software(source: VideoSource) -> Result<Player, VideoError> {
        let mut player = Player::new(false, false)?;
        player.set_source(source)?;
        Ok(player)
    }
    /// Takes the next seek's frame as lost, as the engine sometimes loses it.
    pub(crate) fn lose_next_frame(&mut self) {
        self.lose = true;
    }
    /// The timed text telling of a track added or removed, as it does from a thread of its own.
    pub(crate) fn tell_track(&self, added: bool) {
        // SAFETY: the listener lives as long as the player.
        unsafe {
            match added {
                true => track_added(self.showing, 0),
                false => track_removed(self.showing, 0),
            }
        }
    }
}

#[cfg(test)]
mod hearing {
    //! For tests: an audio effect the engine puts before its sound renderer (`IMFTransform`,
    //! passing each sample on as it is), which keeps what it heard last. The engine plays muted:
    //! nothing reaches the speakers.
    use super::*;

    /// `IMFTransform`; a media type's `MF_MT_AUDIO_NUM_CHANNELS`, `MF_MT_AUDIO_SAMPLES_PER_SECOND`;
    /// `MFAudioFormat_PCM` and `MFAudioFormat_Float`.
    const A_TRANSFORM: [u32; 4] = named(0xBF94_C121, 0x5B05, 0x4E6F, [0x80, 0x00, 0xBA, 0x59, 0x89, 0x61, 0x41, 0x4D]);
    const CHANNELS: [u32; 4] = named(0x37E4_8BF5, 0x645E, 0x4C5B, [0x89, 0xDE, 0xAD, 0xA9, 0xE2, 0x9B, 0x69, 0x6A]);
    const RATE: [u32; 4] = named(0x5FAE_EAE7, 0x0290, 0x4C31, [0x9E, 0x8A, 0xC5, 0x34, 0xF6, 0x8D, 0x9D, 0xBA]);
    const PCM: [u32; 4] = named(1, 0, 0x0010, [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    const FLOAT: [u32; 4] = named(3, 0, 0x0010, [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    const NOT_IMPLEMENTED: i32 = 0x8000_4001_u32 as i32;
    const NO_MORE_TYPES: i32 = 0xC00D_36B9_u32 as i32;
    const INVALID_TYPE: i32 = 0xC00D_36B4_u32 as i32;
    const NOT_ACCEPTING: i32 = 0xC00D_36B5_u32 as i32;
    const TYPE_NOT_SET: i32 = 0xC00D_6D60_u32 as i32;
    const NEED_MORE_INPUT: i32 = 0xC00D_6D72_u32 as i32;

    #[link(name = "mfplat")]
    unsafe extern "system" {
        fn MFCreateMediaType(kind: *mut *mut Com) -> i32;
    }

    /// The type taken and given (the same), the sample held between, and the last sound heard:
    /// its first channel, and its rate.
    pub(super) struct Hearing {
        input: *mut Com,
        output: *mut Com,
        held: *mut Com,
        heard: Vec<f32>,
        rate: u32,
    }

    type This = *mut Listener<Mutex<Hearing>>;

    #[repr(C)]
    struct Table {
        unknown: ListenerTable<Mutex<Hearing>>,
        stream_limits: unsafe extern "system" fn(This, *mut u32, *mut u32, *mut u32, *mut u32) -> i32,
        stream_count: unsafe extern "system" fn(This, *mut u32, *mut u32) -> i32,
        stream_ids: unsafe extern "system" fn(This, u32, *mut u32, u32, *mut u32) -> i32,
        input_info: unsafe extern "system" fn(This, u32, *mut [u32; 6]) -> i32,
        output_info: unsafe extern "system" fn(This, u32, *mut [u32; 3]) -> i32,
        attributes: unsafe extern "system" fn(This, *mut *mut Com) -> i32,
        input_attributes: unsafe extern "system" fn(This, u32, *mut *mut Com) -> i32,
        output_attributes: unsafe extern "system" fn(This, u32, *mut *mut Com) -> i32,
        delete_input: unsafe extern "system" fn(This, u32) -> i32,
        add_inputs: unsafe extern "system" fn(This, u32, *mut u32) -> i32,
        input_available: unsafe extern "system" fn(This, u32, u32, *mut *mut Com) -> i32,
        output_available: unsafe extern "system" fn(This, u32, u32, *mut *mut Com) -> i32,
        set_input: unsafe extern "system" fn(This, u32, *mut Com, u32) -> i32,
        set_output: unsafe extern "system" fn(This, u32, *mut Com, u32) -> i32,
        input_current: unsafe extern "system" fn(This, u32, *mut *mut Com) -> i32,
        output_current: unsafe extern "system" fn(This, u32, *mut *mut Com) -> i32,
        input_status: unsafe extern "system" fn(This, u32, *mut u32) -> i32,
        output_status: unsafe extern "system" fn(This, *mut u32) -> i32,
        output_bounds: unsafe extern "system" fn(This, i64, i64) -> i32,
        event: unsafe extern "system" fn(This, u32, *mut Com) -> i32,
        message: unsafe extern "system" fn(This, u32, usize) -> i32,
        input: unsafe extern "system" fn(This, u32, *mut Com, u32) -> i32,
        output: unsafe extern "system" fn(This, u32, u32, *mut [usize; 4], *mut u32) -> i32,
    }

    static TABLE: Table = Table {
        unknown: ListenerTable { query, add, release },
        stream_limits,
        stream_count,
        stream_ids: no_stream_ids,
        input_info,
        output_info,
        attributes: no_attributes,
        input_attributes: no_stream_attributes,
        output_attributes: no_stream_attributes,
        delete_input: no_deleting,
        add_inputs: no_adding,
        input_available,
        output_available,
        set_input,
        set_output,
        input_current,
        output_current,
        input_status,
        output_status: no_output_status,
        output_bounds: no_bounds,
        event: no_events,
        message,
        input: take_input,
        output: give_output,
    };

    /// Methods it has no use for, each answering "not implemented".
    macro_rules! refused {
        ($($name:ident($($kind:ty),*);)*) => {
            $(unsafe extern "system" fn $name(_: This $(, _: $kind)*) -> i32 {
                NOT_IMPLEMENTED
            })*
        };
    }
    refused! {
        no_stream_ids(u32, *mut u32, u32, *mut u32);
        no_attributes(*mut *mut Com);
        no_stream_attributes(u32, *mut *mut Com);
        no_deleting(u32);
        no_adding(u32, *mut u32);
        no_output_status(*mut u32);
        no_bounds(i64, i64);
        no_events(u32, *mut Com);
    }

    /// A new effect, held once by the caller.
    pub(super) fn listener() -> *mut Listener<Mutex<Hearing>> {
        let hearing = Hearing { input: null_mut(), output: null_mut(), held: null_mut(), heard: Vec::new(), rate: 0 };
        Listener::boxed(&TABLE.unknown, &A_TRANSFORM, Mutex::new(hearing))
    }

    /// What it heard last, and its rate.
    pub(super) fn heard(this: *mut Listener<Mutex<Hearing>>) -> Option<(Vec<f32>, u32)> {
        // SAFETY: the player holds a reference.
        let hearing = unsafe { (*this).told.lock().unwrap() };
        (!hearing.heard.is_empty()).then(|| (hearing.heard.clone(), hearing.rate))
    }

    fn hearing(this: This) -> std::sync::MutexGuard<'static, Hearing> {
        // SAFETY: the engine holds a reference while it calls.
        unsafe { (*this).told.lock().unwrap() }
    }

    unsafe extern "system" fn stream_limits(_: This, least_in: *mut u32, most_in: *mut u32, least_out: *mut u32, most_out: *mut u32) -> i32 {
        // SAFETY: the engine passes places for four numbers.
        unsafe { (*least_in, *most_in, *least_out, *most_out) = (1, 1, 1, 1) };
        0
    }
    unsafe extern "system" fn stream_count(_: This, inputs: *mut u32, outputs: *mut u32) -> i32 {
        // SAFETY: as above.
        unsafe { (*inputs, *outputs) = (1, 1) };
        0
    }
    unsafe extern "system" fn input_info(_: This, _: u32, info: *mut [u32; 6]) -> i32 {
        // SAFETY: `MFT_INPUT_STREAM_INFO`: no latency, whole samples, any size.
        unsafe { *info = [0, 0, 1, 0, 0, 0] };
        0
    }
    unsafe extern "system" fn output_info(_: This, _: u32, info: *mut [u32; 3]) -> i32 {
        // SAFETY: `MFT_OUTPUT_STREAM_INFO`: it gives the samples it takes (0x100), whole.
        unsafe { *info = [0x101, 0, 0] };
        0
    }
    /// The other side's type as the one available, once set.
    unsafe fn available(kind: *mut Com, nth: u32, out: *mut *mut Com, unset: i32) -> i32 {
        if nth > 0 || kind.is_null() {
            return if kind.is_null() { unset } else { NO_MORE_TYPES };
        }
        // SAFETY: a live type, handed out with a reference of its own (1: AddRef).
        unsafe {
            call!(kind, 1, unsafe extern "system" fn(*mut Com) -> u32);
            *out = kind;
        }
        0
    }
    unsafe extern "system" fn input_available(this: This, _: u32, nth: u32, out: *mut *mut Com) -> i32 {
        let output = hearing(this).output;
        if !output.is_null() {
            // SAFETY: as `available` asks.
            return unsafe { available(output, nth, out, NO_MORE_TYPES) };
        }
        let Some(subtype) = [FLOAT, PCM].get(nth as usize) else { return NO_MORE_TYPES };
        // SAFETY: a new media type, handed out, of sound in this format (24: SetGUID).
        unsafe {
            if MFCreateMediaType(out) < 0 {
                return NO_MORE_TYPES;
            }
            call!(*out, 24, unsafe extern "system" fn(*mut Com, *const [u32; 4], *const [u32; 4]) -> i32, &MAJOR_TYPE, &AUDIO);
            call!(*out, 24, unsafe extern "system" fn(*mut Com, *const [u32; 4], *const [u32; 4]) -> i32, &SUBTYPE, subtype);
        }
        0
    }
    unsafe extern "system" fn output_available(this: This, _: u32, nth: u32, out: *mut *mut Com) -> i32 {
        let input = hearing(this).input;
        // SAFETY: as `available` asks.
        unsafe { available(input, nth, out, TYPE_NOT_SET) }
    }
    unsafe extern "system" fn input_current(this: This, _: u32, out: *mut *mut Com) -> i32 {
        let input = hearing(this).input;
        // SAFETY: as `available` asks.
        unsafe { available(input, 0, out, TYPE_NOT_SET) }
    }
    unsafe extern "system" fn output_current(this: This, _: u32, out: *mut *mut Com) -> i32 {
        let output = hearing(this).output;
        // SAFETY: as `available` asks.
        unsafe { available(output, 0, out, TYPE_NOT_SET) }
    }

    /// Whether a type is 16-bit or float sound, and equal to `other` where that is set (of the
    /// type, 10: GetGUID; 35: IsEqual).
    unsafe fn fits(kind: *mut Com, other: *mut Com) -> bool {
        let mut subtype = [0u32; 4];
        let mut flags = 0u32;
        // SAFETY: live types.
        unsafe {
            call!(kind, 10, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut [u32; 4]) -> i32, &SUBTYPE, &mut subtype);
            (subtype == PCM || subtype == FLOAT) && (other.is_null() || call!(kind, 35, unsafe extern "system" fn(*mut Com, *mut Com, *mut u32) -> i32, other, &mut flags) == 0)
        }
    }
    /// Sets one side's type, unless only testing (flag 1); none clears it.
    unsafe fn set(side: &mut *mut Com, other: *mut Com, kind: *mut Com, flags: u32) -> i32 {
        // SAFETY: live types; the one kept holds a reference of its own.
        unsafe {
            if !kind.is_null() && !fits(kind, other) {
                return INVALID_TYPE;
            }
            if flags & 1 != 0 {
                return 0;
            }
            if !kind.is_null() {
                call!(kind, 1, unsafe extern "system" fn(*mut Com) -> u32);
            }
            release_all(&[*side]);
            *side = kind;
        }
        0
    }
    /// Any sound in; the output, set after, must be the same.
    unsafe extern "system" fn set_input(this: This, _: u32, kind: *mut Com, flags: u32) -> i32 {
        let mut hearing = hearing(this);
        // SAFETY: as `set` asks.
        let done = unsafe { set(&mut hearing.input, null_mut(), kind, flags) };
        if done == 0 && flags & 1 == 0 {
            // SAFETY: as above.
            unsafe { set(&mut hearing.output, null_mut(), null_mut(), 0) };
        }
        done
    }
    unsafe extern "system" fn set_output(this: This, _: u32, kind: *mut Com, flags: u32) -> i32 {
        let mut hearing = hearing(this);
        let input = hearing.input;
        if input.is_null() {
            return TYPE_NOT_SET;
        }
        // SAFETY: as `set` asks.
        unsafe { set(&mut hearing.output, input, kind, flags) }
    }
    unsafe extern "system" fn input_status(this: This, _: u32, flags: *mut u32) -> i32 {
        let free = hearing(this).held.is_null();
        // SAFETY: the engine passes a place for the flags.
        unsafe { *flags = free as u32 };
        0
    }
    /// A flush (0) drops the sample held.
    unsafe extern "system" fn message(this: This, message: u32, _: usize) -> i32 {
        let mut hearing = hearing(this);
        if message == 0 {
            // SAFETY: held once, or null.
            unsafe { release_all(&[hearing.held]) };
            hearing.held = null_mut();
        }
        0
    }
    /// Takes a sample and keeps what it holds (of the sample, 41: ConvertToContiguousBuffer;
    /// of the buffer, 3: Lock, 4: Unlock).
    unsafe extern "system" fn take_input(this: This, _: u32, sample: *mut Com, _: u32) -> i32 {
        let mut hearing = hearing(this);
        if !hearing.held.is_null() {
            return NOT_ACCEPTING;
        }
        let (mut buffer, mut bytes, mut most, mut length) = (null_mut(), null_mut::<u8>(), 0u32, 0u32);
        let (mut channels, mut rate, mut subtype) = (1u32, 0u32, [0u32; 4]);
        // SAFETY: a live sample, kept with a reference of its own; its buffer is read while
        // locked, then released; the input type is set.
        unsafe {
            call!(sample, 1, unsafe extern "system" fn(*mut Com) -> u32);
            hearing.held = sample;
            call!(hearing.input, 7, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut u32) -> i32, &CHANNELS, &mut channels);
            call!(hearing.input, 7, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut u32) -> i32, &RATE, &mut rate);
            call!(hearing.input, 10, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut [u32; 4]) -> i32, &SUBTYPE, &mut subtype);
            call!(sample, 41, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut buffer);
            if buffer.is_null() || call!(buffer, 3, unsafe extern "system" fn(*mut Com, *mut *mut u8, *mut u32, *mut u32) -> i32, &mut bytes, &mut most, &mut length) < 0 {
                release_all(&[buffer]);
                return 0;
            }
            let bytes = std::slice::from_raw_parts(bytes, length as usize);
            hearing.heard = match subtype == FLOAT {
                true => bytes.as_chunks::<4>().0.iter().step_by(channels.max(1) as usize).map(|s| f32::from_le_bytes(*s)).collect(),
                false => bytes.as_chunks::<2>().0.iter().step_by(channels.max(1) as usize).map(|s| i16::from_le_bytes(*s) as f32 / 32768.0).collect(),
            };
            hearing.rate = rate;
            call!(buffer, 4, unsafe extern "system" fn(*mut Com) -> i32);
            Com::release(buffer);
        }
        0
    }
    /// Gives the sample held back, as it was (`MFT_OUTPUT_DATA_BUFFER`: its stream, sample,
    /// status, events).
    unsafe extern "system" fn give_output(this: This, _: u32, _: u32, buffers: *mut [usize; 4], status: *mut u32) -> i32 {
        let mut hearing = hearing(this);
        if hearing.held.is_null() {
            return NEED_MORE_INPUT;
        }
        // SAFETY: the engine passes one buffer to fill and a place for the status; the
        // sample's reference goes with it.
        unsafe {
            (*buffers)[1] = hearing.held as usize;
            (*buffers)[2] = 0;
            *status = 0;
        }
        hearing.held = null_mut();
        0
    }

    impl Drop for Hearing {
        fn drop(&mut self) {
            // SAFETY: each held once, or null.
            unsafe { release_all(&[self.input, self.output, self.held]) };
        }
    }
}

#[cfg(test)]
pub(crate) mod played {
    //! For tests: what this process plays, as Windows mixes it (process loopback capture of
    //! WASAPI), at 48 kHz in floats.
    use super::*;

    const LOOPBACK_DEVICE: &str = "VAD\\Process_Loopback";
    const AN_AUDIO_CLIENT: [u32; 4] = named(0x1CB9_AD4C, 0xDBFA, 0x4C32, [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2]);
    const A_CAPTURE_CLIENT: [u32; 4] = named(0xC8AD_BD64, 0xE71E, 0x48A0, [0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17]);
    const A_COMPLETION: [u32; 4] = named(0x41D9_49AB, 0x9862, 0x444A, [0x80, 0xF6, 0xC2, 0x61, 0x33, 0x4D, 0xA5, 0xEB]);
    const AN_AGILE_OBJECT: [u32; 4] = named(0x94EA_2B94, 0xE9CC, 0x49E0, [0xC0, 0xFF, 0xEE, 0x64, 0xCA, 0x8F, 0x5B, 0x90]);
    pub(crate) const RATE: u32 = 48_000;

    #[link(name = "mmdevapi")]
    unsafe extern "system" {
        fn ActivateAudioInterfaceAsync(path: *const u16, interface: *const [u32; 4], parameters: *const Blob, handler: *mut c_void, operation: *mut *mut Com) -> i32;
    }

    /// A `PROPVARIANT` holding a `VT_BLOB`.
    #[repr(C)]
    struct Blob {
        kind: u16,
        reserved: [u16; 3],
        size: u32,
        data: *const u8,
    }

    /// `IActivateAudioInterfaceCompletionHandler`, agile; it does nothing (the result is asked
    /// for until it is there).
    #[repr(C)]
    struct Handler {
        table: &'static HandlerTable,
    }
    #[repr(C)]
    struct HandlerTable {
        query: unsafe extern "system" fn(*mut Handler, *const [u32; 4], *mut *mut c_void) -> i32,
        add: unsafe extern "system" fn(*mut Handler) -> u32,
        release: unsafe extern "system" fn(*mut Handler) -> u32,
        completed: unsafe extern "system" fn(*mut Handler, *mut Com) -> i32,
    }
    static HANDLER: Handler = Handler { table: &HandlerTable { query: handler_query, add: kept, release: kept, completed } };
    unsafe extern "system" fn handler_query(this: *mut Handler, of: *const [u32; 4], out: *mut *mut c_void) -> i32 {
        // SAFETY: COM passes a valid interface id and a place for the answer.
        unsafe {
            let ours = [AN_UNKNOWN, A_COMPLETION, AN_AGILE_OBJECT].contains(&*of);
            *out = if ours { this.cast() } else { null_mut() };
            if ours { 0 } else { 0x8000_4002_u32 as i32 }
        }
    }
    /// It is static: counting references is not needed.
    unsafe extern "system" fn kept(_: *mut Handler) -> u32 {
        1
    }
    unsafe extern "system" fn completed(_: *mut Handler, _: *mut Com) -> i32 {
        0
    }

    /// A capture of what this process plays, started.
    pub(crate) struct Capture {
        client: *mut Com,
        capture: *mut Com,
        event: isize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateEventW(attributes: *const c_void, manual: i32, initial: i32, name: *const u16) -> isize;
        fn CloseHandle(handle: usize) -> i32;
    }

    impl Capture {
        /// Starts capturing what this process plays, stereo floats at 48 kHz; none where Windows
        /// can't (before Windows 10 2004).
        pub(crate) fn start() -> Option<Capture> {
            let parameters = [1u32, std::process::id(), 0];
            let blob = Blob { kind: 65, reserved: [0; 3], size: 12, data: parameters.as_ptr().cast() };
            let path = wide(LOOPBACK_DEVICE);
            let mut operation = null_mut();
            // SAFETY: COM for this thread, of either kind.
            unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 2) };
            // SAFETY: the activation is asked for with parameters that live through the call;
            // its result is asked for until it is there (3: GetActivateResult), then released.
            let client = unsafe {
                if ActivateAudioInterfaceAsync(path.as_ptr(), &AN_AUDIO_CLIENT, &blob, (&raw const HANDLER).cast_mut().cast(), &mut operation) < 0 || operation.is_null() {
                    return None;
                }
                let (mut result, mut client) = (0i32, null_mut());
                let start = std::time::Instant::now();
                while call!(operation, 3, unsafe extern "system" fn(*mut Com, *mut i32, *mut *mut Com) -> i32, &mut result, &mut client) < 0 && start.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Com::release(operation);
                if result < 0 || client.is_null() {
                    return None;
                }
                client
            };
            // `WAVEFORMATEX`: IEEE floats, two channels, 48 kHz, 8 bytes a frame, 32 bits.
            let format: [u16; 9] = [3, 2, RATE as u16, (RATE >> 16) as u16, (RATE * 8) as u16, ((RATE * 8) >> 16) as u16, 8, 32, 0];
            let mut capture = null_mut();
            // SAFETY: the client is alive: it is initialized for loopback with events (3:
            // Initialize, 13: SetEventHandle), started (10), and its capture service asked for
            // (14: GetService).
            unsafe {
                let event = CreateEventW(null(), 0, 0, null());
                let flags = 0x0002_0000 | 0x0004_0000 | 0x8000_0000 | 0x0800_0000;
                let ready = call!(client, 3, unsafe extern "system" fn(*mut Com, u32, u32, i64, i64, *const u16, *const c_void) -> i32, 0, flags, 2_000_000, 0, format.as_ptr(), null()) >= 0
                    && call!(client, 13, unsafe extern "system" fn(*mut Com, isize) -> i32, event) >= 0
                    && call!(client, 14, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut *mut Com) -> i32, &A_CAPTURE_CLIENT, &mut capture) >= 0
                    && call!(client, 10, unsafe extern "system" fn(*mut Com) -> i32) >= 0;
                if !ready || capture.is_null() {
                    release_all(&[capture, client]);
                    CloseHandle(event as usize);
                    return None;
                }
                Some(Capture { client, capture, event })
            }
        }

        /// What was played since last asked: the first channel.
        pub(crate) fn take(&mut self) -> Vec<f32> {
            let mut taken = Vec::new();
            // SAFETY: the capture client is alive; each packet is read and released (5:
            // GetNextPacketSize, 3: GetBuffer, 4: ReleaseBuffer); silent ones (flag 2) are zeros.
            unsafe {
                let mut next = 0u32;
                while call!(self.capture, 5, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut next) >= 0 && next > 0 {
                    let (mut data, mut frames, mut flags) = (null_mut::<f32>(), 0u32, 0u32);
                    if call!(
                        self.capture,
                        3,
                        unsafe extern "system" fn(*mut Com, *mut *mut f32, *mut u32, *mut u32, *mut u64, *mut u64) -> i32,
                        &mut data,
                        &mut frames,
                        &mut flags,
                        null_mut(),
                        null_mut()
                    ) < 0
                    {
                        break;
                    }
                    match flags & 2 != 0 || data.is_null() {
                        true => taken.extend(std::iter::repeat_n(0.0, frames as usize)),
                        false => taken.extend(std::slice::from_raw_parts(data, frames as usize * 2).iter().step_by(2)),
                    }
                    call!(self.capture, 4, unsafe extern "system" fn(*mut Com, u32) -> i32, frames);
                }
            }
            taken
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            // SAFETY: stopped (11) and released; the event closed.
            unsafe {
                call!(self.client, 11, unsafe extern "system" fn(*mut Com) -> i32);
                release_all(&[self.capture, self.client]);
                CloseHandle(self.event as usize);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_whose_engine_was_never_made_drops_cleanly() {
        let Ok(player) = Player::started() else { return eprintln!("no Media Foundation here: skipped") };
        drop(player);
    }

    /// The player's newest frame left on the GPU, asked for until one comes (five seconds at
    /// most).
    #[cfg(feature = "gpu-draw")]
    fn next_texture(player: &mut Player) -> Option<Arc<FrameTexture>> {
        let asked = std::time::Instant::now();
        while asked.elapsed() < Duration::from_secs(5) {
            if let Some((texture, _)) = player.frame_texture() {
                return Some(texture);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// While a window's device holds a frame's texture (its keyed mutex, taken on a device of
    /// its own), the decoder never waits for it: no try costs the UI thread a wait, and the
    /// frame held back is put in once the texture is given back. Prints the longest try.
    #[cfg(feature = "gpu-draw")]
    #[test]
    fn the_decoder_never_waits_for_a_texture_a_window_holds() {
        use crate::gpu_frames_windows::{give_back, opened_on_direct3d_11, take_all};
        let Some(file) = crate::video::tests::test_file(false) else { return eprintln!("no test file here: skipped") };
        let Ok(mut player) = Player::open(VideoSource::from(file.as_path()), false) else { return eprintln!("no Media Foundation here: skipped") };
        if !player.decodes_on_gpu() {
            return eprintln!("no GPU decodes here: skipped");
        }
        player.play();
        let texture = next_texture(&mut player).expect("a first frame");
        let window = make_gpu(player.functions).expect("a device of the window's");
        let Some([held, mutex]) = opened_on_direct3d_11(window.device.cast(), &texture) else { return eprintln!("the frame's texture is not shared here: skipped") };
        assert!(take_all(&[mutex]), "the window's device holds the texture");
        let (mut longest, holding) = (Duration::ZERO, std::time::Instant::now());
        while holding.elapsed() < Duration::from_millis(500) {
            let tried = std::time::Instant::now();
            assert!(player.frame_texture().is_none(), "a frame was put in while a window held the texture");
            longest = longest.max(tried.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        give_back(&[mutex]);
        assert!(next_texture(&mut player).is_some(), "no frame came once the texture was given back");
        // SAFETY: each opened above, held once by the test.
        unsafe { release_all(&[mutex.cast(), held.cast()]) };
        drop(window);
        println!("the longest try while a window held the texture: {longest:?}");
        assert!(longest < Duration::from_millis(FRAME_WAIT.into()), "a try waited {longest:?} for the texture (a wait for it takes FRAME_WAIT)");
    }
}
