//! Text on macOS: drawn by CoreText in the system's fonts, glyph by glyph into the program's own
//! pixels, as FreeType draws under Linux. The system font (San Francisco) for text, its fixed
//! pitch one for code; a character a font lacks comes from the font CoreText picks for it
//! (`CTFontCreateForString`). Named fonts are found by family name, then made bold or italic
//! where the family has such a face.
//!
//! Each glyph is drawn once per size into a grey bitmap (its coverage) and kept; drawing is
//! then laying coverage in a colour. Advances are whole pixels, so every glyph starts on one.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::{FontFile, Set};
use crate::macos_sys::{CFRange, CFRelease, CFStringCreateWithBytes, NSPoint, NSRect, NSSize, Obj, UTF8, array_items, rust_string, send};
use crate::macos_sys::{CGBitmapContextCreate, CGColorSpaceCreateDeviceGray, CGColorSpaceRelease, CGContextRelease, CGContextSetGrayFillColor, CGContextSetShouldAntialias};
use crate::system_fonts::Style;
use crate::text::{Face, Font, Metrics, TextSystem};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateUIFontForLanguage(kind: u32, size: f64, language: *const c_void) -> *const c_void;
    fn CTFontCreateWithName(name: *const c_void, size: f64, matrix: *const c_void) -> *const c_void;
    fn CTFontCreateCopyWithSymbolicTraits(font: *const c_void, size: f64, matrix: *const c_void, value: u32, mask: u32) -> *const c_void;
    fn CTFontCreateForString(font: *const c_void, text: *const c_void, range: CFRange) -> *const c_void;
    fn CTFontGetGlyphsForCharacters(font: *const c_void, characters: *const u16, glyphs: *mut u16, count: isize) -> bool;
    fn CTFontGetAdvancesForGlyphs(font: *const c_void, orientation: u32, glyphs: *const u16, advances: *mut NSSize, count: isize) -> f64;
    fn CTFontGetBoundingRectsForGlyphs(font: *const c_void, orientation: u32, glyphs: *const u16, rects: *mut NSRect, count: isize) -> NSRect;
    fn CTFontDrawGlyphs(font: *const c_void, glyphs: *const u16, positions: *const NSPoint, count: usize, context: *mut c_void);
    fn CTFontGetAscent(font: *const c_void) -> f64;
    fn CTFontGetDescent(font: *const c_void) -> f64;
    fn CTFontGetCapHeight(font: *const c_void) -> f64;
    fn CTFontGetXHeight(font: *const c_void) -> f64;
    fn CTFontCopyAttribute(font: *const c_void, attribute: *const c_void) -> *const c_void;
    fn CTFontCollectionCreateFromAvailableFonts(options: *const c_void) -> *const c_void;
    fn CTFontCollectionCreateMatchingFontDescriptors(collection: *const c_void) -> *const c_void;
    fn CTFontDescriptorCopyAttribute(descriptor: *const c_void, attribute: *const c_void) -> *const c_void;
    static kCTFontURLAttribute: *const c_void;
    static kCTFontFamilyNameAttribute: *const c_void;
    static kCTFontTraitsAttribute: *const c_void;
    static kCTFontWeightTrait: *const c_void;
    static kCTFontSymbolicTrait: *const c_void;
    fn CTFontCreateCopyWithAttributes(font: *const c_void, size: f64, matrix: *const c_void, attributes: *const c_void) -> *const c_void;
    fn CTLineCreateWithAttributedString(text: *const c_void) -> *const c_void;
    fn CTLineGetGlyphRuns(line: *const c_void) -> *const c_void;
    fn CTLineGetTypographicBounds(line: *const c_void, ascent: *mut f64, descent: *mut f64, leading: *mut f64) -> f64;
    fn CTRunGetGlyphCount(run: *const c_void) -> isize;
    fn CTRunGetGlyphs(run: *const c_void, range: CFRange, glyphs: *mut u16);
    fn CTRunGetPositions(run: *const c_void, range: CFRange, positions: *mut NSPoint);
    fn CTRunGetAttributes(run: *const c_void) -> *const c_void;
    static kCTFontAttributeName: *const c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFEqual(one: *const c_void, other: *const c_void) -> bool;
    fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
}

/// CoreText's own fonts for the interface: the system font, and its fixed-pitch one
/// (`kCTFontUIFontSystem`, `kCTFontUIFontUserFixedPitch`).
const SYSTEM: u32 = 2;
const FIXED: u32 = 1;
/// Symbolic traits: italic, bold.
const ITALIC: u32 = 1;
const BOLD: u32 = 2;

/// A CoreFoundation object this owns, released where it ends.
struct Owned(*const c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an object this made (or was handed with a +1 count).
            unsafe { CFRelease(self.0) }
        }
    }
}

/// Text as a `CFString` (owned).
fn cf_string(text: &str) -> Owned {
    // SAFETY: the bytes are copied, as UTF-8.
    Owned(unsafe { CFStringCreateWithBytes(std::ptr::null(), text.as_ptr(), text.len() as isize, UTF8, 0) })
}

/// A face's font at a size in pixels: the system's for the four faces, a family by name for a
/// named one, made bold or italic as its style says where the family has such a face.
fn font_of(face: Face, pixels: f64) -> Owned {
    let (family, of, style) = crate::system_fonts::asked(face);
    let kind = if of == Face::Mono { FIXED } else { SYSTEM };
    // SAFETY: CoreText makes fonts (+1); a family it lacks gives a fallback font, never null.
    let base = match family {
        Some(family) => Owned(unsafe { CTFontCreateWithName(cf_string(&family).0, pixels, std::ptr::null()) }),
        None => Owned(unsafe { CTFontCreateUIFontForLanguage(kind, pixels, std::ptr::null()) }),
    };
    let wanted = if style.weight >= 600 { BOLD } else { 0 } | if style.italic { ITALIC } else { 0 };
    if wanted == 0 || base.0.is_null() {
        return base;
    }
    // SAFETY: as above; null where the family has no such face.
    let styled = Owned(unsafe { CTFontCreateCopyWithSymbolicTraits(base.0, 0.0, std::ptr::null(), wanted, BOLD | ITALIC) });
    if styled.0.is_null() { base } else { styled }
}

/// A character's glyph in a font, if the font has it.
fn glyph_in(font: *const c_void, letter: char) -> Option<u16> {
    let mut units = [0u16; 2];
    let units = letter.encode_utf16(&mut units);
    let mut glyphs = [0u16; 2];
    // SAFETY: as many glyphs as characters are written.
    let found = unsafe { CTFontGetGlyphsForCharacters(font, units.as_ptr(), glyphs.as_mut_ptr(), units.len() as isize) };
    (found && glyphs[0] != 0).then_some(glyphs[0])
}

/// The font CoreText draws a character in that `font` lacks (owned), and its glyph there.
fn fallback(font: *const c_void, letter: char) -> Option<(Owned, u16)> {
    let text = cf_string(letter.encode_utf8(&mut [0; 4]));
    // SAFETY: the range covers the string's UTF-16 units; the font comes back +1.
    let other = Owned(unsafe { CTFontCreateForString(font, text.0, CFRange { location: 0, length: letter.len_utf16() as isize }) });
    let glyph = (!other.0.is_null()).then(|| glyph_in(other.0, letter)).flatten()?;
    Some((other, glyph))
}

/// One glyph as drawn: its coverage (`width` bytes a row, top row first), where that lies from
/// the pen on the baseline, and the pen's advance, in whole pixels.
struct Glyph {
    width: usize,
    left: i32,
    top: i32,
    advance: f32,
    cover: Vec<u8>,
}

/// Draws a glyph of a font into a grey bitmap of its own.
fn drawn(font: *const c_void, glyph: u16) -> Glyph {
    let mut advance = NSSize::default();
    let mut bounds = NSRect::default();
    // SAFETY: one glyph, one advance and one rectangle.
    unsafe {
        CTFontGetAdvancesForGlyphs(font, 0, &glyph, &mut advance, 1);
        CTFontGetBoundingRectsForGlyphs(font, 0, &glyph, &mut bounds, 1);
    }
    let advance = advance.width.round() as f32;
    // A pixel of room on every side, for what antialiasing reaches.
    let (left, bottom) = (bounds.origin.x.floor() as i32 - 1, bounds.origin.y.floor() as i32 - 1);
    let (right, top) = ((bounds.origin.x + bounds.size.width).ceil() as i32 + 1, (bounds.origin.y + bounds.size.height).ceil() as i32 + 1);
    let (width, height) = ((right - left).max(0) as usize, (top - bottom).max(0) as usize);
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 || width * height > 1 << 22 {
        return Glyph { width: 0, left: 0, top: 0, advance, cover: Vec::new() };
    }
    let mut cover = vec![0u8; width * height];
    // SAFETY: the context draws into `cover`, `width` bytes a row, grey without alpha (0), and
    // is released before `cover` is read; its first row in memory is its top.
    unsafe {
        let grey = CGColorSpaceCreateDeviceGray();
        let context = CGBitmapContextCreate(cover.as_mut_ptr().cast(), width, height, 8, width, grey, 0);
        CGColorSpaceRelease(grey);
        if !context.is_null() {
            CGContextSetShouldAntialias(context, true);
            CGContextSetGrayFillColor(context, 1.0, 1.0);
            CTFontDrawGlyphs(font, &glyph, &NSPoint { x: f64::from(-left), y: f64::from(-bottom) }, 1, context);
            CGContextRelease(context);
        }
    }
    Glyph { width, left, top, advance, cover }
}

/// Text drawn by CoreText in the system's fonts, and for characters they lack, the font
/// CoreText picks.
pub struct SystemText {
    /// Each face's font, by face number and size in pixels.
    fonts: HashMap<(u32, u32), Owned>,
    glyphs: HashMap<(u32, u32, char), Glyph>,
}

impl std::fmt::Debug for SystemText {
    /// How many fonts are open and glyphs kept.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemText").field("fonts", &self.fonts.len()).field("letters", &self.glyphs.len()).finish()
    }
}

impl SystemText {
    /// The system's text, as CoreText draws it. (Always there on macOS.)
    pub fn new() -> Option<Self> {
        let text = SystemText { fonts: HashMap::new(), glyphs: HashMap::new() };
        // SAFETY: CoreText's system font, +1, released at once.
        let system = Owned(unsafe { CTFontCreateUIFontForLanguage(SYSTEM, 12.0, std::ptr::null()) });
        (!system.0.is_null()).then_some(text)
    }

    /// A font's key: its face's number and its size in whole pixels; the font is made when
    /// first asked for.
    fn face(&mut self, font: Font, scale: f32) -> (u32, u32) {
        let key = (font.face.number(), (font.size * scale).round().max(1.0) as u32);
        self.fonts.entry(key).or_insert_with(|| font_of(font.face, f64::from(key.1)));
        key
    }

    /// A glyph of a font: drawn when first asked for (from the fallback font where the font
    /// lacks the character), and kept.
    fn glyph(&mut self, (face, pixels): (u32, u32), letter: char) -> &Glyph {
        let font = self.fonts[&(face, pixels)].0;
        self.glyphs.entry((face, pixels, letter)).or_insert_with(|| match glyph_in(font, letter) {
            Some(glyph) => drawn(font, glyph),
            None => fallback(font, letter).map_or_else(|| drawn(font, 0), |(other, glyph)| fallen(drawn(other.0, glyph), font)),
        })
    }
}

/// A fallback glyph as drawn for a font it stands in for: with `fallback-fonts`, kept within
/// that font's line, so nothing it overhangs is left behind on redraw.
fn fallen(glyph: Glyph, font: *const c_void) -> Glyph {
    if !cfg!(feature = "fallback-fonts") {
        return glyph;
    }
    // SAFETY: plain questions of a font.
    let line = unsafe { [CTFontGetAscent(font), CTFontGetDescent(font)] }.map(|reach| reach.ceil() as i32);
    let (top, cover) = crate::system_fonts::within_line(glyph.top, glyph.width, &glyph.cover, line);
    Glyph { top, cover: cover.to_vec(), ..glyph }
}

impl TextSystem for SystemText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        let key = self.face(font, scale);
        let of = self.fonts[&key].0;
        // SAFETY: plain questions of a font.
        let [ascent, descent, cap, small] = unsafe { [CTFontGetAscent(of), CTFontGetDescent(of), CTFontGetCapHeight(of), CTFontGetXHeight(of)] };
        let [ascent, descent, cap, small] = [ascent.ceil(), descent.ceil(), cap.round(), small.round()].map(|pixels| pixels as f32 / scale);
        Metrics { ascent, descent, cap_height: cap, x_height: small }
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        let (key, mut x) = (self.face(font, scale), 0.0);
        for letter in text.chars() {
            x += self.glyph(key, letter).advance;
            if let Some(stops) = stops.as_deref_mut() {
                stops.push(x / scale);
            }
        }
        x / scale
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let ascent = (self.metrics(font, scale).ascent * scale).round() as i32;
        let (key, mut pen, line) = (self.face(font, scale), (x * scale).round(), (y * scale).round() as i32 + ascent);
        for letter in text.chars() {
            let glyph = self.glyph(key, letter);
            canvas.cover(pen as i32 + glyph.left, line - glyph.top, glyph.width, &glyph.cover, color);
            pen += glyph.advance;
        }
    }
}

/// The weight CoreText gives a font (-1 to 1) on the 100 to 900 scale: the nearest named one.
fn weight_of(trait_weight: f64) -> u16 {
    const WEIGHTS: [(f64, u16); 9] = [(-0.8, 100), (-0.6, 200), (-0.4, 300), (0.0, 400), (0.23, 500), (0.3, 600), (0.4, 700), (0.56, 800), (0.62, 900)];
    WEIGHTS.iter().min_by(|(a, _), (b, _)| (a - trait_weight).abs().total_cmp(&(b - trait_weight).abs())).map_or(400, |(_, weight)| *weight)
}

/// The fonts CoreText has, each family and style: all, or one family's.
pub(crate) fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    let _pool = crate::macos_sys::Pool::new();
    // SAFETY: CoreText's collection and its descriptors, +1 each and released; the rest are
    // read through the toll-free bridged Foundation classes.
    let all = Owned(unsafe { CTFontCollectionCreateFromAvailableFonts(std::ptr::null()) });
    let found = Owned(unsafe { CTFontCollectionCreateMatchingFontDescriptors(all.0) });
    let mut listed = Vec::new();
    for descriptor in array_items(found.0.cast_mut()) {
        let name = Owned(unsafe { CTFontDescriptorCopyAttribute(descriptor, kCTFontFamilyNameAttribute) });
        let Some(name) = rust_string(name.0.cast_mut()).filter(|name| family.is_none_or(|family| family.eq_ignore_ascii_case(name))) else { continue };
        let traits = Owned(unsafe { CTFontDescriptorCopyAttribute(descriptor, kCTFontTraitsAttribute) });
        listed.push((name, style_of(traits.0.cast_mut())));
    }
    listed
}

/// A font's style from its traits dictionary.
fn style_of(traits: Obj) -> Style {
    // SAFETY: a dictionary (or nil) of numbers (or nil).
    unsafe {
        let weight: Obj = send(traits, c"objectForKey:", (kCTFontWeightTrait,));
        let symbolic: Obj = send(traits, c"objectForKey:", (kCTFontSymbolicTrait,));
        let (weight, symbolic) = (send::<f64>(weight, c"doubleValue", ()), send::<u32>(symbolic, c"unsignedIntValue", ()));
        Style { weight: weight_of(weight), italic: symbolic & ITALIC != 0 }
    }
}

/// The file of the font a face draws a character in: its own, or the one CoreText picks where
/// it lacks the character.
pub(crate) fn file_for(face: Face, letter: char) -> Option<PathBuf> {
    let _pool = crate::macos_sys::Pool::new();
    let font = font_of(face, 12.0);
    let other = glyph_in(font.0, letter).is_none().then(|| fallback(font.0, letter)).flatten();
    let used = other.as_ref().map_or(font.0, |(other, _)| other.0);
    // SAFETY: the URL comes back +1 (or null); its path is read at once.
    let url = Owned(unsafe { CTFontCopyAttribute(used, kCTFontURLAttribute) });
    let path: Obj = unsafe { send(url.0.cast_mut(), c"path", ()) };
    rust_string(path).map(PathBuf::from)
}

// ---- emoji ----

/// The emoji font CoreText draws in, at one point a font unit (kept for the program's life;
/// none yet while 0), and the serials of the files read of it: CoreText shapes their sequences
/// (`sequence`). Apple Color Emoji joins families, flags and skin tones in `morx`, which
/// `shaping.rs` does not read.
static EMOJI: std::sync::Mutex<(usize, Vec<u32>)> = std::sync::Mutex::new((0, Vec::new()));
/// The most files of it kept track of: more are read only by tests.
const EMOJI_FILES: usize = 64;

/// The system's emoji font, as a file read here: the one CoreText draws a smiling face in (the
/// face of a collection whose glyph for it is CoreText's).
pub(crate) fn emoji_file() -> Option<FontFile> {
    let _pool = crate::macos_sys::Pool::new();
    let base = font_of(Face::Regular, 12.0);
    let (other, glyph) = fallback(base.0, '😀')?;
    // SAFETY: the URL comes back +1 (or null); its path is read at once.
    let url = Owned(unsafe { CTFontCopyAttribute(other.0, kCTFontURLAttribute) });
    let path = rust_string(unsafe { send(url.0.cast_mut(), c"path", ()) })?;
    let file = (0..4).filter_map(|nth| FontFile::open(&path)?.nth(nth)).find(|font| font.glyph('😀') == glyph)?;
    let mut kept = EMOJI.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if kept.0 == 0 {
        // SAFETY: a copy of the font at the file's units per em (+1), kept for good.
        kept.0 = unsafe { CTFontCreateCopyWithAttributes(other.0, f64::from(file.units()), std::ptr::null(), std::ptr::null()) } as usize;
    }
    if kept.1.len() >= EMOJI_FILES {
        kept.1.clear();
    }
    kept.1.push(file.serial());
    Some(file)
}

/// The glyphs CoreText makes of a sequence in the system's emoji font, if `font` is a file of
/// it (see `FontFile::set`): each placed right and up from the pen, in font units; and the
/// pen's advance. `None` for another font, or where CoreText drew some of it from another.
pub(crate) fn sequence(font: &FontFile, letters: &str, glyphs: &mut Vec<Set>) -> Option<f32> {
    let shaper = {
        let kept = EMOJI.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        kept.1.contains(&font.serial()).then_some(kept.0 as *const c_void)?
    };
    let _pool = crate::macos_sys::Pool::new();
    // SAFETY: an attributed string in the kept font (toll-free bridged), laid out as a line
    // (+1, released); its runs are read while it lives, each glyph and place into room for
    // as many as the run says.
    unsafe {
        let looks: Obj = send(crate::macos_sys::class(c"NSDictionary"), c"dictionaryWithObject:forKey:", (shaper, kCTFontAttributeName));
        let text: Obj = send(crate::macos_sys::class(c"NSAttributedString"), c"alloc", ());
        let text: Obj = send(send::<Obj>(text, c"initWithString:attributes:", (crate::macos_sys::ns_string(letters), looks)), c"autorelease", ());
        let line = Owned(CTLineCreateWithAttributedString(text.cast_const()));
        glyphs.clear();
        for run in array_items(CTLineGetGlyphRuns(line.0).cast_mut()) {
            let used = CFDictionaryGetValue(CTRunGetAttributes(run), kCTFontAttributeName);
            if !CFEqual(used, shaper) {
                return None;
            }
            let count = CTRunGetGlyphCount(run).max(0) as usize;
            let (mut ids, mut places) = (vec![0u16; count], vec![NSPoint::default(); count]);
            CTRunGetGlyphs(run, CFRange { location: 0, length: 0 }, ids.as_mut_ptr());
            CTRunGetPositions(run, CFRange { location: 0, length: 0 }, places.as_mut_ptr());
            let placed = ids.into_iter().zip(places).filter(|(glyph, _)| *glyph != u16::MAX);
            glyphs.extend(placed.map(|(glyph, at)| Set { glyph, at: [at.x as f32, at.y as f32] }));
        }
        Some(CTLineGetTypographicBounds(line.0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) as f32)
    }
}
