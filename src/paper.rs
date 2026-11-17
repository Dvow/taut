//! Paper: pages of any size with an app's content, to save as PDF (`pdf.rs`) or print
//! (`print.rs`). The marks are a canvas's: shapes, filled or stroked, in a colour or gradient;
//! text; pictures; and links on top. Everything is in units from the page's top-left, as in a
//! window: a unit is 1/96 inch, so what is 400 units wide on a 100% screen is as wide on paper.
//!
//! Text is kept as a font file's glyphs with the text each stands for, so a viewer can search
//! and copy it. It is shaped as the library shapes text (`shaping.rs`): kerned, with ligatures,
//! and with `right-to-left` reordered and joined. Its fonts are the app's where given
//! ([`Paper::font`]), else the system's: the face's font, and for characters it lacks, the font
//! the system would use. A run of one script is shaped by the font's features for that script
//! and the paper's language ([`Paper::language`]). A page draws the same way on a canvas
//! (`draw`): that is what prints where the system takes pixels, and what tests compare a
//! renderer's output against.
//!
//! Paginated documents are `markdown_pages.rs`'s; and a window's content can go on a page as an
//! image ([`Paper::interface`]).
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::FontFile;
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::path::Path;
use crate::pen::{Full, Inked, Paint, Pen, fully};
use crate::scripts::Language;
use crate::shaping::{LONGEST, Laid, Plan, Script};
use crate::text::{Decoration, Face, Font, Metrics, cluster};
use crate::ui::{Icon, Ui};

/// Pixels per unit where a page, or part of one, is rasterized: 300 per inch, the resolution
/// printers print text at.
pub(crate) const FINE: f32 = 300.0 / 96.0;
/// The most system fonts looked up for characters the fonts found so far lack: each lookup
/// reads a font. (Past that, such a character is the face font's notdef.)
const LOOKS: usize = 64;

/// Pages to save as PDF (`pdf`) or print (`taut::print`): each its own size, with shapes, text,
/// pictures and links. Marks go on the page begun last.
///
/// ```
/// use taut::{Color, Font, Paper, Path};
/// let mut paper = Paper::new();
/// paper.page(Paper::A4);
/// paper.text("Invoice 2026-114", 72.0, 72.0, Font::new(14.0), Color::BLACK);
/// let mut wave = Path::new();
/// wave.move_to(72.0, 160.0).quad_to(160.0, 100.0, 248.0, 160.0);
/// paper.outline(&wave, 2.0, Color::rgb(0x2b6cb0));
/// let file = paper.to_pdf();
/// assert!(file.starts_with(b"%PDF-"));
/// ```
#[derive(Default)]
pub struct Paper {
    /// The document's title: what a viewer shows as its title.
    pub title: String,
    /// The language of text from now on, for its localized glyph forms: the system's at first;
    /// another for text in another ([`Language::named("sr")`](Language::named)), or
    /// [`Language::NONE`] for none.
    pub language: Language,
    pub(crate) pages: Vec<Sheet>,
    /// The page marks go on.
    on: usize,
    /// The font files text uses, each once; the app's own, by face; and system font lookups
    /// done: a face, a character (zero for the face's own font), and the font found, if any.
    pub(crate) files: Vec<FontFile>,
    own: Vec<(Face, u32)>,
    found: Vec<(Face, char, Option<u32>)>,
    /// Each font's shaping plan per script, direction and language: by the font's index among
    /// the files.
    plans: Vec<(u32, Script, bool, Language, Plan)>,
    /// The pictures on the pages, each once; and those an app supplied for the image names a
    /// document uses.
    pub(crate) pictures: Vec<Picture>,
    pub(crate) named: Vec<(String, u32)>,
    /// Link destinations in the pages: a name, the page, and the offset down it.
    pub(crate) anchors: Vec<(String, usize, f32)>,
    /// The JPEG decoder for drawing pages: handed over by the first JPEG placed (`jpeg`), so a
    /// program that prints without one carries no JPEG decoder.
    unpack: Option<Unpack>,
    /// Kept only to reuse memory: shaped glyphs, and a bidirectional line's order.
    laid: Vec<Laid>,
    order: crate::bidi::Order,
}

/// Decodes a JPEG file into its width and pixels.
type Unpack = fn(&[u8]) -> Option<(usize, Vec<u32>)>;

/// A page: its size, and its marks in order.
pub(crate) struct Sheet {
    pub size: Size,
    pub marks: Vec<Mark>,
}

/// One mark on a page.
pub(crate) enum Mark {
    /// A shape, filled or stroked.
    Shape(Box<Drawn>),
    /// Glyphs of one font, in one size and colour.
    Words(Box<Words>),
    /// A picture, by index, stretched over a rectangle.
    Picture(u32, Rect),
    /// From here on, nothing shows outside a rectangle (nor outside earlier clips).
    Clip(Rect),
    /// The last clip ends.
    Unclip,
    /// A link rectangle: to a URL, or `#` and a destination name in the pages
    /// ([`Paper::anchor`]).
    Link(Rect, String),
}

/// A shape and how it is drawn: its stroke pen (none for a fill), with the dashes stored
/// separately, and its paint.
pub(crate) struct Drawn {
    pub path: Path,
    pub pen: Option<Pen<'static>>,
    pub dashes: Vec<f32>,
    pub paint: PagePaint,
    /// Its draw function where the canvas cannot draw it alone (see `pen::fully`).
    pub full: Option<Full>,
}

/// A paint as a page keeps it: one colour, or a linear gradient at so many degrees from up, or
/// radial (see [`Paint`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PagePaint {
    Color(Color),
    Linear(f32, Vec<(f32, Color)>),
    Radial(Vec<(f32, Color)>),
}

/// Glyphs of one font on a page: the font (its index among the paper's files), size and colour;
/// each glyph with its pen position on its baseline and the bytes of `text` it stands for (none
/// for a second glyph of one character). Left to right.
pub(crate) struct Words {
    pub font: u32,
    pub size: f32,
    pub color: Color,
    pub glyphs: Vec<Glyph>,
    pub text: String,
}

/// One glyph on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glyph {
    pub id: u16,
    pub at: Point,
    pub chars: [u32; 2],
}

/// A picture on the pages: rows of non-premultiplied `0xAARRGGBB` pixels; or a JPEG file as is,
/// which the viewer decodes, with its upright size, component count, whether Adobe wrote it,
/// and its EXIF orientation.
pub(crate) enum Picture {
    Pixels { width: usize, pixels: Vec<u32> },
    Jpeg { file: Vec<u8>, size: [usize; 2], planes: u8, adobe: bool, turn: usize },
}

/// A separately shaped run of a line: its bytes, font, whether right-to-left, and its script
/// (none for figures and signs alone).
#[derive(Clone, Debug)]
struct Run {
    bytes: std::ops::Range<usize>,
    file: Option<u32>,
    leftward: bool,
    script: Option<Script>,
}

impl std::fmt::Debug for Paper {
    /// Its title, language and page count; not the pages' content.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paper").field("title", &self.title).field("language", &self.language).field("pages", &self.pages.len()).finish_non_exhaustive()
    }
}

impl Paper {
    /// A4, 210 by 297 millimetres, in units.
    pub const A4: Size = Size { w: 210.0 / 25.4 * 96.0, h: 297.0 / 25.4 * 96.0 };
    /// US Letter, 8½ by 11 inches, in units.
    pub const LETTER: Size = Size { w: 816.0, h: 1056.0 };

    /// No pages yet.
    pub fn new() -> Self {
        Paper { language: Language::of_the_system(), ..Paper::default() }
    }

    /// The largest page side, in units: 200 inches, PDF's maximum (14,400 points).
    pub(crate) const LARGEST: f32 = 200.0 * 96.0;

    /// Begins a page of this size in units ([`Paper::A4`], or any up to 200 inches a side):
    /// marks from now on go on it. A landscape page is the same size swapped.
    pub fn page(&mut self, size: Size) {
        self.pages.push(Sheet { size: Paper::sheet(size), marks: Vec::new() });
        self.on = self.pages.len() - 1;
    }
    /// A page's actual size for `size`: at least a unit a side (also for NaN), at most 200
    /// inches.
    pub(crate) fn sheet(size: Size) -> Size {
        let side = |side: f32| if side >= 1.0 { side.min(Paper::LARGEST) } else { 1.0 };
        Size { w: side(size.w), h: side(size.h) }
    }
    /// Directs marks from now on to page `page` (from 0) instead: to add page numbers once all
    /// are laid out. (Past the last page, to that.)
    pub fn set_page(&mut self, page: usize) {
        self.on = page.min(self.pages.len().saturating_sub(1));
    }
    /// The page count.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
    /// Page `page`'s size, in units.
    pub fn size(&self, page: usize) -> Size {
        self.pages.get(page).map_or(Size { w: 0.0, h: 0.0 }, |sheet| sheet.size)
    }

    /// Adds a mark to the current page (beginning an A4 page if there is none).
    pub(crate) fn put(&mut self, mark: Mark) {
        if self.pages.is_empty() {
            self.page(Paper::A4);
        }
        self.pages[self.on].marks.push(mark);
    }

    // ---- shapes ----

    /// A rectangle filled with a colour.
    pub fn rect(&mut self, r: Rect, color: Color) {
        self.rounded(r, 0.0, color);
    }
    /// A rectangle with `radius` corners, filled with a colour.
    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        let mut path = Path::new();
        path.rounded_rect(r.x, r.y, r.w, r.h, radius);
        self.shape(&path, color);
    }
    /// A rectangle's border with `radius` corners, `width` thick, inside it.
    pub fn border(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let width = width.min(r.w / 2.0).min(r.h / 2.0);
        if width <= 0.0 {
            return;
        }
        let (mut path, mut inner) = (Path::new(), Path::new());
        path.rounded_rect(r.x, r.y, r.w, r.h, radius);
        inner.rounded_rect(r.x + width, r.y + width, r.w - 2.0 * width, r.h - 2.0 * width, (radius - width).max(0.0));
        path.add(inner.reverse());
        self.shape(&path, color);
    }
    /// A rectangle with `radius` corners filled with a gradient, as [`Canvas::gradient`] does:
    /// linear at `turn` degrees clockwise from up, or with none, radial.
    pub fn gradient(&mut self, r: Rect, radius: f32, colors: &[(f32, Color)], turn: Option<f32>) {
        let mut path = Path::new();
        path.rounded_rect(r.x, r.y, r.w, r.h, radius);
        self.shape(&path, turn.map_or(Paint::Radial(colors), |turn| Paint::Linear(turn, colors)));
    }
    /// A shape, filled by the nonzero rule.
    pub fn shape<'a>(&mut self, path: &Path, ink: impl Into<Paint<'a>>) {
        let ink = ink.into();
        self.drawn(path, None, ink, fully(None, &ink));
    }
    /// A shape's stroke with a pen (a plain number is a pen that wide, with round caps and
    /// joins).
    pub fn outline<'a, 'b>(&mut self, path: &Path, pen: impl Into<Pen<'a>>, ink: impl Into<Paint<'b>>) {
        let (pen, ink) = (pen.into(), ink.into());
        self.drawn(path, Some(pen), ink, fully(Some(&pen), &ink));
    }
    /// Stores a shape with its pen and paint: with the draw function for what the canvas cannot
    /// draw alone, which only a call asking for such a shape hands over.
    fn drawn(&mut self, path: &Path, pen: Option<Pen>, paint: Paint, full: Option<Full>) {
        let paint = match paint {
            Paint::Color(color) if color.is_none() => return,
            Paint::Color(color) => PagePaint::Color(color),
            Paint::Linear(turn, stops) => PagePaint::Linear(turn, stops.to_vec()),
            Paint::Radial(stops) => PagePaint::Radial(stops.to_vec()),
        };
        let plain_pen = pen.map(|pen| Pen { width: pen.width, cap: pen.cap, join: pen.join, dashes: &[], offset: pen.offset });
        let dashes = pen.map_or(Vec::new(), |pen| pen.dashes.to_vec());
        self.put(Mark::Shape(Box::new(Drawn { path: path.clone(), pen: plain_pen, dashes, paint, full })));
    }

    // ---- pictures, links and clipping ----

    /// A picture stretched over `r`: rows of non-premultiplied `0xAARRGGBB` pixels (as
    /// [`jpeg::decode`](crate::jpeg::decode) and [`png::decode`](crate::png::decode) return
    /// them), `width` per row. A picture placed on many pages is stored once.
    pub fn picture(&mut self, width: usize, pixels: &[u32], r: Rect) {
        if let Some(at) = self.keep_picture(width, pixels) {
            self.put(Mark::Picture(at, r));
        }
    }
    /// Stores a picture's pixels once: returns its index. None for no pixels.
    fn keep_picture(&mut self, width: usize, pixels: &[u32]) -> Option<u32> {
        if width == 0 || pixels.len() < width {
            return None;
        }
        let pixels = &pixels[..pixels.len() / width * width];
        let same = |kept: &Picture| matches!(kept, Picture::Pixels { width: w, pixels: p } if *w == width && p == pixels);
        if let Some(at) = self.pictures.iter().position(same) {
            return Some(at as u32);
        }
        self.pictures.push(Picture::Pixels { width, pixels: pixels.to_vec() });
        Some(self.pictures.len() as u32 - 1)
    }
    /// A JPEG file stretched over `r`, upright per its EXIF orientation. It is embedded as is
    /// and decoded by the viewer: a photo takes no more room than its file. (Kinds viewers may
    /// not decode are embedded as pixels.) False for a file this cannot read as JPEG.
    pub fn jpeg(&mut self, file: &[u8], r: Rect) -> bool {
        self.unpack = Some(crate::jpeg::decode);
        if let Some(picture) = passed(file) {
            self.pictures.push(picture);
            self.put(Mark::Picture(self.pictures.len() as u32 - 1, r));
            return true;
        }
        let Some((width, pixels)) = crate::jpeg::decode(file) else { return false };
        self.picture(width, &pixels, r);
        true
    }
    /// Supplies the picture a document names `name` (see [`Paper::markdown`]), as `picture`
    /// takes one.
    pub fn set_picture(&mut self, name: &str, width: usize, pixels: &[u32]) {
        if let Some(at) = self.keep_picture(width, pixels) {
            self.named.retain(|(known, _)| known != name);
            self.named.push((name.to_string(), at));
        }
    }
    /// A window's content as a picture over `r`, at 300 pixels per inch: an interface described
    /// by `describe` in a window of `r`'s size, with system text and the light theme. Its text
    /// is image too, not text a viewer finds.
    pub fn interface(&mut self, r: Rect, describe: &mut dyn FnMut(&mut Ui)) {
        let text = crate::platform::system_text();
        let mut ui = Ui::new(text, Box::new(crate::platform::LocalClipboard::default()));
        ui.theme = crate::theme::Theme::light();
        ui.resize(Size { w: r.w.max(1.0), h: r.h.max(1.0) }, FINE);
        // (Run until nothing changes: what one run measures, the next lays out.)
        for _ in 0..4 {
            if ui.run(describe).is_empty() {
                break;
            }
        }
        let drawn = crate::surface::Surface::draw(&mut ui, 64);
        let pixels: Vec<u32> = drawn.pixels.iter().map(|pixel| unmixed(*pixel)).collect();
        self.picture(drawn.width, &pixels, r);
    }
    /// An icon ([`Icon`]) over `r` in a colour: stroked at its own width, or filled.
    pub fn icon(&mut self, icon: Icon, r: Rect, color: Color) {
        let by = r.w.min(r.h) / icon.view.max(1e-3);
        let mut path = Path::svg(icon.path);
        path.transform(crate::geom::Transform::new().scale(by, by).translate(r.x, r.y));
        match icon.stroke > 0.0 {
            true => self.outline(&path, icon.stroke * by, color),
            false => self.shape(&path, color),
        }
    }
    /// A rectangle linking to `to` in a viewer: a URL (`https://…`, `mailto:…`), or `#` and a
    /// destination name in the pages (`anchor`).
    pub fn link(&mut self, r: Rect, to: &str) {
        if !to.is_empty() && !r.is_empty() {
            self.put(Mark::Link(r, to.to_string()));
        }
    }
    /// Names a destination on the current page, `y` units down: a link to `#` and the name goes
    /// there.
    pub fn anchor(&mut self, name: &str, y: f32) {
        if self.pages.is_empty() {
            self.page(Paper::A4);
        }
        self.anchors.push((name.to_string(), self.on, y));
    }
    /// From now on, nothing drawn shows outside `r` (nor outside earlier clips), until
    /// `unclip`.
    pub fn clip(&mut self, r: Rect) {
        self.put(Mark::Clip(r));
    }
    /// Ends the last `clip`.
    pub fn unclip(&mut self) {
        self.put(Mark::Unclip);
    }

    // ---- text ----

    /// Draws a face's text in an app font (characters it lacks fall back to system fonts).
    pub fn font(&mut self, face: Face, file: FontFile) {
        self.files.push(file);
        self.own.retain(|(known, _)| *known != face);
        self.own.push((face, self.files.len() as u32 - 1));
    }

    /// Draws one line of text with its top-left at `x`, `y`. Returns its width.
    pub fn text(&mut self, text: &str, x: f32, y: f32, font: Font, color: Color) -> f32 {
        let line = y + self.metrics(font).ascent;
        self.line(text, x, line, font, color)
    }
    /// Draws one line of text on a baseline `line` units down, from `x`. Returns its width.
    pub(crate) fn line(&mut self, text: &str, x: f32, line: f32, font: Font, color: Color) -> f32 {
        let mut pen = x;
        for run in self.runs(text, font.face) {
            pen += self.write(&text[run.bytes.clone()], &run, font.size, [pen, line], color);
        }
        if font.decoration != Decoration::None && pen > x {
            let top = line - self.metrics(font).ascent;
            self.rule(rect(x, top, pen - x, 0.0), font, color);
        }
        pen - x
    }
    /// A line of text's width.
    pub fn measure(&mut self, text: &str, font: Font) -> f32 {
        self.stops(text, font, &mut Vec::new())
    }
    /// A font's vertical metrics, by the font file it uses.
    pub fn metrics(&mut self, font: Font) -> Metrics {
        let main = self.file_for(font.face, '\0');
        main.map_or(Metrics { ascent: font.size * 0.9, descent: font.size * 0.25, cap_height: font.size * 0.7, x_height: font.size * 0.5 }, |at| self.files[at as usize].metrics(font.size))
    }
    /// Text wrapped to `r`'s width, from its top: at newlines, else spaces. Lines are 1.5 times
    /// the font size, as a document paragraph's are. Returns their total height.
    pub fn paragraph(&mut self, text: &str, r: Rect, font: Font, color: Color) -> f32 {
        let (mut stops, mut lines) = (Vec::new(), Vec::new());
        self.stops(text, font, &mut stops);
        crate::text::wrap(text, &stops, r.w, &mut lines);
        let (high, height) = ((font.size * 1.5).round(), self.metrics(font).height());
        for (n, line) in lines.iter().enumerate() {
            let y = r.y + n as f32 * high + ((high - height) / 2.0).max(0.0);
            self.text(&text[line.start..line.end], r.x, y, font, color);
        }
        lines.len() as f32 * high
    }

    /// A text decoration line, under or through the text, for text in `line` (as
    /// `Painter::rule` draws it in a window).
    fn rule(&mut self, line: Rect, font: Font, color: Color) {
        let m = self.metrics(font);
        let thick = (font.size / 14.0).max(0.5);
        let small = if m.x_height > 0.0 { m.x_height } else { font.size / 2.0 };
        let down = if font.decoration == Decoration::Underline { m.ascent + thick } else { m.ascent - (small + thick) / 2.0 };
        self.rect(rect(line.x, line.y + down, line.w, thick), color);
    }

    /// Each character's end from a line's start, in logical order; and the line's width. (A
    /// character drawn as part of a ligature ends where the ligature does.)
    pub(crate) fn stops(&mut self, text: &str, font: Font, stops: &mut Vec<f32>) -> f32 {
        stops.clear();
        let (mut runs, mut ends, mut whole) = (self.runs(text, font.face), Vec::new(), 0.0);
        runs.sort_by_key(|run| run.bytes.start);
        // (Each run is summed in its font's units, as drawn: so a line is exactly as wide as
        // measured.)
        for run in &runs {
            let piece = &text[run.bytes.clone()];
            let unit = run.file.map_or(0.0, |file| self.files[file as usize].unit(font.size));
            ends.clear();
            ends.resize(piece.chars().count(), 0);
            for laid in self.shaped(piece, run) {
                ends[laid.from as usize] += laid.advance;
            }
            let mut sum = 0;
            for end in &ends {
                sum += end;
                stops.push(whole + sum as f32 * unit);
            }
            whole += sum as f32 * unit;
        }
        whole
    }

    /// Draws a shaped run, its pen at `at` on the baseline: returns its width.
    fn write(&mut self, piece: &str, run: &Run, size: f32, at: [f32; 2], color: Color) -> f32 {
        let Some(file) = run.file else { return 0.0 };
        let unit = self.files[file as usize].unit(size);
        let laid = self.shaped(piece, run);
        // The bytes each glyph stands for: from its character to the next character any glyph
        // starts at.
        let starts: Vec<usize> = piece.char_indices().map(|(at, _)| at).chain([piece.len()]).collect();
        let mut firsts: Vec<u32> = laid.iter().map(|laid| laid.from).collect();
        firsts.sort_unstable();
        firsts.dedup();
        let (mut pen, mut glyphs, mut taken) = (0, Vec::with_capacity(laid.len()), Vec::new());
        for one in &laid {
            let next = firsts.partition_point(|from| *from <= one.from);
            let end = firsts.get(next).map_or(starts.len() - 1, |from| *from as usize);
            let chars = if taken.contains(&one.from) { [0, 0] } else { [starts[one.from as usize] as u32, starts[end] as u32] };
            taken.push(one.from);
            let x = at[0] + (pen + one.offset[0]) as f32 * unit;
            glyphs.push(Glyph { id: one.glyph, at: pt(x, at[1] - one.offset[1] as f32 * unit), chars });
            pen += one.advance;
        }
        if !glyphs.is_empty() && size > 0.0 && size.is_finite() {
            self.put(Mark::Words(Box::new(Words { font: file, size, color, glyphs, text: piece.to_string() })));
        }
        pen as f32 * unit
    }

    /// A run's glyphs, in visual order.
    fn shaped(&mut self, piece: &str, run: &Run) -> Vec<Laid> {
        let Some(file) = run.file else { return Vec::new() };
        let (script, language) = (run.script.unwrap_or(Script::None), self.language);
        let plan = match self.plans.iter().position(|(of, known, way, said, _)| *of == file && *known == script && *way == run.leftward && *said == language) {
            Some(plan) => plan,
            None => {
                self.plans.push((file, script, run.leftward, language, Plan::of(&self.files[file as usize], script, run.leftward, language)));
                self.plans.len() - 1
            }
        };
        self.files[file as usize].shape(&self.plans[plan].4, piece, &mut self.laid);
        std::mem::take(&mut self.laid)
    }

    /// Splits a line into separately shaped runs, in display order: bidirectionally where
    /// needed, by the Unicode algorithm (with `right-to-left`); and each of those into runs of
    /// one font.
    fn runs(&mut self, text: &str, face: Face) -> Vec<Run> {
        let mut stretches = Vec::new();
        match crate::bidi::plain(text) {
            true => stretches.push((0..text.len(), 0)),
            false => {
                self.order.of(text, None);
                self.order.shown(text, &mut stretches);
            }
        }
        let mut runs = Vec::new();
        for (bytes, level) in stretches {
            let first = runs.len();
            self.cut(text, bytes, level % 2 == 1, face, &mut runs);
            if level % 2 == 1 {
                runs[first..].reverse();
            }
        }
        runs
    }

    /// Splits a single-direction range into runs of one font and script (and no more characters
    /// than are shaped at once), in logical order. (A script-less character joins its run.)
    fn cut(&mut self, text: &str, bytes: std::ops::Range<usize>, leftward: bool, face: Face, runs: &mut Vec<Run>) {
        let (mut at, mut count) = (bytes.start, 0);
        while at < bytes.end {
            let size = cluster(&text[at..bytes.end]).max(1);
            let letters = &text[at..at + size];
            let script = letters.chars().find_map(Script::of);
            let apart = |run: &Run| matches!((run.script, script), (Some(one), Some(other)) if one != other);
            let current = runs.last().filter(|run: &&Run| run.bytes.end == at && run.leftward == leftward && count < LONGEST - 64 && !apart(run));
            let keeps = current.and_then(|run| run.file).is_some_and(|file| letters.chars().all(|letter| unseen(letter) || self.files[file as usize].glyph(letter) != 0));
            let first = letters.chars().next().unwrap_or(' ');
            let file = match (keeps, first.is_control()) {
                (_, true) => None,
                (true, false) => current.and_then(|run| run.file),
                (false, false) => self.file_for(face, first),
            };
            let goes_on = runs.last().is_some_and(|run| run.bytes.end == at && run.file == file && run.leftward == leftward && count < LONGEST - 64 && !apart(run));
            match runs.last_mut().filter(|_| goes_on) {
                Some(run) => (run.bytes.end, run.script) = (at + size, run.script.or(script)),
                None => (count, _) = (0, runs.push(Run { bytes: at..at + size, file, leftward, script })),
            }
            (at, count) = (at + size, count + letters.chars().count());
        }
    }

    /// The font for a character in a face: the app's font for the face, where it has the
    /// character; else a system font that has it (looked up once); else the face's own font,
    /// showing its notdef. (Zero for the face's own font.)
    fn file_for(&mut self, face: Face, letter: char) -> Option<u32> {
        let has = |files: &[FontFile], file: u32| letter == '\0' || unseen(letter) || files[file as usize].glyph(letter) != 0;
        let own = self.own.iter().find(|(known, _)| *known == face).map(|(_, file)| *file);
        if let Some(file) = own.filter(|file| has(&self.files, *file)) {
            return Some(file);
        }
        let found = self.found.iter().filter(|(known, ..)| *known == face).find_map(|(.., file)| file.filter(|file| has(&self.files, *file)));
        if found.is_some() {
            return found;
        }
        // (The face's own font first, which a character no font has shows in. Where the system
        // would draw any character in that font anyway, only it is looked up.)
        let main = self.looked(face, '\0');
        let letter = if cfg!(windows) && !cfg!(feature = "fallback-fonts") { '\0' } else { letter };
        let file = self.looked(face, letter);
        file.filter(|file| has(&self.files, *file)).or(own).or(main).or(file)
    }

    /// The system font for a character in a face (the face's own, for zero): as found before,
    /// or looked up now (and kept once), while lookups remain.
    fn looked(&mut self, face: Face, letter: char) -> Option<u32> {
        if let Some((.., file)) = self.found.iter().find(|(known, of, _)| *known == face && *of == letter) {
            return *file;
        }
        if self.found.len() >= LOOKS {
            return None;
        }
        let found = crate::system_fonts::file_for(face, if letter == '\0' { 'a' } else { letter });
        let same = |kept: &FontFile, file: &FontFile| kept.start == file.start && kept.varied.is_none() && kept.bytes().0 == file.bytes().0;
        let at = found.map(|file| match self.files.iter().position(|kept| same(kept, &file)) {
            Some(at) => at as u32,
            None => {
                self.files.push(file);
                self.files.len() as u32 - 1
            }
        });
        self.found.push((face, letter, at));
        at
    }

    // ---- drawing a page ----

    /// Draws page `page` on a canvas, as a viewer would draw the file: canvas units are page
    /// units. (What printing sends a printer that takes pixels.)
    pub fn draw(&self, page: usize, canvas: &mut Canvas) {
        let Some(sheet) = self.pages.get(page) else { return };
        let mut held = Vec::new();
        for mark in &sheet.marks {
            match mark {
                Mark::Shape(drawn) => drawn.draw(canvas),
                Mark::Words(words) => self.draw_words(words, canvas),
                Mark::Picture(at, r) => self.draw_picture(*at, *r, canvas),
                Mark::Clip(r) => held.push(canvas.clip(*r)),
                Mark::Unclip => held.pop().into_iter().for_each(|before| canvas.restore_clip(before)),
                Mark::Link(..) => {}
            }
        }
    }

    /// Glyphs, each filled from its outline.
    fn draw_words(&self, words: &Words, canvas: &mut Canvas) {
        let file = &self.files[words.font as usize];
        let unit = file.unit(words.size);
        for glyph in &words.glyphs {
            let (path, _) = file.outline_through(glyph.id, [unit, 0.0, 0.0, -unit, glyph.at.x, glyph.at.y]);
            canvas.fill(&path, pt(0.0, 0.0), 1.0, words.color);
        }
    }

    /// A picture stretched over `r`: a JPEG decoded first.
    fn draw_picture(&self, at: u32, r: Rect, canvas: &mut Canvas) {
        match &self.pictures[at as usize] {
            Picture::Pixels { width, pixels } => canvas.picture(pixels, *width, r),
            Picture::Jpeg { file, .. } => self.unpack.and_then(|unpack| unpack(file)).into_iter().for_each(|(width, pixels)| canvas.picture(&pixels, width, r)),
        }
    }

    /// All of page `page` as an image, `scale` pixels per unit, on white: each pixel
    /// `0xAARRGGBB`. Drawn a strip at a time.
    pub fn render_page(&self, page: usize, scale: f32) -> (usize, Vec<u32>) {
        let size = self.size(page);
        let (width, height) = ((size.w * scale).round().max(1.0) as usize, (size.h * scale).round().max(1.0) as usize);
        let mut pixels = vec![0xffff_ffff; width * height];
        for top in (0..height).step_by(256) {
            let rows = 256.min(height - top);
            let block = &mut pixels[top * width..(top + rows) * width];
            self.draw(page, &mut Canvas::new(block, width, rows, (0, top as i32), scale));
        }
        (width, pixels)
    }
}

impl Drawn {
    /// Draws it on a canvas in page units, as a window draws an app shape.
    fn draw(&self, canvas: &mut Canvas) {
        let pen = self.pen.map(|pen| Pen { dashes: &self.dashes, ..pen });
        let paint = match &self.paint {
            PagePaint::Color(color) => Paint::Color(*color),
            PagePaint::Linear(turn, stops) => Paint::Linear(*turn, stops),
            PagePaint::Radial(stops) => Paint::Radial(stops),
        };
        let mut inked = Inked::default();
        inked.set(&self.path, pen, paint, self.full);
        inked.draw(canvas, pt(0.0, 0.0));
    }
}

/// Whether a character is invisible: a joiner, or a variation selector. Such a character joins
/// its run.
fn unseen(letter: char) -> bool {
    matches!(letter, '\u{200b}'..='\u{200f}' | '\u{2060}'..='\u{2064}' | '\u{fe00}'..='\u{fe0f}' | '\u{e0000}'..='\u{e007f}' | '\u{e0100}'..='\u{e01ef}')
}

/// A canvas pixel (premultiplied) as a picture takes it: unpremultiplied.
fn unmixed(pixel: u32) -> u32 {
    let a = pixel >> 24;
    match a {
        0 => 0,
        255 => pixel,
        _ => {
            let part = |shift: u32| (((pixel >> shift & 0xff) * 255 + a / 2) / a).min(255) << shift;
            a << 24 | part(16) | part(8) | part(0)
        }
    }
}

/// A JPEG file as embeddable in PDF verbatim: 8-bit samples, baseline or progressive (decodable
/// by every viewer), with one, three, or four components (CMYK as Adobe writes it). None for
/// anything else.
pub(crate) fn passed(file: &[u8]) -> Option<Picture> {
    let mut rest = file.strip_prefix(&[0xFF, 0xD8])?;
    let (mut frame, mut adobe, mut turn) = (None, false, 1);
    while let Some((kind, body, after)) = part(rest) {
        rest = after;
        match kind {
            0xC0..=0xC2 => frame = Some(body),
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA | 0xD9 => break,
            0xE1 => turn = crate::exif_orientation::turn(body).unwrap_or(turn),
            0xEE => adobe |= body.starts_with(b"Adobe"),
            _ => {}
        }
    }
    // (A frame header gives the sample precision, height, width, and component count.)
    let frame = frame?;
    let (depth, high, wide, planes) = (*frame.first()?, u16::from_be_bytes([*frame.get(1)?, *frame.get(2)?]), u16::from_be_bytes([*frame.get(3)?, *frame.get(4)?]), *frame.get(5)?);
    let fits = depth == 8 && high > 0 && wide > 0 && (planes == 1 || planes == 3 || (planes == 4 && adobe)) && (1..=8).contains(&turn);
    let size = if turn >= 5 { [high as usize, wide as usize] } else { [wide as usize, high as usize] };
    fits.then(|| Picture::Jpeg { file: file.to_vec(), size, planes, adobe, turn })
}

/// The next JPEG segment: its marker, its content, and the file after it.
fn part(rest: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let at = rest.windows(2).position(|pair| pair[0] == 0xFF && pair[1] != 0xFF && pair[1] != 0)?;
    let (kind, rest) = (rest[at + 1], &rest[at + 2..]);
    if matches!(kind, 0x01 | 0xD0..=0xD9) {
        return Some((kind, &[], rest));
    }
    let long = u16::from_be_bytes([*rest.first()?, *rest.get(1)?]) as usize;
    Some((kind, rest.get(2..long)?, rest.get(long..)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::latin;

    /// A paper whose regular text uses an installed system font.
    fn paper() -> Option<Paper> {
        let mut paper = Paper::new();
        paper.font(Face::Regular, latin()?);
        Some(paper)
    }

    #[test]
    fn text_is_as_wide_as_it_measures_and_stands_on_its_line() {
        let Some(mut paper) = paper() else { return eprintln!("no font file here: not tested") };
        let font = Font::new(20.0);
        let said = "Kerning: AV To Wa, fi \u{c5}ngstr\u{f6}m";
        let wide = paper.measure(said, font);
        assert!(wide > 200.0, "{wide}");
        assert_eq!(paper.text(said, 10.0, 30.0, font, Color::BLACK), wide);
        let line = 30.0 + paper.metrics(font).ascent;
        let Some(Mark::Words(words)) = paper.pages[0].marks.first() else { panic!("words were written") };
        assert!(words.glyphs.iter().all(|glyph| glyph.at.y == line), "every glyph on the line");
        assert!(words.glyphs.windows(2).all(|pair| pair[0].at.x < pair[1].at.x), "from left to right");
        let mut stops = Vec::new();
        assert_eq!(paper.stops(said, font, &mut stops), wide);
        assert_eq!((stops.len(), stops.last().copied()), (said.chars().count(), Some(wide)));
    }

    #[test]
    fn a_paragraph_breaks_into_rows_no_wider_than_its_room() {
        let Some(mut paper) = paper() else { return eprintln!("no font file here: not tested") };
        let font = Font::new(14.0);
        let said = "Words that go on for a while break into rows, at the spaces between them, no wider than the room.\nAnd a line end starts a row of its own.";
        let high = paper.paragraph(said, rect(20.0, 20.0, 200.0, 0.0), font, Color::BLACK);
        let rows = (high / 21.0).round() as usize;
        assert!(rows >= 4 && high == rows as f32 * 21.0, "{high}");
        for mark in &paper.pages[0].marks {
            let Mark::Words(words) = mark else { continue };
            let last = words.glyphs.last().map_or(0.0, |glyph| glyph.at.x + paper.files[words.font as usize].width(glyph.id) * paper.files[words.font as usize].unit(14.0));
            assert!(last <= 220.5 && words.glyphs[0].at.x >= 20.0, "a row from 20 to {last}");
        }
    }

    #[test]
    fn what_is_not_seen_takes_no_room_and_a_letter_a_font_lacks_comes_from_another() {
        let Some(mut paper) = paper() else { return eprintln!("no font file here: not tested") };
        let font = Font::new(16.0);
        // (A newline takes no width, and ends its run.)
        assert_eq!(paper.measure("a\nb", font), paper.measure("a", font) + paper.measure("b", font));
        paper.text("Latin, then \u{4e2d}\u{6587}", 0.0, 0.0, font, Color::BLACK);
        let fonts: Vec<u32> = paper.pages[0].marks.iter().filter_map(|mark| if let Mark::Words(words) = mark { Some(words.font) } else { None }).collect();
        // (Where the system has a font with Chinese characters, they use it.)
        let written_in = |letter: char| paper.files.iter().position(|file| file.glyph(letter) != 0);
        match written_in('\u{4e2d}') {
            Some(_) => assert!(fonts.len() == 2 && paper.files[fonts[1] as usize].glyph('\u{4e2d}') != 0, "{fonts:?}"),
            None => eprintln!("no font with Chinese letters here: written in the box of the face's own"),
        }
    }

    #[test]
    fn a_page_is_drawn_on_a_canvas_with_its_clip() {
        let mut paper = Paper::new();
        paper.page(Size { w: 40.0, h: 20.0 });
        paper.clip(rect(0.0, 0.0, 20.0, 20.0));
        paper.rect(rect(0.0, 0.0, 40.0, 20.0), Color::rgb(0x112233));
        paper.unclip();
        paper.rect(rect(30.0, 0.0, 10.0, 20.0), Color::rgb(0x445566));
        let (width, pixels) = paper.render_page(0, 1.0);
        assert_eq!((width, pixels.len()), (40, 800));
        let at = |x: usize, y: usize| pixels[y * 40 + x] & 0xff_ffff;
        assert_eq!([at(5, 5), at(25, 5), at(35, 5)], [0x112233, 0xff_ffff, 0x445566]);
    }

    /// The page's MediaBox as the file states it: `[0 0 w h]`.
    fn media_box(file: &[u8]) -> Vec<f32> {
        let file = String::from_utf8_lossy(file);
        let at = file.find("/MediaBox [").expect("a page") + "/MediaBox [".len();
        file[at..].split(']').next().unwrap().split_whitespace().map(|number| number.parse().unwrap()).collect()
    }

    #[test]
    fn a_page_of_endless_size_is_one_a_reader_shows_and_one_that_can_be_drawn() {
        // A PDF page is at most 14,400 points a side: a larger or infinite page is clamped to
        // that.
        let mut paper = Paper::new();
        paper.page(Size { w: f32::INFINITY, h: 1e30 });
        paper.rect(rect(10.0, 10.0, 100.0, 100.0), Color::BLACK);
        let sides = media_box(&paper.to_pdf());
        assert!(sides[2] >= 3.0 && sides[3] >= 3.0 && sides[2] <= 14_400.0 && sides[3] <= 14_400.0, "{sides:?}");
        assert!(paper.size(0).w.is_finite() && paper.size(0).h.is_finite(), "{:?}", paper.size(0));
        // Drawn small, as printing draws a page larger than the paper.
        let (width, pixels) = paper.render_page(0, 0.01);
        assert!(width > 0 && pixels.len() < 1 << 24, "{width} wide, {} pixels", pixels.len());
    }

    #[test]
    fn a_page_with_a_clip_and_a_picture_of_any_size_is_drawn() {
        // Rectangles of any size are clamped to the page when drawn.
        let mut paper = Paper::new();
        paper.page(Size { w: 40.0, h: 30.0 });
        paper.clip(rect(-1e30, -1e30, 2e30, 2e30));
        paper.picture(1, &[0xff11_2233], rect(-1e30, -1e30, 2e30, 2e30));
        paper.unclip();
        paper.picture(1, &[0xff44_5566], rect(f32::NAN, 0.0, 1e38, 10.0));
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| paper.render_page(0, 1.0)));
        let (_, pixels) = drawn.expect("the page is drawn");
        assert_eq!(pixels[20 * 40 + 20] & 0xff_ffff, 0x11_2233, "the picture over all of it shows");
    }
}
