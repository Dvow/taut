//! taut: a GUI library for Windows and Linux with no dependencies. It draws its own UI with
//! what the system has: no other crates, no web view, no GPU needed.
//!
//! An app describes its window in one function, from its own state. The function reruns when
//! something happens; the library lays out the result, diffs it against the screen, and redraws
//! only what changed. Nothing runs while nothing happens.
//!
//! ```no_run
//! use taut::Window;
//!
//! fn main() {
//!     let mut presses = 0;
//!     let window = Window { title: "Hello", width: 320.0, height: 160.0, ..Default::default() };
//!     taut::run(window, move |ui| {
//!         ui.column().pad(16.0).gap(10.0).open();
//!         ui.label(&format!("Pressed {presses} times"));
//!         presses += ui.button("Press").clicked as u32;
//!         ui.close();
//!     });
//! }
//! ```
//!
//! # Where to go next
//!
//! - [`guide`]: every call on one page, in the order an app needs them.
//! - [`Ui`]: describes a window. Its calls make boxes (containers: [`Ui::row`], [`Ui::column`],
//!   [`Ui::grid`]) and parts (widgets: [`Ui::button`], [`Ui::text_box`], [`Ui::table`]).
//! - [`Surface`]: the UI drawn into a pixel buffer, for the app's own renderer (as a texture)
//!   or a DLL; [`testing::Trial`]: a window tested with no screen.
//! - The book, a ten-chapter tutorial from a first window to taut inside other programs, is the
//!   package's `docs` folder (`docs/README.md`); `examples/` holds whole apps.
//!
//! # Features
//!
//! A program carries only the code it calls. What can't be told from the calls is a feature,
//! all off by default: `right-to-left` (Hebrew, Arabic and other RTL scripts), `brahmic` (South
//! and Southeast Asian scripts), `emoji` (colour emoji), `fallback-fonts` (characters a font
//! lacks, in the font Windows would choose), `touch` (touch, pen and touchpad pinch gestures),
//! `cff` (`.otf` fonts with PostScript outlines), `woff2` (WOFF2 web fonts),
//! `color-profiles` (image colour profiles), `gpu` (windows shown by the GPU, see
//! [`Renderer`]), `gpu-draw` (windows drawn by the GPU too, see [`Renderer`]), `hdr`
//! (brightness past SDR white on HDR screens, see [`El::brightness`]), `translations`
//! (built-in words in six more languages) and `video` (video and sound by the system's
//! player, see `Video`).
//!
//! # Systems
//!
//! Windows, Linux under X11 or Wayland, macOS, iOS, Android (through `NativeActivity`, see
//! `platform::run`), and browsers built for `wasm32-unknown-unknown` (see [`web`]); tested on
//! Windows 11, Ubuntu 26.04, Arch Linux, macOS 26, the iOS 26 simulator and an Android 15
//! emulator.
//! Rust 1.88 or newer.
//!
//! # Terms
//!
//! A *part* is a widget, a *box* a container, a *unit* a logical pixel (a DIP), and the *scale*
//! the display scaling factor. What a part *tells* is its accessibility information; a *note*
//! is secondary text. The guide's first section lists the rest.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// Desktop-only code a page never reaches (hotkeys, system locales, file dialogs' own parts).
#![cfg_attr(target_family = "wasm", allow(dead_code))]
#[cfg(all(target_vendor = "apple", not(any(target_os = "macos", target_os = "ios"))))]
compile_error!("taut runs on Windows, Linux, macOS, iOS and the web so far: its windows for tvOS, watchOS and visionOS are not written yet");
// Tests running the examples on a compositor refer to the crate as `taut`, as they do.
#[cfg(test)]
extern crate self as taut;
mod accessibility;
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_access;
#[cfg(any(target_os = "android", test))]
mod android_dex;
#[cfg(target_os = "android")]
mod android_jni;
#[cfg(target_os = "android")]
mod android_sys;
#[cfg(any(target_os = "android", test))]
mod android_text;
mod animation;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod atspi;
mod bidi;
pub mod bmp;
mod brahmic;
mod brotli;
pub mod canvas;
mod cells;
mod cff;
mod chart;
mod clock;
mod code_editor;
pub mod color;
mod color_picker;
mod color_profiles;
mod colr;
mod combo;
#[cfg(all(test, unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod compositors;
#[cfg(target_vendor = "apple")]
mod core_text;
mod date;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod decorations;
mod dock;
mod drag_drop;
mod el;
pub mod emoji;
pub mod event;
mod exif_orientation;
mod file_dialog;
mod folder_listing;
pub mod font_file;
mod frameless;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod freedesktop;
pub mod geom;
pub mod gif;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_d3d11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_d3d12;
#[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
mod gpu_draw;
#[cfg(all(windows, feature = "gpu-draw"))]
mod gpu_draw_d3d12;
#[cfg(all(target_os = "macos", feature = "gpu-draw"))]
mod gpu_draw_metal;
#[cfg(all(feature = "gpu-draw", not(any(target_vendor = "apple", target_os = "android"))))]
mod gpu_draw_opengl;
#[cfg(all(feature = "gpu-draw", not(any(target_vendor = "apple", target_os = "android"))))]
mod gpu_draw_vulkan;
#[cfg(all(windows, feature = "gpu-draw"))]
mod gpu_draw_windows;
#[cfg(all(target_os = "macos", feature = "gpu"))]
mod gpu_metal;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_opengl_windows;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android")), feature = "gpu"))]
mod gpu_opengl_x11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_vulkan_windows;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android")), feature = "gpu"))]
mod gpu_vulkan_x11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_windows;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android")), feature = "gpu"))]
mod gpu_x11;
#[cfg(test)]
mod hardening;
#[cfg_attr(all(feature = "hdr", any(target_vendor = "apple", target_os = "android")), allow(dead_code, reason = "Apple systems and Android show SDR so far"))]
mod hdr;
#[cfg(all(windows, feature = "hdr"))]
mod hdr_windows;
mod hinting;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod hotkeys;
mod id;
mod inspect;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
mod ios_text_input;
pub mod jpeg;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod keymap;
#[cfg(windows)]
pub mod layer_d3d11;
#[cfg(windows)]
pub mod layer_d3d12;
#[cfg(windows)]
pub mod layer_d3d9;
#[cfg(target_vendor = "apple")]
pub mod layer_metal;
pub mod layer_opengl;
pub mod layer_vulkan;
pub mod layout;
mod line_breaks;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod locale;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_composing;
#[cfg(target_os = "macos")]
mod macos_drag_out;
#[cfg(target_os = "macos")]
mod macos_files;
#[cfg(target_os = "macos")]
mod macos_notifications;
#[cfg(target_os = "macos")]
mod macos_shell;
#[cfg(target_vendor = "apple")]
mod macos_sys;
mod markdown;
mod markdown_pages;
mod markdown_view;
mod node_graph;
mod normalization;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod notifications;
#[cfg(target_os = "macos")]
mod ns_accessibility;
#[cfg(target_os = "macos")]
mod ns_spell_checker;
mod number;
#[cfg(windows)]
mod ole_drag;
mod open_with;
mod paint;
mod pan_zoom;
mod panel;
mod paper;
pub mod path;
mod pdf;
mod pdf_archive;
mod pdf_forms;
mod pdf_lock;
mod pdf_structure;
pub mod pen;
mod places;
pub mod platform;
pub mod png;
mod pointer;
mod popups;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod portal;
mod preferences;
mod print;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod print_linux;
#[cfg(windows)]
mod print_windows;
mod rich_text;
mod saved_layout;
mod scene;
mod scene_raster;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod screens;
mod scripts;
mod scroll;
mod second_window;
mod selection;
mod shaping;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod single_instance;
mod sliders;
mod slots;
mod spell;
pub mod storage;
pub mod surface;
pub mod svg;
mod svg_filters;
pub mod system_fonts;
#[cfg(all(feature = "shaping", any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android"))))))]
mod system_forms;
mod table;
mod table_paste;
pub mod testing;
pub mod text;
mod text_edit;
pub mod theme;
mod toolbar;
mod touch;
#[cfg(windows)]
mod touch_windows;
#[cfg(feature = "translations")]
mod translations;
#[cfg_attr(target_os = "android", allow(dead_code))] // (Desktop back ends only.)
mod tray;
mod tree_view;
#[cfg(feature = "hinting")]
mod truetype_hinting;
pub mod ui;
#[cfg(target_os = "ios")]
mod ui_accessibility;
#[cfg(target_os = "ios")]
mod ui_accessibility_text;
#[cfg(windows)]
mod uia;
mod variable_fonts;
mod vertical_text;
#[cfg(feature = "video")]
mod video;
#[cfg(all(feature = "video", unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod video_gstreamer;
#[cfg(all(feature = "video", target_os = "macos"))]
mod video_macos;
#[cfg(all(feature = "video", any(target_os = "ios", target_os = "android", target_family = "wasm")))]
mod video_none;
#[cfg(all(feature = "video", windows))]
mod video_windows;
mod visible_boxes;
mod vk;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod wayland;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod wayland_wire;
pub mod web;
#[cfg(target_family = "wasm")]
mod web_host;
#[cfg(any(test, target_family = "wasm"))]
mod web_input;
#[cfg(any(test, target_family = "wasm"))]
mod web_mirror;
#[cfg(target_family = "wasm")]
mod web_text;
mod web_widgets;
pub mod webp;
mod widgets;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_shell;
mod woff2;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod x11;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod xcursor;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android")), feature = "touch"))]
mod xinput;

pub use accessibility::{AccessNode, Role};
pub use animation::Animation;
pub use bidi::BidiText;
pub use canvas::Canvas;
pub use cells::{CellEditor, CellValue, EditedCell};
pub use chart::{Chart, ChartView, Series};
pub use clock::Time;
pub use code_editor::{CodeEditor, StringSyntax, Syntax, TokenKind};
pub use color::Color;
pub use color_picker::RecentColors;
pub use date::{CalendarWords, Date};
pub use dock::{Dock, DockNode};
pub use drag_drop::{DragData, DropEffect};
pub use el::El;
pub use emoji::Emoji;
pub use event::{Button, Cursor, Event, Key, Mods};
pub use file_dialog::{FileDialog, FileDialogRequest, FileWords};
pub use font_file::{AppFonts, FontFile};
pub use frameless::HitTest;
pub use geom::{Corners, Insets, Point, Rect, Size, Transform};
pub use id::Id;
pub use inspect::{Field, Fields, Inspect, Inspector, InspectorRow};
pub use layout::{Align, Flow, Length};
pub use locale::{DateOrder, Locale};
pub use markdown::Markdown;
pub use node_graph::{Frame, Graph, GraphAction, GraphWords, Node, Port, Reroute, Wire};
pub use notifications::Notification;
pub use number::Number;
pub use open_with::open_with_system;
pub use pan_zoom::PanZoom;
pub use panel::Panel;
pub use paper::Paper;
pub use path::Path;
pub use pen::{Cap, Join, Paint, Pen};
pub use platform::{Clipboard, LocalClipboard, Renderer, Waker, Window, WindowPlacement, run};
pub use popups::FrameWords;
pub use preferences::Preferences;
pub use print::{PrintJob, PrintOutcome, print, print_laid};
pub use rich_text::Span;
pub use scene::{Camera, Mesh, MeshInstance};
pub use screens::Screen;
pub use scripts::Language;
pub use scroll::ScrollState;
pub use second_window::{SecondWindow, WindowKind, WindowOpening};
pub use selection::Selection;
pub use single_instance::{Relaunch, single_instance};
pub use surface::{Surface, Upload};
pub use svg::Drawing;
pub use table::{Column, Table};
pub use text::{Face, Font, TextSystem};
pub use text_edit::{TextCommand, TextEditState};
pub use theme::{ContrastColors, Theme};
pub use touch::{Stylus, Touch, TouchPhase, Zoom};
pub use tree_view::{TreeItem, TreeView};
pub use ui::{Icon, Picture, Response, Ui};
pub use variable_fonts::FontAxis;
#[cfg(feature = "video")]
pub use video::{Video, VideoError, VideoSource};
pub use web_widgets::{Severity, WidgetWords};

/// The README and book examples are compiled as doc tests, so they stay correct. (The guide's
/// are too, through the page below.)
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/01-a-first-window.md")]
#[doc = include_str!("../docs/02-boxes-and-layout.md")]
#[doc = include_str!("../docs/03-parts-and-state.md")]
#[doc = include_str!("../docs/04-a-real-small-app.md")]
#[doc = include_str!("../docs/05-larger-parts.md")]
#[doc = include_str!("../docs/06-text-and-pictures.md")]
#[doc = include_str!("../docs/07-everyone-can-use-it.md")]
#[doc = include_str!("../docs/08-testing.md")]
#[doc = include_str!("../docs/09-inside-another-program.md")]
#[doc = include_str!("../docs/10-coming-from-egui-or-dear-imgui.md")]
struct Written;

// `GUIDE.md` as a page of the crate docs, on docs.rs too: every call on one page. Its examples
// are doc tests from here.
#[cfg(any(doc, doctest))]
pub mod guide {
    #![doc = include_str!("../GUIDE.md")]
}
