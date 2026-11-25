# What is in taut

Every feature, by area. Gaps: [what is missing](what-is-missing.md). Calls: the
[guide](../GUIDE.md).

## Layout

- Rows, columns, grids, layers, wrapping rows; fit, fixed and shared sizes; padding, gaps, line
  gaps, alignment, offsets.
- Wrapping, aligned text; "…" when clipped.
- Boxes that shrink in priority order when room runs short (`.gives_way`).
- Scrolling both ways (wheel, keys, bars that never cover content); rows kept in view
  (`.kept()`); sticky headings (`.sticky()`); the page scrolls when taller than the window.
- Programmatic scrolling, stay-at-end for chats and logs, scroll into view; `ui.available_size` to fit
  content in the same run.

## Parts

- **Text and buttons:** labels, paragraphs, mixed-style text with links, icons, buttons, toggle
  buttons, buttons with icons or pictures.
- **Values:** tick boxes, switches, radio buttons, segmented choices; sliders (two knobs,
  stepped, curved, with value); numbers dragged or typed, with range, unit and validation.
- **Input:** text boxes, password box, select box, filterable combo box, suggestions, colour
  picker, date, date range, time, date and time.
- **Navigation:** tabs (closable, reorderable), folds, trees (lazy for huge trees), selectable
  rows with multi-select.
- **Overlays:** file dialog, tooltips, dialogs, command palette, toasts, popovers, splitters.
- **Web and mobile:** breadcrumbs, steps, pagination, tags and tag input, ratings, search box,
  split button, avatar, card, banner, shortcut labels, loading placeholders.
- **Status:** progress bars, spinners, sparklines.
- Any part can be disabled.

## Charts

Lines (straight or smooth without overshoot), areas, bars, points, pies, rings, stacked bars,
scatter; numeric, named or time axes; legend toggles; hover readout; wheel zoom and drag pan;
keyboard stepping read to screen readers. Live charts redraw only changed strips.

## Menus and tool bars

Menus from a button, at the pointer (Shift+F10, menu key) or in a menu bar; submenus; icons,
checks, radio dots, right-aligned shortcuts; `&` mnemonics with Alt; Alt or F10 reach the bar;
menus flip to fit and scroll when long. Tool bars with groups, toggles and overflow menu.
Popovers anchored to their button.

## Text boxes

- Keyboard and pointer selection by character, word and line; clipboard; undo and redo by word.
- Password box: dots only, no copy, no word jumps, only dots exposed to readers.
- Multi-line boxes that fill their room, or leave Enter to the app (Shift+Enter: new line).
- App Edit menus; readable and settable cursor and selection (with line and column).
- Spell checking with the system's checker and user dictionary (Windows; Enchant on Linux;
  `NSSpellChecker` on macOS), visible words only, on a background thread; suggestions, "Add to
  dictionary", "Ignore"; several languages per box (`Some("en-US,de-DE")`); words written twice
  ("the the").
  About 30 KB, only when used.

## Tables and long lists

- Sort by heading (Shift: several columns); resize, reorder, hide columns.
- Horizontal scrolling with pinned columns; pinned top and bottom rows.
- One Tab stop with a cell cursor; keyboard row selection; in-place editing (text, number,
  choice); Ctrl+C copies rows as spreadsheet text; Ctrl+V pastes it into editable cells;
  Ctrl+Z and Ctrl+Y undo and redo edits and pastes (`ui.cell_changes`).
- Lists of any length cost only visible rows, including rows of varying height.

## Panels, docks, pan and zoom, graphs

- **Panels:** float, move, resize, fold, close, raise; snap to edges and each other; keyboard
  move and resize. `ui.pin` fixes a box to a window edge.
- **Docks:** split, stacked and tabbed; drag tabs to drop markers; float and re-dock; reorder,
  close, maximise; Ctrl+Tab, Ctrl+F6, Ctrl+F4. The layout is a plain value.
- **Pan and zoom:** own coordinate space; pan by drag, wheel and bars; zoom by Ctrl+wheel, pinch and
  keys. Only visible content is described, drawn at its zoomed size.
- **Node graphs:** typed ports, inputs for one wire or several, app parts in nodes, wiring with
  type checks, rubber-band selection, copy and delete, grid snap, frames that carry their nodes,
  bends on wires, undo and redo, raise on pick, panning while dragging at an edge, minimap.
  Wires are items readers hear and keys reach.
  1,000 nodes: 60 µs to redescribe and redraw, about 330 µs with all visible.

## Code, inspector, rich text

- **Code box:** syntax colouring from data (`Syntax`: Rust, C, Python, JavaScript, custom),
  multiple cursors, undo, folding; cost is the visible lines.
- **Inspector:** an app value as name/editor rows; folding sections, editable lists, filter,
  changed marks, reset, undo, several values at once.
- **Rich text:** styled pieces with links, icons and pictures in one wrapping paragraph.
- **Markdown:** CommonMark plus GitHub tables, task lists and strikethrough; highlighted code,
  app pictures; selectable, copyable, keyboard links, heading links scroll; a document to screen
  readers; cost is the visible part.

## Fonts and text

- **System fonts** by name and style; family lists; system fallback for missing letters
  (Windows: feature `fallback-fonts`, else linked fonts; Linux: Fontconfig).
- **App fonts:** TrueType, OpenType, WOFF, WOFF2 (`woff2`), CFF and CFF2 (`cff`), collections,
  variable fonts at any axis position; same output on every system.
- **Hinting:** small Latin, Greek and Cyrillic snap vertically to whole pixels, as FreeType's
  light hinting. With `hinting`, a TrueType font's own instructions do it, any script, as
  FreeType's interpreter (version 40): the same points for every glyph of 19 Windows and Linux
  fonts at 8–20 px. `AppFonts::unhinted` turns hinting off; `AppFonts::autohinted` keeps the
  autohinter.
- **Emoji** (`emoji`): colour emoji in all text, sequences as one glyph and one cursor step;
  from the system's font or the app's. `ui.emoji_in_full()` adds `COLR` v1 and SVG glyphs
  (about 30 KB with fonts already read; 360 KB in the smallest program).
- **Right to left** (`right-to-left`): full Unicode bidi (UAX #9), Arabic and Syriac joining,
  marks placed as HarfBuzz does, visual cursor, split where directions meet.
- **Indic and South-East Asian** (`brahmic`): all of HarfBuzz's Indic and Universal Shaping
  Engine scripts; grapheme clusters kept whole by cursor and line breaks. Matches HarfBuzz on
  263,205 words in 46 fonts.
- **Shaped app fonts** (`shaping`): ligatures, contextual forms, stacked marks, kerning beyond
  pairs, as on pages, in whole pixels; matches HarfBuzz on 1,657 word-font pairs. The system's
  text in the window's language forms (`locl`).
- **Shaping:** script and language features (`Language::named("sr")`), variation selectors,
  fractions, random alternates, Syriac stretching, variable-font feature variations: matches
  HarfBuzz 12.3.2 and 14.5 on 13,066 words in 77 fonts. Mark composition and decomposition:
  matches HarfBuzz 14.5 on 428,444 word-font pairs.
- **Vertical text** (`ui.vertical_label`): UAX #50 orientation, `vert` forms, `vmtx`/`VORG`;
  matches HarfBuzz in seven CJK fonts.
- **Dates, times, numbers** in the user's system format (order, separators, digit counts, group
  sizes such as 12,34,567); month and day names and AM/PM in the system's language; Windows
  region changes followed while running; overridable.
- **Built-in words** (menus, file dialog, calendar, bars) in German, Spanish, French, Japanese,
  Portuguese and Simplified Chinese with `translations`, from the user's language or
  `ui.translate("de")`.

## Drawing and pictures

- Rounded rectangles, borders, shadows, linear and radial gradients, SVG-path icons; paths with
  arcs, holes and transforms; pens with caps, joins and dashes; hit testing; perceptual colour
  mixing, OKLCH, contrast ratios, colour-blind-safe chart colours.
- Curves within 1/64 px with exact edge coverage: circles of 2–1,500 px, filled and stroked, are
  never more than 9/255 off per pixel, under 1/255 on average.
- Read PNG, JPEG, GIF, WebP, BMP, ICO; EXIF orientation; colour profiles (`color-profiles`).
- Write PNG, JPEG (byte-identical to `cjpeg -baseline`) and multi-size ICO.
- Animated GIF, PNG and WebP; only the animation's box redraws.
- Video and sound (`video`): files, addresses and bytes, played by the system's player
  (Media Foundation, GStreamer, AVFoundation on macOS and iOS, the NDK's media codecs and AAudio
  on Android, a `<video>` element on the web), loaded at run time. Only the video's box
  redraws per frame. Play, pause, seek to the exact frame, volume, mute, loop, end; controls for
  keyboard and screen readers.
- SVG: clip paths, masks, patterns, markers, symbols; on request, text, filters (about 73 KB)
  and embedded pictures. Drawing cost is bounded by file length.
- 3D meshes: filled, lit, coloured or outlined, with depth; all CPU cores, only on change; a
  million triangles in about 14 ms at 1920×1080.
- PDF: pages up to 200 inches, shapes, searchable shaped text with subset fonts (as their
  licences allow), colour emoji (`emoji`), pictures with their colour profiles, links, Markdown
  layout.
  Tagged (PDF/UA-1: structure, reading order, alt text, table headings, language), with an
  outline of its headings; PDF/A-2b; form fields; AES-256 encryption. Passes veraPDF; readable by
  Poppler, Ghostscript and xpdf.
- Printing through the system dialog, laid out for the paper picked where asked. Windows: shapes
  and text, blending what GDI can't draw; oversized pages on several sheets. Linux: the portal's
  dialog over the window.
- About 200 KB (PDF), 150 KB (Markdown layout), 45 KB (printing), only when used.

## Windows and the desktop

- **Android** (`NativeActivity`, Android 11+): the activity's window inside system bars, cutout
  and soft keyboard; rotation, pause and resume keep the interface; soft keyboard and its text;
  system fonts; dark mode, accent and text size. `examples/apk.rs` packs an example as an APK.
- Any number of windows sharing app state; several screens; four kinds of second window (normal,
  tool, dialog, always on top); close requests that can be refused, also at logoff.
- Frameless windows with app-drawn title strips (real title bar on Windows: snapping works);
  full screen; transparent windows.
- Saved state: docks, tables, panels, windows, pan-zoom views, window position, recent colours,
  one line of text each;
  `taut::storage` writes atomically to the settings folder.
- Drag and drop between boxes, from other programs (files, text) and out to them (files, text,
  pictures, links).
- Clipboard (Windows, X11, Wayland, macOS): text, pictures, files. iOS: text, pictures. Android
  and the web: text.
- **The web** (`wasm32-unknown-unknown`, no crates): a page's canvas, sharp at its pixel
  ratio; mouse, wheel, pinch, fingers, pens, keys, input methods; the browser's text or the
  page's fonts; ARIA elements for screen readers; `localStorage`, file picker, downloads. A
  frame only when something changed.
- Open files and links with the default program.
- **iOS and iPadOS** (UIKit): one window filling its scene, the safe area and keyboard kept
  clear; rotation, Split View and Stage Manager sizes; dark mode, Dynamic Type, more contrast,
  less motion; fingers, Apple Pencil, trackpad and mouse, hardware keyboards, input methods, the
  system's Copy, Cut, Paste and Select All; VoiceOver; going to the background heard as
  minimized. `examples/ios_bundle.rs` makes an example an app for the simulator or a device.
- Tray icon with native menu, system notifications (on macOS for app bundles), global hotkeys,
  single instance. Unused, none of it is compiled in.
- **GPU or CPU** (`gpu`): Direct3D 11 or 12, Vulkan or OpenGL (Windows), OpenGL ES 3 (EGL) and
  Vulkan (Linux, X11), or Metal (macOS) show the pixels the CPU drew, byte for byte the same; whole frames in step with
  the screen; only changed rectangles uploaded (and presented, where the path can); a missing
  path falls back to the next; a lost device made again, else the CPU. `Window::renderer`,
  `ui.renderer()`, `TAUT_RENDERER=cpu|gpu|d3d11|d3d12|vulkan|opengl|metal`.
- **Drawn by the GPU** (`gpu-draw`): every GPU path draws boxes, borders, shadows, text,
  pictures, gradients, filled shapes and strokes itself, within a few levels of the CPU; scenes,
  SVG drawings and colour emoji go up as CPU-drawn patches. Only changed parts drawn; `Auto` only large ones. `TAUT_DRAW=cpu|gpu|auto`.
- **HDR** (`hdr`): boxes and pictures brighter than SDR white on HDR screens: `El::brightness`,
  `ui.add_hdr_picture`, PQ and HLG PNGs (`cICP`), `ui.headroom()`. Direct3D 11 and 12 present
  in scRGB, Wayland in PQ (`wp_color_manager_v1`); the screen's SDR white and peak followed.
  The rest stays at SDR white; SDR screens get the same pixels.

## Input and accessibility

- **Keyboard:** Tab order, Enter and Space, arrows for menus, sliders and spatial focus; radio
  groups and tab rows are one stop each.
- **Touch** (`touch`; Windows, X11, Wayland, iOS, Android): tap, long press, inertial scrolling, pinch zoom;
  pens with pressure, tilt and eraser; touchpad pinch.
- **Screen readers** (UI Automation, AT-SPI, NSAccessibility, UIAccessibility, Android's `AccessibilityNodeProvider`): role, name, state, position and shortcut for every
  part, including off-screen content; tables and trees with full structure; text by character,
  word, line as wrapped and paragraph, with each letter's place; misspellings in all of a box's
  text; announcements. Long tables: any row reached and selected through the table, every
  selected row told (`ui.tell_selection`). Nothing is built until a reader asks.
- **Themes:** dark, light, two high-contrast; accent, density and text scale; or follow the
  system, live. High contrast keeps text at 7:1. Reduced motion respected.

## Without a window

- `Surface`: draw into memory for any renderer, as a transparent overlay, or in tests;
  `ui.wants_pointer()` and `ui.wants_keyboard()` split input with the host.
- Layers for Direct3D 9, 11 and 12, OpenGL and Vulkan; `Surface::upload` for any other
  renderer's texture upload; a DLL with a C interface.
- A whole desktop on a pixel buffer, pointer drawn by the library: a mouse move at 1280×720
  redraws about 0.1% of the screen in about 50 µs.
- `testing::Trial`: a window with no screen, driven as a user would; parts found as a screen
  reader finds them; every step checks partial against full redraw. Unused, it adds nothing.
