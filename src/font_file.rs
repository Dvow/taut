//! Font files: reading TrueType and OpenType fonts, and drawing text with them. For apps
//! shipping their own fonts or icon fonts, looking the same on every system and any pixel
//! buffer.
//!
//! A font file is a set of tables. Some give metrics (`head`, `hhea`, `maxp`, `hmtx`), `cmap`
//! maps characters to glyphs, and `GPOS` (in older fonts `kern`) kerns glyph pairs. Glyphs are
//! outlines: in `glyf` as on-curve and control points, in `CFF ` or `CFF2` as charstrings (see
//! `cff.rs`). Either becomes a [`Path`]. A variable font, adjustable along a few axes (weight,
//! width…), gives the deltas (see `variable_fonts.rs`); a collection holds several fonts in one
//! file. To draw text, the canvas rasterizes each glyph once per size into a coverage mask,
//! then reuses the mask. Small glyphs are first hinted vertically, as FreeType's light hinting
//! does (see `hinting.rs`), unless the app asks for unhinted glyphs ([`AppFonts::unhinted`]).
//! With the feature `hinting`, a TrueType font's own instructions do it where it has them
//! (`truetype_hinting.rs`).
//!
//! The file is read in place and trusted in nothing: every offset and count is checked, and no
//! glyph may cost more work than a real one. A damaged font is no font, or has blank glyphs; it
//! never panics. Only WOFF is decompressed first, into the original file, which is then read as
//! any other; and WOFF2 (`woff2.rs`) with the feature `woff2`.
//!
//! Pairs are kerned as HarfBuzz kerns left-to-right text: by the pairs `GPOS` lists under its
//! `kern` feature for the characters' script, or by `kern` where `GPOS` has no such feature. A
//! pair is looked up in the file on each request, by binary search through a few lists, with
//! nothing precomputed; drawn text caches the pairs it met. With the feature `shaping`, a
//! window's text is shaped instead, as a page's is (`shaping.rs`): each line once, kept by its
//! text; its glyphs still land on whole pixels.
//!
//! An emoji font has more, which `emoji.rs` draws with. Its glyphs are coloured in one or more
//! of four ways: as layered outlines, each filled with a palette colour (`COLR` v0 and `CPAL`),
//! or as bitmaps in a few sizes, PNGs inside the font (`CBLC` and `CBDT`, or `sbix`); or in the
//! newer formats, which `colr.rs` draws in programs that ask for them: paint graphs (`COLR`
//! v1), or SVG documents (`SVG `). And several characters may form one emoji: a flag is two
//! letters, a family is people with joiners. `GSUB` gives the glyphs replacing such a sequence
//! (one for all, or one per person), and `GPOS` positions glyphs drawn together: `shaping.rs`
//! applies both, as for Hebrew and Arabic text. Such a font is large and little of it is used,
//! so it can be memory-mapped instead of read ([`FontFile::open`]): on Linux only a system
//! font, which is replaced, never truncated, while in use.
//!
//! Not done: `morx` is not read.
use crate::canvas::{Canvas, coverage};
use crate::color::Color;
use crate::geom::{Point, pt};
use crate::path::Path;
use crate::png::inflate;
use crate::scripts::Language;
use crate::shaping::{Plan, Script};
use crate::text::{Face, Font, Metrics, TextSystem};
use std::ops::Range;

/// More points or commands than this in one glyph leaves it blank. No real glyph comes near,
/// and it keeps a damaged file from keeping the reader busy.
const MOST: usize = 1 << 16;
/// The deepest nesting of composite glyphs. (A damaged font can make a glyph include itself.)
const DEEPEST: u8 = 16;
/// A glyph reaching further from its pen than this is left blank: no pixel buffer is that
/// large, so the font is damaged.
const FURTHEST: f32 = 1e6;
/// A glyph larger than this many pixels isn't cached as coverage: it is rasterized from its
/// outline each time.
const LARGEST: f32 = 65536.0;
/// Approximate cache size in bytes. Beyond it all are dropped and redrawn as needed.
const CACHE_LIMIT: usize = 4 << 20;
/// The initial capacity of the kerning pair cache per size.
#[cfg(not(feature = "shaping"))]
const PAIRS: usize = 256;

/// A slice of a font file. Numbers in it are big-endian. Reading outside it gives `None`, which
/// makes a damaged file harmless.
#[derive(Clone, Copy)]
pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl<'a> Bytes<'a> {
    /// `long` bytes from `at`.
    pub fn part(self, at: usize, long: usize) -> Option<Bytes<'a>> {
        self.0.get(at..at.checked_add(long)?).map(Bytes)
    }
    /// A table whose range was checked when the file was read.
    pub fn within(self, table: &Range<usize>) -> Bytes<'a> {
        Bytes(self.0.get(table.clone()).unwrap_or(&[]))
    }
    pub fn u8(self, at: usize) -> Option<u8> {
        self.0.get(at).copied()
    }
    pub fn u16(self, at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(self.part(at, 2)?.0.try_into().ok()?))
    }
    pub fn i16(self, at: usize) -> Option<i16> {
        self.u16(at).map(|number| number as i16)
    }
    pub fn u24(self, at: usize) -> Option<u32> {
        Some(u32::from(self.u16(at)?) << 8 | u32::from(self.u8(at + 2)?))
    }
    pub fn u32(self, at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(self.part(at, 4)?.0.try_into().ok()?))
    }
}

/// A file signature's four bytes as one big-endian number.
const fn name(letters: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*letters)
}

/// In a sorted sequence of `count` items, the index of the first not `before` the target: found
/// by binary search, never a scan.
pub(crate) fn first_not(count: usize, before: impl Fn(usize) -> Option<bool>) -> Option<usize> {
    let (mut from, mut to) = (0, count);
    while from < to {
        let middle = (from + to) / 2;
        (from, to) = if before(middle)? { (middle + 1, to) } else { (from, middle) };
    }
    Some(from)
}

/// The id the next font read gets.
static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// A font file's bytes: read into memory, or memory-mapped (a font opened in place: see
/// [`FontFile::open`]).
enum Held {
    Read(Vec<u8>),
    Lying(Lying),
}

impl std::ops::Deref for Held {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Held::Read(bytes) => bytes,
            // SAFETY: the system keeps these bytes mapped where it said, for as long as the
            // `Lying` that mapped them lives.
            Held::Lying(lying) => unsafe { std::slice::from_raw_parts(lying.at, lying.long) },
        }
    }
}

/// A file mapped into memory without reading it. Pages load when first touched, and the system
/// may evict them under memory pressure. So a large font of which little is used costs only
/// what is used: an emoji font is 12 MB, and a window shows a handful.
struct Lying {
    at: *const u8,
    long: usize,
}

// SAFETY: the mapping is only read, and owned by nothing but this.
unsafe impl Send for Lying {}
unsafe impl Sync for Lying {}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingW(file: usize, security: usize, protect: u32, high: u32, low: u32, name: usize) -> usize;
    fn MapViewOfFile(mapping: usize, access: u32, high: u32, low: u32, long: usize) -> *const u8;
    fn UnmapViewOfFile(at: *const u8) -> i32;
    fn CloseHandle(handle: usize) -> i32;
}
#[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
unsafe extern "C" {
    fn mmap(at: *mut std::ffi::c_void, long: usize, protect: i32, flags: i32, file: i32, from: i64) -> *mut std::ffi::c_void;
    fn munmap(at: *mut std::ffi::c_void, long: usize) -> i32;
}

impl Lying {
    /// Maps an open file. `None` if the system won't (or this system isn't supported): the file
    /// is then read as usual.
    fn of(file: &std::fs::File) -> Option<Lying> {
        let long = usize::try_from(file.metadata().ok()?.len()).ok().filter(|long| *long > 0)?;
        // On Linux, a mapped file another program truncates kills this one on the next read
        // past the new end. So only files the user cannot write are mapped: system files, which
        // are replaced (a new file moved into place), never truncated. Others are read.
        #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
        if !Lying::untouchable(file) {
            return None;
        }
        // SAFETY: the file is open. Windows creates a read-only (2) unnamed mapping of all of
        // it, with default security (the zeros), and maps a read-only (4) view; the view stays
        // valid until unmapped, whatever happens to the file or the mapping handle.
        #[cfg(windows)]
        let at = unsafe {
            let mapping = CreateFileMappingW(std::os::windows::io::AsRawHandle::as_raw_handle(file) as usize, 0, 2, 0, 0, 0);
            let at = MapViewOfFile(mapping, 4, 0, 0, 0);
            CloseHandle(mapping);
            at
        };
        // SAFETY: the file is open. Linux maps all of it read-only (1) and private (2),
        // returning -1 on failure.
        #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
        let at = unsafe { mmap(std::ptr::null_mut(), long, 1, 2, std::os::fd::AsRawFd::as_raw_fd(file), 0) }.cast_const().cast::<u8>();
        #[cfg(not(any(windows, all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))))]
        let at = std::ptr::null::<u8>();
        (!at.is_null() && at as isize != -1).then_some(Lying { at, long })
    }

    /// Whether a file is unwritable by this process's user: a system file (owned by root,
    /// writable by nobody else), with this process not running as root.
    #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
    fn untouchable(file: &std::fs::File) -> bool {
        use std::os::unix::fs::MetadataExt;
        let (Ok(file), Ok(me)) = (file.metadata(), std::fs::metadata("/proc/self")) else { return false };
        file.uid() == 0 && file.mode() & 0o022 == 0 && me.uid() != 0
    }
}

impl Drop for Lying {
    fn drop(&mut self) {
        // SAFETY: the mapping is unmapped once, and not touched after.
        #[cfg(windows)]
        unsafe {
            UnmapViewOfFile(self.at);
        }
        #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
        unsafe {
            munmap(self.at.cast_mut().cast(), self.long);
        }
    }
}

/// One font, read from a `.ttf`, `.otf`, `.ttc`, `.woff` or `.woff2` file.
pub struct FontFile {
    file: Held,
    /// Units per em.
    units: f32,
    /// Ascender and descender, in font units; and cap-height and x-height (0 if it has no "H",
    /// no "x").
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    cap: f32,
    small: f32,
    /// Glyph count.
    glyphs: u16,
    /// Where its table directory starts in the file: at 0, except in a collection.
    pub(crate) start: usize,
    /// For a variable font, its instance coordinates, if not the default (see
    /// `variable_fonts.rs`).
    pub(crate) varied: Option<Box<crate::variable_fonts::Varied>>,
    /// `hmtx`'s range, and its number of long metrics.
    widths: Range<usize>,
    listed: usize,
    /// For vertical text: `vmtx`'s range and its number of long metrics, and `VORG`'s range
    /// (empty in fonts without them).
    heights: Range<usize>,
    tall: usize,
    origins: Range<usize>,
    map: Map,
    shapes: Shapes,
    kerning: Kerning,
    pictures: Pictures,
    /// The ranges of `GSUB`, `GPOS` and `GDEF` (see `shaping.rs`), and what emoji use of them,
    /// in a colour font.
    layout: [Range<usize>; 3],
    emoji: Plan,
    /// A unique id among the fonts the program read.
    serial: u32,
    /// Its blue zones, found when a glyph is first hinted (see `hinting.rs`).
    zones: std::sync::OnceLock<crate::hinting::Zones>,
}

/// Where the glyph outlines are.
enum Shapes {
    /// TrueType: points, in `glyf`.
    Points(Points),
    /// OpenType with PostScript outlines: charstrings, in `CFF ` or `CFF2`. (Only with the
    /// feature `cff`: a program can't tell from its calls which fonts it will get. Without it,
    /// such a font can't be read.)
    Programs(crate::cff::Programs),
    /// None: a bitmap-only font.
    None,
}

impl std::fmt::Debug for FontFile {
    /// Glyph count, units per em, and whether variable or with bitmaps; not its bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (varies, pictured) = (self.varied.is_some(), self.pictured());
        f.debug_struct("FontFile").field("glyphs", &self.glyphs).field("units", &self.units).field("varies", &varies).field("pictured", &pictured).finish_non_exhaustive()
    }
}

impl FontFile {
    /// Reads a font file: bytes read from one, or embedded with `include_bytes!`. `None` if not
    /// a readable font: a PostScript-outline font (usually `.otf`) needs the feature `cff`, and
    /// a WOFF2 file the feature `woff2`.
    pub fn read(file: impl AsRef<[u8]> + Into<Vec<u8>>) -> Option<FontFile> {
        // WOFF is decompressed first, then is a file like any other: uncompressed fonts pay
        // nothing for it. (The compressed one is read in place: embedded bytes are not copied.)
        // WOFF2 only with the feature `woff2`.
        let unpacked = match file.as_ref().get(..4) {
            Some(b"wOFF") => unpacked(Bytes(file.as_ref()))?,
            Some(b"wOF2") if cfg!(feature = "woff2") => crate::woff2::unpacked(file.as_ref())?,
            Some(b"wOF2") => return None,
            _ => file.into(),
        };
        FontFile::of(Held::Read(unpacked), 0)
    }

    /// Opens a font file memory-mapped instead of read: for a large font of which little is
    /// used, such as the system's emoji font. (The file must not be written while in use;
    /// system fonts are replaced, not written.)
    pub fn open(path: impl AsRef<std::path::Path>) -> Option<FontFile> {
        let lying = Lying::of(&std::fs::File::open(&path).ok()?).map(Held::Lying);
        match lying.filter(|lying| !lying.starts_with(b"wOFF") && !lying.starts_with(b"wOF2")) {
            Some(lying) => FontFile::of(lying, 0),
            None => FontFile::read(std::fs::read(path).ok()?),
        }
    }

    /// The `nth` font of a collection (from 0). `None` past the last, or for a single-font file
    /// and `nth` other than 0. (See also `named`, which finds one by name.)
    pub fn nth(self, nth: usize) -> Option<FontFile> {
        FontFile::of(self.file, nth)
    }

    /// The file's bytes.
    pub(crate) fn bytes(&self) -> Bytes<'_> {
        Bytes(&self.file)
    }

    /// Reads the `nth` font of an uncompressed file: the only one, if not a collection.
    fn of(file: Held, nth: usize) -> Option<FontFile> {
        let all = Bytes(&file);
        let start = crate::variable_fonts::start(all, nth)?;
        if ![0x0001_0000, name(b"OTTO"), name(b"true")].contains(&all.u32(start)?) {
            return None;
        }
        let table = |called: &[u8; 4]| listed(all, start, called);
        let (head, hhea, maxp) = (all.within(&table(b"head")?), all.within(&table(b"hhea")?), all.within(&table(b"maxp")?));
        let units = head.u16(18).filter(|units| *units != 0)? as f32;
        let (ascent, descent, glyphs, listed) = (hhea.i16(4)? as f32, -(hhea.i16(6)? as f32), maxp.u16(4)?, hhea.u16(34)? as usize);
        // A colour font may have no outlines; and only such a font is asked about
        // multi-character sequences.
        let pictures = Pictures::read(all, [b"COLR", b"CPAL", b"CBLC", b"CBDT", b"sbix", b"SVG "].map(&table));
        let shapes = match (table(b"glyf"), table(b"CFF ").or_else(|| table(b"CFF2"))) {
            (Some(glyf), _) => Shapes::Points(Points { glyf, loca: table(b"loca")?, long: head.i16(50)? != 0 }),
            (None, Some(programs)) if cfg!(feature = "cff") => Shapes::Programs(crate::cff::Programs::read(all, programs)?),
            (None, Some(_)) => return None,
            (None, None) => pictures.any().then_some(Shapes::None)?,
        };
        let (widths, map) = (table(b"hmtx")?, Map::best(all, table(b"cmap")?)?);
        let (heights, origins) = (table(b"vmtx").unwrap_or_default(), table(b"VORG").unwrap_or_default());
        let tall = table(b"vhea").and_then(|vhea| all.within(&vhea).u16(34)).unwrap_or(0) as usize;
        let kerning = Kerning::read(all, table(b"kern"), table(b"GPOS"));
        let layout = [b"GSUB", b"GPOS", b"GDEF"].map(|name| table(name).unwrap_or_default());
        let mut font = FontFile {
            file,
            units,
            ascent,
            descent,
            cap: 0.0,
            small: 0.0,
            glyphs,
            start,
            varied: None,
            widths,
            listed,
            heights,
            tall,
            origins,
            map,
            shapes,
            kerning,
            pictures,
            layout,
            emoji: Plan::default(),
            serial: SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            zones: std::sync::OnceLock::new(),
        };
        if font.pictures.any() {
            font.emoji = Plan::looked_up(&font, Script::None, false, Language::NONE);
        }
        font.measure();
        Some(font)
    }

    /// Measures the font's cap-height and x-height, in font units: the tops of its "H" and "x",
    /// y down. (A font lacking the letter maps it to glyph 0, which isn't one.) Its blue zones
    /// are recomputed on next use.
    pub(crate) fn measure(&mut self) {
        let reach = |letter: char| Some(self.glyph(letter)).filter(|glyph| *glyph != 0).map_or(0.0, |glyph| (-self.sketch(glyph, Placing::sized(1.0, 0.0)).reach[1]).max(0.0));
        (self.cap, self.small) = (reach('H'), reach('x'));
        self.zones = std::sync::OnceLock::new();
    }

    /// Units per em.
    pub(crate) fn units(&self) -> f32 {
        self.units
    }

    /// A variable font's instance coordinates, -1 to 1 per axis in 16384ths: empty at the
    /// default.
    pub(crate) fn place(&self) -> &[i32] {
        self.varied.as_ref().map_or(&[], |varied| &varied.place)
    }

    /// Its blue zones (see `hinting.rs`), found on first use.
    pub(crate) fn zones(&self) -> &crate::hinting::Zones {
        self.zones.get_or_init(|| crate::hinting::Zones::of(self))
    }

    /// How much of the em lies above the baseline: the typographic ascent over the ascent and
    /// descent (`OS/2`), else the line's. Ruby is set against the em box so (as Chrome does).
    /// (A WOFF font's `OS/2` is not kept, without `hinting`.)
    #[cfg(feature = "ruby")]
    pub(crate) fn em_above(&self) -> f32 {
        let all = Bytes(&self.file);
        let typo = listed(all, self.start, b"OS/2").map_or(Bytes(&[]), |range| all.within(&range));
        let ends = typo.i16(68).zip(typo.i16(70)).filter(|(up, down)| up > &0 && down <= &0);
        ends.map_or(self.ascent / (self.ascent + self.descent), |(up, down)| up as f32 / (up as f32 - down as f32))
    }

    /// Its line metrics at this size: ascent and descent.
    pub fn metrics(&self, size: f32) -> Metrics {
        let part = |of: f32| of * size / self.units;
        Metrics { ascent: part(self.ascent), descent: part(self.descent), cap_height: part(self.cap), x_height: part(self.small) }
    }

    /// The advance of this character at this size.
    pub fn advance(&self, letter: char, size: f32) -> f32 {
        self.width(self.glyph(letter)) * size / self.units
    }

    /// The kerning between two characters at this size: negative is closer.
    pub fn kerning(&self, left: char, right: char, size: f32) -> f32 {
        // A font without kerning isn't asked for the glyphs.
        if self.kerning.lists.is_empty() {
            return 0.0;
        }
        self.closer((self.glyph(left), script(left)), (self.glyph(right), script(right))) * size / self.units
    }

    /// A character's outline at this size, as a [`Path`] in units: x right from the pen, y down
    /// from the line's top. For an icon font's icon:
    /// `ui.el().size(16.0, 16.0).shape(&icons.outline('\u{e88a}', 16.0), color).show()`.
    pub fn outline(&self, letter: char, size: f32) -> Path {
        self.sketch(self.glyph(letter), Placing::sized(size / self.units, self.metrics(size).ascent)).path
    }

    /// A character's glyph: 0, the notdef glyph, if the font lacks it.
    pub(crate) fn glyph(&self, letter: char) -> u16 {
        let found = self.map.glyph(Bytes(&self.file).within(&self.map.at), letter as u32);
        found.filter(|glyph| *glyph < self.glyphs).unwrap_or(0)
    }

    /// The glyph for a character followed by a variation selector (U+FE00 to U+FE0F, or from
    /// U+E0100): its own glyph for the pair (emoji or text presentation, a CJK variant), or the
    /// character's default glyph where the font says the selector maps to it. `None` if the
    /// font says nothing of the pair: as in HarfBuzz, both are then glyphs, and its lookups may
    /// combine them. The format 14 subtable gives, per selector in order, the default-glyph
    /// ranges (first character and count) and the non-default mappings (with the glyph).
    pub(crate) fn variant(&self, letter: char, selector: char) -> Option<u16> {
        let (list, code, selector) = (Bytes(&self.file).within(&self.map.variants), letter as u32, selector as u32);
        let count = (list.u32(6)? as usize).min(list.0.len() / 11);
        let n = first_not(count, |n| Some(list.u24(10 + n * 11)? < selector))?;
        (n < count && list.u24(10 + n * 11)? == selector).then_some(())?;
        let (usual, own) = (list.u32(13 + n * 11)? as usize, list.u32(17 + n * 11)? as usize);
        let stretches = if usual == 0 { 0 } else { (list.u32(usual)? as usize).min(list.0.len() / 4) };
        let m = first_not(stretches, |m| Some(list.u24(usual + 4 + m * 4)? + u32::from(list.u8(usual + 7 + m * 4)?) < code))?;
        if m < stretches && list.u24(usual + 4 + m * 4)? <= code {
            return Some(self.glyph(letter)).filter(|glyph| *glyph != 0);
        }
        let owned = if own == 0 { 0 } else { (list.u32(own)? as usize).min(list.0.len() / 5) };
        let m = first_not(owned, |m| Some(list.u24(own + 4 + m * 5)? < code))?;
        let glyph = list.u16(own + 7 + m * 5).filter(|_| m < owned && list.u24(own + 4 + m * 5) == Some(code))?;
        Some(glyph).filter(|glyph| *glyph != 0 && *glyph < self.glyphs)
    }

    /// A glyph's advance in font units. `hmtx` has four bytes per glyph, the first two its
    /// advance; a font's last glyphs may share the last listed advance.
    pub(crate) fn width(&self, glyph: u16) -> f32 {
        let listed = (glyph as usize).min(self.listed.saturating_sub(1));
        let width = Bytes(&self.file).within(&self.widths).u16(listed * 4).unwrap_or(0) as f32;
        // A variable font instance gives the advance delta there.
        width + self.varied.as_ref().map_or(0.0, |varied| varied.wider(Bytes(&self.file), glyph, self.points(glyph)))
    }

    /// A glyph's vertical layout in font units, as HarfBuzz does: its vertical advance (from
    /// `vmtx`, else the line height), and its origin's offset left of and below the pen. Left
    /// by half its width, centring it in the column; down by its vertical origin, from `VORG`,
    /// else `vmtx` (its top plus top side bearing), else centring its box in the line height.
    pub(crate) fn downward(&self, glyph: u16) -> (i32, [i32; 2]) {
        let all = Bytes(&self.file);
        let line = (self.ascent + self.descent) as i32;
        let (heights, origins) = (all.within(&self.heights), all.within(&self.origins));
        let listed = (glyph as usize).min(self.tall.saturating_sub(1));
        let advance = if heights.0.is_empty() { line } else { heights.u16(listed * 4).unwrap_or(0) as i32 };
        let across = self.width(glyph) as i32 / 2;
        if origins.u16(0) == Some(1) {
            let count = (origins.u16(6).unwrap_or(0) as usize).min(origins.0.len() / 4);
            let n = first_not(count, |n| Some(origins.u16(8 + n * 4)? < glyph)).unwrap_or(count);
            let own = origins.i16(10 + n * 4).filter(|_| n < count && origins.u16(8 + n * 4) == Some(glyph));
            return (advance, [across, own.or(origins.i16(4)).unwrap_or(0) as i32]);
        }
        // A `glyf` glyph's bounding box precedes its points: its bottom 4 bytes in, its top 8.
        let points = self.points(glyph);
        let top = points.and_then(|points| points.i16(8)).map_or(0, |top| top as i32);
        if !heights.0.is_empty() && matches!(self.shapes, Shapes::Points(_)) {
            // A composite uses the component flagged USE_MY_METRICS, recursively, offset as
            // that component is.
            let (mut ruling, mut moved) = (glyph, Placing([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
            for _ in 0..DEEPEST {
                let Some((part, place)) = self.points(ruling).and_then(named) else { break };
                (ruling, moved) = (part, moved.after(place));
            }
            let top = self.points(ruling).and_then(|points| points.i16(8)).map_or(0, |top| top as i32);
            let room = if (ruling as usize) < self.tall { heights.i16(ruling as usize * 4 + 2) } else { heights.i16(self.tall * 4 + (ruling as usize - self.tall) * 2) };
            return (advance, [across, moved.of(pt(0.0, (top + room.unwrap_or(0) as i32) as f32)).y.round() as i32]);
        }
        let [low, high] = match points {
            Some(points) => [points.i16(4).unwrap_or(0) as i32, top],
            None => [0, 1].map(|n| -self.sketch(glyph, Placing::sized(1.0, 0.0)).reach[3 - 2 * n] as i32),
        };
        (advance, [across, high + ((line + low - high) >> 1)])
    }

    /// A glyph's bytes in `glyf`: `None` for a charstring font.
    pub(crate) fn points(&self, glyph: u16) -> Option<Bytes<'_>> {
        let Shapes::Points(points) = &self.shapes else { return None };
        points.bytes(Bytes(&self.file), glyph)
    }

    /// A glyph's left side bearing in font units: `hmtx`'s other two bytes per glyph. Glyphs
    /// sharing the last advance have theirs after all the advances.
    pub(crate) fn edge(&self, glyph: u16) -> Option<f32> {
        let glyph = glyph as usize;
        let at = if glyph < self.listed { glyph * 4 + 2 } else { self.listed * 4 + (glyph - self.listed) * 2 };
        Bytes(&self.file).within(&self.widths).i16(at).map(|edge| edge as f32)
    }

    /// The kerning between two glyphs, in font units.
    fn closer(&self, left: Kerned, right: Kerned) -> f32 {
        // Text in two scripts is two runs, unkerned where they meet. A script-neutral character
        // (a digit, a space, a full stop) goes with its neighbour.
        let script = match (left.1, right.1) {
            (0, script) | (script, 0) => script,
            (one, other) if one == other => one,
            _ => return 0.0,
        };
        let (all, mut sum, mut last_lookup) = (Bytes(&self.file), 0.0, None);
        for pairs in &self.kerning.lists[self.kerning.scripts[script as usize].clone()] {
            let Some(by) = (last_lookup != Some(pairs.lookup)).then(|| pairs.find(all, left.0, right.0, self.varied.as_deref())).flatten() else { continue };
            (sum, last_lookup) = (if pairs.instead { by } else { sum + by }, Some(pairs.lookup));
        }
        sum
    }

    /// A glyph's outline, each point transformed by `place`. A damaged glyph, or one larger
    /// than any real one, is blank rather than partial: no path, an empty box.
    pub(crate) fn sketch(&self, glyph: u16, place: Placing) -> Sketch {
        let mut sketch = Sketch::new();
        let whole = match &self.shapes {
            Shapes::Points(points) => points.draw(self, glyph, place, &mut sketch),
            Shapes::Programs(programs) if cfg!(feature = "cff") => programs.draw(self, glyph, place, &mut sketch),
            Shapes::Programs(_) | Shapes::None => None,
        };
        // A glyph with no points has no box either, and counts as too far out: blank.
        let near = sketch.reach.iter().all(|edge| edge.abs() < FURTHEST);
        if whole.is_some() && near { sketch } else { Sketch { reach: [0.0; 4], ..Sketch::new() } }
    }
}

/// A table's range in the file, for the font whose directory starts at `start`: the directory
/// lists each table's tag, checksum, offset and length.
pub(crate) fn listed(all: Bytes, start: usize, called: &[u8; 4]) -> Option<Range<usize>> {
    let tables = all.part(start + 12, all.u16(start + 4)? as usize * 16)?;
    let entry = Bytes(tables.0.as_chunks::<16>().0.iter().find(|entry| entry.starts_with(called))?);
    let (at, long) = (entry.u32(8)? as usize, entry.u32(12)? as usize);
    all.part(at, long).map(|_| at..at + long)
}

/// The tables read here: every font's (including multi-character sequence and vertical text
/// tables), then variable fonts' (with style names), then colour fonts' only.
#[rustfmt::skip]
const READ: [&[u8; 4]; 28] = [
    b"CFF ", b"GDEF", b"GPOS", b"GSUB", b"VORG", b"cmap", b"glyf", b"head", b"hhea", b"hmtx", b"kern", b"loca", b"maxp",
    b"vhea", b"vmtx",
    b"CFF2", b"HVAR", b"MVAR", b"avar", b"fvar", b"gvar", b"name",
    b"CBDT", b"CBLC", b"COLR", b"CPAL", b"sbix", b"SVG ",
];

/// Of the tables a font has (`has`), those read here, in `READ` order (and with the feature
/// `hinting`, its programs and what they read): only these are decompressed from WOFF, and taken
/// from a font Windows provides. (An East Asian font also has embedded bitmaps and hinting
/// instructions: a third of it.)
pub(crate) fn taken(has: impl Fn(&[u8; 4]) -> bool) -> impl Iterator<Item = &'static [u8; 4]> {
    let coloured = [b"CBDT", b"COLR", b"sbix", b"SVG "].into_iter().any(&has);
    let read = READ.into_iter().take(if coloured { READ.len() } else { 22 });
    #[cfg(feature = "hinting")]
    let read = read.chain([b"cvt ", b"fpgm", b"prep", b"cvar", b"VVAR", b"OS/2", b"vhea"]);
    read.filter(move |name| has(name))
}

/// The header of a font file of these tables (each with its length), which follow in order: the
/// sfnt version and table count in 12 bytes, then 16 per table: tag, checksum, offset, length.
/// (What isn't read here is left zero.)
pub(crate) fn listing<'a>(kind: &[u8], tables: impl ExactSizeIterator<Item = (&'a [u8; 4], usize)>) -> Vec<u8> {
    let count = tables.len();
    let mut file = Vec::with_capacity(12 + count * 16);
    file.extend([kind, &(count as u16).to_be_bytes(), &[0; 6]].concat());
    let mut at = 12 + count * 16;
    for (name, long) in tables {
        file.extend([&name[..], &[0; 4], &(at as u32).to_be_bytes(), &(long as u32).to_be_bytes()].concat());
        at += long;
    }
    file
}

/// A WOFF file as its original sfnt, minus the tables not read here. It starts with a 44-byte
/// header, whose bytes 4 to 8 give the flavour and the next but four the table count. Then 20
/// bytes per table: tag, offset, compressed and original length, checksum. A table is stored as
/// a zlib stream, or uncompressed where that wouldn't be shorter.
fn unpacked(packed: Bytes) -> Option<Vec<u8>> {
    let entries = packed.part(44, packed.u16(12)? as usize * 20)?;
    let entry = |name: &[u8; 4]| entries.0.as_chunks::<20>().0.iter().find(|entry| entry.starts_with(name)).map(|entry| Bytes(entry));
    // The tables present among those read here, each with its original length.
    let mut tables = Vec::new();
    for name in taken(|name| entry(name).is_some()) {
        let entry = entry(name)?;
        tables.push((name, entry.u32(12)? as usize, entry));
    }
    // Each table is decompressed into place before the next, and space is reserved only once it
    // decompressed to its stated length: claims about tables not yet decompressed cost nothing.
    // (Nor may they total more than a few times the compressed size, which no font does.)
    let all = 12 + tables.len() * 16 + tables.iter().map(|(_, long, _)| long).sum::<usize>();
    if all > packed.0.len().saturating_mul(64) {
        return None;
    }
    let mut file = listing(packed.part(4, 4)?.0, tables.iter().map(|(name, long, _)| (*name, *long)));
    for (_, long, entry) in tables {
        let lying = packed.part(entry.u32(4)? as usize, entry.u32(8)? as usize)?.0;
        let unpacked = match lying.len() == long {
            true => None,
            false => Some(inflate(lying, long).filter(|table| table.len() == long)?),
        };
        file.reserve_exact(long);
        file.extend_from_slice(unpacked.as_deref().unwrap_or(lying));
    }
    Some(file)
}

/// The `cmap` subtable mapping characters to glyphs: its range (to the table's end), and which
/// of three formats; and the format 14 variation sequences subtable (empty in fonts without
/// one).
struct Map {
    at: Range<usize>,
    kind: u16,
    variants: Range<usize>,
}

impl Map {
    /// Picks the best `cmap` subtable. Each is for one encoding: full Unicode is best, the BMP
    /// next, and Windows' symbol encoding last. Formats other than the three known here are
    /// skipped.
    fn best(all: Bytes, cmap: Range<usize>) -> Option<Map> {
        let table = all.within(&cmap);
        let lists = table.part(4, table.u16(2)? as usize * 8)?;
        let rated = |list: Bytes| {
            let (at, system) = (list.u32(4)? as usize, (list.u16(0)?, list.u16(2)?));
            let rating = match system {
                (0, 4..) | (3, 10) => 3,
                (0, _) | (3, 1) => 2,
                (3, 0) => 1,
                _ => return None,
            };
            table.u16(at).filter(|kind| matches!(kind, 4 | 6 | 12)).map(|kind| (rating, at, kind))
        };
        let (_, at, kind) = lists.0.as_chunks::<8>().0.iter().filter_map(|list| rated(Bytes(list))).max_by_key(|(rating, ..)| *rating)?;
        // The variation sequences subtable is Unicode, encoding 5.
        let variants = lists.0.as_chunks::<8>().0.iter().map(|list| Bytes(list)).find(|list| list.u16(0) == Some(0) && list.u16(2) == Some(5));
        let variants = variants.and_then(|list| list.u32(4)).map(|at| at as usize).filter(|at| table.u16(*at) == Some(14));
        Some(Map { at: cmap.start + at..cmap.end, kind, variants: variants.map_or(0..0, |at| cmap.start + at..cmap.end) })
    }

    /// A character's glyph, if the font has one.
    fn glyph(&self, list: Bytes, code: u32) -> Option<u16> {
        match self.kind {
            4 => by_stretch(list, code),
            12 => by_group(list, code),
            _ => by_row(list, code),
        }
    }
}

/// Format 4, for the BMP: segments of consecutive characters. Four arrays have an entry per
/// segment: end code, start code, delta, and (where a delta won't do) an offset from that very
/// entry to a glyph array, one per character of the segment.
fn by_stretch(list: Bytes, code: u32) -> Option<u16> {
    let (code, count) = (u16::try_from(code).ok()?, list.u16(6)? as usize / 2);
    // Two padding bytes lie between the first array and the second.
    let row = |row: usize, n: usize| list.u16(14 + row * count * 2 + row.min(1) * 2 + n * 2);
    let n = first_not(count, |n| Some(row(0, n)? < code))?;
    let (first, add, onward) = (row(1, n)?, row(2, n)?, row(3, n)? as usize);
    if n == count || first > code {
        return None;
    }
    if onward == 0 {
        return Some(code.wrapping_add(add));
    }
    let listed = list.u16(16 + count * 6 + n * 2 + onward + (code - first) as usize * 2)?;
    (listed != 0).then(|| listed.wrapping_add(add))
}

/// Format 12, any characters: groups again, each with first and last character and the first
/// one's glyph; the rest follow consecutively.
fn by_group(list: Bytes, code: u32) -> Option<u16> {
    let count = (list.u32(12)? as usize).min(list.0.len() / 12);
    let n = first_not(count, |n| Some(list.u32(20 + n * 12)? < code))?;
    let (first, glyph) = (list.u32(16 + n * 12)?, list.u32(24 + n * 12)?);
    if n == count || first > code {
        return None;
    }
    u16::try_from(glyph.checked_add(code - first)?).ok()
}

/// Format 6, a short run: the first character, the count, and a glyph each.
fn by_row(list: Bytes, code: u32) -> Option<u16> {
    let n = code.checked_sub(list.u16(6)? as u32)? as usize;
    if n < list.u16(8)? as usize { list.u16(10 + n * 2) } else { None }
}

/// A glyph and its character's script: what kerning depends on.
type Kerned = (u16, u8);

/// A character's script for kerning: 1 Latin, 2 Greek, 3 Cyrillic, 0 for everything else (the
/// common characters, digits, spaces, marks, and scripts not told apart here). (Per Unicode, as
/// shaped text on a page is: see `scripts.rs`.)
pub(crate) fn script(letter: char) -> u8 {
    let code = letter as u32;
    let n = KERNED.partition_point(|entry| entry >> 2 <= code);
    n.checked_sub(1).map_or(0, |n| (KERNED[n] & 3) as u8)
}

/// Latin, Greek and Cyrillic characters, in ranges: each range's first character (times 4) with
/// its script, 1, 2 or 3 (0 for any other). From Unicode 17.
#[rustfmt::skip]
const KERNED: [u32; 150] = [
    0x105, 0x16c, 0x185, 0x1ec, 0x2a9, 0x2ac, 0x2e9, 0x2ec, 0x301, 0x35c, 0x361, 0x3dc,
    0x3e1, 0xae4, 0xb81, 0xb94, 0xdc2, 0xdd0, 0xdd6, 0xde0, 0xdea, 0xdf8, 0xdfe, 0xe00,
    0xe12, 0xe14, 0xe1a, 0xe1c, 0xe22, 0xe2c, 0xe32, 0xe34, 0xe3a, 0xe88, 0xe8e, 0xf88,
    0xfc2, 0x1003, 0x1214, 0x121f, 0x14c0, 0x7203, 0x722c, 0x7401, 0x749a, 0x74af, 0x74b1, 0x7576,
    0x7589, 0x759a, 0x75ad, 0x75e3, 0x75e5, 0x76fe, 0x7700, 0x7801, 0x7c02, 0x7c58, 0x7c62, 0x7c78,
    0x7c82, 0x7d18, 0x7d22, 0x7d38, 0x7d42, 0x7d60, 0x7d66, 0x7d68, 0x7d6e, 0x7d70, 0x7d76, 0x7d78,
    0x7d7e, 0x7df8, 0x7e02, 0x7ed4, 0x7eda, 0x7f14, 0x7f1a, 0x7f50, 0x7f5a, 0x7f70, 0x7f76, 0x7fc0,
    0x7fca, 0x7fd4, 0x7fda, 0x7ffc, 0x81c5, 0x81c8, 0x81fd, 0x8200, 0x8241, 0x8274, 0x849a, 0x849c,
    0x84a9, 0x84b0, 0x84c9, 0x84cc, 0x8539, 0x853c, 0x8581, 0x8624, 0xb181, 0xb200, 0xb783, 0xb800,
    0x29903, 0x29a80, 0x29c89, 0x29e20, 0x29e2d, 0x29f74, 0x29fc5, 0x2a000, 0x2acc1, 0x2ad6c, 0x2ad71, 0x2ad96,
    0x2ad99, 0x2ada8, 0x3ec01, 0x3ec1c, 0x3f8bb, 0x3f8c0, 0x3fc85, 0x3fcec, 0x3fd05, 0x3fd6c, 0x40502, 0x4063c,
    0x40682, 0x40684, 0x41e01, 0x41e18, 0x41e1d, 0x41ec4, 0x41ec9, 0x41eec, 0x74802, 0x74918, 0x77c01, 0x77c7c,
    0x77c95, 0x77cac, 0x780c3, 0x781b8, 0x7823f, 0x78240,
];

/// The `GPOS` script tags for each of those scripts, in lookup order: its own, then three
/// meaning any script. (The last two aren't meant to, but some fonts put their all-scripts
/// lookups there, and HarfBuzz looks there too.)
const SCRIPTS: [&[&[u8; 4]]; 4] = [&[b"DFLT", b"dflt", b"latn"], &[b"latn", b"DFLT", b"dflt"], &[b"grek", b"DFLT", b"dflt", b"latn"], &[b"cyrl", b"DFLT", b"dflt", b"latn"]];

/// More subtables than this in a font's kerning lookups aren't read, nor more lookups than this
/// per feature. (Of 900 fonts examined, the most subtables was 66.)
const LISTS: usize = 1024;

/// A font's kerning: every subtable of glyph pairs, and which apply to text in each script.
struct Kerning {
    lists: Vec<Pairs>,
    scripts: [Range<usize>; 4],
}

impl Kerning {
    /// Reads the two kerning tables. The older `kern` applies to every script alike. The newer
    /// `GPOS` has a `kern` feature per script, listing the lookups that kern it. A script with
    /// that feature is kerned by its lookups alone, one without it by `kern`: as HarfBuzz
    /// chooses, and so the browsers and desktops built on it.
    fn read(all: Bytes, kern: Option<Range<usize>>, gpos: Option<Range<usize>>) -> Kerning {
        let mut lists = kern.map_or(Vec::new(), |kern| Pairs::older(all, kern));
        let (older, start) = (0..lists.len(), gpos.as_ref().map_or(0, |gpos| gpos.start));
        let (table, mut room) = (gpos.map_or(Bytes(&[]), |gpos| all.within(&gpos)), LISTS);
        // Scripts kerned by the same lookups share their subtables.
        let mut shared: Vec<(Vec<u16>, Range<usize>)> = Vec::new();
        let scripts = SCRIPTS.map(|names| {
            let Some(lookups) = lookups(table, (names, &[], &[]), &[b"kern"]) else { return older.clone() };
            if let Some((_, lists)) = shared.iter().find(|(same, _)| *same == lookups) {
                return lists.clone();
            }
            let first = lists.len();
            lookups.iter().for_each(|lookup| _ = Pairs::newer(table, start, *lookup, &mut room, &mut lists));
            shared.push((lookups, first..lists.len()));
            first..lists.len()
        });
        Kerning { lists, scripts }
    }
}

/// What a table's lookups are selected by: the script tags to try, in order; the language tags,
/// the same; and a variable font's instance coordinates (-1 to 1 in 16384ths; empty at the
/// default).
pub(crate) type Asked<'a> = (&'a [&'a [u8; 4]], &'a [[u8; 4]], &'a [i32]);

/// The lookup indices a `GPOS` or `GSUB` table has for a script and language, under the
/// features tagged one of `wanted` (`kern`, for kerning), in order. The script is the first tag
/// tried the table lists, and the language the first the script lists; for a variable font, a
/// table's feature variations substitute features at its instance. `None` if the table has no
/// such feature for them. The table starts with its three list offsets: scripts, features,
/// lookups.
pub(crate) fn lookups(table: Bytes, (names, languages, place): Asked, wanted: &[&[u8; 4]]) -> Option<Vec<u16>> {
    let (scripts, features) = (table.u16(4).filter(|_| table.u16(0) == Some(1))? as usize, table.u16(6)? as usize);
    // ScriptList: sorted records of six bytes: tag, and offset from the list.
    let listed = |name: u32| {
        let (count, entry) = (table.u16(scripts)? as usize, |n: usize| scripts + 2 + n * 6);
        let n = first_not(count, |n| Some(table.u32(entry(n))? < name))?;
        table.u16(entry(n) + 4).filter(|_| n < count && table.u32(entry(n)) == Some(name))
    };
    let script = scripts + names.iter().find_map(|called| listed(name(called)))? as usize;
    // A Script table: the default LangSys offset (0: none), then LangSysRecords of six bytes:
    // tag, and offset from the script. As HarfBuzz does, the first of `languages` listed is
    // taken, else one tagged `dflt` (as some fonts name the default), else the default.
    let (count, entry) = (table.u16(script + 2)? as usize, |n: usize| script + 4 + n * 6);
    let system = |called: &[u8; 4]| (0..count).find(|n| table.u32(entry(*n)) == Some(name(called))).and_then(|n| table.u16(entry(n) + 4));
    let own = languages.iter().chain([b"dflt"]).find_map(system);
    let usual = script + own.or_else(|| table.u16(script)).filter(|at| *at != 0)? as usize;
    // A LangSys: its required feature (none, if 65535), then its feature count and indices.
    let required = table.u16(usual + 2).filter(|number| *number != 0xffff);
    let numbers = required.into_iter().chain((0..table.u16(usual + 4)? as usize).filter_map(move |n| table.u16(usual + 6 + n * 2)));
    // FeatureList: records of six bytes too: tag, and offset. Per wanted tag, the first feature
    // counts (or its substitute at the instance). A Feature gives its lookup count and indices.
    let (entries, instead) = (numbers.filter(|n| Some(*n) < table.u16(features)).map(|n| (n, features + 2 + n as usize * 6)), varied(table, place));
    let found = wanted.iter().filter_map(|called| entries.clone().find(|(_, entry)| table.u32(*entry) == Some(name(called))));
    let at = |(n, entry): (u16, usize)| instead.and_then(|instead| standing_in(table, instead, n)).or_else(|| Some(features + table.u16(entry + 4)? as usize));
    let first = at(found.clone().next()?)?;
    let others = found.skip(1).filter_map(at);
    let of = |feature: usize| (0..(table.u16(feature + 2).unwrap_or(0) as usize).min(LISTS)).filter_map(move |n| table.u16(feature + 4 + n * 2));
    let named = std::iter::once(first).chain(others).flat_map(of).take(LISTS);
    // Taken in index order, each once: the next is always the lowest above the last. (For a
    // font's few, that beats sorting.)
    let mut lookups: Vec<u16> = Vec::new();
    while let Some(next) = named.clone().filter(|lookup| lookups.last().is_none_or(|last| lookup > last)).min() {
        lookups.push(next);
    }
    Some(lookups)
}

/// A version 1.1 table's feature substitutions at a variable font instance: those of the first
/// condition set that holds there. A set lists its condition offsets; a condition gives its
/// axis and the range on it where it holds. (An empty set always holds.)
fn varied(table: Bytes, place: &[i32]) -> Option<usize> {
    let variations = table.u32(10).filter(|at| *at != 0 && table.u16(2).is_some_and(|minor| minor >= 1))? as usize;
    let count = (table.u32(variations + 4)? as usize).min(LISTS);
    let holds = |conditions: usize| (0..table.u16(conditions).unwrap_or(0) as usize).all(|n| condition(table, conditions, n, place) == Some(true));
    (0..count).find_map(|n| {
        let (conditions, instead) = (table.u32(variations + 8 + n * 8)? as usize, table.u32(variations + 12 + n * 8)? as usize);
        (conditions == 0 || holds(variations + conditions)).then_some(variations + instead)
    })
}

/// Whether condition `n` of a set holds at the instance: the instance's coordinate on its axis
/// (0 if unspecified) lies in its range.
fn condition(table: Bytes, conditions: usize, n: usize, place: &[i32]) -> Option<bool> {
    let at = conditions + table.u32(conditions + 2 + n * 4)? as usize;
    let on = place.get(table.u16(at + 2)? as usize).copied().unwrap_or(0);
    Some(table.u16(at)? == 1 && table.i16(at + 4)? as i32 <= on && on <= table.i16(at + 6)? as i32)
}

/// The substitute for feature `n`, from a substitution table (a count, then per record the
/// feature index and the substitute's offset).
fn standing_in(table: Bytes, instead: usize, n: u16) -> Option<usize> {
    let count = table.u16(instead + 4)? as usize;
    let found = (0..count).find(|m| table.u16(instead + 6 + m * 6) == Some(n))?;
    Some(instead + table.u32(instead + 8 + found * 6)? as usize)
}

/// One subtable of glyph pairs kerned closer or apart.
struct Pairs {
    /// Its offset in the file, and its format.
    at: usize,
    kind: Kind,
    /// `GPOS` subtables carry their lookup index here: the first with the pair speaks for the
    /// lookup, and the lookups' values add up. A `kern` subtable has its own index,
    lookup: u16,
    /// and may override what the subtables before it said, rather than add.
    instead: bool,
}

/// Pair subtable formats.
enum Kind {
    /// `kern` format 0: so many sorted pairs, six bytes each.
    Each(usize),
    /// `GPOS` PairPos format 1: per first glyph, its pair set.
    Seconds,
    /// `GPOS` PairPos format 2: glyph classes, and a value per first class and second class.
    Classes,
}

impl Pairs {
    /// The useful `kern` subtables: horizontal pair ones. At most eight are examined; a real
    /// font has one or two.
    fn older(all: Bytes, kern: Range<usize>) -> Vec<Pairs> {
        let (table, mut found, mut at) = (all.within(&kern), Vec::new(), 4);
        let lists = if table.u16(0) == Some(0) { table.u16(2).unwrap_or(0).min(8) } else { 0 };
        for lookup in 0..lists {
            let (Some(long), Some(kind), Some(count)) = (table.u16(at + 2), table.u16(at + 4), table.u16(at + 6)) else { break };
            // A subtable's two-byte length is too small for large ones: it is at least as long
            // as its pairs, if the file has room for them.
            let count = (count as usize).min(table.0.len().saturating_sub(at + 14) / 6);
            let pairs = kind & 0xff07 == 1;
            if pairs {
                found.push(Pairs { at: kern.start + at + 14, kind: Kind::Each(count), lookup, instead: kind & 8 != 0 });
            }
            at += (long as usize).max(if pairs { 14 + count * 6 } else { 6 });
        }
        found
    }

    /// Adds the pair subtables of one `GPOS` lookup. (The table starts at `start` in the file;
    /// `room` is how many subtables may still be examined.)
    fn newer(table: Bytes, start: usize, lookup: u16, room: &mut usize, lists: &mut Vec<Pairs>) -> Option<()> {
        for (kind, list) in lists_at(table, lookup_at(table, lookup)?, 9) {
            *room = room.checked_sub(1)?;
            // Pair subtables are type 2, in one of two formats. (Other types kern beyond pairs,
            // not done here.)
            let kind = match (kind, table.u16(list)?) {
                (2, 1) => Kind::Seconds,
                (2, 2) => Kind::Classes,
                _ => continue,
            };
            lists.push(Pairs { at: start + list, kind, lookup, instead: false });
        }
        Some(())
    }

    /// A pair's kerning by this subtable, in font units, for a variable font at `varied`.
    /// `None` if the pair isn't in it.
    fn find(&self, all: Bytes, left: u16, right: u16, varied: Option<&crate::variable_fonts::Varied>) -> Option<f32> {
        let list = Bytes(all.0.get(self.at..)?);
        let (by, moved) = match self.kind {
            Kind::Each(count) => (each(list, count, (left as u32) << 16 | right as u32)?, None),
            Kind::Seconds => by_second(list, left, right)?,
            Kind::Classes => by_class(list, left, right)?,
        };
        Some(by + varied.map_or(0.0, |varied| varied.device(all, list, moved)))
    }
}

/// A `kern` subtable: the pair is binary searched.
fn each(list: Bytes, count: usize, pair: u32) -> Option<f32> {
    let n = first_not(count, |n| Some(list.u32(n * 6)? < pair))?;
    let found = list.part(n * 6, 6).filter(|found| n < count && found.u32(0) == Some(pair))?;
    Some(found.i16(4)? as f32)
}

/// A `GPOS` subtable's value record layout: its size, and where the one value used here is, the
/// first glyph's x advance (`None` if absent); and where its device table offset is, for
/// variable fonts. A record may also hold placements or the second glyph's advance: two bytes
/// each, present per the subtable's value format bits.
fn written(list: Bytes) -> Option<(usize, [Option<usize>; 2])> {
    let (first, second) = (list.u16(4)?, list.u16(6)?);
    let long = (first.count_ones() + second.count_ones()) as usize * 2;
    let at = |bit: u16| (first & bit != 0).then_some((first & (bit - 1)).count_ones() as usize * 2);
    Some((long, [at(4), at(0x40)]))
}

/// The first glyph's x advance in the value record at `at` of a `GPOS` subtable (0 if absent);
/// and its variation table's offset in the subtable.
fn advanced(list: Bytes, at: usize, [advance, moved]: [Option<usize>; 2]) -> Option<(f32, Option<usize>)> {
    let by = advance.map_or(Some(0.0), |advance| Some(list.i16(at + advance)? as f32))?;
    Some((by, moved.map(|moved| at + moved)))
}

/// A PairPos format 1 subtable, with per covered first glyph its sorted pair set: the second
/// glyph is binary searched.
fn by_second(list: Bytes, left: u16, right: u16) -> Option<(f32, Option<usize>)> {
    let (first, (long, advance)) = (covered(list, list.u16(2)? as usize, left)?, written(list)?);
    let seconds = list.u16(10 + first * 2).filter(|at| *at != 0 && Some(first) < list.u16(8).map(usize::from))? as usize;
    let (count, entry) = (list.u16(seconds)? as usize, |n: usize| seconds + 2 + n * (2 + long));
    let n = first_not(count, |n| Some(list.u16(entry(n))? < right))?;
    if n == count || list.u16(entry(n))? != right {
        return None;
    }
    advanced(list, entry(n) + 2, advance)
}

/// A PairPos format 2 subtable: first and second glyphs each classified, a value per class
/// pair. It covers every pair whose first glyph it covers: unnamed glyphs are class 0, which
/// has values too.
fn by_class(list: Bytes, left: u16, right: u16) -> Option<(f32, Option<usize>)> {
    let (_, (long, advance)) = (covered(list, list.u16(2)? as usize, left)?, written(list)?);
    let (first, second) = (class(list, list.u16(8)? as usize, left), class(list, list.u16(10)? as usize, right));
    let (firsts, seconds) = (list.u16(12)? as usize, list.u16(14)? as usize);
    if first >= firsts || second >= seconds {
        return None;
    }
    advanced(list, 16 + (first * seconds + second) * long, advance)
}

/// A glyph's coverage index in a `GPOS` subtable's coverage at `at`. `None` if not covered.
/// Format 1 lists glyphs in order; format 2 ranges: first, last, and first's index.
pub(crate) fn covered(list: Bytes, at: usize, glyph: u16) -> Option<usize> {
    let (kind, count) = (list.u16(at).filter(|_| at != 0)?, list.u16(at + 2)? as usize);
    if kind == 1 {
        let n = first_not(count, |n| Some(list.u16(at + 4 + n * 2)? < glyph))?;
        return (n < count && list.u16(at + 4 + n * 2)? == glyph).then_some(n);
    }
    let stretch = |n: usize| at + 4 + n * 6;
    let n = first_not(count, |n| Some(list.u16(stretch(n) + 2)? < glyph))?;
    let (first, place) = (list.u16(stretch(n))?, list.u16(stretch(n) + 4)? as usize);
    (kind == 2 && n < count && first <= glyph).then(|| place + (glyph - first) as usize)
}

/// A glyph's class in a class definition at `at`. Format 1 gives a class per glyph from a first
/// one on; format 2 ranges of one class: first, last, and class.
pub(crate) fn class(list: Bytes, at: usize, glyph: u16) -> usize {
    let stretch = |n: usize| at + 4 + n * 6;
    let found = match (list.u16(at).filter(|_| at != 0), list.u16(at + 2), list.u16(at + 4)) {
        (Some(1), Some(first), Some(count)) => glyph.checked_sub(first).filter(|n| *n < count).and_then(|n| list.u16(at + 6 + n as usize * 2)),
        (Some(2), Some(count), _) => first_not(count as usize, |n| Some(list.u16(stretch(n) + 2)? < glyph))
            .filter(|n| *n < count as usize && list.u16(stretch(*n)).is_some_and(|first| first <= glyph))
            .and_then(|n| list.u16(stretch(n) + 4)),
        _ => None,
    };
    found.unwrap_or(0) as usize
}

/// A `GSUB` or `GPOS` lookup's offset. The table gives the LookupList's offset, and the list
/// each lookup's.
pub(crate) fn lookup_at(table: Bytes, lookup: u16) -> Option<usize> {
    let listed = table.u16(8)? as usize;
    Some(listed + table.u16(listed + 2 + lookup as usize * 2).filter(|_| Some(lookup) < table.u16(listed))? as usize)
}

/// The subtables of the lookup at `at`: each one's type and offset in the table. A lookup gives
/// its type, subtable count, and each offset. An extension subtable (`far`) only points to
/// another, beyond 16-bit offsets, and gives that one's type.
pub(crate) fn lists_at(table: Bytes, at: usize, far: u16) -> impl Iterator<Item = (u16, usize)> {
    let (kind, count) = (table.u16(at).unwrap_or(0), table.u16(at + 4).unwrap_or(0) as usize);
    (0..count).filter_map(move |n| {
        let list = at + table.u16(at + 6 + n * 2)? as usize;
        match (kind == far, table.u16(list)?) {
            (true, 1) => Some((table.u16(list + 2)?, list.checked_add(table.u32(list + 4)? as usize)?)),
            (true, _) => None,
            (false, _) => Some((kind, list)),
        }
    })
}

/// One glyph of a multi-character sequence (a multi-part emoji), as set: see `FontFile::set`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Set {
    pub glyph: u16,
    /// Its position from the pen at the start of all of them: right and up, in font units.
    pub at: [f32; 2],
}

impl FontFile {
    /// The glyphs a multi-character sequence becomes, and the pen's total advance (in font
    /// units): shaped as script-less text (see `shaping.rs`) by a colour font's lookups. A
    /// presentation selector for the previous character is dropped: every glyph here is the
    /// emoji. `None` if the font lacks one of the characters.
    pub(crate) fn set(&self, letters: &str, glyphs: &mut Vec<Set>) -> Option<f32> {
        glyphs.clear();
        let unseen = |letter: char| matches!(letter, '\u{200c}' | '\u{200d}' | '\u{e0000}'..='\u{e007f}' | '\u{fe00}'..='\u{fe0f}');
        letters.chars().all(|letter| unseen(letter) || self.glyph(letter) != 0).then_some(())?;
        // (On macOS the system's emoji font joins sequences in `morx`: CoreText shapes them.)
        #[cfg(target_vendor = "apple")]
        if let Some(pen) = crate::core_text::sequence(self, letters, glyphs) {
            return Some(pen);
        }
        let mut laid = Vec::new();
        let pen = self.shape(&self.emoji, letters, &mut laid);
        let mut at = 0;
        for one in laid {
            glyphs.push(Set { glyph: one.glyph, at: [(at + one.offset[0]) as f32, one.offset[1] as f32] });
            at += one.advance;
        }
        Some(pen as f32)
    }

    /// The font's unique id among the fonts the program read.
    pub(crate) fn serial(&self) -> u32 {
        self.serial
    }

    /// Glyph count.
    #[cfg(test)]
    pub(crate) fn glyph_count(&self) -> u16 {
        self.glyphs
    }

    /// One of the shaping tables: `GSUB`, `GPOS` or `GDEF` (empty if absent).
    pub(crate) fn table(&self, name: &[u8; 4]) -> Bytes<'_> {
        let at = match name {
            b"GSUB" => 0,
            b"GPOS" => 1,
            _ => 2,
        };
        Bytes(&self.file).within(&self.layout[at])
    }

    /// Two glyphs' kerning by the older `kern` table alone, in font units.
    pub(crate) fn kern_pair(&self, left: u16, right: u16) -> f32 {
        let (all, mut sum, mut last_lookup) = (Bytes(&self.file), 0.0, None);
        for pairs in self.kerning.lists.iter().filter(|pairs| matches!(pairs.kind, Kind::Each(_))) {
            let Some(by) = (last_lookup != Some(pairs.lookup)).then(|| pairs.find(all, left, right, self.varied.as_deref())).flatten() else { continue };
            (sum, last_lookup) = (if pairs.instead { by } else { sum + by }, Some(pairs.lookup));
        }
        sum
    }
}

/// More bitmap sizes per font than this aren't examined, nor more layers per glyph: no font has
/// as many.
const SIZES: usize = 64;
const LAYERS: usize = 1024;

/// Where a font has colour glyphs.
#[derive(Default)]
struct Pictures {
    /// `COLR` and `CPAL`: a glyph's layered outlines and their colours; or (COLR v1) its
    /// paints, which `colr.rs` draws. And `SVG `: glyphs as SVG documents, also drawn there.
    layered: Range<usize>,
    colours: Range<usize>,
    written: Range<usize>,
    /// Bitmap glyphs, per size: the location table and the data table, `CBLC` and `CBDT`, or
    /// `sbix` for both.
    sizes: Vec<Size>,
    index: Range<usize>,
    held: Range<usize>,
}

/// One bitmap size of a font.
struct Size {
    /// The pixel size the bitmaps were made for.
    pixels: f32,
    /// Its offset in its table; and in `CBLC`, its index subtable count.
    at: usize,
    lists: Option<usize>,
}

impl Pictures {
    /// Finds what colour glyph tables there are.
    fn read(all: Bytes, [layered, colours, index, held, both, written]: [Option<Range<usize>>; 6]) -> Pictures {
        // Layers are useless without colours, as is a bitmap location table without its data.
        // And the layers read are COLR v0's base glyph records, or v1's paints.
        let layers = layered.zip(colours).filter(|(layered, _)| Pictures::older(all.within(layered)) || Pictures::newer(all.within(layered)));
        let (layered, colours) = layers.unwrap_or_default();
        let (index, held, sizes) = match (index.zip(held), both) {
            (Some((index, held)), _) => (index.clone(), held, Size::listed(all.within(&index))),
            (None, Some(both)) => (both.clone(), both.clone(), Size::graphics(all.within(&both))),
            (None, None) => Default::default(),
        };
        Pictures { layered, colours, written: written.unwrap_or_default(), sizes, index, held }
    }

    fn any(&self) -> bool {
        !self.layered.is_empty() || !self.sizes.is_empty() || !self.written.is_empty()
    }

    /// Whether `COLR` has v0 layered glyphs: its base glyph count, in bytes 2 to 4.
    fn older(layered: Bytes) -> bool {
        layered.u16(2).is_some_and(|count| count != 0)
    }

    /// Whether `COLR` has v1 paint glyphs: from v1, its BaseGlyphList offset in bytes 14 to 18.
    fn newer(layered: Bytes) -> bool {
        layered.u16(0).is_some_and(|version| version >= 1) && layered.u32(14).is_some_and(|list| list != 0)
    }
}

impl Size {
    /// The sizes of `CBLC`: their count in bytes 4 to 8, and 48 bytes each: first its index
    /// subtable array offset and (from byte 8) count, and near the end its ppem and bit depth.
    /// Only 32-bit colour bitmaps are read.
    fn listed(index: Bytes) -> Vec<Size> {
        let size = |n: usize| {
            let at = 8 + n * 48;
            let size = Size { pixels: index.u8(at + 45)? as f32, at: index.u32(at)? as usize, lists: Some(index.u32(at + 8)? as usize) };
            (index.u8(at + 46) == Some(32)).then_some(size)
        };
        (0..(index.u32(4).unwrap_or(0) as usize).min(SIZES)).filter_map(size).collect()
    }

    /// The sizes of `sbix`: count in bytes 4 to 8, then each one's offset. A strike starts with
    /// its ppem.
    fn graphics(both: Bytes) -> Vec<Size> {
        let size = |n: usize| {
            let at = both.u32(8 + n * 4)? as usize;
            Some(Size { pixels: both.u16(at)? as f32, at, lists: None })
        };
        (0..(both.u32(4).unwrap_or(0) as usize).min(SIZES)).filter_map(size).collect()
    }
}

/// A glyph's bitmap as a font has it: PNG bytes, the pixel size it was made for, and its left
/// and top edges relative to the pen on the baseline, in its own pixels (right and up).
pub(crate) struct Bitmap<'a> {
    pub png: &'a [u8],
    pub pixels: f32,
    pub left: f32,
    pub top: f32,
}

impl FontFile {
    /// Whether the font has colour glyphs in the older formats, drawn without `colr.rs`: COLR
    /// v0 layers, or bitmaps.
    pub(crate) fn pictured(&self) -> bool {
        Pictures::older(Bytes(&self.file).within(&self.pictures.layered)) || !self.pictures.sizes.is_empty()
    }

    /// Whether the font has colour glyphs in the newer formats, which `colr.rs` draws: COLR v1
    /// paints, or SVG.
    pub(crate) fn pictured_newer(&self) -> bool {
        Pictures::newer(Bytes(&self.file).within(&self.pictures.layered)) || !self.pictures.written.is_empty()
    }

    /// The tables `colr.rs` draws with: `COLR`, `CPAL` and `SVG ` (each empty if absent or
    /// unusable).
    pub(crate) fn newer(&self) -> [Bytes<'_>; 3] {
        let all = Bytes(&self.file);
        [all.within(&self.pictures.layered), all.within(&self.pictures.colours), all.within(&self.pictures.written)]
    }

    /// A glyph's outline transformed by `table` (a font point `x`, `y` maps to `a x + c y + e`,
    /// `b x + d y + f`, the six values in that order), and its bounds: left, top, right,
    /// bottom. For glyphs `colr.rs` draws transformed.
    pub(crate) fn outline_through(&self, glyph: u16, table: [f32; 6]) -> (Path, [f32; 4]) {
        let sketch = self.sketch(glyph, Placing(table));
        (sketch.path, sketch.reach)
    }

    /// Pixels per font unit at this pixel size.
    pub(crate) fn unit(&self, pixels: f32) -> f32 {
        pixels / self.units
    }

    /// A glyph's COLR v0 layers, bottom up: each a glyph, whose outline it is, and its fill
    /// colour (`None`: the text colour). `None` for a glyph without layers. `COLR` lists base
    /// glyphs in order, six bytes each: glyph, first layer index, layer count. A layer is four
    /// bytes: glyph and palette index.
    pub(crate) fn layers(&self, glyph: u16) -> Option<impl Iterator<Item = (u16, Option<Color>)> + '_> {
        let (table, colours) = (Bytes(&self.file).within(&self.pictures.layered), Bytes(&self.file).within(&self.pictures.colours));
        let (count, glyphs, layers) = (table.u16(2)? as usize, table.u32(4)? as usize, table.u32(8)? as usize);
        let entry = glyphs + first_not(count, |n| Some(table.u16(glyphs + n * 6)? < glyph))? * 6;
        (entry < glyphs + count * 6 && table.u16(entry) == Some(glyph)).then_some(())?;
        let (first, many) = (table.u16(entry + 2)? as usize, (table.u16(entry + 4)? as usize).min(LAYERS));
        // `CPAL` has four-byte colours (blue, green, red, alpha), and gives where they start
        // and palette 0's first index. Index 0xFFFF means the text colour.
        let (palette, most) = (colours.u32(8)? as usize + colours.u16(12)? as usize * 4, colours.u16(2)?);
        let colour = move |number: u16| colours.u32(palette + number as usize * 4).filter(|_| number < most).map(|colour| Color(colour.swap_bytes()));
        Some((first..first + many).map_while(move |n| Some((table.u16(layers + n * 4)?, colour(table.u16(layers + n * 4 + 2)?)))))
    }

    /// A glyph's bitmap for text `pixels` high: from the smallest size large enough, else the
    /// largest.
    pub(crate) fn bitmap(&self, glyph: u16, pixels: f32) -> Option<Bitmap<'_>> {
        let sizes = || self.pictures.sizes.iter();
        let enough = sizes().filter(|size| size.pixels >= pixels).min_by(|a, b| a.pixels.total_cmp(&b.pixels));
        let size = enough.or(sizes().max_by(|a, b| a.pixels.total_cmp(&b.pixels)))?;
        let (index, held) = (Bytes(&self.file).within(&self.pictures.index), Bytes(&self.file).within(&self.pictures.held));
        match size.lists {
            Some(lists) => sized(index, held, size, lists, glyph),
            None => graphic(held, size, glyph),
        }
    }

    /// A glyph's coverage mask, at `unit` pixels per font unit with the pen `pen` pixels right
    /// of the start and above the baseline: the mask's left and top pixel (right and down), its
    /// width, and a byte per pixel. An empty mask for a glyph with nothing to draw or too
    /// large.
    pub(crate) fn cover(&self, glyph: u16, unit: f32, pen: [f32; 2]) -> ([i32; 2], usize, Vec<u8>) {
        let sketch = self.sketch(glyph, Placing([unit, 0.0, 0.0, -unit, pen[0], -pen[1]]));
        let (left, top) = (sketch.reach[0].floor(), sketch.reach[1].floor());
        let (wide, tall) = (sketch.reach[2].ceil() - left, sketch.reach[3].ceil() - top);
        if !(wide * tall > 0.0 && wide * tall <= LARGEST * 16.0) {
            return ([0; 2], 0, Vec::new());
        }
        ([left as i32, top as i32], wide as usize, coverage(&sketch.path, 1.0, 0.0, (left as i32, top as i32), wide as usize, tall as usize))
    }
}

/// A glyph's bitmap from `CBLC` and `CBDT` at one size. The size has index subtables, each for
/// a glyph range: first and last glyph, and offset. A subtable starts with its index format,
/// image format, and image data offset in `CBDT`; then gives each glyph's image start and end,
/// in one of five formats.
fn sized<'a>(index: Bytes<'a>, held: Bytes<'a>, size: &Size, lists: usize, glyph: u16) -> Option<Bitmap<'a>> {
    let has = |entry: &usize| index.u16(*entry).is_some_and(|first| first <= glyph) && index.u16(entry + 2).is_some_and(|last| glyph <= last);
    let entry = (0..lists.min(MOST)).map(|n| size.at + n * 8).find(has)?;
    let (nth, list) = ((glyph - index.u16(entry)?) as usize, size.at + index.u32(entry + 4)? as usize);
    let (way, kind, from) = (index.u16(list)?, index.u16(list + 2)?, index.u32(list + 4)? as usize);
    let (wide, short) = (|at: usize| index.u32(at).map(|number| number as usize), |at: usize| index.u16(at).map(usize::from));
    let (start, end) = match way {
        // An offset per glyph of the range, 4-byte or 2-byte;
        1 => (wide(list + 8 + nth * 4)?, wide(list + 12 + nth * 4)?),
        3 => (short(list + 8 + nth * 2)?, short(list + 10 + nth * 2)?),
        // fixed-size images;
        2 => (nth.checked_mul(wide(list + 8)?)?, (nth + 1).checked_mul(wide(list + 8)?)?),
        // or a sparse glyph list in order: each with its offset, or all the same size.
        4 => one_of(index, list + 12, 4, wide(list + 8)?, glyph).and_then(|n| short(list + 14 + n * 4).zip(short(list + 18 + n * 4)))?,
        5 => one_of(index, list + 24, 2, wide(list + 20)?, glyph).and_then(|n| Some((n.checked_mul(wide(list + 8)?)?, (n + 1).checked_mul(wide(list + 8)?)?)))?,
        _ => return None,
    };
    let lying = held.part(from.checked_add(start)?, end.checked_sub(start)?)?;
    // An image is a PNG after its length, preceded by its metrics, 5 or 8 bytes: height and
    // width, then left bearing and top bearing. Or the subtable has the metrics, shared.
    let (measures, file) = match kind {
        17 => (lying, 5),
        18 => (lying, 8),
        19 => (index.part(list + 12, 8)?, 0),
        _ => return None,
    };
    let png = lying.part(file + 4, lying.u32(file)? as usize)?.0;
    Some(Bitmap { png, pixels: size.pixels, left: measures.u8(2)? as i8 as f32, top: measures.u8(3)? as i8 as f32 })
}

/// A glyph's index among the glyph ids a `CBLC` subtable lists (sorted, from `at`, `long` bytes
/// each).
fn one_of(index: Bytes, at: usize, long: usize, count: usize, glyph: u16) -> Option<usize> {
    let n = first_not(count, |n| Some(index.u16(at + n * long)? < glyph))?;
    (n < count && index.u16(at + n * long) == Some(glyph)).then_some(n)
}

/// A glyph's bitmap from `sbix` at one size. After four bytes a strike gives each glyph's data
/// offset, ending at the next one's. Its data is the left and bottom bearing, a four-byte type
/// tag, then the image. (Or the tag says it duplicates another glyph's, which is named:
/// followed a few times.)
fn graphic<'a>(both: Bytes<'a>, size: &Size, glyph: u16) -> Option<Bitmap<'a>> {
    let mut glyph = glyph;
    for _ in 0..DEEPEST {
        let (start, end) = (both.u32(size.at + 4 + glyph as usize * 4)? as usize, both.u32(size.at + 8 + glyph as usize * 4)? as usize);
        let lying = both.part(size.at + start, end.checked_sub(start)?)?;
        match (lying.u32(4)?.to_be_bytes(), lying.0.get(8..)?) {
            (kind, other) if &kind == b"dupe" => glyph = Bytes(other).u16(0)?,
            // A PNG gives its height in bytes 20 to 24.
            (kind, png) if &kind == b"png " => {
                let high = Bytes(png).u32(20)? as f32;
                return Some(Bitmap { png, pixels: size.pixels, left: lying.i16(0)? as f32, top: lying.i16(2)? as f32 + high });
            }
            _ => return None,
        }
    }
    None
}

/// A glyph point transform: a 2×2 matrix, then a translation.
#[derive(Clone, Copy)]
pub(crate) struct Placing(pub [f32; 6]);

impl Placing {
    /// For a standalone glyph at `unit` per font unit. Fonts count up from the baseline and
    /// drawings down, so the glyph is flipped, with the baseline `line` down.
    pub fn sized(unit: f32, line: f32) -> Placing {
        Placing([unit, 0.0, 0.0, -unit, 0.0, line])
    }
    /// Where a glyph point goes.
    pub fn of(self, point: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        pt(a * point.x + c * point.y + e, b * point.x + d * point.y + f)
    }
    /// This transform applied to points `first` already transformed.
    pub fn after(self, first: Placing) -> Placing {
        let ([a, b, c, d, e, f], [p, q, r, s, t, u]) = (self.0, first.0);
        Placing([a * p + c * q, b * p + d * q, a * r + c * s, b * r + d * s, a * t + c * u + e, b * t + d * u + f])
    }
}

/// A glyph's outline while built: the path, its bounds (left, top, right, bottom), and how many
/// more points or commands it may take.
pub(crate) struct Sketch {
    pub path: Path,
    pub reach: [f32; 4],
    room: usize,
}

impl Sketch {
    /// Empty, with bounds the first point sets.
    pub fn new() -> Sketch {
        Sketch { path: Path::new(), reach: [f32::MAX, f32::MAX, f32::MIN, f32::MIN], room: MOST }
    }
    /// Deducts points from the glyph's budget. `None` once it exceeds any real glyph.
    pub fn spend(&mut self, points: usize) -> Option<()> {
        self.room = self.room.checked_sub(points)?;
        Some(())
    }
    /// Grows the bounds to hold a point.
    fn hold(&mut self, point: Point) {
        let [left, top, right, bottom] = self.reach;
        self.reach = [left.min(point.x), top.min(point.y), right.max(point.x), bottom.max(point.y)];
    }
    pub fn move_to(&mut self, to: Point) {
        self.hold(to);
        self.path.move_to(to.x, to.y);
    }
    pub fn line_to(&mut self, to: Point) {
        self.hold(to);
        self.path.line_to(to.x, to.y);
    }
    /// A quadratic curve. (It stays within its points' bounds.)
    fn quad_to(&mut self, by: Point, to: Point) {
        [by, to].into_iter().for_each(|point| self.hold(point));
        self.path.quad_to(by.x, by.y, to.x, to.y);
    }
    /// A cubic curve.
    pub fn cubic_to(&mut self, a: Point, b: Point, to: Point) {
        [a, b, to].into_iter().for_each(|point| self.hold(point));
        self.path.cubic_to(a.x, a.y, b.x, b.y, to.x, to.y);
    }
}

/// A glyph still to draw: which, its transform, and its nesting depth in the composite. A
/// component may be positioned by matching points instead of an offset: then `laid` names a
/// point of the earlier components (counted among all points so far) and one of its own, and
/// the second is moved onto the first.
struct Part {
    glyph: u16,
    place: Placing,
    depth: u8,
    laid: Option<[usize; 2]>,
}

/// A TrueType font's outline tables: `glyf`, and `loca`, giving each glyph's offset in it, as
/// 32-bit values (`long`) or 16-bit halves.
struct Points {
    glyf: Range<usize>,
    loca: Range<usize>,
    long: bool,
}

impl Points {
    /// A glyph's bytes: they end where the next glyph's start.
    fn bytes<'a>(&self, all: Bytes<'a>, glyph: u16) -> Option<Bytes<'a>> {
        let (glyf, loca, glyph) = (all.within(&self.glyf), all.within(&self.loca), glyph as usize);
        let start = |glyph: usize| if self.long { loca.u32(glyph * 4).map(|at| at as usize) } else { loca.u16(glyph * 2).map(|at| at as usize * 2) };
        glyf.0.get(start(glyph)?..start(glyph + 1)?).map(Bytes)
    }

    /// Draws a glyph into a sketch. A glyph may be composed of others, recursively: pending
    /// ones wait on a stack, so nothing here recurses.
    fn draw(&self, font: &FontFile, glyph: u16, place: Placing, sketch: &mut Sketch) -> Option<()> {
        let all = Bytes(&font.file);
        // The glyph is placed at its `hmtx` left side bearing, wherever its own points put it
        // (its xMin, in bytes 2 to 4). A composite may flag a component to use instead.
        let ruling = std::iter::successors(Some(glyph), |glyph| named(self.bytes(all, *glyph)?).map(|(part, _)| part)).take(DEEPEST as usize).last()?;
        let shift = font.edge(ruling).zip(self.bytes(all, ruling)?.i16(2)).map_or(0.0, |(edge, left)| edge - left as f32);
        let shift = shift - font.varied.as_ref().map_or(0.0, |varied| varied.left(all, ruling, self.bytes(all, ruling).unwrap_or(Bytes(&[]))));
        let mut waiting = vec![Part { glyph, place: place.after(Placing([1.0, 0.0, 0.0, 1.0, shift, 0.0])), depth: 0, laid: None }];
        // All points are placed first, contours drawn from them last: a component positioned by
        // points can only move there once its points are all known, which is when the stack is
        // back to its depth when the component was popped.
        let (mut dots, mut ends, mut moving) = (Vec::new(), Vec::new(), Vec::new());
        while let Some(Part { glyph, place, depth, laid }) = waiting.pop() {
            let data = self.bytes(all, glyph)?;
            moving.extend(laid.map(|[onto, own]| (waiting.len(), [onto, dots.len(), own])));
            // It starts with its contour count: negative for a composite. (A glyph with nothing
            // to draw, such as a space, has no bytes.)
            match data.i16(0) {
                None | Some(0) => {}
                Some(outlines @ 1..) => simple((font, glyph), data, outlines as usize, place, (&mut dots, &mut ends), sketch)?,
                Some(_) if depth == DEEPEST => return None,
                Some(_) => parts((font, glyph), data, place, depth + 1, (dots.len(), &mut waiting), sketch)?,
            }
            while let Some((_, points)) = moving.pop_if(|(short, _)| waiting.len() <= *short) {
                lay(&mut dots, points)?;
            }
        }
        let mut start = 0;
        for end in ends {
            outline(dots.get(start..end)?, sketch);
            start = end;
        }
        Some(())
    }
}

/// One component of a composite glyph, at `at`: its flags, the component (glyph; offset, scale
/// and rotation; the two points it is positioned by, if so), and the next one's offset. The
/// flags say how the values are encoded. (A variable font may shift it further, `by`.)
fn part(data: Bytes, at: usize, by: Point) -> Option<(u16, Part, usize)> {
    let (flags, glyph, wide) = (data.u16(at)?, data.u16(at + 2)?, data.u16(at)? & 1 != 0);
    // Two values, 16-bit or 8-bit, give the offset. Or, by a flag, two point numbers: one of
    // the earlier components', and one of its own, to be moved onto it.
    let (first, second) = if wide { (data.u16(at + 4)?, data.u16(at + 6)?) } else { (data.u8(at + 4)? as u16, data.u8(at + 5)? as u16) };
    let far = |number: u16| if wide { number as i16 as f32 } else { number as u8 as i8 as f32 };
    let (moved, laid) = if flags & 2 != 0 { (pt(far(first) + by.x, far(second) + by.y), None) } else { (pt(0.0, 0.0), Some([first as usize, second as usize])) };
    // Its transform: none, one scale, one per axis, or a 2×2 (in F2Dot14, 16384ths).
    let turned = at + if wide { 8 } else { 6 };
    let number = |n: usize| data.i16(turned + n * 2).map(|number| number as f32 / 16384.0);
    let (by, long) = match () {
        _ if flags & 0x08 != 0 => ([number(0)?, 0.0, 0.0, number(0)?], 2),
        _ if flags & 0x40 != 0 => ([number(0)?, 0.0, 0.0, number(1)?], 4),
        _ if flags & 0x80 != 0 => ([number(0)?, number(1)?, number(2)?, number(3)?], 8),
        _ => ([1.0, 0.0, 0.0, 1.0], 0),
    };
    // Some fonts scale the offset with the component, and flag it (SCALED_COMPONENT_OFFSET).
    let stretch = if flags & 0x800 != 0 { pt(by[0].hypot(by[2]), by[1].hypot(by[3])) } else { pt(1.0, 1.0) };
    Some((flags, Part { glyph, place: Placing([by[0], by[1], by[2], by[3], moved.x * stretch.x, moved.y * stretch.y]), depth: 0, laid }, turned + long))
}

/// Pushes a composite's components onto the stack. (`from` is the point count so far: a
/// component positioned by points counts earlier points from there.) In a variable font
/// instance a component may move: `gvar` has a phantom point per component giving how far, in
/// whole units, as FreeType does.
fn parts((font, glyph): (&FontFile, u16), data: Bytes, place: Placing, depth: u8, (from, waiting): (usize, &mut Vec<Part>), sketch: &mut Sketch) -> Option<()> {
    let (first, mut at) = (waiting.len(), 10);
    let moved = match &font.varied {
        Some(varied) => varied.moved(Bytes(&font.file), glyph, std::iter::repeat_n(pt(0.0, 0.0), crate::variable_fonts::parts(data)?), std::iter::empty())?,
        None => Vec::new(),
    };
    loop {
        sketch.spend(1)?;
        let by = moved.get(waiting.len() - first).map_or(pt(0.0, 0.0), |by| pt(by.x.round(), by.y.round()));
        let (flags, part, next) = part(data, at, by)?;
        waiting.push(Part { place: place.after(part.place), depth, laid: part.laid.map(|[onto, own]| [from + onto, own]), ..part });
        at = next;
        // A flag says whether another component follows. (The last pushed is popped first:
        // reversed, they draw in written order.)
        if flags & 0x20 == 0 {
            waiting[first..].reverse();
            return Some(());
        }
    }
}

/// The component of a composite flagged USE_MY_METRICS, whose pen position the whole glyph
/// takes, with its transform. `None` for a simple glyph, or with none flagged.
fn named(data: Bytes) -> Option<(u16, Placing)> {
    let (mut at, mut named) = (10, None);
    while data.i16(0)? < 0 {
        let (flags, part, next) = part(data, at, pt(0.0, 0.0))?;
        (at, named) = (next, if flags & 0x200 != 0 { Some((part.glyph, part.place)) } else { named });
        if flags & 0x20 == 0 {
            break;
        }
    }
    named
}

/// Moves a component to where its matching points put it. Its points start at `from`; its point
/// `own` is moved onto point `onto`, which must be earlier.
fn lay(dots: &mut [Dot], [onto, from, own]: [usize; 3]) -> Option<()> {
    let (before, part) = dots.split_at_mut_checked(from)?;
    let (onto, own) = (before.get(onto)?.at, part.get(own)?.at);
    part.iter_mut().for_each(|dot| dot.at = pt(dot.at.x + onto.x - own.x, dot.at.y + onto.y - own.y));
    Some(())
}

/// A `glyf` point: its flags as stored, and its position.
#[derive(Clone, Copy)]
pub(crate) struct Dot {
    pub flags: u8,
    pub at: Point,
}

/// A simple `glyf` glyph: its points are appended to `dots`, transformed, and each contour's
/// end to `ends`. It lists each contour's last point, then (after its instructions, which are
/// skipped) a flag byte per point, then all x deltas, then all y deltas. (A variable font
/// instance moves the points first.)
fn simple((font, glyph): (&FontFile, u16), data: Bytes, outlines: usize, place: Placing, (dots, ends): (&mut Vec<Dot>, &mut Vec<usize>), sketch: &mut Sketch) -> Option<()> {
    let (last, first) = (data.part(10, outlines * 2)?, dots.len());
    let count = last.u16(outlines * 2 - 2)? as usize + 1;
    sketch.spend(count)?;
    let mut at = 12 + outlines * 2 + data.u16(10 + outlines * 2)? as usize;
    while dots.len() < first + count {
        // A flag may repeat for so many more points.
        let (flags, again) = (data.u8(at)?, data.u8(at + 1));
        let times = if flags & 8 != 0 { again? as usize + 1 } else { 1 };
        dots.extend(std::iter::repeat_n(Dot { flags, at: pt(0.0, 0.0) }, times));
        at += if flags & 8 != 0 { 2 } else { 1 };
    }
    if dots.len() > first + count {
        return None;
    }
    let (mut x, mut y) = (0.0, 0.0);
    for dot in &mut dots[first..] {
        x += step(data, &mut at, dot.flags >> 1)?;
        dot.at.x = x;
    }
    for dot in &mut dots[first..] {
        y += step(data, &mut at, dot.flags >> 2)?;
        dot.at.y = y;
    }
    let lasts = (0..outlines).map_while(|n| last.u16(n * 2)).map(usize::from);
    if let Some(varied) = &font.varied {
        let moved = varied.moved(Bytes(&font.file), glyph, dots[first..].iter().map(|dot| dot.at), lasts.clone())?;
        dots[first..].iter_mut().zip(moved).for_each(|(dot, by)| dot.at = pt(dot.at.x + by.x, dot.at.y + by.y));
    }
    dots[first..].iter_mut().for_each(|dot| dot.at = place.of(dot.at));
    ends.extend(lasts.map(|last| first + last + 1));
    Some(())
}

/// One coordinate delta of a `glyf` point, read at `at`, which advances. Flag bit 0 (short)
/// means one byte, with bit 3 its sign; without it, bit 3 means zero (same), else two bytes.
fn step(data: Bytes, at: &mut usize, flags: u8) -> Option<f32> {
    let (step, long) = match (flags & 1 != 0, flags & 8 != 0) {
        (true, true) => (data.u8(*at)? as f32, 1),
        (true, false) => (-(data.u8(*at)? as f32), 1),
        (false, true) => (0.0, 0),
        (false, false) => (data.i16(*at)? as f32, 2),
    };
    *at += long;
    Some(step)
}

/// One closed contour of a `glyf` glyph. Its points are on-curve or control points. Between two
/// consecutive control points lies an implied on-curve point, halfway.
pub(crate) fn outline(dots: &[Dot], sketch: &mut Sketch) {
    let (Some(first), Some(last)) = (dots.first(), dots.last()) else { return };
    let (on, half) = (|dot: &Dot| dot.flags & 1 != 0, |a: Point, b: Point| pt((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
    // It starts at an on-curve point: the first, else the last, else halfway between.
    let start = match (on(first), on(last)) {
        (true, _) => first.at,
        (false, true) => last.at,
        (false, false) => half(first.at, last.at),
    };
    sketch.move_to(start);
    let mut pull = None;
    for dot in &dots[on(first) as usize..] {
        match (pull, on(dot)) {
            (Some(by), true) => sketch.quad_to(by, dot.at),
            (Some(by), false) => sketch.quad_to(by, half(by, dot.at)),
            (None, true) => sketch.line_to(dot.at),
            (None, false) => {}
        }
        pull = (!on(dot)).then_some(dot.at);
    }
    if let Some(by) = pull {
        sketch.quad_to(by, start);
    }
    sketch.path.close();
}

/// One glyph at one size, as cached once drawn.
struct Letter {
    /// Its glyph id and its character's script: what kerning depends on (unshaped).
    glyph: u16,
    #[cfg(not(feature = "shaping"))]
    script: u8,
    /// Its advance, in whole pixels.
    advance: f32,
    /// Its coverage mask: the corner's offset from the pen on the baseline, its width, and
    /// per-pixel coverage. (Empty, with nothing to draw.)
    left: f32,
    top: f32,
    width: usize,
    cover: Vec<u8>,
    /// Too large to cache: it has no mask, and is rasterized from its outline each time.
    large: bool,
}

impl Letter {
    /// Rasterizes a character, as `glyph` of the file, at a pixel size. Its outline is filled,
    /// white on transparent, into a mask just large enough; each pixel's alpha is its coverage.
    /// A `hinted` one is hinted first: with the feature `hinting`, as `own` gives it, by the
    /// font's instructions (`Some(None)`: left unhinted by them); else by `hinting.rs`.
    fn drawn(file: &FontFile, glyph: u16, letter: char, pixels: f32, hinted: bool, #[cfg(feature = "hinting")] own: Own) -> Letter {
        let unit = pixels / file.units;
        #[cfg(feature = "hinting")]
        let (instructed, mut sketch) = match own {
            Some(Some((path, reach))) => (true, Sketch { path, reach, room: 0 }),
            own => (own.is_some(), file.sketch(glyph, Placing::sized(unit, 0.0))),
        };
        #[cfg(not(feature = "hinting"))]
        let (instructed, mut sketch) = (false, file.sketch(glyph, Placing::sized(unit, 0.0)));
        if let Some((path, reach)) = (hinted && !instructed && crate::hinting::fits(letter)).then(|| crate::hinting::set(&sketch.path, file, unit)).flatten() {
            (sketch.path, sketch.reach) = (path, reach);
        }
        let (left, top) = (sketch.reach[0].floor(), sketch.reach[1].floor());
        let (wide, tall) = (sketch.reach[2].ceil() - left, sketch.reach[3].ceil() - top);
        let mut letter = Letter {
            glyph,
            #[cfg(not(feature = "shaping"))]
            script: script(letter),
            advance: (file.width(glyph) * unit).round(),
            left,
            top,
            width: 0,
            cover: Vec::new(),
            large: wide * tall > LARGEST,
        };
        if letter.large || wide * tall == 0.0 {
            return letter;
        }
        letter.width = wide as usize;
        letter.cover = coverage(&sketch.path, 1.0, 0.0, (left as i32, top as i32), letter.width, tall as usize);
        letter
    }

    /// Draws the glyph with its pen at `x` on baseline `line`, in pixels.
    fn lay(&self, canvas: &mut Canvas, file: &FontFile, x: f32, line: f32, size: f32, color: Color) {
        if !(x.abs() < FURTHEST && line.abs() < FURTHEST) {
            return;
        }
        if !self.large {
            return canvas.cover((x + self.left) as i32, (line + self.top) as i32, self.width, &self.cover, color);
        }
        let shape = file.sketch(self.glyph, Placing::sized(size / file.units, 0.0)).path;
        canvas.fill(&shape, pt(x / canvas.scale(), line / canvas.scale()), 1.0, color);
    }
}

/// One file's glyphs at one pixel size, each rasterized when first requested.
struct Drawn {
    file: usize,
    pixels: f32,
    letters: Vec<Letter>,
    /// Each character's glyph index among them, from 1 (0: not drawn yet). Characters are paged
    /// by 256: only pages used exist, and lookup needs no search. (Unshaped.)
    #[cfg(not(feature = "shaping"))]
    pages: Vec<Option<Box<[u32; 256]>>>,
    /// Each shaped glyph's index among them likewise, by glyph id (feature `shaping`).
    #[cfg(feature = "shaping")]
    shapes: Vec<Option<Box<[u32; 256]>>>,
    /// Kerning of glyph pairs met in text, in whole pixels: per pair its two glyph indices (two
    /// bytes each), and the value. Open addressing on a hash of the pair, so lookup barely
    /// searches. `met` counts the pairs; there is always room for a third as many again.
    /// (Unshaped.)
    #[cfg(not(feature = "shaping"))]
    pairs: Vec<(u32, f32)>,
    #[cfg(not(feature = "shaping"))]
    met: usize,
    /// The font's programs run for this size (see `truetype_hinting.rs`).
    #[cfg(feature = "hinting")]
    hints: crate::truetype_hinting::Hints,
}

/// A glyph hinted by its font's instructions: its outline in pixels and its bounds; `Some(None)`
/// where they leave it unhinted; `None` where they don't apply.
#[cfg(feature = "hinting")]
type Own = Option<Option<(Path, [f32; 4])>>;

/// How small glyphs are hinted: not at all, by `hinting.rs`, or by the font's own instructions
/// where it has them (feature `hinting`) and `hinting.rs` elsewhere.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hinting {
    Off,
    Auto,
    Own,
}

impl Drawn {
    /// The glyph index for a character, rasterizing it if new, in the font's form for a
    /// language where `forms` is set. Its memory (page slot, page, glyph) is added to
    /// `cache_bytes`. It is hinted as `hinting` says.
    #[cfg(not(feature = "shaping"))]
    fn letter(&mut self, file: &FontFile, letter: char, hinting: Hinting, forms: Forms, cache_bytes: &mut usize) -> usize {
        let (page, at) = (letter as usize >> 8, letter as usize & 255);
        if self.pages.len() <= page {
            *cache_bytes += (page + 1 - self.pages.len()) * size_of::<Option<Box<[u32; 256]>>>();
            self.pages.resize_with(page + 1, || None);
        }
        if self.pages[page].is_none() {
            *cache_bytes += size_of::<[u32; 256]>();
        }
        let place = &mut self.pages[page].get_or_insert_with(|| Box::new([0; 256]))[at];
        if *place == 0 {
            let glyph = forms.map_or_else(|| file.glyph(letter), |(forms, language)| forms(file, letter, language));
            // (Hinted by the font's own instructions with the feature `hinting`.)
            #[cfg(feature = "hinting")]
            let own = if hinting == Hinting::Own { self.hints.glyph(file, self.pixels, glyph) } else { None };
            #[cfg(feature = "hinting")]
            let drawn = Letter::drawn(file, glyph, letter, self.pixels, hinting != Hinting::Off, own);
            #[cfg(not(feature = "hinting"))]
            let drawn = Letter::drawn(file, glyph, letter, self.pixels, hinting != Hinting::Off);
            *cache_bytes += size_of::<Letter>() + drawn.cover.len();
            self.letters.push(drawn);
            *place = self.letters.len() as u32;
        }
        *place as usize - 1
    }

    /// The index of a glyph shaping gave, rasterizing it if new: hinted as `letter`, the
    /// character it stands for (first), would be. Its memory is added to `cache_bytes`.
    #[cfg(feature = "shaping")]
    fn shaped(&mut self, file: &FontFile, glyph: u16, letter: char, hinting: Hinting, cache_bytes: &mut usize) -> usize {
        let (page, at) = (glyph as usize >> 8, glyph as usize & 255);
        if self.shapes.len() <= page {
            *cache_bytes += (page + 1 - self.shapes.len()) * size_of::<Option<Box<[u32; 256]>>>();
            self.shapes.resize_with(page + 1, || None);
        }
        if self.shapes[page].is_none() {
            *cache_bytes += size_of::<[u32; 256]>();
        }
        let place = &mut self.shapes[page].get_or_insert_with(|| Box::new([0; 256]))[at];
        if *place == 0 {
            // (Hinted by the font's own instructions with the feature `hinting`.)
            #[cfg(feature = "hinting")]
            let own = if hinting == Hinting::Own { self.hints.glyph(file, self.pixels, glyph) } else { None };
            #[cfg(feature = "hinting")]
            let drawn = Letter::drawn(file, glyph, letter, self.pixels, hinting != Hinting::Off, own);
            #[cfg(not(feature = "hinting"))]
            let drawn = Letter::drawn(file, glyph, letter, self.pixels, hinting != Hinting::Off);
            *cache_bytes += size_of::<Letter>() + drawn.cover.len();
            self.letters.push(drawn);
            *place = self.letters.len() as u32;
        }
        *place as usize - 1
    }

    /// The kerning between a glyph and the one before, in whole pixels. Asking the font
    /// searches its subtables; a pair asked before is cached, which is most pairs of any text.
    #[cfg(not(feature = "shaping"))]
    fn closer(&mut self, file: &FontFile, left: usize, right: usize, cache_bytes: &mut usize) -> f32 {
        if file.kerning.lists.is_empty() {
            return 0.0;
        }
        if self.met * 4 >= self.pairs.len() * 3 {
            self.widen(cache_bytes);
        }
        let kerned = |n: usize| (self.letters[n].glyph, self.letters[n].script);
        // Most pairs aren't kerned at all, which needs no rounding.
        let asked = || match file.closer(kerned(left), kerned(right)) {
            0.0 => 0.0,
            closer => (closer * self.pixels / file.units).round(),
        };
        // Glyph indices from 65535 don't fit two bytes.
        if left.max(right) >= 0xffff {
            return asked();
        }
        let pair = (left << 16 | right) as u32;
        let at = place(&self.pairs, pair);
        if self.pairs[at].0 != pair {
            (self.pairs[at], self.met) = ((pair, asked()), self.met + 1);
        }
        self.pairs[at].1
    }

    /// Doubles the pair table and rehashes the pairs met. (The extra memory is added to
    /// `cache_bytes`.)
    #[cfg(not(feature = "shaping"))]
    fn widen(&mut self, cache_bytes: &mut usize) {
        let room = vec![(u32::MAX, 0.0); (self.pairs.len() * 2).max(PAIRS)];
        let met = std::mem::replace(&mut self.pairs, room);
        *cache_bytes += size_of_val(&self.pairs[..]) - size_of_val(&met[..]);
        for (pair, closer) in met.into_iter().filter(|(pair, _)| *pair != u32::MAX) {
            let at = place(&self.pairs, pair);
            self.pairs[at] = (pair, closer);
        }
    }
}

/// A pair's slot in the table, or the free slot it goes in. (The size is a power of two with
/// free slots always; a free one holds glyph 65535 paired with itself, never stored.)
#[cfg(not(feature = "shaping"))]
fn place(pairs: &[(u32, f32)], pair: u32) -> usize {
    let (jumble, last) = (pair.wrapping_mul(0x9e37_79b1), pairs.len() - 1);
    let mut at = (jumble ^ jumble >> 15) as usize & last;
    while pairs[at].0 != pair && pairs[at].0 != u32::MAX {
        at = (at + 1) & last;
    }
    at
}

/// Finds a font's localized form of a character, with the language: where an app asked for such
/// forms ([`AppFonts::language`]).
type Forms = Option<(fn(&FontFile, char, Language) -> u16, Language)>;

/// Text drawn with the app's own fonts: the same glyphs on every system, on any surface.
/// ([`Face::Regular`], [`Face::Bold`], [`Face::Mono`] and [`Face::Italic`] each get a file;
/// missing ones fall back to regular. But monospace, with no file for it and a fallback text
/// system set: that is the fallback's.)
pub struct AppFonts {
    /// The files, and which one each face uses.
    files: Vec<FontFile>,
    faces: [usize; 4],
    /// The fallback for characters the files lack, if any (see `or`).
    other: Option<Box<dyn TextSystem>>,
    /// The glyphs drawn so far, and their approximate memory.
    drawn: Vec<Drawn>,
    cache_bytes: usize,
    /// How small text is hinted (see `unhinted`, `autohinted`).
    hinting: Hinting,
    /// The language whose localized forms are used (see `language`): none until the app asks.
    forms: Forms,
    /// What shaping keeps (feature `shaping`).
    #[cfg(feature = "shaping")]
    shaped: Shaped,
}

/// What shaping keeps for app fonts: each file's plan per script, by file; the runs shaped so
/// far, by file and text, in that order; and roughly how many bytes those take.
#[cfg(feature = "shaping")]
#[derive(Default)]
struct Shaped {
    plans: Vec<(usize, Script, Plan)>,
    runs: Vec<(usize, Box<str>, Box<[Placed]>)>,
    bytes: usize,
}

/// Roughly how many bytes of shaped runs are kept: a window of 10,000 rows of a few labels each.
/// Past it, all are dropped and shaped again as needed.
#[cfg(feature = "shaping")]
const RUNS_LIMIT: usize = 16 << 20;

/// A glyph of a shaped run, as cached in 16 bytes: its id; the character it stands for (its
/// first, by index in the run, and itself, which hinting depends on); and its advance and
/// offset from the pen (right and up), in font units.
#[cfg(feature = "shaping")]
struct Placed {
    glyph: u16,
    from: u16,
    letter: char,
    advance: i16,
    offset: [i16; 2],
}

impl std::fmt::Debug for AppFonts {
    /// The file count, and whether hinting is on.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppFonts").field("files", &self.files.len()).field("hinted", &(self.hinting != Hinting::Off)).finish_non_exhaustive()
    }
}

impl AppFonts {
    /// Text drawn with this file, for every face.
    pub fn new(regular: FontFile) -> Self {
        AppFonts {
            files: vec![regular],
            faces: [0; 4],
            other: None,
            drawn: Vec::new(),
            cache_bytes: 0,
            hinting: Hinting::Own,
            forms: None,
            #[cfg(feature = "shaping")]
            shaped: Shaped::default(),
        }
    }
    /// Unhinted glyphs at every size: for text that scales smoothly, as in a zoomed view.
    /// (Otherwise small text's baseline, x-height and cap-height snap to whole pixels, keeping
    /// it sharp; so glyph height changes in pixel steps as size changes.)
    pub fn unhinted(self) -> Self {
        AppFonts { hinting: Hinting::Off, ..self }
    }
    /// Small text hinted from its outlines (`hinting.rs`, as FreeType's autohinter) even where
    /// a font has hinting instructions of its own. (With the feature `hinting`, those are run
    /// otherwise; without it, this is how every font is hinted.)
    pub fn autohinted(self) -> Self {
        AppFonts { hinting: Hinting::Auto, ..self }
    }
    /// Glyphs in the files' localized forms for a language, where they have some: Serbian б,
    /// Romanian ș with a comma, Japanese forms of CJK characters.
    /// ([`Language::of_the_system()`](Language::of_the_system) for the user's; a window's
    /// language, [`Ui::set_language`](crate::Ui::set_language), overrides it.) Without it, the
    /// default forms. Page text ([`Paper::language`](crate::Paper::language)) uses the same.
    pub fn language(self, language: Language) -> Self {
        AppFonts {
            forms: Some((crate::shaping::local, language)),
            drawn: Vec::new(),
            cache_bytes: 0,
            #[cfg(feature = "shaping")]
            shaped: Shaped::default(),
            ..self
        }
    }
    /// Characters the files lack are drawn by `other`, at the same size and baseline: usually
    /// the system's text, which has a font for every script.
    /// ([`Ui::set_fonts`](crate::Ui::set_fonts) sets this itself.) Without it they show as the
    /// file's notdef box. An emoji goes to the fallback whole, including parts the files have
    /// glyphs for: a keycap's digit, a family's joiners.
    pub fn fallback(self, other: Box<dyn TextSystem>) -> Self {
        AppFonts { other: Some(other), ..self }
    }
    /// Bold text uses this file.
    pub fn bold(self, file: FontFile) -> Self {
        self.with(Face::Bold, file)
    }
    /// Monospace text uses this file.
    pub fn mono(self, file: FontFile) -> Self {
        self.with(Face::Mono, file)
    }
    /// Italic text uses this file.
    pub fn italic(self, file: FontFile) -> Self {
        self.with(Face::Italic, file)
    }
    fn with(mut self, face: Face, file: FontFile) -> Self {
        self.faces[face.number() as usize] = self.files.len();
        self.files.push(file);
        self
    }

    /// Walks a line of text in whole pixels: `each` gets every glyph with its pen position, and
    /// the final pen position is returned. Measuring and drawing both go through here, so they
    /// agree. A run of characters the file lacks goes to `each` as one piece for the fallback,
    /// where there is one: it returns that piece's width in pixels.
    #[cfg(not(feature = "shaping"))]
    fn along(&mut self, text: &str, font: Font, scale: f32, mut each: impl FnMut(Written, f32) -> f32) -> f32 {
        if self.cache_bytes > CACHE_LIMIT {
            self.drawn.clear();
            self.cache_bytes = 0;
        }
        let AppFonts { files, faces, other, drawn, cache_bytes, hinting, forms, .. } = self;
        let (which, pixels) = (faces.get(font.face.number() as usize).copied().unwrap_or(0), font.size * scale);
        let file = &files[which];
        // Sizes compare by bits, so NaN is still one size.
        let known = drawn.iter().position(|drawn| drawn.file == which && drawn.pixels.to_bits() == pixels.to_bits()).unwrap_or(drawn.len());
        if known == drawn.len() {
            drawn.push(Drawn {
                file: which,
                pixels,
                letters: Vec::new(),
                pages: Vec::new(),
                pairs: Vec::new(),
                met: 0,
                #[cfg(feature = "hinting")]
                hints: Default::default(),
            });
            *cache_bytes += size_of::<Drawn>();
        }
        let drawn = &mut drawn[known];
        let (mut pen, mut before, mut lacking) = (0.0, None, 0..0);
        // An emoji (with all its parts) goes to the fallback whole, whatever glyphs the file
        // has for its parts: `theirs` is where the current one ends.
        let (pictures, mut theirs) = (other.is_some() && !crate::emoji::plain(text), 0);
        for (at, letter) in text.char_indices() {
            if pictures && at >= theirs {
                theirs = at + crate::emoji::picture(letter, &text[at..]);
            }
            let now = drawn.letter(file, letter, *hinting, *forms, cache_bytes);
            // A missing character waits for following ones like it.
            if (drawn.letters[now].glyph == 0 || at < theirs) && other.is_some() {
                (lacking, before) = (if lacking.is_empty() { at } else { lacking.start }..at + letter.len_utf8(), None);
                continue;
            }
            pen += left_to(other, &text[std::mem::take(&mut lacking)], pen, &mut each);
            pen += before.map_or(0.0, |before| drawn.closer(file, before, now, cache_bytes));
            let letter = &drawn.letters[now];
            each(Written::Own(file, letter), pen);
            (pen, before) = (pen + letter.advance, Some(now));
        }
        pen + left_to(other, &text[lacking], pen, &mut each)
    }

    /// The same, shaped (feature `shaping`): each character's end is said too.
    #[cfg(feature = "shaping")]
    fn along(&mut self, text: &str, font: Font, scale: f32, each: impl FnMut(Written, f32) -> f32) -> f32 {
        if self.cache_bytes > CACHE_LIMIT {
            self.drawn.clear();
            self.cache_bytes = 0;
        }
        let (which, pixels) = (self.faces.get(font.face.number() as usize).copied().unwrap_or(0), font.size * scale);
        let known = self.drawn_at(which, pixels);
        self.along_shaped(text, which, known, each)
    }

    /// How a line is laid: each glyph with its pen position and rise, and the line's width, in
    /// pixels at one pixel per unit.
    #[cfg(all(test, feature = "shaping"))]
    pub(crate) fn laid(&mut self, text: &str, font: Font) -> (Vec<(u16, [f32; 2])>, f32) {
        let mut glyphs = Vec::new();
        let wide = self.along(text, font, 1.0, |written, pen| match written {
            Written::Own(_, letter, rise) => (glyphs.push((letter.glyph, [pen, rise])), 0.0).1,
            _ => 0.0,
        });
        (glyphs, wide)
    }

    /// The glyphs drawn of file `which` at a pixel size: their index, added if new. (Sizes
    /// compare by bits, so NaN is still one size.)
    #[cfg(feature = "shaping")]
    fn drawn_at(&mut self, which: usize, pixels: f32) -> usize {
        let known = self.drawn.iter().position(|drawn| drawn.file == which && drawn.pixels.to_bits() == pixels.to_bits()).unwrap_or(self.drawn.len());
        if known == self.drawn.len() {
            self.drawn.push(Drawn {
                file: which,
                pixels,
                letters: Vec::new(),
                shapes: Vec::new(),
                #[cfg(feature = "hinting")]
                hints: Default::default(),
            });
            self.cache_bytes += size_of::<Drawn>();
        }
        known
    }

    /// `along`, shaped (feature `shaping`): each run of one script that file `which` has every
    /// character of is shaped as on a page (`shaping.rs`), and its glyphs laid in whole pixels;
    /// the rest goes to the fallback as unshaped. `known` is the file's drawn glyphs at the
    /// size.
    #[cfg(feature = "shaping")]
    fn along_shaped(&mut self, text: &str, which: usize, known: usize, mut each: impl FnMut(Written, f32) -> f32) -> f32 {
        let pictures = self.other.is_some() && !crate::emoji::plain(text);
        let (mut pen, mut at) = (0.0, 0);
        while at < text.len() {
            let (long, own) = piece(&self.files[which], &text[at..], self.other.is_some(), pictures);
            let run = &text[at..at + long];
            pen += match own {
                true => self.lay_run(which, known, run, pen, &mut each),
                false => left_to(&mut self.other, run, pen, &mut each),
            };
            at += long;
        }
        pen
    }

    /// Walks a run shaped in file `which` from `pen`: each glyph at its place, then each
    /// character's end. Returns its width. A glyph's advance is whole pixels as an unshaped
    /// one's is, plus what shaping added to it (rounded as a running sum, so a pair's kerning
    /// split over two glyphs rounds as one); a glyph with none (a mark) is placed from the last
    /// that has one, by their distance in the font.
    #[cfg(feature = "shaping")]
    fn lay_run(&mut self, which: usize, known: usize, run: &str, pen: f32, each: &mut impl FnMut(Written, f32) -> f32) -> f32 {
        let at = self.shaped_run(which, run);
        let AppFonts { files, drawn, shaped, cache_bytes, hinting, .. } = self;
        let (file, drawn, placed) = (&files[which], &mut drawn[known], &shaped.runs[at].2);
        let unit = drawn.pixels / file.units;
        let (mut x, mut exact, mut added, mut base, mut spans) = (0.0, 0, 0.0, (0.0, 0), Vec::with_capacity(placed.len()));
        for one in placed.iter() {
            let n = drawn.shaped(file, one.glyph, one.letter, *hinting, cache_bytes);
            let letter = &drawn.letters[n];
            let (advance, offset) = (one.advance as i32, one.offset.map(i32::from));
            let more = if advance == 0 { 0.0 } else { advance as f32 - file.width(one.glyph) };
            let wide = if advance == 0 { 0.0 } else { letter.advance + ((added + more) * unit).round() - (added * unit).round() };
            added += more;
            let left = match advance {
                0 => base.0 + ((exact + offset[0] - base.1) as f32 * unit).round(),
                _ => x + (offset[0] as f32 * unit).round(),
            };
            if advance != 0 {
                base = (left, exact + offset[0]);
            }
            each(Written::Own(file, letter, (offset[1] as f32 * unit).round()), pen + left);
            spans.push((one.from as u32, [x, x + wide]));
            (x, exact) = (x + wide, exact + advance);
        }
        for end in ends(run, &spans) {
            each(Written::End, pen + end);
        }
        x
    }

    /// The cache index of a run shaped in file `which`, by the plan for its script and the
    /// language asked for: shaped now if new.
    #[cfg(feature = "shaping")]
    fn shaped_run(&mut self, which: usize, run: &str) -> usize {
        if self.shaped.bytes > RUNS_LIMIT {
            (self.shaped.runs, self.shaped.bytes) = (Vec::new(), 0);
        }
        let Shaped { plans, runs, bytes } = &mut self.shaped;
        let at = match runs.binary_search_by(|(file, text, _)| (*file, &**text).cmp(&(which, run))) {
            Ok(at) => return at,
            Err(at) => at,
        };
        let script = run.chars().find_map(Script::of).unwrap_or(Script::None);
        let language = self.forms.map_or(Language::NONE, |(_, language)| language);
        let plan = match plans.iter().position(|(file, known, _)| *file == which && *known == script) {
            Some(plan) => plan,
            None => {
                plans.push((which, script, Plan::of(&self.files[which], script, false, language)));
                plans.len() - 1
            }
        };
        let mut laid = Vec::new();
        self.files[which].shape(&plans[plan].2, run, &mut laid);
        let letters: Vec<char> = run.chars().collect();
        // (Font units fit 16 bits; a damaged font's are held there.)
        let short = |units: i32| units.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let place = |one: &crate::shaping::Laid| Placed {
            glyph: one.glyph,
            from: one.from as u16,
            letter: letters.get(one.from as usize).copied().unwrap_or(' '),
            advance: short(one.advance),
            offset: one.offset.map(short),
        };
        let placed: Box<[Placed]> = laid.iter().map(place).collect();
        *bytes += run.len() + size_of_val(&*placed) + size_of::<(usize, Box<str>, Box<[Placed]>)>();
        runs.insert(at, (which, run.into(), placed));
        at
    }
}

/// The piece at the start of `rest` that goes one way: grapheme clusters a file has every
/// character of, in one script (characters of none join it), at most what is shaped at once;
/// or, with a `fallback`, those it lacks and emoji (where `pictures` may be). Its length, and
/// whether it is the file's.
#[cfg(feature = "shaping")]
fn piece(file: &FontFile, rest: &str, fallback: bool, pictures: bool) -> (usize, bool) {
    let (mut long, mut count, mut script, mut own) = (0, 0, None, None);
    while long < rest.len() && count < crate::shaping::LONGEST - 64 {
        let here = &rest[long..];
        let first = here.chars().next().unwrap_or(' ');
        let picture = if pictures { crate::emoji::picture(first, here) } else { 0 };
        let size = if picture > 0 { picture } else { crate::text::cluster(here).max(1) };
        let has = picture == 0 && (!fallback || here[..size].chars().all(|letter| crate::paper::unseen(letter) || file.glyph(letter) != 0));
        let named = here[..size].chars().find_map(Script::of);
        let apart = matches!((script, named), (Some(one), Some(other)) if one != other);
        if own.is_some_and(|was| was != has || (has && apart)) {
            break;
        }
        (own, script, long, count) = (Some(has), script.or(named), long + size, count + here[..size].chars().count());
    }
    (long, own.unwrap_or(true))
}

/// Each character's end in a shaped run, from its start: the grapheme clusters from a glyph's
/// character to the next glyph's share its span (`spans`: each glyph's character and pen span)
/// evenly, as a ligature's components do.
#[cfg(feature = "shaping")]
fn ends(run: &str, spans: &[(u32, [f32; 2])]) -> Vec<f32> {
    let mut own: Vec<Option<[f32; 2]>> = vec![None; run.chars().count()];
    for (from, [left, right]) in spans.iter().copied() {
        if let Some(span) = own.get_mut(from as usize) {
            *span = Some(span.map_or([left, right], |[l, r]| [l.min(left), r.max(right)]));
        }
    }
    // The clusters: their first character and how many.
    let (mut clusters, mut at, mut n) = (Vec::new(), 0, 0);
    while at < run.len() {
        let size = crate::text::cluster(&run[at..]).max(1);
        let count = run[at..at + size].chars().count();
        clusters.push((n, count));
        (at, n) = (at + size, n + count);
    }
    let span_of = |(first, count): (usize, usize)| own[first..first + count].iter().flatten().copied().reduce(|[l, r], [left, right]| [l.min(left), r.max(right)]);
    let mut ends = vec![0.0; own.len()];
    let mut k = 0;
    while k < clusters.len() {
        let [left, right] = span_of(clusters[k]).unwrap_or([0.0; 2]);
        let shared = clusters[k + 1..].iter().take_while(|cluster| span_of(**cluster).is_none()).count() + 1;
        for (m, (first, count)) in clusters[k..k + shared].iter().copied().enumerate() {
            let end = left + ((right - left) * (m + 1) as f32 / shared as f32).round();
            ends[first..first + count].iter_mut().for_each(|one| *one = end);
        }
        k += shared;
    }
    ends
}

#[cfg(feature = "hinting")]
impl AppFonts {
    /// Cap-height and x-height in pixels as a font's instructions draw them (the tops of its
    /// "H" and "x"), if they hint at this size.
    fn instructed_heights(&mut self, font: Font, scale: f32) -> Option<[Option<f32>; 2]> {
        (self.hinting == Hinting::Own).then_some(())?;
        // (Walking no text makes the size's cache, where its programs' state is kept.)
        self.along("", font, scale, |_, _| 0.0);
        let (which, pixels) = (self.faces.get(font.face.number() as usize).copied().unwrap_or(0), font.size * scale);
        let drawn = self.drawn.iter_mut().find(|drawn| drawn.file == which && drawn.pixels.to_bits() == pixels.to_bits())?;
        drawn.hints.applies(&self.files[which], pixels).then_some(())?;
        let mut tops = Vec::new();
        self.along("Hx", font, scale, |written, _| {
            let top = |letter: &Letter| (letter.glyph != 0 && !letter.cover.is_empty()).then_some(-letter.top);
            tops.extend(if let Written::Own(_, letter, ..) = written { Some(top(letter)) } else { None });
            0.0
        });
        (tops.len() == 2).then(|| [tops[0], tops[1]])
    }
}

/// A line's pieces as walked: a glyph of the app's font, with how far it is raised; the end of
/// a character of the font, where shaped; or a run the font lacks, for the fallback.
enum Written<'a> {
    Own(&'a FontFile, &'a Letter, #[cfg(feature = "shaping")] f32),
    #[cfg(feature = "shaping")]
    End,
    Other(&'a mut dyn TextSystem, &'a str),
}

/// Passes a run the font lacks to the fallback, with the pen position; returns the pen advance.
/// (`None` for an empty run.)
fn left_to(other: &mut Option<Box<dyn TextSystem>>, piece: &str, pen: f32, each: &mut impl FnMut(Written, f32) -> f32) -> f32 {
    match (other, piece.is_empty()) {
        (Some(other), false) => each(Written::Other(other.as_mut(), piece), pen),
        _ => 0.0,
    }
}

/// The fallback's width for a piece, in whole pixels; with `stops`, each character's end, from
/// the line start.
fn measured_by(other: &mut dyn TextSystem, piece: &str, font: Font, scale: f32, pen: f32, stops: Option<&mut Vec<f32>>) -> f32 {
    let from = stops.as_ref().map_or(0, |stops| stops.len());
    let mut stops = stops;
    let wide = (other.measure(piece, font, scale, stops.as_deref_mut()) * scale).round();
    stops.into_iter().for_each(|stops| stops[from..].iter_mut().for_each(|stop| *stop += pen / scale));
    wide
}

impl AppFonts {
    /// The fallback, for a font it takes entirely: monospace with no file given, and a named
    /// system font.
    fn all_by(&mut self, font: Font) -> Option<&mut dyn TextSystem> {
        let other = self.other.as_deref_mut()?;
        (font.face == Face::Mono && self.faces[2] == 0 || matches!(font.face, Face::Named(_))).then_some(other)
    }
}

impl TextSystem for AppFonts {
    /// Line height is given in whole pixels, so lines sit on whole pixels; and cap-height and
    /// x-height as drawn, hinted (see `hinting.rs`), so a line is centred by its visible
    /// glyphs.
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        if let Some(other) = self.all_by(font) {
            return other.metrics(font, scale);
        }
        #[cfg(feature = "hinting")]
        let own = self.instructed_heights(font, scale);
        #[cfg(not(feature = "hinting"))]
        let own = None;
        let file = &self.files[self.faces.get(font.face.number() as usize).copied().unwrap_or(0)];
        let exact = file.metrics(font.size * scale);
        let whole = |of: f32| of.round() / scale;
        let auto = || (self.hinting != Hinting::Off).then(|| crate::hinting::heights(file, font.size * scale / file.units)).flatten();
        let [cap, small] = own.or_else(auto).unwrap_or_default();
        let (cap, small) = (cap.map_or(whole(exact.cap_height), |cap| cap / scale), small.map_or(whole(exact.x_height), |small| small / scale));
        Metrics { ascent: whole(exact.ascent), descent: whole(exact.descent), cap_height: cap, x_height: small }
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        if let Some(other) = self.all_by(font) {
            return other.measure(text, font, scale, stops);
        }
        let end = self.along(text, font, scale, |written, pen| match written {
            // (Unshaped, each glyph is a character, which ends at its advance.)
            #[cfg(not(feature = "shaping"))]
            Written::Own(_, letter) => {
                stops.iter_mut().for_each(|stops| stops.push((pen + letter.advance) / scale));
                0.0
            }
            #[cfg(feature = "shaping")]
            Written::Own(..) => 0.0,
            #[cfg(feature = "shaping")]
            Written::End => (stops.iter_mut().for_each(|stops| stops.push(pen / scale)), 0.0).1,
            Written::Other(other, piece) => measured_by(other, piece, font, scale, pen, stops.as_deref_mut()),
        });
        end / scale
    }

    /// Glyphs use the language's localized forms from now on, if the app asked for them.
    fn set_language(&mut self, language: Language) {
        if let Some((_, own)) = self.forms.as_mut().filter(|(_, own)| *own != language) {
            *own = language;
            (self.drawn, self.cache_bytes) = (Vec::new(), 0);
            #[cfg(feature = "shaping")]
            {
                self.shaped = Shaped::default();
            }
        }
        if let Some(other) = self.other.as_mut() {
            other.set_language(language);
        }
    }

    fn file(&mut self, face: Face, letter: char) -> Option<&FontFile> {
        let own = &self.files[self.faces.get(face.number() as usize).copied().unwrap_or(0)];
        if own.glyph(letter) != 0 || self.other.is_none() {
            return Some(own);
        }
        self.other.as_mut()?.file(face, letter)
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        if let Some(other) = self.all_by(font) {
            return other.draw(canvas, text, x, y, font, color);
        }
        let scale = canvas.scale();
        let ascent = self.metrics(font, scale).ascent;
        // The fallback's glyphs sit on the same baseline: lower by its smaller ascent.
        let lower = self.other.as_mut().map_or(0.0, |other| ascent - other.metrics(font, scale).ascent);
        let (left, line) = ((x * scale).round(), (y * scale).round() + (ascent * scale).round());
        self.along(text, font, scale, |written, pen| match written {
            #[cfg(not(feature = "shaping"))]
            Written::Own(file, letter) => (letter.lay(canvas, file, left + pen, line, font.size, color), 0.0).1,
            #[cfg(feature = "shaping")]
            Written::Own(file, letter, rise) => (letter.lay(canvas, file, left + pen, line - rise, font.size, color), 0.0).1,
            #[cfg(feature = "shaping")]
            Written::End => 0.0,
            Written::Other(other, piece) => {
                other.draw(canvas, piece, (left + pen) / scale, y + lower, font, color);
                measured_by(other, piece, font, scale, pen, None)
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cff::tests::{curved, summed_and_accented};
    use crate::geom::rect;

    pub(crate) type Tables = Vec<([u8; 4], Vec<u8>)>;

    /// Big-endian 16-bit values, as fonts store them.
    pub(crate) fn words(numbers: &[i32]) -> Vec<u8> {
        numbers.iter().flat_map(|number| (*number as u16).to_be_bytes()).collect()
    }

    /// Big-endian 32-bit values.
    pub(crate) fn longs(numbers: &[u32]) -> Vec<u8> {
        numbers.iter().flat_map(|number| number.to_be_bytes()).collect()
    }

    /// A font file: sfnt version, table directory, and tables.
    pub(crate) fn file(kind: &[u8; 4], tables: &Tables) -> Vec<u8> {
        let mut file = [&kind[..], &words(&[tables.len() as i32, 0, 0, 0])].concat();
        let mut at = file.len() + tables.len() * 16;
        for (name, table) in tables {
            file.extend([&name[..], &longs(&[0, at as u32, table.len() as u32])].concat());
            at += table.len();
        }
        tables.iter().for_each(|(_, table)| file.extend(table));
        file
    }

    /// The tables, with one replaced.
    pub(crate) fn with(mut tables: Tables, name: &[u8; 4], table: Vec<u8>) -> Tables {
        tables.iter_mut().find(|(called, _)| called == name).expect("the table is there").1 = table;
        tables
    }

    /// A simple glyph as `glyf` stores it: each contour a list of on-curve or control points.
    /// Every delta is encoded shortest, and repeated flags are run-length coded, as font tools
    /// do.
    pub(crate) fn glyph(outlines: &[&[(i32, i32, bool)]]) -> Vec<u8> {
        let points = outlines.concat();
        let ends: Vec<i32> = (1..=outlines.len()).map(|n| outlines[..n].concat().len() as i32 - 1).collect();
        let left = points.iter().map(|point| point.0).min().unwrap();
        let mut glyph = [words(&[outlines.len() as i32, left, 0, 0, 0]), words(&ends), words(&[0])].concat();
        let steps: Vec<(i32, i32)> = std::iter::once(&(0, 0, true)).chain(&points).zip(&points).map(|(from, to)| (to.0 - from.0, to.1 - from.1)).collect();
        // A delta's flags: zero (same), one byte positive, one byte negative, two bytes.
        let said = |step: i32| match step {
            0 => 0x10,
            1..=255 => 0x12,
            -255..=-1 => 0x02,
            _ => 0x00,
        };
        let flags: Vec<u8> = points.iter().zip(&steps).map(|((_, _, on), (dx, dy))| *on as u8 | said(*dx) | said(*dy) << 1).collect();
        for run in flags.chunk_by(|a, b| a == b) {
            glyph.extend(if run.len() > 1 { vec![run[0] | 8, run.len() as u8 - 1] } else { vec![run[0]] });
        }
        let written = |step: i32| match step {
            0 => vec![],
            -255..=255 => vec![step.unsigned_abs() as u8],
            _ => (step as i16).to_be_bytes().to_vec(),
        };
        glyph.extend(steps.iter().flat_map(|(dx, _)| written(*dx)));
        glyph.extend(steps.iter().flat_map(|(_, dy)| written(*dy)));
        glyph
    }

    /// A composite component: glyph, offset, 2×2 transform, and flags to flip. (Flipping flag 2
    /// makes the offset point numbers: one of earlier components', one of its own.)
    pub(crate) type Piece = (i32, [i32; 2], [f32; 4], i32);

    /// A composite glyph. `left` is its xMin.
    pub(crate) fn made_of(left: i32, parts: &[Piece]) -> Vec<u8> {
        let mut glyph = words(&[-1, left, 0, 0, 0]);
        for (n, (part, moved, by, flags)) in parts.iter().enumerate() {
            let wide = moved.iter().any(|by| !(-128..128).contains(by)) != (flags & 1 != 0);
            let (how, numbers) = match by {
                [a, b, c, d] if b != &0.0 || c != &0.0 => (0x80, vec![*a, *b, *c, *d]),
                [a, _, _, d] if a != d => (0x40, vec![*a, *d]),
                [a, ..] if a != &1.0 => (0x08, vec![*a]),
                _ => (0, vec![]),
            };
            let more = if n + 1 < parts.len() { 0x20 } else { 0 };
            glyph.extend(words(&[(2 | how | more) ^ (flags & !1) | wide as i32, *part]));
            glyph.extend(if wide { words(moved) } else { vec![moved[0] as u8, moved[1] as u8] });
            glyph.extend(words(&numbers.iter().map(|number| (number * 16384.0) as i32).collect::<Vec<_>>()));
        }
        glyph
    }

    /// The test font's glyphs.
    fn glyphs() -> Vec<Vec<u8>> {
        let (on, off, same) = (true, false, [1.0, 0.0, 0.0, 1.0]);
        vec![
            // 0, notdef: a box.
            glyph(&[&[(64, 0, on), (64, 448, on), (256, 448, on), (256, 0, on)]]),
            // 1, "A": a box with a hole.
            glyph(&[&[(0, 0, on), (0, 384, on), (384, 384, on), (384, 0, on)], &[(128, 128, on), (256, 128, on), (256, 256, on), (128, 256, on)]]),
            // 2, "B": curves. One contour starts on-curve, one has no on-curve point, one
            // starts on a control point.
            glyph(&[
                &[(0, 0, on), (0, 512, off), (512, 512, on), (512, 0, on)],
                &[(64, 64, off), (64, 192, off), (192, 192, off), (192, 64, off)],
                &[(320, 64, off), (320, 192, on), (448, 192, off), (448, 64, on)],
            ]),
            // 3, "C": an "A" offset, and a "B" at half size.
            made_of(64, &[(1, [64, 64], same, 0), (2, [512, 0], [0.5, 0.0, 0.0, 0.5], 0)]),
            // 4, space: nothing.
            vec![],
            // 5, "D": a "C" half as wide, and the box rotated a quarter.
            made_of(32, &[(3, [0, 0], [0.5, 0.0, 0.0, 1.0], 0), (0, [512, 64], [0.0, 1.0, -1.0, 0.0], 0)]),
            // 6, "E": composed of itself, which no font should be.
            made_of(0, &[(6, [0, 0], same, 0)]),
            // 7, "F": the box at half size, its offset scaled with it.
            made_of(96, &[(0, [128, 64], [0.5, 0.0, 0.0, 0.5], 0x800)]),
            // 8, "G": a box whose points lie right of its `hmtx` left side bearing.
            glyph(&[&[(128, 0, on), (128, 128, on), (256, 128, on), (256, 0, on)]]),
            // 9, "H": a "G" (flagged USE_MY_METRICS) and the box.
            made_of(64, &[(8, [192, 0], same, 0x200), (0, [0, 0], same, 0)]),
        ]
    }

    /// The test font's `cmap` format 4: a space; "A" to "C" by delta; "D" to "H" by glyph
    /// array.
    fn stretches() -> Vec<u8> {
        let (last, first, add, onward) = ([32, 67, 72, 0xffff], [32, 65, 68, 0xffff], [4 - 32, 1 - 65, 1, 1], [0, 0, 4, 0]);
        [words(&[4, 58, 0, 8, 8, 2, 0]), words(&[&last[..], &[0], &first, &add, &onward, &[4, 5, 6, 7, 8]].concat())].concat()
    }

    /// A 1024-unit font built byte by byte around these glyphs. Ascender 832, descender 192; it
    /// kerns "AB" closer and "BA" apart.
    pub(crate) fn tables_of(glyphs: Vec<Vec<u8>>) -> Tables {
        // Glyphs start on even offsets: `loca` short format counts in halves.
        let glyphs: Vec<Vec<u8>> = glyphs.into_iter().map(|glyph| [&glyph[..], &[0][..glyph.len() % 2]].concat()).collect();
        let starts: Vec<i32> = (0..=glyphs.len()).map(|n| glyphs[..n].concat().len() as i32 / 2).collect();
        vec![
            (*b"head", [words(&[1, 0, 0, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1024]), vec![0; 34]].concat()),
            (*b"hhea", [words(&[1, 0, 832, -192]), vec![0; 26], words(&[7])].concat()),
            (*b"maxp", [words(&[1, 0, 10]), vec![0; 26]].concat()),
            // Seven glyphs' advance and left side bearing, then the rest's left side bearings.
            (*b"hmtx", words(&[320, 64, 448, 0, 576, 0, 640, 64, 256, 0, 640, 32, 384, 0, 96, 64, 999])),
            (*b"cmap", [words(&[0, 1, 3, 1, 0, 12]), stretches()].concat()),
            (*b"loca", words(&starts)),
            (*b"glyf", glyphs.concat()),
            (*b"kern", words(&[0, 1, 0, 26, 1, 2, 0, 0, 0, 1, 2, -128, 2, 1, 64])),
        ]
    }

    pub(crate) fn tables() -> Tables {
        tables_of(glyphs())
    }
    /// The test font's cap-height in units: its "H", which is the box's top. (No lowercase.)
    const CAP: f32 = 448.0;

    fn read(tables: &Tables) -> FontFile {
        FontFile::read(file(&[0, 1, 0, 0], tables)).expect("the font is read")
    }

    /// The drawn pixels, row by row, and whether each is fully covered.
    pub(crate) fn ink(pixels: &[u32], width: usize) -> (Vec<(usize, usize)>, bool) {
        let lit = pixels.iter().enumerate().filter(|(_, pixel)| **pixel != 0).map(|(n, _)| (n % width, n / width));
        (lit.collect(), pixels.iter().all(|pixel| [0, 0xffff_ffff].contains(pixel)))
    }

    /// Every pixel of some columns in some rows, row by row.
    fn block(columns: &[usize], rows: std::ops::Range<usize>) -> Vec<(usize, usize)> {
        rows.flat_map(|y| columns.iter().map(move |x| (*x, y))).collect()
    }

    #[test]
    fn a_font_says_how_large_its_letters_are() {
        let font = read(&tables());
        assert_eq!(font.metrics(1024.0), Metrics { ascent: 832.0, descent: 192.0, cap_height: CAP, x_height: 0.0 });
        assert_eq!(font.metrics(16.0), Metrics { ascent: 13.0, descent: 3.0, cap_height: CAP / 64.0, x_height: 0.0 });
        assert_eq!(" ABCDEFGH".chars().map(|letter| font.glyph(letter)).collect::<Vec<_>>(), [4, 1, 2, 3, 5, 6, 7, 8, 9]);
        assert_eq!(" ABCDE".chars().map(|letter| font.advance(letter, 1024.0)).collect::<Vec<_>>(), [256.0, 448.0, 576.0, 640.0, 640.0, 384.0]);
        assert_eq!("FGH".chars().map(|letter| font.advance(letter, 16.0)).collect::<Vec<_>>(), [6.0; 3], "the last glyphs share the last width");
        assert_eq!([font.kerning('A', 'B', 1024.0), font.kerning('B', 'A', 16.0), font.kerning('A', 'A', 16.0), font.kerning('B', 'C', 16.0)], [-128.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_character_the_font_lacks_is_the_missing_glyph() {
        let font = read(&tables());
        for lacking in ['x', '@', '\u{3b1}', '\u{1F600}', '\u{10FFFF}'] {
            assert_eq!((font.glyph(lacking), font.advance(lacking, 1024.0)), (0, 320.0), "{lacking:?}");
            assert_eq!(font.outline(lacking, 1024.0), Path::svg("M64 832 64 384 256 384 256 832Z"), "{lacking:?}");
        }
        // A subtable mapping to a glyph the font lacks is no better than none.
        let font = read(&with(tables(), b"cmap", words(&[0, 1, 3, 1, 0, 12, 6, 14, 0, 65, 2, 1, 10])));
        assert_eq!([font.glyph('A'), font.glyph('B')], [1, 0]);
    }

    #[test]
    fn every_way_of_listing_characters_finds_the_same_glyphs() {
        // Format 12 groups, two beyond the BMP; and a format 6 run.
        let any = [words(&[12, 0, 0, 64, 0, 0, 0, 4]), longs(&[32, 32, 4, 65, 67, 1, 68, 72, 5, 0x1F600, 0x1F601, 1])].concat();
        let row = words(&[6, 26, 0, 65, 8, 1, 2, 3, 5, 6, 7, 8, 9]);
        for (name, system, list) in
            [("stretches", [3, 1], stretches()), ("any characters", [3, 10], any.clone()), ("any characters, numbered by Unicode itself", [0, 4], any.clone()), ("a row", [0, 3], row.clone())]
        {
            let font = read(&with(tables(), b"cmap", [words(&[0, 1, system[0], system[1], 0, 12]), list].concat()));
            assert_eq!("ABCDEFGH@I\u{80}".chars().map(|letter| font.glyph(letter)).collect::<Vec<_>>(), [1, 2, 3, 5, 6, 7, 8, 9, 0, 0, 0], "{name}");
            assert_eq!(['\u{1F600}', '\u{1F601}', '\u{1F602}'].map(|letter| font.glyph(letter)), if name.starts_with("any") { [1, 2, 0] } else { [0; 3] }, "{name}");
            assert_eq!(font.glyph(' '), if name == "a row" { 0 } else { 4 }, "{name}");
        }
        // Of several subtables the best is taken: here the full Unicode one, not the format 6
        // before it, another platform's, or an unknown format.
        let lists = [words(&[0, 4, 3, 1, 0, 36, 1, 0, 0, 36, 3, 10, 0, 62, 3, 10, 0, 126]), words(&[6, 26, 0, 65, 8, 9, 9, 9, 9, 9, 9, 9, 9]), any, words(&[13, 0])].concat();
        let font = read(&with(tables(), b"cmap", lists));
        assert_eq!(['A', '\u{1F601}'].map(|letter| font.glyph(letter)), [1, 2]);
        let another = with(tables(), b"cmap", [words(&[0, 1, 1, 0, 0, 12]), row].concat());
        assert!(FontFile::read(file(&[0, 1, 0, 0], &another)).is_none(), "no list for Unicode, no font");
    }

    #[test]
    fn a_glyph_is_the_outline_its_points_say() {
        let font = read(&tables());
        assert_eq!(font.outline('A', 1024.0), Path::svg("M0 832 0 448 384 448 384 832ZM128 704 256 704 256 576 128 576Z"));
        // Between two control points lies an implied on-curve one; a contour with no on-curve
        // point starts halfway between its first and last.
        let curves = "M0 832Q0 320 512 320L512 832ZM128 768Q64 768 64 704Q64 640 128 640Q192 640 192 704Q192 768 128 768ZM448 768Q320 768 320 640Q448 640 448 768Z";
        assert_eq!(font.outline('B', 1024.0), Path::svg(curves));
        assert_eq!(font.outline(' ', 1024.0), Path::new());
        // At another size, the same shape larger, still down from the line's top.
        assert_eq!(font.outline('A', 2048.0), Path::svg("M0 1664 0 896 768 896 768 1664ZM256 1408 512 1408 512 1152 256 1152Z"));
        // `loca` may be long format too.
        let starts: Vec<u32> = (0..=glyphs().len()).map(|n| glyphs()[..n].concat().len() as u32).collect();
        let head = [words(&[1, 0, 0, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1024]), vec![0; 30], words(&[1, 0])].concat();
        let long = with(with(with(tables(), b"loca", longs(&starts)), b"glyf", glyphs().concat()), b"head", head);
        let long = FontFile::read(file(b"true", &long)).expect("the font is read");
        "ABCDEFGH x".chars().for_each(|letter| assert_eq!(long.outline(letter, 1024.0), font.outline(letter, 1024.0), "{letter:?}"));
    }

    #[test]
    fn a_glyph_made_of_others_is_their_shapes_stretched_turned_and_moved() {
        let font = read(&tables());
        let moved = "M64 768 64 384 448 384 448 768ZM192 640 320 640 320 512 192 512Z";
        let halved = "M512 832Q512 576 768 576L768 832ZM576 800Q544 800 544 768Q544 736 576 736Q608 736 608 768Q608 800 576 800ZM736 800Q672 800 672 736Q736 736 736 800Z";
        assert_eq!(font.outline('C', 1024.0), Path::svg(&[moved, halved].concat()));
        // Nested components: all of "C" at half width, then the rotated box.
        let narrow = "M32 768 32 384 224 384 224 768ZM96 640 160 640 160 512 96 512ZM256 832Q256 576 384 576L384 832ZM288 800Q272 800 272 768Q272 736 288 736Q304 736 304 768Q304 800 288 800ZM368 800Q336 800 336 736Q368 736 368 800Z";
        assert_eq!(font.outline('D', 1024.0), Path::svg(&[narrow, "M512 704 64 704 64 512 512 512Z"].concat()));
        assert_eq!(font.outline('F', 1024.0), Path::svg("M96 800 96 576 192 576 192 800Z"), "the way it is moved is halved with it");
        // A glyph composed of itself is blank, but keeps its advance.
        assert_eq!((font.outline('E', 1024.0), font.advance('E', 1024.0)), (Path::new(), 384.0));
        // So is one with more components than any real glyph, each empty.
        let mut endless = glyphs();
        endless[3] = made_of(0, &vec![(4, [0, 0], [1.0, 0.0, 0.0, 1.0], 0); MOST + 1]);
        assert_eq!(read(&tables_of(endless)).outline('C', 1024.0), Path::new());
    }

    /// The glyphs, with "D" composed of these components.
    fn with_parts(parts: &[Piece]) -> Vec<Vec<u8>> {
        with_glyph(glyphs(), 5, made_of(32, parts))
    }

    /// The glyphs, with one replaced.
    fn with_glyph(mut glyphs: Vec<Vec<u8>>, glyph: usize, made: Vec<u8>) -> Vec<Vec<u8>> {
        glyphs[glyph] = made;
        glyphs
    }

    #[test]
    fn a_part_is_put_in_its_place_by_a_move_or_by_a_point() {
        let (same, halved) = ([1.0, 0.0, 0.0, 1.0], [0.5, 0.0, 0.0, 0.5]);
        let drawn = |glyphs: Vec<Vec<u8>>| read(&tables_of(glyphs)).outline('D', 1024.0);
        // The box, and an "A" whose point 0 is moved onto the box's point 2, its top right
        // corner: 256 right of the pen and 448 up.
        let laid = drawn(with_parts(&[(0, [0, 0], same, 0), (1, [2, 0], same, 2)]));
        assert_eq!(laid, Path::svg("M64 832 64 384 256 384 256 832ZM256 384 256 0 640 0 640 384ZM384 256 512 256 512 128 384 128Z"));
        assert_eq!(laid, drawn(with_parts(&[(0, [0, 0], same, 0), (1, [256, 448], same, 0)])), "as if it were moved there");
        assert_eq!(laid, drawn(with_parts(&[(0, [0, 0], same, 0), (1, [2, 0], same, 3)])), "the points named in two bytes each");
        // A scaled component is scaled first: the halved "A"'s corner at 192, 192 then lands on
        // the box's corner at 64, 448.
        let stretched = [(0, [0, 0], same, 0), (1, [1, 2], halved, 2)];
        assert_eq!(drawn(with_parts(&stretched)), drawn(with_parts(&[(0, [0, 0], same, 0), (1, [-128, 256], halved, 0)])));
        // A nested composite moves whole ("C"'s point 4 is at 192, 192), and its points count
        // like any for matching (point 9 of "C" is at 512, 256).
        let whole = [(0, [0, 0], same, 0), (3, [0, 4], same, 2)];
        assert_eq!(drawn(with_parts(&whole)), drawn(with_parts(&[(0, [0, 0], same, 0), (3, [-128, -192], same, 0)])));
        let counted = [(3, [0, 0], same, 0), (0, [9, 1], same, 2)];
        assert_eq!(drawn(with_parts(&counted)), drawn(with_parts(&[(3, [0, 0], same, 0), (0, [448, -192], same, 0)])));
        // In a glyph that is itself a component, points count from its own first: the "A" in
        // "F" matches the box in "F", not a point of the "A" "D" starts with.
        let inner = with_glyph(glyphs(), 7, made_of(96, &[(0, [0, 0], same, 0), (1, [2, 0], same, 2)]));
        let outer = with_glyph(inner, 5, made_of(32, &[(1, [0, 0], same, 0), (7, [100, 0], same, 0)]));
        assert_eq!(drawn(outer), drawn(with_parts(&[(1, [0, 0], same, 0), (0, [100, 0], same, 0), (1, [356, 448], same, 0)])));
        // A missing point leaves the glyph blank: one beyond the earlier components' (the box
        // has four), beyond the component's own (an "A" has eight, a space none), and any point
        // for the first component.
        for lacking in
            [[(0, [0, 0], same, 0), (1, [4, 0], same, 2)], [(0, [0, 0], same, 0), (1, [0, 8], same, 2)], [(0, [0, 0], same, 0), (4, [0, 0], same, 2)], [(1, [0, 0], same, 2), (0, [0, 0], same, 0)]]
        {
            assert_eq!(drawn(with_parts(&lacking)), Path::new(), "{lacking:?}");
        }
    }

    #[test]
    fn a_glyph_is_put_where_the_table_of_widths_has_its_left_edge() {
        let font = read(&tables());
        // "G"'s points start 128 units from the pen, but its left side bearing is 64.
        assert_eq!(font.outline('G', 1024.0), Path::svg("M64 832 64 704 192 704 192 832Z"));
        // "H" uses its USE_MY_METRICS component "G"'s position, not its own `hmtx` entry.
        assert_eq!(font.outline('H', 1024.0), Path::svg("M256 832 256 704 384 704 384 832ZM0 832 0 384 192 384 192 832Z"));
    }

    #[test]
    fn pairs_are_kerned_by_every_list_that_is_for_them() {
        // Each subtable here has the one pair "AB"; its format comes with it.
        let kerned = |lists: &[(i32, i32)]| {
            let lists: Vec<i32> = lists.iter().flat_map(|(kind, by)| [0, 20, *kind, 1, 0, 0, 0, 1, 2, *by]).collect();
            read(&with(tables(), b"kern", words(&[&[0, lists.len() as i32 / 10], &lists[..]].concat()))).kerning('A', 'B', 1024.0)
        };
        assert_eq!(kerned(&[(1, -128), (1, 32)]), -96.0, "a second list adds to the first");
        assert_eq!(kerned(&[(1, -128), (9, 32)]), 32.0, "or stands instead of it, if it says so");
        assert_eq!(kerned(&[(1, -128), (0, 32), (5, 32), (0x101, 32)]), -128.0, "lists for text that runs down, or written another way, are passed over");
        // A subtable's two-byte length doesn't fit a large one. Its pairs are found anyway, the
        // last too.
        let many: Vec<i32> = (0..11000).flat_map(|n| [1, n, -n]).collect();
        let font = read(&with(tables(), b"kern", words(&[&[0, 1, 0, (14 + 11000 * 6) % 65536, 1, 11000, 0, 0, 0], &many[..]].concat())));
        assert_eq!([(1, 2), (1, 10999), (1, 11000), (2, 1)].map(|(left, right)| font.closer((left, 0), (right, 0))), [-2.0, -10999.0, 0.0, 0.0]);
        // Without the table nothing is kerned.
        let without: Tables = tables().into_iter().filter(|(name, _)| name != b"kern").collect();
        assert_eq!(read(&without).kerning('A', 'B', 1024.0), 0.0);
    }

    /// Items in sequence, after an offset array (from the start of it all) and, with `names`,
    /// tags. `before` comes first.
    fn listed(before: &[u8], names: Option<&[&[u8; 4]]>, things: &[Vec<u8>]) -> Vec<u8> {
        let mut at = before.len() + 2 + things.len() * if names.is_some() { 6 } else { 2 };
        let mut list = [before, &words(&[things.len() as i32])].concat();
        for (n, thing) in things.iter().enumerate() {
            list.extend(names.map_or(&[][..], |names| &names[n][..]));
            list.extend(words(&[at as i32]));
            at += thing.len();
        }
        [list, things.concat()].concat()
    }

    /// A `GPOS` script or feature: its tag, and its indices.
    pub(crate) type Named<'a> = (&'a [u8; 4], &'a [i32]);

    /// A `GPOS` table. A script is its tag and default LangSys feature indices; a feature its
    /// tag and lookup indices; a lookup its type and subtables.
    pub(crate) fn positions(scripts: &[Named], features: &[Named], lookups: &[(i32, Vec<Vec<u8>>)]) -> Vec<u8> {
        fn names<'a>(named: &[Named<'a>]) -> Vec<&'a [u8; 4]> {
            named.iter().map(|(name, _)| *name).collect()
        }
        let script = |numbers: &[i32]| words(&[&[4, 0, 0, 0xffff, numbers.len() as i32], numbers].concat());
        let feature = |numbers: &[i32]| words(&[&[0, numbers.len() as i32], numbers].concat());
        let scripts = listed(&[], Some(&names(scripts)), &scripts.iter().map(|(_, features)| script(features)).collect::<Vec<_>>());
        let features = listed(&[], Some(&names(features)), &features.iter().map(|(_, lookups)| feature(lookups)).collect::<Vec<_>>());
        let lookups = listed(&[], None, &lookups.iter().map(|(kind, lists)| listed(&words(&[*kind, 0]), None, lists)).collect::<Vec<_>>());
        [words(&[1, 0, 10, 10 + scripts.len() as i32, (10 + scripts.len() + features.len()) as i32]), scripts, features, lookups].concat()
    }

    /// A PairPos format 1 subtable: the first glyphs, the value formats (two bit sets), and per
    /// first glyph its pair count, then each pair with its values.
    pub(crate) fn by_seconds(firsts: &[i32], numbers: [i32; 2], seconds: &[&[i32]]) -> Vec<u8> {
        let lead = 10 + seconds.len() * 2;
        let starts: Vec<i32> = (0..seconds.len()).map(|n| (lead + seconds[..n].concat().len() * 2) as i32).collect();
        let firsts_at = lead + seconds.concat().len() * 2;
        [words(&[1, firsts_at as i32, numbers[0], numbers[1], seconds.len() as i32]), words(&starts), words(&seconds.concat()), words(firsts)].concat()
    }

    /// A PairPos format 2 subtable: first glyphs, value formats, first and second class
    /// definitions, the class counts, and the values per class pair.
    fn by_classes(firsts: &[i32], numbers: [i32; 2], classes: [&[i32]; 2], counts: [i32; 2], pairs: &[i32]) -> Vec<u8> {
        let firsts_at = 16 + pairs.len() * 2;
        let classes_at = [firsts_at + firsts.len() * 2, firsts_at + firsts.len() * 2 + classes[0].len() * 2];
        let lead = words(&[2, firsts_at as i32, numbers[0], numbers[1], classes_at[0] as i32, classes_at[1] as i32, counts[0], counts[1]]);
        [lead, words(pairs), words(firsts), words(classes[0]), words(classes[1])].concat()
    }

    /// The test font's tables plus a `GPOS` kerning every script by these lookups.
    fn kerned_by(lookups: &[(i32, Vec<Vec<u8>>)]) -> Tables {
        let all: Vec<i32> = (0..lookups.len() as i32).collect();
        [tables(), vec![(*b"GPOS", positions(&[(b"DFLT", &[0])], &[(b"kern", &all)], lookups))]].concat()
    }

    /// The font's kerning of some pairs, in its units.
    fn kerning<const N: usize>(tables: &Tables, pairs: [&str; N]) -> [f32; N] {
        let (font, letter) = (read(tables), |pair: &str, n: usize| pair.chars().nth(n).unwrap());
        pairs.map(|pair| font.kerning(letter(pair, 0), letter(pair, 1), 1024.0))
    }

    #[test]
    fn the_newer_table_lists_pairs_by_their_second_glyph() {
        // "A" (glyph 1) kerns with "B" and "C", and "B" with "A"; first glyphs as format 1
        // coverage, one value per pair: x advance.
        let plain = by_seconds(&[1, 2, 1, 2], [4, 0], &[&[2, 2, -100, 3, -50], &[1, 1, 30]]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![plain.clone()])]), ["AB", "AC", "BA", "AA", "AD", "CA", " A"]), [-100.0, -50.0, 30.0, 0.0, 0.0, 0.0, 0.0]);
        // With more values per pair, the x advance is found among them: here after a placement
        // for the first glyph, before one for the second.
        let more = by_seconds(&[2, 1, 1, 3, 0], [5, 2], &[&[1, 2, 7, -100, 9], &[2, 1, 7, 30, 9, 5, 7, -5, 9], &[0]]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![more])]), ["AB", "BA", "BD", "BB", "CA"]), [-100.0, 30.0, -5.0, 0.0, 0.0]);
        // A subtable without the first glyph's x advance still has its pairs, and says nothing.
        let other = by_seconds(&[1, 1, 1], [1, 4], &[&[1, 2, 7, -100]]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![other, plain.clone()])]), ["AB", "AC"]), [0.0, -50.0]);
        // An extension lookup (type 9) only points to the real subtable.
        let far = [words(&[1, 2]), longs(&[8]), plain].concat();
        assert_eq!(kerning(&kerned_by(&[(9, vec![far.clone()])]), ["AB", "BA"]), [-100.0, 30.0]);
        // Other subtable types are skipped, in their own lookup type or via extension: here a
        // single adjustment, type 1. (And `kern` isn't asked.)
        let single = words(&[1, 6, 0, 1, 1, 1]);
        let lookups = [(1, vec![single.clone()]), (9, vec![[words(&[1, 1]), longs(&[8]), single].concat()])];
        assert_eq!(kerning(&kerned_by(&lookups), ["AB", "BA"]), [0.0, 0.0]);
    }

    #[test]
    fn the_newer_table_lists_pairs_by_classes() {
        // First glyphs "A" to "C" as a range. First classes: "A" and "C" 1, "B" 2 (format 1,
        // from "A"); second classes: "A" and "B" 1, "D" and "E" 2 (ranges). Unnamed glyphs are
        // class 0, which has values too.
        let classes: [&[i32]; 2] = [&[1, 1, 3, 1, 2, 1], &[2, 2, 1, 2, 1, 5, 6, 2]];
        let list = by_classes(&[2, 1, 1, 3, 0], [4, 0], classes, [3, 3], &[1, 2, 3, 5, -10, -20, 6, -30, -40]);
        let pairs = ["AA", "AB", "AD", "AE", "BB", "BE", "CA", "AC", "BH", "DA", " A"];
        assert_eq!(kerning(&kerned_by(&[(2, vec![list])]), pairs), [-10.0, -10.0, -20.0, -20.0, -30.0, -40.0, -10.0, 5.0, 6.0, 0.0, 0.0]);
        // The reverse: first glyphs listed singly, their classes as ranges, second classes per
        // glyph; two values per pair, the second the x advance.
        let classes: [&[i32]; 2] = [&[2, 1, 2, 2, 1], &[1, 5, 2, 1, 1]];
        let list = by_classes(&[1, 2, 1, 2], [6, 0], classes, [2, 2], &[9, 1, 9, 2, 9, 3, 9, -60]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![list])]), ["AD", "BD", "BE", "BA", "DD"]), [2.0, -60.0, -60.0, 3.0, 0.0]);
        // A subtable claiming fewer classes than its glyphs use lacks the pairs of the classes
        // beyond.
        let list = by_classes(&[2, 1, 1, 3, 0], [4, 0], [&[1, 1, 3, 1, 2, 1], &[1, 1, 2, 1, 7]], [2, 2], &[0, 0, 0, -10]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![list])]), ["AA", "BA", "AB"]), [-10.0, 0.0, 0.0]);
    }

    #[test]
    fn the_first_list_of_a_lookup_that_has_a_pair_speaks_for_it_and_lookups_add_up() {
        let first = by_seconds(&[1, 1, 1], [4, 0], &[&[1, 2, -100]]);
        let second = by_seconds(&[1, 1, 1], [4, 0], &[&[2, 2, -7, 3, -9]]);
        // "AB" is in both subtables and "AC" only in the second.
        assert_eq!(kerning(&kerned_by(&[(2, vec![first.clone(), second.clone()])]), ["AB", "AC"]), [-100.0, -9.0]);
        // In two lookups, both count. (A lookup listed twice still counts once.)
        assert_eq!(kerning(&kerned_by(&[(2, vec![first.clone()]), (2, vec![second.clone()])]), ["AB", "AC"]), [-107.0, -9.0]);
        let twice = positions(&[(b"DFLT", &[0])], &[(b"kern", &[1, 0, 1, 1])], &[(2, vec![first.clone()]), (2, vec![second.clone()])]);
        assert_eq!(kerning(&[tables(), vec![(*b"GPOS", twice)]].concat(), ["AB", "AC"]), [-107.0, -9.0]);
        // More subtables than any real font has aren't all read: here the first lookup has
        // 2,000, and the second isn't reached.
        let many = read(&kerned_by(&[(2, vec![second.clone(); 2000]), (2, vec![first.clone()])]));
        assert_eq!((many.kerning.scripts[1].len(), many.kerning('A', 'B', 1024.0), many.kerning('A', 'C', 1024.0)), (LISTS, -7.0, -9.0));
        // A class-based subtable covers every pair whose first glyph it covers, including those
        // it kerns by zero: later subtables aren't asked about them.
        let classes = by_classes(&[1, 1, 1], [4, 0], [&[1, 1, 1, 1], &[1, 3, 1, 1]], [2, 2], &[0, 0, 0, -20]);
        assert_eq!(kerning(&kerned_by(&[(2, vec![classes, first, second])]), ["AB", "AC"]), [0.0, -20.0]);
    }

    #[test]
    fn each_script_is_kerned_by_the_lookups_of_its_own_feature() {
        let list = |by: i32| by_seconds(&[1, 1, 1], [4, 0], &[&[1, 2, by]]);
        let lookups = [(2, vec![list(-10)]), (2, vec![list(-20)]), (2, vec![list(-30)])];
        // Latin has its own `kern` feature, Cyrillic has features but no kerning one, and Greek
        // is unknown to the table: it uses the table's any-script entry, as script-less text
        // does.
        let features: [Named; 4] = [(b"kern", &[1]), (b"cpsp", &[2]), (b"kern", &[0, 2]), (b"kern", &[2])];
        let scripts: [Named; 3] = [(b"DFLT", &[1, 0]), (b"cyrl", &[1]), (b"latn", &[1, 2, 3])];
        let font = read(&[tables(), vec![(*b"GPOS", positions(&scripts, &features, &lookups))]].concat());
        let kerned = |left: u8, right: u8| font.closer((1, left), (2, right));
        assert_eq!([kerned(1, 1), kerned(0, 0), kerned(2, 2)], [-40.0, -20.0, -20.0], "Latin by the first `kern` of its script, the others by that of any script");
        assert_eq!([kerned(0, 1), kerned(1, 0), kerned(2, 0)], [-40.0, -40.0, -20.0], "a character of no script goes with its neighbour");
        assert_eq!([kerned(1, 2), kerned(3, 1), kerned(2, 3)], [0.0; 3], "nothing is kerned where two scripts meet");
        // A script `GPOS` doesn't kern is kerned by `kern`; one it does, isn't, whatever `kern`
        // says.
        assert_eq!([kerned(3, 3), font.closer((2, 1), (1, 1)), font.closer((2, 3), (1, 3))], [-128.0, 0.0, 64.0]);
        assert_eq!([font.kerning('A', 'B', 1024.0), font.kerning('B', 'A', 1024.0), font.kerning(' ', 'B', 1024.0)], [-40.0, 0.0, 0.0]);
        // Without a generic script tag, the table's Latin stands for the rest; a table of
        // another major version isn't read, and `kern` kerns alone.
        let only: [Named; 1] = [(b"latn", &[0])];
        let font = read(&[tables(), vec![(*b"GPOS", positions(&only, &features, &lookups))]].concat());
        assert_eq!([0, 1, 2, 3].map(|script| font.closer((1, script), (2, script))), [-20.0; 4]);
        let newer = [words(&[2]), positions(&only, &features, &lookups)[2..].to_vec()].concat();
        let font = read(&[tables(), vec![(*b"GPOS", newer)]].concat());
        assert_eq!([0, 1, 2, 3].map(|script| font.closer((1, script), (2, script))), [-128.0; 4]);
    }

    #[test]
    fn a_character_is_of_the_script_its_letters_are_of() {
        let of = |letters: &str| letters.chars().map(script).collect::<Vec<_>>();
        assert_eq!(of("AZaz\u{c0}\u{ff}\u{141}\u{1e9e}\u{fb01}"), [1; 9], "Latin");
        assert_eq!(of("\u{391}\u{3c9}\u{3ac}\u{1f00}"), [2; 4], "Greek");
        assert_eq!(of("\u{410}\u{44f}\u{401}\u{4d9}"), [3; 4], "Cyrillic");
        assert_eq!(of(" 0.,-(\u{d7}\u{f7}\u{2019}\u{20ac}\u{37e}\u{5d0}\u{4e2d}\u{1f600}"), [0; 14], "no script, or one that is not told apart");
    }

    #[test]
    fn the_first_font_of_a_collection_is_read() {
        // A collection starts with its own header listing its fonts' offsets. Offsets within a
        // font still count from the file's start.
        let front = [&b"ttcf"[..], &longs(&[0x0001_0000, 1, 16])].concat();
        let mut collection = [front.clone(), file(&[0, 1, 0, 0], &tables())].concat();
        for entry in 0..tables().len() {
            let at = front.len() + 12 + entry * 16 + 8;
            let place = u32::from_be_bytes(collection[at..at + 4].try_into().unwrap()) + front.len() as u32;
            collection[at..at + 4].copy_from_slice(&place.to_be_bytes());
        }
        let (single, collection) = (read(&tables()), FontFile::read(collection).expect("a collection is read"));
        "ABCDEFGH x".chars().for_each(|letter| assert_eq!(collection.outline(letter, 64.0), single.outline(letter, 64.0), "{letter:?}"));
        assert!(FontFile::read(Vec::new()).is_none());
    }

    #[test]
    fn text_puts_ink_where_its_letters_are_and_nowhere_else() {
        let (mut fonts, font) = (AppFonts::new(read(&tables())), Font::new(16.0));
        // At 16, a font unit is 1/64 pixel, so these glyphs fill whole pixels: an "A" is six
        // pixels square with a two-pixel hole, on a baseline 13 pixels down.
        let mut px = vec![0u32; 24 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 24, 20, (0, 0), 1.0), "A", 2.0, 1.0, font, Color::WHITE);
        let expected = [block(&[2, 3, 4, 5, 6, 7], 8..10), block(&[2, 3, 6, 7], 10..12), block(&[2, 3, 4, 5, 6, 7], 12..14)].concat();
        assert_eq!(ink(&px, 24), (expected, true));
        // A missing character is notdef's box; a space is just room.
        let mut px = vec![0u32; 24 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 24, 20, (0, 0), 1.0), "x x", 0.0, 0.0, font, Color::WHITE);
        assert_eq!(ink(&px, 24), (block(&[1, 2, 3, 10, 11, 12], 6..13), true));
        // Curves have soft edges, and stay inside their glyph's box.
        let mut px = vec![0u32; 24 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 24, 20, (0, 0), 1.0), "B", 3.0, 2.0, font, Color::WHITE);
        let (lit, whole) = ink(&px, 24);
        assert!(!whole && lit.len() > 30 && lit.iter().all(|at| (3..11).contains(&at.0) && (7..15).contains(&at.1)), "{lit:?}");
        // On a canvas that is a strip of a larger image, clipped to part of it.
        let mut strip = vec![0u32; 24 * 4];
        let mut canvas = Canvas::new(&mut strip, 24, 4, (0, 9), 1.0);
        canvas.clip(rect(0.0, 0.0, 6.0, 20.0));
        fonts.draw(&mut canvas, "A", 2.0, 1.0, font, Color::WHITE);
        assert_eq!(ink(&strip, 24).0, [block(&[2, 3, 4, 5], 0..1), block(&[2, 3], 1..3), block(&[2, 3, 4, 5], 3..4)].concat());
        // In a translucent colour, over what is there.
        let mut px = vec![0xff00_0000u32; 24 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 24, 20, (0, 0), 1.0), "A", 2.0, 1.0, font, Color::WHITE.with_alpha(0.5));
        assert!((0x7e..=0x80).contains(&(px[8 * 24 + 2] & 0xff)) && px[10 * 24 + 4] == 0xff00_0000, "{:08x}", px[8 * 24 + 2]);
    }

    #[test]
    fn letters_the_file_lacks_are_left_to_another_writer() {
        // The font has "A" (7 pixels wide at this size) and "B" (9), and no lowercase. Alone it
        // draws its notdef box in their place, 5 wide.
        let (font, mut stops) = (Font::new(16.0), Vec::new());
        let mut alone = AppFonts::new(read(&tables()));
        assert_eq!(alone.measure("AzB", font, 1.0, None), 21.0);
        // With a fallback for what it lacks (here the built-in font, 12 wide at this size), the
        // "z" is the fallback's: its width, drawn by it, between the font's two glyphs. Each
        // character's end counts from the line start.
        let mut fonts = AppFonts::new(read(&tables())).fallback(Box::new(crate::text::BuiltInFont));
        assert_eq!(fonts.measure("AzB", font, 1.0, Some(&mut stops)), 28.0);
        assert_eq!(stops, [7.0, 19.0, 28.0]);
        assert_eq!(fonts.measure("zz", font, 1.0, None), 24.0);
        // Monospace, with no file from the app, is the fallback's entirely: a proportional font
        // can't stand in for it.
        assert_eq!((fonts.measure("AB", font, 1.0, None), fonts.measure("AB", font.mono(), 1.0, None), alone.measure("AB", font.mono(), 1.0, None)), (14.0, 24.0, 14.0));
        let ink = |fonts: &mut AppFonts, text: &str| {
            let mut px = vec![0u32; 40 * 24];
            fonts.draw(&mut Canvas::new(&mut px, 40, 24, (0, 0), 1.0), text, 2.0, 2.0, font, Color::WHITE);
            (0..40).filter(|x| (0..24).any(|y| px[y * 40 + x] != 0)).collect::<Vec<usize>>()
        };
        // "A z" with the font's space: the "z" is drawn where the pen is after both.
        let (with, without) = (ink(&mut fonts, "AzB"), ink(&mut fonts, "A"));
        assert!(with.iter().any(|x| (9..21).contains(x)) && !without.iter().any(|x| (9..21).contains(x)), "{with:?} {without:?}");
        assert!(with.iter().any(|x| *x >= 21), "the B comes after the other writer's letter: {with:?}");
    }

    #[test]
    fn measuring_goes_by_whole_pixels_and_agrees_with_drawing() {
        let (mut fonts, font) = (AppFonts::new(read(&tables())), Font::new(16.0));
        assert_eq!(fonts.metrics(font, 1.0), Metrics { ascent: 13.0, descent: 3.0, cap_height: (CAP / 64.0).round(), x_height: 0.0 });
        // "A" is 7 pixels wide and "B" 9; "AB" is 2 closer and "BA" 1 apart. (Shaped, half of
        // a pair's kerning goes to each glyph, as HarfBuzz has it for this older table.)
        let (mut stops, first) = (Vec::new(), if cfg!(feature = "shaping") { 6.0 } else { 7.0 });
        assert_eq!(fonts.measure("AB BA", font, 1.0, Some(&mut stops)), 35.0);
        assert_eq!(stops, [first, 14.0, 18.0, 27.0, 35.0]);
        assert_eq!(fonts.measure("", font, 1.0, Some(&mut stops)), 0.0);
        assert_eq!(stops.len(), 5, "nothing is added to the list for no text");
        // The last "A" is drawn where measuring puts it: its box ends a pixel before its
        // advance.
        let mut px = vec![0u32; 48 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 48, 20, (0, 0), 1.0), "AB BA", 0.0, 0.0, font, Color::WHITE);
        assert_eq!(ink(&px, 48).0.iter().map(|at| at.0).max(), Some(35 - 1 - 1));
        // On a denser screen everything rounds to its pixels. At 10 units and 1.5 pixels per
        // unit "A" is 6.56 pixels wide, so 7; the pair is 1.88 closer, so 2; and "B" is 8.44,
        // so 8.
        let (mut stops, small) = (Vec::new(), Font::new(10.0));
        assert_eq!(fonts.measure("AB", small, 1.5, Some(&mut stops)), 13.0 / 1.5);
        assert_eq!(stops, [first / 1.5, 13.0 / 1.5]);
        assert_eq!(fonts.metrics(small, 1.5), Metrics { ascent: 8.0, descent: 2.0, cap_height: (CAP * 15.0 / 1024.0).round() / 1.5, x_height: 0.0 });
        // Drawn 2 units in, "A" starts 3 pixels in and "B" 5 further, 7.5 pixels wide and high;
        // baseline 12 pixels down.
        let mut px = vec![0u32; 48 * 20];
        fonts.draw(&mut Canvas::new(&mut px, 48, 20, (0, 0), 1.5), "AB", 2.0, 0.0, small, Color::WHITE);
        let (lit, _) = ink(&px, 48);
        let (across, down) = (lit.iter().map(|at| at.0), lit.iter().map(|at| at.1));
        assert_eq!([across.clone().min(), across.max(), down.clone().min(), down.max()], [Some(3), Some(3 + 5 + 8 - 1), Some(12 - 8), Some(12 - 1)]);
    }

    #[test]
    #[cfg(feature = "shaping")]
    fn a_line_is_shaped_once_for_every_size() {
        let (mut fonts, font) = (AppFonts::new(read(&tables())), Font::new(16.0));
        let text = "AB BA ABBA";
        let wide = fonts.measure(text, font, 1.0, None);
        let (runs, kept) = (fonts.shaped.runs.len(), [fonts.cache_bytes, fonts.shaped.bytes]);
        assert_eq!([fonts.measure(text, font, 1.0, None), fonts.measure(text, font, 1.0, None)], [wide; 2]);
        assert_eq!((runs, fonts.shaped.runs.len(), [fonts.cache_bytes, fonts.shaped.bytes]), (1, 1, kept));
        // A run is kept in its text, 16 bytes a glyph, and its record.
        assert_eq!(kept[1], text.len() + 10 * 16 + size_of::<(usize, Box<str>, Box<[Placed]>)>());
        // Another size shapes nothing again; another text does.
        fonts.measure(text, Font::new(20.0), 1.5, None);
        assert_eq!(fonts.shaped.runs.len(), 1);
        fonts.measure("BBB", font, 1.0, None);
        assert_eq!(fonts.shaped.runs.len(), 2);
    }

    #[test]
    #[cfg(not(feature = "shaping"))]
    fn the_pairs_of_a_text_are_remembered_however_many_they_are() {
        let (file, mut fonts, font) = (read(&tables()), AppFonts::new(read(&tables())), Font::new(16.0));
        // Every pair of thirty characters: "A" and "B", kerned with each other, and others the
        // font lacks, all the same glyph but each its own character.
        let letters: Vec<char> = "AB".chars().chain('a'..='z').chain("12".chars()).collect();
        let text: String = letters.iter().flat_map(|left| letters.iter().map(move |right| [*left, *right])).flatten().collect();
        let pairs: Vec<(char, char)> = text.chars().zip(text.chars().skip(1)).collect();
        let wide: f32 = text.chars().map(|letter| file.advance(letter, 16.0).round()).sum::<f32>() + pairs.iter().map(|(left, right)| file.kerning(*left, *right, 16.0).round()).sum::<f32>();
        assert!(pairs.iter().filter(|pair| **pair == ('A', 'B')).count() > 1 && pairs.contains(&('B', 'A')));
        // The first time, every pair is asked of the font; after that, none.
        let kept = (fonts.measure(&text, font, 1.0, None), fonts.cache_bytes).1;
        assert_eq!([fonts.measure(&text, font, 1.0, None), fonts.measure(&text, font, 1.0, None)], [wide; 2]);
        let distinct = pairs.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!((fonts.drawn[0].met, fonts.drawn[0].pairs.len(), fonts.cache_bytes), (distinct, 2048, kept), "{distinct} pairs");
        // The pair table's memory is counted with the glyphs.
        assert!(kept > 2048 * size_of::<(u32, f32)>() && kept < 2048 * size_of::<(u32, f32)>() + 12000, "{kept}");
        // A font without kerning caches no pairs.
        let plain: Tables = tables().into_iter().filter(|(name, _)| name != b"kern").collect();
        let mut fonts = AppFonts::new(read(&plain));
        assert_eq!((fonts.measure(&text, font, 1.0, None) > 0.0, fonts.drawn[0].pairs.len()), (true, 0));
    }

    #[test]
    fn each_face_is_written_with_its_file_or_with_the_regular_one() {
        // A second file, in which "A" is wider.
        let wider = || read(&with(tables(), b"hmtx", words(&[320, 64, 1024, 0, 576, 0, 640, 64, 256, 0, 640, 32, 384, 0, 96, 64, 999])));
        let (mut fonts, font) = (AppFonts::new(read(&tables())).bold(wider()), Font::new(16.0));
        assert_eq!([font, font.bold(), font.mono()].map(|font| fonts.measure("A", font, 1.0, None)), [7.0, 16.0, 7.0]);
        assert_eq!(fonts.drawn.len(), 2, "a face without a file shares the letters of the regular one");
        assert_eq!(fonts.mono(wider()).measure("A", font.mono(), 1.0, None), 16.0);
    }

    #[test]
    fn large_letters_are_filled_and_what_is_kept_stays_small() {
        let mut fonts = AppFonts::new(read(&tables()));
        // At 1024 an "A" is 384 pixels square: too large to cache as coverage.
        let mut px = vec![0u32; 400 * 400];
        fonts.draw(&mut Canvas::new(&mut px, 400, 400, (0, 0), 1.0), "A", 4.0, -440.0, Font::new(1024.0), Color::WHITE);
        let at = |x: usize, y: usize| px[y * 400 + x] != 0;
        assert_eq!(
            [at(3, 20), at(4, 8), at(131, 135), at(132, 136), at(259, 263), at(260, 264), at(387, 391), at(388, 391), at(387, 392)],
            [false, true, true, false, false, true, true, false, false]
        );
        assert_eq!(fonts.measure("A", Font::new(1024.0), 1.0, None), 448.0);
        assert!(fonts.cache_bytes < 2000, "{} bytes are kept for a letter that is not kept", fonts.cache_bytes);
        // At an invalid or huge size, nothing is drawn and nothing panics.
        for size in [0.0, -16.0, 1e9, 1e30, f32::INFINITY, f32::NAN] {
            fonts.draw(&mut Canvas::new(&mut px, 400, 400, (0, 0), 1.0), "AB C", 4.0, 4.0, Font::new(size), Color::WHITE);
            let _ = (fonts.measure("AB C", Font::new(size), 1.5, None), fonts.metrics(Font::new(size), 1.5));
        }
        assert_eq!(read(&tables()).outline('A', 1e9), Path::new(), "a shape a million times too large is no shape");
        // Glyphs at ever new sizes don't pile up without end.
        let most = (0..120).map(|size| (fonts.measure("AB C", Font::new(300.0 + size as f32), 1.0, None), fonts.cache_bytes).1).max().unwrap();
        assert!(most > CACHE_LIMIT / 2 && most < CACHE_LIMIT + 4 * LARGEST as usize, "{most}");
    }

    /// Bytes as a zlib stream, simply: one fixed-Huffman block, where runs of 3 to 10 bytes
    /// equal to those 1 to 4 bytes back are back-referenced. (Huffman codes go in MSB first;
    /// bits fill each byte from its LSB.)
    fn packed(bytes: &[u8]) -> Vec<u8> {
        let (mut bits, mut at) = (vec![true, true, false], 0);
        let code = |bits: &mut Vec<bool>, code: usize, long: usize| bits.extend((0..long).rev().map(|bit| code >> bit & 1 == 1));
        while at < bytes.len() {
            let same = |back: usize| (0..10).take_while(|n| at >= back && bytes.get(at + n) == bytes.get(at + n - back)).count();
            match ((1..=4).map(|back| (same(back), back)).max().unwrap(), bytes[at] as usize) {
                ((long @ 3.., back), _) => {
                    code(&mut bits, long - 2, 7);
                    code(&mut bits, back - 1, 5);
                    at += long - 1;
                }
                (_, byte @ ..144) => code(&mut bits, 0x30 + byte, 8),
                (_, byte) => code(&mut bits, 0x190 + byte - 144, 9),
            }
            at += 1;
        }
        // End of block, and after the last whole byte the Adler-32 checksum.
        code(&mut bits, 0, 7);
        let data: Vec<u8> = bits.chunks(8).map(|byte| byte.iter().rev().fold(0, |sum, bit| sum << 1 | *bit as u8)).collect();
        let (low, high) = bytes.iter().fold((1, 0), |(low, high), byte| ((low + *byte as u32) % 65521, (high + low + *byte as u32) % 65521));
        [&[0x78, 0x01][..], &data, &(high << 16 | low).to_be_bytes()].concat()
    }

    /// A font file as WOFF: each table compressed, or stored as is where compression wouldn't
    /// shrink it. Also returns how many tables are compressed.
    fn for_the_web(whole: &[u8]) -> (Vec<u8>, usize) {
        let number = |at: usize| u32::from_be_bytes(whole[at..at + 4].try_into().unwrap()) as usize;
        let entries = whole[12..12 + whole[5] as usize * 16].chunks(16).zip((20..).step_by(16));
        let tables: Vec<(&[u8], &[u8])> = entries.map(|(entry, at)| (&entry[..4], &whole[number(at)..number(at) + number(at + 4)])).collect();
        let lying: Vec<Vec<u8>> = tables.iter().map(|(_, table)| Some(packed(table)).filter(|packed| packed.len() < table.len()).unwrap_or(table.to_vec())).collect();
        let mut file = [&b"wOFF"[..], &whole[..4], &[0; 4], &whole[4..6], &[0; 30]].concat();
        let mut at = file.len() + tables.len() * 20;
        for ((name, table), lying) in tables.iter().zip(&lying) {
            file.extend([*name, &longs(&[at as u32, lying.len() as u32, table.len() as u32, 0])].concat());
            at += lying.len();
        }
        lying.iter().for_each(|table| file.extend(table));
        (file, tables.iter().zip(&lying).filter(|((_, table), lying)| lying.len() < table.len()).count())
    }

    #[test]
    fn a_font_packed_for_the_web_is_the_font_it_was() {
        let classes = by_classes(&[1, 1, 1], [4, 0], [&[1, 1, 1, 1], &[1, 2, 1, 1]], [2, 2], &[0, 0, 0, -20]);
        // A PostScript-outline font only where those are read: see the feature `cff`.
        let wholes = [Some(file(&[0, 1, 0, 0], &kerned_by(&[(2, vec![classes])]))), cfg!(feature = "cff").then(|| curved(None))];
        for whole in wholes.into_iter().flatten() {
            // Some tables are compressed, some too short to benefit.
            let (web, packed) = for_the_web(&whole);
            assert!(packed >= 2 && packed < whole[5] as usize, "{packed} tables are packed");
            let (font, same) = (FontFile::read(whole).expect("the font is read"), FontFile::read(web.clone()).expect("the packed font is read"));
            for letter in " ABCDEFGH@".chars() {
                assert_eq!((same.outline(letter, 64.0), same.advance(letter, 64.0)), (font.outline(letter, 64.0), font.advance(letter, 64.0)), "{letter:?}");
                assert_eq!(same.kerning('A', letter, 1024.0), font.kerning('A', letter, 1024.0), "{letter:?}");
            }
            assert_eq!(same.metrics(64.0), font.metrics(64.0));
            // A table not decompressing to its stated length: no font. (12 bytes before the end
            // of the first table's entry give its length.)
            let mut longer = web.clone();
            longer[44 + 15] ^= 1;
            assert!(FontFile::read(longer).is_none());
            // Nor one holding another file type, or truncated.
            let mut other = web.clone();
            other[4..8].copy_from_slice(b"wOF2");
            assert!(FontFile::read(other).is_none() && FontFile::read(web[..web.len() - 1].to_vec()).is_none());
        }
        // A file claiming WOFF2 that isn't is no font (nor, without the feature `woff2`, is one
        // that is).
        assert!(FontFile::read(b"wOF2, which is packed in a way that takes a long list of words to unpack".to_vec()).is_none());
    }

    /// A variable font in WOFF used to be decompressed without its axes, variations and style
    /// names (only static fonts' tables were taken): whatever its instance, it drew at the
    /// default.
    #[test]
    fn a_variable_font_packed_for_the_web_varies_as_the_font_it_was() {
        let whole = crate::variable_fonts::tests::variable(&[]);
        let web = for_the_web(&whole).0;
        let read = |file: &[u8]| FontFile::read(file.to_vec()).expect("the font is read");
        assert!(!read(&whole).axes().is_empty());
        assert_eq!((read(&web).axes(), read(&web).faces()), (read(&whole).axes(), read(&whole).faces()));
        let (most, same) = (read(&whole).with_variation(b"wght", 900.0), read(&web).with_variation(b"wght", 900.0));
        let bold = [read(&whole).named("Bold"), read(&web).named("Bold")].map(|font| font.expect("a style by its name").outline('A', 64.0));
        assert_eq!(bold[0], bold[1]);
        for letter in "ABCDH ".chars() {
            assert_eq!((same.outline(letter, 64.0), same.advance(letter, 64.0)), (most.outline(letter, 64.0), most.advance(letter, 64.0)), "{letter:?}");
        }
        assert_ne!(most.outline('A', 64.0), read(&whole).outline('A', 64.0));
    }

    /// Everything a font can be asked, asked of whatever these bytes are.
    pub(crate) fn ask_everything(bytes: Vec<u8>) {
        let Some(font) = FontFile::read(bytes) else { return };
        let letters = " ABCDEFGH@x\u{1F600}";
        for letter in letters.chars() {
            let _ = (font.advance(letter, 16.0), font.kerning(letter, 'B', 16.0), font.kerning('A', letter, 16.0), font.outline(letter, 16.0), font.metrics(16.0));
        }
        let mut fonts = AppFonts::new(font);
        // At the second size, glyphs are too large to cache, and are rasterized.
        for (size, scale) in [(11.0, 1.5), (2000.0, 2.0)] {
            let (mut px, mut stops) = (vec![0u32; 64 * 32], Vec::new());
            let wide = fonts.measure(letters, Font::new(size), scale, Some(&mut stops));
            assert_eq!((stops.len(), stops.last().copied()), (letters.chars().count(), Some(wide)));
            fonts.draw(&mut Canvas::new(&mut px, 64, 32, (0, 0), scale), letters, 1.0, 1.0, Font::new(size), Color::WHITE);
        }
    }

    /// A font with `GPOS` kerning (both subtable formats, one via extension, and its own Latin
    /// script), and a glyph with a component positioned by matching points.
    fn kerned_and_laid() -> Vec<u8> {
        let seconds = by_seconds(&[1, 2, 1, 2], [5, 2], &[&[1, 2, 7, -100, 9], &[1, 1, 7, 30, 9]]);
        let classes = by_classes(&[2, 1, 1, 3, 0], [4, 0], [&[1, 1, 3, 1, 2, 1], &[2, 2, 1, 2, 1, 5, 6, 2]], [3, 3], &[1, 2, 3, 5, -10, -20, 6, -30, -40]);
        let far = [words(&[1, 2]), longs(&[8]), by_seconds(&[1, 1, 1], [4, 0], &[&[1, 2, -100]])].concat();
        let (scripts, features): ([Named; 2], [Named; 2]) = ([(b"DFLT", &[0]), (b"latn", &[1])], [(b"kern", &[0, 1]), (b"kern", &[1])]);
        let kerning = positions(&scripts, &features, &[(2, vec![seconds, classes]), (9, vec![far])]);
        let glyphs = with_parts(&[(0, [0, 0], [1.0, 0.0, 0.0, 1.0], 0), (1, [2, 0], [0.5, 0.0, 0.0, 0.5], 2)]);
        file(&[0, 1, 0, 0], &[tables_of(glyphs), vec![(*b"GPOS", kerning)]].concat())
    }

    #[test]
    #[cfg(not(feature = "cff"))]
    fn without_the_feature_a_font_of_postscript_curves_is_none() {
        // Plainly: no font, so the app keeps system text. A TrueType font reads as usual.
        assert!(FontFile::read(curved(None)).is_none() && FontFile::read(summed_and_accented()).is_none());
        assert!(FontFile::read(file(&[0, 1, 0, 0], &tables())).is_some());
    }

    #[test]
    fn a_damaged_font_never_stops_the_program() {
        let curves = [curved(None), curved(Some(vec![3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 4])), summed_and_accented()];
        let wholes = [file(&[0, 1, 0, 0], &tables()), kerned_and_laid(), for_the_web(&kerned_and_laid()).0];
        // PostScript-outline fonts only where those are read: see the feature `cff`.
        for whole in wholes.into_iter().chain(curves.into_iter().filter(|_| cfg!(feature = "cff"))) {
            // Whole, each is a font whose glyphs all have outlines.
            let font = FontFile::read(whole.clone()).expect("the font is read");
            assert!("ABCD".chars().all(|letter| font.outline(letter, 16.0) != Path::new()));
            // Truncated anywhere;
            (0..whole.len()).for_each(|long| ask_everything(whole[..long].to_vec()));
            // or with any one byte set to 0, to 255, or with a bit flipped (another bit per
            // byte).
            for (at, change) in (0..whole.len()).flat_map(|at| [(at, 0), (at, 255), (at, whole[at] ^ 1 << (at % 8))]) {
                let mut damaged = whole.clone();
                damaged[at] = change;
                ask_everything(damaged);
            }
        }
    }

    /// The font at `path`, if it exists and is the very file (by its length) the values below
    /// came from.
    fn known(path: &str, long: usize) -> Option<FontFile> {
        let file = std::fs::read(path).ok().filter(|file| file.len() == long)?;
        Some(FontFile::read(file).expect("a font of the machine is read"))
    }

    #[test]
    fn fonts_of_the_machine_are_read_as_freetype_reads_them() {
        // What FreeType 2.14 reports of two widespread fonts, in font units. A machine without
        // the font, or with another version, is skipped.
        let numbers = |font: &FontFile| {
            let widths = ['-', 'I', 'V', '\u{e9}'].map(|letter| font.advance(letter, 2048.0));
            (font.metrics(2048.0), widths, [('A', 'V'), ('T', 'o'), ('o', 'T')].map(|(left, right)| font.kerning(left, right, 2048.0)))
        };
        if let Some(font) = known("C:/Windows/Fonts/arial.ttf", 1_097_456) {
            assert_eq!(numbers(&font), (Metrics { ascent: 1854.0, descent: 434.0, cap_height: 1466.0, x_height: 1062.0 }, [682.0, 569.0, 1366.0, 1139.0], [-152.0, -227.0, 0.0]));
            assert_eq!(font.outline('I', 2048.0), Path::svg("M191 1854 191 388 385 388 385 1854Z"));
            // `hmtx` puts one glyph a unit left of its points.
            assert_eq!(font.sketch(font.glyph('\u{149}'), Placing::sized(1.0, 0.0)).reach, [-39.0, -1481.0, 1095.0, 0.0]);
        }
        if let Some(font) = known("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 759_720) {
            assert_eq!(numbers(&font), (Metrics { ascent: 1901.0, descent: 483.0, cap_height: 1493.0, x_height: 1120.0 }, [739.0, 604.0, 1401.0, 1260.0], [-131.0, -348.0, 0.0]));
            assert_eq!(font.outline('I', 2048.0), Path::svg("M201 408 403 408 403 1901 201 1901Z"));
        }
    }

    /// A 16-bit offset array followed by what it points to: `head` gets each item's offset
    /// (from the start of it all) and returns the array.
    fn behind(head: impl Fn(&[i32]) -> Vec<i32>, things: &[Vec<u8>]) -> Vec<u8> {
        let lead = head(&vec![0; things.len()]).len() * 2;
        let starts: Vec<i32> = (0..things.len()).map(|n| (lead + things[..n].concat().len()) as i32).collect();
        [words(&head(&starts)), things.concat()].concat()
    }

    /// A coverage table listing these glyphs (format 1).
    fn cover(glyphs: &[i32]) -> Vec<u8> {
        words(&[&[1, glyphs.len() as i32], glyphs].concat())
    }

    /// A class definition as ranges (format 2): first and last glyph, and class.
    fn classed(stretches: &[[i32; 3]]) -> Vec<u8> {
        words(&[&[2, stretches.len() as i32][..], &stretches.concat()].concat())
    }

    /// A single substitution subtable: by delta, or by listing each replacement.
    fn one_added(glyphs: &[i32], add: i32) -> Vec<u8> {
        behind(|at| vec![1, at[0], add], &[cover(glyphs)])
    }
    pub(crate) fn one_for_one(glyphs: &[i32], new: &[i32]) -> Vec<u8> {
        behind(|at| [&[2, at[0], new.len() as i32], new].concat(), &[cover(glyphs)])
    }

    /// A multiple substitution subtable: a glyph sequence replacing each covered glyph.
    fn several_for_one(glyphs: &[i32], rows: &[&[i32]]) -> Vec<u8> {
        let rows = rows.iter().map(|row| words(&[&[row.len() as i32], *row].concat()));
        let things: Vec<Vec<u8>> = std::iter::once(cover(glyphs)).chain(rows).collect();
        behind(|at| [&[1, at[0], things.len() as i32 - 1], &at[1..]].concat(), &things)
    }

    /// A ligature substitution subtable: per first glyph, its ligatures, each with its
    /// component glyphs.
    fn one_for_several(firsts: &[i32], sets: &[&[(i32, &[i32])]]) -> Vec<u8> {
        let set = |ligatures: &&[(i32, &[i32])]| {
            let each: Vec<Vec<u8>> = ligatures.iter().map(|(glyph, rest)| words(&[&[*glyph, rest.len() as i32 + 1], *rest].concat())).collect();
            behind(|at| [&[each.len() as i32], at].concat(), &each)
        };
        let things: Vec<Vec<u8>> = std::iter::once(cover(firsts)).chain(sets.iter().map(set)).collect();
        behind(|at| [&[1, at[0], sets.len() as i32], &at[1..]].concat(), &things)
    }

    /// A chained context format 3 subtable: backtrack glyphs (nearest first), input glyphs from
    /// the first, lookahead; and its lookup records, each with its sequence index.
    fn one_rule(before: &[&[i32]], of: &[&[i32]], after: &[&[i32]], lookups: &[[i32; 2]]) -> Vec<u8> {
        let things: Vec<Vec<u8>> = before.iter().chain(of).chain(after).map(|glyphs| cover(glyphs)).collect();
        let head = |at: &[i32]| {
            let ((before, rest), count) = (at.split_at(before.len()), |row: &[i32]| [row.len() as i32]);
            let (of, after) = rest.split_at(of.len());
            [&[3][..], &count(before), before, &count(of), of, &count(after), after, &[lookups.len() as i32], &lookups.concat()].concat()
        };
        behind(head, &things)
    }

    /// A context or chained context subtable of rule sets, in format 1 or 2: its coverage, its
    /// class definitions (if class-based), and per first glyph or class its rules, each an
    /// array as the font stores it.
    fn sets_of_rules(way: i32, glyphs: &[i32], classes: &[Vec<u8>], sets: &[&[&[i32]]]) -> Vec<u8> {
        let set = |rules: &&[&[i32]]| {
            let each: Vec<Vec<u8>> = rules.iter().map(|rule| words(rule)).collect();
            behind(|at| [&[each.len() as i32], at].concat(), &each)
        };
        let things: Vec<Vec<u8>> = std::iter::once(cover(glyphs)).chain(classes.iter().cloned()).chain(sets.iter().map(set)).collect();
        behind(|at| [&[way], &at[..1 + classes.len()], &[sets.len() as i32], &at[1 + classes.len()..]].concat(), &things)
    }

    /// A single adjustment subtable: its value format (a bit per value), and the values, once
    /// for all glyphs or one each.
    fn moving(glyphs: &[i32], said: i32, numbers: &[i32]) -> Vec<u8> {
        behind(|at| [&[1, at[0], said], numbers].concat(), &[cover(glyphs)])
    }
    fn moving_each(glyphs: &[i32], said: i32, numbers: &[i32]) -> Vec<u8> {
        behind(|at| [&[2, at[0], said, glyphs.len() as i32], numbers].concat(), &[cover(glyphs)])
    }

    /// A mark-to-base subtable: marks (each with class and anchor), and bases (each with an
    /// anchor per class).
    fn fixing(marks: &[(i32, i32, [i32; 2])], onto: &[(i32, &[[i32; 2]])]) -> Vec<u8> {
        let point = |at: &[i32; 2]| words(&[1, at[0], at[1]]);
        let own: Vec<Vec<u8>> = marks.iter().map(|(_, _, at)| point(at)).collect();
        let own = behind(|at| std::iter::once(marks.len() as i32).chain(marks.iter().zip(at).flat_map(|(mark, at)| [mark.1, *at])).collect(), &own);
        let theirs: Vec<Vec<u8>> = onto.iter().flat_map(|(_, points)| points.iter().map(point)).collect();
        let theirs = behind(|at| [&[onto.len() as i32], at].concat(), &theirs);
        let (marks, glyphs): (Vec<i32>, Vec<i32>) = (marks.iter().map(|mark| mark.0).collect(), onto.iter().map(|other| other.0).collect());
        behind(|at| vec![1, at[0], at[1], onto[0].1.len() as i32, at[2], at[3]], &[cover(&marks), cover(&glyphs), own, theirs])
    }

    /// A `GSUB` or `GPOS` table for script-less text: its features, each tag and lookups; and
    /// its lookups, each type, lookup flags, and subtables.
    fn sequences(features: &[Named], lookups: &[Lookup]) -> Vec<u8> {
        let (all, names): (Vec<i32>, Vec<&[u8; 4]>) = ((0..features.len() as i32).collect(), features.iter().map(|(name, _)| *name).collect());
        let scripts = listed(&[], Some(&[b"DFLT"]), &[words(&[&[4, 0, 0, 0xffff, all.len() as i32], &all[..]].concat())]);
        let features = listed(&[], Some(&names), &features.iter().map(|(_, lookups)| words(&[&[0, lookups.len() as i32], *lookups].concat())).collect::<Vec<_>>());
        let lookups = listed(&[], None, &lookups.iter().map(|(kind, passed, lists)| listed(&words(&[*kind, *passed]), None, lists)).collect::<Vec<_>>());
        [words(&[1, 0, 10, 10 + scripts.len() as i32, (10 + scripts.len() + features.len()) as i32]), scripts, features, lookups].concat()
    }

    /// A `cmap` format 12 subtable: groups, each first and last character and first glyph.
    pub(crate) fn mapped(stretches: &[[u32; 3]]) -> Vec<u8> {
        [words(&[0, 1, 3, 10, 0, 12, 12, 0, 0, 0, 0, 0, 0, stretches.len() as i32]), longs(&stretches.concat())].concat()
    }

    /// The test emoji font's four colours: yellow, black, red, and a half-transparent blue.
    const COLOURS: [u32; 4] = [0xffff_cc00, 0xff00_0000, 0xffe0_0000, 0x8000_00ff];

    /// The test font's tables as an emoji font: with some emoji among its characters, three
    /// layered glyphs, and any further tables. "A" (also a smiling face, a man, and the first
    /// regional indicator) is a yellow box under a small black one; "D" is a red "A" under a
    /// small box in the text colour; "H" is a box in half-transparent blue.
    pub(crate) fn coloured(more: &[(&[u8; 4], Vec<u8>)]) -> Tables {
        // A digit, the letters, ZWJ, the keycap mark, a heart, three regional indicators, a
        // skin tone, a hand, a child, a man and a woman, a face.
        let characters = [[0x20, 0x20, 4], [0x31, 0x31, 8], [0x41, 0x43, 1], [0x44, 0x48, 5], [0x200d, 0x200d, 4], [0x20e3, 0x20e3, 7], [0x2764, 0x2764, 8]];
        let emoji = [[0x1f1e6, 0x1f1e8, 1], [0x1f3fd, 0x1f3fd, 7], [0x1f44d, 0x1f44d, 8], [0x1f466, 0x1f466, 8], [0x1f468, 0x1f469, 1], [0x1f600, 0x1f600, 1]];
        // `COLR` v0: three base glyphs (first layer index, layer count), then the layers: glyph
        // and palette index.
        let layered = [words(&[0, 3]), longs(&[14, 32]), words(&[5]), words(&[1, 0, 2, 5, 2, 2, 9, 4, 1]), words(&[0, 0, 8, 1, 1, 2, 8, 0xffff, 0, 3])].concat();
        // `CPAL`: one palette of four colours, starting at the first; then the colours.
        let colours = [words(&[0, 4, 1, 4]), longs(&[14]), words(&[0]), COLOURS.iter().flat_map(|colour| colour.swap_bytes().to_be_bytes()).collect()].concat();
        let mut tables = with(tables(), b"cmap", mapped(&[&characters[..], &emoji[..]].concat()));
        tables.extend([(*b"COLR", layered), (*b"CPAL", colours)]);
        tables.extend(more.iter().map(|(name, table)| (**name, table.clone())));
        tables
    }

    /// A lookup: its type, lookup flags, and subtables.
    type Lookup = (i32, i32, Vec<Vec<u8>>);
    /// A glyph, and its position.
    type Placed = (u16, [f32; 2]);

    /// The glyphs a text becomes in an emoji font with these `GSUB` lookups under `ccmp` and
    /// these `GPOS` lookups under `dist`: each glyph with its position, and the pen's total
    /// advance. (`GPOS` also has an empty `kern` feature, so the font's `kern` table doesn't
    /// apply.)
    fn set(swaps: &[Lookup], moves: &[Lookup], text: &str) -> Option<(Vec<Placed>, f32)> {
        let all = |lookups: usize| (0..lookups as i32).collect::<Vec<_>>();
        let kinds = [words(&[1, 0, 12, 0, 0, 0]), classed(&[[8, 8, 3]])].concat();
        let (swapping, moving) = (sequences(&[(b"ccmp", &all(swaps.len()))], swaps), sequences(&[(b"dist", &all(moves.len())), (b"kern", &[])], moves));
        let font = read(&coloured(&[(b"GSUB", swapping), (b"GPOS", moving), (b"GDEF", kinds)]));
        let mut glyphs = Vec::new();
        let pen = font.set(text, &mut glyphs)?;
        Some((glyphs.iter().map(|set| (set.glyph, set.at)).collect(), pen))
    }

    /// Just the glyphs of that, in a font without `GPOS` lookups.
    fn swapped(swaps: &[Lookup], text: &str) -> Vec<u16> {
        set(swaps, &[], text).expect("the font has the characters").0.iter().map(|(glyph, _)| *glyph).collect()
    }

    #[test]
    fn characters_written_as_one_have_the_glyphs_the_font_puts_in_their_place() {
        // With nothing applied, each character maps to its glyph: "A" 1, "B" 2, "C" 3. A ZWJ is
        // invisible, as is a presentation selector for the previous character; a font lacking a
        // character gives nothing.
        assert_eq!(swapped(&[], "ABC"), [1, 2, 3]);
        assert_eq!(swapped(&[], "A\u{fe0f}\u{200d}B\u{e0067}\u{200c}"), [1, 2]);
        assert_eq!((set(&[], &[], "A\u{3b1}B"), set(&[], &[], "\u{1f601}")), (None, None));
        // Single substitution: by delta, or by list.
        assert_eq!(swapped(&[(1, 0, vec![one_added(&[1, 2], 2)])], "ABC"), [3, 4, 3]);
        assert_eq!(swapped(&[(1, 0, vec![one_for_one(&[1, 3], &[8, 9])])], "ABCA"), [8, 2, 9, 8]);
        // Multiple substitution, or deletion: the "B" is gone.
        assert_eq!(swapped(&[(2, 0, vec![several_for_one(&[1, 2], &[&[5, 5], &[]])])], "ABA"), [5, 5, 5, 5]);
        // Ligature: the first one matching exactly these glyphs.
        let ligatures = one_for_several(&[1], &[&[(7, &[2, 3]), (9, &[2])]]);
        let with = |text: &str| swapped(&[(4, 0, vec![ligatures.clone()])], text);
        assert_eq!([with("ABC"), with("AB"), with("ABAB"), with("AC"), with("BA"), with("A")], [vec![7], vec![9], vec![9, 9], vec![1, 3], vec![2, 1], vec![1]]);
        // A ZWJ with a glyph in the font is a ligature component like any glyph, and a leftover
        // one is still invisible.
        let joined = one_for_several(&[1], &[&[(9, &[4, 2])]]);
        assert_eq!([swapped(&[(4, 0, vec![joined.clone()])], "A\u{200d}B"), swapped(&[(4, 0, vec![joined])], "A\u{200d}C")], [vec![9], vec![1, 3]]);
        // Lookups apply in order, each to the previous result; within a lookup, the first
        // subtable covering a glyph applies.
        let both = [(1, 0, vec![one_added(&[1], 1)]), (4, 0, vec![one_for_several(&[2], &[&[(9, &[2])]])])];
        assert_eq!(swapped(&both, "AB"), [9]);
        assert_eq!(swapped(&[(1, 0, vec![one_for_one(&[1], &[8]), one_for_one(&[1, 2], &[9, 9])])], "AB"), [8, 9]);
        // An extension subtable pointing to the real one.
        let far = [words(&[1, 1]), longs(&[8]), one_added(&[1], 1)].concat();
        assert_eq!(swapped(&[(7, 0, vec![far])], "A"), [2]);
        // A lookup ignoring some glyph class applies to the others: this one ignores marks. Of
        // the replacements a subtable offers, the first is taken; an unknown subtable type does
        // nothing.
        assert_eq!(swapped(&[(1, 8, vec![one_added(&[1], 1)])], "A"), [2]);
        assert_eq!(swapped(&[(3, 0, vec![several_for_one(&[1], &[&[5, 6]])]), (9, 0, vec![one_added(&[5], 1)])], "A"), [5]);
        // Only the default features count: here one, and one for small caps.
        let lookups = [(1, 0, vec![one_added(&[1], 1)]), (1, 0, vec![one_added(&[3], 5)])];
        let font = read(&coloured(&[(b"GSUB", sequences(&[(b"smcp", &[0]), (b"liga", &[1])], &lookups))]));
        let mut glyphs = Vec::new();
        assert_eq!((font.set("AC", &mut glyphs), glyphs.iter().map(|set| set.glyph).collect::<Vec<_>>()), (Some(448.0 + 384.0), vec![1, 8]));
        // A font without colour glyphs isn't asked about sequences at all.
        let plain = read(&[tables(), vec![(*b"GSUB", sequences(&[(b"ccmp", &[0])], &lookups))]].concat());
        assert_eq!((plain.set("A", &mut glyphs), glyphs[0].glyph), (Some(448.0), 1));
    }

    #[test]
    fn a_rule_holds_for_the_glyphs_it_names_and_has_lookups_carried_out_at_them() {
        // Lookups 1 to 4 are only called by rules: "B" becomes "G", "C" becomes "H", "B" and
        // "C" become one "F", and "E" is deleted.
        let called = [(1, 0, vec![one_added(&[2], 6)]), (1, 0, vec![one_added(&[3, 5], 6)]), (4, 0, vec![one_for_several(&[2], &[&[(7, &[3])]])]), (2, 0, vec![several_for_one(&[6], &[&[]])])];
        // The font's feature lists only its first lookup, the rules.
        let ruled = |rules: (i32, Vec<u8>), called: &[(i32, i32, Vec<Vec<u8>>)], text: &str| {
            let font = read(&coloured(&[(b"GSUB", sequences(&[(b"ccmp", &[0])], &[&[(rules.0, 0, vec![rules.1])][..], called].concat()))]));
            let mut glyphs = Vec::new();
            font.set(text, &mut glyphs).expect("the font has the characters");
            glyphs.iter().map(|set| set.glyph).collect::<Vec<_>>()
        };
        let with = |rules: (i32, Vec<u8>), text: &str| ruled(rules, &called, text);
        // One chained rule, by backtrack, input and lookahead: "B" and "C" after an "A" and
        // before a "G" each become another glyph.
        let rule = (6, one_rule(&[&[1]], &[&[2], &[3]], &[&[8]], &[[0, 1], [1, 2]]));
        assert_eq!(["ABCG", "ABC", "BCG", "ABCGABCG", "ACBG"].map(|text| with(rule.clone(), text)), [vec![1, 8, 9, 8], vec![1, 2, 3], vec![2, 3, 8], vec![1, 8, 9, 8, 1, 8, 9, 8], vec![1, 3, 2, 8]]);
        // Glyphs follow a rule's lookups with their sequence indices: the rule's third glyph is
        // its second once the first two ligate.
        assert_eq!(with((6, one_rule(&[], &[&[2], &[3], &[5]], &[], &[[0, 3], [1, 2]])), "ABCD"), [1, 7, 11]);
        // The same with no backtrack or lookahead, as a context subtable.
        assert_eq!(with((5, words(&[3, 2, 1, 14, 20, 1, 2, 1, 1, 2, 1, 1, 3])), "ABCB"), [1, 2, 9, 2]);
        // Class-based rule sets: every "E" after an "A" or "B" is deleted, however many.
        let (before, of) = (classed(&[[1, 2, 1]]), classed(&[[6, 6, 2]]));
        let going = sets_of_rules(2, &[6], &[before, of.clone(), of], &[&[], &[], &[&[1, 1, 1, 0, 1, 0, 4]]]);
        assert_eq!([with((6, going.clone()), "AEEEC"), with((6, going.clone()), "CEA"), with((6, going), "BEAE")], [vec![1, 3], vec![3, 6, 1], vec![2, 1]]);
        // Two of the first three letters in a row become one glyph: the first replaced, the
        // second deleted. (Glyph-based rules.)
        let gone = [(1, 0, vec![one_for_one(&[1, 2, 3], &[9, 9, 9])]), (2, 0, vec![several_for_one(&[1, 2, 3], &[&[], &[], &[]])])];
        let pairs = sets_of_rules(2, &[1, 2, 3], &[classed(&[[1, 3, 1]])], &[&[], &[&[2, 2, 1, 0, 1, 1, 2]]]);
        let pairs = |text: &str| ruled((5, pairs.clone()), &gone, text);
        assert_eq!([pairs("AB"), pairs("ABC"), pairs("ABCA"), pairs("AD")], [vec![9], vec![9, 3], vec![9, 9], vec![1, 5]]);
        // Glyph-based rule sets: after "A", a "B" before "C" becomes "G".
        assert_eq!(with((6, sets_of_rules(1, &[2], &[], &[&[&[1, 1, 1, 1, 3, 1, 0, 1]]])), "ABCAB"), [1, 8, 3, 1, 2]);
        assert_eq!(with((5, sets_of_rules(1, &[2], &[], &[&[&[2, 1, 3, 1, 2]]])), "BCBB"), [2, 9, 2, 2]);
        // A rule calling rules isn't followed, and a rule for no glyphs never matches.
        assert_eq!(with((6, one_rule(&[], &[&[1]], &[], &[[0, 0]])), "AB"), [1, 2]);
        assert_eq!(with((6, one_rule(&[&[1]], &[], &[], &[[0, 1]])), "AB"), [1, 2]);
    }

    #[test]
    fn glyphs_are_moved_to_where_they_belong_together() {
        // "A" is 448 wide and "B" 576. Moved 10 right and 20 up, the pen advancing 30 more; or
        // each glyph by its own values, which may be fewer.
        let each = |moves: &[(i32, i32, Vec<Vec<u8>>)], text: &str| set(&[], moves, text).expect("the font has the characters");
        assert_eq!(each(&[], "AB"), (vec![(1, [0.0, 0.0]), (2, [448.0, 0.0])], 1024.0));
        assert_eq!(each(&[(1, 0, vec![moving(&[1], 7, &[10, 20, 30])])], "AB"), (vec![(1, [10.0, 20.0]), (2, [478.0, 0.0])], 1054.0));
        assert_eq!(each(&[(1, 0, vec![moving_each(&[1, 2], 5, &[10, 30, -8, -100])])], "ABA"), (vec![(1, [10.0, 0.0]), (2, [470.0, 0.0]), (1, [964.0, 0.0])], 1432.0));
        // By a rule: an "A" before a "B" doesn't advance the pen, so they overlap.
        let over = [(8, 0, vec![one_rule(&[], &[&[1], &[2]], &[], &[[0, 1]])]), (1, 0, vec![moving(&[1], 4, &[-448])])];
        let font = read(&coloured(&[(b"GPOS", sequences(&[(b"dist", &[0]), (b"kern", &[])], &over))]));
        let mut glyphs = Vec::new();
        assert_eq!((font.set("ABA", &mut glyphs), glyphs.iter().map(|set| set.at[0]).collect::<Vec<_>>()), (Some(576.0 + 448.0), vec![0.0, 0.0, 576.0]));
        // A mark (the "G", per `GDEF`) attaches to the previous non-mark glyph: its anchor
        // lands on that glyph's anchor for its class, and it doesn't advance the pen.
        let on_base = fixing(&[(8, 1, [20, 0])], &[(1, &[[0, 0], [500, 100]])]);
        assert_eq!(each(&[(4, 0, vec![on_base.clone()])], "AGB"), (vec![(1, [0.0, 0.0]), (8, [480.0, 100.0]), (2, [448.0, 0.0])], 1024.0));
        // Two marks: both attach to the "A"; then the second to the first, the mark before it.
        // Adjustments after attachment move a mark from there.
        let on_mark = fixing(&[(8, 0, [0, 0])], &[(8, &[[300, 0]])]);
        let family = [(4, 0, vec![on_base.clone()]), (1, 0, vec![moving(&[8], 1, &[-80])]), (6, 0, vec![on_mark])];
        assert_eq!(each(&family, "AGG"), (vec![(1, [0.0, 0.0]), (8, [400.0, 100.0]), (8, [700.0, 100.0])], 448.0));
        // A mark with no base, or one the subtable has no anchor for, stays at the pen; no mark
        // advances the pen.
        assert_eq!(each(&[(4, 0, vec![on_base])], "GBG"), (vec![(8, [0.0, 0.0]), (2, [0.0, 0.0]), (8, [576.0, 0.0])], 576.0));
        // Without a `GPOS` `kern` feature, `kern` applies: "A" and "B" are 128 closer, half
        // taken off each, so the second draws that much further left. ("B" and "A" are 64
        // apart.)
        let font = read(&coloured(&[(b"GPOS", sequences(&[(b"dist", &[])], &[]))]));
        assert_eq!((font.set("ABA", &mut glyphs), glyphs.iter().map(|set| set.at[0]).collect::<Vec<_>>()), (Some(448.0 + 576.0 + 448.0 - 64.0), vec![0.0, 320.0, 448.0 - 128.0 + 576.0 + 64.0]));
    }

    #[test]
    fn a_glyph_in_layers_is_outlines_each_filled_with_a_colour() {
        let font = read(&coloured(&[]));
        let layers = |font: &FontFile, glyph: u16| font.layers(glyph).map(|layers| layers.collect::<Vec<_>>());
        let colour = |n: usize| Some(Color(COLOURS[n]));
        assert_eq!(layers(&font, 1), Some(vec![(0, colour(0)), (8, colour(1))]));
        assert_eq!(layers(&font, 5), Some(vec![(1, colour(2)), (8, None)]), "the last number there is stands for the colour of the text");
        assert_eq!(layers(&font, 9), Some(vec![(0, colour(3))]));
        assert_eq!([0, 2, 8].map(|glyph| layers(&font, glyph)), [None, None, None], "a glyph with no layers is its own outline");
        assert!(font.pictured() && !read(&tables()).pictured());
        // A palette starting at the table's second colour: the colours after it, an index past
        // the last being no colour.
        let later = [words(&[0, 3, 1, 4]), longs(&[14]), words(&[1]), COLOURS.iter().flat_map(|colour| colour.swap_bytes().to_be_bytes()).collect()].concat();
        let font = read(&with(coloured(&[]), b"CPAL", later));
        assert_eq!((layers(&font, 1), layers(&font, 9)), (Some(vec![(0, colour(1)), (8, colour(2))]), Some(vec![(0, None)])));
        // Layers without colours are none, as is an empty list; the font is then read as a
        // plain one.
        let without: Tables = coloured(&[]).into_iter().filter(|(name, _)| name != b"CPAL").collect();
        let none = with(coloured(&[]), b"COLR", [words(&[0, 0]), longs(&[14, 14]), words(&[0])].concat());
        assert!(!read(&without).pictured() && !read(&none).pictured());
        // A layer's coverage: at 16 pixels the small box is two pixels square, a pixel right of
        // the pen, on the baseline. With the pen 3.5 pixels on and one up, it falls between two
        // pixels.
        assert_eq!(font.cover(8, font.unit(16.0), [0.0, 0.0]), ([1, -2], 2, vec![255; 4]));
        assert_eq!(font.cover(8, font.unit(16.0), [3.5, 1.0]), ([4, -3], 3, vec![127, 255, 127, 127, 255, 127]));
        assert_eq!((font.cover(4, 1.0, [0.0, 0.0]), font.cover(1, 1e4, [0.0, 0.0])), (([0; 2], 0, vec![]), ([0; 2], 0, vec![])), "nothing to draw, and too large to");
    }

    /// A one-colour 8 by 8 image, as a PNG.
    fn square(colour: u32) -> Vec<u8> {
        crate::png::encode_rgba(8, &[colour; 64]).unwrap()
    }

    /// An image as `CBDT` stores it, in one of three formats: after small (5 bytes) or big (8
    /// bytes) metrics (height, width, bearings, advance), or none; then its length and data.
    fn kept_as(kind: i32, file: &[u8], [left, top]: [i8; 2]) -> Vec<u8> {
        let measures = [8, 8, left as u8, top as u8, 9, 0, 0, 0];
        [&measures[..[5, 8, 0][kind as usize - 17]], &longs(&[file.len() as u32]), file].concat()
    }

    /// One `CBLC` index subtable's images: index format, image format, and each glyph with its
    /// file and bearings.
    type Listed<'a> = (i32, i32, &'a [(i32, Vec<u8>, [i8; 2])]);

    /// `CBLC` and `CBDT`: sizes, each with ppem, bit depth, and index subtables.
    fn in_sizes(sizes: &[(u8, u8, &[Listed])]) -> (Vec<u8>, Vec<u8>) {
        let (mut held, mut arrays) = (words(&[3, 0]), Vec::new());
        for (_, _, lists) in sizes {
            let (mut entries, mut bodies): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
            for (way, kind, pictures) in *lists {
                let mut each: Vec<Vec<u8>> = pictures.iter().map(|(_, file, at)| kept_as(*kind, file, *at)).collect();
                // Where a subtable says its images are all one size, they are.
                let long = each.iter().map(Vec::len).max().unwrap();
                each.iter_mut().filter(|_| matches!(way, 2 | 5)).for_each(|picture| picture.resize(long, 0));
                let starts: Vec<usize> = (0..=each.len()).map(|n| each[..n].concat().len()).collect();
                let (glyphs, at): (Vec<i32>, [i8; 2]) = (pictures.iter().map(|picture| picture.0).collect(), pictures[0].2);
                let measures = vec![8, 8, at[0] as u8, at[1] as u8, 9, 0, 0, 0];
                let body = match way {
                    1 => longs(&starts.iter().map(|start| *start as u32).collect::<Vec<_>>()),
                    3 => words(&starts.iter().map(|start| *start as i32).collect::<Vec<_>>()),
                    2 => [longs(&[long as u32]), measures].concat(),
                    4 => [longs(&[glyphs.len() as u32]), words(&glyphs.iter().chain(&[0]).zip(&starts).flat_map(|(glyph, start)| [*glyph, *start as i32]).collect::<Vec<_>>())].concat(),
                    _ => [longs(&[long as u32]), measures, longs(&[glyphs.len() as u32]), words(&glyphs)].concat(),
                };
                entries.push(words(&[glyphs[0], *glyphs.last().unwrap()]));
                bodies.push([words(&[*way, *kind]), longs(&[held.len() as u32]), body].concat());
                held.extend(each.concat());
            }
            // A size's index subtable array: each first and last glyph and offset, then the
            // subtables.
            let starts = (0..bodies.len()).map(|n| longs(&[(entries.len() * 8 + bodies[..n].concat().len()) as u32]));
            let leads: Vec<Vec<u8>> = entries.iter().zip(starts).map(|(entry, start)| [&entry[..], &start].concat()).collect();
            arrays.push([leads.concat(), bodies.concat()].concat());
        }
        let mut index = [words(&[3, 0]), longs(&[sizes.len() as u32])].concat();
        for (n, (pixels, bits, lists)) in sizes.iter().enumerate() {
            let at = 8 + sizes.len() * 48 + arrays[..n].concat().len();
            index.extend([longs(&[at as u32, arrays[n].len() as u32, lists.len() as u32, 0]), vec![0; 24], words(&[0, 9]), vec![*pixels, *pixels, *bits, 1]].concat());
        }
        index.extend(arrays.concat());
        (index, held)
    }

    /// A bitmap-only font's tables: no outlines, and these bitmap tables.
    pub(crate) fn pictures(more: &[(&[u8; 4], Vec<u8>)]) -> Tables {
        let mut tables: Tables = coloured(&[]).into_iter().filter(|(name, _)| ![b"glyf", b"loca", b"kern", b"COLR", b"CPAL"].contains(&name)).collect();
        tables.extend(more.iter().map(|(name, table)| (**name, table.clone())));
        tables
    }

    /// A font's bitmap of a glyph: its file, the size it was made for, and its bearings.
    fn ready(font: &FontFile, glyph: u16, pixels: f32) -> Option<(Vec<u8>, f32, [f32; 2])> {
        font.bitmap(glyph, pixels).map(|picture| (picture.png.to_vec(), picture.pixels, [picture.left, picture.top]))
    }

    #[test]
    fn ready_made_pictures_are_found_however_they_are_listed() {
        let (red, green, blue) = (square(0xffff_0000), square(0xff00_ff00), square(0x8000_00ff));
        // One size, with an index subtable in each of the five formats, and images in each of
        // the three formats: glyphs 1 and 2, then 3, then 5 to 7, then 8, then 9.
        let lists: [Listed; 5] = [
            (1, 17, &[(1, red.clone(), [1, 7]), (2, green.clone(), [0, 8])]),
            (2, 19, &[(3, blue.clone(), [2, 6])]),
            (3, 18, &[(5, red.clone(), [-1, 7]), (6, green.clone(), [1, 7]), (7, blue.clone(), [1, 9])]),
            (4, 17, &[(8, green.clone(), [3, 3])]),
            (5, 19, &[(9, red.clone(), [1, 5])]),
        ];
        let (index, held) = in_sizes(&[(16, 32, &lists)]);
        let font = read(&pictures(&[(b"CBLC", index), (b"CBDT", held)]));
        let expected = [
            (1, &red, [1.0, 7.0]),
            (2, &green, [0.0, 8.0]),
            (3, &blue, [2.0, 6.0]),
            (5, &red, [-1.0, 7.0]),
            (6, &green, [1.0, 7.0]),
            (7, &blue, [1.0, 9.0]),
            (8, &green, [3.0, 3.0]),
            (9, &red, [1.0, 5.0]),
        ];
        for (glyph, file, at) in expected {
            assert_eq!(ready(&font, glyph, 16.0), Some((file.clone(), 16.0, at)), "glyph {glyph}");
        }
        assert_eq!([0, 4].map(|glyph| ready(&font, glyph, 16.0)), [None, None], "glyphs that no list is for");
        // A bitmap-only font is a font: its glyphs have advances and no outlines; without
        // bitmaps either, it is none.
        assert!(font.pictured() && font.outline('A', 16.0) == Path::new() && font.advance('A', 1024.0) == 448.0);
        assert!(FontFile::read(file(&[0, 1, 0, 0], &pictures(&[]))).is_none());
        // Sparse subtables have nothing for the glyphs of their range they don't list.
        let sparse: [Listed; 2] = [(4, 17, &[(1, red.clone(), [0, 8]), (3, green.clone(), [0, 8])]), (5, 19, &[(5, blue.clone(), [0, 8]), (9, red.clone(), [0, 8])])];
        let (index, held) = in_sizes(&[(16, 32, &sparse)]);
        let font = read(&pictures(&[(b"CBLC", index), (b"CBDT", held)]));
        assert_eq!([1, 2, 3, 5, 7, 9].map(|glyph| ready(&font, glyph, 16.0).map(|found| found.0)), [Some(red.clone()), None, Some(green.clone()), Some(blue.clone()), None, Some(red.clone())]);
        // Of several sizes, the smallest large enough is taken, else the largest; one not in
        // full colour is skipped.
        let (small, large, grey): ([Listed; 1], [Listed; 1], [Listed; 1]) =
            ([(1, 17, &[(1, red.clone(), [0, 8])])], [(1, 17, &[(1, green.clone(), [0, 30])])], [(1, 17, &[(1, blue.clone(), [0, 60])])]);
        let (index, held) = in_sizes(&[(32, 32, &large), (16, 32, &small), (64, 8, &grey)]);
        let font = read(&pictures(&[(b"CBLC", index), (b"CBDT", held)]));
        assert_eq!([8.0, 16.0, 17.0, 32.0, 500.0].map(|pixels| ready(&font, 1, pixels).map(|found| found.1)), [Some(16.0), Some(16.0), Some(32.0), Some(32.0), Some(32.0)]);
    }

    /// An `sbix` glyph: its type tag (or none for no image), its bearings, and the data.
    type Graphic<'a> = (&'a [u8; 4], [i32; 2], Vec<u8>);

    /// `sbix`: strikes, each with its ppem and a glyph for every glyph from the first: type
    /// tag, bearings, data. (No tag: no image.)
    fn graphics(sizes: &[(i32, &[Graphic])]) -> Vec<u8> {
        let size = |(pixels, pictures): &(i32, &[Graphic])| {
            let each: Vec<Vec<u8>> = pictures.iter().map(|(kind, at, file)| if kind == &b"    " { vec![] } else { [&words(at)[..], &kind[..], file].concat() }).collect();
            let starts: Vec<u32> = (0..=each.len()).map(|n| (4 + (each.len() + 1) * 4 + each[..n].concat().len()) as u32).collect();
            [words(&[*pixels, 72]), longs(&starts), each.concat()].concat()
        };
        let sizes: Vec<Vec<u8>> = sizes.iter().map(size).collect();
        let starts: Vec<u32> = (0..sizes.len()).map(|n| (8 + sizes.len() * 4 + sizes[..n].concat().len()) as u32).collect();
        [words(&[1, 1]), longs(&[sizes.len() as u32]), longs(&starts), sizes.concat()].concat()
    }

    #[test]
    fn pictures_of_the_other_kind_are_found_too() {
        let (red, green) = (square(0xffff_0000), square(0xff00_ff00));
        // Glyph 0 has no image, glyph 1 a PNG, glyph 2 duplicates glyph 1, glyph 3 is an unread
        // type, and glyphs 4 and 5 duplicate each other.
        let small: [Graphic; 7] = [
            (b"    ", [0, 0], vec![]),
            (b"png ", [1, -2], red.clone()),
            (b"dupe", [0, 0], words(&[1])),
            (b"jpg ", [0, 0], vec![1, 2, 3]),
            (b"dupe", [0, 0], words(&[5])),
            (b"dupe", [0, 0], words(&[4])),
            (b"    ", [0, 0], vec![]),
        ];
        let large: [Graphic; 3] = [(b"    ", [0, 0], vec![]), (b"png ", [0, 0], green.clone()), (b"    ", [0, 0], vec![])];
        // This font has outlines, as fonts with this table do, and no layers.
        let outlined: Tables = coloured(&[(b"sbix", graphics(&[(16, &small), (32, &large)]))]).into_iter().filter(|(name, _)| ![b"COLR", b"CPAL"].contains(&name)).collect();
        let font = read(&outlined);
        // The image's bottom lies two pixels below the baseline, so its top six above.
        assert_eq!(ready(&font, 1, 16.0), Some((red.clone(), 16.0, [1.0, 6.0])));
        assert_eq!(ready(&font, 2, 12.0), Some((red, 16.0, [1.0, 6.0])));
        assert_eq!(ready(&font, 1, 20.0), Some((green, 32.0, [0.0, 8.0])));
        assert_eq!([0, 3, 4, 5].map(|glyph| ready(&font, glyph, 16.0)), [None, None, None, None]);
        assert!(font.pictured() && font.layers(1).is_none() && font.outline('A', 16.0) != Path::new());
    }

    /// An emoji font with everything read of one: layers, bitmaps of both kinds, and lookups of
    /// every type for sequences.
    fn of_everything() -> Vec<u8> {
        let swaps = [
            (4, 0, vec![one_for_several(&[1], &[&[(9, &[4, 2]), (5, &[2])]])]),
            (6, 0, vec![one_rule(&[&[1]], &[&[2], &[3]], &[&[8]], &[[0, 2], [1, 3]])]),
            (1, 0, vec![one_added(&[2], 6), one_for_one(&[3], &[9])]),
            (2, 0, vec![several_for_one(&[3], &[&[1, 1]])]),
            (5, 0, vec![sets_of_rules(2, &[1, 2, 3], &[classed(&[[1, 3, 1]])], &[&[], &[&[2, 2, 1, 0, 2, 1, 3]]])]),
            (6, 0, vec![sets_of_rules(1, &[2], &[], &[&[&[1, 1, 1, 1, 3, 1, 0, 2]]])]),
        ];
        let moves = [
            (8, 0, vec![one_rule(&[], &[&[1], &[2]], &[], &[[0, 1]])]),
            (1, 0, vec![moving(&[1], 4, &[-448]), moving_each(&[2, 3], 7, &[1, 2, 3, 4, 5, 6])]),
            (4, 0, vec![fixing(&[(8, 0, [20, 0])], &[(1, &[[500, 100]])])]),
            (6, 0, vec![fixing(&[(8, 0, [0, 0])], &[(8, &[[300, 0]])])]),
        ];
        let lists: [Listed; 3] = [(1, 17, &[(2, square(0xffff_0000), [1, 7])]), (5, 19, &[(3, square(0xff00_ff00), [0, 8])]), (4, 18, &[(7, square(0xff00_00ff), [0, 8])])];
        let ((index, held), kinds) = (in_sizes(&[(16, 32, &lists)]), [words(&[1, 0, 12, 0, 0, 0]), classed(&[[8, 8, 3]])].concat());
        let other: [Graphic; 4] = [(b"    ", [0, 0], vec![]), (b"png ", [1, -2], square(0xffff_ffff)), (b"dupe", [0, 0], words(&[1])), (b"    ", [0, 0], vec![])];
        let tables = coloured(&[
            (b"GSUB", sequences(&[(b"ccmp", &[0, 1, 4, 5])], &swaps)),
            (b"GPOS", sequences(&[(b"dist", &[0]), (b"mark", &[2]), (b"mkmk", &[3])], &moves)),
            (b"GDEF", kinds),
            (b"CBLC", index),
            (b"CBDT", held),
            (b"sbix", graphics(&[(16, &other)])),
        ]);
        file(&[0, 1, 0, 0], &tables)
    }

    /// A small layered emoji font, for testing text drawn with one. At 16 pixels a unit is 1/64
    /// pixel, so its shapes lie on whole pixels. A face (also "A", a man, the first regional
    /// indicator) is 7 wide: a yellow box 3 wide and 7 high, a pixel right of the pen, under a
    /// black 2 square. A hand, a child, a heart and the digit 1 are the small box alone, with
    /// no colour of their own, 6 wide. A flag of the first two regional indicators, and a man
    /// joined to a child, are one glyph 10 wide: a red 6 square with a hole, under the small
    /// box in the text colour. And a hand with a skin tone, or a keycap, is one glyph 6 wide:
    /// the 3 by 7 box in half-transparent blue.
    pub(crate) fn emoji_font() -> FontFile {
        let joined = one_for_several(&[1, 8], &[&[(5, &[4, 8]), (5, &[2])], &[(9, &[7])]]);
        read(&coloured(&[(b"GSUB", sequences(&[(b"ccmp", &[0])], &[(4, 0, vec![joined])]))]))
    }

    /// A small bitmap font, made for 32-pixel text: a face is a red 8-pixel square, a pixel
    /// right of the pen, its top `top` above the baseline.
    pub(crate) fn picture_font(top: i8) -> FontFile {
        let lists: [Listed; 1] = [(1, 17, &[(1, square(0xffff_0000), [1, top])])];
        let (index, held) = in_sizes(&[(32, 32, &lists)]);
        read(&pictures(&[(b"CBLC", index), (b"CBDT", held)]))
    }

    /// Everything an emoji font can be asked, asked of whatever these bytes are.
    fn ask_for_pictures(bytes: Vec<u8>) {
        let Some(font) = FontFile::read(bytes) else { return };
        let mut glyphs = Vec::new();
        for text in ["A\u{200d}B", "ABCG", "ABC", "AGG", "\u{1f1e6}\u{1f1e7}", "1\u{fe0f}\u{20e3}", "\u{1f44d}\u{1f3fd}"] {
            let _ = font.set(text, &mut glyphs);
        }
        for glyph in 0..12 {
            let _ = (font.layers(glyph).map(|layers| layers.count()), font.bitmap(glyph, 16.0).map(|picture| picture.png.len()), font.cover(glyph, 0.25, [1.0, 2.0]));
        }
        let mut emoji = crate::emoji::Emoji::around(Box::new(crate::text::BuiltInFont)).with(font);
        let (mut px, mut stops, text) = (vec![0u32; 64 * 32], Vec::new(), "A\u{1f600}B\u{1f1e6}\u{1f1e7}\u{1f44d}\u{1f3fd}");
        let wide = emoji.measure(text, Font::new(16.0), 1.5, Some(&mut stops));
        assert_eq!((stops.len(), stops.last().copied()), (text.chars().count(), Some(wide)));
        emoji.draw(&mut Canvas::new(&mut px, 64, 32, (0, 0), 1.5), text, 1.0, 1.0, Font::new(16.0), Color::WHITE);
    }

    #[test]
    fn a_damaged_font_of_emoji_never_stops_the_program() {
        let whole = of_everything();
        // Whole, it is a font with all of these.
        let font = FontFile::read(whole.clone()).expect("the font is read");
        let mut glyphs = Vec::new();
        assert_eq!((font.set("A\u{200d}B", &mut glyphs), glyphs.len(), font.layers(1).is_some(), font.bitmap(2, 16.0).is_some()), (Some(384.0), 1, true, true));
        (0..whole.len()).step_by(3).for_each(|long| ask_for_pictures(whole[..long].to_vec()));
        for (at, change) in (0..whole.len()).flat_map(|at| [(at, 0), (at, 255), (at, whole[at] ^ 1 << (at % 8))]) {
            let mut damaged = whole.clone();
            damaged[at] = change;
            ask_for_pictures(damaged);
        }
    }

    #[test]
    fn a_font_of_emoji_is_the_same_packed_for_the_web_or_opened_where_it_lies() {
        let whole = of_everything();
        let seen = |font: &FontFile| {
            let mut glyphs = Vec::new();
            let pens = ["A\u{200d}B", "ABCG", "AGG"].map(|text| (font.set(text, &mut glyphs), glyphs.clone()));
            let layers: Vec<_> = (0..10).map(|glyph| font.layers(glyph).map(|layers| layers.collect::<Vec<_>>())).collect();
            let files: Vec<_> = (0..10).map(|glyph| font.bitmap(glyph, 16.0).map(|picture| picture.png.to_vec())).collect();
            (pens, layers, files, "ABCDH".chars().map(|letter| font.outline(letter, 64.0)).collect::<Vec<_>>())
        };
        let font = FontFile::read(whole.clone()).expect("the font is read");
        // From WOFF, its bitmap and sequence tables are decompressed too; of a font without
        // colour glyphs, its `GSUB` is, and tables not read here (`post`) aren't.
        let packed = FontFile::read(for_the_web(&whole).0).expect("the packed font is read");
        assert!(seen(&packed) == seen(&font));
        let plain = file(&[0, 1, 0, 0], &[tables(), vec![(*b"GSUB", vec![0; 4000]), (*b"post", vec![0; 4000])]].concat());
        let (unpacked, as_it_was) = (FontFile::read(for_the_web(&plain).0).expect("the packed font is read"), FontFile::read(plain).expect("the font is read"));
        assert_eq!(unpacked.table(b"GSUB").0.len(), 4000);
        assert!(unpacked.file.len() + 4000 <= as_it_was.file.len(), "{} of {}", unpacked.file.len(), as_it_was.file.len());
        // Opened in place, it isn't read into memory (where mapping is supported), and is the
        // same font; so is a WOFF one, which must be read.
        let folder = std::env::temp_dir().join(format!("taut-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for (name, bytes) in [("whole.ttf", whole.clone()), ("packed.woff", for_the_web(&whole).0)] {
            std::fs::write(folder.join(name), bytes).unwrap();
            let opened = FontFile::open(folder.join(name)).expect("the font is opened");
            assert!(seen(&opened) == seen(&font), "{name}");
            // On Linux, a user-writable file is read anyway: see `Lying::of`.
            let lying = matches!(opened.file, Held::Lying(_));
            assert_eq!(lying, name == "whole.ttf" && cfg!(windows), "{name}");
        }
        assert!(FontFile::open(folder.join("none.ttf")).is_none());
        std::fs::write(folder.join("empty.ttf"), []).unwrap();
        assert!(FontFile::open(folder.join("empty.ttf")).is_none());
        std::fs::remove_dir_all(&folder).unwrap();
        // A system font is mapped on Linux too; unless the user can write it (some CI images ship fonts as 0777).
        #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
        for path in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf"].iter().filter(|path| std::path::Path::new(path).exists()) {
            let untouchable = Lying::untouchable(&std::fs::File::open(path).unwrap());
            assert!(FontFile::open(path).is_some_and(|font| matches!(font.file, Held::Lying(_)) == untouchable), "{path}");
        }
    }

    /// A font file another program truncates while the font is in use (as `cp` does first when
    /// overwriting) still reads as it was: on Linux, where reading a mapped file past its new
    /// end would kill the program, a user-writable file is read into memory instead.
    #[test]
    fn a_font_file_cut_short_meanwhile_is_still_read() {
        let folder = std::env::temp_dir().join(format!("taut-cut-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let copy = folder.join("font.ttf");
        std::fs::write(&copy, file(&[0, 1, 0, 0], &tables())).unwrap();
        let font = FontFile::open(&copy).expect("the font opens");
        let wide = font.advance('B', 16.0);
        // On Windows, a mapped file cannot be truncated at all.
        if std::fs::File::create(&copy).is_ok() {
            assert_eq!(font.advance('B', 16.0), wide);
        }
        drop(font);
        // And so is a copy of a real system font.
        #[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
        for path in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf"].iter().filter(|path| std::path::Path::new(path).exists()) {
            std::fs::copy(path, &copy).unwrap();
            let font = FontFile::open(&copy).expect("the font opens");
            let wide = font.advance('W', 16.0);
            drop(std::fs::File::create(&copy).unwrap());
            assert_eq!(font.advance('W', 16.0), wide, "{path}");
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Emoji sequences of every kind: faces and hands with skin tones, keycaps, regional
    /// indicator and tag flags, families and couples of every combination, people with
    /// professions, genders, hair styles or directions, and some no font has a glyph for.
    #[cfg(target_os = "linux")]
    const SEQUENCES: [&str; 44] = [
        "\u{1f600}",
        "\u{1f44d}\u{1f3fd}",
        "\u{261d}\u{1f3fb}",
        "\u{270c}\u{fe0f}",
        "\u{2764}\u{fe0f}",
        "\u{a9}\u{fe0f}",
        "1\u{fe0f}\u{20e3}",
        "#\u{fe0f}\u{20e3}",
        "*\u{20e3}",
        "\u{1f1e9}\u{1f1ea}",
        "\u{1f1ef}\u{1f1f5}",
        "\u{1f1fa}\u{1f1f3}",
        "\u{1f1fd}\u{1f1fd}",
        "\u{1f1e9}",
        "\u{1f3f4}\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
        "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}",
        "\u{1f3f4}\u{e0067}\u{e0062}\u{e0077}\u{e006c}\u{e0073}\u{e007f}",
        "\u{1f3f4}\u{e0075}\u{e0073}\u{e0074}\u{e0078}\u{e007f}",
        "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}",
        "\u{1f3f3}\u{fe0f}\u{200d}\u{26a7}\u{fe0f}",
        "\u{1f3f4}\u{200d}\u{2620}\u{fe0f}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f466}",
        "\u{1f469}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f467}",
        "\u{1f468}\u{200d}\u{1f467}",
        "\u{1f469}\u{200d}\u{1f466}\u{200d}\u{1f466}",
        "\u{1f468}\u{1f3fb}\u{200d}\u{1f469}\u{1f3ff}\u{200d}\u{1f467}\u{1f3fd}",
        "\u{1f9d1}\u{200d}\u{1f9d1}\u{200d}\u{1f9d2}",
        "\u{1f469}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f468}",
        "\u{1f469}\u{1f3fb}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f468}\u{1f3ff}",
        "\u{1f468}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f48b}\u{200d}\u{1f468}",
        "\u{1f469}\u{1f3fd}\u{200d}\u{2764}\u{fe0f}\u{200d}\u{1f48b}\u{200d}\u{1f469}\u{1f3fb}",
        "\u{1f9d1}\u{1f3fb}\u{200d}\u{1f91d}\u{200d}\u{1f9d1}\u{1f3ff}",
        "\u{1f469}\u{1f3fc}\u{200d}\u{1f91d}\u{200d}\u{1f468}\u{1f3fe}",
        "\u{1faf1}\u{1f3fb}\u{200d}\u{1faf2}\u{1f3ff}",
        "\u{1f469}\u{200d}\u{1f680}",
        "\u{1f468}\u{1f3fd}\u{200d}\u{1f373}",
        "\u{1f3c3}\u{200d}\u{2640}\u{fe0f}",
        "\u{1f3c3}\u{1f3fd}\u{200d}\u{2640}\u{fe0f}\u{200d}\u{27a1}\u{fe0f}",
        "\u{1f469}\u{200d}\u{1f9b0}",
        "\u{1f441}\u{fe0f}\u{200d}\u{1f5e8}\u{fe0f}",
        "\u{2764}\u{fe0f}\u{200d}\u{1f525}",
        "\u{1f600}\u{1f3fd}",
        "\u{1f469}\u{200d}\u{1f408}",
    ];

    /// What the system's HarfBuzz makes of a sequence in the font at `path`: each glyph with
    /// its position, and the pen's total advance, in font units. `None` without HarfBuzz.
    #[cfg(target_os = "linux")]
    fn shaped(path: &std::path::Path, sequences: &[String]) -> Option<Vec<(Vec<Placed>, f32)>> {
        use std::ffi::{CString, c_char, c_int, c_uint, c_void};
        unsafe extern "C" {
            fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
            fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        // HarfBuzz's glyph info and position: five 32-bit values each.
        type Told = [i32; 5];
        // SAFETY: the library is opened by name and its functions looked up by name; each is
        // called as HarfBuzz declares it, with objects it created, which live until the program
        // ends.
        unsafe {
            let library = dlopen(c"libharfbuzz.so.0".as_ptr(), 2);
            let function = |name: &std::ffi::CStr| Some(dlsym(library, name.as_ptr())).filter(|found| !library.is_null() && !found.is_null());
            let from_file: extern "C" fn(*const c_char) -> *mut c_void = std::mem::transmute(function(c"hb_blob_create_from_file")?);
            let face_of: extern "C" fn(*mut c_void, c_uint) -> *mut c_void = std::mem::transmute(function(c"hb_face_create")?);
            let font_of: extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(function(c"hb_font_create")?);
            let buffer: extern "C" fn() -> *mut c_void = std::mem::transmute(function(c"hb_buffer_create")?);
            let add: extern "C" fn(*mut c_void, *const c_char, c_int, c_uint, c_int) = std::mem::transmute(function(c"hb_buffer_add_utf8")?);
            let guess: extern "C" fn(*mut c_void) = std::mem::transmute(function(c"hb_buffer_guess_segment_properties")?);
            let flags: extern "C" fn(*mut c_void, c_uint) = std::mem::transmute(function(c"hb_buffer_set_flags")?);
            let shape: extern "C" fn(*mut c_void, *mut c_void, *const c_void, c_uint) = std::mem::transmute(function(c"hb_shape")?);
            let glyphs: extern "C" fn(*mut c_void, *mut c_uint) -> *const Told = std::mem::transmute(function(c"hb_buffer_get_glyph_infos")?);
            let places: extern "C" fn(*mut c_void, *mut c_uint) -> *const Told = std::mem::transmute(function(c"hb_buffer_get_glyph_positions")?);
            let font = font_of(face_of(from_file(CString::new(path.to_str()?).ok()?.as_ptr()), 0));
            let one = |sequence: &String| {
                // Invisible characters are removed from the output:
                // HB_BUFFER_FLAG_REMOVE_DEFAULT_IGNORABLES (8).
                let (written, mut count) = (buffer(), 0);
                add(written, sequence.as_ptr().cast(), sequence.len() as c_int, 0, sequence.len() as c_int);
                guess(written);
                flags(written, 8);
                shape(font, written, std::ptr::null(), 0);
                let (glyphs, places) = (glyphs(written, &mut count), places(written, &mut count));
                let (glyphs, places) = (std::slice::from_raw_parts(glyphs, count as usize), std::slice::from_raw_parts(places, count as usize));
                let (mut pen, mut placed) = (0, Vec::new());
                for (glyph, place) in glyphs.iter().zip(places) {
                    placed.push((glyph[0] as u16, [(pen + place[2]) as f32, place[3] as f32]));
                    pen += place[0];
                }
                (placed, pen as f32)
            };
            Some(sequences.iter().map(one).collect())
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn sequences_come_to_the_glyphs_harfbuzz_makes_of_them() {
        // The emoji fonts there may be: this system's, that of a Windows this Linux runs under,
        // and a named one. With `TAUT_SEQUENCES` naming Unicode's `emoji-test.txt`, all its
        // sequences.
        let named = std::env::var_os("TAUT_EMOJI").map(std::path::PathBuf::from);
        let fonts = ["/usr/share/fonts/noto/NotoColorEmoji.ttf", "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf", "/mnt/c/Windows/Fonts/seguiemj.ttf"].map(std::path::PathBuf::from);
        let listed = std::env::var_os("TAUT_SEQUENCES").and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
        let written = |line: &str| line.split(';').next()?.split_whitespace().map(|number| u32::from_str_radix(number, 16).ok().and_then(char::from_u32)).collect::<Option<String>>();
        let listed = listed.lines().filter(|line| !line.starts_with('#')).filter_map(written).filter(|sequence| !sequence.is_empty());
        let sequences: Vec<String> = SEQUENCES.iter().map(|sequence| sequence.to_string()).chain(listed).collect();
        for path in fonts.into_iter().chain(named).filter(|path| path.exists()) {
            let Some(theirs) = shaped(&path, &sequences) else { return eprintln!("no HarfBuzz on this system: not tested") };
            let (font, mut glyphs, mut differ) = (FontFile::open(&path).expect("the font is read"), Vec::new(), Vec::new());
            for (sequence, theirs) in sequences.iter().zip(theirs) {
                // A sequence with a character the font lacks isn't set with it at all; HarfBuzz
                // then has the font's notdef glyph in it.
                let Some(pen) = font.set(sequence, &mut glyphs) else {
                    assert!(theirs.0.iter().any(|(glyph, _)| *glyph == 0), "{}: {sequence:?} is lacking here and not there: {theirs:?}", path.display());
                    continue;
                };
                let ours = (glyphs.iter().map(|set| (set.glyph, set.at)).collect::<Vec<_>>(), pen);
                differ.extend((ours != theirs).then(|| format!("{sequence:?} ({}): {ours:?}, not {theirs:?}", sequence.escape_unicode())));
            }
            println!("{}: {} sequences, {} differ", path.display(), sequences.len(), differ.len());
            assert!(differ.is_empty(), "{}:\n{}", path.display(), differ[..differ.len().min(12)].join("\n"));
        }
    }
}
