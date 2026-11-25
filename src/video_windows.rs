//! Videos on Windows: Media Foundation's Media Engine, loaded at run time, in frame-server mode
//! (no window of its own). Each new frame is copied into a WIC bitmap and read from there.
//! Functions are called by vtable index, as the SDK's headers number them.

use crate::video::{News, VideoError, VideoSource};
use crate::windows::{Com, named, wide};
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn GetProcAddress(library: isize, name: *const u8) -> usize;
}

const ENGINE_FACTORY: [u32; 4] = named(0xB443_92DA, 0x499B, 0x446B, [0xA4, 0xCB, 0x00, 0x5F, 0xEA, 0xD0, 0xE6, 0xD5]);
const AN_ENGINE_FACTORY: [u32; 4] = named(0x4D64_5ACE, 0x26AA, 0x4688, [0x9B, 0xE1, 0xDF, 0x35, 0x16, 0x99, 0x0B, 0x93]);
const AN_ENGINE_EX: [u32; 4] = named(0x8301_5EAD, 0xB1E6, 0x40D0, [0xA9, 0x8A, 0x37, 0x14, 0x5F, 0xFE, 0x1A, 0xD1]);
const A_NOTIFY: [u32; 4] = named(0xFEE7_C112, 0xE776, 0x42B5, [0x9B, 0xBF, 0x00, 0x48, 0x52, 0x4E, 0x2B, 0xD5]);
const AN_UNKNOWN: [u32; 4] = named(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
/// `MF_MEDIA_ENGINE_CALLBACK` and `MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT`.
const CALLBACK: [u32; 4] = named(0xC603_81B8, 0x83A4, 0x41F8, [0xA3, 0xD0, 0xDE, 0x05, 0x07, 0x68, 0x49, 0xA9]);
const OUTPUT_FORMAT: [u32; 4] = named(0x5066_893C, 0x8CF9, 0x42BC, [0x8B, 0x8A, 0x47, 0x22, 0x12, 0xE5, 0x27, 0x26]);
const WIC_FACTORY: [u32; 4] = named(0xCACA_F262, 0x9370, 0x4615, [0xA1, 0x3B, 0x9F, 0x55, 0x39, 0xDA, 0x4C, 0x0A]);
const A_WIC_FACTORY: [u32; 4] = named(0xEC5E_C8A9, 0xC395, 0x4314, [0x9C, 0x77, 0x54, 0xD7, 0xA9, 0x35, 0xFF, 0x70]);
const BGRA: [u32; 4] = named(0x6FDD_C324, 0x4E03, 0x4BFE, [0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9, 0x0F]);

/// The functions of Media Foundation and its helpers that are not methods.
struct Functions {
    startup: unsafe extern "system" fn(u32, u32) -> i32,
    shutdown: unsafe extern "system" fn() -> i32,
    attributes: unsafe extern "system" fn(*mut *mut Com, u32) -> i32,
    on_stream: unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32,
    memory_stream: unsafe extern "system" fn(*const u8, u32) -> *mut Com,
    text: unsafe extern "system" fn(*const u16) -> *mut u16,
    free_text: unsafe extern "system" fn(*mut u16),
}

/// The functions, loaded once from the system folder; none where Media Foundation is missing
/// (Windows N editions without the Media Feature Pack).
fn functions() -> Option<&'static Functions> {
    static LOADED: OnceLock<Option<Functions>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref()
}

fn load() -> Option<Functions> {
    let library = |name: &str| {
        let name = wide(name);
        // SAFETY: a name ending in a zero; 0x800 searches only the system folder.
        unsafe { LoadLibraryExW(name.as_ptr(), 0, 0x800) }
    };
    let (mf, ole, shell) = (library("mfplat.dll"), library("oleaut32.dll"), library("shlwapi.dll"));
    let found = |library: isize, name: &[u8]| {
        // SAFETY: a name ending in a zero, of a loaded library (or 0, which finds nothing).
        let at = unsafe { GetProcAddress(library, name.as_ptr()) };
        (library != 0 && at != 0).then_some(at)
    };
    // SAFETY: each is the function of that name, of the type its header gives.
    unsafe {
        Some(Functions {
            startup: function(found(mf, b"MFStartup\0")?),
            shutdown: function(found(mf, b"MFShutdown\0")?),
            attributes: function(found(mf, b"MFCreateAttributes\0")?),
            on_stream: function(found(mf, b"MFCreateMFByteStreamOnStream\0")?),
            memory_stream: function(found(shell, b"SHCreateMemStream\0")?),
            text: function(found(ole, b"SysAllocString\0")?),
            free_text: function(found(ole, b"SysFreeString\0")?),
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

/// What the engine said, as bits.
const LOADED: u32 = 1;
const ENDED: u32 = 2;
const FAILED: u32 = 4;

/// The object the engine tells its events to (`IMFMediaEngineNotify`), from its own threads.
#[repr(C)]
struct Notify {
    table: &'static NotifyTable,
    references: AtomicU32,
    said: AtomicU32,
    /// The `HRESULT` of the last error.
    code: AtomicI32,
}

#[repr(C)]
struct NotifyTable {
    query: unsafe extern "system" fn(*mut Notify, *const [u32; 4], *mut *mut c_void) -> i32,
    add: unsafe extern "system" fn(*mut Notify) -> u32,
    release: unsafe extern "system" fn(*mut Notify) -> u32,
    event: unsafe extern "system" fn(*mut Notify, u32, usize, u32) -> i32,
}

static NOTIFY: NotifyTable = NotifyTable { query, add, release, event };

unsafe extern "system" fn query(this: *mut Notify, of: *const [u32; 4], out: *mut *mut c_void) -> i32 {
    // SAFETY: COM passes a valid interface id and place for the answer.
    unsafe {
        let ours = *of == AN_UNKNOWN || *of == A_NOTIFY;
        *out = if ours { this.cast() } else { null_mut() };
        if !ours {
            return 0x8000_4002_u32 as i32;
        }
        add(this);
    }
    0
}

unsafe extern "system" fn add(this: *mut Notify) -> u32 {
    // SAFETY: the caller holds a reference, so it is alive.
    unsafe { (*this).references.fetch_add(1, Ordering::AcqRel) + 1 }
}

unsafe extern "system" fn release(this: *mut Notify) -> u32 {
    // SAFETY: the caller holds a reference; the last one frees it, as it was boxed.
    unsafe {
        let left = (*this).references.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(this));
        }
        left
    }
}

unsafe extern "system" fn event(this: *mut Notify, event: u32, _: usize, code: u32) -> i32 {
    // (10: metadata loaded, 19: ended, 5: an error, with its HRESULT second.)
    let bit = match event {
        10 => LOADED,
        19 => ENDED,
        5 => FAILED,
        _ => return 0,
    };
    // SAFETY: the engine holds a reference while it calls.
    unsafe {
        if bit == FAILED {
            (*this).code.store(code as i32, Ordering::Release);
        }
        (*this).said.fetch_or(bit, Ordering::AcqRel);
    }
    0
}

/// A Media Engine playing one source, and the bitmap its frames are copied into.
pub(crate) struct Player {
    functions: &'static Functions,
    engine: *mut Com,
    notify: *mut Notify,
    wic: *mut Com,
    bitmap: *mut Com,
    bitmap_size: (u32, u32),
    /// The last frame's time, in 100 ns.
    last: Option<i64>,
}

impl Player {
    /// Opens the source in a new engine; with no sound, muted.
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let functions = functions().ok_or_else(|| VideoError::NoPlayer("Media Foundation is not installed".into()))?;
        // SAFETY: COM and Media Foundation are started for this thread (single-threaded, as
        // OLE wants it; a thread already started either way keeps its kind).
        let started = unsafe {
            crate::windows::sys::CoInitializeEx(null_mut(), 2);
            (functions.startup)(0x0002_0070, 0)
        };
        if started < 0 {
            return Err(VideoError::NoPlayer(format!("Media Foundation did not start ({:#010x})", started as u32)));
        }
        let notify = Box::into_raw(Box::new(Notify { table: &NOTIFY, references: AtomicU32::new(1), said: AtomicU32::new(0), code: AtomicI32::new(0) }));
        let mut player = Player { functions, engine: null_mut(), notify, wic: null_mut(), bitmap: null_mut(), bitmap_size: (0, 0), last: None };
        player.engine = player.make_engine(sound)?;
        player.wic = make_wic().ok_or_else(|| VideoError::NoPlayer("no Windows Imaging Component".into()))?;
        player.set_source(source)?;
        Ok(player)
    }

    /// A new engine telling `notify`, giving frames as 32-bit BGRA.
    fn make_engine(&mut self, sound: bool) -> Result<*mut Com, VideoError> {
        let (mut attributes, mut factory, mut engine) = (null_mut(), null_mut(), null_mut());
        // SAFETY: attributes are made and set (27: SetUnknown, 21: SetUINT32), the factory is
        // made and asked for an engine (3: CreateInstance; 4 forces mute), and both released.
        unsafe {
            if (self.functions.attributes)(&mut attributes, 2) < 0 || attributes.is_null() {
                return Err(VideoError::NoPlayer("no attributes".into()));
            }
            Com::function::<unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut Notify) -> i32>(attributes, 27)(attributes, &CALLBACK, self.notify);
            Com::function::<unsafe extern "system" fn(*mut Com, *const [u32; 4], u32) -> i32>(attributes, 21)(
                attributes,
                &OUTPUT_FORMAT,
                std::env::var("TAUT_FMT").ok().and_then(|v| v.parse().ok()).unwrap_or(87),
            );
            let made = crate::windows::sys::CoCreateInstance(&ENGINE_FACTORY, null_mut(), 1, &AN_ENGINE_FACTORY, &mut factory);
            if made >= 0 && !factory.is_null() {
                Com::function::<unsafe extern "system" fn(*mut Com, u32, *mut Com, *mut *mut Com) -> i32>(factory, 3)(factory, if sound { 0 } else { 4 }, attributes, &mut engine);
                Com::release(factory);
            }
            Com::release(attributes);
        }
        match engine.is_null() {
            true => Err(VideoError::NoPlayer("no Media Engine".into())),
            false => Ok(engine),
        }
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
                None => Com::function::<unsafe extern "system" fn(*mut Com, *mut u16) -> i32>(self.engine, 6)(self.engine, text),
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
        let (mut ex, mut stream) = (null_mut::<Com>(), null_mut());
        // SAFETY: the engine is asked for its extended interface (0), a stream is made over a
        // copy of the bytes, and both released after the engine took them.
        unsafe {
            let memory = (self.functions.memory_stream)(bytes.as_ptr(), bytes.len() as u32);
            if memory.is_null() {
                return -1;
            }
            Com::function::<unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut *mut Com) -> i32>(self.engine, 0)(self.engine, &AN_ENGINE_EX, &mut ex);
            let made = (self.functions.on_stream)(memory, &mut stream);
            let done = match ex.is_null() || made < 0 || stream.is_null() {
                true => -1,
                false => Com::function::<unsafe extern "system" fn(*mut Com, *mut Com, *mut u16) -> i32>(ex, 45)(ex, stream, name),
            };
            release_all(&[ex, stream, memory]);
            done
        }
    }

    /// Calls a method that takes nothing more and returns `R`.
    fn call<R>(&self, at: usize) -> R {
        // SAFETY: the engine is alive, and its method at that index takes only itself.
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com) -> R>(self.engine, at)(self.engine) }
    }

    pub(crate) fn play(&mut self) {
        self.call::<i32>(32);
    }
    pub(crate) fn pause(&mut self) {
        self.call::<i32>(33);
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        self.last = None;
        // SAFETY: the engine is alive (17: SetCurrentTime, in seconds).
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com, f64) -> i32>(self.engine, 17)(self.engine, to.as_secs_f64()) };
    }
    pub(crate) fn position(&mut self) -> Duration {
        Duration::try_from_secs_f64(self.call::<f64>(16)).unwrap_or_default()
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        // (Not a number until known; infinite for a live stream.)
        Duration::try_from_secs_f64(self.call::<f64>(19)).ok()
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        // SAFETY: the engine is alive (37: SetVolume).
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com, f64) -> i32>(self.engine, 37)(self.engine, volume as f64) };
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        // SAFETY: the engine is alive (35: SetMuted).
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com, i32) -> i32>(self.engine, 35)(self.engine, muted as i32) };
    }

    pub(crate) fn news(&mut self) -> News {
        // SAFETY: ours is one of its references.
        let (said, code) = unsafe { ((*self.notify).said.swap(0, Ordering::AcqRel), (*self.notify).code.load(Ordering::Acquire)) };
        let error = (said & FAILED != 0).then(|| format!("the player stopped: {} ({:#010x})", self.error_words(), code as u32));
        News { ended: said & ENDED != 0, error, clears_on_play: false }
    }
    /// The engine's error as words (3: GetError; of the error, 3: GetErrorCode).
    fn error_words(&self) -> &'static str {
        let mut error = null_mut();
        // SAFETY: the engine is alive; its error is read and released.
        let code = unsafe {
            Com::function::<unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32>(self.engine, 3)(self.engine, &mut error);
            if error.is_null() {
                return "an error";
            }
            let code = Com::function::<unsafe extern "system" fn(*mut Com) -> u16>(error, 3)(error);
            Com::release(error);
            code
        };
        ["an error", "aborted", "a network error", "a decoding error", "a format it can't play", "encrypted"][(code as usize).min(5)]
    }

    /// The newest frame where there is one not given yet: its width and time, its pixels in
    /// `into`. (44: OnVideoStreamTick, 40: GetNativeVideoSize, 43: TransferVideoFrame.)
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let (mut at, mut width, mut height) = (0i64, 0u32, 0u32);
        // SAFETY: the engine is alive, and writes the time and size.
        unsafe {
            let tick = Com::function::<unsafe extern "system" fn(*mut Com, *mut i64) -> i32>(self.engine, 44)(self.engine, &mut at);
            if tick != 0 || self.last == Some(at) {
                return None;
            }
            Com::function::<unsafe extern "system" fn(*mut Com, *mut u32, *mut u32) -> i32>(self.engine, 40)(self.engine, &mut width, &mut height);
        }
        if width == 0 || height == 0 || !self.bitmap_of(width, height) {
            return None;
        }
        let whole = [0i32, 0, width as i32, height as i32];
        into.resize(width as usize * height as usize, 0);
        // SAFETY: the bitmap is that size; the frame is copied into it, then its pixels into
        // `into`, which holds them all (43: TransferVideoFrame; of the bitmap, 7: CopyPixels).
        let copied = unsafe {
            let null = std::ptr::null();
            let sent = Com::function::<Transfer>(self.engine, 43)(self.engine, self.bitmap, null, &whole, null);
            sent >= 0 && Com::function::<CopyPixels>(self.bitmap, 7)(self.bitmap, null, width * 4, width * height * 4, into.as_mut_ptr()) >= 0
        };
        if !copied {
            return None;
        }
        into.iter_mut().for_each(|pixel| *pixel |= 0xFF00_0000);
        self.last = Some(at);
        Some((width as usize, Duration::from_nanos(at.max(0) as u64 * 100)))
    }
    /// Makes the bitmap this size where it is not (17: `IWICImagingFactory::CreateBitmap`).
    fn bitmap_of(&mut self, width: u32, height: u32) -> bool {
        if self.bitmap_size == (width, height) && !self.bitmap.is_null() {
            return true;
        }
        // SAFETY: the old bitmap is released, a new one made by the factory (cached on load).
        unsafe {
            if !self.bitmap.is_null() {
                Com::release(self.bitmap);
            }
            self.bitmap = null_mut();
            Com::function::<unsafe extern "system" fn(*mut Com, u32, u32, *const [u32; 4], u32, *mut *mut Com) -> i32>(self.wic, 17)(self.wic, width, height, &BGRA, 1, &mut self.bitmap);
        }
        self.bitmap_size = (width, height);
        !self.bitmap.is_null()
    }
}

/// `TransferVideoFrame` and `CopyPixels`.
type Transfer = unsafe extern "system" fn(*mut Com, *mut Com, *const c_void, *const [i32; 4], *const c_void) -> i32;
type CopyPixels = unsafe extern "system" fn(*mut Com, *const c_void, u32, u32, *mut u32) -> i32;

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

/// WIC's factory.
fn make_wic() -> Option<*mut Com> {
    let mut wic = null_mut();
    // SAFETY: the factory is made with its interface id.
    let made = unsafe { crate::windows::sys::CoCreateInstance(&WIC_FACTORY, null_mut(), 1, &A_WIC_FACTORY, &mut wic) };
    (made >= 0 && !wic.is_null()).then_some(wic)
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: the engine is shut down (42) and released; then the bitmap, the factory and
        // our reference to `notify`; Media Foundation is stopped as often as started.
        unsafe {
            if !self.engine.is_null() {
                self.call::<i32>(42);
                Com::release(self.engine);
            }
            release_all(&[self.bitmap, self.wic]);
            release(self.notify);
            (self.functions.shutdown)();
        }
    }
}
