# Beside other libraries

Measurements: [costs and speed](costs-and-speed.md).

## Beside other Rust libraries

| | taut | egui | iced | Slint | Dioxus | gtk-rs (gtk4) | Tauri |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Model | one function from app state, run on events | immediate mode | Elm: state, messages, update, view | `.slint` markup | React-like components | GTK widgets | a web page talking to Rust |
| Drawn by | the CPU; shown by Direct3D 11/12, Vulkan, OpenGL or the system window | the GPU (wgpu or OpenGL) | wgpu, or tiny-skia on the CPU | femtovg (OpenGL), Skia, or the CPU | the system web view | GTK | the system web view |
| Systems | Windows, Linux | Windows, macOS, Linux, web, Android | Windows, macOS, Linux, web | Windows, macOS, Linux, Android, iOS, embedded, web | Windows, macOS, Linux, iOS, Android, web | where GTK 4 is installed | Windows, macOS, Linux, iOS, Android |
| Screen readers | own: UI Automation, AT-SPI | AccessKit | none in 0.14 | AccessKit | the web view's | GTK's | the web view's |
| Needs besides Rust | nothing | nothing | nothing | nothing | WebKitGTK dev files on Linux | GTK 4 dev files | C++ build tools and WebView2; WebKitGTK 4.1 dev files on Linux |
| Crates in hello | 0 | 179 | 159 | 245 | not measured | not measured | not measured |
| Hello, size-optimized, Windows | 457 KB | 8.3 MB | 5.9 MB | 7.8 MB | "<3 MB" (its README) | not measured | "<600 KB" (its site) |
| Licence | MIT or Apache-2.0 | MIT or Apache-2.0 | MIT | GPL-3.0, royalty-free, or commercial | MIT or Apache-2.0 | MIT (GTK: LGPL) | MIT or Apache-2.0 |
| Minimum Rust | 1.88 | 1.95 | 1.88 | 1.92 | 1.83 | 1.92 | 1.90 |
| Version | 0.1 | "still in flux" | "experimental" | 1.18 | 0.7 | 0.11 | 2.12 |

Same program and release profile ([costs](costs-and-speed.md#what-it-costs)), 2026-11-15; crates
counted with `cargo tree`; descriptions from each project's documents that day.

## Beside Dear ImGui

| | Dear ImGui | taut |
| --- | --- | --- |
| Buttons, tick boxes, radio buttons, sliders, progress bars, password box | yes | yes |
| Dragged numbers | yes | yes, one value at a time |
| Typed numbers with steps | yes | yes, with range, unit, wheel and validation |
| Text boxes, single and multi-line | yes | yes, undo by word |
| Calendar, date range, time | no | yes, in the system's format |
| Drop-downs, tabs, folding headings, trees, selectable rows | yes | yes |
| Filterable drop-down, suggestions | built by the app | built in |
| Tooltips, popups, dialogs, context menus, menu bar, submenus | yes | yes |
| Tables: sort, resize, reorder, hide columns | yes | yes |
| Tables: pinned columns and rows | first ones | first columns, top and bottom rows |
| Multiple system windows | docking branch | yes |
| Inner windows, docking | yes | yes |
| Colour pickers, line and bar plots, custom drawing | yes | yes |
| Charts with axes, legend and readout | ImPlot, separately | built in, read to screen readers |
| Drag and drop; files from other programs | yes | yes |
| Spell checking | no | yes, the system's checker |
| Virtualized long lists, disabled parts | yes | yes |
| Keyboard | yes, and gamepads | yes, including spatial arrows |
| Screen readers | no | yes, Windows and Linux |
| Fonts | own fonts, icon fonts | own fonts, icon fonts, system fonts |
| Picture files | the app brings a loader | reads PNG, JPEG, GIF, WebP, BMP, ICO, SVG; writes PNG, JPEG, ICO |
| Automatic layout (rows, columns, grids, wrapping) | by hand | yes |
| GPU and app-configured renderer | needed | not needed |
| Under the program's own renderer | as triangles | as one texture |
| Redraws only changes; idle when idle | no | yes |
| Redescribe a 7,000-box window | 244 µs | 160 µs, plus 12 µs to redraw |
| Built-in animation | no | yes |
