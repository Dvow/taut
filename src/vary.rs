//! Variable fonts, and font collections: one file, many faces.
//!
//! A variable font can be drawn at any position along a few axes: weight (`wght`), width
//! (`wdth`), optical size (`opsz`), and whatever else its designer added. `fvar` lists the axes
//! with their minimum, default and maximum, and names some positions ("Bold", "SemiBold
//! Condensed"). The file holds the font at its default, plus deltas for everything that varies:
//! `gvar` for `glyf` outlines, `HVAR` for advances, `MVAR` for vertical metrics, `GDEF` for
//! `GPOS` values, and a `CFF2` charstring for its own numbers, via `blend` (see `cff.rs`).
//!
//! A position is first normalized to -1..1 per axis: -1 at the minimum, 0 at the default, 1 at
//! the maximum, linear on each side. `avar` may then remap each axis (version 1) and let axes
//! move each other (version 2). Deltas apply by regions: per axis a region starts, peaks and
//! ends; it scales fully at its peak and less toward its ends, multiplied over all axes; and a
//! value's delta at a position is the sum of each region's delta times its scalar there. `gvar`
//! need not list every point: an unlisted point is interpolated from the listed ones on either
//! side along its contour (IUP). It also moves four phantom points: the advance's start and
//! end, so a glyph at another position is as much wider as it should be, even without `HVAR`.
//!
//! All of it follows FreeType and HarfBuzz, rounding where they round, and is tested against
//! their rules on hand-made fonts. None of it runs for a font at its default position, which is
//! every font an app does not move.
//!
//! A collection (`.ttc`) holds several fonts sharing tables where they can. One is picked by
//! index ([`FontFile::nth`]) or by name, as a variable font's named instance is
//! ([`FontFile::named`]).
//!
//! Not done: `cvar` (it varies hints, and nothing is hinted here); `VVAR`, for vertical text;
//! `GSUB` `FeatureVariations`; and `COLR` variations, which its own reader handles.
use crate::font::{Bytes, FontFile, first_not, listed};
use crate::geom::{Point, pt};
use std::ops::Range;

/// A font with more axes stays at its default: no font has a tenth as many.
const AXES: usize = 64;
/// An item variation data subtable with more regions varies nothing: real fonts have a few
/// dozen, and this bounds the work per delta.
const REGIONS: usize = 1024;
/// Fonts past this in a collection are ignored.
const FONTS: usize = 1024;
/// The most points times tuple variations one glyph's deltas may take. Past it the glyph is
/// blank: real ones have a few hundred points and a few dozen tuples, and this stops a damaged
/// font from stalling the reader.
const WORK: usize = 1 << 22;

/// One axis of a variable font: its tag (`b"wght"` for weight), and its minimum, default and
/// maximum in its own units (weight is 100 to 900).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    /// Its tag: `wght`, `wdth`, `opsz`, `ital`, `slnt`, or a font's own.
    pub name: [u8; 4],
    /// Its minimum.
    pub least: f32,
    /// Its default.
    pub usual: f32,
    /// Its maximum.
    pub most: f32,
}

/// A variable font at a position: what drawing it there takes.
pub(crate) struct Varied {
    /// The position per axis in the axis's own units, as the app asked: what the next
    /// [`FontFile::at`] starts from.
    asked: Vec<f32>,
    /// The same position normalized to -1..1 per axis, after `avar`, in F2DOT14: what every
    /// variation table uses.
    pub place: Vec<i32>,
    /// The locations of `gvar` and `HVAR`, and `GDEF`'s item variation store.
    points: Range<usize>,
    widths: Range<usize>,
    store: Range<usize>,
    /// What varies outlines, advances and kerning (see `Movers`).
    movers: Movers,
}

/// What varies a variable font's outlines, advances and kerning from their defaults: the bodies
/// of `Varied`'s methods of these names. Handed over when a font is first moved off its default
/// ([`FontFile::at`], [`FontFile::named`]); nothing else reaches it, so a program that never
/// does is built without it.
#[derive(Clone, Copy)]
struct Movers {
    wider: fn(&Varied, Bytes, u16, Option<Bytes>) -> f32,
    left: fn(&Varied, Bytes, u16, Bytes) -> f32,
    moved: MovesPoints,
    device: fn(&Varied, Bytes, Bytes, Option<usize>) -> f32,
}
/// Varies a glyph's points (see `Varied::moved`).
type MovesPoints = fn(&Varied, Bytes, u16, Vec<Point>, &[usize]) -> Option<Vec<Point>>;

/// A font's `fvar`: where its axes are, how many, and each one's size; the same for its named
/// instances.
struct Axes<'a> {
    table: Bytes<'a>,
    at: usize,
    count: usize,
    long: usize,
    places: usize,
    place_long: usize,
}

impl<'a> Axes<'a> {
    /// Reads `fvar`: after its version, the axes' offset, count and size, then the instances'
    /// count and size. An axis is its tag, three Fixed values (minimum, default, maximum), and
    /// two fields unread here.
    fn read(table: Bytes<'a>) -> Option<Axes<'a>> {
        let (at, count, long) = (table.u16(4)? as usize, table.u16(8)? as usize, table.u16(10)? as usize);
        let (places, place_long) = (table.u16(12)? as usize, table.u16(14)? as usize);
        let fits = table.u16(0) == Some(1) && (1..=AXES).contains(&count) && long >= 20 && place_long >= 4 + count * 4;
        fits.then_some(Axes { table, at, count, long, places, place_long })
    }

    /// Axis `n`.
    fn axis(&self, n: usize) -> Option<Axis> {
        let at = self.at + n * self.long;
        let number = |at: usize| self.table.u32(at).map(|number| number as i32 as f32 / 65536.0);
        let (least, usual, most) = (number(at + 4)?, number(at + 8)?, number(at + 12)?);
        // (A default outside its range extends the axis to it.)
        Some(Axis { name: self.table.u32(at)?.to_be_bytes(), least: least.min(usual), usual, most: most.max(usual) })
    }

    /// Named instance `n`: its `name` id, its coordinates, and its PostScript name id (if any).
    fn named(&self, n: usize) -> Option<(u16, Vec<f32>, Option<u16>)> {
        let at = self.at + self.count * self.long + n * self.place_long;
        let place = (0..self.count).map(|axis| self.table.u32(at + 4 + axis * 4).map(|number| number as i32 as f32 / 65536.0)).collect::<Option<_>>()?;
        let postscript = self.table.u16(at + 4 + self.count * 4).filter(|_| self.place_long >= 6 + self.count * 4);
        Some((self.table.u16(at)?, place, postscript.filter(|name| *name != 0xffff)))
    }
}

impl FontFile {
    /// The font's axes, if it is variable: see `at`.
    pub fn axes(&self) -> Vec<Axis> {
        let table = self.bytes().within(&listed(self.bytes(), self.start, b"fvar").unwrap_or_default());
        Axes::read(table).map_or(Vec::new(), |axes| (0..axes.count).map_while(|n| axes.axis(n)).collect())
    }

    /// The font at a position on one of its axes, in the axis's own units: heavier with
    /// `.at(b"wght", 700.0)`, narrower with `.at(b"wdth", 75.0)`. Values past the range clamp;
    /// an axis the font lacks, or a non-variable font, changes nothing. Other axes stay as they
    /// were. Vertical metrics and advances follow.
    pub fn at(mut self, axis: &[u8; 4], value: f32) -> FontFile {
        let table = self.bytes().within(&listed(self.bytes(), self.start, b"fvar").unwrap_or_default());
        let Some(axes) = Axes::read(table) else { return self };
        let Some(n) = (0..axes.count).find(|n| axes.axis(*n).is_some_and(|found| &found.name == axis)) else { return self };
        let usual = || (0..axes.count).map(|n| axes.axis(n).map_or(0.0, |axis| axis.usual)).collect();
        let mut asked: Vec<f32> = self.varied.as_ref().map_or_else(usual, |varied| varied.asked.clone());
        asked[n] = value;
        self.set_at(asked);
        self
    }

    /// The collection font, or variable font instance, with this name: its full name
    /// (`"Cambria Math"`, `"Bahnschrift SemiBold Condensed"`), its PostScript name
    /// (`"Bahnschrift-SemiBoldCondensed"`), or for an instance its subfamily name alone
    /// (`"SemiBold Condensed"`). Case, spaces and hyphens are ignored. Nothing if the file has
    /// no such font. (This font's instances are searched first, then each collection font with
    /// its instances.)
    pub fn named(mut self, name: &str) -> Option<FontFile> {
        let all = self.bytes();
        let starts = std::iter::once(Some(self.start)).chain((0..FONTS).map(|nth| start(all, nth)));
        let found = starts.map_while(|start| start).enumerate().find_map(|(n, start)| Some((n, start, called(all, start, name)?)));
        let (n, start, place) = found?;
        // (The first start is this font's own, and the n-th after it is collection font n - 1.)
        if start != self.start {
            self = self.nth(n - 1)?;
        }
        if let Some(place) = place {
            self.set_at(place);
        }
        Some(self)
    }

    /// The names of the file's fonts and their instances, as `named` takes them: for a font
    /// with instances, each prefixed by the family name; for one without, its full name.
    pub fn faces(&self) -> Vec<String> {
        let all = self.bytes();
        let mut faces = Vec::new();
        for start in (0..FONTS).map_while(|nth| start(all, nth)) {
            let names = Names::of(all, start);
            let table = all.within(&listed(all, start, b"fvar").unwrap_or_default());
            let places = Axes::read(table).map_or(Vec::new(), |axes| (0..axes.places).filter_map(|n| axes.named(n)).collect());
            let family = names.read(&[16, 1]).unwrap_or_default();
            faces.extend(places.iter().filter_map(|(name, ..)| Some(format!("{family} {}", names.read(&[*name])?))));
            faces.extend(names.read(&[4]).filter(|_| places.is_empty()));
        }
        faces
    }

    /// Moves the font to a position, one value per axis in axis units: what it varies by from
    /// then on, and its vertical metrics. At the default nothing varies.
    fn set_at(&mut self, asked: Vec<f32>) {
        let (all, start) = (self.bytes(), self.start);
        let table = |name: &[u8; 4]| listed(all, start, name).unwrap_or_default();
        let Some(axes) = Axes::read(all.within(&table(b"fvar"))) else { return };
        let told = asked.iter().enumerate().map(|(n, value)| axes.axis(n).map_or(0, |axis| told(*value, axis)));
        let place = bent(all.within(&table(b"avar")), told.collect());
        let gdef = all.within(&table(b"GDEF"));
        // (`GDEF` has an item variation store from version 1.3.)
        let store = gdef.u32(14).filter(|at| *at != 0 && gdef.u16(0) == Some(1) && gdef.u16(2) >= Some(3)).map_or(0..0, |at| table(b"GDEF").start + at as usize..table(b"GDEF").end);
        // (A `gvar` not matching the font's axis and glyph counts is ignored, as FreeType
        // does.)
        let (gvar, glyphs) = (all.within(&table(b"gvar")), all.within(&table(b"maxp")).u16(4));
        let fits = gvar.u16(0) == Some(1) && gvar.u16(4) == Some(place.len() as u16) && gvar.u16(12) == glyphs;
        let movers = Movers { wider: Varied::wider_here, left: Varied::left_here, moved: Varied::moved_here, device: Varied::device_here };
        let varied = Varied { asked, place, points: if fits { table(b"gvar") } else { 0..0 }, widths: table(b"HVAR"), store, movers };
        // `hhea` gives the ascender and descender; `MVAR` their deltas.
        let (hhea, measures) = (all.within(&table(b"hhea")), all.within(&table(b"MVAR")));
        let (ascent, descent) = (hhea.i16(4).unwrap_or(0) as f32, hhea.i16(6).unwrap_or(0) as f32);
        (self.ascent, self.descent) = (ascent + varied.measure(measures, b"hasc"), -(descent + varied.measure(measures, b"hdsc")));
        self.varied = varied.place.iter().any(|at| *at != 0).then(|| Box::new(varied));
        self.measure();
    }
}

/// An axis value normalized to -1..1 in F2DOT14, as `fvar` defines it before `avar`: linear
/// from the default to either end, clamped.
fn told(value: f32, axis: Axis) -> i32 {
    let value = value.clamp(axis.least, axis.most);
    let part = match () {
        _ if value < axis.usual => (value - axis.usual) / (axis.usual - axis.least),
        _ if value > axis.usual => (value - axis.usual) / (axis.most - axis.usual),
        _ => 0.0,
    };
    (part * 16384.0).round().clamp(-16384.0, 16384.0) as i32
}

/// A position after `avar`, if the font has one. Version 1 has segment maps per axis, pairs of
/// from and to values, interpolating linearly between pairs. Version 2 then varies each axis by
/// all of them: through an item variation store, each axis with a delta set (its own, or one a
/// map names), as in any variation table.
fn bent(avar: Bytes, mut place: Vec<i32>) -> Vec<i32> {
    let (Some(version @ 1..=2), Some(count)) = (avar.u16(0), avar.u16(6)) else { return place };
    if count as usize != place.len() {
        return place;
    }
    let mut at = 8;
    for told in &mut place {
        let Some(pairs) = avar.u16(at).map(usize::from) else { return place };
        *told = mapped(avar, at + 2, pairs, *told).unwrap_or(*told);
        at += 2 + pairs * 4;
    }
    let (Some(rows), Some(store)) = (avar.u32(at), avar.u32(at + 4)) else { return place };
    if version == 1 || store == 0 {
        return place;
    }
    let store = Bytes(avar.0.get(store as usize..).unwrap_or(&[]));
    // (Every axis varies by the others' values before any of them changed.)
    let moved = (0..place.len()).map(|n| row(avar, rows as usize, n as u32).and_then(|(outer, inner)| delta(store, outer, inner, &place)));
    let moved: Vec<f32> = moved.map(|by| by.unwrap_or(0.0).round()).collect();
    place.iter().zip(moved).map(|(told, by)| (*told + by as i32).clamp(-16384, 16384)).collect()
}

/// Where a value maps by the `count` pairs of an `avar` segment map at `at`, as HarfBuzz does:
/// also for maps that are too short, or have two pairs for one value.
fn mapped(avar: Bytes, at: usize, count: usize, told: i32) -> Option<i32> {
    let pair = |n: usize| Some((avar.i16(at + n * 4)? as i32, avar.i16(at + n * 4 + 2)? as i32));
    if count < 2 {
        return Some(if count == 0 { told } else { told - pair(0)?.0 + pair(0)?.1 });
    }
    if told <= pair(0)?.0 {
        return Some(told - pair(0)?.0 + pair(0)?.1);
    }
    let n = (1..count - 1).find(|n| pair(*n).is_none_or(|(from, _)| told <= from)).unwrap_or(count - 1);
    let ((from, to), (before_from, before_to)) = (pair(n)?, pair(n - 1)?);
    if told >= from {
        return Some(told - from + to);
    }
    if before_from == from {
        return Some(before_to);
    }
    Some((before_to as f32 + (to - before_to) as f32 * (told - before_from) as f32 / (from - before_from) as f32).round() as i32)
}

/// Which delta set of an item variation store the `n`-th item uses, by a DeltaSetIndexMap at
/// `at` (where none is given, set `n` of the first subtable). A map gives how many items it
/// covers (later ones use the last), and each entry in one to four bytes: outer index, then
/// inner, in so many bits.
fn row(table: Bytes, at: usize, n: u32) -> Option<(u16, u16)> {
    if at == 0 {
        return Some(((n >> 16) as u16, n as u16));
    }
    let (format, entry) = (table.u8(at)?, table.u8(at + 1)?);
    let (count, rows) = match format {
        0 => (table.u16(at + 2)? as u32, at + 4),
        1 => (table.u32(at + 2)?, at + 6),
        _ => return None,
    };
    let (long, bits) = (((entry >> 4) & 3) as usize + 1, (entry & 15) as u32 + 1);
    let at = rows + n.min(count.checked_sub(1)?) as usize * long;
    let number = (0..long).try_fold(0u32, |number, k| Some(number << 8 | table.u8(at + k)? as u32))?;
    Some(((number >> bits) as u16, (number & ((1 << bits) - 1)) as u16))
}

/// The regions an item variation data subtable uses: a store starts with its format and region
/// list offset (each region a range per axis), then each subtable's offset. A subtable has its
/// item count, word delta count, region index count, the region indices, then its delta sets.
pub(crate) struct Regions<'a> {
    store: Bytes<'a>,
    list: usize,
    at: usize,
    /// How many regions the subtable uses.
    pub count: usize,
}

impl<'a> Regions<'a> {
    /// The regions of the store's `n`-th subtable.
    pub fn of(store: Bytes<'a>, n: u16) -> Option<Regions<'a>> {
        (store.u16(0)? == 1 && n < store.u16(6)?).then_some(())?;
        let (list, at) = (store.u32(2)? as usize, store.u32(8 + n as usize * 4)? as usize);
        let count = store.u16(at + 4).map(usize::from).filter(|count| *count <= REGIONS)?;
        Some(Regions { store, list, at, count })
    }

    /// The scalar of the subtable's `n`-th region at a position: per axis 1 at its peak,
    /// falling to 0 at its start and end, 0 beyond; multiplied over all axes. An axis whose
    /// peak is 0, or whose range is out of order, does not limit it.
    pub fn say(&self, n: usize, place: &[i32]) -> f32 {
        let (store, axes) = (self.store, self.store.u16(self.list).unwrap_or(0) as usize);
        let region = self.store.u16(self.at + 6 + n * 2).filter(|region| Some(*region) < store.u16(self.list + 2));
        let Some(region) = region.map(|region| self.list + 4 + region as usize * axes * 6) else { return 0.0 };
        let mut say = 1.0;
        for axis in 0..axes.min(AXES) {
            let number = |at: usize| store.i16(region + axis * 6 + at).map_or(0, i32::from);
            let ([start, peak, end], at) = ([number(0), number(2), number(4)], place.get(axis).copied().unwrap_or(0));
            say *= match () {
                _ if peak == 0 || at == peak || start > peak || peak > end || (start < 0 && end > 0) => 1.0,
                _ if at <= start || at >= end => 0.0,
                _ if at < peak => (at - start) as f32 / (peak - start) as f32,
                _ => (end - at) as f32 / (end - peak) as f32,
            };
        }
        say
    }

    /// Delta set `row`'s deltas, each with its region: some in two bytes and the rest one (or
    /// four and two, with long words).
    fn moves(&self, row: u16) -> Option<impl Iterator<Item = (usize, f32)> + '_> {
        let (store, at) = (self.store, self.at);
        let (rows, words) = (store.u16(at)?, store.u16(at + 2)?);
        let (long, words) = (words & 0x8000 != 0, (words & 0x7fff) as usize);
        let (wide, narrow) = if long { (4, 2) } else { (2, 1) };
        (row < rows && words <= self.count).then_some(())?;
        let first = at + 6 + self.count * 2 + row as usize * (words * wide + (self.count - words) * narrow);
        let number = move |n: usize| match (n < words, long) {
            (true, true) => store.u32(first + n * 4).map(|number| number as i32 as f32),
            (true, false) => store.i16(first + n * 2).map(f32::from),
            (false, true) => store.i16(first + words * 4 + (n - words) * 2).map(f32::from),
            (false, false) => store.u8(first + words * 2 + n - words).map(|number| number as i8 as f32),
        };
        Some((0..self.count).map_while(move |n| Some((n, number(n)?))))
    }
}

/// The delta for the item at `inner` in subtable `outer` of a store, at a position.
pub(crate) fn delta(store: Bytes, outer: u16, inner: u16, place: &[i32]) -> Option<f32> {
    let regions = Regions::of(store, outer)?;
    Some(regions.moves(inner)?.filter(|(_, by)| *by != 0.0).map(|(n, by)| by * regions.say(n, place)).sum())
}

impl Varied {
    /// `MVAR`'s delta for one of the font's metrics, in whole units. Its records are sorted by
    /// tag, eight bytes each: the tag and its delta set indices.
    fn measure(&self, table: Bytes, called: &[u8; 4]) -> f32 {
        let (long, count, store) = (table.u16(6).map_or(0, usize::from), table.u16(8).map_or(0, usize::from), table.u16(10).map_or(0, usize::from));
        let wanted = u32::from_be_bytes(*called);
        let n = first_not(count, |n| Some(table.u32(12 + n * long)? < wanted)).filter(|n| *n < count && long >= 8 && store != 0);
        let at = n.map(|n| 12 + n * long).filter(|at| table.u32(*at) == Some(wanted));
        let store = Bytes(table.0.get(store..).unwrap_or(&[]));
        at.and_then(|at| delta(store, table.u16(at + 4)?, table.u16(at + 6)?, &self.place)).map_or(0.0, f32::round)
    }

    /// The extra advance after a glyph, in whole units: by `HVAR` where the font has it, else
    /// by `gvar`'s phantom points (`points` are the glyph's `glyf` bytes).
    pub fn wider(&self, all: Bytes, glyph: u16, points: Option<Bytes>) -> f32 {
        (self.movers.wider)(self, all, glyph, points)
    }
    /// How far right the left phantom point of a `glyf` glyph moves: the glyph is drawn that
    /// much further left, so it starts at the pen. (Not with `HVAR`, where, as in FreeType,
    /// that point stays.)
    pub fn left(&self, all: Bytes, glyph: u16, points: Bytes) -> f32 {
        (self.movers.left)(self, all, glyph, points)
    }
    /// Each point's delta for a `glyf` glyph at this position, plus the four phantom points
    /// after them (advance start and end, top and bottom). `original` are the glyph's points as
    /// stored (for a composite, one per component), and `lasts` each contour's last point.
    /// Nothing for a glyph that does not vary, or whose deltas cost more than any real glyph's.
    pub fn moved(&self, all: Bytes, glyph: u16, original: impl Iterator<Item = Point>, lasts: impl Iterator<Item = usize>) -> Option<Vec<Point>> {
        let (original, lasts): (Vec<Point>, Vec<usize>) = (original.chain([pt(0.0, 0.0); 4]).collect(), lasts.collect());
        (self.movers.moved)(self, all, glyph, original, &lasts)
    }
    /// A `GPOS` value's delta from the device table at `at` in `list` (0: none), in whole
    /// units.
    pub fn device(&self, all: Bytes, list: Bytes, at: Option<usize>) -> f32 {
        (self.movers.device)(self, all, list, at)
    }

    /// The body of `wider`.
    fn wider_here(&self, all: Bytes, glyph: u16, points: Option<Bytes>) -> f32 {
        if self.widths.is_empty() {
            let ends = points.and_then(|points| self.ends(all, glyph, points));
            return ends.map_or(0.0, |[start, end]| (end - start).round());
        }
        // `HVAR` has its store's offset, then an advance mapping's offset (none: each glyph's
        // own index in the first subtable).
        let table = all.within(&self.widths);
        let store = table.u32(4).and_then(|at| table.0.get(at as usize..)).map_or(Bytes(&[]), Bytes);
        let found = row(table, table.u32(8).unwrap_or(0) as usize, glyph as u32).and_then(|(outer, inner)| delta(store, outer, inner, &self.place));
        found.map_or(0.0, f32::round)
    }

    /// The body of `left`.
    fn left_here(&self, all: Bytes, glyph: u16, points: Bytes) -> f32 {
        let ends = self.widths.is_empty().then(|| self.ends(all, glyph, points)).flatten();
        ends.map_or(0.0, |[start, _]| start)
    }

    /// How far right `gvar` moves a glyph's two horizontal phantom points.
    fn ends(&self, all: Bytes, glyph: u16, points: Bytes) -> Option<[f32; 2]> {
        // (An empty glyph, a space, has no bytes but still its four phantom points.)
        let count = match points.i16(0) {
            Some(outlines @ 1..) => points.u16(10 + (outlines as usize - 1) * 2)? as usize + 1,
            None | Some(0) => 0,
            Some(_) => parts(points)?,
        };
        let moved = self.moved_here(all, glyph, vec![pt(0.0, 0.0); count + 4], &[])?;
        Some([moved.get(count)?.x, moved.get(count + 1)?.x])
    }

    /// The body of `moved`, for points already followed by the four phantom points.
    ///
    /// `gvar` holds, after a header, shared peak tuples and each glyph's data offset. A glyph's
    /// data is a list of tuple variations, each with its size, its peak (shared or embedded),
    /// and maybe its intermediate start and end. Their data follows: first the shared point
    /// numbers, if flagged, then each tuple's own point numbers (unless shared), x deltas, and
    /// y deltas.
    fn moved_here(&self, all: Bytes, glyph: u16, original: Vec<Point>, lasts: &[usize]) -> Option<Vec<Point>> {
        let (table, mut moved) = (all.within(&self.points), vec![pt(0.0, 0.0); original.len()]);
        // (A font without `gvar`, or a glyph without data, varies nothing.)
        if table.0.is_empty() {
            return Some(moved);
        }
        let (axes, shared, long, array) = (self.place.len(), table.u32(8).unwrap_or(0) as usize, table.u16(14).unwrap_or(0) & 1 != 0, table.u32(16).unwrap_or(0) as usize);
        let start = |glyph: usize| if long { table.u32(20 + glyph * 4).map(|at| at as usize) } else { table.u16(20 + glyph * 2).map(|at| at as usize * 2) };
        let data = Bytes(table.0.get(array + start(glyph as usize)?..array + start(glyph as usize + 1)?)?);
        if data.0.is_empty() {
            return Some(moved);
        }
        let (lists, mut at) = (data.u16(0)?, data.u16(2)? as usize);
        // (Shared points come first. A tuple without private points where none are shared moves
        // nothing, as in FreeType.)
        let shared_points = if lists & 0x8000 != 0 { numbers(data, &mut at)? } else { Some(Vec::new()) };
        let (mut head, mut work) = (4, 0);
        for _ in 0..lists & 0x0fff {
            let (long, index) = (data.u16(head)? as usize, data.u16(head + 2)?);
            // Its peak is embedded or a shared tuple; it may give an intermediate range.
            let common = Some(index & 0x0fff).filter(|nth| Some(*nth) < table.u16(6)).and_then(|nth| table.part(shared + nth as usize * axes * 2, axes * 2));
            let peak = if index & 0x8000 != 0 { data.part(head + 4, axes * 2)? } else { common? };
            let after = head + 4 + if index & 0x8000 != 0 { axes * 2 } else { 0 };
            let ends = if index & 0x4000 != 0 { Some((data.part(after, axes * 2)?, data.part(after + axes * 2, axes * 2)?)) } else { None };
            let say = say(peak, ends, &self.place);
            let mut list = at..at.checked_add(long)?;
            (head, at) = (after + if ends.is_some() { axes * 4 } else { 0 }, list.end);
            if say == 0.0 {
                continue;
            }
            // (A tuple's data is read from its own bytes only.)
            let (own, named) = (Bytes(data.0.get(..list.end)?), index & 0x2000 != 0);
            let named = if named { numbers(own, &mut list.start)? } else { shared_points.clone() };
            work += original.len();
            (work <= WORK).then_some(())?;
            let by = moves(own, &mut list.start, named.as_ref().map_or(original.len(), Vec::len), &original, lasts, named.as_deref())?;
            moved.iter_mut().zip(by).for_each(|(moved, by)| *moved = pt(moved.x + by.x * say, moved.y + by.y * say));
        }
        Some(moved)
    }

    /// The body of `device`. A VariationIndex table names its delta set in `GDEF`'s store,
    /// marked by format `0x8000`. (Other device tables are for hinting, and skipped.)
    fn device_here(&self, all: Bytes, list: Bytes, at: Option<usize>) -> f32 {
        let table = at.and_then(|at| list.u16(at)).filter(|at| *at != 0).map(usize::from);
        let row = table.filter(|at| list.u16(at + 4) == Some(0x8000)).and_then(|at| Some((list.u16(at)?, list.u16(at + 2)?)));
        let store = all.within(&self.store);
        row.and_then(|(outer, inner)| delta(store, outer, inner, &self.place)).map_or(0.0, f32::round)
    }
}

/// A composite glyph's component count: each component has flags, a glyph id, an offset (two or
/// four bytes by a flag), and a transform (zero to eight bytes by three flags); a flag says
/// whether another follows.
pub(crate) fn parts(points: Bytes) -> Option<usize> {
    let (mut at, mut count) = (10, 0);
    loop {
        let flags = points.u16(at)?;
        let stretched = [(0x80, 8), (0x40, 4), (0x08, 2)].iter().find(|(flag, _)| flags & flag != 0).map_or(0, |(_, long)| *long);
        (at, count) = (at + 4 + [2, 4][(flags & 1) as usize] + stretched, count + 1);
        if flags & 0x20 == 0 || count >= u16::MAX as usize {
            return Some(count);
        }
    }
}

/// A `gvar` tuple's scalar at a position: as a region's, except a tuple without an intermediate
/// range implies one from 0 to its peak.
fn say(peak: Bytes, ends: Option<(Bytes, Bytes)>, place: &[i32]) -> f32 {
    let mut say = 1.0;
    for (axis, at) in place.iter().copied().enumerate() {
        let number = |of: Bytes| of.i16(axis * 2).map_or(0, i32::from);
        let (top, [start, end]) = (number(peak), ends.map_or([0; 2], |(start, end)| [number(start), number(end)]));
        say *= match ends {
            _ if top == 0 || at == top => 1.0,
            Some(_) if start > top || top > end || (start < 0 && end > 0) => 1.0,
            Some(_) if at < start || at > end => return 0.0,
            Some(_) if at < top && top != start => (at - start) as f32 / (top - start) as f32,
            Some(_) if at > top && top != end => (end - at) as f32 / (end - top) as f32,
            Some(_) => 1.0,
            None if at == 0 || at < top.min(0) || at > top.max(0) => return 0.0,
            None => at as f32 / top as f32,
        };
    }
    say
}

/// Packed point numbers from `at`: a count (one byte, or two with the high bit set), then runs,
/// each with its length and byte width, every number a delta from the previous. A count of zero
/// means all points.
fn numbers(data: Bytes, at: &mut usize) -> Option<Option<Vec<usize>>> {
    let first = data.u8(*at)? as usize;
    let count = if first & 0x80 != 0 { (first & 0x7f) << 8 | data.u8(*at + 1)? as usize } else { first };
    *at += if first & 0x80 != 0 { 2 } else { 1 };
    if first == 0 {
        return Some(None);
    }
    let (mut numbers, mut number) = (Vec::with_capacity(count), 0usize);
    while numbers.len() < count {
        let run = data.u8(*at)?;
        *at += 1;
        for _ in 0..((run & 0x7f) as usize + 1).min(count - numbers.len()) {
            let step = if run & 0x80 != 0 { data.u16(*at)? as usize } else { data.u8(*at)? as usize };
            (number, *at) = (number + step, *at + if run & 0x80 != 0 { 2 } else { 1 });
            numbers.push(number);
        }
    }
    Some(Some(numbers))
}

/// `count` packed deltas from `at`: in runs, each with its length and whether zeros (not
/// stored), bytes, words, or longs.
fn steps(data: Bytes, at: &mut usize, count: usize) -> Option<Vec<f32>> {
    let mut steps = Vec::with_capacity(count);
    while steps.len() < count {
        let run = data.u8(*at)?;
        let (many, long) = (((run & 0x3f) as usize + 1).min(count - steps.len()), [1, 2, 0, 4][(run >> 6) as usize]);
        let at_first = *at + 1;
        *at = at_first + many * long;
        for n in 0..many {
            let step = match long {
                0 => Some(0.0),
                1 => data.u8(at_first + n).map(|step| step as i8 as f32),
                2 => data.i16(at_first + n * 2).map(f32::from),
                _ => data.u32(at_first + n * 4).map(|step| step as i32 as f32),
            };
            steps.push(step?);
        }
    }
    Some(steps)
}

/// Each point's delta from one `gvar` tuple, read from `at`: its own deltas, `count` x then as
/// many y, for the points `named` (all, if none). An unlisted point on a contour is
/// interpolated from the listed ones before and after it on that contour, x and y separately.
fn moves(data: Bytes, at: &mut usize, count: usize, original: &[Point], lasts: &[usize], named: Option<&[usize]>) -> Option<Vec<Point>> {
    let (across, up) = (steps(data, at, count)?, steps(data, at, count)?);
    let Some(named) = named else { return Some(across.into_iter().zip(up).map(|(x, y)| pt(x, y)).collect()) };
    let (mut moved, mut known) = (vec![pt(0.0, 0.0); original.len()], vec![false; original.len()]);
    for (n, (x, y)) in named.iter().zip(across.into_iter().zip(up)) {
        if let Some(point) = moved.get_mut(*n) {
            (*point, known[*n]) = (pt(point.x + x, point.y + y), true);
        }
    }
    let mut first = 0;
    for last in lasts.iter().copied() {
        if last < first || last + 4 >= original.len() {
            continue;
        }
        let outline = first..last + 1;
        let marks: Vec<usize> = outline.clone().filter(|n| known[*n]).collect();
        first = last + 1;
        for (n, mark) in marks.iter().enumerate() {
            let next = marks[(n + 1) % marks.len()];
            let mut point = if *mark == last { outline.start } else { mark + 1 };
            while point != next {
                moved[point] = between(original, &moved, point, [*mark, next]);
                point = if point == last { outline.start } else { point + 1 };
            }
        }
    }
    Some(moved)
}

/// The delta of a point `gvar` does not list, from the listed points `[before, after]` around
/// it: per axis, the nearer one's where it lies outside both, else proportional to its position
/// between them. (Where the two coincide but move apart, it does not move along that axis.)
fn between(original: &[Point], moved: &[Point], point: usize, [before, after]: [usize; 2]) -> Point {
    let one = |at: fn(Point) -> f32| {
        let (a, b) = ((at(original[before]), at(moved[before])), (at(original[after]), at(moved[after])));
        let ((low, low_by), (high, high_by)) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        let here = at(original[point]);
        match () {
            _ if low == high && low_by != high_by => 0.0,
            _ if here <= low => low_by,
            _ if here >= high => high_by,
            _ => low_by + (here - low) * (high_by - low_by) / (high - low),
        }
    };
    pt(one(|point| point.x), one(|point| point.y))
}

/// Where the `nth` font of a file starts: a collection has its tag, version, font count and
/// each offset; any other file is one font at its start.
pub(crate) fn start(all: Bytes, nth: usize) -> Option<usize> {
    if all.u32(0)? != u32::from_be_bytes(*b"ttcf") {
        return (nth == 0).then_some(0);
    }
    (nth < all.u32(8)? as usize).then_some(())?;
    all.u32(12 + nth * 4).map(|at| at as usize)
}

/// Whether the font at `start` is named `name`, or one of its instances is: nothing if neither;
/// else, for an instance, its coordinates.
fn called(all: Bytes, start: usize, name: &str) -> Option<Option<Vec<f32>>> {
    let names = Names::of(all, start);
    let family = [16, 1];
    if names.is(&[4], None, name) || names.is(&[6], None, name) || names.is(&family, Some(&[17, 2]), name) {
        return Some(None);
    }
    let axes = Axes::read(all.within(&listed(all, start, b"fvar")?))?;
    let place = (0..axes.places).filter_map(|n| axes.named(n)).find(|(own, _, postscript)| {
        let by_itself = names.is(&[*own], None, name) || postscript.is_some_and(|postscript| names.is(&[postscript], None, name));
        by_itself || names.is(&family, Some(&[*own]), name)
    });
    place.map(|(_, place, _)| Some(place))
}

/// A font's `name` table: record count, string storage offset, then twelve bytes per record:
/// platform, encoding and language, name id, length and offset.
struct Names<'a> {
    table: Bytes<'a>,
}

impl<'a> Names<'a> {
    fn of(all: Bytes<'a>, start: usize) -> Names<'a> {
        Names { table: all.within(&listed(all, start, b"name").unwrap_or_default()) }
    }

    /// Every string of the first of the name ids `numbers` the font has: Windows Unicode as
    /// UTF-16BE; old Macintosh as single bytes (read as ASCII). Windows English first.
    fn all(&self, numbers: &[u16]) -> Vec<String> {
        let (table, count, words) = (self.table, self.table.u16(2).unwrap_or(0) as usize, self.table.u16(4).unwrap_or(0) as usize);
        let record = |n: usize| Some((table.u16(6 + n * 12)?, table.u16(8 + n * 12)?, table.u16(10 + n * 12)?, table.u16(12 + n * 12)?, n));
        let records: Vec<_> = (0..count).filter_map(record).collect();
        let Some(number) = numbers.iter().find(|number| records.iter().any(|record| record.3 == **number)) else { return Vec::new() };
        let mut found: Vec<_> = records.into_iter().filter(|record| record.3 == *number).collect();
        found.sort_by_key(|(system, _, language, ..)| (*system != 3, *language != 0x409));
        let text = |(system, way, _, _, n): (u16, u16, u16, u16, usize)| {
            let bytes = table.part(words + table.u16(16 + n * 12)? as usize, table.u16(14 + n * 12)? as usize)?.0;
            match (system, way) {
                (0, _) | (3, 0 | 1 | 10) => char::decode_utf16(bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair))).collect::<Result<String, _>>().ok(),
                (1, 0) => Some(bytes.iter().map(|byte| if byte.is_ascii() { *byte as char } else { '\u{fffd}' }).collect()),
                _ => None,
            }
        };
        found.into_iter().filter_map(text).collect()
    }

    /// The first string of the first of these name ids, if the font has one.
    fn read(&self, numbers: &[u16]) -> Option<String> {
        self.all(numbers).into_iter().next()
    }

    /// Whether `name` is one of name ids `first`, followed by one of `then` if given, comparing
    /// letters and digits only, case-insensitively.
    fn is(&self, first: &[u16], then: Option<&[u16]>, name: &str) -> bool {
        let plain = |text: &str| text.chars().filter(|letter| letter.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
        let (wanted, firsts, thens) = (plain(name), self.all(first), then.map_or(vec![String::new()], |then| self.all(then)));
        !wanted.is_empty() && firsts.iter().any(|first| thens.iter().any(|then| plain(first) + &plain(then) == wanted))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::canvas::Canvas;
    use crate::color::Color;
    use crate::font::OwnFonts;
    use crate::font::tests::{Tables, by_seconds, file, glyph, longs, made_of, positions, tables_of, words};
    use crate::path::Path;
    use crate::text::{Font, Metrics, TextSystem};

    /// Fixed values: axis coordinates, as `fvar` stores them.
    fn fixed(numbers: &[f32]) -> Vec<u8> {
        numbers.iter().flat_map(|number| ((number * 65536.0) as i32).to_be_bytes()).collect()
    }

    /// A named instance: its name id, its coordinates, and its PostScript name id (0xffff:
    /// none).
    type Place<'a> = (i32, &'a [f32], i32);

    /// An `fvar` table: its axes, each a tag with minimum, default and maximum; and its
    /// instances.
    pub(crate) fn axes(axes: &[(&[u8; 4], [f32; 3])], places: &[Place]) -> Vec<u8> {
        let mut table = words(&[1, 0, 16, 2, axes.len() as i32, 20, places.len() as i32, 6 + axes.len() as i32 * 4]);
        for (name, ends) in axes {
            table.extend([&name[..], &fixed(ends), &words(&[0, 256])].concat());
        }
        for (name, place, postscript) in places {
            table.extend([words(&[*name, 0]), fixed(place), words(&[*postscript])].concat());
        }
        table
    }

    /// An item variation data subtable: its region indices, how many have word deltas, and its
    /// delta sets.
    pub(crate) type Rows<'a> = (&'a [i32], usize, &'a [&'a [i32]]);

    /// An item variation store: its regions, each a range per axis (start, peak, end, in
    /// F2DOT14); and its subtables, each with region indices, word delta count (the rest are
    /// bytes), and delta sets.
    pub(crate) fn store(regions: &[&[[i32; 3]]], lists: &[Rows]) -> Vec<u8> {
        let axes = regions.first().map_or(0, |region| region.len());
        let regions = [words(&[axes as i32, regions.len() as i32]), words(&regions.concat().concat())].concat();
        let row = |wide: usize, row: &[i32]| -> Vec<u8> { row.iter().enumerate().flat_map(|(n, by)| if n < wide { words(&[*by]) } else { vec![*by as i8 as u8] }).collect() };
        let lists: Vec<Vec<u8>> = lists
            .iter()
            .map(|(which, wide, rows)| [words(&[rows.len() as i32, *wide as i32, which.len() as i32]), words(which), rows.iter().flat_map(|one| row(*wide, one)).collect()].concat())
            .collect();
        let head = 8 + lists.len() * 4;
        let starts: Vec<u32> = (0..lists.len()).map(|n| (head + regions.len() + lists[..n].concat().len()) as u32).collect();
        [words(&[1]), longs(&[head as u32]), words(&[lists.len() as i32]), longs(&starts), regions, lists.concat()].concat()
    }

    /// A `name` table with Windows English names: each id and string.
    pub(crate) fn names(names: &[(i32, &str)]) -> Vec<u8> {
        let texts: Vec<Vec<u8>> = names.iter().map(|(_, text)| text.encode_utf16().flat_map(u16::to_be_bytes).collect()).collect();
        let mut table = words(&[0, names.len() as i32, 6 + names.len() as i32 * 12]);
        for (n, (number, _)) in names.iter().enumerate() {
            table.extend(words(&[3, 1, 0x409, *number, texts[n].len() as i32, texts[..n].concat().len() as i32]));
        }
        [table, texts.concat()].concat()
    }

    /// A `gvar` tuple variation, as the tests write one.
    #[derive(Clone, Copy)]
    pub(crate) struct Moves<'a> {
        /// Its peak: a shared tuple's index, or embedded.
        peak: Result<usize, &'a [i32]>,
        /// Its intermediate range, if any.
        ends: Option<[&'a [i32]; 2]>,
        /// The points it lists: none for the glyph's shared ones; empty for all points.
        points: Option<&'a [usize]>,
        across: &'a [i32],
        up: &'a [i32],
    }

    /// Packed point numbers: the count in two bytes, then runs of word numbers. (Empty is
    /// written as zero, meaning all points.)
    fn point_numbers(points: &[usize]) -> Vec<u8> {
        if points.is_empty() {
            return vec![0];
        }
        let steps: Vec<i32> = points.iter().scan(0, |last, point| Some(*point as i32 - std::mem::replace(last, *point as i32))).collect();
        let mut bytes = vec![0x80 | (points.len() >> 8) as u8, points.len() as u8];
        for run in steps.chunks(128) {
            bytes.extend([&[0x80 | (run.len() - 1) as u8][..], &words(run)].concat());
        }
        bytes
    }

    /// Packed deltas: runs of zeros, bytes, or words.
    fn packed(moves: &[i32]) -> Vec<u8> {
        let kind = |by: i32| match by {
            0 => 0,
            -128..=127 => 1,
            _ => 2,
        };
        let mut bytes = Vec::new();
        for run in moves.chunk_by(|a, b| kind(*a) == kind(*b)).flat_map(|run| run.chunks(64)) {
            match kind(run[0]) {
                0 => bytes.push(0x80 | (run.len() - 1) as u8),
                1 => bytes.extend([&[(run.len() - 1) as u8][..], &run.iter().map(|by| *by as i8 as u8).collect::<Vec<u8>>()].concat()),
                _ => bytes.extend([&[0x40 | (run.len() - 1) as u8][..], &words(run)].concat()),
            }
        }
        bytes
    }

    /// One glyph's variation data: its tuples, and their shared points, if any.
    fn glyph_moves(axes: usize, shared: Option<&[usize]>, lists: &[Moves]) -> Vec<u8> {
        if lists.is_empty() {
            return Vec::new();
        }
        let (mut heads, mut data) = (Vec::new(), shared.map_or(Vec::new(), point_numbers));
        for list in lists {
            let own = [list.points.map_or(Vec::new(), point_numbers), packed(list.across), packed(list.up)].concat();
            let index = list.peak.map_or(0x8000, |nth| nth as i32) | if list.ends.is_some() { 0x4000 } else { 0 } | if list.points.is_some() { 0x2000 } else { 0 };
            heads.extend(words(&[own.len() as i32, index]));
            heads.extend(list.peak.map_or_else(words, |_| Vec::new()));
            heads.extend(list.ends.map_or(Vec::new(), |[start, end]| [words(start), words(end)].concat()));
            data.extend(own);
        }
        assert!(lists.iter().all(|list| list.peak.map_or_else(|peak| peak.len(), |_| axes) == axes));
        let count = lists.len() as i32 | if shared.is_some() { 0x8000 } else { 0 };
        [words(&[count, 4 + heads.len() as i32]), heads, data].concat()
    }

    /// A `gvar` table: axis count, shared tuples, and each glyph's variation data.
    pub(crate) fn moves(axes: usize, common: &[&[i32]], glyphs: &[(Option<&[usize]>, Vec<Moves>)]) -> Vec<u8> {
        let data: Vec<Vec<u8>> = glyphs.iter().map(|(shared, lists)| glyph_moves(axes, *shared, lists)).collect();
        let starts: Vec<u32> = (0..=data.len()).map(|n| data[..n].concat().len() as u32).collect();
        let (common_at, common) = (20 + starts.len() * 4, words(&common.concat()));
        let head = [words(&[1, 0, axes as i32, (common.len() / 2 / axes.max(1)) as i32]), longs(&[common_at as u32]), words(&[glyphs.len() as i32, 1])].concat();
        [head, longs(&[(common_at + common.len()) as u32]), longs(&starts), common, data.concat()].concat()
    }

    /// The test font's axes: weight 100 to 900, default 400; width 50 to 100, default 100. And
    /// its named instances.
    const AXES: [(&[u8; 4], [f32; 2 + 1]); 2] = [(b"wght", [100.0, 400.0, 900.0]), (b"wdth", [50.0, 100.0, 100.0])];
    const PLACES: [Place; 2] = [(256, &[775.0, 100.0], 0xffff), (257, &[100.0, 50.0], 258)];

    /// The test font's glyphs: notdef box; "A", a row of six points; "B", a square; "C", the
    /// two, "B" shifted right; space; and "D" to "H", the box again.
    fn shapes() -> Vec<Vec<u8>> {
        let on = |points: &[(i32, i32)]| points.iter().map(|(x, y)| (*x, *y, true)).collect::<Vec<_>>();
        let square = glyph(&[&on(&[(64, 0), (64, 448), (256, 448), (256, 0)])]);
        let a = glyph(&[&on(&[(0, 0), (100, 0), (200, 0), (200, 200), (100, 200), (0, 200)])]);
        let b = glyph(&[&on(&[(0, 0), (0, 300), (300, 300), (300, 0)])]);
        let c = made_of(64, &[(1, [64, 64], [1.0, 0.0, 0.0, 1.0], 0), (2, [300, 0], [1.0, 0.0, 0.0, 1.0], 0)]);
        [vec![square.clone(), a, b, c, Vec::new()], vec![square; 5]].concat()
    }

    /// The test font's glyph variations. At maximum weight (shared tuple 0): "A" widens at its
    /// corners, its middle points interpolated, and advances 100 more; "B" widens on the right;
    /// "C" has its "B" shifted right by 40 and advances as much more; the space advances 64
    /// more; "D" has its left phantom point moved left, so the box draws further right and
    /// advances more; and "H" grows taller. At minimum width "A" is narrower. Halfway to
    /// maximum weight "B" rises, less so on either side.
    fn gvar() -> Vec<u8> {
        let most: Result<usize, &[i32]> = Ok(0);
        let moving = |points: Option<&'static [usize]>, across: &'static [i32], up: &'static [i32]| Moves { peak: most, ends: None, points, across, up };
        let narrower = Moves { peak: Err(&[0, -16384]), ends: None, points: Some(&[]), across: &[0, -50, -100, -100, -50, 0, 0, -100, 0, 0], up: &[0; 10] };
        let higher = Moves { peak: Err(&[8192, 0]), ends: Some([&[0, 0], &[16384, 0]]), points: None, across: &[0, 0], up: &[40, 40] };
        let glyphs: Vec<(Option<&[usize]>, Vec<Moves>)> = vec![
            (None, vec![]),
            (None, vec![moving(Some(&[0, 2, 3, 5, 7]), &[-50, 50, 50, -50, 100], &[0; 5]), narrower]),
            (Some(&[1, 2]), vec![higher, moving(None, &[0, 30], &[0, 0])]),
            (None, vec![moving(Some(&[]), &[0, 40, 0, 40, 0, 0], &[0; 6])]),
            (None, vec![moving(Some(&[]), &[0, 64, 0, 0], &[0; 4])]),
            (None, vec![moving(Some(&[4]), &[-30], &[0])]),
            (None, vec![]),
            (None, vec![]),
            (None, vec![]),
            (None, vec![moving(Some(&[]), &[0; 8], &[0, 52, 52, 0, 0, 0, 0, 0])]),
        ];
        moves(2, &[&[16384, 0]], &glyphs)
    }

    /// The test font: ten glyphs (see `shapes`), variable along two axes, plus these tables.
    pub(crate) fn variable(more: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let named = names(&[(1, "Test Sans"), (2, "Regular"), (4, "Test Sans"), (6, "TestSans-Regular"), (256, "Bold"), (257, "Thin Narrow"), (258, "TestSans-ThinNarrow")]);
        let mut tables: Tables = [tables_of(shapes()), vec![(*b"fvar", axes(&AXES, &PLACES)), (*b"gvar", gvar()), (*b"name", named)]].concat();
        tables.extend(more.iter().map(|(name, table)| (**name, table.clone())));
        // (Sorted by tag, as HarfBuzz requires.)
        tables.sort_by_key(|(name, _)| *name);
        file(&[0, 1, 0, 0], &tables)
    }

    /// A character's outline in font units, as closed point rows (x right, y up from the
    /// baseline).
    fn drawn(font: &FontFile, letter: char) -> Path {
        font.sketch(font.glyph(letter), crate::font::Placing([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])).path
    }

    /// Closed point rows, as a path.
    fn polygon(rows: &[&[[f32; 2]]]) -> Path {
        let mut path = Path::new();
        for row in rows {
            path.move_to(row[0][0], row[0][1]);
            row[1..].iter().for_each(|[x, y]| _ = path.line_to(*x, *y));
            path.close();
        }
        path
    }

    /// The font at so much along each of the two axes.
    fn at(file: &[u8], wght: f32, wdth: f32) -> FontFile {
        FontFile::read(file.to_vec()).expect("the font is read").at(b"wght", wght).at(b"wdth", wdth)
    }

    #[test]
    fn a_glyph_moves_as_gvar_says_wherever_the_font_is_set() {
        let file = variable(&[]);
        let usual = FontFile::read(file.clone()).expect("the font is read");
        let a = |points: [f32; 6]| polygon(&[&[[points[0], 0.0], [points[1], 0.0], [points[2], 0.0], [points[3], 200.0], [points[4], 200.0], [points[5], 200.0]]]);
        assert_eq!((drawn(&usual, 'A'), usual.advance('A', 1024.0)), (a([0.0, 100.0, 200.0, 200.0, 100.0, 0.0]), 448.0));
        // At maximum weight its corners move out by 50 and the points between by their
        // interpolated share; and it advances 100 more.
        let most = at(&file, 900.0, 100.0);
        assert_eq!((drawn(&most, 'A'), most.advance('A', 1024.0)), (a([-50.0, 100.0, 250.0, 250.0, 100.0, -50.0]), 548.0));
        assert_eq!(drawn(&at(&file, 650.0, 100.0), 'A'), a([-25.0, 100.0, 225.0, 225.0, 100.0, -25.0]), "halfway");
        // Past an axis's end is its end; a lighter weight varies nothing here.
        assert_eq!(drawn(&at(&file, 2000.0, 100.0), 'A'), drawn(&most, 'A'));
        assert_eq!(drawn(&at(&file, 100.0, 120.0), 'A'), drawn(&usual, 'A'));
        // At minimum width every point is listed, and it advances 100 less.
        let narrow = at(&file, 400.0, 50.0);
        assert_eq!((drawn(&narrow, 'A'), narrow.advance('A', 1024.0)), (a([0.0, 50.0, 100.0, 100.0, 50.0, 0.0]), 348.0));
        assert_eq!(drawn(&at(&file, 900.0, 50.0), 'A'), a([-50.0, 50.0, 150.0, 150.0, 50.0, -50.0]), "both");
        // "B" peaks halfway (rising by 40 there, as its two listed points do) and scales down
        // toward its ends; at maximum weight its right side moves 30 right, the corner below
        // following the listed one above.
        let b = |up: f32, right: f32| polygon(&[&[[0.0, up], [0.0, 300.0 + up], [300.0 + right, 300.0 + up], [300.0 + right, up]]]);
        let b_at = |wght: f32| drawn(&at(&file, wght, 100.0), 'B');
        assert_eq!([b_at(400.0), b_at(525.0), b_at(650.0), b_at(900.0)], [b(0.0, 0.0), b(20.0, 7.5), b(40.0, 15.0), b(0.0, 30.0)]);
        // "C" has its "B" shifted 40 right and advances as much more; the space advances 64
        // more; "D" draws 30 further right and is as much wider.
        let c = polygon(&[&[[14.0, 64.0], [164.0, 64.0], [314.0, 64.0], [314.0, 264.0], [164.0, 264.0], [14.0, 264.0]], &[[340.0, 0.0], [340.0, 300.0], [670.0, 300.0], [670.0, 0.0]]]);
        assert_eq!((drawn(&most, 'C'), most.advance('C', 1024.0), most.advance(' ', 1024.0)), (c, 680.0, 320.0));
        let square = |left: f32| polygon(&[&[[left, 0.0], [left, 448.0], [left + 192.0, 448.0], [left + 192.0, 0.0]]]);
        assert_eq!((drawn(&usual, 'D'), usual.advance('D', 1024.0)), (square(32.0), 640.0));
        assert_eq!((drawn(&most, 'D'), most.advance('D', 1024.0)), (square(62.0), 670.0));
        // "H" grows taller, and so does the font's cap height, which matches it.
        assert_eq!([usual.metrics(1024.0).cap, most.metrics(1024.0).cap], [448.0, 500.0]);
    }

    /// `avar` version 1 mapping halfway weight to three quarters; and version 2, also making
    /// the font half its minimum width at maximum weight.
    fn bending(second: bool) -> Vec<u8> {
        let pairs = [words(&[4, -16384, -16384, 0, 0, 8192, 12288, 16384, 16384]), words(&[3, -16384, -16384, 0, 0, 16384, 16384])].concat();
        let narrower = store(&[&[[0, 16384, 16384], [0, 0, 0]]], &[(&[0], 1, &[&[0], &[-8192]])]);
        let head = words(&[if second { 2 } else { 1 }, 0, 0, 2]);
        let rows = if second { [longs(&[0, (head.len() + pairs.len() + 8) as u32]), narrower].concat() } else { Vec::new() };
        [head, pairs, rows].concat()
    }

    #[test]
    fn avar_bends_the_axes_and_moves_them_by_each_other() {
        let a = |points: [f32; 6]| polygon(&[&[[points[0], 0.0], [points[1], 0.0], [points[2], 0.0], [points[3], 200.0], [points[4], 200.0], [points[5], 200.0]]]);
        let file = variable(&[(b"avar", bending(false))]);
        // Halfway is three quarters: the corners move out by 37.5.
        assert_eq!(drawn(&at(&file, 650.0, 100.0), 'A'), a([-37.5, 100.0, 237.5, 237.5, 100.0, -37.5]));
        assert_eq!(at(&file, 650.0, 100.0).advance('A', 1024.0), 523.0, "448 and 75");
        // The ends stay the ends.
        assert_eq!(drawn(&at(&file, 900.0, 100.0), 'A'), a([-50.0, 100.0, 250.0, 250.0, 100.0, -50.0]));
        // In version 2, maximum weight is also half the minimum width.
        let file = variable(&[(b"avar", bending(true))]);
        assert_eq!(drawn(&at(&file, 900.0, 100.0), 'A'), a([-50.0, 75.0, 200.0, 200.0, 75.0, -50.0]));
        assert_eq!(at(&file, 900.0, 100.0).advance('A', 1024.0), 498.0);
        assert_eq!(drawn(&at(&file, 900.0, 50.0), 'A'), a([-50.0, 50.0, 150.0, 150.0, 50.0, -50.0]), "no narrower than the least");
    }

    /// `HVAR` making "A" 100 wider at maximum weight and "B" 60; and `MVAR` raising the
    /// ascender 32 there and lowering the descender 16.
    fn measured() -> [(&'static [u8; 4], Vec<u8>); 2] {
        let most: &[&[[i32; 3]]] = &[&[[0, 16384, 16384], [0, 0, 0]]];
        let widths = store(most, &[(&[0], 1, &[&[0], &[100], &[60], &[0], &[0], &[0], &[0], &[0], &[0], &[0]])]);
        let hvar = [words(&[1, 0]), longs(&[20, 0, 0, 0]), widths].concat();
        let measures = store(most, &[(&[0], 1, &[&[32], &[-16]])]);
        let mvar = [words(&[1, 0, 0, 8, 2, 28]), b"hasc".to_vec(), words(&[0, 0]), b"hdsc".to_vec(), words(&[0, 1]), measures].concat();
        [(b"HVAR", hvar), (b"MVAR", mvar)]
    }

    #[test]
    fn hvar_says_how_wide_glyphs_are_and_mvar_how_far_the_letters_reach() {
        let file = variable(&measured());
        let (usual, most, half) = (at(&file, 400.0, 100.0), at(&file, 900.0, 100.0), at(&file, 650.0, 100.0));
        assert_eq!("AB D".chars().map(|letter| most.advance(letter, 1024.0)).collect::<Vec<_>>(), [548.0, 636.0, 256.0, 640.0]);
        assert_eq!(half.advance('B', 1024.0), 606.0);
        // (With `HVAR`, the left phantom point stays.)
        assert_eq!(drawn(&most, 'D'), drawn(&usual, 'D'));
        let measures = |font: &FontFile| (font.metrics(1024.0).ascent, font.metrics(1024.0).descent);
        assert_eq!([measures(&usual), measures(&half), measures(&most)], [(832.0, 192.0), (848.0, 200.0), (864.0, 208.0)]);
        assert_eq!(half.metrics(2048.0), Metrics { ascent: 1696.0, descent: 400.0, cap: 948.0, small: 0.0 });
    }

    #[test]
    fn kerning_moves_as_gdef_says() {
        // "A" and "B" kern 100 closer, 40 less at maximum weight.
        let long = by_seconds(&[1, 1, 1], [0x44, 0], &[&[1, 2, -100, 0]]).len();
        let pairs = [by_seconds(&[1, 1, 1], [0x44, 0], &[&[1, 2, -100, long as i32]]), words(&[0, 0, 0x8000])].concat();
        let gpos = positions(&[(b"DFLT", &[0])], &[(b"kern", &[0])], &[(2, vec![pairs])]);
        let gdef = [words(&[1, 3, 0, 0, 0, 0, 0]), longs(&[18]), store(&[&[[0, 16384, 16384], [0, 0, 0]]], &[(&[0], 1, &[&[40]])])].concat();
        let file = variable(&[(b"GPOS", gpos), (b"GDEF", gdef)]);
        let kerned = |wght: f32| at(&file, wght, 100.0).kerning('A', 'B', 1024.0);
        assert_eq!([kerned(400.0), kerned(650.0), kerned(900.0)], [-100.0, -80.0, -60.0]);
        assert_eq!(at(&file, 900.0, 100.0).kerning('B', 'A', 1024.0), 0.0);
    }

    /// Fonts in one file: each starts where the header says, its table offsets counted from the
    /// file's start.
    pub(crate) fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
        let head = 12 + fonts.len() * 4;
        let starts: Vec<u32> = (0..fonts.len()).map(|n| (head + fonts[..n].concat().len()) as u32).collect();
        let mut all = [b"ttcf".to_vec(), longs(&[0x0001_0000, fonts.len() as u32]), longs(&starts)].concat();
        for (font, start) in fonts.iter().zip(starts) {
            let mut font = font.clone();
            for entry in 0..u16::from_be_bytes([font[4], font[5]]) as usize {
                let at = 12 + entry * 16 + 8;
                let place = u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) + start;
                font[at..at + 4].copy_from_slice(&place.to_be_bytes());
            }
            all.extend(font);
        }
        all
    }

    /// A non-variable font named "Other Serif".
    fn other() -> Vec<u8> {
        let mut tables = [crate::font::tests::tables(), vec![(*b"name", names(&[(1, "Other Serif"), (2, "Regular"), (4, "Other Serif")]))]].concat();
        tables.sort_by_key(|(name, _)| *name);
        file(&[0, 1, 0, 0], &tables)
    }

    #[test]
    fn a_font_is_picked_by_its_name_or_its_place_in_a_collection() {
        let file = variable(&[]);
        let font = || FontFile::read(file.clone()).expect("the font is read");
        let wght = Axis { name: *b"wght", least: 100.0, usual: 400.0, most: 900.0 };
        assert_eq!(font().axes(), [wght, Axis { name: *b"wdth", least: 50.0, usual: 100.0, most: 100.0 }]);
        assert_eq!(font().faces(), ["Test Sans Bold", "Test Sans Thin Narrow"]);
        // A named instance by its own name, by family and instance name, or by PostScript name;
        // case, spaces and hyphens ignored.
        let bold = drawn(&at(&file, 775.0, 100.0), 'A');
        for name in ["Bold", "Test Sans Bold", "test-sans bold", "BOLD"] {
            assert_eq!(font().named(name).map(|font| drawn(&font, 'A')), Some(bold.clone()), "{name}");
        }
        let thin = drawn(&at(&file, 100.0, 50.0), 'A');
        assert_eq!(font().named("TestSans-ThinNarrow").map(|font| drawn(&font, 'A')), Some(thin));
        assert_eq!(font().named("Test Sans").map(|font| drawn(&font, 'A')), Some(drawn(&font(), 'A')), "the font itself");
        assert!(["Black", "", "Test", "Sans Bold"].iter().all(|name| font().named(name).is_none()), "no such place");
        // A non-collection font is its own first font and no other.
        assert!(font().nth(0).is_some() && font().nth(1).is_none());
        // A collection of the two: the first is read, and either by index or name.
        let both = collection(&[file.clone(), other()]);
        let fonts = || FontFile::read(both.clone()).expect("a collection is read");
        let plain = FontFile::read(other()).expect("the font is read");
        assert_eq!(drawn(&fonts(), 'B'), drawn(&font(), 'B'));
        assert_eq!(fonts().nth(1).map(|font| drawn(&font, 'B')), Some(drawn(&plain, 'B')));
        assert_eq!(fonts().named("Other Serif").map(|font| drawn(&font, 'B')), Some(drawn(&plain, 'B')));
        assert!(fonts().nth(2).is_none());
        // From the second font, the first font's instance is found too.
        let found = fonts().nth(1).and_then(|font| font.named("Bold"));
        assert_eq!(found.map(|font| drawn(&font, 'A')), Some(bold));
        assert_eq!(fonts().faces(), ["Test Sans Bold", "Test Sans Thin Narrow", "Other Serif"]);
        assert!(plain.axes().is_empty() && plain.at(b"wght", 900.0).named("Other Serif").is_some(), "not variable");
    }

    /// Everything a font can be asked, asked at several positions.
    fn ask_at(bytes: Vec<u8>) {
        let Some(font) = FontFile::read(bytes) else { return };
        let font = font.at(b"wght", 900.0).at(b"wdth", 50.0);
        let _ = (font.axes(), font.faces());
        let letters = " ABCDH\u{e9}";
        for letter in letters.chars() {
            let _ = (font.advance(letter, 16.0), font.kerning(letter, 'B', 16.0), font.outline(letter, 16.0), font.metrics(16.0));
        }
        let mut fonts = OwnFonts::new(font);
        let mut px = vec![0u32; 64 * 32];
        let _ = fonts.measure(letters, Font::new(11.0), 1.5, None);
        fonts.draw(&mut Canvas::new(&mut px, 64, 32, (0, 0), 1.5), letters, 1.0, 1.0, Font::new(11.0), Color::WHITE);
    }

    #[test]
    fn a_damaged_variable_font_never_stops_the_program() {
        let gdef = [words(&[1, 3, 0, 0, 0, 0, 0]), longs(&[18]), store(&[&[[0, 16384, 16384], [0, 0, 0]]], &[(&[0], 1, &[&[40]])])].concat();
        let long = by_seconds(&[1, 1, 1], [0x44, 0], &[&[1, 2, -100, 0]]).len();
        let pairs = [by_seconds(&[1, 1, 1], [0x44, 0], &[&[1, 2, -100, long as i32]]), words(&[0, 0, 0x8000])].concat();
        let gpos = positions(&[(b"DFLT", &[0])], &[(b"kern", &[0])], &[(2, vec![pairs])]);
        let [hvar, mvar] = measured();
        let everything = variable(&[(b"avar", bending(true)), hvar, mvar, (b"GDEF", gdef), (b"GPOS", gpos)]);
        for whole in [everything.clone(), collection(&[other(), everything])] {
            // (Whole, the font varies.)
            let font = FontFile::read(whole.clone()).expect("the font is read").named("Bold").expect("it has the place");
            assert_ne!(drawn(&font, 'A'), drawn(&FontFile::read(whole.clone()).unwrap().nth(whole.starts_with(b"ttcf") as usize).unwrap(), 'A'));
            (0..whole.len()).for_each(|long| ask_at(whole[..long].to_vec()));
            for (at, change) in (0..whole.len()).flat_map(|at| [(at, 0), (at, 255), (at, whole[at] ^ 1 << (at % 8))]) {
                let mut damaged = whole.clone();
                damaged[at] = change;
                ask_at(damaged);
            }
        }
    }
}
