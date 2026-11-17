//! Windows touch and pen input: `WM_POINTER…` messages, read into [`Touch`] and [`Stylus`]
//! events (a pen also drives the mouse).
//!
//! Left to Windows, these become mouse input: a dragged finger selects instead of scrolling,
//! two fingers do nothing, pen pressure is lost. So they are taken here, and Windows makes no
//! mouse of them. (Where a host program runs the window, the mouse messages Windows makes of
//! them anyway carry a mark and are skipped.) Mouse and touchpad stay mouse input: two-finger
//! scrolling comes as the wheel, a pinch as Ctrl+wheel, which `windows` reads as zoom.
//!
//! The records follow `winuser.h`; `finger` and `stylus` read them, and tests feed them records
//! they build.
use crate::event::{Button, Event, Mods};
use crate::geom::{Point, pt};
use crate::touch::{Stylus, Touch, TouchPhase};
use crate::windows::sys::{POINT, ScreenToClient};

/// Pointer messages: moved (or hovering), down, up, out of range, capture lost.
pub(crate) const WM_POINTERUPDATE: u32 = 0x0245;
pub(crate) const WM_POINTERDOWN: u32 = 0x0246;
pub(crate) const WM_POINTERUP: u32 = 0x0247;
pub(crate) const WM_POINTERLEAVE: u32 = 0x024A;
pub(crate) const WM_POINTERCAPTURECHANGED: u32 = 0x024C;
/// Pointer kinds: finger, pen.
const PT_TOUCH: u32 = 2;
const PT_PEN: u32 = 3;
/// Pointer flags: in contact; cancelled.
const INCONTACT: u32 = 0x0004;
const CANCELED: u32 = 0x8000;
/// Modifier keys held.
const MOD_SHIFT: u32 = 0x0004;
const MOD_CTRL: u32 = 0x0008;
/// Pen flags: barrel button held; inverted (eraser end down); eraser button held.
const BARREL: u32 = 0x1;
const INVERTED: u32 = 0x2;
const ERASER: u32 = 0x4;
/// Which pen values are given: pressure, rotation, tilt.
const PRESSURE: u32 = 0x1;
const ROTATION: u32 = 0x2;
const TILT_X: u32 = 0x4;
const TILT_Y: u32 = 0x8;
/// The mark on mouse messages Windows made from pen or touch, in `GetMessageExtraInfo` (the low
/// eight bits vary).
const MADE_UP: u32 = 0xFF51_5700;

/// `POINTER_INFO`, common to all pointers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PointerInfo {
    /// `PT_TOUCH` or `PT_PEN`, and the pointer's id while it lasts.
    pub kind: u32,
    pub id: u32,
    pub frame: u32,
    pub flags: u32,
    pub device: isize,
    /// Target window, and screen position in pixels.
    pub window: isize,
    pub at: [i32; 2],
    pub himetric: [i32; 2],
    pub at_raw: [i32; 2],
    pub himetric_raw: [i32; 2],
    pub time: u32,
    pub history: u32,
    pub data: i32,
    /// Modifiers held (`POINTER_MOD_…`).
    pub keys: u32,
    pub count: u64,
    /// The button that changed with this message (`POINTER_BUTTON_CHANGE_TYPE`): none (0),
    /// first down (1) or up (2), second down (3) or up (4), third (5, 6), then fourth and
    /// fifth.
    pub change: u32,
}

/// `POINTER_TOUCH_INFO`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PointerTouchInfo {
    pub pointer: PointerInfo,
    pub flags: u32,
    pub mask: u32,
    pub contact: [i32; 4],
    pub contact_raw: [i32; 4],
    pub orientation: u32,
    pub pressure: u32,
}

/// `POINTER_PEN_INFO`: pressure (0 to 1024), rotation (0 to 359°) and tilt (-90 to 90°, right
/// and toward the user), as far as `mask` says.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PointerPenInfo {
    pub pointer: PointerInfo,
    pub flags: u32,
    pub mask: u32,
    pub pressure: u32,
    pub rotation: u32,
    pub tilt: [i32; 2],
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetPointerType(id: u32, kind: *mut u32) -> i32;
    fn GetPointerTouchInfo(id: u32, info: *mut PointerTouchInfo) -> i32;
    fn GetPointerPenInfo(id: u32, info: *mut PointerPenInfo) -> i32;
    fn GetMessageExtraInfo() -> isize;
}

/// Whether a message (with `w`) is touch or pen input the UI takes, keeping it from Windows,
/// which would also turn it into mouse input. Mouse and touchpad come as mouse messages.
pub(crate) fn taken(msg: u32, w: usize) -> bool {
    let mut kind = 0;
    // SAFETY: asks for the pointer type of the current message.
    matches!(msg, WM_POINTERUPDATE | WM_POINTERDOWN | WM_POINTERUP | WM_POINTERLEAVE | WM_POINTERCAPTURECHANGED)
        && unsafe { GetPointerType(w as u32 & 0xffff, &mut kind) } != 0
        && (kind == PT_TOUCH || kind == PT_PEN)
}

/// Whether the current mouse message was made by Windows from touch or pen, which are read
/// directly instead.
pub(crate) fn made_up() -> bool {
    // SAFETY: asks about the current message. (The value is 32 bits.)
    (unsafe { GetMessageExtraInfo() }) as u32 & 0xFFFF_FF00 == MADE_UP
}

/// Calls `each` with the events of a touch or pen message, from the pointer's record. `scale`
/// is pixels per unit.
pub(crate) fn events(msg: u32, w: usize, scale: f32, each: &mut impl FnMut(Event)) {
    let (id, mut kind) = (w as u32 & 0xffff, 0);
    // SAFETY: asks about the current message's pointer; each record is filled by Windows and
    // read only when it says so.
    let read = unsafe {
        let (mut touching, mut pen) = (PointerTouchInfo::default(), PointerPenInfo::default());
        match GetPointerType(id, &mut kind) != 0 && kind == PT_TOUCH {
            true if GetPointerTouchInfo(id, &mut touching) != 0 => [finger(msg, &touching, place(&touching.pointer, corner(touching.pointer.window), scale)), None, None],
            true => [None, None, None],
            false if kind == PT_PEN && GetPointerPenInfo(id, &mut pen) != 0 => stylus(msg, &pen, place(&pen.pointer, corner(pen.pointer.window), scale)),
            false => [None, None, None],
        }
    };
    read.into_iter().flatten().for_each(each);
}

/// The screen position of `window`'s client area, in pixels.
fn corner(window: isize) -> [i32; 2] {
    let mut nought = POINT { x: 0, y: 0 };
    // SAFETY: asks about a window by handle; a destroyed one only makes the call fail.
    unsafe { ScreenToClient(window, &mut nought) };
    [-nought.x, -nought.y]
}

/// A pointer's position in units from its window's client origin, which is at `corner` on
/// screen.
fn place(info: &PointerInfo, corner: [i32; 2], scale: f32) -> Point {
    pt((info.at[0] - corner[0]) as f32 / scale, (info.at[1] - corner[1]) as f32 / scale)
}

/// The event of a finger message, the finger at `at`.
fn finger(msg: u32, record: &PointerTouchInfo, at: Point) -> Option<Event> {
    let info = &record.pointer;
    let phase = match msg {
        _ if info.flags & CANCELED != 0 => TouchPhase::Cancel,
        WM_POINTERDOWN => TouchPhase::Down,
        WM_POINTERUPDATE if info.flags & INCONTACT != 0 => TouchPhase::Move,
        WM_POINTERUP => TouchPhase::Up,
        WM_POINTERCAPTURECHANGED => TouchPhase::Cancel,
        _ => return None,
    };
    Some(Event::Touch(Touch { id: info.id, position: at, phase }))
}

/// The events of a pen message, the pen at `at`: its state, the pointer move, and the button
/// that changed, if any (tip is left, tip with barrel button right). Out of range, pen and
/// pointer are gone.
fn stylus(msg: u32, record: &PointerPenInfo, at: Point) -> [Option<Event>; 3] {
    let info = &record.pointer;
    if msg == WM_POINTERLEAVE || msg == WM_POINTERCAPTURECHANGED {
        // Leave first: a button released over nothing clicks nothing.
        return [Some(Event::Stylus(None)), Some(Event::Leave), (msg == WM_POINTERCAPTURECHANGED).then_some(Event::Release(Button::Left))];
    }
    let (says, touching) = (|what: u32| record.mask & what != 0, info.flags & INCONTACT != 0);
    let pressure = match (touching, says(PRESSURE)) {
        (false, _) => 0.0,
        (true, true) => record.pressure.min(1024) as f32 / 1024.0,
        (true, false) => 0.5,
    };
    let tilt = [(TILT_X, record.tilt[0]), (TILT_Y, record.tilt[1])].map(|(what, tilt)| if says(what) { tilt as f32 } else { 0.0 });
    let twist = if says(ROTATION) { record.rotation as f32 } else { 0.0 };
    let (barrel, eraser) = (record.flags & BARREL != 0, record.flags & (INVERTED | ERASER) != 0);
    let pen = Stylus { pressure, tilt, twist, barrel, eraser, touching };
    let mods = Mods { shift: info.keys & MOD_SHIFT != 0, ctrl: info.keys & MOD_CTRL != 0, alt: false };
    let button = match info.change {
        1 => Some(Event::Press(Button::Left, mods)),
        2 => Some(Event::Release(Button::Left)),
        3 => Some(Event::Press(Button::Right, mods)),
        4 => Some(Event::Release(Button::Right)),
        5 => Some(Event::Press(Button::Middle, mods)),
        6 => Some(Event::Release(Button::Middle)),
        _ => None,
    };
    [Some(Event::Stylus(Some(pen))), Some(Event::Move(at)), button]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The records' sizes and field offsets match `winuser.h` on 64 bits.
    #[test]
    fn the_records_are_laid_out_as_windows_lays_them_out() {
        assert_eq!((size_of::<PointerInfo>(), size_of::<PointerTouchInfo>(), size_of::<PointerPenInfo>()), (96, 144, 120));
        let info = PointerInfo::default();
        let base = &raw const info as usize;
        let at = |field: usize| field - base;
        assert_eq!(
            [
                at(&raw const info.flags as usize),
                at(&raw const info.window as usize),
                at(&raw const info.at as usize),
                at(&raw const info.keys as usize),
                at(&raw const info.count as usize),
                at(&raw const info.change as usize)
            ],
            [12, 24, 32, 76, 80, 88]
        );
        let pen = PointerPenInfo::default();
        assert_eq!((&raw const pen.pressure as usize - &raw const pen as usize, &raw const pen.tilt as usize - &raw const pen as usize), (104, 112));
    }

    /// A pointer record of this kind and id, at this screen point, with these flags and button
    /// change.
    fn pointer(kind: u32, id: u32, at: [i32; 2], flags: u32, change: u32) -> PointerInfo {
        PointerInfo { kind, id, at, flags, change, ..Default::default() }
    }

    #[test]
    fn a_fingers_messages_say_where_it_lands_moves_and_lifts() {
        let (corner, scale) = ([100, 50], 1.5);
        let said = |msg: u32, flags: u32| {
            let record = PointerTouchInfo { pointer: pointer(PT_TOUCH, 7, [250, 200], flags, 0), ..Default::default() };
            finger(msg, &record, place(&record.pointer, corner, scale))
        };
        let touch = |phase| Some(Event::Touch(Touch { id: 7, position: pt(100.0, 100.0), phase }));
        assert_eq!(said(WM_POINTERDOWN, 0x0001_0006), touch(TouchPhase::Down));
        assert_eq!(said(WM_POINTERUPDATE, 0x0002_0006), touch(TouchPhase::Move));
        assert_eq!(said(WM_POINTERUP, 0x0004_0002), touch(TouchPhase::Up));
        // Cancelled by the system, or by losing the window.
        assert_eq!(said(WM_POINTERUP, 0x0004_8002), touch(TouchPhase::Cancel));
        assert_eq!(said(WM_POINTERCAPTURECHANGED, 0x0020_0000), touch(TouchPhase::Cancel));
        // Out of range, or an update of a finger not in contact: nothing.
        assert_eq!((said(WM_POINTERLEAVE, 0), said(WM_POINTERUPDATE, 0x0002_0002)), (None, None));
    }

    #[test]
    fn a_pens_messages_say_how_it_presses_leans_and_which_end_is_down() {
        let at = pt(10.0, 20.0);
        let record = |flags: u32, change: u32, pen_flags: u32, mask: u32| PointerPenInfo {
            pointer: PointerInfo { keys: MOD_CTRL, ..pointer(PT_PEN, 3, [0, 0], flags, change) },
            flags: pen_flags,
            mask,
            pressure: 768,
            rotation: 90,
            tilt: [30, -15],
        };
        // Hovering: no pressure, but tilt.
        let hover = stylus(WM_POINTERUPDATE, &record(0x0002_0002, 0, 0, PRESSURE | TILT_X | TILT_Y), at);
        let leaning = Stylus { pressure: 0.0, tilt: [30.0, -15.0], twist: 0.0, barrel: false, eraser: false, touching: false };
        assert_eq!(hover, [Some(Event::Stylus(Some(leaning))), Some(Event::Move(at)), None]);
        // Tip down: left button, three quarters pressure, rotated.
        let down = stylus(WM_POINTERDOWN, &record(0x0001_0016, 1, 0, PRESSURE | ROTATION | TILT_X | TILT_Y), at);
        let pressing = Stylus { pressure: 0.75, twist: 90.0, touching: true, ..leaning };
        assert_eq!(down, [Some(Event::Stylus(Some(pressing))), Some(Event::Move(at)), Some(Event::Press(Button::Left, Mods::CTRL))]);
        // With the barrel button, the right button; without pressure data, half pressure.
        let right = stylus(WM_POINTERDOWN, &record(0x0001_0026, 3, BARREL, 0), at);
        let barrel = Stylus { pressure: 0.5, tilt: [0.0; 2], twist: 0.0, barrel: true, eraser: false, touching: true };
        assert_eq!(right, [Some(Event::Stylus(Some(barrel))), Some(Event::Move(at)), Some(Event::Press(Button::Right, Mods::CTRL))]);
        assert_eq!(stylus(WM_POINTERUP, &record(0x0004_0002, 4, BARREL, 0), at)[2], Some(Event::Release(Button::Right)));
        // Inverted, it erases.
        let [rubbing, ..] = stylus(WM_POINTERDOWN, &record(0x0001_0016, 1, ERASER, PRESSURE), at);
        assert!(matches!(rubbing, Some(Event::Stylus(Some(Stylus { eraser: true, touching: true, .. })))));
        // Out of range, pen and pointer are gone.
        assert_eq!(stylus(WM_POINTERLEAVE, &record(0, 0, 0, 0), at), [Some(Event::Stylus(None)), Some(Event::Leave), None]);
    }
}
