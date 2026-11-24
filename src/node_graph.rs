//! Node graphs: boxes with ports down their sides, joined by wires, in a view that pans and
//! zooms. Computations, materials, audio, machine steps. The app owns the graph ([`Graph`]):
//! its nodes, each carrying app data, and the wires. The library draws it, lets the user
//! select, move, wire, delete and copy, and reports what they did ([`GraphAction`]):
//!
//! ```ignore
//! while let Some(n) = ui.graph(&mut app.graph, "Sums") {
//!     // The node's title and ports are drawn by now: what is described here goes under them.
//!     ui.number(&mut app.graph.nodes[n].data.amount, 1.0);
//!     ui.close();
//! }
//! if app.graph.action != GraphAction::None {
//!     app.work_out();
//! }
//! ```
//!
//! A wire is dragged from port to port: output to input, or back. While dragging, compatible
//! ports stand out and the rest fade; one the app's [`can_connect`](Graph::can_connect) refuses
//! takes no wire. An input takes one wire (a new one replaces the old), or any number where made
//! with [`Node::multi_input`]. A wire pulled off an input and dropped on nothing is deleted; a
//! new one dropped on nothing is reported ([`GraphAction::WireDropped`]), for the app to offer a
//! node there. Nodes are selected by click (Control toggles one, Shift adds one) and by a
//! rectangle dragged on the background; a pressed node under another comes on top (to the end of
//! `nodes`). Selected, they move together, delete with Delete, and copy (Control+C, X, V;
//! Control+D duplicates beside them). Right-click asks the app for its menu
//! ([`GraphAction::Menu`]), with the content point, to place a new node there. Dragging near the
//! view's edge pans it.
//!
//! With a [`grid`](Graph::grid), it is drawn and what the user moves snaps to it. [Frames](Frame)
//! lie under the nodes: dragged by the title, they carry the nodes inside; the corner resizes
//! them; Control+G frames the selection. Double-clicking a wire adds a [bend](Reroute) there;
//! bends are dragged, and double-clicked away. Every edit undoes: Control+Z; Control+Y or
//! Control+Shift+Z redo. F shows the selection (or everything).
//!
//! The keyboard reaches everything. Tab goes through the frames, then from node to node in
//! `nodes` order: a node's title, its ports once (Left and Right cycle them), its wires out once
//! (the same), their bends, and the app's parts in it; the view follows to show it. On a title,
//! arrows move the node (and the rest of the selection), Space selects it, Delete removes it. On
//! a port, Enter starts a wire: arrows then cycle the ports it could connect to, the view
//! showing each, Enter connects and Escape cancels; Delete removes the port's wires. On a wire,
//! Enter goes to its input, B adds a bend, Delete removes it. On a bend or a frame's title,
//! arrows move it (Shift and arrows resize a frame), Delete removes it. Screen readers hear each
//! node as a group named by its title, its title as a selectable row, each port as a button
//! named by its kind and connections, each wire as a button naming both ends.
//!
//! Only the nodes and wires in view are described and laid out, however many there are (plus
//! the focused one and its neighbours, so Tab can continue). Where the graph does not all fit,
//! a minimap of it sits in a corner with the view marked: pressing or dragging on it pans the
//! view there.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Insets, Point, Rect, Size, pt, rect};
use crate::id::{ById, Id, hash};
use crate::layout::{Align, Flow, Length, NONE};
use crate::pan_zoom::{INNER, PanZoom, part_id};
use crate::path::Path;
use crate::ui::{FRAME as TICK, Measure, Response, Rover, Ui};

/// The zoom range.
const ZOOM: std::ops::RangeInclusive<f32> = 0.02..=4.0;
/// A node's title height (with its rule), and each port row's; a port dot's width, and the
/// square the pointer finds it in; the default node width.
const TOP: f32 = 29.0;
const ROW: f32 = 22.0;
const DOT: f32 = 10.0;
const HIT: f32 = 18.0;
const WIDTH: f32 = 160.0;
/// The assumed height of a node's content under its ports until laid out.
const GUESS: f32 = 48.0;
/// The smallest title height, in window units, at which a node is described whole: further out
/// its text would be too small to read and its ports to hit, so it is a plain box (except the
/// focused one).
const FAR: f32 = 8.0;
/// How far an arrow moves selected nodes; how far copies are offset; the margin kept around a
/// node the view pans to.
const NUDGE: f32 = 10.0;
const ASIDE: f32 = 24.0;
const MARGIN: f32 = 24.0;
/// How far a wire runs straight out of its port at least before bending.
const REACH: f32 = 40.0;
/// The minimap's maximum width and corner margin; and its cell size. (Nodes are drawn as the
/// cells they cover, so ten thousand cost the same as a hundred.)
const MAP: f32 = 180.0;
const MAP_EDGE: f32 = 8.0;
const CELL: f32 = 2.0;
/// Key bits telling node parts, and the minimap, apart from anything else.
const TITLE_KEY: u64 = 0x6e0d_e5e0_0000_0001;
const BODY_KEY: u64 = 0x6e0d_e5e0_0000_0002;
const MAP_KEY: u64 = 0x6e0d_e5e0_0000_0003;
const IN_KEY: u64 = 0x6e0d_e5e0_0000_0004;
const OUT_KEY: u64 = 0x6e0d_e5e0_0000_0005;
const ROW_KEY: u64 = 0x6e0d_e5e0_0001_0000;
const SENDS_KEY: u64 = 0x6e0d_e5e0_0000_0006;
const CORNER_KEY: u64 = 0x6e0d_e5e0_0000_0007;
const FRAME_KEY: u64 = 0x6e0d_e5e0_0002_0000;
const BEND_KEY: u64 = 0x6e0d_e5e0_0003_0000;
/// How near the view's edge a drag pans it (window units, at most a quarter of the view), and
/// how fast at the edge (window units a second).
const EDGE: f32 = 32.0;
const SPEED: f32 = 600.0;
/// The finest grid drawn, in window units: finer ones show every second, fourth… line.
const FINEST: f32 = 8.0;
/// How near a wire a double click adds a bend (window units); points sampled per wire piece.
const NEAR: f32 = 6.0;
const SAMPLES: usize = 16;
/// The smallest frame; a bend's dot.
const LEAST: Size = Size { w: 80.0, h: 48.0 };
const BEND: f32 = 8.0;
/// Undo steps kept: at most this many, and at most this many nodes in them together (each step
/// keeps a copy of the graph).
const STEPS: usize = 100;
const KEPT: usize = 100_000;

/// A node graph the user wires up, as the app keeps it: nodes, wires, and its pan and zoom.
#[derive(Clone, Debug)]
pub struct Graph<T> {
    /// The nodes, in screen reader and Tab order. Later ones lie over earlier ones. (Wires name
    /// nodes by index: remove one with `remove`, which renumbers.)
    pub nodes: Vec<Node<T>>,
    /// The wires, each from one node's output into another's input. An input has at most one
    /// (any number where [`multi_input`](Node::multi_input)); an output, any number. Several
    /// into one input are in this order.
    pub wires: Vec<Wire>,
    /// Bends on wires: each wire runs through its own in this order. (Removing a wire removes
    /// its bends.)
    pub reroutes: Vec<Reroute>,
    /// Frames under the nodes, the later over the earlier.
    pub frames: Vec<Frame>,
    /// The grid's spacing in content units, drawn under the nodes: nodes, frames and bends the
    /// user moves snap to it. 0 for none.
    pub grid: f32,
    /// The view's pan and zoom.
    pub view: PanZoom,
    /// The port kinds, in the order they take chart series colours ([`Color::series`]). An
    /// unlisted kind is drawn in the theme's muted text colour.
    pub kinds: Vec<&'static str>,
    /// Whether an output of the first kind may connect to an input of the second: only same
    /// kinds, unless the app says otherwise.
    pub can_connect: fn(&str, &str) -> bool,
    /// What the user did in the run that last described it.
    pub action: GraphAction,
    /// The screen reader words for ports and wires: English unless the app sets its own.
    pub words: GraphWords,
    /// The last copied nodes, with the wires between them (by index among these) and their
    /// bends.
    copied: Vec<Node<T>>,
    copied_wires: Vec<Wire>,
    copied_bends: Vec<Reroute>,
    /// Undo steps, the last last: each the graph as it was before an edit (or, once undone, as
    /// it was after); and how many of the last are undone.
    steps: Vec<Kept<T>>,
    undone: usize,
    /// The last step was a move by key, and the next one adds to it.
    keyed_move: bool,
}

/// What an undo step keeps of a graph: everything the user edits.
#[derive(Clone, Debug)]
struct Kept<T> {
    nodes: Vec<Node<T>>,
    wires: Vec<Wire>,
    reroutes: Vec<Reroute>,
    frames: Vec<Frame>,
}

/// A bend in a wire: the wire runs through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reroute {
    /// The wire.
    pub wire: Wire,
    /// Where, in content units.
    pub at: Point,
}

/// A frame: a titled box under the nodes. Moved by its title, it carries the nodes whose middle
/// lies inside, and the bends.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Its title.
    pub title: String,
    /// Its place and size in content units.
    pub rect: Rect,
}

impl Frame {
    /// A frame with this title around `rect`.
    pub fn new(title: impl Into<String>, rect: Rect) -> Self {
        Frame { title: title.into(), rect }
    }
}

/// One node: a box with a title, inputs down its left, outputs down its right, and the app's
/// content under them.
#[derive(Clone, Debug)]
pub struct Node<T> {
    /// Its title.
    pub title: String,
    /// Its top-left corner in the view.
    pub position: Point,
    /// Its width, in content units.
    pub width: f32,
    /// Its inputs, top to bottom.
    pub inputs: Vec<Port>,
    /// Its outputs, top to bottom.
    pub outputs: Vec<Port>,
    /// It is selected.
    pub selected: bool,
    /// The app's data: what the node is and holds.
    pub data: T,
    /// Its height at the last layout, in content units (0 before); and its layout frame then,
    /// in window units.
    high: f32,
    laid: [f32; 3],
    /// Which of its first 64 inputs take any number of wires, a bit each.
    many: u64,
}

/// A node's input or output.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    /// Its label, written beside it.
    pub name: String,
    /// Its kind: its colour, and what it can connect to ([`Graph::can_connect`]).
    pub kind: &'static str,
}

/// A wire from one node's output into another's input: each end as node index and port index
/// among its outputs or inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Wire {
    /// The node and output it starts from.
    pub from: (usize, usize),
    /// The node and input it goes into.
    pub to: (usize, usize),
}

/// What the user did to a graph in one run. (Selecting, moving and wiring are already applied:
/// this reports them, for the app to react.)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum GraphAction {
    /// Nothing.
    #[default]
    None,
    /// The selection changed. (A pressed node under another also comes to the end of `nodes`,
    /// wires renumbered.)
    Selected,
    /// Nodes, a frame or a bend moved, or a frame was resized.
    Moved,
    /// This wire was made (replacing any a one-wire input had).
    Connected(Wire),
    /// This wire was removed.
    Disconnected(Wire),
    /// The selected nodes were deleted with their wires: later nodes moved up in the list.
    Deleted,
    /// Copies of the last copied nodes were appended and selected.
    Pasted,
    /// A bend was added to this wire or removed from it.
    Rerouted(Wire),
    /// A frame was added or removed.
    Framed,
    /// The last edit was undone, or the last undone one redone: anything may have changed.
    Undone,
    /// See `Undone`.
    Redone,
    /// A new wire from this port was let go on nothing: for the app to offer a node there.
    WireDropped {
        /// The node and port.
        port: (usize, usize),
        /// The port is an output.
        output: bool,
        /// The content point.
        at: Point,
    },
    /// Right-click on a node (selected by then) or the background: for the app's menu, with the
    /// content point.
    Menu {
        /// The node, by index.
        node: Option<usize>,
        /// The content point.
        at: Point,
    },
}

/// The words screen readers hear for a graph's ports and wires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphWords {
    /// Follows an input's kind: "number in".
    pub input: &'static str,
    /// Follows an output's kind: "number out".
    pub output: &'static str,
    /// Before what an input is wired from.
    pub from: &'static str,
    /// Before what an output is wired to.
    pub to: &'static str,
    /// A port wired to nothing.
    pub alone: &'static str,
    /// Announced when a wire is started by keyboard: what the keys do then.
    pub wiring: &'static str,
    /// Announced when a wire is made.
    pub wired: &'static str,
    /// Announced when a wire is removed.
    pub unwired: &'static str,
    /// Announced when a keyboard wire is cancelled.
    pub stopped: &'static str,
    /// Announced before the titles of deleted nodes.
    pub deleted: &'static str,
    /// Announced before the titles of pasted copies.
    pub pasted: &'static str,
    /// Follows the kind of an input that takes several wires.
    pub many: &'static str,
    /// Before a wire's ends.
    pub wire: &'static str,
    /// Before a bend's wire.
    pub bend: &'static str,
    /// Before a frame's title; a new frame's title.
    pub frame: &'static str,
    /// Announced on undo.
    pub undone: &'static str,
    /// Announced on redo.
    pub redone: &'static str,
}

impl Default for GraphWords {
    fn default() -> Self {
        GraphWords {
            input: "in",
            output: "out",
            from: "from",
            to: "to",
            alone: "not wired",
            wiring: "Wiring: the arrows choose where to, Enter wires, Escape stops",
            wired: "Wired",
            unwired: "Unwired",
            stopped: "Not wired",
            deleted: "Deleted",
            pasted: "Pasted",
            many: "several",
            wire: "Wire",
            bend: "Bend",
            frame: "Frame",
            undone: "Undone",
            redone: "Redone",
        }
    }
}

/// Two port kinds fit when equal.
fn same(from: &str, into: &str) -> bool {
    from == into
}

/// An empty graph whose ports fit by equal kind.
impl<T> Default for Graph<T> {
    fn default() -> Self {
        Graph {
            nodes: Vec::new(),
            wires: Vec::new(),
            view: PanZoom::default(),
            kinds: Vec::new(),
            can_connect: same,
            action: GraphAction::None,
            words: GraphWords::default(),
            reroutes: Vec::new(),
            frames: Vec::new(),
            grid: 0.0,
            copied: Vec::new(),
            copied_wires: Vec::new(),
            copied_bends: Vec::new(),
            steps: Vec::new(),
            undone: 0,
            keyed_move: false,
        }
    }
}

impl<T> Graph<T> {
    /// An empty graph whose ports fit by equal kind.
    pub fn new() -> Self {
        Graph::default()
    }
    /// Appends a node, returning its index.
    pub fn add(&mut self, node: Node<T>) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }
    /// Wires output `from` (node, output) into input `to`, where both exist on two nodes and
    /// fit: any wire into a one-wire input is removed. Says whether it is wired.
    pub fn wire(&mut self, from: (usize, usize), to: (usize, usize)) -> bool {
        let (Some(out), Some(into)) = (self.port(from, true), self.port(to, false)) else { return false };
        if from.0 == to.0 || !(self.can_connect)(out.kind, into.kind) {
            return false;
        }
        if self.wires.contains(&Wire { from, to }) {
            return true;
        }
        if !self.nodes[to.0].takes_many(to.1) {
            self.wires.retain(|wire| wire.to != to);
            self.reroutes.retain(|bend| bend.wire.to != to);
        }
        self.wires.push(Wire { from, to });
        true
    }
    /// Removes a wire with its bends. Says whether it was there.
    pub fn unwire(&mut self, wire: Wire) -> bool {
        let had = self.wires.len();
        self.wires.retain(|other| *other != wire);
        self.reroutes.retain(|bend| bend.wire != wire);
        self.wires.len() != had
    }
    /// The wire into input `to` (node, input), if any: the first, where it takes several.
    pub fn wire_into(&self, to: (usize, usize)) -> Option<Wire> {
        self.wires.iter().find(|wire| wire.to == to).copied()
    }
    /// The wires into input `to` (node, input), in order.
    pub fn wires_into(&self, to: (usize, usize)) -> impl Iterator<Item = Wire> + '_ {
        self.wires.iter().filter(move |wire| wire.to == to).copied()
    }
    /// Removes node `node` with its wires; later nodes move up and wires are renumbered.
    pub fn remove(&mut self, node: usize) {
        self.keep_only(|at, _| at != node);
    }
    /// The grid point nearest `point` (`point` itself without a grid).
    pub fn snapped(&self, point: Point) -> Point {
        let step = self.grid;
        if step > 0.0 { pt((point.x / step).round() * step, (point.y / step).round() * step) } else { point }
    }

    /// Keeps the graph as it is now as an undo step: what the user does is kept by itself; an
    /// app keeps it before an edit of its own (a node added from its menu), so that undo goes
    /// back to before it. Each step is a copy of the graph; the oldest go beyond 100 steps or
    /// 100,000 nodes in all.
    pub fn save_undo_step(&mut self)
    where
        T: Clone,
    {
        self.steps.truncate(self.steps.len() - self.undone);
        (self.undone, self.keyed_move) = (0, false);
        self.steps.push(Kept { nodes: self.nodes.clone(), wires: self.wires.clone(), reroutes: self.reroutes.clone(), frames: self.frames.clone() });
        let mut nodes: usize = self.steps.iter().map(|step| step.nodes.len()).sum();
        while self.steps.len() > STEPS || nodes > KEPT && self.steps.len() > 1 {
            nodes -= self.steps.remove(0).nodes.len();
        }
    }
    /// Undoes the last edit kept. Says whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(at) = self.steps.len().checked_sub(self.undone + 1) else { return false };
        self.swap_step(at);
        self.undone += 1;
        true
    }
    /// Redoes the last edit undone. Says whether there was one.
    pub fn redo(&mut self) -> bool {
        if self.undone == 0 {
            return false;
        }
        self.swap_step(self.steps.len() - self.undone);
        self.undone -= 1;
        true
    }
    /// Whether there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        self.steps.len() > self.undone
    }
    /// Whether there is an edit to redo.
    pub fn can_redo(&self) -> bool {
        self.undone > 0
    }
    /// Swaps the graph with undo step `at`: the step then holds what the graph was.
    fn swap_step(&mut self, at: usize) {
        let step = &mut self.steps[at];
        std::mem::swap(&mut self.nodes, &mut step.nodes);
        std::mem::swap(&mut self.wires, &mut step.wires);
        std::mem::swap(&mut self.reroutes, &mut step.reroutes);
        std::mem::swap(&mut self.frames, &mut step.frames);
        self.keyed_move = false;
    }
    /// Adds a bend to `wire` at `at` (snapped to the grid), in piece `piece` of it (0 before its
    /// first bend). Returns the bend's index.
    fn add_bend(&mut self, wire: Wire, piece: usize, at: Point) -> usize {
        let mine: Vec<usize> = (0..self.reroutes.len()).filter(|bend| self.reroutes[*bend].wire == wire).collect();
        let place = mine.get(piece).copied().unwrap_or(mine.last().map_or(self.reroutes.len(), |last| last + 1));
        self.reroutes.insert(place, Reroute { wire, at: self.snapped(at) });
        place
    }
    /// Moves node `node` to the end of the list, on top, where a later node lies over it;
    /// renumbers wires. Returns its index.
    fn raise(&mut self, node: usize) -> usize {
        let last = self.nodes.len().saturating_sub(1);
        let Some(under) = self.nodes.get(node).map(Node::rect) else { return node };
        if node >= last || !self.nodes[node + 1..].iter().any(|over| !over.rect().intersect(under).is_empty()) {
            return node;
        }
        self.nodes[node..].rotate_left(1);
        let places: Vec<usize> = (0..=last).map(|at| if at == node { last } else { at - (at > node) as usize }).collect();
        self.wires.iter_mut().for_each(|wire| _ = renumber(wire, &places));
        self.reroutes.iter_mut().for_each(|bend| _ = renumber(&mut bend.wire, &places));
        last
    }
    /// A node's output (`out`) or input by (node, port index).
    fn port(&self, (node, port): (usize, usize), out: bool) -> Option<&Port> {
        let node = self.nodes.get(node)?;
        if out { node.outputs.get(port) } else { node.inputs.get(port) }
    }
    /// Keeps the nodes `keep` accepts and the wires between them, renumbered.
    fn keep_only(&mut self, keep: impl Fn(usize, &Node<T>) -> bool) {
        // Each node's new index; none for a removed one.
        let (mut places, mut left) = (Vec::with_capacity(self.nodes.len()), 0);
        for (at, node) in self.nodes.iter().enumerate() {
            let keeps = keep(at, node);
            places.push(if keeps { left } else { usize::MAX });
            left += keeps as usize;
        }
        let mut at = 0;
        self.nodes.retain(|_| {
            at += 1;
            places[at - 1] != usize::MAX
        });
        self.wires.retain_mut(|wire| renumber(wire, &places));
        self.reroutes.retain_mut(|bend| renumber(&mut bend.wire, &places));
    }
    /// Copies the selected nodes and the wires between them, for `paste`. Returns how many.
    fn copy_selected(&mut self) -> usize
    where
        T: Clone,
    {
        let (mut places, mut next) = (Vec::with_capacity(self.nodes.len()), 0);
        for node in &self.nodes {
            places.push(if node.selected { next } else { usize::MAX });
            next += node.selected as usize;
        }
        self.copied = self.nodes.iter().filter(|node| node.selected).cloned().collect();
        self.copied_wires = self.wires.iter().copied().filter(|wire| places[wire.from.0] != usize::MAX && places[wire.to.0] != usize::MAX).collect();
        self.copied_wires.iter_mut().for_each(|wire| _ = renumber(wire, &places));
        let mut bends = self.reroutes.clone();
        bends.retain_mut(|bend| renumber(&mut bend.wire, &places));
        self.copied_bends = bends;
        self.copied.len()
    }
    /// Appends copies of the last copied nodes, selected (and nothing else), their top-left at
    /// `at`, or `ASIDE` from the originals.
    fn paste(&mut self, at: Option<Point>)
    where
        T: Clone,
    {
        let around = self.copied.iter().fold(None, |around: Option<Point>, node| Some(around.map_or(node.position, |p| pt(p.x.min(node.position.x), p.y.min(node.position.y)))));
        let Some(around) = around else { return };
        let to = self.snapped(at.unwrap_or(pt(around.x + ASIDE, around.y + ASIDE)));
        let (first, shift) = (self.nodes.len(), pt(to.x - around.x, to.y - around.y));
        let moved = |at: Point| pt(at.x + shift.x, at.y + shift.y);
        self.nodes.iter_mut().for_each(|node| node.selected = false);
        self.nodes.extend(self.copied.iter().cloned().map(|node| Node { position: moved(node.position), selected: true, ..node }));
        let renumbered = |wire: Wire| Wire { from: (wire.from.0 + first, wire.from.1), to: (wire.to.0 + first, wire.to.1) };
        self.wires.extend(self.copied_wires.iter().map(|wire| renumbered(*wire)));
        self.reroutes.extend(self.copied_bends.iter().map(|bend| Reroute { wire: renumbered(bend.wire), at: moved(bend.at) }));
        // (The next paste lies a little further on, as copies of copies do.)
        self.copied.iter_mut().for_each(|node| node.position = moved(node.position));
        self.copied_bends.iter_mut().for_each(|bend| bend.at = moved(bend.at));
    }
    /// The selected nodes' titles, in order.
    fn selected_titles(&self) -> String {
        self.nodes.iter().filter(|node| node.selected).map(|node| node.title.as_str()).collect::<Vec<_>>().join(", ")
    }
    /// Selects the nodes `pick` accepts, and no others. Says whether anything changed.
    fn select_only(&mut self, pick: impl Fn(usize, &Node<T>) -> bool) -> bool {
        let mut changed = false;
        for (at, node) in self.nodes.iter_mut().enumerate() {
            let selects = pick(at, node);
            (changed, node.selected) = (changed || node.selected != selects, selects);
        }
        changed
    }
    /// A port kind's colour on background `ground`.
    fn tint(&self, kind: &str, ground: Color, quiet: Color) -> Color {
        self.kinds.iter().position(|known| *known == kind).map_or(quiet, |nth| Color::series(nth, ground))
    }
    /// Whether a wire may join these two ends: an output and an input, on two nodes, of fitting
    /// kinds.
    fn takes(&self, one: End, other: End) -> bool {
        let (from, into) = if one.out { (one, other) } else { (other, one) };
        let (Some(out), Some(input)) = (self.port((from.node, from.port), true), self.port((into.node, into.port), false)) else { return false };
        from.out && !into.out && from.node != into.node && (self.can_connect)(out.kind, input.kind)
    }
}

/// Renumbers a wire's nodes after removals (`places`: each node's new index, none if removed).
/// Says whether both remain.
fn renumber(wire: &mut Wire, places: &[usize]) -> bool {
    let (from, to) = (places.get(wire.from.0).copied().unwrap_or(usize::MAX), places.get(wire.to.0).copied().unwrap_or(usize::MAX));
    (wire.from.0, wire.to.0) = (from, to);
    from != usize::MAX && to != usize::MAX
}

impl<T> Node<T> {
    /// A node with this title, its top-left at `x`, `y` in the view, holding `data`: no ports
    /// yet, default width.
    pub fn new(title: impl Into<String>, x: f32, y: f32, data: T) -> Self {
        Node { title: title.into(), position: pt(x, y), width: WIDTH, inputs: Vec::new(), outputs: Vec::new(), selected: false, data, high: 0.0, laid: [0.0; 3], many: 0 }
    }
    /// With one more input, below the others: its name and kind.
    pub fn input(mut self, name: impl Into<String>, kind: &'static str) -> Self {
        self.inputs.push(Port { name: name.into(), kind });
        self
    }
    /// With one more input that takes any number of wires (a list, a sum of all): drawn as a
    /// bar. (Only the first 64 inputs can.)
    pub fn multi_input(mut self, name: impl Into<String>, kind: &'static str) -> Self {
        self.many |= 1u64.checked_shl(self.inputs.len() as u32).unwrap_or(0);
        self.input(name, kind)
    }
    /// Whether input `input` takes any number of wires.
    pub fn takes_many(&self, input: usize) -> bool {
        input < 64 && self.many >> input & 1 == 1
    }
    /// With one more output.
    pub fn output(mut self, name: impl Into<String>, kind: &'static str) -> Self {
        self.outputs.push(Port { name: name.into(), kind });
        self
    }
    /// With this width.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    /// Its rectangle in content units, as tall as last laid out (or assumed).
    fn rect(&self) -> Rect {
        let high = if self.high > 0.0 { self.high } else { TOP + self.inputs.len().max(self.outputs.len()) as f32 * ROW + GUESS };
        rect(self.position.x, self.position.y, self.width, high)
    }
    /// Its middle, in content units.
    fn middle(&self) -> Point {
        let r = self.rect();
        pt(r.x + r.w / 2.0, r.y + r.h / 2.0)
    }
}

/// One wire end: a port, as node and index among its outputs (`out`) or inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct End {
    node: usize,
    port: usize,
    out: bool,
}

impl End {
    /// The wire between this end and that: from the output into the input.
    fn wire(self, other: End) -> Wire {
        let (from, into) = if self.out { (self, other) } else { (other, self) };
        Wire { from: (from.node, from.port), to: (into.node, into.port) }
    }
}

/// What a graph box is, for pointer and keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// The node itself, where nothing in it lies.
    Ground,
    /// Its title.
    Title,
    /// One of its inputs, or outputs.
    In(usize),
    Out(usize),
    /// A wire out of it, by index; a bend of one, by index.
    Wire(usize),
    Bend(usize),
    /// A frame's title, or its corner (the node index is the frame's).
    Frame,
    Corner,
}

/// What the pointer is doing to a graph.
#[derive(Clone, Debug, PartialEq)]
enum Doing {
    /// Dragging a wire from this end (the other follows the pointer); `taken`, the wire it was
    /// pulled off an input as, deleted if dropped on nothing.
    Wire { from: End, taken: Option<Wire> },
    /// Moving things together, each with its start position, from the grab point: the selected
    /// nodes; a frame with what it carries; a frame's corner; or a bend (see `BENT`). The first
    /// is the one pressed: it snaps to the grid. A pressed node among several selected is
    /// selected `alone` if released without moving. (Bends and frames go in the nodes' list,
    /// and whether an undo step is kept in `GraphState`: `Doing` is as it was before them, and
    /// so is the code that drops it in programs without graphs.)
    Move { was: Vec<(usize, Point)>, from: Point, alone: Option<usize> },
    /// Rubber-band selecting from this point to the pointer, plus these (Control or Shift was
    /// held).
    Select { from: Point, already: Vec<usize> },
}

/// What a moved thing is, in `Doing::Move`: a node's index as it is; a bend's, a frame's, or a
/// frame's for its bottom right corner (which resizes it), with one of these bits set.
const BENT: usize = 1 << (usize::BITS - 2);
const FRAMED: usize = 1 << (usize::BITS - 3);
const CORNER: usize = 1 << (usize::BITS - 4);

/// A keyboard wire: its start, and the end it currently targets.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyboardWire {
    graph: Id,
    from: End,
    to: Option<End>,
}

/// The state of a graph between passes.
#[derive(Default)]
struct GraphState {
    /// Its node boxes in the last pass and this one: each id, its node, and which part.
    seen: Vec<(Id, usize, Part)>,
    seeing: Vec<(Id, usize, Part)>,
    /// The node focus was in, last pass and this one; and the node the view last panned to (a
    /// frame as `!frame`).
    focus: Option<usize>,
    focusing: Option<usize>,
    shown_for: Option<usize>,
    /// When a drag near the edge last panned the view, in ms of `Ui::now` (wrapping; 0 for not
    /// lately); and whether the drag on it kept an undo step. (Small: they fit where the struct
    /// had padding, so programs without graphs keep their `Ui`.)
    panned_at: u16,
    saved: bool,
    /// The node the graph itself sent focus into this pass: described whether in view or not,
    /// so focus has somewhere to be.
    sent: Option<usize>,
    /// The view's scale in the last pass.
    scale: f32,
    /// What the minimap showed last pass: the view area, its scale, and that area's corner in
    /// the map. (None while there is none.)
    map: Option<(Rect, f32, Point)>,
}

/// The graph being described.
struct OpenGraph {
    /// Its view's id, its inner box (index and id this run), the view's scale, its room in
    /// window units, and its visible area.
    id: Id,
    inner: u32,
    inner_id: Id,
    scale: f32,
    room: Size,
    visible: Rect,
    /// The bounds of all its nodes.
    all: Rect,
    /// The next node to consider; the box of the one being described (`NONE` while none), and
    /// its index.
    next: usize,
    node_box: u32,
    node: usize,
    /// The nodes described whether in view or not: the focused one and its list neighbours, and
    /// a keyboard wire's ends.
    wanted: [usize; 6],
    /// While a wire is dragged: its start, the end it would connect to, and an end under the
    /// pointer that refuses it.
    from: Option<End>,
    target: Option<End>,
    refused: Option<End>,
    /// The pointer's content position.
    pointer: Option<Point>,
    state: GraphState,
}

/// A window's graph state: what pointer and keyboard are doing to one, the state of each
/// between passes, and the one being described.
#[derive(Default)]
pub(crate) struct Graphs {
    open: Option<OpenGraph>,
    states: ById<GraphState>,
    doing: Option<(Id, Doing)>,
    keyed: Option<KeyboardWire>,
    /// For the graph being described: each node as the run passes it; and in one list (so
    /// programs without graphs keep their `Ui`), each port's wire count (each node's inputs
    /// then outputs, node after node), then the bends by wire and order, then (once asked)
    /// each node's wires out (`list_sends`).
    laid: Vec<Laid>,
    wired: Vec<u32>,
    /// The minimap cells being drawn: 0 empty, 1 a node, 2 a selected node. (Kept only to reuse
    /// memory.)
    cells: Vec<u8>,
}

/// A node of the graph being described, as the run passes it: its content position, where its
/// ports start among the wire counts, its input and output counts, and whether selected. (Read
/// once from the nodes, then used by everything that walks them all: a fifth of the memory.)
#[derive(Clone, Copy)]
struct Laid {
    at: Rect,
    first: usize,
    ins: usize,
    outs: usize,
    selected: bool,
}

impl GraphState {
    /// The frame whose title or corner has focus `focus`, as seen last pass.
    fn frame(&self, focus: Id) -> Option<usize> {
        if focus == 0 {
            return None;
        }
        self.seen.iter().find(|(seen, _, part)| *seen == focus && matches!(part, Part::Frame | Part::Corner)).map(|(_, frame, _)| *frame)
    }
}

impl Graphs {
    /// A port's wire count, in the graph being described.
    fn wires_at(&self, end: End) -> u32 {
        let at = self.laid.get(end.node).map_or(usize::MAX, |node| node.first + if end.out { node.ins + end.port } else { end.port });
        self.wired.get(at).copied().unwrap_or(0)
    }
    /// A port's content position in the graph being described: its dot's centre. None for a
    /// missing port.
    fn port_point(&self, end: End) -> Option<Point> {
        let node = self.laid.get(end.node)?;
        let y = node.at.y + TOP + (end.port as f32 + 0.5) * ROW;
        (end.port < if end.out { node.outs } else { node.ins }).then(|| pt(if end.out { node.at.right() } else { node.at.x }, y))
    }
    /// Puts a wire's points in `points`, in the graph being described: its output's dot, its
    /// bends, its input's dot. Says whether both ports are there.
    fn chain(&self, reroutes: &[Reroute], wire: Wire, points: &mut Vec<Point>) -> bool {
        let from = End { node: wire.from.0, port: wire.from.1, out: true };
        let (Some(a), Some(b)) = (self.port_point(from), self.port_point(End { node: wire.to.0, port: wire.to.1, out: false })) else { return false };
        points.clear();
        points.push(a);
        if !reroutes.is_empty() {
            points.extend(self.bends_of(reroutes, wire).iter().filter_map(|at| reroutes.get(*at as usize)).map(|bend| bend.at));
        }
        points.push(b);
        true
    }
    /// A wire's first piece, in the graph being described: from its output's dot to its first
    /// bend, or its input's dot.
    fn first_piece(&self, reroutes: &[Reroute], wire: Wire) -> Option<[Point; 2]> {
        let a = self.port_point(End { node: wire.from.0, port: wire.from.1, out: true })?;
        let b = self.port_point(End { node: wire.to.0, port: wire.to.1, out: false })?;
        let bend = self.bends_of(reroutes, wire).first().and_then(|at| reroutes.get(*at as usize));
        Some([a, bend.map_or(b, |bend| bend.at)])
    }
    /// Where the bends' order, and then the wires out (once listed: `list_sends`), start in
    /// `wired`, for a graph of `bends` bends.
    fn regions(&self, bends: usize) -> [usize; 2] {
        let ports = self.laid.last().map_or(0, |node| node.first + node.ins + node.outs);
        [ports, ports + bends]
    }
    /// Lists each node's wires out after the bends' order, once a pass, when first asked (most
    /// runs describe no wire as a button): a 0, where each node's start, the end, then the wires
    /// by index, node after node, each node's in order.
    fn list_sends(&mut self, wires: &[Wire], bends: usize) {
        let start = self.regions(bends)[1];
        let Graphs { laid, wired, .. } = self;
        let nodes = laid.len();
        if wired.len() > start {
            return;
        }
        // (Counted into the slot after each node's and summed, each slot ends its node's wires;
        // filled from the back, each slot then starts them. The last slot holds the end.)
        wired.resize(start + nodes + 2, 0);
        wires.iter().filter(|wire| fits(laid, wire)).for_each(|wire| wired[start + wire.from.0 + 1] += 1);
        (start + 1..start + nodes + 1).for_each(|at| wired[at] += wired[at - 1]);
        let (list, total) = (start + nodes + 2, wired[start + nodes]);
        wired[start + nodes + 1] = total;
        wired.resize(list + total as usize, 0);
        for (at, wire) in wires.iter().enumerate().rev().filter(|(_, wire)| fits(laid, wire)) {
            let slot = start + wire.from.0 + 1;
            wired[slot] -= 1;
            let place = list + wired[slot] as usize;
            wired[place] = at as u32;
        }
    }
    /// Node `n`'s wires out, by index, once listed (`list_sends`), in a graph of `bends` bends.
    fn sends_of(&self, bends: usize, n: usize) -> &[u32] {
        let (start, nodes) = (self.regions(bends)[1], self.laid.len());
        let (Some(from), Some(to)) = (self.wired.get(start + n + 1), self.wired.get(start + n + 2)) else { return &[] };
        self.wired.get(start + nodes + 2 + *from as usize..start + nodes + 2 + *to as usize).unwrap_or(&[])
    }
    /// A wire's bends, by index, in order.
    fn bends_of(&self, reroutes: &[Reroute], wire: Wire) -> &[u32] {
        let [first, last] = self.regions(reroutes.len());
        let order = self.wired.get(first..last).unwrap_or(&[]);
        let on = |at: &u32| reroutes.get(*at as usize).map(|bend| bend.wire);
        let first = order.partition_point(|at| on(at) < Some(wire));
        &order[first..first + order[first..].iter().take_while(|at| on(at) == Some(wire)).count()]
    }
}

/// Whether node `n` of the graph being described is described whole this run: wanted, or in
/// view and near enough.
fn described(open: &OpenGraph, laid: &[Laid], n: usize) -> bool {
    open.wanted.contains(&n) || laid.get(n).is_some_and(|node| touches(node.at, open.visible)) && open.scale * TOP >= FAR
}

/// Whether two rectangles overlap or touch.
fn touches(a: Rect, b: Rect) -> bool {
    a.x <= b.right() && a.right() >= b.x && a.y <= b.bottom() && a.bottom() >= b.y
}

/// A wire's id: of its item in its output's node.
fn wire_key(wire: Wire) -> u64 {
    hash(wire)
}

/// Node graphs joined by wires.
impl Ui {
    /// Describes a node graph in a pan-and-zoom view (see `node_graph`): each call returns the
    /// next visible node with its title and ports drawn, to describe its content and
    /// [`close`](Self::close); after the last it returns none, and the graph is done. `name` is
    /// its screen reader name.
    /// What the user did is in `graph.action` then.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn graph<T: Clone>(&mut self, graph: &mut Graph<T>, name: &str) -> Option<usize> {
        if self.graph_open() {
            self.node_closed();
        } else {
            self.start_graph(graph, name);
        }
        let Some(node) = self.next_node() else {
            self.end_graph(graph);
            return None;
        };
        self.show_node(graph, node);
        Some(node)
    }

    /// Whether a graph is being described this pass: its view opened this pass and still open.
    /// (Not one left open from an earlier pass by an app that stopped asking for nodes.)
    fn graph_open(&self) -> bool {
        let Some(open) = &self.pan_zoom_views.graphs.open else { return false };
        self.tree.specs.get(open.inner as usize).is_some_and(|spec| spec.id == open.inner_id) && self.open.iter().any(|box_| box_.at == open.inner)
    }

    /// Starts a graph: takes in user actions, opens its view, and describes the grid, the
    /// frames and the visible wires.
    #[cfg_attr(debug_assertions, track_caller)]
    fn start_graph<T: Clone>(&mut self, graph: &mut Graph<T>, name: &str) {
        graph.action = GraphAction::None;
        let (id, outer) = (self.next_id(), self.zoom_here());
        let inner_id = part_id(id, INNER);
        let mut state = self.graph_state(graph, id);
        // User actions, before anything is described.
        let room = self.rooms.get(&inner_id).map_or(self.size(), |room| Size { w: room[0], h: room[1] });
        let ground = self.interact(id);
        self.steer_graph(graph, &mut state, &ground, room, outer);
        self.open_pan_zoom(&mut graph.view, name, ZOOM, false, false);
        let view = &self.pan_zoom_views.this_run[self.pan_zoom_views.this_run.len() - 1];
        let (inner, scale, visible) = (view.inner, view.scale, view.visible);
        let all = self.lay_out(graph);
        if !graph.nodes.is_empty() || !graph.frames.is_empty() {
            self.in_view(all);
        }
        state.scale = scale;
        let first = self.focus == id || state.frame(self.focus).is_some();
        let focus = state.focus.unwrap_or(if first { 0 } else { usize::MAX });
        let keyed = self.pan_zoom_views.graphs.keyed.filter(|keyed| keyed.graph == id);
        let wanted = [
            focus,
            focus.wrapping_sub(1),
            focus.saturating_add(1),
            keyed.map_or(usize::MAX, |keyed| keyed.from.node),
            keyed.and_then(|keyed| keyed.to).map_or(usize::MAX, |to| to.node),
            state.sent.unwrap_or(usize::MAX),
        ];
        let pointer = self.input.pointer.map(|p| pt(graph.view.x + (p.x - ground.rect.x) / scale, graph.view.y + (p.y - ground.rect.y) / scale));
        let opened = OpenGraph { id, inner, inner_id, scale, room, visible, all, next: 0, node_box: NONE, node: 0, wanted, from: None, target: None, refused: None, pointer, state };
        self.pan_zoom_views.graphs.open = Some(opened);
        self.aim_wire(graph);
        self.show_grid(graph);
        self.show_frames(graph);
        self.show_wires(graph);
    }

    /// The state graph `id` kept from the last pass, ready for this one: its focused node or
    /// frame, and the heights of the nodes last laid out (read into `graph`).
    fn graph_state<T>(&mut self, graph: &mut Graph<T>, id: Id) -> GraphState {
        let mut state = self.pan_zoom_views.graphs.states.remove(&id).unwrap_or_default();
        std::mem::swap(&mut state.seen, &mut state.seeing);
        state.seeing.clear();
        // The focused node: the one whose title or port has focus now (it may just have arrived
        // by Tab), else the one focus was in last pass. Or the focused frame.
        let on = state.seen.iter().find(|(seen, ..)| *seen == self.focus).map(|(_, node, part)| (*node, *part));
        let on = on.filter(|(_, part)| !matches!(part, Part::Frame | Part::Corner)).map(|(node, _)| node);
        (state.focus, state.focusing, state.sent) = (on.or(state.focusing.take()), None, None);
        for (box_id, node, _) in state.seen.iter().filter(|(_, _, part)| *part == Part::Ground) {
            let (Some(read), Some(node)) = (self.rooms.get(box_id).copied(), graph.nodes.get_mut(*node)) else { continue };
            self.rooms.remove(box_id);
            (node.laid, node.high) = (read, read[1] / state.scale.max(1e-6));
        }
        state
    }

    /// Walks the nodes once for the run (`Laid`), counts each port's wires (so ports show
    /// whether they have any) and each node's wires out, and sorts the bends by wire. Returns
    /// the bounds of all nodes and frames.
    fn lay_out<T>(&mut self, graph: &Graph<T>) -> Rect {
        let Graphs { laid, wired, .. } = &mut self.pan_zoom_views.graphs;
        laid.clear();
        let (mut ports, mut all) = (0, graph.frames.iter().fold(Rect::EMPTY, |all, frame| all.union(frame.rect)));
        for node in &graph.nodes {
            let (at, ins, outs) = (node.rect(), node.inputs.len(), node.outputs.len());
            laid.push(Laid { at, first: ports, ins, outs, selected: node.selected });
            (ports, all) = (ports + ins + outs, all.union(at));
        }
        wired.clear();
        wired.resize(ports, 0);
        for wire in &graph.wires {
            if !fits(laid, wire) {
                continue;
            }
            let (from, to) = (laid[wire.from.0], laid[wire.to.0]);
            wired[from.first + from.ins + wire.from.1] += 1;
            wired[to.first + wire.to.1] += 1;
        }
        // The bends, by wire then order.
        if !graph.reroutes.is_empty() {
            wired.extend(0..graph.reroutes.len() as u32);
            wired[ports..].sort_unstable_by_key(|at| (graph.reroutes[*at as usize].wire, *at));
        }
        all
    }

    /// The next node to describe whole: visible, or wanted regardless. (Visible ones too small
    /// for that are emitted as plain boxes on the way.)
    fn next_node(&mut self) -> Option<usize> {
        while let Some((node, near)) = self.next_visible() {
            if near {
                return Some(node);
            }
            self.show_far(node);
        }
        None
    }

    /// The next node visible or wanted, and whether to describe it whole.
    fn next_visible(&mut self) -> Option<(usize, bool)> {
        let Graphs { open: Some(open), laid, .. } = &mut self.pan_zoom_views.graphs else { return None };
        let found = (open.next..laid.len()).find(|n| open.wanted.contains(n) || touches(laid[*n].at, open.visible));
        open.next = found.map_or(laid.len(), |n| n + 1);
        found.map(|node| (node, open.wanted.contains(&node) || open.scale * TOP >= FAR))
    }

    /// A node too small to describe whole: a plain box, selected and moved as the node.
    fn show_far(&mut self, n: usize) {
        let (Some(open), t) = (&self.pan_zoom_views.graphs.open, self.theme) else { return };
        let (node, scale) = (self.pan_zoom_views.graphs.laid[n], open.scale.max(1e-6));
        let (border, width) = if node.selected { (t.accent, 2.0) } else { (t.line_strong, 1.0) };
        let far = self.el().key(n as u64).offset(node.at.x, node.at.y).size(node.at.w, node.at.h).fill(t.panel).border(border, width / scale).radius(6.0).sense().show();
        self.seeing(far.id, n, Part::Ground);
    }

    /// After the app closed the last node: notes whether focus is in it.
    fn node_closed(&mut self) {
        let Ui { pan_zoom_views, tree, focus, open, again, focus_shown, .. } = self;
        let Some(graph) = pan_zoom_views.graphs.open.as_mut().filter(|graph| graph.node_box != NONE) else { return };
        debug_assert!(open.last().is_some_and(|open| open.at == graph.inner), "a node of a graph was not closed, or closed more than once");
        let at = std::mem::replace(&mut graph.node_box, NONE) as usize;
        let end = tree.specs.get(at).map_or(at, |spec| spec.end as usize);
        if *focus != 0 && tree.specs[at..end.min(tree.specs.len())].iter().any(|spec| spec.id == *focus) {
            graph.state.focusing = Some(graph.node);
            // (The view pans to a node focus entered by key, in one more pass.)
            *again |= *focus_shown && graph.state.focus != Some(graph.node);
        }
    }

    /// Ends a graph: draws the current interaction over its nodes, closes its view, and puts
    /// its minimap on top.
    fn end_graph<T>(&mut self, graph: &Graph<T>) {
        let Some(mut open) = self.pan_zoom_views.graphs.open.take() else { return };
        self.show_wire_drawn(graph, &open);
        self.show_selecting(&open);
        self.close();
        self.show_map(&mut open);
        self.close();
        self.pan_zoom_views.graphs.states.insert(open.id, open.state);
    }

    /// Records a graph box, for pointer and keyboard to find in the next pass.
    fn seeing(&mut self, id: Id, node: usize, part: Part) {
        if let Some(open) = &mut self.pan_zoom_views.graphs.open {
            open.state.seeing.push((id, node, part));
        }
    }

    // ---- what the person does ----

    /// User actions on a graph: pointer, minimap and keys; and the view pans to a node focus
    /// entered. `ground` is the view, its room `room` (window units), in content zoomed
    /// `outer` times relative to the window.
    fn steer_graph<T: Clone>(&mut self, graph: &mut Graph<T>, state: &mut GraphState, ground: &Response, room: Size, outer: f32) {
        let scale = graph.view.zoom * outer;
        self.pan_near_edge(graph, state, ground, scale);
        let at = self.input.pointer.map(|p| pt(graph.view.x + (p.x - ground.rect.x) / scale, graph.view.y + (p.y - ground.rect.y) / scale));
        let hit = state.seen.iter().find(|(id, ..)| *id == self.hot).map(|(_, node, part)| (*node, *part));
        self.steer_map(graph, ground.id, state, room, scale);
        let doing = self.pan_zoom_views.graphs.doing.as_ref().is_some_and(|(graph, _)| *graph == ground.id);
        if let (true, Some(at)) = (self.input.pressed[0] && !doing, at) {
            state.saved = false;
            // (A pressed node gets focus on its title: described whole, however small.)
            state.sent = self.press_graph(graph, ground, hit, at, scale).or(state.sent);
        }
        if let (true, Some(at)) = (self.input.pressed[1], at) {
            state.sent = self.ask_menu(graph, ground, hit, at).or(state.sent);
        }
        match (self.input.down[0], at) {
            (true, Some(at)) => self.drag_graph(graph, state, ground.id, at),
            (false, _) => self.let_go(graph, ground.id, hit, at),
            _ => {}
        }
        let size = Size { w: room.w / outer, h: room.h / outer };
        self.graph_keys(graph, state, ground.id, at.filter(|_| ground.contains_pointer), size);
        let keys = self.focus_visible();
        pan_to(graph, state, room, scale, keys.then_some(self.focus));
    }

    /// While something is dragged with the pointer near the view's edge (or past it), the view
    /// pans that way, faster nearer, and runs again until it stops.
    fn pan_near_edge<T>(&mut self, graph: &mut Graph<T>, state: &mut GraphState, ground: &Response, scale: f32) {
        let doing = self.pan_zoom_views.graphs.doing.as_ref().filter(|(graph, _)| *graph == ground.id && self.input.down[0]);
        let (r, p, scale) = (ground.rect, self.input.pointer.unwrap_or_default(), scale.max(1e-6));
        let at = pt(graph.view.x + (p.x - r.x) / scale, graph.view.y + (p.y - r.y) / scale);
        // (How far the pointer went since the press, in window units: a click pans nothing.)
        let gone = doing.and_then(|(_, doing)| doing_start(graph, doing)).map_or(0.0, |from| (at.x - from.x).abs().max((at.y - from.y).abs()) * scale);
        let way = pt(push(p.x, r.x, r.right()), push(p.y, r.y, r.bottom()));
        if gone < NEAR || way == pt(0.0, 0.0) {
            state.panned_at = 0;
            return;
        }
        let now = self.now.as_millis() as u16 | 1;
        let since = if state.panned_at == 0 { TICK.as_millis() as u16 } else { now.wrapping_sub(state.panned_at).min(50) };
        let by = SPEED * since as f32 / 1000.0 / scale;
        (graph.view.x, graph.view.y, state.panned_at) = (graph.view.x + way.x * by, graph.view.y + way.y * by, now);
        self.wake_after(TICK);
    }

    /// Left button down on the graph: on a port a wire starts; on a node it comes to the top,
    /// is selected and starts moving with the selection; on a frame's title the frame moves, on
    /// its corner it resizes; on a bend it moves; on a wire it gets focus (a double click bends
    /// it there); on the background a rubber band starts. Returns the node to describe whole.
    fn press_graph<T: Clone>(&mut self, graph: &mut Graph<T>, ground: &Response, hit: Option<(usize, Part)>, at: Point, scale: f32) -> Option<usize> {
        let (mods, double) = (self.input.mods, self.input.clicks == 2);
        let (doing, sent) = match hit {
            Some((node, Part::Out(port))) => (Some(Doing::Wire { from: End { node, port, out: true }, taken: None }), None),
            Some((node, Part::In(port))) => (Some(pull(graph, End { node, port, out: false })), None),
            Some((_, Part::Bend(bend))) => (self.press_bend(graph, bend, at, double), None),
            Some((frame, Part::Frame)) => (graph.frames.get(frame).map(|f| moving(carried(graph, frame, f.rect), at)), None),
            Some((frame, Part::Corner)) => (graph.frames.get(frame).map(|f| moving(vec![(CORNER | frame, pt(f.rect.right(), f.rect.bottom()))], at)), None),
            Some((_, Part::Wire(_))) => self.press_ground(graph, ground.id, at, mods, double, NEAR / scale.max(1e-6)),
            Some((node, _)) => self.press_node(graph, ground.id, node, at, mods),
            None if ground.hovered => self.press_ground(graph, ground.id, at, mods, double, NEAR / scale.max(1e-6)),
            None => (None, None),
        };
        self.pan_zoom_views.graphs.doing = doing.map(|doing| (ground.id, doing));
        sent
    }

    /// A node of the graph with view `id` was pressed at `at` with `mods`: it comes to the top,
    /// is selected (alone; Control toggles; Shift adds), focus to its title. Returns what
    /// follows (the selection drags with the pointer, if selected), and the node's new index.
    fn press_node<T>(&mut self, graph: &mut Graph<T>, id: Id, node: usize, at: Point, mods: Mods) -> (Option<Doing>, Option<usize>) {
        let Some(was) = graph.nodes.get(node).map(|node| node.selected) else { return (None, None) };
        let node = graph.raise(node);
        // (A keyboard wire names nodes by their old places.)
        self.pan_zoom_views.graphs.keyed = self.pan_zoom_views.graphs.keyed.filter(|keyed| keyed.graph != id);
        let changed = match (mods.ctrl, mods.shift || was) {
            (true, _) => graph.select_only(|n, other| if n == node { !was } else { other.selected }),
            (false, true) => graph.select_only(|n, other| n == node || other.selected),
            (false, false) => graph.select_only(|n, _| n == node),
        };
        if changed {
            (graph.action, graph.keyed_move) = (GraphAction::Selected, false);
        }
        let title = part_id(node_id(id, node), TITLE_KEY);
        if self.focus != title {
            self.set_focus(title);
        }
        // (The pressed one first.)
        let mut moving: Vec<_> = graph.nodes.iter().enumerate().filter(|(_, node)| node.selected).map(|(n, node)| (n, node.position)).collect();
        if let Some(first) = moving.iter().position(|(n, _)| *n == node) {
            moving[..=first].rotate_right(1);
        }
        let alone = (was && !mods.ctrl && !mods.shift).then_some(node);
        (graph.nodes[node].selected.then_some(Doing::Move { was: moving, from: at, alone }), Some(node))
    }

    /// The background was pressed at `at`: near a wire, the wire gets focus, or a double click
    /// adds a bend there (`reach` content units counts as near); elsewhere a rubber band
    /// starts (adding to the selection with Control or Shift).
    fn press_ground<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, at: Point, mods: Mods, double: bool, reach: f32) -> (Option<Doing>, Option<usize>) {
        let Some((index, piece)) = wire_near(graph, at, reach) else {
            let already = (0..graph.nodes.len()).filter(|n| (mods.ctrl || mods.shift) && graph.nodes[*n].selected).collect();
            return (Some(Doing::Select { from: at, already }), None);
        };
        let wire = graph.wires[index];
        if !double {
            self.set_focus(part_id(part_id(node_id(id, wire.from.0), SENDS_KEY), wire_key(wire)));
            return (None, Some(wire.from.0));
        }
        graph.save_undo_step();
        graph.add_bend(wire, piece, at);
        (graph.action, self.again) = (GraphAction::Rerouted(wire), true);
        (None, Some(wire.from.0))
    }

    /// A bend was pressed at `at`: it starts moving, or a double click removes it.
    fn press_bend<T: Clone>(&mut self, graph: &mut Graph<T>, bend: usize, at: Point, double: bool) -> Option<Doing> {
        let was = graph.reroutes.get(bend).copied()?;
        if !double {
            return Some(moving(vec![(BENT | bend, was.at)], at));
        }
        graph.save_undo_step();
        graph.reroutes.remove(bend);
        (graph.action, self.again) = (GraphAction::Rerouted(was.wire), true);
        None
    }

    /// Right button down on the graph: on a node, it comes to the top, selected if not
    /// already; or on the background. The app hears it, for its menu. Returns the node.
    fn ask_menu<T>(&mut self, graph: &mut Graph<T>, ground: &Response, hit: Option<(usize, Part)>, at: Point) -> Option<usize> {
        let node = hit.filter(|(_, part)| matches!(part, Part::Ground | Part::Title | Part::In(_) | Part::Out(_))).map(|(node, _)| node);
        let node = node.filter(|node| *node < graph.nodes.len()).map(|node| graph.raise(node));
        if node.is_none() && !ground.hovered {
            return None;
        }
        if let Some(node) = node.filter(|node| !graph.nodes[*node].selected) {
            graph.select_only(|n, _| n == node);
        }
        self.pan_zoom_views.graphs.keyed = self.pan_zoom_views.graphs.keyed.filter(|keyed| keyed.graph != ground.id || node.is_none());
        (graph.action, self.again) = (GraphAction::Menu { node, at }, true);
        node
    }

    /// While the left button is held: what was pressed follows the pointer (selected nodes, a
    /// frame and what it carries, a frame's corner, a bend), or the rubber band does and
    /// selects what it touches.
    fn drag_graph<T: Clone>(&mut self, graph: &mut Graph<T>, state: &mut GraphState, id: Id, at: Point) {
        let Some((_, doing)) = self.pan_zoom_views.graphs.doing.as_mut().filter(|(graph, _)| *graph == id) else { return };
        let did = drag(graph, doing, at, &mut state.saved);
        match doing {
            Doing::Move { was, .. } if was.first().is_some_and(|(what, _)| what & CORNER != 0) => self.set_cursor(Cursor::ResizeDown),
            Doing::Move { .. } => self.set_cursor(Cursor::Move),
            _ => {}
        }
        graph.action = did.unwrap_or(graph.action);
    }

    /// Left button up: a wire connects where dropped (or one pulled off an input is deleted); a
    /// node pressed among several selected and not moved is selected alone; a background press
    /// without a rubber band clears the selection.
    fn let_go<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, hit: Option<(usize, Part)>, at: Option<Point>) {
        let Some(doing) = self.pan_zoom_views.graphs.doing.take_if(|(graph, _)| *graph == id).map(|(_, doing)| doing) else { return };
        let moved = |was: &[(usize, Point)]| was.iter().any(|(what, start)| place_of(graph, *what) != Some(*start));
        let selection_changed = match doing {
            Doing::Move { was, alone: Some(node), .. } if !moved(&was) => graph.select_only(|n, _| n == node),
            Doing::Select { from, already } if at == Some(from) && already.is_empty() => graph.select_only(|_, _| false),
            Doing::Wire { from, taken } => {
                self.drop_wire(graph, from, taken, hit, at);
                false
            }
            _ => false,
        };
        if selection_changed {
            graph.action = GraphAction::Selected;
        }
    }

    /// A wire from `from` was dropped over `hit` at `at`: connected where that is a port (or a
    /// node with a port) that accepts it; the wire it was pulled off an input as, if any, is
    /// deleted. A new wire dropped off every node is reported.
    fn drop_wire<T: Clone>(&mut self, graph: &mut Graph<T>, from: End, taken: Option<Wire>, hit: Option<(usize, Part)>, at: Option<Point>) {
        let made = hit.and_then(|hit| target(graph, from, hit)).filter(|to| graph.takes(from, *to)).map(|to| from.wire(to));
        let on_node = hit.is_some_and(|(_, part)| matches!(part, Part::Ground | Part::Title | Part::In(_) | Part::Out(_)));
        if let (None, None, false, Some(at)) = (made, taken, on_node, at) {
            (graph.action, self.again) = (GraphAction::WireDropped { port: (from.node, from.port), output: from.out, at }, true);
            return;
        }
        let made = made.filter(|wire| Some(*wire) == taken || !graph.wires.contains(wire));
        if made == taken {
            return;
        }
        graph.save_undo_step();
        if let Some(old) = taken {
            graph.unwire(old);
            graph.action = GraphAction::Disconnected(old);
        }
        if let Some(wire) = made.filter(|wire| graph.wire(wire.from, wire.to)) {
            graph.action = GraphAction::Connected(wire);
        }
        self.tell_wire(graph);
        self.again |= graph.action != GraphAction::None;
    }

    /// Presses and drags on a graph's minimap: the view pans to the pointer's point on it.
    fn steer_map<T>(&mut self, graph: &mut Graph<T>, id: Id, state: &GraphState, room: Size, scale: f32) {
        let map = self.interact(part_id(id, MAP_KEY));
        let Some(((world, smaller, corner), p)) = state.map.zip(self.input.pointer).filter(|_| map.pressed) else { return };
        let at = pt(world.x + (p.x - map.rect.x - corner.x) / smaller, world.y + (p.y - map.rect.y - corner.y) / smaller);
        (graph.view.x, graph.view.y) = (at.x - room.w / scale / 2.0, at.y - room.h / scale / 2.0);
        self.set_cursor(Cursor::Move);
    }

    /// Keys while focus is in the view, a title, a port, a wire, a bend or a frame. `at` is the
    /// pointer's content position while over it; `size` the view's room in content units at
    /// zoom 1.
    fn graph_keys<T: Clone>(&mut self, graph: &mut Graph<T>, state: &mut GraphState, id: Id, at: Option<Point>, size: Size) {
        if self.pan_zoom_views.graphs.keyed.is_some_and(|keyed| keyed.graph == id) {
            state.sent = self.keyed_keys(graph, id);
            return;
        }
        let on = state.seen.iter().find(|(seen, ..)| *seen == self.focus).map(|(_, node, part)| (*node, *part));
        if self.focus != id && on.is_none() || self.undo_keys(graph) {
            return;
        }
        let nodes = graph.nodes.len();
        match on {
            Some((node, Part::Title)) if node < nodes => self.title_keys(graph, node),
            Some((node, Part::In(port))) if node < nodes => self.port_keys(graph, id, End { node, port, out: false }),
            Some((node, Part::Out(port))) if node < nodes => self.port_keys(graph, id, End { node, port, out: true }),
            Some((_, Part::Wire(wire))) => state.sent = self.wire_keys(graph, id, wire),
            Some((_, Part::Bend(bend))) => self.bend_keys(graph, id, bend),
            Some((frame, Part::Frame | Part::Corner)) => self.frame_keys(graph, id, frame),
            _ => {}
        }
        self.selection_keys(graph, at, matches!(on, None | Some((_, Part::Title | Part::Ground))));
        self.frame_and_show_keys(graph, size);
    }

    /// Control+Z undoes, Control+Y and Control+Shift+Z redo. Says whether either did.
    fn undo_keys<T>(&mut self, graph: &mut Graph<T>) -> bool {
        let undo = self.key(Key::Char('z'), Mods::CTRL);
        let redo = self.key(Key::Char('y'), Mods::CTRL) || self.key(Key::Char('z'), Mods { shift: true, ..Mods::CTRL });
        let (action, words) = match () {
            _ if undo && graph.undo() => (GraphAction::Undone, graph.words.undone),
            _ if redo && graph.redo() => (GraphAction::Redone, graph.words.redone),
            _ => return false,
        };
        (graph.action, self.again) = (action, true);
        self.pan_zoom_views.graphs.doing = None;
        self.announce(words);
        true
    }

    /// Keys on a title: arrows move the node, with the selection if selected (by the grid's
    /// spacing, where it has one); Space selects it (Control toggles; Shift adds); Delete
    /// removes it, with the selection if selected.
    fn title_keys<T: Clone>(&mut self, graph: &mut Graph<T>, node: usize) {
        let step = self.arrow_step(graph);
        if step != pt(0.0, 0.0) {
            let was_selected = graph.nodes[node].selected;
            let start = graph.nodes[node].position;
            let by = snap_by(graph, start, step);
            keep_keyed_move(graph);
            for other in graph.nodes.iter_mut().enumerate().filter(|(n, other)| if was_selected { other.selected } else { *n == node }).map(|(_, other)| other) {
                other.position = pt(other.position.x + by.x, other.position.y + by.y);
            }
            graph.action = GraphAction::Moved;
        }
        let space = [Mods::NONE, Mods::CTRL, Mods::SHIFT].into_iter().find(|mods| self.key(Key::Space, *mods));
        let selection_changed = match space {
            Some(Mods::CTRL) => graph.select_only(|n, other| if n == node { !other.selected } else { other.selected }),
            Some(Mods::SHIFT) => graph.select_only(|n, other| n == node || other.selected),
            Some(_) => graph.select_only(|n, _| n == node),
            None => false,
        };
        if selection_changed {
            (graph.action, graph.keyed_move) = (GraphAction::Selected, false);
        }
        // (An unselected node is removed alone.)
        if !graph.nodes[node].selected && self.delete_key() {
            graph.select_only(|n, _| n == node);
            self.delete_selected(graph);
        }
    }

    /// How far the arrow keys pressed move something: by the grid's spacing, or a nudge.
    fn arrow_step<T>(&mut self, graph: &Graph<T>) -> Point {
        let arrows = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| self.key(key, Mods::NONE) as i32 as f32);
        let step = if graph.grid > 0.0 { graph.grid } else { NUDGE };
        pt(step * (arrows[1] - arrows[0]), step * (arrows[3] - arrows[2]))
    }

    /// Whether Delete or Backspace was pressed.
    fn delete_key(&mut self) -> bool {
        [Key::Delete, Key::Backspace].iter().any(|key| self.key(*key, Mods::NONE))
    }

    /// Keys on a port: Enter or Space starts a keyboard wire; Delete removes its wires.
    fn port_keys<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, end: End) {
        if self.key(Key::Enter, Mods::NONE) || self.key(Key::Space, Mods::NONE) {
            self.pan_zoom_views.graphs.keyed = Some(KeyboardWire { graph: id, from: end, to: None });
            self.announce(graph.words.wiring);
            return;
        }
        if !self.delete_key() {
            return;
        }
        let at = (end.node, end.port);
        let mine = |wire: &Wire| if end.out { wire.from == at } else { wire.to == at };
        let Some(wire) = graph.wires.iter().rev().find(|wire| mine(wire)).copied() else { return };
        graph.save_undo_step();
        graph.wires.retain(|wire| !mine(wire));
        graph.reroutes.retain(|bend| !mine(&bend.wire));
        (graph.action, self.again) = (GraphAction::Disconnected(wire), true);
        self.tell_wire(graph);
    }

    /// Keys while a keyboard wire is active: arrows cycle the ports it could connect to (focus
    /// and view following), Enter or Space connects, Escape cancels; so does focus leaving.
    /// Returns the node focus was sent into, if any.
    fn keyed_keys<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id) -> Option<usize> {
        let keyed = self.pan_zoom_views.graphs.keyed?;
        let port = |end: End| port_id(id, end);
        if self.focus != port(keyed.from) && keyed.to.is_none_or(|to| self.focus != port(to)) {
            self.pan_zoom_views.graphs.keyed = None;
            return None;
        }
        let forward = [Key::Down, Key::Right].iter().any(|key| self.key(*key, Mods::NONE));
        let back = [Key::Up, Key::Left].iter().any(|key| self.key(*key, Mods::NONE));
        if let Some(to) = (forward || back).then(|| step(graph, keyed.from, keyed.to, forward)).flatten() {
            self.pan_zoom_views.graphs.keyed = Some(KeyboardWire { to: Some(to), ..keyed });
            self.set_focus(port(to));
            self.focus_shown = true;
        }
        let make = self.key(Key::Enter, Mods::NONE) || self.key(Key::Space, Mods::NONE);
        if !make && !self.key(Key::Escape, Mods::NONE) {
            return None;
        }
        self.pan_zoom_views.graphs.keyed = None;
        self.set_focus(port(keyed.from));
        let wire = keyed.to.filter(|to| make && graph.takes(keyed.from, *to)).map(|to| keyed.from.wire(to)).filter(|wire| !graph.wires.contains(wire));
        if wire.is_some() {
            graph.save_undo_step();
        }
        match wire.filter(|wire| graph.wire(wire.from, wire.to)) {
            Some(wire) => {
                (graph.action, self.again) = (GraphAction::Connected(wire), true);
                self.tell_wire(graph);
            }
            None => self.announce(graph.words.stopped),
        }
        Some(keyed.from.node)
    }

    /// Keys on a wire (by index): Enter or Space goes to its input, B adds a bend in its
    /// first piece's middle (focus going there), Delete removes it (focus going to its
    /// output). Returns the node focus went into.
    fn wire_keys<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, index: usize) -> Option<usize> {
        let wire = graph.wires.get(index).copied()?;
        let to = End { node: wire.to.0, port: wire.to.1, out: false };
        if self.key(Key::Enter, Mods::NONE) || self.key(Key::Space, Mods::NONE) {
            (self.focus_shown, self.again) = (true, true);
            self.set_focus(port_id(id, to));
            return Some(to.node);
        }
        if self.key(Key::Char('b'), Mods::NONE) {
            let mut points = Vec::new();
            let middle = wire_points(graph, wire, &mut points).then(|| on_curve(bend(points[0], points[1]), 0.5))?;
            graph.save_undo_step();
            let bend = graph.add_bend(wire, 0, middle);
            graph.action = GraphAction::Rerouted(wire);
            self.set_focus(part_id(part_id(node_id(id, wire.from.0), SENDS_KEY), BEND_KEY + bend as u64));
            self.focus_shown = true;
            return Some(wire.from.0);
        }
        if !self.delete_key() {
            return None;
        }
        graph.save_undo_step();
        graph.unwire(wire);
        (graph.action, self.again) = (GraphAction::Disconnected(wire), true);
        self.tell_wire(graph);
        self.set_focus(port_id(id, End { node: wire.from.0, port: wire.from.1, out: true }));
        self.focus_shown = true;
        Some(wire.from.0)
    }

    /// Keys on a bend: arrows move it, Delete removes it (focus going to its wire).
    fn bend_keys<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, bend: usize) {
        let Some(was) = graph.reroutes.get(bend).copied() else { return };
        let step = self.arrow_step(graph);
        if step != pt(0.0, 0.0) {
            keep_keyed_move(graph);
            graph.reroutes[bend].at = graph.snapped(pt(was.at.x + step.x, was.at.y + step.y));
            graph.action = GraphAction::Moved;
        }
        if self.delete_key() {
            graph.save_undo_step();
            graph.reroutes.remove(bend);
            (graph.action, self.again) = (GraphAction::Rerouted(was.wire), true);
            self.set_focus(part_id(part_id(node_id(id, was.wire.from.0), SENDS_KEY), wire_key(was.wire)));
            self.focus_shown = true;
        }
    }

    /// Keys on a frame's title or corner: arrows move it with what it carries, Shift and arrows
    /// resize it, Delete removes it (not what is in it).
    fn frame_keys<T: Clone>(&mut self, graph: &mut Graph<T>, id: Id, frame: usize) {
        let Some(was) = graph.frames.get(frame).map(|frame| frame.rect) else { return };
        let step = self.arrow_step(graph);
        if step != pt(0.0, 0.0) {
            keep_keyed_move(graph);
            move_by(graph, &carried(graph, frame, was), snap_by(graph, pt(was.x, was.y), step));
            graph.action = GraphAction::Moved;
        }
        let shift = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| self.key(key, Mods::SHIFT) as i32 as f32);
        let by = if graph.grid > 0.0 { graph.grid } else { NUDGE };
        if shift != [0.0; 4] {
            keep_keyed_move(graph);
            let corner = pt(was.right(), was.bottom());
            move_by(graph, &[(CORNER | frame, corner)], snap_by(graph, corner, pt(by * (shift[1] - shift[0]), by * (shift[3] - shift[2]))));
            graph.action = GraphAction::Moved;
        }
        if self.delete_key() {
            graph.save_undo_step();
            graph.frames.remove(frame);
            (graph.action, self.again) = (GraphAction::Framed, true);
            self.set_focus(id);
        }
    }

    /// Selection keys in the view, a title or a port: Control+A selects all, Escape none;
    /// Control+C, X, V copy, cut and paste (at the pointer where it is over the view);
    /// Control+D duplicates beside them; Delete removes them (but on a port it removes wires:
    /// `delete` says whether not on one).
    fn selection_keys<T: Clone>(&mut self, graph: &mut Graph<T>, at: Option<Point>, delete: bool) {
        let any = graph.nodes.iter().any(|node| node.selected);
        let all = self.key(Key::Char('a'), Mods::CTRL) && graph.select_only(|_, _| true);
        let none = any && self.key(Key::Escape, Mods::NONE) && graph.select_only(|_, _| false);
        if all || none {
            (graph.action, graph.keyed_move) = (GraphAction::Selected, false);
        }
        let [copy, cut, twice] = [Key::Char('c'), Key::Char('x'), Key::Char('d')].map(|key| any && self.key(key, Mods::CTRL));
        if copy || cut || twice {
            graph.copy_selected();
        }
        if twice || self.key(Key::Char('v'), Mods::CTRL) && !graph.copied.is_empty() {
            graph.save_undo_step();
            graph.paste(if twice { None } else { at });
            (graph.action, self.again) = (GraphAction::Pasted, true);
            let words = format!("{} {}", graph.words.pasted, graph.selected_titles());
            self.announce(&words);
        }
        if cut || delete && any && self.delete_key() {
            self.delete_selected(graph);
        }
    }

    /// Control+G puts a frame around the selected nodes; F shows them (all nodes where none
    /// are selected) as near as fits, but no nearer than zoom 1. `size` is the view's room in
    /// content units at zoom 1.
    fn frame_and_show_keys<T: Clone>(&mut self, graph: &mut Graph<T>, size: Size) {
        let picked = graph.nodes.iter().filter(|node| node.selected).fold(Rect::EMPTY, |all, node| all.union(node.rect()));
        if !picked.is_empty() && self.key(Key::Char('g'), Mods::CTRL) {
            graph.save_undo_step();
            let around = rect(picked.x - MARGIN, picked.y - MARGIN - TOP, picked.w + 2.0 * MARGIN, picked.h + 2.0 * MARGIN + TOP);
            graph.frames.push(Frame::new(graph.words.frame, around));
            (graph.action, self.again) = (GraphAction::Framed, true);
        }
        if self.key(Key::Char('f'), Mods::NONE) {
            let all = graph.nodes.iter().fold(Rect::EMPTY, |all, node| all.union(node.rect()));
            let near = ZOOM.start().min(graph.view.zoom)..=graph.view.zoom.max(1.0);
            graph.view.fit(if picked.is_empty() { all } else { picked }, size, near);
        }
    }

    /// Removes the selected nodes with their wires.
    fn delete_selected<T: Clone>(&mut self, graph: &mut Graph<T>) {
        graph.save_undo_step();
        let words = format!("{} {}", graph.words.deleted, graph.selected_titles());
        graph.keep_only(|_, node| !node.selected);
        (graph.action, self.again) = (GraphAction::Deleted, true);
        self.announce(&words);
    }

    /// Announces a just made or removed wire to screen readers.
    fn tell_wire<T>(&mut self, graph: &Graph<T>) {
        let (word, wire) = match graph.action {
            GraphAction::Connected(wire) => (graph.words.wired, wire),
            GraphAction::Disconnected(wire) => (graph.words.unwired, wire),
            _ => return,
        };
        if self.listening {
            let words = format!("{word} {}", wire_words(graph, wire));
            self.announce(&words);
        }
    }

    // ---- what is shown ----

    /// While a wire is dragged or keyed: its start, the end it would connect to, and an end
    /// under the pointer that refuses it.
    fn aim_wire<T>(&mut self, graph: &Graph<T>) {
        let Graphs { open: Some(open), doing, keyed, .. } = &mut self.pan_zoom_views.graphs else { return };
        let hit = open.state.seen.iter().find(|(id, ..)| *id == self.hot).map(|(_, node, part)| (*node, *part));
        if let Some(keyed) = keyed.filter(|keyed| keyed.graph == open.id) {
            (open.from, open.target) = (Some(keyed.from), keyed.to);
            return;
        }
        let Some((_, Doing::Wire { from, .. })) = doing.as_ref().filter(|(graph, _)| *graph == open.id) else { return };
        let under = hit.and_then(|hit| target(graph, *from, hit));
        open.from = Some(*from);
        (open.target, open.refused) = (under.filter(|to| graph.takes(*from, *to)), under.filter(|to| !graph.takes(*from, *to)));
        if open.refused.is_some() {
            self.set_cursor(Cursor::No);
        }
    }

    /// The grid under everything, where there is one: lines across what is in view, every
    /// second, fourth… one where they would lie closer than `FINEST` on screen.
    fn show_grid<T>(&mut self, graph: &Graph<T>) {
        let (Some(open), t) = (&self.pan_zoom_views.graphs.open, self.theme) else { return };
        let (visible, scale, mut step) = (open.visible, open.scale.max(1e-6), graph.grid);
        if !(step > 0.0 && step.is_finite()) {
            return;
        }
        while step * scale < FINEST {
            step *= 2.0;
        }
        let (left, top) = ((visible.x / step).floor() * step, (visible.y / step).floor() * step);
        let (across, down) = (((visible.right() - left) / step).ceil() as usize + 1, ((visible.bottom() - top) / step).ceil() as usize + 1);
        let (wide, high, thin) = (across as f32 * step, down as f32 * step, 1.0 / scale);
        let mut lines = Path::new();
        (0..across).for_each(|k| _ = lines.rect(k as f32 * step, 0.0, thin, high));
        (0..down).for_each(|k| _ = lines.rect(0.0, k as f32 * step, wide, thin));
        self.el().offset(left, top).size(wide, high).shape(&lines, t.line).show();
    }

    /// The frames in view (and the focused one), under the wires.
    fn show_frames<T>(&mut self, graph: &Graph<T>) {
        let Some(open) = &self.pan_zoom_views.graphs.open else { return };
        let (visible, focused) = (open.visible, open.state.frame(self.focus));
        for frame in (0..graph.frames.len()).filter(|f| touches(graph.frames[*f].rect, visible) || focused == Some(*f)) {
            self.show_frame(graph, frame);
        }
    }

    /// A frame: a tinted box with its title along the top (moved by it) and a grip in its
    /// bottom right corner (resized by it).
    fn show_frame<T>(&mut self, graph: &Graph<T>, f: usize) {
        let (t, frame) = (self.theme, &graph.frames[f]);
        let r = frame.rect;
        let whole = self.el().key(FRAME_KEY + f as u64).offset(r.x, r.y).size(r.w.max(0.0), r.h.max(0.0)).flow(Flow::Stack);
        whole.fill(t.accent.with_alpha(0.06)).border(t.line_strong, 1.0).radius(8.0).open();
        let pad = Insets { left: 10.0, right: 10.0, ..Insets::ZERO };
        let title = self.el().key(TITLE_KEY).size(r.w.max(0.0), TOP).pad(pad).text(&frame.title, t.font, t.text_dim).clip().sense().focusable().show();
        let words = if self.listening { format!("{}: {}", graph.words.frame, frame.title) } else { String::new() };
        self.tell(&title, Role::Button, None, &words);
        if title.hovered {
            self.set_cursor(Cursor::Move);
        }
        self.seeing(title.id, f, Part::Frame);
        let corner = self.el().key(CORNER_KEY).place(Align::End).size(HIT, HIT).flow(Flow::Stack).align(Align::End, Align::End).pad(4.0).sense().open();
        self.el().size(6.0, 6.0).radius(1.0).fill(t.line_strong).show();
        self.close();
        if corner.hovered {
            self.set_cursor(Cursor::ResizeDown);
        }
        self.seeing(corner.id, f, Part::Corner);
        self.close();
    }

    /// The visible wires, under the nodes, through their bends; and the dots of bends whose
    /// output's node is not described whole (those are described in it). A wire being pulled
    /// off its input is drawn as a dragged one: see `show_wire_drawn`.
    fn show_wires<T>(&mut self, graph: &Graph<T>) {
        let Some(open) = &self.pan_zoom_views.graphs.open else { return };
        let (visible, scale, t) = (open.visible, open.scale, self.theme);
        let pulled = match &self.pan_zoom_views.graphs.doing {
            Some((id, Doing::Wire { taken, .. })) if *id == open.id => *taken,
            _ => None,
        };
        // (The wire whose button the pointer or the keyboard is on stands out. Asked once: a
        // walk over all that was seen.)
        let keys = self.keys_in_graph();
        let lit = keys.then(|| open.state.seen.iter().find(|(seen, _, part)| matches!(part, Part::Wire(_)) && (*seen == self.hot || *seen == self.focus))).flatten().map(|(.., part)| *part);
        // (Wire width in content units: at least one window unit.)
        let [thin, thick] = [2.0, 3.5].map(|width: f32| width.max(1.0) / scale.max(1e-6));
        let (bent, mut points) = (!graph.reroutes.is_empty(), Vec::new());
        for (index, wire) in graph.wires.iter().enumerate().filter(|(_, wire)| Some(**wire) != pulled) {
            let graphs = &self.pan_zoom_views.graphs;
            let (Some(a), Some(b)) = (graphs.port_point(End { node: wire.from.0, port: wire.from.1, out: true }), graphs.port_point(End { node: wire.to.0, port: wire.to.1, out: false })) else {
                continue;
            };
            // (Most wires have no bends: those cost no list.)
            let whole = if bent && graphs.chain(&graph.reroutes, *wire, &mut points) { chain_around(&points, thick) } else { around(bend(a, b), thick) };
            if !touches(whole, visible) {
                continue;
            }
            let lit = lit == Some(Part::Wire(index));
            let selected = lit || graphs.laid[wire.from.0].selected || graphs.laid[wire.to.0].selected;
            let color = if lit { t.accent } else { graph.tint(graph.nodes[wire.from.0].outputs[wire.from.1].kind, t.bg, t.text_dim) };
            let plain = [a, b];
            self.curves(if bent { &points } else { &plain }, color, if selected { thick } else { thin });
            if bent {
                self.loose_bends(graph, *wire, keys);
            }
        }
    }

    /// Whether the keyboard or a screen reader is in the graph being described (or a press just
    /// sent focus into it): only then are wires buttons, and bends described in their nodes.
    fn keys_in_graph(&self) -> bool {
        let Some(open) = &self.pan_zoom_views.graphs.open else { return false };
        let state = &open.state;
        let focused = self.focus == open.id || state.focus.is_some() || state.frame(self.focus).is_some();
        self.listening || state.sent.is_some() || self.focus != 0 && focused
    }

    /// A wire's bends, where its output's node does not describe them (it is not described
    /// whole, or the keyboard is elsewhere: `keys`): for the pointer only.
    fn loose_bends<T>(&mut self, graph: &Graph<T>, wire: Wire, keys: bool) {
        let Graphs { open: Some(open), laid, .. } = &self.pan_zoom_views.graphs else { return };
        let count = self.pan_zoom_views.graphs.bends_of(&graph.reroutes, wire).len();
        if count == 0 || keys && described(open, laid, wire.from.0) {
            return;
        }
        for nth in 0..count {
            let bend = self.pan_zoom_views.graphs.bends_of(&graph.reroutes, wire)[nth] as usize;
            self.bend_dot(graph, bend, pt(0.0, 0.0), false);
        }
    }

    /// A bend's dot, in its wire's colour, placed from `origin` (the content point of the box
    /// it lies in); one the keyboard reaches (`keyed`) is told to readers.
    fn bend_dot<T>(&mut self, graph: &Graph<T>, bend: usize, origin: Point, keyed: bool) {
        let (Some(at), t) = (graph.reroutes.get(bend).copied(), self.theme) else { return };
        let kind = graph.nodes.get(at.wire.from.0).and_then(|node| node.outputs.get(at.wire.from.1)).map_or("", |port| port.kind);
        let place = self.el().key(BEND_KEY + bend as u64).offset(at.at.x - origin.x - HIT / 2.0, at.at.y - origin.y - HIT / 2.0).size(HIT, HIT);
        let dot = place.flow(Flow::Stack).align(Align::Center, Align::Center).sense();
        let dot = if keyed { dot.focusable().open() } else { dot.open() };
        if keyed {
            let words = if self.listening { format!("{}, {}", graph.words.bend, wire_words(graph, at.wire)) } else { String::new() };
            self.tell(&dot, Role::Button, None, &words);
            self.tell_shortcut("Delete");
        }
        self.el().size(BEND, BEND).radius(BEND / 2.0).fill(graph.tint(kind, t.bg, t.text_dim)).border(t.panel, 1.5).show();
        self.close();
        if dot.hovered {
            self.set_cursor(Cursor::Move);
        }
        self.seeing(dot.id, at.wire.from.0, Part::Bend(bend));
    }

    /// A wire through these points, each piece a curve (`bend`), `width` content units wide.
    fn curves(&mut self, points: &[Point], color: Color, width: f32) {
        let whole = chain_around(points, width);
        if points.len() < 2 || !whole.w.is_finite() || !whole.h.is_finite() {
            return;
        }
        let (left, top) = (whole.x, whole.y);
        let mut path = Path::new();
        path.move_to(points[0].x - left, points[0].y - top);
        for pair in points.windows(2) {
            let [_, c, d, b] = bend(pair[0], pair[1]);
            path.cubic_to(c.x - left, c.y - top, d.x - left, d.y - top, b.x - left, b.y - top);
        }
        self.el().offset(left, top).size(whole.w, whole.h).outline(&path, width, color).show();
    }

    /// A node with its title and ports, its wires out and their bends, and the box under them
    /// for the app's parts (left open: the app closes it, and the node with it).
    #[cfg_attr(debug_assertions, track_caller)]
    fn show_node<T>(&mut self, graph: &Graph<T>, n: usize) {
        let (t, node) = (self.theme, &graph.nodes[n]);
        let at = self.tree.specs.len() as u32;
        let (border, width) = if node.selected { (t.accent, 2.0) } else { (t.line_strong, 1.0) };
        let whole = self.el().key(n as u64).offset(node.position.x, node.position.y).w(Length::Px(node.width)).flow(Flow::Column).align(Align::Start, Align::Spread);
        let whole = whole.fill(t.panel).border(border, width).radius(6.0).shadow(4.0).sense().open();
        self.tell(&whole, Role::Group, None, &node.title);
        self.measures.push(Measure { at, within: at, id: whole.id, read: node.laid });
        if let Some(open) = &mut self.pan_zoom_views.graphs.open {
            (open.node_box, open.node) = (at, n);
        }
        self.seeing(whole.id, n, Part::Ground);
        let title = self.el().key(TITLE_KEY).h(Length::Px(TOP - 1.0)).pad(Insets { left: 10.0, right: 10.0, ..Insets::ZERO }).text(&node.title, t.font, t.text).clip().sense().focusable().show();
        self.tell(&title, Role::Row, Some(node.selected), &node.title);
        self.tell_position(n + 1, graph.nodes.len());
        self.seeing(title.id, n, Part::Title);
        self.el().h(Length::Px(1.0)).fill(t.line).show();
        let rows = node.inputs.len().max(node.outputs.len());
        for row in 0..rows {
            self.port_row(graph, n, row);
        }
        self.show_sends(graph, n, pt(node.position.x, node.position.y + TOP + rows as f32 * ROW));
        self.join_next = true;
        // (App content stays inside: drawn outside, it would not be redrawn where it changes.)
        let body = self.el().key(BODY_KEY).flow(Flow::Column).align(Align::Start, Align::Spread).pad(Insets { left: 10.0, top: 4.0, right: 10.0, bottom: 10.0 });
        body.gap(6.0).clip().open();
    }

    /// A node's wires out, each a button on its first piece's middle that the keyboard reaches
    /// (Tab once, Left and Right go round), and their bends; in a box of no height at `origin`
    /// (content units) under the ports. Only while the keyboard or a reader is in the graph:
    /// otherwise no one would reach them, and they would cost every run.
    fn show_sends<T>(&mut self, graph: &Graph<T>, n: usize, origin: Point) {
        let (bends, graphs) = (graph.reroutes.len(), &mut self.pan_zoom_views.graphs);
        let outs = graphs.laid.get(n).map_or(0..0, |node| node.first + node.ins..node.first + node.ins + node.outs);
        if graphs.wired.get(outs).is_none_or(|counts| counts.iter().all(|count| *count == 0)) || !self.keys_in_graph() {
            return;
        }
        self.pan_zoom_views.graphs.list_sends(&graph.wires, bends);
        let count = self.pan_zoom_views.graphs.sends_of(bends, n).len();
        let row = self.el().key(SENDS_KEY).flow(Flow::Stack).h(Length::Px(0.0)).open();
        for nth in 0..count {
            let wire = self.pan_zoom_views.graphs.sends_of(bends, n)[nth] as usize;
            self.wire_item(graph, n, wire, origin, row.id);
        }
        for nth in 0..count {
            let Some(wire) = graph.wires.get(self.pan_zoom_views.graphs.sends_of(bends, n)[nth] as usize).copied() else { continue };
            let count = self.pan_zoom_views.graphs.bends_of(&graph.reroutes, wire).len();
            (0..count).for_each(|nth| self.bend_dot(graph, self.pan_zoom_views.graphs.bends_of(&graph.reroutes, wire)[nth] as usize, origin, true));
        }
        self.close();
    }

    /// Wire `index` of node `n` as a button readers hear by both its ends: on its first piece's
    /// middle, seen only by its focus ring.
    fn wire_item<T>(&mut self, graph: &Graph<T>, n: usize, index: usize, origin: Point, group: Id) {
        let Some(wire) = graph.wires.get(index).copied() else { return };
        let Some([a, b]) = self.pan_zoom_views.graphs.first_piece(&graph.reroutes, wire) else { return };
        let middle = on_curve(bend(a, b), 0.5);
        let item = self.el().key(wire_key(wire)).offset(middle.x - origin.x - HIT / 2.0, middle.y - origin.y - HIT / 2.0).size(HIT, HIT).sense().focusable().show();
        let words = if self.listening { format!("{}, {}", graph.words.wire, wire_words(graph, wire)) } else { String::new() };
        self.tell(&item, Role::Button, None, &words);
        self.tell_shortcut("Delete");
        self.rovers.push(Rover { group, id: item.id, stop: false, selects: false });
        if item.hovered {
            self.set_cursor(Cursor::Hand);
        }
        self.seeing(item.id, n, Part::Wire(index));
    }

    /// A row of a node's ports: that row's input at the left, output at the right, each
    /// labelled.
    fn port_row<T>(&mut self, graph: &Graph<T>, n: usize, row: usize) {
        let (t, node) = (self.theme, &graph.nodes[n]);
        let (input, output) = (node.inputs.get(row), node.outputs.get(row));
        let room = if input.is_some() && output.is_some() { node.width / 2.0 - 14.0 } else { node.width - 24.0 };
        self.el().key(ROW_KEY + row as u64).flow(Flow::Stack).h(Length::Px(ROW)).open();
        if let Some(port) = input {
            self.el().offset(12.0, 0.0).size(room.max(0.0), ROW).text(&port.name, t.small_font, t.text_dim).clip().show();
            self.port_dot(graph, End { node: n, port: row, out: false });
        }
        if let Some(port) = output {
            let name = self.el().place((Align::End, Align::Start)).offset(-12.0, 0.0).size(room.max(0.0), ROW);
            name.text(&port.name, t.small_font, t.text_dim).text_align(Align::End).clip().show();
            self.port_dot(graph, End { node: n, port: row, out: true });
        }
        self.close();
    }

    /// A port's dot, half over its node's edge: filled in its kind's colour where wired, a ring
    /// where not (a bar, for an input that takes several). While a wire is dragged, accepting
    /// ports are ringed, the rest fade, and one under the pointer that refuses is ringed in red.
    fn port_dot<T>(&mut self, graph: &Graph<T>, end: End) {
        let t = self.theme;
        let node = &graph.nodes[end.node];
        let kind = if end.out { node.outputs[end.port].kind } else { node.inputs[end.port].kind };
        let color = graph.tint(kind, t.bg, t.text_dim);
        let wired = self.pan_zoom_views.graphs.wires_at(end) > 0;
        let (from, target, refused) = self.pan_zoom_views.graphs.open.as_ref().map_or((None, None, None), |open| (open.from, open.target, open.refused));
        let takes = from.is_some_and(|from| graph.takes(from, end));
        let (fill, ring) = match () {
            _ if refused == Some(end) => (t.panel, t.red),
            _ if target == Some(end) || from == Some(end) => (t.accent, t.accent),
            _ if takes => (if wired { color } else { t.panel }, t.accent),
            _ if from.is_some() => (if wired { color.with_alpha(0.3) } else { t.panel }, color.with_alpha(0.3)),
            _ => (if wired { color } else { t.panel }, color),
        };
        let words = if self.listening { port_words(graph, end) } else { String::new() };
        let (place, nudge) = if end.out { ((Align::End, Align::Center), HIT / 2.0) } else { ((Align::Start, Align::Center), -HIT / 2.0) };
        let dot = self.el().key(if end.out { OUT_KEY } else { IN_KEY }).place(place).offset(nudge, 0.0).size(HIT, HIT).flow(Flow::Stack).align(Align::Center, Align::Center).sense().focusable().open();
        self.tell(&dot, Role::Button, None, &words);
        // (Tab stops at a node's ports once, and arrows cycle them.)
        let node_id = self.pan_zoom_views.graphs.open.as_ref().map_or(0, |open| part_id(open.inner_id, end.node as u64));
        self.rovers.push(Rover { group: node_id, id: dot.id, stop: false, selects: false });
        let size = if from.is_some() && (takes || from == Some(end)) { DOT + 2.0 } else { DOT };
        let high = if !end.out && node.takes_many(end.port) { size * 1.6 } else { size };
        self.el().size(size, high).radius(size / 2.0).fill(fill).border(ring, 2.0).show();
        self.close();
        if dot.hovered && from.is_none() {
            self.set_cursor(Cursor::Hand);
        }
        self.seeing(dot.id, end.node, if end.out { Part::Out(end.port) } else { Part::In(end.port) });
    }

    /// The wire being dragged, over the nodes: from its start to the port it would connect to,
    /// or to the pointer.
    fn show_wire_drawn<T>(&mut self, graph: &Graph<T>, open: &OpenGraph) {
        let Some(from) = open.from else { return };
        let (graphs, t) = (&self.pan_zoom_views.graphs, self.theme);
        let Some(a) = graphs.port_point(from) else { return };
        let keyed = graphs.keyed.is_some_and(|keyed| keyed.graph == open.id);
        let Some(b) = open.target.and_then(|to| graphs.port_point(to)).or(open.pointer.filter(|_| !keyed)) else { return };
        let node = &graph.nodes[from.node];
        let kind = if from.out { node.outputs[from.port].kind } else { node.inputs[from.port].kind };
        let color = if open.target.is_some() { t.accent } else { graph.tint(kind, t.bg, t.text_dim) };
        let (a, b) = if from.out { (a, b) } else { (b, a) };
        self.curves(&[a, b], color, 3.0f32.max(1.0) / open.scale.max(1e-6));
    }

    /// The rubber band being dragged on the background, over the nodes.
    fn show_selecting(&mut self, open: &OpenGraph) {
        let (Some((_, Doing::Select { from, .. })), Some(to)) = (&self.pan_zoom_views.graphs.doing, open.pointer) else { return };
        let (t, from) = (self.theme, *from);
        let spread = rect(from.x.min(to.x), from.y.min(to.y), (to.x - from.x).abs(), (to.y - from.y).abs());
        self.el().offset(spread.x, spread.y).size(spread.w, spread.h).fill(t.accent.with_alpha(0.12)).border(t.accent, 1.0 / open.scale.max(1e-6)).show();
    }

    /// The minimap in the view's bottom-right corner, where the graph does not all fit: every
    /// node, selected ones in the accent colour, and a frame around the view.
    fn show_map(&mut self, open: &mut OpenGraph) {
        let (t, all, visible, room) = (self.theme, open.all, open.visible, open.room);
        let wide = (room.w / 4.0).min(MAP);
        let high = wide * 0.66;
        open.state.map = None;
        let inside = all.x >= visible.x && all.y >= visible.y && all.right() <= visible.right() && all.bottom() <= visible.bottom();
        if all.is_empty() || inside || wide < 80.0 || room.h < 3.0 * high {
            return;
        }
        let world = all.union(visible);
        let smaller = ((wide - 8.0) / world.w).min((high - 8.0) / world.h);
        let corner = pt((wide - world.w * smaller) / 2.0, (high - world.h * smaller) / 2.0);
        let place = |r: Rect| rect(corner.x + (r.x - world.x) * smaller, corner.y + (r.y - world.y) * smaller, (r.w * smaller).max(1.0), (r.h * smaller).max(1.0));
        let (across, Graphs { cells, laid, .. }) = ((wide / CELL).ceil() as usize, &mut self.pan_zoom_views.graphs);
        cells.clear();
        cells.resize(across * (high / CELL).ceil() as usize, 0);
        for node in laid.iter() {
            mark(cells, across, place(node.at), 1 + node.selected as u8);
        }
        let [nodes, selected] = runs(cells, across);
        let map = self.el().key(MAP_KEY).place(Align::End).offset(-MAP_EDGE, -MAP_EDGE).size(wide, high).flow(Flow::Stack);
        map.fill(t.panel).border(t.line_strong, 1.0).radius(4.0).clip().sense().open();
        self.el().size(wide, high).shape(&nodes, t.text_faint).show();
        self.el().size(wide, high).shape(&selected, t.accent).show();
        let frame = place(visible);
        self.el().offset(frame.x, frame.y).size(frame.w, frame.h).border(t.accent, 1.5).radius(2.0).show();
        self.close();
        open.state.map = Some((world, smaller, corner));
    }
}

/// A wire's control points from `a` to `b`: it leaves `a` rightward and enters `b` from the
/// left.
fn bend(a: Point, b: Point) -> [Point; 4] {
    let reach = ((b.x - a.x).abs() / 2.0).max(REACH);
    [a, pt(a.x + reach, a.y), pt(b.x - reach, b.y), b]
}

/// The bounds of a wire through these control points (`bend`), `width` wide. (A Bézier lies
/// within its control points, and the middle two are level with the ends. Computed for every
/// wire every run: as few steps as possible.)
fn around([a, c, d, b]: [Point; 4], width: f32) -> Rect {
    let (left, right) = (a.x.min(d.x), b.x.max(c.x));
    let (top, bottom) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
    rect(left - width, top - width, right - left + 2.0 * width, bottom - top + 2.0 * width)
}

/// Marks the minimap cells a rectangle covers (at least one) with `with`, where they hold less.
/// The map is `across` cells wide.
fn mark(cells: &mut [u8], across: usize, r: Rect, with: u8) {
    let down = cells.len() / across.max(1);
    if across == 0 || down == 0 || !r.x.is_finite() || !r.y.is_finite() {
        return;
    }
    let (left, top) = (((r.x / CELL).max(0.0) as usize).min(across - 1), ((r.y / CELL).max(0.0) as usize).min(down - 1));
    let (right, bottom) = (((r.right() / CELL).ceil() as usize).clamp(left + 1, across), ((r.bottom() / CELL).ceil() as usize).clamp(top + 1, down));
    for row in top..bottom {
        cells[row * across + left..row * across + right].iter_mut().for_each(|cell| *cell = (*cell).max(with));
    }
}

/// The minimap cells as shapes: a rectangle per run of equal cells along a row, those marked 1
/// in the first shape and 2 in the second.
fn runs(cells: &[u8], across: usize) -> [Path; 2] {
    let mut shapes = [Path::new(), Path::new()];
    for (y, row) in cells.chunks(across.max(1)).enumerate() {
        let mut x = 0;
        while x < row.len() {
            let end = row[x..].iter().position(|cell| *cell != row[x]).map_or(row.len(), |run| x + run);
            if row[x] > 0 {
                shapes[row[x] as usize - 1].rect(x as f32 * CELL, y as f32 * CELL, (end - x) as f32 * CELL, CELL);
            }
            x = end;
        }
    }
    shapes
}

/// Moves the nodes, bends and frames of `was` (each with its start position) `by` from there.
/// Says whether any moved.
fn move_by<T>(graph: &mut Graph<T>, was: &[(usize, Point)], by: Point) -> bool {
    let mut moved = false;
    for (what, start) in was {
        let to = pt(start.x + by.x, start.y + by.y);
        moved |= place_of(graph, *what).is_some_and(|now| now != to);
        put(graph, *what, to);
    }
    moved
}

/// Where a moved thing (see `Doing::Move`) is: a node's top left, a bend, a frame's top left or
/// bottom right.
fn place_of<T>(graph: &Graph<T>, what: usize) -> Option<Point> {
    match what {
        frame if frame & CORNER != 0 => graph.frames.get(frame & !CORNER).map(|frame| pt(frame.rect.right(), frame.rect.bottom())),
        frame if frame & FRAMED != 0 => graph.frames.get(frame & !FRAMED).map(|frame| pt(frame.rect.x, frame.rect.y)),
        bend if bend & BENT != 0 => graph.reroutes.get(bend & !BENT).map(|bend| bend.at),
        node => graph.nodes.get(node).map(|node| node.position),
    }
}

/// Puts a moved thing at `to`. (A frame's corner keeps it no smaller than `LEAST`.)
fn put<T>(graph: &mut Graph<T>, what: usize, to: Point) {
    let sized = |frame: &mut Frame| (frame.rect.w, frame.rect.h) = ((to.x - frame.rect.x).max(LEAST.w), (to.y - frame.rect.y).max(LEAST.h));
    match what {
        frame if frame & CORNER != 0 => _ = graph.frames.get_mut(frame & !CORNER).map(sized),
        frame if frame & FRAMED != 0 => _ = graph.frames.get_mut(frame & !FRAMED).map(|frame| (frame.rect.x, frame.rect.y) = (to.x, to.y)),
        bend if bend & BENT != 0 => _ = graph.reroutes.get_mut(bend & !BENT).map(|bend| bend.at = to),
        node => _ = graph.nodes.get_mut(node).map(|node| node.position = to),
    }
}

/// Selects the nodes a rectangle from `from` to `to` touches, plus those of `already` (in
/// order), and no others. Says whether anything changed.
fn select_within<T>(graph: &mut Graph<T>, from: Point, to: Point, already: &[usize]) -> bool {
    let spread = rect(from.x.min(to.x), from.y.min(to.y), (to.x - from.x).abs(), (to.y - from.y).abs());
    graph.select_only(|n, node| touches(node.rect(), spread) || already.binary_search(&n).is_ok())
}

/// What a drag does with the pointer at `at`: what is moved goes `by` from its start, the
/// first one onto the grid, keeping an undo step first (once a drag moves anything: `saved`);
/// or the rubber band selects.
fn drag<T: Clone>(graph: &mut Graph<T>, doing: &mut Doing, at: Point, saved: &mut bool) -> Option<GraphAction> {
    let moved = match doing {
        Doing::Move { was, from, .. } => {
            let grabbed = was.first().map_or(*from, |(_, start)| *start);
            let by = snap_by(graph, grabbed, pt(at.x - from.x, at.y - from.y));
            keep_once(graph, saved, by);
            move_by(graph, was, by)
        }
        Doing::Select { from, already } => return select_within(graph, *from, at, already).then_some(GraphAction::Selected),
        Doing::Wire { .. } => false,
    };
    moved.then_some(GraphAction::Moved)
}

/// Keeps an undo step before a drag first moves something `by` (`saved` says it has been).
fn keep_once<T: Clone>(graph: &mut Graph<T>, saved: &mut bool, by: Point) {
    if !*saved && by != pt(0.0, 0.0) {
        graph.save_undo_step();
        *saved = true;
    }
}

/// Keeps an undo step before a move by key, unless the last step was one: keys pressed one after
/// another undo as one.
fn keep_keyed_move<T: Clone>(graph: &mut Graph<T>) {
    if !graph.keyed_move {
        graph.save_undo_step();
        graph.keyed_move = true;
    }
}

/// How far something at `start` moves when moved `by`: so that it lands on the grid.
fn snap_by<T>(graph: &Graph<T>, start: Point, by: Point) -> Point {
    let to = graph.snapped(pt(start.x + by.x, start.y + by.y));
    pt(to.x - start.x, to.y - start.y)
}

/// Frame `frame` (at `was`) and what it carries, at their starts: the nodes whose middle lies
/// in it, and the bends in it (see `Doing::Move`).
fn carried<T>(graph: &Graph<T>, frame: usize, was: Rect) -> Vec<(usize, Point)> {
    let mut all = vec![(FRAMED | frame, pt(was.x, was.y))];
    all.extend(graph.nodes.iter().enumerate().filter(|(_, node)| was.contains(node.middle())).map(|(n, node)| (n, node.position)));
    all.extend(graph.reroutes.iter().enumerate().filter(|(_, bend)| was.contains(bend.at)).map(|(b, bend)| (BENT | b, bend.at)));
    all
}

/// A drag that moves `was` (the first grabbed) from `from`.
fn moving(was: Vec<(usize, Point)>, from: Point) -> Doing {
    Doing::Move { was, from, alone: None }
}

/// Where a drag began, in content units.
fn doing_start<T>(graph: &Graph<T>, doing: &Doing) -> Option<Point> {
    match doing {
        Doing::Wire { from, .. } => port_spot(graph, (from.node, from.port), from.out),
        Doing::Move { from, .. } | Doing::Select { from, .. } => Some(*from),
    }
}

/// A press on an input: the last wire into it is pulled off, or a new one starts from it.
fn pull<T>(graph: &Graph<T>, end: End) -> Doing {
    match graph.wires_into((end.node, end.port)).last() {
        Some(wire) => Doing::Wire { from: End { node: wire.from.0, port: wire.from.1, out: true }, taken: Some(wire) },
        None => Doing::Wire { from: end, taken: None },
    }
}

/// Whether a wire's ports are among `laid`'s.
fn fits(laid: &[Laid], wire: &Wire) -> bool {
    let (Some(from), Some(to)) = (laid.get(wire.from.0), laid.get(wire.to.0)) else { return false };
    wire.from.1 < from.outs && wire.to.1 < to.ins
}

/// A port's dot centre in content units, from the node itself (for presses: the run's `Laid`
/// is not there yet).
fn port_spot<T>(graph: &Graph<T>, (node, port): (usize, usize), out: bool) -> Option<Point> {
    let node = graph.nodes.get(node)?;
    let r = node.rect();
    (port < if out { node.outputs.len() } else { node.inputs.len() }).then(|| pt(if out { r.right() } else { r.x }, r.y + TOP + (port as f32 + 0.5) * ROW))
}

/// Puts a wire's points in `points`: its output's dot, its bends, its input's dot. Says whether
/// both ports are there.
fn wire_points<T>(graph: &Graph<T>, wire: Wire, points: &mut Vec<Point>) -> bool {
    let (Some(a), Some(b)) = (port_spot(graph, wire.from, true), port_spot(graph, wire.to, false)) else { return false };
    points.clear();
    points.push(a);
    points.extend(graph.reroutes.iter().filter(|bend| bend.wire == wire).map(|bend| bend.at));
    points.push(b);
    true
}

/// The wire within `reach` of `at` (the last drawn, on top, first), by index, and which of its
/// pieces (between bends).
fn wire_near<T>(graph: &Graph<T>, at: Point, reach: f32) -> Option<(usize, usize)> {
    let mut points = Vec::new();
    (0..graph.wires.len()).rev().find_map(|index| wire_points(graph, graph.wires[index], &mut points).then(|| piece_near(&points, at, reach)).flatten().map(|piece| (index, piece)))
}

/// Which piece of a wire through `points` passes within `reach` of `at`.
fn piece_near(points: &[Point], at: Point, reach: f32) -> Option<usize> {
    let close = |curve: [Point; 4], k: usize| near_line(on_curve(curve, k as f32 / SAMPLES as f32), on_curve(curve, (k + 1) as f32 / SAMPLES as f32), at) <= reach;
    points.windows(2).map(|pair| bend(pair[0], pair[1])).position(|curve| around(curve, reach).contains(at) && (0..SAMPLES).any(|k| close(curve, k)))
}

/// How far `p` is from the line from `a` to `b`.
fn near_line(a: Point, b: Point, p: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let along = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
    ((a.x + along * dx - p.x).powi(2) + (a.y + along * dy - p.y).powi(2)).sqrt()
}

/// The point a share `t` along a curve through these control points.
fn on_curve([a, c, d, b]: [Point; 4], t: f32) -> Point {
    let s = 1.0 - t;
    let [wa, wc, wd, wb] = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    pt(a.x * wa + c.x * wc + d.x * wd + b.x * wb, a.y * wa + c.y * wc + d.y * wd + b.y * wb)
}

/// The bounds of a wire through these points (each piece a `bend`), `width` wide.
fn chain_around(points: &[Point], width: f32) -> Rect {
    points.windows(2).fold(Rect::EMPTY, |all, pair| all.union(around(bend(pair[0], pair[1]), width)))
}

/// Which way, and how hard (0 to 1), the pointer at `p` pushes a view from `low` to `high`
/// along one axis: within `EDGE` of an edge (a quarter of the view at most), or past it.
fn push(p: f32, low: f32, high: f32) -> f32 {
    let near = EDGE.min((high - low) / 4.0);
    if near <= 0.0 {
        return 0.0;
    }
    ((p - (high - near)).max(0.0) - (low + near - p).max(0.0)).clamp(-near, near) / near
}

/// What a wire from `from` would connect to over `hit`: a port of the opposite side on another
/// node; on a node, its first opposite port that accepts it (unwired ones first, then those
/// that take several).
fn target<T>(graph: &Graph<T>, from: End, (node, part): (usize, Part)) -> Option<End> {
    let ports = |out: bool| graph.nodes.get(node).map_or(0, |node| if out { node.outputs.len() } else { node.inputs.len() });
    let end = match part {
        Part::In(port) => Some(End { node, port, out: false }),
        Part::Out(port) => Some(End { node, port, out: true }),
        Part::Ground | Part::Title => {
            let fitting = (0..ports(!from.out)).map(|port| End { node, port, out: !from.out }).filter(|to| graph.takes(from, *to));
            let free = fitting.clone().find(|to| !to.out && graph.wire_into((to.node, to.port)).is_none());
            let many = fitting.clone().find(|to| !to.out && graph.nodes[to.node].takes_many(to.port));
            free.or(many).or(fitting.clone().next())
        }
        _ => None,
    };
    end.filter(|end| end.node != from.node && end.out != from.out)
}

/// The next port a wire from `from` could connect to after `now` (before it, when `forward` is
/// false), in node and port order, wrapping; from the node after `from`'s when none yet.
fn step<T>(graph: &Graph<T>, from: End, now: Option<End>, forward: bool) -> Option<End> {
    let count = |node: &Node<T>| if from.out { node.inputs.len() } else { node.outputs.len() };
    let all = graph.nodes.iter().enumerate().flat_map(|(n, node)| (0..count(node)).map(move |port| End { node: n, port, out: !from.out }));
    let fitting = all.filter(|to| graph.takes(from, *to));
    let here = now.map_or((from.node, if forward { usize::MAX } else { 0 }), |now| (now.node, now.port));
    let next = if forward { fitting.clone().find(|to| (to.node, to.port) > here) } else { fitting.clone().filter(|to| (to.node, to.port) < here).last() };
    next.or_else(|| if forward { fitting.clone().next() } else { fitting.clone().last() })
}

/// A port dot's id in the graph with view `id`.
fn port_id(id: Id, end: End) -> Id {
    part_id(part_id(node_id(id, end.node), ROW_KEY + end.port as u64), if end.out { OUT_KEY } else { IN_KEY })
}

/// A node's box id in the graph with view `id`.
fn node_id(id: Id, node: usize) -> Id {
    part_id(part_id(id, INNER), node as u64)
}

/// Pans the view to show the node or frame focus entered, where it came by key (`keyed`, the
/// focus then) and is not wholly visible: as little as needed, with a margin.
fn pan_to<T>(graph: &mut Graph<T>, state: &mut GraphState, room: Size, scale: f32, keyed: Option<Id>) {
    let target = keyed.and_then(|focus| state.frame(focus)).map(|frame| !frame).or(state.focus);
    if target == state.shown_for {
        return;
    }
    state.shown_for = target;
    let shown = match target {
        Some(frame) if frame > usize::MAX / 2 => graph.frames.get(!frame).map(|frame| frame.rect),
        Some(node) => graph.nodes.get(node).map(Node::rect),
        None => None,
    };
    let Some(shown) = shown.filter(|_| keyed.is_some()) else { return };
    let (want, visible) = (shown.grow(MARGIN), rect(graph.view.x, graph.view.y, room.w / scale, room.h / scale));
    // (Far edge into view first, then near: where the node is larger than the view, its corner
    // shows.)
    let along = |from: f32, long: f32, near: f32, far: f32| if far > from + long { far - long } else { from }.min(near);
    (graph.view.x, graph.view.y) = (along(visible.x, visible.w, want.x, want.right()), along(visible.y, visible.h, want.y, want.bottom()));
}

/// A port's screen reader description: name, kind and direction, and its connections.
fn port_words<T>(graph: &Graph<T>, end: End) -> String {
    let (node, words) = (&graph.nodes[end.node], graph.words);
    let port = if end.out { &node.outputs[end.port] } else { &node.inputs[end.port] };
    let at = (end.node, end.port);
    let ends: Vec<String> =
        graph.wires.iter().filter(|wire| if end.out { wire.from == at } else { wire.to == at }).map(|wire| end_words(graph, if end.out { wire.to } else { wire.from }, !end.out)).collect();
    let way = if end.out { words.output } else { words.input };
    let way = if !end.out && node.takes_many(end.port) { format!("{way}, {}", words.many) } else { way.to_string() };
    match ends.is_empty() {
        true => format!("{}, {} {way}, {}", port.name, port.kind, words.alone),
        false => format!("{}, {} {way}, {} {}", port.name, port.kind, if end.out { words.to } else { words.from }, ends.join(", ")),
    }
}

/// A port as named among others: its node's title and its name.
fn end_words<T>(graph: &Graph<T>, (node, port): (usize, usize), out: bool) -> String {
    let Some(node) = graph.nodes.get(node) else { return String::new() };
    let name = if out { node.outputs.get(port) } else { node.inputs.get(port) }.map_or("", |port| port.name.as_str());
    format!("{}: {name}", node.title)
}

/// A wire as a screen reader hears it: from where, into where.
fn wire_words<T>(graph: &Graph<T>, wire: Wire) -> String {
    format!("{} {} {}", end_words(graph, wire.from, true), graph.words.to, end_words(graph, wire.to, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three nodes: two numbers, and a sum of two.
    fn sums() -> Graph<u32> {
        let mut graph = Graph::new();
        graph.add(Node::new("One", 0.0, 0.0, 1).output("Value", "number"));
        graph.add(Node::new("Two", 0.0, 100.0, 2).output("Value", "number").output("Name", "text"));
        graph.add(Node::new("Sum", 300.0, 50.0, 3).input("A", "number").input("B", "number").output("Sum", "number"));
        graph
    }

    #[test]
    fn a_wire_goes_only_where_it_fits_and_an_input_takes_one() {
        let mut graph = sums();
        assert!(graph.wire((0, 0), (2, 0)));
        assert!(!graph.wire((1, 1), (2, 1)), "text into a number does not fit");
        assert!(!graph.wire((2, 0), (2, 1)), "nor does a node into itself");
        assert!(!graph.wire((0, 3), (2, 1)) && !graph.wire((0, 0), (5, 0)), "nor what is not there");
        assert!(graph.wire((1, 0), (2, 0)), "a second wire into A takes the place of the first");
        assert_eq!(graph.wires, [Wire { from: (1, 0), to: (2, 0) }]);
        graph.can_connect = |from, into| from == into || from == "text";
        assert!(graph.wire((1, 1), (2, 1)), "the app says text fits as well");
        assert_eq!(graph.wire_into((2, 1)), Some(Wire { from: (1, 1), to: (2, 1) }));
    }

    #[test]
    fn a_node_taken_away_takes_its_wires_and_the_rest_are_renumbered() {
        let mut graph = sums();
        graph.wire((0, 0), (2, 0));
        graph.wire((1, 0), (2, 1));
        graph.remove(0);
        assert_eq!(graph.nodes.iter().map(|node| node.title.as_str()).collect::<Vec<_>>(), ["Two", "Sum"]);
        assert_eq!(graph.wires, [Wire { from: (0, 0), to: (1, 1) }]);
    }

    #[test]
    fn copies_keep_the_wires_between_them_and_lie_beside_or_at_a_point() {
        let mut graph = sums();
        graph.wire((0, 0), (2, 0));
        graph.wire((1, 0), (2, 1));
        // The first and the sum: the wire between them is copied, the one from the second is
        // not.
        (graph.nodes[0].selected, graph.nodes[2].selected) = (true, true);
        assert_eq!(graph.copy_selected(), 2);
        graph.paste(None);
        assert_eq!(graph.nodes.len(), 5);
        assert_eq!(graph.wires[2], Wire { from: (3, 0), to: (4, 0) });
        assert_eq!(graph.wires.len(), 3);
        assert_eq!((graph.nodes[3].position, graph.nodes[4].position), (pt(ASIDE, ASIDE), pt(300.0 + ASIDE, 50.0 + ASIDE)));
        assert!(graph.nodes.iter().map(|node| node.selected).eq([false, false, false, true, true]), "only the copies are picked");
        graph.paste(Some(pt(1000.0, 1000.0)));
        assert_eq!((graph.nodes[5].position, graph.nodes[6].position), (pt(1000.0, 1000.0), pt(1300.0, 1050.0)));
    }

    #[test]
    fn the_keyboard_goes_round_the_ports_a_wire_could_go_to() {
        let mut graph = sums();
        graph.add(Node::new("Show", 600.0, 0.0, 4).input("Value", "number").input("Label", "text"));
        let from = End { node: 2, port: 0, out: true };
        // From the sum's output: the inputs of the nodes after it first, then wrapping to the
        // start.
        let first = step(&graph, from, None, true);
        assert_eq!(first, Some(End { node: 3, port: 0, out: false }));
        assert_eq!(step(&graph, from, first, true), first, "the text input of Show does not fit, and the sum's own do not count");
        assert_eq!(step(&graph, End { node: 0, port: 0, out: true }, None, true), Some(End { node: 2, port: 0, out: false }));
        let back = step(&graph, End { node: 0, port: 0, out: true }, None, false);
        assert_eq!(back, Some(End { node: 3, port: 0, out: false }), "back from the first node comes round to the last");
        // From an input: the outputs that fit.
        let into = End { node: 3, port: 1, out: false };
        assert_eq!(step(&graph, into, None, true), Some(End { node: 1, port: 1, out: true }));
    }

    #[test]
    fn a_port_tells_what_it_is_wired_to() {
        let mut graph = sums();
        graph.wire((0, 0), (2, 0));
        graph.wire((0, 0), (2, 1));
        assert_eq!(port_words(&graph, End { node: 2, port: 0, out: false }), "A, number in, from One: Value");
        assert_eq!(port_words(&graph, End { node: 0, port: 0, out: true }), "Value, number out, to Sum: A, Sum: B");
        assert_eq!(port_words(&graph, End { node: 2, port: 0, out: true }), "Sum, number out, not wired");
    }

    #[test]
    fn bends_go_with_their_wires_through_removals_raises_and_copies() {
        let mut graph = sums();
        graph.wire((0, 0), (2, 0));
        graph.wire((1, 0), (2, 1));
        let [a, b] = [graph.wires[0], graph.wires[1]];
        // In order along a wire: one added in its first piece goes before the rest.
        assert_eq!(graph.add_bend(a, 0, pt(100.0, 10.0)), 0);
        assert_eq!(graph.add_bend(b, 0, pt(100.0, 120.0)), 1);
        assert_eq!(graph.add_bend(a, 0, pt(50.0, 5.0)), 0);
        assert_eq!(graph.add_bend(a, 2, pt(200.0, 30.0)), 2);
        assert_eq!(graph.reroutes.iter().map(|bend| bend.at.x).collect::<Vec<_>>(), [50.0, 100.0, 200.0, 100.0]);
        // A node under another, raised: wires and bends renumbered.
        graph.nodes[1].position = pt(10.0, 10.0);
        assert_eq!(graph.raise(0), 2);
        assert_eq!(graph.wires, [Wire { from: (2, 0), to: (1, 0) }, Wire { from: (0, 0), to: (1, 1) }]);
        assert!(graph.reroutes.iter().all(|bend| graph.wires.contains(&bend.wire)));
        assert_eq!(graph.raise(1), 1, "the sum lies under nothing");
        // Copied with both ends: the bends come too.
        (graph.nodes[0].selected, graph.nodes[1].selected) = (true, true);
        graph.copy_selected();
        graph.paste(Some(pt(1000.0, 1000.0)));
        assert_eq!(graph.reroutes.len(), 5);
        assert_eq!(graph.reroutes[4].wire, Wire { from: (3, 0), to: (4, 1) });
        // Removed with its wire; with its node.
        assert!(graph.unwire(Wire { from: (2, 0), to: (1, 0) }));
        assert_eq!(graph.reroutes.len(), 2);
        graph.remove(0);
        assert_eq!((graph.wires.as_slice(), graph.reroutes.len()), ([Wire { from: (2, 0), to: (3, 1) }].as_slice(), 1));
    }

    #[test]
    fn an_input_for_several_keeps_each_wire_once_and_a_wire_replacing_one_takes_its_bends() {
        let mut graph = sums();
        graph.nodes[2] = Node::new("Sum", 300.0, 50.0, 3).multi_input("All", "number").input("B", "number");
        assert!(graph.nodes[2].takes_many(0) && !graph.nodes[2].takes_many(1) && !graph.nodes[2].takes_many(64));
        assert!(graph.wire((0, 0), (2, 0)) && graph.wire((1, 0), (2, 0)) && graph.wire((0, 0), (2, 0)));
        assert_eq!(graph.wires_into((2, 0)).count(), 2);
        graph.wire((0, 0), (2, 1));
        graph.add_bend(Wire { from: (0, 0), to: (2, 1) }, 0, pt(200.0, 200.0));
        assert!(graph.wire((1, 0), (2, 1)));
        assert!(graph.reroutes.is_empty(), "the wire it replaced took its bend away");
    }

    #[test]
    fn undo_keeps_a_bounded_history_and_redo_goes_until_a_new_edit() {
        let mut graph = sums();
        assert!(!graph.undo() && !graph.redo());
        for step in 0..STEPS + 5 {
            graph.save_undo_step();
            graph.nodes[0].position.x = step as f32 + 1.0;
        }
        assert_eq!(graph.steps.len(), STEPS);
        assert!(graph.undo() && graph.undo());
        assert_eq!(graph.nodes[0].position.x, STEPS as f32 + 3.0);
        assert!(graph.redo() && graph.can_redo());
        graph.save_undo_step();
        assert!(!graph.can_redo());
        // Large graphs keep fewer steps, never none.
        let mut large = Graph::new();
        (0..KEPT / 2 + 1).for_each(|n| _ = large.add(Node::new("", n as f32, 0.0, 0)));
        large.save_undo_step();
        large.save_undo_step();
        assert_eq!(large.steps.len(), 1);
    }

    #[test]
    fn the_edge_pushes_harder_nearer_and_past_it_and_not_at_all_in_the_middle() {
        assert_eq!([push(50.0, 0.0, 400.0), push(16.0, 0.0, 400.0), push(-50.0, 0.0, 400.0), push(392.0, 0.0, 400.0)], [0.0, -0.5, -1.0, 0.75]);
        assert_eq!(push(5.0, 0.0, 40.0), -0.5, "a small view's edge is a quarter of it");
        assert_eq!(push(0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn a_wire_is_found_near_any_of_its_pieces() {
        let points = [pt(0.0, 0.0), pt(200.0, 100.0), pt(400.0, 0.0)];
        let middle = on_curve(bend(points[1], points[2]), 0.5);
        assert_eq!(piece_near(&points, pt(middle.x, middle.y + 4.0), 6.0), Some(1));
        assert_eq!(piece_near(&points, pt(middle.x, middle.y + 9.0), 6.0), None);
        assert_eq!(piece_near(&points, on_curve(bend(points[0], points[1]), 0.3), 1.0), Some(0));
    }
}
