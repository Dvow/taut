//! Large trees (folders, outlines, grouped settings): only visible rows cost anything, however
//! many items.
//!
//! ```ignore
//! for row in ui.branches(&mut app.folders, 22.0, |under| app.disk.items_under(under)) {
//!     ui.branch(&mut app.folders, row); // its arrow and its name; more may follow in the row
//!     ui.close();
//! }
//! ui.close();
//! ```
//!
//! The app supplies items on demand: the top level when first described, an item's children
//! when it is expanded. Each is the app's id for the item, its name, and whether it has
//! children. [`Branches`] keeps the visible items in one top-to-bottom list, so a run costs the
//! rows in view, and expanding or collapsing costs the rows that come or go (and moving those
//! after). An app whose items change calls `renew`; expanded items stay expanded. For a small
//! hand-written tree, see [`ui.tree`](Ui::tree).
//!
//! Keys: Up/Down move through visible items; Right expands, or goes to the first child; Left
//! collapses, or goes to the parent; Home/End to the first and last; a letter to the next item
//! starting with it. Moving selects: with Shift, a range from the anchor; with Ctrl, focus
//! only. Space selects like a click with the same modifiers (Ctrl+Space toggles). The pointer
//! selects as in any list. A double click or the arrow expands and collapses.
//!
//! Screen readers get a multi-select tree and its visible items, each with level, position
//! among siblings, expanded and selected state (see `told`), and act through the same keys.
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::layout::Align;
use crate::picks::Picks;
use crate::told::Role;
use crate::ui::{Response, Ui};
use crate::widgets::{DOWN, RIGHT};
use std::collections::BinaryHeap;
use std::ops::Range;

/// Tells a tree's scroll box, and a row's arrow, from the rest.
const BODY: &str = "tree";
const ARROW: &str = "arrow";
/// Indent per level.
const STEP: f32 = 16.0;

/// A tree item as the app supplies it.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    /// The app's id for the item.
    pub item: usize,
    /// Its name: the row's text, and what typed letters find it by.
    pub name: String,
    /// It has children: an arrow, and expands to show them. (One that expands to nothing loses
    /// its arrow.)
    pub opens: bool,
}

impl Branch {
    /// An item without children.
    pub fn new(item: usize, name: impl Into<String>) -> Branch {
        Branch { item, name: name.into(), opens: false }
    }
    /// An item with children, which the app supplies when it is expanded.
    pub fn opening(item: usize, name: impl Into<String>) -> Branch {
        Branch { opens: true, ..Branch::new(item, name) }
    }
}

/// A visible tree item, as the tree keeps it.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    item: usize,
    name: String,
    /// Level: 0 at the top.
    depth: u32,
    /// It has children, and they show.
    opens: bool,
    open: bool,
    /// Position among its siblings, from 1, and their count.
    place: [u32; 2],
}

/// Children supplied under one item (or the top level) as visible items at `depth`, with their
/// positions: last first, to be popped.
fn placed(came: Vec<Branch>, depth: u32) -> Vec<Shown> {
    let count = came.len() as u32;
    let each = came.into_iter().zip(1..count + 1).map(|(branch, nth)| Shown { item: branch.item, name: branch.name, depth, opens: branch.opens, open: false, place: [nth, count] });
    each.rev().collect()
}

/// A tree the user expands, collapses and selects in, kept by the app (one per tree) beside its
/// own items: which are visible, which selected, and where focus is going.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branches {
    /// Visible items, top to bottom.
    shown: Vec<Shown>,
    /// Selected items, by the app's ids.
    pub picks: Picks,
    /// The last item selected alone: where a Shift range starts.
    anchor: usize,
    /// Items to expand as soon as they show.
    reopen: Vec<usize>,
    /// The top level has been asked for.
    asked: bool,
    /// To do before the rows are described again: a row to expand (`true`) or collapse; and the
    /// row focus goes to.
    turn: Option<(usize, bool)>,
    going: Option<usize>,
}

/// What moving focus does to the selection.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Follow {
    /// That item alone is selected.
    Alone,
    /// The range from the anchor to it is selected.
    Stretch,
    /// Nothing changes.
    Not,
}

impl Branches {
    /// A tree whose items with these ids are expanded as soon as they show.
    pub fn with_open(items: &[usize]) -> Branches {
        Branches { reopen: items.to_vec(), ..Default::default() }
    }
    /// Visible items.
    pub fn len(&self) -> usize {
        self.shown.len()
    }
    /// Whether no item is visible.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }
    /// The app's id for the item in `row`.
    pub fn item(&self, row: usize) -> usize {
        self.shown[row].item
    }
    /// Asks again for all items on the next run: the app's items changed. Expanded items stay
    /// expanded.
    pub fn renew(&mut self) {
        self.reopen.extend(self.shown.iter().filter(|shown| shown.open).map(|shown| shown.item));
        (self.shown, self.asked) = (Vec::new(), false);
    }

    /// Asks for the top level, and the children of each item to be expanded, depth first.
    /// (`todo` is the stack still to be listed.)
    fn ask_all(&mut self, under: &mut dyn FnMut(Option<usize>) -> Vec<Branch>) {
        self.reopen = BinaryHeap::from(std::mem::take(&mut self.reopen)).into_sorted_vec();
        let mut todo = placed(under(None), 0);
        self.shown.clear();
        while let Some(mut shown) = todo.pop() {
            if shown.opens && self.reopen.binary_search(&shown.item).is_ok() {
                let came = placed(under(Some(shown.item)), shown.depth + 1);
                (shown.opens, shown.open) = (!came.is_empty(), !came.is_empty());
                todo.extend(came);
            }
            self.shown.push(shown);
        }
        (self.asked, self.reopen) = (true, Vec::new());
    }

    /// Expands or collapses the item in a row. (Sibling positions stay.)
    fn turn_row(&mut self, at: usize, open: bool, under: &mut dyn FnMut(Option<usize>) -> Vec<Branch>) {
        match self.shown.get(at) {
            Some(shown) if shown.opens && shown.open != open => {}
            _ => return,
        }
        if open {
            self.open_row(at, under);
        } else {
            self.close_row(at);
        }
    }
    /// Shows the children of the item in a row, under it. (None come: it has none.)
    fn open_row(&mut self, at: usize, under: &mut dyn FnMut(Option<usize>) -> Vec<Branch>) {
        let came = placed(under(Some(self.shown[at].item)), self.shown[at].depth + 1);
        (self.shown[at].opens, self.shown[at].open) = (!came.is_empty(), !came.is_empty());
        self.shown.splice(at + 1..at + 1, came.into_iter().rev());
    }
    /// Removes all visible descendants of the item in a row.
    fn close_row(&mut self, at: usize) {
        let depth = self.shown[at].depth;
        let end = (at + 1..self.shown.len()).find(|row| self.shown[*row].depth <= depth).unwrap_or(self.shown.len());
        self.shown.drain(at + 1..end);
        self.shown[at].open = false;
    }

    /// The row of the parent of the item in `row`. (Itself, at the top level.)
    fn above(&self, row: usize) -> usize {
        let depth = self.shown[row].depth;
        (0..row).rev().find(|above| self.shown[*above].depth < depth).unwrap_or(row)
    }
    /// The next row after `row` whose name starts with `letter` (any case), wrapping to the
    /// top.
    fn named(&self, row: usize, letter: char) -> Option<usize> {
        let small: Vec<char> = letter.to_lowercase().collect();
        let begins = |at: &usize| self.shown[*at].name.chars().flat_map(char::to_lowercase).take(small.len()).eq(small.iter().copied());
        (row + 1..self.shown.len()).chain(0..row).find(begins)
    }

    /// Moves focus to a row; the selection follows as `follow` says. Returns whether it
    /// changed.
    fn go(&mut self, to: usize, follow: Follow) -> bool {
        self.going = Some(to);
        match follow {
            Follow::Alone => self.pick_alone(to),
            Follow::Stretch => self.stretch(to, false),
            Follow::Not => return false,
        }
        true
    }
    /// A click on a row with `mods`: selects it alone; with Ctrl, toggles it; with Shift,
    /// selects the range from the anchor.
    fn click(&mut self, row: usize, mods: Mods) {
        match (mods.ctrl, mods.shift) {
            (_, true) => self.stretch(row, mods.ctrl),
            (true, false) => self.flip(row),
            (false, false) => self.pick_alone(row),
        }
    }
    fn pick_alone(&mut self, row: usize) {
        let item = self.shown[row].item;
        self.picks.clear();
        self.picks.add(item..item + 1);
        self.anchor = item;
    }
    /// Toggles a row's selection, and moves the anchor there.
    fn flip(&mut self, row: usize) {
        let item = self.shown[row].item;
        if self.picks.has(item) {
            self.picks.remove(item);
        } else {
            self.picks.add(item..item + 1);
        }
        self.anchor = item;
    }
    /// Selects every row from the anchor to `to` (from `to` alone, if the anchor is no longer
    /// visible); adds to the selection with `more`. Lowest first, so a range of 100,000 rows is
    /// selected in one pass.
    fn stretch(&mut self, to: usize, more: bool) {
        let from = self.shown.iter().position(|shown| shown.item == self.anchor).unwrap_or(to);
        if !more {
            self.picks.clear();
        }
        let items = BinaryHeap::from_iter(self.shown[from.min(to)..=from.max(to)].iter().map(|shown| shown.item));
        items.into_sorted_vec().into_iter().for_each(|item| self.picks.add(item..item + 1));
    }
}

/// Trees of the app's own items, of any size: only visible rows are described.
impl Ui {
    /// Opens a large tree the user expands, collapses and selects in, in its own scroll box;
    /// returns the visible rows, each `row` units high. Describe each with
    /// [`branch`](Self::branch), then [`close`](Self::close) the tree.
    ///
    /// `under` supplies the children of an item by id (`None`: the top level), on the first run
    /// and when an item is expanded. `tree` keeps what is visible, the selection (`tree.picks`)
    /// and focus.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn branches(&mut self, tree: &mut Branches, row: f32, mut under: impl FnMut(Option<usize>) -> Vec<Branch>) -> Range<usize> {
        if !tree.asked {
            tree.ask_all(&mut under);
        }
        if let Some((at, open)) = tree.turn.take() {
            tree.turn_row(at, open, &mut under);
        }
        let all = self.column().key(BODY).grow().scroll().open();
        self.tell(&all, Role::Tree, Some(true), "");
        self.join_next = true;
        // The rows' room in the last layout: what `rows` measures them by.
        let room = self.rooms.get(&self.next_id()).map_or(0.0, |read| read[1]);
        let rows = self.rows(tree.shown.len(), row);
        // Focus moves to its target row if in view; else the tree scrolls to it, and focus
        // moves next run.
        match tree.going.take().filter(|to| *to < tree.shown.len()) {
            Some(to) if rows.contains(&to) => self.set_focus(self.id(tree.shown[to].item)),
            Some(to) => {
                let (was, top) = (self.scrolls.get(&all.id).copied().unwrap_or_default(), to as f32 * row);
                self.scrolls.insert(all.id, [was[0], was[1].min(top).max(top + row - room)]);
                (tree.going, self.again) = (Some(to), true);
            }
            None => {}
        }
        rows
    }

    /// A tree row: indented by level, an arrow if it has children (pressing toggles), and the
    /// name. Opens the row: describe more after the name, then close it. `clicked` is set when
    /// pressed; `changed` when the selection changed.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn branch(&mut self, tree: &mut Branches, row: usize) -> Response {
        let t = self.theme;
        let Some(&Shown { item, depth, place, .. }) = tree.shown.get(row) else { return self.row().open() };
        let mut state = self.press(self.id(item));
        if state.focused {
            state.changed |= self.walk_branches(tree, row);
        }
        // A press selects as a click with the same modifiers: Enter and Space alone, Ctrl+Space
        // toggles, Shift+Space a range.
        if state.clicked {
            tree.click(row, self.mods());
            state.changed = true;
        }
        let (opens, open) = (tree.shown[row].opens, tree.shown[row].open);
        if state.double_clicked && opens {
            (tree.turn, self.again) = (Some((row, !open)), true);
        }
        let picked = tree.picks.has(item);
        let fill = if picked {
            t.selected
        } else if state.hovered {
            t.hover
        } else {
            Color::NONE
        };
        self.row().key(item).pad((6.0 + STEP * depth as f32, 0.0, 6.0, 0.0)).gap(4.0).align(Align::Start, Align::Center).fill(fill).radius(4.0).sense().focusable().open();
        self.tell(&state, Role::TreeItem, Some(picked), "");
        self.tell_place(place[0] as usize, place[1] as usize);
        self.tell_branch(depth as usize + 1, opens.then_some(open));
        let arrow = self.interact(self.id(ARROW));
        if arrow.clicked && opens {
            (tree.turn, self.again) = (Some((row, !open)), true);
        }
        let mark = self.el().key(ARROW).size(16.0, 16.0).align(Align::Center, Align::Center);
        match opens {
            true => mark.icon(if open { DOWN } else { RIGHT }, 12.0, if arrow.hovered { t.text } else { t.text_dim }).sense().show(),
            false => mark.show(),
        };
        self.text(&tree.shown[row].name, t.font, t.text);
        state
    }

    /// Keys while focus is on `row`: expand, collapse, move focus, select. Returns whether the
    /// selection changed. (Up/Down at the ends, and Home/End on their row, are left to move
    /// focus out or scroll, as everywhere.)
    fn walk_branches(&mut self, tree: &mut Branches, row: usize) -> bool {
        let (last, depth, opens, open) = (tree.shown.len() - 1, tree.shown[row].depth, tree.shown[row].opens, tree.shown[row].open);
        let (right, left) = (self.key(Key::Right, Mods::NONE), self.key(Key::Left, Mods::NONE));
        if right && opens && !open || left && open {
            (tree.turn, self.again) = (Some((row, right)), true);
            return false;
        }
        // Into an expanded item, out of any other; up, down, to the ends; and on by letter.
        let (none, shift, ctrl) = (Mods::NONE, Mods::SHIFT, Mods::CTRL);
        let into = (right && open).then_some((row + 1, Follow::Alone));
        let out = (left && depth > 0).then(|| (tree.above(row), Follow::Alone));
        let ways = [
            (Key::Down, none, row + 1, Follow::Alone),
            (Key::Up, none, row.wrapping_sub(1), Follow::Alone),
            (Key::Down, shift, row + 1, Follow::Stretch),
            (Key::Up, shift, row.wrapping_sub(1), Follow::Stretch),
            (Key::Down, ctrl, row + 1, Follow::Not),
            (Key::Up, ctrl, row.wrapping_sub(1), Follow::Not),
            (Key::Home, none, 0, Follow::Alone),
            (Key::End, none, last, Follow::Alone),
            (Key::Home, shift, 0, Follow::Stretch),
            (Key::End, shift, last, Follow::Stretch),
        ];
        let way = ways.into_iter().find(|(key, mods, to, _)| *to <= last && *to != row && self.key(*key, *mods)).map(|(_, _, to, follow)| (to, follow));
        let typed = if way.is_none() { self.typed() } else { String::new() };
        let named = typed.chars().find(|letter| !letter.is_whitespace()).and_then(|letter| tree.named(row, letter)).map(|to| (to, Follow::Alone));
        let Some((to, follow)) = into.or(out).or(way).or(named) else { return false };
        self.again = true;
        tree.go(to, follow)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tree as the library's parts describe it, for screen reader tests: ten folders, the
    /// second with three notes, the others empty.
    pub(crate) fn grove(ui: &mut Ui, tree: &mut Branches) {
        let under = |under: Option<usize>| match under {
            None => (0..10).map(|n| Branch::opening(n, format!("folder {n}"))).collect(),
            Some(1) => (10..13).map(|n| Branch::new(n, format!("note {n}"))).collect(),
            Some(_) => Vec::new(),
        };
        for row in ui.branches(tree, 22.0, under) {
            ui.branch(tree, row);
            ui.close();
        }
        ui.close();
    }
    /// That tree in a UI with a screen reader listening, the second folder expanded and its
    /// second note selected, after two runs.
    pub(crate) fn grown() -> (Ui, Branches) {
        let mut ui = Ui::new(Box::new(crate::text::BuiltInFont), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 300.0, h: 400.0 }, 1.0);
        ui.listen(true);
        let mut tree = Branches::with_open(&[1]);
        tree.picks.add(11..12);
        (0..2).for_each(|_| _ = ui.run(&mut |ui| grove(ui, &mut tree)));
        (ui, tree)
    }

    /// A folder tree: ten at the top, each with three folders and a file; each of those folders
    /// with two files, except the third, which is empty. Ids say where they are: 1 to 10 at the
    /// top, then the parent's id times 10 plus 1, 2, ….
    fn folders(under: Option<usize>) -> Vec<Branch> {
        match under {
            None => (1..=10).map(|n| Branch::opening(n, format!("folder {n}"))).collect(),
            Some(top) if top <= 10 => (1..=3).map(|n| Branch::opening(top * 10 + n, format!("folder {top}.{n}"))).chain([Branch::new(top * 10 + 9, "notes")]).collect(),
            Some(inner) if inner % 10 == 3 => Vec::new(),
            Some(inner) => (1..=2).map(|n| Branch::new(inner * 10 + n, format!("file {n}"))).collect(),
        }
    }
    /// What is visible: each item's id, level, and whether expanded.
    fn showing(tree: &Branches) -> Vec<(usize, u32, bool)> {
        tree.shown.iter().map(|shown| (shown.item, shown.depth, shown.open)).collect()
    }

    #[test]
    fn items_come_as_they_open_and_go_as_they_close() {
        let (mut tree, asked) = (Branches::default(), std::cell::RefCell::new(Vec::new()));
        let mut under = |item: Option<usize>| (asked.borrow_mut().push(item), folders(item)).1;
        tree.ask_all(&mut under);
        tree.turn_row(1, true, &mut under);
        tree.turn_row(3, true, &mut under);
        // Only what was expanded was asked for.
        assert_eq!(*asked.borrow(), [None, Some(2), Some(22)]);
        let open: Vec<_> = showing(&tree).into_iter().take(9).collect();
        assert_eq!(open, [(1, 0, false), (2, 0, true), (21, 1, false), (22, 1, true), (221, 2, false), (222, 2, false), (23, 1, false), (29, 1, false), (3, 0, false)]);
        // Each one's position among its siblings, and their count.
        let places: Vec<[u32; 2]> = tree.shown.iter().take(9).map(|shown| shown.place).collect();
        assert_eq!(places, [[1, 10], [2, 10], [1, 4], [2, 4], [1, 2], [2, 2], [3, 4], [4, 4], [3, 10]]);
        // Collapsing removes all below; one that expands to nothing has no children from then
        // on; a missing row, or one with nothing to expand, does nothing.
        tree.turn_row(1, false, &mut under);
        assert_eq!((tree.len(), tree.shown[1].place, tree.shown[2].item), (10, [2, 10], 3));
        tree.turn_row(1, true, &mut under);
        tree.turn_row(4, true, &mut under);
        assert!(!tree.shown[4].opens && !tree.shown[4].open && tree.shown[4].item == 23);
        let before = tree.clone();
        tree.turn_row(99, true, &mut under);
        tree.turn_row(4, true, &mut under);
        tree.turn_row(5, true, &mut under);
        assert_eq!(tree, before);
        assert_eq!(asked.borrow().len(), 5, "an item with nothing under it is not asked again");
    }

    #[test]
    fn a_tree_asked_again_keeps_what_was_open_open() {
        let mut tree = Branches::with_open(&[3, 32]);
        tree.ask_all(&mut folders);
        assert_eq!(showing(&tree)[2..8], [(3, 0, true), (31, 1, false), (32, 1, true), (321, 2, false), (322, 2, false), (33, 1, false)]);
        tree.turn_row(0, true, &mut folders);
        tree.renew();
        assert!(tree.is_empty() && !tree.asked);
        tree.ask_all(&mut folders);
        assert_eq!(showing(&tree).iter().filter(|(_, _, open)| *open).map(|(item, ..)| *item).collect::<Vec<_>>(), [1, 3, 32]);
    }

    #[test]
    fn picks_go_by_the_items_that_show() {
        let mut tree = Branches::with_open(&[1]);
        tree.ask_all(&mut folders);
        let picked = |tree: &Branches| tree.picks.iter().collect::<Vec<_>>();
        tree.click(1, Mods::NONE);
        tree.click(5, Mods::SHIFT);
        // From item 11 to the row of item 2: the rows between, whatever their ids.
        assert_eq!(picked(&tree), [2, 11, 12, 13, 19]);
        tree.click(0, Mods::CTRL);
        tree.click(2, Mods::CTRL);
        assert_eq!(picked(&tree), [1, 2, 11, 13, 19]);
        // Shift ranges from the last selected alone or with Ctrl: item 12 now.
        tree.click(4, Mods { shift: true, ctrl: true, alt: false });
        assert_eq!(picked(&tree), [1, 2, 11, 12, 13, 19]);
        assert!(tree.go(6, Follow::Alone) && picked(&tree) == [3] && tree.going == Some(6));
        assert!(!tree.go(0, Follow::Not) && picked(&tree) == [3]);
        assert!(tree.go(7, Follow::Stretch) && picked(&tree) == [3, 4]);
    }

    #[test]
    fn a_letter_finds_the_next_item_that_begins_with_it() {
        let mut tree = Branches::default();
        tree.ask_all(&mut |under| match under {
            None => ["Alpha", "beta", "Bravo", "Échelle", "alto"].iter().enumerate().map(|(n, name)| Branch::new(n, *name)).collect(),
            Some(_) => Vec::new(),
        });
        assert_eq!([tree.named(0, 'b'), tree.named(1, 'B'), tree.named(2, 'b'), tree.named(4, 'a'), tree.named(0, 'é'), tree.named(0, 'z')], [Some(1), Some(2), Some(1), Some(0), Some(3), None]);
        assert_eq!([tree.above(0), tree.above(3)], [0, 3]);
    }
}
