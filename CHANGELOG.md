# Changes

Changes an app can notice, newest first. Before 1.0, a new minor version (0.1 → 0.2) may need
code changes; a new patch version (0.1.0 → 0.1.1) never does.

## Unreleased

- **Text centred in its box:** a line of text in a button, menu item, list row or any box now
  has the band from its baseline to its cap height in the middle, and every line in one font
  has the same baseline. Before, a label with lowercase letters sat up to a pixel and a half
  higher than one in capitals (8 pixels above "Delete" and 10 below in a 28-pixel button at
  100%). `Metrics::lift` takes the height of the line slot instead of whether the line has
  lowercase.
- **Screen readers on iOS and Android** read text by line as wrapped and by character, word
  and selection, with each letter's place: iOS through `UIAccessibilityReadingContent` and a
  `UITextInput` for each text (iOS 18.1 and later), Android through movement granularities and
  `EXTRA_DATA_TEXT_CHARACTER_LOCATION_KEY`. A long table is a data table (iOS) or a collection
  (Android) of all its rows: a row out of view shows when a reader asks for it or scrolls a page.
- **macOS:** a long table's cell out of view comes on a reader's first ask.
- **Gallery:** in a window narrower than its top bar, as on a phone, the bar wraps: every part
  shows where Tab reaches it.
- **`examples/apk.rs`** packs any file, not only an example: `apk -- tests/android.rs`.
- **Video on iOS, Android and the web** (feature `video`), with the same API as elsewhere:
  iOS through AVFoundation, as macOS; Android through the NDK's media C API (extractor,
  system decoders into an image reader, AAudio for sound), loaded at run time, with no Java;
  the web through a `<video>` element the loader makes, frames read through a canvas, bytes
  through a Blob address. A browser that wants a press before sound makes `play` give
  `error()`, cleared at the next `play`. `video_none.rs` is gone.
- **Fixed:** on the web, the readers' element with the keyboard lost it (to the page) when the
  parts told to readers changed order; the loader now moves only elements out of place.
- `examples/web_page.rs` answers range requests (a browser seeks a video by them), and
  `examples/apk.rs` packs examples with the feature `video`.
- **iOS keyboard:** the system's keyboard reads and edits the focused text box, so it corrects,
  predicts and capitalizes; composed text stands in place of the selection. A long press or a
  secondary click in a text box shows the system's edit menu.
- **iOS files:** `FileDialog::native()` is the system's document picker: open one file or
  several (`pick_many`, all in `chosen`), a folder, or save (the person places the file, the app
  writes it). `open_with_system` opens files in the system's preview. Files, pictures and text
  drop in; `drag_out` lifts with a long press. Files on the clipboard both ways; the program's
  own copy reads back without asking the person.
- **iOS notifications:** `system_notification` and `notification_clicked`, as on macOS (app
  bundles).
- `FileDialogRequest` has a new field `many`: code building one needs it.
- **Android's system parts**, through the same calls as elsewhere, each compiled in only when
  called: the system file dialog (`FileDialog::native`; several files with `pick_many`; picked
  documents come as copies, and what the app writes to a copy goes back to its document);
  pictures and files on the clipboard; notifications, asking Android 13's permission on first
  use, clicks heard; spell checking (`check_spelling`, the system's checker); drops from other
  apps and drags out to them; printing (`print`, `print_laid`) through the print dialog;
  `open_with_system` opens the app's files; `Face::named` finds system fonts. What an input
  method composes shows at the text cursor until committed. They need the classes
  `examples/apk.rs` now packs (`classes.dex`: the activity `taut.Activity`, the file
  provider `taut.Files`), and the permission to notify, which it declares. New example
  `system_parts`.
- **SVG:** every filter primitive: `feImage` (an element or a picture), `feDiffuseLighting`
  and `feSpecularLighting` (distant, point and spot lights), `feConvolveMatrix`,
  `feDisplacementMap`; the `BackgroundImage` and `BackgroundAlpha` inputs. Text on a path
  (`<textPath>`), shaped with the feature `shaping`. SVG files as images (`data:` URLs), and
  images in other files through the app's loader (`svg::enable_images_from`), within the file's
  cost budget. `shape-rendering` after `svg::enable_shape_rendering`; clip paths and masks
  without seams after `svg::enable_seamless_clips`. SMIL animation (`<animate>`, `<set>`,
  `<animateTransform>`): `svg::encode_animation` makes an APNG for `Ui::add_animation` (a
  minute at most), `svg::decode_at` draws one moment. A program calling none of these carries
  none of them.
- **HDR on every GPU path** (`hdr`): Vulkan on Windows presents lit windows in scRGB, or HDR10
  where the GPU has no scRGB, its HDR metadata set; OpenGL on Windows in scRGB through a float
  pixel format, where a screen is HDR as its device is made; Metal on macOS in EDR, following
  the screen's headroom. X11 stays SDR.
- **Ultra HDR JPEGs:** `ui.add_picture_file` reads a JPEG's gain map (ISO 21496-1, Adobe's
  XMP, Android's Ultra HDR, Apple's) as libultrahdr does: SDR screens get the same picture as
  before.
- **Gain maps** keep a gain per colour and are sampled bilinearly, as a GPU samples a texture,
  not nearest; an Ultra HDR map's metadata says how much shows on a screen of less headroom.
- **The GPU on iOS and Android** (features `gpu`, `gpu-draw`), with the same API as elsewhere:
  iOS shows through Metal (a `CAMetalLayer` over the safe area), sending nothing to the GPU in
  the background; Android through Vulkan (`VK_KHR_android_surface`), else OpenGL ES 3 through
  EGL, the device outliving the activity's window. `Renderer::Auto` takes a GPU, not the CPU at
  work (SwiftShader). Only what changed goes up; with `gpu-draw` the GPU draws it. On Android,
  where apps get no environment, `debug.taut.renderer` and `debug.taut.draw` (`adb shell
  setprop`) stand for `TAUT_RENDERER` and `TAUT_DRAW`.
- **Layers on phones:** `layer_metal` on iOS; `layer_vulkan` on Android (`examples/vulkan.rs`
  finds Android's `libvulkan.so`); `layer_opengl` takes OpenGL ES 3.0 too.
- **JPEG:** with the new feature `rare-jpeg`, reads hierarchical JPEGs (`SOF5`–`SOF7`,
  `SOF13`–`SOF15`) and arithmetic-coded lossless ones (`SOF11`). Each plane's quantization
  table is the one at its first scan, as libjpeg takes it.
- **JPEG writing:** `jpeg::Encoder` writes progressive JPEGs, byte for byte as
  `cjpeg -progressive`, and an EXIF orientation, an ICC profile and XMP. `jpeg::encode` is
  unchanged.
- **Icons:** `ui.add_picture_file` keeps every image of an ICO; a box shows the one made for its
  size in pixels, or the nearest larger. `bmp::decode_icon_images` returns them all.
  `bmp::encode_icon` stores each image as PNG or BMP, whichever is smaller, and takes images
  past 256 pixels.
- **Lossy WebP** decodes about 30% faster (a 1920×1080 photo: 86 ms to 60 ms).
- **The system's parts on the web**, through the same calls as on the desktops, each carried
  only by a program that calls it: files and text dropped on the page; text, links, files and
  pictures dragged out; pictures on the clipboard both ways and files pasted in; the browser's
  pickers for several files (`pick_many`) and folders (`pick_folder`, read with
  `web::files_in`); saving through the save picker where the browser has one (`/saved/<name>`,
  `web::save`); notifications, clicks heard; printing through the browser's dialog; one running
  copy across tabs (`single_instance`: a later tab hands its address to the first); the
  person's fonts (`system_fonts::families`, Chrome and Edge); and with `gpu` WebGL2, with
  `gpu-draw` the GPU drawing large parts, the loader's `renderer` and `draw` options standing
  for `TAUT_RENDERER` and `TAUT_DRAW`. In Chrome and Edge a file dialog's `save` now answers
  with the file the person chose, not a download.
- **Vertical text that wraps and edits:** `ui.vertical_paragraph` and `ui.vertical_rich` break
  into columns where their height ends, as CJK line breaking allows; `ui.vertical_text_box` and
  `ui.vertical_text_area` take typing: Up and Down move along a column, Left and Right across
  columns, an input method composes at the cursor, and screen readers read by column with each
  character's place. Fullwidth punctuation beside punctuation is set half width, as Chrome does.
- **Tate-chu-yoko:** `ui.upright_digits(n)` sets runs of up to `n` digits (at most 4) across one
  em in vertical text; `Span::upright` (feature `ruby`) any piece.
- **Ruby** (feature `ruby`): `Span::ruby(reading)` sets a reading over rich text (`ui.rich`) and
  beside vertical text, as CSS Ruby Layout and Chrome set it. Readers hear it after its base, in
  brackets.
- **Locales write as the place does:** `Locale` has six more fields: `date_digits`,
  `am_pm_place` (`AmPmPlace`), `hour_digits`, `time_separator`, `group_sizes` and `zero_digit`.
  A `Locale { .. }` written out in full needs them; one ending in `..Locale::PLAIN` does not.
  An app's own locale writes as its fields say (before: groups of three, two-digit days,
  months and hours, ":").
  - Digits of other scripts (Arabic, Persian, Devanagari, Bengali, Thai, Myanmar…) in numbers,
    dates, times, calendars, sliders and number and time fields; typed in them or in 0–9.
  - AM/PM before the time where the place puts it ("오후 2:05", "下午2:05"); a time box then has
    that field first.
  - Read from the system: Windows' native digits (always, or "by context" where the display
    language reads right to left) and AM/PM's place; glibc's and Foundation's AM/PM place;
    Foundation's group sizes, digits, and month and day names and AM/PM (before: English).
- **`Locale::named("hi-IN")`:** CLDR 49's formats for 34 places, years in full. In a program
  only when called.
- **Locale changes followed** on macOS and iOS (`NSCurrentLocaleDidChangeNotification`),
  Android (configuration changes; the 24-hour setting on resume) and Linux (GNOME's 12/24-hour
  clock through the portal, which overrides the locale's). Android reads the locale through
  `java.text` and ICU (before: the plain formats).
- **Linux screen readers set number and time fields** by value (AT-SPI `Value`, as near as the
  range allows) and by text (`EditableText`).
- **`translations` in 20 languages:** Arabic, Dutch, Hindi, Indonesian, Italian, Korean,
  Polish, Russian, Swedish, Thai, traditional Chinese, Turkish, Ukrainian and Vietnamese added.
  Their month and day names and AM/PM are CLDR's. The feature costs what six languages did
  (35 KB on Windows, 37 KB on Linux): each language is one text.
- **Sticky columns:** `El::sticky_x` sticks a box at the left of the nearest box scrolling
  sideways, as `sticky` does at the top. Each sticks in the nearest box that scrolls its way,
  the page too; `frozen` and `sticky` together stick in the next box out (a table's header on
  a long page); a box may stick both ways.
- **The page scrolls sideways** where what is on it is wider than the window: wheel, bar, Left
  and Right with nothing focused, and screen readers asking to see a part. A program whose page
  ran past the window's right edge now gets a bar there.
- **See-through boxes are kept:** an `El::opacity` box is drawn once into a layer and, while
  neither its content nor what is under it changes, blended again from it: a fade redraws none
  of its content. With `gpu-draw`, the GPU lays that layer; before, such a part was the CPU's.
- **Striped rows** in lists, tables and tree views: `ui.striped_rows(color)` before them. A long
  list's `rows` in a box with `El::stripes` take the stripes over, counted from the first row;
  rows kept in view (`frozen`) are passed over.
- **Pasting into tables:** `ui.paste_adds_rows(&table)` lets a paste run past the last row (its
  cells come in `cell_changes`, the app adds the rows), and an empty table takes a paste.
  `ui.refused_cells(&table)` gives the values a column refused; their cells are outlined in red
  until the table next changes.
- **Markdown documents:**
  - Laid out only as far as seen: a 10,000-line document's first show costs a screenful, the
    rest's height is estimated until it scrolls into view. Links get boxes where they show
    (and the one before and after, for Tab).
  - Hebrew and Arabic paragraphs read right to left across style pieces and start at the
    right, as `dir="auto"` does in a browser; so does `ui.rich`.
  - Wide code blocks scroll sideways (wheel, or Left, Right, Home, End once Tab reaches them)
    instead of wrapping.
  - Tables are tables to screen readers (headings, rows, cells); the selection is told in each
    block it covers; a link's title shows by it on hover or focus and is told as its
    description.
  - A context menu (copy, select all, copy link: `ui.edit_words` has a seventh word, so an
    app that sets all of them needs it; translated with the others), and the view
    scrolls while a selection is dragged past its edge.
  - Lists and quotes nest as deep as written; the 100-deep limit holds only where many blank
    lines would make parsing slow.
- **`AccessNode::description`** and **`ui.tell_description`**: what an element says more than
  its name, told as UI Automation's help text, AT-SPI's description, `accessibilityHelp`,
  Android's tooltip text and `aria-description`.
- **Video captions, tracks and speed** (feature `video`): `Video::sound_tracks` and
  `caption_tracks` (`Track`: language and name) list the file's tracks, `set_sound_track` and
  `set_caption_track` choose them (captions start off), `add_captions` adds the app's WebVTT or
  SRT (`CaptionsError` where it holds no cue), `caption` is the one on show, and `set_speed`
  plays at 0.25 to 2 times, the pitch kept where the system can. `ui.video_captions`, after
  `ui.video` in a stack, draws the caption over the video in the system's caption style and has
  screen readers say it. `ui.video_controls` gains Captions, Sound track (with two or more) and
  Speed menus: Tab now passes Speed between the time and Mute.
- **Video on Windows decodes on the GPU** (Direct3D 11, DXVA) where there is one: HEVC and VP9
  on the GPU's decoder; MJPEG also where Windows' decoder takes only a texture, not a bitmap.
  Frames match software's within 2 levels of 255.
- **Thai, Lao, Khmer and Burmese lines break between words everywhere** (feature `brahmic`;
  on Linux, Android and the web also the new feature `word-lists`): CoreFoundation's tokenizer
  on macOS and iOS; on Linux, Android and the web ICU 78's word lists (804 KB) walked as ICU's
  dictionary break engines walk them, matching ICU on 48,991 texts. The crate's licence adds
  BSD-2-Clause and BSD-3-Clause for the Lao and Burmese lists, whose notices a program built
  with `word-lists` must carry (`THIRD-PARTY.md`).
- **Marks in fonts without `GPOS`** sit on their letters, placed by combining class as
  HarfBuzz's fallback places them (1,269 word-font pairs alike, HarfBuzz 12.3.2 and 14.5).
- **The system's text in a language's forms** (`shaping`) also in lines with marks or letters
  of fallback fonts (shaped in their fonts, as right-to-left text is), and on macOS and iOS.
- **Fallback glyphs** (`fallback-fonts`, now also on Linux, macOS and iOS): on Linux a fallback
  font with a taller line is sized to the face's line, as on Windows; on Windows, Linux, macOS
  and iOS what a fallback glyph still overhangs is clipped to the line, so nothing is left
  behind on redraw.
- **SVG text** (`shaping`) is shaped in runs of one script (a Latin word and a Hindi one in one
  font were shaped as one script), a grapheme cluster in one font, and a family's own font is
  the one it draws a space in (a family without Latin letters fell back for its spaces).
- **Fixed (Windows, feature `gpu`):** a window a GPU path shows draws what changed even where
  Windows drops it from the window's update region and sends no paint (seen while another
  thread of the program painted): the window keeps such places and draws them on a message of
  its own.
- **Fixed (Linux):** windows opening on several threads at once could leave the program's
  `LC_CTYPE` changed for good. The library's brief changes of it now take turns.
- **Fixed (web):** the app's own drag let go on the page was sometimes lost: the drop is now
  handled at once, before the browser ends the drag.
- **Second windows on iPad**, each a scene of its own: asked for as the app first describes it,
  closed as it stops; the person closing the scene is a close request. Touches, keys, drops,
  drags out and screen readers go to the window they are in; typing and the system's dialogs
  to the window in front. An iPhone shows none, as before. `examples/ios_bundle.rs` writes
  `UIApplicationSupportsMultipleScenes`; its `newest_iphone()` is `newest_simulator("iPhone")`.
- **Fixed:** on Android, an input method deleting around the cursor cut an emoji in half and
  took an accented letter whole where it asked for the accent. It now deletes what the input
  method counts, in UTF-16 units (`deleteSurroundingText`) or code points
  (`deleteSurroundingTextInCodePoints`), whole characters, beside the selection, as one undo.
- **Fixed:** on the web, fonts refused by Chrome or Edge (which list none until the person
  allows it) were never asked for again; now the next press asks.
- **Fixed (Android, `gpu`):** a window the GPU gives up (too many devices lost, or none fits) is
  handed back to the CPU in RGBA. It was left at the activity's RGB 565, and the CPU wrote
  32-bit pixels into it. Vulkan swap chains take the window's own RGBA where offered.
- **Fixed (Windows, `hdr`):** while a window is lit, a change of its screen's SDR white or peak
  shows it again, and Vulkan sets its HDR metadata again. Before, the metadata was set only when
  the swap chain was made, and the frame waited for the next change.
- **macOS (`hdr`):** EDR headroom changes come from AppKit's notice; the polling every 250 ms
  while a window is in EDR is gone.
- **Fixed (`hdr`):** an Ultra HDR JPEG with a start of a JPEG that can't be measured before its
  gain map is read with its map, not as SDR.
- X11's and Android's Vulkan paths share one module, their EGL paths another. System libraries
  are opened at run time by one loader, EGL and OpenGL ES on Android too (no longer linked);
  every function table is filled by one loop.
- **Bidi by paragraph:** a wrapped line of mixed-direction text is ordered as its whole paragraph
  orders it (UAX #9), so numbers and punctuation at a line's ends take the direction they would
  unwrapped. `TextSystem::places` and `TextSystem::draw_line` take a `Paragraph` (its
  `right_to_left` and the text around the line) in place of `leftward: bool`;
  `Paragraph::whole(right_to_left)` is the old meaning.
- **Shaped forms drawn by the system:** with `shaping`, a line shaped for its language's forms
  in the system's fonts is drawn by GDI, FreeType or CoreText glyph by glyph
  (`TextSystem::draw_glyphs`), not by taut's rasterizer: its letters look like the system's own
  text. `hello` with `shaping` is 24 KB smaller on Windows, 7 KB on Linux.
- **SVG text on Windows** reads a system font's glyphs as it draws them: a page of CJK text
  peaks at 6 MB, not 51 MB.
- **Vertical text:** marks in fonts without `GPOS` are placed as HarfBuzz places them; full
  stops, commas, colons and quotation marks are trimmed by the kind their glyph shows in the
  font, as Chrome does; a column broken inside a word no longer overshoots by half an em after
  a trimmed mark; Chinese text may break before 〜 and ゠.
- **Line breaks:** a no-break space after a hyphen may start a line (UAX #14 LB12a).
- **SVG animation in a window:** `svg::AnimatedDrawing` plays a file's SMIL animation in a box
  (`ui.animated_drawing`), only that box redrawn per frame. Clicks, presses, pointer moves over
  its shapes and `accessKey(x)` begin and end what waits for them; the app can seek, pause,
  begin or end an element and send events. `animateMotion` (a path, an `<mpath>` or points,
  `keyPoints`, `rotate`); animations begun and ended by others (`a.end+1s`, `a.begin`,
  `a.repeat(2)`), several begins, `restart`, `min` and `max`, here and in
  `svg::encode_animation`. A `set` with `dur` ends after it (it held for ever).
- **SVG filters:** `FillPaint` and `StrokePaint` inputs, and `kernelUnitLength` on lighting and
  convolution, as librsvg draws them.
- **iPad scenes the system lets go of keep their windows.** UIKit may disconnect any scene to
  free memory and bring it back later with the same session. Before, that closed a second
  window, and the first scene coming back closed the main window's session. Now the window
  keeps its view, state and id and moves into the scene's new `UIWindow`; typing into a focused
  text box carries on. Only a discarded session (the person closing the scene, or the app) is a
  close request; for the first window too: the program ends, or, kept open (`cancel_close`), the
  window asks for a new scene.
- **Fixed (iOS):** a second window whose scene request UIKit dropped (one made just after a
  scene closed) never showed: a scene not connected 10 s after it was asked for is asked for
  once more. What a closed window's app takes dropped was kept for good. The first window's new
  scene, after the person closed its scene and the app kept it open, is asked for once more
  the same way.
- **Fixed (iOS):** on an iPad showing one scene at a time, an app closing the second window in
  front went to the background, and the system soon stopped it. The first window's scene now
  comes forward.
- **Documents jump far cheaply:** Ctrl+End, a link to a far heading, the scroll bar dragged to
  the end, a width change and Tab to a link out of view lay out only around where they go,
  estimating what lies between and correcting it as it comes into view; what shows stays in
  place. A jump to the end of 10,000 lines: 110 ms → 1 ms, held heap 3.4 MB → 0.5 MB.
- **Documents right to left:** a list or quote whose first letter is Hebrew or Arabic has its
  markers or bar at the right, on screen and on paper (`dir="auto"`, as Chrome draws it).
- **Screen readers:** the part of a document's selection in a table cell is told in that cell,
  as in other blocks; a cell is named by its words as one text. Cells and column headings are
  read as text by line and letter (UI Automation's text pattern, AT-SPI's Text, macOS, the web).
- **Code editor on Android:** an input method's deletes keep each selection, as in text boxes;
  the code editor takes the system's keyboard without a text box beside it.
- **Fixed:** the system's text on Linux and macOS kept every glyph picture it drew, so text
  drawn at every size (a pan-and-zoom view) grew memory without end. Each of its glyph caches
  now holds at most 8,192 pictures, as on Windows, and macOS at most 256 fonts.
- **Video frames drawn from the decoder's texture** (Windows, `gpu-draw`, Direct3D 11): a frame
  Media Foundation decodes on the GPU is drawn from its texture, shared with the window's device
  under a keyed mutex; it is read back only where the CPU draws it. A 1080p H.264 video took
  2.6 to 3.9 ms of CPU a frame instead of 4.3 to 7.6.
- **Captions in the video stream** (CEA-608 in H.264, feature `video`): a caption track `CC1`,
  found once its first data plays. Windows reads them through Media Foundation's timed text;
  Android and Linux through taut's own decoder (from the extractor's samples, put in display
  order; from GStreamer's caption meta).
- **Windows video:** Media Foundation starts once for the program, not per video (40 opens now
  take half the time); the device videos decode on is made again after a GPU reset (before, every
  video after one failed). Media Foundation keeps the pitch at other speeds: checked at 0.5, 1.5
  and 2 times.
- **Fixed:** a playing video asked for its next frame once per gap between the frames it took,
  so one late frame made it miss every other frame from then on (a 30 fps video showed 10 fps).
  It now asks twice per gap.
- **SVG animation in a window (`AnimatedDrawing`)** keeps its timeline as it plays and adds
  each event to it: an event costs the same after an hour as at the start (the same schedule as
  before). None is forgotten: before, a file waiting for `mousemove` lost an early `click` after
  4,096 moves, and what it began undid itself. A seek back works the timeline out again with
  the last 4,096 events. Each event also allows a little more scheduling work, so a pointer
  moving at frame rate no longer uses up a minute's budget.
- **SVG in a window, as in browsers:** the pointer counts only within clip paths and viewports
  (not masks); an event at what a `<use>` brings bubbles through the `<use>`, not where the
  element lies in the file; `focus`, `blur`, `focusin` and `focusout` come as focus moves (a
  press gives it to the nearest element with a `tabindex`, a link or a focus event waited for;
  by keyboard, the first Tab reaches), and the box takes focus where the file waits for them.
  `wallclock()` begin and end times count from when the drawing first shows.
- **SVG filters:** a pattern as a `FillPaint` or `StrokePaint` input tiles the filter region,
  as librsvg does at the file's size (librsvg does not scale it with the drawing; taut does). A
  kernel unit finer than a pixel runs on a finer grid, as librsvg does, down to an eighth of a
  pixel (before: one pixel); its cost counts the cells in a picture 512 pixels across.
- **Fixed (SVG in a window):** finding the element under the pointer looked through every clip
  and every clip outline for each shape there: 170 ms a pointer move over 8,000 shapes clipped
  each on its own (a 1 MB file). Each shape's own clips are looked at now (3 ms), and reading
  such a file for the pointer is a third faster.
- **Markdown links and the keyboard:** Tab and Shift+Tab wrap round a document's links (Shift+Tab
  from the document reaches its last link, however far). With a link focused, Shift+arrows,
  Shift+Home/End and Ctrl+A select as with the document focused, and Up or Down with no link that
  way scroll a line: each in one run (before, arrow navigation scrolled 20 px a run).
- **SVG drawings in a window** (`AnimatedDrawing`): Tab and Shift+Tab move between the drawing's
  elements in `tabindex` order, as Edge does, and leave it at its ends; where no shape is, the
  root `<svg>` has the pointer (its `click`, `mouseover` and the rest).
- **PDF:** a right-to-left list number, drawn `.1`, reads and copies as `1.` (`ActualText`).
- **Android:** the code editor tells input methods it holds code: no suggestions, no capitals
  (`Ui::code_focused`, new, beside `password_focused`). Fixed: the first box focused with the
  keyboard down kept the input type the input method was given before (plain text).
- **Video frames drawn from the decoder's buffer on Apple, Android and Linux** (`video` with
  `gpu-draw`), as on Windows' Direct3D 11: Metal reads AVFoundation's pixel buffer (on an
  IOSurface); Vulkan and OpenGL ES import Android's hardware buffer; Vulkan and EGL import a
  Linux decoder's linear dmabuf (GStreamer is now offered dmabufs of NV12, I420 or YV12 before
  BGRx). No frame is read back while it plays; the CPU reads one only where it draws it. Vulkan
  reads each frame's bytes through a Y′CbCr conversion and makes them RGB with the CPU's own
  numbers: the same pixels.
- **Captions in the video stream, everywhere but the web** (feature `video`): CEA-708 caption
  services are read too, each a track `Service 1`, `Service 2`… after `CC1` (windows, their pen,
  delays, word wrap, the four print directions; text only), from H.264's and from HEVC's SEI.
  macOS and iOS read them now (and QuickTime closed-caption tracks, as `CC1`); Windows reads
  them through a source reader of its own instead of Media Foundation's timed text; Linux also
  from `h265parse` and from caption distribution packets.
- **Video frames drawn from the decoder's texture on Direct3D 12, Vulkan and OpenGL** (Windows,
  `gpu-draw`), as on Direct3D 11: Direct3D 12 opens the texture and waits for the decoder's
  fence; Vulkan imports it (`VK_KHR_external_memory_win32`) and takes its keyed mutex in the
  submit (`VK_KHR_win32_keyed_mutex`); OpenGL copies it on the GPU and draws the copy through
  `WGL_NV_DX_interop2`. A device that can't open a frame reads frames back from then on. CPU a
  1080p H.264 frame (RTX 2080 Ti): Direct3D 12 2.2 ms instead of 5.9, Vulkan 1.5 instead of 6.0,
  OpenGL 2.7 instead of 7.7. The decoder's texture is now shared by an NT handle.
- **Markdown selection told whole:** on UI Automation, AT-SPI and macOS a document is read by
  all its text (kept once per parse and shared, not copied a run) with its selection, however far past the view it
  runs. Before, only blocks in view told their part.
- **Markdown code right to left:** a code block naming its language reads as its first strong
  letter does, all its lines at the right (GitHub's `dir="auto"`); one naming none stays left
  to right. A wide one comes into view scrolled to where its lines start. A code block is as
  wide as its widest line (before: all its lines end to end, so it scrolled sideways far past
  its text).
- **Tables:** undoing a paste removes the rows it added: `ui.removed_rows(&table)` hands them
  to the app once; redo adds them again.
- **Spelling in the code box:** `ui.check_spelling` before `ui.code_editor` checks its comments
  and strings, by its language's rules; corrections from its menu, undone as one.
- **Pan-zoom text too small to read:** in a view zoomed far out, text under 4 units shows as a
  faint bar per line, where before it drew nothing.
- **Video frames on the GPU** (`video` with `gpu-draw`): every path keeps a decoder's buffers
  imported by one rule: while the video lives, the 8 most recently drawn at most (Linux and
  Android kept a closed video's 8). A device that can't import a frame draws frames read back
  from then on; Linux and Android tried again at every draw, and Vulkan lost the device each
  time. Vulkan on Linux and Android draws frames of two kinds in one part (NV12 beside I420)
  instead of leaving the part to the CPU. On Windows the decoder no longer waits for a texture
  a window's device holds: each try held the interface up to 100 ms.
- **Captions where the file puts them** (feature `video`): `ui.video_captions` draws each
  caption at its place on the frame: CEA-608's rows and indents, CEA-708's window anchors and
  justification, WebVTT's `line`, `position`, `size`, `align` and regions, SRT's `{\an8}`, SSA's
  alignment, margins and `\pos`; italic where the file says. Screen readers are told the text as
  before. `add_captions` also takes SSA (ASS) scripts. Captions take the size the caption
  style asks against the frame (an 18th of its height): before, the theme's text size, as the
  video's box was not known. That box now senses the pointer (so its place is kept).
- **CEA-608's other services:** `CC2` to `CC4` and the text services `T1` to `T4` are caption
  tracks once their data comes (in the video stream, and QuickTime closed-caption tracks on
  macOS, iOS and Windows).
- **Windows:** MP4's and QuickTime's caption tracks (3GPP timed text, WebVTT, `c608`), which
  Media Foundation does not read, are read by taut. An `http` or `https` address is downloaded
  once for the player and the caption readers (before, the captions' reader made a connection of
  its own and read the video again); the video opens once the address's first bytes come.
- **SVG, where subpaths meet:** two subpaths of one shape that meet inside a pixel leave no
  hairline, as in browsers: such a pixel's coverage is worked out from the winding along 64
  lines across it. Shapes of one subpath are drawn as before.
- **SVG clip paths clipped by clip paths** (`clip-path` on a `<clipPath>`) keep only where
  both do, as Edge draws them; a reference that comes back round is dropped, as Edge drops it.
  Before, the inner clip path was ignored.
- **SVG pictures shown smaller** are, at each pixel, the mean of the picture under it, as
  librsvg and browsers show them (exact where square to the drawing). Before, a whole-number
  shrink and a bilinear sample blurred them over about two pixels.
- **`svg::enable_entities()`** writes out the entities a file's doctype declares, as XML
  readers do (some drawing programs name every style by one), at most four times the file's
  length.
- **`AnimatedDrawing`:** a seek back is exact for any number of events (before, the last
  4,096): its timeline goes on from the latest of up to 32 copies kept as it played, and each
  time an event gave an animation is kept (12 bytes).
- **Windows, feature `cff`:** system fonts with PostScript outlines (`CFF `, `CFF2`) are read a
  charstring and subroutine at a time as drawn, as `glyf` fonts are: a page of CJK SVG text in
  Noto Sans CJK peaks at 6.7 MB, not 17.8.
- **Video on macOS and iOS:** the first frame shows as soon as AVFoundation has it, without
  waiting for it to say the first seek is over (up to 9.8 s later on the iOS simulator; at most
  0.07 s now).
- **`Video::position`** is never past `length()`: AVFoundation's clock could stop at 3.09 s of a
  3 s video.
- **CI failures readable without a login:** every test step runs through `.github/ci.rs test`,
  which makes each failed test, with its panic message, a GitHub annotation (crashes and other
  failures too). Flaky tests fixed: `tests/renderers.rs` crashed (0xC0000005) on NVIDIA, as
  `opengl_puts_back_what_it_sets` uploaded its own picture with rows and pixels still skipped
  and OpenGL read past its end; the video memory check measures only once memory stops growing
  (some 200 opens on Windows); `tests/video_apple.rs` shuts down the simulator it booted, which
  kept the Mac busy and its frames late in the steps after.
- **Fixed (Windows video):** a seek made as a video pauses sometimes never showed its frame.
  Media Foundation can end such a seek with no frame (one in eight on a computer with no GPU).
  The player now seeks elsewhere and back for it, its position held. A caption track Media
  Foundation tells of after the first frame (at times a file's second) now shows in
  `caption_tracks()`: the window wakes, and a paused video asks until every track told is listed.

## 0.3.0 (2026-11-24)

Code written for 0.2 builds unchanged; to move up, change `taut = "0.2"` to `taut = "0.3"`.

- **The web:** taut builds for `wasm32-unknown-unknown` and runs in a page's `<canvas>`, with
  no crates (no wasm-bindgen, no web-sys). `taut::web::LOADER` is the loader, a JavaScript
  module served beside the program; `examples/web_page.rs` writes a page and serves a built
  program. The CPU draws as everywhere; changed places go up through `putImageData`, a frame
  only when something changed. Mouse, wheel, touchpad pinch, fingers and pens, keys, focus,
  cursor shapes, resizing; typing and input methods through a hidden text field; copy and paste
  through the browser's clipboard events. Text is the browser's (`fillText`), or the font files
  the page gives the loader. Screen readers read hidden elements with ARIA roles, names and
  states, kept in step by sending only what changed. `storage` keeps files in `localStorage`;
  the system file dialog is the browser's picker (`web::picked`); saving downloads
  (`web::download`). `taut::run` returns at once there. New example `web_notes`.
- **`platform::unix_time()`:** seconds since 1970 by the system's clock, in a browser too
  (where `SystemTime::now()` panics).
- **iOS and iPadOS:** `taut::run` opens a UIKit window filling its scene (`aarch64-apple-ios`,
  `aarch64-apple-ios-sim`). Text by CoreText; the safe area and the keyboard kept clear;
  rotation and iPad multitasking sizes; dark mode, Dynamic Type, more contrast, less motion.
  Fingers are `Event::Touch` with `touch` (else the first finger is the mouse); Apple Pencil,
  trackpad and mouse are the mouse; hardware keys, input methods and the system's edit actions
  work; screen readers read `UIAccessibilityElement`s. Going to the background is
  `Event::Minimized`. `run` never returns there. `examples/ios_bundle.rs` builds an example as
  an app for the simulator or a device.
- **Tagged PDF** (PDF/UA-1): a `Paper` has a structure, as a window tells screen readers:
  `Paper::begin(Role)`, `begin_heading`, `begin_decoration`, `describe`, `end`. Reading order,
  alt text, table column headings, language (`Paper::set_language`). `Paper::markdown` tags
  every block and repeats a table's headings on each page; `Paper::interface` describes its
  picture by what the window tells readers. Headings make the outline (bookmarks).
- **PDF/A-2b** (`Paper::archive`), **form fields** (`text_field`, `tick_field`,
  `choice_field`) and **AES-256 encryption** (`Paper::encrypt`): each carried only by programs
  that call it.
- With `emoji`, colour emoji go into PDF in colour (COLR layers as shapes, bitmaps as pictures),
  and print so; JPEG and PNG
  (`Paper::png`) keep their colour profiles; a font's licence (`fsType`) decides how it goes in.
- **Printing:** `taut::print_laid` lays pages out for the paper picked. Windows prints shapes and
  text, needs no active window, and prints a page larger than the sheet on several sheets
  (`Paper::shrink_to_fit` shrinks it instead, as before). On Linux the portal's dialog shows
  over the program's window.
- **Metal on macOS:** with the feature `gpu`, Metal shows the windows (a `CAMetalLayer`, only
  changed places uploaded), byte for byte the CPU's pixels; with `gpu-draw` it draws them too,
  as the other paths do. `Renderer::Metal`, `TAUT_RENDERER=metal`; `Auto`, `Gpu` and any path
  named mean Metal there. A lost device is made again, else the CPU shows; a window on a screen
  of another GPU moves every window to it. Without the features nothing of it is compiled in.
- **Fixed:** `layer_metal` asked Metal for a texture wider or taller than 16,384 pixels, which
  ends the program; it now returns an error.
- **Video:** the feature `video` plays video and sound (files, addresses, bytes) through the
  system's player, loaded at run time: Media Foundation, GStreamer, AVFoundation. `Video`
  opens, plays, pauses, seeks, loops and tells the end; `ui.video` shows its newest frame,
  redrawing only its box; `ui.video_controls` adds play, position, time, mute and volume, for
  keyboard and screen readers. Example `player`. Without the feature nothing of it is
  compiled in.
- **Screen readers read text by line as wrapped**, and get each letter's place: UI Automation
  (line unit, `GetBoundingRectangles`, `RangeFromPoint`, visible range), AT-SPI
  (`GetStringAtOffset` by line, `GetCharacterExtents`, `GetRangeExtents`,
  `GetOffsetAtPoint`), macOS (`AXLineForIndex`, `AXRangeForLine`, `AXBoundsForRange`,
  `AXRangeForPosition`). Plain text, headings and paragraphs are read so too, not only text
  boxes.
- **Long tables:** a reader reaches any row through the table (it scrolls into view) and selects
  rows through it (UI Automation `Selection`, AT-SPI `AddRowSelection`/`RemoveRowSelection`).
  **`ui.tell_selection(&selection)`** inside a table tells every selected row, in view or not.
- **Misspellings** are told for all of a box's text (up to 64 KB), not only visible words; UI
  Automation's `FindAttribute` finds them.
- Scrolling in or out of view is told on Windows too (`IsOffscreen` changes).
- Tree items an app put in no tree (`ui.tree`) are told in one: Orca reads their levels.
- A scroll costs less while a reader listens: providers follow a move instead of copying the
  list, and the walk finds each box's role without a search.
- **HDR:** the feature `hdr` (with `gpu`) shows boxes and pictures brighter than SDR white on
  HDR screens. `El::brightness(times)`, `Ui::add_hdr_picture` (linear light) and PQ or HLG
  PNGs (`cICP`) give the light; `Ui::headroom` says how much the screen shows. Direct3D 11 and
  12 present such windows in scRGB, Wayland in PQ through its colour management; the screen's
  SDR white and peak are followed. Everything else stays at the system's SDR white, and SDR
  screens get the same pixels. Without the feature, nothing of it is compiled in.
- **Node graphs:** inputs that take several wires (`Node::multi_input`, `Graph::wires_into`);
  a grid, drawn and snapped to (`Graph::grid`); frames that carry the nodes in them
  (`Graph::frames`, `Frame`; Ctrl+G frames the selection); bends on wires
  (`Graph::reroutes`, `Reroute`; double-click a wire); undo and redo of every edit (Ctrl+Z,
  Ctrl+Y, Ctrl+Shift+Z; `Graph::undo`, `redo`, `save_undo_step` for the app's own edits); a
  pressed node under another comes on top (to the end of `nodes`); dragging near the view's
  edge pans it; F shows the selection. Wires are buttons readers hear by both ends, reached by
  Tab after a node's ports (Enter follows, B bends, Delete removes). New actions:
  `Rerouted`, `Framed`, `Undone`, `Redone`, `WireDropped`. `Graph::unwire` removes a
  wire with its bends; `Graph::snapped` puts a point on the grid.
- `GraphWords` has six more words (`many`, `wire`, `bend`, `frame`, `undone`,
  `redone`): one written out in full needs them (`..GraphWords::default()` does not).
- **Android:** `taut::run` opens the window of a `NativeActivity` (Android 11 and later). The
  app exports `android_main`, which calls `main`; `run` returns and the interface runs on the
  main looper. CPU frames into the window (`ANativeWindow_lock`) inside the system bars,
  cutout and soft keyboard; touch, mouse, pen, keys and the back button; the soft keyboard and
  its text; the clipboard's text; dark mode, accent colour, text size and animations off from
  the system; Roboto and the system's fallback fonts from `/system/fonts`; screen readers
  through an `AccessibilityNodeProvider`. The few Java classes this needs are written as a DEX
  file at run time; the app has no Java. `examples/apk.rs` packs an example as an APK.
- **TrueType hinting** (feature `hinting`, off by default): small text of a TrueType font that
  carries hinting instructions is hinted by them, as FreeType's default interpreter (version
  40) does: vertically only, close to Windows' ClearType natural mode. The same points as
  FreeType 2.14.3 for every glyph of 19 Windows and Linux fonts at 8–20 px. Variable fonts vary
  their control values (`cvar`). `AppFonts::autohinted()` keeps the autohinter. Fonts without
  instructions, and every font without the feature, are hinted as before.
- **Shaped app fonts** (feature `shaping`): text in app fonts is shaped as on pages:
  ligatures, contextual forms, marks placed and stacked on their letters, kerning beyond pairs.
  A line is shaped once and kept; glyphs still sit on whole pixels. The system's text (GDI,
  FreeType) takes the window's language forms (`locl`): Serbian б, Romanian ș. Without the
  feature nothing changes.
- **Shaping:** random alternates (`rand`) as HarfBuzz picks them; Syriac's abbreviation mark
  stretched over its word (`stch`); a font's positioning under `calt`, `liga` and the other
  default features (Bahnschrift raises a hyphen between capitals). Pages change where fonts
  have these.
- **Split cursor:** where right-to-left and left-to-right text meet, the cursor shows at both
  places, half high each (`right-to-left`).

## 0.2.2 (2026-11-24)

- **`ui.plot_lines`** draws a smooth curve through the values (`Path::smooth_through`), not
  straight lines from one to the next. Between two values it may bend a little past them, as a
  curve does; a chart line that must not is `Series::smooth`.
- **Fixed:** under Wayland, a program that ends reads what the compositor last said before it
  hangs up (one round trip, at most 250 ms), so the compositor no longer finds the connection
  reset and logs "failed to read client connection".

## 0.2.1 (2026-11-18)

- **Drawn by the GPU:** the feature `gpu-draw` (with `gpu`) has every GPU path draw windows,
  not only show them: Direct3D 11 and 12, Vulkan (Windows, X11, Wayland) and OpenGL (Windows,
  X11). Boxes, rounded corners, borders, shadows, glyphs, icons, GDI's text, pictures,
  gradients, filled shapes and strokes. Scenes, SVG drawings and colour emoji are drawn by the
  CPU into patches the GPU lays in; what neither can (see-through boxes) has the
  CPU draw that part. Within a few levels of 255 of the CPU at edges; GDI's text on a solid
  colour is GDI's own pixels. `Renderer::Auto` draws only large parts on the GPU; a named path
  draws all. `TAUT_DRAW=cpu|gpu|auto` overrides. Nothing breaks; without the feature nothing of
  it is compiled in.
- **Fixed:** a scrolled box could keep redrawing on Linux in optimised builds (its offset flipped
  between 0.0 and -0.0).
- **Renamed, old names kept** (deprecated, removed in 0.3):

  | 0.2.0 | 0.2.1 |
  | --- | --- |
  | `Canvas::held_to` | `Canvas::with_clip` |
  | `Canvas::ink` | `Canvas::fill_mask` |

- **Documents:** words left from before 0.2 are gone (`set_screen_reader`, `set_full_screen`,
  `Event::Zoom`, `AppFonts::unhinted`, the `gpu` feature's list of GPU paths). The guide's
  "Kept between runs" is now "Saving settings between runs", and shows
  `Window::with_placement`, not the old `placed`.
- **`examples/walk.rs`** checks only what shows in a window, fails on a missing or one-colour
  picture, on stale drawing (Linux) and on a demo that does not end when closed, and counts
  its checks.
- **macOS:** taut builds and runs there. AppKit windows (several, frameless, see-through,
  always on top, placed and sized, Retina), drawn by the CPU and shown by Core Animation;
  mouse, wheel and touchpad, pinch, keys, text input with input methods, cursors, focus;
  CoreText text with its fallback fonts; the clipboard (text, pictures, files); files and text
  dropped in; `FileDialog::native` as a sheet; dark mode, accent, contrast, reduced motion,
  screens, locale; screen readers through NSAccessibility (text ranges, tables and trees too);
  tray icon, global hotkeys, single instance; notifications for app bundles; spell checking
  (`NSSpellChecker`); dragging out to other programs; what an input method composes shown at
  the cursor; `emoji` from Apple Color Emoji; `layer_metal`, the UI over a program's Metal
  rendering. Gaps: [what is missing](docs/what-is-missing.md).
- **GPU under Wayland:** with `gpu`, a window asking for a GPU path (`Renderer::Vulkan`, `Gpu`,
  another named, or `TAUT_RENDERER`) is shown through Vulkan: its pixels go to the compositor as
  dmabufs (`zwp_linux_dmabuf_v1`), byte for byte the CPU's. `Auto` keeps shared memory. Any
  failure falls back to shared memory.
- **Locale:** while `ui.locale` is `None`, dates, times and numbers take the system's digit
  counts (3/7/2026), time separator and hour digits, and group sizes (12,34,567); month and day
  names and AM/PM the app left English are the system's (`CalendarWords::of_the_system()`).
  Windows region changes are followed while running.
- **Translations:** the feature `translations` puts every built-in word in German, Spanish,
  French, Japanese, Portuguese or Simplified Chinese, from the user's language;
  `ui.translate("de")`, `FileWords::in_language("de")`.
- **Tables:** `ui.cell_changes` gives edits, pastes, undo and redo; Ctrl+V pastes spreadsheet
  text into editable cells, Ctrl+Z and Ctrl+Y undo and redo.
- **Spelling:** several languages per box (`Some("en-US,de-DE")`); words written twice are
  marked, and their menu offers the word once.
- **Colour picker:** `ui.recent_colors()` and `ui.set_recent_colors(..)` keep the recent colours
  between runs (`RecentColors`, one line of text).
- **File dialog:** a name the system's dialog saves under without an ending asked for gets the
  first (the Linux portal adds none); if that would replace a file, the dialog is asked again.

## 0.2.0 (2026-11-17)

- **GPU or CPU:** the feature `gpu` has the GPU show windows, byte for byte the pixels the CPU
  draws: Direct3D 11 or 12, Vulkan or OpenGL on Windows; OpenGL ES 3 (EGL) or Vulkan on Linux
  under X11.
  `Window::renderer` picks (`Renderer::Auto`, `Cpu`, `Gpu`, `Direct3D11`, `Direct3D12`,
  `Vulkan`, `OpenGL`; a missing path falls back to the next),
  `TAUT_RENDERER=cpu|gpu|d3d11|d3d12|vulkan|opengl` overrides, `Ui::renderer` says which path
  shows the window. Without the feature nothing of it is compiled in.
- **Layers:** Direct3D 9 and 9Ex (`layer_d3d9`), Direct3D 12 (`layer_d3d12`) and Vulkan
  (`layer_vulkan`); `examples/d3d9.rs` and `examples/d3d12.rs` are new, `examples/vulkan.rs`
  uses the layer. `Surface::upload` and `Upload`: each changed place as any renderer's texture
  upload takes it.
- **Code changes:** `Window` has the field `renderer`: a `Window { .. }` written out in full
  needs it (`..Default::default()` covers it).
- **Renamed:** names say what things are, in the terms other GUI libraries use. No aliases:
  rename in your code, by the tables below. Examples `nodes` and `files` are now `node_editor`
  and `file_browser`.

### Renamed, by 0.1 name

| 0.1 | 0.2 |
| --- | --- |
| `Axis` | `FontAxis`; fields `least` → `min`, `usual` → `default`, `most` → `max` |
| `Board` | `PanZoom`; methods `spot` → `to_window`, `point` → `from_window`, `sight` → `visible_rect` |
| `Branch` | `TreeItem`; fields `opens` → `has_children`; methods `opening` → `with_children` |
| `Branches` | `TreeView`; fields `picks` → `selection`; methods `with_open` → `with_expanded`, `renew` → `refresh` |
| `Calendar` | `CalendarWords` |
| `Chart` | methods `across` → `x_range`, `up` → `y_range`, `names` → `categories` |
| `ChartView` | fields `across` → `x_range`, `up` → `y_range` |
| `Code` | `CodeEditor`; fields `rules` → `syntax`, `tab` → `tab_width`, `tabs` → `hard_tabs` |
| `Color` | methods `alpha` → `with_alpha`, `toward` → `lerp` |
| `Column` | fields `entry` → `editor`, `sorts` → `sortable`; methods `edited` → `editable` |
| `Contrast` | `ContrastColors`; fields `ground` → `background`, `off` → `disabled`, `picked` → `selected`, `on_picked` → `selected_text` |
| `Date` | methods `months_on` → `add_months` |
| `Did` | `GraphAction`; variants `Nothing` → `None`, `Picked` → `Selected`, `Wired` → `Connected`, `Unwired` → `Disconnected` |
| `Dock` | fields `places` → `nodes`, `filled` → `maximized` |
| `Edited` | `EditedCell` |
| `El` | methods `at` → `offset`, `kept` → `frozen`, `gives_way` → `shrink`, `narrows` → `shrink_width`, `keep_shape` → `keep_aspect`, `film` → `animation` |
| `png::encode_clear` | `png::encode_rgba` |
| `Entry` | `CellEditor` |
| `Event` | variants `Held` → `ModifiersChanged`, `Focus` → `Focused`, `Hidden` → `Minimized`, `Carry` → `FilesHovered`, `Files` → `FilesDropped`, `CarryText` → `TextHovered` |
| `layer_d3d11::Failed`, `layer_opengl::Failed`, `layer_vulkan::Failed` | `Error` in each; fields `doing` → `operation`, `said` → `message` |
| `Field` | fields `least` → `min`, `most` → `max`; methods `least` → `min`, `most` → `max`, `within` → `range` |
| `FileAsk` | `FileDialogRequest` |
| `Files` | `FileDialog`; fields `system` → `native`; methods `of_the_system` → `native` |
| `Film` | `Animation` |
| `first_copy` | `single_instance` |
| `Flow` | variants `Layers` → `Stack` |
| `taut::font` | `taut::font_file` |
| `Font` | fields `rule` → `decoration`; methods `struck` → `strikethrough` |
| `FontFile` | methods `at` → `with_variation` |
| `taut::fonts` | `taut::system_fonts` |
| `Graph` | fields `board` → `view`, `fits` → `can_connect`, `did` → `action` |
| `Grip` | `HitTest`; variants `Inside` → `Client`, `Strip` → `TitleBar` |
| `Handed` | `DragData` |
| `Ink` | `Paint`; variants `Along` → `Linear`, `Around` → `Radial` |
| `Inspector` | fields `find` → `search`, `files` → `file_dialog` |
| `taut::kept` | `taut::storage` |
| `event::keys` | `event::parse_keys` |
| `Locale` | fields `order` → `date_order`, `between` → `date_separator`, `first_day` → `first_weekday`, `twelve_hours` → `twelve_hour_clock`, `thousands` → `thousands_separator`, `point` → `decimal_point` |
| `Markdown` | methods `set_rules` → `set_syntax`, `read` → `parse` |
| `Metrics` | fields `cap` → `cap_height`, `small` → `x_height` |
| `Node` | fields `at` → `position`, `ins` → `inputs`, `outs` → `outputs`, `picked` → `selected` |
| `Opening` | `WindowOpening`; fields `least` → `min_size`, `at` → `position`, `bare` → `frameless` |
| `Order` | `DateOrder` |
| `Ordered` | `BidiText` |
| `OwnFonts` | `AppFonts`; methods `or` → `fallback`, `smooth` → `unhinted` |
| `png::pack` | `png::zlib_compress` |
| `Paper` | methods `turn_to` → `set_page`, `name_picture` → `set_picture`, `picture_of` → `render_page`, `pdf` → `to_pdf` |
| `PartWords` | `WidgetWords` |
| `Phase` | `TouchPhase` |
| `Picks` | `Selection`; methods `has` → `contains` |
| `Piece` | `Span`; methods `words` → `text`, `back` → `background`, `struck` → `strikethrough` |
| `Place` | `DockNode` |
| `Placed` | `WindowPlacement`; fields `at` → `position` |
| `Printed` | `PrintOutcome` |
| `Printing` | `PrintJob`; methods `done` → `outcome` |
| `Quote` | `StringSyntax`; fields `marks` → `delimiters`, `lines` → `multiline`, `one` → `char_literal` |
| `svg::read_filters` | `svg::enable_filters` |
| `svg::read_pictures` | `svg::enable_images` |
| `svg::read_text` | `svg::enable_text` |
| `Rect` | methods `on_pixels` → `snap_to_pixels` |
| `Response` | fields `over` → `contains_pointer` |
| `Role` | methods `holds` → `is_container`; variants `Tick` → `CheckBox`, `Choice` → `RadioButton`, `Spin` → `SpinButton`, `Secret` → `PasswordBox`, `Select` → `ComboBox`, `Heading` → `ColumnHeader`, `Title` → `Heading`, `MenuChoice` → `MenuItemRadio`, `Item` → `ListItem` |
| `inspect::Row` | `InspectorRow` |
| `text::Rule` | `Decoration`; variants `Under` → `Underline`, `Through` → `Strikethrough` |
| `Rules` | `Syntax`; fields `words` → `keywords`, `kinds` → `types`, `capitals` → `capitalized_types`, `comment` → `line_comment`, `long_comment` → `block_comment`, `quotes` → `strings`, `opens` → `indent_after` |
| `Said` | `AccessNode`; fields `keys` → `shortcut`, `open` → `expanded`, `amount` → `value_range`, `of` → `position`, `within` → `parent`, `usable` → `enabled`, `shown` → `visible`, `spelt_wrong` → `misspelled` |
| `Screen` | fields `all` → `rect`, `work` → `work_area` |
| `Scrolled` | `ScrollState`; fields `by` → `offset`, `most` → `max_offset` |
| `Second` | `SecondWindow`; fields `at` → `position`; methods `at` → `position`, `least` → `min_size`, `on` → `on_screen` |
| `Shown` | `MeshInstance`; methods `at` → `centered_at`, `placed` → `transform` |
| `Sort` | `TokenKind`; variants `Word` → `Keyword`, `Kind` → `Type`, `Quoted` → `String` |
| `layout::Spot` | `layout::Placement`; methods `at` → `new`, `across` → `horizontal`, `down` → `vertical` |
| `Start` | `Relaunch`; fields `folder` → `working_dir` |
| `Surface` | methods `png` → `to_png` |
| `Table` | fields `kept` → `frozen`, `cell` → `focused_cell` |
| `Taken` | `DropEffect` |
| `Theme` | fields `raise` → `raised`, `small` → `small_font`, `control` → `control_height` |
| `Tone` | `Severity` |
| `Touch` | fields `finger` → `id`, `at` → `position` |
| `Transform` | methods `turn` → `rotate`, `move_by` → `translate`, `slant` → `skew`, `undone` → `inverse` |
| `Trial` | methods `told` → `access_nodes`, `said` → `visible_access_nodes`, `tell_time` → `set_time`, `listed` → `dump`, `spot` → `center_of`, `picture` → `save_png` |
| `Typing` | `TextEditState`; fields `selected` → `selection`, `undo` → `can_undo`, `redo` → `can_redo` |
| `Ui` | fields `part_words` → `widget_words`, `calendar` → `calendar_words` |
| `png::unpack` | `png::zlib_decompress` |
| `Value` | `CellValue` |
| `View` | `Camera`; methods `look` → `look_at`, `angle` → `fov`, `flat` → `orthographic`, `reach` → `depth_range`, `lit_from` → `light_from`, `whole` → `from_matrix`, `spot` → `project` |
| `Window` | fields `at` → `position`, `bare` → `frameless`, `clear` → `transparent`; methods `placed` → `with_placement` |
| `Wishes` | `Preferences`; fields `contrast` → `high_contrast`, `still` → `reduced_motion`, `text` → `text_scale` |
| `Zoom` | fields `by` → `factor`, `at` → `center`, `moved` → `pan` |

`Ui` methods:

| 0.1 | 0.2 |
| --- | --- |
| `add_film` | `add_animation` |
| `ask_to_close` | `request_close` |
| `bare_window` | `frameless_window` |
| `board` | `pan_zoom` |
| `branch` | `tree_row` |
| `branches` | `tree_view` |
| `carried` | `drag_source` |
| `carrying` | `drag_payload` |
| `carrying_out` | `drag_leaves_window` |
| `chart_at` | `chart_cursor` |
| `close_asked` | `close_requested` |
| `code_box` | `code_editor` |
| `color` | `color_picker` |
| `come_forward` | `bring_to_front` |
| `dialog_up_to` | `dialog_max_width` |
| `drag` | `drag_value` |
| `drag_in` | `drag_value_in` |
| `drop_film` | `drop_animation` |
| `drop_taken` | `take_drop_accepted` |
| `edited` | `edited_cell` |
| `file_answered` | `file_dialog_answered` |
| `files_over` | `files_hovered` |
| `film_size` | `animation_size` |
| `fingers` | `is_touch` |
| `fit_board` | `fit_pan_zoom` |
| `focus_shows` | `focus_visible` |
| `glide` | `animate` |
| `going_out` | `take_drag_out` |
| `grip_at` | `hit_test` |
| `hand_out` | `drag_out` |
| `held` | `drag_delta` |
| `hotkey_taken` | `hotkey_refused` |
| `in_sight` | `in_view` |
| `keep_open` | `cancel_close` |
| `label_downward` | `vertical_label` |
| `leave_enter` | `enter_submits` |
| `listen` | `set_screen_reader` |
| `menu_check` | `menu_checkbox` |
| `pages` | `pagination` |
| `palette` | `command_palette` |
| `pick_row` | `selectable_row` |
| `picked_tree` | `selectable_tree` |
| `placed` | `window_placement` |
| `placeholder` | `skeleton` |
| `press_to_pick` | `press_to_select` |
| `room` | `available_size` |
| `said` | `visible_access_nodes` |
| `scrolled` | `scroll_state` |
| `secret_box` | `password_box` |
| `set_placed` | `set_window_placement` |
| `set_system_files` | `set_native_file_dialog` |
| `set_taken` | `set_drop_effect` |
| `set_text` | `set_text_system` |
| `set_wishes` | `set_preferences` |
| `show_row` | `scroll_to_row` |
| `started_again` | `relaunches` |
| `stay_at_end` | `stick_to_bottom` |
| `suggest` | `autocomplete` |
| `take_file_ask` | `take_file_dialog_request` |
| `take_forward` | `take_bring_to_front` |
| `tell_amount` | `tell_value` |
| `tell_branch` | `tell_tree_item` |
| `tell_keys` | `tell_shortcut` |
| `tell_on_its_own` | `tell_separately` |
| `tell_place` | `tell_position` |
| `text_over` | `text_hovered` |
| `text_room` | `text_margin` |
| `tokens` | `tag_input` |
| `told` | `access_nodes` |
| `tray_check` | `tray_checkbox` |
| `tray_line` | `tray_separator` |
| `typing` | `text_edit_state` |
| `typing_at` | `ime_area` |
| `typing_hidden` | `password_focused` |
| `uneven_list` | `variable_list` |
| `uneven_rows` | `variable_rows` |
| `wheel` | `wheel_delta` |
| `window_active` | `window_focused` |
| `window_hidden` | `window_minimized` |
| `window_opens` | `window_opening` |
| `window_strip` | `set_title_bar` |
| `work_board` | `pan_zoom_editor` |
| `written_date` | `format_date` |
| `written_number` | `format_number` |
| `written_time` | `format_time` |

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
