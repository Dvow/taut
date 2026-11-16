//! A picture viewer: a folder's pictures as a thumbnail strip along the bottom, the selected
//! one large above it, zoomed and panned by dragging. It shows every format the library reads:
//! PNG, JPEG, GIF (animated), WebP, BMP, ICO, and SVG (sharp at any size).
//!   cargo run -p taut --example viewer [-- <folder>]
//! Keys: Left and Right step through pictures, Home and End to the first and last; Ctrl+Up and
//! Ctrl+Down zoom (as do the wheel and two fingers), F fits the picture to the window, 1 shows
//! it at its own size.
#![windows_subsystem = "windows"]
use std::path::{Path, PathBuf};
use taut::{Align, Color, Drawing, Film, Flow, Icon, Key, Length, Mods, Picture, Role, Ui, Window};

const SMALLER: Icon = Icon::line("M3.5 8h9");
const LARGER: Icon = Icon::line("M8 3.5v9M3.5 8h9");
/// The endings of the files that are shown.
const ENDINGS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"];
/// How far one step zooms, and the least and the most it may be.
const STEP: f32 = 1.25;
const ZOOMS: (f32, f32) = (0.05, 32.0);

/// What a picture is shown with: a still one, one that moves, or a drawing.
#[derive(Clone, Copy)]
enum Shown {
    Still(Picture),
    Moving(Film),
    Drawn(Drawing),
}

/// One picture of the folder: its name, its kind, how many pixels wide and high it is of
/// itself, and what shows it.
pub struct Shot {
    name: String,
    kind: &'static str,
    size: (f32, f32),
    shown: Shown,
}

/// Everything the viewer keeps.
pub struct Viewer {
    folder: PathBuf,
    /// The files that are still to be read: they are read into the interface at its first
    /// run, which is where pictures are kept.
    files: Vec<PathBuf>,
    shots: Vec<Shot>,
    picked: usize,
    /// How large the picture is shown (1: a pixel of it to a unit), or none while it fits
    /// the window; and how far it has been moved from the middle.
    zoom: Option<f32>,
    moved: (f32, f32),
    /// The zoom the picture fits the room it has at (but for a small one, never larger than
    /// it is).
    fitted: f32,
}

impl Viewer {
    /// A viewer of the pictures in `folder`, in the order of their names.
    pub fn new(folder: PathBuf) -> Viewer {
        let known = |path: &PathBuf| path.extension().and_then(|ending| ending.to_str()).is_some_and(|ending| ENDINGS.contains(&ending.to_lowercase().as_str()));
        let mut files: Vec<PathBuf> = std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(known).collect();
        files.sort();
        Viewer { folder, files, shots: Vec::new(), picked: 0, zoom: None, moved: (0.0, 0.0), fitted: 1.0 }
    }

    /// Goes to another picture, fitted to the window.
    fn pick(&mut self, picked: usize) {
        (self.picked, self.zoom, self.moved) = (picked.min(self.shots.len().saturating_sub(1)), None, (0.0, 0.0));
    }
}

/// Reads a picture file into the interface, and asks it how large the picture is. Nothing,
/// for a file that is none it can read.
fn read(path: &Path, ui: &mut Ui) -> Option<Shot> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    // (What kind it is, to say so: as its first bytes have it.)
    let kind = match bytes.as_slice() {
        [0xFF, 0xD8, ..] => "JPEG",
        [b'R', b'I', b'F', b'F', ..] => "WebP",
        [b'B', b'M', ..] => "BMP",
        [0, 0, 1, 0, ..] => "ICO",
        [0x89, b'P', b'N', b'G', ..] => "PNG",
        [b'G', b'I', b'F', ..] => "GIF",
        _ => "SVG",
    };
    let shown = match kind {
        "GIF" => Shown::Moving(ui.add_film(&bytes)?),
        "SVG" => Shown::Drawn(ui.add_drawing(std::str::from_utf8(&bytes).ok()?)?),
        _ => Shown::Still(ui.add_picture_file(&bytes)?),
    };
    let size = match shown {
        Shown::Still(picture) => ui.picture_size(picture).map(|(wide, high)| [wide as f32, high as f32]),
        Shown::Moving(film) => ui.film_size(film).map(|(wide, high)| [wide as f32, high as f32]),
        Shown::Drawn(drawing) => ui.drawing_size(drawing),
    }?;
    Some(Shot { name, kind, size: (size[0], size[1]), shown })
}

/// A box that shows a picture, at the size it is said to have.
fn holding(ui: &mut Ui, shown: Shown, color: taut::Color) -> taut::El<'_> {
    match shown {
        Shown::Still(picture) => ui.el().picture(picture),
        Shown::Moving(film) => ui.el().film(film),
        Shown::Drawn(drawing) => ui.el().drawing(drawing, color),
    }
}

pub fn describe(app: &mut Viewer, ui: &mut Ui) {
    let t = ui.theme;
    for file in std::mem::take(&mut app.files) {
        app.shots.extend(read(&file, ui));
    }
    keys(app, ui);
    ui.column().grow().open();
    bar(app, ui);

    // The picture that is picked, in the middle of the room, as large as it is zoomed to, and
    // moved by as much as it has been dragged. While it fits, it is as large as fits (the
    // room it has is this run's, as it comes out of layout). The wheel and two fingers zoom.
    let view = ui.el().flow(Flow::Layers).grow().clip().fill(Color::BLACK.toward(t.bg, 0.6)).sense().open();
    let room = ui.room();
    if let Some((size, shown)) = app.shots.get(app.picked).map(|shot| (shot.size, shot.shown)) {
        app.fitted = (room.w / size.0).min(room.h / size.1).min(1.0);
        if let Some([_, down]) = ui.wheel(&view) {
            zoom_by(app, if down < 0.0 { STEP } else { 1.0 / STEP });
        }
        if let Some(zoom) = ui.zoom(&view) {
            zoom_by(app, zoom.by);
        }
        let zoom = app.zoom.unwrap_or(app.fitted);
        let (wide, high) = (size.0 * zoom, size.1 * zoom);
        if let Some((gone, caught)) = ui.held(&view, [app.moved.0, app.moved.1, 0.0, 0.0]) {
            app.moved = (caught[0] + gone.x, caught[1] + gone.y);
        }
        holding(ui, shown, t.text).place(Align::Center).at(app.moved.0, app.moved.1).size(wide, high).show();
    } else {
        ui.row().place(Align::Center).open();
        ui.text(&format!("There are no pictures in {}", app.folder.display()), t.font, t.text_faint);
        ui.close();
    }
    ui.close();
    strip(app, ui);
    ui.close();
}

/// The bar over the picture: its name, kind and size, and how large it is shown.
fn bar(app: &mut Viewer, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad((10.0, 6.0)).gap(8.0).fill(t.panel).align(Align::Start, Align::Center).open();
    if let Some(shot) = app.shots.get(app.picked) {
        ui.text(&shot.name, t.font.bold(), t.text);
        ui.note(&format!("{}, {} by {}", shot.kind, shot.size.0, shot.size.1));
    }
    ui.spacer();
    let zoom = app.zoom.map_or("Fitted".to_string(), |zoom| format!("{:.0}%", zoom * 100.0));
    ui.note(&zoom);
    if ui.icon_button(SMALLER, "Smaller").clicked {
        zoom_by(app, 1.0 / STEP);
    }
    if ui.icon_button(LARGER, "Larger").clicked {
        zoom_by(app, STEP);
    }
    if ui.button("Fit").clicked {
        app.pick(app.picked);
    }
    if ui.button("1:1").clicked {
        app.zoom = Some(1.0);
    }
    ui.close();
}

/// Zooms by a step, from what shows now: about the middle of the room.
fn zoom_by(app: &mut Viewer, by: f32) {
    // (While it fits, it shows at the zoom it fits at; the last run said what that was.)
    let now = app.zoom.unwrap_or(app.fitted);
    app.zoom = Some((now * by).clamp(ZOOMS.0, ZOOMS.1));
    app.moved = (app.moved.0 * by, app.moved.1 * by);
}

/// The strip of small pictures along the bottom. A click on one picks it.
fn strip(app: &mut Viewer, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().h(Length::Px(92.0)).pad(8.0).gap(6.0).fill(t.panel).scroll_x().open();
    let mut clicked = None;
    for (number, shot) in app.shots.iter().enumerate() {
        let picked = number == app.picked;
        let edge = if picked { t.accent } else { t.line };
        let small = ui.el().key(&shot.name).size(72.0, 72.0).flow(Flow::Layers).border(edge, 2.0).radius(4.0).sense().focusable().open();
        holding(ui, shot.shown, t.text).place(Align::Center).size(64.0, 64.0).keep_shape().show();
        ui.close();
        ui.tell(&small, Role::Row, Some(picked), &shot.name);
        clicked = clicked.or(small.clicked.then_some(number));
    }
    ui.close();
    if let Some(number) = clicked {
        app.pick(number);
    }
}

/// The keys of the viewer.
fn keys(app: &mut Viewer, ui: &mut Ui) {
    let last = app.shots.len().saturating_sub(1);
    let to = match () {
        _ if ui.key(Key::Right, Mods::NONE) => Some((app.picked + 1).min(last)),
        _ if ui.key(Key::Left, Mods::NONE) => Some(app.picked.saturating_sub(1)),
        _ if ui.key(Key::Home, Mods::NONE) => Some(0),
        _ if ui.key(Key::End, Mods::NONE) => Some(last),
        _ => None,
    };
    if let Some(to) = to {
        app.pick(to);
    }
    if ui.keys("Ctrl+Up") {
        zoom_by(app, STEP);
    }
    if ui.keys("Ctrl+Down") {
        zoom_by(app, 1.0 / STEP);
    }
    if ui.keys("F") {
        app.pick(app.picked);
    }
    if ui.keys("1") {
        app.zoom = Some(1.0);
    }
}

fn main() {
    let folder = std::env::args_os().nth(1).map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    let mut app = Viewer::new(folder);
    let window = Window { title: "Pictures", width: 900.0, height: 640.0, min_width: 360.0, min_height: 280.0, ..Default::default() };
    taut::run(window, move |ui| describe(&mut app, ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::testing::Trial;

    /// A folder with a picture of each kind the library reads, and a file that is none.
    fn made() -> PathBuf {
        let folder = std::env::temp_dir().join(format!("taut-viewer-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let pictures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        for (from, to) in [
            ("inks.jpg", "a-inks.jpg"),
            ("moving.gif", "b-moving.gif"),
            ("logo.svg", "c-logo.svg"),
            ("lossy.webp", "d-lossy.webp"),
            ("bmp-24.bmp", "e-bmp.bmp"),
            ("icon.ico", "f-icon.ico"),
            ("passes.png", "g-passes.png"),
        ] {
            std::fs::copy(pictures.join(from), folder.join(to)).unwrap();
        }
        std::fs::write(folder.join("notes.txt"), "not a picture").unwrap();
        folder
    }

    #[test]
    fn pictures_of_every_kind_are_shown_picked_zoomed_and_moved() {
        let folder = made();
        let mut trial = Trial::new(Viewer::new(folder.clone()), describe);
        trial.resize(640.0, 420.0, 1.0);
        let kinds: Vec<&str> = trial.app.shots.iter().map(|shot| shot.kind).collect();
        assert_eq!(kinds, ["JPEG", "GIF", "SVG", "WebP", "BMP", "ICO", "PNG"], "every picture, and nothing else");
        assert!(trial.has(Role::Text, "a-inks.jpg") && trial.has(Role::Row, "g-passes.png"));
        trial.looks_like("tests/looks/viewer.png");

        // Picked from the strip, and by the keys.
        trial.press(Role::Row, "c-logo.svg");
        assert_eq!(trial.app.picked, 2);
        trial.key("Right");
        trial.key("Right");
        assert!(trial.has(Role::Text, "e-bmp.bmp") && trial.find(Role::Row, "e-bmp.bmp").on == Some(true));
        trial.key("End");
        assert_eq!(trial.app.picked, 6);
        trial.key("Home");

        // Zoomed in by a step from where it fits, then shown at its own size; dragged about;
        // and fitted again, back in the middle.
        let fitted = trial.app.fitted;
        trial.press(Role::Button, "Larger");
        assert_eq!(trial.app.zoom, Some(fitted * STEP));
        // The wheel zooms too, over the picture: in, and out again.
        let middle = trial.spot(Role::Text, "a-inks.jpg");
        let over = taut::geom::pt(middle.x + 200.0, 200.0);
        trial.wheel(over, 0.0, -48.0);
        assert_eq!(trial.app.zoom, Some(fitted * STEP * STEP));
        trial.wheel(over, 0.0, 48.0);
        assert_eq!(trial.app.zoom, Some(fitted * STEP));
        trial.key("1");
        assert!(trial.has(Role::Text, "100%"));
        let middle = trial.spot(Role::Text, "a-inks.jpg");
        let at = taut::geom::pt(middle.x + 200.0, 200.0);
        trial.drag(at, taut::geom::pt(at.x - 60.0, at.y + 30.0));
        assert_eq!(trial.app.moved, (-60.0, 30.0));
        trial.press(Role::Button, "Fit");
        assert!(trial.app.zoom.is_none() && trial.app.moved == (0.0, 0.0) && trial.has(Role::Text, "Fitted"));

        // The moving one moves: its picture changes as time passes.
        trial.press(Role::Row, "b-moving.gif");
        let before = trial.screen.pixels.clone();
        trial.wait(std::time::Duration::from_millis(500));
        assert!(trial.screen.pixels != before, "the film went on");
        _ = std::fs::remove_dir_all(&folder);
    }
}
