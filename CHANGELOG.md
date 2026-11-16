# Changes

Changes an app can notice, newest first. Before 1.0, a new minor version (0.1 → 0.2) may need
code changes; a new patch version (0.1.0 → 0.1.1) never does.

## 0.1.0 (2026-11-16)

First release.

- **Layout:** rows, columns, grids, layers, wrapping rows, absolute placement; boxes that
  scroll, give way when room runs short, stay in view (`kept`) or stick (`sticky`); line gaps,
  stripes, opacity, hidden boxes; long lists that cost only the rows in view.
- **Parts:** buttons (with icons or pictures), tick boxes, switches, radio buttons, sliders,
  numbers, text boxes, select boxes, menus and a menu bar, tool bars, tabs, trees, tables, a
  colour picker, dates and times, dialogs, a file dialog, notices, charts, and web/mobile parts
  (breadcrumbs, steps, tags, ratings, cards, banners).
- **Larger parts:** floating and docking panels, pan-and-zoom boards, node graphs, a code box,
  an inspector, rich text and Markdown.
- **Text:** system fonts or the app's own (TrueType, OpenType, WOFF, WOFF2), shaped as HarfBuzz
  shapes them, vertical text; right-to-left, Indic and South-East Asian scripts, and colour
  emoji behind features; spell checking.
- **Drawing:** shapes, pens, gradients; PNG, JPEG, GIF, WebP, BMP, ICO and SVG read; animated
  GIF, PNG and WebP; 3D meshes; PNG, JPEG and ICO written; PDF files and printing.
- **Windows:** any number, several screens, frameless and transparent windows, the clipboard,
  drag and drop in and out, tray, notifications, global hotkeys, single instance.
- **Access:** full keyboard use; screen readers through UI Automation and AT-SPI; touch, pens
  and pinches; themes, high contrast and system settings followed.
- **Embedding and tests:** `Surface` draws into a pixel buffer for any renderer or a DLL;
  layers for Direct3D 11 and OpenGL; `testing::Trial` runs an app's window with no screen.
- **Features**, all off by default: `right-to-left`, `brahmic`, `touch`, `emoji`,
  `fallback-fonts`, `cff`, `woff2`, `color-profiles`.
- **API conventions:** every public type implements `Debug`; value types are `#[must_use]`;
  enums an app matches on, and `Response`, are `#[non_exhaustive]`. Structs built with
  `..Default::default()` may gain fields.
- **Requires** Rust 1.88. Building for macOS, iOS or the web stops with an error saying so.
