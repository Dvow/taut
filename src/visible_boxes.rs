//! With every box in place: which boxes the pointer can reach, which are in view, and which
//! scroll. Redone only when something moved.
//!
//! Where a box lies, and the part of the window it shows in, follow from its ancestors: a
//! scrolling box moves its content, a clipping box holds it to itself, a box kept in view stays
//! put. `Lying` works that out walking boxes parents first: for the boxes in view, visiting
//! only what can be seen; for the accessibility tree, visiting everything, by the same rules.
use crate::geom::{Rect, rect};
use crate::id::{ById, Id, hash};
use crate::layout::{FROZEN, PAGE, Tree};
use crate::paint::{bar, bar_id};

/// A box in view: where it is in the window, the part of the window it draws on, and a number
/// for where it is (stable while it stays put). Listed in drawing order; a box that holds
/// something back from its content, or draws over it, is listed again after all of it as
/// `leaving`. Of those, a sticky box standing at the start of a scroll area `pins` what follows
/// it in its box to past it (bit 0: right of it, bit 1: below it), and a box in which such boxes
/// did so `lets_go` of that many.
pub(crate) struct VisibleBox {
    pub at: u32,
    pub rect: Rect,
    pub area: Rect,
    pub place: u64,
    pub leaving: bool,
    pub pins: u8,
    pub lets_go: u8,
    /// Also listed as `leaving`: where a see-through box starts (before it) and ends (after all
    /// of it); `area` is all of it that may be seen. Else `No`.
    pub fade: Fade,
}

/// What marks where a see-through box starts (`true`) or ends, as `VisibleBoxes::fade` does.
pub(crate) type FadesHere = fn(&mut VisibleBoxes, &Tree, u32, Rect, Rect, bool);

/// Where a see-through box starts and ends in the list of boxes in view (see
/// [`El::opacity`](crate::El::opacity)): from its start, what is under it is kept; at its end,
/// that shows through what was drawn of it, by its `fade`. One byte: a box in view takes 48.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Fade {
    No,
    Starts,
    Ends,
}

/// A scrolling box in view: where it is, and the part of it that can be seen.
pub(crate) struct Scroller {
    pub at: u32,
    pub id: Id,
    pub rect: Rect,
    pub visible: Rect,
}

/// Where boxes lie as they are walked, parents first: for the boxes entered, innermost last,
/// the origin their content counts from and the part of the window it may be seen in; and for
/// each that sticky boxes cut short (see `pin`), its depth and how many did.
#[derive(Default)]
pub(crate) struct Lying {
    corners: Vec<[f32; 2]>,
    clips: Vec<Rect>,
    pins: Vec<(usize, u8)>,
}

impl Lying {
    /// Starts at a root box.
    pub fn start(&mut self) {
        self.corners.clear();
        self.corners.push([0.0; 2]);
        self.clips.clear();
        self.clips.push(Rect::EVERYTHING);
        self.pins.clear();
    }

    /// Where box `at` lies, its parent being the last box entered; and the part of the window
    /// it may be seen in.
    #[inline(always)]
    pub fn lies(&self, tree: &Tree, at: u32) -> (Rect, Rect) {
        let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
        let (corner, clip) = (self.corners[self.corners.len() - 1], self.clips[self.clips.len() - 1]);
        let r = rect(corner[0] + place.pos[0], corner[1] + place.pos[1], place.dim[0], place.dim[1]);
        if spec.frame.pinned == 0 {
            return (r, clip);
        }
        // A sticky box stands in what scrolls, like the rest; a kept one in all its parent's view.
        let [x, y] = tree.stuck(at);
        (r.moved(x, y), if spec.frame.pinned & FROZEN != 0 { self.kept_clip(tree, at) } else { clip })
    }

    /// Enters box `at`, lying at `r` and visible within `clip`: its content counts from its
    /// corner less its scroll, and may be seen where it shows content, if it clips.
    #[inline(always)]
    pub fn go_in(&mut self, tree: &Tree, at: u32, r: Rect, clip: Rect) {
        let spec = &tree.specs[at as usize];
        self.corners.push([r.x - spec.offset[0], r.y - spec.offset[1]]);
        // A scrolling page clips its content to the window vertically, as any scroll box does:
        // what lies past its edge is out of view. (Not sideways: what lies past the right edge
        // keeps its place there, which apps measure by.)
        let clip = match () {
            _ if spec.frame.clip => tree.scrolled_view(at, r).intersect(clip),
            _ if at == PAGE && page_scrolls(tree) => rect(clip.x, r.y, clip.w, r.h).intersect(clip),
            _ => clip,
        };
        self.clips.push(clip);
    }

    /// Leaves box `at`: where it lies, and how many sticky boxes in it cut short what shows of
    /// it (released now).
    #[inline(always)]
    pub fn go_out(&mut self, tree: &Tree, at: u32) -> (Rect, u8) {
        let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
        let corner = self.corners.pop().unwrap_or_default();
        let depth = self.clips.len() - 1;
        self.clips.pop();
        let pinned = self.pins.pop_if(|(deep, _)| *deep == depth).map_or(0, |(_, count)| count);
        (rect(corner[0] + spec.offset[0], corner[1] + spec.offset[1], place.dim[0], place.dim[1]), pinned)
    }

    /// The part of the window box `at` may be seen in, once left.
    pub fn clip_of(&self, tree: &Tree, at: u32) -> Rect {
        if tree.specs[at as usize].frame.pinned & FROZEN != 0 { self.kept_clip(tree, at) } else { self.clips[self.clips.len() - 1] }
    }

    /// Box `at` was just left, lying at `r`: if it is a sticky box standing past where it would
    /// have scrolled to, what follows it in its box shows only past it (right of it, below it,
    /// or both), scrolling under it. Says which ways it does, as `VisibleBox::pins`. (Only
    /// checked once a box has stuck: see `Sticks`.)
    #[inline(always)]
    pub fn pin(&mut self, tree: &Tree, at: u32, r: Rect) -> u8 {
        tree.sticks.map_or(0, |sticks| (sticks.pin)(self, tree, at, r))
    }
    /// What `pin` does, once a box has stuck.
    pub(crate) fn pin_past(&mut self, tree: &Tree, at: u32, r: Rect) -> u8 {
        let ([x, y], depth) = (tree.sticky_shift(at), self.clips.len() - 1);
        let pins = (x > 0.0) as u8 | ((y > 0.0) as u8) << 1;
        if pins == 0 {
            return 0;
        }
        self.clips[depth] = self.clips[depth].intersect(past(pins, r));
        match self.pins.last_mut() {
            Some((deep, count)) if *deep == depth => *count += 1,
            _ => self.pins.push((depth, 1)),
        }
        pins
    }

    /// The part of the window a box kept in view may be seen in, its parent being the last box
    /// entered: all of the parent's view, not just where its scrolled content shows.
    fn kept_clip(&self, tree: &Tree, at: u32) -> Rect {
        let (up, corner) = (tree.specs[at as usize].frame.parent, self.corners[self.corners.len() - 1]);
        let (around, dim) = (&tree.specs[up as usize], tree.places[up as usize].dim);
        let room = rect(corner[0] + around.offset[0], corner[1] + around.offset[1], dim[0], dim[1]);
        // What the parent itself may be seen in is the last but one clip kept.
        let outer = self.clips.len().checked_sub(2).map_or(Rect::EVERYTHING, |before| self.clips[before]);
        tree.view(up, room).intersect(outer)
    }
}

/// Where what follows a stuck box at `r` may show, as `pins` (see `VisibleBox::pins`) says: right
/// of it, below it, or both.
pub(crate) fn past(pins: u8, r: Rect) -> Rect {
    let from = |axis: usize, edge: f32, anywhere: f32| if pins & 1 << axis != 0 { edge } else { anywhere };
    rect(from(0, r.right(), Rect::EVERYTHING.x), from(1, r.bottom(), Rect::EVERYTHING.y), Rect::EVERYTHING.w, Rect::EVERYTHING.h)
}

/// Whether the page has more content than the window, either way.
fn page_scrolls(tree: &Tree) -> bool {
    tree.overflows(PAGE, 0) || tree.overflows(PAGE, 1)
}

#[derive(Default)]
pub(crate) struct VisibleBoxes {
    /// What marks where a see-through box starts and ends (see `Fade`): set by the first such
    /// box (`El::opacity`), reached by nothing else.
    pub fades: Option<FadesHere>,
    /// Boxes that sense the pointer, bottom to top, with the part of each that can be seen.
    pub hits: Vec<(Id, Rect)>,
    /// The same by id.
    pub rects: ById<Rect>,
    /// Per layer, lowest first: the index of its first box in `hits`, and its id.
    pub layers: Vec<(usize, Id)>,
    /// Each layer's first box, lowest first; and which of them is the first screen readers get:
    /// the topmost modal layer (a dialog), or the lowest.
    pub roots: Vec<u32>,
    pub reader_root: usize,
    /// The height of the layer being walked.
    height: u64,
    pub boxes: Vec<VisibleBox>,
    pub scrollers: Vec<Scroller>,
    /// How many scrollers lie under a layer that keeps the pointer from them.
    pub covered: usize,
    /// How many boxes in view, the first in the list, lie under a modal layer (a dialog).
    pub under: usize,
    /// Where the boxes being entered lie; and where all do, for screen readers (kept so its
    /// memory is reused).
    lying: Lying,
    pub reader_lying: Lying,
    /// What the accessibility tree was last made from: one while a screen reader listens, else
    /// none.
    pub access_cache: Vec<crate::accessibility::AccessCache>,
    open: Vec<u32>,
}

impl VisibleBoxes {
    /// Walks every layer, bottom to top: its root, whether it keeps the pointer from what is
    /// under it, whether it is a dialog, and its height.
    pub fn take(&mut self, tree: &Tree, layers: impl Iterator<Item = (u32, bool, bool, u64)>) {
        self.hits.clear();
        self.rects.clear();
        self.boxes.clear();
        self.scrollers.clear();
        self.layers.clear();
        self.roots.clear();
        self.reader_root = 0;
        let mut open = std::mem::take(&mut self.open);
        for (root, blocks, dialog, height) in layers {
            self.layers.push((self.hits.len(), tree.specs[root as usize].id));
            self.reader_root = if dialog { self.roots.len() } else { self.reader_root };
            self.roots.push(root);
            self.height = height;
            self.covered = if blocks { self.scrollers.len() } else { self.covered.min(self.scrollers.len()) };
            self.under = if dialog { self.boxes.len() } else { self.under.min(self.boxes.len()) };
            self.lying.start();
            tree.walk(root, &mut open, &mut |at, entering| match entering {
                true => self.enter(tree, at),
                false => self.leave(tree, at),
            });
        }
        self.open = open;
    }

    /// Takes a box in. Says whether any of it or its content can be seen.
    fn enter(&mut self, tree: &Tree, at: u32) -> bool {
        let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
        let (r, clip) = self.lying.lies(tree, at);
        // A hidden box and its content are skipped as out of view.
        if place.reach.moved(r.x, r.y).intersect(clip).is_empty() || spec.fade == 255 {
            return false;
        }
        if spec.fade != 0
            && let Some(fades) = self.fades
        {
            fades(self, tree, at, r, clip, true);
        }
        // A see-through box is redrawn with all its content: its look is theirs.
        let drawn = if spec.fade != 0 { place.reach } else { tree.drawn(at) };
        let (seen, area) = (r.intersect(clip), drawn.moved(r.x, r.y).intersect(clip));
        if spec.frame.senses && !seen.is_empty() {
            self.hits.push((spec.id, seen));
            self.rects.insert(spec.id, seen);
        }
        // The page scrolls too, where its content is larger than the window.
        if spec.frame.scroll != [false; 2] || at == PAGE && page_scrolls(tree) {
            self.scrollers.push(Scroller { at, id: spec.id, rect: r, visible: seen });
        }
        if !area.is_empty() {
            let bits = |r: Rect| [r.x.to_bits(), r.y.to_bits(), r.w.to_bits(), r.h.to_bits()];
            // A layer that moves up or down looks new wherever it is. Where content starts in a
            // box, and whether it is clipped, count too: a box can keep its place and size
            // while they change.
            let (pad, cut) = (spec.frame.pad, spec.frame.clip);
            let inside = [pad.left.to_bits(), pad.top.to_bits(), pad.right.to_bits(), pad.bottom.to_bits()];
            self.boxes.push(VisibleBox { at, rect: r, area, place: hash((bits(r), bits(clip), self.height, inside, cut)), leaving: false, pins: 0, lets_go: 0, fade: Fade::No });
        }
        self.lying.go_in(tree, at, r, clip);
        true
    }

    /// Leaves a box. Its scroll bars lie over its content, so the pointer reaches a bar first.
    fn leave(&mut self, tree: &Tree, at: u32) -> bool {
        let spec = &tree.specs[at as usize];
        let (r, lets_go) = self.lying.go_out(tree, at);
        let clip = self.lying.clip_of(tree, at);
        let bars = (0..2).filter_map(|axis| bar(tree, at, r, axis).map(|(strip, ..)| (bar_id(spec.id, axis), strip.intersect(clip))));
        self.hits.extend(bars);
        // After its content come its scroll bar and focus ring, and its clip is released (or,
        // for a box kept in view, its view): drawing must pass here again. So it must where
        // headings in it stood at the top of what scrolls, holding what followed them below
        // them; and for such a heading, from here on.
        let again = spec.frame.clip || spec.frame.focus || spec.frame.scroll != [false; 2] || spec.frame.pinned != 0 || at == PAGE && page_scrolls(tree);
        let shows = again && !tree.drawn(at).moved(r.x, r.y).intersect(clip).is_empty();
        let pins = self.lying.pin(tree, at, r);
        if shows || pins != 0 || lets_go > 0 {
            self.boxes.push(VisibleBox { at, rect: r, area: r.intersect(clip), place: 0, leaving: true, pins, lets_go, fade: Fade::No });
        }
        if spec.fade != 0
            && let Some(fades) = self.fades
        {
            fades(self, tree, at, r, clip, false);
        }
        false
    }

    /// Where box `at`, lying at `r` and drawn see-through, `starts` (before it: all that may be
    /// seen of it and its content) or ends (after all of that).
    pub(crate) fn fade(&mut self, tree: &Tree, at: u32, r: Rect, clip: Rect, starts: bool) {
        let (area, fade) = if starts { (tree.places[at as usize].reach.moved(r.x, r.y).intersect(clip), Fade::Starts) } else { (r.intersect(clip), Fade::Ends) };
        self.boxes.push(VisibleBox { at, rect: r, area, place: 0, leaving: true, pins: 0, lets_go: 0, fade });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_in_view_is_listed_in_no_more_than_it_takes() {
        // Every box in view is listed once or more per walk: a see-through box's two marks went
        // into spare bytes.
        assert_eq!((size_of::<VisibleBox>(), size_of::<Fade>()), (48, 1));
    }
}
