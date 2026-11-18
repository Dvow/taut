//! Docks: panels sharing a box's room side by side, stacked, or as tabs. The user rearranges
//! them: a tab dragged along its strip reorders; dragged off, it drops on a target shown over a
//! place (left, right, above, below, or centre to join its tabs) or on a tab strip; dropped
//! elsewhere, its panel floats. Double-clicking a tab floats its panel; double-clicking a
//! floating panel's title docks it back. Splitters resize; a button on each strip maximizes the
//! front panel within the dock; closable panels have a close button on their tab. The layout is
//! a plain value the app keeps:
//!
//! ```ignore
//! // in the app's state:
//! //   dock: Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Preview"]).above(Dock::of(&["Log"]), 0.7), 0.25)
//! ui.dock(&mut app.dock);
//! if ui.docked(&mut app.dock, "Files") {
//!     ui.label("what the panel holds");
//!     ui.close();
//! }
//! if ui.docked_closable(&mut app.dock, "Log") { .. }
//! // ... the other panels, in any order ...
//! ui.close();
//! ```
//!
//! Keys, as in other programs: Ctrl+Tab brings the next panel of a place to the front and
//! focuses it, Ctrl+F6 focuses the next place, Shift reverses both, Ctrl+F4 closes a closable
//! panel.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::{Id, hash};
use crate::layout::{Align, Flow, Length};
use crate::panel::{LEAST, Panel};
use crate::ui::{Anchor, Icon, Measure, Response, Ui};
use crate::widgets::CROSS;

/// Splitter thickness, tab strip height, and drop target size (and spacing).
const BAR: f32 = 5.0;
const TABS: f32 = 30.0;
const MARK: [f32; 2] = [30.0, 36.0];
/// How far the pointer may stray from a tab strip while reordering a tab: further, and the tab
/// drags its panel.
const STRAY: f32 = 12.0;
/// Tells the drop target layer's id from the dock's.
const MARKS: Id = 0xd0c7_d0c7_d0c7_d0c7;
/// A place's centre: a panel dropped there joins its tabs. (Sides are 0 to 3: left, top, right,
/// bottom.)
const MIDDLE: usize = 4;
/// How far a key moves a splitter, as a fraction of the room.
const STEP: f32 = 0.05;
/// The button that maximizes a panel within its dock, and the one that restores it.
const FILL: Icon = Icon::line("M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10");
const BACK: Icon = Icon::line("M6 2.5V6H2.5M13.5 6H10V2.5M10 13.5V10h3.5M2.5 10H6v3.5");

/// One node of a dock's layout.
#[derive(Clone, Debug, PartialEq)]
pub enum DockNode {
    /// Stacked panels, chosen by tabs.
    Tabs {
        /// Panel names.
        names: Vec<String>,
        /// The one in front.
        front: usize,
    },
    /// Two places sharing this one's room.
    Split {
        /// Side by side; else one above the other.
        across: bool,
        /// The first one's share of the room, 0 to 1.
        share: f32,
        /// Index of the first place: left or top.
        first: usize,
        /// The number of the second.
        second: usize,
    },
    /// An unused place.
    Gone,
}

/// Where a panel that left its place returns: beside panel `beside`, on a side of its place
/// (numbered as `put` does) with `share` of the room, or among its tabs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Home {
    pub name: String,
    pub beside: String,
    pub side: usize,
    pub share: f32,
}

/// A dock's layout, kept by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dock {
    /// Its places; the first is the whole dock.
    pub nodes: Vec<DockNode>,
    /// Panels floating over the window instead, each with its placement.
    pub floating: Vec<(String, Panel)>,
    /// Panels the user closed with their tab's close button (where allowed:
    /// [`docked_closable`](Ui::docked_closable)), oldest first. Hidden until the app reopens
    /// them (`set_open`): an app whose panels are documents takes them from here.
    pub closed: Vec<String>,
    /// The panel maximized within the dock, with the rest of its place: usually none. The user
    /// toggles it with the button at the end of its tabs.
    pub maximized: Option<String>,
    /// Where each floating or closed panel returns.
    pub(crate) homes: Vec<Home>,
}

impl Dock {
    /// A dock of these panels as tabs.
    pub fn of(names: &[&str]) -> Self {
        Dock { nodes: vec![DockNode::Tabs { names: names.iter().map(|name| name.to_string()).collect(), front: 0 }], ..Default::default() }
    }
    /// This dock on the left with `share` of the width, `other` on the right.
    pub fn beside(self, other: Dock, share: f32) -> Self {
        self.joined(other, true, share)
    }
    /// This dock on top with `share` of the height, `other` below.
    pub fn above(self, other: Dock, share: f32) -> Self {
        self.joined(other, false, share)
    }
    /// The names of all shown panels: docked, then floating.
    pub fn panels(&self) -> impl Iterator<Item = &str> {
        let docked = self.nodes.iter().flat_map(|place| if let DockNode::Tabs { names, .. } = place { names.as_slice() } else { &[] });
        docked.map(String::as_str).chain(self.floating.iter().map(|(name, _)| name.as_str()))
    }
    /// Whether a panel is open: not among the closed ones.
    pub fn is_open(&self, name: &str) -> bool {
        !self.closed.iter().any(|closed| closed == name)
    }
    /// Reopens a closed panel where it was, or closes one as its close button does. (For a menu
    /// that checks the open panels.)
    pub fn set_open(&mut self, name: &str, open: bool) {
        match (open, self.is_open(name)) {
            (true, false) => {
                self.closed.retain(|closed| closed != name);
                self.go_home(name);
            }
            (false, true) => self.close(name),
            _ => {}
        }
    }
    /// This dock without the panels not in `names`; their room goes to the others. For a layout
    /// saved earlier (see `written`) by an app that no longer has every panel; panels added
    /// since join the first tabs by themselves.
    pub fn only(mut self, names: &[&str]) -> Self {
        let strangers: Vec<String> = self.panels().filter(|name| !names.contains(name)).map(str::to_string).collect();
        for name in &strangers {
            self.take(name);
        }
        let known = |name: &String| names.contains(&name.as_str());
        self.closed.retain(known);
        self.homes.retain(|home| known(&home.name));
        self.maximized = self.maximized.filter(known);
        self
    }
    fn joined(self, other: Dock, across: bool, share: f32) -> Self {
        // Both place lists go behind a new first place; their indices shift by that.
        let moved = |place: DockNode, by: usize| match place {
            DockNode::Split { across, share, first, second } => DockNode::Split { across, share, first: first + by, second: second + by },
            other => other,
        };
        let second = 1 + self.nodes.len();
        let mut places = vec![DockNode::Split { across, share, first: 1, second }];
        places.extend(self.nodes.into_iter().map(|place| moved(place, 1)));
        places.extend(other.nodes.into_iter().map(|place| moved(place, second)));
        let (floating, closed, homes) = ([self.floating, other.floating].concat(), [self.closed, other.closed].concat(), [self.homes, other.homes].concat());
        Dock { nodes: places, floating, closed, maximized: self.maximized.or(other.maximized), homes }
    }

    /// Where a panel is: place and tab.
    fn find(&self, name: &str) -> Option<(usize, usize)> {
        let tab = |place: &DockNode| if let DockNode::Tabs { names, .. } = place { names.iter().position(|tab| tab == name) } else { None };
        self.nodes.iter().enumerate().find_map(|(at, place)| tab(place).map(|tab| (at, tab)))
    }
    /// The first tab place: where a panel with nowhere else to go goes.
    fn first_tabs(&self) -> usize {
        self.nodes.iter().position(|place| matches!(place, DockNode::Tabs { .. })).unwrap_or(0)
    }
    /// The first panel in a place, descending into the first half of a split: one standing for
    /// the place.
    fn first_in(&self, mut at: usize) -> Option<&str> {
        for _ in 0..self.nodes.len() {
            match self.nodes.get(at)? {
                DockNode::Split { first, .. } => at = *first,
                DockNode::Tabs { names, .. } => return names.first().map(String::as_str),
                DockNode::Gone => return None,
            }
        }
        None
    }
    /// Removes a panel from the dock. A place left empty gives its room to its sibling.
    fn take(&mut self, name: &str) {
        self.floating.retain(|(floating, _)| floating != name);
        let Some((at, mut tab)) = self.find(name) else { return };
        let DockNode::Tabs { names, front } = &mut self.nodes[at] else { return };
        names.remove(tab);
        // The front panel stays in front: removing one before it shifts its index.
        let stays = if tab < *front { *front - 1 } else { *front };
        (*front, tab) = (stays.min(names.len().saturating_sub(1)), names.len());
        let shared = self.nodes.iter().position(|place| matches!(place, DockNode::Split { first, second, .. } if *first == at || *second == at));
        let (Some(shared), 0) = (shared, tab) else { return };
        let DockNode::Split { first, second, .. } = self.nodes[shared] else { return };
        let stays = std::mem::replace(&mut self.nodes[if first == at { second } else { first }], DockNode::Gone);
        (self.nodes[shared], self.nodes[at]) = (stays, DockNode::Gone);
    }
    /// Puts a panel into place `at`: among its tabs (in front) for `MIDDLE`, else on that side,
    /// splitting the place; the panel's side gets `share` of the room.
    fn put(&mut self, name: &str, at: usize, side: usize, share: f32) {
        if self.nodes.is_empty() {
            self.nodes.push(DockNode::Tabs { names: Vec::new(), front: 0 });
        }
        let at = at.min(self.nodes.len() - 1);
        if let (MIDDLE, DockNode::Tabs { names, front }) = (side, &mut self.nodes[at]) {
            names.push(name.into());
            *front = names.len() - 1;
            return;
        }
        // The old place moves to a free slot; this one is held meanwhile so it is not taken as
        // free.
        let was = std::mem::replace(&mut self.nodes[at], DockNode::Tabs { names: Vec::new(), front: 0 });
        let (kept, new) = (self.keep(was), self.keep(DockNode::Tabs { names: vec![name.into()], front: 0 }));
        let (first, second, share) = if side < 2 { (new, kept, share) } else { (kept, new, 1.0 - share) };
        self.nodes[at] = DockNode::Split { across: side.is_multiple_of(2), share, first, second };
    }
    /// Stores a place in a free slot; returns its index.
    fn keep(&mut self, place: DockNode) -> usize {
        let free = self.nodes.iter().skip(1).position(|place| *place == DockNode::Gone).map_or(self.nodes.len(), |at| at + 1);
        self.nodes.resize(self.nodes.len().max(free + 1), DockNode::Gone);
        self.nodes[free] = place;
        free
    }
    /// Moves a panel to panel `beside`'s place: among its tabs, or onto a side, taking `share`
    /// of the room.
    fn move_to(&mut self, name: &str, beside: &str, side: usize, share: f32) {
        self.take(name);
        self.homes.retain(|home| home.name != name);
        let at = self.find(beside).map_or(0, |(at, _)| at);
        self.put(name, at, side, share);
    }

    /// Where a docked panel would return if it left now: among its tab siblings; else, alone in
    /// its place, beside its sibling place, on its side and with its share.
    fn home_of(&self, name: &str) -> Option<Home> {
        let (at, _) = self.find(name)?;
        let DockNode::Tabs { names, .. } = &self.nodes[at] else { return None };
        let home = |beside: &str, side: usize, share: f32| Some(Home { name: name.into(), beside: beside.into(), side, share });
        if let Some(other) = names.iter().find(|other| *other != name) {
            return home(other, MIDDLE, 0.5);
        }
        let halves = |place: &DockNode| match *place {
            DockNode::Split { across, share, first, second } if first == at || second == at => Some((across, share, first == at, if first == at { second } else { first })),
            _ => None,
        };
        let (across, share, before, other) = self.nodes.iter().find_map(halves)?;
        // Before the other: left or above; after it: right or below.
        home(self.first_in(other)?, [[3, 1], [2, 0]][across as usize][before as usize], if before { share } else { 1.0 - share })
    }
    /// Removes a panel from the dock, noting where it was so it can return.
    fn leave(&mut self, name: &str) {
        if let Some(home) = self.home_of(name) {
            self.homes.retain(|home| home.name != name);
            self.homes.push(home);
        }
        self.take(name);
        self.maximized = self.maximized.take().filter(|filled| filled != name);
    }
    /// Puts a panel back where it was, in front: beside the panel it was beside, if docked;
    /// else among the first tabs.
    fn go_home(&mut self, name: &str) {
        self.take(name);
        let home = self.homes.iter().position(|home| home.name == name).map(|at| self.homes.remove(at));
        let at = home.as_ref().and_then(|home| self.find(&home.beside)).map(|(at, _)| at);
        match (home, at) {
            (Some(home), Some(at)) => self.put(name, at, home.side, home.share),
            _ => self.put(name, self.first_tabs(), MIDDLE, 0.5),
        }
    }
    /// Floats a docked panel here.
    pub(crate) fn float(&mut self, name: &str, panel: Panel) {
        self.leave(name);
        self.floating.push((name.into(), panel));
    }
    /// Closes a docked or floating panel: it joins the closed ones.
    pub(crate) fn close(&mut self, name: &str) {
        self.leave(name);
        self.closed.retain(|closed| closed != name);
        self.closed.push(name.into());
    }

    /// Lays out every place in `area`: tab places into `tabs`, each with its room; splitters
    /// into `bars`, each with the place it splits, its rect, and the length of the room it
    /// splits.
    fn lay(&self, area: Rect, tabs: &mut Vec<(usize, Rect)>, bars: &mut Vec<(usize, Rect, f32)>) {
        // Places still to lay out, each with its room: never more than a few at once. (And no
        // more steps than places: a malformed dock whose places form a cycle lays out wrong,
        // but ends.)
        let (mut waiting, mut count, mut steps) = ([(0usize, area); 32], 1, self.nodes.len());
        while count > 0 && steps > 0 {
            (count, steps) = (count - 1, steps - 1);
            let (at, room) = waiting[count];
            let Some(DockNode::Split { across, share, first, second }) = self.nodes.get(at).filter(|_| count + 2 <= waiting.len()) else {
                tabs.extend(self.nodes.get(at).filter(|place| matches!(place, DockNode::Tabs { .. })).map(|_| (at, room)));
                continue;
            };
            let [near, bar, far] = halves(room, *across, *share);
            bars.push((at, bar, if *across { room.w } else { room.h } - BAR));
            (waiting[count], waiting[count + 1], count) = ((*second, far), (*first, near), count + 2);
        }
    }
}

/// A room split by a bar: the first part with `share`, the bar, and the rest.
fn halves(room: Rect, across: bool, share: f32) -> [Rect; 3] {
    let share = share.clamp(0.05, 0.95);
    let near = (((if across { room.w } else { room.h }) - BAR) * share).round().max(0.0);
    match across {
        true => [rect(room.x, room.y, near, room.h), rect(room.x + near, room.y, BAR, room.h), rect(room.x + near + BAR, room.y, (room.w - near - BAR).max(0.0), room.h)],
        false => [rect(room.x, room.y, room.w, near), rect(room.x, room.y + near, room.w, BAR), rect(room.x, room.y + near + BAR, room.w, (room.h - near - BAR).max(0.0))],
    }
}

/// A place's five drop targets: four sides and the centre. Each with the part of the place the
/// panel would take.
fn marks(place: Rect) -> [(Rect, Rect); 5] {
    let (middle, half) = (pt(place.x + place.w / 2.0, place.y + place.h / 2.0), [place.w / 2.0, place.h / 2.0]);
    let mark = |dx: f32, dy: f32| rect(middle.x + dx * MARK[1] - MARK[0] / 2.0, middle.y + dy * MARK[1] - MARK[0] / 2.0, MARK[0], MARK[0]);
    [
        (mark(-1.0, 0.0), rect(place.x, place.y, half[0], place.h)),
        (mark(0.0, -1.0), rect(place.x, place.y, place.w, half[1])),
        (mark(1.0, 0.0), rect(middle.x, place.y, half[0], place.h)),
        (mark(0.0, 1.0), rect(place.x, middle.y, place.w, half[1])),
        (mark(0.0, 0.0), place),
    ]
}

/// What the user did to a tab: selected it; dragged it past the tab before (-1) or after (1);
/// double-clicked; closed it.
struct Done {
    tab: Response,
    moves: i8,
    closed: bool,
    /// Its panel goes to the place in this direction (x, y).
    goes: Option<[f32; 2]>,
}

/// The tab place nearest `room` in direction `way` (x and y, each -1, 0 or 1), by centre
/// distance.
fn place_toward(tabs: &[(usize, Rect)], room: Rect, way: [f32; 2]) -> Option<usize> {
    let middle = |r: Rect| [r.x + r.w / 2.0, r.y + r.h / 2.0];
    let [x, y] = middle(room);
    let ahead = |r: Rect| [(middle(r)[0] - x) * way[0] + (middle(r)[1] - y) * way[1], (middle(r)[0] - x).hypot(middle(r)[1] - y)];
    let toward = tabs.iter().filter(|(_, r)| ahead(*r)[0] > 0.5);
    toward.min_by(|(_, a), (_, b)| ahead(*a)[1].total_cmp(&ahead(*b)[1])).map(|(place, _)| *place)
}

/// Docks: panels the user arranges by dragging their tabs.
impl Ui {
    /// Opens a dock: a box filling its room, laid out as `dock` says. Describe each panel with
    /// [`docked`](Self::docked) (or [`docked_closable`](Self::docked_closable)), in any order,
    /// then [`close`](Self::close) the dock.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn dock(&mut self, dock: &mut Dock) {
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, self.size().h, 0.0]);
        self.el().flow(Flow::Stack).grow().clip().sense().open();
        self.measures.push(Measure { at, within: at, id, read });
        let (mut tabs, mut bars) = (std::mem::take(&mut self.docks), std::mem::take(&mut self.dock_bars));
        tabs.clear();
        bars.clear();
        // A maximized panel gets all of it; the others none.
        let whole = rect(0.0, 0.0, read[0], read[1]);
        let filled = dock.maximized.as_deref().and_then(|name| dock.find(name));
        match filled {
            Some((place, _)) => tabs.push((place, whole)),
            None => dock.lay(whole, &mut tabs, &mut bars),
        }
        dock.maximized = dock.maximized.take().filter(|_| filled.is_some());
        let area = self.visible_rect(id);
        self.dock_keys(dock, &tabs, area);
        for (place, bar, long) in bars.iter().copied() {
            self.dock_bar(dock, place, bar, long);
        }
        for (place, room) in tabs.iter().copied() {
            self.dock_tabs(dock, place, room, area, &tabs);
        }
        (self.docks, self.dock_bars) = (tabs, bars);
        self.let_go_on(dock, id);
        self.frame.docks += 1;
        self.frame.later[2] = Some(Ui::keyboard_into);
    }

    /// Keys while focus is in the dock: Ctrl+Tab brings the next panel of its place to the
    /// front and focuses it (Shift: previous); Ctrl+F6 focuses the front panel of the next
    /// place (Shift: previous; from outside any dock, the first place of the first); Ctrl+F4
    /// closes the focused panel if allowed.
    fn dock_keys(&mut self, dock: &mut Dock, tabs: &[(usize, Rect)], area: Rect) {
        let on = self.visible_rect(self.focus());
        let middle = pt(on.x + on.w / 2.0, on.y + on.h / 2.0);
        let here = tabs.iter().position(|(_, room)| !on.is_empty() && room.moved(area.x, area.y).contains(middle));
        let (ahead, back) = (Mods::CTRL, Mods { ctrl: true, shift: true, alt: false });
        let tab = if here.is_some() { self.key(Key::Tab, ahead) as i32 - self.key(Key::Tab, back) as i32 } else { 0 };
        let anywhere = here.is_some() || self.frame.docks == 0;
        let place = if anywhere { self.key(Key::F(6), ahead) as i32 - self.key(Key::F(6), back) as i32 } else { 0 };
        let shut = here.is_some() && self.key(Key::F(4), Mods::CTRL);
        if tabs.is_empty() || tab == 0 && place == 0 && !shut {
            return;
        }
        // The target place, and its tabs.
        let count = tabs.len() as i32;
        let to = here.map_or(if place < 0 { count - 1 } else { 0 }, |here| (here as i32 + place).rem_euclid(count)) as usize;
        let Some(DockNode::Tabs { names, front }) = dock.nodes.get_mut(tabs[to].0) else { return };
        *front = (*front as i32 + tab).rem_euclid(names.len().max(1) as i32) as usize;
        let Some(name) = names.get(*front).cloned() else { return };
        self.again = true;
        match shut {
            true if self.frame.closable[0].contains(&hash(&name)) => dock.close(&name),
            true => {}
            false => self.frame.into = Some(name),
        }
    }

    /// A splitter between two places: dragging gives one more room. Focusable; arrows move it
    /// (Home/End to its ends); screen readers get a slider named by the panels either side.
    fn dock_bar(&mut self, dock: &mut Dock, place: usize, bar: Rect, long: f32) {
        let t = self.theme;
        let name = match (self.listening, dock.nodes.get(place)) {
            (true, Some(DockNode::Split { first, second, .. })) => [*first, *second].map(|half| dock.first_in(half).unwrap_or("")).join(", "),
            _ => String::new(),
        };
        let Some(DockNode::Split { across, share, .. }) = dock.nodes.get_mut(place) else { return };
        let hold = self.interact(self.id((place, "bar")));
        let was = *share;
        if hold.focused {
            let [more, less] = [[Key::Right, Key::Down], [Key::Left, Key::Up]].map(|keys| keys.into_iter().any(|key| self.key(key, Mods::NONE)));
            let [home, end] = [Key::Home, Key::End].map(|key| self.key(key, Mods::NONE));
            *share = [*share + STEP * (more as i8 - less as i8) as f32, 0.0, 1.0][if home { 1 } else { 2 * end as usize }];
        }
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == hold.id) {
            *share = to;
        }
        *share = share.clamp(0.05, 0.95);
        self.again |= *share != was;
        if hold.hovered || hold.pressed {
            self.set_cursor(if *across { Cursor::ResizeX } else { Cursor::ResizeY });
        }
        if let Some((gone, was)) = self.drag_delta(&hold, [*share, 0.0, 0.0, 0.0]) {
            let to = (was[0] + if *across { gone.x } else { gone.y } / long.max(1.0)).clamp(0.05, 0.95);
            (self.again, *share) = (self.again || to != *share, to);
        }
        let fill = if hold.hovered || hold.pressed || hold.focused { t.accent } else { t.line };
        self.el().key((place, "bar")).offset(bar.x, bar.y).size(bar.w, bar.h).fill(fill).sense().focusable().show();
        self.tell_value_named(&hold, Role::Slider, [*share, 0.05, 0.95], &name);
        self.tell_step(STEP);
    }

    /// A place's tab strip: a click brings a panel to the front; a tab dragged along the strip
    /// reorders, dragged off it drags its panel; a double click floats it, its close button (or
    /// the middle button) closes it. At the strip's end, the maximize/restore button.
    fn dock_tabs(&mut self, dock: &mut Dock, place: usize, room: Rect, area: Rect, places: &[(usize, Rect)]) {
        let (t, filled, split) = (self.theme, dock.maximized.is_some(), dock.nodes.len() > 1);
        let Some(DockNode::Tabs { names, front }) = dock.nodes.get_mut(place) else { return };
        let strip = self.row().key((place, "tabs")).offset(room.x, room.y).size(room.w, TABS).pad((4.0, 0.0)).gap(2.0).fill(t.panel).clip().sense().open();
        self.tell(&strip, Role::Tabs, None, "");
        let (mut selected, mut moved, mut floats, mut shut, mut goes) = (None, None, None, None, None);
        for number in 0..names.len() {
            let beside = [number.checked_sub(1), Some(number + 1)].map(|at| at.and_then(|at| names.get(at)).map(String::as_str));
            let done = self.dock_tab(&names[number], number == *front, beside, strip.rect);
            self.tell_position(number + 1, names.len());
            // A tab dragged and dropped over itself was not selected.
            let (gone, released) = (self.gone(), self.input.released[0]);
            selected = selected.or((done.tab.clicked && !(released && gone.x.abs().max(gone.y.abs()) > 4.0)).then_some(number));
            moved = moved.or((done.moves != 0).then(|| (number, (number as isize + done.moves as isize) as usize)));
            floats = floats.or(done.tab.double_clicked.then_some(number));
            shut = shut.or(done.closed.then_some(number));
            goes = goes.or(done.goes.and_then(|way| place_toward(places, room, way)).map(|to| (number, to)));
        }
        // A tab dragged past another swaps with it; the front one stays in front wherever it
        // goes.
        if let Some((from, to)) = moved.filter(|(_, to)| *to < names.len()) {
            names.swap(from, to);
            *front = [to, from, *front][if *front == from { 0 } else { 1 + (*front != to) as usize }];
            self.again = true;
        }
        *front = selected.unwrap_or(*front);
        let [floats, shut] = [floats, shut].map(|at| at.and_then(|at| names.get(at)).cloned());
        let goes = goes.and_then(|(at, to)| Some((names.get(at)?.clone(), to)));
        self.spacer();
        if split || filled {
            self.fill_button(dock, place);
        }
        self.close();
        self.dock_tab_requests(dock, [floats, shut], goes, room, area);
    }

    /// Acts on what a place's tabs asked: panel `floats` floats near the place (`room`, in
    /// `area`), `shut` closes, and `goes` joins the tabs of the place it was sent to.
    fn dock_tab_requests(&mut self, dock: &mut Dock, [floats, shut]: [Option<String>; 2], goes: Option<(String, usize)>, room: Rect, area: Rect) {
        if let Some(name) = floats {
            let (window, x, y) = (self.size(), area.x + room.x + 24.0, area.y + room.y + 24.0);
            let [w, h] = [room.w.max(LEAST[0]).min(window.w), room.h.max(LEAST[1]).min(window.h)];
            dock.float(&name, Panel::new(x.min(window.w - w).max(0.0).round(), y.min(window.h - h).max(0.0).round(), w.round(), h.round()));
            self.again = true;
        }
        if let Some(name) = shut {
            dock.close(&name);
            self.again = true;
        }
        // Sent by keys to the neighbouring place, the panel joins its tabs, and focus follows
        // it.
        let beside = goes.and_then(|(name, to)| if let Some(DockNode::Tabs { names, front }) = dock.nodes.get(to) { Some((name, names.get(*front)?.clone())) } else { None });
        if let Some((name, beside)) = beside {
            dock.move_to(&name, &beside, MIDDLE, 0.5);
            (self.frame.into, self.again) = (Some(name), true);
        }
    }

    /// One tab: a panel's name, marked if in front, with a close button if closable. `beside`
    /// are the neighbouring tabs' names, `strip` the strip's rect.
    fn dock_tab(&mut self, name: &str, front: bool, beside: [Option<&str>; 2], strip: Rect) -> Done {
        let (t, closable) = (self.theme, self.frame.closable[0].contains(&hash(name)));
        let middle = |of: Rect| if of.is_empty() { f32::NAN } else { of.x + of.w / 2.0 };
        let [before, after] = beside.map(|other| other.map_or(f32::NAN, |other| middle(self.visible_rect(self.id(other)))));
        // The tab and its close button are one box in the strip, keyed by the panel's name,
        // that senses the pointer so neighbouring tabs know where it is. In a strip too narrow
        // for all its tabs, each shrinks, its name ending in "…".
        self.row().key(name).h(Length::Grow(1.0)).align(Align::Start, Align::Spread).shrink(1).min(36.0, 0.0).sense().open();
        let tab = self.press(self.id("tab"));
        if self.frame.into.as_deref() == Some(name) {
            self.frame.into_tab = tab.id;
        }
        // Dragged a little with the button down: along the strip it passes its neighbours, off
        // the strip it drags its panel.
        let (at, dragging) = (self.pointer().unwrap_or_default(), self.dragged_panel == name);
        let held = self.drag_delta(&tab, [0.0; 4]).is_some_and(|(gone, _)| gone.x.abs().max(gone.y.abs()) > 4.0);
        let along = held && !dragging && strip.grow(STRAY).contains(at);
        if held && !along {
            self.dragged_panel.clear();
            self.dragged_panel.push_str(name);
        }
        let x = if along { at.x } else { f32::NAN };
        let ink = if front || tab.hovered { t.text } else { t.text_dim };
        let right = if closable { 4.0 } else { 10.0 };
        self.column().key("tab").pad((10.0, 7.0, right, 0.0)).align(Align::Spread, Align::Spread).shrink(1).sense().focusable().open();
        self.el().text(name, t.font, ink).clip().show();
        self.el().h(Length::Px(2.0)).radius(1.0).fill(if front { t.accent } else { Color::NONE }).show();
        self.close();
        let crossed = closable && self.tab_cross(front, tab.hovered);
        let closed = crossed || closable && (tab.hovered && self.input.pressed[2] || tab.focused && self.key(Key::Delete, Mods::NONE));
        // From the keyboard: Ctrl+Shift+Left/Right move the tab along its strip;
        // Alt+Shift+arrow sends its panel to the place that way.
        let (along, away) = (Mods { ctrl: true, shift: true, alt: false }, Mods { ctrl: false, shift: true, alt: true });
        let keyed = if tab.focused { self.key(Key::Right, along) as i8 - self.key(Key::Left, along) as i8 } else { 0 };
        let ways = [(Key::Left, [-1.0, 0.0]), (Key::Right, [1.0, 0.0]), (Key::Up, [0.0, -1.0]), (Key::Down, [0.0, 1.0])];
        let goes = ways.into_iter().find(|(key, _)| tab.focused && self.key(*key, away)).map(|(_, way)| way);
        self.close();
        // Reported last, so its position among the tabs is known by then.
        self.tell(&tab, Role::Tab, Some(front), "");
        let moves = (x >= after) as i8 - (x < before) as i8;
        Done { tab, moves: if keyed != 0 { keyed } else { moves }, closed, goes }
    }
    /// A tab's close button: returns whether it was pressed.
    fn tab_cross(&mut self, front: bool, over: bool) -> bool {
        let t = self.theme;
        self.column().pad((2.0, 9.0, 8.0, 0.0)).align(Align::Spread, Align::Spread).open();
        let cross = self.press(self.next_id());
        let ink = if cross.hovered {
            t.text
        } else if over || front {
            t.text_dim
        } else {
            t.text_faint
        };
        self.el().icon(CROSS, 12.0, ink).fill(if cross.hovered { t.hover } else { Color::NONE }).radius(3.0).sense().show();
        self.el().h(Length::Px(2.0)).fill(if front { t.accent } else { Color::NONE }).show();
        self.close();
        let words = self.frame_words.close;
        self.tell(&cross, Role::Button, None, words);
        self.tell_shortcut("Ctrl+F4");
        cross.clicked
    }
    /// The button at the end of a tab strip that maximizes its place within the dock, and
    /// restores it.
    fn fill_button(&mut self, dock: &mut Dock, place: usize) {
        let t = self.theme;
        let fills = dock.maximized.as_ref().and_then(|name| dock.find(name)).is_some_and(|(at, _)| at == place);
        let state = self.press(self.id("fill"));
        let ink = if state.hovered || state.focused { t.text } else { t.text_dim };
        self.row().key("fill").size(22.0, 22.0).align(Align::Center, Align::Center).fill(if state.hovered { t.hover } else { Color::NONE }).radius(4.0).sense().focusable().open();
        self.el().icon(if fills { BACK } else { FILL }, 14.0, ink).show();
        self.close();
        let words = if fills { self.frame_words.back } else { self.frame_words.fill };
        self.tell(&state, Role::Button, Some(fills), words);
        self.tooltip(&state, words);
        if state.clicked {
            let front = if let Some(DockNode::Tabs { names, front }) = dock.nodes.get(place) { names.get(*front).cloned() } else { None };
            (dock.maximized, self.again) = (if fills { None } else { front }, true);
        }
    }

    /// Opens a dock panel; returns whether its content shows (not when another panel is in
    /// front of it, or another place is maximized). If so, describe it, then
    /// [`close`](Self::close) it. A panel the dock doesn't know yet joins its first tabs.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn docked(&mut self, dock: &mut Dock, name: &str) -> bool {
        self.docked_as(dock, name, false)
    }
    /// [`docked`](Self::docked), for a panel the user may close: its tab has a close button,
    /// and the middle button, Ctrl+F4 and its floating window's close button close it too. It
    /// then joins `dock.closed`, hidden until the app reopens it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn docked_closable(&mut self, dock: &mut Dock, name: &str) -> bool {
        self.docked_as(dock, name, true)
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn docked_as(&mut self, dock: &mut Dock, name: &str, closable: bool) -> bool {
        if closable && !self.frame.closable[1].contains(&hash(name)) {
            self.frame.closable[1].push(hash(name));
        }
        if !dock.is_open(name) {
            return false;
        }
        if let Some(at) = dock.floating.iter().position(|(floating, _)| floating == name) {
            return self.floating(dock, at, closable);
        }
        let Some((place, tab)) = dock.find(name) else {
            dock.put(name, dock.first_tabs(), MIDDLE, 0.5);
            self.again = true;
            return false;
        };
        let room = self.docks.iter().find(|(at, _)| *at == place).map(|(_, room)| *room);
        let (Some(room), Some(DockNode::Tabs { front, .. })) = (room, dock.nodes.get(place)) else { return false };
        if *front != tab {
            return false;
        }
        let (t, at) = (self.theme, self.tree.specs.len() as u32);
        if self.frame.into.as_deref() == Some(name) {
            self.frame.into_box = Some((at, self.frame.into_tab));
        }
        self.column().key(name).offset(room.x, room.y + TABS).size(room.w, (room.h - TABS).max(0.0)).pad(10.0).gap(t.gap).scroll().sense().open();
        true
    }
    /// Focuses the panel focus was sent to: its first focusable part, or else its tab. (After
    /// the panel is described.)
    pub(crate) fn keyboard_into(&mut self) {
        let (into, into_box) = (self.frame.into.take(), self.frame.into_box.take());
        let Some((at, tab)) = into_box.filter(|_| into.is_some()) else { return };
        let end = self.tree.specs[at as usize].end;
        let first = self.reachable.iter().find(|(_, _, inside)| *inside > at && *inside < end).map_or(tab, |(_, id, _)| *id);
        self.set_focus(first);
        self.focus_shown = true;
    }

    /// A floating dock panel: an ordinary panel, dragged by its title so it can drop on a dock
    /// target. Double-clicking its title docks it back, as does its close button (which closes
    /// it, if closable).
    fn floating(&mut self, dock: &mut Dock, at: usize, closable: bool) -> bool {
        let (name, panel) = &mut dock.floating[at];
        let shows = self.dock_panel(panel, name);
        // Held by its title, it drags once moved a little: a click, or a double click, is no
        // drag.
        let gone = self.gone();
        if self.panel_held && gone.x.abs().max(gone.y.abs()) > 4.0 {
            self.dragged_panel.clone_from(name);
        }
        let (twice, shut) = (std::mem::take(&mut self.frame.title_twice), !panel.open);
        if twice || shut {
            let name = name.clone();
            panel.open = true;
            match shut && closable {
                true => dock.close(&name),
                false => dock.go_home(&name),
            }
            self.again = true;
        }
        shows
    }

    /// While a panel is dragged: shows the targets of the place under the pointer; on drop,
    /// puts the panel where the target under the pointer says (over a tab strip, among its
    /// tabs). A tab dropped on no target floats there.
    fn let_go_on(&mut self, dock: &mut Dock, id: Id) {
        if self.dragged_panel.is_empty() {
            return;
        }
        let (name, area) = (std::mem::take(&mut self.dragged_panel), self.visible_rect(id));
        let at = self.pointer().unwrap_or(Point { x: -1.0, y: -1.0 });
        // The place under the pointer, in window coordinates, and its target under it.
        let under = self.docks.iter().map(|(place, room)| (*place, room.moved(area.x, area.y))).find(|(_, room)| room.contains(at));
        let on_mark = under.and_then(|(_, room)| marks(room).iter().position(|(mark, _)| mark.contains(at)));
        let aimed = on_mark.or(under.filter(|(_, room)| at.y < room.y + TABS).map(|_| MIDDLE));
        self.set_cursor(Cursor::Move);
        if let Some((_, room)) = under {
            self.show_marks(id, room, aimed);
        }
        let held = self.input.down[0] || self.input.released[0];
        if self.input.released[0] {
            self.settle_panel(dock, &name, under.map(|(place, _)| place).zip(aimed), at);
        }
        // Dragged as long as the button is down; the dragger says so every run.
        if held && !self.input.released[0] {
            self.dragged_panel = name;
        }
    }

    /// The targets of the place under the pointer, over everything; the aimed one highlighted,
    /// with the part of the place the panel would take.
    fn show_marks(&mut self, id: Id, room: Rect, aimed: Option<usize>) {
        let t = self.theme;
        self.layer(id ^ MARKS, Anchor::At(pt(0.0, 0.0)), false, Color::NONE);
        let window = self.size();
        self.el().flow(Flow::Stack).size(window.w, window.h).open();
        let takes = aimed.map_or(Rect::EMPTY, |side| marks(room)[side].1);
        self.el().offset(takes.x, takes.y).size(takes.w, takes.h).fill(t.accent.with_alpha(0.22)).show();
        for (side, (mark, _)) in marks(room).iter().enumerate() {
            let fill = if aimed == Some(side) { t.accent } else { t.raised };
            self.el().offset(mark.x, mark.y).size(mark.w, mark.h).radius(6.0).fill(fill).border(t.line_strong, 1.0).show();
        }
        self.close();
    }

    /// Puts a dropped panel where it belongs: on the aimed side of a place (or among its tabs);
    /// or, for a tab dropped on no target, floating where dropped.
    fn settle_panel(&mut self, dock: &mut Dock, name: &str, aimed: Option<(usize, usize)>, at: Point) {
        self.again = true;
        let floats = dock.floating.iter().any(|(floating, _)| floating == name);
        // The front panel of the aimed place identifies it, whatever removing this panel does
        // to place indices.
        let beside = aimed.and_then(|(place, _)| if let Some(DockNode::Tabs { names, .. }) = dock.nodes.get(place) { names.iter().find(|other| *other != name).cloned() } else { None });
        match (aimed, beside) {
            (Some((_, side)), Some(beside)) => dock.move_to(name, &beside, side, 0.5),
            // Alone in the place it was dropped on: it is there already.
            (Some(_), None) => {}
            (None, _) if floats => {}
            (None, _) => dock.float(name, Panel::new(at.x - 60.0, at.y - 14.0, 320.0, 240.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(dock: &Dock, name: &str) -> Vec<String> {
        let (place, _) = dock.find(name).unwrap();
        if let DockNode::Tabs { names, .. } = &dock.nodes[place] { names.clone() } else { Vec::new() }
    }
    /// Front panels' rects by name, in a 400 by 300 dock.
    fn rooms(dock: &Dock) -> Vec<(String, Rect)> {
        let (mut places, mut bars) = (Vec::new(), Vec::new());
        dock.lay(rect(0.0, 0.0, 400.0, 300.0), &mut places, &mut bars);
        let named = |(place, room): &(usize, Rect)| if let DockNode::Tabs { names, front } = &dock.nodes[*place] { (names[*front].clone(), *room) } else { (String::new(), *room) };
        let mut rooms: Vec<_> = places.iter().map(named).collect();
        rooms.sort_by(|a, b| a.0.cmp(&b.0));
        rooms
    }

    #[test]
    fn places_share_the_room_as_they_are_told() {
        let dock = Dock::of(&["a"]).beside(Dock::of(&["b", "c"]).above(Dock::of(&["d"]), 0.6), 0.25);
        // A quarter of the width less the bar for "a"; the rest is "b" over "d".
        let expected = [("a", rect(0.0, 0.0, 99.0, 300.0)), ("b", rect(104.0, 0.0, 296.0, 177.0)), ("d", rect(104.0, 182.0, 296.0, 118.0))];
        assert_eq!(rooms(&dock), expected.map(|(name, room)| (name.to_string(), room)));
        assert_eq!(tabs(&dock, "c"), ["b", "c"]);
    }

    #[test]
    fn a_panel_is_moved_beside_another_and_an_empty_place_gives_up_its_room() {
        let mut dock = Dock::of(&["a"]).beside(Dock::of(&["b", "c"]), 0.5);
        // "c" goes under "a": "a"'s place splits.
        dock.move_to("c", "a", 3, 0.5);
        let names: Vec<String> = rooms(&dock).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(rooms(&dock)[2].1.y > rooms(&dock)[0].1.y, "c lies under a");
        // "b" joins "a"'s tabs: its own place empties, and "a" and "c" get all the room.
        dock.move_to("b", "a", MIDDLE, 0.5);
        assert_eq!(tabs(&dock, "a"), ["a", "b"]);
        let rooms = rooms(&dock);
        assert_eq!(rooms.len(), 2);
        assert_eq!((rooms[0].1.w, rooms[1].1.w), (400.0, 400.0), "one over the other, across the whole dock");
        // The very last panel can be removed and put back.
        let mut alone = Dock::of(&["only"]);
        alone.take("only");
        alone.put("only", 0, MIDDLE, 0.5);
        assert_eq!(tabs(&alone, "only"), ["only"]);
    }

    #[test]
    fn a_dock_gives_up_the_panels_an_app_no_longer_has() {
        let mut dock = Dock::of(&["a"]).beside(Dock::of(&["b", "c"]).above(Dock::of(&["d"]), 0.6), 0.25);
        dock.floating.push(("e".to_string(), Panel::default()));
        dock.close("f");
        assert_eq!(dock.panels().collect::<Vec<_>>(), ["a", "b", "c", "d", "e"]);
        // Without "a", "c", "e" and "f": "b" over "d" fill the dock, and nothing floats.
        let dock = dock.only(&["b", "d", "new"]);
        assert_eq!(dock.panels().collect::<Vec<_>>(), ["b", "d"]);
        assert!(dock.closed.is_empty());
        let expected = [("b", rect(0.0, 0.0, 400.0, 177.0)), ("d", rect(0.0, 182.0, 400.0, 118.0))];
        assert_eq!(rooms(&dock), expected.map(|(name, room)| (name.to_string(), room)));
        // With none of its panels left, it is an empty dock, which takes the next panel.
        let mut none = dock.only(&[]);
        assert_eq!(none.panels().count(), 0);
        none.put("new", 0, MIDDLE, 0.5);
        assert_eq!(tabs(&none, "new"), ["new"]);
    }

    #[test]
    fn places_that_name_each_other_in_a_ring_are_not_laid_out_for_ever() {
        let ring = |first, second| DockNode::Split { across: true, share: 0.5, first, second };
        let tabs = DockNode::Tabs { names: vec!["a".into()], front: 0 };
        for places in [vec![ring(0, 0)], vec![ring(1, 2), ring(0, 2), tabs.clone()], vec![ring(1, 1), tabs.clone()], vec![ring(7, 9)]] {
            let (mut rooms, mut bars) = (Vec::new(), Vec::new());
            Dock { nodes: places, ..Default::default() }.lay(rect(0.0, 0.0, 400.0, 300.0), &mut rooms, &mut bars);
            assert!(rooms.len() <= 2 && bars.len() <= 3);
        }
    }

    #[test]
    fn a_panel_that_floats_or_is_closed_goes_back_where_it_was() {
        let first = Dock::of(&["a"]).beside(Dock::of(&["b", "c"]).above(Dock::of(&["d"]), 0.6), 0.25);
        let before = rooms(&first);
        // Alone in its place, "d" returns under "b" with its old share; "c", a tab sibling,
        // returns among them, in front.
        let mut dock = first.clone();
        dock.float("d", Panel::default());
        dock.close("c");
        assert_eq!((dock.panels().collect::<Vec<_>>(), dock.closed.clone()), (vec!["a", "b", "d"], vec!["c".to_string()]));
        assert_eq!(rooms(&dock).len(), 2, "b has the room of d now");
        dock.go_home("d");
        dock.set_open("c", true);
        assert!(dock.floating.is_empty() && dock.closed.is_empty() && dock.homes.is_empty());
        assert_eq!(tabs(&dock, "c"), ["b", "c"]);
        let after: Vec<(String, Rect)> = rooms(&dock).into_iter().map(|(name, room)| (if name == "c" { "b".to_string() } else { name }, room)).collect();
        assert_eq!(after, before);
        // "a" was left of all the others: it returns left of the place standing for them.
        dock.float("a", Panel::default());
        dock.go_home("a");
        assert!(rooms(&dock)[0].1.x == 0.0 && rooms(&dock)[0].0 == "a");
        // "d" was under all the others, which "a" stands for now: it returns under "a".
        dock.close("d");
        dock.set_open("d", true);
        assert!(rooms(&dock).iter().any(|(name, room)| name == "d" && room.y > 0.0 && room.x == 0.0 && room.w < 400.0));
        // A panel whose tabs are gone meanwhile, or that never had a place, joins the first
        // tabs.
        dock.close("b");
        dock.close("c");
        dock.set_open("b", true);
        dock.set_open("new", false);
        dock.set_open("new", true);
        assert_eq!(tabs(&dock, "new"), ["a", "b", "new"]);
    }

    #[test]
    fn a_panel_that_fills_the_dock_leaves_it_when_it_floats_or_closes() {
        let mut dock = Dock::of(&["a"]).beside(Dock::of(&["b"]), 0.5);
        dock.maximized = Some("b".into());
        dock.float("a", Panel::default());
        assert_eq!(dock.maximized.as_deref(), Some("b"));
        dock.close("b");
        assert_eq!(dock.maximized, None);
    }
}
