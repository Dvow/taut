//! Screen reader support on Linux: AT-SPI, through which Orca and other assistive tools query
//! and operate windows. They talk over D-Bus, implemented here by hand: a socket, and the D-Bus
//! wire format.
//!
//! There are two buses. On the session bus, a service (`org.a11y.Bus`) says whether anyone
//! listens, and where the accessibility bus is. (The windows own the session bus connection and
//! pass its messages here; the portal is reached over it too, in `portal`.) Only while someone
//! listens is this program on the accessibility bus. There it registers with the registry and
//! answers queries about its windows: one object for the application, one per window, and one
//! per entry of the UI's accessibility list ([`Ui::said`]). It answers from a copy of that list
//! taken after every run, needing nothing of the window. Requested actions (press, type, set a
//! slider) are performed as keyboard input would. After each run it sends the events a screen
//! reader needs: focus moves, changed names or states, children added and removed.
//!
//! Nothing blocks. Sockets are only read for what has arrived, a partial message is kept until
//! the rest arrives, and a dead bus is dropped: then nobody listens, that's all. A program
//! nobody listens to has one socket to the session bus and no state; without a session bus,
//! nothing.
use crate::geom::pt;
use crate::id::Id;
use crate::told::{self, Role, Said};
use crate::ui::Ui;
use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::net::UnixStream;

unsafe extern "C" {
    fn geteuid() -> u32;
}

/// The largest message accepted, and the most bytes queued to send.
const MOST: usize = 1 << 24;
/// How many reads a bus gets per turn, so a bus that never stops talking can't starve the
/// windows.
const PIECES: u8 = 16;

/// Message types: method call, method return, error, signal.
pub(crate) const CALL: u8 = 1;
pub(crate) const ANSWER: u8 = 2;
pub(crate) const FAILED: u8 = 3;
pub(crate) const NEWS: u8 = 4;

/// The bus itself, called like any peer; and the interfaces every object has: properties, and
/// peer (liveness).
pub(crate) const BUS: &str = "org.freedesktop.DBus";
pub(crate) const BUS_PATH: &str = "/org/freedesktop/DBus";
pub(crate) const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const PEER: &str = "org.freedesktop.DBus.Peer";
/// The session bus service that says whether anyone listens, and where the accessibility bus
/// is.
const LAUNCHER: &str = "org.a11y.Bus";
const LAUNCHER_PATH: &str = "/org/a11y/bus";
/// The accessibility bus registry: knows which applications exist and which screen reader wants
/// which events.
const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
/// Object paths: the application root, "null", and the prefix of window and element paths.
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const NOTHING: &str = "/org/a11y/atspi/null";
const THINGS: &str = "/org/a11y/atspi/accessible/";
/// The AT-SPI interfaces an object may implement.
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const APPLICATION: &str = "org.a11y.atspi.Application";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const VALUE: &str = "org.a11y.atspi.Value";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE: &str = "org.a11y.atspi.EditableText";
const TABLE: &str = "org.a11y.atspi.Table";
const TABLE_CELL: &str = "org.a11y.atspi.TableCell";
const SELECTION: &str = "org.a11y.atspi.Selection";
/// Each interface's properties, for `GetAll`.
const NAMED: [(&str, &[&str]); 8] = [
    (ACCESSIBLE, &["Name", "Description", "Parent", "ChildCount", "Locale", "AccessibleId", "HelpText"]),
    (APPLICATION, &["ToolkitName", "Version", "ToolkitVersion", "AtspiVersion", "Id"]),
    (ACTION, &["NActions"]),
    (VALUE, &["MinimumValue", "MaximumValue", "MinimumIncrement", "CurrentValue", "Text"]),
    (TEXT, &["CharacterCount", "CaretOffset"]),
    (TABLE, &["NRows", "NColumns", "Caption", "Summary", "NSelectedRows", "NSelectedColumns"]),
    (TABLE_CELL, &["ColumnSpan", "Position", "RowSpan", "Table"]),
    (SELECTION, &["NSelectedChildren"]),
];
/// Error replies: "no such object" (the name Orca recognizes for a defunct object), and "not
/// supported".
const GONE: (&str, &str) = ("org.freedesktop.DBus.Error.UnknownObject", "Object does not exist at path");
const UNKNOWN: (&str, &str) = ("org.freedesktop.DBus.Error.UnknownMethod", "No such method, or not with these values, at path");
/// Events sent whether or not a screen reader subscribed, as long as one subscribed to
/// anything: children changed, state changed, name and role changed. Screen readers rely on
/// these to keep their cache correct.
const ALWAYS: [&str; 4] = ["object:childrenchanged:", "object:statechanged:", "object:propertychange:accessiblename", "object:propertychange:accessiblerole"];

/// The AT-SPI states reported, by their values in `atspi-constants.h`.
const ACTIVE: u32 = 1;
const CHECKED: u32 = 4;
const COLLAPSED: u32 = 5;
const EDITABLE_NOW: u32 = 7;
const ENABLED: u32 = 8;
const EXPANDABLE: u32 = 9;
const EXPANDED: u32 = 10;
const FOCUSABLE: u32 = 11;
const FOCUSED: u32 = 12;
const MODAL: u32 = 16;
const MULTISELECTABLE: u32 = 18;
const PRESSED: u32 = 20;
const RESIZABLE: u32 = 21;
const SELECTABLE: u32 = 22;
const SELECTED: u32 = 23;
const SENSITIVE: u32 = 24;
const SHOWING: u32 = 25;
const VISIBLE: u32 = 30;
const CHECKABLE: u32 = 41;
const READ_ONLY: u32 = 43;

/// Values in D-Bus wire format: each aligned to its size, numbers little-endian, a string with
/// its length before and a NUL after.
#[derive(Default)]
pub(crate) struct Pack {
    pub(crate) bytes: Vec<u8>,
    /// Arrays being written: where each one's length goes, and where its first element starts.
    lists: Vec<[usize; 2]>,
}

impl Pack {
    pub(crate) fn pad(&mut self, to: usize) -> &mut Self {
        self.bytes.resize(self.bytes.len().next_multiple_of(to), 0);
        self
    }
    pub(crate) fn byte(&mut self, value: u8) -> &mut Self {
        self.bytes.push(value);
        self
    }
    pub(crate) fn u32(&mut self, value: u32) -> &mut Self {
        self.pad(4).bytes.extend(value.to_le_bytes());
        self
    }
    fn i32(&mut self, value: i32) -> &mut Self {
        self.u32(value as u32)
    }
    fn i32s(&mut self, values: &[i32]) -> &mut Self {
        values.iter().fold(self, |pack, value| pack.i32(*value))
    }
    pub(crate) fn f64(&mut self, value: f64) -> &mut Self {
        self.pad(8).bytes.extend(value.to_le_bytes());
        self
    }
    /// A string or object path. (D-Bus strings have no NUL: one is dropped.)
    pub(crate) fn text(&mut self, text: &str) -> &mut Self {
        let from = self.u32(0).bytes.len();
        self.bytes.extend(text.bytes().filter(|byte| *byte != 0));
        let len = (self.bytes.len() - from) as u32;
        self.bytes[from - 4..from].copy_from_slice(&len.to_le_bytes());
        self.byte(0)
    }
    /// A signature, as D-Bus writes it: `s` string, `i` and `u` integers, `(so)` object
    /// reference, `a` array of what follows.
    pub(crate) fn kinds(&mut self, kinds: &str) -> &mut Self {
        self.byte(kinds.len() as u8).bytes.extend(kinds.bytes());
        self.byte(0)
    }
    /// Starts an array of elements aligned to `to` bytes; `close` ends it and writes its
    /// length.
    pub(crate) fn open(&mut self, to: usize) -> &mut Self {
        let length = self.u32(0).bytes.len() - 4;
        let from = self.pad(to).bytes.len();
        self.lists.push([length, from]);
        self
    }
    pub(crate) fn close(&mut self) -> &mut Self {
        let [length, from] = self.lists.pop().unwrap_or_default();
        let len = (self.bytes.len() - from) as u32;
        self.bytes[length..length + 4].copy_from_slice(&len.to_le_bytes());
        self
    }
    /// An object reference: the peer's bus name, and the object path.
    fn object(&mut self, name: &str, path: &str) -> &mut Self {
        self.pad(8).text(name).text(path)
    }
}

/// A variant of a basic type: any integer, or a string.
#[derive(Debug, PartialEq)]
pub(crate) enum Value<'a> {
    Number(f64),
    Text(&'a str),
}

/// Values decoded from wire format. All of it comes from another process, so nothing is
/// trusted: every length is checked against the bytes present, and anything that doesn't fit is
/// no value.
#[derive(Clone, Copy)]
pub(crate) struct Unpack<'a> {
    bytes: &'a [u8],
    pub(crate) at: usize,
    /// Numbers are little-endian. (The sender's byte order flag says.)
    little: bool,
}

impl<'a> Unpack<'a> {
    /// The next `len` bytes, from the next `to`-byte alignment.
    pub(crate) fn take(&mut self, to: usize, len: usize) -> Option<&'a [u8]> {
        let from = self.at.checked_next_multiple_of(to)?;
        let taken = self.bytes.get(from..from.checked_add(len)?)?;
        self.at = from + len;
        Some(taken)
    }
    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1, 1)?[0])
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        let bytes = self.take(4, 4)?.try_into().ok()?;
        Some(if self.little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|value| value as i32)
    }
    pub(crate) fn f64(&mut self) -> Option<f64> {
        let bytes = self.take(8, 8)?.try_into().ok()?;
        Some(if self.little { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) })
    }
    pub(crate) fn text(&mut self) -> Option<&'a str> {
        let len = self.u32()? as usize;
        let text = std::str::from_utf8(self.take(1, len)?).ok()?;
        (self.byte()? == 0).then_some(text)
    }
    pub(crate) fn kinds(&mut self) -> Option<&'a str> {
        let len = self.byte()? as usize;
        let kinds = std::str::from_utf8(self.take(1, len)?).ok()?;
        (self.byte()? == 0).then_some(kinds)
    }
    /// An array of elements aligned to `to` starts here: returns where it ends.
    pub(crate) fn list(&mut self, to: usize) -> Option<usize> {
        let len = self.u32()? as usize;
        self.take(to, 0)?;
        self.at.checked_add(len).filter(|end| *end <= self.bytes.len())
    }
    /// A variant, if of a single basic type.
    pub(crate) fn value(&mut self) -> Option<Value<'a>> {
        Some(match self.kinds()? {
            "y" => Value::Number(self.byte()? as f64),
            "b" | "u" => Value::Number(self.u32()? as f64),
            "i" => Value::Number(self.i32()? as f64),
            "d" => Value::Number(self.f64()?),
            "s" | "o" => Value::Text(self.text()?),
            "g" => Value::Text(self.kinds()?),
            _ => return None,
        })
    }
    /// Skips values of the given signature, whatever they are: what a peer sends that isn't
    /// needed. Arrays are skipped by length; structs only align their contents. A variant names
    /// its own signature: that is skipped next, then the rest. `None` if the bytes don't match
    /// the signature.
    pub(crate) fn skip(&mut self, kinds: &'a str) -> Option<()> {
        let mut left = vec![kinds.as_bytes()];
        while let Some(kinds) = left.pop() {
            let Some((kind, rest)) = kinds.split_first() else { continue };
            let said = if *kind == b'v' { self.kinds()? } else { "" };
            let rest = if *kind == b'a' { self.skip_list(rest)? } else { rest };
            match kind {
                b'a' | b'v' | b')' | b'}' => {}
                b'(' | b'{' => _ = self.take(8, 0)?,
                b's' | b'o' => _ = self.text()?,
                b'g' => _ = self.kinds()?,
                b'y' | b'n' | b'q' | b'b' | b'i' | b'u' | b'h' | b'x' | b't' | b'd' => _ = self.take(lined_up(*kind), lined_up(*kind))?,
                _ => return None,
            }
            left.extend([rest, said.as_bytes()]);
        }
        Some(())
    }
    /// Skips an array by its length: `of` starts with its element type. Returns the signature
    /// after that type.
    fn skip_list(&mut self, of: &'a [u8]) -> Option<&'a [u8]> {
        self.at = self.list(lined_up(*of.first()?))?;
        of.get(one_kind(of)?..)
    }
}

/// The alignment of a type, by its first character: also a basic number type's size.
fn lined_up(kind: u8) -> usize {
    match kind {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

/// The length of the first complete type in a signature: one character, an array with its
/// element type, or a whole bracketed struct. `None` if brackets don't match.
fn one_kind(kinds: &[u8]) -> Option<usize> {
    let (mut open, mut len) = (0usize, 0);
    for kind in kinds {
        open = match kind {
            b'(' | b'{' => open + 1,
            b')' | b'}' => open.checked_sub(1)?,
            _ => open,
        };
        len += 1;
        if open == 0 && *kind != b'a' {
            return Some(len);
        }
    }
    None
}

/// A received message: type, sender, target, and body.
pub(crate) struct Message<'a> {
    pub(crate) kind: u8,
    /// Its serial; and, for a reply, the serial it replies to.
    pub(crate) serial: u32,
    pub(crate) answers: u32,
    /// The sender expects a reply.
    waited_on: bool,
    pub(crate) path: &'a str,
    pub(crate) interface: &'a str,
    pub(crate) member: &'a str,
    /// For an error: its name.
    pub(crate) failure: &'a str,
    pub(crate) sender: &'a str,
    pub(crate) body: Unpack<'a>,
}

impl<'a> Message<'a> {
    /// The total length of the message starting with these 16 bytes. `None` if no message
    /// starts so.
    fn span(start: &[u8]) -> Option<usize> {
        let mut head = Unpack { bytes: start.get(..16)?, at: 4, little: start[0] == b'l' };
        let (body, _, fields) = (head.u32()? as usize, head.u32()?, head.u32()? as usize);
        let known = matches!(start[0], b'l' | b'B') && start[3] == 1;
        16usize.checked_add(fields)?.checked_next_multiple_of(8)?.checked_add(body).filter(|_| known)
    }

    /// Parses a whole message. `None` if malformed.
    pub(crate) fn read(bytes: &'a [u8]) -> Option<Self> {
        let little = *bytes.first()? == b'l';
        let mut head = Unpack { bytes, at: 1, little };
        let (kind, flags, _, body, serial) = (head.byte()?, head.byte()?, head.byte()?, head.u32()? as usize, head.u32()?);
        let end = head.list(8)?;
        let empty = Unpack { bytes: &[], at: 0, little };
        let mut message = Message { kind, serial, answers: 0, waited_on: flags & 1 == 0, path: "", interface: "", member: "", failure: "", sender: "", body: empty };
        // The header is an array of numbered fields, each a variant.
        while head.at < end {
            match (head.take(8, 1)?[0], head.value()?) {
                (1, Value::Text(path)) => message.path = path,
                (2, Value::Text(interface)) => message.interface = interface,
                (3, Value::Text(member)) => message.member = member,
                (4, Value::Text(failure)) => message.failure = failure,
                (5, Value::Number(serial)) => message.answers = serial as u32,
                (7, Value::Text(sender)) => message.sender = sender,
                _ => {}
            }
        }
        let from = end.checked_next_multiple_of(8)?;
        message.body.bytes = bytes.get(from..from.checked_add(body)?)?;
        Some(message)
    }
}

/// A message to send: its type and target. (Empty fields are omitted.)
#[derive(Default)]
pub(crate) struct Header<'a> {
    pub(crate) kind: u8,
    /// 1: NO_REPLY_EXPECTED. 2: NO_AUTO_START.
    pub(crate) flags: u8,
    pub(crate) path: &'a str,
    pub(crate) interface: &'a str,
    pub(crate) member: &'a str,
    /// For an error: its name.
    pub(crate) failure: &'a str,
    /// For a reply: the serial it replies to.
    pub(crate) answers: u32,
    pub(crate) to: &'a str,
    /// The body's signature.
    pub(crate) kinds: &'a str,
}

impl Header<'_> {
    /// The message as bytes, with its serial and body.
    fn pack(&self, serial: u32, body: &[u8]) -> Vec<u8> {
        let mut message = Pack::default();
        message.byte(b'l').byte(self.kind).byte(self.flags).byte(1).u32(body.len() as u32).u32(serial).open(8);
        for (code, kind, text) in [(1, "o", self.path), (2, "s", self.interface), (3, "s", self.member), (4, "s", self.failure), (6, "s", self.to)] {
            if !text.is_empty() {
                message.pad(8).byte(code).kinds(kind).text(text);
            }
        }
        if self.answers != 0 {
            message.pad(8).byte(5).kinds("u").u32(self.answers);
        }
        if !self.kinds.is_empty() {
            message.pad(8).byte(8).kinds("g").kinds(self.kinds);
        }
        message.close().pad(8).bytes.extend_from_slice(body);
        message.bytes
    }
}

/// Connects to one bus address. Desktop buses are at a path, or an abstract socket name (Linux
/// only); other transports, such as TCP, are skipped.
fn connect(address: &str) -> Option<UnixStream> {
    let keys = address.strip_prefix("unix:")?;
    let value = |key: &str| keys.split(',').find_map(|pair| pair.strip_prefix(key)).map(plain);
    match value("path=") {
        Some(path) => UnixStream::connect(OsString::from_vec(path)).ok(),
        None => connect_unnamed(&value("abstract=")?),
    }
}

#[cfg(target_os = "linux")]
fn connect_unnamed(name: &[u8]) -> Option<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(name).ok()?).ok()
}
#[cfg(not(target_os = "linux"))]
fn connect_unnamed(_: &[u8]) -> Option<UnixStream> {
    None
}

/// An address value unescaped: `%41` becomes the byte it encodes.
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

/// A bus connection: the socket, and the data queued each way.
pub(crate) struct Bus {
    stream: UnixStream,
    /// Received data not yet a whole message.
    come: Vec<u8>,
    /// Data to send, once the socket accepts it.
    go: Vec<u8>,
    /// The bus completed authentication: from now on, data is messages.
    greeted: bool,
    /// Bytes still to skip of an oversized message.
    pass: usize,
    /// Reads left this turn.
    pieces: u8,
    /// The last serial sent.
    serial: u32,
    /// The bus is gone, or sent something no bus sends.
    pub(crate) dead: bool,
}

impl Bus {
    /// Connects to the session bus. `None` if there is none.
    pub(crate) fn session() -> Option<Self> {
        let named = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|address| !address.is_empty());
        // A session not naming its bus has it at this path, if anywhere.
        let usual = || UnixStream::connect(std::path::Path::new(&std::env::var_os("XDG_RUNTIME_DIR")?).join("bus")).ok().and_then(Bus::over);
        named.map_or_else(usual, |address| Bus::open(&address))
    }

    /// Connects to the first reachable bus of a `;`-separated address list. `None` if none is.
    pub(crate) fn open(addresses: &str) -> Option<Self> {
        Bus::over(addresses.split(';').find_map(connect)?)
    }

    /// Starts a connection over a socket: authenticates as this process's user (the uid's
    /// digits, hex-encoded), which the bus checks against the socket's credentials; sends
    /// BEGIN; and calls `Hello`, the first call on any bus. All of it goes at once, with
    /// whatever is called next right behind: a bus processes in order, so nothing waits, now or
    /// later, and what a program asks at startup is answered while it sets up its windows.
    pub(crate) fn over(stream: UnixStream) -> Option<Self> {
        stream.set_nonblocking(true).ok()?;
        // SAFETY: geteuid cannot fail.
        let me: String = unsafe { geteuid() }.to_string().bytes().map(|digit| format!("{digit:02x}")).collect();
        let go = format!("\0AUTH EXTERNAL {me}\r\nBEGIN\r\n").into_bytes();
        let mut bus = Bus { stream, come: Vec::new(), go, greeted: false, pass: 0, pieces: 0, serial: 0, dead: false };
        bus.ask("Hello", "");
        Some(bus)
    }

    /// Sends a message, or queues it until the socket has room. Returns its serial: a reply
    /// names it.
    pub(crate) fn send(&mut self, header: Header, body: &[u8]) -> u32 {
        self.serial += 1;
        self.go.extend(header.pack(self.serial, body));
        self.flush();
        self.serial
    }

    /// Writes as much queued data as the socket accepts. (A bus that accepts nothing until more
    /// than the largest message is queued is dropped.)
    fn flush(&mut self) {
        while !self.go.is_empty() && !self.dead {
            match self.stream.write(&self.go) {
                Ok(0) => self.dead = true,
                Ok(gone) => drop(self.go.drain(..gone)),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return self.dead = error.kind() != ErrorKind::WouldBlock || self.go.len() > MOST,
            }
        }
    }

    /// The socket to poll, and the events: readable, and writable when data is queued.
    pub(crate) fn waits(&self) -> (i32, i16) {
        (self.stream.as_raw_fd(), if self.go.is_empty() { 1 } else { 1 | 4 })
    }

    /// Reads what arrived: each whole message goes to `hear`, with the bus to reply on. Then
    /// queued data is sent. (Malformed data is skipped.)
    pub(crate) fn turn(&mut self, hear: &mut dyn FnMut(&mut Bus, &Message)) {
        self.pieces = PIECES;
        while let Some(bytes) = self.message() {
            Message::read(&bytes).iter().for_each(|message| hear(self, message));
        }
        self.flush();
    }

    /// The next whole message received. `None` if none, or the rest is still to arrive: what
    /// there is, is kept.
    fn message(&mut self) -> Option<Vec<u8>> {
        let mut piece = [0; 4096];
        while !self.dead {
            if let Some(whole) = self.whole() {
                return Some(whole);
            }
            if self.dead || self.pieces == 0 {
                break;
            }
            match self.stream.read(&mut piece) {
                Ok(0) => self.dead = true,
                Ok(len) => self.keep(&piece[..len]),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => (self.dead, self.pieces) = (error.kind() != ErrorKind::WouldBlock, 0),
            }
        }
        None
    }

    /// Keeps a received chunk, except what belongs to an oversized message.
    fn keep(&mut self, piece: &[u8]) {
        let passed = self.pass.min(piece.len());
        (self.pass, self.pieces) = (self.pass - passed, self.pieces - 1);
        self.come.extend_from_slice(&piece[passed..]);
    }

    /// Takes the first message out of the received data, if all of it is there.
    fn whole(&mut self) -> Option<Vec<u8>> {
        if !self.greeted {
            self.greet();
        }
        while self.greeted && self.come.len() >= 16 {
            let Some(len) = Message::span(&self.come) else {
                self.dead = true;
                break;
            };
            if len <= MOST {
                let rest = (self.come.len() >= len).then(|| self.come.split_off(len))?;
                return Some(std::mem::replace(&mut self.come, rest));
            }
            // An oversized one is skipped, nothing of it kept.
            let here = len.min(self.come.len());
            (self.pass, self.come) = (len - here, self.come.split_off(here));
        }
        None
    }

    /// Before its first message, a bus sends an `OK` line to confirm authentication. Anything
    /// else means it rejected this program, and took nothing of what was sent.
    fn greet(&mut self) {
        let Some(end) = self.come.windows(2).position(|pair| pair == b"\r\n") else {
            return self.dead = self.come.len() > 4096;
        };
        (self.greeted, self.dead) = (self.come.starts_with(b"OK "), !self.come.starts_with(b"OK "));
        self.come.drain(..end + 2);
    }

    /// Calls a method of a peer on this bus.
    pub(crate) fn call(&mut self, to: &str, path: &str, interface: &str, member: &str, kinds: &str, values: &Pack) -> u32 {
        self.send(Header { kind: CALL, to, path, interface, member, kinds, ..Default::default() }, &values.bytes)
    }

    /// Calls a bus method taking one string: `Hello` (no string), or `AddMatch` with a rule.
    pub(crate) fn ask(&mut self, member: &str, rule: &str) -> u32 {
        let mut values = Pack::default();
        if !rule.is_empty() {
            values.text(rule);
        }
        self.call(BUS, BUS_PATH, BUS, member, if rule.is_empty() { "" } else { "s" }, &values)
    }

    /// Replies to a call, if its sender expects one: with values and their signature, or an
    /// error.
    pub(crate) fn answer(&mut self, call: &Message, kinds: Result<&str, (&str, &str)>, values: &Pack) {
        let (header, mut why) = (Header { answers: call.serial, to: call.sender, ..Default::default() }, Pack::default());
        match kinds {
            _ if !call.waited_on => 0,
            Ok(kinds) => self.send(Header { kind: ANSWER, kinds, ..header }, &values.bytes),
            Err((failure, words)) => self.send(Header { kind: FAILED, failure, kinds: "s", ..header }, &why.text(&format!("{words} \u{201c}{}\u{201d}", call.path)).bytes),
        };
    }
}

/// The AT-SPI role of an element: its value in `atspi-constants.h`, and its name.
fn role_of(said: &Said) -> (u32, &'static str) {
    match (said.role, said.on) {
        (Role::Text, _) => (29, "label"),
        (Role::Button, None) | (Role::Fold, _) => (43, "button"),
        (Role::Button, Some(_)) => (62, "toggle button"),
        (Role::Tick, _) => (7, "check box"),
        (Role::Switch, _) => (130, "switch"),
        (Role::Choice, _) => (44, "radio button"),
        (Role::Tab, _) => (37, "page tab"),
        (Role::Row, _) => (32, "list item"),
        (Role::Slider, _) => (51, "slider"),
        (Role::Spin, _) => (52, "spin button"),
        (Role::TextBox, _) => (79, "entry"),
        (Role::Secret, _) => (40, "password text"),
        (Role::Select, _) => (11, "combo box"),
        (Role::MenuItem, None) => (35, "menu item"),
        (Role::MenuItem, Some(_)) => (8, "check menu item"),
        (Role::Heading, _) => (10, "column header"),
        (Role::Progress, _) => (42, "progress bar"),
        (Role::Group, _) => (39, "panel"),
        (Role::Dialog, _) => (16, "dialog"),
        (Role::Menu, _) => (33, "menu"),
        (Role::MenuBar, _) => (34, "menu bar"),
        (Role::Tabs, _) => (38, "page tab list"),
        (Role::Link, _) => (88, "link"),
        (Role::Table, _) => (55, "table"),
        (Role::Cell, _) => (56, "table cell"),
        (Role::TableRow, _) => (90, "table row"),
        (Role::Document, _) => (94, "document text"),
        (Role::Title, _) => (83, "heading"),
        (Role::Paragraph, _) => (73, "paragraph"),
        (Role::List, _) => (31, "list"),
        (Role::Item, _) => (32, "list item"),
        (Role::Code, _) => (85, "section"),
        (Role::Quote, _) => (105, "block quote"),
        (Role::MenuChoice, _) => (45, "radio menu item"),
        (Role::Submenu, _) => (33, "menu"),
        (Role::ToolBar, _) => (63, "tool bar"),
        (Role::Tree, _) => (65, "tree"),
        (Role::TreeItem, _) => (91, "tree item"),
        (Role::Picture, _) => (27, "image"),
    }
}

/// An element's key bindings as AT-SPI writes them: three parts separated by ";", the last
/// being the global shortcut, modifiers in angle brackets ("Ctrl+O" is ";;<Control>O"). Empty
/// if none.
fn keys_of(said: &Said) -> String {
    let (held, last) = said.keys.rsplit_once('+').map_or(("", said.keys.as_str()), |(held, last)| (held, last));
    let held = held.split('+').filter(|key| !key.is_empty()).map(|key| format!("<{}>", if key.trim() == "Ctrl" { "Control" } else { key.trim() }));
    if said.keys.is_empty() { String::new() } else { format!(";;{}{}", held.collect::<String>(), last.trim()) }
}

/// The actions of a pressable element, by AT-SPI names: "click"; and for a tree item with
/// children, expand or collapse, named as on the web while collapsed and while expanded.
fn ways_of(said: &Said) -> &'static [&'static str] {
    match (said.role, said.open) {
        (Role::TreeItem, Some(false)) => &["click", "expand"],
        (Role::TreeItem, Some(true)) => &["click", "collapse"],
        _ => &["click"],
    }
}

/// Whether an element is pressable: a pressable role, or a selectable table row.
fn pressed(said: &Said) -> bool {
    let kinds = matches!(
        said.role,
        Role::Button
            | Role::Tick
            | Role::Switch
            | Role::Choice
            | Role::Tab
            | Role::Row
            | Role::Fold
            | Role::Select
            | Role::MenuItem
            | Role::MenuChoice
            | Role::Submenu
            | Role::Heading
            | Role::Link
            | Role::TreeItem
    );
    kinds || said.role == Role::TableRow && said.on.is_some()
}

/// An element's AT-SPI state set: a bit per state. `active` says its window has keyboard focus.
fn states_of(said: &Said, active: bool) -> u64 {
    let bit = |state: u32, set: bool| (set as u64) << state;
    let on = said.on == Some(true);
    // Scrolled out of view, it is not showing but still visible, as AT-SPI has it.
    let common = bit(VISIBLE, true) | bit(SHOWING, said.shown) | bit(ENABLED, said.usable) | bit(SENSITIVE, said.usable);
    let keyboard = bit(FOCUSABLE, said.takes_keyboard()) | bit(FOCUSED, said.focused && active);
    let own = match said.role {
        Role::Tick | Role::Switch | Role::Choice | Role::MenuItem | Role::MenuChoice => bit(CHECKABLE, said.on.is_some()) | bit(CHECKED, on),
        Role::Tab | Role::Row => bit(SELECTABLE, true) | bit(SELECTED, on),
        Role::TableRow => bit(SELECTABLE, said.on.is_some()) | bit(SELECTED, on),
        // A tree item is selectable and expandable, each where it can be.
        Role::TreeItem => {
            let opens = said.open.is_some();
            bit(SELECTABLE, said.on.is_some()) | bit(SELECTED, on) | bit(EXPANDABLE, opens) | bit(EXPANDED, said.open == Some(true)) | bit(COLLAPSED, said.open == Some(false))
        }
        Role::Tree => bit(MULTISELECTABLE, on),
        Role::Fold | Role::Select | Role::Submenu => bit(EXPANDABLE, true) | bit(EXPANDED, on) | bit(COLLAPSED, !on),
        Role::Button => bit(PRESSED, on),
        // A text box with a suggestion list expands as the list opens.
        Role::TextBox | Role::Secret => bit(EDITABLE_NOW, said.usable) | bit(EXPANDABLE, said.on.is_some()) | bit(EXPANDED, on) | bit(COLLAPSED, said.on == Some(false)),
        Role::Progress => bit(READ_ONLY, true),
        Role::Dialog => bit(MODAL, true),
        _ => 0,
    };
    common | keyboard | own
}

/// The attribute run containing character `letter` of a `count`-character text, and whether it
/// is a misspelled word (of `wrong`, in order): that word, or the text between the misspellings
/// around it.
fn looks(wrong: &[[u32; 2]], letter: i32, count: i32) -> ([i32; 2], bool) {
    let at = letter.clamp(0, count) as u32;
    let after = wrong.partition_point(|[_, end]| *end <= at);
    match wrong.get(after) {
        Some([start, end]) if *start <= at => ([*start as i32, *end as i32], true),
        next => ([after.checked_sub(1).map_or(0, |before| wrong[before][1] as i32), next.map_or(count, |[start, _]| *start as i32)], false),
    }
}

/// The byte offset of character `letter`. (AT-SPI counts characters; negative means the end, as
/// does past it.)
fn byte_at(text: &str, letter: i32) -> usize {
    text.char_indices().nth(letter.max(-1) as usize).map_or(text.len(), |(at, _)| at)
}

/// A text with characters `from` to `to` replaced by `with`.
fn spliced(text: &str, from: i32, to: i32, with: &str) -> String {
    let (from, to) = (byte_at(text, from), byte_at(text, to));
    format!("{}{with}{}", &text[..from.min(to)], &text[from.max(to)..])
}

/// The text unit around a character: the character itself (`unit` 0), its word with what
/// follows up to the next word (1), or its line with its end (else). Sentences are treated as
/// lines.
fn stretch(letters: &[char], at: i32, unit: u32) -> [usize; 2] {
    let at = (at.max(0) as usize).min(letters.len());
    if unit == 0 || letters.is_empty() {
        return [at, (at + 1).min(letters.len())];
    }
    let starts = |place: usize| match unit {
        _ if place == 0 => true,
        1 => letters[place].is_alphanumeric() && !letters[place - 1].is_alphanumeric(),
        _ => letters[place - 1] == '\n',
    };
    let from = (0..=at.min(letters.len() - 1)).rev().find(|place| starts(*place)).unwrap_or(0);
    [from, (at + 1..letters.len()).find(|place| starts(*place)).unwrap_or(letters.len())]
}

/// How one text became another: the first differing character, what was removed from there, and
/// what was inserted.
fn retyped(was: &str, now: &str) -> (usize, String, String) {
    let (was, now): (Vec<char>, Vec<char>) = (was.chars().collect(), now.chars().collect());
    let same = was.iter().zip(&now).take_while(|(old, new)| old == new).count();
    let most = was.len().min(now.len()) - same;
    let tail = was.iter().rev().zip(now.iter().rev()).take(most).take_while(|(old, new)| old == new).count();
    (same, was[same..was.len() - tail].iter().collect(), now[same..now.len() - tail].iter().collect())
}

/// The user's locale for one category.
fn locale(kind: &str) -> String {
    let set = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    set("LC_ALL").or_else(|| set(&format!("LC_{kind}"))).or_else(|| set("LANG")).unwrap_or_else(|| "C".into())
}

/// An event type as screen readers subscribe to it and as the registry passes it on
/// (`object:state-changed:focused`, `Object:StateChanged:Focused`), normalized: lower case, no
/// hyphens.
fn plainly(kind: &str) -> String {
    kind.chars().filter(|letter| *letter != '-').map(|letter| letter.to_ascii_lowercase()).collect()
}

/// Whether a subscription covers an event type: up to three parts, an omitted part matching
/// all.
fn covers(asked: &str, kind: &str) -> bool {
    asked.split(':').zip(kind.split(':')).all(|(asked, kind)| asked.is_empty() || asked == kind)
}

/// One of this program's objects: the application, a window (by list index), or an element (by
/// index in its window's list).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Which {
    Program,
    Window(usize),
    Thing(usize, usize),
}

/// A window, as screen readers see it.
struct Frame {
    /// Its id in the first window's UI; 0 for the first window itself.
    known: Id,
    /// The number in its path.
    number: u32,
    title: String,
    /// Its size in pixels, and pixels per unit.
    size: [i32; 2],
    scale: f32,
    /// It has keyboard focus.
    active: bool,
    /// Reported this run. (One not reported was closed.)
    seen: bool,
    /// Its elements as of the last run, and which list they were (`Ui::told_turn`); and each
    /// element's path number, kept while it exists.
    said: Vec<Said>,
    turn: u64,
    numbers: Vec<u32>,
}

/// A value attached to an event.
#[derive(Debug, PartialEq)]
enum Any {
    Number(i32),
    Real(f64),
    Text(String),
    /// One of this program's objects, by path.
    Object(String),
}

/// An event about an object: its path, its type (in AT-SPI's three parts), and its detail
/// numbers and value.
#[derive(Debug, PartialEq)]
struct News {
    of: String,
    kind: &'static str,
    numbers: [i32; 2],
    value: Any,
}

/// The object path of a window, or of an element in it, by their numbers.
fn path_of(window: u32, thing: Option<u32>) -> String {
    thing.map_or(format!("{THINGS}{window}"), |thing| format!("{THINGS}{window}/{thing}"))
}

/// A path number for a new object: the next after the last.
fn new_number(last: &mut u32) -> u32 {
    *last = last.wrapping_add(1);
    *last
}

/// The events of what changed about an element between two runs. (Focus is reported after
/// everything else: see `differ`.)
fn changes(of: &str, [was, now]: [&Said; 2], [was_on_it, on_it]: [bool; 2], news: &mut Vec<News>) {
    let mut tell = |kind, numbers, value| news.push(News { of: of.into(), kind, numbers, value });
    if was.name != now.name {
        tell("Object:PropertyChange:accessible-name", [0; 2], Any::Text(now.name.clone()));
    }
    if role_of(was) != role_of(now) {
        tell("Object:PropertyChange:accessible-role", [0; 2], Any::Number(role_of(now).0 as i32));
    }
    let state = match now.role {
        Role::Tick | Role::Switch | Role::Choice | Role::MenuItem | Role::MenuChoice => "Object:StateChanged:checked",
        Role::Tab | Role::Row | Role::TableRow | Role::TreeItem => "Object:StateChanged:selected",
        Role::Fold | Role::Select | Role::Submenu | Role::TextBox => "Object:StateChanged:expanded",
        _ => "Object:StateChanged:pressed",
    };
    if was.on != now.on {
        tell(state, [(now.on == Some(true)) as i32, 0], Any::Number(0));
    }
    // A tree item's expanded state, besides its selection.
    if was.open != now.open {
        tell("Object:StateChanged:expanded", [(now.open == Some(true)) as i32, 0], Any::Number(0));
    }
    // Its sort annotation changed: the rows are sorted differently by its column.
    if was.sorted != now.sorted {
        tell("Object:AttributesChanged:", [0; 2], Any::Number(0));
    }
    if was.usable != now.usable {
        tell("Object:StateChanged:enabled", [now.usable as i32, 0], Any::Number(0));
        tell("Object:StateChanged:sensitive", [now.usable as i32, 0], Any::Number(0));
    }
    // It scrolled into or out of view: screen readers track what is showing.
    if was.shown != now.shown {
        tell("Object:StateChanged:showing", [now.shown as i32, 0], Any::Number(0));
    }
    if was.amount != now.amount {
        tell("Object:PropertyChange:accessible-value", [0; 2], Any::Real(now.amount.map_or(0.0, |amount| amount[0] as f64)));
    }
    if was.value != now.value {
        let (from, gone, come) = retyped(&was.value, &now.value);
        for (kind, text) in [("Object:TextChanged:delete", gone), ("Object:TextChanged:insert", come)] {
            if !text.is_empty() {
                tell(kind, [from as i32, text.chars().count() as i32], Any::Text(text));
            }
        }
    }
    // A text box's caret moved, or its selection is another range.
    let (cursor, picked) = (|said: &Said| said.caret.map(|caret| caret[1]), |said: &Said| said.caret.filter(|[from, to]| from != to));
    if let Some(at) = cursor(now).filter(|at| cursor(was) != Some(*at)) {
        tell("Object:TextCaretMoved:", [at as i32, 0], Any::Number(0));
    }
    if picked(was) != picked(now) {
        tell("Object:TextSelectionChanged:", [0; 2], Any::Number(0));
    }
    if was_on_it && !on_it {
        tell("Object:StateChanged:focused", [0; 2], Any::Number(0));
    }
}

/// What changed in a window between runs: its elements' path numbers now, and the events for a
/// screen reader. An element is the same while it has the same id in the same group; anything
/// else was removed or added.
fn differ(frame: &Frame, now: &[Said], active: bool, last: &mut u32) -> (Vec<u32>, Vec<News>) {
    let (was, mut news) = (&frame.said, Vec::new());
    let group = |list: &[Said], at: usize| list[at].within.and_then(|group| list.get(group)).map_or(0, |group| group.id);
    // Which old element each is; and which old ones remain. (Most are where they were in the
    // list or a little further: each is searched from where the previous one was found.)
    let (mut same, mut stays, mut near) = (Vec::with_capacity(now.len()), vec![false; was.len()], 0);
    for (at, said) in now.iter().enumerate() {
        let in_new = said.within.is_some_and(|group| same.get(group).is_none_or(|same: &Option<usize>| same.is_none()));
        let found = (near..was.len()).chain(0..near).find(|old| was[*old].id == said.id);
        let old = found.filter(|old| !in_new && group(was, *old) == group(now, at));
        near = found.map_or(near, |old| old + 1);
        old.into_iter().for_each(|old| stays[old] = true);
        same.push(old);
    }
    let mut number = |old: &Option<usize>| old.map_or_else(|| new_number(last), |old| frame.numbers[old]);
    let numbers: Vec<u32> = same.iter().map(&mut number).collect();
    let path = |numbers: &[u32], at: Option<usize>| path_of(frame.number, at.map(|at| numbers[at]));
    // Of what was removed or added, a screen reader hears the outermost: it queries what is
    // inside.
    for (at, said) in was.iter().enumerate() {
        if !stays[at] && said.within.is_none_or(|group| stays[group]) {
            news.push(News { of: path(&frame.numbers, said.within), kind: "Object:ChildrenChanged:remove", numbers: [0; 2], value: Any::Object(path(&frame.numbers, Some(at))) });
        }
    }
    for (at, said) in now.iter().enumerate() {
        if same[at].is_none() && said.within.is_none_or(|group| same[group].is_some()) {
            let place = now[..at].iter().filter(|other| other.within == said.within).count() as i32;
            news.push(News { of: path(&numbers, said.within), kind: "Object:ChildrenChanged:add", numbers: [place, 0], value: Any::Object(path(&numbers, Some(at))) });
        }
    }
    // Then changed states; and last, the focus.
    let mut onto = None;
    for (at, said) in now.iter().enumerate() {
        let (of, old, on_it) = (path(&numbers, Some(at)), same[at].map(|old| &was[old]), said.focused && active);
        let was_on_it = old.is_some_and(|old| old.focused && frame.active);
        old.into_iter().for_each(|old| changes(&of, [old, said], [was_on_it, on_it], &mut news));
        onto = onto.or((on_it && !was_on_it).then_some(of));
    }
    news.extend(onto.map(|of| News { of, kind: "Object:StateChanged:focused", numbers: [1, 0], value: Any::Number(0) }));
    (numbers, news)
}

/// An action a screen reader requested on an element in a window.
pub(crate) struct Deed {
    /// The window, by its id in the first window's UI (0 for the first itself), and the
    /// element.
    pub window: Id,
    thing: Id,
    act: Act,
}

#[derive(Debug, PartialEq)]
enum Act {
    Press,
    /// Focus it.
    Focus,
    /// Set its text.
    Type(String),
    /// Set its value.
    Reach(f32),
    /// Expand (`true`) or collapse a tree item.
    Open(bool),
    /// Toggle a tree item's selection.
    Pick,
    /// Scroll its container so it shows.
    Show,
}

/// A slider moving to a value a screen reader set: stepped as the keyboard steps it, the UI
/// running after each step.
struct Reach {
    window: Id,
    thing: Id,
    to: f32,
    /// The distance before the last step.
    off: f32,
    /// Steps left to try.
    left: u8,
}

/// The desktop's screen readers, as far as this program deals with them.
pub(crate) struct Readers {
    /// The accessibility bus, while someone listens. (Whether anyone does is said on the
    /// session bus, which the windows own.)
    desk: Option<Bus>,
    /// Pending call serials: on the session bus, whether anyone listens and the accessibility
    /// bus address; on that bus, the registry's desktop object and its event listeners.
    asked: [u32; 4],
    /// This program's unique name on the accessibility bus, once assigned.
    name: String,
    /// The registry's desktop object: the application's windows are its children, and the
    /// application the desktop's.
    desktop: (String, String),
    /// Registered with the registry.
    embedded: bool,
    /// Event listeners: a bus name and an event type. Unknown until the registry says.
    hearers: Option<Vec<[String; 2]>>,
    /// The id the registry assigned the application; it only wants it kept.
    id: i32,
    frames: Vec<Frame>,
    /// The last path number assigned.
    last: u32,
    reach: Option<Reach>,
}

impl Readers {
    /// Starts asking, over the session bus, whether anyone listens, and subscribes to changes.
    /// Nothing waits: the reply comes when it comes, and without a session bus, never.
    pub(crate) fn new(session: Option<&mut Bus>) -> Self {
        let mut readers = Readers { desk: None, asked: [0; 4], name: String::new(), desktop: Default::default(), embedded: false, hearers: None, id: 0, frames: Vec::new(), last: 0, reach: None };
        let Some(bus) = session else { return readers };
        bus.ask("AddMatch", &format!("type='signal',sender='{LAUNCHER}',path='{LAUNCHER_PATH}',interface='{PROPERTIES}',member='PropertiesChanged'"));
        bus.ask("AddMatch", &format!("type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{LAUNCHER}'"));
        readers.ask_whether(bus);
        readers
    }

    /// Asks whether anyone listens: the service's properties, without auto-starting it.
    fn ask_whether(&mut self, session: &mut Bus) {
        let header = Header { kind: CALL, flags: 2, to: LAUNCHER, path: LAUNCHER_PATH, interface: PROPERTIES, member: "GetAll", kinds: "s", ..Default::default() };
        self.asked[0] = session.send(header, &Pack::default().text("org.a11y.Status").bytes);
    }

    /// The accessibility bus socket to poll, and its events. (-1 while not on that bus.)
    pub(crate) fn waits(&self) -> (i32, i16) {
        self.desk.as_ref().map_or((-1, 0), Bus::waits)
    }

    /// A screen reader listens and its bus is up: the UI should keep its accessibility list.
    pub(crate) fn listening(&self) -> bool {
        self.desk.is_some() && !self.name.is_empty()
    }

    /// Reads and answers the accessibility bus. Returns the actions screen readers requested.
    /// `origin` gives a window's screen position.
    pub(crate) fn turn(&mut self, origin: &dyn Fn(Id) -> [i32; 2]) -> Vec<Deed> {
        let mut deeds = Vec::new();
        if let Some(bus) = &mut self.desk {
            bus.pieces = PIECES;
        }
        while let Some(message) = self.desk.as_mut().and_then(Bus::message) {
            self.hear_desk(&message, origin, &mut deeds);
        }
        self.flush();
        deeds
    }

    /// Sends queued data, and drops the accessibility bus if it died: without it nobody
    /// listens, and nothing is kept.
    fn flush(&mut self) {
        self.desk.iter_mut().for_each(Bus::flush);
        if self.desk.as_ref().is_some_and(|bus| bus.dead) {
            self.desk = None;
        }
        if self.desk.is_none() {
            (self.name, self.embedded, self.hearers, self.frames, self.reach) = (String::new(), false, None, Vec::new(), None);
            self.asked[2..].fill(0);
        }
    }

    /// A session bus message that may concern screen readers: a signal that the listening state
    /// may have changed (the service's properties, or its name changing owner), which is then
    /// queried again; or a reply. Calls to this program there are answered here too.
    pub(crate) fn hear_session(&mut self, session: &mut Bus, message: &Message) {
        let (mut body, mut named, mut values) = (message.body, message.body, Pack::default());
        let ours = message.path == LAUNCHER_PATH || message.member == "NameOwnerChanged" && named.text() == Some(LAUNCHER);
        match message.kind {
            // Only liveness is asked of this program here.
            CALL => session.answer(message, peer(message.member, &mut values).filter(|_| message.interface == PEER).ok_or(UNKNOWN), &values),
            NEWS if ours => self.ask_whether(session),
            ANSWER | FAILED if message.answers == self.asked[0] => self.heard_whether(session, message.kind == ANSWER && someone_listens(body) == Some(true)),
            ANSWER if message.answers == self.asked[1] && self.desk.is_none() => self.join(body.text().unwrap_or_default()),
            _ => {}
        }
    }

    /// Someone listens, or nobody does any more: the accessibility bus is found (the desktop
    /// may give its address directly, else the service is asked), or left.
    fn heard_whether(&mut self, session: &mut Bus, someone: bool) {
        let said = std::env::var("AT_SPI_BUS_ADDRESS").ok().filter(|address| !address.is_empty());
        match (someone, said) {
            (false, _) => self.desk = None,
            (true, Some(address)) if self.desk.is_none() => self.join(&address),
            (true, None) if self.desk.is_none() => self.asked[1] = session.call(LAUNCHER, LAUNCHER_PATH, LAUNCHER, "GetAddress", "", &Pack::default()),
            _ => {}
        }
        self.flush();
    }

    /// Joins the accessibility bus at an address, and subscribes to the registry's signals:
    /// listener changes, and the registry itself appearing or disappearing.
    fn join(&mut self, address: &str) {
        self.desk = Bus::open(address);
        let Some(bus) = &mut self.desk else { return };
        bus.ask("AddMatch", &format!("type='signal',sender='{REGISTRY}',interface='{REGISTRY}'"));
        bus.ask("AddMatch", &format!("type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{REGISTRY}'"));
    }

    /// An accessibility bus message: a call to answer, a registry signal, or a reply.
    fn hear_desk(&mut self, bytes: &[u8], origin: &dyn Fn(Id) -> [i32; 2], deeds: &mut Vec<Deed>) {
        let Some(message) = Message::read(bytes) else { return };
        let (mut body, mut values) = (message.body, Pack::default());
        match message.kind {
            CALL => {
                let kinds = self.reply(&message, &mut values, origin, deeds);
                self.desk.iter_mut().for_each(|bus| bus.answer(&message, kinds, &values));
            }
            NEWS => self.hear_registry(&message),
            // `Hello` was the first call: its reply is this program's unique name.
            ANSWER if message.answers == 1 => self.name = body.text().unwrap_or_default().into(),
            ANSWER if message.answers == self.asked[2] => self.desktop = body.take(8, 0).and(body.text()).zip(body.text()).map(|(name, path)| (name.into(), path.into())).unwrap_or_default(),
            ANSWER if message.answers == self.asked[3] => self.hearers = Some(hearers(body).unwrap_or_default()),
            _ => {}
        }
    }

    /// A registry signal: a screen reader subscribed to or unsubscribed from an event type. Or
    /// a signal about the registry: it disappeared, and when it reappears this program must
    /// register again. (It also appears when first activated, which is by registering: then it
    /// already knows.)
    fn hear_registry(&mut self, news: &Message) {
        let mut body = news.body;
        let (Some(first), Some(second)) = (body.text(), body.text()) else { return };
        // Until the registry has listed the listeners, its listener signals are ignored: they
        // are reflected in the list when it arrives.
        match (news.member, &mut self.hearers) {
            ("EventListenerRegistered", Some(hearers)) => hearers.push([first.into(), plainly(second)]),
            ("EventListenerDeregistered", Some(hearers)) => hearers.retain(|[name, kind]| name != first || !covers(&plainly(second), kind)),
            ("NameOwnerChanged", _) if first == REGISTRY && body.text() == Some("") => (self.embedded, self.hearers) = (false, None),
            ("NameOwnerChanged", _) if first == REGISTRY && !self.embedded => self.embed(),
            _ => {}
        }
    }

    /// Registers with the registry, and asks for its event listeners and their changes.
    fn embed(&mut self) {
        let (Some(bus), mut me) = (self.desk.as_mut().filter(|_| !self.name.is_empty()), Pack::default()) else { return };
        self.asked[2] = bus.call(REGISTRY, ROOT, "org.a11y.atspi.Socket", "Embed", "(so)", me.object(&self.name, ROOT));
        self.asked[3] = bus.call(REGISTRY, REGISTRY_PATH, REGISTRY, "GetRegisteredEvents", "", &Pack::default());
        (self.embedded, self.hearers) = (true, None);
    }

    /// Whether an event type is sent: what someone subscribed to, and the always-sent ones.
    /// While nobody subscribed to anything, nothing is; until the registry has listed
    /// listeners, everything is.
    fn wanted(&self, kind: &str) -> bool {
        let (Some(hearers), kind) = (&self.hearers, plainly(kind)) else { return true };
        !hearers.is_empty() && (ALWAYS.iter().any(|always| kind.starts_with(always)) || hearers.iter().any(|[_, asked]| covers(asked, &kind)))
    }

    /// Sends an event, if anyone wants it.
    fn tell(&mut self, news: News) {
        let (true, Some(bus)) = (self.wanted(news.kind), &mut self.desk) else { return };
        let (mut parts, mut body) = (news.kind.split(':'), Pack::default());
        let (group, member, detail) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        body.text(detail).i32s(&news.numbers);
        match &news.value {
            Any::Number(number) => body.kinds("i").i32(*number),
            Any::Real(real) => body.kinds("d").f64(*real),
            Any::Text(text) => body.kinds("s").text(text),
            Any::Object(path) => body.kinds("(so)").object(&self.name, path),
        };
        // Event properties a screen reader may request alongside are not sent: it queries them.
        // So that array is empty.
        let header = Header { kind: NEWS, path: &news.of, interface: &format!("org.a11y.atspi.Event.{group}"), member, kinds: "siiva{sv}", ..Default::default() };
        bus.send(header, &body.open(8).close().bytes);
    }

    /// After a run with a screen reader listening, takes in one window's current elements and
    /// sends events for what changed. `ui` is that window's UI. Returns whether the UI must run
    /// again: a slider is moving somewhere.
    pub(crate) fn keep(&mut self, ui: &mut Ui, known: Id, title: &str, size: [i32; 2], scale: f32) -> bool {
        let at = self.frames.iter().position(|frame| frame.known == known).unwrap_or_else(|| self.opened(known, title));
        let (frame, active) = (&mut self.frames[at], ui.window_focused);
        let (of, renamed, turned) = (path_of(frame.number, None), frame.title != title, frame.active != active);
        let mut news = Vec::new();
        if renamed {
            news.push(News { of: of.clone(), kind: "Object:PropertyChange:accessible-name", numbers: [0; 2], value: Any::Text(title.into()) });
        }
        if turned {
            news.push(News { of: of.clone(), kind: if active { "Window:Activate:" } else { "Window:Deactivate:" }, numbers: [0; 2], value: Any::Text(title.into()) });
            news.push(News { of, kind: "Object:StateChanged:active", numbers: [active as i32, 0], value: Any::Number(0) });
        }
        // (A list of the same turn is not even compared.)
        let turn = ui.told_turn();
        let kept = std::mem::replace(&mut frame.turn, turn) == turn;
        if turned || !kept && frame.said != ui.told() {
            let (numbers, changed) = differ(frame, ui.told(), active, &mut self.last);
            news.extend(changed);
            frame.numbers = numbers;
            frame.said.clear();
            frame.said.extend_from_slice(ui.told());
        }
        (frame.seen, frame.size, frame.scale, frame.active) = (true, size, scale, active);
        if renamed {
            frame.title = title.into();
        }
        // Announcements the app made this run are sent for the window (1: after current
        // speech).
        let window = path_of(frame.number, None);
        news.extend(ui.announced().iter().map(|words| News { of: window.clone(), kind: "Object:Announcement:", numbers: [1, 0], value: Any::Text(words.clone()) }));
        news.into_iter().for_each(|news| self.tell(news));
        self.step(ui, known)
    }

    /// A window not reported before: a child of the application from now on.
    fn opened(&mut self, known: Id, title: &str) -> usize {
        let (number, at) = (new_number(&mut self.last), self.frames.len());
        self.frames.push(Frame { known, number, title: title.into(), size: [0; 2], scale: 1.0, active: false, seen: true, said: Vec::new(), turn: 0, numbers: Vec::new() });
        let of = path_of(number, None);
        self.tell(News { of: ROOT.into(), kind: "Object:ChildrenChanged:add", numbers: [at as i32, 0], value: Any::Object(of.clone()) });
        self.tell(News { of, kind: "Window:Create:", numbers: [0; 2], value: Any::Text(title.into()) });
        at
    }

    /// After a run's windows are taken in: those missing were closed, and the application
    /// registers, if it hasn't yet.
    pub(crate) fn settle(&mut self) {
        for at in (0..self.frames.len()).rev() {
            if !std::mem::take(&mut self.frames[at].seen) {
                let frame = self.frames.remove(at);
                let of = path_of(frame.number, None);
                self.tell(News { of: of.clone(), kind: "Window:Destroy:", numbers: [0; 2], value: Any::Text(frame.title) });
                self.tell(News { of: ROOT.into(), kind: "Object:ChildrenChanged:remove", numbers: [at as i32, 0], value: Any::Object(of) });
            }
        }
        if !self.embedded {
            self.embed();
        }
        self.flush();
    }

    /// Performs a screen reader's requested action on a window's UI. (The UI must run after.)
    pub(crate) fn act(&mut self, ui: &mut Ui, deed: Deed) {
        match deed.act {
            Act::Press => ui.press_for_reader(deed.thing),
            Act::Focus => ui.set_focus(deed.thing),
            Act::Type(text) => ui.type_for_reader(deed.thing, &text),
            Act::Open(open) => ui.open_for_reader(deed.thing, open),
            Act::Pick => ui.pick_for_reader(deed.thing),
            Act::Show => ui.show_for_reader(deed.thing),
            Act::Reach(to) => {
                self.reach = Some(Reach { window: deed.window, thing: deed.thing, to, off: f32::INFINITY, left: 100 });
                self.step(ui, deed.window);
            }
        }
    }

    /// Steps the moving slider toward its target while steps bring it nearer; the one step that
    /// overshoots further than it was is undone. Returns whether it stepped: the UI runs again.
    fn step(&mut self, ui: &mut Ui, window: Id) -> bool {
        let Some(reach) = self.reach.as_mut().filter(|reach| reach.window == window) else { return false };
        let stands = ui.told().iter().find(|said| said.id == reach.thing).and_then(|said| said.amount);
        let off = stands.map_or(0.0, |stands| reach.to - stands[0]);
        // A step that changed nothing hit the slider's end; a slider at its target, or gone,
        // stops.
        let (nearer, further) = (off.abs() < reach.off && off != 0.0, off.abs() > reach.off);
        (reach.off, reach.left) = (off.abs(), reach.left.saturating_sub(1));
        let thing = reach.thing;
        if !nearer || reach.left == 0 {
            self.reach = None;
        }
        if nearer || further {
            ui.step_for_reader(thing, off > 0.0);
        }
        nearer || further
    }

    /// The object a path names; `None` if none (any more).
    fn which(&self, path: &str) -> Option<Which> {
        if path == ROOT {
            return Some(Which::Program);
        }
        let mut numbers = path.strip_prefix(THINGS)?.split('/').map(|number| number.parse::<u32>().ok());
        let (window, thing) = (numbers.next()??, numbers.next());
        let at = self.frames.iter().position(|frame| frame.number == window)?;
        match thing {
            None => Some(Which::Window(at)),
            Some(thing) => numbers.next().is_none().then_some(Which::Thing(at, self.frames[at].numbers.iter().position(|number| Some(*number) == thing)?)),
        }
    }

    /// An object's path on the bus.
    fn path(&self, which: Which) -> String {
        match which {
            Which::Program => ROOT.into(),
            Which::Window(at) => path_of(self.frames[at].number, None),
            Which::Thing(at, thing) => path_of(self.frames[at].number, Some(self.frames[at].numbers[thing])),
        }
    }

    /// An object's element, if it is one; and the window an object is, or is in.
    fn said(&self, which: Which) -> Option<&Said> {
        match which {
            Which::Thing(at, thing) => self.frames[at].said.get(thing),
            _ => None,
        }
    }
    fn frame(&self, which: Which) -> Option<&Frame> {
        match which {
            Which::Program => None,
            Which::Window(at) | Which::Thing(at, _) => self.frames.get(at),
        }
    }

    /// An object's parent. (The application's is the desktop, not its own.)
    fn parent(&self, which: Which) -> Which {
        match which {
            Which::Thing(at, thing) => self.frames[at].said[thing].within.map_or(Which::Window(at), |group| Which::Thing(at, group)),
            _ => Which::Program,
        }
    }

    /// An object's children, in order.
    fn children(&self, which: Which) -> Vec<Which> {
        let things = |at: usize, group: Option<usize>| self.frames[at].said.iter().enumerate().filter(move |(_, said)| said.within == group).map(move |(thing, _)| Which::Thing(at, thing));
        match which {
            Which::Program => (0..self.frames.len()).map(Which::Window).collect(),
            Which::Window(at) => things(at, None).collect(),
            Which::Thing(at, thing) => things(at, Some(thing)).collect(),
        }
    }

    /// An object's descendants at any depth, as indices in its window's list. (A group precedes
    /// its contents, with nothing else between.)
    fn below(&self, which: Which) -> std::ops::Range<usize> {
        let Which::Thing(at, group) = which else { return 0..self.frame(which).map_or(0, |frame| frame.said.len()) };
        let said = &self.frames[at].said;
        let after = (group + 1..said.len()).find(|thing| said[*thing].within.is_none_or(|within| within < group));
        group + 1..after.unwrap_or(said.len())
    }

    /// An object's AT-SPI role and state.
    fn role(&self, which: Which) -> (u32, &'static str) {
        match which {
            Which::Program => (75, "application"),
            Which::Window(_) => (23, "frame"),
            Which::Thing(at, thing) => role_of(&self.frames[at].said[thing]),
        }
    }

    fn states(&self, which: Which) -> u64 {
        match which {
            Which::Program => 0,
            Which::Window(at) => (self.frames[at].active as u64) << ACTIVE | 1 << ENABLED | 1 << SENSITIVE | 1 << SHOWING | 1 << VISIBLE | 1 << RESIZABLE,
            Which::Thing(at, thing) => states_of(&self.frames[at].said[thing], self.frames[at].active),
        }
    }

    /// The interfaces an object implements.
    fn interfaces(&self, which: Which) -> Vec<&'static str> {
        let Some(said) = self.said(which) else { return vec![ACCESSIBLE, if which == Which::Program { APPLICATION } else { COMPONENT }] };
        let (types, moves) = (matches!(said.role, Role::TextBox | Role::Secret), said.amount.is_some());
        let also =
            [(pressed(said), ACTION), (moves, VALUE), (types, TEXT), (types, EDITABLE), (said.role == Role::Table, TABLE), (said.role == Role::Cell, TABLE_CELL), (said.role == Role::Tree, SELECTION)];
        [ACCESSIBLE, COMPONENT].into_iter().chain(also.into_iter().filter_map(|(has, name)| has.then_some(name))).collect()
    }

    /// An object's name: the application by its executable's name, a window by its title.
    fn called(&self, which: Which) -> String {
        let file = || std::env::current_exe().ok().and_then(|path| path.file_stem().map(|name| name.to_string_lossy().into_owned()));
        match which {
            Which::Program => file().unwrap_or_default(),
            Which::Window(at) => self.frames[at].title.clone(),
            Which::Thing(at, thing) => self.frames[at].said[thing].name.clone(),
        }
    }

    /// An object's extent in its window, in pixels: position and size. (In a table, the part
    /// the table shows: a row is reported whole, wherever the table is scrolled horizontally.)
    fn extent(&self, which: Which) -> Option<[i32; 4]> {
        let frame = self.frame(which)?;
        let Which::Thing(_, thing) = which else { return Some([0, 0, frame.size[0], frame.size[1]]) };
        let seen = told::view(&frame.said, thing);
        Some([seen.x, seen.y, seen.w, seen.h].map(|units| (units * frame.scale).round() as i32))
    }

    /// The origin of a coordinate type, relative to the object's window: the screen (0), the
    /// window (1), or the object's parent (2).
    fn counted_from(&self, which: Which, kind: u32, origin: &dyn Fn(Id) -> [i32; 2]) -> [i32; 2] {
        match (kind, self.extent(self.parent(which))) {
            (1, _) => [0; 2],
            (2, Some([x, y, ..])) => [x, y],
            _ => self.frame(which).map_or([0; 2], |frame| origin(frame.known).map(|place| -place)),
        }
    }

    /// Computes a call's reply: writes its values and returns their signature; or returns an
    /// error.
    fn reply(&mut self, call: &Message, values: &mut Pack, origin: &dyn Fn(Id) -> [i32; 2], deeds: &mut Vec<Deed>) -> Result<&'static str, (&'static str, &'static str)> {
        let mut asked = call.body;
        // Liveness may be asked of any path.
        if call.interface == PEER {
            return peer(call.member, values).ok_or(UNKNOWN);
        }
        // The screen readers' library asks every application once for all its objects, to cache
        // them, and complains about one without an answer. No cache is kept here: the list is
        // empty, and it queries each object as needed.
        if call.interface == "org.a11y.atspi.Cache" && call.member == "GetItems" {
            values.open(8).close();
            return Ok("a((so)(so)(so)iiassusau)");
        }
        let done = match (call.interface, self.which(call.path)) {
            (PROPERTIES, Some(which)) => self.properties(which, call.member, &mut asked, values, deeds),
            (ACCESSIBLE, Some(which)) => self.accessible(which, call.member, &mut asked, values),
            (APPLICATION, Some(which)) => application(which, call.member, &mut asked, values),
            (COMPONENT, Some(which)) => self.component(which, call.member, &mut asked, values, origin, deeds),
            (ACTION, Some(which)) => self.action(which, call.member, &mut asked, values, deeds),
            (TEXT, Some(which)) => self.text(which, call.member, &mut asked, values),
            (EDITABLE, Some(which)) => self.editable(which, call.member, &mut asked, values, deeds),
            (TABLE, Some(which)) => self.table(which, call.member, &mut asked, values),
            (TABLE_CELL, Some(which)) => self.cell(which, call.member, values),
            (SELECTION, Some(which)) => self.selection(which, call.member, &mut asked, values, deeds),
            (PROPERTIES | ACCESSIBLE | APPLICATION | COMPONENT | ACTION | TEXT | EDITABLE | TABLE | TABLE_CELL | SELECTION, None) => return Err(GONE),
            _ => None,
        };
        done.ok_or(UNKNOWN)
    }

    /// The properties interface: get one, get all of one interface, or set one.
    fn properties(&mut self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack, deeds: &mut Vec<Deed>) -> Option<&'static str> {
        let interface = asked.text()?;
        match member {
            "Get" => self.property(which, interface, asked.text()?, values).map(|()| "v"),
            "GetAll" => Some(self.all(which, interface, values)),
            "Set" => self.set(which, interface, asked.text()?, asked.value()?, deeds).map(|()| ""),
            _ => None,
        }
    }

    /// All of an object's properties for one interface, by name.
    fn all(&self, which: Which, interface: &str, values: &mut Pack) -> &'static str {
        let names = NAMED.iter().find(|(of, _)| *of == interface).map_or(&[][..], |(_, names)| names);
        values.open(8);
        for name in names {
            let before = values.bytes.len();
            values.pad(8).text(name);
            if self.property(which, interface, name, values).is_none() {
                values.bytes.truncate(before);
            }
        }
        values.close();
        "a{sv}"
    }

    /// One property of an object, as a variant. `None` if it has no such property.
    fn property(&self, which: Which, interface: &str, name: &str, values: &mut Pack) -> Option<()> {
        if !self.interfaces(which).contains(&interface) {
            return None;
        }
        let (said, within) = (self.said(which), self.parent(which));
        let amount = said.and_then(|said| said.amount).unwrap_or_default().map(f64::from);
        match (interface, name) {
            (ACCESSIBLE, "Name") => values.kinds("s").text(&self.called(which)),
            (ACCESSIBLE, "Description" | "HelpText") | (VALUE, "Text") => values.kinds("s").text(""),
            (ACCESSIBLE, "Parent") if which == Which::Program => values.kinds("(so)").object(&self.desktop.0, if self.desktop.1.is_empty() { NOTHING } else { &self.desktop.1 }),
            (ACCESSIBLE, "Parent") => values.kinds("(so)").object(&self.name, &self.path(within)),
            (ACCESSIBLE, "ChildCount") => values.kinds("i").i32(self.children(which).len() as i32),
            (ACCESSIBLE, "Locale") => values.kinds("s").text(&locale("MESSAGES")),
            (ACCESSIBLE, "AccessibleId") => values.kinds("s").text(&said.map_or(String::new(), |said| format!("{:016x}", said.id))),
            (APPLICATION, "ToolkitName") => values.kinds("s").text("taut"),
            (APPLICATION, "Version" | "ToolkitVersion") => values.kinds("s").text(env!("CARGO_PKG_VERSION")),
            (APPLICATION, "AtspiVersion") => values.kinds("s").text("2.1"),
            (APPLICATION, "Id") => values.kinds("i").i32(self.id),
            (ACTION, "NActions") => values.kinds("i").i32(said.map_or(1, |said| ways_of(said).len()) as i32),
            (VALUE, "CurrentValue") => values.kinds("d").f64(amount[0]),
            (VALUE, "MinimumValue") => values.kinds("d").f64(amount[1]),
            (VALUE, "MaximumValue") => values.kinds("d").f64(amount[2]),
            // The step size: zero for values without steps.
            (VALUE, "MinimumIncrement") => values.kinds("d").f64(said?.step as f64),
            (TEXT, "CharacterCount") => values.kinds("i").i32(said?.value.chars().count() as i32),
            // The caret position in the text, while focused: else -1.
            (TEXT, "CaretOffset") => values.kinds("i").i32(said?.caret.map_or(-1, |caret| caret[1] as i32)),
            // A table: its total rows and visible columns, its selected row count; no caption,
            // no summary, no selected column.
            (TABLE, "NRows" | "NColumns") => values.kinds("i").i32(said?.grid.unwrap_or_default()[(name == "NColumns") as usize] as i32),
            (TABLE, "NSelectedRows") => values.kinds("i").i32(self.picked_rows(which).len() as i32),
            (TABLE, "NSelectedColumns") => values.kinds("i").i32(0),
            (TABLE, "Caption" | "Summary") => values.kinds("(so)").object(&self.name, NOTHING),
            // A cell: its row and column, spanning one each, and its table.
            (TABLE_CELL, "Position") => values.kinds("(ii)").pad(8).i32s(&said?.grid.unwrap_or_default().map(|number| number as i32)),
            (TABLE_CELL, "RowSpan" | "ColumnSpan") => values.kinds("i").i32(1),
            (TABLE_CELL, "Table") => values.kinds("(so)").object(&self.name, &self.table_of(which).map_or(NOTHING.into(), |table| self.path(table))),
            // A tree: how many of its reported items are selected.
            (SELECTION, "NSelectedChildren") => values.kinds("i").i32(self.picked_children(which).len() as i32),
            _ => return None,
        };
        Some(())
    }

    /// The table a cell is in.
    fn table_of(&self, which: Which) -> Option<Which> {
        let (Which::Thing(at, thing), Some(frame)) = (which, self.frame(which)) else { return None };
        told::table_of(&frame.said, thing).map(|table| Which::Thing(at, table))
    }
    /// A table's selected rows among those reported, each by its index among all its rows, from
    /// 0.
    fn picked_rows(&self, which: Which) -> Vec<i32> {
        let (Which::Thing(_, table), Some(frame)) = (which, self.frame(which)) else { return Vec::new() };
        told::rows_of(&frame.said, table).filter(|(row, _)| frame.said[*row].on == Some(true)).map(|(_, number)| number as i32).collect()
    }

    /// The table interface: the cell at a row and column, cell indices, a column's header, the
    /// selected rows. (Of a large table only visible rows are reported: a cell of another row
    /// is no object, and is answered as null.) Read-only by row and column: no row is selected
    /// or deselected through it.
    fn table(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack) -> Option<&'static str> {
        let (Which::Thing(at, table), frame) = (which, self.frame(which)?) else { return None };
        let [rows, columns] = self.said(which).filter(|said| said.role == Role::Table)?.grid.unwrap_or_default().map(|number| number as i32);
        let object = |thing: Option<usize>| thing.map_or(NOTHING.into(), |thing| self.path(Which::Thing(at, thing)));
        // A cell by row and column, and by index (counting along rows).
        let in_table = |row: i32, column: i32| row >= 0 && column >= 0 && row < rows && column < columns;
        let cell = |row: i32, column: i32| in_table(row, column).then(|| told::cell_at(&frame.said, table, [row as u32, column as u32])).flatten();
        let (picked, place) = (self.picked_rows(which), |index: i32| [index.div_euclid(columns.max(1)), index.rem_euclid(columns.max(1))]);
        let heading = |column: i32| (column >= 0).then(|| told::heading_at(&frame.said, table, column as u32)).flatten();
        let answer = match member {
            "GetAccessibleAt" => ("(so)", values.object(&self.name, &object(cell(asked.i32()?, asked.i32()?)))),
            "GetIndexAt" => ("i", values.i32(Some([asked.i32()?, asked.i32()?]).filter(|[row, column]| in_table(*row, *column)).map_or(-1, |[row, column]| row * columns + column))),
            "GetRowAtIndex" | "GetColumnAtIndex" => {
                ("i", values.i32(Some(asked.i32()?).filter(|index| (0..rows * columns).contains(index)).map_or(-1, |index| place(index)[(member == "GetColumnAtIndex") as usize])))
            }
            "GetRowDescription" => ("s", values.text("")),
            "GetColumnDescription" => ("s", values.text(&heading(asked.i32()?).map_or(String::new(), |heading| frame.said[heading].name.clone()))),
            "GetRowExtentAt" | "GetColumnExtentAt" => ("i", values.i32(in_table(asked.i32()?, asked.i32()?) as i32)),
            "GetRowHeader" => ("(so)", values.object(&self.name, NOTHING)),
            "GetColumnHeader" => ("(so)", values.object(&self.name, &object(heading(asked.i32()?)))),
            "GetSelectedRows" => ("ai", values.open(4).i32s(&picked).close()),
            "GetSelectedColumns" => ("ai", values.open(4).close()),
            "IsRowSelected" => ("b", values.u32(picked.contains(&asked.i32()?) as u32)),
            "IsColumnSelected" => ("b", values.u32(0)),
            "IsSelected" => ("b", values.u32(Some((asked.i32()?, asked.i32()?)).is_some_and(|(row, column)| in_table(row, column) && picked.contains(&row)) as u32)),
            "AddRowSelection" | "AddColumnSelection" | "RemoveRowSelection" | "RemoveColumnSelection" => ("b", values.u32(0)),
            "GetRowColumnExtentsAtIndex" => {
                let index = asked.i32()?;
                let ([row, column], fits) = (place(index), (0..rows * columns).contains(&index));
                ("biiiib", values.u32(fits as u32).i32s(&[row, column, 1, 1].map(|number| if fits { number } else { 0 })).u32((fits && picked.contains(&row)) as u32))
            }
            _ => return None,
        };
        Some(answer.0)
    }

    /// The table cell interface: its headers (one over its column, none for its row), and its
    /// row and column with their spans.
    fn cell(&self, which: Which, member: &str, values: &mut Pack) -> Option<&'static str> {
        let (Which::Thing(at, thing), frame) = (which, self.frame(which)?) else { return None };
        let [row, column] = self.said(which).filter(|said| said.role == Role::Cell)?.grid.unwrap_or_default();
        let heading = told::table_of(&frame.said, thing).and_then(|table| told::heading_at(&frame.said, table, column));
        let answer = match member {
            "GetRowHeaderCells" => ("a(so)", values.open(8).close()),
            "GetColumnHeaderCells" => ("a(so)", heading.into_iter().fold(values.open(8), |values, heading| values.object(&self.name, &self.path(Which::Thing(at, heading)))).close()),
            "GetRowColumnSpan" => ("iiii", values.i32s(&[row as i32, column as i32, 1, 1])),
            _ => return None,
        };
        Some(answer.0)
    }

    /// A tree's selected children, in order.
    fn picked_children(&self, which: Which) -> Vec<Which> {
        self.children(which).into_iter().filter(|child| self.said(*child).is_some_and(|said| said.on == Some(true))).collect()
    }
    /// The selection interface of a tree, by child index (as counted when its children are
    /// listed), or among the selected ones: which, and whether one is; select one alone, or
    /// deselect one, as the keys do. (Select all and clear are not supported.)
    fn selection(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack, deeds: &mut Vec<Deed>) -> Option<&'static str> {
        let frame = self.frame(which).filter(|_| self.said(which).is_some_and(|said| said.role == Role::Tree))?;
        let (children, picked) = (self.children(which), self.picked_children(which));
        let deed = |child: Option<&Which>, act: Act| child.and_then(|child| self.said(*child)).filter(|said| said.usable).map(|said| Deed { window: frame.known, thing: said.id, act });
        let mut done = |wanted: Option<Deed>| (wanted.is_some() as u32, deeds.extend(wanted)).0;
        let answer = match member {
            "GetSelectedChild" => ("(so)", values.object(&self.name, &picked.get(asked.i32()? as usize).map_or(NOTHING.into(), |child| self.path(*child)))),
            "IsChildSelected" => ("b", values.u32(children.get(asked.i32()? as usize).is_some_and(|child| picked.contains(child)) as u32)),
            "SelectChild" => ("b", values.u32(done(deed(children.get(asked.i32()? as usize), Act::Press)))),
            "DeselectChild" => ("b", values.u32(done(deed(children.get(asked.i32()? as usize).filter(|child| picked.contains(child)), Act::Pick)))),
            "DeselectSelectedChild" => ("b", values.u32(done(deed(picked.get(asked.i32()? as usize), Act::Pick)))),
            "SelectAll" | "ClearSelection" => ("b", values.u32(0)),
            _ => return None,
        };
        Some(answer.0)
    }

    /// Sets a property: the id the registry assigns the application, which it keeps; or a
    /// slider's value, which it then moves to.
    fn set(&mut self, which: Which, interface: &str, name: &str, value: Value, deeds: &mut Vec<Deed>) -> Option<()> {
        match (interface, name, value, self.said(which), self.frame(which)) {
            (APPLICATION, "Id", Value::Number(id), ..) if which == Which::Program => self.id = id as i32,
            (VALUE, "CurrentValue", Value::Number(to), Some(said), Some(frame)) if said.role == Role::Slider && said.usable => {
                deeds.push(Deed { window: frame.known, thing: said.id, act: Act::Reach(to as f32) })
            }
            _ => return None,
        }
        Some(())
    }

    /// The accessible interface: role, state, children. (Each answer is its signature and the
    /// writing of its values.)
    fn accessible(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack) -> Option<&'static str> {
        let (name, inside, states) = (&self.name, self.children(which), self.states(which));
        let place = || self.children(self.parent(which)).iter().position(|child| *child == which).map_or(-1, |place| place as i32);
        let answer = match member {
            "GetChildAtIndex" => ("(so)", values.object(name, &self.path(*inside.get(asked.i32()? as usize)?))),
            "GetChildren" => ("a(so)", inside.iter().fold(values.open(8), |values, child| values.object(name, &self.path(*child))).close()),
            "GetIndexInParent" => ("i", values.i32(place())),
            "GetRole" => ("u", values.u32(self.role(which).0)),
            "GetRoleName" | "GetLocalizedRoleName" => ("s", values.text(self.role(which).1)),
            "GetState" => ("au", values.open(4).u32(states as u32).u32((states >> 32) as u32).close()),
            "GetApplication" => ("(so)", values.object(name, ROOT)),
            "GetInterfaces" => ("as", self.interfaces(which).iter().fold(values.open(4), |values, name| values.text(name)).close()),
            // No relations beyond parent and child, and no attributes: both arrays empty.
            "GetRelationSet" => ("a(ua(so))", values.open(8).close()),
            // Attributes: position in set and set size, where known. A sorted column's header
            // gets its sort direction, as on the web; a tree item its level from 1, as on the
            // web and as Orca reads it.
            "GetAttributes" => {
                let place = self.said(which).and_then(|said| said.of).map(|of| of.map(|number| number.to_string()));
                let sort = self.said(which).and_then(Said::sort_word).map(String::from);
                let level = self.said(which).map(|said| said.level).filter(|level| *level > 0).map(|level| level.to_string());
                let noted = place.iter().flat_map(|[nth, count]| [("posinset", nth), ("setsize", count)]).chain(sort.iter().map(|way| ("sort", way))).chain(level.iter().map(|level| ("level", level)));
                ("a{ss}", noted.fold(values.open(8), |values, (what, number)| values.pad(8).text(what).text(number)).close())
            }
            _ => return None,
        };
        Some(answer.0)
    }

    /// The component interface of windows and elements: extents, hit testing, and focus.
    fn component(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack, origin: &dyn Fn(Id) -> [i32; 2], deeds: &mut Vec<Deed>) -> Option<&'static str> {
        let ([x, y, w, h], frame, said) = (self.extent(which)?, self.frame(which)?, self.said(which));
        let from = |asked: &mut Unpack| asked.u32().map(|kind| self.counted_from(which, kind, origin));
        let corner = |asked: &mut Unpack| from(asked).map(|[left, top]| [x - left, y - top]);
        // A queried point, relative to the window's corner; whether it is on this object; and
        // the innermost element at it.
        let place = |asked: &mut Unpack| Some([asked.i32()?, asked.i32()?]).zip(from(asked)).map(|([px, py], [left, top])| [px + left, py + top]);
        let on = |[px, py]: [i32; 2]| px >= x && px < x + w && py >= y && py < y + h;
        let under = |[px, py]: [i32; 2]| self.below(which).rev().find(|thing| told::view(&frame.said, *thing).contains(pt(px as f32 / frame.scale, py as f32 / frame.scale)));
        let found = |place: [i32; 2]| under(place).filter(|_| on(place)).map_or(NOTHING.into(), |thing| path_of(frame.number, Some(frame.numbers[thing])));
        let taken = said.filter(|said| said.usable && said.takes_keyboard());
        let answer = match member {
            "GetExtents" => ("(iiii)", values.pad(8).i32s(&corner(asked)?).i32s(&[w, h])),
            "GetPosition" => ("ii", values.i32s(&corner(asked)?)),
            "GetSize" => ("ii", values.i32s(&[w, h])),
            "Contains" => ("b", values.u32(on(place(asked)?) as u32)),
            "GetAccessibleAtPoint" => ("(so)", values.object(&self.name, &found(place(asked)?))),
            // AT-SPI layers: 3 for a window's widgets, 7 for a window. Neither in the MDI
            // layer.
            "GetLayer" => ("u", values.u32(if said.is_some() { 3 } else { 7 })),
            "GetMDIZOrder" => ("n", values.pad(2).byte(0xff).byte(0xff)),
            "GetAlpha" => ("d", values.f64(1.0)),
            // Scrolled into view, however the alignment was requested: its container scrolls
            // until it shows. A window is in view as is.
            "ScrollTo" | "ScrollToPoint" => {
                deeds.extend(said.map(|said| Deed { window: frame.known, thing: said.id, act: Act::Show }));
                ("b", values.u32(said.is_some() as u32))
            }
            "GrabFocus" => {
                deeds.extend(taken.map(|said| Deed { window: frame.known, thing: said.id, act: Act::Focus }));
                ("b", values.u32(taken.is_some() as u32))
            }
            _ => return None,
        };
        Some(answer.0)
    }

    /// The action interface of a pressable element: its actions (see `ways_of`), and doing one.
    fn action(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack, deeds: &mut Vec<Deed>) -> Option<&'static str> {
        let (said, frame) = (self.said(which).filter(|said| pressed(said))?, self.frame(which)?);
        let (ways, keys) = (ways_of(said), keys_of(said));
        let answer = match member {
            "GetName" | "GetLocalizedName" => ("s", values.text(ways.get(asked.i32()? as usize).copied().unwrap_or(""))),
            "GetDescription" => ("s", values.text("")),
            "GetKeyBinding" => ("s", values.text(&keys)),
            "GetActions" => ("a(sss)", ways.iter().fold(values.open(8), |values, way| values.text(way).text("").text(if *way == ways[0] { &keys } else { "" })).close()),
            "DoAction" => {
                // The second, for an expandable tree item: expand, or collapse.
                let acts = [Some(Act::Press), (ways.len() > 1).then_some(Act::Open(said.open == Some(false)))];
                let act = acts.into_iter().nth(asked.i32()? as usize).flatten().filter(|_| said.usable);
                let done = act.is_some();
                deeds.extend(act.map(|act| Deed { window: frame.known, thing: said.id, act }));
                ("b", values.u32(done as u32))
            }
            _ => return None,
        };
        Some(answer.0)
    }

    /// The text interface of a text box: all text, the unit around a character, one character,
    /// or the selection. (Character extents aren't known here: those queries aren't answered.)
    fn text(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack) -> Option<&'static str> {
        let letters: Vec<char> = self.said(which).filter(|said| matches!(said.role, Role::TextBox | Role::Secret))?.value.chars().collect();
        let (count, read) = (letters.len() as i32, |from: usize, to: usize| letters[from.min(to)..to].iter().collect::<String>());
        let within = |at: i32| if at < 0 { count } else { at.min(count) } as usize;
        let answer = match member {
            "GetText" => ("s", values.text(&read(within(asked.i32()?), within(asked.i32()?)))),
            "GetCharacterAtOffset" => ("i", values.i32(letters.get(asked.i32()? as usize).map_or(0, |letter| *letter as i32))),
            // The selection, while focused: one range, or none.
            "GetNSelections" => ("i", values.i32(self.said(which)?.caret.is_some_and(|[from, to]| from != to) as i32)),
            "GetSelection" => {
                let [from, to] = self.said(which)?.caret.filter(|[from, to]| from != to && asked.i32() == Some(0))?;
                ("ii", values.i32s(&[from.min(to) as i32, from.max(to) as i32]))
            }
            // A unit around a character, by granularity (five kinds) or, the older way, by
            // boundary type (seven kinds).
            "GetStringAtOffset" | "GetTextAtOffset" => {
                let units: &[u32] = if member == "GetStringAtOffset" { &[0, 1, 2, 2, 2] } else { &[0, 1, 1, 2, 2, 2, 2] };
                let [from, to] = stretch(&letters, asked.i32()?, *units.get(asked.u32()? as usize)?);
                ("sii", values.text(&read(from, to)).i32s(&[from as i32, to as i32]))
            }
            // Attributes around a character, as a run: a misspelled word is "invalid: spelling"
            // (as on the web and in GTK); the rest has none.
            "GetAttributes" | "GetAttributeRun" => {
                let ([from, to], wrong) = looks(&self.said(which)?.spelt_wrong, asked.i32()?, count);
                let marks = values.open(8);
                let marks = if wrong { marks.pad(8).text("invalid").text("spelling") } else { marks };
                ("a{ss}ii", marks.close().i32s(&[from, to]))
            }
            "GetAttributeValue" => {
                let (at, name) = (asked.i32()?, asked.text()?);
                let wrong = name == "invalid" && looks(&self.said(which)?.spelt_wrong, at, count).1;
                ("s", values.text(if wrong { "spelling" } else { "" }))
            }
            "GetDefaultAttributes" | "GetDefaultAttributeSet" => ("a{ss}", values.open(8).close()),
            _ => return None,
        };
        Some(answer.0)
    }

    /// The editable text interface: the whole text replaced, with the requested insertion or
    /// deletion.
    fn editable(&self, which: Which, member: &str, asked: &mut Unpack, values: &mut Pack, deeds: &mut Vec<Deed>) -> Option<&'static str> {
        let (said, frame) = (self.said(which).filter(|said| matches!(said.role, Role::TextBox | Role::Secret) && said.usable)?, self.frame(which)?);
        let text = match member {
            "SetTextContents" => asked.text()?.to_string(),
            "InsertText" => {
                let (at, text, most) = (asked.i32()?, asked.text()?, asked.i32()?);
                spliced(&said.value, at, at, &text[..byte_at(text, most)])
            }
            "DeleteText" => spliced(&said.value, asked.i32()?, asked.i32()?, ""),
            _ => return None,
        };
        deeds.push(Deed { window: frame.known, thing: said.id, act: Act::Type(text) });
        values.u32(1);
        Some("b")
    }
}

/// The application interface: the user's locale, and whether it has its own bus to be queried
/// over (it has none).
fn application(which: Which, member: &str, asked: &mut Unpack, values: &mut Pack) -> Option<&'static str> {
    let kinds = ["MESSAGES", "COLLATE", "CTYPE", "MONETARY", "NUMERIC", "TIME"];
    match (which, member) {
        (Which::Program, "GetLocale") => values.text(&locale(kinds.get(asked.u32()? as usize)?)),
        (Which::Program, "GetApplicationBusAddress") => values.text(""),
        _ => return None,
    };
    Some("s")
}

/// The peer interface every bus client has: `Ping` (the registry pings every 30 seconds,
/// removing an application that doesn't answer), and the machine id.
fn peer(member: &str, values: &mut Pack) -> Option<&'static str> {
    let machine = || ["/etc/machine-id", "/var/lib/dbus/machine-id"].iter().find_map(|file| std::fs::read_to_string(file).ok());
    match member {
        "Ping" => Some(""),
        "GetMachineId" => {
            values.text(machine()?.trim());
            Some("s")
        }
        _ => None,
    }
}

/// Whether the service's properties say someone listens: either of its two flags, the old one
/// and the newer.
fn someone_listens(mut said: Unpack) -> Option<bool> {
    let (end, mut someone) = (said.list(8)?, false);
    while said.at < end {
        let (name, value) = (said.take(8, 0).and(said.text())?, said.value()?);
        someone |= matches!(name, "IsEnabled" | "ScreenReaderEnabled") && value == Value::Number(1.0);
    }
    Some(someone)
}

/// The event listeners as the registry lists them: bus name and event type pairs.
fn hearers(mut listed: Unpack) -> Option<Vec<[String; 2]>> {
    let (end, mut hearers) = (listed.list(8)?, Vec::new());
    while listed.at < end {
        hearers.push([listed.take(8, 0).and(listed.text())?.into(), plainly(listed.text()?)]);
    }
    Some(hearers)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::Size;
    use crate::platform::LocalClipboard;
    use crate::text::BuiltInFont;
    use std::os::unix::net::UnixListener;

    /// The other end of a socket, playing the bus and its peers.
    pub(crate) struct Peer {
        stream: UnixStream,
        come: Vec<u8>,
        serial: u32,
    }

    /// A message as a bus delivers it: type, serial, and numbered fields, each with its type
    /// code (the sender, field 7, is written by the bus).
    pub(crate) fn packed(kind: u8, serial: u32, fields: &[(u8, &str, &str)], body: &Pack) -> Vec<u8> {
        let mut message = Pack::default();
        message.byte(b'l').byte(kind).byte(0).byte(1).u32(body.bytes.len() as u32).u32(serial).open(8);
        for (code, letter, value) in fields {
            message.pad(8).byte(*code).kinds(letter);
            match *letter {
                "u" => message.u32(value.parse().unwrap()),
                "g" => message.kinds(value),
                _ => message.text(value),
            };
        }
        message.close().pad(8).bytes.extend_from_slice(&body.bytes);
        message.bytes
    }

    /// A message's body signature, from its header.
    fn kinds_in(bytes: &[u8]) -> String {
        let mut head = Unpack { bytes, at: 12, little: true };
        let end = head.list(8).unwrap();
        while head.at < end {
            if let (8, Value::Text(kinds)) = (head.take(8, 1).unwrap()[0], head.value().unwrap()) {
                return kinds.into();
            }
        }
        String::new()
    }

    impl Peer {
        pub(crate) fn new(stream: UnixStream) -> Self {
            stream.set_nonblocking(true).unwrap();
            Peer { stream, come: Vec::new(), serial: 100 }
        }

        /// Reads everything the program sent so far.
        fn fill(&mut self) {
            let mut piece = [0; 4096];
            while let Ok(len) = self.stream.read(&mut piece) {
                if len == 0 {
                    break;
                }
                self.come.extend_from_slice(&piece[..len]);
            }
        }

        /// The program's next line, before the bus takes messages.
        fn line(&mut self) -> String {
            self.fill();
            let end = self.come.windows(2).position(|pair| pair == b"\r\n").expect("a line");
            String::from_utf8(self.come.drain(..end + 2).collect()).unwrap()
        }

        /// The whole messages the program sent since last asked.
        pub(crate) fn messages(&mut self) -> Vec<Vec<u8>> {
            self.fill();
            let mut messages = Vec::new();
            while let Some(len) = Message::span(&self.come).filter(|len| *len <= self.come.len()) {
                let rest = self.come.split_off(len);
                messages.push(std::mem::replace(&mut self.come, rest));
            }
            messages
        }

        /// The calls and signals among them: each with its member name, and the message.
        pub(crate) fn said(&mut self) -> Vec<(String, Vec<u8>)> {
            let named = |bytes: &Vec<u8>| Message::read(bytes).map(|message| message.member.to_string()).unwrap();
            self.messages().into_iter().map(|bytes| (named(&bytes), bytes)).collect()
        }

        pub(crate) fn send(&mut self, bytes: &[u8]) {
            self.stream.write_all(bytes).unwrap();
        }

        /// Replies to message `to`, as a bus or service does.
        pub(crate) fn answer(&mut self, to: u32, kinds: &str, body: &Pack) {
            self.serial += 1;
            let fields = [(5, "u", to.to_string()), (7, "s", ":1.0".into()), (8, "g", kinds.into())];
            let fields: Vec<(u8, &str, &str)> = fields.iter().map(|(code, letter, value)| (*code, *letter, value.as_str())).collect();
            self.send(&packed(ANSWER, self.serial, &fields, body));
        }

        /// Plays a bus accepting the program: reads its authentication and BEGIN, confirms, and
        /// assigns a name; `turn` then has the program read that. Returns the rest the program
        /// had sent by then: all of it at once, with no reply awaited.
        pub(crate) fn let_on(&mut self, turn: &mut dyn FnMut()) -> Vec<(String, Vec<u8>)> {
            // SAFETY: geteuid cannot fail.
            let me: String = unsafe { geteuid() }.to_string().bytes().map(|digit| format!("{digit:02x}")).collect();
            assert_eq!([self.line(), self.line()], [format!("\0AUTH EXTERNAL {me}\r\n"), "BEGIN\r\n".into()]);
            let mut said = self.said();
            let (first, hello) = said.remove(0);
            assert_eq!((first.as_str(), Message::read(&hello).unwrap().serial), ("Hello", 1), "being let on is asked first");
            self.send(b"OK 0123456789abcdef0123456789abcdef\r\n");
            self.answer(1, "s", Pack::default().text(":1.42"));
            turn();
            said
        }

        /// Calls a method of one of the program's objects, as a screen reader does, lets the
        /// program take its turn, and returns the actions it was asked to do, and its reply:
        /// whether a success, the signature, and the message.
        fn ask(&mut self, readers: &mut Readers, path: &str, interface: &str, member: &str, kinds: &str, body: &Pack) -> (Vec<Deed>, bool, String, Vec<u8>) {
            self.serial += 1;
            let serial = self.serial;
            let mut fields = vec![(1, "o", path), (2, "s", interface), (3, "s", member), (7, "s", ":1.7")];
            if !kinds.is_empty() {
                fields.push((8, "g", kinds));
            }
            self.send(&packed(CALL, serial, &fields, body));
            let deeds = readers.turn(&|_| [100, 50]);
            let answers = |bytes: &Vec<u8>| Message::read(bytes).is_some_and(|message| message.answers == serial);
            let answer = self.messages().into_iter().find(answers).expect("an answer");
            (deeds, Message::read(&answer).unwrap().kind == ANSWER, kinds_in(&answer), answer)
        }

        /// The same for calls taking one number or none and returning the needed values: the
        /// signature, and the values to read.
        fn get(&mut self, readers: &mut Readers, path: &str, interface: &str, member: &str, numbers: &[i32]) -> (String, Vec<u8>) {
            let kinds = "i".repeat(numbers.len());
            let (_, fine, kinds, answer) = self.ask(readers, path, interface, member, &kinds, Pack::default().i32s(numbers));
            assert!(fine, "{member} of {path} is answered");
            (kinds, answer)
        }

        /// One property of an object: its signature and the reply to read it from.
        fn property(&mut self, readers: &mut Readers, path: &str, interface: &str, name: &str) -> Vec<u8> {
            let (_, fine, kinds, answer) = self.ask(readers, path, PROPERTIES, "Get", "ss", Pack::default().text(interface).text(name));
            assert!(fine && kinds == "v", "{name} of {path} is answered");
            answer
        }

        /// The signals the program sent since last asked: object, type (as AT-SPI writes it),
        /// and its two detail numbers.
        fn news(&mut self) -> Vec<(String, String, [i32; 2])> {
            let mut news = Vec::new();
            for bytes in self.messages() {
                let message = Message::read(&bytes).unwrap();
                let mut body = message.body;
                if message.kind == NEWS {
                    assert_eq!(kinds_in(&bytes), "siiva{sv}");
                    let kind = format!("{}:{}:{}", message.interface.trim_start_matches("org.a11y.atspi.Event."), message.member, body.text().unwrap());
                    news.push((message.path.to_string(), kind, [body.i32().unwrap(), body.i32().unwrap()]));
                }
            }
            news
        }
    }

    /// A reply's body, to read.
    fn body(answer: &[u8]) -> Unpack<'_> {
        Message::read(answer).unwrap().body
    }
    /// The string, or number, in a property reply.
    fn text_in(answer: &[u8]) -> String {
        match body(answer).value() {
            Some(Value::Text(text)) => text.into(),
            other => panic!("no text: {other:?}"),
        }
    }
    fn number_in(answer: &[u8]) -> f64 {
        match body(answer).value() {
            Some(Value::Number(number)) => number,
            other => panic!("no number: {other:?}"),
        }
    }

    /// A window with a table as the library's parts build one: three columns (sorted by the
    /// second, descending), a kept-in-view totals row, and 300 selectable rows, the third
    /// selected; each row's third column has a button.
    fn accounts(ui: &mut Ui, table: &mut crate::Table, picked: usize) {
        use crate::{Align, Column};
        const COLUMNS: [Column; 3] = [Column::new("Name"), Column::new("Size").px(80.0).align(Align::End), Column::new("Do").px(90.0).unsorted()];
        ui.table(table, &COLUMNS);
        ui.table_row().kept().open();
        ui.label("All");
        ui.label("600");
        ui.label("");
        ui.close();
        for row in ui.rows(300, 24.0) {
            ui.pick_row(row, row == picked);
            ui.label(&format!("file {row}"));
            ui.label(&format!("{}", row * 2));
            ui.button("Open");
            ui.close();
        }
        ui.close();
        ui.close();
    }

    /// The object path in a reply.
    fn path_in(answer: &[u8], property: bool) -> String {
        let mut body = body(answer);
        if property {
            body.kinds();
        }
        body.take(8, 0).and(body.text()).and(body.text()).unwrap_or_default().to_string()
    }
    /// The numbers in a reply, consecutive, or as an array.
    fn numbers_in(answer: &[u8], count: usize) -> Vec<i32> {
        let mut body = body(answer);
        (0..count).map_while(|_| body.i32()).collect()
    }
    fn list_in(answer: &[u8]) -> Vec<i32> {
        let mut body = body(answer);
        let end = body.list(4).unwrap();
        std::iter::from_fn(|| (body.at < end).then(|| body.i32()).flatten()).collect()
    }

    #[test]
    fn a_table_is_told_as_a_table_of_rows_and_cells_under_headings() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
        ui.listen(true);
        let mut kept = crate::Table { sort: vec![(1, true)], ..Default::default() };
        (0..2).for_each(|_| _ = ui.run(&mut |ui| accounts(ui, &mut kept, 2)));
        let (mut readers, mut peer) = told_of(&mut ui);
        let r = &mut readers;
        let place = |r: &Readers, role: Role, name: &str| r.frames[0].said.iter().position(|said| said.role == role && said.name == name).unwrap_or_else(|| panic!("no {role:?} {name:?}"));
        let path = |r: &Readers, role: Role, name: &str| r.path(Which::Thing(0, place(r, role, name)));
        let (table, size, row, cell) = (path(r, Role::Table, ""), path(r, Role::Heading, "Size"), path(r, Role::TableRow, "file 2, 4, Open"), path(r, Role::Cell, "4"));
        // Each one's role, and its interfaces.
        let role = |r: &mut Readers, peer: &mut Peer, path: &str| body(&peer.get(r, path, ACCESSIBLE, "GetRole", &[]).1).u32();
        assert_eq!([&table, &size, &row, &cell].map(|path| role(r, &mut peer, path)), [Some(55), Some(10), Some(90), Some(56)]);
        let has = |r: &mut Readers, peer: &mut Peer, path: &str, interface: &str| {
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetInterfaces", &[]);
            let mut names = body(&answer);
            let end = names.list(4).unwrap();
            std::iter::from_fn(|| (names.at < end).then(|| names.text()).flatten()).any(|name| name == interface)
        };
        assert!(has(r, &mut peer, &table, TABLE) && has(r, &mut peer, &cell, TABLE_CELL) && has(r, &mut peer, &row, ACTION) && !has(r, &mut peer, &row, TABLE));

        // The table: 301 rows and 3 columns, one selected row, no caption.
        let number = |r: &mut Readers, peer: &mut Peer, name: &str| number_in(&peer.property(r, &table, TABLE, name));
        assert_eq!(["NRows", "NColumns", "NSelectedRows", "NSelectedColumns"].map(|name| number(r, &mut peer, name)), [301.0, 3.0, 1.0, 0.0]);
        assert_eq!(path_in(&peer.property(r, &table, TABLE, "Caption"), true), NOTHING);
        // A cell by row and column; null for a row out of view, or a missing one.
        let at = |r: &mut Readers, peer: &mut Peer, row: i32, column: i32| path_in(&peer.get(r, &table, TABLE, "GetAccessibleAt", &[row, column]).1, false);
        assert_eq!([at(r, &mut peer, 3, 1), at(r, &mut peer, 200, 0), at(r, &mut peer, 3, 3), at(r, &mut peer, -1, 0)], [cell.clone(), NOTHING.into(), NOTHING.into(), NOTHING.into()]);
        // Cell indices along the rows, and back.
        let ask = |r: &mut Readers, peer: &mut Peer, member: &str, numbers: &[i32], count: usize| numbers_in(&peer.get(r, &table, TABLE, member, numbers).1, count);
        assert_eq!(
            [ask(r, &mut peer, "GetIndexAt", &[3, 1], 1), ask(r, &mut peer, "GetRowAtIndex", &[10], 1), ask(r, &mut peer, "GetColumnAtIndex", &[10], 1), ask(r, &mut peer, "GetRowAtIndex", &[903], 1)],
            [[10], [3], [1], [-1]]
        );
        let (_, answer) = peer.get(r, &table, TABLE, "GetRowColumnExtentsAtIndex", &[10]);
        let mut extents = body(&answer);
        assert_eq!((0..6).map(|_| extents.i32()).collect::<Vec<_>>(), [1, 3, 1, 1, 1, 1].map(Some));
        // The column headers; the selected rows.
        assert_eq!(path_in(&peer.get(r, &table, TABLE, "GetColumnHeader", &[1]).1, false), size);
        assert_eq!(body(&peer.get(r, &table, TABLE, "GetColumnDescription", &[1]).1).text(), Some("Size"));
        assert_eq!(
            (list_in(&peer.get(r, &table, TABLE, "GetSelectedRows", &[]).1), ask(r, &mut peer, "IsRowSelected", &[3], 1), ask(r, &mut peer, "IsSelected", &[4, 0], 1)),
            (vec![3], vec![1], vec![0])
        );

        // A cell: its row and column, spanning one each, its table, its header.
        let answer = peer.property(r, &cell, TABLE_CELL, "Position");
        let mut position = body(&answer);
        assert_eq!((position.kinds(), position.take(8, 0).and(position.i32()), position.i32()), (Some("(ii)"), Some(3), Some(1)));
        assert_eq!(path_in(&peer.property(r, &cell, TABLE_CELL, "Table"), true), table);
        assert_eq!(numbers_in(&peer.get(r, &cell, TABLE_CELL, "GetRowColumnSpan", &[]).1, 4), [3, 1, 1, 1]);
        let (_, answer) = peer.get(r, &cell, TABLE_CELL, "GetColumnHeaderCells", &[]);
        let mut heads = body(&answer);
        let end = heads.list(8).unwrap();
        assert_eq!((heads.take(8, 0).and(heads.text()).and(heads.text()), heads.at == end), (Some(size.as_str()), true));
        // A selected row is selectable, and selected; the sorted column's header says so.
        let states = numbers_in(&peer.get(r, &row, ACCESSIBLE, "GetState", &[]).1, 3);
        assert!(states.len() == 3 && states[1] >> (SELECTABLE as i32) & 1 == 1 && states[1] >> (SELECTED as i32) & 1 == 1, "{states:?}");
        let (_, answer) = peer.get(r, &size, ACCESSIBLE, "GetAttributes", &[]);
        let mut noted = body(&answer);
        let end = noted.list(8).unwrap();
        let pairs: Vec<(String, String)> = std::iter::from_fn(|| (noted.at < end).then(|| Some((noted.take(8, 0).and(noted.text())?.to_string(), noted.text()?.to_string()))).flatten()).collect();
        assert!(pairs.contains(&("sort".into(), "descending".into())), "{pairs:?}");

        // Another row is selected and the sort reversed: the old row is deselected, the new one
        // selected, and the header's attribute changes.
        kept.sort = vec![(1, false)];
        (0..2).for_each(|_| _ = ui.run(&mut |ui| accounts(ui, &mut kept, 3)));
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        let name_of = |readers: &Readers, of: &str| readers.which(of).and_then(|which| readers.said(which)).map_or(String::new(), |said| said.name.clone());
        let news: Vec<(String, String, i32)> = peer.news().into_iter().map(|(of, kind, numbers)| (name_of(&readers, &of), kind, numbers[0])).collect();
        let heard = |name: &str, kind: &str, number: i32| news.contains(&(name.to_string(), kind.to_string(), number));
        assert!(heard("file 2, 4, Open", "Object:StateChanged:selected", 0) && heard("file 3, 6, Open", "Object:StateChanged:selected", 1), "{news:?}");
        assert!(heard("Size", "Object:AttributesChanged:", 0) && !news.iter().any(|(_, kind, _)| kind.ends_with(":pressed")), "{news:?}");
    }

    /// An object's attributes (`GetAttributes`), as name-value pairs.
    fn noted_on(r: &mut Readers, peer: &mut Peer, path: &str) -> Vec<(String, String)> {
        let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetAttributes", &[]);
        let mut noted = body(&answer);
        let end = noted.list(8).unwrap();
        std::iter::from_fn(|| (noted.at < end).then(|| Some((noted.take(8, 0).and(noted.text())?.to_string(), noted.text()?.to_string()))).flatten()).collect()
    }

    #[test]
    fn a_tree_is_told_as_a_tree_of_items_that_open_close_and_are_picked() {
        use crate::branches::tests::{grove, grown};
        let (mut ui, mut tree) = grown();
        let (mut readers, mut peer) = told_of(&mut ui);
        let r = &mut readers;
        let path = |r: &Readers, name: &str| path_of(r, name);
        let whole = r.path(Which::Thing(0, r.frames[0].said.iter().position(|said| said.role == Role::Tree).unwrap()));
        let [closed, open, note, picked] = ["folder 0", "folder 1", "note 10", "note 11"].map(|name| path(r, name));
        // Each one's role and interfaces: a multi-selectable tree, and its items, which are
        // pressable.
        let role = |r: &mut Readers, peer: &mut Peer, path: &str| body(&peer.get(r, path, ACCESSIBLE, "GetRole", &[]).1).u32();
        assert_eq!([&whole, &closed, &note].map(|path| role(r, &mut peer, path)), [Some(65), Some(91), Some(91)]);
        let has = |r: &mut Readers, peer: &mut Peer, path: &str, interface: &str| {
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetInterfaces", &[]);
            let mut names = body(&answer);
            let end = names.list(4).unwrap();
            std::iter::from_fn(|| (names.at < end).then(|| names.text()).flatten()).any(|name| name == interface)
        };
        assert!(has(r, &mut peer, &whole, SELECTION) && has(r, &mut peer, &note, ACTION) && !has(r, &mut peer, &note, SELECTION));
        // Each one's state: expanded, collapsed, selected; and the tree, multi-selectable.
        let states = |r: &mut Readers, peer: &mut Peer, path: &str| {
            let words = numbers_in(&peer.get(r, path, ACCESSIBLE, "GetState", &[]).1, 3);
            [EXPANDABLE, EXPANDED, COLLAPSED, SELECTABLE, SELECTED, MULTISELECTABLE].map(|state| words[1] >> state & 1 == 1)
        };
        assert_eq!(states(r, &mut peer, &open), [true, true, false, true, false, false]);
        assert_eq!(states(r, &mut peer, &closed), [true, false, true, true, false, false]);
        assert_eq!(states(r, &mut peer, &picked), [false, false, false, true, true, false]);
        assert!(states(r, &mut peer, &whole)[5]);
        // Each one's level (as Orca reads it), and position in set.
        let pairs = |level: &str, nth: &str, count: &str| [("posinset", nth), ("setsize", count), ("level", level)].map(|(what, number)| (what.to_string(), number.to_string())).to_vec();
        assert_eq!((noted_on(r, &mut peer, &open), noted_on(r, &mut peer, &picked)), (pairs("1", "2", "10"), pairs("2", "2", "3")));
        // Each one's actions: click, and expand or collapse for one with children.
        let actions = |r: &mut Readers, peer: &mut Peer, path: &str| {
            let count = number_in(&peer.property(r, path, ACTION, "NActions")) as i32;
            (0..count).map(|nth| body(&peer.get(r, path, ACTION, "GetName", &[nth]).1).text().unwrap_or_default().to_string()).collect::<Vec<_>>()
        };
        assert_eq!([actions(r, &mut peer, &closed), actions(r, &mut peer, &open), actions(r, &mut peer, &note)], [vec!["click", "expand"], vec!["click", "collapse"], vec!["click"]]);

        // The selected ones, by child index: the fourth child.
        let ask = |r: &mut Readers, peer: &mut Peer, member: &str, numbers: &[i32]| {
            let (deeds, fine, _, answer) = peer.ask(r, &whole, SELECTION, member, &"i".repeat(numbers.len()), Pack::default().i32s(numbers));
            assert!(fine, "{member} is answered");
            (deeds.into_iter().map(|deed| (deed.thing, deed.act)).collect::<Vec<_>>(), body(&answer).u32())
        };
        assert_eq!(number_in(&peer.property(r, &whole, SELECTION, "NSelectedChildren")), 1.0);
        assert_eq!(path_in(&peer.get(r, &whole, SELECTION, "GetSelectedChild", &[0]).1, false), picked);
        assert_eq!([ask(r, &mut peer, "IsChildSelected", &[3]).1, ask(r, &mut peer, "IsChildSelected", &[2]).1], [Some(1), Some(0)]);
        let id = |r: &Readers, name: &str| r.frames[0].said.iter().find(|said| said.name == name).unwrap().id;
        let (note_id, picked_id) = (id(r, "note 10"), id(r, "note 11"));
        // Selected alone, deselected (as the keys do); an unselected one needs nothing; select
        // all and clear aren't supported.
        assert_eq!(ask(r, &mut peer, "SelectChild", &[2]), (vec![(note_id, Act::Press)], Some(1)));
        assert_eq!(ask(r, &mut peer, "DeselectChild", &[3]), (vec![(picked_id, Act::Pick)], Some(1)));
        assert_eq!(ask(r, &mut peer, "DeselectChild", &[2]), (vec![], Some(0)));
        assert_eq!(ask(r, &mut peer, "DeselectSelectedChild", &[0]), (vec![(picked_id, Act::Pick)], Some(1)));
        assert_eq!(ask(r, &mut peer, "ClearSelection", &[]), (vec![], Some(0)));

        // A screen reader collapses the open folder: its notes go, and the events say so.
        let (deeds, ..) = peer.ask(r, &open, ACTION, "DoAction", "i", Pack::default().i32s(&[1]));
        assert_eq!(deeds.iter().map(|deed| &deed.act).collect::<Vec<_>>(), [&Act::Open(false)]);
        deeds.into_iter().for_each(|deed| r.act(&mut ui, deed));
        ui.run(&mut |ui| grove(ui, &mut tree));
        r.keep(&mut ui, 0, "Settings window", [300, 400], 1.0);
        let news = peer.news();
        assert!(news.contains(&(open.clone(), "Object:StateChanged:expanded".into(), [0, 0])), "{news:?}");
        assert!(news.iter().any(|(of, kind, _)| *of == whole && kind == "Object:ChildrenChanged:remove"), "{news:?}");
        assert!(r.frames[0].said.iter().all(|said| !said.name.starts_with("note")));
    }

    /// The test app's state.
    #[derive(Default)]
    struct App {
        dark: bool,
        size: u8,
        volume: f32,
        name: String,
        applied: u32,
        more: bool,
        /// Whether spell checking is on; and how many text lines come last.
        checked: bool,
        lines: usize,
    }

    /// A window with one element of each kind.
    fn describe(app: &mut App, ui: &mut Ui) {
        ui.label("Settings");
        app.applied += ui.button("Apply").clicked as u32;
        ui.checkbox(&mut app.dark, "Dark");
        ui.radio(&mut app.size, 0, "Small");
        ui.radio(&mut app.size, 1, "Large");
        ui.slider(&mut app.volume, 0.0..=1.0);
        if app.checked {
            ui.spell_with(None, crate::spell::tests::stand_in, true);
        }
        ui.text_box(&mut app.name, "Name");
        ui.progress(0.25);
        if app.more {
            ui.button("More");
        }
        for line in 0..app.lines {
            ui.label(&format!("Line {line}"));
        }
    }

    /// A 420 by 300 UI with a screen reader listening, run so every box knows its place.
    fn window(app: &mut App) -> Ui {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
        ui.listen(true);
        run(&mut ui, app);
        run(&mut ui, app);
        ui
    }
    fn run(ui: &mut Ui, app: &mut App) {
        ui.run(&mut |ui| describe(app, ui));
    }

    /// The screen readers' side of a program on the accessibility bus, with the bus's other
    /// end; after the window above was reported once.
    fn on_the_bus() -> (Readers, Peer) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (mut readers, mut peer) = (Readers::new(None), Peer::new(theirs));
        readers.desk = Bus::over(ours);
        assert!(peer.let_on(&mut || drop(readers.turn(&|_| [0; 2]))).is_empty() && readers.listening());
        (readers, peer)
    }

    /// The session bus as the windows own it, with the screen readers heard over it; and the
    /// bus's other end.
    struct Session {
        bus: Option<Bus>,
        readers: Readers,
    }

    impl Session {
        fn new() -> (Self, Peer) {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let mut bus = Bus::over(ours);
            let readers = Readers::new(bus.as_mut());
            (Session { bus, readers }, Peer::new(theirs))
        }

        /// One turn, as the windows take it: session bus messages go to the screen readers, a
        /// dead bus is dropped, then the accessibility bus is read.
        fn turn(&mut self) {
            let Session { bus, readers } = self;
            bus.iter_mut().for_each(|bus| bus.turn(&mut |bus, message| readers.hear_session(bus, message)));
            bus.take_if(|bus| bus.dead);
            readers.turn(&|_| [0; 2]);
        }
    }
    fn told_of(ui: &mut Ui) -> (Readers, Peer) {
        let (mut readers, mut peer) = on_the_bus();
        readers.keep(ui, 0, "Settings window", [420, 300].map(|units| (units as f32 * ui.scale()) as i32), ui.scale());
        readers.settle();
        peer.messages();
        (readers, peer)
    }

    /// The path of the element named `name` in the first window.
    fn path_of(readers: &Readers, name: &str) -> String {
        let at = readers.frames[0].said.iter().position(|said| said.name == name).unwrap_or_else(|| panic!("nothing is called {name}"));
        readers.path(Which::Thing(0, at))
    }

    /// A deterministic pseudo-random sequence.
    struct Chance(u64);
    impl Chance {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, most: usize) -> usize {
            (self.next() % most.max(1) as u64) as usize
        }
    }

    #[test]
    fn values_are_laid_out_as_d_bus_lays_them_out() {
        let mut pack = Pack::default();
        pack.byte(7).u32(0x0102_0304).text("a\u{17c}").kinds("a(so)").f64(1.5).i32(-2);
        pack.open(4).u32(1).u32(2).close().open(8).close().object(":1.5", "/a").open(8).pad(8).text("k").kinds("s").text("v").close();
        #[rustfmt::skip]
        let bytes = [
            7, 0, 0, 0, 4, 3, 2, 1,                       // a byte, and a number lined up on four
            3, 0, 0, 0, b'a', 0xc5, 0xbc, 0,              // a text: its length in bytes, and a zero after it
            5, b'a', b'(', b's', b'o', b')', 0, 0,        // kinds: a length of one byte; then on to eight
            0, 0, 0, 0, 0, 0, 0xf8, 0x3f,                 // 1.5
            0xfe, 0xff, 0xff, 0xff,                       // -2
            8, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0,           // a list of two numbers: eight bytes long
            0, 0, 0, 0,                                   // an empty list of things lined up on eight:
            0, 0, 0, 0,                                   // its length, and the way to where they would be
            4, 0, 0, 0, b':', b'1', b'.', b'5', 0, 0, 0, 0, 2, 0, 0, 0, b'/', b'a', 0, 0, // an object
            18, 0, 0, 0,                                  // a list of one name with its value,
            1, 0, 0, 0, b'k', 0, 1, b's', 0, 0, 0, 0, 1, 0, 0, 0, b'v', 0,
        ];
        assert_eq!(pack.bytes, bytes);

        let mut unpack = Unpack { bytes: &pack.bytes, at: 0, little: true };
        assert_eq!((unpack.byte(), unpack.u32(), unpack.text(), unpack.kinds(), unpack.f64(), unpack.i32()), (Some(7), Some(0x0102_0304), Some("a\u{17c}"), Some("a(so)"), Some(1.5), Some(-2)));
        let end = unpack.list(4).unwrap();
        assert_eq!((unpack.u32(), unpack.u32(), unpack.at == end), (Some(1), Some(2), true));
        let end = unpack.list(8).unwrap();
        assert_eq!(unpack.at, end, "nothing is in the empty list");
        assert_eq!((unpack.take(8, 0).map(|_| ()), unpack.text(), unpack.text()), (Some(()), Some(":1.5"), Some("/a")));
        let end = unpack.list(8).unwrap();
        assert_eq!((unpack.take(8, 0).and(unpack.text()), unpack.value(), unpack.at == end), (Some("k"), Some(Value::Text("v")), true));
        assert_eq!((unpack.byte(), unpack.u32(), unpack.text()), (None, None, None), "and after the last value there is none");

        // Each basic type as a variant; and both byte orders of a number.
        type Write = fn(&mut Pack) -> &mut Pack;
        let said_with_its_kind: [(&str, Write, Value); 8] = [
            ("y", |pack| pack.byte(200), Value::Number(200.0)),
            ("b", |pack| pack.u32(1), Value::Number(1.0)),
            ("u", |pack| pack.u32(4_000_000_000), Value::Number(4e9)),
            ("i", |pack| pack.i32(-5), Value::Number(-5.0)),
            ("d", |pack| pack.f64(0.25), Value::Number(0.25)),
            ("s", |pack| pack.text("s"), Value::Text("s")),
            ("o", |pack| pack.text("/o"), Value::Text("/o")),
            ("g", |pack| pack.kinds("ii"), Value::Text("ii")),
        ];
        for (kinds, write, value) in said_with_its_kind {
            let mut pack = Pack::default();
            write(pack.kinds(kinds));
            assert_eq!(Unpack { bytes: &pack.bytes, at: 0, little: true }.value(), Some(value), "{kinds}");
        }
        let other_way = [0, 0, 1, 2, 0x40, 0x09, 0x21, 0xfb, 0x54, 0x44, 0x2d, 0x18];
        let mut unpack = Unpack { bytes: &other_way, at: 0, little: false };
        assert_eq!((unpack.u32(), unpack.take(4, 0).map(|_| ()), Unpack { bytes: &other_way[4..], at: 0, little: false }.f64()), (Some(258), Some(()), Some(std::f64::consts::PI)));
        // A non-basic variant is no value; a NUL in a string is dropped.
        assert_eq!(Unpack { bytes: &[2, b'a', b's', 0, 0, 0, 0, 0], at: 0, little: true }.value(), None);
        assert_eq!(Pack::default().text("a\0b").bytes, [2, 0, 0, 0, b'a', b'b', 0]);
    }

    #[test]
    fn values_of_any_kind_are_passed_over() {
        // A value of every basic type; arrays of basic and of struct elements; bare structs;
        // and variants, one containing another. After them all, a number, which is what is
        // wanted.
        let kinds = "ybnqiuxtdsoga{sv}a(ss)(sa(us))vvai";
        let mut pack = Pack::default();
        pack.byte(7).u32(1).pad(2).bytes.extend([1, 0, 2, 0]);
        pack.i32(-3).u32(4).f64(0.0).f64(0.0).f64(1.5).text("a text").text("/a/path").kinds("a(so)");
        pack.open(8).pad(8).text("name").kinds("s").text("value").pad(8).text("other").kinds("u").u32(9).close();
        pack.open(8).pad(8).text("a").text("b").close();
        pack.pad(8).text("called").open(8).pad(8).u32(0).text("*.png").close();
        pack.kinds("(sv)").pad(8).text("in").kinds("ai").open(4).i32(1).i32(2).close();
        pack.kinds("s").text("x").open(4).close().u32(77);
        let over = |bytes: &[u8], kinds: &'static str| {
            let mut values = Unpack { bytes, at: 0, little: true };
            (values.skip(kinds), values.u32(), values.at)
        };
        assert_eq!(over(&pack.bytes, kinds), (Some(()), Some(77), pack.bytes.len()));
        // Truncated anywhere before the number, they don't parse as that signature.
        for len in 0..pack.bytes.len() - 4 {
            assert_eq!(over(&pack.bytes[..len], kinds).0, None, "cut short after {len} bytes");
        }
        // Nor does anything parse as an unknown type, an array of nothing, or an array of an
        // unclosed struct.
        assert_eq!(["z", "a", "a(s", "a{sv"].map(|kinds| over(&pack.bytes, kinds).0), [None; 4]);
        // A variant nesting itself is followed as far as the bytes go, and no further.
        let mut endless = Pack::default();
        (0..100_000).for_each(|_| _ = endless.kinds("v"));
        assert_eq!(over(&endless.bytes, "v").0, None);
    }

    #[test]
    fn a_message_goes_out_and_is_read_back() {
        // `Hello`, byte for byte.
        let hello = Header { kind: CALL, to: BUS, path: BUS_PATH, interface: BUS, member: "Hello", ..Default::default() }.pack(1, &[]);
        let mut bytes = vec![b'l', 1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 109, 0, 0, 0];
        bytes.extend([1, 1, b'o', 0, 21, 0, 0, 0]);
        bytes.extend(b"/org/freedesktop/DBus\0\0\0");
        bytes.extend([2, 1, b's', 0, 20, 0, 0, 0]);
        bytes.extend(b"org.freedesktop.DBus\0\0\0\0");
        bytes.extend([3, 1, b's', 0, 5, 0, 0, 0]);
        bytes.extend(b"Hello\0\0\0");
        bytes.extend([6, 1, b's', 0, 20, 0, 0, 0]);
        bytes.extend(b"org.freedesktop.DBus\0\0\0\0");
        assert_eq!(hello, bytes);
        assert_eq!(Message::span(&hello), Some(hello.len()));

        // Every message type, with its fields.
        let mut values = Pack::default();
        values.text("held").i32(-3);
        let call = Header { kind: CALL, flags: 2, to: ":1.9", path: "/a/b", interface: "a.B", member: "Do", kinds: "si", ..Default::default() }.pack(7, &values.bytes);
        let read = Message::read(&call).unwrap();
        assert_eq!((read.kind, read.serial, read.answers, read.waited_on), (CALL, 7, 0, true));
        assert_eq!((read.path, read.interface, read.member, read.sender), ("/a/b", "a.B", "Do", ""));
        let mut body = read.body;
        assert_eq!((body.text(), body.i32(), body.byte(), kinds_in(&call).as_str(), call[2]), (Some("held"), Some(-3), None, "si", 2));
        assert_eq!(Message::span(&call), Some(call.len()));

        let failed = Header { kind: FAILED, failure: "a.Failure", answers: 7, to: ":1.9", kinds: "s", ..Default::default() }.pack(8, &Pack::default().text("why").bytes);
        let read = Message::read(&failed).unwrap();
        assert_eq!((read.kind, read.serial, read.answers, read.body.clone().text()), (FAILED, 8, 7, Some("why")));

        // As a bus delivers one, with the sender added; and one expecting no reply.
        let mut handed = packed(NEWS, 9, &[(1, "o", "/p"), (2, "s", "i.J"), (3, "s", "Changed"), (7, "s", ":1.3"), (9, "u", "0")], &Pack::default());
        let read = Message::read(&handed).unwrap();
        assert_eq!((read.kind, read.path, read.interface, read.member, read.sender), (NEWS, "/p", "i.J", "Changed", ":1.3"), "a field that is not known is passed over");
        handed[2] = 1;
        assert!(!Message::read(&handed).unwrap().waited_on);

        // The same message from a big-endian sender.
        let mut other = vec![b'B', 2, 0, 1, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 15];
        other.extend([5, 1, b'u', 0, 0, 0, 0, 3, 8, 1, b'g', 0, 1, b'u', 0, 0]);
        other.extend([0, 0, 1, 0]);
        let read = Message::read(&other).unwrap();
        assert_eq!((Message::span(&other), read.kind, read.serial, read.answers, read.body.clone().u32()), (Some(other.len()), ANSWER, 5, 3, Some(256)));
    }

    /// Whatever arrives from a bus is read without harm: a message truncated anywhere, one with
    /// any byte changed, and random bytes.
    #[test]
    fn a_message_cut_short_or_scrambled_is_read_as_far_as_it_holds() {
        let mut values = Pack::default();
        values.text("org.a11y.Status").open(8).pad(8).text("IsEnabled").kinds("b").u32(1).close().open(4).text("gone").close();
        let whole = packed(NEWS, 3, &[(1, "o", LAUNCHER_PATH), (2, "s", PROPERTIES), (3, "s", "PropertiesChanged"), (7, "s", ":1.2"), (8, "g", "sa{sv}as")], &values);
        let read_all = |bytes: &[u8]| {
            let Some(message) = Message::read(bytes) else { return };
            let mut body = message.body;
            std::hint::black_box((body.text(), someone_listens(body), hearers(message.body), body.value(), body.f64(), body.kinds()));
        };
        for len in 0..whole.len() {
            assert!(Message::read(&whole[..len]).is_none(), "cut short after {len} bytes");
            Message::span(&whole[..len]);
        }
        let mut chance = Chance(0x2545_f491_4f6c_dd1d);
        for _ in 0..20_000 {
            let mut scrambled = whole.clone();
            for _ in 0..1 + chance.below(3) {
                let at = chance.below(scrambled.len());
                scrambled[at] = chance.next() as u8;
            }
            scrambled.truncate(scrambled.len() - chance.below(8) * chance.below(2));
            read_all(&scrambled);
            Message::span(&scrambled);
            let noise: Vec<u8> = (0..chance.below(200)).map(|_| chance.next() as u8).collect();
            read_all(&noise);
            Message::span(&noise);
        }
        // Lengths claiming more than there is, up to the largest a number can hold.
        for claimed in [whole.len() as u32, 1 << 20, u32::MAX - 7, u32::MAX] {
            let mut lying = whole.clone();
            lying[4..8].copy_from_slice(&claimed.to_le_bytes());
            assert!(Message::read(&lying).is_none());
            lying[12..16].copy_from_slice(&claimed.to_le_bytes());
            assert!(Message::read(&lying).is_none());
            Message::span(&lying);
        }
        assert_eq!(Message::span(&[b'x'; 16]), None, "what does not begin as a message has no length");
    }

    #[test]
    fn the_address_of_a_bus_is_read() {
        assert_eq!(plain("/run/user/1000/at-spi/bus_0"), b"/run/user/1000/at-spi/bus_0");
        assert_eq!(plain("a%20b%2c%3B%zz%4"), b"a b,;%zz%4", "what is no byte written out stays as it is");
        // A bus at a path with a space and a comma; listed after a missing one, one of another
        // transport, and an invalid address.
        let folder = std::env::temp_dir().join(format!("taut atspi,{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let listener = UnixListener::bind(folder.join("bus")).unwrap();
        let coded = folder.join("bus").to_string_lossy().replace('%', "%25").replace(' ', "%20").replace(',', "%2c");
        assert!(connect(&format!("unix:path={coded},guid=0123")).is_some());
        assert!(connect(&format!("unix:guid=0123,path={coded}")).is_some());
        let bus = Bus::open(&format!("unix:path=/nowhere/at/all;tcp:host=localhost,port=1;nonsense;;unix:path={coded}")).expect("the one that is there");
        let (mut theirs, mut said) = ([(); 3].map(|_| listener.accept().unwrap().0), [0; 15]);
        theirs[2].read_exact(&mut said).unwrap();
        assert_eq!((&said, bus.greeted, bus.dead), (b"\0AUTH EXTERNAL ", false, false));
        std::fs::remove_dir_all(&folder).unwrap();
        for nowhere in ["", ";", "unix:", "unix:path=", "unix:path=/nowhere/at/all", "unix:abstract=taut-test-nobody-is-here", "unix:tmpdir=/tmp", "tcp:host=localhost"] {
            assert!(Bus::open(nowhere).is_none(), "{nowhere:?}");
        }
    }

    /// A bus at an abstract socket name, as Linux has: where older desktops put the
    /// accessibility bus.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_bus_under_a_name_that_is_no_file_is_found() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("taut-atspi-test-{}", std::process::id());
        let listener = UnixListener::bind_addr(&std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap()).unwrap();
        assert!(Bus::open(&format!("unix:abstract={name},guid=0123")).is_some());
        assert!(listener.accept().is_ok());
    }

    #[test]
    fn a_bus_is_greeted_and_asked_in_one_go() {
        let (mut session, mut peer) = Session::new();
        // Authentication, BEGIN, and all the program's calls go at once, in order: no reply
        // awaited between.
        let asked: Vec<String> = peer.let_on(&mut || session.turn()).into_iter().map(|(member, _)| member).collect();
        assert_eq!(asked, ["AddMatch", "AddMatch", "GetAll"]);
        assert!(session.bus.is_some() && !session.readers.listening(), "being on the session bus is not being listened to");

        // A bus rejecting the program is dropped; so is one saying nothing a bus says, and one
        // talking without ending its line.
        for words in [&b"REJECTED EXTERNAL\r\n"[..], b"l\x01\0\x01\0\0\0\0\x01\0\0\0\0\0\0\0\r\n", &[b'x'; 5000]] {
            let (mut session, mut peer) = Session::new();
            peer.send(words);
            session.turn();
            assert!(session.bus.is_none(), "{:?}", &words[..8]);
            assert_eq!(session.readers.waits(), (-1, 0));
        }
    }

    /// The test's own session bus, if any, is spoken to as every bus here: authentication,
    /// BEGIN and the first call all at once. Whichever bus implementation it is (desktops use
    /// two), it answers each in turn. (Only names are asked: no other client on it hears of the
    /// test.)
    #[test]
    fn the_sessions_own_bus_takes_what_is_said_at_once() {
        let Some(mut bus) = Bus::session() else { return eprintln!("no session bus: not tested") };
        let asked = bus.call(BUS, BUS_PATH, BUS, "GetId", "", &Pack::default());
        let (mut answers, since) = (Vec::new(), std::time::Instant::now());
        while answers.len() < 2 && !bus.dead && since.elapsed() < std::time::Duration::from_secs(5) {
            bus.turn(&mut |_, message| {
                let mut said = message.body;
                answers.extend((message.kind == ANSWER).then(|| (message.answers, said.text().unwrap_or_default().to_string())));
            });
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(bus.greeted && !bus.dead, "the bus let the program on");
        assert_eq!(answers.iter().map(|(to, _)| *to).collect::<Vec<_>>(), [1, asked]);
        assert!(answers[0].1.starts_with(':') && answers[1].1.len() == 32, "its name for the program, and its own: {answers:?}");
    }

    #[test]
    fn a_message_that_comes_in_pieces_is_kept_until_it_is_whole() {
        let (mut session, mut peer) = Session::new();
        let asked = peer.let_on(&mut || session.turn());
        let serial = Message::read(&asked[2].1).unwrap().serial;
        // The "someone listens" reply a byte at a time: nothing happens until its last byte.
        // Then the program asks for the accessibility bus address.
        let mut values = Pack::default();
        values.open(8).pad(8).text("IsEnabled").kinds("b").u32(1).close();
        let answer = packed(ANSWER, 9, &[(5, "u", &serial.to_string()), (7, "s", ":1.0"), (8, "g", "a{sv}")], &values);
        for byte in &answer[..answer.len() - 1] {
            peer.send(&[*byte]);
            session.turn();
        }
        assert!(peer.said().is_empty());
        peer.send(&answer[answer.len() - 1..]);
        // Two messages in one chunk are both read: the second signals the answer may have
        // changed, so it is asked again.
        peer.send(&packed(NEWS, 10, &[(1, "o", LAUNCHER_PATH), (2, "s", PROPERTIES), (3, "s", "PropertiesChanged"), (7, "s", ":1.0")], &Pack::default()));
        session.turn();
        let asked: Vec<String> = peer.said().into_iter().map(|(member, _)| member).collect();
        assert_eq!(asked, if std::env::var_os("AT_SPI_BUS_ADDRESS").is_some() { vec!["GetAll"] } else { vec!["GetAddress", "GetAll"] });
    }

    #[test]
    fn a_message_too_long_to_take_is_let_pass() {
        let (mut session, mut peer) = Session::new();
        peer.let_on(&mut || session.turn());
        // More than the largest accepted, then a call answered as usual.
        let mut long = packed(CALL, 50, &[(1, "o", "/"), (3, "s", "Huge"), (7, "s", ":1.7"), (8, "g", "ay")], &Pack::default());
        long[4..8].copy_from_slice(&(MOST as u32 + 5000).to_le_bytes());
        long.resize(long.len() + MOST + 5000, 0x55);
        long.extend(packed(CALL, 51, &[(1, "o", "/"), (2, "s", PEER), (3, "s", "Ping"), (7, "s", ":1.7")], &Pack::default()));
        let (mut sent, mut most_kept) = (0, 0);
        while sent < long.len() {
            match peer.stream.write(&long[sent..]) {
                Ok(len) => sent += len,
                Err(_) => session.turn(),
            }
            most_kept = most_kept.max(session.bus.as_ref().unwrap().come.capacity());
        }
        (0..64).for_each(|_| session.turn());
        assert!(most_kept <= 16 * 4096, "nothing of it is kept: {most_kept}");
        let answers: Vec<u32> = peer.messages().iter().map(|bytes| Message::read(bytes).unwrap().answers).collect();
        assert_eq!(answers, [51], "the call after it is answered, and it is not");
    }

    #[test]
    fn a_bus_that_is_gone_is_let_go_of() {
        let mut app = App::default();
        let mut ui = window(&mut app);
        let (mut readers, peer) = told_of(&mut ui);
        assert!(readers.listening() && !readers.frames.is_empty());
        drop(peer);
        readers.turn(&|_| [0; 2]);
        assert!(!readers.listening() && readers.frames.is_empty() && readers.hearers.is_none(), "nobody listens, and nothing is kept");
        assert_eq!(readers.waits(), (-1, 0));
        // Reporting the window anyway comes to nothing, harmlessly.
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        assert!(!readers.listening());

        // One whose other end closes while data is still queued.
        let (mut readers, peer) = on_the_bus();
        drop(peer);
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        readers.turn(&|_| [0; 2]);
        assert!(!readers.listening());
    }

    #[test]
    fn whether_anyone_listens_is_asked_of_the_session_and_followed() {
        if std::env::var_os("AT_SPI_BUS_ADDRESS").is_some() {
            return eprintln!("the desktop says outright where the accessibility bus is: not tested");
        }
        let (mut session, mut peer) = Session::new();
        let asked = peer.let_on(&mut || session.turn());
        // The service's properties are asked for, without auto-starting it.
        let (member, get_all) = &asked[2];
        let read = Message::read(get_all).unwrap();
        assert_eq!((member.as_str(), read.path, read.interface, read.body.clone().text(), get_all[2]), ("GetAll", LAUNCHER_PATH, PROPERTIES, Some("org.a11y.Status"), 2));
        let says = |peer: &mut Peer, to: u32, words: &[(&str, u32)]| {
            let mut values = Pack::default();
            values.open(8);
            for (word, on) in words {
                values.pad(8).text(word).kinds("b").u32(*on);
            }
            peer.answer(to, "a{sv}", values.close());
        };
        // Nobody listens: nothing more is asked, and no accessibility bus sought.
        says(&mut peer, read.serial, &[("IsEnabled", 0), ("ScreenReaderEnabled", 0)]);
        session.turn();
        assert!(peer.said().is_empty() && session.readers.desk.is_none());

        // A signal that it may have changed: asked again. Now someone listens (by the older
        // flag): the accessibility bus address is requested, and that bus joined.
        let changed = |peer: &mut Peer| peer.send(&packed(NEWS, 77, &[(1, "o", LAUNCHER_PATH), (2, "s", PROPERTIES), (3, "s", "PropertiesChanged"), (7, "s", ":1.0")], &Pack::default()));
        let owned = |peer: &mut Peer, name: &str| {
            peer.send(&packed(NEWS, 78, &[(1, "o", BUS_PATH), (2, "s", BUS), (3, "s", "NameOwnerChanged"), (7, "s", BUS), (8, "g", "sss")], Pack::default().text(name).text("").text(":1.9")));
        };
        let asks_again = |peer: &mut Peer, session: &mut Session| {
            session.turn();
            let said = peer.said();
            assert_eq!(said.iter().map(|(member, _)| member.as_str()).collect::<Vec<_>>(), ["GetAll"]);
            Message::read(&said[0].1).unwrap().serial
        };
        changed(&mut peer);
        let serial = asks_again(&mut peer, &mut session);
        says(&mut peer, serial, &[("ScreenReaderEnabled", 1)]);
        session.turn();
        let said = peer.said();
        let read = Message::read(&said[0].1).unwrap();
        assert_eq!((said.len(), said[0].0.as_str(), read.interface, read.path), (1, "GetAddress", LAUNCHER, LAUNCHER_PATH));
        let folder = std::env::temp_dir().join(format!("taut-atspi-session-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let listener = UnixListener::bind(folder.join("bus")).unwrap();
        peer.answer(read.serial, "s", Pack::default().text(&format!("unix:path={}", folder.join("bus").display())));
        session.turn();
        let mut desk = Peer::new(listener.accept().expect("the program comes to the second bus").0);
        // There it subscribes to the registry's signals, and is listened to from then on.
        let rules: Vec<String> = desk.let_on(&mut || session.turn()).iter().map(|(member, asked)| format!("{member} {}", body(asked).text().unwrap())).collect();
        let rule = |of: &str| format!("AddMatch type='signal',sender='{of}',interface='{of}'");
        assert_eq!(rules, [rule(REGISTRY), format!("{},member='NameOwnerChanged',arg0='{REGISTRY}'", rule(BUS))]);
        assert!(session.readers.listening());

        // Other parts use the session bus too. Signals not from the service (another name
        // changing owner, a desktop setting changing) and replies to others' calls are ignored:
        // nothing is asked again.
        owned(&mut peer, "org.example.Other");
        peer.send(&packed(
            NEWS,
            79,
            &[(1, "o", "/org/example"), (2, "s", "org.example.Settings"), (3, "s", "SettingChanged"), (7, "s", ":1.5"), (8, "g", "ss")],
            Pack::default().text(LAUNCHER).text("x"),
        ));
        peer.answer(4000, "u", Pack::default().u32(1));
        session.turn();
        assert!(peer.said().is_empty() && session.readers.listening());

        // The service restarts (its name changes owner): asked again. It no longer says anyone
        // listens, and the accessibility bus is left.
        owned(&mut peer, LAUNCHER);
        let serial = asks_again(&mut peer, &mut session);
        says(&mut peer, serial, &[]);
        session.turn();
        assert!(session.readers.desk.is_none() && !session.readers.listening());
        desk.fill();
        assert_eq!(desk.stream.read(&mut [0; 8]).ok(), Some(0), "the socket to it is closed");

        // And where the service isn't running at all, the reply is an error: nobody listens.
        owned(&mut peer, LAUNCHER);
        let serial = asks_again(&mut peer, &mut session);
        peer.send(&packed(FAILED, 80, &[(4, "s", "org.freedesktop.DBus.Error.ServiceUnknown"), (5, "u", &serial.to_string()), (7, "s", BUS), (8, "g", "s")], Pack::default().text("no such service")));
        session.turn();
        assert!(peer.said().is_empty() && session.readers.desk.is_none());
        // Pinged on this bus too, it replies; anything else, it can't.
        let mut asks = |path: &str, interface: &str, member: &str, serial: u32| {
            peer.send(&packed(CALL, serial, &[(1, "o", path), (2, "s", interface), (3, "s", member), (7, "s", ":1.7")], &Pack::default()));
            session.turn();
            let answers: Vec<(u8, u32)> = peer.messages().iter().map(|bytes| Message::read(bytes).unwrap()).map(|answer| (answer.kind, answer.answers)).collect();
            answers == [(ANSWER, serial)]
        };
        assert_eq!((asks("/", PEER, "Ping", 90), asks(ROOT, ACCESSIBLE, "GetChildren", 91)), (true, false));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_registry_is_told_that_the_program_is_there() {
        let mut app = App::default();
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = on_the_bus();
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        assert!(peer.said().iter().all(|(member, _)| member != "Embed"), "not before the windows of a run are all taken in");
        readers.settle();
        let said = peer.said();
        let asked: Vec<&str> = said.iter().map(|(member, _)| member.as_str()).filter(|member| !member.contains("Changed") && !member.contains("ate")).collect();
        assert_eq!(asked, ["Embed", "GetRegisteredEvents"]);
        let embed = Message::read(&said.iter().find(|(member, _)| member == "Embed").unwrap().1).unwrap();
        let mut me = embed.body;
        assert_eq!((embed.path, embed.interface, me.text(), me.text()), (ROOT, "org.a11y.atspi.Socket", Some(":1.42"), Some(ROOT)));
        // The registry replies with the desktop object, the application's parent from now on.
        peer.answer(embed.serial, "(so)", Pack::default().object(":1.0", ROOT));
        readers.turn(&|_| [0; 2]);
        let parent = peer.property(&mut readers, ROOT, ACCESSIBLE, "Parent");
        let mut parent = body(&parent);
        assert_eq!((parent.kinds(), parent.take(8, 0).and(parent.text()), parent.text()), (Some("(so)"), Some(":1.0"), Some(ROOT)));
        // It assigns the application an id, which the application keeps.
        let (_, fine, ..) = peer.ask(&mut readers, ROOT, PROPERTIES, "Set", "ssv", Pack::default().text(APPLICATION).text("Id").kinds("i").i32(31));
        assert!(fine && number_in(&peer.property(&mut readers, ROOT, APPLICATION, "Id")) == 31.0);
        // Another run reports nothing again.
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        assert!(peer.said().is_empty());
        // The registry's name gets an owner: the registry starting when first activated, and it
        // knows of this program. But when it leaves and comes back, the program registers
        // again.
        let owned = |peer: &mut Peer, readers: &mut Readers, was: &str, now: &str| {
            peer.send(&packed(NEWS, 90, &[(1, "o", BUS_PATH), (2, "s", BUS), (3, "s", "NameOwnerChanged"), (7, "s", BUS)], Pack::default().text(REGISTRY).text(was).text(now)));
            readers.turn(&|_| [0; 2]);
            peer.said().into_iter().map(|(member, _)| member).collect::<Vec<_>>()
        };
        assert_eq!(owned(&mut peer, &mut readers, "", ":1.0"), [""; 0]);
        assert_eq!(owned(&mut peer, &mut readers, ":1.0", ""), [""; 0]);
        assert_eq!(owned(&mut peer, &mut readers, "", ":1.77"), ["Embed", "GetRegisteredEvents"]);
        assert_eq!(owned(&mut peer, &mut readers, "", ":1.77"), [""; 0]);
    }

    #[test]
    fn a_window_is_told_as_a_tree_of_what_is_in_it() {
        let mut app = App { volume: 0.5, name: "Ada L".into(), ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        let r = &mut readers;

        // The application, with the window as its child.
        let (kinds, answer) = peer.get(r, ROOT, ACCESSIBLE, "GetRole", &[]);
        assert_eq!((kinds.as_str(), body(&answer).u32()), ("u", Some(75)));
        assert_eq!(text_in(&peer.property(r, ROOT, APPLICATION, "ToolkitName")), "taut");
        assert_eq!(text_in(&peer.property(r, ROOT, APPLICATION, "AtspiVersion")), "2.1");
        assert_eq!(number_in(&peer.property(r, ROOT, ACCESSIBLE, "ChildCount")), 1.0);
        let (kinds, answer) = peer.get(r, ROOT, ACCESSIBLE, "GetChildren", &[]);
        let mut children = body(&answer);
        let end = children.list(8).unwrap();
        let window = (children.take(8, 0).and(children.text()), children.text().unwrap().to_string());
        assert_eq!((kinds.as_str(), window.0, children.at == end), ("a(so)", Some(":1.42"), true));
        let window = window.1;
        let (_, answer) = peer.get(r, ROOT, ACCESSIBLE, "GetInterfaces", &[]);
        let mut names = body(&answer);
        names.list(4).unwrap();
        assert_eq!((names.text(), names.text(), names.text()), (Some(ACCESSIBLE), Some(APPLICATION), None));
        let (_, fine, ..) = peer.ask(r, ROOT, COMPONENT, "GetSize", "", &Pack::default());
        assert!(!fine, "the program itself is nowhere");

        // The window: a frame with its title, its size, and active.
        assert_eq!(text_in(&peer.property(r, &window, ACCESSIBLE, "Name")), "Settings window");
        let (_, answer) = peer.get(r, &window, ACCESSIBLE, "GetRole", &[]);
        let (_, name) = peer.get(r, &window, ACCESSIBLE, "GetRoleName", &[]);
        assert_eq!((body(&answer).u32(), body(&name).text()), (Some(23), Some("frame")));
        let (kinds, answer) = peer.get(r, &window, ACCESSIBLE, "GetState", &[]);
        let mut states = body(&answer);
        states.list(4).unwrap();
        let states = states.u32().unwrap() as u64 | (states.u32().unwrap() as u64) << 32;
        assert_eq!((kinds.as_str(), states), ("au", 1 << ACTIVE | 1 << ENABLED | 1 << SENSITIVE | 1 << SHOWING | 1 << VISIBLE | 1 << RESIZABLE));
        let (kinds, answer) = peer.get(r, &window, COMPONENT, "GetExtents", &[0]);
        let mut extents = body(&answer);
        assert_eq!((kinds.as_str(), extents.take(8, 0).is_some(), [(); 4].map(|_| extents.i32().unwrap())), ("(iiii)", true, [100, 50, 420, 300]), "on the screen, where the window is");
        let (_, answer) = peer.get(r, &window, ACCESSIBLE, "GetIndexInParent", &[]);
        assert_eq!(body(&answer).i32(), Some(0));

        // Each element, in order: role, name, state, extents, and interfaces.
        let (_, answer) = peer.get(r, &window, ACCESSIBLE, "GetChildren", &[]);
        let mut children = body(&answer);
        let (end, mut paths) = (children.list(8).unwrap(), Vec::new());
        while children.at < end {
            children.take(8, 0).and(children.text()).unwrap();
            paths.push(children.text().unwrap().to_string());
        }
        let said = ui.said().to_vec();
        assert_eq!((paths.len(), said.len()), (8, 8));
        let common = 1 << ENABLED | 1 << SENSITIVE | 1 << SHOWING | 1 << VISIBLE;
        let expected = [
            (29, "label", "Settings", common, vec![ACCESSIBLE, COMPONENT]),
            (43, "button", "Apply", common | 1 << FOCUSABLE, vec![ACCESSIBLE, COMPONENT, ACTION]),
            (7, "check box", "Dark", common | 1 << FOCUSABLE | 1 << CHECKABLE, vec![ACCESSIBLE, COMPONENT, ACTION]),
            (44, "radio button", "Small", common | 1 << FOCUSABLE | 1 << CHECKABLE | 1 << CHECKED, vec![ACCESSIBLE, COMPONENT, ACTION]),
            (44, "radio button", "Large", common | 1 << FOCUSABLE | 1 << CHECKABLE, vec![ACCESSIBLE, COMPONENT, ACTION]),
            (51, "slider", "", common | 1 << FOCUSABLE, vec![ACCESSIBLE, COMPONENT, VALUE]),
            (79, "entry", "Name", common | 1 << FOCUSABLE | 1 << EDITABLE_NOW, vec![ACCESSIBLE, COMPONENT, TEXT, EDITABLE]),
            (42, "progress bar", "", common | 1u64 << READ_ONLY, vec![ACCESSIBLE, COMPONENT, VALUE]),
        ];
        for (at, (path, (role, role_name, name, states, interfaces))) in paths.iter().zip(expected).enumerate() {
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetRole", &[]);
            let (_, called) = peer.get(r, path, ACCESSIBLE, "GetRoleName", &[]);
            assert_eq!((body(&answer).u32(), body(&called).text(), text_in(&peer.property(r, path, ACCESSIBLE, "Name")).as_str()), (Some(role), Some(role_name), name));
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetState", &[]);
            let mut told = body(&answer);
            told.list(4).unwrap();
            assert_eq!(told.u32().unwrap() as u64 | (told.u32().unwrap() as u64) << 32, states, "{name} {role_name}");
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetInterfaces", &[]);
            let mut names = body(&answer);
            let (end, mut told) = (names.list(4).unwrap(), Vec::new());
            while names.at < end {
                told.push(names.text().unwrap());
            }
            assert_eq!(told, interfaces, "{name} {role_name}");
            let (_, answer) = peer.get(r, path, COMPONENT, "GetExtents", &[1]);
            let mut extents = body(&answer);
            extents.take(8, 0).unwrap();
            let rect = said[at].rect;
            assert_eq!([(); 4].map(|_| extents.i32().unwrap()), [rect.x, rect.y, rect.w, rect.h].map(|units| units.round() as i32), "{name} {role_name}");
            let (_, answer) = peer.get(r, path, ACCESSIBLE, "GetIndexInParent", &[]);
            let parent = peer.property(r, path, ACCESSIBLE, "Parent");
            let mut parent = body(&parent);
            assert_eq!((body(&answer).i32(), parent.kinds(), parent.take(8, 0).and(parent.text()), parent.text()), (Some(at as i32), Some("(so)"), Some(":1.42"), Some(window.as_str())));
            let (_, answer) = peer.get(r, &window, ACCESSIBLE, "GetChildAtIndex", &[at as i32]);
            let mut child = body(&answer);
            assert_eq!((child.take(8, 0).and(child.text()), child.text()), (Some(":1.42"), Some(path.as_str())));
            assert_eq!(text_in(&peer.property(r, path, ACCESSIBLE, "AccessibleId")), format!("{:016x}", said[at].id));
        }

        // Extents on screen and relative to the parent; and hit testing.
        let button = said[1].rect;
        let (x, y) = ((button.x + button.w / 2.0) as i32, (button.y + button.h / 2.0) as i32);
        let (kinds, answer) = peer.get(r, &paths[1], COMPONENT, "GetPosition", &[0]);
        let mut place = body(&answer);
        assert_eq!((kinds.as_str(), place.i32(), place.i32()), ("ii", Some(button.x.round() as i32 + 100), Some(button.y.round() as i32 + 50)));
        let (_, answer) = peer.get(r, &paths[1], COMPONENT, "GetPosition", &[2]);
        let mut place = body(&answer);
        assert_eq!((place.i32(), place.i32()), (Some(button.x.round() as i32), Some(button.y.round() as i32)), "counted from the window it is in");
        let (kinds, answer) = peer.get(r, &paths[1], COMPONENT, "GetSize", &[]);
        let mut size = body(&answer);
        assert_eq!((kinds.as_str(), size.i32(), size.i32()), ("ii", Some(button.w.round() as i32), Some(button.h.round() as i32)));
        for (place, kind, found) in [([x, y], 1, paths[1].as_str()), ([x + 100, y + 50], 0, paths[1].as_str()), ([x, y], 0, NOTHING), ([415, 295], 1, NOTHING), ([-5, 2], 1, NOTHING)] {
            let (kinds, answer) = peer.get(r, &window, COMPONENT, "GetAccessibleAtPoint", &[place[0], place[1], kind]);
            let mut at = body(&answer);
            assert_eq!((kinds.as_str(), at.take(8, 0).and(at.text()), at.text()), ("(so)", Some(":1.42"), Some(found)), "{place:?} {kind}");
        }
        let (_, answer) = peer.get(r, &paths[1], COMPONENT, "Contains", &[x, y, 1]);
        let (_, outside) = peer.get(r, &paths[1], COMPONENT, "Contains", &[x, y + 200, 1]);
        assert_eq!((body(&answer).u32(), body(&outside).u32()), (Some(1), Some(0)));
        let (kinds, answer) = peer.get(r, &paths[1], COMPONENT, "GetLayer", &[]);
        let (_, of_window) = peer.get(r, &window, COMPONENT, "GetLayer", &[]);
        let (order, depth) = peer.get(r, &paths[1], COMPONENT, "GetMDIZOrder", &[]);
        let (alpha, see_through) = peer.get(r, &paths[1], COMPONENT, "GetAlpha", &[]);
        assert_eq!((kinds.as_str(), body(&answer).u32(), body(&of_window).u32()), ("u", Some(3), Some(7)));
        assert_eq!((order.as_str(), body(&depth).take(2, 2), alpha.as_str(), body(&see_through).f64()), ("n", Some(&[0xff, 0xff][..]), "d", Some(1.0)));

        // A pressable element's actions; and a value.
        assert_eq!(number_in(&peer.property(r, &paths[2], ACTION, "NActions")), 1.0);
        let (kinds, answer) = peer.get(r, &paths[2], ACTION, "GetName", &[0]);
        let (list_kinds, list) = peer.get(r, &paths[2], ACTION, "GetActions", &[]);
        let mut list = body(&list);
        let end = list.list(8).unwrap();
        assert_eq!((kinds.as_str(), body(&answer).text(), list_kinds.as_str()), ("s", Some("click"), "a(sss)"));
        assert_eq!((list.take(8, 0).and(list.text()), list.text(), list.text(), list.at == end), (Some("click"), Some(""), Some(""), true));
        let (_, fine, ..) = peer.ask(r, &paths[0], ACTION, "GetActions", "", &Pack::default());
        assert!(!fine, "a text is not pressed");
        // A slider step is a twentieth of its range.
        for (name, value) in [("CurrentValue", 0.5), ("MinimumValue", 0.0), ("MaximumValue", 1.0), ("MinimumIncrement", 0.05)] {
            assert!((number_in(&peer.property(r, &paths[5], VALUE, name)) - value).abs() < 1e-6, "{name}");
        }
        assert_eq!(number_in(&peer.property(r, &paths[7], VALUE, "CurrentValue")), 0.25);
        // All of one interface's properties at once, by name.
        let (_, fine, kinds, answer) = peer.ask(r, &paths[5], PROPERTIES, "GetAll", "s", Pack::default().text(VALUE));
        let mut all = body(&answer);
        let (end, mut names) = (all.list(8).unwrap(), Vec::new());
        while all.at < end {
            names.push(all.take(8, 0).and(all.text()).unwrap());
            all.value().unwrap();
        }
        assert_eq!((fine, kinds.as_str(), names), (true, "a{sv}", NAMED[3].1.to_vec()));
        let (_, _, _, answer) = peer.ask(r, &paths[0], PROPERTIES, "GetAll", "s", Pack::default().text(VALUE));
        let mut none = body(&answer);
        assert_eq!(none.list(8), Some(none.at), "a text has no amount");

        // What no longer exists, or never did, gets the error screen readers know a defunct
        // object by; what an object doesn't support, "not supported".
        for (path, interface, member, failure) in [
            ("/org/a11y/atspi/accessible/1/99999", ACCESSIBLE, "GetRole", GONE),
            ("/org/a11y/atspi/accessible/7", ACCESSIBLE, "GetRole", GONE),
            ("/org/a11y/atspi/accessible/1/2/3", ACCESSIBLE, "GetRole", GONE),
            ("/org/a11y/atspi/cache", "org.a11y.atspi.Cache", "GetNothing", UNKNOWN),
            ("/org/a11y/atspi/cache", ACCESSIBLE, "GetRole", GONE),
            (ROOT, "org.a11y.atspi.Cache", "GetNothing", UNKNOWN),
            (window.as_str(), ACCESSIBLE, "GetNothing", UNKNOWN),
            (window.as_str(), "", "GetRole", UNKNOWN),
        ] {
            let (_, fine, kinds, answer) = peer.ask(r, path, interface, member, "", &Pack::default());
            let mut head = Unpack { bytes: &answer, at: 12, little: true };
            let (end, mut named) = (head.list(8).unwrap(), "");
            while head.at < end {
                if let (4, Value::Text(name)) = (head.take(8, 1).unwrap()[0], head.value().unwrap()) {
                    named = name;
                }
            }
            assert_eq!((fine, kinds.as_str(), named, body(&answer).text()), (false, "s", failure.0, Some(format!("{} \u{201c}{path}\u{201d}", failure.1).as_str())));
        }
        let (_, fine, ..) = peer.ask(r, &window, ACCESSIBLE, "GetChildAtIndex", "i", Pack::default().i32(8));
        let (_, below, ..) = peer.ask(r, &window, ACCESSIBLE, "GetChildAtIndex", "i", Pack::default().i32(-1));
        assert!(!fine && !below, "there is no ninth thing, and none before the first");
        // Liveness may be asked of any path, and is answered with nothing.
        let (_, fine, kinds, _) = peer.ask(r, "/", PEER, "Ping", "", &Pack::default());
        assert_eq!((fine, kinds.as_str()), (true, ""));
        // And the cache that libatspi first requests is empty.
        let (_, fine, kinds, answer) = peer.ask(r, "/org/a11y/atspi/cache", "org.a11y.atspi.Cache", "GetItems", "", &Pack::default());
        let mut items = body(&answer);
        assert_eq!((fine, kinds.as_str(), items.list(8)), (true, "a((so)(so)(so)iiassusau)", Some(8)));
    }

    #[test]
    fn groups_are_told_with_what_is_in_them() {
        let (mut open, mut dark) = (crate::Panel::default(), false);
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 420.0, h: 300.0 }, 2.0);
        ui.listen(true);
        let mut describe = |ui: &mut Ui| {
            ui.label("Outside");
            if ui.panel(&mut open, "Options") {
                ui.checkbox(&mut dark, "Dark");
                ui.close();
            }
        };
        ui.run(&mut describe);
        ui.run(&mut describe);
        let (mut readers, mut peer) = told_of(&mut ui);
        let said = ui.said().to_vec();
        let (group, tick) = (said.iter().position(|said| said.role == Role::Group).expect("a panel"), said.iter().position(|said| said.role == Role::Tick).unwrap());
        assert_eq!(said[tick].within, Some(group));
        let [panel, inside] = [group, tick].map(|at| readers.path(Which::Thing(0, at)));
        let r = &mut readers;
        let (_, answer) = peer.get(r, &panel, ACCESSIBLE, "GetRole", &[]);
        assert_eq!((body(&answer).u32(), text_in(&peer.property(r, &panel, ACCESSIBLE, "Name")).as_str()), (Some(39), "Options"));
        let parent = peer.property(r, &inside, ACCESSIBLE, "Parent");
        let mut parent = body(&parent);
        assert_eq!((parent.kinds(), parent.take(8, 0).and(parent.text()), parent.text()), (Some("(so)"), Some(":1.42"), Some(panel.as_str())));
        let count = said.iter().filter(|said| said.within == Some(group)).count();
        assert_eq!(number_in(&peer.property(r, &panel, ACCESSIBLE, "ChildCount")), count as f64);
        let place = said.iter().filter(|said| said.within == Some(group)).position(|said| said.role == Role::Tick).unwrap();
        let (_, answer) = peer.get(r, &inside, ACCESSIBLE, "GetIndexInParent", &[]);
        assert_eq!(body(&answer).i32(), Some(place as i32));
        // Its extents relative to its group, in pixels (two per unit here); and found by hit
        // testing as the innermost element there.
        let (tick_at, group_at) = (said[tick].rect, said[group].rect);
        let (_, answer) = peer.get(r, &inside, COMPONENT, "GetPosition", &[2]);
        let mut from_group = body(&answer);
        let expected = [(tick_at.x * 2.0).round() as i32 - (group_at.x * 2.0).round() as i32, (tick_at.y * 2.0).round() as i32 - (group_at.y * 2.0).round() as i32];
        assert_eq!([from_group.i32().unwrap(), from_group.i32().unwrap()], expected);
        let middle = [((tick_at.x + tick_at.w / 2.0) * 2.0) as i32, ((tick_at.y + tick_at.h / 2.0) * 2.0) as i32];
        for asked in [readers.path(Which::Window(0)), panel.clone()] {
            let (_, answer) = peer.get(&mut readers, &asked, COMPONENT, "GetAccessibleAtPoint", &[middle[0], middle[1], 1]);
            let mut found = body(&answer);
            assert_eq!((found.take(8, 0).and(found.text()), found.text()), (Some(":1.42"), Some(inside.as_str())));
        }
        assert_eq!(readers.below(Which::Thing(0, group)), group + 1..group + 1 + said.iter().skip(group + 1).take_while(|said| said.within.is_some_and(|within| within >= group)).count());
    }

    #[test]
    fn what_changes_is_told_as_news() {
        let mut app = App { volume: 0.5, ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = on_the_bus();
        // The first report: the window appears, active, and its elements are added, each at its
        // index.
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        let window = readers.path(Which::Window(0));
        let news = peer.news();
        let kinds: Vec<(&str, &str, [i32; 2])> = news.iter().map(|(of, kind, numbers)| (of.as_str(), kind.as_str(), *numbers)).collect();
        let first = [
            (ROOT, "Object:ChildrenChanged:add", [0, 0]),
            (window.as_str(), "Window:Create:", [0, 0]),
            (window.as_str(), "Window:Activate:", [0, 0]),
            (window.as_str(), "Object:StateChanged:active", [1, 0]),
        ];
        assert_eq!(kinds[..4], first);
        let added: Vec<[i32; 2]> = kinds[4..].iter().map(|(of, kind, numbers)| (*numbers, assert_eq!((*of, *kind), (window.as_str(), "Object:ChildrenChanged:add"))).0).collect();
        assert_eq!(added, [0, 1, 2, 3, 4, 5, 6, 7].map(|place| [place, 0]));
        // A run where nothing changed reports nothing.
        run(&mut ui, &mut app);
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        assert!(peer.messages().is_empty());

        // The app changes things: each changed element is reported.
        let told = |ui: &mut Ui, app: &mut App, readers: &mut Readers, peer: &mut Peer| {
            run(ui, app);
            readers.keep(ui, 0, "Settings window", [420, 300], 1.0);
            readers.settle();
            peer.news().into_iter().map(|(of, kind, numbers)| (readers.which(&of).and_then(|which| readers.said(which)).map_or(of, |said| said.name.clone()), kind, numbers)).collect::<Vec<_>>()
        };
        let one = |name: &str, kind: &str, numbers: [i32; 2]| (name.to_string(), kind.to_string(), numbers);
        app.dark = true;
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Dark", "Object:StateChanged:checked", [1, 0])]);
        app.size = 1;
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Small", "Object:StateChanged:checked", [0, 0]), one("Large", "Object:StateChanged:checked", [1, 0])]);
        app.volume = 0.75;
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("", "Object:PropertyChange:accessible-value", [0, 0])]);
        // Text: what was removed and what inserted, in characters.
        app.name = "h\u{e9}llo".into();
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Name", "Object:TextChanged:insert", [0, 5])]);
        app.name = "h\u{e9}y you".into();
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Name", "Object:TextChanged:delete", [2, 3]), one("Name", "Object:TextChanged:insert", [2, 5])]);
        app.name = "h\u{e9}y".into();
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Name", "Object:TextChanged:delete", [3, 4])]);

        // Focus moves to an element, then the next: it leaves one, then enters the other.
        ui.event(crate::Event::Key(crate::Key::Tab, crate::Mods::NONE));
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Apply", "Object:StateChanged:focused", [1, 0])]);
        ui.event(crate::Event::Key(crate::Key::Tab, crate::Mods::NONE));
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one("Apply", "Object:StateChanged:focused", [0, 0]), one("Dark", "Object:StateChanged:focused", [1, 0])]);
        // The window loses focus: no element is focused in it. When it regains it, the same
        // element is focused again.
        let window_name = window.clone();
        ui.event(crate::Event::Focus(false));
        assert_eq!(
            told(&mut ui, &mut app, &mut readers, &mut peer),
            [one(&window_name, "Window:Deactivate:", [0, 0]), one(&window_name, "Object:StateChanged:active", [0, 0]), one("Dark", "Object:StateChanged:focused", [0, 0])]
        );
        ui.event(crate::Event::Focus(true));
        assert_eq!(
            told(&mut ui, &mut app, &mut readers, &mut peer),
            [one(&window_name, "Window:Activate:", [0, 0]), one(&window_name, "Object:StateChanged:active", [1, 0]), one("Dark", "Object:StateChanged:focused", [1, 0])]
        );

        // An element appears and disappears: reported with its index, under a new path each
        // time.
        app.more = true;
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one(&window_name, "Object:ChildrenChanged:add", [8, 0])]);
        let first = path_of(&readers, "More");
        app.more = false;
        assert_eq!(told(&mut ui, &mut app, &mut readers, &mut peer), [one(&window_name, "Object:ChildrenChanged:remove", [0, 0])]);
        assert_eq!(readers.which(&first), None);
        app.more = true;
        told(&mut ui, &mut app, &mut readers, &mut peer);
        assert_ne!(path_of(&readers, "More"), first, "what comes again is not taken for what was there before");
        // The other elements keep their paths throughout.
        assert_eq!(readers.frames[0].numbers[..8], [2, 3, 4, 5, 6, 7, 8, 9]);

        // The window is retitled, then closed.
        readers.keep(&mut ui, 0, "Other title", [420, 300], 1.0);
        readers.settle();
        assert_eq!(peer.news(), [(window.clone(), "Object:PropertyChange:accessible-name".to_string(), [0, 0])]);
        assert_eq!(text_in(&peer.property(&mut readers, &window, ACCESSIBLE, "Name")), "Other title");
        readers.settle();
        assert_eq!(peer.news(), [(window.clone(), "Window:Destroy:".to_string(), [0, 0]), (ROOT.to_string(), "Object:ChildrenChanged:remove".to_string(), [0, 0])]);
        assert!(readers.frames.is_empty() && readers.which(&window).is_none());
    }

    #[test]
    fn only_news_that_someone_asked_for_is_sent() {
        let mut app = App { volume: 0.5, ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        // The registry lists its listeners; a change to every kind of element at once is then
        // checked against them.
        let embed = |readers: &Readers| readers.asked[3];
        let mut change = |readers: &mut Readers, peer: &mut Peer| {
            (app.dark, app.volume, app.name, app.more) = (!app.dark, app.volume + 0.05, format!("{}x", app.name), !app.more);
            run(&mut ui, &mut app);
            readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
            readers.settle();
            let mut kinds: Vec<String> = peer.news().into_iter().map(|(_, kind, _)| kind).collect();
            kinds.sort();
            kinds.dedup();
            kinds
        };
        let always = ["Object:ChildrenChanged:add", "Object:ChildrenChanged:remove", "Object:StateChanged:checked"];
        assert_eq!(change(&mut readers, &mut peer).len(), 4, "until the registry has said, everything is sent");
        peer.answer(embed(&readers), "a(ss)", Pack::default().open(8).close());
        readers.turn(&|_| [0; 2]);
        assert_eq!(change(&mut readers, &mut peer), [""; 0], "nobody asked for anything: nothing is sent");

        // A screen reader subscribes to focus events: then the always-sent events go too, and
        // nothing else.
        let registry = |peer: &mut Peer, readers: &mut Readers, member: &str, values: &Pack| {
            peer.send(&packed(NEWS, 60, &[(1, "o", REGISTRY_PATH), (2, "s", REGISTRY), (3, "s", member), (7, "s", ":1.0")], values));
            readers.turn(&|_| [0; 2]);
        };
        registry(&mut peer, &mut readers, "EventListenerRegistered", Pack::default().text(":1.7").text("Object:StateChanged:Focused").open(4).close());
        assert_eq!(change(&mut readers, &mut peer), [always[0], always[2]]);
        // It subscribes to all text events, and another one to value events.
        registry(&mut peer, &mut readers, "EventListenerRegistered", Pack::default().text(":1.7").text("Object:TextChanged:").open(4).close());
        registry(&mut peer, &mut readers, "EventListenerRegistered", Pack::default().text(":1.8").text("object:property-change:accessible-value").open(4).close());
        assert_eq!(change(&mut readers, &mut peer), [always[1], "Object:PropertyChange:accessible-value", always[2], "Object:TextChanged:insert"]);
        // The first unsubscribes from everything (as when it exits): value events still go.
        registry(&mut peer, &mut readers, "EventListenerDeregistered", Pack::default().text(":1.7").text(""));
        assert_eq!(change(&mut readers, &mut peer), [always[0], "Object:PropertyChange:accessible-value", always[2]]);
        registry(&mut peer, &mut readers, "EventListenerDeregistered", Pack::default().text(":1.8").text("Object:PropertyChange:AccessibleValue"));
        assert_eq!(change(&mut readers, &mut peer), [""; 0]);
        // One subscribing to all object events gets all object events.
        registry(&mut peer, &mut readers, "EventListenerRegistered", Pack::default().text(":1.9").text("Object::").open(4).close());
        assert_eq!(change(&mut readers, &mut peer).len(), 4);

        // The listeners the registry lists from before this program appeared are taken alike,
        // in either spelling.
        assert_eq!(
            hearers(Unpack { bytes: &Pack::default().open(8).pad(8).text(":1.7").text("Window:Activate:").pad(8).text(":1.8").text("object:text-caret-moved").close().bytes, at: 0, little: true }),
            Some(vec![[":1.7".to_string(), "window:activate:".to_string()], [":1.8".to_string(), "object:textcaretmoved".to_string()]])
        );
        for (asked, kind, covered) in [
            ("object:statechanged:focused", "object:statechanged:focused", true),
            ("object:statechanged:", "object:statechanged:focused", true),
            ("object::", "object:statechanged:focused", true),
            ("object", "object:statechanged:focused", true),
            ("object:statechanged:checked", "object:statechanged:focused", false),
            ("window::", "object:statechanged:focused", false),
            ("window:activate", "window:activate:", true),
            ("object:textchanged:insert", "object:textchanged:delete", false),
        ] {
            assert_eq!(covers(asked, kind), covered, "{asked} {kind}");
        }
    }

    #[test]
    fn a_screen_reader_works_the_window() {
        let mut app = App { volume: 0.5, name: "Ada".into(), ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        // Performs the requested actions, runs the UI as its window would, and reports the
        // result: as often as a moving slider needs.
        fn does(ui: &mut Ui, readers: &mut Readers, app: &mut App, deeds: Vec<Deed>) -> u32 {
            let mut runs = 0;
            deeds.into_iter().for_each(|deed| readers.act(ui, deed));
            loop {
                run(ui, app);
                runs += 1;
                if !readers.keep(ui, 0, "Settings window", [420, 300], 1.0) || runs > 200 {
                    readers.settle();
                    return runs;
                }
            }
        }
        let paths: Vec<String> = ["Apply", "Dark", "Large", "Name", "Settings"].iter().map(|name| path_of(&readers, name)).collect();

        // Pressing: a button, a check box, a radio button.
        let (deeds, fine, kinds, answer) = peer.ask(&mut readers, &paths[0], ACTION, "DoAction", "i", Pack::default().i32(0));
        assert_eq!((deeds.len(), fine, kinds.as_str(), body(&answer).u32()), (1, true, "b", Some(1)));
        does(&mut ui, &mut readers, &mut app, deeds);
        assert_eq!(app.applied, 1);
        for (path, large) in [(&paths[1], false), (&paths[2], true)] {
            let (deeds, ..) = peer.ask(&mut readers, path, ACTION, "DoAction", "i", Pack::default().i32(0));
            does(&mut ui, &mut readers, &mut app, deeds);
            assert_eq!((app.dark, app.size == 1), (true, large));
        }
        let news: Vec<String> = peer.news().into_iter().map(|(_, kind, numbers)| format!("{kind} {}", numbers[0])).collect();
        assert!(news.contains(&"Object:StateChanged:checked 1".to_string()) && news.contains(&"Object:StateChanged:focused 1".to_string()), "{news:?}");
        // An action it doesn't have isn't done, and fails.
        let (deeds, fine, _, answer) = peer.ask(&mut readers, &paths[0], ACTION, "DoAction", "i", Pack::default().i32(1));
        assert_eq!((deeds.len(), fine, body(&answer).u32()), (0, true, Some(0)));

        // Focus goes to an element.
        let (deeds, _, _, answer) = peer.ask(&mut readers, &paths[3], COMPONENT, "GrabFocus", "", &Pack::default());
        assert_eq!(body(&answer).u32(), Some(1));
        does(&mut ui, &mut readers, &mut app, deeds);
        assert!(ui.said().iter().find(|said| said.name == "Name").unwrap().focused);
        let (deeds, _, _, answer) = peer.ask(&mut readers, &paths[4], COMPONENT, "GrabFocus", "", &Pack::default());
        assert_eq!((deeds.len(), body(&answer).u32()), (0, Some(0)), "it cannot be on a text");

        // A slider is set: it gets there a step per run, as near as its steps allow, stopping
        // at its end.
        let slider = readers.path(Which::Thing(0, 5));
        for (to, reached) in [(0.75, 0.75), (0.52, 0.5), (0.13, 0.15), (9.0, 1.0), (-3.0, 0.0), (0.0, 0.0), (f64::NAN, 0.0)] {
            let (deeds, fine, ..) = peer.ask(&mut readers, &slider, PROPERTIES, "Set", "ssv", Pack::default().text(VALUE).text("CurrentValue").kinds("d").f64(to));
            let runs = does(&mut ui, &mut readers, &mut app, deeds);
            assert!(fine && (app.volume - reached).abs() < 1e-4 && runs <= 24, "asked for {to}: {} after {runs} runs", app.volume);
            assert!(readers.reach.is_none());
        }
        assert!((number_in(&peer.property(&mut readers, &slider, VALUE, "CurrentValue")) - 0.0).abs() < 1e-4);
        let progress = readers.path(Which::Thing(0, 7));
        let (deeds, fine, ..) = peer.ask(&mut readers, &progress, PROPERTIES, "Set", "ssv", Pack::default().text(VALUE).text("CurrentValue").kinds("d").f64(0.9));
        assert!(deeds.is_empty() && !fine, "how far something has come is not for a screen reader to say");

        // Text is set in a text box: all of it, an insertion, a deletion; in characters,
        // whatever the bytes.
        let name = paths[3].clone();
        let edit = |ui: &mut Ui, readers: &mut Readers, app: &mut App, peer: &mut Peer, member: &str, kinds: &str, values: &Pack| {
            let (deeds, fine, told, answer) = peer.ask(readers, &name, EDITABLE, member, kinds, values);
            assert!(fine && told == "b" && body(&answer).u32() == Some(1), "{member}");
            does(ui, readers, app, deeds);
            app.name.clone()
        };
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "SetTextContents", "s", Pack::default().text("\u{17c}\u{f3}\u{142}w")), "\u{17c}\u{f3}\u{142}w");
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "InsertText", "isi", Pack::default().i32(2).text("-in-").i32(4)), "\u{17c}\u{f3}-in-\u{142}w");
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "InsertText", "isi", Pack::default().i32(99).text("end and more").i32(3)), "\u{17c}\u{f3}-in-\u{142}wend");
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "DeleteText", "ii", Pack::default().i32(1).i32(6)), "\u{17c}\u{142}wend");
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "DeleteText", "ii", Pack::default().i32(5).i32(2)), "\u{17c}\u{142}d", "from either end");
        assert_eq!(edit(&mut ui, &mut readers, &mut app, &mut peer, "DeleteText", "ii", Pack::default().i32(1).i32(-1)), "\u{17c}", "to the end");
        // The clipboard isn't for a screen reader to use through this: it isn't touched.
        for (member, kinds, values) in [("CopyText", "ii", Pack::default().i32s(&[0, 1])), ("CutText", "ii", Pack::default().i32s(&[0, 1])), ("PasteText", "i", Pack::default().i32(0))] {
            let (deeds, fine, ..) = peer.ask(&mut readers, &name, EDITABLE, member, kinds, values);
            assert!(deeds.is_empty() && !fine, "{member}");
        }
        assert_eq!((ui.clipboard(), app.name.as_str()), (None, "\u{17c}"));
    }

    #[test]
    fn what_is_out_of_view_and_the_words_spelt_wrong_are_told() {
        let mut app = App { name: "the quikc fox".into(), checked: true, lines: 40, ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        // A misspelled word is a run with "invalid: spelling"; the text between misspellings
        // has no attributes.
        let name = path_of(&readers, "Name");
        let looks = |peer: &mut Peer, readers: &mut Readers, member: &str, kinds: &str, asked: &Pack| {
            let (_, fine, _, answer) = peer.ask(readers, &name, TEXT, member, kinds, asked);
            let mut looks = body(&answer);
            let (end, mut found) = (looks.list(8).unwrap(), Vec::new());
            while looks.at < end {
                found.push(format!("{}: {}", looks.take(8, 0).and(looks.text()).unwrap(), looks.text().unwrap()));
            }
            (fine, found, looks.i32().unwrap(), looks.i32().unwrap())
        };
        let r = &mut readers;
        assert_eq!(looks(&mut peer, r, "GetAttributes", "i", Pack::default().i32(6)), (true, vec!["invalid: spelling".to_string()], 4, 9));
        assert_eq!(looks(&mut peer, r, "GetAttributeRun", "ib", Pack::default().i32(1).u32(1)), (true, vec![], 0, 4));
        assert_eq!(looks(&mut peer, r, "GetAttributes", "i", Pack::default().i32(11)), (true, vec![], 9, 13));
        let value = |peer: &mut Peer, r: &mut Readers, at: i32| body(&peer.ask(r, &name, TEXT, "GetAttributeValue", "is", Pack::default().i32(at).text("invalid")).3).text().unwrap().to_string();
        assert_eq!((value(&mut peer, r, 4), value(&mut peer, r, 9)), ("spelling".to_string(), String::new()));
        // Forty lines below a 300-high window: the last is visible but not showing; the
        // window's first element is showing. (A state set is an array of two words, after its
        // length.)
        let state = |peer: &mut Peer, readers: &mut Readers, name: &str| {
            let path = path_of(readers, name);
            let words = numbers_in(&peer.get(readers, &path, ACCESSIBLE, "GetState", &[]).1, 3);
            [VISIBLE, SHOWING].map(|state| words[1 + state as usize / 32] as u32 >> (state % 32) & 1 == 1)
        };
        assert_eq!((state(&mut peer, &mut readers, "Settings"), state(&mut peer, &mut readers, "Line 39")), ([true, true], [true, false]));
        // Asked to scroll to it, the window's page scrolls, and it shows.
        let last = path_of(&readers, "Line 39");
        let (deeds, fine, kinds, answer) = peer.ask(&mut readers, &last, COMPONENT, "ScrollTo", "u", Pack::default().u32(0));
        assert_eq!((deeds.len(), fine, kinds.as_str(), body(&answer).u32()), (1, true, "b", Some(1)));
        peer.messages();
        deeds.into_iter().for_each(|deed| readers.act(&mut ui, deed));
        run(&mut ui, &mut app);
        readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
        readers.settle();
        // And an event is sent: screen readers track what is showing.
        let news: Vec<String> = peer.news().into_iter().map(|(_, kind, numbers)| format!("{kind} {}", numbers[0])).collect();
        assert!(news.contains(&"Object:StateChanged:showing 1".to_string()) && news.contains(&"Object:StateChanged:showing 0".to_string()), "{news:?}");
        assert_eq!((state(&mut peer, &mut readers, "Settings"), state(&mut peer, &mut readers, "Line 39")), ([true, false], [true, true]));
    }

    #[test]
    fn the_text_of_a_text_box_is_told_in_stretches() {
        let mut app = App { name: "One two,  thr\u{e9}e\nfour".into(), ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        let (name, r) = (path_of(&readers, "Name"), &mut readers);
        assert_eq!(number_in(&peer.property(r, &name, TEXT, "CharacterCount")), 20.0);
        assert_eq!(number_in(&peer.property(r, &name, TEXT, "CaretOffset")), -1.0, "a box that is not typed into has no cursor");
        let text = |peer: &mut Peer, r: &mut Readers, from: i32, to: i32| body(&peer.get(r, &name, TEXT, "GetText", &[from, to]).1).text().unwrap().to_string();
        assert_eq!(text(&mut peer, r, 0, -1), "One two,  thr\u{e9}e\nfour");
        assert_eq!(text(&mut peer, r, 10, 15), "thr\u{e9}e");
        assert_eq!((text(&mut peer, r, 15, 99), text(&mut peer, r, 7, 3), text(&mut peer, r, 40, 50)), ("\nfour".to_string(), String::new(), String::new()));
        // A unit around a character: the character, its word with what follows, its line.
        let around = |peer: &mut Peer, r: &mut Readers, member: &str, at: i32, unit: i32| {
            let (kinds, answer) = peer.get(r, &name, TEXT, member, &[at, unit]);
            let mut stretch = body(&answer);
            assert_eq!(kinds, "sii");
            (stretch.text().unwrap().to_string(), stretch.i32().unwrap(), stretch.i32().unwrap())
        };
        let s = |text: &str, from: i32, to: i32| (text.to_string(), from, to);
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 13, 0), s("\u{e9}", 13, 14));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 5, 1), s("two,  ", 4, 10));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 8, 1), s("two,  ", 4, 10), "what is between words goes with the word before");
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 0, 1), s("One ", 0, 4));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 12, 3), s("One two,  thr\u{e9}e\n", 0, 16));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 17, 3), s("four", 16, 20));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 17, 4), s("four", 16, 20));
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", 20, 0), s("", 20, 20), "after the last letter there is none");
        assert_eq!(around(&mut peer, r, "GetStringAtOffset", -1, 3), s("One two,  thr\u{e9}e\n", 0, 16));
        // The older boundary-type way.
        assert_eq!(around(&mut peer, r, "GetTextAtOffset", 5, 2), s("two,  ", 4, 10));
        assert_eq!(around(&mut peer, r, "GetTextAtOffset", 5, 5), s("One two,  thr\u{e9}e\n", 0, 16));
        assert_eq!(around(&mut peer, r, "GetTextAtOffset", 13, 0), s("\u{e9}", 13, 14));
        let (_, fine, ..) = peer.ask(r, &name, TEXT, "GetStringAtOffset", "iu", Pack::default().i32(0).u32(9));
        assert!(!fine, "there is no ninth kind of stretch");
        // One character, by index.
        let (_, answer) = peer.get(r, &name, TEXT, "GetCharacterAtOffset", &[13]);
        let (_, none) = peer.get(r, &name, TEXT, "GetCharacterAtOffset", &[99]);
        assert_eq!((body(&answer).i32(), body(&none).i32()), (Some(0xe9), Some(0)));
        // An unfocused box has no selection.
        let (_, answer) = peer.get(r, &name, TEXT, "GetNSelections", &[]);
        assert_eq!(body(&answer).i32(), Some(0));
        // What isn't known here isn't answered: a selection not selected, setting the caret,
        // attributes, character extents.
        for (member, numbers) in [("GetSelection", &[0][..]), ("SetCaretOffset", &[3]), ("GetCharacterExtents", &[3, 0]), ("GetOffsetAtPoint", &[5, 5, 1])] {
            let (_, fine, ..) = peer.ask(r, &name, TEXT, member, &"i".repeat(numbers.len()), Pack::default().i32s(numbers));
            assert!(!fine, "{member}");
        }
        let (_, fine, ..) = peer.ask(r, &path_of(r, "Apply"), TEXT, "GetText", "ii", Pack::default().i32s(&[0, -1]));
        assert!(!fine, "a button has no text to type in");

        // The units themselves, at their edges.
        let letters: Vec<char> = "ab  cd\n\nef".chars().collect();
        assert_eq!([stretch(&letters, 0, 1), stretch(&letters, 3, 1), stretch(&letters, 4, 1), stretch(&letters, 10, 1)], [[0, 4], [0, 4], [4, 8], [8, 10]]);
        assert_eq!([stretch(&letters, 0, 2), stretch(&letters, 6, 2), stretch(&letters, 7, 2), stretch(&letters, 99, 2)], [[0, 7], [0, 7], [7, 8], [8, 10]]);
        assert_eq!([stretch(&[], 0, 0), stretch(&[], 5, 1), stretch(&[], -5, 2)], [[0, 0]; 3]);
        assert_eq!([byte_at("a\u{17c}b", 0), byte_at("a\u{17c}b", 2), byte_at("a\u{17c}b", 3), byte_at("a\u{17c}b", 9), byte_at("a\u{17c}b", -1), byte_at("a\u{17c}b", i32::MIN)], [0, 3, 4, 4, 4, 4]);
        assert_eq!([spliced("abc", 1, 2, "X"), spliced("abc", 2, 1, "X"), spliced("abc", -1, -1, "X"), spliced("", 0, 5, "X")], ["aXc", "aXc", "abcX", "X"]);
        let was_now = |was: &str, now: &str| retyped(was, now);
        assert_eq!(was_now("hello", "help"), (3, "lo".into(), "p".into()));
        assert_eq!(was_now("aaa", "aaaa"), (3, String::new(), "a".into()));
        assert_eq!(was_now("abcabc", "abc"), (3, "abc".into(), String::new()));
        assert_eq!(was_now("", ""), (0, String::new(), String::new()));
        assert_eq!(was_now("x\u{17c}y", "x\u{f3}y"), (1, "\u{17c}".into(), "\u{f3}".into()));
    }

    /// Calls of every kind with random values, and malformed calls: each is answered or
    /// ignored, harmlessly.
    #[test]
    fn whatever_is_asked_is_answered_or_passed_over() {
        let mut app = App { volume: 0.5, name: "Ada L".into(), ..Default::default() };
        let mut ui = window(&mut app);
        let (mut readers, mut peer) = told_of(&mut ui);
        let paths: Vec<String> = [ROOT.into(), readers.path(Which::Window(0)), "/".into(), "/org/a11y/atspi/accessible/".into(), "/org/a11y/atspi/accessible/1/4294967295".into()]
            .into_iter()
            .chain((0..8).map(|at| readers.path(Which::Thing(0, at))))
            .collect();
        let interfaces = [ACCESSIBLE, APPLICATION, COMPONENT, ACTION, VALUE, TEXT, EDITABLE, PROPERTIES, PEER, REGISTRY, ""];
        #[rustfmt::skip]
        let members = [
            "Get", "GetAll", "Set", "Ping", "GetMachineId", "GetChildAtIndex", "GetChildren", "GetIndexInParent", "GetRole", "GetRoleName", "GetState", "GetApplication", "GetInterfaces",
            "GetRelationSet", "GetAttributes", "GetLocale", "GetApplicationBusAddress", "GetExtents", "GetPosition", "GetSize", "Contains", "GetAccessibleAtPoint", "GetLayer", "GetMDIZOrder",
            "GetAlpha", "GrabFocus", "ScrollTo", "GetName", "GetActions", "DoAction", "GetText", "GetStringAtOffset", "GetTextAtOffset", "GetTextBeforeOffset", "GetTextAfterOffset",
            "GetCharacterAtOffset", "GetCharacterExtents", "GetRangeExtents", "GetAttributeRun", "GetBoundedRanges", "SetTextContents", "InsertText", "CopyText", "CutText", "DeleteText",
            "PasteText", "EventListenerRegistered", "EventListenerDeregistered", "NameOwnerChanged", "Nonsense", "",
        ];
        // The calls, each with its argument types: a number, a small one, a string, an
        // interface name, a property name, a variant.
        #[rustfmt::skip]
        let calls = [
            (ACCESSIBLE, "GetChildAtIndex", "i"), (ACCESSIBLE, "GetChildren", ""), (ACCESSIBLE, "GetIndexInParent", ""), (ACCESSIBLE, "GetState", ""), (ACCESSIBLE, "GetInterfaces", ""),
            (APPLICATION, "GetLocale", "u"), (COMPONENT, "GetExtents", "u"), (COMPONENT, "GetPosition", "u"), (COMPONENT, "Contains", "iiu"), (COMPONENT, "GetAccessibleAtPoint", "iiu"),
            (COMPONENT, "GrabFocus", ""), (ACTION, "GetName", "i"), (ACTION, "DoAction", "i"), (TEXT, "GetText", "ii"), (TEXT, "GetStringAtOffset", "iu"), (TEXT, "GetTextAtOffset", "iu"),
            (TEXT, "GetTextBeforeOffset", "iu"), (TEXT, "GetTextAfterOffset", "iu"), (TEXT, "GetCharacterAtOffset", "i"), (TEXT, "GetCharacterExtents", "iu"), (TEXT, "GetRangeExtents", "iiu"),
            (EDITABLE, "SetTextContents", "s"), (EDITABLE, "InsertText", "isi"), (EDITABLE, "CopyText", "ii"), (EDITABLE, "CutText", "ii"), (EDITABLE, "DeleteText", "ii"),
            (EDITABLE, "PasteText", "i"), (PROPERTIES, "Get", "IP"), (PROPERTIES, "GetAll", "I"), (PROPERTIES, "Set", "IPv"), (PEER, "Ping", ""),
        ];
        let texts = ["", "a", "\u{17c}\u{f3}\u{142}w \u{1f44d}", "two words\nand a line", "\0"];
        let (mut chance, mut answered, mut asked_to_do) = (Chance(0x9e37_79b9_7f4a_7c15), 0, 0);
        // The same messages go to the program's session bus side, which must also take no harm.
        let (mut session, mut other) = Session::new();
        for round in 0..30_000 {
            let (mut interface, mut member, mut takes) = (interfaces[chance.below(interfaces.len())], members[chance.below(members.len())], "");
            if chance.below(4) > 0 {
                (interface, member, takes) = calls[chance.below(calls.len())];
            }
            let path = &paths[chance.below(paths.len())];
            let mut values = Pack::default();
            for kind in takes.chars() {
                let named = NAMED[chance.below(5)];
                match kind {
                    'i' => values.i32(chance.below(40) as i32 - 6),
                    'u' => values.u32(chance.below(8) as u32),
                    's' => values.text(texts[chance.below(texts.len())]),
                    'I' => values.text(interfaces[chance.below(7)]),
                    'P' => values.text(named.1[chance.below(named.1.len())]),
                    _ => values.kinds(["d", "d", "i", "s"][chance.below(4)]).f64(chance.below(100) as f64 / 50.0 - 0.5),
                };
            }
            // And arguments with no call, or the wrong ones: any values.
            for _ in 0..chance.below(3) * (takes.is_empty() as usize) {
                match chance.below(4) {
                    0 => values.i32(chance.below(40) as i32 - 5),
                    1 => values.text(interfaces[chance.below(interfaces.len())]),
                    2 => values.byte(chance.next() as u8),
                    _ => values.u32(chance.next() as u32),
                };
            }
            let kind = [CALL, CALL, CALL, NEWS, ANSWER, FAILED, 0, 9][chance.below(8)];
            let mut bytes = packed(kind, 1000 + round, &[(1, "o", path), (2, "s", interface), (3, "s", member), (5, "u", &chance.below(6).to_string()), (7, "s", ":1.7")], &values);
            if chance.below(8) == 0 {
                let at = chance.below(bytes.len());
                bytes[at] = chance.next() as u8;
            }
            let mut deeds = Vec::new();
            readers.hear_desk(&bytes, &|_| [3, 4], &mut deeds);
            if let (Some(bus), Some(message)) = (&mut session.bus, Message::read(&bytes)) {
                session.readers.hear_session(bus, &message);
            }
            other.messages();
            asked_to_do += deeds.len();
            deeds.into_iter().for_each(|deed| readers.act(&mut ui, deed));
            // Requested actions are performed and the window reported again: so the tree being
            // queried keeps changing under the queries.
            if round % 64 == 0 {
                run(&mut ui, &mut app);
                readers.keep(&mut ui, 0, "Settings window", [420, 300], 1.0);
                readers.settle();
            }
            for answer in peer.messages() {
                let read = Message::read(&answer).expect("what is sent is laid out as a message");
                answered += (read.kind == ANSWER) as usize;
            }
            assert!(readers.desk.is_some(), "the bus is not let go of over what was asked");
        }
        assert!(answered > 2_000 && asked_to_do > 100, "{answered} answered, {asked_to_do} things asked to be done");
        for _ in 0..5_000 {
            let noise: Vec<u8> = (0..chance.below(120)).map(|_| chance.next() as u8).collect();
            readers.hear_desk(&noise, &|_| [0; 2], &mut Vec::new());
        }
    }

    #[test]
    fn roles_and_states_are_told_by_the_numbers_at_spi_gives_them() {
        let said = |role, on, usable, focused| {
            let (name, rect) = (String::new(), crate::Rect::EMPTY);
            Said { on, focused, usable, ..Said::plain(1, role, name, rect, None) }
        };
        let roles = [
            (Role::Text, None, 29),
            (Role::Button, None, 43),
            (Role::Button, Some(true), 62),
            (Role::Tick, Some(false), 7),
            (Role::Switch, Some(true), 130),
            (Role::Choice, Some(true), 44),
            (Role::Tab, Some(true), 37),
            (Role::Row, Some(false), 32),
            (Role::Slider, None, 51),
            (Role::Spin, None, 52),
            (Role::TextBox, None, 79),
            (Role::Secret, None, 40),
            (Role::Select, Some(false), 11),
            (Role::Fold, Some(true), 43),
            (Role::MenuItem, None, 35),
            (Role::MenuItem, Some(true), 8),
            (Role::Heading, None, 10),
            (Role::Progress, None, 42),
            (Role::Group, None, 39),
            (Role::Dialog, None, 16),
            (Role::Menu, None, 33),
            (Role::MenuBar, None, 34),
            (Role::Tabs, None, 38),
            (Role::Link, None, 88),
            (Role::Document, None, 94),
            (Role::Title, None, 83),
            (Role::Paragraph, None, 73),
            (Role::List, None, 31),
            (Role::Item, None, 32),
            (Role::Code, None, 85),
            (Role::Quote, None, 105),
        ];
        for (role, on, number) in roles {
            assert_eq!(role_of(&said(role, on, true, false)).0, number, "{role:?}");
        }
        let shown = 1u64 << 25 | 1 << 30;
        assert_eq!(states_of(&said(Role::Text, None, true, false), true), shown | 1 << 8 | 1 << 24);
        assert_eq!(states_of(&said(Role::Text, None, false, false), true), shown, "switched off: neither enabled nor sensitive");
        assert_eq!(states_of(&said(Role::Tick, Some(true), true, true), true), shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 12 | 1 << 41 | 1 << 4);
        assert_eq!(
            states_of(&said(Role::Tick, Some(true), true, true), false),
            shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 41 | 1 << 4,
            "in a window that is not typed into, the keyboard is on nothing"
        );
        assert_eq!(states_of(&said(Role::Fold, Some(true), true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 9 | 1 << 10);
        assert_eq!(states_of(&said(Role::Fold, Some(false), true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 9 | 1 << 5);
        assert_eq!(states_of(&said(Role::Button, Some(true), true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 20);
        assert_eq!(states_of(&said(Role::TextBox, None, true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 11 | 1 << 7);
        assert_eq!(states_of(&said(Role::Progress, None, true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 43);
        assert_eq!(states_of(&said(Role::MenuItem, None, true, false), true), shown | 1 << 8 | 1 << 24 | 1 << 11, "one that is not ticked off cannot be");
    }
}
