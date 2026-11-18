//! Where an app keeps its files between runs (settings, window and table layout; see the
//! guide's "Saving settings between runs"), and writing a file so it is never left half
//! written.
//!
//! ```no_run
//! let file = taut::storage::folder("Notes").expect("a name that can be a folder").join("kept.txt");
//! let text = taut::storage::read(&file).unwrap_or_default();
//! // … the app reads its values from `text`, runs, and writes them out again as it closes:
//! taut::storage::write(&file, "300 200 1000 640 0\n").expect("the disk takes it");
//! ```
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The folder app `app` keeps its files in, for the current user:
///
/// | System  | Folder |
/// |---------|--------|
/// | Windows | `%APPDATA%\app` (roaming) |
/// | Linux   | `$XDG_CONFIG_HOME/app`, else `~/.config/app` |
/// | macOS   | `~/Library/Application Support/app` |
///
/// Not created here: [`write()`] makes it. `None` if `app` cannot be one folder's name (empty,
/// `.`, `..`, or with `/`, `\` or `:`), or the system names no user folder.
pub fn folder(app: &str) -> Option<PathBuf> {
    folder_with(app, |name| std::env::var_os(name))
}

/// [`folder`], with the variables read through `var` (for tests).
fn folder_with(app: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let refused = |letter: char| letter.is_control() || "/\\:*?\"<>|".contains(letter);
    if app.is_empty() || app == "." || app == ".." || app.chars().any(refused) {
        return None;
    }
    // An empty or relative variable counts as none, as the XDG spec says.
    let set = |name: &str| var(name).map(PathBuf::from).filter(|place| place.is_absolute());
    let home = set(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let base = if cfg!(windows) {
        set("APPDATA").or_else(|| home.map(|home| home.join("AppData").join("Roaming")))
    } else if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library").join("Application Support"))
    } else {
        set("XDG_CONFIG_HOME").or_else(|| home.map(|home| home.join(".config")))
    };
    base.map(|base| base.join(app))
}

/// Writes `text` into `file` whole or not at all: into a file beside it, which then replaces
/// it. If the program stops or the disk is full, `file` is as it was (a stopped program may
/// leave `file` + its process id + `.new` beside it). Creates the folder if missing.
pub fn write(file: &Path, text: &str) -> io::Result<()> {
    let name = file.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a file needs a name"))?;
    let folder = file.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder)?;
    // One temporary name per process: two writers never share one.
    let mut beside = name.to_os_string();
    beside.push(format!(".{}.new", std::process::id()));
    let beside = folder.join(beside);
    let written = File::create(&beside).and_then(|mut new| new.write_all(text.as_bytes()).and_then(|()| new.sync_all()));
    let done = written.and_then(|()| std::fs::rename(&beside, file));
    if done.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    // On Linux the rename is durable only once the folder is synced: a crash after this keeps
    // one file or the other, whole.
    #[cfg(unix)]
    if done.is_ok() {
        let _ = File::open(folder).and_then(|folder| folder.sync_all());
    }
    done
}

/// What was written into `file`; `None` if it is missing or not text.
pub fn read(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system's variables, by name.
    fn vars(set: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| set.iter().find(|(wanted, _)| *wanted == name).map(|(_, value)| value.into())
    }

    #[test]
    fn an_app_keeps_its_files_where_the_system_keeps_a_persons_settings() {
        if cfg!(windows) {
            let set = vars(&[("APPDATA", r"C:\Users\a\AppData\Roaming"), ("USERPROFILE", r"C:\Users\a")]);
            assert_eq!(folder_with("Notes", set), Some(PathBuf::from(r"C:\Users\a\AppData\Roaming\Notes")));
            // No variable for it: the same folder through the user's profile.
            assert_eq!(folder_with("Notes", vars(&[("USERPROFILE", r"C:\Users\a")])), Some(PathBuf::from(r"C:\Users\a\AppData\Roaming\Notes")));
            assert_eq!(folder_with("Notes", vars(&[("APPDATA", ""), ("USERPROFILE", r"C:\Users\a")])), Some(PathBuf::from(r"C:\Users\a\AppData\Roaming\Notes")));
        } else if cfg!(target_os = "macos") {
            assert_eq!(folder_with("Notes", vars(&[("HOME", "/Users/a")])), Some(PathBuf::from("/Users/a/Library/Application Support/Notes")));
        } else {
            assert_eq!(folder_with("Notes", vars(&[("XDG_CONFIG_HOME", "/home/a/settings"), ("HOME", "/home/a")])), Some(PathBuf::from("/home/a/settings/Notes")));
            assert_eq!(folder_with("Notes", vars(&[("HOME", "/home/a")])), Some(PathBuf::from("/home/a/.config/Notes")));
            // An empty or relative variable is passed over.
            assert_eq!(folder_with("Notes", vars(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/a")])), Some(PathBuf::from("/home/a/.config/Notes")));
            assert_eq!(folder_with("Notes", vars(&[("XDG_CONFIG_HOME", "settings"), ("HOME", "/home/a")])), Some(PathBuf::from("/home/a/.config/Notes")));
        }
        // Nowhere to keep anything, and names that cannot be one folder.
        assert_eq!(folder_with("Notes", vars(&[])), None);
        let home = vars(&[("APPDATA", r"C:\Users\a\AppData\Roaming"), ("USERPROFILE", r"C:\Users\a"), ("HOME", "/home/a")]);
        for name in ["", ".", "..", "a/b", r"a\b", "C:", "a\nb", "a?"] {
            assert_eq!(folder_with(name, &home), None, "{name:?}");
        }
        assert!(folder_with("My Notes 2.0", &home).is_some());
        assert!(folder_with("Notes", &home).unwrap().ends_with("Notes"));
    }

    #[test]
    fn a_file_is_written_whole_or_left_as_it_was() {
        let work = std::env::temp_dir().join(format!("taut-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        // Into a folder not there yet; and read back.
        let file = work.join("Notes").join("kept.txt");
        assert_eq!(read(&file), None);
        write(&file, "first\n").unwrap();
        assert_eq!(read(&file).as_deref(), Some("first\n"));
        // Over the old file, leaving nothing beside it.
        write(&file, "second\nand more\n").unwrap();
        assert_eq!(read(&file).as_deref(), Some("second\nand more\n"));
        let left: Vec<_> = std::fs::read_dir(work.join("Notes")).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(left, ["kept.txt"]);
        // If the new file cannot replace the old (a non-empty folder has its name), the old one
        // stays and nothing is left beside it.
        let taken = work.join("taken");
        write(&taken.join("inside.txt"), "kept").unwrap();
        assert!(write(&taken, "never").is_err());
        assert_eq!(read(&taken.join("inside.txt")).as_deref(), Some("kept"));
        let left: Vec<_> = std::fs::read_dir(&work).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        // No folder inside a file; and a file needs a name.
        assert!(write(&file.join("under"), "never").is_err());
        assert!(write(Path::new(""), "never").is_err());
        assert_eq!(read(&file).as_deref(), Some("second\nand more\n"));
        std::fs::remove_dir_all(&work).unwrap();
    }
}
