//! taut: a GUI library for Windows and Linux with no dependencies. It draws its own UI with
//! what the system has: no other crates, no GPU, no web view.
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
//! `cff` (`.otf` fonts with PostScript outlines), `woff2` (WOFF2 web fonts) and
//! `color-profiles` (image colour profiles).
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
#[cfg(any(target_os = "macos", target_os = "ios", target_family = "wasm"))]
compile_error!("taut runs on Windows and Linux so far: its windows for macOS, iOS and the web are not written yet");
// Tests running the examples on a compositor refer to the crate as `taut`, as they do.
#[cfg(test)]
extern crate self as taut;
mod accents;
#[cfg(all(unix, not(target_os = "macos")))]
mod atspi;
mod bare;
pub mod bmp;
mod board;
mod brahmic;
mod branches;
mod breaks;
mod brotli;
pub mod canvas;
mod carry;
mod cells;
mod cff;
mod chart;
mod clock;
mod code;
pub mod color;
mod colr;
mod combo;
#[cfg(all(test, unix, not(target_os = "macos")))]
mod compositors;
#[cfg(windows)]
mod contacts;
mod copies;
mod crisp;
mod date;
mod dock;
mod document;
mod edit;
mod el;
pub mod emoji;
pub mod event;
mod familiar;
mod files;
mod film;
mod filter;
pub mod font;
pub mod fonts;
#[cfg(all(unix, not(target_os = "macos")))]
mod frame;
#[cfg(all(unix, not(target_os = "macos")))]
mod freedesktop;
pub mod geom;
pub mod gif;
#[cfg(test)]
mod hardening;
mod hotkeys;
mod id;
mod inspect;
pub mod jpeg;
pub mod kept;
#[cfg(all(unix, not(target_os = "macos")))]
mod keymap;
#[cfg(windows)]
pub mod layer_d3d11;
pub mod layer_opengl;
pub mod layout;
mod listing;
mod locale;
mod markdown;
mod menus;
mod nodes;
mod notices;
mod number;
#[cfg(windows)]
mod ole;
mod opening;
mod order;
mod pages;
mod paint;
mod panel;
mod paper;
pub mod path;
mod pdf;
pub mod pen;
mod picker;
mod picks;
mod places;
pub mod platform;
pub mod png;
mod pointer;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
mod print;
#[cfg(all(unix, not(target_os = "macos")))]
mod printer;
mod profile;
mod raster;
mod rich;
mod scene;
mod screens;
mod scripts;
mod scrolled;
mod second;
mod shape;
mod shelf;
#[cfg(windows)]
mod shell;
mod slide;
mod spell;
#[cfg(windows)]
mod spooler;
pub mod surface;
mod survey;
pub mod svg;
mod table;
pub mod testing;
pub mod text;
pub mod theme;
mod told;
mod toolbar;
mod touch;
mod tray;
pub mod ui;
#[cfg(windows)]
mod uia;
mod upright;
mod vary;
mod vertical;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland;
pub mod webp;
mod widgets;
#[cfg(windows)]
mod windows;
#[cfg(all(unix, not(target_os = "macos")))]
mod wire;
mod wishes;
mod woff2;
mod written;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;
#[cfg(all(unix, not(target_os = "macos")))]
mod xcursor;
#[cfg(all(unix, not(target_os = "macos"), feature = "touch"))]
mod xinput;

pub use bare::Grip;
pub use board::Board;
pub use branches::{Branch, Branches};
pub use canvas::Canvas;
pub use carry::{Handed, Taken};
pub use cells::{Edited, Entry, Value};
pub use chart::{Chart, ChartView, Series};
pub use clock::Time;
pub use code::{Code, Quote, Rules, Sort};
pub use color::Color;
pub use copies::{Start, first_copy};
pub use date::{Calendar, Date};
pub use dock::{Dock, Place};
pub use edit::{TextCommand, Typing};
pub use el::El;
pub use emoji::Emoji;
pub use event::{Button, Cursor, Event, Key, Mods};
pub use familiar::{PartWords, Tone};
pub use files::{FileAsk, FileWords, Files};
pub use film::Film;
pub use font::{FontFile, OwnFonts};
pub use geom::{Corners, Insets, Point, Rect, Size, Transform};
pub use id::Id;
pub use inspect::{Field, Fields, Inspect, Inspector, Row};
pub use layout::{Align, Flow, Length};
pub use locale::{Locale, Order};
pub use markdown::Markdown;
pub use menus::FrameWords;
pub use nodes::{Did, Graph, GraphWords, Node, Port, Wire};
pub use notices::Notification;
pub use number::Number;
pub use opening::open_with_system;
pub use order::Ordered;
pub use panel::Panel;
pub use paper::Paper;
pub use path::Path;
pub use pen::{Cap, Ink, Join, Pen};
pub use picks::Picks;
pub use platform::{Clipboard, LocalClipboard, Placed, Waker, Window, run};
pub use print::{Printed, Printing, print};
pub use rich::Piece;
pub use scene::{Mesh, Shown, View};
pub use screens::Screen;
pub use scripts::Language;
pub use scrolled::Scrolled;
pub use second::{Opening, Second, WindowKind};
pub use surface::Surface;
pub use svg::Drawing;
pub use table::{Column, Table};
pub use text::{Face, Font, TextSystem};
pub use theme::{Contrast, Theme};
pub use told::{Role, Said};
pub use touch::{Phase, Stylus, Touch, Zoom};
pub use ui::{Icon, Picture, Response, Ui};
pub use vary::Axis;
pub use wishes::Wishes;

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
