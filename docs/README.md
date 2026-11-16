# taut's documents

Overview: [the front page](../README.md).

## The book

A tutorial in ten chapters.

1. [A first window](01-a-first-window.md): a window with a button, and what happens inside.
2. [Boxes and layout](02-boxes-and-layout.md): rows, columns, grids, sizes, scrolling, layers.
3. [Parts and state](03-parts-and-state.md): who keeps what, ids and keys, custom parts.
4. [A real small app](04-a-real-small-app.md): a to-do list kept in a file.
5. [Larger parts](05-larger-parts.md): tables, lists, trees, menus, dialogs, panels, docks,
   windows, tray, charts, drag and drop.
6. [Text and pictures](06-text-and-pictures.md): fonts, scripts, emoji, pictures, SVG, shapes,
   3D scenes, PDF and printing.
7. [Everyone can use it](07-everyone-can-use-it.md): keyboard, screen readers, themes, sizes,
   contrast.
8. [Testing](08-testing.md): driving a window with no screen in `cargo test`.
9. [Inside another program](09-inside-another-program.md): as a texture, under Direct3D 11 or
   OpenGL, in a DLL, as a whole screen.
10. [Coming from egui or Dear ImGui](10-coming-from-egui-or-dear-imgui.md): what differs, and
    the usual calls side by side.

Every Rust block in the book is compiled by `cargo test`. Each picture is drawn from the code
above it by `examples/book.rs`
(`cargo run -p taut --features emoji --example book -- docs/pictures`), on Windows 11.

## Reference

- [The guide](../GUIDE.md): every call on one page; also on docs.rs as
  [`taut::guide`](https://docs.rs/taut/latest/taut/guide/).
- [The API on docs.rs](https://docs.rs/taut): each item marked with the system and feature it
  needs.

## Details

- [What is in it](what-is-in-it.md): every feature, by area.
- [What is missing](what-is-missing.md): known gaps and limits.
- [Costs and speed](costs-and-speed.md): sizes, memory, feature costs, benchmarks.
- [How it is tested](how-it-is-tested.md).
- [Inside other programs](inside-other-programs.md): renderers and DLLs, in short.
- [Beside other libraries](beside-other-libraries.md): Rust GUI libraries, and Dear ImGui part
  by part.

## Project

- [Changes](../CHANGELOG.md)
- [Design goals](../GOALS.md)
