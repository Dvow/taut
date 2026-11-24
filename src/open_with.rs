//! Opening a file, folder or link with the program the system opens it with, as a double click
//! in the file manager does. On Windows the shell (`ShellExecuteW`) uses the person's own
//! choices. On Linux a link goes to the desktop portal (`org.freedesktop.portal.OpenURI`); a
//! file, or a link with no portal, goes to `xdg-open`. (The portal takes files only as open
//! file descriptors, which is not worth it here.)
//!
//! Either runs on a thread of its own, so a window never waits; only a program that calls
//! `open_with_system` carries this.
use std::ffi::OsStr;

/// Opens a file, folder or link (`https://...`, `mailto:...`) with the program the system opens
/// it with, as a double click in the file manager does. A relative path is taken from the
/// working folder. It runs on a thread of its own and reports nothing: a file with no program
/// is handled as the system handles it.
///
/// A program it is handed is started. Hand it what the person chose; check a link someone else
/// wrote (in a document, a message) is of the kind the app means to open, such as `https:`. A
/// link can name a program or a file on another computer, which the system would run.
pub fn open_with_system(what: impl AsRef<OsStr>) {
    let what = what.as_ref().to_os_string();
    // A page opens a link in a new tab.
    #[cfg(target_family = "wasm")]
    return crate::web_host::open(&what.to_string_lossy());
    #[cfg(not(target_family = "wasm"))]
    std::thread::spawn(move || opened(&what));
}

/// Whether `text` is a link: a scheme of two letters or more (a letter, then letters, digits,
/// `+`, `-` or `.`), then a colon. A drive path such as `C:` is not.
#[cfg(any(test, target_os = "ios", target_os = "android", all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
fn is_link(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else { return false };
    let fits = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.');
    scheme.len() > 1 && scheme.starts_with(|c: char| c.is_ascii_alphabetic()) && scheme.chars().all(fits)
}

#[cfg(windows)]
mod shell {
    use std::ffi::c_void;
    #[link(name = "shell32")]
    unsafe extern "system" {
        pub fn ShellExecuteW(hwnd: *mut c_void, verb: *const u16, file: *const u16, parameters: *const u16, folder: *const u16, show: i32) -> *mut c_void;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        pub fn CoInitializeEx(reserved: *mut c_void, how: u32) -> i32;
        pub fn CoUninitialize();
    }
    /// COM on the asking thread, apartment-threaded and without DDE (as `ShellExecuteW` wants);
    /// and `SW_SHOWNORMAL` for the program it starts.
    pub const COINIT_APARTMENTTHREADED: u32 = 0x2;
    pub const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
    pub const SW_SHOWNORMAL: i32 = 1;
}

/// Asks the shell to open `what` with its default verb, as a double click does. Says whether it
/// did (the shell returns more than 32).
#[cfg(windows)]
fn opened(what: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    let wide: Vec<u16> = what.encode_wide().chain([0]).collect();
    // SAFETY: COM is started on this thread, which is the module's own and does nothing else,
    // and ended after; the text the shell reads ends in a zero, and every other argument is
    // null.
    unsafe {
        let com = shell::CoInitializeEx(null_mut(), shell::COINIT_APARTMENTTHREADED | shell::COINIT_DISABLE_OLE1DDE);
        let done = shell::ShellExecuteW(null_mut(), null(), wide.as_ptr(), null(), null(), shell::SW_SHOWNORMAL) as usize > 32;
        if com >= 0 {
            shell::CoUninitialize();
        }
        done
    }
}

/// Hands `what` to the portal on the session bus if it is a link, else (or with no portal) to
/// `xdg-open`. Says whether either took it.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn opened(what: &OsStr) -> bool {
    open_by(what, crate::atspi::Bus::session(), OsStr::new("xdg-open"))
}

/// Hands a link to the app Android opens it with. Says whether one took it. (Files are not
/// opened: an app shares a file through a content provider of its own.)
#[cfg(target_os = "android")]
fn opened(what: &OsStr) -> bool {
    what.to_str().is_some_and(|link| is_link(link) && crate::android::open_link(link))
}

/// Hands `what` to `open`, which opens it as a double click in Finder does. Says whether it
/// did. (A name starting with a dash is given as `./-...`, so it is not read as an option.)
#[cfg(target_os = "macos")]
fn opened(what: &OsStr) -> bool {
    let dashed = what.as_encoded_bytes().starts_with(b"-");
    let what = if dashed { std::path::Path::new(".").join(what).into_os_string() } else { what.to_os_string() };
    std::process::Command::new("/usr/bin/open").arg(what).status().is_ok_and(|status| status.success())
}

/// Hands a link to the system (`UIApplication openURL:`), on the main thread. Files have no
/// program to open them on iOS: says it took none.
#[cfg(target_os = "ios")]
fn opened(what: &OsStr) -> bool {
    what.to_str().is_some_and(|text| is_link(text) && crate::ios::open_link(text))
}

/// The same over `bus`, with `opener` for what the portal does not take.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn open_by(what: &OsStr, bus: Option<crate::atspi::Bus>, opener: &OsStr) -> bool {
    let link = what.to_str().filter(|text| is_link(text) && !text.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:")));
    if link.zip(bus).is_some_and(|(link, bus)| portal_opens(bus, link)) {
        return true;
    }
    // A name starting with a dash would be read as an option of the opener.
    let named = match what.as_encoded_bytes().starts_with(b"-") {
        true => std::path::Path::new(".").join(what).into_os_string(),
        false => what.to_os_string(),
    };
    let mut run = std::process::Command::new(opener);
    run.arg(named).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    run.spawn().and_then(|mut started| started.wait()).is_ok_and(|ended| ended.success())
}

/// The portal, its object path, and the method that opens a link.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
const PORTAL: &str = "org.freedesktop.portal.Desktop";
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
const OPEN_URI: &str = "org.freedesktop.portal.OpenURI";

/// Asks the portal to open `link` for no window in particular and waits a while. Says whether
/// it took it: it answers at once with its request (it may then ask the person which program to
/// use, which is not waited for), or that it cannot; with no portal, the bus says so.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn portal_opens(mut bus: crate::atspi::Bus, link: &str) -> bool {
    use crate::atspi::{ANSWER, FAILED, Pack};
    let mut asks = Pack::default();
    asks.text("").text(link).open(8).close();
    let call = bus.call(PORTAL, PORTAL_PATH, OPEN_URI, "OpenURI", "ssa{sv}", &asks);
    let (since, mut answer) = (std::time::Instant::now(), None);
    while answer.is_none() && !bus.dead && since.elapsed() < std::time::Duration::from_secs(5) {
        bus.turn(&mut |_, message| {
            if message.answers == call && matches!(message.kind, ANSWER | FAILED) {
                answer = Some(message.kind == ANSWER);
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    answer == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn links_are_told_from_paths() {
        for link in ["https://example.org/a b", "mailto:someone@example.org", "file:///tmp/x", "ms-settings:display", "x-man-page:ls"] {
            assert!(is_link(link), "{link}");
        }
        for path in ["C:\\Users\\a.txt", "c:/a", "notes.txt", "/tmp/a:b", "./a:b", "", ":x", "1x:y"] {
            assert!(!is_link(path), "{path}");
        }
    }

    /// A stand-in for whatever the system would start: built in a folder whose name has a space
    /// and a non-ASCII letter, it writes what it was handed (one per line) into `opened.txt`
    /// beside itself, and shows nothing.
    #[cfg_attr(any(target_vendor = "apple", target_os = "android"), allow(dead_code))]
    fn stand_in(name: &str) -> PathBuf {
        let folder = std::env::current_exe().unwrap().parent().unwrap().join(format!("stand in \u{e9} {name}"));
        _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("main.rs");
        std::fs::write(
            &source,
            "#![windows_subsystem = \"windows\"]\nfn main() {\n    let handed: Vec<String> = std::env::args().skip(1).collect();\n    let me = std::env::current_exe().unwrap();\n    std::fs::write(me.with_file_name(\"opened.txt\"), handed.join(\"\\n\")).unwrap();\n}\n",
        )
        .unwrap();
        let program = folder.join(if cfg!(windows) { "stand-in.exe" } else { "stand-in" });
        let built = std::process::Command::new("rustc").arg("-O").arg("-o").arg(&program).arg(&source).status().expect("rustc builds the stand-in");
        assert!(built.success(), "the stand-in was built");
        program
    }

    /// What the stand-in at `program` was handed, once it ran; nothing if it did not run within
    /// a few seconds.
    #[cfg_attr(any(target_vendor = "apple", target_os = "android"), allow(dead_code))]
    fn handed(program: &Path) -> Option<String> {
        let note = program.with_file_name("opened.txt");
        let since = std::time::Instant::now();
        while since.elapsed() < std::time::Duration::from_secs(10) {
            if let Ok(text) = std::fs::read_to_string(&note) {
                return Some(text);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }

    /// The shell opens a program with itself. (A document type the stand-in opened would need
    /// registering in the person's settings, which tests leave alone.)
    #[test]
    #[cfg(windows)]
    fn the_shell_opens_what_it_is_handed() {
        let program = stand_in("shell");
        assert!(opened(program.as_os_str()), "the shell took it");
        assert_eq!(handed(&program).as_deref(), Some(""), "and the program it stands for ran");
        _ = std::fs::remove_dir_all(program.parent().unwrap());
    }

    /// A portal played by a test thread on a private bus: it notes each link it is asked to
    /// open, and takes it or (`refuses`) says it cannot.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn portal(address: &str, refuses: bool) -> (std::sync::Arc<std::sync::Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
        use crate::atspi::{BUS, BUS_PATH, Bus, CALL, Pack};
        let mut bus = Bus::open(address).expect("the test's own bus");
        bus.call(BUS, BUS_PATH, BUS, "RequestName", "su", Pack::default().text(PORTAL).u32(0));
        let (mut named, since) = (false, std::time::Instant::now());
        while !named && since.elapsed() < std::time::Duration::from_secs(5) {
            bus.turn(&mut |_, message| named |= message.member == "NameAcquired" && message.body.clone().text() == Some(PORTAL));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(named, "the bus gave the stand-in the portal's name");
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let its = asked.clone();
        let thread = std::thread::spawn(move || {
            let since = std::time::Instant::now();
            while its.lock().unwrap().is_empty() && since.elapsed() < std::time::Duration::from_secs(10) {
                bus.turn(&mut |bus, call| {
                    if call.kind != CALL || (call.interface, call.member) != (OPEN_URI, "OpenURI") {
                        return;
                    }
                    let mut said = call.body;
                    let (window, link) = (said.text().unwrap_or("?").to_string(), said.text().unwrap_or("?").to_string());
                    its.lock().unwrap().push(format!("{window}|{link}"));
                    let answer = if refuses { Err(("org.freedesktop.portal.Error.NotAllowed", "Not allowed")) } else { Ok("o") };
                    bus.answer(call, answer, Pack::default().text("/org/freedesktop/portal/desktop/request/1_1/t"));
                });
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // The answer goes out before the bus is let go.
            bus.turn(&mut |_, _| {});
        });
        (asked, thread)
    }

    /// A link goes to the portal only; a link the portal refuses, and a file, go to `xdg-open`
    /// (a stand-in here), each as given.
    #[test]
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn a_link_goes_to_the_portal_and_a_file_to_xdg_open() {
        let Some(own) = crate::portal::tests::OwnBus::start() else { return eprintln!("no dbus-daemon here: the portal is not tried") };
        let opener = stand_in("xdg-open");
        let note = opener.with_file_name("opened.txt");

        let (asked, thread) = portal(&own.address, false);
        assert!(open_by(OsStr::new("https://example.org/a?b=c d"), crate::atspi::Bus::open(&own.address), opener.as_os_str()));
        thread.join().unwrap();
        assert_eq!(*asked.lock().unwrap(), ["|https://example.org/a?b=c d"], "the portal was asked, for no window");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!note.exists(), "and xdg-open was not");

        let (asked, thread) = portal(&own.address, true);
        assert!(open_by(OsStr::new("mailto:someone@example.org"), crate::atspi::Bus::open(&own.address), opener.as_os_str()));
        thread.join().unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(handed(&opener).as_deref(), Some("mailto:someone@example.org"), "a link the portal will not take goes to xdg-open");

        _ = std::fs::remove_file(&note);
        let file = opener.with_file_name("a file \u{e9}.txt");
        assert!(open_by(file.as_os_str(), crate::atspi::Bus::open(&own.address), opener.as_os_str()));
        assert_eq!(handed(&opener).as_deref(), file.to_str(), "a file goes to xdg-open by its path");

        _ = std::fs::remove_file(&note);
        assert!(open_by(OsStr::new("https://example.org"), None, opener.as_os_str()));
        assert_eq!(handed(&opener).as_deref(), Some("https://example.org"), "with no bus, a link goes to xdg-open too");

        _ = std::fs::remove_file(&note);
        assert!(open_by(OsStr::new("-v.txt"), None, opener.as_os_str()));
        assert_eq!(handed(&opener).as_deref(), Some("./-v.txt"), "a name that starts with a dash is no option");
        _ = std::fs::remove_dir_all(opener.parent().unwrap());
    }
}
