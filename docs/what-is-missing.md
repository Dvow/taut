# What is missing

Known gaps and limits. Features: [what is in it](what-is-in-it.md).

## Systems

- Windows, X11, Wayland, macOS, iOS, Android and the web (tested on Windows 11, Ubuntu 26.04,
  Arch Linux; on GitHub runners macOS 26 on arm64, iOS 26 in its simulator and Android 15 on an
  x86-64 emulator; Edge and Chrome, headless).
- Android: one window, the activity's; second windows are not shown. The CPU draws and shows:
  `Renderer` and `gpu` change nothing. No tray, notifications, hotkeys, drag and drop,
  printing, spell checking, system file dialog or pictures and files on the clipboard. What an
  input method composes is typed into the box as it goes, not underlined. `open_with_system`
  opens links only. `Face::named` fonts are not looked up. Untried: TalkBack itself (nodes read
  with `uiautomator`, and in the app through its provider), a real phone, arm64 beyond building.
- Web: one window, the page's; no second windows, tray, hotkeys, system notifications, drag and
  drop with other programs, printing, one running copy, GPU paths or threads. Clipboard: text
  only; a menu's Paste has the page's last copy or paste unless the person let the page read the
  clipboard. Files: the browser's picker opens one file, read whole into memory; saving is a
  download. The person's fonts aren't listed (`system_fonts::families` is empty). Code after
  `taut::run` runs at once. Readers: tested against the DOM, not with a screen reader itself.
- macOS: `layer_vulkan` untested (no Vulkan loader or MoltenVK on the runner).
- iOS: the CPU draws and Core Animation shows; `Renderer` and `gpu` change nothing. No tray or
  global hotkeys (iOS has neither). No second windows: an iPhone app has one; on iPad they are
  not opened as scenes yet. Without `touch`, only the first finger works, as the mouse.
- iOS: in a password box and the code box the keyboard sees only what an input method composes
  (no corrections). The system's loupe and selection handles don't show: taut draws its own
  selection. Reading what another app put on the clipboard asks the person, except through the
  system's Paste.
- iOS: a drag out lifts only with a long press (a pointer dragged past the window takes nothing
  out); a dropped folder is not taken. A notification click that starts the program is not
  heard. Unchecked: a notification shown (the simulator never grants permission), the document
  picker's own screens (it is answered through its delegate).
- iOS: readers get letters and words through a text interface from iOS 18.1 on (before, lines
  only). Text is read through it, not typed.
- Android: readers move through text by character, word, line and paragraph, not by page.
- iOS: checked in the simulator only, through the methods UIKit calls: not on a device, not with
  VoiceOver itself, real touches or a real hardware keyboard.
- macOS: what an input method composes shows in a small panel at the text cursor, over the box,
  not laid into its text. A text range's place is its box's, as on Windows.
- macOS: system notifications only for an app bundle (the notification centre serves no bare
  executable); clicks are heard. No touch or pens beyond pinch. Apple Color Emoji's sequences
  are shaped by CoreText, not `shaping.rs`. Unchecked: x86-64 Macs, VoiceOver itself, a
  notification shown (the runner never grants permission).
- macOS: the tray icon's menu opens on a right click or Control-click; a click is `.clicked`.
  Hotkeys: Ctrl is Command, Super is Control; no media keys. Desktop places are points from
  the main screen's top left.
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
  `ui.window_placement()` reports the frame's corner.
- Linux dark mode comes from the portal, else from the X theme name; if the app starts the
  portal, the first frames precede its answer.

## Layout and parts

- Sticky headings stick to the nearest vertical scroll box. `frozen` and `sticky` together in one
  box are unsupported. The page scrolls vertically only. `.opacity` re-blends what's under it on
  each redraw. Stripes only in grids and columns.
- Built-in words other than month and day names and AM/PM are English until set, or with
  `translations` in six more languages only.
- Locale: digits 0–9 only. An app's own `Locale` writes groups of three, two-digit days, months
  and hours, and ":". Linux reads the locale once (Windows follows changes). AM/PM always
  after the time.
- Linux screen readers can't set number or time fields by value (typing and stepping work).
- Pasting into tables: no pasting into new rows; a value a column refuses is skipped silently.
  Windows time axes use today's UTC offset. Uneven-height lists estimate unseen rows. Second windows don't show the first's meshes.
  Pan-zoom text under 4 units isn't drawn.
- Node graphs: frames don't nest or hold other frames; only the first 64 inputs of a node can
  take several wires; a wire's bends are reached by keys only while its output's node is in
  view; undo keeps a copy of the graph per step (100 steps or 100,000 nodes in all).
- Spelling: code box unchecked; readers get errors
  among visible words only; UI Automation's `FindAttribute` unanswered.

## Renderers

- Layers for Direct3D 9, 11 and 12, OpenGL 3.3, Vulkan and Metal; others through
  `Surface::upload` (chapter 9).
- Layers draw into single-sample targets; Vulkan only in a render pass (not dynamic rendering).
  Direct3D 12 and Vulkan leave their state set after `draw`.
- Lost devices: plain Direct3D 9's `Reset` is untested (it makes no device on a desktop of its
  own; 9Ex was tested); Vulkan's loss can't be simulated.
- `gpu`: see-through windows stay on the CPU.
- `hdr`: Vulkan, OpenGL, X11 and macOS show SDR. HDR pictures only from PNG (`cICP`): not
  AVIF, JPEG XL or Ultra HDR JPEGs. Gain maps are sampled nearest, not smoothed. Tested on
  HDR screens faked: this PC's are SDR, and no compositor on the test bed has an HDR output.
- `gpu-draw`: scenes, SVG drawings (their blur too: drawn once, then kept), colour emoji, fills
  of other inks and shapes of over 256 edges in 16 rows are drawn by the CPU (into patches). A
  see-through box (`El::opacity`) has the CPU draw its part: the GPU has no layers yet. GDI's
  text off one solid colour loses GDI's contrast (up to 13 levels in the walk). Direct3D 12 and
  Vulkan wait for each part drawn. Tested on WARP, SwiftShader, llvmpipe, lavapipe and an RTX
  2080 Ti; Wayland's drawing not run (no device here exports a dmabuf). On Windows, OpenGL's
  walk can't be pictured on a private desktop.
- `gpu` on Windows: the first frames are the CPU's until the device is made (0.3–0.9 s here). A
  window the GPU showed can't go back to GDI: a GPU lost for good leaves it to Direct3D 11's
  software device (WARP). Direct3D 12's device loss is tested for real (`RemoveDevice`), the
  others' simulated. OpenGL swaps whole frames (WGL has no partial swap); Vulkan names changed
  rectangles only with `VK_KHR_incremental_present`. OpenGL's window can't be pictured on a
  private desktop, so its tests compare the back buffer only. Smooth live resizing and tearing
  were not judged by eye.
- `gpu` on Linux: Vulkan tested on lavapipe and dozen (WSL); its device loss is simulated.
  Under X, `Auto` takes only a hardware device that shows frames directly (DRI3 with Mesa).
  Measured only under Xvfb, where every frame goes through X and the GPU path costs more than the
  CPU's.
- `gpu` on macOS: tested only on GitHub's runner (a VM's paravirtual Apple GPU, one screen at
  scale 1): a window moving to a screen of another GPU, a removed GPU (loss simulated) and Retina
  scales in a window untried; x86-64 Macs untried. Frames wait for the screen (60 Hz there).
- `gpu` under Wayland: Vulkan dmabufs only when asked; `Auto` keeps shared memory (cheaper on
  the one device measured). Tested only with lavapipe exporting through `/dev/udmabuf` (GitHub's
  Ubuntu runner) to Weston's GL renderer: no hardware GPU, Mutter, KWin or Sway, no tiled or
  multi-plane layout. Feedback v4 and the main device only against a simulated compositor.
  Waits for each copy before the commit (no explicit sync).

## Documents (Markdown)

- Measured whole on first show (700 lines: about 9 ms); every link has a box each run (5,000
  links: 0.4 ms per run).
- Rows run left to right: Hebrew or Arabic paragraphs aren't reordered across style pieces.
- Code blocks wrap; tables are told as text; selection isn't told; no context menu, edge
  auto-scroll or link titles. Nesting over 100 deep becomes text.

## File dialog

- The library's own unless `FileDialog::native()`: Windows, or a Linux portal (v3+).
- A portal silent for 5 s falls back to the library's.
- Windows `FileDialog::native()` without a taut window (renderer, DLL) answers nothing;
  `FileDialog { native: true, ..Default::default() }` falls back anywhere.

## Text and fonts

- Vertical text: labels only, no editing, breaks only at line ends, no tate-chu-yoko or ruby.
- Without `shaping`, app fonts draw glyph by glyph, kerned by pairs (pages shape fully); system
  text keeps its default forms. With it, language forms in system text skip lines with marks,
  letters from fallback fonts, or scripts shaped by syllable; not on macOS.
- Marks in fonts without `GPOS` are not placed by HarfBuzz's fallback.
- `shaping` shapes each line the first time it is seen (15–40 µs in Segoe UI or Calibri: 0.4 s
  for a window of 10,000 new rows) and keeps up to 16 MB of runs; a full draw takes about 40%
  longer.
- `brahmic`: Thai, Lao, Khmer, Myanmar break by word only on Windows. Scripts with no Windows
  font need an app font. A bare Ubuntu has none.
- Emoji: `COLR` v1 and SVG only on request; variable emoji fonts at default; sRGB mixing; no
  flags on Windows; no emoji font, plain letters.
- IME window placement verified on Linux with a real IME, on Windows only by reading back.
- Fonts' hinting instructions run only with `hinting` (else small Latin, Greek, Cyrillic are
  auto-hinted), vertically only; at a variable font's instance about 1 glyph in 400 lands 1/64
  px off FreeType's. `VVAR`, variable `COLR` and vertical-metric variations not read. `CFF2`
  checked only with hand-made fonts.
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

## Video

- No captions, no choice of sound or caption track, no playback speed.
- Frames are copied through the CPU into a picture, also with `gpu`.
- Windows: decoded in software (no Direct3D device given to Media Foundation), so only what
  Windows decodes so: H.264, MPEG-4 Part 2, and what its extensions add; not MJPEG. N editions need
  the Media Feature Pack. `open_silent` plays muted.
- Linux: needs GStreamer's `playbin` and `appsink` and the plugins of the file's codecs; a
  missing one is `VideoError::NoPlayer` or `video.error()`.
- macOS and iOS: `open_silent` plays muted; bytes are played from a temporary file. iOS: a
  video with sound sets the audio session's category to `Playback` unless the app set one.
  Checked only on GitHub's runner and in the iOS simulator.
- Android: what the system's decoders read, through the NDK (no `MediaPlayer`); frames come
  through an image reader in YUV and are made RGB on the CPU. Volume scales the samples (AAudio
  has none). Addresses play only in an app's process. No sound device: it plays silent.
  Checked only on an emulator (software decoders); there, a command-line program that ended
  the moment it dropped its last video once aborted in Android's own decoder thread.
- Web: the browser's formats (H.264 in MP4 in most; VP8, VP9, AV1 in WebM). Sound plays only
  after a press on the page: before, `play` gives `error()`, cleared at the next `play`;
  muted, it plays. Another site's address needs CORS headers, or its frames can't
  be read. A path is an address relative to the page. Under Node there is no player.
- Tested with small made files (lossless H.264, MJPEG, Theora); playback of large files not
  measured.

## PDF and printing

- Non-TrueType and positioned variable fonts as shapes; COLR v1 and SVG emoji as their plain
  outlines. Forms: no signatures, scripts or radio groups. PDF/UA not claimed in a reader's sense
  beyond what veraPDF checks.
- Pages from `print` (not `print_laid`) aren't laid out again for the paper picked.
- Printing tested only against stand-ins (a metafile on Windows, a private bus on Linux).

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
- Long tables: a row out of view is reached by scrolling it into view (no UI Automation
  virtualized items). Windows lists the selected rows in view.
- Letter places only for text that shows and is one box's (not `ui.words` pieces or a row read
  as one); elsewhere lines are as typed.
- Misspellings told for all of a box's text up to 64 KB; past that, the visible words.
- While a reader listens, a scroll walks every box: 10,000 rows take 1.6 times as long per
  scroll (9.7 ms instead of 6.2, measured on a busy PC). No cost without a reader. Long lists
  still cost only visible rows.
