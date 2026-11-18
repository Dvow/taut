# taut

[![CI](https://github.com/Dvow/taut/actions/workflows/ci.yml/badge.svg)](https://github.com/Dvow/taut/actions/workflows/ci.yml)

A GUI library for Rust on Windows, Linux and macOS with no dependencies: no crates, no web view, no
graphics card needed. One function describes the window from the app's state; the library draws
only what changed, and nothing while nothing happens.

| Windows 11 | Linux (X11) |
| --- | --- |
| ![The demo window on Windows: a table of five thousand files, sorted by size](docs/windows.png) | ![The demo window on Linux, with its colour picker open](docs/linux.png) |

## A first program

```toml
[dependencies]
taut = "0.2"
```

```rust,no_run
use taut::Window;

fn main() {
    let mut presses = 0;
    let window = Window { title: "Hello", width: 320.0, height: 160.0, ..Default::default() };
    taut::run(window, move |ui| {
        ui.column().pad(16.0).gap(10.0).open();
        ui.label(&format!("Pressed {presses} times"));
        presses += ui.button("Press").clicked as u32;
        ui.close();
    });
}
```

The function runs when something happens. Open a box, describe its content, close it. A part
takes `&mut` to what it changes and returns what the user did.

## Why taut

- **Small:** the program above is 457 KB on Windows, 758 KB on Linux (iced 5.9 MB, Slint 7.8 MB,
  egui 8.3 MB). Only the parts called are compiled in.
- **No dependencies:** 0 crates (iced 159, egui 179, Slint 245). No GPU needed.
- **Fast:** a 1,000-row window is described and redrawn in 172 µs (one frame: Dear ImGui 244 µs,
  egui 6,114 µs). Idle costs nothing.
- **Accessible:** full keyboard use; screen readers through UI Automation, AT-SPI and
  NSAccessibility.
- **Tested:** over 1,100 tests on Windows 11, Ubuntu and Arch Linux, and on macOS 26; every
  interface test checks partial against full redraw, pixel for pixel; every parser of outside
  data is fuzzed.

## At a glance

- **Systems:** Windows; Linux on X11 or Wayland; macOS (AppKit). Not iOS, Android or the web
  yet.
- **Rust:** 1.88+. **Licence:** MIT or Apache-2.0.
- **Features** (all off by default): `right-to-left`, `brahmic`, `emoji`, `fallback-fonts`,
  `touch`, `cff`, `woff2`, `color-profiles`, `gpu`, `gpu-draw`, `translations`
  ([costs](docs/costs-and-speed.md#what-it-costs)).
- **GPU or CPU:** the CPU draws every pixel; with `gpu`, the GPU shows them, byte for byte the
  same: Direct3D 11 or 12, Vulkan or OpenGL on Windows; OpenGL (EGL) or Vulkan under X11;
  Vulkan (dmabufs) under Wayland, when asked ([costs](docs/costs-and-speed.md#gpu-or-cpu)). With
  `gpu-draw`, Direct3D 11 and OpenGL draw them too, within a few levels.
- **State:** 0.2. Names may change before 1.0 ([changes](CHANGELOG.md),
  [what is missing](docs/what-is-missing.md)).

## Examples

`cargo run --example <name>`:

| Example | What it is |
| --- | --- |
| `hello` | the program above |
| `gallery` | most parts, on nine pages (the pictures above) |
| `todo` | a to-do list kept in a file (built in chapter 4 of the book) |
| `file_browser` | a file browser: folder tree, sortable table |
| `editor` | a text editor: tabs, find and replace, menus |
| `settings` | a settings window that follows the system's light or dark |
| `chat` | a message list that stays at its end |
| `viewer` | a viewer for PNG, JPEG, GIF, WebP, BMP, ICO and SVG |
| `dashboard` | tiles, live charts, a table |
| `node_editor` | a node editor on a pan-and-zoom area |
| `report` | a chart and text, saved as PDF or printed |
| `tray` | a tray icon, notifications, a global hotkey |
| `desktop` | a whole desktop drawn into a pixel buffer |
| `picture` | an interface drawn into a PNG, no window |
| `d3d9`, `d3d11`, `d3d12`, `opengl`, `vulkan` | the interface over the program's own renderer |
| `overlay` | the library in a DLL with a C interface |
| `bench` | the [timings](docs/costs-and-speed.md#how-fast-it-is) |

`book`, `walk` and `walk_reader` are the library's own tools (book pictures, demo measurements).

## Inside your own renderer

A layer, made once from your device, draws the interface over each frame and uploads only what
changed ([chapter 9](docs/09-inside-another-program.md)):

| Renderer | Layer | Each frame |
| --- | --- | --- |
| Direct3D 9, 9Ex | `layer_d3d9` | `layer.draw(&mut ui)` |
| Direct3D 11 | `layer_d3d11` | `layer.draw(&mut ui)` |
| Direct3D 12 | `layer_d3d12` | `layer.draw(&mut ui, list, area)` |
| OpenGL 3.3+ | `layer_opengl` | `layer.draw(&mut ui)` |
| Vulkan 1.0+ | `layer_vulkan` | `layer.draw(&mut ui, commands, area)` |
| Metal (macOS) | `layer_metal` | `layer.draw(&mut ui, encoder, area)` |
| wgpu, bgfx, Skia, any other | `Surface::upload` | your `write_texture` of each changed place |

## Beside other Rust libraries

A label and a button, built for size on Windows, 2026-11-15:

| | taut | egui | iced | Slint |
| --- | --- | --- | --- | --- |
| Program | 457 KB | 8.3 MB | 5.9 MB | 7.8 MB |
| Other crates | 0 | 179 | 159 | 245 |
| Drawn by | CPU; shown by Direct3D 11/12, Vulkan, OpenGL or the CPU, same pixels; or drawn by Direct3D 11 or OpenGL (`gpu-draw`) | GPU (wgpu or OpenGL) | GPU (wgpu), or CPU | GPU (OpenGL, Skia), or CPU |
| Screen readers | own (UI Automation, AT-SPI) | AccessKit | none in 0.14 | AccessKit |

[More libraries, and Dear ImGui part by part](docs/beside-other-libraries.md).

## Documentation

- **Start:** [a first window](docs/01-a-first-window.md), then [the book](docs/README.md#the-book)
  (ten chapters).
- **Reference:** [the guide](GUIDE.md) (every call on one page) and [the API](https://docs.rs/taut).
- **Details:** [what is in it](docs/what-is-in-it.md), [what is missing](docs/what-is-missing.md),
  [costs and speed](docs/costs-and-speed.md), [testing](docs/how-it-is-tested.md),
  [inside other programs](docs/inside-other-programs.md),
  [other libraries](docs/beside-other-libraries.md).
- **Project:** [changes](CHANGELOG.md), [design goals](GOALS.md).

## Licence

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
Unicode, HarfBuzz and Brotli data: [THIRD-PARTY.md](THIRD-PARTY.md).

Contributions are taken under the same terms, unless you state otherwise.
