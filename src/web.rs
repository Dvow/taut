//! taut in a browser: the same app, built for `wasm32-unknown-unknown`, in a page's
//! `<canvas>`. No crates, no bundler: the page loads the program with the loader below.
//!
//! ```text
//! rustup target add wasm32-unknown-unknown
//! cargo build --release --target wasm32-unknown-unknown --example hello
//! cargo run --example web_page -- target/wasm32-unknown-unknown/release/examples/hello.wasm
//! ```
//!
//! The last line writes `index.html` and `taut.js` beside the program and serves the folder
//! at `http://127.0.0.1:8000` (any static server will do: a page cannot load a program from a
//! `file:` address). A page of one's own loads it so:
//!
//! ```html
//! <script type="module">
//!   import { start } from "./taut.js";
//!   start("hello.wasm", { element: document.body, fonts: { regular: "Inter.ttf" } });
//! </script>
//! ```
//!
//! | | On the web |
//! | --- | --- |
//! | Window | The element given (the page by default); [`Window`](crate::Window)'s size and place are not used; its title is the page's. [`run`](crate::run) returns at once. |
//! | Pixels | The CPU's, as everywhere; what changed goes to the canvas (`putImageData`), sharp at the device's pixel ratio. A frame only when something changed. With the feature `gpu`, through WebGL2 (`gpu-draw`: the GPU draws large parts); the loader's `renderer` and `draw` say what `TAUT_RENDERER` and `TAUT_DRAW` say elsewhere. |
//! | Input | Mouse, wheel, touchpad pinch (as Ctrl+wheel), fingers and pens (pointer events; as on other systems, the feature `touch` makes taps and pinches of fingers), keys, focus. |
//! | Typing | A hidden text field takes typed text and input methods; while one composes, the field shows it at the text cursor. |
//! | Text | The browser's (canvas `fillText`): the person's fonts, every script, emoji. Font files given to the loader (`fonts`) are used instead, with the browser's for letters they lack: the same letters as on every system. [`system_fonts::families`](crate::system_fonts::families) lists the person's fonts where the browser does (`queryLocalFonts`). |
//! | Readers | Every part as a hidden element with its ARIA role, name and states, where it is drawn; focus follows; announcements in a live region. |
//! | Clipboard | Text: copied through the browser's copy event, pasted from its paste event (Ctrl+V). Pictures as PNG, both ways; files pasted are read in. A menu's Paste has what was last copied or pasted in the page, or read from the clipboard where the person let the page. |
//! | Drag and drop | Files and text from other programs dropped on the page; text, links, files and pictures dragged out as the browser's drag. |
//! | Saving | [`storage`](crate::storage) keeps files in the page's `localStorage`. |
//! | Files | The system file dialog is the browser's picker: a file, several, or a folder (`picked` and `files_in` have them); saving through the save picker where the browser has one, else a download (`save`, `download`). |
//! | Notifications | The browser's, once the person allows them; clicks are heard. |
//! | Printing | The browser's print dialog, each page a picture at 288 dpi on a sheet of its size. |
//! | One copy | [`single_instance`](crate::single_instance()): a later tab of the program hands its address to the first (Web Locks, `BroadcastChannel`). |
//! | Video | With the feature `video`, `Video` plays through a `<video>` element: the browser's formats; a path is an address relative to the page; sound only after a press on the page. |
//! | Not on the web | Second windows, tray, global hotkeys: a page has none. Files can't be put on the clipboard. Threads: none (`wasm32-unknown-unknown`). |

/// The loader: a JavaScript module that makes the page's canvas, starts a program built for
/// `wasm32-unknown-unknown`, and passes events and frames between them. Serve it beside the
/// program (`taut.js`) and import its `start(program, options)`; see the [module](self) docs.
pub const LOADER: &str = include_str!("web.js");

/// A whole page that runs `program` (a `.wasm` file's address, relative to the page) with the
/// loader served as `taut.js` beside it.
pub fn page(title: &str, program: &str) -> String {
    let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    let (title, program) = (escape(title), escape(program).replace('\\', "\\\\"));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n\
         <style>html, body {{ margin: 0; height: 100%; overflow: hidden; }}</style>\n\
         <script type=\"module\">import {{ start }} from \"./taut.js\"; start(\"{program}\");</script>\n</html>\n"
    )
}

/// The bytes of a file the person handed the page: picked in the browser's file picker (what
/// the file dialog answered, `/picked/<name>`, or inside a folder picked), dropped on the page
/// (`/dropped/<name>`) or pasted into it (`/pasted/<name>`). Kept until another of the same
/// path comes.
#[cfg(target_family = "wasm")]
pub fn picked(path: &std::path::Path) -> Option<Vec<u8>> {
    crate::web_host::picked(path)
}

/// The files the page holds inside `folder`, by path: those of a folder the person picked
/// (what the file dialog answered, `/picked/<folder>`), at any depth.
#[cfg(target_family = "wasm")]
pub fn files_in(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
    crate::web_files::files_in(folder)
}

/// Writes `bytes` where the file dialog saved: the file the person chose in the browser's save
/// picker (`/saved/<name>`, where the browser has one: Chrome and Edge), or as a download
/// (`/downloads/<name>`). The browser writes the chosen file after this returns; an error
/// then goes to the console.
#[cfg(target_family = "wasm")]
pub fn save(file: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Ok(name) = file.strip_prefix("/downloads") {
        crate::web_host::save_as(&name.to_string_lossy(), bytes);
        return Ok(());
    }
    match crate::web_files::save_chosen(file, bytes) {
        true => Ok(()),
        false => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "the file dialog saved nowhere of this name")),
    }
}

/// Has the browser save `bytes` as a download named `name` (into the person's downloads, or
/// where they choose, as their browser does). Call it from a press or a key: browsers may
/// refuse a download nobody asked for.
#[cfg(target_family = "wasm")]
pub fn download(name: &str, bytes: &[u8]) {
    crate::web_host::save_as(name, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_loads_the_program_with_the_loader() {
        let made = page("Notes <1>", "notes.wasm");
        assert!(made.contains("<title>Notes &lt;1></title>") && made.contains("start(\"notes.wasm\")"), "{made}");
        assert!(LOADER.contains("export async function start"), "the loader exports start");
        // Every function the program imports from the loader, the loader has (WebGL2's too).
        let modules = [
            include_str!("web_host.rs"),
            include_str!("web_clipboard.rs"),
            include_str!("web_desktop.rs"),
            include_str!("web_drag_drop.rs"),
            include_str!("web_files.rs"),
            include_str!("web_fonts.rs"),
            include_str!("web_gpu.rs"),
            include_str!("web_print.rs"),
            include_str!("video_web.rs"),
        ];
        for module in modules {
            for block in module.split("unsafe extern \"C\" {").skip(1) {
                let imports = &block[..block.find("\n}").unwrap()];
                for line in imports.lines().filter(|line| line.contains("fn ")) {
                    let name = line.split("fn ").nth(1).unwrap().split('(').next().unwrap();
                    assert!(LOADER.contains(&format!("            {name}: (")), "the loader lacks {name}");
                }
            }
        }
        // And every function the loader calls, the program exports.
        let host = modules[0];
        for called in LOADER.split("\"taut_").skip(1).map(|rest| rest.split('"').next().unwrap()) {
            assert!(host.contains(&format!("pub extern \"C\" fn taut_{called}(")), "the program lacks taut_{called}");
        }
        assert!(LOADER.contains("exports.taut_alloc(") && host.contains("pub extern \"C\" fn taut_alloc("));
    }
}
