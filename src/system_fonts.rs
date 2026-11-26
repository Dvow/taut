//! System fonts: any by name, the family list for a font picker, and fallback fonts for
//! characters a face's font lacks.
//!
//! A system font is requested by family name and style: weight and italic
//! (`Face::named("Georgia").italic()`). Named faces are kept in one program-wide list, so a
//! [`Face`] stays a small plain value: its index in that list. The system finds the font:
//! Windows by name, synthesizing missing weights and italics; Linux by asking Fontconfig for
//! the closest match.
//!
//! No font has every character. Where a face's font lacks one, it is drawn in the font the
//! system would fall back to, at the same size and on the same baseline. On Linux Fontconfig
//! says which (`x11`). On Windows it is chosen here, as Windows' own text rendering chooses: a
//! character of a script Windows assigns its own font (Devanagari to Nirmala UI, Thai to
//! Leelawadee UI) in that font; any other in the first font the registry links the face's font
//! to (`FontLink\SystemLink`; for an unlinked font, Segoe UI's links) that has it; a
//! supplementary-plane character in the font the registry names for its plane
//! (`LanguagePack\SurrogateFallback`); failing all, any system font that has it. (The
//! script-to-font table isn't in the registry: Windows keeps it internally. It is given here as
//! Windows 11 has it, and the tests check it against DirectWrite.)
//!
//! A line mixing fonts keeps its face's line metrics: every glyph sits on one baseline, and a
//! fallback font with a taller line is scaled down to fit (as Windows does with linked fonts);
//! what still overflows the line is clipped, so nothing is left behind on redraw.
//!
//! A program whose text is all in its fonts pays nothing: ASCII lines are not scanned, and a
//! font's character map is read only when a non-ASCII character is first checked.
//!
//! On Windows the choosing is done here only with the feature `fallback-fonts`: a program can't
//! tell from its calls whether users will type such text. Without it, a window's text still
//! gets those characters from Windows' linked fonts, measured and drawn by GDI;
//! [`SystemText`](crate::platform::SystemText), which rasterizes glyph by glyph into the
//! program's own pixels, and drawing text (`file_for`) then show the notdef box for them.
use crate::text::Face;
use std::sync::{Mutex, MutexGuard};

/// A named system font face: its index among the program's named faces. ([`Face::named`]
/// returns one; the same name and style always give the same one.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Named([u8; 2]);

/// A family style: weight and italic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    /// Weight, 100 to 900: 300 light, 400 regular, 600 semibold, 700 bold, 900 black.
    pub weight: u16,
    /// Italic or oblique.
    pub italic: bool,
}

impl Style {
    /// Upright, regular weight.
    pub const REGULAR: Style = Style { weight: 400, italic: false };
}

/// A face other than the three system faces: a family by name, or a system face's font, in a
/// style.
#[derive(Clone, Copy, PartialEq)]
struct Kept {
    /// The family, by name index (see `family`); `None` for the system font of face `of`.
    family: Option<u16>,
    /// The system face whose font is used if the family isn't installed (or none was named):
    /// one of the three.
    of: Face,
    style: Style,
    /// Scale factor below the requested size: under 1 for a fallback font whose line is taller
    /// than the face's.
    scale: f32,
}

/// The faces named so far, by index. An index is never reused, so a face keeps its meaning for
/// the program's life: past the 65,536th, a new face is its kind's system font (see `keep`).
/// Each is a few bytes, so all of them come to a megabyte at most.
static NAMED: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// The family names of the faces named so far, each once, by index.
static FAMILIES: Mutex<Vec<Box<str>>> = Mutex::new(Vec::new());

/// The longest family name kept, in bytes. No system names a longer one: Windows takes 31
/// characters, and of 571 families Fontconfig listed on an Arch install the longest was 36
/// bytes. A longer name gets the system font, as the system does for a missing family.
const LONGEST: usize = 128;

/// How many family names are kept: beyond that, a face of a new family gets the system font.
/// (Several times any system's family count; with `LONGEST`, names take half a megabyte at
/// most, however many an app or document names.)
const FAMILIES_KEPT: usize = 4096;

/// A family name's index (the same name, the same index), kept from now on; `None` for a name
/// too long to be a family's, or past the limit.
fn family(name: &str) -> Option<u16> {
    let mut families = held(&FAMILIES);
    if let Some(at) = families.iter().position(|known| **known == *name) {
        return Some(at as u16);
    }
    if name.len() > LONGEST || families.len() >= FAMILIES_KEPT {
        return None;
    }
    families.push(name.into());
    Some(families.len() as u16 - 1)
}

/// A family's name, by index.
fn name(family: u16) -> String {
    held(&FAMILIES).get(usize::from(family)).map_or_else(String::new, |name| name.to_string())
}

/// Locks a program-wide list, even if a thread holding it panicked.
fn held<T>(list: &Mutex<T>) -> MutexGuard<'_, T> {
    list.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Face {
    /// A system font by family name ("Georgia", "Noto Serif"), upright and regular. If the
    /// system lacks the family, the system's text font is used instead (`families` lists what
    /// it has).
    pub fn named(family: &str) -> Face {
        keep(Kept { family: self::family(family), of: Face::Regular, style: Style::REGULAR, scale: 1.0 })
    }
    /// The same font, italic: the family's italic where the system has one, else synthesized.
    pub fn italic(self) -> Face {
        let kept = kept(self);
        keep(Kept { style: Style { italic: true, ..kept.style }, ..kept })
    }
    /// The same font at this weight (100 to 900, 400 regular, 700 bold): that weight's file if
    /// the system has one, else the nearest.
    pub fn weight(self, weight: u16) -> Face {
        let kept = kept(self);
        keep(Kept { style: Style { weight: weight.clamp(1, 1000), ..kept.style }, ..kept })
    }
    /// The face's weight and italic.
    pub fn style(self) -> Style {
        kept(self).style
    }
    /// The face as a unique number: 0 to 3 for the four system faces, from 4 the named ones.
    /// (Cached text is keyed by it.)
    pub(crate) fn number(self) -> u32 {
        match self {
            Face::Regular => 0,
            Face::Bold => 1,
            Face::Mono => 2,
            Face::Italic => 3,
            Face::Named(Named(number)) => 4 + u32::from(u16::from_le_bytes(number)),
        }
    }
}

/// The face for `kept`: an existing index, or a new one. (Past the 65,536th, the system face it
/// is based on.)
fn keep(kept: Kept) -> Face {
    #[cfg(target_os = "android")]
    crate::android_text::hand_named();
    let mut named = held(&NAMED);
    let at = named.iter().position(|known| *known == kept).unwrap_or(named.len());
    if at > usize::from(u16::MAX) {
        return kept.of;
    }
    if at == named.len() {
        named.push(kept);
    }
    Face::Named(Named((at as u16).to_le_bytes()))
}

/// A face's data: for one of the four system faces, its font in its default style.
fn kept(face: Face) -> Kept {
    let own = |style| Kept { family: None, of: face, style, scale: 1.0 };
    match face {
        Face::Named(Named(number)) => held(&NAMED).get(usize::from(u16::from_le_bytes(number))).copied().unwrap_or(own(Style::REGULAR)),
        Face::Bold => own(Style { weight: BOLD, italic: false }),
        Face::Italic => own(Style { italic: true, ..Style::REGULAR }),
        _ => own(Style::REGULAR),
    }
}

/// The bold face's weight: Windows' bold face is Segoe UI made a little heavier.
const BOLD: u16 = if cfg!(windows) { 600 } else { 700 };

/// The scale for a fallback font so its line is at most as tall as the face's line: from the
/// two line heights at the same size. (Windows and Linux scale fallback glyphs this way.)
#[cfg(any(test, windows, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
pub(crate) fn fit(face_line: f32, own_line: f32) -> f32 {
    if own_line > face_line && own_line > 0.0 { face_line / own_line } else { 1.0 }
}

/// The rows of a fallback glyph's picture that lie within the face's line, `ascent` pixels above
/// the baseline to `descent` below (its `top` row is that many above it), so nothing it
/// overhangs is left behind on redraw: its new top, and those rows' coverage.
#[cfg(any(windows, all(unix, not(target_os = "android"))))]
pub(crate) fn within_line(top: i32, width: usize, cover: &[u8], [ascent, descent]: [i32; 2]) -> (i32, &[u8]) {
    let rows = cover.len().checked_div(width).unwrap_or(0) as i32;
    let (first, end) = ((top - ascent).clamp(0, rows), (top + descent).clamp(0, rows));
    let kept = cover.get(first as usize * width..(end.max(first) as usize) * width).unwrap_or(&[]);
    (top - first, kept)
}

/// How many glyph pictures each cache of the system's text keeps.
#[cfg(any(windows, all(unix, not(target_os = "android"))))]
pub(crate) const GLYPHS: usize = 8192;

/// The value `cache` keeps under `key`, made by `make` if missing. A cache holding `most` is
/// dropped first: what it held is made again on demand.
#[cfg(any(windows, all(unix, not(target_os = "android"))))]
pub(crate) fn cached<K: Eq + std::hash::Hash, V>(cache: &mut std::collections::HashMap<K, V>, most: usize, key: K, make: impl FnOnce() -> V) -> &mut V {
    // (A new map, not `clear`: it frees the table too, and costs less code.)
    if cache.len() >= most {
        *cache = std::collections::HashMap::new();
    }
    cache.entry(key).or_insert_with(make)
}

/// Whether every font is taken to have a character, which is never looked up elsewhere: ASCII,
/// and anything invisible.
#[cfg(windows)]
fn plain(letter: char) -> bool {
    letter.is_ascii() || letter.is_control()
}

/// The system's font families, sorted by name: for a font picker. (On Windows those its own
/// font dialog lists; on Linux those Fontconfig knows, by their first name. On the web those
/// the browser lists, once it has answered the first call: Chrome and Edge do, after asking the
/// person; others list none.)
pub fn families() -> Vec<String> {
    let mut families: Vec<String> = listed(None).into_iter().map(|(family, _)| family).collect();
    families.sort_by_key(|family| family.to_lowercase());
    families.dedup();
    families
}

/// The styles the system has of a family, lightest first, upright before italic; empty for a
/// missing family.
pub fn styles(family: &str) -> Vec<Style> {
    let mut styles: Vec<Style> = listed(Some(family)).into_iter().filter(|(of, _)| of.eq_ignore_ascii_case(family)).map(|(_, style)| style).collect();
    styles.sort_by_key(|style| (style.weight, style.italic));
    styles.dedup();
    styles
}

/// The system's fonts, each family and style: all, or one family's.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    crate::x11::listed(family.map(escaped).as_deref())
}

/// The system's fonts on Android: the named families of `fonts.xml`.
#[cfg(target_os = "android")]
fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    crate::android_text::listed(family)
}

/// The fonts CoreText has.
#[cfg(target_vendor = "apple")]
fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    crate::core_text::listed(family)
}

/// The person's fonts, as the browser lists them: none until it answers, none where it lists
/// none.
#[cfg(target_family = "wasm")]
fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    crate::web_fonts::listed(family)
}

/// A style by its name ("Bold Italic", "SemiBold", "Light Oblique"): the weight its words name,
/// and whether it slants.
#[cfg(any(test, target_family = "wasm"))]
pub(crate) fn style_named(name: &str) -> Style {
    const WEIGHTS: [(&str, u16); 14] = [
        ("hairline", 100),
        ("thin", 100),
        ("extralight", 200),
        ("ultralight", 200),
        ("semilight", 350),
        ("light", 300),
        ("medium", 500),
        ("semibold", 600),
        ("demibold", 600),
        ("extrabold", 800),
        ("ultrabold", 800),
        ("bold", 700),
        ("black", 900),
        ("heavy", 900),
    ];
    let name = name.to_ascii_lowercase().replace([' ', '-'], "");
    let weight = WEIGHTS.iter().find(|(word, _)| name.contains(word)).map_or(400, |(_, weight)| *weight);
    Style { weight, italic: name.contains("italic") || name.contains("oblique") }
}

/// The font a face draws a character with, read from the system's file: the face's own, or its
/// fallback if that lacks the character, as system text does. For drawings that render text in
/// system fonts. `None` if the system provides no file.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) fn file_for(face: Face, letter: char) -> Option<crate::font_file::FontFile> {
    crate::x11::file_for(face, letter).and_then(crate::font_file::FontFile::open)
}

/// The same on Android: the first of the system's fonts that has it.
#[cfg(target_os = "android")]
pub(crate) fn file_for(_face: Face, letter: char) -> Option<crate::font_file::FontFile> {
    crate::android_text::file_for(letter)
}

/// The same from the file CoreText draws the character from.
#[cfg(target_vendor = "apple")]
pub(crate) fn file_for(face: Face, letter: char) -> Option<crate::font_file::FontFile> {
    crate::core_text::file_for(face, letter).and_then(crate::font_file::FontFile::open)
}

/// How CoreText, a browser or Android's font matcher is asked for a face's font: the family
/// named, if any (else the system's font for face `of`), and the style.
#[cfg(any(target_vendor = "apple", target_family = "wasm", target_os = "android"))]
pub(crate) fn asked(face: Face) -> (Option<String>, Face, Style) {
    let kept = kept(face);
    (kept.family.map(name), kept.of, kept.style)
}

/// The same where system fonts are unknown.
#[cfg(not(any(windows, unix)))]
pub(crate) fn file_for(_face: Face, _letter: char) -> Option<crate::font_file::FontFile> {
    None
}

/// The same on Windows, where GDI provides a font's tables (see `tables`). (The CSS generic
/// families map to the fonts Windows browsers use for them.) Of the families `asked` names, the
/// first installed; at a weight a family ships as a separately named family ("Segoe UI
/// Semibold"), that one, if installed: GDI would synthesize it from the regular. Its glyphs
/// are read from GDI as they are drawn (see `font_file::Fetched`).
#[cfg(windows)]
pub(crate) fn file_for(face: Face, letter: char) -> Option<crate::font_file::FontFile> {
    let asked = asked(face_for(face, letter));
    let general = |name: &String| match name.to_ascii_lowercase().as_str() {
        "sans-serif" | "system-ui" => "Arial".to_string(),
        "serif" => "Times New Roman".to_string(),
        "monospace" => "Courier New".to_string(),
        "cursive" => "Comic Sans MS".to_string(),
        "fantasy" => "Impact".to_string(),
        _ => name.clone(),
    };
    let mut tried = asked.names.iter().map(general).flat_map(|name| [weighted(&name, asked.weight).map(|named| (named, 400)), Some((name, asked.weight))].into_iter().flatten());
    // SAFETY: a program-owned DC, freed with `selected`.
    let mut selected = Selected { dc: unsafe { CreateCompatibleDC(0) }, font: 0, one_at_a_time: Mutex::new(()) };
    // SAFETY: each font is created and selected into that DC; one not installed is released.
    selected.font = tried.find_map(|(name, weight)| unsafe { named(selected.dc, &name, weight, asked.italic) })?;
    let (file, glyphs) = tables(selected.dc)?;
    let read: Box<crate::font_file::TablePart> = Box::new(move |table, at, long| selected.read(table, at, long));
    crate::font_file::FontFile::assembled(file, (glyphs > 0).then_some((glyphs, read)))
}

/// A system font selected into a DC of its own, its tables read as asked, by one thread at a
/// time. Both are freed with it.
#[cfg(windows)]
struct Selected {
    dc: isize,
    font: isize,
    one_at_a_time: Mutex<()>,
}

#[cfg(windows)]
impl Selected {
    /// `long` bytes from byte `at` of a table of the font.
    fn read(&self, table: &[u8; 4], at: usize, long: usize) -> Option<Vec<u8>> {
        let _alone = held(&self.one_at_a_time);
        let mut part = vec![0u8; long];
        // SAFETY: Windows writes at most the room it is told of.
        let read = long == 0 || unsafe { GetFontData(self.dc, u32::from_le_bytes(*table), at as u32, part.as_mut_ptr(), long as u32) } as usize == long;
        read.then_some(part)
    }
}

#[cfg(windows)]
impl Drop for Selected {
    fn drop(&mut self) {
        // SAFETY: the DC and font are this one's; the font is deselected (13, `SYSTEM_FONT`)
        // before it is deleted.
        unsafe {
            SelectObject(self.dc, GetStockObject(13));
            DeleteObject(self.font);
            DeleteDC(self.dc);
        }
    }
}

/// The GDI family name a family uses for a weight it ships separately: the weight's name after
/// the family's. (Regular and bold are the family's own.)
#[cfg(windows)]
fn weighted(name: &str, weight: i32) -> Option<String> {
    let style = match weight {
        ..150 => "Thin",
        150..250 => "ExtraLight",
        250..325 => "Light",
        325..375 => "Semilight",
        450..550 => "Medium",
        550..650 => "Semibold",
        750..850 => "ExtraBold",
        850.. => "Black",
        _ => return None,
    };
    Some(format!("{name} {style}"))
}

/// A family's font at a weight, selected into a DC: if installed. (If not, GDI substitutes
/// another, which is released.)
///
/// # Safety
/// `dc` is a program-owned device context.
#[cfg(windows)]
unsafe fn named(dc: isize, name: &str, weight: i32, italic: bool) -> Option<isize> {
    let mut put = [0u16; 64];
    // SAFETY: as the caller promises; GDI writes at most the name buffer's length.
    unsafe {
        let font = CreateFontW(-MEASURED, 0, 0, 0, weight, italic as u32, 0, 0, 1, 0, 0, 0, 0, wide(name).as_ptr());
        let before = SelectObject(dc, font);
        let long = GetTextFaceW(dc, put.len() as i32, put.as_mut_ptr()).clamp(0, put.len() as i32) as usize;
        if String::from_utf16_lossy(&put[..long]).trim_end_matches('\0').eq_ignore_ascii_case(name) {
            return Some(font);
        }
        SelectObject(dc, before);
        DeleteObject(font);
        None
    }
}

/// The DC's font as a standalone font file of the tables read (`font_file::taken`), each
/// fetched by tag: for a font in a collection, not the others (the whole collection makes an
/// East Asian font half as large again). `glyf` is listed empty, to be read a glyph at a time:
/// its length comes with the file. `None` if Windows provides no tables, or the directory
/// claims more than the file holds.
#[cfg(windows)]
fn tables(dc: isize) -> Option<(Vec<u8>, usize)> {
    // SAFETY: Windows writes at most the room it is told of.
    let read = |table: &[u8; 4], from: usize, into: &mut [u8]| unsafe { GetFontData(dc, u32::from_le_bytes(*table), from as u32, into.as_mut_ptr(), into.len() as u32) } as usize == into.len();
    // The table directory: 12 bytes, then 16 per table, tag first and length last. (Table 0 is
    // where the font starts, in a collection too.)
    let mut head = [0u8; 12];
    read(&[0; 4], 0, &mut head).then_some(())?;
    let mut list = vec![0u8; 16 * usize::from(u16::from_be_bytes([head[4], head[5]]))];
    read(&[0; 4], 12, &mut list).then_some(())?;
    let long = |name: &[u8; 4]| list.as_chunks::<16>().0.iter().find(|entry| entry.starts_with(name)).map(|entry| u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize);
    let tables: Vec<(&[u8; 4], usize)> = crate::font_file::taken(|name| long(name).is_some()).map(|name| (name, long(name).unwrap_or(0))).collect();
    // The file length: for a collection, all of it. Given no buffer, Windows reports the size.
    // SAFETY: Windows is given no buffer, and writes nothing.
    let whole = [*b"ttcf", [0; 4]].map(|table| unsafe { GetFontData(dc, u32::from_le_bytes(table), 0, std::ptr::null_mut(), 0) });
    let all = tables.iter().map(|(_, long)| long).sum::<usize>();
    if whole.iter().all(|whole| *whole == u32::MAX || all > *whole as usize) {
        return None;
    }
    let glyphs = long(b"glyf").unwrap_or(0);
    let kept = tables.iter().map(|(name, long)| (*name, if *name == b"glyf" { 0 } else { *long }));
    let mut file = crate::font_file::listing(&head[..4], kept.clone());
    file.reserve_exact(all - glyphs);
    for (name, long) in kept {
        let at = file.len();
        file.resize(at + long, 0);
        (long == 0 || read(name, 0, &mut file[at..])).then_some(())?;
    }
    Some((file, glyphs))
}

// ---- Linux: what Fontconfig is asked ----

/// Fontconfig's weights: its value, its name, and the 100 to 900 weight of each.
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
const WEIGHTS: [(u16, u8, &str); 12] = [
    (100, 0, "thin"),
    (200, 40, "extralight"),
    (300, 50, "light"),
    (350, 55, "demilight"),
    (380, 75, "book"),
    (400, 80, "regular"),
    (500, 100, "medium"),
    (600, 180, "demibold"),
    (700, 200, "bold"),
    (800, 205, "extrabold"),
    (900, 210, "black"),
    (950, 215, "extrablack"),
];

/// A Fontconfig weight on the 100 to 900 scale: the nearest named one.
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
pub(crate) fn weight_of(fontconfig: i32) -> u16 {
    WEIGHTS.iter().min_by_key(|(_, own, _)| (i32::from(*own) - fontconfig).abs()).map_or(400, |(weight, ..)| *weight)
}

/// The Fontconfig pattern for a face: a family name, or the desktop's generic ("sans",
/// "monospace"), with the style.
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
pub(crate) fn pattern(face: Face) -> String {
    let kept = kept(face);
    let family = kept.family.map(name);
    pattern_of(family.as_deref().or(Some(if kept.of == Face::Mono { "monospace" } else { "sans" })), kept.style)
}

/// A family and style as a Fontconfig pattern: "Noto Serif:weight=bold:slant=italic".
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
fn pattern_of(family: Option<&str>, style: Style) -> String {
    let (_, _, weight) = WEIGHTS.iter().min_by_key(|(own, ..)| own.abs_diff(style.weight)).copied().unwrap_or(WEIGHTS[5]);
    escaped(family.unwrap_or_default()) + ":weight=" + weight + if style.italic { ":slant=italic" } else { "" }
}

/// A family name escaped for Fontconfig: its special characters backslash-escaped.
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
fn escaped(family: &str) -> String {
    let mut escaped = String::with_capacity(family.len());
    for letter in family.chars() {
        if matches!(letter, '\\' | '-' | ':' | ',' | '=') {
            escaped.push('\\');
        }
        escaped.push(letter);
    }
    escaped
}

// ---- Windows: how a face is asked for, and the fonts that stand in ----

/// How a face's font is requested from Windows: the family names to try (the first installed is
/// used), weight, italic, and the scale below the requested size.
#[cfg(windows)]
pub(crate) struct Asked {
    /// Family names to try, in order.
    pub names: Vec<String>,
    /// Weight, as GDI counts it (as [`Style`] does).
    pub weight: i32,
    /// Italic.
    pub italic: bool,
    /// The scale below the requested size (see `fit`).
    pub scale: f32,
}

/// The fonts of the system faces, the first installed of each: Segoe UI, and for code the newer
/// monospace face where present. (Not every Windows has it; all have the older ones.)
#[cfg(windows)]
fn own_names(face: Face) -> &'static [&'static str] {
    match face {
        Face::Mono => &["Cascadia Mono", "Consolas", "Courier New"],
        _ => &["Segoe UI", "Tahoma"],
    }
}

/// How a face's font is requested from Windows. (A missing family falls back to its face's
/// system font.)
#[cfg(windows)]
pub(crate) fn asked(face: Face) -> Asked {
    let kept = kept(face);
    let names = kept.family.map(name).into_iter().chain(own_names(kept.of).iter().map(|name| name.to_string())).collect();
    Asked { names, weight: i32::from(kept.style.weight), italic: kept.style.italic, scale: kept.scale }
}

/// The face a character draws in: `face` if its font has it, else the fallback font's face (as
/// Windows would choose). `face` itself for a character no system font has.
#[cfg(windows)]
pub(crate) fn face_for(face: Face, letter: char) -> Face {
    if plain(letter) || !cfg!(feature = "fallback-fonts") {
        return face;
    }
    let mut learned = held(&LEARNED);
    learned.get_or_insert_with(Learned::new).face_for(face, &[letter])
}

/// The same face at the size asked for: for a fallback face (see `face_for`), not fitted to the
/// line of the face it stands in for.
#[cfg(all(windows, feature = "shaping"))]
pub(crate) fn full_size(face: Face) -> Face {
    let kept = kept(face);
    if kept.scale == 1.0 {
        return face;
    }
    keep(Kept { scale: 1.0, ..kept })
}

/// Splits a line into runs by drawing face, where the face's font lacks some characters: each
/// with its face (`face`, or a fallback), in order. `false` with no runs if the font has every
/// character: an ASCII line isn't even scanned. A grapheme cluster (see
/// [`text::cluster`](crate::text::cluster)) is never split: it draws in the first font that has
/// all of it.
#[cfg(windows)]
pub(crate) fn pieces(text: &str, face: Face, pieces: &mut Vec<(std::ops::Range<usize>, Face)>) -> bool {
    pieces.clear();
    if text.is_ascii() || !cfg!(feature = "fallback-fonts") {
        return false;
    }
    let mut learned = held(&LEARNED);
    let learned = learned.get_or_insert_with(Learned::new);
    let (mut at, mut letters) = (0, Vec::new());
    while at < text.len() {
        let end = at + crate::text::cluster(&text[at..]).max(1);
        letters.clear();
        letters.extend(text[at..end].chars());
        let written = if letters.iter().copied().all(plain) { face } else { learned.face_for(face, &letters) };
        match pieces.last_mut() {
            Some((last, was)) if *was == written => last.end = end,
            _ => pieces.push((at..end, written)),
        }
        at = end;
    }
    let mixed = pieces.iter().any(|(_, written)| *written != face);
    if !mixed {
        pieces.clear();
    }
    mixed
}

/// The scripts Windows draws in their own fonts, whatever font was requested: each range's
/// first and last character, and the font (in `SCRIPT_FONTS`). As Windows 11 has them
/// (DirectWrite and Uniscribe agree).
#[cfg(any(test, windows))]
#[rustfmt::skip]
const SCRIPTS: [(u32, u32, u8); 55] = [
    (0x700, 0x74f, 0), (0x780, 0x7b1, 1), (0x7c0, 0x7ff, 2), (0x900, 0xdf4, 3), (0xe01, 0xedf, 4), (0xf00, 0xfda, 5), (0x1000, 0x109f, 6), (0x1200, 0x1399, 2),
    (0x13a0, 0x167f, 7), (0x1680, 0x16f8, 0), (0x1780, 0x17f9, 4), (0x1800, 0x18aa, 8), (0x18b0, 0x18f5, 7), (0x1950, 0x1974, 9), (0x1980, 0x19df, 10), (0x19e0, 0x1a1f, 4),
    (0x1c50, 0x1c7f, 3), (0x1cd0, 0x1cfa, 3), (0x1dcd, 0x1dcd, 2), (0x202f, 0x202f, 8), (0x2c00, 0x2cff, 0), (0x2d30, 0x2dde, 2), (0xa000, 0xa4c6, 11), (0xa500, 0xa6f7, 2),
    (0xa830, 0xa839, 3), (0xa840, 0xa877, 12), (0xa8e0, 0xa8ff, 3), (0xa980, 0xa9df, 13), (0xa9e0, 0xa9fe, 6), (0xaa60, 0xaa7f, 6), (0xab01, 0xab2e, 2), (0xab70, 0xabbf, 7),
    (0x10280, 0x102d0, 0), (0x10300, 0x1034a, 0), (0x10380, 0x103d5, 0), (0x10450, 0x1047f, 0), (0x10480, 0x104a9, 2), (0x104b0, 0x104fb, 7), (0x10800, 0x1085f, 0), (0x10900, 0x1093f, 0),
    (0x109a0, 0x109b7, 0), (0x109be, 0x109bf, 0), (0x10a00, 0x10a7f, 0), (0x10b40, 0x10b7f, 0), (0x10c00, 0x10c48, 0), (0x11000, 0x1107f, 0), (0x110d0, 0x11147, 3), (0x111e1, 0x111f4, 3),
    (0x11660, 0x1166c, 8), (0x11ab0, 0x11abf, 7), (0x12000, 0x12474, 0), (0x13000, 0x13438, 0), (0x16800, 0x16a38, 0), (0x1e7e0, 0x1e7fe, 2), (0x1e900, 0x1e95f, 2),
];

/// The fonts of `SCRIPTS`.
#[cfg(any(test, windows))]
const SCRIPT_FONTS: [&str; 14] = [
    "Segoe UI Historic",
    "MV Boli",
    "Ebrima",
    "Nirmala UI",
    "Leelawadee UI",
    "Microsoft Himalaya",
    "Myanmar Text",
    "Gadugi",
    "Mongolian Baiti",
    "Microsoft Tai Le",
    "Microsoft New Tai Lue",
    "Microsoft Yi Baiti",
    "Microsoft PhagsPa",
    "Javanese Text",
];

/// The font Windows draws a character's script in, if it is one of `SCRIPTS`.
#[cfg(any(test, windows))]
fn script_font(letter: char) -> Option<&'static str> {
    let after = SCRIPTS.partition_point(|(_, last, _)| *last < letter as u32);
    SCRIPTS.get(after).filter(|(first, ..)| *first <= letter as u32).map(|(.., font)| SCRIPT_FONTS[usize::from(*font)])
}

/// A font's characters from its `cmap` table, as sorted ranges (first and last). For a symbol
/// font, whose glyphs start at U+F000, also the ASCII characters: Windows maps those to them.
#[cfg(any(test, windows))]
fn letters_of(cmap: &[u8]) -> Vec<[u32; 2]> {
    let read16 = |at: usize| cmap.get(at..at + 2).map_or(0, |two| u32::from(u16::from_be_bytes([two[0], two[1]])));
    let read32 = |at: usize| cmap.get(at..at + 4).map_or(0, |four| u32::from_be_bytes([four[0], four[1], four[2], four[3]]));
    // The best subtable: full Unicode (12), else the BMP (4); a symbol font's own last.
    let rank = |(platform, encoding, format): (u32, u32, u32)| match (platform, encoding, format) {
        (3, 10, 12) | (0, _, 12) => 4,
        (3, 1, 4) | (0, _, 4) => 3,
        (3, 0, 4) => 2,
        _ => 0,
    };
    let lists = (0..read16(2) as usize).map(|n| (read16(4 + n * 8), read16(6 + n * 8), read32(8 + n * 8)));
    let best = lists.map(|(platform, encoding, at)| (rank((platform, encoding, read16(at as usize))), platform == 3 && encoding == 0, at as usize)).max_by_key(|(rank, ..)| *rank);
    let Some((rank, symbols, at)) = best.filter(|(rank, ..)| *rank > 0) else { return Vec::new() };
    let mut letters = if rank == 4 { stretches_12(cmap, at) } else { stretches_4(cmap, at) };
    if symbols {
        let plain: Vec<[u32; 2]> = letters.iter().filter(|[first, last]| *first <= 0xf0ff && *last >= 0xf000).map(|[first, last]| [first.max(&0xf000) - 0xf000, last.min(&0xf0ff) - 0xf000]).collect();
        letters.extend(plain);
        letters = in_order(letters);
    }
    joined(letters)
}

/// The ranges of a format 4 subtable: segments, each mapping a range one way, excluding
/// characters that map to glyph 0.
#[cfg(any(test, windows))]
fn stretches_4(cmap: &[u8], at: usize) -> Vec<[u32; 2]> {
    let read16 = |at: usize| cmap.get(at..at + 2).map_or(0, |two| u32::from(u16::from_be_bytes([two[0], two[1]])));
    let segments = read16(at + 6) as usize / 2;
    let (ends, starts, deltas, offsets) = (at + 14, at + 16 + segments * 2, at + 16 + segments * 4, at + 16 + segments * 6);
    let mut letters = Vec::new();
    for n in 0..segments {
        let (first, last, delta, offset) = (read16(starts + n * 2), read16(ends + n * 2), read16(deltas + n * 2), read16(offsets + n * 2) as usize);
        // The terminating segment at U+FFFF holds no character.
        if first > last || first == 0xffff {
            continue;
        }
        // Delta mapping: only the character landing on glyph 0 is missing. Glyph array: each
        // entry that is 0.
        let glyph = |letter: u32| match offset {
            0 => (letter + delta) & 0xffff,
            _ => read16(offsets + n * 2 + offset + (letter - first) as usize * 2),
        };
        letters.extend((first..=last.min(0xfffe)).filter(|letter| glyph(*letter) != 0).map(|letter| [letter, letter]));
    }
    joined(letters)
}

/// The ranges of a format 12 subtable: groups of consecutive glyphs, of which a group's first
/// character is missing if its glyph is 0.
#[cfg(any(test, windows))]
fn stretches_12(cmap: &[u8], at: usize) -> Vec<[u32; 2]> {
    let read32 = |at: usize| cmap.get(at..at + 4).map_or(0, |four| u32::from_be_bytes([four[0], four[1], four[2], four[3]]));
    let groups = (read32(at + 12) as usize).min(cmap.len() / 12);
    let group = |n: usize| [read32(at + 16 + n * 12), read32(at + 20 + n * 12), read32(at + 24 + n * 12)];
    let letters: Vec<[u32; 2]> = (0..groups).map(group).map(|[first, last, glyph]| [first.saturating_add(u32::from(glyph == 0)), last.min(0x10ffff)]).filter(|[first, last]| first <= last).collect();
    joined(in_order(letters))
}

/// Ranges sorted. (A heap sort: `sort_unstable`'s quicksort would be compiled in for these
/// alone, about 6 KB; this runs once per font examined.)
#[cfg(any(test, windows))]
fn in_order(letters: Vec<[u32; 2]>) -> Vec<[u32; 2]> {
    std::collections::BinaryHeap::from(letters).into_sorted_vec()
}

/// Sorted ranges, with touching or overlapping ones merged.
#[cfg(any(test, windows))]
fn joined(letters: Vec<[u32; 2]>) -> Vec<[u32; 2]> {
    let mut joined: Vec<[u32; 2]> = Vec::with_capacity(letters.len());
    for [first, last] in letters {
        match joined.last_mut() {
            Some(before) if first <= before[1].saturating_add(1) => before[1] = before[1].max(last),
            _ => joined.push([first, last]),
        }
    }
    joined.shrink_to_fit();
    joined
}

/// Whether a font has a character, by its ranges.
#[cfg(any(test, windows))]
fn has(letters: &[[u32; 2]], letter: char) -> bool {
    let after = letters.partition_point(|[_, last]| *last < letter as u32);
    letters.get(after).is_some_and(|[first, _]| *first <= letter as u32)
}

/// Character ranges, each with the first font having it (`first`, sorted), plus those of a
/// font's ranges (`letters`) none of them has yet, assigned to that font.
#[cfg(any(test, windows))]
fn adding(first: &[([u32; 2], u16)], letters: &[[u32; 2]], font: u16) -> Vec<([u32; 2], u16)> {
    // Both are sorted: each of the font's ranges is cut where earlier ones overlap it. `past`
    // is where the output so far ends.
    let (mut all, mut before, mut past) = (Vec::with_capacity(first.len() + letters.len()), first.iter().peekable(), 0);
    for [from, to] in letters.iter().copied() {
        let mut from = from.max(past);
        while let Some(&&([start, end], owner)) = before.peek().filter(|([start, _], _)| *start <= to) {
            if start > from {
                all.push(([from, start - 1], font));
            }
            all.push(([start, end], owner));
            (from, past) = (from.max(end + 1), end + 1);
            before.next();
        }
        if from <= to {
            all.push(([from, to], font));
            past = to + 1;
        }
    }
    all.extend(before);
    all
}

// ---- Windows: what it is asked about its fonts, and what is learned of them ----

#[cfg(windows)]
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: isize) -> isize;
    fn DeleteDC(dc: isize) -> i32;
    fn CreateFontW(
        height: i32,
        width: i32,
        turn: i32,
        lean: i32,
        weight: i32,
        italic: u32,
        under: u32,
        through: u32,
        set: u32,
        precision: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        name: *const u16,
    ) -> isize;
    fn SelectObject(dc: isize, object: isize) -> isize;
    fn DeleteObject(object: isize) -> i32;
    fn GetStockObject(which: i32) -> isize;
    fn GetTextFaceW(dc: isize, room: i32, name: *mut u16) -> i32;
    fn GetFontData(dc: isize, table: u32, from: u32, into: *mut u8, room: u32) -> u32;
    fn EnumFontFamiliesExW(dc: isize, asked: *const Listed, each: unsafe extern "system" fn(*const Listed, *const u8, u32, isize) -> i32, list: isize, flags: u32) -> i32;
}
#[cfg(windows)]
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(key: isize, under: *const u16, name: *const u16, kinds: u32, kind: *mut u32, value: *mut u8, bytes: *mut u32) -> i32;
}

/// A font as GDI enumerates it (`ENUMLOGFONTEXW`), and as enumeration is requested (its
/// `LOGFONTW` part): metrics (the fifth is the weight), flags (the first is italic, the fourth
/// the charset), and names.
#[cfg(windows)]
#[repr(C)]
struct Listed {
    sizes: [i32; 5],
    does: [u8; 8],
    name: [u16; 32],
    full: [u16; 64],
    style: [u16; 32],
    script: [u16; 32],
}

/// Text as Windows reads it: NUL-terminated UTF-16.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Text Windows wrote, up to its NUL.
#[cfg(windows)]
fn narrow(text: &[u16]) -> String {
    String::from_utf16_lossy(text.split(|unit| *unit == 0).next().unwrap_or_default())
}

/// Windows' fonts, each family and style, as GDI enumerates them (as Windows' font dialog
/// does): all, or one family's. Bitmap fonts, and vertical-writing variants (named with "@"),
/// are left out.
#[cfg(windows)]
fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    let mut asked = Listed { sizes: [0; 5], does: [0, 0, 0, 1, 0, 0, 0, 0], name: [0; 32], full: [0; 64], style: [0; 32], script: [0; 32] };
    family.into_iter().flat_map(|family| family.encode_utf16().take(31).enumerate()).for_each(|(at, unit)| asked.name[at] = unit);
    let mut list: Vec<(String, Style)> = Vec::new();
    // SAFETY: GDI calls `each` with every font during this call, passing it the list; the DC is
    // program-owned for the call's duration.
    unsafe {
        let dc = CreateCompatibleDC(0);
        EnumFontFamiliesExW(dc, &asked, each, &mut list as *mut Vec<(String, Style)> as isize, 0);
        DeleteDC(dc);
    }
    list
}

/// One font GDI enumerates: added to the list unless a raster font (1) or a vertical variant.
#[cfg(windows)]
unsafe extern "system" fn each(font: *const Listed, _: *const u8, kind: u32, list: isize) -> i32 {
    // SAFETY: GDI passes a fully described font, and the list `listed` gave it.
    let (font, list) = unsafe { (&*font, &mut *(list as *mut Vec<(String, Style)>)) };
    let name = narrow(&font.name);
    if kind & 1 == 0 && !name.starts_with('@') {
        list.push((name, Style { weight: font.sizes[4].clamp(1, 1000) as u16, italic: font.does[0] != 0 }));
    }
    1
}

/// A `HKEY_LOCAL_MACHINE` string value, as lines.
#[cfg(windows)]
fn registry(under: &str, name: &str) -> Option<Vec<String>> {
    let (under, name, mut bytes) = (wide(under), wide(name), 0u32);
    // SAFETY: Windows is first asked the value's size, then writes it into a buffer that size.
    // (0x22: RRF_RT_REG_SZ | RRF_RT_REG_MULTI_SZ.)
    unsafe {
        let machine = 0x8000_0002u32 as i32 as isize;
        if RegGetValueW(machine, under.as_ptr(), name.as_ptr(), 0x22, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes) != 0 {
            return None;
        }
        let mut value = vec![0u16; bytes as usize / 2 + 1];
        if RegGetValueW(machine, under.as_ptr(), name.as_ptr(), 0x22, std::ptr::null_mut(), value.as_mut_ptr().cast(), &mut bytes) != 0 {
            return None;
        }
        Some(value.split(|unit| *unit == 0).filter(|line| !line.is_empty()).map(String::from_utf16_lossy).collect())
    }
}

/// The fonts the registry links a font to (`FontLink\SystemLink`), each once, in order: each
/// line names a file and a font, some with scaling values after.
#[cfg(windows)]
fn links_of(name: &str) -> Option<Vec<Box<str>>> {
    let lines = registry(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontLink\SystemLink", name)?;
    let mut links: Vec<Box<str>> = Vec::new();
    for font in lines.iter().filter_map(|line| line.split(',').nth(1)).map(str::trim) {
        if !links.iter().any(|known| known.eq_ignore_ascii_case(font)) {
            links.push(font.into());
        }
    }
    Some(links)
}

/// The font the registry names for a supplementary plane's characters
/// (`LanguagePack\SurrogateFallback`: rare CJK ideographs).
#[cfg(windows)]
fn plane_font(letter: char) -> Option<String> {
    let plane = letter as u32 >> 16;
    let under = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\LanguagePack\SurrogateFallback";
    if plane == 0 {
        return None;
    }
    registry(under, &format!("Plane{plane}"))?.into_iter().next()
}

/// The height fonts are created at to read their tables.
#[cfg(windows)]
const MEASURED: i32 = 1024;

/// A Windows font, as far as examined.
#[cfg(windows)]
struct Known {
    /// Its family name, as GDI knows it.
    name: Box<str>,
    /// Installed.
    here: bool,
    /// Its characters (see `letters_of`).
    letters: Vec<[u32; 2]>,
    /// Its line height (ascender plus descender) per unit of size, from its `hhea` table: what
    /// both systems read for a line.
    line: f32,
}

/// What is known of the system's fonts, for the program's life: each is examined once.
#[cfg(windows)]
struct Learned {
    /// A program-owned DC for querying fonts.
    dc: isize,
    /// The fonts examined so far.
    fonts: Vec<Known>,
    /// Which of them each face draws with.
    owns: Vec<(Face, usize)>,
    /// The fonts the registry links each font to, by font name, as read.
    links: Vec<(Box<str>, Vec<Box<str>>)>,
    /// Faces made for a font as another face's fallback: by face and font.
    made: Vec<(Face, usize, Face)>,
    /// Fonts a full scan found, tried first next time.
    found: Vec<Box<str>>,
    /// The result of the one full scan: where each character range starts (low 21 bits, any
    /// character) whose first font is the same, in name order, with that font's index in
    /// `owners` in the high bits (`NONE` for a range no font has). `None` until a character is
    /// first searched in all fonts.
    first: Option<Vec<u32>>,
    /// The fonts first to have a character, of all the system's, by name.
    owners: Vec<Box<str>>,
    /// The font the registry names for each supplementary plane, as read.
    planes: Vec<(u32, Option<String>)>,
}

/// The bits of a `Learned::first` entry giving its start: any character fits.
#[cfg(windows)]
const LETTER: u32 = 0x1f_ffff;

/// The bits above those, all set: a range no font has.
#[cfg(windows)]
const NONE: u32 = !LETTER;

/// What is known: nothing until the first non-ASCII character.
#[cfg(windows)]
static LEARNED: Mutex<Option<Learned>> = Mutex::new(None);

// SAFETY: the DC is a program-owned handle any thread may use, one at a time: its holder is
// behind a lock.
#[cfg(windows)]
unsafe impl Send for Learned {}

#[cfg(windows)]
impl Learned {
    fn new() -> Learned {
        // SAFETY: a program-owned memory DC, on no window.
        let dc = unsafe { CreateCompatibleDC(0) };
        Learned { dc, fonts: Vec::new(), owns: Vec::new(), links: Vec::new(), made: Vec::new(), found: Vec::new(), first: None, owners: Vec::new(), planes: Vec::new() }
    }

    /// The face a grapheme cluster (`letters`) draws in, from `face`: `face` if its font has
    /// all of it, else the face of the fallback font for the first character the face's font
    /// lacks (if that font has all of it, or the face's font also lacks the first character).
    fn face_for(&mut self, face: Face, letters: &[char]) -> Face {
        let Some(own) = self.own(face) else { return face };
        let lacked = letters.iter().copied().find(|letter| !plain(*letter) && !has(&self.fonts[own].letters, *letter));
        let Some((lacked, other)) = lacked.and_then(|lacked| Some((lacked, self.stand_in(own, lacked)?))) else { return face };
        let all = letters.iter().all(|letter| plain(*letter) || has(&self.fonts[other].letters, *letter));
        if !all && lacked != letters[0] {
            return face;
        }
        self.standing(face, own, other)
    }

    /// The known font a face draws with: the first of its names installed. (`None` if none.)
    fn own(&mut self, face: Face) -> Option<usize> {
        if let Some((_, at)) = self.owns.iter().find(|(of, _)| *of == face) {
            return Some(*at);
        }
        let at = asked(face).names.iter().find_map(|name| self.font(name).filter(|at| self.fonts[*at].here))?;
        self.owns.push((face, at));
        Some(at)
    }

    /// The face of font `other` as fallback for `face`, whose own font is `own`: in the face's
    /// style, scaled down until its line fits the face's. (Made once.)
    fn standing(&mut self, face: Face, own: usize, other: usize) -> Face {
        if let Some((.., made)) = self.made.iter().find(|(of, by, _)| *of == face && *by == other) {
            return *made;
        }
        // Past the family limit, the character stays in the face's own font.
        let (kept, Some(family)) = (kept(face), family(&self.fonts[other].name)) else { return face };
        let scale = fit(self.fonts[own].line, self.fonts[other].line) * kept.scale;
        let made = keep(Kept { family: Some(family), scale, ..kept });
        self.made.push((face, other, made));
        made
    }

    /// The font for a character font `own` lacks, as Windows would choose: its script's font,
    /// the first of the fonts the registry links `own` to that has it, its plane's font, or any
    /// font that has it. `None` if none has it.
    fn stand_in(&mut self, own: usize, letter: char) -> Option<usize> {
        let script = script_font(letter).and_then(|name| self.font(name));
        if let Some(at) = script.filter(|at| self.has(*at, letter)) {
            return Some(at);
        }
        let linked = self.linked(own).iter().find_map(|name| self.font(name).filter(|at| self.has(*at, letter)));
        match linked {
            Some(at) => Some(at),
            None => self.plane(letter).or_else(|| self.any(letter)),
        }
    }

    /// Whether font `at` is installed and has a character.
    fn has(&self, at: usize, letter: char) -> bool {
        self.fonts[at].here && has(&self.fonts[at].letters, letter)
    }

    /// The fonts the registry links font `own` to, in order: under its own name, or Segoe UI's
    /// if it has none. (Each once, by name: the scaling values some entries add are left to
    /// `fit`.)
    fn linked(&mut self, own: usize) -> Vec<Box<str>> {
        let name = self.fonts[own].name.clone();
        if !self.links.iter().any(|(of, _)| *of == name) {
            let read = links_of(&name).or_else(|| links_of("Segoe UI")).unwrap_or_default();
            self.links.push((name.clone(), read));
        }
        self.links.iter().find(|(of, _)| *of == name).map(|(_, links)| links.clone()).unwrap_or_default()
    }

    /// The font the registry names for a character's plane, if it has the character. (Read once
    /// per plane.)
    fn plane(&mut self, letter: char) -> Option<usize> {
        let plane = letter as u32 >> 16;
        if !self.planes.iter().any(|(of, _)| *of == plane) {
            self.planes.push((plane, plane_font(letter)));
        }
        let name = self.planes.iter().find(|(of, _)| *of == plane).and_then(|(_, name)| name.clone())?;
        self.font(&name).filter(|at| self.has(*at, letter))
    }

    /// The first of all system fonts, in name order, that has a character: previously found
    /// ones first. (`None` if none.) Every font's coverage is learned at the first such
    /// character, once: a text of characters no font has costs no more than one of them.
    fn any(&mut self, letter: char) -> Option<usize> {
        let known = self.found.clone().iter().find_map(|name| self.font(name).filter(|at| self.has(*at, letter)));
        if known.is_some() {
            return known;
        }
        if self.first.is_none() {
            self.first = Some(self.all());
        }
        let first = self.first.as_deref().unwrap_or_default();
        let after = first.partition_point(|stretch| stretch & LETTER <= letter as u32);
        let family = self.owners.get((first.get(after.checked_sub(1)?)? >> 21) as usize)?.to_string();
        self.found.push(family.as_str().into());
        self.font(&family)
    }

    /// Which system font first has each character, in name order (see `first`). Of a font not
    /// known before, only what no earlier font has is kept.
    fn all(&mut self) -> Vec<u32> {
        let (families, mut first) = (families(), Vec::new());
        for (number, family) in families.iter().enumerate() {
            let known = self.fonts.iter().find(|font| font.name.eq_ignore_ascii_case(family));
            let learned = if known.is_none() { Some(self.learn(family)) } else { None };
            if let Some(font) = known.or(learned.as_ref()).filter(|font| font.here) {
                first = adding(&first, &font.letters, number as u16);
            }
        }
        // Each range at its start, and after it, where a range of none starts.
        let (mut kept, mut past) = (Vec::with_capacity(first.len() + first.len() / 8), 0);
        for ([from, to], family) in first {
            if from > past {
                kept.push(past | NONE);
            }
            let owner = self.owners.iter().position(|name| **name == *families[usize::from(family)]).unwrap_or(self.owners.len());
            if owner == self.owners.len() && owner < (NONE >> 21) as usize {
                self.owners.push(families[usize::from(family)].as_str().into());
            }
            kept.push(from | (owner as u32).min(NONE >> 21) << 21);
            past = to + 1;
        }
        kept.push(past | NONE);
        kept
    }

    /// The index of the known font of this name, examining it if new.
    fn font(&mut self, name: &str) -> Option<usize> {
        if let Some(at) = self.fonts.iter().position(|font| font.name.eq_ignore_ascii_case(name)) {
            return Some(at);
        }
        let known = self.learn(name);
        self.fonts.push(known);
        Some(self.fonts.len() - 1)
    }

    /// What Windows says of the font of this name: installed, its characters, and its line
    /// height.
    fn learn(&self, name: &str) -> Known {
        let (dc, mut got) = (self.dc, [0u16; 32]);
        // SAFETY: a font is created, selected into the DC, queried, and released once the stock
        // font has replaced it there. Windows writes the chosen font's name into the buffer it
        // is told of.
        let font = unsafe {
            let font = CreateFontW(-MEASURED, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, wide(name).as_ptr());
            SelectObject(dc, font);
            GetTextFaceW(dc, got.len() as i32, got.as_mut_ptr());
            font
        };
        let here = narrow(&got).eq_ignore_ascii_case(name);
        let [cmap, hhea, head] = [b"cmap", b"hhea", b"head"].map(|table| if here { self.table(table) } else { Vec::new() });
        // SAFETY: as above.
        unsafe {
            SelectObject(dc, GetStockObject(13));
            DeleteObject(font);
        }
        let number = |table: &[u8], at: usize| table.get(at..at + 2).map_or(0, |two| i32::from(i16::from_be_bytes([two[0], two[1]])));
        let units = head.get(18..20).map_or(1, |two| u16::from_be_bytes([two[0], two[1]]).max(1));
        Known { name: name.into(), here, letters: letters_of(&cmap), line: (number(&hhea, 4) - number(&hhea, 6)) as f32 / f32::from(units) }
    }

    /// A table of the DC's font, by tag: empty if it has none.
    fn table(&self, name: &[u8; 4]) -> Vec<u8> {
        let name = u32::from_le_bytes(*name);
        // SAFETY: Windows is first asked the table's size, then writes it into a buffer that
        // size.
        unsafe {
            let long = GetFontData(self.dc, name, 0, std::ptr::null_mut(), 0);
            let mut table = vec![0u8; if long == u32::MAX { 0 } else { long as usize }];
            GetFontData(self.dc, name, 0, table.as_mut_ptr(), table.len() as u32);
            table
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system font's glyphs, read from GDI one at a time as drawn, are the font's own: each
    /// outline as from the whole file Windows has, with only the glyphs drawn read.
    #[cfg(windows)]
    #[test]
    fn a_system_fonts_glyphs_are_read_as_drawn() {
        let letters = ['a', 'Q', '\u{e9}', '\u{431}', '\u{65e5}', '\u{672c}', '\u{d55c}'];
        let windows = std::path::PathBuf::from(std::env::var_os("WINDIR").expect("Windows' folder")).join("Fonts");
        let mut held = 0;
        for (face, file) in [(Face::Regular, "segoeui.ttf"), (Face::named("Yu Gothic"), "YuGothR.ttc"), (Face::named("Malgun Gothic"), "malgun.ttf")] {
            let (Some(fetched), Some(whole)) = (file_for(face, 'a'), crate::font_file::FontFile::open(windows.join(file))) else { continue };
            for letter in letters.iter().copied().filter(|letter| whole.glyph(*letter) != 0) {
                assert_eq!(fetched.glyph(letter), whole.glyph(letter), "{file}: {letter}");
                assert!(!fetched.outline(letter, 32.0).steps().is_empty(), "{file}: {letter}");
                assert_eq!(fetched.outline(letter, 32.0), whole.outline(letter, 32.0), "{file}: {letter}");
            }
            assert!(fetched.bytes().0.len() * 2 < whole.bytes().0.len(), "{file}: {} of {} bytes held", fetched.bytes().0.len(), whole.bytes().0.len());
            held += 1;
        }
        assert!(held > 0, "no font of these here");
    }

    #[test]
    fn a_style_is_read_from_its_name() {
        let read = |name: &str| (style_named(name).weight, style_named(name).italic);
        assert_eq!(read("Regular"), (400, false));
        assert_eq!(read("Bold Italic"), (700, true));
        assert_eq!(read("SemiBold"), (600, false));
        assert_eq!(read("Extra Bold"), (800, false));
        assert_eq!(read("Light Oblique"), (300, true));
        assert_eq!(read("Semilight"), (350, false));
        assert_eq!(read("Heavy"), (900, false));
        assert_eq!(read("Thin"), (100, false));
    }

    #[test]
    fn a_face_by_name_is_one_small_number() {
        let georgia = Face::named("Georgia");
        assert_eq!(georgia, Face::named("Georgia"), "the same name is the same face");
        assert_ne!(georgia, Face::named("Georgia").italic());
        assert_eq!(georgia.italic(), Face::named("Georgia").italic());
        assert_eq!((georgia.style(), georgia.italic().weight(700).style()), (Style::REGULAR, Style { weight: 700, italic: true }));
        // The system faces have their own styles, which a named face starts from.
        assert_eq!((Face::Regular.style(), Face::Mono.italic().style()), (Style::REGULAR, Style { weight: 400, italic: true }));
        assert_eq!(Face::Bold.italic().style(), Style { weight: BOLD, italic: true });
        // A face is three bytes, and a font (face, size, underline or strikethrough) still
        // eight: as before named faces existed.
        assert_eq!((size_of::<Face>(), size_of::<crate::text::Font>()), (3, 8));
    }

    /// A `cmap` table with one subtable.
    fn cmap(platform: u16, encoding: u16, list: Vec<u8>) -> Vec<u8> {
        [&[0, 0, 0, 1][..], &platform.to_be_bytes(), &encoding.to_be_bytes(), &12u32.to_be_bytes(), &list].concat()
    }

    /// A format 4 subtable of these segments: first, last, delta, and glyph array offset (with
    /// the glyphs).
    fn format_4(segments: &[(u16, u16, u16, u16)], glyphs: &[u16]) -> Vec<u8> {
        let n = segments.len() as u16;
        let mut list = [4u16, 0, 0, n * 2, 0, 0, 0].iter().flat_map(|word| word.to_be_bytes()).collect::<Vec<u8>>();
        list.extend(segments.iter().flat_map(|(_, last, ..)| last.to_be_bytes()));
        list.extend([0, 0]);
        list.extend(segments.iter().flat_map(|(first, ..)| first.to_be_bytes()));
        list.extend(segments.iter().flat_map(|(_, _, delta, _)| delta.to_be_bytes()));
        list.extend(segments.iter().flat_map(|(.., offset)| offset.to_be_bytes()));
        list.extend(glyphs.iter().flat_map(|glyph| glyph.to_be_bytes()));
        list
    }

    #[test]
    fn which_characters_a_font_has_is_read_from_its_table() {
        // A to Z by delta (glyph of "A" 1, "Z" 26); a, b, c from a glyph array where b is
        // missing; and the terminating segment.
        let list = format_4(&[(0x41, 0x5a, 0xffc0, 0), (0x61, 0x63, 0, 4), (0xffff, 0xffff, 1, 0)], &[5, 0, 7]);
        let letters = letters_of(&cmap(3, 1, list));
        assert_eq!(letters, [[0x41, 0x5a], [0x61, 0x61], [0x63, 0x63]]);
        assert!(has(&letters, 'A') && has(&letters, 'Z') && !has(&letters, 'b') && has(&letters, 'c') && !has(&letters, '@'));
        // Full Unicode, in groups: one starting at glyph 0 lacks its first character.
        let groups = [[0x20u32, 0x7e, 1], [0x1d400, 0x1d433, 0], [0x1d434, 0x1d44f, 200]];
        let list =
            [&[0u8, 12, 0, 0][..], &(16 + 12 * 3u32).to_be_bytes(), &0u32.to_be_bytes(), &3u32.to_be_bytes(), &groups.iter().flatten().flat_map(|n| n.to_be_bytes()).collect::<Vec<u8>>()].concat();
        assert_eq!(letters_of(&cmap(3, 10, list)), [[0x20, 0x7e], [0x1d401, 0x1d44f]]);
        // A symbol font has the ASCII characters too, which Windows maps to its own.
        let list = format_4(&[(0xf020, 0xf07e, 0x0fe3, 0), (0xffff, 0xffff, 1, 0)], &[]);
        assert_eq!(letters_of(&cmap(3, 0, list)), [[0x20, 0x7e], [0xf020, 0xf07e]]);
        // Nothing from a malformed table.
        assert!(letters_of(&[]).is_empty() && letters_of(&[0, 0, 0, 9]).is_empty());
    }

    /// A group at the largest code point, at glyph 0, holds no character: its first character,
    /// one past that, isn't one. (Computing it by adding one overflowed: a debug build
    /// panicked, and a release build gave the font every character.)
    #[test]
    fn a_group_that_starts_at_the_largest_number_holds_no_character() {
        let groups = [[0x41u32, 0x5a, 1], [u32::MAX, u32::MAX, 0], [u32::MAX - 1, u32::MAX, 0]];
        let list =
            [&[0u8, 12, 0, 0][..], &(16 + 12 * 3u32).to_be_bytes(), &0u32.to_be_bytes(), &3u32.to_be_bytes(), &groups.iter().flatten().flat_map(|n| n.to_be_bytes()).collect::<Vec<u8>>()].concat();
        assert_eq!(letters_of(&cmap(3, 10, list)), [[0x41, 0x5a]]);
    }

    #[test]
    fn the_scripts_windows_writes_in_their_own_fonts_are_in_order() {
        assert!(SCRIPTS.windows(2).all(|two| two[0].1 < two[1].0) && SCRIPTS.iter().all(|(first, last, font)| first <= last && usize::from(*font) < SCRIPT_FONTS.len()));
        assert_eq!([script_font('\u{915}'), script_font('\u{e01}'), script_font('A'), script_font('\u{4e2d}')], [Some("Nirmala UI"), Some("Leelawadee UI"), None, None]);
    }

    #[test]
    fn a_font_whose_line_is_taller_is_written_smaller_until_it_fits() {
        assert_eq!([fit(1.33, 1.33), fit(1.33, 1.0), fit(1.33, 2.66), fit(1.33, 0.0)], [1.0, 1.0, 0.5, 1.0]);
    }

    #[test]
    fn a_family_is_kept_by_its_name_once_and_only_where_it_can_be_a_familys() {
        // However many faces of a family are named, its name is kept once.
        let faces = [Face::named("Kept Once"), Face::named("Kept Once").italic(), Face::named("Kept Once").weight(700).italic()];
        assert!(faces[0] != faces[1] && faces[1] != faces[2]);
        assert_eq!(held(&FAMILIES).iter().filter(|known| ***known == *"Kept Once").count(), 1);
        assert!(faces.iter().all(|face| kept(*face).family.map(name).as_deref() == Some("Kept Once")));
        // A name longer than any family's gets the system font, in the requested style, and
        // isn't kept: two such names are the same face. One exactly `LONGEST` is kept.
        let (long, longer) = (Face::named(&"a".repeat(LONGEST + 1)), Face::named(&"b".repeat(4 * LONGEST)));
        assert_eq!((long, kept(long).family, kept(long.italic()).style), (longer, None, Style { italic: true, ..Style::REGULAR }));
        assert!(!held(&FAMILIES).iter().any(|known| known.len() > LONGEST));
        assert_eq!(kept(Face::named(&"c".repeat(LONGEST))).family.map(name), Some("c".repeat(LONGEST)));
    }

    #[test]
    fn of_the_fonts_that_have_a_character_the_first_is_kept() {
        // Ranges of fonts 0 and 1, then what a third adds: characters neither has, cut around
        // theirs, none twice.
        let first = adding(&adding(&[], &[[10, 20], [40, 50]], 0), &[[15, 30]], 1);
        assert_eq!(first, [([10, 20], 0), ([21, 30], 1), ([40, 50], 0)]);
        let all = adding(&first, &[[0, 12], [18, 45], [49, 60], [100, 100]], 2);
        assert_eq!(all, [([0, 9], 2), ([10, 20], 0), ([21, 30], 1), ([31, 39], 2), ([40, 50], 0), ([51, 60], 2), ([100, 100], 2)]);
        // A font with nothing new adds nothing; a first font is taken as is.
        assert_eq!(adding(&all, &[[10, 12], [25, 26], [55, 55]], 3), all);
        assert_eq!(adding(&[], &[[5, 5], [7, 9]], 4), [([5, 5], 4), ([7, 9], 4)]);
    }

    #[test]
    fn fontconfig_is_asked_for_a_family_and_its_style() {
        assert_eq!(pattern(Face::Regular), "sans:weight=regular");
        assert_eq!(pattern(Face::Mono.italic()), "monospace:weight=regular:slant=italic");
        assert_eq!(pattern(Face::named("Noto Serif").weight(600)), "Noto Serif:weight=demibold");
        assert_eq!(pattern(Face::named("Odd-Name: 1,2").weight(320)), "Odd\\-Name\\: 1\\,2:weight=light");
        assert_eq!([weight_of(80), weight_of(200), weight_of(181), weight_of(0)], [400, 700, 600, 100]);
    }

    #[test]
    #[cfg(any(windows, all(unix, not(target_os = "android"))))]
    fn a_fallback_glyph_keeps_the_rows_within_the_line() {
        // Twenty rows from 10 above the baseline, in a line 8 above it and 4 below: rows 2 to 13.
        let cover: Vec<u8> = (0..40).collect();
        assert_eq!(within_line(10, 2, &cover, [8, 4]), (8, &cover[4..28]));
        // Wholly inside, wholly outside.
        assert_eq!(within_line(5, 2, &cover[..4], [8, 4]), (5, &cover[..4]));
        assert_eq!(within_line(30, 2, &cover[..4], [8, 4]).1, &[][..]);
    }

    #[test]
    #[cfg(all(feature = "fallback-fonts", any(windows, all(unix, not(any(target_vendor = "apple", target_os = "android"))))))]
    fn letters_of_fallback_fonts_stay_within_the_line() {
        use crate::text::{Font, TextSystem};
        // Tibetan stacks, Burmese, Javanese and Balinese stacks: letters of fonts with taller
        // lines than the face's, where installed.
        let Some(mut text) = crate::platform::SystemText::new() else { return };
        let font = Font::new(24.0);
        let metrics = text.metrics(font, 1.0);
        let line = 40..40 + (metrics.ascent + metrics.descent).round() as usize;
        for word in ["ཀྵྰཱཱུ", "ဿ္လျှ", "ꦏ꧀ꦏ꧀ꦏ", "ᬓ᭄ᬓ᭄ᬓ"] {
            let mut pixels = vec![0u32; 300 * 120];
            text.draw(&mut crate::canvas::Canvas::new(&mut pixels, 300, 120, (0, 0), 1.0), word, 4.0, 40.0, font, crate::color::Color::WHITE);
            let rows: Vec<usize> = (0..120).filter(|row| pixels[row * 300..(row + 1) * 300].iter().any(|pixel| *pixel != 0)).collect();
            assert!(rows.iter().all(|row| line.contains(row)), "{word}: rows {rows:?} outside {line:?}");
        }
    }
}

#[cfg(all(test, windows))]
mod on_windows {
    use super::*;
    use crate::TextSystem;
    use crate::text::Font;

    /// The family a face draws in, as requested from Windows.
    #[cfg(feature = "fallback-fonts")]
    fn family(face: Face) -> String {
        asked(face).names[0].clone()
    }

    #[test]
    fn the_families_are_those_windows_lists_with_their_styles() {
        let families = families();
        assert!(families.iter().any(|family| family == "Segoe UI") && families.iter().any(|family| family == "Arial"), "{families:?}");
        assert!(!families.iter().any(|family| family.starts_with('@')), "no forms for writing downwards");
        let arial = styles("Arial");
        for style in [Style::REGULAR, Style { weight: 400, italic: true }, Style { weight: 700, italic: false }, Style { weight: 700, italic: true }] {
            assert!(arial.contains(&style), "{style:?} of Arial in {arial:?}");
        }
        assert!(styles("No Such Family Here").is_empty());
    }

    #[test]
    #[cfg(feature = "fallback-fonts")]
    fn a_letter_the_face_lacks_is_written_in_the_font_windows_would_choose() {
        // As DirectWrite chooses them on Windows 11 (checked character by character against it
        // and against GDI's rendering): each script's font; uninstalled fonts aren't checked.
        let here = families();
        for (letter, font) in
            [('\u{915}', "Nirmala UI"), ('\u{e01}', "Leelawadee UI"), ('\u{1200}', "Ebrima"), ('\u{13a0}', "Gadugi"), ('\u{ac00}', "Malgun Gothic"), ('\u{13000}', "Segoe UI Historic")]
        {
            if here.iter().any(|family| family == font) {
                assert_eq!(family(face_for(Face::Regular, letter)), font, "U+{:04X}", letter as u32);
            }
        }
        // A character of the face's own, and one no font has, stay in the face.
        assert_eq!([face_for(Face::Regular, 'A'), face_for(Face::Regular, '\u{e9}'), face_for(Face::Regular, '\u{10fffd}')], [Face::Regular; 3]);
        // The fallback draws in the face's style.
        assert_eq!(face_for(Face::Bold, '\u{915}').style(), Face::Bold.style());
    }

    #[test]
    #[cfg(feature = "fallback-fonts")]
    fn a_line_of_several_scripts_is_cut_where_the_font_changes() {
        let (text, mut pieces) = ("Hi \u{915}\u{93f} \u{e01}\u{e35}!", Vec::new());
        assert!(!super::pieces("Plain text", Face::Regular, &mut pieces) && pieces.is_empty());
        assert!(!super::pieces("caf\u{e9} \u{2014}", Face::Regular, &mut pieces), "letters Segoe UI has are not cut out");
        assert!(super::pieces(text, Face::Regular, &mut pieces));
        let cut: Vec<&str> = pieces.iter().map(|(bytes, _)| &text[bytes.clone()]).collect();
        assert_eq!(cut, ["Hi ", "\u{915}\u{93f}", " ", "\u{e01}\u{e35}", "!"]);
        assert_eq!(pieces.iter().map(|(_, face)| *face == Face::Regular).collect::<Vec<_>>(), [true, false, true, false, true]);
    }

    #[test]
    fn a_named_font_is_written_in_that_font() {
        let mut text = crate::platform::SystemText::new().unwrap();
        let plain = text.measure("Handgloves", Font::new(20.0), 1.0, None);
        let georgia = text.measure("Handgloves", Font { face: Face::named("Georgia"), ..Font::new(20.0) }, 1.0, None);
        let italic = text.measure("Handgloves", Font { face: Face::named("Georgia").italic(), ..Font::new(20.0) }, 1.0, None);
        assert!(plain != georgia && georgia != italic, "{plain} {georgia} {italic}");
        // A missing family gets the regular face's font.
        assert_eq!(text.measure("Handgloves", Font { face: Face::named("No Such Family Here"), ..Font::new(20.0) }, 1.0, None), plain);
    }

    #[test]
    #[cfg(not(feature = "fallback-fonts"))]
    fn without_the_feature_a_line_stays_in_its_face_and_windows_measures_what_it_lacks() {
        // Nothing is split and no font chosen here: Windows' linked fonts measure (and in a
        // window, draw) the characters Segoe UI lacks, each as wide as there.
        let (text, mut pieces) = ("Hi कि 中文!", Vec::new());
        assert!(!super::pieces(text, Face::Regular, &mut pieces) && pieces.is_empty());
        assert_eq!([face_for(Face::Regular, 'क'), face_for(Face::Bold, '가')], [Face::Regular, Face::Bold]);
        let mut system = crate::platform::SystemText::new().unwrap();
        let (font, mut stops) = (Font::new(30.0), Vec::new());
        system.measure("中文中文", font, 1.0, Some(&mut stops));
        let widths: Vec<f32> = stops.iter().scan(0.0, |end, stop| Some(stop - std::mem::replace(end, *stop))).collect();
        assert!(widths.iter().all(|wide| (26.0..=34.0).contains(wide)), "{widths:?}: a letter of Chinese is about as wide as it is high");
    }

    /// 300 private-use characters, which no font has. Each used to be searched in all system
    /// fonts (about 40 ms), and only the last 256 misses were remembered: measuring a line of
    /// them took 5 seconds every time. All fonts' coverage is now learned once.
    #[test]
    #[cfg(feature = "fallback-fonts")]
    fn a_line_of_letters_no_font_has_looks_through_the_fonts_once() {
        let (text, mut pieces) = ((0..300).filter_map(|n| char::from_u32(0xf0000 + n)).collect::<String>(), Vec::new());
        let start = std::time::Instant::now();
        assert!(!super::pieces(&text, Face::Regular, &mut pieces), "no font has them");
        let once = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            super::pieces(&text, Face::Regular, &mut pieces);
        }
        let again = start.elapsed();
        // (Ten more took 50 s before. Idle they take some 20 ms; a busy computer, several times
        // that: the bound is far from both.)
        assert!(once.as_secs_f32() < 2.0 && again.as_secs_f32() < 1.0, "the first time {once:?}, ten times more {again:?}");
    }

    /// The single full scan finds, for any character, the font a search for that character
    /// alone finds: the first, in name order, that has it (checked by asking each font).
    #[test]
    #[cfg(feature = "fallback-fonts")]
    fn the_first_font_that_has_a_letter_is_found_as_asking_each_finds_it() {
        let mut learned = Learned::new();
        let first = learned.all();
        let fonts: Vec<Known> = families().iter().map(|family| learned.learn(family)).collect();
        assert!(first.windows(2).all(|two| two[0] & LETTER < two[1] & LETTER) && first.len() < 1 << 16, "{} stretches", first.len());
        let found = |letter: char| {
            let at = first.partition_point(|stretch| stretch & LETTER <= letter as u32);
            first.get(at.wrapping_sub(1)).and_then(|stretch| learned.owners.get((stretch >> 21) as usize)).map(|name| name.to_string())
        };
        // Every 37th character of the first three planes, and the first of each private-use
        // area.
        for letter in (0x80..0x3_0000).step_by(37).chain([0xe000, 0xf0000, 0x10_fffd]).filter_map(char::from_u32) {
            let asked = fonts.iter().find(|font| font.here && has(&font.letters, letter)).map(|font| font.name.to_string());
            assert_eq!(found(letter), asked, "U+{:04X}", letter as u32);
        }
    }

    /// MS Gothic's file holds three fonts sharing outlines, plus embedded bitmaps (not read): a
    /// third of the file. It used to be passed whole (9 MB, read into memory per lookup).
    #[test]
    fn a_font_of_a_collection_is_handed_over_as_itself_with_the_tables_that_are_read() {
        let Some(path) = ["C:/Windows/Fonts/msgothic.ttc"].into_iter().find(|path| std::path::Path::new(path).exists()) else { return eprintln!("no MS Gothic: not tested") };
        let whole = std::fs::metadata(path).unwrap().len() as usize;
        let font = file_for(Face::named("MS UI Gothic"), 'a').expect("the font is handed over");
        let own = crate::font_file::FontFile::open(path).and_then(|file| file.named("MS UI Gothic")).expect("the file is read");
        assert!(font.bytes().0.len() < whole * 2 / 3, "{} bytes of {whole}", font.bytes().0.len());
        // It is the font the file holds: its glyphs draw as they do there.
        for letter in "Ag\u{3042}\u{4e2d}\u{ff21}".chars() {
            assert_eq!((font.outline(letter, 32.0), font.advance(letter, 32.0)), (own.outline(letter, 32.0), own.advance(letter, 32.0)), "{letter:?}");
        }
        // A font not in a collection is itself, minus unread tables.
        let segoe = file_for(Face::Regular, 'a').expect("the font is handed over");
        let file = crate::font_file::FontFile::open("C:/Windows/Fonts/segoeui.ttf").expect("the file is read");
        assert!(segoe.bytes().0.len() < file.bytes().0.len());
        assert_eq!((segoe.outline('g', 32.0), segoe.metrics(32.0)), (file.outline('g', 32.0), file.metrics(32.0)));
    }

    #[test]
    #[cfg(feature = "fallback-fonts")]
    fn mixed_scripts_are_drawn_with_no_box_on_one_line() {
        // A line of Latin, Devanagari, Thai, Chinese, Korean and a Segoe UI Symbol sign, drawn
        // with system text into a pixel buffer: each character draws as its font draws it alone
        // (so no box), all on one baseline.
        let mut text = crate::platform::SystemText::new().unwrap();
        let font = Font::new(24.0);
        let ascent = text.metrics(font, 1.0).ascent;
        let line = "Ag \u{915}\u{e01}\u{4e2d}\u{ac00}\u{2200}";
        let (width, height) = (240usize, 40usize);
        let mut pixels = vec![0u32; width * height];
        let mut canvas = crate::Canvas::new(&mut pixels, width, height, (0, 0), 1.0);
        text.draw(&mut canvas, line, 2.0, 4.0, font, crate::Color::WHITE);
        drop(canvas);
        let ink = |pixels: &[u32], from: usize, to: usize| (from..to).filter(|x| (0..height).any(|y| pixels[y * width + x] != 0)).count();
        let stops = {
            let mut stops = Vec::new();
            text.measure(line, font, 1.0, Some(&mut stops));
            stops
        };
        // Each non-ASCII character has ink of its own in its own advance.
        for n in 3..line.chars().count() {
            let (from, to) = ((2.0 + stops[n - 1]) as usize, (2.0 + stops[n]) as usize);
            assert!(ink(&pixels, from, to) > 2, "letter {n} drew nothing");
        }
        // Fallback glyphs sit on the face's line: none reaches above its top.
        assert!((0..4 * width).all(|at| pixels[at] == 0), "ink above the line");
        let _ = ascent;
        let _ = std::fs::write(std::env::temp_dir().join("taut-mixed-scripts.png"), crate::png::encode(width, &pixels).unwrap());
    }
}

#[cfg(all(test, unix, not(any(target_vendor = "apple", target_os = "android"))))]
mod on_linux {
    use super::*;
    use crate::TextSystem;
    use crate::text::Font;

    #[test]
    fn the_families_are_those_fontconfig_lists() {
        // Fontconfig's own `fc-list` is the reference: the first name of every scalable family.
        let Ok(said) = std::process::Command::new("fc-list").args([":scalable=true", "family"]).output() else { return eprintln!("no fc-list: not tested") };
        let mut listed: Vec<String> =
            String::from_utf8_lossy(&said.stdout).lines().filter_map(|line| line.split(',').next()).map(|family| family.replace('\\', "")).filter(|family| !family.is_empty()).collect();
        listed.sort_by_key(|family| family.to_lowercase());
        listed.dedup();
        assert_eq!(families(), listed);
    }

    #[test]
    fn a_named_font_is_written_in_that_font() {
        let Some(mut text) = crate::platform::SystemText::new() else { return eprintln!("no fonts: not tested") };
        let families = families();
        let Some(serif) = ["DejaVu Serif", "Noto Serif"].into_iter().find(|family| families.iter().any(|known| known == family)) else { return eprintln!("no serif font: not tested") };
        let wide = |text: &mut crate::platform::SystemText, face: Face| text.measure("Handgloves", Font { face, ..Font::new(20.0) }, 1.0, None);
        let (plain, named) = (wide(&mut text, Face::Regular), wide(&mut text, Face::named(serif)));
        assert!(plain != named, "{plain} {named}");
        assert!(styles(serif).contains(&Style::REGULAR), "{:?}", styles(serif));
    }
}
