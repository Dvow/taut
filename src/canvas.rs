//! The software rasterizer: a pixel buffer and the few primitives a UI is made of. Rectangles
//! (square or rounded, filled or outlined), paths, and glyph coverage from the text system. All
//! anti-aliased, on the CPU.
//!
//! The buffer need not be the whole window. A canvas knows where its buffer sits in the window,
//! so a tall window can be drawn through a short buffer, strip by strip, and memory does not
//! grow with the window.
use crate::color::Color;
use crate::geom::{Point, Rect, pt, rect};
use crate::path::Path;

/// A shape's edges: calls the given function once per straight edge, with its two ends in
/// window pixels.
pub type Edges<'a> = &'a mut dyn FnMut(&mut dyn FnMut(Point, Point));

/// Subcells per pixel, each way, when stroking: lines closer than a pixel stay apart.
pub(crate) const FINE: usize = 2;
/// Pixels per eight subcells: skipped at once where no stroke reached.
const GROUP: usize = 8 / FINE;
/// How far past a line's edge a subcell's centre can be, in subcells, and the line still cover
/// some of it: half the diagonal, plus a little.
pub(crate) const SOFT: f32 = 0.75;

/// The drawing target: a pixel buffer (usually a few-row strip of the window), on which shapes,
/// text and images are placed in units, sharp at any scale.
pub struct Canvas<'a> {
    /// Rows of premultiplied `0xAARRGGBB` (see `Color::over`), top row first.
    pixels: &'a mut [u32],
    width: i32,
    height: i32,
    /// The buffer's top left in the window, in pixels.
    origin: (i32, i32),
    /// Pixels per unit: 1.0 at 100%, 1.5 at 150%.
    scale: f32,
    /// The clip rect in window pixels: left, top, right, bottom. And the same in units, which
    /// every visible box checks before drawing.
    clip: [i32; 4],
    visible: Rect,
    /// Reused by every fill and stroke.
    cover: Vec<f32>,
    cells: Vec<u8>,
    /// Where drawing is recorded for the GPU instead (see `gpu_draw`).
    #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
    list: Option<&'a mut crate::gpu_draw::DrawList>,
}

impl std::fmt::Debug for Canvas<'_> {
    /// Its size, position and scale; not its pixels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas").field("width", &self.width).field("height", &self.height).field("origin", &self.origin).field("scale", &self.scale).finish_non_exhaustive()
    }
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`: `width` by `height`, at `origin` in the window.
    ///
    /// # Panics
    ///
    /// If `pixels` holds fewer than `width` times `height` pixels.
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize, origin: (i32, i32), scale: f32) -> Self {
        assert!(pixels.len() >= width * height, "the block is smaller than it says");
        let (width, height) = (width as i32, height as i32);
        let mut canvas = Canvas {
            pixels,
            width,
            height,
            origin,
            scale,
            clip: [0; 4],
            visible: Rect::EMPTY,
            cover: Vec::new(),
            cells: Vec::new(),
            #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
            list: None,
        };
        canvas.restore_clip([origin.0, origin.1, origin.0 + width, origin.1 + height]);
        canvas
    }
    /// A canvas recording into `list` what is drawn on it, for the GPU: `width` by `height`
    /// pixels at `origin` in the window. It has no pixels of its own.
    #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
    #[inline]
    pub(crate) fn recording(list: &'a mut crate::gpu_draw::DrawList, width: usize, height: usize, origin: (i32, i32), scale: f32) -> Self {
        let mut canvas = Canvas::new(&mut [], 0, 0, origin, scale);
        (canvas.width, canvas.height, canvas.list) = (width as i32, height as i32, Some(list));
        canvas.restore_clip([origin.0, origin.1, origin.0 + width as i32, origin.1 + height as i32]);
        canvas
    }
    /// What drawing is recorded into for the GPU, if it is.
    #[cfg(all(windows, feature = "gpu-draw"))]
    #[inline]
    pub(crate) fn list(&mut self) -> Option<&mut crate::gpu_draw::DrawList> {
        self.list.as_deref_mut()
    }
    /// Pixels per unit.
    pub fn scale(&self) -> f32 {
        self.scale
    }
    /// The buffer's position in the window, in pixels.
    pub fn origin(&self) -> (i32, i32) {
        self.origin
    }
    /// The clip rect in window pixels: left, top, right, bottom.
    pub fn clip_pixels(&self) -> [i32; 4] {
        self.clip
    }
    /// The window area this buffer covers, in units.
    pub fn area(&self) -> Rect {
        rect(self.origin.0 as f32 / self.scale, self.origin.1 as f32 / self.scale, self.width as f32 / self.scale, self.height as f32 / self.scale)
    }

    /// The clip rect in units: nothing outside it shows.
    pub fn visible(&self) -> Rect {
        self.visible
    }

    /// Clips drawing to `area`, within the current clip. Returns the previous clip, for
    /// [`restore_clip`](Self::restore_clip).
    pub fn clip(&mut self, area: Rect) -> [i32; 4] {
        let before = self.clip;
        let s = self.scale;
        let want = [whole(area.x * s), whole(area.y * s), whole(area.right() * s), whole(area.bottom() * s)];
        self.restore_clip([want[0].max(before[0]), want[1].max(before[1]), want[2].min(before[2]), want[3].min(before[3])]);
        before
    }
    /// Restores a clip `clip` returned.
    pub fn restore_clip(&mut self, clip: [i32; 4]) {
        let ([left, top, right, bottom], s) = (clip, self.scale);
        (self.clip, self.visible) = (clip, rect(left as f32 / s, top as f32 / s, (right - left) as f32 / s, (bottom - top) as f32 / s));
    }
    /// Runs `draw` clipped to `area`.
    pub fn with_clip(&mut self, area: Rect, draw: impl FnOnce(&mut Self)) {
        let before = self.clip(area);
        draw(self);
        self.restore_clip(before);
    }
    /// Renamed to [`with_clip`](Self::with_clip).
    #[deprecated(since = "0.2.1", note = "renamed to `with_clip`")]
    pub fn held_to(&mut self, area: Rect, draw: impl FnOnce(&mut Self)) {
        self.with_clip(area, draw);
    }

    /// Saves the pixels under `area` (units) within the clip, appended to `kept`: what a
    /// translucent box covers (see [`El::opacity`](crate::El::opacity)). Returns their window
    /// pixel rect, for `let_through`.
    pub(crate) fn keep_under(&mut self, area: Rect, kept: &mut Vec<u32>) -> [i32; 4] {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            list.failed = true;
            return [0; 4];
        }
        let (s, [cx0, cy0, cx1, cy1]) = (self.scale, self.clip);
        // One pixel more all round: an edge covering half a pixel is included.
        let part = [whole(area.x * s - 1.0).max(cx0), whole(area.y * s - 1.0).max(cy0), whole(area.right() * s + 1.0).min(cx1), whole(area.bottom() * s + 1.0).min(cy1)];
        let [left, top, right, bottom] = part;
        if left < right {
            (top..bottom).for_each(|y| kept.extend_from_slice(self.row(y, left, right)));
        }
        part
    }
    /// Blends the pixels `keep_under` last saved (the end of `kept`) back through what was
    /// drawn over them since, by `fade` (0 none, 255 fully), and removes them from `kept`.
    pub(crate) fn let_through(&mut self, [left, top, right, bottom]: [i32; 4], kept: &mut Vec<u32>, fade: u8) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if self.list.is_some() {
            return;
        }
        let wide = if left < right && top < bottom { (right - left) as usize } else { return };
        let from = kept.len().saturating_sub(wide * (bottom - top) as usize);
        for (y, under) in (top..bottom).zip(kept[from..].chunks_exact(wide)) {
            self.row(y, left, right).iter_mut().zip(under).for_each(|(drawn, under)| *drawn = between(*under, *drawn, 255 - fade as u32));
        }
        kept.truncate(from);
    }

    /// The pixels of row `y` from `x0` to `x1`. (Always inlined, like other per-pixel or
    /// per-row helpers: drawing speed must not depend on the optimizer's choices.)
    #[inline(always)]
    fn row(&mut self, y: i32, x0: i32, x1: i32) -> &mut [u32] {
        let at = ((y - self.origin.1) * self.width + (x0 - self.origin.0)) as usize;
        &mut self.pixels[at..at + (x1 - x0) as usize]
    }

    /// Fills the whole buffer.
    pub fn clear(&mut self, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.failed = true;
        }
        spread(self.pixels, color.over(0, 255));
    }
    /// Clears the clip rect to transparent.
    pub fn erase(&mut self) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.failed = true;
        }
        let [left, top, right, bottom] = self.clip;
        for y in top..bottom.max(top) {
            spread(self.row(y, left, right.max(left)), 0);
        }
    }

    /// A square-cornered rectangle. Its edges snap to whole pixels, so it is crisp. (Most of
    /// what is drawn is this, so it takes the short path: whole rows of pixels.)
    pub fn rect(&mut self, r: Rect, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.rect(r, color, self.scale, self.clip);
        }
        let (s, [cx0, cy0, cx1, cy1]) = (self.scale, self.clip);
        let (left, top) = (((r.x * s).round() as i32).max(cx0), ((r.y * s).round() as i32).max(cy0));
        let (right, bottom) = (((r.right() * s).round() as i32).min(cx1), ((r.bottom() * s).round() as i32).min(cy1));
        if color.is_none() || left >= right {
            return;
        }
        let (solid, fill) = (color.a() == 255, color.0 | 0xff00_0000);
        for y in top..bottom {
            match solid {
                true => spread(self.row(y, left, right), fill),
                false => self.row(y, left, right).iter_mut().for_each(|p| *p = color.over(*p, 255)),
            }
        }
    }

    /// A filled rectangle with rounded corners.
    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        match radius > 0.0 {
            true => self.ring(r, radius, f32::INFINITY, color),
            false => self.rect(r, color),
        }
    }

    /// A rectangle's outline, `width` thick, inside it.
    pub fn border(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        if width > 0.0 {
            self.ring(r, radius, width, color);
        }
    }

    /// A soft drop shadow around a rounded rectangle: darkest at its edge, gone at `reach`, and
    /// offset slightly down.
    pub fn shadow(&mut self, r: Rect, radius: f32, reach: f32, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.shadow(r, radius, reach, color, self.scale, self.clip);
        }
        if color.is_none() || r.is_empty() || reach <= 0.0 {
            return;
        }
        let s = self.scale;
        let (reach, drop) = (reach * s, reach * s / 4.0);
        let (x0, y0, x1, y1) = (r.x * s, r.y * s + drop, r.right() * s, r.bottom() * s + drop);
        let radius = (radius * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (left, top) = (((x0 - reach).floor() as i32).max(cx0), ((y0 - reach).floor() as i32).max(cy0));
        let (right, bottom) = (((x1 + reach).ceil() as i32).min(cx1), ((y1 + reach).ceil() as i32).min(cy1));
        if left >= right || top >= bottom {
            return;
        }
        for y in top..bottom {
            let py = y as f32 + 0.5;
            let dy = (y0 + radius - py).max(py - (y1 - radius)).max(0.0);
            for (i, p) in self.row(y, left, right).iter_mut().enumerate() {
                let px = (left + i as i32) as f32 + 0.5;
                let dx = (x0 + radius - px).max(px - (x1 - radius)).max(0.0);
                // This pixel's distance outside the rectangle. Under the box nothing shows
                // anyway; but the shadow hangs lower, and the strip showing below the box's
                // bottom edge is darkest.
                let away = (dx * dx + dy * dy).sqrt() - radius;
                let near = if away > -1.0 { (1.0 - away / reach).clamp(0.0, 1.0) } else { (py >= y1 - drop - radius - 1.0) as u8 as f32 };
                *p = color.over(*p, (near * near * 255.0) as u32);
            }
        }
    }

    /// A rounded rectangle, of which only the outer `thick` is drawn (all of it for infinity).
    fn ring(&mut self, r: Rect, radius: f32, thick: f32, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.ring(r, radius, thick, color, self.scale, self.clip);
        }
        if color.is_none() || r.is_empty() {
            return;
        }
        let s = self.scale;
        let (x0, y0, x1, y1) = ((r.x * s).round(), (r.y * s).round(), (r.right() * s).round(), (r.bottom() * s).round());
        let radius = (radius * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let thick = if thick.is_finite() { (thick * s).round().max(1.0) } else { f32::INFINITY };
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (left, top, right, bottom) = ((x0 as i32).max(cx0), (y0 as i32).max(cy0), (x1 as i32).min(cx1), (y1 as i32).min(cy1));
        if left >= right || top >= bottom {
            return;
        }
        let hollow = thick.is_finite();
        // A pixel's coverage by the shape; for an outline, minus the hole's.
        let cover_at = |x: i32, y: i32| -> f32 {
            let outer = inside(x, y, x0, y0, x1, y1, radius);
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Inside the hole's bounding box but outside its rounded corner counts as outline.
            if !hollow || outer <= 0.0 || px < x0 + thick || px > x1 - thick || py < y0 + thick || py > y1 - thick {
                return outer;
            }
            outer - inside(x, y, x0 + thick, y0 + thick, x1 - thick, y1 - thick, (radius - thick).max(0.0)).min(outer)
        };
        let (solid, fill) = (color.a() == 255, color.0 | 0xff00_0000);
        // Fully covered pixels are filled a run at a time.
        let lay = |stretch: &mut [u32]| match solid {
            true => spread(stretch, fill),
            false => stretch.iter_mut().for_each(|p| *p = color.over(*p, 255)),
        };
        // Only the four corners need per-pixel work. A row through them has a middle that is
        // all shape or all hole; a row between them is all shape, or for an outline its two
        // sides.
        let edge = if hollow { thick as i32 } else { 0 };
        // Clamped to 2^24 pixels, as are the corners: beyond that is far off any canvas, and no
        // sum overflows.
        let ends = (radius.ceil() as i32).max(edge).min(1 << 24);
        // The four corners are one corner mirrored: its per-pixel coverage is computed once,
        // when the corner is small enough to cache and any corner is drawn at all. (For a long
        // list row where only a button changed, none is.)
        let far = |at: f32| at.clamp(-16_777_216.0, 16_777_216.0) as i32;
        let (x0, y0, x1, y1) = (far(x0), far(y0), far(x1), far(y1));
        let (mut corner, kept) = ([0u8; 256], ends <= 16);
        let shows = (top < y0 + ends || bottom > y1 - ends) && (left < x0 + ends || right > x1 - ends);
        for at in 0..if kept && shows { ends * ends } else { 0 } {
            corner[at as usize] = (cover_at(x0 + at % ends, y0 + at / ends) * 255.0) as u8;
        }
        let covered = |x: i32, y: i32| match kept {
            true => corner[((y - y0).min(y1 - 1 - y) * ends + (x - x0).min(x1 - 1 - x)) as usize] as u32,
            false => (cover_at(x, y) * 255.0) as u32,
        };
        for y in top..bottom {
            let cornered = y < y0 + ends || y >= y1 - ends;
            let side = if cornered { ends } else { edge };
            let from = (x0 + side).clamp(left, right);
            let to = (x1 - side).clamp(from, right);
            let whole = cover_at((x0 + x1) / 2, y) >= 1.0;
            let (near, rest) = self.row(y, left, right).split_at_mut((from - left) as usize);
            let (middle, far) = rest.split_at_mut((to - from) as usize);
            match cornered {
                true => near.iter_mut().enumerate().for_each(|(i, p)| *p = color.over(*p, covered(left + i as i32, y))),
                false => lay(near),
            }
            match cornered {
                true => far.iter_mut().enumerate().for_each(|(i, p)| *p = color.over(*p, covered(to + i as i32, y))),
                false => lay(far),
            }
            if whole {
                lay(middle);
            }
        }
    }

    /// A rounded rectangle filled with a gradient. Each of `colors` has a position from 0 to 1,
    /// ascending. The gradient runs along a line through the centre at `turn` degrees from
    /// straight up (90 left to right, 180 top to bottom); or, with no `turn`, radially from the
    /// centre to the edges. The first and last colours are exactly the end pixels.
    pub fn gradient(&mut self, r: Rect, radius: f32, colors: &[(f32, Color)], turn: Option<f32>) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.gradient(r, radius, colors, turn, self.scale, self.clip);
        }
        let s = self.scale;
        let (x0, y0, x1, y1) = ((r.x * s).round(), (r.y * s).round(), (r.right() * s).round(), (r.bottom() * s).round());
        let radius = (radius * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (left, top, right, bottom) = ((x0 as i32).max(cx0), (y0 as i32).max(cy0), (x1 as i32).min(cx1), (y1 as i32).min(cy1));
        if left >= right || top >= bottom {
            return;
        }
        // From the box's centre to its outermost pixels' centres, x and y; the line's
        // direction; and the distance along it from the centre to the outermost pixel.
        let (mid_x, mid_y, half_x, half_y) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, ((x1 - x0 - 1.0) / 2.0).max(0.5), ((y1 - y0 - 1.0) / 2.0).max(0.5));
        let (sin, cos) = turn.map_or((0.0, 0.0), |turn| turn.to_radians().sin_cos());
        let reach = (sin.abs() * half_x + cos.abs() * half_y).max(0.5);
        for y in top..bottom {
            for (i, p) in self.row(y, left, right).iter_mut().enumerate() {
                let x = left + i as i32;
                let (dx, dy) = (x as f32 + 0.5 - mid_x, y as f32 + 0.5 - mid_y);
                let along = if turn.is_some() { 0.5 + (dx * sin - dy * cos) / (2.0 * reach) } else { (dx / half_x).hypot(dy / half_y) };
                *p = color_at(colors, along).over(*p, (inside(x, y, x0, y0, x1, y1, radius) * 255.0) as u32);
            }
        }
    }

    /// A filled path, scaled by `size` and placed at `at`.
    pub fn fill(&mut self, path: &Path, at: Point, size: f32, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            let (s, to) = (size * self.scale, pt(at.x * self.scale, at.y * self.scale));
            return list.fill_one(&mut |each| path.flatten(s, to, 1.0, each), color, self.scale, self.clip);
        }
        let (s, to) = (size * self.scale, pt(at.x * self.scale, at.y * self.scale));
        if !color.is_none() {
            self.fill_with(&mut |each| path.flatten(s, to, 1.0, each), false, |_, _| color);
        }
    }

    /// A path's stroke, `width` thick (in path units, at least a pixel), with round caps and
    /// joins. Each segment is a round-capped bar, and overlaps (at every join, and all along a
    /// curve) count once: a curve's stroke is as smooth as a straight one's.
    pub fn stroke(&mut self, path: &Path, at: Point, size: f32, width: f32, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.stroke(path, at, size, width, color, self.scale, self.clip);
        }
        let (fine, [cx0, cy0, cx1, cy1]) = (FINE as f32, self.clip);
        let (s, to) = (size * self.scale * fine, pt(at.x * self.scale * fine, at.y * self.scale * fine));
        let reach = (width * s / 2.0).max(0.5 * fine) + SOFT;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        path.flatten(s, to, fine, |a, b| {
            (x0, y0) = (x0.min(a.x).min(b.x), y0.min(a.y).min(b.y));
            (x1, y1) = (x1.max(a.x).max(b.x), y1.max(a.y).max(b.y));
        });
        // The pixels it reaches within the clip. (A subcell's coverage depends only on its
        // window position, not on where drawing is clipped.)
        let (left, top) = ((((x0 - reach) / fine).floor() as i32).max(cx0), (((y0 - reach) / fine).floor() as i32).max(cy0));
        let (right, bottom) = ((((x1 + reach) / fine).ceil() as i32).min(cx1), (((y1 + reach) / fine).ceil() as i32).min(cy1));
        if color.is_none() || left >= right || top >= bottom {
            return;
        }
        // Subcells are all clear between strokes: laying them down clears them.
        let (stride, mut cells) = ((right - left) as usize * FINE, std::mem::take(&mut self.cells));
        let used = stride * (bottom - top) as usize * FINE;
        cells.resize(cells.len().max(used), 0);
        path.flatten(s, to, fine, |a, b| thick(&mut cells[..used], stride, (left * FINE as i32, top * FINE as i32), a, b, reach));
        for (y, rows) in cells[..used].chunks_exact_mut(stride * FINE).enumerate() {
            lay(self.row(top + y as i32, left, right), rows, stride, color);
        }
        self.cells = cells;
    }

    /// Fills whatever `edges` encloses in one colour, by the nonzero rule: as `fill_with`, but
    /// drawn by the GPU itself where it draws (`gpu-draw`).
    #[inline(always)]
    pub(crate) fn fill_one(&mut self, edges: Edges, color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.fill_one(edges, color, self.scale, self.clip);
        }
        self.fill_with(edges, false, |_, _| color);
    }

    /// Fills whatever `edges` encloses. It is asked for its edges twice: for the bounds, then
    /// to draw. With `even_odd`, the even-odd rule; else nonzero. `ink` gives the colour of
    /// each covered pixel by its window position, so a shape can be filled with a gradient, as
    /// SVG drawings are.
    pub fn fill_with(&mut self, edges: Edges, even_odd: bool, mut ink: impl FnMut(i32, i32) -> Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.fill_with(edges, even_odd, ink, self.scale, self.clip);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        edges(&mut |a, b| {
            (x0, y0) = (x0.min(a.x).min(b.x), y0.min(a.y).min(b.y));
            (x1, y1) = (x1.max(a.x).max(b.x), y1.max(a.y).max(b.y));
        });
        // The shape's columns in this buffer, whatever is clipped: a pixel's coverage then
        // never depends on the clip. Of the rows, only those clipped in: each row is computed
        // alone.
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (left, right) = ((x0.floor() as i32).max(self.origin.0), (x1.ceil() as i32).saturating_add(1).min(self.origin.0 + self.width));
        let (top, bottom) = ((y0.floor() as i32).max(cy0), (y1.ceil() as i32).min(cy1));
        let (from, to) = (left.max(cx0), right.min(cx1));
        if from >= to || top >= bottom {
            return;
        }
        // Each edge adds its coverage of each pixel it crosses, and "all to the right is
        // inside" after it. A prefix sum along a row then gives each pixel's coverage. (A row
        // has two cells more than pixels, for what edges at its far right add past its end.)
        let (stride, h) = ((right - left) as usize + 2, (bottom - top) as usize);
        let mut cover = std::mem::take(&mut self.cover);
        cover.clear();
        cover.resize(stride * h, 0.0);
        let near = |p: Point| pt(p.x - left as f32, p.y - top as f32);
        edges(&mut |a, b| edge(&mut cover, stride, h, near(a), near(b)));
        for (y, cells) in cover.chunks_exact(stride).enumerate() {
            let skipped = (from - left) as usize;
            let mut sum: f32 = cells[..skipped].iter().sum();
            for ((p, cell), x) in self.row(top + y as i32, from, to).iter_mut().zip(&cells[skipped..]).zip(from..) {
                sum += cell;
                // Winding twice, four times… is as good as zero.
                let share = if even_odd { (sum - 2.0 * (sum / 2.0).round()).abs() } else { sum.abs().min(1.0) };
                // The colour is asked for only where the shape is: it may be costly.
                let color = if share > 0.0 { ink(x, top + y as i32) } else { Color::NONE };
                *p = color.over(*p, (share * 255.0 + 0.5) as u32);
            }
        }
        self.cover = cover;
    }

    /// An image stretched to `r`: rows of `0xAARRGGBB` (not premultiplied), `width` per row.
    /// Enlarged, bilinear; reduced, each pixel the mean of the ones it covers.
    pub fn picture(&mut self, pixels: &[u32], width: usize, r: Rect) {
        self.rounded_picture(pixels, width, r, 0.0);
    }
    /// The same, clipped to rounded corners of this radius (an avatar, an image in a card).
    pub fn rounded_picture(&mut self, pixels: &[u32], width: usize, r: Rect, radius: f32) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.picture(pixels, width, r, radius, self.scale, self.clip);
        }
        let (s, height) = (self.scale, pixels.len() / width.max(1));
        let (x0, y0, x1, y1) = (whole(r.x * s), whole(r.y * s), whole(r.right() * s), whole(r.bottom() * s));
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (from, to) = (x0.max(cx0), x1.min(cx1));
        if from >= to || y1 <= y0 || height == 0 {
            return;
        }
        let (step_x, step_y) = (width as f32 / (x1 - x0) as f32, height as f32 / (y1 - y0) as f32);
        if step_x > 1.0 || step_y > 1.0 {
            return self.shrunk(pixels, width, [x0, y0, x1, y1], radius * s);
        }
        // Blends two colours `t`/256 of the way: red and blue in one step, green and alpha in
        // another.
        let covered = corners([x0, y0, x1, y1], radius * s);
        let mix = |a: u32, b: u32, t: u32| ((((a & 0xff00ff) * (256 - t) + (b & 0xff00ff) * t) >> 8) & 0xff00ff) | (((a >> 8 & 0xff00ff) * (256 - t) + (b >> 8 & 0xff00ff) * t) & 0xff00_ff00);
        for y in y0.max(cy0)..y1.min(cy1) {
            // Where this pixel's centre falls in the image: between which two rows, and how
            // far.
            let v = (((y - y0) as f32 + 0.5) * step_y - 0.5).clamp(0.0, (height - 1) as f32);
            let (above, below, down) = (v as usize * width, (v as usize + 1).min(height - 1) * width, (v.fract() * 256.0) as u32);
            for (i, p) in self.row(y, from, to).iter_mut().enumerate() {
                let u = (((from - x0) as f32 + i as f32 + 0.5) * step_x - 0.5).clamp(0.0, (width - 1) as f32);
                let (left, right, along) = (u as usize, (u as usize + 1).min(width - 1), (u.fract() * 256.0) as u32);
                let color = mix(mix(pixels[above + left], pixels[above + right], along), mix(pixels[below + left], pixels[below + right], along), down);
                *p = Color(color).over(*p, covered(from + i as i32, y));
            }
        }
    }

    /// An image drawn reduced over pixels `x0, y0` to `x1, y1`: each shows the mean of the
    /// image area on it.
    fn shrunk(&mut self, pixels: &[u32], width: usize, [x0, y0, x1, y1]: [i32; 4], radius: f32) {
        let height = pixels.len() / width;
        let (step_x, step_y) = (width as f32 / (x1 - x0) as f32, height as f32 / (y1 - y0) as f32);
        // The image span on pixel `i` along one axis, the image being `size` long: `step`
        // pixels of it. (Or one, where the image isn't reduced that way: interpolated between
        // neighbours.)
        let on = |i: i32, step: f32, size: usize| {
            let from = (i as f32 + 0.5) * step - step.max(1.0) / 2.0;
            [from.max(0.0), (from + step.max(1.0)).min(size as f32)]
        };
        let [cx0, cy0, cx1, cy1] = self.clip;
        let (first, covered) = (x0.max(cx0), corners([x0, y0, x1, y1], radius));
        for y in y0.max(cy0)..y1.min(cy1) {
            let [top, bottom] = on(y - y0, step_y, height);
            for (i, p) in self.row(y, first, x1.min(cx1)).iter_mut().enumerate() {
                let [left, right] = on(first - x0 + i as i32, step_x, width);
                *p = Color(mean(pixels, width, [left, top, right, bottom])).over(*p, covered(first + i as i32, y));
            }
        }
    }

    /// Draws a buffer of canvas pixels (premultiplied `0xAARRGGBB`), `width` per row, top left
    /// at `at` in window pixels: blended over what is there, or, if `solid` (no transparency),
    /// copied.
    pub fn pixels(&mut self, pixels: &[u32], width: usize, [x, y]: [i32; 2], solid: bool) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.pixels(pixels, width, [x, y], self.clip);
        }
        let ([cx0, cy0, cx1, cy1], high) = (self.clip, (pixels.len() / width.max(1)) as i32);
        let (from, to) = (x.max(cx0), x.saturating_add(width as i32).min(cx1));
        for row in y.max(cy0)..y.saturating_add(high).min(cy1) {
            if from >= to {
                return;
            }
            let start = (row - y) as usize * width;
            let (under, block) = (self.row(row, from, to), &pixels[start + (from - x) as usize..start + (to - x) as usize]);
            match solid {
                true => under.copy_from_slice(block),
                false => under.iter_mut().zip(block).for_each(|(under, over)| *under = laid(*over, *under)),
            }
        }
    }

    /// Draws a buffer something was drawn on in white (canvas pixels), `width` per row, top
    /// left at `at` in window pixels: each canvas pixel covered by the buffer's coverage, in
    /// the colour `color` gives for its window position. So overlapping strokes are drawn into
    /// such a buffer one by one and laid down at once, with a gradient.
    pub fn fill_mask(&mut self, block: &[u32], width: usize, [x, y]: [i32; 2], mut color: impl FnMut(i32, i32) -> Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            let area = [x, y, x.saturating_add(width as i32), y.saturating_add((block.len() / width.max(1)) as i32)];
            return list.patch(area, self.clip, self.scale, |canvas| canvas.fill_mask(block, width, [x, y], color));
        }
        let ([cx0, cy0, cx1, cy1], high) = (self.clip, (block.len() / width.max(1)) as i32);
        let (from, to) = (x.max(cx0), x.saturating_add(width as i32).min(cx1));
        for row in y.max(cy0)..y.saturating_add(high).min(cy1) {
            if from >= to {
                return;
            }
            let start = (row - y) as usize * width + (from - x) as usize;
            let covered = self.row(row, from, to).iter_mut().zip(&block[start..]).zip(from..).filter(|((_, cover), _)| **cover >> 24 != 0);
            covered.for_each(|((pixel, cover), x)| *pixel = color(x, row).over(*pixel, cover >> 24));
        }
    }
    /// Renamed to [`fill_mask`](Self::fill_mask).
    #[deprecated(since = "0.2.1", note = "renamed to `fill_mask`")]
    pub fn ink(&mut self, block: &[u32], width: usize, at: [i32; 2], color: impl FnMut(i32, i32) -> Color) {
        self.fill_mask(block, width, at, color);
    }

    /// Draws a coverage mask, one byte per pixel (0 none, 255 full): a glyph, as the text
    /// system draws it. `x` and `y` are window pixels.
    pub fn cover(&mut self, x: i32, y: i32, width: usize, cover: &[u8], color: Color) {
        #[cfg(all(feature = "gpu-draw", not(any(target_os = "ios", target_os = "android"))))]
        if let Some(list) = self.list.as_deref_mut() {
            return list.mask(x, y, width, cover, color, self.clip);
        }
        if width == 0 || color.is_none() {
            return;
        }
        let [cx0, cy0, cx1, cy1] = self.clip;
        for (j, line) in cover.chunks_exact(width).enumerate() {
            // A position too far to be a pixel count is simply out of view.
            let py = y.saturating_add(j as i32);
            if py < cy0 || py >= cy1 {
                continue;
            }
            let (from, to) = (x.max(cx0), x.saturating_add(width as i32).min(cx1));
            if from >= to {
                return;
            }
            let dest = self.row(py, from, to);
            let covered = dest.iter_mut().zip(&line[(from - x) as usize..]).filter(|(_, cover)| **cover > 0);
            covered.for_each(|(p, cover)| *p = color.over(*p, *cover as u32));
        }
    }
}

#[inline]
/// Interpolates from pixel `from` to pixel `to` by `along` (0 none, 255 all): each channel in
/// its own byte, rounded as `laid` does.
fn between(from: u32, to: u32, along: u32) -> u32 {
    // Each channel pair weighted and summed, then divided by 255 as `laid` does.
    let part = |from: u32, to: u32| {
        let sum = from * (255 - along) + to * along + 0x0080_0080;
        (sum + (sum >> 8 & 0x00ff_00ff)) >> 8 & 0x00ff_00ff
    };
    part(from & 0x00ff_00ff, to & 0x00ff_00ff) | part(from >> 8 & 0x00ff_00ff, to >> 8 & 0x00ff_00ff) << 8
}

/// Composites premultiplied pixel `over` onto `under`.
#[inline]
fn laid(over: u32, under: u32) -> u32 {
    let clear = 255 - (over >> 24);
    if clear == 0 || clear == 255 {
        return if clear == 0 { over } else { under };
    }
    // Two channels of the lower pixel at a time, times the upper's transparency, over 255: each
    // stays in its own byte.
    let part = |pair: u32| (pair * clear + 0x0080_0080 + ((pair * clear + 0x0080_0080) >> 8 & 0x00ff_00ff)) >> 8 & 0x00ff_00ff;
    over + (part(under & 0x00ff_00ff) | part(under >> 8 & 0x00ff_00ff) << 8)
}

/// Per-pixel coverage of a path, one byte per pixel (0 none, 255 full), in a `width` by
/// `height` block with top left at `origin`: how glyphs and icons are cached, to draw in any
/// colour with [`Canvas::cover`]. Path units are scaled by `size`. With `stroke` above 0, the
/// path's stroke, that wide in path units, round-capped; else the filled path.
pub fn coverage(path: &Path, size: f32, stroke: f32, origin: (i32, i32), width: usize, height: usize) -> Vec<u8> {
    if stroke > 0.0 {
        // As in `Canvas::stroke`.
        let (fine, stride, mut cells) = (FINE as f32, width * FINE, vec![0u8; width * FINE * height * FINE]);
        let reach = (stroke * size / 2.0).max(0.5) * fine + SOFT;
        path.flatten(size * fine, pt(0.0, 0.0), fine, |a, b| thick(&mut cells, stride, (origin.0 * FINE as i32, origin.1 * FINE as i32), a, b, reach));
        return cells.chunks_exact((stride * FINE).max(1)).flat_map(|rows| (0..width).map(move |x| covered(rows, stride, x) as u8)).collect();
    }
    let (stride, mut cells) = (width + 2, vec![0.0f32; (width + 2) * height]);
    let mut add = |a: Point, b: Point| edge(&mut cells, stride, height, pt(a.x - origin.0 as f32, a.y - origin.1 as f32), pt(b.x - origin.0 as f32, b.y - origin.1 as f32));
    path.flatten(size, pt(0.0, 0.0), 1.0, &mut add);
    // As in `fill_with`: a prefix sum along a row gives each pixel's coverage.
    let mut covers = Vec::with_capacity(width * height);
    for row in cells.chunks_exact(stride) {
        let mut sum = 0.0;
        covers.extend(row[..width].iter().map(|cell| {
            sum += cell;
            (sum.abs().min(1.0) * 255.0) as u8
        }));
    }
    covers
}

/// Sets every pixel of a run to `pixel`. Only the first few are written one by one; the rest
/// copied from what is there, doubling each time. (Copies are done many pixels at a time by the
/// system, so a large fill is as fast in a size-optimized build as in a speed-optimized one.)
#[inline(always)]
fn spread(stretch: &mut [u32], pixel: u32) {
    let mut done = stretch.len().min(16);
    stretch[..done].fill(pixel);
    while done < stretch.len() {
        let more = done.min(stretch.len() - done);
        stretch.copy_within(..more, done);
        done += more;
    }
}

/// A pixel position, rounded: clamped to ±16 million (0 for NaN), so any two are an `i32`
/// apart. (A page's rects may be any size; a screen's never come near.)
fn whole(at: f32) -> i32 {
    at.round().clamp(-16_777_216.0, 16_777_216.0) as i32
}

/// Per-pixel coverage of an image spanning `x0, y0` to `x1, y1` (pixels) with corners of this
/// radius: full everywhere except near a rounded corner.
fn corners([x0, y0, x1, y1]: [i32; 4], radius: f32) -> impl Fn(i32, i32) -> u32 {
    let radius = radius.min((x1 - x0) as f32 / 2.0).min((y1 - y0) as f32 / 2.0).max(0.0);
    let (left, top, right, bottom) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
    move |x, y| if radius > 0.0 { (inside(x, y, left, top, right, bottom, radius) * 255.0) as u32 } else { 255 }
}

/// How far inside a rounded rectangle a pixel's centre is: 1 well inside, 0 outside, between
/// along a corner's curve.
#[inline(always)]
fn inside(px: i32, py: i32, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) -> f32 {
    let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
    let (dx, dy) = ((x0 + radius - x).max(x - (x1 - radius)), (y0 + radius - y).max(y - (y1 - radius)));
    // Only in a corner does the distance to its circle matter.
    if dx > 0.0 && dy > 0.0 { (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0) } else { 1.0 }
}

/// The colour `along` a gradient of stops `colors` (ascending): the two it lies between,
/// blended; before the first or past the last, that one.
pub(crate) fn color_at(colors: &[(f32, Color)], along: f32) -> Color {
    let after = colors.partition_point(|color| color.0 <= along);
    match (after.checked_sub(1).map(|before| colors[before]), colors.get(after)) {
        (Some(before), Some(after)) => before.1.lerp(after.1, (along - before.0) / (after.0 - before.0)),
        (Some(only), None) | (None, Some(&only)) => only.1,
        (None, None) => Color::NONE,
    }
}

/// The mean colour of an image area from `left, top` to `right, bottom`, in (fractional)
/// pixels. (Each pixel counts by its alpha.)
fn mean(pixels: &[u32], width: usize, [left, top, right, bottom]: [f32; 4]) -> u32 {
    let mut sum = [0.0f32; 4];
    for y in top as usize..bottom.ceil() as usize {
        let high = bottom.min(y as f32 + 1.0) - top.max(y as f32);
        for x in left as usize..right.ceil() as usize {
            let (pixel, part) = (pixels[y * width + x], high * (right.min(x as f32 + 1.0) - left.max(x as f32)));
            let solid = (pixel >> 24) as f32 * part;
            sum = [sum[0] + solid, sum[1] + (pixel >> 16 & 255) as f32 * solid, sum[2] + (pixel >> 8 & 255) as f32 * solid, sum[3] + (pixel & 255) as f32 * solid];
        }
    }
    let (area, solid) = ((right - left) * (bottom - top), sum[0].max(1e-6));
    let [how_solid, red, green, blue] = [sum[0] / area, sum[1] / solid, sum[2] / solid, sum[3] / solid].map(|part| (part + 0.5).min(255.0) as u32);
    how_solid << 24 | red << 16 | green << 8 | blue
}

/// An image halved each way, and its width: each pixel the mean of the four it covers (two,
/// along an odd edge).
pub(crate) fn halve(pixels: &[u32], width: usize) -> (usize, Vec<u32>) {
    let (height, wide) = (pixels.len() / width.max(1), width.div_ceil(2));
    let at = |x: usize, y: usize| pixels[y.min(height - 1) * width + x.min(width - 1)];
    let four = |x: usize, y: usize| [at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)];
    // The mean of four pixels in integers; each colour counts by its alpha.
    let mean = |four: [u32; 4]| {
        let solid: u32 = four.iter().map(|pixel| pixel >> 24).sum();
        let part = |shift: u32| (four.iter().map(|pixel| (pixel >> shift & 255) * (pixel >> 24)).sum::<u32>() + solid / 2) / solid.max(1);
        ((solid + 2) / 4) << 24 | part(16) << 16 | part(8) << 8 | part(0)
    };
    (wide, (0..wide * height.div_ceil(2)).map(|i| mean(four(i % wide * 2, i / wide * 2))).collect())
}

/// Lays a round-capped segment onto the subcells near it, `stride` per row, the first being
/// subcell `origin`. A subcell is covered (255 fully) by how much lies inside the line's edge,
/// at `reach` minus `SOFT` from its middle: as if the edge passed straight by the subcell at
/// the segment's slant (see `Square`). Each subcell keeps the maximum any segment gives it, so
/// overlaps count once. (At the caps the edge is also taken at the segment's slant: where
/// segments join the next one's side covers that, and a line's end is off by a few hundredths
/// of a subcell at most.)
fn thick(cells: &mut [u8], stride: usize, origin: (i32, i32), a: Point, b: Point, reach: f32) {
    let ((dx, dy), rows) = ((b.x - a.x, b.y - a.y), (cells.len() / stride.max(1)) as i32);
    // Reciprocals taken once, for all subcells.
    let (per_long, per_down) = (1.0 / (dx * dx + dy * dy).max(1e-12), if dy == 0.0 { 0.0 } else { 1.0 / dy });
    // No subcell further from the middle than half the line plus half the square, at its slant,
    // is covered: the cells around are not visited.
    let square = Square::of(dx, dy, reach - SOFT);
    let reach = square.deep;
    let first = ((a.y.min(b.y) - reach).floor() as i32).saturating_sub(origin.1).max(0);
    let last = ((a.y.max(b.y) + reach).ceil() as i32).saturating_sub(origin.1).min(rows);
    for y in first..last {
        // The part of the segment this row's subcells can be near, and from that the row's
        // subcells that can be.
        let down = (y + origin.1) as f32 + 0.5 - a.y;
        let (u, v) = if dy == 0.0 { (0.0, 1.0) } else { ((down - reach) * per_down, (down + reach) * per_down) };
        let (p, q) = (a.x + u.clamp(0.0, 1.0) * dx, a.x + v.clamp(0.0, 1.0) * dx);
        let from = ((p.min(q) - reach).floor() as i32).saturating_sub(origin.0).clamp(0, stride as i32);
        let to = ((p.max(q) + reach).ceil() as i32).saturating_sub(origin.0).min(stride as i32).max(from);
        let row = &mut cells[y as usize * stride..][from as usize..to as usize];
        for (cell, x) in row.iter_mut().zip(from..) {
            // The distance from the subcell's centre to the nearest point of the segment.
            let across = (x + origin.0) as f32 + 0.5 - a.x;
            let along = ((across * dx + down * dy) * per_long).clamp(0.0, 1.0);
            let away = ((across - along * dx).powi(2) + (down - along * dy).powi(2)).sqrt();
            *cell = (*cell).max(square.cover(away));
        }
    }
}

/// The exact area of a subcell inside the edge of a line passing straight by it, at any slant.
/// Seen along the edge's normal, a square spans `a + b` (the smaller and larger of the normal's
/// components, for a unit-length normal). Its first `a` is a widening corner, then `b - a` at
/// full width, then the other corner: the area inside the edge is a sum of four such corners,
/// each as large as the edge reaches past its start. What depends only on the slant is computed
/// once per segment.
struct Square {
    a: f32,
    b: f32,
    /// How deep into the square the edge lies, from its outermost corner, for a subcell centred
    /// on the line: half the line's width plus half the square.
    deep: f32,
    /// 1 / (2ab): the area scale of the corners.
    per: f32,
}

impl Square {
    /// For a line `half` wide each side, going `dx` across and `dy` down.
    fn of(dx: f32, dy: f32, half: f32) -> Square {
        let long = dx.hypot(dy);
        let (p, q) = if long > 0.0 { ((dy / long).abs(), (dx / long).abs()) } else { (0.0, 1.0) };
        // An exactly horizontal or vertical line is taken as slanted a hair, which changes no
        // subcell by a tenth of a 255th and avoids dividing by zero: a pixel-aligned line
        // covers its pixels fully, and nothing of the next.
        let (a, b) = (p.min(q).max(1.0 / 1024.0), p.max(q).max(std::f32::consts::FRAC_1_SQRT_2));
        Square { a, b, deep: half + (a + b) / 2.0, per: 1.0 / (2.0 * a * b) }
    }
    /// The coverage (255 full) of a subcell whose centre is `away` from the line's middle.
    /// (Branch-free between the corners, so the CPU computes many subcells at once.)
    #[inline(always)]
    fn cover(&self, away: f32) -> u8 {
        let (Square { a, b, per, .. }, deep) = (self, self.deep - away);
        let corner = |past: f32| past.max(0.0) * past.max(0.0);
        let cover = (corner(deep) - corner(deep - *a) - corner(deep - *b) + corner(deep - *a - *b)) * *per;
        (cover.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    }
}

/// Draws one pixel row's subcells onto it in a colour, and clears the subcells. Where eight
/// subcells in a row are clear in every subcell row (nearly everywhere under a thin line in a
/// wide area), their pixels are skipped at once.
fn lay(pixels: &mut [u32], rows: &mut [u8], stride: usize, color: Color) {
    for (group, x) in pixels.chunks_mut(GROUP).zip((0..).step_by(GROUP)) {
        // Fewer than eight subcells, at a row's end, are never skipped.
        let eight = |row: &[u8]| row[x * FINE..].first_chunk::<8>().map_or(1, |cells| u64::from_ne_bytes(*cells));
        if rows.chunks_exact(stride).all(|row| eight(row) == 0) {
            continue;
        }
        let covers = group.iter_mut().enumerate().map(|(i, p)| (p, covered(rows, stride, x + i)));
        covers.filter(|(_, cover)| *cover > 0).for_each(|(p, cover)| *p = color.over(*p, cover));
        rows.chunks_exact_mut(stride).for_each(|row| row[x * FINE..(x * FINE + 8).min(stride)].fill(0));
    }
}

/// The stroke coverage of pixel `x`, 255 full: the mean of its subcells. `cells` holds one
/// pixel row's subcell rows, `stride` per row.
#[inline(always)]
fn covered(cells: &[u8], stride: usize, x: usize) -> u32 {
    cells.chunks_exact(stride).map(|row| row[x * FINE..(x + 1) * FINE].iter().map(|cell| *cell as u32).sum::<u32>()).sum::<u32>() / (FINE * FINE) as u32
}

/// Adds one edge of a shape to the coverage of the pixels it crosses, in rows of `stride`
/// cells. (After font-rs, by Raph Levien: each pixel gets the area of it right of the edge.)
fn edge(cover: &mut [f32], stride: usize, h: usize, a: Point, b: Point) {
    if a.y == b.y {
        return;
    }
    // Walk it downward; an upward edge counts negatively.
    let (dir, a, b) = if a.y < b.y { (1.0, a, b) } else { (-1.0, b, a) };
    let (slope, wide) = ((b.x - a.x) / (b.y - a.y), (stride - 2) as f32);
    for y in (a.y.max(0.0) as usize)..h.min(b.y.ceil().max(0.0) as usize) {
        // The edge's span in this row, from the edge's own ends, not the row above: so a row
        // comes out the same in any buffer.
        let (from, to) = ((y as f32).max(a.y), ((y + 1) as f32).min(b.y));
        let d = (to - from) * dir;
        // Left of the buffer counts as its first pixel, right of it as just past its last.
        let (p, q) = ((a.x + (from - a.y) * slope).clamp(0.0, wide), (a.x + (to - a.y) * slope).clamp(0.0, wide));
        let (x0, x1) = if p < q { (p, q) } else { (q, p) };
        let (first, last) = (x0.floor() as usize, x1.ceil() as usize);
        // The span's start and end, from the first pixel it touches.
        let (row, start, end) = (y * stride, x0 - first as f32, x1 - first as f32);
        if last <= first + 1 {
            // It stays within one pixel in this row.
            let mid = 0.5 * (start + end);
            cover[row + first] += d - d * mid;
            cover[row + first + 1] += d * mid;
            continue;
        }
        // It crosses several: each gets its part of the slanted edge.
        let run = 1.0 / (end - start);
        let head = 0.5 * run * (1.0 - start) * (1.0 - start);
        let into_last = end - (last - first) as f32 + 1.0;
        let tail = 0.5 * run * into_last * into_last;
        cover[row + first] += d * head;
        if last == first + 2 {
            cover[row + first + 1] += d * (1.0 - head - tail);
        } else {
            let second = run * (1.5 - start);
            cover[row + first + 1] += d * (second - head);
            for i in first + 2..last - 1 {
                cover[row + i] += d * run;
            }
            let before = second + (last - first - 3) as f32 * run;
            cover[row + last - 1] += d * (1.0 - before - tail);
        }
        cover[row + last] += d * tail;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(w: usize, h: usize) -> Vec<u32> {
        vec![0; w * h]
    }

    #[test]
    fn a_rectangle_fills_exactly_its_pixels() {
        let mut px = block(8, 8);
        Canvas::new(&mut px, 8, 8, (0, 0), 1.0).rect(rect(2.0, 3.0, 4.0, 2.0), Color::WHITE);
        let lit: Vec<usize> = px.iter().enumerate().filter(|(_, p)| **p != 0).map(|(i, _)| i).collect();
        assert_eq!(lit, [26, 27, 28, 29, 34, 35, 36, 37]);
    }

    #[test]
    fn a_picture_drawn_smaller_shows_the_mean_of_what_falls_on_each_pixel() {
        // Eight by eight, black and white checks. Reduced to four, two or one pixels, every
        // pixel is half black, half white: grey; to three, a little more of one or the other.
        let checks: Vec<u32> = (0..64).map(|i| if (i % 8 + i / 8) % 2 == 0 { 0xff00_0000 } else { 0xffff_ffff }).collect();
        for (side, greys) in [(4usize, 0x7f..=0x80), (2, 0x7f..=0x80), (1, 0x7f..=0x80), (3, 0x78..=0x87)] {
            let mut px = block(side, side);
            Canvas::new(&mut px, side, side, (0, 0), 1.0).picture(&checks, 8, rect(0.0, 0.0, side as f32, side as f32));
            assert!(px.iter().all(|p| greys.contains(&(p & 255)) && p >> 24 == 255), "drawn {side} wide: {px:x?}");
        }
        // Halved, an image is the mean of each four pixels; transparent pixels give no colour,
        // only transparency. (Three wide: red, clear, blue. Two wide: red half clear, and blue
        // alone.)
        let (red, clear, blue) = (0xffff_0000, 0x0000_ff00, 0xff00_00ff);
        assert_eq!(halve(&[red, clear, blue, red, clear, blue], 3), (2, vec![0x80ff_0000, blue]));
    }

    #[test]
    fn a_shadow_is_dark_right_from_the_lower_edge_of_its_box() {
        // A box with rounded corners from 10 to 30 over a white page: its shadow reaches 8 and
        // is 2 lower. Just under the box the page is darkest, with no pale seam between box and
        // shadow; further down it fades, and beyond its reach it is white.
        let mut px = vec![0xffff_ffffu32; 48 * 48];
        Canvas::new(&mut px, 48, 48, (0, 0), 1.0).shadow(rect(10.0, 10.0, 20.0, 20.0), 6.0, 8.0, Color::BLACK.with_alpha(0.4));
        let grey = |y: usize| px[y * 48 + 20] & 255;
        assert!(grey(30) == grey(31) && grey(31) < grey(34) && grey(34) < grey(38), "{:?}", [30, 31, 32, 34, 38, 44].map(grey));
        assert_eq!((grey(44), grey(20)), (255, 255), "white past its reach, and under the box (which covers that)");
    }

    #[test]
    fn nothing_is_drawn_outside_the_clip_or_the_block() {
        let mut px = block(8, 8);
        let mut c = Canvas::new(&mut px, 8, 8, (0, 0), 1.0);
        c.clip(rect(4.0, 0.0, 100.0, 100.0));
        c.rounded(rect(-10.0, -10.0, 100.0, 100.0), 3.0, Color::WHITE);
        c.stroke(&Path::svg("M-5 4h30"), pt(0.0, 0.0), 1.0, 2.0, Color::WHITE);
        assert!(px.chunks(8).all(|row| row[..4].iter().all(|p| *p == 0) && row[4..].iter().all(|p| *p != 0)));
    }

    /// The names 0.2.1 replaced still work, and draw what the new ones do.
    #[test]
    #[allow(deprecated)]
    fn the_old_names_draw_as_the_new_ones() {
        let mask: Vec<u32> = (0..16).map(|at| (at * 16) << 24 | 0x00ff_ffff).collect();
        let draw = |old: bool| {
            let mut px = block(8, 8);
            let mut c = Canvas::new(&mut px, 8, 8, (0, 0), 1.0);
            let fill = |c: &mut Canvas| c.rounded(rect(0.0, 0.0, 8.0, 8.0), 0.0, Color::WHITE);
            let color = |x: i32, y: i32| Color::rgb(0x10_2030 + (x * 8 + y) as u32);
            match old {
                true => (c.held_to(rect(2.0, 2.0, 3.0, 3.0), fill), c.ink(&mask, 4, [5, 4], color)),
                false => (c.with_clip(rect(2.0, 2.0, 3.0, 3.0), fill), c.fill_mask(&mask, 4, [5, 4], color)),
            };
            px
        };
        let new = draw(false);
        assert_eq!(draw(true), new);
        assert_eq!(new.iter().filter(|p| **p == Color::WHITE.0).count(), 9, "the clip held the fill to 3 by 3 pixels");
        assert!(new[5 * 8 + 6] != 0, "the mask laid down its colours");
    }

    #[test]
    fn a_strip_of_a_window_holds_the_same_pixels_as_the_whole() {
        let draw = |c: &mut Canvas| {
            c.rounded(rect(1.0, 1.0, 14.0, 14.0), 5.0, Color::rgb(0x8b93ff));
            c.border(rect(1.0, 1.0, 14.0, 14.0), 5.0, 1.0, Color::WHITE);
            c.fill(&Path::svg("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z"), pt(0.0, 0.0), 1.0, Color::BLACK);
            c.stroke(&Path::svg("M2.5 7a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0M10.5 10.5 14 14"), pt(0.3, 0.6), 1.0, 1.5, Color::WHITE);
        };
        let mut whole = block(16, 16);
        draw(&mut Canvas::new(&mut whole, 16, 16, (0, 0), 1.0));
        let mut strip = block(16, 4);
        draw(&mut Canvas::new(&mut strip, 16, 4, (0, 8), 1.0));
        assert_eq!(strip[..], whole[8 * 16..12 * 16]);
    }

    /// A path's stroke coverage per pixel, computed the slow way: counting which of 16 by 16
    /// sample points per pixel lie within half the line width of the path.
    fn counted(d: &str, px: usize, stroke: f32) -> Vec<f32> {
        let (scale, mut pieces) = (px as f32 / 16.0, Vec::new());
        Path::svg(d).flatten(scale * 4.0, pt(0.0, 0.0), 4.0, |a, b| pieces.push((pt(a.x / 4.0, a.y / 4.0), pt(b.x / 4.0, b.y / 4.0))));
        let half = (stroke * scale / 2.0).max(0.5);
        let near = |x: f32, y: f32| {
            pieces.iter().any(|(a, b)| {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let t = (((x - a.x) * dx + (y - a.y) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
                (x - a.x - t * dx).powi(2) + (y - a.y - t * dy).powi(2) <= half * half
            })
        };
        let inside = |i: usize, sub: usize| near((i % px) as f32 + (sub % 16) as f32 / 16.0 + 1.0 / 32.0, (i / px) as f32 + (sub / 16) as f32 / 16.0 + 1.0 / 32.0);
        (0..px * px).map(|i| (0..256).filter(|sub| inside(i, *sub)).count() as f32 / 256.0).collect()
    }

    #[test]
    fn an_outline_covers_each_pixel_by_as_much_as_lies_on_it() {
        // Icon-like line drawings at icon sizes: a folder with rounded corners, a star with
        // sharp ones, a ring with a handle, a tick, and three dots that are rings too small to
        // have a hole. No pixel is off by more than a twelfth, and the total by less than a
        // twentieth of the drawing. (Curve segments overlap all along a curve, and at corners:
        // counted once per overlap, a ring had no smooth edge at all, and half its pixels off
        // by more than half.)
        let drawings = [
            "M1.8 4.3a1 1 0 0 1 1-1h3l1.4 1.5h6a1 1 0 0 1 1 1v6.4a1 1 0 0 1-1 1H2.8a1 1 0 0 1-1-1z",
            "M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z",
            "M2.5 7a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0M10.5 10.5 14 14",
            "M3 8.5 6.5 12 13 4.5",
            "M2.7 8a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0-1.6 0M7.2 8a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0-1.6 0M11.7 8a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0-1.6 0",
        ];
        // Coverage error against counting: the worst pixel, and the total as a fraction of the
        // drawing.
        let off = |d: &str, px: usize| {
            let (have, want) = (coverage(&Path::svg(d), px as f32 / 16.0, 1.5, (0, 0), px, px), counted(d, px, 1.5));
            let off = have.iter().zip(&want).map(|(have, want)| (*have as f32 / 255.0 - want).abs());
            let (worst, all) = off.fold((0.0f32, 0.0), |(worst, all), off| (worst.max(off), all + off));
            (worst, all / want.iter().sum::<f32>())
        };
        for (d, px) in drawings.iter().flat_map(|d| [12usize, 13, 16, 18, 24].map(|px| (*d, px))) {
            let (worst, all) = off(d, px);
            assert!(worst < 0.085 && all < 0.05, "{px} pixels: the worst pixel is off by {worst:.3}, all by {all:.3} of the drawing, of {d}");
        }
        // Lines closer than a pixel stay apart, and lines touching all along their edges (the
        // hardest case) leave a seam no darker than a quarter pixel.
        for px in [12, 13, 16, 18, 24] {
            let (worst, all) = off("M3 4h10M3 6h10M3 9.5h10M3 11h10", px);
            assert!(worst < 0.26 && all < 0.07, "{px} pixels: the worst pixel is off by {worst:.3}, all by {all:.3} of the drawing");
        }
    }

    #[test]
    fn a_circle_is_round_at_any_size() {
        // Filled, and stroked 1 to 8 pixels wide, a circle covers each edge pixel by its area
        // inside the true circle (counted at 32 by 32 points per pixel), from dot size to
        // screen size: no pixel is off by more than 1/25, and edge pixels by 1/170 on average.
        // (Cut into a few segments each, as small curves were, a 2-pixel circle was off by an
        // eighth of a pixel; drawn as four curves of at most 48 segments each, a 600-pixel one
        // by a sixth.)
        let inside = |[r, inner]: [f32; 2], middle: f32, x: i32, y: i32| {
            let points = (0..32 * 32).filter(|at| {
                let (px, py) = (x as f32 + (at % 32) as f32 / 32.0 + 1.0 / 64.0 - middle, y as f32 + (at / 32) as f32 / 32.0 + 1.0 / 64.0 - middle);
                (inner..=r).contains(&px.hypot(py))
            });
            points.count() as f32 / 1024.0
        };
        for (r, width) in [2.0f32, 4.5, 10.0, 40.0, 600.0].into_iter().flat_map(|r| [0.0f32, 1.0, 2.0, 8.0].map(|width| (r, width))) {
            let side = (2.0 * r + width + 6.0).ceil() as usize;
            let (middle, mut px) = (side as f32 / 2.0, block(side, side));
            let mut ring = Path::new();
            ring.circle(middle, middle, r);
            let mut canvas = Canvas::new(&mut px, side, side, (0, 0), 1.0);
            match width {
                0.0 => canvas.fill(&ring, pt(0.0, 0.0), 1.0, Color::WHITE),
                _ => canvas.stroke(&ring, pt(0.0, 0.0), 1.0, width, Color::WHITE),
            }
            let edges = if width == 0.0 { [r, -1.0] } else { [r + width / 2.0, r - width / 2.0] };
            // Only pixels near an edge: the rest are fully in or out.
            let (mut worst, mut all, mut count) = (0.0f32, 0.0, 0);
            for (x, y) in (0..side as i32).flat_map(|y| (0..side as i32).map(move |x| (x, y))) {
                let away = (x as f32 + 0.5 - middle).hypot(y as f32 + 0.5 - middle);
                if edges.iter().all(|edge| (away - edge).abs() > 1.5) {
                    continue;
                }
                let off = ((px[y as usize * side + x as usize] >> 24) as f32 / 255.0 - inside(edges, middle, x, y)).abs();
                (worst, all, count) = (worst.max(off), all + off, count + 1);
            }
            assert!(worst < 0.04 && all / (count as f32) < 1.5 / 255.0, "radius {r}, line {width}: the worst pixel is off by {worst:.4}, the mean one by {:.4}", all / count as f32);
        }
    }

    #[test]
    fn a_round_corner_is_smooth() {
        let mut px = block(12, 12);
        Canvas::new(&mut px, 12, 12, (0, 0), 1.0).rounded(rect(0.0, 0.0, 12.0, 12.0), 6.0, Color::WHITE);
        assert_eq!(px[0], 0, "the very corner is outside");
        assert_eq!(px[6 * 12 + 6], 0xffffffff, "the middle is inside");
        assert!(px.iter().any(|p| *p != 0 && *p != 0xffffffff), "the edge has in-between pixels");
    }

    #[test]
    fn a_filled_square_shape_covers_its_area() {
        let mut px = block(10, 10);
        Canvas::new(&mut px, 10, 10, (0, 0), 1.0).fill(&Path::svg("M2 2h6v6h-6z"), pt(0.0, 0.0), 1.0, Color::WHITE);
        let lit = px.iter().filter(|p| **p == 0xffffffff).count();
        assert_eq!(lit, 36);
        assert_eq!(px.iter().filter(|p| **p != 0).count(), 36, "its edges fall on whole pixels, so nothing is half covered");
    }
}
