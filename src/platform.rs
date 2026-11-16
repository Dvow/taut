//! What differs per system: opening a window, input, drawing text, the clipboard. Everything
//! else is the same code everywhere.
use std::path::PathBuf;

/// The window to open.
#[must_use]
pub struct Window<'a> {
    /// Its title bar text.
    pub title: &'a str,
    /// Width of the client area, in units.
    pub width: f32,
    /// Height of the client area, in units.
    pub height: f32,
    /// Least width the person can resize it to.
    pub min_width: f32,
    /// Least height.
    pub min_height: f32,
    /// Where the top left of its frame goes, in screen pixels: on the screen at that place, or
    /// if none is there any more, the nearest one. The whole window is put on its screen as far
    /// as it fits. Left to the system when `None`.
    pub at: Option<(i32, i32)>,
    /// The screen it opens centred on while `at` is `None`: its index in
    /// [`Ui::screens`](crate::Ui::screens), the main screen being 0.
    pub screen: Option<usize>,
    /// It opens maximized, with the size above to restore to.
    pub maximized: bool,
    /// It opens minimized and without focus: what a program started with the computer does.
    pub minimized: bool,
    /// The program's icon in the window frame, task bar and window list: square pictures in as
    /// many sizes as given, each its width in pixels and its pixels in rows from the top,
    /// `0xAARRGGBB` not premultiplied (as [`Ui::add_picture`](crate::Ui::add_picture) takes).
    /// The system shows the one that fits. With none, the system's default: on Windows the
    /// first icon in the program's file.
    pub icons: &'a [(usize, &'a [u32])],
    /// No system frame: no title bar or border. The app draws its own and says which box moves
    /// the window ([`Ui::window_strip`](crate::Ui::window_strip)); the person resizes it by its
    /// edges and corners, and the app's own buttons minimize, maximize and close it
    /// ([`Ui::minimize`](crate::Ui::minimize), [`Ui::set_maximized`](crate::Ui::set_maximized),
    /// [`Ui::quit`](crate::Ui::quit)). Its size and place above then include all of it.
    pub bare: bool,
    /// See-through wherever its interface is: where nothing is drawn (it opens on a page of
    /// [`Color::NONE`](crate::Color::NONE)), and through what is drawn as far as that is
    /// transparent. For a desktop widget, or a shaped splash picture. It has no system frame
    /// either, and keeps its opening size unless also `bare`, which makes it resizable by its
    /// edges.
    pub clear: bool,
}

impl std::fmt::Debug for Window<'_> {
    /// All its fields, but only the number of icon sizes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut said = f.debug_struct("Window");
        said.field("title", &self.title).field("width", &self.width).field("height", &self.height).field("min_width", &self.min_width).field("min_height", &self.min_height);
        said.field("at", &self.at).field("screen", &self.screen).field("maximized", &self.maximized).field("minimized", &self.minimized);
        said.field("icons", &self.icons.len()).field("bare", &self.bare).field("clear", &self.clear).finish()
    }
}

impl Default for Window<'_> {
    fn default() -> Self {
        Window { title: "", width: 900.0, height: 600.0, min_width: 200.0, min_height: 120.0, at: None, screen: None, maximized: false, minimized: false, icons: &[], bare: false, clear: false }
    }
}

impl Window<'_> {
    /// The same window, opened where [`Ui::placed`](crate::Ui::placed) said it was (if
    /// anything): what an app keeps on closing and opens with next time.
    pub fn placed(self, place: Option<Placed>) -> Self {
        let Some(Placed { at, width, height, maximized }) = place else { return self };
        Window { at: Some(at), width, height, maximized, ..self }
    }
}

/// Where a window is on the screen, as the person left it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placed {
    /// The top left of its frame in screen pixels, and its client size in units, as when not
    /// maximized.
    pub at: (i32, i32),
    /// Client width.
    pub width: f32,
    /// Client height.
    pub height: f32,
    /// It is maximized.
    pub maximized: bool,
}

/// The clipboard, as far as an interface needs it: text, a picture or files, both ways. What is
/// put on it replaces what was there, of every kind. (A text-only clipboard implements just
/// that: it then has no pictures and no files.)
pub trait Clipboard {
    /// The text on the clipboard, if any.
    fn get(&mut self) -> Option<String>;
    /// Puts text on the clipboard, replacing whatever was there.
    fn set(&mut self, text: &str);
    /// The picture on the clipboard, if any: its width in pixels and its pixels in rows from
    /// the top, `0xAARRGGBB` not premultiplied, as [`Ui::add_picture`](crate::Ui::add_picture)
    /// takes.
    fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
        None
    }
    /// Puts a picture on the clipboard: `width` pixels a row, rows from the top, each as
    /// `picture` gives them.
    fn set_picture(&mut self, _width: usize, _pixels: &[u32]) {}
    /// The files on the clipboard: what was copied in a file manager.
    fn files(&mut self) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Puts files on the clipboard, for a file manager to paste.
    fn set_files(&mut self, _files: &[PathBuf]) {}
}

/// How pictures and files go on and off the system clipboard `C`: the clipboard's own code for
/// that (including a PNG reader and writer). Set by the first call that copies or asks for one
/// ([`Ui::copy_picture`](crate::Ui::copy_picture),
/// [`Ui::clipboard_picture`](crate::Ui::clipboard_picture),
/// [`Ui::copy_files`](crate::Ui::copy_files),
/// [`Ui::clipboard_files`](crate::Ui::clipboard_files), through `hand_pictures_and_files`), and
/// reached no other way: a program that never does is built without it, and its clipboard holds
/// text alone.
pub(crate) struct PicturesAndFiles<C> {
    pub picture: TakesPicture<C>,
    pub set_picture: fn(&mut C, usize, &[u32]),
    pub files: fn(&mut C) -> Vec<PathBuf>,
    pub set_files: fn(&mut C, &[PathBuf]),
}

/// Takes a picture off the clipboard: its width and pixels.
type TakesPicture<C> = fn(&mut C) -> Option<(usize, Vec<u32>)>;

impl<C> Clone for PicturesAndFiles<C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C> Copy for PicturesAndFiles<C> {}

/// Hands the system clipboard its picture and file code (see `PicturesAndFiles`), on this
/// thread, where the interface's clipboard is used.
pub(crate) fn hand_pictures_and_files() {
    #[cfg(windows)]
    crate::windows::hand_pictures_and_files();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::x11::hand_pictures_and_files();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::wayland::hand_pictures_and_files();
}

/// Hands the windows their drop handling (files and text dragged in from other programs), on
/// this thread: the first time an app asks about drops
/// ([`Ui::files_over`](crate::Ui::files_over), [`Ui::files_dropped`](crate::Ui::files_dropped),
/// [`Ui::text_over`](crate::Ui::text_over), [`Ui::text_dropped`](crate::Ui::text_dropped)).
/// Reached no other way, so a program that never asks is built without it, and its windows
/// accept no drops.
pub(crate) fn hand_drops() {
    #[cfg(windows)]
    crate::windows::hand_drops();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::x11::hand_drops();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::wayland::hand_drops();
}

/// Hands the windows their drag-out code, on this thread: the first time an app hands something
/// out ([`Ui::hand_out`](crate::Ui::hand_out)). Reached no other way, so a program that never
/// does is built without it. (On Windows OLE hands it to the system, see `ole::Outward`; on X11
/// the windows speak XDND themselves.)
pub(crate) fn hands_out() {
    #[cfg(windows)]
    crate::windows::hands_out();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::x11::hands_out();
}

/// Hands the windows their way to ask for the system's file dialog, on this thread: the first
/// time an app asks for it ([`Files::system`](crate::Files::system)). Reached no other way, so
/// a program that never asks is built without it. (On Linux the desktop portal is asked, on
/// Windows the system itself.)
pub(crate) fn hand_file_dialog() {
    #[cfg(windows)]
    crate::windows::hand_file_dialog();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::portal::hand_file_dialog();
}

/// Hands the windows their desktop integration, on this thread: the first time an app asks for
/// a tray icon ([`Ui::tray`](crate::Ui::tray)), a system notification
/// ([`Ui::system_notification`](crate::Ui::system_notification)), a global hotkey
/// ([`Ui::hotkey`](crate::Ui::hotkey)), to hide its window
/// ([`Ui::hide_window`](crate::Ui::hide_window)), or to be the single running instance
/// (`first_copy`). Reached no other way, so a program that never asks is built without it. (On
/// Windows through the shell, in `shell`; on Linux through D-Bus, in `freedesktop`, and X for
/// hotkeys.)
pub(crate) fn hand_desktop() {
    #[cfg(windows)]
    crate::windows::hand_desktop();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::x11::hand_desktop();
    #[cfg(all(unix, not(target_os = "macos")))]
    crate::wayland::hand_desktop();
}

/// An in-process clipboard, for where there is no system to ask: what is copied stays inside
/// the program.
#[derive(Default)]
pub struct LocalClipboard {
    /// The text copied last.
    pub text: String,
    /// The picture copied last: its width and pixels.
    pub picture: Option<(usize, Vec<u32>)>,
    /// The files copied last.
    pub files: Vec<PathBuf>,
}

impl std::fmt::Debug for LocalClipboard {
    /// What is on it: the text, the picture's size (width, height), and the files.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let picture = self.picture.as_ref().map(|(width, pixels)| (*width, pixels.len() / (*width).max(1)));
        f.debug_struct("LocalClipboard").field("text", &self.text).field("picture", &picture).field("files", &self.files).finish()
    }
}

impl Clipboard for LocalClipboard {
    fn get(&mut self) -> Option<String> {
        (!self.text.is_empty()).then(|| self.text.clone())
    }
    fn set(&mut self, text: &str) {
        *self = LocalClipboard { text: text.into(), ..Default::default() };
    }
    fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
        self.picture.clone()
    }
    fn set_picture(&mut self, width: usize, pixels: &[u32]) {
        *self = LocalClipboard { picture: Some((width, pixels.to_vec())), ..Default::default() };
    }
    fn files(&mut self) -> Vec<PathBuf> {
        self.files.clone()
    }
    fn set_files(&mut self, files: &[PathBuf]) {
        *self = LocalClipboard { files: files.to_vec(), ..Default::default() };
    }
}

/// The system's text renderer, ready for [`Ui::new`](crate::Ui::new): [`SystemText`] where the
/// system has what it needs (on Linux, FreeType and Fontconfig), else the library's plain
/// built-in letters, [`BuiltInFont`](crate::text::BuiltInFont). What a program drawing into its
/// own pixels, or a test wanting the pictures it will ship, hands its interface:
///
/// ```no_run
/// let mut ui = taut::Ui::new(taut::platform::system_text(), Box::new(taut::LocalClipboard::default()));
/// ```
pub fn system_text() -> Box<dyn crate::text::TextSystem> {
    match SystemText::new() {
        Some(text) => Box::new(text),
        None => Box::new(crate::text::BuiltInFont),
    }
}

#[cfg(windows)]
pub use crate::windows::{Messages, SystemClipboard};
// Every system has these, each its own: the docs mark them as no single system's.
#[cfg(all(unix, not(target_os = "macos")))]
#[cfg_attr(docsrs, doc(auto_cfg = false))]
pub use crate::wayland::run;
#[cfg(windows)]
#[cfg_attr(docsrs, doc(auto_cfg = false))]
pub use crate::windows::{SystemText, Waker, run};
#[cfg(all(unix, not(target_os = "macos")))]
#[cfg_attr(docsrs, doc(auto_cfg = false))]
pub use crate::x11::{SystemText, Waker};
