//! taut inside a DLL, for a program with its own renderer (a Direct3D game, say). The program
//! feeds the overlay mouse and keyboard input, asks it for an image, and shows that as a
//! texture over its own: transparent wherever the overlay draws nothing. Everything goes
//! through plain C functions, so the program can be in any language.
//!   cargo build -p taut --example overlay --release
//!
//! Each frame the program calls `overlay_frame`. That costs nothing unless something happened;
//! when it did, only the changed part is redrawn, and the program copies just that part to its
//! texture (pixels are blue, green, red, alpha, premultiplied).
use std::time::{Duration, Instant};
use taut::geom::{Size, pt};
use taut::{Button, Color, Event, Key, Length, LocalClipboard, Mods, Surface, Ui};

/// What the overlay's own parts are set to.
#[derive(Default)]
struct Settings {
    names: bool,
    range: f32,
    presses: u32,
}

pub struct Overlay {
    ui: Ui,
    surface: Surface,
    settings: Settings,
    /// Something happened that the picture does not show yet.
    waiting: bool,
    since: Instant,
}

fn describe(settings: &mut Settings, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().w(Length::Px(220.0)).offset(16.0, 16.0).pad(14.0).gap(10.0).fill(t.panel.with_alpha(0.92)).border(t.line_strong, 1.0).radius(10.0).shadow(12.0).open();
    ui.text("Overlay", t.font.bold(), t.text);
    ui.switch(&mut settings.names, "Show names");
    ui.slider(&mut settings.range, 0.0..=100.0);
    settings.presses += ui.button("Count").clicked as u32;
    ui.note(&format!("{} presses, range {:.0}", settings.presses, settings.range));
    ui.close();
}

/// A new overlay, `width` by `height` pixels, for a screen that shows `scale` pixels to a
/// unit. Hand what comes back to the other functions, and to `overlay_close` in the end.
#[unsafe(no_mangle)]
pub extern "C" fn overlay_open(width: u32, height: u32, scale: f32) -> *mut Overlay {
    let mut ui = Ui::new(taut::platform::system_text(), Box::new(LocalClipboard::default()));
    ui.theme.bg = Color::NONE;
    ui.resize(Size { w: width as f32 / scale, h: height as f32 / scale }, scale);
    let surface = Surface::draw(&mut ui, 64);
    Box::into_raw(Box::new(Overlay { ui, surface, settings: Settings::default(), waiting: true, since: Instant::now() }))
}

/// # Safety
/// `overlay` came from `overlay_open`, and is not used after this.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_close(overlay: *mut Overlay) {
    drop(unsafe { Box::from_raw(overlay) });
}

/// The pointer is at these pixels of the overlay.
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_pointer(overlay: *mut Overlay, x: f32, y: f32) {
    let overlay = unsafe { &mut *overlay };
    let scale = overlay.ui.scale();
    overlay.waiting |= overlay.ui.event(Event::Move(pt(x / scale, y / scale)));
}

/// The left mouse button went down (1) or came up (0).
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_button(overlay: *mut Overlay, down: u32) {
    let overlay = unsafe { &mut *overlay };
    let event = if down != 0 { Event::Press(Button::Left, Mods::NONE) } else { Event::Release(Button::Left) };
    overlay.waiting |= overlay.ui.event(event);
}

/// A key went down: 0 for Tab, 1 for Space, 2 for Enter, 3 and 4 for the left and right arrows.
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_key(overlay: *mut Overlay, key: u32) {
    let overlay = unsafe { &mut *overlay };
    let Some(key) = [Key::Tab, Key::Space, Key::Enter, Key::Left, Key::Right].get(key as usize) else { return };
    overlay.waiting |= overlay.ui.event(Event::Key(*key, Mods::NONE));
}

/// A message that the program's window got from Windows: mouse, keyboard, focus. (Instead of
/// the three functions above, where there is such a window.)
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed; `w` and `l` are what Windows
/// sent with `msg`.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_message(overlay: *mut Overlay, msg: u32, w: usize, l: isize) {
    thread_local!(static MESSAGES: std::cell::RefCell<taut::platform::Messages> = Default::default());
    let overlay = unsafe { &mut *overlay };
    let (scale, ui, waiting) = (overlay.ui.scale(), &mut overlay.ui, &mut overlay.waiting);
    // SAFETY: what came with the message is what Windows sent with it, as the caller promises.
    MESSAGES.with_borrow_mut(|messages| unsafe { messages.events(msg, w, l, scale, |event| *waiting |= ui.event(event)) });
}

/// The picture to show now: rows of pixels, as many to a row as the overlay is wide. If
/// something happened since the last call, the part that changed is drawn again first, and
/// `changed` says which part that is, in pixels: left, top, right, bottom (all 0 for none).
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed; `changed` is room for four
/// numbers. The picture stays where it is until the overlay is closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_frame(overlay: *mut Overlay, changed: *mut [i32; 4]) -> *const u32 {
    let Overlay { ui, surface, settings, waiting, since } = unsafe { &mut *overlay };
    ui.set_time(since.elapsed());
    // Something is moving (a switch sliding over): the interface asked to be run by now.
    let due = ui.wake_in() == Some(Duration::ZERO);
    let mut area = [0; 4];
    if std::mem::take(waiting) || due {
        let part = ui.run(&mut |ui| describe(settings, ui)).snap_to_pixels(ui.scale());
        surface.redraw(ui, part);
        let scale = ui.scale();
        area = [(part.x * scale) as i32, (part.y * scale) as i32, (part.right() * scale) as i32, (part.bottom() * scale) as i32];
    }
    unsafe { *changed = area };
    surface.pixels.as_ptr()
}

/// Whether the pointer is the overlay's right now (1) or the program's own (0): it is the
/// overlay's over its panel, and while one of its parts is held. A program tells the
/// overlay of everything the mouse does, calls `overlay_frame`, and then asks this: what
/// the mouse does while the answer is 0 is for the program itself.
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_wants_pointer(overlay: *const Overlay) -> u32 {
    unsafe { (*overlay).ui.wants_pointer() as u32 }
}

/// The same for the keyboard: it is the overlay's while it is on one of its parts.
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_wants_keyboard(overlay: *const Overlay) -> u32 {
    unsafe { (*overlay).ui.wants_keyboard() as u32 }
}

/// How many times the overlay's button has been pressed.
///
/// # Safety
/// `overlay` came from `overlay_open` and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_presses(overlay: *const Overlay) -> u32 {
    unsafe { (*overlay).settings.presses }
}
