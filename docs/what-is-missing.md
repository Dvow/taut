# What is missing

Known gaps and limits. Features: [what is in it](what-is-in-it.md).

## Systems

- Windows, X11 and Wayland only (tested on Windows 11, Ubuntu 26.04, Arch Linux). No macOS, iOS
  or Android; macOS, iOS and web builds stop with an error. Android untried.
- Wayland where a compositor answers, else X (`TAUT_BACKEND=x11` or `wayland` forces one).
  Wayland: no window positions, no always-on-top, bringing to front only if the compositor
  allows. Tested on KWin, Mutter, Sway and Weston. Weston: no fractional scale, no bringing to
  front, no named cursors, no input-method v3, no input when headless. Mutter: no window
  pictures. Sway: floating windows can't fill the screen.
- A `WAYLAND_SOCKET` handed down stays in child programs' environment.
- X11 touch (XInput 2; pinch needs 2.4): no pen twist; pen leaving range is inferred; touchpad
  two-finger scrolling stays wheel notches.
- Wayland pens and touchpad pinch tested only against a simulated compositor; tablet pads, pen
  distance and pen wheels not read.
- Without a window manager, Linux windows can't maximize, minimize, stay on top or come forward.
  `ui.placed()` reports the frame's corner.
- Linux dark mode comes from the portal, else from the X theme name; if the app starts the
  portal, the first frames precede its answer.

## Layout and parts

- Sticky headings stick to the nearest vertical scroll box. `kept` and `sticky` together in one
  box are unsupported. The page scrolls vertically only. `.opacity` re-blends what's under it on
  each redraw. Stripes only in grids and columns.
- Built-in words are English until set (`ui.calendar`, `files.words`, `ui.edit_words`,
  `ui.search_word`, `ui.number_words`, `ui.part_words`).
- Locale: month and day names and AM/PM are the app's; fixed digit counts (2-digit day and
  month, 4-digit year), `:` in times, digits 0–9 grouped by three; changes while running ignored.
- Linux screen readers can't set number or time fields by value (typing and stepping work).
- Recent colours are not saved. Tables don't accept paste. Windows time axes use today's UTC
  offset. Uneven-height lists estimate unseen rows. Second windows don't show the first's meshes.
  Board text under 4 units isn't drawn.
- Node graphs: one wire per input; no grid snap, frames, reroutes, undo, raise on pick or
  auto-pan; readers hear wires only through ports.
- Spelling: one language per box; no repeated-word check; code box unchecked; readers get errors
  among visible words only; UI Automation's `FindAttribute` unanswered.

## Renderers

Layers for Direct3D 11 and OpenGL 3.3. Vulkan and wgpu by hand (chapter 9). None for Direct3D 9
or 12, or Metal.

## Documents (Markdown)

- Measured whole on first show (700 lines: about 9 ms); every link has a box each run (5,000
  links: 0.4 ms per run).
- Rows run left to right: Hebrew or Arabic paragraphs aren't reordered across style pieces.
- Code blocks wrap; tables are told as text; selection isn't told; no context menu, edge
  auto-scroll or link titles. Nesting over 100 deep becomes text.

## File dialog

- The library's own unless `Files::of_the_system()`: Windows, or a Linux portal (v3+).
- Linux: no extension added to typed names. A portal silent for 5 s falls back to the library's.
- Windows `of_the_system()` without a taut window (renderer, DLL) answers nothing;
  `Files { system: true, ..Default::default() }` falls back anywhere.

## Text and fonts

- No split cursor at direction changes.
- Vertical text: labels only, no editing, breaks only at line ends, no tate-chu-yoko or ruby.
- Window fonts don't compose marks, apply `rand` or `stch`, or form ligatures; they apply `locl`
  and kern by pairs (pages shape fully). System text gets no language forms.
- `brahmic`: Thai, Lao, Khmer, Myanmar break by word only on Windows. Scripts with no Windows
  font need an app font. A bare Ubuntu has none.
- Emoji: `COLR` v1 and SVG only on request; variable emoji fonts at default; sRGB mixing; no
  flags on Windows; no emoji font, plain letters.
- IME window placement verified on Linux with a real IME, on Windows only by reading back.
- Fonts' hinting instructions are not run (small Latin, Greek, Cyrillic are auto-hinted
  vertically). Pair kerning only. `cvar`, `VVAR`, variable `COLR` and vertical-metric variations
  not read. Variable fonts and `CFF2` checked only with hand-made fonts.
- Fallback glyphs that overhang aren't clipped; Linux fallback line metrics aren't matched.
- Named faces are kept for the program's life; names over 128 bytes match nothing; beyond 4,096
  families or 65,536 faces, new names use the system font.
- Windows: the first letter no linked font has costs a full font scan (about 60 ms, 47 KB);
  SVG text loads whole fonts (a CJK page peaked at about 43 MB).

## Pictures

- JPEG: no hierarchical or arithmetic lossless. Icons: the largest image only.
- Refused as implausible: single-colour arithmetic JPEGs with more blocks than file bits;
  single-colour lossless WebPs over 2,048 pixels per file byte.
- Lossy WebP decodes about 3× slower than an equivalent JPEG.
- SVG not read: external or nested SVG images; `feImage`, lighting, `feConvolveMatrix`,
  `feDisplacementMap`; background inputs; `shape-rendering`; text on a path; animation.
  Small blurs use a true Gaussian (as browsers; librsvg differs). SVG text is unshaped. Masks and
  non-box clips multiply edge coverages; abutting subpaths can show a hairline. Of 2,986 Linux
  desktop icons, 386 differ visibly from librsvg at 1×, 199 at 3×.
- SVG cost beyond the file's budget is skipped; a small file can still take seconds at
  4096×4096.
- Highly compressible PNGs (30:1+) decode up to 1/7 slower than with an exact buffer.
- JPEG writing: baseline only, no metadata. ICO writing: 32-bit, up to 256 px.
- `add_picture_file` includes every decoder; call one decoder (`jpeg::decode`) and
  `add_picture` to include only that one.

## PDF and printing

- No outline, tags, forms, encryption or PDF/A. Emoji as outlines; colour profiles dropped;
  font permissions unchecked; non-TrueType and positioned variable fonts as shapes; table
  headings not repeated.
- Windows prints pixels, needs an active window, shrinks oversized pages. Linux: the portal's
  paper choice doesn't re-lay pages; the dialog isn't parented.
- Printing tested only against stand-ins.

## Windows

- Only the first window can be transparent. Windows: a full 32-bit copy, greyscale text, clicks
  pass through clear areas. X11: clicks don't pass through; black without a compositor.
- Window managers may keep a frame on frameless windows. Windows: maximized frameless windows
  cover an auto-hide task bar; no snap layout menu on app-drawn buttons.
- Dragging out of a window on Windows never observed end to end; no drag image; no copy-only
  option; X11 waits up to 10 s for a silent target.
- Linux opens files only via `xdg-open`. Opening was tested only against stand-ins.
- Linux clipboard contents go when the program exits (without a clipboard manager); no primary
  selection; GNOME Files 3.30–42 file copies unread. Wayland reports data taken once read.

## Tray, notifications, hotkeys

- Tested only against stand-ins (a test task bar on Windows, a private bus on Linux).
- X desktops without `StatusNotifierWatcher`: no icon. Tray menus: no submenus or icons.
  Notifications: no buttons; on Windows, clicks after the icon is gone are lost.
- X: Ctrl+Alt+F1–F12 can't be grabbed. Wayland hotkeys need the portal's `GlobalShortcuts`.
- Linux single instance needs `XDG_RUNTIME_DIR`.

## Accessibility

- Checked with UI Automation and libatspi clients in real windows; never with a real screen
  reader.
- Long tables expose visible rows only; no selection through the table interface; Windows
  tables don't list their selection.
- Text is read by typed lines, not wrapped ones; letter positions not exposed.
- While a reader listens, a scroll moves every told element: 10,000 rows take 6.1 ms per scroll
  instead of 3.0 (an idle run 1.1 ms instead of 1.0). No cost without a reader. Long lists still
  cost only visible rows.
- No events when content scrolls in or out of view.
- A hand-written `ui.tree` needs a box told as `Role::Tree` for Orca to read levels.
