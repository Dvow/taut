//! PDF output: a [`Paper`]'s pages as a file any viewer shows and any printer prints, using
//! only this library.
//!
//! A PDF file is a list of numbered objects (numbers, names, strings, arrays, dictionaries, and
//! streams with a dictionary), then a cross-reference table of their offsets, so a reader jumps
//! straight to one. Pages form a tree of dictionaries, each with its size and a content stream
//! of short operators ("x y m" moves, "x y l" draws a line, "f" fills). Here every page draws
//! in window units: its stream first scales a point (1/72 inch, PDF's unit) to a unit and flips
//! the page to count down from its top, as a window does.
//!
//! Text is glyphs from a font embedded with it, subset to the glyphs used. A TrueType font
//! (`glyf` outlines) is embedded as its own subset font, renumbered from zero in original
//! order, without hinting instructions (taut draws unhinted too): a two-byte "CIDFontType2".
//! Any other font this reads (CFF outlines, a variable font at a non-default position) goes in
//! as glyph outlines drawn as paths (a "Type 3" font of up to 256 glyphs, as many as needed).
//! Either way a "ToUnicode" map makes the text searchable and copyable.
//!
//! Gradients become shadings, which the viewer computes from the stops; where opacity varies
//! along them, a grey shading supplies it as a soft mask. Pictures are compressed as PNG rows
//! are ([`png::pack`](crate::png::pack)), with alpha as a separate picture; a JPEG goes in as
//! is. Links are page rectangles (annotations) to a URL or a place in the pages.
//!
//! Every number and name is written by this file; nothing from the app goes to the viewer
//! verbatim except URL text and the title, written as strings that cannot terminate early.
use crate::color::Color;
use crate::font::{Bytes, FontFile, listed};
use crate::geom::{Point, Rect};
use crate::paper::{Drawn, Kept, Mark, Paper, Picture, Words};
use crate::path::{Path, Step};
use crate::pen::{Cap, Join};
use std::collections::BTreeMap;

/// Points (PDF's unit) per unit: a unit is 1/96 inch, a point 1/72.
const POINT: f32 = 0.75;

impl Paper {
    /// The pages as a PDF file: its bytes, to save or send to a printer.
    pub fn pdf(&self) -> Vec<u8> {
        write(self)
    }
}

/// A PDF being written: its bytes so far, and each object's offset by number (0 is none).
struct File {
    bytes: Vec<u8>,
    at: Vec<usize>,
}

impl File {
    /// A number for an object written later.
    fn number(&mut self) -> u32 {
        self.at.push(0);
        self.at.len() as u32 - 1
    }
    /// Writes an object.
    fn object(&mut self, number: u32, body: &str) {
        self.at[number as usize] = self.bytes.len();
        self.bytes.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    /// Writes a stream: its dictionary besides the length, and its bytes, compressed.
    fn stream(&mut self, number: u32, said: &str, data: &[u8]) {
        let packed = crate::png::pack(data);
        self.raw(number, &format!("{said} /Filter /FlateDecode"), &packed);
    }
    /// Writes a stream whose bytes are already in final form (compressed already).
    fn raw(&mut self, number: u32, said: &str, data: &[u8]) {
        self.at[number as usize] = self.bytes.len();
        self.bytes.extend_from_slice(format!("{number} 0 obj\n<< {said} /Length {} >>\nstream\n", data.len()).as_bytes());
        self.bytes.extend_from_slice(data);
        self.bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }
}

/// How a font's glyphs are encoded: as a subset font, each glyph two bytes (its new id); or as
/// Type 3 fonts, each glyph one byte in one of them. Each used glyph with its encoding.
enum Coded {
    Whole(BTreeMap<u16, u16>),
    Drawn(BTreeMap<u16, (u32, u8)>),
}

/// A gradient on a page: the shape's fill and bounds, for a shape on a page of this height in
/// units; and once written, its index among the gradients, its shading, the graphics state for
/// its opacity (where not opaque throughout), and for a stroke, its pattern.
struct Blend {
    ink: Kept,
    around: Rect,
    page: f32,
    nth: usize,
    shading: u32,
    state: Option<usize>,
    pattern: Option<u32>,
}

/// A graphics state: fill and stroke opacity, 0 to 255 (or none, for a soft mask), and its
/// object number once written (0 until then).
type State = (Option<[u8; 2]>, u32);

/// What pages share: each font's encoding, the graphics states, and the gradients, by the marks
/// they belong to (page and index).
struct Shared {
    coded: Vec<Option<Coded>>,
    states: Vec<State>,
    blends: BTreeMap<(usize, usize), Blend>,
}

/// Writes a paper's pages as a PDF file.
fn write(paper: &Paper) -> Vec<u8> {
    let mut file = File { bytes: b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec(), at: vec![0] };
    let (catalog, tree, info, resources) = (file.number(), file.number(), file.number(), file.number());
    let mut shared = Shared { coded: Vec::new(), states: Vec::new(), blends: BTreeMap::new() };
    let mut fonts = String::new();
    for (at, font) in paper.files.iter().enumerate() {
        let used = used(paper, at as u32);
        let coded = (!used.is_empty()).then(|| write_font(&mut file, font, at, &used, &mut fonts));
        shared.coded.push(coded);
    }
    let mut pictures = String::new();
    for (at, picture) in paper.pictures.iter().enumerate() {
        let number = write_picture(&mut file, picture);
        pictures.push_str(&format!("/I{at} {number} 0 R "));
    }
    let (mut blends, mut patterns) = (String::new(), String::new());
    for (page, sheet) in paper.pages.iter().enumerate() {
        for (place, mark) in sheet.marks.iter().enumerate() {
            let Mark::Shape(drawn) = mark else { continue };
            if matches!(drawn.ink, Kept::Color(_)) {
                continue;
            }
            let (around, nth) = (drawn.path.bounds(), shared.blends.len());
            let mut blend = Blend { ink: drawn.ink.clone(), around, page: sheet.size.h, nth, shading: 0, state: None, pattern: None };
            write_blend(&mut file, &mut blend, drawn.pen.is_some(), &mut shared.states);
            blends.push_str(&format!("/S{nth} {} 0 R ", blend.shading));
            if let Some(pattern) = blend.pattern {
                patterns.push_str(&format!("/P{nth} {pattern} 0 R "));
            }
            shared.blends.insert((page, place), blend);
        }
    }
    // The pages, each with its content and links. (Each has its number first: a link may point
    // to a later page.)
    let kids: Vec<u32> = paper.pages.iter().map(|_| file.number()).collect();
    for (page, sheet) in paper.pages.iter().enumerate() {
        let contents = file.number();
        let drawn = content(paper, page, &mut shared);
        file.stream(contents, "", &drawn);
        let links = write_links(&mut file, paper, page, &kids);
        let (w, h) = (sheet.size.w * POINT, sheet.size.h * POINT);
        let annots = if links.is_empty() { String::new() } else { format!(" /Annots [{links}]") };
        file.object(kids[page], &format!("<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 {} {}] /Resources {resources} 0 R /Contents {contents} 0 R{annots} >>", num(w), num(h)));
    }
    // (Opacities are known once every page is written.)
    for at in 0..shared.states.len() {
        if let (Some([fill, line]), 0) = shared.states[at] {
            shared.states[at].1 = file.number();
            file.object(shared.states[at].1, &format!("<< /Type /ExtGState /ca {} /CA {} >>", num(fill as f32 / 255.0), num(line as f32 / 255.0)));
        }
    }
    let states: String = shared.states.iter().enumerate().map(|(at, (_, number))| format!("/G{at} {number} 0 R ")).collect();
    let resources_said = format!("<< /Font << {fonts}>> /XObject << {pictures}>> /ExtGState << {states}>> /Shading << {blends}>> /Pattern << {patterns}>> >>");
    file.object(resources, &resources_said);
    let kids_said: String = kids.iter().map(|kid| format!("{kid} 0 R ")).collect();
    file.object(tree, &format!("<< /Type /Pages /Kids [{kids_said}] /Count {} >>", kids.len()));
    file.object(catalog, &format!("<< /Type /Catalog /Pages {tree} 0 R >>"));
    let title = if paper.title.is_empty() { String::new() } else { format!("/Title {} ", text(&paper.title)) };
    file.object(info, &format!("<< {title}/Producer (taut) >>"));
    finish(file, catalog, info)
}

/// Writes the cross-reference table and the trailer, which points at it.
fn finish(mut file: File, catalog: u32, info: u32) -> Vec<u8> {
    let table = file.bytes.len();
    let mut out = format!("xref\n0 {}\n0000000000 65535 f \n", file.at.len());
    for at in &file.at[1..] {
        out.push_str(&format!("{at:010} 00000 n \n"));
    }
    // (The file's two identifiers: here the same, a hash of its content.)
    let mark = format!("{:016x}{:016x}", crate::id::hash(&file.bytes), crate::id::hash((&file.bytes, file.at.len())));
    out.push_str(&format!("trailer\n<< /Size {} /Root {catalog} 0 R /Info {info} 0 R /ID [<{mark}> <{mark}>] >>\nstartxref\n{table}\n%%EOF\n", file.at.len()));
    file.bytes.extend_from_slice(out.as_bytes());
    file.bytes
}

// ---- what is drawn on a page ----

/// A page's content stream operators.
fn content(paper: &Paper, page: usize, shared: &mut Shared) -> Vec<u8> {
    let sheet = &paper.pages[page];
    let mut out = format!("q {} 0 0 {} 0 {} cm\n", num(POINT), num(-POINT), num(sheet.size.h * POINT));
    let mut held = 0;
    for (place, mark) in sheet.marks.iter().enumerate() {
        match mark {
            Mark::Shape(drawn) => shape(&mut out, drawn, shared.blends.get(&(page, place)), &mut shared.states),
            Mark::Words(words) => write_words(&mut out, words, &paper.files[words.font as usize], shared.coded[words.font as usize].as_ref(), &mut shared.states),
            Mark::Picture(at, r) => placed_picture(&mut out, &paper.pictures[*at as usize], *at, *r),
            Mark::Clip(r) => {
                held += 1;
                out.push_str(&format!("q {} {} {} {} re W n\n", num(r.x), num(r.y), num(r.w.max(0.0)), num(r.h.max(0.0))));
            }
            Mark::Unclip if held > 0 => {
                held -= 1;
                out.push_str("Q\n");
            }
            Mark::Unclip | Mark::Link(..) => {}
        }
    }
    out.push_str(&"Q\n".repeat(held + 1));
    out.into_bytes()
}

/// The graphics state with fill opacity `solid[0]` and stroke opacity `solid[1]` (0 to 255), by
/// its index among the states: each written once, with the pages, and named in every page.
fn state(states: &mut Vec<State>, solid: [u8; 2]) -> usize {
    match states.iter().position(|(known, _)| *known == Some(solid)) {
        Some(at) => at,
        None => {
            states.push((Some(solid), 0));
            states.len() - 1
        }
    }
}

/// A shape: its path, then filled or stroked; in its colour, or its gradient (a shading).
fn shape(out: &mut String, drawn: &Drawn, blend: Option<&Blend>, states: &mut Vec<State>) {
    out.push_str("q ");
    if let Some(pen) = drawn.pen {
        let cap = match pen.cap {
            Cap::Flat => 0,
            Cap::Round => 1,
            Cap::Square => 2,
        };
        let (join, limit) = match pen.join {
            Join::Mitre(limit) => (0, limit.max(1.0)),
            Join::Round => (1, 10.0),
            Join::Bevel => (2, 10.0),
        };
        let dashes: Vec<String> = drawn.dashes.iter().map(|dash| num(dash.max(0.0))).collect();
        let dashed = drawn.dashes.iter().sum::<f32>() > 0.0;
        let dashes = if dashed { format!("[{}] {} d", dashes.join(" "), num(pen.offset)) } else { "[] 0 d".to_string() };
        out.push_str(&format!("{} w {cap} J {join} j {} M {dashes} ", num(pen.width.max(0.0)), num(limit)));
    }
    match (&drawn.ink, blend) {
        (Kept::Color(color), _) => {
            let solid = state(states, if drawn.pen.is_some() { [255, color.a()] } else { [color.a(), 255] });
            let paint = if drawn.pen.is_some() { "RG" } else { "rg" };
            out.push_str(&format!("/G{solid} gs {} {paint}\n", rgb(*color)));
            path(out, &drawn.path);
            out.push_str(if drawn.pen.is_some() { "S Q\n" } else { "f Q\n" });
        }
        (_, Some(blend)) => {
            if let Some(at) = blend.state {
                out.push_str(&format!("/G{at} gs "));
            }
            // (A stroke is drawn with a shading pattern; a fill is clipped to the shape and the
            // shading painted over that.)
            let nth = blend.nth;
            if drawn.pen.is_some() {
                out.push_str(&format!("/Pattern CS /P{nth} SCN\n"));
            }
            path(out, &drawn.path);
            match drawn.pen {
                Some(_) => out.push_str("S Q\n"),
                None => out.push_str(&format!("W n {} cm /S{nth} sh Q\n", matrix(space(&blend.ink, blend.around)))),
            }
        }
        (_, None) => out.push_str("Q\n"),
    }
}

/// A path's operators: a quadratic as the equivalent cubic.
fn path(out: &mut String, path: &Path) {
    let mut at = Point::default();
    let mut start = at;
    for step in path.steps() {
        match *step {
            Step::Move(p) => {
                out.push_str(&format!("{} {} m\n", num(p.x), num(p.y)));
                (at, start) = (p, p);
            }
            Step::Line(p) => {
                out.push_str(&format!("{} {} l\n", num(p.x), num(p.y)));
                at = p;
            }
            Step::Quad(c, p) => {
                let third = |from: Point| Point { x: from.x + (c.x - from.x) * 2.0 / 3.0, y: from.y + (c.y - from.y) * 2.0 / 3.0 };
                let (a, b) = (third(at), third(p));
                out.push_str(&format!("{} {} {} {} {} {} c\n", num(a.x), num(a.y), num(b.x), num(b.y), num(p.x), num(p.y)));
                at = p;
            }
            Step::Cubic(a, b, p) => {
                out.push_str(&format!("{} {} {} {} {} {} c\n", num(a.x), num(a.y), num(b.x), num(b.y), num(p.x), num(p.y)));
                at = p;
            }
            Step::Close => {
                out.push_str("h\n");
                at = start;
            }
        }
    }
}

/// One font's glyphs: one line of operators showing them, each placed. A glyph advances from
/// the previous one's pen by its width; where it should stand elsewhere, the difference is
/// given between the two (in thousandths of the font size), and a rise or drop from the
/// baseline is set before it.
fn write_words(out: &mut String, words: &Words, font: &FontFile, coded: Option<&Coded>, states: &mut Vec<State>) {
    let (Some(coded), Some(first)) = (coded, words.glyphs.first()) else { return };
    let solid = state(states, [words.color.a(), 255]);
    out.push_str(&format!("q /G{solid} gs {} rg BT 1 0 0 -1 {} {} Tm\n", rgb(words.color), num(first.at.x), num(first.at.y)));
    let (mut named, mut rise, mut x) = (None, 0.0, first.at.x);
    for glyph in &words.glyphs {
        let (part, code) = match coded {
            Coded::Whole(map) => (None, format!("{:04x}", map.get(&glyph.id).copied().unwrap_or(0))),
            Coded::Drawn(map) => {
                let (part, code) = map.get(&glyph.id).copied().unwrap_or((0, 0));
                (Some(part), format!("{code:02x}"))
            }
        };
        if named != Some(part) {
            out.push_str(if named.is_some() { "] TJ " } else { "" });
            out.push_str(&format!("/F{}{} {} Tf [", words.font, part.map_or(String::new(), |part| format!("-{part}")), num(words.size)));
            named = Some(part);
        }
        let up = first.at.y - glyph.at.y;
        if (up - rise).abs() > 1e-3 {
            out.push_str(&format!("] TJ {} Ts [", num(up)));
            rise = up;
        }
        let along = glyph.at.x - x;
        if along.abs() > 1e-3 {
            out.push_str(&format!("{} ", num(-along * 1000.0 / words.size)));
        }
        out.push_str(&format!("<{code}>"));
        x = glyph.at.x + width(font, glyph.id) * words.size / 1000.0;
    }
    out.push_str("] TJ ET Q\n");
}

/// A glyph's width in thousandths of the font size: as the file gives it to the viewer, which
/// advances by it.
fn width(font: &FontFile, glyph: u16) -> f32 {
    (font.width(glyph) * font.unit(1000.0) * 1e4).round() / 1e4
}

/// A picture stretched over `r`, rows going down: a JPEG rotated per its EXIF orientation.
fn placed_picture(out: &mut String, picture: &Picture, at: u32, r: Rect) {
    let turn = if let Picture::Jpeg { turn, .. } = picture { *turn } else { 1 };
    // Where the stored picture's left-to-right and top-to-bottom go when upright: across and
    // down the box, each from one side or the other (the last four are rotated a quarter).
    let [across, down] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let table: [[[f32; 3]; 2]; 8] = [
        [across, down],
        [[-1.0, 0.0, 1.0], down],
        [[-1.0, 0.0, 1.0], [0.0, -1.0, 1.0]],
        [across, [0.0, -1.0, 1.0]],
        [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        [[0.0, -1.0, 1.0], [1.0, 0.0, 0.0]],
        [[0.0, -1.0, 1.0], [-1.0, 0.0, 1.0]],
        [[0.0, 1.0, 0.0], [-1.0, 0.0, 1.0]],
    ];
    // (Each upright coordinate is a combination of the stored picture's:
    // `dx = a sx + b sy + c`. The image is a unit square with its first row at the top,
    // `sx = u`, `sy = 1 - v`.)
    let [[a, b, c], [d, e, f]] = table[turn.clamp(1, 8) - 1];
    let m = [r.w * a, r.h * d, -r.w * b, -r.h * e, r.x + r.w * (b + c), r.y + r.h * (e + f)];
    out.push_str(&format!("q {} cm /I{at} Do Q\n", matrix(m)));
}

// ---- fonts ----

/// The glyphs of font `font` the pages show, each with the text it first stood for.
fn used(paper: &Paper, font: u32) -> BTreeMap<u16, String> {
    let mut used = BTreeMap::new();
    for mark in paper.pages.iter().flat_map(|sheet| &sheet.marks) {
        let Mark::Words(words) = mark else { continue };
        if words.font != font {
            continue;
        }
        for glyph in &words.glyphs {
            let said = &words.text[glyph.chars[0] as usize..glyph.chars[1] as usize];
            let known = used.entry(glyph.id).or_insert_with(String::new);
            if known.is_empty() {
                known.push_str(said);
            }
        }
    }
    used
}

/// Writes a font's used glyphs: as a subset TrueType font where it is one at its default
/// position, else as Type 3 fonts; and adds it to the font resources. Returns its encoding.
fn write_font(file: &mut File, font: &FontFile, at: usize, used: &BTreeMap<u16, String>, fonts: &mut String) -> Coded {
    if let Some((program, kept)) = cut_down(font, used).filter(|_| font.varied.is_none()) {
        let number = whole_font(file, font, &program, &kept, used);
        fonts.push_str(&format!("/F{at} {number} 0 R "));
        return Coded::Whole(kept);
    }
    let mut map = BTreeMap::new();
    let glyphs: Vec<u16> = used.keys().copied().collect();
    for (part, some) in glyphs.chunks(256).enumerate() {
        let number = drawn_font(file, font, some, used);
        fonts.push_str(&format!("/F{at}-{part} {number} 0 R "));
        map.extend(some.iter().enumerate().map(|(code, glyph)| (*glyph, (part as u32, code as u8))));
    }
    Coded::Drawn(map)
}

/// Writes a TrueType font subset to its used glyphs, with its wrappers: the two-byte CID font,
/// its descendant, its descriptor, glyph widths and ToUnicode. Returns the font's object
/// number.
fn whole_font(file: &mut File, font: &FontFile, program: &[u8], kept: &BTreeMap<u16, u16>, used: &BTreeMap<u16, String>) -> u32 {
    let (top, made, about, data, map) = (file.number(), file.number(), file.number(), file.number(), file.number());
    let units = font.unit(1000.0);
    let name = format!("{}+{}", tag(kept), postscript_name(font));
    file.stream(data, &format!("/Length1 {}", program.len()), program);
    let widths: String = kept.iter().map(|(old, new)| format!("{new} [{}] ", num(width(font, *old)))).collect();
    let metrics = font.metrics(1000.0);
    let head = font_table(font, b"head");
    let edge = |at: usize| head.i16(at).unwrap_or(0) as f32 * units;
    let bounds = format!("[{} {} {} {}]", num(edge(36)), num(edge(38)), num(edge(40)), num(edge(42)));
    let said = format!(
        "<< /Type /FontDescriptor /FontName /{name} /Flags 4 /FontBBox {bounds} /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 {data} 0 R >>",
        num(metrics.ascent),
        num(-metrics.descent),
        num(if metrics.cap > 0.0 { metrics.cap } else { metrics.ascent })
    );
    file.object(about, &said);
    file.object(made, &format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {about} 0 R /W [{widths}] /CIDToGIDMap /Identity >>"));
    let pairs: Vec<(u32, &str)> = kept.iter().map(|(old, new)| (*new as u32, used.get(old).map_or("", String::as_str))).collect();
    file.stream(map, "", to_unicode(&pairs, 2).as_bytes());
    file.object(top, &format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{made} 0 R] /ToUnicode {map} 0 R >>"));
    top
}

/// Writes up to 256 glyphs as a Type 3 font, one byte per glyph: each glyph's outline, filled
/// in the text colour. Returns its number.
fn drawn_font(file: &mut File, font: &FontFile, glyphs: &[u16], used: &BTreeMap<u16, String>) -> u32 {
    let (top, map) = (file.number(), file.number());
    let k = font.unit(1000.0);
    let (mut procs, mut names, mut widths) = (String::new(), String::new(), String::new());
    let mut whole = Rect::EMPTY;
    for (code, glyph) in glyphs.iter().enumerate() {
        let (outline, _) = font.outline_through(*glyph, [k, 0.0, 0.0, k, 0.0, 0.0]);
        let (wide, around) = (width(font, *glyph), outline.bounds());
        whole = if whole.is_empty() { around } else { whole.union(around) };
        let mut drawn = format!("{} 0 {} {} {} {} d1\n", num(wide), num(around.x), num(around.y), num(around.right()), num(around.bottom()));
        path(&mut drawn, &outline);
        drawn.push_str("f\n");
        let number = file.number();
        file.stream(number, "", drawn.as_bytes());
        procs.push_str(&format!("/g{code} {number} 0 R "));
        names.push_str(&format!("/g{code} "));
        widths.push_str(&format!("{} ", num(wide)));
    }
    let pairs: Vec<(u32, &str)> = glyphs.iter().enumerate().map(|(code, glyph)| (code as u32, used.get(glyph).map_or("", String::as_str))).collect();
    file.stream(map, "", to_unicode(&pairs, 1).as_bytes());
    let bounds = format!("[{} {} {} {}]", num(whole.x), num(whole.y), num(whole.right()), num(whole.bottom()));
    file.object(
        top,
        &format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox {bounds} /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << {procs}>> /Encoding << /Type /Encoding /Differences [0 {names}] >> /FirstChar 0 /LastChar {} /Widths [{widths}] /Resources << >> /ToUnicode {map} 0 R >>",
            glyphs.len() - 1
        ),
    );
    top
}

/// A ToUnicode CMap from each glyph's code (`width` bytes) to its text, for viewers to find and
/// copy text. Glyphs standing for nothing are omitted.
fn to_unicode(pairs: &[(u32, &str)], width: usize) -> String {
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n",
    );
    out.push_str(&format!("1 begincodespacerange\n<{}> <{}>\nendcodespacerange\n", "00".repeat(width), "FF".repeat(width)));
    let said: Vec<&(u32, &str)> = pairs.iter().filter(|(_, text)| !text.is_empty()).collect();
    for hundred in said.chunks(100) {
        out.push_str(&format!("{} beginbfchar\n", hundred.len()));
        for (code, text) in hundred {
            let units: String = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
            out.push_str(&format!("<{code:0w$X}> <{units}>\n", w = width * 2));
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}

/// Six capitals distinguishing this subset from others, as subset font names require.
fn tag(kept: &BTreeMap<u16, u16>) -> String {
    let mut key = crate::id::hash(kept.keys().copied().collect::<Vec<u16>>());
    (0..6)
        .map(|_| {
            let letter = (b'A' + (key % 26) as u8) as char;
            key /= 26;
            letter
        })
        .collect()
}

/// The font's PostScript name (`name` id 6), in the characters such names allow; or one of its
/// own for a font with none.
fn postscript_name(font: &FontFile) -> String {
    let table = font_table(font, b"name");
    let (count, strings) = (table.u16(2).unwrap_or(0) as usize, table.u16(4).unwrap_or(0) as usize);
    let mut found = String::new();
    for at in (0..count).map(|n| 6 + 12 * n) {
        let (platform, which, long, from) = (table.u16(at), table.u16(at + 6), table.u16(at + 8).unwrap_or(0) as usize, table.u16(at + 10).unwrap_or(0) as usize);
        let Some(bytes) = table.part(strings + from, long).filter(|_| which == Some(6)) else { continue };
        found = match platform {
            Some(0 | 3) => char::decode_utf16(bytes.0.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair))).filter_map(Result::ok).collect(),
            _ => bytes.0.iter().map(|byte| *byte as char).collect(),
        };
        break;
    }
    let found: String = found.chars().filter(|letter| letter.is_ascii_alphanumeric() || *letter == '-').take(63).collect();
    if found.is_empty() { "Font".to_string() } else { found }
}

/// One of a font's tables (empty where missing).
fn font_table<'a>(font: &'a FontFile, name: &[u8; 4]) -> Bytes<'a> {
    let all = font.bytes();
    listed(all, font.start, name).map_or(Bytes(&[]), |table| all.within(&table))
}

/// A TrueType font of `font`'s used glyphs (plus the glyphs composites use, and glyph 0, the
/// notdef), renumbered from zero in original order, without instructions: the file, and each
/// glyph's new id by its old. Nothing for a non-TrueType or damaged font.
fn cut_down(font: &FontFile, used: &BTreeMap<u16, String>) -> Option<(Vec<u8>, BTreeMap<u16, u16>)> {
    let (head, hhea, maxp, hmtx) = (font_table(font, b"head"), font_table(font, b"hhea"), font_table(font, b"maxp"), font_table(font, b"hmtx"));
    let (glyf, loca) = (font_table(font, b"glyf"), font_table(font, b"loca"));
    let (long, count, metrics) = (head.i16(50)? != 0, maxp.u16(4)? as usize, hhea.u16(34)? as usize);
    if glyf.0.is_empty() || loca.0.is_empty() || metrics == 0 {
        return None;
    }
    let place = |glyph: usize| if long { loca.u32(glyph * 4).map(|at| at as usize) } else { loca.u16(glyph * 2).map(|at| at as usize * 2) };
    let bytes = |glyph: u16| -> Option<&[u8]> {
        let (from, to) = (place(glyph as usize)?, place(glyph as usize + 1)?);
        Some(if to > from { glyf.part(from, to - from)?.0 } else { &[] })
    };
    // Every used glyph and every component of those, found iteratively: a worklist still to
    // inspect.
    let mut kept: BTreeMap<u16, u16> = BTreeMap::new();
    let mut left: Vec<u16> = std::iter::once(0).chain(used.keys().copied()).filter(|glyph| (*glyph as usize) < count).collect();
    while let Some(glyph) = left.pop() {
        if kept.insert(glyph, 0).is_some() {
            continue;
        }
        left.extend(parts(bytes(glyph)?).into_iter().map(|(_, part)| part).filter(|part| (*part as usize) < count && !kept.contains_key(part)));
    }
    for (new, number) in kept.values_mut().enumerate() {
        *number = new as u16;
    }
    // The glyphs, each four-byte aligned; their offsets; and each one's advance and left side
    // bearing.
    let (mut glyphs, mut starts, mut widths) = (Vec::new(), Vec::new(), Vec::new());
    for old in kept.keys() {
        starts.extend((glyphs.len() as u32).to_be_bytes());
        glyphs.extend(plain(bytes(*old)?, &kept));
        glyphs.resize(glyphs.len().next_multiple_of(4), 0);
        let listed = (*old as usize).min(metrics - 1);
        let edge = if (*old as usize) < metrics { hmtx.u16(listed * 4 + 2) } else { hmtx.u16(metrics * 4 + (*old as usize - metrics) * 2) };
        widths.extend([hmtx.u16(listed * 4)?.to_be_bytes(), edge.unwrap_or(0).to_be_bytes()].concat());
    }
    starts.extend((glyphs.len() as u32).to_be_bytes());
    let n = (kept.len() as u16).to_be_bytes();
    let mut head = head.0.to_vec();
    let mut hhea = hhea.0.to_vec();
    let mut maxp = maxp.0.to_vec();
    head.get_mut(8..12)?.fill(0);
    head.get_mut(50..52)?.copy_from_slice(&[0, 1]);
    hhea.get_mut(34..36)?.copy_from_slice(&n);
    maxp.get_mut(4..6)?.copy_from_slice(&n);
    let post = [0x0003_0000u32.to_be_bytes(), [0; 4], [0; 4], [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]].concat();
    // (An empty `cmap`: viewers reach each glyph by id. One Windows Unicode subtable, with only
    // the terminating segment.)
    let none: [u16; 18] = [0, 1, 3, 1, 0, 12, 4, 24, 0, 2, 2, 0, 0, 0xFFFF, 0, 0xFFFF, 1, 0];
    let cmap = none.iter().flat_map(|number| number.to_be_bytes()).collect();
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"cmap", cmap), (b"glyf", glyphs), (b"head", head), (b"hhea", hhea), (b"hmtx", widths), (b"loca", starts), (b"maxp", maxp), (b"post", post)];
    Some((font_file(tables), kept))
}

/// A composite glyph's components: where in its bytes each glyph id is, and the id. (None for a
/// simple glyph.)
fn parts(glyph: &[u8]) -> Vec<(usize, u16)> {
    let read = Bytes(glyph);
    let mut found = Vec::new();
    if read.i16(0).is_none_or(|contours| contours >= 0) {
        return found;
    }
    let mut at = 10;
    while let (Some(flags), Some(part)) = (read.u16(at), read.u16(at + 2)) {
        found.push((at + 2, part));
        // (Two offset arguments, of words or bytes; then the transform: one, two or four
        // values.)
        let placing = if flags & 1 != 0 { 4 } else { 2 };
        let scaling = if flags & 8 != 0 {
            2
        } else if flags & 0x40 != 0 {
            4
        } else if flags & 0x80 != 0 {
            8
        } else {
            0
        };
        at += 4 + placing + scaling;
        if flags & 0x20 == 0 {
            break;
        }
    }
    found
}

/// A glyph as the subset has it: a composite's components renumbered, and hinting instructions
/// removed.
fn plain(glyph: &[u8], kept: &BTreeMap<u16, u16>) -> Vec<u8> {
    let read = Bytes(glyph);
    let Some(contours) = read.i16(0) else { return Vec::new() };
    if contours >= 0 {
        // Ten header bytes, contour end points, instruction length and instructions; then the
        // points.
        let ends = 10 + 2 * contours as usize;
        let Some(long) = read.u16(ends) else { return glyph.to_vec() };
        let rest = glyph.get(ends + 2 + long as usize..).unwrap_or(&[]);
        return [&glyph[..ends], &[0, 0], rest].concat();
    }
    let parts = parts(glyph);
    let mut out = glyph.to_vec();
    for (at, part) in &parts {
        out[*at..*at + 2].copy_from_slice(&kept.get(part).copied().unwrap_or(0).to_be_bytes());
    }
    // (The last component's flags say whether instructions follow: that flag is cleared and
    // they are dropped.)
    if let Some((at, _)) = parts.last() {
        let flags = u16::from_be_bytes([out[at - 2], out[at - 1]]);
        let placing = if flags & 1 != 0 { 4 } else { 2 };
        let scaling = if flags & 8 != 0 {
            2
        } else if flags & 0x40 != 0 {
            4
        } else if flags & 0x80 != 0 {
            8
        } else {
            0
        };
        out.truncate((at + 2 + placing + scaling).min(out.len()));
        out[at - 2..*at].copy_from_slice(&(flags & !0x100).to_be_bytes());
    }
    out
}

/// A font file of these tables, sorted by tag, with its table directory, each table's checksum,
/// and the whole file's checksum adjustment in `head`.
fn font_file(tables: Vec<(&[u8; 4], Vec<u8>)>) -> Vec<u8> {
    let sum = |bytes: &[u8]| bytes.chunks(4).fold(0u32, |sum, four| sum.wrapping_add(u32::from_be_bytes(std::array::from_fn(|n| four.get(n).copied().unwrap_or(0)))));
    let count = tables.len() as u16;
    let mut file = [0x0001_0000u32.to_be_bytes().as_slice(), &count.to_be_bytes(), &[0, 64, 0, 2, 0, 48]].concat();
    let mut at = 12 + 16 * tables.len();
    for (name, table) in &tables {
        file.extend([&name[..], &sum(table).to_be_bytes(), &(at as u32).to_be_bytes(), &(table.len() as u32).to_be_bytes()].concat());
        at += table.len().next_multiple_of(4);
    }
    let mut head = 0;
    for (name, table) in &tables {
        head = if *name == b"head" { file.len() } else { head };
        file.extend_from_slice(table);
        file.resize(file.len().next_multiple_of(4), 0);
    }
    let whole = 0xB1B0_AFBAu32.wrapping_sub(sum(&file));
    file[head + 8..head + 12].copy_from_slice(&whole.to_be_bytes());
    file
}

// ---- pictures ----

/// Writes a picture: its colours (PNG Up-filtered rows, which compress photos far smaller), and
/// alpha where any pixel is not opaque; or a JPEG as is. Returns its number.
fn write_picture(file: &mut File, picture: &Picture) -> u32 {
    let number = file.number();
    match picture {
        Picture::Jpeg { file: jpeg, size, planes, adobe, turn } => {
            let stored = if *turn >= 5 { [size[1], size[0]] } else { *size };
            let (space, decode) = match (planes, adobe) {
                (1, _) => ("/DeviceGray", ""),
                (4, true) => ("/DeviceCMYK", " /Decode [1 0 1 0 1 0 1 0]"),
                _ => ("/DeviceRGB", ""),
            };
            file.raw(number, &format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {space} /BitsPerComponent 8{decode} /Filter /DCTDecode", stored[0], stored[1]), jpeg);
        }
        Picture::Pixels { width, pixels } => {
            let height = pixels.len() / width;
            let clear = pixels.iter().any(|pixel| pixel >> 24 != 255);
            let mask = clear.then(|| {
                let solid: Vec<u8> = pixels.iter().map(|pixel| (pixel >> 24) as u8).collect();
                let number = file.number();
                file.stream(number, &format!("/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /DecodeParms << /Predictor 12 /Colors 1 /Columns {width} >> /Interpolate true"), &upward::<1>(*width, &solid));
                format!(" /SMask {number} 0 R")
            });
            let colours: Vec<u8> = pixels.iter().flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]).collect();
            let said = format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /DecodeParms << /Predictor 12 /Colors 3 /Columns {width} >> /Interpolate true{}",
                mask.unwrap_or_default()
            );
            file.stream(number, &said, &upward::<3>(*width, &colours));
        }
    }
    number
}

/// Rows of `N`-byte pixels as PNG's Up filter stores them: each row begins with 2, and each
/// byte is its difference from the one above.
fn upward<const N: usize>(width: usize, bytes: &[u8]) -> Vec<u8> {
    let stride = width * N;
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / stride.max(1));
    for (n, row) in bytes.chunks_exact(stride.max(1)).enumerate() {
        out.push(2);
        let above = if n == 0 { None } else { bytes.get((n - 1) * stride..n * stride) };
        out.extend(row.iter().enumerate().map(|(at, byte)| byte.wrapping_sub(above.map_or(0, |above| above[at]))));
    }
    out
}

// ---- colours that go over into each other ----

/// A gradient shading's space, as a six-number transform to the page's: the shape's bounds for
/// a linear gradient (its shading gives its ends in page units), or a unit circle round the
/// box's centre, stretched to the box, for a radial one.
fn space(ink: &Kept, around: Rect) -> [f32; 6] {
    match ink {
        Kept::Around(_) => [around.w / 2.0, 0.0, 0.0, around.h / 2.0, around.x + around.w / 2.0, around.y + around.h / 2.0],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

/// Writes a gradient's shading, and the soft mask for its opacity where that varies along it
/// (or the graphics state for a uniform opacity); and for a stroke, a shading pattern in page
/// space.
fn write_blend(file: &mut File, blend: &mut Blend, line: bool, states: &mut Vec<State>) {
    let stops = match &blend.ink {
        Kept::Along(_, stops) | Kept::Around(stops) => stops.clone(),
        Kept::Color(color) => vec![(0.0, *color)],
    };
    let geometry = match &blend.ink {
        Kept::Along(turn, _) => {
            let (r, (sin, cos)) = (blend.around, turn.to_radians().sin_cos());
            let (mid, reach) = (Point { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }, (sin.abs() * r.w / 2.0 + cos.abs() * r.h / 2.0).max(1e-3));
            format!("/ShadingType 2 /Coords [{} {} {} {}]", num(mid.x - sin * reach), num(mid.y + cos * reach), num(mid.x + sin * reach), num(mid.y - cos * reach))
        }
        _ => "/ShadingType 3 /Coords [0 0 0 0 0 1]".to_string(),
    };
    blend.shading = file.number();
    let colours = function(&stops, rgb);
    file.object(blend.shading, &format!("<< {geometry} /ColorSpace /DeviceRGB /Function {colours} /Extend [true true] >>"));
    let solids: Vec<u8> = stops.iter().map(|(_, color)| color.a()).collect();
    if solids.iter().all(|solid| *solid == solids[0]) {
        let at = state(states, if line { [255, solids[0]] } else { [solids[0], 255] });
        blend.state = (solids[0] != 255).then_some(at);
    } else {
        let (grey, form, mask) = (file.number(), file.number(), file.number());
        let alphas = function(&stops, |color| num(color.a() as f32 / 255.0));
        file.object(grey, &format!("<< {geometry} /ColorSpace /DeviceGray /Function {alphas} /Extend [true true] >>"));
        // (The mask is computed in page units where it is set, as the shape is; what lies
        // outside the shape is not drawn anyway.)
        let drawn = format!("q {} cm /A sh Q", matrix(space(&blend.ink, blend.around)));
        let all = "[-100000 -100000 100000 100000]";
        file.stream(form, &format!("/Type /XObject /Subtype /Form /BBox {all} /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Shading << /A {grey} 0 R >> >>"), drawn.as_bytes());
        file.object(mask, &format!("<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G {form} 0 R >> >>"));
        states.push((None, mask));
        blend.state = Some(states.len() - 1);
    }
    if line {
        let pattern = file.number();
        let page = [POINT, 0.0, 0.0, -POINT, 0.0, blend.page * POINT];
        let m = then(space(&blend.ink, blend.around), page);
        file.object(pattern, &format!("<< /PatternType 2 /Shading {} 0 R /Matrix [{}] >>", blend.shading, matrix(m)));
        blend.pattern = Some(pattern);
    }
}

/// A function of one value from 0 to 1 giving `stops`' colours at their offsets, interpolated
/// linearly between them, and extended with the first before and the last after: each colour
/// written by `color`.
fn function(stops: &[(f32, Color)], color: impl Fn(Color) -> String) -> String {
    let mut points: Vec<(f32, Color)> = Vec::new();
    for (at, c) in stops {
        let at = at.clamp(0.0, 1.0);
        let at = points.last().map_or(at, |(last, _)| at.max(last + 1e-4));
        points.push((at, *c));
    }
    let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) else { return "<< /FunctionType 2 /Domain [0 1] /N 1 >>".into() };
    if first.0 > 0.0 {
        points.insert(0, (0.0, first.1));
    }
    if last.0 < 1.0 {
        points.push((1.0, last.1));
    }
    let piece = |a: Color, b: Color| format!("<< /FunctionType 2 /Domain [0 1] /C0 [{}] /C1 [{}] /N 1 >>", color(a), color(b));
    if points.len() < 2 {
        return piece(first.1, first.1);
    }
    let pieces: Vec<String> = points.windows(2).map(|pair| piece(pair[0].1, pair[1].1)).collect();
    let bounds: Vec<String> = points[1..points.len() - 1].iter().map(|(at, _)| num(*at)).collect();
    let encode = "0 1 ".repeat(pieces.len());
    format!("<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{encode}] >>", pieces.join(" "), bounds.join(" "))
}

// ---- links ----

/// Writes a page's links: returns their array, for the page to reference.
fn write_links(file: &mut File, paper: &Paper, page: usize, kids: &[u32]) -> String {
    let height = paper.pages[page].size.h;
    let mut links = String::new();
    for mark in &paper.pages[page].marks {
        let Mark::Link(r, to) = mark else { continue };
        let action = match to.strip_prefix('#') {
            Some(name) => {
                let Some((_, on, y)) = paper.anchors.iter().find(|(known, ..)| known == name) else { continue };
                let top = (paper.pages[*on].size.h - y) * POINT;
                format!("/Dest [{} 0 R /XYZ null {} null]", kids[*on], num(top))
            }
            None => format!("/A << /S /URI /URI {} >>", ascii(to)),
        };
        let number = file.number();
        let rect = [r.x * POINT, (height - r.bottom()) * POINT, r.right() * POINT, (height - r.y) * POINT].map(num).join(" ");
        file.object(number, &format!("<< /Type /Annot /Subtype /Link /Rect [{rect}] /Border [0 0 0] {action} >>"));
        links.push_str(&format!("{number} 0 R "));
    }
    links
}

// ---- writing numbers and texts ----

/// A number as PDF writes it: at most four decimals, no unneeded ones. (NaN is 0.)
fn num(value: f32) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    let text = format!("{:.4}", value);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() { "0".into() } else { text.into() }
}

/// A transform's six numbers.
fn matrix(m: [f32; 6]) -> String {
    m.map(num).join(" ")
}

/// Transform `a`, then `b`.
fn then(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3], a[4] * b[0] + a[5] * b[2] + b[4], a[4] * b[1] + a[5] * b[3] + b[5]]
}

/// A colour as three values from 0 to 1.
fn rgb(color: Color) -> String {
    [16, 8, 0].map(|shift| num((color.0 >> shift & 0xff) as f32 / 255.0)).join(" ")
}

/// A text string a viewer shows (a title): UTF-16BE after its byte order mark, in hex.
fn text(said: &str) -> String {
    let units: String = said.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
    format!("<FEFF{units}>")
}

/// A URL as an ASCII string: characters a URL cannot hold verbatim (a space, non-ASCII)
/// percent-encoded, and string delimiters escaped with ``.
fn ascii(said: &str) -> String {
    let mut out = String::from("(");
    for byte in said.bytes() {
        match byte {
            b'(' | b')' | b'\\' => out.push_str(&format!("\\{}", byte as char)),
            0x21..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out.push(')');
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::{Size, rect};
    use crate::pen::{Ink, Pen};
    use crate::text::{Face, Font};

    /// A system font file with Latin letters, if any.
    pub(crate) fn latin() -> Option<FontFile> {
        let places = ["C:/Windows/Fonts/arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf"];
        places.iter().find_map(FontFile::open)
    }

    /// A page with a bit of everything.
    pub(crate) fn sample() -> Paper {
        let mut paper = Paper::new();
        paper.title = "A sample, ünïcödé".into();
        if let Some(font) = latin() {
            paper.font(Face::Regular, font);
        }
        paper.page(Paper::A4);
        paper.rect(rect(40.0, 40.0, 200.0, 100.0), Color::rgb(0x2b6cb0));
        paper.rounded(rect(260.0, 40.0, 200.0, 100.0), 16.0, Color::rgb(0xe53e3e).alpha(0.5));
        paper.border(rect(480.0, 40.0, 200.0, 100.0), 12.0, 4.0, Color::BLACK);
        paper.gradient(rect(40.0, 160.0, 300.0, 80.0), 8.0, &[(0.0, Color::rgb(0xffcc00)), (0.5, Color::rgb(0x00aa88)), (1.0, Color::rgb(0x3355ff))], Some(90.0));
        paper.gradient(rect(360.0, 160.0, 160.0, 120.0), 0.0, &[(0.0, Color::WHITE), (1.0, Color::rgb(0x330066).alpha(0.2))], None);
        let mut wave = Path::new();
        wave.move_to(40.0, 330.0).quad_to(140.0, 260.0, 240.0, 330.0).cubic_to(300.0, 380.0, 360.0, 280.0, 420.0, 330.0);
        paper.outline(&wave, Pen::new(6.0).cap(crate::pen::Cap::Flat).join(Join::Mitre(4.0)).dashes(&[18.0, 8.0], 0.0), Color::rgb(0x805ad5));
        let mut star = Path::new();
        star.star(560.0, 330.0, 50.0, 22.0, 5);
        paper.shape(&star, Ink::Along(180.0, &[(0.0, Color::rgb(0xff0000)), (1.0, Color::rgb(0x0000ff))]));
        paper.outline(&star, 3.0, Ink::Around(&[(0.0, Color::BLACK), (1.0, Color::rgb(0x00ff00))]));
        let font = Font::new(28.0);
        paper.text("Hello, PDF: kerning AV To fi", 40.0, 400.0, font, Color::BLACK);
        paper.text("Underlined", 40.0, 450.0, font.underlined(), Color::rgb(0x2b6cb0));
        let pixels: Vec<u32> = (0..64 * 48).map(|n| 0xff00_0000 | ((n % 64) * 4) << 16 | ((n / 64) * 5) << 8 | 0x80).collect();
        paper.picture(64, &pixels, rect(40.0, 500.0, 256.0, 192.0));
        let clear: Vec<u32> = (0..32 * 32).map(|n| ((n % 32) * 8) << 24 | 0xff0000).collect();
        paper.picture(32, &clear, rect(320.0, 500.0, 128.0, 128.0));
        paper.link(rect(40.0, 400.0, 300.0, 34.0), "https://example.com/a path?x=(1)");
        paper.clip(rect(480.0, 500.0, 100.0, 100.0));
        paper.rounded(rect(450.0, 470.0, 200.0, 200.0), 100.0, Color::rgb(0x38a169));
        paper.unclip();
        paper.page(Size { w: 600.0, h: 300.0 });
        paper.anchor("second", 0.0);
        paper.text("A second page, wider than high", 20.0, 20.0, font, Color::BLACK);
        paper.link(rect(20.0, 100.0, 200.0, 30.0), "#second");
        paper
    }

    /// The first of some font files that exists.
    pub(crate) fn font_at(places: &[&str]) -> Option<FontFile> {
        places.iter().find_map(FontFile::open)
    }
    /// A font with Arabic letters, if any.
    pub(crate) fn arabic() -> Option<FontFile> {
        font_at(&["C:/Windows/Fonts/arial.ttf", "/mnt/c/Windows/Fonts/arial.ttf", "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"])
    }
    /// A font with Chinese characters (the first of a collection), if any.
    pub(crate) fn chinese() -> Option<FontFile> {
        font_at(&["C:/Windows/Fonts/msyh.ttc", "/mnt/c/Windows/Fonts/msyh.ttc", "C:/Windows/Fonts/simsun.ttc", "/mnt/c/Windows/Fonts/simsun.ttc"])
    }

    /// A PDF file's objects, as its own cross-reference table locates them: each one's number
    /// and bytes, from its number to `endobj`. Fails where the table and file disagree.
    fn objects(file: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let at = |what: &[u8]| file.windows(what.len()).rposition(|bytes| bytes == what).unwrap_or_else(|| panic!("no {:?}", String::from_utf8_lossy(what)));
        let line = |from: usize| String::from_utf8_lossy(&file[from..]).lines().next().unwrap_or("").to_string();
        let table: usize = line(at(b"startxref\n") + 10).parse().unwrap();
        assert_eq!(table, at(b"xref\n0 "), "the end of the file says where the table is");
        let count: usize = line(table + 7).parse().unwrap();
        assert!(String::from_utf8_lossy(&file[at(b"trailer")..]).contains(&format!("/Size {count} ")));
        let mut found = Vec::new();
        for n in 1..count {
            let entry = String::from_utf8_lossy(&file[table + 7 + count.to_string().len() + 1 + 20 * n..][..20]).into_owned();
            assert!(entry.ends_with(" 00000 n \n"), "{entry:?}");
            let start: usize = entry[..10].parse().unwrap();
            assert!(file[start..].starts_with(format!("{n} 0 obj\n").as_bytes()), "object {n} is where the table says");
            let end = start + file[start..].windows(8).position(|bytes| bytes == b"\nendobj\n").unwrap();
            found.push((n as u32, file[start..end].to_vec()));
        }
        found
    }

    /// A stream object's bytes, decompressed where compressed.
    fn streamed(object: &[u8]) -> Option<Vec<u8>> {
        let from = object.windows(10).position(|bytes| bytes == b">>\nstream\n")? + 10;
        let said = String::from_utf8_lossy(&object[..from]).into_owned();
        let long: usize = said[said.find("/Length ")? + 8..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        let data = &object[from..from + long];
        assert!(object[from + long..].starts_with(b"\nendstream"), "a stream is as long as it says");
        match said.contains("/FlateDecode") {
            true => Some(crate::png::unpack(data, 1 << 28).expect("a packed stream unpacks")),
            false => Some(data.to_vec()),
        }
    }

    #[test]
    fn a_file_is_laid_out_as_readers_find_their_way_in_it() {
        let file = sample().pdf();
        assert!(file.starts_with(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n") && file.ends_with(b"%%EOF\n"));
        let objects = objects(&file);
        assert!(objects.len() > 20, "{} objects", objects.len());
        let streams = objects.iter().filter(|(_, object)| object.windows(7).any(|seven| seven == b"stream\n")).count();
        assert_eq!(objects.iter().filter_map(|(_, object)| streamed(object)).count(), streams, "every stream is whole");
        // The same pages make the same file, byte for byte.
        assert_eq!(file, sample().pdf());
        // The title as the viewer shows it, and a link's URL (with its brackets and space
        // written so they cannot terminate the string).
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("/Title <FEFF00410020007300610020006D0070006C0065002C00200075006E00EF00630064006E00E9>") || text.contains("/Title <FEFF"));
        assert!(text.contains("/URI (https://example.com/a%20path?x=\\(1\\))"), "the address as it was given");
        assert!(text.contains("/Dest ["), "a link to a place in the pages");
    }

    #[test]
    fn a_font_cut_down_holds_the_glyphs_used_as_they_were() {
        let Some(font) = latin() else { return eprintln!("no font file here: not tested") };
        let text = "Hello, \u{c4}\u{e9}\u{f1}! fi 0123";
        let used: BTreeMap<u16, String> = text.chars().map(|letter| (font.glyph(letter), letter.to_string())).collect();
        let (program, kept) = cut_down(&font, &used).expect("a TrueType font is cut down");
        let small = FontFile::read(program.clone()).expect("what is cut down is a font");
        assert!(kept.contains_key(&0) && kept.len() >= used.len(), "the glyph for letters it lacks, and each used");
        assert!(program.len() < font.bytes().0.len() / 10, "{} bytes of {}", program.len(), font.bytes().0.len());
        let unit = [0.05, 0.0, 0.0, -0.05, 0.0, 40.0];
        for (old, new) in &kept {
            assert_eq!(small.outline_through(*new, unit).0, font.outline_through(*old, unit).0, "glyph {old}, now {new}, has its shape");
            assert_eq!(small.width(*new), font.width(*old));
        }
        // A composite glyph (an A with diaeresis, in fonts that compose it) brings its
        // components, renumbered. (Two components: the first offset by a byte each way, the
        // second by words, with instructions after.)
        let glyph = [0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x22, 0x00, 0x24, 1, 2, 0x01, 0x01, 0x00, 0x05, 3, 4, 5, 6, 0, 2, 9, 9];
        assert_eq!(parts(&glyph), [(12, 0x24), (18, 5)]);
        let renumbered = plain(&glyph, &BTreeMap::from([(0x24, 1), (5, 2)]));
        assert_eq!(renumbered, [0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x22, 0x00, 0x01, 1, 2, 0x00, 0x01, 0x00, 0x02, 3, 4, 5, 6], "and loses its instructions");
    }

    #[test]
    fn each_glyph_says_what_it_stands_for() {
        let Some(font) = latin() else { return eprintln!("no font file here: not tested") };
        let mut paper = Paper::new();
        paper.font(Face::Regular, font);
        let written = "Office of fine \u{c5}ngstr\u{f6}m, \u{2014} it\u{2019}s 100%";
        paper.text(written, 10.0, 10.0, Font::new(20.0), Color::BLACK);
        let used = used(&paper, 0);
        let Some(Mark::Words(words)) = paper.pages[0].marks.first() else { panic!("words were written") };
        let read: String = words.glyphs.iter().map(|glyph| used[&glyph.id].as_str()).collect();
        assert_eq!(read, written, "the glyphs, read back in order, are the text");
        // And in the file, each glyph's two bytes map to its text.
        let file = paper.pdf();
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("/Encoding /Identity-H") && text.contains("/CIDToGIDMap /Identity") && text.contains("/ToUnicode"));
        let maps: Vec<String> =
            objects(&file).iter().filter_map(|(_, object)| streamed(object)).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).filter(|text| text.contains("beginbfchar")).collect();
        assert_eq!(maps.len(), 1);
        assert!(maps[0].contains("<00C5>") && maps[0].contains("<2014>") && maps[0].contains("<2019>"), "{}", maps[0]);
    }

    #[test]
    fn a_font_set_on_its_axes_is_written_as_drawn_glyphs() {
        let Some(font) = font_at(&["C:/Windows/Fonts/SegUIVar.ttf", "/mnt/c/Windows/Fonts/SegUIVar.ttf"]) else { return eprintln!("no variable font here: not tested") };
        let mut paper = Paper::new();
        paper.font(Face::Regular, font.at(b"wght", 700.0));
        let written: String = (0..300).map(|n| char::from_u32(0x41 + n % 26 + (n / 26 % 2) * 32).unwrap()).chain("\u{e9}\u{e8}\u{ea}\u{eb}".chars()).collect();
        paper.text(&written, 10.0, 10.0, Font::new(16.0), Color::BLACK);
        let file = paper.pdf();
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("/Subtype /Type3") && !text.contains("/CIDFontType2"), "a font of drawn glyphs");
        let Some(Mark::Words(words)) = paper.pages[0].marks.first() else { panic!("words were written") };
        let used = used(&paper, 0);
        let read: String = words.glyphs.iter().map(|glyph| used[&glyph.id].as_str()).collect();
        assert_eq!(read, written);
    }

    #[test]
    fn a_jpeg_goes_in_as_it_is_and_others_as_pixels() {
        let jpeg = |name: &str| std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pictures").join(name)).unwrap();
        let (plain, inks, deep, arithmetic) = (jpeg("plain.jpg"), jpeg("inks.jpg"), jpeg("deep.jpg"), jpeg("arithmetic.jpg"));
        let mut paper = Paper::new();
        assert!(paper.jpeg(&plain, rect(0.0, 0.0, 67.0, 45.0)) && paper.jpeg(&inks, rect(0.0, 50.0, 48.0, 37.0)));
        // (Twelve-bit and arithmetic-coded: not every viewer decodes those.)
        assert!(paper.jpeg(&deep, rect(0.0, 100.0, 40.0, 33.0)) && paper.jpeg(&arithmetic, rect(0.0, 140.0, 40.0, 33.0)));
        assert!(!paper.jpeg(b"no JPEG", rect(0.0, 0.0, 1.0, 1.0)));
        let file = paper.pdf();
        let objects = objects(&file);
        let passed: Vec<&Vec<u8>> = objects.iter().map(|(_, object)| object).filter(|object| object.windows(10).any(|ten| ten == b"/DCTDecode")).collect();
        assert_eq!(passed.len(), 2, "two went in as they are");
        assert!(passed.iter().any(|object| object.windows(plain.len()).any(|bytes| bytes == plain.as_slice())), "byte for byte");
        let said: Vec<String> = passed.iter().map(|object| String::from_utf8_lossy(&object[..200.min(object.len())]).into_owned()).collect();
        assert!(said.iter().any(|said| said.contains("/DeviceCMYK /BitsPerComponent 8 /Decode [1 0 1 0 1 0 1 0]")), "{said:?}");
        let pixels = objects.iter().filter(|(_, object)| String::from_utf8_lossy(&object[..120.min(object.len())]).contains("/ColorSpace /DeviceRGB /BitsPerComponent 8 /DecodeParms")).count();
        assert_eq!(pixels, 2, "the other two as pixels");
    }

    #[test]
    fn a_picture_is_kept_once_and_what_shows_through_it_is_said() {
        let solid: Vec<u32> = (0..64).map(|n| 0xff00_0000 | (n * 4)).collect();
        let clear: Vec<u32> = (0..64).map(|n| (n * 4) << 24 | 0xff).collect();
        let mut paper = Paper::new();
        for at in 0..3 {
            paper.picture(8, &solid, rect(at as f32 * 20.0, 0.0, 16.0, 16.0));
        }
        paper.picture(8, &clear, rect(0.0, 20.0, 16.0, 16.0));
        assert_eq!(paper.pictures.len(), 2);
        let file = paper.pdf();
        let images = objects(&file).iter().filter(|(_, object)| object.windows(13).any(|bytes| bytes == b"/Subtype /Ima")).count();
        assert_eq!(images, 3, "two pictures, and how solid the second is");
        assert!(String::from_utf8_lossy(&file).contains("/SMask "));
    }

    #[test]
    fn what_a_window_shows_goes_on_a_page_as_a_picture() {
        let mut paper = Paper::new();
        paper.page(Paper::LETTER);
        paper.interface(rect(72.0, 72.0, 200.0, 80.0), &mut |ui| {
            ui.label("Shown in a window");
            ui.button("Press");
        });
        let Some(crate::paper::Picture::Pixels { width, pixels }) = paper.pictures.first() else { panic!("a picture") };
        assert_eq!((*width, pixels.len() / width), (625, 250), "three hundred pixels to an inch");
        assert!(pixels.iter().all(|pixel| pixel >> 24 == 255), "on the theme's ground");
        assert!(pixels.iter().any(|pixel| pixel & 0xff_ffff != pixels[0] & 0xff_ffff), "with something on it");
    }
}

#[cfg(test)]
pub(crate) mod readers {
    use crate::paper::Paper;
    use std::process::Command;

    /// What a PDF renderer on this machine draws of a file's pages, `scale` pixels per unit,
    /// each page `0x00RRGGBB` (white where empty): Poppler's `pdftoppm`, or Ghostscript's `gs`.
    /// Nothing where it is not installed.
    pub(crate) fn drawn_by(reader: &str, file: &[u8], scale: f32) -> Option<Vec<(usize, Vec<u32>)>> {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let folder = std::env::temp_dir().join(format!("taut-pdf-{}-{}", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::write(folder.join("in.pdf"), file).ok()?;
        let dpi = format!("{}", scale * 96.0);
        let ran = match reader {
            "pdftoppm" => Command::new("pdftoppm").args(["-r", &dpi, "-png", "in.pdf", "page"]).current_dir(&folder).output(),
            _ => Command::new("gs")
                .args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-sDEVICE=png16m", "-dTextAlphaBits=4", "-dGraphicsAlphaBits=4", &format!("-r{dpi}"), "-sOutputFile=page-%d.png", "in.pdf"])
                .current_dir(&folder)
                .output(),
        };
        let ran = ran.ok().filter(|ran| ran.status.success());
        let mut names: Vec<_> =
            std::fs::read_dir(&folder).ok()?.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.extension().is_some_and(|ending| ending == "png")).collect();
        names.sort_by_key(|path| path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.rsplit('-').next()).and_then(|n| n.parse::<u32>().ok()));
        let pages = names.iter().filter_map(|name| crate::png::decode(&std::fs::read(name).ok()?)).collect();
        let _ = std::fs::remove_dir_all(&folder);
        ran.map(|_| pages)
    }

    /// How far a renderer's page is from taut's own: the mean per-pixel difference (the largest
    /// of red, green and blue, 0 to 255), and the share of pixels differing by more than a
    /// quarter. Nothing where sizes differ (a pixel more or less each way is tolerated).
    pub(crate) fn apart(theirs: &(usize, Vec<u32>), ours: &(usize, Vec<u32>)) -> Option<(f32, f32)> {
        let ([w1, h1], [w2, h2]) = ([theirs.0, theirs.1.len() / theirs.0], [ours.0, ours.1.len() / ours.0]);
        if w1.abs_diff(w2) > 1 || h1.abs_diff(h2) > 1 {
            return None;
        }
        let (w, h) = (w1.min(w2), h1.min(h2));
        let (mut sum, mut far) = (0u64, 0u64);
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (theirs.1[y * w1 + x], ours.1[y * w2 + x]);
                let d = [16, 8, 0].map(|shift| ((a >> shift & 0xff) as i32 - (b >> shift & 0xff) as i32).unsigned_abs()).into_iter().max().unwrap_or(0);
                sum += d as u64;
                far += (d > 64) as u64;
            }
        }
        let all = (w * h) as f32;
        Some((sum as f32 / all, far as f32 / all))
    }

    /// A renderer, a page, and its distance from taut's own drawing.
    pub(crate) type Compared = (&'static str, usize, Option<(f32, f32)>);

    /// Each page of a paper as each installed renderer draws it, compared with taut's: per
    /// renderer and page, how far apart.
    pub(crate) fn compared(paper: &Paper, scale: f32) -> Vec<Compared> {
        let file = paper.pdf();
        let mut found = Vec::new();
        for reader in ["pdftoppm", "gs"] {
            let Some(pages) = drawn_by(reader, &file, scale) else { continue };
            assert_eq!(pages.len(), paper.pages(), "{reader} drew every page");
            for (page, theirs) in pages.iter().enumerate() {
                let ours = paper.picture_of(page, scale);
                found.push((reader, page, apart(theirs, &ours)));
                eprintln!("{reader}, page {page}: {:?}", found.last().map(|(.., apart)| apart));
            }
        }
        found
    }

    /// The text a viewer extracts from a file: `pdftotext` (Poppler's or xpdf's), a line per
    /// line found, in page layout. Nothing where it is not installed.
    pub(crate) fn text_of(file: &[u8]) -> Option<String> {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("taut-text-{}-{}.pdf", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(&path, file).ok()?;
        let ran = Command::new("pdftotext").args(["-layout", "-enc", "UTF-8"]).arg(&path).arg("-").output();
        let _ = std::fs::remove_file(&path);
        let ran = ran.ok().filter(|ran| ran.status.success())?;
        // (The direction marks some extractors put around reversed text are not part of it.)
        let said: String = String::from_utf8_lossy(&ran.stdout).chars().filter(|letter| !matches!(letter, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect();
        Some(said.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n"))
    }

    /// Lines of Latin, Arabic and Chinese, each in a font that has them (as far as installed),
    /// on a page.
    pub(crate) fn scripts() -> (Paper, Vec<&'static str>) {
        use crate::text::{Face, Font};
        let mut paper = Paper::new();
        paper.page(crate::geom::Size { w: 960.0, h: 460.0 });
        let mut written = Vec::new();
        // (The last in a variable font at a non-default position: a Type 3 font.)
        let varied = super::tests::font_at(&["C:/Windows/Fonts/SegUIVar.ttf", "/mnt/c/Windows/Fonts/SegUIVar.ttf"]).map(|font| font.at(b"wght", 700.0));
        let lines = [
            (super::tests::latin(), "Office files: fine Ångström AV Wa To, — 100% “done”"),
            (super::tests::arabic(), "مرحبا بالعالم"),
            (super::tests::chinese(), "你好，世界。文字可以复制"),
            (varied, "Drawn glyphs, set heavier: Quartz 0123"),
        ];
        for (n, (font, line)) in lines.into_iter().enumerate() {
            let Some(font) = font else { continue };
            let face = Face::named(&format!("Script {n}"));
            paper.font(face, font);
            paper.text(line, 30.0, 30.0 + 100.0 * n as f32, Font { face, ..Font::new(32.0) }, crate::color::Color::BLACK);
            written.push(line);
        }
        (paper, written)
    }

    #[test]
    fn a_reader_takes_out_the_text_that_was_written() {
        let (paper, written) = scripts();
        let Some(read) = text_of(&paper.pdf()) else { return eprintln!("no pdftotext here: not read back") };
        let lines: Vec<&str> = read.lines().collect();
        // (Arabic is shaped and reordered only with `right-to-left`: without it, an extractor
        // reads its letters in display order.)
        let arabic = |line: &&str| line.chars().any(|letter| ('؀'..='ۿ').contains(&letter));
        let expected: Vec<&str> = written.iter().copied().filter(|line| cfg!(feature = "right-to-left") || !arabic(line)).collect();
        let found: Vec<&str> = lines.iter().copied().filter(|line| cfg!(feature = "right-to-left") || !arabic(line)).collect();
        assert_eq!(found, expected, "the reader takes the text out as it was written");
        assert!(written.len() >= 3, "fonts for all three scripts were here: {}", written.len());
    }

    #[test]
    fn readers_draw_text_in_every_script_as_taut_does() {
        let (paper, _) = scripts();
        for (reader, page, apart) in compared(&paper, 2.0) {
            let (mean, far) = apart.expect("the same size");
            assert!(mean < 2.0 && far < 0.02, "{reader}, page {page}: {mean} {far}");
        }
    }

    #[test]
    fn readers_draw_the_sample_as_taut_does() {
        let found = compared(&super::tests::sample(), 2.0);
        if found.is_empty() {
            return eprintln!("no reader of PDF files here: not compared");
        }
        for (reader, page, apart) in found {
            let (mean, far) = apart.expect("the same size");
            assert!(mean < 1.5 && far < 0.01, "{reader}, page {page}: {mean} {far}");
        }
    }
}
