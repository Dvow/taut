# What is missing

Known gaps and limits. Features: [what is in it](what-is-in-it.md).

## Systems

- Windows, X11, Wayland, macOS, iOS, Android and the web (tested on Windows 11, Ubuntu 26.04,
  Arch Linux; on GitHub runners macOS 26 on arm64, iOS 26 in its simulator and Android 15 on an
  x86-64 emulator; Edge and Chrome, headless).
- Android: one window, the activity's; second windows are not shown. No tray or hotkeys
  (Android has none); `hide_window` does nothing. The system's file dialog picks files, not
  folders (a folder of documents has no path): `pick_folder` shows the library's dialog. Picked
  documents are copies, written back after the run that changed them. What an input method
  composes shows in a small panel at the text cursor, over the box. The system parts need the
  package's classes (`examples/apk.rs`); an app packed otherwise has none of them. Untried:
  TalkBack itself (nodes read with `uiautomator`, and in the app through its provider), a real
  phone, arm64 beyond building; the emulator's own keyboard composes nothing (a test keyboard
  did).
- Web: one window, the page's: no second windows, tray or global hotkeys (a page has none), no
  threads (`wasm32-unknown-unknown`). Code after `taut::run` runs at once. Clipboard: a menu's
  Paste has the page's last copy or paste unless the person let the page read the clipboard;
  files can't be put on it (`copy_files` does nothing). Files are read whole into memory; the
  save picker only in Chrome and Edge (elsewhere a download), written after `save` returns. A
  file dragged out goes as one the page made (Chrome: and a download where it is let go): the
  DevTools protocol carries no such file, so its drop elsewhere is untested. Notifications: none
  in phone browsers (they need a service worker). One copy: two tabs opened at the same moment
  may both start as first; a tab can't come forward. Printing: pages as pictures (288 dpi), not
  text; the browser doesn't say whether the person printed (`Sent` once its dialog closes).
  Fonts: listed in Chrome and Edge only, once the person allows it (until then they list none;
  the page asks again at the next press). Readers: tested against the
  DOM, not with a screen reader itself.
- macOS: `layer_vulkan` untested (no Vulkan loader or MoltenVK on the runner).
- iOS: no tray or global hotkeys (iOS has neither). Second windows only on iPad (an iPhone shows
  one scene), each a scene the system sizes and places: a second window's size, place, screen
  and kind (tool, dialog, on top) are not used, though a dialog still keeps the windows opened
  before it from input. A scene the person opens from the system has nothing to show and
  closes, unless the first window, kept open as the person closed its scene, has none. The
  bundle's `Info.plist` needs `UIApplicationSupportsMultipleScenes` (as `examples/ios_bundle.rs`
  writes it). Without `touch`, only the first finger works, as the mouse.
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

- A see-through box (`.opacity`) is drawn again whole when what is under it changes: the
  system's text draws onto what lies under it. One over 4 million pixels keeps no layer.
  Hand-written trees (`ui.tree`) aren't striped.
- Built-in words other than month and day names and AM/PM are English until set, or with
  `translations` in 20 languages.
- Locale: Gregorian calendar only; no minimum grouping (CLDR leaves 1234 ungrouped in Spanish
  and Polish); no right-to-left marks in Arabic dates; no 0–11 hours (Japanese `K`). glibc's
  locales write 0–9 only. Linux takes a region changed while running at the next start (GTK and
  Qt too); only GNOME's 12/24-hour clock is followed live.
- Windows time axes use today's UTC offset. Uneven-height lists estimate unseen rows. Second
  windows don't show the first's meshes. Pan-zoom text under 4 units shows as a bar per line.
- Node graphs: frames don't nest or hold other frames; only the first 64 inputs of a node can
  take several wires; a wire's bends are reached by keys only while its output's node is in
  view; undo keeps a copy of the graph per step (100 steps or 100,000 nodes in all).
- Spelling: readers get a code box's errors among the lines in view only; UI Automation's
  `FindAttribute` unanswered.

## Renderers

- Layers for Direct3D 9, 11 and 12, OpenGL 3.3, OpenGL ES 3.0, Vulkan and Metal; others through
  `Surface::upload` (chapter 9). On OpenGL ES the layer's texture holds the pixels' bytes as
  they are (blue where red is read).
- Layers draw into single-sample targets; Vulkan only in a render pass (not dynamic rendering).
  Direct3D 12 and Vulkan leave their state set after `draw`.
- Lost devices: plain Direct3D 9's `Reset` is untested (it makes no device on a desktop of its
  own; 9Ex was tested); Vulkan's loss can't be simulated.
- `gpu`: see-through windows stay on the CPU.
- `hdr`: X has no HDR: X11 windows show SDR, whatever path. iOS and Android show SDR. HDR
  pictures from PNG (`cICP`) and Ultra HDR JPEGs: not AVIF or JPEG XL.
- `hdr` on Windows' OpenGL: a window keeps its pixel format for life, so a device made while
  no screen is HDR shows SDR until it is made again (a lost device, a new program).
- `hdr` under Wayland: a lit window is handed over in PQ through shared memory, Vulkan's too.
- Ultra HDR: a gain map's SDR and HDR offsets are taken as one (their mean); applied in the
  picture's own colours; with `color-profiles`, colours a Display P3 picture has outside sRGB
  are cut before they are brightened.
- `hdr` tested on HDR screens faked: this PC's are SDR, no compositor on the test bed has an
  HDR output, and the macOS runner's screen has no EDR headroom. What reaches the swap chain,
  layer or buffer is checked by number.
- `gpu-draw`: scenes, SVG drawings (their blur too: drawn once, then kept), colour emoji, fills
  of other inks and shapes of over 256 edges in 16 rows are drawn by the CPU (into patches). A
  see-through box (`El::opacity`) has the CPU draw its part, from the box's kept layer where
  that holds: the GPU has no layers of its own. GDI's
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
- `gpu` on iOS: tested only in the simulator (its Metal is the Mac's). The app switcher's
  picture taken in the background shows the last frame: nothing is drawn there. A device lost
  for real untried (simulated).
- `gpu` on Android: tested only on the emulator (SwiftShader's Vulkan and OpenGL ES: `Auto`
  takes neither, so the CPU shows there), not on a phone's GPU. The device is opened on the
  main thread as the window first shows. Swap chains have no transform of their own: on a
  turned screen the compositor turns each frame. OpenGL ES draws the whole window each frame
  (naming only what changed). Device loss simulated.
- `gpu` on the web: WebGL2 only (no WebGPU); each frame draws the whole canvas (WebGL has no
  partial present). A lost context leaves the page to the CPU for good (loss simulated with
  `WEBGL_lose_context`). Tested in headless Edge on ANGLE over Direct3D 11 and headless Chrome
  on SwiftShader (GitHub's Ubuntu runner).
- `gpu` under Wayland: Vulkan dmabufs only when asked; `Auto` keeps shared memory (cheaper on
  the one device measured). Tested only with lavapipe exporting through `/dev/udmabuf` (GitHub's
  Ubuntu runner) to Weston's GL renderer: no hardware GPU, Mutter, KWin or Sway, no tiled or
  multi-plane layout. Feedback v4 and the main device only against a simulated compositor.
  Waits for each copy before the commit (no explicit sync).

## Documents (Markdown)

- iOS, Android and the web tell each block in view its part of the selection: their readers read
  no document whole.
- Past 100 deep, nesting becomes text where blank lines times depth would pass 4 million.

## File dialog

- The library's own unless `FileDialog::native()`: Windows, or a Linux portal (v3+).
- A portal silent for 5 s falls back to the library's.
- Windows `FileDialog::native()` without a taut window (renderer, DLL) answers nothing;
  `FileDialog { native: true, ..Default::default() }` falls back anywhere.

## Text and fonts

- Vertical text: no icons or pictures among it, no underlines in it (links get one while hovered),
  no colour emoji; its text boxes are not spell checked and set no digits across. A run set across
  one em is centred, as CSS has it; Chrome sets it off centre by up to 0.1 em. A word longer than a
  column breaks where it must (CSS lets it overflow). “ ” turn as Unicode's orientation table has
  them (Chrome stands them upright in Yu Gothic). Punctuation is trimmed in any language (Chrome
  trims none in text marked English).
- Ruby in rich text with the system's text places the reading by the font's ascent and descent, not
  its typographic em box (app fonts use that). Readings in one line are each placed against their own
  base, not on one line. Horizontal text does not trim fullwidth punctuation.
- Without `shaping`, app fonts draw glyph by glyph, kerned by pairs (pages shape fully); system
  text keeps its default forms. With it, scripts shaped by syllable take language forms only
  with `brahmic` too; system text on the web takes none.
- `shaping` shapes each line the first time it is seen (15–40 µs in Segoe UI or Calibri: 0.4 s
  for a window of 10,000 new rows) and keeps up to 16 MB of runs; a full draw takes about 40%
  longer. A program that shapes no app fonts still carries the whole shaper (`hello` on
  Windows: 158 KB): the system's fonts take their forms and marks from it, as GDI and FreeType
  can't. Uniscribe could shape them on Windows, with other mark placement.
- `brahmic`: scripts with no Windows font need an app font. A bare Ubuntu has none.
- Emoji: `COLR` v1 and SVG only on request; variable emoji fonts at default; sRGB mixing; no
  flags on Windows; no emoji font, plain letters.
- IME window placement verified on Linux with a real IME, on Windows only by reading back.
- Fonts' hinting instructions run only with `hinting` (else small Latin, Greek, Cyrillic are
  auto-hinted), vertically only; at a variable font's instance about 1 glyph in 400 lands 1/64
  px off FreeType's. `VVAR`, variable `COLR` and vertical-metric variations not read. `CFF2`
  checked only with hand-made fonts.
- A fallback glyph that overhangs the face's line is clipped with `fallback-fonts` (browsers make
  the line taller); without it, not.
- Named faces are kept for the program's life; names over 128 bytes match nothing; beyond 4,096
  families or 65,536 faces, new names use the system font.
- Windows: the first letter no linked font has costs a full font scan (about 60 ms, 47 KB). SVG
  text reads `CFF` fonts whole (`glyf` fonts a glyph at a time).

## Pictures

- Refused as implausible: single-colour arithmetic JPEGs with more blocks than file bits;
  single-colour lossless WebPs over 2,048 pixels per file byte.
- Icons pick their image by size, then depth; Windows prefers a deeper image of another size.
- Lossy WebP decodes about 4× slower than an equivalent JPEG (a 1920×1080 photo: 60 ms against
  14 ms), most of it the loop filter and the boolean decoder, done one pixel and one bit at a
  time.
- SVG: images in other files only through the app's loader; drawings an image brings, four
  deep at most. SMIL as frames (`svg::encode_animation`): a minute at most, timed parts only. In
  a window (`AnimatedDrawing`): a seek back works its timeline out again from the start with the
  last 4,096 events; the pointer is over a shape's fill or stroke (masks aside, as in browsers),
  text as its whole element. A kernel unit finer than an eighth of a pixel counts as an eighth. Small blurs use a true Gaussian (as browsers; librsvg differs). SVG text is unshaped
  without `shaping`. Masks and non-box clips multiply edge coverages; two opposite subpaths of
  one shape can show a hairline. Crisp edges paint pixels more than half covered (librsvg:
  pixels whose sample point is). Of 2,986 Linux desktop icons, 392 differ visibly from librsvg
  at 1×, 185 at 3×.
- SVG cost beyond the file's budget is skipped; a small file can still take seconds at
  4096×4096. With a filter's kernel unit finer than a pixel, a picture under 512 pixels across
  may cost as much as one 512 across (a 389-byte file: 68 ms at 100 pixels).
- Highly compressible PNGs (30:1+) decode up to 1/7 slower than with an exact buffer.
- JPEG writing: YCbCr only, no restart markers, no arithmetic coding; XMP up to 64 KB (no
  extended XMP). ICO images past 256 px: Windows reads them as 256 and scales.
- `add_picture_file` includes every decoder; call one decoder (`jpeg::decode`) and
  `add_picture` to include only that one.

## Video

- Frames are drawn from the decoder's buffer only with `gpu-draw`, four videos a part (a fifth,
  or one shown more than 8 times smaller, is read back), by:
  - Windows: Direct3D 11 and 12; Vulkan where the GPU has `VK_KHR_external_memory_win32`,
    `VK_KHR_win32_keyed_mutex` and `dualSrcBlend`; OpenGL where the driver has
    `WGL_NV_DX_interop2`, through a copy on the GPU (the interop refuses a texture opened from
    another device's). A window on another adapter than the decoder's reads them back.
  - macOS and iOS: Metal.
  - Android: Vulkan and OpenGL ES; not a frame whose visible part starts off its top left.
    OpenGL ES makes it RGB by the driver's matrix (on the emulator 2 levels off the CPU's), and
    compiles the shader that reads frames when a part first shows one (20 s in the emulator's
    software OpenGL ES; not measured on a phone).
  - Linux: Vulkan and EGL, from a linear dmabuf of NV12, I420 or YV12 in one file, as VA-API's
    decoders give through GStreamer. A tiled dmabuf, or a software decoder's frame, is read back.
    Not seen here with a real decoder: the import is checked with a file GStreamer takes as a
    dmabuf (lavapipe, llvmpipe), and on GitHub's runner.
  - Not on the web: a frame is read back to the canvas, then drawn.
- Captions: their text only, without the file's positions, regions or styles. In the video
  stream (the SEI of H.264 and HEVC), read by taut: CEA-608's channel 1 of field 1 (`CC1`) and
  each CEA-708 service (`Service 1`…), tracks once their first data is read; not CC2 to CC4 or
  CEA-608's text channels. CEA-708 gives its windows' text, the highest first: not their
  colours, fonts, sizes, borders or places on the screen. After a seek, a caption shows from its
  next command on (CEA-608 and CEA-708 say only what changes). Not on the web.
- Windows: the file's own captions only from Matroska (SRT, SSA, as text): Media Foundation reads
  no caption track in MP4. Captions in the video stream come from a source reader of their own
  (the video stream read twice, up to 5 s ahead of the video). Decoded on the GPU through
  Direct3D 11 where there is one; without one, in software (MJPEG then only where Windows'
  decoder writes into a bitmap: not on every machine). AV1 needs Windows' AV1 extension (on
  the GPU where it decodes AV1). N editions need the Media Feature Pack. `open_silent` plays
  muted.
- Linux: needs GStreamer's `playbin` and `appsink` and the plugins of the file's codecs; a
  missing one is `VideoError::NoPlayer` or `video.error()`. The pitch is kept at other speeds
  with `scaletempo` (plugins-good); without it the pitch moves. The file's captions need its
  demuxer (`qtdemux` for MP4, `matroskademux`): none with base and bad plugins only; those in
  the video stream a parser that marks them on frames (`h264parse`, `h265parse`) and a decoder
  (`avdec_h264`, `avdec_h265`). No caption settings to follow: the usual style.
- macOS and iOS: `open_silent` plays muted; bytes are played from a temporary file. iOS: a
  video with sound sets the audio session's category to `Playback` unless the app set one.
  Caption tracks of files only (3GPP timed text; QuickTime closed-caption tracks, CEA-608 read by
  taut; each read by an `AVAssetReader`), not of streams; the captions in a file's video stream
  are read by taut through another (up to 5 s ahead of the video), not those of a stream (HLS).
  The file's tracks are named by their language only. Checked only on GitHub's runner and in
  the iOS simulator.
- Android: what the system's decoders read, through the NDK (no `MediaPlayer`); frames come
  through an image reader in YUV and are made RGB on the CPU (with `gpu-draw`, where the CPU
  draws them). Volume scales the samples (AAudio
  has none); other speeds are stretched by taut (WSOLA), the last 40 ms of sound not played.
  Captions: 3GPP (MP4), SRT (Matroska), WebVTT (WebM), each until the next (Matroska's durations
  not read); tracks named by their language. Those in the video stream are read from the
  samples as they are decoded. The caption style follows the system's in an app only.
  Addresses play only in an app's process. No sound device: it plays silent. Checked only on an
  emulator (software decoders); there, a command-line program that ended the moment it dropped
  its last video once aborted in Android's own decoder thread.
- Web: the browser's formats (H.264 in MP4 in most; VP8, VP9, AV1 in WebM). Sound tracks only
  where the browser lists them (Safari; not Chrome, Edge or Firefox), and the file's captions
  only where it reads them (Safari, whose text tracks also hold a QuickTime closed-caption
  track's and an HLS stream's CEA-608; not checked here); the app's captions everywhere. A page
  gets no samples: taut reads no captions in the video stream there. A page can't read the
  system's caption settings: the usual style. Sound plays only after a press on the page:
  before, `play` gives `error()`, cleared at the next `play`; muted, it plays. Another site's
  address needs CORS headers, or its frames can't be read. A path is an address relative to
  the page. Under Node there is no player.
- Tested with small made files (lossless H.264 and HEVC, MJPEG, Theora; HEVC, VP9 and AV1 made
  by ffmpeg); playback of large files not measured.

## PDF and printing

- Non-TrueType and positioned variable fonts as shapes; COLR v1 and SVG emoji as their plain
  outlines. Forms: no signatures, scripts or radio groups. PDF/UA not claimed in a reader's sense
  beyond what veraPDF checks.
- Pages from `print` (not `print_laid`) aren't laid out again for the paper picked.
- Printing tested only against stand-ins (a metafile on Windows, a private bus on Linux, a PDF
  of the print style in a headless browser).

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
