//! What a browser's input events become: keys by their DOM names, held modifiers, wheel
//! distances, and pointers (mouse, finger, pen). Plain data in, [`Event`]s out, so it is tested
//! on every system; `web_host` hands it what the loader (`web.js`) reads from each DOM event.
use crate::event::{Button, Event, Key, Mods};
use crate::geom::Point;
use crate::touch::{Stylus, Touch, TouchPhase};

/// Modifier bits as the loader sends them: Control or Command, Shift, Alt (Option).
pub(crate) const CTRL: u32 = 1;
pub(crate) const SHIFT: u32 = 2;
pub(crate) const ALT: u32 = 4;

/// The scroll distance of a wheel notch, in units: one "line" of a wheel that counts lines
/// (as on Linux).
pub(crate) const NOTCH: f32 = 48.0;

/// The modifiers in the loader's bits.
pub(crate) fn mods(bits: u32) -> Mods {
    Mods { ctrl: bits & CTRL != 0, shift: bits & SHIFT != 0, alt: bits & ALT != 0 }
}

/// The key a `keydown` names: by `key` for keys with names, else the letter it types; a letter
/// typed with Control or Alt is taken from `code` where `key` is no plain letter (an Option
/// letter on a Mac, a non-Latin layout). `None` for a modifier on its own, a dead key, or an
/// input method's key.
pub(crate) fn key(key: &str, code: &str, held: Mods) -> Option<Key> {
    let named = match key {
        "Enter" => Key::Enter,
        "Escape" | "Esc" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" | "Del" => Key::Delete,
        "ArrowLeft" | "Left" => Key::Left,
        "ArrowRight" | "Right" => Key::Right,
        "ArrowUp" | "Up" => Key::Up,
        "ArrowDown" | "Down" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        " " | "Spacebar" => Key::Space,
        "ContextMenu" | "Apps" => Key::Menu,
        _ => return function_key(key).or_else(|| letter(key, code, held)),
    };
    Some(named)
}

/// F1 to F12.
fn function_key(key: &str) -> Option<Key> {
    let number = key.strip_prefix('F')?.parse::<u8>().ok()?;
    (1..=12).contains(&number).then_some(Key::F(number))
}

/// The letter or digit a key types, lower case: from `key` if it is one character, but with
/// Control or Alt held, from `code` (`KeyA`, `Digit1`) when `key` is not an ASCII letter or
/// digit.
fn letter(key: &str, code: &str, held: Mods) -> Option<Key> {
    let mut letters = key.chars();
    let typed = letters.next().filter(|_| letters.next().is_none());
    let plain = typed.filter(|typed| typed.is_ascii_alphanumeric() || !(held.ctrl || held.alt));
    let from_code = || code.strip_prefix("Key").or_else(|| code.strip_prefix("Digit")).and_then(|rest| rest.chars().next()).filter(|_| held.ctrl || held.alt);
    let letter = plain.or_else(from_code)?;
    let letter = letter.to_lowercase().next()?;
    (!letter.is_control()).then_some(Key::Char(letter))
}

/// The text a `keydown` types where no text field takes it (the window's own element has the
/// keyboard): its `key`, if one printable character and no Control or Alt.
pub(crate) fn typed(key: &str, held: Mods) -> Option<&str> {
    let mut letters = key.chars();
    let one = letters.next().is_some_and(|first| !first.is_control()) && letters.next().is_none();
    (one && !held.ctrl && !held.alt).then_some(key)
}

/// Whether the loader stops the browser's own handling of a `keydown`: every key the
/// interface acts on, but Control with C, X or V (the browser's copy and paste events carry the
/// clipboard), and what the browser keeps for itself (reload, full screen, its tools, its
/// tabs).
pub(crate) fn stops(key: Option<Key>, held: Mods) -> bool {
    match key {
        None => false,
        Some(Key::F(5 | 11 | 12)) => false,
        Some(Key::Char(letter)) if held.ctrl => !matches!(letter, 'c' | 'x' | 'v' | 'r' | 't' | 'w' | 'n') && !(held.shift && matches!(letter, 'i' | 'j')),
        // Typed letters reach the text field, whose input events carry them.
        Some(Key::Char(_)) => held.alt,
        Some(_) => true,
    }
}

/// A wheel's turn in units, right and down: `delta_mode` 0 counts pixels (units here), 1
/// lines, 2 pages (of `page` units).
pub(crate) fn wheel(dx: f32, dy: f32, delta_mode: u32, page: [f32; 2]) -> (f32, f32) {
    match delta_mode {
        1 => (dx * NOTCH, dy * NOTCH),
        2 => (dx * page[0], dy * page[1]),
        _ => (dx, dy),
    }
}

/// What a pointer did, as the loader numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Did {
    Move,
    Down,
    Up,
    /// The browser took it (a gesture of its own, a lost capture).
    Cancel,
    /// It left the page's element.
    Leave,
}

impl Did {
    /// From the loader's number: 0 move, 1 down, 2 up, 3 cancel, 4 leave.
    pub(crate) fn of(number: u32) -> Did {
        [Did::Move, Did::Down, Did::Up, Did::Cancel, Did::Leave].get(number as usize).copied().unwrap_or(Did::Move)
    }
}

/// One pointer event, as a `PointerEvent` gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pointer {
    pub did: Did,
    /// `pointerType`: 0 mouse, 1 touch, 2 pen.
    pub kind: u32,
    pub id: u32,
    /// It is the first of its kind (the first finger).
    pub primary: bool,
    pub at: Point,
    /// `button`: 0 left, 1 middle, 2 right, 5 a pen's eraser.
    pub button: i32,
    /// `buttons`, as bits: 1 left (a pen's tip), 2 right (its barrel), 32 the eraser.
    pub buttons: u32,
    pub pressure: f32,
    /// `tiltX`, `tiltY`, `twist`.
    pub tilt: [f32; 2],
    pub twist: f32,
    pub mods: Mods,
}

/// The mouse button a pointer's `button` names.
fn button_of(button: i32) -> Option<Button> {
    match button {
        0 => Some(Button::Left),
        1 => Some(Button::Middle),
        2 => Some(Button::Right),
        _ => None,
    }
}

/// The events a pointer event makes. A mouse: the place, then a press or release. A finger
/// with `touch` on: a touch, which the interface makes taps, scrolls and pinches of; without
/// it, the first finger is the mouse and the others do nothing. A pen with `touch` on: what the
/// pen is (see [`Stylus`]) first, then as a mouse; without it, a mouse.
pub(crate) fn pointer(event: Pointer, touch: bool) -> Vec<Event> {
    match (event.kind, touch) {
        (1, true) => finger(event).into_iter().collect(),
        (1, false) if !event.primary => Vec::new(),
        (2, true) => pen(event),
        _ => mouse(event),
    }
}

/// A mouse, or what acts as one.
fn mouse(event: Pointer) -> Vec<Event> {
    let pressed = button_of(event.button).unwrap_or(Button::Left);
    match event.did {
        Did::Leave => vec![Event::Leave],
        Did::Move => vec![Event::Move(event.at)],
        Did::Down => vec![Event::Move(event.at), Event::Press(pressed, event.mods)],
        Did::Up => vec![Event::Move(event.at), Event::Release(pressed)],
        // A finger the browser took back lifts the button where it was.
        Did::Cancel => vec![Event::Release(Button::Left), Event::Leave],
    }
}

/// A finger.
fn finger(event: Pointer) -> Option<Event> {
    let phase = match event.did {
        Did::Down => TouchPhase::Down,
        Did::Move => TouchPhase::Move,
        Did::Up => TouchPhase::Up,
        Did::Cancel => TouchPhase::Cancel,
        Did::Leave => return None,
    };
    Some(Event::Touch(Touch { id: event.id, position: event.at, phase }))
}

/// A pen: the pen, the place, and its tip or barrel button going down or up.
fn pen(event: Pointer) -> Vec<Event> {
    if matches!(event.did, Did::Leave | Did::Cancel) {
        let lifted = (event.did == Did::Cancel).then_some(Event::Release(Button::Left));
        return [Some(Event::Stylus(None)), Some(Event::Leave), lifted].into_iter().flatten().collect();
    }
    let touching = event.buttons & (1 | 32) != 0;
    let eraser = event.button == 5 || event.buttons & 32 != 0;
    let barrel = event.buttons & 2 != 0 || event.button == 2;
    let pressure = if touching && event.pressure == 0.0 { 0.5 } else { event.pressure };
    let stylus = Stylus { pressure, tilt: event.tilt, twist: event.twist, barrel, eraser, touching };
    let which = if event.button == 2 { Button::Right } else { Button::Left };
    let button = match event.did {
        Did::Down => Some(Event::Press(which, event.mods)),
        Did::Up => Some(Event::Release(which)),
        _ => None,
    };
    [Some(Event::Stylus(Some(stylus))), Some(Event::Move(event.at)), button].into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pt;

    #[test]
    fn keys_are_read_by_their_dom_names() {
        let none = Mods::NONE;
        let named = [
            ("Enter", Key::Enter),
            ("Escape", Key::Escape),
            ("Tab", Key::Tab),
            ("Backspace", Key::Backspace),
            ("Delete", Key::Delete),
            ("ArrowLeft", Key::Left),
            ("ArrowRight", Key::Right),
            ("ArrowUp", Key::Up),
            ("ArrowDown", Key::Down),
            ("Home", Key::Home),
            ("End", Key::End),
            ("PageUp", Key::PageUp),
            ("PageDown", Key::PageDown),
            (" ", Key::Space),
            ("ContextMenu", Key::Menu),
            ("F2", Key::F(2)),
            ("F12", Key::F(12)),
        ];
        for (name, expected) in named {
            assert_eq!(key(name, "", none), Some(expected), "{name}");
        }
        // Letters lower case, as typed; with Control from the key's place where the layout
        // types no Latin letter (a Russian layout's Ctrl+C), and an Option letter on a Mac.
        assert_eq!(key("a", "KeyA", none), Some(Key::Char('a')));
        assert_eq!(key("A", "KeyA", Mods::SHIFT), Some(Key::Char('a')));
        assert_eq!(key("с", "KeyC", Mods::CTRL), Some(Key::Char('c')));
        assert_eq!(key("å", "KeyA", Mods { alt: true, ..none }), Some(Key::Char('a')));
        assert_eq!(key("é", "Digit2", none), Some(Key::Char('é')));
        assert_eq!(key("1", "Digit1", Mods::CTRL), Some(Key::Char('1')));
        // Modifiers alone, dead keys and an input method's keys are no key.
        for name in ["Shift", "Control", "Alt", "Meta", "Dead", "Process", "Unidentified", "F13", "CapsLock"] {
            assert_eq!(key(name, "", none), None, "{name}");
        }
    }

    #[test]
    fn the_browser_keeps_copy_and_paste_and_its_own_keys() {
        let ctrl = Mods::CTRL;
        assert!(stops(Some(Key::Tab), Mods::NONE) && stops(Some(Key::Char('s')), ctrl) && stops(Some(Key::Char('a')), ctrl) && stops(Some(Key::F(2)), Mods::NONE));
        assert!(!stops(Some(Key::Char('c')), ctrl) && !stops(Some(Key::Char('v')), ctrl) && !stops(Some(Key::Char('x')), ctrl));
        assert!(!stops(Some(Key::F(5)), Mods::NONE) && !stops(Some(Key::F(12)), Mods::NONE) && !stops(Some(Key::Char('i')), Mods { shift: true, ..ctrl }));
        // A typed letter goes on to the text field; nothing for no key.
        assert!(!stops(Some(Key::Char('a')), Mods::NONE) && !stops(Some(Key::Char('a')), Mods::SHIFT) && !stops(None, ctrl));
        assert_eq!([typed("a", Mods::NONE), typed("é", Mods::SHIFT), typed("a", ctrl), typed("Enter", Mods::NONE), typed("\u{7f}", Mods::NONE)], [Some("a"), Some("é"), None, None, None]);
        assert_eq!(mods(CTRL | ALT), Mods { ctrl: true, shift: false, alt: true });
    }

    #[test]
    fn a_wheel_counts_pixels_lines_or_pages() {
        assert_eq!(wheel(0.0, 100.0, 0, [800.0, 600.0]), (0.0, 100.0));
        assert_eq!(wheel(1.0, -3.0, 1, [800.0, 600.0]), (NOTCH, -3.0 * NOTCH));
        assert_eq!(wheel(0.0, 1.0, 2, [800.0, 600.0]), (0.0, 600.0));
    }

    /// A pointer event of a kind, at a place.
    fn at(kind: u32, did: Did, button: i32, buttons: u32) -> Pointer {
        Pointer { did, kind, id: 3, primary: true, at: pt(10.0, 20.0), button, buttons, pressure: 0.0, tilt: [0.0; 2], twist: 0.0, mods: Mods::NONE }
    }

    #[test]
    fn a_mouse_moves_then_presses() {
        let place = Event::Move(pt(10.0, 20.0));
        assert_eq!(pointer(at(0, Did::Down, 0, 1), false), [place.clone(), Event::Press(Button::Left, Mods::NONE)]);
        assert_eq!(pointer(at(0, Did::Up, 2, 0), true), [place.clone(), Event::Release(Button::Right)]);
        assert_eq!(pointer(at(0, Did::Down, 1, 4), false), [place.clone(), Event::Press(Button::Middle, Mods::NONE)]);
        assert_eq!(pointer(at(0, Did::Leave, -1, 0), false), [Event::Leave]);
        assert_eq!(Did::of(2), Did::Up);
        assert_eq!(Did::of(99), Did::Move);
    }

    #[test]
    fn a_finger_is_a_touch_or_the_mouse() {
        let down = at(1, Did::Down, 0, 1);
        assert_eq!(pointer(down, true), [Event::Touch(Touch { id: 3, position: pt(10.0, 20.0), phase: TouchPhase::Down })]);
        assert_eq!(pointer(Pointer { did: Did::Cancel, ..down }, true), [Event::Touch(Touch { id: 3, position: pt(10.0, 20.0), phase: TouchPhase::Cancel })]);
        // Without `touch`: the first finger is the mouse, a second does nothing.
        assert_eq!(pointer(down, false), [Event::Move(pt(10.0, 20.0)), Event::Press(Button::Left, Mods::NONE)]);
        assert_eq!(pointer(Pointer { primary: false, ..down }, false), []);
        assert_eq!(pointer(Pointer { did: Did::Cancel, ..down }, false), [Event::Release(Button::Left), Event::Leave]);
    }

    #[test]
    fn a_pen_says_what_it_is_before_it_presses() {
        let down = Pointer { pressure: 0.75, tilt: [30.0, -15.0], twist: 90.0, ..at(2, Did::Down, 0, 1) };
        let pen = Stylus { pressure: 0.75, tilt: [30.0, -15.0], twist: 90.0, barrel: false, eraser: false, touching: true };
        assert_eq!(pointer(down, true), [Event::Stylus(Some(pen)), Event::Move(pt(10.0, 20.0)), Event::Press(Button::Left, Mods::NONE)]);
        let hovering = Pointer { did: Did::Move, buttons: 0, pressure: 0.0, ..down };
        assert!(matches!(pointer(hovering, true)[..], [Event::Stylus(Some(Stylus { touching: false, pressure: 0.0, .. })), Event::Move(_)]));
        let rubbing = Pointer { button: 5, buttons: 32, pressure: 0.0, ..down };
        assert!(matches!(pointer(rubbing, true)[..], [Event::Stylus(Some(Stylus { eraser: true, touching: true, pressure: 0.5, .. })), Event::Move(_), Event::Press(Button::Left, _)]));
        let barrel = Pointer { button: 2, buttons: 3, ..down };
        assert!(matches!(pointer(barrel, true)[..], [Event::Stylus(Some(Stylus { barrel: true, .. })), Event::Move(_), Event::Press(Button::Right, _)]));
        assert_eq!(pointer(Pointer { did: Did::Leave, ..down }, true), [Event::Stylus(None), Event::Leave]);
        // Without `touch`, a pen is a mouse.
        assert_eq!(pointer(down, false), [Event::Move(pt(10.0, 20.0)), Event::Press(Button::Left, Mods::NONE)]);
    }
}
