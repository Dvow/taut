//! Tests of hostile or out-of-range input to `hinting.rs`, `single_instance.rs` and
//! `freedesktop.rs`. Each section belongs in the `tests` module of the file it names.

use crate::path::Path;

/// Set for a test run by itself in a process of its own.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
const ALONE: &str = "TAUT_TEST_ALONE";

/// Runs one ignored test in a process of its own (this program again) and ends it after
/// `seconds`. Gives back what it printed, or why it failed.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn alone(test: &str, seconds: u64, env: &[(&str, &str)]) -> Result<String, String> {
    let mut started = std::process::Command::new(std::env::current_exe().unwrap());
    started.args(["--exact", test, "--ignored", "--nocapture", "--test-threads=1"]).env(ALONE, "1");
    for (name, value) in env {
        started.env(name, value);
    }
    let mut child = started.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let since = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if since.elapsed() > std::time::Duration::from_secs(seconds) {
            _ = child.kill();
            _ = child.wait();
            return Err(format!("{test} was still running after {seconds} seconds"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ran = child.wait_with_output().unwrap();
    let output = format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
    match ran.status.success() && output.contains("1 passed") {
        true => Ok(output),
        false => Err(format!("{test} ended with {:?}:\n{output}", ran.status)),
    }
}

// ---- hinting.rs ----

#[test]
fn a_letter_of_very_many_points_takes_little_time_to_set_on_whole_pixels() {
    // A glyph may have 65,535 points: here 8,000 thin bars, each a little higher than the last.
    let Some(font) = crate::pdf::tests::latin() else { return eprintln!("no font file here: not tested") };
    let mut path = Path::new();
    for n in 0..8_000 {
        let (x, y) = ((n % 7) as f32 * 40.0, -(n as f32) * 0.1);
        path.move_to(x, y).line_to(x + 30.0, y).line_to(x + 30.0, y - 0.05).line_to(x, y - 0.05).close();
    }
    let since = std::time::Instant::now();
    _ = crate::hinting::set(&path, &font, 12.0 / font.units());
    let took = since.elapsed();
    assert!(took < std::time::Duration::from_secs(1), "set in {took:?}");
}

// ---- single_instance.rs, on Linux ----

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod single_instance {
    use super::{ALONE, alone};

    /// The runtime folder the test's programs share.
    fn folder() -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("taut-test-copies-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn two_programs_whose_names_differ_after_their_last_dot_are_each_a_first_copy() {
        // "org.example.Notes" runs; "org.example.Mail" starts: another program, its own first copy.
        let folder = folder();
        let place = folder.to_string_lossy().into_owned();
        let env = [("XDG_RUNTIME_DIR", place.as_str())];
        let notes = std::thread::spawn({
            let place = place.clone();
            move || alone("hardening::single_instance::notes_runs_for_a_while", 30, &[("XDG_RUNTIME_DIR", place.as_str())])
        });
        // Notes is the first copy once its lock is there.
        let since = std::time::Instant::now();
        while std::fs::read_dir(&folder).map_or(0, |listed| listed.count()) < 2 && since.elapsed().as_secs() < 20 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mail = alone("hardening::single_instance::mail_starts", 30, &env);
        let notes = notes.join().unwrap();
        _ = std::fs::remove_dir_all(&folder);
        mail.unwrap();
        let notes = notes.unwrap();
        assert!(!notes.contains("heard a start"), "Notes heard Mail's start:\n{notes}");
    }

    #[test]
    #[ignore = "a part of the test before it"]
    fn notes_runs_for_a_while() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(crate::single_instance::single_instance("org.example.Notes"));
        let since = std::time::Instant::now();
        while since.elapsed() < std::time::Duration::from_secs(4) {
            if crate::single_instance::forward().is_some() {
                println!("heard a start");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "a part of the test before it"]
    fn mail_starts() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(crate::single_instance::single_instance("org.example.Mail"), "Mail is a program of its own: its first copy");
    }
}

// ---- freedesktop.rs: what comes over the session bus ----

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod bus {
    use super::{ALONE, alone};
    use crate::atspi::tests::{Peer, packed};
    use crate::atspi::{Bus, CALL, Message, Pack};
    use std::os::unix::net::UnixStream;

    /// The program's end of a bus and the test's end (the bus and every program on it), let on.
    fn side() -> (Bus, Peer) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut bus = Bus::over(ours).unwrap();
        let mut peer = Peer::new(theirs);
        peer.let_on(&mut || bus.turn(&mut |_, _| {}));
        (bus, peer)
    }

    /// Another program calls this one, which takes turns until it has answered. Gives back
    /// the whole messages it sent.
    fn called(bus: &mut Bus, peer: &mut Peer, [path, interface, member, kinds]: [&str; 4], body: &Pack) -> Vec<Vec<u8>> {
        let fields = [(1, "o", path), (2, "s", interface), (3, "s", member), (7, "s", ":1.66"), (8, "g", kinds)];
        let mut message = packed(CALL, 9, &fields, body).into_iter();
        let mut output = Vec::new();
        for _ in 0..10_000 {
            // The socket takes so much at a time.
            let piece: Vec<u8> = message.by_ref().take(16_384).collect();
            if !piece.is_empty() {
                peer.send(&piece);
            }
            bus.turn(&mut |bus, message| _ = crate::freedesktop::heard(bus, message));
            output.extend(peer.messages());
            if output.iter().any(|bytes| Message::read(bytes).is_some_and(|message| message.answers == 9)) {
                break;
            }
        }
        output
    }

    #[test]
    fn a_panel_that_asks_for_rows_of_the_menu_with_a_list_cut_short_hangs_nothing() {
        // In a process of its own: when it fails, it never ends.
        alone("hardening::bus::rows_asked_for_by_a_list_cut_short", 20, &[]).unwrap();
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own"]
    fn rows_asked_for_by_a_list_cut_short() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        let (mut bus, mut peer) = side();
        // A list of one byte where row numbers (four bytes each) are read.
        let mut list = Pack::default();
        list.open(1).byte(7).close();
        let output = called(&mut bus, &mut peer, ["/MenuBar", "com.canonical.dbusmenu", "GetGroupProperties", "ay"], &list);
        assert!(output.iter().any(|bytes| Message::read(bytes).is_some_and(|message| message.answers == 9)), "the call is answered");
    }

    #[test]
    fn a_panel_that_asks_for_a_row_many_times_over_is_answered_with_it_once() {
        let (mut bus, mut peer) = side();
        // 20,000 copies of one row's number get that row once: an answer larger than the bus
        // takes would end the connection.
        let mut list = Pack::default();
        list.open(4);
        for _ in 0..20_000 {
            list.u32(0);
        }
        list.close();
        let output = called(&mut bus, &mut peer, ["/MenuBar", "com.canonical.dbusmenu", "GetGroupProperties", "ai"], &list);
        let answer = output.iter().find(|bytes| Message::read(bytes).is_some_and(|message| message.answers == 9)).expect("an answer");
        assert!(answer.len() < 4096, "an answer of {} bytes", answer.len());
    }

    #[test]
    fn a_token_for_coming_to_the_front_that_no_desktop_gives_is_not_kept() {
        // A token longer than a Wayland message (4096 bytes) would get the program disconnected.
        let (mut bus, mut peer) = side();
        let mut long = Pack::default();
        long.text(&"t".repeat(100_000));
        called(&mut bus, &mut peer, ["/StatusNotifierItem", "org.kde.StatusNotifierItem", "ProvideXdgActivationToken", "s"], &long);
        let asked = crate::freedesktop::ran(Some(&mut bus), true, "");
        assert!(asked.token.len() < 1024, "a token of {} bytes", asked.token.len());
    }

    /// Whether a text is a D-Bus object path: `/`, or `/`-separated names of `[A-Za-z0-9_]`.
    fn a_path(text: &str) -> bool {
        text == "/" || text.strip_prefix('/').is_some_and(|rest| rest.split('/').all(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')))
    }

    #[test]
    fn a_session_another_program_names_is_never_sent_on_as_a_path_that_is_none() {
        // Any program on the bus may fake the portal's answer to a request for a shortcuts
        // session. A session handle that is no object path is never sent back as one: the bus
        // would end the connection.
        let (mut bus, mut peer) = side();
        let mut trial = crate::testing::Trial::new((), |_, ui| _ = ui.hotkey("Ctrl+Alt+K"));
        trial.run();
        crate::freedesktop::ran(Some(&mut bus), true, "wayland:first");
        let asked = peer.messages();
        assert!(asked.iter().any(|bytes| Message::read(bytes).is_some_and(|message| message.member == "CreateSession")), "a session is asked for");
        let mut answer = Pack::default();
        answer.u32(0).open(8).pad(8).text("session_handle").kinds("s").text("not a path/..").close();
        let fields = [(1, "o", "/org/freedesktop/portal/desktop/request/1_42/taut_keys1"), (2, "s", "org.freedesktop.portal.Request"), (3, "s", "Response"), (7, "s", ":1.66"), (8, "g", "ua{sv}")];
        peer.send(&packed(crate::atspi::NEWS, 9, &fields, &answer));
        bus.turn(&mut |bus, message| _ = crate::freedesktop::heard(bus, message));
        for bytes in peer.messages() {
            let message = Message::read(&bytes).unwrap();
            if message.member == "BindShortcuts" {
                let session = message.body.clone().text().unwrap_or_default().to_string();
                assert!(a_path(&session), "BindShortcuts names {session:?} as the session's path");
            }
        }
    }
}
