//! The system's own parts, each a button away: its file dialog (a file, several, saving), the
//! clipboard's pictures and files, a notification, a box checked for spelling, faces named by
//! family, printing, opening a file in another app, and dragging out and dropping in. Under
//! each, what came of it. At the top, a number, a day and a time in the person's formats.
//!   cargo run --example system_parts
//!   cargo run --example apk -- system_parts --target x86_64-linux-android
use std::path::PathBuf;
use taut::{Color, Date, DragData, Face, FileDialog, Font, Notification, Paper, PrintJob, PrintOutcome, Time, Ui, Window};

/// What the dialog is up for.
#[derive(Clone, Copy, PartialEq)]
enum Asked {
    Open,
    Save,
}

/// What the app keeps.
pub struct Parts {
    files: FileDialog,
    asked: Asked,
    /// Where the app's own files go.
    folder: PathBuf,
    /// The box checked for spelling.
    note: String,
    notification: Option<Notification>,
    printing: Option<PrintJob>,
    /// What came of each part: the files, the clipboard, the notification, printing, drops.
    said: [String; 5],
}

/// What came of each part, by its place in `said`.
const FILES: usize = 0;
const CLIPBOARD: usize = 1;
const NOTIFIED: usize = 2;
const PRINTED: usize = 3;
const DROPS: usize = 4;

/// A picture of two by two pixels: red, green, blue, white.
const PICTURE: [u32; 4] = [0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff];

pub fn parts() -> Parts {
    let folder = taut::storage::folder("taut-system-parts").unwrap_or_else(std::env::temp_dir);
    Parts { files: FileDialog::native(), asked: Asked::Open, folder, note: "Speling is hard".into(), notification: None, printing: None, said: Default::default() }
}

pub fn describe(parts: &mut Parts, ui: &mut Ui) {
    answers(parts, ui);
    ui.column().pad(12.0).gap(6.0).grow().scroll().open();
    // A number, a day and a time as the person's locale writes them.
    ui.label(&format!("Formats: {} · {} · {}", ui.format_number(1_234_567.5, 1), ui.format_date(Date::new(2026, 3, 7)), ui.format_time(Time::new(14, 5, 0), false)));
    file_buttons(parts, ui);
    clipboard_buttons(parts, ui);
    ui.row().gap(8.0).open();
    if ui.button("Notify").clicked {
        parts.notification = Some(ui.system_notification("Hello", "From the system parts", Some((2, &PICTURE))));
        parts.said[NOTIFIED] = "Notification: posted".into();
    }
    if ui.button("Print").clicked {
        let mut paper = Paper::new();
        paper.page(Paper::A4);
        paper.text("Printed by the system parts", 72.0, 72.0, Font::new(18.0), Color::BLACK);
        parts.printing = Some(taut::print(paper, "System parts"));
        parts.said[PRINTED] = "Printing: the dialog is up".into();
    }
    if ui.button("Open in another app").clicked {
        let page = parts.folder.join("page.html");
        let written = taut::storage::write(&page, "<h1>A page of the system parts</h1>").is_ok();
        parts.said[FILES] = if written { "Files: opened page.html".into() } else { "Files: page.html not written".into() };
        taut::open_with_system(page);
    }
    ui.close();
    ui.label("Spelling:");
    ui.check_spelling(Some("en-US"));
    ui.text_box(&mut parts.note, "Words to check");
    let ink = ui.theme.text;
    for (name, family) in [("Sans", "sans-serif"), ("Serif", "serif"), ("Mono", "monospace")] {
        ui.text(&format!("{name}: Handgloves 123"), Font { face: Face::named(family), ..Font::new(20.0) }, ink);
    }
    drags(parts, ui);
    for said in &parts.said {
        ui.label(said);
    }
    ui.close();
}

/// What the system answered since the last run: the file dialog, a click on the notification,
/// printing.
fn answers(parts: &mut Parts, ui: &mut Ui) {
    if let Some(file) = ui.file_dialog(&mut parts.files) {
        parts.said[FILES] = match parts.asked {
            Asked::Save => std::fs::write(&file, "Saved by the system parts\n").map_or_else(|error| format!("Files: not saved: {error}"), |()| format!("Files: saved {}", name(&file))),
            Asked::Open => {
                let read: Vec<String> = parts.files.chosen.iter().map(|file| format!("{} ({})", name(file), first_line(file))).collect();
                format!("Files: opened {}", read.join(", "))
            }
        };
    }
    if let Some(clicked) = ui.notification_clicked() {
        let ours = parts.notification == Some(clicked);
        parts.said[NOTIFIED] = format!("Notification: clicked{}", if ours { "" } else { ", from before" });
    }
    if let Some(outcome) = parts.printing.as_ref().and_then(PrintJob::outcome) {
        let what = match outcome {
            PrintOutcome::Sent => "sent",
            PrintOutcome::Closed => "closed",
            _ => "unable",
        };
        parts.said[PRINTED] = format!("Printing: {what}");
        parts.printing = None;
    }
}

/// The file dialog's buttons.
fn file_buttons(parts: &mut Parts, ui: &mut Ui) {
    ui.row().gap(8.0).open();
    if ui.button("Open").clicked {
        parts.asked = Asked::Open;
        parts.files.pick(&parts.folder);
    }
    if ui.button("Open several").clicked {
        parts.asked = Asked::Open;
        parts.files.pick_many(&parts.folder);
    }
    if ui.button("Save").clicked {
        parts.asked = Asked::Save;
        parts.files.save(&parts.folder, "notes.txt");
    }
    ui.close();
}

/// The clipboard's buttons: a picture and a file, both ways.
fn clipboard_buttons(parts: &mut Parts, ui: &mut Ui) {
    ui.row().gap(8.0).open();
    if ui.button("Copy picture").clicked {
        ui.copy_picture(2, &PICTURE);
        parts.said[CLIPBOARD] = "Clipboard: picture copied".into();
    }
    if ui.button("Paste picture").clicked {
        parts.said[CLIPBOARD] = match ui.clipboard_picture() {
            Some((width, pixels)) => format!("Clipboard: picture {width}x{}, {}", pixels.len() / width.max(1), if pixels == PICTURE { "as copied" } else { "another" }),
            None => "Clipboard: no picture".into(),
        };
    }
    ui.close();
    ui.row().gap(8.0).open();
    if ui.button("Copy file").clicked {
        let file = parts.folder.join("copied.txt");
        parts.said[CLIPBOARD] = match taut::storage::write(&file, "Copied by the system parts\n") {
            Ok(()) => "Clipboard: file copied".into(),
            Err(error) => format!("Clipboard: file not written: {error}"),
        };
        ui.copy_files(&[file]);
    }
    if ui.button("Paste files").clicked {
        let pasted: Vec<String> = ui.clipboard_files().iter().map(|file| format!("{} ({})", name(file), first_line(file))).collect();
        parts.said[CLIPBOARD] = format!("Clipboard: files {}", pasted.join(", "));
    }
    ui.close();
}

/// A thing to drag out, and a place to drop on.
fn drags(parts: &mut Parts, ui: &mut Ui) {
    ui.row().gap(8.0).open();
    let thing = ui.selectable(false, "Drag me");
    if ui.drag_source(&thing, 1) {
        ui.label("Drag me");
        ui.close();
    }
    if let Some(effect) = ui.drag_out(&thing, DragData::Text("Dragged from the system parts")) {
        parts.said[DROPS] = format!("Drops: taken by another app ({effect:?})");
    }
    let file = ui.selectable(false, "Drag a file");
    let dragged = [parts.folder.join("dragged.txt")];
    if file.pressed && !dragged[0].exists() {
        let _ = taut::storage::write(&dragged[0], "Dragged by the system parts\n");
    }
    if let Some(effect) = ui.drag_out(&file, DragData::Files(&dragged)) {
        parts.said[DROPS] = format!("Drops: file taken by another app ({effect:?})");
    }
    let line = ui.theme.line;
    let target = ui.column().pad(8.0).border(line, 1.0).sense().open();
    ui.label(if ui.files_hovered() || ui.text_hovered() { "Let go here" } else { "Drop here" });
    ui.close();
    if ui.dropped(&target).is_some() {
        parts.said[DROPS] = "Drops: dragged here inside the window".into();
    }
    if let Some(files) = ui.files_dropped(&target) {
        let read: Vec<String> = files.iter().map(|file| format!("{} ({})", name(file), first_line(file))).collect();
        parts.said[DROPS] = format!("Drops: files {}", read.join(", "));
    }
    if let Some(text) = ui.text_dropped(&target) {
        parts.said[DROPS] = format!("Drops: text {text}");
    }
    ui.close();
}

/// A file's name.
fn name(file: &std::path::Path) -> String {
    file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A file's first line, as text.
fn first_line(file: &std::path::Path) -> String {
    std::fs::read_to_string(file).map(|text| text.lines().next().unwrap_or_default().to_string()).unwrap_or_else(|_| "unread".into())
}

fn main() {
    let mut parts = parts();
    taut::run(Window { title: "System parts", width: 420.0, height: 640.0, ..Default::default() }, move |ui| describe(&mut parts, ui));
}

/// Where Android starts the app (it is a library there): see `taut::run`.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
#[allow(clippy::main_recursion)]
extern "C" fn android_main() {
    main()
}
