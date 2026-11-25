//! The system's parts on the web. One file, two programs:
//!
//! - Built for `wasm32-unknown-unknown`, it is a page (`page`): buttons for the file pickers
//!   (several files, a folder, saving), the clipboard's pictures and files, a notification,
//!   printing and the person's fonts; boxes dragged out (text, a link, a file) and one to drop on;
//!   and a line for each part saying what came of it. It asks to be the program's one running
//!   copy.
//! - Here (`checks`), it builds that page and drives it in a headless browser (Edge or Chrome;
//!   `TAUT_BROWSER` names one) through the DevTools protocol, with real input: the file chooser
//!   answered with files on disk, drags and drops through `Input.dispatchDragEvent`, the
//!   clipboard, notifications and fonts with the browser's permission granted, printing to a
//!   PDF, a second tab, and WebGL2's pixels against the CPU's (then the context lost).
//!
//! Each part says why and passes where it lacks the wasm target or a browser, unless
//! `TAUT_WEB=required`.
#[cfg(not(target_family = "wasm"))]
fn main() {
    checks::all();
}

#[cfg(target_family = "wasm")]
fn main() {
    page::main();
}

#[cfg(target_family = "wasm")]
mod page {
    use std::path::{Path, PathBuf};
    use taut::{Color, DragData, FileDialog, Font, Notification, Paper, PrintJob, PrintOutcome, Ui, Window};

    /// A picture of two by two pixels: red, green, blue, white.
    const PICTURE: [u32; 4] = [0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff];

    /// What the file dialog is up for.
    #[derive(Clone, Copy, PartialEq)]
    enum Asked {
        Files,
        Folder,
        Save,
    }

    /// What the page keeps: the file dialog and what it is up for, whether this is the first
    /// copy, the notification and printing asked for, whether fonts were asked for, the file
    /// dragged out, and what came of each part.
    struct Parts {
        files: FileDialog,
        asked: Asked,
        first: bool,
        notification: Option<Notification>,
        printing: Option<PrintJob>,
        fonts: bool,
        dragged: PathBuf,
        said: [String; 6],
    }

    /// What came of each part, by its place in `said`.
    const FILES: usize = 0;
    const CLIPBOARD: usize = 1;
    const NOTIFIED: usize = 2;
    const PRINTED: usize = 3;
    const DROPS: usize = 4;
    const STARTS: usize = 5;

    pub fn main() {
        let first = taut::single_instance("org.example.WebChecks");
        let dragged = taut::storage::folder("taut-web-checks").unwrap().join("dragged.txt");
        taut::storage::write(&dragged, "Dragged from the checks\n").unwrap();
        let mut parts = Parts { files: FileDialog::native(), asked: Asked::Files, first, notification: None, printing: None, fonts: false, dragged, said: Default::default() };
        parts.said = ["Files: none", "Clipboard: none", "Notification: none", "Printing: none", "Drops: none", "Starts: none"].map(String::from);
        taut::run(Window { title: "web checks", ..Default::default() }, move |ui| describe(&mut parts, ui));
    }

    fn describe(parts: &mut Parts, ui: &mut Ui) {
        answers(parts, ui);
        ui.column().pad(12.0).gap(6.0).grow().open();
        ui.label(if parts.first { "Copy: first" } else { "Copy: later" });
        file_buttons(parts, ui);
        ui.row().gap(8.0).open();
        if ui.button("Copy picture").clicked {
            ui.copy_picture(2, &PICTURE);
            parts.said[CLIPBOARD] = "Clipboard: picture copied".into();
        }
        if ui.button("Paste picture").clicked {
            parts.said[CLIPBOARD] = match ui.clipboard_picture() {
                Some((width, pixels)) => format!("Clipboard: picture {width}x{}, {}", pixels.len() / width.max(1), if pixels == PICTURE { "as copied" } else { "another" }),
                None => "Clipboard: no picture".into(),
            };
        }
        if ui.button("Paste files").clicked {
            let pasted: Vec<String> = ui.clipboard_files().iter().map(|file| name(file)).collect();
            parts.said[CLIPBOARD] = format!("Clipboard: files {}", pasted.join(", "));
        }
        ui.close();
        ui.row().gap(8.0).open();
        if ui.button("Notify").clicked {
            parts.notification = Some(ui.system_notification("Hello", "From the web checks", Some((2, &PICTURE))));
            parts.said[NOTIFIED] = "Notification: posted".into();
        }
        if ui.button("Print").clicked {
            parts.printing = Some(taut::print(two_pages(), "Web checks"));
            parts.said[PRINTED] = "Printing: the dialog is up".into();
        }
        parts.fonts |= ui.button("Fonts").clicked;
        ui.close();
        if parts.fonts {
            ui.label(&format!("Fonts: {} families", taut::system_fonts::families().len()));
        }
        drags(parts, ui);
        for said in &parts.said {
            ui.label(said);
        }
        ui.close();
    }

    /// Two pages: A4, then Letter.
    fn two_pages() -> Paper {
        let mut paper = Paper::new();
        for (size, words) in [(Paper::A4, "Printed by the web checks"), (Paper::LETTER, "Its second page")] {
            paper.page(size);
            paper.text(words, 72.0, 72.0, Font::new(18.0), Color::BLACK);
        }
        paper
    }

    /// What the browser answered since the last run: the file dialog, a click on the
    /// notification, printing, later tabs.
    fn answers(parts: &mut Parts, ui: &mut Ui) {
        if let Some(file) = ui.file_dialog(&mut parts.files) {
            parts.said[FILES] = match parts.asked {
                Asked::Save => taut::storage::write(&file, "Saved by the web checks\n").map_or_else(|error| format!("Files: not saved: {error}"), |()| format!("Files: saved {}", file.display())),
                Asked::Folder => format!("Files: folder {}", read(&taut::web::files_in(&file))),
                Asked::Files => format!("Files: opened {}", read(&parts.files.chosen)),
            };
        }
        if let Some(clicked) = ui.notification_clicked() {
            parts.said[NOTIFIED] = format!("Notification: clicked{}", if parts.notification == Some(clicked) { "" } else { ", another" });
        }
        if let Some(outcome) = parts.printing.as_ref().and_then(PrintJob::outcome) {
            parts.said[PRINTED] = format!("Printing: {}", if outcome == PrintOutcome::Sent { "sent" } else { "unable" });
            parts.printing = None;
        }
        for start in ui.relaunches() {
            parts.said[STARTS] = format!("Starts: {}", start.args.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>().join(" "));
        }
    }

    /// The file dialog's buttons.
    fn file_buttons(parts: &mut Parts, ui: &mut Ui) {
        ui.row().gap(8.0).open();
        if ui.button("Open several").clicked {
            parts.asked = Asked::Files;
            parts.files.pick_many("/");
        }
        if ui.button("Open a folder").clicked {
            parts.asked = Asked::Folder;
            parts.files.pick_folder("/");
        }
        if ui.button("Save").clicked {
            parts.asked = Asked::Save;
            parts.files.save("/", "notes.txt");
        }
        ui.close();
    }

    /// Boxes to drag out, and a place to drop on.
    fn drags(parts: &mut Parts, ui: &mut Ui) {
        ui.row().gap(8.0).open();
        let text = ui.selectable(false, "Drag text");
        if ui.drag_source(&text, 1) {
            ui.label("Drag text");
            ui.close();
        }
        let link = ui.selectable(false, "Drag a link");
        let file = ui.selectable(false, "Drag a file");
        let dragged = [parts.dragged.clone()];
        let taken = [ui.drag_out(&text, DragData::Text("Dragged from the checks")), ui.drag_out(&link, DragData::Link("https://example.org/checks")), ui.drag_out(&file, DragData::Files(&dragged))];
        if let Some(effect) = taken.into_iter().flatten().next() {
            parts.said[DROPS] = format!("Drops: taken elsewhere ({effect:?})");
        }
        let line = ui.theme.line;
        let target = ui.column().pad(8.0).border(line, 1.0).sense().open();
        ui.label(if ui.files_hovered() || ui.text_hovered() { "Let go here" } else { "Drop here" });
        ui.close();
        if ui.dropped(&target).is_some() {
            parts.said[DROPS] = "Drops: dragged here inside the page".into();
        }
        if let Some(files) = ui.files_dropped(&target) {
            parts.said[DROPS] = format!("Drops: files {}", read(&files));
        }
        if let Some(text) = ui.text_dropped(&target) {
            parts.said[DROPS] = format!("Drops: text {text}");
        }
        ui.close();
    }

    /// Files as their names and first lines.
    fn read(files: &[PathBuf]) -> String {
        let read: Vec<String> = files.iter().map(|file| format!("{} ({})", name(file), taut::storage::read(file).unwrap_or_default().lines().next().unwrap_or("unread"))).collect();
        read.join(", ")
    }

    fn name(file: &Path) -> String {
        file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// The page's server: as the examples serve pages.
#[cfg(not(target_family = "wasm"))]
#[allow(dead_code)]
#[path = "../examples/web_page.rs"]
mod web_page;

#[cfg(not(target_family = "wasm"))]
mod checks {
    use crate::devtools::{Browser, Json};
    use std::io::ErrorKind;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// The page the checks run in: the program in an element of 700 by 640 CSS pixels, and
    /// beside it a place standing for another program that takes drops. Notifications shown
    /// are kept, to be clicked; printing only notes it was asked (the checks print to a PDF).
    const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<style>html, body { margin: 0; background: #808080; } #app { width: 700px; height: 640px; } #elsewhere { position: absolute; left: 720px; top: 20px; width: 200px; height: 200px; background: #ccc; }</style>
<div id="app"></div>
<div id="elsewhere">Elsewhere</div>
<script type="module">
import { start } from "./taut.js";
window.errors = [];
const told = console.error;
console.error = (...said) => {
    errors.push(said.join(" "));
    told(...said);
};
addEventListener("error", (event) => errors.push(String(event.error?.stack || event.message)));
const elsewhere = document.getElementById("elsewhere");
elsewhere.addEventListener("dragover", (event) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
});
elsewhere.addEventListener("drop", (event) => {
    event.preventDefault();
    const data = event.dataTransfer;
    window.droppedElsewhere = [...data.types].map((type) => [type, type === "Files" ? [...data.files].map((file) => file.name).join(",") : data.getData(type)]);
});
addEventListener("dragstart", (event) => (window.started = [...event.dataTransfer.types].map((type) => [type, type === "Files" ? [...event.dataTransfer.files].map((file) => file.name).join(",") : event.dataTransfer.getData(type)])));
window.shown = [];
if (typeof Notification === "function") {
    window.Notification = class extends Notification {
        constructor(title, options) {
            super(title, options);
            this.said = [title, options.body, Boolean(options.icon)];
            shown.push(this);
        }
    };
}
window.printed = 0;
window.print = () => (window.printed += 1);
const renderer = new URLSearchParams(location.search).get("renderer") || undefined;
window.page = await start("page.wasm", { element: document.getElementById("app"), renderer, draw: renderer });
</script>
"#;

    /// Most a channel of WebGL2's pixels may differ from the CPU's (as `gpu_draw` allows).
    const TOLERANCE: u32 = 4;

    pub fn all() {
        let Some(program) = page() else { return };
        let Some(browser) = browser() else {
            skipped(false, "no browser (TAUT_BROWSER names one)");
            return;
        };
        let folder = work(&program);
        let server = Server::start(&folder);
        let mut browser = Browser::start(&browser, &folder.join("profile"));
        let address = format!("http://127.0.0.1:{}/", server.port);
        browser.grant(&address, &["clipboardReadWrite", "clipboardSanitizedWrite", "notifications", "localFonts"]);
        let mut tab = Tab::open(&mut browser, &format!("{address}?tab=first"));
        later_tab(&mut tab, &address);
        files(&mut tab, &folder);
        clipboard(&mut tab);
        notification(&mut tab);
        printing(&mut tab);
        fonts(&mut tab);
        drops(&mut tab, &folder);
        drags(&mut tab);
        let errors = tab.eval("JSON.stringify(window.errors)");
        assert_eq!(errors.text(), "[]", "the page said nothing wrong");
        webgl(&mut tab, &address);
        drop(tab);
        drop(server);
        println!("the system's parts on the web: every check passed");
    }

    /// Whether to go on: `false`, said, where `there` is not, unless `TAUT_WEB=required`.
    fn skipped(there: bool, missing: &str) -> bool {
        assert!(there || std::env::var("TAUT_WEB").as_deref() != Ok("required"), "{missing}");
        if !there {
            println!("{missing}: skipped");
        }
        there
    }

    /// This file built for the web, as a page's program; none without the wasm target.
    fn page() -> Option<PathBuf> {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("web-system");
        let mut build = Command::new(cargo);
        build.current_dir(env!("CARGO_MANIFEST_DIR")).args(["build", "--release", "--test", "web_system", "--features", "gpu-draw", "--target", "wasm32-unknown-unknown", "--message-format=json"]);
        let made = build.env("CARGO_TARGET_DIR", &target).output().expect("cargo runs");
        // (With JSON messages the compiler's errors come on standard output.)
        let (out, said) = (String::from_utf8_lossy(&made.stdout), String::from_utf8_lossy(&made.stderr));
        let missing = ["may not be installed", "can't find crate for `std`", "can't find crate for `core`"].iter().any(|told| out.contains(told) || said.contains(told));
        if !made.status.success() && !skipped(!missing, "no wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)") {
            return None;
        }
        assert!(made.status.success(), "the page builds for the web:\n{out}\n{said}");
        let line = out.lines().find(|line| line.contains("\"executable\":\"") && line.contains("\"name\":\"web_system\""));
        let program = line.and_then(|line| line.split("\"executable\":\"").nth(1)?.split('"').next()).expect("a program built");
        Some(PathBuf::from(program.replace("\\\\", "\\")))
    }

    /// A browser: `TAUT_BROWSER`, else Edge or Chrome where they are usually installed.
    fn browser() -> Option<PathBuf> {
        if let Some(named) = std::env::var_os("TAUT_BROWSER") {
            return Some(named.into());
        }
        let mut places: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            let edge = Path::new(r"C:\Program Files (x86)\Microsoft\EdgeCore");
            places.extend(std::fs::read_dir(edge).into_iter().flatten().flatten().map(|version| version.path().join("msedge.exe")));
            places.extend([r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Google\Chrome\Application\chrome.exe"].map(PathBuf::from));
        }
        let found = places.into_iter().find(|place| place.exists());
        let on_path = |name: &&str| Command::new(name).arg("--version").output().is_ok_and(|ran| ran.status.success());
        found.or_else(|| ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"].into_iter().find(on_path).map(PathBuf::from))
    }

    /// The checks' folder, emptied: the page, the loader, the program, and files to pick, drop
    /// and save into.
    fn work(program: &Path) -> PathBuf {
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("web-system-checks");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("picked").join("inner")).unwrap();
        std::fs::create_dir_all(folder.join("downloads")).unwrap();
        std::fs::write(folder.join("index.html"), PAGE).unwrap();
        std::fs::write(folder.join("taut.js"), taut::web::LOADER).unwrap();
        std::fs::copy(program, folder.join("page.wasm")).unwrap();
        for (name, text) in [("one.txt", "one\n"), ("two.txt", "two\n"), ("three.txt", "three\n"), ("inner/four.txt", "four\n")] {
            std::fs::write(folder.join("picked").join(name), text).unwrap();
        }
        folder
    }

    /// Serves the checks' folder on a thread of its own, each connection on its own thread (a
    /// browser keeps idle ones open).
    struct Server {
        port: u16,
        stop: Arc<Mutex<bool>>,
    }

    impl Server {
        fn start(folder: &Path) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let (port, stop) = (listener.local_addr().unwrap().port(), Arc::new(Mutex::new(false)));
            let (folder, stopped) = (folder.to_path_buf(), stop.clone());
            std::thread::spawn(move || {
                while !*stopped.lock().unwrap() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let folder = folder.clone();
                            stream.set_nonblocking(false).unwrap();
                            std::thread::spawn(move || crate::web_page::answer(stream, &folder, &mut |_, _| {}));
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                        Err(error) => panic!("{error}"),
                    }
                }
            });
            Server { port, stop }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            *self.stop.lock().unwrap() = true;
        }
    }

    /// A tab of the browser showing the page, and what it is driven through.
    struct Tab<'a> {
        browser: &'a mut Browser,
        session: String,
    }

    impl<'a> Tab<'a> {
        /// A new tab at `address`, once the page has started.
        fn open(browser: &'a mut Browser, address: &str) -> Tab<'a> {
            let session = browser.open_tab(address);
            let mut tab = Tab { browser, session };
            tab.call("Emulation.setFocusEmulationEnabled", r#"{"enabled":true}"#);
            tab.call("Page.enable", "{}");
            tab.until("the page started", "window.page !== undefined && page.readers.querySelector('[data-key]') !== null");
            tab
        }

        fn call(&mut self, method: &str, params: &str) -> Json {
            self.browser.call(Some(&self.session), method, params)
        }

        /// What a script gives, awaited.
        fn eval(&mut self, script: &str) -> Json {
            let params = format!(r#"{{"expression":{},"awaitPromise":true,"returnByValue":true}}"#, Json::quoted(script));
            let answer = self.call("Runtime.evaluate", &params);
            assert!(answer.get("exceptionDetails").is_none(), "{script}: {answer:?}");
            answer.get("result").and_then(|result| result.get("value")).cloned().unwrap_or(Json::Null)
        }

        /// Waits until a script says true, at most 30 seconds.
        fn until(&mut self, what: &str, script: &str) {
            let since = Instant::now();
            while self.eval(script) != Json::Bool(true) {
                assert!(since.elapsed() < Duration::from_secs(30), "{what}: not within 30 s");
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// The line the page says starting with `start`, once it says it has `wanted` in it.
        fn said(&mut self, start: &str, wanted: &str) -> String {
            let script = format!("[...page.readers.querySelectorAll('[data-key]')].map((made) => made.textContent).find((line) => line.startsWith({}))", Json::quoted(start));
            let since = Instant::now();
            loop {
                let line = self.eval(&script).text();
                if line.contains(wanted) || since.elapsed() > Duration::from_secs(30) {
                    return line;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// The middle of a part, by its name, in CSS pixels.
        fn middle(&mut self, name: &str) -> (f64, f64) {
            let script = format!(
                "(() => {{ const made = [...page.readers.querySelectorAll('[data-key]')].find((made) => made.getAttribute('aria-label') === {0} || made.textContent === {0}); const place = made.getBoundingClientRect(); return [place.left + place.width / 2, place.top + place.height / 2]; }})()",
                Json::quoted(name)
            );
            let at = self.eval(&script);
            (at.at(0).number(), at.at(1).number())
        }

        fn mouse(&mut self, kind: &str, (x, y): (f64, f64), buttons: u32) {
            let button = if kind == "mouseMoved" && buttons == 0 { "none" } else { "left" };
            self.call("Input.dispatchMouseEvent", &format!(r#"{{"type":"{kind}","x":{x},"y":{y},"button":"{button}","buttons":{buttons},"clickCount":1}}"#));
        }

        /// Clicks a part with the mouse, by its name.
        fn click(&mut self, name: &str) {
            let at = self.middle(name);
            self.mouse("mouseMoved", at, 0);
            self.mouse("mousePressed", at, 1);
            self.mouse("mouseReleased", at, 0);
        }

        /// The pixels shown in the program's element, by a picture of the page: its width and
        /// pixels (`0xAARRGGBB`).
        fn picture(&mut self) -> (usize, Vec<u32>) {
            let shot = self.call("Page.captureScreenshot", r#"{"format":"png","clip":{"x":0,"y":0,"width":700,"height":640,"scale":1}}"#);
            let png = base64(&shot.get("data").expect("a picture").text());
            taut::png::decode(&png).expect("a PNG")
        }
    }

    impl Drop for Tab<'_> {
        fn drop(&mut self) {
            let session = self.session.clone();
            self.browser.close_tab(&session);
        }
    }

    /// A second tab of the program hands its address to the first, and is told it is not the
    /// first.
    fn later_tab(first: &mut Tab, address: &str) {
        assert!(first.said("Copy:", "").ends_with("first"), "the first tab");
        let later = format!("{address}?tab=later");
        let mut second = Tab::open(first.browser, &later);
        assert_eq!(second.said("Copy:", "later"), "Copy: later");
        drop(second);
        assert_eq!(first.said("Starts:", "later"), format!("Starts: {later}"));
    }

    /// Several files and a folder picked in the browser's file chooser (answered here with files
    /// on disk), and a file saved: through the save picker where the browser offers one (its
    /// own screen stood in for), else as a download (handed to the browser: headless Edge cancels
    /// downloads at their end).
    fn files(tab: &mut Tab, folder: &Path) {
        tab.call("Page.setInterceptFileChooserDialog", r#"{"enabled":true}"#);
        let picked = folder.join("picked");
        let paths = [picked.join("one.txt"), picked.join("two.txt")];
        tab.click("Open several");
        choose(tab, &paths, "selectMultiple");
        assert_eq!(tab.said("Files:", "opened"), "Files: opened one.txt (one), two.txt (two)");
        tab.click("Open a folder");
        choose(tab, std::slice::from_ref(&picked), "");
        let line = tab.said("Files:", "folder");
        for file in ["one.txt (one)", "two.txt (two)", "three.txt (three)", "four.txt (four)"] {
            assert!(line.contains(file), "{file} in {line}");
        }
        // The save picker: its screen answered as the person would, the file written through
        // the handle it gives.
        if tab.eval("typeof showSaveFilePicker === 'function'") == Json::Bool(true) {
            tab.eval("window.showSaveFilePicker = async (options) => (window.saving = { name: options.suggestedName, written: [], createWritable: async () => ({ write: async (bytes) => saving.written.push(new TextDecoder().decode(bytes)), close: async () => {} }) })");
            tab.click("Save");
            assert_eq!(tab.said("Files:", "saved"), "Files: saved /saved/notes.txt");
            tab.until("the file written", "window.saving.written.join('') === 'Saved by the web checks\\n'");
        }
        // Without one, a download.
        tab.eval("Object.defineProperty(window, 'showSaveFilePicker', { value: undefined, configurable: true }); page.download = (name, bytes) => (window.downloaded = [name, new TextDecoder().decode(bytes)]); true");
        tab.click("Save");
        assert_eq!(tab.said("Files:", "downloads"), "Files: saved /downloads/notes.txt");
        assert_eq!(tab.eval("JSON.stringify(window.downloaded)").text(), r#"["notes.txt","Saved by the web checks\n"]"#);
    }

    /// Answers the file chooser the page opened with `paths`.
    fn choose(tab: &mut Tab, paths: &[PathBuf], mode: &str) {
        let opened = tab.browser.event(&tab.session, "Page.fileChooserOpened");
        assert!(mode.is_empty() || opened.get("mode").map(Json::text).as_deref() == Some(mode), "{opened:?}");
        let node = opened.get("backendNodeId").expect("the chooser's input").number();
        let files: Vec<String> = paths.iter().map(|path| Json::quoted(&path.to_string_lossy())).collect();
        tab.call("DOM.setFileInputFiles", &format!(r#"{{"files":[{}],"backendNodeId":{node}}}"#, files.join(",")));
    }

    /// A picture copied is on the system clipboard as PNG; one put there by another program is
    /// the clipboard's once the page has the keyboard again; one pasted with Control+V comes as
    /// the picture and as a pasted file.
    fn clipboard(tab: &mut Tab) {
        tab.click("Copy picture");
        assert_eq!(tab.said("Clipboard:", "copied"), "Clipboard: picture copied");
        tab.until("a picture on the system clipboard", "navigator.clipboard.read().then((items) => items.some((item) => item.types.includes('image/png')))");
        let read = "(async () => { const items = await navigator.clipboard.read(); const png = await items.find((item) => item.types.includes('image/png')).getType('image/png'); const shown = await createImageBitmap(png); const canvas = new OffscreenCanvas(shown.width, shown.height); const context = canvas.getContext('2d'); context.drawImage(shown, 0, 0); return [shown.width, ...context.getImageData(0, 0, shown.width, shown.height).data]; })()";
        let copied = tab.eval(read);
        assert_eq!(copied.numbers(), [2, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255], "the picture on the system clipboard");
        // Another program copies a picture of three pixels; the page gains the keyboard.
        let other = "(async () => { const canvas = new OffscreenCanvas(3, 1); const context = canvas.getContext('2d'); context.fillStyle = '#123456'; context.fillRect(0, 0, 3, 1); await navigator.clipboard.write([new ClipboardItem({ 'image/png': await canvas.convertToBlob({ type: 'image/png' }) })]); document.activeElement.blur(); page.box.focus(); return true; })()";
        tab.eval(other);
        std::thread::sleep(Duration::from_millis(300));
        tab.click("Paste picture");
        assert_eq!(tab.said("Clipboard:", "picture 3x1"), "Clipboard: picture 3x1, another");
        // Pasted with Control+V: the picture, and its file.
        tab.eval("page.box.focus(); true");
        for kind in ["rawKeyDown", "keyUp"] {
            tab.call("Input.dispatchKeyEvent", &format!(r#"{{"type":"{kind}","modifiers":2,"key":"v","code":"KeyV","windowsVirtualKeyCode":86,"commands":["paste"]}}"#));
        }
        std::thread::sleep(Duration::from_millis(300));
        tab.click("Paste files");
        let pasted = tab.said("Clipboard:", "files ");
        assert!(pasted.starts_with("Clipboard: files ") && pasted.len() > "Clipboard: files ".len(), "{pasted}");
    }

    /// A notification shows with the title, text and picture asked for; its click is heard.
    fn notification(tab: &mut Tab) {
        tab.click("Notify");
        tab.until("a notification shown", "shown.length === 1");
        assert_eq!(tab.eval("JSON.stringify(shown[0].said)").text(), r#"["Hello","From the web checks",true]"#);
        tab.eval("shown[0].dispatchEvent(new Event('click')); true");
        assert_eq!(tab.said("Notification:", "clicked"), "Notification: clicked");
    }

    /// Two pages printed to a PDF through the print style the page gets: two sheets, A4 and
    /// Letter, each a picture; then the dialog closes.
    fn printing(tab: &mut Tab) {
        tab.click("Print");
        tab.until("print asked for", "window.printed === 1");
        let pdf = tab.call("Page.printToPDF", r#"{"preferCSSPageSize":true,"printBackground":true}"#);
        let pdf = base64(&pdf.get("data").expect("a PDF").text());
        let text = String::from_utf8_lossy(&pdf);
        let sheets: Vec<&str> = text.split("/MediaBox").skip(1).map(|rest| rest.split(']').next().unwrap_or_default()).collect();
        assert_eq!(sheets.len(), 2, "two sheets");
        let sizes: Vec<Vec<f64>> = sheets.iter().map(|sheet| sheet.trim_start_matches([' ', '[']).split_whitespace().filter_map(|number| number.parse().ok()).collect()).collect();
        let near = |size: &[f64], wanted: [f64; 2]| size.len() == 4 && (size[2] - wanted[0]).abs() < 1.0 && (size[3] - wanted[1]).abs() < 1.0;
        assert!(near(&sizes[0], [595.3, 841.9]) && near(&sizes[1], [612.0, 792.0]), "A4, then Letter: {sizes:?}");
        assert!(text.matches("/Subtype /Image").count() >= 2, "each sheet a picture");
        tab.eval("dispatchEvent(new Event('afterprint')); true");
        assert_eq!(tab.said("Printing:", "sent"), "Printing: sent");
    }

    /// The person's fonts, as the browser lists them (granted), where it can list them.
    fn fonts(tab: &mut Tab) {
        tab.click("Fonts");
        if tab.eval("typeof queryLocalFonts === 'function'") != Json::Bool(true) {
            assert_eq!(tab.said("Fonts:", ""), "Fonts: 0 families", "none listed where the browser lists none");
            return;
        }
        tab.until("the fonts listed", "[...page.readers.querySelectorAll('[data-key]')].some((made) => /^Fonts: [1-9]/.test(made.textContent))");
    }

    /// Files and text dragged onto the drop box from outside the page.
    fn drops(tab: &mut Tab, folder: &Path) {
        let at = tab.middle("Drop here");
        let file = Json::quoted(&folder.join("picked").join("three.txt").to_string_lossy());
        drag_in(tab, at, &format!(r#"{{"items":[],"files":[{file}],"dragOperationsMask":1}}"#));
        assert_eq!(tab.said("Drops:", "files"), "Drops: files three.txt (three)");
        drag_in(tab, at, r#"{"items":[{"mimeType":"text/plain","data":"dropped words"}],"dragOperationsMask":1}"#);
        assert_eq!(tab.said("Drops:", "text"), "Drops: text dropped words");
    }

    /// A drag from outside the page, over `at` and let go there.
    fn drag_in(tab: &mut Tab, (x, y): (f64, f64), data: &str) {
        for kind in ["dragEnter", "dragOver", "drop"] {
            tab.call("Input.dispatchDragEvent", &format!(r#"{{"type":"{kind}","x":{x},"y":{y},"data":{data}}}"#));
        }
    }

    /// Boxes dragged with the mouse: text and a link go out as what the program hands out, and
    /// another program takes a copy; a file goes out as a file; text let go on the page's own
    /// box drops there.
    fn drags(tab: &mut Tab) {
        tab.call("Input.setInterceptDrags", r#"{"enabled":true}"#);
        let (elsewhere, target) = ((820.0, 120.0), tab.middle("Drop here"));
        for (name, kind, wanted) in [("Drag text", "text/plain", "Dragged from the checks"), ("Drag a link", "text/uri-list", "https://example.org/checks")] {
            let data = drag_out(tab, name, elsewhere);
            drag_in(tab, elsewhere, &data.to_string());
            tab.mouse("mouseReleased", elsewhere, 0);
            let dropped = tab.eval("JSON.stringify(window.droppedElsewhere)").text();
            assert!(dropped.contains(&format!("[\"{kind}\",\"{wanted}\"]")), "{name}: {dropped}");
            assert_eq!(tab.said("Drops:", "Copied"), "Drops: taken elsewhere (Copied)", "{name}");
            drag_in(tab, target, r#"{"items":[{"mimeType":"text/plain","data":"again"}],"dragOperationsMask":1}"#);
            assert_eq!(tab.said("Drops:", "again"), "Drops: text again");
        }
        // A file goes out as a file (seen as the drag starts: the protocol carries no file a
        // page made), and for Chrome as a download where it is let go on the desktop.
        tab.eval("window.started = undefined; true");
        let data = drag_out(tab, "Drag a file", elsewhere);
        let started = tab.eval("JSON.stringify(window.started)").text();
        assert!(started.contains(r#"["Files","dragged.txt"]"#), "{started}");
        assert!(data.to_string().contains(r#""mimeType":"downloadurl","data":"application/octet-stream:dragged.txt:blob:http"#), "{data}");
        drag_in(tab, elsewhere, &data.to_string());
        tab.mouse("mouseReleased", elsewhere, 0);
        assert_eq!(tab.said("Drops:", "Copied"), "Drops: taken elsewhere (Copied)", "a file");
        // Let go on the page's own box, it drops there.
        let data = drag_out(tab, "Drag text", target);
        drag_in(tab, target, &data.to_string());
        tab.mouse("mouseReleased", target, 0);
        assert_eq!(tab.said("Drops:", "inside"), "Drops: dragged here inside the page");
    }

    /// Presses a box and moves the mouse to `to`: the drag the browser starts, as it hands it
    /// over.
    fn drag_out(tab: &mut Tab, name: &str, to: (f64, f64)) -> Json {
        press_and_move(tab, name, to);
        tab.browser.event(&tab.session, "Input.dragIntercepted").get("data").cloned().expect("the drag's data")
    }

    /// Presses a box with the mouse and moves it to `to` in steps.
    fn press_and_move(tab: &mut Tab, name: &str, to: (f64, f64)) {
        let from = tab.middle(name);
        tab.mouse("mouseMoved", from, 0);
        tab.mouse("mousePressed", from, 1);
        for step in 1..=10 {
            let part = f64::from(step) / 10.0;
            tab.mouse("mouseMoved", (from.0 + (to.0 - from.0) * part, from.1 + (to.1 - from.1) * part), 1);
        }
    }

    /// The page shown through WebGL2, drawing by the GPU: what it shows against the CPU's; then
    /// its context lost, the CPU's again.
    fn webgl(first: &mut Tab, address: &str) {
        let cpu = {
            let mut tab = Tab::open(first.browser, &format!("{address}?renderer=cpu"));
            tab.picture()
        };
        let mut tab = Tab::open(first.browser, &format!("{address}?renderer=gpu"));
        if !skipped(tab.eval("page.gl !== null") == Json::Bool(true), "no WebGL2 in this browser") {
            return;
        }
        let name = tab.eval("page.glName").text();
        assert!(tab.eval("page.glObjects.length > 8") == Json::Bool(true), "the GPU drew the page itself ({name})");
        let gpu = tab.picture();
        let most = most_apart(&cpu, &gpu);
        println!("WebGL2 on {name}: at most {most} levels from the CPU's");
        assert!(most <= TOLERANCE, "WebGL2's pixels {most} levels from the CPU's");
        tab.eval("page.gl.getExtension('WEBGL_lose_context').loseContext(); true");
        tab.until("the CPU shows the page again", "page.glCanvas === null");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(most_apart(&cpu, &tab.picture()), 0, "after the context was lost, the CPU's pixels");
    }

    /// The most two pictures' channels differ.
    fn most_apart(one: &(usize, Vec<u32>), other: &(usize, Vec<u32>)) -> u32 {
        assert_eq!((one.0, one.1.len()), (other.0, other.1.len()), "pictures of one size");
        let apart = |a: u32, b: u32| (0..4).map(|byte| ((a >> (byte * 8)) & 255).abs_diff((b >> (byte * 8)) & 255)).max().unwrap_or(0);
        one.1.iter().zip(&other.1).map(|(a, b)| apart(*a, *b)).max().unwrap_or(0)
    }

    /// Bytes from base64.
    fn base64(text: &str) -> Vec<u8> {
        let value = |letter: u8| match letter {
            b'A'..=b'Z' => letter - b'A',
            b'a'..=b'z' => letter - b'a' + 26,
            b'0'..=b'9' => letter - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let letters: Vec<u8> = text.bytes().filter(|letter| *letter != b'=').map(value).collect();
        letters
            .chunks(4)
            .flat_map(|four| {
                let word = four.iter().enumerate().fold(0u32, |word, (at, six)| word | u32::from(*six) << (18 - 6 * at));
                word.to_be_bytes()[1..four.len()].to_vec()
            })
            .collect()
    }
}

/// The DevTools protocol over a WebSocket, as much as the checks use: a browser started with a
/// profile of its own, its tabs, calls and their answers, and events.
#[cfg(not(target_family = "wasm"))]
mod devtools {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// A headless browser and the WebSocket it is driven through; events heard while waiting
    /// for answers, kept until asked for.
    pub struct Browser {
        child: Child,
        socket: TcpStream,
        next: u64,
        events: Vec<Json>,
    }

    impl Browser {
        /// Starts `program` headless with the profile folder `profile`.
        pub fn start(program: &Path, profile: &Path) -> Browser {
            let mut started = Command::new(program);
            started.args(["--headless=new", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-sync", "--mute-audio", "--hide-scrollbars"]);
            started.args(["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--enable-unsafe-swiftshader", "--window-size=1000,700", "--force-device-scale-factor=1"]);
            started.arg("--remote-debugging-port=0").arg(format!("--user-data-dir={}", profile.display())).arg("about:blank");
            let child = started.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the browser starts");
            let port_file = profile.join("DevToolsActivePort");
            let since = Instant::now();
            let said = loop {
                if let Some(said) = std::fs::read_to_string(&port_file).ok().filter(|said| said.lines().count() >= 2) {
                    break said;
                }
                assert!(since.elapsed() < Duration::from_secs(60), "the browser listens within a minute");
                std::thread::sleep(Duration::from_millis(50));
            };
            let mut lines = said.lines();
            let (port, path) = (lines.next().unwrap().trim().to_string(), lines.next().unwrap().trim().to_string());
            let socket = connect(&port, &path);
            Browser { child, socket, next: 0, events: Vec::new() }
        }

        /// Grants `permissions` to the pages of `origin`.
        pub fn grant(&mut self, origin: &str, permissions: &[&str]) {
            let named: Vec<String> = permissions.iter().map(|name| Json::quoted(name)).collect();
            self.call(None, "Browser.grantPermissions", &format!(r#"{{"origin":{},"permissions":[{}]}}"#, Json::quoted(origin.trim_end_matches('/')), named.join(",")));
        }

        /// A new tab at `address`: the session it is driven through.
        pub fn open_tab(&mut self, address: &str) -> String {
            let made = self.call(None, "Target.createTarget", &format!(r#"{{"url":{}}}"#, Json::quoted(address)));
            let target = made.get("targetId").expect("a tab").text();
            let attached = self.call(None, "Target.attachToTarget", &format!(r#"{{"targetId":{},"flatten":true}}"#, Json::quoted(&target)));
            attached.get("sessionId").expect("a session").text()
        }

        /// Closes the tab of `session`.
        pub fn close_tab(&mut self, session: &str) {
            let target = self.call(Some(session), "Target.getTargetInfo", "{}");
            let target = target.get("targetInfo").and_then(|info| info.get("targetId")).map(Json::text).unwrap_or_default();
            self.call(None, "Target.closeTarget", &format!(r#"{{"targetId":{}}}"#, Json::quoted(&target)));
        }

        /// Calls `method` (in a tab's `session`, or the browser's): its result.
        pub fn call(&mut self, session: Option<&str>, method: &str, params: &str) -> Json {
            self.next += 1;
            let id = self.next;
            let session = session.map(|session| format!(r#""sessionId":{},"#, Json::quoted(session))).unwrap_or_default();
            send(&mut self.socket, &format!(r#"{{"id":{id},{session}"method":"{method}","params":{params}}}"#));
            loop {
                let heard = Json::parse(&receive(&mut self.socket));
                if heard.get("id").map(Json::number) != Some(id as f64) {
                    self.events.extend(heard.get("method").is_some().then_some(heard));
                    continue;
                }
                assert!(heard.get("error").is_none(), "{method}: {heard:?}");
                return heard.get("result").cloned().unwrap_or(Json::Null);
            }
        }

        /// The next event `method` of a tab's `session`: its parameters. At most 30 seconds.
        pub fn event(&mut self, session: &str, method: &str) -> Json {
            let ours = |heard: &Json| heard.get("method").map(Json::text).as_deref() == Some(method) && heard.get("sessionId").map(Json::text).as_deref() == Some(session);
            self.socket.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            loop {
                if let Some(at) = self.events.iter().position(ours) {
                    return self.events.remove(at).get("params").cloned().unwrap_or(Json::Null);
                }
                let heard = Json::parse(&receive(&mut self.socket));
                self.events.push(heard);
            }
        }
    }

    impl Drop for Browser {
        /// Only the browser started here is stopped, by its own handle.
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Opens the WebSocket at `path` on `port`.
    fn connect(port: &str, path: &str) -> TcpStream {
        let mut socket = TcpStream::connect(format!("127.0.0.1:{port}")).expect("the browser's port");
        let ask =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
        socket.write_all(ask.as_bytes()).unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains(" 101 "), "the browser took the WebSocket: {line}");
        // (The rest of the answer's head, up to its empty line.)
        while line != "\r\n" {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0, "the answer's head ends");
        }
        socket
    }

    /// Sends a text frame, masked as a client's are.
    fn send(socket: &mut TcpStream, text: &str) {
        let (bytes, mask) = (text.as_bytes(), [0x12u8, 0x34, 0x56, 0x78]);
        let mut frame = vec![0x81u8];
        match bytes.len() {
            length @ 0..=125 => frame.push(0x80 | length as u8),
            length @ 126..=65535 => frame.extend([0x80 | 126].into_iter().chain((length as u16).to_be_bytes())),
            length => frame.extend([0x80 | 127].into_iter().chain((length as u64).to_be_bytes())),
        }
        frame.extend(mask);
        frame.extend(bytes.iter().enumerate().map(|(at, byte)| byte ^ mask[at % 4]));
        socket.write_all(&frame).unwrap();
    }

    /// Receives a whole message (its frames joined), answering pings.
    fn receive(socket: &mut TcpStream) -> String {
        let mut message = Vec::new();
        loop {
            let mut head = [0u8; 2];
            socket.read_exact(&mut head).expect("the browser answers");
            let length = match head[1] & 0x7f {
                126 => {
                    let mut more = [0u8; 2];
                    socket.read_exact(&mut more).unwrap();
                    u64::from(u16::from_be_bytes(more))
                }
                127 => {
                    let mut more = [0u8; 8];
                    socket.read_exact(&mut more).unwrap();
                    u64::from_be_bytes(more)
                }
                short => u64::from(short),
            };
            let mut payload = vec![0u8; length as usize];
            socket.read_exact(&mut payload).unwrap();
            match head[0] & 0x0f {
                9 => socket.write_all(&[0x8a, 0x80, 0, 0, 0, 0]).unwrap(),
                0..=2 => message.extend(payload),
                _ => {}
            }
            if head[0] & 0x80 != 0 && head[0] & 0x0f != 9 {
                return String::from_utf8(message).expect("text");
            }
        }
    }

    /// A JSON value, as the protocol sends them.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Json {
        Null,
        Bool(bool),
        Number(f64),
        Text(String),
        List(Vec<Json>),
        Object(Vec<(String, Json)>),
    }

    impl Json {
        /// Reads a whole value.
        pub fn parse(text: &str) -> Json {
            let mut reader = Reader { text: text.as_bytes(), at: 0 };
            reader.value()
        }

        /// `text` as a JSON string.
        pub fn quoted(text: &str) -> String {
            let mut quoted = String::from("\"");
            for letter in text.chars() {
                match letter {
                    '"' => quoted.push_str("\\\""),
                    '\\' => quoted.push_str("\\\\"),
                    '\n' => quoted.push_str("\\n"),
                    letter if (letter as u32) < 0x20 => quoted.push_str(&format!("\\u{:04x}", letter as u32)),
                    letter => quoted.push(letter),
                }
            }
            quoted + "\""
        }

        /// A member of an object.
        pub fn get(&self, name: &str) -> Option<&Json> {
            let Json::Object(members) = self else { return None };
            members.iter().find(|(named, _)| named == name).map(|(_, value)| value)
        }

        /// An item of a list (null past its end).
        pub fn at(&self, at: usize) -> &Json {
            match self {
                Json::List(items) => items.get(at).unwrap_or(&Json::Null),
                _ => &Json::Null,
            }
        }

        /// A string's text (empty for anything else).
        pub fn text(&self) -> String {
            match self {
                Json::Text(text) => text.clone(),
                _ => String::new(),
            }
        }

        /// A number (0 for anything else).
        pub fn number(&self) -> f64 {
            match self {
                Json::Number(number) => *number,
                _ => 0.0,
            }
        }

        /// A list of numbers, as whole numbers.
        pub fn numbers(&self) -> Vec<i64> {
            match self {
                Json::List(items) => items.iter().map(|item| item.number() as i64).collect(),
                _ => Vec::new(),
            }
        }
    }

    impl std::fmt::Display for Json {
        /// Written back as JSON.
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Json::Null => write!(f, "null"),
                Json::Bool(yes) => write!(f, "{yes}"),
                Json::Number(number) => write!(f, "{number}"),
                Json::Text(text) => write!(f, "{}", Json::quoted(text)),
                Json::List(items) => write!(f, "[{}]", items.iter().map(Json::to_string).collect::<Vec<_>>().join(",")),
                Json::Object(members) => write!(f, "{{{}}}", members.iter().map(|(name, value)| format!("{}:{value}", Json::quoted(name))).collect::<Vec<_>>().join(",")),
            }
        }
    }

    /// Reads JSON from bytes.
    struct Reader<'a> {
        text: &'a [u8],
        at: usize,
    }

    impl Reader<'_> {
        fn blank(&mut self) {
            while self.text.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
        }

        fn value(&mut self) -> Json {
            self.blank();
            match self.text[self.at] {
                b'{' => self.object(),
                b'[' => self.list(),
                b'"' => Json::Text(self.string()),
                b't' => self.word("true", Json::Bool(true)),
                b'f' => self.word("false", Json::Bool(false)),
                b'n' => self.word("null", Json::Null),
                _ => self.number(),
            }
        }

        fn word(&mut self, word: &str, value: Json) -> Json {
            self.at += word.len();
            value
        }

        fn number(&mut self) -> Json {
            let start = self.at;
            while self.text.get(self.at).is_some_and(|byte| b"+-.eE0123456789".contains(byte)) {
                self.at += 1;
            }
            Json::Number(std::str::from_utf8(&self.text[start..self.at]).unwrap().parse().unwrap())
        }

        fn string(&mut self) -> String {
            let mut bytes = Vec::new();
            self.at += 1;
            while self.text[self.at] != b'"' {
                if self.text[self.at] != b'\\' {
                    bytes.push(self.text[self.at]);
                    self.at += 1;
                    continue;
                }
                self.at += 2;
                match self.text[self.at - 1] {
                    b'n' => bytes.push(b'\n'),
                    b't' => bytes.push(b'\t'),
                    b'r' => bytes.push(b'\r'),
                    b'b' => bytes.push(8),
                    b'f' => bytes.push(12),
                    b'u' => self.escaped(&mut bytes),
                    other => bytes.push(other),
                }
            }
            self.at += 1;
            String::from_utf8(bytes).unwrap()
        }

        /// A `\u` escape (its pair too, for a letter beyond the first plane), as UTF-8.
        fn escaped(&mut self, bytes: &mut Vec<u8>) {
            let unit = |reader: &mut Reader| {
                let hex = std::str::from_utf8(&reader.text[reader.at..reader.at + 4]).unwrap();
                reader.at += 4;
                u32::from_str_radix(hex, 16).unwrap()
            };
            let mut code = unit(self);
            if (0xD800..0xDC00).contains(&code) && self.text.get(self.at) == Some(&b'\\') {
                self.at += 2;
                code = 0x10000 + ((code - 0xD800) << 10) + (unit(self) - 0xDC00);
            }
            let letter = char::from_u32(code).unwrap_or('\u{FFFD}');
            bytes.extend(letter.encode_utf8(&mut [0; 4]).as_bytes());
        }

        fn list(&mut self) -> Json {
            let mut items = Vec::new();
            self.at += 1;
            loop {
                self.blank();
                match self.text[self.at] {
                    b']' => break,
                    b',' => self.at += 1,
                    _ => items.push(self.value()),
                }
            }
            self.at += 1;
            Json::List(items)
        }

        fn object(&mut self) -> Json {
            let mut members = Vec::new();
            self.at += 1;
            loop {
                self.blank();
                match self.text[self.at] {
                    b'}' => break,
                    b',' => self.at += 1,
                    _ => {
                        let name = self.string();
                        self.blank();
                        self.at += 1;
                        members.push((name, self.value()));
                    }
                }
            }
            self.at += 1;
            Json::Object(members)
        }
    }
}
