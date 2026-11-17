//! Touch, pen and touchpad pinch input on X, through the X Input Extension (XI 2.2 and later):
//! read into the interface's events ([`Touch`], [`Stylus`] and [`Event::Zoom`]), as
//! `touch_windows` reads Windows' pointer records.
//!
//! By default X emulates a mouse from all of these: a touchscreen's first finger presses the
//! left button and the others are ignored, a pen is a mouse without pressure, and a touchpad
//! pinch is nothing. A window selecting touch events gets them instead of the emulated mouse;
//! and one selecting XI2 pointer events instead of core ones learns, with every motion and
//! press, which device caused it and its axis values. So each window of a program built with
//! `touch` selects both, plus touchpad pinches where X supports them (XI 2.4). XI2 pointer
//! events are converted here into the core events' shape, which the window handles as always:
//! the mouse is unchanged, wheel included. (Keys, and pointer enter and leave, still arrive as
//! core events.)
//!
//! Touches are selected by the window itself: X delivers a touch once no other client (a
//! gesture-handling window manager, say) holds a grab on it, with its full history. So nothing
//! a window hears of a touch is ever retracted.
//!
//! A pen is a device with a pressure axis ("Abs Pressure"); its eraser end is a separate device
//! whose name says so ("eraser"). Tilt is given in degrees, as pen drivers report it. X does
//! not report a pen leaving proximity: it is taken as gone when another device moves the
//! pointer, or the pointer leaves the window.
//!
//! Nothing is linked at build time: libXi is opened by name with the first window. On a display
//! without the extension, or below XI 2.2, windows receive core events, as in a program built
//! without the feature.
use crate::event::Event;
use crate::geom::pt;
use crate::touch::{Stylus, Touch, TouchPhase};
use crate::x11::{dlclose, dlopen, library, symbol};
use std::ffi::{CStr, c_char, c_int, c_uchar, c_uint, c_ulong, c_void};

library!(XInput, ["libXi.so.6", "libXi.so"], {
    fn XIQueryVersion(*mut c_void, *mut c_int, *mut c_int) -> c_int;
    fn XISelectEvents(*mut c_void, c_ulong, *mut EventMask, c_int) -> c_int;
    fn XIQueryDevice(*mut c_void, c_int, *mut c_int) -> *mut DeviceInfo;
    fn XIFreeDeviceInfo(*mut DeviceInfo) -> ();
    // (Xlib's own, which the extension needs: resolved through it, since Xlib is its
    // dependency.)
    fn XQueryExtension(*mut c_void, *const c_char, *mut c_int, *mut c_int, *mut c_int) -> c_int;
    fn XGetEventData(*mut c_void, *mut Cookie) -> c_int;
    fn XFreeEventData(*mut c_void, *mut Cookie) -> ();
});

/// The XI2 event types selected: `XI_ButtonPress`, `XI_ButtonRelease`, `XI_Motion`;
/// `XI_TouchBegin`, `XI_TouchUpdate`, `XI_TouchEnd`; `XI_GesturePinchBegin`, `Update`, `End`;
/// and display-wide `XI_HierarchyChanged`.
const PRESS: c_int = 4;
const RELEASE: c_int = 5;
const MOTION: c_int = 6;
const DEVICES_CHANGED: c_int = 11;
const TOUCH_BEGIN: c_int = 18;
const TOUCH_END: c_int = 20;
const PINCH_BEGIN: c_int = 27;
const PINCH_END: c_int = 29;
/// `XIValuatorClass`.
const AXIS: c_int = 2;
/// `XIAllDevices`, and `XIAllMasterDevices`.
const ALL: c_int = 0;
const ALL_MAIN: c_int = 1;

/// The generic event header (`XGenericEventCookie`): extension opcode, event type, and where X
/// keeps the payload until fetched (`XGetEventData`).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Cookie {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: *mut c_void,
    extension: c_int,
    evtype: c_int,
    cookie: c_uint,
    data: *mut c_void,
}

/// `XIEventMask`: a bit per selected event type, for a device.
#[repr(C)]
struct EventMask {
    device: c_int,
    length: c_int,
    mask: *mut c_uchar,
}

/// `XIDeviceInfo`: its id and name, and its classes, of which only valuators are used.
#[repr(C)]
struct DeviceInfo {
    id: c_int,
    name: *const c_char,
    used: c_int,
    attachment: c_int,
    enabled: c_int,
    classes: c_int,
    class: *const *const ValuatorClass,
}

/// `XIValuatorClassInfo`: its index among the device's valuators, its label atom, and its
/// range. (Every class starts with its type, which says whether it is one.)
#[repr(C)]
#[derive(Clone, Copy)]
struct ValuatorClass {
    kind: c_int,
    source: c_int,
    number: c_int,
    label: c_ulong,
    min: f64,
    max: f64,
    value: f64,
    resolution: c_int,
    mode: c_int,
}

/// `XIDeviceEvent`: a pointer or touch event.
#[repr(C)]
#[derive(Clone, Copy)]
struct DeviceEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: *mut c_void,
    extension: c_int,
    evtype: c_int,
    time: c_ulong,
    /// The master pointer it moved, and the source device that moved it.
    device: c_int,
    source: c_int,
    /// The button, or the touch id.
    detail: c_int,
    root: c_ulong,
    window: c_ulong,
    child: c_ulong,
    /// Its root and window coordinates, in fractional pixels.
    root_x: f64,
    root_y: f64,
    x: f64,
    y: f64,
    flags: c_int,
    /// The buttons held before it: a bit each, by number.
    buttons: Bits,
    /// Its valuators: the values of those its mask bits name, in order.
    axes: Bits,
    values: *const f64,
    /// The held modifiers: `effective` (the last) as now, and the rest.
    keys: [c_int; 4],
    group: [c_int; 4],
}

/// `XIButtonState`/`XIValuatorState`: a byte count, and a pointer to that many mask bytes.
#[repr(C)]
#[derive(Clone, Copy)]
struct Bits {
    length: c_int,
    bits: *const c_uchar,
}

/// `XIGesturePinchEvent`: chiefly the pinch's cumulative scale since it began.
#[repr(C)]
#[derive(Clone, Copy)]
struct PinchEvent {
    kind: c_int,
    serial: c_ulong,
    sent: c_int,
    display: *mut c_void,
    extension: c_int,
    evtype: c_int,
    time: c_ulong,
    device: c_int,
    source: c_int,
    fingers: c_int,
    root: c_ulong,
    window: c_ulong,
    child: c_ulong,
    root_x: f64,
    root_y: f64,
    x: f64,
    y: f64,
    moved: [f64; 4],
    scale: f64,
    turned: f64,
    flags: c_int,
    keys: [c_int; 4],
    group: [c_int; 4],
}

/// A pointer event in core event terms: its type (6 `MotionNotify`, 4 `ButtonPress`, 5
/// `ButtonRelease`), window and position, root position, time, modifier and button state, and
/// button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pointer {
    pub(crate) kind: c_int,
    pub(crate) window: c_ulong,
    pub(crate) at: [c_int; 2],
    pub(crate) root: [c_int; 2],
    pub(crate) time: c_ulong,
    pub(crate) state: c_uint,
    pub(crate) detail: c_uint,
}

/// What an XI2 event means for the windows.
#[derive(Debug, PartialEq)]
pub(crate) enum Heard {
    /// Nothing they need.
    Nothing,
    /// A pointer event, which its window handles as the core event; preceded by pen state, if
    /// that changed.
    Pointer(Option<Event>, Pointer),
    /// A touch, or a touchpad pinch, in a window.
    Said(c_ulong, Event),
}

/// A pen as X reports it: whether it is the eraser; and its pressure, x-tilt and y-tilt
/// valuators: each one's index among the device's valuators and its range.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pen {
    eraser: bool,
    axes: [Option<(usize, f64, f64)>; 3],
}

/// The pen in proximity: its device id, its last valuator values (events report only changes),
/// and its state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Near {
    device: c_int,
    values_now: [f64; 3],
    stylus: Stylus,
}

/// A program's XI2 state on one display, for pointer, touch, pen and pinch input.
pub(crate) struct Inputs {
    xi: XInput,
    display: *mut c_void,
    /// The extension's major opcode: its events carry it.
    opcode: c_int,
    /// X reports touchpad pinches (XI 2.4 or later).
    pinches: bool,
    /// The label atoms of a pen's pressure, x-tilt and y-tilt valuators.
    labels: [c_ulong; 3],
    /// Each device that has moved the pointer, by id: a pen, or not. (Cleared on hierarchy
    /// changes: X may reuse an id.)
    devices: Vec<(c_int, Option<Pen>)>,
    heard: Hearing,
}

/// State kept between events: the pen in proximity, and a touchpad pinch's last reported scale.
#[derive(Default)]
struct Hearing {
    near: Option<Near>,
    pinched: f64,
}

impl Inputs {
    /// The extension on `display`, for a touch-enabled program: XI 2.2 at least, 2.4 where
    /// available; with hierarchy changes selected (on `root`, the root window). `labels` are
    /// the atoms of a pen's valuator names ("Abs Pressure", "Abs Tilt X", "Abs Tilt Y"). None
    /// where the extension is missing.
    ///
    /// # Safety
    /// `display` is open, and stays so while this is used.
    pub(crate) unsafe fn open(display: *mut c_void, root: c_ulong, labels: [c_ulong; 3]) -> Option<Inputs> {
        let xi = XInput::open()?;
        let (mut opcode, mut first, mut failed, mut version) = (0, 0, 0, [2, 4]);
        // SAFETY: the documented calls; the name is NUL-terminated, and each pointer is to a
        // number filled in.
        let found = unsafe { (xi.XQueryExtension)(display, c"XInputExtension".as_ptr(), &mut opcode, &mut first, &mut failed) != 0 };
        if !found || unsafe { (xi.XIQueryVersion)(display, &mut version[0], &mut version[1]) } != 0 || version < [2, 2] {
            return None;
        }
        let mut changes = (1u32 << DEVICES_CHANGED).to_le_bytes();
        let mut asked = EventMask { device: ALL, length: 4, mask: changes.as_mut_ptr() };
        // SAFETY: as above; the mask outlives the call.
        unsafe { (xi.XISelectEvents)(display, root, &mut asked, 1) };
        Some(Inputs { xi, display, opcode, pinches: version >= [2, 4], labels, devices: Vec::new(), heard: Hearing::default() })
    }

    /// A program window selects XI2 pointer, touch and pinch events from all master devices.
    pub(crate) fn select(&self, window: c_ulong) {
        let pinching = if self.pinches { 7 << PINCH_BEGIN } else { 0 };
        let kinds = 1u32 << PRESS | 1 << RELEASE | 1 << MOTION | 7 << TOUCH_BEGIN | pinching;
        let mut bits = kinds.to_le_bytes();
        let mut asked = EventMask { device: ALL_MAIN, length: 4, mask: bits.as_mut_ptr() };
        // SAFETY: the display is open (see `open`), and the mask outlives the call. A destroyed
        // window is ignored.
        unsafe { (self.xi.XISelectEvents)(self.display, window, &mut asked, 1) };
    }

    /// The pen left, if one was in proximity: news for the window the pointer leaves.
    pub(crate) fn pen_gone(&mut self) -> Option<Event> {
        self.heard.pen_gone()
    }

    /// What a delivered event means, if it is an XI2 one; `scale` is pixels per unit. (Its
    /// cookie data is freed.)
    ///
    /// # Safety
    /// `event` was delivered on the display, of type 35 (`GenericEvent`).
    pub(crate) unsafe fn read(&mut self, event: &mut Cookie, scale: f32) -> Heard {
        // SAFETY: the event is X's; its data is fetched and freed on its own display; its
        // layout follows from its type.
        unsafe {
            if event.extension != self.opcode || (self.xi.XGetEventData)(self.display, event) == 0 {
                return Heard::Nothing;
            }
            let heard = match event.evtype {
                PRESS | RELEASE | MOTION => self.pointer(&*event.data.cast::<DeviceEvent>()),
                TOUCH_BEGIN..=TOUCH_END => finger(&*event.data.cast::<DeviceEvent>(), scale),
                PINCH_BEGIN..=PINCH_END => self.heard.pinch(&*event.data.cast::<PinchEvent>()),
                DEVICES_CHANGED => self.devices_changed(),
                _ => Heard::Nothing,
            };
            (self.xi.XFreeEventData)(self.display, event);
            heard
        }
    }

    /// Devices were added or removed: what is known of them is cleared.
    fn devices_changed(&mut self) -> Heard {
        self.devices.clear();
        Heard::Nothing
    }

    /// A motion, press or release, by the device the event names.
    fn pointer(&mut self, e: &DeviceEvent) -> Heard {
        let pen = self.device(e.source);
        self.heard.pointer(e, pen)
    }

    /// What a device id is: a pen with its valuators, or not. X is queried the first time a
    /// device moves the pointer.
    fn device(&mut self, id: c_int) -> Option<Pen> {
        if let Some((_, known)) = self.devices.iter().find(|(known, _)| *known == id) {
            return *known;
        }
        // SAFETY: the display is open; X's answer is read as documented, and freed.
        let pen = unsafe {
            let mut count = 0;
            let info = (self.xi.XIQueryDevice)(self.display, id, &mut count);
            let pen = (!info.is_null() && count > 0).then(|| pen_of(&*info, self.labels)).flatten();
            if !info.is_null() {
                (self.xi.XIFreeDeviceInfo)(info);
            }
            pen
        };
        self.devices.push((id, pen));
        pen
    }
}

impl Hearing {
    /// A motion, press or release, by `pen` or a non-pen device: in the core event's terms,
    /// with any pen state first. (The core event has whole-pixel positions: the fraction is
    /// dropped.)
    fn pointer(&mut self, e: &DeviceEvent, pen: Option<Pen>) -> Heard {
        let (buttons, axes) = (bits(&e.buttons), bits(&e.axes));
        let held = buttons.first().map_or(0, |first| u32::from(*first) >> 1 & 0x1f);
        let values = match e.values.is_null() {
            true => &[][..],
            // SAFETY: X gives a value for each valuator its mask names.
            false => unsafe { std::slice::from_raw_parts(e.values, axes.iter().map(|byte| byte.count_ones() as usize).sum()) },
        };
        let news = match pen {
            Some(pen) => self.pen_event(e.source, pen, after(e.evtype, e.detail, held), axes, values),
            None => self.pen_gone(),
        };
        let whole = |at: f64| at.floor() as c_int;
        let state = e.keys[3] as c_uint & 0xff | held << 8;
        let (at, root) = ([whole(e.x), whole(e.y)], [whole(e.root_x), whole(e.root_y)]);
        Heard::Pointer(news, Pointer { kind: e.evtype, window: e.window, at, root, time: e.time, state, detail: e.detail as c_uint })
    }

    /// Pen `device`'s current state, with the buttons `held` after its event (a bit each, the
    /// first for its tip), and the values of the valuators the `axes` mask names: news if it
    /// changed. (Tip pressed, it touches; with a barrel button, that is held.)
    fn pen_event(&mut self, device: c_int, pen: Pen, held: u32, axes: &[u8], values: &[f64]) -> Option<Event> {
        let mut values_now = self.near.filter(|near| near.device == device).map_or([0.0; 3], |near| near.values_now);
        for (which, axis) in pen.axes.iter().enumerate() {
            if let Some(value) = axis.and_then(|(number, ..)| value_of(axes, values, number)) {
                values_now[which] = value;
            }
        }
        let share = |(_, least, most): (usize, f64, f64)| if most > least { ((values_now[0] - least) / (most - least)).clamp(0.0, 1.0) as f32 } else { 0.5 };
        let touching = held & 1 != 0;
        let pressure = if touching { pen.axes[0].map_or(0.5, share) } else { 0.0 };
        let tilt = [1, 2].map(|which| pen.axes[which].map_or(0.0, |_| values_now[which].clamp(-90.0, 90.0) as f32));
        let stylus = Stylus { pressure, tilt, twist: 0.0, barrel: held & 6 != 0, eraser: pen.eraser, touching };
        let was = self.near.replace(Near { device, values_now, stylus });
        (was.map(|was| was.stylus) != Some(stylus)).then_some(Event::Stylus(Some(stylus)))
    }

    /// The pen left, if one was in proximity.
    fn pen_gone(&mut self) -> Option<Event> {
        self.near.take().map(|_| Event::Stylus(None))
    }

    /// A touchpad pinch began, continued or ended: the target scales by its growth since last
    /// reported.
    fn pinch(&mut self, e: &PinchEvent) -> Heard {
        if e.evtype == PINCH_BEGIN {
            self.pinched = 1.0;
        }
        let by = e.scale / self.pinched;
        self.pinched = e.scale;
        match by.is_finite() && by > 0.0 && by != 1.0 {
            true => Heard::Said(e.window, Event::Zoom(by as f32)),
            false => Heard::Nothing,
        }
    }
}

/// The pen a device is, if any: it has a pressure valuator (the first of `labels`, a pen's
/// valuator atoms).
///
/// # Safety
/// `info` is X's complete description of the device.
unsafe fn pen_of(info: &DeviceInfo, labels: [c_ulong; 3]) -> Option<Pen> {
    // SAFETY: X lists as many classes as it says, and the device name is NUL-terminated.
    let (classes, name) = unsafe { (std::slice::from_raw_parts(info.class, info.classes.max(0) as usize), CStr::from_ptr(info.name).to_bytes()) };
    let mut axes = [None; 3];
    // SAFETY: each class starts with its type, and a valuator class is complete.
    for class in classes.iter().filter(|class| unsafe { (***class).kind } == AXIS).map(|class| unsafe { **class }) {
        if let Some(which) = labels.iter().position(|label| *label == class.label) {
            axes[which] = Some((class.number.max(0) as usize, class.min, class.max));
        }
    }
    let eraser = name.windows(6).any(|word| word.eq_ignore_ascii_case(b"eraser"));
    axes[0].map(|_| Pen { eraser, axes })
}

/// The bytes of a mask X passed, valid while its event lasts.
fn bits(bits: &Bits) -> &[u8] {
    match bits.bits.is_null() {
        true => &[],
        // SAFETY: X passes as many bytes as it says; they are read before the event is freed.
        false => unsafe { std::slice::from_raw_parts(bits.bits, bits.length.max(0) as usize) },
    }
}

/// The buttons held after event `kind` of button `button`, from those held before (a bit each
/// for the first five, the first for button 1).
fn after(kind: c_int, button: c_int, held: u32) -> u32 {
    let bit = if (1..=5).contains(&button) { 1 << (button - 1) } else { 0 };
    match kind {
        PRESS => held | bit,
        RELEASE => held & !bit,
        _ => held,
    }
}

/// The value an event gives valuator `number`, if any: from `values`, which belong to the
/// valuators `axes` names, in order.
fn value_of(axes: &[u8], values: &[f64], number: usize) -> Option<f64> {
    let named = |at: usize| axes.get(at / 8).is_some_and(|byte| byte >> (at % 8) & 1 != 0);
    named(number).then(|| values.get((0..number).filter(|at| named(*at)).count()).copied()).flatten()
}

/// A touch began, moved or ended in a window, at a fractional pixel position.
fn finger(e: &DeviceEvent, scale: f32) -> Heard {
    let phase = [TouchPhase::Down, TouchPhase::Move, TouchPhase::Up][(e.evtype - TOUCH_BEGIN) as usize];
    Heard::Said(e.window, Event::Touch(Touch { id: e.detail as u32, position: pt(e.x as f32 / scale, e.y as f32 / scale), phase }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::mem::offset_of;
    use std::ptr::{null, null_mut};

    /// The records match the sizes in `XInput2.h` and `Xlib.h` on 64-bit, each field at its
    /// offset.
    #[test]
    fn the_records_are_laid_out_as_x_lays_them_out() {
        let sizes = [size_of::<Cookie>(), size_of::<EventMask>(), size_of::<DeviceInfo>(), size_of::<ValuatorClass>(), size_of::<DeviceEvent>(), size_of::<PinchEvent>()];
        assert_eq!(sizes, [56, 16, 40, 56, 200, 208]);
        assert_eq!([offset_of!(Cookie, extension), offset_of!(Cookie, evtype), offset_of!(Cookie, data)], [32, 36, 48]);
        assert_eq!([offset_of!(DeviceInfo, name), offset_of!(DeviceInfo, classes), offset_of!(DeviceInfo, class)], [8, 28, 32]);
        assert_eq!([offset_of!(ValuatorClass, number), offset_of!(ValuatorClass, label), offset_of!(ValuatorClass, min), offset_of!(ValuatorClass, max)], [8, 16, 24, 32]);
        let device = [
            offset_of!(DeviceEvent, time),
            offset_of!(DeviceEvent, device),
            offset_of!(DeviceEvent, source),
            offset_of!(DeviceEvent, detail),
            offset_of!(DeviceEvent, window),
            offset_of!(DeviceEvent, root_x),
            offset_of!(DeviceEvent, x),
            offset_of!(DeviceEvent, flags),
            offset_of!(DeviceEvent, buttons),
            offset_of!(DeviceEvent, axes),
            offset_of!(DeviceEvent, values),
            offset_of!(DeviceEvent, keys),
        ];
        assert_eq!(device, [40, 48, 52, 56, 72, 88, 104, 120, 128, 144, 160, 168]);
        assert_eq!([offset_of!(PinchEvent, window), offset_of!(PinchEvent, x), offset_of!(PinchEvent, scale), offset_of!(PinchEvent, flags)], [72, 104, 152, 168]);
    }

    /// A pointer event as X delivers it: type `kind`, by `button` (or none), at a point in
    /// window 7, with buttons `held` before it (mask bits, as X counts) and the valuators
    /// `axes` names giving `values`.
    fn event(kind: c_int, button: c_int, at: [f64; 2], held: &[u8], axes: &[u8], values: &[f64]) -> DeviceEvent {
        let bits = |bytes: &[u8]| Bits { length: bytes.len() as c_int, bits: bytes.as_ptr() };
        DeviceEvent {
            kind: 35,
            serial: 0,
            sent: 0,
            display: null_mut(),
            extension: 131,
            evtype: kind,
            time: 1000,
            device: 2,
            source: 12,
            detail: button,
            root: 1,
            window: 7,
            child: 0,
            root_x: at[0] + 100.0,
            root_y: at[1] + 50.0,
            x: at[0],
            y: at[1],
            flags: 0,
            buttons: bits(held),
            axes: bits(axes),
            values: values.as_ptr(),
            keys: [4, 0, 0, 4],
            group: [0; 4],
        }
    }

    /// A pen whose fifth valuator is pressure, 0 to 1,000, and sixth and seventh tilt; or its
    /// eraser.
    fn pen(eraser: bool) -> Pen {
        Pen { eraser, axes: [Some((4, 0.0, 1000.0)), Some((5, -64.0, 63.0)), Some((6, -64.0, 63.0))] }
    }

    #[test]
    fn a_pens_events_say_how_hard_it_presses_how_it_leans_and_which_end_is_down() {
        let mut heard = Hearing::default();
        // In proximity, tilted: touching nothing, so no pressure. The core event's fields come
        // with it, in whole pixels: the held modifiers (Control) and buttons (none yet).
        let hover = heard.pointer(&event(MOTION, 0, [10.75, 20.5], &[0], &[0b0111_0011], &[110.75, 70.5, 300.0, 30.0, -15.0]), Some(pen(false)));
        let leaning = Stylus { pressure: 0.0, tilt: [30.0, -15.0], twist: 0.0, barrel: false, eraser: false, touching: false };
        let pointer = Pointer { kind: MOTION, window: 7, at: [10, 20], root: [110, 70], time: 1000, state: 4, detail: 0 };
        assert_eq!(hover, Heard::Pointer(Some(Event::Stylus(Some(leaning))), pointer));
        // Its tip goes down at a quarter pressure; then three quarters, reported alone.
        let down = heard.pointer(&event(PRESS, 1, [10.0, 20.0], &[0], &[0b0001_0000], &[250.0]), Some(pen(false)));
        let pressing = Stylus { pressure: 0.25, touching: true, ..leaning };
        assert_eq!(down, Heard::Pointer(Some(Event::Stylus(Some(pressing))), Pointer { kind: PRESS, at: [10, 20], detail: 1, root: [110, 70], ..pointer }));
        let harder = heard.pointer(&event(MOTION, 0, [10.0, 21.0], &[0b10], &[0b0001_0000], &[750.0]), Some(pen(false)));
        let Heard::Pointer(news, moved) = harder else { panic!("{harder:?}") };
        assert_eq!((news, moved.state), (Some(Event::Stylus(Some(Stylus { pressure: 0.75, ..pressing }))), 4 | 1 << 8));
        // An event with no new pen state is no pen news.
        assert!(matches!(heard.pointer(&event(MOTION, 0, [11.0, 21.0], &[0b10], &[0b11], &[111.0, 71.0]), Some(pen(false))), Heard::Pointer(None, _)));
        // The barrel button, and the tip released: it hovers again.
        let side = heard.pointer(&event(PRESS, 3, [11.0, 21.0], &[0b10], &[], &[]), Some(pen(false)));
        assert!(matches!(side, Heard::Pointer(Some(Event::Stylus(Some(Stylus { barrel: true, touching: true, .. }))), _)));
        let up = heard.pointer(&event(RELEASE, 1, [11.0, 21.0], &[0b1010], &[], &[]), Some(pen(false)));
        assert!(matches!(up, Heard::Pointer(Some(Event::Stylus(Some(Stylus { pressure: 0.0, barrel: true, touching: false, .. }))), _)));
        // The eraser, pressed: it erases, with pressure from its own valuator.
        let rubbing = heard.pointer(&event(PRESS, 1, [11.0, 21.0], &[0], &[0b0001_0000], &[500.0]), Some(pen(true)));
        assert!(matches!(rubbing, Heard::Pointer(Some(Event::Stylus(Some(Stylus { pressure: 0.5, eraser: true, touching: true, .. }))), _)));
        // The mouse moves: the pen leaves, once.
        assert!(matches!(heard.pointer(&event(MOTION, 0, [30.0, 40.0], &[0], &[0b11], &[1.0, 1.0]), None), Heard::Pointer(Some(Event::Stylus(None)), _)));
        assert!(matches!(heard.pointer(&event(MOTION, 0, [31.0, 40.0], &[0], &[0b11], &[1.0, 1.0]), None), Heard::Pointer(None, _)));
    }

    /// A pen without a pressure valuator presses halfway; and an event with an empty valuator
    /// mask (and no values) says nothing of them.
    #[test]
    fn a_pen_that_does_not_say_how_hard_presses_half_way() {
        let mut heard = Hearing::default();
        let quiet = Pen { eraser: false, axes: [None; 3] };
        let mut down = event(PRESS, 1, [0.0, 0.0], &[], &[], &[]);
        down.values = null();
        assert!(matches!(heard.pointer(&down, Some(quiet)), Heard::Pointer(Some(Event::Stylus(Some(Stylus { pressure: 0.5, touching: true, .. }))), _)));
    }

    #[test]
    fn a_touchpads_pinch_says_how_much_larger_since_it_was_last_heard_of() {
        let mut heard = Hearing::default();
        let pinch = |kind: c_int, scale: f64| PinchEvent {
            kind: 35,
            serial: 0,
            sent: 0,
            display: null_mut(),
            extension: 131,
            evtype: kind,
            time: 0,
            device: 2,
            source: 9,
            fingers: 2,
            root: 1,
            window: 7,
            child: 0,
            root_x: 0.0,
            root_y: 0.0,
            x: 0.0,
            y: 0.0,
            moved: [0.0; 4],
            scale,
            turned: 0.0,
            flags: 0,
            keys: [0; 4],
            group: [0; 4],
        };
        let heard =
            [(PINCH_BEGIN, 1.0), (PINCH_BEGIN + 1, 1.5), (PINCH_BEGIN + 1, 1.8), (PINCH_END, 1.8), (PINCH_BEGIN, 1.0), (PINCH_BEGIN + 1, 0.5)].map(|(kind, scale)| heard.pinch(&pinch(kind, scale)));
        let zoom = |by: f32| Heard::Said(7, Event::Zoom(by));
        assert_eq!(heard, [Heard::Nothing, zoom(1.5), zoom((1.8f64 / 1.5) as f32), Heard::Nothing, Heard::Nothing, zoom(0.5)]);
    }

    #[test]
    fn a_fingers_events_say_where_it_lands_moves_and_lifts() {
        let said = |kind: c_int| finger(&event(kind, 3, [100.0, 50.5], &[], &[], &[]), 2.0);
        let touch = |phase| Heard::Said(7, Event::Touch(Touch { id: 3, position: pt(50.0, 25.25), phase }));
        assert_eq!([TOUCH_BEGIN, TOUCH_BEGIN + 1, TOUCH_END].map(said), [touch(TouchPhase::Down), touch(TouchPhase::Move), touch(TouchPhase::Up)]);
    }

    #[test]
    fn the_value_of_an_axis_is_found_among_the_values_of_those_an_event_names() {
        let (axes, values) = ([0b1001_0001, 0b1], [1.0, 2.0, 3.0, 4.0]);
        let found = [0, 4, 7, 8, 1, 9, 30].map(|number| value_of(&axes, &values, number));
        assert_eq!(found, [Some(1.0), Some(2.0), Some(3.0), Some(4.0), None, None, None]);
        assert_eq!([after(PRESS, 3, 0b1), after(RELEASE, 1, 0b101), after(MOTION, 2, 0b1), after(PRESS, 9, 0)], [0b101, 0b100, 0b1, 0]);
    }
}
