//! Scenes: triangle meshes seen from a viewpoint, in a box of the window.
//!
//! ```ignore
//! // once: the body is kept, as a picture is
//! let hill = ui.add_mesh(points, triangles);
//! // each time the window is described: where it is looked at from, and what is in view
//! let view = Camera::look_at([0.0, 40.0, -90.0], [0.0, 0.0, 0.0]);
//! ui.el().grow().scene(&view, &[hill.filled(t.accent).outlined(t.line)]);
//! ```
//!
//! A scene is redrawn only when what it shows or its viewpoint changed, on every processor
//! core: a million triangles take a few ms.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::{Point, Rect, pt};
use crate::id::{fold, hash};
use crate::scene_raster::{Matrix, Piece, Target, edges_of, shades_of};
use crate::slots::Slots;

/// A mesh the interface keeps for scenes: points, and triangles between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mesh(pub(crate) u32);

/// A mesh as it is kept.
#[derive(Default)]
pub(crate) struct Body {
    points: Vec<[f32; 3]>,
    /// A colour per point, or none.
    colors: Vec<u32>,
    triangles: Vec<[u32; 3]>,
    /// How often it was replaced or recoloured.
    version: u32,
    /// Its edges, each once: worked out when first outlined.
    edges: Option<Vec<[u32; 2]>>,
    /// Each triangle's brightness in the light that last fell on it, and that light's direction
    /// (in the mesh's own axes).
    shades: Option<([u32; 3], Vec<u8>)>,
}

/// How the view is projected onto the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lens {
    /// Perspective: what is further away is smaller. The number is how far up from the middle
    /// the view reaches at distance one.
    Angle(f32),
    /// Orthographic, as a plan is drawn: the view is this many units high.
    Flat(f32),
    /// A matrix the app has from elsewhere.
    Whole(Matrix),
}

/// Where a scene is seen from, and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    from: [f32; 3],
    at: [f32; 3],
    up: [f32; 3],
    lens: Lens,
    reach: [f32; 2],
    light: [f32; 3],
}

impl Camera {
    /// Seen from `from`, looking at `at`, y up: a 50° vertical field of view, showing
    /// everything between 0.1 and 10,000 units away. Light from above, slightly front and
    /// right.
    pub fn look_at(from: [f32; 3], at: [f32; 3]) -> Camera {
        Camera { from, at, up: [0.0, 1.0, 0.0], lens: Lens::Angle((25.0f32).to_radians().tan()), reach: [0.1, 10_000.0], light: [0.35, 0.85, -0.4] }
    }
    /// With another up direction: `[0.0, 0.0, 1.0]` for a z-up world.
    pub fn up(self, up: [f32; 3]) -> Camera {
        Camera { up, ..self }
    }
    /// With this vertical field of view, in degrees.
    pub fn fov(self, degrees: f32) -> Camera {
        Camera { lens: Lens::Angle((degrees.clamp(1.0, 170.0) / 2.0).to_radians().tan()), ..self }
    }
    /// Orthographic: nothing shrinks with distance, and the view is `high` units tall.
    pub fn orthographic(self, high: f32) -> Camera {
        Camera { lens: Lens::Flat(high.max(f32::MIN_POSITIVE)), ..self }
    }
    /// Showing everything between `near` and `far`. (Nearer than `near` is clipped; depth
    /// precision far away depends on `near` being no smaller than needed.)
    pub fn depth_range(self, near: f32, far: f32) -> Camera {
        Camera { reach: [near.max(f32::MIN_POSITIVE), far.max(near.max(f32::MIN_POSITIVE) * 1.001)], ..self }
    }
    /// With light from that direction: triangles facing it are brightest.
    pub fn light_from(self, direction: [f32; 3]) -> Camera {
        Camera { light: direction, ..self }
    }
    /// A matrix the app has from elsewhere (a game's camera): four rows of four, applied to a
    /// point with a 1 after it; the result is x from -w (left) to w (right), y from -w (bottom)
    /// to w (top), depth from 0 (nearest shown) to w, and w. (Direct3D's convention. Used as
    /// is, whatever the box's aspect.)
    pub fn from_matrix(rows: [[f32; 4]; 4]) -> Camera {
        Camera { lens: Lens::Whole(rows), ..Camera::look_at([0.0; 3], [0.0, 0.0, 1.0]) }
    }

    /// Where a scene point shows in a box at `within`, to put a label or mark there. Nothing
    /// for a point behind the eye.
    pub fn project(&self, point: [f32; 3], within: Rect) -> Option<Point> {
        let through = |row: [f32; 4]| row[0] * point[0] + row[1] * point[1] + row[2] * point[2] + row[3];
        let [x, y, z, w] = self.table(within.w / within.h.max(f32::MIN_POSITIVE)).map(through);
        (z >= 0.0 && w > 0.0).then(|| pt(within.x + (x / w * 0.5 + 0.5) * within.w, within.y + (0.5 - y / w * 0.5) * within.h))
    }

    /// The matrix a point goes through to where it shows, in a box `shape` times as wide as
    /// high.
    fn table(&self, shape: f32) -> Matrix {
        let ahead = united(minus(self.at, self.from));
        let right = united(cross(ahead, self.up));
        let above = cross(right, ahead);
        // Each row is one of the eye's axes, and the eye's position along it.
        let row = |along: [f32; 3], times: f32| [along[0] * times, along[1] * times, along[2] * times, -dot(along, self.from) * times];
        let ([near, far], deep) = (self.reach, row(ahead, 1.0));
        match self.lens {
            Lens::Whole(rows) => rows,
            Lens::Angle(up) => {
                let depth = deep.map(|part| part * far / (far - near));
                [row(right, 1.0 / (up * shape)), row(above, 1.0 / up), [depth[0], depth[1], depth[2], depth[3] - near * far / (far - near)], deep]
            }
            Lens::Flat(high) => {
                let depth = deep.map(|part| part / (far - near));
                [row(right, 2.0 / (high * shape)), row(above, 2.0 / high), [depth[0], depth[1], depth[2], depth[3] - near / (far - near)], [0.0, 0.0, 0.0, 1.0]]
            }
        }
    }

    /// A number that differs for another view.
    fn key(&self) -> u64 {
        let bits = |three: [f32; 3]| hash(three.map(f32::to_bits));
        let lens = match self.lens {
            Lens::Angle(up) => hash((1, up.to_bits())),
            Lens::Flat(high) => hash((2, high.to_bits())),
            Lens::Whole(rows) => hash(rows.map(|row| row.map(f32::to_bits))),
        };
        [bits(self.at), bits(self.up), lens, hash(self.reach.map(f32::to_bits)), bits(self.light)].into_iter().fold(bits(self.from), fold)
    }
}

fn minus(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
/// The same direction at length one. (A zero vector stays zero.)
fn united(a: [f32; 3]) -> [f32; 3] {
    let long = dot(a, a).sqrt();
    if long > 0.0 { a.map(|part| part / long) } else { a }
}

/// A mesh as a scene shows it: where, and in which colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshInstance {
    mesh: Mesh,
    /// Its placement: three rows of four, applied to each of its points.
    place: [[f32; 4]; 3],
    fill: Color,
    edges: Color,
    lit: bool,
    one_sided: bool,
}

impl Mesh {
    /// The mesh with its triangles filled in `color`. (A mesh given a colour per point with
    /// [`Ui::color_mesh`](crate::Ui::color_mesh) is filled in those instead, unless `color` is
    /// fully clear.)
    pub fn filled(self, color: Color) -> MeshInstance {
        MeshInstance { mesh: self, place: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], fill: color, edges: Color::NONE, lit: true, one_sided: false }
    }
    /// The mesh as its triangles' outlines only, in `color`.
    pub fn outlined(self, color: Color) -> MeshInstance {
        self.filled(Color::NONE).outlined(color)
    }
}

impl MeshInstance {
    /// With its triangles' outlines drawn over it in `color`, one pixel thick.
    pub fn outlined(self, color: Color) -> MeshInstance {
        MeshInstance { edges: color, ..self }
    }
    /// Moved: the centre of the mesh's points goes to `to`.
    pub fn centered_at(self, to: [f32; 3]) -> MeshInstance {
        let [x, y, z] = self.place;
        MeshInstance { place: [[x[0], x[1], x[2], to[0]], [y[0], y[1], y[2], to[1]], [z[0], z[1], z[2], to[2]]], ..self }
    }
    /// Placed by the app's own matrix: three rows of four, applied to each point (with a 1
    /// after it). Rotate, scale, move: all of it.
    pub fn transform(self, rows: [[f32; 4]; 3]) -> MeshInstance {
        MeshInstance { place: rows, ..self }
    }
    /// In its colours as they are, whichever way a triangle faces: no shading.
    pub fn unlit(self) -> MeshInstance {
        MeshInstance { lit: false, ..self }
    }
    /// One-sided: seen only from the side where a triangle's corners go clockwise. (A closed
    /// mesh's far side is then not drawn: half the work.)
    pub fn one_sided(self) -> MeshInstance {
        MeshInstance { one_sided: true, ..self }
    }
}

/// A scene of this run: its view, which of the run's meshes it shows, and a number that changes
/// when any of that does.
struct Scene {
    view: Camera,
    shown: [usize; 2],
    key: u64,
}

/// A scene's picture as last drawn: for which scene, on what background.
#[derive(Default)]
struct Drawn {
    target: Target,
    key: u64,
    ground: u32,
}

/// What redraws a scene's picture: which scene of the run, its size, its background.
type Afresh = fn(&mut Scenes, usize, [usize; 2], u32);

/// Everything the interface keeps for scenes.
#[derive(Default)]
pub(crate) struct Scenes {
    bodies: Slots<Body>,
    scenes: Vec<Scene>,
    shown: Vec<MeshInstance>,
    drawn: Vec<Drawn>,
    /// What redraws a scene's picture. Set by the first mesh taken in and reached no other way,
    /// so a program that never takes one in does not carry the triangle rasterizer (some 40
    /// KB).
    afresh: Option<Afresh>,
}

impl Scenes {
    /// Takes a mesh in. (A triangle with a corner that is none of the points is dropped.)
    pub fn add(&mut self, points: Vec<[f32; 3]>, mut triangles: Vec<[u32; 3]>) -> Mesh {
        triangles.retain(|corners| corners.iter().all(|corner| (*corner as usize) < points.len()));
        self.afresh = Some(Scenes::draw);
        Mesh(self.bodies.put(Body { points, triangles, ..Default::default() }))
    }
    /// Replaces a mesh with another.
    pub fn set(&mut self, mesh: Mesh, points: Vec<[f32; 3]>, mut triangles: Vec<[u32; 3]>) {
        triangles.retain(|corners| corners.iter().all(|corner| (*corner as usize) < points.len()));
        let Some(body) = self.bodies.get_mut(mesh.0) else { return };
        *body = Body { points, triangles, version: body.version + 1, ..Default::default() };
    }
    /// Lets a mesh go. Says whether it was there.
    pub fn let_go(&mut self, mesh: Mesh) -> bool {
        self.bodies.take(mesh.0).is_some()
    }
    /// Gives a mesh a colour per point; with another number of colours than points, it has none
    /// of its own again.
    pub fn color(&mut self, mesh: Mesh, colors: Vec<Color>) {
        let Some(body) = self.bodies.get_mut(mesh.0) else { return };
        let fits = colors.len() == body.points.len();
        (body.colors, body.version) = (colors.into_iter().filter(|_| fits).map(|color| color.0).collect(), body.version + 1);
    }

    /// Swaps the meshes taken in (and what draws them) with another window's scenes: called
    /// twice, all is back. (Meshes belong to every window; a run's scenes and pictures to each
    /// window.)
    pub fn share(&mut self, with: &mut Scenes) {
        std::mem::swap(&mut self.bodies, &mut with.bodies);
        std::mem::swap(&mut self.afresh, &mut with.afresh);
    }

    /// Before the app describes the window: no scene yet.
    pub fn begin(&mut self) {
        self.scenes.clear();
        self.shown.clear();
    }
    /// A scene the app describes: its index in this run.
    pub fn note(&mut self, view: &Camera, shown: &[MeshInstance]) -> u32 {
        let Scenes { bodies, scenes, shown: all, .. } = self;
        let first = all.len();
        // A mesh the app let go is in no scene.
        all.extend(shown.iter().filter(|shown| bodies.get(shown.mesh.0).is_some()));
        let version = |shown: &MeshInstance| bodies.get(shown.mesh.0).map_or(0, |body| body.version);
        let part = |shown: &MeshInstance| hash((shown.mesh.0, version(shown), shown.place.map(|row| row.map(f32::to_bits)), shown.fill.0, shown.edges.0, shown.lit, shown.one_sided));
        let key = all[first..].iter().map(part).fold(view.key(), fold);
        scenes.push(Scene { view: *view, shown: [first, all.len()], key });
        scenes.len() as u32 - 1
    }
    /// The number that changes when scene `scene` of this run looks different.
    pub fn key(&self, scene: u32) -> u64 {
        self.scenes[scene as usize].key
    }

    /// Draws scene `scene` of this run where its box lies: the kept picture, redrawn first if
    /// the scene, its size or the background differs from what it was drawn for. `ground` is
    /// the box's colour if opaque: the picture then has it wherever no mesh is, and is copied
    /// as is; over any other background it is blended pixel by pixel.
    pub fn show(&mut self, scene: u32, within: Rect, ground: Option<Color>, canvas: &mut Canvas) {
        let scale = canvas.scale();
        let corner = |x: f32, y: f32| ((x * scale).round() as i32, (y * scale).round() as i32);
        let ((left, top), (right, bottom)) = (corner(within.x, within.y), corner(within.right(), within.bottom()));
        let size = [(right - left).max(0) as usize, (bottom - top).max(0) as usize];
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        self.drawn.resize_with(self.scenes.len().max(self.drawn.len()), Default::default);
        let (key, ground_pixel) = (self.scenes[scene as usize].key, ground.map_or(0, |ground| ground.0));
        let kept = &self.drawn[scene as usize];
        let stale = (kept.key, kept.ground, [kept.target.width, kept.target.height]) != (key, ground_pixel, size);
        if let Some(afresh) = self.afresh.filter(|_| stale) {
            afresh(self, scene as usize, size, ground_pixel);
        }
        canvas.pixels(&self.drawn[scene as usize].target.pixels, size[0], [left, top], ground.is_some());
    }

    /// Redraws the picture of scene `scene`.
    fn draw(&mut self, scene: usize, size: [usize; 2], ground: u32) {
        let workers = std::thread::available_parallelism().map_or(1, |count| count.get());
        let Scene { view, shown: [first, end], key } = self.scenes[scene];
        let table = view.table(size[0] as f32 / size[1] as f32);
        // What each mesh needs worked out for this drawing comes first. (One let go since the
        // scene was described is skipped.)
        for shown in &self.shown[first..end] {
            let body = self.bodies.get_mut(shown.mesh.0);
            body.into_iter().for_each(|body| body.ready(shown, united(view.light), workers));
        }
        let piece = |shown: &MeshInstance| self.bodies.get(shown.mesh.0).map(|body| body.piece(shown, &table));
        let pieces: Vec<Piece> = self.shown[first..end].iter().filter_map(piece).collect();
        let drawn = &mut self.drawn[scene];
        drawn.target.render(size, ground, &pieces, workers);
        (drawn.key, drawn.ground) = (key, ground);
    }

    /// After a run: pictures of scenes no longer there are let go.
    pub fn settle(&mut self) {
        self.drawn.truncate(self.scenes.len());
    }
}

impl Body {
    /// The light's direction for a mesh placed so, in the mesh's own axes, as the bits of three
    /// numbers that tell one light from another.
    fn light(shown: &MeshInstance, light: [f32; 3]) -> [u32; 3] {
        let [x, y, z] = shown.place;
        united([x[0] * light[0] + y[0] * light[1] + z[0] * light[2], x[1] * light[0] + y[1] * light[1] + z[1] * light[2], x[2] * light[0] + y[2] * light[1] + z[2] * light[2]]).map(f32::to_bits)
    }

    /// Works out what showing it so needs and is missing: its edges for outlines, each
    /// triangle's brightness in that light.
    fn ready(&mut self, shown: &MeshInstance, light: [f32; 3], workers: usize) {
        if !shown.edges.is_none() && self.edges.is_none() {
            self.edges = Some(edges_of(&self.triangles));
        }
        let from = Body::light(shown, light);
        if shown.lit && self.shades.as_ref().is_none_or(|(lit_from, _)| *lit_from != from) {
            self.shades = Some((from, shades_of(&self.points, &self.triangles, from.map(f32::from_bits), workers)));
        }
    }

    /// The mesh as drawn for `shown`, through the scene's matrix.
    fn piece(&self, shown: &MeshInstance, table: &Matrix) -> Piece<'_> {
        // The mesh's points go through its placement, then the scene's matrix: both at once, as
        // one matrix.
        let placed = [shown.place[0], shown.place[1], shown.place[2], [0.0, 0.0, 0.0, 1.0]];
        let through = |row: &[f32; 4], part: usize| row[0] * placed[0][part] + row[1] * placed[1][part] + row[2] * placed[2][part] + row[3] * placed[3][part];
        let to_view = table.map(|row| [through(&row, 0), through(&row, 1), through(&row, 2), through(&row, 3)]);
        let shades = self.shades.as_ref().filter(|_| shown.lit).map_or(&[][..], |(_, shades)| shades);
        let lines = self.edges.as_ref().filter(|_| !shown.edges.is_none()).map_or(&[][..], |edges| edges);
        // A mesh with its own colours is hidden by a fully clear fill, as any is.
        let colors = if shown.fill.is_none() { &[][..] } else { &self.colors[..] };
        Piece { points: &self.points, colors, triangles: &self.triangles, shades, lines, to_view, ink: shown.fill.0, edges: shown.edges.0, one_sided: shown.one_sided }
    }
}
