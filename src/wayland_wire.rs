//! The Wayland wire protocol: the bytes a program and its compositor exchange over the
//! compositor's socket, written and parsed by hand.
//!
//! Everything on the wire is a message to or from an object: a surface, the keyboard, a pixel
//! buffer. A message is the object id, then its size and opcode, then its arguments, each
//! padded to four bytes. File descriptors (shared memory, the keymap, a pipe end) travel as
//! ancillary data, as sockets pass them between programs. The program allocates its own object
//! ids from 1 (the display) up, and reuses an id only after the compositor confirms deletion:
//! until then, events for it may still be in flight.
//!
//! Nothing is linked, and nothing of Wayland's needs to be installed but the compositor: the
//! program talks to the socket itself. (libwayland would need every interface's messages
//! described in C terms, and would answer through program callbacks handed to C: the same
//! description, at more remove.)
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{OsStr, c_int, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

unsafe extern "C" {
    fn sendmsg(fd: c_int, message: *const Letter, flags: c_int) -> isize;
    fn recvmsg(fd: c_int, message: *mut Letter, flags: c_int) -> isize;
    fn memfd_create(name: *const u8, flags: u32) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn mmap(at: *mut c_void, length: usize, protection: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
    fn munmap(at: *mut c_void, length: usize) -> c_int;
    fn pipe2(ends: *mut c_int, flags: c_int) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn sigemptyset(set: *mut [u64; 16]) -> c_int;
    fn sigaddset(set: *mut [u64; 16], signal: c_int) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const [u64; 16], old: *mut [u64; 16]) -> c_int;
}

/// The socket inherited from the launcher (`WAYLAND_SOCKET`) has been taken. It is the
/// program's once: its number stays in the environment, where windows opened a second time
/// would find it again, closed by then or reused by another file.
static HANDED_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `sendmsg`/`recvmsg`'s `msghdr`: no address (the socket is connected), the bytes as one
/// iovec, and room for ancillary data (`Beside`).
#[repr(C)]
struct Letter {
    address: *mut c_void,
    address_len: u32,
    pieces: *mut Piece,
    piece_count: usize,
    beside: *mut c_void,
    beside_len: usize,
    flags: c_int,
}

/// One iovec.
#[repr(C)]
struct Piece {
    at: *mut c_void,
    len: usize,
}

/// Ancillary data: its length, `SOL_SOCKET`/`SCM_RIGHTS` (1, 1), then the fds. Room for as many
/// as the compositor sends at once (at most 28).
#[repr(C)]
struct Beside {
    len: usize,
    level: c_int,
    kind: c_int,
    files: [c_int; FILES_AT_ONCE],
}
const FILES_AT_ONCE: usize = 28;

/// Flags: `MSG_NOSIGNAL` (no SIGPIPE when the peer is gone), `MSG_DONTWAIT`, and
/// `MSG_CMSG_CLOEXEC` for received fds.
const NO_SIGNAL: c_int = 0x4000;
const DO_NOT_WAIT: c_int = 0x40;
const CLOSED_ON_START: c_int = 0x4000_0000;

/// The largest message, as every compositor accepts. A longer title is truncated.
pub(crate) const LONGEST: usize = 4096;

/// Compositor-allocated object ids start here; the program's own are below.
const THEIRS: u32 = 0xff00_0000;

/// An object's interface. The compositor's messages are parsed by it, and an unused id is
/// `Free`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interface {
    Free,
    Display,
    Registry,
    Callback,
    Compositor,
    Subcompositor,
    Subsurface,
    Surface,
    Shm,
    Pool,
    Buffer,
    WmBase,
    XdgSurface,
    Toplevel,
    Seat,
    Pointer,
    Keyboard,
    Output,
    DataManager,
    DataDevice,
    DataOffer,
    DataSource,
    Decorations,
    Decoration,
    Shapes,
    ShapeDevice,
    Viewporter,
    Viewport,
    Scales,
    Scale,
    Activation,
    Token,
    Exporter,
    Exported,
    Icons,
    Icon,
    TextInputs,
    TextInput,
    Touch,
    Gestures,
    Pinch,
    Tablets,
    TabletSeat,
    Tablet,
    TabletTool,
    /// `zwp_linux_dmabuf_v1`, its feedback, and a buffer's parameters (feature `gpu`).
    #[cfg(feature = "gpu")]
    Dmabuf,
    #[cfg(feature = "gpu")]
    DmabufFeedback,
    #[cfg(feature = "gpu")]
    DmabufParams,
    /// (KWin's, through which tests move the pointer and press keys.)
    #[cfg(test)]
    FakeInput,
}

impl Interface {
    /// Whether a compositor event to such an object carries an fd: the keymap (0), a pipe end
    /// to write a selection into (1), and the table of dmabuf formats (1).
    fn takes_file(self, message: u16) -> bool {
        #[cfg(feature = "gpu")]
        if (self, message) == (Interface::DmabufFeedback, 1) {
            return true;
        }
        matches!((self, message), (Interface::Keyboard, 0) | (Interface::DataSource, 1))
    }
}

/// One message argument.
pub(crate) enum Arg<'a> {
    Int(i32),
    Uint(u32),
    /// A 24.8 fixed-point number.
    #[cfg(test)]
    Fixed(f64),
    Text(&'a str),
    /// A null string or object.
    Nothing,
    Object(u32),
    /// An fd, sent as ancillary data.
    File(RawFd),
    /// An array, as bytes.
    #[cfg(all(test, feature = "gpu"))]
    Bytes(&'a [u8]),
}

/// A compositor event to one of the program's objects, with its arguments to read in order.
pub(crate) struct Message {
    pub(crate) object: u32,
    pub(crate) of: Interface,
    pub(crate) opcode: u16,
    body: Vec<u8>,
    at: usize,
    /// Its fd, if it has one.
    pub(crate) file: Option<OwnedFd>,
}

impl Message {
    /// The next four bytes, raw.
    fn word(&mut self) -> [u8; 4] {
        let word = self.body.get(self.at..self.at + 4).and_then(|word| word.first_chunk().copied()).unwrap_or_default();
        self.at += 4;
        word
    }
    pub(crate) fn uint(&mut self) -> u32 {
        u32::from_ne_bytes(self.word())
    }
    pub(crate) fn int(&mut self) -> i32 {
        i32::from_ne_bytes(self.word())
    }
    /// A 24.8 fixed-point number.
    pub(crate) fn fixed(&mut self) -> f32 {
        self.int() as f32 / 256.0
    }
    /// A length-prefixed byte array, padded to four.
    pub(crate) fn bytes(&mut self) -> &[u8] {
        let (len, from) = (self.uint() as usize, self.at);
        self.at = (from + len).next_multiple_of(4);
        self.body.get(from..from + len).unwrap_or_default()
    }
    /// A string, NUL-terminated (empty for null).
    pub(crate) fn text(&mut self) -> String {
        let bytes = self.bytes();
        String::from_utf8_lossy(bytes.strip_suffix(&[0]).unwrap_or(bytes)).into_owned()
    }
    /// All its arguments as numbers: for a test playing compositor and reading the program's
    /// requests.
    #[cfg(test)]
    pub(crate) fn words(&self) -> Vec<i32> {
        self.body.as_chunks::<4>().0.iter().map(|word| i32::from_ne_bytes(*word)).collect()
    }
}

/// The program's end of the compositor socket.
pub(crate) struct Wire {
    socket: UnixStream,
    /// Requests written but not sent: they go when the program waits, or when an fd must go
    /// with them.
    out: RefCell<Vec<u8>>,
    /// Received bytes not yet parsed; and their fds, in order.
    inbox: RefCell<Vec<u8>>,
    files: RefCell<VecDeque<OwnedFd>>,
    /// Each program-owned id's interface (the id is the index), and the ids the compositor has
    /// deleted, for reuse; and each compositor-owned id's interface (indexed from `THEIRS`).
    objects: RefCell<Vec<Interface>>,
    free: RefCell<Vec<u32>>,
    theirs: RefCell<Vec<Interface>>,
    /// The compositor is gone, or sent a protocol error: nothing more can be said to it.
    pub(crate) broken: Cell<bool>,
}

/// The compositor socket the environment names: an open fd inherited from the launcher
/// (`WAYLAND_SOCKET`), or a named socket (`WAYLAND_DISPLAY`), as a path or a name in the user's
/// runtime directory (`XDG_RUNTIME_DIR`). None where the environment names none.
pub(crate) enum Named {
    Open(RawFd),
    At(PathBuf),
}

/// Where the compositor is, from the environment (each value as named above, or none).
pub(crate) fn named(handed: Option<&OsStr>, display: Option<&OsStr>, runtime: Option<&OsStr>) -> Option<Named> {
    let number = handed.and_then(|handed| handed.to_str()?.parse::<RawFd>().ok()).filter(|fd| *fd >= 0);
    if let Some(fd) = number {
        return Some(Named::Open(fd));
    }
    let display = display.filter(|display| !display.is_empty())?;
    if display.as_bytes().starts_with(b"/") {
        return Some(Named::At(display.into()));
    }
    Some(Named::At(PathBuf::from(runtime.filter(|runtime| !runtime.is_empty())?).join(display)))
}

impl Wire {
    /// Connects to the compositor the environment names. None where it names none, or nobody
    /// listens.
    pub(crate) fn connect() -> Option<Self> {
        let (handed, display, runtime) = (std::env::var_os("WAYLAND_SOCKET"), std::env::var_os("WAYLAND_DISPLAY"), std::env::var_os("XDG_RUNTIME_DIR"));
        let handed = handed.filter(|_| !HANDED_TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst));
        Wire::connect_to(named(handed.as_deref(), display.as_deref(), runtime.as_deref())?)
    }

    /// Connects to the named compositor socket. None where nobody listens, or the inherited
    /// number is no open fd. (An inherited socket is set close-on-exec, `F_SETFD` 2 with 1:
    /// child programs must not use it too.)
    pub(crate) fn connect_to(named: Named) -> Option<Self> {
        let socket = match named {
            // SAFETY: the number was inherited for this program to take, and is taken once (see
            // `connect`), where it is an open fd.
            Named::Open(fd) => unsafe { (fcntl(fd, 2, 1 as c_int) == 0).then(|| UnixStream::from_raw_fd(fd))? },
            Named::At(path) => UnixStream::connect(path).ok()?,
        };
        Some(Wire::over(socket))
    }

    /// Speaks over an already connected socket.
    pub(crate) fn over(socket: UnixStream) -> Self {
        let objects = RefCell::new(vec![Interface::Free, Interface::Display]);
        Wire { socket, out: RefCell::default(), inbox: RefCell::default(), files: RefCell::default(), objects, free: RefCell::default(), theirs: RefCell::default(), broken: Cell::new(false) }
    }

    /// The socket fd, for polling.
    pub(crate) fn fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    /// A new program-owned id: one the compositor deleted, or the next never used.
    pub(crate) fn make(&self, of: Interface) -> u32 {
        let mut objects = self.objects.borrow_mut();
        let id = self.free.borrow_mut().pop().unwrap_or(objects.len() as u32);
        if id as usize == objects.len() {
            objects.push(of);
        } else {
            objects[id as usize] = of;
        }
        id
    }

    /// Records a compositor-created object (passed in an event), or a released one (as `Free`).
    /// (An id the compositor has no business sending is ignored.)
    pub(crate) fn note(&self, id: u32, of: Interface) {
        let mut theirs = self.theirs.borrow_mut();
        let Some(at) = id.checked_sub(THEIRS).map(|at| at as usize).filter(|at| *at <= theirs.len() + 64) else { return };
        if theirs.len() <= at {
            theirs.resize(at + 1, Interface::Free);
        }
        theirs[at] = of;
    }

    /// The compositor deleted an object: its id may be reused.
    pub(crate) fn deleted(&self, id: u32) {
        if let Some(object) = self.objects.borrow_mut().get_mut(id as usize).filter(|object| id > 1 && **object != Interface::Free) {
            *object = Interface::Free;
            self.free.borrow_mut().push(id);
        }
    }

    /// An object's interface.
    pub(crate) fn of(&self, id: u32) -> Interface {
        let found = match id.checked_sub(THEIRS) {
            Some(at) => self.theirs.borrow().get(at as usize).copied(),
            None => self.objects.borrow().get(id as usize).copied(),
        };
        found.unwrap_or(Interface::Free)
    }

    /// Sends a request to an object. It is buffered with what follows, unless an fd goes with
    /// it: then it is sent at once, with everything queued before it.
    pub(crate) fn send(&self, object: u32, opcode: u16, args: &[Arg]) {
        let mut message = Vec::with_capacity(64);
        message.extend_from_slice(&object.to_ne_bytes());
        message.extend_from_slice(&[0; 4]);
        let mut file = None;
        for arg in args {
            match arg {
                Arg::Int(value) => message.extend_from_slice(&value.to_ne_bytes()),
                Arg::Uint(value) | Arg::Object(value) => message.extend_from_slice(&value.to_ne_bytes()),
                #[cfg(test)]
                Arg::Fixed(value) => message.extend_from_slice(&((value * 256.0).round() as i32).to_ne_bytes()),
                Arg::Nothing => message.extend_from_slice(&[0; 4]),
                Arg::Text(text) => written(&mut message, &[text.as_bytes(), &[0]].concat()),
                Arg::File(fd) => file = Some(*fd),
                #[cfg(all(test, feature = "gpu"))]
                Arg::Bytes(bytes) => written(&mut message, bytes),
            }
        }
        // (Compositors reject longer messages and disconnect the client; and the size field is
        // two bytes. Strings are truncated before they get here: one that is not, from another
        // program, is not sent at all.)
        if message.len() > LONGEST {
            return;
        }
        let size = (message.len() as u32) << 16 | opcode as u32;
        message[4..8].copy_from_slice(&size.to_ne_bytes());
        let full = self.out.borrow().len() + message.len() > LONGEST;
        if full {
            self.flush();
        }
        self.out.borrow_mut().extend_from_slice(&message);
        if let Some(fd) = file {
            self.post(Some(fd));
        }
    }

    /// Binds a registry global by its name, as the given interface and version; the program's
    /// object is created.
    pub(crate) fn bind(&self, registry: u32, name: u32, interface: &str, version: u32, of: Interface) -> u32 {
        let id = self.make(of);
        self.send(registry, 0, &[Arg::Uint(name), Arg::Text(interface), Arg::Uint(version), Arg::Uint(id)]);
        id
    }

    /// Sends everything queued.
    pub(crate) fn flush(&self) {
        self.post(None);
    }

    /// Sends what is queued, with an fd if given. (The socket blocks while the compositor lags
    /// in reading: a client's messages are few and short.)
    fn post(&self, file: Option<RawFd>) {
        let mut out = self.out.borrow_mut();
        let mut sent = 0;
        let mut beside = Beside { len: 16 + 4, level: 1, kind: 1, files: [0; FILES_AT_ONCE] };
        beside.files[0] = file.unwrap_or(-1);
        while sent < out.len() && !self.broken.get() {
            let mut piece = Piece { at: out[sent..].as_mut_ptr().cast(), len: out.len() - sent };
            let with = file.filter(|_| sent == 0).is_some();
            let letter =
                Letter { address: std::ptr::null_mut(), address_len: 0, pieces: &mut piece, piece_count: 1, beside: (&raw mut beside).cast(), beside_len: if with { 24 } else { 0 }, flags: 0 };
            // SAFETY: the header names bytes and fds alive until the call returns.
            let done = unsafe { sendmsg(self.fd(), &letter, NO_SIGNAL) };
            match done {
                ..0 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
                ..=0 => self.broken.set(true),
                _ => sent += done as usize,
            }
        }
        out.clear();
    }

    /// Reads what the compositor sent, without blocking. Says whether the compositor is still
    /// there.
    pub(crate) fn read(&self) -> bool {
        let mut room = [0u8; 4096];
        loop {
            let mut beside = Beside { len: 0, level: 0, kind: 0, files: [-1; FILES_AT_ONCE] };
            let mut piece = Piece { at: room.as_mut_ptr().cast(), len: room.len() };
            let mut letter = Letter { address: std::ptr::null_mut(), address_len: 0, pieces: &mut piece, piece_count: 1, beside: (&raw mut beside).cast(), beside_len: size_of::<Beside>(), flags: 0 };
            // SAFETY: the header names room alive until the call returns, and the system writes
            // no more than it says.
            let got = unsafe { recvmsg(self.fd(), &mut letter, DO_NOT_WAIT | CLOSED_ON_START) };
            if got < 0 {
                let kind = std::io::Error::last_os_error().kind();
                return matches!(kind, std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) && !self.broken.get();
            }
            if got == 0 {
                self.broken.set(true);
                return false;
            }
            self.inbox.borrow_mut().extend_from_slice(&room[..got as usize]);
            self.take_files(&beside, letter.beside_len);
        }
    }

    /// Keeps the fds received with the bytes, in order.
    fn take_files(&self, beside: &Beside, len: usize) {
        if len < 16 || (beside.level, beside.kind) != (1, 1) {
            return;
        }
        let count = ((beside.len.min(len) - 16) / 4).min(FILES_AT_ONCE);
        // SAFETY: each is an fd the system just gave this program.
        let files = beside.files[..count].iter().filter(|fd| **fd >= 0).map(|fd| unsafe { OwnedFd::from_raw_fd(*fd) });
        self.files.borrow_mut().extend(files);
    }

    /// The next complete event, if any.
    pub(crate) fn next(&self) -> Option<Message> {
        let mut inbox = self.inbox.borrow_mut();
        let head: [u8; 8] = *inbox.first_chunk()?;
        let (object, size) = (u32::from_ne_bytes(head[..4].try_into().ok()?), u32::from_ne_bytes(head[4..].try_into().ok()?));
        let (len, opcode) = ((size >> 16) as usize, size as u16);
        if len < 8 {
            // (A message shorter than its own header: the peer is not speaking Wayland.)
            self.broken.set(true);
            inbox.clear();
            return None;
        }
        if inbox.len() < len {
            return None;
        }
        let body = inbox[8..len].to_vec();
        inbox.drain(..len);
        let of = self.of(object);
        let file = if of.takes_file(opcode) { self.files.borrow_mut().pop_front() } else { None };
        Some(Message { object, of, opcode, body, at: 0, file })
    }
}

/// A copy of `len` bytes of a received file: the keymap arrives that way. Read from its start
/// with `pread`, without moving the file offset the compositor shares with every client it
/// sends it to; and no further than its real size, whatever the compositor claims (it may have
/// overstated, or truncated it since). None where it cannot be read.
pub(crate) fn read_file(file: &OwnedFd, len: usize) -> Option<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::from(file.try_clone().ok()?);
    let mut copy = vec![0; len.min(file.metadata().ok()?.len() as usize)];
    let mut got = 0;
    while got < copy.len() {
        match file.read_at(&mut copy[got..], got as u64) {
            Ok(0) => break,
            Ok(read) => got += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    copy.truncate(got);
    Some(copy)
}

/// A pipe: its read end and write end, both close-on-exec (`O_CLOEXEC`, 0o2000000). None where
/// the system gives none.
pub(crate) fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut ends = [-1; 2];
    // SAFETY: the system writes the two fds into the given room; this program owns them from
    // then on.
    unsafe {
        if pipe2(ends.as_mut_ptr(), 0o2000000) != 0 {
            return None;
        }
        Some((OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])))
    }
}

/// Keeps SIGPIPE (13) from ending the program when this thread writes into a pipe whose reader
/// is gone: the write fails instead. Rust programs ignore it from the start, but a C program
/// hosting this library may not. (Blocked, the signal stays pending on this thread and goes
/// when it ends: so this is for a thread that only writes, then ends.)
pub(crate) fn no_signal_for_pipes() {
    let mut blocked = [0u64; 16];
    // SAFETY: the set is the C library's size (1,024 bits), filled by its own functions; only
    // this thread's mask changes (`SIG_BLOCK`, 0).
    unsafe {
        sigemptyset(&mut blocked);
        sigaddset(&mut blocked, 13);
        pthread_sigmask(0, &blocked, std::ptr::null_mut());
    }
}

/// Writes a length-prefixed byte array into a message, padded to four.
fn written(message: &mut Vec<u8>, bytes: &[u8]) {
    message.extend_from_slice(&(bytes.len() as u32).to_ne_bytes());
    message.extend_from_slice(bytes);
    message.resize(message.len().next_multiple_of(4), 0);
}

/// Memory shared with the compositor: a memfd, mapped into this program. The compositor gets
/// the fd.
pub(crate) struct Shared {
    pub(crate) file: OwnedFd,
    at: *mut u8,
    pub(crate) len: usize,
}

impl Shared {
    /// A zeroed block of `len` bytes. None where the system refuses.
    pub(crate) fn new(len: usize) -> Option<Self> {
        // SAFETY: the documented calls for creating such a file (`MFD_CLOEXEC`, 1) and mapping
        // it read-write (3), shared (`MAP_SHARED`, 1). Their results are checked before use.
        unsafe {
            let fd = memfd_create(c"taut".as_ptr().cast(), 1);
            if fd < 0 {
                return None;
            }
            let file = OwnedFd::from_raw_fd(fd);
            if ftruncate(fd, len as i64) != 0 {
                return None;
            }
            let at = mmap(std::ptr::null_mut(), len.max(1), 3, 1, fd, 0);
            (at as isize != -1).then(|| Shared { file, at: at.cast(), len })
        }
    }

    /// The block as four-byte pixels.
    pub(crate) fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the block is mapped while this lives, page-aligned, and nothing else in this
        // program touches it meanwhile.
        unsafe { std::slice::from_raw_parts_mut(self.at.cast(), self.len / 4) }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the block was mapped in `new` and is not used after this.
        unsafe { munmap(self.at.cast(), self.len.max(1)) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A wire, and the compositor's end of its socket.
    fn pair() -> (Wire, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        (Wire::over(ours), theirs)
    }

    #[test]
    fn a_message_is_laid_out_as_wayland_has_it() {
        let (wire, mut theirs) = pair();
        let surface = wire.make(Interface::Surface);
        assert_eq!(surface, 2, "1 is the compositor's own");
        wire.send(surface, 2, &[Arg::Int(-1), Arg::Uint(7), Arg::Fixed(1.5), Arg::Text("abcd"), Arg::Nothing, Arg::Object(9)]);
        wire.flush();
        let mut got = vec![0; 64];
        let len = std::io::Read::read(&mut theirs, &mut got).unwrap();
        let words: Vec<u32> = got[..len].as_chunks::<4>().0.iter().map(|word| u32::from_ne_bytes(*word)).collect();
        // The object, then the size (here 40 bytes) and opcode (2); then -1, 7, 1.5 as 384,
        // "abcd" as five bytes with its NUL, padded to eight, a null string, and an object.
        assert_eq!(words[..6], [2, 40 << 16 | 2, u32::MAX, 7, 384, 5]);
        assert_eq!(&got[24..32], b"abcd\0\0\0\0");
        assert_eq!(words[8..], [0, 9]);
        assert_eq!(len, 40);
    }

    #[test]
    fn messages_are_read_whole_whatever_pieces_they_come_in() {
        let (wire, mut theirs) = pair();
        let callback = wire.make(Interface::Callback);
        // Two events, a callback's done (1234) and the display's delete_id, sent in chunks that
        // split across them.
        let mut bytes = Vec::new();
        for (object, opcode, value) in [(callback, 0u32, 1234u32), (1, 1, callback)] {
            bytes.extend_from_slice(&object.to_ne_bytes());
            bytes.extend_from_slice(&(12 << 16 | opcode).to_ne_bytes());
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        std::io::Write::write_all(&mut theirs, &bytes[..5]).unwrap();
        assert!(wire.read());
        assert!(wire.next().is_none(), "half a head is no message");
        std::io::Write::write_all(&mut theirs, &bytes[5..]).unwrap();
        assert!(wire.read());
        let mut first = wire.next().unwrap();
        assert_eq!((first.object, first.of, first.opcode, first.uint()), (callback, Interface::Callback, 0, 1234));
        let mut second = wire.next().unwrap();
        assert_eq!((second.object, second.of, second.opcode), (1, Interface::Display, 1));
        wire.deleted(second.uint());
        assert!(wire.next().is_none());
        // The id is free again, and is reused next.
        assert_eq!(wire.of(callback), Interface::Free);
        assert_eq!(wire.make(Interface::Buffer), callback);
        drop(theirs);
        assert!(!wire.read(), "the compositor is gone");
    }

    #[test]
    fn the_compositors_own_objects_are_kept_apart_from_the_programs() {
        let (wire, _theirs) = pair();
        // The compositor allocates ids from 0xff000000 up: those are tracked separately and
        // take none of the program's ids.
        wire.note(THEIRS + 1, Interface::DataOffer);
        assert_eq!((wire.of(THEIRS + 1), wire.of(THEIRS)), (Interface::DataOffer, Interface::Free));
        assert_eq!(wire.make(Interface::Surface), 2, "the program's next number is its own next");
        wire.note(THEIRS + 1, Interface::Free);
        assert_eq!(wire.of(THEIRS + 1), Interface::Free, "let go of");
        wire.note(THEIRS + 100_000, Interface::DataOffer);
        assert_eq!(wire.of(THEIRS + 100_000), Interface::Free, "a number far out of turn is passed over");
        wire.deleted(THEIRS + 1);
        assert_eq!(wire.make(Interface::Buffer), 3, "only the program's numbers are taken back into use");
    }

    #[test]
    fn texts_and_bytes_are_read_as_they_were_written() {
        let (wire, _theirs) = pair();
        let mut body = Vec::new();
        written(&mut body, b"seat0\0");
        body.extend_from_slice(&(-512i32).to_ne_bytes());
        written(&mut body, &[]);
        written(&mut body, &[1, 2, 3]);
        let mut message = Message { object: 1, of: Interface::Seat, opcode: 1, body, at: 0, file: None };
        assert_eq!((message.text(), message.fixed(), message.text()), ("seat0".to_string(), -2.0, String::new()));
        assert_eq!(message.bytes(), &[1, 2, 3]);
        assert_eq!(message.uint(), 0, "past the end, a message reads as noughts");
        drop(wire);
    }

    #[test]
    fn a_file_goes_beside_the_message_and_comes_out_beside_its_own() {
        let (wire, theirs) = pair();
        // The other end parses with a wire of its own, as a compositor would.
        let other = Wire::over(theirs);
        let keyboard = other.make(Interface::Keyboard);
        let mut block = Shared::new(4096).unwrap();
        block.pixels()[0] = 0x1234_5678;
        wire.send(keyboard, 0, &[Arg::Uint(1), Arg::File(block.file.as_raw_fd()), Arg::Uint(4096)]);
        assert!(other.read());
        let mut message = other.next().unwrap();
        assert_eq!((message.of, message.uint(), message.uint()), (Interface::Keyboard, 1, 4096));
        let file = message.file.take().expect("the file came beside it");
        // SAFETY: the file is a 4096-byte block, mapped read-only here.
        let at = unsafe { mmap(std::ptr::null_mut(), 4096, 1, 1, file.as_raw_fd(), 0) };
        assert_ne!(at as isize, -1);
        // SAFETY: as above.
        assert_eq!(unsafe { *at.cast::<u32>() }, 0x1234_5678, "the same memory, seen through the file that came");
        unsafe { munmap(at, 4096) };
    }

    #[test]
    fn the_compositor_is_found_where_the_environment_says() {
        let at = |named: Option<Named>| match named {
            Some(Named::At(path)) => Some(path),
            _ => None,
        };
        let os = |text: &'static str| Some(OsStr::new(text));
        assert_eq!(at(named(None, os("wayland-1"), os("/run/user/1000"))), Some(PathBuf::from("/run/user/1000/wayland-1")));
        assert_eq!(at(named(None, os("/tmp/own"), None)), Some(PathBuf::from("/tmp/own")));
        assert!(named(None, None, os("/run/user/1000")).is_none(), "no display named: no compositor");
        assert!(named(None, os(""), os("/run/user/1000")).is_none());
        assert!(named(None, os("wayland-0"), None).is_none(), "a name with no folder to look in");
        assert!(matches!(named(os("7"), os("wayland-0"), os("/run")), Some(Named::Open(7))), "a socket handed down goes first");
        assert!(at(named(os("x"), os("wayland-0"), os("/run"))).is_some());
    }

    unsafe extern "C" {
        fn signal(signal: c_int, way: usize) -> usize;
    }

    /// Makes writing into a pipe or socket whose peer is gone end the program (SIGPIPE, 13,
    /// `SIG_DFL`, 0), as it does a C program: for a test running alone.
    pub(crate) fn as_in_c() {
        // SAFETY: the test runs alone, single-threaded, in its own process.
        unsafe { signal(13, 0) };
    }

    /// Tells a test it runs in its own process.
    pub(crate) const ALONE: &str = "TAUT_TEST_ALONE";

    /// Runs a test of this file in its own process (this binary again, told to run only that
    /// test, with no compositor named): one that changes the environment and closes fds by
    /// number, or that a signal would end. Requires it to pass, and reports how it ended where
    /// it did not.
    pub(crate) fn by_itself(test: &str) {
        let mut program = std::process::Command::new(std::env::current_exe().unwrap());
        program.args(["--exact", test, "--ignored", "--nocapture", "--test-threads=1"]).env(ALONE, "1");
        let ran = program.env_remove("WAYLAND_DISPLAY").env_remove("WAYLAND_SOCKET").output().unwrap();
        let (said, signal) = (String::from_utf8_lossy(&ran.stdout), std::os::unix::process::ExitStatusExt::signal(&ran.status));
        assert!(ran.status.success() && said.contains("1 passed"), "ended by signal {signal:?}\n{said}\n{}", String::from_utf8_lossy(&ran.stderr));
    }

    /// A keymap is read as far as its file goes, whatever size the compositor claims.
    /// (Regression: mapped as it was, the bytes past the file's end raised SIGBUS, signal 7,
    /// and ended the program.)
    #[test]
    fn a_map_said_to_be_longer_than_its_file_is_read_as_far_as_the_file_goes() {
        by_itself("wayland_wire::tests::a_map_said_to_be_longer_than_its_file_alone");
    }

    #[test]
    #[ignore = "run by the test before it, in a program of its own"]
    fn a_map_said_to_be_longer_than_its_file_alone() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        let mut block = Shared::new(100).unwrap();
        block.pixels().fill(u32::from_ne_bytes(*b"keys"));
        let read = read_file(&block.file, 8192).expect("the file can be read");
        assert_eq!((read.len(), &read[..8]), (100, &b"keyskeys"[..]), "as far as the file goes");
        assert_eq!(read_file(&block.file, 10).map(|read| read.len()), Some(10), "and no further than said");
    }

    /// An inherited socket is taken once: windows opened again do not take its number again (by
    /// then another file's, which would be closed under its owner), and child programs do not
    /// inherit it. (Regression: taken again, the second wire closed the file opened in between,
    /// and its owner, closing it again, hit an fd-ownership abort in the standard library.)
    #[test]
    fn a_socket_handed_down_is_taken_once_and_kept_from_programs_started() {
        by_itself("wayland_wire::tests::a_socket_handed_down_is_taken_once_alone");
    }

    #[test]
    #[ignore = "run by the test before it, in a program of its own"]
    fn a_socket_handed_down_is_taken_once_alone() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        use std::os::fd::IntoRawFd;
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let number = ours.into_raw_fd();
        // SAFETY: changes an open fd's flags (inherited, it is not close-on-exec: starting this
        // program passed it down) and the environment; the test runs alone, single-threaded, in
        // its own process.
        unsafe {
            fcntl(number, 2, 0 as c_int);
            std::env::set_var("WAYLAND_SOCKET", number.to_string());
        }
        let first = Wire::connect().expect("the socket handed down");
        // SAFETY: reads an open fd's flags.
        let marked = unsafe { fcntl(number, 1) } & 1 == 1;
        drop(first);
        let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(file.as_raw_fd(), number, "the number is the next file's");
        let second = Wire::connect();
        let again = second.is_some();
        // (Taken again, the wire is not dropped: it would close the file under its owner.)
        std::mem::forget(second);
        assert!(!again && marked, "taken a second time: {again}; marked to be closed in programs this one starts: {marked}");
        assert!(file.metadata().is_ok(), "the file is still open");
        assert!(Wire::connect_to(Named::Open(-1)).is_none() && Wire::connect_to(Named::Open(9999)).is_none(), "no open file is no socket");
    }

    #[test]
    fn what_is_sent_to_the_compositor_never_says_a_length_it_is_not() {
        // The size field is 16 bits: a message longer than compositors accept is not sent at
        // all, rather than sent with a wrong size that turns its remainder into other messages.
        use std::io::Read;
        let (wire, mut theirs) = pair();
        let token = "t".repeat(70_000);
        wire.send(5, 2, &[Arg::Text(&token), Arg::Uint(7)]);
        wire.send(6, 1, &[Arg::Uint(1)]);
        wire.flush();
        theirs.set_nonblocking(true).unwrap();
        let mut came = Vec::new();
        let mut piece = [0u8; 65536];
        while let Ok(len) = theirs.read(&mut piece) {
            if len == 0 {
                break;
            }
            came.extend_from_slice(&piece[..len]);
        }
        // Walked by their sizes, the messages end exactly where the bytes do, each at most 4096
        // bytes.
        let mut at = 0;
        let mut objects = Vec::new();
        while at + 8 <= came.len() {
            let object = u32::from_ne_bytes(came[at..at + 4].try_into().unwrap());
            let len = (u32::from_ne_bytes(came[at + 4..at + 8].try_into().unwrap()) >> 16) as usize;
            assert!((8..=4096).contains(&len), "a message of {len} bytes at {at}");
            objects.push(object);
            at += len;
        }
        assert_eq!(at, came.len(), "the messages end where the bytes do");
        assert_eq!(objects.last(), Some(&6), "the last message is the last sent: {objects:?}");
    }
}
