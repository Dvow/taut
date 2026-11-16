//! Drawing the boxes, once laid out.
use crate::canvas::{Canvas, coverage};
use crate::color::Color;
use crate::geom::{Point, Rect, pt, rect};
use crate::id::Id;
use crate::layout::{Align, BAR, Content, Flow, Tree};
use crate::path::Path;
use crate::scene::Scenes;
use crate::shelf::Shelf;
use crate::survey::{Fade, Seen};
use crate::text::{Font, Line, Rule, TextSystem};
use crate::theme::Theme;
use crate::ui::{Icon, Stored};
use std::collections::HashMap;

/// An icon rasterized once at one size: each pixel's coverage.
pub(crate) struct Mask {
    pub(crate) width: usize,
    pub(crate) cover: Vec<u8>,
}

/// Fills every other line of box `at`'s content (the second, fourth and so on) in `color`, the
/// box being at `r` (see [`El::stripes`](crate::El::stripes)): a grid's rows of cells, a
/// column's children. A line spans the box less its padding, and half the gap to its
/// neighbours.
pub(crate) fn stripes(tree: &Tree, at: u32, r: Rect, color: Color, canvas: &mut Canvas) {
    let spec = &tree.specs[at as usize];
    let each = match spec.frame.flow {
        Flow::Grid(columns) => columns.max(1) as usize,
        Flow::Column => 1,
        _ => return,
    };
    let (top, inside) = (r.y - spec.offset[1], r.inset(spec.frame.pad));
    // Each line's top and bottom: the highest top and lowest bottom of its boxes.
    let (mut lines, mut child, mut n): (Vec<[f32; 2]>, u32, usize) = (Vec::new(), at + 1, 0);
    while child < spec.end {
        let place = &tree.places[child as usize];
        let [y, bottom] = [top + place.pos[1], top + place.pos[1] + place.dim[1]];
        if n % each == 0 || lines.is_empty() {
            lines.push([y, bottom]);
        } else if let Some(line) = lines.last_mut() {
            *line = [line[0].min(y), line[1].max(bottom)];
        }
        (child, n) = (tree.specs[child as usize].end, n + 1);
    }
    for k in (1..lines.len()).step_by(2) {
        let before = (lines[k][0] - lines[k - 1][1]) / 2.0;
        let after = lines.get(k + 1).map_or(before, |next| (next[0] - lines[k][1]) / 2.0);
        canvas.rect(rect(inside.x, lines[k][0] - before, inside.w, lines[k][1] - lines[k][0] + before + after), color);
    }
}

/// The id of the scroll bar of box `of` along `axis`.
pub(crate) fn bar_id(of: Id, axis: usize) -> Id {
    of ^ [0x5c50_11ba_51de_0a15, 0x5c50_11ba_5c50_11ba][axis]
}

/// Where box `at`'s scroll bar along `axis` lies, the box being at `r`: its track (along the
/// bottom or down the right; where both show, each stops short of the corner), and where its
/// thumb starts and how long it is.
pub(crate) fn bar(tree: &Tree, at: u32, r: Rect, axis: usize) -> Option<(Rect, f32, f32)> {
    if !tree.has_bar(at, axis) {
        return None;
    }
    // In a long list laid out from a row near the visible ones, the rows before count too: as
    // scrolled past, and as more to scroll.
    let (room, end, offset, left_out) = (tree.room(at, axis), tree.scroll_end(at, axis), tree.specs[at as usize].offset[axis], tree.shift(at, axis));
    let short = if tree.has_bar(at, 1 - axis) { BAR } else { 0.0 };
    let strip = [rect(r.x, r.bottom() - BAR, r.w - short, BAR), rect(r.right() - BAR, r.y, BAR, r.h - short)][axis];
    let (from, run) = [(strip.x, strip.w), (strip.y, strip.h)][axis];
    let long = (run * room / (room + end + left_out)).max(24.0).min(run);
    Some((strip, from + (run - long) * (offset + left_out) / (end + left_out), long))
}

/// How content only some programs show is drawn: pictures, SVG drawings, a film's due frame,
/// icons, the app's own shapes, shadows, gradients. Each is set when the app first describes or
/// hands over such a thing ([`El::picture`](crate::El::picture),
/// [`Ui::add_drawing`](crate::Ui::add_drawing), [`Ui::add_film`](crate::Ui::add_film),
/// [`El::icon`](crate::El::icon), [`El::shape`](crate::El::shape),
/// [`El::shadow`](crate::El::shadow), [`El::gradient_along`](crate::El::gradient_along)...),
/// and not before. Nothing else reaches that code, so a program that shows none is built
/// without it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Shows {
    /// Lays a picture's pixels into a box with its corners, as [`Canvas::rounded_picture`]
    /// does: a picture, or a film's frame.
    pub picture: Option<ShowsPicture>,
    pub drawing: Option<fn(&mut crate::svg::Kept, Rect, Color, &mut Canvas)>,
    pub film: Option<ShowsFilm>,
    /// Rasterizes an icon at a size, as `draw_mask` does.
    pub icon: Option<fn(Icon, usize) -> Mask>,
    /// Draws an app's shape with its top left at a point, as `Inked::draw` does.
    pub shape: Option<fn(&crate::pen::Inked, &mut Canvas, Point)>,
    /// Draws a box's shadow, as [`Canvas::shadow`] does.
    pub shadow: Option<ShowsShadow>,
    /// Fills a box with a gradient, as [`Canvas::gradient`] does.
    pub gradient: Option<ShowsGradient>,
    /// Keeps what is under a see-through box and lets it show through what was drawn of the
    /// box, as `Painter::fade` does.
    pub fade: Option<Fades>,
    /// Fills every other line of a box's content, as `stripes` does.
    pub stripes: Option<ShowsStripes>,
}
/// Fills every other line of a box's content: the tree, the box, its place, the colour.
type ShowsStripes = fn(&Tree, u32, Rect, Color, &mut Canvas);
/// Keeps the pixels under a see-through box where it starts, and lets them show through where
/// it ends.
type Fades = fn(&mut Painter<'_>, &Seen, &mut Canvas<'_>);
/// Brings a film to a frame and hands out its pixels.
type ShowsFilm = for<'r> fn(&'r mut crate::film::Reel, usize) -> (&'r [u32], usize);
/// Fills a box with a gradient.
type ShowsGradient = fn(&mut Canvas, Rect, f32, &[(f32, Color)], Option<f32>);
/// Draws a box's shadow: its place, corner radius, reach and colour.
type ShowsShadow = fn(&mut Canvas, Rect, f32, f32, Color);
/// Lays a picture into a box: its pixels, row length, the box, its corner radius.
type ShowsPicture = fn(&mut Canvas, &[u32], usize, Rect, f32);

/// Everything drawing needs, for one paint.
pub(crate) struct Painter<'a> {
    pub tree: &'a Tree,
    pub icons: &'a [Icon],
    pub shapes: &'a [crate::pen::Inked],
    pub pictures: &'a mut Shelf<Stored>,
    pub drawings: &'a mut Shelf<crate::svg::Kept>,
    pub films: &'a mut Shelf<crate::film::Reel>,
    /// How content only some programs show is drawn, once described (see `Shows`).
    pub shows: Shows,
    pub scenes: &'a mut Scenes,
    /// The gradients' colour stops.
    pub blends: &'a [(f32, Color)],
    pub masks: &'a mut HashMap<(usize, [u32; 3]), Mask>,
    pub text: &'a mut dyn TextSystem,
    pub theme: &'a Theme,
    /// The box with focus, when its ring is shown; else 0.
    pub focus: Id,
    /// The scroll bar the pointer is over or holds.
    pub bar: Id,
    /// The canvas clip before each clipping box, innermost last.
    pub clips: &'a mut Vec<[i32; 4]>,
    /// Where a line's characters end: kept so its memory is reused.
    pub stops: &'a mut Vec<f32>,
    /// The text cursor shows at the edge of the next character, not the previous one (see
    /// `Edit::after`).
    pub after: bool,
    /// What spell checking keeps once a box asked for it: the misspelled words' wavy lines and
    /// what draws them (see `spell`).
    pub spelling: &'a crate::spell::Spelling,
    /// What lies under the see-through boxes being drawn: their pixels one box after another,
    /// and which pixels each box has.
    pub under: &'a mut (Vec<u32>, Vec<[i32; 4]>),
}

impl Painter<'_> {
    /// Draws the boxes in view in the survey's order, as far as the canvas covers them. Boxes
    /// out of view are not looked at: a window of 100,000 boxes draws as fast as its visible
    /// screenful.
    pub fn draw(&mut self, seen: &[Seen], canvas: &mut Canvas) {
        self.clips.clear();
        for step in seen {
            match step.leaving {
                false => self.enter(step, canvas),
                true => self.leave(step, canvas),
            }
        }
    }

    /// The box itself, under its content.
    fn enter(&mut self, step: &Seen, canvas: &mut Canvas) {
        let (spec, r) = (&self.tree.specs[step.at as usize], step.rect);
        if matches!(spec.frame.kept, 1 | 2) {
            self.widen(step.at, canvas);
        }
        if !step.area.intersect(canvas.visible()).is_empty() {
            if spec.frame.shadow > 0.0 {
                self.shadow(r, spec.radius, spec.frame.shadow, canvas);
            }
            match spec.content {
                Content::Gradient { stops, turn } => self.gradient(r, spec.radius, stops, turn, canvas),
                _ => canvas.rounded(r, spec.radius, spec.fill),
            }
            canvas.border(r, spec.radius, spec.border_width, spec.border);
            self.content(step.at, r, canvas);
        }
        if spec.frame.clip {
            self.clips.push(canvas.clip(self.tree.scrolled(step.at, r)));
        }
    }

    /// The shadow under a box at `r` with corner `radius`, reaching so far: where the app
    /// described one, which hands over what draws it (see `Shows`).
    fn shadow(&self, r: Rect, radius: f32, reach: f32, canvas: &mut Canvas) {
        if let Some(shadow) = self.shows.shadow {
            shadow(canvas, r, radius, reach, Color::BLACK.alpha(0.4));
        }
    }

    /// A box at `r` filled with the gradient's stops `from..to` along `turn`: where the app
    /// described one, which hands over what draws it (see `Shows`).
    fn gradient(&self, r: Rect, radius: f32, [from, to]: [u32; 2], turn: Option<f32>, canvas: &mut Canvas) {
        if let Some(gradient) = self.shows.gradient {
            gradient(canvas, r, radius, &self.blends[from as usize..to as usize], turn);
        }
    }

    /// A box kept in view shows in all of its parent's view, not just where the parent's
    /// scrolled content shows: the canvas is clipped to that until the box is left.
    fn widen(&mut self, at: u32, canvas: &mut Canvas) {
        let (up, held) = (self.tree.specs[at as usize].frame.parent, canvas.clip_pixels());
        // The last clip put away is the parent's.
        canvas.restore_clip(self.clips.last().copied().unwrap_or(held));
        canvas.clip(self.tree.view(up, self.tree.rect_of(up)));
        self.clips.push(held);
    }

    /// What lies over a box's content: its scroll bars and focus ring. (And the clip sticky
    /// headings in it set is released; or, for such a heading, what follows it in its box is
    /// clipped to below it, as the survey has it: see `Lying::pin`.)
    fn leave(&mut self, step: &Seen, canvas: &mut Canvas) {
        if step.fade != Fade::No {
            return self.shows.fade.map_or((), |fade| fade(self, step, canvas));
        }
        let (spec, r) = (&self.tree.specs[step.at as usize], step.rect);
        if let Some(before) = (0..step.lets_go).filter_map(|_| self.clips.pop()).last() {
            canvas.restore_clip(before);
        }
        if let Some(before) = self.clips.pop_if(|_| spec.frame.clip) {
            canvas.restore_clip(before);
        }
        for axis in 0..2 {
            let Some((strip, from, long)) = bar(self.tree, step.at, r, axis) else { continue };
            let color = if self.bar == bar_id(spec.id, axis) { self.theme.text_faint } else { self.theme.line_strong };
            let part = [rect(from + 2.0, strip.y + 2.0, long - 4.0, 6.0), rect(strip.x + 2.0, from + 2.0, 6.0, long - 4.0)][axis];
            canvas.rounded(part, 3.0, color);
        }
        // On a box filled with the accent, the ring takes the colour of text on it, to be seen.
        if self.focus != 0 && spec.id == self.focus {
            let ring = if spec.fill.solid() == self.theme.accent.solid() { self.theme.on_accent } else { self.theme.accent };
            canvas.border(r, spec.radius, 2.0, ring);
        }
        if let Some(before) = self.clips.pop_if(|_| matches!(spec.frame.kept, 1 | 2)) {
            canvas.restore_clip(before);
        }
        if step.pins {
            self.clips.push(canvas.clip(rect(Rect::EVERYTHING.x, r.bottom(), Rect::EVERYTHING.w, Rect::EVERYTHING.h)));
        }
    }

    /// Where a see-through box starts, what is under it is kept; where it ends, that shows
    /// through all drawn of it since (see `Fade`), as through one picture.
    pub(crate) fn fade(&mut self, step: &Seen, canvas: &mut Canvas) {
        let (kept, parts) = &mut *self.under;
        match step.fade {
            Fade::Starts => parts.push(canvas.keep_under(step.area, kept)),
            Fade::Ends => parts.pop().into_iter().for_each(|part| canvas.let_through(part, kept, self.tree.specs[step.at as usize].fade)),
            Fade::No => {}
        }
    }

    /// What a box at `r` holds: a picture, a shape, an icon, or text.
    fn content(&mut self, at: u32, r: Rect, canvas: &mut Canvas) {
        let spec = &self.tree.specs[at as usize];
        if matches!(spec.content, Content::None | Content::Gradient { .. }) {
            return;
        }
        let (inside, scale) = (r.inset(spec.frame.pad), canvas.scale());
        // A clipping box (a table cell) keeps its own content inside too, but for its text's
        // lift: an accent is not cut off by it.
        let [up, down] = self.tree.lifted(at);
        let held = spec.frame.clip.then(|| canvas.clip(rect(r.x, r.y - up, r.w, r.h + up + down)));
        match spec.content {
            Content::None | Content::Gradient { .. } => {}
            Content::Stripes { color } => self.shows.stripes.map_or((), |stripes| stripes(self.tree, at, r, color, canvas)),
            Content::Text { .. } => self.words(at, inside, canvas),
            // A picture the app let go draws nothing.
            Content::Picture { picture, keep } => {
                let to = self.pictures.get(picture).map_or(inside, |stored| kept_shape(inside, [stored.width as f32, (stored.pixels.len() / stored.width.max(1)) as f32], keep));
                let shown = self.shows.picture.zip(self.pictures.get_mut(picture)).map(|(lay, stored)| (lay, stored.shown(to.w * scale, to.h * scale)));
                shown.map_or((), |(lay, (pixels, width))| lay(canvas, pixels, width, to, spec.radius))
            }
            Content::Drawing { drawing, color, keep } => {
                let to = self.drawings.get(drawing).map_or(inside, |kept| kept_shape(inside, kept.size(), keep));
                self.shows.drawing.zip(self.drawings.get_mut(drawing)).map_or((), |(show, kept)| show(kept, to, color, canvas))
            }
            Content::Film { film, frame, keep } => {
                let to = self.films.get(film).map_or(inside, |reel| kept_shape(inside, reel.size().map(|side| side as f32), keep));
                let shown = self.shows.film.zip(self.films.get_mut(film)).map(|(shown, reel)| shown(reel, frame as usize));
                shown.zip(self.shows.picture).map_or((), |((pixels, width), lay)| lay(canvas, pixels, width, to, spec.radius))
            }
            // A box with a solid fill, square corners and no border gets its scene drawn on
            // that colour and copied as is; any other has it blended over the box.
            Content::Scene { scene } => {
                let plain = spec.fill.a() == 255 && spec.radius == 0.0 && spec.border_width == 0.0;
                self.scenes.show(scene, inside, plain.then_some(spec.fill), canvas)
            }
            Content::Icon { icon, color } => self.icon(icon, color, inside, r, canvas),
            // The app's own shape, clipped to its box.
            Content::Shape { shape } => {
                let draw = self.shows.shape.map(|draw| move |canvas: &mut Canvas| draw(&self.shapes[shape as usize], canvas, pt(inside.x, inside.y)));
                draw.map_or((), |draw| canvas.held_to(r, draw))
            }
        }
        if let Some(before) = held {
            canvas.restore_clip(before);
        }
    }

    /// An icon at `inside` in a colour, clipped to its box at `r`. Rasterized once per size
    /// shown and kept, keyed by its path and how it is drawn (one path may show filled and
    /// stroked). It is a square as wide as `inside` on whole pixels: in a box shorter than
    /// wide, or at a scale putting its corner and width half way between pixels, a pixel could
    /// lie past the box, where changes are not tracked; so none does.
    fn icon(&mut self, at: u32, color: Color, inside: Rect, r: Rect, canvas: &mut Canvas) {
        let (icon, scale, Some(draw_mask)) = (self.icons[at as usize], canvas.scale(), self.shows.icon) else { return };
        let px = (inside.w * scale).round().max(1.0) as u32;
        let drawn_as = [px, icon.stroke.to_bits(), icon.view.to_bits()];
        let mask = self.masks.entry((icon.path.as_ptr() as usize, drawn_as)).or_insert_with(|| draw_mask(icon, px as usize));
        let held = canvas.clip(r);
        canvas.cover((inside.x * scale).round() as i32, (inside.y * scale).round() as i32, mask.width, &mask.cover, color);
        canvas.restore_clip(held);
    }

    /// A box's text line by line from the corner of `inside`, with its selection and cursor.
    fn words(&mut self, at: u32, inside: Rect, canvas: &mut Canvas) {
        let (tree, scale, visible) = (self.tree, canvas.scale(), canvas.visible());
        let (spec, place) = (&tree.specs[at as usize], &tree.places[at as usize]);
        let Content::Text { start, end, font, color, wrap, side } = spec.content else { return };
        // Text that does not wrap is one line. (Its real width matters only where it does not
        // start at the start.)
        let asked = place.fit[0] - spec.frame.pad.horizontal();
        let wide = if side == Align::Start { asked } else { tree.text_width(at, scale).unwrap_or(asked) };
        let whole = [Line { start: start as usize, end: end as usize, width: wide }];
        let lines = if wrap { &tree.lines[place.lines.0 as usize..place.lines.1 as usize] } else { &whole[..] };
        // How much of the room a line leaves goes before it.
        let before = [0.0, 0.5, 1.0, 0.0][side as usize];
        // The selection (from, to) and the cursor, as byte offsets in the run's text.
        let marked = spec.select.map(|[a, b]| (start as usize + a.min(b) as usize, start as usize + a.max(b) as usize, start as usize + b as usize));
        let cursor_line = marked.and_then(|(.., at)| lines.iter().rposition(|l| l.start <= at));
        // One line is centred in its box's height; wrapped lines start at the top.
        let top = inside.y + if wrap { 0.0 } else { ((inside.h - place.line_height) / 2.0).max(0.0) };
        // One line too long for a clipping box is cut and ends in "…".
        let cut = if wrap || marked.is_some() || !spec.frame.clip { None } else { self.cut(at, inside.w, scale) };
        for (i, line) in lines.iter().enumerate() {
            let y = top + i as f32 * place.line_height;
            if y >= visible.bottom() || y + place.line_height <= visible.y {
                continue;
            }
            // A line runs in its paragraph's direction (the paragraph starts after the last
            // line end before it); a right-to-left line starts at the right.
            let paragraph = tree.strings[start as usize..line.start].rfind('\n').map_or(start as usize, |at| start as usize + at + 1);
            let leftward = crate::order::leftward(&tree.strings[paragraph..end as usize]);
            let x = inside.x + ((inside.w - line.width).max(0.0) * if leftward { 1.0 - before } else { before }).round();
            // The line's selected part (several pieces where it runs both ways), and where the
            // cursor shows on it.
            let (mut selected, text) = (Vec::new(), &tree.strings[line.start..line.end]);
            let at = marked.filter(|_| spec.caret && cursor_line == Some(i)).map(|(.., at)| at);
            let cursor = marked.and_then(|(from, to, _)| crate::text::marks(self.text, text, line.start, line.width, font, scale, leftward, [from, to], at, self.after, &mut selected));
            for [left, right] in selected {
                canvas.rect(rect(x + left, y, right - left, place.line_height), self.theme.accent.alpha(0.35));
            }
            // Letters are centred in the line's height (see `Metrics::lift`). A cut line keeps
            // its start (at the right for right-to-left) and the dots go at its end.
            let room = inside.x + inside.w;
            let ends = cut.map(|(_, wide)| if leftward { (room - wide, room - wide - self.text.measure("…", font, scale, None)) } else { (x, x + wide) });
            let from = ends.map_or(x, |(from, _)| from);
            self.text.draw_line(canvas, &tree.strings[line.start..cut.map_or(line.end, |(end, _)| end)], from, y - place.lift, font, color, leftward);
            if let Some((_, dots)) = ends {
                self.text.draw(canvas, "…", dots, y - place.lift, font, color);
            }
            if font.rule != Rule::None {
                self.rule(canvas, rect(x, y - place.lift, cut.map_or(line.width, |_| inside.w), place.line_height), font, color);
            }
            if let Some(at) = cursor {
                canvas.rect(rect(x + at, y, 1.0, place.line_height), self.theme.text);
            }
        }
        // And the wavy lines under misspelled words, where checked (see `spell`).
        if let Some(hooks) = self.spelling.hooks() {
            (hooks.wavy)(canvas, self.spelling, at);
        }
    }
}

impl Painter<'_> {
    /// A font's decoration line for one line of text in `line`: just under the letters, or
    /// through the middle of the x-height. A fourteenth of the letters' height thick, at least
    /// a pixel.
    fn rule(&mut self, canvas: &mut Canvas, line: Rect, font: Font, color: Color) {
        let (m, scale) = (self.text.metrics(font, canvas.scale()), canvas.scale());
        let thick = (font.size / 14.0 * scale).round().max(1.0) / scale;
        // A font without an x-height: about half its size.
        let small = if m.small > 0.0 { m.small } else { font.size / 2.0 };
        let down = if font.rule == Rule::Under { m.ascent + thick } else { m.ascent - (small + thick) / 2.0 };
        canvas.rect(rect(line.x, line.y + down, line.w, thick), color);
    }

    /// Where the single line in box `at` is cut to fit `room` with "…" after it: the end byte
    /// and the remaining width. Nothing when it all fits.
    fn cut(&mut self, at: u32, room: f32, scale: f32) -> Option<(usize, f32)> {
        let Content::Text { start, end, font, .. } = self.tree.specs[at as usize].content else { return None };
        let piece = &self.tree.strings[start as usize..end as usize];
        let wide = self.tree.text_width(at, scale).unwrap_or_else(|| self.text.measure(piece, font, scale, None));
        if wide <= room {
            return None;
        }
        self.stops.clear();
        self.text.measure(piece, font, scale, Some(self.stops));
        let dots = self.text.measure("…", font, scale, None);
        // As many characters as end early enough to leave room for the dots.
        let kept = self.stops.partition_point(|stop| *stop + dots <= room);
        let bytes = piece.char_indices().nth(kept).map_or(piece.len(), |(byte, _)| byte);
        Some((start as usize + bytes, if kept == 0 { 0.0 } else { self.stops[kept - 1] }))
    }
}

/// Where in `inside` something `size` large is drawn: all of it; or keeping its aspect
/// (`keep`), as large as fits, centred. (Something of no size covers all of it, as nothing.)
fn kept_shape(inside: Rect, [wide, high]: [f32; 2], keep: bool) -> Rect {
    if !keep || wide <= 0.0 || high <= 0.0 {
        return inside;
    }
    let by = (inside.w / wide).min(inside.h / high);
    let (w, h) = (wide * by, high * by);
    rect(inside.x + ((inside.w - w) / 2.0).round(), inside.y + ((inside.h - h) / 2.0).round(), w, h)
}

/// An icon rasterized at `px` pixels square: each pixel's coverage.
pub(crate) fn draw_mask(icon: Icon, px: usize) -> Mask {
    Mask { width: px, cover: coverage(&Path::svg(icon.path), px as f32 / icon.view, icon.stroke, (0, 0), px, px) }
}

#[cfg(test)]
mod tests {
    use crate::geom::Size;
    use crate::text::BuiltInFont;
    use crate::{Color, Icon, LocalClipboard, Path, Picture, Surface, Ui};

    /// A window of plain words, and with a picture, a row of the five things only some programs
    /// show above them: five boxes 40 units square, 10 apart, from the top left.
    fn describe(ui: &mut Ui, all: Option<Picture>) {
        let red = Color::rgb(0xff0000);
        if let Some(picture) = all {
            ui.row().gap(10.0).open();
            ui.el().icon(Icon::line("M2 2 14 14M14 2 2 14"), 40.0, red).show();
            ui.el().size(40.0, 40.0).shape(&Path::svg("M0 0h40v40h-40z"), red).show();
            ui.el().size(40.0, 40.0).fill(red).shadow(6.0).show();
            ui.el().size(40.0, 40.0).gradient(red, Color::rgb(0x0000ff)).show();
            ui.el().picture(picture).size(40.0, 40.0).show();
            ui.close();
        }
        ui.label("Plain words");
    }

    #[test]
    fn what_only_some_programs_show_is_handed_over_by_the_first_call_that_describes_it() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 260.0, h: 120.0 }, 1.0);
        ui.theme.bg = Color::WHITE;
        ui.run(&mut |ui| describe(ui, None));
        // A program that never described an icon, shape, shadow, gradient or picture has
        // nothing that draws them, so it is built without that. (Taking a picture in is not
        // showing it.)
        let green = ui.add_picture(1, vec![0xff00_ff00]);
        let shows = ui.shows;
        assert!(shows.icon.is_none() && shows.shape.is_none() && shows.shadow.is_none() && shows.gradient.is_none() && shows.picture.is_none());
        ui.run(&mut |ui| describe(ui, Some(green)));
        let shows = ui.shows;
        assert!(shows.icon.is_some() && shows.shape.is_some() && shows.shadow.is_some() && shows.gradient.is_some() && shows.picture.is_some());
        // And each is drawn: red where the icon crosses, all over the shape and the third box,
        // a shadow under that box, blue at the bottom of the fourth.
        let screen = Surface::draw(&mut ui, 64);
        let at = |x: usize, y: usize| screen.pixels[y * 260 + x] & 0xff_ffff;
        assert_eq!([at(20, 20), at(70, 20), at(120, 20)], [0xff0000; 3], "the icon's crossing, the shape, the filled box");
        assert!(at(120, 43) != 0xff_ffff && at(120, 43) & 0xff == at(120, 43) >> 16, "a grey shadow under the third box");
        assert!(at(170, 37) & 0xff > 0x80 && at(170, 2) & 0xff < 0x20, "the fourth box goes over into blue");
        assert_eq!(at(220, 20), 0x00ff00, "the picture fills the fifth");
    }

    /// A bar across the top and, under it, stacked boxes: what lies under, and at the right a
    /// panel drawing only its shadow, which reaches up over the bar. In it, a filled column.
    #[derive(Default)]
    struct Shaded {
        open: bool,
        wide: bool,
        count: u32,
    }
    fn shaded(app: &mut Shaded, ui: &mut Ui) {
        use crate::{Align, Flow, Length, Role};
        let t = ui.theme;
        ui.column().grow().open();
        ui.row().h(Length::Px(40.0)).fill(t.panel).open();
        ui.label(&format!("Bar {}", app.count));
        ui.close();
        ui.el().flow(Flow::Layers).grow().open();
        ui.column().grow().pad(10.0).open();
        app.open ^= ui.button("Panel").clicked;
        app.wide ^= ui.button("Wider").clicked;
        app.count += ui.button("Count").clicked as u32;
        ui.close();
        if app.open {
            let wide = if app.wide { 250.0 } else { 150.0 };
            ui.el().place((Align::End, Align::Start)).w(Length::Px(wide)).h(Length::Grow(1.0)).shadow(16.0).open();
            let shown = ui.column().grow().fill(t.raise).sense().open();
            ui.tell(&shown, Role::Group, None, "Side");
            ui.label(&format!("In it {}", app.count));
            ui.close();
            ui.close();
        }
        ui.close();
        ui.close();
    }

    #[test]
    fn a_shadow_that_reaches_past_the_box_is_drawn_again_where_it_was() {
        use crate::Role;
        use crate::testing::Trial;
        // Its shadow comes, goes, grows and changes over the bar step by step; each time the
        // redraw matches a full redraw (`Trial` checks that). A box drawing only its shadow was
        // once taken to draw nothing, and its shadow over the bar stayed when it went.
        let mut trial = Trial::new(Shaded::default(), shaded);
        for _ in 0..2 {
            trial.press(Role::Button, "Panel");
            trial.press(Role::Button, "Count");
            trial.press(Role::Button, "Wider");
            trial.press(Role::Button, "Panel");
        }
        assert!(!trial.app.open && trial.app.count == 2);
    }

    #[test]
    fn an_icon_draws_nothing_outside_its_box() {
        use crate::testing::Trial;
        // An icon 11 units wide, 7 units from the left, at 150%: corner at 10.5 pixels and
        // width 16.5, each rounded up, so it drew to pixel 28, one past its box (which ends at
        // 27). Removed, it left that column behind.
        fn icon(on: &mut bool, ui: &mut Ui) {
            let red = Color::rgb(0xff0000);
            ui.column().grow().pad(7.0).open();
            *on ^= ui.button("Icon").clicked;
            if *on {
                ui.el().icon(Icon::solid("M0 0h24v24H0z"), 11.0, red).show();
            }
            ui.close();
        }
        let mut trial = Trial::new(true, icon);
        trial.resize(300.0, 200.0, 1.5);
        trial.press(crate::Role::Button, "Icon");
        trial.press(crate::Role::Button, "Icon");
        trial.press(crate::Role::Button, "Icon");
        assert!(!trial.app);
    }
}
