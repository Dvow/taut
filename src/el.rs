//! Box builder. [`ui.el()`](Ui::el) starts a box, each method sets one property, and `open()`
//! or `show()` adds it to the window:
//!
//! ```ignore
//! ui.el().size(16.0, 16.0).radius(4.0).fill(t.accent).show();
//! ```
//!
//! A box is built in place and pushed in one piece: each property costs a few stores, and
//! nothing is looked up.
use crate::color::Color;
use crate::geom::Insets;
use crate::id::{Id, fold, hash, mix};
use crate::layout::{Align, Content, Flow, Frame, Length, NONE, Part, Placement, Spec};
use crate::path::Path;
use crate::pen::{self, Inked, Paint, Pen};
use crate::scene::{Camera, MeshInstance};
use crate::text::Font;
use crate::ui::{Icon, Open, Picture, Response, Ui};
use std::hash::Hash;

/// A box being built. Each method sets one property.
#[must_use = "a box is only in the window once it is opened or shown"]
pub struct El<'a> {
    ui: &'a mut Ui,
    /// The properties set so far.
    spec: Spec,
    /// A hash of everything layout reads, updated as properties are set. (With constant
    /// arguments the compiler computes it ahead, so detecting layout changes costs nothing.)
    sum: u64,
    key: Option<u64>,
}

/// The run hash's marker for a closed box, and for the line gap of the next box, beside it.
const CLOSED: u64 = 0x636c_6f73_6564;
const LINE_GAP: u64 = 0x6c69_6e65_0000_0000;

/// Boxes: adding, opening and closing them; and their ids.
impl Ui {
    /// A new box. Set its properties, then `open` it to add children, or `show` it empty.
    #[inline]
    #[doc(alias("widget", "element", "container"))]
    pub fn el(&mut self) -> El<'_> {
        El { ui: self, spec: Spec::default(), sum: 0, key: None }
    }
    /// A box laying out its children left to right, vertically centred.
    #[inline]
    pub fn row(&mut self) -> El<'_> {
        self.el().flow(Flow::Row).align(Align::Start, Align::Center)
    }
    /// A box laying out its children top to bottom, at its full width.
    #[inline]
    pub fn column(&mut self) -> El<'_> {
        self.el().flow(Flow::Column).align(Align::Start, Align::Spread)
    }
    /// A box laying out its children left to right, wrapping to a new line when out of width:
    /// tags, toolbar buttons, thumbnails. (It wraps at the width it is given: a column's, or in
    /// a row, a set width or `grow`.)
    #[inline]
    pub fn wrapping_row(&mut self) -> El<'_> {
        self.el().flow(Flow::Wrap).align(Align::Start, Align::Center)
    }
    /// A box laying out its children in this many columns, row by row: a form, a table.
    #[inline]
    pub fn grid(&mut self, columns: u16) -> El<'_> {
        self.el().flow(Flow::Grid(columns))
    }
    /// Empty space taking whatever is left: pushes what follows to the far end.
    #[inline]
    pub fn spacer(&mut self) {
        self.el().grow().show();
    }

    /// Closes the last opened box: what is described next goes after it, not in it.
    ///
    /// # Panics
    ///
    /// With debug assertions, if no box is open (one more close than opens). The panic names
    /// the app's line. In release builds the extra close does nothing.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn close(&mut self) {
        // With only a second window itself left open, this ends that window.
        if self.open.len() <= 1 {
            debug_assert!(self.first.is_some(), "ui.close() at {} closes no box: every box this run opened was closed already", std::panic::Location::caller());
            return self.end_window();
        }
        while self.open.len() > 1 {
            let Some(closed) = self.open.pop() else { return };
            self.tree.specs[closed.at as usize].end = self.tree.specs.len() as u32;
            self.layout_key = fold(self.layout_key, CLOSED);
            if !closed.with_parent {
                return;
            }
        }
    }

    /// Appends a piece of text to this run's text; returns its range there.
    fn keep(&mut self, text: &str) -> [u32; 2] {
        let (start, end) = (self.text_said, self.text_said + text.len());
        // While this run repeats the last one's text in the same places, it is there already:
        // nothing is written, and its width is known. From the first piece that differs, the
        // rest is written again.
        let same = self.same_text && self.tree.strings.as_bytes().get(start..end) == Some(text.as_bytes());
        if !same {
            self.tree.strings.truncate(start);
            self.tree.strings.push_str(text);
        }
        (self.same_text, self.text_said) = (same, end);
        // Piece boundaries matter to layout too: "1" and "23" are other boxes than "12" and
        // "3".
        self.layout_key = fold(self.layout_key, text.len() as u64);
        [start as u32, end as u32]
    }

    /// The id the next box here will get, to ask about it before it is made.
    pub fn next_id(&self) -> Id {
        let (id, around) = (self.id_for(None), self.open.last());
        self.ahead.set(around.map_or((NONE, 0, 0), |around| (around.at, around.children, id)));
        id
    }
    /// The id a box with this key will get here.
    pub fn id(&self, key: impl Hash) -> Id {
        self.id_for(Some(hash(key)))
    }
    fn id_for(&self, key: Option<u64>) -> Id {
        let Some(around) = self.open.last() else { return 0 };
        // A box without a key is identified by its index among its siblings.
        hash((self.tree.specs[around.at as usize].id, key.unwrap_or(around.children as u64 | 1 << 63)))
    }

    /// Adds a built box to the open box; `stays_open` leaves it open for children. `sum` is the
    /// hash of what layout reads of it.
    fn add(&mut self, mut spec: Spec, mut sum: u64, key: Option<u64>, stays_open: bool) -> Response {
        let at = self.tree.specs.len() as u32;
        let Some(around) = self.open.last_mut() else { return Response::default() };
        let (parent, layer, seat, columns) = (around.at, around.layer, around.children, around.columns);
        let (row, cell) = (around.row, Some(columns[0].saturating_add(around.children)).filter(|column| around.cells && *column < columns[1]));
        around.children += 1;
        (spec.frame.parent, spec.end) = (parent, at + 1);
        // Rare: the box is fitted to a large list or a table, or it is disabled.
        if row > 0.0 || cell.is_some() || !self.enabled {
            sum = self.fit_in(&mut spec, row, cell);
        }
        // Only a box that something will ask about needs an id: one with children, input,
        // focus, scrolling, or a key.
        let scrolls = spec.frame.scroll != [false; 2];
        if stays_open || key.is_some() || spec.frame.senses || spec.frame.focus || scrolls {
            let (in_box, in_seat, ahead) = self.ahead.get();
            spec.id = if key.is_none() && (in_box, in_seat) == (parent, seat) { ahead } else { hash((self.tree.specs[parent as usize].id, key.unwrap_or(seat as u64 | 1 << 63))) };
        }
        if scrolls {
            spec.offset = self.scrolls.get(&spec.id).copied().unwrap_or_default();
            self.scrollers.push(at);
            sum ^= crate::layout::grow_where_it_scrolls(&mut spec.frame);
        }
        // One that had a scroll bar keeps its gutter free: see `Ui::update_gutter`.
        if scrolls && self.gutters.get(&spec.id).is_some_and(|gutter| gutter.reserved) {
            let was = spec.frame.part(Part::Scroll);
            spec.frame.reserve_gutter = true;
            sum ^= mix(Part::Scroll as u64, was) ^ mix(Part::Scroll as u64, spec.frame.part(Part::Scroll));
        }
        if scrolls || spec.offset != [0.0; 2] {
            self.offsets_key = fold(self.offsets_key, (spec.offset[0].to_bits() as u64) << 32 | spec.offset[1].to_bits() as u64);
        }
        if spec.content.shape().is_some_and(|(_, wrap)| wrap) {
            self.tree.wrapping.push(at);
        }
        // What layout reads of this box joins the run's hash compared with the last run's; and
        // with it, whether what follows goes inside.
        debug_assert_eq!(sum, spec.sum(), "the number kept for a box while it was described does not stand for the box");
        self.layout_key = fold(self.layout_key, sum ^ stays_open as u64);
        let (id, senses, focus, edited) = (spec.id, spec.frame.senses, spec.frame.focus, spec.select.is_some());
        self.tree.specs.push(spec);
        if focus {
            self.reachable.push((layer, id, at));
        }
        if edited {
            self.typed_in = Some(at);
        }
        if stays_open {
            let (with_parent, cells) = (self.join_next, self.cells_next);
            #[cfg(debug_assertions)]
            let opened = std::panic::Location::caller();
            self.open.push(Open {
                at,
                children: 0,
                with_parent,
                layer,
                columns,
                cells,
                row: 0.0,
                count: 0,
                #[cfg(debug_assertions)]
                opened,
            });
        }
        (self.join_next, self.cells_next) = (false, false);
        // A part that asked about this box just before describing it already has the answer.
        match senses {
            true if self.asked.id == id => self.asked,
            true => self.interact(id),
            false => Response { id, ..Default::default() },
        }
    }

    /// For a box that is not exactly as described: in a large list, a row as high as the rows;
    /// in a table row, a cell as wide as its column; disabled, paler and skipped by focus.
    /// Returns the new hash of what layout reads. (`spec` is never lent from here to a function
    /// pointer such as a table's: it could keep the reference, and `add` would then copy every
    /// box once more before pushing it, in case it changed. That copy was an eighth of the time
    /// of describing a window.)
    #[cold]
    fn fit_in(&mut self, spec: &mut Spec, row: f32, cell: Option<u32>) -> u64 {
        if row > 0.0 {
            spec.frame.size[1] = Length::Px(row);
        }
        // Screen readers get the row's index and count: only rows in view are described, and so
        // reported.
        if let Some(around) = self.open.last().filter(|_| row > 0.0 && self.listening) {
            let place = [around.children - 1, around.count];
            self.access_rows.push((self.tree.specs.len() as u32, place));
        }
        // Cells only exist in a table, which has set what it does with them. It gets a copy of
        // the box, for the reason above.
        if let (Some(column), Some(hooks)) = (cell, self.grids.hooks) {
            let mut seated = *spec;
            (hooks.seat)(self, &mut seated, column);
            *spec = seated;
        }
        if !self.enabled {
            (spec.frame.focus, spec.fill, spec.border) = (false, spec.fill.with_alpha(0.5), spec.border.with_alpha(0.5));
        }
        if let (false, Content::Text { color, .. } | Content::Icon { color, .. } | Content::Drawing { color, .. }) = (self.enabled, &mut spec.content) {
            *color = self.theme.text_faint;
        }
        if let (false, Content::Shape { shape }) = (self.enabled, spec.content) {
            self.shapes[shape as usize].fade(self.theme.text_faint);
        }
        if let (false, Content::Stripes { color }) = (self.enabled, &mut spec.content) {
            *color = color.with_alpha(0.5);
        }
        if let (false, Content::Gradient { stops: [from, to], .. }) = (self.enabled, spec.content) {
            self.blends[from as usize..to as usize].iter_mut().for_each(|stop| stop.1 = stop.1.with_alpha(0.5));
        }
        spec.sum()
    }
}

impl std::fmt::Debug for El<'_> {
    /// Its flow and size: what tells boxes apart while debugging.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("El").field("flow", &self.spec.frame.flow).field("size", &self.spec.frame.size).finish_non_exhaustive()
    }
}

impl El<'_> {
    /// Changes one part of what layout reads, keeping the hash in step: the old part is taken
    /// out, the new one put in.
    #[inline(always)]
    fn frame(mut self, part: Part, change: impl FnOnce(&mut Frame)) -> Self {
        let was = self.spec.frame.part(part);
        change(&mut self.spec.frame);
        self.sum ^= mix(part as u64, was) ^ mix(part as u64, self.spec.frame.part(part));
        self
    }
    /// Sets the content, replacing any.
    #[inline(always)]
    fn holds(mut self, content: Content) -> Self {
        self.sum ^= self.spec.content.shape_key() ^ content.shape_key();
        self.spec.content = content;
        self
    }

    /// Adds the box and leaves it open: what is described next goes into it, until
    /// [`close()`](Ui::close).
    ///
    /// With debug assertions, the call site is kept: a box never closed is reported with the
    /// app's line when the run ends (see [`Ui::run`]).
    #[inline]
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn open(self) -> Response {
        let ui = self.ui;
        let opened = ui.add(self.spec, self.sum, self.key, true);
        // The app's call site: `add` itself only knows its own caller.
        #[cfg(debug_assertions)]
        if let Some(open) = ui.open.last_mut() {
            open.opened = std::panic::Location::caller();
        }
        opened
    }
    /// Adds the box, empty.
    #[inline]
    pub fn show(self) -> Response {
        self.ui.add(self.spec, self.sum, self.key, false)
    }

    /// Identifies this box among its siblings in any order: for boxes that keep state (a scroll
    /// position) in lists that change.
    #[inline]
    pub fn key(mut self, key: impl Hash) -> Self {
        self.key = Some(hash(key));
        self
    }
    /// How it lays out its children: row, column, grid, stack.
    #[inline]
    pub fn flow(self, flow: Flow) -> Self {
        self.frame(Part::Flow, |frame| frame.flow = flow)
    }
    /// Width: fit to content ([`Length::Fit`]), fixed units ([`Length::Px`]), or a share of the
    /// room left by the others ([`Length::Grow`]).
    #[inline]
    pub fn w(self, width: Length) -> Self {
        self.frame(Part::Width, |frame| frame.size[0] = width)
    }
    /// Height, the same way.
    #[inline]
    pub fn h(self, height: Length) -> Self {
        self.frame(Part::Height, |frame| frame.size[1] = height)
    }
    /// Fixed width and height.
    #[inline]
    pub fn size(self, width: f32, height: f32) -> Self {
        self.w(Length::Px(width)).h(Length::Px(height))
    }
    /// Takes all the room its parent has left, both ways.
    #[inline]
    pub fn grow(self) -> Self {
        self.w(Length::Grow(1.0)).h(Length::Grow(1.0))
    }
    /// Minimum width and height, in units.
    #[inline]
    pub fn min(self, width: f32, height: f32) -> Self {
        self.frame(Part::Least, |frame| frame.min = [width, height])
    }
    /// Maximum width and height, in units.
    #[inline]
    pub fn max(self, width: f32, height: f32) -> Self {
        self.frame(Part::Most, |frame| frame.max = [width, height])
    }
    /// Padding between its edge and its content.
    #[inline]
    #[doc(alias("padding"))]
    pub fn pad(self, pad: impl Into<Insets>) -> Self {
        let pad = pad.into();
        self.frame(Part::PadBefore, |frame| (frame.pad.left, frame.pad.top) = (pad.left, pad.top)).frame(Part::PadAfter, |frame| (frame.pad.right, frame.pad.bottom) = (pad.right, pad.bottom))
    }
    /// Gap between its children.
    #[inline]
    #[doc(alias("spacing"))]
    pub fn gap(self, gap: f32) -> Self {
        self.frame(Part::Gap, |frame| frame.gap = gap)
    }
    /// Gap between lines of a wrapping row, or rows of a grid, if not `gap` (as CSS `row-gap`):
    /// `.gap(6.0).line_gap(12.0)`. Ignored in other flows.
    pub fn line_gap(self, gap: f32) -> Self {
        // Kept beside the box, not in it (see `Tree::line_gaps`); the box goes at the end of
        // the list. Layout reads it, so it is in the layout hash.
        let at = self.ui.tree.specs.len() as u32;
        self.ui.tree.line_gaps.push((at, gap));
        self.ui.layout_key = fold(self.ui.layout_key, LINE_GAP ^ gap.to_bits() as u64);
        self
    }
    /// Where its children sit: along the flow, and across it. (In a scroll box, along it while
    /// they take less room than it has: a chat's few messages at its bottom, with
    /// [`Align::End`].)
    #[inline]
    pub fn align(self, along: Align, across: Align) -> Self {
        self.frame(Part::Align, |frame| (frame.along, frame.across) = (along, across))
    }
    /// Where this box sits, overriding its parent's alignment: across the parent's flow, and in
    /// a stack, both ways. One value sets both ([`Align::End`]: the far corner); a pair sets
    /// each, so a button strip at a row's end is `.place((Align::End, Align::Center))`: right,
    /// and vertically centred, whatever either's height.
    #[inline]
    pub fn place(self, place: impl Into<Placement>) -> Self {
        let place = place.into();
        self.frame(Part::Place, |frame| frame.place = Some(place))
    }
    /// Offsets it from where layout puts it, without moving what follows. In a stack, this puts
    /// it anywhere.
    #[inline]
    pub fn offset(self, x: f32, y: f32) -> Self {
        self.frame(Part::Nudge, |frame| frame.nudge = [x, y])
    }
    /// Fill colour.
    #[inline]
    pub fn fill(mut self, color: Color) -> Self {
        self.spec.fill = color;
        self
    }
    /// A vertical gradient fill, top to bottom. (Replaces other content: put children inside.)
    #[inline]
    pub fn gradient(self, top: Color, bottom: Color) -> Self {
        self.gradient_along(180.0, &[(0.0, top), (1.0, bottom)])
    }
    /// A horizontal gradient fill, left to right.
    #[inline]
    pub fn gradient_across(self, left: Color, right: Color) -> Self {
        self.gradient_along(90.0, &[(0.0, left), (1.0, right)])
    }
    /// A linear gradient of any number of stops, along a line through the centre at `degrees`
    /// from straight up: 90 is left to right, 180 top to bottom, 45 bottom left to top right.
    /// Each stop is a position from 0 to 1 and a colour, in order.
    pub fn gradient_along(self, degrees: f32, colors: &[(f32, Color)]) -> Self {
        self.ui.shows.gradient = Some(|canvas, r, radius, colors, turn| canvas.gradient(r, radius, colors, turn));
        let from = self.ui.blends.len() as u32;
        self.ui.blends.extend_from_slice(colors);
        self.holds(Content::Gradient { stops: [from, from + colors.len() as u32], turn: Some(degrees) })
    }
    /// Fills every other line of its content with `color`, from the second: zebra stripes for a
    /// grid's rows, or a column's boxes. A stripe spans the box inside its padding, and halfway
    /// into the gaps either side. (Replaces other content. In other flows it fills nothing.)
    pub fn stripes(self, color: Color) -> Self {
        self.ui.shows.stripes = Some(crate::paint::stripes);
        self.holds(Content::Stripes { color })
    }
    /// A radial gradient from the centre: the first colour in the middle, the last at the edges
    /// (and in the corners beyond).
    pub fn gradient_around(self, colors: &[(f32, Color)]) -> Self {
        self.ui.shows.gradient = Some(|canvas, r, radius, colors, turn| canvas.gradient(r, radius, colors, turn));
        let from = self.ui.blends.len() as u32;
        self.ui.blends.extend_from_slice(colors);
        self.holds(Content::Gradient { stops: [from, from + colors.len() as u32], turn: None })
    }
    /// A border inside its edge: colour and width.
    #[inline]
    pub fn border(mut self, color: Color, width: f32) -> Self {
        (self.spec.border, self.spec.border_width) = (color, width);
        self
    }
    /// Corner radius, in units.
    #[inline]
    pub fn radius(mut self, radius: f32) -> Self {
        self.spec.radius = radius;
        self
    }
    /// A soft shadow of this reach: for what floats over the rest.
    #[inline]
    pub fn shadow(self, reach: f32) -> Self {
        self.ui.shows.shadow = Some(|canvas, r, radius, reach, color| canvas.shadow(r, radius, reach, color));
        self.frame(Part::Shadow, |frame| frame.shadow = reach)
    }
    /// Draws it, with its content, as one layer at this opacity: 1 opaque, 0.5 half transparent
    /// (a part not ready yet, one fading with [`Ui::animate`]). At 0 it is hidden, as `hidden`
    /// does. Layout and input are unchanged.
    pub fn opacity(self, solid: f32) -> Self {
        let solid = if solid.is_nan() { 1.0 } else { solid.clamp(0.0, 1.0) };
        let fade = 255 - (solid * 255.0).round() as u8;
        if fade != 0 && fade != 255 {
            self.ui.shows.fade = Some(|painter, step, canvas| painter.fade(step, canvas));
            self.ui.visible_boxes.fades = Some(crate::visible_boxes::VisibleBoxes::fade);
        }
        self.fade(fade)
    }
    /// Hides it with its content if `hidden`, keeping its room: nothing is drawn, and pointer,
    /// focus and screen readers skip it. (An `if` removes its room too; this keeps the rest in
    /// place.)
    pub fn hidden(self, hidden: bool) -> Self {
        if hidden { self.fade(255) } else { self }
    }
    /// How much of it is faded out when drawn (see `Spec::fade`).
    fn fade(mut self, fade: u8) -> Self {
        self.sum ^= crate::layout::fade_key(self.spec.fade) ^ crate::layout::fade_key(fade);
        self.spec.fade = fade;
        if fade == 255 {
            self.ui.hiding = Some(Ui::keep_from_hidden);
        }
        self
    }
    /// Clips its content to its bounds.
    #[inline]
    pub fn clip(self) -> Self {
        self.frame(Part::Clip, |frame| frame.clip = true)
    }
    /// Shrinks when its row lacks room: down to its `min`, its content clipped (text ends in
    /// "…"). `readiness` is 1, 2 or 3: higher numbers give all they can before lower ones give
    /// any; equal ones share by how much each can give. (In a column, its height shrinks.)
    #[inline]
    pub fn shrink(self, readiness: u8) -> Self {
        self.frame(Part::Shrink, |frame| frame.shrink = readiness.clamp(1, 3)).clip()
    }
    /// Like `shrink`, but in rows only: in a column short of room it keeps its height
    /// (overflow runs out at the bottom). For a part whose text can wrap onto more lines and
    /// must never be squeezed away, such as a check box.
    #[inline]
    pub fn shrink_width(self, readiness: u8) -> Self {
        self.frame(Part::Shrink, |frame| frame.shrink = readiness.clamp(1, 3) | crate::layout::SHRINK_WIDTH).clip()
    }
    /// Content may be taller than the box, and scrolls with the wheel or scroll bar. With no
    /// height (no `h`, no `max` height) and not shrinking, it takes the height it is given, as
    /// `.h(Length::Grow(1.0))` does: at its content's height it would have nothing to scroll.
    #[inline]
    pub fn scroll(self) -> Self {
        self.frame(Part::Scroll, |frame| frame.scroll[1] = true).clip()
    }
    /// Content may be wider than the box, and scrolls sideways. With no width, it takes the
    /// width it is given, as `scroll` does the height.
    #[inline]
    pub fn scroll_x(self) -> Self {
        self.frame(Part::Scroll, |frame| frame.scroll[0] = true).clip()
    }
    /// Stays in view while its parent scrolls vertically: at the top if described before
    /// everything that scrolls in the parent, at the bottom if after (a table's header rows, a
    /// totals row, a list's heading). Content scrolls between; horizontally it scrolls along.
    /// Ignored in a box that does not scroll vertically.
    pub fn frozen(self) -> Self {
        let specs = &self.ui.tree.specs;
        let Some(around) = self.ui.open.last().map(|around| around.at).filter(|at| specs[*at as usize].frame.scroll[1]) else { return self };
        // At the top, while everything before it in that box is kept too. (Checked only here,
        // so unkept boxes cost nothing.)
        let before = std::iter::successors(Some(around + 1), |at| specs.get(*at as usize).map(|spec| spec.end)).take_while(|at| (*at as usize) < specs.len());
        let first = before.filter(|at| specs[*at as usize].frame.parent == around).all(|at| specs[at as usize].frame.pinned != 0);
        self.frame(Part::Pinned, |frame| frame.pinned = if first { 1 } else { 2 })
    }
    /// Sticks to the top of the nearest vertical scroll box while what follows it in its parent
    /// scrolls under it: a list's group heading, as CSS `position: sticky`. It scrolls away
    /// when its parent ends, or the next sticky box there arrives: so give each group its own
    /// box, or put headings in sequence with their rows between. What lies under it is hidden
    /// meanwhile. Without a vertical scroll box around it, it stays where it is.
    ///
    /// ```
    /// # use taut::Ui;
    /// # fn describe(ui: &mut Ui, groups: &[(&str, Vec<String>)]) {
    /// let t = ui.theme;
    /// ui.column().grow().scroll().open();
    /// for (name, rows) in groups {
    ///     ui.column().open();
    ///     ui.row().pad(6.0).fill(t.panel).sticky().open();
    ///     ui.label(name);
    ///     ui.close();
    ///     for row in rows {
    ///         ui.label(row);
    ///     }
    ///     ui.close();
    /// }
    /// ui.close();
    /// # }
    /// ```
    #[inline]
    pub fn sticky(self) -> Self {
        self.ui.tree.sticks = Some(&crate::layout::STICKING);
        self.frame(Part::Pinned, |frame| frame.pinned = crate::layout::STICKY)
    }
    /// Draws its content this far left and up of where layout puts it: for a box following
    /// another's scrolling.
    #[inline]
    pub(crate) fn shifted(mut self, x: f32, y: f32) -> Self {
        self.spec.offset = [x, y];
        self
    }
    /// Takes pointer input: hover and clicks. (Reported in what `open` or `show` returns.)
    #[inline]
    pub fn sense(self) -> Self {
        self.frame(Part::Senses, |frame| frame.senses = true)
    }
    /// Takes focus: a Tab stop, and a click focuses it.
    #[inline]
    pub fn focusable(self) -> Self {
        self.frame(Part::Focus, |frame| frame.focus = true)
    }

    /// Holds text. A single line is vertically centred; wrapping text starts at the top.
    #[inline]
    pub fn text(self, text: &str, font: Font, color: Color) -> Self {
        let [start, end] = self.ui.keep(text);
        self.holds(Content::Text { start, end, font, color, wrap: false, side: Align::Start })
    }
    /// Horizontal text alignment when the box is wider than the text: start, centre or end. Per
    /// line, for wrapping text.
    #[inline]
    pub fn text_align(mut self, to: Align) -> Self {
        if let Content::Text { side, .. } = &mut self.spec.content {
            *side = to;
        }
        self
    }
    /// Wraps its text to the box's width.
    #[inline]
    pub fn wrap(self) -> Self {
        let Content::Text { start, end, font, color, side, .. } = self.spec.content else { return self };
        self.holds(Content::Text { start, end, font, color, wrap: true, side })
    }
    /// Its text is being edited: the selection (the second end is the cursor), in bytes, and
    /// whether the cursor shows now.
    pub(crate) fn select(mut self, range: Option<[u32; 2]>, cursor: bool) -> Self {
        (self.spec.select, self.spec.caret, self.spec.typed) = (range, cursor, true);
        self.frame(Part::Edited, |frame| frame.edited = range.is_some())
    }
    /// Holds an icon, `size` units square.
    #[inline]
    pub fn icon(self, icon: Icon, size: f32, color: Color) -> Self {
        self.ui.shows.icon = Some(crate::paint::draw_mask);
        self.drawn_icon(icon, size, color)
    }
    /// Holds an icon drawn the way the UI was given (`Shows::icon`): for the library's own
    /// title bar, which draws its few lines without parsing SVG paths.
    #[inline]
    pub(crate) fn drawn_icon(self, icon: Icon, size: f32, color: Color) -> Self {
        self.ui.icons.push(icon);
        let icon = self.ui.icons.len() as u32 - 1;
        self.holds(Content::Icon { icon, color }).size(size, size)
    }
    /// Holds the app's own filled shape: a chart's area, a drawing. Filled with a colour or a
    /// gradient ([`Paint::Linear`], [`Paint::Radial`]). The shape is in units from the box's
    /// top left, clipped to the box.
    #[inline]
    pub fn shape<'a>(self, shape: &Path, ink: impl Into<Paint<'a>>) -> Self {
        let ink = ink.into();
        self.inked(shape, None, ink, pen::fully(None, &ink))
    }
    /// Holds the app's own shape, stroked: with a number, that width with round caps and joins;
    /// with a [`Pen`], its caps, joins and dashes. In a colour or a gradient.
    #[inline]
    pub fn outline<'a, 'b>(self, shape: &Path, pen: impl Into<Pen<'a>>, ink: impl Into<Paint<'b>>) -> Self {
        let (pen, ink) = (pen.into(), ink.into());
        self.inked(shape, Some(pen), ink, pen::fully(Some(&pen), &ink))
    }
    /// Holds a shape with its paint, and the code that draws it where the canvas cannot alone.
    fn inked(self, shape: &Path, pen: Option<Pen>, ink: Paint, full: Option<pen::Full>) -> Self {
        let ui = &mut *self.ui;
        ui.shows.shape = Some(Inked::draw);
        let at = ui.shapes_used;
        if at == ui.shapes.len() {
            ui.shapes.push(Inked::default());
        }
        ui.shapes[at].set(shape, pen, ink, full);
        ui.shapes_used += 1;
        self.holds(Content::Shape { shape: at as u32 })
    }
    /// Holds a 3D scene: triangle meshes ([`Ui::add_mesh`]), each placed as `shown` says, seen
    /// from `view`. It fills the box at any size, and redraws only when something changed.
    pub fn scene(self, view: &Camera, shown: &[MeshInstance]) -> Self {
        let scene = self.ui.scenes.note(view, shown);
        self.holds(Content::Scene { scene })
    }
    /// Holds an image, at its pixel size unless a size is set after this.
    pub fn picture(self, picture: Picture) -> Self {
        self.ui.shows.picture = Some(|canvas, pixels, width, r, radius| canvas.rounded_picture(pixels, width, r, radius));
        let size = |stored: &crate::ui::Stored| (stored.width as f32, (stored.pixels.len() / stored.width.max(1)) as f32);
        // A released image takes no room and shows nothing.
        let (wide, high) = self.ui.pictures.get(picture.0).map_or((0.0, 0.0), size);
        self.holds(Content::Picture { picture: picture.0, keep: false }).size(wide, high)
    }
    /// Holds an SVG drawing ([`Ui::add_drawing`]), at its own size unless a size is set after
    /// this, sharp at any size. `color` is `currentColor` (most icon sets' lines); unused by a
    /// drawing with its own colours.
    pub fn drawing(self, drawing: crate::svg::Drawing, color: Color) -> Self {
        // A released drawing takes no room and shows nothing.
        let ([wide, high], color) = self.ui.drawings.get(drawing.0).map_or(([0.0; 2], color), |kept| (kept.size(), kept.takes(color)));
        self.holds(Content::Drawing { drawing: drawing.0, color, keep: false }).size(wide, high)
    }
    /// Holds an animation ([`Ui::add_animation`]), at its pixel size unless a size is set after
    /// this. It shows the current frame and asks for a rerun when the next is due: only this
    /// box is redrawn then. (Paused while the window is minimized.)
    pub fn animation(self, animation: crate::animation::Animation) -> Self {
        let ui = &mut *self.ui;
        ui.shows.picture = Some(|canvas, pixels, width, r, radius| canvas.rounded_picture(pixels, width, r, radius));
        // A released animation takes no room and shows nothing.
        let Some(anim) = ui.animations.get(animation.0) else { return self.holds(Content::Animation { animation: animation.0, frame: 0, keep: false }).size(0.0, 0.0) };
        let ([wide, high], (frame, wait)) = (anim.size(), anim.due(ui.now.saturating_sub(anim.since)));
        if let Some(wait) = wait.filter(|_| !ui.window_minimized()) {
            ui.wake_after(wait);
        }
        self.holds(Content::Animation { animation: animation.0, frame: frame as u32, keep: false }).size(wide as f32, high as f32)
    }
    /// Draws its image, animation or drawing as large as fits, aspect ratio kept, centred,
    /// instead of stretched. The box keeps its given size: `.grow()` after this makes a photo
    /// fill its room as large as it fits whole. (Ignored before the content, or on other
    /// content.)
    #[inline]
    pub fn keep_aspect(mut self) -> Self {
        if let Content::Picture { keep, .. } | Content::Drawing { keep, .. } | Content::Animation { keep, .. } = &mut self.spec.content {
            *keep = true;
        }
        self
    }
}
