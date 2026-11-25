//! File dialog built from the library's own parts: the same on every system, and available
//! where no system dialog is (another renderer, a DLL, a Linux desktop without a portal, the
//! library's own full-screen desktop).
//!
//! The familiar layout: back, forward and up buttons and a clickable path (or a typed one); a
//! sidebar of places (the user's folders as the system names them, the app's own, recent
//! folders, drives and volumes); the folder as a table sortable by name, size, type or date, or
//! as a thumbnail grid of images; type-ahead search; new folder in place; a name box and
//! buttons. Saving, the name is pre-selected without its extension, overwriting asks first, and
//! the requested extension is added. Multiple selection with Ctrl and Shift; folder picking.
//!
//! A folder is read once on its own thread (see `listing`) and shown while it loads: 100,000
//! files cost a dialog run what a screenful does.
//!
//! Fully keyboard accessible: Tab through everything, arrows through list and grid, typed
//! letters jump to names, and the system dialogs' shortcuts (Alt+arrows back, forward and up;
//! Backspace up; Ctrl+L type a path; Ctrl+F search; Ctrl+H hidden files; F5 reload;
//! Ctrl+Shift+N new folder; Ctrl+A select all). Screen readers get every part: the list as a
//! table, grid and places as selectable rows, the path as a button group, errors as they
//! happen.
//!
//! ```ignore
//! // in the app's state:  files: Files
//! if ui.button("Open…").clicked {
//!     app.files.pick(".");
//! }
//! if let Some(path) = ui.file_dialog(&mut app.files) {
//!     open(path);
//! }
//! ```
use crate::accessibility::Role;
use crate::event::{Key, Mods};
use crate::folder_listing::{By, Listing, Reader, Small, Wanted, ending, ends_in};
use crate::layout::{Align, Length};
use crate::places::Place;
use crate::table::{Column, Table};
use crate::ui::{Icon, Picture, Response, Stroke, Ui};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FOLDER: Icon = Icon::line("M2 4.5h4l1.5 1.5H14v6.5H2z");
const PAGE: Icon = Icon::line("M4 2h5l3 3v9H4zM9 2v3h3");
const PICTURE: Icon = Icon::line("M2.5 3.5h11v9h-11zM2.5 10.5l3-3 3 3 2-2 3 3");
const BACK: Icon = Icon::line("M13 8H3.5M7.5 4 3.5 8l4 4");
const AHEAD: Icon = Icon::line("M3 8h9.5M8.5 4l4 4-4 4");
const UP: Icon = Icon::line("M8 13V3.5M4 7.5l4-4 4 4");
const SEARCH: Icon = Icon::line("M7 2.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9zM10.3 10.3l3.2 3.2");
const TYPE: Icon = Icon::line("M11 2.5 13.5 5 6 12.5H3.5V10z");
const NEW_FOLDER: Icon = Icon::line("M2 4.5h4l1.5 1.5H14v6.5H2zM8 7.5v4M6 9.5h4");
const VIEW: Icon = Icon::line("M2.5 4.5h11M2.5 8h11M2.5 11.5h11");
const PLACES: Icon = Icon::line("M8 2.5l1.7 3.5 3.8.5-2.8 2.6.7 3.8L8 11.1l-3.4 1.8.7-3.8-2.8-2.6 3.8-.5z");
const HOME: Icon = Icon::line("M2.5 7.5 8 3l5.5 4.5M4 6.5V13h8V6.5");
const DRIVE: Icon = Icon::line("M2 9.5h12v3H2zM3.5 9.5l1.5-5h6l1.5 5");
const RECENT: Icon = Icon::line("M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11zM8 5v3l2 1.5");
const BETWEEN: Icon = Icon::line("M6.5 4 10.5 8 6.5 12");

/// Minimum list row height (more if the text needs it); grid cell width, and its thumbnail;
/// sidebar width.
const ROW: f32 = 24.0;
const CELL: f32 = 100.0;
const SMALL: f32 = 64.0;
const SIDE: f32 = 148.0;
/// The list's minimum name column width, and the other columns' widths. The room the list needs
/// for names and types; with sizes; with dates too. (Each column adds 13 between it and the
/// next, and the row 16 at its ends.)
const WITH_KINDS: f32 = 140.0 + 64.0 + 2.0 * 13.0 + 16.0;
const WITH_SIZES: f32 = WITH_KINDS + 80.0 + 13.0;
const WITH_DATES: f32 = WITH_SIZES + 128.0 + 13.0;
/// Polling interval while a folder or thumbnail is being read.
const LOOK_AGAIN: Duration = Duration::from_millis(40);
/// Letters typed on the list within this interval form one name; after a pause, a new letter
/// starts over.
const TYPED_TOGETHER: Duration = Duration::from_millis(900);
/// Most thumbnails cached: beyond this, those out of view are dropped.
const KEPT_PICTURES: usize = 256;
/// Most recent folders remembered.
const RECENT_KEPT: usize = 8;
/// Alt, and Ctrl+Shift, as modifiers.
const ALT: Mods = Mods { alt: true, ..Mods::NONE };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

/// The file dialog's words: English (with the feature `translations`, the user's language)
/// until the app sets its own in `files.words`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileWords {
    /// Title when opening a file.
    pub open_a_file: &'static str,
    /// Title when opening several files.
    pub open_files: &'static str,
    /// Title when saving.
    pub save_as: &'static str,
    /// Title when picking a folder.
    pub pick_a_folder: &'static str,
    /// Back button.
    pub back: &'static str,
    /// Forward button.
    pub ahead: &'static str,
    /// Up button.
    pub up: &'static str,
    /// The path, as a group.
    pub path: &'static str,
    /// The button to type a path.
    pub type_path: &'static str,
    /// The search box, and the button that opens it.
    pub search: &'static str,
    /// The sidebar, and the button listing its places when there is no room for it.
    pub places: &'static str,
    /// The user's home folder, among the places.
    pub home: &'static str,
    /// Heading over recent folders.
    pub recent: &'static str,
    /// Heading over drives and volumes; on Windows, also what is above a drive's root.
    pub drives: &'static str,
    /// The new folder button, and a new folder's initial name.
    pub new_folder: &'static str,
    /// The view options button.
    pub view: &'static str,
    /// The option to show hidden files.
    pub hidden: &'static str,
    /// The option to show image thumbnails in a grid.
    pub grid: &'static str,
    /// Cancel button.
    pub cancel: &'static str,
    /// Open button.
    pub open: &'static str,
    /// Save button.
    pub save: &'static str,
    /// The button that picks a folder: the one selected in the list, or with none, the one
    /// shown.
    pub choose: &'static str,
    /// The name box's placeholder, when a file is asked for.
    pub name_hint: &'static str,
    /// The same, when a folder is asked for.
    pub folder_hint: &'static str,
    /// Column headings: name.
    pub name: &'static str,
    /// Size.
    pub size: &'static str,
    /// Type.
    pub kind: &'static str,
    /// Date.
    pub date: &'static str,
    /// A folder's type.
    pub folder: &'static str,
    /// Shown while a folder loads.
    pub reading: &'static str,
    /// When no name matches the search.
    pub none_found: &'static str,
    /// When a name names nothing that exists.
    pub not_there: &'static str,
    /// The overwrite question (after the file's name), and its button.
    pub there_already: &'static str,
    /// The overwrite button.
    pub replace: &'static str,
}

impl Default for FileWords {
    fn default() -> Self {
        #[cfg(feature = "translations")]
        return FileWords::users();
        #[cfg(not(feature = "translations"))]
        FileWords::english()
    }
}

impl FileWords {
    /// The words in English.
    pub(crate) fn english() -> Self {
        FileWords {
            open_a_file: "Open a file",
            open_files: "Open files",
            save_as: "Save as",
            pick_a_folder: "Pick a folder",
            back: "Back",
            ahead: "Forward",
            up: "Up",
            path: "Where",
            type_path: "Type a path",
            search: "Search",
            places: "Places",
            home: "Home",
            recent: "Recent",
            drives: "Drives",
            new_folder: "New folder",
            view: "View",
            hidden: "Hidden files",
            grid: "Small pictures",
            cancel: "Cancel",
            open: "Open",
            save: "Save",
            choose: "Choose this folder",
            name_hint: "The name of a file",
            folder_hint: "A folder in this one, or none for this one itself",
            name: "Name",
            size: "Size",
            kind: "Kind",
            date: "Date",
            folder: "Folder",
            reading: "Reading the folder…",
            none_found: "Nothing has that in its name",
            not_there: "There is nothing of this name",
            there_already: "is there already. Put the new one in its place?",
            replace: "Replace",
        }
    }
}

/// A request for the system's file dialog, for the window's host, which asks the system.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDialogRequest {
    /// Saving, not opening.
    pub saving: bool,
    /// A folder is asked for, not a file.
    pub folders: bool,
    /// The starting folder.
    pub folder: PathBuf,
    /// The starting name.
    pub name: String,
    /// Only files ending in one of these are listed; all if empty.
    pub endings: Vec<String>,
    /// The title.
    pub title: &'static str,
    /// Several files may be chosen ([`FileDialog::pick_many`]): asked of the system on iOS and
    /// Android; elsewhere the library's dialog picks several.
    pub many: bool,
}

/// The library's own dialog: what [`Ui::file_dialog`] shows instead of the system's.
pub(crate) type Own = fn(&mut Ui, &mut FileDialog) -> Option<PathBuf>;

/// A file dialog as the app keeps it: one per app, shown when `pick`, `pick_many`, `save` or
/// `pick_folder` asks.
pub struct FileDialog {
    /// Show the system's dialog instead of the library's where the window can: the one users
    /// know from every other program. Where it cannot (another renderer, a DLL, a Linux desktop
    /// without a portal; several files, but on iOS and Android; a folder, on Android), the
    /// library's is shown, if this dialog has it (see `native`). Answers come back the same way
    /// either way.
    pub native: bool,
    /// The library's own dialog, where this one may show it (see `native`): set only by
    /// what may show it. So on Windows a program whose dialogs are all the system's does not
    /// carry the library's.
    own: Option<Own>,
    /// The folder shown.
    pub folder: PathBuf,
    /// The name box's text: the selected file, or what was typed. (Several selected files are
    /// each in quotes.)
    pub name: String,
    /// Shown.
    pub shown: bool,
    /// Its words.
    pub words: FileWords,
    /// Every file chosen at the last answer: the one returned, or after `pick_many`, all
    /// selected.
    pub chosen: Vec<PathBuf>,
    /// The folders files were last chosen in, latest first: offered among the places. The app
    /// can save and restore them between runs.
    pub recent: Vec<PathBuf>,
    /// The app's own places, each with a name: offered first.
    pub own_places: Vec<(String, PathBuf)>,
    /// Hidden files show (the user toggles it with Ctrl+H, or in the view menu).
    pub show_hidden: bool,
    /// The folder shows as a thumbnail grid, not a table: if the app enabled thumbnails
    /// (`show_pictures`).
    pub grid: bool,
    /// Saving, not opening: a name that is no file yet will do.
    saving: bool,
    /// A folder is asked for, not a file: only folders are listed.
    folders: bool,
    /// Several files can be selected.
    many: bool,
    /// Only files ending in one of these are listed; all if empty. Saving, a name ending in
    /// none of them gets the first.
    endings: Vec<String>,
    /// The folder's contents as far as read; and whether to read it again (another folder, or a
    /// reload).
    listing: Listing,
    stale: bool,
    /// The list's sort and column widths as the user left them; and whether the user chose the
    /// visible columns (until then, a list short of room drops dates, then sizes).
    table: Table,
    columns_chosen: bool,
    /// The sidebar places: the app's own, the user's folders, drives and volumes (recent
    /// folders are the app's, above); and whether to find them again on showing (just asked
    /// for).
    places: Vec<Place>,
    drives: Vec<Place>,
    places_due: bool,
    /// Folders shown before this one, and those gone back from.
    back: Vec<PathBuf>,
    ahead: Vec<PathBuf>,
    /// The search text, and whether the search box shows (instead of the path).
    seek: String,
    searching: bool,
    /// The path being typed, if it is (instead of the path buttons).
    typing_path: Option<String>,
    /// The name of a folder being created, if one is (its own row, first in the list).
    making: Option<String>,
    /// The file that would be overwritten, while the user is asked.
    asking: Option<PathBuf>,
    /// Something just shown is to get focus: the name box (just opened), the search box, the
    /// path box, the new folder's name box, or the overwrite question.
    fresh: Fresh,
    /// The last error (a folder that cannot be read or created, a name that names nothing), as
    /// the system words it.
    trouble: String,
    trouble_new: bool,
    /// How many folders have been shown: each folder's list is its own, starting at its top.
    visits: u32,
    /// Focus at the end of the last run (so a row it just reached is known as such); the row
    /// focus goes to once in view; the letters typed on the list so far, and when the last was.
    focus_was: crate::id::Id,
    /// What had focus when the dialog opened: it gets it back when the dialog closes. The same
    /// for the overwrite question.
    opened_from: Option<crate::id::Id>,
    asked_from: crate::id::Id,
    jump: Option<usize>,
    /// Once the folder is read, focus goes to the item of this name (or the first, for none):
    /// where it was in the previous folder's list.
    onto: Option<String>,
    typed: (String, Duration),
    /// The image reader, if the app enabled thumbnails; its thread; and the cached thumbnails,
    /// with their item and shown size.
    read: Option<Reader>,
    small: Option<Small>,
    pictures: Vec<(u32, Picture, [f32; 2])>,
}

/// What gets focus next run, having just appeared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Fresh {
    #[default]
    Nothing,
    Name,
    Search,
    Path,
    NewFolder,
    Question,
}

/// Where the dialog goes.
enum Go {
    /// This folder.
    To(PathBuf),
    /// Back to the previous folder.
    Back,
    /// Forward to the folder gone back from.
    Ahead,
}

/// What the user did in the list (or grid) in one run.
#[derive(Default)]
struct Did {
    /// The focused item's row, if any.
    on: Option<usize>,
    /// An item was double-clicked or had Enter pressed: its index.
    taken: Option<u32>,
    /// The selection changed.
    picked: bool,
    /// The image files in view.
    pictures: Vec<u32>,
}

/// What the dialog's parts did in one run.
struct DialogParts {
    /// Where the bar, the places or the buttons asked to go.
    went: Option<Go>,
    did: Did,
    /// The name box.
    name: Response,
    /// The choose (true) or cancel (false) button.
    pressed: Option<bool>,
    /// The area outside the dialog.
    outside: Response,
}

/// The library's own dialog, everywhere.
impl Default for FileDialog {
    fn default() -> Self {
        FileDialog::new(false, Some(Ui::own_file_dialog))
    }
}

impl std::fmt::Debug for FileDialog {
    /// Its folder, the typed name, what was chosen, and whether it is open.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDialog")
            .field("folder", &self.folder)
            .field("name", &self.name)
            .field("chosen", &self.chosen)
            .field("shown", &self.shown)
            .field("native", &self.native)
            .finish_non_exhaustive()
    }
}

impl FileDialog {
    /// A file dialog that is the system's wherever the window can ask for one; elsewhere the
    /// library's own (a Linux desktop without a portal). (On Windows a window can always ask,
    /// and the library's is not carried for it: under another renderer or in a DLL it answers
    /// at once with nothing. For the system's where possible and the library's anywhere else:
    /// `FileDialog { native: true, ..Default::default() }`.)
    pub fn native() -> Self {
        // (A page's is the browser's picker: it has no folders to show the library's in.
        // Android's picks documents, which are not files the library's could list.)
        #[cfg(any(windows, target_family = "wasm", target_os = "android"))]
        let own = None;
        #[cfg(not(any(windows, target_family = "wasm", target_os = "android")))]
        let own = Some(Ui::own_file_dialog as Own);
        FileDialog::new(true, own)
    }
    /// A file dialog not yet asked for.
    fn new(system: bool, own: Option<Own>) -> Self {
        FileDialog {
            native: system,
            own,
            folder: PathBuf::new(),
            name: String::new(),
            shown: false,
            words: FileWords::default(),
            chosen: Vec::new(),
            recent: Vec::new(),
            own_places: Vec::new(),
            show_hidden: false,
            grid: false,
            saving: false,
            folders: false,
            many: false,
            endings: Vec::new(),
            listing: Listing::default(),
            stale: false,
            table: Table::default(),
            columns_chosen: false,
            places: Vec::new(),
            drives: Vec::new(),
            places_due: false,
            back: Vec::new(),
            ahead: Vec::new(),
            seek: String::new(),
            searching: false,
            typing_path: None,
            making: None,
            asking: None,
            fresh: Fresh::Nothing,
            trouble: String::new(),
            trouble_new: false,
            visits: 0,
            focus_was: 0,
            opened_from: None,
            asked_from: 0,
            jump: None,
            onto: None,
            typed: (String::new(), Duration::ZERO),
            read: None,
            small: None,
            pictures: Vec::new(),
        }
    }
    /// Lists only files ending in one of these: `files.only(&[".png", ".jpg"])`. Saving, a name
    /// typed without one gets the first.
    pub fn only(&mut self, endings: &[&str]) {
        (self.endings, self.stale) = (endings.iter().map(|ending| ending.to_lowercase()).collect(), true);
    }
    /// Enables image thumbnails (PNG, JPEG, GIF, WebP, BMP, ICO) in a grid the user can switch
    /// to. Read on their own thread, for files in view only. (A program that never calls this
    /// does not carry the image readers for it.)
    pub fn show_pictures(&mut self) {
        self.read = Some(crate::ui::read_picture);
    }
    /// Shows the dialog to pick an existing file, starting in `folder`.
    pub fn pick(&mut self, folder: impl Into<PathBuf>) {
        self.show(folder.into(), String::new(), [false, false, false]);
    }
    /// Shows the dialog to pick one or more files (with Ctrl and Shift), starting in `folder`:
    /// all selected are in `chosen` when it answers.
    pub fn pick_many(&mut self, folder: impl Into<PathBuf>) {
        // The library's dialog picks several where the system's may not (Android's does).
        #[cfg(not(target_os = "android"))]
        self.own.get_or_insert(Ui::own_file_dialog);
        self.show(folder.into(), String::new(), [false, false, true]);
    }
    /// Shows the dialog to choose where to save and under what name. The name starts selected
    /// without its extension, to type over.
    pub fn save(&mut self, folder: impl Into<PathBuf>, name: &str) {
        self.show(folder.into(), name.into(), [true, false, false]);
    }
    /// Shows the dialog to pick a folder, starting in `folder`: the answer is the folder
    /// selected in its list, or with none, the one shown.
    pub fn pick_folder(&mut self, folder: impl Into<PathBuf>) {
        // Android's dialog picks no folders: they have no path there.
        #[cfg(target_os = "android")]
        self.own.get_or_insert(Ui::own_file_dialog);
        self.show(folder.into(), String::new(), [false, true, false]);
    }
    fn show(&mut self, folder: PathBuf, name: String, [saving, folders, many]: [bool; 3]) {
        // The absolute path, so "up" has somewhere to go.
        let folder = std::path::absolute(&folder).unwrap_or(folder);
        (self.saving, self.folders, self.many, self.shown, self.fresh) = (saving, folders, many, true, Fresh::Name);
        self.back.clear();
        self.ahead.clear();
        self.places_due = true;
        self.arrive(folder);
        self.name = name;
    }

    /// Lists the places again: the app's own, the user's folders, and the drives.
    fn find_places(&mut self) {
        self.places = self.own_places.iter().map(|(name, folder)| Place { name: name.clone(), folder: folder.clone() }).collect();
        self.places.extend(crate::places::own_folders(self.words.home));
        self.drives = crate::places::volumes();
    }

    /// Goes to another folder as asked, remembering the way back. If focus was on the list, it
    /// stays there: on the folder just left if it is in the new one, else on the first item.
    fn go(&mut self, way: Go, on_list: bool) {
        // The target; whether to push the way back or forward; and whether the forward history
        // is cleared, as when going somewhere new.
        let (to, back, fresh) = match way {
            Go::To(folder) => (Some(folder), true, true),
            Go::Back => (self.back.pop(), false, false),
            Go::Ahead => (self.ahead.pop(), true, false),
        };
        let Some(to) = to.filter(|to| *to != self.folder) else { return };
        let was = std::mem::take(&mut self.folder);
        let left = was.parent().filter(|above| *above == to).and(was.file_name()).map(|name| name.to_string_lossy().into_owned());
        let onto = on_list.then(|| left.unwrap_or_default());
        match back {
            true => self.back.push(was),
            false => self.ahead.push(was),
        }
        if fresh {
            self.ahead.clear();
        }
        self.arrive(to);
        self.onto = onto;
    }
    /// Shows a folder afresh: what was going on in the previous one is dropped.
    fn arrive(&mut self, folder: PathBuf) {
        (self.folder, self.stale, self.searching, self.typing_path, self.making) = (folder, true, false, None, None);
        self.visits = self.visits.wrapping_add(1);
        (self.jump, self.onto) = (None, None);
        self.seek.clear();
        self.trouble.clear();
        // Saving, the name stays: it saves under that name wherever.
        if !self.saving {
            self.name.clear();
        }
    }
    /// The parent folder, if any: on Windows, above a drive's root, the drives.
    fn above(&self) -> Option<PathBuf> {
        let empty = self.folder.as_os_str().is_empty();
        self.folder.parent().map(PathBuf::from).or((cfg!(windows) && !empty).then(PathBuf::new))
    }

    /// The selected items' names for the name box: one alone, several each in quotes. Saving,
    /// only a file's name goes in: a folder is no name to save under.
    fn selected_names(&self) -> Option<String> {
        let names: Vec<&str> = self.listing.selected().filter(|at| !self.saving || !self.listing.entry(*at).folder()).map(|at| self.listing.name(at)).collect();
        match names[..] {
            [] if self.saving => None,
            [] => Some(String::new()),
            [one] => Some(one.into()),
            _ => Some(names.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(" ")),
        }
    }
    /// The name box's names as absolute paths: quoted names each, or the one name; `~` is the
    /// home folder; a relative path is in the folder shown.
    fn named(&self) -> Vec<PathBuf> {
        let text = self.name.trim();
        let quoted: Vec<&str> = text.split('"').skip(1).step_by(2).filter(|name| !name.trim().is_empty()).collect();
        let names = if text.starts_with('"') && !quoted.is_empty() { quoted } else { vec![text] };
        names.into_iter().filter(|name| !name.is_empty()).map(|name| self.full(name)).collect()
    }
    /// A name or path, as an absolute path.
    fn full(&self, name: &str) -> PathBuf {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        match (name.strip_prefix('~'), home) {
            (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => home.join(rest.trim_start_matches(['/', '\\'])),
            _ => self.folder.join(name),
        }
    }

    /// Takes the name box's content: a folder is entered; a file is the answer (saving, so is a
    /// new name), unless a folder is asked for. With several names, each must be an existing
    /// file.
    fn take(&mut self) -> Option<PathBuf> {
        let named = self.named();
        let [first, ..] = &named[..] else { return None };
        if named.len() == 1 && first.is_dir() {
            self.go(Go::To(first.clone()), false);
            // Saving, a folder's name is no name to save under.
            self.name.clear();
            return None;
        }
        if self.folders {
            return None;
        }
        if self.saving {
            return self.save_as(first.clone());
        }
        match named.iter().find(|file| !file.is_file()) {
            Some(missing) => self.tell_trouble(format!("{}: {}", self.words.not_there, missing.file_name().unwrap_or_default().to_string_lossy())),
            None => return self.answer(if self.many { named } else { vec![first.clone()] }),
        }
        None
    }
    /// Takes a name to save under: with the requested extension, in an existing folder; asking
    /// first if it would overwrite a file.
    fn save_as(&mut self, file: PathBuf) -> Option<PathBuf> {
        let file = self.ended(file);
        if !file.parent().is_some_and(Path::is_dir) {
            self.tell_trouble(format!("{}: {}", self.words.not_there, file.parent().unwrap_or(&file).to_string_lossy()));
            return None;
        }
        if file.is_dir() {
            self.go(Go::To(file), false);
            return None;
        }
        if file.exists() {
            (self.asking, self.fresh) = (Some(file), Fresh::Question);
            return None;
        }
        self.answer(vec![file])
    }
    /// `file` with the first ending asked for, unless its name has one of them.
    fn ended(&self, file: PathBuf) -> PathBuf {
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        match self.endings.first() {
            Some(first) if !self.endings.iter().any(|ending| ends_in(&name, ending)) => file.with_file_name(name + first),
            _ => file,
        }
    }
    /// The system's dialog is to be asked again, for `file`: in its folder, with its name.
    fn ask_again(&mut self, file: &Path) {
        self.name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.folder = file.parent().map_or(std::mem::take(&mut self.folder), Path::to_path_buf);
    }
    /// The system's dialog answered with `answer`, or nothing (closed). Where the user went is
    /// where the dialog starts next time.
    fn system_answered(&mut self, answer: Option<PathBuf>) -> Option<PathBuf> {
        self.shown = false;
        let went = answer.as_deref().and_then(|file| if self.folders { Some(file) } else { file.parent() }).map(PathBuf::from);
        self.folder = went.unwrap_or(std::mem::take(&mut self.folder));
        // (Android's dialog may answer with several.)
        #[cfg(target_os = "android")]
        let chosen = crate::android_file_dialog::chosen;
        #[cfg(not(target_os = "android"))]
        let chosen = |file| vec![file];
        answer.and_then(|file| self.answer(chosen(file)))
    }
    /// Whether the system's dialog does what is asked: several files only on Android and iOS,
    /// and on Android no folder.
    fn system_does(&self) -> bool {
        if cfg!(target_os = "android") { !self.folders } else { !self.many || cfg!(target_os = "ios") }
    }
    /// Takes a folder, when one is asked for: the one named in the box, or with none, the one
    /// shown. (Above the drives there is none.)
    fn choose(&mut self) -> Option<PathBuf> {
        let folder = self.named().into_iter().next().unwrap_or(self.folder.clone());
        if folder.as_os_str().is_empty() || !folder.is_dir() {
            return None;
        }
        self.answer(vec![folder])
    }
    /// Answers the dialog with these: it closes, and their folder becomes the first recent one.
    fn answer(&mut self, chosen: Vec<PathBuf>) -> Option<PathBuf> {
        let first = chosen.first()?.clone();
        let folder = if self.folders { Some(first.as_path()) } else { first.parent() };
        if let Some(folder) = folder.filter(|folder| !folder.as_os_str().is_empty()) {
            self.recent.retain(|recent| recent != folder);
            self.recent.insert(0, folder.to_path_buf());
            self.recent.truncate(RECENT_KEPT);
        }
        (self.shown, self.chosen, self.asking) = (false, chosen, None);
        Some(first)
    }
    /// Shows an error in the dialog (and tells screen readers).
    fn tell_trouble(&mut self, trouble: String) {
        (self.trouble, self.trouble_new) = (trouble, true);
    }
    /// Creates the folder whose name was typed in place, and enters it.
    fn make_folder(&mut self) {
        let Some(name) = self.making.take() else { return };
        let new = self.folder.join(name.trim());
        match std::fs::create_dir(&new) {
            Ok(()) => self.go(Go::To(new), false),
            Err(error) => self.tell_trouble(error.to_string()),
        }
    }
    /// A new folder name not yet used here: "New folder", or with a number.
    fn free_name(&self) -> String {
        let name = |number: u32| if number < 2 { self.words.new_folder.to_string() } else { format!("{} {number}", self.words.new_folder) };
        (1..1000).map(name).find(|name| !self.folder.join(name).exists()).unwrap_or_default()
    }
    /// The title.
    fn title(&self) -> &'static str {
        match (self.folders, self.saving, self.many) {
            (true, ..) => self.words.pick_a_folder,
            (false, true, _) => self.words.save_as,
            (false, false, true) => self.words.open_files,
            (false, false, false) => self.words.open_a_file,
        }
    }
    /// The list's sort, from the table's headings: by what, and whether descending.
    fn sorted(&self) -> (By, bool) {
        let (by, down) = self.table.sort.first().copied().unwrap_or((0, false));
        ([By::Name, By::Size, By::Kind, By::Date][by.min(3)], down)
    }
}

/// The file and folder dialog: the library's own, or the system's.
impl Ui {
    /// The file dialog, while shown (after `files.pick(folder)`, `pick_many`, `save` or
    /// `pick_folder`). Returns the file once, when the user chooses (all of them in
    /// `files.chosen`); Escape, Cancel or a click outside close it with no answer. When a
    /// folder is asked for, Enter and double click enter folders as usual, and the button picks
    /// one.
    pub fn file_dialog(&mut self, files: &mut FileDialog) -> Option<PathBuf> {
        if files.shown && files.native && self.system_files && files.system_does() {
            return self.ask_system(files);
        }
        // With no library dialog, this one is the system's, on Windows: see
        // `FileDialog::native`. Closed, it has nothing to do.
        match files.own {
            Some(own) => own(self, files),
            None => {
                files.shown = false;
                None
            }
        }
    }

    /// The library's own file dialog (see [`file_dialog`](Self::file_dialog)), and its cleanup
    /// after closing.
    pub(crate) fn own_file_dialog(&mut self, files: &mut FileDialog) -> Option<PathBuf> {
        if !files.shown {
            self.close_file_dialog(files);
            return None;
        }
        if std::mem::take(&mut files.places_due) {
            files.find_places();
        }
        self.read_folder(files);
        let parts = self.file_dialog_parts(files);
        self.answer_file_dialog(files, parts)
    }

    /// After closing: focus goes back to what had it when the dialog opened, and the
    /// thumbnails go.
    fn close_file_dialog(&mut self, files: &mut FileDialog) {
        if let Some(back) = files.opened_from.take() {
            self.set_focus(back);
        }
        self.let_pictures_go(files);
    }

    /// Lays the dialog out, as large as the window allows: the list is what shrinks. Without
    /// room for the sidebar, a button lists the places.
    fn file_dialog_parts(&mut self, files: &mut FileDialog) -> DialogParts {
        let (t, room, words) = (self.theme, self.size(), files.words);
        let (wide, high) = ((room.w - 76.0).clamp(240.0, 720.0), (room.h - 220.0).clamp(72.0, 420.0));
        let side = wide >= 480.0;
        let outside = self.dialog_max_width(wide + 36.0);
        // In a window too short for all of it, the dialog's content scrolls.
        self.column().key("all").gap(12.0).max(f32::INFINITY, (room.h - 60.0).max(120.0)).scroll().open();
        self.text(files.title(), t.font.bold(), t.text);
        let mut went = self.file_bar(files, wide);
        self.row().size(wide, high).gap(8.0).align(Align::Start, Align::Spread).open();
        if side {
            went = went.or(self.file_places(files));
        }
        let did = match (files.grid && files.read.is_some(), files.listing.found.is_empty() && files.making.is_none()) {
            (_, true) => self.nothing_listed(files),
            (true, false) => self.file_grid(files, [wide - if side { SIDE + 8.0 } else { 0.0 }, high]),
            (false, false) => self.file_table(files, [wide - if side { SIDE + 8.0 } else { 0.0 }, high]),
        };
        self.close();
        self.file_note(files, wide);
        // Keyed, not positional: the line above them comes and goes.
        self.row().key("name").w(Length::Px(wide)).open();
        let name = self.text_box(&mut files.name, if files.folders { words.folder_hint } else { words.name_hint });
        self.close();
        let (bottom, pressed) = self.file_buttons(files, !side, wide);
        self.close();
        self.close();
        DialogParts { went: went.or(bottom), did, name, pressed, outside }
    }

    /// Acts on what the user did this run: goes where asked, closes, or answers.
    fn answer_file_dialog(&mut self, files: &mut FileDialog, parts: DialogParts) -> Option<PathBuf> {
        let DialogParts { mut went, did, name, pressed, outside } = parts;
        let replace = self.ask_to_replace(files);
        self.after_list(files, &did);
        self.fresh_keyboard(files, &name);
        files.focus_was = self.focus();
        let keyed = self.file_keys(files, did.on.is_some());
        went = went.or(keyed.0);
        let went_anywhere = went.is_some();
        if let Some(way) = went {
            files.go(way, did.on.is_some());
        }
        let closed = files.asking.is_none() && (outside.clicked || keyed.1 == Some(false) || pressed == Some(false));
        if closed {
            files.shown = false;
        }
        let answer = match (replace, pressed, keyed.1, did.taken) {
            (Some(true), ..) => files.asking.take().and_then(|file| files.answer(vec![file])),
            (_, Some(true), ..) if files.folders => files.choose(),
            (_, Some(true), ..) | (_, _, Some(true), _) => files.take(),
            (.., Some(at)) => self.take_thing(files, at),
            _ => None,
        };
        // Errors are announced to screen readers as they happen.
        if std::mem::take(&mut files.trouble_new) && !files.trouble.is_empty() {
            self.announce(&files.trouble);
        }
        // Whatever was done, the window shows it at once.
        let acted = went_anywhere || pressed.is_some() || keyed.1.is_some() || did.taken.is_some() || replace.is_some();
        if acted || answer.is_some() || !files.shown || files.stale || name.changed || did.picked {
            self.rerun();
        }
        answer
    }

    /// Reads the folder if the listing is not of it, and takes in what more was read. While
    /// reading, the dialog polls again shortly.
    fn read_folder(&mut self, files: &mut FileDialog) {
        if std::mem::take(&mut files.stale) {
            // Of a missing folder, the nearest existing ancestor.
            if let Some(there) = files.folder.ancestors().find(|folder| folder.is_dir()).filter(|there| *there != files.folder) {
                files.folder = there.to_path_buf();
            }
            self.let_pictures_go(files);
            let wanted = Wanted { folders_only: files.folders, endings: files.endings.clone() };
            files.listing = Listing::of(files.folder.clone(), wanted, files.sorted(), files.show_hidden);
            files.listing.show(&files.seek, files.show_hidden);
            files.tell_trouble(files.listing.trouble.clone());
            let onto = files.onto.take().filter(|_| !files.listing.found.is_empty());
            files.jump = onto.map(|name| files.listing.found.iter().position(|at| files.listing.name(*at) == name).unwrap_or(0));
        }
        files.listing.take_coming();
        if files.listing.reading() || files.small.as_ref().is_some_and(|small| small.out > 0) {
            self.wake_after(LOOK_AGAIN);
        }
        // Thumbnails that came back are cached.
        let scale = self.scale;
        while let Some((at, small)) = files.small.as_mut().and_then(Small::answer) {
            let Some((width, pixels)) = small.filter(|(width, pixels)| *width > 0 && !pixels.is_empty()) else { continue };
            let [wide, high] = [width as f32, (pixels.len() / width) as f32].map(|side| side / scale);
            let fit = (SMALL / wide.max(high)).min(1.0);
            files.pictures.push((at, self.add_picture(width, pixels), [wide * fit, high * fit]));
        }
    }
    /// Drops the thumbnails and the thread that reads them.
    fn let_pictures_go(&mut self, files: &mut FileDialog) {
        for (_, picture, _) in files.pictures.drain(..) {
            self.drop_picture(picture);
        }
        files.small = None;
    }

    /// The top bar: back, forward and up, then the path buttons (or the path being typed, or
    /// the search box), and the buttons to type a path and to search. Returns where the user
    /// asked to go.
    fn file_bar(&mut self, files: &mut FileDialog, wide: f32) -> Option<Go> {
        let (words, usable) = (files.words, self.enabled);
        self.row().w(Length::Px(wide)).gap(4.0).align(Align::Start, Align::Center).open();
        self.enabled = usable && !files.back.is_empty();
        let back = self.icon_button(BACK, words.back).clicked.then_some(Go::Back);
        self.enabled = usable && !files.ahead.is_empty();
        let ahead = self.icon_button(AHEAD, words.ahead).clicked.then_some(Go::Ahead);
        self.enabled = usable && files.above().is_some();
        let up = self.icon_button(UP, words.up).clicked.then(|| files.above()).flatten().map(Go::To);
        self.enabled = usable;
        let room = wide - 5.0 * (self.theme.control_height + 4.0);
        let path = match (files.typing_path.is_some(), files.searching) {
            (true, _) => self.path_box(files),
            (false, true) => self.search_box(files),
            (false, false) => self.path_buttons(files, room),
        };
        let mut typing = files.typing_path.is_some();
        if self.icon_toggle(&mut typing, TYPE, words.type_path).changed {
            (files.typing_path, files.searching, files.fresh) = (typing.then(|| files.folder.to_string_lossy().into_owned()), false, Fresh::Path);
        }
        let mut searching = files.searching;
        if self.icon_toggle(&mut searching, SEARCH, words.search).changed {
            self.look_for_names(files, searching);
        }
        self.close();
        back.or(ahead).or(up).or(path)
    }
    /// Opens the search box instead of the path, or closes it: then the whole folder shows
    /// again.
    fn look_for_names(&mut self, files: &mut FileDialog, open: bool) {
        (files.searching, files.typing_path, files.fresh) = (open, None, if open { Fresh::Search } else { Fresh::Nothing });
        if !open {
            files.seek.clear();
            files.listing.show("", files.show_hidden);
        }
    }

    /// The path buttons, each going there: as many of the last as fit in `room`, and before
    /// them a button listing the others.
    fn path_buttons(&mut self, files: &FileDialog, room: f32) -> Option<Go> {
        let (t, mut went) = (self.theme, None);
        let ancestors = files.folder.ancestors().filter(|folder| !folder.as_os_str().is_empty());
        let mut parts: Vec<(String, PathBuf)> = ancestors.map(|folder| (crumb(folder), folder.to_path_buf())).collect();
        parts.reverse();
        // On Windows the drives are above each drive's root.
        if cfg!(windows) {
            parts.insert(0, (files.words.drives.into(), PathBuf::new()));
        }
        // From the last, as many as fit; the rest go in a menu button.
        let (mut left, mut fit) = (room - 40.0, 0);
        for (name, _) in parts.iter().rev() {
            left -= self.text_width(name, t.font) + 30.0;
            if left < 0.0 && fit > 0 {
                break;
            }
            fit += 1;
        }
        let hidden = parts.len() - fit;
        let bar = self.row().grow().min(0.0, t.control_height).gap(0.0).align(Align::Start, Align::Center).clip().open();
        self.tell(&bar, Role::Group, None, files.words.path);
        if hidden > 0 {
            went = self.path_menu(&parts[..hidden]);
        }
        for (nth, (name, folder)) in parts.iter().enumerate().skip(hidden) {
            if nth > hidden {
                self.el().icon(BETWEEN, 12.0, t.text_faint).show();
            }
            if self.folder_crumb(name, *folder == files.folder).clicked {
                went = Some(Go::To(folder.clone()));
            }
        }
        self.close();
        went
    }
    /// The button before the path buttons that lists those without room.
    fn path_menu(&mut self, parts: &[(String, PathBuf)]) -> Option<Go> {
        let opens = self.folder_crumb("…", false);
        if opens.clicked {
            self.open_popup(opens.id, opens.rect);
        }
        if !self.menu(opens.id) {
            return None;
        }
        let mut picked = None;
        for (name, folder) in parts.iter().rev() {
            if self.menu_item(name).clicked {
                picked = Some(Go::To(folder.clone()));
            }
        }
        self.close();
        picked
    }
    /// One path button: a folder name that goes there. The current one in the text colour, the
    /// others muted.
    fn folder_crumb(&mut self, name: &str, here: bool) -> Response {
        let t = self.theme;
        let state = self.press(self.next_id());
        let fill = if state.hovered || state.pressed { t.hover } else { crate::color::Color::NONE };
        let font = if here { t.font.bold() } else { t.font };
        // A long name shrinks and ends in "…" when the bar runs out of room.
        let el = self.el().text(name, font, if here { t.text } else { t.text_dim }).pad((6.0, 0.0)).fill(fill).radius(4.0).sense().focusable();
        el.min(24.0, t.control_height).shrink(if here { 1 } else { 2 }).show();
        state
    }
    /// The path box, instead of the path buttons. Enter goes there (to a file: that is the
    /// answer's name); Escape brings the buttons back.
    fn path_box(&mut self, files: &mut FileDialog) -> Option<Go> {
        let mut typed = files.typing_path.take().unwrap_or_default();
        let state = self.text_box(&mut typed, files.words.type_path);
        files.typing_path = Some(typed);
        if files.fresh == Fresh::Path {
            files.fresh = Fresh::Nothing;
            self.set_focus(state.id);
            self.select_all();
        }
        if !state.focused {
            return None;
        }
        if self.key(Key::Escape, Mods::NONE) {
            files.typing_path = None;
            return None;
        }
        if !self.key(Key::Enter, Mods::NONE) {
            return None;
        }
        let path = files.full(files.typing_path.as_deref().unwrap_or_default().trim());
        if path.is_dir() {
            return Some(Go::To(path));
        }
        // A file (or a name to save under): its folder is entered, and its name goes in the
        // box.
        let folder = path.parent().filter(|folder| folder.is_dir()).map(PathBuf::from);
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
        let Some((folder, name)) = folder.zip(name).filter(|_| !files.folders) else {
            files.tell_trouble(format!("{}: {}", files.words.not_there, path.to_string_lossy()));
            return None;
        };
        (files.typing_path, files.name, files.fresh) = (None, name, Fresh::Name);
        Some(Go::To(folder))
    }
    /// The search box, instead of the path: what shows is what matches the typed text. Escape
    /// clears it, then closes it; Enter takes the one match left, or focuses the first.
    fn search_box(&mut self, files: &mut FileDialog) -> Option<Go> {
        let state = self.text_box(&mut files.seek, files.words.search);
        if files.fresh == Fresh::Search {
            files.fresh = Fresh::Nothing;
            self.set_focus(state.id);
        }
        if state.changed {
            files.listing.show(&files.seek, files.show_hidden);
        }
        if !state.focused {
            return None;
        }
        if self.key(Key::Escape, Mods::NONE) {
            let open = !files.seek.is_empty();
            self.look_for_names(files, open);
            files.seek.clear();
            files.listing.show("", files.show_hidden);
        }
        // Enter with one match: its name goes into the box and the dialog takes it as Enter
        // does. With more, focus goes to the first.
        match files.listing.found[..] {
            [only] if self.enter_waiting() => files.name = files.listing.name(only).into(),
            _ if self.key(Key::Enter, Mods::NONE) => files.jump = Some(0),
            _ => {}
        }
        None
    }

    /// The sidebar: the app's places, the user's folders, recent folders and drives, each with
    /// an icon. Returns where the user asked to go.
    fn file_places(&mut self, files: &FileDialog) -> Option<Go> {
        let (t, mut to) = (self.theme, None);
        let around = self.column().w(Length::Px(SIDE)).gap(1.0).scroll().open();
        self.tell(&around, Role::Group, None, files.words.places);
        let own = files.own_places.len();
        for (nth, place) in files.places.iter().enumerate() {
            let mark = if nth < own {
                PLACES
            } else if nth == own {
                HOME
            } else {
                FOLDER
            };
            to = to.or(self.place_row(mark, &place.name, &place.folder, &files.folder));
        }
        if !files.recent.is_empty() {
            self.el().text(files.words.recent, t.small_font, t.text_faint).pad((8.0, 6.0, 0.0, 2.0)).show();
        }
        for folder in &files.recent {
            to = to.or(self.place_row(RECENT, &crumb(folder), folder, &files.folder));
        }
        self.el().text(files.words.drives, t.small_font, t.text_faint).pad((8.0, 6.0, 0.0, 2.0)).show();
        for place in &files.drives {
            to = to.or(self.place_row(DRIVE, &place.name, &place.folder, &files.folder));
        }
        self.close();
        to.map(Go::To)
    }
    /// One place: a row with its icon and name, selected while it is the folder shown. Returns
    /// the folder when clicked.
    fn place_row(&mut self, mark: Icon, name: &str, folder: &Path, shown: &Path) -> Option<PathBuf> {
        let (t, picked) = (self.theme, folder == shown);
        let state = self.press(self.next_id());
        let fill = if picked {
            t.selected
        } else if state.hovered {
            t.hover
        } else {
            crate::color::Color::NONE
        };
        self.row().min(0.0, t.control_height).pad((8.0, 0.0)).gap(6.0).align(Align::Start, Align::Center).fill(fill).radius(4.0).clip().sense().focusable().open();
        self.tell(&state, Role::Row, Some(picked), name);
        self.el().icon(mark, 16.0, t.text_dim).show();
        self.label(name);
        self.close();
        state.clicked.then(|| folder.to_path_buf())
    }
    /// Without room for the sidebar: a button listing the places in a menu.
    fn places_menu(&mut self, files: &FileDialog) -> Option<Go> {
        let opens = self.icon_button(PLACES, files.words.places);
        if opens.clicked {
            self.open_popup(opens.id, opens.rect);
        }
        if !self.menu(opens.id) {
            return None;
        }
        // In sidebar order: places, recent folders, drives.
        let recent = files.recent.iter().map(|folder| Place { name: crumb(folder), folder: folder.clone() });
        let all: Vec<Place> = files.places.iter().cloned().chain(recent).chain(files.drives.iter().cloned()).collect();
        let mut picked = None;
        for place in all {
            if self.menu_item(&place.name).clicked {
                picked = Some(Go::To(place.folder));
            }
        }
        self.close();
        picked
    }

    /// Whether Enter was pressed and nothing took it yet.
    fn enter_waiting(&self) -> bool {
        self.input.strokes.iter().any(|stroke| matches!(stroke, Stroke::Key(Key::Enter, mods) if *mods == Mods::NONE))
    }
    /// The folder as a table: name, size, type and date columns, sorted by clicking a heading.
    /// Only rows in view are described.
    fn file_table(&mut self, files: &mut FileDialog, [wide, high]: [f32; 2]) -> Did {
        let words = files.words;
        // Where names would have too little room beside the other columns, they keep enough,
        // and the table scrolls sideways with names in view.
        let columns = [Column::new(words.name), Column::new(words.size).px(80.0).align(Align::End), Column::new(words.kind).px(64.0), Column::new(words.date).px(128.0)];
        // Short of room for all columns, dates then sizes are dropped until the user chooses
        // the columns. Types stay: they show which items are folders.
        if !files.columns_chosen {
            files.table.hidden = [(3, wide < WITH_DATES), (1, wide < WITH_SIZES)].into_iter().filter(|(_, out)| *out).map(|(column, _)| column).collect();
        }
        let hidden = files.table.hidden.len();
        let (t, mut did) = (self.theme, Did::default());
        files.table.frozen = 1;
        self.el().key(files.visits).grow().border(t.line, 1.0).radius(6.0).clip().open();
        let sorted = self.table_of_rows(&mut files.table, &columns).changed;
        files.columns_chosen |= files.table.hidden.len() != hidden;
        if sorted {
            let (by, down) = files.sorted();
            files.listing.sort(by, down);
        }
        let row = ROW.max(self.text.metrics(t.font, self.scale).height().ceil() + 6.0);
        self.start_rows(files.jump, row, files.listing.found.len(), high - t.control_height - 1.0);
        if files.making.is_some() {
            self.table_row().open();
            self.new_folder_box(files);
            self.close();
        }
        let enter = self.enter_waiting();
        let mut written = std::mem::take(&mut self.scratch);
        for row in self.rows(files.listing.found.len(), row) {
            let at = files.listing.found[row];
            let state = self.file_row(files, at, &mut written);
            self.row_did(files, row, at, &state, enter, &mut did);
        }
        self.scratch = written;
        self.close();
        self.close();
        self.close();
        did
    }
    /// Before the list's (or grid's) rows, `row` high and `count` in all, in a box showing
    /// `high` of them: if focus is to go to a row out of view (`jump`), the box scrolls there
    /// first.
    fn start_rows(&mut self, jump: Option<usize>, row: f32, count: usize, high: f32) {
        let Some(jump) = jump.filter(|jump| *jump < count) else { return };
        let Some(open) = self.open.last() else { return };
        let (id, offset) = (self.tree.specs[open.at as usize].id, self.tree.specs[open.at as usize].offset);
        let (top, high) = (jump as f32 * row, high.max(row));
        if top < offset[1] || top + row > offset[1] + high {
            self.scrolls.insert(id, [offset[0], (top - (high - row) / 2.0).max(0.0)]);
            self.again = true;
        }
    }
    /// One table row: an item, its size, type and modification time. (`written` is a scratch
    /// buffer.)
    fn file_row(&mut self, files: &FileDialog, at: u32, written: &mut String) -> Response {
        let (entry, name) = (files.listing.entry(at), files.listing.name(at));
        let state = self.selectable_row(at, entry.selected());
        self.row().gap(6.0).align(Align::Start, Align::Center).open();
        self.icon(match (entry.folder(), files.listing.is_picture(at)) {
            (true, _) => FOLDER,
            (false, true) => PICTURE,
            (false, false) => PAGE,
        });
        self.label(name);
        self.close();
        written.clear();
        if !entry.folder() {
            size(written, entry.size);
        }
        self.note(written);
        written.clear();
        match entry.folder() {
            true => written.push_str(files.words.folder),
            false => written.extend(ending(name).chars().map(|letter| letter.to_ascii_uppercase())),
        }
        self.note(written);
        written.clear();
        if entry.time != 0 {
            let [year, month, day, hour, minute] = crate::places::local_time(entry.time);
            let _ = write!(written, "{year}-{month:02}-{day:02} {hour:02}:{minute:02}");
        }
        self.note(written);
        self.close();
        state
    }
    /// What a row (or grid cell) did: selected by click (Ctrl and Shift add), reached by keys
    /// (Shift+arrow selects a range), or double-clicked or Enter pressed (taken).
    fn row_did(&mut self, files: &mut FileDialog, row: usize, at: u32, state: &Response, enter: bool, did: &mut Did) {
        // Focus put on a row by the dialog is not the user moving there.
        if files.jump == Some(row) {
            self.set_focus(state.id);
            files.jump = None;
        }
        let arrived = state.focused && self.focus_visible() && files.focus_was != state.id;
        if state.focused {
            did.on = Some(row);
        }
        // Enter takes the selection and selects nothing itself; Space selects the row.
        let by_key = state.clicked && !self.input.released[0];
        if state.clicked && !(by_key && enter) {
            let mods = if by_key { Mods::NONE } else { self.mods() };
            files.listing.click(row, mods.ctrl && files.many, mods.shift && files.many);
            did.picked = true;
        }
        if state.clicked {
            did.taken = did.taken.or((state.double_clicked || by_key && enter).then_some(at));
        } else if arrived && !files.listing.entry(at).selected() {
            files.listing.pick_only(Some(at));
            did.picked = true;
        }
    }
    /// The row of a folder being created, first in the list: a box with its name, created by
    /// Enter, dropped by Escape.
    fn new_folder_box(&mut self, files: &mut FileDialog) {
        let mut name = files.making.take().unwrap_or_default();
        self.row().gap(6.0).align(Align::Start, Align::Center).open();
        self.icon(FOLDER);
        let state = self.text_box(&mut name, files.words.new_folder);
        self.close();
        files.making = Some(name);
        if files.fresh == Fresh::NewFolder {
            (files.fresh, files.jump) = (Fresh::Nothing, None);
            self.set_focus(state.id);
            self.select_all();
        }
        if state.focused && self.key(Key::Escape, Mods::NONE) {
            files.making = None;
        }
        if state.focused && self.key(Key::Enter, Mods::NONE) {
            files.make_folder();
        }
    }

    /// The folder as a thumbnail grid: images as their thumbnails (as they load), everything
    /// else as its icon. Only grid rows in view are described.
    fn file_grid(&mut self, files: &mut FileDialog, [wide, high]: [f32; 2]) -> Did {
        let (t, mut did) = (self.theme, Did::default());
        let across = ((wide - 20.0) / CELL).floor().max(1.0) as usize;
        // A cell is as high as its thumbnail plus a line for the name.
        let high_cell = SMALL + self.text.metrics(t.small_font, self.scale).height().ceil() + 16.0;
        let count = files.listing.found.len();
        self.column().key(files.visits).grow().border(t.line, 1.0).radius(6.0).clip().scroll().open();
        self.start_rows(files.jump.map(|row| row / across), high_cell, count.div_ceil(across), high);
        if files.making.is_some() {
            self.row().pad((8.0, 4.0)).open();
            self.new_folder_box(files);
            self.close();
        }
        let enter = self.enter_waiting();
        for line in self.rows(count.div_ceil(across), high_cell) {
            self.row().open();
            for row in line * across..(line * across + across).min(count) {
                let at = files.listing.found[row];
                let state = self.file_cell(files, at, [row, count], &mut did);
                self.row_did(files, row, at, &state, enter, &mut did);
            }
            self.close();
        }
        self.close();
        self.close();
        did
    }
    /// One grid cell: an item's thumbnail or icon, its name below. Reported as a selectable
    /// list row.
    fn file_cell(&mut self, files: &FileDialog, at: u32, [nth, count]: [usize; 2], did: &mut Did) -> Response {
        let (t, entry, name) = (self.theme, files.listing.entry(at), files.listing.name(at));
        let state = self.press(self.id(("cell", at)));
        let fill = if entry.selected() {
            t.selected
        } else if state.hovered {
            t.hover
        } else {
            crate::color::Color::NONE
        };
        self.column().key(("cell", at)).w(Length::Px(CELL)).h(Length::Grow(1.0)).pad(4.0).gap(4.0).align(Align::Start, Align::Center).fill(fill).radius(6.0).sense().focusable().open();
        self.tell(&state, Role::Row, Some(entry.selected()), name);
        self.tell_position(nth + 1, count);
        self.el().size(SMALL, SMALL).align(Align::Center, Align::Center).open();
        let mark = if entry.folder() {
            FOLDER
        } else if files.listing.is_picture(at) {
            PICTURE
        } else {
            PAGE
        };
        match files.pictures.iter().find(|(of, ..)| *of == at) {
            Some((_, picture, [wide, high])) => self.el().picture(*picture).size(*wide, *high).show(),
            None => self.el().icon(mark, 40.0, t.text_dim).show(),
        };
        self.close();
        self.el().w(Length::Grow(1.0)).text(name, t.small_font, t.text).text_align(Align::Center).clip().show();
        self.close();
        if files.listing.is_picture(at) {
            did.pictures.push(at);
        }
        state
    }
    /// Instead of an empty list: why it is empty.
    fn nothing_listed(&mut self, files: &FileDialog) -> Did {
        let t = self.theme;
        let why = match (files.listing.reading(), files.seek.is_empty()) {
            (true, _) => files.words.reading,
            (false, false) => files.words.none_found,
            (false, true) => "",
        };
        self.column().grow().border(t.line, 1.0).radius(6.0).pad(12.0).align(Align::Center, Align::Center).open();
        self.el().text(why, t.small_font, t.text_dim).show();
        self.close();
        Did::default()
    }

    /// Below the list: the error, or that the folder is still loading, or nothing.
    fn file_note(&mut self, files: &FileDialog, wide: f32) {
        let t = self.theme;
        let (text, color) = match (files.trouble.is_empty(), files.listing.reading()) {
            (false, _) => (files.trouble.as_str(), t.red),
            (true, true) => (files.words.reading, t.text_dim),
            (true, false) => return,
        };
        self.el().w(Length::Px(wide)).clip().text(text, t.small_font, color).show();
    }
    /// The bottom button row: new folder, places (without room for the sidebar), view options;
    /// Cancel, and the answering button. Returns where the user asked to go, and which of the
    /// last two was pressed (Cancel: `false`).
    fn file_buttons(&mut self, files: &mut FileDialog, places: bool, wide: f32) -> (Option<Go>, Option<bool>) {
        let (t, words) = (self.theme, files.words);
        let button = match (files.folders, files.saving) {
            (true, _) => words.choose,
            (false, true) => words.save,
            (false, false) => words.open,
        };
        // Short of room in one row, the buttons wrap onto more, aligned to the end: with large
        // text, say.
        let wanted = 3.0 * (t.control_height + 8.0) + self.text_width(words.cancel, t.font) + self.text_width(button, t.font.bold()) + 60.0;
        let row = if wanted > wide { self.wrapping_row().w(Length::Px(wide)).align(Align::End, Align::Center) } else { self.row().align(Align::Start, Align::Center) };
        row.key("buttons").gap(8.0).open();
        if self.icon_button(NEW_FOLDER, words.new_folder).clicked {
            self.new_folder(files);
        }
        let went = if places { self.places_menu(files) } else { None };
        self.view_menu(files);
        if wanted <= wide {
            self.spacer();
        }
        let cancel = self.button(words.cancel).clicked;
        let pressed = self.primary_button(button).clicked;
        self.close();
        (went, if cancel { Some(false) } else { pressed.then_some(true) })
    }
    /// Starts creating a folder in place: a row first in the list, with a name to type over.
    fn new_folder(&mut self, files: &mut FileDialog) {
        (files.making, files.fresh, files.jump) = (Some(files.free_name()), Fresh::NewFolder, Some(0));
        self.rerun();
    }
    /// The view menu: hidden files, thumbnail grid or table (if the app enabled thumbnails),
    /// and the sort (choosing the current one reverses it).
    fn view_menu(&mut self, files: &mut FileDialog) {
        let opens = self.icon_button(VIEW, files.words.view);
        if opens.clicked {
            self.open_popup(opens.id, opens.rect);
        }
        if !self.menu(opens.id) {
            return;
        }
        let words = files.words;
        if self.menu_checkbox(&mut files.show_hidden, words.hidden).changed {
            files.listing.show(&files.seek, files.show_hidden);
        }
        if files.read.is_some() {
            self.menu_checkbox(&mut files.grid, words.grid);
        }
        self.separator();
        let (by, down) = files.table.sort.first().copied().unwrap_or((0, false));
        for (column, name) in [words.name, words.size, words.kind, words.date].into_iter().enumerate() {
            let mut on = by == column;
            if self.menu_checkbox(&mut on, name).changed {
                files.table.sort = vec![(column, by == column && !down)];
                let (by, down) = files.sorted();
                files.listing.sort(by, down);
            }
        }
        self.close();
    }

    /// Asks whether a file may be overwritten, while that is the question: a box over the
    /// dialog with Cancel (focused first) and Replace. Returns which was pressed: Replace,
    /// `true`.
    fn ask_to_replace(&mut self, files: &mut FileDialog) -> Option<bool> {
        let file = files.asking.clone()?;
        let (t, words) = (self.theme, files.words);
        let beside = self.dialog_max_width(380.0);
        self.el().text(&file.file_name().unwrap_or_default().to_string_lossy(), t.font.bold(), t.text).show();
        self.paragraph(words.there_already);
        self.row().gap(8.0).align(Align::End, Align::Center).open();
        let cancel = self.button(words.cancel);
        let replace = self.primary_button(words.replace).clicked;
        self.close();
        self.close();
        if files.fresh == Fresh::Question {
            (files.fresh, files.asked_from) = (Fresh::Nothing, self.focus());
            self.set_focus(cancel.id);
        }
        // Answered no, it returns focus to what had it when it asked.
        let no = cancel.clicked || beside.clicked || self.key(Key::Escape, Mods::NONE);
        if no {
            files.asking = None;
            self.set_focus(files.asked_from);
        }
        if no { Some(false) } else { replace.then_some(true) }
    }

    /// After the list is described: the selection goes into the name box, then letters typed on
    /// the list, range and page keys, and the thumbnails wanted.
    fn after_list(&mut self, files: &mut FileDialog, did: &Did) {
        if let Some(names) = files.selected_names().filter(|_| did.picked) {
            files.name = names;
        }
        if let Some(row) = did.on {
            self.keys_on_list(files, row);
        }
        // Thumbnails of the files in view, requested one or two at a time.
        let Some(read) = files.read.filter(|_| files.grid && !did.pictures.is_empty()) else { return };
        let most = (SMALL * self.scale).ceil() as usize;
        let small = files.small.get_or_insert_with(Small::default);
        for at in did.pictures.iter().copied() {
            if small.out < 2 && files.listing.ask_picture(at) {
                small.ask(at, files.folder.join(files.listing.name(at)), read, most);
            }
        }
        // Past the limit, thumbnails out of view are dropped.
        if files.pictures.len() > KEPT_PICTURES {
            let (keep, gone): (Vec<_>, Vec<_>) = files.pictures.drain(..).partition(|(at, ..)| did.pictures.contains(at));
            files.pictures = keep;
            for (at, picture, _) in gone {
                files.listing.forget_picture(at);
                self.drop_picture(picture);
            }
        }
    }
    /// Keys while focus is on the list: letters jump to a matching name; Shift+arrow selects a
    /// range; Ctrl+Space toggles one; Home, End, Page Up and Page Down move that far; Ctrl+A
    /// selects all.
    fn keys_on_list(&mut self, files: &mut FileDialog, row: usize) {
        let count = files.listing.found.len();
        let page = 8;
        let typed = self.typed();
        if !typed.is_empty() {
            let (so_far, when) = &mut files.typed;
            if self.now.saturating_sub(*when) > TYPED_TOGETHER {
                so_far.clear();
            }
            so_far.push_str(&typed);
            *when = self.now;
            // Letters typed together go to the name starting with all of them, staying put
            // while the current one does. One letter, or the same letter repeated where no name
            // starts with them all, cycles through names starting with it.
            let first = so_far.chars().next().map(String::from).unwrap_or_default();
            let again = so_far.chars().all(|letter| first.starts_with(letter));
            let whole = files.listing.starting(so_far, row).filter(|_| so_far.chars().nth(1).is_some());
            let found = whole.or_else(|| again.then(|| files.listing.starting(&first, row + 1)).flatten());
            self.go_to_row(files, found);
        }
        let wanted = [(Key::Up, Mods::SHIFT), (Key::Down, Mods::SHIFT), (Key::Home, Mods::NONE), (Key::End, Mods::NONE), (Key::PageUp, Mods::NONE), (Key::PageDown, Mods::NONE)];
        let which = wanted.iter().position(|(key, mods)| self.key(*key, *mods));
        let to = which.map(|which| match which {
            0 => row.saturating_sub(1),
            1 => row + 1,
            2 => 0,
            3 => count.saturating_sub(1),
            4 => row.saturating_sub(page),
            _ => row + page,
        });
        // Shift+arrow selects a range, where several can be selected.
        match to.map(|to| to.min(count.saturating_sub(1))) {
            Some(to) if files.many && which < Some(2) => self.stretch_to_row(files, to),
            to => self.go_to_row(files, to),
        }
        let [one, all] = [(Key::Space, Mods::CTRL), (Key::Char('a'), Mods::CTRL)].map(|(key, mods)| files.many && self.key(key, mods));
        if one {
            files.listing.click(row, true, false);
        }
        if all {
            files.listing.pick_all();
        }
        if one || all {
            files.name = files.selected_names().unwrap_or_default();
            self.rerun();
        }
    }
    /// Focuses a list row and selects its item.
    fn go_to_row(&mut self, files: &mut FileDialog, row: Option<usize>) {
        let Some(row) = row else { return };
        let Some(&at) = files.listing.found.get(row) else { return };
        files.listing.pick_only(Some(at));
        if let Some(names) = files.selected_names() {
            files.name = names;
        }
        files.jump = Some(row);
        self.rerun();
    }
    /// Focuses a list row and selects everything from the anchor to it.
    fn stretch_to_row(&mut self, files: &mut FileDialog, row: usize) {
        if row >= files.listing.found.len() {
            return;
        }
        files.listing.click(row, false, true);
        files.name = files.selected_names().unwrap_or_default();
        files.jump = Some(row);
        self.rerun();
    }

    /// Gives focus to what just appeared and wants it.
    fn fresh_keyboard(&mut self, files: &mut FileDialog, name: &Response) {
        if files.fresh != Fresh::Name {
            return;
        }
        files.fresh = Fresh::Nothing;
        files.opened_from = files.opened_from.or(Some(self.focus()));
        self.set_focus(name.id);
        // Saving, the name is selected without its extension, to type over.
        let stem = files.name.rfind('.').filter(|at| *at > 0).unwrap_or(files.name.len());
        self.reader_selects = Some((name.id, [0, stem], false));
    }
    /// The dialog's own keys. Returns where the user asked to go, and whether to answer (Enter:
    /// `true`) or close (Escape: `false`).
    fn file_keys(&mut self, files: &mut FileDialog, on_list: bool) -> (Option<Go>, Option<bool>) {
        if files.asking.is_some() {
            return (None, None);
        }
        files.stale |= self.key(Key::F(5), Mods::NONE);
        let went = match () {
            _ if self.key(Key::Left, ALT) => Some(Go::Back),
            _ if self.key(Key::Right, ALT) => Some(Go::Ahead),
            _ if self.key(Key::Up, ALT) || on_list && self.key(Key::Backspace, Mods::NONE) => files.above().map(Go::To),
            _ => None,
        };
        let [path, search, hidden, new] = [('l', Mods::CTRL), ('f', Mods::CTRL), ('h', Mods::CTRL), ('n', CTRL_SHIFT)].map(|(letter, mods)| self.key(Key::Char(letter), mods));
        if path {
            (files.typing_path, files.searching, files.fresh) = (Some(files.folder.to_string_lossy().into_owned()), false, Fresh::Path);
        }
        if search {
            self.look_for_names(files, true);
        }
        if hidden {
            files.show_hidden = !files.show_hidden;
            files.listing.show(&files.seek, files.show_hidden);
        }
        if new {
            self.new_folder(files);
        }
        if path || search || hidden {
            self.rerun();
        }
        let done = match () {
            _ if self.key(Key::Escape, Mods::NONE) => Some(false),
            _ if self.key(Key::Enter, Mods::NONE) => Some(true),
            _ => None,
        };
        (went, done)
    }
    /// An item was taken (double-clicked, or Enter): a folder is entered, a file is the answer
    /// (when a file is asked for), with the other selected ones.
    fn take_thing(&mut self, files: &mut FileDialog, at: u32) -> Option<PathBuf> {
        let path = files.folder.join(files.listing.name(at));
        if files.listing.entry(at).folder() {
            files.go(Go::To(path), true);
            return None;
        }
        if !files.many || !files.listing.entry(at).selected() {
            files.name = files.listing.name(at).into();
        }
        files.take()
    }

    /// The system's dialog instead of the library's: requested once through the window's host,
    /// answered one or more runs later. (Nothing is drawn here: the system's dialog is over the
    /// window meanwhile.)
    fn ask_system(&mut self, files: &mut FileDialog) -> Option<PathBuf> {
        if let Some(answer) = self.file_answer.take() {
            self.file_asked = false;
            // A name saved without an ending asked for gets the first (the Linux portal adds
            // none). If that would replace a file, the system's dialog is asked again with it, to
            // ask before replacing.
            let ended = answer.clone().map(|file| if files.saving { files.ended(file) } else { file });
            match ended.as_ref().filter(|ended| answer.as_ref() != Some(*ended) && ended.exists()) {
                Some(ended) => files.ask_again(ended),
                // iOS's picker answers with every file chosen: the first comes back, all are
                // in `chosen`.
                #[cfg(target_os = "ios")]
                None if files.many => return files.system_answered(ended).inspect(|_| files.chosen = crate::ios_files::all_picked()),
                None => return files.system_answered(ended),
            }
        }
        if !std::mem::replace(&mut self.file_asked, true) {
            crate::platform::hand_file_dialog();
            let (folder, name, endings) = (files.folder.clone(), files.name.clone(), files.endings.clone());
            self.file_ask = Some(FileDialogRequest { saving: files.saving, folders: files.folders, folder, name, endings, title: files.title(), many: files.many });
        }
        None
    }

    /// For the window's host: it can show the system's file dialog when an app asks for that
    /// ([`FileDialog::native`]).
    pub fn set_native_file_dialog(&mut self, can: bool) {
        self.system_files = can;
    }
    /// For the window's host: the request for the system's dialog, when the app just asked.
    /// (Returned once.)
    pub fn take_file_dialog_request(&mut self) -> Option<FileDialogRequest> {
        self.file_ask.take()
    }
    /// For the window's host: the system's dialog was answered, with a file or none (closed).
    /// Run the UI then: the app gets the answer in that run.
    pub fn file_dialog_answered(&mut self, file: Option<PathBuf>) {
        self.file_answer = Some(file);
    }
    /// After a run: an answer no file dialog took is dropped (the app stopped asking while the
    /// system's dialog was up), and the next request starts afresh.
    pub(crate) fn forget_answer(&mut self) {
        if self.file_answer.take().is_some() {
            self.file_asked = false;
        }
    }
}

/// A folder's label in the path bar: its last component, or for a drive or the root, all of it.
fn crumb(folder: &Path) -> String {
    let name = folder.file_name().map_or(folder.as_os_str(), |name| name);
    let name = name.to_string_lossy();
    // "C:\" is the drive "C:".
    if cfg!(windows) && name.len() > 1 { name.trim_end_matches('\\').to_string() } else { name.into_owned() }
}

/// Writes a file size for people: bytes, KB, MB, GB.
fn size(written: &mut String, bytes: u64) {
    const KB: u64 = 1024;
    let _ = match bytes {
        bytes if bytes < KB => write!(written, "{bytes} B"),
        bytes if bytes < KB * KB => write!(written, "{} KB", bytes.div_ceil(KB)),
        bytes if bytes < KB * KB * KB => write!(written, "{:.1} MB", bytes as f64 / (KB * KB) as f64),
        bytes => write!(written, "{:.1} GB", bytes as f64 / (KB * KB * KB) as f64),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library's dialog is set only where it may show: by a library dialog
    /// (`FileDialog::default`), by one that is the system's where possible and the library's
    /// elsewhere (`system: true` beside it), by one picking several files, and by a system
    /// dialog where a window may have none (a Linux desktop without a portal). On Windows a
    /// system dialog has none: nothing reaches the library's from it, so a program whose
    /// dialogs are all such does not carry it. The system is asked, and the answer comes back
    /// as usual; without a system dialog at hand, it answers at once with nothing.
    #[test]
    fn the_librarys_own_dialog_is_handed_over_only_where_it_may_show() {
        let mut system = FileDialog::native();
        assert_eq!(system.own.is_none(), cfg!(windows));
        assert!(FileDialog::default().own.is_some() && FileDialog { native: true, ..Default::default() }.own.is_some());
        let mut many = FileDialog::native();
        many.pick_many(".");
        assert!(many.own.is_some());
        // Asked, the system's dialog is requested once; answered, its answer comes back.
        let mut windowed = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        windowed.set_native_file_dialog(true);
        system.pick_folder(".");
        let mut got = Vec::new();
        windowed.run(&mut |ui| got.extend(ui.file_dialog(&mut system)));
        assert!(windowed.take_file_dialog_request().is_some_and(|ask| ask.folders) && windowed.take_file_dialog_request().is_none());
        let folder = std::path::absolute(".").unwrap();
        windowed.file_dialog_answered(Some(folder.clone()));
        windowed.run(&mut |ui| got.extend(ui.file_dialog(&mut system)));
        windowed.run(&mut |ui| got.extend(ui.file_dialog(&mut system)));
        assert_eq!((got, system.shown, system.chosen.clone()), (vec![folder.clone()], false, vec![folder]));
        // Nothing of the library's dialog was set up for it: no sidebar places.
        assert!(system.places.is_empty() && system.drives.is_empty());
        // On Windows, with no system dialog at hand, it closes with nothing.
        if cfg!(windows) {
            let mut alone = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
            system.pick(".");
            let mut answers = 0;
            alone.run(&mut |ui| answers += ui.file_dialog(&mut system).is_some() as u32);
            assert_eq!((answers, system.shown, alone.take_file_dialog_request().is_none()), (0, false, true));
        }
    }

    /// A name the system's dialog saves under without the ending asked for gets it; if a file
    /// has that name, the system's dialog is asked again, with it, so that it asks before
    /// replacing.
    #[test]
    fn a_name_the_systems_dialog_saves_under_gets_the_ending_asked_for() {
        let folder = std::env::temp_dir().join(format!("taut-ended-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("there.md"), "").unwrap();
        let mut windowed = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        windowed.set_native_file_dialog(true);
        let (mut files, mut got) = (FileDialog::native(), Vec::new());
        files.only(&[".md", ".txt"]);
        let mut answered = |file: &Path, got: &mut Vec<PathBuf>, files: &mut FileDialog| {
            windowed.file_dialog_answered(Some(file.to_path_buf()));
            windowed.run(&mut |ui| got.extend(ui.file_dialog(files)));
            windowed.take_file_dialog_request()
        };
        for (typed, saved) in [("notes", "notes.md"), ("notes.TXT", "notes.TXT"), ("notes.txt.bak", "notes.txt.bak.md")] {
            files.save(&folder, "");
            answered(&folder.join(typed), &mut got, &mut files);
            assert_eq!(got.pop(), Some(folder.join(saved)), "{typed}");
        }
        // "there" would replace "there.md": asked again, nothing answered yet.
        files.save(&folder, "");
        let again = answered(&folder.join("there"), &mut got, &mut files).expect("asked again");
        assert_eq!((again.name.as_str(), again.folder.as_path(), got.len(), files.shown), ("there.md", folder.as_path(), 0, true));
        // Confirmed there, it is the answer.
        answered(&folder.join("there.md"), &mut got, &mut files);
        assert_eq!((got, files.shown), (vec![folder.join("there.md")], false));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn sizes_are_written_the_way_people_read_them() {
        let written = |bytes: u64| {
            let mut text = String::new();
            size(&mut text, bytes);
            text
        };
        assert_eq!([0, 1023, 1024, 1025, 5 << 20, 3 << 30].map(written), ["0 B", "1023 B", "1 KB", "2 KB", "5.0 MB", "3.0 GB"]);
        assert_eq!(crumb(Path::new("/")), "/");
        #[cfg(windows)]
        assert_eq!((crumb(Path::new("C:\\")), crumb(Path::new("C:\\Users"))), ("C:".to_string(), "Users".to_string()));
    }

    #[test]
    fn names_in_the_box_are_whole_paths_and_get_the_ending_asked_for() {
        let folder = std::env::temp_dir().join(format!("taut-named-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut files = FileDialog { folder: folder.clone(), ..Default::default() };
        files.name = "\"one.txt\" \"two words.txt\"".into();
        assert_eq!(files.named(), [folder.join("one.txt"), folder.join("two words.txt")]);
        files.name = "  notes.txt ".into();
        assert_eq!(files.named(), [folder.join("notes.txt")]);
        // Saving: the first requested extension, unless the name already has one of them.
        files.save(&folder, "report");
        files.only(&[".md", ".txt"]);
        assert_eq!(files.save_as(folder.join("report")), Some(folder.join("report.md")));
        files.save(&folder, "");
        assert_eq!(files.save_as(folder.join("Report.TXT")), Some(folder.join("Report.TXT")));
        assert_eq!(files.recent, std::slice::from_ref(&folder), "the folder is the first of the recent ones, once");
        std::fs::remove_dir_all(folder).unwrap();
    }
}
