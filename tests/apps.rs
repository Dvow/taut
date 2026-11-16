//! The example apps, each through the test at the end of its own file: built here as tests, so
//! `cargo test` runs them. Each is clicked and typed through in a headless window
//! (`taut::testing`), as an app tests itself.
// (Each app's `main` is not used here, nor is the attribute that keeps a console away.)
#![allow(dead_code, unused_attributes)]

#[path = "../examples/files.rs"]
mod files;

#[path = "../examples/editor.rs"]
mod editor;

#[path = "../examples/settings.rs"]
mod settings;

#[path = "../examples/chat.rs"]
mod chat;

#[path = "../examples/viewer.rs"]
mod viewer;

#[path = "../examples/dashboard.rs"]
mod dashboard;

#[path = "../examples/nodes.rs"]
mod nodes;

#[path = "../examples/report.rs"]
mod report;
