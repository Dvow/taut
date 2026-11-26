//! Tests on an Android emulator, driven from the computer through `adb`: what the screen
//! shows, read with `uiautomator dump` (the window's nodes, as the library tells them) and
//! `screencap`, and input from `input`. The emulator must be up; each test is skipped where no
//! app to test is named.
use crate::android_text::attribute;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// `adb`, from the SDK or the path: what it printed.
pub(crate) fn adb(args: &[&str]) -> Vec<u8> {
    adb_said(args).0
}

/// How long the emulator may stay out of reach before a test gives up on it.
const OUT_OF_REACH: Duration = Duration::from_secs(120);

/// `adb`: what it printed, and its errors.
fn adb_said(args: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let done = adb_run(args);
    (done.stdout, done.stderr)
}

/// `adb`, run until it reaches the emulator. The emulator's connection drops now and then (on
/// a busy runner, after a large copy) and comes back: a command that never reached it is sent
/// again once it is back.
fn adb_run(args: &[&str]) -> Output {
    let tools = std::env::var_os("ANDROID_HOME").map(|home| PathBuf::from(home).join("platform-tools").join("adb"));
    let program = tools.filter(|path| path.exists() || path.with_extension("exe").exists()).unwrap_or_else(|| "adb".into());
    let since = Instant::now();
    loop {
        let done = Command::new(&program).args(args).output().expect("adb runs");
        if !never_reached(&done.stderr) {
            return done;
        }
        assert!(since.elapsed() < OUT_OF_REACH, "the emulator stayed out of reach: adb {args:?}: {}", String::from_utf8_lossy(&done.stderr));
        settle(1);
    }
}

/// Copies the emulator's file `path` into `folder`, again until it comes whole: a copy that a
/// dropped connection cuts short fails.
pub(crate) fn pull(path: &str, folder: &Path) {
    let since = Instant::now();
    loop {
        let done = adb_run(&["pull", path, &folder.to_string_lossy()]);
        if done.status.success() {
            return;
        }
        assert!(since.elapsed() < OUT_OF_REACH, "{path} was not copied: {}", String::from_utf8_lossy(&done.stderr));
        settle(1);
    }
}

/// The emulator's file `path`, whole, by `pull`. (What `exec-out` prints can end early and
/// still exit 0: one copy of a 32 MB font in nine, on the runner.)
pub(crate) fn read(path: &str) -> Vec<u8> {
    let folder = std::env::temp_dir().join(format!("taut-adb-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    pull(path, &folder);
    let copy = folder.join(path.rsplit('/').next().unwrap_or(path));
    let bytes = std::fs::read(&copy).unwrap_or_default();
    let _ = std::fs::remove_file(copy);
    bytes
}

/// Whether `adb`'s errors say it never reached the emulator, so the command did nothing there.
fn never_reached(errors: &[u8]) -> bool {
    let errors = String::from_utf8_lossy(errors);
    // (After "error: " from most commands, "adb: " or "adb.exe: " from installs.)
    let said = errors.lines().next().and_then(|line| line.split_once(": ")).map_or("", |(_, said)| said.trim_end());
    said == "device offline" || said == "no devices/emulators found" || (said.starts_with("device '") && said.ends_with("' not found"))
}

/// Installs an APK: what went wrong, if it was not installed.
pub(crate) fn install(apk: &Path) -> Result<(), String> {
    let (said, errors) = adb_said(&["install", "-r", &apk.to_string_lossy()]);
    let said = String::from_utf8_lossy(&said).into_owned() + &String::from_utf8_lossy(&errors);
    if said.contains("Success") { Ok(()) } else { Err(said) }
}

pub(crate) fn shell(command: &str) -> String {
    String::from_utf8_lossy(&adb(&["shell", command])).into_owned()
}

pub(crate) fn settle(seconds: u64) {
    std::thread::sleep(Duration::from_secs(seconds));
}

/// One node of a `uiautomator dump`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    pub class: String,
    pub text: String,
    /// Its content description (what system apps name their icons by).
    pub described: String,
    pub checked: bool,
    pub focused: bool,
    pub bounds: [i32; 4],
}

impl Node {
    pub(crate) fn middle(&self) -> (i32, i32) {
        ((self.bounds[0] + self.bounds[2]) / 2, (self.bounds[1] + self.bounds[3]) / 2)
    }
}

/// The focused window's nodes, as `uiautomator` reads them; the dump is kept in `out`. A dump
/// that failed is asked again: on a busy emulator `uiautomator` itself can fail to connect to
/// the system and stop with an exception.
pub(crate) fn dump(out: &Path, name: &str) -> Vec<Node> {
    let since = Instant::now();
    while !shell("rm -f /sdcard/taut-ui.xml; uiautomator dump /sdcard/taut-ui.xml 2>&1").contains("dumped to") {
        assert!(since.elapsed() < OUT_OF_REACH, "uiautomator dumped nothing");
        settle(1);
    }
    let xml = String::from_utf8_lossy(&read("/sdcard/taut-ui.xml")).into_owned();
    let _ = std::fs::write(out.join(format!("{name}.xml")), &xml);
    xml.split("<node ").skip(1).map(node_of).collect()
}

/// Whether an app of one of these packages crashed since the log was cleared: the log's crash
/// buffer names its process. (`uiautomator` stopping on its own is no app's crash.)
pub(crate) fn crashed(packages: &[&str]) -> bool {
    let crashes = String::from_utf8_lossy(&adb(&["logcat", "-d", "-b", "crash"])).into_owned();
    crashes.lines().any(|line| packages.iter().any(|package| line.contains(package)))
}

fn node_of(tag: &str) -> Node {
    let value = |name: &str| unescaped(attribute(tag, name).unwrap_or_default());
    let corners: Vec<i32> = value("bounds").split(|c: char| !c.is_ascii_digit() && c != '-').filter_map(|part| part.parse().ok()).collect();
    let (checked, focused) = (value("checked") == "true", value("focused") == "true");
    Node { class: value("class"), text: value("text"), described: value("content-desc"), checked, focused, bounds: corners.try_into().unwrap_or_default() }
}

/// An attribute's text with XML's escapes undone: the five named ones, and letters by number
/// (`&#128512;`, `&#x1F600;`, as `uiautomator` writes an emoji).
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        match rest.split_once(';').and_then(|(name, after)| Some((escaped(name)?, after))) {
            Some((letter, after)) => {
                out.push(letter);
                rest = after;
            }
            None => out.push('&'),
        }
    }
    out + rest
}

/// The letter an XML escape stands for, by its name between `&` and `;`.
fn escaped(name: &str) -> Option<char> {
    let number = match name {
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "amp" => return Some('&'),
        _ => match name.strip_prefix("#x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => name.strip_prefix('#')?.parse().ok()?,
        },
    };
    char::from_u32(number)
}

pub(crate) fn find<'a>(nodes: &'a [Node], class: &str, text: &str) -> Option<&'a Node> {
    nodes.iter().find(|node| node.class == class && node.text.starts_with(text))
}

/// The node whose text or description is `text`, else the first that starts with it.
pub(crate) fn named<'a>(nodes: &'a [Node], text: &str) -> Option<&'a Node> {
    let exactly = nodes.iter().find(|node| node.text == text || node.described == text);
    exactly.or_else(|| nodes.iter().find(|node| node.text.starts_with(text) || node.described.starts_with(text)))
}

pub(crate) fn tap(node: &Node) {
    let (x, y) = node.middle();
    shell(&format!("input tap {x} {y}"));
    settle(2);
}

/// The screen, as a picture: width and pixels.
pub(crate) fn screen(out: &Path, name: &str) -> (usize, Vec<u32>) {
    shell("screencap -p /sdcard/taut-screen.png");
    let png = read("/sdcard/taut-screen.png");
    let _ = std::fs::write(out.join(format!("{name}.png")), &png);
    crate::png::decode(&png).unwrap_or_default()
}

/// Dumps the focused window until a node's text or description starts with `text`, for 30 s
/// at most: that node.
pub(crate) fn wait_for(out: &Path, text: &str) -> Option<Node> {
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if let Some(found) = named(&dump(out, "waited"), text) {
            return Some(found.clone());
        }
        settle(1);
    }
    None
}

/// Taps the node named `text` once it shows: whether it did.
pub(crate) fn tap_named(out: &Path, text: &str) -> bool {
    wait_for(out, text).map(|node| tap(&node)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_dex::opcodes::*;
    use crate::android_dex::{BARE, Class, Method, Op, Ref, dex};

    /// A keyboard for tests: an input method with no keys of its own, which composes as keys
    /// say: C composes "hel", D "hello", E commits what is composed, F drops it. G commits two
    /// emoji and an "e" with a combining acute; H deletes one UTF-16 unit before the cursor, I
    /// three, J one code point. (The emulator's own keyboard commits every letter at once,
    /// everywhere: it composes nothing.)
    const KEYBOARD: Class = Class {
        name: "taut/Keyboard",
        parent: "android/inputmethodservice/InputMethodService",
        methods: &[Method {
            name: "onKeyDown",
            descriptor: "(ILandroid/view/KeyEvent;)Z",
            // v0 the connection, v1 the key compared with, v2 1, v3 a text; this v4, the key
            // v5, the event v6.
            registers: 7,
            code: &[
                Op::Invoke(VIRTUAL, Ref::Method("android/inputmethodservice/InputMethodService", "getCurrentInputConnection", "()Landroid/view/inputmethod/InputConnection;"), &[4]),
                Op::Units(&[move_result_object(0), const4(2, 1)]),
                // C (31): compose "hel". (Each test jumps past its key's code if the key differs.)
                Op::Units(&[op(0x13, 1), 31, op2(0x33, 5, 1), 8]),
                Op::With(op(CONST_STRING, 3), Ref::Text("hel")),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "setComposingText", "(Ljava/lang/CharSequence;I)Z"), &[0, 3, 2]),
                Op::Units(&[return_int(2)]),
                // D (32): compose "hello".
                Op::Units(&[op(0x13, 1), 32, op2(0x33, 5, 1), 8]),
                Op::With(op(CONST_STRING, 3), Ref::Text("hello")),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "setComposingText", "(Ljava/lang/CharSequence;I)Z"), &[0, 3, 2]),
                Op::Units(&[return_int(2)]),
                // E (33): commit.
                Op::Units(&[op(0x13, 1), 33, op2(0x33, 5, 1), 6]),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "finishComposingText", "()Z"), &[0]),
                Op::Units(&[return_int(2)]),
                // F (34): drop.
                Op::Units(&[op(0x13, 1), 34, op2(0x33, 5, 1), 8]),
                Op::With(op(CONST_STRING, 3), Ref::Text("")),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "setComposingText", "(Ljava/lang/CharSequence;I)Z"), &[0, 3, 2]),
                Op::Units(&[return_int(2)]),
                // G (35): commit two emoji and an accented "e".
                Op::Units(&[op(0x13, 1), 35, op2(0x33, 5, 1), 8]),
                Op::With(op(CONST_STRING, 3), Ref::Text("😀😀e\u{301}")),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "commitText", "(Ljava/lang/CharSequence;I)Z"), &[0, 3, 2]),
                Op::Units(&[return_int(2)]),
                // H (36), I (37): delete 1 and 3 UTF-16 units before the cursor (v1), none after (v3).
                Op::Units(&[op(0x13, 1), 36, op2(0x33, 5, 1), 8, const4(1, 1), const4(3, 0)]),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "deleteSurroundingText", "(II)Z"), &[0, 1, 3]),
                Op::Units(&[return_int(2)]),
                Op::Units(&[op(0x13, 1), 37, op2(0x33, 5, 1), 8, const4(1, 3), const4(3, 0)]),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "deleteSurroundingText", "(II)Z"), &[0, 1, 3]),
                Op::Units(&[return_int(2)]),
                // J (38): delete one code point before the cursor.
                Op::Units(&[op(0x13, 1), 38, op2(0x33, 5, 1), 8, const4(1, 1), const4(3, 0)]),
                Op::Invoke(INTERFACE, Ref::Method("android/view/inputmethod/InputConnection", "deleteSurroundingTextInCodePoints", "(II)Z"), &[0, 1, 3]),
                Op::Units(&[return_int(2)]),
                // Any other key: as the input method service does.
                Op::Invoke(SUPER, Ref::Method("android/inputmethodservice/InputMethodService", "onKeyDown", "(ILandroid/view/KeyEvent;)Z"), &[4, 5, 6]),
                Op::Units(&[move_result(0), return_int(0)]),
            ],
        }],
        ..BARE
    };

    /// The test keyboard as an APK, made in `out` with the SDK's tools (as `examples/apk.rs`
    /// makes apps), signed with the key `keystore` holds: its path.
    fn keyboard_apk(out: &Path, keystore: &Path) -> PathBuf {
        let home = PathBuf::from(std::env::var_os("ANDROID_HOME").expect("ANDROID_HOME"));
        let newest = |folder: PathBuf| std::fs::read_dir(folder).unwrap().filter_map(Result::ok).map(|entry| entry.path()).max().unwrap();
        let tools = newest(home.join("build-tools"));
        let jar = newest(home.join("platforms")).join("android.jar");
        let tool = |name: &str| ["", ".exe", ".bat"].iter().map(|end| tools.join(format!("{name}{end}"))).find(|path| path.is_file()).unwrap();
        let run = |program: PathBuf, args: &[&str]| assert!(Command::new(&program).args(args).current_dir(out).status().unwrap().success(), "{}", program.display());
        std::fs::create_dir_all(out.join("res/xml")).unwrap();
        std::fs::write(out.join("res/xml/method.xml"), "<input-method xmlns:android=\"http://schemas.android.com/apk/res/android\" />").unwrap();
        std::fs::write(
            out.join("AndroidManifest.xml"),
            r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="taut.test.keyboard">
    <application android:label="test keyboard" android:hasCode="true">
        <service android:name="taut.Keyboard" android:permission="android.permission.BIND_INPUT_METHOD" android:exported="true">
            <intent-filter><action android:name="android.view.InputMethod" /></intent-filter>
            <meta-data android:name="android.view.im" android:resource="@xml/method" />
        </service>
    </application>
</manifest>"#,
        )
        .unwrap();
        std::fs::write(out.join("classes.dex"), dex(&[KEYBOARD])).unwrap();
        run(tool("aapt2"), &["compile", "--dir", "res", "-o", "res.zip"]);
        run(tool("aapt2"), &["link", "-o", "unaligned.apk", "--manifest", "AndroidManifest.xml", "-I", &jar.to_string_lossy(), "res.zip", "--min-sdk-version", "30", "--target-sdk-version", "35"]);
        run(tool("aapt"), &["add", "-f", "unaligned.apk", "classes.dex"]);
        run(tool("zipalign"), &["-f", "-p", "4", "unaligned.apk", "aligned.apk"]);
        let keys = keystore.to_string_lossy().into_owned();
        run(tool("apksigner"), &["sign", "--ks", &keys, "--ks-pass", "pass:android", "--key-pass", "pass:android", "--out", "keyboard.apk", "aligned.apk"]);
        out.join("keyboard.apk")
    }

    /// The text of the box the app checks for spelling.
    fn box_text(out: &Path) -> String {
        dump(out, "box").into_iter().find(|node| node.class == "android.widget.EditText").map(|node| node.text).unwrap_or_default()
    }

    /// How many pixels of a part of the screen are red: the spell checker's waves.
    fn red_in(shown: &(usize, Vec<u32>), [left, top, right, bottom]: [i32; 4]) -> usize {
        let (width, pixels) = shown;
        let red = |pixel: &u32| pixel >> 16 & 0xff > 180 && pixel >> 8 & 0xff < 110 && pixel & 0xff < 110;
        (top.max(0) as usize..bottom.max(0) as usize).flat_map(|y| (left.max(0) as usize..right.max(0) as usize).map(move |x| y * width + x)).filter(|at| pixels.get(*at).is_some_and(red)).count()
    }

    /// The pixels of a part of the screen.
    fn crop(shown: &(usize, Vec<u32>), [left, top, right, bottom]: [i32; 4]) -> Vec<u32> {
        (top as usize..bottom as usize).flat_map(|y| (left as usize..right as usize).map(move |x| shown.1.get(y * shown.0 + x).copied().unwrap_or(0))).collect()
    }

    /// The formats line follows the app's language as the system sets it (a configuration
    /// change, the app running on), written as Android's `java.text` and ICU write each place:
    /// its digits, separators, group sizes, date order, 12 or 24 hours and AM/PM's place.
    fn formats(out: &Path, wrong: &mut Vec<String>) {
        let app = "taut.example.system_parts";
        let places = [
            ("ar-EG", "Formats: ١٬٢٣٤٬٥٦٧٫٥ · ٧/٣/٢٠٢٦ · ٢:٠٥ م"),
            ("hi-IN", "Formats: 12,34,567.5 · 7/3/2026 · 2:05 pm"),
            ("ko-KR", "Formats: 1,234,567.5 · 2026.3.7 · 오후 2:05"),
            ("de-DE", "Formats: 1.234.567,5 · 7.3.2026 · 14:05"),
        ];
        for (place, wanted) in places {
            shell(&format!("cmd locale set-app-locales {app} --locales {place}"));
            if wait_for(out, wanted).is_none() {
                let shown = named(&dump(out, "formats"), "Formats:").map_or_else(String::new, |node| node.text.clone());
                wrong.push(format!("{place}: {shown:?}, not {wanted:?}"));
            }
        }
        shell(&format!("cmd locale set-app-locales {app} --locales en-US"));
    }

    /// Faces named by family are drawn in other fonts than the system's.
    fn named_faces(out: &Path, wrong: &mut Vec<String>) {
        let nodes = dump(out, "faces");
        let shown = screen(out, "faces");
        let crops: Vec<Vec<u32>> =
            ["Sans:", "Serif:", "Mono:"].iter().filter_map(|name| named(&nodes, name)).map(|node| crop(&shown, [node.bounds[0], node.bounds[1], node.bounds[0] + 300, node.bounds[1] + 40])).collect();
        if crops.len() != 3 || crops[0] == crops[1] || crops[0] == crops[2] || crops[1] == crops[2] {
            wrong.push("the named faces were not drawn each in its own font".into());
        }
    }

    /// Misspelled words typed into the box are marked, and its menu offers the right word.
    fn spelling(out: &Path, wrong: &mut Vec<String>) {
        let Some(edit) = find(&dump(out, "spelling"), "android.widget.EditText", "").cloned() else { return wrong.push("no box to spell in".into()) };
        tap(&edit);
        shell("input keyevent KEYCODE_MOVE_END");
        shell("input text '%sqwzxv%srecieve%s'");
        let until = Instant::now() + Duration::from_secs(60);
        while red_in(&screen(out, "spelled"), edit.bounds) < 20 && Instant::now() < until {
            settle(2);
        }
        if red_in(&screen(out, "spelled"), edit.bounds) < 20 {
            wrong.push("no misspelled word was marked".into());
        }
        // The cursor into "recieve", and the menu key: its first rows are suggestions.
        (0..3).for_each(|_| _ = shell("input keyevent KEYCODE_DPAD_LEFT"));
        shell("input keyevent KEYCODE_MENU");
        if wait_for(out, "receive").is_none() {
            wrong.push("the menu offered no \"receive\" for \"recieve\"".into());
        }
        shell("input keyevent KEYCODE_ESCAPE");
    }

    /// The test keyboard composes into the box: shown, not typed, until committed; dropped,
    /// nothing is typed.
    fn composing(out: &Path, keystore: &Path, wrong: &mut Vec<String>) {
        if let Err(said) = install(&keyboard_apk(&out.join("keyboard"), keystore)) {
            return wrong.push(format!("the test keyboard was not installed: {said}"));
        }
        // (The system lists a new input method a moment after it is installed.)
        let until = Instant::now() + Duration::from_secs(30);
        while !shell("ime list -a -s").contains("taut.test.keyboard") && Instant::now() < until {
            settle(1);
        }
        let chosen = shell("ime enable taut.test.keyboard/taut.Keyboard; ime set taut.test.keyboard/taut.Keyboard");
        if !chosen.contains("selected") {
            return wrong.push(format!("the test keyboard was not chosen: {chosen}"));
        }
        let Some(edit) = find(&dump(out, "composing"), "android.widget.EditText", "").cloned() else { return wrong.push("no box to compose in".into()) };
        tap(&edit);
        shell("input keyevent KEYCODE_MOVE_END");
        let before = box_text(out);
        shell("input keyevent KEYCODE_C");
        settle(1);
        shell("input keyevent KEYCODE_D");
        settle(1);
        let _ = screen(out, "composing");
        if box_text(out) != before {
            wrong.push(format!("composing typed into the box: {:?}", box_text(out)));
        }
        shell("input keyevent KEYCODE_E");
        settle(1);
        let committed = box_text(out);
        if committed != format!("{before}hello") {
            wrong.push(format!("committing left {committed:?}, not {before:?} and hello"));
        }
        shell("input keyevent KEYCODE_C");
        settle(1);
        shell("input keyevent KEYCODE_F");
        settle(1);
        if box_text(out) != committed {
            wrong.push(format!("dropping what was composed left {:?}", box_text(out)));
        }
        deleting(out, wrong);
        shell("ime reset");
    }

    /// The test keyboard deletes before the cursor: an accent without its letter, whole emoji
    /// counted in UTF-16 units, and in code points.
    fn deleting(out: &Path, wrong: &mut Vec<String>) {
        let before = box_text(out);
        for (key, wanted) in [("G", "😀😀e\u{301}"), ("H", "😀😀e"), ("I", "😀"), ("J", "")] {
            shell(&format!("input keyevent KEYCODE_{key}"));
            settle(1);
            let shown = box_text(out);
            if shown != format!("{before}{wanted}") {
                wrong.push(format!("after {key}: {shown:?}, not {before:?} and {wanted:?}"));
            }
        }
    }

    /// The document picker, from the folder Downloads: taps the card of the file `name`.
    fn pick_card(out: &Path, name: &str, hold: bool) {
        let Some(label) = wait_for(out, name) else { return };
        let (x, y) = (label.middle().0, label.bounds[1] - 150);
        shell(&if hold { format!("input swipe {x} {y} {x} {y} 1200") } else { format!("input tap {x} {y}") });
        settle(1);
    }

    /// Goes to Downloads in the document picker.
    fn to_downloads(out: &Path) {
        tap_named(out, "Show roots");
        let nodes = dump(out, "roots");
        // (The list of roots names Downloads, as does the chip above the files.)
        if let Some(downloads) = nodes.iter().filter(|node| node.text == "Downloads").max_by_key(|node| node.bounds[2] - node.bounds[0]) {
            tap(downloads);
        }
    }

    /// Opens a file, then two, then saves one, through the system's dialog; the saved document
    /// holds what the app wrote to its copy.
    fn file_dialog(out: &Path, wrong: &mut Vec<String>) {
        let mut check = |said: &str| {
            if wait_for(out, said).is_none() {
                wrong.push(format!("not told \"{said}\""));
            }
        };
        tap_named(out, "Open");
        to_downloads(out);
        pick_card(out, "a.txt", false);
        check("Files: opened a.txt (First line of a)");
        tap_named(out, "Open several");
        pick_card(out, "a.txt", true);
        pick_card(out, "b.txt", false);
        tap_named(out, "Select");
        check("Files: opened a.txt (First line of a), b.txt (First line of b)");
        tap_named(out, "Save");
        tap_named(out, "SAVE");
        check("Files: saved notes.txt");
        if shell("cat /sdcard/Download/notes.txt").trim() != "Saved by the system parts" {
            wrong.push("the saved document does not hold what the app wrote".into());
        }
    }

    /// A picture and a file go on the clipboard and come back as they went.
    fn clipboard(out: &Path, wrong: &mut Vec<String>) {
        for (copy, paste, said) in [("Copy picture", "Paste picture", "Clipboard: picture 2x2, as copied"), ("Copy file", "Paste files", "Clipboard: files copied.txt (Copied by the system parts)")] {
            tap_named(out, copy);
            tap_named(out, paste);
            if wait_for(out, said).is_none() {
                wrong.push(format!("not told \"{said}\""));
            }
        }
    }

    /// The first notification asks the person's permission; allowed, it shows, and a click on
    /// it is heard.
    fn notification(out: &Path, wrong: &mut Vec<String>) {
        tap_named(out, "Notify");
        if !tap_named(out, "Allow") {
            wrong.push("the permission to notify was not asked".into());
        }
        settle(2);
        shell("cmd statusbar expand-notifications");
        if !tap_named(out, "From the system parts") {
            return wrong.push("the notification did not show".into());
        }
        if wait_for(out, "Notification: clicked").is_none() {
            wrong.push("the click on the notification was not heard".into());
        }
    }

    /// The print dialog previews the pages; closed, the app hears so; saved as PDF, the pages
    /// are in the file and the app hears they were sent.
    fn printing(out: &Path, wrong: &mut Vec<String>) {
        tap_named(out, "Print");
        wait_for(out, "Copies:");
        let _ = screen(out, "print-preview");
        shell("input keyevent KEYCODE_BACK");
        if wait_for(out, "Printing: closed").is_none() {
            wrong.push("closing the print dialog was not heard".into());
        }
        tap_named(out, "Print");
        // (The dialog keeps the printer picked last: a fresh one has none.)
        if wait_for(out, "Copies:").is_some_and(|_| named(&dump(out, "printer"), "Select a printer").is_some()) {
            tap_named(out, "Select a printer");
            tap_named(out, "Save as PDF");
        }
        tap_named(out, "Save to PDF");
        tap_named(out, "SAVE");
        if wait_for(out, "Printing: sent").is_none() {
            wrong.push("printing to a PDF was not heard as sent".into());
        }
        if !shell("head -c 5 '/sdcard/Download/System parts.pdf'").starts_with("%PDF-") {
            wrong.push("the PDF printed to holds no PDF".into());
        }
    }

    /// The app's own file opens in another app (HTML Viewer), read through the provider.
    fn open_file(out: &Path, wrong: &mut Vec<String>) {
        tap_named(out, "Open in another app");
        tap_named(out, "HTML Viewer");
        tap_named(out, "Just once");
        if wait_for(out, "A page of the system parts").is_none() {
            wrong.push("the page did not open in HTML Viewer".into());
        }
        let _ = screen(out, "page");
        shell("input keyevent KEYCODE_BACK");
    }

    /// A finger dragging from `from` to `to`, a step at a time, through `by`.
    fn drag(from: (i32, i32), by: &[(i32, i32)], to: (i32, i32)) {
        shell(&format!("input motionevent DOWN {} {}", from.0, from.1));
        for (x, y) in by.iter().chain([&to]) {
            shell(&format!("input motionevent MOVE {x} {y}"));
        }
        settle(1);
        shell(&format!("input motionevent UP {} {}", to.0, to.1));
        settle(2);
    }

    /// Dragged past the window's bottom edge (the system's drag) and back onto the box to drop
    /// on, the thing drops there.
    fn drag_back(out: &Path, wrong: &mut Vec<String>) {
        let nodes = dump(out, "drags");
        let (Some(thing), Some(target)) = (named(&nodes, "Drag me"), named(&nodes, "Drop here")) else { return wrong.push("nothing to drag".into()) };
        let (from, to) = (thing.middle(), target.middle());
        drag(from, &[(from.0, from.1 + 300), (from.0, 2390), (to.0, 1500)], to);
        if wait_for(out, "Drops: dragged here inside the window").is_none() {
            wrong.push("dragged out and back, the thing did not drop inside the window".into());
        }
    }

    /// Two apps side by side (free windows): text and a file dragged from one drop on the
    /// other, and the first hears the other took them.
    fn drag_across(out: &Path, other: &str, wrong: &mut Vec<String>) {
        shell("settings put global enable_freeform_support 1; settings put global force_resizable_activities 1");
        shell("am start --windowingMode 5 -n taut.example.system_parts/taut.Activity");
        shell(&format!("am start --windowingMode 5 -n {other}/taut.Activity"));
        settle(4);
        let tasks = shell("dumpsys activity activities");
        let task = |package: &str| tasks.lines().find(|line| line.contains("* Task{") && line.contains(package)).and_then(|line| line.split('#').nth(1)?.split_whitespace().next().map(String::from));
        let (Some(left), Some(right)) = (task("taut.example.system_parts"), task(other)) else { return wrong.push("no two free windows".into()) };
        shell(&format!("am task resize {left} 0 200 540 2300; am task resize {right} 540 200 1080 2300"));
        settle(3);
        let nodes = dump(out, "across");
        let (Some(target), Some(text), Some(file)) = (named(&nodes, "Drop here"), named(&nodes, "Drag me"), named(&nodes, "Drag a file")) else {
            return wrong.push("the right app's parts are not told".into());
        };
        let (to, at) = (target.middle(), |node: &Node| (node.middle().0 - 540, node.middle().1));
        let steps = |from: (i32, i32)| [(from.0 + 60, from.1), (300, from.1), (520, from.1), (600, to.1), (800, to.1)];
        drag(at(text), &steps(at(text)), to);
        if wait_for(out, "Drops: text Dragged from the system parts").is_none() {
            wrong.push("text dragged from the other app was not dropped".into());
        }
        drag(at(file), &steps(at(file)), to);
        if wait_for(out, "Drops: files dragged.txt (Dragged by the system parts)").is_none() {
            wrong.push("a file dragged from the other app was not dropped".into());
        }
        // The first app's window, as it is now.
        shell(&format!("input tap 270 {}", to.1 + 600));
        if wait_for(out, "Drops: file taken by another app (Copied)").is_none() {
            wrong.push("the first app did not hear its file was taken".into());
        }
    }

    #[test]
    fn xml_escapes_in_a_dump_are_undone() {
        assert_eq!(unescaped("a&lt;b&amp;&quot;c&#128512;&#x1F600;"), "a<b&\"c😀😀");
        assert_eq!(unescaped("&unknown; & &#xZZ; fine"), "&unknown; & &#xZZ; fine", "what isn't an escape stays");
    }

    #[test]
    fn adb_errors_from_a_dropped_emulator_are_told_apart() {
        let lost = ["error: device offline\n", "error: no devices/emulators found\n", "error: device 'emulator-5554' not found\n", "adb.exe: device 'emulator-5554' not found\r\n"];
        for lost in lost {
            assert!(never_reached(lost.as_bytes()), "{lost}");
        }
        for reached in ["", "All files should be loaded. Notifying the device.\n", "cat: /sdcard/x: No such file or directory\n", "error: closed\n"] {
            assert!(!never_reached(reached.as_bytes()), "{reached}");
        }
    }

    /// The system parts on an Android emulator: the example `system_parts`, packed twice by
    /// `examples/apk.rs` (`TAUT_ANDROID_PARTS_APK`, and `TAUT_ANDROID_OTHER_APK` with the package
    /// `taut.example.parts2`, for dragging between apps), each part used as a person would. What
    /// was seen is kept in `TAUT_PICTURES` (or a temporary folder).
    #[test]
    fn the_system_parts_on_an_android_emulator() {
        let (Some(apk), Some(other)) = (std::env::var_os("TAUT_ANDROID_PARTS_APK"), std::env::var_os("TAUT_ANDROID_OTHER_APK")) else { return eprintln!("no TAUT_ANDROID_PARTS_APK") };
        let out = std::env::var_os("TAUT_PICTURES").map_or_else(|| std::env::temp_dir().join("taut-android-parts"), PathBuf::from);
        std::fs::create_dir_all(&out).unwrap();
        let keystore = PathBuf::from(&apk).parent().unwrap().join("debug.keystore");
        let mut wrong: Vec<String> = Vec::new();
        for one in [&apk, &other] {
            install(Path::new(one)).unwrap_or_else(|said| panic!("{} was not installed: {said}", one.to_string_lossy()));
        }
        for (name, text) in [("a.txt", "First line of a\nmore\n"), ("b.txt", "First line of b\n")] {
            std::fs::write(out.join(name), text).unwrap();
            adb(&["push", &out.join(name).to_string_lossy(), &format!("/sdcard/Download/{name}")]);
        }
        shell("rm -f /sdcard/Download/notes.txt '/sdcard/Download/System parts.pdf'; settings put global hide_error_dialogs 1");
        adb(&["logcat", "-c"]);
        shell("am start -W -n taut.example.system_parts/taut.Activity");
        if wait_for(&out, "Drag me").is_none() {
            panic!("the app did not show");
        }
        formats(&out, &mut wrong);
        named_faces(&out, &mut wrong);
        spelling(&out, &mut wrong);
        composing(&out, &keystore, &mut wrong);
        file_dialog(&out, &mut wrong);
        clipboard(&out, &mut wrong);
        notification(&out, &mut wrong);
        printing(&out, &mut wrong);
        open_file(&out, &mut wrong);
        drag_back(&out, &mut wrong);
        drag_across(&out, "taut.example.parts2", &mut wrong);
        let log = String::from_utf8_lossy(&adb(&["logcat", "-d"])).into_owned();
        let _ = std::fs::write(out.join("logcat.txt"), &log);
        assert!(!crashed(&["taut.example.system", "taut.example.parts2"]), "an app crashed: see logcat.txt");
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
