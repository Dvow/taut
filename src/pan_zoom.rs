//! Pan and zoom: a box with its own space that the user pans and zooms: a map, an image, a
//! plan, a node graph. The view is a value the app keeps ([`PanZoom`]), saved as text like
//! other kept state:
//!
//! ```ignore
//! // in the app's state:  map: PanZoom::default()
//! ui.pan_zoom(&mut app.map, "Map", 0.25..=8.0);
//! for city in &app.cities {
//!     // Only what is in view is described. All of it is counted, for the bars and for
//!     // showing everything at once.
//!     if ui.in_view(city.rect) {
//!         ui.el().at(city.rect.x, city.rect.y).size(city.rect.w, city.rect.h).fill(t.panel).text(&city.name, t.font, t.text).show();
//!     }
//! }
//! ui.close();
//! ```
//!
//! Content is described in content units, and scaled by the zoom after the window is described:
//! a button in a view is a real button at any size, and text is set at its shown size, never
//! stretched. Shapes and images are drawn at that size too. Panning moves the content as
//! scrolling does: nothing is laid out again unless something enters or leaves the view.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Insets, Point, Rect, Size, pt, rect};
use crate::id::{ById, Id, fold, hash};
use crate::layout::{Align, BAR, Content, Flow, Length, NONE, Spec};
use crate::pen::Inked;
use crate::ui::{Measure, Response, Ui};
use std::ops::RangeInclusive;

/// Tells a view's inner box and bars from anything else in it.
pub(crate) const INNER: u64 = 0xb0a2_d1a2_b0a2_d1a2;
const BARS: [u64; 2] = [0xb0a2_dba2_0000_0001, 0xb0a2_dba2_0000_0002];
/// How far an arrow key pans, and how much a zoom key zooms, per press.
const STEP: f32 = 40.0;
const NEARER: f32 = 1.25;
/// The margin around everything when all is shown at once.
const MARGIN: f32 = 24.0;
/// The smallest text drawn in a view, in window units.
const LEGIBLE: f32 = 4.0;

/// A pan-and-zoom view: what an app keeps of one. As text, three numbers (`to_string()`, and
/// back with `parse()`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanZoom {
    /// The content point at the view's top left: x.
    pub x: f32,
    /// And y.
    pub y: f32,
    /// Window units per content unit: 2 is twice as near.
    pub zoom: f32,
}

/// The content's origin at the top left, at zoom 1.
impl Default for PanZoom {
    fn default() -> Self {
        PanZoom { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

impl PanZoom {
    /// Where a content point shows in the window, the view at `at` (the `rect` of what
    /// [`Ui::pan_zoom`] returned).
    pub fn to_window(&self, point: Point, at: Rect) -> Point {
        pt(at.x + (point.x - self.x) * self.zoom, at.y + (point.y - self.y) * self.zoom)
    }
    /// The content point at `place` in the window, the view at `at`.
    pub fn from_window(&self, place: Point, at: Rect) -> Point {
        pt(self.x + (place.x - at.x) / self.zoom, self.y + (place.y - at.y) / self.zoom)
    }
    /// The part of the content in view in a room of `size` window units.
    pub fn visible_rect(&self, size: Size) -> Rect {
        rect(self.x, self.y, size.w / self.zoom, size.h / self.zoom)
    }
    /// Zooms by `by` (out, below 1), clamped to `zoom`, keeping the content point at `at` (from
    /// its top left, in window units) in place.
    pub fn zoom_at(&mut self, by: f32, at: Point, zoom: RangeInclusive<f32>) {
        let was = self.zoom;
        self.zoom = (self.zoom * by).clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (self.x + at.x / was - at.x / self.zoom, self.y + at.y / was - at.y / self.zoom);
    }
    /// Shows all of `all` (content units) centred in a room of `size`, as near as leaves a
    /// small margin, clamped to `zoom`. Nothing changes for an empty `all`.
    pub fn fit(&mut self, all: Rect, size: Size, zoom: RangeInclusive<f32>) {
        if all.w <= 0.0 && all.h <= 0.0 {
            return;
        }
        let near = ((size.w - 2.0 * MARGIN) / all.w.max(1e-6)).min((size.h - 2.0 * MARGIN) / all.h.max(1e-6));
        self.zoom = near.clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (all.x + all.w / 2.0 - size.w / 2.0 / self.zoom, all.y + all.h / 2.0 - size.h / 2.0 / self.zoom);
    }
    /// Repairs what no view can be: a zoom that is NaN, zero or less, or outside `zoom`; a NaN
    /// position.
    fn repair(&mut self, zoom: &RangeInclusive<f32>) {
        let zoom_ok = self.zoom.is_finite() && self.zoom > 0.0;
        self.zoom = if zoom_ok { self.zoom } else { 1.0 }.clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (if self.x.is_finite() { self.x } else { 0.0 }, if self.y.is_finite() { self.y } else { 0.0 });
    }
}

/// This run's views, and the state of each between runs.
#[derive(Default)]
pub(crate) struct PanZoomViews {
    /// Scales view content after the window is described. Set by the first view, so a program
    /// with none does not carry the code.
    pub settle: Option<fn(&mut Ui)>,
    /// This run's views, in the order opened.
    pub this_run: Vec<OpenView>,
    /// The state of each view between runs, by id.
    state: ById<ViewState>,
    /// The next view is to show all its content.
    fit_next: bool,
    /// For the view being settled: which boxes lie in it. (Kept only to reuse its memory.)
    inside: Vec<bool>,
    /// The state of node graphs in views (see `node_graph`).
    pub graphs: crate::node_graph::Graphs,
}

/// A view opened this run.
pub(crate) struct OpenView {
    id: Id,
    /// Its inner box: holds the content, and moves as the view pans.
    pub inner: u32,
    /// Its zoom relative to the box it lies in; and relative to the window, with the views it
    /// lies in.
    zoom: f32,
    pub scale: f32,
    /// The part in view, in content units; then the bounds of all its content.
    pub visible: Rect,
    bounds: Option<Rect>,
    /// Its bars, as described.
    bars: [Option<[f32; 2]>; 2],
}

/// The state of a view between runs.
#[derive(Clone, Copy, Default)]
struct ViewState {
    /// Bounds of its content last time.
    bounds: Option<Rect>,
    /// Being dragged: by which button, from where, and the view then.
    dragged: Option<(usize, Point, [f32; 2])>,
    /// A bar is held: which, the grab offset into its thumb, and the content's start and length
    /// at the press.
    held: Option<(usize, f32, [f32; 2])>,
    /// Focus was on something in it.
    focus_inside: bool,
}

/// The bars' thumbs along each axis, as fractions of the room: start and length. `None` where
/// all is in view that way.
fn bars(bounds: Option<Rect>, visible: Rect) -> [Option<[f32; 2]>; 2] {
    let all = bounds.map_or(visible, |bounds| around(bounds, visible));
    let along = |from: f32, long: f32, start: f32, whole: f32| (whole > long + 0.5).then(|| [(from - start) / whole, long / whole]);
    [along(visible.x, visible.w, all.x, all.w), along(visible.y, visible.h, all.y, all.h)]
}

/// The rectangle around both, even if one has no width or height (a line, a point).
fn around(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    rect(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
}

/// The id of a box with this key in box `of`.
pub(crate) fn part_id(of: Id, key: u64) -> Id {
    hash((of, hash(key)))
}

/// Pan-and-zoom views: the app's own content that the user pans and zooms.
impl Ui {
    /// Opens a pan-and-zoom view: a box filling its room, with its own space. Pans by dragging
    /// its ground (or anything in it with the middle button or with Space held), the wheel and
    /// its bars; zooms with Ctrl+wheel, pinch, Ctrl+plus/minus, Ctrl+0 (actual size), Shift+1
    /// (all); arrows pan while it has focus. Zoom stays within `zoom`. All of this is written
    /// into `view`; `name` is its screen reader name. Describe its content
    /// ([`in_view`](Self::in_view)) in content units, then [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn pan_zoom(&mut self, view: &mut PanZoom, name: &str, zoom: RangeInclusive<f32>) -> Response {
        self.open_pan_zoom(view, name, zoom, true, true)
    }
    /// A view where a plain drag belongs to the app (to select with a rectangle, to draw).
    /// Pans by middle button drag, or drag with Space held.
    pub fn pan_zoom_editor(&mut self, view: &mut PanZoom, name: &str, zoom: RangeInclusive<f32>) -> Response {
        self.open_pan_zoom(view, name, zoom, false, true)
    }
    /// Registers a thing at `at` (content units) in the view being described; returns whether
    /// any of it is in view, and so should be described. Register everything, in view or not:
    /// the bars and "show all" use it.
    pub fn in_view(&mut self, at: Rect) -> bool {
        let Some(view) = self.view_here() else { return true };
        let open = &mut self.pan_zoom_views.this_run[view];
        open.bounds = Some(open.bounds.map_or(at, |bounds| around(bounds, at)));
        let visible = open.visible;
        at.x <= visible.right() && at.right() >= visible.x && at.y <= visible.bottom() && at.bottom() >= visible.y
    }
    /// Makes the next view show all its content, as Shift+1 does.
    pub fn fit_pan_zoom(&mut self) {
        (self.pan_zoom_views.fit_next, self.again) = (true, true);
    }

    /// The view being described now: its index among this run's views.
    fn view_here(&self) -> Option<usize> {
        let layer = self.open.last()?.layer;
        self.pan_zoom_views.this_run.iter().rposition(|view| self.open.iter().any(|open| open.at == view.inner && open.layer == layer))
    }
    /// The zoom of what is being described now, relative to the window: 1, unless in a view.
    pub(crate) fn zoom_here(&self) -> f32 {
        self.view_here().map_or(1.0, |view| self.pan_zoom_views.this_run[view].scale)
    }

    /// A view, panned by plain drags of its ground or not (`drag_pans`); and whether closing
    /// its content closes the view too (a graph adds more over it first).
    #[cfg_attr(debug_assertions, track_caller)]
    pub(crate) fn open_pan_zoom(&mut self, view: &mut PanZoom, name: &str, zoom: RangeInclusive<f32>, drag_pans: bool, close_together: bool) -> Response {
        self.pan_zoom_views.settle = Some(settle);
        let zoom = zoom.start().max(1e-6)..=zoom.end().max(zoom.start().max(1e-6));
        view.repair(&zoom);
        let (id, outer) = (self.next_id(), self.zoom_here());
        let ground = self.interact(id);
        let inner_id = part_id(id, INNER);
        // Its room, in window units and in units of what it lies in.
        let room = self.rooms.get(&inner_id).map_or([self.size().w, self.size().h], |room| [room[0], room[1]]);
        let size = Size { w: room[0] / outer, h: room[1] / outer };
        let mut state = self.pan_zoom_views.state.get(&id).copied().unwrap_or_default();
        self.pinch_and_wheel(view, &ground, outer, &zoom);
        self.drag_to_pan(view, &ground, &mut state, outer, drag_pans);
        self.view_keys(view, &ground, &state, size, &zoom);
        let shown = view.zoom * outer;
        let visible = view.visible_rect(size);
        let bars = bars(state.bounds, visible);
        // The view, with gutters for its bars along the right and bottom when it has them:
        // content never lies under them.
        let pad = Insets { right: if bars[1].is_some() { BAR } else { 0.0 }, bottom: if bars[0].is_some() { BAR } else { 0.0 }, ..Insets::ZERO };
        self.el().flow(Flow::Stack).grow().pad(pad).clip().sense().focusable().open();
        self.tell(&ground, Role::Group, None, name);
        for (axis, along) in bars.into_iter().enumerate() {
            self.scroll_bar(view, &mut state, id, axis, along, size, visible);
        }
        // The content, offset by the view.
        self.join_next = close_together;
        let inner = self.tree.specs.len() as u32;
        self.el().key(INNER).flow(Flow::Stack).grow().shifted(view.x * shown, view.y * shown).open();
        self.measures.push(Measure { at: inner, within: inner, id: inner_id, read: [room[0], room[1], 0.0] });
        // The zoom is part of its layout.
        self.layout_key = fold(self.layout_key, view.zoom.to_bits() as u64);
        self.pan_zoom_views.this_run.push(OpenView { id, inner, zoom: view.zoom, scale: shown, visible, bounds: None, bars });
        self.pan_zoom_views.state.insert(id, state);
        ground
    }

    /// Two fingers, a touchpad pinch or Ctrl+wheel (see `touch`): the view follows the fingers,
    /// and zooms about them or the pointer (from its corner, in units of what it lies in). A
    /// plain wheel pans. The view shows `outer` times as near as the window.
    fn pinch_and_wheel(&mut self, view: &mut PanZoom, ground: &Response, outer: f32, zoom: &RangeInclusive<f32>) {
        if let Some(asked) = self.zoom_taken(ground) {
            let shown = view.zoom * outer;
            (view.x, view.y) = (view.x - asked.pan.x / shown, view.y - asked.pan.y / shown);
            view.zoom_at(asked.factor, pt((asked.center.x - ground.rect.x) / outer, (asked.center.y - ground.rect.y) / outer), zoom.clone());
        }
        let wheel = self.input.wheel;
        if ground.contains_pointer && wheel != [0.0; 2] && !self.scrolls_under(ground.rect) {
            (view.x, view.y, self.input.wheel) = (view.x + wheel[0] / outer / view.zoom, view.y + wheel[1] / outer / view.zoom, [0.0; 2]);
        }
    }

    /// Keys of a view: zoom keys while focus is on it or something in it, or nowhere with the
    /// pointer over it; arrows while it has focus itself. `size` is its room in units of what
    /// it lies in.
    fn view_keys(&mut self, view: &mut PanZoom, ground: &Response, state: &ViewState, size: Size, zoom: &RangeInclusive<f32>) {
        let keyed = ground.focused || state.focus_inside || self.focus == 0 && ground.contains_pointer;
        let middle = pt(size.w / 2.0, size.h / 2.0);
        let both = Mods { shift: true, ..Mods::CTRL };
        let nearer = [Key::Char('='), Key::Char('+')].iter().any(|key| keyed && (self.key(*key, Mods::CTRL) || self.key(*key, both)));
        let further = keyed && self.key(Key::Char('-'), Mods::CTRL);
        if nearer || further {
            view.zoom_at(if nearer { NEARER } else { 1.0 / NEARER }, middle, zoom.clone());
        }
        if keyed && self.key(Key::Char('0'), Mods::CTRL) {
            view.zoom_at(1.0 / view.zoom, middle, zoom.clone());
        }
        if std::mem::take(&mut self.pan_zoom_views.fit_next) | (keyed && self.key(Key::Char('1'), Mods::SHIFT)) {
            view.fit(state.bounds.unwrap_or_default(), size, zoom.clone());
        }
        let arrows = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| ground.focused && self.key(key, Mods::NONE));
        let step = STEP / view.zoom;
        view.x += step * (arrows[1] as i32 - arrows[0] as i32) as f32;
        view.y += step * (arrows[3] as i32 - arrows[2] as i32) as f32;
    }

    /// Whether the pointer is over something scrollable in the view at `r`: that takes the
    /// wheel, not the view.
    fn scrolls_under(&self, r: Rect) -> bool {
        let Some(p) = self.input.pointer else { return false };
        let inside = |s: &Rect| s.x >= r.x && s.y >= r.y && s.right() <= r.right() && s.bottom() <= r.bottom() && (s.w < r.w || s.h < r.h);
        self.visible_boxes.scrollers.iter().any(|scroller| scroller.visible.contains(p) && inside(&scroller.rect))
    }

    /// Drag-panning: middle button anywhere, left button with Space held (focus on the view or
    /// nowhere), and a plain drag of its ground where `drag_pans`.
    fn drag_to_pan(&mut self, view: &mut PanZoom, ground: &Response, state: &mut ViewState, outer: f32, drag_pans: bool) {
        let p = self.input.pointer.unwrap_or_default();
        let spaced = ground.contains_pointer && (self.focus == 0 || ground.focused) && self.key_held(Key::Space);
        let button = match () {
            _ if !ground.contains_pointer => None,
            _ if self.input.pressed[2] => Some(2),
            _ if self.input.pressed[0] && (spaced || drag_pans && ground.hovered) => Some(0),
            _ => None,
        };
        if let Some(button) = button {
            state.dragged = Some((button, p, [view.x, view.y]));
            // A press that pans presses nothing in the view.
            self.input.pressed[0] &= !spaced;
        }
        if spaced {
            self.set_cursor(Cursor::Move);
        }
        let Some((button, from, was)) = state.dragged else { return };
        if !self.input.down[button] {
            state.dragged = None;
            return;
        }
        let shown = view.zoom * outer;
        (view.x, view.y) = (was[0] - (p.x - from.x) / shown, was[1] - (p.y - from.y) / shown);
        self.set_cursor(Cursor::Move);
    }

    /// A view's scroll bar along `axis`, if it has one: `along` is its thumb's start and length
    /// as fractions. Dragging the thumb pans; pressing beside it jumps there.
    #[allow(clippy::too_many_arguments)]
    fn scroll_bar(&mut self, view: &mut PanZoom, state: &mut ViewState, of: Id, axis: usize, along: Option<[f32; 2]>, size: Size, visible: Rect) {
        let Some([from, long]) = along else {
            state.held = state.held.filter(|held| held.0 != axis);
            return;
        };
        let id = part_id(of, BARS[axis]);
        let strip = self.interact(id);
        let (run, seen) = ([size.w, size.h][axis], [visible.w, visible.h][axis]);
        let (thumb_long, start) = ((long * run).max(24.0).min(run), [visible.x, visible.y][axis]);
        let thumb_from = from * (run - thumb_long) / (1.0 - long).max(1e-6);
        // The content's start and length now.
        let whole = [start - from * seen / long, seen / long];
        let p = self.input.pointer.map_or(0.0, |p| [p.x - strip.rect.x, p.y - strip.rect.y][axis]);
        if strip.pressed && self.input.pressed[0] {
            let caught = if p >= thumb_from && p < thumb_from + thumb_long { p - thumb_from } else { thumb_long / 2.0 };
            state.held = Some((axis, caught, whole));
        }
        if let Some((_, caught, [first, length])) = state.held.filter(|held| held.0 == axis && strip.pressed) {
            let to = first + (p - caught) / (run - thumb_long).max(1.0) * (length - seen);
            *[&mut view.x, &mut view.y][axis] = to.clamp(first, first + length - seen);
        }
        let color = if strip.hovered || strip.pressed { self.theme.text_faint } else { self.theme.line_strong };
        self.draw_scroll_bar(axis, thumb_from, thumb_long, color);
    }

    /// A scroll bar along `axis`, its thumb from `thumb_from`, `thumb_long` long.
    fn draw_scroll_bar(&mut self, axis: usize, thumb_from: f32, thumb_long: f32, color: Color) {
        let (place, at) = ([(Align::Start, Align::End), (Align::End, Align::Start)][axis], [[0.0, BAR], [BAR, 0.0]][axis]);
        let strip = self.el().key(BARS[axis]).flow(Flow::Stack).place(place).offset(at[0], at[1]).sense();
        let strip = if axis == 0 { strip.w(Length::Grow(1.0)).h(Length::Px(BAR)) } else { strip.w(Length::Px(BAR)).h(Length::Grow(1.0)) };
        strip.open();
        let (x, y, w, h) = if axis == 0 { (thumb_from + 2.0, 2.0, thumb_long - 4.0, 6.0) } else { (2.0, thumb_from + 2.0, 6.0, thumb_long - 4.0) };
        self.el().offset(x, y).size(w.max(0.0), h.max(0.0)).radius(3.0).fill(color).show();
        self.close();
    }
}

/// After the window is described: scales each view's content by its zoom, and keeps each
/// view's bounds for its bars and "show all". A view that now needs other bars has the window
/// described once more.
fn settle(ui: &mut Ui) {
    let mut this_run = std::mem::take(&mut ui.pan_zoom_views.this_run);
    for view in &this_run {
        let end = ui.tree.specs[view.inner as usize].end;
        zoom_boxes(ui, view.inner, end, view.zoom);
        let focus = ui.focus;
        let inside = focus != 0 && ui.tree.specs[view.inner as usize + 1..end as usize].iter().any(|spec| spec.id == focus);
        let state = ui.pan_zoom_views.state.entry(view.id).or_default();
        state.focus_inside = inside;
        if state.bounds != view.bounds {
            state.bounds = view.bounds;
            ui.again |= bars(view.bounds, view.visible) != view.bars;
        }
    }
    this_run.clear();
    ui.pan_zoom_views.this_run = this_run;
}

/// Scales every box in box `inner` (those up to `end`) by `by`: size, padding, margins,
/// position, corners, lines, text and shapes. A layer started while the view was described (a
/// menu opened from its content) lies among them and is left as is: it shows over the view at
/// window scale.
fn zoom_boxes(ui: &mut Ui, inner: u32, end: u32, by: f32) {
    if by == 1.0 {
        return;
    }
    let Ui { tree, shapes, pan_zoom_views, .. } = ui;
    let inside = &mut pan_zoom_views.inside;
    inside.clear();
    inside.resize((end - inner) as usize, false);
    inside[0] = true;
    for at in inner + 1..end {
        let spec = &mut tree.specs[at as usize];
        let up = spec.frame.parent;
        let on = up != NONE && up >= inner && inside.get((up - inner) as usize) == Some(&true);
        inside[(at - inner) as usize] = on;
        if on {
            zoom_box(spec, shapes, by);
        }
    }
}

/// Scales one box by `by`.
fn zoom_box(spec: &mut Spec, shapes: &mut [Inked], by: f32) {
    let frame = &mut spec.frame;
    for axis in 0..2 {
        if let Length::Px(size) = &mut frame.size[axis] {
            *size *= by;
        }
        (frame.min[axis], frame.max[axis], frame.nudge[axis]) = (frame.min[axis] * by, frame.max[axis] * by, frame.nudge[axis] * by);
    }
    let pad = frame.pad;
    frame.pad = Insets { left: pad.left * by, top: pad.top * by, right: pad.right * by, bottom: pad.bottom * by };
    (frame.gap, frame.shadow, spec.radius, spec.border_width) = (frame.gap * by, frame.shadow * by, spec.radius * by, spec.border_width * by);
    match &mut spec.content {
        // Text too small to read is not drawn: it takes its room and shows nothing.
        Content::Text { font, color, .. } => {
            font.size *= by;
            *color = if font.size < LEGIBLE { Color::NONE } else { *color };
        }
        Content::Shape { shape } => {
            if let Some(shape) = shapes.get_mut(*shape as usize) {
                shape.scale_by(by);
            }
        }
        _ => {}
    }
}
