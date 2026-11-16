//! Where the file dialog offers to go at once, as the system says: the person's folders
//! (Windows' known folders, wherever they moved and named in the person's language; on Linux
//! those in the desktop's user-dirs file, in any language), drives and mounted volumes, and on
//! Linux the root. A folder's English name is only guessed where the system says nothing.
//!
//! Also the local time a file's date is shown in: the system's time zone, with daylight saving,
//! at the moment the file was written.
use std::path::{Path, PathBuf};

/// A place to go to at once: its name, and the folder.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    pub name: String,
    pub folder: PathBuf,
}

impl Place {
    /// A folder, named by the last part of its path (all of it, for a drive's root).
    fn of(folder: PathBuf) -> Place {
        Place { name: folder.file_name().map_or(folder.to_string_lossy(), |name| name.to_string_lossy()).into_owned(), folder }
    }
}

/// The person's folder, named `home`, then those of the usual folders in it that exist:
/// desktop, documents, downloads, music, pictures, videos.
pub(crate) fn own_folders(home: &str) -> Vec<Place> {
    let mut found = system_folders();
    if let Some(first) = found.first_mut() {
        first.name = home.into();
    }
    let mut places: Vec<Place> = Vec::new();
    for place in found.into_iter().filter(|place| place.folder.is_dir()) {
        if !places.iter().any(|known| known.folder == place.folder) {
            places.push(place);
        }
    }
    places
}

/// The person's folder and the usual ones as Windows knows them: by known-folder id, wherever
/// they moved.
#[cfg(windows)]
fn system_folders() -> Vec<Place> {
    const KNOWN: [[u32; 4]; 7] = [
        guid(0x5E6C_858F, 0x0E22, 0x4760, [0x9A, 0xFE, 0xEA, 0x33, 0x17, 0xB6, 0x71, 0x73]), // home
        guid(0xB4BF_CC3A, 0xDB2C, 0x424C, [0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41]), // desktop
        guid(0xFDD3_9AD0, 0x238F, 0x46AF, [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7]), // documents
        guid(0x374D_E290, 0x123F, 0x4565, [0x91, 0x64, 0x39, 0xC4, 0x92, 0x5E, 0x46, 0x7B]), // downloads
        guid(0x4BD8_D571, 0x6D19, 0x48D3, [0xBE, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0E, 0x43]), // music
        guid(0x33E2_8130, 0x4E1E, 0x4676, [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB]), // pictures
        guid(0x1898_9B1D, 0x99B5, 0x455B, [0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC]), // videos
    ];
    KNOWN.iter().filter_map(known_folder).map(|folder| Place { name: shown_name(&folder), folder }).collect()
}

/// The same on Linux: the person's folder and those named in `~/.config/user-dirs.dirs`, or
/// where it names none, the usual English names, if such folders exist.
#[cfg(not(windows))]
fn system_folders() -> Vec<Place> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let settings = std::env::var_os("XDG_CONFIG_HOME").map_or(home.join(".config"), PathBuf::from);
    let named = std::fs::read_to_string(settings.join("user-dirs.dirs")).unwrap_or_default();
    user_folders(&home, &named).into_iter().map(Place::of).collect()
}

/// The home folder `home`, then desktop, documents, downloads, music, pictures and videos,
/// where `named` (lines like `XDG_DOCUMENTS_DIR="$HOME/Papiere"`) puts them, else in the usual
/// place.
#[cfg_attr(windows, allow(dead_code))]
fn user_folders(home: &Path, named: &str) -> Vec<PathBuf> {
    let usual = [("DESKTOP", "Desktop"), ("DOCUMENTS", "Documents"), ("DOWNLOAD", "Downloads"), ("MUSIC", "Music"), ("PICTURES", "Pictures"), ("VIDEOS", "Videos")];
    let said = |key: &str| named.lines().find(|line| line.trim_start().strip_prefix("XDG_").and_then(|line| line.strip_prefix(key)).is_some_and(|rest| rest.starts_with("_DIR=")));
    let folder = |line: &str| {
        let folder = line.split('"').nth(1)?;
        Some(folder.strip_prefix("$HOME").map_or(PathBuf::from(folder), |rest| home.join(rest.trim_start_matches('/'))))
    };
    let found = usual.iter().map(|(key, english)| said(key).map_or(Some(home.join(english)), folder));
    std::iter::once(home.to_path_buf()).chain(found.flatten()).collect()
}

/// The drives, each by its name.
#[cfg(windows)]
pub(crate) fn volumes() -> Vec<Place> {
    drive_roots().map(|root| Place { name: drive_name(&root), folder: root }).collect()
}

/// The mounted volumes, each named by the last part of its path, and the root.
#[cfg(not(windows))]
pub(crate) fn volumes() -> Vec<Place> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut places: Vec<Place> = mounted(&std::fs::read_to_string("/proc/self/mounts").unwrap_or_default(), &user).into_iter().map(Place::of).collect();
    places.push(Place { name: "/".into(), folder: PathBuf::from("/") });
    places
}

/// The drive roots the computer has (`C:` and so on), as Windows lists them. None is touched,
/// so an empty drive keeps nobody waiting.
pub(crate) fn drive_roots() -> impl Iterator<Item = PathBuf> {
    #[cfg(windows)]
    let there = crate::windows::drives();
    #[cfg(not(windows))]
    let there = 0u32;
    ('A'..='Z').enumerate().filter(move |(bit, _)| there >> bit & 1 == 1).map(|(_, letter)| PathBuf::from(format!("{letter}:\\")))
}

/// The volumes a person mounted or plugged in, from `/proc/self/mounts`: those under `/media`,
/// `/run/media/<user>` or `/mnt`, of a kind that holds files (not one in memory only).
#[cfg_attr(windows, allow(dead_code))]
fn mounted(mounts: &str, user: &str) -> Vec<PathBuf> {
    const IN_MEMORY: [&str; 8] = ["tmpfs", "ramfs", "overlay", "squashfs", "autofs", "proc", "sysfs", "devtmpfs"];
    let media = Path::new("/run/media").join(user);
    let mut found: Vec<PathBuf> = Vec::new();
    for line in mounts.lines() {
        let [_, Some(point), Some(kind)] = [0, 1, 2].map(|nth| line.split_whitespace().nth(nth)) else { continue };
        let point = PathBuf::from(unescaped(point));
        let within = point.parent().is_some_and(|parent| parent == Path::new("/mnt") || parent == Path::new("/media") || parent == media || parent.parent() == Some(Path::new("/media")));
        if within && !IN_MEMORY.contains(&kind) && !found.contains(&point) {
            found.push(point);
        }
    }
    found
}

/// A name from the mounts list, where space, tab, newline and backslash are written as a
/// backslash and three octal digits (`\040`).
#[cfg_attr(windows, allow(dead_code))]
fn unescaped(name: &str) -> String {
    let mut written = String::new();
    let mut rest = name;
    while let Some(at) = rest.find('\\') {
        let code = rest.get(at + 1..at + 4).and_then(|digits| u8::from_str_radix(digits, 8).ok());
        written.push_str(&rest[..at]);
        written.push(code.map_or('\\', char::from));
        rest = &rest[at + if code.is_some() { 4 } else { 1 }..];
    }
    written + rest
}

/// The local date and time `seconds` after 1970-01-01 UTC: year, month, day, hour, minute.
#[cfg(windows)]
pub(crate) fn local_time(seconds: i64) -> [u16; 5] {
    // Windows counts 100 ns ticks since 1601.
    let ticks = (seconds.max(-11_644_473_600) + 11_644_473_600) as u64 * 10_000_000;
    let (mut greenwich, mut here) = ([0u16; 8], [0u16; 8]);
    // SAFETY: the system fills in the values it is handed, which outlive the calls; with no
    // time zone named, it uses the person's.
    let known = unsafe { FileTimeToSystemTime(&ticks, &mut greenwich) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &greenwich, &mut here) != 0 };
    let [year, month, _, day, hour, minute, ..] = if known { here } else { greenwich };
    [year, month, day, hour, minute]
}

/// The parts of a moment as the C library gives them (`struct tm`), the same on every Unix.
#[cfg(unix)]
#[repr(C)]
struct Moment {
    second: i32,
    minute: i32,
    hour: i32,
    day: i32,
    month: i32,
    year: i32,
    rest: [i32; 3],
    ahead: i64,
    zone: *const u8,
}

#[cfg(unix)]
unsafe extern "C" {
    fn localtime_r(time: *const i64, into: *mut Moment) -> *mut Moment;
}

#[cfg(unix)]
pub(crate) fn local_time(seconds: i64) -> [u16; 5] {
    let mut moment = Moment { second: 0, minute: 0, hour: 0, day: 1, month: 0, year: 70, rest: [0; 3], ahead: 0, zone: std::ptr::null() };
    // SAFETY: the C library fills in the moment; both pointers are to values that outlive the
    // call.
    unsafe { localtime_r(&seconds, &mut moment) };
    [moment.year + 1900, moment.month + 1, moment.day, moment.hour, moment.minute].map(|part| part.clamp(0, u16::MAX as i32) as u16)
}

/// With no system to ask about time zones: UTC.
#[cfg(not(any(windows, unix)))]
pub(crate) fn local_time(seconds: i64) -> [u16; 5] {
    let date = crate::date::Date::from_days(seconds.div_euclid(86_400));
    let minutes = seconds.rem_euclid(86_400) / 60;
    [date.year.clamp(0, u16::MAX as i32) as u16, date.month as u16, date.day as u16, (minutes / 60) as u16, (minutes % 60) as u16]
}

/// A GUID, as Windows knows known folders, as four numbers.
#[cfg(windows)]
const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u32; 4] {
    [a, b as u32 | (c as u32) << 16, u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])]
}

#[cfg(windows)]
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(folder: *const [u32; 4], flags: u32, person: isize, path: *mut *mut u16) -> i32;
    fn SHGetLocalizedName(path: *const u16, module: *mut u16, room: u32, id: *mut i32) -> i32;
}
#[cfg(windows)]
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *mut core::ffi::c_void);
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ExpandEnvironmentStringsW(from: *const u16, to: *mut u16, room: u32) -> u32;
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn FreeLibrary(module: isize) -> i32;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetVolumeInformationW(root: *const u16, name: *mut u16, room: u32, serial: *mut u32, longest: *mut u32, flags: *mut u32, system: *mut u16, system_room: u32) -> i32;
    fn FileTimeToSystemTime(time: *const u64, moment: *mut [u16; 8]) -> i32;
    fn SystemTimeToTzSpecificLocalTime(zone: *const core::ffi::c_void, greenwich: *const [u16; 8], here: *mut [u16; 8]) -> i32;
}
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn LoadStringW(module: isize, id: u32, text: *mut u16, room: i32) -> i32;
}

/// A path as Windows takes it: UTF-16, ending in a zero.
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The UTF-16 text up to its first zero.
#[cfg(windows)]
fn up_to_zero(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|unit| *unit == 0).unwrap_or(text.len())])
}

/// Where a known folder is, if the person has it.
#[cfg(windows)]
fn known_folder(id: &[u32; 4]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut path = std::ptr::null_mut();
    // SAFETY: the system writes a pointer to the path in memory the caller frees (found or
    // not); the path ends at its first zero.
    unsafe {
        let found = SHGetKnownFolderPath(id, 0, 0, &mut path) >= 0 && !path.is_null();
        let length = if found { (0..).take_while(|at| *path.add(*at) != 0).count() } else { 0 };
        let folder = found.then(|| PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, length))));
        CoTaskMemFree(path.cast());
        folder
    }
}

/// A folder's name as Windows shows it: the localized name the folder declares (in its
/// `desktop.ini`, as a string in a system file), or else the last part of its path.
#[cfg(windows)]
fn shown_name(folder: &Path) -> String {
    let (mut module, mut expanded, mut text, mut id) = ([0u16; 260], [0u16; 260], [0u16; 260], 0i32);
    // SAFETY: each buffer is as long as said, and each text read ends in a zero; the file
    // holding the name is opened for its strings only, then closed.
    let length = unsafe {
        let said = SHGetLocalizedName(wide(folder).as_ptr(), module.as_mut_ptr(), 260, &mut id) >= 0;
        let file = if said && ExpandEnvironmentStringsW(module.as_ptr(), expanded.as_mut_ptr(), 260) != 0 { LoadLibraryExW(expanded.as_ptr(), 0, 0x2 | 0x20) } else { 0 };
        let length = if file != 0 { LoadStringW(file, id.unsigned_abs(), text.as_mut_ptr(), 260) } else { 0 };
        if file != 0 {
            FreeLibrary(file);
        }
        length
    };
    match length > 0 {
        true => String::from_utf16_lossy(&text[..length as usize]),
        false => Place::of(folder.to_path_buf()).name,
    }
}

/// A drive's name: its letter after its label, if any (`Data (D:)`). Network drives and discs
/// are not asked: a slow network or a disc spinning up would keep the dialog waiting.
#[cfg(windows)]
fn drive_name(root: &Path) -> String {
    let (letter, path, mut label) = (root.to_string_lossy().trim_end_matches('\\').to_string(), wide(root), [0u16; 261]);
    // SAFETY: the root ends in a zero, and the name buffer is as long as said; nothing else is
    // asked.
    let named = crate::listing::quietly(|| unsafe {
        matches!(GetDriveTypeW(path.as_ptr()), 2 | 3 | 6)
            && GetVolumeInformationW(path.as_ptr(), label.as_mut_ptr(), 261, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) != 0
    });
    let label = if named { up_to_zero(&label) } else { String::new() };
    if label.is_empty() { letter } else { format!("{label} ({letter})") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folders_of_the_desktop_file_are_found_and_the_usual_ones_where_it_names_none() {
        let home = Path::new("/home/someone");
        let named = "# where things are kept
XDG_DOCUMENTS_DIR=\"$HOME/Papiere\"
XDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"
XDG_MUSIC_DIR=\"$HOME/\"
XDG_VIDEOS_DIR=\"/data/Filme\"
";
        let found = user_folders(home, named);
        let expected = [".", "Schreibtisch", "Papiere", "Downloads", ".", "Pictures", "/data/Filme"].map(|folder| match folder {
            "." => home.to_path_buf(),
            folder => home.join(folder),
        });
        assert_eq!(found, expected);
    }

    #[test]
    fn mounted_volumes_are_the_ones_a_person_mounts() {
        let mounts = "none /mnt/wsl tmpfs rw 0 0
/dev/sdd / ext4 rw 0 0
none /mnt/wslg/doc overlay rw 0 0
C:\\134 /mnt/c 9p rw 0 0
/dev/sdb1 /media/someone/USB\\040STICK vfat rw 0 0
/dev/sdc1 /run/media/someone/Backup ext4 rw 0 0
/dev/sdc2 /run/media/other/Theirs ext4 rw 0 0
proc /proc proc rw 0 0
";
        let found = mounted(mounts, "someone");
        assert_eq!(found, ["/mnt/c", "/media/someone/USB STICK", "/run/media/someone/Backup"].map(PathBuf::from));
        assert_eq!(Place::of(found[1].clone()).name, "USB STICK");
    }

    #[test]
    fn the_persons_own_folders_are_there_and_the_drives_or_the_top_of_everything() {
        let own = own_folders("Home");
        assert_eq!(own.first().map(|place| place.name.as_str()), Some("Home"));
        assert!(own.iter().all(|place| place.folder.is_dir() && !place.name.is_empty()), "{own:?}");
        let volumes = volumes();
        assert!(!volumes.is_empty() && volumes.iter().all(|place| place.folder.is_dir()), "{volumes:?}");
        #[cfg(windows)]
        assert!(volumes.iter().all(|place| place.name.ends_with(':') || place.name.ends_with(":)")), "{volumes:?}");
        #[cfg(not(windows))]
        assert_eq!(volumes.last().map(|place| place.name.as_str()), Some("/"));
        // On an English system the known folders have their English names.
        #[cfg(windows)]
        for name in ["Desktop", "Documents", "Downloads"] {
            assert!(own.iter().any(|place| place.name == name), "{name} among {own:?}");
        }
    }

    #[test]
    fn a_moment_is_told_in_the_time_here() {
        // 2026-06-01 noon UTC: that day or the next anywhere, with the minute kept (time zones
        // are whole quarter hours).
        let [year, month, day, _, minute] = local_time(1_780_315_200);
        assert_eq!((year, month, minute % 15), (2026, 6, 0));
        assert!((1..=2).contains(&day), "{year}-{month}-{day}");
    }
}
