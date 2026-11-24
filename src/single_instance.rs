//! Single instance: a second start hands its arguments to the running copy and exits; the first
//! gets them on its next run ([`Ui::relaunches`]) and comes to the front, as far as the
//! system allows.
//!
//! ```ignore
//! fn main() {
//!     if !taut::single_instance("org.example.Notes") {
//!         return; // the copy that runs has what this start was given
//!     }
//!     taut::run(Window { title: "Notes", ..Default::default() }, describe);
//! }
//! ```
//!
//! - **Windows:** the first copy holds a session mutex named for the program, and a hidden
//!   window (`shell`) that a second start finds and sends its start to (`WM_COPYDATA`),
//!   allowing it to come to the front.
//! - **Linux:** the first copy locks a file in `XDG_RUNTIME_DIR` (private to the user) and
//!   listens on a socket beside it, on a thread that sleeps until a start comes. A socket, not
//!   a bus name: no bus needed, no wait at startup, and the lock is released by the system so a
//!   crash leaves nothing to clean up. A second start passes on its activation token
//!   (`XDG_ACTIVATION_TOKEN` on Wayland, `DESKTOP_STARTUP_ID` on X), which the first copy uses
//!   to come forward.
//!
//! The arguments are passed as given, with the working folder, so relative paths resolve.
use crate::ui::Ui;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Mutex;

/// Most bytes a start may pass (arguments and folder).
pub(crate) const MOST: usize = 1 << 20;
/// Most starts kept until a run takes them; the oldest go first. (They come from other
/// programs, and an app that never asks would keep them all.)
pub(crate) const KEPT: usize = 64;
/// How long a second start keeps trying to reach the first (which may be starting or ending);
/// after that it runs as a first copy.
pub(crate) const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// A later start, as the first copy gets it: its arguments and working folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relaunch {
    /// Arguments after the program name: usually files to open, and the app's switches.
    pub args: Vec<OsString>,
    /// The working folder: relative paths are relative to it.
    pub working_dir: PathBuf,
}

/// Starts not yet taken by a run; and the activation tokens of the last one, until the windows
/// use them. (Filled from whichever thread hears a start.)
static CAME: Mutex<Vec<Relaunch>> = Mutex::new(Vec::new());
static FORWARD: Mutex<Option<Tokens>> = Mutex::new(None);

/// A start's activation tokens: Wayland's, and X's startup id (which holds its time).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub wayland: String,
    pub x: String,
}

/// Makes this the one running copy of program `name` (the same in every copy:
/// "org.example.Notes"). `true` for the first copy, which from now on gets every later start
/// ([`Ui::relaunches`]) and comes forward for it. `false` for a later one, which has just
/// handed its start to the first and should exit. Call once, before `taut::run`, on the window
/// thread. If the first copy cannot be reached within five seconds, or the system has no way,
/// every start is a first.
pub fn single_instance(name: &str) -> bool {
    let here = Relaunch { args: std::env::args_os().skip(1).collect(), working_dir: std::env::current_dir().unwrap_or_default() };
    let tokens = Tokens { wayland: var("XDG_ACTIVATION_TOKEN"), x: var("DESKTOP_STARTUP_ID") };
    let packed = packed(&tokens, &here);
    crate::platform::hand_desktop();
    #[cfg(windows)]
    return crate::windows_shell::first_copy(name, &packed);
    #[cfg(unix)]
    return linux::first_copy(name, &packed);
    // A page is always the first: there is no other copy to hand a start to.
    #[cfg(target_family = "wasm")]
    let _ = (name, packed);
    #[allow(unreachable_code)]
    true
}

/// An environment variable as text (empty if unset).
fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// A start came: the next run gets it, and the windows come forward with its tokens. (From any
/// thread.)
pub(crate) fn came(tokens: Tokens, start: Relaunch) {
    let mut came = CAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if came.len() >= KEPT {
        came.remove(0);
    }
    came.push(start);
    drop(came);
    *FORWARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tokens);
    crate::platform::Waker.wake();
}

/// The tokens to come forward with, if a start came since last asked.
#[cfg_attr(target_os = "ios", allow(dead_code, reason = "iOS has no such part"))]
pub(crate) fn forward() -> Option<Tokens> {
    FORWARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
}

/// A start as bytes: tokens, folder and arguments, each as a 4-byte little-endian length, then
/// the bytes.
pub(crate) fn packed(tokens: &Tokens, start: &Relaunch) -> Vec<u8> {
    let mut bytes = Vec::new();
    let parts = [OsStr::new(&tokens.wayland), OsStr::new(&tokens.x), start.working_dir.as_os_str()].into_iter().chain(start.args.iter().map(OsString::as_os_str));
    for part in parts {
        let part = bytes_of(part);
        bytes.extend((part.len() as u32).to_le_bytes());
        bytes.extend(part);
    }
    bytes
}

/// Reads back a packed start. `None` if the bytes (from another program) are malformed or over
/// the limit.
pub(crate) fn unpacked(mut bytes: &[u8]) -> Option<(Tokens, Relaunch)> {
    if bytes.len() > MOST {
        return None;
    }
    let mut parts = Vec::new();
    while let Some((len, rest)) = bytes.split_first_chunk::<4>() {
        let len = u32::from_le_bytes(*len) as usize;
        parts.push(of_bytes(rest.get(..len)?)?);
        bytes = &rest[len..];
    }
    if !bytes.is_empty() || parts.len() < 3 {
        return None;
    }
    let mut parts = parts.into_iter();
    let mut text = || parts.next().map(|part| part.to_string_lossy().into_owned()).unwrap_or_default();
    let tokens = Tokens { wayland: text(), x: text() };
    let folder = parts.next()?.into();
    Some((tokens, Relaunch { args: parts.collect(), working_dir: folder }))
}

/// An argument as bytes: UTF-16 little-endian on Windows; raw elsewhere.
#[cfg(windows)]
fn bytes_of(word: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    word.encode_wide().flat_map(u16::to_le_bytes).collect()
}
#[cfg(windows)]
fn of_bytes(bytes: &[u8]) -> Option<OsString> {
    use std::os::windows::ffi::OsStringExt;
    let (units, rest) = bytes.as_chunks::<2>();
    rest.is_empty().then(|| OsString::from_wide(&units.iter().map(|unit| u16::from_le_bytes(*unit)).collect::<Vec<_>>()))
}
#[cfg(unix)]
fn bytes_of(word: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    word.as_bytes().to_vec()
}
#[cfg(unix)]
fn of_bytes(bytes: &[u8]) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    Some(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(any(windows, unix)))]
fn bytes_of(word: &OsStr) -> Vec<u8> {
    word.as_encoded_bytes().to_vec()
}
#[cfg(not(any(windows, unix)))]
fn of_bytes(bytes: &[u8]) -> Option<OsString> {
    String::from_utf8(bytes.to_vec()).ok().map(OsString::from)
}

/// Single instance.
impl Ui {
    /// Later starts of the program since the last call, if this is the first copy
    /// ([`taut::single_instance`](crate::single_instance())): each one's arguments and folder,
    /// in order. (The window comes forward for them by itself.)
    pub fn relaunches(&mut self) -> Vec<Relaunch> {
        let starts = std::mem::take(&mut *CAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        self.again |= !starts.is_empty();
        starts
    }
}

/// Linux and macOS: a file lock and a socket in the user's runtime folder (on macOS, where
/// there is none, the user's own temporary folder).
#[cfg(unix)]
pub(crate) mod linux {
    use super::{MOST, PATIENCE, came, unpacked};
    use std::ffi::c_int;
    #[cfg(target_vendor = "apple")]
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn flock(fd: c_int, how: c_int) -> c_int;
        fn send(fd: c_int, bytes: *const u8, len: usize, flags: c_int) -> isize;
        #[cfg(target_vendor = "apple")]
        fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
    }

    /// macOS: a socket that never raises SIGPIPE (`SOL_SOCKET`, `SO_NOSIGPIPE`). Elsewhere each
    /// send says so itself (`NO_SIGNAL`).
    fn quiet(stream: &UnixStream) {
        #[cfg(target_vendor = "apple")]
        // SAFETY: an option of a socket this program holds, set from a 4-byte one.
        unsafe {
            setsockopt(stream.as_raw_fd(), 0xffff, 0x1022, (&1i32 as *const i32).cast(), 4)
        };
        #[cfg(not(target_vendor = "apple"))]
        let _ = stream;
    }
    /// Exclusive lock, without waiting (`LOCK_EX | LOCK_NB`); and a send that fails instead of
    /// raising SIGPIPE when the other end is gone (`MSG_NOSIGNAL`; macOS has no such flag, and
    /// sets it on the socket instead, see `quiet`).
    const ALONE: c_int = 2 | 4;
    #[cfg(not(target_vendor = "apple"))]
    const NO_SIGNAL: c_int = 0x4000;
    #[cfg(target_vendor = "apple")]
    const NO_SIGNAL: c_int = 0;
    /// The longest name kept as it is in the socket path: macOS's paths hold 104 bytes, and its
    /// temporary folder takes half.
    const NAMED: usize = if cfg!(target_vendor = "apple") { 32 } else { 64 };
    /// The first copy's reply to a start it took.
    const TAKEN: u8 = b'y';

    /// The lock and socket path of program `name`, without endings: in the user's runtime
    /// folder (in a sandbox, the app's part of it, shared by every copy). `None` without one. A
    /// name that would make the socket path too long, or holds a slash, becomes a hash.
    pub(crate) fn place(name: &str) -> Option<PathBuf> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|folder| !folder.is_empty());
        #[cfg(target_vendor = "apple")]
        let runtime = runtime.or_else(|| std::env::var_os("TMPDIR").filter(|folder| !folder.is_empty()));
        let mut folder = PathBuf::from(runtime?);
        if let Some(app) = std::env::var_os("FLATPAK_ID").filter(|app| !app.is_empty()) {
            folder = folder.join("app").join(app);
        }
        let plain = name.len() <= NAMED && !name.is_empty() && !name.contains(['/', '\0']);
        let named = if plain { name.to_string() } else { format!("{:016x}", crate::id::hash(name.as_bytes())) };
        Some(folder.join(format!("taut-{named}")))
    }

    /// The lock or socket path: `place` with `ending` appended. (Appended, not replacing an
    /// extension: "org.example.Notes" and "org.example.Mail" are two programs.)
    fn beside(place: &std::path::Path, ending: &str) -> PathBuf {
        let mut path = place.as_os_str().to_owned();
        path.push(ending);
        path.into()
    }

    /// The first copy listens for later starts; a later one hands over its start (`packed`).
    /// See `super::first_copy`.
    pub(crate) fn first_copy(name: &str, packed: &[u8]) -> bool {
        let Some(place) = place(name) else { return true };
        let since = Instant::now();
        while since.elapsed() < PATIENCE {
            if let Some(listener) = take_lock(&place) {
                return listen(listener);
            }
            if hand_over(&place, packed) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    }

    /// Takes the lock if no other copy holds it, clears what a dead copy left, and listens on
    /// the socket, for as long as the program runs. `None` if another copy holds it.
    fn take_lock(place: &std::path::Path) -> Option<UnixListener> {
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(beside(place, ".lock")).ok()?;
        // SAFETY: a lock on a file this program has open.
        if unsafe { flock(lock.as_raw_fd(), ALONE) } != 0 {
            return None;
        }
        let socket = beside(place, ".socket");
        _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).ok()?;
        // Kept open, the lock is held until the program exits; the system releases it then.
        std::mem::forget(lock);
        Some(listener)
    }

    /// Hears later starts on a thread of its own from now on; returns `true` (first copy).
    /// Without a thread, starts go unheard; the program still runs.
    fn listen(listener: UnixListener) -> bool {
        let heard = std::thread::Builder::new().name("taut: later starts".into()).stack_size(64 * 1024);
        _ = heard.spawn(move || listener.incoming().flatten().for_each(hear));
        true
    }

    /// Hears one later start: reads all of it, then replies that it took it.
    fn hear(stream: UnixStream) {
        quiet(&stream);
        _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut bytes = Vec::new();
        if (&stream).take(MOST as u64 + 1).read_to_end(&mut bytes).is_err() {
            return;
        }
        let Some((tokens, start)) = unpacked(&bytes) else { return };
        // SAFETY: one byte from this program's memory, into a socket it holds.
        unsafe { send(stream.as_raw_fd(), &TAKEN, 1, NO_SIGNAL) };
        came(tokens, start);
    }

    /// Hands this start to the first copy; returns whether it was taken.
    fn hand_over(place: &std::path::Path, packed: &[u8]) -> bool {
        let Ok(stream) = UnixStream::connect(beside(place, ".socket")) else { return false };
        quiet(&stream);
        _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut sent = 0;
        while sent < packed.len() {
            // SAFETY: the rest of the bytes, into a socket this program holds.
            let wrote = unsafe { send(stream.as_raw_fd(), packed[sent..].as_ptr(), packed.len() - sent, NO_SIGNAL) };
            if wrote <= 0 {
                return false;
            }
            sent += wrote as usize;
        }
        _ = stream.shutdown(std::net::Shutdown::Write);
        let mut answer = [0u8];
        matches!((&stream).read(&mut answer), Ok(1)) && answer[0] == TAKEN
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Serializes tests that hand over starts: the queue is per process, and one test's app
    /// would take another's.
    pub(crate) fn starts_alone() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts no run took, for a test whose app takes none.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    pub(crate) fn queued() -> std::sync::MutexGuard<'static, Vec<Relaunch>> {
        CAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn start(args: &[&str], folder: &str) -> Relaunch {
        Relaunch { args: args.iter().map(OsString::from).collect(), working_dir: folder.into() }
    }

    #[test]
    fn a_start_is_handed_over_as_bytes_and_read_back_whole() {
        let tokens = Tokens { wayland: "kwin-123".into(), x: "launcher-4_TIME5678".into() };
        let given = start(&["notes.txt", "--new", "", "Über café.txt"], "/home/someone/Documents");
        assert_eq!(unpacked(&packed(&tokens, &given)), Some((tokens, given.clone())));
        assert_eq!(unpacked(&packed(&Tokens::default(), &start(&[], ""))), Some((Tokens::default(), start(&[], ""))));
        // Bytes from another program are trusted in nothing: truncated, longer than they say,
        // over the limit, or missing the folder.
        let bytes = packed(&Tokens::default(), &given);
        for cut in 1..bytes.len() {
            assert!(unpacked(&bytes[..cut]).is_none_or(|(_, read)| read.args.len() < given.args.len()), "cut at {cut}");
        }
        let mut long = bytes.clone();
        long[0] = 0xff;
        assert_eq!(unpacked(&long), None);
        assert_eq!(unpacked(&vec![0; MOST + 4]), None);
        assert_eq!(unpacked(&[0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[cfg(windows)]
    #[test]
    fn words_that_are_not_whole_text_on_windows_are_handed_over_as_they_are() {
        use std::os::windows::ffi::OsStringExt;
        // A Windows file name may hold an unpaired UTF-16 surrogate.
        let odd = OsString::from_wide(&[0x61, 0xD800, 0x62]);
        let given = Relaunch { args: vec![odd], working_dir: "C:\\Users\\someone".into() };
        assert_eq!(unpacked(&packed(&Tokens::default(), &given)).map(|(_, read)| read), Some(given));
    }

    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn words_that_are_not_text_on_linux_are_handed_over_as_they_are() {
        use std::os::unix::ffi::OsStringExt;
        let odd = OsString::from_vec(vec![0x61, 0xff, 0x62]);
        let given = Relaunch { args: vec![odd], working_dir: "/home/someone".into() };
        assert_eq!(unpacked(&packed(&Tokens::default(), &given)).map(|(_, read)| read), Some(given));
    }

    #[test]
    fn starts_that_came_are_heard_once_by_the_next_run() {
        let _alone = starts_alone();
        let mut trial = crate::testing::Trial::new(Vec::new(), |heard: &mut Vec<Relaunch>, ui| {
            heard.extend(ui.relaunches());
            ui.label("The window");
        });
        // That the windows come forward is tested with a whole program, in `shell` and `x11`.
        came(Tokens { wayland: "token".into(), x: String::new() }, start(&["a.txt"], "/copies test one"));
        came(Tokens::default(), start(&["b.txt"], "/copies test two"));
        trial.run();
        trial.run();
        trial.app.retain(|start| start.working_dir.to_string_lossy().starts_with("/copies test"));
        assert_eq!(trial.app, [start(&["a.txt"], "/copies test one"), start(&["b.txt"], "/copies test two")]);
    }
}

/// Single instance on Linux, with real processes: this test program, started again.
#[cfg(all(test, unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod linux_tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    /// Tells a copy of the test program which part it plays: the listening first copy, or a
    /// later start.
    const PART: &str = "TAUT_TEST_COPY";

    /// This test program, started again to run one test as `part`, with the given runtime
    /// folder (one per test).
    fn part(test: &str, part: &str, folder: &std::path::Path) -> Command {
        let mut started = Command::new(std::env::current_exe().unwrap());
        started.args(["--exact", test, "--ignored", "--nocapture", "notes of mine.txt"]).env(PART, part).env("XDG_RUNTIME_DIR", folder);
        started.env_remove("FLATPAK_ID").env("XDG_ACTIVATION_TOKEN", "token-of-the-start").stdout(Stdio::piped());
        started
    }

    #[test]
    fn a_later_start_hands_what_it_was_given_to_the_first_over_a_socket() {
        let folder = std::env::temp_dir().join(format!("taut-copies-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // The first copy says when it listens; a later start, from another folder, hands over
        // its arguments and exits; the first copy prints what it heard.
        let mut first = part("single_instance::linux_tests::the_first_copy", "first", &folder).spawn().unwrap();
        let mut said = BufReader::new(first.stdout.take().unwrap()).lines();
        assert!(said.by_ref().map_while(Result::ok).any(|line| line == "listening"), "the first copy listens");
        let later = part("single_instance::linux_tests::a_later_start", "later", &folder).current_dir("/tmp").output().unwrap();
        assert!(later.status.success(), "{}", String::from_utf8_lossy(&later.stdout));
        let heard: Vec<String> = said.map_while(Result::ok).filter(|line| line.starts_with("heard")).collect();
        assert!(first.wait().unwrap().success());
        assert_eq!(heard, ["heard /tmp notes of mine.txt token-of-the-start"]);
        // With the first copy gone, its leftover socket does not stop the next start from being
        // first.
        let again = part("single_instance::linux_tests::a_first_copy_again", "again", &folder).output().unwrap();
        assert!(again.status.success(), "{}", String::from_utf8_lossy(&again.stdout));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    #[ignore = "a part of the test before it"]
    fn the_first_copy() {
        if std::env::var(PART).as_deref() != Ok("first") {
            return;
        }
        assert!(single_instance("org.example.Tests"));
        println!("listening");
        let since = std::time::Instant::now();
        while since.elapsed() < std::time::Duration::from_secs(20) {
            let starts = std::mem::take(&mut *CAME.lock().unwrap());
            if let Some(start) = starts.first() {
                let tokens = forward().unwrap_or_default();
                println!("heard {} {} {}", start.working_dir.display(), start.args.last().unwrap().to_string_lossy(), tokens.wayland);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no later start came");
    }

    #[test]
    #[ignore = "a part of the test before it"]
    fn a_later_start() {
        if std::env::var(PART).as_deref() == Ok("later") {
            assert!(!single_instance("org.example.Tests"), "the first copy took the start");
        }
    }

    #[test]
    #[ignore = "a part of the test before it"]
    fn a_first_copy_again() {
        if std::env::var(PART).as_deref() == Ok("again") {
            assert!(single_instance("org.example.Tests"), "the copy that held the lock is gone");
        }
    }
}
