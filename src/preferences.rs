//! What the person asked their system for about how windows look and move: dark mode, an accent
//! colour, high contrast, reduced motion, larger text. Whoever runs a window reads it from the
//! system and hands it to the interface, again on every change, and the interface runs for the
//! app to hear. An app that follows all of it uses [`ui.system_theme()`](Ui::system_theme) as
//! its theme; what the library moves itself (`animate`, a switch, a spinner) keeps still on its
//! own.
//!
//! On Windows it is read here: light or dark apps, the accent with Windows' shades of it (the
//! window-frame colour function gives it blended for a frame, slightly darker), the
//! high-contrast setting and its theme's colours, whether animations are on, and the text size.
//! On Linux the desktop portal says it (`portal`), and where the portal says nothing, XSETTINGS
//! do: those are read here too.
use crate::color::Color;
use crate::theme::{ContrastColors, Theme};
use crate::ui::Ui;
use std::cell::Cell;

/// What the person asked their system for about how windows look and move. The window code
/// reads it; an app that runs the interface itself (in a DLL, under its own renderer) hands it
/// over with [`ui.set_preferences`](Ui::set_preferences).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preferences {
    /// Dark mode.
    pub dark: bool,
    /// The accent colour the person picked, where the system keeps one.
    pub accent: Option<Color>,
    /// High contrast, in these colours: the system's where it has them, else the library's
    /// ([`ContrastColors::DARK`] or [`ContrastColors::LIGHT`], by `dark`).
    pub high_contrast: Option<ContrastColors>,
    /// Reduced motion: nothing glides, slides or turns by itself.
    pub reduced_motion: bool,
    /// Text scale: 1 as it is, 1.5 half as large again.
    pub text_scale: f32,
}

impl Default for Preferences {
    /// Nothing asked for: light mode, text as it is.
    fn default() -> Self {
        Preferences { dark: false, accent: None, high_contrast: None, reduced_motion: false, text_scale: 1.0 }
    }
}

/// What an interface keeps of the preferences: them, and the theme that follows them once
/// asked for, kept until they change (an app that asks does so every run; one that never does
/// carries none of its making).
#[derive(Clone, Debug, Default)]
pub(crate) struct SystemPreferences {
    pub(crate) current: Preferences,
    theme: Cell<Option<Theme>>,
}

/// What the person asked the system for: dark or light, colours, contrast, less motion, larger
/// text.
impl Ui {
    /// For the window code (or an app running the interface itself): what the person asked the
    /// system for. Says whether that is news (then the interface should run, for the app to
    /// hear).
    pub fn set_preferences(&mut self, preferences: Preferences) -> bool {
        if preferences == self.preferences.current {
            return false;
        }
        self.preferences = SystemPreferences { current: preferences, theme: Cell::new(None) };
        true
    }
    /// Whether the person asked the system for dark mode. An app that follows it sets
    /// [`ui.theme`](Ui::theme) from it as it starts describing its window; the interface runs
    /// when the person changes it. (The window frame is dark where the system draws it and the
    /// theme's background is.)
    pub fn system_dark(&self) -> bool {
        self.preferences.current.dark
    }
    /// The accent colour the person picked, where the system keeps one: always on Windows, on
    /// Linux where the desktop says.
    pub fn system_accent(&self) -> Option<Color> {
        self.preferences.current.accent
    }
    /// Whether the person asked for high contrast. [`ui.system_theme()`](Ui::system_theme) is
    /// then the high-contrast theme in the system's colours, where it has them.
    pub fn high_contrast(&self) -> bool {
        self.preferences.current.high_contrast.is_some()
    }
    /// Whether the person asked for reduced motion. The library then keeps still by itself:
    /// `animate` jumps to its end, a switch flips without sliding, a spinner stands and fades
    /// slowly in and out instead of turning. An app that animates things of its own asks this.
    pub fn reduced_motion(&self) -> bool {
        self.preferences.current.reduced_motion
    }
    /// The text scale the person asked for: 1 as it is. (On Linux the desktop enlarges the
    /// whole window through its scale instead, so it is 1 there.)
    pub fn text_scale(&self) -> f32 {
        self.preferences.current.text_scale
    }
    /// The theme that follows all the person asked their system for ([`Theme::following`]):
    /// what an app sets [`ui.theme`](Ui::theme) to as it starts describing its window, to look
    /// like the person's system. Made once per change, so asking costs nothing.
    pub fn system_theme(&self) -> Theme {
        let theme = self.preferences.theme.get().unwrap_or_else(|| Theme::following(&self.preferences.current));
        self.preferences.theme.set(Some(theme));
        theme
    }
}

/// The person's preferences as Windows keeps them now. Read whenever Windows says a setting
/// changed: a few numbers, read in a few µs.
#[cfg(windows)]
pub(crate) fn of_windows() -> Preferences {
    #[cfg(test)]
    if let Some(preferences) = tests::PRETENDED.get() {
        return preferences;
    }
    let mut preferences = Preferences { dark: number("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "AppsUseLightTheme") == Some(0), ..Preferences::default() };
    // Text scale from 100 to 225 percent (1 where nothing is said).
    preferences.text_scale = number("Software\\Microsoft\\Accessibility", "TextScaleFactor").map_or(1.0, |percent| percent.clamp(100, 225) as f32 / 100.0);
    // The accent as Windows keeps it with lighter and darker shades: eight colours of four
    // bytes (red, green, blue, one more), lightest first, the fourth the one picked.
    let mut palette = [0u8; 32];
    let read = registry("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent", "AccentPalette", 0x08, &mut palette) == Some(32);
    preferences.accent = read.then(|| Color::rgb(u32::from_be_bytes([0, palette[12], palette[13], palette[14]])));
    let (mut moves, mut contrast) = (1i32, [0u32; 4]);
    // SAFETY: each call fills in a value as large as it says: whether animations are on, and
    // the high-contrast state (room for its size, flags and theme name).
    unsafe {
        preferences.reduced_motion = sys::SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut moves).cast(), 0) != 0 && moves == 0;
        contrast[0] = 16;
        let read = sys::SystemParametersInfoW(SPI_GETHIGHCONTRAST, 16, contrast.as_mut_ptr().cast(), 0) != 0;
        preferences.high_contrast = (read && contrast[1] & HCF_HIGHCONTRASTON != 0).then(system_colors);
    }
    preferences
}

/// What `SystemParametersInfoW` is asked: animations on, the high-contrast state; and its "on"
/// flag.
#[cfg(windows)]
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
#[cfg(windows)]
const SPI_GETHIGHCONTRAST: u32 = 0x0042;
#[cfg(windows)]
const HCF_HIGHCONTRASTON: u32 = 1;

/// The colours of the high-contrast theme the person picked, as Windows' system colours by
/// index, each `0x00BBGGRR`: window background and text (5, 8), links (26), disabled (17),
/// selection and its text (13, 14), buttons and their text (15, 18).
#[cfg(windows)]
fn system_colors() -> ContrastColors {
    // SAFETY: plain queries.
    let color = |index: i32| {
        let bgr = unsafe { sys::GetSysColor(index) };
        Color::rgb((bgr & 0xff) << 16 | (bgr & 0xff00) | (bgr >> 16 & 0xff))
    };
    ContrastColors { background: color(5), text: color(8), link: color(26), disabled: color(17), selected: color(13), selected_text: color(14), button: color(15), button_text: color(18) }
}

/// A number in the person's settings under `key`, by `name`; none if there is no such number.
#[cfg(windows)]
fn number(key: &str, name: &str) -> Option<u32> {
    let mut value = [0u8; 4];
    registry(key, name, 0x10, &mut value).filter(|bytes| *bytes == 4).map(|_| u32::from_le_bytes(value))
}

/// A setting under `HKEY_CURRENT_USER\key` by `name`, of the types `kinds` allows (0x10 a
/// DWORD, 0x08 binary), read into `value`: its length, or none if absent or too big.
#[cfg(windows)]
fn registry(key: &str, name: &str, kinds: u32, value: &mut [u8]) -> Option<u32> {
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, name, mut bytes) = (wide(key), wide(name), value.len() as u32);
    // SAFETY: reads one value of `HKEY_CURRENT_USER` (0x8000_0001) into the given bytes, as
    // many as said; both names end in a zero.
    let read = unsafe { sys::RegGetValueW(0x8000_0001u32 as i32 as isize, key.as_ptr(), name.as_ptr(), kinds, std::ptr::null_mut(), value.as_mut_ptr(), &mut bytes) };
    (read == 0).then_some(bytes)
}

/// The Windows functions read here, declared here.
#[cfg(windows)]
#[allow(non_snake_case)]
mod sys {
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn SystemParametersInfoW(action: u32, number: u32, value: *mut core::ffi::c_void, change: u32) -> i32;
        pub fn GetSysColor(index: i32) -> u32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        pub fn RegGetValueW(key: isize, under: *const u16, name: *const u16, kinds: u32, kind: *mut u32, value: *mut u8, bytes: *mut u32) -> i32;
    }
}

/// What XSETTINGS say the person asked for: what windows go by where the portal says nothing.
/// One block of bytes: the byte order, then two numbers, the second the count of settings; each
/// setting its type (0 integer, 1 string, 2 colour), name length, name, serial and value,
/// aligned to four bytes. Nothing if the bytes are not laid out so.
///
/// Dark mode: a theme whose name ends in "-dark", or the switch asking programs for their
/// theme's dark variant. High contrast: a theme of that name, as GNOME names them
/// ("HighContrastInverse" is the dark one). Reduced motion: animations switched off. (Larger
/// text comes through the screen scale, which the window already follows.)
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn of_x_settings(settings: &[u8]) -> Option<Preferences> {
    let ends_first = *settings.first()? == 0;
    let number = |at: usize, len: usize| {
        let mut bytes = [0; 4];
        bytes[..len].copy_from_slice(settings.get(at..at.checked_add(len)?)?);
        Some(if ends_first { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) >> (32 - 8 * len) } as usize)
    };
    let (mut at, mut preferences, mut theme) = (12, Preferences::default(), &b""[..]);
    for _ in 0..number(8, 4)? {
        let (kind, named) = (*settings.get(at)?, number(at + 2, 2)?);
        let (name, value) = (settings.get(at + 4..at + 4 + named)?, (at + 4 + named).next_multiple_of(4) + 4);
        let long = match kind {
            0 => 4,
            1 => number(value, 4)?.checked_next_multiple_of(4)?.checked_add(4)?,
            _ => 8,
        };
        let held = settings.get(value..value.checked_add(long)?)?;
        match (kind, name) {
            (0, b"Gtk/ApplicationPreferDarkTheme") => preferences.dark |= number(value, 4)? == 1,
            (0, b"Gtk/EnableAnimations") => preferences.reduced_motion = number(value, 4)? == 0,
            (1, b"Net/ThemeName") => theme = &held[4..][..number(value, 4)?],
            _ => {}
        }
        at = value + long;
    }
    let named = |start: &[u8], end: &[u8]| theme.len() >= start.len().max(end.len()) && theme[..start.len()].eq_ignore_ascii_case(start) && theme[theme.len() - end.len()..].eq_ignore_ascii_case(end);
    preferences.dark |= named(b"", b"-dark") || named(b"HighContrastInverse", b"");
    preferences.high_contrast = named(b"HighContrast", b"").then_some(if preferences.dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
    Some(preferences)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::color::Color;

    std::thread_local! {
        /// What a test has Windows say instead, on its own thread: so a window's handling of a
        /// change is tested without changing the person's settings.
        pub(crate) static PRETENDED: std::cell::Cell<Option<Preferences>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn an_interface_hears_what_the_person_asked_for_once() {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        assert!(!ui.system_dark() && ui.system_accent().is_none() && !ui.high_contrast() && !ui.reduced_motion() && ui.text_scale() == 1.0);
        assert_eq!(ui.system_theme(), Theme::light());
        let preferences = Preferences { dark: true, accent: Some(Color::rgb(0x0078d4)), high_contrast: None, reduced_motion: true, text_scale: 1.25 };
        // News once, and the theme that follows it made then.
        assert!(ui.set_preferences(preferences) && !ui.set_preferences(preferences));
        assert!(ui.system_dark() && ui.system_accent() == Some(Color::rgb(0x0078d4)) && ui.reduced_motion() && ui.text_scale() == 1.25);
        assert_eq!(ui.system_theme(), Theme::following(&preferences));
        assert!(ui.set_preferences(Preferences { high_contrast: Some(ContrastColors::DARK), ..preferences }) && ui.high_contrast());
        assert_eq!(ui.system_theme(), Theme::high_contrast(ContrastColors::DARK).text_scaled(1.25));
    }

    /// With reduced motion, nothing of the library's moves by itself: a gliding number is at
    /// its end at once, a switch flips without sliding, and a spinner stands still (fading
    /// slowly in and out instead, drawn a third as often).
    #[test]
    fn nothing_moves_by_itself_for_a_person_who_asked_for_less_movement() {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 40.0, h: 40.0 }, 1.0);
        let over = std::time::Duration::from_millis(120);
        let glided = |ui: &mut Ui, target: f32| {
            let mut value = f32::NAN;
            ui.run(&mut |ui| value = ui.animate(7, target, over));
            (value, ui.wake_in())
        };
        // By default: on its way, and the interface runs again a frame from now.
        assert_eq!(glided(&mut ui, 0.0), (0.0, None));
        assert_eq!(glided(&mut ui, 1.0), (0.0, Some(crate::ui::FRAME)));
        ui.set_preferences(Preferences { reduced_motion: true, ..Preferences::default() });
        // Reduced: there at once, with nothing more to run for; on the way back too.
        assert_eq!(glided(&mut ui, 1.0), (1.0, None));
        assert_eq!(glided(&mut ui, 0.0), (0.0, None));

        // A spinner drawn at two moments half a second apart: turned, or standing.
        let spun = |ui: &mut Ui, at: u64| {
            ui.set_time(std::time::Duration::from_millis(at));
            ui.run(&mut |ui| _ = ui.spinner());
            let drawn = crate::Surface::draw(ui, 64).pixels;
            // Which pixels differ from the corner's background, and what they are.
            (drawn.iter().map(|pixel| *pixel != drawn[0]).collect::<Vec<bool>>(), drawn, ui.wake_in())
        };
        // Standing: at the same point of its fade a second later it is drawn identically; at
        // another point, fainter.
        let (first, later) = (spun(&mut ui, 500), spun(&mut ui, 1500));
        assert!(first.1 == later.1 && spun(&mut ui, 0).1 != first.1, "standing where it was, and fading");
        assert_eq!((first.2, later.2), (Some(crate::ui::FRAME * 3), Some(crate::ui::FRAME * 3)));
        ui.set_preferences(Preferences::default());
        let (first, later) = (spun(&mut ui, 1000), spun(&mut ui, 1250));
        assert!(first.0 != later.0 && first.2 == Some(crate::ui::FRAME), "turned a quarter of the way round");
    }

    /// What Windows has set here, read as the window reads it: within what Windows can say, and
    /// the same twice.
    #[cfg(windows)]
    #[test]
    fn what_windows_keeps_is_read() {
        let preferences = of_windows();
        assert_eq!(of_windows(), preferences);
        assert!((1.0..=2.25).contains(&preferences.text_scale), "{preferences:?}");
        // Windows 10 and 11 always keep an accent.
        assert!(preferences.accent.is_some_and(|accent| accent.a() == 255), "{preferences:?}");
        // What a test pretends comes first, on the test's own thread only.
        let pretend = Preferences { dark: !preferences.dark, reduced_motion: true, text_scale: 2.0, ..Preferences::default() };
        PRETENDED.set(Some(pretend));
        assert_eq!((of_windows(), std::thread::spawn(of_windows).join().unwrap()), (pretend, preferences));
        PRETENDED.set(None);
        assert_eq!(of_windows(), preferences);
    }

    /// A value among XSETTINGS.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) enum Kept<'a> {
        Number(u32),
        Text(&'a str),
        Colour,
    }

    /// XSETTINGS as the settings daemon writes them, in either byte order.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) fn settings_of(ends_first: bool, list: &[(&str, Kept)]) -> Vec<u8> {
        let number = |value: u32, len: usize| if ends_first { value.to_le_bytes()[..len].to_vec() } else { value.to_be_bytes()[4 - len..].to_vec() };
        let mut bytes = vec![!ends_first as u8, 0, 0, 0];
        bytes.extend([number(7, 4), number(list.len() as u32, 4)].concat());
        for (name, kept) in list {
            let (kind, value) = match kept {
                Kept::Number(value) => (0, number(*value, 4)),
                Kept::Text(text) => (1, [&number(text.len() as u32, 4), text.as_bytes(), &[0u8; 3][..text.len().next_multiple_of(4) - text.len()]].concat()),
                Kept::Colour => (2, vec![0xff; 8]),
            };
            bytes.extend([&[kind, 0][..], &number(name.len() as u32, 2), name.as_bytes()].concat());
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            bytes.extend([number(1, 4), value].concat());
        }
        bytes
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_desktops_settings_say_what_the_person_asked_for() {
        use Kept::{Colour, Number, Text};
        let read = |list: &[(&str, Kept)]| [true, false].map(|ends_first| of_x_settings(&settings_of(ends_first, list)));
        let dark = |list: &[(&str, Kept)]| read(list).map(|preferences| preferences.map(|preferences| preferences.dark));
        // By the theme's name, in any case, among settings of every type and name length.
        assert_eq!(dark(&[("Net/ThemeName", Text("Adwaita-dark"))]), [Some(true); 2]);
        let many = [("Xft/DPI", Number(98_304)), ("Gtk/ColorPalette", Colour), ("Net/IconThemeName", Text("Papirus")), ("Net/ThemeName", Text("Breeze-Dark")), ("Gtk/FontName", Text("Sans 10"))];
        assert_eq!(dark(&many), [Some(true); 2]);
        // A theme is not dark for having the word somewhere in its name, nor are windows dark
        // for a dark icon theme.
        for light in ["Adwaita", "Arc-Darker", "Materia-dark-compact", "dark", ""] {
            assert_eq!(dark(&[("Net/ThemeName", Text(light)), ("Net/IconThemeName", Text("Papirus-dark"))]), [Some(false); 2], "{light:?}");
        }
        // By the switch asking programs for their theme's dark variant.
        assert_eq!(dark(&[("Net/ThemeName", Text("Adwaita")), ("Gtk/ApplicationPreferDarkTheme", Number(1))]), [Some(true); 2]);
        assert_eq!(dark(&[("Gtk/ApplicationPreferDarkTheme", Number(0)), ("Net/ThemeName", Text("Adwaita"))]), [Some(false); 2]);
        // A string under the switch's name is no switch, and a number under the theme's no
        // name.
        assert_eq!(dark(&[("Gtk/ApplicationPreferDarkTheme", Text("1")), ("Net/ThemeName", Number(1))]), [Some(false); 2]);
        assert_eq!(read(&[]), [Some(Preferences::default()); 2]);

        // High contrast by the theme's name: the light one and the dark one.
        let contrast = |name: &'static str| read(&[("Net/ThemeName", Text(name))]).map(|preferences| preferences.map(|preferences| (preferences.dark, preferences.high_contrast)));
        assert_eq!(contrast("HighContrast"), [Some((false, Some(ContrastColors::LIGHT))); 2]);
        assert_eq!(contrast("HighContrastInverse"), [Some((true, Some(ContrastColors::DARK))); 2]);
        assert_eq!(contrast("highcontrast"), [Some((false, Some(ContrastColors::LIGHT))); 2]);
        assert_eq!(contrast("Adwaita-HighContrast-ish"), [Some((false, None)); 2]);
        // Reduced motion, by the switch that turns animations off.
        let still = |list: &[(&str, Kept)]| read(list).map(|preferences| preferences.map(|preferences| preferences.reduced_motion));
        assert_eq!([still(&[("Gtk/EnableAnimations", Number(0))]), still(&[("Gtk/EnableAnimations", Number(1))])], [[Some(true); 2], [Some(false); 2]]);
        assert_eq!(still(&[("Gtk/EnableAnimations", Text("0"))]), [Some(false); 2]);

        // Cut short anywhere, they are no settings; with any byte changed, they are read
        // without harm.
        let whole = settings_of(true, &[("Gtk/EnableAnimations", Number(0)), ("Net/ThemeName", Text("HighContrast"))]);
        (0..whole.len()).for_each(|len| assert_eq!(of_x_settings(&whole[..len]), None, "cut short after {len} bytes"));
        let whole = settings_of(true, &many);
        (0..whole.len()).for_each(|len| assert_eq!(of_x_settings(&whole[..len]), None, "cut short after {len} bytes"));
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut chance = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        for _ in 0..50_000 {
            let mut scrambled = whole.clone();
            for _ in 0..1 + chance(4) {
                let at = chance(scrambled.len());
                scrambled[at] = chance(256) as u8;
            }
            std::hint::black_box(of_x_settings(&scrambled));
        }
    }
}
