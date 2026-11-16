//! Panels: boxes floating over the page that the person moves by the title, resizes by the
//! edges, folds and closes. The app keeps their place and size:
//!
//! ```ignore
//! // in the app's state:  tools: Panel::new(40.0, 60.0, 260.0, 320.0)
//! if ui.panel(&mut app.tools, "Tools") {
//!     ui.label("Anything goes in here.");
//!     ui.close();
//! }
//! ```
//!
//! A panel is a layer of its own, so moving it lays nothing out again; a press anywhere on it
//! brings it in front of the other panels. Moved or resized to within a few units of a window
//! edge or another panel, it snaps to that edge; when the window shrinks, a panel that was
//! inside stays inside.
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::id::Id;
use crate::layout::{Flow, Length};
use crate::told::Role;
use crate::ui::{Anchor, Ui};
use crate::widgets::{Before, CROSS, DOWN, RIGHT};

/// Tells a panel's layer apart from a box of the same name.
const PANEL: Id = 0x9a9e_1f1f_9a9e_1f1f;
/// How far outside a panel's edge the pointer still grabs the edge; and how near a corner it
/// grabs both edges meeting there.
const RING: f32 = 5.0;
const CORNER: f32 = 14.0;
/// The smallest panel size.
pub(crate) const LEAST: [f32; 2] = [140.0, 72.0];
/// How near an edge comes to another before it snaps; and how far a key moves a panel or one of
/// its edges.
const SNAP: f32 = 8.0;
const KEYED: f32 = 10.0;

/// A panel as the app keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    /// Its place and size in the window. A width or height of 0 fits its content (and then the
    /// person cannot resize it).
    pub rect: Rect,
    /// It shows. Its close button makes this false; the app makes it true again.
    pub open: bool,
    /// Only its title shows.
    pub folded: bool,
}

/// An open panel near the window's corner, sized to its content.
impl Default for Panel {
    fn default() -> Self {
        Panel::new(40.0, 40.0, 0.0, 0.0)
    }
}

impl Panel {
    /// An open panel with its corner at (`x`, `y`).
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Panel { rect: rect(x, y, width, height), open: true, folded: false }
    }
}

/// Which edges of `r` a point just outside it is at: left, top, right, bottom. Near a corner,
/// both edges meeting there.
fn edges_at(p: Point, r: Rect) -> [bool; 4] {
    // Beside the box, it is at the top or bottom edge only near that corner; above or below it,
    // likewise for left and right.
    let (beside, over) = (p.x < r.x || p.x >= r.right(), p.y < r.y || p.y >= r.bottom());
    let (near_x, near_y) = (if over { CORNER } else { 0.0 }, if beside { CORNER } else { 0.0 });
    [p.x < r.x + near_x, p.y < r.y + near_y, p.x >= r.right() - near_x, p.y >= r.bottom() - near_y]
}

/// `was` with its held edges moved by `gone`, never smaller than the least panel size.
fn stretched(was: Rect, gone: Point, edges: [bool; 4]) -> Rect {
    let (mut left, mut top, mut right, mut bottom) = (was.x, was.y, was.right(), was.bottom());
    if edges[0] {
        left = (left + gone.x).min(right - LEAST[0]);
    }
    if edges[1] {
        top = (top + gone.y).min(bottom - LEAST[1]);
    }
    if edges[2] {
        right = (right + gone.x).max(left + LEAST[0]);
    }
    if edges[3] {
        bottom = (bottom + gone.y).max(top + LEAST[1]);
    }
    rect(left.round(), top.round(), (right - left).round(), (bottom - top).round())
}

/// A panel's two edges along `axis` (left and right, or top and bottom).
fn span(r: Rect, axis: usize) -> [f32; 2] {
    if axis == 0 { [r.x, r.right()] } else { [r.y, r.bottom()] }
}

/// The edges along `axis` a panel edge snaps to: the window's, and those of other panels
/// overlapping `r` the other way (near enough to touch).
fn edges_near(r: Rect, others: &[Rect], window: Size, axis: usize) -> impl Iterator<Item = f32> {
    let [low, high] = span(r, 1 - axis);
    let beside = move |other: &&Rect| span(**other, 1 - axis)[0] <= high + SNAP && span(**other, 1 - axis)[1] >= low - SNAP;
    let theirs = others.iter().filter(beside).flat_map(move |other| span(*other, axis));
    [0.0, if axis == 0 { window.w } else { window.h }].into_iter().chain(theirs)
}

/// How far `at` moves to snap to the nearest of `edges` within reach; nothing if none is.
fn snap(at: f32, edges: impl Iterator<Item = f32>) -> Option<f32> {
    edges.map(|edge| edge - at).filter(|by| by.abs() <= SNAP).min_by(|a, b| a.abs().total_cmp(&b.abs()))
}

/// A moving panel snapped to the nearest edges within reach: on each axis, whichever of its two
/// edges comes nearer.
fn moved_against(r: Rect, others: &[Rect], window: Size) -> Rect {
    let by = |axis: usize| {
        let ends = span(r, axis).map(|end| snap(end, edges_near(r, others, window, axis)));
        ends.into_iter().flatten().min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(0.0)
    };
    r.moved(by(0), by(1))
}

/// A panel whose `held` edges are being dragged, each snapped to the nearest edge within reach
/// (never below the least size).
fn stretched_against(r: Rect, held: [bool; 4], others: &[Rect], window: Size) -> Rect {
    let (mut sides, mut room) = ([r.x, r.y, r.right(), r.bottom()], r);
    for side in (0..4).filter(|side| held[*side]) {
        let (axis, end) = (side % 2, side / 2);
        let to = sides[side] + snap(sides[side], edges_near(room, others, window, axis)).unwrap_or(0.0);
        let least = LEAST[axis] * if end == 0 { -1.0 } else { 1.0 };
        sides[side] = if end == 0 { to.min(sides[side + 2] + least) } else { to.max(sides[side - 2] + least) };
        room = rect(sides[0], sides[1], sides[2] - sides[0], sides[3] - sides[1]);
    }
    room
}

/// A panel at `r` that was inside a window of size `was`, kept inside it at size `now`: moved
/// back in where the window shrank, and shrunk where the window is now smaller than it (never
/// below the least size). A panel that was not inside is left alone.
fn kept_inside(r: Rect, was: Size, now: Size) -> Rect {
    let along = |at: f32, long: f32, was: f32, now: f32, least: f32| {
        if at < 0.0 || at + long > was + 0.5 {
            return [at, long];
        }
        let long = if long > now { now.max(least.min(long)) } else { long };
        [at.min(now - long).max(0.0).round(), long.round()]
    };
    let ([x, w], [y, h]) = (along(r.x, r.w, was.w, now.w, LEAST[0]), along(r.y, r.h, was.h, now.h, LEAST[1]));
    rect(x, y, w, h)
}

/// Panels floating over the page, moved and resized by the person.
impl Ui {
    /// Starts a panel floating over the page, and says whether its content shows; if so,
    /// describe it, then [`close`](Self::close) it. The person moves it by its title, resizes
    /// it by its edges and corners, folds it with the arrow by the title (or a double click on
    /// the title), and closes it with the cross; all of that goes into `state`. `title` also
    /// tells panels apart. With focus on the arrow or the cross, Alt+Shift+arrow moves the
    /// panel and Control+arrow resizes it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn panel(&mut self, state: &mut Panel, title: &str) -> bool {
        self.panel_as(state, title, true)
    }
    /// The same for a floating dock panel: a double click on its title does not fold it but is
    /// noted for the dock (which puts the panel back where it was docked).
    pub(crate) fn dock_panel(&mut self, state: &mut Panel, title: &str) -> bool {
        self.panel_as(state, title, false)
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn panel_as(&mut self, state: &mut Panel, title: &str, folds: bool) -> bool {
        self.panel_held = false;
        if !state.open {
            return false;
        }
        let (t, id) = (self.theme, self.id(title) ^ PANEL);
        let height = self.bring_forward(id);
        self.layer(id, Anchor::At(pt(state.rect.x - RING, state.rect.y - RING)), false, Color::NONE);
        let layer = self.layers.len() - 1;
        self.layers[layer].height = height;
        // A ring around the panel takes the pointer just outside its edges.
        let sized = state.rect.w > 0.0 && state.rect.h > 0.0 && !state.folded;
        if sized {
            self.stretch_panel(state, id);
        }
        let wide = if state.rect.w > 0.0 { Length::Px(state.rect.w + 2.0 * RING) } else { Length::Fit };
        let high = if sized { Length::Px(state.rect.h + 2.0 * RING) } else { Length::Fit };
        let ring = self.el().flow(Flow::Layers).pad(RING).w(wide).h(high);
        if sized {
            ring.sense().open()
        } else {
            ring.open()
        };
        self.join_next = true;
        let whole = self.column().grow().fill(t.panel).border(t.line_strong, 1.0).radius(8.0).shadow(18.0).clip().sense().open();
        self.tell(&whole, Role::Group, None, title);
        let size = self.keep_inside(state, whole.rect);
        self.panel_title(state, title, id, folds);
        // The title may just have moved it.
        self.layers[layer].anchor = Anchor::At(pt(state.rect.x - RING, state.rect.y - RING));
        let lies = Rect { x: state.rect.x, y: state.rect.y, ..size };
        self.frame.panels[1].push((id, lies));
        if state.folded {
            self.close();
            return false;
        }
        self.separator();
        self.join_next = true;
        self.column().grow().scroll().pad(10.0).gap(t.gap).open();
        true
    }

    /// Where a panel lies (`lies`, as of the last layout) and its size; a content-sized panel
    /// gives no size of its own. If the window shrank since the last pass, a panel that was
    /// inside is kept inside (and the window described once more, to lay it out there).
    fn keep_inside(&mut self, state: &mut Panel, lies: Rect) -> Rect {
        let size = Rect { w: if state.rect.w > 0.0 { state.rect.w } else { lies.w }, h: if state.rect.h > 0.0 { state.rect.h } else { lies.h }, ..state.rect };
        let (was, now) = (self.frame.window, self.size());
        if was == now || was.w <= 0.0 || lies.is_empty() {
            return size;
        }
        let kept = kept_inside(size, was, now);
        let wanted = rect(kept.x, kept.y, if state.rect.w > 0.0 { kept.w } else { 0.0 }, if state.rect.h > 0.0 { kept.h } else { 0.0 });
        (self.again, state.rect) = (self.again || wanted != state.rect, wanted);
        kept
    }

    /// Finds the panel of layer `id` among the panels, back to front: a new one goes in front,
    /// as does one the pointer presses anywhere. Gives back its layer's height.
    fn bring_forward(&mut self, id: Id) -> u64 {
        let known = self.floats.iter().position(|float| *float == id);
        let pressed = self.hot_layer == id && (self.input.pressed[0] || self.input.pressed[1]);
        if let Some(at) = known.filter(|_| pressed) {
            self.floats.remove(at);
        }
        if known.is_none() || pressed {
            self.floats.push(id);
        }
        1 + self.floats.iter().position(|float| *float == id).unwrap_or(0) as u64
    }

    /// Where the panels other than layer `id` lie, as of the last pass.
    fn other_panels(&self, id: Id) -> Vec<Rect> {
        self.frame.panels[0].iter().filter(|(other, _)| *other != id).map(|(_, lies)| *lies).collect()
    }

    /// The ring around a panel takes the pointer: dragged, it moves the edges it caught. (It is
    /// the next box described.)
    fn stretch_panel(&mut self, state: &mut Panel, id: Id) {
        let ring = self.interact(self.next_id());
        let Some(at) = self.pointer().filter(|_| ring.hovered || ring.pressed) else { return };
        let r = state.rect;
        let held = self.held(&ring, [r.x, r.y, r.w, r.h]);
        // The edges caught, for as long as it is held; else the ones it is at now.
        let (from, was) = held.map_or((at, r), |(gone, was)| (pt(at.x - gone.x, at.y - gone.y), rect(was[0], was[1], was[2], was[3])));
        let edges = edges_at(from, was);
        self.set_cursor(match edges {
            [true, true, ..] | [_, _, true, true] => Cursor::ResizeDown,
            [true, _, _, true] | [_, true, true, _] => Cursor::ResizeUp,
            [true, ..] | [_, _, true, _] => Cursor::ResizeX,
            _ => Cursor::ResizeY,
        });
        if let Some((gone, _)) = held {
            state.rect = stretched_against(stretched(was, gone, edges), edges, &self.other_panels(id), self.size());
        }
    }

    /// The bar across a panel's top: a fold arrow, its title, and a close cross. The panel is
    /// moved by this bar and snaps to edges it nears. A double click folds the panel where it
    /// `folds`, else is noted for its dock.
    fn panel_title(&mut self, state: &mut Panel, title: &str, id: Id, folds: bool) {
        let t = self.theme;
        let bar = self.interact(self.next_id());
        if let Some((gone, was)) = self.held(&bar, [state.rect.x, state.rect.y, 0.0, 0.0]) {
            self.panel_held = true;
            // Never so far out of the window that it cannot be grabbed again.
            let (left, right, low) = (40.0 - state.rect.w.max(40.0), self.size().w - 40.0, self.size().h - t.control);
            let at = pt((was[0] + gone.x).min(right).max(left).round(), (was[1] + gone.y).min(low).max(0.0).round());
            let lies = self.frame.panels[0].iter().find(|(panel, _)| *panel == id).map_or(state.rect, |(_, lies)| *lies);
            let against = moved_against(Rect { x: at.x, y: at.y, ..lies }, &self.other_panels(id), self.size());
            (state.rect.x, state.rect.y) = (against.x.round(), against.y.round());
        }
        match folds {
            true => state.folded ^= bar.double_clicked,
            false => self.frame.title_twice |= bar.double_clicked,
        }
        self.row().h(Length::Px(t.control)).pad((2.0, 0.0)).gap(2.0).fill(t.raise).sense().open();
        let fold = self.button_like(Before::Icon(if state.folded { RIGHT } else { DOWN }), "", None, false);
        self.tell(&fold, Role::Fold, Some(!state.folded), title);
        state.folded ^= fold.clicked;
        // In a panel too narrow for it, the title gives way: the buttons stay in the panel.
        self.el().text(title, t.font.bold(), t.text).gives_way(1).show();
        self.spacer();
        let cross = self.button_like(Before::Icon(CROSS), "", None, false);
        let words = self.frame_words.close;
        self.tell(&cross, Role::Button, None, words);
        state.open = !cross.clicked;
        self.close();
        if fold.focused || cross.focused {
            self.panel_keys(state);
        }
    }

    /// With focus on a panel's title: Alt+Shift+arrow moves the panel, Control+arrow moves its
    /// right or bottom edge, a few units at a time. (Kept within reach as dragging keeps it,
    /// never below the least size.)
    fn panel_keys(&mut self, state: &mut Panel) {
        let way = |ui: &mut Ui, mods: Mods| {
            let [right, left, down, up] = [Key::Right, Key::Left, Key::Down, Key::Up].map(|key| ui.key(key, mods) as i8 as f32);
            [(right - left) * KEYED, (down - up) * KEYED]
        };
        let ([dx, dy], [dw, dh]) = (way(self, Mods { shift: true, alt: true, ctrl: false }), way(self, Mods::CTRL));
        let (room, was, low) = (self.size(), state.rect, self.theme.control);
        state.rect.x = (was.x + dx).min(room.w - 40.0).max(40.0 - was.w.max(40.0));
        state.rect.y = (was.y + dy).min(room.h - low).max(0.0);
        state.rect.w = if was.w > 0.0 { (was.w + dw).max(LEAST[0]) } else { 0.0 };
        state.rect.h = if was.h > 0.0 { (was.h + dh).max(LEAST[1]) } else { 0.0 };
        self.again |= state.rect != was;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size { w: 400.0, h: 300.0 };

    #[test]
    fn a_panel_that_is_moved_lies_against_the_nearest_edge_within_reach() {
        let others = [rect(200.0, 50.0, 100.0, 100.0)];
        // Near the window's left edge, and near the other panel's left edge with its right.
        assert_eq!(moved_against(rect(6.0, 60.0, 90.0, 50.0), &others, WINDOW), rect(0.0, 60.0, 90.0, 50.0));
        assert_eq!(moved_against(rect(105.0, 60.0, 90.0, 50.0), &others, WINDOW), rect(110.0, 60.0, 90.0, 50.0));
        // Of two edges in reach, the nearer; and its top in line with the other's.
        assert_eq!(moved_against(rect(303.0, 46.0, 93.0, 50.0), &others, WINDOW), rect(300.0, 50.0, 93.0, 50.0));
        // Not against a panel far away on the other axis; nor out of reach.
        assert_eq!(moved_against(rect(105.0, 200.0, 90.0, 50.0), &others, WINDOW), rect(105.0, 200.0, 90.0, 50.0));
        assert_eq!(moved_against(rect(20.0, 20.0, 90.0, 50.0), &others, WINDOW), rect(20.0, 20.0, 90.0, 50.0));
    }

    #[test]
    fn a_stretched_edge_lies_against_the_nearest_edge_and_the_panel_keeps_its_least_size() {
        let others = [rect(200.0, 50.0, 100.0, 100.0)];
        let right = [false, false, true, false];
        assert_eq!(stretched_against(rect(10.0, 60.0, 185.0, 80.0), right, &others, WINDOW), rect(10.0, 60.0, 190.0, 80.0));
        let bottom = [false, false, false, true];
        assert_eq!(stretched_against(rect(10.0, 60.0, 185.0, 235.0), bottom, &others, WINDOW), rect(10.0, 60.0, 185.0, 240.0));
        // An edge that would snap only by making the panel too small stays.
        let left = [true, false, false, false];
        assert_eq!(stretched_against(rect(195.0, 60.0, 140.0, 80.0), left, &others, WINDOW), rect(195.0, 60.0, 140.0, 80.0));
        assert_eq!(stretched_against(rect(305.0, 60.0, 140.0, 80.0), left, &others, WINDOW), rect(300.0, 60.0, 145.0, 80.0));
    }

    #[test]
    fn a_panel_inside_a_window_that_gets_smaller_is_kept_inside_it() {
        let now = Size { w: 300.0, h: 200.0 };
        // Moved in as far as needed, and no further.
        assert_eq!(kept_inside(rect(250.0, 150.0, 100.0, 60.0), WINDOW, now), rect(200.0, 140.0, 100.0, 60.0));
        assert_eq!(kept_inside(rect(20.0, 20.0, 100.0, 60.0), WINDOW, now), rect(20.0, 20.0, 100.0, 60.0));
        // Wider than the window is now: as wide as it, at its edge; never below the least.
        assert_eq!(kept_inside(rect(10.0, 10.0, 380.0, 60.0), WINDOW, now), rect(0.0, 10.0, 300.0, 60.0));
        assert_eq!(kept_inside(rect(10.0, 10.0, 380.0, 60.0), WINDOW, Size { w: 100.0, h: 200.0 }), rect(0.0, 10.0, 140.0, 60.0));
        // One the person put partly outside the window stays where it is.
        assert_eq!(kept_inside(rect(350.0, 20.0, 100.0, 60.0), WINDOW, now), rect(350.0, 20.0, 100.0, 60.0));
    }
}
