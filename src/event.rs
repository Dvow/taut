//! Input events, the same on every system.
use crate::geom::Point;

/// A button of the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Button {
    /// The one that presses and picks.
    Left,
    /// The one that opens a menu where the pointer is.
    Right,
    /// The one under the wheel.
    Middle,
}

/// Keys acted on by name. Typed text comes as [`Event::Text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Key {
    /// Enter: presses the focused part; a new line where lines are typed.
    Enter,
    /// Escape: leaves a menu or dialog.
    Escape,
    /// Tab: focus to the next part; with Shift, to the one before.
    Tab,
    /// Backspace: deletes before the cursor.
    Backspace,
    /// Delete: deletes after the cursor.
    Delete,
    /// The arrow to the left.
    Left,
    /// The arrow to the right.
    Right,
    /// The arrow up.
    Up,
    /// The arrow down.
    Down,
    /// Home: to the start of the line, or of all.
    Home,
    /// End: to the end of the line, or of all.
    End,
    /// Page Up: a page back.
    PageUp,
    /// Page Down: a page on.
    PageDown,
    /// Space: presses the focused part.
    Space,
    /// The menu key: opens the focused part's menu, as Shift+F10 does.
    Menu,
    /// F1 to F12.
    F(u8),
    /// A letter or digit key, lower case: for shortcuts like Ctrl+C.
    Char(char),
}

/// Parses a shortcut as a menu shows it: "Ctrl+S", "F2", "Ctrl+Shift+Z". Case-insensitive;
/// `None` if it names no key.
pub fn parse_keys(written: &str) -> Option<(Key, Mods)> {
    let (held, last) = written.rsplit_once('+').unwrap_or(("", written));
    let mut mods = Mods::NONE;
    for word in held.split('+').map(str::trim).filter(|word| !word.is_empty()) {
        match word.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            _ => return None,
        }
    }
    let name = last.trim().to_ascii_lowercase();
    let numbered = name.strip_prefix('f').and_then(|number| number.parse::<u8>().ok()).filter(|number| (1..=12).contains(number));
    let mut letters = name.chars();
    let key = match (name.as_str(), numbered, letters.next(), letters.next()) {
        ("enter" | "return", ..) => Key::Enter,
        ("escape" | "esc", ..) => Key::Escape,
        ("tab", ..) => Key::Tab,
        ("backspace", ..) => Key::Backspace,
        ("delete" | "del", ..) => Key::Delete,
        ("left", ..) => Key::Left,
        ("right", ..) => Key::Right,
        ("up", ..) => Key::Up,
        ("down", ..) => Key::Down,
        ("home", ..) => Key::Home,
        ("end", ..) => Key::End,
        ("pageup" | "page up" | "pgup", ..) => Key::PageUp,
        ("pagedown" | "page down" | "pgdn", ..) => Key::PageDown,
        ("space", ..) => Key::Space,
        ("menu" | "apps", ..) => Key::Menu,
        (_, Some(number), ..) => Key::F(number),
        (_, None, Some(letter), None) => Key::Char(letter),
        _ => return None,
    };
    Some((key, mods))
}

/// Modifier keys held during a key or button press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
}

impl Mods {
    /// None held.
    pub const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    /// Control alone.
    pub const CTRL: Mods = Mods { ctrl: true, ..Mods::NONE };
    /// Shift alone.
    pub const SHIFT: Mods = Mods { shift: true, ..Mods::NONE };
}

/// One event of a window: input, or news from the system.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The pointer moved here, in units from the window's top left.
    Move(Point),
    /// The pointer left the window.
    Leave,
    /// A button went down, with these modifiers.
    Press(Button, Mods),
    /// A button came up again.
    Release(Button),
    /// The wheel turned: units to scroll right and down.
    Wheel(f32, f32),
    /// Ctrl+wheel or a touchpad pinch: scale what is under the pointer by this factor. Taken by
    /// a zooming box ([`Ui::zoom`](crate::Ui::zoom)) or a board.
    Zoom(f32),
    /// A finger on a touch screen landed, moved or lifted. Becomes taps, scrolling, long
    /// presses and pinches.
    Touch(crate::touch::Touch),
    /// A pen came near or changed, or left (`None`). Comes before the `Move`, `Press` or
    /// `Release` it makes; see [`Ui::stylus`](crate::Ui::stylus).
    Stylus(Option<crate::touch::Stylus>),
    /// A key went down (or repeats), with these modifiers.
    Key(Key, Mods),
    /// A key came up. (Shift, Ctrl and Alt come as `Held`.)
    KeyUp(Key),
    /// Text typed, or committed by an input method.
    Text(String),
    /// Shift, Ctrl or Alt went down or up: these are held now.
    ModifiersChanged(Mods),
    /// The window gained (`true`) or lost keyboard focus.
    Focused(bool),
    /// The window was minimized (`true`) or restored.
    Minimized(bool),
    /// Files from another program are dragged over the window (`true`), or left or were
    /// dropped. `Move` gives the pointer meanwhile.
    FilesHovered(bool),
    /// Files from another program were dropped at the pointer.
    FilesDropped(Vec<std::path::PathBuf>),
    /// Text from another program is dragged over the window (`true`), or no longer is.
    TextHovered(bool),
    /// Text from another program was dropped at the pointer.
    TextDropped(String),
}

/// The pointer's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    /// The usual arrow.
    #[default]
    Arrow,
    /// A hand: can be pressed.
    Hand,
    /// An I-beam: text.
    Text,
    /// Left-right arrows: a divider to drag.
    ResizeX,
    /// Up-down arrows: an edge to drag.
    ResizeY,
    /// Diagonal arrows for the top left and bottom right corners.
    ResizeDown,
    /// Diagonal arrows for the top right and bottom left corners.
    ResizeUp,
    /// Arrows every way: this can be moved.
    Move,
    /// A barred circle: cannot drop here.
    No,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_the_way_a_menu_writes_them() {
        let (ctrl, shift) = (Mods::CTRL, Mods::SHIFT);
        let both = Mods { ctrl: true, shift: true, alt: false };
        assert_eq!([parse_keys("Ctrl+S"), parse_keys("ctrl + s"), parse_keys("Control+S")], [Some((Key::Char('s'), ctrl)); 3]);
        assert_eq!([parse_keys("Ctrl+Shift+Z"), parse_keys("Shift+Ctrl+z")], [Some((Key::Char('z'), both)); 2]);
        assert_eq!([parse_keys("F2"), parse_keys("f12"), parse_keys("Alt+F4")], [Some((Key::F(2), Mods::NONE)), Some((Key::F(12), Mods::NONE)), Some((Key::F(4), Mods { alt: true, ..Mods::NONE }))]);
        assert_eq!([parse_keys("F"), parse_keys("/"), parse_keys("Shift+Tab")], [Some((Key::Char('f'), Mods::NONE)), Some((Key::Char('/'), Mods::NONE)), Some((Key::Tab, shift))]);
        let named =
            [("Enter", Key::Enter), ("Esc", Key::Escape), ("Delete", Key::Delete), ("Page Down", Key::PageDown), ("PgUp", Key::PageUp), ("Space", Key::Space), ("Left", Key::Left), ("End", Key::End)];
        for (written, key) in named {
            assert_eq!(parse_keys(written), Some((key, Mods::NONE)), "{written}");
        }
        // No key, or an unknown modifier.
        assert_eq!([parse_keys(""), parse_keys("Ctrl+"), parse_keys("F13"), parse_keys("Hyper+S"), parse_keys("Save")], [None; 5]);
    }
}
