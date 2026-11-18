//! Global hotkeys: a key with Ctrl, Alt, Shift or the logo key that reaches the program while
//! another has focus. The app names the ones it wants every run ([`Ui::hotkey`]): each is
//! registered after the first run naming it, and released after the first run that doesn't. A
//! key the system refuses never fires; [`Ui::hotkey_refused`] says so, to tell the user.
//!
//! - **Windows:** `RegisterHotKey` (in `shell`).
//! - **X11:** a passive grab on the root window (in `x11`). A key another program grabbed is
//!   refused, as is Ctrl+Alt+F1…F12, which X keeps for switching consoles.
//! - **Wayland:** no program can take a key from another, so the portal's `GlobalShortcuts` is
//!   asked (in `freedesktop`); it may show the user the request and let them confirm or choose
//!   another key. A desktop whose portal lacks it gives none.
//!
//! Keys are written as menus write them, case-insensitive: `Ctrl`, `Alt`, `Shift`, and the logo
//! key (`Super`, `Win` or `Logo`), then the key: a letter, a digit, `F1` to `F24`, or a name in
//! `NAMES`.
use crate::ui::Ui;
use std::cell::RefCell;

/// Modifier bits: Ctrl, Alt, Shift, and the logo key (Windows key, Super).
pub(crate) const CTRL: u8 = 1;
pub(crate) const ALT: u8 = 2;
pub(crate) const SHIFT: u8 = 4;
pub(crate) const LOGO: u8 = 8;

/// A key by name: a lower-case letter or digit as itself, F1 to F24 by number, the others by
/// name. (The last four are media keys.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Named {
    Char(u8),
    F(u8),
    Space,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    Pause,
    Print,
    Play,
    Stop,
    Next,
    Previous,
}

/// Key names, besides letters, digits and F1 to F24.
const NAMES: [(&str, Named); 25] = [
    ("space", Named::Space),
    ("enter", Named::Enter),
    ("return", Named::Enter),
    ("tab", Named::Tab),
    ("escape", Named::Escape),
    ("esc", Named::Escape),
    ("backspace", Named::Backspace),
    ("delete", Named::Delete),
    ("del", Named::Delete),
    ("insert", Named::Insert),
    ("home", Named::Home),
    ("end", Named::End),
    ("pageup", Named::PageUp),
    ("pagedown", Named::PageDown),
    ("left", Named::Left),
    ("right", Named::Right),
    ("up", Named::Up),
    ("down", Named::Down),
    ("pause", Named::Pause),
    ("print", Named::Print),
    ("printscreen", Named::Print),
    ("play", Named::Play),
    ("stop", Named::Stop),
    ("next", Named::Next),
    ("previous", Named::Previous),
];

/// A global hotkey: the key and its modifiers (`CTRL`, `ALT`, `SHIFT`, `LOGO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Combo {
    pub key: Named,
    pub modifiers: u8,
}

/// Parses "Ctrl+Alt+N" and the like. `None` if it names no key, or a key with no modifier
/// (which would steal it from typing everywhere: only F1 to F24 and media keys may stand
/// alone).
pub(crate) fn combo(written: &str) -> Option<Combo> {
    let (modifier_words, last) = written.rsplit_once('+').unwrap_or(("", written));
    let mut modifiers = 0;
    for word in modifier_words.split('+').map(str::trim).filter(|word| !word.is_empty()) {
        modifiers |= match word.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => CTRL,
            "alt" => ALT,
            "shift" => SHIFT,
            "super" | "win" | "logo" | "meta" => LOGO,
            _ => return None,
        };
    }
    let name = last.trim().to_ascii_lowercase().replace(' ', "");
    let numbered = name.strip_prefix('f').and_then(|number| number.parse::<u8>().ok()).filter(|number| (1..=24).contains(number));
    let key = match (name.as_bytes(), numbered) {
        (_, Some(number)) => Named::F(number),
        ([letter], None) if letter.is_ascii_alphanumeric() => Named::Char(*letter),
        _ => NAMES.iter().find(|(called, _)| *called == name)?.1,
    };
    let alone = matches!(key, Named::F(_) | Named::Play | Named::Stop | Named::Next | Named::Previous);
    (modifiers != 0 || alone).then_some(Combo { key, modifiers })
}

impl Named {
    /// Its Windows virtual-key code.
    #[cfg(windows)]
    pub(crate) fn virtual_key(self) -> u32 {
        match self {
            Named::Char(letter) => letter.to_ascii_uppercase() as u32,
            Named::F(number) => 0x6F + number as u32,
            Named::Space => 0x20,
            Named::Enter => 0x0D,
            Named::Tab => 0x09,
            Named::Escape => 0x1B,
            Named::Backspace => 0x08,
            Named::Delete => 0x2E,
            Named::Insert => 0x2D,
            Named::Home => 0x24,
            Named::End => 0x23,
            Named::PageUp => 0x21,
            Named::PageDown => 0x22,
            Named::Left => 0x25,
            Named::Up => 0x26,
            Named::Right => 0x27,
            Named::Down => 0x28,
            Named::Pause => 0x13,
            Named::Print => 0x2C,
            Named::Play => 0xB3,
            Named::Stop => 0xB2,
            Named::Next => 0xB0,
            Named::Previous => 0xB1,
        }
    }

    /// Its X keysym, and its name, which is how the portal is told of a key.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) fn keysym(self) -> (u32, String) {
        let named = |number: u32, name: &str| (number, name.to_string());
        match self {
            Named::Char(letter) => (letter as u32, (letter as char).to_string()),
            Named::F(number) => (0xFFBD + number as u32, format!("F{number}")),
            Named::Space => named(0x20, "space"),
            Named::Enter => named(0xFF0D, "Return"),
            Named::Tab => named(0xFF09, "Tab"),
            Named::Escape => named(0xFF1B, "Escape"),
            Named::Backspace => named(0xFF08, "BackSpace"),
            Named::Delete => named(0xFFFF, "Delete"),
            Named::Insert => named(0xFF63, "Insert"),
            Named::Home => named(0xFF50, "Home"),
            Named::End => named(0xFF57, "End"),
            Named::PageUp => named(0xFF55, "Prior"),
            Named::PageDown => named(0xFF56, "Next"),
            Named::Left => named(0xFF51, "Left"),
            Named::Up => named(0xFF52, "Up"),
            Named::Right => named(0xFF53, "Right"),
            Named::Down => named(0xFF54, "Down"),
            Named::Pause => named(0xFF13, "Pause"),
            Named::Print => named(0xFF61, "Print"),
            Named::Play => named(0x1008_FF14, "XF86AudioPlay"),
            Named::Stop => named(0x1008_FF15, "XF86AudioStop"),
            Named::Previous => named(0x1008_FF16, "XF86AudioPrev"),
            Named::Next => named(0x1008_FF17, "XF86AudioNext"),
        }
    }
}

impl Combo {
    /// The key as the portal takes it: modifiers in capitals, then the key's name, joined by
    /// `+` ("CTRL+ALT+n", "LOGO+F12").
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) fn trigger(self) -> String {
        let mut written = String::new();
        for (bit, word) in [(CTRL, "CTRL+"), (ALT, "ALT+"), (SHIFT, "SHIFT+"), (LOGO, "LOGO+")] {
            if self.modifiers & bit != 0 {
                written.push_str(word);
            }
        }
        written + &self.key.keysym().1
    }
}

/// A wanted key's registration state: to register, pending, registered, or refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Registration {
    New,
    // Only the portal answers later.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    Pending,
    Registered,
    Refused,
}

/// A key the app wants, and its state.
pub(crate) struct Wanted {
    pub combo: Combo,
    /// Its own id, for the system to report it by (Windows), or the portal (as text).
    pub number: u32,
    pub state: Registration,
    /// Named this run; pressed since a run last asked.
    pub requested: bool,
    pub pressed: bool,
}

/// The global hotkeys the app wants, on this thread.
#[derive(Default)]
pub(crate) struct Hotkeys {
    pub keys: Vec<Wanted>,
    /// How many were ever wanted: new ones are numbered from this.
    pub count: u32,
}

thread_local! {
    pub(crate) static HOTKEYS: RefCell<Hotkeys> = RefCell::new(Hotkeys::default());
}

impl Hotkeys {
    /// After a run: keys not named are dropped (released through `give_back`, if held), and
    /// those named must be named again next run.
    pub(crate) fn settle(&mut self, mut give_back: impl FnMut(&Wanted)) {
        self.keys.iter().filter(|wanted| !wanted.requested).for_each(&mut give_back);
        self.keys.retain(|wanted| wanted.requested);
        self.keys.iter_mut().for_each(|wanted| (wanted.requested, wanted.pressed) = (false, false));
    }

    /// A key was pressed, as the system reports it by id: the next run that asks hears it.
    /// Returns whether it is one of the app's.
    pub(crate) fn pressed(&mut self, number: u32) -> bool {
        let found = self.keys.iter_mut().find(|wanted| wanted.number == number && wanted.state == Registration::Registered);
        found.map(|wanted| wanted.pressed = true).is_some()
    }
}

/// Global hotkeys, heard while another program has focus.
impl Ui {
    /// A global hotkey: `true` once per press, whichever program has focus. Call it every run
    /// while the app wants it; it is registered after the first run calling it, and released
    /// after the first that doesn't. Written as menus write keys ("Ctrl+Alt+N", "Super+F12"),
    /// with a modifier unless it is F1 to F24 or a media key. (Only in windows the library
    /// runs, `taut::run`. Under Wayland the portal is asked, which may ask the user first.)
    pub fn hotkey(&mut self, keys: &str) -> bool {
        let Some(combo) = combo(keys) else { return false };
        crate::platform::hand_desktop();
        let pressed = HOTKEYS.with_borrow_mut(|hotkeys| {
            let Some(wanted) = hotkeys.keys.iter_mut().find(|wanted| wanted.combo == combo) else {
                hotkeys.count += 1;
                let number = hotkeys.count;
                hotkeys.keys.push(Wanted { combo, number, state: Registration::New, requested: true, pressed: false });
                return false;
            };
            wanted.requested = true;
            std::mem::take(&mut wanted.pressed)
        });
        self.again |= pressed;
        pressed
    }

    /// Whether the system refused a global hotkey asked for with [`hotkey`](Self::hotkey):
    /// another program has it, the user declined, or this system has no global hotkeys (text
    /// naming no key is never held either). `false` while the system has not answered.
    pub fn hotkey_refused(&self, keys: &str) -> bool {
        let Some(combo) = combo(keys) else { return true };
        HOTKEYS.with_borrow(|hotkeys| hotkeys.keys.iter().any(|wanted| wanted.combo == combo && wanted.state == Registration::Refused))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_of_the_whole_system_are_read_the_way_a_menu_writes_them() {
        let both = CTRL | ALT;
        assert_eq!(combo("Ctrl+Alt+N"), Some(Combo { key: Named::Char(b'n'), modifiers: both }));
        assert_eq!(combo("alt + control + n"), combo("Ctrl+Alt+N"));
        assert_eq!(combo("Super+Shift+F12"), Some(Combo { key: Named::F(12), modifiers: LOGO | SHIFT }));
        assert_eq!(combo("Win+F24").map(|combo| combo.modifiers), Some(LOGO));
        assert_eq!(combo("Ctrl+Alt+Shift+Win+F24"), Some(Combo { key: Named::F(24), modifiers: CTRL | ALT | SHIFT | LOGO }));
        assert_eq!(combo("Ctrl+Page Down"), Some(Combo { key: Named::PageDown, modifiers: CTRL }));
        assert_eq!(combo("Ctrl+7"), Some(Combo { key: Named::Char(b'7'), modifiers: CTRL }));
        // F1 to F24 and media keys may stand alone; nothing else may.
        assert_eq!(combo("F13"), Some(Combo { key: Named::F(13), modifiers: 0 }));
        assert_eq!(combo("Play"), Some(Combo { key: Named::Play, modifiers: 0 }));
        assert_eq!([combo("N"), combo("Space"), combo("Ctrl+F25"), combo("Hyper+N"), combo("Ctrl+"), combo("Ctrl+Save")], [None; 6]);
    }

    #[cfg(windows)]
    #[test]
    fn keys_have_the_numbers_windows_knows_them_by() {
        let numbers = |written: &str| combo(written).unwrap().key.virtual_key();
        // As in WinUser.h: VK_F1 0x70, VK_F24 0x87, 'A' 0x41, VK_NEXT 0x22, VK_MEDIA_PLAY_PAUSE 0xB3.
        assert_eq!(["F1", "F24", "Ctrl+a", "Ctrl+9", "Ctrl+PageDown", "Play", "Previous"].map(numbers), [0x70, 0x87, 0x41, 0x39, 0x22, 0xB3, 0xB1]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn keys_have_the_numbers_and_names_x_and_the_portal_know_them_by() {
        let numbers = |written: &str| combo(written).unwrap().key.keysym().0;
        // As in X11/keysymdef.h and XF86keysym.h: XK_F1 0xffbe, XK_F24 0xffd5, XK_Next 0xff56.
        assert_eq!(["F1", "F24", "Ctrl+a", "Ctrl+PageDown", "Play", "Next"].map(numbers), [0xFFBE, 0xFFD5, 0x61, 0xFF56, 0x1008_FF14, 0x1008_FF17]);
        let triggers = |written: &str| combo(written).unwrap().trigger();
        assert_eq!(["Ctrl+Alt+N", "Super+F12", "Shift+Ctrl+Enter", "Ctrl+Space"].map(triggers), ["CTRL+ALT+n", "LOGO+F12", "CTRL+SHIFT+Return", "CTRL+space"]);
    }

    #[test]
    fn a_key_is_wanted_while_runs_say_it_and_pressed_once() {
        HOTKEYS.set(Hotkeys::default());
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        assert!(!ui.hotkey("Ctrl+Alt+N") && !ui.hotkey("Ctrl+Alt+N") && !ui.hotkey("Nonsense"));
        let number = HOTKEYS.with_borrow(|hotkeys| {
            assert_eq!(hotkeys.keys.len(), 1, "one key, however often it is said");
            hotkeys.keys[0].number
        });
        // Not held yet, it is not pressed; held, it fires once.
        HOTKEYS.with_borrow_mut(|hotkeys| assert!(!hotkeys.pressed(number)));
        HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.keys[0].state = Registration::Registered);
        HOTKEYS.with_borrow_mut(|hotkeys| assert!(hotkeys.pressed(number) && !hotkeys.pressed(number + 1)));
        assert!(ui.hotkey("ctrl+alt+n") && !ui.hotkey("Ctrl+Alt+N"));
        // A run that doesn't name it releases it to the system.
        let mut given = Vec::new();
        HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.settle(|wanted| given.push(wanted.number)));
        assert!(given.is_empty(), "said in the run, it is kept");
        HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.settle(|wanted| given.push(wanted.number)));
        assert_eq!(given, [number]);
        assert!(HOTKEYS.with_borrow(|hotkeys| hotkeys.keys.is_empty()));
        // A refused one says so.
        ui.hotkey("Ctrl+Alt+M");
        assert!(!ui.hotkey_refused("Ctrl+Alt+M"));
        HOTKEYS.with_borrow_mut(|hotkeys| hotkeys.keys[0].state = Registration::Refused);
        assert!(ui.hotkey_refused("Ctrl+Alt+M") && ui.hotkey_refused("Nonsense") && !ui.hotkey_refused("Ctrl+Alt+L"));
    }
}
