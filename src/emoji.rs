//! Colour emoji in any text.
//!
//! No text system here draws them: the system engines draw their outlines in the text colour,
//! or nothing. So [`Emoji`] wraps any text system. Text without emoji passes straight through,
//! at the cost of a scan of its bytes. A line with some is split: plain runs still go to the
//! inner system, and each emoji is drawn here from an emoji font that `font_file.rs` reads.
//! That font is the system's (or one the app supplies), looked up only when the first emoji
//! appears: a program that shows none never opens it.
//!
//! An emoji is often several characters: a flag is two letters, a family is people with
//! joiners, a hand has a skin tone after it. Which characters belong together follows the
//! grapheme cluster rule in `text.rs`; the font decides how it looks, showing the parts side by
//! side where it lacks one glyph for a sequence. Everywhere else a sequence is one thing: one
//! width, the cursor before or after it, no line break inside.
//!
//! An emoji is as large as the surrounding text (the font's own size for it) and sits on its
//! baseline. It is rasterized once per size into a cached bitmap. The text colour does not tint
//! it; the colour's alpha fades it.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::{Bitmap, FontFile, Set};
use crate::geom::rect;
use crate::png;
use crate::text::{Font, Metrics, TextSystem, among, cluster, pictured};
use std::ops::Range;

/// Draws a glyph from COLR v1 or SVG: `colr::drawn`.
type Full = fn(&FontFile, u16, f32, [f32; 2], Color) -> Option<crate::colr::Drawn>;

/// Approximate cache size in bytes. Beyond it all are dropped and redrawn as needed.
const CACHE_LIMIT: usize = 4 << 20;
/// Bitmaps with more pixels than this are not drawn: no text is that large.
const LARGEST: usize = 1 << 22;

/// Ranges (first and last) of characters shown as emoji by default: Unicode's
/// `Emoji_Presentation`. Other emoji-capable characters (a heart, a sun, ©) are text, and emoji
/// only when followed by U+FE0F.
#[rustfmt::skip]
const SHOWN: [(u32, u32); 80] = [
    (0x231a, 0x231b), (0x23e9, 0x23ec), (0x23f0, 0x23f0), (0x23f3, 0x23f3), (0x25fd, 0x25fe), (0x2614, 0x2615), (0x2648, 0x2653), (0x267f, 0x267f), (0x2693, 0x2693), (0x26a1, 0x26a1),
    (0x26aa, 0x26ab), (0x26bd, 0x26be), (0x26c4, 0x26c5), (0x26ce, 0x26ce), (0x26d4, 0x26d4), (0x26ea, 0x26ea), (0x26f2, 0x26f3), (0x26f5, 0x26f5), (0x26fa, 0x26fa), (0x26fd, 0x26fd),
    (0x2705, 0x2705), (0x270a, 0x270b), (0x2728, 0x2728), (0x274c, 0x274c), (0x274e, 0x274e), (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797), (0x27b0, 0x27b0), (0x27bf, 0x27bf),
    (0x2b1b, 0x2b1c), (0x2b50, 0x2b50), (0x2b55, 0x2b55), (0x1f004, 0x1f004), (0x1f0cf, 0x1f0cf), (0x1f18e, 0x1f18e), (0x1f191, 0x1f19a), (0x1f1e6, 0x1f1ff), (0x1f201, 0x1f201), (0x1f21a, 0x1f21a),
    (0x1f22f, 0x1f22f), (0x1f232, 0x1f236), (0x1f238, 0x1f23a), (0x1f250, 0x1f251), (0x1f300, 0x1f320), (0x1f32d, 0x1f335), (0x1f337, 0x1f37c), (0x1f37e, 0x1f393), (0x1f3a0, 0x1f3ca), (0x1f3cf, 0x1f3d3),
    (0x1f3e0, 0x1f3f0), (0x1f3f4, 0x1f3f4), (0x1f3f8, 0x1f43e), (0x1f440, 0x1f440), (0x1f442, 0x1f4fc), (0x1f4ff, 0x1f53d), (0x1f54b, 0x1f54e), (0x1f550, 0x1f567), (0x1f57a, 0x1f57a), (0x1f595, 0x1f596),
    (0x1f5a4, 0x1f5a4), (0x1f5fb, 0x1f64f), (0x1f680, 0x1f6c5), (0x1f6cc, 0x1f6cc), (0x1f6d0, 0x1f6d2), (0x1f6d5, 0x1f6d9), (0x1f6dc, 0x1f6df), (0x1f6eb, 0x1f6ec), (0x1f6f4, 0x1f6fc), (0x1f7e0, 0x1f7eb),
    (0x1f7f0, 0x1f7f0), (0x1f90c, 0x1f93a), (0x1f93c, 0x1f945), (0x1f947, 0x1f9ff), (0x1fa70, 0x1fa7c), (0x1fa80, 0x1fac6), (0x1fac8, 0x1fac8), (0x1facc, 0x1fadd), (0x1fadf, 0x1faeb), (0x1faef, 0x1fafa),
];

/// Whether a text has no emoji, from its bytes alone. Nearly all text has none, so this is all
/// it usually costs. A UTF-8 lead byte says roughly where a character lies: emoji start with
/// one of a few high bytes, as do the characters that make plain ones emoji (the keycap mark,
/// U+FE0F). The dashes, ellipsis and quotes sharing the lowest of those lead bytes are told
/// apart by the next two bytes; CJK, lying between, has other lead bytes.
pub(crate) fn plain(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Eight bytes at a time first: in most text none is as high as the lowest of those. (A byte
    // is when its top bit is set and the rest is at least so much: adding carries into the top
    // bit exactly then.)
    let (eights, rest) = bytes.as_chunks::<8>();
    let high = |eight: &[u8; 8]| {
        let eight = u64::from_ne_bytes(*eight);
        ((eight & 0x7f7f_7f7f_7f7f_7f7f) + 0x1e1e_1e1e_1e1e_1e1e) & eight & 0x8080_8080_8080_8080 != 0
    };
    if !eights.iter().any(high) && rest.iter().all(|byte| *byte < 0xe2) {
        return true;
    }
    let next = |at: usize| bytes.get(at).copied().unwrap_or(0);
    let pictured = |at: usize| match bytes[at] {
        ..0xe2 | 0xe4..=0xee => false,
        0xe2 => next(at + 1) != 0x80 || next(at + 2) >= 0xbc,
        0xe3 => matches!((next(at + 1), next(at + 2)), (0x80, 0xb0 | 0xbd) | (0x8a, 0x97 | 0x99)),
        0xef => next(at + 1) == 0xb8,
        _ => true,
    };
    !(0..bytes.len()).any(pictured)
}

/// The byte length of `text`'s first character (with what belongs to it) if it shows as an
/// emoji; 0 if it is plain text. (`first` is that character, which the caller has at hand.)
pub(crate) fn picture(first: char, text: &str) -> usize {
    // Nearly every character fails here at once: emoji are few, in a few ranges. (A digit is
    // one only as a keycap: with the keycap mark after it, whose lead byte is a high one.)
    let key = matches!(first, '#' | '*' | '0'..='9');
    let may = match first {
        '#' | '*' | '0'..='9' => text.as_bytes().get(1).is_some_and(|byte| *byte >= 0xe2),
        '\u{a9}' | '\u{ae}' => true,
        _ => first >= '\u{203c}' && !('\u{329a}'..'\u{1f000}').contains(&first) && pictured(first),
    };
    if !may {
        return 0;
    }
    let long = cluster(text);
    let shown = match text[first.len_utf8()..].chars().next() {
        _ if key => text[..long].ends_with('\u{20e3}'),
        // The next character may ask for text (U+FE0E) or emoji (U+FE0F) presentation; a skin
        // tone or a joiner after it makes it an emoji too.
        Some('\u{fe0e}') => false,
        Some('\u{fe0f}' | '\u{200d}' | '\u{1f3fb}'..='\u{1f3ff}') => true,
        _ => among(&SHOWN, first),
    };
    if shown { long } else { 0 }
}

/// Where emoji come from.
enum Source {
    /// The system's emoji font, not looked up yet.
    Unasked,
    /// No font: the system has none. Emoji are drawn as text.
    Nowhere,
    Font(Box<FontFile>),
}

/// One emoji sequence at one size.
struct Met {
    /// Its advance, in whole pixels.
    wide: f32,
    /// Its bitmap, once drawn.
    drawn: Option<Picture>,
}

/// A cached bitmap: its top left pixel's offset from the pen on the baseline (right and down),
/// its width, and its pixels as a canvas stores them.
struct Picture {
    at: [i32; 2],
    width: usize,
    pixels: Vec<u32>,
    /// The text colour it was drawn for, if any part uses it (a font may give a glyph or a
    /// layer the text colour): redrawn for text of another colour.
    ink: Option<Color>,
}

/// A layer of an emoji, before compositing.
enum Part {
    /// An outline: its block's corner, width, per-pixel coverage, and fill colour.
    Outline([i32; 2], usize, Vec<u8>, Color),
    /// A bitmap glyph: width, pixels, and the rect it fills (left, top, right, bottom).
    Ready(usize, Vec<u32>, [i32; 4]),
    /// A glyph drawn by `colr.rs`: its top left pixel, width, and pixels as a canvas stores
    /// them.
    Laid([i32; 2], usize, Vec<u32>),
}

impl Part {
    /// The pixels it covers: left, top, right, bottom.
    fn reach(&self) -> [i32; 4] {
        match self {
            Part::Outline([x, y], width, cover, _) => [*x, *y, x + *width as i32, y + (cover.len() / (*width).max(1)) as i32],
            Part::Ready(.., place) => *place,
            Part::Laid([x, y], width, pixels) => [*x, *y, x + *width as i32, y + (pixels.len() / (*width).max(1)) as i32],
        }
    }
}

/// Colour emoji: a text system wrapping another, which draws everything else. With the feature
/// `emoji`, a [`Ui`](crate::Ui) wraps its text system in one by itself; an app that supplies
/// its own text system ([`Ui::set_text_system`](crate::Ui::set_text_system)) wraps it the same
/// way to keep emoji: [`Emoji::around(Box::new(own))`](Emoji::around).
pub struct Emoji {
    text: Box<dyn TextSystem>,
    source: Source,
    /// The COLR v1 / SVG renderer, if the program asked for it (see `colr.rs`).
    full: Option<Full>,
    /// Sequences met so far, each at one pixel size (its bits), sorted by both: with width and
    /// bitmap, or `None` if the font lacks it (then drawn as text).
    met: Vec<(u32, Box<str>, Option<Met>)>,
    /// Approximate bytes all of that takes.
    cache_bytes: usize,
    // Kept between lines only to reuse their memory: a line's pieces (byte range, and an
    // emoji's width), a sequence's glyphs, and a faded bitmap.
    pieces: Vec<(Range<usize>, Option<f32>)>,
    glyphs: Vec<Set>,
    pale: Vec<u32>,
}

/// The text system for all text: `text` wrapped in [`Emoji`]; or, without the feature `emoji`,
/// `text` alone, which draws emoji as any other letter. (What users type cannot be told from
/// the calls: a program that wants this enables the feature.)
pub(crate) fn in_colour(text: Box<dyn TextSystem>) -> Box<dyn TextSystem> {
    match cfg!(feature = "emoji") {
        true => Box::new(Emoji::around(text)),
        false => text,
    }
}

impl std::fmt::Debug for Emoji {
    /// Whether it found an emoji font, whether it draws COLR v1 and SVG, and how many sequences
    /// it has measured.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let font = matches!(self.source, Source::Font(_));
        f.debug_struct("Emoji").field("font", &font).field("in_full", &self.full.is_some()).field("measured", &self.met.len()).finish_non_exhaustive()
    }
}

impl Emoji {
    /// Emoji from the system's emoji font, around a text system that draws the rest. (Without
    /// such a font, the inner system draws everything.)
    pub fn around(text: Box<dyn TextSystem>) -> Self {
        Emoji { text, source: Source::Unasked, full: None, met: Vec::new(), cache_bytes: 0, pieces: Vec::new(), glyphs: Vec::new(), pale: Vec::new() }
    }

    /// Emoji from this font, supplied by the app: the same on every system, in every format the
    /// font has (see [`in_full`](Self::in_full)). ([`Ui::set_emoji`](crate::Ui::set_emoji) does
    /// this for a [`Ui`](crate::Ui).)
    pub fn with(self, font: FontFile) -> Self {
        Emoji { source: Source::Font(Box::new(font)), ..self }.in_full()
    }

    /// Draws every emoji format: also COLR v1 (gradients, transforms, compositing) and SVG.
    /// Without this, COLR v0 layers and bitmaps are drawn, and a font with neither is not used.
    /// The code for the newer formats is only in programs that ask for it.
    /// ([`Ui::emoji_in_full`](crate::Ui::emoji_in_full) does this for a [`Ui`](crate::Ui).)
    pub fn in_full(self) -> Self {
        let source = match self.source {
            Source::Nowhere => Source::Unasked,
            source => source,
        };
        Emoji { source, full: Some(crate::colr::drawn), met: Vec::new(), cache_bytes: 0, ..self }
    }

    /// A sequence's width at a size, in pixels, if the font has it. The first time, the font is
    /// asked for its glyphs (and the very first time, the font is found).
    fn wide(&mut self, sequence: &str, pixels: f32) -> Option<f32> {
        let Emoji { source, met, cache_bytes, glyphs, full, .. } = self;
        let at = match place(met, sequence, pixels) {
            Ok(at) => return met[at].2.as_ref().map(|met| met.wide),
            Err(at) => at,
        };
        if let Source::Unasked = source {
            *source = system(full.is_some()).map_or(Source::Nowhere, |font| Source::Font(Box::new(font)));
        }
        let Source::Font(font) = source else { return None };
        let wide = font.set(sequence, glyphs).map(|wide| (wide * font.unit(pixels)).round());
        *cache_bytes += sequence.len() + size_of::<(u32, Box<str>, Option<Met>)>();
        met.insert(at, (pixels.to_bits(), sequence.into(), wide.map(|wide| Met { wide, drawn: None })));
        wide
    }

    /// Splits a line into pieces: each emoji alone, with its width, and the plain runs between.
    /// Returns whether there is an emoji.
    fn cut(&mut self, text: &str, pixels: f32) -> bool {
        if self.cache_bytes > CACHE_LIMIT {
            self.met.clear();
            self.cache_bytes = 0;
        }
        self.pieces.clear();
        // Sizes of zero or NaN have no emoji.
        let sized = pixels > 0.0 && pixels < 4096.0;
        // `words` is where the plain run not yet in a piece starts; `next` where the emoji
        // being scanned ends.
        let (mut words, mut next) = (0, 0);
        for (at, letter) in text.char_indices().filter(|_| sized) {
            let long = if at < next { 0 } else { picture(letter, &text[at..]) };
            let wide = if long == 0 { None } else { self.wide(&text[at..at + long], pixels) };
            next = next.max(at + long);
            if let Some(wide) = wide {
                let two = [(words..at, None), (at..next, Some(wide))];
                self.pieces.extend(two.into_iter().filter(|(piece, _)| !piece.is_empty()));
                words = next;
            }
        }
        self.pieces.extend(Some((words..text.len(), None)).filter(|(piece, _)| !piece.is_empty()));
        words > 0
    }

    /// Draws a sequence's bitmap with the pen at `x` on baseline `line`, in pixels, at
    /// `color`'s alpha. Rasterized first if never drawn, or drawn for another text colour it
    /// uses.
    fn lay(&mut self, canvas: &mut Canvas, sequence: &str, pixels: f32, [x, line]: [i32; 2], color: Color) {
        let Emoji { source: Source::Font(font), met, cache_bytes, glyphs, pale, full, .. } = self else { return };
        let Some((.., Some(met))) = place(met, sequence, pixels).ok().map(|at| &mut met[at]) else { return };
        let ink = color.solid();
        if met.drawn.as_ref().is_none_or(|drawn| drawn.ink.is_some_and(|was| was != ink)) {
            let _ = font.set(sequence, glyphs);
            *cache_bytes -= met.drawn.as_ref().map_or(0, |drawn| drawn.pixels.len() * 4);
            let drawn = met.drawn.insert(painted(font, glyphs, pixels, ink, *full));
            *cache_bytes += drawn.pixels.len() * 4;
        }
        let Some(drawn) = met.drawn.as_ref().filter(|drawn| drawn.width > 0) else { return };
        let (at, solid) = ([x + drawn.at[0], line + drawn.at[1]], color.a() as u32);
        if solid == 255 {
            return canvas.pixels(&drawn.pixels, drawn.width, at, false);
        }
        // Pixels are premultiplied: each channel times the text's alpha fades it that much.
        let paler = |pixel: &u32| (0..4).fold(0, |paler, part| paler | ((((pixel >> (part * 8)) & 255) * solid + 127) / 255) << (part * 8));
        pale.clear();
        pale.extend(drawn.pixels.iter().map(paler));
        canvas.pixels(pale, drawn.width, at, false);
    }
}

/// A sequence at a size among those met: its index, or where it belongs.
fn place(met: &[(u32, Box<str>, Option<Met>)], sequence: &str, pixels: f32) -> Result<usize, usize> {
    met.binary_search_by(|(size, known, _)| (*size, &**known).cmp(&(pixels.to_bits(), sequence)))
}

/// Draws a sequence's glyphs for text `pixels` high into one bitmap. Each glyph is drawn as its
/// font has it: COLR v0 layers, each an outline in one colour; a scaled bitmap; or a plain
/// outline in the text colour (how emoji fonts carry ordinary letters). With `full`, COLR v1
/// and SVG come first (see `colr.rs`).
fn painted(font: &FontFile, glyphs: &[Set], pixels: f32, ink: Color, full: Option<Full>) -> Picture {
    let (unit, mut parts, mut inked) = (font.unit(pixels), Vec::new(), false);
    for set in glyphs {
        let pen = [set.at[0] * unit, set.at[1] * unit];
        let mut outline = |glyph: u16, colour: Option<Color>| {
            let (at, width, cover) = font.cover(glyph, unit, pen);
            inked |= colour.is_none();
            parts.push(Part::Outline(at, width, cover, colour.unwrap_or(ink)));
        };
        if let Some(drawn) = full.and_then(|full| full(font, set.glyph, unit, pen, ink)) {
            inked |= drawn.inked;
            parts.push(Part::Laid(drawn.at, drawn.width, drawn.pixels));
            continue;
        }
        match (font.layers(set.glyph), font.bitmap(set.glyph, pixels)) {
            (Some(layers), _) => layers.for_each(|(glyph, colour)| outline(glyph, colour)),
            (None, Some(ready)) => parts.extend(fitted(ready, pixels, pen)),
            (None, None) => outline(set.glyph, None),
        }
    }
    // The bitmap spans all layers, composited in order.
    let reach = parts.iter().map(Part::reach).filter(|[left, top, right, bottom]| left < right && top < bottom);
    let around = reach.reduce(|[left, top, right, bottom], [l, t, r, b]| [left.min(l), top.min(t), right.max(r), bottom.max(b)]);
    let [left, top, right, bottom] = around.unwrap_or([0; 4]);
    let (width, height) = ((right - left) as usize, (bottom - top) as usize);
    if width * height == 0 || width * height > LARGEST {
        return Picture { at: [0; 2], width: 0, pixels: Vec::new(), ink: None };
    }
    let mut block = vec![0u32; width * height];
    let mut canvas = Canvas::new(&mut block, width, height, (left, top), 1.0);
    for part in &parts {
        match part {
            Part::Outline([x, y], width, cover, colour) => canvas.cover(*x, *y, *width, cover, *colour),
            Part::Ready(width, pixels, [l, t, r, b]) => canvas.picture(pixels, *width, rect(*l as f32, *t as f32, (r - l) as f32, (b - t) as f32)),
            Part::Laid(at, width, pixels) => canvas.pixels(pixels, *width, *at, false),
        }
    }
    Picture { at: [left, top], width, pixels: block, ink: inked.then_some(ink) }
}

/// A font's bitmap of a glyph, placed for text `pixels` high with the pen `pen` pixels right of
/// the start and above the baseline: it was made for one size, and scales with the text. `None`
/// if unreadable.
fn fitted(ready: Bitmap, pixels: f32, pen: [f32; 2]) -> Option<Part> {
    let (width, picture) = png::decode(ready.png)?;
    let (by, height) = (pixels / ready.pixels, picture.len() / width);
    // Corner and size are each rounded to whole pixels (not the four edges: a square stays
    // square), and at least one pixel either way.
    let (left, top) = ((pen[0] + ready.left * by).round(), (-pen[1] - ready.top * by).round());
    let place = [left, top, left + (width as f32 * by).round().max(1.0), top + (height as f32 * by).round().max(1.0)];
    // A placement beyond all pixels means a damaged font: no bitmap.
    place.iter().all(|edge| edge.abs() < 1e6).then(|| Part::Ready(width, picture, place.map(|edge| edge as i32)))
}

impl TextSystem for Emoji {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.text.metrics(font, scale)
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        if plain(text) || !self.cut(text, font.size * scale) {
            return self.text.measure(text, font, scale, stops);
        }
        // The pen advances in whole pixels. Each character's end is from the line's start; all
        // characters of an emoji end where it ends.
        let mut pen = 0.0;
        for (piece, wide) in &self.pieces {
            let (from, ends) = (stops.as_ref().map_or(0, |stops| stops.len()), pen / scale);
            let wide = match wide {
                Some(wide) => *wide,
                None => (self.text.measure(&text[piece.clone()], font, scale, stops.as_deref_mut()) * scale).round(),
            };
            pen += wide;
            let Some(stops) = stops.as_deref_mut() else { continue };
            stops[from..].iter_mut().for_each(|stop| *stop += ends);
            stops.extend(text[piece.clone()].chars().skip(stops.len() - from).map(|_| pen / scale));
            // A piece ends where the pen is after it, exactly: the line's last character ends
            // at the line's width.
            stops.last_mut().into_iter().for_each(|last| *last = pen / scale);
        }
        pen / scale
    }

    fn file(&mut self, face: crate::text::Face, letter: char) -> Option<&FontFile> {
        self.text.file(face, letter)
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        if plain(text) || !self.cut(text, font.size * scale) {
            return self.text.draw(canvas, text, x, y, font, color);
        }
        let metrics = self.text.metrics(font, scale);
        // Everything sits on the inner system's baseline, and an emoji stays within its line:
        // its own width, and the line's height.
        let (left, line) = ((x * scale).round(), ((y * scale).round() + (metrics.ascent * scale).round()) as i32);
        let (mut pen, pieces) = (0.0, std::mem::take(&mut self.pieces));
        for (piece, wide) in &pieces {
            let (piece, from) = (&text[piece.clone()], (left + pen) / scale);
            let Some(wide) = wide else {
                self.text.draw(canvas, piece, from, y, font, color);
                pen += (self.text.measure(piece, font, scale, None) * scale).round();
                continue;
            };
            let room = canvas.clip(rect(from, y, wide / scale, metrics.height()));
            self.lay(canvas, piece, font.size * scale, [(left + pen) as i32, line], color);
            canvas.restore_clip(room);
            pen += wide;
        }
        self.pieces = pieces;
    }
}

/// The system's emoji font, if any: on Windows, always in the same place. (With `full`, any
/// colour format will do.)
#[cfg(windows)]
fn system(full: bool) -> Option<FontFile> {
    let windows = std::env::var_os("WINDIR")?;
    FontFile::open(std::path::Path::new(&windows).join("Fonts").join("seguiemj.ttf")).filter(|font| font.pictured() || full && font.pictured_newer())
}

/// The system's emoji font, if any: elsewhere, the one Fontconfig names (known where system
/// text is in use), or on macOS the one CoreText draws in, if it has colour glyphs and a
/// smiling face; else the first such font in the font folders. User fonts come before system
/// ones, and a folder's files before its subfolders.
/// A page has no font files of the system's: the browser's own text draws emoji.
#[cfg(target_family = "wasm")]
fn system(_full: bool) -> Option<FontFile> {
    None
}

#[cfg(not(any(windows, target_family = "wasm")))]
fn system(full: bool) -> Option<FontFile> {
    use std::path::PathBuf;
    let smiles = |font: &FontFile| (font.pictured() || full && font.pictured_newer()) && font.set("\u{1f600}", &mut Vec::new()).is_some();
    #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
    let named = crate::x11::emoji_file().and_then(FontFile::open).filter(smiles);
    #[cfg(target_os = "android")]
    let named = crate::android_text::emoji_file().filter(smiles);
    #[cfg(target_vendor = "apple")]
    let named = crate::core_text::emoji_file().filter(smiles);
    if named.is_some() {
        return named;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let own = std::env::var_os("XDG_DATA_HOME").map_or(home.join(".local/share"), PathBuf::from).join("fonts");
    // The last folder is visited next: listed last to first. No more folders are visited than
    // any system has.
    let mut folders = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts"), home.join(".fonts"), own];
    for _ in 0..4096 {
        let mut found: Vec<PathBuf> = std::fs::read_dir(folders.pop()?).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        found.sort();
        folders.extend(found.iter().rev().filter(|path| path.is_dir()).cloned());
        let font = found.iter().filter(|path| coloured(path)).find_map(|path| FontFile::open(path).filter(smiles));
        if font.is_some() {
            return font;
        }
    }
    None
}

/// Whether a file may be a colour font: by its name, and by colour table tags among its first
/// bytes, where a font lists its tables. (So not every font has to be opened and parsed.)
#[cfg(not(any(windows, target_family = "wasm")))]
fn coloured(path: &std::path::Path) -> bool {
    use std::io::Read;
    let kind = path.extension().and_then(|kind| kind.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut start = [0; 4096];
    let read = std::fs::File::open(path).ok().filter(|_| ["ttf", "otf", "ttc"].contains(&kind.as_str())).and_then(|mut file| file.read(&mut start).ok());
    start[..read.unwrap_or(0)].windows(4).any(|name| [b"COLR", b"CBDT", b"sbix", b"SVG "].contains(&name.try_into().unwrap_or(&[0; 4])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_file::AppFonts;
    use crate::font_file::tests::{coloured, emoji_font, file, picture_font};
    use crate::geom::Size;
    use crate::platform::LocalClipboard;
    use crate::surface::Surface;
    use crate::text::BuiltInFont;
    use crate::ui::Ui;

    /// The test emoji font's colours (see `emoji_font`), as pixels.
    const YELLOW: u32 = 0xffff_cc00;
    const BLACK: u32 = 0xff00_0000;
    const RED: u32 = 0xffe0_0000;
    /// Its blue is half transparent: this is what that gives on a clear block.
    const BLUE: u32 = 0x7f00_007f;

    /// The built-in font, 16 units high, 12 wide per letter, in a 20-high line: baseline 16
    /// down. With it, the test emoji font.
    fn own() -> Emoji {
        Emoji::around(Box::new(BuiltInFont)).with(emoji_font())
    }
    const FONT: Font = Font::new(16.0);

    /// A line drawn 2 pixels in and 2 down on a 64 by 24 block of `ground` pixels: baseline 18
    /// down.
    fn drawn(writer: &mut dyn TextSystem, text: &str, color: Color, ground: u32) -> Vec<u32> {
        let mut pixels = vec![ground; 64 * 24];
        writer.draw(&mut Canvas::new(&mut pixels, 64, 24, (0, 0), 1.0), text, 2.0, 2.0, FONT, color);
        pixels
    }

    /// Where one colour's pixels are in such a block: left, top, right, bottom, and count.
    fn reach(pixels: &[u32], width: usize, colour: impl Fn(u32) -> bool) -> ([usize; 4], usize) {
        let found: Vec<(usize, usize)> = pixels.iter().enumerate().filter(|(_, pixel)| colour(**pixel)).map(|(n, _)| (n % width, n / width)).collect();
        let around = found.iter().fold([usize::MAX, usize::MAX, 0, 0], |[left, top, right, bottom], (x, y)| [left.min(*x), top.min(*y), right.max(x + 1), bottom.max(y + 1)]);
        (around, found.len())
    }

    /// A text's width, and where each character ends.
    fn measured(writer: &mut dyn TextSystem, text: &str) -> (f32, Vec<f32>) {
        let mut stops = Vec::new();
        (writer.measure(text, FONT, 1.0, Some(&mut stops)), stops)
    }

    #[test]
    fn text_without_pictures_is_the_other_writers_to_the_pixel() {
        // Plain ASCII; Latin-1; a dash, an ellipsis and quotes, whose lead bytes are as high as
        // emoji's; Chinese; a combining accent; and emoji-capable characters that are text by
        // default: a heart, a digit, ©.
        let texts = ["", "plain words", "caf\u{e9} na\u{ef}ve", "wait\u{2026} \u{2013} \u{201c}so\u{201d}", "\u{4e2d}\u{6587}", "e\u{301}", "\u{2764} 1 \u{a9} \u{2122}"];
        // Both kinds of inner system: the built-in font, and an app's font.
        let letters = || AppFonts::new(FontFile::read(file(&[0, 1, 0, 0], &coloured(&[]))).unwrap());
        let mut pairs: [(Box<dyn TextSystem>, Emoji); 2] = [(Box::new(BuiltInFont), own()), (Box::new(letters()), Emoji::around(Box::new(letters())).with(emoji_font()))];
        for (alone, around) in &mut pairs {
            for text in texts {
                assert_eq!(measured(around, text), measured(alone.as_mut(), text), "{text:?}");
                assert!(drawn(around, text, Color::WHITE, 0) == drawn(alone.as_mut(), text, Color::WHITE, 0), "{text:?} is drawn differently");
                assert_eq!(around.metrics(FONT, 1.5), alone.metrics(FONT, 1.5));
            }
            assert!(around.met.is_empty(), "nothing was taken for a picture");
        }
        // U+FE0E, an emoji the font lacks, a flag it has no letters for, and a digit without
        // keycap: all the inner system's too.
        let (mut alone, mut around) = (BuiltInFont, own());
        for text in ["a\u{1f600}\u{fe0e}b", "\u{1f601}", "\u{1f1e9}\u{1f1ea}", "1\u{fe0f}", "\u{1f600}\u{fe0e}\u{1f601}x"] {
            assert_eq!(measured(&mut around, text), measured(&mut alone, text), "{text:?}");
            assert!(drawn(&mut around, text, Color::WHITE, 0) == drawn(&mut alone, text, Color::WHITE, 0), "{text:?} is drawn differently");
        }
        // No emoji is ever missed by the byte scan. Checked for every character up to the last
        // emoji-capable one, alone and with each modifier that makes a plain one an emoji, at
        // every offset within eight bytes.
        let after = ["", "\u{fe0f}", "\u{20e3}", "\u{1f3fd}", "\u{200d}\u{1f600}"];
        for (first, after) in (0..0x20000).filter_map(char::from_u32).flat_map(|first| after.map(|after| (first, after))) {
            let text = ["abcdefgh"[..first as usize % 8].to_string().as_str(), first.to_string().as_str(), after].concat();
            assert!(picture(first, &text[first as usize % 8..]) == 0 || !plain(&text), "{text:?} is a picture, and is taken for plain text");
        }
        assert!(["plain", "caf\u{e9}", "wait\u{2026} \u{2013} \u{201c}so\u{201d} \u{2022}", "\u{4e2d}\u{6587}\u{3001}\u{3002}", "\u{d55c}\u{ae00}", "\u{3b1}\u{431}"].iter().all(|text| plain(text)));
        // The system emoji font is not looked up until an emoji appears; without one, emoji are
        // the inner system's too.
        let mut unasked = Emoji::around(Box::new(BuiltInFont));
        texts.iter().for_each(|text| _ = (measured(&mut unasked, text), drawn(&mut unasked, text, Color::WHITE, 0)));
        assert!(matches!(unasked.source, Source::Unasked));
        let mut none = Emoji { source: Source::Nowhere, ..Emoji::around(Box::new(BuiltInFont)) };
        assert_eq!(measured(&mut none, "a\u{1f600}\u{1f44d}\u{1f3fd}b"), measured(&mut alone, "a\u{1f600}\u{1f44d}\u{1f3fd}b"));
        assert!(drawn(&mut none, "a\u{1f600}b", Color::WHITE, 0) == drawn(&mut alone, "a\u{1f600}b", Color::WHITE, 0));
    }

    #[test]
    fn a_picture_stands_on_the_line_in_its_own_colours() {
        let mut emoji = own();
        // A letter 12 wide, the face 7 wide, a letter: each character ends where it does.
        assert_eq!(measured(&mut emoji, "a\u{1f600}b"), (31.0, vec![12.0, 19.0, 31.0]));
        // The face's pen is 14 pixels in. Its yellow box is 3 wide and 7 high, a pixel right of
        // the pen, on the baseline; its black box 2 square, over the yellow's foot.
        let pixels = drawn(&mut emoji, "a\u{1f600}b", Color::WHITE, 0);
        assert_eq!(reach(&pixels, 64, |pixel| pixel == YELLOW), ([15, 11, 18, 18], 3 * 7 - 4));
        assert_eq!(reach(&pixels, 64, |pixel| pixel == BLACK), ([15, 16, 17, 18], 4));
        // The letters around it are where the inner system puts them: "a" at the start, "b" 21
        // pixels in, past the face.
        let letters = |x: f32| {
            let mut pixels = vec![0u32; 64 * 24];
            BuiltInFont.draw(&mut Canvas::new(&mut pixels, 64, 24, (0, 0), 1.0), if x == 2.0 { "a" } else { "b" }, x, 2.0, FONT, Color::WHITE);
            pixels
        };
        let (a, b) = (letters(2.0), letters(21.0));
        assert!((0..64 * 24).filter(|n| n % 64 < 14).all(|n| pixels[n] == a[n]) && (0..64 * 24).filter(|n| n % 64 >= 21).all(|n| pixels[n] == b[n]));
        // The text colour does not tint an emoji.
        let green = drawn(&mut emoji, "a\u{1f600}b", Color::rgb(0x00ff00), 0);
        assert_eq!((reach(&green, 64, |pixel| pixel == YELLOW), reach(&green, 64, |pixel| pixel == BLACK)), (([15, 11, 18, 18], 17), ([15, 16, 17, 18], 4)));
        // The text's alpha fades it: half transparent over black, its yellow is half.
        let pale = drawn(&mut emoji, "\u{1f600}", Color::WHITE.with_alpha(0.5), BLACK);
        let [red, green, blue] = [16, 8, 0].map(|part| pale[11 * 64 + 3] >> part & 255);
        assert!((127..=129).contains(&red) && (101..=103).contains(&green) && blue == 0 && pale[11 * 64 + 3] >> 24 == 255, "{:08x}", pale[11 * 64 + 3]);
        // A layer the font leaves in the text colour takes it: the flag is a red square with a
        // hole, 6 wide, and the small box at its foot. For another text colour it is redrawn;
        // an emoji without such a layer is not.
        let kept = (drawn(&mut emoji, "\u{1f1e6}\u{1f1e7}", Color::WHITE, 0), emoji.cache_bytes).1;
        for ink in [Color::WHITE, Color::rgb(0x00ff00), Color::WHITE] {
            let flag = drawn(&mut emoji, "\u{1f1e6}\u{1f1e7}", ink, 0);
            assert_eq!((reach(&flag, 64, |pixel| pixel == RED), reach(&flag, 64, |pixel| pixel == ink.0)), (([2, 12, 8, 18], 36 - 4 - 4), ([3, 16, 5, 18], 4)), "{ink}");
            assert_eq!(emoji.cache_bytes, kept);
        }
        // A glyph with no colours at all is its outline in the text colour: a hand is the small
        // box, 6 wide.
        let hand = drawn(&mut emoji, "\u{1f44d}", Color::rgb(0x8b93ff), 0);
        assert_eq!((reach(&hand, 64, |pixel| pixel == 0xff8b_93ff), reach(&hand, 64, |pixel| pixel != 0).1, measured(&mut emoji, "\u{1f44d}x").0), (([3, 16, 5, 18], 4), 4, 18.0));
    }

    #[test]
    fn characters_that_belong_together_are_one_picture_with_one_width() {
        let mut emoji = own();
        // A flag between two letters: its two characters end at the same place.
        assert_eq!(measured(&mut emoji, "x\u{1f1e6}\u{1f1e7}y"), (34.0, vec![12.0, 22.0, 22.0, 34.0]));
        // A man joined to a child, a keycap, a hand with a skin tone, a heart with U+FE0F: each
        // is the font's one glyph for it.
        for (sequence, wide) in [("\u{1f468}\u{200d}\u{1f466}", 10.0), ("1\u{fe0f}\u{20e3}", 6.0), ("1\u{20e3}", 6.0), ("\u{1f44d}\u{1f3fd}", 6.0), ("\u{2764}\u{fe0f}", 6.0)] {
            assert_eq!(measured(&mut emoji, sequence), (wide, vec![wide; sequence.chars().count()]), "{sequence:?}");
            assert_eq!(emoji.pieces.len(), 1, "{sequence:?}");
        }
        // The keycap and the skin tone give the same glyph here: the box in half-transparent
        // blue.
        for sequence in ["1\u{fe0f}\u{20e3}", "\u{1f44d}\u{1f3fd}"] {
            let pixels = drawn(&mut emoji, sequence, Color::WHITE, 0);
            assert_eq!((reach(&pixels, 64, |pixel| pixel == BLUE), reach(&pixels, 64, |pixel| pixel != 0).1), (([3, 11, 6, 18], 21), 21), "{sequence:?}");
        }
        // A sequence without its own glyph is its parts side by side, still one thing to the
        // cursor: a face with a skin tone is the face and the tone's glyph.
        assert_eq!(measured(&mut emoji, "\u{1f600}\u{1f3fd}a"), (25.0, vec![13.0, 13.0, 25.0]));
        let parts = drawn(&mut emoji, "\u{1f600}\u{1f3fd}", Color::WHITE, 0);
        assert_eq!((reach(&parts, 64, |pixel| pixel == YELLOW).0, reach(&parts, 64, |pixel| pixel >> 24 != 0 && pixel != YELLOW && pixel != BLACK).0), ([3, 11, 6, 18], [10, 14, 12, 18]));
        // Three regional indicators are a flag and a letter.
        assert_eq!(measured(&mut emoji, "\u{1f1e6}\u{1f1e7}\u{1f1e8}"), (20.0, vec![10.0, 10.0, 20.0]));
        // In mixed text, each piece is where the widths before it put it.
        let text = "a\u{1f600}\u{1f1e6}\u{1f1e7}b\u{1f44d}\u{1f3fd}";
        assert_eq!(measured(&mut emoji, text), (47.0, vec![12.0, 19.0, 29.0, 29.0, 41.0, 47.0, 47.0]));
        let pixels = drawn(&mut emoji, text, Color::WHITE, 0);
        assert_eq!(
            (reach(&pixels, 64, |pixel| pixel == YELLOW).0, reach(&pixels, 64, |pixel| pixel == RED).0, reach(&pixels, 64, |pixel| pixel == BLUE).0),
            ([15, 11, 18, 18], [21, 12, 27, 18], [44, 11, 47, 18])
        );
    }

    #[test]
    fn a_picture_is_as_large_as_the_letters_are() {
        // Text twice as high, or a screen twice as dense: the face is 14 pixels wide, its
        // yellow box 6 by 14, 2 pixels right of the pen, which is 4 pixels in.
        for (font, scale) in [(Font::new(32.0), 1.0), (FONT, 2.0)] {
            let (mut emoji, mut pixels) = (own(), vec![0u32; 64 * 48]);
            assert_eq!(emoji.measure("\u{1f600}", font, scale, None), 14.0 / scale);
            emoji.draw(&mut Canvas::new(&mut pixels, 64, 48, (0, 0), scale), "\u{1f600}", 4.0 / scale, 4.0 / scale, font, Color::WHITE);
            assert_eq!(reach(&pixels, 64, |pixel| pixel == YELLOW), ([6, 22, 12, 36], 6 * 14 - 16), "{font:?} at {scale}");
        }
        // At a size between, edges fall inside pixels and are soft; the width is whole pixels.
        let (mut emoji, mut pixels) = (own(), vec![0u32; 64 * 48]);
        assert_eq!(emoji.measure("\u{1f600}", FONT, 1.5, None), 11.0 / 1.5);
        emoji.draw(&mut Canvas::new(&mut pixels, 64, 48, (0, 0), 1.5), "\u{1f600}", 2.0, 2.0, FONT, Color::WHITE);
        let (around, count) = reach(&pixels, 64, |pixel| pixel != 0);
        assert!(around == [4, 16, 9, 27] && count > 40 && pixels.iter().any(|pixel| pixel >> 24 != 0 && pixel >> 24 != 255), "{around:?} {count}");
        // A bitmap made for text 32 pixels high: a red 8 square, a pixel right of the pen, 7
        // above the baseline (34 down at that size, 18 and 10 for the smaller). At half the
        // size it is half as large each way, at a quarter, a quarter.
        for (size, place) in [(32.0, [3, 27, 11, 35]), (16.0, [3, 14, 7, 18]), (8.0, [2, 8, 4, 10])] {
            let (mut emoji, mut pixels) = (Emoji::around(Box::new(BuiltInFont)).with(picture_font(7)), vec![0u32; 64 * 48]);
            emoji.draw(&mut Canvas::new(&mut pixels, 64, 48, (0, 0), 1.0), "\u{1f600}", 2.0, 2.0, Font::new(size), Color::WHITE);
            assert_eq!(reach(&pixels, 64, |pixel| pixel == 0xffff_0000), (place, (place[2] - place[0]) * (place[3] - place[1])), "at {size}");
            assert_eq!(emoji.measure("\u{1f600}", Font::new(size), 1.0, None), (448.0 * size / 1024.0).round());
        }
        // Sizes of zero, negative, huge or NaN have no emoji, and nothing panics.
        for size in [0.0, -16.0, 4096.0, 1e9, f32::INFINITY, f32::NAN] {
            let (mut emoji, mut alone, mut pixels) = (own(), BuiltInFont, vec![0u32; 64 * 24]);
            let wide = emoji.measure("a\u{1f600}", Font::new(size), 1.0, None);
            assert!(wide.to_bits() == alone.measure("a\u{1f600}", Font::new(size), 1.0, None).to_bits(), "{size}");
            // The built-in font itself is only drawn at possible sizes.
            let font = Font::new(if size > 4096.0 { 0.0 } else { size });
            emoji.draw(&mut Canvas::new(&mut pixels, 64, 24, (0, 0), 1.0), "a\u{1f600}", 2.0, 2.0, font, Color::WHITE);
            assert!(emoji.met.is_empty());
        }
    }

    #[test]
    fn a_picture_keeps_to_the_canvas_and_to_its_line() {
        let (mut emoji, text) = (own(), "a\u{1f600}\u{1f1e6}\u{1f1e7}b\u{1f44d}\u{1f3fd}");
        let whole = drawn(&mut emoji, text, Color::WHITE, 0);
        // Drawn in four-row strips of the block, it is the same picture;
        let mut strips = vec![0u32; 64 * 24];
        for (n, strip) in strips.chunks_mut(64 * 4).enumerate() {
            emoji.draw(&mut Canvas::new(strip, 64, 4, (0, n as i32 * 4), 1.0), text, 2.0, 2.0, FONT, Color::WHITE);
        }
        assert!(strips == whole);
        // and on a canvas clipped to a part, nothing shows outside it.
        let mut held = vec![0u32; 64 * 24];
        let mut canvas = Canvas::new(&mut held, 64, 24, (0, 0), 1.0);
        canvas.clip(rect(16.0, 0.0, 9.0, 14.0));
        emoji.draw(&mut canvas, text, 2.0, 2.0, FONT, Color::WHITE);
        assert!((0..64 * 24).all(|n| held[n] == if (16..25).contains(&(n % 64)) && n / 64 < 14 { whole[n] } else { 0 }));
        // A bitmap reaching above its line is clipped at the line's top: this one is 4 pixels
        // square at this size, its top 18 above the baseline, which is 2 above a line starting
        // 2 down.
        let mut tall = Emoji::around(Box::new(BuiltInFont)).with(picture_font(36));
        assert_eq!(reach(&drawn(&mut tall, "\u{1f600}", Color::WHITE, 0), 64, |pixel| pixel != 0), ([3, 2, 7, 4], 4 * 2));
    }

    #[test]
    fn pictures_are_drawn_once_and_kept_up_to_a_limit() {
        let mut emoji = own();
        assert_eq!(emoji.cache_bytes, 0);
        // Measuring caches the width, drawing the bitmap; neither is done twice.
        let measured = (emoji.measure("\u{1f600}\u{1f1e6}\u{1f1e7}", FONT, 1.0, None), emoji.cache_bytes).1;
        let kept = (drawn(&mut emoji, "\u{1f600}\u{1f1e6}\u{1f1e7}", Color::WHITE, 0), emoji.cache_bytes).1;
        assert!(measured > 0 && kept >= measured + (3 * 7 + 6 * 6) * 4 && kept < 1000, "{measured} {kept}");
        for _ in 0..3 {
            let _ = (emoji.measure("a\u{1f600}\u{1f1e6}\u{1f1e7}", FONT, 1.0, None), drawn(&mut emoji, "\u{1f1e6}\u{1f1e7} \u{1f600}", Color::WHITE, 0));
        }
        assert_eq!((emoji.cache_bytes, emoji.met.len()), (kept, 2));
        // A sequence the font lacks is remembered as such and not asked again.
        let lacking = (emoji.measure("\u{1f601}", FONT, 1.0, None), emoji.cache_bytes).1;
        assert_eq!((emoji.measure("\u{1f601}\u{1f601}", FONT, 1.0, None), emoji.cache_bytes), (24.0, lacking));
        // Emoji at ever new sizes don't pile up without end: a face 900 pixels high or more is
        // a third of a megabyte.
        let mut most = 0;
        for size in 0..40 {
            let mut pixels = vec![0u32; 16 * 16];
            emoji.draw(&mut Canvas::new(&mut pixels, 16, 16, (0, 0), 1.0), "\u{1f600}", 0.0, 0.0, Font::new(900.0 + size as f32), Color::WHITE);
            most = most.max(emoji.cache_bytes);
        }
        assert!(most > CACHE_LIMIT && most < CACHE_LIMIT + 500_000 && emoji.cache_bytes < most, "{most} {}", emoji.cache_bytes);
    }

    #[test]
    fn a_font_of_the_apps_own_leaves_pictures_whole_to_the_writer_of_what_it_lacks() {
        // The text font has "A" (7 wide), "B" (9) and "1", and kerns "AB" 2 closer. A keycap is
        // an emoji all the same, digit included, and so is a flag of letters the font maps
        // elsewhere.
        let letters = FontFile::read(file(&[0, 1, 0, 0], &coloured(&[]))).unwrap();
        let mut fonts = AppFonts::new(letters).fallback(Box::new(own()));
        // (Shaped, half of the pair's kerning goes to each.)
        assert_eq!(measured(&mut fonts, "AB"), (14.0, vec![if cfg!(feature = "shaping") { 6.0 } else { 7.0 }, 14.0]));
        assert_eq!(measured(&mut fonts, "A1B"), (22.0, vec![7.0, 13.0, 22.0]));
        assert_eq!(measured(&mut fonts, "A1\u{fe0f}\u{20e3}B"), (22.0, vec![7.0, 13.0, 13.0, 13.0, 22.0]));
        assert_eq!(measured(&mut fonts, "A\u{1f1e6}\u{1f1e7}\u{1f600}B"), (33.0, vec![7.0, 17.0, 17.0, 24.0, 33.0]));
        // Emoji sit on the text font's baseline: 13 pixels below the top here, so 15 down. The
        // keycap's glyph is the blue box, 7 pixels high.
        let pixels = drawn(&mut fonts, "A1\u{fe0f}\u{20e3}B", Color::WHITE, 0);
        assert_eq!(reach(&pixels, 64, |pixel| pixel == BLUE), ([10, 8, 13, 15], 21));
    }

    #[test]
    fn an_interface_has_picture_letters_in_colour_by_itself_only_with_the_feature() {
        // With the feature `emoji`, a UI's text system is wrapped in `Emoji`, which draws a
        // smiling face in colour from the system font (where there is one); without it, the
        // UI's text system draws it as any letter: here the built-in font, in the text colour
        // only.
        let colours = |pixels: &[u32]| pixels.iter().filter(|pixel| *pixel & 0xff_ffff != 0 && (*pixel >> 16 & 255 != *pixel & 255 || *pixel >> 8 & 255 != *pixel & 255)).count();
        let face = drawn(in_colour(Box::new(BuiltInFont)).as_mut(), "\u{1f600}", Color::WHITE, 0);
        match cfg!(feature = "emoji") && system(false).is_some() {
            true => assert!(colours(&face) > 20, "the face is in colour"),
            false => assert_eq!((colours(&face), face), (0, drawn(&mut BuiltInFont, "\u{1f600}", Color::WHITE, 0))),
        }
    }

    #[test]
    fn an_interface_shows_pictures_and_draws_only_what_changed_of_them() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 240.0, h: 80.0 }, 1.0);
        // Text 16 high, so the test font's shapes lie on whole pixels.
        ui.theme.font = FONT;
        ui.set_emoji(emoji_font());
        let mut screen = Surface::draw(&mut ui, 64);
        // After every run only what changed is redrawn, and the window is compared with a fresh
        // full redraw.
        let mut shown = |ui: &mut Ui, build: &mut dyn FnMut(&mut Ui)| {
            let changed = ui.run(build);
            screen.redraw(ui, changed);
            assert!(screen.pixels == Surface::draw(ui, 64).pixels, "drawing only what changed ({changed:?}) left the window different");
            (reach(&screen.pixels, 240, |pixel| pixel == YELLOW).1, reach(&screen.pixels, 240, |pixel| pixel == RED).1)
        };
        // A label with a face, then with a flag too, then with neither; a button.
        assert_eq!(shown(&mut ui, &mut |ui| _ = ui.label("a\u{1f600}b")), (17, 0));
        assert_eq!(shown(&mut ui, &mut |ui| _ = ui.label("a\u{1f600}\u{1f1e6}\u{1f1e7}b")), (17, 28));
        assert_eq!(shown(&mut ui, &mut |ui| _ = ui.label("ab")), (0, 0));
        assert_eq!(shown(&mut ui, &mut |ui| _ = ui.button("\u{1f1e6}\u{1f1e7} go \u{1f600}")), (17, 28));
        // A text box: a man joined to a child is typed, and is one thing to the cursor, which
        // one key moves over and another deletes.
        let mut text = String::new();
        let mut focused = false;
        let mut typing = |ui: &mut Ui, text: &mut String| {
            let state = ui.text_box(text, "");
            if !std::mem::replace(&mut focused, true) {
                ui.set_focus(state.id);
            }
        };
        for event in [None, Some(crate::Event::Text("x\u{1f468}\u{200d}\u{1f466}".into())), Some(crate::Event::Key(crate::Key::Left, crate::Mods::NONE))] {
            event.into_iter().for_each(|event| _ = ui.event(event));
            shown(&mut ui, &mut |ui| typing(ui, &mut text));
        }
        assert_eq!((text.as_str(), ui.edit.cursor), ("x\u{1f468}\u{200d}\u{1f466}", 1));
        ui.event(crate::Event::Key(crate::Key::Delete, crate::Mods::NONE));
        assert_eq!((shown(&mut ui, &mut |ui| typing(ui, &mut text)), text.as_str()), ((0, 0), "x"));
    }

    /// The system emoji font, and with `TAUT_EMOJI` set, that font file too: each wrapping text
    /// from `letters`, with a name.
    fn real(letters: impl Fn() -> Box<dyn TextSystem>) -> Vec<(String, Emoji)> {
        let named = std::env::var_os("TAUT_EMOJI").map(std::path::PathBuf::from);
        let named = named.map(|path| (path.file_stem().unwrap().to_string_lossy().into_owned(), Emoji::around(letters()).with(FontFile::open(&path).expect("the font is read"))));
        let mut system = Emoji::around(letters());
        system.measure("\u{1f600}", FONT, 1.0, None);
        match system.source {
            Source::Font(_) => std::iter::once(("system".to_string(), system)).chain(named).collect(),
            _ => {
                eprintln!("no font of emoji is found on this system: its own emoji are not tested");
                named.into_iter().collect()
            }
        }
    }

    #[test]
    fn the_emoji_of_a_real_font_are_drawn_in_colour() {
        // A face, a hand with a skin tone, a heart with U+FE0F, a keycap, a flag, a family, a
        // woman astronaut, and two people of different skin tones with a heart between.
        let (flag, family) = ("\u{1f1e9}\u{1f1ea}", "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}");
        let sequences = [
            "\u{1f600}",
            "\u{1f44d}\u{1f3fd}",
            "\u{2764}\u{fe0f}",
            "1\u{fe0f}\u{20e3}",
            flag,
            family,
            "\u{1f469}\u{200d}\u{1f680}",
            "\u{1f469}\u{1f3fb}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f468}\u{1f3ff}",
        ];
        let text = ["ab ", &sequences.join(" "), " ok"].concat();
        for (name, mut emoji) in real(|| Box::new(BuiltInFont)) {
            for (font, scale) in [(FONT, 1.0), (Font::new(32.0), 1.5)] {
                let pixels = font.size * scale;
                // Each sequence is one piece with one width: all its characters end where it
                // ends. (A flag too, where the font only has its two letters.)
                for sequence in sequences {
                    let mut stops = Vec::new();
                    let wide = emoji.measure(sequence, font, scale, Some(&mut stops));
                    assert_eq!((emoji.pieces.len(), &stops), (1, &vec![wide; sequence.chars().count()]), "{name}: {sequence:?}");
                    assert!((0.8..=3.0).contains(&(wide * scale / pixels)) && (wide * scale).fract() == 0.0, "{name}: {sequence:?} is {wide} wide");
                }
                // The whole line, on white: 4 units in and 4 down.
                let (wide, high) = (emoji.measure(&text, font, scale, None), emoji.metrics(font, scale).height());
                let (width, height) = (((wide + 8.0) * scale).ceil() as usize, ((high + 8.0) * scale).ceil() as usize);
                let mut line = vec![0xffff_ffffu32; width * height];
                emoji.draw(&mut Canvas::new(&mut line, width, height, (0, 0), scale), &text, 4.0, 4.0, font, Color::BLACK);
                if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
                    let file = format!("emoji-{}-{name}-{}-{scale}.png", std::env::consts::OS, font.size);
                    std::fs::write(std::path::Path::new(&folder).join(file), png::encode(width, &line).unwrap()).unwrap();
                }
                // It is in colour: a good part of what is drawn is neither black, white nor
                // grey. (The letters around are black on white.)
                let coloured =
                    |pixel: &&u32| [16, 8, 0].map(|part| (**pixel >> part & 255) as i32).iter().max().unwrap() - [16, 8, 0].map(|part| (**pixel >> part & 255) as i32).iter().min().unwrap() > 48;
                let (colours, marks) = (line.iter().filter(coloured).count(), line.iter().filter(|pixel| **pixel != 0xffff_ffff).count());
                assert!(colours * 3 > marks && colours as f32 > pixels * pixels * 2.0, "{name}: {colours} pixels of {marks} are in colour");
                // The face is about as high as the text, on its baseline: within the line,
                // across its middle.
                let mut face = vec![0u32; (pixels * 3.0) as usize * height];
                emoji.draw(&mut Canvas::new(&mut face, (pixels * 3.0) as usize, height, (0, 0), scale), "\u{1f600}", 4.0, 4.0, font, Color::BLACK);
                let ([_, top, _, bottom], _) = reach(&face, (pixels * 3.0) as usize, |pixel| pixel >> 24 > 128);
                let (line_top, line_bottom) = ((4.0 * scale) as usize, ((4.0 + high) * scale) as usize);
                assert!(((bottom - top) as f32 / pixels - 1.05).abs() < 0.25, "{name}: the face is {} pixels high for letters of {pixels}", bottom - top);
                assert!(
                    top >= line_top && bottom <= line_bottom && top < (line_top + line_bottom) / 2 && bottom > (line_top + line_bottom) / 2,
                    "{name}: the face is from {top} to {bottom} in a line from {line_top} to {line_bottom}"
                );
            }
            // A family is one emoji, wider than high or about square: not four people in a row.
            // So is a flag, where the font has flags.
            let Source::Font(file) = &emoji.source else { unreachable!() };
            let mut glyphs = Vec::new();
            let wide = file.set(family, &mut glyphs).expect("the font has a family") * file.unit(1.0);
            assert!((0.8..=1.6).contains(&wide), "{name}: a family is {wide} of its height wide");
            let flags = file.set(flag, &mut glyphs).is_some_and(|_| glyphs.len() == 1);
            println!(
                "{name}: a family is {} glyphs, {wide:.2} of the letters' height wide; a flag is {}",
                file.set(family, &mut glyphs).map_or(0, |_| glyphs.len()),
                if flags { "one glyph" } else { "its two letters" }
            );
        }
    }

    /// The system's text system, where there is one.
    fn of_the_system() -> Option<Box<dyn TextSystem>> {
        crate::platform::SystemText::new().map(|text| Box::new(text) as Box<dyn TextSystem>)
    }

    #[test]
    fn pictures_stand_among_the_letters_of_the_system() {
        let Some(mut alone) = of_the_system() else { return eprintln!("no text of the system's own here: not tested") };
        // Text without emoji matches the system's to the pixel, in every face.
        let mut around = Emoji::around(of_the_system().unwrap());
        for (text, font) in [("Hello, world", FONT), ("caf\u{e9} \u{2026} \u{2013} \u{4e2d}\u{6587}", Font::new(13.0).bold()), ("let x = 1;", Font::new(13.0).mono())] {
            let both = [alone.as_mut(), &mut around as &mut dyn TextSystem].map(|writer| {
                let (mut stops, mut pixels) = (Vec::new(), vec![0u32; 240 * 40]);
                let wide = writer.measure(text, font, 1.5, Some(&mut stops));
                writer.draw(&mut Canvas::new(&mut pixels, 240, 40, (0, 0), 1.5), text, 2.0, 2.0, font, Color::WHITE);
                (wide, stops, pixels)
            });
            assert!(both[0] == both[1], "{text:?} is not as the system writes it");
        }
        assert!(matches!(around.source, Source::Unasked));
        // A face among letters: in colour, about the line's height, sitting on it: its foot
        // near the baseline, not at the line's top nor below it.
        let text = "Hxg \u{1f600} Hxg \u{1f44d}\u{1f3fd} 1\u{fe0f}\u{20e3} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}.";
        for (name, mut emoji) in real(|| of_the_system().unwrap()) {
            for (font, scale) in [(Font::new(13.0), 1.0), (Font::new(13.0), 1.5), (Font::new(20.0), 2.0)] {
                let (pixels, metrics) = (font.size * scale, emoji.metrics(font, scale));
                let (width, height) = (((emoji.measure(text, font, scale, None) + 8.0) * scale).ceil() as usize, ((metrics.height() + 8.0) * scale).ceil() as usize);
                let mut line = vec![0xffff_ffffu32; width * height];
                emoji.draw(&mut Canvas::new(&mut line, width, height, (0, 0), scale), text, 4.0, 4.0, font, Color::BLACK);
                if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
                    let file = format!("text-{}-{name}-{}-{scale}.png", std::env::consts::OS, font.size);
                    std::fs::write(std::path::Path::new(&folder).join(file), png::encode(width, &line).unwrap()).unwrap();
                }
                let coloured = |pixel: u32| (pixel >> 16 & 255).abs_diff(pixel & 255) > 60 || (pixel >> 8 & 255).abs_diff(pixel & 255) > 60;
                let ([_, top, _, bottom], count) = reach(&line, width, coloured);
                let stands = ((4.0 + metrics.ascent) * scale).round();
                assert!(count as f32 > pixels * pixels, "{name}: {count} pixels are in colour at {pixels}");
                assert!(((bottom - top) as f32 / pixels - 1.1).abs() < 0.3, "{name}: the pictures are from {top} to {bottom} for letters of {pixels}");
                assert!((bottom as f32 - stands) / pixels > -0.1 && (bottom as f32 - stands) / pixels < 0.4, "{name}: the pictures' foot is at {bottom}, and the letters stand on {stands}");
            }
        }
    }

    /// Where the desktop has Fontconfig, it names an emoji font file that exists; and if that
    /// is a colour font, the system has an emoji font. (A machine without one is named some
    /// other, which is not taken for one.)
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    #[test]
    fn the_font_the_desktop_names_for_emoji_is_asked_first() {
        // The desktop is asked as system text is found.
        let _letters = crate::x11::SystemText::new();
        let Some(named) = crate::x11::emoji_file() else { return };
        assert!(named.is_file(), "{named:?} is named and is not there");
        let pictured = FontFile::open(&named).is_some_and(|font| font.pictured());
        assert!(!pictured || system(false).is_some(), "{named:?} has pictures, and no font of emoji was found");
    }
}
