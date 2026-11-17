# Changes

Changes an app can notice, newest first. Before 1.0, a new minor version (0.1 → 0.2) may need
code changes; a new patch version (0.1.0 → 0.1.1) never does.

## 0.2.0 (unreleased)

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

  needs it (`..Default::default()` covers it).

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
