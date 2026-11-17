//! Colour glyphs in the newer formats: COLR v1 paint graphs and `SVG ` documents. `emoji.rs`
//! draws the older ones itself (COLR v0 layers of one-colour outlines; `CBDT` or `sbix`
//! bitmaps), and asks this first only where a program asked for full emoji
//! ([`Ui::emoji_in_full`](crate::Ui::emoji_in_full), or its own font:
//! [`Ui::set_emoji`](crate::Ui::set_emoji)). Nothing else reaches this, so a program doing
//! neither does not carry it.
//!
//! A font may have a glyph in several formats. It is drawn as Firefox does: SVG if present,
//! else COLR v1, else v0, else bitmap. Chrome does the same except SVG, which it ignores;
//! FreeType the same, but bitmaps first. (The spec only puts v1 before v0.)
//!
//! COLR v1 writes a glyph as nested paints: a glyph outline filled by its child; layers; a
//! solid colour; linear, radial and sweep gradients; transforms; and a composite of two paints
//! in one of 28 modes. Paints are shared, so a glyph's paints form a graph, which a damaged
//! font can make cyclic, or deeper and wider than any drawing needs. It is walked once, with an
//! explicit task stack instead of recursion, into a flat step list: fill a shape, push a layer,
//! clip the top layer to a shape and composite it down, composite the top two. A paint
//! re-entered inside itself, or deeper than `DEEPEST`, is skipped (the rest of the glyph draws,
//! as the spec says); a glyph of more than `MOST` paints and colour stops is not drawn this way
//! at all, nor one whose paints are unbounded with no clip box. Drawing is then one pass over
//! the steps.
//!
//! Not done: variable fonts draw at their default instance, with palette 0. Colours blend (in
//! gradients and composites) as premultiplied sRGB values: as browsers do, and as the bitmap
//! versions of the same glyphs were made. (The spec asks for linear light.) Of SVG, whatever
//! `svg.rs` doesn't read.
use crate::canvas::{Canvas, coverage};
use crate::color::Color;
use crate::font_file::{Bytes, FontFile, first_not};
use crate::geom::{Point, pt};
use crate::path::Path;
use crate::svg::{Kept, Table, both, through, undone};
use std::f32::consts::{PI, TAU};

/// The deepest paint nesting: beyond any real drawing.
const DEEPEST: usize = 64;
/// Most paints and colour stops per glyph, each counted per use: shared paints let a few stand
/// for more than any drawing has.
const MOST: usize = 1 << 15;
/// A colour line with more stops than this is skipped.
const STOPS: usize = 1024;
/// Layers of more pixels than this are not drawn: no text is that large.
const LARGEST: usize = 1 << 22;
/// The most an `SVG ` document may decompress to.
const LONGEST: usize = 1 << 24;

/// A drawn glyph: its top left pixel's offset from the line start (right and down), its row
/// width, its pixels as a canvas stores them (premultiplied `0xAARRGGBB`), and whether any of
/// it uses the text colour.
pub(crate) struct Drawn {
    pub at: [i32; 2],
    pub width: usize,
    pub pixels: Vec<u32>,
    pub inked: bool,
}

/// Draws a glyph in a newer format, if the font has one: SVG, else COLR v1. A font unit is
/// `unit` pixels, the pen is `pen` pixels right of the line start and above the baseline, and
/// `ink` is the text colour. `None` if the font has neither (or its paints are refused, as
/// above).
pub(crate) fn drawn(font: &FontFile, glyph: u16, unit: f32, pen: [f32; 2], ink: Color) -> Option<Drawn> {
    let [colr, cpal, svg] = font.newer();
    written(font, svg, glyph, unit, pen, ink).or_else(|| painted(font, [colr, cpal], glyph, unit, pen, ink))
}

/// A glyph from an `SVG ` document. The table lists its documents after six bytes, each for a
/// glyph range, in order: first and last glyph, offset and length. A document may be gzipped.
/// In it, the glyph is the element with `id` `glyph` plus its number, drawn in font units, y
/// down from the pen.
fn written(font: &FontFile, svg: Bytes, glyph: u16, unit: f32, pen: [f32; 2], ink: Color) -> Option<Drawn> {
    let list = svg.u32(2)? as usize;
    let count = svg.u16(list)? as usize;
    let nth = first_not(count, |n| Some(svg.u16(list + 4 + n * 12)? < glyph))?;
    let entry = list + 2 + nth * 12;
    (nth < count && svg.u16(entry)? <= glyph).then_some(())?;
    let file = svg.part(list + svg.u32(entry + 4)? as usize, svg.u32(entry + 8)? as usize)?.0;
    let unpacked = if file.starts_with(&[0x1f, 0x8b, 8]) { Some(gunzip(file)?) } else { None };
    let text = std::str::from_utf8(unpacked.as_deref().unwrap_or(file)).ok()?;
    let em = 1.0 / font.unit(1.0);
    let drawing = Kept::glyph(text, &format!("glyph{glyph}"), em)?;
    // A drawing doesn't state its bounds. It is drawn on a layer with room for any glyph (half
    // an em left of the pen and below the baseline, an em and a half above and right), then
    // trimmed to what was drawn.
    let reach = |from: f32, by: f32| (from + by * em * unit).floor() as i32;
    let [left, top, right, bottom] = [reach(pen[0], -0.5), reach(-pen[1], -1.5), reach(pen[0], 1.5) + 1, reach(-pen[1], 0.5) + 1];
    let size = [(right - left) as usize, (bottom - top) as usize];
    (size[0] * size[1] <= LARGEST && em.is_finite()).then_some(())?;
    let place = [unit, 0.0, 0.0, unit, pen[0] - left as f32, -pen[1] - top as f32];
    let pixels = drawing.glyph_pixels(&place, size, ink);
    Some(trimmed(pixels, size[0], [left, top], !drawing.takes(ink).is_none()))
}

/// A gzip file's content. After a 10-byte header (the fourth byte flags what follows before the
/// data: extra field with its length, a NUL-terminated name and comment, a header CRC), the
/// deflate data; then the CRC-32 and length of the content. `None` if damaged.
fn gunzip(file: &[u8]) -> Option<Vec<u8>> {
    let (flags, mut at) = (*file.get(3)?, 10);
    if flags & 4 != 0 {
        at += 2 + u16::from_le_bytes(*file.get(at..)?.first_chunk::<2>()?) as usize;
    }
    for _ in [8, 16].into_iter().filter(|flag| flags & flag != 0) {
        at += file.get(at..)?.iter().position(|byte| *byte == 0)? + 1;
    }
    at += if flags & 2 != 0 { 2 } else { 0 };
    let (unpacked, long) = crate::png::deflated(file.get(at..)?, LONGEST)?;
    let tail = file.get(at + long..)?.first_chunk::<8>()?;
    // CRC-32, a byte at a time, through a 256-entry table.
    let checks: [u32; 256] = std::array::from_fn(|byte| (0..8).fold(byte as u32, |check, _| if check & 1 == 1 { check >> 1 ^ 0xEDB8_8320 } else { check >> 1 }));
    let check = !unpacked.iter().fold(!0u32, |check, byte| checks[(check as u8 ^ byte) as usize] ^ check >> 8);
    (tail[..4] == check.to_le_bytes() && tail[4..] == (unpacked.len() as u32).to_le_bytes()).then_some(unpacked)
}

/// A layer trimmed to its non-empty rows and columns. Its top left pixel was at `at`, and the
/// trimmed one's position is kept.
fn trimmed(pixels: Vec<u32>, width: usize, at: [i32; 2], inked: bool) -> Drawn {
    let used = pixels.iter().enumerate().filter(|(_, pixel)| **pixel != 0).map(|(n, _)| [n % width, n / width]);
    let [left, top, right, bottom] = used.fold([usize::MAX, usize::MAX, 0, 0], |[l, t, r, b], [x, y]| [l.min(x), t.min(y), r.max(x + 1), b.max(y + 1)]);
    if left >= right {
        return Drawn { at, width: 0, pixels: Vec::new(), inked };
    }
    let kept = (top..bottom).flat_map(|y| pixels[y * width + left..y * width + right].iter().copied()).collect();
    Drawn { at: [at[0] + left as i32, at[1] + top as i32], width: right - left, pixels: kept, inked }
}

/// A clip shape: a glyph outline, or a box (left, bottom, right, top, in font units).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Glyph(u16),
    Box([f32; 4]),
}

/// One drawing step. Steps work on a stack of glyph-sized layers, each on the top one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// A fill: within a shape, placed by its transform, or over the whole layer; with the
    /// fill's own geometry placed by the other transform.
    Fill(Option<(Shape, Table)>, Fill, Table),
    /// Push an empty layer.
    Open,
    /// Clip the top layer to a shape (clearing outside it), and composite it onto the one
    /// below.
    Clip(Shape, Table),
    /// Composite the top layer (source) with the one below (backdrop) in a mode (see
    /// `together`), and the result onto the layer below both.
    Composite(u8),
}

/// A fill: a solid colour; or a gradient along a colour line (see `Line`): linear from a point
/// (towards a second, which marks one step along the line), radial from one circle to another
/// (each centre and radius), or sweep around a point between two angles (counter-clockwise,
/// radians). A colour is red, green, blue, alpha, 0 to 1, premultiplied.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fill {
    Solid([f32; 4]),
    Linear(Line, Point, Point),
    Radial(Line, [f32; 3], [f32; 3]),
    Sweep(Line, Point, [f32; 2]),
}

/// A colour line: its stops' range among all lines' stops (each position and colour, in order),
/// and its extend mode: pad (0), repeat (1), or reflect (2).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    stops: [u32; 2],
    extend: u8,
}

/// A task while walking the paints, the next one last.
enum Task {
    /// A paint, by offset in `COLR`, with its transform; and the shape it fills, where it is
    /// the only fill of that shape.
    Paint(usize, Table, Option<(Shape, Table)>),
    /// A step, taken once everything pushed after it is done.
    Then(Step),
    /// The last entered paint is done.
    Leave,
}

/// A glyph's paints while walked: their tables, the text colour and whether it was used, and
/// the result.
struct Paints<'a> {
    colr: Bytes<'a>,
    cpal: Bytes<'a>,
    ink: Color,
    inked: bool,
    steps: Vec<Step>,
    /// The stops of all colour lines.
    stops: Vec<(f32, [f32; 4])>,
    /// Paints and stops visited so far.
    count: usize,
}

/// A glyph drawn from its paints in `COLR` (v1 and up), with colours from `CPAL`. After the
/// 14-byte v0 header, `COLR` gives the offset of its BaseGlyphList.
fn painted(font: &FontFile, [colr, cpal]: [Bytes; 2], glyph: u16, unit: f32, pen: [f32; 2], ink: Color) -> Option<Drawn> {
    (colr.u16(0)? >= 1 && cpal.u16(2)? > 0).then_some(())?;
    let mut paints = Paints { colr, cpal, ink, inked: false, steps: Vec::new(), stops: Vec::new(), count: 0 };
    let (root, boxed) = (paints.root(glyph)?, paints.boxed(glyph));
    // Font units go up from the baseline and pixels down: the glyph is flipped and placed at
    // its pen.
    let table = [unit, 0.0, 0.0, -unit, pen[0], -pen[1]];
    paints.walk(root, boxed, table)?;
    let (shapes, mut reach, bounded) = paints.shapes(font);
    // The glyph's bounds are its clip box, or its shapes'; if neither bounds it, it is
    // unbounded and not drawn.
    if let Some(edges) = boxed {
        reach = Some(placed(&outline(edges, &table)));
    }
    let [left, top, right, bottom] = reach.filter(|_| bounded || boxed.is_some())?;
    let area = [left.floor() as i32, top.floor() as i32, right.ceil() as i32, bottom.ceil() as i32];
    let size = (area[2] - area[0]).max(0) as usize * (area[3] - area[1]).max(0) as usize;
    if size == 0 || size > LARGEST {
        return Some(Drawn { at: [0; 2], width: 0, pixels: Vec::new(), inked: paints.inked });
    }
    let pixels = paints.draw(&shapes, area);
    Some(Drawn { at: [area[0], area[1]], width: (area[2] - area[0]) as usize, pixels, inked: paints.inked })
}

/// A 3-byte offset in `COLR`.
fn offset(colr: Bytes, at: usize) -> Option<usize> {
    Some((colr.u8(at)? as usize) << 16 | colr.u16(at + 1)? as usize)
}

/// A translation by `x`, `y`.
fn moved_by(x: f32, y: f32) -> Table {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// A transform about a centre other than the origin.
fn round(table: Table, [x, y]: [f32; 2]) -> Table {
    both(&both(&moved_by(x, y), &table), &moved_by(-x, -y))
}

/// A box's outline (left, bottom, right, top) placed by a transform.
fn outline([left, lower, right, upper]: [f32; 4], table: &Table) -> Path {
    let corner = |x: f32, y: f32| through(table, pt(x, y));
    let [a, b, c, d] = [corner(left, lower), corner(right, lower), corner(right, upper), corner(left, upper)];
    let mut path = Path::new();
    path.move_to(a.x, a.y).line_to(b.x, b.y).line_to(c.x, c.y).line_to(d.x, d.y).close();
    path
}

/// The bounds of a path of straight lines: left, top, right, bottom.
fn placed(path: &Path) -> [f32; 4] {
    let mut reach = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    path.flatten(1.0, pt(0.0, 0.0), 1.0, |a, b| reach = [reach[0].min(a.x).min(b.x), reach[1].min(a.y).min(b.y), reach[2].max(a.x).max(b.x), reach[3].max(a.y).max(b.y)]);
    reach
}

impl Paints<'_> {
    /// The offset of a glyph's root paint in `COLR`. The BaseGlyphList gives its count, then
    /// six bytes per glyph, in order: glyph id, and paint offset from the list's start.
    fn root(&self, glyph: u16) -> Option<usize> {
        let list = self.colr.u32(14).filter(|list| *list != 0)? as usize;
        let count = self.colr.u32(list)? as usize;
        let nth = first_not(count, |n| Some(self.colr.u16(list + 4 + n * 6)? < glyph))?;
        let entry = list + 4 + nth * 6;
        (nth < count && self.colr.u16(entry)? == glyph).then_some(list + self.colr.u32(entry + 2)? as usize)
    }

    /// A glyph's clip box, if the font gives one. `COLR` gives the ClipList's offset: a format
    /// byte and count, then seven bytes per range, in glyph order: first and last glyph, and
    /// the clip's 3-byte offset from the list's start. A clip is a format byte, then its four
    /// edges.
    fn boxed(&self, glyph: u16) -> Option<[f32; 4]> {
        let list = self.colr.u32(22).filter(|list| *list != 0)? as usize;
        let count = self.colr.u32(list + 1)? as usize;
        let nth = first_not(count, |n| Some(self.colr.u16(list + 7 + n * 7)? < glyph))?;
        let entry = list + 5 + nth * 7;
        (nth < count && self.colr.u16(entry)? <= glyph).then_some(())?;
        let at = list + offset(self.colr, entry + 4)?;
        let edge = |n: usize| self.colr.i16(at + 1 + n * 2).map(f32::from);
        matches!(self.colr.u8(at)?, 1 | 2).then_some([edge(0)?, edge(1)?, edge(2)?, edge(3)?])
    }

    /// Walks a glyph's paints from `root`, clipped to its box if it has one, into drawing
    /// steps. `None` if they exceed `MOST`.
    fn walk(&mut self, root: usize, boxed: Option<[f32; 4]>, table: Table) -> Option<()> {
        let mut tasks = vec![Task::Paint(root, table, None)];
        if let Some(edges) = boxed {
            self.steps.push(Step::Open);
            tasks.insert(0, Task::Then(Step::Clip(Shape::Box(edges), table)));
        }
        // The paints entered and not done: the path down to the current one.
        let mut path = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Paint(at, table, within) => self.enter(at, table, within, &mut tasks, &mut path)?,
                Task::Then(step) => self.steps.push(step),
                Task::Leave => _ = path.pop(),
            }
        }
        Some(())
    }

    /// Enters the paint at `at`, unless it is its own ancestor or too deep: then it is skipped.
    /// `None` once there have been too many.
    fn enter(&mut self, at: usize, table: Table, within: Option<(Shape, Table)>, tasks: &mut Vec<Task>, path: &mut Vec<usize>) -> Option<()> {
        self.count += 1;
        (self.count <= MOST).then_some(())?;
        if path.len() < DEEPEST && !path.contains(&at) {
            path.push(at);
            tasks.push(Task::Leave);
            _ = self.paint(at, table, within, tasks);
        }
        Some(())
    }

    /// Takes one paint: what it draws becomes steps, its children go on the task stack. A paint
    /// starts with its format byte (one of 32). Those with one child give its offset, from
    /// their own start, in the next three bytes.
    fn paint(&mut self, at: usize, table: Table, within: Option<(Shape, Table)>, tasks: &mut Vec<Task>) -> Option<()> {
        let kind = self.colr.u8(at)?;
        let colr = self.colr;
        let child = |from: usize| Some(at + offset(colr, at + from)?);
        if (12..=31).contains(&kind) {
            let turned = self.turned(at, kind)?;
            tasks.push(Task::Paint(child(1)?, both(&table, &turned), within));
            return Some(());
        }
        if (2..=9).contains(&kind) {
            let fill = self.fill(at, kind)?;
            self.steps.push(Step::Fill(within, fill, table));
            return Some(());
        }
        // Anything else filling a shape is drawn on its own layer, then clipped to the shape.
        if let Some((shape, held)) = within.filter(|_| [1, 10, 11, 32].contains(&kind)) {
            self.steps.push(Step::Open);
            tasks.push(Task::Then(Step::Clip(shape, held)));
        }
        match kind {
            1 => self.layers(at, table, tasks),
            10 => {
                let shape = Some((Shape::Glyph(self.colr.u16(at + 4)?), table));
                tasks.push(Task::Paint(child(1)?, table, shape));
                Some(())
            }
            11 => self.reused(self.colr.u16(at + 1)?, table, tasks),
            32 => self.composite(at, table, tasks),
            _ => None,
        }
    }

    /// PaintColrLayers: a count (a byte) and first index (four bytes) into the LayerList.
    /// `COLR` gives the LayerList's offset: a count, then each paint's offset from the list's
    /// start. The first is drawn first.
    fn layers(&mut self, at: usize, table: Table, tasks: &mut Vec<Task>) -> Option<()> {
        let (many, first) = (self.colr.u8(at + 1)? as usize, self.colr.u32(at + 2)? as usize);
        let list = self.colr.u32(18).filter(|list| *list != 0)? as usize;
        let all = self.colr.u32(list)? as usize;
        for n in (first..first.saturating_add(many).min(all)).rev() {
            tasks.push(Task::Paint(list + self.colr.u32(list + 4 + n * 4)? as usize, table, None));
        }
        Some(())
    }

    /// PaintColrGlyph: another glyph's paints as part of this one, clipped to that glyph's box.
    fn reused(&mut self, glyph: u16, table: Table, tasks: &mut Vec<Task>) -> Option<()> {
        let root = self.root(glyph)?;
        if let Some(edges) = self.boxed(glyph) {
            self.steps.push(Step::Open);
            tasks.push(Task::Then(Step::Clip(Shape::Box(edges), table)));
        }
        tasks.push(Task::Paint(root, table, None));
        Some(())
    }

    /// PaintComposite: the source's offset (three bytes), the mode (a byte), the backdrop's
    /// offset. The backdrop draws first, each on its own layer. An unknown mode skips the
    /// paint.
    fn composite(&mut self, at: usize, table: Table, tasks: &mut Vec<Task>) -> Option<()> {
        let (source, mode, backdrop) = (at + offset(self.colr, at + 1)?, self.colr.u8(at + 4)?, at + offset(self.colr, at + 5)?);
        (mode <= 27).then_some(())?;
        self.steps.push(Step::Open);
        tasks.extend([Task::Then(Step::Composite(mode)), Task::Paint(source, table, None), Task::Then(Step::Open), Task::Paint(backdrop, table, None)]);
        Some(())
    }

    /// The transform of a transform paint (formats 12 to 31) for its child, in font units. Each
    /// comes in pairs, the second variable, with four more bytes at its end (not read here).
    /// Angles are in half turns; scales and angles are F2Dot14; positions are font units.
    fn turned(&self, at: usize, kind: u8) -> Option<Table> {
        let colr = self.colr;
        let part = |n: usize| Some(colr.i16(at + n)? as f32 / 16384.0);
        let unit = |n: usize| colr.i16(at + n).map(f32::from);
        let centre = |n: usize| Some([unit(n)?, unit(n + 2)?]);
        let turn = |angle: f32| [(angle * PI).cos(), (angle * PI).sin(), -(angle * PI).sin(), (angle * PI).cos(), 0.0, 0.0];
        let slant = |x: f32, y: f32| [1.0, (y * PI).tan(), -(x * PI).tan(), 1.0, 0.0, 0.0];
        let stretch = |x: f32, y: f32| [x, 0.0, 0.0, y, 0.0, 0.0];
        Some(match kind {
            12 | 13 => self.matrix(at + offset(self.colr, at + 4)?)?,
            14 | 15 => moved_by(unit(4)?, unit(6)?),
            16 | 17 => stretch(part(4)?, part(6)?),
            18 | 19 => round(stretch(part(4)?, part(6)?), centre(8)?),
            20 | 21 => stretch(part(4)?, part(4)?),
            22 | 23 => round(stretch(part(4)?, part(4)?), centre(6)?),
            24 | 25 => turn(part(4)?),
            26 | 27 => round(turn(part(4)?), centre(6)?),
            28 | 29 => slant(part(4)?, part(6)?),
            _ => round(slant(part(4)?, part(6)?), centre(8)?),
        })
    }

    /// An Affine2x3: six Fixed (16.16) values, in [`Table`]'s order.
    fn matrix(&self, at: usize) -> Option<Table> {
        let number = |n: usize| Some(self.colr.u32(at + n * 4)? as i32 as f32 / 65536.0);
        Some([number(0)?, number(1)?, number(2)?, number(3)?, number(4)?, number(5)?])
    }

    /// A fill (formats 2 to 9, each paired as transforms are). A solid gives a palette index
    /// (two bytes) and alpha (F2Dot14). A gradient gives its ColorLine offset (three bytes),
    /// then its geometry: three points for linear, two circles for radial (centre and radius,
    /// never negative), and for sweep a centre and two angles, stored a half turn less. A
    /// gradient that draws nothing, by the spec's rules, is skipped.
    fn fill(&mut self, at: usize, kind: u8) -> Option<Fill> {
        let colr = self.colr;
        let unit = |n: usize| colr.i16(at + n).map(f32::from);
        let point = |n: usize| Some(pt(unit(n)?, unit(n + 2)?));
        let part = |n: usize| Some(colr.i16(at + n)? as f32 / 16384.0);
        if kind <= 3 {
            return Some(Fill::Solid(self.colour(colr.u16(at + 1)?, part(3)?)));
        }
        let line = self.line(at + offset(self.colr, at + 1)?, kind % 2 == 1)?;
        let [low, high] = [line.stops[0], line.stops[1].saturating_sub(1)].map(|n| self.stops.get(n as usize).map_or(0.0, |stop| stop.0));
        // Repeat or reflect with all stops at one position has no length to repeat.
        (line.extend == 0 || low < high).then_some(())?;
        match kind {
            4 | 5 => linear(line, [point(4)?, point(8)?, point(12)?]),
            6 | 7 => {
                let circles = [[unit(4)?, unit(6)?, colr.u16(at + 8)? as f32], [unit(10)?, unit(12)?, colr.u16(at + 14)? as f32]];
                (circles[0] != circles[1]).then_some(Fill::Radial(line, circles[0], circles[1]))
            }
            _ => {
                let angles = [(part(8)? + 1.0) * PI, (part(10)? + 1.0) * PI];
                (line.extend == 0 || angles[0] != angles[1]).then_some(Fill::Sweep(line, point(4)?, angles))
            }
        }
    }

    /// A ColorLine: extend mode (a byte), stop count (two bytes), then each stop's position and
    /// colour, as a solid gives one, plus four bytes in a variable font. Stops are sorted by
    /// position; of stops at one position, the first applies before it and the last after.
    fn line(&mut self, at: usize, variable: bool) -> Option<Line> {
        let (extend, many, long) = (self.colr.u8(at)?, self.colr.u16(at + 1)? as usize, if variable { 10 } else { 6 });
        self.count += many;
        (many <= STOPS && self.count <= MOST).then_some(())?;
        let first = self.stops.len();
        for stop in (0..many).map(|n| at + 3 + n * long) {
            let (along, colour) = (self.colr.i16(stop)? as f32 / 16384.0, self.colr.u16(stop + 2)?);
            let stood = self.colour(colour, self.colr.i16(stop + 4)? as f32 / 16384.0);
            self.stops.push((along, stood));
        }
        // Insertion sort, which keeps stops at one position in order.
        for n in first + 1..self.stops.len() {
            let mut k = n;
            while k > first && self.stops[k - 1].0 > self.stops[k].0 {
                self.stops.swap(k - 1, k);
                k -= 1;
            }
        }
        Some(Line { stops: [first as u32, self.stops.len() as u32], extend })
    }

    /// A colour of palette 0, its alpha times `solid`; index 0xFFFF is the text colour. `CPAL`
    /// gives the palette size, the colour records' offset, and each palette's first index; a
    /// colour is four bytes: blue, green, red, alpha. An index beyond the palette is
    /// transparent.
    fn colour(&mut self, number: u16, solid: f32) -> [f32; 4] {
        let cpal = self.cpal;
        let own = || {
            (number < cpal.u16(2)?).then_some(())?;
            Some(cpal.u32(cpal.u32(8)? as usize + (cpal.u16(12)? as usize + number as usize) * 4)?.swap_bytes())
        };
        let colour = if number == 0xffff { Some(self.ink.0) } else { own() };
        self.inked |= number == 0xffff;
        let colour = colour.unwrap_or(0);
        let alpha = (colour >> 24) as f32 / 255.0 * solid.clamp(0.0, 1.0);
        let part = |shift: u32| (colour >> shift & 255) as f32 / 255.0 * alpha;
        [part(16), part(8), part(0), alpha]
    }

    /// The steps' shapes, placed in line pixels (none for steps without one); their bounds, if
    /// anything is drawn; and whether everything drawn is clipped to them, which the spec
    /// derives from the paints as this does from the steps.
    fn shapes(&self, font: &FontFile) -> (Vec<Path>, Option<[f32; 4]>, bool) {
        let (mut shapes, mut reach, mut held) = (Vec::with_capacity(self.steps.len()), None::<[f32; 4]>, vec![true]);
        for step in &self.steps {
            let (shape, bounded) = match *step {
                Step::Fill(Some((shape, table)), ..) | Step::Clip(shape, table) => (Some(shaped(font, shape, &table)), true),
                Step::Fill(None, ..) => (None, false),
                _ => (None, true),
            };
            // What a step adds to the top layer is bounded by shapes or not; a layer becomes
            // bounded once clipped, or as its composite mode says.
            match *step {
                Step::Open => held.push(true),
                Step::Clip(..) => _ = held.pop(),
                Step::Composite(mode) => bounded_together(&mut held, mode),
                _ => {}
            }
            if !bounded {
                held.last_mut().into_iter().for_each(|held| *held = false);
            }
            let Some((path, [l, t, r, b])) = shape else {
                shapes.push(Path::new());
                continue;
            };
            reach = Some(reach.map_or([l, t, r, b], |[left, top, right, bottom]| [left.min(l), top.min(t), right.max(r), bottom.max(b)]));
            shapes.push(path);
        }
        (shapes, reach.filter(|[left, top, right, bottom]| left < right && top < bottom), held.first().copied().unwrap_or(false))
    }

    /// Draws the steps on layers covering `area` of the line (left, top, right, bottom,
    /// pixels), with `shapes` computed for them. Returns the bottom layer.
    fn draw(&self, shapes: &[Path], area: [i32; 4]) -> Vec<u32> {
        let width = (area[2] - area[0]) as usize;
        let mut blocks = vec![vec![0u32; width * (area[3] - area[1]) as usize]];
        for (step, shape) in self.steps.iter().zip(shapes) {
            match *step {
                Step::Fill(held, fill, table) => self.fill_in(blocks.last_mut(), area, held.map(|_| shape), fill, &table),
                Step::Open => blocks.push(vec![0; blocks[0].len()]),
                Step::Clip(..) => held_to(&mut blocks, shape, area),
                Step::Composite(mode) => laid_together(&mut blocks, mode, width),
            }
        }
        blocks.swap_remove(0)
    }

    /// Fills a shape on a layer (or the whole layer) with a fill whose geometry `table` places
    /// on the line.
    fn fill_in(&self, block: Option<&mut Vec<u32>>, area: [i32; 4], shape: Option<&Path>, fill: Fill, table: &Table) {
        let Some(block) = block else { return };
        // A gradient whose geometry the transform collapses onto a line draws nothing.
        let back = match fill {
            Fill::Solid(_) => Some([0.0; 6]),
            _ => undone(table),
        };
        let Some(back) = back else { return };
        let (width, height) = ((area[2] - area[0]) as usize, (area[3] - area[1]) as usize);
        let mut canvas = Canvas::new(block, width, height, (area[0], area[1]), 1.0);
        let box_edges = outline([area[0] as f32, area[1] as f32, area[2] as f32, area[3] as f32], &moved_by(0.0, 0.0));
        let edges = shape.unwrap_or(&box_edges);
        canvas.fill_with(&mut |each| edges.flatten(1.0, pt(0.0, 0.0), 1.0, each), false, |x, y| color(self.shade(&fill, &back, x, y)));
    }

    /// A fill's colour at the centre of pixel `x`, `y` of the line; `back` maps the pixel back
    /// to the fill's own geometry.
    fn shade(&self, fill: &Fill, back: &Table, x: i32, y: i32) -> [f32; 4] {
        let p = through(back, pt(x as f32 + 0.5, y as f32 + 0.5));
        let (line, along) = match *fill {
            Fill::Solid(colour) => return colour,
            Fill::Linear(line, from, step) => (line, (p.x - from.x) * step.x + (p.y - from.y) * step.y),
            Fill::Radial(line, from, to) => (line, outwards(from, to, p).unwrap_or(f32::NAN)),
            Fill::Sweep(line, centre, [start, end]) => (line, swept((p.y - centre.y).atan2(p.x - centre.x).rem_euclid(TAU), start, end)),
        };
        if along.is_nan() { [0.0; 4] } else { self.along(line, along) }
    }

    /// A colour line's colour at `along`.
    fn along(&self, line: Line, along: f32) -> [f32; 4] {
        let stops = &self.stops[line.stops[0] as usize..line.stops[1] as usize];
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else { return [0.0; 4] };
        let (low, long) = (first.0, last.0 - first.0);
        let along = match line.extend {
            1 => low + (along - low).rem_euclid(long),
            2 => low + long - ((along - low).rem_euclid(2.0 * long) - long).abs(),
            _ => along,
        };
        let after = stops.partition_point(|stop| stop.0 <= along);
        match (after.checked_sub(1).map(|before| stops[before]), stops.get(after)) {
            (Some((from, a)), Some((to, b))) => [0, 1, 2, 3].map(|n| a[n] + (b[n] - a[n]) * (along - from) / (to - from)),
            (Some((_, only)), None) | (None, Some(&(_, only))) => only,
            (None, None) => [0.0; 4],
        }
    }
}

/// A linear gradient from three points: the colour line's start (p0), end (p1), and a third
/// (p2) setting its orientation. Colours are constant along p0 to p2, so the end is p1
/// projected onto the perpendicular of that (p3 in the spec): the step is one unit from p0 to
/// p3. `None` if degenerate.
fn linear(line: Line, [from, to, turned]: [Point; 3]) -> Option<Fill> {
    let across = pt(-(turned.y - from.y), turned.x - from.x);
    let far = ((to.x - from.x) * across.x + (to.y - from.y) * across.y) / (across.x * across.x + across.y * across.y);
    let way = pt(across.x * far, across.y * far);
    let long = way.x * way.x + way.y * way.y;
    (long.is_normal()).then(|| Fill::Linear(line, from, pt(way.x / long, way.y / long)))
}

/// How far from the first circle to the second (each centre and radius) the last circle through
/// `p` lies: later circles draw over earlier ones, and none with negative radius draws. `None`
/// if none passes through it.
fn outwards([x0, y0, r0]: [f32; 3], [x1, y1, r1]: [f32; 3], p: Point) -> Option<f32> {
    let (cx, cy, dr, px, py) = (x1 - x0, y1 - y0, r1 - r0, p.x - x0, p.y - y0);
    // Circle `t` has centre c0 + t (c1 - c0) and radius r0 + t (r1 - r0): one through `p` has a
    // t solving a t² - 2 b t + c = 0.
    let (a, b, c) = (cx * cx + cy * cy - dr * dr, px * cx + py * cy + r0 * dr, px * px + py * py - r0 * r0);
    let fits = |t: &f32| t.is_finite() && r0 + t * dr >= 0.0;
    if a.abs() <= (cx * cx + cy * cy + dr * dr) * 1e-6 {
        return Some(c / (2.0 * b)).filter(fits);
    }
    let root = (b * b - a * c).sqrt();
    let [one, other] = [(b + root) / a, (b - root) / a];
    [one.max(other), one.min(other)].into_iter().find(fits)
}

/// How far from `start` to `end` an angle lies (all radians). If the two are equal, it is
/// before or after.
fn swept(angle: f32, start: f32, end: f32) -> f32 {
    let long = end - start;
    match long != 0.0 {
        true => (angle - start) / long,
        false if angle < start => f32::MIN,
        false => f32::MAX,
    }
}

/// A `Fill` colour as a [`Color`].
fn color([red, green, blue, solid]: [f32; 4]) -> Color {
    if solid <= 0.0 {
        return Color::NONE;
    }
    let byte = |part: f32| (part.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    Color(byte(solid) << 24 | byte(red / solid) << 16 | byte(green / solid) << 8 | byte(blue / solid))
}

/// A shape placed by `table`, and its bounds.
fn shaped(font: &FontFile, shape: Shape, table: &Table) -> (Path, [f32; 4]) {
    match shape {
        Shape::Glyph(glyph) => font.outline_through(glyph, *table),
        Shape::Box(edges) => {
            let path = outline(edges, table);
            let reach = placed(&path);
            (path, reach)
        }
    }
}

/// Whether a composite of the top two layers is bounded by shapes, from whether each is (the
/// top two of `held`, which become one), as the spec says per mode.
fn bounded_together(held: &mut Vec<bool>, mode: u8) {
    let (source, backdrop) = (held.pop().unwrap_or(true), held.pop().unwrap_or(true));
    let bounded = match mode {
        0 => true,
        1 | 7 => source,
        2 | 8 => backdrop,
        5 | 6 => source || backdrop,
        _ => source && backdrop,
    };
    if !bounded {
        held.last_mut().into_iter().for_each(|held| *held = false);
    }
}

/// Clips the top layer to a shape (each pixel by its coverage) and composites it onto the one
/// below.
fn held_to(blocks: &mut Vec<Vec<u32>>, shape: &Path, area: [i32; 4]) {
    let (width, height) = ((area[2] - area[0]) as usize, (area[3] - area[1]) as usize);
    let cover = coverage(shape, 1.0, 0.0, (area[0], area[1]), width, height);
    let Some(mut top) = blocks.pop() else { return };
    top.iter_mut().zip(&cover).for_each(|(pixel, cover)| *pixel = times(*pixel, *cover as u32));
    laid(blocks, &top, width);
}

/// Composites the top two layers (see `together`), and the result onto the one below.
fn laid_together(blocks: &mut Vec<Vec<u32>>, mode: u8, width: usize) {
    let (Some(source), Some(mut backdrop)) = (blocks.pop(), blocks.pop()) else { return };
    backdrop.iter_mut().zip(&source).for_each(|(backdrop, source)| *backdrop = together(mode, *source, *backdrop));
    laid(blocks, &backdrop, width);
}

/// Composites a layer onto the top one of `blocks`.
fn laid(blocks: &mut [Vec<u32>], over: &[u32], width: usize) {
    let Some(under) = blocks.last_mut() else { return };
    let height = under.len() / width.max(1);
    Canvas::new(under, width, height, (0, 0), 1.0).pixels(over, width, [0, 0], false);
}

/// A canvas pixel times `part`/255.
fn times(pixel: u32, part: u32) -> u32 {
    let pair = |pair: u32| (pair * part + 0x0080_0080 + ((pair * part + 0x0080_0080) >> 8 & 0x00ff_00ff)) >> 8 & 0x00ff_00ff;
    pair(pixel & 0x00ff_00ff) | pair(pixel >> 8 & 0x00ff_00ff) << 8
}

/// A canvas pixel as premultiplied colour (red, green, blue) and alpha, 0 to 1.
fn parts(pixel: u32) -> ([f32; 3], f32) {
    ([16, 8, 0].map(|shift| (pixel >> shift & 255) as f32 / 255.0), (pixel >> 24) as f32 / 255.0)
}

/// A colour and alpha as a canvas pixel.
fn pixel(colour: [f32; 3], solid: f32) -> u32 {
    let solid = solid.clamp(0.0, 1.0);
    let byte = |part: f32| (part.clamp(0.0, solid) * 255.0 + 0.5) as u32;
    byte(solid) << 24 | byte(colour[0]) << 16 | byte(colour[1]) << 8 | byte(colour[2])
}

/// Composites two pixels in a `COLR` mode, as W3C Compositing and Blending defines it: source
/// `s` with backdrop `b`. The first 13 modes are Porter-Duff; the rest blend the colours where
/// both exist and composite the source over the backdrop.
fn together(mode: u8, s: u32, b: u32) -> u32 {
    let ((s, sa), (b, ba)) = (parts(s), parts(b));
    let (of_s, of_b) = match mode {
        0 => (0.0, 0.0),
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        3 => (1.0, 1.0 - sa),
        4 => (1.0 - ba, 1.0),
        5 => (ba, 0.0),
        6 => (0.0, sa),
        7 => (1.0 - ba, 0.0),
        8 => (0.0, 1.0 - sa),
        9 => (ba, 1.0 - sa),
        10 => (1.0 - ba, sa),
        11 => (1.0 - ba, 1.0 - sa),
        12 => (1.0, 1.0),
        _ => return blended(mode, (s, sa), (b, ba)),
    };
    pixel([0, 1, 2].map(|n| s[n] * of_s + b[n] * of_b), sa * of_s + ba * of_b)
}

/// The source over the backdrop, colours blended where both exist by a blend mode (13 to 27).
fn blended(mode: u8, (s, sa): ([f32; 3], f32), (b, ba): ([f32; 3], f32)) -> u32 {
    let own = |colour: [f32; 3], solid: f32| colour.map(|part| if solid > 0.0 { part / solid } else { 0.0 });
    let (cs, cb) = (own(s, sa), own(b, ba));
    let mixed = match mode {
        24 => with_light(with_strength(cs, strength(cb)), light(cb)),
        25 => with_light(with_strength(cb, strength(cs)), light(cb)),
        26 => with_light(cs, light(cb)),
        27 => with_light(cb, light(cs)),
        _ => [0, 1, 2].map(|n| mix(mode, cs[n], cb[n])),
    };
    pixel([0, 1, 2].map(|n| s[n] * (1.0 - ba) + b[n] * (1.0 - sa) + sa * ba * mixed[n]), sa + ba - sa * ba)
}

/// One channel of source `s` and backdrop `b`, blended by a separable mode (13 to 23).
fn mix(mode: u8, s: f32, b: f32) -> f32 {
    // Hard light multiplies where the source is dark and screens where it is light.
    let hard = |b: f32, s: f32| if s <= 0.5 { b * 2.0 * s } else { b + (2.0 * s - 1.0) - b * (2.0 * s - 1.0) };
    let soft = |d: f32| b + (2.0 * s - 1.0) * (d - b);
    match mode {
        13 => b + s - b * s,
        14 => hard(s, b),
        15 => b.min(s),
        16 => b.max(s),
        17 if b <= 0.0 => 0.0,
        17 if s >= 1.0 => 1.0,
        17 => (b / (1.0 - s)).min(1.0),
        18 if b >= 1.0 => 1.0,
        18 if s <= 0.0 => 0.0,
        18 => 1.0 - ((1.0 - b) / s).min(1.0),
        19 => hard(b, s),
        20 if s <= 0.5 => b - (1.0 - 2.0 * s) * b * (1.0 - b),
        20 if b <= 0.25 => soft(((16.0 * b - 12.0) * b + 4.0) * b),
        20 => soft(b.sqrt()),
        21 => (b - s).abs(),
        22 => b + s - 2.0 * b * s,
        _ => b * s,
    }
}

/// Luminosity, weighting red, green and blue as the eye does.
fn light([red, green, blue]: [f32; 3]) -> f32 {
    0.3 * red + 0.59 * green + 0.11 * blue
}

/// Saturation: the spread between the largest and smallest channel.
fn strength(colour: [f32; 3]) -> f32 {
    colour.iter().fold(f32::MIN, |most, part| most.max(*part)) - colour.iter().fold(f32::MAX, |least, part| least.min(*part))
}

/// A colour with saturation `strong`, channel order kept.
fn with_strength(colour: [f32; 3], strong: f32) -> [f32; 3] {
    let (least, now) = (colour.iter().fold(f32::MAX, |least, part| least.min(*part)), strength(colour));
    colour.map(|part| if now > 0.0 { (part - least) * strong / now } else { 0.0 })
}

/// A colour with luminosity `to`, clipped back into gamut.
fn with_light(colour: [f32; 3], to: f32) -> [f32; 3] {
    let colour = colour.map(|part| part + to - light(colour));
    let (l, least, most) = (light(colour), colour.iter().fold(f32::MAX, |least, part| least.min(*part)), colour.iter().fold(f32::MIN, |most, part| most.max(*part)));
    let colour = colour.map(|part| if least < 0.0 { l + (part - l) * l / (l - least) } else { part });
    colour.map(|part| if most > 1.0 { l + (part - l) * (1.0 - l) / (most - l) } else { part })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_file::tests::{coloured, file, words};

    /// Big-endian 4-byte numbers.
    fn longs(numbers: &[u32]) -> Vec<u8> {
        numbers.iter().flat_map(|number| number.to_be_bytes()).collect()
    }

    /// A 3-byte offset.
    fn three(at: usize) -> Vec<u8> {
        (at as u32).to_be_bytes()[1..].to_vec()
    }

    /// The tests' paints. A paint's children are named by index among them all, and must follow
    /// it.
    enum P {
        Solid(u16, f32),
        Glyph(u16, usize),
        Layers(u8, u32),
        Reused(u16),
        Moved(i16, i16, usize),
        Turned(f32, usize),
        Together(usize, u8, usize),
        Linear([i16; 6], u8, Vec<(f32, u16)>),
    }

    /// A test paint's encoded length.
    fn long(paint: &P) -> usize {
        match paint {
            P::Solid(..) => 5,
            P::Glyph(..) | P::Layers(..) | P::Turned(..) => 6,
            P::Reused(_) => 3,
            P::Moved(..) | P::Together(..) => 8,
            P::Linear(_, _, stops) => 16 + 3 + stops.len() * 6,
        }
    }

    /// A test paint, encoded with offsets from `starts`.
    fn written(paint: &P, n: usize, starts: &[usize]) -> Vec<u8> {
        let from = |child: &usize| three(starts[*child] - starts[n]);
        match paint {
            P::Solid(colour, solid) => [vec![2], words(&[*colour as i32, (*solid * 16384.0) as i32])].concat(),
            P::Glyph(glyph, child) => [vec![10], from(child), words(&[*glyph as i32])].concat(),
            P::Layers(many, first) => [vec![1, *many], longs(&[*first])].concat(),
            P::Reused(glyph) => [vec![11], words(&[*glyph as i32])].concat(),
            P::Moved(x, y, child) => [vec![14], from(child), words(&[*x as i32, *y as i32])].concat(),
            P::Turned(angle, child) => [vec![24], from(child), words(&[(*angle * 16384.0) as i32])].concat(),
            P::Together(source, mode, backdrop) => [vec![32], from(source), vec![*mode], from(backdrop)].concat(),
            P::Linear(points, extend, stops) => {
                let stops: Vec<u8> = stops.iter().flat_map(|(at, colour)| words(&[(*at * 16384.0) as i32, *colour as i32, 16384])).collect();
                [vec![4], three(16), words(&points.map(i32::from)), vec![*extend], words(&[(stops.len() / 6) as i32]), stops].concat()
            }
        }
    }

    /// A COLR v1 table: glyphs with their root paint (by index), layers (by index), the paints,
    /// and clip boxes for some glyphs.
    fn colr(roots: &[(u16, usize)], layers: &[usize], paints: &[P], boxes: &[(u16, [i16; 4])]) -> Vec<u8> {
        let starts: Vec<usize> = (0..paints.len()).map(|n| paints[..n].iter().map(long).sum()).collect();
        let base_at = 34;
        let list_at = base_at + 4 + roots.len() * 6;
        let clips_at = list_at + 4 + layers.len() * 4;
        let paints_at = clips_at + 5 + boxes.len() * 16;
        let mut table = [words(&[1, 0]), longs(&[0, 0]), words(&[0]), longs(&[base_at as u32, list_at as u32, if boxes.is_empty() { 0 } else { clips_at as u32 }, 0, 0])].concat();
        table.extend(longs(&[roots.len() as u32]));
        roots.iter().for_each(|(glyph, paint)| table.extend([words(&[*glyph as i32]), longs(&[(paints_at + starts[*paint] - base_at) as u32])].concat()));
        table.extend(longs(&[layers.len() as u32]));
        layers.iter().for_each(|paint| table.extend(longs(&[(paints_at + starts[*paint] - list_at) as u32])));
        table.extend([vec![1], longs(&[boxes.len() as u32])].concat());
        for (n, (glyph, _)) in boxes.iter().enumerate() {
            table.extend([words(&[*glyph as i32, *glyph as i32]), three(5 + boxes.len() * 7 + n * 9)].concat());
        }
        boxes.iter().for_each(|(_, edges)| table.extend([vec![1], words(&edges.map(i32::from))].concat()));
        paints.iter().enumerate().for_each(|(n, paint)| table.extend(written(paint, n, &starts)));
        table
    }

    /// The test emoji font of `font_file.rs`'s tests, with this `COLR` instead of its own, plus
    /// these tables. At 1024 units, glyph 0 is a box from 64 to 256 across and up to 448, glyph
    /// 8 a small box from 128 to 256 across and up to 128.
    fn font(colr: Vec<u8>, more: &[(&[u8; 4], Vec<u8>)]) -> FontFile {
        let mut tables: Vec<([u8; 4], Vec<u8>)> = coloured(more).into_iter().filter(|(name, _)| name != b"COLR").collect();
        tables.push((*b"COLR", colr));
        FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read")
    }

    /// A glyph drawn at 16 pixels per 1024 units, pen at the line start, for green text.
    fn draw(font: &FontFile, glyph: u16) -> Option<Drawn> {
        drawn(font, glyph, 16.0 / 1024.0, [0.0, 0.0], Color::rgb(0x00ff00))
    }

    /// A drawing's pixel at `x`, `y` of the line (transparent outside it).
    fn at(drawn: &Drawn, x: i32, y: i32) -> u32 {
        let (dx, dy) = (x - drawn.at[0], y - drawn.at[1]);
        let inside = dx >= 0 && dy >= 0 && (dx as usize) < drawn.width && (dy as usize) < drawn.pixels.len() / drawn.width.max(1);
        if inside { drawn.pixels[dy as usize * drawn.width + dx as usize] } else { 0 }
    }

    const YELLOW: u32 = 0xffff_cc00;
    const BLACK: u32 = 0xff00_0000;
    const RED: u32 = 0xffe0_0000;

    #[test]
    fn a_glyph_is_its_paints_filled_layered_moved_and_laid_together() {
        let paints = [
            // 0: glyph 1 is the small box in red.
            P::Glyph(8, 1),
            P::Solid(2, 1.0),
            // 2: glyph 2 is a yellow box under the small one in black, moved 128 right.
            P::Layers(2, 0),
            P::Glyph(0, 6),
            P::Moved(128, 0, 5),
            P::Glyph(8, 7),
            P::Solid(0, 1.0),
            P::Solid(1, 1.0),
            // 8: glyph 3 is the yellow box with the small one cut out.
            P::Together(9, 8, 10),
            P::Glyph(8, 11),
            P::Glyph(0, 12),
            P::Solid(1, 1.0),
            P::Solid(0, 1.0),
            // 13: glyph 5 is glyph 1, with the text colour at half alpha over it.
            P::Layers(2, 2),
            P::Reused(1),
            P::Glyph(8, 16),
            P::Solid(0xffff, 0.5),
        ];
        let font = font(colr(&[(1, 0), (2, 2), (3, 8), (5, 13)], &[3, 4, 14, 15], &paints, &[]), &[]);
        // At 16 pixels the small box is 2 square, a pixel right of the pen (its left edge in
        // the font), on the baseline.
        let one = draw(&font, 1).expect("glyph 1 is in paints");
        assert_eq!((one.at, one.width, one.pixels.clone(), one.inked), ([1, -2], 2, vec![RED; 4], false));
        // The box under it is 3 wide and 7 high, from a pixel right of the pen; the small one
        // is moved 2 right.
        let two = draw(&font, 2).expect("glyph 2 is in paints");
        assert_eq!([at(&two, 1, -1), at(&two, 2, -5), at(&two, 3, -1), at(&two, 4, -2), at(&two, 2, -1), at(&two, 5, -1)], [YELLOW, YELLOW, BLACK, BLACK, YELLOW, 0]);
        // Composited so the small box is cut out of the large one.
        let three = draw(&font, 3).expect("glyph 3 is in paints");
        assert_eq!([at(&three, 3, -1), at(&three, 1, -1), at(&three, 2, -2), at(&three, 1, -3)], [YELLOW, 0, 0, YELLOW]);
        // Another glyph's paints, and the text colour, which is reported as used.
        let five = draw(&font, 5).expect("glyph 5 is in paints");
        let half = at(&five, 1, -1);
        assert!(five.inked && half >> 24 == 255 && (half >> 16 & 255).abs_diff(0x70) <= 2 && (half >> 8 & 255).abs_diff(0x80) <= 2, "{half:08x}");
        // A glyph without paints is not drawn here.
        assert!(draw(&font, 4).is_none() && draw(&font, 0).is_none());
    }

    #[test]
    fn colours_go_into_each_other_along_a_line() {
        // Glyph 0's box, red at its left edge to black at its right; then rotated a quarter, so
        // it runs from bottom to top.
        let line = || vec![(0.0, 2), (1.0, 1)];
        let paints = [P::Glyph(0, 1), P::Linear([64, 0, 256, 0, 64, 100], 0, line()), P::Glyph(0, 3), P::Turned(0.5, 4), P::Linear([0, 0, 448, 0, 0, 100], 0, line())];
        let font = font(colr(&[(1, 0), (2, 2)], &[], &paints, &[]), &[]);
        let along = draw(&font, 1).expect("drawn");
        let reds: Vec<u32> = (1..4).map(|x| at(&along, x, -3) >> 16 & 255).collect();
        assert!(reds[0] > reds[1] && reds[1] > reds[2] && reds[0] > 150 && reds[2] < 80, "{reds:?}");
        assert!((-6..0).all(|y| at(&along, 2, y) == at(&along, 2, -1)), "the same down each column");
        let up = draw(&font, 2).expect("drawn");
        let reds: Vec<u32> = [-1, -4, -6].iter().map(|y| at(&up, 2, *y) >> 16 & 255).collect();
        assert!(reds[0] > reds[1] && reds[1] > reds[2], "{reds:?}");
    }

    #[test]
    fn paints_that_go_round_too_deep_or_too_wide_are_refused() {
        // Layers whose second is themselves: the first draws, and that's all.
        let round = [P::Layers(2, 0), P::Glyph(8, 2), P::Solid(2, 1.0)];
        assert_eq!(draw(&font(colr(&[(1, 0)], &[1, 0], &round, &[]), &[]), 1).map(|drawn| drawn.pixels), Some(vec![RED; 4]));
        // A glyph drawn as itself is nothing.
        assert!(draw(&font(colr(&[(1, 0)], &[], &[P::Reused(1)], &[]), &[]), 1).is_none());
        // Translated 60 times, the box draws; 70 times is too deep, and it doesn't.
        let deep = |times: usize| {
            let mut paints: Vec<P> = (0..times).map(|n| P::Moved(0, 0, n + 1)).collect();
            paints.extend([P::Glyph(8, times + 1), P::Solid(2, 1.0)]);
            draw(&font(colr(&[(1, 0)], &[], &paints, &[]), &[]), 1).map(|drawn| drawn.pixels)
        };
        assert_eq!((deep(60), deep(70)), (Some(vec![RED; 4]), None));
        // Layers of the same layers twice, 24 deep, stand for 16 million boxes: refused at
        // once.
        let mut wide: Vec<P> = (0..24u32).map(|n| P::Layers(2, n * 2)).collect();
        wide.extend([P::Glyph(8, 25), P::Solid(2, 1.0)]);
        let layers: Vec<usize> = (0..24).flat_map(|n| [n + 1, n + 1]).collect();
        let started = std::time::Instant::now();
        assert!(draw(&font(colr(&[(1, 0)], &layers, &wide, &[]), &[]), 1).is_none());
        assert!(started.elapsed().as_secs_f32() < 1.0);
        // A colour over everything with no clip box is not drawn; with one, it fills the box.
        let flood = [P::Solid(2, 1.0)];
        assert!(draw(&font(colr(&[(1, 0)], &[], &flood, &[]), &[]), 1).is_none());
        let boxed = draw(&font(colr(&[(1, 0)], &[], &flood, &[(1, [0, 0, 128, 64])]), &[]), 1).expect("held to its box");
        assert_eq!((boxed.at, boxed.width, boxed.pixels), ([0, -1], 2, vec![RED; 2]));
    }

    #[test]
    fn a_damaged_table_never_stops_the_program() {
        let paints = [P::Together(1, 23, 4), P::Layers(2, 0), P::Glyph(0, 3), P::Solid(0, 1.0), P::Glyph(8, 5), P::Linear([0, 0, 512, 0, 0, 512], 2, vec![(0.0, 2), (0.5, 1), (0.25, 0)])];
        let whole = colr(&[(1, 0), (2, 1)], &[2, 4], &paints, &[(2, [0, 0, 512, 512])]);
        assert!(draw(&font(whole.clone(), &[]), 1).is_some());
        for cut in 0..whole.len() {
            let cut_short = font(whole[..cut].to_vec(), &[]);
            let _ = (draw(&cut_short, 1), draw(&cut_short, 2));
            for byte in [0x00, 0x01, 0x20, 0x7f, 0xff] {
                let mut wrong = whole.clone();
                wrong[cut] = byte;
                let wrong = font(wrong, &[]);
                let _ = (draw(&wrong, 1), draw(&wrong, 2), draw(&wrong, 3));
            }
        }
    }

    /// An `SVG ` table of one document, for glyphs `first` to `last`.
    fn svg_table(first: u16, last: u16, document: &[u8]) -> Vec<u8> {
        [words(&[0]), longs(&[10, 0]), words(&[1, first as i32, last as i32]), longs(&[14, document.len() as u32]), document.to_vec()].concat()
    }

    /// Bytes as a gzip file, with a name.
    fn gzipped(bytes: &[u8]) -> Vec<u8> {
        let zlib = crate::png::zlib_compress(bytes);
        let checks: [u32; 256] = std::array::from_fn(|byte| (0..8).fold(byte as u32, |check, _| if check & 1 == 1 { check >> 1 ^ 0xEDB8_8320 } else { check >> 1 }));
        let check = !bytes.iter().fold(!0u32, |check, byte| checks[(check as u8 ^ byte) as usize] ^ check >> 8);
        [&[0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 255][..], b"glyph.svg\0", &zlib[2..zlib.len() - 4], &check.to_le_bytes(), &(bytes.len() as u32).to_le_bytes()].concat()
    }

    #[test]
    fn a_glyph_written_as_svg_is_drawn_in_the_fonts_units_packed_or_not() {
        // Glyphs 7 and 8 in one document: 8 a red 128-unit square on the baseline, 128 right of
        // the pen, in a group with another fill; 7 a 64-unit square in the text colour.
        let document = br##"<svg xmlns="http://www.w3.org/2000/svg"><g fill="blue"><rect id="glyph8" x="128" y="-128" width="128" height="128" fill="#ff0000"/></g><rect id="glyph7" x="0" y="-64" width="64" height="64" fill="currentColor"/></svg>"##;
        // Glyph 8 also has paints, in black: the SVG takes precedence.
        let both = colr(&[(8, 0)], &[], &[P::Glyph(8, 1), P::Solid(1, 1.0)], &[]);
        for file in [document.to_vec(), gzipped(document)] {
            let font = font(both.clone(), &[(b"SVG ", svg_table(7, 8, &file))]);
            let eight = draw(&font, 8).expect("glyph 8 is written");
            assert_eq!((eight.at, eight.width, eight.pixels, eight.inked), ([2, -2], 2, vec![0xffff_0000; 4], false));
            let seven = draw(&font, 7).expect("glyph 7 is written");
            assert_eq!((seven.at, seven.width, seven.pixels, seven.inked), ([0, -1], 1, vec![0xff00_ff00], true));
            assert!(draw(&font, 6).is_none() && draw(&font, 9).is_none());
        }
        // A document damaged anywhere, or truncated, is no image, and panics nothing.
        let packed = gzipped(document);
        assert_eq!(gunzip(&packed).as_deref(), Some(&document[..]));
        for cut in 0..packed.len() {
            assert!(gunzip(&packed[..cut]).is_none());
            let mut wrong = packed.clone();
            wrong[cut] ^= 0x10;
            let _ = draw(&font(both.clone(), &[(b"SVG ", svg_table(7, 8, &wrong))]), 8);
        }
    }

    #[test]
    fn a_font_drawn_only_the_newer_ways_is_used_where_that_is_asked_for() {
        use crate::text::{BuiltInFont, Font, TextSystem};
        // The test font with glyph 1 (a face) in paints only: the small box in red.
        let colr = colr(&[(1, 0)], &[], &[P::Glyph(8, 1), P::Solid(2, 1.0)], &[]);
        let tables: Vec<([u8; 4], Vec<u8>)> = coloured(&[]).into_iter().map(|(name, table)| if &name == b"COLR" { (name, colr.clone()) } else { (name, table) }).collect();
        let font = FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read");
        assert!(!font.pictured() && font.pictured_newer());
        let mut pixels = vec![0u32; 32 * 24];
        let mut emoji = crate::emoji::Emoji::around(Box::new(BuiltInFont)).with(font);
        emoji.draw(&mut Canvas::new(&mut pixels, 32, 24, (0, 0), 1.0), "\u{1f600}", 0.0, 0.0, Font::new(16.0), Color::WHITE);
        assert_eq!(pixels.iter().filter(|pixel| **pixel == RED).count(), 4);
    }

    /// Every glyph with paints in the system's emoji font draws at three sizes without
    /// panicking, and nearly all draw something.
    #[test]
    fn every_glyph_of_the_systems_font_is_drawn() {
        let names = ["C:/Windows/Fonts/seguiemj.ttf", "/mnt/c/Windows/Fonts/seguiemj.ttf"];
        let Some(font) = names.iter().find_map(FontFile::open) else { return eprintln!("no font with paints here") };
        let (mut something, mut painted, started) = (0, 0, std::time::Instant::now());
        for glyph in 0..u16::MAX {
            let sizes = [16.0, 33.0, 109.0].map(|pixels| drawn(&font, glyph, font.unit(pixels), [0.0, 0.0], Color::BLACK));
            let Some(small) = &sizes[0] else { continue };
            painted += 1;
            something += small.pixels.iter().any(|pixel| *pixel != 0) as usize;
        }
        println!("{something} of {painted} glyphs with paints come to something; {:.2} ms a glyph at three sizes", started.elapsed().as_secs_f64() * 1000.0 / painted.max(1) as f64);
        assert!(painted > 3000 && something * 100 >= painted * 99, "{something} of {painted}");
    }

    /// The fonts in the folder `TAUT_COLR` names that have bitmap twins of the same glyphs:
    /// each glyph drawn from its paints or SVG at the bitmaps' size, compared with its bitmap,
    /// both over white.
    #[test]
    #[ignore]
    fn the_glyphs_of_real_fonts_are_drawn_as_their_pictures_show_them() {
        let Some(folder) = std::env::var_os("TAUT_COLR").map(std::path::PathBuf::from) else { return eprintln!("TAUT_COLR names no folder") };
        let twins = [
            ("noto-glyf_colr_1.ttf", "noto-sbix.ttf"),
            ("noto-cff_colr_1.otf", "noto-sbix.ttf"),
            ("twemoji-glyf_colr_1.ttf", "twemoji-sbix.ttf"),
            ("noto_flags-glyf_colr_1.ttf", "noto_flags-sbix.ttf"),
            ("noto-picosvgz.ttf", "noto-sbix.ttf"),
            ("twemoji-picosvg.ttf", "twemoji-sbix.ttf"),
        ];
        for (drawn_by, shown_by) in twins {
            let (Some(font), Some(pictures)) = (FontFile::open(folder.join(drawn_by)), FontFile::open(folder.join(shown_by))) else { continue };
            let (mut counted, mut close, mut worst, mut spent) = (0, 0, (0.0f32, 0u16), std::time::Duration::ZERO);
            // The two number their glyphs differently: a glyph is found by its character in
            // each.
            let one = |font: &FontFile, letter: char| {
                let mut glyphs = Vec::new();
                font.set(&letter.to_string(), &mut glyphs).filter(|_| glyphs.len() == 1).map(|_| glyphs[0].glyph)
            };
            for letter in (0x20..0x20000).filter_map(char::from_u32) {
                let (Some(glyph), Some(twin)) = (one(&font, letter), one(&pictures, letter)) else { continue };
                let Some((picture, (width, png))) = pictures.bitmap(twin, 128.0).and_then(|picture| Some((crate::png::decode(picture.png)?, picture)).map(|(png, picture)| (picture, png))) else {
                    continue;
                };
                let started = std::time::Instant::now();
                let Some(mine) = drawn(&font, glyph, font.unit(picture.pixels), [0.0, 0.0], Color::BLACK) else { continue };
                spent += started.elapsed();
                let mean = apart(&mine, &png, width, [picture.left as i32, -picture.top as i32]);
                counted += 1;
                close += (mean < 8.0) as usize;
                worst = if mean > worst.0 { (mean, glyph) } else { worst };
                if let Some(folder) = std::env::var_os("TAUT_PICTURES").filter(|_| mean > 30.0 && glyph % 7 == 0) {
                    let ours: Vec<u32> = (0..png.len()).map(|n| at(&mine, picture.left as i32 + (n % width) as i32, -picture.top as i32 + (n / width) as i32)).collect();
                    std::fs::write(std::path::Path::new(&folder).join(format!("{drawn_by}-{glyph}-{mean:.0}.png")), crate::png::encode_rgba(width, &[ours, png.clone()].concat()).unwrap()).unwrap();
                }
            }
            println!(
                "{drawn_by}: {close} of {counted} glyphs within 8 of 255 on the mean; the worst {:.1} (glyph {}); {:.2} ms a glyph",
                worst.0,
                worst.1,
                spent.as_secs_f64() * 1000.0 / counted.max(1) as f64
            );
            // A flags-only font has no glyph for one character, and is skipped.
            assert!(close * 100 >= counted * 95, "{drawn_by}: {close} of {counted}");
        }
    }

    /// The difference between a drawing and a bitmap (its top left pixel at `corner` of the
    /// line), over white: the mean over every channel of every bitmap pixel, in 255ths.
    fn apart(mine: &Drawn, png: &[u32], width: usize, corner: [i32; 2]) -> f32 {
        let white = |pixel: u32, multiplied: bool| {
            let solid = pixel >> 24;
            [16, 8, 0].map(|shift| if multiplied { (pixel >> shift & 255) + 255 - solid } else { ((pixel >> shift & 255) * solid + 255 * (255 - solid)) / 255 })
        };
        let sum: u64 = png
            .iter()
            .enumerate()
            .map(|(n, theirs)| {
                let ours = white(at(mine, corner[0] + (n % width) as i32, corner[1] + (n / width) as i32), true);
                ours.iter().zip(white(*theirs, false)).map(|(a, b)| a.abs_diff(b) as u64).sum::<u64>()
            })
            .sum();
        sum as f32 / (3 * png.len().max(1)) as f32
    }
}
