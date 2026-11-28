//! A video at an address downloaded once on Windows, and read by Media Foundation's engine and
//! by taut's caption readers alike: each reads through a byte stream of its own (an
//! `IMFByteStream` of taut's) over the same download, kept in memory a block at a time. The
//! download is Media Foundation's own byte stream for the address (its HTTP, proxies and
//! sign-ins), read forward as the readers ask; a read far from where it is seeks it there.

use crate::video_mp4::Source;
use crate::video_windows::{Functions, Listener, ListenerTable, add, call, query, release};
use crate::windows::{Com, named};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};

/// `IMFByteStream`.
const A_BYTE_STREAM: [u32; 4] = named(0xAD4C_1B00, 0x4BF7, 0x422F, [0x91, 0x75, 0x75, 0x66, 0x93, 0xD9, 0x13, 0x0D]);
/// The download is kept in blocks this big, at most this many (the least used go first).
const BLOCK: u64 = 256 << 10;
const BLOCKS: usize = 256;
/// A read this far ahead of the download is waited for; farther, the download seeks there.
const SKIP: u64 = 4 << 20;
/// `MFBYTESTREAM_IS_READABLE`, `..._IS_WRITABLE`, `..._IS_SEEKABLE`.
const READABLE: u32 = 1;
const WRITABLE: u32 = 2;
const SEEKABLE: u32 = 4;
/// `E_NOTIMPL`, `E_FAIL`, `E_INVALIDARG`.
const NOT_DONE: i32 = 0x8000_4001_u32 as i32;
const FAILED: i32 = 0x8000_4005_u32 as i32;
const BAD_ARGUMENT: i32 = 0x8007_0057_u32 as i32;

/// A download of an address, shared by its readers: its length (none where the address does
/// not say) and what it can do, once started; read under its own lock, so a slow read holds up
/// no reader that only asks those.
pub(crate) struct Download {
    functions: &'static Functions,
    address: Vec<u16>,
    opened: OnceLock<(Option<u64>, u32)>,
    source: Mutex<Downloading>,
    kept: Mutex<Kept>,
}

/// Media Foundation's byte stream of the address (made at the first read; null where it can't
/// be), and where it stands (`u64::MAX` after a failed read: unknown).
struct Downloading {
    stream: *mut Com,
    at: u64,
}

/// The blocks read, each with when it was last used.
#[derive(Default)]
struct Kept {
    blocks: HashMap<u64, (Arc<[u8]>, u64)>,
    uses: u64,
}

// SAFETY: Media Foundation's byte streams are free-threaded; the stream is only used while its
// lock is held.
unsafe impl Send for Downloading {}

impl Drop for Downloading {
    fn drop(&mut self) {
        if !self.stream.is_null() {
            // SAFETY: held once.
            unsafe { Com::release(self.stream) };
        }
    }
}

impl Download {
    /// A download of `address` (not started: the first read starts it, on its reader's thread).
    pub(crate) fn new(functions: &'static Functions, address: &str) -> Arc<Download> {
        let source = Downloading { stream: null_mut(), at: 0 };
        Arc::new(Download { functions, address: crate::windows::wide(address), opened: OnceLock::new(), source: Mutex::new(source), kept: Mutex::default() })
    }

    /// Its length, once the download started; none where the address does not say.
    pub(crate) fn length(&self) -> Option<u64> {
        self.opened().0
    }

    /// What Media Foundation's byte stream of it can do, but write.
    fn capabilities(&self) -> u32 {
        self.opened().1 & !WRITABLE
    }

    /// Its length and what it can do, the download started where it was not.
    fn opened(&self) -> (Option<u64>, u32) {
        match self.opened.get() {
            Some(opened) => *opened,
            None => self.started().1,
        }
    }

    /// The download, started where it was not, and its length and what it can do.
    fn started(&self) -> (MutexGuard<'_, Downloading>, (Option<u64>, u32)) {
        let mut source = self.source.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let opened = *self.opened.get_or_init(|| {
            source.stream = self.open();
            (stream_length(source.stream), stream_capabilities(source.stream))
        });
        (source, opened)
    }

    /// Media Foundation's byte stream of the address (`MFCreateSourceResolver`; of the resolver,
    /// 3: CreateObjectFromURL as a byte stream (2) to read (0x10000)): null where it can't be
    /// read.
    fn open(&self) -> *mut Com {
        let (mut resolver, mut kind, mut made, mut stream) = (null_mut(), 0u32, null_mut::<Com>(), null_mut());
        // SAFETY: the address ends in a zero; what the resolver makes is ours, asked for its
        // byte stream (0: QueryInterface) and released.
        unsafe {
            if (self.functions.source_resolver)(&mut resolver) < 0 || resolver.is_null() {
                return null_mut();
            }
            let resolved = call!(
                resolver,
                3,
                unsafe extern "system" fn(*mut Com, *const u16, u32, *mut c_void, *mut u32, *mut *mut Com) -> i32,
                self.address.as_ptr(),
                0x1_0002,
                null_mut(),
                &mut kind,
                &mut made
            );
            if resolved >= 0 && !made.is_null() {
                call!(made, 0, unsafe extern "system" fn(*mut Com, *const [u32; 4], *mut *mut Com) -> i32, &A_BYTE_STREAM, &mut stream);
                Com::release(made);
            }
            Com::release(resolver);
        }
        stream
    }

    /// Reads from `at` into `into`: how many bytes (fewer at the end, none past it or on a
    /// failure).
    pub(crate) fn read(&self, at: u64, into: &mut [u8]) -> usize {
        let mut done = 0;
        while done < into.len() {
            let here = at + done as u64;
            let Some(block) = self.block(here / BLOCK) else { break };
            let from = (here % BLOCK) as usize;
            let taken = block.len().saturating_sub(from).min(into.len() - done);
            if taken == 0 {
                break;
            }
            into[done..done + taken].copy_from_slice(&block[from..from + taken]);
            done += taken;
        }
        done
    }

    /// Block `nth` of the download: kept, or read now; none past the end or on a failure. A
    /// block cut short before the end (a failed read) is not kept: it is read again when next
    /// asked for.
    fn block(&self, nth: u64) -> Option<Arc<[u8]>> {
        if let Some(kept) = self.kept_block(nth) {
            return Some(kept);
        }
        let (mut source, (length, _)) = self.started();
        // (Another reader may have read it meanwhile.)
        if let Some(kept) = self.kept_block(nth) {
            return Some(kept);
        }
        let start = nth.checked_mul(BLOCK).filter(|start| length.is_none_or(|length| *start < length))?;
        let ahead = start.checked_sub(source.at).filter(|ahead| *ahead <= SKIP && *ahead % BLOCK == 0);
        if ahead.is_none() && !seek_stream(source.stream, start) {
            return None;
        }
        source.at = match ahead {
            Some(_) => source.at,
            None => start,
        };
        // (Read on to the block, keeping what is passed, so another reader finds it.)
        loop {
            let Some(read) = read_stream(source.stream, BLOCK as usize) else {
                source.at = u64::MAX;
                return None;
            };
            let (number, read_length) = (source.at / BLOCK, read.len() as u64);
            source.at += read_length;
            let read: Arc<[u8]> = read.into();
            let whole = read_length == BLOCK || length.is_none_or(|length| source.at >= length);
            if !whole {
                source.at = u64::MAX;
                return (number == nth).then_some(read);
            }
            self.keep(number, Arc::clone(&read));
            if number == nth {
                return Some(read);
            }
            if read_length < BLOCK {
                return None;
            }
        }
    }

    fn kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn kept_block(&self, nth: u64) -> Option<Arc<[u8]>> {
        let mut kept = self.kept();
        kept.uses += 1;
        let uses = kept.uses;
        kept.blocks.get_mut(&nth).map(|(block, used)| {
            *used = uses;
            Arc::clone(block)
        })
    }

    /// Keeps a block read, the least used one going where there are too many.
    fn keep(&self, nth: u64, block: Arc<[u8]>) {
        let mut kept = self.kept();
        kept.uses += 1;
        let uses = kept.uses;
        kept.blocks.insert(nth, (block, uses));
        if kept.blocks.len() > BLOCKS
            && let Some(oldest) = kept.blocks.iter().min_by_key(|(_, (_, used))| *used).map(|(nth, _)| *nth)
        {
            kept.blocks.remove(&oldest);
        }
    }
}

/// A download read by taut's readers.
impl Source for Arc<Download> {
    fn read_at(&mut self, at: u64, into: &mut [u8]) -> usize {
        Download::read(self, at, into)
    }
}

/// A byte stream's length (4: GetLength); none where unknown.
fn stream_length(stream: *mut Com) -> Option<u64> {
    let mut length = 0u64;
    // SAFETY: a live byte stream, or none.
    let got = !stream.is_null() && unsafe { call!(stream, 4, unsafe extern "system" fn(*mut Com, *mut u64) -> i32, &mut length) } >= 0;
    (got && length != u64::MAX).then_some(length)
}

/// What a byte stream can do (3: GetCapabilities); nothing for none.
fn stream_capabilities(stream: *mut Com) -> u32 {
    let mut capabilities = 0u32;
    if !stream.is_null() {
        // SAFETY: a live byte stream.
        unsafe { call!(stream, 3, unsafe extern "system" fn(*mut Com, *mut u32) -> i32, &mut capabilities) };
    }
    capabilities
}

/// Puts a byte stream at `at` (7: SetCurrentPosition): whether it went.
fn seek_stream(stream: *mut Com, at: u64) -> bool {
    // SAFETY: a live byte stream, or none.
    !stream.is_null() && unsafe { call!(stream, 7, unsafe extern "system" fn(*mut Com, u64) -> i32, at) } >= 0
}

/// Reads up to `length` bytes on from where a byte stream stands (9: Read, until it has them or
/// the stream ends): none on a failure, or at the end.
fn read_stream(stream: *mut Com, length: usize) -> Option<Vec<u8>> {
    if stream.is_null() {
        return None;
    }
    let (mut bytes, mut done) = (vec![0u8; length], 0);
    while done < length {
        let mut read = 0u32;
        // SAFETY: a live byte stream, writing within the room given.
        let got = unsafe { call!(stream, 9, unsafe extern "system" fn(*mut Com, *mut u8, u32, *mut u32) -> i32, bytes[done..].as_mut_ptr(), (length - done) as u32, &mut read) };
        if got < 0 || read == 0 {
            break;
        }
        done += read as usize;
    }
    bytes.truncate(done);
    (!bytes.is_empty()).then_some(bytes)
}

// ---- a reader's byte stream ----

/// What a reader's byte stream holds: the download, where the reader stands, the reads done
/// and not yet ended (each an asynchronous result and how many bytes), and the thread that does
/// them.
pub(crate) struct Reading {
    download: Arc<Download>,
    at: Arc<AtomicU64>,
    done: Arc<Mutex<Vec<(usize, u32)>>>,
    reads: Mutex<Option<mpsc::Sender<Asked>>>,
}

/// A read asked for: where, into where and how much, and its asynchronous result.
struct Asked {
    at: u64,
    into: *mut u8,
    length: u32,
    result: *mut Com,
}

// SAFETY: the buffer and result belong to the read until its callback is invoked; only the
// reading thread uses them meanwhile.
unsafe impl Send for Asked {}

/// `IUnknown`'s methods, then `IMFByteStream`'s.
#[repr(C)]
struct ByteStreamTable {
    unknown: ListenerTable<Reading>,
    capabilities: unsafe extern "system" fn(This, *mut u32) -> i32,
    length: unsafe extern "system" fn(This, *mut u64) -> i32,
    set_length: unsafe extern "system" fn(This, u64) -> i32,
    position: unsafe extern "system" fn(This, *mut u64) -> i32,
    set_position: unsafe extern "system" fn(This, u64) -> i32,
    at_end: unsafe extern "system" fn(This, *mut i32) -> i32,
    read: unsafe extern "system" fn(This, *mut u8, u32, *mut u32) -> i32,
    begin_read: unsafe extern "system" fn(This, *mut u8, u32, *mut Com, *mut Com) -> i32,
    end_read: unsafe extern "system" fn(This, *mut Com, *mut u32) -> i32,
    write: unsafe extern "system" fn(This, *const u8, u32, *mut u32) -> i32,
    begin_write: unsafe extern "system" fn(This, *const u8, u32, *mut Com, *mut Com) -> i32,
    end_write: unsafe extern "system" fn(This, *mut Com, *mut u32) -> i32,
    seek: unsafe extern "system" fn(This, u32, i64, u32, *mut u64) -> i32,
    flush: unsafe extern "system" fn(This) -> i32,
    close: unsafe extern "system" fn(This) -> i32,
}

type This = *mut Listener<Reading>;

static BYTE_STREAM: ByteStreamTable = ByteStreamTable {
    unknown: ListenerTable { query, add, release },
    capabilities,
    length,
    set_length,
    position,
    set_position,
    at_end,
    read,
    begin_read,
    end_read,
    write,
    begin_write,
    end_write,
    seek,
    flush,
    close,
};

/// A new byte stream over the download for one reader, at its start, held once by the caller.
pub(crate) fn byte_stream(download: &Arc<Download>) -> *mut Com {
    let reading = Reading { download: Arc::clone(download), at: Arc::new(AtomicU64::new(0)), done: Arc::default(), reads: Mutex::new(None) };
    Listener::boxed(&BYTE_STREAM.unknown, &A_BYTE_STREAM, reading).cast()
}

/// What the byte stream holds.
///
/// # Safety
/// `this` is a live byte stream of ours.
unsafe fn reading<'a>(this: This) -> &'a Reading {
    // SAFETY: as the caller promises: alive while it is called.
    unsafe { &(*this).told }
}

unsafe extern "system" fn capabilities(this: This, out: *mut u32) -> i32 {
    // SAFETY: Media Foundation passes a live stream and a place for the answer.
    unsafe { *out = reading(this).download.capabilities() | READABLE | SEEKABLE };
    0
}

unsafe extern "system" fn length(this: This, out: *mut u64) -> i32 {
    // SAFETY: as above.
    let Some(length) = (unsafe { reading(this) }).download.length() else { return FAILED };
    // SAFETY: as above.
    unsafe { *out = length };
    0
}

unsafe extern "system" fn set_length(_: This, _: u64) -> i32 {
    NOT_DONE
}

unsafe extern "system" fn position(this: This, out: *mut u64) -> i32 {
    // SAFETY: as above.
    unsafe { *out = reading(this).at.load(Ordering::Acquire) };
    0
}

unsafe extern "system" fn set_position(this: This, at: u64) -> i32 {
    // SAFETY: as above.
    unsafe { reading(this).at.store(at, Ordering::Release) };
    0
}

unsafe extern "system" fn at_end(this: This, out: *mut i32) -> i32 {
    // SAFETY: as above.
    let reading = unsafe { reading(this) };
    let ended = reading.download.length().is_some_and(|length| reading.at.load(Ordering::Acquire) >= length);
    // SAFETY: as above.
    unsafe { *out = ended as i32 };
    0
}

unsafe extern "system" fn read(this: This, into: *mut u8, length: u32, out: *mut u32) -> i32 {
    // SAFETY: as above, with a buffer of `length` bytes.
    let (reading, into) = unsafe { (reading(this), std::slice::from_raw_parts_mut(into, length as usize)) };
    let at = reading.at.load(Ordering::Acquire);
    let read = reading.download.read(at, into);
    reading.at.store(at + read as u64, Ordering::Release);
    // SAFETY: as above.
    unsafe { *out = read as u32 };
    0
}

/// Reads on the byte stream's own thread, telling `callback` when done (`MFCreateAsyncResult`,
/// `MFInvokeCallback` there).
unsafe extern "system" fn begin_read(this: This, into: *mut u8, length: u32, callback: *mut Com, state: *mut Com) -> i32 {
    // SAFETY: as above.
    let reading = unsafe { reading(this) };
    let mut result = null_mut();
    // SAFETY: the callback and state are Media Foundation's; the result made is ours until the
    // reading thread invokes it.
    if unsafe { (reading.download.functions.async_result)(null_mut(), callback, state, &mut result) } < 0 || result.is_null() {
        return FAILED;
    }
    let asked = Asked { at: reading.at.load(Ordering::Acquire), into, length, result };
    let mut reads = reading.reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sender = reads.get_or_insert_with(|| reads_thread(reading));
    match sender.send(asked) {
        Ok(()) => 0,
        Err(mpsc::SendError(asked)) => {
            // SAFETY: never sent: still ours.
            unsafe { Com::release(asked.result) };
            FAILED
        }
    }
}

/// The thread that does a byte stream's asynchronous reads, in order, until the stream goes.
fn reads_thread(reading: &Reading) -> mpsc::Sender<Asked> {
    let (sender, asked) = mpsc::channel::<Asked>();
    let (download, at, done) = (Arc::clone(&reading.download), Arc::clone(&reading.at), Arc::clone(&reading.done));
    std::thread::spawn(move || {
        // SAFETY: COM for this thread, multithreaded (0), as Media Foundation's callbacks want.
        let com = unsafe { crate::windows::sys::CoInitializeEx(null_mut(), 0) } >= 0;
        for read in asked {
            // SAFETY: the buffer is the read's, `length` bytes, until its callback is invoked.
            let into = unsafe { std::slice::from_raw_parts_mut(read.into, read.length as usize) };
            let length = download.read(read.at, into);
            at.store(read.at + length as u64, Ordering::Release);
            done.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((read.result as usize, length as u32));
            // SAFETY: the result is ours (5: SetStatus); invoking it hands it to Media
            // Foundation, which holds its own reference.
            unsafe {
                call!(read.result, 5, unsafe extern "system" fn(*mut Com, i32) -> i32, 0);
                (download.functions.invoke)(read.result);
                Com::release(read.result);
            }
        }
        // (Media Foundation's byte stream, where this held the last of the download, is
        // released while COM is still started here.)
        drop(download);
        if com {
            // SAFETY: started above.
            unsafe { crate::windows::sys::CoUninitialize() };
        }
    });
    sender
}

unsafe extern "system" fn end_read(this: This, result: *mut Com, out: *mut u32) -> i32 {
    // SAFETY: as above.
    let reading = unsafe { reading(this) };
    let mut done = reading.done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // (The latest read of this result: an earlier one never ended may have had its address.)
    let Some(at) = done.iter().rposition(|(of, _)| *of == result as usize) else { return BAD_ARGUMENT };
    let (_, length) = done.remove(at);
    // SAFETY: as above; the result is live (4: GetStatus).
    unsafe {
        *out = length;
        call!(result, 4, unsafe extern "system" fn(*mut Com) -> i32)
    }
}

unsafe extern "system" fn write(_: This, _: *const u8, _: u32, _: *mut u32) -> i32 {
    NOT_DONE
}

unsafe extern "system" fn begin_write(_: This, _: *const u8, _: u32, _: *mut Com, _: *mut Com) -> i32 {
    NOT_DONE
}

unsafe extern "system" fn end_write(_: This, _: *mut Com, _: *mut u32) -> i32 {
    NOT_DONE
}

/// Moves the reader from its start (0) or where it stands (1).
unsafe extern "system" fn seek(this: This, origin: u32, offset: i64, _: u32, out: *mut u64) -> i32 {
    // SAFETY: as above.
    let reading = unsafe { reading(this) };
    let from = if origin == 0 { 0 } else { reading.at.load(Ordering::Acquire) };
    let Some(to) = from.checked_add_signed(offset) else { return BAD_ARGUMENT };
    reading.at.store(to, Ordering::Release);
    if !out.is_null() {
        // SAFETY: as above.
        unsafe { *out = to };
    }
    0
}

unsafe extern "system" fn flush(_: This) -> i32 {
    0
}

unsafe extern "system" fn close(_: This) -> i32 {
    0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    /// A server of `bytes` on a port of its own, answering ranges, counting its connections and
    /// the bytes it sends; serving until the test ends.
    pub(crate) fn serve(bytes: Vec<u8>) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/clip.mp4", listener.local_addr().unwrap());
        let (connections, sent) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (counted, sending) = (Arc::clone(&connections), Arc::clone(&sent));
        let bytes: Arc<[u8]> = bytes.into();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::AcqRel);
                let (bytes, sending) = (Arc::clone(&bytes), Arc::clone(&sending));
                std::thread::spawn(move || answer(stream, &bytes, &sending));
            }
        });
        (address, connections, sent)
    }

    /// Answers a connection's requests (`GET`, `HEAD`, with or without a range).
    fn answer(mut stream: std::net::TcpStream, bytes: &[u8], sent: &AtomicUsize) {
        let mut asked = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let Ok(read) = stream.read(&mut buffer) else { return };
            if read == 0 {
                return;
            }
            asked.extend_from_slice(&buffer[..read]);
            while let Some(end) = asked.windows(4).position(|four| four == b"\r\n\r\n") {
                let request = String::from_utf8_lossy(&asked[..end]).into_owned();
                asked.drain(..end + 4);
                if respond(&mut stream, &request, bytes, sent).is_err() {
                    return;
                }
            }
        }
    }

    fn respond(stream: &mut std::net::TcpStream, request: &str, bytes: &[u8], sent: &AtomicUsize) -> std::io::Result<()> {
        let range = request.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned));
        let (from, to) = range.and_then(|range| range.split_once('-').map(|(from, to)| (from.parse().unwrap_or(0), to.parse().unwrap_or(bytes.len() - 1)))).unwrap_or((0, bytes.len() - 1));
        let to = to.min(bytes.len() - 1);
        let body = if request.starts_with("HEAD") { &[][..] } else { &bytes[from..=to] };
        let status = if request.to_ascii_lowercase().contains("range:") { "206 Partial Content" } else { "200 OK" };
        let head = format!("HTTP/1.1 {status}\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{to}/{}\r\n\r\n", to + 1 - from, bytes.len());
        stream.write_all(head.as_bytes())?;
        stream.write_all(body)?;
        sent.fetch_add(body.len(), Ordering::AcqRel);
        Ok(())
    }

    fn functions() -> &'static Functions {
        crate::video_windows::started_functions().expect("Media Foundation")
    }

    #[test]
    fn a_download_is_read_once_by_several_readers() {
        let bytes: Vec<u8> = (0..3 * BLOCK as usize + 1000).map(|at| (at % 251) as u8).collect();
        let (address, connections, sent) = serve(bytes.clone());
        let download = Download::new(functions(), &address);
        let mut one = vec![0u8; bytes.len()];
        assert_eq!(download.read(0, &mut one), bytes.len());
        assert_eq!(one, bytes);
        // (A second reader: all of it kept.)
        let mut part = vec![0u8; 5000];
        assert_eq!(download.read(BLOCK - 100, &mut part), 5000);
        assert_eq!(part, bytes[BLOCK as usize - 100..][..5000]);
        assert!(sent.load(Ordering::Acquire) <= bytes.len() + 8192, "{} bytes sent for {}", sent.load(Ordering::Acquire), bytes.len());
        assert_eq!(download.length(), Some(bytes.len() as u64));
        assert!(connections.load(Ordering::Acquire) >= 1);
        assert_eq!(download.read(bytes.len() as u64, &mut part), 0);
    }

    #[test]
    fn an_address_that_cannot_be_read_reads_nothing() {
        let download = Download::new(functions(), "http://127.0.0.1:9/nothing.mp4");
        assert_eq!(download.read(0, &mut [0u8; 16]), 0);
        assert_eq!(download.length(), None);
    }
}
