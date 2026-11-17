# How taut is tested

Testing an app's window: [chapter 8](08-testing.md).

```sh
cargo test -p taut
cargo test -p taut --all-features
```

| Tests | What they check |
| --- | --- |
| unit and doc tests | the library, and every code block in the README, guide and book |
| `tests/interface.rs`, `tests/larger_widgets.rs` | click, type, drag and scroll a window with no screen; after every step the partial redraw must equal a full redraw, pixel for pixel; the layout-unchanged shortcut must agree with a full comparison; drags describe at most twice per move |
| `tests/drawings.rs`, `tests/animations.rs` | SVG drawings and animations read from files |
| `tests/logic.rs` | fixed core logic bugs stay fixed |
| `tests/accessibility.rs` | every part by keyboard alone, and as a screen reader is told of it |
| `tests/book.rs` | each book picture is drawn by `examples/book.rs` from its chapter's code, letter for letter |
| `tests/app_requests.rs` | behaviour apps rely on: keys that show their effect at once, page scrolling, programmatic scrolling, variable-height lists, app Edit menus, pictures fitted to their room |
| `tests/fuzzed*.rs`, `tests/drawing_costs.rs` | inputs found by coverage-guided fuzzing of every parser of outside data (pictures, fonts, documents, saved layouts, Wayland messages) and of the example apps under random input; each failed before its fix |
| `tests/dll.rs` | loads the `overlay` example as a DLL and drives it through its C functions |
| `tests/flat.rs` | no library line indented past 16 spaces; no folders in `src` |

Every public item is documented; the compiler warns otherwise.

On Linux, about thirty more tests (real clipboard via `xclip`, window placement, minimize,
full screen, transparency, input methods, the system file dialog, drag between programs,
global hotkeys) run on request, on their own display:

```sh
xvfb-run cargo test -p taut --lib -- --include-ignored --skip how_long --skip made_afresh
```

- `how_long` is a measurement, run alone. `made_afresh` regenerates `tests/pictures` with ffmpeg,
  vips, giflib and rsvg-convert.
- Wayland tests open real windows on KWin, Mutter, Sway or Weston (headless or nested in Xvfb),
  using each compositor's fake input (KWin fake input, Mutter remote desktop, wlroots virtual
  pointer, keyboard and input method, XTest) and screen capture. One compares the gallery's
  window in a screenshot with its own pixels. Without a compositor they check nothing.

Measurements:

- Times: `examples/bench.rs`.
- Memory, and a picture after each step: `cargo run -p taut --example walk --release` (on Linux
  under `xvfb-run`) drives the demo with simulated input.
- Sizes: the release profile in [costs](costs-and-speed.md#what-it-costs).

Everything runs on Windows 11, Ubuntu and Arch Linux. [Design goals](../GOALS.md).
