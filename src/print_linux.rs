//! Printing on Linux: the desktop portal (`org.freedesktop.portal.Print`) shows its print
//! dialog (`PreparePrint`) over the program's window, then prints the pages, handed over as an
//! open PDF file it reads (`Print`). Pages laid out on asking ([`print_laid`]) are laid out
//! for the paper picked in the dialog.
//!
//! The portal is reached over the program's own connection to the session bus, from a thread of
//! its own: the dialog stays up as long as the person likes while the window goes on. A file
//! descriptor travels beside a message only on a connection that negotiated it at its start;
//! the windows' connection does not (nothing else it asks takes a file). With no bus, no
//! portal, or a portal that cannot print, nothing is printed.
use crate::atspi::{ANSWER, CALL, FAILED, Message, NEWS, Pack, Unpack};
use crate::geom::Size;
use crate::paper::Paper;
use crate::print::{Pages, PrintOutcome, tell};
use std::ffi::{c_int, c_void};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn sendmsg(fd: c_int, message: *const Letter, flags: c_int) -> isize;
    fn recvmsg(fd: c_int, message: *mut Letter, flags: c_int) -> isize;
    fn geteuid() -> u32;
}

/// The portal's bus name, its one object, its print interface, and the path prefix of its
/// requests.
pub(crate) const PORTAL: &str = "org.freedesktop.portal.Desktop";
pub(crate) const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub(crate) const PRINTING: &str = "org.freedesktop.portal.Print";
pub(crate) const REQUEST: &str = "org.freedesktop.portal.Request";
pub(crate) const REQUESTS: &str = "/org/freedesktop/portal/desktop/request";
/// How long the bus and portal have to answer a call. (The dialog itself stays up as long as
/// the person likes.)
const PATIENCE: Duration = Duration::from_secs(10);
/// `MSG_NOSIGNAL`: a write to a closed peer fails instead of killing the program.
const NO_SIGNAL: c_int = 0x4000;

/// `struct msghdr` for `sendmsg` and `recvmsg`: no address, the bytes as one piece, and room
/// for a file descriptor.
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

/// `struct iovec`.
#[repr(C)]
struct Piece {
    at: *mut c_void,
    len: usize,
}

/// The control message: its length, `SOL_SOCKET` (1), `SCM_RIGHTS` (1), and the descriptors.
#[repr(C)]
struct Beside {
    len: usize,
    level: c_int,
    kind: c_int,
    files: [c_int; 4],
}

/// Prints from a thread of its own, through the portal on the session bus, its dialog over the
/// program's window.
pub(crate) fn ask(pages: Pages, title: &str, outcome: Arc<Mutex<Option<PrintOutcome>>>) {
    let (title, parent) = (title.to_string(), crate::print::parent());
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|address| !address.is_empty());
    let address = address.or_else(|| std::env::var("XDG_RUNTIME_DIR").ok().map(|folder| format!("unix:path={folder}/bus")));
    std::thread::spawn(move || tell(&outcome, address.map_or(PrintOutcome::Unable, |address| print_through(&address, pages, &title, &parent))));
}

/// The program's own connection to a bus, which passes file descriptors: what arrived but is
/// not a whole message yet, descriptors that came with it, the last serial sent, and the
/// program's unique name.
pub(crate) struct Line {
    stream: UnixStream,
    come: Vec<u8>,
    pub(crate) files: Vec<OwnedFd>,
    serial: u32,
    pub(crate) me: String,
}

/// A header field: its code, its signature, and its value.
pub(crate) type Field<'a> = (u8, &'a str, FieldValue<'a>);

/// A header field's value: a string (or object path, or signature), or a number.
#[derive(Clone, Copy)]
pub(crate) enum FieldValue<'a> {
    Text(&'a str),
    Number(u32),
}

impl Line {
    /// Connects to the bus at `address` (a path, or on Linux an abstract name): authenticates,
    /// negotiates file descriptors, and says Hello. Nothing if there is no bus there, or it
    /// takes no files.
    pub(crate) fn open(address: &str) -> Option<Line> {
        let stream = address.split(';').find_map(connect)?;
        stream.set_read_timeout(Some(PATIENCE)).ok()?;
        // SAFETY: asks which user the program runs as, which cannot fail.
        let me: String = unsafe { geteuid() }.to_string().bytes().map(|digit| format!("{digit:02x}")).collect();
        let mut line = Line { stream, come: Vec::new(), files: Vec::new(), serial: 0, me: String::new() };
        line.stream.write_all(format!("\0AUTH EXTERNAL {me}\r\nNEGOTIATE_UNIX_FD\r\nBEGIN\r\n").as_bytes()).ok()?;
        let said = line.greeting()?;
        (said.starts_with("OK ") && said.contains("\r\nAGREE_UNIX_FD")).then_some(())?;
        let hello = line.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", &Pack::default(), None);
        let answer = line.answer(hello, Instant::now() + PATIENCE)?;
        line.me = Message::read(&answer)?.body.text()?.to_string();
        Some(line)
    }

    /// The bus's two lines before its first message: authentication accepted, and file
    /// descriptors agreed.
    fn greeting(&mut self) -> Option<String> {
        let mut piece = [0u8; 256];
        while self.come.windows(2).filter(|pair| *pair == b"\r\n").count() < 2 {
            let got = self.stream.read(&mut piece).ok().filter(|got| *got > 0)?;
            self.come.extend_from_slice(&piece[..got]);
            if self.come.starts_with(b"ERROR") || self.come.len() > 4096 {
                return None;
            }
        }
        let end = self.come.windows(2).enumerate().filter(|(_, pair)| *pair == b"\r\n").nth(1)?.0 + 2;
        let said = String::from_utf8_lossy(&self.come[..end]).into_owned();
        self.come.drain(..end);
        Some(said)
    }

    /// Sends a message (its type, fields and body, with a file beside it). Gives back its
    /// serial.
    pub(crate) fn send(&mut self, kind: u8, fields: &[Field], body: &Pack, file: Option<RawFd>) -> u32 {
        self.serial += 1;
        let mut message = Pack::default();
        message.byte(b'l').byte(kind).byte(0).byte(1).u32(body.bytes.len() as u32).u32(self.serial).open(8);
        // A file beside it is declared in a field of its own (`UNIX_FDS`: how many).
        let files = file.map(|_| (9, "u", FieldValue::Number(1)));
        for (code, kind, said) in fields.iter().chain(files.as_ref()) {
            message.pad(8).byte(*code).kinds(kind);
            match (said, *kind) {
                (FieldValue::Text(text), "g") => _ = message.kinds(text),
                (FieldValue::Text(text), _) => _ = message.text(text),
                (FieldValue::Number(number), _) => _ = message.u32(*number),
            }
        }
        message.close().pad(8).bytes.extend_from_slice(&body.bytes);
        self.post(&message.bytes, file);
        self.serial
    }

    /// Calls a method of an object of a program on the bus.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call(&mut self, to: &str, path: &str, interface: &str, member: &str, kinds: &str, body: &Pack, file: Option<RawFd>) -> u32 {
        let mut fields = vec![(1, "o", FieldValue::Text(path)), (2, "s", FieldValue::Text(interface)), (3, "s", FieldValue::Text(member)), (6, "s", FieldValue::Text(to))];
        if !kinds.is_empty() {
            fields.push((8, "g", FieldValue::Text(kinds)));
        }
        self.send(CALL, &fields, body, file)
    }

    /// Writes a message's bytes, with a file beside the first of them.
    fn post(&mut self, bytes: &[u8], file: Option<RawFd>) {
        let mut sent = 0;
        let mut beside = Beside { len: 16 + 4, level: 1, kind: 1, files: [file.unwrap_or(-1), -1, -1, -1] };
        while sent < bytes.len() {
            let mut piece = Piece { at: bytes[sent..].as_ptr().cast_mut().cast(), len: bytes.len() - sent };
            let with = file.is_some() && sent == 0;
            let letter =
                Letter { address: std::ptr::null_mut(), address_len: 0, pieces: &mut piece, piece_count: 1, beside: (&raw mut beside).cast(), beside_len: if with { 24 } else { 0 }, flags: 0 };
            // SAFETY: the bytes and file the letter names outlive the call.
            let done = unsafe { sendmsg(self.stream.as_raw_fd(), &letter, NO_SIGNAL) };
            match done {
                ..0 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
                ..=0 => return,
                _ => sent += done as usize,
            }
        }
    }

    /// The next whole message, waiting until `until`; descriptors that came with it are kept.
    /// Nothing once the bus is gone or time is up.
    pub(crate) fn next(&mut self, until: Instant) -> Option<Vec<u8>> {
        loop {
            if let Some(whole) = self.whole() {
                return Some(whole);
            }
            let left = until.checked_duration_since(Instant::now()).filter(|left| !left.is_zero())?;
            self.stream.set_read_timeout(Some(left)).ok()?;
            self.take()?;
        }
    }

    /// Reads what arrived, with descriptors beside it.
    fn take(&mut self) -> Option<()> {
        let mut room = [0u8; 4096];
        let mut beside = Beside { len: 0, level: 0, kind: 0, files: [-1; 4] };
        let mut piece = Piece { at: room.as_mut_ptr().cast(), len: room.len() };
        let mut letter = Letter { address: std::ptr::null_mut(), address_len: 0, pieces: &mut piece, piece_count: 1, beside: (&raw mut beside).cast(), beside_len: size_of::<Beside>(), flags: 0 };
        // SAFETY: the letter names room that outlives the call, and the system writes no more
        // than it says.
        let got = unsafe { recvmsg(self.stream.as_raw_fd(), &mut letter, 0x4000_0000) };
        if got <= 0 {
            return (got < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted).then_some(());
        }
        self.come.extend_from_slice(&room[..got as usize]);
        if letter.beside_len >= 16 && (beside.level, beside.kind) == (1, 1) {
            let count = ((beside.len.min(letter.beside_len) - 16) / 4).min(4);
            // SAFETY: each is a descriptor the system just gave this program.
            self.files.extend(beside.files[..count].iter().filter(|fd| **fd >= 0).map(|fd| unsafe { OwnedFd::from_raw_fd(*fd) }));
        }
        Some(())
    }

    /// Takes the first message out of what arrived, if whole: 16 bytes, its fields, then its
    /// body, each part aligned to 8 bytes.
    fn whole(&mut self) -> Option<Vec<u8>> {
        let head = self.come.get(..16)?;
        let number = |at: usize| {
            let four = [head[at], head[at + 1], head[at + 2], head[at + 3]];
            if head[0] == b'l' { u32::from_le_bytes(four) } else { u32::from_be_bytes(four) }
        };
        let long = (16 + number(12) as usize).next_multiple_of(8) + number(4) as usize;
        (self.come.len() >= long).then(|| self.come.drain(..long).collect())
    }

    /// The reply to call `serial`, arriving before `until`; anything else (and any descriptor
    /// with it) is dropped, since another program may send descriptors without end. Nothing if
    /// the call failed.
    fn answer(&mut self, serial: u32, until: Instant) -> Option<Vec<u8>> {
        loop {
            let bytes = self.next(until)?;
            self.files.clear();
            let message = Message::read(&bytes)?;
            match message.kind {
                ANSWER if message.answers == serial => return Some(bytes),
                FAILED if message.answers == serial => return None,
                _ => {}
            }
        }
    }
}

/// Opens the socket a bus address names: at a path, or (Linux only) an abstract name.
fn connect(address: &str) -> Option<UnixStream> {
    let keys = address.strip_prefix("unix:")?;
    let value = |key: &str| keys.split(',').find_map(|pair| pair.strip_prefix(key)).map(plain);
    match value("path=") {
        Some(path) => UnixStream::connect(std::ffi::OsString::from(String::from_utf8(path).ok()?)).ok(),
        None => unnamed(&value("abstract=")?),
    }
}

#[cfg(target_os = "linux")]
fn unnamed(name: &[u8]) -> Option<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(name).ok()?).ok()
}
#[cfg(not(target_os = "linux"))]
fn unnamed(_: &[u8]) -> Option<UnixStream> {
    None
}

/// An address value as bytes: `%41` becomes the byte it names.
fn plain(coded: &str) -> Vec<u8> {
    let (coded, mut bytes, mut at) = (coded.as_bytes(), Vec::new(), 0);
    let digit = |at: usize| coded.get(at).and_then(|byte| (*byte as char).to_digit(16));
    while at < coded.len() {
        let byte = if coded[at] == b'%' { digit(at + 1).zip(digit(at + 2)).map(|(high, low)| (high * 16 + low) as u8) } else { None };
        bytes.push(byte.unwrap_or(coded[at]));
        at += if byte.is_some() { 3 } else { 1 };
    }
    bytes
}

/// Prints through the portal on the bus at `address`: its dialog first, over `parent` (as the
/// portal names windows; "" for none), for the first page's size (or A4, for pages laid out on
/// asking); then the pages, laid out for the paper picked where they are on asking, handed over
/// open as a PDF file. Says what came of it.
pub(crate) fn print_through(address: &str, mut pages: Pages, title: &str, parent: &str) -> PrintOutcome {
    let Some(mut line) = Line::open(address) else { return PrintOutcome::Unable };
    let size = match &pages {
        Pages::Ready(paper) => paper.size(0),
        Pages::Laid(_) => Paper::A4,
    };
    let (token, sheet) = match line.request(&prepared(parent, title, [size.w, size.h], "taut_print_1"), "PreparePrint", "ssa{sv}a{sv}a{sv}", "taut_print_1", None) {
        Some((0, token, sheet)) => (token, sheet),
        Some(_) => return PrintOutcome::Closed,
        None => return PrintOutcome::Unable,
    };
    let file = pages.for_sheet(sheet).to_pdf();
    let Some(held) = held_file(&file) else { return PrintOutcome::Unable };
    let mut handed = Pack::default();
    handed.text("").text(title).u32(0).open(8);
    handed.pad(8).text("handle_token").kinds("s").text("taut_print_2");
    handed.pad(8).text("token").kinds("u").u32(token);
    handed.close();
    match line.request(&handed, "Print", "ssha{sv}", "taut_print_2", Some(held.as_raw_fd())) {
        Some((0, ..)) => PrintOutcome::Sent,
        Some(_) => PrintOutcome::Closed,
        None => PrintOutcome::Unable,
    }
}

/// The `PreparePrint` arguments: the parent window, the title, no settings, a page of `size`
/// units (in mm, with orientation), and the request's handle token.
fn prepared(parent: &str, title: &str, size: [f32; 2], token: &str) -> Pack {
    let millimetres = |units: f32| f64::from(units) * 25.4 / 96.0;
    let mut asked = Pack::default();
    asked.text(parent).text(title).open(8).close();
    asked.open(8);
    asked.pad(8).text("Width").kinds("d").f64(millimetres(size[0]));
    asked.pad(8).text("Height").kinds("d").f64(millimetres(size[1]));
    asked.pad(8).text("Orientation").kinds("s").text(if size[0] > size[1] { "landscape" } else { "portrait" });
    asked.close().open(8);
    asked.pad(8).text("handle_token").kinds("s").text(token);
    asked.pad(8).text("modal").kinds("b").u32(1);
    asked.close();
    asked
}

impl Line {
    /// Calls the portal for something it answers later as a request named `token`, subscribing
    /// to its response before calling. Gives back the response code (0: done, 1: cancelled by
    /// the person, 2: other), the token the portal gives (0 if none), and the paper picked, if
    /// said. Nothing if the portal is absent or refused the call.
    fn request(&mut self, asked: &Pack, member: &str, kinds: &str, token: &str, file: Option<RawFd>) -> Option<(u32, u32, Option<Size>)> {
        let path = format!("{REQUESTS}/{}/{token}", self.me.trim_start_matches(':').replace('.', "_"));
        let rule = format!("type='signal',interface='{REQUEST}',member='Response',path='{path}'");
        let mut said = Pack::default();
        said.text(&rule);
        let listen = self.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", "s", &said, None);
        self.answer(listen, Instant::now() + PATIENCE)?;
        let serial = self.call(PORTAL, PORTAL_PATH, PRINTING, member, kinds, asked, file);
        let (mut answered, mut request, patience) = (false, path, Instant::now() + PATIENCE);
        // The reply comes first, with the request's path; then, when the person is done with
        // the dialog, the request's response. The bus gets `PATIENCE` to reply; the person has
        // as long as they like.
        loop {
            let until = if answered { Instant::now() + Duration::from_secs(3600) } else { patience };
            let bytes = self.next(until)?;
            self.files.clear();
            let message = Message::read(&bytes)?;
            match message.kind {
                ANSWER if message.answers == serial => (request, answered) = (message.body.clone().text().map_or(request, str::to_string), true),
                FAILED if message.answers == serial => return None,
                NEWS if message.member == "Response" && message.path == request => return ended(message.body),
                _ => {}
            }
        }
    }
}

/// A request's response code, the portal's "token", and the paper picked, from its `Response`
/// signal: the code, then results as name and variant.
fn ended(mut said: Unpack) -> Option<(u32, u32, Option<Size>)> {
    let how = said.u32()?;
    let end = said.list(8)?;
    let (mut token, mut sheet) = (0, None);
    while said.at < end {
        said.take(8, 0)?;
        let (name, kinds) = (said.text()?, said.kinds()?);
        match (name, kinds) {
            ("token", "u") => token = said.u32()?,
            ("page-setup", "a{sv}") => sheet = paper_of(&mut said),
            _ => said.skip(kinds)?,
        }
    }
    Some((how, token, sheet))
}

/// The paper a page setup names, in units: its width and height (in mm, upright), across where
/// it is turned. Nothing where it doesn't say.
fn paper_of(said: &mut Unpack) -> Option<Size> {
    let end = said.list(8)?;
    let (mut sides, mut across) = ([0.0f64; 2], false);
    while said.at < end {
        said.take(8, 0)?;
        let (name, kinds) = (said.text()?, said.kinds()?);
        match (name, kinds) {
            ("Width", "d") => sides[0] = said.f64()?,
            ("Height", "d") => sides[1] = said.f64()?,
            ("Orientation", "s") => across = said.text()?.ends_with("landscape"),
            _ => said.skip(kinds)?,
        }
    }
    let [w, h] = sides.map(|side| (side * 96.0 / 25.4) as f32);
    (w > 0.0 && h > 0.0).then_some(if across { Size { w: h, h: w } } else { Size { w, h } })
}

/// A file of the program's own holding `bytes`, open for the portal to read from the start:
/// written in the runtime folder and unlinked at once (an open file stays readable). It is
/// created fresh, readable by its owner only: with no runtime folder it goes in the shared
/// temporary folder, where a file of that name may be another user's, or a link to one of this
/// user's.
fn held_file(bytes: &[u8]) -> Option<OwnedFd> {
    use std::os::unix::fs::OpenOptionsExt;
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let folder = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    for _ in 0..16 {
        let path = folder.join(format!("taut-print-{}-{}.pdf", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let Ok(mut file) = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) else { continue };
        let written = file.write_all(bytes).is_ok();
        let opened = std::fs::File::open(&path);
        let _ = std::fs::remove_file(&path);
        return opened.ok().filter(|_| written).map(OwnedFd::from);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi::{ANSWER, CALL, FAILED, Message, NEWS, Pack};
    use crate::portal::tests::OwnBus;
    use crate::print::{Pages, PrintOutcome};
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// How a stand-in portal answers: how its dialog ends (0: printed, 1: closed by the
    /// person), and whether it prints at all (a desktop without printing).
    #[derive(Clone, Copy)]
    struct Stand {
        ends: u32,
        prints: bool,
    }

    /// What a stand-in portal was asked: each call by name, parent, title and options; and the
    /// bytes of each file it was handed.
    #[derive(Default)]
    struct Heard {
        calls: Vec<String>,
        files: Vec<Vec<u8>>,
    }

    /// A portal played by a test thread on a private bus: it takes the portal's name and
    /// answers printing calls.
    fn stand_in(address: &str, stand: Stand) -> (Arc<Mutex<Heard>>, std::thread::JoinHandle<()>) {
        let mut line = Line::open(address).expect("the test's own bus");
        let mut named = Pack::default();
        named.text(PORTAL).u32(0);
        let asked = line.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName", "su", &named, None);
        let since = Instant::now();
        while let Some(bytes) = line.next(since + Duration::from_secs(5)) {
            if Message::read(&bytes).is_some_and(|message| message.kind == ANSWER && message.answers == asked) {
                break;
            }
        }
        let heard = Arc::new(Mutex::new(Heard::default()));
        let theirs = heard.clone();
        let thread = std::thread::spawn(move || {
            let mut ended = false;
            while let Some(bytes) = line.next(Instant::now() + Duration::from_secs(10)) {
                let Some(message) = Message::read(&bytes) else { continue };
                if message.kind == CALL && message.interface == PRINTING {
                    ended |= play(&mut line, &message, stand, &theirs);
                }
                if ended {
                    return;
                }
            }
        });
        (heard, thread)
    }

    /// What the stand-in does with one call: notes it, replies with its request, and ends the
    /// request as `stand` says (reading a file handed over). Says whether it is done printing.
    fn play(line: &mut Line, call: &Message, stand: Stand, heard: &Mutex<Heard>) -> bool {
        let mut said = call.body;
        let (parent, title) = (said.text().unwrap_or("").to_string(), said.text().unwrap_or("").to_string());
        let mut fields = Vec::new();
        if call.member == "PreparePrint" {
            said.skip("a{sv}").expect("its settings");
            fields.extend(entries(&mut said));
        } else {
            fields.push(format!("file {}", said.u32().unwrap_or(9)));
        }
        fields.extend(entries(&mut said));
        heard.lock().unwrap().calls.push(format!("{} [{parent}] {title} | {}", call.member, fields.join(" | ")));
        let token = fields.iter().find_map(|entry| entry.strip_prefix("handle_token=")).unwrap_or("none").to_string();
        let request = format!("{REQUESTS}/{}/{token}", call.sender.trim_start_matches(':').replace('.', "_"));
        let sender = call.sender.to_string();
        if !stand.prints {
            let mut why = Pack::default();
            why.text("No printing here");
            line.send(
                FAILED,
                &[
                    (4, "s", FieldValue::Text("org.freedesktop.DBus.Error.UnknownMethod")),
                    (5, "u", FieldValue::Number(call.serial)),
                    (6, "s", FieldValue::Text(&sender)),
                    (8, "g", FieldValue::Text("s")),
                ],
                &why,
                None,
            );
            return true;
        }
        if call.member == "Print" {
            let mut file = std::fs::File::from(line.files.remove(0));
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            heard.lock().unwrap().files.push(bytes);
        }
        let mut path = Pack::default();
        path.text(&request);
        line.send(ANSWER, &[(5, "u", FieldValue::Number(call.serial)), (6, "s", FieldValue::Text(&sender)), (8, "g", FieldValue::Text("o"))], &path, None);
        let mut end = Pack::default();
        let how = if call.member == "PreparePrint" { stand.ends } else { 0 };
        end.u32(how).open(8);
        end.pad(8).text("token").kinds("u").u32(7);
        end.pad(8).text("settings").kinds("a{sv}").open(8).close();
        // (US Letter, turned.)
        end.pad(8).text("page-setup").kinds("a{sv}").open(8);
        end.pad(8).text("Width").kinds("d").f64(215.9);
        end.pad(8).text("Height").kinds("d").f64(279.4);
        end.pad(8).text("Orientation").kinds("s").text("landscape");
        end.close().close();
        let fields = [
            (1, "o", FieldValue::Text(&request)),
            (2, "s", FieldValue::Text(REQUEST)),
            (3, "s", FieldValue::Text("Response")),
            (6, "s", FieldValue::Text(&sender)),
            (8, "g", FieldValue::Text("ua{sv}")),
        ];
        line.send(NEWS, &fields, &end, None);
        call.member == "Print" || how != 0
    }

    /// The names and plain values of an options dictionary a portal is handed, as `name=value`.
    fn entries(said: &mut crate::atspi::Unpack) -> Vec<String> {
        let mut found = Vec::new();
        let Some(end) = said.list(8) else { return found };
        while said.at < end {
            said.take(8, 0);
            let (Some(name), Some(kinds)) = (said.text(), said.kinds()) else { break };
            let value = match kinds {
                "s" => said.text().unwrap_or("").to_string(),
                "u" | "b" => said.u32().unwrap_or(0).to_string(),
                "d" => format!("{:.1}", said.f64().unwrap_or(0.0)),
                _ => (said.skip(kinds), "?".to_string()).1,
            };
            found.push(format!("{name}={value}"));
        }
        found
    }

    /// Pages of a size, with a rectangle on.
    fn pages(size: Size) -> Paper {
        let mut paper = Paper::new();
        paper.page(size);
        paper.rect(crate::geom::rect(10.0, 10.0, 50.0, 50.0), crate::color::Color::BLACK);
        paper
    }

    #[test]
    fn the_portal_shows_its_dialog_and_is_handed_the_pages_as_a_file() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let (heard, thread) = stand_in(&own.address, Stand { ends: 0, prints: true });
        let file = pages(Paper::A4).to_pdf();
        assert_eq!(print_through(&own.address, Pages::Ready(Box::new(pages(Paper::A4))), "A report", "x11:2a00007"), PrintOutcome::Sent);
        thread.join().unwrap();
        let heard = heard.lock().unwrap();
        assert_eq!(heard.files, [file], "the portal read the file it was handed, all of it");
        assert_eq!(
            heard.calls,
            [
                "PreparePrint [x11:2a00007] A report | Width=210.0 | Height=297.0 | Orientation=portrait | handle_token=taut_print_1 | modal=1",
                "Print [] A report | file 0 | handle_token=taut_print_2 | token=7",
            ]
        );
    }

    #[test]
    fn pages_are_laid_out_for_the_paper_picked() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let (heard, thread) = stand_in(&own.address, Stand { ends: 0, prints: true });
        let asked = Arc::new(Mutex::new(Vec::new()));
        let told = asked.clone();
        let laid = Pages::Laid(Box::new(move |sheet| {
            told.lock().unwrap().push(sheet);
            pages(sheet)
        }));
        assert_eq!(print_through(&own.address, laid, "Laid", ""), PrintOutcome::Sent);
        thread.join().unwrap();
        // The picked paper is Letter, turned: laid out across, and only once.
        let sheet = asked.lock().unwrap()[0];
        assert!((sheet.w - 1056.0).abs() < 0.1 && (sheet.h - 816.0).abs() < 0.1 && asked.lock().unwrap().len() == 1, "{sheet:?}");
        assert_eq!(heard.lock().unwrap().files, [pages(sheet).to_pdf()]);
        assert!(heard.lock().unwrap().calls[0].contains("Width=210.0 | Height=297.0"), "the dialog offered A4 first");
    }

    #[test]
    fn a_dialog_the_person_closes_prints_nothing() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let (heard, thread) = stand_in(&own.address, Stand { ends: 1, prints: true });
        assert_eq!(print_through(&own.address, Pages::Ready(Box::new(pages(Size { w: 816.0, h: 600.0 }))), "Closed", ""), PrintOutcome::Closed);
        thread.join().unwrap();
        let heard = heard.lock().unwrap();
        assert_eq!((heard.calls.len(), heard.files.len()), (1, 0), "only the dialog was asked for");
        assert!(heard.calls[0].contains("Orientation=landscape"), "{}", heard.calls[0]);
    }

    #[test]
    fn where_nothing_prints_nothing_is_printed() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        // No portal on the bus; a portal without printing; and no bus.
        let since = Instant::now();
        let page = || Pages::Ready(Box::new(pages(Size { w: 100.0, h: 100.0 })));
        assert_eq!(print_through(&own.address, page(), "Nobody", ""), PrintOutcome::Unable);
        assert!(since.elapsed() < Duration::from_secs(5), "the bus says at once that nobody has the name");
        let (_, thread) = stand_in(&own.address, Stand { ends: 0, prints: false });
        assert_eq!(print_through(&own.address, page(), "No printing", ""), PrintOutcome::Unable);
        thread.join().unwrap();
        assert_eq!(print_through("unix:path=/nonexistent/bus", page(), "No bus", ""), PrintOutcome::Unable);
    }

    #[test]
    fn the_file_handed_to_the_portal_is_made_anew_and_read_by_its_owner_alone() {
        // With no runtime folder the file goes in the shared temporary folder: so in a process
        // of its own, whose temporary folder is the test's.
        let folder = std::env::temp_dir().join(format!("taut-held-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let test = "print_linux::tests::a_file_made_where_everyone_makes_them";
        let ran = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--ignored", "--nocapture"])
            .env_remove("XDG_RUNTIME_DIR")
            .env("TMPDIR", &folder)
            .env("TAUT_TEST_HELD", "1")
            .output()
            .unwrap();
        _ = std::fs::remove_dir_all(&folder);
        let said = String::from_utf8_lossy(&ran.stdout);
        assert!(ran.status.success() && said.contains("1 passed"), "{said}{}", String::from_utf8_lossy(&ran.stderr));
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own"]
    fn a_file_made_where_everyone_makes_them() {
        use std::os::unix::fs::PermissionsExt;
        if std::env::var_os("TAUT_TEST_HELD").is_none() {
            return;
        }
        // What another user left where the file goes: a link to a file of this user's.
        let folder = std::env::temp_dir();
        let kept = folder.join("kept.txt");
        std::fs::write(&kept, b"kept").unwrap();
        for n in 0..4 {
            std::os::unix::fs::symlink(&kept, folder.join(format!("taut-print-{}-{n}.pdf", std::process::id()))).unwrap();
        }
        let held = std::fs::File::from(held_file(b"%PDF the pages").expect("a file is made"));
        assert_eq!(std::fs::read(&kept).unwrap(), b"kept", "the file a link leads to is untouched");
        assert_eq!(held.metadata().unwrap().permissions().mode() & 0o777, 0o600, "only its owner reads it");
        let mut read = Vec::new();
        (&held).read_to_end(&mut read).unwrap();
        assert_eq!(read, b"%PDF the pages");
    }
}
