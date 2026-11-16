//! A settings window: sections listed on the left, the selected section's form on the right.
//! The window follows the system's light or dark mode unless the user picks one; accent colour,
//! text size and density are theirs to choose too, and the whole window applies them at once.
//!   cargo run -p taut --example settings
#![windows_subsystem = "windows"]
use taut::{Align, Color, Font, Length, Theme, Ui, Window};

const SECTIONS: [&str; 4] = ["Appearance", "Notifications", "Account", "About"];
/// Light or dark: as the system asks, or always the one or the other.
const LOOKS: [&str; 3] = ["As the system", "Light", "Dark"];
/// Colours that stand out well on both grounds, by name.
const ACCENTS: [(&str, u32); 4] = [("Violet", 0x7b83f0), ("Teal", 0x1f9e8c), ("Amber", 0xd5832f), ("Rose", 0xd8587a)];
const GATHER: [&str; 3] = ["As they come", "Every hour", "Once a day"];

/// Everything the window lets the person set, and which section is shown.
pub struct Settings {
    section: usize,
    look: usize,
    accent: Color,
    letters: f32,
    close: bool,
    notify: bool,
    sound: bool,
    gather: usize,
    /// When nothing is said, from one hour of the day to another.
    quiet: (f32, f32),
    name: String,
    email: String,
    /// How many days of history are kept.
    days: i32,
    reports: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            section: 0,
            look: 0,
            accent: Color::rgb(ACCENTS[0].1),
            letters: 13.0,
            close: false,
            notify: true,
            sound: false,
            gather: 0,
            quiet: (22.0, 7.0),
            name: String::new(),
            email: String::new(),
            days: 30,
            reports: true,
        }
    }
}

/// The theme the settings ask for: light or dark, with the person's colour and letters.
fn theme(app: &Settings, system_dark: bool) -> Theme {
    let dark = [system_dark, false, true][app.look];
    let base = if dark { Theme::dark() } else { Theme::light() };
    let (control, gap) = if app.close { (24.0, 5.0) } else { (base.control, base.gap) };
    let letters = app.letters.round();
    Theme { accent: app.accent, font: Font::new(letters), small: Font::new(letters - 1.5), control, gap, ..base }
}

pub fn describe(app: &mut Settings, ui: &mut Ui) {
    ui.theme = theme(app, ui.system_dark());
    let t = ui.theme;
    ui.row().grow().align(Align::Start, Align::Start).open();

    // The sections, in a list down the left.
    ui.column().w(Length::Px(180.0)).h(Length::Grow(1.0)).pad(8.0).gap(2.0).fill(t.panel).open();
    for (number, name) in SECTIONS.iter().enumerate() {
        if ui.selectable(app.section == number, name).clicked {
            app.section = number;
        }
    }
    ui.close();

    // The form of the section that is picked: a heading, then each setting, its name
    // over what sets it (so the form holds at any width).
    ui.column().grow().scroll().pad((24.0, 16.0)).gap(t.gap * 2.0).open();
    ui.text(SECTIONS[app.section], t.font.bold(), t.text);
    match app.section {
        0 => appearance(app, ui),
        1 => notifications(app, ui),
        2 => account(app, ui),
        _ => about(ui),
    }
    ui.close();
    ui.close();
}

/// Starts a setting: a column with its name in it. What sets it comes next, then
/// `ui.close()`.
fn setting(ui: &mut Ui, name: &str) {
    let t = ui.theme;
    ui.column().gap(4.0).open();
    ui.text(name, t.small, t.text_dim);
}

fn appearance(app: &mut Settings, ui: &mut Ui) {
    setting(ui, "Colours");
    ui.segmented(&mut app.look, &LOOKS);
    ui.close();
    setting(ui, "Colour that stands out");
    ui.wrapping_row().gap(8.0).open();
    let mut named = ACCENTS.iter().position(|(_, rgb)| Color::rgb(*rgb) == app.accent).unwrap_or(usize::MAX);
    if ui.segmented(&mut named, &ACCENTS.map(|(name, _)| name)).changed {
        app.accent = Color::rgb(ACCENTS[named].1);
    }
    ui.color(&mut app.accent);
    ui.close();
    ui.close();
    setting(ui, &format!("Size of letters: {}", app.letters.round()));
    ui.row().max(320.0, f32::INFINITY).open();
    ui.slider(&mut app.letters, 11.0..=18.0);
    ui.close();
    ui.close();
    setting(ui, "Spacing");
    ui.switch(&mut app.close, "Closer together");
    ui.close();
}

fn notifications(app: &mut Settings, ui: &mut Ui) {
    setting(ui, "Notifications");
    ui.switch(&mut app.notify, "Show them");
    ui.close();
    // (What only matters while they are shown is switched off while they are not.)
    ui.set_enabled(app.notify);
    setting(ui, "Sound");
    ui.checkbox(&mut app.sound, "With a sound");
    ui.close();
    setting(ui, "Gather them");
    ui.select(&mut app.gather, &GATHER);
    ui.close();
    setting(ui, &format!("Quiet from {:02}:00 to {:02}:00", app.quiet.0, app.quiet.1));
    ui.row().max(320.0, f32::INFINITY).open();
    ui.range(&mut app.quiet.0, &mut app.quiet.1, 0.0..=24.0);
    (app.quiet.0, app.quiet.1) = (app.quiet.0.round(), app.quiet.1.round());
    ui.close();
    ui.close();
    if ui.button("Send one to try").clicked {
        ui.notice("This is how a notification looks.");
    }
    ui.set_enabled(true);
}

fn account(app: &mut Settings, ui: &mut Ui) {
    setting(ui, "Name");
    ui.text_box(&mut app.name, "Your name");
    ui.close();
    setting(ui, "Email");
    ui.text_box(&mut app.email, "name@example.com");
    ui.close();
    setting(ui, "Days of history to keep");
    ui.number(&mut app.days, 1);
    app.days = app.days.clamp(1, 365);
    ui.close();
    setting(ui, "When something fails");
    ui.checkbox(&mut app.reports, "Send a report");
    ui.close();
}

fn about(ui: &mut Ui) {
    let t = ui.theme;
    setting(ui, "Made with");
    ui.text("taut", t.font.bold(), t.accent);
    ui.close();
    setting(ui, "The system asks for");
    ui.label(if ui.system_dark() { "Dark windows" } else { "Light windows" });
    ui.close();
}

fn main() {
    let mut app = Settings::default();
    let window = Window { title: "Settings", width: 720.0, height: 460.0, min_width: 480.0, min_height: 300.0, ..Default::default() };
    taut::run(window, move |ui| describe(&mut app, ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use taut::Role;
    use taut::testing::Trial;

    #[test]
    fn sections_are_picked_and_the_window_takes_what_is_set() {
        let mut trial = Trial::new(Settings::default(), describe);
        trial.resize(720.0, 400.0, 1.0);
        // As the system asks: light, then dark when the system asks for dark.
        assert_eq!(trial.ui.theme.bg, Theme::light().bg);
        trial.ui.set_wishes(taut::Wishes { dark: true, ..Default::default() });
        trial.run();
        assert_eq!(trial.ui.theme.bg, Theme::dark().bg);
        trial.looks_like("tests/looks/settings-dark.png");
        // Light whatever the system asks; a colour by its name; larger letters.
        trial.press(Role::Tab, "Light");
        trial.press(Role::Tab, "Teal");
        assert_eq!((trial.ui.theme.bg, trial.ui.theme.accent), (Theme::light().bg, Color::rgb(0x1f9e8c)));
        trial.press(Role::Slider, "Size of letters: 13");
        (0..10).for_each(|_| trial.key("Right"));
        assert_eq!((trial.app.letters, trial.ui.theme.font.size), (18.0, 18.0));
        trial.press(Role::Switch, "Closer together");
        assert_eq!(trial.ui.theme.control, 24.0);
        trial.looks_like("tests/looks/settings-light.png");

        // What only matters while notifications are shown is switched off with them.
        trial.press(Role::Row, "Notifications");
        assert!(trial.find(Role::Tick, "With a sound").usable);
        trial.press(Role::Switch, "Show them");
        assert!(!trial.find(Role::Tick, "With a sound").usable);
        trial.press(Role::Switch, "Show them");
        trial.press(Role::Select, "Gather them");
        trial.press(Role::MenuItem, "Once a day");
        assert_eq!(trial.app.gather, 2);
        trial.press(Role::Button, "Send one to try");
        assert!(trial.has(Role::Text, "This is how a notification looks."));
        trial.wait(Duration::from_secs(5));
        assert!(!trial.has(Role::Text, "This is how a notification looks."));

        // The account's boxes and numbers.
        trial.press(Role::Row, "Account");
        trial.fill("Your name", "Ada Lovelace");
        // (A box for a number is told as a spin button, by the words in front of it.)
        trial.press(Role::Spin, "Days of history to keep");
        trial.key("Ctrl+A");
        trial.type_text("400");
        trial.key("Enter");
        assert_eq!((trial.app.name.as_str(), trial.app.days), ("Ada Lovelace", 365));

        // About says what the system asks for.
        trial.press(Role::Row, "About");
        assert!(trial.has(Role::Text, "Dark windows"));
    }
}
