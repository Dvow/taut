//! Linux desktop services over the session bus, per freedesktop.org: the tray icon (`tray`) as
//! a `StatusNotifierItem` with a `com.canonical.dbusmenu` menu; notifications (`notices`) via
//! `org.freedesktop.Notifications`, or the portal in a sandbox; and under Wayland, global
//! hotkeys (`hotkeys`) via the portal's `GlobalShortcuts`. (Under X, hotkeys come from X
//! itself, in `x11`.)
//!
//! It uses the windows' bus connection (`atspi::Bus`), as the portal does: this program exports
//! an object for the panel to query about its icon and menu, and calls the services it needs.
//! Nothing blocks: replies arrive when they arrive, and requests to a missing service are
//! dropped. The icon goes to whichever program hosts tray icons (KDE's panel, GNOME's with its
//! extension), which the `StatusNotifierWatcher` tells of it; the program owns a bus name for
//! the icon while it shows, so its removal is seen. With no watcher there is no icon; one that
//! appears later is told of it. (The old XEmbed tray is not supported.)
//!
//! Activation tokens the desktop gives (Wayland's, handed over by a panel before a click on the
//! icon, or by the notification server before a click on a notification) are kept for the next
//! time the first window comes forward.
//!
//! Set by the first call needing any of this (through `platform::hand_desktop`, and the `x11`
//! and `wayland` windows); nothing else reaches this code, so a program that never asks does
//! not carry it.
use crate::atspi::{ANSWER, BUS, BUS_PATH, Bus, CALL, FAILED, Header, Message, NEWS, PROPERTIES, Pack, Unpack};
use crate::copies;
use crate::hotkeys::{HOTKEYS, Stand};
use crate::notices::{NOTICES, Out};
use crate::tray::{self, Did, HINT, PICTURES, ROWS, Row, RowKind, TRAY, Tray};
use std::cell::RefCell;

/// The tray watcher's and the item's interface names.
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
/// The icon's menu interface.
const MENU: &str = "com.canonical.dbusmenu";
const MENU_PATH: &str = "/MenuBar";
/// The notification server.
const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The portal, and the interfaces used: notifications, global shortcuts, request, session.
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_NOTIFICATION: &str = "org.freedesktop.portal.Notification";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
/// The error returned for an unknown method.
const UNKNOWN: (&str, &str) = ("org.freedesktop.DBus.Error.UnknownMethod", "No such method at path");

/// What the first window should do after a run, from the app's desktop requests: hide; come
/// forward (a later start came); and the activation token received since, if any (under X, a
/// later start's startup id, which carries its time).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Asked {
    pub hide: bool,
    pub forward: bool,
    pub token: String,
}

/// Global shortcuts as the portal holds them: the session (its path, once created), the request
/// tokens for creating it and binding, and the two calls' serials; the window the portal shows
/// its dialog over; and whether the portal lacks the interface.
#[derive(Default)]
struct Keys {
    session: String,
    made: String,
    bound: String,
    calls: [u32; 2],
    parent: String,
    missing: bool,
}

/// The desktop state, on this thread.
#[derive(Default)]
struct Desk {
    /// Signal matches were added on the bus.
    listening: bool,
    /// The icon's bus name while shown (empty otherwise); how many it had; the menu revision.
    name: String,
    names: u32,
    revision: u32,
    /// Notifications: pending calls with their id; and those the server shows, by its id and
    /// this program's.
    asked: Vec<(u32, u32)>,
    shown: Vec<(u32, u32)>,
    /// Portal requests made, each with its own token.
    count: u32,
    keys: Keys,
    /// The last activation token the desktop gave, until used.
    token: String,
}

thread_local! {
    static DESK: RefCell<Desk> = RefCell::new(Desk::default());
}

/// The program's name, as the desktop shows it.
fn program() -> String {
    std::env::current_exe().ok().and_then(|path| path.file_stem().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default()
}

/// After a run: passes the app's requests to the desktop over the session bus (none without a
/// bus), and returns what the first window should do. Global shortcuts go through the portal
/// under Wayland (`wayland`), which shows its dialog over the window it knows as `parent`.
pub(crate) fn ran(bus: Option<&mut Bus>, wayland: bool, parent: &str) -> Asked {
    let ((said, changed), hide) = TRAY.with_borrow_mut(|tray| (tray.settle(), std::mem::take(&mut tray.hide)));
    let out = NOTICES.with_borrow_mut(|notices| {
        notices.clicked.clear();
        std::mem::take(&mut notices.out)
    });
    let later = copies::forward();
    let mut token = DESK.with_borrow_mut(|desk| std::mem::take(&mut desk.token));
    match bus {
        Some(bus) => DESK.with_borrow_mut(|desk| {
            desk.listen(bus);
            desk.tray(bus, said, changed);
            out.into_iter().for_each(|out| desk.notify(bus, out));
            if wayland {
                desk.bind(bus, parent);
            }
        }),
        // Without a bus, no portal.
        None if wayland => HOTKEYS.with_borrow_mut(|hotkeys| {
            hotkeys.settle(|_| {});
            refuse(&mut hotkeys.keys, |_| true);
        }),
        None => {}
    }
    if let Some(later) = &later {
        token = if wayland { later.wayland.clone() } else { later.x.clone() };
    }
    Asked { hide, forward: later.is_some(), token }
}

/// Refuses the keys `which` selects: they are taken, and the app hears so.
fn refuse(keys: &mut [crate::hotkeys::Wanted], which: impl Fn(&crate::hotkeys::Wanted) -> bool) {
    let mut refused = false;
    for wanted in keys.iter_mut().filter(|wanted| wanted.stand != Stand::Taken && which(wanted)) {
        (wanted.stand, refused) = (Stand::Taken, true);
    }
    if refused {
        crate::platform::Waker.wake();
    }
}

/// A bus message arrived: whatever of it concerns the icon, its menu, notifications or
/// shortcuts is taken in. Returns whether the UI should run for it.
pub(crate) fn heard(bus: &mut Bus, message: &Message) -> bool {
    DESK.with_borrow_mut(|desk| desk.heard(bus, message))
}

impl Desk {
    /// Adds the signal matches once: tray watchers coming and going, notification server and
    /// portal signals, and shortcut signals.
    fn listen(&mut self, bus: &mut Bus) {
        if std::mem::replace(&mut self.listening, true) {
            return;
        }
        bus.ask("AddMatch", &format!("type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{WATCHER}'"));
        bus.ask("AddMatch", &format!("type='signal',interface='{NOTIFICATIONS}',path='{NOTIFICATIONS_PATH}'"));
        bus.ask("AddMatch", &format!("type='signal',interface='{PORTAL_NOTIFICATION}',member='ActionInvoked'"));
        bus.ask("AddMatch", &format!("type='signal',interface='{SHORTCUTS}',member='Activated'"));
    }

    /// Passes the icon on as the run says: it takes a bus name and registers with the watcher;
    /// or it leaves the bus; or changes are signalled to the host.
    fn tray(&mut self, bus: &mut Bus, said: bool, changed: u8) {
        match (said, self.name.is_empty()) {
            (true, true) => {
                self.names += 1;
                self.name = format!("{ITEM}-{}-{}", std::process::id(), self.names);
                // 4: DBUS_NAME_FLAG_DO_NOT_QUEUE.
                bus.call(BUS, BUS_PATH, BUS, "RequestName", "su", Pack::default().text(&self.name).u32(4));
                self.register(bus);
            }
            (false, false) => {
                bus.call(BUS, BUS_PATH, BUS, "ReleaseName", "s", Pack::default().text(&std::mem::take(&mut self.name)));
            }
            (true, false) => self.changed(bus, changed),
            (false, true) => {}
        }
    }

    /// Signals the host what changed: icon images, tooltip and title, menu (new revision).
    fn changed(&mut self, bus: &mut Bus, changed: u8) {
        let news = [(PICTURES, ITEM_PATH, ITEM, "NewIcon"), (HINT, ITEM_PATH, ITEM, "NewToolTip"), (HINT, ITEM_PATH, ITEM, "NewTitle")];
        for (_, path, interface, member) in news.into_iter().filter(|(what, ..)| changed & what != 0) {
            bus.send(Header { kind: NEWS, path, interface, member, ..Default::default() }, &[]);
        }
        if changed & ROWS != 0 {
            self.revision += 1;
            let values = Pack::default().u32(self.revision).u32(0).bytes.clone();
            bus.send(Header { kind: NEWS, path: MENU_PATH, interface: MENU, member: "LayoutUpdated", kinds: "ui", ..Default::default() }, &values);
        }
    }

    /// Registers this icon with the tray watcher, by its bus name.
    fn register(&mut self, bus: &mut Bus) {
        bus.call(WATCHER, WATCHER_PATH, WATCHER, "RegisterStatusNotifierItem", "s", Pack::default().text(&self.name));
    }

    /// Shows a notification: through the portal in a sandbox (Flatpak), else the notification
    /// server. Its image is the app's, else the tray icon's, if there is one.
    fn notify(&mut self, bus: &mut Bus, mut out: Out) {
        let picture = out.picture.take().or_else(|| TRAY.with_borrow(|tray| tray::fitting(&tray.pictures, 64).map(|pixels| (64, pixels))));
        if std::path::Path::new("/.flatpak-info").exists() {
            return notify_portal(bus, &out, picture);
        }
        let mut values = Pack::default();
        values.text(&program()).u32(0).text("").text(&out.title).text(&out.text);
        // The one action: the default (clicking it).
        values.open(4).text("default").text("Open").close();
        values.open(8);
        if let Some((width, pixels)) = &picture {
            image_data(&mut values, *width, pixels);
        }
        values.close().u32(-1i32 as u32);
        let call = bus.call(NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS, "Notify", "susssasa{sv}i", &values);
        self.asked.push((call, out.number));
    }

    /// Binds the app's shortcuts in a new portal session: a new one each time they change (the
    /// portal binds a session's shortcuts once), closing the last. Each is requested as the
    /// app's key, by its id, with the key as its description; the portal replies which it
    /// bound. (A portal without the interface, or without a session, refuses all.)
    fn bind(&mut self, bus: &mut Bus, parent: &str) {
        let missing = self.keys.missing;
        let rebind = HOTKEYS.with_borrow_mut(|hotkeys| {
            let mut changed = false;
            hotkeys.settle(|wanted| changed |= wanted.stand != Stand::Taken);
            changed |= hotkeys.keys.iter().any(|wanted| wanted.stand == Stand::New);
            if changed && missing {
                refuse(&mut hotkeys.keys, |_| true);
            }
            if changed && !missing {
                hotkeys.keys.iter_mut().for_each(|wanted| wanted.stand = Stand::Asked);
            }
            changed && !missing
        });
        if !rebind {
            return;
        }
        if !self.keys.session.is_empty() {
            bus.call(PORTAL, &self.keys.session, SESSION, "Close", "", &Pack::default());
        }
        self.count += 1;
        self.keys = Keys { made: format!("taut_keys{}", self.count), parent: parent.into(), ..Keys::default() };
        let mut asks = Pack::default();
        asks.open(8).pad(8).text("handle_token").kinds("s").text(&self.keys.made);
        asks.pad(8).text("session_handle_token").kinds("s").text(&self.keys.made).close();
        self.keys.calls[0] = bus.call(PORTAL, PORTAL_PATH, SHORTCUTS, "CreateSession", "a{sv}", &asks);
    }

    /// The portal created the shortcut session: the shortcuts are bound in it, with the first
    /// window as the dialog's parent.
    fn made(&mut self, bus: &mut Bus, session: &str) {
        let parent = self.keys.parent.clone();
        self.count += 1;
        (self.keys.session, self.keys.bound) = (session.into(), format!("taut_keys{}", self.count));
        let mut asks = Pack::default();
        asks.text(session).open(8);
        HOTKEYS.with_borrow(|hotkeys| {
            for wanted in hotkeys.keys.iter().filter(|wanted| wanted.stand == Stand::Asked) {
                let trigger = wanted.combo.trigger();
                asks.pad(8).text(&wanted.number.to_string()).open(8);
                asks.pad(8).text("description").kinds("s").text(&trigger);
                asks.pad(8).text("preferred_trigger").kinds("s").text(&trigger).close();
            }
        });
        asks.close().text(&parent).open(8).pad(8).text("handle_token").kinds("s").text(&self.keys.bound).close();
        self.keys.calls[1] = bus.call(PORTAL, PORTAL_PATH, SHORTCUTS, "BindShortcuts", "oa(sa{sv})sa{sv}", &asks);
    }

    /// One bus message: see `heard`.
    fn heard(&mut self, bus: &mut Bus, message: &Message) -> bool {
        match (message.kind, message.path, message.member) {
            (CALL, ITEM_PATH, _) => self.item_asked(bus, message),
            (CALL, MENU_PATH, _) => menu_asked(bus, message, self.revision),
            (NEWS, _, "NameOwnerChanged") => self.listed(bus, message.body),
            (NEWS, NOTIFICATIONS_PATH, _) if message.interface == NOTIFICATIONS => self.notified(message),
            (NEWS, PORTAL_PATH, "ActionInvoked") if message.interface == PORTAL_NOTIFICATION => portal_clicked(message.body),
            (NEWS, _, "Activated") if message.interface == SHORTCUTS => self.pressed(message.body),
            (NEWS, path, "Response") if message.interface == REQUEST => self.responded(bus, path, message.body),
            (ANSWER | FAILED, ..) => self.answered(message),
            _ => false,
        }
    }

    /// A tray watcher appeared or went away: a new one is told of the icon, if there is one.
    fn listed(&mut self, bus: &mut Bus, mut said: Unpack) -> bool {
        let (Some(WATCHER), Some(_), Some(now)) = (said.text(), said.text(), said.text()) else { return false };
        if !now.is_empty() && !self.name.is_empty() {
            self.register(bus);
        }
        false
    }

    /// A reply arrived: from the notification server to a request (with its id for it), or a
    /// failed shortcuts request: the portal lacks the interface (not asked again) or a session,
    /// and the keys are refused.
    fn answered(&mut self, message: &Message) -> bool {
        if let Some(at) = self.asked.iter().position(|(call, _)| *call == message.answers) {
            let (_, number) = self.asked.remove(at);
            let mut said = message.body;
            if let (ANSWER, Some(id)) = (message.kind, said.u32()) {
                self.shown.push((id, number));
            }
            return false;
        }
        let Some(which) = self.keys.calls.iter().position(|call| *call == message.answers && *call != 0).filter(|_| message.kind == FAILED) else { return false };
        self.keys.missing |= which == 0;
        self.keys.calls[which] = 0;
        HOTKEYS.with_borrow_mut(|hotkeys| refuse(&mut hotkeys.keys, |wanted| wanted.stand == Stand::Asked));
        false
    }

    /// A notification server signal: it was clicked (its action invoked), an activation token,
    /// or it closed.
    fn notified(&mut self, message: &Message) -> bool {
        let mut said = message.body;
        let Some(id) = said.u32() else { return false };
        let number = self.shown.iter().find(|(shown, _)| *shown == id).map(|(_, number)| *number);
        match (message.member, number) {
            ("ActionInvoked", Some(number)) => {
                NOTICES.with_borrow_mut(|notices| notices.clicked.push(number));
                true
            }
            ("ActivationToken", Some(_)) => {
                self.token = token(said.text());
                false
            }
            ("NotificationClosed", Some(_)) => {
                self.shown.retain(|(shown, _)| *shown != id);
                false
            }
            _ => false,
        }
    }

    /// The portal says a shortcut was pressed in this program's session, with an activation
    /// token.
    fn pressed(&mut self, mut said: Unpack) -> bool {
        let (Some(session), Some(id)) = (said.text(), said.text()) else { return false };
        let Some(number) = id.parse::<u32>().ok().filter(|_| session == self.keys.session) else { return false };
        // Then the timestamp, and the options: the token.
        if said.take(8, 8).is_some() {
            self.token = token(token_in(said));
        }
        HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.pressed(number))
    }

    /// The portal replied to a shortcuts request: the session is created (or not), or which
    /// shortcuts it bound.
    fn responded(&mut self, bus: &mut Bus, path: &str, mut said: Unpack) -> bool {
        let ends = |token: &str| !token.is_empty() && path.strip_suffix(token).is_some_and(|path| path.ends_with('/'));
        let (made, bound) = (ends(&self.keys.made), ends(&self.keys.bound) && !self.keys.session.is_empty());
        if !made && !bound {
            return false;
        }
        let fine = said.u32() == Some(0);
        if made {
            self.keys.made.clear();
            match results(said, "session_handle", "s").and_then(|mut value| value.text()).filter(|session| fine && object(session)) {
                Some(session) => self.made(bus, session),
                None => HOTKEYS.with_borrow_mut(|hotkeys| refuse(&mut hotkeys.keys, |wanted| wanted.stand == Stand::Asked)),
            }
            return false;
        }
        self.keys.bound.clear();
        let had = results(said, "shortcuts", "a(sa{sv})").filter(|_| fine).map(bound_ids).unwrap_or_default();
        HOTKEYS.with_borrow_mut(|hotkeys| {
            for wanted in hotkeys.keys.iter_mut().filter(|wanted| wanted.stand == Stand::Asked && had.contains(&wanted.number)) {
                wanted.stand = Stand::Held;
            }
            refuse(&mut hotkeys.keys, |wanted| wanted.stand == Stand::Asked);
        });
        true
    }

    /// The panel calls the icon: a property, or `Activate` (or `SecondaryActivate` or `Scroll`,
    /// which do nothing here; the menu is its own object). Before `Activate` it may pass an
    /// activation token.
    fn item_asked(&mut self, bus: &mut Bus, message: &Message) -> bool {
        let (mut asked, mut values) = (message.body, Pack::default());
        let (kinds, clicked) = match (message.interface, message.member) {
            (PROPERTIES, "GetAll") => {
                TRAY.with_borrow(|tray| item_properties(&mut values, tray, None));
                ("a{sv}", false)
            }
            (PROPERTIES, "Get") => match asked.text().and(asked.text()) {
                Some(name) if TRAY.with_borrow(|tray| item_properties(&mut values, tray, Some(name))) => ("v", false),
                _ => return answer_unknown(bus, message),
            },
            (ITEM, "Activate") => ("", true),
            (ITEM, "SecondaryActivate" | "ContextMenu" | "Scroll") => ("", false),
            (ITEM, "ProvideXdgActivationToken") => {
                self.token = token(asked.text());
                ("", false)
            }
            _ => return answer_unknown(bus, message),
        };
        bus.answer(message, Ok(kinds), &values);
        if clicked {
            TRAY.with_borrow_mut(|tray| tray.news.push(Did::Clicked));
        }
        clicked
    }
}

/// Writes the icon's properties as `a{sv}` (all) or one `v` (the named one); returns whether it
/// exists. What a panel reads: category, title, icon pixmaps, tooltip, status, and the menu's
/// path.
fn item_properties(values: &mut Pack, tray: &Tray, only: Option<&str>) -> bool {
    let names = ["Category", "Id", "Title", "Status", "WindowId", "IconName", "IconPixmap", "OverlayIconName", "AttentionIconName", "ToolTip", "ItemIsMenu", "Menu"];
    if only.is_none() {
        values.open(8);
    }
    let mut found = false;
    for name in names.into_iter().filter(|name| only.is_none_or(|only| only == *name)) {
        found = true;
        if only.is_none() {
            values.pad(8).text(name);
        }
        match name {
            "Category" => _ = values.kinds("s").text("ApplicationStatus"),
            "Id" => _ = values.kinds("s").text(&program()),
            "Title" => _ = values.kinds("s").text(&tray.hint.lines().next().map_or_else(program, str::to_string)),
            "Status" => _ = values.kinds("s").text("Active"),
            "WindowId" => _ = values.kinds("i").u32(0),
            "ItemIsMenu" => _ = values.kinds("b").u32(0),
            "Menu" => _ = values.kinds("o").text(MENU_PATH),
            "IconPixmap" => pixmaps(values.kinds("a(iiay)"), &tray.pictures),
            "ToolTip" => _ = values.kinds("(sa(iiay)ss)").pad(8).text("").open(8).close().text(&tray.hint).text(""),
            _ => _ = values.kinds("s").text(""),
        }
    }
    if only.is_none() {
        values.close();
    }
    found
}

/// Pixmaps as a panel reads them (`a(iiay)`): width, height, then 4-byte ARGB pixels.
fn pixmaps(values: &mut Pack, pictures: &[(usize, Vec<u32>)]) {
    values.open(8);
    for (width, pixels) in pictures {
        values.pad(8).u32(*width as u32).u32((pixels.len() / (*width).max(1)) as u32).open(1);
        pixels.iter().for_each(|pixel| values.bytes.extend(pixel.to_be_bytes()));
        values.close();
    }
    values.close();
}

/// A notification image (`image-data`): width, height, row stride, has-alpha, 8 bits per
/// sample, 4 channels, then RGBA pixels.
fn image_data(values: &mut Pack, width: usize, pixels: &[u32]) {
    let high = pixels.len() / width.max(1);
    values.pad(8).text("image-data").kinds("(iiibiiay)").pad(8);
    values.u32(width as u32).u32(high as u32).u32(width as u32 * 4).u32(1).u32(8).u32(4).open(1);
    pixels.iter().for_each(|pixel| values.bytes.extend([(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8, (pixel >> 24) as u8]));
    values.close();
}

/// Shows a notification through the portal, for a sandboxed program: by its own id ("taut" and
/// a number), with title, body, a PNG icon, and a default action (which the portal signals back
/// as `ActionInvoked`).
fn notify_portal(bus: &mut Bus, out: &Out, picture: Option<(usize, Vec<u32>)>) {
    let mut values = Pack::default();
    values.text(&format!("taut{}", out.number)).open(8);
    values.pad(8).text("title").kinds("s").text(&out.title);
    values.pad(8).text("body").kinds("s").text(&out.text);
    values.pad(8).text("default-action").kinds("s").text("default");
    if let Some(png) = picture.and_then(|(width, pixels)| crate::png::encode_clear(width, &pixels)) {
        values.pad(8).text("icon").kinds("(sv)").pad(8).text("bytes").kinds("ay").open(1).bytes.extend_from_slice(&png);
        values.close();
    }
    values.close();
    bus.call(PORTAL, PORTAL_PATH, PORTAL_NOTIFICATION, "AddNotification", "sa{sv}", &values);
}

/// The portal says a notification was clicked: by its id, "taut" and a number.
fn portal_clicked(mut said: Unpack) -> bool {
    let Some(number) = said.text().and_then(|id| id.strip_prefix("taut")).and_then(|number| number.parse().ok()) else { return false };
    NOTICES.with_borrow_mut(|notices| notices.clicked.push(number));
    true
}

/// From a portal reply's results (`a{sv}`): the named value, if of these types.
fn results<'a>(mut said: Unpack<'a>, name: &str, kinds: &str) -> Option<Unpack<'a>> {
    let end = said.list(8)?;
    while said.at < end {
        let (key, of) = (said.take(8, 0).and(said.text())?, said.kinds()?);
        if (key, of) == (name, kinds) {
            return Some(said);
        }
        said.skip(of)?;
    }
    None
}

/// Whether a string is a valid D-Bus object path: `/`, or `/`-separated names of letters,
/// digits and `_`. (A portal's session handle is sent back as such a path, and the bus
/// disconnects a program sending an invalid one. The handle may come from another program.)
fn object(path: &str) -> bool {
    let named = |name: &str| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    path == "/" || path.strip_prefix('/').is_some_and(|rest| rest.split('/').all(named))
}

/// The ids of the shortcuts a portal bound, from its list (`a(sa{sv})`).
fn bound_ids(mut said: Unpack) -> Vec<u32> {
    let mut ids = Vec::new();
    let Some(end) = said.list(8) else { return ids };
    while said.at < end {
        let Some(id) = said.take(8, 0).and(said.text()) else { break };
        ids.extend(id.parse::<u32>());
        if said.skip("a{sv}").is_none() {
            break;
        }
    }
    ids
}

/// The activation token among a shortcut's options (`a{sv}`).
fn token_in(said: Unpack<'_>) -> Option<&str> {
    results(said, "activation_token", "s")?.text()
}

/// The longest activation token kept (desktops give a few dozen bytes).
pub(crate) const TOKEN: usize = 512;

/// An activation token another bus client sent: kept as is if no longer than a desktop's, else
/// dropped. (A far longer one is no desktop's, and under Wayland must not be forwarded: a
/// compositor disconnects a client sending a message over 4096 bytes.)
fn token(said: Option<&str>) -> String {
    said.filter(|token| token.len() <= TOKEN).unwrap_or_default().into()
}

/// A menu label as dbusmenu writes it: the access key marked `_` where the app marked it `&`
/// (`&&` is a literal `&`), and a literal `_` doubled.
fn underlined(label: &str) -> String {
    let mut written = String::with_capacity(label.len());
    let mut letters = label.chars().peekable();
    while let Some(letter) = letters.next() {
        match letter {
            '&' if letters.peek() == Some(&'&') => written.push(letters.next().unwrap_or('&')),
            '&' => written.push('_'),
            '_' => written.push_str("__"),
            '\t' => break,
            _ => written.push(letter),
        }
    }
    written
}

/// A menu item's properties as dbusmenu has them (`a{sv}`): a separator, or its label, enabled
/// state, and check or radio state.
fn row_properties(values: &mut Pack, row: Option<&Row>) {
    values.open(8);
    match row.map(|row| (row.kind, row)) {
        None => _ = values.pad(8).text("children-display").kinds("s").text("submenu"),
        Some((RowKind::Line, _)) => _ = values.pad(8).text("type").kinds("s").text("separator"),
        Some((kind, row)) => {
            values.pad(8).text("label").kinds("s").text(&underlined(&row.label));
            values.pad(8).text("enabled").kinds("b").u32(row.enabled as u32);
            let toggle = match kind {
                RowKind::Tick(on) => Some(("checkmark", on)),
                RowKind::Choice(on) => Some(("radio", on)),
                _ => None,
            };
            if let Some((kind, on)) = toggle {
                values.pad(8).text("toggle-type").kinds("s").text(kind);
                values.pad(8).text("toggle-state").kinds("i").u32(on as u32);
            }
        }
    }
    values.close();
}

/// The menu, or one item, with its children (`(ia{sv}av)`): its id (0 for the root, whose
/// children are variants; an item's id is its position from 1), its properties, and its
/// children (items have none).
fn layout(values: &mut Pack, rows: &[Row], id: usize) {
    let row = |values: &mut Pack, id: usize| {
        values.pad(8).u32(id as u32);
        row_properties(values, id.checked_sub(1).and_then(|at| rows.get(at)));
    };
    row(values, id);
    values.open(1);
    for at in (0..rows.len()).filter(|_| id == 0) {
        row(values.kinds("(ia{sv}av)"), at + 1);
        values.open(1).close();
    }
    values.close();
}

/// The panel calls the menu: its layout (root and items, at which revision), some items'
/// properties, an item clicked, whether to update before showing (always current), or the
/// menu's own properties.
fn menu_asked(bus: &mut Bus, message: &Message, revision: u32) -> bool {
    let (mut asked, mut values) = (message.body, Pack::default());
    let rows = TRAY.with_borrow(|tray| tray.rows.clone());
    let (kinds, picked) = match (message.interface, message.member) {
        (MENU, "GetLayout") => {
            let id = asked.u32().unwrap_or(0) as usize;
            values.u32(revision);
            layout(&mut values, &rows, if id <= rows.len() { id } else { 0 });
            ("u(ia{sv}av)", None)
        }
        (MENU, "GetGroupProperties") => {
            // Each requested item is returned once, however often requested: the list is
            // another program's, and may be truncated, or name one item a million times.
            let (mut asked_for, end) = (vec![false; rows.len() + 1], asked.list(4).unwrap_or(0));
            while let Some(id) = (asked.at < end).then(|| asked.u32()).flatten() {
                asked_for.get_mut(id as usize).into_iter().for_each(|asked| *asked = true);
            }
            values.open(8);
            for id in (0..asked_for.len()).filter(|id| asked_for[*id]) {
                values.pad(8).u32(id as u32);
                row_properties(&mut values, id.checked_sub(1).and_then(|at| rows.get(at)));
            }
            values.close();
            ("a(ia{sv})", None)
        }
        (MENU, "Event") => {
            let (id, event) = (asked.u32().unwrap_or(0) as usize, asked.text().unwrap_or_default());
            ("", (event == "clicked").then_some(id))
        }
        (MENU, "EventGroup") => {
            values.open(4).close();
            ("ai", first_click(asked))
        }
        (MENU, "AboutToShow") => {
            values.u32(0);
            ("b", None)
        }
        (MENU, "AboutToShowGroup") => {
            values.open(4).close().open(4).close();
            ("aiai", None)
        }
        (PROPERTIES, "GetAll") => {
            values.open(8).pad(8).text("Version").kinds("u").u32(3).pad(8).text("TextDirection").kinds("s").text("ltr");
            values.pad(8).text("Status").kinds("s").text("normal").close();
            ("a{sv}", None)
        }
        (PROPERTIES, "Get") => {
            match asked.text().and(asked.text()) {
                Some("Version") => _ = values.kinds("u").u32(3),
                Some("TextDirection") => _ = values.kinds("s").text("ltr"),
                Some("Status") => _ = values.kinds("s").text("normal"),
                _ => return answer_unknown(bus, message),
            }
            ("v", None)
        }
        _ => return answer_unknown(bus, message),
    };
    bus.answer(message, Ok(kinds), &values);
    let Some(at) = picked.and_then(|id| id.checked_sub(1)).filter(|at| rows.get(*at).is_some_and(|row| row.enabled && row.kind != RowKind::Line)) else { return false };
    TRAY.with_borrow_mut(|tray| tray.news.push(Did::Picked(at)));
    true
}

/// The first clicked item among menu events as a panel batches them (`a(isvu)`: item id, event,
/// data, timestamp).
fn first_click(mut said: Unpack) -> Option<usize> {
    let end = said.list(8)?;
    while said.at < end {
        let (id, event) = (said.take(8, 0).and(said.u32())?, said.text()?);
        if event == "clicked" {
            return Some(id as usize);
        }
        said.skip("vu")?;
    }
    None
}

/// Replies to an unknown method with that error.
fn answer_unknown(bus: &mut Bus, message: &Message) -> bool {
    bus.answer(message, Err(UNKNOWN), &Pack::default());
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi::tests::{Peer, packed};
    use crate::testing::Trial;
    use crate::ui::Ui;
    use std::os::unix::net::UnixStream;

    /// This program's end of the session bus; the test plays the bus and everyone on it.
    struct Side {
        bus: Bus,
        peer: Peer,
        serial: u32,
    }

    impl Side {
        fn new() -> Self {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let mut bus = Bus::over(ours).unwrap();
            let mut peer = Peer::new(theirs);
            peer.let_on(&mut || bus.turn(&mut |_, _| {}));
            Side { bus, peer, serial: 1000 }
        }

        /// The end of a run, as the Wayland (or X) windows handle it: what the first window
        /// gets, and what went out over the bus.
        fn ran(&mut self, wayland: bool) -> (Asked, Vec<(String, Vec<u8>)>) {
            let asked = ran(Some(&mut self.bus), wayland, "wayland:first");
            (asked, self.peer.said())
        }

        /// A message arrives from `from` for the object at `path`: whether the UI runs for it,
        /// and the reply.
        fn comes(&mut self, kind: u8, from: &str, [path, interface, member, kinds]: [&str; 4], body: &Pack) -> (bool, Vec<Vec<u8>>) {
            self.serial += 1;
            let mut fields = vec![(1, "o", path), (2, "s", interface), (3, "s", member), (7, "s", from)];
            if !kinds.is_empty() {
                fields.push((8, "g", kinds));
            }
            self.peer.send(&packed(kind, self.serial, &fields, body));
            let mut news = false;
            self.bus.turn(&mut |bus, message| news |= heard(bus, message));
            (news, self.peer.messages())
        }

        /// The panel calls the program: whether the UI runs for it, and the reply.
        fn call(&mut self, path: &str, interface: &str, member: &str, kinds: &str, body: &Pack) -> (bool, Vec<u8>) {
            let (news, said) = self.comes(CALL, ":1.7", [path, interface, member, kinds], body);
            let serial = self.serial;
            let answer = said.into_iter().find(|bytes| Message::read(bytes).is_some_and(|message| message.answers == serial)).expect("an answer");
            (news, answer)
        }

        /// What `from` replies to a call of the program's.
        fn answers(&mut self, from: &str, to: u32, kinds: &str, body: &Pack) {
            self.peer.send(&packed(ANSWER, 600, &[(5, "u", &to.to_string()), (7, "s", from), (8, "g", kinds)], body));
            self.bus.turn(&mut |bus, message| _ = heard(bus, message));
        }
    }

    fn names(said: &[(String, Vec<u8>)]) -> Vec<&str> {
        said.iter().map(|(name, _)| name.as_str()).collect()
    }
    fn body(bytes: &[u8]) -> Unpack<'_> {
        Message::read(bytes).unwrap().body
    }
    fn called<'a>(said: &'a [(String, Vec<u8>)], member: &str) -> &'a [u8] {
        &said.iter().find(|(name, _)| name == member).unwrap_or_else(|| panic!("{member} was not said: {:?}", names(said))).1
    }

    /// The entries of an `a{sv}`: each name, signature and value.
    fn entries(mut said: Unpack) -> Vec<(String, String, Unpack)> {
        let (end, mut entries) = (said.list(8).unwrap(), Vec::new());
        while said.at < end {
            let (name, kinds) = (said.take(8, 0).and(said.text()).unwrap(), said.kinds().unwrap());
            entries.push((name.to_string(), kinds.to_string(), said));
            said.skip(kinds).unwrap();
        }
        entries
    }
    fn text_of(entries: &[(String, String, Unpack)], name: &str) -> String {
        let (_, kinds, mut value) = entries.iter().find(|(named, ..)| named == name).cloned().unwrap_or_else(|| panic!("no {name}"));
        match kinds.as_str() {
            "s" | "o" => value.text().unwrap().to_string(),
            _ => value.u32().unwrap().to_string(),
        }
    }

    /// The tray tests' app: its icon (2 by 2, half-transparent red), tooltip, menu, and what it
    /// heard.
    struct Tray {
        shown: bool,
        hint: &'static str,
        quiet: bool,
        heard: Vec<&'static str>,
    }
    const PIXELS: [u32; 4] = [0x80ff_0000; 4];

    fn tray(app: &mut Tray, ui: &mut Ui) {
        if app.shown {
            if ui.tray(&[(2, &PIXELS)], app.hint).clicked {
                app.heard.push("clicked");
            }
            ui.tray_item("&Show");
            if ui.tray_check(&mut app.quiet, "Quiet") {
                app.heard.push("quiet");
            }
            ui.tray_line();
            ui.set_enabled(false);
            ui.tray_item("Save_as");
            ui.set_enabled(true);
        }
        ui.label("The window");
    }

    /// The menu items as dbusmenu lays them out: each id, and label ("-" for a separator),
    /// enabled state, and toggle state.
    fn rows_in(answer: &[u8]) -> (u32, Vec<String>) {
        let mut said = body(answer);
        let revision = said.u32().unwrap();
        assert_eq!((said.take(8, 0).and(said.u32()), text_of(&entries(said), "children-display")), (Some(0), "submenu".to_string()));
        said.skip("a{sv}").unwrap();
        let (end, mut rows) = (said.list(1).unwrap(), Vec::new());
        while said.at < end {
            assert_eq!(said.kinds(), Some("(ia{sv}av)"));
            let id = said.take(8, 0).and(said.u32()).unwrap();
            let entries = entries(said);
            said.skip("a{sv}").unwrap();
            assert_eq!(said.list(1), Some(said.at), "a row holds no rows");
            let row = match entries.iter().any(|(name, ..)| name == "type") {
                true => format!("{id} -"),
                false => format!("{id} {} {}", text_of(&entries, "label"), text_of(&entries, "enabled")),
            };
            let toggle = entries.iter().any(|(name, ..)| name == "toggle-type").then(|| format!(" {} {}", text_of(&entries, "toggle-type"), text_of(&entries, "toggle-state")));
            rows.push(row + &toggle.unwrap_or_default());
        }
        (revision, rows)
    }

    #[test]
    fn the_icon_is_on_the_bus_for_the_panel_and_what_the_person_does_with_it_comes_back() {
        let mut trial = Trial::new(Tray { shown: true, hint: "Notes\nTwo new", quiet: false, heard: Vec::new() }, tray);
        let mut side = Side::new();
        // It takes its own bus name and registers with the watcher.
        let (_, said) = side.ran(false);
        assert_eq!(names(&said), ["AddMatch", "AddMatch", "AddMatch", "AddMatch", "RequestName", "RegisterStatusNotifierItem"]);
        let name = format!("{ITEM}-{}-1", std::process::id());
        let mut asked = body(called(&said, "RequestName"));
        assert_eq!((asked.text(), asked.u32()), (Some(name.as_str()), Some(4)));
        assert_eq!(body(called(&said, "RegisterStatusNotifierItem")).text(), Some(name.as_str()));

        // The panel reads it: category, title (the tooltip's first line), pixmaps (ARGB), menu.
        let (_, answer) = side.call(ITEM_PATH, PROPERTIES, "GetAll", "s", Pack::default().text(ITEM));
        let read = entries(body(&answer));
        let texts = ["Category", "Status", "Title", "Menu", "ItemIsMenu"].map(|name| text_of(&read, name));
        assert_eq!(texts, ["ApplicationStatus", "Active", "Notes", MENU_PATH, "0"]);
        let (_, kinds, mut pictures) = read.iter().find(|(name, ..)| name == "IconPixmap").cloned().unwrap();
        pictures.list(8).unwrap();
        let (wide, high) = (pictures.take(8, 0).and(pictures.u32()), pictures.u32());
        let len = pictures.list(1).unwrap() - pictures.at;
        assert_eq!((kinds.as_str(), wide, high, pictures.take(1, len).unwrap()), ("a(iiay)", Some(2), Some(2), &[0x80, 0xff, 0, 0].repeat(4)[..]));
        let (_, answer) = side.call(ITEM_PATH, PROPERTIES, "Get", "ss", Pack::default().text(ITEM).text("ToolTip"));
        let mut hint = body(&answer);
        assert_eq!(hint.kinds(), Some("(sa(iiay)ss)"));
        hint.take(8, 0).and(hint.text()).and(hint.list(8)).unwrap();
        assert_eq!((hint.text(), hint.text()), (Some("Notes\nTwo new"), Some("")));
        let (_, answer) = side.call(ITEM_PATH, PROPERTIES, "Get", "ss", Pack::default().text(ITEM).text("Nonsense"));
        assert_eq!(Message::read(&answer).unwrap().kind, FAILED);

        // A click, with an activation token passed before it.
        side.call(ITEM_PATH, ITEM, "ProvideXdgActivationToken", "s", Pack::default().text("token-1"));
        let (news, answer) = side.call(ITEM_PATH, ITEM, "Activate", "ii", Pack::default().u32(10).u32(20));
        assert!(news && Message::read(&answer).unwrap().kind == ANSWER);
        trial.run();
        assert_eq!(std::mem::take(&mut trial.app.heard), ["clicked"]);
        assert_eq!(side.ran(false).0.token, "token-1");

        // Its menu: the items, at a revision. Access keys are marked as dbusmenu marks them; a
        // disabled item says so.
        let (_, answer) = side.call(MENU_PATH, MENU, "GetLayout", "iias", Pack::default().u32(0).u32(-1i32 as u32).open(4).close());
        let (revision, rows) = rows_in(&answer);
        assert_eq!((revision, rows), (0, vec!["1 _Show 1".to_string(), "2 Quiet 1 checkmark 0".into(), "3 -".into(), "4 Save__as 0".into()]));
        // The check item is clicked: the app hears, and the panel is told the menu changed.
        let (news, _) = side.call(MENU_PATH, MENU, "Event", "isvu", Pack::default().u32(2).text("clicked").kinds("i").u32(0).u32(0));
        assert!(news);
        trial.run();
        assert_eq!((std::mem::take(&mut trial.app.heard), trial.app.quiet), (vec!["quiet"], true));
        let (_, said) = side.ran(false);
        assert_eq!(names(&said), ["LayoutUpdated"]);
        let mut updated = body(&said[0].1);
        assert_eq!((updated.u32(), updated.u32()), (Some(1), Some(0)));
        let (_, answer) = side.call(MENU_PATH, MENU, "GetLayout", "iias", Pack::default().u32(0).u32(-1i32 as u32).open(4).close());
        assert_eq!(rows_in(&answer).1[1], "2 Quiet 1 checkmark 1");
        // A disabled item, a separator, and a missing item are not clicked.
        for id in [3, 4, 9] {
            let (news, _) = side.call(MENU_PATH, MENU, "Event", "isvu", Pack::default().u32(id).text("clicked").kinds("i").u32(0).u32(0));
            assert!(!news, "row {id}");
        }
        let (news, _) = side.call(
            MENU_PATH,
            MENU,
            "EventGroup",
            "a(isvu)",
            Pack::default().open(8).pad(8).u32(1).text("hovered").kinds("i").u32(0).u32(0).pad(8).u32(1).text("clicked").kinds("i").u32(0).u32(0).close(),
        );
        assert!(news, "the second of two at once was a click on the first row");

        // A new tooltip is signalled; so is a newly appeared watcher.
        trial.app.hint = "Notes\nThree new";
        trial.run();
        assert_eq!(names(&side.ran(false).1), ["NewToolTip", "NewTitle"]);
        let (news, said) = side.comes(NEWS, BUS, [BUS_PATH, BUS, "NameOwnerChanged", "sss"], Pack::default().text(WATCHER).text("").text(":1.9"));
        assert!(!news);
        assert_eq!(said.iter().map(|bytes| Message::read(bytes).unwrap().member.to_string()).collect::<Vec<_>>(), ["RegisterStatusNotifierItem"]);
        // Not shown, it leaves the bus, and the watcher sees it go.
        trial.app.shown = false;
        trial.run();
        let (_, said) = side.ran(false);
        assert_eq!(body(called(&said, "ReleaseName")).text(), Some(name.as_str()));
    }

    #[test]
    fn a_notification_is_asked_of_the_server_and_its_click_comes_back() {
        let mut trial = Trial::new((Vec::new(), Vec::new()), |(sent, clicked): &mut (Vec<crate::Notification>, Vec<crate::Notification>), ui| {
            if sent.is_empty() {
                sent.push(ui.system_notification("Saved", "notes.txt is on the disk", Some((1, &[0xff11_2233]))));
            }
            clicked.extend(ui.notification_clicked());
        });
        let mut side = Side::new();
        let (_, said) = side.ran(false);
        let notify = called(&said, "Notify");
        let mut asked = body(notify);
        let firsts = (asked.text().map(str::to_string), asked.u32(), asked.text(), asked.text(), asked.text());
        assert_eq!(firsts, (Some(program()), Some(0), Some(""), Some("Saved"), Some("notes.txt is on the disk")));
        let end = asked.list(4).unwrap();
        let mut actions = Vec::new();
        while asked.at < end {
            actions.push(asked.text().unwrap());
        }
        assert_eq!(actions, ["default", "Open"]);
        let hints = entries(asked);
        let (_, kinds, mut picture) = hints.iter().find(|(name, ..)| name == "image-data").cloned().unwrap();
        picture.take(8, 0).unwrap();
        let numbers: Vec<u32> = (0..6).map(|_| picture.u32().unwrap()).collect();
        picture.list(1).unwrap();
        assert_eq!((kinds.as_str(), numbers, picture.take(1, 4).unwrap()), ("(iiibiiay)", vec![1, 1, 4, 1, 8, 4], &[0x11, 0x22, 0x33, 0xff][..]));
        // The server replies with its id; then the user clicks it, the server passing a token
        // first.
        side.answers(":1.5", Message::read(notify).unwrap().serial, "u", Pack::default().u32(42));
        let news = |side: &mut Side, member: &str, kinds: &str, values: &Pack| side.comes(NEWS, ":1.5", [NOTIFICATIONS_PATH, NOTIFICATIONS, member, kinds], values).0;
        assert!(!news(&mut side, "ActivationToken", "us", Pack::default().u32(42).text("token-2")));
        assert!(news(&mut side, "ActionInvoked", "us", Pack::default().u32(42).text("default")));
        assert!(!news(&mut side, "ActionInvoked", "us", Pack::default().u32(43).text("default")), "another program's");
        trial.run();
        assert_eq!(trial.app.1, trial.app.0);
        assert_eq!(side.ran(false).0.token, "token-2");
        // Closed, it is forgotten.
        assert!(!news(&mut side, "NotificationClosed", "uu", Pack::default().u32(42).u32(2)));
        assert!(!news(&mut side, "ActionInvoked", "us", Pack::default().u32(42).text("default")));
    }

    /// The shortcut tests' app: the keys it wants, what it heard, and which were refused.
    #[derive(Default)]
    struct Keyed {
        both: bool,
        pressed: Vec<&'static str>,
        taken: Vec<&'static str>,
    }

    fn keyed(app: &mut Keyed, ui: &mut Ui) {
        let keys: &[&'static str] = if app.both { &["Ctrl+Alt+N", "Super+F12"] } else { &["Super+F12"] };
        for keys in keys {
            if ui.hotkey(keys) {
                app.pressed.push(keys);
            }
        }
        app.taken = keys.iter().copied().filter(|keys| ui.hotkey_taken(keys)).collect();
    }

    /// The portal's `Response` to a request, on its request path.
    fn responds(side: &mut Side, token: &str, results: &Pack) -> (bool, Vec<Vec<u8>>) {
        let path = format!("/org/freedesktop/portal/desktop/request/1_42/{token}");
        side.comes(NEWS, ":1.3", [&path, REQUEST, "Response", "ua{sv}"], results)
    }

    #[test]
    fn under_wayland_keys_of_the_whole_system_are_bound_by_the_portal() {
        let mut trial = Trial::new(Keyed { both: true, ..Keyed::default() }, keyed);
        let mut side = Side::new();
        // A session is requested, with a request token and a session token.
        let (_, said) = side.ran(true);
        let tokens: Vec<String> = entries(body(called(&said, "CreateSession"))).iter().map(|(name, _, value)| format!("{name}={}", value.clone().text().unwrap())).collect();
        assert_eq!(tokens, ["handle_token=taut_keys1", "session_handle_token=taut_keys1"]);
        // Created, the shortcuts are bound in it, each by id with the app's key, over the first
        // window.
        let session = "/org/freedesktop/portal/desktop/session/1_42/taut_keys1";
        let (_, said) = responds(&mut side, "taut_keys1", Pack::default().u32(0).open(8).pad(8).text("session_handle").kinds("s").text(session).close());
        let bind = said.iter().find(|bytes| Message::read(bytes).unwrap().member == "BindShortcuts").expect("the keys are bound");
        let mut asked = body(bind);
        assert_eq!(asked.text(), Some(session));
        let (end, mut keys) = (asked.list(8).unwrap(), Vec::new());
        while asked.at < end {
            let id = asked.take(8, 0).and(asked.text()).unwrap();
            let said = entries(asked);
            keys.push(format!("{id} {} {}", text_of(&said, "description"), text_of(&said, "preferred_trigger")));
            asked.skip("a{sv}").unwrap();
        }
        assert_eq!(keys, ["1 CTRL+ALT+n CTRL+ALT+n", "2 LOGO+F12 LOGO+F12"]);
        assert_eq!((asked.text(), text_of(&entries(asked), "handle_token")), (Some("wayland:first"), "taut_keys2".to_string()));
        // The portal bound the first and not the second: the app hears it was refused.
        let mut bound = Pack::default();
        bound.u32(0).open(8).pad(8).text("shortcuts").kinds("a(sa{sv})").open(8).pad(8).text("1").open(8).pad(8).text("trigger_description").kinds("s").text("Ctrl+Alt+N").close().close().close();
        assert!(responds(&mut side, "taut_keys2", &bound).0);
        trial.run();
        assert_eq!(trial.app.taken, ["Super+F12"]);
        // The first is pressed, with an activation token.
        let mut pressed = Pack::default();
        pressed.text(session).text("1").pad(8).bytes.extend(5u64.to_le_bytes());
        pressed.open(8).pad(8).text("activation_token").kinds("s").text("token-3").close();
        assert!(side.comes(NEWS, ":1.3", [PORTAL_PATH, SHORTCUTS, "Activated", "osta{sv}"], &pressed).0);
        assert!(!side.comes(NEWS, ":1.3", [PORTAL_PATH, SHORTCUTS, "Activated", "osta{sv}"], Pack::default().text("/another").text("1").pad(8).u32(0).u32(0).open(8).close()).0);
        trial.run();
        assert_eq!(trial.app.pressed, ["Ctrl+Alt+N"]);
        assert_eq!(side.ran(true).0.token, "token-3");
        // A bound shortcut no longer wanted closes the session and binds the rest in a new one.
        // (A refused one is no reason to.)
        trial.app.both = false;
        trial.run();
        let (_, said) = side.ran(true);
        assert_eq!(names(&said), ["Close", "CreateSession"]);
        assert_eq!(body(called(&said, "Close")).text(), None, "a session is closed by its own path");
        assert_eq!(Message::read(called(&said, "Close")).unwrap().path, session);
        // A portal without the interface refuses all, and is not asked again.
        let create = Message::read(called(&said, "CreateSession")).unwrap().serial;
        side.peer.send(&packed(
            FAILED,
            601,
            &[(4, "s", "org.freedesktop.DBus.Error.UnknownMethod"), (5, "u", &create.to_string()), (7, "s", ":1.3"), (8, "g", "s")],
            Pack::default().text("No such method"),
        ));
        side.bus.turn(&mut |bus, message| _ = heard(bus, message));
        trial.run();
        assert_eq!(trial.app.taken, ["Super+F12"]);
        trial.app.both = true;
        trial.run();
        assert!(side.ran(true).1.is_empty());
        trial.run();
        assert_eq!(trial.app.taken, ["Ctrl+Alt+N", "Super+F12"]);
    }

    #[test]
    fn in_a_sandbox_a_notification_is_asked_of_the_portal_and_its_click_comes_back() {
        let mut side = Side::new();
        let out = Out { number: 5, title: "Saved".into(), text: "notes.txt".into(), picture: None };
        notify_portal(&mut side.bus, &out, Some((1, vec![0xff11_2233])));
        let said = side.peer.said();
        let mut asked = body(called(&said, "AddNotification"));
        assert_eq!(asked.text(), Some("taut5"), "a name of its own, by its number");
        let read = entries(asked);
        let texts = ["title", "body", "default-action"].map(|name| text_of(&read, name));
        assert_eq!(texts, ["Saved", "notes.txt", "default"]);
        // Its icon as PNG, in the portal's form: ("bytes", <ay>).
        let (_, kinds, mut icon) = read.iter().find(|(name, ..)| name == "icon").cloned().unwrap();
        let (kind, of) = (icon.take(8, 0).and(icon.text()), icon.kinds());
        let len = icon.list(1).unwrap() - icon.at;
        let png = icon.take(1, len).unwrap();
        assert_eq!((kinds.as_str(), kind, of, crate::png::decode(png)), ("(sv)", Some("bytes"), Some("ay"), Some((1, vec![0xff11_2233]))));
        // The portal says it was clicked, by its id: the next run hears which.
        let (news, _) = side.comes(NEWS, ":1.3", [PORTAL_PATH, PORTAL_NOTIFICATION, "ActionInvoked", "ssav"], Pack::default().text("taut5").text("default").open(1).close());
        assert!(news && NOTICES.with_borrow(|notices| notices.clicked == [5]));
        let (news, _) = side.comes(NEWS, ":1.3", [PORTAL_PATH, PORTAL_NOTIFICATION, "ActionInvoked", "ssav"], Pack::default().text("another").text("default").open(1).close());
        assert!(!news, "not one of this program's");
    }

    #[test]
    fn rows_are_written_with_the_letter_that_picks_them_marked_as_dbusmenu_marks_it() {
        assert_eq!(["&Open", "Save_as", "Fish && chips", "&Quit\tCtrl+Q", "A&&B&c"].map(underlined), ["_Open", "Save__as", "Fish & chips", "_Quit", "A&B_c"]);
    }

    // ---- on a bus of the test's own, with the desktop played by a program of the test's ----

    use crate::portal::tests::OwnBus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    /// What the desktop stand-in tracks.
    #[derive(Default)]
    struct Played {
        /// Its own pending calls: to read the icon.
        reading: Vec<(u32, String)>,
        /// What it did, in order, for the test to check.
        done: Vec<String>,
    }

    /// A unique bus name as it appears in a request path: without its colon, dots as
    /// underscores.
    fn in_path(name: &str) -> String {
        name.trim_start_matches(':').replace('.', "_")
    }

    /// A string value from an `a{sv}`.
    fn text_in(said: Unpack, name: &str) -> String {
        entries(said).into_iter().find(|(named, ..)| named == name).and_then(|(_, _, mut value)| value.text().map(str::to_string)).unwrap_or_default()
    }

    /// A signal from the stand-in to one bus client.
    fn tell(bus: &mut Bus, to: &str, path: &str, interface: &str, member: &str, kinds: &str, values: &Pack) {
        bus.send(Header { kind: NEWS, path, interface, member, to, kinds, ..Default::default() }, &values.bytes);
    }

    /// What the desktop stand-in does with a message: as the watcher, it registers an icon and
    /// has the panel read it; as the panel, it clicks the menu's first item and the icon once
    /// read; as the notification server, it shows one and has the user click it; as the portal,
    /// it creates a shortcut session, binds them, and has the user press the first.
    fn play(bus: &mut Bus, message: &Message, played: &mut Played) {
        let (mut asked, from) = (message.body, message.sender.to_string());
        match (message.kind, message.member) {
            (CALL, "RegisterStatusNotifierItem") => {
                let item = asked.text().unwrap_or_default().to_string();
                bus.answer(message, Ok(""), &Pack::default());
                let call = bus.call(&item, ITEM_PATH, PROPERTIES, "GetAll", "s", Pack::default().text(ITEM));
                played.reading.push((call, item.clone()));
                played.done.push(format!("registered {}", item.starts_with(ITEM)));
            }
            (ANSWER, _) if played.reading.iter().any(|(call, _)| *call == message.answers) => {
                let at = played.reading.iter().position(|(call, _)| *call == message.answers).unwrap_or(0);
                let (_, item) = played.reading.remove(at);
                played.done.push(format!("read {}", text_of(&entries(asked), "Title")));
                bus.call(&item, MENU_PATH, MENU, "Event", "isvu", Pack::default().u32(1).text("clicked").kinds("i").u32(0).u32(0));
                bus.call(&item, ITEM_PATH, ITEM, "Activate", "ii", Pack::default().u32(5).u32(5));
            }
            (CALL, "Notify") => {
                bus.answer(message, Ok("u"), Pack::default().u32(7));
                tell(bus, "", NOTIFICATIONS_PATH, NOTIFICATIONS, "ActionInvoked", "us", Pack::default().u32(7).text("default"));
                played.done.push("notified".into());
            }
            (CALL, "CreateSession") => {
                let token = text_in(asked, "handle_token");
                let request = format!("/org/freedesktop/portal/desktop/request/{}/{token}", in_path(&from));
                let session = format!("/org/freedesktop/portal/desktop/session/{}/{}", in_path(&from), text_in(asked, "session_handle_token"));
                bus.answer(message, Ok("o"), Pack::default().text(&request));
                tell(bus, &from, &request, REQUEST, "Response", "ua{sv}", Pack::default().u32(0).open(8).pad(8).text("session_handle").kinds("s").text(&session).close());
                played.done.push("session".into());
            }
            (CALL, "BindShortcuts") => {
                let session = asked.text().unwrap_or_default().to_string();
                let end = asked.list(8).unwrap_or_default();
                let first = asked.take(8, 0).and(asked.text()).unwrap_or_default().to_string();
                asked.at = end;
                asked.text();
                let token = text_in(asked, "handle_token");
                let request = format!("/org/freedesktop/portal/desktop/request/{}/{token}", in_path(&from));
                bus.answer(message, Ok("o"), Pack::default().text(&request));
                let mut bound = Pack::default();
                bound.u32(0).open(8).pad(8).text("shortcuts").kinds("a(sa{sv})").open(8).pad(8).text(&first).open(8).close().close().close();
                tell(bus, &from, &request, REQUEST, "Response", "ua{sv}", &bound);
                let mut pressed = Pack::default();
                pressed.text(&session).text(&first).pad(8).bytes.extend(9u64.to_le_bytes());
                pressed.open(8).close();
                tell(bus, &from, PORTAL_PATH, SHORTCUTS, "Activated", "osta{sv}", &pressed);
                played.done.push(format!("bound {first}"));
            }
            _ => {}
        }
    }

    /// The desktop, played by a test thread on the test's own bus: it takes the watcher's,
    /// notification server's and portal's names, says when it has them, and plays them (`play`)
    /// until stopped; then returns what it did.
    fn desktop(address: String, ready: mpsc::Sender<()>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut bus = Bus::open(&address).expect("the test's own bus");
            for name in [WATCHER, NOTIFICATIONS, PORTAL] {
                bus.call(BUS, BUS_PATH, BUS, "RequestName", "su", Pack::default().text(name).u32(4));
            }
            let (mut named, mut played, mut told) = (0, Played::default(), false);
            while !stop.load(Ordering::Relaxed) && !bus.dead {
                bus.turn(&mut |bus, message| {
                    named += (message.member == "NameAcquired") as u32;
                    play(bus, message, &mut played);
                });
                if named >= 3 && !std::mem::replace(&mut told, true) {
                    ready.send(()).unwrap();
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            played.done
        })
    }

    /// What this test's app heard, by name.
    #[derive(Default)]
    struct Whole {
        heard: Vec<&'static str>,
        sent: bool,
    }

    fn whole(app: &mut Whole, ui: &mut Ui) {
        if ui.tray(&[(1, &[0xff00_ff00])], "Notes").clicked {
            app.heard.push("clicked");
        }
        if ui.tray_item("&Show") {
            app.heard.push("picked");
        }
        if ui.hotkey("Ctrl+Alt+N") {
            app.heard.push("key");
        }
        if !std::mem::replace(&mut app.sent, true) {
            ui.system_notification("Saved", "notes.txt", None);
        }
        if ui.notification_clicked().is_some() {
            app.heard.push("notification");
        }
        ui.label("The window");
    }

    #[test]
    fn through_a_real_bus_the_desktop_hears_of_the_icon_the_notification_and_the_keys_and_answers() {
        let Some(own) = OwnBus::start() else { return eprintln!("no dbus-daemon: not tested") };
        let (stop, (ready, wait)) = (Arc::new(AtomicBool::new(false)), mpsc::channel());
        let played = desktop(own.address.clone(), ready, stop.clone());
        wait.recv_timeout(Duration::from_secs(5)).expect("the stand-in has its names");
        // The program, as its Wayland windows run it: a run, its requests passed on, bus
        // messages taken in, the next run when something came.
        let mut trial = Trial::new(Whole::default(), whole);
        let mut bus = Bus::open(&own.address).expect("the test's own bus");
        let since = Instant::now();
        let all = |app: &Whole| ["clicked", "picked", "key", "notification"].iter().all(|what| app.heard.contains(what));
        while since.elapsed() < Duration::from_secs(10) && !all(&trial.app) {
            trial.run();
            ran(Some(&mut bus), true, "");
            bus.turn(&mut |bus, message| _ = heard(bus, message));
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::Relaxed);
        let done = played.join().unwrap();
        let heard = Mutex::new(trial.app.heard.clone());
        heard.lock().unwrap().sort();
        assert_eq!(*heard.lock().unwrap(), ["clicked", "key", "notification", "picked"], "the desktop did: {done:?}");
        assert!(done.contains(&"registered true".to_string()) && done.contains(&"read Notes".to_string()) && done.contains(&"bound 1".to_string()), "{done:?}");
    }
}
