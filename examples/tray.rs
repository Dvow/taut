//! A program living in the system tray. Its tray icon has a menu; closing its window hides it
//! to the tray, and clicking the icon restores it. A global hotkey (Ctrl+Alt+D) brings it
//! forward from anywhere. It reports events as system notifications, and clicking one brings it
//! forward. Launched again, it forwards its arguments to the running instance, which comes
//! forward.
//!
//!     cargo run -p taut --example tray -- notes.txt
use taut::{Start, Ui, Window};

struct App {
    /// The icon, sixteen pixels a side; whether notifications are kept quiet; the files it
    /// was given, by every start; and what it says in its window.
    icon: Vec<u32>,
    quiet: bool,
    files: Vec<String>,
    said: String,
}

/// The icon: a square of blue with a lighter border.
fn icon() -> Vec<u32> {
    let edge = |at: usize| at == 0 || at == 15;
    (0..256).map(|at| if edge(at % 16) || edge(at / 16) { 0xff8a_b4f8 } else { 0xff1a_73e8 }).collect()
}

/// What a start was given, as files: each named from the folder it was started in.
fn files_of(start: &Start) -> Vec<String> {
    start.args.iter().map(|arg| start.folder.join(arg).display().to_string()).collect()
}

fn describe(app: &mut App, ui: &mut Ui) {
    // The icon in the tray, and its menu.
    if ui.tray(&[(16, &app.icon)], "Tray example").clicked {
        ui.come_forward();
    }
    if ui.tray_item("&Show") {
        ui.come_forward();
    }
    ui.tray_check(&mut app.quiet, "&Quiet");
    ui.tray_line();
    if ui.tray_item("&Quit") {
        ui.quit();
    }
    // Closing the window hides it in the tray instead.
    if ui.close_asked() {
        ui.keep_open("");
        ui.hide_window();
    }
    // A key of the whole system brings it forward, whichever program has the keyboard.
    if ui.hotkey("Ctrl+Alt+D") {
        ui.come_forward();
    }
    // A later start hands over what it was given; the window comes forward by itself.
    for start in ui.started_again() {
        let files = files_of(&start);
        app.said = format!("Started again with {} files", files.len());
        app.files.extend(files);
    }
    // Notifications of the system, and what a click on one does.
    if ui.notification_clicked().is_some() {
        ui.come_forward();
    }

    ui.column().pad(16.0).gap(8.0).open();
    ui.label(if ui.hotkey_taken("Ctrl+Alt+D") { "Ctrl+Alt+D is another program's" } else { "Ctrl+Alt+D brings this window forward" });
    if ui.button("Say it in a notification").clicked && !app.quiet {
        ui.system_notification("Tray example", "This came from the window", None);
    }
    if ui.button("Hide in the tray").clicked {
        ui.hide_window();
    }
    ui.label(&app.said);
    for file in &app.files {
        ui.label(file);
    }
    ui.close();
}

fn main() {
    if !taut::first_copy("org.example.TautTray") {
        return; // the copy that runs has what this start was given
    }
    let here = Start { args: std::env::args_os().skip(1).collect(), folder: std::env::current_dir().unwrap_or_default() };
    let mut app = App { icon: icon(), quiet: false, files: files_of(&here), said: String::new() };
    let icons = [(16, app.icon.clone())];
    let icons: Vec<(usize, &[u32])> = icons.iter().map(|(width, pixels)| (*width, &pixels[..])).collect();
    taut::run(Window { title: "Tray", width: 360.0, height: 240.0, icons: &icons, ..Default::default() }, move |ui| describe(&mut app, ui));
}
