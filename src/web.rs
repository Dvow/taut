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
//! | Pixels | The CPU's, as everywhere; what changed goes to the canvas (`putImageData`), sharp at the device's pixel ratio. A frame only when something changed. |
//! | Input | Mouse, wheel, touchpad pinch (as Ctrl+wheel), fingers and pens (pointer events; as on other systems, the feature `touch` makes taps and pinches of fingers), keys, focus. |
//! | Typing | A hidden text field takes typed text and input methods; while one composes, the field shows it at the text cursor. |
//! | Text | The browser's (canvas `fillText`): the person's fonts, every script, emoji. Font files given to the loader (`fonts`) are used instead, with the browser's for letters they lack: the same letters as on every system. |
//! | Readers | Every part as a hidden element with its ARIA role, name and states, where it is drawn; focus follows; announcements in a live region. |
//! | Clipboard | Text: copied through the browser's copy event, pasted from its paste event (Ctrl+V). A menu's Paste has what was last copied or pasted in the page. |
//! | Saving | [`storage`](crate::storage) keeps files in the page's `localStorage`. |
//! | Files | The system file dialog is the browser's file picker (`picked` has the bytes, on the web); saving downloads (`download`). |
//! | Video | With the feature `video`, `Video` plays through a `<video>` element: the browser's formats; a path is an address relative to the page; sound only after a press on the page. |
//! | Not on the web | Second windows, tray, hotkeys, system notices, drag and drop with other programs, printing, one running copy: there is no browser equivalent, and the calls do nothing. Threads: none (`wasm32-unknown-unknown`). |

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

/// The bytes of a file the person picked in the browser's file picker: `path` is what the file
/// dialog answered (`/picked/<name>`). Kept until another file of the same name is picked.
#[cfg(target_family = "wasm")]
pub fn picked(path: &std::path::Path) -> Option<Vec<u8>> {
    crate::web_host::picked(path)
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
        // Every function the program imports from the loader, the loader has.
        let host = include_str!("web_host.rs");
        let imports = &host[host.find("unsafe extern \"C\" {").unwrap()..];
        let imports = &imports[..imports.find("\n}").unwrap()];
        for line in imports.lines().filter(|line| line.contains("fn ")) {
            let name = line.split("fn ").nth(1).unwrap().split('(').next().unwrap();
            assert!(LOADER.contains(&format!("            {name}: (")), "the loader lacks {name}");
        }
        // And every function the loader calls, the program exports.
        for called in LOADER.split("\"taut_").skip(1).map(|rest| rest.split('"').next().unwrap()) {
            assert!(host.contains(&format!("pub extern \"C\" fn taut_{called}(")), "the program lacks taut_{called}");
        }
        assert!(LOADER.contains("exports.taut_alloc(") && host.contains("pub extern \"C\" fn taut_alloc("));
    }
}
