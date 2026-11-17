//! Touchscreen fingers, pens, and touchpad pinches: turned into what the interface already
//! knows from the mouse, plus the few things a mouse cannot do.
//!
//! The system reports each finger's down, move and up ([`Touch`]), and here that becomes what
//! users expect of a touchscreen on Windows and Linux:
//!
//! - A tap is a click where the finger was; two in quick succession are a double click.
//! - A finger held still for half a second is a context menu press: a right click.
//! - A finger dragged across a box that scrolls that way scrolls it, and released while moving,
//!   the box keeps going at that speed, decelerating to a stop. Nothing under a scrolling
//!   finger is pressed or clicked, and a finger landing on a coasting box stops it.
//! - Elsewhere, and on a dragged part (a slider, a splitter, a dock tab, a scroll bar, anything
//!   held with `held`), a finger is the mouse with its button down.
//! - Two fingers pinch and pan a zooming box (`zoom`); elsewhere they scroll what is under
//!   them.
//!
//! A finger is wider than a pointer: one landing beside every pressable part presses the
//! nearest within reach. No tooltip appears under a finger, and once the fingers lift, nothing
//! is hovered any more.
//!
//! A pen is the mouse, and also reports pressure, tilt, and its barrel button or eraser
//! (`stylus`). Ctrl+wheel, which is also what a touchpad pinch is on Windows, zooms as two
//! fingers do. A pan-zoom area (`pan_zoom`) takes all of these as a zooming box does.
//!
//! Gestures are recognized here from the interface's clock (hold durations, velocities), so
//! every gesture replays exactly. Windows' pointer records are read into [`Touch`], [`Stylus`]
//! and [`Event::Zoom`] by `touch_windows`, X's XInput 2 touch, pen and pinch events by
//! `xinput`, and a Wayland compositor's (`wl_touch`, `tablet`, `pointer-gestures`) by
//! `wayland`.
//!
//! Fingers and pens are the feature `touch`, which a program enables when it wants them.
//! Without it, only the system's mouse emulation exists: a tap clicks, a finger drag drags, a
//! pen presses as the mouse does. (Ctrl+wheel still zooms, and so does a Windows touchpad
//! pinch, which arrives as Ctrl+wheel.) (Under Wayland, where no compositor emulates a mouse
//! from touch, the library emulates one from the first finger.)
use crate::event::{Button, Event, Mods};
use crate::geom::{Point, Rect, pt};
use crate::id::Id;
use crate::ui::{FRAME, Response, Ui};
use std::time::Duration;

/// How long a finger is held still before it is a context menu press.
const HOLD: Duration = Duration::from_millis(500);
/// How far a finger may move and still tap; further, it scrolls or drags.
const SLOP: f32 = 8.0;
/// How far beside a part a finger may land on nothing and still press it.
const REACH: f32 = 12.0;
/// How near the previous tap a tap must land to make a double click.
pub(crate) const TAP_NEAR: f32 = 16.0;
/// Fling deceleration: speed falls to about a third in this many seconds. And the speeds, in
/// units per second, below which a release flings nothing, above which none goes, and below
/// which a coasting box stops.
const COAST: f32 = 0.325;
const FLICK: f32 = 100.0;
const FASTEST: f32 = 8000.0;
const STILL: f32 = 20.0;
/// How far back finger positions count toward the release velocity.
const LATELY: Duration = Duration::from_millis(100);
/// The zoom factor per Ctrl+wheel notch.
const NOTCH: f32 = 1.25;

/// The zoom factor for `notches` of Ctrl+wheel: a quarter larger per notch forward (away from
/// the user), as much smaller (four fifths) per notch back. A touchpad pinch is that too, in
/// fractional notches.
pub(crate) fn wheel_zoom(notches: f32) -> f32 {
    NOTCH.powf(notches)
}

/// One finger on the screen, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    /// The system's id for the finger, stable from down to up.
    pub id: u32,
    /// Its position, in units from the window's top-left.
    pub position: Point,
    /// What it did.
    pub phase: TouchPhase,
}

/// A finger event phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TouchPhase {
    /// It touched down.
    Down,
    /// It moved.
    Move,
    /// It lifted.
    Up,
    /// The system cancelled it (another program, or a system gesture, took it): whatever it was
    /// doing comes to nothing, and nothing is clicked.
    Cancel,
}

/// A pen (stylus) near the screen, beyond what the mouse it also is reports. For drawing apps:
/// stroke width and opacity, and erasing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stylus {
    /// Tip pressure, 0 to 1: 0 while not touching. A pen without pressure reports 0.5 while
    /// touching.
    pub pressure: f32,
    /// Tilt in degrees from upright: right, and toward the user (each -90 to 90). 0 where not
    /// reported.
    pub tilt: [f32; 2],
    /// Rotation about its own axis, 0 to 360 degrees. 0 where not reported.
    pub twist: f32,
    /// The barrel button is held.
    pub barrel: bool,
    /// It is the eraser end, or the button that makes the tip erase is held.
    pub eraser: bool,
    /// It touches the screen; else it hovers.
    pub touching: bool,
}

/// What two fingers, a touchpad pinch or Ctrl+wheel did to a zooming box since the last run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    /// The scale factor for the box's content: over 1 to grow, under to shrink.
    pub factor: f32,
    /// The window point it scales about: what is there now stays there.
    pub center: Point,
    /// How far the fingers panned it meanwhile, in units: right and down.
    pub pan: Point,
}

/// What the fingers are doing together.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Doing {
    /// No finger is down; or their gesture is done, and nothing more comes until they lift.
    #[default]
    Nothing,
    /// One finger is down and has not yet shown whether it taps, holds or moves. (`true`: it
    /// stopped a coasting box, and does not tap.)
    Waiting(bool),
    /// One finger is the mouse with its button down.
    Mouse,
    /// The fingers scroll box `id`: its scroll offset and the fingers' centroid when they began
    /// (or last hit its end), and the axes.
    Scrolling { id: Id, from: [f32; 2], start: Point, axes: [bool; 2] },
    /// Two fingers zoom box `id`: their distance and centroid at their last move.
    Pinching { id: Id, apart: f32, middle: Point },
}

/// A box coasting after a fling: from where, at what velocity (units per second per axis),
/// since when.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coast {
    id: Id,
    from: [f32; 2],
    speed: [f32; 2],
    since: Duration,
}

impl Coast {
    /// Its position at `now`, and whether still moving: exponential decay, its speed falling by
    /// the same fraction each moment.
    fn at(&self, now: Duration) -> ([f32; 2], bool) {
        let left = (-now.saturating_sub(self.since).as_secs_f32() / COAST).exp();
        let to = [0, 1].map(|axis| self.from[axis] + self.speed[axis] * COAST * (1.0 - left));
        (to, self.speed[0].hypot(self.speed[1]) * left > STILL)
    }
}

/// The interface's finger and pen state between runs.
#[derive(Default)]
pub(crate) struct Touching {
    /// The fingers down: each one's id, where it landed, and where it is.
    fingers: Vec<(u32, Point, Point)>,
    doing: Doing,
    /// When the first of them landed.
    since: Duration,
    /// The fingers' recent centroids and times: for the release velocity.
    lately: Vec<(Duration, Point)>,
    coast: Option<Coast>,
    /// The button to release after the next pass: a tap and a long press are a press, then once
    /// seen, a release.
    release: Option<Button>,
    /// The fingers lifted: the pointer goes with them once the release is seen.
    lifted: bool,
    /// The next press is a finger's; and the last one was.
    pressing: bool,
    finger: bool,
    /// The dragged parts, and the zooming boxes (with each one's rect), as of the last
    /// described pass. (`Ui::begin` clears them.)
    pub dragged: Vec<Id>,
    pub zooms: Vec<(Id, Rect)>,
    /// A zoom waiting for its box: which box, and how.
    zoom: Option<(Id, Zoom)>,
    /// The pen, while in proximity.
    stylus: Option<Stylus>,
}

impl Touching {
    /// Whether the press being processed is a finger's (and so whether the last press was). A
    /// double tap is looser than a double click: a finger lands less precisely than a pointer.
    pub(crate) fn finger_pressed(&mut self) -> bool {
        self.finger = std::mem::take(&mut self.pressing);
        self.finger
    }

    /// The first two fingers' centroid and distance.
    fn middle(&self) -> (Point, f32) {
        let ((_, _, one), (_, _, other)) = (self.fingers[0], *self.fingers.get(1).unwrap_or(&self.fingers[0]));
        (pt((one.x + other.x) / 2.0, (one.y + other.y) / 2.0), (one.x - other.x).hypot(one.y - other.y))
    }
}

/// The distance from `p` to the nearest point of `r`.
fn gap(r: Rect, p: Point) -> f32 {
    let (x, y) = ((r.x - p.x).max(p.x - r.right()).max(0.0), (r.y - p.y).max(p.y - r.bottom()).max(0.0));
    x.hypot(y)
}

/// Fingers, pens and pinches.
impl Ui {
    /// Processes a finger event. Says whether the interface must run. (Without `touch`, a
    /// finger means nothing to the interface: where the system emulates a mouse from it, it is
    /// that.)
    pub(crate) fn touched(&mut self, touch: Touch) -> bool {
        if !cfg!(feature = "touch") {
            return false;
        }
        match touch.phase {
            TouchPhase::Down => self.finger_down(touch.id, touch.position),
            TouchPhase::Move => self.finger_moved(touch.id, touch.position),
            TouchPhase::Up | TouchPhase::Cancel => self.finger_up(touch.id, touch.phase == TouchPhase::Up),
        }
    }

    /// A finger touched down. The first one moves the pointer there; and on a dragged part, or
    /// on nothing scrollable, it presses at once, as the mouse does. Else it waits to see what
    /// it does: a scroll box cannot tell a tap from a scroll until the finger lifts or moves.
    fn finger_down(&mut self, finger: u32, at: Point) -> bool {
        let touch = &mut self.touch;
        touch.fingers.retain(|(number, ..)| *number != finger);
        touch.fingers.push((finger, at, at));
        match touch.fingers.len() {
            1 => {}
            2 => return self.second_finger(),
            _ => return false,
        }
        (touch.since, touch.lifted) = (self.now, false);
        touch.lately.clear();
        let stopped = touch.coast.take().is_some();
        let landed = self.within_reach(at);
        self.event(Event::Move(landed));
        self.find_hot();
        // (A finger holds a scroll bar as the mouse does: its id is no box's.)
        let dragged = self.hot != 0 && (self.touch.dragged.contains(&self.hot) || !self.visible_boxes.rects.contains_key(&self.hot));
        if dragged || !stopped && self.scroller_under(landed, 0).or(self.scroller_under(landed, 1)).is_none() {
            self.press_by_finger();
        } else {
            self.touch.doing = Doing::Waiting(stopped);
        }
        true
    }

    /// The finger is the mouse with its left button pressed at the pointer.
    fn press_by_finger(&mut self) {
        (self.touch.doing, self.touch.pressing) = (Doing::Mouse, true);
        self.event(Event::Press(Button::Left, Mods::NONE));
    }

    /// Where a finger landing at `at` presses: there, where a pointer-sensing part is; else (on
    /// nothing, or on a menu's or dialog's background) on the nearest part within reach above
    /// that background, at its point nearest the finger.
    fn within_reach(&self, at: Point) -> Point {
        let (hits, floors) = (&self.visible_boxes.hits, &self.visible_boxes.layers);
        let top = hits.iter().rposition(|(_, r)| r.contains(at));
        if top.is_some_and(|top| !floors.iter().any(|(_, floor)| *floor == hits[top].0)) {
            return at;
        }
        let over = &hits[top.map_or(0, |top| top + 1)..];
        let nearest = over.iter().map(|(_, r)| (gap(*r, at), *r)).filter(|(far, r)| *far <= REACH && !r.is_empty()).min_by(|a, b| a.0.total_cmp(&b.0));
        nearest.map_or(at, |(_, r)| pt(at.x.clamp(r.x, r.right() - 0.5), at.y.clamp(r.y, r.bottom() - 0.5)))
    }

    /// The innermost box under `p` with more content along `axis` than it shows, not covered by
    /// a menu or dialog: what the wheel would scroll.
    fn scroller_under(&self, p: Point, axis: usize) -> Option<u32> {
        let in_reach = &self.visible_boxes.scrollers[self.visible_boxes.covered.min(self.visible_boxes.scrollers.len())..];
        in_reach.iter().rev().find(|s| s.visible.contains(p) && self.tree.overflows(s.at, axis)).map(|s| s.at)
    }

    /// A second finger landed: the first one's action comes to nothing (its press clicks
    /// nothing), and the two zoom a zooming box under their centroid, or else scroll the scroll
    /// box there.
    fn second_finger(&mut self) -> bool {
        if self.touch.doing == Doing::Mouse {
            self.unpress();
        }
        let (middle, apart) = self.touch.middle();
        let zooms = self.touch.zooms.iter().filter(|(_, r)| r.contains(middle));
        let zooming = zooms.min_by(|a, b| (a.1.w * a.1.h).total_cmp(&(b.1.w * b.1.h))).map(|(id, _)| *id);
        self.touch.doing = match (zooming, self.scroller_under(middle, 0).or(self.scroller_under(middle, 1))) {
            (Some(id), _) => Doing::Pinching { id, apart, middle },
            (None, Some(at)) => self.scrolling(at, [true; 2], middle),
            (None, None) => Doing::Nothing,
        };
        self.touch.lately.clear();
        // (Nothing is hovered under two fingers.)
        self.event(Event::Leave);
        true
    }

    /// The fingers start scrolling box `at`, along those of `ways` it scrolls, from their
    /// current centroid.
    fn scrolling(&self, at: u32, ways: [bool; 2], start: Point) -> Doing {
        let spec = &self.tree.specs[at as usize];
        let axes = [ways[0] && self.tree.overflows(at, 0), ways[1] && self.tree.overflows(at, 1)];
        Doing::Scrolling { id: spec.id, from: self.scrolls.get(&spec.id).copied().unwrap_or(spec.offset), start, axes }
    }

    /// A finger's press comes to nothing: no click, and anything it was dragging is dropped on
    /// nothing.
    fn unpress(&mut self) {
        let input = &mut self.input;
        (input.pointer, input.down[0], input.pressed[0], self.active) = (None, false, false, 0);
    }

    /// A finger moved.
    fn finger_moved(&mut self, finger: u32, at: Point) -> bool {
        let Some(moved) = self.touch.fingers.iter_mut().find(|(number, ..)| *number == finger) else { return false };
        moved.2 = at;
        let (now, (middle, _)) = (self.now, self.touch.middle());
        self.touch.lately.retain(|(when, _)| now.saturating_sub(*when) <= LATELY);
        self.touch.lately.push((now, middle));
        match self.touch.doing {
            Doing::Waiting(_) => self.set_off(at),
            Doing::Mouse => self.event(Event::Move(at)),
            Doing::Scrolling { id, from, start, axes } => self.scroll_with(id, from, start, axes),
            Doing::Pinching { id, apart, middle } => self.pinch(id, apart, middle),
            Doing::Nothing => false,
        }
    }

    /// A waiting finger is now at `at`. Once past tap distance, it scrolls the box under where
    /// it landed if that box scrolls in its direction (along one axis only, where it went
    /// mostly one way); else it is the mouse, pressed where it landed and moved to where it is.
    fn set_off(&mut self, at: Point) -> bool {
        let (_, landed, _) = self.touch.fingers[0];
        let (x, y) = (at.x - landed.x, at.y - landed.y);
        if x.hypot(y) < SLOP {
            return false;
        }
        let (ways, main) = ([y.abs() <= 2.0 * x.abs(), x.abs() <= 2.0 * y.abs()], (y.abs() > x.abs()) as usize);
        let pressed = self.input.pointer.unwrap_or(landed);
        let under = self.scroller_under(pressed, main).or_else(|| self.scroller_under(pressed, 1 - main).filter(|_| ways[1 - main]));
        let Some(box_at) = under else {
            self.press_by_finger();
            return self.event(Event::Move(at));
        };
        self.touch.doing = self.scrolling(box_at, ways, at);
        self.event(Event::Leave);
        true
    }

    /// The fingers scroll box `id` to where they have taken it, clamped to its range. (Where it
    /// stops at an end, it moves back as soon as the fingers do.)
    fn scroll_with(&mut self, id: Id, mut from: [f32; 2], mut start: Point, axes: [bool; 2]) -> bool {
        let Some(at) = self.visible_boxes.scrollers.iter().map(|s| s.at).find(|at| self.tree.specs[*at as usize].id == id) else { return false };
        let (middle, _) = self.touch.middle();
        let (now, mut to) = ([middle.x, middle.y], self.scrolls.get(&id).copied().unwrap_or(from));
        for axis in (0..2).filter(|axis| axes[*axis]) {
            let wanted = from[axis] - (now[axis] - [start.x, start.y][axis]);
            to[axis] = wanted.clamp(0.0, self.tree.scroll_end(at, axis));
            if to[axis] != wanted {
                from[axis] = to[axis];
                start = if axis == 0 { pt(middle.x, start.y) } else { pt(start.x, middle.y) };
            }
        }
        self.touch.doing = Doing::Scrolling { id, from, start, axes };
        self.scrolls.insert(id, to);
        true
    }

    /// Two fingers spread, pinched, or moved together: box `id` zooms by as much about their
    /// centroid, and pans with them.
    fn pinch(&mut self, id: Id, apart: f32, was: Point) -> bool {
        let (middle, now_apart) = self.touch.middle();
        self.touch.doing = Doing::Pinching { id, apart: now_apart, middle };
        let by = if apart > 0.0 && now_apart > 0.0 { now_apart / apart } else { 1.0 };
        self.zoom_by(id, Zoom { factor: by, center: middle, pan: pt(middle.x - was.x, middle.y - was.y) })
    }

    /// Box `id` zooms like this, on top of what was already pending.
    fn zoom_by(&mut self, id: Id, zoom: Zoom) -> bool {
        let Zoom { factor: by, pan: moved, .. } = zoom;
        self.touch.zoom = Some(match self.touch.zoom {
            Some((was, before)) if was == id => (id, Zoom { factor: before.factor * by, pan: pt(before.pan.x + moved.x, before.pan.y + moved.y), ..zoom }),
            _ => (id, zoom),
        });
        true
    }

    /// A finger lifted (`up`), or the system cancelled it.
    fn finger_up(&mut self, finger: u32, up: bool) -> bool {
        let Some(at) = self.touch.fingers.iter().position(|(number, ..)| *number == finger) else { return false };
        _ = self.touch.fingers.remove(at);
        if !self.touch.fingers.is_empty() {
            return self.fewer_fingers();
        }
        self.touch.lifted = true;
        let held = self.now.saturating_sub(self.touch.since) >= HOLD;
        match std::mem::take(&mut self.touch.doing) {
            Doing::Waiting(false) if up && held => self.finger_held(),
            Doing::Waiting(false) if up => {
                self.press_by_finger();
                self.touch.release = Some(Button::Left);
            }
            Doing::Mouse if up => _ = self.event(Event::Release(Button::Left)),
            Doing::Mouse => self.unpress(),
            Doing::Scrolling { id, axes, .. } if up => self.flick(id, axes),
            _ => {}
        }
        true
    }

    /// One of several fingers lifted: the rest keep scrolling from where they are; two
    /// remaining keep zooming; one left after zooming does nothing more.
    fn fewer_fingers(&mut self) -> bool {
        let (middle, apart) = self.touch.middle();
        self.touch.lately.clear();
        self.touch.doing = match self.touch.doing {
            Doing::Scrolling { id, axes, .. } => Doing::Scrolling { id, from: self.scrolls.get(&id).copied().unwrap_or_default(), start: middle, axes },
            Doing::Pinching { id, .. } if self.touch.fingers.len() > 1 => Doing::Pinching { id, apart, middle },
            _ => Doing::Nothing,
        };
        false
    }

    /// A finger held still long enough: a context menu press where it is, released once seen.
    fn finger_held(&mut self) {
        self.touch.pressing = true;
        self.event(Event::Press(Button::Right, Mods::NONE));
        self.touch.release = Some(Button::Right);
    }

    /// The fingers scrolling box `id` along `axes` lifted: if still moving, the box coasts on
    /// at that speed.
    fn flick(&mut self, id: Id, axes: [bool; 2]) {
        let (Some(&(first, from)), Some(&(last, to))) = (self.touch.lately.first(), self.touch.lately.last()) else { return };
        let took = last.saturating_sub(first).as_secs_f32();
        if took < 0.005 || self.now.saturating_sub(last) > LATELY {
            return;
        }
        let speed = [(from.x - to.x) / took, (from.y - to.y) / took].map(|speed| speed.clamp(-FASTEST, FASTEST));
        let speed = [0, 1].map(|axis| if axes[axis] { speed[axis] } else { 0.0 });
        if speed[0].hypot(speed[1]) >= FLICK {
            self.touch.coast = Some(Coast { id, from: self.scrolls.get(&id).copied().unwrap_or_default(), speed, since: self.now });
        }
    }

    /// Before a run, what time has brought: a finger held long enough becomes a context menu
    /// press, and a coasting box advances. (A finger resting on a part shows no tooltip.)
    pub(crate) fn touch_time(&mut self) {
        if !cfg!(feature = "touch") {
            return;
        }
        if !self.touch.fingers.is_empty() {
            self.rested = None;
        }
        if matches!(self.touch.doing, Doing::Waiting(false)) && self.now.saturating_sub(self.touch.since) >= HOLD {
            self.touch.doing = Doing::Nothing;
            self.finger_held();
        }
        let Some(coast) = self.touch.coast else { return };
        let ((to, going), id) = (coast.at(self.now), coast.id);
        let Some(at) = self.visible_boxes.scrollers.iter().map(|s| s.at).find(|at| self.tree.specs[*at as usize].id == id) else {
            self.touch.coast = None;
            return;
        };
        let ends = [0, 1].map(|axis| self.tree.scroll_end(at, axis));
        let to = [0, 1].map(|axis| to[axis].clamp(0.0, ends[axis]));
        // (It stops at the end it moves toward.)
        let open = [0, 1].map(|axis| coast.speed[axis] < 0.0 && to[axis] > 0.0 || coast.speed[axis] > 0.0 && to[axis] < ends[axis]);
        if !going || !open[0] && !open[1] {
            self.touch.coast = None;
        }
        self.scrolls.insert(id, to);
    }

    /// After each pass: a finger's press is released once seen; once that is seen too, the
    /// pointer leaves with the fingers. And the interface wakes for what time will bring. (A
    /// zoom no box took is dropped.)
    pub(crate) fn touch_settled(&mut self) {
        self.touch.zoom = None;
        if !cfg!(feature = "touch") {
            return;
        }
        if let Some(button) = self.touch.release.take() {
            (self.input.down[button as usize], self.input.released[button as usize], self.again) = (false, true, true);
            return;
        }
        if self.touch.fingers.is_empty() && std::mem::take(&mut self.touch.lifted) {
            (self.input.pointer, self.rested) = (None, None);
        }
        if matches!(self.touch.doing, Doing::Waiting(false)) {
            self.wake_after((self.touch.since + HOLD).saturating_sub(self.now));
        }
        if self.touch.coast.is_some() {
            self.wake_after(FRAME);
        }
    }

    /// Ctrl+wheel, or a touchpad pinch, by `by`: the zooming box under the pointer zooms by as
    /// much about the pointer.
    pub(crate) fn zoomed(&mut self, by: f32) -> bool {
        let Some(at) = self.input.pointer else { return false };
        let zooms = self.touch.zooms.iter().filter(|(_, r)| r.contains(at));
        let Some(id) = zooms.min_by(|a, b| (a.1.w * a.1.h).total_cmp(&(b.1.w * b.1.h))).map(|(id, _)| *id) else { return false };
        self.zoom_by(id, Zoom { factor: by, center: at, pan: Point::default() })
    }

    /// The pen came into proximity or its state changed; or it left (`None`).
    pub(crate) fn stylus_near(&mut self, stylus: Option<Stylus>) -> bool {
        let news = std::mem::replace(&mut self.touch.stylus, stylus) != stylus;
        news && self.active != 0
    }

    /// Box `of` zooms: what was done to it since the last run, taken. (A pan-and-zoom area,
    /// which takes it before describing its content, needs no extra run for it.)
    pub(crate) fn zoom_taken(&mut self, of: &Response) -> Option<Zoom> {
        self.touch.zooms.push((of.id, of.rect));
        let (_, zoom) = self.touch.zoom.filter(|(id, _)| *id == of.id)?;
        self.touch.zoom = None;
        Some(zoom)
    }

    // ---- for the app ----

    /// Makes box `of` (a pointer-sensing one) zoomable: two fingers on it pinch and pan it, as
    /// do a touchpad pinch and Ctrl+wheel over it. Returns what they did since the last run;
    /// none when nothing. Call it every run while the box zooms.
    ///
    /// ```ignore
    /// let map = ui.el().grow().sense().show();
    /// if let Some(zoom) = ui.zoom(&map) {
    ///     app.scale *= zoom.factor; // and move what is drawn so that `zoom.center` stays put
    /// }
    /// ```
    pub fn zoom(&mut self, of: &Response) -> Option<Zoom> {
        // (What was described before this call may need describing again.)
        let zoom = self.zoom_taken(of)?;
        self.again = true;
        Some(zoom)
    }
    /// The pen's state while one is in proximity: pressure, tilt, buttons. None for mouse and
    /// fingers. (The pen is also the mouse: its position and whether it presses are queried as
    /// for the mouse.)
    pub fn stylus(&self) -> Option<Stylus> {
        self.touch.stylus
    }
    /// Whether the last press was a finger's: for an app giving touch users more room, taller
    /// rows and easier-to-hit buttons.
    pub fn is_touch(&self) -> bool {
        self.touch.finger
    }
}

// (Finger behaviour with the feature on is tested through a window, in `tests/touch.rs`.)
#[cfg(test)]
#[cfg(not(feature = "touch"))]
mod tests {
    use super::*;
    use crate::geom::Size;

    /// What the test interface showed and heard: a button and a zooming box.
    #[derive(Default)]
    struct Seen {
        button: Rect,
        map: Rect,
        clicks: u32,
        zooms: Vec<Zoom>,
    }

    fn describe(seen: &mut Seen, ui: &mut Ui) {
        let pressed = ui.button("Press");
        let shown = ui.el().size(120.0, 50.0).sense().show();
        (seen.button, seen.map, seen.clicks) = (pressed.rect, shown.rect, seen.clicks + pressed.clicked as u32);
        seen.zooms.extend(ui.zoom(&shown));
    }

    /// Without `touch`, a finger means nothing to the interface: it arrives as the system's
    /// mouse emulation, if at all. So a finger set down on a button and lifted requests no run
    /// and presses nothing. Ctrl+wheel still zooms the zooming box under the pointer.
    #[test]
    fn without_the_feature_a_finger_is_nothing_and_ctrl_and_the_wheel_still_zoom() {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(Size { w: 200.0, h: 100.0 }, 1.0);
        let mut seen = Seen::default();
        ui.run(&mut |ui| describe(&mut seen, ui));
        // (A box's position is known a run after it first shows.)
        ui.run(&mut |ui| describe(&mut seen, ui));
        let (button, map) = (seen.button, seen.map);
        let on = pt(button.x + button.w / 2.0, button.y + button.h / 2.0);
        for phase in [TouchPhase::Down, TouchPhase::Up] {
            assert!(!ui.event(Event::Touch(Touch { id: 1, position: on, phase })));
            ui.run(&mut |ui| describe(&mut seen, ui));
        }
        assert_eq!((seen.clicks, ui.is_touch(), ui.stylus()), (0, false, None));
        let over = pt(map.x + map.w / 2.0, map.y + map.h / 2.0);
        ui.event(Event::Move(over));
        assert!(ui.event(Event::Zoom(1.25)));
        ui.run(&mut |ui| describe(&mut seen, ui));
        assert_eq!(seen.zooms, vec![Zoom { factor: 1.25, center: over, pan: Point::default() }]);
    }
}
