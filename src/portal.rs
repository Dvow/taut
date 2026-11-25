//! The desktop portal (`xdg-desktop-portal`) on Linux, reached over the session bus, which
//! forwards requests to whatever desktop runs. Two things are asked of it: the user's
//! appearance and motion preferences ([`Preferences`]), read at start and then followed through
//! change signals; and the desktop's own file dialog over a window, which the portal shows from
//! its own process and answers later, so nothing waits and the window keeps running.
//!
//! It is spoken to over the windows' bus connection (`atspi::Bus`), in hand-written D-Bus:
//! nothing is linked and no program is run. Nothing is kept while nothing is asked, and nothing
//! runs until the portal speaks. Without a bus or portal, no preference is known from here
//! (windows use the X settings instead), and the file dialog is the library's own.
//!
//! The `org.freedesktop.appearance` settings arrived one by one: `color-scheme` in 1.12 (2021),
//! `accent-color` in 1.17 (2023), `contrast` in 1.19 (2024), `reduced-motion` in 1.21 (2026). A
//! portal reports only what its desktop knows: an older one, or a desktop without a setting,
//! says nothing of it, which counts as no preference.
use crate::atspi::{ANSWER, BUS, Bus, CALL, FAILED, Header, Message, NEWS, PROPERTIES, Pack, Unpack, Value};
use crate::color::Color;
use crate::file_dialog::FileDialogRequest;
use crate::id::Id;
use crate::preferences::Preferences;
use crate::theme::ContrastColors;
use crate::x11::path_of;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

/// The portal's bus name, its single object path, and its interface. A requested file dialog is
/// an object of its own, a request, whose path begins with `REQUESTS`.
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const CHOOSER: &str = "org.freedesktop.portal.FileChooser";
const REQUESTS: &str = "/org/freedesktop/portal/desktop/request";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// The appearance settings group, and its settings. `color-scheme`: 1 dark, 2 light, 0 no
/// preference. `accent-color`: red, green, blue, each 0 to 1 (anything else is no colour).
/// `contrast` and `reduced-motion`: 1 where wanted, 0 for no preference.
const APPEARANCE: &str = "org.freedesktop.appearance";
const SCHEME: &str = "color-scheme";
const ACCENT: &str = "accent-color";
const CONTRAST: &str = "contrast";
const MOTION: &str = "reduced-motion";
/// How long the portal has to acknowledge a file dialog request. (It does so at once; the
/// dialog then stays up as long as the user likes.)
const PATIENCE: Duration = Duration::from_secs(5);

/// How the portal is asked for its file dialog (`Portal::ask`). Handed over by the first
/// request for the system dialog ([`FileDialog::native`](crate::FileDialog::native), through
/// `platform::hand_file_dialog`); nothing else reaches that code, so a program that never asks
/// is built without it.
type Asks = fn(&mut Portal, Option<&mut Bus>, &str, Id, &FileDialogRequest);

thread_local! {
    /// How the portal is asked for its file dialog: unset until an interface on this thread
    /// asks (see `Asks`).
    static ASKS: std::cell::Cell<Option<Asks>> = const { std::cell::Cell::new(None) };
}

/// Hands the windows their way of asking the portal for its file dialog, on this thread (see
/// `Asks`).
pub(crate) fn hand_file_dialog() {
    ASKS.set(Some(|portal, session, parent, known, ask| portal.ask(session, parent, known, ask)));
}

/// A file dialog the portal was asked to show.
struct Asked {
    /// The window that asked, by the first window's interface's id (0 for the first).
    window: Id,
    /// The serial of the call that asked, until the portal replies: then 0.
    call: u32,
    /// The request object standing for the dialog: its `Response` signal tells how it ended.
    request: String,
    /// When it was asked.
    since: Duration,
}

/// The desktop portal, as far as this program uses it.
pub(crate) struct Portal {
    /// The user's colour scheme preference, as far as the portal said: dark or light. None for
    /// no preference, or unknown.
    pub(crate) dark: Option<bool>,
    /// The user's accent colour, where the portal gave one; and whether they want higher
    /// contrast, and reduced motion (no, while unknown).
    pub(crate) accent: Option<Color>,
    pub(crate) high_contrast: bool,
    pub(crate) reduced_motion: bool,
    /// The portal has a file dialog that can also pick folders.
    pub(crate) files: bool,
    /// Ended file dialogs, until the windows take them: the window that asked, and the picked
    /// file or none. (A request the portal refused ends with none too, and with `files` false:
    /// the library's own is shown.)
    pub(crate) answers: Vec<(Id, Option<PathBuf>)>,
    /// The serials of the questions out, 0 for none: the appearance settings, and the file
    /// chooser's version.
    asked: [u32; 2],
    /// The portal's current unique bus name (like ":1.42"): only messages from it are the
    /// portal's word.
    owner: String,
    /// This program's unique name, as it appears in a request path.
    me: String,
    /// The file dialogs up; and how many were requested so far, which names each request.
    up: Vec<Asked>,
    count: u32,
    /// The time, as the windows set it before each turn.
    pub(crate) now: Duration,
}

impl Portal {
    /// Subscribes over the session bus to preference changes and to the portal appearing or
    /// leaving, and asks the current state. Nothing is waited for: answers come when they come,
    /// and without a bus never.
    pub(crate) fn new(session: Option<&mut Bus>) -> Self {
        let mut portal = Portal {
            dark: None,
            accent: None,
            high_contrast: false,
            reduced_motion: false,
            files: false,
            answers: Vec::new(),
            asked: [0; 2],
            owner: String::new(),
            me: String::new(),
            up: Vec::new(),
            count: 0,
            now: Duration::ZERO,
        };
        let Some(bus) = session else { return portal };
        bus.ask("AddMatch", &format!("type='signal',sender='{PORTAL}',path='{PORTAL_PATH}',interface='{SETTINGS}',member='SettingChanged',arg0='{APPEARANCE}'"));
        bus.ask("AddMatch", &format!("type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{PORTAL}'"));
        portal.ask_how(bus);
        portal
    }

    /// Asks the portal's state: all its appearance settings (in one call every portal version
    /// knows), and its file chooser's version. (A portal not running is activated for that, as
    /// the session sets it up.)
    fn ask_how(&mut self, bus: &mut Bus) {
        self.asked[0] = bus.call(PORTAL, PORTAL_PATH, SETTINGS, "ReadAll", "as", Pack::default().open(4).text(APPEARANCE).close());
        self.asked[1] = bus.call(PORTAL, PORTAL_PATH, PROPERTIES, "Get", "ss", Pack::default().text(CHOOSER).text("version"));
    }

    /// The user's preferences, as far as the portal said; and as the X settings (`desk`) say
    /// where it says nothing. A colour scheme preference beats the desktop's theme; contrast or
    /// reduced motion counts where either says so; text size comes only from X. (The portal
    /// names no high-contrast colours: the library's own pair is used, dark or light as the
    /// user prefers.)
    pub(crate) fn preferences(&self, desk: Preferences) -> Preferences {
        let dark = self.dark.unwrap_or(desk.dark);
        let contrast = (self.high_contrast || desk.high_contrast.is_some()).then_some(if dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
        Preferences { dark, accent: self.accent, high_contrast: contrast, reduced_motion: self.reduced_motion || desk.reduced_motion, text_scale: desk.text_scale }
    }

    /// A message came over the session bus, perhaps for this part: a reply to a question, a bus
    /// signal about the portal, or a portal signal.
    pub(crate) fn hear(&mut self, bus: &mut Bus, message: &Message) {
        let (mut body, from_portal) = (message.body, message.sender == self.owner);
        match (message.kind, message.member) {
            // (Hello was the first call on the bus: its reply is this program's unique name. In
            // a request path it appears without its colon, with underscores for dots.)
            (ANSWER, _) if message.answers == 1 => self.me = body.text().unwrap_or_default().trim_start_matches(':').replace('.', "_"),
            (ANSWER | FAILED, _) if message.answers != 0 => self.answered(message),
            (NEWS, "NameOwnerChanged") if message.sender == BUS => self.owned(bus, body),
            (NEWS, "SettingChanged") if from_portal => self.set(body),
            (NEWS, "Response") if from_portal => self.ended(message.path, body),
            _ => {}
        }
    }

    /// A reply came: to one of the questions if its serial matches, else perhaps to a file
    /// dialog request.
    fn answered(&mut self, message: &Message) {
        let (fine, body) = (message.kind == ANSWER, message.body);
        let Some(which) = self.asked.iter().position(|asked| *asked == message.answers) else { return self.taken(message) };
        self.asked[which] = 0;
        // (A reply from the portal itself, not the bus on its behalf, comes from its current
        // owner.)
        if message.sender != BUS {
            self.owner = message.sender.into();
        }
        match which {
            // (An error is an answer too: the portal has no settings, or there is no portal. No
            // preference is known then.)
            0 => self.read_all(fine.then_some(body)),
            // (A file chooser that can pick folders is version 3, of 2020.)
            _ => self.files = fine && number_in(body).is_some_and(|version| version >= 3),
        }
    }

    /// The portal replied with its appearance settings, or did not (none): a dict of groups,
    /// each a dict of settings and values. Only the one group was asked for; an unlisted
    /// setting is no preference.
    fn read_all(&mut self, body: Option<Unpack>) {
        (self.dark, self.accent, self.high_contrast, self.reduced_motion) = (None, None, false, false);
        let Some(mut body) = body else { return };
        let Some(end) = body.list(8) else { return };
        while body.at < end && self.read_group(&mut body).is_some() {}
    }

    /// One settings group, read from `body`: its name, then its settings, each taken in if the
    /// group is the appearance one. Nothing where the bytes are not laid out so.
    fn read_group(&mut self, body: &mut Unpack) -> Option<()> {
        let (group, end) = (body.take(8, 0).and(body.text())?, body.list(8)?);
        while body.at < end {
            let name = body.take(8, 0).and(body.text())?;
            if group == APPEARANCE {
                self.heard(name, *body);
            }
            body.skip("v")?;
        }
        Some(())
    }

    /// The portal replied to a file dialog request, if this is that reply: it took it, naming
    /// the request (as told, but its word goes); or it refused, and there is no dialog.
    fn taken(&mut self, message: &Message) {
        let Some(at) = self.up.iter().position(|asked| asked.call == message.answers) else { return };
        let (mut body, asked) = (message.body, self.up.remove(at));
        match body.text().filter(|_| message.kind == ANSWER) {
            Some(request) => self.up.insert(at, Asked { call: 0, request: request.into(), ..asked }),
            None => self.refused(asked.window),
        }
    }

    /// The bus says a name changed owner. If it is the portal's: a departed portal has no
    /// dialog to show and none up; an arrived one is asked its state. (Unless that question is
    /// already out: asking was what activated it.)
    fn owned(&mut self, bus: &mut Bus, mut body: Unpack) {
        let (Some(PORTAL), Some(_), Some(now)) = (body.text(), body.text(), body.text()) else { return };
        self.owner = now.into();
        if now.is_empty() {
            self.gone();
        } else if self.asked == [0; 2] {
            self.ask_how(bus);
        }
    }

    /// The portal says a setting changed: its group, name and value.
    fn set(&mut self, mut body: Unpack) {
        if let (Some(APPEARANCE), Some(name)) = (body.text(), body.text()) {
            self.heard(name, body);
        }
    }

    /// An appearance setting by name; its value follows in `value`. A value of the wrong type
    /// is no preference.
    fn heard(&mut self, name: &str, value: Unpack) {
        match name {
            SCHEME => self.dark = dark_in(value),
            ACCENT => self.accent = color_in(value),
            CONTRAST => self.high_contrast = number_in(value) == Some(1),
            MOTION => self.reduced_motion = number_in(value) == Some(1),
            _ => {}
        }
    }

    /// The portal says how a request ended. If it is one of this program's file dialogs: a pick
    /// (0, with what was picked), or no file. The user cancelled by its button (1), or closed
    /// it another way, by a key or its window's close button (2). (A portal whose dialog could
    /// not show at all says 2 as well. The two cannot be told apart, and a closed dialog is
    /// what happens: so it counts as that.)
    fn ended(&mut self, request: &str, mut body: Unpack) {
        let Some(at) = self.up.iter().position(|asked| asked.request == request) else { return };
        let file = if body.u32() == Some(0) { chosen_file(body) } else { None };
        self.answers.push((self.up.remove(at).window, file));
    }

    /// The portal refused the file dialog request, or is not there: the window has nothing to
    /// wait for, and gets the library's own dialog. A portal that failed once is not asked
    /// again until it reappears.
    fn refused(&mut self, window: Id) {
        self.files = false;
        self.answers.push((window, None));
    }

    /// The portal, or the bus to it, is gone: no desktop file dialog can show, and those up
    /// will get no answer.
    pub(crate) fn gone(&mut self) {
        self.files = false;
        for asked in std::mem::take(&mut self.up) {
            self.answers.push((asked.window, None));
        }
    }

    /// Asks the portal for its file dialog over a window, as `ask` does, once an app on this
    /// thread asked for the system dialog: that request hands over how (see `Asks`). (Nothing
    /// else asks for the dialog; should anything, it is answered at once with no file.)
    pub(crate) fn ask_for_files(&mut self, session: Option<&mut Bus>, parent: &str, known: Id, ask: &FileDialogRequest) {
        match ASKS.get() {
            Some(asks) => asks(self, session, parent, known, ask),
            None => self.answers.push((known, None)),
        }
    }

    /// Asks the portal for its file dialog over a window: `parent` is the window's portal
    /// handle ("x11:" and its hex id, "wayland:" and an exported handle, or empty), `known` its
    /// id in the first window's interface. The portal is told the request's handle token, so
    /// where the answer comes from is known before it replies. With no portal (any more), the
    /// request is refused at once.
    pub(crate) fn ask(&mut self, session: Option<&mut Bus>, parent: &str, known: Id, ask: &FileDialogRequest) {
        let Some(bus) = session.filter(|_| self.files) else { return self.refused(known) };
        self.count += 1;
        let token = format!("taut{}", self.count);
        // The parent window, the title, then the options, each a name and a variant.
        let mut asks = Pack::default();
        asks.text(parent).text(ask.title).open(8);
        asks.pad(8).text("handle_token").kinds("s").text(&token);
        // (The starting folder: its path's bytes, NUL-terminated.)
        asks.pad(8).text("current_folder").kinds("ay").open(1).bytes.extend_from_slice(ask.folder.as_os_str().as_bytes());
        asks.byte(0).close();
        if ask.folders {
            asks.pad(8).text("directory").kinds("b").u32(1);
        }
        if ask.saving && !ask.name.is_empty() {
            asks.pad(8).text("current_name").kinds("s").text(&ask.name);
        }
        if !ask.endings.is_empty() && !ask.folders {
            list_only(&mut asks, &ask.endings);
        }
        let call = bus.call(PORTAL, PORTAL_PATH, CHOOSER, if ask.saving { "SaveFile" } else { "OpenFile" }, "ssa{sv}", asks.close());
        self.up.push(Asked { window: known, call, request: format!("{REQUESTS}/{}/{token}", self.me), since: self.now });
    }

    /// When the first request the portal has not replied to is given up on, if any: for whoever
    /// sleeps meanwhile.
    pub(crate) fn patience_ends(&self) -> Option<Duration> {
        self.up.iter().filter(|asked| asked.call != 0).map(|asked| asked.since + PATIENCE).min()
    }

    /// Gives up on every file dialog request the portal left unanswered for its whole time, and
    /// says whether there was one. Its window is answered with no file and gets the library's
    /// own dialog, so no app waits for a dialog that never comes; the portal is told to close
    /// it, should it show after all; and a portal silent once is not asked again until it
    /// reappears. (What the portal took stays up as long as the user likes.)
    pub(crate) fn give_up(&mut self, mut session: Option<&mut Bus>) -> bool {
        let now = self.now;
        let late = |asked: &Asked| asked.call != 0 && now >= asked.since + PATIENCE;
        let (given_up, up): (Vec<Asked>, Vec<Asked>) = std::mem::take(&mut self.up).into_iter().partition(late);
        self.up = up;
        for asked in &given_up {
            if let Some(bus) = session.as_deref_mut() {
                bus.send(Header { kind: CALL, flags: 1, to: PORTAL, path: &asked.request, interface: REQUEST, member: "Close", ..Default::default() }, &[]);
            }
            self.refused(asked.window);
        }
        !given_up.is_empty()
    }

    /// A window is gone: its file dialogs are closed, and nothing more is heard of them. (No
    /// reply is awaited: the 1 flag.)
    pub(crate) fn forget(&mut self, session: Option<&mut Bus>, window: Id) {
        let theirs = self.up.iter().filter(|asked| asked.window == window);
        if let Some(bus) = session {
            theirs.for_each(|asked| _ = bus.send(Header { kind: CALL, flags: 1, to: PORTAL, path: &asked.request, interface: REQUEST, member: "Close", ..Default::default() }, &[]));
        }
        self.up.retain(|asked| asked.window != window);
    }
}

/// Adds a filter listing only files ending in one of `endings`: one filter named by its
/// patterns, a glob per ending (the 0 before each marks a glob).
fn list_only(asks: &mut Pack, endings: &[String]) {
    let mut called = String::new();
    for ending in endings {
        called.push_str(if called.is_empty() { "*" } else { " *" });
        called.push_str(ending);
    }
    asks.pad(8).text("filters").kinds("a(sa(us))").open(8).pad(8).text(&called).open(8);
    for ending in endings {
        asks.pad(8).u32(0).text(&pattern(ending));
    }
    asks.close().close();
}

/// An ending as a case-insensitive glob. The portal's globs are case-sensitive, so each letter
/// becomes a bracket of both cases: ".png" is "*.[pP][nN][gG]". (A glob metacharacter goes in
/// brackets too, to stand for itself. A non-ASCII letter stays as written: endings are plain,
/// and the full case table would come into the program for that one case.)
fn pattern(ending: &str) -> String {
    let mut pattern = vec![b'*'];
    for letter in ending.bytes() {
        match letter {
            b'a'..=b'z' | b'A'..=b'Z' => pattern.extend_from_slice(&[b'[', letter.to_ascii_lowercase(), letter.to_ascii_uppercase(), b']']),
            b'*' | b'?' | b'[' => pattern.extend_from_slice(&[b'[', letter, b']']),
            _ => pattern.push(letter),
        }
    }
    // (Only ASCII was changed: it is still valid UTF-8.)
    String::from_utf8(pattern).unwrap_or_default()
}

/// The colour scheme preference from the portal's setting: dark for 1, light for 2, none for
/// anything else.
fn dark_in(value: Unpack) -> Option<bool> {
    match number_in(value) {
        Some(1) => Some(true),
        Some(2) => Some(false),
        _ => None,
    }
}

/// The number in a variant, as the portal hands a setting.
fn number_in(value: Unpack) -> Option<u32> {
    match unwrapped(value).value()? {
        Value::Number(number) => Some(number as u32),
        Value::Text(_) => None,
    }
}

/// The accent colour from the portal's setting: red, green, blue, each 0 to 1. Nothing for
/// another type, or a component out of range (which the portal's spec says means no colour).
fn color_in(value: Unpack) -> Option<Color> {
    let mut value = unwrapped(value);
    value.kinds().filter(|kinds| *kinds == "(ddd)").and(value.take(8, 0))?;
    let mut part = || value.f64().filter(|part| (0.0..=1.0).contains(part)).map(|part| (part * 255.0).round() as u32);
    let (red, green, blue) = (part()?, part()?, part()?);
    Some(Color::rgb(red << 16 | green << 8 | blue))
}

/// A variant with any wrapping variants removed: the older `Read` call wraps its value in a
/// second one, and however many there are, they come off.
fn unwrapped(mut value: Unpack) -> Unpack {
    let mut inner = value;
    while inner.kinds() == Some("v") {
        value = inner;
    }
    value
}

/// The file a file dialog answered with: of the results dict, `uris` lists the picked
/// addresses, and the first is the one. Nothing if it is no local file.
fn chosen_file(mut body: Unpack) -> Option<PathBuf> {
    let end = body.list(8)?;
    while body.at < end {
        let (name, kinds) = (body.take(8, 0).and(body.text())?, body.kinds()?);
        if (name, kinds) == ("uris", "as") {
            let last = body.list(4)?;
            return path_of(body.text().filter(|_| body.at <= last)?.as_bytes());
        }
        body.skip(kinds)?;
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::atspi::BUS_PATH;
    use crate::atspi::tests::{Peer, packed};
    use std::ffi::c_ulong;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    /// The portal's unique name in the tests that play both ends over a socket; and a program
    /// on the bus that is neither.
    const THEM: &str = ":1.0";
    const STRANGER: &str = ":1.66";
    /// What a portal says of a method it lacks.
    const NO_SUCH_CALL: &str = "org.freedesktop.DBus.Error.UnknownMethod";

    /// A program's session bus connection with the portal part on it; and the socket's other
    /// end, which plays bus and portal.
    struct Side {
        bus: Bus,
        portal: Portal,
        peer: Peer,
    }

    impl Side {
        /// On the bus as a program is at start; and what it had asked by then.
        fn new() -> (Self, Vec<(String, Vec<u8>)>) {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let mut bus = Bus::over(ours).unwrap();
            let (mut portal, mut peer) = (Portal::new(Some(&mut bus)), Peer::new(theirs));
            let asked = peer.let_on(&mut || bus.turn(&mut |bus, message| portal.hear(bus, message)));
            (Side { bus, portal, peer }, asked)
        }

        /// The same, once a portal replied: no colour scheme preference, file chooser version
        /// 4.
        fn with_a_portal() -> Self {
            let (mut side, asked) = Side::new();
            side.answers(THEM, serial(&asked, "ReadAll"), "a{sa{sv}}", &appearance(&Stand { scheme: Some(0), ..Stand::default() }));
            side.answers(THEM, serial(&asked, "Get"), "v", Pack::default().kinds("u").u32(4));
            assert!(side.portal.files && side.portal.dark.is_none());
            side
        }

        /// Has the program take in what came, and returns what it asked in turn.
        fn turn(&mut self) -> Vec<(String, Vec<u8>)> {
            let Side { bus, portal, peer } = self;
            bus.turn(&mut |bus, message| portal.hear(bus, message));
            peer.said()
        }

        /// `from` replies to the call with serial `to`.
        fn answers(&mut self, from: &str, to: u32, kinds: &str, values: &Pack) -> Vec<(String, Vec<u8>)> {
            self.peer.send(&packed(ANSWER, 600, &[(5, "u", &to.to_string()), (7, "s", from), (8, "g", kinds)], values));
            self.turn()
        }

        /// `from` says the call with serial `to` failed, and how.
        fn fails(&mut self, from: &str, to: u32, failure: &str) -> Vec<(String, Vec<u8>)> {
            self.peer.send(&packed(FAILED, 601, &[(4, "s", failure), (5, "u", &to.to_string()), (7, "s", from), (8, "g", "s")], Pack::default().text("why")));
            self.turn()
        }

        /// A signal from `from`, of the object at `path`: a name changed owner, a request
        /// ended, or a setting changed.
        fn news(&mut self, from: &str, path: &str, member: &str, kinds: &str, values: &Pack) -> Vec<(String, Vec<u8>)> {
            let interface = match member {
                "NameOwnerChanged" => BUS,
                "Response" => REQUEST,
                _ => SETTINGS,
            };
            self.peer.send(&packed(NEWS, 602, &[(1, "o", path), (2, "s", interface), (3, "s", member), (7, "s", from), (8, "g", kinds)], values));
            self.turn()
        }

        /// Asks for a file dialog over X window `window`, for interface window `known`; and
        /// returns the call sent to the portal.
        fn asks(&mut self, window: c_ulong, known: Id, ask: &FileDialogRequest) -> Vec<(String, Vec<u8>)> {
            self.portal.ask(Some(&mut self.bus), &format!("x11:{window:x}"), known, ask);
            self.peer.said()
        }

        /// The answers the windows would take, taken.
        fn answered(&mut self) -> Vec<(Id, Option<PathBuf>)> {
            std::mem::take(&mut self.portal.answers)
        }
    }

    /// The serial of the call named `member` among what a program asked; and the names of all
    /// it asked.
    fn serial(asked: &[(String, Vec<u8>)], member: &str) -> u32 {
        let (_, bytes) = asked.iter().find(|(name, _)| name == member).unwrap_or_else(|| panic!("{member} was not asked"));
        Message::read(bytes).unwrap().serial
    }
    fn names(asked: &[(String, Vec<u8>)]) -> Vec<&str> {
        asked.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// A setting and its value, as the portal signals a change.
    fn setting(group: &str, name: &str, value: u32) -> Pack {
        let mut said = Pack::default();
        said.text(group).text(name).kinds("u").u32(value);
        said
    }

    /// A file to open, asked for as an app does.
    fn ask() -> FileDialogRequest {
        FileDialogRequest {
            saving: false,
            folders: false,
            folder: "/home/someone/Pictures of mine".into(),
            name: String::new(),
            endings: vec![".png".into(), ".jpeg".into()],
            title: "Open a file",
            many: false,
        }
    }

    /// What a file dialog request says: the window, the title, then each option as
    /// `name=value`.
    pub(crate) fn asking(mut said: Unpack) -> Vec<String> {
        let mut lines = vec![said.text().unwrap().to_string(), said.text().unwrap().to_string()];
        let end = said.list(8).unwrap();
        while said.at < end {
            let (name, kinds) = (said.take(8, 0).and(said.text()).unwrap(), said.kinds().unwrap());
            let value = match kinds {
                "s" => said.text().unwrap().to_string(),
                "b" => said.u32().unwrap().to_string(),
                "ay" => {
                    let last = said.list(1).unwrap();
                    String::from_utf8_lossy(said.take(1, last - said.at).unwrap()).into_owned()
                }
                "a(sa(us))" => choices(&mut said),
                other => panic!("{name} is of a kind no file dialog is asked for: {other}"),
            };
            lines.push(format!("{name}={value}"));
        }
        lines
    }

    /// The filters a dialog is asked to offer: each as its name, a colon, and its globs
    /// comma-separated.
    fn choices(said: &mut Unpack) -> String {
        let (end, mut choices) = (said.list(8).unwrap(), Vec::new());
        while said.at < end {
            let (called, last, mut patterns) = (said.take(8, 0).and(said.text()).unwrap(), said.list(8).unwrap(), Vec::new());
            while said.at < last {
                assert_eq!(said.take(8, 0).and(said.u32()), Some(0), "a pattern for names");
                patterns.push(said.text().unwrap());
            }
            choices.push(format!("{called}:{}", patterns.join(",")));
        }
        choices.join(";")
    }

    /// How a file dialog ended, as the portal says it: 0 with the picked addresses, 1 for
    /// cancelled, 2 for another ending. (Other results of a pick come before the addresses, as
    /// the real portal has them: the chosen filter, and the filters listed.)
    pub(crate) fn ended(how: u32, addresses: &[&str]) -> Pack {
        let mut said = Pack::default();
        said.u32(how).open(8);
        if how == 0 {
            said.pad(8).text("choices").kinds("a(ss)").open(8).pad(8).text("encoding").text("utf8").close();
            said.pad(8).text("current_filter").kinds("(sa(us))").pad(8).text("Pictures").open(8).pad(8).u32(0).text("*.png").close();
            said.pad(8).text("uris").kinds("as").open(4);
            addresses.iter().for_each(|address| _ = said.text(address));
            said.close();
        }
        said.close();
        said
    }

    /// The file an ending names, as the portal part reads it.
    fn file_in(ending: &[u8]) -> Option<PathBuf> {
        let mut said = Message::read(ending)?.body;
        said.u32()?;
        chosen_file(said)
    }

    #[test]
    fn the_portal_is_asked_as_a_program_starts_and_tells_of_changes() {
        let (mut side, asked) = Side::new();
        // Subscriptions to settings changes and to the portal appearing or leaving. Then the
        // two questions, which activate a portal not running (no 2 in the third byte).
        let texts = |bytes: &Vec<u8>| {
            let mut said = Message::read(bytes).unwrap().body;
            [said.text(), said.text()].map(|text| text.unwrap_or_default().to_string())
        };
        assert_eq!(names(&asked), ["AddMatch", "AddMatch", "ReadAll", "Get"]);
        let settings = "type='signal',sender='org.freedesktop.portal.Desktop',path='/org/freedesktop/portal/desktop',interface='org.freedesktop.portal.Settings'";
        assert_eq!(texts(&asked[0].1)[0], format!("{settings},member='SettingChanged',arg0='org.freedesktop.appearance'"));
        assert_eq!(texts(&asked[1].1)[0], "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.portal.Desktop'");
        let read = |at: usize| Message::read(&asked[at].1).map(|call| (call.path.to_string(), call.interface.to_string(), texts(&asked[at].1).join(" "), asked[at].1[2])).unwrap();
        assert_eq!(read(3), (PORTAL_PATH.into(), PROPERTIES.into(), "org.freedesktop.portal.FileChooser version".into(), 0));
        // (All of the one group's settings, asked as a list of wanted namespaces.)
        let call = Message::read(&asked[2].1).unwrap();
        let mut all = call.body;
        let end = all.list(4);
        assert_eq!((call.path, call.interface, all.text(), end == Some(all.at), asked[2].1[2]), (PORTAL_PATH, SETTINGS, Some(APPEARANCE), true, 0));
        assert!(side.portal.dark.is_none() && !side.portal.files, "until the portal has said otherwise");

        // The user wants dark windows and reduced motion, and picked a colour; the file chooser
        // can pick folders. Nothing more is asked.
        let how = Stand { scheme: Some(1), accent: Some([0.0, 120.0 / 255.0, 212.0 / 255.0]), motion: Some(1), ..Stand::default() };
        assert!(side.answers(THEM, serial(&asked, "ReadAll"), "a{sa{sv}}", &appearance(&how)).is_empty() && !side.portal.files);
        assert_eq!((side.portal.dark, side.portal.accent, side.portal.high_contrast, side.portal.reduced_motion), (Some(true), Some(Color::rgb(0x0078d4)), false, true));
        assert!(side.answers(THEM, serial(&asked, "Get"), "v", Pack::default().kinds("u").u32(3)).is_empty() && side.portal.files);
        assert_eq!((side.portal.asked, side.portal.owner.as_str(), side.portal.me.as_str()), ([0; 2], THEM, "1_42"));

        // The user changes their mind, and the portal signals it: light, dark, no preference.
        let mut changed = |from: &str, group: &str, name: &str, value: u32| {
            assert!(side.news(from, PORTAL_PATH, "SettingChanged", "ssv", &setting(group, name, value)).is_empty());
            side.portal.dark
        };
        assert_eq!([2, 1, 0, 1].map(|scheme| changed(THEM, APPEARANCE, SCHEME, scheme)), [Some(false), Some(true), None, Some(true)]);
        // Another program's word is not the portal's, and another setting is not this one.
        assert_eq!([changed(STRANGER, APPEARANCE, SCHEME, 2), changed(THEM, APPEARANCE, CONTRAST, 2), changed(THEM, "org.example", SCHEME, 2)], [Some(true); 3]);
        // (A non-number says nothing of the colour scheme.)
        assert!(side.news(THEM, PORTAL_PATH, "SettingChanged", "ssv", Pack::default().text(APPEARANCE).text(SCHEME).kinds("s").text("dark")).is_empty() && side.portal.dark.is_none());

        // Contrast and reduced motion, on and off; another colour, and an invalid one (a
        // component out of range).
        let mut told = |name: &str, how: &Stand| {
            let mut said = Pack::default();
            said.text(APPEARANCE).text(name);
            assert!(value_of(&mut said, name, how));
            side.news(THEM, PORTAL_PATH, "SettingChanged", "ssv", &said);
            (side.portal.high_contrast, side.portal.reduced_motion, side.portal.accent)
        };
        assert_eq!(told(CONTRAST, &Stand { contrast: Some(1), ..Stand::default() }), (true, true, Some(Color::rgb(0x0078d4))));
        assert_eq!(told(MOTION, &Stand { motion: Some(0), ..Stand::default() }), (true, false, Some(Color::rgb(0x0078d4))));
        assert_eq!(told(ACCENT, &Stand { accent: Some([1.0, 0.5, 0.0]), ..Stand::default() }), (true, false, Some(Color::rgb(0xff8000))));
        assert_eq!(told(ACCENT, &Stand { accent: Some([1.0, -0.5, 0.0]), ..Stand::default() }), (true, false, None));
        assert_eq!(told(CONTRAST, &Stand { contrast: Some(0), ..Stand::default() }), (false, false, None));

        // A file chooser before 2020 cannot pick folders: the library's own is shown.
        for (version, usable) in [(1, false), (2, false), (3, true), (9, true)] {
            let (mut side, asked) = Side::new();
            side.answers(THEM, serial(&asked, "Get"), "v", Pack::default().kinds("u").u32(version));
            assert_eq!(side.portal.files, usable, "version {version}");
        }
    }

    #[test]
    fn what_the_portal_says_of_its_settings_comes_together_with_the_desktops() {
        // A portal without settings, or none at all: no preference is known, and the X settings
        // go.
        let desk = Preferences { dark: true, high_contrast: Some(ContrastColors::DARK), reduced_motion: true, text_scale: 1.5, ..Preferences::default() };
        for (failure, from) in [("org.freedesktop.portal.Error.NotFound", THEM), (NO_SUCH_CALL, THEM), ("org.freedesktop.DBus.Error.ServiceUnknown", BUS)] {
            let (mut side, asked) = Side::new();
            side.portal.dark = Some(false);
            assert!(side.fails(from, serial(&asked, "ReadAll"), failure).is_empty());
            assert_eq!((side.portal.preferences(Preferences::default()), side.portal.preferences(desk)), (Preferences::default(), desk), "{failure}");
        }
        // One knowing some of them, among other groups and settings of every type: what it does
        // not say is no preference.
        let (mut side, asked) = Side::new();
        let mut all = Pack::default();
        all.open(8).pad(8).text("org.gnome.desktop.interface").open(8).pad(8).text(CONTRAST).kinds("u").u32(1).pad(8).text("font-name").kinds("s").text("Cantarell 11").close();
        all.pad(8).text(APPEARANCE).open(8).pad(8).text("future-setting").kinds("a{sv}").open(8).close().pad(8).text(SCHEME).kinds("u").u32(2);
        all.pad(8).text(MOTION).kinds("v").kinds("u").u32(1).close().close();
        side.answers(THEM, serial(&asked, "ReadAll"), "a{sa{sv}}", &all);
        assert_eq!((side.portal.dark, side.portal.accent, side.portal.high_contrast, side.portal.reduced_motion), (Some(false), None, false, true));
        // The colour scheme preference beats the desktop's theme; contrast or reduced motion
        // counts where either says so, in the library's colours, dark or light as windows are;
        // text size is the desktop's.
        let joined = side.portal.preferences(desk);
        assert_eq!(joined, Preferences { dark: false, accent: None, high_contrast: Some(ContrastColors::LIGHT), reduced_motion: true, text_scale: 1.5 });
        side.portal.high_contrast = true;
        assert_eq!(side.portal.preferences(Preferences::default()).high_contrast, Some(ContrastColors::LIGHT));
        side.portal.dark = Some(true);
        assert_eq!(side.portal.preferences(Preferences::default()).high_contrast, Some(ContrastColors::DARK));
        // An accent colour comes only from the portal.
        side.portal.accent = Some(Color::rgb(0x3584e4));
        assert_eq!(side.portal.preferences(desk).accent, Some(Color::rgb(0x3584e4)));
    }

    #[test]
    fn a_portal_that_comes_later_is_asked_then_and_one_that_goes_has_no_dialog() {
        let (mut side, asked) = Side::new();
        // No portal: the bus itself says so, to both questions.
        for member in ["ReadAll", "Get"] {
            assert!(side.fails(BUS, serial(&asked, member), "org.freedesktop.DBus.Error.ServiceUnknown").is_empty());
        }
        assert!(side.portal.dark.is_none() && !side.portal.files && side.portal.owner.is_empty());
        // (A file dialog asked anyway is refused at once, and nothing is sent.)
        assert!(side.asks(9, 5, &ask()).is_empty());
        assert_eq!(side.answered(), [(5, None)]);

        let owned = |side: &mut Side, from: &str, name: &str, now: &str| side.news(from, BUS_PATH, "NameOwnerChanged", "sss", Pack::default().text(name).text("").text(now));
        // Another name gets an owner; and a program other than the bus says the portal arrived:
        // neither is news of the portal.
        assert!(owned(&mut side, BUS, "org.example.Other", ":1.8").is_empty() && owned(&mut side, STRANGER, PORTAL, ":1.8").is_empty());
        // The portal arrives: it is asked its state. Told again while that is out (arrival is
        // signalled also when the question activated it), nothing is asked twice.
        let asked = owned(&mut side, BUS, PORTAL, ":1.8");
        assert_eq!((names(&asked), owned(&mut side, BUS, PORTAL, ":1.8").len()), (vec!["ReadAll", "Get"], 0));
        side.answers(":1.8", serial(&asked, "ReadAll"), "a{sa{sv}}", &appearance(&Stand { scheme: Some(1), ..Stand::default() }));
        side.answers(":1.8", serial(&asked, "Get"), "v", Pack::default().kinds("u").u32(4));
        assert!(side.portal.dark == Some(true) && side.portal.files);
        // The former portal's word is nobody's now.
        side.news(THEM, PORTAL_PATH, "SettingChanged", "ssv", &setting(APPEARANCE, SCHEME, 2));
        assert_eq!(side.portal.dark, Some(true));

        // The portal leaves while a dialog is up: no answer will come, the window gets the
        // library's own, and no further dialog is asked of an absent portal. (The colour scheme
        // preference stands.)
        let call = side.asks(9, 5, &ask());
        side.answers(":1.8", serial(&call, "OpenFile"), "o", Pack::default().text("/org/freedesktop/portal/desktop/request/1_42/taut1"));
        assert!(side.news(BUS, BUS_PATH, "NameOwnerChanged", "sss", Pack::default().text(PORTAL).text(":1.8").text("")).is_empty());
        assert_eq!((side.answered(), side.portal.files, side.portal.dark, side.portal.up.len()), (vec![(5, None)], false, Some(true), 0));
        // It reappears: asked again, and it has a dialog again.
        let asked = owned(&mut side, BUS, PORTAL, ":1.9");
        side.answers(":1.9", serial(&asked, "Get"), "v", Pack::default().kinds("u").u32(4));
        assert!(side.portal.files);
    }

    #[test]
    fn a_file_dialog_is_asked_for_and_answered_when_the_person_has() {
        let mut side = Side::with_a_portal();
        let said = side.asks(0x2a0_0007, 7, &ask());
        let call = Message::read(&said[0].1).unwrap();
        assert_eq!((names(&said), call.path, call.interface), (vec!["OpenFile"], PORTAL_PATH, CHOOSER));
        let folder = "current_folder=/home/someone/Pictures of mine\0";
        assert_eq!(asking(call.body), ["x11:2a00007", "Open a file", "handle_token=taut1", folder, "filters=*.png *.jpeg:*.[pP][nN][gG],*.[jJ][pP][eE][gG]"]);
        // Where the answer comes from is known before the portal replies.
        let request = |number: u32| format!("/org/freedesktop/portal/desktop/request/1_42/taut{number}");
        assert_eq!(side.portal.up[0].request, request(1));
        // The portal takes the request. Later the user picks a file whose name has a space, a
        // percent sign and a two-byte letter: the first counts.
        assert!(side.answers(THEM, call.serial, "o", Pack::default().text(&request(1))).is_empty() && side.answered().is_empty());
        side.news(THEM, &request(1), "Response", "ua{sv}", &ended(0, &["file:///home/someone/Pictures%20of%20mine/100%25%20gr%C3%BCn.png", "file:///second"]));
        assert_eq!(side.answered(), [(7, Some(PathBuf::from("/home/someone/Pictures of mine/100% gr\u{fc}n.png")))]);
        assert!(side.portal.up.is_empty() && side.portal.files);

        // The user closes the next: no file, and the portal is asked again next time.
        let said = side.asks(0x2a0_0007, 7, &ask());
        assert_eq!(asking(Message::read(&said[0].1).unwrap().body)[2], "handle_token=taut2");
        side.news(THEM, &request(2), "Response", "ua{sv}", &ended(1, &[]));
        assert_eq!((side.answered(), side.portal.files), (vec![(7, None)], true));

        // Another program's word on the request is not the portal's, and the portal's word on
        // another request is not about this dialog: it is still up.
        let said = side.asks(0x2a0_0007, 7, &ask());
        side.news(STRANGER, &request(3), "Response", "ua{sv}", &ended(0, &["file:///not/this"]));
        side.news(THEM, &request(99), "Response", "ua{sv}", &ended(0, &["file:///nor/this"]));
        assert!(side.answered().is_empty() && side.portal.up.len() == 1);
        // A portal may name the request differently than told: its word goes.
        side.answers(THEM, serial(&said, "OpenFile"), "o", Pack::default().text("/org/freedesktop/portal/desktop/request/of/its/own"));
        side.news(THEM, &request(3), "Response", "ua{sv}", &ended(0, &["file:///not/this"]));
        side.news(THEM, "/org/freedesktop/portal/desktop/request/of/its/own", "Response", "ua{sv}", &ended(0, &["file:///this%20one"]));
        assert_eq!(side.answered(), [(7, Some(PathBuf::from("/this one")))]);

        // A dialog ending "another way" was closed by a key or its window's close button (so
        // the real portal reports it): no file, and that is all.
        side.asks(1, 0, &ask());
        side.news(THEM, &request(4), "Response", "ua{sv}", &ended(2, &[]));
        assert_eq!((side.answered(), side.portal.files, side.portal.up.len()), (vec![(0, None)], true, 0));
    }

    #[test]
    fn a_dialog_is_asked_to_save_or_for_a_folder() {
        let mut side = Side::with_a_portal();
        let mut asked_as = |ask: &FileDialogRequest| {
            let said = side.asks(0x40, 0, ask);
            let call = Message::read(&said[0].1).unwrap();
            (call.member.to_string(), asking(call.body)[3..].join(" | "))
        };
        // Where to save, with a starting name; and without.
        let save = FileDialogRequest { saving: true, name: "notes of mine.txt".into(), endings: vec![".txt".into()], title: "Save as", ..ask() };
        let folder = "current_folder=/home/someone/Pictures of mine\0";
        assert_eq!(asked_as(&save), ("SaveFile".into(), format!("{folder} | current_name=notes of mine.txt | filters=*.txt:*.[tT][xX][tT]")));
        assert_eq!(asked_as(&FileDialogRequest { name: String::new(), endings: Vec::new(), ..save }), ("SaveFile".into(), folder.into()));
        // A folder: filters mean nothing there. And a starting name means nothing when opening.
        assert_eq!(asked_as(&FileDialogRequest { folders: true, title: "Pick a folder", ..ask() }), ("OpenFile".into(), format!("{folder} | directory=1")));
        assert_eq!(asked_as(&FileDialogRequest { name: "report.txt".into(), endings: Vec::new(), ..ask() }), ("OpenFile".into(), folder.into()));
    }

    /// The portal is asked for its dialog in the way the first system dialog request hands over
    /// (`FileDialog::native`): before that, nothing is sent and the request is refused at once,
    /// as with no portal; after, the portal is asked.
    #[test]
    fn the_portal_is_asked_for_a_dialog_once_an_app_has_asked_for_one() {
        let mut side = Side::with_a_portal();
        assert!(ASKS.get().is_none(), "nothing has asked on this thread");
        side.portal.ask_for_files(Some(&mut side.bus), "x11:1", 7, &ask());
        assert!(side.peer.said().is_empty() && side.portal.up.is_empty());
        assert_eq!(side.answered(), [(7, None)]);
        crate::platform::hand_file_dialog();
        side.portal.ask_for_files(Some(&mut side.bus), "x11:1", 8, &ask());
        assert_eq!((names(&side.peer.said()), side.portal.up.len(), side.answered()), (vec!["OpenFile"], 1, vec![]));
    }

    #[test]
    fn a_dialog_the_portal_cannot_show_is_answered_at_once() {
        // The portal refuses: no file, and the library's own dialog from then on, with nothing
        // more asked of the portal.
        let mut side = Side::with_a_portal();
        let said = side.asks(1, 7, &ask());
        side.fails(THEM, serial(&said, "OpenFile"), NO_SUCH_CALL);
        assert_eq!((side.answered(), side.portal.files, side.portal.up.len()), (vec![(7, None)], false, 0));
        assert!(side.asks(1, 8, &ask()).is_empty());
        assert_eq!(side.answered(), [(8, None)]);
        // The bus is gone by the time a window asks.
        let mut side = Side::with_a_portal();
        side.portal.ask(None, "x11:1", 3, &ask());
        assert_eq!((side.answered(), side.portal.files), (vec![(3, None)], false));
        // The bus goes while two dialogs are up.
        let mut side = Side::with_a_portal();
        side.asks(1, 0, &ask());
        side.asks(2, 7, &ask());
        side.portal.gone();
        assert_eq!((side.answered(), side.portal.files, side.portal.up.len()), (vec![(0, None), (7, None)], false, 0));
    }

    #[test]
    fn a_portal_that_says_nothing_to_an_asking_is_given_up_on() {
        let mut side = Side::with_a_portal();
        let request = |number: u32| format!("/org/freedesktop/portal/desktop/request/1_42/taut{number}");
        // Two dialogs are requested a second apart. The portal takes the first, and ignores the
        // second.
        let first = side.asks(1, 7, &ask());
        side.portal.now = Duration::from_secs(1);
        let second = side.asks(2, 8, &ask());
        side.answers(THEM, serial(&first, "OpenFile"), "o", Pack::default().text(&request(1)));
        // It has five seconds: until then nothing is given up, and whoever sleeps is told when
        // they end.
        assert_eq!(side.portal.patience_ends(), Some(Duration::from_secs(6)));
        side.portal.now = Duration::from_millis(5900);
        assert!(!side.portal.give_up(Some(&mut side.bus)) && side.answered().is_empty());
        // Then the second window is answered with no file and has the library's own dialog from
        // then on; the portal is told to close that dialog, should it show; and the first,
        // which it took, stays up.
        side.portal.now = Duration::from_secs(6);
        assert!(side.portal.give_up(Some(&mut side.bus)));
        let said = side.peer.said();
        let close = Message::read(&said[0].1).unwrap();
        assert_eq!((names(&said), close.path, close.interface), (vec!["Close"], request(2).as_str(), REQUEST));
        assert_eq!((side.answered(), side.portal.files, side.portal.up.len(), side.portal.patience_ends()), (vec![(8, None)], false, 1, None));
        assert!(!side.portal.give_up(Some(&mut side.bus)));
        // The portal's later word on that request is nobody's; and the dialog it took is
        // answered whenever the user picks.
        side.answers(THEM, serial(&second, "OpenFile"), "o", Pack::default().text(&request(2)));
        side.news(THEM, &request(2), "Response", "ua{sv}", &ended(0, &["file:///too/late"]));
        assert!(side.answered().is_empty());
        side.portal.now = Duration::from_secs(600);
        assert!(!side.portal.give_up(Some(&mut side.bus)));
        side.news(THEM, &request(1), "Response", "ua{sv}", &ended(0, &["file:///in/time"]));
        assert_eq!(side.answered(), [(7, Some(PathBuf::from("/in/time")))]);
    }

    #[test]
    fn the_dialogs_of_a_window_that_is_gone_are_taken_down() {
        let mut side = Side::with_a_portal();
        for known in [0, 7, 7] {
            side.asks(1, known, &ask());
        }
        // The portal is told to close both of that window's, and no reply is awaited (the 1 in
        // the third byte).
        side.portal.forget(Some(&mut side.bus), 7);
        let closed: Vec<(String, String, String, u8)> = side
            .peer
            .said()
            .iter()
            .map(|(member, bytes)| (member, Message::read(bytes).unwrap(), bytes[2]))
            .map(|(member, call, flags)| (member.clone(), call.interface.into(), call.path.into(), flags))
            .collect();
        let request = |number: u32| format!("/org/freedesktop/portal/desktop/request/1_42/taut{number}");
        assert_eq!(closed, [("Close".into(), REQUEST.into(), request(2), 1), ("Close".into(), REQUEST.into(), request(3), 1)]);
        assert_eq!(side.portal.up.len(), 1);
        // How they end anyway is nobody's news; the other window's dialog is answered.
        side.news(THEM, &request(2), "Response", "ua{sv}", &ended(0, &["file:///late"]));
        side.news(THEM, &request(1), "Response", "ua{sv}", &ended(0, &["file:///first"]));
        assert_eq!(side.answered(), [(0, Some(PathBuf::from("/first")))]);
        // (Without a bus to say it over, they are only forgotten.)
        side.asks(1, 4, &ask());
        side.portal.forget(None, 4);
        assert!(side.portal.up.is_empty() && side.answered().is_empty());
    }

    #[test]
    fn an_ending_is_a_pattern_for_names_in_either_case() {
        assert_eq!([pattern(".png"), pattern(".JPG"), pattern(".tar.gz")], ["*.[pP][nN][gG]", "*.[jJ][pP][gG]", "*.[tT][aA][rR].[gG][zZ]"]);
        // Digits and signs have one case; so does a letter whose capital is two letters.
        assert_eq!([pattern(".7z"), pattern("_v2"), pattern(".stra\u{df}e"), pattern("")], ["*.7[zZ]", "*_[vV]2", "*.[sS][tT][rR][aA]\u{df}[eE]", "*"]);
        assert_eq!(pattern(".\u{e4}\u{44f}x"), "*.\u{e4}\u{44f}[xX]", "a letter outside the plain alphabet stands as it is written");
        // A glob metacharacter stands for itself.
        assert_eq!(pattern(".a*b?[c]"), "*.[aA][*][bB][?][[][cC]]");
    }

    #[test]
    fn what_the_portal_sends_is_read_as_far_as_it_holds() {
        let number = |kinds: &str, values: &Pack| number_in(Message::read(&packed(NEWS, 1, &[(8, "g", kinds)], values)).unwrap().body);
        assert_eq!(number("v", Pack::default().kinds("u").u32(1)), Some(1));
        assert_eq!(number("v", Pack::default().kinds("v").kinds("u").u32(2)), Some(2), "wrapped twice");
        assert_eq!(number("v", Pack::default().kinds("v").kinds("v").kinds("v").kinds("i").u32(1)), Some(1), "or more often");
        assert_eq!(number("v", Pack::default().kinds("s").text("dark")), None);
        assert_eq!(number("v", Pack::default().kinds("v").kinds("v")), None, "wrapped around nothing");
        assert_eq!(number("", &Pack::default()), None);
        // A colour: three components 0 to 1, of that type; anything else is none.
        let color = |values: &Pack| color_in(Message::read(&packed(NEWS, 1, &[(8, "g", "v")], values)).unwrap().body);
        assert_eq!(color(Pack::default().kinds("(ddd)").pad(8).f64(0.2).f64(0.4).f64(1.0)), Some(Color::rgb(0x3366ff)));
        assert_eq!(color(Pack::default().kinds("v").kinds("(ddd)").pad(8).f64(0.0).f64(0.0).f64(0.0)), Some(Color::BLACK), "wrapped twice");
        for odd in [[0.2, 1.01, 1.0], [-0.0001, 0.0, 0.0], [f64::NAN, 0.0, 0.0], [f64::INFINITY, 0.0, 0.0]] {
            assert_eq!(color(Pack::default().kinds("(ddd)").pad(8).f64(odd[0]).f64(odd[1]).f64(odd[2])), None, "{odd:?}");
        }
        assert_eq!((color(Pack::default().kinds("(dd)").pad(8).f64(0.2).f64(0.4)), color(Pack::default().kinds("u").u32(1))), (None, None));

        // All settings at once, truncated anywhere or with bytes changed: read without harm, as
        // far as they hold.
        let full = Stand { scheme: Some(1), accent: Some([0.25, 0.5, 0.75]), contrast: Some(1), motion: Some(1), ..Stand::default() };
        let settings = packed(ANSWER, 1, &[(5, "u", "3"), (8, "g", "a{sa{sv}}")], &appearance(&full));
        let read_all = |bytes: &[u8]| {
            let mut portal = Portal::new(None);
            portal.read_all(Message::read(bytes).map(|message| message.body));
            (portal.dark, portal.accent, portal.high_contrast, portal.reduced_motion)
        };
        assert_eq!(read_all(&settings), (Some(true), Some(Color::rgb(0x4080bf)), true, true));
        (0..settings.len()).for_each(|len| _ = std::hint::black_box(read_all(&settings[..len])));

        // The picked file: the first address, wherever the addresses stand among the results,
        // if it is a local file.
        let ending = |said: &Pack| packed(NEWS, 1, &[(8, "g", "ua{sv}")], said);
        let file = |addresses: &[&str]| file_in(&ending(&ended(0, addresses)));
        assert_eq!(file(&["file:///tmp/a%20b", "file:///tmp/c"]), Some("/tmp/a b".into()));
        assert_eq!(file(&["file://localhost/tmp/%C3%A4%25.txt"]), Some("/tmp/\u{e4}%.txt".into()), "the machine's name is dropped");
        assert_eq!(file(&["file:///tmp/100%"]), Some("/tmp/100%".into()), "a percent sign that begins no byte stays");
        assert_eq!((file(&[]), file(&["sftp://elsewhere/tmp/a"]), file(&["relative/path"]), file(&[""])), (None, None, None, None));
        // (A dialog ended without a file has no addresses.)
        assert_eq!((file_in(&ending(&ended(1, &[]))), file_in(&ending(Pack::default().u32(0).open(8).close()))), (None, None));
        // Addresses of a type other than a string array are not addresses.
        let mut odd = Pack::default();
        odd.u32(0).open(8).pad(8).text("uris").kinds("s").text("file:///tmp/a").close();
        assert_eq!(file_in(&ending(&odd)), None);

        // Truncated anywhere, or with bytes changed, it is read without harm.
        let whole = ending(&ended(0, &["file:///tmp/a%20b", "file:///tmp/c"]));
        (0..whole.len()).for_each(|len| _ = file_in(&whole[..len]));
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut chance = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        for _ in 0..20_000 {
            let mut scrambled = whole.clone();
            for _ in 0..1 + chance(3) {
                let at = chance(scrambled.len());
                scrambled[at] = chance(256) as u8;
            }
            std::hint::black_box((file_in(&scrambled), Message::read(&scrambled).map(|message| number_in(message.body))));
        }
        for _ in 0..20_000 {
            let mut scrambled = settings.clone();
            for _ in 0..1 + chance(3) {
                let at = chance(scrambled.len());
                scrambled[at] = chance(256) as u8;
            }
            std::hint::black_box(read_all(&scrambled));
        }
    }

    /// A private `dbus-daemon` for the test, ended with it. It activates nothing (a session bus
    /// would activate the real portal) and lets any program take any name.
    pub(crate) struct OwnBus {
        daemon: Child,
        folder: PathBuf,
        pub(crate) address: String,
    }

    impl OwnBus {
        /// Nothing where there is no `dbus-daemon`.
        pub(crate) fn start() -> Option<Self> {
            static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let number = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let folder = std::env::temp_dir().join(format!("taut-bus-{}-{number}", std::process::id()));
            std::fs::create_dir_all(&folder).ok()?;
            let rules = format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context=\"default\">
    <allow send_destination=\"*\" eavesdrop=\"true\"/>
    <allow eavesdrop=\"true\"/>
    <allow own=\"*\"/>
  </policy>
</busconfig>",
                folder.display()
            );
            std::fs::write(folder.join("bus.conf"), rules).ok()?;
            let started = Command::new("dbus-daemon")
                .arg(format!("--config-file={}", folder.join("bus.conf").display()))
                .args(["--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let (mut daemon, mut address) = (started.ok()?, String::new());
            BufReader::new(daemon.stdout.take()?).read_line(&mut address).ok()?;
            let own = OwnBus { daemon, folder, address: address.trim().into() };
            (!own.address.is_empty()).then_some(own)
        }
    }

    impl Drop for OwnBus {
        fn drop(&mut self) {
            let _ = (self.daemon.kill(), self.daemon.wait());
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    /// How a stand-in portal behaves.
    #[derive(Clone)]
    pub(crate) struct Stand {
        /// Its colour scheme setting (1 for dark); none where it lacks the setting.
        pub(crate) scheme: Option<u32>,
        /// The accent colour as red, green, blue from 0 to 1; and contrast and reduced motion
        /// (1 for each): none where it lacks the setting.
        pub(crate) accent: Option<[f64; 3]>,
        pub(crate) contrast: Option<u32>,
        pub(crate) motion: Option<u32>,
        /// Its file chooser version; none where it has none.
        pub(crate) version: Option<u32>,
        /// How its file dialog ends, and how long after the request.
        pub(crate) ends: Ends,
        pub(crate) after: Duration,
    }

    impl Default for Stand {
        /// A portal without settings, with a version 4 file chooser the user cancels at once.
        fn default() -> Self {
            Stand { scheme: None, accent: None, contrast: None, motion: None, version: Some(4), ends: Ends::Closed, after: Duration::ZERO }
        }
    }

    /// How a stand-in portal's file dialog ends.
    #[derive(Clone)]
    pub(crate) enum Ends {
        /// The user picks files: their addresses.
        Picked(Vec<String>),
        /// The user cancels by its button.
        Closed,
        /// The user closes it by a key or its window's close button: the portal reports another
        /// ending.
        Otherwise,
        /// The portal refuses the request.
        Refused,
        /// It stays up.
        Never,
        /// The portal ignores the request: neither takes nor refuses it.
        Silent,
    }

    /// A stand-in portal's open file dialog: when and how it ends, the program that asked, and
    /// its request object.
    struct Up {
        at: Instant,
        ends: Ends,
        to: String,
        request: String,
    }

    /// What a test tells its stand-in to do: signal an appearance setting's current value, or a
    /// change in another group.
    enum Tell {
        Setting(&'static str),
        Other,
    }

    /// A portal played by a test thread on a private bus, through the library's own client: it
    /// owns the portal's name there, answers what programs ask of a portal, and signals what a
    /// portal signals. Dropped, it leaves the bus.
    pub(crate) struct StandIn {
        /// How it behaves from now on; and what it was asked: each file dialog as the call's
        /// name and `asking`'s rendering, joined by " | "; each request it was told to close as
        /// "Close" and the request.
        pub(crate) how: Arc<Mutex<Stand>>,
        pub(crate) asked: Arc<Mutex<Vec<String>>>,
        tell: Option<mpsc::Sender<Tell>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl StandIn {
        /// A portal whose user wants dark windows or not, with a version 4 file chooser ending
        /// at once as `ends` says.
        pub(crate) fn start(address: &str, dark: bool, ends: Ends) -> Self {
            StandIn::of(address, Stand { scheme: Some(if dark { 1 } else { 2 }), ends, ..Stand::default() })
        }

        pub(crate) fn of(address: &str, how: Stand) -> Self {
            let mut bus = Bus::open(address).expect("the test's own bus");
            // (The name is this one's once the bus says so: at once, or when an earlier
            // stand-in leaves.)
            bus.call(BUS, BUS_PATH, BUS, "RequestName", "su", Pack::default().text(PORTAL).u32(0));
            let (mut named, since) = (false, Instant::now());
            while !named && since.elapsed() < Duration::from_secs(5) {
                bus.turn(&mut |_, message| named |= message.member == "NameAcquired" && message.body.clone().text() == Some(PORTAL));
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(named, "the bus gave the stand-in the portal's name");
            let (how, asked, (tell, told)) = (Arc::new(Mutex::new(how)), Arc::new(Mutex::new(Vec::new())), mpsc::channel());
            let (its_how, its_asked) = (how.clone(), asked.clone());
            let thread = std::thread::spawn(move || {
                let mut up: Vec<Up> = Vec::new();
                // (Until the test drops its end, or the bus is gone.)
                while !bus.dead {
                    let how = its_how.lock().unwrap().clone();
                    bus.turn(&mut |bus, message| play(bus, message, &how, &mut its_asked.lock().unwrap(), &mut up));
                    match told.try_recv() {
                        Ok(Tell::Setting(name)) => say(&mut bus, name, &its_how.lock().unwrap()),
                        Ok(Tell::Other) => say_other(&mut bus),
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                    let now = Instant::now();
                    up.iter().filter(|up| up.at <= now).for_each(|up| end(&mut bus, up));
                    up.retain(|up| up.at > now);
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            StandIn { how, asked, tell: Some(tell), thread: Some(thread) }
        }

        /// The user sets the colour scheme: 1 dark, 2 light.
        pub(crate) fn say(&self, scheme: u32) {
            self.change(SCHEME, |how| how.scheme = Some(scheme));
        }

        /// The user changes appearance setting `name` as `change` does: the portal signals it.
        pub(crate) fn change(&self, name: &'static str, change: impl FnOnce(&mut Stand)) {
            change(&mut self.how.lock().unwrap());
            self.tell.iter().for_each(|tell| tell.send(Tell::Setting(name)).unwrap());
        }

        /// A desktop setting outside the appearance group changes.
        pub(crate) fn say_other(&self) {
            self.tell.iter().for_each(|tell| tell.send(Tell::Other).unwrap());
        }
    }

    impl Drop for StandIn {
        fn drop(&mut self) {
            self.tell = None;
            self.thread.take().into_iter().for_each(|thread| _ = thread.join());
        }
    }

    /// The stand-in signals an appearance setting's current value. (A setting it lacks, it says
    /// nothing of.)
    fn say(bus: &mut Bus, name: &str, how: &Stand) {
        let mut said = Pack::default();
        said.text(APPEARANCE).text(name);
        if value_of(&mut said, name, how) {
            bus.send(Header { kind: NEWS, path: PORTAL_PATH, interface: SETTINGS, member: "SettingChanged", kinds: "ssv", ..Default::default() }, &said.bytes);
        }
    }

    /// The stand-in signals a change in another group.
    fn say_other(bus: &mut Bus) {
        let mut said = Pack::default();
        said.text("org.gnome.desktop.interface").text("gtk-theme").kinds("s").text("Adwaita");
        bus.send(Header { kind: NEWS, path: PORTAL_PATH, interface: SETTINGS, member: "SettingChanged", kinds: "ssv", ..Default::default() }, &said.bytes);
    }

    /// Writes setting `name`'s value as a portal like `how` has it, as a variant; and says
    /// whether it has one.
    fn value_of(said: &mut Pack, name: &str, how: &Stand) -> bool {
        match (name, how.scheme, how.accent, how.contrast, how.motion) {
            (SCHEME, Some(value), ..) | (CONTRAST, _, _, Some(value), _) | (MOTION, .., Some(value)) => _ = said.kinds("u").u32(value),
            (ACCENT, _, Some([red, green, blue]), ..) => _ = said.kinds("(ddd)").pad(8).f64(red).f64(green).f64(blue),
            _ => return false,
        }
        true
    }

    /// All the appearance settings a portal like `how` has, as it answers for them: one group,
    /// each setting it has.
    fn appearance(how: &Stand) -> Pack {
        let mut said = Pack::default();
        said.open(8).pad(8).text(APPEARANCE).open(8);
        for name in [SCHEME, ACCENT, CONTRAST, MOTION] {
            let before = said.bytes.len();
            said.pad(8).text(name);
            if !value_of(&mut said, name, how) {
                said.bytes.truncate(before);
            }
        }
        said.close().close();
        said
    }

    /// The stand-in tells the requesting program how its file dialog ended.
    fn end(bus: &mut Bus, up: &Up) {
        let said = match &up.ends {
            Ends::Picked(addresses) => ended(0, &addresses.iter().map(String::as_str).collect::<Vec<_>>()),
            Ends::Closed => ended(1, &[]),
            Ends::Otherwise => ended(2, &[]),
            Ends::Refused | Ends::Never | Ends::Silent => return,
        };
        bus.send(Header { kind: NEWS, to: &up.to, path: &up.request, interface: REQUEST, member: "Response", kinds: "ua{sv}", ..Default::default() }, &said.bytes);
    }

    /// What a stand-in portal does with a call: answers settings and file chooser questions as
    /// a portal like `how` (one with no settings does not answer for them, as the real one does
    /// not since 2025); takes a file dialog request or not; and closes a dialog when told.
    fn play(bus: &mut Bus, call: &Message, how: &Stand, asked: &mut Vec<String>, up: &mut Vec<Up>) {
        const NO_CALL: (&str, &str) = (NO_SUCH_CALL, "No such method");
        if call.kind != CALL {
            return;
        }
        // (A portal that ignores file dialog requests notes them, and no more.)
        if call.interface == CHOOSER && matches!(how.ends, Ends::Silent) {
            return asked.push(format!("{} {}", call.member, asking(call.body).join(" | ")));
        }
        let (mut said, mut values) = (call.body, Pack::default());
        let about = (said.text(), said.text());
        let settings = how.scheme.or(how.contrast).or(how.motion).is_some() || how.accent.is_some();
        let answer = match (call.interface, call.member, how.version) {
            (SETTINGS, "ReadAll", _) if settings => Ok(("a{sa{sv}}", values = appearance(how)).0),
            (PROPERTIES, "Get", Some(version)) if about == (Some(CHOOSER), Some("version")) => Ok(("v", values.kinds("u").u32(version)).0),
            (CHOOSER, "OpenFile" | "SaveFile", Some(_)) => show(call, how, asked, up, &mut values),
            (REQUEST, "Close", ..) => {
                asked.push(format!("Close {}", call.path));
                up.retain(|up| up.request != call.path);
                Ok("")
            }
            _ => Err(NO_CALL),
        };
        bus.answer(call, answer, &values);
    }

    /// A stand-in portal asked for a file dialog: notes the request, and takes it (replying
    /// with the request object, by the handle token it was given) unless it is one that
    /// refuses.
    fn show(call: &Message, how: &Stand, asked: &mut Vec<String>, up: &mut Vec<Up>, values: &mut Pack) -> Result<&'static str, (&'static str, &'static str)> {
        let said = asking(call.body);
        let named = said.iter().find_map(|line| line.strip_prefix("handle_token=")).unwrap_or("unnamed");
        let request = format!("{REQUESTS}/{}/{named}", call.sender.trim_start_matches(':').replace('.', "_"));
        asked.push(format!("{} {}", call.member, said.join(" | ")));
        if matches!(how.ends, Ends::Refused) {
            return Err(("org.freedesktop.DBus.Error.Failed", "Nothing here shows a file dialog"));
        }
        values.text(&request);
        up.push(Up { at: Instant::now() + how.after, ends: how.ends.clone(), to: call.sender.into(), request });
        Ok("o")
    }

    /// Has a program's bus side take in what comes until something holds, or five seconds pass.
    /// Says whether it came to hold.
    fn until(bus: &mut Bus, portal: &mut Portal, so: impl Fn(&Portal) -> bool) -> bool {
        let since = Instant::now();
        while !so(portal) && since.elapsed() < Duration::from_secs(5) {
            bus.turn(&mut |bus, message| portal.hear(bus, message));
            std::thread::sleep(Duration::from_millis(1));
        }
        so(portal)
    }

    /// The same exchange as above, over a real bus with a stand-in portal as another client: so
    /// the bus's own Hello, signal routing and name-owner changes are in it.
    #[test]
    fn a_program_and_a_portal_talk_over_a_real_bus() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon to start a bus with: not tested") };
        let mut bus = Bus::open(&own.address).expect("the bus that was just started");
        let mut portal = Portal::new(Some(&mut bus));
        // Nobody owns the portal's name: the bus says so, to both questions.
        assert!(until(&mut bus, &mut portal, |portal| portal.asked == [0; 2] && !portal.me.is_empty()));
        assert!(portal.dark.is_none() && !portal.files && portal.owner.is_empty() && !bus.dead);

        // A portal arrives whose user wants dark windows: the bus signals it, and it is asked.
        let stand = StandIn::start(&own.address, true, Ends::Picked(vec!["file:///tmp/a%20file%20of%20100%25/gr%C3%BCn.png".into()]));
        assert!(until(&mut bus, &mut portal, |portal| portal.dark == Some(true) && portal.files));
        // The user changes their mind twice. A change to another setting is not even routed
        // here: all it hears is counted.
        for (scheme, dark) in [(2, false), (1, true)] {
            stand.say_other();
            stand.say(scheme);
            assert!(until(&mut bus, &mut portal, |portal| portal.dark == Some(dark)), "scheme {scheme}");
        }
        let mut heard = 0;
        stand.say_other();
        stand.say(2);
        let since = Instant::now();
        while since.elapsed() < Duration::from_millis(300) {
            bus.turn(&mut |bus, message| (heard, _) = (heard + 1, portal.hear(bus, message)));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!((heard, portal.dark), (1, Some(false)), "one piece of news came, and it was the one asked for");
        // The user asks for contrast and reduced motion, and picks a colour: the portal signals
        // each.
        stand.change(CONTRAST, |how| how.contrast = Some(1));
        stand.change(MOTION, |how| how.motion = Some(1));
        stand.change(ACCENT, |how| how.accent = Some([1.0, 0.0, 0.5]));
        assert!(until(&mut bus, &mut portal, |portal| portal.high_contrast && portal.reduced_motion && portal.accent == Some(Color::rgb(0xff0080))));
        stand.change(MOTION, |how| how.motion = Some(0));
        assert!(until(&mut bus, &mut portal, |portal| !portal.reduced_motion) && portal.high_contrast);

        // A file dialog: requested, taken, answered with the file.
        portal.ask(Some(&mut bus), "x11:1e00003", 4, &ask());
        assert!(until(&mut bus, &mut portal, |portal| !portal.answers.is_empty()));
        assert_eq!(std::mem::take(&mut portal.answers), [(4, Some(PathBuf::from("/tmp/a file of 100%/gr\u{fc}n.png")))]);
        let folder = "current_folder=/home/someone/Pictures of mine\0";
        assert_eq!(*stand.asked.lock().unwrap(), [format!("OpenFile x11:1e00003 | Open a file | handle_token=taut1 | {folder} | filters=*.png *.jpeg:*.[pP][nN][gG],*.[jJ][pP][eE][gG]")]);

        // One that stays up, for a window that then goes: the portal is told to close it, by
        // the request path the program worked out itself.
        stand.how.lock().unwrap().ends = Ends::Never;
        portal.ask(Some(&mut bus), "x11:1e00003", 4, &ask());
        assert!(until(&mut bus, &mut portal, |portal| portal.up.first().is_some_and(|up| up.call == 0)));
        let request = portal.up[0].request.clone();
        portal.forget(Some(&mut bus), 4);
        let closed = |stand: &StandIn| stand.asked.lock().unwrap().last().cloned();
        let since = Instant::now();
        while closed(&stand) != Some(format!("Close {request}")) && since.elapsed() < Duration::from_secs(5) {
            bus.turn(&mut |bus, message| portal.hear(bus, message));
        }
        assert_eq!((closed(&stand), request.starts_with(&format!("{REQUESTS}/{}/taut2", portal.me))), (Some(format!("Close {request}")), true));

        // And one up when the portal leaves the bus: answered with no file, and there is no
        // desktop dialog any more.
        portal.ask(Some(&mut bus), "x11:1e00003", 4, &ask());
        assert!(until(&mut bus, &mut portal, |portal| portal.up.first().is_some_and(|up| up.call == 0)));
        drop(stand);
        assert!(until(&mut bus, &mut portal, |portal| !portal.files));
        assert_eq!((std::mem::take(&mut portal.answers), portal.up.len(), portal.owner.as_str(), bus.dead), (vec![(4, None)], 0, "", false));
    }
}
