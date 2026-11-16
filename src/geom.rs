//! Points, sizes, rectangles and transforms, in units: one unit is one pixel at 100% scale.

/// A point, in units right and down from a top left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub struct Point {
    /// Right.
    pub x: f32,
    /// Down.
    pub y: f32,
}

/// A size, in units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub struct Size {
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// A rectangle: its top left corner and size, in units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// Space on the four sides of something.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub struct Insets {
    /// At the left.
    pub left: f32,
    /// At the top.
    pub top: f32,
    /// At the right.
    pub right: f32,
    /// At the bottom.
    pub bottom: f32,
}

/// A [`Point`].
pub const fn pt(x: f32, y: f32) -> Point {
    Point { x, y }
}
/// A [`Rect`]: left, top, width, height.
pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    /// An empty rectangle at the origin.
    pub const EMPTY: Rect = rect(0.0, 0.0, 0.0, 0.0);
    /// Larger than anything ever drawn: "no clipping".
    pub const EVERYTHING: Rect = rect(-1e9, -1e9, 2e9, 2e9);

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// Width and height.
    pub fn size(&self) -> Size {
        Size { w: self.w, h: self.h }
    }
    /// Whether it has no width or no height.
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    /// Whether the point is inside. All four sides are tested at once, without branches: this
    /// runs for every box the pointer could be over, on every move.
    #[inline(always)]
    pub fn contains(&self, p: Point) -> bool {
        (p.x >= self.x) & (p.x < self.right()) & (p.y >= self.y) & (p.y < self.bottom())
    }
    /// The overlap of both; empty if they do not touch.
    #[must_use]
    pub fn intersect(&self, o: Rect) -> Rect {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        rect(x, y, (self.right().min(o.right()) - x).max(0.0), (self.bottom().min(o.bottom()) - y).max(0.0))
    }
    /// The smallest rectangle holding both. An empty one adds nothing.
    #[must_use]
    pub fn union(&self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return *self;
        }
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        rect(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    /// Shrunk by the insets (grown, with negative ones).
    #[must_use]
    pub fn inset(&self, by: Insets) -> Rect {
        rect(self.x + by.left, self.y + by.top, (self.w - by.left - by.right).max(0.0), (self.h - by.top - by.bottom).max(0.0))
    }
    /// Grown just enough for its edges to lie on whole pixels at `scale` pixels per unit: the
    /// area to redraw.
    #[must_use]
    pub fn on_pixels(&self, scale: f32) -> Rect {
        let (x, y) = ((self.x * scale).floor() / scale, (self.y * scale).floor() / scale);
        rect(x, y, (self.right() * scale).ceil() / scale - x, (self.bottom() * scale).ceil() / scale - y)
    }
    /// Grown by `by` on every side (shrunk, if negative).
    #[must_use]
    pub fn grow(&self, by: f32) -> Rect {
        rect(self.x - by, self.y - by, self.w + 2.0 * by, self.h + 2.0 * by)
    }
    /// Moved right `dx` and down `dy`.
    #[must_use]
    pub fn moved(&self, dx: f32, dy: f32) -> Rect {
        rect(self.x + dx, self.y + dy, self.w, self.h)
    }
}

impl Insets {
    /// No room on any side.
    pub const ZERO: Insets = Insets::all(0.0);

    /// The same on every side.
    pub const fn all(v: f32) -> Self {
        Insets { left: v, top: v, right: v, bottom: v }
    }
    /// `x` on the left and right, `y` on the top and bottom.
    pub const fn xy(x: f32, y: f32) -> Self {
        Insets { left: x, top: y, right: x, bottom: y }
    }
    /// Left plus right.
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    /// Top plus bottom.
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

impl From<f32> for Insets {
    fn from(v: f32) -> Self {
        Insets::all(v)
    }
}
impl From<(f32, f32)> for Insets {
    fn from((x, y): (f32, f32)) -> Self {
        Insets::xy(x, y)
    }
}
impl From<(f32, f32, f32, f32)> for Insets {
    fn from((left, top, right, bottom): (f32, f32, f32, f32)) -> Self {
        Insets { left, top, right, bottom }
    }
}

/// Corner radii, in units. One number sets all four; a tuple of four goes clockwise from the
/// top left, as CSS `border-radius` does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners {
    /// Top left.
    pub top_left: f32,
    /// Top right.
    pub top_right: f32,
    /// Bottom right.
    pub bottom_right: f32,
    /// Bottom left.
    pub bottom_left: f32,
}

impl From<f32> for Corners {
    fn from(r: f32) -> Self {
        Corners { top_left: r, top_right: r, bottom_right: r, bottom_left: r }
    }
}
impl From<(f32, f32, f32, f32)> for Corners {
    fn from((top_left, top_right, bottom_right, bottom_left): (f32, f32, f32, f32)) -> Self {
        Corners { top_left, top_right, bottom_right, bottom_left }
    }
}

/// A 2D affine transform: rotate, scale, move and skew, each step after the ones before.
/// `Transform::new().scale(2.0, 2.0).turn(90.0)` doubles a shape, then turns it a quarter.
///
/// `(x, y)` maps to `(a x + c y + e, b x + d y + f)`, with `a` to `f` in order, as SVG's
/// `matrix(..)` writes them.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
pub struct Transform(pub [f32; 6]);

impl Default for Transform {
    fn default() -> Self {
        Transform::new()
    }
}

impl Transform {
    /// The identity.
    pub const fn new() -> Self {
        Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }
    /// This, then `next`.
    pub fn then(self, next: Transform) -> Self {
        let ([a, b, c, d, e, f], [p, q, r, s, t, u]) = (self.0, next.0);
        Transform([p * a + r * b, q * a + s * b, p * c + r * d, q * c + s * d, p * e + r * f + t, q * e + s * f + u])
    }
    /// This, then rotated `degrees` clockwise (y grows down) about the origin.
    pub fn turn(self, degrees: f32) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        self.then(Transform([cos, sin, -sin, cos, 0.0, 0.0]))
    }
    /// This, then scaled by `x` and `y` about the origin. Negative mirrors.
    pub fn scale(self, x: f32, y: f32) -> Self {
        self.then(Transform([x, 0.0, 0.0, y, 0.0, 0.0]))
    }
    /// This, then moved right `x` and down `y`.
    pub fn move_by(self, x: f32, y: f32) -> Self {
        self.then(Transform([1.0, 0.0, 0.0, 1.0, x, y]))
    }
    /// This, then skewed: verticals lean `x` degrees, horizontals `y` degrees, as CSS
    /// `skew(x, y)`.
    pub fn slant(self, x: f32, y: f32) -> Self {
        self.then(Transform([1.0, y.to_radians().tan(), x.to_radians().tan(), 1.0, 0.0, 0.0]))
    }
    /// The inverse; `None` if it flattens everything onto a line or point.
    pub fn undone(self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.0;
        let area = a * d - b * c;
        area.is_normal().then(|| Transform([d / area, -b / area, -c / area, a / area, (c * f - d * e) / area, (b * e - a * f) / area]))
    }
    /// Where it maps `p`.
    #[inline]
    pub fn apply(self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        pt(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(p: Point, q: Point) -> bool {
        (p.x - q.x).abs() < 1e-4 && (p.y - q.y).abs() < 1e-4
    }

    #[test]
    fn a_transform_does_its_steps_in_the_order_they_are_said() {
        let p = pt(3.0, 1.0);
        // A quarter turn clockwise takes right to down.
        assert!(near(Transform::new().turn(90.0).apply(pt(1.0, 0.0)), pt(0.0, 1.0)));
        assert!(near(Transform::new().turn(90.0).apply(pt(0.0, 1.0)), pt(-1.0, 0.0)));
        // Move-then-scale differs from scale-then-move.
        assert!(near(Transform::new().move_by(1.0, 2.0).scale(2.0, 2.0).apply(p), pt(8.0, 6.0)));
        assert!(near(Transform::new().scale(2.0, 2.0).move_by(1.0, 2.0).apply(p), pt(7.0, 4.0)));
        // Turned about a point: move there, turn, move back.
        let round_middle = Transform::new().move_by(-5.0, -5.0).turn(180.0).move_by(5.0, 5.0);
        assert!(near(round_middle.apply(pt(6.0, 5.0)), pt(4.0, 5.0)) && near(round_middle.apply(pt(5.0, 5.0)), pt(5.0, 5.0)));
        // Skewed 45 degrees: a point moves right as far as it is down.
        assert!(near(Transform::new().slant(45.0, 0.0).apply(pt(0.0, 2.0)), pt(2.0, 2.0)));
        assert!(near(Transform::new().slant(0.0, 45.0).apply(pt(2.0, 0.0)), pt(2.0, 2.0)));
        // `then` equals the steps in sequence.
        let (one, two) = (Transform::new().turn(30.0).scale(2.0, 0.5), Transform::new().slant(10.0, 0.0).move_by(4.0, -1.0));
        let joined = one.then(two);
        let stepped = Transform::new().turn(30.0).scale(2.0, 0.5).slant(10.0, 0.0).move_by(4.0, -1.0);
        assert!(near(joined.apply(p), two.apply(one.apply(p))) && near(stepped.apply(p), joined.apply(p)));
        // Inverted, every point comes back; a flattening transform has no inverse.
        let back = joined.undone().unwrap();
        assert!([pt(0.0, 0.0), p, pt(-7.0, 12.5)].iter().all(|q| near(back.apply(joined.apply(*q)), *q)));
        assert_eq!(Transform::new().scale(1.0, 0.0).undone(), None);
        assert_eq!(Transform::default(), Transform::new());
    }

    #[test]
    fn corners_are_said_by_one_number_or_four() {
        assert_eq!(Corners::from(3.0), Corners { top_left: 3.0, top_right: 3.0, bottom_right: 3.0, bottom_left: 3.0 });
        assert_eq!(Corners::from((1.0, 2.0, 3.0, 4.0)), Corners { top_left: 1.0, top_right: 2.0, bottom_right: 3.0, bottom_left: 4.0 });
    }
}
