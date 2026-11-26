//! SVG files, read and drawn: icons and logos from drawing programs and icon sets, in their own
//! colours, sharp at any size. No other library needed.
//!
//! Reading walks the element tree once into a flat list of shapes, each with its transform,
//! fill and stroke; what a group or a `<use>` says is applied to every shape it holds. Drawing
//! renders the shapes in order at the asked size through the canvas, flattening curves as
//! finely as that size needs and filling strokes as outlines.
//!
//! Groups with opacity, clip paths, masks and filters render to an offscreen block first, then
//! composite. A box aligned with the picture (a nested viewport, or a clip path that is such a
//! box) needs no block: shapes are clipped to it exactly. A pattern's tile renders to a block
//! just before the shape it fills. Clip paths, masks, patterns and markers are expanded where
//! used, as `<use>` is. Filters run on their block (see `svg_filters.rs`).
//!
//! Read:
//! - shapes: `path`, `rect`, `circle`, `ellipse`, `line`, `polyline`, `polygon`; groups and
//!   `transform`; `viewBox` with `preserveAspectRatio`
//! - paint: named colours, `#rgb`, `#rrggbb`, `rgb()`, `currentColor` (the box's colour); fill,
//!   stroke and group opacity; both fill rules
//! - strokes: width, caps, joins, miter limit, dashes, `vector-effect`, `paint-order`;
//!   with [`enable_shape_rendering`], `shape-rendering` (`crispEdges` and `optimizeSpeed` draw
//!   whole pixels)
//! - `style="…"` and presentation attributes; style sheet rules by element name, class and id
//! - `<defs>`, `<symbol>`, `<use>`; nested `<symbol>` and `<svg>` viewports, clipped unless
//!   `overflow` says otherwise
//! - linear and radial gradients; clip paths (user space or bounding box, either rule);
//!   luminance masks; patterns (units, transform, `viewBox`, `href` inheritance) for fills and
//!   strokes
//! - markers at start, corners and end, oriented or not, sized by stroke width or in units
//! - filters on shapes and groups, as lists (`url(#…)`, `blur()`, `drop-shadow()` and the other
//!   Filter Effects functions), with filter regions and `color-interpolation-filters`:
//!   `feGaussianBlur`, `feOffset`, `feFlood`, `feMerge`, `feComposite` (all operators),
//!   `feBlend` (all Compositing and Blending modes), `feColorMatrix`, `feComponentTransfer`,
//!   `feDropShadow`, `feMorphology`, `feTile`, `feTurbulence`, `feImage`, `feDiffuseLighting`
//!   and `feSpecularLighting` (distant, point and spot lights), `feConvolveMatrix`,
//!   `feDisplacementMap`, with subregions and named inputs (`SourceGraphic`, `SourceAlpha`,
//!   `BackgroundImage`, `BackgroundAlpha`, results)
//! - with [`enable_text`]: `<text>`, `<tspan>` and `<textPath>` in the system font of the
//!   first installed family named, with size, weight, style, per-letter positions,
//!   `text-anchor` and `letter-spacing`; shaped as pages are with the feature `shaping`
//! - with [`enable_images`]: `<image>` with a `data:` URL of a PNG, JPEG, GIF, WebP, BMP, ICO
//!   or SVG, fitted by `preserveAspectRatio`; by path or URL through the app's loader
//!   ([`enable_images_from`])
//! - with [`AnimatedDrawing`], [`decode_at`] and [`encode_animation`]: `<animate>`, `<set>`,
//!   `<animateTransform>`, `<animateMotion>`, timed or (in a window) begun and ended by the
//!   pointer and keys (see `svg_animation.rs`)
//!
//! Images and `feImage` elements bring drawings of their own: each is read once the file's
//! own walk is done, into the same lists, and its items put where it was met (so nothing
//! reads recursively). Four deep at most, all within the outermost file's budget.
//!
//! Not read: a pattern as a `FillPaint` or `StrokePaint` input (nothing); glyphs are drawn
//! without filters. Style rules by position among siblings match nothing, and rules apply in
//! source order. A clip path's own clip path, `mask-type`, and marker clipping are ignored.
//! Text is not rotated per letter; unshaped (without `shaping`) it has no joining or
//! right-to-left order; it uses the family's font or the system's fallback. A gradient focus
//! outside its circle is moved onto it; a subpath starting where the previous unclosed one
//! ended continues it; `em` is 16 units; percentages refer to the root viewport, also in nested
//! ones.
//!
//! Deliberate differences from librsvg: a blur with deviation under two pixels integrates the
//! Gaussian over each pixel (librsvg samples further out, so its small blurs come out
//! narrower); stitched turbulence tiles meet at their edges (librsvg's do not); `lighter` adds
//! (librsvg composites over); `lighting-color` is taken into linear light where the filter
//! works there, as browsers take it (librsvg takes its sRGB values as linear); a crisp shape
//! paints a pixel more than half covered (librsvg one whose sampling point it covers: they
//! differ on a few pixels of a curve); text paths are drawn (librsvg draws none); a picture's
//! edges are cut sharp (librsvg fades them over one of its pixels); a convolution keeping alpha
//! on a grid of kernel units resamples premultiplied colours (librsvg resamples plain ones as if
//! premultiplied, which tints the last cells a region fills in part).
//!
//! Cost is bounded by what a file of its length could cost written out, however often it reuses
//! elements: shapes, outline segments and reading steps (elements, attributes, style rules
//! tried, letters and font lookups), and passes over the whole picture counted while reading
//! (each shape the area of its bounding box, more where colour is computed per pixel; each
//! offscreen group, clip and tile its block's area; each filter its region times its cost, 18
//! for a blur; each image as a tile). The bound is 64 passes, plus one per 16 bytes of file. A
//! filter over budget is skipped (its element drawn unfiltered); what lies past the budget is
//! not drawn. (The costliest icon of a whole desktop icon theme uses a third of it.)
//!
//! With [`enable_seamless_clips`], clip paths and masks fill their abutting opaque shapes of one
//! colour as one outline, each turned to go round positively first: no seam where they meet
//! (as browsers draw them).
//!
//! Accuracy: a shape is filled by summing each edge's coverage per pixel (see
//! [`Canvas::fill_with`]), exact where one edge crosses a pixel. Where two do, both count: two
//! opposite subpaths of one shape touching leave a hairline seam, and a stroke overlapping
//! itself (sharp corners between segments shorter than its width, self-crossings, reversals,
//! touching dash ends) darkens the edge pixels of the overlap by up to a quarter. Two
//! subpaths' strokes are drawn separately and combined: exact where they cross, up to a quarter
//! off where they run along each other. Non-rectangular clips, masks and viewport clips of
//! strokes multiply coverages at shared edge pixels (as librsvg does, except for strokes in a
//! viewport). Transformed pattern tiles are sampled bilinearly.
use crate::canvas::{Canvas, color_at};
use crate::color::Color;
use crate::font_file::FontFile;
use crate::geom::{Point, Rect, Transform, pt};
use crate::path::{Path, Step};
use crate::pen::{self, Cap, FINER, Join, Work};
use crate::svg_filters::{self, Convolution, Edge, Filter, Function, Input, Kind, Lighting, Mode, Noise, Operator, Source, Space};
use crate::text::Face;
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

pub use crate::svg_animation::AnimatedDrawing;

/// A drawing the interface keeps, to show wherever asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawing(pub(crate) u32);

/// The most pixels a drawing is cached as. Shown larger, only its visible rows are drawn,
/// afresh each time.
const MOST: usize = 1 << 24;
/// How many cached pictures of a drawing are kept: one per size and colour shown.
const KEPT: usize = 4;
/// How deeply semi-transparent groups may nest and each still composite as one (each renders to
/// its own block).
const LAYERS: usize = 4;
/// How deeply elements may nest, counting what `<use>` brings in.
const DEEP: usize = 64;
/// The least number of full-picture passes drawing a file may cost (as `Reading::passes`
/// counts): a short file cannot write out much more, but it can reuse a few elements over and
/// over, each an offscreen or clipped group, a full-size shape, or a pattern fill. A longer
/// file gets one more pass per `BYTES` of its own.
const PASSES: f32 = 64.0;
const BYTES: f32 = 16.0;
/// How much more filling the same pixels costs with a gradient or pattern than with one colour:
/// each pixel's colour is computed.
const BLENDS: f32 = 8.0;
const TILES: f32 = 20.0;
/// How many more outline segments a segment costs whose stroke is as wide as the picture (or
/// wider): it crosses all rows, a thin one a few.
const LINED: f32 = 32.0;
/// And how many more one with round joins or caps as wide as the picture costs (the square root
/// of its share, for a thinner one).
const ROUNDS: f32 = 16.0;
/// The reading cost of setting one letter of text, in steps (see `Reading::steps`): far more
/// than an element.
const LETTER: usize = 32;
/// The least number of system font lookups reading may make: each can take tens of ms (a letter
/// no font has is looked for in all). A longer file gets one more per 512 bytes. Past that,
/// text uses the fonts found so far, and what they lack is left out. (A whole desktop icon
/// theme needs ten at most.)
const LOOKUPS: usize = 32;
/// How many bytes of system fonts reading may hold before it looks up no more: a font is handed
/// over whole (on Windows), and a CJK one is some 20 MB.
const HELD: usize = 64 << 20;
/// The cost of trying a style rule, in steps (a matching rule is applied, which is most of it);
/// an attribute and an element cost one each.
const RULE: usize = 16;

/// Named colours with their RGB.
const NAMES: &str = "aliceblue=f0f8ff antiquewhite=faebd7 aqua=00ffff aquamarine=7fffd4 azure=f0ffff beige=f5f5dc bisque=ffe4c4 black=000000 blanchedalmond=ffebcd blue=0000ff \
    blueviolet=8a2be2 brown=a52a2a burlywood=deb887 cadetblue=5f9ea0 chartreuse=7fff00 chocolate=d2691e coral=ff7f50 cornflowerblue=6495ed cornsilk=fff8dc crimson=dc143c cyan=00ffff \
    darkblue=00008b darkcyan=008b8b darkgoldenrod=b8860b darkgray=a9a9a9 darkgreen=006400 darkgrey=a9a9a9 darkkhaki=bdb76b darkmagenta=8b008b darkolivegreen=556b2f darkorange=ff8c00 \
    darkorchid=9932cc darkred=8b0000 darksalmon=e9967a darkseagreen=8fbc8f darkslateblue=483d8b darkslategray=2f4f4f darkslategrey=2f4f4f darkturquoise=00ced1 darkviolet=9400d3 \
    deeppink=ff1493 deepskyblue=00bfff dimgray=696969 dimgrey=696969 dodgerblue=1e90ff firebrick=b22222 floralwhite=fffaf0 forestgreen=228b22 fuchsia=ff00ff gainsboro=dcdcdc \
    ghostwhite=f8f8ff gold=ffd700 goldenrod=daa520 gray=808080 green=008000 greenyellow=adff2f grey=808080 honeydew=f0fff0 hotpink=ff69b4 indianred=cd5c5c indigo=4b0082 ivory=fffff0 \
    khaki=f0e68c lavender=e6e6fa lavenderblush=fff0f5 lawngreen=7cfc00 lemonchiffon=fffacd lightblue=add8e6 lightcoral=f08080 lightcyan=e0ffff lightgoldenrodyellow=fafad2 \
    lightgray=d3d3d3 lightgreen=90ee90 lightgrey=d3d3d3 lightpink=ffb6c1 lightsalmon=ffa07a lightseagreen=20b2aa lightskyblue=87cefa lightslategray=778899 lightslategrey=778899 \
    lightsteelblue=b0c4de lightyellow=ffffe0 lime=00ff00 limegreen=32cd32 linen=faf0e6 magenta=ff00ff maroon=800000 mediumaquamarine=66cdaa mediumblue=0000cd mediumorchid=ba55d3 \
    mediumpurple=9370db mediumseagreen=3cb371 mediumslateblue=7b68ee mediumspringgreen=00fa9a mediumturquoise=48d1cc mediumvioletred=c71585 midnightblue=191970 mintcream=f5fffa \
    mistyrose=ffe4e1 moccasin=ffe4b5 navajowhite=ffdead navy=000080 oldlace=fdf5e6 olive=808000 olivedrab=6b8e23 orange=ffa500 orangered=ff4500 orchid=da70d6 palegoldenrod=eee8aa \
    palegreen=98fb98 paleturquoise=afeeee palevioletred=db7093 papayawhip=ffefd5 peachpuff=ffdab9 peru=cd853f pink=ffc0cb plum=dda0dd powderblue=b0e0e6 purple=800080 \
    rebeccapurple=663399 red=ff0000 rosybrown=bc8f8f royalblue=4169e1 saddlebrown=8b4513 salmon=fa8072 sandybrown=f4a460 seagreen=2e8b57 seashell=fff5ee sienna=a0522d silver=c0c0c0 \
    skyblue=87ceeb slateblue=6a5acd slategray=708090 slategrey=708090 snow=fffafa springgreen=00ff7f steelblue=4682b4 tan=d2b48c teal=008080 thistle=d8bfd8 tomato=ff6347 \
    turquoise=40e0d0 violet=ee82ee wheat=f5deb3 white=ffffff whitesmoke=f5f5f5 yellow=ffff00 yellowgreen=9acd32";

/// Draws the text in SVG files from now on, in every drawing of the program: each letter in the
/// system font of the first installed family its text names. Without this, a drawing's text is
/// left out and the rest drawn; text needs the font reader (some 150 KB), which a program whose
/// drawings hold no text need not carry.
pub fn enable_text() {
    _ = TEXT.set(|reading, at, style| reading.text(at, style));
}

/// Draws the images in SVG files from now on, in every drawing of the program: each `<image>`
/// (and `feImage`) holding its picture in a `data:` URL as PNG, JPEG, GIF, WebP, BMP or ICO,
/// or as SVG (drawn sharp at any size, clipped to its box), sized by its box and fitted by
/// `preserveAspectRatio`. Without this they are left out and the rest drawn; reading them
/// needs those decoders, which a program whose drawings hold no images need not carry. An image
/// in a separate file, by path or URL, is read only through a loader the app gives (see
/// [`enable_images_from`]). Drawings an SVG image holds are read four deep at most, all within
/// the outermost file's cost budget.
pub fn enable_images() {
    _ = PICTURES.set(|reading, at, style, place| reading.picture(at, style, place));
}

/// Draws the images in SVG files from now on, as [`enable_images`] does, and those in separate
/// files too: `load` is handed each `href` that is no `data:` URL, as written, and gives
/// the file's bytes, or nothing to leave the image out. Each drawing read asks once per image
/// element. (The first loader given stays for the program's life.)
pub fn enable_images_from(load: impl Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'static) {
    enable_images();
    _ = LOADER.set(Box::new(load));
}

/// What reads images in separate files, once a program gave it (see `enable_images_from`).
static LOADER: std::sync::OnceLock<Box<Loader>> = std::sync::OnceLock::new();

/// Gives the bytes of an image file an `href` names.
type Loader = dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync;

/// Draws shapes as `shape-rendering` asks from now on, in every drawing of the program: those
/// marked `crispEdges` or `optimizeSpeed` cover whole pixels, each pixel they cover more than
/// half of (librsvg: each whose sample point they cover). Without this, every shape is drawn
/// smooth, and a program need not carry the code (about 1 KB).
pub fn enable_shape_rendering() {
    _ = CRISP.set(crisp);
}

/// Fills the shapes of clip paths and masks as one from now on, in every drawing of the
/// program: where two meet inside a pixel, no seam shows, as browsers draw them. (Each run of
/// opaque fills of one colour becomes one outline, each turned to go round positively first:
/// exact unless a shape goes round negatively where another overlaps it.) Without this, each is
/// drawn on its own, as librsvg does, and a program need not carry the code (about 7 KB).
pub fn enable_seamless_clips() {
    _ = SEAMLESS.set(|reading, from| reading.merge_fills(from));
}

/// How crisp shapes are drawn (see `crisp`), once a program called `enable_shape_rendering`.
static CRISP: std::sync::OnceLock<Crisp> = std::sync::OnceLock::new();

/// Draws a fill or stroke where its outline may reach, crisp (see `crisp`).
type Crisp = for<'a> fn(&mut Canvas, (&Path, &Table, f32, (usize, usize, i32)), &Ready<'a>, &mut dyn FnMut(&mut Canvas));

/// How the shapes of a clip path or mask are merged (see `Reading::merge_fills`), once a
/// program called `enable_seamless_clips`.
static SEAMLESS: std::sync::OnceLock<Seamless> = std::sync::OnceLock::new();

/// Merges the fills of a clip path or mask from an item on.
type Seamless = for<'a, 'b> fn(&'b mut Reading<'a>, usize);

/// Draws the filters in SVG files from now on, in every drawing of the program: blurs and
/// shadows, colour mixing and transfer, morphology, tiles and noise, images, lighting,
/// convolution and displacement, and the CSS `filter` functions (`blur()`, `drop-shadow()`
/// and the rest). Without this, an element with a filter is drawn unfiltered; filters take some
/// 90 KB of code, which a program whose drawings hold none need not carry. (Glyphs are always
/// drawn without filters.)
pub fn enable_filters() {
    _ = FILTERS.set(Filters { read: |reading, value, walk| reading.filtered(value, walk), apply: svg_filters::apply });
}

/// How an `<image>`'s picture is read and kept (`Reading::picture`), once a program called
/// `enable_images`.
static PICTURES: std::sync::OnceLock<Pictures> = std::sync::OnceLock::new();

/// Reads an image's picture, in a box where given (see `Reading::picture`).
type Pictures = for<'a, 'b> fn(&'b mut Reading<'a>, usize, &Style, Option<[f32; 4]>);

/// How a text element's letters are set (`Reading::text`), once a program called `enable_text`.
static TEXT: std::sync::OnceLock<Texts> = std::sync::OnceLock::new();

/// Sets a text element's letters (see `Reading::text`).
type Texts = for<'a, 'b> fn(&'b mut Reading<'a>, usize, &Style) -> Vec<(u32, Style)>;

/// How filters are read and drawn, once a program called `enable_filters`.
static FILTERS: std::sync::OnceLock<Filters> = std::sync::OnceLock::new();

/// What reads an element's filters (see `Reading::filtered`), and what applies one to a block
/// (`svg_filters::apply`).
struct Filters {
    read: for<'a, 'b> fn(&'b mut Reading<'a>, [u32; 2], &Walk),
    apply: fn(&Filter, &mut Vec<Block>),
}

/// Reads an SVG file and draws it as a `width` by `height` picture: its pixels, rows from the
/// top, each `0xAARRGGBB` (clear where the drawing has nothing), and the row length, as
/// [`Ui::add_picture`](crate::Ui::add_picture) and
/// [`png::encode_rgba`](crate::png::encode_rgba) take them. `currentColor` is black. Nothing
/// for a file holding no drawing, or a picture of no size.
pub fn decode(svg: &str, width: usize, height: usize) -> Option<(usize, Vec<u32>)> {
    let kept = ParsedDrawing::read(svg).filter(|_| width > 0 && height > 0)?;
    let mut pixels = kept.draw([width, height], [0, height], Color::BLACK);
    // A canvas keeps premultiplied colours; a picture does not.
    pixels.iter_mut().for_each(|pixel| *pixel = unmultiplied(*pixel));
    Some((width, pixels))
}

/// Draws an SVG file as its SMIL animation (`<animate>`, `<set>`, `<animateTransform>`) has it
/// `seconds` after it begins, as [`decode`] draws it. A file with no animation is as `decode`
/// draws it at any time.
pub fn decode_at(svg: &str, width: usize, height: usize, seconds: f32) -> Option<(usize, Vec<u32>)> {
    decode(&crate::svg_animation::Timeline::read(svg).frame_at(seconds), width, height)
}

/// Encodes an SVG file's SMIL animation as an animated PNG of `width` by `height` pixels, to
/// play with [`Ui::add_animation`](crate::Ui::add_animation): 30 frames a second, fewer where all
/// frames would pass 64 megapixels, a minute at most. An animation that ends plays once and
/// stays at its end; one that runs for ever is one cycle of all that repeats, from when all of
/// it has begun, on repeat. Each frame costs what drawing the file costs. Nothing for a file
/// with no drawing or no animation.
pub fn encode_animation(svg: &str, width: usize, height: usize) -> Option<Vec<u8>> {
    crate::svg_animation::encode(svg, width, height)
}

/// Whether a number is positive (NaN is not).
fn positive(number: f32) -> bool {
    number > 0.0
}

/// An affine transform: `(x, y)` maps to `(a x + c y + e, b x + d y + f)`, the six in SVG's
/// order a, b, c, d, e, f.
pub(crate) type Table = [f32; 6];

/// The identity transform.
const SAME: Table = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// One transform doing both: `inner` first, then `outer`.
pub(crate) fn both(outer: &Table, inner: &Table) -> Table {
    let ([a, b, c, d, e, f], [p, q, r, s, t, u]) = (*outer, *inner);
    [a * p + c * q, b * p + d * q, a * r + c * s, b * r + d * s, a * t + c * u + e, b * t + d * u + f]
}

/// A point through a transform.
pub(crate) fn through(table: &Table, p: Point) -> Point {
    pt(table[0] * p.x + table[2] * p.y + table[4], table[1] * p.x + table[3] * p.y + table[5])
}

/// The inverse transform. Nothing for a singular one.
pub(crate) fn undone(table: &Table) -> Option<Table> {
    let [a, b, c, d, e, f] = *table;
    let area = a * d - b * c;
    area.is_normal().then(|| [d / area, -b / area, -c / area, a / area, (c * f - d * e) / area, (b * e - a * f) / area])
}

/// The number a text starts with, and the rest: optional sign, digits with an optional point,
/// optional exponent; it may run into the previous one ("1-2", ".5.5").
pub(crate) fn number(text: &str) -> Option<(f32, &str)> {
    let text = text.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    let bytes = text.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|byte| byte.is_ascii_digit()).count();
    let mut end = matches!(bytes.first(), Some(b'-' | b'+')) as usize;
    end += digits(end);
    if bytes.get(end) == Some(&b'.') {
        end += 1 + digits(end + 1);
    }
    // An `e` without digits starts a unit: "2em".
    let signed = matches!(bytes.get(end + 1), Some(b'-' | b'+')) as usize;
    let power = if matches!(bytes.get(end), Some(b'e' | b'E')) { digits(end + 1 + signed) } else { 0 };
    end += if power > 0 { 1 + signed + power } else { 0 };
    text[..end].parse().ok().map(|number| (number, &text[end..]))
}

/// All numbers in a text, up to the first non-number.
pub(crate) fn numbers(mut text: &str) -> impl Iterator<Item = f32> + '_ {
    std::iter::from_fn(move || {
        let (number, rest) = number(text)?;
        text = rest;
        Some(number)
    })
}

/// A length in user units: a number with an optional absolute unit, or a percentage of `whole`.
fn length(text: &str, whole: f32) -> Option<f32> {
    let (number, unit) = number(text)?;
    Some(number * times(unit.trim(), whole))
}

/// A length as `length` reads it, in `f64`, as librsvg computes filter regions: a percentage is
/// converted to a fraction first.
fn exact_length(text: &str, whole: f64) -> Option<f64> {
    let (number, unit) = number(text)?;
    Some(match unit.trim() {
        "%" => f64::from(number / 100.0) * whole,
        unit => f64::from(number) * f64::from(times(unit, 0.0)),
    })
}

/// User units per one of an absolute unit (per percent, of `whole`).
fn times(unit: &str, whole: f32) -> f32 {
    match unit {
        "%" => whole / 100.0,
        "pt" => 4.0 / 3.0,
        "pc" | "em" => 16.0,
        "ex" => 8.0,
        "mm" => 96.0 / 25.4,
        "cm" => 96.0 / 2.54,
        "in" => 96.0,
        _ => 1.0,
    }
}

/// A fraction: a number from 0 to 1, or a percentage.
fn share(text: &str) -> Option<f32> {
    number(text).map(|(part, unit)| if unit.starts_with('%') { part / 100.0 } else { part })
}

/// A colour as written, lowercase: `#rgb` or `#rrggbb` (each with an optional alpha digit or
/// two), `rgb(…)` of numbers up to 255 or percentages, or a name.
pub(crate) fn color(text: &str) -> Option<Color> {
    if let Some(digits) = text.strip_prefix('#').filter(|digits| digits.bytes().all(|digit| digit.is_ascii_hexdigit())) {
        let value = u32::from_str_radix(digits, 16).ok()?;
        // Of three or four digits each is doubled.
        let twice = |shift: u32| (value >> shift & 15) * 17;
        return match digits.len() {
            3 => Some(Color::rgb(twice(8) << 16 | twice(4) << 8 | twice(0))),
            4 => Some(Color(twice(0) << 24 | twice(12) << 16 | twice(8) << 8 | twice(4))),
            6 => Some(Color::rgb(value)),
            8 => Some(Color(value.rotate_right(8))),
            _ => None,
        };
    }
    if let Some(parts) = text.strip_prefix("rgba(").or_else(|| text.strip_prefix("rgb(")) {
        let mut parts = parts.split(|c: char| c == ',' || c == '/' || c == ')' || c.is_whitespace()).filter(|part| !part.is_empty());
        let part = |text: Option<&str>, whole: f32| text.and_then(number).map(|(part, unit)| if unit.starts_with('%') { part * whole / 100.0 } else { part });
        let [red, green, blue] = [part(parts.next(), 255.0)?, part(parts.next(), 255.0)?, part(parts.next(), 255.0)?].map(|part| (part.clamp(0.0, 255.0) + 0.5) as u32);
        return Some(Color::rgb(red << 16 | green << 8 | blue).with_alpha(part(parts.next(), 1.0).unwrap_or(1.0)));
    }
    let named = NAMES.split_ascii_whitespace().find_map(|pair| pair.strip_prefix(text)?.strip_prefix('='));
    named.and_then(|digits| u32::from_str_radix(digits, 16).ok()).map(Color::rgb)
}

/// A `transform`: any of `matrix`, `translate`, `scale`, `rotate`, `skewX`, `skewY` in
/// sequence, the last applied first.
fn turned(text: &str) -> Table {
    let mut table = SAME;
    for step in text.split(')') {
        let Some((name, inside)) = step.split_once('(') else { continue };
        let mut n = [0.0f32; 6];
        let count = n.iter_mut().zip(numbers(inside)).map(|(slot, number)| *slot = number).count();
        let (sin, cos) = n[0].to_radians().sin_cos();
        let one = match name.trim_matches(|c: char| c.is_whitespace() || c == ',') {
            "matrix" if count == 6 => n,
            "translate" => [1.0, 0.0, 0.0, 1.0, n[0], n[1]],
            "scale" => [n[0], 0.0, 0.0, if count > 1 { n[1] } else { n[0] }, 0.0, 0.0],
            // Around the point given after the angle, or the origin.
            "rotate" => [cos, sin, -sin, cos, n[1] - cos * n[1] + sin * n[2], n[2] - sin * n[1] - cos * n[2]],
            "skewX" => [1.0, 0.0, n[0].to_radians().tan(), 1.0, 0.0, 0.0],
            "skewY" => [1.0, n[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => continue,
        };
        table = both(&table, &one);
    }
    table
}

/// One element of a file.
pub(crate) struct Element<'a> {
    pub(crate) name: &'a str,
    /// Its attributes' range in the list of all attributes.
    attributes: [usize; 2],
    /// The first element after it that is not inside it.
    pub(crate) end: usize,
}

/// A parsed file: its elements in source order, each knowing where its descendants end; all
/// attributes (`name="value"`); ids to elements; and its style rules, each with its selector
/// and declarations.
pub(crate) struct File<'a> {
    /// The file's text.
    pub(crate) text: &'a str,
    pub(crate) elements: Vec<Element<'a>>,
    attributes: Vec<(&'a str, &'a str)>,
    pub(crate) named: HashMap<&'a str, usize>,
    rules: Vec<(&'a str, &'a str)>,
    /// The character data of text elements and their pieces, by innermost element.
    words: Vec<(usize, &'a str)>,
}

impl<'a> File<'a> {
    /// Parses a file. Malformed parts are skipped; elements never closed end at the file's end.
    pub(crate) fn read(text: &'a str) -> File<'a> {
        let mut file = File { text, elements: Vec::new(), attributes: Vec::new(), named: HashMap::new(), rules: Vec::new(), words: Vec::new() };
        // The open elements, innermost last.
        let (mut open, mut rest) = (Vec::new(), text);
        while let Some(start) = rest.find('<') {
            // Character data inside a text or its pieces is its words.
            if let Some(&inside) = open.last().filter(|inside: &&usize| start > 0 && matches!(file.elements[**inside].name, "text" | "tspan" | "a" | "textPath")) {
                file.words.push((inside, &rest[..start]));
            }
            let tag = &rest[start + 1..];
            let long = tag_length(tag);
            (rest, open) = (tag.get(long + 1..).unwrap_or(""), file.tag(&tag[..long], open));
            // A style element holds rules, not elements.
            let sheet = open.last().filter(|at| file.elements[**at].name == "style" && !tag[..long].starts_with('/'));
            if sheet.is_some() {
                file.sheet(&rest[..rest.find("</").unwrap_or(rest.len())]);
            }
        }
        open.into_iter().for_each(|at| file.elements[at].end = file.elements.len());
        file
    }

    /// Takes in one tag, from after its `<` to before its `>`: an opening element with
    /// attributes, or a closing one. Gives back the open elements.
    fn tag(&mut self, tag: &'a str, mut open: Vec<usize>) -> Vec<usize> {
        let local = |name: &'a str| name.strip_prefix("svg:").unwrap_or(name);
        // A comment, processing instruction, or doctype.
        if tag.starts_with(['!', '?']) {
            return open;
        }
        if let Some(name) = tag.strip_prefix('/') {
            // It closes the innermost element of that name and all left open inside it.
            let closed = open.iter().rposition(|at| self.elements[*at].name == local(name.trim())).unwrap_or(open.len());
            open.drain(closed..).for_each(|at| self.elements[at].end = self.elements.len());
            return open;
        }
        let (name, mut says) = tag.split_at(tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len()));
        let (at, first) = (self.elements.len(), self.attributes.len());
        while let Some((pair, after)) = next_attribute(says) {
            self.attributes.push(pair);
            says = after;
        }
        self.elements.push(Element { name: local(name), attributes: [first, self.attributes.len()], end: at + 1 });
        if let Some(id) = self.attribute(at, "id") {
            self.named.entry(id).or_insert(at);
        }
        if !tag.ends_with('/') {
            open.push(at);
        }
        open
    }

    /// Takes in a style sheet: rules `selector, selector { name: value; … }`.
    fn sheet(&mut self, sheet: &'a str) {
        for rule in sheet.split('}') {
            let Some((whom, says)) = rule.split_once('{') else { continue };
            self.rules.extend(whom.split(',').map(|whom| (whom.trim().trim_start_matches("<![CDATA[").trim(), says)));
        }
    }

    /// Whether a selector matches an element: by name, `.class`, `#id`, a name with either, or
    /// `*`; any other selector matches nothing.
    fn is_for(&self, whom: &str, at: usize) -> bool {
        let (whom, class) = whom.split_once('.').map_or((whom, None), |(whom, class)| (whom, Some(class)));
        let (name, id) = whom.split_once('#').map_or((whom, None), |(name, id)| (name, Some(id)));
        let named = (name.is_empty() && (class.is_some() || id.is_some())) || name == "*" || name == self.elements[at].name;
        let classed = class.is_none_or(|class| self.attribute(at, "class").is_some_and(|classes| classes.split_whitespace().any(|own| own == class)));
        named && classed && id.is_none_or(|id| self.attribute(at, "id") == Some(id))
    }

    /// An attribute of an element, if present.
    pub(crate) fn attribute(&self, at: usize, name: &str) -> Option<&'a str> {
        let [first, last] = self.elements[at].attributes;
        self.attributes[first..last].iter().find(|(attributes, _)| *attributes == name).map(|(_, value)| *value)
    }

    /// Where a slice of the file's text lies in it (nowhere, for text not from the file).
    pub(crate) fn place_of(&self, piece: &str) -> [u32; 2] {
        let from = (piece.as_ptr() as usize).wrapping_sub(self.text.as_ptr() as usize);
        match from.checked_add(piece.len()).is_some_and(|end| end <= self.text.len()) {
            true => [from as u32, (from + piece.len()) as u32],
            false => [0; 2],
        }
    }

    /// The element's children, in order (not deeper descendants).
    pub(crate) fn inside(&self, at: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.elements[at].end;
        std::iter::successors(Some(at + 1).filter(|first| *first < end), move |child| Some(self.elements[*child].end).filter(|next| *next < end))
    }

    /// How each of red, green, blue and alpha is transformed, as the `feFuncR`, `feFuncG`,
    /// `feFuncB` and `feFuncA` inside an element say (the last of each; identity where none).
    fn functions(&self, at: usize) -> [Function; 4] {
        let mut functions = [Function::Same, Function::Same, Function::Same, Function::Same];
        for child in self.inside(at) {
            let Some(n) = ["feFuncR", "feFuncG", "feFuncB", "feFuncA"].iter().position(|name| *name == self.elements[child].name) else { continue };
            let one = |name: &str, fallback: f32| self.attribute(child, name).and_then(number).map_or(fallback, |(number, _)| number);
            let values: Vec<f32> = self.attribute(child, "tableValues").map(|text| numbers(text).collect()).unwrap_or_default();
            functions[n] = match self.attribute(child, "type").map(str::trim) {
                Some("table") => Function::Table(values),
                Some("discrete") => Function::Discrete(values),
                Some("linear") => Function::Linear(one("slope", 1.0), one("intercept", 0.0)),
                Some("gamma") => Function::Gamma(one("amplitude", 1.0), one("exponent", 1.0), one("offset", 0.0)),
                _ => Function::Same,
            };
        }
        functions
    }

    /// The element another one references (`href="#id"`), if any.
    pub(crate) fn linked(&self, at: usize) -> Option<usize> {
        let link = self.attribute(at, "href").or_else(|| self.attribute(at, "xlink:href"))?;
        self.named.get(link.trim().strip_prefix('#')?).copied()
    }

    /// A property an element sets itself, in its `style` attribute or as an attribute (style
    /// rules aside): for those read only where used.
    pub(crate) fn own_property(&self, at: usize, name: &str) -> Option<&'a str> {
        let declared = self.attribute(at, "style").and_then(|declarations| declarations.split(';').filter_map(|one| one.split_once(':')).rfind(|(said, _)| said.trim() == name));
        declared.map(|(_, value)| value.trim()).or_else(|| self.attribute(at, name).map(str::trim))
    }
}

/// A tag's length from after its `<` to before the `>` ending it. A comment ends at `-->` only;
/// elsewhere a quoted `>` does not end it.
fn tag_length(tag: &str) -> usize {
    let ended_by = |end: &str| tag.find(end).map_or(tag.len(), |at| at + end.len() - 1);
    let special = [("!--", "-->"), ("![CDATA[", "]]>"), ("?", "?>")].into_iter().find(|(start, _)| tag.starts_with(start));
    if let Some((_, end)) = special {
        return ended_by(end);
    }
    // A doctype may have an internal subset in brackets, with tags of its own.
    let listed = tag.starts_with('!') && tag.find(['[', '>']).is_some_and(|at| tag[at..].starts_with('['));
    let from = if listed { tag.find(']').unwrap_or(tag.len()) } else { 0 };
    let mut quote = 0;
    for (at, byte) in tag.bytes().enumerate().skip(from) {
        match byte {
            b'>' if quote == 0 => return at,
            b'"' | b'\'' if quote == 0 => quote = byte,
            _ if byte == quote => quote = 0,
            _ => {}
        }
    }
    tag.len()
}

/// The next attribute in a tag, `name="value"`, and the rest of the tag.
fn next_attribute(tag: &str) -> Option<((&str, &str), &str)> {
    let (name, rest) = tag.split_once('=')?;
    let rest = rest.trim_start();
    let quote = rest.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
    let (value, rest) = rest[1..].split_once(quote)?;
    // The name is the last word before `=`: a bare word says nothing.
    Some(((name.split_whitespace().last()?.trim_start_matches('/'), value), rest))
}

/// What a file says a shape is filled or stroked with.
#[derive(Clone, Copy, PartialEq)]
enum Paint {
    None,
    /// A colour of the file's own.
    Own(Color),
    /// `currentColor`: the colour given where the drawing is used.
    CurrentColor,
    /// A gradient: its element.
    Gradient(usize),
    /// A pattern: its element.
    Tiles(usize),
}

/// A fill or stroke given in a word: `none`, `currentColor`, or a colour.
fn plain(text: &str) -> Option<Paint> {
    match text.trim().to_ascii_lowercase().as_str() {
        "none" | "transparent" => Some(Paint::None),
        "currentcolor" => Some(Paint::CurrentColor),
        other => color(other).map(Paint::Own),
    }
}

/// A stroke's pen: width, caps and joins, miter limit (as a multiple of the width), dashes
/// (which of the kept dash arrays, and the offset), and `vector-effect: non-scaling-stroke`
/// (`fixed`): width and dashes then stay in pixels of the drawing at its stated size, however
/// the picture and shape are scaled.
#[derive(Clone, Copy, PartialEq)]
struct Pen {
    width: f32,
    cap: Cap,
    join: Join,
    miter: f32,
    dashes: [u32; 2],
    offset: f32,
    fixed: bool,
}

impl Pen {
    /// The library's pen with these dashes: miter joins where the file allows the miter.
    fn drawn<'a>(&self, dashes: &'a [f32]) -> pen::Pen<'a> {
        let join = if let Join::Mitre(_) = self.join { Join::Mitre(self.miter) } else { self.join };
        pen::Pen { width: self.width, cap: self.cap, join, dashes, offset: self.offset }
    }
}

/// How a shape is drawn, as its element and ancestors say.
#[derive(Clone, Copy)]
struct Style {
    /// Its fill and stroke paints.
    paints: [Paint; 2],
    /// Their opacities.
    solid: [f32; 2],
    /// Its opacity from ancestor groups not composited as one.
    faded: f32,
    even_odd: bool,
    pen: Pen,
    /// What `currentColor` is, where the file sets `color` itself.
    ink: Option<Color>,
    hidden: bool,
    /// Whether its stroke is painted under its fill.
    under: bool,
    /// Its transform to the drawing's units.
    table: Table,
    /// Whether clip paths inside it use the even-odd rule.
    clip_even_odd: bool,
    /// Whether it is part of a clip path: then only its outline counts, filled opaque.
    clipping: bool,
    /// Whether its shapes are drawn without anti-aliasing (`shape-rendering` `crispEdges` or
    /// `optimizeSpeed`).
    crisp: bool,
    /// The kept viewport box aligned with the picture it is clipped to, if any.
    boxed: Option<u32>,
    /// The markers at its outline's start, corners and end.
    markers: [Option<usize>; 3],
    /// Its text: font family (where its name is in the file), size, weight, italic, anchor (0
    /// start, 0.5 middle, 1 end), and letter spacing.
    family: [u32; 2],
    font_size: f32,
    weight: u16,
    italic: bool,
    anchor: f32,
    spacing: f32,
    /// Its own opacity. (This and what follows applies to the element only and is not
    /// inherited.)
    opacity: f32,
    /// Whether it is left out with all it holds.
    gone: bool,
    /// As a gradient stop: its colour and opacity.
    stop: (Paint, f32),
    /// The clip path and mask it is clipped by, by element.
    clip: Option<usize>,
    mask: Option<usize>,
    /// Whether it shows what lies outside its box, as a nested viewport.
    overflows: bool,
    /// Its filter list: where in the file it is.
    filter: Option<[u32; 2]>,
    /// A flood filter step's colour and opacity.
    flood: (Paint, f32),
    /// The colour space filter steps work in. (Unlike what is just before it, this is
    /// inherited.)
    space: Space,
    /// How much of the picture a square unit is, after its transform; and how much of the
    /// picture its target block is (the picture itself, or a pattern tile): its drawing cost,
    /// read before the picture's size is known.
    share: [f32; 2],
}

impl Default for Style {
    fn default() -> Style {
        let pen = Pen { width: 1.0, cap: Cap::Flat, join: Join::Mitre(4.0), miter: 4.0, dashes: [0, 0], offset: 0.0, fixed: false };
        let black = Paint::Own(Color::BLACK);
        Style {
            paints: [black, Paint::None],
            solid: [1.0; 2],
            faded: 1.0,
            even_odd: false,
            pen,
            ink: None,
            hidden: false,
            under: false,
            table: SAME,
            clip_even_odd: false,
            clipping: false,
            crisp: false,
            boxed: None,
            markers: [None; 3],
            family: [0; 2],
            font_size: 16.0,
            weight: 400,
            italic: false,
            anchor: 0.0,
            spacing: 0.0,
            opacity: 1.0,
            gone: false,
            stop: (black, 1.0),
            clip: None,
            mask: None,
            overflows: false,
            filter: None,
            flood: (black, 1.0),
            space: Space::Linear,
            share: [0.0, 1.0],
        }
    }
}

/// What a shape is finally filled or stroked with.
#[derive(Clone, Copy, PartialEq)]
enum Ink {
    None,
    /// A colour of the file's own.
    Own(Color),
    /// The box's colour, at this opacity.
    CurrentColor(f32),
    /// A gradient: which kept one, and opacity.
    Gradient(u32, f32),
    /// A pattern: which kept tile, and opacity. (The tile is drawn just before the shape, on a
    /// block of its own on top while the shape is.)
    Tiles(u32, f32),
    /// An image: which kept picture, and opacity.
    Picture(u32, f32),
}

/// One shape of a drawing: which kept outline, its transform to drawing units, its fill and
/// fill rule, and its stroke with which kept pen.
struct Shape {
    path: u32,
    table: Table,
    fill: Ink,
    even_odd: bool,
    line: Ink,
    pen: u32,
    /// The viewport box aligned with the picture it is clipped to: which kept box.
    boxed: Option<u32>,
    /// Whether it is drawn without anti-aliasing (`shape-rendering`).
    crisp: bool,
}

/// One drawing instruction, in order.
enum Item {
    Shape(Shape),
    /// What follows is drawn on a block of its own,
    Begin,
    /// and here composited over what came before at this opacity.
    End(f32),
    /// Or the top block is a coverage mask for the one under it, and is dropped.
    Cut(Keep),
    /// What follows, up to the shape it fills, is a pattern tile (which kept one), drawn on a
    /// block as large as it shows.
    Tile(u32),
    /// The top block is filtered (which kept filter).
    Filter(u32),
}

/// How much of a block another keeps: by alpha (a clip path), or by luminance times alpha (a
/// mask).
#[derive(Clone, Copy)]
enum Keep {
    Solid,
    Bright,
}

/// What a block is clipped by once drawn: a clip path or mask (by element), a viewport box
/// (left, top, width, height in the block's units), or a kept box aligned with the picture
/// clipping its surroundings (filtered content is clipped after the filter).
#[derive(Clone, Copy)]
enum Cut {
    Clip(usize),
    Mask(usize),
    Box([f32; 4]),
    DrawingBox(u32),
}

/// A pattern as one shape is filled with it: tiles edge to edge, each a drawing of its own.
struct Tile {
    /// From tile units to drawing units: the tile origin and rotation.
    table: Table,
    /// Tile size, in those units.
    size: [f32; 2],
    /// From its content units to tile units.
    inner: Table,
}

impl Tile {
    /// A block to draw a tile on, as many pixels as it covers on `under` (at most 1024 a side).
    fn block(&self, under: &Block) -> Block {
        let shown = both(&under.fit, &self.table);
        let side = |n: usize| ((shown[2 * n].hypot(shown[2 * n + 1]) * self.size[n]).ceil() as usize).clamp(1, 1024);
        let [wide, high] = [side(0), side(1)];
        let fit = both(&[wide as f32 / self.size[0], 0.0, 0.0, high as f32 / self.size[1], 0.0, 0.0], &self.inner);
        Block { pixels: vec![0; wide * high], wide, top: 0, fit, zoom: under.zoom }
    }
}

/// A block of pixels a drawing or part of it is drawn on: `wide` per row, its first row being
/// picture row `top`; the transform from drawing units to its pixels; and its pixels per
/// drawing pixel at the drawing's stated size.
pub(crate) struct Block {
    pub(crate) pixels: Vec<u32>,
    pub(crate) wide: usize,
    pub(crate) top: i32,
    pub(crate) fit: Table,
    zoom: f32,
}

impl Block {
    /// An empty block like this one.
    fn blank(&self) -> Block {
        Block { pixels: vec![0; self.pixels.len()], ..*self }
    }
}

/// `preserveAspectRatio`: stretch to fill, else slice (fill and crop) or meet; and the
/// alignment across and down, 0 start to 1 end.
#[derive(Clone, Copy)]
struct Aspect {
    stretched: bool,
    cut: bool,
    sits: [f32; 2],
}

impl Aspect {
    /// As written: `none`, or an alignment (`xMidYMid`) then `meet` or `slice`.
    fn read(value: &str) -> Aspect {
        let sits = |low: &str, high: &str| [0.5, 0.0, 1.0][value.contains(low) as usize + 2 * (value.contains(high) && !value.contains(low)) as usize];
        Aspect { stretched: value.contains("none"), cut: value.contains("slice"), sits: [sits("xMin", "xMax"), sits("YMin", "YMax")] }
    }

    /// The transform bringing `view` of a space (left, top, width, height) into a box of `size`
    /// at its origin.
    fn fit(&self, [left, top, across, down]: [f32; 4], [wide, high]: [f32; 2]) -> Table {
        let (x, y) = (wide / across, high / down);
        let (x, y) = match (self.stretched, self.cut) {
            (true, _) => (x, y),
            (_, false) => (x.min(y), x.min(y)),
            (_, true) => (x.max(y), x.max(y)),
        };
        [x, 0.0, 0.0, y, (wide - across * x) * self.sits[0] - left * x, (high - down * y) * self.sits[1] - top * y]
    }
}

/// A gradient's geometry.
#[derive(Clone, Copy)]
enum Way {
    /// Linear: from one point to another (x and y of each).
    Line([f32; 4]),
    /// Radial: out to a circle (centre x, y and radius) from a focus inside it.
    Round([f32; 5]),
}

/// `spreadMethod`: before the first stop and after the last.
#[derive(Clone, Copy)]
enum Spread {
    /// The end colours.
    Pad,
    /// The stops again from the first,
    Repeat,
    /// or there and back.
    Reflect,
}

/// A gradient as one shape is filled with it.
#[derive(Clone, Copy)]
struct Gradient {
    /// Which kept stops: colour and offset each.
    stops: [u32; 2],
    way: Way,
    spread: Spread,
    /// From its geometry units to the shape's.
    table: Table,
    /// Whether those units are the shape's bounding box (`objectBoundingBox`).
    boxed: bool,
}

impl Gradient {
    /// Where a point lies along the gradient: 0 at its start, 1 at its end.
    fn along(&self, p: Point) -> f32 {
        let along = match self.way {
            Way::Line([x1, y1, x2, y2]) => ((p.x - x1) * (x2 - x1) + (p.y - y1) * (y2 - y1)) / ((x2 - x1) * (x2 - x1) + (y2 - y1) * (y2 - y1)),
            // The circles grow from the focus to the circle, their centres moving between the
            // two: which one is the point on?
            Way::Round([cx, cy, r, fx, fy]) => {
                let ((ex, ey), (dx, dy)) = ((cx - fx, cy - fy), (p.x - fx, p.y - fy));
                let (room, toward) = (r * r - ex * ex - ey * ey, dx * ex + dy * ey);
                (-toward + (toward * toward + room * (dx * dx + dy * dy)).sqrt()) / room
            }
        };
        match self.spread {
            Spread::Pad => along,
            Spread::Repeat => along.rem_euclid(1.0),
            Spread::Reflect => 1.0 - (along.rem_euclid(2.0) - 1.0).abs(),
        }
    }
}

/// A cached picture of a drawing: size, `currentColor` used, pixels.
struct Drawn {
    size: [usize; 2],
    current_color: Color,
    pixels: Vec<u32>,
}

/// An image in a drawing, premultiplied as a canvas keeps it, `wide` per row. (The shape it
/// fills is in its pixels.)
struct Picture {
    pixels: Vec<u32>,
    wide: usize,
}

/// What a drawing holds besides shapes, if any: its filters and images, the drawings waiting to
/// be read once the file's own is (see `Reading::read_later`), and where in a window it was
/// last shown, where noted (see `ParsedDrawing::show_noted`).
#[derive(Default)]
struct More {
    filters: Vec<Filter>,
    pictures: Vec<Picture>,
    later: Vec<Later>,
    shown: Option<Rect>,
}

/// What lies where in a drawing, to find the element under the pointer: each shape kept for an
/// element, in drawing order, and the `<use>` elements that brought them (each shape's range of
/// them, innermost first).
#[derive(Default)]
pub(crate) struct Hits {
    shapes: Vec<Hit>,
    uses: Vec<u32>,
}

/// A shape of an element as it takes the pointer: its outline (which kept one) and transform,
/// whether its fill counts (with its rule), its stroke's width (0: none), and how far it reaches
/// (left, top, right, bottom in drawing units, its stroke included).
#[derive(Clone, Copy)]
struct Hit {
    element: u32,
    uses: [u32; 2],
    path: u32,
    table: Table,
    filled: bool,
    even_odd: bool,
    stroke: f32,
    reach: [f32; 4],
}

/// Notes the shapes a reading kept for an element (see `Reading::note_hits`).
type Noting<'a> = for<'b> fn(&'b mut Reading<'a>, usize, usize, &[Walk]);

/// What an image element holds, as read (see `Reading::picture_of`): a kept picture (which), an
/// SVG file's text to read once the file's own is, or nothing to show.
#[derive(Clone)]
enum Pictured {
    Picture(u32),
    Drawing(Rc<str>),
    Nothing,
}

/// A drawing read once the file's own is (see `Reading::read_later`): an element a `feImage`
/// draws, or an SVG file an image holds. Its items go in at `at` among the items of the drawing
/// it was met in.
struct Later {
    at: usize,
    brought: Brought,
    /// From its box's units to the drawing's, and the share of the picture a square unit of the
    /// drawing and its block are (see `Style::share`).
    table: Table,
    share: [f32; 2],
    /// A file's box: left, top, width and height, each where given (else its own size), and
    /// how its view fits in it; the opacity it is composited at; and the viewport box clipping
    /// it (which kept one).
    place: [Option<f32>; 4],
    aspect: Aspect,
    solid: f32,
    boxed: Option<u32>,
    /// How many drawings deep it is brought: one for those the file brings itself.
    depth: u8,
}

/// What a waiting drawing is: an element of the file, or another file's text.
enum Brought {
    Element(usize),
    File(Rc<str>),
}

/// How many drawings deep images and `feImage` may bring drawings: a file holding itself goes no
/// further.
const BROUGHT: u8 = 4;

/// A drawing as kept: everything it is drawn from, and cached pictures at the sizes last shown.
pub(crate) struct ParsedDrawing {
    /// Its stated size, in units.
    size: [f32; 2],
    /// Its `viewBox`: left, top, width, height.
    view: [f32; 4],
    /// How the view fits a box of another shape.
    aspect: Aspect,
    items: Vec<Item>,
    paths: Vec<Path>,
    pens: Vec<Pen>,
    /// All dash and gap lengths.
    dashes: Vec<f32>,
    gradients: Vec<Gradient>,
    stops: Vec<(f32, Color)>,
    tiles: Vec<Tile>,
    /// Boxes aligned with the picture that shapes are clipped to: left, top, right, bottom of
    /// each, in drawing units.
    boxes: Vec<[f32; 4]>,
    /// Its filters and images, if any (`More`). (Held as `dyn Any` so a program that never
    /// reads them carries none of the code that drops them either: only the readers create it,
    /// and a program carries those once it asks.)
    more: Option<Box<dyn std::any::Any>>,
    /// Whether any of it uses `currentColor`.
    uses_current_color: bool,
    drawn: Vec<Drawn>,
}

/// State while reading a file: the file, what is kept so far, and cached lookups.
struct Reading<'a> {
    file: File<'a>,
    parsed: ParsedDrawing,
    /// Elements to kept outlines.
    outlined: HashMap<usize, u32>,
    /// Gradients read, by element; none for one with no stops.
    gradients_read: HashMap<usize, Option<Gradient>>,
    /// Images read, by element.
    pictured: HashMap<usize, Pictured>,
    /// System fonts letters were found in, each with the face and letter it was found for (0:
    /// the face's own font): which font file.
    fonts: Vec<(Face, char, Option<u32>)>,
    /// Their font files, each once: the system writes many letters in the same one.
    files: Vec<FontFile>,
    /// The font faces texts name, each with where its family name is in the file, its weight,
    /// and italic.
    faces: Vec<([u32; 2], u16, bool, Face)>,
    /// How many outline segments the shapes kept so far come to, each counted as often as kept:
    /// their drawing cost.
    pieces: usize,
    /// Reading steps taken: per element, one per walk it lies in, per attribute and per style
    /// rule tried; more per letter set. Reading's cost.
    steps: usize,
    /// System font lookups made.
    lookups: usize,
    /// Full-picture passes drawing what is kept so far costs: each shape its bounding box's
    /// share (more for per-pixel paints), each block begun its share.
    passes: f32,
    /// Shape vertices that get markers: which marker, the point, the path direction there (an
    /// angle), and whether it is the start.
    corners: Vec<(usize, Point, f32, bool)>,
    /// The filter reader, where filters are read: in a file once a program asked (see
    /// `enable_filters`), not in glyphs.
    filtering: Option<&'static Filters>,
    /// The file length its budget is reckoned from (see `spent`): the outermost file's, for a
    /// drawing it brings.
    budget: usize,
    /// How many drawings deep this one is brought (see `Later::depth`), and what reads those
    /// waiting once the walk is done, where any wait.
    depth: u8,
    later: Option<fn(&mut Reading<'a>)>,
    /// What notes the shapes kept for each element, where asked (see `ParsedDrawing::read_with_hits`),
    /// and what it noted.
    noting: Option<Noting<'a>>,
    hits: Hits,
}

/// A text piece (the text element itself, a `<tspan>` or a `<textPath>`) while its letters are
/// set: its element, style, parent piece, its letters' given positions (x, y, dx, dy) by letter
/// index, how many letters are given, a `<textPath>`'s path (in the text's units) with where
/// along it the text starts, and its outline so far.
struct Piece {
    at: usize,
    style: Style,
    around: usize,
    places: [Vec<f32>; 4],
    letters: usize,
    path: Option<(Track, f32)>,
    outline: Path,
}

/// A text path laid flat: its segments, each with how far along the path it starts, and the
/// path's length. A glyph is placed by a search, not a walk of the whole path.
#[derive(Default)]
pub(crate) struct Track {
    segments: Vec<(Point, Point, f32)>,
    pub(crate) length: f32,
}

impl Track {
    /// A path's segments, its curves flattened to within a 16,384th of its size.
    pub(crate) fn of(path: &Path) -> Track {
        let Rect { w, h, .. } = path.bounds();
        let mut track = Track::default();
        path.flatten(1.0, pt(0.0, 0.0), (w.max(h) / 256.0).max(1e-6), |a, b| {
            let long = (b.x - a.x).hypot(b.y - a.y);
            if long > 0.0 {
                track.segments.push((a, b, track.length));
                track.length += long;
            }
        });
        track
    }

    /// The point `along` the path, and its direction there (cosine, sine): the start before
    /// it, the end past it. Nothing for an empty track.
    pub(crate) fn point_at(&self, along: f32) -> Option<(Point, [f32; 2])> {
        let at = self.segments.partition_point(|(.., from)| *from <= along).saturating_sub(1);
        let (a, b, from) = *self.segments.get(at)?;
        let long = (b.x - a.x).hypot(b.y - a.y);
        let t = ((along - from) / long).clamp(0.0, 1.0);
        Some((pt(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), [(b.x - a.x) / long, (b.y - a.y) / long]))
    }
}

/// A letter of a text as given: its piece, and its position as the pieces give it (x, y, dx,
/// dy, each where given).
struct Given {
    letter: char,
    piece: usize,
    at: [Option<f32>; 4],
}

/// A glyph as set in a chunk: its piece, its outline (the baseline at the pen's height), and
/// where the middle of its advance lies along the line.
struct Set {
    piece: usize,
    outline: Path,
    middle: f32,
}

/// A chunk's glyphs as set from where it starts: the glyphs, the pen once all are set, and the
/// last letter's spacing (no part of the chunk, as nothing follows it).
struct Line {
    set: Vec<Set>,
    pen: Point,
    apart: f32,
}

/// How a filter's steps are read (see `Reading::step`): lengths in bounding-box fractions
/// (`in_box`) of the box (left, top, width, height), else in units; the filter region; the
/// results made so far, by name; and the images its steps take (see `Imaged`).
struct Primitives<'a> {
    in_box: bool,
    square: [f32; 4],
    region: [f64; 4],
    results: Vec<(&'a str, usize)>,
    images: Vec<Imaged>,
}

impl Primitives<'_> {
    /// Which of the images is the element's fill (`which` 0) or stroke (1) paint: one, however
    /// many steps take it.
    fn paint(&mut self, which: usize) -> usize {
        let found = self.images.iter().position(|image| matches!(image, Imaged::Paint(paint) if *paint == which));
        found.unwrap_or_else(|| {
            self.images.push(Imaged::Paint(which));
            self.images.len() - 1
        })
    }
}

/// An image a filter's steps take: what a `feImage` step draws (an element of the file, with
/// the transform from its units to the filter's; or what its own `href` names, by its element,
/// in its box: left, top, width, height in the filter's units); or the element's fill (0) or
/// stroke (1) paint (`FillPaint`, `StrokePaint`).
enum Imaged {
    Element(usize, Table),
    Href(usize, [f32; 4]),
    Paint(usize),
}

/// A range of elements being walked.
#[derive(Clone, Copy)]
struct Walk {
    /// The next, and the first past the range.
    at: usize,
    end: usize,
    /// The style of their surroundings.
    style: Style,
    /// The element a `<use>` brought here, if this is that; or the pattern or marker being
    /// drawn.
    used: Option<usize>,
    /// The opacity of the block they are composited onto, where they get one: a
    /// semi-transparent or clipped group.
    layer: Option<f32>,
    /// The filter list that block goes through once drawn (where in the file),
    filter: Option<[u32; 2]>,
    /// and what clips it after that, in order: clip path, mask, viewport box, and the kept box
    /// aligned with the picture clipping its surroundings.
    cuts: [Option<Cut>; 4],
    /// Where this is itself what clips the block under it: how much it keeps.
    keep: Option<Keep>,
    /// The block's first item and the first after its drawing (before its clips): the shapes
    /// between are what a bounding-box clip path or mask fits to.
    first: usize,
    last: usize,
    /// The shape walked again as itself: once its block and clips are begun (`own`), or its
    /// pattern tiles drawn (`tiled`, with the tiles for fill and stroke).
    own: Option<usize>,
    tiled: Option<(usize, [Option<u32>; 2])>,
    /// The tile to begin once this is drawn: the next pattern's, for a shape filled with two.
    then: Option<u32>,
    /// A `<use>`'s width and height for what it brings, where given.
    use_size: [Option<f32>; 2],
    /// The units the block's clips are in: the group's, or those of the shape walked again as
    /// itself.
    units: Table,
    /// Whether it walks a shape's markers (`Reading::corners` from `at` to `end`), not
    /// elements: each marker's walk begins once the previous ends, so thousands never sit side
    /// by side among the walks.
    marking: bool,
}

impl Walk {
    /// A walk through elements `at` to `end` with `style`, nothing else.
    fn through(at: usize, end: usize, style: Style) -> Walk {
        Walk {
            at,
            end,
            style,
            used: None,
            layer: None,
            filter: None,
            cuts: [None; 4],
            keep: None,
            first: 0,
            last: usize::MAX,
            own: None,
            tiled: None,
            then: None,
            use_size: [None; 2],
            units: style.table,
            marking: false,
        }
    }
}

impl ParsedDrawing {
    /// Reads a file. Nothing if it holds no drawing: no `<svg>`, or one of no size.
    pub(crate) fn read(text: &str) -> Option<ParsedDrawing> {
        ParsedDrawing::read_part(text, None)
    }

    /// Reads one glyph's drawing from a font's `SVG ` table document: the element with id `id`,
    /// as if it were all the file drew (with what it uses of the rest). The space is the
    /// font's: the origin is the glyph's pen on the baseline, and it is `em` units wide and
    /// high, unless it says how much of it that is. Nothing without such an element.
    pub(crate) fn glyph(text: &str, id: &str, em: f32) -> Option<ParsedDrawing> {
        ParsedDrawing::read_part(text, Some((id, em)))
    }

    /// Reads a file, or with `glyph` the glyph part of it (see `glyph`).
    fn read_part(text: &str, glyph: Option<(&str, f32)>) -> Option<ParsedDrawing> {
        Reading::of(text, glyph).map(|reading| reading.parsed)
    }

    /// Reads a file, noting what lies where in it (see `element_at`).
    pub(crate) fn read_with_hits(text: &str) -> Option<(ParsedDrawing, Hits)> {
        Reading::noted(text).map(|reading| (reading.parsed, reading.hits))
    }

    /// Shows the drawing as `show` does, noting where (see `element_at`).
    pub(crate) fn show_noted(&mut self, within: Rect, current_color: Color, canvas: &mut Canvas) {
        self.note(within);
        self.show(within, current_color, canvas);
    }

    /// Notes where in a window the drawing shows.
    pub(crate) fn note(&mut self, within: Rect) {
        self.more().shown = Some(within);
    }

    /// Where in a window the drawing last showed, where noted.
    pub(crate) fn noted(&self) -> Option<Rect> {
        self.held()?.shown
    }

    /// The element whose shape lies at point `at` of the window (where the drawing was last
    /// noted showing), topmost first, of those `takes` says take the pointer (given an element
    /// and the `<use>` elements that brought it): it, and those `<use>` elements. A shape takes
    /// the pointer where its fill or stroke would paint, clips and masks aside.
    pub(crate) fn element_at<'h>(&self, hits: &'h Hits, at: Point, takes: impl Fn(usize, &[u32]) -> bool) -> Option<(usize, &'h [u32])> {
        let within = self.noted()?;
        let back = undone(&both(&[1.0, 0.0, 0.0, 1.0, within.x, within.y], &self.fit([within.w, within.h])))?;
        let point = through(&back, at);
        let uses = |hit: &Hit| &hits.uses[hit.uses[0] as usize..hit.uses[1] as usize];
        let near = |hit: &&Hit| point.x >= hit.reach[0] && point.y >= hit.reach[1] && point.x <= hit.reach[2] && point.y <= hit.reach[3];
        let hit = hits.shapes.iter().rev().filter(near).find(|hit| self.lies_in(hit, point) && takes(hit.element as usize, uses(hit)))?;
        Some((hit.element as usize, uses(hit)))
    }

    /// Whether a point (in drawing units) lies in a shape's fill, where it is filled, or within
    /// its stroke.
    fn lies_in(&self, hit: &Hit, point: Point) -> bool {
        let (Some(back), Some(path)) = (undone(&hit.table), self.paths.get(hit.path as usize)) else { return false };
        let inner = through(&back, point);
        (hit.filled && inside(path, inner, hit.even_odd)) || (hit.stroke > 0.0 && path.distance(inner) <= hit.stroke / 2.0)
    }

    /// What it holds besides shapes, if any.
    fn held(&self) -> Option<&More> {
        self.more.as_ref()?.downcast_ref()
    }

    /// What it holds besides shapes, created now if none yet.
    fn more(&mut self) -> &mut More {
        if !self.more.as_ref().is_some_and(|more| more.is::<More>()) {
            self.more = Some(Box::new(More::default()));
        }
        // It is never anything else.
        self.more.as_mut().and_then(|more| more.downcast_mut()).unwrap_or_else(|| unreachable!())
    }

    /// The drawing's stated size, in units.
    pub(crate) fn size(&self) -> [f32; 2] {
        self.size
    }

    /// The `currentColor` as the drawing takes it: none for a drawing that does not use it, so
    /// one picture serves any colour.
    pub(crate) fn takes(&self, current_color: Color) -> Color {
        if self.uses_current_color { current_color } else { Color::NONE }
    }

    /// The transform bringing the drawing's space into a picture of `size` pixels.
    fn fit(&self, size: [f32; 2]) -> Table {
        self.aspect.fit(self.view, size)
    }

    /// Shows the drawing in `within` of a window with `current_color` as `currentColor`: the
    /// cached picture at that size, drawn first if missing.
    pub(crate) fn show(&mut self, within: Rect, current_color: Color, canvas: &mut Canvas) {
        let scale = canvas.scale();
        let corner = |x: f32, y: f32| ((x * scale).round() as i32, (y * scale).round() as i32);
        let ((left, top), (right, bottom)) = (corner(within.x, within.y), corner(within.right(), within.bottom()));
        let size = [(right - left).max(0) as usize, (bottom - top).max(0) as usize];
        // One picture for any colour, if the drawing does not use it.
        let current_color = self.takes(current_color);
        if size[0].saturating_mul(size[1]) > MOST {
            let [_, first, _, last] = canvas.clip_pixels();
            let rows = [first.saturating_sub(top).clamp(0, size[1] as i32) as usize, last.saturating_sub(top).clamp(0, size[1] as i32) as usize];
            let pixels = if rows[0] < rows[1] { self.draw(size, rows, current_color) } else { Vec::new() };
            return canvas.pixels(&pixels, size[0], [left, top + rows[0] as i32], false);
        }
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        let kept = self.drawn.iter().position(|drawn| (drawn.size, drawn.current_color) == (size, current_color));
        let at = kept.unwrap_or_else(|| {
            // The oldest cached picture makes room once `KEPT` are cached.
            self.drawn.drain(..self.drawn.len().saturating_sub(KEPT - 1));
            self.drawn.push(Drawn { size, current_color, pixels: self.draw(size, [0, size[1]], current_color) });
            self.drawn.len() - 1
        });
        canvas.pixels(&self.drawn[at].pixels, size[0], [left, top], false);
    }

    /// Rows `rows[0]` to `rows[1]` of the drawing at `size` pixels with `current_color` as
    /// `currentColor`: premultiplied pixels.
    fn draw(&self, size: [usize; 2], rows: [usize; 2], current_color: Color) -> Vec<u32> {
        self.drawn_by(self.fit(size.map(|side| side as f32)), size[0], rows, current_color)
    }

    /// A glyph drawing's pixels (see `glyph`), `size` pixels, where `place` maps font units to
    /// them.
    pub(crate) fn glyph_pixels(&self, place: &Table, size: [usize; 2], current_color: Color) -> Vec<u32> {
        self.drawn_by(both(place, &self.fit(self.size)), size[0], [0, size[1]], current_color)
    }

    /// Rows `rows[0]` to `rows[1]` of the drawing as `fit` brings it into a picture `wide`
    /// pixels wide.
    fn drawn_by(&self, fit: Table, wide: usize, rows: [usize; 2], current_color: Color) -> Vec<u32> {
        // Picture pixels per drawing pixel at the drawing's stated size: a non-scaling stroke
        // is that many pixels per one of its own.
        let zoom = ((fit[0] * fit[3]).abs() * self.view[2] * self.view[3] / (self.size[0] * self.size[1])).sqrt();
        // Offscreen groups, clips and tiles render to their own block first: innermost on top.
        let mut blocks = vec![Block { pixels: vec![0; wide * (rows[1] - rows[0])], wide, top: rows[0] as i32, fit, zoom }];
        let mut work = Work::default();
        for item in &self.items {
            let top = blocks.len() - 1;
            match item {
                Item::Shape(shape) => self.lay(shape, current_color, &mut blocks, &mut work),
                Item::Begin => blocks.push(blocks[top].blank()),
                Item::End(solid) => merge(&mut blocks, *solid),
                Item::Cut(keep) => cut(&mut blocks, *keep),
                Item::Tile(tile) => blocks.push(self.tiles[*tile as usize].block(&blocks[top])),
                Item::Filter(filter) => self.filter(*filter, &mut blocks),
            }
        }
        blocks.swap_remove(0).pixels
    }

    /// Filters the top block's content through a kept filter. (Only a program that asked for
    /// filters keeps any: see `enable_filters`.)
    fn filter(&self, filter: u32, blocks: &mut Vec<Block>) {
        if let Some(filters) = FILTERS.get() {
            let Some(filter) = self.held().and_then(|more| more.filters.get(filter as usize)) else { return };
            (filters.apply)(filter, blocks);
        }
    }

    /// Draws one shape on the top block: fill, then stroke. (Where filled with patterns, their
    /// tiles lie on top: the stroke's over the fill's.)
    fn lay(&self, shape: &Shape, current_color: Color, blocks: &mut Vec<Block>, work: &mut Work) {
        let tiled = |ink: Ink| matches!(ink, Ink::Tiles(..));
        let line_tile = if tiled(shape.line) && blocks.len() > 1 { blocks.pop() } else { None };
        let fill_tile = if tiled(shape.fill) && blocks.len() > 1 { blocks.pop() } else { None };
        let Some(block) = blocks.last_mut() else { return };
        let (fit, zoom, wide, top) = (block.fit, block.zoom, block.wide, block.top);
        let table = both(&fit, &shape.table);
        // Pixels per shape unit. It is processed in units that many times finer, and `FINER`
        // again: each a fraction of a pixel, for flattening curves.
        let fine = (table[0] * table[3] - table[1] * table[2]).abs().sqrt() * FINER;
        if !fine.is_normal() {
            return;
        }
        let (path, pen, to_pixels) = (&self.paths[shape.path as usize], &self.pens[shape.pen as usize], both(&table, &[1.0 / fine, 0.0, 0.0, 1.0 / fine, 0.0, 0.0]));
        let high = block.pixels.len() / wide.max(1);
        // Within a box, a shape is clipped to its edges; its stroke is drawn separately and
        // multiplied by the box's coverage per pixel.
        let place = shape.boxed.map(|boxed| self.boxes[boxed as usize]).map(|[left, top, right, bottom]| [through(&fit, pt(left, top)), through(&fit, pt(right, bottom))]);
        let place = place.map(|[a, b]| [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)]);
        let mut canvas = Canvas::new(&mut block.pixels, wide, high, (0, top), 1.0);
        if let Some(ink) = self.ready(shape.fill, current_color, (&table, &fit), fill_tile.as_ref()) {
            // Its outline clipped to the box is in pixels.
            let cut = place.map(|place| cut_to(path, &table, place));
            let (outline, fine, to_pixels) = match &cut {
                Some(cut) => (cut, FINER, [1.0 / FINER, 0.0, 0.0, 1.0 / FINER, 0.0, 0.0]),
                None => (path, fine, to_pixels),
            };
            let mut fill = |onto: &mut Canvas, ink: &Ready| work.fill(onto, outline, fine, Transform(to_pixels), shape.even_odd, ink.painter());
            match CRISP.get().filter(|_| shape.crisp) {
                Some(crisp) => crisp(&mut canvas, (path, &table, 0.0, (wide, high, top)), &ink, &mut |onto| fill(onto, &Ready::Flat(Color::WHITE))),
                None => fill(&mut canvas, &ink),
            }
        }
        let Some(ink) = self.ready(shape.line, current_color, (&table, &fit), line_tile.as_ref()) else { return };
        // A non-scaling stroke is computed in fractions of a picture pixel, not of a shape
        // unit: the outline is transformed there first (`there`), and the stroke's edges
        // brought back from where it was computed (`back`).
        let (grain, there, back) = match pen.fixed {
            true => (FINER * zoom, both(&[FINER, 0.0, 0.0, FINER, 0.0, 0.0], &to_pixels), [1.0 / FINER, 0.0, 0.0, 1.0 / FINER, 0.0, 0.0]),
            false => (fine, SAME, to_pixels),
        };
        let dashes = &self.dashes[pen.dashes[0] as usize..pen.dashes[1] as usize];
        let mut stroke = |onto: &mut Canvas, ink: &Ready| work.line(onto, path, &pen.drawn(dashes), [fine, grain, 0.0], [Transform(there), Transform(back)], ink.painter());
        // A crisp stroke reaches as far as a miter may, in pixels.
        let out = pen.width * pen.miter.max(2.0) / 2.0 * if pen.fixed { zoom } else { fine / FINER };
        let mut line = |onto: &mut Canvas| match CRISP.get().filter(|_| shape.crisp) {
            Some(crisp) => crisp(onto, (path, &table, out, (wide, high, top)), &ink, &mut |onto| stroke(onto, &Ready::Flat(Color::WHITE))),
            None => stroke(onto, &ink),
        };
        let Some(place) = place else { return line(&mut canvas) };
        let mut over = vec![0; wide * high];
        line(&mut Canvas::new(&mut over, wide, high, (0, top), 1.0));
        boxed(&mut over, wide, top, place);
        canvas.pixels(&over, wide, [0, top], false);
    }

    /// Prepares a paint to fill a shape whose units the first transform maps to pixels (the
    /// second maps drawing units), with its drawn tile for a pattern. Nothing for no paint.
    fn ready<'b>(&'b self, ink: Ink, current_color: Color, (table, fit): (&Table, &Table), tile: Option<&'b Block>) -> Option<Ready<'b>> {
        match ink {
            Ink::None => None,
            Ink::Own(color) => Some(Ready::Flat(color)),
            Ink::CurrentColor(solid) => Some(Ready::Flat(current_color.with_alpha(solid))),
            Ink::Gradient(blend, solid) => {
                let blend = &self.gradients[blend as usize];
                let stops = &self.stops[blend.stops[0] as usize..blend.stops[1] as usize];
                undone(&both(table, &blend.table)).map(|back| Ready::Gradient(blend, stops, solid, back))
            }
            Ink::Tiles(kept, solid) => {
                let (kept, tile) = (&self.tiles[kept as usize], tile?);
                let high = tile.pixels.len() / tile.wide;
                let shown = both(fit, &both(&kept.table, &[kept.size[0] / tile.wide as f32, 0.0, 0.0, kept.size[1] / high as f32, 0.0, 0.0]));
                undone(&shown).map(|back| Ready::Tiles(Cow::Borrowed(&tile.pixels), tile.wide, back, solid, true))
            }
            // An image shown smaller than it is: each screen pixel averages all image pixels
            // under it, not the nearest four.
            Ink::Picture(kept, solid) => {
                let picture = self.held()?.pictures.get(kept as usize)?;
                let back = undone(table)?;
                let by = [back[0].hypot(back[1]), back[2].hypot(back[3])].map(|by| by.floor().clamp(1.0, 64.0) as usize);
                let (wide, pixels) = shrunk(&picture.pixels, picture.wide, by);
                let back = both(&[1.0 / by[0] as f32, 0.0, 0.0, 1.0 / by[1] as f32, 0.0, 0.0], &back);
                Some(Ready::Tiles(pixels, wide, back, solid, false))
            }
        }
    }
}

/// A paint prepared for one shape: a colour; a gradient with its stops, opacity and the
/// transform from a pixel back to gradient units; or tile or image pixels (`wide` per row) with
/// the transform from a pixel back to theirs, opacity, and whether they repeat (tiles) or not
/// (an image).
enum Ready<'a> {
    Flat(Color),
    Gradient(&'a Gradient, &'a [(f32, Color)], f32, Table),
    Tiles(Cow<'a, [u32]>, usize, Table, f32, bool),
}

impl Ready<'_> {
    /// What gives the colour at each pixel (see `at`).
    fn painter(&self) -> impl Fn(i32, i32) -> Color + '_ {
        move |x, y| self.at(x, y)
    }

    /// The colour at pixel `x`, `y`: for a gradient, by the pixel's centre; for tiles, the tile
    /// pixel it lies on.
    #[inline(always)]
    fn at(&self, x: i32, y: i32) -> Color {
        let middle = pt(x as f32 + 0.5, y as f32 + 0.5);
        match self {
            Ready::Flat(color) => *color,
            Ready::Gradient(blend, stops, solid, back) => color_at(stops, blend.along(through(back, middle))).with_alpha(*solid),
            Ready::Tiles(pixels, wide, back, solid, again) => Color(unmultiplied(tiled(pixels, *wide, through(back, middle), *again))).with_alpha(*solid),
        }
    }
}

/// A pattern's colour at a point (in tile pixels, `wide` per row), premultiplied: bilinear
/// between the four nearest tile pixels, the tiles repeating (`again`), or one tile, an image,
/// clamped at its edges.
fn tiled(pixels: &[u32], wide: usize, at: Point, again: bool) -> u32 {
    let (across_tile, high) = (wide as f32, (pixels.len() / wide.max(1)) as f32);
    let (x, y) = (at.x - 0.5, at.y - 0.5);
    let (across, down) = (x - x.floor(), y - y.floor());
    // The two columns and rows it lies between, each found once for all four.
    let held = |at: f32, side: f32| if again { at.rem_euclid(side) } else { at.clamp(0.0, side - 1.0) };
    let [left, right] = [0.0, 1.0].map(|dx| (held(x.floor() + dx, across_tile) as usize).min(wide.max(1) - 1));
    let [upper, lower] = [0.0, 1.0].map(|dy| (held(y.floor() + dy, high) as usize).min((high as usize).max(1) - 1) * wide);
    let pixel = |row: usize, column: usize| pixels.get(row + column).copied().unwrap_or(0);
    let near = [(pixel(upper, left), (1.0 - across) * (1.0 - down)), (pixel(upper, right), across * (1.0 - down)), (pixel(lower, left), (1.0 - across) * down), (pixel(lower, right), across * down)];
    let part = |shift: u32| near.iter().map(|(pixel, weight)| (pixel >> shift & 255) as f32 * weight).sum::<f32>().round().min(255.0) as u32;
    part(24) << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// An image's pixels (`wide` per row) shrunk by a factor across and down, each pixel the mean
/// of those it replaces (the image itself where not smaller): its row length and pixels.
fn shrunk(pixels: &[u32], wide: usize, [across, down]: [usize; 2]) -> (usize, Cow<'_, [u32]>) {
    if across * down == 1 {
        return (wide, Cow::Borrowed(pixels));
    }
    let high = pixels.len() / wide.max(1);
    let [small_wide, small_high] = [wide.div_ceil(across), high.div_ceil(down)];
    let mut sums = vec![[0u32; 5]; small_wide * small_high];
    for (at, pixel) in pixels.iter().enumerate() {
        let sum = &mut sums[(at / wide / down) * small_wide + at % wide / across];
        *sum = [sum[0] + (pixel >> 24), sum[1] + (pixel >> 16 & 255), sum[2] + (pixel >> 8 & 255), sum[3] + (pixel & 255), sum[4] + 1];
    }
    let mean = |sum: &[u32; 5]| [0, 1, 2, 3].map(|n| (sum[n] + sum[4] / 2) / sum[4].max(1));
    (small_wide, Cow::Owned(sums.iter().map(mean).map(|[a, r, g, b]| a << 24 | r << 16 | g << 8 | b).collect()))
}

/// A picture pixel premultiplied, as a canvas keeps it.
fn premultiplied(pixel: u32) -> u32 {
    let solid = pixel >> 24;
    let part = |shift: u32| ((pixel >> shift & 255) * solid + 127) / 255;
    solid << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// The bytes a `data:` URL holds: base64 (spaces allowed), or literal with `%` and two hex
/// digits per byte. Nothing for another URL, or a malformed one.
fn data(address: &str) -> Option<Vec<u8>> {
    let (head, body) = address.trim().strip_prefix("data:")?.split_once(',')?;
    if !head.trim_end().ends_with(";base64") {
        let mut bytes = Vec::with_capacity(body.len());
        let mut rest = body.as_bytes();
        while let [first, after @ ..] = rest {
            let hex = after.get(..2).filter(|_| *first == b'%').and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
            bytes.push(hex.unwrap_or(*first));
            rest = &after[if hex.is_some() { 2 } else { 0 }..];
        }
        return Some(bytes);
    }
    let (mut bytes, mut bits, mut held) = (Vec::with_capacity(body.len() / 4 * 3), 0u32, 0);
    for c in body.bytes().filter(|c| !c.is_ascii_whitespace()).take_while(|c| *c != b'=') {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        (bits, held) = ((bits << 6 | value as u32) & 0xff_ffff, held + 6);
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
        }
    }
    Some(bytes)
}

/// The text of an SVG file in bytes: UTF-8 (a byte order mark aside) whose first tag opens
/// after nothing but whitespace. Nothing for other bytes.
fn svg_text(bytes: &[u8]) -> Option<Rc<str>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.trim_start().starts_with('<').then(|| Rc::from(text))
}

/// A canvas pixel unpremultiplied, as a picture keeps it.
fn unmultiplied(pixel: u32) -> u32 {
    let solid = pixel >> 24;
    if matches!(solid, 0 | 255) {
        return pixel;
    }
    let part = |shift: u32| (((pixel >> shift & 255) * 255 + solid / 2) / solid).min(255);
    solid << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// Shapes drawn so far, plus one more: after dropping the tiles it is filled with and what was
/// drawn on them (each tile's block began where `begun` says, the last on top).
fn with_shape<'s>(mut shapes: Vec<&'s Shape>, begun: &mut Vec<usize>, shape: &'s Shape) -> Vec<&'s Shape> {
    for _ in [shape.fill, shape.line].iter().filter(|ink| matches!(ink, Ink::Tiles(..))) {
        shapes.truncate(begun.pop().unwrap_or(shapes.len()));
    }
    shapes.push(shape);
    shapes
}

/// The part of a shape's outline within a box: the outline transformed by `table` to pixels,
/// flattened to `FINER` pixel fractions, each ring clipped to the box's four sides (left, top,
/// right, bottom in pixels) by Sutherland–Hodgman. What any ring encloses inside the box is
/// kept; outside, nothing.
fn cut_to(path: &Path, table: &Table, [left, top, right, bottom]: [f32; 4]) -> Path {
    let mut shown = path.clone();
    shown.transform(Transform(*table));
    let mut rings: Vec<Vec<Point>> = Vec::new();
    shown.flatten_closed(FINER, pt(0.0, 0.0), FINER, |a, b| match rings.last_mut() {
        Some(ring) if ring.last() == Some(&a) => ring.push(b),
        _ => rings.push(vec![a, b]),
    });
    let mut cut = Path::new();
    for ring in &rings {
        let sides = [(0, left, true), (1, top, true), (0, right, false), (1, bottom, false)];
        let ring = sides.iter().fold(ring.clone(), |ring, (axis, edge, low)| cut_ring(&ring, *axis, edge * FINER, *low));
        if let [first, rest @ ..] = &ring[..] {
            cut.move_to(first.x / FINER, first.y / FINER);
            rest.iter().for_each(|p| _ = cut.line_to(p.x / FINER, p.y / FINER));
            cut.close();
        }
    }
    cut
}

/// A ring clipped at one side of a box: what lies inside along `axis` (0 across, 1 down), at or
/// past `edge` when `low`, else short of it, with a point where it crosses.
fn cut_ring(ring: &[Point], axis: usize, edge: f32, low: bool) -> Vec<Point> {
    let along = |p: Point| [p.x, p.y][axis];
    let inside = |p: Point| (along(p) >= edge) == low || along(p) == edge;
    let cross = |a: Point, b: Point| {
        let part = (edge - along(a)) / (along(b) - along(a));
        pt(a.x + (b.x - a.x) * part, a.y + (b.y - a.y) * part)
    };
    let mut cut = Vec::with_capacity(ring.len() + 4);
    for (n, b) in ring.iter().enumerate() {
        let a = ring[(n + ring.len() - 1) % ring.len()];
        match (inside(a), inside(*b)) {
            (true, true) => cut.push(*b),
            (true, false) => cut.push(cross(a, *b)),
            (false, true) => cut.extend([cross(a, *b), *b]),
            (false, false) => {}
        }
    }
    cut
}

/// Draws a fill or stroke without anti-aliasing (`shape-rendering: crispEdges`) where its
/// outline, through `table` and grown by `out` pixels, may reach on a block (`wide` by
/// `high`, its first row `top`): what `draw` covers there, each pixel more than half covered
/// then painted wholly with `ink`, the rest not at all.
fn crisp(canvas: &mut Canvas, (path, table, out, block): (&Path, &Table, f32, (usize, usize, i32)), ink: &Ready, draw: &mut dyn FnMut(&mut Canvas)) {
    let [left, top, right, bottom] = reached(path, table, out, block);
    let [wide, high] = [(right - left).max(0) as usize, (bottom - top).max(0) as usize];
    if wide * high == 0 {
        return;
    }
    let mut cover = vec![0; wide * high];
    draw(&mut Canvas::new(&mut cover, wide, high, (left, top), 1.0));
    for (at, pixel) in cover.iter_mut().enumerate() {
        let [x, y] = [left + (at % wide) as i32, top + (at / wide) as i32];
        *pixel = if *pixel >> 24 > 127 { premultiplied(ink.at(x, y).0) } else { 0 };
    }
    canvas.pixels(&cover, wide, [left, top], false);
}

/// The pixels of a block (`wide` by `high`, its first row picture row `top`) a shape may
/// cover: the reach of its outline through `table`, grown by `out` pixels each way, rounded out
/// and kept within the block. Left, top, right, bottom.
fn reached(path: &Path, table: &Table, out: f32, (wide, high, top): (usize, usize, i32)) -> [i32; 4] {
    let Some([left, upper, right, lower]) = reach(path) else { return [0; 4] };
    let corners = [pt(left, upper), pt(right, upper), pt(left, lower), pt(right, lower)].map(|corner| through(table, corner));
    let side = |pick: fn(f32, f32) -> f32, axis: fn(Point) -> f32| corners.iter().map(|p| axis(*p)).fold(axis(corners[0]), pick);
    let [x0, y0] = [side(f32::min, |p| p.x) - out, side(f32::min, |p| p.y) - out].map(|low| low.floor());
    let [x1, y1] = [side(f32::max, |p| p.x) + out, side(f32::max, |p| p.y) + out].map(|high| high.ceil());
    let (bottom, right) = (top as f32 + high as f32, wide as f32);
    [x0.clamp(0.0, right) as i32, y0.clamp(top as f32, bottom) as i32, x1.clamp(0.0, right) as i32, y1.clamp(top as f32, bottom) as i32]
}

/// What makes an item one of a run of fills to merge (see `Reading::merge_fills`): its colour
/// and box, where it is an opaque fill of one colour by the nonzero rule, drawn smooth and with
/// no stroke. Nothing for any other.
fn plain_fill(item: &Item) -> Option<(u32, Option<u32>)> {
    let Item::Shape(shape) = item else { return None };
    let Ink::Own(color) = shape.fill else { return None };
    (color.0 >> 24 == 255 && shape.line == Ink::None && !shape.even_odd && !shape.crisp).then_some((color.0, shape.boxed))
}

/// Which way an outline goes round, by its area: positive where clockwise on screen (y down),
/// negative where the other way. (Curves are flattened to a 256th of its size.)
fn turning(path: &Path) -> f32 {
    let Rect { w, h, .. } = path.bounds();
    let mut area = 0.0;
    path.flatten_closed(1.0, pt(0.0, 0.0), (w.max(h) / 256.0).max(1e-6), |a, b| area += a.x * b.y - b.x * a.y);
    area
}

/// Multiplies each pixel of a block by a box's coverage (left, top, right, bottom in pixels):
/// all inside, part on its edges, none outside. (The block is `wide` pixels wide, its first row
/// picture row `top`.)
fn boxed(pixels: &mut [u32], wide: usize, top: i32, [left, upper, right, lower]: [f32; 4]) {
    let covered = |from: f32, low: f32, high: f32| (high.min(from + 1.0) - low.max(from)).clamp(0.0, 1.0);
    for (at, pixel) in pixels.iter_mut().enumerate() {
        let [x, y] = [(at % wide.max(1)) as f32, (at / wide.max(1)) as f32 + top as f32];
        *pixel = faded(*pixel, (covered(x, left, right) * covered(y, upper, lower) * 256.0) as u32);
    }
}

/// A premultiplied pixel scaled by `part`/256.
fn faded(pixel: u32, part: u32) -> u32 {
    ((((pixel & 0x00ff_00ff) * part) >> 8) & 0x00ff_00ff) | (((pixel >> 8 & 0x00ff_00ff) * part) & 0xff00_ff00)
}

/// Composites the top block over the one under it at opacity `solid`, and drops it. (The bottom
/// block is the drawing itself, never dropped; a block of another size than the one under it,
/// which only a truncated file leaves, is dropped unseen.)
fn merge(blocks: &mut Vec<Block>, solid: f32) {
    if blocks.len() < 2 {
        return;
    }
    let (part, Some(mut over)) = ((solid.clamp(0.0, 1.0) * 256.0) as u32, blocks.pop()) else { return };
    let Some(under) = blocks.last_mut().filter(|under| under.pixels.len() == over.pixels.len()) else { return };
    over.pixels.iter_mut().for_each(|pixel| *pixel = faded(*pixel, part));
    let high = over.pixels.len() / under.wide.max(1);
    Canvas::new(&mut under.pixels, under.wide, high, (0, 0), 1.0).pixels(&over.pixels, under.wide, [0, 0], false);
}

/// Clips the block under the top one by the top one, per pixel: by its alpha or its luminance;
/// then drops the top one.
fn cut(blocks: &mut Vec<Block>, keep: Keep) {
    if blocks.len() < 3 {
        return;
    }
    let Some(by) = blocks.pop() else { return };
    let Some(under) = blocks.last_mut().filter(|under| under.pixels.len() == by.pixels.len()) else { return };
    for (pixel, by) in under.pixels.iter_mut().zip(&by.pixels) {
        // Luminance of a premultiplied colour: a mask keeps its luminance and alpha at once.
        let part = match keep {
            Keep::Solid => by >> 24,
            Keep::Bright => (13926 * (by >> 16 & 255) + 46885 * (by >> 8 & 255) + 4725 * (by & 255)) >> 16,
        };
        *pixel = faded(*pixel, part + (part >> 7));
    }
}

impl<'a> Reading<'a> {
    /// Reads a file, or with `glyph` its glyph part (see `ParsedDrawing::glyph`): what is kept
    /// and its drawing cost. Nothing if it holds no drawing.
    fn of(text: &'a str, glyph: Option<(&str, f32)>) -> Option<Reading<'a>> {
        Reading::prepared(text, glyph).map(|(reading, start)| reading.walked(start))
    }

    /// Reads a file noting what lies where in it (see `ParsedDrawing::read_with_hits`).
    fn noted(text: &'a str) -> Option<Reading<'a>> {
        let (mut reading, start) = Reading::prepared(text, None)?;
        reading.noting = Some(Reading::note_hits);
        Some(reading.walked(start))
    }

    /// Walks a prepared file from element `start`, then reads the drawings it brings.
    fn walked(mut self, start: usize) -> Reading<'a> {
        // (A drawing unit is this share of the picture: its view is all of it.)
        let [.., wide, high] = self.parsed.view;
        self.walk(start, Style { share: [1.0 / (wide * high), 1.0], ..Style::default() });
        if let Some(read_later) = self.later {
            read_later(&mut self);
        }
        self
    }

    /// A file parsed, with its size, view and fit, ready to walk; and where the walk starts.
    /// Nothing if it holds no drawing.
    fn prepared(text: &'a str, glyph: Option<(&str, f32)>) -> Option<(Reading<'a>, usize)> {
        let file = File::read(text);
        let root = file.elements.iter().position(|element| element.name == "svg")?;
        let start = match glyph {
            Some((id, _)) => *file.named.get(id)?,
            None => root,
        };
        let side = |name: &str| file.attribute(root, name).and_then(|text| length(text, f32::NAN)).filter(|side| *side > 0.0);
        let (wide, high) = (side("width"), side("height"));
        // The view: as `viewBox` says, else from the origin to its size (300 by 150 when
        // unstated, or a glyph's em).
        let stated: Vec<f32> = file.attribute(root, "viewBox").map(|value| numbers(value).take(5).collect()).unwrap_or_default();
        let fallback = glyph.map_or([300.0, 150.0], |(_, em)| [em, em]);
        let view = match stated[..] {
            [left, top, across, down] if across > 0.0 && down > 0.0 => [left, top, across, down],
            _ => [0.0, 0.0, wide.unwrap_or(fallback[0]), high.unwrap_or(fallback[1])],
        };
        let [_, _, across, down] = view;
        if !view.iter().all(|side| side.is_finite()) {
            return None;
        }
        // An unstated side follows from the other and the view's aspect.
        let size = match glyph {
            Some(_) => [wide.unwrap_or(fallback[0]), high.unwrap_or(fallback[1])],
            None => [wide.or(high.map(|high| high * across / down)).unwrap_or(across), high.or(wide.map(|wide| wide * down / across)).unwrap_or(down)],
        };
        let aspect = Aspect::read(file.attribute(root, "preserveAspectRatio").unwrap_or(""));
        let filtering = FILTERS.get().filter(|_| glyph.is_none());
        Some((Reading::with(file, (size, view, aspect), filtering), start))
    }

    /// A reading of a parsed file into a drawing of this size, view and fit, nothing read yet.
    fn with(file: File<'a>, (size, view, aspect): ([f32; 2], [f32; 4], Aspect), filtering: Option<&'static Filters>) -> Reading<'a> {
        let parsed = ParsedDrawing {
            size,
            view,
            aspect,
            items: Vec::new(),
            paths: Vec::new(),
            pens: Vec::new(),
            dashes: Vec::new(),
            gradients: Vec::new(),
            stops: Vec::new(),
            tiles: Vec::new(),
            boxes: Vec::new(),
            more: None,
            uses_current_color: false,
            drawn: Vec::new(),
        };
        Reading {
            budget: file.text.len(),
            file,
            parsed,
            outlined: HashMap::new(),
            gradients_read: HashMap::new(),
            pictured: HashMap::new(),
            fonts: Vec::new(),
            files: Vec::new(),
            faces: Vec::new(),
            pieces: 0,
            steps: 0,
            lookups: 0,
            passes: 0.0,
            corners: Vec::new(),
            filtering,
            depth: 0,
            later: None,
            noting: None,
            hits: Hits::default(),
        }
    }

    /// Walks the elements from `root` on, styled `style` around it, keeping every shape to
    /// draw, as far as `spent` allows.
    fn walk(&mut self, root: usize, style: Style) {
        // The walks in progress, innermost last.
        let mut walks = vec![Walk::through(root, root + 1, style)];
        while let Some(deep) = walks.len().checked_sub(1) {
            // Each step looks through the walks it is in.
            self.steps += deep + 1;
            let walk = &mut walks[deep];
            // A shape whose tiles are drawn already is kept all the same: they lie on top of
            // the blocks and are dropped only as it is drawn.
            let tiled = walk.tiled.is_some_and(|(own, _)| own == walk.at);
            if walk.at >= walk.end || (!tiled && self.spent()) {
                self.finish(&mut walks);
                continue;
            }
            if walk.marking {
                self.next_marker(&mut walks);
                continue;
            }
            let (at, around, brought, itself) = (walk.at, walk.style, walk.used == Some(walk.at), walk.own == Some(walk.at));
            let (given, tiled) = (if brought { walk.use_size } else { [None; 2] }, walk.tiled.filter(|(own, _)| *own == at).map(|(_, tiles)| tiles));
            let (name, end) = (self.file.elements[at].name, self.file.elements[at].end);
            walk.at = end;
            let mut style = self.styled(at, &around);
            // What it holds to walk, if it holds shapes: the range, and for a `<use>` the
            // element it brings. (Not one it is inside, nor one already being brought: that
            // would never end. And no group in a clip path.)
            let free = |named: &usize| !(*named..self.file.elements[*named].end).contains(&at) && walks.iter().all(|walk| walk.used != Some(*named));
            let held = match name {
                "g" | "svg" | "a" | "switch" if !around.clipping => Some((at + 1, end, None)),
                "symbol" if brought => Some((at + 1, end, None)),
                "use" => self.file.linked(at).filter(free).map(|named| (named, named + 1, Some(named))),
                _ => None,
            };
            if style.gone {
                continue;
            }
            // A `<use>` and a nested `<svg>` position what they hold; a nested viewport is
            // fitted to its box and clipped at its edges.
            let [.., wide, high] = self.parsed.view;
            let length_of = |name: &str, whole: f32| self.file.attribute(at, name).and_then(|text| length(text, whole));
            if matches!(name, "use" | "svg") && at != root {
                style.table = both(&style.table, &[1.0, 0.0, 0.0, 1.0, length_of("x", wide).unwrap_or(0.0), length_of("y", high).unwrap_or(0.0)]);
            }
            let size = [given[0].or(length_of("width", wide)).unwrap_or(wide), given[1].or(length_of("height", high)).unwrap_or(high)];
            // A `<use>` gives the size of what it brings, for a `<symbol>` that fills it.
            let brings = if name == "use" { [length_of("width", wide), length_of("height", high)] } else { [None; 2] };
            let mut room = if (name == "svg" && at != root) || (name == "symbol" && brought) { self.fit_viewport(at, size, &mut style) } else { None };
            if itself {
                (style.clip, style.mask, style.opacity, style.filter) = (None, None, 1.0, None);
            }
            // Filtered content is clipped after the filter, by its own clip path and its
            // viewport, never before. (Not in a clip path, whose shapes are outlines only.)
            let layers = walks.iter().filter(|walk| walk.layer.is_some()).count();
            let filtered = style.filter.is_some() && self.filtering.is_some() && !around.clipping && layers < LAYERS;
            let around_box = if filtered { style.boxed.take() } else { None };
            // A box aligned with the picture clips its content exactly: a nested viewport, or a
            // clip path that is such a box.
            if let Some(boxed) = room.filter(|_| !filtered).and_then(|place| self.boxed(place, &style)) {
                (style.boxed, room) = (Some(boxed), None);
            }
            if let Some(boxed) = style.clip.filter(|_| !filtered).and_then(|clip| self.clip_box(clip, &style)).and_then(|place| self.boxed(place, &style)) {
                (style.boxed, style.clip) = (Some(boxed), None);
            }
            // A semi-transparent group composites as one, so its overlapping shapes do not show
            // through each other; so does clipped or filtered content.
            let cuts = [style.clip.map(Cut::Clip), style.mask.map(Cut::Mask), room.map(Cut::Box), around_box.map(Cut::DrawingBox)];
            let cut = filtered || (cuts.iter().any(Option::is_some) && layers < LAYERS);
            let layer = ((style.opacity < 1.0 || cut) && layers < LAYERS).then_some(style.opacity);
            let first = self.parsed.items.len() + layer.is_some() as usize;
            let (cuts, filter) = if cut { (cuts, style.filter.filter(|_| filtered)) } else { ([None; 4], None) };
            let Some((from, to, used)) = held.filter(|_| walks.len() < DEEP) else {
                // A clipped shape is walked again as itself on a block of its own: a filtered
                // one without its surrounding box.
                let inner = Style { boxed: around.boxed.filter(|_| around_box.is_none()), ..around };
                let again = Walk { own: itself.then_some(at), ..Walk::through(at, at + 1, inner) };
                let cutting = (cut && !itself).then_some(Walk { layer, cuts, filter, first, own: Some(at), units: style.table, ..again });
                self.leaf((at, name, &style), (again, cutting, tiled), &mut walks);
                continue;
            };
            match layer {
                Some(_) => self.begin(&style),
                None => style.faded *= style.opacity,
            }
            walks.push(Walk { used, layer, cuts, filter, first, use_size: brings, ..Walk::through(from, to, style) });
        }
    }

    /// Ends the top walk once drawn: its block is filtered, then clipped by each clip in turn
    /// (each a walk of its own); then it clips the block under it or composites onto it; and
    /// the next tile begins.
    fn finish(&mut self, walks: &mut Vec<Walk>) {
        let Some(walk) = walks.last_mut() else { return };
        walk.last = walk.last.min(self.parsed.items.len());
        let (filter, cut, done) = (walk.filter.take(), walk.cuts.iter_mut().find_map(Option::take), *walk);
        if let (Some(filter), Some(filters)) = (filter, self.filtering) {
            (filters.read)(self, filter, &done);
        }
        if let Some(cut) = cut {
            let cutting = self.cutting(cut, &done);
            return walks.push(cutting);
        }
        if let (Some(_), Some(merge)) = (done.keep, SEAMLESS.get()) {
            merge(self, done.first);
        }
        self.parsed.items.extend(done.keep.map(Item::Cut).into_iter().chain(done.layer.map(Item::End)).chain(done.then.map(Item::Tile)));
        walks.pop();
    }

    /// Makes each run of opaque fills of one colour on the block of a clip path or mask (its
    /// items from `from` on) one shape: drawn apart, two that abut leave a seam where each
    /// covers part of a pixel. Each is first turned to go round positively, so that where two
    /// overlap they add up. (Exact unless a shape goes round negatively somewhere another
    /// overlaps it.)
    fn merge_fills(&mut self, from: usize) {
        let mut at = from;
        while at < self.parsed.items.len() {
            let first = plain_fill(&self.parsed.items[at]);
            let run = self.parsed.items[at..].iter().take_while(|item| first.is_some() && plain_fill(item) == first).count().max(1);
            if run > 1 {
                self.merge_run(at, run);
            }
            at += 1;
        }
    }

    /// Makes `run` fills from item `at` on one shape (see `merge_fills`).
    fn merge_run(&mut self, at: usize, run: usize) {
        let mut merged = Path::new();
        let Item::Shape(first) = &self.parsed.items[at] else { return };
        let (fill, pen, boxed) = (first.fill, first.pen, first.boxed);
        for item in &self.parsed.items[at..at + run] {
            let Item::Shape(shape) = item else { continue };
            let mut outline = self.parsed.paths[shape.path as usize].clone();
            outline.transform(Transform(shape.table));
            if turning(&outline) < 0.0 {
                outline.reverse();
            }
            merged.add(&outline);
        }
        self.pieces += merged.steps().len();
        let path = self.kept_path(merged);
        let shape = Shape { path, table: SAME, fill, even_odd: false, line: Ink::None, pen, boxed, crisp: false };
        self.parsed.items.splice(at..at + run, [Item::Shape(shape)]);
        // Drawings waiting to go in after the run go in that much earlier.
        self.parsed.more().later.iter_mut().filter(|later| later.at > at).for_each(|later| later.at -= run - 1);
    }

    /// Keeps a shape the walk reached, styled by `style`, or begins what it needs first: a
    /// clipped shape is walked again as itself on its own block (`cutting`), and one filled
    /// with a pattern whose tiles are not drawn yet (`tiled`) once the tile is drawn (`again`).
    /// Walks through its markers follow it.
    fn leaf(&mut self, (at, name, style): (usize, &str, &Style), (again, cutting, tiled): (Walk, Option<Walk>, Option<[Option<u32>; 2]>), walks: &mut Vec<Walk>) {
        if let Some(cutting) = cutting {
            self.begin(style);
            return walks.push(cutting);
        }
        if let Some((tiles, drawings)) = tiled.is_none().then(|| self.tiling(at, name, style, walks)).flatten() {
            walks.push(Walk { tiled: Some((at, tiles)), ..again });
            return walks.extend(drawings);
        }
        let first = self.parsed.items.len();
        walks.extend(self.shape(at, name, style, tiled.unwrap_or([None; 2])));
        if let Some(note) = self.noting {
            note(self, at, first, walks);
        }
    }

    /// Notes the shapes kept for element `at` from item `first` on (see `Hits`), with the
    /// `<use>` elements that brought it, where `walks` are none of a clip path, mask, pattern
    /// or marker: what is drawn of the file itself.
    fn note_hits(&mut self, at: usize, first: usize, walks: &[Walk]) {
        let named = |walk: &Walk| walk.used.map(|used| self.file.elements[used].name);
        if walks.iter().any(|walk| walk.keep.is_some() || walk.style.clipping || matches!(named(walk), Some("pattern" | "marker"))) {
            return;
        }
        let from = self.hits.uses.len() as u32;
        for (n, walk) in walks.iter().enumerate().skip(1).rev() {
            let Some(brought) = walk.used else { continue };
            self.hits.uses.extend(self.user(brought, walks[n - 1].at).map(|user| user as u32));
        }
        let uses = [from, self.hits.uses.len() as u32];
        for item in &self.parsed.items[first.min(self.parsed.items.len())..] {
            let Item::Shape(shape) = item else { continue };
            let stroke = if shape.line == Ink::None { 0.0 } else { self.parsed.pens[shape.pen as usize].width };
            let (path, table, filled, even_odd) = (shape.path, shape.table, shape.fill != Ink::None, shape.even_odd);
            let reach = reach_of(&self.parsed.paths[path as usize], &table, stroke);
            self.hits.shapes.push(Hit { element: at as u32, uses, path, table, filled, even_odd, stroke, reach });
        }
    }

    /// The `<use>` element that brought `brought`, the walk around it being past it at `after`:
    /// the last `<use>` before then that ends there and names it.
    fn user(&self, brought: usize, after: usize) -> Option<usize> {
        let elements = &self.file.elements;
        (0..after.min(elements.len())).rev().find(|at| elements[*at].name == "use" && elements[*at].end == after && self.file.linked(*at) == Some(brought))
    }

    /// Continues a shape's markers (the top walk goes through them): begins the next one's
    /// walk. (Not one already being drawn, which would never end; nor deeper than elements may
    /// nest.)
    fn next_marker(&mut self, walks: &mut Vec<Walk>) {
        let Some(walk) = walks.last_mut() else { return };
        let ((marker, point, way, first), style) = (self.corners[walk.at], walk.style);
        walk.at += 1;
        if walks.len() <= DEEP && walks.iter().all(|walk| walk.used != Some(marker)) {
            let marked = self.marker(marker, (point, way, first), &style);
            walks.push(marked);
        }
    }

    /// Begins what clips `walk`'s block once drawn: on a block of its own, as a walk ending in
    /// the clip.
    fn cutting(&mut self, cut: Cut, walk: &Walk) -> Walk {
        self.begin(&walk.style);
        let first = self.parsed.items.len();
        let table = walk.units;
        match cut {
            // Filling its whole block, a box costs the block's area. A kept box is in drawing
            // units.
            Cut::Box(_) | Cut::DrawingBox(_) => {
                self.spend(walk.style.share[1]);
                let (place, table) = self.box_cut(cut, table);
                self.parsed.paths.push(rect(place, [None; 2]));
                let (path, pen) = (self.parsed.paths.len() as u32 - 1, self.pen(&walk.style));
                self.parsed.items.push(Item::Shape(Shape { path, table, fill: Ink::Own(Color::WHITE), even_odd: false, line: Ink::None, pen, boxed: None, crisp: false }));
                Walk { keep: Some(Keep::Solid), first, ..Walk::through(0, 0, walk.style) }
            }
            // A clip path is in the units of what it clips, or its bounding box.
            Cut::Clip(at) => {
                let mut style = self.styled(at, &Style { clipping: true, table, share: walk.style.share, ..Style::default() });
                let boxed = self.file.attribute(at, "clipPathUnits") == Some("objectBoundingBox");
                style.table = both(&style.table, &if boxed { self.box_of(walk) } else { SAME });
                Walk { keep: Some(Keep::Solid), first, ..Walk::through(at + 1, self.file.elements[at].end, style) }
            }
            // A mask is drawn as it says within its own region: in the bounding box of what it
            // masks (10% larger each way by default), or in units. Its content is in units or
            // bounding box.
            Cut::Mask(at) => {
                let ([.., wide, high], square) = (self.parsed.view, self.box_of(walk));
                let boxed = self.file.attribute(at, "maskUnits") != Some("userSpaceOnUse");
                let [wide, high] = if boxed { [1.0; 2] } else { [wide, high] };
                let side = |name: &str, whole: f32, fallback: f32| self.file.attribute(at, name).and_then(|text| length(text, whole)).unwrap_or(fallback * whole);
                let place = [side("x", wide, -0.1), side("y", high, -0.1), side("width", wide, 1.2), side("height", high, 1.2)];
                let place = if boxed { [square[4] + place[0] * square[0], square[5] + place[1] * square[3], place[2] * square[0], place[3] * square[3]] } else { place };
                let mut style = self.styled(at, &Style { table, share: walk.style.share, ..Style::default() });
                let inside = self.file.attribute(at, "maskContentUnits") == Some("objectBoundingBox");
                let back = if inside { undone(&square) } else { Some(SAME) };
                style.table = both(&style.table, &[square, SAME][(!inside) as usize]);
                let place = back.map(|back| [through(&back, pt(place[0], place[1])), through(&back, pt(place[0] + place[2], place[1] + place[3]))]);
                let place = place.map(|[corner, far]| [corner.x, corner.y, far.x - corner.x, far.y - corner.y]).unwrap_or([0.0; 4]);
                let cuts = [Some(Cut::Box(place)), None, None, None];
                Walk { keep: Some(Keep::Bright), first, cuts, ..Walk::through(at + 1, self.file.elements[at].end, style) }
            }
        }
    }

    /// Reads the filters a list names (`filter`: `url(#…)` references and functions such as
    /// `blur()` and `drop-shadow()`), applied to what `walk` drew on its block: each kept and
    /// run by an item in turn, after the blocks its images are drawn on, its cost counted. If
    /// any is no filter, none is read, and the element drawn unfiltered (as browsers and
    /// librsvg do).
    fn filtered(&mut self, [from, to]: [u32; 2], walk: &Walk) {
        let list = self.file.text.get(from as usize..to as usize).unwrap_or("");
        let square = self.box_of(walk);
        let mut filters = Vec::new();
        for (name, inside) in calls(list).unwrap_or_default() {
            let filter = match name {
                "url" => self.file.named.get(inside.trim().trim_matches(['"', '\'']).trim_start_matches('#')).copied().filter(|at| self.file.elements[*at].name == "filter"),
                _ => None,
            };
            let filter = match filter {
                Some(at) => Some(self.filter_of(at, walk, &square)),
                None => function(name, inside, walk).map(|filter| (filter, Vec::new())),
            };
            let Some(filter) = filter else { return };
            filters.push(filter);
        }
        for (filter, images) in filters {
            // Each step covers its whole region and more; each image its block. A filter
            // costing more than the budget left is skipped with those after it: the element is
            // drawn unfiltered. Else a few elements could stand for any number of full-picture
            // blurs.
            let [l, t, r, b] = filter.bounds();
            let share = ((r - l) * (b - t) * walk.style.share[0]).min(walk.style.share[1]);
            let weights: f32 = filter.steps.iter().map(|step| svg_filters::weight(&step.kind) + svg_filters::background_weight(step)).sum();
            let cost = share * (svg_filters::FIXED + weights) + images.len() as f32 * walk.style.share[1];
            if self.passes + cost > passes(self.budget) {
                return;
            }
            self.spend(cost - images.len() as f32 * walk.style.share[1]);
            for image in images {
                self.image_block(image, &filter, walk);
            }
            let filters = &mut self.parsed.more().filters;
            filters.push(filter);
            let filter = Item::Filter(filters.len() as u32 - 1);
            self.parsed.items.push(filter);
        }
    }

    /// Begins the block a filter's image is drawn on, for a filter applied to what `walk` drew: a
    /// `feImage` step's element, read once the file's own is, or its picture in its box; or the
    /// element's fill or stroke paint over the filter region.
    fn image_block(&mut self, image: Imaged, filter: &Filter, walk: &Walk) {
        self.begin(&walk.style);
        let style = Style { table: filter.table, share: walk.style.share, ..Style::default() };
        match (image, PICTURES.get()) {
            (Imaged::Element(element, units), _) => {
                let (at, depth, table) = (self.parsed.items.len(), self.depth + 1, both(&filter.table, &units));
                self.wait(Later { at, brought: Brought::Element(element), table, share: style.share, place: [None; 4], aspect: Aspect::read(""), solid: 1.0, boxed: None, depth });
            }
            (Imaged::Href(at, place), Some(picture)) => picture(self, at, &style, Some(place)),
            (Imaged::Href(..), None) => {}
            (Imaged::Paint(which), _) => self.paint_block(which, filter, walk),
        }
    }

    /// Fills the filter region of a filter applied to what `walk` drew with the element's fill
    /// (`which` 0) or stroke (1) paint, at its opacity, as librsvg does (more than the region:
    /// the filter cuts it): a gradient in units of the element's bounding box fits that box. (A
    /// pattern paints nothing.)
    fn paint_block(&mut self, which: usize, filter: &Filter, walk: &Walk) {
        let own = walk.own.map_or(walk.style, |at| self.styled(at, &walk.style));
        let square = self.box_of(walk);
        let fitted = self.kept_path(rect([square[4], square[5], square[0], square[3]], [None; 2]));
        let fill = self.ink(own.paints[which], own.solid[which] * own.faded, own.ink, fitted, None);
        // Half as large again each way: the region's pixels, rounded out, are covered whole.
        let [x, y, w, h] = filter.region.map(|side| side as f32);
        let path = self.kept_path(rect([x - w / 2.0, y - h / 2.0, 2.0 * w, 2.0 * h], [None; 2]));
        let pen = self.pen(&own);
        self.parsed.items.push(Item::Shape(Shape { path, table: filter.table, fill, even_odd: false, line: Ink::None, pen, boxed: None, crisp: false }));
    }

    /// A filter element applied to what a walk drew, whose bounding box is `square` (the
    /// transform taking a unit square onto it): its region and steps, and what its `feImage`
    /// steps draw. Each child it knows is a step, in order; steps after the reading budget is
    /// spent are not read.
    fn filter_of(&mut self, at: usize, walk: &Walk, square: &Table) -> (Filter, Vec<Imaged>) {
        let ([.., wide, high], end) = (self.parsed.view, self.file.elements[at].end);
        let [bw, bh, bx, by] = [square[0], square[3], square[4], square[5]];
        let attr = |name: &str| self.file.attribute(at, name);
        // Its region: in bounding-box fractions (by default 10% larger each way), or in units.
        let [x, y, w, h] = [bx, by, bw, bh].map(f64::from);
        let region = match attr("filterUnits") != Some("userSpaceOnUse") {
            true => {
                let part = |name: &str, fallback: f32| f64::from(attr(name).and_then(share).unwrap_or(fallback));
                [x + part("x", -0.1) * w, y + part("y", -0.1) * h, part("width", 1.2) * w, part("height", 1.2) * h]
            }
            false => {
                let side = |name: &str, whole: f32, fallback: f32| attr(name).and_then(|text| exact_length(text, whole.into())).unwrap_or(f64::from(fallback) * f64::from(whole));
                [side("x", wide, -0.1), side("y", high, -0.1), side("width", wide, 1.2), side("height", high, 1.2)]
            }
        };
        let in_box = attr("primitiveUnits") == Some("objectBoundingBox");
        let style = self.styled(at, &Style::default());
        let mut read = Primitives { in_box, square: [bx, by, bw, bh], region, results: Vec::new(), images: Vec::new() };
        let (mut steps, mut child) = (Vec::new(), at + 1);
        while child < end && !self.spent() {
            self.steps += self.file.elements[child].end - child;
            if let Some(step) = self.step(child, &style, steps.len(), &mut read) {
                read.results.extend(self.file.attribute(child, "result").map(|result| (result.trim(), steps.len())));
                steps.push(step);
            }
            child = self.file.elements[child].end;
        }
        (Filter { table: walk.units, region, steps, images: read.images.len() }, read.images)
    }

    /// One filter step as its element says, in a filter styled `around`, after `made` steps
    /// (see `Primitives`). Nothing for a non-step, or a malformed step (librsvg skips those
    /// too).
    fn step(&mut self, at: usize, around: &Style, made: usize, read: &mut Primitives<'a>) -> Option<svg_filters::Step> {
        let style = self.styled(at, around);
        let (file, [.., wide, high]) = (&self.file, self.parsed.view);
        let attr = |name: &str| file.attribute(at, name);
        let inputs = [input_of(attr("in").map(str::trim), read, made), input_of(attr("in2").map(str::trim), read, made)];
        let nodes = file.inside(at).filter(|node| file.elements[*node].name == "feMergeNode");
        let merged: Vec<Input> = nodes.map(|node| input_of(file.attribute(node, "in").map(str::trim), read, made)).collect();
        let ([bx, by, bw, bh], in_box) = (read.square, read.in_box);
        // Lengths and subregion: in bounding-box fractions, or units.
        let scaled = |[x, y]: [f32; 2]| if in_box { [x * bw, y * bh] } else { [x, y] };
        let pair = |name: &str, fallback: f32| {
            let stated: Vec<f32> = attr(name).map(|text| numbers(text).take(2).collect()).unwrap_or_default();
            [stated.first().copied().unwrap_or(fallback), stated.get(1).or(stated.first()).copied().unwrap_or(fallback)]
        };
        let one = |name: &str, fallback: f32| attr(name).and_then(number).map_or(fallback, |(number, _)| number);
        let place = |name: &str, axis: usize, start: bool| match in_box {
            true => attr(name).and_then(share).map(|part| f64::from([bx, by][axis]) * start as u8 as f64 + f64::from(part) * f64::from([bw, bh][axis])),
            false => attr(name).and_then(|text| exact_length(text, [wide, high][axis].into())),
        };
        let place = [place("x", 0, true), place("y", 1, true), place("width", 0, false), place("height", 1, false)];
        let flood = match style.flood.0 {
            Paint::Own(color) => color,
            _ => style.ink.unwrap_or(Color::BLACK),
        };
        let flood = flood.with_alpha(style.flood.1);
        let unit = kernel_unit(attr("kernelUnitLength"), if in_box { [bw, bh] } else { [1.0; 2] });
        let kind = match file.elements[at].name {
            "feGaussianBlur" => Kind::Blur(scaled(pair("stdDeviation", 0.0)).map(|deviation| deviation.max(0.0))),
            "feOffset" => Kind::Offset(scaled([one("dx", 0.0), one("dy", 0.0)])),
            "feFlood" => Kind::Flood(flood),
            "feMerge" => Kind::Merge(merged),
            "feComposite" => Kind::Composite(operator(attr("operator").unwrap_or("over").trim(), [one("k1", 0.0), one("k2", 0.0), one("k3", 0.0), one("k4", 0.0)])),
            "feBlend" => Kind::Blend(Mode::named(attr("mode").unwrap_or("normal").trim()).unwrap_or(Mode::Normal)),
            "feColorMatrix" => Kind::Matrix(matrix(attr("type").unwrap_or("matrix").trim(), attr("values"))?),
            "feComponentTransfer" => Kind::Transfer(Box::new(file.functions(at))),
            "feDropShadow" => Kind::Shadow(scaled([one("dx", 2.0), one("dy", 2.0)]), scaled(pair("stdDeviation", 2.0)).map(|deviation| deviation.max(0.0)), flood),
            "feMorphology" => Kind::Morphology(attr("operator").map(str::trim) == Some("dilate"), scaled(pair("radius", 0.0))),
            "feTile" => Kind::Tile,
            "feTurbulence" => {
                let base = pair("baseFrequency", 0.0);
                let (fractal, stitch) = (attr("type").map(str::trim) == Some("fractalNoise"), attr("stitchTiles").map(str::trim) == Some("stitch"));
                (base[0] >= 0.0 && base[1] >= 0.0).then_some(())?;
                Kind::Noise(Noise { base, octaves: one("numOctaves", 1.0).clamp(0.0, svg_filters::OCTAVES as f32) as u32, seed: one("seed", 0.0), stitch, fractal })
            }
            "feImage" => {
                read.images.push(self.imaged(at, read, place)?);
                Kind::Image(read.images.len() - 1)
            }
            "feDiffuseLighting" | "feSpecularLighting" => Kind::Light(Box::new(self.lighting(at, &style, read, unit)?)),
            "feConvolveMatrix" => Kind::Convolve(Box::new(self.convolution(at, unit)?)),
            "feDisplacementMap" => {
                let channel = |name: &str| ["R", "G", "B", "A"].iter().position(|channel| Some(*channel) == attr(name).map(str::trim));
                Kind::Displace(scaled([one("scale", 0.0); 2]), [channel("xChannelSelector").unwrap_or(3), channel("yChannelSelector").unwrap_or(3)])
            }
            _ => return None,
        };
        Some(svg_filters::Step { kind, inputs, place, space: style.space })
    }

    /// What a `feImage` step draws (see `Imaged`): its subregion is `place` where given, else
    /// the filter region's. Nothing where it names nothing.
    fn imaged(&self, at: usize, read: &Primitives<'a>, place: [Option<f64>; 4]) -> Option<Imaged> {
        let href = self.file.attribute(at, "href").or_else(|| self.file.attribute(at, "xlink:href"))?.trim();
        if let Some(id) = href.strip_prefix('#') {
            let [bx, by, bw, bh] = read.square;
            let units = if read.in_box { [bw, 0.0, 0.0, bh, bx, by] } else { SAME };
            return Some(Imaged::Element(*self.file.named.get(id)?, units));
        }
        let place: [f64; 4] = std::array::from_fn(|n| place[n].unwrap_or(read.region[n]));
        Some(Imaged::Href(at, place.map(|side| side as f32)))
    }

    /// A lighting step's light as its element and its first light child say (lengths and
    /// positions as `read` takes them), with its kernel unit (see `kernel_unit`). Nothing without
    /// a light, or with a specular exponent outside 1 to 128.
    fn lighting(&self, at: usize, style: &Style, read: &Primitives<'a>, unit: Option<[f32; 2]>) -> Option<Lighting> {
        let one = |at: usize, name: &str, fallback: f32| self.file.attribute(at, name).and_then(number).map_or(fallback, |(number, _)| number);
        let lights = ["feDistantLight", "fePointLight", "feSpotLight"];
        let light = self.file.inside(at).find(|child| lights.contains(&self.file.elements[*child].name))?;
        // Positions in bounding-box fractions or units; depths as a fraction of the box's
        // diagonal over the square root of two.
        let [bx, by, bw, bh] = read.square;
        let point = |[x, y, z]: [&str; 3]| match read.in_box {
            true => [bx + one(light, x, 0.0) * bw, by + one(light, y, 0.0) * bh, one(light, z, 0.0) * ((bw * bw + bh * bh) / 2.0).sqrt()],
            false => [one(light, x, 0.0), one(light, y, 0.0), one(light, z, 0.0)],
        };
        let source = match self.file.elements[light].name {
            "feDistantLight" => Source::Distant([one(light, "azimuth", 0.0), one(light, "elevation", 0.0)]),
            "fePointLight" => Source::Point(point(["x", "y", "z"])),
            _ => {
                let cone = self.file.attribute(light, "limitingConeAngle").and_then(number).map(|(degrees, _)| degrees.abs());
                Source::Spot(point(["x", "y", "z"]), point(["pointsAtX", "pointsAtY", "pointsAtZ"]), one(light, "specularExponent", 1.0), cone)
            }
        };
        let specular = self.file.elements[at].name == "feSpecularLighting";
        let exponent = one(at, "specularExponent", 1.0);
        (!specular || (1.0..=128.0).contains(&exponent)).then_some(())?;
        let constant = one(at, if specular { "specularConstant" } else { "diffuseConstant" }, 1.0);
        let color = match self.file.own_property(at, "lighting-color").and_then(plain) {
            Some(Paint::Own(color)) => color,
            Some(Paint::CurrentColor) => style.ink.unwrap_or(Color::BLACK),
            _ => Color::WHITE,
        };
        Some(Lighting { specular, surface: one(at, "surfaceScale", 1.0), constant, exponent, color, source, unit })
    }

    /// A convolution step as its element says, with its kernel unit (see `kernel_unit`). Nothing
    /// where the kernel's order is no positive whole number, its values are not as many as it
    /// has places, or the target lies outside.
    fn convolution(&self, at: usize, unit: Option<[f32; 2]>) -> Option<Convolution> {
        let attr = |name: &str| self.file.attribute(at, name);
        let stated: Vec<f32> = attr("order").map(|text| numbers(text).take(2).collect()).unwrap_or_default();
        let order = [stated.first().copied().unwrap_or(3.0), stated.get(1).or(stated.first()).copied().unwrap_or(3.0)];
        (order.iter().all(|side| *side >= 1.0 && side.fract() == 0.0 && *side <= 1e6)).then_some(())?;
        let order = order.map(|side| side as usize);
        let kernel: Vec<f32> = attr("kernelMatrix").map(|text| numbers(text).collect()).unwrap_or_default();
        (kernel.len() == order[0] * order[1]).then_some(())?;
        let sum: f32 = kernel.iter().sum();
        let divisor = attr("divisor").and_then(number).map(|(divisor, _)| divisor).filter(|divisor| *divisor != 0.0).unwrap_or(if sum == 0.0 { 1.0 } else { sum });
        let target = |name: &str, side: usize| attr(name).and_then(number).map_or(Some(side / 2), |(at, _)| (at >= 0.0 && at < side as f32).then_some(at as usize));
        let target = [target("targetX", order[0])?, target("targetY", order[1])?];
        let edge = match attr("edgeMode").map(str::trim) {
            Some("wrap") => Edge::Wrap,
            Some("none") => Edge::None,
            _ => Edge::Duplicate,
        };
        let bias = attr("bias").and_then(number).map_or(0.0, |(bias, _)| bias);
        Some(Convolution { order, kernel, divisor, bias, target, edge, keep_alpha: attr("preserveAlpha").map(str::trim) == Some("true"), unit })
    }

    /// A box clip's box (left, top, width, height), with the transform from its units to
    /// drawing units: those of the clipped block (`units`), or for a kept box, the drawing's
    /// own.
    fn box_cut(&self, cut: Cut, units: Table) -> ([f32; 4], Table) {
        match cut {
            Cut::DrawingBox(boxed) => {
                let [left, top, right, bottom] = self.parsed.boxes[boxed as usize];
                ([left, top, right - left, bottom - top], SAME)
            }
            Cut::Box(place) => (place, units),
            _ => ([0.0; 4], units),
        }
    }

    /// The bounding box of the shapes a walk put on its block, in the walk's units, as the
    /// transform taking a unit square onto it. (Of no size where there are none.)
    fn box_of(&self, walk: &Walk) -> Table {
        let back = undone(&walk.units).unwrap_or(SAME);
        // Not the shapes of clips or tiles inside it: each such block begins where the shapes
        // so far end, and what was drawn on it is dropped with it.
        let (mut shapes, mut begun): (Vec<&Shape>, Vec<usize>) = (Vec::new(), Vec::new());
        for item in &self.parsed.items[walk.first.min(walk.last)..walk.last.min(self.parsed.items.len())] {
            match item {
                Item::Begin | Item::Tile(_) => begun.push(shapes.len()),
                Item::End(_) => _ = begun.pop(),
                Item::Cut(_) => shapes.truncate(begun.pop().unwrap_or(shapes.len())),
                Item::Shape(shape) => shapes = with_shape(shapes, &mut begun, shape),
                Item::Filter(_) => {}
            }
        }
        let corners = shapes.into_iter().flat_map(|shape| {
            let ([left, top, wide, high], table) = (bounds(&self.parsed.paths[shape.path as usize]), both(&back, &shape.table));
            [pt(left, top), pt(left + wide, top), pt(left, top + high), pt(left + wide, top + high)].map(|corner| through(&table, corner))
        });
        let (low, high) =
            corners.filter(|p| p.x.is_finite() && p.y.is_finite()).fold(([f32::MAX; 2], [f32::MIN; 2]), |(low, high), p| ([low[0].min(p.x), low[1].min(p.y)], [high[0].max(p.x), high[1].max(p.y)]));
        if low[0] > high[0] {
            return [0.0; 6];
        }
        [high[0] - low[0], 0.0, 0.0, high[1] - low[1], low[0], low[1]]
    }

    /// Keeps a clip box for content styled `style` (left, top, width, height in its units)
    /// where it is aligned with the picture: as its intersection with any enclosing box (which
    /// kept box). Nothing for a rotated or skewed box, which clips as any shape does.
    fn boxed(&mut self, [x, y, wide, high]: [f32; 4], style: &Style) -> Option<u32> {
        let [across, b, c, down, left, top] = style.table;
        if b != 0.0 || c != 0.0 {
            return None;
        }
        let [x0, x1, y0, y1] = [across * x + left, across * (x + wide) + left, down * y + top, down * (y + high) + top];
        let mut place = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        if let Some([outer_left, outer_top, outer_right, outer_bottom]) = style.boxed.map(|around| self.parsed.boxes[around as usize]) {
            place = [place[0].max(outer_left), place[1].max(outer_top), place[2].min(outer_right), place[3].min(outer_bottom)];
        }
        self.parsed.boxes.push(place);
        Some(self.parsed.boxes.len() as u32 - 1)
    }

    /// A clip path's box, where it is a single square-cornered rectangle in user units (left,
    /// top, width, height, in the units of `style`, which it clips). Nothing for any other clip
    /// path.
    fn clip_box(&mut self, at: usize, style: &Style) -> Option<[f32; 4]> {
        let only = at + 1;
        if self.file.elements[at].end != at + 2 || self.file.attribute(at, "clipPathUnits") == Some("objectBoundingBox") {
            return None;
        }
        let round = ["rx", "ry"].iter().any(|name| self.file.attribute(only, name).and_then(number).is_some_and(|(round, _)| round > 0.0));
        let clipping = self.styled(at, &Style { clipping: true, ..*style });
        let rect = self.styled(only, &clipping);
        let [across, b, c, down, left, top] = both(&undone(&style.table)?, &rect.table);
        if self.file.elements[only].name != "rect" || round || rect.gone || rect.hidden || rect.clip.is_some() || b != 0.0 || c != 0.0 {
            return None;
        }
        let [.., wide, high] = self.parsed.view;
        let side = |name: &str, whole: f32| self.file.attribute(only, name).and_then(|text| length(text, whole)).unwrap_or(0.0);
        let [x, y, w, h] = [side("x", wide), side("y", high), side("width", wide), side("height", high)];
        Some([across * x + left, down * y + top, across * w.max(0.0), down * h.max(0.0)])
    }

    /// Fits a nested viewport (an inner `<svg>`, or a `<symbol>` a `<use>` brings) into its
    /// `size` box: its `viewBox` into the box per `preserveAspectRatio`. Gives back the box in
    /// its own units, where it is clipped: none where `overflow` is visible.
    fn fit_viewport(&self, at: usize, size: [f32; 2], style: &mut Style) -> Option<[f32; 4]> {
        let view: Vec<f32> = self.file.attribute(at, "viewBox").map(|value| numbers(value).take(5).collect()).unwrap_or_default();
        let fit = match view[..] {
            [left, top, across, down] if across > 0.0 && down > 0.0 => Aspect::read(self.file.attribute(at, "preserveAspectRatio").unwrap_or("")).fit([left, top, across, down], size),
            _ => SAME,
        };
        style.table = both(&style.table, &fit);
        let [x, _, _, y, left, top] = fit;
        (!style.overflows && size.iter().all(|side| positive(*side)) && x * y != 0.0).then(|| [-left / x, -top / y, size[0] / x, size[1] / y])
    }

    /// An element's style: its parent's, overridden by its own presentation attributes, then
    /// matching style rules, then its `style` attribute.
    fn styled(&mut self, at: usize, around: &Style) -> Style {
        let black = (Paint::Own(Color::BLACK), 1.0);
        let mut style = Style { opacity: 1.0, gone: false, stop: black, clip: None, mask: None, overflows: false, filter: None, flood: black, ..*around };
        style.pen.fixed = false;
        let [first, last] = self.file.elements[at].attributes;
        self.steps += last - first + RULE * self.file.rules.len();
        for index in first..last {
            let (name, value) = self.file.attributes[index];
            self.take_property(&mut style, name, value);
        }
        for rule in 0..self.file.rules.len() {
            let (whom, declarations) = self.file.rules[rule];
            if self.file.is_for(whom, at) {
                self.take_declarations(&mut style, declarations);
            }
        }
        if let Some(declarations) = self.file.attribute(at, "style") {
            self.take_declarations(&mut style, declarations);
        }
        style
    }

    /// Takes in a declaration list: `name: value; name: value`.
    fn take_declarations(&mut self, style: &mut Style, declarations: &str) {
        for (name, value) in declarations.split(';').filter_map(|one| one.split_once(':')) {
            self.take_property(style, name.trim(), value);
        }
    }

    /// Takes in one property. (Unknown names say nothing.)
    fn take_property(&mut self, style: &mut Style, name: &str, value: &str) {
        let value = value.trim().trim_end_matches("!important").trim_end();
        let [.., wide, high] = self.parsed.view;
        let part = |fallback: f32| share(value).unwrap_or(fallback).clamp(0.0, 1.0);
        let thick = |fallback: f32| length(value, ((wide * wide + high * high) / 2.0).sqrt()).unwrap_or(fallback);
        match name {
            "fill" => style.paints[0] = self.paint(value).unwrap_or(style.paints[0]),
            "stroke" => style.paints[1] = self.paint(value).unwrap_or(style.paints[1]),
            "color" => style.ink = color(&value.to_ascii_lowercase()).or(style.ink),
            "fill-opacity" => style.solid[0] = part(style.solid[0]),
            "stroke-opacity" => style.solid[1] = part(style.solid[1]),
            "opacity" => style.opacity = part(1.0),
            "fill-rule" => style.even_odd = if value == "evenodd" { true } else { value != "nonzero" && style.even_odd },
            "stroke-width" => style.pen.width = thick(style.pen.width).max(0.0),
            "stroke-miterlimit" => style.pen.miter = number(value).map_or(style.pen.miter, |(limit, _)| limit.max(1.0)),
            "stroke-dashoffset" => style.pen.offset = thick(style.pen.offset),
            "vector-effect" => style.pen.fixed = value == "non-scaling-stroke",
            "stroke-dasharray" => style.pen.dashes = self.dashes(value),
            "stroke-linecap" => style.pen.cap = [Cap::Round, Cap::Square, Cap::Flat][["round", "square", "butt"].iter().position(|cap| *cap == value).unwrap_or(2)],
            "stroke-linejoin" => style.pen.join = [Join::Round, Join::Bevel, Join::Mitre(4.0)][["round", "bevel", "miter"].iter().position(|join| *join == value).unwrap_or(2)],
            "display" => style.gone = value == "none",
            "visibility" => style.hidden = value != "visible",
            // Stroke under fill, where it is named before fill, or fill not at all.
            "paint-order" => style.under = value.find("stroke").is_some_and(|line| value.find("fill").is_none_or(|fill| line < fill)),
            "stop-color" => style.stop.0 = plain(value).unwrap_or(style.stop.0),
            "stop-opacity" => style.stop.1 = part(1.0),
            "transform" => style.table = both(&style.table, &turned(value)),
            "clip-path" => style.clip = self.pointed_to(value, &["clipPath"]),
            "mask" => style.mask = self.pointed_to(value, &["mask"]),
            "clip-rule" => style.clip_even_odd = if value == "evenodd" { true } else { value != "nonzero" && style.clip_even_odd },
            "overflow" => style.overflows = matches!(value, "visible" | "auto"),
            "shape-rendering" => style.crisp = matches!(value, "crispEdges" | "optimizeSpeed"),
            "marker-start" => style.markers[0] = self.pointed_to(value, &["marker"]),
            "marker-mid" => style.markers[1] = self.pointed_to(value, &["marker"]),
            "marker-end" => style.markers[2] = self.pointed_to(value, &["marker"]),
            "marker" => style.markers = [self.pointed_to(value, &["marker"]); 3],
            "font-family" => style.family = self.file.place_of(value),
            "font-size" => style.font_size = font_size(value, style.font_size).unwrap_or(style.font_size),
            "font-weight" => style.weight = weight(value, style.weight),
            "font-style" => style.italic = matches!(value, "italic" | "oblique") || (value != "normal" && style.italic),
            "text-anchor" => style.anchor = ["start", "middle", "end"].iter().position(|anchor| *anchor == value).map_or(style.anchor, |at| at as f32 / 2.0),
            "letter-spacing" => style.spacing = if value == "normal" { 0.0 } else { length(value, style.font_size).unwrap_or(style.spacing) },
            "filter" => style.filter = (value != "none").then(|| self.file.place_of(value)),
            "flood-color" => style.flood.0 = plain(value).unwrap_or(style.flood.0),
            "flood-opacity" => style.flood.1 = part(1.0),
            "color-interpolation-filters" => {
                let spaces = [("linearRGB", Space::Linear), ("sRGB", Space::Screen), ("auto", Space::Taken)];
                style.space = spaces.iter().find(|(name, _)| *name == value).map_or(style.space, |(_, space)| *space);
            }
            _ => {}
        }
    }

    /// A fill or stroke value: `none`, a colour, `currentColor`, or `url(#…)` to a gradient or
    /// pattern with an optional fallback. Nothing for anything else, which changes nothing.
    fn paint(&self, text: &str) -> Option<Paint> {
        let Some(link) = text.strip_prefix("url(") else { return plain(text) };
        let or_else = link.split_once(')').map_or("", |(_, rest)| rest);
        let to = self.pointed_to(text, &["linearGradient", "radialGradient", "pattern"]);
        let paint = to.map(|at| if self.file.elements[at].name == "pattern" { Paint::Tiles(at) } else { Paint::Gradient(at) });
        paint.or_else(|| plain(or_else)).or(Some(Paint::None))
    }

    /// The element `url(#id)` references, if of one of the kinds named. (Nothing for `none` or
    /// a dangling reference.)
    fn pointed_to(&self, text: &str, kinds: &[&str]) -> Option<usize> {
        let id = text.trim().strip_prefix("url(")?.split_once(')')?.0;
        let id = id.trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'')).trim_start_matches('#');
        self.file.named.get(id).copied().filter(|at| kinds.contains(&self.file.elements[*at].name))
    }

    /// Keeps a stroke's dash array (dash and gap lengths alternating). Says where it is kept.
    /// (None where the lengths make no dashes: a negative one, or a zero total.)
    fn dashes(&mut self, text: &str) -> [u32; 2] {
        let [.., wide, high] = self.parsed.view;
        let first = self.parsed.dashes.len();
        let lengths = text.split(|c: char| c == ',' || c.is_whitespace()).filter(|length| !length.is_empty());
        self.parsed.dashes.extend(lengths.map_while(|text| length(text, ((wide * wide + high * high) / 2.0).sqrt())));
        let dashes = &self.parsed.dashes[first..];
        if dashes.iter().any(|dash| *dash < 0.0) || !positive(dashes.iter().sum::<f32>()) {
            self.parsed.dashes.truncate(first);
        }
        [first as u32, self.parsed.dashes.len() as u32]
    }

    /// Keeps the shape an element is, styled `style`, with the pattern tiles `tiles` for fill
    /// and stroke. Gives back the walk through its markers, if any. (A non-shape is none; an
    /// invisible one is not kept.)
    fn shape(&mut self, at: usize, name: &str, style: &Style, tiles: [Option<u32>; 2]) -> Option<Walk> {
        if name == "image" {
            // Once a program asks: see `enable_images`.
            if let Some(picture) = PICTURES.get() {
                picture(self, at, style, None);
            }
            return None;
        }
        if name == "text" {
            // Once a program asks: see `enable_text`.
            let pieces = TEXT.get().map(|text| text(self, at, style)).unwrap_or_default();
            pieces.into_iter().for_each(|(path, style)| self.keep(path, &style, [None; 2]));
            return None;
        }
        let path = self.outline(at, name).filter(|_| !style.hidden)?;
        self.keep(path, style, tiles);
        (matches!(name, "path" | "line" | "polyline" | "polygon") && style.markers != [None; 3]).then(|| self.markers(path, style)).flatten()
    }

    /// Keeps an image's picture, styled `style`: in its box (`place` where given, else as its
    /// element says: one unit per pixel where it gives no size), fitted per
    /// `preserveAspectRatio`, clipped at its edges. An SVG file is read once the file's own is
    /// (see `read_later`). One not to be had, or unreadable, is left out, as is one in a clip
    /// path (outlines only).
    fn picture(&mut self, at: usize, style: &Style, place: Option<[f32; 4]>) {
        if style.hidden || style.clipping {
            return;
        }
        if !self.pictured.contains_key(&at) {
            let read = self.picture_of(at).unwrap_or(Pictured::Nothing);
            self.pictured.insert(at, read);
        }
        match self.pictured.get(&at).cloned() {
            Some(Pictured::Picture(kept)) => self.keep_picture(at, kept, style, place),
            Some(Pictured::Drawing(text)) => self.drawing_later(at, text, style, place),
            _ => {}
        }
    }

    /// Keeps a shape showing kept picture `kept` for image element `at` (see `picture`).
    fn keep_picture(&mut self, at: usize, kept: u32, style: &Style, place: Option<[f32; 4]>) {
        let picture = &self.parsed.more().pictures[kept as usize];
        let [wide, high] = [picture.wide as f32, (picture.pixels.len() / picture.wide) as f32];
        let [x, y, w, h] = self.image_box(at, place);
        let [x, y, w, h] = [x.unwrap_or(0.0), y.unwrap_or(0.0), w.unwrap_or(wide), h.unwrap_or(high)];
        let fit = both(&[1.0, 0.0, 0.0, 1.0, x, y], &Aspect::read(self.file.attribute(at, "preserveAspectRatio").unwrap_or("")).fit([0.0, 0.0, wide, high], [w, h]));
        // It shows where the picture and its box overlap: in picture pixels.
        let [left, top] = [(x - fit[4]) / fit[0], (y - fit[5]) / fit[3]].map(|side| side.max(0.0));
        let [right, bottom] = [(x + w - fit[4]) / fit[0], (y + h - fit[5]) / fit[3]];
        let [right, bottom] = [right.min(wide), bottom.min(high)];
        let solid = style.faded * style.opacity;
        if !positive(solid) || !(right > left && bottom > top) {
            return;
        }
        let path = self.kept_path(rect([left, top, right - left, bottom - top], [None; 2]));
        let shown = Style { table: both(&style.table, &fit), ..*style };
        self.pieces += 5;
        self.spend(self.covers(path, &shown, false) * TILES);
        let pen = self.pen(style);
        self.parsed.items.push(Item::Shape(Shape { path, table: shown.table, fill: Ink::Picture(kept, solid), even_odd: false, line: Ink::None, pen, boxed: style.boxed, crisp: false }));
    }

    /// An image's box: left, top, width and height, as `place` says where given, else each as
    /// its element says, where it does.
    fn image_box(&self, at: usize, place: Option<[f32; 4]>) -> [Option<f32>; 4] {
        if let Some(place) = place {
            return place.map(Some);
        }
        let [.., across, down] = self.parsed.view;
        let length_of = |name: &str, whole: f32| self.file.attribute(at, name).and_then(|text| length(text, whole));
        [length_of("x", across), length_of("y", down), length_of("width", across), length_of("height", down)]
    }

    /// Reads an image: a picture is kept, an SVG file's text kept to read later. Nothing if it
    /// holds none readable. The bytes come from a `data:` URL, else from the app's loader (see
    /// `enable_images_from`); reading counts a step per 64 bytes before and per 64 pixels after.
    fn picture_of(&mut self, at: usize) -> Option<Pictured> {
        let href = self.file.attribute(at, "href").or_else(|| self.file.attribute(at, "xlink:href"))?;
        let loaded = || LOADER.get().filter(|_| !href.trim_start().starts_with("data:")).and_then(|load| load(href.trim()));
        let bytes = data(href).or_else(loaded)?;
        self.steps += bytes.len() / 64;
        if self.spent() {
            return None;
        }
        if let Some(text) = svg_text(&bytes) {
            return Some(Pictured::Drawing(text));
        }
        let (wide, pixels) = crate::ui::read_picture(&bytes)?;
        self.steps += pixels.len() / 64;
        let pixels = pixels.into_iter().map(premultiplied).collect();
        let pictures = &mut self.parsed.more().pictures;
        pictures.push(Picture { pixels, wide });
        Some(Pictured::Picture(pictures.len() as u32 - 1))
    }

    /// Waits to read the SVG file image element `at` holds (see `picture`), as deep as drawings
    /// may be brought.
    fn drawing_later(&mut self, at: usize, text: Rc<str>, style: &Style, place: Option<[f32; 4]>) {
        let aspect = Aspect::read(self.file.attribute(at, "preserveAspectRatio").unwrap_or(""));
        let solid = style.faded * style.opacity;
        let later = Later {
            at: self.parsed.items.len(),
            brought: Brought::File(text),
            table: style.table,
            share: style.share,
            place: self.image_box(at, place),
            aspect,
            solid,
            boxed: style.boxed,
            depth: self.depth + 1,
        };
        if positive(solid) {
            self.wait(later);
        }
    }

    /// Keeps a drawing to read once the file's own is, unless brought too deep.
    fn wait(&mut self, later: Later) {
        if later.depth <= BROUGHT {
            self.parsed.more().later.push(later);
            self.later = Some(Reading::read_later);
        }
    }

    /// Reads the drawings waiting (`More::later`), and those they bring in turn, each as far as
    /// the budget goes; then puts each one's items where it was met, the innermost first, so
    /// that the places met later stay right.
    fn read_later(&mut self) {
        self.later = None;
        // Each drawing read: the one it was met in (none: this one), where, and its items.
        let mut read: Vec<(Option<usize>, usize, Vec<Item>)> = Vec::new();
        let mut waiting: std::collections::VecDeque<(Option<usize>, Later)> = std::mem::take(&mut self.parsed.more().later).into_iter().map(|later| (None, later)).collect();
        while let Some((within, later)) = waiting.pop_front() {
            let at = later.at;
            let items = self.read_one(later);
            let brought = std::mem::take(&mut self.parsed.more().later);
            waiting.extend(brought.into_iter().map(|later| (Some(read.len()), later)));
            read.push((within, at, items));
        }
        for n in (0..read.len()).rev() {
            let (within, at, items) = (read[n].0, read[n].1, std::mem::take(&mut read[n].2));
            let list = match within {
                Some(within) => &mut read[within].2,
                None => &mut self.parsed.items,
            };
            let at = at.min(list.len());
            list.splice(at..at, items);
        }
    }

    /// Reads one waiting drawing (see `read_later`) into what this reading keeps: its items, as
    /// they go in where it was met. None once the budget is spent.
    fn read_one(&mut self, later: Later) -> Vec<Item> {
        if self.spent() {
            return Vec::new();
        }
        match &later.brought {
            Brought::Element(at) => self.read_element(*at, &later),
            Brought::File(text) => self.read_file(text, &later),
        }
    }

    /// Reads an element of this file as a drawing of its own (see `read_later`), in the units
    /// `later` gives, onto the block begun for it.
    fn read_element(&mut self, at: usize, later: &Later) -> Vec<Item> {
        let file = std::mem::replace(&mut self.file, File::read(""));
        let mut brought = Reading::with(file, (self.parsed.size, self.parsed.view, self.parsed.aspect), self.filtering);
        self.lend_kept(&mut brought);
        self.lend_cached(&mut brought);
        brought.depth = later.depth;
        brought.walk(at, Style { table: later.table, share: later.share, ..Style::default() });
        self.lend_kept(&mut brought);
        self.lend_cached(&mut brought);
        self.file = brought.file;
        brought.parsed.items
    }

    /// Reads another SVG file (see `read_later`): its view fitted into its box, on a block of
    /// its own clipped to that box and composited at its opacity. (Parsing it costs a step per
    /// 16 bytes.)
    fn read_file(&mut self, text: &str, later: &Later) -> Vec<Item> {
        self.steps += text.len() / 16;
        let Some((mut brought, start)) = Reading::prepared(text, None).filter(|_| !self.spent()) else { return Vec::new() };
        let [x, y] = [later.place[0].unwrap_or(0.0), later.place[1].unwrap_or(0.0)];
        let [w, h] = [later.place[2].unwrap_or(brought.parsed.size[0]), later.place[3].unwrap_or(brought.parsed.size[1])];
        if !(positive(w) && positive(h)) {
            return Vec::new();
        }
        let fit = later.aspect.fit(brought.parsed.view, [w, h]);
        let table = both(&later.table, &both(&[1.0, 0.0, 0.0, 1.0, x, y], &fit));
        // It shows as a picture of its view would: where its view, fitted, lies in its box.
        let [left, top, across, down] = brought.parsed.view;
        let [shown_left, shown_top] = [(x + fit[4] + left * fit[0]).max(x), (y + fit[5] + top * fit[3]).max(y)];
        let [shown_right, shown_bottom] = [(x + fit[4] + (left + across) * fit[0]).min(x + w), (y + fit[5] + (top + down) * fit[3]).min(y + h)];
        self.lend_kept(&mut brought);
        brought.depth = later.depth;
        brought.walk(start, Style { table, share: later.share, boxed: later.boxed, ..Style::default() });
        self.lend_kept(&mut brought);
        let style = Style { table: later.table, share: later.share, ..Style::default() };
        let path = self.kept_path(rect([shown_left, shown_top, shown_right - shown_left, shown_bottom - shown_top], [None; 2]));
        self.spend(2.0 * later.share[1] + self.covers(path, &style, false));
        let pen = self.pen(&style);
        let edge = Shape { path, table: later.table, fill: Ink::Own(Color::WHITE), even_odd: false, line: Ink::None, pen, boxed: None, crisp: false };
        let mut items = vec![Item::Begin];
        items.append(&mut brought.parsed.items);
        items.extend([Item::Begin, Item::Shape(edge), Item::Cut(Keep::Solid), Item::End(later.solid)]);
        items
    }

    /// Swaps what a drawing being brought shares with this one (see `read_later`): all it keeps,
    /// so that indices hold in either; the costs so far and the budget; and the fonts found.
    /// Swapped again, all is given back.
    fn lend_kept(&mut self, other: &mut Reading<'_>) {
        let (mine, theirs) = (&mut self.parsed, &mut other.parsed);
        std::mem::swap(&mut mine.paths, &mut theirs.paths);
        std::mem::swap(&mut mine.pens, &mut theirs.pens);
        std::mem::swap(&mut mine.dashes, &mut theirs.dashes);
        std::mem::swap(&mut mine.gradients, &mut theirs.gradients);
        std::mem::swap(&mut mine.stops, &mut theirs.stops);
        std::mem::swap(&mut mine.tiles, &mut theirs.tiles);
        std::mem::swap(&mut mine.boxes, &mut theirs.boxes);
        std::mem::swap(&mut mine.more, &mut theirs.more);
        std::mem::swap(&mut mine.uses_current_color, &mut theirs.uses_current_color);
        std::mem::swap(&mut self.pieces, &mut other.pieces);
        std::mem::swap(&mut self.steps, &mut other.steps);
        std::mem::swap(&mut self.lookups, &mut other.lookups);
        std::mem::swap(&mut self.passes, &mut other.passes);
        std::mem::swap(&mut self.budget, &mut other.budget);
        std::mem::swap(&mut self.fonts, &mut other.fonts);
        std::mem::swap(&mut self.files, &mut other.files);
    }

    /// Swaps what this reading caches by element with one of an element of this same file
    /// (see `lend_kept`).
    fn lend_cached(&mut self, other: &mut Reading<'_>) {
        std::mem::swap(&mut self.outlined, &mut other.outlined);
        std::mem::swap(&mut self.gradients_read, &mut other.gradients_read);
        std::mem::swap(&mut self.pictured, &mut other.pictured);
        std::mem::swap(&mut self.faces, &mut other.faces);
    }

    /// Keeps a shape with outline `path`, styled `style` (with tiles `tiles`).
    fn keep(&mut self, path: u32, style: &Style, tiles: [Option<u32>; 2]) {
        let pen = self.pen(style);
        // In a clip path only the outline counts, filled opaque.
        if style.clipping {
            self.pieces += self.parsed.paths[path as usize].steps().len();
            self.spend(self.covers(path, style, false));
            self.parsed.items.push(Item::Shape(Shape {
                path,
                table: style.table,
                fill: Ink::Own(Color::WHITE),
                even_odd: style.clip_even_odd,
                line: Ink::None,
                pen,
                boxed: style.boxed,
                crisp: style.crisp,
            }));
            return;
        }
        // A semi-transparent shape both filled and stroked composites as one, else its stroke
        // would show its fill's edge through. (Not one filled with tiles, whose blocks lie
        // where that one would begin.)
        let layered = style.opacity < 1.0 && style.paints[0] != Paint::None && style.paints[1] != Paint::None && style.pen.width > 0.0 && tiles == [None; 2];
        let solid = style.faded * if layered { 1.0 } else { style.opacity };
        let fill = self.ink(style.paints[0], style.solid[0] * solid, style.ink, path, tiles[0]);
        let line = if style.pen.width > 0.0 { self.ink(style.paints[1], style.solid[1] * solid, style.ink, path, tiles[1]) } else { Ink::None };
        // A stroke costing more than the budget left is not drawn: one shape alone could cost
        // all of it and more (see `spent`).
        let (steps, [_, outlines]) = (self.parsed.paths[path as usize].steps().len(), most(self.budget));
        let lined = if line == Ink::None { 0 } else { self.line_pieces(path, style) };
        let (line, lined) = if self.pieces + 2 * steps + lined > outlines { (Ink::None, 0) } else { (line, lined) };
        if fill == Ink::None && line == Ink::None {
            return;
        }
        // A shape stroked under its fill is kept as two: stroke, then fill.
        let (even_odd, table, boxed, crisp) = (style.even_odd, style.table, style.boxed, style.crisp);
        let shapes = match style.under {
            true => vec![Shape { path, table, fill: Ink::None, even_odd, line, pen, boxed, crisp }, Shape { path, table, fill, even_odd, line: Ink::None, pen, boxed, crisp }],
            false => vec![Shape { path, table, fill, even_odd, line, pen, boxed, crisp }],
        };
        self.pieces += steps * shapes.len() + lined;
        // Fill and stroke each cover their bounding box's share of the block, more for
        // per-pixel paints; a stroke clipped by a box is drawn on its own block, which is
        // covered again to clip it.
        let worth = |ink: Ink| match ink {
            Ink::None => 0.0,
            Ink::Own(_) | Ink::CurrentColor(_) => 1.0,
            Ink::Gradient(..) => BLENDS,
            Ink::Tiles(..) | Ink::Picture(..) => TILES,
        };
        let boxed_line = if boxed.is_some() && line != Ink::None { 2.0 * style.share[1] } else { 0.0 };
        let crisp = if style.crisp { 2.0 } else { 1.0 };
        self.spend(self.covers(path, style, line != Ink::None) * (worth(fill) + worth(line)) * crisp + boxed_line);
        if layered {
            self.begin(style);
        }
        self.parsed.items.extend(shapes.into_iter().map(Item::Shape));
        self.parsed.items.extend(layered.then_some(Item::End(style.opacity)));
    }

    /// The letters of a text element and its `<tspan>`s and `<textPath>`s, styled `style`: each
    /// piece's letters as one kept outline, with its style. Each letter is its glyph's outline
    /// in the system font of the first installed family its piece names (shaped as pages are,
    /// with the feature `shaping`), set along a line from the positions the text and its pieces
    /// give, or along a text path; each chunk starting at a position of its own is placed there
    /// by its anchor. Whitespace collapses as in a browser: trimmed at both ends, runs made one.
    fn text(&mut self, at: usize, style: &Style) -> Vec<(u32, Style)> {
        let end = self.file.elements[at].end;
        self.steps += end - at + self.file.words.len();
        let mut pieces = vec![self.piece(at, *style, 0)];
        for inner in at + 1..end {
            // A piece belongs to the innermost piece around it; anything else (such as a title)
            // writes nothing.
            let around = pieces.iter().rposition(|piece| inner < self.file.elements[piece.at].end).unwrap_or(0);
            if matches!(self.file.elements[inner].name, "tspan" | "a" | "textPath") {
                let style = self.styled(inner, &pieces[around].style);
                pieces.push(self.piece(inner, style, around));
            }
        }
        let words: Vec<(usize, String)> =
            self.file.words.iter().filter(|(owner, _)| (at..end).contains(owner)).filter_map(|(owner, words)| Some((pieces.iter().position(|piece| piece.at == *owner)?, characters(words)))).collect();
        let letters = self.given(&words, &mut pieces);
        let mut pen = pt(0.0, 0.0);
        let mut from = 0;
        while from < letters.len() {
            let path = on_path(&pieces, letters[from].piece);
            let starts = |given: &Given| given.at[0].is_some() || given.at[1].is_some() || on_path(&pieces, given.piece) != path;
            let to = from + 1 + letters[from + 1..].iter().take_while(|given| !starts(given)).count();
            pen = self.chunk(&letters[from..to], &mut pieces, pen);
            from = to;
        }
        // Text is drawn smooth whatever `shape-rendering` says.
        let outlines: Vec<(Path, Style)> = pieces.into_iter().filter(|piece| !piece.outline.steps().is_empty()).map(|piece| (piece.outline, Style { crisp: false, ..piece.style })).collect();
        outlines.into_iter().map(|(outline, style)| (self.kept_path(outline), style)).collect()
    }

    /// A text piece as it starts: its element, style, parent piece, its letters' given
    /// positions (x, y, dx, dy), and a `<textPath>`'s path.
    fn piece(&mut self, at: usize, style: Style, around: usize) -> Piece {
        let [.., wide, high] = self.parsed.view;
        let list =
            |name: &str, whole: f32| self.file.attribute(at, name).map(|value| value.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|one| length(one, whole)).collect()).unwrap_or_default();
        let places = [list("x", wide), list("y", high), list("dx", wide), list("dy", high)];
        let path = (self.file.elements[at].name == "textPath").then(|| self.text_path(at));
        Piece { at, style, around, places, letters: 0, path, outline: Path::new() }
    }

    /// A `<textPath>`'s path in the text's units, laid flat: as its `path` says, else the shape
    /// it references (with that shape's own transform); and where along it the text starts
    /// (`startOffset`, a length or a share of the path's). An empty track for one naming no
    /// shape: its letters then show nowhere. (Laying it flat costs a step per segment.)
    fn text_path(&mut self, at: usize) -> (Track, f32) {
        let path = match self.file.attribute(at, "path") {
            Some(d) => Path::svg(d),
            None => self.shape_path(at).unwrap_or_default(),
        };
        let track = Track::of(&path);
        self.steps += track.segments.len();
        let from = self.file.attribute(at, "startOffset").and_then(|text| length(text, track.length)).unwrap_or(0.0);
        (track, from)
    }

    /// The outline of the shape an element references, with that shape's own transform.
    fn shape_path(&mut self, at: usize) -> Option<Path> {
        let shape = self.file.linked(at)?;
        let kept = self.outline(shape, self.file.elements[shape].name)?;
        let mut path = self.parsed.paths[kept as usize].clone();
        path.transform(Transform(self.file.attribute(shape, "transform").map_or(SAME, turned)));
        Some(path)
    }

    /// A text's letters, each with its piece and given position: whitespace collapsed (see
    /// `text`), as many as the budget allows. (A text can stand for as much as a file could.)
    fn given(&mut self, words: &[(usize, String)], pieces: &mut [Piece]) -> Vec<Given> {
        let (mut letters, mut space) = (Vec::new(), None);
        for (piece, letter) in words.iter().flat_map(|(piece, words)| words.chars().map(move |letter| (*piece, letter))) {
            if self.spent() {
                break;
            }
            if letter.is_whitespace() {
                space = (!letters.is_empty()).then_some(piece);
                continue;
            }
            if let Some(space) = space.take() {
                letters.push(given(' ', space, pieces));
            }
            letters.push(given(letter, piece, pieces));
            self.steps += LETTER;
        }
        letters
    }

    /// Sets a chunk of letters (see `text`): from where its first letter is given, else where
    /// the previous chunk ended (`pen`), or along its text path; placed by its first letter's
    /// anchor. Gives where it ends.
    fn chunk(&mut self, letters: &[Given], pieces: &mut [Piece], pen: Point) -> Point {
        let first = &letters[0];
        let path = on_path(pieces, first.piece);
        let start = match path {
            Some(_) => pt(first.at[0].unwrap_or(0.0), 0.0),
            None => pt(first.at[0].unwrap_or(pen.x), first.at[1].unwrap_or(pen.y)),
        };
        let line = self.line(letters, pieces, start);
        let back = (line.pen.x - line.apart - start.x) * pieces[first.piece].style.anchor;
        let Some((track, from)) = path.and_then(|path| pieces[path].path.as_ref()) else {
            for Set { piece, mut outline, .. } in line.set {
                outline.transform(Transform([1.0, 0.0, 0.0, 1.0, -back, 0.0]));
                pieces[piece].outline.add(&outline);
            }
            return line.pen;
        };
        // Each glyph turned to the path's direction where its middle falls; one falling off
        // either end is left out.
        let mut placed = Vec::with_capacity(line.set.len());
        for Set { piece, mut outline, middle } in line.set {
            let along = from + middle - back;
            let Some((p, [cos, sin])) = track.point_at(along).filter(|_| (0.0..=track.length).contains(&along)) else { continue };
            outline.transform(Transform([cos, sin, -sin, cos, p.x - cos * middle, p.y - sin * middle]));
            placed.push((piece, outline));
        }
        let end = track.point_at(from + line.pen.x - line.apart - back).map_or(pen, |(p, _)| p);
        for (piece, outline) in placed {
            pieces[piece].outline.add(&outline);
        }
        end
    }

    /// Sets a chunk's letters along a line from `start`, each in its piece's font, kerned by
    /// pairs, spaced and moved as given.
    #[cfg(not(feature = "shaping"))]
    fn line(&mut self, letters: &[Given], pieces: &[Piece], start: Point) -> Line {
        let mut line = Line { set: Vec::new(), pen: start, apart: 0.0 };
        // The letter before: its font, letter and size.
        let mut before: Option<(usize, char, f32)> = None;
        for given in letters {
            line.pen = pt(line.pen.x + given.at[2].unwrap_or(0.0), line.pen.y + given.at[3].unwrap_or(0.0));
            let (style, letter) = (pieces[given.piece].style, given.letter);
            let face = self.face(style.family, style.weight, style.italic);
            let Some(font) = self.font(face, letter) else { continue };
            let size = style.font_size;
            // Two letters in the same font are kerned.
            let kerned = before.filter(|(font_before, _, size_before)| *font_before == font && *size_before == size);
            let kerned = kerned.map_or(0.0, |(_, before, _)| self.file_of(font).map_or(0.0, |file| file.kerning(before, letter, size)));
            let Some(file) = self.file_of(font) else { continue };
            let mut outline = file.outline(letter, size);
            let (x, advance) = (line.pen.x + kerned, file.advance(letter, size));
            outline.transform(Transform([1.0, 0.0, 0.0, 1.0, x, line.pen.y - file.metrics(size).ascent]));
            line.set.push(Set { piece: given.piece, outline, middle: x + advance / 2.0 });
            line.pen = pt(x + advance + style.spacing, line.pen.y);
            (line.apart, before) = (style.spacing, Some((font, letter, size)));
        }
        line
    }

    /// Sets a chunk's letters along a line from `start` as pages set text: in the order shown,
    /// right-to-left stretches reversed (with `right-to-left`); each run of one piece and font
    /// shaped by its script's plan; spaced and moved as given.
    #[cfg(feature = "shaping")]
    fn line(&mut self, letters: &[Given], pieces: &[Piece], start: Point) -> Line {
        let text: String = letters.iter().map(|given| given.letter).collect();
        let mut stretches = vec![(0..text.len(), 0)];
        if !crate::bidi::plain(&text) {
            let mut order = crate::bidi::Order::default();
            order.of(&text, None);
            order.shown(&text, &mut stretches);
        }
        // Each letter's first byte, for turning stretches into letters.
        let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
        let fonts = self.letter_fonts(letters, pieces, &text);
        let mut shaper = Shaper { line: Line { set: Vec::new(), pen: start, apart: 0.0 }, plans: Vec::new(), laid: Vec::new(), moved: vec![false; letters.len()] };
        for (bytes, level) in stretches {
            let [first, last] = [bytes.start, bytes.end].map(|byte| starts.partition_point(|at| *at < byte));
            let mut runs = runs(&letters[..last], &fonts, first);
            if level % 2 == 1 {
                runs.reverse();
            }
            for run in runs {
                self.shape_run(&letters[run.clone()], run.start, (&fonts, pieces, level % 2 == 1), &mut shaper);
            }
        }
        shaper.line
    }

    /// The font each letter of a chunk (as `text`) is drawn in, as its piece's style finds it:
    /// a grapheme cluster's in its first letter's font where that has them (an accent with its
    /// letter, a joiner with its syllable).
    #[cfg(feature = "shaping")]
    fn letter_fonts(&mut self, letters: &[Given], pieces: &[Piece], text: &str) -> Vec<Option<usize>> {
        let mut fonts = Vec::with_capacity(letters.len());
        let (mut at, mut n) = (0, 0);
        while at < text.len() {
            let end = at + crate::text::cluster(&text[at..]).max(1);
            let mut cluster: Option<usize> = None;
            for given in &letters[n..n + text[at..end].chars().count()] {
                let has = |font: usize| self.file_of(font).is_some_and(|file| file.glyph(given.letter) != 0);
                let kept = cluster.filter(|font| has(*font));
                let font = if kept.is_some() { kept } else { self.letter_font(given, pieces) };
                cluster = cluster.or(font);
                fonts.push(font);
            }
            (at, n) = (end, fonts.len());
        }
        fonts
    }

    /// The font a letter of a chunk is drawn in, as its piece's style finds it.
    #[cfg(feature = "shaping")]
    fn letter_font(&mut self, given: &Given, pieces: &[Piece]) -> Option<usize> {
        let style = pieces[given.piece].style;
        let face = self.face(style.family, style.weight, style.italic);
        self.font(face, given.letter)
    }

    /// Shapes a run of a chunk's letters (the first being letter `first` of the chunk) and
    /// sets its glyphs after those set so far: each letter's movement (dx, dy) before its first
    /// glyph, its spacing after its last. (`fonts` are the chunk's letters' fonts.)
    #[cfg(feature = "shaping")]
    fn shape_run(&mut self, letters: &[Given], first: usize, (fonts, pieces, leftward): (&[Option<usize>], &[Piece], bool), shaper: &mut Shaper) {
        let (Some(font), style) = (fonts[first], pieces[letters[0].piece].style) else { return };
        let Some(file) = self.file_of(font) else { return };
        let text: String = letters.iter().map(|given| given.letter).collect();
        let script = text.chars().find_map(crate::shaping::Script::of).unwrap_or(crate::shaping::Script::None);
        let plan = match shaper.plans.iter().position(|(of, known, way, _)| (*of, *known, *way) == (font, script, leftward)) {
            Some(plan) => plan,
            None => {
                shaper.plans.push((font, script, leftward, crate::shaping::Plan::of(file, script, leftward, crate::scripts::Language::NONE)));
                shaper.plans.len() - 1
            }
        };
        file.shape(&shaper.plans[plan].3, &text, &mut shaper.laid);
        let unit = file.unit(style.font_size);
        let line = &mut shaper.line;
        for (n, laid) in shaper.laid.iter().enumerate() {
            let own = first + laid.from as usize;
            if !std::mem::replace(&mut shaper.moved[own], true) {
                let at = letters[laid.from as usize].at;
                line.pen = pt(line.pen.x + at[2].unwrap_or(0.0), line.pen.y + at[3].unwrap_or(0.0));
            }
            let [dx, dy] = laid.offset.map(|offset| offset as f32 * unit);
            let (outline, _) = file.outline_through(laid.glyph, [unit, 0.0, 0.0, -unit, line.pen.x + dx, line.pen.y - dy]);
            let advance = laid.advance as f32 * unit;
            line.set.push(Set { piece: letters[0].piece, outline, middle: line.pen.x + advance / 2.0 });
            line.pen.x += advance;
            // A letter's spacing follows its last glyph.
            if shaper.laid.get(n + 1).is_none_or(|next| next.from != laid.from) {
                (line.pen.x, line.apart) = (line.pen.x + style.spacing, style.spacing);
            }
        }
        self.steps += LETTER * shaper.laid.len();
    }

    /// The system font face for a family list (as written, where in the file): the first
    /// installed family, or a generic one the system resolves (`serif`, `sans-serif`,
    /// `monospace`); else the last. (Each looked up once.)
    fn face(&mut self, family: [u32; 2], weight: u16, italic: bool) -> Face {
        if let Some((.., face)) = self.faces.iter().find(|(named, heavy, leans, _)| (*named, *heavy, *leans) == (family, weight, italic)) {
            return *face;
        }
        // Past the lookup budget the system is not asked whether it has a family: see
        // `LOOKUPS`.
        let looks = self.looks();
        let names: Vec<&str> =
            self.file.text.get(family[0] as usize..family[1] as usize).unwrap_or("").split(',').map(|name| name.trim().trim_matches(['"', '\''])).filter(|name| !name.is_empty()).collect();
        let standing = |name: &str| ["serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui"].contains(&name.to_ascii_lowercase().as_str());
        let has = |name: &str| standing(name) || (looks && !crate::system_fonts::styles(name).is_empty());
        let name = names.iter().copied().find(|name| has(name)).or(names.last().copied()).unwrap_or("sans-serif");
        let face = Face::named(name).weight(weight);
        let face = if italic { face.italic() } else { face };
        self.faces.push((family, weight, italic, face));
        face
    }

    /// The font a face's letter is drawn in, as the system finds it (each once): which found
    /// font. The face's own where it has the letter, else the system's fallback for it.
    fn font(&mut self, face: Face, letter: char) -> Option<usize> {
        let own = self.found(face, '\0');
        if self.file_of(own).is_some_and(|font| font.glyph(letter) != 0) {
            return Some(own);
        }
        // A font found for another letter of the face that has this one is what the system
        // would find: a line of letters the face lacks is looked up once.
        let before = self.fonts.iter().position(|(of, _, file)| *of == face && file.is_some_and(|file| self.files[file as usize].glyph(letter) != 0));
        let other = before.unwrap_or_else(|| self.found(face, letter));
        [other, own].into_iter().find(|at| self.fonts[*at].2.is_some())
    }

    /// Whether system fonts may still be looked up once more (see `LOOKUPS`, `HELD`).
    fn looks(&mut self) -> bool {
        self.lookups += 1;
        self.lookups <= LOOKUPS.max(self.budget / 512) && self.files.iter().map(|file| file.bytes().0.len()).sum::<usize>() < HELD
    }

    /// A found font's file, if any.
    fn file_of(&self, font: usize) -> Option<&FontFile> {
        self.fonts[font].2.map(|file| &self.files[file as usize])
    }

    /// Which found font the system draws `letter` of `face` in (the face's own font for a zero
    /// letter), found now if not before.
    fn found(&mut self, face: Face, letter: char) -> usize {
        if let Some(at) = self.fonts.iter().position(|(own, of, _)| *own == face && *of == letter) {
            return at;
        }
        // Past the lookup budget a letter has no font: see `LOOKUPS`.
        // (The face's own font is the one it draws a space in: every font has one, whatever its
        // script.)
        let file = if self.looks() { crate::system_fonts::file_for(face, if letter == '\0' { ' ' } else { letter }) } else { None };
        // A file found before for another letter is kept once: one copy per letter the face
        // lacks would come to gigabytes.
        let same = |kept: &FontFile, file: &FontFile| kept.start == file.start && kept.varied.is_none() && kept.bytes().0 == file.bytes().0;
        let at = file.map(|file| match self.files.iter().position(|kept| same(kept, &file)) {
            Some(at) => at as u32,
            None => {
                self.files.push(file);
                self.files.len() as u32 - 1
            }
        });
        self.fonts.push((face, letter, at));
        self.fonts.len() - 1
    }

    /// Whether what is read so far exceeds what a file this long could stand for. A few
    /// mutually referencing elements can stand for more shapes than any file could hold: no
    /// more shapes are kept than a file this long could write out, nor more outline segments
    /// (each takes a few bytes at least), nor more steps than those shapes could need, nor more
    /// full-picture passes than such a file could cause (see `passes`).
    fn spent(&self) -> bool {
        let long = self.budget;
        let [shapes, outlines] = most(long);
        self.steps > 16 * shapes || self.parsed.items.len() > shapes || self.pieces > outlines || self.passes > passes(long)
    }

    /// The outline segments a shape's stroke costs beyond the outline's own: each segment
    /// counts its stroke width over the block's (a stroke as wide as the block crosses all of
    /// it, at its corners), more with round joins; and each dash as a segment (up to the 10,000
    /// dashes `pen.rs` cuts a ring into).
    fn line_pieces(&self, path: u32, style: &Style) -> usize {
        let steps = self.parsed.paths[path as usize].steps();
        let [a, b, c, d, ..] = style.table;
        let (side, wide) = ((style.share[1] / style.share[0]).sqrt(), style.pen.width * (a * d - b * c).abs().sqrt());
        // Round joins and caps are arcs of more points the wider they are: up to 256 for one as
        // wide as a 4096-pixel picture.
        let across = (wide / side).min(1.0);
        let round = matches!(style.pen.join, Join::Round) || style.pen.cap == Cap::Round;
        let each = LINED * across + if round { ROUNDS * across.sqrt() } else { 0.0 };
        let dashes = &self.parsed.dashes[style.pen.dashes[0] as usize..style.pen.dashes[1] as usize];
        let rings = steps.iter().filter(|step| matches!(step, Step::Move(_))).count() as f32;
        let cut = (reach_along(steps) * dashes.len() as f32 / (2.0 * dashes.iter().sum::<f32>())).min(10_000.0 * rings);
        let more = steps.len() as f32 * each + if dashes.is_empty() { 0.0 } else { cut * (1.0 + each) };
        more.min(usize::MAX as f32) as usize
    }

    /// Counts a drawing cost in full-picture passes. (A NaN costs nothing: nothing is drawn of
    /// it.)
    fn spend(&mut self, passes: f32) {
        self.passes += passes.max(0.0);
    }

    /// Begins a block for what follows, the size of the one `style` is drawn on.
    fn begin(&mut self, style: &Style) {
        self.spend(style.share[1]);
        self.parsed.items.push(Item::Begin);
    }

    /// How much of the picture a shape's bounding box covers (with its stroke, if `lined`),
    /// styled `style`: at most its whole block.
    fn covers(&self, path: u32, style: &Style, lined: bool) -> f32 {
        let Some([left, top, right, bottom]) = reach(&self.parsed.paths[path as usize]) else { return 0.0 };
        // A miter sticks out further than half the stroke width: as far as the limit allows.
        let out = if lined { style.pen.width * style.pen.miter.max(2.0) / 2.0 } else { 0.0 };
        let corners = [pt(left - out, top - out), pt(right + out, top - out), pt(left - out, bottom + out), pt(right + out, bottom + out)].map(|corner| through(&style.table, corner));
        let low = corners.iter().fold(pt(f32::MAX, f32::MAX), |low, p| pt(low.x.min(p.x), low.y.min(p.y)));
        let high = corners.iter().fold(pt(f32::MIN, f32::MIN), |high, p| pt(high.x.max(p.x), high.y.max(p.y)));
        ((high.x - low.x) * (high.y - low.y) * style.share[0]).min(style.share[1])
    }

    /// Keeps an outline: which kept one.
    fn kept_path(&mut self, outline: Path) -> u32 {
        self.parsed.paths.push(outline);
        self.parsed.paths.len() as u32 - 1
    }

    /// Which kept pen `style` draws with: the last kept, if it is that one.
    fn pen(&mut self, style: &Style) -> u32 {
        if self.parsed.pens.last() != Some(&style.pen) {
            self.parsed.pens.push(style.pen);
        }
        self.parsed.pens.len() as u32 - 1
    }

    /// Begins the pattern tiles a shape is filled and stroked with, if any: gives back which
    /// tiles (fill, stroke) and walks through their content, the first to draw last. (A pattern
    /// already being drawn, or invisible, has none.)
    fn tiling(&mut self, at: usize, name: &str, style: &Style, walks: &[Walk]) -> Option<([Option<u32>; 2], Vec<Walk>)> {
        let free = |pattern: &usize| walks.len() + 3 < DEEP && walks.iter().all(|walk| walk.used != Some(*pattern));
        let seen = [style.solid[0], style.solid[1] * positive(style.pen.width) as u8 as f32].map(|solid| positive(solid * style.faded * style.opacity) && !style.hidden);
        let patterns: [Option<usize>; 2] = std::array::from_fn(|n| match style.paints[n] {
            Paint::Tiles(pattern) if seen[n] && !style.clipping => Some(pattern).filter(free),
            _ => None,
        });
        let path = self.outline(at, name).filter(|_| patterns != [None; 2])?;
        let bounds = bounds(&self.parsed.paths[path as usize]);
        let tiles = patterns.map(|pattern| pattern.and_then(|pattern| self.tile(pattern, bounds, style)));
        // The first tile begins at once, the other once the first is drawn.
        let numbers = tiles.map(|tile| tile.map(|(number, _)| number));
        let mut drawings: Vec<Walk> = tiles.iter().flatten().map(|(_, walk)| *walk).collect();
        let mut begun = numbers.iter().flatten().copied();
        self.parsed.items.extend(begun.next().map(Item::Tile));
        if let Some(first) = drawings.first_mut() {
            first.then = begun.next();
        }
        drawings.reverse();
        Some((numbers, drawings))
    }

    /// Keeps a pattern tile for a shape styled `style` with bounding box `bounds` (its units):
    /// which kept tile, and a walk through its content. Nothing for a pattern of no size or
    /// content.
    fn tile(&mut self, at: usize, [left, top, across, down]: [f32; 4], style: &Style) -> Option<(u32, Walk)> {
        // The element and the patterns it inherits from (`href`) for what it does not say.
        let mut line = vec![at];
        while let Some(after) = self.file.linked(line[line.len() - 1]).filter(|after| line.len() < 8 && !line.contains(after) && self.file.elements[*after].name == "pattern") {
            line.push(after);
        }
        let attr = |name: &str| line.iter().find_map(|at| self.file.attribute(*at, name));
        let [.., wide, high] = self.parsed.view;
        // Tile position and size: in bounding-box fractions, or units.
        let side = |name: &str, whole: f32| attr(name).and_then(|text| length(text, whole)).unwrap_or(0.0);
        let [x, y, w, h] = match attr("patternUnits") != Some("userSpaceOnUse") {
            true => [left + side("x", 1.0) * across, top + side("y", 1.0) * down, side("width", 1.0) * across, side("height", 1.0) * down],
            false => [side("x", wide), side("y", high), side("width", wide), side("height", high)],
        };
        let content = line.iter().copied().find(|at| self.file.elements[*at].end > at + 1).filter(|_| positive(w) && positive(h))?;
        // Its content is in units from its origin, bounding-box fractions, or per its
        // `viewBox`.
        let view: Vec<f32> = attr("viewBox").map(|value| numbers(value).take(5).collect()).unwrap_or_default();
        let inner = match view[..] {
            [left, top, across, down] if across > 0.0 && down > 0.0 => Aspect::read(attr("preserveAspectRatio").unwrap_or("")).fit([left, top, across, down], [w, h]),
            _ if attr("patternContentUnits") == Some("objectBoundingBox") => [across, 0.0, 0.0, down, 0.0, 0.0],
            _ => SAME,
        };
        let table = both(&style.table, &both(&attr("patternTransform").map_or(SAME, turned), &[1.0, 0.0, 0.0, 1.0, x, y]));
        // Its block is as many pixels as it shows (`Tile::block`): this share of the picture.
        // Its content is in tile units, so many per block.
        let block = table[0].hypot(table[1]) * table[2].hypot(table[3]) * w * h * style.share[0];
        self.spend(block);
        self.parsed.tiles.push(Tile { table, size: [w, h], inner });
        let share = [block * (inner[0] * inner[3] - inner[1] * inner[2]).abs() / (w * h), block];
        let style = Style { table: SAME, share, ..self.styled(at, &Style::default()) };
        Some((self.parsed.tiles.len() as u32 - 1, Walk { used: Some(at), ..Walk::through(content + 1, self.file.elements[content].end, style) }))
    }

    /// The walk through a shape's markers, outline `path`, styled `style`: at its start, each
    /// vertex and its end, each placed and oriented along the outline. Nothing where it has
    /// none.
    fn markers(&mut self, path: u32, style: &Style) -> Option<Walk> {
        let (corners, from) = (corners(&self.parsed.paths[path as usize]), self.corners.len());
        let last = corners.len().saturating_sub(1);
        for (n, (point, way)) in corners.into_iter().enumerate() {
            let which = match n {
                0 => 0,
                _ if n == last => 2,
                _ => 1,
            };
            self.corners.extend(style.markers[which].map(|marker| (marker, point, way, n == 0)));
        }
        (self.corners.len() > from).then(|| Walk { marking: true, ..Walk::through(from, self.corners.len(), *style) })
    }

    /// A walk through a marker's content, placed at `point` of a shape styled `style`, rotated
    /// to `way` (an angle) if it says so, and reversed at the outline's start (`first`) if it
    /// says that too.
    fn marker(&mut self, at: usize, (point, way, first): (Point, f32, bool), style: &Style) -> Walk {
        let attr = |name: &str| self.file.attribute(at, name);
        let angle = match attr("orient").map(str::trim) {
            Some("auto") => way,
            Some("auto-start-reverse") => way + std::f32::consts::PI * first as u8 as f32,
            other => other.and_then(number).map_or(0.0, |(degrees, _)| degrees.to_radians()),
        };
        // It is scaled by the stroke width unless in user units; its `viewBox` is fitted to its
        // box, and its reference point put on the vertex.
        let scale = if attr("markerUnits") == Some("userSpaceOnUse") { 1.0 } else { style.pen.width };
        let unit = |name: &str, fallback: f32| attr(name).and_then(|text| length(text, 1.0)).unwrap_or(fallback);
        let view: Vec<f32> = attr("viewBox").map(|value| numbers(value).take(5).collect()).unwrap_or_default();
        let fit = match view[..] {
            [left, top, across, down] if across > 0.0 && down > 0.0 => {
                Aspect::read(attr("preserveAspectRatio").unwrap_or("")).fit([left, top, across, down], [unit("markerWidth", 3.0), unit("markerHeight", 3.0)])
            }
            _ => SAME,
        };
        let reference = through(&fit, pt(unit("refX", 0.0), unit("refY", 0.0)));
        let (sin, cos) = angle.sin_cos();
        let placed = both(&[cos * scale, sin * scale, -sin * scale, cos * scale, point.x, point.y], &both(&[1.0, 0.0, 0.0, 1.0, -reference.x, -reference.y], &fit));
        let marked = self.styled(at, &Style { table: both(&style.table, &placed), boxed: style.boxed, share: style.share, ..Style::default() });
        Walk { used: Some(at), ..Walk::through(at + 1, self.file.elements[at].end, marked) }
    }

    /// An element's shape outline: which kept one. Computed once, however often the element is
    /// used. Nothing for a non-shape.
    fn outline(&mut self, at: usize, name: &str) -> Option<u32> {
        if let Some(kept) = self.outlined.get(&at) {
            return Some(*kept);
        }
        let [.., wide, high] = self.parsed.view;
        let across = ((wide * wide + high * high) / 2.0).sqrt();
        let length_of = |name: &str, whole: f32| self.file.attribute(at, name).and_then(|text| length(text, whole));
        let (x, y) = (|name: &str| length_of(name, wide).unwrap_or(0.0), |name: &str| length_of(name, high).unwrap_or(0.0));
        let points = self.file.attribute(at, "points").unwrap_or("");
        let path = match name {
            "path" => Path::svg(self.file.attribute(at, "d").unwrap_or("")),
            "rect" => rect([x("x"), y("y"), x("width"), y("height")], [length_of("rx", wide), length_of("ry", high)]),
            "circle" => ellipse([x("cx"), y("cy")], [length_of("r", across).unwrap_or(0.0); 2]),
            "ellipse" => ellipse([x("cx"), y("cy")], [x("rx"), y("ry")]),
            "line" => Path::svg(&format!("M{} {}L{} {}", x("x1"), y("y1"), x("x2"), y("y2"))),
            // A polyline is written as path points.
            "polyline" => Path::svg(&format!("M{points}")),
            "polygon" => Path::svg(&format!("M{points}z")),
            _ => return None,
        };
        self.parsed.paths.push(path);
        self.outlined.insert(at, self.parsed.paths.len() as u32 - 1);
        Some(self.parsed.paths.len() as u32 - 1)
    }

    /// A shape's final fill or stroke: `paint` as written, at opacity `solid`, `current_color`
    /// being what the file makes of `currentColor`, `path` the outline, `tile` its pattern
    /// tile.
    fn ink(&mut self, paint: Paint, solid: f32, current_color: Option<Color>, path: u32, tile: Option<u32>) -> Ink {
        match (paint, current_color) {
            _ if !positive(solid) => Ink::None,
            (Paint::None, _) => Ink::None,
            (Paint::Own(color), _) | (Paint::CurrentColor, Some(color)) => Ink::Own(color.with_alpha(solid)),
            (Paint::CurrentColor, None) => {
                self.parsed.uses_current_color = true;
                Ink::CurrentColor(solid)
            }
            (Paint::Gradient(at), _) => self.gradient(at, solid, path),
            (Paint::Tiles(_), _) => tile.map_or(Ink::None, |tile| Ink::Tiles(tile, solid)),
        }
    }

    /// An element's gradient as it fills the shape with outline `path`.
    fn gradient(&mut self, at: usize, solid: f32, path: u32) -> Ink {
        let read = match self.gradients_read.get(&at) {
            Some(read) => *read,
            None => self.read_blend(at),
        };
        self.gradients_read.insert(at, read);
        let Some(mut blend) = read else { return Ink::None };
        let last = self.parsed.stops[blend.stops[1] as usize - 1].1;
        // A zero-length gradient, and a single stop, give the last colour everywhere.
        let flat = match blend.way {
            Way::Line([x1, y1, x2, y2]) => (x1, y1) == (x2, y2),
            Way::Round([_, _, r, _, _]) => !positive(r),
        };
        if flat || blend.stops[1] - blend.stops[0] == 1 {
            return Ink::Own(last.with_alpha(solid));
        }
        // In bounding-box units, the geometry follows the shape's box; a shape with no width or
        // height has none.
        let [left, top, wide, high] = bounds(&self.parsed.paths[path as usize]);
        if blend.boxed && !(wide > 0.0 && high > 0.0) {
            return Ink::None;
        }
        if blend.boxed {
            blend.table = both(&[wide, 0.0, 0.0, high, left, top], &blend.table);
        }
        self.parsed.gradients.push(blend);
        Ink::Gradient(self.parsed.gradients.len() as u32 - 1, solid)
    }

    /// Reads a gradient from its element and those it inherits from (`href`) for what it does
    /// not say. Nothing without stops.
    fn read_blend(&mut self, at: usize) -> Option<Gradient> {
        // The element and those it inherits from, nearest first.
        let mut line = vec![at];
        while let Some(after) = self.file.linked(line[line.len() - 1]).filter(|after| line.len() < 8 && !line.contains(after)) {
            line.push(after);
        }
        let attr = |name: &str| line.iter().find_map(|at| self.file.attribute(*at, name));
        let [.., wide, high] = self.parsed.view;
        let boxed = attr("gradientUnits") != Some("userSpaceOnUse");
        let [wide, high, across] = if boxed { [1.0; 3] } else { [wide, high, ((wide * wide + high * high) / 2.0).sqrt()] };
        // Unstated values are these fractions of the whole.
        let lies = |name: &str, whole: f32, fallback: f32| attr(name).and_then(|text| length(text, whole)).unwrap_or(fallback * whole);
        let way = if self.file.elements[at].name == "radialGradient" {
            let (cx, cy, r) = (lies("cx", wide, 0.5), lies("cy", high, 0.5), lies("r", across, 0.5));
            // The focus: the centre unless given. One outside the circle is moved onto it (just
            // inside: on it, nothing could grow).
            let (fx, fy) = (lies("fx", wide, cx / wide) - cx, lies("fy", high, cy / high) - cy);
            let out = (fx.hypot(fy) / (r * 0.99)).max(1.0);
            Way::Round([cx, cy, r, cx + fx / out, cy + fy / out])
        } else {
            Way::Line([lies("x1", wide, 0.0), lies("y1", high, 0.0), lies("x2", wide, 1.0), lies("y2", high, 0.0)])
        };
        let spread = match attr("spreadMethod") {
            Some("reflect") => Spread::Reflect,
            Some("repeat") => Spread::Repeat,
            _ => Spread::Pad,
        };
        let (table, first) = (attr("gradientTransform").map_or(SAME, turned), self.parsed.stops.len());
        // Its stops are its own, or those of the nearest it inherits from that has any.
        for from in line {
            self.stops(from);
            if self.parsed.stops.len() > first {
                break;
            }
        }
        (self.parsed.stops.len() > first).then_some(Gradient { stops: [first as u32, self.parsed.stops.len() as u32], way, spread, table, boxed })
    }

    /// Keeps a gradient element's stops, each with its offset, never before the previous one.
    fn stops(&mut self, blend: usize) {
        let (mut at, end, first) = (blend + 1, self.file.elements[blend].end, self.parsed.stops.len());
        while at < end {
            let style = self.styled(at, &Style::default());
            let color = match style.stop.0 {
                Paint::Own(color) => color,
                _ => style.ink.unwrap_or(Color::BLACK),
            };
            let along = self.file.attribute(at, "offset").and_then(share).unwrap_or(0.0);
            let least = self.parsed.stops[first..].last().map_or(0.0, |before| before.0);
            if self.file.elements[at].name == "stop" {
                self.parsed.stops.push((along.max(least).min(1.0), color.with_alpha(style.stop.1)));
            }
            at = self.file.elements[at].end;
        }
    }
}

/// The calls a filter list writes, each `name(arguments)`, separated by spaces or commas.
/// Nothing for a list not written so.
fn calls(list: &str) -> Option<Vec<(&str, &str)>> {
    let (mut calls, mut rest) = (Vec::new(), list.trim());
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = open + closing(&rest[open..])?;
        let name = rest[..open].trim();
        (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(())?;
        calls.push((name, &rest[open + 1..close]));
        rest = rest[close + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    Some(calls)
}

/// Where the bracket a text starts with closes, counting nested ones. Nothing where it does
/// not.
fn closing(text: &str) -> Option<usize> {
    let mut deep = 0;
    for (at, c) in text.char_indices() {
        deep += (c == '(') as i32 - (c == ')') as i32;
        if deep == 0 {
            return Some(at);
        }
    }
    None
}

/// The filter a CSS filter function writes (`blur(2px)`, `drop-shadow(…)`, `grayscale(50%)` and
/// the others of Filter Effects), applied to what `walk` drew: one step over the whole drawing.
/// Nothing for an unknown function.
fn function(name: &str, inside: &str, walk: &Walk) -> Option<Filter> {
    let inside = inside.trim();
    let amount = |fallback: f32| if inside.is_empty() { Some(fallback) } else { share(inside).filter(|amount| *amount >= 0.0) };
    let colors = |function: Function| Box::new([function.clone(), function.clone(), function, Function::Same]);
    let kind = match name {
        "blur" => Kind::Blur([if inside.is_empty() { 0.0 } else { length(inside, 0.0)? }.max(0.0); 2]),
        "drop-shadow" => shadow(inside, walk.style.ink.unwrap_or(Color::BLACK))?,
        "grayscale" => Kind::Matrix(svg_filters::toned(amount(1.0)?, false)),
        "sepia" => Kind::Matrix(svg_filters::toned(amount(1.0)?, true)),
        "saturate" => Kind::Matrix(svg_filters::saturated(amount(1.0)?)),
        "hue-rotate" => Kind::Matrix(svg_filters::hue_turned(if inside.is_empty() { 0.0 } else { angle(inside)? })),
        "invert" => Kind::Transfer(colors(Function::Table(vec![amount(1.0)?.min(1.0), 1.0 - amount(1.0)?.min(1.0)]))),
        "opacity" => Kind::Transfer(Box::new([Function::Same, Function::Same, Function::Same, Function::Table(vec![0.0, amount(1.0)?.min(1.0)])])),
        "brightness" => Kind::Transfer(colors(Function::Linear(amount(1.0)?, 0.0))),
        "contrast" => Kind::Transfer(colors(Function::Linear(amount(1.0)?, 0.5 - 0.5 * amount(1.0)?))),
        _ => return None,
    };
    let steps = vec![svg_filters::Step { kind, inputs: [Input::Source; 2], place: [None; 4], space: walk.style.space }];
    Some(Filter { table: walk.units, region: [-1e9, -1e9, 2e9, 2e9], steps, images: 0 })
}

/// What `drop-shadow()` says: a colour (or the surrounding colour), the x and y offsets, and
/// the blur (the deviation, as librsvg reads it).
fn shadow(inside: &str, around: Color) -> Option<Kind> {
    let (mut lengths, mut ink) = (Vec::new(), None);
    let mut deep = 0;
    // Space-separated words, except inside brackets: `rgb(0, 0, 0)` is one.
    let words = inside.split(|c: char| {
        deep += (c == '(') as i32 - (c == ')') as i32;
        c.is_whitespace() && deep == 0
    });
    for word in words.filter(|word| !word.is_empty()) {
        match number(word).filter(|(_, unit)| unit.chars().all(|c| c.is_ascii_alphabetic())) {
            Some(_) => lengths.push(length(word, 0.0)?),
            None => ink = Some(if word.eq_ignore_ascii_case("currentcolor") { around } else { color(&word.to_ascii_lowercase())? }),
        }
    }
    let ([dx, dy], blur) = (*lengths.first_chunk::<2>()?, lengths.get(2).copied().unwrap_or(0.0).max(0.0));
    (lengths.len() <= 3).then_some(Kind::Shadow([dx, dy], [blur; 2], ink.unwrap_or(around)))
}

/// An angle as written, in degrees: from degrees, turns, radians or grads.
fn angle(text: &str) -> Option<f32> {
    let (number, unit) = number(text)?;
    let times = match unit.trim() {
        "" | "deg" => 1.0,
        "turn" => 360.0,
        "rad" => 180.0 / std::f32::consts::PI,
        "grad" => 0.9,
        _ => return None,
    };
    Some(number * times)
}

/// A filter step's input per its `in` (none where unstated), after `made` steps named by
/// `read.results`: by default, or where it names what nothing made, the previous step's result
/// (`SourceGraphic` for the first). A paint input is one of the filter's images (see
/// `Primitives::paint`).
fn input_of(named: Option<&str>, read: &mut Primitives, made: usize) -> Input {
    let fallback = if made > 0 { Input::Made(made - 1) } else { Input::Source };
    match named {
        None => fallback,
        Some("SourceGraphic") => Input::Source,
        Some("SourceAlpha") => Input::Alpha,
        Some("BackgroundImage") => Input::Background,
        Some("BackgroundAlpha") => Input::BackgroundAlpha,
        Some("FillPaint") => Input::Image(read.paint(0)),
        Some("StrokePaint") => Input::Image(read.paint(1)),
        Some(name) => read.results.iter().rev().find(|(result, _)| *result == name).map_or(fallback, |(_, at)| Input::Made(*at)),
    }
}

/// A step's `kernelUnitLength`: one or two positive numbers (across, down), in its units, or in
/// bounding-box fractions where `scale` is the box's width and height. Nothing where it gives
/// none (librsvg ignores the attribute then too).
fn kernel_unit(text: Option<&str>, scale: [f32; 2]) -> Option<[f32; 2]> {
    let stated: Vec<f32> = numbers(text?).take(3).collect();
    let [x, y] = match stated[..] {
        [x] => [x, x],
        [x, y] => [x, y],
        _ => return None,
    };
    (x > 0.0 && y > 0.0).then_some([x * scale[0], y * scale[1]])
}

/// A composite step's operator, with its four `arithmetic` coefficients. (Over, for an unknown
/// one.)
fn operator(name: &str, k: [f32; 4]) -> Operator {
    match name {
        "in" => Operator::In,
        "out" => Operator::Out,
        "atop" => Operator::Atop,
        "xor" => Operator::Xor,
        "lighter" => Operator::Lighter,
        "arithmetic" => Operator::Arithmetic(k),
        _ => Operator::Over,
    }
}

/// A colour matrix step's matrix by type and values. (Identity for a `matrix` without twenty
/// values; nothing for an unknown type.)
fn matrix(kind: &str, values: Option<&str>) -> Option<[f32; 20]> {
    let values: Vec<f32> = values.map(|text| numbers(text).collect()).unwrap_or_default();
    let first = values.first().copied();
    match kind {
        "matrix" => Some(values.as_slice().try_into().unwrap_or(svg_filters::SAME)),
        "saturate" => Some(svg_filters::saturated(first.unwrap_or(1.0).max(0.0))),
        "hueRotate" => Some(svg_filters::hue_turned(first.unwrap_or(0.0))),
        "luminanceToAlpha" => Some(svg_filters::LUMINANCE),
        _ => None,
    }
}

/// A rectangle as written: left, top, width, height, and corner radii across and down.
fn rect([x, y, wide, high]: [f32; 4], round: [Option<f32>; 2]) -> Path {
    if !(wide > 0.0 && high > 0.0) {
        return Path::new();
    }
    // A missing radius takes the other's; neither exceeds half a side.
    let (rx, ry) = (round[0].or(round[1]).unwrap_or(0.0).max(0.0).min(wide / 2.0), round[1].or(round[0]).unwrap_or(0.0).max(0.0).min(high / 2.0));
    let (right, bottom) = (x + wide, y + high);
    if rx == 0.0 || ry == 0.0 {
        return Path::svg(&format!("M{x} {y}H{right}V{bottom}H{x}z"));
    }
    let corner = format!("A{rx} {ry} 0 0 1");
    Path::svg(&format!(
        "M{} {y}H{}{corner} {right} {}V{}{corner} {} {bottom}H{}{corner} {x} {}V{}{corner} {} {y}z",
        x + rx,
        right - rx,
        y + ry,
        bottom - ry,
        right - rx,
        x + rx,
        bottom - ry,
        y + ry,
        x + rx
    ))
}

/// An ellipse around a centre with radii across and down: as two halves, from its rightmost
/// point round by its lowest.
fn ellipse([x, y]: [f32; 2], [rx, ry]: [f32; 2]) -> Path {
    if !(rx > 0.0 && ry > 0.0) {
        return Path::new();
    }
    Path::svg(&format!("M{} {y}A{rx} {ry} 0 1 1 {} {y}A{rx} {ry} 0 1 1 {} {y}z", x + rx, x - rx, x + rx))
}

/// How far an outline reaches through a transform, with a stroke `stroke` wide (its units): left,
/// top, right, bottom. (Past everything, for an empty outline.)
fn reach_of(path: &Path, table: &Table, stroke: f32) -> [f32; 4] {
    let [left, top, wide, high] = bounds(path);
    let out = stroke / 2.0;
    let corners = [pt(left - out, top - out), pt(left + wide + out, top - out), pt(left - out, top + high + out), pt(left + wide + out, top + high + out)];
    let corners = corners.map(|corner| through(table, corner));
    let least = corners.iter().fold([f32::MAX; 2], |least, p| [least[0].min(p.x), least[1].min(p.y)]);
    let most = corners.iter().fold([f32::MIN; 2], |most, p| [most[0].max(p.x), most[1].max(p.y)]);
    [least[0], least[1], most[0], most[1]]
}

/// Whether a point lies inside an outline, by the even-odd rule or the nonzero one (an open
/// subpath closed by a line, as filling closes it).
fn inside(path: &Path, p: Point, even_odd: bool) -> bool {
    let Rect { w, h, .. } = path.bounds();
    let mut round = 0;
    path.flatten_closed(1.0, pt(0.0, 0.0), (w.max(h) / 1024.0).max(1e-6), |a, b| {
        if (a.y <= p.y) != (b.y <= p.y) && a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) > p.x {
            round += if b.y > a.y { 1 } else { -1 };
        }
    });
    if even_odd { round % 2 != 0 } else { round != 0 }
}

/// A shape's bounding box: left, top, width, height. Of a curve, how far it really reaches, not
/// its control points. (For a shape with no segments, a box starting past everything, with
/// negative size.)
fn bounds(path: &Path) -> [f32; 4] {
    let lined = path.steps().iter().any(|step| !matches!(step, Step::Move(_) | Step::Close));
    let Rect { x, y, w, h } = path.bounds();
    if lined { [x, y, w, h] } else { [f32::MAX, f32::MAX, f32::NEG_INFINITY, f32::NEG_INFINITY] }
}

/// The most shapes and outline segments a file of `long` bytes may keep: as many as it could
/// write out (each takes a few bytes at least), but never so few that a drawing of any size is
/// cut short.
fn most(long: usize) -> [usize; 2] {
    [(long / 8).max(1 << 16), (long / 2).max(1 << 16)]
}

/// The most full-picture passes drawing a file of `long` bytes may cost: see `PASSES`.
fn passes(long: usize) -> f32 {
    (long as f32 / BYTES).max(PASSES)
}

/// The bounding box of an outline's points, control points included (so it holds the whole
/// outline): left, top, right, bottom. Nothing for an outline with no points.
fn reach(path: &Path) -> Option<[f32; 4]> {
    let points = path.steps().iter().flat_map(|step| match *step {
        Step::Move(p) | Step::Line(p) => [Some(p), None, None],
        Step::Quad(c, p) => [Some(c), Some(p), None],
        Step::Cubic(c, d, p) => [Some(c), Some(d), Some(p)],
        Step::Close => [None; 3],
    });
    points.flatten().fold(None, |reach, p| {
        let [left, top, right, bottom] = reach.unwrap_or([p.x, p.y, p.x, p.y]);
        Some([left.min(p.x), top.min(p.y), right.max(p.x), bottom.max(p.y)])
    })
}

/// The longest step of an outline, along its control polygon (longer than the curves).
fn reach_along(steps: &[Step]) -> f32 {
    let (mut at, mut start, mut long) = (pt(0.0, 0.0), pt(0.0, 0.0), 0.0);
    let mut to = |p: Point, at: &mut Point| {
        long += (p.x - at.x).hypot(p.y - at.y);
        *at = p;
    };
    for step in steps {
        match *step {
            Step::Move(p) => (at, start) = (p, p),
            Step::Line(p) => to(p, &mut at),
            Step::Quad(c, p) => [c, p].into_iter().for_each(|p| to(p, &mut at)),
            Step::Cubic(c, d, p) => [c, d, p].into_iter().for_each(|p| to(p, &mut at)),
            Step::Close => to(start, &mut at),
        }
    }
    long
}

/// An outline's marker vertices in order, each with the path direction there as an angle: the
/// bisector of the incoming and outgoing directions, and at an end the one direction it has. (A
/// closed outline comes into its start and leaves from its end.)
fn corners(path: &Path) -> Vec<(Point, f32)> {
    // Each vertex with its incoming and outgoing directions, where present.
    let mut corners: Vec<(Point, Option<Point>, Option<Point>)> = Vec::new();
    let (mut at, mut start, mut first) = (pt(0.0, 0.0), pt(0.0, 0.0), 0);
    for step in path.steps() {
        let (to, out, back) = match *step {
            Step::Move(p) => {
                (at, start, first) = (p, p, corners.len());
                corners.push((p, None, None));
                continue;
            }
            Step::Line(b) => (b, way(at, &[b]), way(b, &[at])),
            Step::Quad(c, b) => (b, way(at, &[c, b]), way(b, &[c, at])),
            Step::Cubic(c, d, b) => (b, way(at, &[c, d, b]), way(b, &[d, c, at])),
            Step::Close => (start, way(at, &[start]), way(start, &[at])),
        };
        let (closing, into) = (matches!(step, Step::Close), back.map(|back| pt(-back.x, -back.y)));
        if let Some(last) = corners.last_mut() {
            last.2 = out;
        }
        if !closing || out.is_some() {
            corners.push((to, into, None));
        }
        // Where it closes, its last vertex leaves as the first does, and the first is entered
        // as the last is.
        if let (true, Some(&(_, into, _))) = (closing, corners.last()) {
            let out = corners.get(first).and_then(|corner| corner.2);
            corners.last_mut().into_iter().for_each(|last| last.2 = out);
            corners.get_mut(first).into_iter().for_each(|corner| corner.1 = into);
        }
        at = to;
    }
    corners.into_iter().map(|(point, into, out)| (point, halfway(into, out))).collect()
}

/// A letter of piece `piece` as given: its position from the piece or the nearest ancestor
/// piece giving one for that letter index. Every piece it is in counts one more letter.
fn given(letter: char, piece: usize, pieces: &mut [Piece]) -> Given {
    let (mut at, mut inside) = ([None; 4], Some(piece));
    while let Some(within) = inside {
        let own = &mut pieces[within];
        at = std::array::from_fn(|n| at[n].or(own.places[n].get(own.letters).copied()));
        own.letters += 1;
        inside = (within != 0).then_some(own.around);
    }
    Given { letter, piece, at }
}

/// The `<textPath>` piece a piece lies in (or is), if any.
fn on_path(pieces: &[Piece], piece: usize) -> Option<usize> {
    let mut inside = piece;
    while pieces[inside].path.is_none() && inside != 0 {
        inside = pieces[inside].around;
    }
    pieces[inside].path.as_ref().map(|_| inside)
}

/// What shaping a chunk keeps: the line so far; each font's plan per script and direction; the
/// last run's glyphs; and which letters were moved already (see `Reading::shape_run`).
#[cfg(feature = "shaping")]
struct Shaper {
    line: Line,
    plans: Vec<(usize, crate::shaping::Script, bool, crate::shaping::Plan)>,
    laid: Vec<crate::shaping::Laid>,
    moved: Vec<bool>,
}

/// The runs a chunk's letters from `first` on are shaped in (see `Reading::line`), in order:
/// each of one piece, font and script (figures, signs and marks going with the letters around
/// them), and no longer than is shaped at once.
#[cfg(feature = "shaping")]
fn runs(letters: &[Given], fonts: &[Option<usize>], first: usize) -> Vec<std::ops::Range<usize>> {
    let script = |at: usize| crate::shaping::Script::of(letters[at].letter);
    let mut runs: Vec<(std::ops::Range<usize>, Option<crate::shaping::Script>)> = Vec::new();
    for at in first..letters.len() {
        let same = |(run, own): &(std::ops::Range<usize>, Option<crate::shaping::Script>)| {
            let script = script(at);
            (letters[run.start].piece, fonts[run.start]) == (letters[at].piece, fonts[at]) && (script.is_none() || own.is_none() || script == *own) && run.len() < crate::shaping::LONGEST - 64
        };
        match runs.last_mut().filter(|run| same(run)) {
            Some((run, own)) => (run.end, *own) = (at + 1, own.or(script(at))),
            None => runs.push((at..at + 1, script(at))),
        }
    }
    runs.into_iter().map(|(run, _)| run).collect()
}

/// Character data as written, with `&name;` and `&#number;` references decoded.
fn characters(words: &str) -> String {
    let (mut read, mut rest) = (String::with_capacity(words.len()), words);
    while let Some(at) = rest.find('&') {
        read.push_str(&rest[..at]);
        rest = &rest[at..];
        let named = rest.find(';').filter(|end| *end < 12).and_then(|end| Some((end, named(&rest[1..end])?)));
        let (long, letter) = named.map_or((1, '&'), |(end, letter)| (end + 1, letter));
        read.push(letter);
        rest = &rest[long..];
    }
    read + rest
}

/// The character a reference between `&` and `;` stands for: one of the five named, or a
/// number.
fn named(name: &str) -> Option<char> {
    let letter = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = name.strip_prefix('#')?;
            let number = number.strip_prefix(['x', 'X']).map_or_else(|| number.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())?;
            char::from_u32(number)?
        }
    };
    Some(letter)
}

/// The direction from `from` to the first of `towards` not at the same point; none if none is.
fn way(from: Point, towards: &[Point]) -> Option<Point> {
    towards.iter().find(|p| **p != from).map(|p| pt(p.x - from.x, p.y - from.y))
}

/// The bisector angle of two directions (each where present).
fn halfway(into: Option<Point>, out: Option<Point>) -> f32 {
    let unit = |way: Point| pt(way.x / way.x.hypot(way.y), way.y / way.x.hypot(way.y));
    let way = match (into.map(unit), out.map(unit)) {
        (Some(a), Some(b)) if (a.x + b.x).hypot(a.y + b.y) > 1e-6 => pt(a.x + b.x, a.y + b.y),
        (Some(a), _) | (None, Some(a)) => a,
        (None, None) => pt(1.0, 0.0),
    };
    way.y.atan2(way.x)
}

/// A font size as written: a length, `em` or percentage relative to the inherited size, or a
/// keyword from `xx-small` to `xx-large`.
fn font_size(text: &str, around: f32) -> Option<f32> {
    let words = ["xx-small", "x-small", "small", "medium", "large", "x-large", "xx-large"];
    if let Some(step) = words.iter().position(|word| *word == text) {
        return Some([9.0, 10.0, 13.0, 16.0, 18.0, 24.0, 32.0][step]);
    }
    let (size, unit) = number(text)?;
    match unit.trim() {
        "em" => Some(size * around),
        "%" => Some(size * around / 100.0),
        "ex" => Some(size * around / 2.0),
        _ => length(text, around),
    }
}

/// A font weight as written: 1 to 1000, or a keyword (two of them relative to the inherited
/// weight).
fn weight(text: &str, around: u16) -> u16 {
    match text {
        "normal" => 400,
        "bold" => 700,
        "bolder" => [400, 700, 900][(around >= 400) as usize + (around >= 600) as usize],
        "lighter" => [100, 400][(around >= 800) as usize].min(around),
        // Rounded to the nearest hundred, as system fonts come: each weight a drawing names is
        // a face the program keeps while it runs.
        _ => number(text).map_or(around, |(weight, _)| ((weight / 100.0).round() * 100.0).clamp(100.0, 900.0) as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A weight is rounded to the nearest hundred, as system fonts come: a drawing naming a
    /// thousand weights makes no more than nine faces of a family.
    #[test]
    fn weights_are_taken_to_the_nearest_hundred() {
        let weights = ["350", "351", "1", "1000", "649.9", "bold", "lighter", "x"].map(|text| weight(text, 300));
        assert_eq!(weights, [400, 400, 100, 900, 600, 700, 100, 300]);
    }

    /// A drawing of `view` by `view` units, drawn `side` by `side` pixels: its pixels.
    fn drawn(view: usize, side: usize, shapes: &str) -> Vec<u32> {
        decode(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}">{shapes}</svg>"#), side, side).unwrap().1
    }
    /// How many pixels are exactly this colour.
    fn count(pixels: &[u32], color: u32) -> usize {
        pixels.iter().filter(|pixel| **pixel == color).count()
    }
    /// Total coverage: every pixel by its alpha, 1 for full.
    fn covered(pixels: &[u32]) -> f32 {
        pixels.iter().map(|pixel| (pixel >> 24) as f32 / 255.0).sum()
    }
    const RED: u32 = 0xffff_0000;

    #[test]
    fn numbers_lengths_and_colours_are_read_as_files_write_them() {
        assert_eq!(
            [number("12"), number("-.5e2px"), number(" ,3.25,4"), number("1.e"), number("2em")],
            [Some((12.0, "")), Some((-50.0, "px")), Some((3.25, ",4")), Some((1.0, "e")), Some((2.0, "em"))]
        );
        assert_eq!([number(""), number("-"), number("."), number("x1")], [None; 4]);
        // Numbers may run together: a sign or a second point starts the next.
        assert_eq!(numbers("1-2.5.5e1+3 4,5").collect::<Vec<f32>>(), [1.0, -2.5, 5.0, 3.0, 4.0, 5.0]);
        assert_eq!(
            [length("3", 80.0), length("25%", 80.0), length("1in", 80.0), length("2.54cm", 80.0), length("12pt", 80.0), length("1pc", 80.0)],
            [Some(3.0), Some(20.0), Some(96.0), Some(96.0), Some(16.0), Some(16.0)]
        );
        assert_eq!(length("auto", 80.0), None);
        // Colours: three and six digits, with alpha; numbers and percentages; names.
        let colours =
            ["#f80", "#ff8800", "#f808", "#ff880080", "rgb(255,136,0)", "rgb(100%, 53.4%, 0%)", "rgba(255 136 0 / 50%)", "rgba(255,136,0,.5)", "darkorange", "rebeccapurple", "#ff880", "nothing", ""];
        let read = [0xffff_8800, 0xffff_8800, 0x88ff_8800, 0x80ff_8800, 0xffff_8800, 0xffff_8800, 0x80ff_8800, 0x80ff_8800, 0xffff_8c00, 0xff66_3399].map(|color| Some(Color(color)));
        assert_eq!(colours.map(color), [&read[..], &[None; 3]].concat()[..]);
        // All 148 names, each a colour, alphabetical.
        let names: Vec<&str> = NAMES.split_ascii_whitespace().map(|pair| pair.split_once('=').unwrap().0).collect();
        assert!(names.len() == 148 && names.is_sorted() && names.iter().all(|name| color(name).is_some()));
        assert!([plain("None"), plain("currentColor"), plain(" Red ")] == [Some(Paint::None), Some(Paint::CurrentColor), Some(Paint::Own(Color(RED)))]);
    }

    #[test]
    fn what_transform_says_is_done_last_first() {
        let near = |p: Point, x: f32, y: f32| (p.x - x).abs() < 1e-4 && (p.y - y).abs() < 1e-4;
        // Moved after being doubled: (1, 1) goes to (2, 2), then (12, 22).
        assert!(near(through(&turned("translate(10 20) scale(2)"), pt(1.0, 1.0)), 12.0, 22.0));
        assert!(near(through(&turned("scale(2),translate(10,20)"), pt(1.0, 1.0)), 22.0, 42.0));
        // A quarter turn takes (1, 0) to (0, 1); around (1, 1) it takes (2, 1) to (1, 2).
        assert!(near(through(&turned("rotate(90)"), pt(1.0, 0.0)), 0.0, 1.0) && near(through(&turned("rotate(90 1 1)"), pt(2.0, 1.0)), 1.0, 2.0));
        assert!(near(through(&turned("matrix(1 2 3 4 5 6)"), pt(1.0, 1.0)), 9.0, 12.0) && near(through(&turned("skewX(45) skewY(45)"), pt(1.0, 1.0)), 3.0, 2.0));
        assert_eq!(
            [turned("translate(7)"), turned("scale(3)"), turned("matrix(1 2 3)"), turned("spin(4) translate(1 2")],
            [[1.0, 0.0, 0.0, 1.0, 7.0, 0.0], [3.0, 0.0, 0.0, 3.0, 0.0, 0.0], SAME, [1.0, 0.0, 0.0, 1.0, 1.0, 2.0]]
        );
        // A transform is undone by its inverse; a flattening one has none.
        let (table, p) = (turned("translate(3 4) rotate(30) scale(2 5) skewX(20)"), pt(7.0, -2.0));
        let back = through(&undone(&table).unwrap(), through(&table, p));
        assert!(near(back, p.x, p.y) && undone(&turned("scale(2 0)")).is_none());
    }

    #[test]
    fn a_file_is_taken_apart_whatever_stands_in_it() {
        let file = File::read(
            r#"<?xml version="1.0"?><!DOCTYPE svg [ <!ENTITY a "b > c"> ]><!-- <g> not this --><svg:svg a="1" b='x > y' hidden c = "3"><g id="one"><path d="M0 0"/><rect></g>
            <style><![CDATA[ .a, rect#two { fill: red } ]]></style><svg:rect id="two" class="b a"/><circle"#,
        );
        let names: Vec<&str> = file.elements.iter().map(|element| element.name).collect();
        assert_eq!(names, ["svg", "g", "path", "rect", "style", "rect", "circle"]);
        // Unclosed elements end with their parent, or the file.
        assert_eq!(file.elements.iter().map(|element| element.end).collect::<Vec<usize>>(), [7, 4, 3, 4, 5, 6, 7]);
        assert_eq!(
            [file.attribute(0, "a"), file.attribute(0, "b"), file.attribute(0, "c"), file.attribute(0, "hidden"), file.attribute(2, "d")],
            [Some("1"), Some("x > y"), Some("3"), None, Some("M0 0")]
        );
        assert_eq!((file.named.get("one"), file.named.get("two")), (Some(&1), Some(&5)));
        // The sheet's rules: by class, by name and id; not by negation.
        assert_eq!(file.rules.iter().map(|(whom, _)| *whom).collect::<Vec<&str>>(), [".a", "rect#two"]);
        assert_eq!(
            [file.is_for(".a", 5), file.is_for("rect#two", 5), file.is_for(".a", 3), file.is_for("rect#two", 3), file.is_for("g rect", 5), file.is_for("*", 2), file.is_for("", 2)],
            [true, true, false, false, false, true, false]
        );
    }

    #[test]
    fn shapes_fill_exactly_the_pixels_they_cover() {
        // A square on pixel boundaries is crisp at any size.
        for side in [8, 16, 96] {
            let pixels = drawn(8, side, r#"<rect x="2" y="1" width="4" height="5" fill="red"/>"#);
            assert_eq!((count(&pixels, RED), count(&pixels, 0)), (20 * side * side / 64, 44 * side * side / 64), "{side} pixels");
        }
        // The same square five other ways.
        let ways = [
            r#"<path d="M2 1h4v5h-4z" fill="red"/>"#,
            r#"<polygon points="2,1 6,1 6,6 2,6" fill="red"/>"#,
            r#"<polyline points="2 1 6 1 6 6 2 6" fill="red"/>"#,
            r#"<line x1="2" y1="3.5" x2="6" y2="3.5" stroke="red" stroke-width="5"/>"#,
            r##"<g transform="translate(2 1) scale(4 5)"><rect width="100%" height="12.5%" transform="scale(.125 1)" fill="#f00"/></g>"##,
        ];
        for way in ways {
            assert_eq!(drawn(8, 16, way), drawn(8, 16, r#"<rect x="2" y="1" width="4" height="5" fill="red"/>"#), "{way}");
        }
        // Round shapes cover their area: a circle, an ellipse, and a rounded rectangle (each
        // corner loses a square minus a quarter circle).
        let area = |shape: &str| covered(&drawn(40, 80, shape)) / 4.0;
        let round = std::f32::consts::PI;
        assert!((area(r#"<circle cx="20" cy="20" r="15"/>"#) - round * 225.0).abs() < 0.5);
        assert!((area(r#"<ellipse cx="20" cy="20" rx="15" ry="6"/>"#) - round * 90.0).abs() < 0.5);
        assert!((area(r#"<rect x="5" y="5" width="30" height="20" rx="4"/>"#) - (600.0 - (4.0 - round) * 16.0)).abs() < 0.5);
        assert!((area(r#"<rect x="5" y="5" width="30" height="20" rx="4" ry="40"/>"#) - (600.0 - (4.0 - round) * 40.0)).abs() < 0.5);
        // And what has no size is nothing.
        for nothing in [r#"<rect width="0" height="5"/>"#, r#"<circle r="0"/>"#, r#"<ellipse rx="4"/>"#, r#"<rect width="-3" height="5"/>"#, r#"<path/>"#, r#"<polygon points="1"/>"#] {
            assert_eq!(covered(&drawn(8, 8, nothing)), 0.0, "{nothing}");
        }
    }

    #[test]
    fn what_is_inside_a_shape_goes_by_its_rule() {
        // Three nested squares all drawn the same way round: by one rule the whole largest is
        // filled, by the other the middle one is a hole and the smallest not.
        let nested = |rule: &str| drawn(12, 12, &format!(r#"<path fill-rule="{rule}" fill="red" d="M0 0h12v12h-12z M2 2h8v8h-8z M4 4h4v4h-4z"/>"#));
        assert_eq!((count(&nested("nonzero"), RED), count(&nested("evenodd"), RED)), (144, 144 - 64 + 16));
        // A square drawn the other way round is a hole by either rule.
        let other_way = |rule: &str| drawn(12, 12, &format!(r#"<path fill-rule="{rule}" fill="red" d="M0 0h12v12h-12z M2 2v8h8v-8z"/>"#));
        assert_eq!((count(&other_way("nonzero"), RED), count(&other_way("evenodd"), RED)), (80, 80));
        // An open outline is filled as if closed.
        assert_eq!(count(&drawn(12, 12, r#"<path fill="red" d="M2 2h8v8"/>"#), RED), 28);
    }

    #[test]
    fn a_line_ends_and_turns_as_its_pen_says() {
        let line = |pen: &str, d: &str| drawn(20, 20, &format!(r#"<path fill="none" stroke="red" stroke-width="4" {pen} d="{d}"/>"#));
        // From 6 to 14 across, four thick: butt, square (half the width longer each end), or
        // round caps.
        let ends = |cap: &str| line(&format!(r#"stroke-linecap="{cap}""#), "M6 10h8");
        assert_eq!((count(&ends("butt"), RED), count(&ends("square"), RED)), (32, 48));
        assert!((covered(&ends("round")) - (32.0 + std::f32::consts::PI * 4.0)).abs() < 0.2);
        // A corner: miter, bevel, or round. (The miter adds a half-width square, bevel half of
        // that, round a quarter circle.)
        let corner = |join: &str| covered(&line(&format!(r#"stroke-linejoin="{join}""#), "M4 14h10v-10"));
        let legs = 2.0 * 10.0 * 4.0 - 4.0;
        assert!((corner("miter") - (legs + 4.0)).abs() < 0.05 && (corner("bevel") - (legs + 2.0)).abs() < 0.05, "{} {}", corner("miter"), corner("bevel"));
        assert!((corner("round") - (legs + std::f32::consts::PI)).abs() < 0.1);
        // A miter beyond the limit is beveled: a right angle's miter is 1.42 times the width.
        let limited = |limit: &str| covered(&line(&format!(r#"stroke-miterlimit="{limit}""#), "M4 14h10v-10"));
        assert!((limited("1.5") - (legs + 4.0)).abs() < 0.05 && (limited("1.4") - (legs + 2.0)).abs() < 0.05);
        // A closed outline has no ends, and a join where it closes.
        assert_eq!(count(&line("", "M6 6h8v8h-8z"), RED), 144 - 16);
        // A zero-length line is a dot with round caps, a square with square caps, nothing with
        // butt caps.
        let dot = |cap: &str| covered(&line(&format!(r#"stroke-linecap="{cap}""#), "M10 10h0"));
        assert!((dot("round") - std::f32::consts::PI * 4.0).abs() < 0.25 && dot("square") == 16.0 && dot("butt") == 0.0, "{:?}", [dot("round"), dot("square"), dot("butt")]);
        // A non-scaling stroke: a drawing 40 pixels wide of four units has it two pixels thick,
        // not twenty; shown ten times larger, twenty, not two hundred.
        let fixed = |side: usize, pen: &str| {
            count(&decode(&format!(r#"<svg width="40" height="40" viewBox="0 0 4 4"><path stroke="red" stroke-width="2" {pen} d="M0 2h4"/></svg>"#), side, side).unwrap().1, RED)
        };
        assert_eq!([fixed(40, ""), fixed(40, r#"vector-effect="non-scaling-stroke""#), fixed(400, r#"vector-effect="non-scaling-stroke""#)], [40 * 20, 40 * 2, 400 * 20]);
        // Zero width, no stroke; unstated, one unit.
        assert_eq!(covered(&drawn(20, 20, r#"<path stroke="red" stroke-width="0" d="M6 10h8"/>"#)), 0.0);
        assert_eq!(count(&drawn(20, 20, r#"<path stroke="red" d="M6 10.5h8"/>"#), RED), 8);
    }

    /// How much of each pixel lies within `half` of a stroke along `d`, the long way: counting
    /// which of 8 by 8 points per pixel do. (`side` pixels a side, for a 24-unit drawing.)
    fn counted(d: &str, side: usize, half: f32) -> Vec<f32> {
        let (scale, mut pieces) = (side as f32 / 24.0, Vec::new());
        Path::svg(d).flatten(scale * 16.0, pt(0.0, 0.0), 16.0, |a, b| pieces.push((pt(a.x / 16.0, a.y / 16.0), pt(b.x / 16.0, b.y / 16.0))));
        let near = |x: f32, y: f32| {
            pieces.iter().any(|(a, b)| {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let t = (((x - a.x) * dx + (y - a.y) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
                (x - a.x - t * dx).powi(2) + (y - a.y - t * dy).powi(2) <= half * half * scale * scale
            })
        };
        let inside = |i: usize, sub: usize| near((i % side) as f32 + (sub % 8) as f32 / 8.0 + 1.0 / 16.0, (i / side) as f32 + (sub / 8) as f32 / 8.0 + 1.0 / 16.0);
        (0..side * side).map(|i| (0..64).filter(|sub| inside(i, *sub)).count() as f32 / 64.0).collect()
    }

    #[test]
    fn a_round_line_covers_what_lies_within_half_its_thickness() {
        // With round caps a stroke is everything within half its width. Each pixel is held to
        // that count, at four sizes; how far the worst pixel may be off, as a fraction of
        // itself, depends on the stroke.
        let lines = [
            // Strokes overlapping nothing, of every kind: a ring of two arcs with a cross in
            // it, sharp corners, a thick one around a tight bend, nested squares.
            ("M3 12a9 9 0 1 0 18 0a9 9 0 1 0-18 0M8 12h8M12 8v8", 2.0, 0.1),
            ("M4 20 9 4l3 12 3-12 5 16", 1.5, 0.1),
            ("M8 16a4 4 0 0 1 8 0", 7.0, 0.1),
            ("M6 6h12v12h-12zM10 10h4v4h-4z", 2.0, 0.1),
            // Two strokes touching from both sides: where both cover part of a pixel, the
            // coverages are treated as independent, though they are not.
            ("M4 6c6-6 10 14 16 0M4 18q8-10 16 0", 3.0, 0.2),
            // And strokes overlapping themselves: dots, a bend too short to have a middle, a
            // reversal. Their edge is where both halves end, and each half counts there.
            ("M12 12h.01M5 5h0M19 19l.5.5-.5.5", 2.5, 0.25),
            ("M6 12h10l-10 .5", 2.0, 0.4),
        ];
        for ((d, thick, most), side) in lines.iter().flat_map(|line| [16usize, 24, 37, 96].map(|side| (*line, side))) {
            let text = format!(r##"<svg viewBox="0 0 24 24"><path d="{d}" fill="none" stroke="#000" stroke-width="{thick}" stroke-linecap="round" stroke-linejoin="round"/></svg>"##);
            let (have, want) = (decode(&text, side, side).unwrap().1, counted(d, side, thick / 2.0));
            let off = have.iter().zip(&want).map(|(have, want)| ((have >> 24) as f32 / 255.0 - want).abs());
            let worst = off.fold(0.0f32, f32::max);
            assert!(worst < most, "{side} pixels: the worst pixel is off by {worst:.3} of itself, of {d}");
        }
    }

    #[test]
    fn dashes_lie_where_their_lengths_say() {
        let across = |pen: &str| drawn(40, 40, &format!(r#"<path fill="none" stroke="red" stroke-width="2" {pen} d="M0 1h40"/>"#))[..40].to_vec();
        let lit = |row: &[u32]| row.iter().map(|pixel| if *pixel == RED { '#' } else { '.' }).collect::<String>();
        // Dashes of five with gaps of three; an odd-length array repeated; offsets of three and
        // of minus three.
        assert_eq!(lit(&across(r#"stroke-dasharray="5 3""#)), "#####...#####...#####...#####...#####...");
        assert_eq!(lit(&across(r#"stroke-dasharray="4,2,1""#)), "####..#....##.####..#....##.####..#....#");
        assert_eq!(lit(&across(r#"stroke-dasharray="5 3" stroke-dashoffset="3""#)), "##...#####...#####...#####...#####...###");
        assert_eq!(lit(&across(r#"stroke-dasharray="5 3" stroke-dashoffset="-3""#)), "...#####...#####...#####...#####...#####");
        // Arrays that make no dashes leave the stroke whole.
        for whole in ["none", "0 0", "4 -1", "", "x"] {
            assert_eq!(lit(&across(&format!(r#"stroke-dasharray="{whole}""#))), "#".repeat(40), "{whole}");
        }
        // Zero-length dashes with round caps are dots, one every eight from the start. (Where
        // the stroke ends just as a dash would start, there is none.)
        let dots = across(r#"stroke-dasharray="0 8" stroke-linecap="round""#);
        assert_eq!(dots.iter().enumerate().filter(|(_, pixel)| *pixel >> 24 > 100).map(|(x, _)| x).collect::<Vec<usize>>(), [0, 7, 8, 15, 16, 23, 24, 31, 32]);
        // A dash continues around a corner; on a closed outline the last and first dashes join
        // around the corner between them.
        let square = drawn(20, 20, r#"<path fill="none" stroke="red" stroke-width="2" stroke-dasharray="7 5" stroke-dashoffset="3" d="M4 4h12v12h-12z"/>"#);
        let at = |x: usize, y: usize| square[y * 20 + x] == RED;
        assert!(at(3, 3) && at(3, 6) && at(6, 3) && !at(9, 3) && at(16, 3) && at(16, 5), "the corner it starts in is whole");
        // Dashes too fine to tell apart are one stroke.
        assert_eq!(lit(&across(r#"stroke-dasharray="0.00001""#)), "#".repeat(40));
    }

    #[test]
    fn the_ends_of_dashes_stand_square_to_a_curve() {
        // A radius-30 ring cut into twelve dashes and twelve equal gaps, each a 24th of the
        // circumference. A dash ends square to the ring, along a radius: just before each end
        // the ring is covered from inner to outer edge, just after it is clear.
        let part = std::f32::consts::TAU * 30.0 / 24.0;
        let text = format!(r##"<svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="30" fill="none" stroke="#000" stroke-width="16" stroke-dasharray="{part}"/></svg>"##);
        let (_, pixels) = decode(&text, 400, 400).unwrap();
        let solid = |turn: f32, out: f32| {
            let (sin, cos) = (turn * std::f32::consts::TAU / 24.0).sin_cos();
            pixels[((50.0 + out * sin) * 4.0) as usize * 400 + ((50.0 + out * cos) * 4.0) as usize] >> 24
        };
        for (end, out) in (1..24).flat_map(|end| [22.5, 26.0, 30.0, 34.0, 37.5].map(|out| (end as f32, out))) {
            let (before, after) = (solid(end - 0.05, out), solid(end + 0.05, out));
            let dash_before = end as usize % 2 == 1;
            assert_eq!((before == 255, after == 255, before == 0, after == 0), (dash_before, !dash_before, !dash_before, dash_before), "end {end}, {out} out");
        }
    }

    #[test]
    fn a_fill_goes_from_colour_to_colour_along_its_way() {
        let blend = |blend: &str, shape: &str| drawn(100, 100, &format!(r##"<defs>{blend}</defs><rect {shape} fill="url(#b)"/>"##));
        let red_at = |pixels: &[u32], x: usize, y: usize| (pixels[y * 100 + x] >> 16 & 255) as i32;
        let stops = r##"<stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#f00"/>"##;
        // By default across the shape's bounding box, left to right: black at 20, red at 80,
        // halfway at 50.
        let across = blend(&format!(r#"<linearGradient id="b">{stops}</linearGradient>"#), r#"x="20" y="10" width="60" height="30""#);
        assert!([(20, 2), (35, 66), (50, 130), (79, 253)].iter().all(|(x, red)| (red_at(&across, *x, 20) - red).abs() <= 2), "{:?}", [20, 35, 50, 79].map(|x| red_at(&across, x, 20)));
        // In user units, down from 10 to 30; padded with the end colours.
        let down = blend(&format!(r#"<linearGradient id="b" gradientUnits="userSpaceOnUse" x1="0" y1="10" x2="0" y2="30">{stops}</linearGradient>"#), r#"width="100" height="100""#);
        assert!([(5, 0), (20, 134), (29, 249), (60, 255)].iter().all(|(y, red)| (red_at(&down, 50, *y) - red).abs() <= 2), "{:?}", [5, 20, 29, 60].map(|y| red_at(&down, 50, y)));
        // Repeat and reflect; and a transformed gradient.
        let again =
            |spread: &str| blend(&format!(r#"<linearGradient id="b" gradientUnits="userSpaceOnUse" x1="0" x2="20" spreadMethod="{spread}">{stops}</linearGradient>"#), r#"width="100" height="100""#);
        assert!((red_at(&again("repeat"), 25, 5) - 70).abs() <= 2 && (red_at(&again("reflect"), 25, 5) - 185).abs() <= 2 && red_at(&again("pad"), 25, 5) == 255);
        let turned =
            blend(&format!(r#"<linearGradient id="b" gradientUnits="userSpaceOnUse" x2="50" gradientTransform="rotate(90) scale(2)">{stops}</linearGradient>"#), r#"width="100" height="100""#);
        assert!((red_at(&turned, 3, 50) - 129).abs() <= 2 && (red_at(&turned, 90, 50) - 129).abs() <= 2);
        // Radial from the box's centre to a circle touching its sides.
        let out = blend(&format!(r#"<radialGradient id="b">{stops}</radialGradient>"#), r#"width="100" height="100""#);
        assert!([(50, 2), (75, 128), (99, 252)].iter().all(|(x, red)| (red_at(&out, *x, 50) - red).abs() <= 3) && red_at(&out, 2, 2) == 255);
        // From an off-centre focus: the colours crowd toward the side the focus is nearer.
        let off = blend(&format!(r#"<radialGradient id="b" fx="0.75" fy="0.5">{stops}</radialGradient>"#), r#"width="100" height="100""#);
        assert!(red_at(&off, 75, 50) <= 6 && (red_at(&off, 87, 50) - 125).abs() <= 6 && (red_at(&off, 37, 50) - 128).abs() <= 6, "{:?}", [75, 87, 37].map(|x| red_at(&off, x, 50)));
        // A gradient inherits its stops from another it references.
        let after = blend(&format!(r##"<linearGradient id="a">{stops}</linearGradient><linearGradient id="b" xlink:href="#a" x1="1" x2="0"/>"##), r#"width="100" height="100""#);
        assert!(red_at(&after, 2, 50) >= 248 && red_at(&after, 97, 50) <= 7);
        // One stop or zero length gives the last colour everywhere; no stops, no fill; a
        // missing gradient gives its fallback, or nothing.
        let one = blend(r#"<linearGradient id="b"><stop stop-color="red" stop-opacity=".5"/></linearGradient>"#, r#"width="100" height="100""#);
        let flat = blend(&format!(r#"<linearGradient id="b" x1="3" x2="3">{stops}</linearGradient>"#), r#"width="100" height="100""#);
        assert_eq!((count(&one, 0x7fff_0000), count(&flat, RED)), (10000, 10000));
        assert_eq!(covered(&blend(r#"<linearGradient id="b"/>"#, r#"width="100" height="100""#)), 0.0);
        assert_eq!(count(&drawn(10, 10, r##"<rect width="10" height="10" fill="url(#none) red"/><rect width="5" height="10" fill="url(#none)"/>"##), RED), 100);
        // A shape with no height has no bounding box for the gradient: no stroke.
        assert_eq!(covered(&blend(&format!(r#"<linearGradient id="b">{stops}</linearGradient>"#), r##"stroke="url(#b)" stroke-width="4" width="60" height="0""##)), 0.0);
    }

    #[test]
    fn what_is_less_than_solid_shows_what_is_under_it() {
        let at = |pixels: &[u32], x: usize, y: usize| pixels[y * 20 + x] >> 24;
        // Two overlapping half-opaque squares: more shows where both are.
        let each = drawn(20, 20, r#"<rect width="12" height="12" fill="red" fill-opacity=".5"/><rect x="8" y="8" width="12" height="12" fill="red" opacity=".5"/>"#);
        assert_eq!((at(&each, 2, 2), at(&each, 18, 18), at(&each, 10, 10)), (127, 127, 191));
        // As a half-opaque group they composite as one: uniform.
        let group = drawn(20, 20, r#"<g opacity=".5"><rect width="12" height="12" fill="red"/><rect x="8" y="8" width="12" height="12" fill="red"/></g>"#);
        assert_eq!((at(&group, 2, 2), at(&group, 18, 18), at(&group, 10, 10)), (127, 127, 127));
        // And so does a stroked shape: the stroke does not show the fill's edge.
        let lined = drawn(20, 20, r#"<rect x="4" y="4" width="12" height="12" fill="red" stroke="red" stroke-width="4" opacity=".5"/>"#);
        assert_eq!((2..18).map(|x| at(&lined, x, 10)).collect::<Vec<u32>>(), [127; 16]);
        // A stroke lies over its fill, half on it; or with `paint-order` under it, only the
        // outer half showing.
        let ringed = |order: &str| drawn(20, 20, &format!(r##"<rect x="4" y="4" width="12" height="12" fill="red" stroke="#00f" stroke-width="4" {order}/>"##));
        let (over, under) = (ringed(""), ringed(r#"paint-order="stroke""#));
        assert_eq!([count(&over, RED), count(&under, RED), count(&under, 0xff00_00ff)], [64, 144, 256 - 144]);
        // Groups nested past the offscreen depth still multiply their opacities: a half of a
        // half of a half and so on.
        let deep = drawn(20, 20, &format!(r#"{}<rect width="20" height="20" fill="red"/>{}"#, r#"<g opacity=".5">"#.repeat(7), "</g>".repeat(7)));
        assert!((1..=3).contains(&at(&deep, 10, 10)), "{:x}", deep[210]);
    }

    #[test]
    fn what_is_said_of_a_shape_comes_from_many_places() {
        let green = 0xff00_8000;
        // A shape inherits its ancestors' style unless it sets its own.
        assert_eq!(count(&drawn(4, 4, r#"<g fill="green"><rect width="4" height="2"/><rect y="2" width="4" height="2" fill="red"/></g>"#), green), 8);
        // `style` beats style rules, which beat attributes; of the rules, the later wins.
        let sheet = r#"<style>rect { fill: blue } .a { fill: green } #b { fill: red }</style>"#;
        let drawn_with = |shape: &str| drawn(4, 4, &format!("{sheet}{shape}"))[0];
        assert_eq!([drawn_with(r#"<rect class="a" fill="red" width="4" height="4"/>"#), drawn_with(r#"<rect class="x a" id="b" width="4" height="4"/>"#)], [green, RED]);
        assert_eq!([drawn_with(r#"<rect width="4" height="4" fill="red"/>"#), drawn_with(r#"<rect class="a" style="fill:red" width="4" height="4"/>"#)], [0xff00_00ff, RED]);
        // `currentColor`: black in a picture, or what the file sets.
        assert_eq!(drawn(4, 4, r#"<rect width="4" height="4" fill="currentColor"/>"#)[0], 0xff00_0000);
        assert_eq!(drawn(4, 4, r#"<g color="red"><rect width="4" height="4" fill="currentColor"/></g>"#)[0], RED);
        let said = |shapes: &str| ParsedDrawing::read(&format!("<svg>{shapes}</svg>")).unwrap().uses_current_color;
        assert_eq!(
            [said(r#"<path stroke="currentColor" d="M0 0h9"/>"#), said(r#"<path fill="red" d="M0 0h9v9z"/>"#), said(r#"<path color="red" fill="currentColor" d="M0 0h9v9z"/>"#)],
            [true, false, false]
        );
        // What is hidden is not drawn, with all it holds or alone.
        assert_eq!(
            covered(&drawn(4, 4, r#"<g display="none"><rect width="4" height="4"/></g><rect width="4" height="4" visibility="hidden"/><rect width="4" height="4" style="display:none"/>"#)),
            0.0
        );
        assert_eq!(covered(&drawn(4, 4, r#"<g visibility="hidden"><rect width="4" height="2" visibility="visible"/><rect y="2" width="4" height="2"/></g>"#)), 8.0);
    }

    #[test]
    fn a_shape_is_drawn_wherever_it_is_used() {
        // In `<defs>` or a `<symbol>`, a shape is drawn only where a `<use>` puts it, in the
        // colours set there.
        let used = drawn(
            12,
            12,
            r##"<defs><rect id="a" width="2" height="2"/></defs><symbol id="b"><rect width="2" height="2"/></symbol>
            <use href="#a" x="1" y="1" fill="red"/><use xlink:href="#b" transform="translate(5 5)" fill="red"/><use href="#a" x="9" transform="scale(1 6)" fill="red"/>"##,
        );
        assert_eq!((count(&used, RED), count(&used, 0)), (4 + 4 + 24, 144 - 32));
        assert!(used[13] == RED && used[5 * 12 + 5] == RED && used[11 * 12 + 9] == RED);
        // A used group with groups in it; and one used by a used one.
        let groups = drawn(
            12,
            12,
            r##"<defs><g id="a"><g transform="translate(1 0)"><rect width="1" height="1" fill="red"/></g></g><g id="b"><use href="#a"/><use href="#a" y="2"/></g></defs>
            <use href="#b"/><use href="#b" x="4"/>"##,
        );
        assert_eq!((count(&groups, RED), groups[1], groups[2 * 12 + 5]), (4, RED, RED));
        // A `<use>` of an ancestor draws nothing, nor does one of what is already being
        // brought: one using itself through another is drawn once where the other puts it, and
        // reading ends.
        let own = drawn(12, 12, r##"<g id="a"><rect width="1" height="1" fill="red"/><use href="#a" x="2"/><use href="#b" x="4"/></g><g id="b"><use href="#a" y="3"/></g>"##);
        assert_eq!((count(&own, RED), own[0], own[3 * 12 + 4], own[3 * 12]), (3, RED, RED, RED));
        // References to nothing, or to non-shapes, draw nothing.
        assert_eq!(covered(&drawn(4, 4, r##"<defs><linearGradient id="a"/></defs><use href="#a"/><use href="#none"/><use/>"##)), 0.0);
    }

    #[test]
    fn a_few_elements_cannot_stand_for_more_shapes_than_a_file_could_hold() {
        // Ten levels each using the previous ten times: ten billion squares in a few hundred
        // bytes. No more are kept than a file that long could write out.
        let mut text = String::from(r##"<svg viewBox="0 0 4 4"><defs><rect id="a0" width="1" height="1"/>"##);
        for level in 1..=10 {
            text += &format!(r##"<g id="a{level}">{}</g>"##, format!(r##"<use href="#a{}"/>"##, level - 1).repeat(10));
        }
        text += r##"</defs><use href="#a10"/></svg>"##;
        let kept = ParsedDrawing::read(&text).unwrap();
        assert!(kept.items.len() <= (1 << 16) + 1 && kept.paths.len() == 1, "{} shapes of {} outlines", kept.items.len(), kept.paths.len());
        // Groups nested deeper than any real drawing are skipped from there on.
        let deep = format!(r#"<svg viewBox="0 0 4 4">{}<rect width="4" height="4"/>{}<rect width="2" height="2" fill="red"/></svg>"#, "<g>".repeat(5000), "</g>".repeat(5000));
        assert_eq!(count(&decode(&deep, 4, 4).unwrap().1, RED), 4);
    }

    #[test]
    fn a_drawing_fits_the_box_it_is_shown_in() {
        let size = |head: &str| ParsedDrawing::read(&format!("<svg {head}/>")).map(|kept| (kept.size, kept.view));
        // Its stated size; one side given, the other from the aspect; none, the picture's size
        // or 300 by 150.
        assert_eq!(size(r#"width="24" height="12""#), Some(([24.0, 12.0], [0.0, 0.0, 24.0, 12.0])));
        assert_eq!(size(r#"width="2in" viewBox="5 5 40 10""#), Some(([192.0, 48.0], [5.0, 5.0, 40.0, 10.0])));
        assert_eq!(size(r#"viewBox="0 0 16 16" width="100%""#), Some(([16.0; 2], [0.0, 0.0, 16.0, 16.0])));
        assert_eq!(size(""), Some(([300.0, 150.0], [0.0, 0.0, 300.0, 150.0])));
        // A zero or unreadable size counts as unstated.
        assert_eq!(
            [size(r#"viewBox="0 0 0 10" width="8" height="8""#), size(r#"viewBox="1 2 3""#)].map(|read| read.map(|(_, view)| view)),
            [Some([0.0, 0.0, 8.0, 8.0]), Some([0.0, 0.0, 300.0, 150.0])]
        );
        assert!(ParsedDrawing::read("<g/>").is_none() && ParsedDrawing::read("").is_none() && ParsedDrawing::read(r#"<svg viewBox="0 0 1e39 5"/>"#).is_none());
        // In a box of another shape all of it shows, centred; or as the file asks: aligned to a
        // side, stretched, or sliced.
        let wide = |fitted: &str| decode(&format!(r#"<svg viewBox="0 0 4 4" preserveAspectRatio="{fitted}"><rect width="4" height="4" fill="red"/></svg>"#), 8, 4).unwrap().1;
        let lit = |pixels: &[u32]| pixels[..8].iter().map(|pixel| if *pixel == RED { '#' } else { '.' }).collect::<String>();
        assert_eq!(
            [lit(&wide("xMidYMid")), lit(&wide("xMinYMid meet")), lit(&wide("xMaxYMax")), lit(&wide("none")), lit(&wide("xMidYMid slice"))],
            ["..####..", "####....", "....####", "########", "########"]
        );
        assert_eq!(decode(r#"<svg viewBox="0 0 4 4"/>"#, 0, 4), None);
    }

    #[test]
    fn what_is_not_read_leaves_the_rest_as_it_is() {
        // An external image, animation, unknown elements: shapes around them draw as if they
        // were absent, shapes inside them do not. (Text on a path writes nothing.)
        let rest = r#"<rect x="1" y="1" width="6" height="3" fill="red"/><circle cx="4" cy="6" r="1.5" fill="blue"/>"#;
        let with = format!(
            r##"<defs><path id="t" d="M0 0h8"/></defs><image href="none.png" width="8" height="8"/>
            <foreignObject width="8" height="8"><p xmlns="http://www.w3.org/1999/xhtml">html</p></foreignObject><unknown><rect width="8" height="8"/></unknown>
            <g>{rest}<animate attributeName="x" from="0" to="8" dur="1s"/></g><script>var x = 1 &lt; 2;</script><textPath href="#t">words</textPath>"##
        );
        assert_eq!(drawn(8, 32, &with), drawn(8, 32, rest));
        // A pattern of no size or content fills with nothing: its fallback is not used.
        let empty = r##"<pattern id="p" width="2" height="2"/><pattern id="q" width="0" height="2"><rect width="1" height="1"/></pattern>"##;
        let filled =
            drawn(4, 4, &format!(r##"{empty}<rect width="4" height="1" fill="url(#p)"/><rect y="1" width="4" height="1" fill="url(#q)"/><rect y="2" width="4" height="2" fill="url(#p) red"/>"##));
        assert_eq!((count(&filled, RED), count(&filled, 0)), (0, 16));
    }

    #[test]
    fn a_clip_path_keeps_only_what_lies_inside_it() {
        // Its shapes, in the clipped element's units (or its bounding box), filled by their own
        // rule, wherever and whatever colour; their strokes count for nothing.
        let clipped = |clip: &str, shape: &str| count(&drawn(8, 8, &format!(r##"<clipPath id="c" {clip}</clipPath>{shape}"##)), RED);
        let square = r#"<rect width="8" height="8" fill="red" clip-path="url(#c)"/>"#;
        assert_eq!(clipped(r#"><rect x="2" y="2" width="4" height="4" fill="none" stroke="blue" stroke-width="2"/>"#, square), 16);
        assert_eq!(clipped(r#"clipPathUnits="objectBoundingBox"><rect width=".5" height="1"/>"#, r#"<rect x="2" width="4" height="8" fill="red" clip-path="url(#c)"/>"#), 16);
        assert_eq!(clipped(r#"transform="translate(4 4)"><rect width="8" height="8"/>"#, square), 16);
        let nested = r#"><path clip-rule="evenodd" d="M0 0h8v8h-8z M2 2h4v4h-4z"/>"#;
        assert_eq!((clipped(nested, square), clipped(&nested.replace("evenodd", "nonzero"), square)), (64 - 16, 64));
        // A group is clipped as one, with what it uses; a clip path holds no groups; a dangling
        // one clips nothing.
        let group = r##"<defs><rect id="r" x="4" width="4" height="8" fill="red"/></defs><g clip-path="url(#c)"><rect width="4" height="8" fill="red"/><use href="#r"/></g>"##;
        assert_eq!(clipped(r#"><rect y="6" width="8" height="2"/>"#, group), 16);
        assert_eq!(clipped(r#"><g><rect width="8" height="8"/></g>"#, square), 0);
        assert_eq!(clipped(r#"><rect width="1" height="1"/>"#, &square.replace("#c", "#none")), 64);
    }

    #[test]
    fn a_box_square_to_the_picture_cuts_exactly_at_its_edges() {
        // A shape and its clip box ending in the same part of a pixel: the pixel keeps the part
        // both cover (three quarters), not that fraction of the fraction (as multiplying two
        // coverages would). For a box clip path and for a nested viewport.
        let edge = |shapes: &str| drawn(4, 4, shapes)[4] >> 24;
        let clip = |turn: &str| format!(r##"<clipPath id="c"><rect x=".25" width="3.5" height="4" {turn}/></clipPath>"##);
        assert!(edge(&format!(r##"{}<rect x=".25" width="3.5" height="4" fill="red" clip-path="url(#c)"/>"##, clip(""))).abs_diff(191) <= 1);
        assert!(edge(r#"<svg x=".25" width="3.5" height="4" viewBox=".25 0 3.5 4"><rect x=".25" width="3.5" height="4" fill="red"/></svg>"#).abs_diff(191) <= 1);
        // A stroke is clipped as any clip path clips: its coverage (half here) times the box's
        // (three quarters).
        assert!(edge(&format!(r##"{}<path d="M.25 0V4" stroke="red" stroke-width=".5" clip-path="url(#c)"/>"##, clip(""))).abs_diff(96) <= 1);
        // Rotated, a box clips as any shape: the coverages multiplied.
        let turned = r##"<clipPath id="c"><rect y=".25" width="4" height="3.5" transform="rotate(90 2 2)"/></clipPath>"##;
        assert!(edge(&format!(r##"{turned}<rect x=".25" width="3.5" height="4" fill="red" clip-path="url(#c)"/>"##)).abs_diff(143) <= 2);
    }

    #[test]
    fn a_mask_keeps_as_much_as_it_is_bright() {
        // White keeps all, black nothing, grey half; the mask's alpha counts too.
        let masked = |mask: &str| drawn(4, 4, &format!(r##"<mask id="m">{mask}</mask><rect width="4" height="4" fill="red" mask="url(#m)"/>"##));
        assert_eq!(count(&masked(r#"<rect width="4" height="4" fill="white"/>"#), RED), 16);
        assert_eq!(covered(&masked(r#"<rect width="4" height="4" fill="black"/>"#)), 0.0);
        let half = |mask: &str| masked(mask).iter().all(|pixel| (pixel >> 24).abs_diff(128) <= 2);
        assert!(half(r##"<rect width="4" height="4" fill="#808080"/>"##) && half(r#"<rect width="4" height="4" fill="white" fill-opacity=".5"/>"#));
        // It extends a tenth of the masked element's bounding box past each side by default, in
        // bounding-box fractions or units; its content in units, or bounding box.
        let cut = |mask: &str| count(&drawn(8, 8, &format!(r##"<mask id="m" {mask}</mask><rect x="2" y="2" width="4" height="4" fill="red" mask="url(#m)"/>"##)), RED);
        let white = r#"<rect width="8" height="8" fill="white"/>"#;
        assert_eq!(
            [cut(&format!(">{white}")), cut(&format!(r#"x="0" y="0" width=".5" height=".5">{white}"#)), cut(&format!(r#"maskUnits="userSpaceOnUse" x="3" y="3" width="2" height="2">{white}"#))],
            [16, 4, 4]
        );
        assert_eq!(cut(r#"maskContentUnits="objectBoundingBox"><rect width=".5" height="1" fill="white"/>"#), 8);
    }

    #[test]
    fn a_pattern_fills_a_shape_with_its_tiles_laid_edge_to_edge() {
        // A 2×2-unit tile with a red square in a corner over an 8×8 shape: one pixel in four
        // red, at any pixel size where tile edges fall on pixel edges.
        let tiled = |pattern: &str, side: usize| drawn(8, side, &format!(r##"<pattern id="p" {pattern}</pattern><rect width="8" height="8" fill="url(#p)"/>"##));
        let corner = r#"patternUnits="userSpaceOnUse" width="2" height="2"><rect width="1" height="1" fill="red"/>"#;
        for side in [8, 16, 32] {
            let pixels = tiled(corner, side);
            assert_eq!(count(&pixels, RED), side * side / 4, "{side} pixels");
            assert!(pixels[0] == RED && pixels[side / 8] == 0 && pixels[side / 4] == RED, "{side} pixels");
        }
        // In bounding-box fractions; offset; content fitted via `viewBox`, or in bounding-box
        // fractions: the same tiles.
        assert_eq!(tiled(r#"width=".25" height=".25"><rect width="1" height="1" fill="red"/>"#, 16), tiled(corner, 16));
        assert_eq!(tiled(&corner.replace(r#"width="2""#, r#"patternTransform="translate(4 2)" width="2""#), 16), tiled(corner, 16));
        assert_eq!(tiled(r#"patternUnits="userSpaceOnUse" width="2" height="2" viewBox="0 0 10 10"><rect width="5" height="5" fill="red"/>"#, 16), tiled(corner, 16));
        assert_eq!(tiled(r#"width=".25" height=".25" patternContentUnits="objectBoundingBox"><rect width=".125" height=".125" fill="red"/>"#, 16), tiled(corner, 16));
        // A pattern inherits from another what it does not say; and strokes are tiled too.
        let after = drawn(8, 8, &format!(r##"<pattern id="p" {corner}</pattern><pattern id="q" href="#p"/><rect width="8" height="8" fill="url(#q)"/>"##));
        assert_eq!(after, tiled(corner, 8));
        let line = drawn(8, 8, &format!(r##"<pattern id="p" {corner}</pattern><rect x="1" y="1" width="6" height="6" fill="none" stroke="url(#p)" stroke-width="2"/>"##));
        assert_eq!(count(&line, RED), 12);
    }

    #[test]
    fn markers_stand_at_the_ends_and_the_corners_of_an_outline() {
        // A 2×2-unit marker centred on each vertex of a three-segment path: start, middle and
        // end markers, each where given.
        let marked = |markers: &str, d: &str| drawn(16, 16, &format!(r##"<marker id="m" markerUnits="userSpaceOnUse" {markers}"##).replace("$d", d));
        let square = r#"markerWidth="2" markerHeight="2" refX="1" refY="1"><rect width="2" height="2" fill="red"/></marker><path d="$d" fill="none" stroke-width="0" marker-start="url(#m)" marker-mid="url(#m)" marker-end="url(#m)"/>"#;
        assert_eq!(count(&marked(square, "M3 3H13V13H3"), RED), 4 * 4);
        assert_eq!(count(&marked(&square.replace(r#" marker-mid="url(#m)""#, ""), "M3 3H13V13H3"), RED), 2 * 4);
        // Oriented along the path where it says so: a 3×1 marker lies along each segment at its
        // end, and the start one reversed where it says that.
        let long =
            |orient: &str, d: &str| marked(&format!(r#"orient="{orient}" markerWidth="3" refX="0" refY="0.5"><rect width="3" height="1" fill="red"/></marker><path d="$d" marker-end="url(#m)"/>"#), d);
        let pixels = long("auto", "M8.5 2V8");
        assert!(count(&pixels, RED) == 3 && (8..11).all(|y| pixels[y * 16 + 8] == RED), "down, along the line");
        let pixels = long("0", "M8 2.5V8.5");
        assert!(count(&pixels, RED) == 3 && (8..11).all(|x| pixels[8 * 16 + x] == RED), "not turned");
        // Scaled by the stroke width unless in user units.
        let thick = drawn(16, 16, r##"<marker id="m" markerWidth="2" markerHeight="2"><rect width="2" height="2" fill="red"/></marker><path d="M2 2h8" stroke-width="2" marker-end="url(#m)"/>"##);
        assert_eq!(count(&thick, RED), 16);
    }

    #[test]
    fn what_drawing_costs_is_counted_as_times_over_the_whole_picture() {
        let passes = |shapes: &str| Reading::of(&format!(r#"<svg viewBox="0 0 100 100">{shapes}</svg>"#), None).unwrap().passes;
        let costs = |shapes: &str, passes_kept: f32| assert!((passes(shapes) - passes_kept).abs() < 1e-3, "{shapes} costs {} times over the picture", passes(shapes));
        // A shape costs its bounding box's share of the picture (never more than all, nothing
        // without size): rotated, its rotated box.
        costs(r#"<rect width="100" height="100"/>"#, 1.0);
        costs(r#"<rect x="50" y="50" width="50" height="50"/><rect width="1000" height="1000"/><rect width="0" height="5"/>"#, 1.25);
        costs(r#"<rect width="25" height="50" transform="scale(2 1)"/><rect width="10" height="10" transform="rotate(45 5 5)"/>"#, 0.25 + 0.02);
        // Its stroke as much again, out to the miter; a per-pixel paint more (a gradient 8
        // times).
        costs(r#"<rect x="10" y="10" width="80" height="80" stroke="red" stroke-width="5"/>"#, 2.0);
        costs(r#"<linearGradient id="g"><stop/><stop offset="1" stop-color="red"/></linearGradient><rect width="50" height="100" fill="url(#g)"/>"#, 4.0);
        // An offscreen group is a full-picture block; clipped by a clip path, two; by a mask,
        // three, plus the mask's box filled.
        costs(r#"<g opacity=".5"/>"#, 1.0);
        costs(r#"<clipPath id="c"><circle cx="50" cy="50" r="50"/></clipPath><g clip-path="url(#c)"><rect width="100" height="50"/></g>"#, 1.0 + 0.5 + 1.0 + 1.0);
        costs(r#"<mask id="m"><rect width="100" height="100" fill="white"/></mask><g mask="url(#m)"><rect width="100" height="50"/></g>"#, 1.0 + 0.5 + 1.0 + 1.0 + 2.0);
        // A tile is a block the size it shows, and its content is costed in fractions of that;
        // a pattern-filled shape costs 20 times a solid one.
        let tiles = r#"<pattern id="p" width="50" height="50" patternUnits="userSpaceOnUse"><rect width="50" height="25"/></pattern><rect width="100" height="100" fill="url(#p)"/>"#;
        costs(tiles, 0.25 + 0.125 + 20.0);
    }

    #[test]
    fn a_marker_that_holds_a_group_is_drawn_at_every_corner() {
        // A hundred vertices, each with a marker holding a one-pixel square in a group: all
        // drawn, not only the last sixty (their walks lie side by side, not nested, none deeper
        // than the path).
        let corners: String = (1..=100).map(|n| format!(" {} 1", 2 * n)).collect();
        let text = format!(
            r##"<svg viewBox="0 0 202 2"><marker id="k" markerUnits="userSpaceOnUse"><g><rect width="1" height="1" fill="red"/></g></marker><path d="M0 1{corners}" fill="none" marker="url(#k)"/></svg>"##
        );
        assert_eq!(count(&decode(&text, 202, 2).unwrap().1, RED), 101);
    }

    #[test]
    fn a_drawing_in_the_drawing_fits_its_box_and_is_cut_off_at_its_edges() {
        // A `<symbol>` brought by a `<use>`, sized by the use: its `viewBox` fitted to that
        // box, the rest clipped.
        let used = drawn(8, 8, r##"<symbol id="s" viewBox="0 0 2 2"><rect x="-1" y="-1" width="4" height="4" fill="red"/></symbol><use href="#s" x="2" y="2" width="4" height="4"/>"##);
        assert!(count(&used, RED) == 16 && used[2 * 8 + 2] == RED && used[5 * 8 + 5] == RED);
        // A nested `<svg>` likewise, sized as it says; with `overflow` visible, not clipped.
        let inner =
            |overflow: &str| count(&drawn(8, 8, &format!(r#"<svg x="2" y="2" width="4" height="4" viewBox="0 0 1 1" {overflow}><rect x="-.25" width="1.5" height="1" fill="red"/></svg>"#)), RED);
        assert_eq!((inner(""), inner(r#"overflow="visible""#)), (16, 24));
        // One fitted into a box of another shape sits centred.
        let middle = drawn(8, 8, r#"<svg width="8" height="4" viewBox="0 0 4 4"><rect width="4" height="4" fill="red"/></svg>"#);
        assert!(count(&middle, RED) == 16 && middle[2] == RED && middle[1] == 0 && middle[5] == RED && middle[6] == 0);
    }

    #[test]
    fn text_is_written_in_the_fonts_of_the_system() {
        enable_text();
        // Where the system has fonts: letters in their piece's colour, along a line from the
        // text's position, anchored at start, middle or end.
        let written = |text: &str| drawn(64, 64, text);
        let words = written(r#"<text x="8" y="40" font-family="sans-serif" font-size="20" fill="red">Wave</text>"#);
        let columns = |pixels: &[u32]| (0..64).filter(|x| (0..64).any(|y| pixels[y * 64 + x] >> 24 > 0)).collect::<Vec<usize>>();
        let (left, right) = (columns(&words)[0], *columns(&words).last().unwrap());
        assert!(count(&words, RED) > 20 && left >= 7 && right > 40 && (0..64).all(|x| words[44 * 64 + x] == 0 && words[20 * 64 + x] == 0), "{left}..{right}");
        let ended = written(r#"<text x="56" y="40" text-anchor="end" font-family="sans-serif" font-size="20" fill="red">Wave</text>"#);
        // The ink ends a little before the advance, by the last glyph's right bearing.
        let (end, inked) = (*columns(&ended).last().unwrap(), columns(&ended).len());
        assert!((50..=56).contains(&end) && inked.abs_diff(columns(&words).len()) <= 2, "{end}, {inked} columns");
        let middle = written(r#"<text x="32" y="40" text-anchor="middle" font-family="sans-serif" font-size="20" fill="red">Wave</text>"#);
        let (first, last) = (columns(&middle)[0], *columns(&middle).last().unwrap());
        assert!((first + last).abs_diff(64) <= 4, "{first}..{last}");
        // A piece in its own colour, moved down. Repeated and edge whitespace, and entity
        // references, as a browser reads them.
        let pieces = written(r#"<text x="8" y="30" font-family="sans-serif" font-size="16" fill="red">A<tspan fill="blue" dy="10">B</tspan></text>"#);
        let tinted = |rgb: u32| pieces.iter().filter(|pixel| *pixel & 0xff_ffff == rgb && **pixel >> 24 > 0x80).count();
        assert!(tinted(0xff_0000) > 10 && tinted(0xff) > 10);
        let plain = written(r#"<text x="4" y="30" font-family="sans-serif" font-size="12">A B&amp;C</text>"#);
        assert_eq!(written("<text x=\"4\" y=\"30\" font-family=\"sans-serif\" font-size=\"12\">\n  A \t B&#38;C  </text>"), plain);
        // Letter spacing takes more room.
        let apart = written(r#"<text x="4" y="30" font-family="sans-serif" font-size="12" letter-spacing="3">A B&amp;C</text>"#);
        assert!(columns(&apart).last() > columns(&plain).last());
        // Each letter at its own position, where the text gives one.
        let placed = written(r#"<text x="4 40" y="30" font-family="sans-serif" font-size="16">II</text>"#);
        assert!(columns(&placed).iter().any(|x| *x > 40) && !columns(&placed).iter().any(|x| (20..40).contains(x)));
    }

    /// Text against librsvg with the same system fonts: within the tests' drawing bounds (no
    /// pixel more than 32 levels off, mean under 1.2). Without `rsvg-convert` there is nothing
    /// to compare. (`cargo test --lib svg -- --ignored`.)
    #[test]
    #[ignore = "needs rsvg-convert, which writes with the system's fonts"]
    fn text_comes_out_as_librsvg_writes_it() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        enable_text();
        let file = include_str!("../tests/pictures/words.svg");
        let weighed = |pixel: u32| [24, 16, 8, 0].map(|shift| (pixel >> shift & 255) as i32 * if shift == 24 { 255 } else { (pixel >> 24) as i32 } / 255);
        for [wide, high] in [[200, 100], [400, 200]] {
            let Ok(mut drawing) = Command::new("rsvg-convert").args(["-w", &wide.to_string(), "-h", &high.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else { return };
            drawing.stdin.take().unwrap().write_all(file.as_bytes()).unwrap();
            let (_, wanted) = crate::png::decode(&drawing.wait_with_output().unwrap().stdout).unwrap();
            let (_, pixels) = decode(file, wide, high).unwrap();
            let off: Vec<i32> = pixels.iter().zip(&wanted).flat_map(|(one, other)| (0..4).map(|part| (weighed(*one)[part] - weighed(*other)[part]).abs())).collect();
            let (worst, mean) = (*off.iter().max().unwrap(), off.iter().sum::<i32>() as f32 / off.len() as f32);
            assert!(worst <= 32 && mean < 1.2, "{wide} by {high}: the worst pixel is {worst} levels off, the mean {mean:.3}");
        }
    }

    /// The test drawings, as they lie beside the tests.
    const FILES: [&str; 6] = [
        include_str!("../tests/pictures/lines.svg"),
        include_str!("../tests/pictures/solid.svg"),
        include_str!("../tests/pictures/logo.svg"),
        include_str!("../tests/pictures/shapes.svg"),
        include_str!("../tests/pictures/lines-of-all-kinds.svg"),
        include_str!("../tests/pictures/dashes.svg"),
    ];

    #[test]
    fn a_damaged_file_gives_nothing_or_some_drawing() {
        let sound = |text: &str| decode(text, 20, 14).is_none_or(|(width, pixels)| width == 20 && pixels.len() == 280);
        for file in FILES {
            // Truncated at every length (also inside a multi-byte letter).
            assert!((0..file.len()).all(|cut| sound(&String::from_utf8_lossy(&file.as_bytes()[..cut]))));
            // Then whole, each byte changed in turn: a flipped bit, a `<`, a quote, a non-ASCII
            // byte.
            for at in 0..file.len() {
                let damaged = |to: u8| String::from_utf8_lossy(&[&file.as_bytes()[..at], &[to], &file.as_bytes()[at + 1..]].concat()).into_owned();
                assert!([file.as_bytes()[at] ^ (1 << (at % 8)), b'<', b'"', 0xF0].into_iter().all(|to| sound(&damaged(to))), "byte {at}");
            }
        }
    }

    #[test]
    fn numbers_beyond_all_measure_stop_nothing() {
        // Sizes, positions, widths, dashes and transforms no real drawing has: each read and
        // drawn (as little), quickly.
        let wild = ["1e38", "-1e38", "1e-38", "9e99", "-9e99", "NaN", "inf", "0", "-0", "1e20", "0.0000001"];
        for (a, b) in wild.iter().flat_map(|a| wild.iter().map(move |b| (*a, *b))) {
            let text = format!(
                r##"<svg viewBox="0 0 100 100"><defs><linearGradient id="g" x1="{a}" x2="{b}" gradientTransform="scale({a} {b}) rotate({b})"><stop offset="{a}" stop-color="red"/><stop offset="{b}"/></linearGradient>
                <radialGradient id="r" cx="{a}" r="{b}" fx="{b}"><stop stop-color="red"/><stop offset="1"/></radialGradient></defs>
                <rect x="{a}" y="{b}" width="{b}" height="{a}" rx="{a}" fill="url(#g)" stroke="url(#r)" stroke-width="{b}" stroke-dasharray="{a} {b}" stroke-dashoffset="{a}"/>
                <circle cx="{a}" cy="{b}" r="{a}" stroke="red" stroke-width="{a}" stroke-miterlimit="{b}"/><ellipse rx="{a}" ry="{b}" transform="matrix({a} {b} {b} {a} {a} {b})"/>
                <path d="M{a} {b}L{b} {a}A{a} {b} {a} 1 1 {b} {a}C{a} {b} {b} {a} 5 5z" stroke="blue" stroke-width="{b}" stroke-linejoin="round" stroke-linecap="round" transform="scale({a}) skewX({b})"/>
                <polyline points="{a} {b} 5 5 {b} {a}" stroke="#000" stroke-linecap="square" opacity="{a}"/><g transform="translate({a} {b})"><rect width="50" height="50" fill="url(#g)"/></g></svg>"##
            );
            assert!(decode(&text, 24, 24).is_some(), "{a} and {b}");
            let sized = format!(r#"<svg width="{a}" height="{b}" viewBox="{a} {b} {b} {a}"><rect width="{a}" height="{b}"/><path d="M0 0h{a}v{b}z" stroke="red" stroke-width="{a}"/></svg>"#);
            assert!(decode(&sized, 24, 24).is_none_or(|(width, pixels)| width == 24 && pixels.len() == 576), "a drawing {a} by {b}");
        }
    }

    /// `CORPUS=folder`: every drawing in it and its subfolders is read (with text, as a program
    /// asking for text reads it), and none comes near any cost bound. The nearest are printed
    /// with their costs.
    #[test]
    #[ignore = "reads a folder of drawings: CORPUS=folder"]
    fn no_drawing_in_a_folder_comes_near_what_a_drawing_may_cost() {
        let Ok(corpus) = std::env::var("CORPUS") else { return eprintln!("CORPUS=folder names no folder: no drawings are read") };
        // With the text, filters and images a program that asks reads.
        enable_text();
        enable_images();
        enable_filters();
        let (mut files, mut folders) = (Vec::new(), vec![std::path::PathBuf::from(corpus)]);
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let (path, kind) = (entry.path(), entry.file_type().unwrap());
                match kind.is_dir() {
                    true => folders.push(path),
                    false => files.extend(path.extension().is_some_and(|ending| ending == "svg").then_some(path)),
                }
            }
        }
        // For each, how near it comes to each bound (1 is there), and its passes.
        let (mut near, mut most_lookups) = (Vec::new(), (0, std::path::PathBuf::new()));
        for file in &files {
            let read = |text: String| Reading::of(&text, None).map(|reading| (reading.passes, [reading.parsed.items.len(), reading.pieces, reading.steps, reading.lookups], text.len()));
            let Some((passes_kept, [items, pieces, steps, lookups], long)) = std::fs::read_to_string(file).ok().and_then(read) else { continue };
            most_lookups = most_lookups.max((lookups, file.clone()));
            let [shapes, outlines] = most(long);
            let near_each =
                [passes_kept / passes(long), items as f32 / shapes as f32, pieces as f32 / outlines as f32, steps as f32 / (16 * shapes) as f32, lookups as f32 / LOOKUPS.max(long / 512) as f32];
            near.push((near_each, passes_kept, long, file.clone()));
        }
        for bound in 0..5 {
            near.sort_by(|a, b| b.0[bound].total_cmp(&a.0[bound]));
            println!("by bound {bound}, of {} drawings:", near.len());
            for (ratios, passes_kept, long, file) in near.iter().take(12) {
                println!("  {:.3} {:.3} {:.3} {:.3} {:.3} {passes_kept:8.2} passes {long:8} bytes {}", ratios[0], ratios[1], ratios[2], ratios[3], ratios[4], file.display());
            }
        }
        let mut all: Vec<f32> = near.iter().map(|(_, passes_kept, ..)| *passes_kept).collect();
        all.sort_by(f32::total_cmp);
        let at = |part: f32| all.get(((all.len() as f32 - 1.0) * part) as usize).copied().unwrap_or(0.0);
        println!("passes: half {:.2}, 90% {:.2}, 99% {:.2}, 99.9% {:.2}, most {:.2}", at(0.5), at(0.9), at(0.99), at(0.999), at(1.0));
        println!("fonts looked for: most {} ({})", most_lookups.0, most_lookups.1.display());
        let worst = near.iter().map(|(ratios, ..)| ratios.iter().fold(0.0f32, |a, b| a.max(*b))).fold(0.0, f32::max);
        assert!(worst < 0.5, "a drawing comes to {worst} of a bound");
    }

    /// One row of a 40×40-unit drawing drawn at 40×40 pixels.
    fn row_of(shapes: &str, row: usize) -> Vec<u32> {
        decode(&format!(r#"<svg viewBox="0 0 40 40">{shapes}</svg>"#), 40, 40).unwrap().1[row * 40..(row + 1) * 40].to_vec()
    }
    /// Whether two pixel rows are within so many levels, premultiplied (so barely visible
    /// differences count as little as they show).
    fn near(have: &[u32], wanted: &[u32], levels: u32) -> bool {
        let weighed = |pixel: u32| [24, 16, 8, 0].map(|shift| if shift == 24 { pixel >> 24 } else { (pixel >> shift & 255) * (pixel >> 24) / 255 });
        have.len() == wanted.len() && have.iter().zip(wanted).all(|(have, wanted)| weighed(*have).iter().zip(weighed(*wanted)).all(|(a, b)| a.abs_diff(b) <= levels))
    }
    /// A pixel row as written from a picture: hex each, a dot for clear.
    fn written(row: &str) -> Vec<u32> {
        row.split_whitespace().map(|pixel| if pixel == "." { 0 } else { u32::from_str_radix(pixel, 16).unwrap() }).collect()
    }
    /// A red square from 10 to 30 units each way, filtered by `e`.
    const SQUARE: &str = r#"<rect x="10" y="10" width="20" height="20" fill="red" filter="url(#e)"/>"#;
    /// Red and green halves of the drawing, filtered by `e`.
    const HALVES: &str = r##"<g filter="url(#e)"><rect width="20" height="40" fill="#f00"/><rect x="20" width="20" height="40" fill="#0f0"/></g>"##;
    // Rows of what librsvg 2.62 (`rsvg-convert` on Arch Linux) draws of the same files. (Row
    // 20.)
    const BLURRED: &str = ". . . . . . . 35ff0000 4fff0000 6eff0000 \
        8fff0000 acff0000 c7ff0000 dcff0000 ebff0000 f5ff0000 fbff0000 feff0000 ffff0000 ffff0000 \
        ffff0000 ffff0000 feff0000 fbff0000 f5ff0000 ebff0000 dcff0000 c7ff0000 acff0000 8fff0000 \
        6eff0000 4fff0000 35ff0000 . . . . . . .";
    // (Row 20.)
    const LINEAR: &str = "64ff0000 7fff0000 9bff0000 b7ff0000 d4ff0000 e3ff0000 eeff0000 f6ff0000 fbff0000 fdff0000 \
        fffe1600 fffd2200 fffb3500 fff84700 fff45800 ffee6a00 ffe67e00 ffdc9100 ffd1a300 ffc3b400 \
        ffb4c300 ffa3d100 ff91dc00 ff7ee600 ff6aee00 ff58f400 ff47f800 ff35fb00 ff22fd00 ff16fe00 \
        fd00ff00 fb00ff00 f400ff00 eb00ff00 dd00ff00 cc00ff00 b000ff00 9400ff00 7800ff00 5d00ff00";
    // (Row 20.)
    const SCREEN: &str = "64ff0000 7fff0000 9bff0000 b7ff0000 d4ff0000 e3ff0000 eeff0000 f6ff0000 fbff0000 fdff0000 \
        fffd0200 fffb0400 fff60900 ffef1000 ffe61900 ffda2500 ffca3500 ffb74800 ffa25e00 ff8b7400 \
        ff748b00 ff5ea200 ff48b700 ff35ca00 ff25da00 ff19e600 ff10ef00 ff09f600 ff04fb00 ff02fd00 \
        fd00ff00 fb00ff00 f400ff00 eb00ff00 dd00ff00 cc00ff00 b000ff00 9400ff00 7800ff00 5d00ff00";
    // (Row 10.)
    const MOVED: &str = ". . . . . . . . . . \
        . . 10ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 \
        40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 40ff0000 \
        . . . . . . . . . .";
    // (Row 0.)
    const NOISE: &str = ". 1e666f80 20708f9f 1a6289b1 196685ad 05006699 114b78a5 14596699 1b5e687b 1c494949 \
        . 27625c41 378b7d5d 2f92886d 1586796d 03000000 19857a52 2e907a3d 338c6e28 235f422c \
        . 2c68574b 4f8e7754 5e9b8d5c 579b985e 46929d5f 3f9e9e75 329e9e80 26948d80 156d6d6d \
        . 21645564 438d5b6b 52ab7364 4ab77553 36b35e34 42b96c4d 39b77067 23a05f66 18754055";
    // (Row 11.)
    const THICKER: &str = ". . . ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 \
        ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 . \
        . . . . . . . . . . \
        . . . . . . . . . .";
    // (Row 3.)
    const TILED: &str = ". . ffff0000 ffff0000 ffff0000 . . ffff0000 ffff0000 ffff0000 \
        . . ffff0000 ffff0000 ffff0000 . . ffff0000 ffff0000 ffff0000 \
        . . ffff0000 ffff0000 ffff0000 . . ffff0000 ffff0000 ffff0000 \
        . . ffff0000 ffff0000 ffff0000 . . ffff0000 ffff0000 ffff0000";
    // (Row 15.)
    const SHADOWED: &str = ". . . . . . . . . . \
        ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 ffff0000 \
        ff0000ff . . . . . . . . . \
        . . . . . . . . . .";
    // (Row 22.)
    const SHADOW_FUNCTION: &str = ". . . . 020000ff 030000ff 060000ff 0a0000ff 100000ff 170000ff \
        210000ff 2d0000ff 3a0000ff 480000ff 550000ff 610000ff 6b0000ff 730000ff 770000ff 770000ff \
        730000ff 6b0000ff 610000ff 550000ff 480000ff 3a0000ff 2d0000ff 210000ff 170000ff 100000ff \
        0a0000ff 060000ff 030000ff 020000ff . . . . . .";
    // (Row 20.)
    const CUT_AFTER: &str = ". . . . . . . 2f3100fa 465700f8 607200ec \
        709200db 79b100c6 7ecc00aa 80df0089 80ef0064 80f90040 80fd001c 80ff0000 80ff0000 80ff0000 \
        . . . . . . . . . . \
        . . . . . . . . . .";

    #[test]
    fn a_blur_comes_out_as_librsvg_blurs_it() {
        enable_filters();
        // Deviation of two pixels or more, by three box blurs: to the level, within the
        // square's region grown 10% each way. (librsvg reads -10% in `f32`, a hair over a
        // tenth, so its region includes the pixel that edge would otherwise stop at; so does
        // this one.)
        assert_eq!(row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="3"/></filter>{SQUARE}"#), 20), written(BLURRED));
        // Any deviation, at the same steps per pixel: 3,000 pixels spread the square so thin
        // almost nothing is left.
        let wide = row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="3000"/></filter>{SQUARE}"#), 20);
        assert!(wide.iter().all(|pixel| *pixel >> 24 <= 1));
        // In linear light unless the file says otherwise: red and green blurred together are
        // lighter between them than in sRGB.
        for (space, row) in [(r#" color-interpolation-filters="linearRGB""#, LINEAR), ("", LINEAR), (r#" style="color-interpolation-filters:sRGB""#, SCREEN)] {
            let head = format!(r#"<filter id="e" x="0" y="0" width="1" height="1"{space}><feGaussianBlur stdDeviation="4"/></filter>"#);
            assert_eq!(row_of(&format!("{head}{HALVES}"), 20), written(row), "{space}");
        }
        // Deviation under two pixels, by the Gaussian itself: each pixel takes from each
        // neighbour the integral of the curve over it. (librsvg samples a pixel further out:
        // its small blurs come out narrower.)
        let blurred = row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="1"/></filter>{SQUARE}"#), 20);
        let bell = |from: f32, to: f32| (0..10_000).map(|n| from + (to - from) * (n as f32 + 0.5) / 10_000.0).map(|x| (-x * x / 2.0).exp()).sum::<f32>() * (to - from) / 10_000.0;
        let whole = bell(-3.5, 3.5);
        for (x, out) in [(7, 2.5), (8, 1.5), (9, 0.5), (10, -0.5)] {
            let wanted = (bell(out, 3.5) / whole * 255.0).round() as u32;
            assert!((blurred[x] >> 24).abs_diff(wanted) <= 1, "{x}: {} for {wanted}", blurred[x] >> 24);
        }
        // Zero or negative deviation passes the input through; so does a one-axis zero for that
        // axis.
        for nothing in ["0", "-1", "0 0"] {
            let text = format!(r#"<filter id="e"><feGaussianBlur stdDeviation="{nothing}"/></filter>{SQUARE}"#);
            assert_eq!(row_of(&text, 20), row_of(SQUARE.split(" filter").next().unwrap(), 20), "{nothing}");
        }
        let down_only = row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="0 3"/></filter>{SQUARE}"#), 10);
        assert!(down_only[10..30].iter().all(|pixel| *pixel == down_only[10]) && down_only[9] == 0 && down_only[10] >> 24 < 255);
    }

    #[test]
    fn a_filter_draws_only_within_its_region() {
        enable_filters();
        // In bounding-box fractions: its start and width in pixels, as librsvg has them. (Where
        // an edge read in `f32` falls a hair before a pixel edge, that pixel is in.)
        for (x, width, first, across) in [("-0.1", "1.2", 7, 26), ("0", "1.2", 10, 25), ("-0.05", "1.1", 8, 24), ("0.25", "0.5", 15, 10), ("0", "1", 10, 20), ("-0.5", "2", 0, 40)] {
            let row = row_of(&format!(r#"<filter id="e" x="{x}" y="0" width="{width}" height="1"><feFlood flood-color="blue"/></filter>{SQUARE}"#), 20);
            let blue: Vec<usize> = (0..40).filter(|at| row[*at] == 0xff00_00ff).collect();
            assert_eq!((blue[0], blue.len()), (first, across), "x {x}, width {width}");
        }
        // In user units, extended to whole pixels, nothing outside; a step's subregion within
        // it.
        let user = r#"<filter id="e" filterUnits="userSpaceOnUse" x="10.4" y="10.6" width="10.2" height="5.5"><feFlood flood-color="red"/></filter><rect width="40" height="40" filter="url(#e)"/>"#;
        let (inside, below) = (row_of(user, 16), row_of(user, 17));
        assert!((0..40).all(|x| (inside[x] == RED) == (10..21).contains(&x)) && below.iter().all(|pixel| *pixel == 0));
        let part = row_of(&format!(r#"<filter id="e" x="0" y="0" width="1" height="1"><feFlood flood-color="red" x="15" width="5.5"/></filter>{SQUARE}"#), 20);
        assert!((0..40).all(|x| (part[x] == RED) == (15..21).contains(&x)), "{part:x?}");
        // A filter of no size, no steps, or none known draws nothing.
        for nothing in [r#"<filter id="e" width="0"><feFlood/></filter>"#, r#"<filter id="e"/>"#, r#"<filter id="e"><feSomething/></filter>"#] {
            assert!(row_of(&format!("{nothing}{SQUARE}"), 20).iter().all(|pixel| *pixel == 0), "{nothing}");
        }
    }

    #[test]
    fn steps_take_in_what_the_steps_before_them_made() {
        enable_filters();
        // Offset by fractions of a pixel: interpolated, clipped to its subregion as offset
        // (librsvg clips the same way, edges and all).
        let moved = format!(r#"<filter id="e" x="0" y="0" width="1" height="1"><feOffset dx="2.5" dy="0.5"/></filter>{SQUARE}"#);
        assert_eq!(row_of(&moved, 10), written(MOVED));
        // A shadow, blurred, offset and coloured, under its source; as a step and as the CSS
        // function.
        let shadow = r#"<filter id="e"><feDropShadow dx="4" dy="4" stdDeviation="0" flood-color="blue"/></filter><rect x="10" y="10" width="10" height="10" fill="red" filter="url(#e)"/>"#;
        assert_eq!(row_of(shadow, 15), written(SHADOWED));
        assert_eq!(row_of(r#"<g style="filter:drop-shadow(4px 4px 4px blue)"><rect x="10" y="10" width="10" height="10" fill="red"/></g>"#, 22), written(SHADOW_FUNCTION));
        // Step inputs: by name, else the previous result (also for a name nothing made);
        // `SourceAlpha` in black; and background inputs, not read, so clear.
        let steps = |inside: &str| row_of(&format!(r#"<filter id="e" x="0" y="0" width="1" height="1">{inside}</filter>{SQUARE}"#), 20)[20];
        assert_eq!(steps(r#"<feFlood flood-color="blue" result="a"/><feFlood flood-color="lime"/><feOffset in="a"/>"#), 0xff00_00ff);
        assert_eq!(steps(r#"<feFlood flood-color="blue" result="a"/><feFlood flood-color="lime"/><feOffset in="b"/>"#), 0xff00_ff00);
        assert_eq!(steps(r#"<feFlood flood-color="blue"/><feOffset in="SourceAlpha"/>"#), 0xff00_0000);
        assert_eq!(steps(r#"<feOffset in="BackgroundImage"/>"#), 0);
        // Merged (in linear light, so half red over blue is lighter than in sRGB); and
        // composited with each operator, of a half-opaque colour over nothing and over an
        // opaque one. (librsvg reads `lighter` as `over`.)
        assert_eq!(
            steps(r#"<feFlood flood-color="blue" result="a"/><feFlood flood-color="red" flood-opacity=".5" result="b"/><feMerge><feMergeNode in="a"/><feMergeNode in="b"/></feMerge>"#),
            0xffbc_00bb
        );
        let laid = |operator: &str| {
            let floods = r#"<feFlood flood-color="blue" flood-opacity=".5" result="a"/><feFlood flood-color="red" x="20" result="b"/>"#;
            let row = row_of(&format!(r#"<filter id="e" x="0" y="0" width="1" height="1">{floods}<feComposite in="a" in2="b" {operator}/></filter>{SQUARE}"#), 20);
            [row[15], row[25]]
        };
        assert_eq!(laid(r#"operator="over""#), [0x8000_00ff, 0xffbb_00bc]);
        assert_eq!(laid(r#"operator="in""#), [0, 0x8000_00ff]);
        assert_eq!(laid(r#"operator="out""#), [0x8000_00ff, 0]);
        assert_eq!(laid(r#"operator="atop""#), [0, 0xffbb_00bc]);
        assert_eq!(laid(r#"operator="xor""#), [0x8000_00ff, 0x7fff_0000]);
        assert_eq!(laid(r#"operator="lighter""#), [0x8000_00ff, 0xffff_00bc]);
        assert_eq!(laid(r#"operator="arithmetic" k2=".5" k3=".5""#), [0x4000_00ff, 0xc0d5_009b]);
    }

    #[test]
    fn colours_are_mixed_and_worked_out_afresh() {
        enable_filters();
        let one =
            |inside: &str, fill: &str| row_of(&format!(r#"<filter id="e" color-interpolation-filters="sRGB">{inside}</filter><rect width="40" height="40" fill="{fill}" filter="url(#e)"/>"#), 20)[20];
        // Blended with a colour under it, each mode of Compositing and Blending, dark orange
        // over light blue (matching librsvg to the level, or one off).
        let blend = |mode: &str| one(&format!(r##"<feFlood flood-color="#8cf" result="b"/><feBlend in="SourceGraphic" in2="b" mode="{mode}"/>"##), "#c84");
        let modes = ["normal", "multiply", "screen", "darken", "lighten", "difference", "exclusion", "hue", "luminosity", "nonsense"];
        let mixed = [0xffcc8844, 0xff6d6d44, 0xffe7e7ff, 0xff888844, 0xffccccff, 0xff4444bb, 0xff7a7abb, 0xffedb276, 0xff60a4d7, 0xffcc8844];
        assert_eq!(modes.map(blend), mixed);
        // Saturate 3 (unclamped, as librsvg), hue rotate half a turn, luminance to alpha; a
        // `matrix` without twenty values is identity.
        assert_eq!(one(r#"<feColorMatrix type="saturate" values="3"/>"#, "#c84"), 0xffff7500);
        assert_eq!(one(r#"<feColorMatrix type="hueRotate" values="180"/>"#, "red"), 0xff00_6d6d);
        assert_eq!(one(r#"<feColorMatrix type="luminanceToAlpha"/>"#, "white"), 0xff00_0000);
        assert_eq!(one(r#"<feColorMatrix values="1 2 3"/>"#, "#c84"), 0xffcc_8844);
        // Where nothing is drawn a matrix can still draw: a clear pixel is transparent black.
        assert_eq!(one(r#"<feColorMatrix values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 0 1"/>"#, "none"), 0);
        // Each channel by a transfer function: discrete, table, linear, gamma.
        let transferred = one(
            r#"<feComponentTransfer><feFuncR type="discrete" tableValues="0 .5 1"/><feFuncG type="table" tableValues="1 0"/><feFuncB type="linear" slope="2" intercept=".1"/><feFuncA type="gamma" amplitude=".5" exponent="2" offset=".25"/></feComponentTransfer>"#,
            "#c84",
        );
        assert_eq!(transferred, 0xbfff_77a2);
    }

    #[test]
    fn thicker_thinner_tiles_and_noise() {
        enable_filters();
        // Dilate and erode as librsvg makes them; each costs the same per pixel at any radius.
        let morph = r#"<filter id="e"><feMorphology operator="dilate" radius="2"/></filter><filter id="f"><feMorphology radius="2"/></filter><rect x="5" y="5" width="12" height="12" fill="red" filter="url(#e)"/><rect x="22" y="22" width="12" height="12" fill="red" filter="url(#f)"/>"#;
        assert_eq!(row_of(morph, 11), written(THICKER));
        assert_eq!((0..40).filter(|x| row_of(morph, 28)[*x] == RED).collect::<Vec<usize>>(), (24..32).collect::<Vec<usize>>());
        let far = r#"<filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feMorphology operator="dilate" radius="1000"/></filter><rect x="19" y="19" width="2" height="2" fill="red" filter="url(#e)"/>"#;
        assert!(row_of(far, 0).iter().all(|pixel| *pixel == RED));
        // A subregion tiled over the rest.
        let tiles = r#"<filter id="e" x="0" y="0" width="1" height="1"><feFlood flood-color="red" x="0" y="0" width="5" height="5"/><feOffset dx="2" dy="2"/><feTile/></filter><rect width="40" height="40" filter="url(#e)"/>"#;
        assert_eq!(row_of(tiles, 3), written(TILED));
        // Turbulence as the specification computes it, to a level or two.
        let noise = r#"<filter id="e" x="0" y="0" width="1" height="1"><feTurbulence baseFrequency="0.1" numOctaves="2" seed="3"/></filter><rect width="40" height="40" filter="url(#e)"/>"#;
        assert!(near(&row_of(noise, 0), &written(NOISE), 2));
    }

    #[test]
    fn what_goes_through_a_filter_and_what_does_not() {
        enable_filters();
        let plain = row_of(&SQUARE.replace(r#" filter="url(#e)""#, ""), 20);
        // A missing or non-filter reference leaves the element unfiltered, with the whole list;
        // as does one inside a clip path.
        for none in ["", r#"<rect id="e" width="1" height="1"/>"#, r#"<filter id="f"><feFlood/></filter>"#] {
            assert_eq!(row_of(&format!("{none}{SQUARE}"), 20), plain, "{none}");
        }
        let listed = SQUARE.replace("url(#e)", "url(#f) url(#e)");
        assert_eq!(row_of(&format!(r#"<filter id="f"><feFlood/></filter>{listed}"#), 20), plain);
        let clipped = r#"<filter id="e"><feFlood flood-color="blue"/></filter><clipPath id="c"><rect width="40" height="40" filter="url(#e)"/></clipPath><rect x="10" y="10" width="20" height="20" fill="red" clip-path="url(#c)"/>"#;
        assert_eq!(row_of(clipped, 20), plain);
        // Each in a list in turn: offset across, then down.
        let both = r#"<filter id="a"><feOffset dx="5"/></filter><filter id="b"><feOffset dy="5"/></filter><rect x="10" y="10" width="10" height="10" fill="red" filter="url(#a) url(#b)"/>"#;
        assert_eq!((0..40).filter(|x| row_of(both, 16)[*x] == RED).collect::<Vec<usize>>(), (15..21).collect::<Vec<usize>>());
        assert!(row_of(both, 14).iter().all(|pixel| *pixel == 0));
        // A group filtered as one: its shapes blurred together.
        let group = r#"<filter id="e"><feGaussianBlur stdDeviation="2"/></filter><g filter="url(#e)"><rect x="10" y="10" width="10" height="20" fill="red"/><rect x="20" y="10" width="10" height="20" fill="red"/></g>"#;
        assert_eq!(row_of(group, 20), row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="2"/></filter>{SQUARE}"#), 20));
        // Filtered first, then clipped, then opacity, as librsvg does.
        let order = r#"<filter id="e"><feGaussianBlur stdDeviation="2"/></filter><rect x="10" y="10" width="20" height="20" fill="red" stroke="blue" stroke-width="4" style="filter:url(#e)" clip-path="url(#c)" opacity="0.5"/><clipPath id="c"><rect width="20" height="40"/></clipPath>"#;
        let have = row_of(order, 20);
        assert!(near(&have, &written(CUT_AFTER), 2), "{have:x?}");
        // A box aligned with the picture clips after the blur: inside, the blur is untouched.
        let blurred = row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="3"/></filter>{SQUARE}"#), 20);
        let boxed = row_of(&format!(r#"<filter id="e"><feGaussianBlur stdDeviation="3"/></filter><clipPath id="c"><rect width="20" height="40"/></clipPath><g clip-path="url(#c)">{SQUARE}</g>"#), 20);
        assert!(boxed[..20] == blurred[..20] && boxed[20..].iter().all(|pixel| *pixel == 0));
        // CSS filter functions: grayscale, invert, half opacity.
        let function = |name: &str| row_of(&format!(r#"<rect width="40" height="40" fill="red" style="filter:{name}"/>"#), 20)[20];
        let grey = function("grayscale(1)");
        assert!(grey >> 16 & 255 == grey >> 8 & 255 && grey >> 8 & 255 == grey & 255 && grey >> 24 == 255);
        assert_eq!([function("invert(100%)"), function("opacity(.5)"), function("unknown(1)")], [0xff00_ffff, 0x80ff_0000, RED]);
    }

    #[test]
    fn filters_of_numbers_beyond_all_measure_stop_nothing() {
        enable_filters();
        // Every step with sizes, positions, deviations, frequencies and matrices no real
        // drawing has: each read and drawn quickly. (Stitched turbulence at frequency 1e40 once
        // overflowed.)
        let wild = ["1e40", "-1e38", "1e-38", "NaN", "inf", "0", "-0", "1e20", "0.0000001", "16777217"];
        for (a, b) in wild.iter().flat_map(|a| wild.iter().map(move |b| (*a, *b))) {
            let text = format!(
                r##"<svg viewBox="0 0 100 100"><filter id="f" x="{a}" width="{b}" primitiveUnits="objectBoundingBox"><feGaussianBlur stdDeviation="{a} {b}" result="r"/><feOffset dx="{a}" dy="{b}"/>
                <feTurbulence baseFrequency="{a} {b}" numOctaves="{b}" seed="{a}" stitchTiles="stitch" x="{b}" width="{a}"/><feTile/><feMorphology radius="{a} {b}" operator="dilate"/>
                <feDropShadow dx="{b}" stdDeviation="{a}"/><feComposite in2="r" operator="arithmetic" k1="{a}" k2="{b}" k3="{a}" k4="{b}"/><feColorMatrix type="hueRotate" values="{a}"/>
                <feComponentTransfer><feFuncR type="gamma" exponent="{a}" amplitude="{b}"/><feFuncA type="table" tableValues="{a} {b}"/></feComponentTransfer><feFlood flood-opacity="{b}" height="{a}"/></filter>
                <rect x="{a}" width="{b}" height="50" filter="url(#f)" transform="scale({a} {b})"/><circle r="40" cx="50" cy="50" style="filter:blur({a}) drop-shadow({a} {b} {a}) hue-rotate({b}turn)"/></svg>"##
            );
            assert!(decode(&text, 24, 24).is_some(), "{a} and {b}");
        }
        let found = [
            r#"<svg><filter id="a"><feTurbulence baseFrequency="1e40" stitchTiles="stitch"/></filter><circle r="5" filter="url(#a)"/></svg>"#,
            r#"<svg><text filter="url(f)">0<filter id="f"><feTurbulence baseFrequency=".7e30" stitchTiles="stitch"/></filter></text></svg>"#,
        ];
        assert!(found.iter().all(|found| decode(found, 24, 24).is_some()));
    }

    #[test]
    fn the_glyphs_of_fonts_are_drawn_without_filters() {
        // Filters are read once a program asks.
        enable_filters();
        let font = r##"<svg><filter id="f"><feGaussianBlur stdDeviation="9"/></filter><path id="glyph3" d="M0 0h100v-100h-100z" filter="url(#f)"/></svg>"##;
        let (glyph, file) = (ParsedDrawing::glyph(font, "glyph3", 100.0).unwrap(), ParsedDrawing::read(font).unwrap());
        assert!(glyph.held().is_none() && file.held().is_some_and(|more| more.filters.len() == 1));
    }

    #[test]
    fn what_a_filter_costs_is_counted() {
        enable_filters();
        let cost = |shapes: &str| Reading::of(&format!(r#"<svg viewBox="0 0 100 100">{shapes}</svg>"#), None).unwrap().passes;
        // Each step costs the filter region's share times its weight (blur 18, flood 2), plus
        // four for copying the region out and back; the element and its block cost as before.
        let blurred =
            r#"<filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="50" height="100"><feGaussianBlur stdDeviation="3"/><feFlood/></filter><rect width="100" height="100" filter="url(#e)"/>"#;
        assert!((cost(blurred) - (1.0 + 1.0 + 0.5 * (4.0 + 18.0 + 2.0))).abs() < 1e-3, "{}", cost(blurred));
        // A few hundred bytes that, uncounted, would blur the whole picture forty times in each
        // of 64 groups: 2,500 times. Counted, two of the first group's blurs exhaust the
        // budget: the rest of its filters, and the later groups', are skipped (those groups
        // drawn unfiltered while there is room).
        let mut text = String::from(r#"<svg viewBox="0 0 100 100"><defs><g id="a0" style="filter:"#);
        text += &"blur(9) ".repeat(40);
        text += r#""><rect width="100" height="100"/></g>"#;
        for level in 1..=3 {
            text += &format!(r##"<g id="a{level}">{}</g>"##, format!(r##"<use href="#a{}"/>"##, level - 1).repeat(4));
        }
        text += r##"</defs><use href="#a3"/></svg>"##;
        let reading = Reading::of(&text, None).unwrap();
        let filters = reading.parsed.held().map_or(&[][..], |more| &more.filters[..]);
        let steps: usize = filters.iter().map(|filter| filter.steps.len()).sum();
        assert!(text.len() < 700 && steps == 2 && filters.len() == 2, "{} bytes, {steps} steps of {} filters", text.len(), filters.len());
        assert!(reading.passes <= passes(text.len()) + 3.0, "{} passes", reading.passes);
    }

    // Rows of what librsvg 2.62 draws of this round's test files (row 12 of a point light,
    // row 20 of the rest), and what Edge draws for lighting colour.
    const POINT_LIT: &str = "ffe0e0e0 ffdbdbdb ffc4c4c4 ffa1a1a1 ff848484 ff7c7c7c ff868686 ffa0a0a0 ffc4c4c4 ffe3e3e3 \
        fff5f5f5 fff8f8f8 fff6f6f6 fff3f3f3 fff0f0f0 ffededed ffe9e9e9 ffe5e5e5 ffe0e0e0 ffdcdcdc \
        ffd7d7d7 ffd3d3d3 ffcecece ffcacaca ffc6c6c6 ffc2c2c2 ffbebebe ffb9b9b9 ffa8a8a8 ff7c7c7c \
        ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff2e2e2e ff8c8c8c ff9f9f9f";
    const SPOT_LIT: &str = ". . . . . . . 7bffffff aaffffff aaffffff \
        a9ffffff a8ffffff a7ffffff a4ffffff a2ffffff 9fffffff 9bffffff 97ffffff 93ffffff 8fffffff \
        8bffffff 87ffffff 82ffffff 7effffff 7affffff 75ffffff 71ffffff 6dffffff 69ffffff 65ffffff \
        61ffffff 5effffff 0dffffff . . 5effffff 5bffffff 58ffffff 55ffffff 52ffffff";
    const CONVOLVED: &str = ". . . . . 33787878 33787878 33787878 33787878 33787878 \
        33787878 33787878 33787878 33787878 f534cb66 ff33cc66 ff33cc66 ff33cc66 ff33cc66 ff33cc66 \
        ff33cc66 ff33cc66 ff33cc66 ff33cc66 ff33cc66 f433cb65 33787878 33787878 33787878 33787878 \
        33787878 33787878 33787878 33787878 33787878 . . . . .";
    const DISPLACED: &str = "ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee \
        ffd4e4d4 ff6cbf6c ff009900 ff009900 ff009900 ff009900 ff009900 ff009900 ff009900 ff009900 \
        ff009900 ff009900 ff009900 ff009900 ff009900 ff009900 ff009900 ff35ac35 ffbcdbbc ffeeeeee \
        ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee ffeeeeee abededed";
    const NESTED: &str = ". . . . . . . . ffcc3300 ffcc3300 \
        ffcc3300 ffcc3300 ffcc3300 ffcc3300 ff453387 ff0033cc ff0033cc ff0033cc ff0033cc ff0033cc \
        ff0033cc ff0033cc ff0033cc ff0033cc ff0633c6 ff3d338f ff883344 ffc83304 ffcc3300 ffcc3300 \
        ffcc3300 ffcc3300 . . . . . . . .";

    #[test]
    fn light_falls_on_the_surface_alpha_makes_as_librsvg_lights_it() {
        enable_filters();
        let point = r#"<filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feGaussianBlur in="SourceAlpha" stdDeviation="2"/><feDiffuseLighting surfaceScale="5"><fePointLight x="10" y="8" z="15"/></feDiffuseLighting></filter><rect x="6" y="6" width="28" height="28" rx="6" filter="url(#e)"/>"#;
        assert!(near(&row_of(point, 12), &written(POINT_LIT), 3));
        // A spot light's cone and fall-off; on the circle's own edge, its normals follow how
        // the edge is smoothed.
        let spot = r#"<filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feSpecularLighting surfaceScale="3" specularConstant="1.5" specularExponent="12"><feSpotLight x="0" y="0" z="30" pointsAtX="25" pointsAtY="25" specularExponent="2" limitingConeAngle="25"/></feSpecularLighting></filter><circle cx="20" cy="20" r="14" filter="url(#e)"/>"#;
        assert!(near(&row_of(spot, 20)[8..32], &written(SPOT_LIT)[8..32], 1));
        // A specular exponent outside 1 to 128 makes no step.
        let wrong =
            r#"<filter id="e"><feFlood flood-color="red"/><feSpecularLighting specularExponent="200"><feDistantLight/></feSpecularLighting></filter><rect width="40" height="40" filter="url(#e)"/>"#;
        assert_eq!(row_of(wrong, 20)[20], RED);
    }

    #[test]
    fn the_light_colour_is_taken_into_linear_light_as_browsers_take_it() {
        enable_filters();
        // A flat surface lit from 35 degrees up: Edge's colour, to the level. (librsvg uses the
        // colour's sRGB values as linear ones: paler.)
        let lit = r##"<filter id="e" x="0" y="0" width="1" height="1"><feDiffuseLighting in="SourceAlpha" surfaceScale="4" diffuseConstant="1.2" lighting-color="#fc8"><feDistantLight azimuth="45" elevation="35"/></feDiffuseLighting></filter><rect width="40" height="40" filter="url(#e)"/>"##;
        assert_eq!(row_of(lit, 20)[20], 0xffd8_ad72);
    }

    #[test]
    fn a_kernel_convolves_as_librsvg_convolves() {
        enable_filters();
        // A kernel wider than high, its target off centre, wrapping at the edges, with a divisor
        // and a bias (added to colours as much as they are solid).
        let kernel = r##"<filter id="e" x="0" y="0" width="1" height="1"><feConvolveMatrix order="5 3" kernelMatrix="1 2 3 2 1 0 0 0 0 0 -1 -2 -3 -2 -1" divisor="4" bias=".2" targetX="1" targetY="2" edgeMode="wrap"/></filter><rect x="5" y="5" width="30" height="30" fill="#c63" filter="url(#e)"/><circle cx="20" cy="20" r="6" fill="#3c6"/>"##;
        let row = row_of(kernel, 20);
        assert!(near(&row, &written(CONVOLVED), 3), "{row:x?}");
        // Alpha kept: a box blur of the colours only (in linear light), nothing past the edges.
        // (As librsvg draws it, to two levels.)
        let kept = r##"<filter id="e" x="0" y="0" width="1" height="1"><feConvolveMatrix kernelMatrix="1 1 1 1 1 1 1 1 1" edgeMode="none" preserveAlpha="true"/></filter><rect width="40" height="40" fill="#08f" fill-opacity=".5" filter="url(#e)"/>"##;
        assert_eq!([row_of(kept, 20)[20], row_of(kept, 20)[0]], [0x7f00_89ff, 0x7f00_70d7]);
        // A kernel of the wrong size, or a target outside it, makes no step.
        for wrong in [r#"kernelMatrix="1 1 1""#, r#"kernelMatrix="1 1 1 1" order="2" targetX="2""#] {
            let text = format!(r#"<filter id="e"><feFlood flood-color="red"/><feConvolveMatrix {wrong}/></filter><rect width="40" height="40" filter="url(#e)"/>"#);
            assert_eq!(row_of(&text, 20)[20], RED, "{wrong}");
        }
    }

    // Rows 20 of what librsvg 2.62 draws of the same files (`rsvg-convert` on Garuda, 40 pixels).
    const UNIT_CONVOLVED: &str = ". . . . . 78008800 68008900 50008900 30008a00 1a3b803b 0f886688 14cc00cc 2bca00ca 4bc900c9 \
        75cb00cb 9dcb00cb c3cc00cc dfcc00cc f0cc00cc f9cc00cc f9cc00cc f0cc00cc dfcc00cc c3cc00cc 9dcb00cb 74cc00cc 4acb00cb \
        2ace00ce 14cc00cc 0e926d92 1a3b803b 30008a00 50008900 67008800 77008700 . . . . .";
    const UNIT_LIT: &str = ". . . . . . a9565656 ff565656 ff737373 ff8a8a8a ff9e9e9e ffb9b9b9 ffd0d0d0 ffe4e4e4 ffe7e7e7 \
        ffeaeaea ffededed ffebebeb ffe9e9e9 ffe6e6e6 ffe3e3e3 ffe0e0e0 ffdcdcdc ffcecece ffbebebe ffadadad ff919191 ff696969 \
        ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 . . . . . .";
    const FILL_PAINT: &str = ". . f3fb0000 fff4000d ffed0016 ffe60016 ffdf0022 ffd80026 ffd1002e ffca0035 ffc3003d ffbc0042 \
        ffb50049 ffae0051 ffa60058 ff9f0060 ff980068 ff91006e ff8a0075 ff83007c ff7c0083 f175008a f26e0091 ff680098 ff60009f \
        ff5800a6 ff5100ae ff4900b5 ff4200bc ff3d00c3 ff3500ca ff2e00d1 ff2600d8 ff2200df ff1600e6 ff1600ed ff0d00f4 f10000fb . .";
    const STROKE_PAINT: &str = ". . . . . . . 8000a900 8000a900 c000a900 ff005500 ff000000 ff000000 ff000000 ff000000 ff000000 \
        ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 ff000000 \
        ff005500 c000a900 8000a900 8000a900 . . . . . . .";

    #[test]
    fn a_kernel_unit_runs_convolution_and_lighting_on_a_coarser_grid_as_librsvg_does() {
        enable_filters();
        let convolved = r##"<filter id="e" x="0" y="0" width="1" height="1"><feConvolveMatrix order="3" kernelMatrix="1 2 1 2 4 2 1 2 1" kernelUnitLength="2 4"/></filter><g filter="url(#e)"><rect x="5" y="5" width="30" height="30" fill="none" stroke="#080" stroke-width="4"/><path d="M10 32 L20 8 L30 32 Z" fill="#c0c"/></g>"##;
        let row = row_of(convolved, 20);
        assert!(near(&row, &written(UNIT_CONVOLVED), 3), "{row:x?}");
        let lit = r##"<filter id="e" x="0" y="0" width="1" height="1"><feGaussianBlur in="SourceAlpha" stdDeviation="3"/><feDiffuseLighting surfaceScale="4" kernelUnitLength="3" lighting-color="white"><fePointLight x="10" y="10" z="20"/></feDiffuseLighting></filter><circle cx="20" cy="20" r="14" filter="url(#e)"/>"##;
        let row = row_of(lit, 20);
        assert!(near(&row, &written(UNIT_LIT), 3), "{row:x?}");
        // A unit of one pixel, or none, or one of no size: as without.
        for unit in [r#"kernelUnitLength="1""#, r#"kernelUnitLength="0 2""#, r#"kernelUnitLength="-1""#] {
            assert_eq!(row_of(&convolved.replace(r#"kernelUnitLength="2 4""#, unit), 20), row_of(&convolved.replace(r#" kernelUnitLength="2 4""#, ""), 20), "{unit}");
        }
    }

    #[test]
    fn paint_inputs_fill_the_filter_region_with_the_element_s_paint_as_librsvg_does() {
        enable_filters();
        // A gradient in units of the group's box, kept where the group is.
        let filled = r##"<linearGradient id="g"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient><filter id="e" x="0" y="0" width="1" height="1"><feComposite in="FillPaint" in2="SourceAlpha" operator="in"/></filter><g fill="url(#g)" filter="url(#e)"><circle cx="12" cy="20" r="10"/><circle cx="30" cy="20" r="8"/></g>"##;
        let row = row_of(filled, 20);
        assert!(near(&row, &written(FILL_PAINT), 10), "{row:x?}");
        // A colour at its opacity, over the whole region (10% more than the box each way).
        let stroked = r##"<filter id="e"><feMerge><feMergeNode in="StrokePaint"/><feMergeNode in="SourceGraphic"/></feMerge></filter><rect x="10" y="10" width="20" height="20" fill="#000" stroke="#0a0" stroke-opacity="0.5" stroke-width="2" filter="url(#e)"/>"##;
        let row = row_of(stroked, 20);
        assert!(near(&row, &written(STROKE_PAINT), 2), "{row:x?}");
        // No paint, nothing.
        let unpainted = stroked.replace(r##" stroke="#0a0""##, "");
        assert_eq!(row_of(&unpainted, 20)[7], 0);
    }

    #[test]
    fn a_map_moves_pixels_as_librsvg_moves_them() {
        enable_filters();
        // Moved by the map's blue across and alpha down, blended between pixels.
        let moved = r##"<filter id="e" x="0" y="0" width="1" height="1" color-interpolation-filters="sRGB"><feFlood flood-color="#c48" result="m"/><feDisplacementMap in="SourceGraphic" in2="m" scale="10" xChannelSelector="B" yChannelSelector="A"/></filter><g filter="url(#e)"><rect width="40" height="40" fill="#eee"/><circle cx="20" cy="20" r="10" fill="#090"/></g>"##;
        assert!(near(&row_of(moved, 20), &written(DISPLACED), 6));
    }

    #[test]
    fn an_image_step_draws_an_element_or_a_picture_in_its_place() {
        enable_filters();
        enable_images();
        let shape = r##"<path id="p" d="M5 5L35 20L5 35z" fill="#e60"/>"##;
        let plain = row_of(shape, 20);
        // An element, in the units of what is filtered: as it is drawn by itself.
        let element = format!(
            r##"<defs>{shape}</defs><filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feImage href="#p"/></filter><rect width="1" height="1" filter="url(#e)"/>"##
        );
        assert!(near(&row_of(&element, 20), &plain, 1));
        // In bounding-box units, fitted to the box of what is filtered (here half the size).
        let boxed = r##"<defs><path id="q" d="M0 0H1V1H0z" fill="#e60"/></defs><filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40" primitiveUnits="objectBoundingBox"><feImage href="#q"/></filter><rect x="10" y="10" width="20" height="20" filter="url(#e)"/>"##;
        let row = row_of(boxed, 20);
        assert!((0..40).all(|x| (row[x] == 0xffee_6600) == (10..30).contains(&x)));
        // A picture in its subregion: an SVG one, stretched.
        let picture = r##"<filter id="e" x="0" y="0" width="1" height="1"><feImage preserveAspectRatio="none" x="10" width="20" href="data:image/svg+xml;utf8,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 4 4%22%3E%3Crect width=%224%22 height=%224%22 fill=%22%23a0f%22/%3E%3C/svg%3E"/></filter><rect width="40" height="40" filter="url(#e)"/>"##;
        let row = row_of(picture, 20);
        assert!((0..40).all(|x| (row[x] == 0xffaa_00ff) == (10..30).contains(&x)), "{row:x?}");
    }

    #[test]
    fn a_background_input_takes_what_was_drawn_under_the_element() {
        enable_filters();
        // Blue on the left half, drawn first; then an element whose filter moves what lies
        // under it across by ten, and takes its alpha in black.
        let under = |input: &str| {
            row_of(
                &format!(
                    r#"<rect width="20" height="40" fill="blue"/><filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feOffset in="{input}" dx="10"/></filter><rect width="40" height="40" fill="none" filter="url(#e)"/>"#
                ),
                20,
            )
        };
        let moved = under("BackgroundImage");
        assert!((0..40).all(|x| (moved[x] == 0xff00_00ff) == (0..30).contains(&x)), "{moved:x?}");
        let alpha = under("BackgroundAlpha");
        assert!((10..30).all(|x| alpha[x] == 0xff00_0000) && alpha[5] == 0xff00_00ff && alpha[35] == 0);
    }

    #[test]
    fn crisp_shapes_cover_whole_pixels_where_their_middles_lie() {
        enable_shape_rendering();
        let crisp =
            drawn(40, 40, r#"<circle cx="12.3" cy="20" r="8.3" fill="red" shape-rendering="crispEdges"/><path d="M24 4L37 36" stroke="red" stroke-width="2.6" shape-rendering="optimizeSpeed"/>"#);
        assert!(crisp.iter().all(|pixel| *pixel == 0 || *pixel == RED));
        // The circle's pixels are those whose middles it holds (but where a curve leaves a pixel
        // all but half covered).
        let inside = |at: usize| ((at % 40) as f32 + 0.5 - 12.3).hypot((at / 40) as f32 + 0.5 - 20.0) < 8.3;
        let missed = (0..1600).filter(|at| at % 40 < 22 && (crisp[*at] == RED) != inside(*at)).count();
        assert!(missed <= 2, "{missed}");
        let smooth = drawn(40, 40, r#"<circle cx="12.3" cy="20" r="8.3" fill="red" shape-rendering="geometricPrecision"/>"#);
        assert!(smooth.iter().any(|pixel| (1..255).contains(&(pixel >> 24))));
    }

    #[test]
    fn abutting_shapes_of_a_clip_path_or_mask_leave_no_seam() {
        enable_seamless_clips();
        // Two rectangles meeting inside a pixel, as Edge draws them: whole there. (librsvg
        // leaves a seam of three quarters.)
        let clipped = row_of(
            r#"<clipPath id="c"><rect x="5" y="5" width="15.5" height="30"/><rect x="20.5" y="5" width="14.5" height="30"/></clipPath><rect width="40" height="40" fill="red" clip-path="url(#c)"/>"#,
            20,
        );
        assert!((5..35).all(|x| clipped[x] == RED), "{clipped:x?}");
        // Two triangles going round opposite ways, meeting along a slant.
        let slant = drawn(40, 40, r#"<clipPath id="c"><path d="M4 4L36 4L4 36z"/><path d="M36 4L4 36L36 36z"/></clipPath><rect width="40" height="40" fill="red" clip-path="url(#c)"/>"#);
        assert!((4..36).all(|y| (4..36).all(|x| slant[y * 40 + x] == RED)));
        let masked = row_of(
            r##"<mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><path d="M6 6h14.3v28h-14.3z" fill="#fff"/><path d="M20.3 6h13.7v28h-13.7z" fill="#fff"/></mask><rect width="40" height="40" fill="red" mask="url(#m)"/>"##,
            20,
        );
        assert!((6..34).all(|x| masked[x] == RED), "{masked:x?}");
    }

    /// An SVG file as a `data:` URL, its view 4 units square.
    fn svg_url(inside: &str) -> String {
        let file = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4">{inside}</svg>"#);
        format!("data:image/svg+xml,{}", file.replace('%', "%25").replace('"', "%22").replace('<', "%3C").replace('>', "%3E").replace('#', "%23"))
    }

    #[test]
    fn a_drawing_brought_into_a_mask_whose_fills_merge_stays_in_the_mask() {
        enable_images();
        enable_seamless_clips();
        // Two fills that become one, then an image whose drawing is read after the walk.
        let white = svg_url(r#"<rect width="4" height="4" fill="white"/>"#);
        let text = format!(
            r#"<mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><rect width="10" height="40" fill="white"/><rect x="10" width="10" height="40" fill="white"/><image x="20" width="20" height="40" preserveAspectRatio="none" href="{white}"/></mask><rect width="40" height="40" fill="red" mask="url(#m)"/>"#
        );
        let row = row_of(&text, 20);
        assert!(row.iter().all(|pixel| *pixel == RED), "{row:x?}");
    }

    #[test]
    fn an_image_read_again_is_its_own_file_whatever_other_files_hold() {
        enable_filters();
        enable_images();
        // The image (element 2) holds a red file whose own element 2 is an image of a blue one;
        // a filter draws the group holding the image again, after that file was read.
        let blue = svg_url(r#"<rect width="4" height="4" fill="blue"/>"#);
        let red = svg_url(&format!(r#"<rect width="4" height="4" fill="red"/><image width="0" height="0" href="{blue}"/>"#));
        let text = format!(
            r##"<g id="g"><image width="20" height="40" preserveAspectRatio="none" href="{red}"/></g><filter id="e" filterUnits="userSpaceOnUse" x="0" y="0" width="40" height="40"><feImage href="#g"/></filter><rect width="40" height="40" filter="url(#e)"/>"##
        );
        let row = row_of(&text, 20);
        assert!((0..20).all(|x| row[x] == RED), "{row:x?}");
    }

    #[test]
    fn an_svg_image_is_drawn_sharp_in_its_box_as_librsvg_draws_it() {
        enable_images();
        // Its view fitted to the box and clipped there, as a picture of its own size would be.
        let nested = r#"<image x="4" y="6" width="32" height="24" href="data:image/svg+xml;utf8,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 10 10%22%3E%3Ccircle cx=%225%22 cy=%225%22 r=%226%22 fill=%22%23c30%22/%3E%3Crect x=%222%22 y=%222%22 width=%226%22 height=%226%22 fill=%22%2303c%22 transform=%22rotate(20 5 5)%22/%3E%3C/svg%3E"/>"#;
        let row = row_of(nested, 12);
        assert!(near(&row, &written(NESTED), 10), "{row:x?}");
    }

    #[test]
    fn images_in_other_files_come_from_the_app_s_loader() {
        enable_images_from(|href| (href == "loaded.svg").then(|| br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><rect width="2" height="2" fill="red"/></svg>"#.to_vec()));
        let row = row_of(r#"<image x="10" width="20" height="40" href="loaded.svg"/><image x="30" width="10" height="40" href="other.svg"/>"#, 20);
        assert!((0..40).all(|x| (row[x] == RED) == (10..30).contains(&x)), "{row:x?}");
    }

    #[test]
    fn drawings_brought_in_cost_from_the_file_s_budget() {
        enable_filters();
        enable_images();
        // A drawing holding itself as an image four times, and a filter whose image is what it
        // filters: each read only so deep, within what the file may cost.
        let mut image = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#);
        for _ in 0..8 {
            let escaped = image.replace('%', "%25").replace('"', "%22").replace('<', "%3C").replace('>', "%3E").replace('#', "%23");
            let one = format!(r#"<image width="10" height="10" href="data:image/svg+xml,{escaped}"/>"#);
            image = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">{}</svg>"#, one.repeat(4));
        }
        let looped = r##"<svg viewBox="0 0 40 40"><filter id="e"><feImage href="#r"/><feGaussianBlur stdDeviation="9"/></filter><rect id="r" width="40" height="40" filter="url(#e)"/></svg>"##;
        for text in [&image[..], looped] {
            let reading = Reading::of(text, None).unwrap();
            assert!(reading.passes <= passes(text.len()) + 3.0 && !reading.parsed.items.is_empty(), "{} passes of {} bytes", reading.passes, text.len());
            assert!(decode(text, 64, 64).is_some());
        }
    }

    #[test]
    fn new_filter_steps_of_numbers_beyond_all_measure_stop_nothing() {
        enable_filters();
        let wild = ["1e40", "-1e38", "1e-38", "NaN", "inf", "0", "-0", "1e20", "16777217", "3"];
        for (a, b) in wild.iter().flat_map(|a| wild.iter().map(move |b| (*a, *b))) {
            let text = format!(
                r##"<svg viewBox="0 0 100 100"><filter id="f" x="{a}" width="{b}" primitiveUnits="objectBoundingBox"><feDiffuseLighting surfaceScale="{a}" diffuseConstant="{b}"><fePointLight x="{a}" y="{b}" z="{a}"/></feDiffuseLighting>
                <feSpecularLighting specularExponent="{a}" surfaceScale="{b}"><feSpotLight x="{b}" pointsAtX="{a}" specularExponent="{b}" limitingConeAngle="{a}"/></feSpecularLighting><feDiffuseLighting><feDistantLight azimuth="{a}" elevation="{b}"/></feDiffuseLighting>
                <feConvolveMatrix order="{a} {b}" kernelMatrix="{a} {b} {a} {b}" divisor="{b}" bias="{a}" targetX="{b}"/><feConvolveMatrix kernelMatrix="{a} {b} {a} {b} {a} {b} {a} {b} {a}" bias="{b}" edgeMode="wrap"/>
                <feDisplacementMap in2="SourceGraphic" scale="{a}" xChannelSelector="R"/><feImage href="#r" x="{a}" width="{b}"/><feOffset in="BackgroundImage" dx="{a}"/></filter>
                <rect id="r" x="{a}" width="{b}" height="50" filter="url(#f)" transform="scale({a} {b})" shape-rendering="crispEdges"/><circle r="40" cx="50" cy="50" shape-rendering="crispEdges" stroke="red" stroke-width="{a}"/></svg>"##
            );
            assert!(decode(&text, 24, 24).is_some(), "{a} and {b}");
        }
    }

    #[test]
    fn kernel_units_and_paint_inputs_of_numbers_beyond_all_measure_stop_nothing() {
        enable_filters();
        let wild = ["1e40", "-1e38", "1e-38", "NaN", "inf", "0", "-0", "1e20", "16777217", "0.5"];
        for (a, b) in wild.iter().flat_map(|a| wild.iter().map(move |b| (*a, *b))) {
            let text = format!(
                r##"<svg viewBox="0 0 100 100"><linearGradient id="g" x2="{a}"><stop offset="{b}" stop-color="red"/><stop offset="1"/></linearGradient><filter id="f" x="{a}" width="{b}" primitiveUnits="objectBoundingBox"><feConvolveMatrix kernelMatrix="{a} 1 {b} 1 {a} 1 {b} 1 {a}" kernelUnitLength="{a} {b}" preserveAlpha="true"/>
                <feDiffuseLighting kernelUnitLength="{b} {a}" surfaceScale="{a}"><fePointLight x="{a}" y="{b}" z="{a}"/></feDiffuseLighting><feComposite in="FillPaint" in2="StrokePaint" operator="arithmetic" k1="{a}" k2="{b}"/></filter>
                <rect x="{a}" width="{b}" height="50" fill="url(#g)" stroke="#0a0" stroke-opacity="{a}" filter="url(#f)" transform="scale({a} {b})"/></svg>"##
            );
            assert!(decode(&text, 24, 24).is_some(), "{a} and {b}");
        }
        // A kernel value past what a number holds (found by fuzzing).
        let huge = r##"<svg viewBox="0 0 40 40"><filter id="e" x="0" y="0" width="1" height="1"><feConvolveMatrix order="3" kernelMatrix="1 2 1 2 340282366920938463463374607431768211456 2 1 2 1" kernelUnitLength="2 4"/></filter><rect x="5" y="5" width="30" height="30" fill="#080" filter="url(#e)"/></svg>"##;
        assert!(decode(huge, 40, 40).is_some());
    }

    #[test]
    fn text_follows_its_path_turned_with_it() {
        enable_text();
        // Down a vertical line: the letters stand in a narrow column about it; what runs past
        // its end is left out.
        let text = r##"<path id="p" d="M20 2V30"/><text font-family="sans-serif" font-size="8" fill="red"><textPath href="#p">IIIIIIIIIIIIIIIIIIIIIIIIIIIII</textPath></text>"##;
        let down = drawn(40, 40, text);
        let inked = |at: &usize| down[*at] >> 24 > 0x80;
        let (columns, rows): (Vec<usize>, Vec<usize>) = (0..1600).filter(inked).map(|at| (at % 40, at / 40)).unzip();
        assert!(!columns.is_empty() && columns.iter().all(|x| (12..=28).contains(x)), "{columns:?}");
        assert!(rows.iter().all(|y| (1..=31).contains(y)) && rows.iter().any(|y| *y < 6) && rows.iter().any(|y| *y > 26), "{rows:?}");
        // A text path naming no shape shows nothing.
        assert!(drawn(40, 40, r##"<text font-size="8"><textPath href="#none">II</textPath></text>"##).iter().all(|pixel| *pixel == 0));
    }

    #[cfg(feature = "shaping")]
    #[test]
    fn shaped_text_joins_its_letters() {
        enable_text();
        // Arabic seens joined into one stroke along the baseline: no gap between the first ink
        // and the last. (Where the system has no Arabic font, nothing is drawn to judge.)
        let joined = drawn(64, 64, r#"<text x="60" y="40" font-family="sans-serif" font-size="24" text-anchor="end">سسس</text>"#);
        let columns: Vec<usize> = (0..64).filter(|x| (0..64).any(|y| joined[y * 64 + x] >> 24 > 0x40)).collect();
        if let (Some(first), Some(last)) = (columns.first(), columns.last()) {
            assert_eq!(columns.len(), last - first + 1, "{columns:?}");
        }
    }

    /// A path's outline as SVG path data.
    #[cfg(all(target_os = "linux", feature = "shaping"))]
    fn path_data(path: &Path) -> String {
        use crate::path::Step;
        let point = |at: Point| format!("{} {}", at.x, at.y);
        let step = |step: &Step| match step {
            Step::Move(to) => format!("M{}", point(*to)),
            Step::Line(to) => format!("L{}", point(*to)),
            Step::Quad(by, to) => format!("Q{} {}", point(*by), point(*to)),
            Step::Cubic(a, b, to) => format!("C{} {} {}", point(*a), point(*b), point(*to)),
            Step::Close => "Z".to_string(),
        };
        path.steps().iter().map(step).collect()
    }

    /// A text's runs of one script, figures, signs and marks going with the letters before
    /// them.
    #[cfg(all(target_os = "linux", feature = "shaping"))]
    fn script_runs(text: &str) -> Vec<String> {
        let mut runs: Vec<(String, Option<crate::shaping::Script>)> = Vec::new();
        for letter in text.chars() {
            let script = crate::shaping::Script::of(letter);
            match runs.last_mut().filter(|(_, own)| script.is_none() || own.is_none() || *own == script) {
                Some((run, own)) => (run.push(letter), *own = own.or(script)).1,
                None => runs.push((letter.to_string(), script)),
            }
        }
        runs.into_iter().map(|(run, _)| run).collect()
    }

    #[cfg(all(target_os = "linux", feature = "shaping"))]
    #[test]
    fn text_comes_out_as_harfbuzz_shapes_it() {
        enable_text();
        // Words in families of a Linux desktop, where installed: Arabic with its vowels, Hindi,
        // Thai, Latin letters with marks the font places, ligatures.
        let words = [
            ("Noto Sans Arabic", "مَرْحَبًا بِالعَالَمِ", cfg!(feature = "right-to-left")),
            ("DejaVu Sans", "مرحبا بالعالم", cfg!(feature = "right-to-left")),
            ("Noto Sans Devanagari", "हिन्दी भाषा क्षत्रिय", cfg!(feature = "brahmic")),
            ("Noto Sans Thai", "ภาษาไทยน้ำ", cfg!(feature = "brahmic")),
            ("DejaVu Sans", "q\u{303}x\u{323}\u{301} o\u{328}\u{304}", true),
            ("Noto Sans", "office flow q\u{30c}\u{323}", true),
        ];
        let mut tried = 0;
        for (family, word, shaped) in words {
            if !shaped {
                continue;
            }
            let first = word.chars().find(|letter| crate::shaping::Script::of(*letter).is_some()).unwrap();
            let Some(path) = crate::x11::file_for(Face::named(family), first).filter(|path| path.to_string_lossy().contains(&family.replace(' ', ""))) else { continue };
            let Some(font) = FontFile::open(&path) else { continue };
            // (HarfBuzz shapes each script's letters on their own, with the signs after them.)
            let runs = script_runs(word);
            let Some(theirs) = crate::shaping::harfbuzz::harfbuzz_in(&path, 0, &runs, "") else { return eprintln!("no HarfBuzz here: not tested") };
            let (mut placed, mut pen) = (Vec::new(), 0);
            for (glyphs, total) in &theirs {
                placed.extend(glyphs.iter().map(|(glyph, [x, y])| (*glyph, [pen + x, *y])));
                pen += total;
            }
            // Their glyphs, each outline where HarfBuzz puts it: 40 units high, from (10, 60).
            let unit = 40.0 / font.units();
            let outline = |(glyph, [x, y]): &(u16, [i32; 2])| path_data(&font.outline_through(*glyph, [unit, 0.0, 0.0, -unit, 10.0 + *x as f32 * unit, 60.0 - *y as f32 * unit]).0);
            let data: String = placed.iter().map(outline).collect();
            let page = |shapes: String| decode(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">{shapes}</svg>"#), 600, 100).unwrap().1;
            let ours = page(format!(r#"<text x="10" y="60" font-family="{family}" font-size="40">{word}</text>"#));
            let wanted = page(format!(r#"<path d="{data}"/>"#));
            let apart = |at: usize| (0..4).map(|byte| ((ours[at] >> (byte * 8)) & 255).abs_diff((wanted[at] >> (byte * 8)) & 255)).max().unwrap_or(0);
            let differ = (0..ours.len()).filter(|at| apart(*at) > 24).count();
            assert!(differ == 0 && wanted.iter().any(|pixel| *pixel != 0), "{family}, {word}: {differ} pixels differ");
            println!("{family}: {word} as HarfBuzz shapes it");
            tried += 1;
        }
        println!("{tried} words held against HarfBuzz");
    }

    #[cfg(feature = "shaping")]
    #[test]
    fn text_is_shaped_in_runs_of_one_script() {
        // In one font and piece: Latin, then Greek with the sign after it, then Latin; a run
        // starting with a figure takes the script after it.
        let given = |text: &str| text.chars().map(|letter| Given { letter, piece: 0, at: [None; 4] }).collect::<Vec<_>>();
        let letters = given("ab αβγ, cd");
        let fonts = vec![Some(0); letters.len()];
        assert_eq!(runs(&letters, &fonts, 0), [0..3, 3..8, 8..10]);
        let letters = given("1αβ");
        assert_eq!(runs(&letters, &[Some(0); 3], 0).first(), Some(&(0..3)));
        // Another font is another run.
        assert_eq!(runs(&given("abc"), &[Some(0), Some(1), Some(1)], 0), [0..1, 1..3]);
    }
}
