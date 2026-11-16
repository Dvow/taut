//! Light hinting: small text snapped to whole pixels vertically only, so an app's own fonts are
//! as crisp at 100% as system text. Drawn exactly, a glyph's baseline, x-height, cap-height and
//! descender fall between pixel rows as often as not, greying both rows: the text looks soft.
//! So, as FreeType's light hinting does (which Linux desktops ask for), each glyph is stretched
//! slightly vertically, never horizontally:
//!
//! - Once per font, the heights of those lines ("blue zones") are found from the glyphs that
//!   reach them: the median top of "THEZOCQS" is the cap-height, and so on. Each line has a
//!   flat height (the top of "x") and a round one ("o" overshoots to look as tall).
//! - In a glyph, the horizontal segments of its outline are found; those at one height make an
//!   edge. An edge where ink starts and the one below where it ends are the two sides of a stem
//!   (the bar of "H", the top of "o").
//! - An edge on a font line snaps to where that line is snapped: a whole pixel. The other side
//!   of its stem keeps the stem's thickness. A stem on no line is placed to fill whole rows as
//!   nearly as its thickness allows, and an edge in no stem moves with its neighbours.
//! - An edge's points move with it; every other point is interpolated between the edges above
//!   and below, so nothing flips and the glyph keeps its shape.
//!
//! Nothing moves horizontally: advances, kerning and line breaks are as without it. Glyphs of
//! `LARGE` pixels per em or more are left alone: there it no longer sharpens, it only distorts.
//! It ignores a font's own hinting instructions (fonts need not have them, and most new ones
//! don't): it is what FreeType's autohinter does with any font, matched glyph by glyph (see the
//! tests).
use crate::font::{FontFile, Placing};
use crate::geom::{Point, pt};
use crate::path::{Path, Step};

/// Glyphs at this many pixels per em or more are not hinted.
pub(crate) const LARGE: f32 = 24.0;
/// Nor are glyphs with more outline segments than this, far beyond any alphabet's: the work is
/// quadratic, and a font may have a glyph of tens of thousands.
const MOST: usize = 2000;

/// The Latin blue zones, each with the glyphs it is measured from, and whether it is a top (ink
/// below) or a bottom: as FreeType's autohinter has them.
const LINES: [(&str, bool); 6] = [("THEZOCQS", true), ("HEZLOCUS", false), ("fijkdbh", true), ("uvxzoesc", true), ("nrxzoesc", false), ("pqgjy", false)];
/// Which of them is the x-height: a glyph's vertical stretch is set by it.
const SMALL: usize = 3;

/// Whether a character is hinted with the zones found here: a Latin, Greek or Cyrillic letter,
/// or a digit, punctuation or symbol used with them. (Other scripts have their own lines, which
/// aren't found; they draw as outlined.)
pub(crate) fn fits(letter: char) -> bool {
    matches!(letter as u32, 0..=0x2ff | 0x370..=0x52f | 0x1e00..=0x1fff | 0x2000..=0x20cf | 0x2c60..=0x2c7f | 0xa640..=0xa69f | 0xa720..=0xa7ff)
}

/// One of a font's lines, in font units downward from the baseline: its flat and round heights.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Zone {
    flat: f32,
    round: f32,
    top: bool,
}

/// The lines one font has glyphs for.
pub(crate) struct Zones {
    zones: [Option<Zone>; 6],
}

impl Zones {
    /// Finds a font's lines from its glyphs. (A font lacking them all has none, and its glyphs
    /// only get their stems snapped.)
    pub(crate) fn of(font: &FontFile) -> Zones {
        Zones { zones: LINES.map(|(letters, top)| Zone::found(font, letters, top)) }
    }
}

impl Zone {
    /// The line some glyphs of a font reach: up to it (`top`), else down to it.
    fn found(font: &FontFile, letters: &str, top: bool) -> Option<Zone> {
        let (mut flats, mut rounds) = (Vec::new(), Vec::new());
        for glyph in letters.chars().map(|letter| font.glyph(letter)).filter(|glyph| *glyph != 0) {
            let dots = Dots::of(&font.sketch(glyph, Placing::sized(1.0, 0.0)).path);
            if let Some((reach, flat)) = dots.furthest(top, font.units() / 14.0) {
                [&mut rounds, &mut flats][flat as usize].push(reach);
            }
        }
        Zone::of(flats, rounds, top)
    }

    /// A line from its flat and round heights: the median of each. A round height not beyond
    /// the flat ones is no overshoot: the line is then halfway between.
    fn of(mut flats: Vec<f32>, mut rounds: Vec<f32>, top: bool) -> Option<Zone> {
        let middle = |all: &mut Vec<f32>| {
            in_order(all, |y| -*y);
            all.get(all.len() / 2).copied()
        };
        let (flat, round) = match (middle(&mut flats), middle(&mut rounds)) {
            (None, None) => return None,
            (flat, round) => (flat.or(round)?, round.or(flat)?),
        };
        let both = (flat + round) / 2.0;
        let further = if top { round <= flat } else { round >= flat };
        Some(if further { Zone { flat, round, top } } else { Zone { flat: both, round: both, top } })
    }

    /// This line at `unit` pixels per font unit, and where it snaps: its flat height rounded to
    /// a whole pixel; its round one with it if the overshoot is under half a pixel, half a
    /// pixel beyond if under three quarters, else a pixel. `None` if the overshoot is larger
    /// still: the size is large enough to need none of this. (Heights are first rounded to 1/64
    /// pixel, as the autohinter counts.)
    fn set(&self, unit: f32) -> Option<Set> {
        let (flat, round) = (sixty_fourths(self.flat * unit), sixty_fourths(self.round * unit));
        let over = (round - flat).abs();
        let further = match over {
            ..0.5 => 0.0,
            ..0.75 => 0.5,
            _ => 1.0,
        };
        let flat_to = whole(flat);
        let round_to = flat_to + if self.top { -further } else { further };
        (over <= 0.75).then_some(Set { flat, round, top: self.top, flat_to, round_to })
    }
}

/// A font line at one size, in pixels downward: its flat and round heights, and where each
/// snaps.
#[derive(Clone, Copy, Debug)]
struct Set {
    flat: f32,
    round: f32,
    top: bool,
    flat_to: f32,
    round_to: f32,
}

/// A stable insertion sort by key, smallest first. (A glyph has a few dozen items at most, and
/// this is a fraction of the code size of a general sort.)
fn in_order<T>(items: &mut [T], key: impl Fn(&T) -> f32) {
    for n in 1..items.len() {
        let mut at = n;
        while at > 0 && key(&items[at - 1]) > key(&items[at]) {
            items.swap(at - 1, at);
            at -= 1;
        }
    }
}

/// The nearest whole pixel to a height (downward): a half rounds up, as the autohinter, which
/// counts upward, rounds.
fn whole(y: f32) -> f32 {
    (y - 0.5).ceil()
}

/// A height in pixels, rounded to 1/64.
fn sixty_fourths(pixels: f32) -> f32 {
    (pixels * 64.0).round() / 64.0
}

/// An outline's points: position, and on-curve or control point; where each contour ends among
/// them; and which point each path point is, in path order (a contour's closing point, back at
/// its first, is that first one).
struct Dots {
    dots: Vec<(Point, bool)>,
    ends: Vec<usize>,
    path: Vec<usize>,
}

impl Dots {
    fn of(path: &Path) -> Dots {
        let room = path.steps().len() * 2;
        let mut all = Dots { dots: Vec::with_capacity(room), ends: Vec::new(), path: Vec::with_capacity(room) };
        for step in path.steps() {
            match *step {
                Step::Move(at) => all.begin(at),
                Step::Line(to) => all.add(&[(to, true)]),
                Step::Quad(by, to) => all.add(&[(by, false), (to, true)]),
                Step::Cubic(a, b, to) => all.add(&[(a, false), (b, false), (to, true)]),
                Step::Close => all.end(),
            }
        }
        all.end();
        all
    }

    /// Begins another contour at `at`.
    fn begin(&mut self, at: Point) {
        self.end();
        self.add(&[(at, true)]);
    }

    /// The path's next points.
    fn add(&mut self, dots: &[(Point, bool)]) {
        self.path.extend(self.dots.len()..self.dots.len() + dots.len());
        self.dots.extend_from_slice(dots);
    }

    /// Closes the last begun contour, if any: its closing point, back at its first, is the
    /// first.
    fn end(&mut self) {
        let start = self.ends.last().copied().unwrap_or(0);
        if self.dots.len() > start + 1 && self.dots.last() == self.dots.get(start) {
            self.dots.pop();
            self.path.last_mut().into_iter().for_each(|last| *last = start);
        }
        if self.dots.len() > start {
            self.ends.push(self.dots.len());
        }
    }

    /// Each closed contour: its first point's index among all, and its points.
    fn outlines(&self) -> impl Iterator<Item = (usize, &[(Point, bool)])> {
        self.ends.iter().scan(0, |start, end| Some((*start, &self.dots[std::mem::replace(start, *end)..*end])))
    }

    /// How far up (`top`, the least far down) or down the outline reaches, in font units, and
    /// whether flat there, as the autohinter judges: the points either side of the extreme one
    /// that are about as far (within 5 units, or along a slope under 1 in 20) make a segment,
    /// which is flat if its first and last on-curve points are more than `long` apart, or it
    /// starts and ends on-curve. (A one-point contour draws nothing and is ignored.)
    fn furthest(&self, top: bool, long: f32) -> Option<(f32, bool)> {
        let sign = if top { -1.0 } else { 1.0 };
        let further = |a: &(Point, bool), b: &(Point, bool)| (sign * a.0.y).total_cmp(&(sign * b.0.y));
        let outlines = self.outlines().map(|(_, outline)| outline).filter(|outline| outline.len() > 1);
        let (outline, at) = outlines.filter_map(|outline| Some((outline, (0..outline.len()).max_by(|a, b| further(&outline[*a], &outline[*b]))?))).max_by(|(a, n), (b, m)| further(&a[*n], &b[*m]))?;
        let (count, best) = (outline.len(), outline[at].0);
        let near = |n: usize| {
            let dot = outline[n % count].0;
            let off = (dot.y - best.y).abs();
            off <= 5.0 || (dot.x - best.x).abs() > 20.0 * off
        };
        let after = (1..count).take_while(|n| near(at + n)).count();
        let before = (1..count - after).take_while(|n| near(at + count - n)).count();
        let stretch: Vec<(Point, bool)> = (at + count - before..=at + count + after).map(|n| outline[n % count]).collect();
        let on: Vec<f32> = stretch.iter().filter(|dot| dot.1).map(|dot| dot.0.x).collect();
        let wide = on.first().zip(on.last()).is_some_and(|(first, last)| (last - first).abs() > long);
        let ends_on = stretch[0].1 && stretch[stretch.len() - 1].1;
        Some((best.y, wide || ends_on))
    }
}

/// A horizontal run of a contour's points at one height: its height, horizontal extent, whether
/// ink is below (or above), whether it is a curve's round extreme, and its points (a range of a
/// list of them).
#[derive(Clone, Debug)]
struct Run {
    y: f32,
    left: f32,
    right: f32,
    top: bool,
    round: bool,
    points: std::ops::Range<usize>,
}

/// Runs at one height merged: original and snapped height (once placed), horizontal extent,
/// whether ink is below, whether round (as most of its runs are), where its blue zone snaps,
/// and the edge on the other side of its stem.
#[derive(Clone, Copy, Debug)]
struct Edge {
    at: f32,
    to: Option<f32>,
    left: f32,
    right: f32,
    top: bool,
    round: bool,
    line: Option<f32>,
    other: Option<usize>,
}

/// A glyph's outline, in pixels from its pen on the baseline (y down), at `unit` pixels per
/// unit of `font`: hinted vertically, with its bounding box (left, top, right, bottom). `None`
/// if left as is: too large, nothing to snap, or too many segments (`MOST`).
pub(crate) fn set(path: &Path, font: &FontFile, unit: f32) -> Option<(Path, [f32; 4])> {
    let em = font.units() * unit;
    if em.is_nan() || em >= LARGE || path.steps().is_empty() || path.steps().len() > MOST {
        return None;
    }
    let zones = font.zones();
    let tall = stretch(zones, unit, em);
    let lines = zones.zones.map(|zone| zone.and_then(|zone| zone.set(unit * tall)));
    let mut dots = Dots::of(path);
    dots.dots.iter_mut().for_each(|dot| dot.0.y *= tall);
    let (mut runs, members) = runs(&dots, em);
    in_order(&mut runs, |run| run.y);
    let (mut edges, of) = edges(&runs, &lines, em);
    link(&mut edges, em);
    place(&mut edges);
    // Each edge's points move with it.
    let mut pinned = vec![None; dots.dots.len()];
    for (run, to) in runs.iter().zip(of).filter_map(|(run, edge)| Some((run, edges[edge].to?))) {
        members[run.points.clone()].iter().for_each(|dot| pinned[*dot] = Some(to));
    }
    Some(warped(path, tall, &held(&edges), (&pinned, &dots.path)))
}

/// A font's cap-height and x-height when drawn at `unit` pixels per font unit with hinting: in
/// whole pixels, for each the font has glyphs for. `None` at sizes not hinted.
pub(crate) fn heights(font: &FontFile, unit: f32) -> Option<[Option<f32>; 2]> {
    let em = font.units() * unit;
    if em.is_nan() || em >= LARGE {
        return None;
    }
    let zones = font.zones();
    let tall = stretch(zones, unit, em);
    let height = |n: usize| Some(-zones.zones[n]?.set(unit * tall)?.flat_to);
    Some([height(0), height(SMALL)])
}

/// The vertical stretch of a font's glyphs at a size: enough to put the x-height overshoot on a
/// whole pixel (up from a little over a third of a pixel past one, else down), as the
/// autohinter does; never by 2 pixels per em or more. (The height is first rounded to 1/64
/// pixel, as the autohinter counts: one a hair under the mark goes up as it does there.)
fn stretch(zones: &Zones, unit: f32, em: f32) -> f32 {
    let Some(small) = zones.zones[SMALL] else { return 1.0 };
    let high = sixty_fourths(-small.round * unit);
    let whole = (high + 40.0 / 64.0).floor();
    let tall = whole / high;
    if high > 0.0 && whole > 0.0 && (em * (tall - 1.0)).abs() < 2.0 { tall } else { 1.0 }
}

/// An outline's runs, and each one's points: point sequences (on-curve or control) where each
/// step is horizontal (14 times more across than up or down), the same direction, within a
/// pixel's height. The ink side follows from the contour's direction. A run is round if it
/// starts or ends at a control point and its on-curve points are under 1/14 em (`em` pixels)
/// apart, as in the autohinter.
fn runs(dots: &Dots, em: f32) -> (Vec<Run>, Vec<usize>) {
    let area: f32 = dots.outlines().flat_map(|(_, outline)| outline.iter().zip(outline.iter().cycle().skip(1))).map(|((a, _), (b, _))| a.x * b.y - b.x * a.y).sum();
    let (mut runs, mut members, mut ways) = (Vec::new(), Vec::with_capacity(dots.dots.len()), Vec::new());
    for (from, outline) in dots.outlines() {
        // Each step to the next point: its direction, if horizontal.
        ways.clear();
        ways.extend(outline.iter().zip(outline.iter().cycle().skip(1)).map(|((a, _), (b, _))| ((b.y - a.y).abs() * 14.0 < (b.x - a.x).abs()).then_some(b.x > a.x)));
        let count = ways.len();
        // Started after a non-horizontal step, so no run is split.
        let Some(first) = ways.iter().position(Option::is_none) else { continue };
        let mut n = first + 1;
        while n < first + 1 + count {
            let Some(way) = ways[round(n, count)] else {
                n += 1;
                continue;
            };
            let length = (n..first + 1 + count).take_while(|m| ways[round(*m, count)] == Some(way)).count();
            let start = members.len();
            members.extend((n..=n + length).map(|m| from + round(m, count)));
            runs.extend(Run::of(&dots.dots, &members[start..], way == (area > 0.0), em, start..members.len()));
            n += length;
        }
    }
    (runs, members)
}

/// The index of point `n` of `count` in a contour walked more than once around: wrapped back to
/// the first lap.
fn round(n: usize, count: usize) -> usize {
    let mut n = n;
    while n >= count {
        n -= count;
    }
    n
}

impl Run {
    /// A run of these points (`made`, which are `points` among all runs' points), unless they
    /// vary in height by more than a pixel.
    fn of(dots: &[(Point, bool)], made: &[usize], top: bool, em: f32, points: std::ops::Range<usize>) -> Option<Run> {
        let stretch = made.iter().map(|dot| dots[*dot]);
        let (ys, xs) = (stretch.clone().map(|dot| dot.0.y), stretch.clone().map(|dot| dot.0.x));
        let (high, low) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(f32::MIN, f32::max));
        let on = stretch.filter(|dot| dot.1).map(|dot| dot.0.x);
        let across = on.clone().fold(f32::MIN, f32::max) - on.fold(f32::MAX, f32::min);
        let beside = !dots[made[0]].1 || !dots[made[made.len() - 1]].1;
        let round = beside && across < em / 14.0;
        let (left, right) = (xs.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max));
        (low - high <= 1.0).then_some(Run { y: (high + low) / 2.0, left, right, top, round, points })
    }
}

/// An outline's edges, top down: its runs (top down), with same-direction runs within a quarter
/// pixel of each other merged; and each run's edge. An edge on one of `lines` snaps where that
/// line does.
fn edges(runs: &[Run], lines: &[Option<Set>], em: f32) -> (Vec<Edge>, Vec<usize>) {
    let (mut edges, mut of) = (Vec::<Edge>::new(), Vec::with_capacity(runs.len()));
    for run in runs {
        let joins = (0..edges.len()).rev().take_while(|e| run.y - edges[*e].at < 0.25).find(|e| edges[*e].top == run.top);
        of.push(joins.unwrap_or(edges.len()));
        if joins.is_none() {
            edges.push(Edge { at: run.y, to: None, left: run.left, right: run.right, top: run.top, round: false, line: None, other: None });
        }
        let edge = &mut edges[of[of.len() - 1]];
        (edge.left, edge.right) = (edge.left.min(run.left), edge.right.max(run.right));
    }
    for (n, edge) in edges.iter_mut().enumerate() {
        let votes: i32 = runs.iter().zip(&of).filter(|(_, e)| **e == n).map(|(run, _)| if run.round { 1 } else { -1 }).sum();
        edge.round = votes >= 0;
        edge.line = on_line(edge, lines, em);
    }
    (edges, of)
}

/// Where an edge snaps if it is on a line: within 1/40 em (and half a pixel) of the line's flat
/// height, or for a round edge beyond that, of its round height. The nearest line facing the
/// same way.
fn on_line(edge: &Edge, lines: &[Option<Set>], em: f32) -> Option<f32> {
    let mut best = (f32::min(em / 40.0, 0.5), None);
    for line in lines.iter().flatten().filter(|line| line.top == edge.top) {
        let flat = (edge.at - line.flat).abs();
        if flat < best.0 {
            best = (flat, Some(line.flat_to));
        }
        let past = if line.top { edge.at < line.flat } else { edge.at > line.flat };
        let round = (edge.at - line.round).abs();
        if edge.round && past && flat > 0.0 && round < best.0 {
            best = (round, Some(line.round_to));
        }
    }
    best.1
}

/// Pairs the edges that are a stem's two sides: an edge where ink starts, and the nearest
/// opposing edge below it where ink ends, each the other's nearest, at most a quarter em apart.
fn link(edges: &mut [Edge], em: f32) {
    let across = |a: &Edge, b: &Edge| a.right.min(b.right) - a.left.max(b.left) >= em / 125.0;
    let below = |edges: &[Edge], n: usize| (n + 1..edges.len()).find(|m| !edges[*m].top && across(&edges[n], &edges[*m]));
    let above = |edges: &[Edge], m: usize| (0..m).rev().find(|n| edges[*n].top && across(&edges[*n], &edges[m]));
    for n in 0..edges.len() {
        let Some(m) = below(edges, n).filter(|m| edges[n].top && edges[*m].at - edges[n].at < em / 4.0) else { continue };
        if above(edges, m) == Some(n) {
            (edges[n].other, edges[m].other) = (Some(m), Some(n));
        }
    }
}

/// Places the edges bottom up, as the autohinter's light mode does: those on a line first, each
/// with its stem's other side; then each stem on no line, moved as the first placed edge (the
/// anchor) was and snapped to fill whole rows as its thickness allows; then each edge in no
/// stem, interpolated between the placed edges around it, or moved as the anchor was (to half a
/// pixel).
fn place(edges: &mut [Edge]) {
    let mut anchor = None;
    for n in (0..edges.len()).rev() {
        let other = edges[n].other;
        let (on, with) = match (edges[n].line, other.filter(|m| edges[*m].line.is_some())) {
            _ if edges[n].to.is_some() => continue,
            (Some(_), _) => (n, other),
            (None, Some(m)) => (m, Some(n)),
            (None, None) => continue,
        };
        let line = edges[on].line.unwrap_or(edges[on].at);
        edges[on].to = Some(line);
        if let Some(m) = with.filter(|m| edges[*m].line.is_none()) {
            edges[m].to = Some(line + edges[m].at - edges[on].at);
        }
        anchor = anchor.or(Some(n));
    }
    for n in (0..edges.len()).rev() {
        let Some(m) = edges[n].other.filter(|_| edges[n].to.is_none()) else { continue };
        let ([upper, lower], thick) = ([n.min(m), n.max(m)], (edges[m].at - edges[n].at).abs());
        if let Some(to) = edges[m].to {
            edges[n].to = Some(to + edges[n].at - edges[m].at);
            continue;
        }
        let top = match anchor {
            None if thick >= 1.5 => whole(edges[lower].at) - thick,
            None => stroke(edges[upper].at, thick),
            Some(anchor) => stroke(edges[upper].at + moved_by(&edges[anchor]), thick),
        };
        (edges[upper].to, edges[lower].to) = (Some(top), Some(top + thick));
        anchor = anchor.or(Some(n));
    }
    for n in (0..edges.len()).rev() {
        if edges[n].to.is_some() {
            continue;
        }
        let Some(first) = anchor else {
            (edges[n].to, anchor) = (Some(whole(edges[n].at)), Some(n));
            continue;
        };
        let below = (n + 1..edges.len()).find(|m| edges[*m].to.is_some());
        let above = (0..n).rev().find(|m| edges[*m].to.is_some());
        edges[n].to = Some(match (below.map(|m| edges[m]), above.map(|m| edges[m])) {
            (Some(below), Some(above)) => between(edges[n].at, &below, &above),
            _ => edges[first].to.unwrap_or(edges[first].at) + halves(edges[n].at - edges[first].at),
        });
    }
}

/// How far an edge moved (zero if not placed).
fn moved_by(edge: &Edge) -> f32 {
    edge.to.map_or(0.0, |to| to - edge.at)
}

/// Where a height between two placed edges goes: proportionally between where they went.
fn between(at: f32, below: &Edge, above: &Edge) -> f32 {
    let (from, to) = (below.to.unwrap_or(below.at), above.to.unwrap_or(above.at));
    if below.at == above.at { from } else { from + (at - below.at) * (to - from) / (above.at - below.at) }
}

/// A distance rounded to half a pixel (downward: a half goes up).
fn halves(by: f32) -> f32 {
    ((by - 0.25) * 2.0).ceil() / 2.0
}

/// Where a `thick`-pixel stem's top goes, if it would start at `at`: a pixel or less is centred
/// in a row; up to 1.5 pixels, near a row's top; thicker, with its top or bottom on a whole
/// pixel, whichever moves its middle less.
fn stroke(at: f32, thick: f32) -> f32 {
    let middle = at + thick / 2.0;
    if thick < 1.5 {
        let (up, down) = if thick <= 1.0 { (0.5, 0.5) } else { (26.0 / 64.0, 38.0 / 64.0) };
        let whole = whole(middle);
        let to = if (middle - (whole - up)).abs() <= (middle - (whole + down)).abs() { whole - up } else { whole + down };
        return to - thick / 2.0;
    }
    let (by_top, by_bottom) = (whole(at), whole(at + thick) - thick);
    if (by_top - at).abs() <= (by_bottom - at).abs() { by_top } else { by_bottom }
}

/// The heights the placed edges (top down) fix, from where they were to where they went: never
/// one above the previous.
fn held(edges: &[Edge]) -> Vec<(f32, f32)> {
    let mut held: Vec<(f32, f32)> = edges.iter().filter_map(|edge| Some((edge.at, edge.to?))).collect();
    let mut lowest = f32::MIN;
    for (_, to) in &mut held {
        *to = to.max(lowest);
        lowest = *to;
    }
    held
}

/// Where a height goes: between two fixed ones, proportionally; above the first or below the
/// last, moved as that one was.
fn moved(y: f32, held: &[(f32, f32)]) -> f32 {
    let after = held.partition_point(|(at, _)| *at <= y);
    match (after.checked_sub(1).map(|n| held[n]), held.get(after).copied()) {
        (Some((a, to_a)), Some((b, to_b))) if b > a => to_a + (y - a) * (to_b - to_a) / (b - a),
        (Some((a, to_a)), _) => y + to_a - a,
        (None, Some((b, to_b))) => y + to_b - b,
        (None, None) => y,
    }
}

/// The outline with every point moved vertically (after stretching by `tall`): an edge's points
/// to where it went (`pinned`, per contour point, and which contour point each path point is),
/// others as `held` says; and its new bounding box.
fn warped(path: &Path, tall: f32, held: &[(f32, f32)], (pinned, of): (&[Option<f32>], &[usize])) -> (Path, [f32; 4]) {
    let (mut out, mut reach, mut next) = (Path::new(), [f32::MAX, f32::MAX, f32::MIN, f32::MIN], 0);
    for step in path.steps() {
        // The segment's points moved, and the segment rebuilt through them.
        let (mut points, count) = match *step {
            Step::Move(a) | Step::Line(a) => ([a; 3], 1),
            Step::Quad(a, b) => ([a, b, b], 2),
            Step::Cubic(a, b, c) => ([a, b, c], 3),
            Step::Close => ([pt(0.0, 0.0); 3], 0),
        };
        for p in &mut points[..count] {
            let y = of.get(next).and_then(|dot| pinned[*dot]).unwrap_or_else(|| moved(p.y * tall, held));
            (next, reach, p.y) = (next + 1, [reach[0].min(p.x), reach[1].min(y), reach[2].max(p.x), reach[3].max(y)], y);
        }
        let [a, b, c] = points;
        match *step {
            Step::Move(_) => out.move_to(a.x, a.y),
            Step::Line(_) => out.line_to(a.x, a.y),
            Step::Quad(..) => out.quad_to(a.x, a.y, b.x, b.y),
            Step::Cubic(..) => out.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y),
            Step::Close => out.close(),
        };
    }
    (out, reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Canvas;
    use crate::color::Color;
    use crate::font::OwnFonts;
    use crate::font::tests::{file, glyph, longs, words};
    use crate::text::{Font, TextSystem};

    /// A 1000-unit font of two box glyphs: an "x" 530 units high, and an "H" 720 high with a
    /// bar from 330 to 400. (No size makes those whole pixels.)
    fn boxes() -> FontFile {
        let (on, box_of) = (true, |[x0, y0, x1, y1]: [i32; 4]| vec![(x0, y0, true), (x0, y1, true), (x1, y1, true), (x1, y0, true)]);
        let (x, stems) = (box_of([50, 0, 450, 530]), [box_of([60, 0, 150, 720]), box_of([450, 0, 540, 720]), box_of([150, 330, 450, 400])]);
        let glyphs = [vec![], glyph(&[&x]), glyph(&[&stems[0], &stems[1], &stems[2]]), glyph(&[&[(0, 0, on), (0, 10, on), (10, 10, on)]])];
        let glyphs: Vec<Vec<u8>> = glyphs.into_iter().map(|glyph| [&glyph[..], &[0][..glyph.len() % 2]].concat()).collect();
        let starts: Vec<i32> = (0..=glyphs.len()).map(|n| glyphs[..n].concat().len() as i32 / 2).collect();
        // cmap groups: "H" is glyph 2 and "x" glyph 1.
        let groups = [[72, 72, 2], [120, 120, 1]];
        let map = [words(&[0, 1, 3, 10]), longs(&[12]), words(&[12, 0]), longs(&[16 + 12 * 2, 0, 2]), longs(&groups.concat())].concat();
        let tables = vec![
            (*b"head", [words(&[1, 0, 0, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1000]), vec![0; 34]].concat()),
            (*b"hhea", [words(&[1, 0, 800, -200]), vec![0; 26], words(&[4])].concat()),
            (*b"maxp", [words(&[1, 0, 4]), vec![0; 26]].concat()),
            (*b"hmtx", words(&[500, 0, 500, 50, 600, 60, 100, 0])),
            (*b"cmap", map),
            (*b"loca", words(&starts)),
            (*b"glyf", glyphs.concat()),
        ];
        FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read")
    }

    /// The per-pixel coverage of `text` at `size`, in 40 by 24 pixels, baseline 16 down.
    fn written(fonts: &mut OwnFonts, text: &str, size: f32) -> Vec<Vec<u8>> {
        let mut px = vec![0u32; 40 * 24];
        let ascent = fonts.metrics(Font::new(size), 1.0).ascent;
        fonts.draw(&mut Canvas::new(&mut px, 40, 24, (0, 0), 1.0), text, 0.0, 16.0 - ascent, Font::new(size), Color::WHITE);
        px.chunks(40).map(|row| row.iter().map(|pixel| (pixel >> 24) as u8).collect()).collect()
    }

    #[test]
    fn the_lines_of_a_font_are_found_from_its_letters() {
        // Its "x" and "H" tops are its x-height and cap-height, both on the baseline; it has no
        // glyphs for the others.
        let zones = Zones::of(&boxes()).zones;
        let line = |flat: f32, top: bool| Some(Zone { flat, round: flat, top });
        assert_eq!(zones, [line(-720.0, true), line(0.0, false), None, line(-530.0, true), line(0.0, false), None]);
        // A round height not beyond the flat ones is no overshoot.
        assert_eq!(Zone::of(vec![-500.0, -510.0, -505.0], vec![-520.0, -530.0], true), Some(Zone { flat: -505.0, round: -530.0, top: true }));
        assert_eq!(Zone::of(vec![-500.0], vec![-490.0], true), Some(Zone { flat: -495.0, round: -495.0, top: true }));
        // At 13 pixels the x-height is 6.89: it snaps to 7, and every height with it. The round
        // cap-height reaches 0.6 pixel further: half a pixel.
        let unit = 13.0 / 1000.0;
        assert_eq!(stretch(&Zones::of(&boxes()), unit, 13.0), 7.0 / sixty_fourths(530.0 * unit));
        let set = Zone { flat: -720.0, round: -766.0, top: true }.set(unit).unwrap();
        assert_eq!((set.flat_to, set.round_to), (-9.0, -9.5));
        assert!(Zone { flat: -720.0, round: -780.0, top: true }.set(unit).is_none(), "three quarters of a pixel and more is too large to set");
    }

    #[test]
    fn the_tops_and_bars_of_small_letters_fill_whole_rows() {
        // At 13 pixels an "x" reaches 6.89 pixels up, so its top row is 89% covered; and the
        // "H" bar spans two rows, a fifth of one and seven tenths of the other.
        let soft = written(&mut OwnFonts::new(boxes()).smooth(), "xH", 13.0);
        let sharp = written(&mut OwnFonts::new(boxes()), "xH", 13.0);
        let rows = |picture: &[Vec<u8>], column: usize| picture.iter().map(|row| row[column]).collect::<Vec<u8>>();
        // Column 2 is inside the "x", column 8 in the "H"'s left stem, column 10 in its bar.
        assert_eq!(&rows(&soft, 2)[8..17], [0, 225, 255, 255, 255, 255, 255, 255, 0]);
        assert_eq!(&rows(&soft, 10)[9..13], [0, 50, 180, 0]);
        // Hinted, the "x" is 7 full rows, and the bar one row: centred in the row nearest the
        // bar's middle, as thick as it was (92% of a pixel).
        assert_eq!(&rows(&sharp, 2)[8..17], [0, 255, 255, 255, 255, 255, 255, 255, 0]);
        assert_eq!(&rows(&sharp, 10)[9..13], [0, 0, 234, 0]);
        // Nothing moved horizontally: the same columns are covered, and a row through both
        // glyphs below the bar is the same; caps are 10 pixels high (9.36, stretched with the
        // x-height).
        let columns = |picture: &[Vec<u8>]| (0..40).filter(|x| picture.iter().any(|row| row[*x] > 0)).collect::<Vec<usize>>();
        assert_eq!((columns(&soft), &soft[13]), (columns(&sharp), &sharp[13]));
        assert_eq!(rows(&sharp, 8).iter().position(|cover| *cover > 0), Some(16 - 10));
        // A text line is vertically centred by its glyphs' drawn heights: so they report their
        // hinted heights.
        let heights = |mut fonts: OwnFonts| fonts.metrics(Font::new(13.0), 1.0);
        assert_eq!(heights(OwnFonts::new(boxes())).cap, 10.0);
        assert_eq!([heights(OwnFonts::new(boxes()).smooth()).cap, heights(OwnFonts::new(boxes())).small], [9.0, 7.0]);
    }

    #[test]
    fn nothing_moves_sideways() {
        // Glyph ends and line breaks are the same with hinting and without, kerned pairs
        // included (`font.rs`' test font kerns "AB" and "BA"), at every size and scale.
        let text = "AB BA ABBA xH Hx BAB AxB HHxx A B AB BA ABBA";
        for (size, scale) in [(9.0, 1.0), (11.0, 1.0), (13.0, 1.0), (13.0, 1.5), (16.0, 1.25), (13.0, 2.0)] {
            let font = Font::new(size);
            for read in [|| FontFile::read(file(&[0, 1, 0, 0], &crate::font::tests::tables())).unwrap(), boxes] {
                let (mut sharp, mut soft) = (OwnFonts::new(read()), OwnFonts::new(read()).smooth());
                let (mut a, mut b, mut lines) = (Vec::new(), Vec::new(), [Vec::new(), Vec::new()]);
                assert_eq!(sharp.measure(text, font, scale, Some(&mut a)), soft.measure(text, font, scale, Some(&mut b)));
                assert_eq!(a, b, "at {size} by {scale}");
                crate::text::wrap(text, &a, 60.0, &mut lines[0]);
                crate::text::wrap(text, &b, 60.0, &mut lines[1]);
                assert_eq!(format!("{:?}", lines[0]), format!("{:?}", lines[1]));
            }
        }
    }

    #[test]
    fn large_letters_and_other_scripts_are_drawn_as_they_are() {
        // From 24 pixels per em, outlines are as crisp as smaller hinted ones; hinting would
        // only distort.
        for size in [24.0, 30.0] {
            assert_eq!(written(&mut OwnFonts::new(boxes()), "xH", size), written(&mut OwnFonts::new(boxes()).smooth(), "xH", size));
        }
        assert_ne!(written(&mut OwnFonts::new(boxes()), "xH", 23.0), written(&mut OwnFonts::new(boxes()).smooth(), "xH", 23.0));
        assert!("Az09.,;€–“ÉßЖωắ".chars().all(fits));
        assert!(!"אبअ中ก한".chars().any(fits));
    }

    #[test]
    fn a_stroke_fills_whole_rows_as_far_as_its_thickness_lets_it() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // A stem of a pixel or less is centred in the row nearest its middle.
        assert!(near(stroke(-5.9, 0.9), -5.95) && near(stroke(-5.6, 0.9), -5.95));
        // One up to 1.5 pixels, its middle 26/64 above a whole pixel or 38/64 below: its top
        // lands near a row's top.
        assert!(near(stroke(-4.1, 1.25), -3.0 - 26.0 / 64.0 - 0.625));
        // A thicker one with its top or bottom on a whole pixel, whichever is nearer.
        assert!(near(stroke(-6.2, 2.5), -6.0) && near(stroke(-6.4, 2.5), -6.5));
        // A half rounds up.
        assert_eq!((whole(-3.5), whole(2.5), halves(-0.25), halves(0.25)), (-4.0, 2.0, -0.5, 0.0));
    }
}
