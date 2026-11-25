//! A note kept between visits, opened from a file and saved as one: the same code on the
//! desktop and in a browser, where it is kept in the page's storage, opened with the
//! browser's file picker and saved where the person picks (Chrome, Edge) or as a download.
//!   cargo run --example web_notes
//!   cargo build --release --target wasm32-unknown-unknown --example web_notes
//!   cargo run --example web_page -- target/wasm32-unknown-unknown/release/examples/web_notes.wasm
use std::path::PathBuf;
use taut::{FileDialog, Ui, Window};

/// The note, where it is kept, and the file dialog.
pub struct Notes {
    pub text: String,
    kept: Option<PathBuf>,
    files: FileDialog,
    /// The dialog is up to save, not to open.
    saving: bool,
    /// What was last done, shown under the buttons.
    pub said: String,
}

/// The note as last kept, if it was.
pub fn notes() -> Notes {
    let kept = taut::storage::folder("taut-notes").map(|folder| folder.join("note.txt"));
    let text = kept.as_deref().and_then(taut::storage::read).unwrap_or_default();
    Notes { text, kept, files: FileDialog::native(), saving: false, said: String::new() }
}

pub fn describe(notes: &mut Notes, ui: &mut Ui) {
    // The file dialog's answer first: the note below shows what was opened.
    if let Some(file) = ui.file_dialog(&mut notes.files) {
        notes.said = match notes.saving {
            true => taut::storage::write(&file, &notes.text).map_or_else(|error| format!("Not saved: {error}"), |()| format!("Saved as {}", file.display())),
            false => taut::storage::read(&file).map_or("Not text".into(), |text| {
                notes.text = text;
                format!("Opened {}", file.display())
            }),
        };
        ui.announce(&notes.said);
    }
    ui.column().pad(16.0).gap(10.0).grow().open();
    ui.label("A note, kept between visits");
    ui.text_area(&mut notes.text, "Write something", (4, 12));
    ui.row().gap(8.0).open();
    if ui.button("Keep").clicked {
        let kept = notes.kept.as_deref().map(|file| taut::storage::write(file, &notes.text));
        notes.said = if matches!(kept, Some(Ok(()))) { "Kept".into() } else { "Not kept: nowhere to keep it".into() };
        ui.announce(&notes.said);
    }
    if ui.button("Open a file\u{2026}").clicked {
        notes.saving = false;
        notes.files.pick(std::env::home_dir().unwrap_or_default());
    }
    if ui.button("Save as\u{2026}").clicked {
        notes.saving = true;
        notes.files.save(std::env::home_dir().unwrap_or_default(), "note.txt");
    }
    if ui.link("taut's source").clicked {
        taut::open_with_system("https://github.com/Dvow/taut");
    }
    ui.close();
    ui.label(&notes.said);
    ui.close();
}

fn main() {
    let mut notes = notes();
    taut::run(Window { title: "Notes", width: 420.0, height: 300.0, ..Default::default() }, move |ui| describe(&mut notes, ui));
}
