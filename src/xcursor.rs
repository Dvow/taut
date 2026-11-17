//! Pointer pictures from the desktop's cursor theme, for Wayland compositors that cannot show a
//! shape by name: the program shows the pointer as a small surface of its own.
//!
//! A theme is a folder of Xcursor files, one per shape, each with pictures at several sizes
//! (and frames for animated ones: the first is used). The theme and size are `XCURSOR_THEME`
//! and `XCURSOR_SIZE`; a missing shape is looked for in the themes it inherits, as the desktop
//! does. A file is read when its shape is first shown, and only the wanted size is kept.
use crate::event::Cursor;
use crate::wayland_wire::{Arg, Interface, Shared, Wire};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

/// The file's magic, the type of an image entry in its table, and the image header's length.
const MAGIC: &[u8; 4] = b"Xcur";
const PICTURE: u32 = 0xfffd_0002;
const HEAD: usize = 36;

/// One picture of a shape: its size, its hot spot, and its pixels, `0xAARRGGBB` premultiplied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Picture {
    pub(crate) size: [i32; 2],
    pub(crate) hot: [i32; 2],
    pub(crate) pixels: Vec<u32>,
}

/// The names a shape goes by in themes, newest first.
fn names(cursor: Cursor) -> &'static [&'static str] {
    match cursor {
        Cursor::Arrow => &["default", "left_ptr"],
        Cursor::Hand => &["pointer", "hand2", "hand1"],
        Cursor::Text => &["text", "xterm"],
        Cursor::ResizeX => &["ew-resize", "col-resize", "sb_h_double_arrow"],
        Cursor::ResizeY => &["ns-resize", "row-resize", "sb_v_double_arrow"],
        Cursor::ResizeDown => &["nwse-resize", "bd_double_arrow", "size_fdiag"],
        Cursor::ResizeUp => &["nesw-resize", "fd_double_arrow", "size_bdiag"],
        Cursor::Move => &["move", "fleur", "all-scroll"],
        Cursor::No => &["not-allowed", "crossed_circle", "forbidden"],
    }
}

/// The picture in an Xcursor file nearest to `size` (the first frame of an animated one).
/// Nothing for a file that is not one, or holds no picture.
pub(crate) fn pick(file: &[u8], size: u32) -> Option<Picture> {
    let word = |at: usize| file.get(at..at + 4).and_then(|word| word.first_chunk()).map(|word| u32::from_le_bytes(*word));
    if file.get(..4) != Some(MAGIC) {
        return None;
    }
    let (head, count) = (word(4)? as usize, word(12)? as usize);
    let listed = (0..count.min(4096)).filter_map(|nth| Some((word(head + nth * 12)?, word(head + nth * 12 + 4)?, word(head + nth * 12 + 8)? as usize)));
    let (_, _, at) = listed.filter(|(kind, ..)| *kind == PICTURE).min_by_key(|(_, nominal, _)| nominal.abs_diff(size))?;
    let [wide, high, x, y] = [at + 16, at + 20, at + 24, at + 28].map(|at| word(at).unwrap_or(0));
    if word(at)? as usize != HEAD || wide == 0 || high == 0 || wide > 1024 || high > 1024 {
        return None;
    }
    let pixels: Option<Vec<u32>> = (0..(wide * high) as usize).map(|nth| word(at + HEAD + nth * 4)).collect();
    Some(Picture { size: [wide as i32, high as i32], hot: [x as i32, y as i32], pixels: pixels? })
}

/// The folders themes are looked for in: `XCURSOR_PATH`, or the desktop's usual ones.
fn places() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data = std::env::var_os("XDG_DATA_HOME").map_or(home.join(".local/share"), PathBuf::from);
    match std::env::var_os("XCURSOR_PATH") {
        Some(paths) => std::env::split_paths(&paths).collect(),
        None => vec![data.join("icons"), home.join(".icons"), "/usr/share/icons".into(), "/usr/share/pixmaps".into()],
    }
}

/// A shape's file in a theme or the themes it inherits, nearest first: each theme is looked for
/// in every folder, and where it lacks the shape, the themes its `index.theme` names are tried
/// next. Nothing where none has it.
pub(crate) fn file_of(theme: &str, cursor: Cursor, places: &[PathBuf]) -> Option<PathBuf> {
    let (mut themes, mut at) = (vec![theme.to_string()], 0);
    while let Some(theme) = themes.get(at).cloned() {
        at += 1;
        let folders = places.iter().map(|place| place.join(&theme).join("cursors"));
        let found = folders.flat_map(|folder| names(cursor).iter().map(move |name| folder.join(name))).find(|file| file.is_file());
        if found.is_some() {
            return found;
        }
        let index = places.iter().find_map(|place| std::fs::read_to_string(place.join(&theme).join("index.theme")).ok()).unwrap_or_default();
        let builds_on = index.lines().find_map(|line| line.trim().strip_prefix("Inherits")?.trim_start().strip_prefix('=')).unwrap_or_default();
        for other in builds_on.split([',', ';']).map(str::trim).filter(|other| !other.is_empty()) {
            if !themes.iter().any(|known| known == other) && themes.len() < 16 {
                themes.push(other.into());
            }
        }
    }
    None
}

/// The pointer the program shows: its surface, the theme and size, the pictures read so far (by
/// shape and whole scale), and the buffers the compositor may still show (with their memory)
/// until it releases them.
pub(crate) struct Pointers {
    surface: u32,
    theme: String,
    size: u32,
    places: Vec<PathBuf>,
    read: Vec<(Cursor, i32, Option<Picture>)>,
    buffers: Vec<(u32, u32, Shared)>,
}

impl Pointers {
    /// The pointer's surface, before anything is shown on it.
    pub(crate) fn new(wire: &Wire, compositor: u32) -> Self {
        let surface = wire.make(Interface::Surface);
        wire.send(compositor, 0, &[Arg::Uint(surface)]);
        let theme = std::env::var("XCURSOR_THEME").ok().filter(|theme| !theme.is_empty()).unwrap_or_else(|| "default".into());
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|size| size.parse().ok()).filter(|size| *size > 0).unwrap_or(24);
        Pointers { surface, theme, size, places: places(), read: Vec::new(), buffers: Vec::new() }
    }

    /// A shape's picture at a whole scale, read the first time it is asked for.
    fn picture(&mut self, cursor: Cursor, scale: i32) -> Option<Picture> {
        if let Some((.., kept)) = self.read.iter().find(|(shape, at, _)| (*shape, *at) == (cursor, scale)) {
            return kept.clone();
        }
        let file = file_of(&self.theme, cursor, &self.places).and_then(|file| std::fs::read(file).ok());
        let picked = file.and_then(|file| pick(&file, self.size * scale as u32));
        self.read.push((cursor, scale, picked.clone()));
        picked
    }

    /// Shows a shape as the pointer (`pointer` is the program's pointer object, `serial` the
    /// enter event's serial): a buffer of its picture at the scale of the window under it. A
    /// shape the theme lacks leaves the pointer as it is.
    pub(crate) fn show(&mut self, wire: &Wire, shm: u32, pointer: u32, serial: u32, cursor: Cursor, scale: f32) {
        let whole = (scale.ceil() as i32).max(1);
        let Some(picture) = self.picture(cursor, whole).or_else(|| self.picture(Cursor::Arrow, whole)) else { return };
        let len = picture.pixels.len() * 4;
        let Some(mut memory) = Shared::new(len) else { return };
        memory.pixels().copy_from_slice(&picture.pixels);
        let (pool, buffer) = (wire.make(Interface::Pool), wire.make(Interface::Buffer));
        wire.send(shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(len as i32)]);
        let [wide, high] = picture.size;
        wire.send(pool, 0, &[Arg::Uint(buffer), Arg::Int(0), Arg::Int(wide), Arg::Int(high), Arg::Int(wide * 4), Arg::Uint(0)]);
        wire.send(self.surface, 1, &[Arg::Object(buffer), Arg::Int(0), Arg::Int(0)]);
        wire.send(self.surface, 8, &[Arg::Int(whole)]);
        wire.send(self.surface, 2, &[Arg::Int(0), Arg::Int(0), Arg::Int(wide), Arg::Int(high)]);
        wire.send(self.surface, 6, &[]);
        let [x, y] = picture.hot.map(|at| at / whole);
        wire.send(pointer, 0, &[Arg::Uint(serial), Arg::Object(self.surface), Arg::Int(x), Arg::Int(y)]);
        self.buffers.push((buffer, pool, memory));
    }

    /// The compositor released a buffer: if it is the pointer's and not the last shown, it
    /// goes.
    pub(crate) fn released(&mut self, wire: &Wire, id: u32) {
        let last = self.buffers.last().map(|(buffer, ..)| *buffer);
        if let Some(at) = self.buffers.iter().position(|(buffer, ..)| *buffer == id && Some(id) != last) {
            let (buffer, pool, _) = self.buffers.remove(at);
            wire.send(buffer, 0, &[]);
            wire.send(pool, 1, &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Xcursor file with pictures of the given sizes, each `size` pixels square (all `fill`,
    /// hot spot in the middle), with another entry listed before them.
    fn file(sizes: &[(u32, u32)]) -> Vec<u8> {
        let mut file = Vec::new();
        let words = |file: &mut Vec<u8>, words: &[u32]| words.iter().for_each(|word| file.extend_from_slice(&word.to_le_bytes()));
        words(&mut file, &[u32::from_le_bytes(*MAGIC), 16, 0x1_0000, sizes.len() as u32 + 1]);
        let mut at = 16 + 12 * (sizes.len() + 1);
        words(&mut file, &[0xfffe_0001, 1, 0]); // a comment entry, skipped
        for (size, _) in sizes {
            words(&mut file, &[PICTURE, *size, at as u32]);
            at += HEAD + (size * size * 4) as usize;
        }
        for (size, fill) in sizes {
            words(&mut file, &[HEAD as u32, PICTURE, *size, 1, *size, *size, size / 2, size / 2, 0]);
            words(&mut file, &vec![*fill; (size * size) as usize]);
        }
        file
    }

    #[test]
    fn the_picture_nearest_the_size_asked_for_is_taken() {
        let file = file(&[(24, 0xff00_0000), (32, 0xffff_ffff), (48, 0x8080_8080)]);
        let picked = pick(&file, 30).unwrap();
        assert_eq!((picked.size, picked.hot, picked.pixels[0], picked.pixels.len()), ([32, 32], [16, 16], 0xffff_ffff, 32 * 32));
        assert_eq!(pick(&file, 48).unwrap().pixels[5], 0x8080_8080);
        assert_eq!(pick(&file, 1).unwrap().size, [24, 24]);
        assert!(pick(&file[..file.len() - 4], 48).is_none(), "a picture cut short is none");
        assert!(pick(b"not a pointer", 24).is_none());
    }

    #[test]
    fn a_shape_is_found_in_the_theme_or_the_ones_it_builds_on() {
        let folder = std::env::temp_dir().join(format!("taut-pointers-{}", std::process::id()));
        let (own, base) = (folder.join("own"), folder.join("base"));
        std::fs::create_dir_all(own.join("cursors")).unwrap();
        std::fs::create_dir_all(base.join("cursors")).unwrap();
        std::fs::write(own.join("index.theme"), "[Icon Theme]\nName=Own\nInherits = missing, base\n").unwrap();
        std::fs::write(own.join("cursors/left_ptr"), file(&[(24, 1)])).unwrap();
        std::fs::write(base.join("cursors/xterm"), file(&[(24, 2)])).unwrap();
        let places = [folder.clone()];
        assert_eq!(file_of("own", Cursor::Arrow, &places), Some(own.join("cursors/left_ptr")), "by its older name");
        assert_eq!(file_of("own", Cursor::Text, &places), Some(base.join("cursors/xterm")), "from what the theme builds on");
        assert_eq!(file_of("own", Cursor::Hand, &places), None);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn the_desktops_own_theme_is_read() {
        let found = ["Adwaita", "breeze_cursors", "default"].into_iter().find_map(|theme| file_of(theme, Cursor::Arrow, &places()));
        let Some(found) = found else { return eprintln!("no theme of pointers here: not tested") };
        let picture = pick(&std::fs::read(&found).unwrap(), 24).expect("the theme's arrow is read");
        assert!(picture.size[0] >= 16 && picture.pixels.iter().any(|pixel| pixel >> 24 == 0xff), "{found:?}: a picture with something solid in it");
    }
}
