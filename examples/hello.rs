//! The smallest program: a window with a label and a button. It is what a program pays for taut
//! when it uses almost nothing, and what the library's size is measured by.
//!   cargo run -p taut --example hello --release
use taut::Window;

fn main() {
    let mut presses = 0;
    let window = Window { title: "Hello", width: 320.0, height: 160.0, ..Default::default() };
    taut::run(window, move |ui| {
        ui.column().pad(16.0).gap(10.0).open();
        ui.label(&format!("Pressed {presses} times"));
        presses += ui.button("Press").clicked as u32;
        ui.close();
    });
}

/// Where Android starts the app (it is a library there): see `taut::run`.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
#[allow(clippy::main_recursion)]
extern "C" fn android_main() {
    main()
}
