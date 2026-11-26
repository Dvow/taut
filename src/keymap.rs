//! Wayland keyboard input: key identity and typed text. The compositor sends the keymap as XKB
//! text, then only key codes and modifier state; libxkbcommon (on every Wayland desktop, loaded
//! by name with the first window) interprets them. Dead keys and compose sequences use the
//! user's language's compose table.
use crate::dynamic_library::library;
use crate::event::{Key, Mods};
use crate::x11::key;
use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::time::Duration;

library!(Xkb, ["libxkbcommon.so.0", "libxkbcommon.so"], {
    fn xkb_context_new(c_int) -> *mut c_void;
    fn xkb_context_unref(*mut c_void);
    fn xkb_keymap_new_from_string(*mut c_void, *const c_char, c_int, c_int) -> *mut c_void;
    fn xkb_keymap_unref(*mut c_void);
    fn xkb_keymap_key_repeats(*mut c_void, u32) -> c_int;
    fn xkb_state_new(*mut c_void) -> *mut c_void;
    fn xkb_state_unref(*mut c_void);
    fn xkb_state_update_mask(*mut c_void, u32, u32, u32, u32, u32, u32) -> c_int;
    fn xkb_state_key_get_one_sym(*mut c_void, u32) -> u32;
    fn xkb_state_key_get_utf8(*mut c_void, u32, *mut c_char, usize) -> c_int;
    fn xkb_state_mod_name_is_active(*mut c_void, *const c_char, c_int) -> c_int;
    fn xkb_compose_table_new_from_locale(*mut c_void, *const c_char, c_int) -> *mut c_void;
    fn xkb_compose_table_unref(*mut c_void);
    fn xkb_compose_state_new(*mut c_void, c_int) -> *mut c_void;
    fn xkb_compose_state_unref(*mut c_void);
    fn xkb_compose_state_feed(*mut c_void, u32) -> c_int;
    fn xkb_compose_state_get_status(*mut c_void) -> c_int;
    fn xkb_compose_state_get_utf8(*mut c_void, *mut c_char, usize) -> c_int;
    fn xkb_compose_state_reset(*mut c_void);
});

/// XKB compose status: composing (1), composed (2), cancelled (3); 0 is nothing.
const COMPOSING: c_int = 1;
const COMPOSED: c_int = 2;
const CANCELLED: c_int = 3;

/// XKB state component: effective modifiers, whether depressed, latched or locked (1 << 3).
const IN_EFFECT: c_int = 1 << 3;

/// The keyboard: keymap, current state, and compose state.
pub(crate) struct Keyboard {
    xkb: Xkb,
    context: *mut c_void,
    map: *mut c_void,
    state: *mut c_void,
    /// The language's compose table, and the sequence in progress. (Null if the language has
    /// none.)
    table: *mut c_void,
    composing: *mut c_void,
    /// Key repeat delay and interval; `None` if keys don't repeat (as the compositor says).
    pub(crate) repeat: Option<(Duration, Duration)>,
}

/// The user's language for its compose table: `LC_ALL`, else `LC_CTYPE`, else `LANG` ("C" if
/// none).
fn language() -> CString {
    let said = ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    CString::new(said.unwrap_or_else(|| "C".into())).unwrap_or_default()
}

impl Keyboard {
    /// The keyboard before the compositor sends a keymap. `None` without libxkbcommon.
    pub(crate) fn new() -> Option<Self> {
        let xkb = Xkb::open()?;
        // SAFETY: libxkbcommon's documented calls to create a context and load a compose table;
        // either may return null, which is checked.
        unsafe {
            let context = (xkb.xkb_context_new)(0);
            if context.is_null() {
                return None;
            }
            let table = (xkb.xkb_compose_table_new_from_locale)(context, language().as_ptr(), 0);
            let composing = if table.is_null() { std::ptr::null_mut() } else { (xkb.xkb_compose_state_new)(table, 0) };
            let repeat = Some((Duration::from_millis(600), Duration::from_millis(40)));
            Some(Keyboard { xkb, context, map: std::ptr::null_mut(), state: std::ptr::null_mut(), table, composing, repeat })
        }
    }

    /// Takes the compositor's keymap text. Returns whether it parsed.
    pub(crate) fn take_map(&mut self, text: &[u8]) -> bool {
        let text = CString::new(text.split(|byte| *byte == 0).next().unwrap_or_default()).unwrap_or_default();
        // SAFETY: the text is NUL-terminated; it is parsed as the one keymap format (1).
        let map = unsafe { (self.xkb.xkb_keymap_new_from_string)(self.context, text.as_ptr(), 1, 0) };
        self.with_map(map)
    }

    /// The keyboard of an XKB layout (`us`, `us(intl)`), with the keymap a compositor would
    /// send: for tests to type on.
    #[cfg(test)]
    pub(crate) fn named(layout: &str) -> Option<Self> {
        let mut keyboard = Keyboard::new()?;
        let map = format!(
            "xkb_keymap {{ xkb_keycodes {{ include \"evdev+aliases(qwerty)\" }}; xkb_types {{ include \"complete\" }}; xkb_compat {{ include \"complete\" }}; xkb_symbols {{ include \"pc+{layout}+inet(evdev)\" }}; }};"
        );
        keyboard.take_map(map.as_bytes()).then_some(keyboard)
    }

    /// Switches to a new keymap, if any: the old one and its state are freed.
    fn with_map(&mut self, map: *mut c_void) -> bool {
        if map.is_null() {
            return false;
        }
        // SAFETY: the old keymap and state were made here and nothing of them is kept; the new
        // state is made on the new keymap.
        unsafe {
            self.drop_map();
            (self.map, self.state) = (map, (self.xkb.xkb_state_new)(map));
        }
        !self.state.is_null()
    }

    /// Frees the keymap and its state.
    ///
    /// # Safety
    /// Neither is used afterwards, until new ones are made.
    unsafe fn drop_map(&mut self) {
        // SAFETY: each was made by libxkbcommon and is freed once.
        unsafe {
            if !self.state.is_null() {
                (self.xkb.xkb_state_unref)(self.state);
            }
            if !self.map.is_null() {
                (self.xkb.xkb_keymap_unref)(self.map);
            }
        }
        (self.map, self.state) = (std::ptr::null_mut(), std::ptr::null_mut());
    }

    /// The compositor's modifier state: depressed, latched, locked, and the layout group.
    pub(crate) fn hold(&mut self, held: u32, latched: u32, locked: u32, group: u32) {
        if !self.state.is_null() {
            // SAFETY: the state belongs to the keymap.
            unsafe { (self.xkb.xkb_state_update_mask)(self.state, held, latched, locked, 0, 0, group) };
        }
    }

    /// Which of Ctrl, Shift and Alt are in effect.
    pub(crate) fn held(&self) -> Mods {
        if self.state.is_null() {
            return Mods::NONE;
        }
        // SAFETY: the state belongs to the keymap, and the names are NUL-terminated.
        let on = |name: &CStr| unsafe { (self.xkb.xkb_state_mod_name_is_active)(self.state, name.as_ptr(), IN_EFFECT) } > 0;
        Mods { ctrl: on(c"Control"), shift: on(c"Shift"), alt: on(c"Mod1") }
    }

    /// A key went down (by compositor key code): the key as the UI knows keys, and what it
    /// types. A key in a compose sequence types nothing until the sequence completes, then the
    /// character.
    pub(crate) fn press(&mut self, code: u32) -> (Option<Key>, String) {
        if self.state.is_null() {
            return (None, String::new());
        }
        // XKB key codes are evdev codes plus 8.
        let code = code + 8;
        // SAFETY: the state belongs to the keymap.
        let sym = unsafe { (self.xkb.xkb_state_key_get_one_sym)(self.state, code) };
        let named = key(sym as c_ulong);
        (named, self.typed(code, sym))
    }

    /// A key by compositor key code, as the UI knows keys, typing nothing: for a released key.
    pub(crate) fn which(&self, code: u32) -> Option<Key> {
        if self.state.is_null() {
            return None;
        }
        // SAFETY: the state belongs to the keymap.
        let sym = unsafe { (self.xkb.xkb_state_key_get_one_sym)(self.state, code + 8) };
        key(sym as c_ulong)
    }

    /// What a key types, by code and keysym: combined with earlier keys if in a compose
    /// sequence.
    fn typed(&mut self, code: u32, sym: u32) -> String {
        let (status, mut room) = (self.compose(sym), [0 as c_char; 64]);
        // SAFETY: XKB writes at most the room it is told of and returns the length it needed;
        // the compose state exists whenever a key produced a sequence.
        let len = unsafe {
            match status {
                COMPOSING | CANCELLED => 0,
                COMPOSED => (self.xkb.xkb_compose_state_get_utf8)(self.composing, room.as_mut_ptr(), room.len()),
                _ => (self.xkb.xkb_state_key_get_utf8)(self.state, code, room.as_mut_ptr(), room.len()),
            }
        };
        if matches!(status, COMPOSED | CANCELLED) {
            // SAFETY: as above. (The next key starts afresh.)
            unsafe { (self.xkb.xkb_compose_state_reset)(self.composing) };
        }
        let bytes: Vec<u8> = room[..(len.max(0) as usize).min(room.len() - 1)].iter().map(|byte| *byte as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Feeds a keysym to the compose state; returns its status (0: the key is in no sequence).
    fn compose(&mut self, sym: u32) -> c_int {
        if self.composing.is_null() {
            return 0;
        }
        // SAFETY: the compose state was made for this keyboard. (0: the key was ignored, as one
        // in no sequence, such as Shift.)
        unsafe {
            if (self.xkb.xkb_compose_state_feed)(self.composing, sym) == 0 {
                return 0;
            }
            (self.xkb.xkb_compose_state_get_status)(self.composing)
        }
    }

    /// Whether a key repeats while held: letters do, Shift doesn't.
    pub(crate) fn repeats(&self, code: u32) -> bool {
        // SAFETY: the keymap is the keyboard's own.
        !self.map.is_null() && unsafe { (self.xkb.xkb_keymap_key_repeats)(self.map, code + 8) } != 0
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        // SAFETY: everything was made by libxkbcommon for this keyboard and is freed once; the
        // library itself closes after this, when `xkb` drops.
        unsafe {
            self.drop_map();
            if !self.composing.is_null() {
                (self.xkb.xkb_compose_state_unref)(self.composing);
            }
            if !self.table.is_null() {
                (self.xkb.xkb_compose_table_unref)(self.table);
            }
            (self.xkb.xkb_context_unref)(self.context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evdev codes of some keys (as the compositor passes them).
    const A: u32 = 30;
    const E: u32 = 18;
    const APOSTROPHE: u32 = 40;
    const ENTER: u32 = 28;
    const LEFT_SHIFT: u32 = 42;
    const F2: u32 = 60;

    #[test]
    fn a_key_is_named_and_types_what_the_map_says() {
        let Some(mut keyboard) = Keyboard::named("us") else { return eprintln!("no libxkbcommon: not tested") };
        assert_eq!(keyboard.press(A), (Some(Key::Char('a')), "a".into()));
        assert_eq!(keyboard.press(ENTER).0, Some(Key::Enter));
        assert_eq!(keyboard.press(F2), (Some(Key::F(2)), String::new()));
        assert_eq!(keyboard.held(), Mods::NONE);
        // With Shift (XKB's first modifier) the key types a capital, and is still the same key
        // for shortcuts.
        keyboard.hold(1, 0, 0, 0);
        assert_eq!(keyboard.held(), Mods::SHIFT);
        assert_eq!(keyboard.press(A), (Some(Key::Char('a')), "A".into()));
        // Ctrl: the modifiers in effect are reported; keeping the typed character out of text
        // is the host's job.
        keyboard.hold(4, 0, 0, 0);
        assert_eq!(keyboard.held(), Mods::CTRL);
        keyboard.hold(0, 0, 0, 0);
        assert!(keyboard.repeats(A) && !keyboard.repeats(LEFT_SHIFT), "letters go on while held; Shift does not");
    }

    #[test]
    fn an_accent_waits_for_its_letter_and_the_two_make_one_character() {
        let Some(mut keyboard) = Keyboard::named("us(intl)") else { return eprintln!("no libxkbcommon: not tested") };
        if keyboard.composing.is_null() {
            return eprintln!("no compose table for the language: not tested");
        }
        // On the international layout the apostrophe is a dead key.
        assert_eq!(keyboard.press(APOSTROPHE), (None, String::new()));
        assert_eq!(keyboard.press(E).1, "é");
        assert_eq!(keyboard.press(E).1, "e", "after the character, keys type as ever");
    }

    /// libxkbcommon's log level for critical messages only (`XKB_LOG_LEVEL_CRITICAL`); a map
    /// it cannot parse is logged at the level above it, as an error.
    const CRITICAL: c_int = 10;

    /// Runs `read` with libxkbcommon told to log only messages of `level` or worse, then sets
    /// the level back to what it was. (It writes what it cannot parse to stderr: a test that
    /// hands it nonsense on purpose keeps that out of the test's output.)
    fn logging_only<T>(keyboard: &mut Keyboard, level: c_int, read: impl FnOnce(&mut Keyboard) -> T) -> T {
        type Get = unsafe extern "C" fn(*mut c_void) -> c_int;
        type Set = unsafe extern "C" fn(*mut c_void, c_int);
        let xkb = &keyboard.xkb.library;
        let (Some(get), Some(set)) = (xkb.symbol(c"xkb_context_get_log_level"), xkb.symbol(c"xkb_context_set_log_level")) else { return read(keyboard) };
        // SAFETY: both are libxkbcommon's functions, declared as it documents them, and called
        // on the keyboard's own context, which outlives this.
        let (get, set) = unsafe { (std::mem::transmute::<*const c_void, Get>(get), std::mem::transmute::<*const c_void, Set>(set)) };
        // SAFETY: as above.
        let was = unsafe { get(keyboard.context) };
        // SAFETY: as above.
        unsafe { set(keyboard.context, level) };
        let read = read(keyboard);
        // SAFETY: as above.
        unsafe { set(keyboard.context, was) };
        read
    }

    #[test]
    fn a_map_handed_over_as_text_is_read() {
        let Some(mut keyboard) = Keyboard::new() else { return eprintln!("no libxkbcommon: not tested") };
        assert!(!logging_only(&mut keyboard, CRITICAL, |keyboard| keyboard.take_map(b"not a map\0")), "nonsense is no map");
        assert_eq!(keyboard.press(A), (None, String::new()), "with no map, keys mean nothing");
        let map = b"xkb_keymap { xkb_keycodes { <AC01> = 38; }; xkb_types { include \"basic\" }; xkb_compat { }; xkb_symbols { key <AC01> { [ q, Q ] }; }; };\0";
        assert!(keyboard.take_map(map));
        assert_eq!(keyboard.press(A), (Some(Key::Char('q')), "q".into()), "the key on the map as given, whatever it is on other keyboards");
    }
}
