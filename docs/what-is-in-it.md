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
- Programmatic scrolling, stay-at-end for chats and logs, scroll into view; `ui.room` to fit
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
- Spell checking with the system's checker and user dictionary (Windows; Enchant on Linux),
  visible words only, on a background thread; suggestions, "Add to dictionary", "Ignore".
  About 30 KB, only when used.

## Tables and long lists

- Sort by heading (Shift: several columns); resize, reorder, hide columns.
- Horizontal scrolling with pinned columns; pinned top and bottom rows.
- One Tab stop with a cell cursor; keyboard row selection; in-place editing (text, number,
  choice); Ctrl+C copies rows as spreadsheet text.
- Lists of any length cost only visible rows, including rows of varying height.

## Panels, docks, boards, graphs

- **Panels:** float, move, resize, fold, close, raise; snap to edges and each other; keyboard
  move and resize. `ui.pin` fixes a box to a window edge.
- **Docks:** split, stacked and tabbed; drag tabs to drop markers; float and re-dock; reorder,
  close, maximise; Ctrl+Tab, Ctrl+F6, Ctrl+F4. The layout is a plain value.
- **Boards:** own coordinate space; pan by drag, wheel and bars; zoom by Ctrl+wheel, pinch and
  keys. Only visible content is described, drawn at its zoomed size.
- **Node graphs:** typed ports, app parts in nodes, wiring with type checks, rubber-band
  selection, copy and delete, minimap; fully keyboard-operable and readable by screen readers.
  1,000 nodes: 60 µs to redescribe and redraw, about 330 µs with all visible.

## Code, inspector, rich text

- **Code box:** syntax colouring from data (`Rules`: Rust, C, Python, JavaScript, custom),
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
  light hinting; `OwnFonts::smooth` turns it off.
- **Emoji** (`emoji`): colour emoji in all text, sequences as one glyph and one cursor step;
  from the system's font or the app's. `ui.emoji_in_full()` adds `COLR` v1 and SVG glyphs
  (about 30 KB with fonts already read; 360 KB in the smallest program).
- **Right to left** (`right-to-left`): full Unicode bidi (UAX #9), Arabic and Syriac joining,
  marks placed as HarfBuzz does, visual cursor.
- **Indic and South-East Asian** (`brahmic`): all of HarfBuzz's Indic and Universal Shaping
  Engine scripts; grapheme clusters kept whole by cursor and line breaks. Matches HarfBuzz on
  263,205 words in 46 fonts.
- **Shaping:** script and language features (`Language::named("sr")`), variation selectors,
  fractions, variable-font feature variations: matches HarfBuzz 12.3.2 and 14.5 on 13,066 words
  in 77 fonts. Mark composition and decomposition: matches HarfBuzz 14.5 on 428,444 word-font
  pairs.
- **Vertical text** (`ui.label_downward`): UAX #50 orientation, `vert` forms, `vmtx`/`VORG`;
  matches HarfBuzz in seven CJK fonts.
- **Dates, times, numbers** in the user's system format; overridable.

## Drawing and pictures

- Rounded rectangles, borders, shadows, linear and radial gradients, SVG-path icons; paths with
  arcs, holes and transforms; pens with caps, joins and dashes; hit testing; perceptual colour
  mixing, OKLCH, contrast ratios, colour-blind-safe chart colours.
- Curves within 1/64 px with exact edge coverage: circles of 2–1,500 px, filled and stroked, are
  never more than 9/255 off per pixel, under 1/255 on average.
- Read PNG, JPEG, GIF, WebP, BMP, ICO; EXIF orientation; colour profiles (`color-profiles`).
- Write PNG, JPEG (byte-identical to `cjpeg -baseline`) and multi-size ICO.
- Animated GIF, PNG and WebP; only the film's box redraws.
- SVG: clip paths, masks, patterns, markers, symbols; on request, text, filters (about 73 KB)
  and embedded pictures. Drawing cost is bounded by file length.
- 3D meshes: filled, lit, coloured or outlined, with depth; all CPU cores, only on change; a
  million triangles in about 14 ms at 1920×1080.
- PDF: pages up to 200 inches, shapes, searchable shaped text with subset fonts, pictures,
  links, Markdown layout; printing through the system dialog. Readable by Poppler, Ghostscript
  and xpdf. About 200 KB (PDF), 150 KB (Markdown layout), 45 KB (printing), only when used.

## Windows and the desktop

- Any number of windows sharing app state; several screens; four kinds of second window (normal,
  tool, dialog, always on top); close requests that can be refused, also at logoff.
- Frameless windows with app-drawn title strips (real title bar on Windows: snapping works);
  full screen; transparent windows.
- Saved state: docks, tables, panels, windows, boards, window position, one line of text each;
  `taut::kept` writes atomically to the settings folder.
- Drag and drop between boxes, from other programs (files, text) and out to them (files, text,
  pictures, links).
- Clipboard (Windows, X11, Wayland): text, pictures, files.
- Open files and links with the default program.
- Tray icon with native menu, system notifications, global hotkeys, single instance. Unused,
  none of it is compiled in.

## Input and accessibility

- **Keyboard:** Tab order, Enter and Space, arrows for menus, sliders and spatial focus; radio
  groups and tab rows are one stop each.
- **Touch** (`touch`; Windows, X11, Wayland): tap, long press, inertial scrolling, pinch zoom;
  pens with pressure, tilt and eraser; touchpad pinch.
- **Screen readers** (UI Automation, AT-SPI): role, name, state, position and shortcut for every
  part, including off-screen content; tables and trees with full structure; text by character,
  word and line; announcements. Nothing is built until a reader asks.
- **Themes:** dark, light, two high-contrast; accent, density and text scale; or follow the
  system, live. High contrast keeps text at 7:1. Reduced motion respected.

## Without a window

- `Surface`: draw into memory for any renderer, as a transparent overlay, or in tests;
  `ui.wants_pointer()` and `ui.wants_keyboard()` split input with the host.
- Layers for Direct3D 11 and OpenGL; a DLL with a C interface.
- A whole desktop on a pixel buffer, pointer drawn by the library: a mouse move at 1280×720
  redraws about 0.1% of the screen in about 50 µs.
- `testing::Trial`: a window with no screen, driven as a user would; parts found as a screen
  reader finds them; every step checks partial against full redraw. Unused, it adds nothing.
