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
