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
//! `color-profiles` (image colour profiles) and `gpu` (windows shown by the GPU, see
//! [`Renderer`]).
//!
//! # Systems
//!
//! Windows, and Linux under X11 or Wayland (tested on Windows 11, Ubuntu 26.04 and Arch Linux).
//! Not yet macOS, iOS, Android or the web: for macOS, iOS and the web the crate does not build.
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
#[cfg(any(target_os = "ios", target_family = "wasm"))]
compile_error!("taut runs on Windows and Linux so far: its windows for macOS, iOS and the web are not written yet");
// Tests running the examples on a compositor refer to the crate as `taut`, as they do.
#[cfg(test)]
extern crate self as taut;
mod accessibility;
mod animation;
#[cfg(all(unix, not(target_os = "macos")))]
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
#[cfg(all(test, unix, not(target_os = "macos")))]
mod compositors;
#[cfg(target_os = "macos")]
mod core_text;
mod date;
#[cfg(all(unix, not(target_os = "macos")))]
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
#[cfg(all(unix, not(target_os = "macos")))]
mod freedesktop;
pub mod geom;
pub mod gif;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_d3d11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_d3d12;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_opengl_windows;
#[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
mod gpu_opengl_x11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_vulkan_windows;
#[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
mod gpu_vulkan_x11;
#[cfg(all(windows, feature = "gpu"))]
mod gpu_windows;
#[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
mod gpu_x11;
#[cfg(test)]
mod hardening;
mod hinting;
mod hotkeys;
mod id;
mod inspect;
pub mod jpeg;
#[cfg(all(unix, not(target_os = "macos")))]
mod keymap;
#[cfg(windows)]
pub mod layer_d3d11;
#[cfg(windows)]
pub mod layer_d3d12;
#[cfg(windows)]
pub mod layer_d3d9;
pub mod layer_opengl;
pub mod layer_vulkan;
pub mod layout;
mod line_breaks;
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
#[cfg(target_os = "macos")]
mod macos_sys;
mod markdown;
mod markdown_pages;
mod markdown_view;
mod node_graph;
mod normalization;
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
pub mod pen;
mod places;
pub mod platform;
pub mod png;
mod pointer;
mod popups;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
mod preferences;
mod print;
#[cfg(all(unix, not(target_os = "macos")))]
mod print_linux;
#[cfg(windows)]
mod print_windows;
mod rich_text;
mod saved_layout;
mod scene;
mod scene_raster;
mod screens;
mod scripts;
mod scroll;
mod second_window;
mod selection;
mod shaping;
mod single_instance;
mod sliders;
mod slots;
mod spell;
pub mod storage;
pub mod surface;
pub mod svg;
mod svg_filters;
pub mod system_fonts;
mod table;
pub mod testing;
pub mod text;
mod text_edit;
pub mod theme;
mod toolbar;
mod touch;
#[cfg(windows)]
mod touch_windows;
mod tray;
mod tree_view;
pub mod ui;
#[cfg(windows)]
mod uia;
mod variable_fonts;
mod vertical_text;
mod visible_boxes;
mod vk;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland_wire;
mod web_widgets;
pub mod webp;
mod widgets;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_shell;
mod woff2;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;
#[cfg(all(unix, not(target_os = "macos")))]
mod xcursor;
#[cfg(all(unix, not(target_os = "macos"), feature = "touch"))]
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
pub use node_graph::{Graph, GraphAction, GraphWords, Node, Port, Wire};
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
pub use print::{PrintJob, PrintOutcome, print};
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
