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
use crate::video::{CaptionStyle, News, Track, Tracks, VideoError, VideoSource};
use crate::video_captions::Cue;
use crate::windows::{Com, function_in, named, system_library, wide};
use std::ffi::{CStr, c_void};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[link(name = "ole32")]
unsafe extern "system" {
    fn PropVariantClear(value: *mut PropVariant) -> i32;
    fn CoTaskMemFree(memory: *mut c_void);
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

/// The functions of Media Foundation and its helpers that are not methods.
struct Functions {
    startup: unsafe extern "system" fn(u32, u32) -> i32,
    attributes: unsafe extern "system" fn(*mut *mut Com, u32) -> i32,
    on_stream: unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32,
    device_manager: unsafe extern "system" fn(*mut u32, *mut *mut Com) -> i32,
    memory_stream: unsafe extern "system" fn(*const u8, u32) -> *mut Com,
    text: unsafe extern "system" fn(*const u16) -> *mut u16,
    free_text: unsafe extern "system" fn(*mut u16),
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

fn load() -> Option<Functions> {
    let (mf, ole, shell, d3d11) = (system_library("mfplat.dll"), system_library("oleaut32.dll"), system_library("shlwapi.dll"), system_library("d3d11.dll"));
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

// ---- the GPU the engines decode on ----

/// The Direct3D 11 device videos' engines decode with, its immediate context, and the DXGI
/// device manager that hands it to Media Foundation. One for the program, made again once lost.
struct Gpu {
    device: *mut Com,
    context: *mut Com,
    manager: *mut Com,
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
        unsafe { release_all(&[self.manager, self.context, self.device]) };
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
    Some(Gpu { device, context, manager })
}

/// `QueryInterface`.
type Ask = unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut *mut Com) -> i32;

// ---- what the engine tells, from its own threads ----

/// A COM object of ours that the engine tells things to: its vtable, the one interface it has
/// (besides `IUnknown`), its references, and what it was told.
#[repr(C)]
struct Listener<S: 'static> {
    table: &'static ListenerTable<S>,
    interface: &'static [u32; 4],
    references: AtomicU32,
    told: S,
}

/// `IUnknown`'s three methods, then the interface's own, of any shape (each table below is
/// one of these with them after).
#[repr(C)]
struct ListenerTable<S: 'static> {
    query: unsafe extern "system" fn(*mut Listener<S>, *const [u32; 4], *mut *mut c_void) -> i32,
    add: unsafe extern "system" fn(*mut Listener<S>) -> u32,
    release: unsafe extern "system" fn(*mut Listener<S>) -> u32,
}

impl<S> Listener<S> {
    /// A new one, held once by the caller.
    fn boxed(table: &'static ListenerTable<S>, interface: &'static [u32; 4], told: S) -> *mut Listener<S> {
        Box::into_raw(Box::new(Listener { table, interface, references: AtomicU32::new(1), told }))
    }
}

unsafe extern "system" fn query<S>(this: *mut Listener<S>, of: *const [u32; 4], out: *mut *mut c_void) -> i32 {
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

unsafe extern "system" fn add<S>(this: *mut Listener<S>) -> u32 {
    // SAFETY: the caller holds a reference, so it is alive.
    unsafe { (*this).references.fetch_add(1, Ordering::AcqRel) + 1 }
}

unsafe extern "system" fn release<S>(this: *mut Listener<S>) -> u32 {
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
/// and track's; and whether its tracks changed since asked (captions in the video stream,
/// CEA-608, are a track added once their first data plays).
struct Showing {
    cues: Mutex<Vec<(u32, u32, Cue)>>,
    tracks_changed: AtomicBool,
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

static SHOWING: ShowingTable = ShowingTable {
    unknown: ListenerTable { query, add, release },
    track_added: tracks_changed,
    track_removed: tracks_changed,
    track_selected: ignore_selected,
    track_ready: ignore_track,
    error: ignore_error,
    cue,
    reset,
};

unsafe extern "system" fn ignore_track(_: *mut Listener<Showing>, _: u32) {}
unsafe extern "system" fn ignore_selected(_: *mut Listener<Showing>, _: u32, _: i32) {}
unsafe extern "system" fn ignore_error(_: *mut Listener<Showing>, _: u32, _: i32, _: u32) {}

unsafe extern "system" fn tracks_changed(this: *mut Listener<Showing>, _: u32) {
    // SAFETY: the timed text holds a reference while it calls.
    unsafe { (*this).told.tracks_changed.store(true, Ordering::Release) };
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
    /// The source's metadata was read: its tracks are known.
    loaded: bool,
    /// The engine's streams that are sound tracks, in order.
    sound_streams: Vec<u32>,
    /// The engine's timed text (`IMFTimedText`), its caption tracks' ids, and the one chosen.
    timed_text: *mut Com,
    caption_ids: Vec<u32>,
    captions: Option<usize>,
    /// (Tests: the effect that hears what plays, or null.)
    #[cfg(test)]
    hearing: *mut Listener<Mutex<hearing::Hearing>>,
}

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
            showing: Listener::boxed(&SHOWING.unknown, &A_CUE_NOTIFY, Showing { cues: Mutex::new(Vec::new()), tracks_changed: AtomicBool::new(false) }),
            frames: Frames::Bitmap { wic: null_mut(), bitmap: null_mut(), size: (0, 0) },
            last: None,
            loaded: false,
            sound_streams: Vec::new(),
            timed_text: null_mut(),
            caption_ids: Vec::new(),
            captions: None,
            #[cfg(test)]
            hearing: null_mut(),
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

    /// Gives the engine its source: a path or address as text (6: SetSource), bytes as a
    /// stream (45: `IMFMediaEngineEx::SetSourceFromByteStream`), named for their kind.
    fn set_source(&mut self, source: VideoSource) -> Result<(), VideoError> {
        let (address, bytes) = match source {
            VideoSource::File(path) => (std::path::absolute(&path).unwrap_or(path).display().to_string(), None),
            VideoSource::Url(url) => (url, None),
            VideoSource::Bytes(bytes) => (format!("memory.{}", crate::video::kind_of(&bytes)), Some(bytes)),
        };
        let address = wide(&address);
        // SAFETY: the text is made and freed around the call; the stream holds a copy of the
        // bytes, and the engine its own reference to it.
        let done = unsafe {
            let text = (self.functions.text)(address.as_ptr());
            let done = match bytes {
                None => call!(self.engine, 6, unsafe extern "system" fn(*mut Com, *mut u16) -> i32, text),
                Some(bytes) => self.set_bytes(&bytes, text),
            };
            (self.functions.free_text)(text);
            done
        };
        match done < 0 {
            true => Err(VideoError::Refused(format!("the source was refused ({:#010x})", done as u32))),
            false => Ok(()),
        }
    }

    /// # Safety
    /// `name` is a `BSTR`.
    unsafe fn set_bytes(&mut self, bytes: &[u8], name: *mut u16) -> i32 {
        let mut stream = null_mut();
        // SAFETY: a stream is made over a copy of the bytes, and released after the engine
        // took it.
        unsafe {
            let memory = (self.functions.memory_stream)(bytes.as_ptr(), bytes.len() as u32);
            if memory.is_null() {
                return -1;
            }
            let made = (self.functions.on_stream)(memory, &mut stream);
            let done = match made < 0 || stream.is_null() {
                true => -1,
                false => call!(self.ex, 45, unsafe extern "system" fn(*mut Com, *mut Com, *mut u16) -> i32, stream, name),
            };
            release_all(&[stream, memory]);
            done
        }
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
        self.last = None;
        self.set(17, to.as_secs_f64());
    }
    pub(crate) fn position(&mut self) -> Duration {
        Duration::try_from_secs_f64(self.call::<f64>(16)).unwrap_or_default()
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

    pub(crate) fn news(&mut self) -> News {
        // SAFETY: ours is one of its references.
        let told = unsafe { &(*self.said).told };
        let (said, code) = (told.bits.swap(0, Ordering::AcqRel), told.code.load(Ordering::Acquire));
        self.loaded |= said & LOADED != 0;
        let error = (said & FAILED != 0).then(|| format!("the player stopped: {} ({:#010x})", self.error_words(), code as u32));
        // SAFETY: as above.
        let tracks_changed = unsafe { (*self.showing).told.tracks_changed.swap(false, Ordering::AcqRel) };
        News { ended: said & ENDED != 0, error, clears_on_play: false, tracks_changed }
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
        // SAFETY: the engine is alive, and writes the time and size.
        unsafe {
            let tick = call!(self.engine, 44, unsafe extern "system" fn(*mut Com, *mut i64) -> i32, &mut at);
            if tick != 0 || self.last == Some(at) {
                return None;
            }
            call!(self.engine, 40, unsafe extern "system" fn(*mut Com, *mut u32, *mut u32) -> i32, &mut width, &mut height);
        }
        let whole = [0i32, 0, width as i32, height as i32];
        if width == 0 || height == 0 || !self.frames.sized(width, height) || !self.frames.transfer(self.engine, &whole) {
            return None;
        }
        self.last = Some(at);
        Some((Duration::from_nanos(at.max(0) as u64 * 100), width as usize))
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
    fn caption_tracks(&self) -> (Vec<u32>, Vec<Track>) {
        let mut list = null_mut();
        // SAFETY: the timed text is alive; it gives a list of its own.
        let listed = !self.timed_text.is_null() && unsafe { call!(self.timed_text, 13, unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32, &mut list) } >= 0;
        if !listed || list.is_null() {
            return (Vec::new(), Vec::new());
        }
        // SAFETY: a live list.
        let count = unsafe { call!(list, 3, unsafe extern "system" fn(*mut Com) -> u32) };
        let found = (0..count).filter_map(|at| caption_track(list, at)).unzip();
        // SAFETY: held once.
        unsafe { Com::release(list) };
        found
    }

    /// Shows the `nth` caption track (of the timed text, 4: SelectTrack), or none.
    pub(crate) fn choose_captions(&mut self, nth: Option<usize>) {
        if let Some(id) = self.captions.and_then(|at| self.caption_ids.get(at)) {
            // SAFETY: the timed text is alive, and the id one of its tracks'.
            unsafe { call!(self.timed_text, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, *id, 0) };
        }
        self.captions = nth.filter(|at| *at < self.caption_ids.len());
        if let Some(at) = self.captions {
            // SAFETY: as above.
            unsafe { call!(self.timed_text, 4, unsafe extern "system" fn(*mut Com, u32, i32) -> i32, self.caption_ids[at], 1) };
        }
    }

    /// The chosen caption track's text on show now: its cues the timed text says are on show.
    pub(crate) fn caption(&mut self) -> Option<String> {
        let id = *self.caption_ids.get(self.captions?)?;
        // SAFETY: ours is one of its references.
        let shown = unsafe { (*self.showing).told.cues() };
        let mut cues: Vec<&Cue> = shown.iter().filter(|(_, track, _)| *track == id).map(|(_, _, cue)| cue).collect();
        cues.sort_by_key(|cue| cue.from);
        let first = cues.first()?.text.clone();
        Some(cues[1..].iter().fold(first, |all, cue| all + "\n" + &cue.text))
    }
}

/// The `at`th track of a timed text's track list, unless it holds metadata (kind 3): its id, and
/// what it is (of the list, 4: GetTrack; of the track, 3: GetId, 4: GetLabel, 6: GetLanguage,
/// 7: GetTrackKind).
fn caption_track(list: *mut Com, at: u32) -> Option<(u32, Track)> {
    let mut track = null_mut();
    // SAFETY: a live list; the track it gives is read and released.
    unsafe {
        if call!(list, 4, unsafe extern "system" fn(*mut Com, u32, *mut *mut Com) -> i32, at, &mut track) < 0 || track.is_null() {
            return None;
        }
        let captions = call!(track, 7, unsafe extern "system" fn(*mut Com) -> u32) != 3;
        let found = captions.then(|| (call!(track, 3, unsafe extern "system" fn(*mut Com) -> u32), Track { language: owned_text(track, 6), name: owned_text(track, 4) }));
        Com::release(track);
        found
    }
}

/// A cue's times and lines (6: GetStartTime, 7: GetDuration, 12: GetLineCount, 13: GetLine;
/// of a line, 3: GetText).
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
    Cue { from, to: from + length, text: crate::video_captions::plain(&lines.join("\n")) }
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
    /// TransferVideoFrame): whether it did. A texture whose keyed mutex was abandoned (a
    /// device lost while holding it) is dropped, to be made again.
    fn transfer(&mut self, engine: *mut Com, whole: &[i32; 4]) -> bool {
        // SAFETY: the engine is alive; the target is a live texture or bitmap of that size,
        // the texture held by this device while drawn into.
        let sent = unsafe {
            match self {
                Frames::Gpu { texture: Some(texture), .. } => texture.held(|| call!(engine, 43, Transfer, texture.drawn, null(), whole, null()) >= 0),
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
/// a keyed mutex orders the devices' work on it.
pub(crate) struct FrameTexture {
    gpu: Arc<Gpu>,
    drawn: *mut Com,
    readable: *mut Com,
    /// Its keyed mutex (`IDXGIKeyedMutex`), and the handle other devices open it by: null and 0
    /// where the device could not share it.
    #[cfg(feature = "gpu-draw")]
    mutex: *mut Com,
    #[cfg(feature = "gpu-draw")]
    pub(crate) handle: isize,
    /// Its width and height.
    pub(crate) size: (u32, u32),
}

// SAFETY: its device is multithread protected (see `Gpu`); the textures are used under its
// lock, the shared one under its keyed mutex.
unsafe impl Send for FrameTexture {}
// SAFETY: as above.
unsafe impl Sync for FrameTexture {}

/// How long a device waits for another's work on a frame's texture, in ms (`AcquireSync`), and
/// what it gives when that passes.
#[cfg(feature = "gpu-draw")]
pub(crate) const FRAME_WAIT: u32 = 100;
const WAIT_TIMEOUT: i32 = 0x102;

/// `IDXGIKeyedMutex` and `IDXGIResource`.
#[cfg(feature = "gpu-draw")]
pub(crate) const A_KEYED_MUTEX: [u32; 4] = named(0x9D8E_1289, 0xD7B3, 0x465F, [0x81, 0x26, 0x25, 0x0E, 0x34, 0x9A, 0xF8, 0x5D]);
#[cfg(feature = "gpu-draw")]
const A_DXGI_RESOURCE: [u32; 4] = named(0x035F_3AB4, 0x482E, 0x4E50, [0xB4, 0x1F, 0x8A, 0x7F, 0x8B, 0xD8, 0x96, 0x0B]);

impl FrameTexture {
    /// The two textures, this size, on `gpu`; none where they can't be made. (0x28: a render
    /// and shader target, with `gpu-draw` shared with a keyed mutex, 0x100, where the device
    /// can; 3, 0x20000: staging, read by the CPU.)
    fn new(gpu: &Arc<Gpu>, (width, height): (u32, u32)) -> Option<FrameTexture> {
        let shared = if cfg!(feature = "gpu-draw") { 0x100 } else { 0 };
        // SAFETY: the device is alive.
        let (mut drawn, readable) = unsafe { (texture(gpu, width, height, [0, 0x28, 0, shared]), texture(gpu, width, height, [3, 0, 0x20000, 0])) };
        if drawn.is_null() {
            // SAFETY: as above.
            drawn = unsafe { texture(gpu, width, height, [0, 0x28, 0, 0]) };
        }
        #[cfg(not(feature = "gpu-draw"))]
        let made = FrameTexture { gpu: gpu.clone(), drawn, readable, size: (width, height) };
        #[cfg(feature = "gpu-draw")]
        let made = FrameTexture { gpu: gpu.clone(), drawn, readable, mutex: null_mut(), handle: 0, size: (width, height) }.shared();
        (!made.drawn.is_null() && !made.readable.is_null()).then_some(made)
    }

    /// The same with its keyed mutex and shared handle (of `IDXGIResource`, 8: GetSharedHandle),
    /// where it is shared: else windows' devices can't open it, and its frames are read back.
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
                call!(resource, 8, unsafe extern "system" fn(*mut Com, *mut isize) -> i32, &mut self.handle);
                Com::release(resource);
            }
        }
        self
    }

    /// Does `work` on the texture drawn into, holding its keyed mutex (8: AcquireSync, 9:
    /// ReleaseSync): other devices' work on it is done first, and theirs waits for this. Not
    /// where the mutex can't be taken within `FRAME_WAIT`: what `AcquireSync` gave instead.
    fn held<R>(&self, work: impl FnOnce() -> R) -> Result<R, i32> {
        #[cfg(feature = "gpu-draw")]
        if !self.mutex.is_null() {
            // SAFETY: the mutex is alive; key 0 is the one every device takes (`S_OK` only: a
            // time-out and an abandoned mutex are success codes).
            let taken = unsafe { call!(self.mutex, 8, unsafe extern "system" fn(*mut Com, u64, u32) -> i32, 0, FRAME_WAIT) };
            if taken != 0 {
                return Err(taken);
            }
            let done = work();
            // SAFETY: taken above.
            unsafe { call!(self.mutex, 9, unsafe extern "system" fn(*mut Com, u64) -> i32, 0) };
            return Ok(done);
        }
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
            self.held(|| call!(self.gpu.context, 47, unsafe extern "system" fn(*mut Com, *mut Com, *mut Com), self.readable, self.drawn)).is_ok()
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
        READ_BACK.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// (Tests: frames read back from the GPU so far.)
#[cfg(test)]
pub(crate) static READ_BACK: AtomicU32 = AtomicU32::new(0);

impl Drop for FrameTexture {
    fn drop(&mut self) {
        // SAFETY: each is held once, or null (the handle goes with the texture).
        unsafe {
            #[cfg(feature = "gpu-draw")]
            release_all(&[self.mutex]);
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
}

#[cfg(test)]
mod hearing {
    //! For tests: an audio effect the engine puts before its sound renderer (`IMFTransform`,
    //! passing each sample on as it is), which keeps what it heard last. The engine plays muted:
    //! nothing reaches the speakers.
    use super::*;

    /// `IMFTransform`; a media type's `MF_MT_SUBTYPE`, `MF_MT_AUDIO_NUM_CHANNELS`,
    /// `MF_MT_AUDIO_SAMPLES_PER_SECOND`; `MFAudioFormat_PCM` and `MFAudioFormat_Float`.
    const A_TRANSFORM: [u32; 4] = named(0xBF94_C121, 0x5B05, 0x4E6F, [0x80, 0x00, 0xBA, 0x59, 0x89, 0x61, 0x41, 0x4D]);
    const SUBTYPE: [u32; 4] = named(0xF7E3_4C9A, 0x42E8, 0x4714, [0xB7, 0x4B, 0xCB, 0x29, 0xD7, 0x2C, 0x35, 0xE5]);
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
}
