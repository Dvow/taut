//! Videos on the web. One file, two programs:
//!
//! - Built for `wasm32-unknown-unknown`, it is a page (`page`): a video at the top left, its
//!   controls and caption, buttons that open, seek, loop, add captions and open many, and a line
//!   saying the video's state.
//! - Here (`checks`), it builds that page and runs it under Node (no page: `NoPlayer`) and in a
//!   headless browser (Edge or Chrome; `TAUT_BROWSER` names one), muted, which presses its
//!   parts through the readers' elements and keys and reads the frame numbers off the canvas:
//!   frames at their times, pause, seeks to the frame asked, the end once, a loop, the controls,
//!   the app's captions (shown and told), speed, the browser's own tracks,
//!   bytes, a play with sound refused before a press, a missing file, and many opens.
//!
//! Each half says why and passes where it lacks the wasm target, Node or a browser, unless
//! `TAUT_VIDEO=required`.
#[cfg(not(target_family = "wasm"))]
fn main() {
    checks::all();
}

#[cfg(target_family = "wasm")]
fn main() {
    page::main();
}

#[allow(dead_code)]
mod files {
    include!("video/files.rs");
}

#[cfg(target_family = "wasm")]
mod page {
    use std::time::Duration;
    use taut::{Ui, Video, VideoError, Window};

    /// The page's state: the video, how often it said it ended, what opening said, and the
    /// opens left to make (with whether the one open plays).
    #[derive(Default)]
    struct Page {
        video: Option<Video>,
        ended: u32,
        opened: String,
        many: Option<(u32, bool)>,
    }

    /// The app's captions: one a second, none from 2 s to 2.5 s.
    const NOTES: &str = "WEBVTT\n\n00:00.000 --> 00:01.000\nFirst\n\n00:01.000 --> 00:02.000\nSecond\n\n00:02.500 --> 00:03.000\nThird";

    pub fn main() {
        let mut page = Page::default();
        page.open(Video::open_silent("silent.mp4"));
        taut::run(Window { title: "video checks", ..Default::default() }, move |ui| page.step(ui));
    }

    impl Page {
        fn open(&mut self, made: Result<Video, VideoError>) {
            (self.ended, self.opened) = (0, String::new());
            match made {
                Ok(video) => self.video = Some(video),
                Err(why) => self.opened = why.to_string(),
            }
        }
        fn close(&mut self, ui: &mut Ui) {
            if let Some(video) = self.video.take() {
                ui.drop_video(video);
            }
        }

        fn step(&mut self, ui: &mut Ui) {
            self.many(ui);
            ui.column().open();
            if let Some(video) = &mut self.video {
                ui.video(video).show();
                ui.video_controls(video);
                ui.video_captions(video);
                self.ended += video.ended() as u32;
            }
            ui.row().gap(4.0).open();
            for (name, to) in [("Seek 0.5", 0.5), ("Seek 1.2", 1.2), ("Seek 2.0", 2.0), ("Seek 2.5", 2.5), ("Seek 2.9", 2.9)] {
                if let (true, Some(video)) = (ui.button(name).clicked, &mut self.video) {
                    video.seek(Duration::from_secs_f32(to));
                }
            }
            if let (true, Some(video)) = (ui.button("Loop").clicked, &mut self.video) {
                video.set_looping(true);
            }
            if let (true, Some(video)) = (ui.button("Notes").clicked, &mut self.video) {
                let notes = video.add_captions(NOTES, "en", "Notes").expect("WebVTT");
                video.set_caption_track(Some(notes));
            }
            ui.close();
            ui.row().gap(4.0).open();
            let asked = ["Silent", "With sound", "Bytes", "Missing", "Many 5", "Many 40", "Languages"].map(|name| ui.button(name).clicked);
            ui.close();
            ui.label(&self.state());
            ui.close();
            self.asked(asked, ui);
        }

        fn asked(&mut self, [silent, sound, bytes, missing, five, forty, languages]: [bool; 7], ui: &mut Ui) {
            if silent || sound || bytes || missing || five || forty || languages {
                self.close(ui);
            }
            match () {
                _ if silent => self.open(Video::open_silent("silent.mp4")),
                _ if sound => self.open(Video::open("with-sound.mp4")),
                _ if bytes => self.open(Video::open_silent(super::files::mp4(30, 10, false))),
                _ if missing => self.open(Video::open_silent("missing.mp4")),
                _ if languages => self.open(Video::open_silent("languages.mp4")),
                _ if five || forty => {
                    self.open(Video::open_silent("with-sound.mp4"));
                    self.many = Some((if five { 5 } else { 40 }, false));
                }
                _ => {}
            }
        }

        /// Opens many in turn: each until its first frame, then plays a tenth of a second.
        fn many(&mut self, ui: &mut Ui) {
            let Some((left, playing)) = self.many else { return };
            let ready = self.video.as_ref().is_some_and(|video| video.size().is_some());
            let played = self.video.as_ref().is_some_and(|video| video.position() >= Duration::from_millis(100));
            match (ready, playing) {
                (true, false) => {
                    if let Some(video) = &mut self.video {
                        video.play();
                    }
                    self.many = Some((left, true));
                }
                (true, true) if played => {
                    self.close(ui);
                    self.open(Video::open_silent("with-sound.mp4"));
                    self.many = (left > 1).then_some((left - 1, false));
                }
                _ => {}
            }
            ui.wake_after(Duration::from_millis(10));
        }

        /// One line the checks read: `name=value` words. (While many open, only how many are
        /// left and whether one has a frame: each new line drawn is kept a while.)
        fn state(&self) -> String {
            let Some(video) = &self.video else { return format!("state opened={}", self.opened) };
            if let Some((left, _)) = self.many {
                return format!("state many={left} size={}", if video.size().is_some() { "64x48" } else { "none" });
            }
            let length = video.length().map_or(-1.0, |length| length.as_secs_f32());
            let size = video.size().map_or("none".into(), |(width, height)| format!("{width}x{height}"));
            format!(
                "state playing={} position={:.3} length={length:.3} size={size} frames={} ended={} muted={} volume={:.2} speed={:.2} tracks={}/{} caption={} many={} error={}",
                video.playing() as u8,
                video.position().as_secs_f32(),
                video.frames_shown(),
                self.ended,
                video.muted() as u8,
                video.volume(),
                video.speed(),
                video.sound_tracks().len(),
                video.caption_tracks().len(),
                video.caption().unwrap_or("none"),
                self.many.map_or(0, |(left, _)| left),
                video.error().unwrap_or("none"),
            )
        }
    }
}

#[cfg(not(target_family = "wasm"))]
#[allow(dead_code)]
#[path = "../examples/web_page.rs"]
mod web_page;

#[cfg(not(target_family = "wasm"))]
mod checks {
    use super::files::{TWO_CAPTIONS, TWO_LANGUAGES, movie, mp4, written};
    use std::io::ErrorKind;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    pub fn all() {
        let Some(program) = page() else { return };
        under_node(&program);
        in_a_browser(&program);
        println!("video on the web: every check passed");
    }

    /// Whether to go on: `false`, said, where `there` is not, unless `TAUT_VIDEO=required`.
    fn skipped(there: bool, missing: &str) -> bool {
        let required = std::env::var("TAUT_VIDEO").as_deref() == Ok("required");
        assert!(there || !required, "{missing}");
        if !there {
            println!("{missing}: skipped");
        }
        there
    }

    /// This file built for the web, as a page's program; none without the wasm target.
    fn page() -> Option<PathBuf> {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("video-web");
        let mut build = Command::new(cargo);
        build.current_dir(env!("CARGO_MANIFEST_DIR")).args(["build", "--release", "--test", "video_web", "--features", "video", "--target", "wasm32-unknown-unknown", "--message-format=json"]);
        let made = build.env("CARGO_TARGET_DIR", &target).output().expect("cargo runs");
        // (With JSON messages the compiler's errors come on standard output.)
        let (out, said) = (String::from_utf8_lossy(&made.stdout), String::from_utf8_lossy(&made.stderr));
        let missing = ["may not be installed", "can't find crate for `std`", "can't find crate for `core`"].iter().any(|told| out.contains(told) || said.contains(told));
        if !made.status.success() && !skipped(!missing, "no wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)") {
            return None;
        }
        assert!(made.status.success(), "the page builds for the web:\n{said}");
        let line = out.lines().find(|line| line.contains("\"executable\":\"") && line.contains("\"name\":\"video_web\""));
        let program = line.and_then(|line| line.split("\"executable\":\"").nth(1)?.split('"').next()).expect("a program built");
        Some(PathBuf::from(program.replace("\\\\", "\\")))
    }

    /// A program on the path, if it runs.
    fn on_path(name: &str) -> Option<PathBuf> {
        let runs = Command::new(name).arg("--version").output().is_ok_and(|ran| ran.status.success());
        runs.then(|| PathBuf::from(name))
    }

    /// A folder of the check's own, emptied, with the loader, the page's program and the
    /// video files.
    fn work(name: &str, program: &Path) -> PathBuf {
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("video-web-checks").join(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("taut.js"), taut::web::LOADER).unwrap();
        std::fs::copy(program, folder.join("page.wasm")).unwrap();
        for (name, sound) in [("silent.mp4", false), ("with-sound.mp4", true)] {
            std::fs::copy(written(name, &mp4(30, 10, sound)), folder.join(name)).unwrap();
        }
        std::fs::write(folder.join("languages.mp4"), movie(30, 10, false, &TWO_LANGUAGES, &TWO_CAPTIONS)).unwrap();
        folder
    }

    /// Under Node, with no page: opening says there is no player.
    const UNDER_NODE: &str = r#"
import { start } from "./taut.js";
import { readFileSync, writeFileSync } from "node:fs";
const page = await start(readFileSync("page.wasm"), { element: null });
page.resize(400, 300, 1);
page.frame();
writeFileSync("told.txt", [...page.told.values()].map((node) => node.n + node.t).join("\n"));
"#;

    fn under_node(program: &Path) {
        let Some(node) = on_path("node") else {
            skipped(false, "no Node");
            return;
        };
        let folder = work("node", program);
        std::fs::write(folder.join("run.mjs"), UNDER_NODE).unwrap();
        let ran = Command::new(node).current_dir(&folder).arg("run.mjs").output().expect("node runs");
        assert!(ran.status.success(), "node: {}", String::from_utf8_lossy(&ran.stderr));
        let told = std::fs::read_to_string(folder.join("told.txt")).unwrap();
        assert!(told.contains("state opened=no video player: no page to play in"), "under Node: {told}");
        println!("under Node: no player, said so");
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
        found.or_else(|| ["google-chrome", "google-chrome-stable", "microsoft-edge"].into_iter().find_map(on_path))
    }

    const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<style>html, body { margin: 0; background: #808080; } #app { width: 700px; height: 400px; }</style>
<div id="app"></div>
<script type="module" src="checks.js"></script>
"#;

    /// The checks, in the page: what failed, and what was seen, posted as `name=JSON` lines.
    const CHECKS: &str = r##"
import { start } from "./taut.js";
const results = { failed: [] };
const fail = (what) => results.failed.push(what);
const post = (path, body) => fetch(path, { method: "POST", body });
const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
// Blob addresses made and not yet freed.
let blobs = 0;
const [make, free] = [URL.createObjectURL, URL.revokeObjectURL];
URL.createObjectURL = (blob) => (blobs++, make(blob));
URL.revokeObjectURL = (address) => (blobs--, free(address));

let page;
async function settle() {
    for (let i = 0; i < 3; i++) await frame();
    for (let i = 0; i < 100 && page.frameAsked; i++) await frame();
}
const all = (role) => [...page.readers.querySelectorAll(role ? `[role="${role}"]` : "[data-key]")];
const named = (role, name) => all(role).find((made) => made.getAttribute("aria-label") === name);
// The state line, as numbers where they are.
function state() {
    const line = all().map((made) => made.textContent).find((text) => text.startsWith("state ")) || "";
    const words = Object.fromEntries(line.slice(6).split(" error=")[0].split(" ").map((word) => word.split("=")));
    for (const [name, value] of Object.entries(words)) if (value !== "" && !isNaN(value)) words[name] = Number(value);
    words.error = line.includes(" error=") ? line.split(" error=")[1] : "none";
    words.line = line;
    return words;
}
// The frame number the canvas shows at the top left (the cells' middles, bright or dark).
function shown() {
    const seen = page.context.getImageData(0, 0, 64, 48).data;
    let number = 0;
    for (let cell = 0; cell < 8; cell++) {
        const at = ((Math.floor(cell / 4) * 24 + 12) * 64 + (cell % 4) * 16 + 8) * 4;
        number |= (seen[at] + seen[at + 1] + seen[at + 2] > 384 ? 1 : 0) << cell;
    }
    return number;
}
async function press(name, role = "button") {
    const made = named(role, name);
    if (!made) return fail(`no ${role} ${name}: ${all().map((made) => made.getAttribute("aria-label")).join(", ")}`);
    made.click();
    await settle();
}
async function key(name, code = name) {
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: name, code, bubbles: true, cancelable: true }));
    document.activeElement.dispatchEvent(new KeyboardEvent("keyup", { key: name, code, bubbles: true, cancelable: true }));
    await settle();
}
// Waits up to `seconds` for the state to hold `done`.
async function until(what, done, seconds = 10) {
    const begun = performance.now();
    while (!done(state())) {
        if (performance.now() - begun > seconds * 1000) return fail(`${what} did not happen: ${state().line}`), false;
        await pause(20);
        await settle();
    }
    return true;
}
async function opened(button) {
    await press(button);
    return until(`the first frame of ${button}`, (now) => now.size === "64x48" && now.frames > 0);
}
async function seek(button, frame) {
    const before = state().frames;
    await press(button);
    await until(`the frame after ${button}`, (now) => now.frames > before);
    await pause(100);
    await settle();
    if (shown() !== frame) fail(`after ${button}: frame ${shown()}, not ${frame}`);
}

async function frames_at_their_times() {
    if (!(await opened("Silent"))) return;
    const first = state();
    results.first = [first.size, shown(), first.length];
    if (shown() !== 0 || Math.abs(first.length - 3) > 0.05) fail(`first: frame ${shown()}, length ${first.length}`);
    await press("Play");
    const seen = [];
    while (state().playing && seen.length < 100) {
        await pause(50);
        await settle();
        const [number, at] = [shown(), state().position];
        seen.push([number, at]);
        if (!(at > number / 10 - 0.08 && at < number / 10 + 0.18)) fail(`frame ${number} shown at ${at} s`);
    }
    results.seen = seen;
    const numbers = seen.map(([number]) => number);
    if (!numbers.every((number, at) => at === 0 || numbers[at - 1] <= number) || numbers[numbers.length - 1] < 25) fail(`frames shown: ${numbers}`);
}

async function pause_holds_and_seeks_land() {
    if (!(await opened("Silent"))) return;
    await press("Play");
    await until("a second played", (now) => now.position >= 1);
    await press("Pause");
    await pause(200);
    await settle();
    const [held, at] = [shown(), state().position];
    await pause(400);
    await settle();
    results.held = [held, at, shown(), state().position];
    if (shown() !== held || state().position !== at) fail(`paused: ${results.held}`);
    if (Math.abs(held - at * 10) > 1.5) fail(`paused at ${at} on frame ${held}`);
    for (const [button, frame] of [["Seek 2.0", 20], ["Seek 0.5", 5], ["Seek 2.9", 29]]) await seek(button, frame);
}

async function the_end_once_and_a_loop() {
    if (!(await opened("Silent"))) return;
    await seek("Seek 2.5", 25);
    await press("Play");
    await until("the end", (now) => !now.playing);
    await pause(300);
    await settle();
    results.end = [state().ended, shown()];
    if (state().ended !== 1 || shown() !== 29) fail(`the end: told ${state().ended} times, frame ${shown()}`);
    await press("Loop");
    await press("Play");
    const frames = state().frames;
    await until("playing from the start", (now) => now.position < 0.5 && now.frames > frames);
    await pause(300);
    await settle();
    results.looped = state().line;
    if (!state().playing || state().ended !== 1 || shown() >= 10) fail(`looped: ${state().line}, frame ${shown()}`);
}

async function the_controls() {
    if (!(await opened("Silent"))) return;
    for (const [role, name] of [["button", "Play"], ["slider", "Position"], ["slider", "Volume"]]) if (!named(role, name)) fail(`no ${role} ${name}`);
    results.position_max = named("slider", "Position")?.getAttribute("aria-valuemax");
    if (Number(results.position_max) !== 3) fail(`position's most: ${results.position_max}`);
    await press("Play");
    if (!state().playing || !named("button", "Pause")) fail(`Play: ${state().line}`);
    await press("Pause");
    if (state().playing) fail(`Pause: ${state().line}`);
    named("button", "Play").focus();
    await settle();
    const frames = state().frames;
    await key("Tab");
    await key("End");
    await until("the frame after End", (now) => now.frames > frames);
    await pause(100);
    await settle();
    if (shown() !== 29) fail(`End: frame ${shown()}`);
    // Past Speed to Mute.
    await key("Tab");
    await key("Tab");
    await key(" ", "Space");
    if (state().muted !== 0) fail(`Space on Mute: ${state().line}`);
    await key("Tab");
    await key("ArrowLeft");
    if (Math.abs(state().volume - 0.95) > 0.001) fail(`Left on Volume: ${state().line}`);
    results.controls = state().line;
}

async function bytes_and_errors() {
    if (await opened("Bytes")) await seek("Seek 1.2", 12);
    // Sound before any press: refused, said, and nothing hangs; muted, the same video plays.
    if (await opened("With sound")) {
        await press("Play");
        await until("a refused play", (now) => now.error !== "none");
        results.refused = state().line;
        if (state().playing || !state().error.includes("press")) fail(`a play with sound: ${state().line}`);
        named("button", "Play").focus();
        await key("Tab");
        await key("Tab");
        await key("Tab");
        await key(" ", "Space");
        await press("Play");
        await until("a muted play", (now) => now.playing && now.position > 0.3);
        if (state().error !== "none") fail(`a muted play: ${state().line}`);
    }
    await press("Missing");
    await until("the error of a missing file", (now) => now.error !== "none");
    results.missing = state().line;
}

async function captions_and_speed() {
    if (!(await opened("Silent"))) return;
    await press("Notes");
    await seek("Seek 1.2", 12);
    results.caption = state().caption;
    if (state().caption !== "Second") fail(`the caption at 1.2 s: ${state().line}`);
    if (!all().some((made) => made.textContent === "Second")) fail("the caption is not told to readers");
    await seek("Seek 2.0", 20);
    if (state().caption !== "none") fail(`no caption at 2.0 s: ${state().line}`);
    await press("Speed");
    await press("2×", "menuitemradio");
    if (state().speed !== 2) fail(`the speed chosen: ${state().line}`);
    await seek("Seek 0.5", 5);
    await press("Play");
    await until("playing", (now) => now.position >= 0.6);
    const [from, begun] = [state().position, performance.now()];
    await pause(600);
    await settle();
    results.rate = (state().position - from) / ((performance.now() - begun) / 1000);
    if (Math.abs(results.rate - 2) > 0.4) fail(`at 2× the time moved ${results.rate} s a second`);
}

// The browser's own tracks of a file with two sound and two caption tracks: what it lists.
async function the_files_tracks() {
    if (!(await opened("Languages"))) return;
    await pause(300);
    await settle();
    results.tracks = state().tracks;
}

async function many_opens() {
    const held = () => {
        if (typeof gc === "function") gc();
        return [page.exports.memory.buffer.byteLength, performance.memory?.usedJSHeapSize || 0, page.videos.size, blobs];
    };
    const many = async (button) => {
        await press(button);
        await until(button, (now) => now.many === 0 && now.size === "64x48", 60);
        await pause(300);
        return held();
    };
    await many("Many 5");
    const before = await many("Many 40");
    const after = await many("Many 40");
    results.many = [before, after];
    if (after[0] > before[0] + (4 << 20) || after[1] > before[1] + (8 << 20) || after[2] !== 1 || after[3] !== 0) fail(`many opens: before ${before}, after ${after}`);
}

try {
    page = await start("page.wasm", { element: document.getElementById("app") });
    await settle();
    for (const check of [frames_at_their_times, pause_holds_and_seeks_land, the_end_once_and_a_loop, the_controls, bytes_and_errors, captions_and_speed, the_files_tracks, many_opens]) {
        const failed = results.failed.length;
        await check();
        results[check.name] = results.failed.length === failed ? "passed" : "FAILED";
    }
} catch (error) {
    fail(String(error.stack || error));
}
await post("/done", Object.entries(results).map(([name, value]) => `${name}=${JSON.stringify(value)}`).join("\n"));
"##;

    fn in_a_browser(program: &Path) {
        let Some(browser) = browser() else {
            skipped(false, "no browser (TAUT_BROWSER names one)");
            return;
        };
        let folder = work("browser", program);
        std::fs::write(folder.join("index.html"), PAGE).unwrap();
        std::fs::write(folder.join("checks.js"), CHECKS).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://127.0.0.1:{}/", listener.local_addr().unwrap().port());
        let mut started = Command::new(&browser);
        started.args(["--headless=new", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-sync", "--mute-audio", "--hide-scrollbars"]);
        started.args(["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--window-size=800,500", "--force-device-scale-factor=1"]);
        // (Sound only after a person's press, as for most visitors; `gc()` for the memory check.)
        started.args(["--autoplay-policy=document-user-activation-required", "--js-flags=--expose-gc", "--enable-precise-memory-info"]);
        started.arg(format!("--user-data-dir={}", folder.join("profile").display())).arg(&address).stderr(std::process::Stdio::null());
        let mut running = started.spawn().expect("the browser starts");
        let done = served(&listener, &folder);
        // Only the browser started here is stopped, by its own handle.
        let _ = running.kill();
        let _ = running.wait();
        let done = done.expect("the page reported within four minutes");
        println!("in {}:\n{done}", browser.display());
        assert!(done.lines().any(|line| line == "failed=[]"), "the checks in the browser failed:\n{done}");
    }

    /// Serves the folder, each request on a thread of its own (a browser may open a connection
    /// and send nothing on it for a while), until the page posts `/done`: what it posted.
    fn served(listener: &TcpListener, folder: &Path) -> Option<String> {
        let (posts, posted) = std::sync::mpsc::channel();
        let since = Instant::now();
        while since.elapsed() < Duration::from_secs(240) {
            if let Ok((path, body)) = posted.try_recv()
                && path == "/done"
            {
                return String::from_utf8(body).ok();
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let (posts, folder) = (posts.clone(), folder.to_path_buf());
                    std::thread::spawn(move || super::web_page::answer(stream, &folder, &mut |path, body| drop(posts.send((path.to_string(), body)))));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("{error}"),
            }
        }
        None
    }
}
