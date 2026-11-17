//! A tray icon (Windows' notification area, a Linux panel's tray): its picture, a hint shown
//! while the pointer rests on it, what a click does, and its menu. The app says it every run,
//! with the menu's rows after it:
//!
//! ```ignore
//! if ui.tray(&[(32, &icon)], "Notes").clicked {
//!     ui.bring_to_front(); // back from the tray
//! }
//! if ui.tray_item("&Show") {
//!     ui.bring_to_front();
//! }
//! ui.tray_checkbox(&mut app.quiet, "&Quiet");
//! ui.tray_separator();
//! if ui.tray_item("&Quit") {
//!     ui.quit();
//! }
//! ```
//!
//! The icon is added after the first run that says it, updated after a run that says it
//! differently, and removed after the first run that does not. A program living in the tray
//! hides its window there ([`Ui::hide_window`]), off the screen and the task bar, and brings it
//! back with [`Ui::bring_to_front`].
//!
//! The menu is the system's own: it is what the person knows from every other tray icon, works
//! with the system's keys and screen readers, and under Wayland is the only kind possible (no
//! program can put a window beside the panel). The system shows the rows the app described
//! last; a chosen row comes back as the next run says that row again. On Windows it is a system
//! menu (`TrackPopupMenu`, in `windows_shell`); on Linux the panel draws it from what the
//! program tells over D-Bus (`com.canonical.dbusmenu`, beside the `StatusNotifierItem` icon, in
//! `freedesktop`).
use crate::ui::{Response, Ui};
use std::cell::RefCell;

/// A tray menu row: one to pick, one with a check mark that picking flips, one of several
/// choices with a dot on the chosen one, or a separator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RowKind {
    Item,
    Tick(bool),
    Choice(bool),
    Line,
}

/// A tray menu row as the app described it: its label (`&` before its access key), its kind,
/// and whether it is enabled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub label: String,
    pub kind: RowKind,
    pub enabled: bool,
}

/// What the person did with the icon: clicked it (or pressed Enter on it), double-clicked it,
/// or chose the menu row at this index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Did {
    Clicked,
    Twice,
    Picked(usize),
}

/// What changed of the icon since the system was last told: pictures, hint, menu.
pub(crate) const PICTURES: u8 = 1;
pub(crate) const HINT: u8 = 2;
pub(crate) const ROWS: u8 = 4;

/// The tray icon as the app says it, and what the person did with it, on this thread.
#[derive(Default)]
pub(crate) struct Tray {
    /// It was described in this run.
    pub described: bool,
    /// Its pictures (each its width and pixels), its hint, its menu.
    pub pictures: Vec<(usize, Vec<u32>)>,
    pub hint: String,
    pub rows: Vec<Row>,
    /// How many rows were described so far in this pass.
    rows_described: usize,
    /// What changed since the system was last told (`PICTURES`, `HINT`, `ROWS`).
    pub changed: u8,
    /// What the person did, until a run takes it.
    pub news: Vec<Did>,
    /// The app asked for its first window to be hidden.
    pub hide: bool,
}

thread_local! {
    pub(crate) static TRAY: RefCell<Tray> = RefCell::new(Tray::default());
}

impl Tray {
    /// After a run: rows the run no longer described leave the menu. Gives back whether the
    /// icon was described and what changed of it, which the system is now told; from here on
    /// nothing has changed and nothing was described. (What the person did that no run took is
    /// dropped.)
    pub(crate) fn settle(&mut self) -> (bool, u8) {
        if self.described && self.rows_described < self.rows.len() {
            self.rows.truncate(self.rows_described);
            self.changed |= ROWS;
        }
        let described = (std::mem::take(&mut self.described), std::mem::take(&mut self.changed));
        self.rows_described = 0;
        self.news.clear();
        described
    }

    /// Takes what the person did, if it was this.
    fn took(&mut self, did: Did) -> bool {
        let at = self.news.iter().position(|news| *news == did);
        at.map(|at| self.news.remove(at)).is_some()
    }
}

/// A tray icon, its menu, and a window hidden there.
impl Ui {
    /// The program's tray icon: its pictures (square, in as many sizes as given, each its width
    /// in pixels and pixels, as [`Window::icons`](crate::Window::icons) takes them; the system
    /// shows the one that fits), and a hint shown while the pointer rests on it. `clicked` when
    /// the person clicks it (or chooses it from the keyboard), `double_clicked` where the
    /// system tells a double click apart (Windows). Say it every run while it should stay; its
    /// menu rows follow ([`tray_item`](Self::tray_item),
    /// [`tray_checkbox`](Self::tray_checkbox), [`tray_radio`](Self::tray_radio),
    /// [`tray_separator`](Self::tray_separator)). Only in a window run by `taut::run`.
    pub fn tray(&mut self, pictures: &[(usize, &[u32])], hint: &str) -> Response {
        crate::platform::hand_desktop();
        let (clicked, double_clicked) = TRAY.with_borrow_mut(|tray| {
            let same = tray.pictures.len() == pictures.len() && tray.pictures.iter().zip(pictures).all(|((width, kept), (wide, now))| width == wide && kept[..] == now[..]);
            if !same {
                tray.pictures = pictures.iter().map(|(width, pixels)| (*width, pixels.to_vec())).collect();
                tray.changed |= PICTURES;
            }
            if tray.hint != hint {
                tray.hint = hint.into();
                tray.changed |= HINT;
            }
            (tray.described, tray.rows_described) = (true, 0);
            (tray.took(Did::Clicked), tray.took(Did::Twice))
        });
        self.again |= clicked | double_clicked;
        Response { clicked, double_clicked, ..Response::default() }
    }

    /// A tray menu row: true once, in the run after the person picked it. (`"&Show"` makes S
    /// its access key. What [`set_enabled(false)`](Self::set_enabled) disabled shows pale and
    /// cannot be chosen.)
    pub fn tray_item(&mut self, label: &str) -> bool {
        self.tray_row(label, RowKind::Item)
    }

    /// A tray menu row with a check mark while `on`: picking it flips that. Says whether it
    /// did.
    pub fn tray_checkbox(&mut self, on: &mut bool, label: &str) -> bool {
        let chosen = self.tray_row(label, RowKind::Tick(*on));
        *on ^= chosen;
        chosen
    }

    /// A tray menu row that is one of several exclusive choices, with a dot while chosen:
    /// picking it sets `choice` to `value`. Says whether that changed `choice`.
    pub fn tray_radio<T: PartialEq>(&mut self, choice: &mut T, value: T, label: &str) -> bool {
        let chosen = self.tray_row(label, RowKind::Choice(*choice == value)) && *choice != value;
        if chosen {
            *choice = value;
        }
        chosen
    }

    /// A separator between tray menu rows.
    pub fn tray_separator(&mut self) {
        self.tray_row("", RowKind::Line);
    }

    /// Hides the first window entirely: off the screen, the task bar and the system's window
    /// lists. What a tray program does when its window is closed or minimized;
    /// [`bring_to_front`](Self::bring_to_front) brings it back.
    /// ([`window_minimized`](Self::window_minimized) says so meanwhile, and the interface still
    /// runs for the icon, global hotkeys and notifications.)
    pub fn hide_window(&mut self) {
        crate::platform::hand_desktop();
        TRAY.with_borrow_mut(|tray| tray.hide = true);
    }

    /// A tray menu row described after the icon: kept as the menu's row at that index (the menu
    /// changes where it differs), and whether the person chose it.
    fn tray_row(&mut self, label: &str, kind: RowKind) -> bool {
        let enabled = self.enabled;
        let chosen = TRAY.with_borrow_mut(|tray| {
            let at = tray.rows_described;
            tray.rows_described += 1;
            let same = tray.rows.get(at).is_some_and(|row| row.label == label && row.kind == kind && row.enabled == enabled);
            if !same {
                tray.rows.truncate(at);
                tray.rows.push(Row { label: label.into(), kind, enabled });
                tray.changed |= ROWS;
            }
            enabled && tray.took(Did::Picked(at))
        });
        self.again |= chosen;
        chosen
    }
}

/// A picture resized to `side` pixels square from one `width` wide: each pixel the
/// alpha-weighted mean of those it covers (so a clear pixel's colour does not bleed at an
/// edge), or where the picture is smaller, the pixel it lies on. Pixels are `0xAARRGGBB`, not
/// premultiplied. For systems that want one exact size.
pub(crate) fn sized(width: usize, pixels: &[u32], side: usize) -> Vec<u32> {
    let (wide, high) = (width.max(1), (pixels.len() / width.max(1)).max(1));
    let mut made = Vec::with_capacity(side * side);
    for y in 0..side {
        let rows = (y * high / side)..((y + 1) * high).div_ceil(side).max(y * high / side + 1);
        for x in 0..side {
            let columns = (x * wide / side)..((x + 1) * wide).div_ceil(side).max(x * wide / side + 1);
            made.push(mean(pixels, wide, rows.clone(), columns));
        }
    }
    made
}

/// The mean of some rows and columns of a picture `wide` pixels wide (outside it, pixels are
/// clear).
fn mean(pixels: &[u32], wide: usize, rows: std::ops::Range<usize>, columns: std::ops::Range<usize>) -> u32 {
    let (mut sums, mut count) = ([0u64; 4], 0u64);
    for y in rows {
        for x in columns.clone() {
            let pixel = pixels.get(y * wide + x).copied().unwrap_or(0) as u64;
            let solid = pixel >> 24;
            sums = [sums[0] + solid, sums[1] + (pixel >> 16 & 0xff) * solid, sums[2] + (pixel >> 8 & 0xff) * solid, sums[3] + (pixel & 0xff) * solid];
            count += 1;
        }
    }
    if sums[0] == 0 {
        return 0;
    }
    let [solid, red, green, blue] = [sums[0] / count, sums[1] / sums[0], sums[2] / sums[0], sums[3] / sums[0]];
    (solid << 24 | red << 16 | green << 8 | blue) as u32
}

/// The icon picture that best fits where the system shows `side` pixels: the smallest at least
/// that wide, else the widest; resized to exactly that.
pub(crate) fn fitting(pictures: &[(usize, Vec<u32>)], side: usize) -> Option<Vec<u32>> {
    let fits = pictures.iter().filter(|(width, _)| *width >= side).min_by_key(|(width, _)| *width);
    let (width, pixels) = fits.or(pictures.iter().max_by_key(|(width, _)| *width))?;
    Some(if *width == side && pixels.len() == side * side { pixels.clone() } else { sized(*width, pixels, side) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Trial;

    /// What the test app keeps: the icon's hint, its two check marks, which of two choices, and
    /// what it heard.
    #[derive(Default)]
    struct App {
        hint: &'static str,
        quiet: bool,
        which: u8,
        more: bool,
        heard: Vec<&'static str>,
    }

    const RED: [u32; 4] = [0xffff_0000; 4];

    fn describe(app: &mut App, ui: &mut Ui) {
        let icon = ui.tray(&[(2, &RED)], app.hint);
        if icon.clicked {
            app.heard.push("clicked");
        }
        if icon.double_clicked {
            app.heard.push("twice");
        }
        if ui.tray_item("&Show") {
            app.heard.push("show");
        }
        if ui.tray_checkbox(&mut app.quiet, "&Quiet") {
            app.heard.push("quiet");
        }
        ui.tray_radio(&mut app.which, 0, "One");
        ui.tray_radio(&mut app.which, 1, "Two");
        if app.more {
            ui.tray_separator();
            ui.set_enabled(false);
            if ui.tray_item("Pale") {
                app.heard.push("pale");
            }
            ui.set_enabled(true);
        }
        ui.label("The window");
    }

    /// One run of the app as a window runs it, and what the windows then tell the system.
    fn run(trial: &mut Trial<App>) -> (bool, u8, Vec<Row>) {
        trial.run();
        let (said, changed) = TRAY.with_borrow_mut(Tray::settle);
        (said, changed, TRAY.with_borrow(|tray| tray.rows.clone()))
    }

    fn rows(labels: &[(&str, RowKind, bool)]) -> Vec<Row> {
        labels.iter().map(|(label, kind, enabled)| Row { label: label.to_string(), kind: *kind, enabled: *enabled }).collect()
    }

    #[test]
    fn the_icon_and_its_menu_are_what_the_last_run_said_and_only_changes_are_news() {
        TRAY.set(Tray::default());
        let mut trial = Trial::new(App { hint: "Notes", ..App::default() }, describe);
        let (said, changed, menu) = run(&mut trial);
        assert!(said && changed == PICTURES | HINT | ROWS);
        let first = rows(&[("&Show", RowKind::Item, true), ("&Quiet", RowKind::Tick(false), true), ("One", RowKind::Choice(true), true), ("Two", RowKind::Choice(false), true)]);
        assert_eq!(menu, first);
        assert_eq!(TRAY.with_borrow(|tray| tray.pictures.clone()), [(2, RED.to_vec())]);
        // The same again is no news.
        assert_eq!(run(&mut trial).1, 0);
        // A new hint is; so are rows that come and go.
        trial.app.hint = "Notes: 2 new";
        assert_eq!(run(&mut trial).1, HINT);
        trial.app.more = true;
        let (_, changed, menu) = run(&mut trial);
        assert_eq!((changed, menu.len()), (ROWS, 6));
        assert_eq!(menu[4..], rows(&[("", RowKind::Line, true), ("Pale", RowKind::Item, false)]));
        trial.app.more = false;
        assert_eq!(run(&mut trial), (true, ROWS, first));
    }

    #[test]
    fn what_the_person_does_with_the_icon_comes_back_once_to_the_run_that_asks() {
        TRAY.set(Tray::default());
        let mut trial = Trial::new(App { hint: "Notes", more: true, ..App::default() }, describe);
        run(&mut trial);
        // A click, a double click, the first row, the check mark, the second choice, a disabled
        // row.
        let news = [Did::Clicked, Did::Twice, Did::Picked(0), Did::Picked(1), Did::Picked(3), Did::Picked(5)];
        TRAY.with_borrow_mut(|tray| tray.news.extend(news));
        let (_, changed, menu) = run(&mut trial);
        assert_eq!(trial.app.heard, ["clicked", "twice", "show", "quiet"], "each once, and nothing from a row that is off");
        assert!(trial.app.quiet && trial.app.which == 1);
        // The menu shows the check mark and the dot where they are now.
        assert_eq!((changed, menu[1].kind, menu[2].kind, menu[3].kind), (ROWS, RowKind::Tick(true), RowKind::Choice(false), RowKind::Choice(true)));
        // What no run took is dropped.
        assert!(TRAY.with_borrow(|tray| tray.news.is_empty()));
        // A run that does not say the icon removes it.
        let mut none = Trial::new((), |_, ui| _ = ui.label("Nothing"));
        none.run();
        assert!(!TRAY.with_borrow_mut(Tray::settle).0);
    }

    #[test]
    fn a_picture_is_made_the_size_the_system_asks_for() {
        // Four by four, left half solid red, right half clear (green underneath): two by two
        // keeps the halves, and the clear pixels' colour shows nowhere.
        let red_and_clear: Vec<u32> = (0..16).map(|at| if at % 4 < 2 { 0xffff_0000 } else { 0x0000_ff00 }).collect();
        assert_eq!(sized(4, &red_and_clear, 2), [0xffff_0000, 0, 0xffff_0000, 0]);
        // One by one is the mean: half opaque, all red.
        assert_eq!(sized(4, &red_and_clear, 1), [0x7fff_0000]);
        // Larger, each pixel is the one it lies on.
        assert_eq!(sized(1, &[0x8012_3456], 3), [0x8012_3456; 9]);
        // Of several, the smallest wide enough, else the widest.
        let pictures = vec![(1, vec![1]), (4, red_and_clear.clone()), (2, vec![2; 4])];
        assert_eq!(fitting(&pictures, 2), Some(vec![2; 4]));
        assert_eq!(fitting(&pictures, 3).map(|made| made.len()), Some(9));
        assert_eq!(fitting(&pictures, 8).map(|made| made[0]), Some(0xffff_0000));
        assert_eq!(fitting(&[], 16), None);
    }
}
