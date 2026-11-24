//! taut in a browser: programs built for `wasm32-unknown-unknown`, run by the loader
//! (`taut::web::LOADER`).
//!
//! - Under Node, with no page: `hello`'s frames are the very pixels the same interface draws
//!   here, before and after a press through the loader's calls.
//! - In a headless browser (Edge or Chrome; `TAUT_BROWSER` names one), with a profile folder of
//!   its own: the gallery's first page is the frame drawn here at the same size, at one and two
//!   device pixels a unit; parts are pressed by mouse, finger and pen, typed into (an input
//!   method too), scrolled, copied and pasted through dispatched DOM events; readers' elements
//!   say what the interface tells. The notes example keeps its note in `localStorage`, opens a
//!   picked file, saves as a download and opens a link.
//!
//! Both sides draw text with the same font file: the system's (Segoe UI, DejaVu Sans), or
//! `TAUT_FONT`. Each test says why and passes where it lacks the wasm target
//! (`rustup target add wasm32-unknown-unknown`), Node, a browser or a font. With
//! `TAUT_PICTURES` naming a folder, frames that differ are saved there.
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use taut::geom::{Size, pt};
use taut::{AppFonts, Button, ContrastColors, Event, FontFile, LocalClipboard, Mods, Preferences, Surface, Ui};

#[allow(dead_code)]
#[path = "../examples/web_page.rs"]
mod web_page;

#[allow(dead_code, unused_attributes)]
#[path = "../examples/gallery.rs"]
mod gallery;

/// The examples built for the web, in their folder; or why they could not be.
fn built() -> Result<&'static Path, &'static str> {
    static BUILT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let built = BUILT.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("web");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let examples = ["--example", "hello", "--example", "gallery", "--example", "web_notes"];
        // With the features these tests have: the pages are held against what is drawn here.
        let features = [
            ("right-to-left", cfg!(feature = "right-to-left")),
            ("brahmic", cfg!(feature = "brahmic")),
            ("shaping", cfg!(feature = "shaping")),
            ("touch", cfg!(feature = "touch")),
            ("emoji", cfg!(feature = "emoji")),
            ("fallback-fonts", cfg!(feature = "fallback-fonts")),
            ("cff", cfg!(feature = "cff")),
            ("hinting", cfg!(feature = "hinting")),
            ("color-profiles", cfg!(feature = "color-profiles")),
            ("woff2", cfg!(feature = "woff2")),
            ("translations", cfg!(feature = "translations")),
            ("gpu", cfg!(feature = "gpu")),
            ("gpu-draw", cfg!(feature = "gpu-draw")),
            ("video", cfg!(feature = "video")),
            ("hdr", cfg!(feature = "hdr")),
        ];
        let features: Vec<&str> = features.iter().filter(|(_, on)| *on).map(|(name, _)| *name).collect();
        let mut build = Command::new(cargo);
        build.current_dir(env!("CARGO_MANIFEST_DIR")).args(["build", "--release", "--target", "wasm32-unknown-unknown"]).args(examples).env("CARGO_TARGET_DIR", &target);
        if !features.is_empty() {
            build.args(["--features", &features.join(",")]);
        }
        let made = build.output().expect("cargo runs");
        let said = String::from_utf8_lossy(&made.stderr);
        if !made.status.success() && (said.contains("may not be installed") || said.contains("can't find crate for `std`") || said.contains("can't find crate for `core`")) {
            return Err("no wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)".to_string());
        }
        assert!(made.status.success(), "the examples build for the web:\n{said}");
        Ok(target.join("wasm32-unknown-unknown").join("release").join("examples"))
    });
    built.as_ref().map(PathBuf::as_path).map_err(String::as_str)
}

/// A program on the path, if it runs.
fn on_path(name: &str) -> Option<PathBuf> {
    let runs = Command::new(name).arg("--version").output().is_ok_and(|ran| ran.status.success());
    runs.then(|| PathBuf::from(name))
}

/// A browser to run the pages in: `TAUT_BROWSER`, else Edge or Chrome where they are usually
/// installed.
fn browser() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("TAUT_BROWSER") {
        return Some(named.into());
    }
    let mut places: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let edge = Path::new(r"C:\Program Files (x86)\Microsoft\EdgeCore");
        let versions = std::fs::read_dir(edge).into_iter().flatten().flatten().map(|version| version.path().join("msedge.exe"));
        places.extend(versions);
        places.extend([r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Google\Chrome\Application\chrome.exe"].map(PathBuf::from));
    }
    let found = places.into_iter().find(|place| place.exists());
    found.or_else(|| ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"].into_iter().find_map(on_path))
}

/// A font file both sides draw with: `TAUT_FONT`, else the system's usual one.
fn font() -> Option<Vec<u8>> {
    let named = std::env::var_os("TAUT_FONT").map(PathBuf::from);
    let windows = std::env::var_os("WINDIR").map(|folder| Path::new(&folder).join("Fonts").join("segoeui.ttf"));
    let linux = ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/dejavu/DejaVuSans.ttf"].map(PathBuf::from);
    named.into_iter().chain(windows).chain(linux).find_map(|file| std::fs::read(file).ok())
}

/// What every test needs, or why it is skipped.
fn needs(tool: Option<PathBuf>, what: &str) -> Option<(PathBuf, &'static Path, Vec<u8>)> {
    let Some(tool) = tool else {
        eprintln!("skipped: no {what}");
        return None;
    };
    let Some(font) = font() else {
        eprintln!("skipped: no font file (TAUT_FONT names one)");
        return None;
    };
    match built() {
        Ok(examples) => Some((tool, examples, font)),
        Err(why) => {
            eprintln!("skipped: {why}");
            None
        }
    }
}

/// A folder of the test's own, emptied.
fn work(name: &str) -> PathBuf {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("web-checks").join(name);
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// An interface drawing text with `font`, as the loader's page does when given it for the
/// regular and mono faces.
fn interface(font: &[u8], preferences: Preferences) -> Ui {
    let file = || FontFile::read(font.to_vec()).expect("a font");
    let mut ui = Ui::new(Box::new(AppFonts::new(file()).mono(file())), Box::new(LocalClipboard::default()));
    ui.set_screen_reader(true);
    ui.set_preferences(preferences);
    ui
}

/// Runs `describe` until it settles, at this size, and draws it whole.
fn frame(ui: &mut Ui, size: [f32; 3], describe: &mut dyn FnMut(&mut Ui)) -> Surface {
    ui.resize(Size { w: size[0], h: size[1] }, size[2]);
    for _ in 0..8 {
        ui.run(describe);
    }
    Surface::draw(ui, 64)
}

/// How many pixels of the page's (red, green, blue, alpha) differ from the surface's; with
/// `TAUT_PICTURES`, both are saved when some do.
fn differing(page: &[u8], surface: &Surface, name: &str) -> usize {
    assert_eq!(page.len(), surface.pixels.len() * 4, "{name}: the page's frame is the surface's size ({}x{})", surface.width, surface.height);
    let theirs: Vec<u32> = page.as_chunks::<4>().0.iter().map(|p| u32::from(p[3]) << 24 | u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])).collect();
    let differ = theirs.iter().zip(&surface.pixels).filter(|(a, b)| a != b).count();
    if let Some(folder) = std::env::var_os("TAUT_PICTURES").filter(|_| differ > 0) {
        let folder = Path::new(&folder);
        std::fs::write(folder.join(format!("{name}-page.png")), taut::png::encode(surface.width, &theirs).unwrap()).unwrap();
        std::fs::write(folder.join(format!("{name}-here.png")), surface.to_png()).unwrap();
    }
    differ
}

/// `hello`'s interface, as its `main` describes it.
fn hello(presses: &mut u32, ui: &mut Ui) {
    ui.column().pad(16.0).gap(10.0).open();
    ui.label(&format!("Pressed {presses} times"));
    *presses += ui.button("Press").clicked as u32;
    ui.close();
}

/// Starts `hello` with no page, draws a frame, presses its button where the readers' node
/// says it is, draws again; writes both frames and the nodes' names.
const UNDER_NODE: &str = r#"
import { start } from "./taut.mjs";
import { readFileSync, writeFileSync } from "node:fs";
const [program, font, out] = process.argv.slice(2);
const bytes = readFileSync(font);
const page = await start(readFileSync(program), { element: null, fonts: { regular: bytes, mono: bytes } });
page.resize(320, 160, 1);
page.frame();
writeFileSync(`${out}/before.rgba`, page.pixels);
const button = [...page.told.values()].find((node) => node.r === "button" && node.n === "Press");
const [x, y] = [button.x + button.w / 2, button.y + button.h / 2];
for (const did of [0, 1, 2]) {
    page.call("taut_pointer", did, 0, 1, 1, x, y, 0, did === 1 ? 1 : 0, 0, 0, 0, 0, 0);
}
page.frame();
writeFileSync(`${out}/after.rgba`, page.pixels);
writeFileSync(`${out}/told.txt`, [...page.told.values()].map((node) => `${node.r}:${node.n}${node.t}`).join("\n") + `\nat ${x} ${y}`);
"#;

#[test]
fn hello_under_node_draws_what_it_draws_here_and_hears_a_press() {
    let Some((node, examples, font)) = needs(on_path("node"), "Node") else { return };
    let folder = work("node");
    std::fs::write(folder.join("taut.mjs"), taut::web::LOADER).unwrap();
    std::fs::write(folder.join("run.mjs"), UNDER_NODE).unwrap();
    std::fs::write(folder.join("font.ttf"), &font).unwrap();
    let ran = Command::new(node).current_dir(&folder).args(["run.mjs"]).arg(examples.join("hello.wasm")).args(["font.ttf", "."]).output().expect("node runs");
    assert!(ran.status.success(), "node: {}", String::from_utf8_lossy(&ran.stderr));
    let told = std::fs::read_to_string(folder.join("told.txt")).unwrap();
    assert!(told.contains(":Pressed 1 times") && told.contains("button:Press"), "{told}");
    // The same interface here, pressed at the same place.
    let mut ui = interface(&font, Preferences::default());
    let mut presses = 0;
    let before = frame(&mut ui, [320.0, 160.0, 1.0], &mut |ui| hello(&mut presses, ui));
    assert_eq!(differing(&std::fs::read(folder.join("before.rgba")).unwrap(), &before, "hello-before"), 0, "hello's first frame under Node");
    let at: Vec<f32> = told.lines().last().unwrap().split(' ').skip(1).map(|n| n.parse().unwrap()).collect();
    for event in [Event::Move(pt(at[0], at[1])), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
        ui.event(event);
    }
    let after = frame(&mut ui, [320.0, 160.0, 1.0], &mut |ui| hello(&mut presses, ui));
    assert_eq!(presses, 1);
    assert_eq!(differing(&std::fs::read(folder.join("after.rgba")).unwrap(), &after, "hello-after"), 0, "hello's frame after the press");
}

// ---- in a browser ----

/// The page the checks run in: an element of 900 by 600 CSS pixels.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<style>html, body { margin: 0; background: #808080; } #app { width: 900px; height: 600px; }</style>
<div id="app"></div>
<script type="module" src="checks.js"></script>
"#;

/// The checks, run in the page: each result posted as a line `name=JSON`, frames as their
/// own posts.
const CHECKS: &str = r##"
import { start } from "./taut.js";
const asked = new URLSearchParams(location.search);
const results = {};
const errors = [];
const told = console.error;
console.error = (...said) => {
    errors.push(said.join(" "));
    told(...said);
};
addEventListener("error", (event) => errors.push(String(event.error?.stack || event.message)));
const post = (path, body) => fetch(path, { method: "POST", body });
const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
async function settle(page) {
    for (let i = 0; i < 4; i++) await frame();
    for (let i = 0; i < 100 && page.frameAsked; i++) await frame();
}
const all = (page, role) => [...page.readers.querySelectorAll(role ? `[role="${role}"]` : "[data-key]")];
const named = (page, role, name) => all(page, role).find((made) => made.getAttribute("aria-label") === name);
function middle(made) {
    const place = made.getBoundingClientRect();
    return [place.left + place.width / 2, place.top + place.height / 2];
}
function point(page, type, [x, y], kind = "mouse") {
    const down = type === "pointerdown";
    const options = { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: kind === "mouse" ? 1 : 7, pointerType: kind, isPrimary: true };
    page.canvas.dispatchEvent(new PointerEvent(type, { ...options, button: type === "pointermove" ? -1 : 0, buttons: down ? 1 : 0, pressure: down ? 0.5 : 0 }));
}
async function press(page, made, kind = "mouse") {
    const at = middle(made);
    for (const type of ["pointermove", "pointerdown", "pointerup"]) point(page, type, at, kind);
    await settle(page);
}
function key(target, type, name, code, mods = {}) {
    const event = new KeyboardEvent(type, { key: name, code, bubbles: true, cancelable: true, ...mods });
    target.dispatchEvent(event);
    return event.defaultPrevented;
}
async function pixels(page, path) {
    const { width, height } = page.canvas;
    await post(path, page.context.getImageData(0, 0, width, height).data);
}

async function gallery(page) {
    results.title = document.title;
    results.size = [page.canvas.width, page.canvas.height];
    const asks = (query) => (matchMedia(query).matches ? 1 : 0);
    results.looks = [asks("(prefers-color-scheme: dark)"), asks("(prefers-contrast: more)"), asks("(prefers-reduced-motion: reduce)")];
    await pixels(page, "/first");
    results.roles = [...new Set(all(page).map((made) => made.getAttribute("role") || "text"))].sort();
    // The page's list scrolls under the wheel: what is told of it moves up.
    const tops = new Map(all(page).map((made) => [made, made.getBoundingClientRect().top]));
    page.canvas.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true, clientX: 450, clientY: 400, deltaY: 120, deltaMode: 0 }));
    await settle(page);
    results.scrolled = [...tops].filter(([made, top]) => !made.isConnected || made.getBoundingClientRect().top < top).length;
    // Tab moves the keyboard to the first part, and the reader's focus with it.
    page.box.focus();
    results.tab_kept = key(document.activeElement, "keydown", "Tab", "Tab");
    await settle(page);
    results.focused = [document.activeElement.dataset.key !== undefined, document.activeElement.getAttribute("role")];
    // Pages chosen by the mouse, a finger and a pen.
    const tabs = all(page, "tab").map((made) => made.getAttribute("aria-label"));
    results.tabs = tabs;
    for (const [name, kind] of [["Table", "touch"], ["Parts", "pen"], ["More", "mouse"]]) {
        await press(page, named(page, "tab", name), kind);
        results[`chosen_${kind}`] = named(page, "tab", name)?.getAttribute("aria-selected");
    }
    // A text box: pointed at, pressed, typed into, composed in, copied from, pasted into.
    const box = () => all(page, "textbox")[0];
    results.box = box()?.getAttribute("aria-label");
    point(page, "pointermove", middle(box()));
    await settle(page);
    results.cursor = page.canvas.style.cursor;
    await press(page, box());
    results.field_has_keyboard = document.activeElement === page.field;
    const field = page.field;
    field.dispatchEvent(new InputEvent("input", { data: "abc", inputType: "insertText", bubbles: true }));
    await settle(page);
    results.typed = box().textContent;
    field.dispatchEvent(new CompositionEvent("compositionstart", { data: "", bubbles: true }));
    field.dispatchEvent(new CompositionEvent("compositionupdate", { data: "にほ", bubbles: true }));
    results.composing = [page.compose.style.display, page.compose.textContent];
    field.dispatchEvent(new CompositionEvent("compositionend", { data: "日本", bubbles: true }));
    await settle(page);
    results.composed = [box().textContent, page.compose.style.display];
    results.select_all_kept = key(field, "keydown", "a", "KeyA", { ctrlKey: true });
    await settle(page);
    results.copy_kept = key(field, "keydown", "c", "KeyC", { ctrlKey: true });
    const copied = new DataTransfer();
    field.dispatchEvent(new ClipboardEvent("copy", { clipboardData: copied, bubbles: true, cancelable: true }));
    results.copied = copied.getData("text/plain");
    key(field, "keydown", "End", "End");
    await settle(page);
    key(field, "keydown", "v", "KeyV", { ctrlKey: true });
    const pasted = new DataTransfer();
    pasted.setData("text/plain", "XYZ");
    field.dispatchEvent(new ClipboardEvent("paste", { clipboardData: pasted, bubbles: true, cancelable: true }));
    await settle(page);
    results.pasted = box().textContent;
    // A phone's keyboard deletes through the field's input, with no key of its own.
    field.dispatchEvent(new InputEvent("input", { inputType: "deleteContentBackward", bubbles: true }));
    await settle(page);
    results.deleted = box().textContent;
    // What a frame costs to show: `putImageData` against a WebGL texture upload and draw.
    results.show_ms = showing(page.canvas.width, page.canvas.height);
    // A smaller page: the canvas follows, in whole device pixels.
    document.getElementById("app").style.width = "700px";
    await pause(100);
    await settle(page);
    results.resized = [page.canvas.width, page.size[0]];
}

// What the canvas shows inside an element's place: how many pixels differ from its first,
// and how many are strongly coloured (red, green and blue far apart).
function ink(page, made) {
    const place = made.getBoundingClientRect();
    const scale = devicePixelRatio;
    const seen = page.context.getImageData(Math.round(place.left * scale), Math.round(place.top * scale), Math.round(place.width * scale), Math.round(place.height * scale)).data;
    let [inked, coloured] = [0, 0];
    for (let at = 0; at < seen.length; at += 4) {
        inked += seen[at] !== seen[0] || seen[at + 1] !== seen[1] || seen[at + 2] !== seen[2] ? 1 : 0;
        coloured += Math.max(seen[at], seen[at + 1], seen[at + 2]) - Math.min(seen[at], seen[at + 1], seen[at + 2]) > 120 ? 1 : 0;
    }
    return [inked, coloured];
}

// Milliseconds to show a whole frame of this size, by `putImageData` and by WebGL (a texture
// upload and a draw), each the middle of 15, waited for (a pixel read back).
function showing(width, height) {
    const pixels = new Uint8ClampedArray(width * height * 4).map((_, at) => at * 7);
    const middle = (times) => times.sort((a, b) => a - b)[times.length >> 1];
    const timed = (show, wait) => middle(Array.from({ length: 15 }, () => {
        const begun = performance.now();
        show();
        wait();
        return performance.now() - begun;
    }));
    const two = Object.assign(document.createElement("canvas"), { width, height }).getContext("2d", { alpha: false });
    const image = new ImageData(pixels, width, height);
    const put = timed(() => two.putImageData(image, 0, 0), () => two.getImageData(0, 0, 1, 1));
    const gl = Object.assign(document.createElement("canvas"), { width, height }).getContext("webgl2");
    if (!gl) return [put, null, null];
    const shader = (kind, text) => {
        const made = gl.createShader(kind);
        gl.shaderSource(made, text);
        gl.compileShader(made);
        return made;
    };
    const program = gl.createProgram();
    gl.attachShader(program, shader(gl.VERTEX_SHADER, "#version 300 es\nout vec2 at; void main() { at = vec2(gl_VertexID & 1, gl_VertexID >> 1); gl_Position = vec4(at * 2.0 - 1.0, 0, 1); }"));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, "#version 300 es\nprecision mediump float; in vec2 at; uniform sampler2D page; out vec4 colour; void main() { colour = texture(page, at); }"));
    gl.linkProgram(program);
    gl.useProgram(program);
    gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, width, height);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    const read = new Uint8Array(4);
    const shown = () => {
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    };
    const webgl = timed(shown, () => gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, read));
    return [put, webgl, gl.getParameter(gl.RENDERER)];
}

async function notes(page) {
    const box = () => all(page, "textbox")[0];
    results.kept_before = box().textContent;
    // The browser's own text: a label in ink, an emoji typed in colour.
    results.label_ink = ink(page, all(page).find((made) => made.textContent === "A note, kept between visits"));
    results.box_before = ink(page, box());
    // (Whether the browser itself draws emoji in colour: not without a colour emoji font.)
    const probe = Object.assign(document.createElement("canvas"), { width: 40, height: 40 }).getContext("2d");
    probe.font = page.font("normal 400 30px\nsans");
    probe.fillText("😀", 2, 32);
    const drawn = probe.getImageData(0, 0, 40, 40).data;
    results.browser_colours = drawn.some((_, at) => at % 4 === 0 && Math.max(drawn[at], drawn[at + 1], drawn[at + 2]) - Math.min(drawn[at], drawn[at + 1], drawn[at + 2]) > 120);
    await press(page, box());
    page.field.dispatchEvent(new InputEvent("input", { data: "!😀", inputType: "insertText", bubbles: true }));
    await settle(page);
    results.box_after = ink(page, box());
    await press(page, named(page, "button", "Keep"));
    results.stored = localStorage.getItem("/storage/taut-notes/note.txt");
    await pause(120);
    results.announced = page.live.textContent;
    page.askFile = (accept) => {
        results.accept = accept;
        page.call("taut_file", "picked.txt", new TextEncoder().encode("picked text"));
    };
    await press(page, named(page, "button", "Open a file\u2026"));
    await settle(page);
    results.opened = box().textContent;
    page.download = (name, bytes) => (results.downloaded = [name, new TextDecoder().decode(bytes)]);
    await press(page, named(page, "button", "Save as\u2026"));
    await settle(page);
    window.open = (link) => {
        results.link = link;
        return null;
    };
    await press(page, all(page, "link")[0]);
    localStorage.clear();
}

try {
    const program = asked.get("program");
    if (program === "web_notes") localStorage.setItem("/storage/taut-notes/note.txt", "from before");
    // The gallery with the page's font; the notes in the browser's own.
    const font = program === "gallery" ? { regular: "font.ttf", mono: "font.ttf" } : {};
    const begun = performance.now();
    const page = await start(`${program}.wasm`, { element: document.getElementById("app"), fonts: font });
    await settle(page);
    results.started_ms = Math.round(performance.now() - begun);
    results.loaded_ms = Math.round(performance.now());
    await (program === "gallery" ? gallery(page) : notes(page));
} catch (error) {
    results.failed = String(error.stack || error);
}
results.errors = errors;
await post("/done", Object.entries(results).map(([name, value]) => `${name}=${JSON.stringify(value)}`).join("\n"));
"##;

/// Runs the checks on `program` in the browser at `scale` device pixels a unit: what was
/// posted, by path, and the results by name (each as JSON).
fn in_browser(browser: &Path, examples: &Path, font: &[u8], program: &str, scale: u32) -> (HashMap<String, Vec<u8>>, HashMap<String, String>) {
    let folder = work(&format!("{program}-{scale}"));
    for (name, text) in [("index.html", PAGE), ("checks.js", CHECKS), ("taut.js", taut::web::LOADER)] {
        std::fs::write(folder.join(name), text).unwrap();
    }
    std::fs::write(folder.join("font.ttf"), font).unwrap();
    std::fs::copy(examples.join(format!("{program}.wasm")), folder.join(format!("{program}.wasm"))).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://127.0.0.1:{}/?program={program}", listener.local_addr().unwrap().port());
    let profile = folder.join("profile");
    let mut started = Command::new(browser);
    started.args(["--headless=new", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-sync", "--mute-audio", "--hide-scrollbars"]);
    started.args(["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--window-size=1000,700", &format!("--force-device-scale-factor={scale}")]);
    started.arg(format!("--user-data-dir={}", profile.display())).arg(&address).stderr(std::process::Stdio::null());
    let mut browser = started.spawn().expect("the browser starts");
    let (mut posted, since) = (HashMap::new(), Instant::now());
    while !posted.contains_key("/done") && since.elapsed() < Duration::from_secs(180) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                web_page::answer(stream, &folder, &mut |path, body| drop(posted.insert(path.to_string(), body)));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("{error}"),
        }
    }
    // Only the browser started here is stopped, by its own handle.
    let _ = browser.kill();
    let _ = browser.wait();
    let done = String::from_utf8(posted.remove("/done").expect("the page reported within three minutes")).unwrap();
    let results = done.lines().filter_map(|line| line.split_once('=')).map(|(name, value)| (name.to_string(), value.to_string())).collect();
    (posted, results)
}

/// The preferences the page had, as `[dark, contrast, still]`.
fn looks(results: &HashMap<String, String>) -> Preferences {
    let [dark, contrast, still] = ["[1,", ",1,", ",1]"].map(|said| results["looks"].contains(said));
    let high_contrast = contrast.then_some(if dark { ContrastColors::DARK } else { ContrastColors::LIGHT });
    Preferences { dark, high_contrast, reduced_motion: still, ..Preferences::default() }
}

#[test]
fn the_gallery_in_a_browser_is_drawn_as_here_and_heard_through_the_dom() {
    let Some((browser, examples, font)) = needs(browser(), "browser (TAUT_BROWSER names one)") else { return };
    for scale in [1, 2] {
        let (posted, results) = in_browser(&browser, examples, &font, "gallery", scale);
        let said = |name: &str| results.get(name).map_or("(none)", String::as_str);
        assert_eq!(said("size"), format!("[{},{}]", 900 * scale, 600 * scale), "{results:#?}");
        // The first page, as drawn here at the same size and scale.
        let mut app = gallery::app();
        let mut ui = interface(&font, looks(&results));
        let here = frame(&mut ui, [900.0, 600.0, scale as f32], &mut |ui| gallery::describe(&mut app, ui));
        assert_eq!(differing(&posted["/first"], &here, &format!("gallery-{scale}")), 0, "the gallery's first page at scale {scale}: {results:#?}");
        assert_eq!([said("failed"), said("errors")], ["(none)", "[]"], "{results:#?}");
        assert_eq!(said("title"), "\"taut gallery\"");
        if scale == 2 {
            continue;
        }
        // Readers hear parts by their roles; the wheel scrolls; Tab moves the keyboard.
        for role in ["\"button\"", "\"textbox\"", "\"tab\"", "\"menubar\""] {
            assert!(said("roles").contains(role), "{role} in {}", said("roles"));
        }
        assert!(said("scrolled").parse::<u32>().is_ok_and(|moved| moved > 3), "{}", said("scrolled"));
        assert_eq!(said("tab_kept"), "true");
        assert!(said("focused").starts_with("[true,"), "{results:#?}");
        // Pages chosen by finger, pen and mouse.
        for kind in ["touch", "pen", "mouse"] {
            assert_eq!(said(&format!("chosen_{kind}")), "\"true\"", "{kind}: {results:#?}");
        }
        // Typing, an input method, copy and paste.
        assert_eq!(said("cursor"), "\"text\"");
        assert_eq!(said("field_has_keyboard"), "true");
        assert!(said("typed").contains("abc"), "{}", said("typed"));
        assert_eq!(said("composing"), "[\"block\",\"にほ\"]");
        assert!(said("composed").contains("abc日本") && said("composed").ends_with(",\"none\"]"), "{}", said("composed"));
        assert_eq!([said("select_all_kept"), said("copy_kept")], ["true", "false"]);
        assert!(said("copied").contains("abc日本"), "{}", said("copied"));
        assert!(said("pasted").contains("abc日本XYZ"), "{}", said("pasted"));
        assert_eq!(said("deleted"), said("pasted").replace("XYZ", "XY"));
        assert_eq!(said("resized"), "[700,700]");
        eprintln!("gallery: started in {} ms; a whole frame shown, putImageData then WebGL (ms, renderer): {}", said("started_ms"), said("show_ms"));
    }
}

#[test]
fn a_page_keeps_opens_and_saves_through_the_browser() {
    let Some((browser, examples, font)) = needs(browser(), "browser (TAUT_BROWSER names one)") else { return };
    let (_, results) = in_browser(&browser, examples, &font, "web_notes", 1);
    let said = |name: &str| results.get(name).map_or("(none)", String::as_str);
    assert_eq!([said("failed"), said("errors")], ["(none)", "[]"], "{results:#?}");
    assert_eq!(said("kept_before"), "\"from before\"");
    assert_eq!(said("stored"), "\"from before!😀\"");
    // The browser drew the label, and the emoji typed in colour.
    let counts = |name: &str| -> Vec<u32> { said(name).trim_matches(['[', ']']).split(',').map(|n| n.parse().unwrap()).collect() };
    assert!(counts("label_ink")[0] > 100, "{results:#?}");
    if said("browser_colours") == "true" {
        assert!(counts("box_after")[1] > counts("box_before")[1] + 20, "{results:#?}");
    } else {
        eprintln!("not checked: the browser draws no emoji in colour (no colour emoji font)");
    }
    eprintln!("notes: loaded in {} ms, started in {} ms", said("loaded_ms"), said("started_ms"));
    assert_eq!(said("announced"), "\"Kept\"");
    assert_eq!([said("accept"), said("opened")], ["\"\"", "\"picked text\""], "{results:#?}");
    assert_eq!(said("downloaded"), "[\"note.txt\",\"picked text\"]", "{results:#?}");
    assert_eq!(said("link"), "\"https://github.com/Dvow/taut\"", "{results:#?}");
}
