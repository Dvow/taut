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
//! takes no wire. An input takes one wire: a new one replaces the old, and a wire pulled off an
//! input and dropped on nothing is deleted. Nodes are selected by click (Control toggles one,
//! Shift adds one) and by a rectangle dragged on the background; selected, they move together,
//! delete with Delete, and copy (Control+C, X, V; Control+D duplicates beside them).
//! Right-click asks the app for its menu ([`GraphAction::Menu`]), with the content point, to
//! place a new node there.
//!
//! The keyboard reaches everything. Tab goes from node to node in `nodes` order: a node's
//! title, its ports once (Left and Right cycle them), and the app's parts in it; the view
//! follows to show it. On a title, arrows move the node (and the rest of the selection), Space
//! selects it, Delete removes it. On a port, Enter starts a wire: arrows then cycle the ports
//! it could connect to, the view showing each, Enter connects and Escape cancels; Delete
//! removes the port's wires. Screen readers hear each node as a group named by its title, its
//! title as a selectable row, and each port as a button named by its kind and connections.
//!
//! Only the nodes and wires in view are described and laid out, however many there are (plus
//! the focused one and its neighbours, so Tab can continue). Where the graph does not all fit,
//! a minimap of it sits in a corner with the view marked: pressing or dragging on it pans the
//! view there.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Insets, Point, Rect, Size, pt, rect};
use crate::id::{ById, Id};
use crate::layout::{Align, Flow, Length, NONE};
use crate::pan_zoom::{INNER, PanZoom, part_id};
use crate::path::Path;
use crate::ui::{Measure, Response, Rover, Ui};

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

/// A node graph the user wires up, as the app keeps it: nodes, wires, and its pan and zoom.
#[derive(Clone, Debug)]
pub struct Graph<T> {
    /// The nodes, in screen reader and Tab order. Later ones lie over earlier ones. (Wires name
    /// nodes by index: remove one with `remove`, which renumbers.)
    pub nodes: Vec<Node<T>>,
    /// The wires, each from one node's output into another's input. An input has at most one;
    /// an output, any number.
    pub wires: Vec<Wire>,
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
    /// The last copied nodes, with the wires between them (by index among these).
    copied: Vec<Node<T>>,
    copied_wires: Vec<Wire>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The selection changed.
    Selected,
    /// The selected nodes moved.
    Moved,
    /// This wire was made (replacing any the input had).
    Connected(Wire),
    /// This wire was removed.
    Disconnected(Wire),
    /// The selected nodes were deleted with their wires: later nodes moved up in the list.
    Deleted,
    /// Copies of the last copied nodes were appended and selected.
    Pasted,
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
            copied: Vec::new(),
            copied_wires: Vec::new(),
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
    /// fit: any wire into `to` is removed. Says whether it was wired.
    pub fn wire(&mut self, from: (usize, usize), to: (usize, usize)) -> bool {
        let (Some(out), Some(into)) = (self.port(from, true), self.port(to, false)) else { return false };
        if from.0 == to.0 || !(self.can_connect)(out.kind, into.kind) {
            return false;
        }
        self.wires.retain(|wire| wire.to != to);
        self.wires.push(Wire { from, to });
        true
    }
    /// The wire into input `to` (node, input), if any.
    pub fn wire_into(&self, to: (usize, usize)) -> Option<Wire> {
        self.wires.iter().find(|wire| wire.to == to).copied()
    }
    /// Removes node `node` with its wires; later nodes move up and wires are renumbered.
    pub fn remove(&mut self, node: usize) {
        self.keep_only(|at, _| at != node);
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
        let to = at.unwrap_or(pt(around.x + ASIDE, around.y + ASIDE));
        let (first, shift) = (self.nodes.len(), pt(to.x - around.x, to.y - around.y));
        self.nodes.iter_mut().for_each(|node| node.selected = false);
        self.nodes.extend(self.copied.iter().cloned().map(|node| Node { position: pt(node.position.x + shift.x, node.position.y + shift.y), selected: true, ..node }));
        let wires = self.copied_wires.iter().map(|wire| Wire { from: (wire.from.0 + first, wire.from.1), to: (wire.to.0 + first, wire.to.1) });
        self.wires.extend(wires);
        // (The next paste lies a little further on, as copies of copies do.)
        self.copied.iter_mut().for_each(|node| node.position = pt(node.position.x + shift.x, node.position.y + shift.y));
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
        Node { title: title.into(), position: pt(x, y), width: WIDTH, inputs: Vec::new(), outputs: Vec::new(), selected: false, data, high: 0.0, laid: [0.0; 3] }
    }
    /// With one more input, below the others: its name and kind.
    pub fn input(mut self, name: impl Into<String>, kind: &'static str) -> Self {
        self.inputs.push(Port { name: name.into(), kind });
        self
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
}

/// What the pointer is doing to a graph.
#[derive(Clone, Debug, PartialEq)]
enum Doing {
    /// Dragging a wire from this end (the other follows the pointer); `taken`, the wire it was
    /// pulled off an input as, deleted if dropped on nothing.
    Wire { from: End, taken: Option<Wire> },
    /// Moving the selected nodes: each with its start position, from the grab point; and the
    /// pressed node, selected alone if released without moving (it was among several selected).
    Move { was: Vec<(usize, Point)>, from: Point, alone: Option<usize> },
    /// Rubber-band selecting from this point to the pointer, plus these (Control or Shift was
    /// held).
    Select { from: Point, already: Vec<usize> },
}

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
    /// The node focus was in, last pass and this one; and the node the view last panned to.
    focus: Option<usize>,
    focusing: Option<usize>,
    shown_for: Option<usize>,
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
    /// For the graph being described: each node as the run passes it, and each port's wire
    /// count (each node's inputs then outputs, node after node).
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
}

/// Whether two rectangles overlap or touch.
fn touches(a: Rect, b: Rect) -> bool {
    a.x <= b.right() && a.right() >= b.x && a.y <= b.bottom() && a.bottom() >= b.y
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

    /// Starts a graph: takes in user actions, opens its view, and describes the visible wires.
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
        if !graph.nodes.is_empty() {
            self.in_view(all);
        }
        state.scale = scale;
        let focus = state.focus.unwrap_or(if self.focus == id { 0 } else { usize::MAX });
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
        self.show_wires(graph);
    }

    /// The state graph `id` kept from the last pass, ready for this one: its focused node, and
    /// the heights of the nodes last laid out (read into `graph`).
    fn graph_state<T>(&mut self, graph: &mut Graph<T>, id: Id) -> GraphState {
        let mut state = self.pan_zoom_views.graphs.states.remove(&id).unwrap_or_default();
        std::mem::swap(&mut state.seen, &mut state.seeing);
        state.seeing.clear();
        // The focused node: the one whose title or port has focus now (it may just have arrived
        // by Tab), else the one focus was in last pass.
        let on = state.seen.iter().find(|(seen, ..)| *seen == self.focus).map(|(_, node, _)| *node);
        (state.focus, state.focusing, state.sent) = (on.or(state.focusing.take()), None, None);
        for (box_id, node, _) in state.seen.iter().filter(|(_, _, part)| *part == Part::Ground) {
            let (Some(read), Some(node)) = (self.rooms.get(box_id).copied(), graph.nodes.get_mut(*node)) else { continue };
            self.rooms.remove(box_id);
            (node.laid, node.high) = (read, read[1] / state.scale.max(1e-6));
        }
        state
    }

    /// Walks the nodes once for the run (`Laid`) and counts each port's wires, so ports show
    /// whether they have any. Returns the bounds of all nodes.
    fn lay_out<T>(&mut self, graph: &Graph<T>) -> Rect {
        let Graphs { laid, wired, .. } = &mut self.pan_zoom_views.graphs;
        laid.clear();
        let (mut ports, mut all) = (0, Rect::EMPTY);
        for node in &graph.nodes {
            let (at, ins, outs) = (node.rect(), node.inputs.len(), node.outputs.len());
            laid.push(Laid { at, first: ports, ins, outs, selected: node.selected });
            (ports, all) = (ports + ins + outs, all.union(at));
        }
        wired.clear();
        wired.resize(ports, 0);
        for wire in &graph.wires {
            let (Some(from), Some(to)) = (laid.get(wire.from.0), laid.get(wire.to.0)) else { continue };
            if wire.from.1 < from.outs && wire.to.1 < to.ins {
                wired[from.first + from.ins + wire.from.1] += 1;
                wired[to.first + wire.to.1] += 1;
            }
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
        let at = self.input.pointer.map(|p| pt(graph.view.x + (p.x - ground.rect.x) / scale, graph.view.y + (p.y - ground.rect.y) / scale));
        let hit = state.seen.iter().find(|(id, ..)| *id == self.hot).map(|(_, node, part)| (*node, *part));
        self.steer_map(graph, ground.id, state, room, scale);
        let doing = self.pan_zoom_views.graphs.doing.as_ref().is_some_and(|(graph, _)| *graph == ground.id);
        if let (true, Some(at)) = (self.input.pressed[0] && !doing, at) {
            self.press_graph(graph, ground, hit, at);
            // (A pressed node gets focus on its title: described whole, however small.)
            state.sent = hit.map(|(node, _)| node).or(state.sent);
        }
        if let (true, Some(at)) = (self.input.pressed[1], at) {
            self.ask_menu(graph, ground, hit, at);
        }
        match (self.input.down[0], at) {
            (true, Some(at)) => self.drag_graph(graph, ground.id, at),
            (false, _) => self.let_go(graph, ground.id, hit, at),
            _ => {}
        }
        self.graph_keys(graph, state, ground.id, at.filter(|_| ground.contains_pointer));
        let keys = self.focus_visible();
        pan_to(graph, state, room, scale, keys);
    }

    /// Left button down on the graph: on a port a wire starts; on a node it is selected and
    /// starts moving with the selection; on the background a rubber band starts.
    fn press_graph<T>(&mut self, graph: &mut Graph<T>, ground: &Response, hit: Option<(usize, Part)>, at: Point) {
        let mods = self.input.mods;
        let doing = match hit {
            Some((node, Part::Out(port))) => Some(Doing::Wire { from: End { node, port, out: true }, taken: None }),
            Some((node, Part::In(port))) => Some(match graph.wire_into((node, port)) {
                Some(wire) => Doing::Wire { from: End { node: wire.from.0, port: wire.from.1, out: true }, taken: Some(wire) },
                None => Doing::Wire { from: End { node, port, out: false }, taken: None },
            }),
            Some((node, _)) => self.press_node(graph, ground.id, node, at, mods),
            None if ground.hovered => Some(Doing::Select { from: at, already: (0..graph.nodes.len()).filter(|n| (mods.ctrl || mods.shift) && graph.nodes[*n].selected).collect() }),
            None => None,
        };
        self.pan_zoom_views.graphs.doing = doing.map(|doing| (ground.id, doing));
    }

    /// A node of the graph with view `id` was pressed at `at` with `mods`: selected (alone;
    /// Control toggles; Shift adds), focus to its title. Returns what follows: the selection
    /// drags with the pointer, if selected.
    fn press_node<T>(&mut self, graph: &mut Graph<T>, id: Id, node: usize, at: Point, mods: Mods) -> Option<Doing> {
        let was = graph.nodes.get(node)?.selected;
        let changed = match (mods.ctrl, mods.shift || was) {
            (true, _) => graph.select_only(|n, other| if n == node { !was } else { other.selected }),
            (false, true) => graph.select_only(|n, other| n == node || other.selected),
            (false, false) => graph.select_only(|n, _| n == node),
        };
        if changed {
            graph.action = GraphAction::Selected;
        }
        let title = part_id(part_id(part_id(id, INNER), node as u64), TITLE_KEY);
        if self.focus != title {
            self.set_focus(title);
        }
        let moving = graph.nodes.iter().enumerate().filter(|(_, node)| node.selected).map(|(n, node)| (n, node.position)).collect();
        graph.nodes[node].selected.then_some(Doing::Move { was: moving, from: at, alone: (was && !mods.ctrl && !mods.shift).then_some(node) })
    }

    /// Right button down on the graph: on a node, selected if not already; or on the
    /// background. The app hears it, for its menu.
    fn ask_menu<T>(&mut self, graph: &mut Graph<T>, ground: &Response, hit: Option<(usize, Part)>, at: Point) {
        let node = hit.map(|(node, _)| node).filter(|node| *node < graph.nodes.len());
        if node.is_none() && !ground.hovered {
            return;
        }
        if let Some(node) = node.filter(|node| !graph.nodes[*node].selected) {
            graph.select_only(|n, _| n == node);
        }
        (graph.action, self.again) = (GraphAction::Menu { node, at }, true);
    }

    /// While the left button is held: the selected nodes follow the pointer, or the rubber band
    /// does and selects what it touches.
    fn drag_graph<T>(&mut self, graph: &mut Graph<T>, id: Id, at: Point) {
        let Some((_, doing)) = self.pan_zoom_views.graphs.doing.as_ref().filter(|(graph, _)| *graph == id) else { return };
        let did = match doing {
            Doing::Move { was, from, .. } => move_by(graph, was, pt(at.x - from.x, at.y - from.y)).then_some(GraphAction::Moved),
            Doing::Select { from, already } => select_within(graph, *from, at, already).then_some(GraphAction::Selected),
            Doing::Wire { .. } => None,
        };
        if matches!(doing, Doing::Move { .. }) {
            self.set_cursor(Cursor::Move);
        }
        graph.action = did.unwrap_or(graph.action);
    }

    /// Left button up: a wire connects where dropped (or one pulled off an input is deleted); a
    /// node pressed among several selected and not moved is selected alone; a background press
    /// without a rubber band clears the selection.
    fn let_go<T>(&mut self, graph: &mut Graph<T>, id: Id, hit: Option<(usize, Part)>, at: Option<Point>) {
        let Some(doing) = self.pan_zoom_views.graphs.doing.take_if(|(graph, _)| *graph == id).map(|(_, doing)| doing) else { return };
        let moved = |was: &[(usize, Point)]| was.iter().any(|(node, start)| graph.nodes.get(*node).is_some_and(|node| node.position != *start));
        let selection_changed = match doing {
            Doing::Move { was, alone: Some(node), .. } if !moved(&was) => graph.select_only(|n, _| n == node),
            Doing::Select { from, already } if at == Some(from) && already.is_empty() => graph.select_only(|_, _| false),
            Doing::Wire { from, taken } => {
                self.drop_wire(graph, from, taken, hit);
                false
            }
            _ => false,
        };
        if selection_changed {
            graph.action = GraphAction::Selected;
        }
    }

    /// A wire from `from` was dropped over `hit`: connected where that is a port (or a node
    /// with a port) that accepts it; and the wire it was pulled off an input as, if any, is
    /// deleted.
    fn drop_wire<T>(&mut self, graph: &mut Graph<T>, from: End, taken: Option<Wire>, hit: Option<(usize, Part)>) {
        let made = hit.and_then(|hit| target(graph, from, hit)).filter(|to| graph.takes(from, *to)).map(|to| from.wire(to));
        if made.is_some() && made == taken {
            return;
        }
        if let Some(old) = taken {
            graph.wires.retain(|wire| *wire != old);
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

    /// Keys while focus is in the view, a title or a port. `at` is the pointer's content
    /// position while over it.
    fn graph_keys<T: Clone>(&mut self, graph: &mut Graph<T>, state: &mut GraphState, id: Id, at: Option<Point>) {
        if self.pan_zoom_views.graphs.keyed.is_some_and(|keyed| keyed.graph == id) {
            state.sent = self.keyed_keys(graph, id);
            return;
        }
        let on = state.seen.iter().find(|(seen, ..)| *seen == self.focus).map(|(_, node, part)| (*node, *part)).filter(|(node, _)| *node < graph.nodes.len());
        if self.focus != id && on.is_none() {
            return;
        }
        match on {
            Some((node, Part::Title)) => self.title_keys(graph, node),
            Some((node, Part::In(port))) => self.port_keys(graph, id, End { node, port, out: false }),
            Some((node, Part::Out(port))) => self.port_keys(graph, id, End { node, port, out: true }),
            _ => {}
        }
        self.selection_keys(graph, at, !matches!(on, Some((_, Part::In(_) | Part::Out(_)))));
    }

    /// Keys on a title: arrows move the node, with the selection if selected; Space selects it
    /// (Control toggles; Shift adds); Delete removes it, with the selection if selected.
    fn title_keys<T>(&mut self, graph: &mut Graph<T>, node: usize) {
        let arrows = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| self.key(key, Mods::NONE) as i32 as f32);
        let step = pt(NUDGE * (arrows[1] - arrows[0]), NUDGE * (arrows[3] - arrows[2]));
        if step != pt(0.0, 0.0) {
            let was_selected = graph.nodes[node].selected;
            for other in graph.nodes.iter_mut().enumerate().filter(|(n, other)| if was_selected { other.selected } else { *n == node }).map(|(_, other)| other) {
                other.position = pt(other.position.x + step.x, other.position.y + step.y);
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
            graph.action = GraphAction::Selected;
        }
        // (An unselected node is removed alone.)
        if !graph.nodes[node].selected && [Key::Delete, Key::Backspace].iter().any(|key| self.key(*key, Mods::NONE)) {
            graph.select_only(|n, _| n == node);
            self.delete_selected(graph);
        }
    }

    /// Keys on a port: Enter or Space starts a keyboard wire; Delete removes its wires.
    fn port_keys<T>(&mut self, graph: &mut Graph<T>, id: Id, end: End) {
        if self.key(Key::Enter, Mods::NONE) || self.key(Key::Space, Mods::NONE) {
            self.pan_zoom_views.graphs.keyed = Some(KeyboardWire { graph: id, from: end, to: None });
            self.announce(graph.words.wiring);
            return;
        }
        if ![Key::Delete, Key::Backspace].iter().any(|key| self.key(*key, Mods::NONE)) {
            return;
        }
        let at = (end.node, end.port);
        let mine = |wire: &Wire| if end.out { wire.from == at } else { wire.to == at };
        if let Some(wire) = graph.wires.iter().rev().find(|wire| mine(wire)).copied() {
            graph.wires.retain(|wire| !mine(wire));
            (graph.action, self.again) = (GraphAction::Disconnected(wire), true);
            self.tell_wire(graph);
        }
    }

    /// Keys while a keyboard wire is active: arrows cycle the ports it could connect to (focus
    /// and view following), Enter or Space connects, Escape cancels; so does focus leaving.
    /// Returns the node focus was sent into, if any.
    fn keyed_keys<T>(&mut self, graph: &mut Graph<T>, id: Id) -> Option<usize> {
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
        let wire = keyed.to.filter(|_| make).map(|to| keyed.from.wire(to)).filter(|wire| graph.wire(wire.from, wire.to));
        match wire {
            Some(wire) => {
                (graph.action, self.again) = (GraphAction::Connected(wire), true);
                self.tell_wire(graph);
            }
            None => self.announce(graph.words.stopped),
        }
        Some(keyed.from.node)
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
            graph.action = GraphAction::Selected;
        }
        let [copy, cut, twice] = [Key::Char('c'), Key::Char('x'), Key::Char('d')].map(|key| any && self.key(key, Mods::CTRL));
        if copy || cut || twice {
            graph.copy_selected();
        }
        if twice || self.key(Key::Char('v'), Mods::CTRL) && !graph.copied.is_empty() {
            graph.paste(if twice { None } else { at });
            (graph.action, self.again) = (GraphAction::Pasted, true);
            let words = format!("{} {}", graph.words.pasted, graph.selected_titles());
            self.announce(&words);
        }
        if cut || delete && any && [Key::Delete, Key::Backspace].iter().any(|key| self.key(*key, Mods::NONE)) {
            self.delete_selected(graph);
        }
    }

    /// Removes the selected nodes with their wires.
    fn delete_selected<T>(&mut self, graph: &mut Graph<T>) {
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

    /// The visible wires, under the nodes. (A wire being pulled off its input is drawn as a
    /// dragged one: see `show_wire_drawn`.)
    fn show_wires<T>(&mut self, graph: &Graph<T>) {
        let Some(open) = &self.pan_zoom_views.graphs.open else { return };
        let (visible, scale, t) = (open.visible, open.scale, self.theme);
        let pulled = match &self.pan_zoom_views.graphs.doing {
            Some((id, Doing::Wire { taken, .. })) if *id == open.id => *taken,
            _ => None,
        };
        // (Wire width in content units: at least one window unit.)
        let [thin, thick] = [2.0, 3.5].map(|width: f32| width.max(1.0) / scale.max(1e-6));
        for wire in graph.wires.iter().filter(|wire| Some(**wire) != pulled) {
            let graphs = &self.pan_zoom_views.graphs;
            let from = End { node: wire.from.0, port: wire.from.1, out: true };
            let (Some(a), Some(b)) = (graphs.port_point(from), graphs.port_point(End { node: wire.to.0, port: wire.to.1, out: false })) else { continue };
            let points = bend(a, b);
            if !touches(around(points, thick), visible) {
                continue;
            }
            let selected = graphs.laid[wire.from.0].selected || graphs.laid[wire.to.0].selected;
            let color = graph.tint(graph.nodes[wire.from.0].outputs[wire.from.1].kind, t.bg, t.text_dim);
            self.curve(&points, color, if selected { thick } else { thin });
        }
    }

    /// A wire through these control points (`bend`), `width` content units wide.
    fn curve(&mut self, [a, c, d, b]: &[Point; 4], color: Color, width: f32) {
        let whole = around([*a, *c, *d, *b], width);
        if !whole.w.is_finite() || !whole.h.is_finite() {
            return;
        }
        let (left, top) = (whole.x, whole.y);
        let mut path = Path::new();
        path.move_to(a.x - left, a.y - top).cubic_to(c.x - left, c.y - top, d.x - left, d.y - top, b.x - left, b.y - top);
        self.el().offset(left, top).size(whole.w, whole.h).outline(&path, width, color).show();
    }

    /// A node with its title and ports, and the box under them for the app's parts (left open:
    /// the app closes it, and the node with it).
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
        for row in 0..node.inputs.len().max(node.outputs.len()) {
            self.port_row(graph, n, row);
        }
        self.join_next = true;
        // (App content stays inside: drawn outside, it would not be redrawn where it changes.)
        let body = self.el().key(BODY_KEY).flow(Flow::Column).align(Align::Start, Align::Spread).pad(Insets { left: 10.0, top: 4.0, right: 10.0, bottom: 10.0 });
        body.gap(6.0).clip().open();
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
    /// where not. While a wire is dragged, accepting ports are ringed, the rest fade, and one
    /// under the pointer that refuses is ringed in red.
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
        let node = self.pan_zoom_views.graphs.open.as_ref().map_or(0, |open| part_id(open.inner_id, end.node as u64));
        self.rovers.push(Rover { group: node, id: dot.id, stop: false, selects: false });
        let size = if from.is_some() && (takes || from == Some(end)) { DOT + 2.0 } else { DOT };
        self.el().size(size, size).radius(size / 2.0).fill(fill).border(ring, 2.0).show();
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
        self.curve(&bend(a, b), color, 3.0f32.max(1.0) / open.scale.max(1e-6));
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

/// Moves nodes `was` (each with its start position) `by` from there. Says whether any moved.
fn move_by<T>(graph: &mut Graph<T>, was: &[(usize, Point)], by: Point) -> bool {
    let mut moved = false;
    for (node, start) in was {
        let Some(node) = graph.nodes.get_mut(*node) else { continue };
        let to = pt(start.x + by.x, start.y + by.y);
        (moved, node.position) = (moved || node.position != to, to);
    }
    moved
}

/// Selects the nodes a rectangle from `from` to `to` touches, plus those of `already` (in
/// order), and no others. Says whether anything changed.
fn select_within<T>(graph: &mut Graph<T>, from: Point, to: Point, already: &[usize]) -> bool {
    let spread = rect(from.x.min(to.x), from.y.min(to.y), (to.x - from.x).abs(), (to.y - from.y).abs());
    graph.select_only(|n, node| touches(node.rect(), spread) || already.binary_search(&n).is_ok())
}

/// What a wire from `from` would connect to over `hit`: a port of the opposite side on another
/// node; on a node, its first opposite port that accepts it (unwired ones first).
fn target<T>(graph: &Graph<T>, from: End, (node, part): (usize, Part)) -> Option<End> {
    let ports = |out: bool| graph.nodes.get(node).map_or(0, |node| if out { node.outputs.len() } else { node.inputs.len() });
    let end = match part {
        Part::In(port) => Some(End { node, port, out: false }),
        Part::Out(port) => Some(End { node, port, out: true }),
        Part::Ground | Part::Title => {
            let fitting = (0..ports(!from.out)).map(|port| End { node, port, out: !from.out }).filter(|to| graph.takes(from, *to));
            let free = fitting.clone().find(|to| !to.out && graph.wire_into((to.node, to.port)).is_none());
            free.or(fitting.clone().next())
        }
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
    let node = part_id(part_id(id, INNER), end.node as u64);
    part_id(part_id(node, ROW_KEY + end.port as u64), if end.out { OUT_KEY } else { IN_KEY })
}

/// Pans the view to show the node focus entered, where it came by key and is not wholly
/// visible: as little as needed, with a margin.
fn pan_to<T>(graph: &mut Graph<T>, state: &mut GraphState, room: Size, scale: f32, keys: bool) {
    if state.focus == state.shown_for {
        return;
    }
    state.shown_for = state.focus;
    let Some(node) = state.focus.and_then(|node| graph.nodes.get(node)).filter(|_| keys) else { return };
    let (want, visible) = (node.rect().grow(MARGIN), rect(graph.view.x, graph.view.y, room.w / scale, room.h / scale));
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
}
