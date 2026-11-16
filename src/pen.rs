//! How shapes are drawn: the pen for strokes (width, caps, joins, dashes), and the ink a fill
//! or stroke uses (one colour, or a gradient across it).
//!
//! A stroke is drawn as the shape it covers. Its curves are flattened, the segments split into
//! dashes if the pen has any, and each dash is outlined: along each segment, the segment offset
//! half the width to each side; between segments and at the ends, as the pen's joins and caps
//! say. That outline is then filled as any shape is ([`Canvas::fill_with`]). Strokes from SVG
//! drawings go the same way.
//!
//! One kind of stroke is drawn differently: round caps and joins, no dashes, one colour (what
//! `.outline(&path, 2.0, color)` draws). Such a stroke is everything within half its width of
//! the path, which [`Canvas::stroke`] renders exactly, even where it overlaps itself; filled as
//! a shape, overlapping parts would come out up to a quarter too dark at their edges.
//! Single-colour fills and such strokes are drawn by the canvas alone: the code for any other
//! pen or ink is only handed over by a call asking for one (see `fully`), so a program that
//! never does is built without it.
use crate::canvas::{Canvas, color_at};
use crate::color::Color;
use crate::geom::{Point, Transform, pt};
use crate::id::hash;
use crate::path::Path;
use std::f32::consts::PI;

/// Subpixel units per pixel strokes are computed in. (Round caps and joins are flattened by
/// their size in these: no segment strays more than a fiftieth of a pixel from the true arc.)
pub(crate) const FINER: f32 = 4.0;
/// Segments shorter than this, in those units, are skipped.
const SHORT: f32 = 1.0 / 16.0;
/// Points beyond this, in pixels, are clamped: a canvas counts no further.
const FAR_OUT: f32 = 1e6;

/// How a stroke ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cap {
    /// Butt: cut square at the end.
    Flat,
    /// A semicircle round the end.
    Round,
    /// Cut square, half the width past the end.
    Square,
}

/// How a stroke turns a corner, on the outside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    /// To a point, unless the point would extend more than this many times the width: then cut
    /// as `Bevel`. (4 is the common default; a right angle extends 1.42 times the width, and
    /// sharper corners extend further.)
    Mitre(f32),
    /// Round.
    Round,
    /// Cut straight across.
    Bevel,
}

/// The pen a stroke is drawn with: width (in the shape's units), cap, join, and dashes.
/// [`Pen::new(2.0)`](Pen::new) draws with round caps and joins; a plain number in its place
/// means the same. A stroke is never drawn thinner than a pixel.
///
/// ```
/// # use taut::{Cap, Join, Pen};
/// let dashed = Pen::new(2.0).cap(Cap::Flat).join(Join::Mitre(4.0)).dashes(&[6.0, 3.0], 0.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
pub struct Pen<'a> {
    /// Its width.
    pub width: f32,
    /// Its cap.
    pub cap: Cap,
    /// Its join.
    pub join: Join,
    /// Dash and gap lengths, alternating, in the shape's units. Empty for a solid stroke. (An
    /// odd count is repeated twice so dashes and gaps alternate evenly.)
    pub dashes: &'a [f32],
    /// How far into the dash pattern the stroke starts.
    pub offset: f32,
}

impl Pen<'static> {
    /// A `width` pen drawing solid strokes with round caps and joins.
    pub const fn new(width: f32) -> Self {
        Pen { width, cap: Cap::Round, join: Join::Round, dashes: &[], offset: 0.0 }
    }
}

impl<'a> Pen<'a> {
    /// With this cap.
    pub const fn cap(self, cap: Cap) -> Self {
        Pen { cap, ..self }
    }
    /// With this join.
    pub const fn join(self, join: Join) -> Self {
        Pen { join, ..self }
    }
    /// With dashes: dash and gap lengths, alternating, starting `offset` into them.
    /// (Zero-length dashes are dots under round caps.)
    pub const fn dashes<'b>(self, dashes: &'b [f32], offset: f32) -> Pen<'b> {
        Pen { width: self.width, cap: self.cap, join: self.join, dashes, offset }
    }
    /// Whether its stroke is exactly everything within half its width of the path: round caps
    /// and joins, and solid.
    fn is_round(&self) -> bool {
        self.cap == Cap::Round && self.join == Join::Round && self.dashes.is_empty()
    }
}

/// A pen with round caps and joins, this wide.
impl From<f32> for Pen<'static> {
    fn from(width: f32) -> Self {
        Pen::new(width)
    }
}

/// What a fill or stroke is painted with: one colour, or a gradient across it, as a box's
/// ([`El::gradient_along`](crate::El::gradient_along)). Each stop is an offset from 0 to 1 and
/// a colour, in order. The gradient spans the shape's bounds ([`Path::bounds`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink<'a> {
    /// One colour.
    Color(Color),
    /// Linear, along a line through the centre at so many degrees clockwise from up: 90 left to
    /// right, 180 top to bottom.
    Along(f32, &'a [(f32, Color)]),
    /// Radial from the centre: the first colour at the centre, the last at the bounds' edges.
    Around(&'a [(f32, Color)]),
}

/// One colour.
impl From<Color> for Ink<'static> {
    fn from(color: Color) -> Self {
        Ink::Color(color)
    }
}

/// An ink's gradient as a shape stores it: none, linear at so many degrees from up, or radial.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Way {
    #[default]
    None,
    Along(f32),
    Around,
}

/// Draws a shape the canvas does not draw by itself.
pub(crate) type Full = fn(&Inked, &mut Canvas, Point);

/// The draw function for this pen (none: a fill) and ink, where the canvas cannot draw it
/// alone: none for one colour with a round solid pen, or no pen. (Always inlined. Where the
/// call passes a colour and a plain number, the answer is known at compile time, and the
/// general code is left out.)
#[inline(always)]
pub(crate) fn fully(pen: Option<&Pen>, ink: &Ink) -> Option<Full> {
    let plain = matches!(ink, Ink::Color(_)) && pen.is_none_or(Pen::is_round);
    if plain { None } else { Some(Inked::draw_fully) }
}

/// An app shape as the interface keeps it from description to drawing: the path, the stroke pen
/// (none for a fill) with its dashes stored separately, its ink (one colour, or `stops` going
/// `way`), and its draw function where the canvas cannot draw it alone. Kept between runs, its
/// memory reused by whatever shape takes its place.
#[derive(Default)]
pub(crate) struct Inked {
    path: Path,
    pen: Option<Pen<'static>>,
    dashes: Vec<f32>,
    color: Color,
    way: Way,
    stops: Vec<(f32, Color)>,
    full: Option<Full>,
}

impl Inked {
    /// Becomes this shape, drawn so: by `full` where the canvas cannot draw it alone (see
    /// `fully`).
    pub(crate) fn set(&mut self, path: &Path, pen: Option<Pen>, ink: Ink, full: Option<Full>) {
        self.path.set(path);
        self.full = full;
        self.pen = pen.map(|pen| Pen { width: pen.width, cap: pen.cap, join: pen.join, dashes: &[], offset: pen.offset });
        self.dashes.clear();
        self.dashes.extend_from_slice(pen.map_or(&[], |pen| pen.dashes));
        let (color, way, stops) = match ink {
            Ink::Color(color) => (color, Way::None, &[][..]),
            Ink::Along(turn, stops) => (Color::NONE, Way::Along(turn), stops),
            Ink::Around(stops) => (Color::NONE, Way::Around, stops),
        };
        (self.color, self.way) = (color, way);
        self.stops.clear();
        self.stops.extend_from_slice(stops);
    }
    /// Scales by `by`: its path, and its pen with its dashes. (For zoomed board content.)
    pub(crate) fn scale_by(&mut self, by: f32) {
        self.path.transform(Transform::new().scale(by, by));
        if let Some(pen) = &mut self.pen {
            (pen.width, pen.offset) = (pen.width * by, pen.offset * by);
        }
        self.dashes.iter_mut().for_each(|dash| *dash *= by);
    }
    /// Drawn in one colour from now on: what a disabled box does.
    pub(crate) fn fade(&mut self, color: Color) {
        (self.color, self.way) = (color, Way::None);
        self.stops.clear();
    }
    /// A hash: equal for the same shape drawn the same way, practically never for another.
    pub(crate) fn key(&self) -> u64 {
        let pen = self.pen.map(|pen| (pen.width.to_bits(), [pen.cap as u8, pen.join_kind()], pen.join_limit().to_bits(), pen.offset.to_bits()));
        let way = match self.way {
            Way::None => u32::MAX,
            Way::Along(turn) => turn.to_bits(),
            Way::Around => u32::MAX - 1,
        };
        // (Computed for every visible shape every run: lengths and colours are folded in one by
        // one, with nothing allocated.)
        let key = self.dashes.iter().fold(hash((self.path.key(), pen, self.color.0, way)), |key, dash| hash((key, dash.to_bits())));
        self.stops.iter().fold(key, |key, (at, color)| hash((key, at.to_bits(), color.0)))
    }

    /// Draws it on a canvas, its origin at `at`.
    pub(crate) fn draw(&self, canvas: &mut Canvas, at: Point) {
        let s = canvas.scale();
        match (self.full, self.pen) {
            (Some(full), _) => full(self, canvas, at),
            // (A single-colour stroke with round caps and joins and no dashes is rendered
            // exactly as everything near enough to its path.)
            (None, Some(pen)) => canvas.stroke(&self.path, at, 1.0, pen.width, self.color),
            (None, None) => canvas.fill_with(&mut |each| self.path.flatten_closed(s, pt(at.x * s, at.y * s), 1.0, each), false, |_, _| self.color),
        }
    }

    /// Draws it on a canvas, its origin at `at`, whatever its pen and ink.
    fn draw_fully(&self, canvas: &mut Canvas, at: Point) {
        let (s, pen) = (canvas.scale(), self.pen.map(|pen| Pen { dashes: &self.dashes, ..pen }));
        if self.color.is_none() && self.way == Way::None {
            return;
        }
        // Each pixel's colour: by its centre's position across the shape's bounds, in window
        // pixels.
        let bounds = self.path.bounds();
        let box_in_pixels = [(at.x + bounds.x) * s, (at.y + bounds.y) * s, (at.x + bounds.right()) * s, (at.y + bounds.bottom()) * s];
        let ink = inking(self.color, self.way, &self.stops, box_in_pixels);
        let mut work = Work::default();
        match pen {
            Some(pen) => {
                let back = Transform::new().scale(1.0 / FINER, 1.0 / FINER).move_by(at.x * s, at.y * s);
                work.line(canvas, &self.path, &pen, [s * FINER, s * FINER, FINER / 2.0], [Transform::new(), back], ink);
            }
            None => work.fill(canvas, &self.path, s, Transform::new().move_by(at.x * s, at.y * s), false, ink),
        }
    }
}

impl Pen<'_> {
    /// Its join, as a number.
    fn join_kind(&self) -> u8 {
        match self.join {
            Join::Mitre(_) => 0,
            Join::Round => 1,
            Join::Bevel => 2,
        }
    }
    /// How far a mitred corner may extend (0 for a pen without mitre joins).
    fn join_limit(&self) -> f32 {
        if let Join::Mitre(limit) = self.join { limit } else { 0.0 }
    }
}

/// An ink's colour for each pixel by window position, for a shape whose bounds run from
/// `x0, y0` to `x1, y1` in window pixels. (As a box's gradients do: see [`Canvas::gradient`].
/// The first and last stops are the pixels at the two ends.)
fn inking(color: Color, way: Way, stops: &[(f32, Color)], [x0, y0, x1, y1]: [f32; 4]) -> impl Fn(i32, i32) -> Color + '_ {
    let (mid_x, mid_y, half_x, half_y) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, ((x1 - x0 - 1.0) / 2.0).max(0.5), ((y1 - y0 - 1.0) / 2.0).max(0.5));
    let (sin, cos) = if let Way::Along(turn) = way { turn.to_radians().sin_cos() } else { (0.0, 0.0) };
    let reach = (sin.abs() * half_x + cos.abs() * half_y).max(0.5);
    move |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - mid_x, y as f32 + 0.5 - mid_y);
        match way {
            Way::None => color,
            Way::Along(_) => color_at(stops, 0.5 + (dx * sin - dy * cos) / (2.0 * reach)),
            Way::Around => color_at(stops, (dx / half_x).hypot(dy / half_y)),
        }
    }
}

/// Scratch space for filling and stroking, reused: one outline's points, the same as stroke
/// input, one stroke's outline, all edges to fill, where each stroke's edges end among them,
/// and stroke coverage.
#[derive(Default)]
pub(crate) struct Work {
    pub(crate) points: Vec<Point>,
    pub(crate) moved: Vec<Point>,
    pub(crate) ring: Vec<Point>,
    pub(crate) edges: Vec<[Point; 2]>,
    pub(crate) ends: Vec<usize>,
    pub(crate) covers: Vec<u32>,
}

impl Work {
    /// Fills a shape on a canvas, `ink` colouring each pixel by window position: by the nonzero
    /// rule, or with `even_odd` the even-odd rule (an open outline is closed by a straight
    /// edge). Curves are flattened for `fine` pixels per unit, and outlines mapped through
    /// `to_pixels` to the window, from units that many times finer.
    pub(crate) fn fill(&mut self, canvas: &mut Canvas, path: &Path, fine: f32, to_pixels: Transform, even_odd: bool, ink: impl FnMut(i32, i32) -> Color) {
        let Work { points, edges, .. } = self;
        edges.clear();
        outlines(path, fine, per_pixel(to_pixels), points, |points| edges.extend((0..points.len()).map(|at| [points[at], points[(at + 1) % points.len()]])));
        canvas.fill_with(&mut placed(edges, to_pixels), even_odd, ink);
    }

    /// Strokes a shape's path with a pen on a canvas, `ink` colouring each pixel by window
    /// position. Curves are flattened for `fine` subpixel units per shape unit; points go
    /// through `there`, where a pen length unit is `grain` subpixel units, and the stroke is
    /// never thinner than `least` of them each side; and the outline goes through `back` to the
    /// window.
    pub(crate) fn line(&mut self, canvas: &mut Canvas, path: &Path, pen: &Pen, [fine, grain, least]: [f32; 3], [there, back]: [Transform; 2], ink: impl FnMut(i32, i32) -> Color) {
        let Work { points, moved, ring, edges, ends, covers } = self;
        // Each outline's stroke, and each dash, is a separate stroke: where each one's edges
        // end is recorded.
        edges.clear();
        ends.clear();
        let half = (pen.width * grain / 2.0).max(least);
        let mut line = |dash: &[Point]| {
            widen(dash, half, pen, ring, edges);
            ends.push(edges.len());
        };
        // A wide stroke's curves are flattened finer the wider it is, in pixels: at joints its
        // offset edge turns as much as the segments do, and its ends are square to the last
        // segment. So flattened, they stray no more than a fifth of a pixel from the curve's
        // own.
        let wide = (half / per_pixel(back)).max(1.0);
        outlines(path, fine, per_pixel(there) * per_pixel(back) / wide, points, |points| {
            moved.clear();
            moved.extend(points.iter().map(|point| there.apply(*point)));
            dashed(moved, pen.dashes, [grain, pen.offset * grain], &mut line)
        });
        match ends.len() {
            0 | 1 => canvas.fill_with(&mut placed(edges, back), false, ink),
            _ => lines(canvas, (edges, ends), back, ink, covers),
        }
    }
}

/// Feeds a canvas a shape's edges: each of `edges` mapped through `to_pixels` to the window
/// (clamped to what a canvas counts).
pub(crate) fn placed(edges: &[[Point; 2]], to_pixels: Transform) -> impl FnMut(&mut dyn FnMut(Point, Point)) + '_ {
    let near = |at: f32| if at.is_nan() { 0.0 } else { at.clamp(-FAR_OUT, FAR_OUT) };
    let place = move |p: Point| pt(near(to_pixels.apply(p).x), near(to_pixels.apply(p).y));
    move |each| edges.iter().for_each(|[a, b]| each(place(*a), place(*b)))
}

/// Renders several possibly overlapping strokes in one ink: `ends` says where each stroke's
/// edges end in `edges`. Each stroke fills a coverage-only mask on its own, so a pixel two
/// strokes cover counts once; then the mask is composited in the ink. (Filled as one shape,
/// such a pixel would count twice, leaving a dark spot at each crossing.) The mask covers the
/// part of the canvas the strokes reach.
fn lines(canvas: &mut Canvas, (edges, ends): (&[[Point; 2]], &[usize]), to_pixels: Transform, ink: impl FnMut(i32, i32) -> Color, covers: &mut Vec<u32>) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    placed(edges, to_pixels)(&mut |a, b| {
        (x0, y0) = (x0.min(a.x).min(b.x), y0.min(a.y).min(b.y));
        (x1, y1) = (x1.max(a.x).max(b.x), y1.max(a.y).max(b.y));
    });
    let [cx0, cy0, right, bottom] = canvas.clip_pixels();
    let (left, first) = ((x0.floor() as i32).clamp(cx0, right.max(cx0)), (y0.floor() as i32).clamp(cy0, bottom.max(cy0)));
    let (across, down) = ((x1.ceil() as i32 + 1).clamp(left, right.max(left)) - left, (y1.ceil() as i32).clamp(first, bottom.max(first)) - first);
    covers.clear();
    covers.resize((across * down) as usize, 0);
    let (mut cover, mut from) = (Canvas::new(covers, across as usize, down as usize, (left, first), 1.0), 0);
    for end in ends {
        cover.fill_with(&mut placed(&edges[from..*end], to_pixels), false, |_, _| Color::WHITE);
        from = *end;
    }
    canvas.ink(covers, across as usize, [left, first], ink);
}

/// The fewest `to_pixels` input units per pixel: by its largest stretch. (So a curve flattened
/// in them is fine enough however it is stretched.)
fn per_pixel(to_pixels: Transform) -> f32 {
    let [a, b, c, d, ..] = to_pixels.0;
    let (all, area) = (a * a + b * b + c * c + d * d, a * d - b * c);
    // (The square of the largest singular value: the larger root of x² - all x + area².)
    let most = (all + (all * all - 4.0 * area * area).max(0.0).sqrt()) / 2.0;
    1.0 / most.sqrt()
}

/// Calls `each` with each outline of a shape as its points: curves flattened for `fine` pixels
/// per unit, points in units that many times finer, `pixel` of which make a pixel where drawn.
/// `points` is scratch space.
pub(crate) fn outlines(path: &Path, fine: f32, pixel: f32, points: &mut Vec<Point>, mut each: impl FnMut(&[Point])) {
    points.clear();
    path.flatten(fine, pt(0.0, 0.0), pixel, |a, b| {
        // A segment not starting where the last ended starts a new outline; so does one
        // continuing from an outline that returned to its start.
        let closed = points.len() > 2 && points.first() == points.last();
        if points.last() != Some(&a) || closed {
            if points.len() > 1 {
                each(points);
            }
            points.clear();
            points.push(a);
        }
        points.push(b);
    });
    if points.len() > 1 {
        each(points);
    }
}

/// Splits a stroke into dashes and calls `each` with each one's points. `dashes` are dash and
/// gap lengths, alternating; `fine` is stroke units per dash unit, and `offset` how far into
/// them it starts. (Without dashes the stroke is one dash; so is one that would split into more
/// dashes than could ever be distinguished.)
fn dashed(points: &[Point], dashes: &[f32], [fine, offset]: [f32; 2], mut each: impl FnMut(&[Point])) {
    let long = |a: Point, b: Point| (b.x - a.x).hypot(b.y - a.y);
    // (An odd count is repeated twice so dashes and gaps alternate evenly.)
    let (count, around) = (dashes.len(), 2.0 * fine * dashes.iter().sum::<f32>());
    let whole: f32 = points.windows(2).map(|pair| long(pair[0], pair[1])).sum();
    if count == 0 || !(around > 0.0 && whole / around * count as f32 <= 10_000.0) {
        return each(points);
    }
    // Which dash or gap the stroke starts in (a dash if even), and how much of it remains.
    let (mut at, mut left) = (0, offset.rem_euclid(around));
    while at < 2 * count && left > 0.0 && left >= dashes[at % count] * fine {
        (at, left) = (at + 1, left - dashes[at % count] * fine);
    }
    left = dashes[at % count] * fine - left;
    // The current dash's points; empty in a gap. On a closed stroke the first dash is held
    // back: if the stroke also ends in a dash, the two merge through the start.
    let (mut dash, mut held, mut cuts) = (Vec::from_iter(points.first().filter(|_| at % 2 == 0).copied()), Vec::new(), 0);
    let closed = points.first() == points.last();
    for pair in points.windows(2) {
        let (mut from, to) = (pair[0], pair[1]);
        let mut rest = long(from, to);
        // The current dash or gap ends on this segment, while the segment has more than remains
        // of it: there the next starts.
        while rest > left && cuts < 100_000 {
            from = pt(from.x + (to.x - from.x) * left / rest, from.y + (to.y - from.y) * left / rest);
            dash.push(from);
            match (at % 2, closed && cuts == 0) {
                (0, true) => held.clone_from(&dash),
                (0, false) => each(&dash),
                _ => {}
            }
            dash.clear();
            (rest, at, cuts) = (rest - left, at + 1, cuts + 1);
            left = dashes[at % count] * fine;
            dash.extend((at % 2 == 0).then_some(from));
        }
        left -= rest;
        dash.extend((at % 2 == 0).then_some(to));
    }
    if dash.len() > 1 {
        dash.extend(held.drain(..).skip(1));
    }
    [dash, held].iter().filter(|dash| dash.len() > 1).for_each(|dash| each(dash));
}

/// Adds to `edges` the outline of a stroke along `points`, `2 * half` wide: filling it draws
/// the stroke. `ring` is scratch space.
///
/// The outline runs along one side and back along the other. Along each segment it is the
/// segment offset by `half`; between segments it goes round their joint (see `corner`). A
/// closed stroke has two outlines, one inside the other.
fn widen(points: &[Point], half: f32, pen: &Pen, ring: &mut Vec<Point>, edges: &mut Vec<[Point; 2]>) {
    let Some(first) = points.first().copied() else { return };
    // The segments with length: start, end and direction. (One too short to see has none: its
    // direction could not be told from its ends. A stroke of no length at all is a dot: one
    // zero-length segment, pointing right.)
    let piece = |pair: &[Point]| {
        let long = (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y);
        (long > SHORT).then(|| [pair[0], pair[1], pt((pair[1].x - pair[0].x) / long, (pair[1].y - pair[0].y) / long)])
    };
    let mut pieces: Vec<[Point; 3]> = points.windows(2).filter_map(piece).collect();
    // A stroke returning to its start is closed. So is one with a round solid pen that ends
    // where it began without closing (a circle of two arcs, as many icon sets write one):
    // coincident round caps equal the round join they would be, and as two caps they would draw
    // twice.
    let last = points[points.len() - 1];
    let meets = pen.cap == Cap::Round && pen.join == Join::Round && (last.x - first.x).hypot(last.y - first.y) <= SHORT;
    let closed = pieces.len() > 1 && (last == first || meets);
    if pieces.is_empty() {
        pieces.push([first, first, pt(1.0, 0.0)]);
    }
    // And the same segments reversed, last to first: the way back.
    let count = pieces.len();
    pieces.extend_from_within(..);
    pieces[count..].reverse();
    pieces[count..].iter_mut().for_each(|[from, to, way]| (*from, *to, *way) = (*to, *from, pt(-way.x, -way.y)));
    // An open stroke is outlined once, there and back; a closed one once each way.
    let rounds = if closed { [0..count, count..2 * count] } else { [0..2 * count, 0..0] };
    for round in rounds {
        ring.clear();
        let (start, turns) = (round.start, round.len());
        // The segments meeting at each turn of the loop, and whether that is the stroke's end.
        let meeting = |turn: usize| (pieces[start + turn % turns], pieces[start + (turn + 1) % turns], !closed && (turn % turns + 1).is_multiple_of(count));
        // How much of its two segments a turn's inside takes: none at an end.
        let taken = |turn: usize| if meeting(turn).2 { 0.0 } else { reach(meeting(turn).0[2], meeting(turn).1[2], half) };
        for turn in 0..turns {
            // What remains of each of the two segments for this turn: all that the turn at its
            // other end does not take.
            let (from, to, end) = meeting(turn);
            let long = |piece: [Point; 3]| (piece[1].x - piece[0].x).hypot(piece[1].y - piece[0].y);
            let room = (long(from) - taken(turn + turns - 1)).min(long(to) - taken(turn + 1));
            corner(ring, [from, to], half, pen, (end, room));
        }
        edges.extend((0..ring.len()).map(|at| [ring[at], ring[(at + 1) % ring.len()]]));
    }
}

/// How far the inside of a turn reaches along its two segments, from the vertex to where their
/// offset edges cross; one segment goes `from`, the next `to`. (Zero on the outside; infinite
/// where the stroke turns straight back.)
fn reach(from: Point, to: Point, half: f32) -> f32 {
    let turn = (from.x * to.y - from.y * to.x).atan2(from.x * to.x + from.y * to.y);
    match half * (-turn / 2.0).tan() {
        _ if turn >= 0.0 => 0.0,
        reach if reach >= 0.0 => reach,
        _ => f32::INFINITY,
    }
}

/// Continues a stroke outline from one segment's end to the next's start, adding the points to
/// `ring`. At the stroke's end (`end`), the next segment is the same one going back, and the
/// outline goes round the end as the cap says. Else the stroke turns here. On the outer side it
/// goes round as the join says: an arc, a point, or straight across. On the inner side the
/// offset edges cross, and the outline turns there if both segments have `room`; where one
/// lacks it, it goes through the stroke's own vertex, which fills the same and is never wrong.
fn corner(ring: &mut Vec<Point>, [[_, pivot, d1], [_, _, d2]]: [[Point; 3]; 2], half: f32, pen: &Pen, (end, room): (bool, f32)) {
    let aside = |way: Point, by: f32| pt(pivot.x + way.y * half + way.x * by, pivot.y - way.x * half + way.y * by);
    let (start, stop) = (aside(d1, 0.0), aside(d2, 0.0));
    // How far the stroke turns here: positive means this is the outer side.
    let (cross, dot) = (d1.x * d2.y - d1.y * d2.x, d1.x * d2.x + d1.y * d2.y);
    let turn = cross.atan2(dot);
    // Where the two offset edges meet, extended straight (or back).
    let meet = pt(pivot.x + (start.x + stop.x - 2.0 * pivot.x) / (1.0 + dot), pivot.y + (start.y + stop.y - 2.0 * pivot.y) / (1.0 + dot));
    match (end, pen.cap, pen.join) {
        (true, Cap::Round, _) => arc(ring, pivot, start, PI),
        (true, Cap::Square, _) => ring.extend([aside(d1, half), aside(d2, -half)]),
        (true, Cap::Flat, _) => ring.extend([start, stop]),
        _ if turn < 0.0 && reach(d1, d2, half) <= room => ring.push(meet),
        _ if turn < 0.0 => ring.extend([start, pivot, stop]),
        (_, _, Join::Round) => arc(ring, pivot, start, turn),
        // (A point, unless it would extend further than the miter limit allows.)
        (_, _, Join::Mitre(limit)) if 2.0 / (1.0 + dot) <= limit * limit => ring.push(meet),
        _ => ring.extend([start, stop]),
    }
}

/// Adds an arc's points to `ring`: from `from`, `turn` round `pivot`, the way a stroke's outer
/// side turns. (In segments short enough that none strays more than a fiftieth of a pixel from
/// the true arc.)
fn arc(ring: &mut Vec<Point>, pivot: Point, from: Point, turn: f32) {
    let (x, y) = (from.x - pivot.x, from.y - pivot.y);
    let pieces = ((turn * (x.hypot(y) / FINER).sqrt() * 2.5).ceil() as usize).clamp(1, 256);
    ring.extend((0..=pieces).map(|piece| {
        let (sin, cos) = (turn * piece as f32 / pieces as f32).sin_cos();
        pt(pivot.x + x * cos - y * sin, pivot.y + x * sin + y * cos)
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(0xff0000);

    /// Stores a shape, drawn so, as `El::shape` and `El::outline` do.
    fn set(shape: &mut Inked, path: &Path, pen: Option<Pen>, ink: Ink) {
        shape.set(path, pen, ink, fully(pen.as_ref(), &ink));
    }

    /// The coverage of a `side`×`side` pixel block by a stroke along `d` with `pen` in red,
    /// each pixel 0 to 255.
    fn drawn(side: usize, d: &str, pen: Pen, ink: Ink) -> Vec<u32> {
        let (mut pixels, mut shape) = (vec![0u32; side * side], Inked::default());
        set(&mut shape, &Path::svg(d), Some(pen), ink);
        shape.draw(&mut Canvas::new(&mut pixels, side, side, (0, 0), 1.0), pt(0.0, 0.0));
        pixels.iter().map(|pixel| pixel >> 24).collect()
    }
    fn covered(covers: &[u32]) -> f32 {
        covers.iter().map(|cover| *cover as f32 / 255.0).sum()
    }
    fn whole(covers: &[u32]) -> usize {
        covers.iter().filter(|cover| **cover == 255).count()
    }

    #[test]
    fn a_line_ends_and_turns_as_its_pen_says() {
        let pen = Pen::new(4.0).join(Join::Mitre(4.0));
        let line = |pen: Pen, d: &str| drawn(20, d, pen, RED.into());
        // From 6 to 14 across, four wide: butt, square (half the width longer at each end), or
        // round caps.
        assert_eq!((whole(&line(pen.cap(Cap::Flat), "M6 10h8")), whole(&line(pen.cap(Cap::Square), "M6 10h8"))), (32, 48));
        assert!((covered(&line(pen, "M6 10h8")) - (32.0 + PI * 4.0)).abs() < 0.2);
        assert!((covered(&line(Pen::new(4.0), "M6 10h8")) - (32.0 + PI * 4.0)).abs() < 0.3, "round all through, by the other way");
        // A corner: mitred, bevelled, or round. (A mitre adds a square of half the width; a
        // bevel half of that; round a quarter circle.)
        let corner = |join: Join| covered(&line(pen.cap(Cap::Flat).join(join), "M4 14h10v-10"));
        let legs = 2.0 * 10.0 * 4.0 - 4.0;
        assert!((corner(Join::Mitre(4.0)) - (legs + 4.0)).abs() < 0.05 && (corner(Join::Bevel) - (legs + 2.0)).abs() < 0.05);
        assert!((corner(Join::Round) - (legs + PI)).abs() < 0.1);
        // A mitre that would exceed the limit is bevelled: a right angle extends 1.42 times the
        // width.
        assert!((corner(Join::Mitre(1.5)) - (legs + 4.0)).abs() < 0.05 && (corner(Join::Mitre(1.4)) - (legs + 2.0)).abs() < 0.05);
        // A closed outline has no caps; and a zero-length stroke is a dot under round caps, a
        // square under square caps, and nothing under butt caps.
        assert_eq!(whole(&line(pen.cap(Cap::Flat), "M6 6h8v8h-8z")), 144 - 16);
        let dot = |cap: Cap| covered(&line(pen.cap(cap), "M10 10h0"));
        assert!((dot(Cap::Round) - PI * 4.0).abs() < 0.25 && dot(Cap::Square) == 16.0 && dot(Cap::Flat) == 0.0);
        // Never thinner than a pixel: a stroke a tenth of a pixel wide covers a full pixel
        // width along it.
        assert!((covered(&line(Pen::new(0.1).cap(Cap::Flat).join(Join::Bevel), "M4 10.5h10")) - 10.0).abs() < 0.01);
    }

    #[test]
    fn dashes_lie_where_their_lengths_say() {
        let across = |dashes: &[f32], offset: f32| drawn(40, "M0 1h40", Pen::new(2.0).cap(Cap::Flat).dashes(dashes, offset), RED.into())[..40].to_vec();
        let lit = |row: &[u32]| row.iter().map(|cover| if *cover == 255 { '#' } else { '.' }).collect::<String>();
        assert_eq!(lit(&across(&[5.0, 3.0], 0.0)), "#####...#####...#####...#####...#####...");
        assert_eq!(lit(&across(&[4.0, 2.0, 1.0], 0.0)), "####..#....##.####..#....##.####..#....#");
        assert_eq!(lit(&across(&[5.0, 3.0], 3.0)), "##...#####...#####...#####...#####...###");
        assert_eq!(lit(&across(&[5.0, 3.0], -3.0)), "...#####...#####...#####...#####...#####");
        assert_eq!(lit(&across(&[], 0.0)), "#".repeat(40));
        // Zero-length dashes are dots under round caps: one every eight, from the start.
        let dots = drawn(40, "M0 1h40", Pen::new(2.0).dashes(&[0.0, 8.0], 0.0), RED.into());
        assert_eq!(dots[..40].iter().enumerate().filter(|(_, cover)| **cover > 100).map(|(x, _)| x).collect::<Vec<usize>>(), [0, 7, 8, 15, 16, 23, 24, 31, 32]);
    }

    #[test]
    fn a_line_is_drawn_as_a_drawing_from_a_file_draws_it() {
        // The same path with the same pen renders identically, pixel for pixel, whether an app
        // draws it or a file specifies it: there is one stroker.
        let lines = [
            ("M3 30 15 6l12 24 12-24", r#"stroke-linecap="butt" stroke-linejoin="miter""#, Pen::new(5.0).cap(Cap::Flat).join(Join::Mitre(4.0))),
            ("M3 30 15 6l12 24 12-24", r#"stroke-linecap="square" stroke-linejoin="bevel""#, Pen::new(5.0).cap(Cap::Square).join(Join::Bevel)),
            ("M3 30 15 6l12 24 12-24", r#"stroke-linecap="round" stroke-linejoin="miter" stroke-miterlimit="1.5""#, Pen::new(5.0).join(Join::Mitre(1.5))),
            (
                "M6 20a14 14 0 1 1 28 0q0 14-14 14z",
                r#"stroke-dasharray="7 3 1 3" stroke-dashoffset="2" stroke-linejoin="miter""#,
                Pen::new(3.0).cap(Cap::Flat).join(Join::Mitre(4.0)).dashes(&[7.0, 3.0, 1.0, 3.0], 2.0),
            ),
            ("M4 4h32v32H4zM12 12l16 16M28 12 12 28", r#"stroke-dasharray="5" stroke-linecap="square" stroke-linejoin="round""#, Pen::new(2.5).cap(Cap::Square).join(Join::Round).dashes(&[5.0], 0.0)),
            ("M5 35C5 5 35 35 35 5", r#"stroke-dasharray="0 6" stroke-linecap="round""#, Pen::new(4.0).cap(Cap::Round).join(Join::Mitre(4.0)).dashes(&[0.0, 6.0], 0.0)),
        ];
        for (d, said, pen) in lines {
            let file = format!(r#"<svg viewBox="0 0 40 40"><path d="{d}" fill="none" stroke="red" stroke-width="{}" {said}/></svg>"#, pen.width);
            let (_, from_file) = crate::svg::decode(&file, 40, 40).unwrap();
            let by_app = drawn(40, d, pen, RED.into());
            assert_eq!(from_file.iter().map(|pixel| pixel >> 24).collect::<Vec<u32>>(), by_app, "{d} {said}");
            assert!(covered(&by_app) > 40.0);
        }
    }

    #[test]
    fn a_shape_is_filled_with_colours_across_it() {
        let (black, red) = (Color::BLACK, RED);
        let fill = |d: &str, ink: Ink| {
            let (mut pixels, mut shape) = (vec![0u32; 40 * 40], Inked::default());
            set(&mut shape, &Path::svg(d), None, ink);
            shape.draw(&mut Canvas::new(&mut pixels, 40, 40, (0, 0), 1.0), pt(0.0, 0.0));
            pixels
        };
        // Left to right across a triangle's bounds: black at its leftmost pixels, red at its
        // rightmost, halfway between in the middle.
        let along = fill("M10 30 30 10v20z", Ink::Along(90.0, &[(0.0, black), (1.0, red)]));
        let red_at = |pixels: &[u32], x: usize, y: usize| pixels[y * 40 + x] >> 16 & 255;
        assert_eq!((red_at(&along, 10, 29), red_at(&along, 29, 29)), (0, 255));
        assert!((red_at(&along, 19, 29) + red_at(&along, 20, 29)).abs_diff(255) <= 3, "{} {}", red_at(&along, 19, 29), red_at(&along, 20, 29));
        // Radial from the centre to a circle's bounds: black at the centre.
        let around = fill("M4 20a16 16 0 1 0 32 0a16 16 0 1 0-32 0z", Ink::Around(&[(0.0, black), (1.0, red)]));
        assert!(red_at(&around, 20, 20) <= 10 && red_at(&around, 35, 20) >= 240 && red_at(&around, 28, 20).abs_diff(128) <= 12, "{:?}", [20, 35, 28].map(|x| red_at(&around, x, 20)));
        // One colour fills every inside pixel; and an open shape is closed.
        let flat = fill("M10 10h20v20h-20", red.into());
        assert_eq!(flat.iter().filter(|pixel| **pixel == 0xffff_0000).count(), 400, "{:x?}", &flat[20 * 40..21 * 40]);
        // A gradient stroke, dashed: each dash coloured by its position across the stroke.
        let (mut pixels, mut shape) = (vec![0u32; 40 * 40], Inked::default());
        set(&mut shape, &Path::svg("M2 20h36"), Some(Pen::new(4.0).dashes(&[4.0, 2.0], 0.0)), Ink::Along(90.0, &[(0.0, black), (1.0, red)]));
        shape.draw(&mut Canvas::new(&mut pixels, 40, 40, (0, 0), 1.0), pt(0.0, 0.0));
        assert!(red_at(&pixels, 3, 20) < 20 && red_at(&pixels, 35, 20) > 235 && pixels[20 * 40 + 35] >> 24 == 255, "{:x}", pixels[20 * 40 + 35]);
    }

    #[test]
    fn a_kept_shape_knows_when_it_is_drawn_differently() {
        let key = |pen: Option<Pen>, ink: Ink| {
            let mut shape = Inked::default();
            set(&mut shape, Path::new().rect(0.0, 0.0, 4.0, 4.0), pen, ink);
            shape.key()
        };
        let stops = [(0.0, RED), (1.0, Color::BLACK)];
        let all = [
            key(None, RED.into()),
            key(None, Color::BLACK.into()),
            key(None, Ink::Along(90.0, &stops)),
            key(None, Ink::Along(0.0, &stops)),
            key(None, Ink::Around(&stops)),
            key(Some(Pen::new(1.0)), RED.into()),
            key(Some(Pen::new(2.0)), RED.into()),
            key(Some(Pen::new(1.0).cap(Cap::Flat)), RED.into()),
            key(Some(Pen::new(1.0).join(Join::Mitre(4.0))), RED.into()),
            key(Some(Pen::new(1.0).join(Join::Mitre(2.0))), RED.into()),
            key(Some(Pen::new(1.0).dashes(&[1.0, 2.0], 0.0)), RED.into()),
            key(Some(Pen::new(1.0).dashes(&[1.0, 2.0], 1.0)), RED.into()),
        ];
        assert!(all.iter().enumerate().all(|(i, one)| all[i + 1..].iter().all(|other| other != one)), "{all:x?}");
        assert_eq!(key(Some(Pen::new(1.0).dashes(&[1.0, 2.0], 0.0)), RED.into()), all[10]);
        // Disabled, it draws in one colour, whatever its ink.
        let mut shape = Inked::default();
        set(&mut shape, Path::new().rect(0.0, 0.0, 4.0, 4.0), None, Ink::Along(90.0, &stops));
        shape.fade(Color::WHITE);
        let mut pixels = vec![0u32; 16];
        shape.draw(&mut Canvas::new(&mut pixels, 4, 4, (0, 0), 1.0), pt(0.0, 0.0));
        assert!(pixels.iter().all(|pixel| *pixel == 0xffff_ffff) && shape.key() == key(None, Color::WHITE.into()));
    }
}
