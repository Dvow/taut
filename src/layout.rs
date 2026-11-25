//! Layout. A UI is a tree of boxes: each lays out its children in a row, a column, a grid, or a
//! stack, and each child sizes itself to fit its content, at a fixed size, or as a share of the
//! leftover room.
//!
//! Boxes are kept in one list in pre-order. Sizes come from four passes over it, never by
//! trial: widths bottom-up (each box tells its parent what it needs), then top-down (each takes
//! its share); text wraps to the widths it got; then heights the same way. Each pass visits
//! each box once.
//!
//! What the app sets on a box (`Spec`) is kept apart from what layout computes (`Place`), and
//! positions are relative to the parent. So a run where nothing layout reads changed keeps
//! every `Place`, and scrolling moves content without any relayout.
use crate::color::Color;
use crate::geom::{Insets, Rect, rect};
use crate::id::{ById, Id, hash, mix};
use crate::text::{Font, Line, TextSystem, wrap};

/// "No box".
pub(crate) const NONE: u32 = u32::MAX;
/// The page's index: every run's first box.
pub(crate) const PAGE: u32 = 0;
/// Width of a box's scroll bar gutter.
pub(crate) const BAR: f32 = 10.0;
/// How far a glyph may overhang the box its text was measured for, in units.
pub(crate) const OVERHANG: f32 = 2.0;

/// A box's size along one axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Fit to content.
    #[default]
    Fit,
    /// Exactly this many units.
    Px(f32),
    /// A share of the leftover room, by weight. Across the parent's flow: all of it.
    Grow(f32),
}

/// How a box lays out its children.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Flow {
    /// Side by side, from the left.
    Row,
    /// Top to bottom.
    #[default]
    Column,
    /// Stacked, each placed on its own.
    Stack,
    /// In this many columns, row by row: a column is as wide as its widest box, a row as high
    /// as its tallest. Columns with a growing box share the leftover room. The gap applies
    /// between columns and rows, unless the box sets a row gap
    /// ([`El::line_gap`](crate::El::line_gap)).
    Grid(u16),
    /// Left to right, wrapping to a new line when out of width: tags, toolbar buttons,
    /// thumbnails. A line is as high as its tallest box; the gap also separates lines unless
    /// set otherwise ([`El::line_gap`](crate::El::line_gap)); a box that grows across gets a
    /// line to itself.
    Wrap,
}

impl Flow {
    /// The variant as an index; and the column count, for a grid.
    #[inline(always)]
    fn kind(self) -> usize {
        match self {
            Flow::Row => 0,
            Flow::Column => 1,
            Flow::Stack => 2,
            Flow::Grid(_) => 3,
            Flow::Wrap => 4,
        }
    }
    #[inline(always)]
    fn columns(self) -> u16 {
        if let Flow::Grid(columns) = self { columns } else { 0 }
    }
}

/// Where children sit when there is room to spare.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Align {
    /// At the start: left in a row, top in a column.
    #[default]
    Start,
    /// In the middle.
    Center,
    /// At the end: right in a row, bottom in a column.
    End,
    /// Along the flow: spare room goes between children. Across it: children stretch.
    Spread,
}

/// Where a box places itself, horizontally and vertically. An [`Align`] alone sets both
/// ([`Align::End`]: the far corner, [`Align::Center`]: the middle); a pair sets each
/// (`(Align::End, Align::Center)`: right, vertically centred). (One non-zero byte, so
/// `Option<Placement>` costs boxes without one nothing: every box is copied as it is described,
/// and 8 more bytes per box made describing a window a sixth slower.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement(std::num::NonZeroU8);

impl Placement {
    /// `across` horizontally, `down` vertically.
    pub const fn new(across: Align, down: Align) -> Placement {
        Placement(std::num::NonZeroU8::new(1 + across as u8 + ((down as u8) << 2)).unwrap())
    }
    /// Horizontal alignment.
    pub const fn horizontal(self) -> Align {
        [Align::Start, Align::Center, Align::End, Align::Spread][((self.0.get() - 1) & 3) as usize]
    }
    /// Vertical alignment.
    pub const fn vertical(self) -> Align {
        [Align::Start, Align::Center, Align::End, Align::Spread][((self.0.get() - 1) >> 2) as usize]
    }
}

impl From<Align> for Placement {
    #[inline]
    fn from(both: Align) -> Placement {
        Placement::new(both, both)
    }
}
impl From<(Align, Align)> for Placement {
    #[inline]
    fn from((across, down): (Align, Align)) -> Placement {
        Placement::new(across, down)
    }
}

/// A box's content besides child boxes.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum Content {
    #[default]
    None,
    /// Text in the tree's `strings`, `start` to `end`. `side` aligns its lines in a wider box.
    Text { start: u32, end: u32, font: Font, color: Color, wrap: bool, side: Align },
    /// An icon, by index in the UI's list.
    Icon { icon: u32, color: Color },
    /// The app's own shape, by index in the UI's list, which also says how it is drawn.
    Shape { shape: u32 },
    /// An image the UI keeps, by index. (For these three, `keep` draws it aspect-fit: see
    /// [`El::keep_aspect`](crate::El::keep_aspect).)
    Picture { picture: u32, keep: bool },
    /// An SVG drawing, by index, with the colour for its `currentColor` parts.
    Drawing { drawing: u32, color: Color, keep: bool },
    /// An animation, by index, and its current frame.
    Animation { animation: u32, frame: u32, keep: bool },
    /// A 3D scene of triangle meshes: which of this run's.
    Scene { scene: u32 },
    /// No content, but a gradient fill: the UI's stops `stops[0]` to `stops[1]`, along a line
    /// at `turn` degrees from straight up, or with none, radial from the centre.
    Gradient { stops: [u32; 2], turn: Option<f32> },
    /// No content, but every other line of child boxes filled with `color`: a grid's rows, any
    /// other flow's boxes (see [`El::stripes`](crate::El::stripes)).
    Stripes { color: Color },
}

impl Content {
    /// What layout reads of it besides the text: font, and whether it wraps.
    pub fn shape(&self) -> Option<(Font, bool)> {
        match *self {
            Content::Text { font, wrap, .. } => Some((font, wrap)),
            _ => None,
        }
    }
    /// The same as a hash: equal for equal font and wrapping, 0 for non-text.
    #[inline]
    pub fn shape_key(&self) -> u64 {
        let Content::Text { font, wrap, .. } = *self else { return 0 };
        mix(Part::Text as u64, (font.face.number() as u64) << 40 | (font.size.to_bits() as u64) << 8 | (font.decoration as u64) << 3 | wrap as u64)
    }
}

/// The parts of a frame, each hashed separately; a frame's hash is all of them, each well
/// mixed, xor-ed together: so when one part changes, the hash is updated without touching the
/// others.
#[derive(Clone, Copy)]
pub(crate) enum Part {
    Flow,
    Width,
    Height,
    Least,
    Most,
    PadBefore,
    PadAfter,
    Gap,
    Align,
    Place,
    Nudge,
    Scroll,
    Clip,
    Senses,
    Focus,
    Shadow,
    Edited,
    Pinned,
    Shrink,
    /// (Not in the frame, but read by layout too: the text's font and wrapping.)
    Text,
}

/// What layout reads of a box. While none of it changes between runs, every box stays put.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Frame {
    /// The parent box.
    pub parent: u32,
    pub flow: Flow,
    /// Width and height.
    pub size: [Length; 2],
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub pad: Insets,
    pub gap: f32,
    /// Children's alignment along the flow, and across it.
    pub along: Align,
    pub across: Align,
    /// This box's own alignment across its parent's flow, if it differs from its siblings'; in
    /// a stack, both ways.
    pub place: Option<Placement>,
    /// Offset from where layout puts it; what follows does not move.
    pub nudge: [f32; 2],
    /// Which axes it scrolls on. And whether it keeps its vertical scroll bar gutter free (see
    /// `Ui::update_gutter`).
    pub scroll: [bool; 2],
    pub reserve_gutter: bool,
    /// Children are clipped to it.
    pub clip: bool,
    /// It takes pointer input.
    pub senses: bool,
    /// It takes focus: a Tab stop.
    pub focus: bool,
    /// Reach of a soft shadow around it; 0 for none.
    pub shadow: f32,
    /// Its text is being edited, so a cursor and selection marks draw just outside the text.
    pub edited: bool,
    /// It stays in view while its parent scrolls vertically: 1 at the parent's top (where it is
    /// when unscrolled), 2 at its bottom (where it is when fully scrolled); `STICKY` at the top
    /// of the nearest scroll box, while the rest of its parent passes under it; 0 scrolls with
    /// the rest.
    pub pinned: u8,
    /// How readily it shrinks along its parent's flow when short of room: 0 never; of 1 to 3,
    /// higher first. Plus `SHRINK_WIDTH`: in rows only, it narrows but never gets shorter.
    pub shrink: u8,
}

/// Added to `shrink`: shrinks its width in a row, never its height in a column.
pub(crate) const SHRINK_WIDTH: u8 = 4;
/// `Frame::pinned` for a sticky heading of what follows it in its parent: at the top of the
/// nearest vertical scroll box while that scrolls under it (see
/// [`El::sticky`](crate::El::sticky)).
pub(crate) const STICKY: u8 = 3;

/// What only sticky headings need: how far one sits below its scrolled position
/// (`Tree::pinned`), which one lies over a box at the top of a scroll view
/// (`Tree::heading_over`), and what follows a pinned one is kept below it (`Lying::pin_below`).
/// Set by the first sticky box ([`El::sticky`](crate::El::sticky)) and reached by nothing else:
/// a program without one does not carry it.
pub(crate) struct Sticks {
    pub pinned: fn(&Tree, u32) -> f32,
    pub over: fn(&Tree, u32, u32) -> Option<u32>,
    pub pin: fn(&mut crate::visible_boxes::Lying, &Tree, u32, Rect) -> bool,
}
pub(crate) static STICKING: Sticks = Sticks { pinned: Tree::pinned, over: Tree::heading_over, pin: crate::visible_boxes::Lying::pin_below };

/// Whether any value in a frame is NaN: then the frame is unequal even to itself.
#[cfg(debug_assertions)]
pub(crate) fn holds_nan(frame: &Frame) -> bool {
    let lengths = frame.size.map(|length| if let Length::Px(number) | Length::Grow(number) = length { number } else { 0.0 });
    let pad = [frame.pad.left, frame.pad.top, frame.pad.right, frame.pad.bottom];
    lengths.into_iter().chain(frame.min).chain(frame.max).chain(pad).chain(frame.nudge).chain([frame.gap, frame.shadow]).any(f32::is_nan)
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            parent: NONE,
            flow: Flow::Column,
            size: [Length::Fit; 2],
            min: [0.0; 2],
            max: [f32::INFINITY; 2],
            pad: Insets::ZERO,
            gap: 0.0,
            along: Align::Start,
            across: Align::Start,
            place: None,
            nudge: [0.0; 2],
            scroll: [false; 2],
            reserve_gutter: false,
            clip: false,
            senses: false,
            focus: false,
            shadow: 0.0,
            edited: false,
            pinned: 0,
            shrink: 0,
        }
    }
}

/// A box scrolling on an axis with no size set on it (no length, no max, no shrinking) takes
/// the room it is given there, as a growing box does: at its content's size it would have
/// nothing to scroll. Returns the change to the frame hash. (Out of line: only scroll boxes
/// come here. Per axis, so computing one part's hash stays that part's work.)
#[inline(never)]
pub(crate) fn grow_where_it_scrolls(frame: &mut Frame) -> u64 {
    let unsaid = |frame: &Frame, axis: usize| frame.scroll[axis] && frame.size[axis] == Length::Fit && frame.max[axis] == f32::INFINITY && frame.yields(axis) == 0;
    let mut change = 0;
    if unsaid(frame, 0) {
        let was = frame.part(Part::Width);
        frame.size[0] = Length::Grow(1.0);
        change ^= mix(Part::Width as u64, was) ^ mix(Part::Width as u64, frame.part(Part::Width));
    }
    if unsaid(frame, 1) {
        let was = frame.part(Part::Height);
        frame.size[1] = Length::Grow(1.0);
        change ^= mix(Part::Height as u64, was) ^ mix(Part::Height as u64, frame.part(Part::Height));
    }
    change
}

impl Frame {
    /// How readily it shrinks along `axis` (1 to 3, see `shrink`); 0 if it doesn't, as
    /// vertically for a box that only narrows.
    #[inline(always)]
    fn yields(&self, axis: usize) -> u8 {
        if axis == 1 && self.shrink & SHRINK_WIDTH != 0 { 0 } else { self.shrink & 3 }
    }
    /// One part of the frame, as a hash.
    #[inline(always)]
    pub fn part(&self, part: Part) -> u64 {
        let pair = |a: f32, b: f32| (a.to_bits() as u64) << 32 | b.to_bits() as u64;
        let length = |length: Length| match length {
            Length::Fit => 0,
            Length::Px(size) => 1 << 32 | size.to_bits() as u64,
            Length::Grow(weight) => 2 << 32 | weight.to_bits() as u64,
        };
        match part {
            Part::Flow => self.flow.kind() as u64 | (self.flow.columns() as u64) << 8,
            Part::Width => length(self.size[0]),
            Part::Height => length(self.size[1]),
            Part::Least => pair(self.min[0], self.min[1]),
            Part::Most => pair(self.max[0], self.max[1]),
            Part::PadBefore => pair(self.pad.left, self.pad.top),
            Part::PadAfter => pair(self.pad.right, self.pad.bottom),
            Part::Gap => self.gap.to_bits() as u64,
            Part::Align => self.along as u64 | (self.across as u64) << 8,
            Part::Place => self.place.map_or(16, |place| place.horizontal() as u64 | (place.vertical() as u64) << 2),
            Part::Nudge => pair(self.nudge[0], self.nudge[1]),
            Part::Scroll => self.scroll[0] as u64 | (self.scroll[1] as u64) << 1 | (self.reserve_gutter as u64) << 2,
            Part::Clip => self.clip as u64,
            Part::Senses => self.senses as u64,
            Part::Focus => self.focus as u64,
            Part::Shadow => self.shadow.to_bits() as u64,
            Part::Edited => self.edited as u64,
            Part::Pinned => self.pinned as u64,
            Part::Shrink => self.shrink as u64,
            Part::Text => 0,
        }
    }
    /// A hash equal for equal frames and practically never otherwise: every part except the
    /// parent. A run is compared with the last by these.
    pub fn sum(&self) -> u64 {
        const PARTS: [Part; 19] = [
            Part::Flow,
            Part::Width,
            Part::Height,
            Part::Least,
            Part::Most,
            Part::PadBefore,
            Part::PadAfter,
            Part::Gap,
            Part::Align,
            Part::Place,
            Part::Nudge,
            Part::Scroll,
            Part::Clip,
            Part::Senses,
            Part::Focus,
            Part::Shadow,
            Part::Edited,
            Part::Pinned,
            Part::Shrink,
        ];
        PARTS.iter().fold(0, |sum, part| sum ^ mix(*part as u64, self.part(*part)))
    }
    fn pad_before(&self, axis: usize) -> f32 {
        if axis == 0 { self.pad.left } else { self.pad.top }
    }
    fn pad_along(&self, axis: usize) -> f32 {
        if axis == 0 { self.pad.horizontal() } else { self.pad.vertical() }
    }
    /// Whether children follow one another along this axis.
    fn flows(&self, axis: usize) -> bool {
        matches!((self.flow, axis), (Flow::Row, 0) | (Flow::Column, 1))
    }
}

/// What the app set on a box this run.
#[derive(Clone, Copy, Default)]
pub(crate) struct Spec {
    pub frame: Frame,
    /// The index of the first box after this one's subtree.
    pub end: u32,
    /// Its id across runs; 0 for a box nothing asks about.
    pub id: Id,
    /// Scroll offset.
    pub offset: [f32; 2],
    pub fill: Color,
    pub border: Color,
    pub border_width: f32,
    pub radius: f32,
    pub content: Content,
    /// For text being edited: selection anchor and cursor, in bytes from the text's start; and
    /// whether the cursor is drawn now (it blinks).
    pub select: Option<[u32; 2]>,
    pub caret: bool,
    /// How much it and its content are faded when drawn: 0 not at all, 255 fully (hidden,
    /// keeping its room: see [`El::opacity`](crate::El::opacity) and
    /// [`El::hidden`](crate::El::hidden)).
    pub fade: u8,
}

/// A fade's contribution to the layout hash: a box hidden or shown is surveyed again, and one
/// faded more or less is redrawn.
#[inline(always)]
pub(crate) fn fade_key(fade: u8) -> u64 {
    if fade == 0 { 0 } else { mix(0x6661_6465, fade as u64) }
}

impl Spec {
    /// The hash of everything layout reads of this box, except the parent: 0 for an empty spec.
    /// (While a box is built the hash is updated part by part; this computes it from the whole,
    /// the slow and certain way.)
    pub fn sum(&self) -> u64 {
        self.frame.sum() ^ Frame::default().sum() ^ self.content.shape_key() ^ fade_key(self.fade)
    }
}

/// What layout computed for a box.
#[derive(Clone, Copy, Default)]
pub(crate) struct Place {
    /// Its requested size.
    pub fit: [f32; 2],
    /// Its corner, relative to its parent's.
    pub pos: [f32; 2],
    pub dim: [f32; 2],
    /// How much room its children take (see `Tree::extent`).
    taken_up: [f32; 2],
    /// Its wrapped lines in the tree's `lines`; the line height; and how far above its line
    /// slot the text is drawn (see [`Metrics::lift`](crate::text::Metrics::lift)).
    pub lines: (u32, u32),
    pub line_height: f32,
    pub lift: f32,
    /// In a wrapping row: which line it is on.
    on_line: u32,
    /// The bounds of it and its content, from its own corner: drawing skips a box whose bounds
    /// are out of view, without looking inside.
    pub reach: Rect,
    /// The room its kept-in-view boxes take at its top and bottom: scrolled content shows
    /// between.
    pub bands: [f32; 2],
    // What its children report on the way up:
    sum: f32,
    most: f32,
    count: f32,
    fixed: f32,
    weight: f32,
    /// How much its shrinking children can give, at each of the three readiness levels.
    give: [f32; 3],
    // What it has for its children on the way down:
    available: f32,
    spare: f32,
    shares: f32,
    pen: f32,
    between: f32,
    /// How much its children overflow its room; and how much the shrinking ones can give, as
    /// reported on the way up.
    over: f32,
    gave: [f32; 3],
}

impl Place {
    /// How much a child of this box shrinks with readiness `gives` and `can` to give: nothing
    /// until the readier ones have given all they can; then its share, with others as ready, of
    /// the remaining overflow.
    #[inline]
    fn takes(&self, gives: u8, can: f32) -> f32 {
        let (first, mine) = self.gave.split_at(gives as usize);
        let left = (self.over - first.iter().sum::<f32>()).max(0.0);
        (left * can / mine[0].max(f32::MIN_POSITIVE)).min(can)
    }
}

/// One run's boxes, in pre-order.
#[derive(Default)]
pub(crate) struct Tree {
    pub specs: Vec<Spec>,
    pub places: Vec<Place>,
    /// All the run's text, back to back.
    pub strings: String,
    /// The lines of every wrapping text.
    pub lines: Vec<Line>,
    /// The boxes whose text wraps.
    pub wrapping: Vec<u32>,
    stops: Vec<f32>,
    /// Each text's single-line width, its line height and its lift: as measured the first
    /// time, kept between runs.
    sizes: ById<[f32; 3]>,
    /// The columns or rows of the grid being laid out: size and growth weight, then start and
    /// size.
    tracks: Vec<[f32; 2]>,
    /// The scroll boxes holding a list so large it is laid out from a row near the view, not
    /// its first (see [`Ui::rows`](crate::Ui::rows)): each with how far up its content is
    /// shifted for the rows left out.
    pub shifted: Vec<(u32, f32)>,
    /// The boxes that set a gap between lines or grid rows other than their gap
    /// ([`El::line_gap`](crate::El::line_gap)), each with that gap, in order. (Kept beside the
    /// boxes, not in them: every box is copied as it is described, and its record has no room
    /// for a value nearly every box leaves unset.)
    pub line_gaps: Vec<(u32, f32)>,
    /// What sticky headings need, once a box sticks (see `Sticks`).
    pub sticks: Option<&'static Sticks>,
}

impl Tree {
    /// Forgets last run's boxes. (Their places stay: they may still hold. So does their text,
    /// which the next run mostly repeats.)
    pub fn clear(&mut self) {
        self.specs.clear();
        self.wrapping.clear();
        self.shifted.clear();
        self.line_gaps.clear();
    }
    /// The gap between lines of box `at` (a wrapping row) or between a grid's rows: its gap,
    /// unless set otherwise ([`El::line_gap`](crate::El::line_gap)).
    fn line_gap(&self, at: u32) -> f32 {
        let said = self.line_gaps.partition_point(|(of, _)| *of <= at).checked_sub(1).map(|last| self.line_gaps[last]);
        said.filter(|(of, _)| *of == at).map_or(self.specs[at as usize].frame.gap, |(_, gap)| gap)
    }
    /// How far box `at`'s content is shifted along `axis` for the rows of a large list left out
    /// of it: it scrolls that much less, and has that much less to scroll.
    pub fn shift(&self, at: u32, axis: usize) -> f32 {
        self.shifted.iter().find(|(shifted, _)| *shifted == at && axis == 1).map_or(0.0, |(_, by)| *by)
    }

    /// The direct children of box `of`.
    pub fn children(&self, of: u32) -> impl Iterator<Item = u32> + '_ {
        let stop = self.specs[of as usize].end;
        // Skips each child's subtree; only boxes whose parent this is count. (A layer started
        // in the middle of this box lies between, and is not in it.)
        let all = std::iter::successors(Some(of + 1), move |at| self.specs.get(*at as usize).map(|spec| spec.end));
        all.take_while(move |at| *at < stop).filter(move |at| self.specs[*at as usize].frame.parent == of)
    }

    /// Box `at`'s rect in the window.
    pub fn rect_of(&self, at: u32) -> Rect {
        let place = &self.places[at as usize];
        let (mut x, mut y, mut up) = (place.pos[0], place.pos[1] + self.stuck(at), self.specs[at as usize].frame.parent);
        while let Some(spec) = self.specs.get(up as usize) {
            let around = &self.places[up as usize];
            (x, y, up) = (x + around.pos[0] - spec.offset[0], y + around.pos[1] - spec.offset[1] + self.stuck(up), spec.frame.parent);
        }
        rect(x, y, place.dim[0], place.dim[1])
    }
    /// How far below its scrolled position box `at` sits: zero, except for a box kept in view.
    /// One kept at the top sits where it would unscrolled, one at the bottom where it would
    /// fully scrolled; a sticky heading, as `pinned` says.
    pub fn stuck(&self, at: u32) -> f32 {
        let frame = &self.specs[at as usize].frame;
        match frame.pinned {
            0 => 0.0,
            1 => self.specs[frame.parent as usize].offset[1],
            2 => self.specs[frame.parent as usize].offset[1] - self.scroll_end(frame.parent, 1),
            _ => self.sticks.map_or(0.0, |sticks| (sticks.pinned)(self, at)),
        }
    }
    /// How far below its scrolled position a sticky heading (`STICKY`) at `at` sits: where it
    /// would scroll above the top of its nearest vertical scroll box (or the page), it sits at
    /// that top; but no lower than keeps it inside its parent, and above the next sticky
    /// heading there. (Only computed for such a heading: it finds what it needs walking up its
    /// ancestors and along its following siblings.)
    fn pinned(&self, at: u32) -> f32 {
        let (parent, high) = (self.specs[at as usize].frame.parent, self.places[at as usize].dim[1]);
        // Where its parent's content coordinates start, relative to the scroll box's content
        // coordinates.
        let (mut from, mut up) = (0.0, parent);
        while up != NONE && !self.scrolls(up, 1) {
            from += self.places[up as usize].pos[1] - self.specs[up as usize].offset[1];
            up = self.specs[up as usize].frame.parent;
        }
        if up == NONE {
            return 0.0;
        }
        // The top of the view: below the scroll box's top kept-in-view boxes.
        let (scrolls, [top, _]) = (&self.specs[up as usize], self.places[up as usize].bands);
        let line = scrolls.offset[1] + if top > 0.0 { scrolls.frame.pad.top + top } else { 0.0 };
        // How far down it may go: to its parent's end (of all its content, if the parent is the
        // scroll box), and to the next sticky heading there.
        let end = match parent == up {
            true => scrolls.frame.pad.top + self.extent(up, 1),
            false => from + self.specs[parent as usize].offset[1] + self.places[parent as usize].dim[1],
        };
        let (mut next, stop) = (self.specs[at as usize].end, self.specs[parent as usize].end);
        while next < stop && !(self.specs[next as usize].frame.parent == parent && self.specs[next as usize].frame.pinned == STICKY) {
            next = self.specs[next as usize].end;
        }
        let end = if next < stop { end.min(from + self.places[next as usize].pos[1]) } else { end };
        let natural = from + self.places[at as usize].pos[1];
        (line.min(end - high) - natural).max(0.0)
    }
    /// The sticky heading (`STICKY`) over box `at` while it is at the top of what `within`
    /// scrolls: the last one before it in one of its ancestors, inside `within`. `None` if
    /// there is none, and for content of a sticky heading (which is at the top itself then).
    fn heading_over(&self, at: u32, within: u32) -> Option<u32> {
        let mut inner = at;
        while inner != NONE && inner != within {
            if self.specs[inner as usize].frame.pinned == STICKY {
                return None;
            }
            let parent = self.specs[inner as usize].frame.parent;
            let heads = self.children(parent).take_while(|child| *child < inner).filter(|child| self.specs[*child as usize].frame.pinned == STICKY).last();
            if heads.is_some() {
                return heads;
            }
            inner = parent;
        }
        None
    }

    /// The room box `at` has for its children along `axis`. (Less the scroll bar gutter:
    /// vertically the bottom one, if it scrolls sideways; horizontally the side one, if it
    /// keeps that free.)
    pub fn available(&self, at: u32, axis: usize) -> f32 {
        let frame = &self.specs[at as usize].frame;
        let strip = if axis == 1 && self.has_bar(at, 0) || axis == 0 && frame.reserve_gutter { BAR } else { 0.0 };
        self.places[at as usize].dim[axis] - frame.pad_along(axis) - strip
    }
    /// The part of box `at` (at `r`) its content shows in: all but those gutters.
    pub fn view(&self, at: u32, r: Rect) -> Rect {
        let (side, under) = (self.specs[at as usize].frame.reserve_gutter, self.has_bar(at, 0));
        rect(r.x, r.y, r.w - if side { BAR } else { 0.0 }, r.h - if under { BAR } else { 0.0 })
    }
    /// The part of box `at` (at `r`) its scrolled content shows in: its view between the
    /// kept-in-view boxes at its top and bottom.
    pub fn scrolled_view(&self, at: u32, r: Rect) -> Rect {
        let (view, [top, bottom], pad) = (self.view(at, r), self.places[at as usize].bands, self.specs[at as usize].frame.pad);
        if top + bottom == 0.0 {
            return view;
        }
        let (top, bottom) = (if top > 0.0 { pad.top + top } else { 0.0 }, if bottom > 0.0 { pad.bottom + bottom } else { 0.0 });
        rect(view.x, view.y + top, view.w, (view.h - top - bottom).max(0.0))
    }
    /// The room box `at`'s children take along `axis`: more than it has, in a scroll box.
    pub fn extent(&self, at: u32, axis: usize) -> f32 {
        let (frame, taken) = (&self.specs[at as usize].frame, self.places[at as usize].taken_up[axis]);
        if frame.flows(axis) { (taken - frame.gap).max(0.0) } else { taken }
    }
    /// Whether box `at` has more content along `axis` than room, so it scrolls. (The page, by
    /// more than half a unit: its children's shares may add up to a little more than it.)
    pub fn overflows(&self, at: u32, axis: usize) -> bool {
        self.specs[at as usize].frame.scroll[axis] && self.extent(at, axis) > self.available(at, axis) || axis == 1 && at == PAGE && self.extent(at, 1) > self.available(at, 1) + 0.5
    }
    /// Whether box `at` scrolls along `axis`: a box set to ([`El::scroll`](crate::El::scroll)),
    /// and the page vertically. The page is laid out as a non-scrolling box, so growing
    /// children share the window's height as usual; only if its content still comes out taller
    /// than the window is there more to scroll to.
    pub fn scrolls(&self, at: u32, axis: usize) -> bool {
        self.specs[at as usize].frame.scroll[axis] || axis == 1 && at == PAGE
    }
    /// Whether box `at` shows a scroll bar along `axis`: when it overflows that way, unless too
    /// thin for one: a box one text line high has no room for a horizontal bar, and scrolls by
    /// wheel (and typing).
    pub fn has_bar(&self, at: u32, axis: usize) -> bool {
        self.overflows(at, axis) && self.places[at as usize].dim[1 - axis] >= 4.0 * BAR
    }
    /// How far box `at` scrolls along `axis`: until its content's end shows. (Not at all for a
    /// page no taller than the window.)
    pub fn scroll_end(&self, at: u32, axis: usize) -> f32 {
        if at == PAGE && !self.overflows(at, axis) {
            return 0.0;
        }
        (self.extent(at, axis) - self.available(at, axis)).max(0.0)
    }
    /// Everything box `at` draws on, from its own corner: the box, its shadow (offset a quarter
    /// down), and for edited text the cursor and selection marks just outside it. Unclipped
    /// text may also reach past the box: a "T" or "j" overhangs its measured advance, and
    /// systems may fringe glyph edges. Text is also drawn a little above or below its line slot
    /// (see [`Metrics::lift`](crate::text::Metrics::lift)): an accent may then leave the box. A
    /// line taller than its box (large text in a fixed-height part) overflows above and below
    /// alike; a wider one (large text in a fixed-width column), on either side. (So when only
    /// text changes, the redraw covers all of it: nothing of the old glyphs is left.)
    pub fn drawn(&self, at: u32) -> Rect {
        let (spec, place) = (&self.specs[at as usize], &self.places[at as usize]);
        let (dim, frame) = (place.dim, &spec.frame);
        let around = rect(0.0, 0.0, dim[0], dim[1]).grow((frame.shadow * 1.25).max(if frame.edited { 4.0 } else { 0.0 }));
        let Content::Text { wrap, .. } = spec.content else { return around };
        // Horizontally, only where the box doesn't clip its text, and only beyond what its
        // padding already absorbs: a button's text ends well inside the button. A line wider
        // than an unclipped box overflows by that much, on either side, as it may be aligned to
        // either.
        let wider = if wrap { 0.0 } else { (place.fit[0] - dim[0]).max(0.0).ceil() };
        let (past, [up, down]) = (if frame.clip { 0.0 } else { (OVERHANG - frame.pad.left.min(frame.pad.right)).max(0.0) + wider }, self.lifted(at));
        let spill = if wrap || frame.clip { 0.0 } else { ((place.line_height - dim[1] + frame.pad.vertical()) / 2.0).max(0.0).ceil() };
        around.union(rect(-past, -up - spill, dim[0] + 2.0 * past, dim[1] + up + down + 2.0 * spill))
    }

    /// How far box `at`'s text may reach out of the box at the top and bottom, from being drawn
    /// above or below its line slot (see [`Metrics::lift`](crate::text::Metrics::lift)): beyond
    /// what the box's room around its text absorbs (its padding, and for one line in a taller
    /// box, half the excess). Zero for a box without text.
    pub fn lifted(&self, at: u32) -> [f32; 2] {
        let (spec, place) = (&self.specs[at as usize], &self.places[at as usize]);
        let Content::Text { wrap, .. } = spec.content else { return [0.0; 2] };
        let over = if wrap { 0.0 } else { ((place.dim[1] - spec.frame.pad.vertical() - place.line_height) / 2.0).max(0.0) };
        // A line is placed on whole pixels: lifted by a fraction, it may draw a whole pixel
        // higher.
        let (up, down) = (place.lift.max(0.0).ceil(), (-place.lift).max(0.0).ceil());
        [(up - spec.frame.pad.top - over).max(0.0), (down - spec.frame.pad.bottom - over).max(0.0)]
    }

    /// Walks `root`'s subtree in pre-order. `visit(box, true)` is called entering a box and
    /// returns whether to descend; `visit(box, false)` leaving a box that was entered, after
    /// its subtree. `open` is the stack, reused.
    pub fn walk(&self, root: u32, open: &mut Vec<u32>, visit: &mut impl FnMut(u32, bool) -> bool) {
        let (stop, mut at) = (self.specs[root as usize].end, root);
        open.clear();
        loop {
            // Leave every box that ends here.
            if let Some(done) = open.pop_if(|top| self.specs[*top as usize].end <= at) {
                visit(done, false);
                continue;
            }
            if at >= stop {
                return;
            }
            let spec = &self.specs[at as usize];
            let inside = at == root || open.last() == Some(&spec.frame.parent);
            if inside && visit(at, true) {
                open.push(at);
                at += 1;
            } else {
                at = spec.end;
            }
        }
    }

    /// Lays out every box. `roots` are boxes in no other box: each gets all of `size`.
    pub fn layout(&mut self, roots: impl Iterator<Item = u32>, size: [f32; 2], text: &mut dyn TextSystem, scale: f32) {
        self.places.resize(self.specs.len(), Place::default());
        for root in roots {
            (self.places[root as usize].pos, self.places[root as usize].dim) = ([0.0; 2], size);
        }
        if self.sizes.len() > 8192 {
            self.sizes.clear(); // kept small: forgetting only costs measuring again
        }
        self.fit(0, text, scale);
        self.place(0);
        self.wrap(text, scale);
        self.fit(1, text, scale);
        self.place(1);
        self.reach();
    }

    /// Bottom-up: the size each box wants along `axis`. Each, once it knows, tells its parent.
    fn fit(&mut self, axis: usize, text: &mut dyn TextSystem, scale: f32) {
        for i in (0..self.specs.len()).rev() {
            let own = self.text_size(i, axis, text, scale);
            // Grids and wrapping rows compute their children's sizes themselves.
            let grid = match self.specs[i].frame.flow {
                Flow::Grid(columns) => Some(self.measure_tracks(i as u32, axis, columns as usize)),
                Flow::Wrap => Some(self.measure_lines(i as u32, axis)),
                _ => None,
            };
            // A box fitting its content is taller by a horizontal bar's gutter if it overflows
            // sideways; one fitting its width is wider by the side gutter it keeps free.
            let strip = if axis == 1 && self.overflows(i as u32, 0) || axis == 0 && self.specs[i].frame.reserve_gutter { BAR } else { 0.0 };
            let (frame, place) = (&self.specs[i].frame, &mut self.places[i]);
            // The content: along the flow sizes add up, across it the largest wins.
            let inside = if frame.flows(axis) { own + place.sum + frame.gap * (place.count - 1.0).max(0.0) } else { own.max(place.most) };
            let want = match frame.size[axis] {
                Length::Px(size) => size,
                _ => grid.unwrap_or(inside) + frame.pad_along(axis) + if axis == 0 || inside + frame.pad_along(axis) >= 3.0 * BAR { strip } else { 0.0 },
            };
            let want = want.max(frame.min[axis]).min(frame.max[axis]);
            (place.fit[axis], place.sum, place.most) = (want, 0.0, 0.0);
            let Some(around) = self.specs.get(frame.parent as usize) else { continue };
            let up = &mut self.places[frame.parent as usize];
            (up.sum, up.most, up.count) = (up.sum + want, up.most.max(want), up.count + 1.0);
            // A growing box takes a share of what the others leave, by weight.
            match frame.size[axis] {
                Length::Grow(weight) if !around.frame.scroll[axis] => up.weight += weight,
                _ => up.fixed += want,
            }
            // A shrinking box reports how much it can give, readiest first: 3, then 2, then 1.
            if frame.yields(axis) != 0 && around.frame.flows(axis) {
                up.give[3 - frame.yields(axis) as usize] += (want - frame.min[axis]).max(0.0);
            }
        }
    }

    /// The size of box `i`'s own text along `axis`: its single-line width, or its lines'
    /// height. The system is asked only about text not measured before.
    fn text_size(&mut self, i: usize, axis: usize, text: &mut dyn TextSystem, scale: f32) -> f32 {
        let Content::Text { start, end, font, wrap, .. } = self.specs[i].content else { return 0.0 };
        let place = &mut self.places[i];
        if axis == 1 {
            return if wrap { (place.lines.1 - place.lines.0) as f32 * place.line_height } else { place.line_height };
        }
        let piece = &self.strings[start as usize..end as usize];
        let key = hash((piece, font.face, font.size.to_bits(), scale.to_bits()));
        // With the line's lift in its slot, which is a whole number of units tall.
        let sized = |metrics: crate::text::Metrics, wide: f32| {
            let height = metrics.height().ceil();
            [wide.ceil(), height, metrics.lift(height)]
        };
        let size = self.sizes.entry(key).or_insert_with(|| sized(text.metrics(font, scale), text.measure(piece, font, scale, None)));
        (place.line_height, place.lift) = (size[1], size[2]);
        size[0]
    }

    /// Forgets all text sizes: text is to be drawn with another font.
    pub fn forget_sizes(&mut self) {
        self.sizes.clear();
    }

    /// Box `at`'s single-line text width, as measured for layout.
    pub fn text_width(&self, at: u32, scale: f32) -> Option<f32> {
        let Content::Text { start, end, font, .. } = self.specs[at as usize].content else { return None };
        let piece = &self.strings[start as usize..end as usize];
        self.sizes.get(&hash((piece, font.face, font.size.to_bits(), scale.to_bits()))).map(|size| size[0])
    }

    /// Top-down: each box takes its size and position along `axis` from its parent, then
    /// computes what that leaves for its own children.
    fn place(&mut self, axis: usize) {
        for i in 0..self.specs.len() {
            self.seat(i, axis);
            self.share(i, axis);
        }
    }

    /// Gives box `i` its size and position along `axis` from its parent's room. (A root keeps
    /// what it was given; grids and wrapping rows seat their own children.)
    fn seat(&mut self, i: usize, axis: usize) {
        let frame = &self.specs[i].frame;
        let Some(around) = self.specs.get(frame.parent as usize).map(|spec| &spec.frame) else { return };
        if matches!(around.flow, Flow::Grid(_) | Flow::Wrap) {
            return;
        }
        let (before, from) = self.places.split_at_mut(i);
        let (up, place) = (&mut before[frame.parent as usize], &mut from[0]);
        if around.flows(axis) {
            // In sequence: at the pen, which then advances.
            place.dim[axis] = match frame.size[axis] {
                Length::Grow(weight) if !around.scroll[axis] => (up.spare * weight / up.shares.max(f32::MIN_POSITIVE)).max(frame.min[axis]).min(frame.max[axis]),
                _ if frame.yields(axis) != 0 && up.over > 0.0 => place.fit[axis] - up.takes(3 - frame.yields(axis), (place.fit[axis] - frame.min[axis]).max(0.0)),
                _ => place.fit[axis],
            };
            place.pos[axis] = up.pen + frame.nudge[axis];
            (up.pen, up.taken_up[axis]) = (up.pen + place.dim[axis] + up.between, up.taken_up[axis] + place.dim[axis] + around.gap);
            // A kept-in-view box takes its height, and the gap after it, from the scrolled
            // area.
            if matches!(frame.pinned, 1 | 2) && axis == 1 {
                up.bands[frame.pinned as usize - 1] += place.dim[1] + up.between;
            }
            return;
        }
        // Across the flow, and in a stack: each child on its own.
        let own = frame.place.map(|place| [place.horizontal(), place.vertical()][axis]);
        let how = own.unwrap_or(if around.flow == Flow::Stack && axis == 0 { around.along } else { around.across });
        let fills = matches!(frame.size[axis], Length::Grow(_)) || (how == Align::Spread && frame.size[axis] == Length::Fit);
        let keeps = around.scroll[axis] || matches!(frame.size[axis], Length::Px(_));
        // In a box scrolling this way, a filling child is no smaller than its content.
        let want = if fills && around.scroll[axis] {
            up.available.max(place.fit[axis])
        } else if fills {
            up.available
        } else if keeps {
            place.fit[axis]
        } else {
            place.fit[axis].min(up.available)
        };
        place.dim[axis] = want.max(frame.min[axis]).min(frame.max[axis]);
        let spare = match how {
            Align::Center => (up.available - place.dim[axis]) / 2.0,
            Align::End => up.available - place.dim[axis],
            _ => 0.0,
        };
        place.pos[axis] = around.pad_before(axis) + spare + frame.nudge[axis];
        up.taken_up[axis] = up.taken_up[axis].max(place.dim[axis]);
    }

    /// Computes what box `i` has for its children along `axis`: room, spare room, and the first
    /// child's position.
    fn share(&mut self, i: usize, axis: usize) {
        match self.specs[i].frame.flow {
            Flow::Grid(columns) => self.seat_grid(i as u32, axis, columns as usize),
            Flow::Wrap => self.seat_lines(i as u32, axis),
            _ => {}
        }
        let room = self.available(i as u32, axis);
        let (frame, place) = (&self.specs[i].frame, &mut self.places[i]);
        let spare = (room - place.fixed - frame.gap * (place.count - 1.0).max(0.0)).max(0.0);
        // Spare room no child takes goes before the children, or between them.
        let (mut pen, mut between) = (frame.pad_before(axis), frame.gap);
        // In a scroll box too, while the content is smaller than it: a chat's few messages at
        // the bottom. Once larger, nothing is spare.
        match frame.along {
            _ if place.weight > 0.0 => {}
            Align::Center => pen += spare / 2.0,
            Align::End => pen += spare,
            Align::Spread if place.count > 1.0 => between += spare / (place.count - 1.0),
            _ => {}
        }
        (place.available, place.spare, place.shares, place.pen, place.between) = (room, spare, place.weight, pen, between);
        // How much the children overflow, where they flow this way and nothing scrolls: the
        // shrinking ones make up for it between them.
        let over = place.fixed + frame.gap * (place.count - 1.0).max(0.0) - room;
        place.over = if frame.flows(axis) && !frame.scroll[axis] { over.max(0.0) } else { 0.0 };
        place.gave = std::mem::take(&mut place.give);
        // The children's reports are used up; grids and wrapping rows have just reported their
        // content size, the others will as they are seated.
        (place.count, place.fixed, place.weight) = (0.0, 0.0, 0.0);
        if !matches!(frame.flow, Flow::Grid(_) | Flow::Wrap) {
            place.taken_up[axis] = 0.0;
        }
        if axis == 1 {
            (place.reach, place.bands) = (Rect::EMPTY, [0.0; 2]);
        }
        let (spare, shares) = (self.places[i].spare, self.places[i].shares);
        if shares > 0.0 && self.specs[i].frame.flows(axis) && !self.specs[i].frame.scroll[axis] {
            (self.places[i].spare, self.places[i].shares) = self.held(i as u32, axis, spare, shares);
        }
    }

    /// What box `i` shares out along `axis` among growing children when some hit a min or max:
    /// what a maxed child can't take goes to the others, and what a minned child needs comes
    /// out of theirs. (The breakpoints where a child starts or stops being clamped are swept
    /// once, in order.) Returns the spare room and shares to seat them by: units per weight,
    /// and 1; or `spare` and `shares` unchanged where no child is clamped.
    fn held(&mut self, i: u32, axis: usize, spare: f32, shares: f32) -> (f32, f32) {
        let (mut turns, mut least) = (std::mem::take(&mut self.tracks), 0.0);
        turns.clear();
        for child in self.children(i) {
            let frame = &self.specs[child as usize].frame;
            let Length::Grow(weight) = frame.size[axis] else { continue };
            let (low, high) = (frame.min[axis], frame.max[axis].max(frame.min[axis]));
            least += low;
            // From `low / weight` on, a unit of weight gives it more; from `high / weight` on,
            // not.
            turns.extend((weight > 0.0).then_some([[low / weight, weight], [high / weight, -weight]]).into_iter().flatten());
        }
        if least == 0.0 && !turns.iter().any(|[turn, change]| *change < 0.0 && turn.is_finite()) {
            self.tracks = turns;
            return (spare, shares);
        }
        // Sorted by breakpoint with an insertion sort over few items: the standard sort would
        // add about a thousand bytes to the program.
        for at in 1..turns.len() {
            let mut back = at;
            while back > 0 && turns[back - 1][0] > turns[back][0] {
                turns.swap(back - 1, back);
                back -= 1;
            }
        }
        // Along the breakpoints: units per weight so far, what the children take there, and how
        // much more per further unit.
        let (mut at, mut have, mut slope, mut rate) = (0.0f32, least, 0.0f32, None);
        for [turn, change] in turns.iter().copied() {
            let reach = if slope > 0.0 { have + slope * (turn - at) } else { have };
            if reach >= spare {
                rate = Some(if slope > 0.0 { at + (spare - have).max(0.0) / slope } else { at });
                break;
            }
            (at, have, slope) = (turn, reach, slope + change);
        }
        self.tracks = turns;
        (rate.unwrap_or(at), 1.0)
    }

    /// For a grid: each column's width (`axis` 0) or row's height (1): its largest box's
    /// request; and its growth weight: its greediest box's. Left in `tracks`. Returns the
    /// tracks' total, with the gaps.
    fn measure_tracks(&mut self, grid: u32, axis: usize, columns: usize) -> f32 {
        let mut tracks = std::mem::take(&mut self.tracks);
        tracks.clear();
        for (seat, child) in self.children(grid).enumerate() {
            let track = if axis == 0 { seat % columns.max(1) } else { seat / columns.max(1) };
            if track == tracks.len() {
                tracks.push([0.0; 2]);
            }
            let eager = if let Length::Grow(weight) = self.specs[child as usize].frame.size[axis] { weight } else { 0.0 };
            tracks[track] = [tracks[track][0].max(self.places[child as usize].fit[axis]), tracks[track][1].max(eager)];
        }
        let gap = if axis == 0 { self.specs[grid as usize].frame.gap } else { self.line_gap(grid) };
        let gaps = gap * tracks.len().saturating_sub(1) as f32;
        self.tracks = tracks;
        self.tracks.iter().map(|track| track[0]).sum::<f32>() + gaps
    }

    /// Gives a grid's boxes their positions along `axis`: each in its column, or row.
    fn seat_grid(&mut self, grid: u32, axis: usize, columns: usize) {
        let used = self.measure_tracks(grid, axis, columns);
        let frame = self.specs[grid as usize].frame;
        let gap = if axis == 0 { frame.gap } else { self.line_gap(grid) };
        let spare = (self.available(grid, axis) - used).max(0.0);
        // Spare room goes to the growing tracks; then each track's size becomes its start and
        // size.
        let mut tracks = std::mem::take(&mut self.tracks);
        let eager = tracks.iter().map(|track| track[1]).sum::<f32>().max(f32::MIN_POSITIVE);
        let mut from = frame.pad_before(axis);
        for track in &mut tracks {
            let size = track[0] + spare * track[1] / eager;
            (*track, from) = ([from, size], from + size + gap);
        }
        let (mut child, mut seat, end) = (grid + 1, 0, self.specs[grid as usize].end);
        while child < end {
            // On to the next box after this one's subtree. (A box here that isn't the grid's
            // own is a layer started in it: skipped.)
            let (cell, place) = (&self.specs[child as usize].frame, &mut self.places[child as usize]);
            child = self.specs[child as usize].end;
            if cell.parent != grid {
                continue;
            }
            let [from, room] = tracks[if axis == 0 { seat % columns.max(1) } else { seat / columns.max(1) }];
            seat += 1;
            // Horizontally a box is its column's width. Vertically it keeps its own height
            // unless it grows, aligned in its row as the grid says.
            let fills = (axis == 0 && !matches!(cell.size[0], Length::Px(_))) || matches!(cell.size[axis], Length::Grow(_)) || frame.across == Align::Spread;
            let want = if fills { room } else { place.fit[axis].min(room) };
            place.dim[axis] = want.max(cell.min[axis]).min(cell.max[axis]);
            let spare = match (axis, frame.across) {
                (1, Align::Center) => (room - place.dim[axis]) / 2.0,
                (1, Align::End) => room - place.dim[axis],
                _ => 0.0,
            };
            place.pos[axis] = from + spare + cell.nudge[axis];
        }
        self.places[grid as usize].taken_up[axis] = (from - gap - frame.pad_before(axis)).max(0.0);
        self.tracks = tracks;
    }

    /// For a wrapping row: what its boxes request along `axis`. Horizontally, all on one line
    /// (the row then gets what there is, and wraps to that). Vertically, its lines stacked,
    /// each as high as its tallest box: by then every box knows its line. Line heights are left
    /// in `tracks`.
    fn measure_lines(&mut self, row: u32, axis: usize) -> f32 {
        let (mut tracks, gap) = (std::mem::take(&mut self.tracks), self.specs[row as usize].frame.gap);
        tracks.clear();
        let mut one_line = 0.0;
        for child in self.children(row) {
            let place = &self.places[child as usize];
            one_line += place.fit[0] + gap;
            tracks.resize(tracks.len().max(place.on_line as usize + 1), [0.0; 2]);
            tracks[place.on_line as usize][0] = tracks[place.on_line as usize][0].max(place.fit[1]);
        }
        let high = tracks.iter().map(|line| line[0]).sum::<f32>() + self.line_gap(row) * tracks.len().saturating_sub(1) as f32;
        self.tracks = tracks;
        if axis == 0 { (one_line - gap).max(0.0) } else { high }
    }

    /// Gives a wrapping row's boxes their sizes and positions along `axis`. Horizontally: each
    /// as wide as it asks (its content's width, never wider than the row; a set width; the
    /// whole row if it grows), wrapping to the next line when it doesn't fit; spare room on a
    /// line goes where the row's alignment says. Vertically: each on its line, aligned in the
    /// line's height as the row's cross alignment says.
    fn seat_lines(&mut self, row: u32, axis: usize) {
        let (frame, room, end, down) = (self.specs[row as usize].frame, self.available(row, axis), self.specs[row as usize].end, self.line_gap(row));
        if axis == 0 {
            self.break_lines(row, room);
        } else {
            self.measure_lines(row, 1);
        }
        // Each line's size becomes its start and size: horizontally where its first box goes
        // and the gap between its boxes; vertically its top and height.
        let mut tracks = std::mem::take(&mut self.tracks);
        let (mut top, mut widest) = (frame.pad_before(1), 0.0f32);
        for line in &mut tracks {
            let [size, count] = *line;
            let spare = (room - size).max(0.0);
            let (first, between) = match frame.along {
                Align::Center => (spare / 2.0, frame.gap),
                Align::End => (spare, frame.gap),
                Align::Spread if count > 1.0 => (0.0, frame.gap + spare / (count - 1.0)),
                _ => (0.0, frame.gap),
            };
            *line = if axis == 0 { [frame.pad_before(0) + first, between] } else { [top, size] };
            (top, widest) = (top + size + down, widest.max(size));
        }
        let mut child = row + 1;
        while child < end {
            // As in a grid: on past this box's subtree, and past any layer started here.
            let (cell, place) = (&self.specs[child as usize].frame, &mut self.places[child as usize]);
            child = self.specs[child as usize].end;
            let Some(line) = tracks.get_mut(place.on_line as usize).filter(|_| cell.parent == row) else { continue };
            if axis == 0 {
                (place.pos[0], line[0]) = (line[0] + cell.nudge[0], line[0] + place.dim[0] + line[1]);
                continue;
            }
            let fills = matches!(cell.size[1], Length::Grow(_)) || frame.across == Align::Spread && cell.size[1] == Length::Fit;
            place.dim[1] = if fills { line[1] } else { place.fit[1] }.max(cell.min[1]).min(cell.max[1]);
            let spare = match frame.across {
                Align::Center => (line[1] - place.dim[1]) / 2.0,
                Align::End => line[1] - place.dim[1],
                _ => 0.0,
            };
            place.pos[1] = line[0] + spare + cell.nudge[1];
        }
        self.places[row as usize].taken_up[axis] = if axis == 0 { widest } else { (top - down - frame.pad_before(1)).max(0.0) };
        self.tracks = tracks;
    }

    /// For a wrapping row with `room` across: gives each box its width and line. Leaves in
    /// `tracks`, per line, the width its boxes take with gaps, and its box count.
    fn break_lines(&mut self, row: u32, room: f32) {
        let (mut tracks, frame, end) = (std::mem::take(&mut self.tracks), self.specs[row as usize].frame, self.specs[row as usize].end);
        tracks.clear();
        tracks.push([0.0; 2]);
        let mut child = row + 1;
        while child < end {
            let (cell, place) = (&self.specs[child as usize].frame, &mut self.places[child as usize]);
            child = self.specs[child as usize].end;
            if cell.parent != row {
                continue;
            }
            let wide = match cell.size[0] {
                Length::Grow(_) => room,
                Length::Px(size) => size,
                Length::Fit => place.fit[0].min(room),
            };
            place.dim[0] = wide.max(cell.min[0]).min(cell.max[0]);
            // To the next line, unless it is the first on this one: that stays, however wide.
            let [used, count] = tracks[tracks.len() - 1];
            if count > 0.0 && used + frame.gap + place.dim[0] > room + 0.01 {
                tracks.push([0.0; 2]);
            }
            let line = tracks.len() - 1;
            place.on_line = line as u32;
            tracks[line] = [tracks[line][0] + place.dim[0] + if tracks[line][1] > 0.0 { frame.gap } else { 0.0 }, tracks[line][1] + 1.0];
        }
        self.tracks = tracks;
    }

    /// Wraps every wrapping text into lines that fit the width it got.
    fn wrap(&mut self, text: &mut dyn TextSystem, scale: f32) {
        self.lines.clear();
        for at in 0..self.wrapping.len() {
            let i = self.wrapping[at] as usize;
            let Content::Text { start, end, font, .. } = self.specs[i].content else { continue };
            let (first, piece) = (self.lines.len(), &self.strings[start as usize..end as usize]);
            let width = self.places[i].dim[0] - self.specs[i].frame.pad_along(0);
            // Its single-line width was measured on the way up, and is cached.
            let long = self.sizes.get(&hash((piece, font.face, font.size.to_bits(), scale.to_bits()))).map_or(0.0, |size| size[0]);
            if long > width || piece.contains('\n') {
                self.stops.clear();
                text.measure(piece, font, scale, Some(&mut self.stops));
                wrap(piece, &self.stops, width.max(1.0), &mut self.lines);
            } else {
                self.lines.push(Line { start: 0, end: piece.len(), width: long });
            }
            // Lines are in bytes of the whole run's text, not of this piece.
            for line in &mut self.lines[first..] {
                (line.start, line.end) = (line.start + start as usize, line.end + start as usize);
            }
            self.places[i].lines = (first as u32, self.lines.len() as u32);
        }
    }

    /// Bottom-up: the bounds of each box and its content. (A clipping box reaches no further
    /// than itself.)
    fn reach(&mut self) {
        for i in (0..self.specs.len()).rev() {
            let (frame, place, own) = (&self.specs[i].frame, self.places[i], self.drawn(i as u32));
            let reach = if frame.clip { own } else { own.union(place.reach) };
            self.places[i].reach = reach;
            if let Some(up) = self.places.get_mut(frame.parent as usize) {
                up.reach = up.reach.union(reach.moved(place.pos[0], place.pos[1]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::BuiltInFont;

    /// A box in box `parent`, as `frame` says.
    fn add(tree: &mut Tree, parent: u32, frame: Frame) -> u32 {
        tree.specs.push(Spec { frame: Frame { parent, ..frame }, ..Default::default() });
        tree.specs.len() as u32 - 1
    }
    fn sized(w: Length, h: Length) -> Frame {
        Frame { size: [w, h], ..Default::default() }
    }
    fn label(tree: &mut Tree, parent: u32, text: &str, wrap: bool) -> u32 {
        let start = tree.strings.len() as u32;
        tree.strings.push_str(text);
        let at = add(tree, parent, Frame::default());
        tree.specs[at as usize].content = Content::Text { start, end: tree.strings.len() as u32, font: Font::new(8.0), color: Color::WHITE, wrap, side: Align::Start };
        tree.wrapping.extend(wrap.then_some(at));
        at
    }
    /// A 200 by 100 row as root, filled by `build`, laid out. Returns every box's rect but the
    /// root's.
    fn laid_out(root: Frame, build: impl FnOnce(&mut Tree)) -> (Tree, Vec<Rect>) {
        let mut tree = Tree::default();
        add(&mut tree, NONE, Frame { flow: Flow::Row, ..root });
        build(&mut tree);
        // Every box ends where its last descendant ends.
        for i in (0..tree.specs.len()).rev() {
            let (end, parent) = (tree.specs[i].end.max(i as u32 + 1), tree.specs[i].frame.parent);
            tree.specs[i].end = end;
            if let Some(up) = tree.specs.get_mut(parent as usize) {
                up.end = up.end.max(end);
            }
        }
        tree.layout([0].into_iter(), [200.0, 100.0], &mut BuiltInFont, 1.0);
        let rects = (1..tree.specs.len() as u32).map(|at| tree.rect_of(at)).collect();
        (tree, rects)
    }

    #[test]
    fn fixed_boxes_follow_each_other_and_the_rest_is_shared() {
        let (_, rects) = laid_out(Frame { gap: 10.0, ..Default::default() }, |t| {
            add(t, 0, sized(Length::Px(30.0), Length::Px(20.0)));
            add(t, 0, sized(Length::Grow(1.0), Length::Grow(1.0)));
            add(t, 0, sized(Length::Grow(3.0), Length::Fit));
        });
        assert_eq!(rects, [rect(0.0, 0.0, 30.0, 20.0), rect(40.0, 0.0, 37.5, 100.0), rect(87.5, 0.0, 112.5, 0.0)]);
    }

    #[test]
    fn boxes_that_give_way_get_smaller_when_there_is_too_little_room_the_readiest_first() {
        // In a 200-wide row: a 20-letter name (120 wide), a 30-letter path (180), and a 30-wide
        // number that never shrinks. `gives` sets how readily name and path shrink; the name
        // never gets narrower than 24.
        let row = |gives: [u8; 2], number: f32| {
            let (_, rects) = laid_out(Frame::default(), |t| {
                let name = label(t, 0, &"n".repeat(20), false) as usize;
                (t.specs[name].frame.shrink, t.specs[name].frame.min[0]) = (gives[0], 24.0);
                let path = label(t, 0, &"p".repeat(30), false) as usize;
                t.specs[path].frame.shrink = gives[1];
                add(t, 0, sized(Length::Px(number), Length::Px(10.0)));
            });
            [rects[0].w, rects[1].w, rects[2].x]
        };
        // Nothing shrinks: the row is 330 wide, and the number lies past 200.
        assert_eq!(row([0, 0], 30.0), [120.0, 180.0, 300.0]);
        // The path shrinks before the name: it absorbs all 130 of overflow.
        assert_eq!(row([1, 2], 30.0), [120.0, 50.0, 170.0]);
        // With a 100-wide number, 200 overflow: the path gives all it has, then the name the
        // remaining 20.
        assert_eq!(row([1, 2], 100.0), [100.0, 0.0, 100.0]);
        // Equally ready, they share by how much each can give: 96 and 180 of 130.
        let even = row([2, 2], 30.0);
        assert!((even[0] - (120.0 - 130.0 * 96.0 / 276.0)).abs() < 0.01 && (even[0] + even[1] - 170.0).abs() < 0.01, "{even:?}");
        // What can't be absorbed stays overflow: the name keeps its 24.
        assert_eq!(row([1, 2], 250.0), [24.0, 0.0, 24.0]);
        // With room to spare, nothing shrinks.
        let (_, rects) = laid_out(Frame::default(), |t| {
            let name = label(t, 0, "name", false) as usize;
            t.specs[name].frame.shrink = 1;
            add(t, 0, sized(Length::Grow(1.0), Length::Px(10.0)));
        });
        assert_eq!((rects[0].w, rects[1].w), (24.0, 176.0));
    }

    /// Every box is copied as it is described, 100,000 times a run in a large window: its
    /// record size matters to the byte. 8 more bytes (a two-axis alignment as two bytes) made
    /// describing a window a sixth slower.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_box_is_no_bigger_than_it_has_to_be() {
        assert_eq!((size_of::<Frame>(), size_of::<Spec>(), size_of::<Option<Placement>>()), (84, 160, 1));
        // A box's own alignment, each axis, round-trips through its one byte.
        for (across, down) in [(Align::Start, Align::Start), (Align::End, Align::Center), (Align::Center, Align::Spread), (Align::Spread, Align::End)] {
            let spot = Placement::new(across, down);
            assert_eq!((spot.horizontal(), spot.vertical(), Placement::from((across, down)), Placement::from(across)), (across, down, spot, Placement::new(across, across)));
        }
    }

    #[test]
    fn a_box_that_only_narrows_keeps_its_height_in_a_column_that_is_too_full() {
        // A 100-high column with a 60-high shrinking box and a 70-high fixed one: 30 overflow.
        // `gives` sets how the first shrinks.
        let column = |gives: u8| {
            let (_, rects) = laid_out(Frame::default(), |t| {
                let column = add(t, 0, Frame { flow: Flow::Column, ..sized(Length::Px(50.0), Length::Px(100.0)) });
                add(t, column, Frame { shrink: gives, ..sized(Length::Px(50.0), Length::Px(60.0)) });
                add(t, column, sized(Length::Px(50.0), Length::Px(70.0)));
            });
            [rects[1].h, rects[2].y]
        };
        // Along its parent's flow, whichever way: it shrinks its height.
        assert_eq!(column(1), [30.0, 30.0]);
        // One that only narrows keeps it, and the overflow runs out of the column.
        assert_eq!(column(1 | SHRINK_WIDTH), [60.0, 60.0]);
        // In a row it narrows like any shrinking box.
        let (_, rects) = laid_out(Frame::default(), |t| {
            add(t, 0, Frame { shrink: 1 | SHRINK_WIDTH, ..sized(Length::Px(150.0), Length::Px(10.0)) });
            add(t, 0, sized(Length::Px(120.0), Length::Px(10.0)));
        });
        assert_eq!((rects[0].w, rects[1].x), (80.0, 80.0));
    }

    #[test]
    fn a_box_is_as_big_as_what_is_in_it() {
        let (_, rects) = laid_out(Frame::default(), |t| {
            let card = add(t, 0, Frame { pad: Insets::all(4.0), gap: 2.0, ..Default::default() });
            label(t, card, "Submit", false);
            label(t, card, "all", false);
        });
        // "Submit" is 6 letters 6 wide, lines 10 high: 36 by 10, twice, 2 apart, padding 4.
        assert_eq!((rects[0], rects[2]), (rect(0.0, 0.0, 44.0, 30.0), rect(4.0, 16.0, 18.0, 10.0)));
    }

    #[test]
    fn spare_room_can_go_before_between_or_around() {
        let with = |along: Align, across: Align| {
            let (_, rects) = laid_out(Frame { along, across, ..Default::default() }, |t| {
                add(t, 0, sized(Length::Px(50.0), Length::Px(40.0)));
                add(t, 0, sized(Length::Px(50.0), Length::Fit));
            });
            (rects[0], rects[1])
        };
        assert_eq!(with(Align::End, Align::Center), (rect(100.0, 30.0, 50.0, 40.0), rect(150.0, 50.0, 50.0, 0.0)));
        assert_eq!(with(Align::Center, Align::End), (rect(50.0, 60.0, 50.0, 40.0), rect(100.0, 100.0, 50.0, 0.0)));
        assert_eq!(with(Align::Spread, Align::Spread), (rect(0.0, 0.0, 50.0, 40.0), rect(150.0, 0.0, 50.0, 100.0)));
    }

    #[test]
    fn text_wraps_to_the_width_it_gets_and_grows_its_box() {
        let (tree, rects) = laid_out(Frame::default(), |t| {
            let column = add(t, 0, sized(Length::Px(80.0), Length::Fit));
            label(t, column, "fix the jump arc on slopes", true);
        });
        let lines: Vec<&str> = tree.lines.iter().map(|l| &tree.strings[l.start..l.end]).collect();
        assert_eq!(lines, ["fix the jump", "arc on slopes"]);
        assert_eq!(rects, [rect(0.0, 0.0, 80.0, 20.0), rect(0.0, 0.0, 80.0, 20.0)], "the column is as tall as its two lines");
        // So does text whose box has a max width, given all the room there is: as wide as
        // allowed, as high as its lines need.
        let (tree, rects) = laid_out(Frame::default(), |t| {
            let words = label(t, 0, "fix the jump arc on slopes", true) as usize;
            t.specs[words].frame.max[0] = 80.0;
        });
        assert_eq!((tree.lines.len(), rects[0]), (2, rect(0.0, 0.0, 80.0, 20.0)));
    }

    #[test]
    fn text_draws_a_little_past_its_box_sideways_unless_the_box_leaves_room_or_cuts_it() {
        // A bare label reaches 2 units past each side of its box (strokes may overhang); one
        // with 11 units of padding doesn't, nor does a clipping one, nor a box without text.
        let (tree, _) = laid_out(Frame::default(), |t| {
            label(t, 0, "Table", false);
            let padded = label(t, 0, "Table", false) as usize;
            t.specs[padded].frame.pad = Insets::xy(11.0, 0.0);
            let cut = label(t, 0, "Table", false) as usize;
            t.specs[cut].frame.clip = true;
            add(t, 0, sized(Length::Px(30.0), Length::Px(10.0)));
        });
        let drawn: Vec<Rect> = (1..5).map(|at| tree.drawn(at)).collect();
        assert_eq!(drawn, [rect(-2.0, 0.0, 34.0, 10.0), rect(0.0, 0.0, 52.0, 10.0), rect(0.0, 0.0, 30.0, 10.0), rect(0.0, 0.0, 30.0, 10.0)]);
    }

    #[test]
    fn a_grid_lines_boxes_up_in_columns_and_rows() {
        let (_, rects) = laid_out(Frame::default(), |t| {
            let grid = add(t, 0, Frame { flow: Flow::Grid(2), gap: 4.0, size: [Length::Grow(1.0), Length::Fit], across: Align::Center, ..Default::default() });
            label(t, grid, "Name", false);
            add(t, grid, sized(Length::Grow(1.0), Length::Px(20.0)));
            label(t, grid, "Message", false);
            add(t, grid, sized(Length::Px(50.0), Length::Px(10.0)));
        });
        // The first column is as wide as "Message" (7 letters of 6); the second has a growing
        // box, so it takes the rest. The first row is as high as its tallest box, and the label
        // beside it is centred in it.
        assert_eq!(rects, [rect(0.0, 0.0, 200.0, 34.0), rect(0.0, 5.0, 42.0, 10.0), rect(46.0, 0.0, 154.0, 20.0), rect(0.0, 24.0, 42.0, 10.0), rect(46.0, 24.0, 50.0, 10.0)]);
    }

    #[test]
    fn a_row_that_wraps_and_a_grid_can_have_other_room_between_lines_than_between_boxes() {
        // Five 60 by 10 boxes, 4 apart, in a 200-wide wrapping row: three per line. And four in
        // a two-column grid. Each with a line gap of 12, and without.
        let lines = |line_gap: Option<f32>, flow: Flow| {
            let (_, rects) = laid_out(Frame::default(), |t| {
                let holder = add(t, 0, Frame { flow, gap: 4.0, ..sized(Length::Grow(1.0), Length::Fit) });
                t.line_gaps.extend(line_gap.map(|gap| (holder, gap)));
                for _ in 0..5 - (flow != Flow::Wrap) as usize {
                    add(t, holder, sized(Length::Px(60.0), Length::Px(10.0)));
                }
            });
            rects
        };
        // The first rect is the row's or grid's; then its boxes'.
        let wrap = lines(Some(12.0), Flow::Wrap);
        assert_eq!((wrap[0].h, wrap[3].x, wrap[4], wrap[5]), (32.0, 128.0, rect(0.0, 22.0, 60.0, 10.0), rect(64.0, 22.0, 60.0, 10.0)), "4 across, 12 down");
        assert_eq!((lines(None, Flow::Wrap)[0].h, lines(None, Flow::Wrap)[4].y), (24.0, 14.0), "4 both ways");
        let grid = lines(Some(12.0), Flow::Grid(2));
        assert_eq!((grid[0].h, grid[2].x, grid[3].y), (32.0, 64.0, 22.0), "between the columns 4, between the rows 12");
        assert_eq!(lines(None, Flow::Grid(2))[3].y, 14.0);
    }

    #[test]
    fn the_room_between_lines_said_anew_is_laid_out_anew() {
        use crate::testing::Trial;
        use crate::{Role, Ui};
        // Tags in a wrapping row whose line gap the app changes between runs: nothing else
        // changes, and it is laid out again all the same.
        fn tags(down: &mut f32, ui: &mut Ui) {
            ui.column().grow().pad(10.0).open();
            *down += 10.0 * ui.button("Further").clicked as u32 as f32;
            ui.wrapping_row().w(Length::Px(200.0)).gap(4.0).line_gap(*down).open();
            let t = ui.theme;
            for tag in ["one", "two", "three", "four", "five", "six", "seven"] {
                ui.el().pad(4.0).fill(t.raised).text(tag, t.font, t.text).show();
            }
            ui.close();
            ui.close();
        }
        let mut trial = Trial::new(4.0, tags);
        let first = |trial: &Trial<f32>| (trial.find(Role::Text, "one").rect.y, trial.find(Role::Text, "seven").rect.y);
        let (one, low) = first(&trial);
        trial.press(Role::Button, "Further");
        // Each tag is a 20-high line plus 4 padding: so many lines lie between.
        let between = (low - one) / (28.0 + 4.0);
        assert!(between >= 1.0 && first(&trial) == (one, low + 10.0 * between), "{:?} for {between} lines", first(&trial));
    }

    #[test]
    fn a_scrolling_box_lets_its_children_be_taller_than_it_is() {
        let (tree, rects) = laid_out(Frame::default(), |t| {
            let list = add(t, 0, Frame { scroll: [false, true], ..sized(Length::Grow(1.0), Length::Grow(1.0)) });
            t.specs[list as usize].offset = [0.0, 25.0];
            for _ in 0..5 {
                add(t, list, sized(Length::Grow(1.0), Length::Px(40.0)));
            }
        });
        assert_eq!((tree.extent(1, 0), tree.extent(1, 1)), (200.0, 200.0));
        assert_eq!((rects[1], rects[5]), (rect(0.0, -25.0, 200.0, 40.0), rect(0.0, 135.0, 200.0, 40.0)));
    }
}
