//! Shapes of lines and curves: icons, charts, anything an app draws itself. A shape can be
//! written as SVG path data (`"M8 3.5v9M3.5 8h9"`), so web icons work as they are.
use crate::geom::{Corners, Point, Rect, Transform, pt, rect};
use crate::id::hash;

/// Flattening tolerance for measuring a shape, relative to the size of its control polygon:
/// lengths come out to a millionth.
const FINELY: f32 = 1e-6;
/// The same for hit tests (inside, distance): done on every pointer move, good to a
/// five-thousandth of the shape's size.
const NEARLY: f32 = 2e-4;
/// Flattening tolerance for drawing, in pixels: a sixty-fourth, which changes a pixel's
/// coverage by at most 1/64. (Every renderer draws curves as line segments, system text
/// included; quality comes from how close the segments lie and how exactly coverage is computed
/// along them.)
pub(crate) const TRUE: f32 = 1.0 / 64.0;
/// The most segments one curve is cut into: enough for a curve tens of thousands of pixels
/// across. (A drawing from a file may hold tens of thousands of curves, so this caps what
/// flattening can cost.)
pub(crate) const CUTS: usize = 256;

/// One pen step: move to a point, a line or a curve (quadratic or cubic) to one, or close.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Step {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}

/// A shape: a pen moved, drawn along lines and curves, and lifted. In units from the top left
/// of whatever holds it.
#[derive(Clone, Debug, Default, PartialEq)]
#[must_use]
pub struct Path {
    steps: Vec<Step>,
}

impl Path {
    /// An empty shape.
    pub fn new() -> Self {
        Path::default()
    }
    /// The pen's steps, in order.
    pub(crate) fn steps(&self) -> &[Step] {
        &self.steps
    }
    /// Becomes a copy of `other`, reusing its memory.
    pub(crate) fn set(&mut self, other: &Path) {
        self.steps.clone_from(&other.steps);
    }
    /// A hash: equal for equal shapes, practically never for others.
    pub(crate) fn key(&self) -> u64 {
        let bits = |p: Point| [p.x.to_bits(), p.y.to_bits()];
        self.steps.iter().fold(0, |key, step| match *step {
            Step::Move(a) => hash((key, 0u8, bits(a))),
            Step::Line(a) => hash((key, 1u8, bits(a))),
            Step::Quad(a, b) => hash((key, 2u8, bits(a), bits(b))),
            Step::Cubic(a, b, c) => hash((key, 3u8, bits(a), bits(b), bits(c))),
            Step::Close => hash((key, 4u8)),
        })
    }
    /// Starts a new subpath here.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.steps.push(Step::Move(pt(x, y)));
        self
    }
    /// A straight line to here.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.steps.push(Step::Line(pt(x, y)));
        self
    }
    /// A quadratic curve to `x`, `y` with control point `cx`, `cy`.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.steps.push(Step::Quad(pt(cx, cy), pt(x, y)));
        self
    }
    /// A cubic curve to `x`, `y` with control points `ax`, `ay` and `bx`, `by`.
    pub fn cubic_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) -> &mut Self {
        self.steps.push(Step::Cubic(pt(ax, ay), pt(bx, by), pt(x, y)));
        self
    }
    /// A straight line back to the subpath's start, closing it.
    pub fn close(&mut self) -> &mut Self {
        self.steps.push(Step::Close);
        self
    }
    /// A circle, from its rightmost point, clockwise on screen. (Twelve cubics, a twelfth each:
    /// within four ten-millionths of the radius, under 1/64 pixel for a circle fifty thousand
    /// pixels across. Four, as many renderers use, would be a pixel off at a few thousand.)
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Self {
        self.ellipse(cx, cy, r, r)
    }
    /// An ellipse with radii `rx` and `ry`, from its rightmost point, clockwise on screen.
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) -> &mut Self {
        self.move_to(cx + rx, cy);
        self.round(pt(cx, cy), [rx, ry], (0.0, 1.0), [0.0, std::f32::consts::TAU], pt(cx + rx, cy));
        self.close()
    }
    /// An arc of a circle round `cx`, `cy`: from `from` degrees (0 right, 90 down), `sweep`
    /// degrees on, clockwise on screen (anticlockwise if negative). A line leads to its start
    /// from the pen; in an empty shape it starts a new subpath. So a pie slice is
    /// `move_to(cx, cy).arc(cx, cy, r, from, sweep).close()`.
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, from: f32, sweep: f32) -> &mut Self {
        let (start, span) = (from.to_radians(), sweep.clamp(-360.0, 360.0).to_radians());
        self.lead_to(pt(cx + r * start.cos(), cy + r * start.sin()));
        let end = pt(cx + r * (start + span).cos(), cy + r * (start + span).sin());
        self.round(pt(cx, cy), [r, r], (0.0, 1.0), [start, span], end);
        self
    }
    /// A rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        self.move_to(x, y).line_to(x + w, y).line_to(x + w, y + h).line_to(x, y + h).close()
    }
    /// A rectangle with rounded corners: one radius for all, or each its own
    /// (`(top_left, top_right, bottom_right, bottom_left)`). Radii that would overlap along a
    /// side are all scaled down together, as CSS does.
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: impl Into<Corners>) -> &mut Self {
        let Corners { top_left, top_right, bottom_right, bottom_left } = radius.into();
        let [a, b, c, d] = [top_left, top_right, bottom_right, bottom_left].map(|r| r.max(0.0));
        // (How far each side has room for its two corners.)
        let room = |side: f32, two: f32| if two > side { side / two } else { 1.0 };
        let fit = room(w, a + b).min(room(w, d + c)).min(room(h, a + d)).min(room(h, b + c));
        let [a, b, c, d] = [a, b, c, d].map(|r| r * fit);
        // Each corner a quarter circle round a point that far in, from the side before to the
        // side after (angles in quarter turns, clockwise from the right).
        let corner = |path: &mut Path, r: f32, middle: Point, from: f32, end: Point| {
            if r > 0.0 {
                path.round(middle, [r, r], (0.0, 1.0), [from * std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2], end);
            }
        };
        self.move_to(x + a, y).line_to(x + w - b, y);
        corner(self, b, pt(x + w - b, y + b), -1.0, pt(x + w, y + b));
        self.line_to(x + w, y + h - c);
        corner(self, c, pt(x + w - c, y + h - c), 0.0, pt(x + w - c, y + h));
        self.line_to(x + d, y + h);
        corner(self, d, pt(x + d, y + h - d), 1.0, pt(x, y + h - d));
        self.line_to(x, y + a);
        corner(self, a, pt(x + a, y + a), 2.0, pt(x + a, y));
        self.close()
    }
    /// A regular polygon with `sides` sides, corners `r` from the middle, the first straight
    /// up. (Fewer than three sides make none.)
    pub fn polygon(&mut self, cx: f32, cy: f32, r: f32, sides: usize) -> &mut Self {
        self.points_round(pt(cx, cy), [r, r], sides)
    }
    /// A star with `points` points `outer` from the middle and inner corners `inner` from it,
    /// the first point straight up. (Fewer than two points make none.)
    pub fn star(&mut self, cx: f32, cy: f32, outer: f32, inner: f32, points: usize) -> &mut Self {
        match points {
            0 | 1 => self,
            _ => self.points_round(pt(cx, cy), [outer, inner], 2 * points),
        }
    }
    /// A smooth curve through each of `points` (`(x, y)`), bending as little as it can: no
    /// loops or cusps the points do not have, however unevenly spaced (centripetal
    /// Catmull-Rom). A list ending where it began continues smoothly through its start and
    /// closes. As with `arc`, a line leads to the first point; in an empty shape it starts a
    /// new subpath. (A chart line that must not overshoot its values is
    /// [`Series::smooth`](crate::Series::smooth).)
    pub fn smooth_through(&mut self, points: &[(f32, f32)]) -> &mut Self {
        let mut points: Vec<Point> = points.iter().map(|&(x, y)| pt(x, y)).collect();
        points.dedup();
        let Some(&first) = points.first() else { return self };
        self.lead_to(first);
        let closed = points.len() > 3 && points.last() == Some(&first);
        if closed {
            points.pop();
        }
        let count = points.len() as isize;
        // The points before and after each segment: wrapping round a closed curve, and
        // extrapolated past an open one's ends by its first or last segment.
        let at = |i: isize| match i {
            _ if closed => points[i.rem_euclid(count) as usize],
            -1 => pt(2.0 * points[0].x - points[1].x, 2.0 * points[0].y - points[1].y),
            i if i == count => pt(2.0 * points[i as usize - 1].x - points[i as usize - 2].x, 2.0 * points[i as usize - 1].y - points[i as usize - 2].y),
            i => points[i as usize],
        };
        for i in 0..if closed { count } else { count - 1 } {
            let (a, b) = handles([at(i - 1), at(i), at(i + 1), at(i + 2)]);
            self.steps.push(Step::Cubic(a, b, at(i + 1)));
        }
        if closed { self.close() } else { self }
    }
    /// Straight lines through each of `points` (`(x, y)`), each corner rounded by an arc of
    /// `radius`: smaller where the lines are too short, so a round never takes more than half a
    /// line. (A wire with rounded bends, a hand-drawn outline.) A list ending where it began
    /// rounds that corner too and closes. As with `arc`, a line leads to the first point.
    pub fn rounded_through(&mut self, points: &[(f32, f32)], radius: f32) -> &mut Self {
        let mut points: Vec<Point> = points.iter().map(|&(x, y)| pt(x, y)).collect();
        points.dedup();
        let (Some(&first), Some(&last)) = (points.first(), points.last()) else { return self };
        let closed = points.len() > 3 && last == first;
        if !closed {
            self.lead_to(first);
            (1..points.len().saturating_sub(1)).for_each(|at| self.corner(points[at - 1], points[at], points[at + 1], radius));
            self.steps.extend((points.len() > 1).then_some(Step::Line(last)));
            return self;
        }
        // (Round all of a closed one's corners, the first too: it starts halfway along its
        // first line.)
        points.pop();
        let count = points.len();
        self.lead_to(pt((points[0].x + points[1].x) / 2.0, (points[0].y + points[1].y) / 2.0));
        (1..=count).for_each(|at| self.corner(points[at - 1], points[at % count], points[(at + 1) % count], radius));
        self.close()
    }
    /// Goes on to `through`, from `from` toward `to`, its corner rounded as
    /// [`rounded_through`](Self::rounded_through) says: a line to where the round starts, then
    /// the round.
    fn corner(&mut self, from: Point, through: Point, to: Point, radius: f32) {
        let (back, on) = (pt(from.x - through.x, from.y - through.y), pt(to.x - through.x, to.y - through.y));
        let (long_back, long_on) = (back.x.hypot(back.y), on.x.hypot(on.y));
        // The angle at the corner, and how far from it each line touches a circle of that
        // radius.
        let between = ((back.x * on.x + back.y * on.y) / (long_back * long_on)).clamp(-1.0, 1.0).acos();
        let reach = (radius / (between / 2.0).tan()).min(long_back / 2.0).min(long_on / 2.0);
        // (No real corner, or a full turn back: not rounded.)
        if !(radius > 0.0 && reach > 0.0 && between > 1e-3 && between < std::f32::consts::PI - 1e-3) {
            self.steps.push(Step::Line(through));
            return;
        }
        let (start, end) = (pt(through.x + back.x * reach / long_back, through.y + back.y * reach / long_back), pt(through.x + on.x * reach / long_on, through.y + on.y * reach / long_on));
        self.steps.push(Step::Line(start));
        // Clockwise on screen where the line turns that way.
        let clockwise = back.x * on.y - back.y * on.x < 0.0;
        self.arc_to(start, reach * (between / 2.0).tan(), reach * (between / 2.0).tan(), 0.0, false, clockwise, end);
    }
    /// Leads the pen to `point`: a line from where it is, or in an empty shape a new subpath
    /// there.
    fn lead_to(&mut self, point: Point) {
        match self.steps.last() {
            None | Some(Step::Close) => self.steps.push(Step::Move(point)),
            _ if self.end() != point => self.steps.push(Step::Line(point)),
            _ => {}
        }
    }
    /// A closed subpath through `count` points evenly round a middle, the first straight up:
    /// even ones `far[0]` out, odd ones `far[1]`.
    fn points_round(&mut self, middle: Point, far: [f32; 2], count: usize) -> &mut Self {
        if count < 3 {
            return self;
        }
        for i in 0..count {
            let (sin, cos) = (std::f32::consts::TAU * i as f32 / count as f32 - std::f32::consts::FRAC_PI_2).sin_cos();
            let p = pt(middle.x + far[i % 2] * cos, middle.y + far[i % 2] * sin);
            self.steps.push(if i == 0 { Step::Move(p) } else { Step::Line(p) });
        }
        self.close()
    }

    /// Appends another shape, subpath by subpath. (Where it does not begin with
    /// [`move_to`](Self::move_to), its first line continues from this one's end.) Filled by the
    /// nonzero rule: to cut a hole, add a subpath going the other way (see `reverse`).
    pub fn add(&mut self, other: &Path) -> &mut Self {
        self.steps.extend_from_slice(&other.steps);
        self
    }
    /// Reverses every subpath: it covers the same, but added to one going the same way round,
    /// it now cuts a hole.
    pub fn reverse(&mut self) -> &mut Self {
        let (mut back, mut from, mut cur) = (Vec::with_capacity(self.steps.len() + 2), 0, pt(0.0, 0.0));
        // Each subpath alone: from its start step to the next one's.
        while from < self.steps.len() {
            let to = piece_end(&self.steps, from);
            cur = turned_back(&self.steps[from..to], cur, &mut back);
            from = to;
        }
        self.steps = back;
        self
    }
    /// Transforms every point: rotate, scale, translate or skew. (A transformed curve is the
    /// curve through its transformed control points: nothing is lost.)
    pub fn transform(&mut self, transform: Transform) -> &mut Self {
        let to = |p: &mut Point| *p = transform.apply(*p);
        for step in &mut self.steps {
            match step {
                Step::Move(a) | Step::Line(a) => to(a),
                Step::Quad(a, b) => [a, b].into_iter().for_each(to),
                Step::Cubic(a, b, c) => [a, b, c].into_iter().for_each(to),
                Step::Close => {}
            }
        }
        self
    }

    /// The pen's position: the last step's end (the subpath start, after `close`).
    fn end(&self) -> Point {
        let last = |step: &Step| match *step {
            Step::Move(p) | Step::Line(p) | Step::Quad(_, p) | Step::Cubic(_, _, p) => Some(p),
            Step::Close => None,
        };
        match self.steps.last() {
            Some(Step::Close) => self.steps.iter().rev().find_map(|step| if let Step::Move(p) = step { Some(*p) } else { None }).unwrap_or_default(),
            step => step.and_then(last).unwrap_or_default(),
        }
    }

    /// Parses SVG path data (a `<path>`'s `d`). What it cannot read is skipped.
    pub fn svg(d: &str) -> Path {
        let mut pen = Pen { numbers: Numbers { s: d.as_bytes(), at: 0 }, path: Path::new(), at: pt(0.0, 0.0), start: pt(0.0, 0.0), handle: None, relative: false };
        let mut command = b' ';
        loop {
            pen.numbers.skip_space();
            let Some(&next) = pen.numbers.s.get(pen.numbers.at) else { break };
            if next.is_ascii_alphabetic() {
                (command, pen.numbers.at) = (next, pen.numbers.at + 1);
            }
            if command == b' ' {
                break; // numbers before any command: not a path
            }
            pen.relative = command.is_ascii_lowercase();
            let Some(handle) = pen.carry_out(command.to_ascii_lowercase()) else { break };
            pen.handle = handle;
            // Extra points after a move are lines; after a close, a new command must come.
            command = match command {
                b'm' => b'l',
                b'M' => b'L',
                b'z' | b'Z' => b' ',
                same => same,
            };
        }
        pen.path
    }

    /// Whether a point is inside, by the nonzero rule `.shape` fills with. (An open subpath
    /// counts as closed by a line, as filling closes it.)
    pub fn contains(&self, p: Point) -> bool {
        // The winding number: each crossing of the ray to the right counts +1 downward, -1
        // upward.
        let mut round = 0;
        self.cut(true, NEARLY, |a, b, _| {
            if (a.y <= p.y) != (b.y <= p.y) && a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) > p.x {
                round += if b.y > a.y { 1 } else { -1 };
            }
        });
        round != 0
    }
    /// The distance from a point to the nearest point of the outline: whether a click hit a
    /// line (`path.distance(at) <= 4.0`). Infinite for an empty shape.
    pub fn distance(&self, p: Point) -> f32 {
        let mut nearest = f32::INFINITY;
        self.cut(false, NEARLY, |a, b, _| {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let along = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy).max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
            nearest = nearest.min((p.x - a.x - along * dx).hypot(p.y - a.y - along * dy));
        });
        nearest
    }
    /// The tight bounding box: of a curve, its true extremes, not its control points. Empty for
    /// an empty shape.
    pub fn bounds(&self) -> Rect {
        let (mut least, mut most) = (pt(f32::INFINITY, f32::INFINITY), pt(f32::NEG_INFINITY, f32::NEG_INFINITY));
        let mut take = |p: Point| (least, most) = (pt(least.x.min(p.x), least.y.min(p.y)), pt(most.x.max(p.x), most.y.max(p.y)));
        self.pieces(false, |piece| match piece {
            Piece::Line(a, b) => [a, b].into_iter().for_each(&mut take),
            Piece::Curve(c) => [c[0], c[3]].into_iter().chain(turns(c).into_iter().flatten().map(|t| piece.at(t))).for_each(&mut take),
        });
        match least.x <= most.x {
            true => rect(least.x, least.y, most.x - least.x, most.y - least.y),
            false => Rect::EMPTY,
        }
    }
    /// The outline's total length.
    pub fn length(&self) -> f32 {
        let mut all = 0.0;
        self.cut(false, FINELY, |a, b, _| all += (b.x - a.x).hypot(b.y - a.y));
        all
    }
    /// The point `length` along the outline, and its direction there in degrees as
    /// [`Transform::rotate`] takes them (0 right, 90 down): so an arrowhead is placed at a
    /// line's end by rotating it that much and moving it there. Negative is the start, past the
    /// end the end. Nothing for an empty shape.
    pub fn point_at(&self, length: f32) -> Option<(Point, f32)> {
        let (mut gone, mut first, mut found) = (0.0, None, None);
        self.cut(false, FINELY, |a, b, (piece, [t0, t1])| {
            let long = (b.x - a.x).hypot(b.y - a.y);
            first = first.or(Some(a));
            // (Zero-length segments have no direction; once the segment holding the point is
            // found, the rest are skipped.)
            if long > 0.0 && (found.is_none() || gone <= length) {
                let t = t0 + (t1 - t0) * ((length - gone) / long).clamp(0.0, 1.0);
                let way = piece.way(t);
                found = Some((piece.at(t), if way == pt(0.0, 0.0) { pt(b.x - a.x, b.y - a.y) } else { way }));
            }
            gone += long;
        });
        let found = found.or(first.map(|p| (p, pt(1.0, 0.0))));
        found.map(|(p, way)| (p, way.y.atan2(way.x).to_degrees()))
    }

    /// Calls `each` with every segment, in order; with `closing`, also the line closing an open
    /// subpath, as filling does. (A closing line of no length is skipped.)
    fn pieces(&self, closing: bool, mut each: impl FnMut(Piece)) {
        let (mut cur, mut start) = (pt(0.0, 0.0), pt(0.0, 0.0));
        for step in &self.steps {
            let from = cur;
            let piece = match *step {
                Step::Move(_) => (closing && from != start).then_some(Piece::Line(from, start)),
                Step::Line(p) => Some(Piece::Line(from, p)),
                Step::Quad(c, p) => Some(Piece::Curve([from, toward(from, c), toward(p, c), p])),
                Step::Cubic(a, b, p) => Some(Piece::Curve([from, a, b, p])),
                Step::Close => (from != start).then_some(Piece::Line(from, start)),
            };
            piece.into_iter().for_each(&mut each);
            (cur, start) = match *step {
                Step::Move(p) => (p, p),
                Step::Line(p) | Step::Quad(_, p) | Step::Cubic(_, _, p) => (p, start),
                Step::Close => (start, start),
            };
        }
        if closing && cur != start {
            each(Piece::Line(cur, start));
        }
    }
    /// The same, with every curve flattened finely enough that no line strays further than
    /// `within` of its control polygon's size: `each` gets each line's ends, the segment it
    /// lies on, and the parameters of its two ends (0 to 1).
    fn cut(&self, closing: bool, within: f32, mut each: impl FnMut(Point, Point, (Piece, [f32; 2]))) {
        self.pieces(closing, |piece| {
            let cuts = piece.cuts(within);
            let at = |i: usize| i as f32 / cuts as f32;
            (0..cuts).for_each(|i| each(piece.at(at(i)), piece.at(at(i + 1)), (piece, [at(i), at(i + 1)])));
        });
    }

    /// An SVG elliptical arc from `from` to `to`, as cubics.
    #[allow(clippy::too_many_arguments)]
    fn arc_to(&mut self, from: Point, rx: f32, ry: f32, turn: f32, large: bool, sweep: bool, to: Point) {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 || from == to {
            self.steps.push(Step::Line(to));
            return;
        }
        let (sin, cos) = turn.to_radians().sin_cos();
        // The ends relative to their midpoint, with the ellipse unrotated.
        let (dx, dy) = ((from.x - to.x) / 2.0, (from.y - to.y) / 2.0);
        let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        let too_small = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if too_small > 1.0 {
            (rx, ry) = (rx * too_small.sqrt(), ry * too_small.sqrt());
        }
        let under = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0) / (rx * rx * y1 * y1 + ry * ry * x1 * x1);
        let side = if large == sweep { -under.sqrt() } else { under.sqrt() };
        let (cx1, cy1) = (side * rx * y1 / ry, -side * ry * x1 / rx);
        let (cx, cy) = (cos * cx1 - sin * cy1 + (from.x + to.x) / 2.0, sin * cx1 + cos * cy1 + (from.y + to.y) / 2.0);
        let angle = |x: f32, y: f32| y.atan2(x);
        let start = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
        let mut span = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry) - start;
        if sweep && span < 0.0 {
            span += std::f32::consts::TAU;
        } else if !sweep && span > 0.0 {
            span -= std::f32::consts::TAU;
        }
        self.round(pt(cx, cy), [rx, ry], (sin, cos), [start, span], to);
    }

    /// An elliptical arc round `middle` with `radii`, rotated by the angle whose sine and
    /// cosine `tilt` holds: from parameter angle `turns[0]` on by `turns[1]` (radians), as
    /// cubics from the pen, the last ending exactly at `end` (where the arithmetic puts it,
    /// nearly).
    fn round(&mut self, middle: Point, [rx, ry]: [f32; 2], (sin, cos): (f32, f32), [start, span]: [f32; 2], end: Point) {
        // At most a twelfth of a turn per cubic: such a cubic strays from the arc by four
        // ten-millionths of the radius, a quarter turn's by three ten-thousandths. (Error grows
        // with the sixth power of the angle.)
        let pieces = (span.abs() / (std::f32::consts::TAU / 12.0) - 1e-4).ceil().max(1.0) as usize;
        let step = span / pieces as f32;
        let reach = 4.0 / 3.0 * (step / 4.0).tan();
        let on = |a: f32| pt(middle.x + rx * a.cos() * cos - ry * a.sin() * sin, middle.y + rx * a.cos() * sin + ry * a.sin() * cos);
        let along = |a: f32| pt(-rx * a.sin() * cos - ry * a.cos() * sin, -rx * a.sin() * sin + ry * a.cos() * cos);
        for i in 0..pieces {
            let (a, b) = (start + step * i as f32, start + step * (i + 1) as f32);
            let (p0, p1, d0, d1) = (on(a), if i + 1 == pieces { end } else { on(b) }, along(a), along(b));
            self.steps.push(Step::Cubic(pt(p0.x + reach * d0.x, p0.y + reach * d0.y), pt(p1.x - reach * d1.x, p1.y - reach * d1.y), p1));
        }
    }

    /// The shape as line segments, each curve flattened finely enough to look exact: none
    /// strays more than `TRUE` of a pixel. `scale` and `offset` place it, `pixel` is units per
    /// pixel; `each` gets every segment's ends.
    pub(crate) fn flatten(&self, scale: f32, offset: Point, pixel: f32, mut each: impl FnMut(Point, Point)) {
        let place = |p: Point| pt(p.x * scale + offset.x, p.y * scale + offset.y);
        let within = TRUE * pixel;
        let (mut cur, mut start) = (pt(0.0, 0.0), pt(0.0, 0.0));
        for step in &self.steps {
            let from = cur;
            cur = match *step {
                Step::Move(p) | Step::Line(p) => place(p),
                Step::Quad(c, p) => curve([from, toward(from, place(c)), toward(place(p), place(c)), place(p)], within, &mut each),
                Step::Cubic(a, b, p) => curve([from, place(a), place(b), place(p)], within, &mut each),
                Step::Close => start,
            };
            match *step {
                Step::Move(_) => start = cur,
                Step::Line(_) => each(from, cur),
                Step::Close if from != cur => each(from, cur),
                _ => {}
            }
        }
    }
}

impl Path {
    /// As `flatten`, with each open subpath closed by a line to its start: the edges of what
    /// filling covers.
    pub(crate) fn flatten_closed(&self, scale: f32, offset: Point, pixel: f32, mut each: impl FnMut(Point, Point)) {
        // (A segment not starting where the last ended starts a new outline.)
        let (mut start, mut end) = (None, None);
        self.flatten(scale, offset, pixel, |a, b| {
            if let (Some(start), Some(end)) = (start, end.filter(|end| *end != a)) {
                each(end, start);
            }
            start = if end == Some(a) { start } else { Some(a) };
            end = Some(b);
            each(a, b);
        });
        if let (Some(start), Some(end)) = (start, end) {
            each(end, start);
        }
    }
}

/// The SVG path parser's state: numbers to read, the shape so far, the pen, the current
/// subpath's start, the last curve's ending control point, and whether the command is relative.
struct Pen<'a> {
    numbers: Numbers<'a>,
    path: Path,
    at: Point,
    start: Point,
    handle: Option<Point>,
    relative: bool,
}

impl Pen<'_> {
    /// A point as the command means it: relative to the pen, or absolute.
    fn point(&mut self) -> Option<Point> {
        let (x, y) = (self.numbers.next()?, self.numbers.next()?);
        Some(if self.relative { pt(self.at.x + x, self.at.y + y) } else { pt(x, y) })
    }

    /// Carries out one command (lower case) and moves to its end. Returns the control point a
    /// curve ended with; nothing at all when numbers run out or the command is unknown.
    fn carry_out(&mut self, command: u8) -> Option<Option<Point>> {
        let (at, from) = (self.at, if self.relative { self.at } else { pt(0.0, 0.0) });
        // "s" and "t" omit their first control point: it reflects the previous one.
        let mirrored = self.handle.map_or(at, |h| pt(2.0 * at.x - h.x, 2.0 * at.y - h.y));
        let step = match command {
            b'm' => Step::Move(self.point()?),
            b'l' => Step::Line(self.point()?),
            b'h' => Step::Line(pt(from.x + self.numbers.next()?, at.y)),
            b'v' => Step::Line(pt(at.x, from.y + self.numbers.next()?)),
            b'c' => Step::Cubic(self.point()?, self.point()?, self.point()?),
            b's' => Step::Cubic(mirrored, self.point()?, self.point()?),
            b'q' => Step::Quad(self.point()?, self.point()?),
            b't' => Step::Quad(mirrored, self.point()?),
            b'a' => return self.arc_to(),
            b'z' => Step::Close,
            _ => return None,
        };
        self.path.steps.push(step);
        let (to, handle) = match step {
            Step::Move(p) | Step::Line(p) => (p, None),
            Step::Quad(c, p) => (p, Some(c)),
            Step::Cubic(_, b, p) => (p, Some(b)),
            Step::Close => (self.start, None),
        };
        (self.at, self.start) = (to, if command == b'm' { to } else { self.start });
        Some(handle)
    }

    /// An arc from the pen to the command's end point.
    fn arc_to(&mut self) -> Option<Option<Point>> {
        let (rx, ry, turn) = (self.numbers.next()?, self.numbers.next()?, self.numbers.next()?);
        let (large, sweep, to) = (self.numbers.flag()?, self.numbers.flag()?, self.point()?);
        self.path.arc_to(self.at, rx, ry, turn, large, sweep, to);
        self.at = to;
        Some(None)
    }
}

/// Flattens one cubic (start, two control points, end) into segments within `within` of it, and
/// returns its end. The segments are equal parameter steps, as many as Wang's formula says: a
/// cubic strays from the chord between two of its points by at most three quarters of its
/// second difference over the square of the step count. (So a curve is cut by how much it bends
/// and how large it is drawn: a tight round into many, a flat one into few, a straight one into
/// one. Never more than `CUTS`.)
///
/// A chord lies wholly inside the curve's bend: a shape drawn so would come out a little small,
/// and thin glyphs thinner. So each joint between segments is lifted off the curve, outward, by
/// a third of the chord's shortfall at its middle: then the segments lie as much outside as
/// inside, and stray two thirds as far.
fn curve([p0, a, b, p1]: [Point; 4], within: f32, each: &mut impl FnMut(Point, Point)) -> Point {
    // The second differences at the start and end: the curve's second difference anywhere
    // between blends from one to the other (six times it is the curvature direction, toward the
    // inside of the bend).
    let (bent, bends) = (pt(p0.x - 2.0 * a.x + b.x, p0.y - 2.0 * a.y + b.y), pt(a.x - 2.0 * b.x + p1.x, a.y - 2.0 * b.y + p1.y));
    // (NaN, or zero, comes to one segment.)
    let pieces = ((0.5 * bent.x.hypot(bent.y).max(bends.x.hypot(bends.y)) / within).sqrt().ceil() as usize).clamp(1, CUTS);
    let (step, mut from) = (1.0 / pieces as f32, p0);
    let lift = step * step / 2.0;
    for i in 1..pieces {
        let (t, u) = (i as f32 * step, 1.0 - i as f32 * step);
        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let on = pt(w0 * p0.x + w1 * a.x + w2 * b.x + w3 * p1.x, w0 * p0.y + w1 * a.y + w2 * b.y + w3 * p1.y);
        let to = pt(on.x - lift * (u * bent.x + t * bends.x), on.y - lift * (u * bent.y + t * bends.y));
        each(from, to);
        from = to;
    }
    each(from, p1);
    p1
}

/// The control points of the segment from `b` to `c` of a centripetal Catmull-Rom curve through
/// `a`, `b`, `c`, `d`: each knot interval the square root of its points' distance. (So unevenly
/// spaced points never loop between two close ones.)
fn handles([a, b, c, d]: [Point; 4]) -> (Point, Point) {
    let apart = |p: Point, q: Point| (q.x - p.x).hypot(q.y - p.y).sqrt().max(1e-6);
    let (d1, d2, d3) = (apart(a, b), apart(b, c), apart(c, d));
    let mix = |[p, q, r]: [Point; 3], [u, v, w]: [f32; 3], by: f32| pt((u * p.x + v * q.x + w * r.x) / by, (u * p.y + v * q.y + w * r.y) / by);
    let first = mix([c, a, b], [d1 * d1, -d2 * d2, 2.0 * d1 * d1 + 3.0 * d1 * d2 + d2 * d2], 3.0 * d1 * (d1 + d2));
    let second = mix([b, d, c], [d3 * d3, -d2 * d2, 2.0 * d3 * d3 + 3.0 * d3 * d2 + d2 * d2], 3.0 * d3 * (d3 + d2));
    (first, second)
}

/// A quadratic is the cubic whose control points lie two thirds of the way from each end to its
/// control point: the one for end `p`, given control point `c`.
fn toward(p: Point, c: Point) -> Point {
    pt(p.x + 2.0 / 3.0 * (c.x - p.x), p.y + 2.0 / 3.0 * (c.y - p.y))
}

/// Where one subpath's steps end, from `from`: before the next `move_to`, or with the next
/// [`close`](Path::close).
fn piece_end(steps: &[Step], from: usize) -> usize {
    let later = steps[from..].iter().enumerate().skip(1).find(|(at, step)| matches!(step, Step::Move(_)) || matches!(steps[from + at - 1], Step::Close));
    later.map_or(steps.len(), |(at, _)| from + at)
}

/// Appends to `back` one subpath reversed. `cur` is the pen before its steps; returns the pen
/// after them (in the original direction).
fn turned_back(steps: &[Step], cur: Point, back: &mut Vec<Step>) -> Point {
    let start = if let Some(Step::Move(p)) = steps.first() { *p } else { cur };
    let drawn: Vec<Step> = steps.iter().copied().filter(|step| !matches!(step, Step::Move(_) | Step::Close)).collect();
    // Where each drawing step starts: the subpath start, or the previous end.
    let ends = |step: &Step| match *step {
        Step::Move(p) | Step::Line(p) | Step::Quad(_, p) | Step::Cubic(_, _, p) => p,
        Step::Close => start,
    };
    let froms: Vec<Point> = std::iter::once(start).chain(drawn.iter().map(ends)).collect();
    let (last, closed) = (froms[froms.len() - 1], steps.last() == Some(&Step::Close));
    // A closed subpath starts where it did, and runs first along its closing line.
    back.push(Step::Move(if closed { start } else { last }));
    if closed && last != start {
        back.push(Step::Line(last));
    }
    back.extend(drawn.iter().zip(&froms).rev().map(|(step, from)| match *step {
        Step::Quad(c, _) => Step::Quad(c, *from),
        Step::Cubic(a, b, _) => Step::Cubic(b, a, *from),
        _ => Step::Line(*from),
    }));
    back.extend(closed.then_some(Step::Close));
    if closed { start } else { last }
}

/// One outline segment: a line, or a cubic (start, two control points, end).
#[derive(Clone, Copy)]
enum Piece {
    Line(Point, Point),
    Curve([Point; 4]),
}

impl Piece {
    /// The point at parameter `t`, 0 at its start to 1 at its end.
    fn at(self, t: f32) -> Point {
        match self {
            Piece::Line(a, b) => pt(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
            Piece::Curve([p0, a, b, p1]) => {
                let u = 1.0 - t;
                let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                pt(w0 * p0.x + w1 * a.x + w2 * b.x + w3 * p1.x, w0 * p0.y + w1 * a.y + w2 * b.y + w3 * p1.y)
            }
        }
    }
    /// Its derivative at `t` (zero where a curve stops for a moment).
    fn way(self, t: f32) -> Point {
        match self {
            Piece::Line(a, b) => pt(b.x - a.x, b.y - a.y),
            Piece::Curve([p0, a, b, p1]) => {
                let u = 1.0 - t;
                let (w0, w1, w2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
                pt(w0 * (a.x - p0.x) + w1 * (b.x - a.x) + w2 * (p1.x - b.x), w0 * (a.y - p0.y) + w1 * (b.y - a.y) + w2 * (p1.y - b.y))
            }
        }
    }
    /// How many equal parameter steps it is cut into, so none strays further than `within` of
    /// its control polygon's size. (Wang's formula: a cubic strays from such a chord by at most
    /// three quarters of its second difference over the square of the count.)
    fn cuts(self, within: f32) -> usize {
        let Piece::Curve([p0, a, b, p1]) = self else { return 1 };
        let bend = |p: Point, q: Point, r: Point| (p.x - 2.0 * q.x + r.x).hypot(p.y - 2.0 * q.y + r.y);
        let apart = (a.x - p0.x).hypot(a.y - p0.y) + (b.x - a.x).hypot(b.y - a.y) + (p1.x - b.x).hypot(p1.y - b.y);
        ((0.75 * bend(p0, a, b).max(bend(a, b, p1)) / (apart * within)).sqrt().ceil() as usize).clamp(1, 4096)
    }
}

/// Where along a cubic (0 to 1) it reaches its extremes across and down: none, one or two for
/// each axis.
fn turns([p0, a, b, p1]: [Point; 4]) -> [Option<f32>; 4] {
    // Where its derivative along an axis is zero: a quadratic in t.
    let roots = |p0: f32, a: f32, b: f32, p1: f32| {
        let (square, once, alone) = (p1 - 3.0 * b + 3.0 * a - p0, 2.0 * (b - 2.0 * a + p0), a - p0);
        let inside = |t: f32| (t > 0.0 && t < 1.0).then_some(t);
        let under = (once * once - 4.0 * square * alone).sqrt();
        match square.abs() < 1e-12 {
            true => [inside(-alone / once), None],
            false => [inside((-once + under) / (2.0 * square)), inside((-once - under) / (2.0 * square))],
        }
    };
    let ([x0, x1], [y0, y1]) = (roots(p0.x, a.x, b.x, p1.x), roots(p0.y, a.y, b.y, p1.y));
    [x0, x1, y0, y1]
}

/// Reads SVG path numbers, which may run together: "1-2", "0.5.5", "1e-3".
struct Numbers<'a> {
    s: &'a [u8],
    at: usize,
}

impl Numbers<'_> {
    fn skip_space(&mut self) {
        while self.s.get(self.at).is_some_and(|c| c.is_ascii_whitespace() || *c == b',') {
            self.at += 1;
        }
    }
    fn next(&mut self) -> Option<f32> {
        self.skip_space();
        let from = self.at;
        let (mut dot, mut exp) = (false, false);
        while let Some(&c) = self.s.get(self.at) {
            let sign = (c == b'-' || c == b'+') && (self.at == from || matches!(self.s[self.at - 1], b'e' | b'E'));
            let first_dot = c == b'.' && !dot && !exp;
            let first_exp = (c == b'e' || c == b'E') && !exp && self.at > from;
            if !(c.is_ascii_digit() || sign || first_dot || first_exp) {
                break;
            }
            dot |= first_dot;
            exp |= first_exp;
            self.at += 1;
        }
        std::str::from_utf8(&self.s[from..self.at]).ok()?.parse().ok()
    }
    /// An arc's two flags are single digits, and may abut the next number.
    fn flag(&mut self) -> Option<bool> {
        self.skip_space();
        let c = *self.s.get(self.at)?;
        self.at += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(d: &str) -> Vec<(Point, Point)> {
        let mut out = Vec::new();
        Path::svg(d).flatten(1.0, pt(0.0, 0.0), 1.0, |a, b| out.push((a, b)));
        out
    }

    #[test]
    fn a_plus_sign() {
        assert_eq!(lines("M8 3.5v9M3.5 8h9"), [(pt(8.0, 3.5), pt(8.0, 12.5)), (pt(3.5, 8.0), pt(12.5, 8.0))]);
    }

    #[test]
    fn numbers_that_run_together() {
        assert_eq!(lines("M4 4l8 8M12 4l-8 8"), [(pt(4.0, 4.0), pt(12.0, 12.0)), (pt(12.0, 4.0), pt(4.0, 12.0))]);
        assert_eq!(lines("M.5.5L1-2z").len(), 2);
    }

    #[test]
    fn an_arc_ends_where_it_says() {
        let got = lines("M13.2 8a5.2 5.2 0 1 1-1.6-3.7");
        let end = got.last().unwrap().1;
        assert!((end.x - 11.6).abs() < 0.01 && (end.y - 4.3).abs() < 0.01, "{end:?}");
        // every point lies on its circle
        let (cx, cy) = (8.0, 8.0);
        assert!(got.iter().all(|(_, p)| (((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt() - 5.2).abs() < 0.1));
    }

    fn near(p: Point, q: Point, by: f32) -> bool {
        (p.x - q.x).abs() <= by && (p.y - q.y).abs() <= by
    }
    fn near_rect(r: Rect, x: f32, y: f32, w: f32, h: f32) -> bool {
        [r.x - x, r.y - y, r.w - w, r.h - h].iter().all(|d| d.abs() < 1e-3)
    }

    #[test]
    fn round_shapes_lie_where_their_numbers_say() {
        // Every point of an ellipse lies on it (within a few millionths of its size, as near as
        // cubics come), and it reaches its radii, no further.
        let (mut ellipse, mut points) = (Path::new(), Vec::new());
        ellipse.ellipse(10.0, 20.0, 8.0, 4.0);
        ellipse.cut(false, FINELY, |_, b, _| points.push(b));
        assert!(points.iter().all(|p| (((p.x - 10.0) / 8.0).hypot((p.y - 20.0) / 4.0) - 1.0).abs() < 1e-5));
        assert!(near_rect(ellipse.bounds(), 2.0, 16.0, 16.0, 8.0), "{:?}", ellipse.bounds());
        assert_eq!(Path::new().circle(1.0, 2.0, 3.0), Path::new().ellipse(1.0, 2.0, 3.0, 3.0));
        // A pie slice: from the middle up, a quarter turn clockwise to the right, and back. Two
        // radii and a quarter circle long. (Its cubics match a circle to a few millionths of
        // its length.)
        let mut pie = Path::new();
        pie.move_to(50.0, 50.0).arc(50.0, 50.0, 20.0, -90.0, 90.0).close();
        let length = 40.0 + std::f32::consts::PI * 10.0;
        assert!((pie.length() - length).abs() < 0.01, "{}", pie.length());
        assert!(near(pie.point_at(20.0).unwrap().0, pt(50.0, 30.0), 1e-4) && near(pie.point_at(pie.length() - 20.0).unwrap().0, pt(70.0, 50.0), 1e-3));
        assert!(pie.contains(pt(60.0, 40.0)) && !pie.contains(pt(40.0, 40.0)) && !pie.contains(pt(60.0, 60.0)));
        assert!(near_rect(pie.bounds(), 50.0, 30.0, 20.0, 20.0), "{:?}", pie.bounds());
        // An arc in an empty shape starts its own subpath; a negative sweep goes anticlockwise;
        // and past a full turn is a full turn.
        let mut half = Path::new();
        half.arc(0.0, 0.0, 10.0, 0.0, -180.0);
        assert!(near_rect(half.bounds(), -10.0, -10.0, 20.0, 10.0), "{:?}", half.bounds());
        let mut whole = Path::new();
        whole.arc(0.0, 0.0, 10.0, 0.0, 720.0);
        assert!((whole.length() - std::f32::consts::TAU * 10.0).abs() < 0.01 && near(whole.point_at(1e9).unwrap().0, pt(10.0, 0.0), 1e-3));
        // After a line, an arc continues from the pen with a straight line.
        let mut after = Path::new();
        after.move_to(0.0, 0.0).line_to(5.0, 0.0).arc(20.0, 0.0, 10.0, 180.0, 180.0);
        assert!((after.length() - (10.0 + std::f32::consts::PI * 10.0)).abs() < 0.01);
    }

    #[test]
    fn rectangles_polygons_and_stars_have_their_corners_where_they_say() {
        let mut plain = Path::new();
        plain.rect(1.0, 2.0, 3.0, 4.0);
        assert_eq!(plain, Path::svg("M1 2H4V6H1Z"));
        // Each corner as round as given: a rectangle less what four corner squares lose to
        // their quarter circles. (The area is summed over the finely flattened outline; the
        // cubics match a quarter circle to a few millionths of its area.)
        let area = |path: &Path| {
            let mut twice = 0.0;
            path.cut(true, FINELY, |a, b, _| twice += a.x * b.y - b.x * a.y);
            twice / 2.0
        };
        let mut rounded = Path::new();
        rounded.rounded_rect(0.0, 0.0, 20.0, 10.0, (0.0, 2.0, 4.0, 6.0));
        let lost = (1.0 - std::f32::consts::FRAC_PI_4) * (4.0 + 16.0 + 36.0);
        assert!((area(&rounded) - (200.0 - lost)).abs() < 0.02, "{}", area(&rounded));
        assert!(rounded.contains(pt(0.05, 0.05)) && !rounded.contains(pt(19.9, 0.1)), "the square corner is square, the round one round");
        // Radii that would overlap are scaled down together: four of 8 on a side of 10 become
        // four of 5, a circle.
        let mut tight = Path::new();
        tight.rounded_rect(0.0, 0.0, 10.0, 10.0, 8.0);
        assert!((area(&tight) - std::f32::consts::PI * 25.0).abs() < 0.04, "{}", area(&tight));
        // One radius for all four: each corner three curves, the first starting where the side
        // before ends, the last ending exactly where the next side starts.
        let mut one = Path::new();
        one.rounded_rect(1.0, 1.0, 14.0, 10.0, 3.0);
        assert_eq!(one.steps[1], Step::Line(pt(12.0, 1.0)));
        assert!(matches!(one.steps[2..6], [Step::Cubic(..), Step::Cubic(..), Step::Cubic(_, _, end), Step::Line(next)] if end == pt(15.0, 4.0) && next == pt(15.0, 8.0)));
        // A hexagon, its first corner straight up; and a star whose points and inner corners
        // lie at their given distances.
        let mut six = Path::new();
        six.polygon(10.0, 10.0, 4.0, 6);
        let corners: Vec<Point> = (0..6)
            .map(|i| {
                let a = (i as f32 * 60.0 - 90.0).to_radians();
                pt(10.0 + 4.0 * a.cos(), 10.0 + 4.0 * a.sin())
            })
            .collect();
        assert!(six.steps.len() == 7 && six.steps[..6].iter().zip(&corners).all(|(step, corner)| matches!(step, Step::Move(p) | Step::Line(p) if near(*p, *corner, 1e-5))));
        assert!((six.length() - 24.0).abs() < 1e-4, "six sides as long as the radius");
        let mut star = Path::new();
        star.star(0.0, 0.0, 10.0, 4.0, 5);
        let far: Vec<f32> = star.steps[..10].iter().map(|step| if let Step::Move(p) | Step::Line(p) = step { p.x.hypot(p.y) } else { 0.0 }).collect();
        assert!(far.iter().enumerate().all(|(i, far)| (far - [10.0, 4.0][i % 2]).abs() < 1e-5) && near(star.point_at(0.0).unwrap().0, pt(0.0, -10.0), 1e-5));
        assert!(star.contains(pt(0.0, 0.0)) && star.contains(pt(0.0, -8.0)) && !star.contains(pt(3.5, -7.0)));
        assert_eq!((Path::new().polygon(0.0, 0.0, 1.0, 2).clone(), Path::new().star(0.0, 0.0, 2.0, 1.0, 1).clone()), (Path::new(), Path::new()));
    }

    #[test]
    fn shapes_are_put_together_turned_round_and_moved() {
        // A ring: a circle, and a smaller one reversed, which cuts a hole.
        let (mut ring, mut both) = (Path::new(), Path::new());
        ring.circle(10.0, 10.0, 8.0).add(Path::new().circle(10.0, 10.0, 4.0).reverse());
        both.circle(10.0, 10.0, 8.0).add(Path::new().circle(10.0, 10.0, 4.0));
        assert!(ring.contains(pt(10.0, 3.0)) && !ring.contains(pt(10.0, 10.0)) && both.contains(pt(10.0, 10.0)));
        // Reversed, a shape covers the same, is as long, and starts where it ended (a closed
        // one where it started); twice, it is the same shape again.
        for d in ["M2 2h10v10z", "M0 0c10 0 10 10 20 10q5 0 5-5", "M0 0h4M10 10l3 3 0 4z", "M1 1 5 1Z L 9 9"] {
            let shape = Path::svg(d);
            let mut back = shape.clone();
            back.reverse();
            assert!((back.length() - shape.length()).abs() < 1e-3, "{d}");
            assert!((0..400).map(|i| pt((i % 20) as f32 + 0.3, (i / 20) as f32 + 0.7)).all(|p| back.contains(p) == shape.contains(p)), "{d}");
            back.reverse();
            assert!((back.length() - shape.length()).abs() < 1e-3 && back.bounds() == shape.bounds(), "{d}");
        }
        let mut open = Path::svg("M0 0c10 0 10 10 20 10q5 0 5-5");
        let end = open.point_at(1e9).unwrap().0;
        assert!(near(open.reverse().point_at(0.0).unwrap().0, end, 1e-5));
        // Transformed, a shape is the one through its transformed points: a square turned an
        // eighth stands on a corner; a circle scaled one way is an ellipse.
        let mut square = Path::new();
        square.rect(-1.0, -1.0, 2.0, 2.0).transform(Transform::new().rotate(45.0).translate(5.0, 5.0));
        let half = std::f32::consts::SQRT_2;
        assert!(near_rect(square.bounds(), 5.0 - half, 5.0 - half, 2.0 * half, 2.0 * half), "{:?}", square.bounds());
        // (Up to the last few bits, where the two computations round differently.)
        let (mut stretched, ellipse) = (Path::new(), Path::new().ellipse(0.0, 0.0, 6.0, 2.0).clone());
        stretched.circle(0.0, 0.0, 2.0).transform(Transform::new().scale(3.0, 1.0));
        let points = |step: &Step| match *step {
            Step::Move(p) | Step::Line(p) => vec![p],
            Step::Quad(a, p) => vec![a, p],
            Step::Cubic(a, b, p) => vec![a, b, p],
            Step::Close => vec![],
        };
        let alike = |a: &Step, b: &Step| std::mem::discriminant(a) == std::mem::discriminant(b) && points(a).iter().zip(points(b)).all(|(p, q)| near(*p, q, 1e-5));
        assert!(stretched.steps.len() == ellipse.steps.len() && stretched.steps.iter().zip(&ellipse.steps).all(|(a, b)| alike(a, b)), "{stretched:?}");
    }

    #[test]
    fn what_is_inside_a_shape_is_what_the_canvas_fills() {
        // Every pixel the canvas covers fully, or not at all, has its centre inside the shape,
        // or outside: for shapes of lines, curves, holes and overlapping subpaths.
        let shapes = [
            Path::new().star(32.0, 32.0, 30.0, 12.0, 5).clone(),
            Path::new().circle(32.0, 32.0, 28.0).add(Path::new().circle(36.0, 30.0, 12.0).reverse()).clone(),
            Path::new().circle(24.0, 32.0, 18.0).add(Path::new().circle(40.0, 32.0, 18.0)).clone(),
            Path::svg("M4 60C4 4 60 4 60 60Q32 20 4 60Z"),
            Path::new().rounded_rect(3.0, 5.0, 58.0, 50.0, (20.0, 0.0, 10.0, 30.0)).clone(),
        ];
        for shape in shapes {
            let mut pixels = vec![0u32; 64 * 64];
            crate::canvas::Canvas::new(&mut pixels, 64, 64, (0, 0), 1.0).fill(&shape, pt(0.0, 0.0), 1.0, crate::color::Color::WHITE);
            let (mut whole, mut none) = (0, 0);
            for (i, pixel) in pixels.iter().enumerate() {
                let inside = shape.contains(pt((i % 64) as f32 + 0.5, (i / 64) as f32 + 0.5));
                match *pixel >> 24 {
                    255 => (whole, _) = (whole + 1, assert!(inside, "pixel {i}")),
                    0 => (none, _) = (none + 1, assert!(!inside, "pixel {i}")),
                    _ => {}
                }
            }
            assert!(whole > 300 && none > 300);
        }
        // An open subpath counts as closed; nothing is inside an empty shape.
        assert!(Path::svg("M0 0h10v10").contains(pt(8.0, 2.0)) && !Path::svg("M0 0h10v10").contains(pt(2.0, 8.0)));
        assert!(!Path::new().contains(pt(0.0, 0.0)));
    }

    /// A cubic's length the slow way: a million steps along it, in double precision.
    fn measured([p0, a, b, p1]: [[f64; 2]; 4]) -> f64 {
        let on = |t: f64, i: usize| {
            let u = 1.0 - t;
            u * u * u * p0[i] + 3.0 * u * u * t * a[i] + 3.0 * u * t * t * b[i] + t * t * t * p1[i]
        };
        (0..1_000_000).map(|i| (i as f64 / 1e6, (i + 1) as f64 / 1e6)).map(|(s, t)| (on(t, 0) - on(s, 0)).hypot(on(t, 1) - on(s, 1))).sum()
    }

    #[test]
    fn a_shape_is_as_long_and_as_large_as_measured() {
        // Straight lines: exact.
        assert_eq!(Path::svg("M0 0l3 4").length(), 5.0);
        assert_eq!(Path::new().rect(0.0, 0.0, 3.0, 4.0).length(), 14.0);
        assert_eq!((Path::new().length(), Path::svg("M3 3").length(), Path::svg("M3 3").bounds()), (0.0, 0.0, Rect::EMPTY));
        // Curves: as the slow way says, to a hundred-thousandth.
        for c in [[[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]], [[0.0, 0.0], [40.0, -30.0], [-20.0, 30.0], [30.0, 5.0]], [[1.0, 1.0], [1.0, 1.0], [9.0, 2.0], [9.0, 2.0]]] {
            let mut curve = Path::new();
            curve.move_to(c[0][0] as f32, c[0][1] as f32).cubic_to(c[1][0] as f32, c[1][1] as f32, c[2][0] as f32, c[2][1] as f32, c[3][0] as f32, c[3][1] as f32);
            let want = measured(c) as f32;
            assert!((curve.length() - want).abs() < want * 1e-5, "{} against {want}", curve.length());
        }
        // A cubic reaches as far as it really goes: halfway, three quarters of the way to its
        // control points.
        assert!(near_rect(Path::svg("M0 0C0 10 10 10 10 0").bounds(), 0.0, 0.0, 10.0, 7.5));
        assert!(near_rect(Path::svg("M0 0Q5 10 10 0").bounds(), 0.0, 0.0, 10.0, 5.0));
        // And as far as a million points along each cubic, found the slow way, reach.
        for c in [[[0.0, 0.0], [40.0, 0.0], [-30.0, 10.0], [10.0, 10.0]], [[0.0, 0.0], [20.0, 0.0], [-10.0, 0.0], [10.0, 0.0]], [[5.0, 5.0], [-9.0, 20.0], [30.0, -12.0], [6.0, 4.0]]] {
            let on = |t: f64, i: usize| {
                let u = 1.0 - t;
                u * u * u * c[0][i] + 3.0 * u * u * t * c[1][i] + 3.0 * u * t * t * c[2][i] + t * t * t * c[3][i]
            };
            let reach = |i: usize| (0..=1_000_000).map(|n| on(n as f64 / 1e6, i)).fold([f64::MAX, f64::MIN], |[least, most], at| [least.min(at), most.max(at)]);
            let ([x0, x1], [y0, y1]) = (reach(0), reach(1));
            let mut curve = Path::new();
            curve.move_to(c[0][0] as f32, c[0][1] as f32).cubic_to(c[1][0] as f32, c[1][1] as f32, c[2][0] as f32, c[2][1] as f32, c[3][0] as f32, c[3][1] as f32);
            let r = curve.bounds();
            assert!(near_rect(r, x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32), "{r:?} against {:?}", [x0, y0, x1, y1]);
        }
    }

    #[test]
    fn points_along_a_shape_and_how_far_points_are_from_it() {
        // Along a line, its direction; at a corner, the next segment's.
        let corner = Path::svg("M0 0h10v10");
        assert_eq!(corner.point_at(5.0), Some((pt(5.0, 0.0), 0.0)));
        assert_eq!(corner.point_at(15.0), Some((pt(10.0, 5.0), 90.0)));
        assert_eq!(corner.point_at(-3.0), Some((pt(0.0, 0.0), 0.0)));
        assert_eq!(corner.point_at(99.0), Some((pt(10.0, 10.0), 90.0)));
        assert_eq!((Path::new().point_at(1.0), Path::svg("M4 4h0").point_at(1.0)), (None, Some((pt(4.0, 4.0), 0.0))));
        // Round a circle from its right, clockwise on screen: a quarter of the way it is at the
        // bottom, going left; each point lies on the circle, tangent to it.
        let mut circle = Path::new();
        circle.circle(0.0, 0.0, 10.0);
        let whole = circle.length();
        let (bottom, way) = circle.point_at(whole / 4.0).unwrap();
        assert!(near(bottom, pt(0.0, 10.0), 0.01) && (way - 180.0).abs() < 0.05, "{bottom:?} {way}");
        for part in 0..32 {
            let (p, way) = circle.point_at(whole * part as f32 / 32.0).unwrap();
            let round = (p.y.atan2(p.x).to_degrees() + 90.0 - way).rem_euclid(360.0);
            assert!((p.x.hypot(p.y) - 10.0).abs() < 0.003 && !(0.1..359.9).contains(&round), "{p:?} {way}");
        }
        // Distances from a line, a corner and a circle.
        assert_eq!(corner.distance(pt(5.0, 3.0)), 3.0);
        assert_eq!(corner.distance(pt(13.0, 14.0)), 5.0);
        assert!((circle.distance(pt(3.0, 4.0)) - 5.0).abs() < 0.005 && (circle.distance(pt(30.0, 0.0)) - 20.0).abs() < 0.005);
        assert_eq!(Path::new().distance(pt(0.0, 0.0)), f32::INFINITY);
    }

    #[test]
    fn a_smooth_line_goes_through_its_points_without_loops() {
        // Through every point in turn, and smoothly through each: the control points either
        // side of a point are collinear with it.
        let points = [(0.0, 0.0), (30.0, 20.0), (60.0, 0.0), (90.0, 25.0)];
        let mut line = Path::new();
        line.smooth_through(&points);
        let ends: Vec<Point> = line.steps.iter().map(|step| if let Step::Move(p) | Step::Cubic(_, _, p) = step { *p } else { pt(f32::NAN, 0.0) }).collect();
        assert_eq!(ends, points.map(|(x, y)| pt(x, y)));
        for at in 1..3 {
            let (Step::Cubic(_, before, p), Step::Cubic(after, _, _)) = (line.steps[at], line.steps[at + 1]) else { panic!("{:?}", line.steps) };
            let turn = (p.x - before.x) * (after.y - p.y) - (p.y - before.y) * (after.x - p.x);
            assert!(turn.abs() < 1e-3, "a corner at {p:?}: {turn}");
        }
        // Two close points between two far ones: the curve between them stays near them, where
        // uniform Catmull-Rom would loop out by tens of units.
        let mut uneven = Path::new();
        uneven.smooth_through(&[(0.0, 0.0), (100.0, 0.0), (101.0, 1.0), (200.0, 0.0)]);
        let mut middle = Path::new();
        middle.move_to(100.0, 0.0).add(&Path { steps: vec![uneven.steps[2]] });
        let reach = middle.bounds();
        assert!(reach.x > 99.0 && reach.right() < 102.0 && reach.y > -1.0 && reach.bottom() < 2.0, "{reach:?}");
        // Two points are a straight line; one, an empty subpath; and after a line, a line leads
        // to the first point.
        let mut two = Path::new();
        two.smooth_through(&[(0.0, 0.0), (10.0, 10.0)]);
        assert!((two.length() - 200f32.sqrt()).abs() < 1e-3 && two.distance(pt(5.0, 5.0)) < 1e-4);
        assert_eq!(Path::new().smooth_through(&[(3.0, 4.0)]).steps, [Step::Move(pt(3.0, 4.0))]);
        assert_eq!(Path::new().move_to(0.0, 0.0).smooth_through(&[(5.0, 0.0), (9.0, 0.0)]).steps[1], Step::Line(pt(5.0, 0.0)));
        // A list ending where it began closes smoothly through its start: a square's four
        // corners make a round shape, longer than the square, shorter than the circle through
        // them.
        let mut round = Path::new();
        round.smooth_through(&[(10.0, 0.0), (20.0, 10.0), (10.0, 20.0), (0.0, 10.0), (10.0, 0.0)]);
        assert_eq!((round.steps.len(), round.steps.last()), (6, Some(&Step::Close)));
        let (Step::Cubic(_, before, _), Step::Cubic(after, _, _)) = (round.steps[4], round.steps[1]) else { panic!() };
        assert!((before.y - after.y).abs() < 1e-4, "smooth through its start: {before:?} {after:?}");
        assert!(round.length() > 4.0 * 200f32.sqrt() + 1.0 && round.length() < std::f32::consts::TAU * 10.0, "{}", round.length());
        assert!(round.contains(pt(10.0, 10.0)) && round.contains(pt(10.0, 1.0)) && !round.contains(pt(1.0, 1.0)));
    }

    #[test]
    fn lines_with_rounded_corners_are_as_long_and_as_large_as_measured() {
        // A right angle with radius 3: two lines 7 long, and a quarter circle.
        let quarter = std::f32::consts::FRAC_PI_2;
        let mut bend = Path::new();
        bend.rounded_through(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 3.0);
        assert!((bend.length() - (14.0 + quarter * 3.0)).abs() < 1e-3, "{}", bend.length());
        assert!(near_rect(bend.bounds(), 0.0, 0.0, 10.0, 10.0) && bend.distance(pt(10.0 - 3.0 + 3.0 * 0.5f32.sqrt(), 3.0 - 3.0 * 0.5f32.sqrt())) < 1e-3);
        // Turning the other way, it bends the other way; and a radius too large for its lines
        // takes half of each.
        let mut other = Path::new();
        other.rounded_through(&[(0.0, 10.0), (10.0, 10.0), (10.0, 0.0)], 3.0);
        assert!((other.length() - (14.0 + quarter * 3.0)).abs() < 1e-3 && other.distance(pt(7.0 + 3.0 * 0.5f32.sqrt(), 7.0 + 3.0 * 0.5f32.sqrt())) < 1e-3);
        let mut tight = Path::new();
        tight.rounded_through(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)], 10.0);
        assert!((tight.length() - (4.0 + quarter * 2.0)).abs() < 1e-3, "{}", tight.length());
        // Straight on, or with no radius, a corner is a corner.
        let mut straight = Path::new();
        straight.rounded_through(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)], 2.0);
        assert_eq!(straight.steps, [Step::Move(pt(0.0, 0.0)), Step::Line(pt(5.0, 0.0)), Step::Line(pt(10.0, 0.0))]);
        assert_eq!(Path::new().rounded_through(&[(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)], 0.0).steps.len(), 3);
        // A closed square with every corner rounded, the first too: what a square has over a
        // quarter circle, lost at each of its four corners.
        let mut square = Path::new();
        square.rounded_through(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)], 2.0);
        let mut twice = 0.0;
        square.cut(true, FINELY, |a, b, _| twice += a.x * b.y - b.x * a.y);
        assert!((twice / 2.0 - (100.0 - (4.0 - std::f32::consts::PI) * 4.0)).abs() < 0.01, "{}", twice / 2.0);
        assert!(square.contains(pt(5.0, 0.2)) && !square.contains(pt(0.2, 0.2)) && !square.contains(pt(9.8, 9.8)));
    }
}
