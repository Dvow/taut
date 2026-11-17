# Design goals

What taut aims to be, and how each aim is checked.

## Goals

| Goal | Means | Checked by |
| --- | --- | --- |
| **General** | Nothing is made for one particular app. General means (boxes, shapes, pointer and keys on any box) come before special parts. | Reading the change: a part only one app could use does not go in. |
| **Portable** | The same interface code everywhere; only the window, text and clipboard differ by system. Runs in its own window, as a texture under any renderer, or in a DLL. | Tests on Windows, Ubuntu and Arch Linux; `tests/dll.rs` drives the DLL; every interface test draws into a `Surface`. |
| **Light** | Zero dependencies. No frame loop. A program carries only what it calls; what can't be told from the calls is a feature, off by default. Outside data is bounded by its cost, not just its count. | `examples/walk` measures memory, size and idle CPU (zero); [costs](docs/costs-and-speed.md#what-it-costs). |
| **Fast** | In every category (describe, lay out, redraw, full draw, event to pixels, idle) at least as fast as Dear ImGui at every window size. | `examples/bench` before and after every core change; [speed](docs/costs-and-speed.md#how-fast-it-is). |
| **Complete** | What Dear ImGui and egui can do, taut can do. A feature works by keyboard and pointer, at every scale, on every system, with and without its feature flag, and is tested. | [Beside Dear ImGui](docs/beside-other-libraries.md#beside-dear-imgui), [what is missing](docs/what-is-missing.md), `tests/accessibility.rs`. |
| **Easy** | One function describes the window. A part is one call; it takes `&mut` to what it changes and returns what the user did. | `GUIDE.md` covers every call; its examples compile in `cargo test`. |
| **Flat** | Apps open and close boxes instead of nesting closures. The library has no recursion, no line indented past 16 spaces, and no folders in `src`. | `tests/flat.rs`. |
| **Clean** | One file per thing. No dead code, no second way to do the same thing, no wrappers that only rename. | `cargo clippy` with no warnings; `cargo fmt`. |
| **Proven** | Nothing is claimed until it is seen working. | After every test step, the partial redraw is compared with a full redraw; demo runs on all three systems. |

## Design decisions

- No dependencies. MIT or Apache-2.0.
- Rust 1.88 is the minimum (`rust-version`); raising it is noted in the changelog.
- State belongs to the app. The library keeps only what must outlast a run: scroll offsets,
  cursors, focus, open menus.
- Text is drawn by the system's engine (GDI, FreeType), or from font files the app supplies.
- Small letters of the library's own fonts snap to whole pixels vertically, as FreeType's light
  hinting does; never horizontally.
- Every public type implements `Debug`; value types are `#[must_use]`; growing enums and
  `Response` are `#[non_exhaustive]`.
- Files from other programs are read, never memory-mapped. Writes to pipes never raise
  SIGPIPE.
- macOS, iOS and Android come after Windows and Linux are complete; nothing may rule them out.

## Contributing

A change is ready when:

1. It adds no dependency.
2. It is flat: `cargo test --test flat` passes.
3. It has tests through the harness in `tests/common`, so its redraw is checked at every step.
4. `cargo fmt` and `cargo clippy --all-targets` are clean (every public item documented).
5. All tests pass on Windows, Ubuntu and Arch Linux, with no features and with all. CI runs them
   on Windows and Ubuntu (`.github/workflows/ci.yml`).
6. Core changes are benchmarked before and after.
7. `GUIDE.md` shows it in a compiled example; [costs](docs/costs-and-speed.md) and
   [what is missing](docs/what-is-missing.md) are updated.

## What is left

- Back ends for macOS, Android and iOS.
- The rest of [what is missing](docs/what-is-missing.md).
