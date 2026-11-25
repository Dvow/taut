//! Drawing on the GPU (the feature `gpu-draw`): a paint recorded as quads, which a GPU path
//! draws into a window's texture instead of the CPU drawing it into the strip.
//!
//! | Drawn by the GPU | How |
//! | --- | --- |
//! | rectangles, rounded or not, borders | the CPU's coverage formula, per pixel |
//! | shadows | the same |
//! | glyphs and icons | their coverage, kept in the cache texture by content |
//! | GDI's text (Windows) | each run drawn once by GDI, its three coverages kept |
//! | pictures | their pixels uploaded; enlarged bilinear, reduced by the mean, as the CPU |
//! | gradients | the CPU's formula per pixel; the stops uploaded |
//! | filled shapes (one colour) | each pixel's exact area inside, summed over the edges near it |
//! | strokes (round, one colour) | four subcells a pixel, each as covered as by the nearest segment |
//!
//! Shapes go up in bands of 16 rows, each band only its edges; one with more than
//! `MOST_EDGES` in a band is drawn by the CPU. Everything else (scenes, SVG drawings, colour
//! emoji, fills of other inks) is drawn by the CPU into a patch as small as it is, laid in by
//! the GPU. What can't be done that way (see-through
//! boxes, a clear background, an atlas full) makes the CPU draw that part of the window, as
//! without the feature: never anything wrong.
//!
//! Colours blend as the CPU's do, but for rounding: a pixel differs by at most a few levels of
//! 255 where something is part covered (edges, text). GDI's text is blended per channel in
//! the colour space, where GDI applies its own contrast: up to a tenth near its edges.
use crate::canvas::{Canvas, Edges, FINE, SOFT};
use crate::color::Color;
use crate::geom::{Point, Rect, pt};
use crate::path::Path;
use std::collections::HashMap;

/// The shaders' kinds of quad.
pub(crate) const SOLID: f32 = 0.0;
pub(crate) const RING: f32 = 1.0;
pub(crate) const SHADOW: f32 = 2.0;
pub(crate) const MASK: f32 = 3.0;
#[cfg(windows)]
pub(crate) const RUN: f32 = 4.0;
pub(crate) const PICTURE: f32 = 5.0;
pub(crate) const PATCH: f32 = 6.0;
#[cfg(windows)]
pub(crate) const KEPT: f32 = 7.0;
pub(crate) const GRADIENT: f32 = 8.0;
pub(crate) const PATH: f32 = 9.0;
pub(crate) const STROKE: f32 = 10.0;

/// Rows of pixels a path's quads cover each, each quad reading only the edges that reach them.
const BAND: i32 = 16;
/// The most edges a band of a path may have for the GPU to draw it (each pixel visits each);
/// more, and the CPU draws it into a patch.
pub(crate) const MOST_EDGES: usize = 256;

/// How many quads back a run of text looks for the solid colour it lies on.
#[cfg(windows)]
const LOOKED_BACK: usize = 512;

/// The side of the two textures quads read: kept glyphs and runs; this part's patches and
/// pictures.
pub(crate) const SIDE: i32 = 2048;

/// The most a reduced picture's pixel averages along each axis on the GPU; more is drawn by the
/// CPU.
const MOST_REDUCED: f32 = 8.0;

/// What a part of a window this many pixels or larger is drawn by on `Renderer::Auto`: the GPU
/// (`Drawing::Large`). Smaller ones cost about the same either way, so the CPU draws them (see
/// docs/costs-and-speed.md).
pub(crate) const LARGE: i64 = 65_536;

/// One quad, as the shaders read it: six rows of four numbers.
///
/// | Row | Holds |
/// | --- | --- |
/// | 0 | where it is drawn: left, top, right, bottom in window pixels (inside the clip) |
/// | 1 | the shape: its rectangle, or for a mask, patch or run its top left |
/// | 2 | corner radius, then by kind: a ring's thickness (-1 filled); a shadow's reach and drop |
/// | 3 | where its texels are: left, top, width, height |
/// | 4 | its colour: red, green, blue, alpha, 0 to 255, not premultiplied |
/// | 5 | its kind, which texture (0 cache, 1 patches), and the window's width and height |
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Quad {
    pub(crate) rows: [[f32; 4]; 6],
}

/// One rectangle of texels to put into a texture before drawing: `at` (left, top, width,
/// height), read from `staging` at `from`, `at[2]` a row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Upload {
    pub(crate) at: [i32; 4],
    pub(crate) from: usize,
    /// 0: the cache; 1: the patches.
    pub(crate) texture: u8,
}

/// Room in a texture, shelf by shelf: each taken from the left of the lowest shelf, a new shelf
/// started where it doesn't fit.
#[derive(Clone, Copy, Debug, Default)]
struct Shelves {
    x: i32,
    y: i32,
    tall: i32,
}

impl Shelves {
    /// The top left of room `w` by `h`; nothing where the texture is full.
    #[inline]
    fn place(&mut self, w: i32, h: i32) -> Option<[i32; 2]> {
        if w > SIDE || h > SIDE {
            return None;
        }
        if self.x + w > SIDE {
            (self.x, self.y, self.tall) = (0, self.y + self.tall, 0);
        }
        if self.y + h > SIDE {
            return None;
        }
        let at = [self.x, self.y];
        (self.x, self.tall) = (self.x + w, self.tall.max(h));
        Some(at)
    }
}

/// A paint recorded for the GPU: its quads, and the texels they need put in first. Kept by a
/// GPU device and reused, so a steady paint allocates nothing.
#[derive(Default)]
pub(crate) struct DrawList {
    pub(crate) quads: Vec<Quad>,
    pub(crate) uploads: Vec<Upload>,
    pub(crate) staging: Vec<u32>,
    /// What was put in the cache texture, by key: left, top, width, height.
    cache: HashMap<u64, [i32; 4]>,
    cache_room: Shelves,
    /// Where kept runs drawn on a colour change it: left, top, right, bottom in the run.
    extents: HashMap<u64, [i32; 4]>,
    patch_room: Shelves,
    /// The window's size in pixels.
    size: [f32; 2],
    /// Something can't be drawn this way: the CPU draws the part.
    pub(crate) failed: bool,
    /// The device can't blend per channel: a part with a run of GDI's not on a solid colour
    /// (`run`) fails.
    #[cfg(windows)]
    pub(crate) no_runs: bool,
    /// What a patch is drawn into first.
    scratch: Vec<u32>,
    /// A path's edges (left, top, right, bottom ends), and its bands: first row, end row, first
    /// edge, count. Kept for their room.
    edges: Vec<[f32; 4]>,
    bands: Vec<[i32; 4]>,
}

impl std::fmt::Debug for DrawList {
    /// How much it holds.
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrawList").field("quads", &self.quads.len()).field("uploads", &self.uploads.len()).field("kept", &self.cache.len()).field("failed", &self.failed).finish()
    }
}

/// A hash of words, multiplied and rotated in (as rustc's own): for telling glyphs and runs
/// apart by what they are.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Fast(pub(crate) u64);

impl std::hash::Hasher for Fast {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        words.iter().for_each(|word| self.write_u64(u64::from_le_bytes(*word)));
        let rest = rest.iter().fold(bytes.len() as u64, |sum, byte| sum << 8 | *byte as u64);
        self.write_u64(rest);
    }
    #[inline]
    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// `inner` cut to `outer`, both left, top, right, bottom.
#[inline]
fn cut(inner: [i32; 4], outer: [i32; 4]) -> [i32; 4] {
    [inner[0].max(outer[0]), inner[1].max(outer[1]), inner[2].min(outer[2]), inner[3].min(outer[3])]
}

/// A pixel position, rounded and kept within ±16 million pixels, as the CPU's.
#[inline]
fn whole(at: f32) -> f32 {
    at.round().clamp(-16_777_216.0, 16_777_216.0)
}

/// A colour as the shaders take it.
#[inline]
fn channels(color: Color) -> [f32; 4] {
    [color.0 >> 16 & 255, color.0 >> 8 & 255, color.0 & 255, color.0 >> 24].map(|part| part as f32)
}

impl DrawList {
    /// Starts recording a part of a window `size` pixels big: no quads, no uploads.
    #[inline]
    pub(crate) fn begin(&mut self, size: [i32; 2]) {
        (self.size, self.failed, self.patch_room) = (size.map(|side| side as f32), false, Shelves::default());
        self.quads.clear();
        self.uploads.clear();
        self.staging.clear();
    }

    /// Records the part `area` (in units) of `ui`'s window, `size` pixels big.
    #[cfg(any(target_vendor = "apple", target_os = "android", target_family = "wasm"))]
    pub(crate) fn record(&mut self, ui: &mut crate::ui::Ui, area: Rect, size: [i32; 2]) {
        self.begin(size);
        let mut canvas = Canvas::recording(self, size[0] as usize, size[1] as usize, (0, 0), ui.scale());
        canvas.clip(area);
        ui.paint(&mut canvas);
    }

    /// Ends a part: `drawn` by the GPU, or not, when what it put in the cache never went up and
    /// the cache is emptied.
    #[inline]
    pub(crate) fn finish(&mut self, drawn: bool) {
        if !drawn {
            (self.cache_room, self.failed) = (Shelves::default(), true);
            self.cache.clear();
            self.extents.clear();
        }
    }

    /// Adds a quad drawn over `at` (cut to `clip`), unless that is empty.
    #[inline]
    fn quad(&mut self, at: [f32; 4], clip: [i32; 4], rows: [[f32; 4]; 4], kind: f32, texture: f32) {
        let [left, top, right, bottom] = cut(at.map(|side| side.clamp(-16_777_216.0, 16_777_216.0) as i32), clip);
        if left < right && top < bottom {
            let place = [left, top, right, bottom].map(|side| side as f32);
            let [width, height] = self.size;
            self.quads.push(Quad { rows: [place, rows[0], rows[1], rows[2], rows[3], [kind, texture, width, height]] });
        }
    }

    /// A square rectangle, its edges on whole pixels (`Canvas::rect`).
    #[inline]
    pub(crate) fn rect(&mut self, r: Rect, color: Color, scale: f32, clip: [i32; 4]) {
        if !color.is_none() {
            let at = [r.x, r.y, r.right(), r.bottom()].map(|side| (side * scale).round());
            self.quad(at, clip, [[0.0; 4], [0.0; 4], [0.0; 4], channels(color)], SOLID, 0.0);
        }
    }

    /// A rounded rectangle, of which only the outer `thick` shows (all of it for infinity), as
    /// `Canvas::ring`.
    #[inline]
    pub(crate) fn ring(&mut self, r: Rect, radius: f32, thick: f32, color: Color, scale: f32, clip: [i32; 4]) {
        if color.is_none() || r.is_empty() {
            return;
        }
        let at = [r.x, r.y, r.right(), r.bottom()].map(|side| (side * scale).round());
        let radius = (radius * scale).min((at[2] - at[0]) / 2.0).min((at[3] - at[1]) / 2.0).max(0.0);
        let thick = if thick.is_finite() { (thick * scale).round().max(1.0) } else { -1.0 };
        self.quad(at, clip, [at, [radius, thick, 0.0, 0.0], [0.0; 4], channels(color)], RING, 0.0);
    }

    /// A soft shadow under a rounded rectangle (`Canvas::shadow`).
    #[inline]
    pub(crate) fn shadow(&mut self, r: Rect, radius: f32, reach: f32, color: Color, scale: f32, clip: [i32; 4]) {
        if color.is_none() || r.is_empty() || reach <= 0.0 {
            return;
        }
        let (reach, drop) = (reach * scale, reach * scale / 4.0);
        let shape = [r.x * scale, r.y * scale + drop, r.right() * scale, r.bottom() * scale + drop];
        let radius = (radius * scale).min((shape[2] - shape[0]) / 2.0).min((shape[3] - shape[1]) / 2.0).max(0.0);
        let at = [(shape[0] - reach).floor(), (shape[1] - reach).floor(), (shape[2] + reach).ceil(), (shape[3] + reach).ceil()];
        self.quad(at, clip, [shape, [radius, reach, drop, 0.0], [0.0; 4], channels(color)], SHADOW, 0.0);
    }

    /// A coverage mask, one byte a pixel, `width` a row, its top left at `x`, `y` in window
    /// pixels: a glyph or an icon (`Canvas::cover`). Kept in the cache texture by its content.
    pub(crate) fn mask(&mut self, x: i32, y: i32, width: usize, cover: &[u8], color: Color, clip: [i32; 4]) {
        let high = cover.len() / width.max(1);
        if width == 0 || high == 0 || color.is_none() {
            return;
        }
        let key = {
            let mut hash = Fast(width as u64);
            std::hash::Hasher::write(&mut hash, cover);
            hash.0
        };
        let Some(texels) = self.kept(key, [width as i32, high as i32]) else { return };
        if let Some(from) = texels.1 {
            let room = &mut self.staging[from..from + width * high];
            room.iter_mut().zip(cover).for_each(|(texel, cover)| *texel = *cover as u32 * 0x0101_0101);
        }
        let [left, top, ..] = texels.0;
        let (at, x, y) = ([x, y, x + width as i32, y + high as i32].map(|side| side as f32), x as f32, y as f32);
        self.quad(at, clip, [[x, y, 0.0, 0.0], [0.0; 4], [left as f32, top as f32, width as f32, high as f32], channels(color)], MASK, 0.0);
    }

    /// Where `key`'s texels are in the cache texture, `size` big, and where in `staging` to
    /// write them if they are new. Nothing where the cache is full: it is emptied, and the CPU
    /// draws this part.
    fn kept(&mut self, key: u64, size: [i32; 2]) -> Option<([i32; 4], Option<usize>)> {
        if let Some(at) = self.cache.get(&key) {
            return Some((*at, None));
        }
        let Some([left, top]) = self.cache_room.place(size[0], size[1]) else {
            (self.failed, self.cache_room) = (true, Shelves::default());
            self.cache.clear();
            self.extents.clear();
            return None;
        };
        let (at, from) = ([left, top, size[0], size[1]], self.staging.len());
        self.staging.resize(from + (size[0] * size[1]) as usize, 0);
        self.uploads.push(Upload { at, from, texture: 0 });
        self.cache.insert(key, at);
        Some((at, Some(from)))
    }

    /// A run of text drawn once and kept by `key`, `size` big, its top left at `at` in window
    /// pixels: three coverages (red, green, blue) a pixel, which `draw` fills in a row after
    /// another, blended in `color`.
    #[cfg(windows)]
    pub(crate) fn run(&mut self, key: u64, at: [i32; 2], size: [i32; 2], color: Color, clip: [i32; 4], draw: impl FnOnce(&mut [u32])) {
        if self.no_runs {
            return self.failed = true;
        }
        let Some(texels) = self.kept(key, size) else { return };
        if let Some(from) = texels.1 {
            draw(&mut self.staging[from..from + (size[0] * size[1]) as usize]);
        }
        let [left, top, ..] = texels.0;
        let place = [at[0], at[1], at[0] + size[0], at[1] + size[1]].map(|side| side as f32);
        let source = [left as f32, top as f32, size[0] as f32, size[1] as f32];
        self.quad(place, clip, [[at[0] as f32, at[1] as f32, 0.0, 0.0], [0.0; 4], source, channels(color)], RUN, 0.0);
    }

    /// A run of text as `run`, but drawn by `draw` on the opaque colour `under`, and laid as it
    /// is where it differs from that. Recorded only where all it changes lies on that colour now
    /// (`solid_under`): says whether it was.
    #[cfg(windows)]
    pub(crate) fn run_on(&mut self, key: u64, at: [i32; 2], size: [i32; 2], under: Color, clip: [i32; 4], draw: impl FnOnce(&mut [u32])) -> bool {
        let Some(texels) = self.kept(key, size) else { return true };
        if let Some(from) = texels.1 {
            let room = &mut self.staging[from..from + (size[0] * size[1]) as usize];
            draw(room);
            self.extents.insert(key, inked(room, size[0], under));
        }
        let [x0, y0, x1, y1] = self.extents.get(&key).copied().unwrap_or_default();
        let changed = cut([at[0] + x0, at[1] + y0, at[0] + x1, at[1] + y1], clip);
        if changed[0] >= changed[2] || changed[1] >= changed[3] {
            return true;
        }
        if self.solid_under(changed) != Some(under) {
            return false;
        }
        let [left, top, ..] = texels.0;
        let place = [at[0], at[1], at[0] + size[0], at[1] + size[1]].map(|side| side as f32);
        let source = [left as f32, top as f32, size[0] as f32, size[1] as f32];
        self.quad(place, clip, [[at[0] as f32, at[1] as f32, 0.0, 0.0], [0.0; 4], source, [255.0; 4]], KEPT, 0.0);
        true
    }

    /// The opaque colour all of `area` (window pixels) shows so far, if one does: the last quad
    /// drawn over any of it, if that is a solid colour over all of it (a rectangle, or a
    /// rounded one away from its corners). Only the last `LOOKED_BACK` quads are looked at.
    #[cfg(windows)]
    pub(crate) fn solid_under(&self, area: [i32; 4]) -> Option<Color> {
        let [left, top, right, bottom] = area.map(|side| side as f32);
        let covers = |r: [f32; 4]| r[0] <= left && r[1] <= top && r[2] >= right && r[3] >= bottom;
        for quad in self.quads.iter().rev().take(LOOKED_BACK) {
            let ([x0, y0, x1, y1], kind, color) = (quad.rows[0], quad.rows[5][0], quad.rows[4]);
            if x1 <= left || x0 >= right || y1 <= top || y0 >= bottom {
                continue;
            }
            let ([radius, thick, ..], [sx0, sy0, sx1, sy1]) = (quad.rows[2], quad.rows[1]);
            // A border whose hole holds all of it draws nothing on it.
            let (hole, inner) = ([sx0 + thick, sy0 + thick, sx1 - thick, sy1 - thick], (radius - thick).max(0.0));
            if kind == RING && thick >= 0.0 && (covers([hole[0] + inner, hole[1], hole[2] - inner, hole[3]]) || covers([hole[0], hole[1] + inner, hole[2], hole[3] - inner])) {
                continue;
            }
            let filled = kind == RING && thick < 0.0 && (covers([x0 + radius, y0, x1 - radius, y1]) || covers([x0, y0 + radius, x1, y1 - radius]));
            let solid = color[3] == 255.0 && (kind == SOLID || filled) && covers([x0, y0, x1, y1]);
            let [red, green, blue, _] = color.map(|part| part as u32);
            return solid.then_some(Color(0xff00_0000 | red << 16 | green << 8 | blue));
        }
        None
    }

    /// Room for `size` texels in this part's patch texture: where they go in it, and where in
    /// `staging` to write them. Nothing where it is full: the CPU draws this part.
    fn patch_room(&mut self, size: [i32; 2]) -> Option<([i32; 2], usize)> {
        let Some(at) = self.patch_room.place(size[0], size[1]) else {
            self.failed = true;
            return None;
        };
        let from = self.staging.len();
        self.uploads.push(Upload { at: [at[0], at[1], size[0], size[1]], from, texture: 1 });
        Some((at, from))
    }

    /// Canvas pixels (premultiplied) laid over what is there, `width` a row, top left at `at`
    /// in window pixels (`Canvas::pixels`). Only the rows and columns inside `clip` go up.
    pub(crate) fn pixels(&mut self, pixels: &[u32], width: usize, [x, y]: [i32; 2], clip: [i32; 4]) {
        let high = (pixels.len() / width.max(1)) as i32;
        let [left, top, right, bottom] = cut([x, y, x.saturating_add(width as i32), y.saturating_add(high)], clip);
        if left >= right || top >= bottom {
            return;
        }
        let Some((texels, from)) = self.patch_room([right - left, bottom - top]) else { return };
        let rows = (top - y) as usize..(bottom - y) as usize;
        rows.for_each(|row| self.staging.extend_from_slice(&pixels[row * width + (left - x) as usize..row * width + (right - x) as usize]));
        debug_assert_eq!(self.staging.len() - from, ((right - left) * (bottom - top)) as usize);
        self.laid([left, top, right, bottom], texels);
    }

    /// A patch quad over `area` (window pixels), its texels at `texels` in the patch texture.
    #[inline]
    fn laid(&mut self, area: [i32; 4], texels: [i32; 2]) {
        let [left, top, right, bottom] = area.map(|side| side as f32);
        let source = [texels[0] as f32, texels[1] as f32, right - left, bottom - top];
        let clip = area;
        self.quad([left, top, right, bottom], clip, [[left, top, 0.0, 0.0], [0.0; 4], source, [255.0; 4]], PATCH, 1.0);
    }

    /// Has the CPU draw something into a patch over `area` (window pixels, cut to `clip`) on a
    /// clear background, which the GPU then lays over what is there. Drawing is clipped as on
    /// the window.
    pub(crate) fn patch(&mut self, area: [i32; 4], clip: [i32; 4], scale: f32, draw: impl FnOnce(&mut Canvas)) {
        let [left, top, right, bottom] = cut(area, clip);
        if left >= right || top >= bottom {
            return;
        }
        let (wide, high) = ((right - left) as usize, (bottom - top) as usize);
        let Some((texels, from)) = self.patch_room([wide as i32, high as i32]) else { return };
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(wide * high, 0);
        let mut canvas = Canvas::new(&mut scratch, wide, high, (left, top), scale);
        canvas.restore_clip([left, top, right, bottom]);
        draw(&mut canvas);
        self.staging.extend_from_slice(&scratch);
        debug_assert_eq!(self.staging.len() - from, wide * high);
        self.scratch = scratch;
        self.laid([left, top, right, bottom], texels);
    }

    /// Whatever `edges` (window pixels) encloses, by the nonzero rule, in one colour
    /// (`Canvas::fill_one`): each pixel's exact area inside, as the CPU's. Drawn by the CPU into a
    /// patch where a band has more than `MOST_EDGES`.
    pub(crate) fn fill_one(&mut self, edges: Edges, color: Color, scale: f32, clip: [i32; 4]) {
        if color.is_none() {
            return;
        }
        let mut kept = std::mem::take(&mut self.edges);
        kept.clear();
        edges(&mut |a, b| kept.push([a.x, a.y, b.x, b.y]));
        let [x0, y0, x1, y1] = reached(&kept, 0.0);
        let far = |at: f32| at.clamp(-16_777_216.0, 16_777_216.0) as i32;
        let area = [far(x0.floor()), far(y0.floor()), far(x1.ceil()).saturating_add(1), far(y1.ceil())];
        self.edges = kept;
        if !self.banded(area, clip, [1.0, 0.0], PATH, color) {
            let kept = std::mem::take(&mut self.edges);
            self.patch(area, clip, scale, |canvas| canvas.fill_with(&mut |each| kept.iter().for_each(|e| each(pt(e[0], e[1]), pt(e[2], e[3]))), false, |_, _| color));
        }
    }

    /// A path's stroke (`Canvas::stroke`): each pixel the mean of its four subcells, each as
    /// much as the nearest segment covers it, as the CPU's. Drawn by the CPU into a patch where a
    /// band has more than `MOST_EDGES`.
    #[allow(clippy::too_many_arguments)] // (What `Canvas::stroke` takes, and the canvas's scale and clip.)
    pub(crate) fn stroke(&mut self, path: &Path, at: Point, size: f32, width: f32, color: Color, scale: f32, clip: [i32; 4]) {
        if color.is_none() {
            return;
        }
        let fine = FINE as f32;
        let (s, to) = (size * scale * fine, crate::geom::pt(at.x * scale * fine, at.y * scale * fine));
        let reach = (width * s / 2.0).max(0.5 * fine) + SOFT;
        let mut kept = std::mem::take(&mut self.edges);
        kept.clear();
        path.flatten(s, to, fine, |a, b| kept.push([a.x, a.y, b.x, b.y]));
        let [x0, y0, x1, y1] = reached(&kept, reach).map(|side| side / fine);
        let far = |at: f32| at.clamp(-16_777_216.0, 16_777_216.0) as i32;
        let area = [far(x0.floor()), far(y0.floor()), far(x1.ceil()), far(y1.ceil())];
        self.edges = kept;
        if !self.banded(area, clip, [fine, reach], STROKE, color) {
            self.patch(area, clip, scale, |canvas| canvas.stroke(path, at, size, width, color));
        }
    }

    /// Puts the edges up (window pixels; or subcells, `fine[0]` a pixel, each reaching
    /// `fine[1]` round) in bands of `BAND` rows, with a quad of `kind` over each band of `area`
    /// (cut to `clip`). Says whether it did; not where a band has more than `MOST_EDGES`.
    fn banded(&mut self, area: [i32; 4], clip: [i32; 4], [fine, reach]: [f32; 2], kind: f32, color: Color) -> bool {
        let [left, top, right, bottom] = cut(area, clip);
        if left >= right || top >= bottom || self.edges.is_empty() {
            return true;
        }
        let (mut bands, mut total, mut y) = (std::mem::take(&mut self.bands), 0, top);
        bands.clear();
        while y < bottom {
            let end = (y + BAND).min(bottom);
            let (from, to) = (y as f32 * fine - reach, end as f32 * fine + reach);
            let count = self.edges.iter().filter(|e| e[1].min(e[3]) < to && e[1].max(e[3]) > from).count();
            bands.push([y, end, total, count as i32]);
            (total, y) = (total + count as i32, end);
        }
        let most = bands.iter().map(|band| band[3]).max().unwrap_or(0) as usize;
        let wide = (total as usize * 4).clamp(4, SIDE as usize);
        let room = (most <= MOST_EDGES).then(|| self.patch_room([wide as i32, (total as usize * 4).div_ceil(wide).max(1) as i32])).flatten();
        let Some((texels, from)) = room else {
            self.bands = bands;
            return false;
        };
        self.staging.resize(from + wide * (total as usize * 4).div_ceil(wide).max(1), 0);
        let mut at = from;
        for band in &bands {
            let (from, to) = (band[0] as f32 * fine - reach, band[1] as f32 * fine + reach);
            let reaching = self.edges.iter().filter(|e| e[1].min(e[3]) < to && e[1].max(e[3]) > from);
            reaching.flat_map(|e| e.map(f32::to_bits)).for_each(|bits| (self.staging[at], at) = (bits, at + 1));
        }
        let (shape, source) = ([0.0, 0.0, wide as f32, reach - SOFT], [texels[0] as f32, texels[1] as f32, 0.0, 0.0]);
        for band in &bands {
            let place = [left, band[0], right, band[1]].map(|side| side as f32);
            self.quad(place, clip, [[band[2] as f32, band[3] as f32, shape[2], shape[3]], [0.0; 4], source, channels(color)], kind, 1.0);
        }
        self.bands = bands;
        true
    }

    /// A rounded rectangle filled with a gradient (`Canvas::gradient`): its stops go up, and
    /// the GPU works out each pixel's colour as the CPU does.
    #[allow(clippy::too_many_arguments)] // (What `Canvas::gradient` takes, and the canvas's scale and clip.)
    pub(crate) fn gradient(&mut self, r: Rect, radius: f32, colors: &[(f32, Color)], turn: Option<f32>, scale: f32, clip: [i32; 4]) {
        let at = [r.x, r.y, r.right(), r.bottom()].map(|side| (side * scale).round());
        let shown = cut(at.map(|side| side.clamp(-16_777_216.0, 16_777_216.0) as i32), clip);
        if colors.is_empty() || shown[0] >= shown[2] || shown[1] >= shown[3] {
            return;
        }
        if colors.len() * 2 > SIDE as usize {
            return self.patch(at.map(whole).map(|side| side as i32), clip, scale, |canvas| canvas.gradient(r, radius, colors, turn));
        }
        let radius = (radius * scale).min((at[2] - at[0]) / 2.0).min((at[3] - at[1]) / 2.0).max(0.0);
        let Some((texels, from)) = self.patch_room([colors.len() as i32 * 2, 1]) else { return };
        self.staging.extend(colors.iter().flat_map(|(along, color)| [color.0, along.to_bits()]));
        debug_assert_eq!(self.staging.len() - from, colors.len() * 2);
        let (sin, cos) = turn.map_or((0.0, 0.0), |turn| turn.to_radians().sin_cos());
        let (more, source) = ([radius, sin, cos, turn.is_some() as u8 as f32], [texels[0] as f32, texels[1] as f32, colors.len() as f32, 0.0]);
        self.quad(at, clip, [at, more, source, [255.0; 4]], GRADIENT, 1.0);
    }

    /// Whatever `edges` encloses (`Canvas::fill_with`), drawn by the CPU into a patch.
    pub(crate) fn fill_with(&mut self, edges: Edges, even_odd: bool, ink: impl FnMut(i32, i32) -> Color, scale: f32, clip: [i32; 4]) {
        let area = bounds(edges, 1.0);
        self.patch(area, clip, scale, |canvas| canvas.fill_with(edges, even_odd, ink));
    }

    /// A picture in a box at `r` with rounded corners (`Canvas::rounded_picture`). Its pixels
    /// go up as they are and the GPU lays them as the CPU would; one reduced more than
    /// `MOST_REDUCED` times, or too big to go up, is drawn by the CPU into a patch.
    pub(crate) fn picture(&mut self, pixels: &[u32], width: usize, r: Rect, radius: f32, scale: f32, clip: [i32; 4]) {
        let height = pixels.len() / width.max(1);
        let shape = [r.x, r.y, r.right(), r.bottom()].map(|side| whole(side * scale));
        let [left, top, right, bottom] = shape.map(|side| side as i32);
        let shown = cut([left, top, right, bottom], clip);
        if shown[0] >= shown[2] || bottom <= top || height == 0 {
            return;
        }
        let steps = [width as f32 / (right - left) as f32, height as f32 / (bottom - top) as f32];
        let fits = width as i32 <= SIDE && height as i32 <= SIDE && steps.iter().all(|step| *step <= MOST_REDUCED);
        if !fits {
            return self.patch([left, top, right, bottom], clip, scale, |canvas| canvas.rounded_picture(pixels, width, r, radius));
        }
        let Some((texels, _)) = self.patch_room([width as i32, height as i32]) else { return };
        self.staging.extend_from_slice(&pixels[..width * height]);
        let source = [texels[0] as f32, texels[1] as f32, width as f32, height as f32];
        let more = [radius * scale, steps[0], steps[1], 0.0];
        self.quad(shape, clip, [shape, more, source, [255.0; 4]], PICTURE, 1.0);
    }
}

/// Where a run drawn on `under` differs from it (left, top, right, bottom), its texels `width`
/// a row: those made opaque, the rest clear, so only what was drawn is laid.
#[cfg(windows)]
fn inked(texels: &mut [u32], width: i32, under: Color) -> [i32; 4] {
    let mut extent = [i32::MAX, i32::MAX, 0, 0];
    for (at, texel) in (0..).zip(texels.iter_mut()) {
        let ([x, y], changed) = ([at % width, at / width], (*texel ^ under.0) & 0x00ff_ffff != 0);
        *texel = if changed { *texel | 0xff00_0000 } else { 0 };
        if changed {
            extent = [extent[0].min(x), extent[1].min(y), extent[2].max(x + 1), extent[3].max(y + 1)];
        }
    }
    extent
}

/// The pixels the edges `each` gives can reach, `reach` more each way: left, top, right,
/// bottom.
fn bounds(edges: Edges, reach: f32) -> [i32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    edges(&mut |a, b| {
        (x0, y0) = (x0.min(a.x).min(b.x), y0.min(a.y).min(b.y));
        (x1, y1) = (x1.max(a.x).max(b.x), y1.max(a.y).max(b.y));
    });
    let far = |at: f32| at.clamp(-16_777_216.0, 16_777_216.0) as i32;
    [far((x0 - reach).floor()), far((y0 - reach).floor()), far((x1 + reach).ceil()) + 1, far((y1 + reach).ceil()) + 1]
}

/// The ends of `edges` reach this far, `reach` more each way: left, top, right, bottom.
fn reached(edges: &[[f32; 4]], reach: f32) -> [f32; 4] {
    let all = edges.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |all, e| [all[0].min(e[0]).min(e[2]), all[1].min(e[1]).min(e[3]), all[2].max(e[0]).max(e[2]), all[3].max(e[1]).max(e[3])]);
    [all[0] - reach, all[1] - reach, all[2] + reach, all[3] + reach]
}

/// Whether a part of a window `area` pixels big is drawn by the GPU: never, where large, or
/// always.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawing {
    Never,
    Large,
    Always,
}

impl Drawing {
    /// What `TAUT_DRAW` (cpu, gpu, auto) says over `asked`: `Large` for `Renderer::Auto`,
    /// `Always` for a GPU path named, never on a software device (`gpu` false).
    #[inline]
    pub(crate) fn chosen(auto: bool, gpu: bool, said: Option<&str>) -> Drawing {
        match said.map(str::to_ascii_lowercase).as_deref() {
            Some("cpu") => Drawing::Never,
            Some("gpu") => Drawing::Always,
            _ if !gpu => Drawing::Never,
            Some("auto") => Drawing::Large,
            _ if auto => Drawing::Large,
            _ => Drawing::Always,
        }
    }
    /// Whether a part `area` (left, top, right, bottom) is drawn by the GPU.
    #[inline]
    pub(crate) fn takes(self, area: [i32; 4]) -> bool {
        let pixels = (area[2] - area[0]) as i64 * (area[3] - area[1]) as i64;
        self == Drawing::Always || (self == Drawing::Large && pixels >= LARGE)
    }
}

/// The shaders' work per pixel, in the common part of HLSL and GLSL: each language's prelude
/// defines `vec2`/`vec4`/`ivec2`/`fract`, `texel(which, x, y)` (0 to 1, red first),
/// `place` (the quad's rows, flat), and the pixel's centre `at`. `shade` gives the colour to
/// blend in (premultiplied) and, for GDI's runs, how much of each channel it covers.
pub(crate) const SHADING: &str = r#"
float inside_of(vec2 p, vec4 r, float radius) {
    float dx = max(r.x + radius - p.x, p.x - (r.z - radius));
    float dy = max(r.y + radius - p.y, p.y - (r.w - radius));
    if (dx > 0.0 && dy > 0.0) {
        return clamp(radius - sqrt(dx * dx + dy * dy) + 0.5, 0.0, 1.0);
    }
    return 1.0;
}
float ring_of(vec2 p, vec4 r, float radius, float thick) {
    float outer = inside_of(p, r, radius);
    if (thick < 0.0 || outer <= 0.0 || p.x < r.x + thick || p.x > r.z - thick || p.y < r.y + thick || p.y > r.w - thick) {
        return outer;
    }
    return outer - min(inside_of(p, vec4(r.x + thick, r.y + thick, r.z - thick, r.w - thick), max(radius - thick, 0.0)), outer);
}
float shadow_of(vec2 p, vec4 r, float radius, float reach, float drop) {
    float dy = max(max(r.y + radius - p.y, p.y - (r.w - radius)), 0.0);
    float dx = max(max(r.x + radius - p.x, p.x - (r.z - radius)), 0.0);
    float away = sqrt(dx * dx + dy * dy) - radius;
    float near = 0.0;
    if (away > -1.0) {
        near = clamp(1.0 - away / reach, 0.0, 1.0);
    } else if (p.y >= r.w - drop - radius - 1.0) {
        near = 1.0;
    }
    return floor(near * near * 255.0);
}
vec4 bytes(int which, float x, float y) {
    return floor(texel(which, int(x), int(y)) * 255.0 + 0.5);
}
vec4 mixed(vec4 a, vec4 b, float t) {
    return floor((a * (256.0 - t) + b * t) / 256.0);
}
vec4 enlarged(vec4 shape, vec4 more, vec4 source, vec2 p) {
    float v = clamp((p.y - shape.y) * more.z - 0.5, 0.0, source.w - 1.0);
    float u = clamp((p.x - shape.x) * more.y - 0.5, 0.0, source.z - 1.0);
    float above = floor(v);
    float below = min(above + 1.0, source.w - 1.0);
    float left = floor(u);
    float right = min(left + 1.0, source.z - 1.0);
    float down = floor(fract(v) * 256.0);
    float along = floor(fract(u) * 256.0);
    vec4 top = mixed(bytes(1, source.x + left, source.y + above), bytes(1, source.x + right, source.y + above), along);
    vec4 bottom = mixed(bytes(1, source.x + left, source.y + below), bytes(1, source.x + right, source.y + below), along);
    return mixed(top, bottom, down);
}
vec4 reduced(vec4 shape, vec4 more, vec4 source, vec2 p) {
    vec2 stride = more.yz;
    vec2 most = vec2(max(stride.x, 1.0), max(stride.y, 1.0));
    vec2 from = (p - shape.xy) * stride - most / 2.0;
    float left = max(from.x, 0.0);
    float right = min(from.x + most.x, source.z);
    float top = max(from.y, 0.0);
    float bottom = min(from.y + most.y, source.w);
    vec4 sum = vec4(0.0, 0.0, 0.0, 0.0);
    float y = floor(top);
    for (int j = 0; j < 10; j++) {
        if (y >= ceil(bottom)) {
            break;
        }
        float high = min(bottom, y + 1.0) - max(top, y);
        float x = floor(left);
        for (int i = 0; i < 10; i++) {
            if (x >= ceil(right)) {
                break;
            }
            vec4 pixel = bytes(1, source.x + x, source.y + y);
            float solid = pixel.a * high * (min(right, x + 1.0) - max(left, x));
            sum += vec4(pixel.rgb * solid, solid);
            x += 1.0;
        }
        y += 1.0;
    }
    float area = (right - left) * (bottom - top);
    float solid = max(sum.a, 0.000001);
    return min(floor(vec4(sum.rgb / solid, sum.a / area) + 0.5), vec4(255.0, 255.0, 255.0, 255.0));
}
float bits_of(vec4 b) {
    return as_float(uint(b.a) << 24 | uint(b.r) << 16 | uint(b.g) << 8 | uint(b.b));
}
vec4 gradient_at(vec4 shape, vec4 more, vec4 source, vec2 p) {
    float mid_x = (shape.x + shape.z) / 2.0;
    float mid_y = (shape.y + shape.w) / 2.0;
    float half_x = max((shape.z - shape.x - 1.0) / 2.0, 0.5);
    float half_y = max((shape.w - shape.y - 1.0) / 2.0, 0.5);
    float reach = max(abs(more.y) * half_x + abs(more.z) * half_y, 0.5);
    float dx = p.x - mid_x;
    float dy = p.y - mid_y;
    float along = sqrt((dx / half_x) * (dx / half_x) + (dy / half_y) * (dy / half_y));
    if (more.w > 0.5) {
        along = 0.5 + (dx * more.y - dy * more.z) / (2.0 * reach);
    }
    int count = int(source.z);
    int after = count;
    for (int i = 0; i < count; i++) {
        if (bits_of(bytes(1, source.x + float(2 * i + 1), source.y)) > along) {
            after = i;
            break;
        }
    }
    if (after == 0) {
        return bytes(1, source.x, source.y);
    }
    vec4 before = bytes(1, source.x + float(2 * after - 2), source.y);
    if (after == count) {
        return before;
    }
    float from = bits_of(bytes(1, source.x + float(2 * after - 1), source.y));
    float to = bits_of(bytes(1, source.x + float(2 * after + 1), source.y));
    float t = floor(clamp((along - from) / (to - from), 0.0, 1.0) * 256.0);
    return mixed(before, bytes(1, source.x + float(2 * after), source.y), t);
}
vec4 edge_at(vec4 shape, vec4 source, int i) {
    int wide = int(shape.z);
    int at = (int(shape.x) + i) * 4;
    float x = source.x + float(at - at / wide * wide);
    float y = source.y + float(at / wide);
    return vec4(bits_of(bytes(1, x, y)), bits_of(bytes(1, x + 1.0, y)), bits_of(bytes(1, x + 2.0, y)), bits_of(bytes(1, x + 3.0, y)));
}
float right_of(float u) {
    float c = clamp(u, 0.0, 1.0);
    return min(u, 0.0) + c - c * c / 2.0;
}
float edge_cover(vec4 e, vec2 pixel) {
    if (e.y == e.w) {
        return 0.0;
    }
    float dir = 1.0;
    vec2 a = e.xy;
    vec2 b = e.zw;
    if (a.y > b.y) {
        dir = -1.0;
        a = e.zw;
        b = e.xy;
    }
    float top = max(a.y, pixel.y);
    float bottom = min(b.y, pixel.y + 1.0);
    if (bottom <= top) {
        return 0.0;
    }
    float slope = (b.x - a.x) / (b.y - a.y);
    float u0 = a.x + (top - a.y) * slope - pixel.x;
    float u1 = a.x + (bottom - a.y) * slope - pixel.x;
    float tall = bottom - top;
    if (abs(u1 - u0) < 0.0001) {
        return dir * tall * clamp(1.0 - (u0 + u1) / 2.0, 0.0, 1.0);
    }
    return dir * tall * (right_of(u1) - right_of(u0)) / (u1 - u0);
}
float path_cover(vec4 shape, vec4 source, vec2 p) {
    vec2 pixel = floor(p);
    float sum = 0.0;
    int count = int(shape.y);
    for (int i = 0; i < count; i++) {
        sum += edge_cover(edge_at(shape, source, i), pixel);
    }
    return floor(min(abs(sum), 1.0) * 255.0 + 0.5);
}
float corner_of(float past) {
    return max(past, 0.0) * max(past, 0.0);
}
float subcell(vec4 e, vec2 c, vec4 square) {
    float dx = e.z - e.x;
    float dy = e.w - e.y;
    float across = c.x - e.x;
    float down = c.y - e.y;
    float along = clamp((across * dx + down * dy) * (1.0 / max(dx * dx + dy * dy, 0.000000000001)), 0.0, 1.0);
    float away = sqrt((across - along * dx) * (across - along * dx) + (down - along * dy) * (down - along * dy));
    float deep = square.z - away;
    float cover = (corner_of(deep) - corner_of(deep - square.x) - corner_of(deep - square.y) + corner_of(deep - square.x - square.y)) * square.w;
    return floor(clamp(cover, 0.0, 1.0) * 255.0 + 0.5);
}
float stroke_cover(vec4 shape, vec4 source, vec2 p) {
    vec2 pixel = floor(p) * 2.0 + vec2(0.5, 0.5);
    vec4 most = vec4(0.0, 0.0, 0.0, 0.0);
    int count = int(shape.y);
    for (int i = 0; i < count; i++) {
        vec4 e = edge_at(shape, source, i);
        float dx = e.z - e.x;
        float dy = e.w - e.y;
        float length_of = sqrt(dx * dx + dy * dy);
        float p_of = 0.0;
        float q_of = 1.0;
        if (length_of > 0.0) {
            p_of = abs(dy / length_of);
            q_of = abs(dx / length_of);
        }
        float a = max(min(p_of, q_of), 1.0 / 1024.0);
        float b = max(max(p_of, q_of), 0.70710677);
        vec4 square = vec4(a, b, shape.w + (a + b) / 2.0, 1.0 / (2.0 * a * b));
        most = max(most, vec4(subcell(e, pixel, square), subcell(e, pixel + vec2(1.0, 0.0), square), subcell(e, pixel + vec2(0.0, 1.0), square), subcell(e, pixel + vec2(1.0, 1.0), square)));
    }
    return floor((most.x + most.y + most.z + most.w) / 4.0);
}
vec4 laid(vec4 color, float cover) {
    float a = floor(color.a * cover / 255.0);
    float k = a >= 255.0 ? 1.0 : a / 256.0;
    return vec4(color.rgb / 255.0 * k, k);
}
vec4 shade(vec2 at, vec4 shape, vec4 more, vec4 source, vec4 color, float kind, out vec4 covers) {
    vec2 p = at;
    vec4 drawn = vec4(0.0, 0.0, 0.0, 0.0);
    if (kind == 0.0) {
        drawn = laid(color, 255.0);
    } else if (kind == 1.0) {
        drawn = laid(color, floor(ring_of(p, shape, more.x, more.y) * 255.0));
    } else if (kind == 2.0) {
        drawn = laid(color, shadow_of(p, shape, more.x, more.y, more.z));
    } else if (kind == 3.0) {
        drawn = laid(color, bytes(0, source.x + floor(p.x) - shape.x, source.y + floor(p.y) - shape.y).a);
    } else if (kind == 4.0) {
        vec4 cover = bytes(0, source.x + floor(p.x) - shape.x, source.y + floor(p.y) - shape.y);
        vec3 a = floor(color.a * cover.rgb / 255.0) / 255.0;
        covers = vec4(a, max(a.r, max(a.g, a.b)));
        return vec4(color.rgb / 255.0 * a, covers.a);
    } else if (kind == 8.0) {
        drawn = laid(gradient_at(shape, more, source, p), floor(inside_of(p, shape, more.x) * 255.0));
    } else if (kind == 9.0) {
        drawn = laid(color, path_cover(shape, source, p));
    } else if (kind == 10.0) {
        drawn = laid(color, stroke_cover(shape, source, p));
    } else if (kind == 5.0) {
        vec4 pixel = more.y > 1.0 || more.z > 1.0 ? reduced(shape, more, source, p) : enlarged(shape, more, source, p);
        float radius = min(min(more.x, (shape.z - shape.x) / 2.0), (shape.w - shape.y) / 2.0);
        float cover = radius > 0.0 ? floor(inside_of(p, shape, radius) * 255.0) : 255.0;
        drawn = laid(pixel, cover);
    } else {
        drawn = bytes(kind == 6.0 ? 1 : 0, source.x + floor(p.x) - shape.x, source.y + floor(p.y) - shape.y) / 255.0;
    }
    covers = vec4(drawn.a, drawn.a, drawn.a, drawn.a);
    return drawn;
}
"#;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Icon, Picture, Ui};
    use crate::{LocalClipboard, Surface, geom::Size, text::BuiltInFont};

    /// The most a channel of a pixel the GPU drew differs from the CPU's.
    pub(crate) const TOLERANCE: u32 = 4;

    /// A window of every kind of thing the GPU draws, as the CPU drew it.
    pub(crate) struct Scene {
        pub(crate) ui: Ui,
        pub(crate) size: [i32; 2],
        /// What the texture held before: none of it shows.
        pub(crate) under: Vec<u32>,
        pub(crate) cpu: Vec<u32>,
    }

    /// Rounded and square boxes, see-through and solid, borders, a shadow, an icon, pictures
    /// enlarged, reduced and rounded (made the first time), text; gradients along and around,
    /// filled shapes and strokes.
    pub(crate) fn every_kind(ui: &mut Ui, pictures: &mut Option<[Picture; 2]>) {
        let [big, small] = *pictures.get_or_insert_with(|| pictures_of(ui));
        let (red, ink) = (Color::rgb(0xd02020), Color::rgb(0x203040));
        ui.column().grow().pad(7.0).gap(5.0).fill(Color::rgb(0xf4f1ea)).open();
        ui.label("Plain words, and more of them: jumpy quartz glyphs 0123");
        ui.row().gap(7.0).open();
        ui.el().size(40.0, 40.0).fill(red).radius(9.5).show();
        ui.el().size(41.0, 37.0).fill(Color::rgb(0x2050a0).with_alpha(0.6)).border(ink, 3.0).radius(10.0).show();
        ui.el().size(40.0, 40.0).fill(Color::WHITE).shadow(10.0).radius(6.0).show();
        ui.el().size(30.0, 30.0).border(red.with_alpha(0.5), 1.0).show();
        ui.el().icon(Icon::line("M2 2 14 14M14 2 2 14"), 31.0, ink).show();
        ui.close();
        ui.row().gap(7.0).open();
        ui.el().picture(small).size(61.0, 43.0).radius(8.0).show();
        ui.el().picture(big).size(33.0, 21.0).show();
        ui.el().picture(big).size(17.0, 12.0).show();
        ui.el().size(40.0, 40.0).gradient(red, Color::rgb(0x2040ff)).show();
        ui.el().size(40.0, 40.0).shape(&Path::svg("M0 0h40v40h-40z M10 10h20v20h-20z"), red).show();
        ui.close();
        ui.row().gap(7.0).open();
        ui.el().size(40.0, 40.0).outline(&Path::svg("M4 30C10 2 30 2 36 30L20 36"), 3.0, ink).show();
        let stops = [(0.0, red), (0.4, Color::rgb(0xf0c020)), (1.0, Color::rgb(0x2040ff).with_alpha(0.5))];
        ui.el().size(56.0, 30.0).radius(8.0).gradient_along(30.0, &stops).show();
        ui.el().size(40.0, 40.0).radius(20.0).gradient_around(&[(0.0, Color::WHITE), (1.0, ink)]).show();
        ui.el().size(40.0, 40.0).shape(&Path::svg("M20 2 38 38H2Z"), Color::rgb(0x30a050).with_alpha(0.7)).show();
        ui.el().size(40.0, 12.0).outline(&Path::svg("M0 6h40"), 1.0, red).show();
        ui.close();
        ui.label("Small 9");
        ui.close();
    }

    /// A larger picture of bands of colour, and a small one of see-through stripes.
    fn pictures_of(ui: &mut Ui) -> [Picture; 2] {
        let small = (0..48).map(|i| if i % 5 == 0 { 0x40ff_8000 } else { 0xff00_0000 | (i * 0x05_0b11) as u32 }).collect();
        let big = (0..96 * 64).map(|i: u32| 0xff00_0000 | ((i % 96) * 2) << 16 | ((i / 96) * 4) << 8 | ((i % 7) * 30)).collect();
        [ui.add_picture(96, big), ui.add_picture(8, small)]
    }

    impl Scene {
        /// The scene at `scale`, drawn by the CPU.
        pub(crate) fn new(scale: f32) -> Scene {
            let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            ui.resize(Size { w: 360.0, h: 210.0 }, scale);
            let mut pictures = None;
            ui.run(&mut |ui| every_kind(ui, &mut pictures));
            let cpu = Surface::draw(&mut ui, 64);
            let size = [cpu.width as i32, cpu.height as i32];
            Scene { ui, size, under: vec![0xff12_3456; cpu.pixels.len()], cpu: cpu.pixels }
        }

        /// Records the scene into `list`, all of the window.
        pub(crate) fn record(&mut self, list: &mut DrawList) {
            list.begin(self.size);
            let mut canvas = Canvas::recording(list, self.size[0] as usize, self.size[1] as usize, (0, 0), self.ui.scale());
            self.ui.paint(&mut canvas);
        }
    }

    /// Checks what the GPU drew against the CPU: no channel more than `TOLERANCE` off (alpha
    /// aside: a window has none). Says how many pixels differ at all.
    pub(crate) fn compare(scene: &Scene, gpu: &[u32], what: &str) -> usize {
        let width = scene.size[0] as usize;
        let off = |a: u32, b: u32| [16, 8, 0].map(|shift| (a >> shift & 255).abs_diff(b >> shift & 255)).into_iter().max().unwrap_or(0);
        let (mut most, mut at, mut differ) = (0, 0, 0);
        for (i, (cpu, gpu)) in scene.cpu.iter().zip(gpu).enumerate() {
            let by = off(*cpu, *gpu);
            differ += (by > 0) as usize;
            if by > most {
                (most, at) = (by, i);
            }
        }
        eprintln!("{what}: {differ} of {} pixels differ, at most by {most} (at {}, {})", gpu.len(), at % width, at / width);
        assert!(most <= TOLERANCE, "{what}: pixel {}, {} is {:08x} on the GPU, {:08x} on the CPU", at % width, at / width, gpu[at], scene.cpu[at]);
        differ
    }

    #[test]
    fn shelves_fill_row_by_row_and_say_when_full() {
        let mut room = Shelves::default();
        assert_eq!(room.place(1500, 10), Some([0, 0]));
        assert_eq!(room.place(600, 20), Some([0, 10]), "too wide for the first shelf");
        assert_eq!(room.place(100, 5), Some([600, 10]));
        assert_eq!(room.place(SIDE + 1, 1), None);
        assert_eq!(room.place(10, SIDE), None, "no shelf left that tall");
    }

    #[test]
    fn a_glyph_goes_up_once_and_is_found_by_its_content_after() {
        let mut list = DrawList::default();
        list.begin([100, 100]);
        let cover = [0u8, 128, 255, 64];
        list.mask(5, 6, 2, &cover, Color::BLACK, [0, 0, 100, 100]);
        assert_eq!((list.uploads.len(), list.staging[..4].to_vec()), (1, vec![0, 0x8080_8080, 0xffff_ffff, 0x4040_4040]));
        list.begin([100, 100]);
        list.mask(50, 60, 2, &cover, Color::BLACK, [0, 0, 100, 100]);
        assert!(list.uploads.is_empty(), "kept from before");
        assert_eq!(list.quads[0].rows[0], [50.0, 60.0, 52.0, 62.0]);
        list.mask(0, 0, 2, &[1, 2, 3, 4], Color::BLACK, [0, 0, 100, 100]);
        assert_eq!(list.uploads.len(), 1, "another glyph is another");
    }

    #[test]
    fn a_quad_is_cut_to_the_clip_and_nothing_outside_is_drawn() {
        let mut list = DrawList::default();
        list.begin([100, 100]);
        list.rect(crate::geom::rect(-10.0, 5.0, 50.0, 10.0), Color::BLACK, 1.0, [0, 0, 30, 100]);
        list.rect(crate::geom::rect(40.0, 5.0, 50.0, 10.0), Color::BLACK, 1.0, [0, 0, 30, 100]);
        assert_eq!(list.quads.len(), 1);
        assert_eq!(list.quads[0].rows[0], [0.0, 5.0, 30.0, 15.0]);
        assert_eq!(list.quads[0].rows[5], [SOLID, 0.0, 100.0, 100.0]);
    }

    #[test]
    fn what_a_patch_draws_is_cut_to_the_clip_and_the_patch_texture_full_fails_the_part() {
        let mut list = DrawList::default();
        list.begin([4000, 4000]);
        list.patch([0, 0, 10, 10], [5, 0, 100, 100], 1.0, |canvas| canvas.rect(crate::geom::rect(0.0, 0.0, 10.0, 10.0), Color::rgb(0xff0000)));
        assert_eq!(list.staging.len(), 50);
        assert!(list.staging.iter().all(|pixel| *pixel == 0xffff_0000));
        assert!(!list.failed);
        list.patch([0, 0, 3000, 10], [0, 0, 4000, 4000], 1.0, |_| {});
        assert!(list.failed, "wider than the patch texture");
    }

    #[test]
    fn gradients_shapes_and_strokes_are_shader_math_not_patches() {
        let mut scene = Scene::new(1.5);
        let mut list = DrawList::default();
        scene.record(&mut list);
        let kinds: Vec<f32> = list.quads.iter().map(|quad| quad.rows[5][0]).collect();
        assert!(!kinds.contains(&PATCH), "nothing in the scene is drawn by the CPU");
        assert!([GRADIENT, PATH, STROKE].iter().all(|kind| kinds.contains(kind)), "{kinds:?}");
    }

    #[test]
    fn a_path_is_cut_into_bands_and_one_too_busy_is_drawn_by_the_cpu() {
        let mut list = DrawList::default();
        list.begin([200, 200]);
        let triangle = Path::svg("M10 10 60 10 35 60Z");
        list.fill_one(&mut |each| triangle.flatten(1.0, Point::default(), 1.0, each), Color::BLACK, 1.0, [0, 0, 200, 200]);
        let bands: Vec<[f32; 4]> = list.quads.iter().map(|quad| quad.rows[0]).collect();
        assert_eq!(bands, [[10.0, 10.0, 61.0, 26.0], [10.0, 26.0, 61.0, 42.0], [10.0, 42.0, 61.0, 58.0], [10.0, 58.0, 61.0, 60.0]]);
        assert_eq!(list.quads[0].rows[1][1], 2.0, "the band's edges: the two slanted ones (a level one covers nothing)");
        assert_eq!(list.quads[3].rows[1][1], 2.0, "the bottom band's edges");
        // A zigzag of more edges than a band may have, all across one band.
        let mut zigzag = String::from("M0 0");
        (1..MOST_EDGES + 2).for_each(|at| zigzag.push_str(&format!(" L{} {}", at % 2 * 10, at as f32 * 0.01)));
        let zigzag = Path::svg(&zigzag);
        list.begin([200, 200]);
        list.fill_one(&mut |each| zigzag.flatten(1.0, Point::default(), 1.0, each), Color::BLACK, 1.0, [0, 0, 200, 200]);
        assert_eq!(list.quads.iter().map(|quad| quad.rows[5][0]).collect::<Vec<_>>(), [PATCH]);
    }

    #[test]
    fn auto_draws_only_large_parts_on_a_gpu_and_the_user_has_the_last_word() {
        assert_eq!(Drawing::chosen(true, true, None), Drawing::Large);
        assert_eq!(Drawing::chosen(false, true, None), Drawing::Always);
        assert_eq!(Drawing::chosen(false, false, None), Drawing::Never, "not on a software device");
        assert_eq!(Drawing::chosen(true, false, Some("GPU")), Drawing::Always);
        assert_eq!(Drawing::chosen(false, true, Some("cpu")), Drawing::Never);
        assert!(Drawing::Large.takes([0, 0, 800, 600]) && !Drawing::Large.takes([0, 0, 300, 40]));
    }
}
