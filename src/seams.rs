//! Exact coverage where a shape's subpaths meet inside a pixel. Summed coverage (see
//! `Canvas::fill_with`) is exact where one subpath's edges cross a pixel; where two subpaths'
//! edges do, their sums can cancel: two that touch going round opposite ways leave a hairline,
//! where browsers show none. So such a pixel's coverage is worked out from the winding along
//! `LINES` lines across its row, exact along each line. A shape of one subpath never comes here.
use crate::canvas::{Summed, rows};
use crate::geom::Point;

/// Lines across a pixel where its coverage is worked out from the winding. Exact for straight
/// edges; a corner or crossing inside a line's band is off by at most its share (1/64).
const LINES: usize = 64;

/// An edge's pixels in one row: from `first` to before `last`.
#[derive(Clone, Copy)]
struct Span {
    row: u32,
    first: u32,
    last: u32,
    edge: u32,
}

/// The coverage of each pixel that edges of two subpaths cross, by its cell in the coverage
/// summed over `area`, in order: `edges` in window pixels, each subpath's ending among them in
/// `ends`; by the even-odd rule, or nonzero.
pub(crate) fn exact(area: &Summed, (edges, ends): (&[[Point; 2]], &[usize]), even_odd: bool) -> Vec<(usize, f32)> {
    let (stride, h) = area.size();
    let mut spans = Vec::new();
    for (n, [a, b]) in edges.iter().enumerate() {
        rows(stride, h, area.near(*a), area.near(*b), |row, x0, x1| {
            let first = x0.floor() as u32;
            spans.push(Span { row: row as u32, first, last: (x1.ceil() as u32).max(first + 1), edge: n as u32 });
        });
    }
    let (spans, starts) = in_rows(spans, h);
    let mut meeting = Meeting { subpath: vec![u32::MAX; stride], met: Vec::new() };
    let mut exact = Vec::new();
    for (y, row) in starts.windows(2).map(|range| &spans[range[0]..range[1]]).enumerate() {
        meeting.mark(row, ends);
        let shares = meeting.worked_out(y, row, (edges, area), even_odd);
        exact.extend(meeting.met.iter().zip(shares).map(|(x, share)| (y * stride + x, share)));
        meeting.clear(row);
    }
    exact
}

/// Spans in the order of their rows (`h` of them), and where each row's begin among them (and,
/// last, where the last ends): counted first, then each put in its place.
fn in_rows(spans: Vec<Span>, h: usize) -> (Vec<Span>, Vec<usize>) {
    let mut starts = vec![0; h + 1];
    spans.iter().for_each(|span| starts[span.row as usize + 1] += 1);
    (1..=h).for_each(|y| starts[y] += starts[y - 1]);
    let (mut next, mut placed) = (starts.clone(), spans.clone());
    for span in spans {
        let at = &mut next[span.row as usize];
        placed[*at] = span;
        *at += 1;
    }
    (placed, starts)
}

/// For each pixel of a row, the subpath whose edges cross it (`u32::MAX`: none); and the
/// pixels another's cross too, in order.
struct Meeting {
    subpath: Vec<u32>,
    met: Vec<usize>,
}

impl Meeting {
    /// Notes the pixels a row's spans cross (`ends`: where each subpath's edges end), and
    /// those two subpaths' edges cross.
    fn mark(&mut self, row: &[Span], ends: &[usize]) {
        let mut found = Vec::new();
        for span in row {
            let subpath = ends.partition_point(|end| *end <= span.edge as usize) as u32;
            let crossed = self.subpath.iter_mut().enumerate().take(span.last as usize).skip(span.first as usize);
            for (x, owner) in crossed {
                found.extend((*owner != u32::MAX && *owner != subpath).then_some(x));
                *owner = subpath;
            }
        }
        found.into_iter().for_each(|x| in_order(&mut self.met, x));
        self.met.dedup();
    }

    /// Forgets a row's spans.
    fn clear(&mut self, row: &[Span]) {
        for span in row {
            self.subpath[span.first as usize..span.last as usize].fill(u32::MAX);
        }
        self.met.clear();
    }

    /// The coverage of each pixel of row `y` where two subpaths meet, edges (in window pixels,
    /// summed over `area`) crossing the row as `row` says. Along each of `LINES` lines across it,
    /// the winding at each such pixel's left side counts the crossings left of it; inside the
    /// pixel, the stretches between its own crossings are inside or not by their winding.
    fn worked_out(&self, y: usize, row: &[Span], (edges, area): (&[[Point; 2]], &Summed), even_odd: bool) -> Vec<f32> {
        if self.met.is_empty() {
            return Vec::new();
        }
        let count = self.met.len();
        let (mut shares, mut turns, mut inner) = (vec![0.0; count], vec![0.0; count + 1], Vec::new());
        let near = |span: &Span| edges[span.edge as usize].map(|end| area.near(end));
        for line in 0..LINES {
            let at = y as f32 + (line as f32 + 0.5) / LINES as f32;
            turns.fill(0.0);
            inner.clear();
            for (x, turn) in row.iter().filter_map(|span| crosses(near(span), at)) {
                // The first pixel right of it, and the one it is in, if two subpaths meet there.
                let next = self.met.partition_point(|met| *met as f32 <= x);
                turns[next] += turn;
                let within = next.checked_sub(1).filter(|before| x < self.met[*before] as f32 + 1.0);
                within.into_iter().for_each(|before| in_order(&mut inner, (before, x, turn)));
            }
            let mut winding = 0.0;
            for (n, share) in shares.iter_mut().enumerate() {
                winding += turns[n];
                *share += self.covered(n, winding, &inner, even_odd) / LINES as f32;
            }
        }
        shares
    }

    /// How much of the `n`th pixel where two subpaths meet is inside along one line across it:
    /// `winding` at its left side, and the crossings inside it among `inner` (in order: the
    /// pixel, where, and how the winding turns).
    fn covered(&self, n: usize, mut winding: f32, inner: &[(usize, f32, f32)], even_odd: bool) -> f32 {
        let first = inner.partition_point(|(at, ..)| *at < n);
        let crossings = inner[first..].iter().take_while(|(at, ..)| *at == n).map(|(_, x, turn)| (*x, *turn));
        let (mut from, mut covered) = (self.met[n] as f32, 0.0);
        for (x, turn) in crossings.chain([(self.met[n] as f32 + 1.0, 0.0)]) {
            covered += if inside(winding, even_odd) { x - from } else { 0.0 };
            (from, winding) = (x, winding + turn);
        }
        covered
    }
}

/// Puts an item into a list in order (after those equal to it).
fn in_order<T: PartialOrd>(list: &mut Vec<T>, item: T) {
    let at = list.partition_point(|other| *other <= item);
    list.insert(at, item);
}

/// Whether a winding is inside: by the even-odd rule, odd; else not zero.
fn inside(winding: f32, even_odd: bool) -> bool {
    if even_odd { winding as i64 % 2 != 0 } else { winding != 0.0 }
}

/// Where an edge crosses the line across at height `at`, and how the winding turns there (1
/// going down, -1 up): a crossing at its top end counts, one at its bottom end does not, so a
/// corner where two edges meet counts once. Left of the buffer counts as its first pixel.
fn crosses([a, b]: [Point; 2], at: f32) -> Option<(f32, f32)> {
    let (turn, top, bottom) = if a.y < b.y { (1.0, a, b) } else { (-1.0, b, a) };
    (top.y <= at && at < bottom.y).then(|| ((top.x + (at - top.y) * (bottom.x - top.x) / (bottom.y - top.y)).max(0.0), turn))
}

#[cfg(test)]
mod tests {
    use crate::canvas::Canvas;
    use crate::color::Color;
    use crate::geom::{Point, pt};

    /// A shape of subpaths (each a list of corners) filled white on 12 by 4 pixels: each
    /// pixel's coverage, by the nonzero rule (or even-odd), exact where they meet or summed.
    fn filled(subpaths: &[&[Point]], even_odd: bool, exact: bool) -> Vec<u32> {
        let (mut edges, mut ends) = (Vec::new(), Vec::new());
        for corners in subpaths {
            edges.extend((0..corners.len()).map(|at| [corners[at], corners[(at + 1) % corners.len()]]));
            ends.push(edges.len());
        }
        let mut pixels = vec![0; 12 * 4];
        let mut canvas = Canvas::new(&mut pixels, 12, 4, (0, 0), 1.0);
        match exact {
            true => canvas.fill_subpaths(&edges, &ends, even_odd, |_, _| Color::WHITE),
            false => canvas.fill_with(&mut |each| edges.iter().for_each(|[a, b]| each(*a, *b)), even_odd, |_, _| Color::WHITE),
        }
        pixels.iter().map(|pixel| pixel >> 24).collect()
    }

    /// A box from `x0` to `x1`, all four rows high, going round clockwise or not.
    fn bar(x0: f32, x1: f32, clockwise: bool) -> Vec<Point> {
        let corners = vec![pt(x0, 0.0), pt(x1, 0.0), pt(x1, 4.0), pt(x0, 4.0)];
        if clockwise { corners } else { corners.into_iter().rev().collect() }
    }

    #[test]
    fn subpaths_going_round_opposite_ways_leave_no_hairline_where_they_meet() {
        // Two boxes meeting halfway across pixel 5: summed, its coverages cancel.
        let (one, other) = (bar(2.0, 5.5, true), bar(5.5, 9.0, false));
        assert_eq!(filled(&[&one, &other], false, false)[5], 0, "summed");
        let exact = filled(&[&one, &other], false, true);
        assert_eq!(&exact[..12], &[0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0]);
        // Slanted, meeting across several pixels: each as one shape would cover it.
        let (one, other) = ([pt(1.0, 0.0), pt(4.0, 0.0), pt(7.0, 4.0), pt(1.0, 4.0)], [pt(4.0, 0.0), pt(10.0, 0.0), pt(10.0, 4.0), pt(7.0, 4.0)]);
        let reversed: Vec<Point> = other.iter().rev().copied().collect();
        let whole = filled(&[&[pt(1.0, 0.0), pt(10.0, 0.0), pt(10.0, 4.0), pt(1.0, 4.0)]], false, false);
        assert_eq!(filled(&[&one, &reversed], false, true), whole);
        // Their tops halfway down the first row: the winding changes down a row too.
        let lowered = |corners: Vec<Point>| corners.into_iter().map(|corner| pt(corner.x, corner.y.max(0.5))).collect::<Vec<Point>>();
        let (one, other) = (lowered(bar(2.0, 5.5, true)), lowered(bar(5.5, 9.0, false)));
        assert_eq!(filled(&[&one, &other], false, true), filled(&[&lowered(bar(2.0, 9.0, true))], false, false));
    }

    #[test]
    fn where_subpaths_overlap_the_rule_decides_as_without_them_meeting() {
        // Overlapping the same way inside pixel 5 (from 5.25 to 5.5): winding twice is inside by
        // the nonzero rule, outside by even-odd.
        let (one, other) = (bar(2.0, 5.5, true), bar(5.25, 9.0, true));
        let row = |even_odd: bool| filled(&[&one, &other], even_odd, true)[..12].to_vec();
        assert_eq!(row(false), [0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0]);
        // Pixel 5 three quarters covered, the next whole: as summed, which is exact there.
        assert_eq!(row(true), filled(&[&one, &other], true, false)[..12].to_vec());
        assert_eq!(row(true)[6], 255);
        // A hole going round the other way, its edge in a pixel with the outline's: as summed.
        let (outer, hole) = (bar(1.0, 11.0, true), bar(1.5, 8.75, false));
        assert_eq!(filled(&[&outer, &hole], false, true), filled(&[&outer, &hole], false, false));
    }
}
