//! Boards: a box with its own space that the user pans and zooms: a map, an image, a plan, a
//! node graph. The view is a value the app keeps ([`Board`]), saved as text like other kept
//! state:
//!
//! ```ignore
//! // in the app's state:  map: Board::default()
//! ui.board(&mut app.map, "Map", 0.25..=8.0);
//! for city in &app.cities {
//!     // Only what is in view is described. All of it is counted, for the bars and for
//!     // showing everything at once.
//!     if ui.in_sight(city.rect) {
//!         ui.el().at(city.rect.x, city.rect.y).size(city.rect.w, city.rect.h).fill(t.panel).text(&city.name, t.font, t.text).show();
//!     }
//! }
//! ui.close();
//! ```
//!
//! Content is described in board units, and scaled by the zoom after the window is described: a
//! button on a board is a real button at any size, and text is set at its shown size, never
//! stretched. Shapes and images are drawn at that size too. Panning moves the content as
//! scrolling does: nothing is laid out again unless something enters or leaves the view.
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Insets, Point, Rect, Size, pt, rect};
use crate::id::{ById, Id, fold, hash};
use crate::layout::{Align, BAR, Content, Flow, Length, NONE, Spec};
use crate::pen::Inked;
use crate::told::Role;
use crate::ui::{Measure, Response, Ui};
use std::ops::RangeInclusive;

/// Tells a board's inner box and bars from anything else in it.
pub(crate) const INNER: u64 = 0xb0a2_d1a2_b0a2_d1a2;
const BARS: [u64; 2] = [0xb0a2_dba2_0000_0001, 0xb0a2_dba2_0000_0002];
/// How far an arrow key pans, and how much a zoom key zooms, per press.
const STEP: f32 = 40.0;
const NEARER: f32 = 1.25;
/// The margin around everything when all is shown at once.
const MARGIN: f32 = 24.0;
/// The smallest text drawn on a board, in window units.
const LEGIBLE: f32 = 4.0;

/// A board's view: what an app keeps of one. As text, three numbers (`to_string()`, and back
/// with `parse()`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Board {
    /// The board point at the view's top left: x.
    pub x: f32,
    /// And y.
    pub y: f32,
    /// Window units per board unit: 2 is twice as near.
    pub zoom: f32,
}

/// The board's origin at the top left, at zoom 1.
impl Default for Board {
    fn default() -> Self {
        Board { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

impl Board {
    /// Where a board point shows in the window, the board at `at` (the `rect` of what
    /// [`Ui::board`] returned).
    pub fn spot(&self, point: Point, at: Rect) -> Point {
        pt(at.x + (point.x - self.x) * self.zoom, at.y + (point.y - self.y) * self.zoom)
    }
    /// The board point at `place` in the window, the board at `at`.
    pub fn point(&self, place: Point, at: Rect) -> Point {
        pt(self.x + (place.x - at.x) / self.zoom, self.y + (place.y - at.y) / self.zoom)
    }
    /// The part of the board in view in a room of `size` window units.
    pub fn sight(&self, size: Size) -> Rect {
        rect(self.x, self.y, size.w / self.zoom, size.h / self.zoom)
    }
    /// Zooms by `by` (out, below 1), clamped to `zoom`, keeping the board point at `at` (from
    /// its top left, in window units) in place.
    pub fn zoom_at(&mut self, by: f32, at: Point, zoom: RangeInclusive<f32>) {
        let was = self.zoom;
        self.zoom = (self.zoom * by).clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (self.x + at.x / was - at.x / self.zoom, self.y + at.y / was - at.y / self.zoom);
    }
    /// Shows all of `all` (board units) centred in a room of `size`, as near as leaves a small
    /// margin, clamped to `zoom`. Nothing changes for an empty `all`.
    pub fn fit(&mut self, all: Rect, size: Size, zoom: RangeInclusive<f32>) {
        if all.w <= 0.0 && all.h <= 0.0 {
            return;
        }
        let near = ((size.w - 2.0 * MARGIN) / all.w.max(1e-6)).min((size.h - 2.0 * MARGIN) / all.h.max(1e-6));
        self.zoom = near.clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (all.x + all.w / 2.0 - size.w / 2.0 / self.zoom, all.y + all.h / 2.0 - size.h / 2.0 / self.zoom);
    }
    /// Repairs what no board can be: a zoom that is NaN, zero or less, or outside `zoom`; a NaN
    /// position.
    fn mend(&mut self, zoom: &RangeInclusive<f32>) {
        let zoom_ok = self.zoom.is_finite() && self.zoom > 0.0;
        self.zoom = if zoom_ok { self.zoom } else { 1.0 }.clamp(*zoom.start(), *zoom.end());
        (self.x, self.y) = (if self.x.is_finite() { self.x } else { 0.0 }, if self.y.is_finite() { self.y } else { 0.0 });
    }
}

/// This run's boards, and what is kept of each between runs.
#[derive(Default)]
pub(crate) struct Boards {
    /// Scales board content after the window is described. Set by the first board, so a program
    /// with none does not carry the code.
    pub settle: Option<fn(&mut Ui)>,
    /// This run's boards, in the order opened.
    pub run: Vec<OnRun>,
    /// What is kept of each board, by id.
    kept: ById<Kept>,
    /// The next board is to show all its content.
    fit: bool,
    /// For the board being settled: which boxes lie on it. (Kept only to reuse its memory.)
    inside: Vec<bool>,
    /// What node graphs on boards keep (see `nodes`).
    pub graphs: crate::nodes::Graphs,
}

/// A board of this run.
pub(crate) struct OnRun {
    id: Id,
    /// Its inner box: holds the content, and moves as the board pans.
    pub inner: u32,
    /// Its zoom relative to the box it lies in; and relative to the window, with the boards it
    /// lies on.
    zoom: f32,
    pub scale: f32,
    /// The part in view, in board units; then the bounds of all its content.
    pub sight: Rect,
    bounds: Option<Rect>,
    /// Its bars, as described.
    bars: [Option<[f32; 2]>; 2],
}

/// What is kept of a board between runs.
#[derive(Clone, Copy, Default)]
struct Kept {
    /// Bounds of its content last time.
    bounds: Option<Rect>,
    /// Being dragged: by which button, from where, and the view then.
    dragged: Option<(usize, Point, [f32; 2])>,
    /// A bar is held: which, the grab offset into its thumb, and the content's start and length
    /// at the press.
    held: Option<(usize, f32, [f32; 2])>,
    /// Focus was on something on it.
    focus_inside: bool,
}

/// The bars' thumbs along each axis, as fractions of the room: start and length. `None` where
/// all is in view that way.
fn bars(bounds: Option<Rect>, sight: Rect) -> [Option<[f32; 2]>; 2] {
    let all = bounds.map_or(sight, |bounds| around(bounds, sight));
    let along = |from: f32, long: f32, start: f32, whole: f32| (whole > long + 0.5).then(|| [(from - start) / whole, long / whole]);
    [along(sight.x, sight.w, all.x, all.w), along(sight.y, sight.h, all.y, all.h)]
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

/// Boards: the app's own content that the user pans and zooms.
impl Ui {
    /// Opens a board: a box filling its room, with its own space. Pans by dragging its ground
    /// (or anything on it with the middle button or with Space held), the wheel and its bars;
    /// zooms with Ctrl+wheel, pinch, Ctrl+plus/minus, Ctrl+0 (actual size), Shift+1 (all);
    /// arrows pan while it has focus. Zoom stays within `zoom`. All of this is written into
    /// `board`; `name` is its screen reader name. Describe its content
    /// ([`in_sight`](Self::in_sight)) in board units, then [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn board(&mut self, board: &mut Board, name: &str, zoom: RangeInclusive<f32>) -> Response {
        self.open_board(board, name, zoom, true, true)
    }
    /// A board where a plain drag belongs to the app (to select with a rectangle, to draw).
    /// Pans by middle button drag, or drag with Space held.
    pub fn work_board(&mut self, board: &mut Board, name: &str, zoom: RangeInclusive<f32>) -> Response {
        self.open_board(board, name, zoom, false, true)
    }
    /// Registers a thing at `at` (board units) on the board being described; returns whether
    /// any of it is in view, and so should be described. Register everything, in view or not:
    /// the bars and "show all" use it.
    pub fn in_sight(&mut self, at: Rect) -> bool {
        let Some(board) = self.board_here() else { return true };
        let on = &mut self.boards.run[board];
        on.bounds = Some(on.bounds.map_or(at, |bounds| around(bounds, at)));
        let sight = on.sight;
        at.x <= sight.right() && at.right() >= sight.x && at.y <= sight.bottom() && at.bottom() >= sight.y
    }
    /// Makes the next board show all its content, as Shift+1 does.
    pub fn fit_board(&mut self) {
        (self.boards.fit, self.again) = (true, true);
    }

    /// The board being described now: its index among this run's boards.
    fn board_here(&self) -> Option<usize> {
        let layer = self.open.last()?.layer;
        self.boards.run.iter().rposition(|board| self.open.iter().any(|open| open.at == board.inner && open.layer == layer))
    }
    /// The zoom of what is being described now, relative to the window: 1, unless on a board.
    pub(crate) fn zoom_here(&self) -> f32 {
        self.board_here().map_or(1.0, |board| self.boards.run[board].scale)
    }

    /// A board, panned by plain drags of its ground or not; and whether closing its content
    /// closes the board too (a graph adds more over it first).
    #[cfg_attr(debug_assertions, track_caller)]
    pub(crate) fn open_board(&mut self, board: &mut Board, name: &str, zoom: RangeInclusive<f32>, plain: bool, join: bool) -> Response {
        self.boards.settle = Some(settle);
        let zoom = zoom.start().max(1e-6)..=zoom.end().max(zoom.start().max(1e-6));
        board.mend(&zoom);
        let (id, outer) = (self.next_id(), self.zoom_here());
        let ground = self.interact(id);
        let inner_id = part_id(id, INNER);
        // Its room, in window units and in units of what it lies on.
        let room = self.rooms.get(&inner_id).map_or([self.size().w, self.size().h], |room| [room[0], room[1]]);
        let size = Size { w: room[0] / outer, h: room[1] / outer };
        let mut kept = self.boards.kept.get(&id).copied().unwrap_or_default();
        self.steer(board, &ground, &mut kept, size, outer, plain, &zoom);
        let shown = board.zoom * outer;
        let sight = board.sight(size);
        let bars = bars(kept.bounds, sight);
        // The board, with gutters for its bars along the right and bottom when it has them:
        // content never lies under them.
        let pad = Insets { right: if bars[1].is_some() { BAR } else { 0.0 }, bottom: if bars[0].is_some() { BAR } else { 0.0 }, ..Insets::ZERO };
        self.el().flow(Flow::Layers).grow().pad(pad).clip().sense().focusable().open();
        self.tell(&ground, Role::Group, None, name);
        for (axis, along) in bars.into_iter().enumerate() {
            self.bar(board, &mut kept, id, axis, along, size, sight);
        }
        // The content, offset by the view.
        self.join_next = join;
        let inner = self.tree.specs.len() as u32;
        self.el().key(INNER).flow(Flow::Layers).grow().shifted(board.x * shown, board.y * shown).open();
        self.measures.push(Measure { at: inner, within: inner, id: inner_id, read: [room[0], room[1], 0.0] });
        // The zoom is part of its layout.
        self.layout_key = fold(self.layout_key, board.zoom.to_bits() as u64);
        self.boards.run.push(OnRun { id, inner, zoom: board.zoom, scale: shown, sight, bounds: None, bars });
        self.boards.kept.insert(id, kept);
        ground
    }

    /// Input to a board: wheel, pinch, drag, keys. `size` is its room in units of what it lies
    /// on, which shows it `outer` times as near as the window.
    #[allow(clippy::too_many_arguments)]
    fn steer(&mut self, board: &mut Board, ground: &Response, kept: &mut Kept, size: Size, outer: f32, plain: bool, zoom: &RangeInclusive<f32>) {
        // Two fingers, a touchpad pinch or Ctrl+wheel (see `touch`): the board follows the
        // fingers, and zooms about them or the pointer (from its corner, in units of what it
        // lies on).
        if let Some(asked) = self.zoom_taken(ground) {
            let shown = board.zoom * outer;
            (board.x, board.y) = (board.x - asked.moved.x / shown, board.y - asked.moved.y / shown);
            board.zoom_at(asked.by, pt((asked.at.x - ground.rect.x) / outer, (asked.at.y - ground.rect.y) / outer), zoom.clone());
        }
        let wheel = self.input.wheel;
        if ground.over && wheel != [0.0; 2] && !self.scrolls_under(ground.rect) {
            (board.x, board.y, self.input.wheel) = (board.x + wheel[0] / outer / board.zoom, board.y + wheel[1] / outer / board.zoom, [0.0; 2]);
        }
        self.drag_board(board, ground, kept, outer, plain);
        // Keys: while focus is on the board or something on it, or nowhere with the pointer
        // over the board.
        let keyed = ground.focused || kept.focus_inside || self.focus == 0 && ground.over;
        let middle = pt(size.w / 2.0, size.h / 2.0);
        let both = Mods { shift: true, ..Mods::CTRL };
        let nearer = [Key::Char('='), Key::Char('+')].iter().any(|key| keyed && (self.key(*key, Mods::CTRL) || self.key(*key, both)));
        let further = keyed && self.key(Key::Char('-'), Mods::CTRL);
        if nearer || further {
            board.zoom_at(if nearer { NEARER } else { 1.0 / NEARER }, middle, zoom.clone());
        }
        if keyed && self.key(Key::Char('0'), Mods::CTRL) {
            board.zoom_at(1.0 / board.zoom, middle, zoom.clone());
        }
        if std::mem::take(&mut self.boards.fit) | (keyed && self.key(Key::Char('1'), Mods::SHIFT)) {
            board.fit(kept.bounds.unwrap_or_default(), size, zoom.clone());
        }
        // Arrows pan it while the board itself has focus.
        let arrows = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| ground.focused && self.key(key, Mods::NONE));
        let step = STEP / board.zoom;
        board.x += step * (arrows[1] as i32 - arrows[0] as i32) as f32;
        board.y += step * (arrows[3] as i32 - arrows[2] as i32) as f32;
    }

    /// Whether the pointer is over something scrollable on the board at `r`: that takes the
    /// wheel, not the board.
    fn scrolls_under(&self, r: Rect) -> bool {
        let Some(p) = self.input.pointer else { return false };
        let inside = |s: &Rect| s.x >= r.x && s.y >= r.y && s.right() <= r.right() && s.bottom() <= r.bottom() && (s.w < r.w || s.h < r.h);
        self.survey.scrollers.iter().any(|scroller| scroller.seen.contains(p) && inside(&scroller.rect))
    }

    /// Drag-panning: middle button anywhere, left button with Space held (focus on the board or
    /// nowhere), and a plain drag of its ground on a board panned that way.
    fn drag_board(&mut self, board: &mut Board, ground: &Response, kept: &mut Kept, outer: f32, plain: bool) {
        let p = self.input.pointer.unwrap_or_default();
        let spaced = ground.over && (self.focus == 0 || ground.focused) && self.key_held(Key::Space);
        let button = match () {
            _ if !ground.over => None,
            _ if self.input.pressed[2] => Some(2),
            _ if self.input.pressed[0] && (spaced || plain && ground.hovered) => Some(0),
            _ => None,
        };
        if let Some(button) = button {
            kept.dragged = Some((button, p, [board.x, board.y]));
            // A press that pans presses nothing on the board.
            self.input.pressed[0] &= !spaced;
        }
        if spaced {
            self.set_cursor(Cursor::Move);
        }
        let Some((button, from, was)) = kept.dragged else { return };
        if !self.input.down[button] {
            kept.dragged = None;
            return;
        }
        let shown = board.zoom * outer;
        (board.x, board.y) = (was[0] - (p.x - from.x) / shown, was[1] - (p.y - from.y) / shown);
        self.set_cursor(Cursor::Move);
    }

    /// A board's bar along `axis`, if it has one: `along` is its thumb's start and length as
    /// fractions. Dragging the thumb pans; pressing beside it jumps there.
    #[allow(clippy::too_many_arguments)]
    fn bar(&mut self, board: &mut Board, kept: &mut Kept, of: Id, axis: usize, along: Option<[f32; 2]>, size: Size, sight: Rect) {
        let Some([from, long]) = along else {
            kept.held = kept.held.filter(|held| held.0 != axis);
            return;
        };
        let t = self.theme;
        let id = part_id(of, BARS[axis]);
        let strip = self.interact(id);
        let run = [size.w, size.h][axis];
        let (part_long, start) = ((long * run).max(24.0).min(run), [sight.x, sight.y][axis]);
        let part_from = from * (run - part_long) / (1.0 - long).max(1e-6);
        // The content's start and length now.
        let whole = [start - from * [sight.w, sight.h][axis] / long, [sight.w, sight.h][axis] / long];
        let p = self.input.pointer.map_or(0.0, |p| [p.x - strip.rect.x, p.y - strip.rect.y][axis]);
        if strip.pressed && self.input.pressed[0] {
            let caught = if p >= part_from && p < part_from + part_long { p - part_from } else { part_long / 2.0 };
            kept.held = Some((axis, caught, whole));
        }
        if let Some((_, caught, [first, length])) = kept.held.filter(|held| held.0 == axis && strip.pressed) {
            let to = first + (p - caught) / (run - part_long).max(1.0) * (length - [sight.w, sight.h][axis]);
            let to = to.clamp(first, first + length - [sight.w, sight.h][axis]);
            *[&mut board.x, &mut board.y][axis] = to;
        }
        let color = if strip.hovered || strip.pressed { t.text_faint } else { t.line_strong };
        let (place, at) = ([(Align::Start, Align::End), (Align::End, Align::Start)][axis], [[0.0, BAR], [BAR, 0.0]][axis]);
        let strip_box = self.el().key(BARS[axis]).flow(Flow::Layers).place(place).at(at[0], at[1]).sense();
        let strip_box = if axis == 0 { strip_box.w(Length::Grow(1.0)).h(Length::Px(BAR)) } else { strip_box.w(Length::Px(BAR)).h(Length::Grow(1.0)) };
        strip_box.open();
        let (x, y, w, h) = if axis == 0 { (part_from + 2.0, 2.0, part_long - 4.0, 6.0) } else { (2.0, part_from + 2.0, 6.0, part_long - 4.0) };
        self.el().at(x, y).size(w.max(0.0), h.max(0.0)).radius(3.0).fill(color).show();
        self.close();
    }
}

/// After the window is described: scales each board's content by its zoom, and keeps each
/// board's bounds for its bars and "show all". A board that now needs other bars has the window
/// described once more.
fn settle(ui: &mut Ui) {
    let mut run = std::mem::take(&mut ui.boards.run);
    for board in &run {
        let end = ui.tree.specs[board.inner as usize].end;
        zoom_boxes(ui, board.inner, end, board.zoom);
        let focus = ui.focus;
        let inside = focus != 0 && ui.tree.specs[board.inner as usize + 1..end as usize].iter().any(|spec| spec.id == focus);
        let kept = ui.boards.kept.entry(board.id).or_default();
        kept.focus_inside = inside;
        if kept.bounds != board.bounds {
            kept.bounds = board.bounds;
            ui.again |= bars(board.bounds, board.sight) != board.bars;
        }
    }
    run.clear();
    ui.boards.run = run;
}

/// Scales every box in box `inner` (those up to `end`) by `by`: size, padding, margins,
/// position, corners, lines, text and shapes. A layer started while the board was described (a
/// menu opened from its content) lies among them and is left as is: it shows over the board at
/// window scale.
fn zoom_boxes(ui: &mut Ui, inner: u32, end: u32, by: f32) {
    if by == 1.0 {
        return;
    }
    let Ui { tree, shapes, boards, .. } = ui;
    let inside = &mut boards.inside;
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
