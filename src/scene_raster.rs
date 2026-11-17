//! Triangle rasterization: what scenes are drawn with. A million-triangle mesh draws in a few
//! milliseconds, on every core.
//!
//! The image is split into row bands. For each mesh, every core first projects a share of its
//! points, then bins a share of its triangles by the bands they touch; then each band is filled
//! by one core at a time, so no two ever write the same pixel, and the result is the same
//! whatever the core count. A pixel belongs to a triangle when its centre lies inside, and a
//! shared edge is computed identically for both triangles: neighbours leave no gap and never
//! both take a pixel.
use crate::color::Color;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Barrier, Mutex, RwLock};

/// A 4×4 matrix: a point (with w = 1) is multiplied row by row.
pub(crate) type Matrix = [[f32; 4]; 4];

/// Below this many triangles and edges, waking other cores is not worth it.
const ALONE: usize = 6000;
/// The minimum rows per band: bands are what cores share out.
const BAND: usize = 16;
/// Beyond this distance in pixels, a corner is clipped to the guard band first: past it, an
/// edge's row crossing could no longer be computed to a fraction of a pixel.
const FAR_OUT: f32 = 30_000.0;
/// An outline is taken as this much nearer than its triangle, relative to its depth: so it
/// shows on its triangle and not through what is in front.
const LIFT: f32 = 0.002;

/// One mesh, transformed and coloured, ready to draw. (Every triangle corner and edge end is a
/// point index, and colours are per point or absent: the caller ensures that.)
#[derive(Clone, Copy)]
pub(crate) struct Piece<'a> {
    pub points: &'a [[f32; 3]],
    /// A colour per point (`0xAARRGGBB`), or none: then `ink` colours it all.
    pub colors: &'a [u32],
    pub triangles: &'a [[u32; 3]],
    /// Each triangle's brightness, 255 for full colour (see `shades_of`); empty for all full.
    pub shades: &'a [u8],
    /// The edges to outline, each once (see `edges_of`); empty for none.
    pub lines: &'a [[u32; 2]],
    /// From the mesh's own coordinates to clip space: x and y from -w (left, bottom) to w, z
    /// from 0 (nearest) to w (furthest).
    pub to_view: Matrix,
    /// The triangle fill (`0xAARRGGBB`); nothing if fully transparent.
    pub ink: u32,
    /// The outline colour.
    pub edges: u32,
    /// Only the side whose corners run clockwise shows (back-face culling).
    pub one_sided: bool,
}

/// Every edge of a mesh's triangles, once: for outlines. (Most edges are shared; drawn for both
/// triangles, one would show twice as strong as a rim edge.)
pub(crate) fn edges_of(triangles: &[[u32; 3]]) -> Vec<[u32; 2]> {
    let ends = triangles.iter().flat_map(|[a, b, c]| [(*a, *b), (*b, *c), (*c, *a)]).filter(|(from, to)| from != to);
    let mut edges: Vec<u64> = ends.map(|(from, to)| (from.min(to) as u64) << 32 | from.max(to) as u64).collect();
    edges.sort_unstable();
    edges.dedup();
    edges.into_iter().map(|edge| [(edge >> 32) as u32, edge as u32]).collect()
}

/// Each triangle's brightness under light from `light` (a unit direction in mesh coordinates):
/// 255 facing it, a third of that edge-on. Lit from behind as from the front: a sheet has no
/// dark side.
pub(crate) fn shades_of(points: &[[f32; 3]], triangles: &[[u32; 3]], light: [f32; 3], workers: usize) -> Vec<u8> {
    let mut shades = vec![0u8; triangles.len()];
    let share = triangles.len().div_ceil(workers.max(1)).max(1);
    let shares = triangles.chunks(share).zip(shades.chunks_mut(share));
    let some = |(triangles, shades): (&[[u32; 3]], &mut [u8])| triangles.iter().zip(shades).for_each(|(corners, shade)| *shade = shade_of(corners.map(|corner| points[corner as usize]), light));
    match triangles.len() < ALONE || workers < 2 {
        true => shares.for_each(some),
        false => std::thread::scope(|scope| shares.for_each(|share| drop(scope.spawn(move || some(share))))),
    }
    shades
}

/// One triangle's brightness in that light.
fn shade_of([a, b, c]: [[f32; 3]; 3], light: [f32; 3]) -> u8 {
    let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    let up = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let long = (up[0] * up[0] + up[1] * up[1] + up[2] * up[2]).sqrt();
    let facing = if long > 0.0 { (up[0] * light[0] + up[1] * light[1] + up[2] * light[2]).abs() / long } else { 0.0 };
    (255.0 * (0.34 + 0.66 * facing.min(1.0))) as u8
}

/// What a core binned for the bands: the triangles and edges touching each, and those with a
/// vertex behind the eye or far outside the image, which are clipped.
#[derive(Default)]
struct Sorted {
    triangles: Vec<Vec<u32>>,
    cut: Vec<u32>,
    lines: Vec<Vec<u32>>,
    cut_lines: Vec<u32>,
}

/// One band of the image: its index, its rows from `top`, and each pixel's depth.
struct Band<'a> {
    number: usize,
    top: usize,
    pixels: &'a mut [u32],
    depth: &'a mut [f32],
}

/// The image a scene is drawn into, kept between draws.
#[derive(Default)]
pub(crate) struct Target {
    pub width: usize,
    pub height: usize,
    /// Rows of premultiplied `0xAARRGGBB`.
    pub pixels: Vec<u32>,
    depth: Vec<f32>,
    /// Every point of the mesh being drawn, projected: x and y in pixels, depth, and 1/w (0 for
    /// a point behind the eye). Stored as bits so every core can write its share and all can
    /// read all.
    shown: Vec<[AtomicU32; 4]>,
    sorted: Vec<RwLock<Sorted>>,
}

/// A projected triangle corner: x, y, depth, 1/w.
type Corner = [f32; 4];

/// A corner being clipped: its clip-space position before the perspective divide, and its
/// colour as blue, green, red and alpha.
type Loose = [f32; 8];

/// How a triangle is coloured.
#[derive(Clone, Copy)]
enum Paint {
    /// One opaque colour.
    Solid(u32),
    /// One translucent colour.
    Sheer(u32),
    /// A colour per corner, interpolated across the triangle.
    Blend([u32; 3]),
}

/// One draw of the image, as every working core sees it.
struct Frame<'a> {
    pieces: &'a [Piece<'a>],
    shown: &'a [[AtomicU32; 4]],
    sorted: &'a [RwLock<Sorted>],
    bands: &'a [Mutex<Band<'a>>],
    size: [usize; 2],
    /// Rows per band.
    rows: usize,
    ground: u32,
    /// For clearing and for each mesh: the next unclaimed band.
    next: &'a [AtomicUsize],
    /// Where cores wait for each other between steps.
    step: Barrier,
}

impl Target {
    /// Redraws the image, `width` by `height` pixels: all `ground` (premultiplied, as stored),
    /// then the meshes in order, each depth-tested against what is there, on `workers` cores.
    pub fn render(&mut self, [width, height]: [usize; 2], ground: u32, pieces: &[Piece], workers: usize) {
        (self.width, self.height) = (width, height);
        self.pixels.resize(width * height, 0);
        self.depth.resize(width * height, 0.0);
        if width == 0 || height == 0 {
            return;
        }
        let work: usize = pieces.iter().map(|piece| piece.triangles.len() + piece.lines.len()).sum();
        let workers = if work < ALONE { 1 } else { workers.max(1) };
        let points = pieces.iter().map(|piece| piece.points.len()).max().unwrap_or(0);
        self.shown.resize_with(points.max(self.shown.len()), Default::default);
        self.sorted.resize_with(workers.max(self.sorted.len()), Default::default);
        // (Four bands per core, so one with little in it does not leave a core idle; none
        // shorter than `BAND` rows.)
        let rows = height.div_ceil(workers * 4).max(BAND);
        let chunks = self.pixels.chunks_mut(rows * width).zip(self.depth.chunks_mut(rows * width)).enumerate();
        let bands: Vec<_> = chunks.map(|(number, (pixels, depth))| Mutex::new(Band { number, top: number * rows, pixels, depth })).collect();
        let next: Vec<AtomicUsize> = (0..=pieces.len()).map(|_| AtomicUsize::new(0)).collect();
        let frame = Frame { pieces, shown: &self.shown, sorted: &self.sorted[..workers], bands: &bands, size: [width, height], rows, ground, next: &next, step: Barrier::new(workers) };
        // (The calling thread is the first worker: one fewer to start.)
        let frame = &frame;
        std::thread::scope(|scope| {
            (1..workers).for_each(|worker| drop(scope.spawn(move || frame.work(worker))));
            frame.work(0);
        });
    }
}

impl Frame<'_> {
    /// One core's work: clears bands, then for each mesh projects a share of its points, bins a
    /// share of its triangles and fills bands, in step with the others.
    fn work(&self, worker: usize) {
        self.each_band(0, |band| band.clear(self.ground));
        self.step.wait();
        for (number, piece) in self.pieces.iter().enumerate() {
            // (A small mesh is one core's work: the others wait.)
            let alone = piece.triangles.len() + piece.lines.len() < ALONE || self.sorted.len() == 1;
            match (alone, worker) {
                (true, 0) => self.whole(number, piece),
                (true, _) => {}
                (false, _) => self.share(number, piece, worker),
            }
            self.step.wait();
        }
    }

    /// A whole mesh, by this core.
    fn whole(&self, number: usize, piece: &Piece) {
        place(piece, 0..piece.points.len(), self.size, self.shown);
        sort(piece, self.shown, [0, 1], self.size, self.rows, &self.sorted[0]);
        self.each_band(number + 1, |band| fill_band(piece, self.shown, &self.sorted[..1], self.size, band));
    }

    /// This core's share of a mesh.
    fn share(&self, number: usize, piece: &Piece, worker: usize) {
        let (workers, points) = (self.sorted.len(), piece.points.len());
        let each = points.div_ceil(workers);
        place(piece, (worker * each).min(points)..((worker + 1) * each).min(points), self.size, self.shown);
        self.step.wait();
        sort(piece, self.shown, [worker, workers], self.size, self.rows, &self.sorted[worker]);
        self.step.wait();
        self.each_band(number + 1, |band| fill_band(piece, self.shown, self.sorted, self.size, band));
    }

    /// Claims step `step`'s unclaimed bands one after another, doing `work` on each.
    fn each_band(&self, step: usize, mut work: impl FnMut(&mut Band)) {
        while let Some(band) = self.bands.get(self.next[step].fetch_add(1, Relaxed)) {
            work(&mut band.lock().unwrap_or_else(|held| held.into_inner()));
        }
    }
}

impl Band<'_> {
    /// Clears to the ground colour and the far depth.
    fn clear(&mut self, ground: u32) {
        self.pixels.fill(ground);
        self.depth.fill(f32::INFINITY);
    }
    /// The row after its last.
    #[inline]
    fn bottom(&self, wide: usize) -> usize {
        self.top + self.pixels.len() / wide
    }
}

/// A mesh point in clip space, before the perspective divide.
#[inline]
fn loose(piece: &Piece, point: u32) -> [f32; 4] {
    let ([x, y, z], [a, b, c, d]) = (piece.points[point as usize], &piece.to_view);
    let through = |row: &[f32; 4]| row[0] * x + row[1] * y + row[2] * z + row[3];
    [through(a), through(b), through(c), through(d)]
}

/// Projects some of a mesh's points.
fn place(piece: &Piece, some: std::ops::Range<usize>, [wide, high]: [usize; 2], shown: &[[AtomicU32; 4]]) {
    let size = [wide as f32, high as f32];
    for (point, shown) in (some.start as u32..).zip(&shown[some]) {
        let at = loose(piece, point);
        let at = if at[2] >= 0.0 && at[3] > 0.0 { on_screen(at, size) } else { [0.0; 4] };
        shown[0].store(at[0].to_bits(), Relaxed);
        shown[1].store(at[1].to_bits(), Relaxed);
        shown[2].store(at[2].to_bits(), Relaxed);
        shown[3].store(at[3].to_bits(), Relaxed);
    }
}

/// A point in front of the eye, projected into an image of that size.
#[inline]
fn on_screen(at: [f32; 4], [wide, high]: [f32; 2]) -> Corner {
    let near = 1.0 / at[3];
    [(at[0] * near * 0.5 + 0.5) * wide, (0.5 - at[1] * near * 0.5) * high, at[2] * near, near]
}

/// A point's computed projection.
#[inline]
fn corner(shown: &[[AtomicU32; 4]], point: u32) -> Corner {
    let [x, y, z, near] = &shown[point as usize];
    [f32::from_bits(x.load(Relaxed)), f32::from_bits(y.load(Relaxed)), f32::from_bits(z.load(Relaxed)), f32::from_bits(near.load(Relaxed))]
}

/// A triangle's three projected corners.
#[inline]
fn corners(shown: &[[AtomicU32; 4]], [a, b, c]: [u32; 3]) -> [Corner; 3] {
    [corner(shown, a), corner(shown, b), corner(shown, c)]
}

/// The bands touched by something between two heights (and, to skip what lies beside the image,
/// two x positions): none if it covers no pixel centre. `more` is how many pixels it extends
/// past its corners.
#[inline]
fn reach([left, top, right, bottom]: [f32; 4], more: i32, [wide, high]: [i32; 2], rows: usize) -> std::ops::Range<usize> {
    let (first, end) = ((first_from(top) - more).max(0), (first_from(bottom) + more).min(high));
    if first >= end || (first_from(left) - more).max(0) >= (first_from(right) + more).min(wide) {
        return 0..0;
    }
    first as usize / rows..(end as usize - 1) / rows + 1
}

/// Bins one share of a mesh's triangles and edges (share `share[0]` of `share[1]`): for each
/// band, those touching it, by mesh index, in order.
fn sort(piece: &Piece, shown: &[[AtomicU32; 4]], share: [usize; 2], [wide, high]: [usize; 2], rows: usize, sorted: &RwLock<Sorted>) {
    let mut sorted = sorted.write().unwrap_or_else(|held| held.into_inner());
    let Sorted { triangles, cut, lines, cut_lines } = &mut *sorted;
    let (size, bands) = ([wide as i32, high as i32], high.div_ceil(rows));
    for lists in [&mut *triangles, &mut *lines] {
        lists.resize_with(bands.max(lists.len()), Vec::new);
        lists.iter_mut().for_each(Vec::clear);
    }
    let far = |at: &Corner| at[0] < -FAR_OUT || at[0] > FAR_OUT || at[1] < -FAR_OUT || at[1] > FAR_OUT;
    let mine = |count: usize| (count.div_ceil(share[1]) * share[0]).min(count)..(count.div_ceil(share[1]) * (share[0] + 1)).min(count);
    cut.clear();
    let filled = piece.ink >> 24 != 0 || !piece.colors.is_empty();
    for number in mine(piece.triangles.len()).filter(|_| filled) {
        let [a, b, c] = corners(shown, piece.triangles[number]);
        let behind = (a[3] == 0.0) as u8 + (b[3] == 0.0) as u8 + (c[3] == 0.0) as u8;
        if behind != 0 || far(&a) || far(&b) || far(&c) {
            cut.extend((behind < 3).then_some(number as u32));
            continue;
        }
        let around = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
        reach(around, 0, size, rows).for_each(|band| triangles[band].push(number as u32));
    }
    cut_lines.clear();
    for number in mine(piece.lines.len()) {
        let (a, b) = (corner(shown, piece.lines[number][0]), corner(shown, piece.lines[number][1]));
        let behind = (a[3] == 0.0) as u8 + (b[3] == 0.0) as u8;
        if behind != 0 || far(&a) || far(&b) {
            cut_lines.extend((behind < 2).then_some(number as u32));
            continue;
        }
        // (An outline is antialiased: it reaches a pixel past its ends.)
        reach([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])], 1, size, rows).for_each(|band| lines[band].push(number as u32));
    }
}

/// Fills one band with everything binned for it, in mesh order: triangle fills first, then
/// outlines over them.
fn fill_band(piece: &Piece, shown: &[[AtomicU32; 4]], sorted: &[RwLock<Sorted>], size: [usize; 2], rows: &mut Band) {
    let found: Vec<_> = sorted.iter().map(|sorted| sorted.read().unwrap_or_else(|held| held.into_inner())).collect();
    for sorted in &found {
        sorted.triangles[rows.number].iter().for_each(|number| whole(piece, *number, shown, size[0], rows));
    }
    for sorted in &found {
        sorted.cut.iter().for_each(|number| cut_to_fit(piece, *number, size, rows));
    }
    for sorted in &found {
        sorted.lines[rows.number].iter().for_each(|number| outline(piece, *number, shown, size[0], rows));
    }
    for sorted in &found {
        sorted.cut_lines.iter().for_each(|number| cut_outline(piece, *number, size, rows));
    }
}

/// Twice a triangle's signed screen area, if it shows: its winding says which side faces the
/// eye, and zero area means seen edge-on.
#[inline]
fn facing(at: &[Corner; 3], one_sided: bool) -> Option<f32> {
    let turn = (at[1][0] - at[0][0]) * (at[2][1] - at[0][1]) - (at[2][0] - at[0][0]) * (at[1][1] - at[0][1]);
    (turn != 0.0 && !(one_sided && turn < 0.0)).then_some(turn)
}

/// Fills one unclipped triangle of the mesh.
fn whole(piece: &Piece, number: u32, shown: &[[AtomicU32; 4]], wide: usize, rows: &mut Band) {
    let [a, b, c] = piece.triangles[number as usize];
    let at = corners(shown, [a, b, c]);
    let Some(turn) = facing(&at, piece.one_sided) else { return };
    let shade = piece.shades.get(number as usize).map_or(256, |shade| *shade as u32 + 1);
    let own = |point: u32| dimmed(piece.colors[point as usize], shade);
    let paint = match piece.colors.is_empty() {
        true => one_colour(dimmed(piece.ink, shade)),
        false => Paint::Blend([own(a), own(b), own(c)]),
    };
    fill(at, turn, paint, wide, rows);
}

/// Draws one unclipped edge of the mesh.
fn outline(piece: &Piece, number: u32, shown: &[[AtomicU32; 4]], wide: usize, rows: &mut Band) {
    let [from, to] = piece.lines[number as usize];
    line(corner(shown, from), corner(shown, to), piece.edges, wide, rows);
}

/// One colour, as a triangle paint.
#[inline]
fn one_colour(ink: u32) -> Paint {
    if ink >> 24 == 255 { Paint::Solid(ink) } else { Paint::Sheer(ink) }
}

/// A colour with red, green and blue scaled by `shade`/256.
#[inline]
fn dimmed(color: u32, shade: u32) -> u32 {
    let (both, green) = (((color & 0x00ff_00ff) * shade) >> 8 & 0x00ff_00ff, ((color & 0xff00) * shade) >> 8 & 0xff00);
    color & 0xff00_0000 | both | green
}

/// Pixel `n`'s centre is at `n + 0.5`: the first whose centre is not before `at`. (Truncation,
/// which is all the processor does natively, plus one where that fell short.)
#[inline]
fn first_from(at: f32) -> i32 {
    let (at, cut) = (at - 0.5, (at - 0.5) as i32);
    cut + (at > cut as f32) as i32
}

/// The integer just below a coordinate, or the coordinate itself.
#[inline]
fn whole_below(at: f32) -> i32 {
    let cut = at as i32;
    cut - (at < cut as f32) as i32
}

/// A value linear across a triangle (depth, a colour channel): its value at the image's origin,
/// and its step per pixel across and down.
#[derive(Clone, Copy)]
struct Even {
    at: f32,
    across: f32,
    down: f32,
}

impl Even {
    /// From its values at the three corners. `per` is one over twice the area.
    #[inline]
    fn of(at: &[Corner; 3], values: [f32; 3], per: f32) -> Even {
        let (ab, ac) = (values[1] - values[0], values[2] - values[0]);
        let across = (ab * (at[2][1] - at[0][1]) - ac * (at[1][1] - at[0][1])) * per;
        let down = (ac * (at[1][0] - at[0][0]) - ab * (at[2][0] - at[0][0])) * per;
        Even { at: values[0] - at[0][0] * across - at[0][1] * down, across, down }
    }
    /// Its value at the centre of the pixel in that column and row.
    #[inline]
    fn in_pixel(&self, column: f32, row: f32) -> f32 {
        self.at + (column + 0.5) * self.across + (row + 0.5) * self.down
    }
}

/// A triangle edge, from its upper end down (of two equally high, from the left): its start,
/// and its x step per pixel down. Both triangles sharing it compute this from the same end to
/// the same numbers: so its row crossings agree.
#[derive(Clone, Copy)]
struct Edge {
    from: [f32; 2],
    lean: f32,
}

impl Edge {
    #[inline]
    fn of(from: &Corner, to: &Corner) -> Edge {
        Edge { from: [from[0], from[1]], lean: (to[0] - from[0]) / (to[1] - from[1]) }
    }
    /// Its x at height `down`.
    #[inline]
    fn crossing(&self, down: f32) -> f32 {
        self.from[0] + (down - self.from[1]) * self.lean
    }
}

/// A triangle's corners top down: of two equally high, the left first.
#[inline]
fn top_down(mut at: [Corner; 3]) -> [Corner; 3] {
    let before = |a: &Corner, b: &Corner| a[1] < b[1] || a[1] == b[1] && a[0] < b[0];
    if before(&at[1], &at[0]) {
        at.swap(0, 1);
    }
    if before(&at[2], &at[1]) {
        at.swap(1, 2);
    }
    if before(&at[1], &at[0]) {
        at.swap(0, 1);
    }
    at
}

/// Fills a triangle: every band pixel whose centre lies inside, where nothing nearer is.
fn fill(at: [Corner; 3], turn: f32, paint: Paint, wide: usize, rows: &mut Band) {
    let [top, middle, bottom] = top_down(at);
    let (first, end) = (first_from(top[1]).max(rows.top as i32), first_from(bottom[1]).min(rows.bottom(wide) as i32));
    if first >= end {
        return;
    }
    let edges = [Edge::of(&top, &bottom), Edge::of(&top, &middle), Edge::of(&middle, &bottom)];
    // (Depth and colours are set up at the first row with a pixel: in a million-triangle mesh,
    // many have none.)
    let mut planes = None;
    for row in first..end {
        let level = row as f32 + 0.5;
        let (long, short) = (edges[0].crossing(level), edges[1 + (level >= middle[1]) as usize].crossing(level));
        let (from, to) = (first_from(long.min(short)).max(0), first_from(long.max(short)).min(wide as i32));
        if from >= to {
            continue;
        }
        let (line, from, to) = ((row as usize - rows.top) * wide, from as usize, to as usize);
        let (pixels, depth) = (&mut rows.pixels[line + from..line + to], &mut rows.depth[line + from..line + to]);
        let (deep, blend) = planes.get_or_insert_with(|| planes_of(&at, turn, paint));
        let start = deep.in_pixel(from as f32, row as f32);
        match (paint, blend.as_ref()) {
            (Paint::Solid(ink), _) => solid(pixels, depth, start, deep.across, ink),
            (Paint::Sheer(ink), _) => sheer(pixels, depth, start, deep.across, ink),
            (Paint::Blend(_), Some(blend)) => blend.row(pixels, depth, [start, deep.across], [from as f32, row as f32]),
            (Paint::Blend(_), None) => {}
        }
    }
}

/// A triangle's depth plane, and for a per-corner coloured one, its colour planes. `turn` is
/// twice its area.
fn planes_of(at: &[Corner; 3], turn: f32, paint: Paint) -> (Even, Option<Blend>) {
    let per = 1.0 / turn;
    let blend = if let Paint::Blend(colors) = paint { Some(Blend::of(at, colors, per)) } else { None };
    (Even::of(at, [at[0][2], at[1][2], at[2][2]], per), blend)
}

/// A row span in one opaque colour: written where nothing nearer is, updating depth.
#[inline]
fn solid(pixels: &mut [u32], depth: &mut [f32], mut deep: f32, step: f32, ink: u32) {
    for (pixel, kept) in pixels.iter_mut().zip(depth) {
        if deep < *kept {
            (*pixel, *kept) = (ink, deep);
        }
        deep += step;
    }
}

/// A row span in a translucent colour: blended over what is there, where nothing nearer is. (It
/// does not write depth.)
#[inline]
fn sheer(pixels: &mut [u32], depth: &mut [f32], mut deep: f32, step: f32, ink: u32) {
    for (pixel, kept) in pixels.iter_mut().zip(depth) {
        if deep < *kept {
            *pixel = Color(ink).over(*pixel, 255);
        }
        deep += step;
    }
}

/// Per-corner colours interpolated over the triangle itself, not its projection: a far corner's
/// colour covers less of the image than a near one's. So each channel is kept divided by w,
/// with 1/w beside it, both of which are linear in screen space.
struct Blend {
    /// Blue, green, red and alpha, each over w; and 1/w.
    parts: [Even; 5],
    /// Every corner is opaque: so is everything between.
    solid: bool,
}

impl Blend {
    fn of(at: &[Corner; 3], colors: [u32; 3], per: f32) -> Blend {
        let part = |shift: u32| Even::of(at, [0, 1, 2].map(|corner| (colors[corner] >> shift & 255) as f32 * at[corner][3]), per);
        let near = Even::of(at, [at[0][3], at[1][3], at[2][3]], per);
        Blend { parts: [part(0), part(8), part(16), part(24), near], solid: colors.iter().all(|color| color >> 24 == 255) }
    }

    /// A row span, from the pixel at column and row `first`.
    fn row(&self, pixels: &mut [u32], depth: &mut [f32], [mut deep, step]: [f32; 2], first: [f32; 2]) {
        let mut now = self.parts.map(|part| part.in_pixel(first[0], first[1]));
        for (pixel, kept) in pixels.iter_mut().zip(depth) {
            if deep < *kept {
                let far = 1.0 / now[4];
                let [blue, green, red, solid] = [now[0], now[1], now[2], now[3]].map(|part| (part * far).clamp(0.0, 255.0) as u32);
                let ink = solid << 24 | red << 16 | green << 8 | blue;
                (*pixel, *kept) = if self.solid { (ink, deep) } else { (Color(ink).over(*pixel, 255), *kept) };
            }
            deep += step;
            let [blue, green, red, solid, near] = &self.parts;
            now = [now[0] + blue.across, now[1] + green.across, now[2] + red.across, now[3] + solid.across, now[4] + near.across];
        }
    }
}

/// An antialiased one-pixel line between two points, a little nearer than its ends: it shows on
/// the triangles it borders. It steps along its major axis a pixel at a time, splitting each
/// step's coverage between the two pixels it passes between.
fn line(from: Corner, to: Corner, ink: u32, wide: usize, rows: &mut Band) {
    let steep = (to[1] - from[1]).abs() > (to[0] - from[0]).abs();
    // (Along and beside: x and y for a shallow line, y and x for a steep one.)
    let (along, beside) = if steep { (1, 0) } else { (0, 1) };
    let (from, to) = if from[along] <= to[along] { (from, to) } else { (to, from) };
    let long = to[along] - from[along];
    if long <= 0.0 {
        return;
    }
    let (top, bottom) = (rows.top as i32, rows.bottom(wide) as i32);
    // The steps in this band (or, for a shallow line, in the image): every one whose centre the
    // line reaches.
    let (least, most) = if steep { (top, bottom) } else { (0, wide as i32) };
    let (mut first, mut end) = (first_from(from[along]).max(least), first_from(to[along]).min(most));
    let (slope, deeper) = ((to[beside] - from[beside]) / long, (to[2] - from[2]) / long);
    // (A rising shallow line lies in this band for only part of its length.)
    if !steep && slope != 0.0 {
        let (enters, leaves) = (from[0] + (top as f32 - 1.0 - from[1]) / slope, from[0] + (bottom as f32 + 1.0 - from[1]) / slope);
        (first, end) = (first.max(enters.min(leaves) as i32 - 1), end.min(enters.max(leaves) as i32 + 2));
    }
    for step in first..end {
        let gone = step as f32 + 0.5 - from[along];
        let (middle, deep) = (from[beside] + gone * slope - 0.5, from[2] + gone * deeper);
        let (low, deep) = (whole_below(middle), deep - (1.0 - deep) * LIFT - 1e-6);
        let cover = ((middle - low as f32) * 255.0) as u32;
        match steep {
            true => (dot(rows, wide, [low, step], deep, ink, 255 - cover), dot(rows, wide, [low + 1, step], deep, ink, cover)),
            false => (dot(rows, wide, [step, low], deep, ink, 255 - cover), dot(rows, wide, [step, low + 1], deep, ink, cover)),
        };
    }
}

/// One line pixel with `cover` coverage (255 full): blended over what is there, if in the band
/// and nothing nearer is.
#[inline]
fn dot(rows: &mut Band, wide: usize, [column, row]: [i32; 2], deep: f32, ink: u32, cover: u32) {
    let down = row.wrapping_sub(rows.top as i32) as usize;
    if column as usize >= wide || down >= rows.pixels.len() / wide {
        return;
    }
    let at = down * wide + column as usize;
    if deep < rows.depth[at] {
        rows.pixels[at] = Color(ink).over(rows.pixels[at], cover);
    }
}

/// The clip planes for an image of that size: the near plane, and four guard-band planes far
/// outside the edges. A point is inside one when its x, y, z and w dotted with the plane are at
/// least zero.
fn sides([wide, high]: [f32; 2]) -> [[f32; 4]; 5] {
    let reach = [2.0 * FAR_OUT / wide, 2.0 * FAR_OUT / high];
    [[0.0, 0.0, 1.0, 0.0], [1.0, 0.0, 0.0, reach[0]], [-1.0, 0.0, 0.0, reach[0]], [0.0, 1.0, 0.0, reach[1]], [0.0, -1.0, 0.0, reach[1]]]
}

/// A point's signed distance inside a plane: negative is outside.
#[inline]
fn inside(side: &[f32; 4], at: &[f32]) -> f32 {
    side[0] * at[0] + side[1] * at[1] + side[2] * at[2] + side[3] * at[3]
}

/// A triangle with a vertex behind the eye or far outside the image: what remains in front of
/// the eye and within the guard band is filled, as the triangles it becomes. (Each plane can
/// add a corner: five planes, so eight at most.)
fn cut_to_fit(piece: &Piece, number: u32, [wide, high]: [usize; 2], rows: &mut Band) {
    let (corners, size) = (piece.triangles[number as usize], [wide as f32, high as f32]);
    let shade = piece.shades.get(number as usize).map_or(256, |shade| *shade as u32 + 1);
    let colour = |point: u32| dimmed(piece.colors.get(point as usize).copied().unwrap_or(piece.ink), shade);
    let with_colour = |point: u32| {
        let (at, colour) = (loose(piece, point), colour(point));
        [at[0], at[1], at[2], at[3], (colour & 255) as f32, (colour >> 8 & 255) as f32, (colour >> 16 & 255) as f32, (colour >> 24) as f32]
    };
    let (mut kept, mut count) = ([[0.0f32; 8]; 8], 3);
    kept[..3].copy_from_slice(&corners.map(with_colour));
    for side in sides(size) {
        (kept, count) = cut_by(&kept[..count], side);
    }
    // (A remaining corner has positive w, except one at the eye itself.)
    if count < 3 || kept[..count].iter().any(|at| at[3] <= 0.0) {
        return;
    }
    let shown = |at: &Loose| on_screen([at[0], at[1], at[2], at[3]], size);
    let coloured = |at: &Loose| (at[7] as u32) << 24 | (at[6] as u32) << 16 | (at[5] as u32) << 8 | at[4] as u32;
    for next in 2..count {
        let three = [&kept[0], &kept[next - 1], &kept[next]];
        let (at, colours) = (three.map(shown), three.map(coloured));
        let Some(turn) = facing(&at, piece.one_sided) else { continue };
        let paint = if piece.colors.is_empty() { one_colour(colours[0]) } else { Paint::Blend(colours) };
        fill(at, turn, paint, wide, rows);
    }
}

/// What remains of a polygon on the inner side of `side`.
fn cut_by(corners: &[Loose], side: [f32; 4]) -> ([Loose; 8], usize) {
    let (mut kept, mut count) = ([[0.0f32; 8]; 8], 0);
    for (number, from) in corners.iter().enumerate() {
        let to = &corners[(number + 1) % corners.len()];
        let (here, there) = (inside(&side, from), inside(&side, to));
        if here >= 0.0 && count < 8 {
            (kept[count], count) = (*from, count + 1);
        }
        // An edge crossing the plane leaves a corner at the crossing.
        if (here >= 0.0) != (there >= 0.0) && count < 8 {
            let gone = here / (here - there);
            (kept[count], count) = (std::array::from_fn(|part| from[part] + (to[part] - from[part]) * gone), count + 1);
        }
    }
    (kept, count)
}

/// An edge with an end behind the eye or far outside the image: what remains in front of the
/// eye and within the guard band is drawn.
fn cut_outline(piece: &Piece, number: u32, [wide, high]: [usize; 2], rows: &mut Band) {
    let ([from, to], size) = (piece.lines[number as usize].map(|point| loose(piece, point)), [wide as f32, high as f32]);
    // The parameters where the remaining part starts and ends along the edge.
    let (mut start, mut end) = (0.0f32, 1.0f32);
    for side in sides(size) {
        let (here, there) = (inside(&side, &from), inside(&side, &to));
        let crossed = here / (here - there);
        (start, end) = (if here < 0.0 { start.max(crossed) } else { start }, if there < 0.0 { end.min(crossed) } else { end });
        // (Both ends outside this plane: nothing remains.)
        end = if here < 0.0 && there < 0.0 { -1.0 } else { end };
    }
    let at = |gone: f32| [0, 1, 2, 3].map(|part| from[part] + (to[part] - from[part]) * gone);
    let ends = [at(start), at(end)];
    if start < end && ends.iter().all(|end| end[3] > 0.0) {
        line(on_screen(ends[0], size), on_screen(ends[1], size), piece.edges, wide, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An orthographic view of an image that size: x and y are pixels, y down, and z (0 to 1)
    /// is depth.
    fn straight(wide: f32, high: f32) -> Matrix {
        [[2.0 / wide, 0.0, 0.0, -1.0], [0.0, -2.0 / high, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
    }

    fn piece<'a>(points: &'a [[f32; 3]], triangles: &'a [[u32; 3]], to_view: Matrix, ink: u32) -> Piece<'a> {
        Piece { points, colors: &[], triangles, shades: &[], lines: &[], to_view, ink, edges: 0, one_sided: false }
    }

    /// The image of some meshes, drawn on so many cores.
    fn drawn(size: [usize; 2], ground: u32, pieces: &[Piece], workers: usize) -> Vec<u32> {
        let mut target = Target::default();
        target.render(size, ground, pieces, workers);
        target.pixels
    }

    /// Deterministic pseudo-random numbers.
    fn chance(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    #[test]
    fn a_triangle_takes_the_pixels_whose_middles_lie_in_it() {
        let mut seed = 7;
        for _ in 0..300 {
            let corners: Vec<[f32; 3]> = (0..3).map(|_| [chance(&mut seed) * 40.0, chance(&mut seed) * 30.0, 0.5]).collect();
            let pixels = drawn([40, 30], 0, &[piece(&corners, &[[0, 1, 2]], straight(40.0, 30.0), 0xffff_ffff)], 1);
            // Each edge's side for a pixel centre, in much higher precision than the corners:
            // on the inner side of all three, it is in.
            let edge = |a: [f32; 3], b: [f32; 3], x: f64, y: f64| (b[0] as f64 - a[0] as f64) * (y - a[1] as f64) - (b[1] as f64 - a[1] as f64) * (x - a[0] as f64);
            let mut wrong = 0;
            for (at, pixel) in pixels.iter().enumerate() {
                let (x, y) = ((at % 40) as f64 + 0.5, (at / 40) as f64 + 0.5);
                let sides = [edge(corners[0], corners[1], x, y), edge(corners[1], corners[2], x, y), edge(corners[2], corners[0], x, y)];
                // (A centre on an edge within that precision may go either way.)
                if sides.iter().any(|side| side.abs() < 1e-3) {
                    continue;
                }
                let inside = sides.iter().all(|side| *side > 0.0) || sides.iter().all(|side| *side < 0.0);
                wrong += (inside != (*pixel != 0)) as usize;
            }
            assert_eq!(wrong, 0, "{corners:?}");
        }
    }

    #[test]
    fn triangles_that_share_edges_leave_no_gap_and_take_no_pixel_twice() {
        // A grid of slightly jittered points, two triangles per square: drawn translucent,
        // every pixel inside the sheet is covered exactly once.
        let (mut seed, across, mut points, mut triangles) = (3, 24u32, Vec::new(), Vec::new());
        for at in 0..across * across {
            let (column, row) = ((at % across) as f32, (at / across) as f32);
            points.push([column * 3.7 + chance(&mut seed) * 2.0 - 4.0, row * 2.9 + chance(&mut seed) * 2.0 - 4.0, 0.5]);
        }
        for at in (0..across * (across - 1)).filter(|at| at % across != across - 1) {
            triangles.extend([[at, at + 1, at + across], [at + 1, at + across + 1, at + across]]);
        }
        let pixels = drawn([64, 48], 0, &[piece(&points, &triangles, straight(64.0, 48.0), 0x4000_00ff)], 1);
        let once = Color(0x4000_00ff).over(0, 255);
        assert!(pixels.iter().all(|pixel| *pixel == once), "{:x?}", pixels.iter().find(|pixel| **pixel != once));
    }

    #[test]
    fn what_is_nearer_hides_what_is_further_whichever_comes_first() {
        let points = [[2.0, 2.0, 0.3], [30.0, 2.0, 0.3], [2.0, 30.0, 0.3], [0.0, 0.0, 0.7], [32.0, 0.0, 0.7], [0.0, 32.0, 0.7]];
        let (near, far) = ([[0, 1, 2]], [[3, 4, 5]]);
        let (red, blue) = (piece(&points, &near, straight(32.0, 32.0), 0xffff_0000), piece(&points, &far, straight(32.0, 32.0), 0xff00_00ff));
        let pictures = [drawn([32, 32], 0, &[red, blue], 1), drawn([32, 32], 0, &[blue, red], 1)];
        assert!(pictures[0] == pictures[1]);
        assert_eq!((pictures[0][5 * 32 + 5], pictures[0][32], pictures[0][31 * 32 + 31]), (0xffff_0000, 0xff00_00ff, 0));
    }

    /// A hilly sheet of so many squares each way, seen from above and in front.
    fn hills(across: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>, Matrix) {
        let (mut points, mut triangles) = (Vec::new(), Vec::new());
        for at in 0..(across + 1) * (across + 1) {
            let (x, z) = ((at % (across + 1)) as f32 / across as f32 - 0.5, (at / (across + 1)) as f32 / across as f32 - 0.5);
            points.push([x * 10.0, ((x * 23.0).sin() + (z * 17.0).cos()) * 0.35, z * 10.0]);
        }
        for at in (0..(across + 1) * across).filter(|at| at % (across + 1) != across) {
            triangles.extend([[at, at + 1, at + across + 1], [at + 1, at + across + 2, at + across + 1]]);
        }
        // The eye at (0, 4, -9) looks at the centre; depth from half a unit to sixty.
        let (far, near) = (60.0f32, 0.5f32);
        let (down, level) = (4.0 / 97.0f32.sqrt(), 9.0 / 97.0f32.sqrt());
        let eye = [[1.0, 0.0, 0.0, 0.0], [0.0, level, down, -4.0 * level + 9.0 * down], [0.0, -down, level, 4.0 * down + 9.0 * level]];
        let deep = eye[2].map(|part| part * far / (far - near));
        let to_view = [eye[0].map(|part| part * 1.2), eye[1].map(|part| part * 1.6), [deep[0], deep[1], deep[2], deep[3] - near * far / (far - near)], eye[2]];
        (points, triangles, to_view)
    }

    #[test]
    fn many_processors_draw_what_one_draws() {
        let (points, triangles, to_view) = hills(90);
        let colors: Vec<u32> = (0..points.len() as u32).map(|at| 0xff00_0000 | at.wrapping_mul(2654435761) >> 8).collect();
        let (lines, shades) = (edges_of(&triangles), shades_of(&points, &triangles, [0.0, 1.0, 0.0], 3));
        assert_eq!((lines.len(), shades.len()), (90 * 91 * 2 + 90 * 90, triangles.len()));
        assert!(shades == shades_of(&points, &triangles, [0.0, 1.0, 0.0], 1) && shades.iter().all(|shade| *shade >= 86));
        let looks = [(0xff40_80c0, 0, false), (0x8040_80c0, 0xffff_ffff, false), (0, 0xff20_2020, false), (0xff40_80c0, 0, true)];
        for (ink, edges, own) in looks {
            let body = Piece { colors: if own { &colors } else { &[] }, edges, shades: &shades, lines: if edges != 0 { &lines } else { &[] }, ..piece(&points, &triangles, to_view, ink) };
            // (With a small mesh beside the large one, which one core draws alone.)
            let small = piece(&points[..3], &[[0, 1, 2]], to_view, 0xffff_ff00);
            let (one, seven) = (drawn([320, 200], 0xff10_1010, &[small, body, small], 1), drawn([320, 200], 0xff10_1010, &[small, body, small], 7));
            assert!(one == seven, "with {ink:x} and {edges:x}");
            assert!(one.iter().filter(|pixel| **pixel != 0xff10_1010).count() > 20_000);
        }
    }

    #[test]
    fn what_is_behind_the_eye_is_cut_off_where_the_eye_is() {
        // A floor from far in front of the eye to far behind it shows from the image's bottom
        // to the horizon, with nothing missing; and the same floor of many small squares gives
        // the same image below the horizon.
        let floor = |across: u32| {
            let (mut points, mut triangles) = (Vec::new(), Vec::new());
            for at in 0..(across + 1) * (across + 1) {
                let (x, z) = ((at % (across + 1)) as f32 / across as f32 - 0.5, (at / (across + 1)) as f32 / across as f32 - 0.5);
                points.push([x * 400.0, -1.0, z * 400.0]);
            }
            for at in (0..(across + 1) * across).filter(|at| at % (across + 1) != across) {
                triangles.extend([[at, at + 1, at + across + 1], [at + 1, at + across + 2, at + across + 1]]);
            }
            (points, triangles)
        };
        // The eye at the origin, looking along z: x and y as they are, depth from 0.1 to 1000.
        let (near, far) = (0.1f32, 1000.0f32);
        let to_view = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, far / (far - near), -near * far / (far - near)], [0.0, 0.0, 1.0, 0.0]];
        let draw = |across: u32, edges: u32| {
            let (points, triangles) = floor(across);
            let lines = if edges != 0 { edges_of(&triangles) } else { Vec::new() };
            drawn([80, 60], 0, &[Piece { edges, lines: &lines, ..piece(&points, &triangles, to_view, 0xff00_ff00) }], 1)
        };
        let (two, many) = (draw(1, 0), draw(64, 0));
        // Below the horizon (the middle row) all is floor; above, nothing.
        assert!(two[..29 * 80].iter().all(|pixel| *pixel == 0) && two[31 * 80..].iter().all(|pixel| *pixel == 0xff00_ff00));
        assert!(two[31 * 80..] == many[31 * 80..]);
        // Its outline is clipped there too: the edge from behind the eye to far in front shows
        // as a line from the image's bottom to the horizon. (It runs down the left edge, half
        // outside: so half white, on green.)
        let outlined = draw(1, 0xffff_ffff);
        let white = |row: usize| outlined[row * 80..(row + 1) * 80].iter().filter(|pixel| **pixel & 0xff > 60).count();
        assert!((34..60).all(|row| white(row) >= 1) && (0..28).all(|row| white(row) == 0));
    }

    #[test]
    fn colours_at_the_corners_go_evenly_from_one_to_the_other() {
        let points = [[0.0, 0.0, 0.5], [64.0, 0.0, 0.5], [0.0, 64.0, 0.5]];
        let colors = [0xffff_0000, 0xff00_ff00, 0xff00_00ff];
        let pixels = drawn([64, 64], 0, &[Piece { colors: &colors, ..piece(&points, &[[0, 1, 2]], straight(64.0, 64.0), 0) }], 1);
        let part = |pixel: u32, shift: u32| (pixel >> shift & 255) as i32;
        // Near each corner its colour; halfway along an edge, half of each end's.
        assert!(part(pixels[0], 16) > 245 && part(pixels[62], 8) > 240 && part(pixels[62 * 64], 0) > 240);
        let halfway = pixels[31];
        assert!((part(halfway, 16) - 127).abs() < 6 && (part(halfway, 8) - 127).abs() < 6 && part(halfway, 0) < 4, "{halfway:x}");
    }

    #[test]
    fn an_outline_runs_round_a_triangle_and_shows_on_it() {
        let points = [[4.0, 4.0, 0.5], [28.0, 6.0, 0.5], [10.0, 26.0, 0.5], [30.0, 28.0, 0.5]];
        let one = [[0, 1, 2]];
        let lines = edges_of(&one);
        let pixels = drawn([32, 32], 0, &[Piece { edges: 0xffff_ffff, lines: &lines, ..piece(&points, &one, straight(32.0, 32.0), 0xff00_0080) }], 1);
        // Along its top edge the outline's white is over the fill; well inside, the fill alone.
        let white = |pixel: u32| pixel >> 8 & 255;
        assert!((5..27).all(|x| (3..8).any(|y| white(pixels[y * 32 + x]) > 120)));
        assert_eq!(pixels[12 * 32 + 14], 0xff00_0080);
        // An outline alone: nothing inside.
        let alone = drawn([32, 32], 0, &[Piece { edges: 0xffff_ffff, lines: &lines, ..piece(&points, &one, straight(32.0, 32.0), 0) }], 1);
        assert_eq!(alone[12 * 32 + 14], 0);
        assert!(alone.iter().filter(|pixel| **pixel != 0).count() > 60);
        // A shared edge is one edge: five edges for two triangles, and the shared one looks the
        // same with or without the second triangle.
        let two = [[0, 1, 2], [1, 3, 2]];
        let lines = edges_of(&two);
        assert_eq!(lines, [[0, 1], [0, 2], [1, 2], [1, 3], [2, 3]]);
        let both = drawn([32, 32], 0, &[Piece { edges: 0xffff_ffff, lines: &lines, ..piece(&points, &two, straight(32.0, 32.0), 0) }], 1);
        assert!((10..20).all(|at| both[at * 32 + 21 - at / 2] == alone[at * 32 + 21 - at / 2]));
    }

    /// How long a million triangles take:
    /// `cargo test -p taut --release --lib raster -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement: run it by itself, built for release"]
    fn how_long_a_million_triangles_take() {
        let (points, triangles, to_view) = hills(708);
        let all = std::thread::available_parallelism().map_or(1, |count| count.get());
        let start = std::time::Instant::now();
        let lines = edges_of(&triangles);
        let found = start.elapsed().as_secs_f64() * 1000.0;
        let shades = shades_of(&points, &triangles, [0.0, 1.0, 0.0], all);
        let shaded = start.elapsed().as_secs_f64() * 1000.0 - found;
        println!("{} triangles, {} points, {} edges (found in {found:.0} ms; shaded in {shaded:.1} ms), {all} processors", triangles.len(), points.len(), lines.len());
        let colors: Vec<u32> = (0..points.len() as u32).map(|at| 0xff00_0000 | at.wrapping_mul(2654435761) >> 8).collect();
        let looks = [
            ("one colour, lit", 0xff40_80c0, 0, false),
            ("lit, with outlines", 0xff40_80c0, 0xff10_2030, false),
            ("outlines alone", 0, 0xffe0_e0e0, false),
            ("a colour at each point", 0xff40_80c0, 0, true),
        ];
        for (name, ink, edges, own) in looks {
            for workers in [1, all] {
                let mut target = Target::default();
                let body = Piece { colors: if own { &colors } else { &[] }, edges, shades: &shades, lines: if edges != 0 { &lines } else { &[] }, ..piece(&points, &triangles, to_view, ink) };
                let mut best = f64::MAX;
                for _ in 0..12 {
                    let start = std::time::Instant::now();
                    target.render([1920, 1080], 0xff10_1010, &[body], workers);
                    best = best.min(start.elapsed().as_secs_f64() * 1000.0);
                }
                println!("{name:>24}, {workers:>2} at work: {best:>6.2} ms");
            }
        }
    }
}
