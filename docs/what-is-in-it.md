# What is in taut

Every feature, by area. Gaps: [what is missing](what-is-missing.md). Calls: the
[guide](../GUIDE.md).

## Layout

- Rows, columns, grids, layers, wrapping rows; fit, fixed and shared sizes; padding, gaps, line
  gaps, alignment, offsets.
- Wrapping, aligned text; "…" when clipped.
- Boxes that shrink in priority order when room runs short (`.gives_way`).
- Scrolling both ways (wheel, keys, bars that never cover content); rows kept in view
  (`.frozen()`); sticky headings and columns (`.sticky()`, `.sticky_x()`), in the nearest box
  scrolling their way, or with `.frozen()` the next one out; the page scrolls both ways when
  larger than the window (wheel, bars, keys, screen readers).
- See-through boxes (`.opacity`) kept as a layer: a new opacity blends it again without drawing
  it.
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
  choice); Ctrl+C copies rows as spreadsheet text; Ctrl+V pastes it into editable cells, past
  the last row adding rows where the app lets it (`ui.paste_adds_rows`), values a column
  refuses outlined and handed to the app (`ui.refused_cells`); Ctrl+Z and Ctrl+Y undo and redo
  edits and pastes (`ui.cell_changes`), an undone paste handing back the rows it added
  (`ui.removed_rows`).
- Lists of any length cost only visible rows, including rows of varying height.
- Striped rows in lists, tables and tree views (`ui.striped_rows`).

## Panels, docks, pan and zoom, graphs

- **Panels:** float, move, resize, fold, close, raise; snap to edges and each other; keyboard
  move and resize. `ui.pin` fixes a box to a window edge.
- **Docks:** split, stacked and tabbed; drag tabs to drop markers; float and re-dock; reorder,
  close, maximise; Ctrl+Tab, Ctrl+F6, Ctrl+F4. The layout is a plain value.
- **Pan and zoom:** own coordinate space; pan by drag, wheel and bars; zoom by Ctrl+wheel, pinch and
  keys. Only visible content is described, drawn at its zoomed size; text under 4 units shows
  as a faint bar per line.
- **Node graphs:** typed ports, inputs for one wire or several, app parts in nodes, wiring with
  type checks, rubber-band selection, copy and delete, grid snap, frames that carry their nodes,
  bends on wires, undo and redo, raise on pick, panning while dragging at an edge, minimap.
  Wires are items readers hear and keys reach.
  1,000 nodes: 60 µs to redescribe and redraw, about 330 µs with all visible.

## Code, inspector, rich text

- **Code box:** syntax colouring from data (`Syntax`: Rust, C, Python, JavaScript, custom),
  multiple cursors, undo, folding; the system's keyboard on Android (no suggestions or capitals;
  its deletes keep each selection); comments and strings spell-checked when asked
  (`ui.check_spelling`); cost is the visible lines.
- **Inspector:** an app value as name/editor rows; folding sections, editable lists, filter,
  changed marks, reset, undo, several values at once.
- **Rich text:** styled pieces with links, icons and pictures in one wrapping paragraph.
- **Markdown:** CommonMark plus GitHub tables, task lists and strikethrough; highlighted code
  that scrolls sideways, app pictures, any nesting depth; Hebrew and Arabic right to left, with
  list markers, quote bars and code naming its language at the right (as GitHub has them);
  selectable, copyable, a context menu, keyboard links with titles (Tab wraps round them;
  Shift+arrows select from one), heading links scroll; a document to screen readers, tables as
  tables, the selection told, in cells too, and whole however far past the view (UI Automation,
  AT-SPI, macOS); laid out only around what is seen, a far jump (Ctrl+End, a heading link, the
  scroll bar) included: cost is the visible part.

## Fonts and text

- **System fonts** by name and style; family lists; system fallback for missing letters
  (Windows: feature `fallback-fonts`, else linked fonts; Linux: Fontconfig); with that feature a
  taller fallback font is sized to the face's line, and what still overhangs it is clipped.
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
- **Right to left** (`right-to-left`): full Unicode bidi (UAX #9), each paragraph ordered whole
  (a wrapped line's numbers and punctuation as unwrapped: 1,251,592 wrapped lines of Unicode's
  tests), Arabic and Syriac joining, marks placed as HarfBuzz does, visual cursor, split where
  directions meet.
- **Indic and South-East Asian** (`brahmic`): all of HarfBuzz's Indic and Universal Shaping
  Engine scripts; grapheme clusters kept whole by cursor and line breaks. Matches HarfBuzz on
  263,205 words in 46 fonts. Thai, Lao, Khmer and Burmese lines break between words: Uniscribe
  on Windows, CoreFoundation on macOS and iOS, elsewhere ICU 78's word lists (`word-lists`,
  804 KB) walked as ICU walks them, matching ICU on 48,991 texts.
- **Shaped app fonts** (`shaping`): ligatures, contextual forms, stacked marks, kerning beyond
  pairs, as on pages, in whole pixels; matches HarfBuzz on 1,657 word-font pairs. The system's
  text in the window's language forms (`locl`), also in lines with marks or letters of fallback
  fonts, and on macOS and iOS: such lines are shaped here and drawn by the system, glyph by
  glyph (GDI, FreeType, CoreText).
- **Shaping:** script and language features (`Language::named("sr")`), variation selectors,
  fractions, random alternates, Syriac stretching, variable-font feature variations: matches
  HarfBuzz 12.3.2 and 14.5 on 13,066 words in 77 fonts. Mark composition and decomposition:
  matches HarfBuzz 14.5 on 428,444 word-font pairs. Marks in fonts without `GPOS` placed by
  combining class, as HarfBuzz's fallback: matches HarfBuzz 12.3.2 and 14.5 on 1,269 word-font
  pairs across, and 197 down a column.
- **Vertical text**: labels, paragraphs and rich text in columns right to left
  (`ui.vertical_label`, `ui.vertical_paragraph`, `ui.vertical_rich`), breaking as CJK line breaking
  (UAX #14, kinsoku) allows; text boxes (`ui.vertical_text_box`, `ui.vertical_text_area`) with
  cursor, selection, keys along and across columns, pointer, input methods at the cursor, clipboard
  and undo, read by column with each character's place. UAX #50 orientation, `vert` forms,
  `vmtx`/`VORG` (matches HarfBuzz in seven CJK fonts); tate-chu-yoko (`ui.upright_digits`;
  `Span::upright` with `ruby`); fullwidth punctuation trimmed beside punctuation, each mark of the
  kind its glyph in the font shows, and 〜 and ゠ starting a line in Chinese, as Chrome does.
  Matches Chrome within a pixel on 108 clusters in Yu Gothic, and on 21 lines of punctuation and
  breaks in Yu Gothic, Microsoft JhengHei and Microsoft YaHei.
- **Ruby** (`Span::ruby`, feature `ruby`): readings over rich text and beside vertical text,
  spread and reaching over neighbours as CSS Ruby and JLREQ have it; within a pixel of Chrome.
  Readers hear each reading after its base, in brackets.
- **Dates, times, numbers** in the user's system format (order, separators, digit counts, group
  sizes such as 12,34,567, the script's digits such as ١٢٣, AM/PM before or after); month and
  day names and AM/PM in the system's language; on Windows, macOS, iOS, Android and Linux
  (glibc). Changes followed while running (Linux: GNOME's 12/24-hour clock). Overridable, or
  CLDR's for 34 places (`Locale::named("hi-IN")`). Typed in the script's digits or 0–9.
- **Built-in words** (menus, file dialog, calendar, bars) in 20 languages with `translations`
  (Arabic, Chinese, Dutch, French, German, Hindi, Indonesian, Italian, Japanese, Korean,
  Polish, Portuguese, Russian, Spanish, Swedish, Thai, Turkish, Ukrainian, Vietnamese), from
  the user's language or `ui.translate("de")`.

## Drawing and pictures

- Rounded rectangles, borders, shadows, linear and radial gradients, SVG-path icons; paths with
  arcs, holes and transforms; pens with caps, joins and dashes; hit testing; perceptual colour
  mixing, OKLCH, contrast ratios, colour-blind-safe chart colours.
- Curves within 1/64 px with exact edge coverage: circles of 2–1,500 px, filled and stroked, are
  never more than 9/255 off per pixel, under 1/255 on average.
- Read PNG, JPEG (with `rare-jpeg`, every process of the standard: hierarchical and arithmetic
  lossless too), GIF, WebP, BMP, ICO (each image, shown at the size it was made for); EXIF
  orientation; colour profiles (`color-profiles`).
- Write PNG, JPEG (baseline or progressive, byte-identical to `cjpeg`; EXIF orientation, ICC
  profile, XMP) and multi-size ICO (PNG or BMP inside, past 256 px).
- Animated GIF, PNG and WebP; only the animation's box redraws.
- Video and sound (`video`): files, addresses and bytes, played by the system's player
  (Media Foundation, GStreamer, AVFoundation on macOS and iOS, the NDK's media codecs and AAudio
  on Android, a `<video>` element on the web), loaded at run time. Only the video's box
  redraws per frame. Play, pause, seek to the exact frame, volume, mute, loop, end; speed from
  0.25 to 2 times, the pitch kept; sound and caption tracks listed with their languages and
  chosen; captions (the file's own, those in the video stream (CEA-608 as track `CC1`, each
  CEA-708 service as `Service 1`…, in H.264 and HEVC), and the app's WebVTT or SRT) drawn over
  the video in the system's caption style and said by screen readers; controls for keyboard and
  screen readers. Windows decodes on the GPU (DXVA) what the GPU decodes (checked: H.264, HEVC,
  VP9). With `gpu-draw`, frames are drawn from the decoder's own buffer, never read back while
  they play: Direct3D 11 and 12, Vulkan and OpenGL from Windows' texture (1080p H.264: 1.5 to
  2.7 ms of CPU a frame instead of 6 to 7.7), Metal from AVFoundation's pixel buffer, Vulkan and OpenGL ES from Android's hardware buffer,
  Vulkan and EGL from a Linux decoder's dmabuf. The same pixels as read back (Android's OpenGL
  ES: within 2 levels). A device keeps each buffer imported while its video lives (8 at most);
  one it can't import makes it draw frames read back from then on. Vulkan draws frames of
  several kinds in one part. Windows' decoder never waits for a texture a window holds.
- SVG: clip paths, masks, patterns, markers, symbols; on request, `shape-rendering`, seamless
  clip paths and masks, text (on paths too; shaped with `shaping`), filters (every primitive,
  lighting, `BackgroundImage`, paint inputs (patterns too) and kernel units included; about 90
  KB), pictures and nested SVG (in the file, or through the app's loader). SMIL animation,
  motion along paths included: as frames, or playing in a window (`AnimatedDrawing`), where the
  pointer (within clip paths; the root where no shape is), keys, focus (Tab and Shift+Tab between
  elements in `tabindex` order) and the wall clock begin and end what waits for them;
  an event costs the same however long it has played. Drawing cost is bounded by file length.
  Windows' system fonts are read a glyph at a time (a page of CJK text peaks at 6 MB, not 51).
- 3D meshes: filled, lit, coloured or outlined, with depth; all CPU cores, only on change; a
  million triangles in about 14 ms at 1920×1080.
- PDF: pages up to 200 inches, shapes, searchable shaped text with subset fonts (as their
  licences allow), colour emoji (`emoji`), pictures with their colour profiles, links, Markdown
  layout.
  Tagged (PDF/UA-1: structure, reading order, alt text, table headings, language; right-to-left
  list numbers read as written), with an outline of its headings; PDF/A-2b; form fields; AES-256
  encryption. Passes veraPDF; readable by Poppler, Ghostscript and xpdf.
- Printing through the system dialog, laid out for the paper picked where asked. Windows: shapes
  and text, blending what GDI can't draw; oversized pages on several sheets. Linux: the portal's
  dialog over the window. The web: the browser's dialog, each page a picture at 288 dpi.
- About 200 KB (PDF), 150 KB (Markdown layout), 45 KB (printing), only when used.

## Windows and the desktop

- **Android** (`NativeActivity`, Android 11+): the activity's window inside system bars, cutout
  and soft keyboard; rotation, pause and resume keep the interface; soft keyboard and its text,
  what it composes shown; system fonts, named ones too; dark mode, accent and text size. The
  system's file dialog (documents as copies, written back), notifications, spell checking,
  printing, drops and drags between apps, opening the app's files. `examples/apk.rs` packs an
  example as an APK.
- Any number of windows sharing app state; several screens; four kinds of second window (normal,
  tool, dialog, always on top); close requests that can be refused, also at logoff.
- Frameless windows with app-drawn title strips (real title bar on Windows: snapping works);
  full screen; transparent windows.
- Saved state: docks, tables, panels, windows, pan-zoom views, window position, recent colours,
  one line of text each;
  `taut::storage` writes atomically to the settings folder.
- Drag and drop between boxes, from other programs (files, text) and out to them (files, text,
  pictures, links).
- Clipboard (Windows, X11, Wayland, macOS, iOS, Android): text, pictures, files. The web: text,
  pictures, files pasted in.
- **The web** (`wasm32-unknown-unknown`, no crates): a page's canvas, sharp at its pixel
  ratio; mouse, wheel, pinch, fingers, pens, keys, input methods; the browser's text or the
  page's fonts, and the person's fonts listed (Chrome, Edge); ARIA elements for screen readers;
  `localStorage`; the browser's pickers (a file, several, a folder; saving where the browser
  has a save picker, else a download); drops in and drags out; notifications; printing; one
  running copy across tabs; WebGL2 (`gpu`, `gpu-draw`). A frame only when something changed.
- Open files and links with the default program.
- **iOS and iPadOS** (UIKit): a window filling its scene, the safe area and keyboard kept
  clear; on iPad each second window a scene of its own; a scene the system lets go of to free
  memory keeps its window and state, the person closing one is a close request, the app closing
  the one in front brings the first forward; rotation, Split View and Stage Manager sizes; dark
  mode, Dynamic Type, more contrast, less motion; fingers, Apple Pencil, trackpad and mouse,
  hardware keyboards, input methods, the system's keyboard reading text boxes (corrections,
  predictions, capitals) and its edit menu; the document picker as the system file dialog
  (several files, folders, saving); files opened in the system's preview; files, pictures and
  text dropped in; drag out; files on the clipboard; VoiceOver; going to the background heard as
  minimized. `examples/ios_bundle.rs` makes an example an app
  for the simulator or a device.
- Tray icon with native menu, system notifications (on macOS and iOS for app bundles), global
  hotkeys, single instance. Unused, none of it is compiled in.
- **GPU or CPU** (`gpu`): Direct3D 11 or 12, Vulkan or OpenGL (Windows), OpenGL ES 3 (EGL) and
  Vulkan (Linux, X11), Metal (macOS, iOS), Vulkan and OpenGL ES 3 (Android), or WebGL2 (the
  web) show the pixels the CPU drew, byte for byte the same; whole frames in step with the
  screen; only changed rectangles uploaded (and presented, where the path can); a missing path
  falls back to the next; a lost device made again, else the CPU. On iOS no GPU work in the
  background; on Android the device outlives the activity's window. `Window::renderer`,
  `ui.renderer()`, `TAUT_RENDERER=cpu|gpu|d3d11|d3d12|vulkan|opengl|metal` (Android:
  `debug.taut.renderer`; the web: the loader's `renderer` option).
- **Drawn by the GPU** (`gpu-draw`): every GPU path draws boxes, borders, shadows, text,
  pictures, gradients, filled shapes and strokes itself, within a few levels of the CPU; scenes,
  SVG drawings and colour emoji go up as CPU-drawn patches. Only changed parts drawn; `Auto` only large ones. `TAUT_DRAW=cpu|gpu|auto`.
- **HDR** (`hdr`): boxes and pictures brighter than SDR white on HDR screens: `El::brightness`,
  `ui.add_hdr_picture`, PQ and HLG PNGs (`cICP`), Ultra HDR JPEGs (ISO 21496-1, Adobe's and
  Apple's gain maps, as libultrahdr reads them), `ui.headroom()`. Gain maps per colour, sampled
  bilinearly, shown by the screen's headroom as their metadata says. Direct3D 11 and 12,
  Vulkan and OpenGL (a float pixel format) present in scRGB (Vulkan in HDR10 where the GPU has
  no scRGB); Wayland hands over PQ (`wp_color_manager_v1`); macOS shows EDR through Metal.
  The screen's SDR white and peak (macOS: EDR headroom) followed, Vulkan's HDR metadata too. The
  rest stays at SDR white;
  SDR screens get the same pixels.

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
- Layers for Direct3D 9, 11 and 12, OpenGL, OpenGL ES, Vulkan and Metal; `Surface::upload` for
  any other renderer's texture upload; a DLL with a C interface.
- A whole desktop on a pixel buffer, pointer drawn by the library: a mouse move at 1280×720
  redraws about 0.1% of the screen in about 50 µs.
- `testing::Trial`: a window with no screen, driven as a user would; parts found as a screen
  reader finds them; every step checks partial against full redraw. Unused, it adds nothing.
